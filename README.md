# blake.bike

A local-first Tauri 2 desktop app for running structured workouts on Bluetooth
FTMS smart trainers. The initial target platforms are macOS and Ubuntu/Pop!_OS
24.04.

## Features

- FTMS trainer discovery, telemetry, ERG power control, and safe stop behavior
- Built-in simulated trainer for development and hardware-free use
- Structured workout editor with steady, ramp, free-ride, and imported repeat blocks
- ZWO import/export, rider FTP and safety power limit
- Crash-resistant SQLite ride recording, history charts, and CSV/FIT export
- Persistent Garmin-compatible Ride Files with a guided Garmin Connect handoff

## Development

Requirements: Node 22 (`nvm use` reads `.nvmrc`), pnpm 12 (`corepack enable`
installs the pinned version), stable Rust, and the
[Tauri system dependencies](https://v2.tauri.app/start/prerequisites/) for your
platform.

On Ubuntu/Pop!_OS:

```sh
sudo apt install libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev \
  patchelf libdbus-1-dev pkg-config
```

Then:

```sh
pnpm install
pnpm tauri dev
```

macOS asks for Bluetooth access on first scan. On Linux, BlueZ must be running
and the signed-in user must have access to its system D-Bus service.

## Debugging

Everything the app does is written to a single log file, shown (with a
"Show in folder" button) under Settings → Data & Diagnostics:

- macOS: `~/Library/Logs/com.bpeterman.blakebike/blakebike.log`
- Linux: `~/.local/share/com.bpeterman.blakebike/logs/blakebike.log`

Every finalized ride is also written to the app data directory under
`Ride Files/`. These FIT files are permanent local copies; missing files are
regenerated from SQLite when the app starts. History → Ride Detail → Upload to
Garmin opens Garmin Connect and reveals the selected FIT file in the system file
manager. Drag that file onto Garmin's import page and confirm the upload.

By default the app logs at `debug` for its own code and for btleplug's
Bluetooth internals, and `info` for everything else. That covers every state
transition, each step of a trainer connection (GATT connect, service discovery,
characteristic list, power range, subscriptions, control-point writes and their
responses), workout lifecycle events, UI navigation breadcrumbs and every error
the UI shows. When reporting a problem, note the time, reproduce it, and grep
the log around that time:

```sh
tail -f ~/Library/Logs/com.bpeterman.blakebike/blakebike.log   # follow live
grep -n "ERROR\|WARN" ~/Library/Logs/com.bpeterman.blakebike/blakebike.log
```

For a one-off run with even more detail (raw BLE notifications, every
peripheral seen during a scan), set the filter when launching:

```sh
RUST_LOG=trace pnpm tauri dev
```

## Verification

```sh
pnpm check
pnpm test
pnpm build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
```

Hardware verification should cover discovery, control acquisition, steady and
ramp targets, pause/resume, skip, disconnect, app quit, and ride recovery on
both target operating-system families.
