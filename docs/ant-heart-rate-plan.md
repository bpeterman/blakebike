# ANT+ Heart-Rate Support — Plan

## Goal

Allow BlakeBike to discover, connect to, remember, and read an ANT+ heart-rate
monitor through a USB ANT stick, with Linux as the first supported platform.
ANT heart rate must feed the existing heart-rate device slot and telemetry fuser,
so rides, recording, source selection, stale-data fallback, and FIT export keep
working without transport-specific code.

The first release is intentionally heart-rate only. It establishes the ANT USB
transport and lifecycle cleanly enough that ANT+ power and cadence can be added
later, but does not implement those profiles yet.

## Current state

- Bluetooth Heart Rate Service support is complete: discovery, connection,
  parsing, per-device diagnostics, persistence, automatic reconnect, source
  fusion, ride display, and recording.
- `DeviceInfo` and known-device persistence currently assume every physical
  device is Bluetooth.
- `sensor::Sensor` combines BLE/GATT transport with profile decoding. The ANT
  path cannot use GATT, but it can reuse `DeviceSlot`, `Reading::HeartRate`, and
  `TelemetryFuser`.
- Linux packaging currently has Bluetooth guidance but no USB permission rule or
  ANT diagnostics.
- The stick attached while this plan was written is visible in the macOS USB
  registry as **ANT USBStick2**, Dynastream VID `0x0fcf`, PID `0x1008`. This
  proves USB enumeration on the current machine, not ANT communication or Linux
  access; the Linux inventory below remains the implementation gate.

## Decisions

1. **Native Rust backend.** Keep USB and ANT processing in the Tauri backend.
   Do not add a Node native module or a long-running sidecar to the packaged app.
2. **Small, HR-first protocol surface.** Implement only the ANT framing,
   initialization, channel commands, receive events, and HR profile fields needed
   for reliable heart-rate reception. Keep message parsing pure and well tested.
3. **Transport is explicit data.** Add `DeviceTransport { Ble, Ant }` to
   `DeviceInfo` and `KnownDevice` (`Ble` as the serde default for old records).
   Give new ANT identities a namespaced stable key such as
   `ant:hrm:<device-number>:<transmission-type>` so they cannot collide with BLE
   peripheral IDs.
4. **One heart-rate role, not separate BLE and ANT roles.** The existing Heart
   rate card can hold either transport. Auto source priority remains based on
   role, so no fuser or ride-recorder policy changes are required.
5. **ANT is optional.** No stick attached is not a scan failure. A detected stick
   that is busy, unsupported, unplugged, or inaccessible is a transport-specific
   diagnostic and must not hide working BLE results.
6. **Linux first, portable boundary.** Build the USB layer behind an `AntIo`
   trait and avoid Linux-only types above it. Validate Linux before claiming
   macOS or Windows support.

## Phase 0 — Identify the actual stick and prove the wire path

Do this on the affected Linux computer before selecting the final USB crate or
driver path.

Capture:

```text
lsusb -nn
lsusb -t
udevadm info --attribute-walk --name=<device node, if one exists>
journalctl -k --since "5 minutes ago"
ls -l /dev/ttyUSB* /dev/ttyACM*  (if present)
```

Record vendor/product ID, USB interfaces/endpoints, kernel driver, device node,
and permissions. Older sticks may bind through `cp210x` as a serial device;
newer sticks are commonly accessed through libusb. The implementation must be
chosen from observed hardware, not the product name printed on the shell.

Build a throwaway backend diagnostic that:

1. enumerates supported sticks and prints descriptors/endpoints;
2. opens the selected stick (or reports `not found`, `permission denied`, or
   `busy` distinctly);
3. resets ANT and receives the startup event;
4. requests capabilities/version;
5. configures a wildcard receive channel for the ANT+ HRM profile; and
6. logs device number plus raw eight-byte broadcast pages from a worn strap.

Exit criterion: ten minutes of broadcasts from the user's strap on Linux with no
hang, and clean behavior when the stick is unplugged and reinserted. Preserve a
short, anonymized frame fixture for automated tests.

If direct USB and serial paths are both needed for supported sticks, provide two
`AntIo` implementations. If only one is present, ship that path first and list
the exact supported VID/PID values in the UI and documentation.

## Phase 1 — ANT protocol core

Add `src-tauri/src/devices/ant/`:

- `io.rs`: device enumeration/open/read/write/close behind `AntIo`; bounded read
  timeouts; cancellation-safe shutdown; typed open errors.
