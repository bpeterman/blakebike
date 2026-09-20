# Dual Power Recording and Accuracy Analysis — Plan

Goal: when a trainer and a power meter are both connected, record both power
streams for the ride and produce the accuracy comparison reviewers build by hand
in a spreadsheet — the two traces overlaid, mean offset, drift as the trainer
warms up, divergence by power level and by cadence, spread and worst case — as
one shareable PNG, plus a live delta readout while riding. Neither source is
treated as truth: the report is "trainer versus meter", never "your trainer is
wrong".

## Background

The second stream already reaches the process and is discarded. Both roles
connect today; the fuser's `Metric::Power` priority
(src-tauri/src/devices/fuser.rs:49) makes the meter win, and the trainer's power
reading dies inside `Inner::latest` after its 3 s staleness window. Every
reading passes through `TelemetryFuser::ingest`
(src-tauri/src/devices/fuser.rs:198 for the trainer,
src-tauri/src/devices/fuser.rs:214 for the meter) with the role and a timestamp,
so that is the one place to tap.

There is no temperature anywhere in the codebase and FTMS does not report one.
Warm-up is measured on **elapsed time from ride start**, and the UI says so.

Timestamps on both streams are arrival times (`Utc::now()` when the
notification lands, src-tauri/src/devices/trainer.rs:416 and
src-tauri/src/devices/sensor.rs:380); neither FTMS nor Cycling Power carries a
device clock. The lag estimate is therefore bounded by Bluetooth arrival jitter,
which is fine at the sub-second resolution that matters here.

## Decisions

1. **Per-source readings get their own table, on their own clocks.** The fused
   sample is emitted at most every 200 ms and only when *some* device reports
   (src-tauri/src/devices/fuser.rs:140, :232), so attaching per-source values to
   `telemetry_samples.payload_json` (src-tauri/src/storage.rs:520) would either
   duplicate each device's latest value onto every fused emit (sample-and-hold
   data that biases the lag estimate) or drop readings between emits. Lag
   estimation needs each device's own arrival timestamps, so a new
   `source_samples` table keyed by `(session_id, role, timestamp_ms)` holds one
   row per device reading with plain numeric columns (`power_watts`,
   `cadence_rpm`, `balance_left_percent`, all nullable). `Telemetry`
   (src-tauri/src/domain.rs:269) is untouched: it is serialized field-for-field
   into the CSV export (src-tauri/src/commands.rs:747) and the stored payload, so
   new fields there would change the CSV columns and grow every ride (the FIT
   record message, src-tauri/src/fit.rs:299, picks its fields explicitly and
   would not change). The row shape — role, timestamp, watts, cadence, balance —
   is what a FIT developer field could carry per record later, and `cadence_rpm`
   as its own column is what the cadence-source comparison will read. Two
   readings from one device in the same millisecond would collapse onto one row
   under the `INSERT OR REPLACE` upsert; at ≤ 4 Hz that does not happen, and the
   upsert is what makes a retried batch idempotent.
2. **The fuser emits a side-band `SourceSample` stream; fusion is unchanged.**
   `ingest` gains one `broadcast::Sender<SourceSample>` and sends on every
   `Reading::Trainer` and `Reading::Power`, *before* the 200 ms throttle so the
   stream runs at device rate. `pick`, `fuse`, `SourcePreferences` and the fused
   values on `trainer://telemetry` are not touched; ERG control and the
   displayed power keep their exact behaviour. `DeviceHub` exposes
   `subscribe_sources()` next to `subscribe()`
   (src-tauri/src/devices/mod.rs:944). A send with no subscriber is ignored, as
   the telemetry send already is.
3. **A trainer frame without power is not a 0 W reading.** `parse_indoor_bike_data`
   (src-tauri/src/ftms.rs:151) only fills `power_watts` when flag bit 6 is set
   and otherwise leaves `Telemetry::default()`'s 0, which `ingest`
   (src-tauri/src/devices/fuser.rs:199) records as real power. Trainers that
   alternate frames or omit power intermittently have been feeding zeros into
   fusion, masked so far by the 200 ms throttle and by the meter winning; a
   device-rate source stream would have no mask (halved bin means, spurious
   coasting, a degraded lag estimate). The parser returns an `IndoorBikeData`
   struct with `power_watts: Option<u16>` instead of a half-filled `Telemetry`;
   `Reading::Trainer` carries it; the fuser records power and emits a source
   sample only when present. This fixes the latent fused-power bug in the same
   change.
