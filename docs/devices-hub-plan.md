# Devices Hub — Plan

Goal: turn the single-trainer connection flow into a hub that connects a trainer, a heart-rate monitor, a power meter, and a cadence sensor at the same time, shows live data and a per-device log for each, and remembers devices between launches (manual reconnect, with a way to forget them).

## Decisions (confirmed)

- Persistence: remember known devices, connect manually at launch, with "Forget" per device and "Forget all". Revised (reliability work): a device that drops out during the session reconnects automatically (fresh targeted scan, growing backoff) for as long as a ride is active, and for a handful of attempts otherwise; a manual Connect or Disconnect cancels the attempt.
- Per-device data: everything (live value and rate, battery, signal, uptime/drops, device info, raw packets), with battery, signal strength and uptime always visible on every card, and the rest highlighted per role. Trainer: power and target, plus control status (ERG granted, power range). HR: bpm and sensor-contact state. Power meter: watts, whether crank data is present, pedal balance if reported. Cadence: rpm and coasting/stalled state. Device info and raw hex live in the expanded log view.
- Metric merging: the user picks the source per metric (power, cadence, heart rate) on the Devices page. Default is dedicated-sensor-wins: HR strap over trainer HR; power meter over trainer power; cadence sensor over power-meter crank cadence over trainer cadence. The selection is stored in settings and shown as a selector on each metric. If the chosen source is stale (~3 s, HR 5 s) or disconnected, the fuser falls back down the default order and the UI marks the metric as "falling back to <device>".
- Placement: a new Devices page. The header connection pill becomes a summary of all four roles and opens the page.

## What exists today, and what changes

`DeviceManager` (src-tauri/src/devices.rs) owns one peripheral, one notification worker, one `Telemetry` broadcast, and emits `trainer://connect-progress` lines that the `DevicePicker` modal renders and discards on close. The runner and ride recorder only consume the fused `Telemetry` broadcast, so they need no changes as long as the hub keeps producing one merged stream with the same shape and the same `trainer://telemetry` event name.

## Backend (Rust)

### Module layout

Split `devices.rs` into `devices/`:

- `mod.rs` — `DeviceHub`: four `DeviceSlot`s keyed by `DeviceRole { Trainer, HeartRate, Power, Cadence }`, shared adapter, scan, fuser, known-device persistence hooks.
- `ble.rs` — shared plumbing pulled out of today's code: adapter init, scan window, `with_timeout`, `bluetooth_uuid`, connect + service discovery, and generic reads of Battery Level (0x180F/0x2A19) and Device Information (0x180A: manufacturer 0x2A29, model 0x2A24, firmware 0x2A26).
- `trainer.rs` — today's FTMS connect/control/worker logic, unchanged in behavior, now operating on a slot.
- `heart_rate.rs` — Heart Rate Service 0x180D, Heart Rate Measurement 0x2A37. Parser handles the u8/u16 value flag, sensor-contact bits, and skips energy/RR fields.
- `cycling_power.rs` — Cycling Power Service 0x1818, Cycling Power Measurement 0x2A63. Parser reads flags (u16), instantaneous power (i16), and the optional crank revolution data (cumulative crank revs u16 + last crank event time u16 in 1/1024 s), skipping pedal balance, accumulated torque, and wheel data as flags dictate.
- `cadence.rs` — Cycling Speed and Cadence 0x1816, CSC Measurement 0x2A5B. Parser reads crank revolution data when the crank flag is set (wheel data skipped).
- `crank.rs` — `CrankCadence` state machine shared by power meters and cadence sensors: derives rpm from deltas of cumulative revs and event time with u16 rollover handling, and reports 0 rpm once the event time stops advancing for ~2 s (coasting). Fully unit tested, including rollover and stalls.
- `fuser.rs` — `TelemetryFuser`: holds the latest reading per (metric, source) with timestamps, applies the user's `SourcePreferences { power, cadence, heartRate }` (each `Auto` or a specific role) with staleness fallback down the default order, and produces a fused `Telemetry` plus a `TelemetrySources { power, cadence, heartRate }` record saying which role actually supplied each value and whether it was a fallback. Emits on every incoming reading, coalesced to at most ~4 Hz.
- `log.rs` — `DeviceLog`: a bounded ring buffer (200 lines) of `DeviceLogLine { at, level, step, detail }` per slot. `progress()` becomes `slot.log(...)`, which pushes to the buffer, writes to `tracing` as today, and emits a `devices://log` event tagged with the role.

### Slot model

```
DeviceSlot {
  role, state: SlotState,          // Idle | Connecting | Connected | Reconnecting | Error
  device: Option<DeviceInfo>,
  stats: SlotStats,                // samples, last_sample_ms, rate_hz, rssi, battery_pct,
                                   // connected_since, drops, last_raw_hex, device_info
  log: DeviceLog,
  worker: Option<JoinHandle<()>>,
}
```

The trainer slot additionally keeps the control point, power range, and command lock it has today.

### Scanning and role assignment

