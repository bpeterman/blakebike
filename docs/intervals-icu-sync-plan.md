# Intervals.icu Sync — Plan

Goal: make blake.bike a first-class Intervals.icu client in two PRs. Phase 1 turns the
existing settings sync into something a rider can predict: one FTP, two symmetric
opt-in zone imports, honest copy, per-item outcomes, and no silent loss of hand-edited
zones. Phase 2 adds the training calendar (today's planned workout on the home screen,
startable) and the workout library (mirrored into the local library, distinguishable
from local workouts), with an offline-tolerant cache and a visible sync cadence.

## Where the integration stands today

`src-tauri/src/intervals.rs` is a flat pair of async functions that each build their
own `reqwest::Client` (src-tauri/src/intervals.rs:53, src-tauri/src/intervals.rs:91),
hardcode `athlete/0` (src-tauri/src/intervals.rs:58, src-tauri/src/intervals.rs:97) and
authenticate with `basic_auth("API_KEY", key)`. `commands::refresh_estimated_ftp`
(src-tauri/src/commands.rs:299) calls both and writes profile + zones. The UI is the
Intervals.icu card in `SettingsPage` (src/App.tsx:1805) plus a refresh icon on the
Overview FTP card (src/App.tsx:867).

## Verified against the API (2026-09-19)

Pulled from the OpenAPI document at `https://intervals.icu/api/v1/docs` and the forum
guide "Downloading planned workouts from the API". Not re-derived from memory.

| Need | Endpoint | Shape that matters |
| --- | --- | --- |
| Athlete id | `GET /api/v1/athlete/0` | `WithSportSettings.id` (string, e.g. `i12345`), `name`, plus `sportSettings[]` |
| Cycling settings | `GET /api/v1/athlete/{id}/sport-settings` | `SportSettings[]`: `types[]`, `ftp`, **`indoor_ftp`**, `max_hr`, `hr_zones[]`, `hr_zone_names[]`, `power_zones[]`, `power_zone_names[]` |
| Modeled FTP | `GET /api/v1/athlete/{id}/mmp-model?type=Ride` | `PowerModel.ftp` (dropped, see below) |
| Calendar | `GET /api/v1/athlete/{id}/events?oldest=&newest=&category=WORKOUT&ext=zwo` | `Event[]`: `id`, `start_date_local`, `name`, `description`, `type`, `category`, `icu_training_load`, `moving_time`, `workout_doc`, `updated`, and with `ext=zwo` **`workout_filename` + `workout_file_base64`** on each event. `oldest`/`newest` are local ISO dates, inclusive; defaults are today and today + 6. |
| One event as ZWO | `GET /api/v1/athlete/{id}/events/{eventId}/download.zwo` | raw ZWO text (not needed when `ext=zwo` is used on the list) |
| Library | `GET /api/v1/athlete/{id}/workouts` | `Workout[]`: `id` (int), `name`, `description`, `type`, `folder_id`, `updated`, `icu_training_load`, `moving_time`, `workout_doc`, `target` |
| Folders | `GET /api/v1/athlete/{id}/folders` | `Folder[]`: `id`, `name`, `type` (`FOLDER` \| `PLAN`), `children: Workout[]` |
| Library workout as ZWO | `POST /api/v1/athlete/{id}/download-workout.zwo` with the `Workout` JSON as body | ZWO text; "the athlete's settings are used to resolve power targets" |
| Auth | HTTP basic, user `API_KEY`, password = key | unchanged |

`workout_doc` is an untyped object in the schema (steps with `duration`, `power {units,
value|start,end}`, `reps`, nested `steps`, `ramp`, `freeride`, `text`, `cadence`). It is
richer than ZWO but undocumented; the ZWO route lets Intervals.icu do the `%MMP` and
`%FTP` resolution with its own model and hands us a format `formats::import_zwo`
already reads.

## Phase 1 — heart-rate zones as a first-class, explicit sync item

### Decisions