4. **Recording is automatic and gated on the rider having two power devices.**
   The ride timeline (src-tauri/src/runner.rs:920) subscribes to the source
   stream and admits a role's sample when another power-capable role's slot has
   a device — `Connecting`, `Reconnecting`, `Ready` or `Controlling`
   (src-tauri/src/devices/mod.rs:93), not `Idle`, `Scanning` or `Error`. Gating
   on connection intent rather than on a recent reading means a 4 s Bluetooth
   hiccup on the meter leaves the trainer's stream whole, so coverage shows
   truthfully which device went quiet. A single-trainer ride records nothing
   extra, a meter that joins mid-ride starts both streams from that moment, and
   disconnecting one stops both. Nothing to switch on; the analysis handles
   partial overlap. With the fixed role set there are exactly two power-capable
   roles (Trainer, Power), so "three sources" cannot happen today; the table is
   keyed by role and the analysis takes a pair, so a future third role means
   choosing pairs, not a schema change.
5. **The second stream gets its own recorder instance, not a share of the
   first.** `Recorder` (src-tauri/src/runner.rs:1383) is the crash-resistant
   path and must not regress. It uses one `sync_channel(256)` for samples and
   flushes; a full channel counts as `dropped` (src-tauri/src/runner.rs:1421),
   which `RecordingHealth::warning` (src-tauri/src/runner.rs:1350) shows the
   rider, and `flush` spins until a slot frees. Adding ~5 msg/s of source
   samples to that channel would double its fill rate and let a slow disk drop
   telemetry sooner — the opposite of inert. So `Recorder`, `recorder_loop` and
   `write_with_retries` become generic over a `Recordable` sample type (one
   implementation, two instances): the telemetry recorder is exactly what it is
   today, and a second `Recorder<SourceSample>` has its own channel, thread,
   backlog cap and health that nothing reads except the log. Its flush runs
   after the telemetry flush result is captured, with a 1 s grace whose error is
   logged and never merged into the save warning. A failed source write cannot
   fail the ride, the flush, or the summary.
6. **The devices of a ride are recorded with the session.** The report needs
   both device names and firmware versions for a *historical* ride, but only
   the live `SlotStats` (src-tauri/src/devices/mod.rs:138) knows them. A new
   `session_devices` table stores one `RideDevice` per role — id, name,
   transport, simulated flag, manufacturer, model, firmware, last calibration —
   captured at ride start for every connected role and again when a role that
   was not connected at start first has a sample admitted. This is recorded
   for every ride, not just dual-power ones: it is what a FIT `device_info`
   message needs later, and the `id` lets it be matched to `known_devices`.
7. **Zero-offset drift becomes persistent.** `CalibrationRecord`
   (src-tauri/src/devices/mod.rs:186) keeps only the latest offset; the drift
   against the previous zero is computed in `zero_offset`
   (src-tauri/src/devices/cycling_power.rs:508) and shown once in the dialog,
   then lost. The record gains `previous_offset_raw` (serde-defaulted, so old
   rows still parse), set from the same `previous` at
   src-tauri/src/devices/cycling_power.rs:514. The report can then say "zeroed
   2 h before the ride, offset 1019, +3 versus the zero before" from the
   `RideDevice` snapshot alone. `offsetDrift` in src/devices.ts already formats
   it.
8. **The statistics live in Rust, in one module, verified against synthetic
   streams.** `src-tauri/src/dual_power.rs` turns two `SourceSample` streams
   plus the session summary and devices into a serializable `PowerComparison`.
   Rust is where the raw streams are and where the FIT developer-field export
   will read them; the frontend receives a finished report and only formats and
   draws. A new command `get_power_comparison(id)` returns `Option<…>` — `None`
   when the ride has fewer than two recorded power sources, so every existing
   ride renders exactly as before. It is separate from `get_session`
   (src-tauri/src/commands.rs:738) because that call also restores a running
   ride's history at startup and must stay cheap.
