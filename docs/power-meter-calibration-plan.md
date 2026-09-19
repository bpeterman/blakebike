# Power Meter Calibration (Zero Offset) — Plan

Goal: let the rider zero a Bluetooth power meter connected in the Power role
(pedals, crank, spider) from the Devices page, the same way the trainer card
already offers an FTMS spin-down. The app sends the Cycling Power Service
"Start Offset Compensation" procedure, shows the result, remembers the offset
per device so drift is visible over time, and hints when the meter itself asks
to be zeroed.

## Background: what "calibrating" a power meter means

A strain-gauge power meter does not run a spin-down. It has one user procedure,
**zero offset** (Favero, SRM, Quarq: "zero offset"; Garmin, Stages: "calibrate"),
which tells the meter "there is no load on me right now — treat this reading as
zero". Temperature changes and battery swaps move that zero point, so vendors
recommend doing it before each ride once the meter has reached room
temperature. The meter answers with a number in its own raw units (ADC counts
or Nm/32 for most brands); the absolute value means nothing to the rider, but a
jump from one session to the next is the standard sign that something is wrong
(a cleat dragging on the pedal, a loose crank, a dying battery).

Over Bluetooth this is the Cycling Power Service (0x1818) **Cycling Power
Control Point** (0x2A66), an indicate characteristic like the FTMS control
point:

| Item | Value |
| --- | --- |
| Feature characteristic | Cycling Power Feature 0x2A65, `u32` flags (read once) |
| Feature bit 8 | Offset Compensation Indicator supported (meter can *ask* for a zero) |
| Feature bit 9 | Offset Compensation supported (meter accepts the procedure) |
| Feature bit 19 | Enhanced Offset Compensation supported (CPS 1.1, richer response) |
| Measurement flag bit 12 | Offset Compensation Indicator: set in 0x2A63 notifications when the meter wants to be zeroed |
| Request | write `[0x0C]` (Start Offset Compensation) With Response; `[0x10]` for the enhanced form |
| Response | indication `[0x20, request_opcode, result, params…]`; result 0x01 Success, 0x02 Op Code Not Supported, 0x03 Invalid Parameter, 0x04 Operation Failed |
| Success parameter | `s16` offset in the meter's raw units; the enhanced form adds a manufacturer-specific length byte and bytes |
| ATT errors | 0x80 Procedure Already In Progress, 0x81 CCCD Improperly Configured (we forgot to subscribe) |
| Procedure timeout | the spec allows the meter 30 s; pedals answer in 2–5 s in practice |

Rider-side rules the UI must state, because the meter cannot check them:
unclip and keep the bike still, no weight on the pedals or cranks, meter at
room temperature (not straight from a cold garage), and for crank-based meters
the crank position the vendor specifies (Stages/4iiii: arm hanging straight
down; pedal meters and spiders do not care). A zero done under load "succeeds"
and silently biases every watt afterwards, so the copy leads with "unclip".

## Current state

- `devices/cycling_power.rs` parses 0x2A63 (power, balance, torque skip, wheel,
  crank) and drives cadence through `CrankCadence`. It ignores flag bit 12 and
  knows nothing about 0x2A65 / 0x2A66.
- `devices/sensor.rs` (`Sensor`) is **notify-only**: connect, subscribe to one
  measurement characteristic, decode. It keeps the `Peripheral` but exposes no
  way to write a characteristic or receive indications from a second one. The
  Power and Cadence roles are `Sensor`s; the trainer is its own `Trainer` type.
- `devices/trainer.rs` already implements the write-then-await-indication
  mechanics we need (`write_control_locked`: bounded write, discard late
  answers to earlier commands, abandon when the link goes down) plus the
  spin-down state machine, `trainer://calibration` progress events, the
  `calibrating` / `calibration_supported` flags on `SlotStats`, and a
  simulated path.
- `SlotStats.calibration_supported` / `calibrating` are role-agnostic by name
  but only the trainer sets them; the Devices page only draws the Calibrate
  button on the trainer card.
- `TrainerCalibrationModal` is hard-wired to the spin-down phases
  (`accelerate` / `stopPedaling`), a speed gauge, and "Cancel and disconnect".
