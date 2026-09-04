//! Wii Balance Board pairing-PIN derivation.
//!
//! The Balance Board (RVL-WBC-01) has exactly one pairing mode: the red
//! SYNC button inside the battery compartment, which is Bluetooth
//! **bonding**. It has no Wiimote-style "1 + 2" guest mode, so only the
//! bonding PIN rule below applies to it.
//!
//! Cross-checked against:
//! - WiiBrew *Wiimote § Bluetooth Pairing*: for SYNC/bonding the PIN is
//!   the **host's** Bluetooth address, in reverse byte order. The device's
//!   own address reversed is only the rule for the Wiimote's temporary
//!   1 + 2 mode.
//! - WiiBalanceWalker `FormBluetooth.cs`: "Sync button requires host
//!   address, holding 1+2 buttons requires device address."
//! - BBC v1.5.2 / 32feet `WiiBluetoothPin.TryCreateFromHostMac`, which
//!   builds the PIN from the local radio's MAC.
//!
//! Earlier revisions of this crate sent the *board's own* MAC reversed,
//! which is the 1 + 2 rule — the wrong rule for a SYNC-only board — and
//! real pairing stalled at the auth callback.
//!
//! # Byte order
//!
//! Win32 stores a Bluetooth address in `BLUETOOTH_ADDRESS.rgBytes` in
//! little-endian order (least-significant octet first). That layout is
//! already the "reversed BD_ADDR" that WiiBrew describes for the bonding
//! PIN, so [`wii_sync_pin_raw`] is just an identity over the host radio's
//! `rgBytes` array.
//!
//! For a host displayed as `60:E3:2B:58:81:6B`, `rgBytes` is
//! `[0x6B, 0x81, 0x58, 0x2B, 0xE3, 0x60]`, which is the raw PIN.

/// Length of a Wii pairing PIN, in bytes. Equal to the Bluetooth address
/// length.
pub const WII_PIN_LEN: usize = 6;

/// Which byte string to hand Windows as the legacy pairing PIN.
///
/// [`PinEncoding::Raw`] sends the six raw host-MAC bytes that WiiBrew and
/// WiiBalanceWalker describe. [`PinEncoding::BbcUtf8Like`] reproduces the
/// exact byte string BBC v1.5.2 / 32feet sends down its Ex callback path:
/// 32feet builds a string whose character code points equal the reversed
/// host bytes, then UTF-8-encodes that string into `pinInfo.pin`. The two
/// differ whenever any host byte is >= 0x80 (which is the case for this
/// machine's adapter). We cannot tell from static analysis alone whether
/// Windows or the board tolerates the UTF-8 expansion, so both are offered
/// and the CLI defaults to [`PinEncoding::Raw`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinEncoding {
    /// The six raw host-MAC bytes (Win32 `rgBytes` order).
    Raw,
    /// 32feet's UTF-8-of-a-char-string encoding (see module docs).
    BbcUtf8Like,
}

impl PinEncoding {
    /// Compute the PIN byte string that this encoding would send for the
    /// given host radio address (already in Win32 `rgBytes` order).
    pub fn to_pin_bytes(self, host_rg_bytes: [u8; WII_PIN_LEN]) -> Vec<u8> {
        match self {
            PinEncoding::Raw => wii_sync_pin_raw(host_rg_bytes).to_vec(),
            PinEncoding::BbcUtf8Like => wii_sync_pin_bbc_utf8like(host_rg_bytes),
        }
    }
}

/// The raw SYNC/bonding PIN: the **host radio's** Bluetooth address in
/// Win32 `rgBytes` order (little-endian, equivalent to "reversed BD_ADDR"
/// if you read addresses big-endian the way Bluetooth UIs do).
///
/// This is an identity over the input; the function exists to encode the
/// convention and the host-vs-device rule in one place so callers cannot
/// accidentally pass the board's own address (the 1 + 2 guest rule, which
/// does not apply to the Balance Board).
#[must_use]
pub fn wii_sync_pin_raw(host_rg_bytes: [u8; WII_PIN_LEN]) -> [u8; WII_PIN_LEN] {
    host_rg_bytes
}