- `frame.rs`: ANT sync/length/message/checksum framing with incremental parsing,
  recovery after garbage or a bad checksum, and outbound encoding.
- `messages.rs`: only the commands and events needed for reset, network setup,
  capabilities, channel assignment/configuration/open/close, channel ID, and
  broadcast/extended data.
- `manager.rs`: one owner task for the USB handle. Commands arrive over a Tokio
  channel; parsed events are broadcast internally. No UI command may read from
  the stick directly.
- `hrm.rs`: HR channel configuration and profile decoding.

Use the official ANT+ network/channel parameters and HRM profile document as the
normative source. For HR, accept device type 120 and decode the common bytes on
every page: page/toggle byte, heartbeat event time, heartbeat count, and computed
heart rate. A computed value of zero is missing data, not a valid sample. Decode
manufacturer, serial, capabilities, and battery pages later without blocking
the initial BPM path.

The manager state machine must use timeouts and validate responses at every
step. It must turn stick removal, channel closure, malformed frames, and write
failure into events instead of panics or permanently blocked tasks.

### Dependency gate

Before committing to `rusb`, a serial crate, or a small imported implementation,
record:

- supported stick generations and platforms;
- maintenance activity and license compatibility;
- whether system `libusb`/`libudev` development packages are needed to build;
- whether the dependency can be bundled into AppImage and `.deb`; and
- unplug/cancellation behavior under Tokio.

Do not use the old `ant-plus` Rust crate: its only release is yanked and it does
not provide a maintained production base. Existing open-source ANT libraries are
useful behavioral references, not an automatic runtime dependency.

## Phase 2 — Integrate with the device hub

Introduce a small heart-rate coordinator rather than teaching the BLE sensor
worker about ANT:

```text
HeartRateDevice
  shared DeviceSlot
  BLE Sensor
  ANT HeartRateReceiver
  connect(device) -> dispatch by device.transport
  disconnect()     -> stop the active transport
```

Both receivers write the same slot statistics/log and ingest
`Reading::HeartRate { bpm, sensor_contact }` into the existing fuser. ANT reports
`sensor_contact: None` unless the profile supplies equivalent information.

Change hub behavior as follows:

- scan BLE and ANT independently and merge/deduplicate results;
- keep partial results if one transport fails;
- expose adapter diagnostics per transport instead of one global scan error;
- dispatch connect/find/reconnect by `DeviceTransport`;
- never run BLE lookup for a remembered ANT sensor;
- on link loss, forget the heart-rate reading immediately so the existing
  five-second stale/fallback behavior remains authoritative;
- preserve the current rule that one physical device occupies only one role;
- serialize access to the single ANT stick inside `AntManager`, independently
  of the existing BlueZ connect lock.

For discovery, prefer ANT extended receive messages so each broadcast carries a
channel ID. If the user's stick cannot do background/extended scanning, the
fallback is a bounded wildcard HR search that identifies one sensor at a time;
the UI must describe that limitation rather than inventing a multi-device list.

Persistence migration is additive: old JSON without `transport` deserializes as
BLE. ANT reconnect uses the stored ANT device number/transmission type, first
tries a specific channel, and falls back to a bounded search only when needed.

## Phase 3 — UI and diagnostics

- Rename BLE-only copy (`BLUETOOTH SENSORS`, Bluetooth-only heart-rate hints) to
  transport-neutral wording.
- Add a BLE or ANT badge to discovery rows, known devices, and the connected HR
  card.
- Show an ANT adapter status near scan results: not attached, ready, busy,
  permission denied, unsupported stick, or disconnected.
- Keep "no ANT stick" quiet for users who only use Bluetooth.
- For Linux permission failures, show the detected VID/PID and the exact next
  action. For a busy stick, tell the user another fitness/Garmin process may own
  it. Do not collapse either case into "device not found."
- Log stick open, reset, capabilities, channel configuration, detected sensor
  ID, first HR sample, sample-rate summaries, parse failures, channel loss, and
  stick removal. Do not log the ANT+ network key.

The connected-card signal field should be `N/A` until an ANT message supplies a
real received-signal value; do not map USB presence to radio RSSI. Battery is
also optional for the first release.

## Phase 4 — Linux permissions and packaging

Support only the verified VID/PID set. Provide a least-privilege udev rule using
`TAG+="uaccess"` and mode `0660` for those products, rather than making arbitrary
USB devices world-writable. Cover both the raw USB interface and a serial device
node if Phase 0 shows both are required.

- Bundle/install the rule in the Debian package if the Tauri packaging path can
  do so reliably.
