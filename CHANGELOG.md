# Changelog

All notable changes to blake.bike are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[semantic versioning](https://semver.org/spec/v2.0.0.html).

This file is the single source of release notes: it is published to the GitHub
release when a version is tagged, and it is bundled into the app and shown on
the About card in Settings.

Conventions:

- Add entries under `## [Unreleased]

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

### Changed

- The old "Live ride cards" list becomes the ride-screen editor. Layouts saved
  before this release are carried over as a single screen holding the cards you
  had switched on, in the order you had them.

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