9. **One renderer draws both the on-screen view and the PNG.** A canvas painter
   (`src/powerComparisonReport.ts`) lays out the report against a small
   `Painter` interface; `CanvasPainter` implements it for the modal and for
   export, and a recording painter implements it in tests. What the rider sees
   in the ride detail is the image they share, and there is no second Recharts
   rendering to keep in sync. The canvas draws at `devicePixelRatio`, waits for
   `document.fonts.ready`, and sits next to a textual summary of the headline
   numbers and the reading guide, so screen readers and component tests have
   the content without pixels; `getContext` returning `null` (jsdom) is
   handled. Export uses the existing save-dialog pattern (src/api.ts
   `exportSessionFit`) and a new `export_png` command that takes the PNG bytes
   as a raw IPC body (`invoke(cmd, Uint8Array, { headers })` →
   `tauri::ipc::Request`) with the percent-encoded path in a header, so there
   is no base64 inflation and no plotting or font stack in Rust (CI builds on
   Ubuntu without one).
10. **Live delta piggybacks on the fused event.** `FusedTelemetry`
    (src-tauri/src/devices/fuser.rs:116) gains `power_by_source`, a map of
    power-capable role → fresh watts built in `fuse` from `Inner::latest`
    through the same `fresh()` staleness rule; it repeats the chosen role's
    value that `sources` already names, so the card has both numbers in one
    payload. No new event, no new rate. The Stats for Nerds fusion band
    (src/DeviceStats.tsx:118) shows "Trainer 248 W · Meter 241 W · +7 W
    (+2.9%)" plus a 30 s rolling mean from a small ring buffer the card feeds
    from its live `telemetry` prop — not from `telemetryHistory`, which only
    accumulates while a ride is running (src/App.tsx:300) and is empty during
    the pre-ride warm-up when riders compare the most.
11. **Sign convention is stated everywhere it appears.** Difference is always
    `trainer − meter`; positive means the trainer reads higher. The report
    struct carries the sentence, the live readout labels it, and the PNG footer
    prints it.

## The analysis

Inputs: stream A (Trainer), stream B (Power), the session's `started_at` and
`elapsed_seconds`, and the two `RideDevice`s. All constants below are fields of
the report so the image can print the rules it followed.

1. **Overlap.** `[max(firstA, firstB), min(lastA, lastB)]`. Under 60 s → the
   report is `Insufficient { reason, overlap_seconds }` and the UI says how much
   overlap there was. Coverage (ride seconds, overlap seconds, each source's
   seconds with data) is always reported so partial coverage is visible, not
   hidden.
2. **Lag by cross-correlation, at 250 ms.** Both streams are binned onto a
   250 ms grid over the overlap (mean of readings per cell, empty cells held
   for at most 1 s so a 1 Hz meter fills its cells). Mean-removed, normalized
   correlation at lags −10…+10 s in 250 ms steps over the cells both have; the
   peak is refined by parabolic interpolation and reported as fractional
   seconds. Positive lag means the trainer trails the meter. If either series
   has standard deviation under 5 W, the peak coefficient is under 0.5 (steady
   ERG has nothing to correlate), or the peak sits on the search boundary, the
   lag is reported as 0 with `confident: false` and the UI says lag could not
   be estimated. B's raw timestamps are shifted by the lag before anything else
   is computed, so alignment does not lose the sub-second part.
3. **Resample to 1 Hz.** For each grid second `t` in the overlap, a source's
   value is the mean of its readings in `(t − 1 s, t]`; an empty second takes
   the value interpolated at its centre between the readings either side of
   it when they are no more than 1.5 s apart (a 1 Hz meter with arrival
   jitter straddles bins), otherwise it is a gap. No multi-second hold: held
   values would be duplicates that flatter the 1 s spread. Cadence is
   resampled the same way.
