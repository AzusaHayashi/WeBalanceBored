//! Auto-pair the Wii Balance Board on Windows.
//!
//! The board (RVL-WBC-01) only pairs through its red SYNC button, which is
//! Bluetooth **bonding**. Bonding uses the legacy (non-SSP) PIN exchange
//! with a PIN equal to the **host PC's** Bluetooth address in reversed byte
//! order (see [`pin`]) — not the board's own address, which is the Wiimote
//! 1+2 "guest" rule and does not apply to a SYNC-only board.
//!
//! The pairing flow implemented here mirrors what BBC v1.5.2 / 32feet does
//! on this machine and what WiiBalanceWalker does with "Permanent sync":
//! register a `BluetoothRegisterForAuthenticationEx` callback that answers
//! with the host-derived PIN, then initiate with the *legacy*
//! `BluetoothAuthenticateDevice` API, then enable the HID service so
//! Windows installs the board as a normal Bluetooth HID device that
//! `balance-board-io`'s hidapi discovery can find (VID `0x057E`, PID
//! `0x0306`).
//!
//! After a *successful authenticated* pairing, Windows stores a link key
//! and the board stores a host entry for this PC, so later reconnects
//! (board powered on in range) can happen without pressing SYNC again. If
//! only the unauthenticated HID-service install succeeds
//! (`Authenticated = false`), reconnecting after a power cycle will likely
//! still require SYNC — the post-pair state reported by the CLI tells the
//! two cases apart.
//!
//! ```pwsh
//! balance-board-pair            # press SYNC, finds + pairs the board
//! balance-board-pair --scan     # list nearby Wii devices, no pairing
//! balance-board-pair --forget   # unpair every Balance Board
//! balance-board-pair --pin-mode bbc   # send the 32feet UTF-8 byte string instead of raw
//! ```
//!
//! # Why Windows-only
//!
//! Pairing requires platform-specific Bluetooth APIs. The Win32 surface is
//! one we can drive directly via `windows-sys`. Linux (`bluetoothctl`) and
//! macOS (`blueutil`) have their own tools that already pair these devices.

#![warn(missing_docs)]

pub mod pin;

#[cfg(windows)]
mod bluetooth;

#[cfg(windows)]
pub use bluetooth::{
    forget_all_balance_boards, pair_first, scan, DeviceState, PairResult, WiiDevice,
};