One scan window returns `DiscoveredDevice { id, name, rssi, capabilities: [Ftms, HeartRate, CyclingPower, Csc] }` by inspecting advertised service UUIDs. A trainer that also advertises Cycling Power shows both chips; the UI defaults such a device to the Trainer role but lets the user pick. Connecting a known device without a fresh scan runs a short targeted scan (up to 10 s) until its id appears, then connects; this is needed because btleplug only hands out peripherals it has seen.

Connections are serialized with a hub-wide lock and scanning is stopped before a connect, because BlueZ is unreliable when connecting mid-scan. Each connected peripheral has its own notification stream, so workers stay independent.

### Runtime logging per device

Beyond the connect steps that exist today, each slot logs: subscription armed, first sample received, a rate summary every 60 s, parse failures (first five, then every hundredth), battery reads, link lost, and each manual reconnect attempt. Log lines are delivered via `devices://log` while the app is open and can be re-fetched in full via a command, so opening the hub after connecting still shows the history.

### Commands and events

Commands: `scan_devices`, `connect_device(role, id)`, `disconnect_device(role)`, `devices_snapshot` (all slots with stats and their log tails), `device_log(role)`, `known_devices`, `forget_device(id)`, `forget_all_devices`, `get_source_preferences`, `set_source_preferences`. The existing trainer commands remain as thin wrappers during the migration and are removed once the UI is switched.

Events: `devices://state` (full snapshot on any slot change), `devices://log` (one line), `devices://stats` (throttled to 1 Hz per role), `trainer://telemetry` (fused sample, unchanged shape, now with an added `sources` field the UI may ignore).

### Persistence

New SQLite table `known_devices(id TEXT PRIMARY KEY, name, role, capabilities_json, last_connected_at)`. A row is written on every successful connect. Source preferences are stored alongside the profile (three nullable columns or a small `settings` key/value table), defaulting to Auto. `forget_device` deletes a row; `forget_all_devices` clears the table and is also exposed under Settings → Data & Diagnostics. Peripheral ids are the CoreBluetooth UUID on macOS (stable per host) and the MAC on Linux, so a known device is per machine; that is acceptable for a local-first app.

### Simulator

Extend the simulator so each role has a simulated device (Simulator HR, Simulator PM with crank data, Simulator Cadence). This makes the hub, the fuser priorities, and the fallback path testable with no hardware.

## Frontend (React)

- New `Devices` view. Header pill shows four small role dots (green connected, grey idle, amber reconnecting, red error) and opens the page.
- `DeviceCard` per role: role icon and device name (or "Not connected"), status chip, a fixed strip with battery, signal and uptime on every card, one large live value with a role-specific highlight (trainer: power + target + ERG status; HR: bpm + contact; power meter: watts + crank data present; cadence: rpm + coasting), a secondary stats row (rate Hz, last sample age, drops), and Connect / Disconnect / Change actions. An expandable `DeviceLog` section reuses the readout styling from the modal (extract a `Readout` component), shows the last raw packet in hex, and lists manufacturer/model/firmware.
- "Sources" panel: three selectors (Power, Cadence, Heart rate), each listing Auto plus the connected roles that can supply the metric, with the currently active source and any fallback shown live.
- "Known devices" list at the top of the page with one-click Connect and a Forget button per row.
- Scan picker: reuse the modal, now showing capability chips per discovered device and a role selector when a device supports more than one role.
- Ride view: a small "via <device>" badge under Power, Cadence and Heart Rate driven by `sources`.
- Types: `DeviceRole`, `DiscoveredDevice`, `DeviceSlot`, `DevicesSnapshot`, `DeviceLogLine`, `KnownDevice`, `TelemetrySources`; `api.ts` gains the new commands and listeners.

## Phasing

1. Backend refactor with no behavior change: `DeviceHub` with slots, shared BLE helpers, per-slot log ring buffer, FTMS logic moved into the trainer slot, new commands/events, current UI adapted to the snapshot. Existing tests still pass; new tests for the log buffer.
2. Heart rate: parser + tests, HR slot, fuser with source preferences (Auto default) and staleness fallback, simulated HR.
3. Power and cadence: CPS and CSC parsers + tests, `CrankCadence` with rollover/stall tests, both slots, full default order, simulators.
4. Devices page: cards with per-role highlights, live stats, expandable logs, Sources panel, known-devices list, scan picker with roles, header pill, ride-view source badges.
5. Persistence: `known_devices` table, save on connect, Forget / Forget all, stored source preferences.
6. Hardware verification on macOS and Linux: connect all four at once, unplug each in turn and confirm fallback plus log lines, reconnect manually, forget, quit mid-ride and confirm recovery still works.

## Risks and open questions

- Some left-only power meters send crank data, others do not; if a power meter lacks crank data, cadence falls back to the trainer and the card says so.
- Trainers rarely populate the FTMS heart-rate field; the fallback mostly matters for cadence and power.
- Whether a device that appears in multiple roles (trainer + power meter) should ever be connected twice: no, the hub refuses to assign one peripheral to two slots.
- Event rename: keeping `trainer://telemetry` avoids touching the runner and ride view now; a rename to `telemetry://sample` can happen later if desired.

