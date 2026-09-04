//! Windows Bluetooth glue: scan, pair (legacy), enable HID service, forget.
//!
//! This module is the only place that talks to `BluetoothAPIs.dll` /
//! `bthprops.cpl`.
//!
//! # Pairing strategy
//!
//! BBC v1.5.2 (through 32feet.NET `InTheHand.Net.Personal.dll`, reverse
//! engineered from the DLLs shipped next to `BalanceBoardApp.exe`) pairs
//! the board on this machine with the following Win32 sequence:
//!
//! 1. Compute the SYNC/bonding PIN from the **local radio's** MAC (see
//!    [`crate::pin`]).
//! 2. Register a per-device authentication handler with
//!    `BluetoothRegisterForAuthenticationEx` and keep it alive.
//! 3. Call the *legacy* `BluetoothAuthenticateDevice(NULL, NULL, &info,
//!    NULL /*pszPin*/, 0)`. Passing a null passkey makes Windows run a
//!    legacy PIN exchange and deliver the PIN-request to our registered
//!    callback, which answers with the computed PIN via
//!    `BluetoothSendAuthenticationResponseEx`.
//! 4. Enable the HID service with `BluetoothSetServiceState`, then wait
//!    for Windows to finish installing the device.
//!
//! Earlier revisions of this crate instead initiated with
//! `BluetoothAuthenticateDeviceEx` plus `MITMProtectionNotRequiredBonding`
//! and sent the **board's own** MAC as the PIN. On real hardware that flow
//! reached the auth callback (`authMethod == LEGACY`) and then stalled
//! forever: no "Send returned" line, board LED eventually off. WiiFitToVRC
//! independently reports the same "PIN + PairRequest via the Ex path fails
//! or hangs on generic Windows Bluetooth drivers". We therefore mirror the
//! proven BBC/32feet sequence: host-MAC PIN, Ex-registered callback, legacy
//! `BluetoothAuthenticateDevice` to initiate.
//!
//! # Threading
//!
//! Windows invokes the Ex authentication callback on its own thread; the
//! pairing call itself blocks until the exchange finishes. A message pump
//! is *not* required for the callback to fire — it was observed firing in
//! this plain console process. 32feet runs the same blocking call from a
//! GUI thread without a dedicated pump for the callback.
//!
//! # Registration lifetime
//!
//! Like 32feet (which constructs a `BluetoothWin32Authentication` and
//! never disposes it before the next pairing attempt), the registration
//! and its context are intentionally kept alive for the rest of the
//! process. This CLI exits right after pairing, so there is nothing to
//! clean up; a long-lived host should release the registration with
//! `BluetoothUnregisterAuthenticationEx` (not exposed by `windows-sys`
//! 0.61) once the exchange completes.

#![cfg(windows)]
#![allow(non_snake_case)]

use std::ffi::c_void;
use std::io;
use std::mem;
use std::ptr;
use std::thread;
use std::time::Duration;

use windows_sys::Win32::Devices::Bluetooth::*;
use windows_sys::Win32::Foundation::*;

use crate::pin::{format_pin, PinEncoding};

/// One Wii-family device returned by [`scan`].
#[derive(Debug, Clone)]
pub struct WiiDevice {
    /// 6-byte Bluetooth address in Win32 `rgBytes` order (little-endian).
    pub address: [u8; 6],
    /// Friendly name from the device, e.g. `Nintendo RVL-WBC-01`.
    pub name: String,
    /// `true` when Windows considers the device already paired.
    pub authenticated: bool,
    /// `true` when Windows is currently connected to the device.
    pub connected: bool,
    /// `true` when Windows has the device in its known/remembered list.
    pub remembered: bool,
}

impl WiiDevice {
    /// Is this specifically a Balance Board (vs. a Wiimote)?
    #[must_use]
    pub fn is_balance_board(&self) -> bool {
        self.name.starts_with("Nintendo RVL-WBC-01")
    }
}

const WII_NAME_PREFIXES: &[&str] = &[
    "Nintendo RVL-WBC-01", // Balance Board
    "Nintendo RVL-CNT-01", // Wiimote / Wiimote Plus
];