- Known devices are stored as one JSON payload per id, replaced wholesale on
  every successful connect (`Storage::remember_device` upsert). Nothing about a
  device's calibration history is stored.
- The test fixtures name an Assioma; that is the pedal meter to verify against
  first. ANT+ support is heart-rate only, so ANT+ power meters (calibration
  page 0x01) are out of scope here.

## Decisions

1. **One calibration entry point for every role.** `DeviceHub::calibrate(role)`
   replaces `calibrate_trainer`; the Tauri command becomes `calibrate_device
   { role }` and `api.calibrateTrainer()` becomes `api.calibrateDevice(role)`.
   The trainer path keeps its behaviour; the old names go away in the same
   change rather than living on as wrappers. When ANT+ power arrives, its
   calibration page plugs into the same entry point.
2. **One progress event, `devices://calibration`, replacing
   `trainer://calibration`.** Payload carries `role`, `phase`, `message`, and a
   tagged `detail`: `{ kind: "spinDown", targetLowKph, targetHighKph }` or
   `{ kind: "zeroOffset", offsetRaw, previousOffsetRaw }`. Phases stay a
   flat string union so the modal can share its shell: existing
   `preparing | accelerate | stopPedaling | success | error` plus `holdStill`
   for the meter.
3. **Control-point mechanics are shared, not copied.** Extract the
   write-and-await-indication core of `Trainer::write_control_locked` into
   `devices/control_point.rs` (`run_procedure`: write future, response
   receiver, `matches(&[u8]) -> bool`, write/ack timeouts, link-down watch).
   `Trainer` and `Sensor` both call it; `ControlError` gains nothing new, the
   trainer's behaviour is preserved by its existing tests. The alternative
   (duplicating ~80 lines of timeout and stale-answer handling in `Sensor`) is
   exactly the drift the project rule forbids.
4. **`Sensor` grows an optional control point, declared by its `Decoder`.**
   `Decoder::control_point() -> Option<ControlPointSpec { characteristic,
   feature, name }>` (default `None`). On connect, when the decoder declares
   one and the peripheral exposes it, `Sensor` reads the feature
   characteristic (handed to `decoder.features(&[u8])`), subscribes to the
   control point's indications, routes them to a `broadcast::Sender<Vec<u8>>`,
   and offers `Sensor::procedure(payload) -> Result<Vec<u8>, ControlError>`
   serialised by a `command_lock`. Heart rate and cadence decoders declare
   nothing and behave exactly as today.
5. **The zero-offset state machine lives next to the CPS protocol,** in
   `devices/cycling_power.rs` (pure parsing) plus a `zero_offset(sensor,
   fuser, cancel)` orchestrator in the same module. `Sensor` stays
   kind-agnostic. `DeviceHub::calibrate(DeviceRole::Power)` calls it.
6. **Availability mirrors the trainer:** the button is enabled when the meter
   is connected, exposes 0x2A66, sets feature bit 9, is not already
   calibrating, and no ride is recording (`DeviceHub::ride_active`). If
   hardware verification shows the Assioma (or another common meter) leaves
   bit 9 clear while answering 0x0C, relax the gate to "control point present"
   and log a warning line. Zeroing during a *paused* ride is a listed
   follow-up, not v1.
