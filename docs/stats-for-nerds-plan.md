# Stats for Nerds — Plan

Goal: while riding, show everything every connected device is actually reporting — per-device link health, raw readings, packet counts, and which device is feeding each fused metric — behind a setting that is off by default.

## Decisions

- **It is a ride card, not an overlay.** The live ride screen already renders an ordered, per-card list driven by `RideDisplayPreferences`, and Settings → *Live ride cards* already toggles and reorders those cards. A new `deviceStats` card reuses that machinery, so the "toggle in settings" is the existing checkbox and the rider gets ordering for free. No second preference store, no second source of truth.
- **Default hidden.** Today both `normalizeRideDisplayPreferences` (src/types.ts:313) and `RideDisplayPreferences::normalized` (src-tauri/src/storage.rs:178) default any *unknown-to-the-saved-prefs* card to `visible: true`. That is wrong for a diagnostics card: every existing user would suddenly get it. The card list on both sides becomes a table of `(id, defaultVisible)` and the fill-in path uses that default. `deviceStats` is the first entry with `false`.
- **Not gated on developer mode.** Developer mode is about offering simulated devices in scans; nerd stats are useful with real hardware. Keep them independent.
- **Data comes from the devices hub, live.** `DevicesSnapshot.slots[].stats` (`SlotStats`, src/types.ts:82) already carries samples, parse failures, rate, RSSI, battery, make/model/firmware, uptime, drops, reconnect attempts, `lastReading` and `lastRawHex`; `App` keeps it fresh from `devices://slot` and `devices://log` events (src/App.tsx:313). Nothing new is needed from Rust for the readings themselves.
- **No new backend commands.** The only Rust change is the card-id table plus its default.

## Scope of the card

Three bands, top to bottom.

1. **Fusion** — for each of power / cadence / heart rate: the fused value, which role supplied it, and whether it is a fallback (`Telemetry.sources`, already on live samples; fall back to `DevicesSnapshot.sources` between samples). Plus app-side numbers the Devices page cannot show: raw vs. displayed (smoothed) power and the smoothing mode, current target, `runner.control` (`ok` / `degraded` / `lost`), age of the newest telemetry sample, and the number of samples held for the charts.
2. **Per device** — one row per slot that has a device (connected, connecting, reconnecting, or errored; idle roles collapse into a single "Trainer, cadence sensor not connected" line). Each row: role, device name, transport (BLE/ANT), status with reconnect attempt, `lastReading`, rate in Hz, last-sample age, uptime, RSSI, battery, drops, parse failures, total samples, make/model/firmware, and `lastRawHex` in a `<code>`.
3. **Adapter** — ANT adapter status when one is attached, and scan state, straight from the snapshot.

Ages and uptimes need a 1 s clock; the card owns the same `now` interval the Devices page uses.

## Changes

### Shared device readouts (new file)

`src/DeviceStats.tsx` holds the presentation that the ride card and the Devices page both want:

- `Stat` and `statusOf` move out of src/DevicesPage.tsx:482 / src/DevicesPage.tsx:514 into this file and are imported back by `DevicesPage` — one definition, not a copy.
- `DeviceStatsCard` — the ride card, taking `slots`, `sources`, `antAdapter`, `telemetry`, `displayPowerWatts`, `powerSmoothing`, `historySampleCount`, `control`.
- `useNowTick(intervalMs = 1000)` — the one-second clock, pulled from `DevicesPage` so both use it.

`formatUptime`, `formatAge`, `makeAndModel`, `transportLabel`, `sourceNote` already exist in src/devices.ts and are reused as-is.

### Ride screen

- `Ride` gains required props `slots: DeviceSlot[]`, `deviceSources: TelemetrySources | undefined`, `antAdapter: AntAdapterStatus`, `scanning: boolean` (or a single `hub: DevicesSnapshot | null` — simpler signature, and `Ride` already takes ~18 props, so pass the snapshot).
- `cardContent` gains a `deviceStats` entry rendering `DeviceStatsCard`; the `satisfies Record<RideCardId, ReactNode>` check keeps it honest.
- The grid's compact/wide class switch (src/App.tsx:1362) treats `deviceStats` as wide.
- `App` passes `hub` through at src/App.tsx:537/641.

### Preferences (TypeScript)

- src/types.ts: `rideCardIds` gains `"deviceStats"`; add `rideCardDefaultVisible: Record<RideCardId, boolean>` with every current card `true` and `deviceStats` `false`; `defaultRideDisplayPreferences` and `normalizeRideDisplayPreferences` both read that map instead of hard-coding `true`.
- src/App.tsx:1508 `rideCardLabels` gains `deviceStats: "Stats for nerds"`.
- Settings needs one sentence under the card list so the checkbox is self-explanatory: stats for nerds shows raw per-device data during a ride.

### Preferences (Rust)

- src-tauri/src/storage.rs:136: `RIDE_CARD_IDS: [&str; 9]` becomes `RIDE_CARDS: [(&str, bool); 10]` (id, default visible). `Default` and `normalized` map over it; the version stays `2` — adding a card is compatible in both directions and needs no migration, because unknown ids are already dropped and missing ids are already filled in.

### Styling

src/App.css gains a `.ride-nerd-stats` block. It reuses the existing `.device-stat` / `.device-grid` / `.device-detail` styles for the rows, tightened for the ride grid (denser type, scrollable body at small heights) so the card never pushes the primary metrics off screen.

## Risks

- **Re-render cost.** `devices://slot` events already re-render `App` at their current rate, so the card adds no new event traffic; the 1 s clock is one extra render per second and is mounted only when the card is visible. Keep the per-row markup cheap and do not memoize prematurely.
- **Card overflow.** Four devices × a dozen numbers is tall. Rows are a responsive grid and the band scrolls internally rather than stretching the ride grid.
- **Test fallout.** `Ride` gets a required prop, so src/Ride.test.tsx and any other caller must pass a snapshot. That is the point — a default would hide the wiring.

## Tests

- src/types.test.ts: a card absent from saved prefs is filled in at its declared default, so `deviceStats` lands hidden while other missing cards land visible; defaults round-trip.
- src/Ride.test.tsx: with `deviceStats` visible, the card shows each connected device's reading, rate, drops and raw hex, names the source feeding each metric, marks a fallback, and reports roles with no device; with it hidden (the default) nothing renders.
- src/DevicesPage.test.tsx: unchanged and still green after `Stat` / `statusOf` move — proof the extraction was behavior-preserving.
- src/SettingsPage.test.tsx: the new toggle appears in the card list and persists.
- src-tauri/src/storage.rs tests: defaults include `deviceStats` hidden; saved prefs without it get it appended hidden; an explicit `visible: true` survives a round trip.

## Verification

1. `pnpm check` and `pnpm test` (Node via nvm first).
2. `cargo test` in src-tauri.
3. Scratchpad Vite harness importing `Ride` with a fabricated snapshot (connected trainer + HR, one errored slot, one idle) to eyeball the card in the browser pane — the Tauri app itself does not run there.
4. Blake runs the real app with the simulator: toggle the card on in Settings, start a free ride, confirm rates and raw packets tick and that reordering the card works.

## Out of scope

- Recording nerd stats into the session or FIT file.
- Showing the card outside a ride (the Devices page covers idle).
- Per-device charts or history; this card is a live readout.
