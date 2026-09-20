# Ride Screens — Plan

A Garmin-style ride display: several screens you page between, each a grid of
slots, each slot showing a *field* you choose. Replaces the single ordered list
of fixed cards that `RideDisplayPreferences` v2 describes.

## Decisions

- **A field is a descriptor, not a card type.** `{ metric, scope, aggregate }`
  instead of one `RideCardId` per combination. Average power, interval average
  power and max power are then one metric with three modifiers, not three card
  types. Without this, every new number is another entry in `rideCardIds`,
  another entry in `RIDE_CARDS`, another `cardContent` branch and another label
  — which is exactly the growth we already have after adding energy, average
  power, W/kg, distance, elapsed and remaining.
- **Panels stay panels.** The workout timeline, target & bias, the two charts,
  time-in-zone and stats-for-nerds are not single numbers and are not worth
  forcing into the field model. A screen item is either a field slot or a
  panel, and panels keep their existing components unchanged.
- **Not every combination is legal.** `{ heartRate, interval, total }` is
  meaningless. One `rideFieldDefinition()` decides what a field is called, what
  unit it carries and how to compute it; anything it does not recognise does
  not exist. The editor offers only what that table defines, so the UI cannot
  produce a field the resolver cannot render.
- **One resolver, one source of truth for each number.** Fields are computed by
  a single pure function over a `RideFieldContext` (telemetry, history, runner,
  profile, zones). No card computes its own number inline, which is what keeps
  "average power" the same number wherever it appears.
- **Distance stays a backend number.** The live estimate already comes from
  `RunnerState.distanceMeters`, fed by the same `DistanceAccumulator` that
  saves the session. The field reads it; it does not re-derive it.
- **Migration is silent and lossless.** A v2 preference becomes a single screen
  whose items are the v2 cards in their saved order and visibility. Nobody
  loses their layout, and nobody is asked to rebuild it.

## The model (TypeScript, mirrored in Rust)

```ts
type RideMetric =
  | "power" | "cadence" | "heartRate" | "speed" | "wattsPerKilogram"
  | "distance" | "energy" | "calories" | "time" | "targetPower";

type RideScope = "ride" | "interval";              // Garmin's "lap"
type RideAggregate = "current" | "average" | "max" | "total" | "remaining";

type RideField = { metric: RideMetric; scope: RideScope; aggregate: RideAggregate };

type RideSlotSpan = 1 | 2 | 4;                     // of a four-column grid

type RideScreenItem =
  | { kind: "field"; field: RideField; span: RideSlotSpan }
  | { kind: "panel"; panel: RidePanelId };

type RideScreen = { id: string; name: string; items: RideScreenItem[] };

type RideDisplayPreferences = { version: 3; screens: RideScreen[] };
```

`RidePanelId` is the surviving half of today's `RideCardId`:
`workoutTimeline | targetAndBias | powerChart | heartRateChart | timeInZone |
deviceStats`.

### Field definitions

`rideFieldDefinition(field)` returns `{ label, unit, resolve }` or `null`. The
legal set at first release:

| metric | scope | aggregates |
| --- | --- | --- |
| power | ride, interval | current, average, max |
| cadence | ride, interval | current, average, max |
| heartRate | ride, interval | current, average, max |
| speed | ride, interval | current, average, max |
| wattsPerKilogram | ride, interval | current, average |
| targetPower | ride | current |
| distance | ride | total |
| energy | ride, interval | total |
| calories | ride, interval | total |
| time | ride, interval | total, remaining |

`current` on an interval-scoped field means the same as on a ride-scoped one
and is not offered twice, and distance has no per-block total to offer because
the runner only totals it over the whole ride. `remaining` resolves to null on an open-ended ride,
and a field that resolves to null is skipped rather than rendered as a hole —
the behaviour today's `remainingTime` card already has.

### Scope

Interval scope is the ride's current workout interval, which the runner already
identifies (`intervalIndex`, `intervalElapsedSeconds`). The resolver slices
`telemetryHistory` to the samples inside the current interval; on a free ride
the interval is the whole ride, so interval-scoped fields equal ride-scoped
ones rather than disappearing.

## Changes

### Shared field model (new file, `src/rideFields.ts`)

- The types above, `rideFieldDefinition`, the legal-combination table, a
  `rideFieldKey(field)` for React keys and preference identity, and
  `resolveRideField(field, context)` returning `{ value, unit } | null`.
- The existing per-card arithmetic stays in `types.ts` (it is ride maths, not
  display) and the resolver calls it: `workKilojoules`,
  `kilojoulesToKilocalories`, and a generalized `timeWeightedMean`/`peak` pair
  so every averaged or peaked field shares one gap rule.
- The preference document lives beside it in `src/rideScreens.ts`, which keeps
  `types.ts` free of the display model and avoids an import cycle.

### Ride screen (`src/App.tsx`)

- `Ride` renders the active screen's items into the four-column grid: field
  slots become `LiveMetric` tiles at their span, panels render the components
  they render today.
