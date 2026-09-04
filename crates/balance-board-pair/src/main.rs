//! `balance-board-pair` — auto-pair the Balance Board on Windows.

use std::time::Duration;

use balance_board_pair::pin::PinEncoding;

const HELP: &str = "\
Usage: balance-board-pair [--scan | --forget | --pin-mode <raw|bbc> | --help]

  (no flags)    Default. Scan for nearby Wii devices, find the Balance
                Board, pair it (SYNC bonding PIN derived from the PC's
                Bluetooth radio MAC, answered through a registered auth
                callback, pairing initiated with the legacy
                BluetoothAuthenticateDevice API), then enable the HID
                service so Windows treats it as a normal game controller.

  --scan        List nearby Wii-family Bluetooth devices and exit.
                Doesn't pair anything. Useful for sanity-checking that
                the board is in pairing mode (press SYNC inside the
                battery cover).

  --forget      Unpair every Balance Board currently known to Windows.
                Useful if the device cache is in a bad state. Destructive:
                removes the Windows-side pairing record (the board keeps
                its own copy of the bond, which cannot be cleared remotely).

  --pin-mode    Which byte string to send as the legacy PIN:
                  raw  (default) the six raw host-MAC bytes (WiiBrew /
                        WiiBalanceWalker rule for SYNC bonding).
                  bbc  the byte string 32feet actually hands Windows on its
                        Ex callback path (UTF-8 of a char string of the
                        reversed host-MAC bytes). Use for A/B testing only.

  --install-only  WiiFitToVRC-style: enable the HID service on a SYNC-discovered\n                board with NO PIN exchange. Use when the stale-key deadlock\n                makes normal pairing hang (BTHUSB/16). Profile will likely be\n                authenticated=false (SYNC needed again next session).\n\n  --remove-stale  Before pairing, remove existing Windows records for all
                remembered Balance Boards (like BBC/WiiFitToVRC do). Only
                needed when a stale, inconsistent record blocks re-pairing;
                destructive to the Windows-side pairing record.

  --help, -h    Show this help.
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        eprintln!("{HELP}");
        return;
    }

    #[cfg(not(windows))]
    {
        eprintln!(
            "balance-board-pair is Windows-only. On Linux use bluetoothctl, \
             on macOS use blueutil — both pair these boards when SYNC is pressed.\n\
             This stub binary will exit now."
        );
        std::process::exit(2);
    }

    #[cfg(windows)]
    if let Err(e) = run(&args) {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    use balance_board_pair::pin::format_bd_addr;
    use balance_board_pair::{forget_all_balance_boards, install_only, pair_first, scan};

    let mut encoding = PinEncoding::Raw;
    let mut remove_stale = false;
    let mut positional = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--pin-mode" => {
                let value = it.next().ok_or("--pin-mode requires a value: raw or bbc")?;
                encoding = match value.as_str() {
                    "raw" => PinEncoding::Raw,
                    "bbc" => PinEncoding::BbcUtf8Like,
                    other => {
                        return Err(
                            format!("unknown --pin-mode '{other}' (expected raw or bbc)").into(),
                        )
                    }
                };
            }
            "--remove-stale" => remove_stale = true,
            other => positional.push(other.to_string()),
        }
    }

    if positional.iter().any(|a| a == "--scan") {
        eprintln!("Scanning (~10s). Press SYNC on any unpaired devices you want to see.");
        let devices = scan(Duration::from_secs(10))?;
        if devices.is_empty() {
            println!("No Wii-family devices found nearby.");
            return Ok(());
        }
        println!(
            "{:<24}  {:<17}  {:<6}  {:<4}  {:<5}",
            "name", "address", "paired", "conn", "remem"
        );
        for d in &devices {
            println!(
                "{name:<24}  {addr:<17}  {p:<6}  {c:<4}  {r:<5}",
                name = d.name,
                addr = format_bd_addr(d.address),
                p = if d.authenticated { "yes" } else { "no" },
                c = if d.connected { "yes" } else { "no" },
                r = if d.remembered { "yes" } else { "no" },
            );
        }
        return Ok(());
    }

    if positional.iter().any(|a| a == "--forget") {
        let n = forget_all_balance_boards()?;
        eprintln!("Removed {n} Balance Board(s).");
        return Ok(());
    }

    if positional.iter().any(|a| a == "--install-only") {
        eprintln!(
            "Install-only mode: no PIN exchange. Press the red SYNC button when a\n\
             discovery round prints; HID service is enabled as soon as the board appears."
        );
        let result = install_only(Duration::from_secs(5), 12)?;
        eprintln!(
            "{name} ({addr}) install-only finished. Post state: authenticated={authenticated} remembered={remembered} connected={connected}",
            name = result.name,
            addr = format_bd_addr(result.address),
            authenticated = result.post.authenticated,
            remembered = result.post.remembered,
            connected = result.post.connected
        );
        return Ok(());
    }

    if !positional.is_empty() {
        eprintln!("Unknown argument(s): {positional:?}\n\n{HELP}");
        std::process::exit(2);
    }

    if remove_stale {
        eprintln!("[pair] removing existing remembered Balance Board records first (like BBC/WiiFitToVRC)...");
        let n = forget_all_balance_boards()?;
        eprintln!("[pair] removed {n} stale Balance Board record(s).");
    }

    eprintln!(
        "Press SYNC inside the battery cover IMMEDIATELY before continuing.\n\
         Scanning briefly (~5s), then pairing. The SYNC discoverable window is ~20s, so\n\
         keep the scan tight to leave time for the handshake."
    );
    let result = pair_first(Duration::from_secs(5), 6, encoding)?;
    if result.already_paired {
        eprintln!(
            "{name} ({addr}) was already authenticated; HID service (re)enabled.",
            name = result.name,
            addr = format_bd_addr(result.address),
        );
    } else {
        eprintln!(
            "{name} ({addr}) pairing flow finished (PIN bytes and mode were printed above).",
            name = result.name,
            addr = format_bd_addr(result.address),
        );
    }
    eprintln!(
        "Post-pair Windows state: authenticated={} remembered={} connected={}",
        result.post.authenticated, result.post.remembered, result.post.connected
    );
    if result.post.authenticated {
        eprintln!(
            "Windows reports an authenticated bond. If the board keeps that bond, later\n\
             power-cycles should reconnect without SYNC while the Windows record stays intact."
        );
    } else {
        eprintln!(
            "No authenticated bond was reported. Expect that reconnecting after a power\n\
             cycle will still require SYNC until a genuinely authenticated pairing succeeds."
        );
    }
    eprintln!("\nNext: start BBC or run balance-board-bridge to open the HID session.");
    Ok(())
}
