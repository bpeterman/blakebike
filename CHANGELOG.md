# Changelog

All notable changes to blake.bike are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[semantic versioning](https://semver.org/spec/v2.0.0.html).

This file is the single source of release notes: it is published to the GitHub
release when a version is tagged, and it is bundled into the app and shown on
the About card in Settings.

Conventions:

- Add entries under `## [Unreleased]` as you work. `pnpm release` turns that
  section into a dated version heading.
- The paragraph directly under a version heading is the **summary** shown on the
  About card. Keep it to one sentence, written for a rider, not a developer.
- Group the details under `### Added`, `### Changed`, `### Fixed`, or
  `### Removed`.

## [Unreleased]

Today's Intervals.icu plan on the home screen, a mirrored Intervals.icu workout library, an honest training-settings sync, and ride screens you build from fields.

### Added

- Ride screens. The live ride view is now several screens you page between with
  the tabs at the top or the ← and → keys, and each screen is a grid of fields
  you choose. A field is a metric (power, cadence, heart rate, speed, watts per
  kilogram, distance, energy, calories, time, target) measured over the whole
  ride or just the block you are riding, shown live, averaged or at its maximum
  — so average power, block average power and max power are all there without
  each being its own card. Fields can be small, wide or full width, and the
  charts, workout timeline, target & bias, time in zone and stats for nerds sit
  alongside them as panels. Build it all under Settings → Ride screens.
- Distance is now estimated live while you ride, by the same model that saves it
  with the session.
- New numbers to put on a screen: energy in kilojoules and the calories that
  burned, average and maximum power, cadence, heart rate and speed, watts per
  kilogram, distance, elapsed time and time remaining.
- Today's planned workout from your Intervals.icu calendar on the home screen: its name, structure, duration and planned load, with Start. With a trainer connected it starts at once; otherwise the Ride page opens with it selected, and it sits at the top of the ride picker marked "Today". A plan without structured steps offers a free ride instead. The next seven days are cached, so the card works offline and says how old the cache is.
- Your Intervals.icu workout library is mirrored into Workouts. Mirrored workouts are labelled with their Intervals.icu folder and planned load, can be ridden and exported like any other, and are read-only: "Edit a copy" makes a local workout from one, and they disappear again when they leave your library or you turn the mirror off.
- The Settings card names the athlete the key belongs to, has separate toggles for the calendar and the library, and says when the mirrors last synced. They refresh when blake.bike starts and when you press Sync now; the training-settings sync stays behind its own button because it rewrites your FTP and zones.
- Saving an Intervals.icu API key now checks it against Intervals.icu first. A rejected key is not saved; being offline says so instead of blaming the key.
- Trainer versus power meter. Ride with a trainer and a power meter connected
  and both power streams are recorded, each on its own clock, without changing
  which one drives ERG or the display. The ride detail then shows the accuracy
  comparison reviewers build by hand: mean offset in watts and percent, spread
  and worst case, drift minute by minute as the trainer warms up, difference by
  power level and by cadence, a Bland-Altman plot, and the lag between the two
  streams, all after lining the streams up and leaving out coasting and power
  steps. Export it as a single PNG with both devices, their firmware, the sign
  convention and the rules it followed printed on it. Neither device is treated
  as the truth. Stats for nerds shows the live trainer-versus-meter difference
  with a 30 s mean.
- The devices that rode with you are now stored with each ride, and a power
  meter's zero offset remembers the zero it replaced, so drift between zeros
  survives past the calibration dialog.

### Changed

- Importing a ZWO file now refuses workouts with step elements blake.bike does not understand instead of silently dropping them, and reads the older `SolidState` and `MaxEffort` steps.
- The old "Live ride cards" list becomes the ride-screen editor. Layouts saved
  before this release are carried over as a single screen holding the cards you
  had switched on, in the order you had them.
- The Intervals.icu sync pulls your FTP and maximum heart rate from your cycling sport settings, preferring the indoor FTP when you have set one, and no longer uses the modeled eFTP. Power zones scale from that same number, so the FTP you see and the FTP behind your zones always agree, and the status line names which one was used.
- Heart-rate zones now have their own "Import from Intervals.icu" toggle, next to the power zones toggle in each zone editor. Both are off by default and both are saved with your zone settings.
- After a sync the Settings card reports each item: the new FTP and max heart rate with their previous values, and for each zone set whether it was imported, already up to date, left alone because import is off, not configured on Intervals.icu, or unusable and why.
- Zones imported from Intervals.icu are labelled "From Intervals.icu" in the editor instead of "Custom", and can be reset to derived zones like any other set.
- "Refresh training settings" is now "Sync from Intervals.icu", on the Settings card and on the home screen's FTP card.

### Fixed

- Heart-rate zones were imported on every refresh whether or not you wanted them, replacing hand-edited zones without warning. They are now imported only when their toggle is on, and turning the toggle on over custom zones asks first.
- Pressing Refresh silently saved whatever you had half-typed in the zone editors. The sync button now waits until unsaved profile or zone edits are saved, and says so.
- A failed zone request used to be reported as "Intervals.icu did not return usable training zones" while the FTP was written anyway. The sync is now a single request: if it fails nothing changes and the error says why; if it succeeds, missing zones are reported as not configured rather than as a failure.
- Pausing a ride no longer flashes "Trainer pause not confirmed" every time. The warning now waits to see whether the trainer is actually slow to acknowledge the pause, so it only appears when the trainer really might still be holding resistance.
- A trainer frame without a power field was read as 0 W. Trainers that
  alternate frames or omit power intermittently were feeding zeros into fusion;
  such a frame now leaves power alone.

## [0.3.0] - 2026-09-19

A "Stats for nerds" card for diagnosing your connected devices, and zeroing power meters and trainer spin-downs right from the app.

### Added

- A "Stats for nerds" card for the ride screen, showing what every connected device is actually reporting: its live reading, sample rate, last packet, uptime, signal, battery, drops and bad packets, plus which device is feeding power, cadence and heart rate, and whether a metric has fallen back to another device. It also names the roles with nothing connected. Turn it on under Settings → Live ride cards, where it is off by default and can be reordered like any other card.
- Zero your power meter from its card on the Devices page. The app sends the standard Bluetooth offset-compensation command, refuses to start while the cranks are turning or the meter reads load, shows the offset it came back with and how far it drifted since last time, and remembers it per device so the card and the remembered-device list can say when it was last zeroed. A meter that asks to be zeroed (some raise a flag when they have drifted) is called out on its card.
- Before a ride, if the power meter that will feed power has not been zeroed in the last day, the start screen offers to zero it.
- The trainer's spin-down is remembered the same way, so its card says when it was last calibrated.

### Changed

- One calibration dialog serves both the trainer spin-down and the power meter zero offset, with copy, live readout and cancel behaviour per device. Cancelling a zero offset only stops waiting; cancelling a spin-down still disconnects the trainer, because the spin-down holds control of it.

## [0.2.0] - 2026-09-19

A live workout preview in the builder, a bigger built-in workout library, ride-history chart statistics, and one-click reconnection for remembered devices.

### Added

- Drag across the power or heart-rate chart in a ride's detail view to highlight a section and see its average, maximum, and minimum power and heart rate. Click the chart, press Clear, or press Escape to dismiss it.
- The library now starts with nine built-in workouts instead of one sample: an FTP test, endurance rides at 60 and 90 minutes, tempo, sweet spot, threshold, over-unders, a VO2 max session, and a recovery spin. Each one names its structure and leads its description with duration and zone. The FTP test sits at the top, because every other target is a percentage of your FTP.
- Existing libraries pick up the new workouts too, dated below your own so yours keep the top of the list. A workout of yours that shares a name with a built-in one is left alone, and the old "FTP Builder" sample is removed only if you never edited it.
- The workout builder shows a live picture of the workout as you edit it: blocks are coloured by power zone, ramps slope, free ride is hatched, and an FTP line, time axis, and repeat brackets show the shape at a glance. A stats strip gives duration, average intensity, estimated training stress, and block count. Hover a block or a row to see its counterpart, and click a block to jump to its row.
- A "Connect all" button on the Devices page brings back the most recently used remembered device for every role that has nothing connected, one after another. A sensor that does not answer is mentioned in a calm summary line and can be connected from its card once it is awake; the others connect regardless.

### Changed

- Devices moved down the sidebar to sit just above Settings, so the day-to-day pages (Overview, Workouts, Ride, History) come first.
- Workout thumbnails on the home page, in the library, and in the ride picker, along with the live-ride timeline, are drawn by the same renderer as the builder preview, so ramps, free ride, and watt targets look the same everywhere and long interval sets stay proportional.

### Fixed

- Editing a workout step that targets watts no longer converts it to a percentage of FTP.

## [0.1.0] - 2026-09-19

First release: structured workouts on an FTMS smart trainer, with Bluetooth and ANT+ heart rate, saved locally on your own computer.

### Added

- Structured ERG workouts with a built-in library, workout editor, timeline, and bulk export.
- Free ride mode with manual power control.
- FTMS smart trainer control, including guided spindown calibration.
- Heart-rate monitor support over both Bluetooth LE and ANT+, with battery status.
- Devices hub that remembers your sensors and reconnects them automatically, and keeps the ride going when a sensor drops out.
- Customizable live ride layout: choose which cards appear during a ride and in what order.
- Ride history with power and heart-rate charts, time-in-zone breakdowns, and post-ride metrics.
- Garmin-compatible FIT files written for every ride, plus CSV export and a Garmin Connect upload handoff.
- Intervals.icu integration for importing modeled eFTP, heart-rate zones, and power zones.
- Flat-road distance estimates for trainers that do not report speed.
- Confirmation dialogs and an undo toast for destructive actions, with keyboard dismissal throughout.
- Developer mode, off by default, that keeps simulated devices out of scans until you ask for them.
- Crash, quit, and sleep recovery so an interrupted ride is never lost.
- Linux packaging with udev rules and a diagnostic collector for ANT+ sticks.