fn is_wii_name(name: &str) -> bool {
    WII_NAME_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// Post-pair device state as reported by the Windows Bluetooth cache.
#[derive(Debug, Clone, Copy, Default)]
pub struct DeviceState {
    /// `fAuthenticated` after the pairing attempt.
    pub authenticated: bool,
    /// `fRemembered` after the pairing attempt (device present in the
    /// Windows pairing database / Settings list).
    pub remembered: bool,
    /// `fConnected` after the pairing attempt.
    pub connected: bool,
}

/// Scan for nearby Wii-family Bluetooth devices, including ones that are
/// already paired. Issues a fresh inquiry; the SYNC button on the board
/// must be active for an unpaired board to respond.
///
/// `timeout` is rounded up to the nearest 1.28-second unit (Windows'
/// inquiry quantum); minimum 1 unit, maximum 48 (~61 s).
pub fn scan(timeout: Duration) -> io::Result<Vec<WiiDevice>> {
    let timeout_units = ((timeout.as_secs_f32() / 1.28).ceil() as u8).clamp(1, 48);

    let mut params: BLUETOOTH_DEVICE_SEARCH_PARAMS = unsafe { mem::zeroed() };
    params.dwSize = mem::size_of::<BLUETOOTH_DEVICE_SEARCH_PARAMS>() as u32;
    params.fReturnAuthenticated = 1;
    params.fReturnRemembered = 1;
    params.fReturnUnknown = 1;
    params.fReturnConnected = 1;
    params.fIssueInquiry = 1;
    params.cTimeoutMultiplier = timeout_units;
    params.hRadio = ptr::null_mut();

    let mut info: BLUETOOTH_DEVICE_INFO = unsafe { mem::zeroed() };
    info.dwSize = mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32;

    let find = unsafe { BluetoothFindFirstDevice(&params, &mut info) };
    if find.is_null() {
        let err = unsafe { GetLastError() };
        if err == ERROR_NO_MORE_ITEMS {
            return Ok(Vec::new());
        }
        return Err(io::Error::from_raw_os_error(err as i32));
    }

    let mut found = Vec::new();
    loop {
        let device = device_from_info(&info);
        if is_wii_name(&device.name) {
            found.push(device);
        }
        // Reset for the next iteration; dwSize must be set again.
        info = unsafe { mem::zeroed() };
        info.dwSize = mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32;
        let ok = unsafe { BluetoothFindNextDevice(find, &mut info) };
        if ok == 0 {
            break;
        }
    }

    unsafe { BluetoothFindDeviceClose(find) };
    Ok(found)
}

/// Outcome of a [`pair_first`] call.
#[derive(Debug, Clone)]
pub struct PairResult {
    /// The device we paired with.
    pub address: [u8; 6],
    /// Its friendly name.
    pub name: String,
    /// `true` if the device was already authenticated and we skipped the
    /// pairing handshake (only HID service was (re)enabled).
    pub already_paired: bool,
    /// Windows cache state observed shortly after the pairing attempt.
    pub post: DeviceState,
}

/// Find the first Balance Board nearby, pair it (if not already paired),
/// enable its HID service, and report the post-pair Windows state.
///
/// This mirrors the BBC v1.5.2 / 32feet sequence described in the module
/// docs: SYNC-bonding PIN derived from the **host** radio MAC, auth
/// answered through an Ex-registered callback, pairing initiated with the
/// legacy `BluetoothAuthenticateDevice` API.
pub fn pair_first(
    timeout: Duration,
    max_rounds: u32,
    encoding: PinEncoding,
) -> io::Result<PairResult> {
    for round in 1..=max_rounds {
        eprintln!(
            "[pair] discovery round {round}/{max_rounds}: scanning {:.0}s — press the red SYNC button on the board now if you have not yet.",
            timeout.as_secs_f32()
        );
        let devices = scan(timeout)?;
        if let Some(board) = devices.into_iter().find(WiiDevice::is_balance_board) {
            return pair_one(&board, encoding);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("No Balance Board found after {max_rounds} discovery rounds. Press SYNC inside the battery cover and re-run."),
    ))
}

/// Pair one already-discovered board using the BBC/32feet-equivalent flow.
fn pair_one(board: &WiiDevice, encoding: PinEncoding) -> io::Result<PairResult> {
    let radio = LocalRadio::open()?;
    eprintln!(
        "[pair] host (local) radio MAC: {} — SYNC/bonding PIN is derived from this, not the board",
        crate::pin::format_bd_addr(radio.address)
    );
    eprintln!(
        "[pair] PIN mode: {encoding:?}; bytes to send: {}",
        format_pin(&encoding.to_pin_bytes(radio.address))
    );

    // Use the discovery-cache `fAuthenticated` flag (the flag on a fresh
    // `info_for_address` is always zero, which is why the old code always
    // attempted a fresh pairing even for an already-paired board).
    let already_paired = board.authenticated;
    let mut info = info_for_address(board.address);

    if !already_paired {
        authenticate(&radio, &mut info, encoding)?;
    } else {
        eprintln!("[pair] board is already authenticated; skipping the pairing handshake.");
    }

    enable_hid_service(&radio, &info)?;

    // Give Windows a moment to finish installing / updating the device,
    // then read its cache flags so the caller can tell whether a real
    // authenticated bond (link key) was created vs. only an unauthenticated
    // HID-service install.
    let post = poll_device_state(&radio, board.address, 12, Duration::from_millis(250));
    eprintln!(
        "[pair] post-pair Windows state: authenticated={} remembered={} connected={}",
        post.authenticated, post.remembered, post.connected
    );
    if !post.authenticated {
        eprintln!(
            "[pair] NOTE: Windows does not report this device as authenticated. \
             Without a genuinely authenticated bond, reconnecting after a power cycle \
             may still require SYNC. If pairing produced a BTHUSB/16 event-log error, \
             the legacy PIN exchange itself failed."
        );
    }

    Ok(PairResult {
        address: board.address,
        name: board.name.clone(),
        already_paired,
        post,
    })
}

/// RAII handle to the local Bluetooth radio.
///
/// The handle is needed to read the host MAC for PIN derivation and to
/// enable the HID service. Note: the pairing calls themselves are made
/// with a NULL radio handle on purpose — that is exactly what 32feet does
/// (`m_radioHandle` is always `Zero` in `BluetoothWin32Authentication`),
/// and it is what works on this machine.
struct LocalRadio {
    handle: HANDLE,
    address: [u8; 6],
}

impl LocalRadio {
    fn open() -> io::Result<Self> {
        let mut find_params: BLUETOOTH_FIND_RADIO_PARAMS = unsafe { mem::zeroed() };
        find_params.dwSize = mem::size_of::<BLUETOOTH_FIND_RADIO_PARAMS>() as u32;

        let mut handle: HANDLE = ptr::null_mut();
        let h_find = unsafe { BluetoothFindFirstRadio(&find_params, &mut handle) };
        if h_find.is_null() {
            let err = unsafe { GetLastError() };
            return Err(io::Error::other(format!(
                "BluetoothFindFirstRadio failed: os error {err}"
            )));
        }
        unsafe { BluetoothFindRadioClose(h_find) };

        let mut info: BLUETOOTH_RADIO_INFO = unsafe { mem::zeroed() };
        info.dwSize = mem::size_of::<BLUETOOTH_RADIO_INFO>() as u32;
        let rc = unsafe { BluetoothGetRadioInfo(handle, &mut info) };
        if rc != ERROR_SUCCESS {
            unsafe { CloseHandle(handle) };
            return Err(io::Error::other(format!(
                "BluetoothGetRadioInfo failed: os error {rc}"
            )));
        }
        let address = unsafe { info.address.Anonymous.rgBytes };
        Ok(LocalRadio { handle, address })
    }
}

impl Drop for LocalRadio {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.handle) };
    }
}