7. **The backend refuses to zero a moving meter.** Before writing, the
   orchestrator asks the fuser for the Power role's latest reading; cadence
   > 0 or watts > 0 within the last 3 s returns a clear error ("Meter is still
   moving (84 rpm). Unclip, stop the cranks and try again"). Meters that keep
   reporting 0 W while coasting pass; that is fine, this is a guard against
   the common mistake, not a proof of no load.
8. **Offsets are remembered per device in their own column.** `known_devices`
   gains `last_calibration_json` (`{ at, kind: "zeroOffset", offsetRaw }`),
   written by `Storage::record_calibration(id, record)` and left untouched by
   `remember_device`'s upsert. `KnownDevice` exposes it as
   `lastCalibration: Option<…>`. Persistence stays in the command layer (the
   hub never sees storage, as today); the UI joins the connected card to its
   known-device row by id to show "Last zeroed 2 h ago · offset 1023".
9. **The meter's own request is surfaced.** Measurement flag bit 12 becomes
   `CyclingPowerMeasurement.offset_compensation_requested`, the decoder passes
   it to `SlotStats.calibration_requested`, and the Power card shows an amber
   "Meter requests zeroing" hint. Cheap, spec-correct, and the only time the
   meter tells us it has drifted.
10. **Cancel does not disconnect.** There is no "stop offset compensation"
    opcode and the procedure is bounded by 30 s, so Cancel just stops waiting
    (the late indication is discarded as stale by the shared helper). The
    trainer keeps "Cancel and disconnect" because its procedure holds control.

## Backend (Rust)

### Phase 1 — Protocol (pure, tested) in `devices/cycling_power.rs`

- Constants `CYCLING_POWER_FEATURE = 0x2A65`, `CYCLING_POWER_CONTROL_POINT =
  0x2A66`; `ControlOpcode { StartOffsetCompensation = 0x0C,
  StartEnhancedOffsetCompensation = 0x10, ResponseCode = 0x20 }`;
  `ResponseValue { Success, OpCodeNotSupported, InvalidParameter,
  OperationFailed, Unknown(u8) }`.
- `CyclingPowerFeature::parse(&[u8]) -> Result<Self, CpsError>` with
  `offset_compensation`, `offset_compensation_indicator`,
  `enhanced_offset_compensation` booleans (4-byte little-endian, truncated is
  an error).
- `parse_cycling_power_measurement` gains `offset_compensation_requested`
  (flag bit 12). Existing tests updated; no behaviour change otherwise.
- `start_offset_compensation() -> Vec<u8>` and
  `parse_offset_compensation_response(data, requested_opcode) ->
  Result<ZeroOffset { raw: i16, vendor_bytes: Vec<u8> }, CpsError>`. Errors:
  `Truncated`, `UnexpectedResponse` (wrong header or opcode), `Rejected {
  opcode, result }`. The enhanced form reads the length byte and copies the
  vendor bytes for the log only.
- `CpsError: Display`, mapped to `ControlError` where the trainer does the
  same for `FtmsError`.
- Tests: feature word with and without bits 8/9/19; success `[20 0C 01 FF 03]`
  → 1023; negative offsets; each rejection code; wrong opcode; truncated
  parameter; enhanced response with 3 vendor bytes; measurement with flag 12
  set and clear.

### Phase 2 — Shared control-point helper and `Sensor` procedures

- New `devices/control_point.rs`: `pub async fn run_procedure(...)` carrying
  today's rules from `Trainer::write_control_locked` verbatim: bounded write
  (`CONTROL_WRITE_TIMEOUT`), drain queued responses before waiting, wait for
  the first response the `matches` closure accepts within
  `CONTROL_ACK_TIMEOUT` (parameterised; CPS passes 30 s), return
  `ControlError::NotConnected` the moment the slot's state says the link is
  down. `Trainer::write_control_locked` becomes a thin caller; its unit tests
  (simulated faults: `fail_writes`, `ack_delay_ms`, `refuse_with`) must pass
  unchanged.
- `Decoder` trait: `fn control_point(&self) -> Option<ControlPointSpec>
  { None }` and `fn features(&mut self, _bytes: &[u8]) {}`. `PowerDecoder`
  declares `{ characteristic: 0x2A66, feature: Some(0x2A65), name: "Cycling
  Power Control Point" }` and stores the parsed feature.
- `Sensor::connect_inner`: after locating the measurement characteristic, if
  the decoder declares a control point: read the feature characteristic
  (optional, logged "Power features · offset compensation supported / not
  advertised"), subscribe to the control point (logged "Control point armed"),
  remember the `Characteristic`, and call
  `slot.set_calibration(Some(supported), Some(false))`. Missing control point
  is not an error: log "No control point · calibration unavailable" and set
  `calibration_supported = false`. The worker forwards notifications on the
  control-point UUID to `procedure_responses` instead of tracing them as
  "Other notification".
- `Sensor::procedure(&self, payload: &[u8], matches: impl Fn(&[u8]) -> bool,
  ack_timeout: Duration) -> Result<Vec<u8>, ControlError>` under a new
  `command_lock: Mutex<()>`; the simulated path (no peripheral) hands the
  payload to `Decoder::simulate_procedure(payload) -> Option<Vec<u8>>` (default
  `None` → `OpCodeNotSupported`) after a short delay.
- `Sensor::disconnect` clears the control-point handle; `reset` on reconnect
  re-reads features (a firmware update can change them).
- Tests (headless hub, simulated decoder): procedure succeeds; stale queued
  response is discarded; refused response maps to `ControlError::Refused`;
  timeout; link-down mid-procedure returns `NotConnected`; HR/cadence
  decoders' `procedure` returns `Unsupported` without touching BLE.

### Phase 3 — Zero-offset orchestrator and hub entry point

- `cycling_power::zero_offset(sensor: &Sensor, fuser: &TelemetryFuser,
  cancel: &Notify, emit: impl Fn(CalibrationProgress)) -> Result<ZeroOffset,
  String>`:
  1. State must be `Ready` (else "Connect a power meter before zeroing");
     `calibration_supported` must be true (else "This power meter does not
     advertise offset compensation"); `calibrating` CAS false→true (else
     "already running"). `DeviceHub::calibrate` rejects while
     `ride_active()` with the same wording the trainer uses.
  2. Movement guard (decision 7) via a new `TelemetryFuser::latest(role,
     now_ms) -> Option<Reading>`.
  3. `slot.set_calibration(None, Some(true))`, log "Zero offset started", emit
     `preparing` ("Unclip and keep the bike still.").
  4. Emit `holdStill` and call `sensor.procedure(start_offset_compensation(),
     matches = response header 0x20/0x0C, 30 s)`. Use the enhanced opcode only
     when feature bit 19 is set and fall back to 0x0C on `OpCodeNotSupported`.
  5. Parse; on success log "Zero offset complete · 1023 (was 1019)", emit
     `success` with `{ kind: "zeroOffset", offsetRaw, previousOffsetRaw }`
     (previous comes from the slot's `last_calibration`, seeded by the command
     layer at connect from the known-device row; see phase 5). On rejection
     or timeout emit `error` with a rider-facing sentence per result code
     (OperationFailed → "The meter refused the zero. Make sure nothing is
     touching the pedals and try again"; timeout → "The meter did not answer
     within 30 s"). Always clear `calibrating`.
  6. `cancel.notified()` in a `select!` returns "Zeroing was cancelled";
     `Sensor::disconnect` and `DeviceHub::disconnect_role(Power)` also fire it.
- `DeviceHub::calibrate(role) -> Result<CalibrationOutcome, String>` matches on
  role: Trainer → `trainer.calibrate()` (returns `SpinDown`), Power →
  `zero_offset(...)` (returns `ZeroOffset`), HeartRate/Cadence → Err
  ("<role> has no calibration procedure"). `CalibrationProgress` moves from
  `trainer.rs` to `mod.rs`, gains `role` and `detail`, and is emitted as
  `devices://calibration` by both paths. `SlotStats` gains
  `calibration_requested: bool` and `last_calibration: Option<CalibrationRecord>`.
- Command `calibrate_device(role)` replaces `calibrate_trainer`; on
  `Ok(ZeroOffset)` it calls `storage.record_calibration(device_id, record)`
  and pushes the record back into the slot so the next zero shows the delta.
- Simulated power meter: `simulate_procedure([0x0C])` answers `[20 0C 01 lo
  hi]` with an offset that walks ±3 counts per call around 1020 after ~1.5 s,
  so the drift line has something to show in dev mode. A `debug_inject_power_fault`
  command (debug builds, beside `debug_inject_trainer_fault`) can set
  `respond_with` (result code) and `ack_delay_ms` to rehearse the refused and
  timed-out paths.
- Tests: the full orchestrator against the simulated meter (success with
  previous offset, moving-meter guard, refused, timeout, cancel, disconnect
  mid-procedure, blocked during ride); `DeviceHub::calibrate` dispatch per
  role; `calibration_requested` flows from a flagged measurement to the slot.

### Phase 4 — Storage

- Migration: `ALTER TABLE known_devices ADD COLUMN last_calibration_json TEXT`
  (guarded like the existing `CREATE TABLE IF NOT EXISTS` block; SQLite has
  no `ADD COLUMN IF NOT EXISTS`, so check `pragma_table_info` first).
- `Storage::record_calibration(id, &CalibrationRecord)` updates only that
  column; `known_devices()` reads both columns and fills
  `KnownDevice.last_calibration` (serde default `None`, so old payload JSON
  keeps deserialising). `remember_device` is unchanged and therefore cannot
  wipe the record.
- `forget_device` / `forget_all_devices` drop the row and with it the record;
  the undo path (`restore_known_devices`) must restore the record too, so
  `restore_known_devices` writes both columns.
- Tests: record then reconnect keeps the offset; forget + restore keeps it;
  legacy payload without the field loads.

## Frontend (React)

### Phase 5 — API, types, modal, Devices page

- `types.ts`: `CalibrationPhase` union adds `holdStill`; `CalibrationProgress
  { role, phase, message, detail: SpinDownDetail | ZeroOffsetDetail | null }`;
  `SlotStats` adds `calibrationRequested`, `lastCalibration`; `KnownDevice`
  adds `lastCalibration`; `CalibrationRecord { at, kind, offsetRaw }`.
- `api.ts`: `calibrateDevice(role)`, `onCalibrationProgress` listens to
  `devices://calibration`. `calibrateTrainer` and the `trainer://calibration`
  listener are removed.
- `TrainerCalibrationModal.tsx` → `CalibrationModal.tsx` taking `role` and
  `slot`. Shared shell (icon, phase heading, message, actions, `useDialog`
  close rules); per-role content:
  - Trainer: unchanged copy, speed gauge, target range from `detail`,
    "Cancel and disconnect".
  - Power: label "ZERO OFFSET"; idle copy: warm the meter to room
    temperature, unclip, bike upright and still, nothing on the pedals or
    cranks, crank-arm position note for crank-based meters, "takes a few
    seconds". Live panel shows the card's `lastReading` ("0 W · 0 rpm" is what
    the rider wants to see) and, once `success` arrives, the offset with the
    delta versus `previousOffsetRaw` ("Offset 1023 · was 1019 · drift +4") and
    a one-line reading of it (|drift| ≤ ~2 % of the previous value → "steady";
    larger → "large change: check for a dragging cleat or loose crank and
    zero again"). Plain "Cancel" while active, per decision 10.
  - Modal filters progress events to its `role` so a trainer spin-down and a
    meter zero cannot cross-talk.
- `DevicesPage.tsx`: the Calibrate button renders for `trainer` and `power`
  with the same gating expression fed by the slot's stats; tooltip and the
  "unavailable" `<small>` lines take role-specific wording ("This power meter
  does not advertise offset compensation"). Power card adds a "Last zeroed
  {relative time} · offset {n}" line from the joined known-device row and an
  amber "Meter requests zeroing" hint when `calibrationRequested`. Button
  label "Zero offset" on the Power card, "Calibrate" on the trainer.
- `App.tsx`: `calibrationOpen: DeviceRole | null`; `onCalibrate(role)`.
- Tests: `CalibrationModal.test.tsx` covers both roles (existing trainer
  assertions ported; power: begin → holdStill → success shows offset and
  drift; refused → "Try again"; Cancel does not call disconnect);
  `DevicesPage.test.tsx` fixture gains a power slot with
  `calibrationSupported` true/false, `calibrationRequested`, and a known-device
  row with `lastCalibration`, asserting button state and the two hint lines.

### Phase 6 — Pre-ride nudge (small, optional, after hardware verification)

When a ride is about to start and the fused power source is the Power role,
and its known-device row has no `lastCalibration` or one older than 24 h, show
a dismissible line on the ride start card: "Zero your power meter before
riding?" with a button that opens the modal. No blocking, no modal on its own.

## Hardware verification

Section **H. Power meter zero offset** in `docs/devices-hub-verification.md` (G is the trainer spin-down):

- [ ] **H1 · Features read.** Connect the Assioma to the Power role. Card log
      shows "Power features · offset compensation supported" and "Control
      point armed"; the Zero offset button is enabled. If the log says "not
      advertised" but the Favero app can zero the pedals, apply decision 6's
      relaxation and note the feature word from the log.
- [ ] **H2 · Clean zero.** Unclip, bike still, press Zero offset → Begin.
      Expect `holdStill` within a second and `success` within ~5 s with an
      offset; repeat twice, the value should move by only a few counts.
      Compare against the value the Favero app reports for the same pedals.
- [ ] **H3 · Moving meter.** Begin while turning the cranks: the guard refuses
      with the "still moving" message and nothing is written (log shows no
      "Control write").
- [ ] **H4 · Loaded meter.** Stand on a pedal and zero: note whether the meter
      answers `OperationFailed` or a wildly different offset; the drift line
      must flag the latter.
- [ ] **H5 · Persistence.** Relaunch; the Power card shows "Last zeroed … ·
      offset …" before connecting (from Known devices) and after. Zero again:
      the modal shows the delta against the stored value.
- [ ] **H6 · Ride guard.** Start a free ride; the button disables with the
      ride wording. End the ride; it re-enables.
- [ ] **H7 · Trainer unaffected.** Trainer spin-down still works end to end
      after the control-point refactor, on the real trainer and the simulator.
- [ ] **H8 · Indicator.** Leave the pedals cold from the garage and connect;
      if the meter sets flag 12 the card shows "Meter requests zeroing" and
      the hint clears after a successful zero.

## Out of scope / follow-ups

- ANT+ power meters (calibration page 0x01: 0xAA request, 0xAC/0xAF
  response) — plugs into `DeviceHub::calibrate(Power)` once ANT+ power exists.
- Other CPS procedures (crank length, sensor location, cumulative value) —
  `Sensor::procedure` makes them one parser and one button each.
- Zeroing during a paused ride.
- Auto-zero detection (some meters zero themselves while coasting; nothing to
  do in the app beyond the indicator).
- Trainers that also advertise CPS with a control point: the Trainer role
  keeps using FTMS spin-down; a trainer assigned to the Power role gets the
  zero-offset button like any meter.

## Changelog entry (Unreleased → Added)

"Zero your power meter from its Devices card. The app sends the standard
offset-compensation command, refuses to zero while the cranks are moving,
shows the offset and how far it drifted since last time, remembers it per
device, and points out when the meter itself asks to be zeroed."

## Status

- Plan written 2026-09-19.
- Phases 1–6 built 2026-09-19 in one change: `devices/control_point.rs` (shared `run_procedure`, `ControlError` moved here with a neutral `Unsupported` variant); `Sensor` grew a decoder-declared control point (feature read, indication routing, `procedure`, cancel, `SensorFaults`); `cycling_power.rs` has the feature word, opcodes, response parsing, indicator flag, `movement_blocker` and the `zero_offset` orchestrator; `DeviceHub::calibrate(role)` / `cancel_calibration(role)` replace `calibrate_trainer`; one `devices://calibration` event with `role`, `phase` and a tagged `detail`; `CalibrationRecord` on `SlotStats` and `KnownDevice`, kept in a `last_calibration_json` column; commands `calibrate_device`, `cancel_calibration`, `debug_inject_device_fault` (replaces the trainer-only one). Frontend: `CalibrationModal` (renamed from `TrainerCalibrationModal`) with per-role copy, live meter reading, offset + drift verdict; Devices cards offer Zero offset, show "Zeroed … · offset …" and "Meter requests zeroing"; remembered-device rows show the last zero; the Ride start page nudges when the feeding meter is due. 188 Rust tests, 151 vitest.
- Deviations from the plan: the enhanced (0x10) procedure is not attempted, plain 0x0C always (the extra bytes would only be logged); the movement guard is skipped for the simulator (it pedals most of the time) and covered by unit tests on the pure function; a spin-down success is recorded too, so the trainer card gets a "last calibrated" line for free; cancel is a hub command (`cancel_calibration`) rather than a modal-only concern.
- Next: hardware section H in `docs/devices-hub-verification.md` against the Assioma, and revisit decision 6 if it leaves feature bit 9 clear.
