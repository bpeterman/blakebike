# Releasing blake.bike

One version number, one changelog, three places it shows up: the repo, the
GitHub release, and the About card in the app's settings.

## Version numbers

`package.json` is the source of truth. `pnpm release` writes the same version
into `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`, and `Cargo.lock`, so
every manifest is explicit rather than relying on Tauri's fallback order. CI runs
`pnpm release:check`, which fails the build if any of them drift or if the
changelog's newest entry is not the current version.

Semver, as it applies to this app:

| Bump  | When                                                                    |
| ----- | ----------------------------------------------------------------------- |
| patch | Bug fixes and small corrections a rider would not need explained.        |
| minor | New features, new hardware support, new UI.                              |
| major | A break: a data migration that cannot roll back, or a removed feature.   |

## While you work

Add entries to `## [Unreleased]` in [CHANGELOG.md](../CHANGELOG.md) as you go,
in the same commit as the change:

```markdown
## [Unreleased]

Rides now survive a trainer that drops mid-interval.

### Fixed

- Reconnect the trainer without ending the ride.
```

Two rules the tooling enforces:

- The paragraph under the heading is the **summary**. It is the one line shown
  on the About card, so write it for a rider, not a developer.
- Details go under `### Added`, `### Changed`, `### Fixed`, or `### Removed`.

## Cutting a release

```bash
pnpm release minor   # or patch, major, or an explicit 1.2.0
```

That rewrites `## [Unreleased]` into a dated version heading and bumps the
version in `package.json`, `tauri.conf.json`, `Cargo.toml`, and `Cargo.lock`.
It commits nothing —
review the diff first, then:

```bash
git add CHANGELOG.md package.json src-tauri/tauri.conf.json \
  src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "Release v0.2.0"
git tag v0.2.0
git push --follow-tags
```

Pushing the tag triggers [.github/workflows/release.yml](../.github/workflows/release.yml),
which reruns the checks and tests, builds a universal macOS dmg, and creates the
GitHub release with the notes for that version pulled straight out of the
changelog. Nothing is published unless the tag matches the committed version.

## What people download

The dmg is **not signed or notarized** — there is no Apple Developer account
behind it yet. On first launch macOS will refuse to open it, and the release
notes tell people the workaround: right-click the app in Applications and choose
**Open**. Gatekeeper only asks once.

To make it just work later: buy the $99/year Apple Developer membership, create
a Developer ID Application certificate, add `APPLE_CERTIFICATE`,
`APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`,
`APPLE_PASSWORD`, and `APPLE_TEAM_ID` as repository secrets, and uncomment the
`env:` block in the release workflow. Nothing else changes.

## Apple Silicon and Intel

The release builds `universal-apple-darwin`: one dmg with both slices, native on
either machine. This is not optional for Intel support — Rosetta translates x86
to ARM, not the reverse, so an ARM-only build will not launch on an Intel Mac at
all.

Every dev machine and CI runner here is Apple Silicon, so the Intel half is only
ever cross-compiled. `ci.yml` builds `x86_64-apple-darwin` on every push to
catch a cross-compile break before a tag turns it into a failed release.

## Adding Linux artifacts

`tauri.conf.json` already targets appimage and deb, so local Linux builds work
today. To publish them alongside the dmg, add an `ubuntu-24.04` job to the
release workflow that installs the same system dependencies as `ci.yml` and runs
`tauri-action` with `args: --bundles appimage,deb` and the same `tagName` — both
jobs attach their artifacts to the one release.

## Where the notes show up in the app

[src/releaseNotes.ts](../src/releaseNotes.ts) imports `CHANGELOG.md` as a raw
string at build time and parses it. The About card at the bottom of Settings
shows the running version and the summary for it; the **Release notes** button
opens the full history. There is no generated file to keep in sync — edit the
changelog and rebuild.
