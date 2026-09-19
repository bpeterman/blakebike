# Devices Hub — Hardware Verification

Run this on both target platforms (macOS and Ubuntu/Pop!_OS). Every step says
what to do, what the Devices page should show, and what to look for in the
per-device log or `blakebike.log` if it does not. Tick the boxes as you go and
note anything odd next to the step; the log file path is under Settings →
Data & Diagnostics.

Before starting, launch with the trace filter once so raw BLE packets are in
the log if anything needs a closer look:

```sh
RUST_LOG=trace pnpm tauri dev
```

## A. Discovery

- [ ] **A1 · Scan finds every device.** Wake all four (pedal the trainer, wear the strap, spin the crank for the power meter and cadence sensor), open Devices → any card → Connect. Expected: the picker for that role lists only devices advertising a matching service, with capability chips (FTMS / Power / Cadence / Heart rate) and a dBm figure. The trainer should appear for the Trainer role; if it also advertises Cycling Power it should appear for Power and Cadence too.
- [ ] **A2 · Nothing missing.** Compare against what the vendor app or `bluetoothctl scan on` (Linux) sees. If a device is missing, grep the log for `Saw peripheral` around the scan time — the trace line lists every advertised service UUID, which tells you whether the device advertises the standard service or only a proprietary one.
- [ ] **A3 · Bluetooth off.** Turn Bluetooth off, scan. Expected: a red error banner on the Devices page with platform guidance; the simulators still listed in the picker when developer mode is on, and no devices at all when it is off.

## B. Connect each role

For each of Trainer, Heart rate, Power meter, Cadence sensor:

- [ ] **B1 · Connect from the picker.** Expected in the readout: Link established → Services discovered → the role's measurement characteristic → Device information (make · model · fw · battery) → Notifications armed → (trainer only) Control granted. Card flips to Connected (trainer: ERG control once a ride starts), battery / signal / uptime populate, and "First sample received" lands in the card log within a few seconds with a plausible value.
- [ ] **B2 · Manufacturer shown.** The card shows make · model under the device name. If it shows nothing, expand the card log: "No battery or device information" means the device does not expose the Device Information service (some cheap sensors don't) — note the device so we can add a name-based fallback.
- [ ] **B3 · Rate is sane.** Card shows roughly 1 Hz for HR straps, 1–4 Hz for power meters and cadence sensors, 2–4 Hz for trainers. A rate near 0 with samples counting up means timestamps are wrong; a high "bad packets" count means the parser and this device disagree — the log line carries the raw hex, copy it.
- [ ] **B4 · Values match the vendor display.** Power within a few watts of the head unit / vendor app, cadence within 1–2 rpm, heart rate within 1–2 bpm. For power meters check pedal balance shows (L/R) when the meter reports it.
- [ ] **B5 · Cadence goes to zero when you stop pedalling** (power meter and cadence sensor). Expected: within ~2 s the reading shows "0 rpm · coasting", and resumes correctly when you pedal again. If it sticks at the last value, note the device; its event-time behaviour differs from the spec.

## C. All four at once

- [ ] **C1 · Four connected.** Header pill shows four green dots and "4 of 4 connected". No card shows drops or bad packets after two minutes idle.
- [ ] **C2 · Sources panel.** With everything on Auto: Power "from <power meter>", Cadence "from <cadence sensor>", Heart rate "from <strap>". The trainer card says it is feeding nothing (or only speed).
- [ ] **C3 · Ride screen.** Start a free ride. Tiles show "via power meter" / "via cadence sensor" / "via heart rate". ERG targets still reach the trainer (adjust manual power and watch the trainer respond).
- [ ] **C4 · Pinned source.** Set Power to the trainer explicitly. Expected: Power tile switches to the trainer's watts, label "via trainer", no fallback marker. Set back to Auto.

## D. Drop-outs and fallback

- [ ] **D1 · Strap off.** Take the strap off / move it out of range. Expected within ~5 s: Heart rate source shows "falling back to <trainer>" if the trainer reports HR, otherwise "no data" and the ride tile shows "—". The strap card goes to "Link lost" with a red chip, drops = 1, and a "Link lost" line in its log.
- [ ] **D2 · Power meter asleep.** Stop pedalling long enough for the meter to sleep (or pull a battery). Expected within ~3 s: Power "falling back to <trainer>", Cadence falls to the cadence sensor (still Auto) — the trainer should not steal cadence while the dedicated sensor is alive.
- [ ] **D3 · Reconnect manually.** Wake the device, press Reconnect on its card (or Connect in Known devices). Expected: log shows "Looking for remembered device" → "Device found" → normal connect steps, sources return to the dedicated sensor, no duplicate rows in Known devices.
- [ ] **D4 · Trainer drop mid-ride.** Power-cycle the trainer during a free ride. Expected: the ride clock keeps running and other sensors keep recording; the ride screen shows "Trainer link lost" and the trainer card "Link lost · reconnecting (attempt n)"; the app reconnects on its own once the trainer is back (log: "Reconnect attempt" → normal connect steps → "Automatic reconnect succeeded"), then sends Start/Resume and the current target; the banner clears. No "Workout aborted" in the log; the ride ends normally with a complete History entry and FIT file.
- [ ] **D5 · Fault rehearsal without hardware (debug builds).** With the simulator connected and a ride running, invoke `debug_inject_trainer_fault` with `failWrites` (count 3) and then `dropLink`. Expected: "Trainer not acknowledging targets" then clear; "Trainer link lost" then the simulator reconnects within a few seconds and the banner clears.

## E. Known devices and persistence

- [ ] **E1 · Remembered.** After connecting all four, quit and relaunch. Devices page lists all four under Known devices with make · model and "last used …", none connected.
- [ ] **E2 · One-click connect.** Press Connect on a remembered device that is awake, without scanning first. Expected: connects within ~10 s via the targeted scan. Press Connect on one that is asleep: "Device not found" after 10 s with the wake-it-up guidance, card in Error state, no crash.
- [ ] **E3 · Forget.** Forget one device: row disappears, the device stays connected if it was. Settings → Forget all devices: list empties. Reconnecting a forgotten device re-adds it.
- [ ] **E5 · Connect all.** With four remembered devices and none connected, press Connect all. Expected: the roles connect one after another (trainer first), each card moving through Connecting → Connected; the button reads "Connecting…" meanwhile and is disabled once every remembered role is connected. Leave one sensor asleep: the others still connect, a calm line names the sleeping one ("… didn't answer, probably asleep …") with no red error banner, and pressing Connect on its card once it is awake works. A role that is already connected is left untouched.
- [ ] **E4 · Source preference survives relaunch.** Pin a source, relaunch, check the selector still shows it.

## F. Recovery and shutdown

- [ ] **F1 · Quit mid-ride.** With all four connected and a ride running, quit the app. Relaunch: ride recovery still works as before, all slots idle, no stale "connected" state.
- [ ] **F2 · Linux: BlueZ restart.** `sudo systemctl restart bluetooth` while connected. Expected: all cards go to Link lost; the next scan or reconnect succeeds (the hub recreates the adapter after a "Channel closed" error). Check the log for `Discarding cached Bluetooth adapter`.
- [ ] **F3 · macOS: Bluetooth permission.** Fresh install path only: first scan prompts for Bluetooth access; denying shows the guidance banner; allowing in System Settings and rescanning works without a relaunch.

## G. Trainer calibration

- [ ] **G1 · Capability gating.** Connect a trainer that advertises Target Setting Features bit 15. Expected: its ready card enables Calibrate. A trainer without the bit keeps the action disabled and explains that FTMS spin-down is unavailable.
- [ ] **G2 · Target-speed guidance.** Open Calibrate, review the safety guidance, and begin. Expected: the modal shows the low/high speed range returned by the trainer, updates current speed from Indoor Bike Data, and prompts you to pedal into that range.
- [ ] **G3 · Coast-down.** When the trainer sends Spin Down Status `Stop Pedaling`, expected: the prompt changes immediately. Stop pedalling and wait; a success status shows Calibration complete and adds start/completion lines to the trainer card log.
- [ ] **G4 · Errors and retry.** Force a failed or timed-out calibration if the vendor app provides a test path. Expected: the modal shows an actionable error, the card leaves its calibrating state, and Try again can start a fresh procedure.
- [ ] **G5 · Disconnect cancellation.** Start calibration, then disconnect or power off the trainer. Expected: calibration exits with a cancellation/link error rather than hanging, and reconnect remains available.
- [ ] **G6 · Workout exclusion.** Start a free ride or workout so the card reads ERG control. Expected: Calibrate is disabled with an in-workout explanation and calibration cannot interrupt target-power commands.
- [ ] **G7 · Simulator.** Connect BlakeBike Simulator and run calibration. Expected: accelerate → stop pedalling → success completes deterministically without hardware.

## Reporting

For each failure note: platform, device (make/model/firmware from the card), the step, and the timestamp. Then grab the surrounding log:

```sh
grep -n "WARN\|ERROR" ~/Library/Logs/com.bpeterman.blakebike/blakebike.log | tail -50   # macOS
grep -n "WARN\|ERROR" ~/.local/share/com.bpeterman.blakebike/logs/blakebike.log | tail -50   # Linux
```

The card's own log (expand Log on the card) usually has the shorter story; the file has the raw packets.