4. **Coasting and power-step exclusion.** A second is excluded when either
   source is under 30 W (coasting) or when either source moved more than 50 W
   since the previous second (a power step), and so are the 3 s on each side:
   a trainer's flywheel decays power over several seconds after the rider
   stops, and it follows an ERG step a beat behind a crank meter, which is
   response time, not inaccuracy. Without the step rule, rising steps drop
   negative differences into higher power bins and falling steps drop
   positive ones into lower bins, and the trend line acquires a slope that no
   device error caused (this showed up in the synthetic tests as a spurious
   −1 %/100 W slope). Coasting, step and compared seconds are all reported.
5. **Three smoothing windows.** Excluded seconds are removed from both series
   first; then centered moving averages of 1 s (none), 3 s and 30 s, each
   needing at least half of the window that fits present, so a 30 s window
   straddling a coast averages only the compared seconds in it. Per window: count, mean
   difference in W, headline percent as ratio of sums (`Σa / Σb − 1`, so
   low-power pairs do not dominate), standard deviation of the difference,
   Bland-Altman limits of agreement (mean ± 1.96 sd), and the 95th percentile
   and maximum of |difference| in W and %, so one transient does not stand in
   for the spread. The 3 s window is the headline; 1 s shows the noise floor;
   30 s shows what a ride summary would see.
6. **Additive versus proportional.** A least-squares line of difference on
   mean power over the 3 s pairs gives `intercept_watts` and `slope_percent`:
   "+2 W plus 1.8% of power" and "+5 W flat" are different faults with
   different fixes, and the report says which it sees.
7. **Drift versus elapsed.** 3 s pairs binned per elapsed minute from
   `started_at`: mean W, mean %, count; bins with under 20 pairs are dropped.
   A `warmup` summary compares the first 10 minutes of compared time with the
   rest so "3% high then settles" is a number, not a squint.
8. **By power level.** 50 W bins of `mean(a, b)`; bins with under 30 pairs are
   suppressed, and the count is printed on every bin that survives.
9. **By cadence.** 10 rpm bins on the meter's cadence when it covers at least
   half the pairs (crank-based cadence is the interesting axis), otherwise the
   trainer's; the report names which. Same 30-pair floor.
10. **Traces and points for drawing.** The two 3 s-smoothed series over elapsed
    time (excluded seconds included, so the overlay shows the ride as ridden),
    evenly thinned to at most 900 points each, with each source's mean and
    maximum over the compared seconds; and the Bland-Altman `(mean, diff)`
    pairs thinned to at most 600.

Synthetic tests construct B as a known profile (ERG steps, ramps, sprints,
coasting) and A as `B × (1 + drift(t)) + offset`, delayed by a known lag,
smoothed with a short EMA, sampled at 4 Hz with seeded noise, and assert the
recovered lag (including a sub-second one), offset, warm-up drift, slope and
intercept for offset-only and scale-only cases, per-bin counts, exclusion counts
(the guard removes exactly 7 s per isolated coasting second), and the
insufficient and unconfident paths against the constructed values.

## Changes

### Rust: FTMS and trainer

- `ftms.rs`: `IndoorBikeData { timestamp_ms, power_watts: Option<u16>,
  cadence_rpm, speed_kph, heart_rate_bpm }`; `parse_indoor_bike_data` returns
  it. Tests cover a frame without bit 6.
- `trainer.rs`: the notification worker and the simulator produce
  `Reading::Trainer(IndoorBikeData)`; the simulator's target is carried by a
  `target_power_watts` field on the same struct so the fuser's
  `trainer_target_watts` behaviour is unchanged; `describe` takes the struct.

### Rust: fuser (src-tauri/src/devices/fuser.rs)

- `TelemetryFuser::new` takes a `broadcast::Sender<SourceSample>`; `ingest`
  sends one for `Reading::Trainer` (when the frame carries power or cadence,
  with `power_watts: None` for a cadence-only frame) and `Reading::Power` at
  device rate; a missing trainer power is never recorded as 0 W.
