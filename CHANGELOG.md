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