/// Unpair every Balance Board currently known to Windows. Returns the
/// number removed.
pub fn forget_all_balance_boards() -> io::Result<usize> {
    let devices = scan(Duration::from_secs(2))?;
    let mut count = 0;
    for d in devices {
        if !d.is_balance_board() || !d.remembered {
            continue;
        }
        let mut addr: BLUETOOTH_ADDRESS = unsafe { mem::zeroed() };
        addr.Anonymous.rgBytes = d.address;
        let rc = unsafe { BluetoothRemoveDevice(&addr) };
        if rc == ERROR_SUCCESS {
            count += 1;
        }
    }
    Ok(count)
}

// --- Internals -----------------------------------------------------------

fn device_from_info(info: &BLUETOOTH_DEVICE_INFO) -> WiiDevice {
    let address = unsafe { info.Address.Anonymous.rgBytes };
    WiiDevice {
        address,
        name: wide_to_string(&info.szName),
        authenticated: info.fAuthenticated != 0,
        connected: info.fConnected != 0,
        remembered: info.fRemembered != 0,
    }
}

fn wide_to_string(wide: &[u16]) -> String {
    let len = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..len])
}

fn info_for_address(address: [u8; 6]) -> BLUETOOTH_DEVICE_INFO {
    let mut info: BLUETOOTH_DEVICE_INFO = unsafe { mem::zeroed() };
    info.dwSize = mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32;
    info.Address.Anonymous.rgBytes = address;
    info
}