- `FusedTelemetry.power_by_source: BTreeMap<DeviceRole, u16>` filled in `fuse`.
- Tests: both roles emit through the throttle; the fused JSON carries
  `powerBySource`; a stale role leaves the map; a frame without power does not
  become 0 W.

### Rust: domain, devices hub, storage

- `domain.rs`: `SourceSample { role, timestamp_ms, power_watts, cadence_rpm,
  balance_left_percent }`, `RideDevice { role, id, name, transport, simulated,
  manufacturer, model, firmware, last_calibration }`.
- `devices/mod.rs`: `DeviceHub` owns the source channel and exposes
  `subscribe_sources()`; `DeviceSlot::ride_device()` builds a `RideDevice`;
  `CalibrationRecord.previous_offset_raw` with `#[serde(default)]`, set in
  `cycling_power::zero_offset`.
- `storage.rs`: `source_samples` and `session_devices` tables (both
  `ON DELETE CASCADE`, created in the same `execute_batch` as the others so a
  pre-existing database gains them on open); `record_source_samples` (one
  transaction, upsert), `source_samples(id)`, `record_session_device`,
  `session_devices(id)`. `delete_session` and the empty-orphan cleanup need
  nothing new thanks to the cascade.
- Tests: round trips, cascade on delete, opening a legacy database (extending
  src-tauri/src/storage.rs:2048), the calibration record with and without
  `previous_offset_raw`.

### Rust: runner (src-tauri/src/runner.rs)

- `Recorder<T: Recordable>`, `recorder_loop<T>`, `write_with_retries<T>`;
  `SampleSink` gains `write_source_samples` (required, so the test sinks state
  what they do with it); `Recordable` picks the sink method per type.
- `Ride` gains `sources_rx` and `source_recorder`; `run_timeline` adds a select
  arm that checks the gate against the other power role's slot state, pushes to
  the source recorder, snapshots a newly admitted role's `RideDevice`, and
  handles `Lagged` like the telemetry arm. `start_ride` records `RideDevice`s
  for every connected role. The end-of-ride sequence flushes telemetry first
  and captures its result, then flushes sources with a short grace and only
  logs.
- Tests: the gate; source samples are flushed with the ride; a sink whose
  source writes always fail still finishes with no warning; a saturated source
  recorder leaves telemetry `dropped == 0` and its flush green; a ride on the
  simulated trainer plus the simulated power meter stores both roles' source
  samples and both `RideDevice`s (the existing `Rig`,
  src-tauri/src/runner.rs:1836).

### Rust: analysis and commands

- `src-tauri/src/dual_power.rs`: `compare(a, b, &summary, devices) ->
  PowerComparison` with the algorithm above; `PowerComparison` is a
  `#[serde(tag = "status")]` enum of `Insufficient` and `Ready`.
- `commands.rs`: `get_power_comparison(id)` and `export_png(request)`; both
  registered in `lib.rs`. `percent-encoding` (already in the dependency tree)
  becomes a direct dependency for the path header.

### TypeScript

- `src/types.ts`: `SourceSample`, `RideDevice`, `PowerComparison` and its parts;
  `Telemetry.powerBySource?: Partial<Record<DeviceRole, number>>`;
  `CalibrationRecord.previousOffsetRaw?: number | null`.
- `src/api.ts`: `powerComparison(id)`, `exportPowerComparisonPng(session, png)`
  using the same save-dialog shape as the FIT export and a raw-body invoke.
- `src/dualPower.ts`: the sign-convention sentence, `liveDelta`, a
  `DeltaWindow` ring buffer for the 30 s rolling mean, and number formatting
  shared by the card and the painter.
- `src/powerComparisonReport.ts`: `Painter` interface, `paintPowerComparison`
  (header with ride and both devices; headline tiles; the two traces overlaid;
  drift-per-minute chart; Bland-Altman scatter with mean, limits and trend
  line; by-power and by-cadence bars with counts; windows table; footer with
  lag, exclusion rule, sign convention, calibration context and the blake.bike
  mark), `CanvasPainter`, and a `toPngBlob` helper. Laid out for a 1200 px wide
  logical page, exported at 2×.