## Status

- Phase 1 done (2026-09-18): `DeviceHub` with four role slots in `src-tauri/src/devices/` (`mod.rs`, `ble.rs`, `log.rs`, `trainer.rs`); per-slot 200-line log ring buffer emitted as `devices://log` and kept for later; `devices://slot` state/stats events (stats throttled to 1 Hz); scan detects FTMS/HR/CPS/CSC capabilities; battery + Device Information read during trainer connect; commands `devices_snapshot`, `scan_devices`, `connect_device`, `disconnect_device`, `device_log` alongside the old trainer commands; TS types/API added. Old UI untouched and still works via the wrapper commands. Blake's free-ride / manual ERG work (runner, storage, commands, App) merged alongside.
- Phase 2 done (2026-09-18): `devices/fuser.rs` (`TelemetryFuser`, `SourcePreferences` Auto/Role per metric, staleness fallback, `TelemetrySources` provenance flattened into the `trainer://telemetry` payload as `sources`); `devices/sensor.rs` generic notify-only sensor slot driven by a `Decoder` trait (shared by HR now, power/cadence next); `devices/heart_rate.rs` (0x2A37 parser + simulated strap); trainer now feeds the fuser instead of emitting directly; `settings` table in SQLite holds source preferences, loaded at startup; commands `get_source_preferences` / `set_source_preferences`; snapshot carries `sourcePreferences` and `sources`. `Telemetry` gained `PartialEq`. No UI yet for connecting HR — that lands with the Devices page (phase 4).
- Phase 3 done (2026-09-18): `devices/crank.rs` (`CrankCadence`: rpm from cumulative crank revs + 1/1024 s event time, u16 rollover on both, holds then reports 0 after 2 s stall, rejects >250 rpm glitches); `devices/cycling_power.rs` (0x2A63 parser incl. balance/torque/wheel/crank, `PowerDecoder`, simulated power meter that pedals ~92 rpm and coasts 8 s/min, shared LE `Reader`); `devices/cadence.rs` (0x2A5B parser, `CadenceDecoder` that also accepts a power meter assigned to the Cadence role, simulated cadence sensor at 85 rpm); Power and Cadence roles are live `Sensor` slots; `Reading::Power` carries `balance_left_percent` for display; `SlotStats.last_reading` holds a per-role one-line summary ("215 W · 88 rpm · L 49% / R 51%", "0 rpm · coasting", trainer "200 W · 88 rpm · 30.1 km/h · target 200 W") for the hub cards. 76 Rust tests. Still no UI to connect non-trainer roles.
- Phase 4 done (2026-09-18): new `src/DevicesPage.tsx` (card per role: status chip, battery/signal/uptime strip, big `lastReading`, rate/last-sample/drops/bad-packets line, "feeding …" badge, Connect/Change/Disconnect, expandable log with make/model/firmware/last packet/samples and a clock-stamped readout; Sources panel with Auto/role selects showing live "from X" / "falling back to X"). `src/DevicePicker.tsx` replaces the trainer-only modal: role-aware, filters scan results by capability, shows capability chips, drives the connect readout from `devices://log` (connect lines). `src/devices.ts` holds shared pure helpers (tested in `devices.test.ts`); `DevicesPage.test.tsx` renders the page against a fake snapshot. `App.tsx`: "Devices" nav item, hub snapshot state fed by `devices://slot` + `devices://log`, sidebar pill shows four role dots and opens the page, ride metrics show "via …"/"fallback: …" notes. 13 vitest tests. Blake's free-ride/FIT/Garmin code untouched.
- Phase 5 done (2026-09-18): `known_devices` table (JSON payload + last_connected_at) with `remember_device` / `known_devices` / `forget_device` / `forget_all_devices` in storage; `KnownDevice` carries manufacturer/model so the list shows make even when disconnected; connect commands remember the device on success (never fails the connect); `Ble::find(id)` targeted scan (≤10 s) used automatically by `DeviceHub::connect` when a device is not in the last scan, with "Looking for remembered device" / "Device found" / "Device not found" log lines; commands `known_devices`, `forget_device`, `forget_all_devices`. UI: Known devices section on the Devices page (make · model, last used, Connect/Disconnect, Forget), make · model under each connected card's name, remembered make on picker rows, "Forget all devices" under Settings → Data & Diagnostics. 84 Rust / 16 vitest tests.
- Phase 6 (hardware): checklist written to `docs/devices-hub-verification.md` — sections A–F covering discovery, per-role connect, four-at-once, drop-outs/fallback, known devices/persistence, recovery/BlueZ restart/macOS permission. Needs Blake's hardware on both platforms; fix whatever it turns up.
- Possible follow-ups surfaced while building: name-based manufacturer fallback for sensors without Device Information; re-applying the ERG target after a mid-ride trainer reconnect (verify in D4).
