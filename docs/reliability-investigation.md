# Ride reliability investigation

Investigated September 19, 2026, against commit `eefe6366d1089ea9d7e965398f6e3ef7f70e9dcc`.

The app has concrete failure paths that can interrupt trainer control or lose ride data while its window and ride timer continue working. Eight undesirable behaviors were reproduced with isolated tests. No spontaneous native-process crash was reproduced, and this investigation does not establish a failure rate during ordinary riding.

During the original investigation, application source was not changed. Its tests and fault injection ran in a disposable copy using simulated devices and disposable databases.

## Follow-up implementation

This change addresses R1, R3, and R5, and the flush-queue timeout in R6:

- Real Bluetooth connections are checked every five seconds with a four-second check timeout, independently of notification delivery. Repeated trainer transport failures also request reconnection. Quiet connected sensors are not treated as disconnected.
- Pause remains unconfirmed until acknowledged and retries after failures. Reconnecting or reacquiring control during a pause does not issue Start/Resume.
- Recording errors and sample drops appear during the ride. Temporary write errors clear after successful persistence; actual sample-loss warnings remain. Finalization problems appear in the completion prompt and are retained in History when the database is writable. A failed flush cannot silently count as a successful save, and it does not create a stale FIT snapshot.
- Flush queueing and acknowledgement share a deadline and do not block async runtime threads.

Validation: 146 backend tests and 56 frontend tests pass, including 18 new regression tests. Frontend type/lint checks and build, Rust formatting, and Clippy pass. Physical-trainer and operating-system power-management validation remain outstanding. R2, R4, R7, and R8 are outside this follow-up's scope.

The findings and reproduction patch below describe the original investigation at the recorded commit, before these changes.

## Priority and evidence

P1 means address before relying on automatic ride recovery and control. P2 means a narrower reliability or data-integrity problem. These are remediation priorities, not estimates of how frequently a failure occurs.

| ID | Priority | Finding | Evidence |
| --- | --- | --- | --- |
| R1 | P1 | A silent Bluetooth disconnect can leave the app permanently retrying a dead connection without reconnecting | Dependency source inspection and 60-second simulated failure |
| R2 | P1 | Concurrent Start requests create two rides sharing control state | Two simultaneous starts both returned different successful session IDs |
| R3 | P1 | Recording can fail completely while the app reports successful completion | Every sample insert failed; zero samples persisted; runner reported Finished/completed |
| R4 | P1 | Ride-task panic recovery leaves trainer control active | Injected panic: no Stop command, intent still Running, ride_active still true |
| R5 | P1 | One failed pause command is never retried while paused | One transient failure; no second attempt after 20 simulated seconds |
| R6 | P2 | Recording flush can block beyond its stated deadline | A 20 ms grace took 301 ms because enqueueing blocked first |
| R7 | P2 | Moving the system clock backward suppresses telemetry | A 60-second rollback suppressed all fused samples until time caught up |
| R8 | P2 | Paused rides continue recording samples into ride statistics | Three samples persisted after the paused state was reached |

## R1 — Reconnection depends on a notification stream ending

The trainer notification worker awaits the next notification indefinitely. The device hub starts reconnection only after a slot becomes `Reconnecting`. Repeated GATT failures change the runner's control status to `Lost`, but do not change the device slot or request reconnection.

This distinction matters on macOS. In the installed `btleplug 0.13.1` source, CoreBluetooth's peripheral event handler ignores `PeripheralEventInternal::Disconnected` for notification-stream closure. Notifications use a broadcast sender retained in shared peripheral state. The application does not subscribe to adapter disconnect events or run a connection-health watchdog during a ride. A physical disconnect therefore need not produce the stream termination that the existing simulator disconnect test assumes.

The probe stopped simulated notifications without changing connection state and made subsequent writes fail. After the runner reached `Lost`, another 60 simulated seconds passed with no reconnect attempt; the slot still reported connected. A physical trainer disconnect was not tested, so real-hardware reproduction remains necessary.

**Impact:** power data may freeze or disappear and resistance control may remain unavailable for the remainder of a ride, despite a running clock. Sensors use a similar notification-loop pattern.

**Fix direction:** consume transport disconnect events, monitor connection/telemetry health using monotonic time, and route persistent transport failures through a single reconnect transition. Test an open-but-silent stream, not just EOF. Distinguish legitimate sensor silence from transport loss.

