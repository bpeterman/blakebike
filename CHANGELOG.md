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