/// Context passed through Windows' auth callback so the callback can build
/// the legacy PIN response. Mirrors 32feet's `BluetoothWin32Authentication`
/// (which stores the pin on the authenticator instance).
struct AuthContext {
    pin: [u8; 16],
    pin_len: u8,
    /// Local radio handle used for the legacy PIN response.
    radio_handle: HANDLE,
}

unsafe extern "system" fn auth_callback(
    pv_param: *const c_void,
    auth_params: *const BLUETOOTH_AUTHENTICATION_CALLBACK_PARAMS,
) -> i32 {
    if pv_param.is_null() || auth_params.is_null() {
        eprintln!("[auth_callback] null parameter, returning ERROR_INVALID_PARAMETER");
        return ERROR_INVALID_PARAMETER as i32;
    }
    // SAFETY: `pv_param` points at an `AuthContext` we registered; the
    // registration is intentionally kept alive for the process lifetime
    // (see module docs), so the pointee outlives every callback.
    let ctx = unsafe { &*(pv_param.cast::<AuthContext>()) };
    // SAFETY: `auth_params` is provided by the OS and valid for the
    // duration of the callback.
    let params = unsafe { &*auth_params };

    eprintln!(
        "[auth_callback] fired. negotiated authMethod = {} (1=legacy, 2=oob, 3=numeric, 4=passkey-keyboard, 5=passkey-display)",
        params.authenticationMethod
    );

    let mut response: BLUETOOTH_AUTHENTICATE_RESPONSE = unsafe { mem::zeroed() };
    response.bthAddressRemote = params.deviceInfo.Address;
    response.authMethod = BLUETOOTH_AUTHENTICATION_METHOD_LEGACY;
    let len = ctx.pin_len as usize;
    response.Anonymous.pinInfo.pin[..len].copy_from_slice(&ctx.pin[..len]);
    response.Anonymous.pinInfo.pinLength = ctx.pin_len;
    response.negativeResponse = 0;

    // hRadio = NULL, matching 32feet exactly (it always passes Zero here).
    let rc = unsafe { BluetoothSendAuthenticationResponseEx(ctx.radio_handle, &response) };
    eprintln!(
        "[auth_callback] BluetoothSendAuthenticationResponseEx returned {rc} ({})",
        if rc == ERROR_SUCCESS {
            "success"
        } else {
            "error"
        }
    );
    rc as i32
}