/// Reproduce what BBC v1.5.2 / 32feet actually places into the legacy PIN
/// response on its Ex callback path: a string whose character code points
/// are the reversed host-MAC bytes, UTF-8 encoded (so bytes >= 0x80 expand
/// to two bytes), truncated at 16 bytes.
///
/// This is provided for A/B testing against [`wii_sync_pin_raw`]; it is
/// *not* what WiiBrew describes as the board's expected PIN.
#[must_use]
pub fn wii_sync_pin_bbc_utf8like(host_rg_bytes: [u8; WII_PIN_LEN]) -> Vec<u8> {
    // Every byte is in 0..=255, so `as char` is always a valid code point
    // and `String::as_bytes()` yields exactly the UTF-8 encoding of it.
    let chars: String = host_rg_bytes.iter().map(|&b| b as char).collect();
    let mut encoded = chars.as_bytes().to_vec();
    encoded.truncate(16);
    encoded
}

/// Format a PIN as colon-separated uppercase hex (e.g. `6B:81:58:2B:E3:60`).
#[must_use]
pub fn format_pin(pin: &[u8]) -> String {
    let mut s = String::with_capacity(pin.len() * 3 - 1);
    for (i, b) in pin.iter().enumerate() {
        if i > 0 {
            s.push(':');
        }
        s.push_str(&format!("{b:02X}"));
    }
    s
}

/// Format a Bluetooth address (Win32 `rgBytes` order) as colon-separated
/// uppercase hex in the human-readable big-endian convention used by most
/// Bluetooth UIs (e.g. `60:E3:2B:58:81:6B` from `rgBytes`
/// `[0x6B, 0x81, 0x58, 0x2B, 0xE3, 0x60]`).
#[must_use]
pub fn format_bd_addr(rg_bytes: [u8; 6]) -> String {
    let mut s = String::with_capacity(6 * 3 - 1);
    for (i, b) in rg_bytes.iter().rev().enumerate() {
        if i > 0 {
            s.push(':');
        }
        s.push_str(&format!("{b:02X}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    // This machine: host radio 60:E3:2B:58:81:6B -> rgBytes LSB-first.
    const HOST_RG: [u8; 6] = [0x6B, 0x81, 0x58, 0x2B, 0xE3, 0x60];

    #[test]
    fn raw_sync_pin_is_host_rgbytes_unchanged() {
        let pin = wii_sync_pin_raw(HOST_RG);
        assert_eq!(pin, HOST_RG, "raw PIN must be host rgBytes verbatim");
    }

    #[test]
    fn raw_pin_matches_documented_reversed_host_address() {
        // WiiBrew example: host 11:22:33:44:55:66 -> PIN 66 55 44 33 22 11.
        let host = [0x66, 0x55, 0x44, 0x33, 0x22, 0x11];
        assert_eq!(wii_sync_pin_raw(host), host);
        assert_eq!(format_bd_addr(host), "11:22:33:44:55:66");
    }

    #[test]
    fn bbc_utf8like_expands_high_bytes() {
        // 32feet sends UTF-8 of a char string whose code points equal the
        // reversed host bytes: chars k, U+0081, X, +, U+00E3, ` -> bytes
        // 6B C2 81 58 2B C3 A3 60.
        let expected = [0x6B, 0xC2, 0x81, 0x58, 0x2B, 0xC3, 0xA3, 0x60];
        assert_eq!(wii_sync_pin_bbc_utf8like(HOST_RG), expected);
    }

    #[test]
    fn bbc_utf8like_is_at_most_12_bytes_for_a_host_mac() {
        // UTF-8 of any U+00xx code point is at most two bytes, so a 6-byte
        // host MAC can never produce more than 12 bytes; the 16-byte pinInfo
        // cap in 32feet therefore never truncates a real host address.
        assert_eq!(wii_sync_pin_bbc_utf8like([0xFF; 6]).len(), 12);
        assert_eq!(wii_sync_pin_bbc_utf8like([0x80; 6]).len(), 12);
        assert_eq!(wii_sync_pin_bbc_utf8like([0x00; 6]).len(), 6);
    }

    #[test]
    fn pin_encoding_to_bytes_matches_rules() {
        assert_eq!(PinEncoding::Raw.to_pin_bytes(HOST_RG), HOST_RG.to_vec());
        assert_eq!(
            PinEncoding::BbcUtf8Like.to_pin_bytes(HOST_RG),
            [0x6B, 0xC2, 0x81, 0x58, 0x2B, 0xC3, 0xA3, 0x60]
        );
    }

    #[test]
    fn pin_format_is_colon_hex() {
        assert_eq!(format_pin(&HOST_RG), "6B:81:58:2B:E3:60");
    }

    #[test]
    fn bd_addr_formats_in_human_readable_big_endian() {
        assert_eq!(format_bd_addr(HOST_RG), "60:E3:2B:58:81:6B");
    }
}