- Screen paging: a `role="tablist"` strip of screen names (the current one
  `aria-selected`), `←`/`→` keyboard shortcuts added to `rideKeyAction`, and the
  active index held in `Ride` state, reset when a ride starts. The strip is
  hidden when there is only one screen.
- `cardContent` and `compactRideCards` are deleted; the panel half becomes
  `panelContent: Record<RidePanelId, ReactNode>`.

### Preferences (TypeScript, `src/types.ts`)

- The v3 document lives in `src/rideScreens.ts`; `rideCardIds`,
  `rideCardDefaultVisible`, `RideCardPreference` and the v2
  `RideDisplayPreferences` are removed from `types.ts`.
- `normalizeRideDisplayPreferences` drops unknown panels and illegal fields,
  drops empty screens, guarantees at least one screen, and caps screens at a
  sane number (8) so a corrupt file cannot produce a thousand tabs.
- `migrateRideDisplayPreferences(saved)` turns a v2 (or unrecognised) payload
  into v3: one screen named "Ride", visible v2 cards in order, metric cards
  mapped to their field equivalents at span 1, panels at span 4.
- `defaultRideDisplayPreferences` ships two screens, so the feature explains
  itself the first time it is seen: **Ride** (power, cadence, heart rate,
  speed, average power, W/kg, distance, elapsed, remaining, then the workout
  timeline, target & bias and the power chart) and **Detail** (the heart-rate
  chart, time in zone, energy, calories, interval average power).

### Preferences (Rust, `src-tauri/src/storage.rs`)

- `RideDisplayPreferences` mirrors the v3 shape. `RIDE_CARDS` is replaced by
  `RIDE_PANELS` and `RIDE_FIELDS`, the field-validity table. These duplicate
  the TypeScript lists, so a TypeScript test reads `storage.rs` and asserts the
  two sides hold exactly the same fields and panels — the duplication cannot
  drift silently.
- Loading deserializes v3, falling back to a v2 shape and migrating it, falling
  back to the default. `normalized()` enforces the same rules as the frontend.
- The frontend and backend normalizers are the same rules written twice; the
  Rust tests assert the migration's output field-by-field against the
  TypeScript fixtures' expectations.

### Settings (`src/RideScreensEditor.tsx`, rendered by `SettingsPage`)

The "Live ride cards" section becomes "Ride screens":

- A list of screens: rename, reorder, delete (never the last one), add.
- Inside a screen, a list of slots: each row is a field picker (metric, scope,
  aggregate selects that only offer legal combinations) or a panel picker, a
  span picker for fields, reorder buttons and a remove button.
- "Add field" / "Add panel" per screen.
- Everything stays in the existing draft-then-save shape, so one save writes
  the whole preference document.

### Styling (`src/App.css`)

- `.live-ride-grid` becomes an explicit four-column grid;
  `.ride-slot[data-span]` sets the span. Below 900 px every span collapses to
  two columns, below 600 px to one.
- A `.ride-screen-tabs` strip above the grid.
- Tile type scales with span so a span-1 tile is not the same 35 px as a span-4
  one.

## Risks

- **A rider's saved layout.** The migration is the only thing standing between
  an existing rider and an empty screen, so it is tested on both sides,
  including the "settings row is unparseable garbage" case that already falls
  back to defaults today.
- **Scope creep into the resolver.** The temptation is to add NP/IF/TSS in the
  same change. They are a separate metric family with their own windowing and
  belong in a follow-up, once the model is carrying real weight.
- **Keyboard collision.** `←`/`→` are free today, but the target input is not:
  the existing guard that ignores key events from `HTMLInputElement` covers it.

## Tests

- `src/rideFields.test.ts`: the definition table rejects illegal combinations;
  each aggregate resolves correctly over a fixture history; interval scope
  slices to the current interval; `remaining` is null on an open-ended ride;
  W/kg divides by rider weight, not rider plus bike.
- `src/types.test.ts`: v2 → v3 migration preserves order and visibility and
  drops hidden cards; an unknown payload yields the defaults; normalization
  drops illegal fields, unknown panels and empty screens, and keeps at least
  one screen.
- `src/Ride.test.tsx`: the active screen renders its slots at their spans;
  paging with the tabs and with `←`/`→` changes screens; a null-resolving field
  leaves no hole; panels still render.
- `src/SettingsPage.test.tsx`: adding, removing, reordering and re-spanning a
  slot; the field pickers offer only legal combinations; deleting the last
  screen is refused.
- `src-tauri/src/storage.rs`: v3 round-trips; a stored v2 document migrates to
  the same shape the frontend produces; the panel list matches the TypeScript
  one; corrupt input falls back to defaults.

## Verification

Run the ride screen in the scratch Vite harness with a synthetic history and
check each screen at desktop, 900 px and 600 px widths, then walk the Settings
editor through add / reorder / span / delete and confirm the ride screen
follows.

## Out of scope

- NP, IF and TSS; lap markers of the rider's own (the interval is the lap);
  per-screen auto-paging on a timer; drag-and-drop in the editor (the existing
  up/down buttons carry over); syncing layouts between machines.