- `src/PowerComparisonCard.tsx`: the modal section — textual summary, the
  canvas, an "Export PNG" button, a one-line reading guide, the
  insufficient-overlap note.
- `src/App.tsx`: an effect keyed on `selectedSession?.summary.id` fetches the
  comparison and ignores a response whose id no longer matches; it is passed to
  `RideDetailModal` (src/App.tsx:1569), which renders the card under the charts
  (src/App.tsx:1629) only when a report exists. Post ride and history both open
  the same modal, so both get it.
- `src/DeviceStats.tsx`: the fusion band adds the live delta line when
  `powerBySource` has both roles.
- `src/App.css`: `.power-comparison` block.

## Risks

- **Recorder regression.** Mitigated by the generic recorder: the telemetry
  instance is unchanged in behaviour, the source instance shares no channel,
  thread, transaction or health field with it, and a failing-sink test plus a
  saturation test assert the ride ends clean.
- **Lag estimate on steady rides.** ERG at a fixed target has no signal to
  correlate. The confidence flag and the 0 s fallback make that explicit rather
  than reporting a spurious lag.
- **Canvas in tests.** jsdom has no 2D context, so the painter is tested
  through the `Painter` interface with a recording fake; pixels are checked in
  the browser harness.
- **Payload size.** A 90 minute ride at 4 Hz + 1 Hz is ~27 k source rows; the
  analysis reads them once per modal open, off the main thread via `blocking`.
  The report itself is small (bins and ≤ 900-point traces).
- **Storage growth** only for dual-source rides, by roughly the size of the
  fused stream; single-source rides are unchanged.

## Tests

- `dual_power.rs`: synthetic known-answer tests for lag (integer and
  sub-second), offset-only and scale-only errors, drift, bins, exclusion,
  partial overlap, insufficient overlap, unconfident lag, cadence source
  choice, trace and Bland-Altman thinning.
- `ftms.rs`, `fuser.rs`, `storage.rs`, `runner.rs`, `devices/mod.rs`: as listed
  above.
- `src/dualPower.test.ts`: live delta, ring buffer, sign and formatting.
- `src/powerComparisonReport.test.ts`: the painted text includes both device
  names and firmware, the sign convention, the lag line, the exclusion rule,
  every surviving bin's count, and no suppressed bin; insufficient reports
  paint the reason.
- `src/PostRide.test.tsx` / `src/AppPostRide.test.tsx`: the modal shows the
  comparison when the API returns one and nothing when it returns `null`.
- `src/Ride.test.tsx`: the nerd card shows the delta line only with two sources.

## Verification

1. `pnpm check` and `pnpm test` (Node via nvm), `cargo test` in src-tauri.
2. Scratchpad Vite harness rendering `RideDetailModal` with a synthetic
   `PowerComparison` (and `Insufficient`) to eyeball the canvas and export.
3. Blake: connect the simulated trainer and simulated power meter in developer
   mode, ride a few minutes, open the ride, export the PNG.

## Out of scope

- Writing source streams into the FIT as developer fields (the table shape and
  `RideDevice` are designed for it).
- Cadence source comparison (reads `source_samples.cadence_rpm`, same
  analysis skeleton).
- Any change to which source feeds control or display.

## Review notes

An independent review of the first draft changed the plan in these ways:
the trainer-frame-without-power bug (decision 3) was found and is fixed here
rather than worked around; the gate moved from "reported within 3 s" to "the
other role has a device" (decision 4) so a Bluetooth hiccup cannot punch a hole
in the healthy stream; the second stream got its own recorder instance
(decision 5) after the shared-channel hazard was pointed out; the PNG gained
the overlaid traces, per-source averages, P95, ratio-of-sums percent and the
additive/proportional trend line; lag moved to a 250 ms grid with sub-second
refinement; the multi-second hold was dropped; the live delta got a ring buffer
instead of relying on ride-only history; and the PNG travels as a raw IPC body.

Implementation added one more rule the synthetic tests forced: excluding the
seconds around a power step (analysis step 4), after a 320 ↔ 160 W interval
profile produced a spurious negative slope in the additive/proportional fit
with nothing but response-time differences to cause it.
