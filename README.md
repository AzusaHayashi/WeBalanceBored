# WeBalanceBored

[![CI](https://github.com/ChromiteExabyte/WeBalanceBored/actions/workflows/ci.yml/badge.svg)](https://github.com/ChromiteExabyte/WeBalanceBored/actions/workflows/ci.yml)

A Wii Balance Board → Steam Input bridge for Windows. Built as a Rust workspace
so the protocol parsing and calibration math are reusable by other Balance
Board projects, not locked inside this app.

## Status

Pre-alpha.

| Layer | State |
| --- | --- |
| Protocol parsing & calibration | Implemented, unit-tested with byte fixtures |
| HID I/O (`hidapi`) | Implemented; needs hardware to verify |
| vJoy output (runtime LoadLibraryW FFI) | Implemented; needs vJoy + hardware to verify |
| End-to-end bridge binary, with tare + smoothing + calibration cache | Implemented |
| Auto-pair tool (Win32 Bluetooth) | Scan implemented + verified; pair implemented, needs a SYNC-pressed board to fully verify |
| Steam Input setup guide for Superflight | [docs/steam-input/superflight.md](docs/steam-input/superflight.md) |
| System tray / config UI | Not started |

## Workspace layout

| Crate | License | Purpose |
| --- | --- | --- |
| `balance-board-protocol` | MPL-2.0 | Pure parsing, calibration, center-of-gravity math, smoothing filter. No I/O, zero deps, runs on any machine without a board. |
| `balance-board-io` | MPL-2.0 | HID glue via `hidapi`. Reads bytes off the wire, hands them to the protocol crate. Cross-platform. |
| `balance-board-bridge` | GPL-3.0-or-later | The end-user binary. vJoy output, tare + smoothing, calibration cache. |
| `balance-board-pair` | GPL-3.0-or-later | Windows-only auto-pair tool. Computes the Wii SYNC-bonding PIN (host BD_ADDR reversed) and drives the legacy Bluetooth pairing flow. |

The split licensing is deliberate: the reusable crates use file-level copyleft
(MPL-2.0) so anyone can pull them into their own projects; the bridge binary
is GPL-3.0 to keep derivative end-user tools open.

## Build & run

```pwsh
cargo test -p balance-board-protocol                              # no hardware needed
cargo build --release --workspace                                  # everything
cargo run --release -p balance-board-pair -- --scan                # list nearby Wii devices
cargo run --release -p balance-board-pair                          # auto-pair the board
cargo run --release -p balance-board-io --example print_sensors    # smoke test (board, no vJoy)
cargo run --release -p balance-board-bridge                        # full bridge (board + vJoy)
cargo run --release -p balance-board-bridge -- --help              # see all flags
```

### Prerequisites

1. **Rust toolchain** — `winget install Rustlang.Rustup`, or grab `rustup-init.exe` from <https://rustup.rs>.
2. **Pair the Balance Board.** Easiest path: press SYNC inside the battery cover, then run `cargo run --release -p balance-board-pair`. That tool scans for `Nintendo RVL-WBC-01`, computes the special SYNC-bonding PIN (the PC Bluetooth radio MAC in reversed byte order), runs the legacy Bluetooth pairing flow, and enables the HID service — none of which the standard Windows Bluetooth wizard does correctly. Run with `--scan` first if you want to verify the board is in range without committing to pairing, or `--forget` to unpair if state gets weird.
3. **Install vJoy** for the bridge binary: <https://github.com/jshafer817/vJoy/releases>. Run **Configure vJoy** afterwards, ensure device #1 is enabled, and check at least axes X, Y, Z, Rx, Ry, Rz. (The smoke-test example does *not* need vJoy.)
4. **Steam Input mapping** — launch a game with controller support, open Steam's controller settings, and bind vJoy's X/Y to the in-game stick of your choice. For Superflight: vJoy X → right-stick X, vJoy Y → right-stick Y, plus a small radial deadzone.

## Goals

1. Play Superflight (and other Steam games) using a Wii Balance Board, via
   the path `Balance Board → Bluetooth HID → vJoy → Steam Input → game`.
   Step-by-step guide: [docs/steam-input/superflight.md](docs/steam-input/superflight.md).
2. Provide a clean, documented Rust crate that other Balance Board projects
   can depend on for parsing, calibration, and center-of-gravity math.

Inspired by, and rewritten from scratch over,
[lshachar/WiiBalanceWalker](https://github.com/lshachar/WiiBalanceWalker).

## License

This repository ships under two licenses depending on the crate.
Each crate's `Cargo.toml` declares its license via SPDX identifier; the
canonical license texts are at `LICENSE-MPL-2.0` and `LICENSE-GPL-3.0`.

- `balance-board-protocol`, `balance-board-io` — MPL-2.0
- `balance-board-bridge` — GPL-3.0-or-later