- For AppImage and development builds, include a checked-in install script or
  documented copy command plus `udevadm control --reload-rules` and a replug
  step.
- Add a startup diagnostic that distinguishes a missing rule from a missing
  stick.
- Document required build packages only if the selected dependency actually
  needs them; prefer vendored/static libusb where licensing and packaging allow.

## Test strategy

### Automated

- Frame encoder/parser: split reads, multiple frames per read, checksum failure,
  garbage resynchronization, maximum length, and unknown message IDs.
- HR pages: normal values, page-toggle bit, every observed background page,
  zero/invalid BPM, heartbeat/event rollovers, and truncated payloads.
- Manager state machine with fake `AntIo`: successful startup, timeout at each
  step, wrong response, search timeout, unplug during read/write, reconnect, and
  shutdown while blocked.
- Hub tests: merged BLE/ANT scan, partial transport failure, connect dispatch,
  ANT sample entering the fuser, fallback after loss, remembered ANT reconnect,
  and backward-compatible BLE persistence.
- React tests: transport badges, ANT-specific errors, no-stick quiet state, and
  removal of Bluetooth-only copy.
- Linux CI builds the ANT feature and runs all fake-I/O tests; hardware tests are
  not required in CI.

### Hardware acceptance on Linux

1. Fresh install detects the supported USB stick and clearly diagnoses missing
   permissions.
2. A worn HR strap appears with a stable ANT device ID and connects without BLE.
3. First plausible BPM arrives within five seconds and subsequent data is near
   the expected ANT HR rate (about 4 Hz).
4. BPM agrees with a known-good bike computer/vendor display within 1–2 bpm.
5. An FTMS trainer can remain connected over BLE while HR arrives over ANT for a
   60-minute ride; recording and FIT export contain HR.
6. Pulling the strap out of range clears/falls back cleanly; returning it invokes
   bounded automatic reconnect without duplicating samples.
7. Pulling and reinserting the USB stick never hangs or crashes the app and
   produces actionable state/log transitions.
8. Quit/relaunch and Known devices reconnect select ANT rather than attempting a
   Bluetooth lookup.
9. A machine with no ANT stick has unchanged BLE behavior and no alarming error.
10. BlueZ restart does not disturb ANT HR, and ANT stick removal does not disturb
    BLE trainer control.

Repeat the core cases with every supported stick VID/PID. macOS and Windows stay
unclaimed until their USB ownership/driver behavior passes the same matrix.

## Delivery slices

1. **Hardware/protocol spike:** Linux inventory, raw frames, dependency decision,
   supported-stick table, and captured fixtures.
2. **Protocol core:** `AntIo`, framing, manager, wildcard/specific HR channels,
   decoder, fake-I/O tests, and a backend diagnostic command.
3. **Hub integration:** transport-aware models/persistence, merged scan,
   heart-rate coordinator, fuser/reconnect wiring, and Rust tests.
4. **Product UI:** transport badges, adapter status/errors, neutral copy, logs,
   and React tests.
5. **Distribution:** udev/package work, Linux documentation, CI build, and the
   hardware acceptance run.

## Definition of done

ANT HR is done when a supported stick and strap pass the Linux acceptance matrix,
BLE-only behavior remains green, permission/busy/unplug failures are actionable,
the package contains a workable permissions path, and the supported hardware and
current platform limits are documented. Merely parsing a captured HR page or
working as root does not satisfy completion.

## Follow-ups, not part of the first release

- ANT+ cycling power, cadence, fitness equipment, and FE-C control.
- Multiple simultaneous ANT sensors/profiles and channel allocation UI.
- Optional HR beat-to-beat/RR-derived metrics.
- Battery/manufacturer pages beyond what is needed for useful diagnostics.
- macOS and Windows support after platform-specific hardware validation.

## Reference material

- [Official ANT+ device profile index](https://thisisant.developer.garmin.com/pages/developer/ant-plus/device-profiles/index.html)
- [ANT USB2 Stick datasheet](https://thisisant.developer.garmin.com/assets/assets/resources/Deprecated_D00001367_ANT_USB2_Stick_Datasheet_Rev1.4.pdf)
- [Debian antpm notes on Linux `cp210x`, device nodes, and udev](https://manpages.debian.org/unstable/antpm/antpm-downloader.1.en.html)
- [Yanked `ant-plus` Rust crate](https://docs.rs/crate/ant-plus/0.0.1)
- [Incyclist ANT+ library as a cross-platform behavioral reference](https://github.com/incyclist/ant-plus)
