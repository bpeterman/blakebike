# blake.bike

A local-first Tauri 2 desktop app for running structured workouts on Bluetooth
FTMS smart trainers. The initial target platforms are macOS and Ubuntu/Pop!_OS
24.04.

## Features

- FTMS trainer discovery, telemetry, ERG power control, and safe stop behavior
- BLE and ANT+ heart-rate monitors (ANT USBStick2 support is Linux-first)
- Built-in simulated trainer for development and hardware-free use
- Structured workout editor with steady, ramp, free-ride, and imported repeat blocks
- ZWO import/export, rider FTP and safety power limit
- Crash-resistant SQLite ride recording, history charts, and CSV/FIT export
- Persistent Garmin-compatible Ride Files with a guided Garmin Connect handoff
- Estimated indoor distance from trainer speed or a flat-road power model
- Independent kg/lb and km/mi display preferences with metric backend storage

## Development

Requirements: Node 22 (`nvm use` reads `.nvmrc`), pnpm 12 (`corepack enable`
installs the pinned version), stable Rust, and the
[Tauri system dependencies](https://v2.tauri.app/start/prerequisites/) for your
platform.

On Ubuntu/Pop!_OS:

```sh
sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev \
  patchelf libdbus-1-dev pkg-config
```

Then:

```sh
pnpm install
pnpm tauri dev
```

macOS asks for Bluetooth access on first scan. On Linux, BlueZ must be running
and the signed-in user must have access to its system D-Bus service.

### ANT+ heart rate on Linux

BlakeBike supports the Dynastream ANT USBStick2 (`0fcf:1008`) through its
Linux USB serial interface. The Debian package installs the least-privilege udev
rule. For development or AppImage use, install the included rule once:

```sh
./scripts/install-ant-udev.sh
```

Then unplug and reconnect the stick. Open Devices, choose Heart rate, and scan
while wearing the strap. ANT devices are marked `ANT+`; Bluetooth devices are
marked `BLE`. A missing ANT stick is ignored, while permission and busy-device
failures are shown alongside the scan results. Close Garmin Express or another
fitness app if it has exclusive access to the stick.

If the stick or monitor is still missing, collect a read-only Linux diagnostic:

```sh
./scripts/diagnose-ant-linux.sh 2>&1 | tee ant-diagnostics.txt
```

The report checks USB descriptors, sysfs interfaces and drivers, raw USB and
serial permissions, process holders, udev rules, kernel messages, and recent
BlakeBike ANT log lines. Paste the complete `ant-diagnostics.txt` into the
shared troubleshooting note. It does not use `sudo` or modify the system.

## Debugging

Everything the app does is written to a daily log file (14 days kept), shown
(with a "Show in folder" button) under Settings → Data & Diagnostics:

- macOS: `~/Library/Logs/com.bpeterman.blakebike/blakebike.YYYY-MM-DD.log`
- Linux: `~/.local/share/com.bpeterman.blakebike/logs/blakebike.YYYY-MM-DD.log`

Dates are UTC. On startup the app also closes any ride the previous run never
finished (crash, kill, power loss) from the samples it had recorded, and
regenerates missing FIT files.

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
LOG=~/Library/Logs/com.bpeterman.blakebike/blakebike.$(date -u +%F).log
tail -f "$LOG"                 # follow live
grep -n "ERROR\|WARN" "$LOG"
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

Ride reliability checks cover silent Bluetooth disconnect detection, pause
acknowledgement retries (including reconnecting while paused), and recording
failures. Recording warnings appear during the ride and remain in History when
measurements were lost; transient write failures clear after the buffered data
is saved. Save failures do not report an unqualified successful completion.

Hardware verification should cover discovery, control acquisition, steady and
ramp targets, pause/resume, skip, disconnect, app quit, and ride recovery on
both target operating-system families.