Code: [notification loop](https://github.com/bpeterman/blakebike/blob/eefe6366d1089ea9d7e965398f6e3ef7f70e9dcc/src-tauri/src/devices/trainer.rs#L461), [reconnect gate](https://github.com/bpeterman/blakebike/blob/eefe6366d1089ea9d7e965398f6e3ef7f70e9dcc/src-tauri/src/devices/mod.rs#L997), [failure classification](https://github.com/bpeterman/blakebike/blob/eefe6366d1089ea9d7e965398f6e3ef7f70e9dcc/src-tauri/src/runner.rs#L1246). Dependency evidence: `btleplug-0.13.1/src/corebluetooth/peripheral.rs`, disconnect handler at line 189 and notifications method at line 600.

## R2 — Ride startup is not serialized

Startup reads the current runner state, then awaits trainer control before publishing a running state. Two calls can both observe Idle and both proceed. The worker mutex protects replacement of a handle, not the whole startup transition. The frontend Start buttons are not disabled while their requests are pending.

The probe issued two starts concurrently with a 100 ms simulated acknowledgement delay. Both succeeded with distinct session IDs. Each starts its own timeline, recorder, and target writer, while sharing controls and the visible runner state. One worker handle replaces the other.

**Impact:** a double click or repeated Start request can create competing rides. They can overwrite visible state and target intent; completion of one sends Stop through state shared with the other. The concurrent creation is reproduced; the complete range of downstream interleavings has not been enumerated.

**Fix direction:** serialize the entire start/stop lifecycle, reserve a Starting state before awaiting, and give each ride its own controls and owned workers. Disable pending Start actions in the UI as an additional guard.

Code: [startup](https://github.com/bpeterman/blakebike/blob/eefe6366d1089ea9d7e965398f6e3ef7f70e9dcc/src-tauri/src/runner.rs#L386), [worker replacement](https://github.com/bpeterman/blakebike/blob/eefe6366d1089ea9d7e965398f6e3ef7f70e9dcc/src-tauri/src/runner.rs#L540), [Start controls](https://github.com/bpeterman/blakebike/blob/eefe6366d1089ea9d7e965398f6e3ef7f70e9dcc/src/App.tsx#L1139).

## R3 — Recording failures are not reflected in ride success

The recorder retries failed writes, but its flush acknowledgement contains no success/failure result and is sent even when the retry budget expires with samples still pending. The runner reports Finished independently of recording success. Recorder-thread startup failure and queue drops are logged without exposing live recording health to the rider. Backlog trimming also does not increment the runner's dropped-sample counter.

The probe installed a trigger on a disposable SQLite database that rejected all telemetry inserts while allowing session metadata writes. After telemetry flowed and the ride ended, the database contained zero samples, the session had an end time and `completed=true`, and the runner reported Finished. This models write failure; it does not physically fill the disk.

**Impact:** the user can complete a ride and see “Ride saved to history” even though its measurements were never saved. Startup recovery cannot reconstruct missing samples.

**Fix direction:** expose recorder health and persisted progress during the ride; return a meaningful flush result; retain/recover unsaved data where possible; distinguish workout completion from successful data persistence. Count every loss path and show actionable recording warnings without unnecessarily stopping the ride.

Code: [recorder startup](https://github.com/bpeterman/blakebike/blob/eefe6366d1089ea9d7e965398f6e3ef7f70e9dcc/src-tauri/src/runner.rs#L1283), [flush acknowledgement](https://github.com/bpeterman/blakebike/blob/eefe6366d1089ea9d7e965398f6e3ef7f70e9dcc/src-tauri/src/runner.rs#L1351), [unconditional Finished state](https://github.com/bpeterman/blakebike/blob/eefe6366d1089ea9d7e965398f6e3ef7f70e9dcc/src-tauri/src/runner.rs#L515), [saved message](https://github.com/bpeterman/blakebike/blob/eefe6366d1089ea9d7e965398f6e3ef7f70e9dcc/src/App.tsx#L1168).

## R4 — Panic handling finalizes data but omits trainer cleanup

The ride-task supervisor catches a panic and attempts to finalize recorded samples, then publishes Error. It does not set the trainer intent to Stopped, clear the active-ride flag, or release the sleep assertion. Dropping the writer's task handle during unwinding does not establish that the writer has stopped.

A test-only panic point at timeline entry exercised the existing production error handler. After Error appeared, trainer intent remained Running, `ride_active` remained true, and no Stop command had been sent. The probe does not identify a naturally occurring timeline panic; it verifies that the promised containment is incomplete if one occurs. The sleep-assertion omission is source-reviewed, not exercised in the headless probe.

**Impact:** the ride engine stops, while trainer control can remain active and resource/state cleanup is incomplete. The displayed recovery message can also claim data was saved even when recovery failed or deleted an empty session.

**Fix direction:** use one cleanup path for normal finish, errors, panic, and cancellation; own and supervise child workers; attempt bounded trainer stop and recorder draining; report the actual recovery outcome.

Code: [normal cleanup and panic supervisor](https://github.com/bpeterman/blakebike/blob/eefe6366d1089ea9d7e965398f6e3ef7f70e9dcc/src-tauri/src/runner.rs#L488).

## R5 — Pause failure is treated as already paused

The target writer sets its internal `started` flag to false before the pause command succeeds. Paused intent has no retry timer. The ride clock pauses immediately, even if the trainer rejected or never received the pause.

The probe failed exactly one pause write, then allowed writes to succeed. Twenty simulated seconds later there had still been only one pause attempt and the control status remained degraded.

**Impact:** the screen can show a paused ride while the trainer has not paused and may retain its previous resistance.

**Fix direction:** separate desired state from acknowledged trainer state, retry failed pause with bounded attempts/backoff, and make unresolved pause status visible. Apply the same acknowledgement discipline to Stop.

Code: [pause command](https://github.com/bpeterman/blakebike/blob/eefe6366d1089ea9d7e965398f6e3ef7f70e9dcc/src-tauri/src/runner.rs#L1130), [retry timer](https://github.com/bpeterman/blakebike/blob/eefe6366d1089ea9d7e965398f6e3ef7f70e9dcc/src-tauri/src/runner.rs#L1190).

## R6 — Flush timeout excludes a blocking operation

`Recorder::flush` calls blocking `SyncSender::send` before entering its acknowledgement timeout. If the queue is full and the recorder cannot consume, that send can wait without a deadline on an async runtime thread.

The probe filled a bounded queue and released it after approximately 300 ms. A flush with a 20 ms grace returned after 301 ms. The test released the blockage deliberately; it did not hang the application indefinitely.

**Impact:** ending a ride can remain stuck in finalization, and a runtime worker is blocked. The app's outer shutdown timer is a separate safeguard, but it does not make this operation deadline-bounded or guarantee final data persistence.

**Fix direction:** avoid blocking enqueue on runtime threads and enforce one deadline over enqueue, database write/drain, and acknowledgement. Make failed/unfinished finalization visible and recoverable.

Code: [flush](https://github.com/bpeterman/blakebike/blob/eefe6366d1089ea9d7e965398f6e3ef7f70e9dcc/src-tauri/src/runner.rs#L1319).

## R7 — Wall-clock corrections stop fused telemetry

Telemetry throttling compares the current UTC timestamp against the previous emission timestamp. If UTC moves backward, every reading fails the minimum-spacing check until the old timestamp is reached again. The ride timer itself uses a monotonic clock and continues.

The probe moved the fuser's supplied time back by 60 seconds and delivered readings every 200 ms. All were suppressed until the previous timestamp plus the emission interval was reached.

**Impact:** recording and live measurements stop temporarily while the ride clock and trainer commands continue. This requires an actual system-clock correction, not merely changing the displayed timezone.

**Fix direction:** use monotonic time for throttling, freshness, and watchdogs. Preserve wall time for exported timestamps and explicitly handle discontinuities.

Code: [emission throttle](https://github.com/bpeterman/blakebike/blob/eefe6366d1089ea9d7e965398f6e3ef7f70e9dcc/src-tauri/src/devices/fuser.rs#L224).

## R8 — Recording does not respect the paused ride state

The timeline records each incoming sample before checking whether the ride is paused. The live frontend omits paused history samples, but persisted history includes them. Startup recovery infers active time from gaps between samples, so pause periods can be counted as riding time when the trainer keeps transmitting.

The probe waited for Paused, then allowed 1.6 seconds of simulated telemetry. Three samples after the pause were persisted while the ride clock was stopped.

**Impact:** live and saved ride statistics can disagree; distance, averages, and recovered duration can include pauses. This is a data-integrity problem, not a process-crash trigger.

**Fix direction:** either exclude paused samples from active recording or persist explicit ride-phase events and make every statistics/export/recovery path honor them.

Code: [sample handling](https://github.com/bpeterman/blakebike/blob/eefe6366d1089ea9d7e965398f6e3ef7f70e9dcc/src-tauri/src/runner.rs#L901), [recovery](https://github.com/bpeterman/blakebike/blob/eefe6366d1089ea9d7e965398f6e3ef7f70e9dcc/src-tauri/src/storage.rs#L815).

## Verification completed

- Frontend: type checking, lint, 49 tests, and production build passed. The build reports a bundle-size warning.
- Backend: formatting check, Clippy with warnings denied, and all 132 existing tests passed.
- Isolated audit: 11 test entries passed, including eight tests that intentionally assert the reproduced undesirable behavior, two positive integrity tests, and a child-process helper.
- Eight-hour synthetic dataset: 144,000 samples at 5 Hz persisted, recovered as an unfinished session, and exported to a 2,160,378-byte FIT file in approximately 3.8 seconds. This checks data-volume handling; it is not an eight-hour realtime soak or WebView test.
- Abrupt-exit recovery: a disposable child process exited without running destructors after committing 100 samples. All 100 were recovered. This does not test sudden power failure, uncommitted recorder buffers, or a kill during a database transaction.
- Frontend calculation benchmark: eight hours of 1 Hz history took approximately 4 ms median / 15 ms maximum for the tested calculations in Node; reloaded 5 Hz history took 42 ms / 74 ms. Charts were reduced to 1,200 points. These measurements exclude rendering and do not establish WebView stability or memory limits.
- Available application log: one `No control response in time` event for target-power opcode `0x05` appeared in the inspected reliability-event subset. No panic line matched that search. This does not prove the log event was R1 or establish that no historical crash occurred.

The initial shell selected Node 16 and failed to invoke the pinned pnpm installation. Frontend checks were rerun successfully with installed Node 22.23.2. This was a local tooling issue, not an application reliability finding.

## Remaining validation and source-review concerns

1. Real trainer: disconnect/reconnect, adapter toggling, control ownership changes, slow acknowledgements, and multi-device reconnection on macOS and Linux. No physical trainer or Bluetooth settings were changed in this investigation.
2. Realtime 4–8-hour packaged-app soak measuring memory, CPU, event-loop latency, command responsiveness, telemetry gaps, and saved sample counts. Growing UI history remains a performance concern, but the calculation benchmark did not demonstrate a crash.
3. Sleep/wake, lid closure, power changes, background operation, and failure to acquire the sleep inhibitor. Inhibitor failure currently produces a log warning while the ride proceeds.
4. Writer-task death: closing the status channel only stops status polling; it does not restart the writer or mark control lost. This is a source-reviewed supervision gap beyond the reproduced timeline-panic test.
5. Bluetooth discovery contains awaits without enclosing deadlines while connection attempts share a global connect lock. A stuck platform call could delay other device reconnections. The advertised find timeout is not an end-to-end timeout. No stuck native Bluetooth call was induced.
6. Process termination during writes/finalization, full-disk and filesystem-permission failures, delayed storage recovery after a ride ends, and true power-loss durability. The targeted insert-failure and abrupt-exit tests cover only part of this matrix.

Recommended implementation order: R1/R2 for ride continuity, R3/R4/R5 for trustworthy recording and control, then R6/R7/R8, followed by the real-hardware and packaged-app soak matrix.

## Reproducing the audit

Evidence files are [audit results](reliability-evidence/audit-results.txt) and an [investigation-only patch](reliability-evidence/audit-probes.patch). Apply the patch only to a disposable copy of the recorded commit. It adds fault-injection tests, including a test-only timeline panic point. It is not an application fix or a regression suite asserting desired behavior.

Copy `audit-probes.patch` alongside the disposable checkout directory. From that checkout:

```sh
git apply ../audit-probes.patch
cargo test --manifest-path src-tauri/Cargo.toml audit_ -- --nocapture --test-threads=1
```

The eight reproduction tests should pass while these defects remain. Correct regression tests should reverse their expectations after fixes. Use serial execution because the panic probe has a shared test-only injection flag. Build the frontend first if the disposable copy lacks the `dist` assets expected by its Tauri configuration.