- **One request, one FTP: the cycling sport settings are authoritative, and the
  `mmp-model` round trip goes away.** Today `profile.ftp_watts` is set from the
  modeled eFTP (src-tauri/src/commands.rs:307) while power zone boundaries scale off
  `imported.cycling_ftp_watts` when present (src-tauri/src/commands.rs:334), so the
  displayed FTP and the FTP behind the zones can disagree. The sport-settings FTP is the
  number Intervals.icu itself uses for zones, workout targets and every ZWO it exports
  (which matters for Phase 2), so it wins. Because blake.bike is an indoor app, the
  `indoor_ftp` field is used when the rider has set one and `ftp` otherwise; the result
  says which (`ftpSource: "indoorFtp" | "ftp"`) and the status line reads it back, e.g.
  "FTP 265 W from your Intervals.icu indoor FTP (was 250 W)". Power zone boundaries
  scale from that chosen FTP, never from anything else. Ride settings with neither
  value in the 50–500 W range are an error ("Intervals.icu cycling settings have no
  FTP") raised before anything is written. `fetch_estimated_ftp`,
  `MmpModel`, `normalized_ftp` and their tests (src-tauri/src/intervals.rs:8,
  src-tauri/src/intervals.rs:90, src-tauri/src/intervals.rs:124,
  src-tauri/src/intervals.rs:262) are removed, not kept as a fallback.
- **Two symmetric opt-in toggles, both default off.** `sync_power_zones_from_intervals`
  (src-tauri/src/storage.rs:63, default `false` at src-tauri/src/storage.rs:74) gains a
  twin `sync_heart_rate_zones_from_intervals`, also `false`. The HR import at
  src-tauri/src/commands.rs:319 currently has no gate at all. `TrainingZoneSettings`
  has a container-level `#[serde(default)]` over its hand-written `Default`
  (src-tauri/src/storage.rs:60, src-tauri/src/storage.rs:70), so saved settings without
  the new field load with it off; `version` stays `1` on both sides (src/types.ts:299).
  The toggles stay part of the zone draft and are persisted by "Save zone settings", on
  purpose: `useEffect(() => setZoneDraft(trainingZones))` (src/App.tsx:1627) resets the
  draft whenever saved settings change, so saving a toggle immediately would wipe
  unsaved boundary edits sitting next to it.
- **Zone provenance becomes explicit: `ZoneMode` gains `intervals`.** The sync sets
  `heart_rate_mode = ZoneMode::Custom` (src-tauri/src/commands.rs:327) and the same for
  power (src-tauri/src/commands.rs:337), which is exactly why hand-edited and imported
  zones are indistinguishable today. With a third mode (src-tauri/src/storage.rs:53,
  src/types.ts:296) the editor head can say "From Intervals.icu" instead of "Custom",
  the app can tell a hand-tuned set from an imported one, and Phase 2 can reason about
  the same field. `validate_zones` (src-tauri/src/storage.rs:101) validates `intervals`
  like `custom`; `effectivePowerZones` / `effectiveHeartRateZones` (src/types.ts:408,
  src/types.ts:416) switch from `=== "custom"` to `!== "derived"`. Editing a boundary
  still switches the set to `custom` (src/App.tsx:1791, src/App.tsx:1799). `ZoneEditor`'s
  `mode` prop is typed `"derived" | "custom"` and shows "Reset defaults" only for
  `custom` (src/App.tsx:2154, src/App.tsx:2166); it becomes `ZoneMode` and resets for
  anything that is not `derived`, so an imported set can go back to derived without
  first hand-editing a boundary.
- **A sync replaces a zone set only when its toggle is on, and turning a toggle on
  over custom zones asks first.** The toggles move out of a single checkbox above both
  editors (src/App.tsx:1770) into the head of each `ZoneEditor` (src/App.tsx:2144), so
  each set says where it comes from and whether it is imported. Ticking a toggle while
  that set's mode is `custom` opens the existing `ConfirmDialog` (src/ConfirmDialog.tsx)
  — "Replace your custom power zones? The next sync from Intervals.icu overwrites the
  boundaries you set by hand." Cancel leaves the toggle off. If the rider later edits an
  imported set while its toggle is on, the editor shows an inline note that the next
  sync replaces those edits; not silent, and they can untick the toggle right there.
  The backend rule stays simple and testable: toggle on → import; toggle off → leave
  the set alone and report `syncOff`.
- **FTP and max HR are always synced; zones are the opt-in part.** Max HR is currently
  overwritten with no mention anywhere (src-tauri/src/commands.rs:318). FTP is the same
  kind of value — a single editable anchor on the rider profile form — and pulling
  those two numbers is the card's stated purpose, so both are imported on every sync
  and both appear in the result with their previous value. What needs the guard is the
  many hand-tuned boundaries, and those are the toggles. If Intervals.icu has no
  `max_hr`, the local value is kept and the result says so. HR boundaries without a
  usable max HR are still rejected by the normalizer (src-tauri/src/intervals.rs:172),
  and the rule that drops a top boundary at or above max HR (src-tauri/src/commands.rs:321)
  applies against the max HR that was just imported.
- **Request failure and "nothing configured" are different outcomes, and a failed
  request writes nothing.** Today a zone-fetch `Err` is a `tracing::warn`
  (src-tauri/src/commands.rs:347) while the FTP write still goes through, and the UI
  says "did not return usable training zones" for both cases (src/App.tsx:1673). With a
  single request there is no partial state: the request fails → the command errors and
  neither profile nor zones are touched. When the request succeeds, each zone set
  reports one of `imported`, `unchanged` (toggle on, stored mode already `intervals`
  and boundaries and names identical; a `custom` set with the same numbers is still
  `imported`, because its mode flips), `syncOff` (toggle off), `notConfigured`
  (Intervals.icu has no boundaries for that set) or `invalid { reason }` (Intervals.icu
  has boundaries but they do not increase, fall outside range, or HR zones come without
  a max HR). Today `normalized_cycling_zones` returns one `Err` for the whole response
  in those cases (src-tauri/src/intervals.rs:140, src-tauri/src/intervals.rs:172), which
  with a single request would fail the FTP import over a bad HR zone the rider never
  asked for; the client therefore normalizes each set on its own and returns
  `Result<ZoneBoundaries, String>` per set. Only FTP-level problems are fatal: no Ride
  sport settings at all ("Intervals.icu has no cycling settings for this athlete") or
  no usable FTP.
- **Rename to what it does.** `refresh_estimated_ftp` (src-tauri/src/commands.rs:300,
  src-tauri/src/lib.rs:196), `api.refreshEstimatedFtp` (src/api.ts:81), and the
  `refreshEstimatedFtp` handler in `SettingsPage` (src/App.tsx:1661) become
  `sync_training_settings` / `api.syncTrainingSettings` / `syncTrainingSettings`. The
  button reads "Sync from Intervals.icu"; the Overview icon (src/App.tsx:867) keeps its
  place and gets the same label.
- **Sync never commits a draft.** `refreshEstimatedFtp` calls
  `api.saveTrainingZones(zoneDraft)` first (src/App.tsx:1664), so pressing Refresh
  persists whatever is half-typed in the editors. That line goes. Instead the Sync
  button is disabled while either the profile draft or the zone draft differs from the
  saved value, with a hint naming the unsaved card ("Save your zone settings first",
  "Save your rider profile first", or both) — because a sync result replaces both
  drafts (src/App.tsx:1626, src/App.tsx:1627) and unsaved edits would be lost either
  way. The Overview button has no drafts and is never blocked.
- **The client type arrives now, small.** `intervals.rs` is being rewritten in this
  phase anyway, so it becomes `IntervalsClient { api_key, base_url, http }` with one
  method, `cycling_settings()`. Phase 2 adds methods to the same type rather than
  refactoring twice. `base_url` stays a constructor parameter (`with_base_url`) because
  the tests depend on that seam (src-tauri/src/intervals.rs:209).
- **Copy tells the truth.** The card paragraph (src/App.tsx:1809) currently promises
  "modeled eFTP, cycling heart-rate zones, and power zones when enabled above", and the
  one checkbox says "Import power zones". New copy: "Sync pulls your FTP and maximum
  heart rate from your Intervals.icu cycling settings. Power and heart-rate zones are
  imported only when you turn them on in Training zones above." Title: "Intervals.icu".

### Changes

#### `src-tauri/src/intervals.rs`

- `pub struct IntervalsClient` with `new(api_key: &str)` (production base URL, 15 s
  timeout, one `reqwest::Client`) and `with_base_url(api_key, base_url)` for tests.
- `pub async fn cycling_settings(&self, athlete_id: &str) -> Result<Option<CyclingSettings>, IntervalsError>`
  hitting `/api/v1/athlete/{athlete_id}/sport-settings`; Phase 1 passes `"0"`, Phase 2
  passes the stored id, so the signature does not move again.
  `CyclingSettings { ftp: Option<CyclingFtp { watts, source }>, max_heart_rate_bpm:
  Option<u16>, heart_rate_zones: Result<ZoneBoundaries, String>, power_zones:
  Result<ZoneBoundaries, String> }` where `ZoneBoundaries { boundaries, names }` and an
  empty `Ok` means not configured. `normalized_boundaries` stays as it is; the
  `normalized_cycling_zones` wrapper is rewritten to fill the per-set `Result`s instead
  of short-circuiting.
- `IntervalsError` enum: `Unauthorized`, `Network(String)`, `Http { status, what }`,
  `Unreadable { what, error }`, with `Display` producing today's messages and
  `From<IntervalsError> for String` for the command layer, so "rejected the API key" and
  "could not reach" stay distinguishable.
- `mmp-model` code removed.

#### `src-tauri/src/storage.rs`

- `ZoneMode::Intervals` (`"intervals"`).
- `TrainingZoneSettings.sync_heart_rate_zones_from_intervals: bool`, default `false`.
- `validate_zones` treats `Intervals` like `Custom`.

#### `src-tauri/src/commands.rs`

- `refresh_estimated_ftp` → `sync_training_settings`, structured as: read key → one
  `cycling_settings()` call → build `TrainingSyncOutcome` via a pure function
  `apply_cycling_settings(profile, zones, settings) -> (Profile, TrainingZoneSettings, TrainingSyncResult)`
  → save both. The pure function is what the tests exercise.
- `TrainingSyncResult` becomes:

  ```text
  profile, zones,
  ftp: { watts, previousWatts, source: "indoorFtp" | "ftp" },
  maxHeartRate: { bpm, previousBpm } | null,      // null = Intervals.icu has none
  powerZones: { status: "imported" | "unchanged" | "syncOff" | "notConfigured" }
            | { status: "invalid", reason },
  heartRateZones: same
  ```

  The TypeScript type lives in `src/types.ts` next to `describeTrainingSync` (today it
  sits in src/api.ts:30), so the helper and its input have one home.

- `zone_definitions` and `power_zone_boundaries` (src-tauri/src/commands.rs:368,
  src-tauri/src/commands.rs:381) are unchanged and reused; the HR "drop the boundary at
  or above max HR" rule (src-tauri/src/commands.rs:321) moves into the pure function
  and gets a test.

#### Frontend

- `src/types.ts`: `ZoneMode` adds `"intervals"`; `TrainingZoneSettings` and
  `defaultTrainingZoneSettings` add `syncHeartRateZonesFromIntervals: false`; the two
  `effective*Zones` helpers use `!== "derived"`.
- `src/api.ts`: `TrainingSyncResult` mirrors the Rust shape; `syncTrainingSettings()`.
- `src/App.tsx`:
  - `ZoneEditor` gains `syncFromIntervals`, `onSyncFromIntervals` props and renders the
    toggle in its head; `mode` is typed `ZoneMode` and the label maps `derived` →
    "Derived", `custom` → "Custom", `intervals` → "From Intervals.icu"; "Reset
    defaults" shows for any non-derived mode. When `mode === "custom"` and the toggle
    is on it shows the inline replace-on-sync note.
  - `SettingsPage` owns a `zoneSyncConfirm: "power" | "heartRate" | null` state that
    renders `ConfirmDialog` when a toggle is ticked over custom zones.
  - `SettingsPage` computes `dirty = draft !== profile || zoneDraft !== trainingZones`
    (structural compare via `JSON.stringify`, as the values are small plain objects) and
    disables Sync while dirty.
  - The status line is built by a pure `describeTrainingSync(result)` in `src/types.ts`
    (next to the zone helpers) so the copy is unit-tested rather than snapshot-tested.
  - Overview `onRefreshFtp` (src/App.tsx:557) → `onSyncTrainingSettings`.
- `CHANGELOG.md` gets an Unreleased entry (Changed + Fixed).

### Tests

- `src-tauri/src/intervals.rs`: `serve_once` stays; tests cover the client fetching
  sport settings with basic auth against the seam, `indoor_ftp` preferred over `ftp`,
  401 → `Unauthorized`, 500 → `Http`, and no Ride entry → `Ok(None)`.
- `src-tauri/src/commands.rs` (`training_sync_tests`): the pure apply function with
  both toggles off (FTP + max HR change, both sets `syncOff`, zones untouched), both on
  (mode `Intervals`, boundaries scaled from the chosen FTP, `imported`), toggle on with
  identical stored `intervals` zones (`unchanged`) versus identical `custom` zones
  (`imported`), toggle on with empty boundaries (`notConfigured`), toggle on with a bad
  set (`invalid`, other set still imported, FTP still written), no usable FTP (error,
  nothing written), missing `max_hr` (`maxHeartRate: null`, local kept), and the
  drop-top-HR-boundary rule.
- `src-tauri/src/storage.rs`: settings saved without the new field load with it off;
  `intervals` mode round-trips and validates.
- `src-tauri/src/intervals.rs` additionally: a response whose HR zones are bad still
  yields `Ok` with `power_zones: Ok(..)` and `heart_rate_zones: Err(..)`.
- `src/types.test.ts`: `effective*Zones` honour `intervals` mode; `describeTrainingSync`
  wording for each outcome combination.
- `src/SettingsPage.test.tsx` (replacing the three Intervals.icu cases at
  src/SettingsPage.test.tsx:51, src/SettingsPage.test.tsx:98, src/SettingsPage.test.tsx:134):
  each editor has its own toggle and both save through "Save zone settings"; ticking a
  toggle over custom zones opens the confirm and cancel leaves it off; an `intervals`
  set shows "From Intervals.icu" and can be reset to derived; Sync is disabled with the
  hint while a draft is dirty and never calls `saveTrainingZones`; a sync result
  publishes profile and zones and renders the status line; the key save/clear case is
  kept as is.

### Verification

1. `pnpm check` and `pnpm test` (Node 22 via nvm).
2. `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`.
3. Scratchpad Vite harness rendering `SettingsPage` with a fake `api` to eyeball the
   editor heads, the confirm dialog and the disabled-while-dirty state.
4. Blake syncs against his real account and confirms the FTP source line.

## Phase 2 — training calendar and workout library

### Decisions

- **Resolve and store the real athlete id when the key is saved, and lazily for keys
  saved before this change.** Every call today uses `athlete/0`
  (src-tauri/src/intervals.rs:58). `save_intervals_api_key` (src-tauri/src/commands.rs:290)
  becomes async: it calls `GET /api/v1/athlete/0`, and only if that succeeds stores the
  key and an `IntervalsAthlete { id, name }` under a new settings key. A bad key is
  rejected at save time with "Intervals.icu rejected the API key"; a network failure is
  reported as "Could not reach Intervals.icu; the key was not saved" so an offline rider
  does not conclude the key is wrong. Existing installs already have a key
  (src-tauri/src/storage.rs:23) and no athlete, so every command that needs the id goes
  through one `resolve_athlete(state)` helper: stored athlete → use it; key but no
  athlete → fetch `athlete/0`, store, continue; no key → "Save an Intervals.icu API key
  first". `configured` derives from the key; `athleteName` is `null` until resolved.
  `clear_intervals_api_key` (src-tauri/src/commands.rs:295) also clears the athlete, the
  sync state, the calendar cache and the mirrored library workouts (they cannot be
  refreshed without the key; the card says so). `cycling_settings()` takes the resolved
  id too.
- **ZWO is the transfer format on both paths, via `import_zwo`.** The calendar list
  with `ext=zwo` carries `workout_file_base64` per event, so today's plan costs one
  request and no per-event follow-up. Library workouts have no `ext` on the list, so a
  changed workout costs one `POST download-workout.zwo`; the sync only re-fetches
  workouts whose `updated` differs from the cached copy, so a steady library costs one
  `GET`. `formats::import_zwo` (src-tauri/src/formats.rs:9) is reused as is for
  structure. The `base64` crate is added for decoding.
- **Two latent `import_zwo` gaps get fixed because the imports now come from a source
  we do not control.** Unknown elements are silently dropped (src-tauri/src/formats.rs:28
  and src-tauri/src/formats.rs:40), which would turn a `MaxEffort` block into a missing
  step. `MaxEffort` maps to `FreeRide` and `SolidState` to `Steady`; any other unknown
  direct child of `<workout>` fails the import naming the element (children of steps
  such as `textevent` stay ignored). ZWO can only express two-step repeats as
  `IntervalsT`; longer repeats arrive flattened, which rides identically and only
  changes the block count shown.
- **Planned workouts live in their own table; library workouts live in `workouts`.**
  A calendar entry is a dated instance, not a library item, so it goes in
  `planned_workouts (event_id INTEGER PK, workout_uuid TEXT NOT NULL, date TEXT, name,
  description, activity_type, planned_load INTEGER NULL, duration_seconds INTEGER NULL,
  workout_json TEXT NULL, parse_error TEXT NULL, intervals_updated TEXT, fetched_at TEXT)`.
  `workout_uuid` is derived, `Uuid::new_v5(BLAKEBIKE_NAMESPACE, event_id)`, and is also
  the embedded `Workout.id`, so a sync finishing between render and Start still resolves
  the id the UI is holding, and `sessions.workout_id` stays meaningful across re-syncs
  (`uuid` gains the `v5` feature). `workout_json` is `NULL` when the event has no
  structure (`parse_error` `NULL`: free-form or notes only) or when its ZWO failed to
  parse (`parse_error` set); the home card shows name and load either way, offers
  "Start free ride", and shows the parse error when there is one. Library workouts are
  mirrored into `workouts` with `source = "intervals"` — `source` (src-tauri/src/domain.rs:55)
  is the one discriminator, already shown on the card (src/App.tsx:952) — plus a new
  `origin: Option<WorkoutOrigin { externalId, folderId, folder, updated, plannedLoad }>`
  carrying only what is new. `origin` has `#[serde(default)]`, so every stored workout
  still loads with `None`. A new `external_id` column on `workouts` (added through
  `ensure_column`, src-tauri/src/storage.rs:1080, written by `save_workout` from
  `origin`, with a `UNIQUE` index) is the upsert key so a re-sync keeps the local `Uuid`
  stable and ride history keeps pointing at the same workout. Both caches are on disk,
  so an offline launch shows the last plan and library.
- **Mirrored workouts are read-only, enforced in the backend.** Deleting one would
  bring it back on the next sync, and editing it locally would be undone.
  `commands::save_workout` and `commands::delete_workout` reject a workout whose stored
  copy has an `origin` ("This workout is mirrored from Intervals.icu; edit a copy or turn
  off library sync"), so the guard has one home and the UI merely stops offering the
  buttons. The library card for an `intervals` workout shows an "Intervals.icu · Folder"
  badge instead of `SOURCE` (src/App.tsx:952), drops Delete and replaces Edit with "Edit
  a copy", which opens the editor on a fresh local `Workout` (new id, `source: "local"`,
  no `origin`). Ride and Export work as for any workout. Workouts that disappear from
  the Intervals.icu library are removed locally by the sync itself (a storage call, not
  the guarded command); sessions keep `workout_name` so history is unaffected. Folder
  names are resolved from the fetched folder list on every sync and `folder_id` is part
  of the change comparison, so a rename or move on Intervals.icu updates the badge.
- **Only cycling comes across.** Events and library workouts whose `type` does not
  contain "ride" (case-insensitive: `Ride`, `VirtualRide`, `GravelRide`, …) are skipped
  and counted in the report, matching how the Ride sport settings are chosen
  (src-tauri/src/intervals.rs:83). Events are fetched with `category=WORKOUT` for the
  local dates today .. today + 6 (`chrono::Local`), so a day rollover while offline still
  finds tomorrow's plan in the cache. `icu_training_load` is stored as `planned_load`
  even though nothing consumes it yet; "ride to today's target load" reads it next.
- **Starting a planned workout goes through the existing runner and the existing
  command.** `start_workout` (src-tauri/src/commands.rs:569) resolves its id with
  `storage.workout(id)` (src-tauri/src/commands.rs:575). It gains a fallback to the
  planned cache (`storage.planned_workout_by_uuid`), so one command and one code path
  start both kinds. In the UI, `App` composes `rideable = [...todaysPlanned, ...workouts]`
  and hands that to `Ride` (src/App.tsx:611), so today's plan appears first in the ride
  picker with a "Today" chip and the timeline's `activeWorkout` lookup by name
  (src/App.tsx:1019) keeps working. The home card's Start button calls the same
  `onRide(uuid)`.
- **Sync runs on launch and on demand; the card says so.** A periodic timer would be
  invisible and would fight the offline story. `sync_intervals` runs after `load()`
  without blocking first paint, and again from "Sync now" in Settings and the refresh
  icon on the home card. Two toggles, stored in a new `IntervalsSyncSettings
  { calendar: bool, library: bool }` (both default on once a key exists), gate the two
  halves; unlike the zone toggles they are not part of any draft and save on change.
  The card shows "Synced when blake.bike starts and when you press Sync now", the last
  sync time, and the last error if any. A launch-time failure is not a toast: the home
  card shows the cached plan with "Last synced 2 h ago · Intervals.icu unreachable", and
  Settings shows the error text.
- **`App` owns every piece of Intervals.icu state and runs every sync.** Today
  `SettingsPage` fetches `intervalsApiKeyConfigured` in its own effect and flips a local
  flag (src/App.tsx:1622); with a home card and a settings card both showing status,
  two copies would drift. `App` holds `intervalsStatus` and `plannedWorkouts`, passes
  them down, and exposes `onSyncIntervals`, `onSaveIntervalsKey`, `onClearIntervalsKey`
  and `onIntervalsSyncSettings` callbacks the same way it already routes zone and
  profile saves (src/App.tsx:665). After a sync whose library half succeeded, and after
  clearing the key, `App` reloads `workouts` from storage, because the mirror lands in
  the same table the library renders from. The Overview keeps two refresh icons with
  different tooltips: the FTP card's "Sync FTP and zones from Intervals.icu" rewrites
  the profile, the Today card's "Refresh plan" only refreshes the mirrors.
- **Training-settings sync stays a separate button.** It rewrites the rider's FTP and
  zones, which is a deliberate act; the calendar and library sync are read-only mirrors
  that are safe to run on every launch. Bundling them would make launch mutate the
  profile.

### Changes

#### `src-tauri/src/intervals.rs`

- `IntervalsClient::athlete() -> Result<IntervalsAthlete, IntervalsError>`.
- `planned_workouts(&self, athlete_id, oldest: NaiveDate, newest: NaiveDate) -> Result<Vec<PlannedWorkoutFetch>>`:
  `GET events?oldest&newest&category=WORKOUT&ext=zwo`, decode base64, `import_zwo`,
  keep name/description/type/load/duration/updated from the event. Each fetch carries
  `workout: Option<Workout>` and `parse_error: Option<String>`; a ZWO that fails to
  parse (including `Workout::validate`'s six-hour cap and empty-steps rule,
  src-tauri/src/domain.rs:85) is a per-event failure, not fatal for the sync.
- `library(&self, athlete_id) -> Result<LibraryFetch { folders, workouts }>` from
  `GET /workouts` and `GET /folders` (folder names for the badge).
- `workout_zwo(&self, athlete_id, raw: &serde_json::Value) -> Result<String>` via
  `POST download-workout.zwo`.
- Test helper grows a `serve_routes(Vec<(path_prefix, status, body)>)` variant that
  answers several requests on one listener, since a library sync is a `GET` plus `POST`s.

#### `src-tauri/src/storage.rs`

- Settings keys `intervals_athlete` and `intervals_sync` (`IntervalsSyncSettings`
  plus `last_synced_at`, `last_error`).
- `planned_workouts` table with `replace_planned_workouts(rows)`, `planned_workouts()`
  (the whole cached range; the UI picks today by local date),
  `planned_workout_by_uuid`, `clear_planned_workouts`.
- `workouts.external_id` column with a unique index; `save_workout` writes it;
  `workout_by_external_id`, `intervals_workouts()`,
  `delete_intervals_workouts_not_in(external_ids)`.
- `sync_library(fetched)` is a pure-ish merge: for each Ride workout, compare
  `updated` and `folder_id` with the stored origin, upsert new/changed, delete vanished.
  Returns `LibrarySyncReport { added, updated, removed, skipped }`.

#### `src-tauri/src/domain.rs`

- `Workout.origin: Option<WorkoutOrigin>`; `WorkoutOrigin { external_id, folder_id,
  folder, updated, planned_load }`. `Workout::new` sets `None`.

#### `src-tauri/src/formats.rs`

- `MaxEffort` → `FreeRide`, `SolidState` → `Steady`, unknown `<workout>` children fail
  with the element name; depth tracking so step children stay ignored.

#### `src-tauri/src/commands.rs` and `lib.rs`

- `save_intervals_api_key` async with athlete resolution; `clear_intervals_api_key`
  purges.
- `resolve_athlete(state)` helper (stored → fetch-and-store → error).
- `sync_intervals -> IntervalsSyncReport { syncedAt, calendar: {fetched, unstructured,
  failed, skipped} | error, library: LibrarySyncReport | error }`; runs the enabled
  halves, records `last_synced_at` / `last_error`.
- `planned_workouts -> Vec<PlannedWorkout>` (the cached range; the UI picks today by
  local date), `set_intervals_sync_settings` (turning a mirror off purges its copies
  at once, so nothing read-only lingers that the sync no longer refreshes), and
  `intervals_status -> { configured, athleteId, athleteName, settings, lastSyncedAt,
  lastError }` replacing `intervals_api_key_configured`; the settings are read
  through the status rather than a command of their own.
- `start_workout` fallback to the planned cache; `save_workout` / `delete_workout`
  reject mirrored workouts.

#### Frontend

- `src/types.ts`: `WorkoutOrigin`, `Workout.origin?`, `PlannedWorkout`,
  `IntervalsSyncSettings`, `IntervalsStatus`, `IntervalsSyncReport`, plus
  `describeIntervalsSync(report)` for the status line.
- `src/api.ts`: the new commands; `intervalsApiKeyConfigured` replaced by
  `intervalsStatus`.
- `src/App.tsx`:
  - `App` state `plannedWorkouts`, `intervalsStatus`; loads them in `load()`; calls
    `api.syncIntervals()` once after load and then reloads planned workouts, status
    and (when the library half ran) `workouts`, on success or failure. `SettingsPage`
    loses its own configured-flag fetch and takes `intervalsStatus` plus callbacks.
  - `Overview` gets a `TodayCard` above "Up next" (src/App.tsx:881): name, description,
    `WorkoutProfile` preview, duration, planned load, Start (or "Start free ride" when
    unstructured), plus the last-synced / unreachable line and a refresh icon. When
    nothing is planned it says so in one line rather than hiding.
  - `WorkoutLibrary` (src/App.tsx:905) renders the origin badge and read-only actions.
    It stays a props-only component so `src/WorkoutLibrary.test.tsx` keeps working.
  - `Ride` picker (src/App.tsx:1395) shows the "Today" chip for planned entries.
  - `SettingsPage` Intervals.icu card: athlete name once resolved, the two sync
    toggles saved immediately (they are not part of the zone draft), the cadence
    sentence, last sync line and "Sync now".
- `CHANGELOG.md` Unreleased entry (Added, plus a Changed line: ZWO import now refuses
  files with unknown step elements instead of dropping them, which also applies to the
  manual Import ZWO button).

### Tests

- `src-tauri/src/intervals.rs`: athlete id resolution; events list decoded from a
  canned base64 ZWO into steps with load and date; an event without a workout file
  has `workout: None` and no error; a broken ZWO reports per event; library `GET` +
  `POST` round trip against `serve_routes`; non-ride types skipped.
- `src-tauri/src/commands.rs`: `resolve_athlete` with a key but no stored athlete
  fetches and stores it (storage-level: key present, athlete absent, then present).
- `src-tauri/src/formats.rs`: `MaxEffort`, `SolidState`, unknown element error,
  `textevent` still ignored.
- `src-tauri/src/storage.rs`: planned cache replace/read/by-uuid/clear, and replacing
  twice yields the same `workout_uuid`; `external_id` upsert keeps the `Uuid`; library
  merge adds, updates (including a folder move), removes and leaves local workouts
  alone; `origin` round-trips and old payloads load with `None`.
- `src-tauri/src/commands.rs`: `start_workout` resolution order (library first, then
  planned) via a storage-level test; mirrored workouts rejected by save/delete; report
  shaping.
- `src-tauri/src/formats.rs` also: the manual import path (`import_zwo_workout`) still
  imports a plain Zwift file.
- `src/types.test.ts`: `describeIntervalsSync`.
- `src/Overview.test.tsx` (new; `Overview` becomes an export): today card with a
  structured plan starts it, unstructured plan offers free ride, empty day, and the
  unreachable line.
- `src/WorkoutLibrary.test.tsx`: mirrored workout shows the badge, has no Delete, and
  "Edit a copy" hands the caller a new local workout.
- `src/SettingsPage.test.tsx`: sync toggles persist immediately; "Sync now" renders
  the report line; athlete name shown.

### Verification

1. `pnpm check`, `pnpm test`, Rust checks.
2. Scratchpad Vite harness for `Overview`, `WorkoutLibrary` and the Settings card with
   fabricated planned workouts and origins.
3. Blake: save key (athlete name appears), plan a workout on Intervals.icu for today,
   relaunch, see it on the home screen, start it, confirm the library shows folder
   badges and that "Edit a copy" leaves the mirror untouched. Then quit, go offline,
   relaunch, and confirm the cached plan still shows with the unreachable line.

## Risks

- **ZWO fidelity from Intervals.icu's exporter is unverified against a real download.**
  The spec and forum guide confirm the transport, not the exact elements emitted.
  Mitigation: the importer now fails loudly on unknown step elements instead of
  dropping them, the sync reports per-event parse failures, and the first real sync
  in step 3 above is the check. If something common is missing, `import_zwo` grows
  another arm — the transport does not change.
- **Library sync cost.** First sync of a large library is one `POST` per cycling
  workout, sequential, on the async runtime, not blocking the UI. Later syncs touch
  only changed workouts.
- **Two PRs, one storage file.** Phase 2 adds columns and a table; both use
  `IF NOT EXISTS` / `ensure_column`, so a Phase 1 build opening a Phase 2 database is
  fine and vice versa.
- **Test helper serving several requests** needs care with `Connection: close` and
  request ordering; keep it single-threaded and deterministic (routes matched by path
  prefix, not by order).

## Out of scope

- Pushing completed activities or FIT files to Intervals.icu (and it would not reach
  Garmin Connect anyway).
- Writing workouts or events back to Intervals.icu.
- "Ride to today's target load" — the `planned_load` column and `TARGET` events are
  the hook; the category filter is a parameter so `TARGET` can be added without
  reshaping the cache.
- Wellness, activities, folders as a browsable tree in the UI (the badge carries the
  folder name).