/// Register an Ex authentication callback carrying the PIN, then initiate
/// a legacy pairing with the old `BluetoothAuthenticateDevice` API — the
/// same combination BBC/32feet uses.
fn authenticate(
    radio: &LocalRadio,
    info: &mut BLUETOOTH_DEVICE_INFO,
    encoding: PinEncoding,
) -> io::Result<()> {
    // SYNC/bonding PIN is derived from the HOST adapter address. See pin.rs
    // for why passing the board's own address here was wrong.
    let pin_bytes = encoding.to_pin_bytes(radio.address);
    let pin_len = pin_bytes.len();
    debug_assert!(pin_len <= 16);
    let mut ctx = Box::new(AuthContext {
        pin: [0u8; 16],
        pin_len: pin_len as u8,
        radio_handle: radio.handle,
    });
    ctx.pin[..pin_len].copy_from_slice(&pin_bytes);
    let ctx_ptr = Box::into_raw(ctx);

    let mut reg_handle: isize = 0;
    // SAFETY: `info` is initialized; `auth_callback` is a valid extern fn;
    // `ctx_ptr` is intentionally kept alive for the rest of the process.
    let rc = unsafe {
        BluetoothRegisterForAuthenticationEx(
            info,
            &mut reg_handle,
            Some(auth_callback),
            ctx_ptr.cast::<c_void>(),
        )
    };
    if rc != ERROR_SUCCESS {
        // SAFETY: Box::from_raw on a pointer we created via into_raw.
        unsafe { drop(Box::from_raw(ctx_ptr)) };
        return Err(io::Error::other(format!(
            "BluetoothRegisterForAuthenticationEx failed: os error {rc}"
        )));
    }

    eprintln!(
        "[pair] auth callback registered (handle {reg_handle:#x}); initiating legacy pairing..."
    );

    // Legacy initiate with NULL passkey: Windows runs the legacy PIN
    // exchange and asks our callback for the PIN. hwndParent is NULL; the local
    // radio handle is passed explicitly (NULL here failed with os error 6). The
    // passkey/length are zero, i.e. the registered-callback path that 32feet reaches.
    // This API returns a BOOL (non-zero = success), unlike the `Ex`
    // variants which return an error code.
    let auth_rc = unsafe {
        BluetoothAuthenticateDevice(
            ptr::null_mut(), // hwndParent
            radio.handle, // hRadio: explicit handle (NULL failed with os error 6 here)
            info,
            ptr::null(), // pszPin -> NULL forces the registered-callback path
            0,           // ulPinLength
        )
    };
    eprintln!(
        "[pair] BluetoothAuthenticateDevice returned {auth_rc} ({})",
        if auth_rc != 0 {
            "BOOL success"
        } else {
            "BOOL FAILURE"
        }
    );
    if auth_rc == 0 {
        let err = unsafe { GetLastError() };
        eprintln!(
            "[pair] legacy pairing failed (os error {err}). If this is the BTHUSB/16 \
             mutual-auth failure, the PIN byte string or the Windows driver path is at fault; \
             try the other --pin-mode, or remove the stale device record first (explicit --remove-stale)."
        );
        return Err(io::Error::from_raw_os_error(err as i32));
    }

    // NOTE: registration + ctx are intentionally leaked; see module docs.
    let _ = reg_handle;
    Ok(())
}

/// Poll the Windows device cache a few times and report the device flags.
fn poll_device_state(
    radio: &LocalRadio,
    address: [u8; 6],
    attempts: u32,
    pause: Duration,
) -> DeviceState {
    let mut last = DeviceState::default();
    for _ in 0..attempts {
        let mut info = info_for_address(address);
        let rc = unsafe { BluetoothGetDeviceInfo(radio.handle, &mut info) };
        if rc == ERROR_SUCCESS {
            last = DeviceState {
                authenticated: info.fAuthenticated != 0,
                remembered: info.fRemembered != 0,
                connected: info.fConnected != 0,
            };
            if last.authenticated || last.connected || last.remembered {
                break;
            }
        }
        thread::sleep(pause);
    }
    last
}

fn enable_hid_service(radio: &LocalRadio, info: &BLUETOOTH_DEVICE_INFO) -> io::Result<()> {
    // GUID for the HID Service Class. From the Bluetooth SIG:
    // {0000_1124-0000-1000-8000-00805F9B34FB}
    let hid_guid = windows_sys::core::GUID {
        data1: 0x0000_1124,
        data2: 0x0000,
        data3: 0x1000,
        data4: [0x80, 0x00, 0x00, 0x80, 0x5F, 0x9B, 0x34, 0xFB],
    };
    let rc = unsafe {
        BluetoothSetServiceState(radio.handle, info, &hid_guid, BLUETOOTH_SERVICE_ENABLE)
    };
    if rc != ERROR_SUCCESS {
        return Err(io::Error::other(format!(
            "BluetoothSetServiceState (HID enable) failed: os error {rc}"
        )));
    }
    eprintln!("[pair] HID service enabled.");
    Ok(())
}
