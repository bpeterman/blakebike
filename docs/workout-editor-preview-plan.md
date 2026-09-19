# Workout Editor Preview — Plan

## Goal

Show a live picture of the workout while it is being built in the workout
editor. As the rider adds, removes, or edits blocks, a power-over-time profile
above the step list updates immediately, so the shape of the session
(warm-up, intervals, ramps, recovery, free-ride sections) is obvious without
reading durations and percentages row by row.

The preview must reflect what the ride runner will actually execute: the same
interval expansion (including repeat groups) and the same watt targets for the
current FTP. When this lands there must be exactly one way the app draws a
workout, and it must be built so that repeat-group editing, drag editing, and
the live-ride timeline can be layered on later without a rewrite.

## Current state

- `WorkoutEditor` in `src/App.tsx` is a modal with a name field, description,
  a flat list of `step-editor` rows, an "Add block" strip, and Save/Cancel.
  It shows total duration as text but has no visual of the workout.
- The editor does not receive the rider's FTP or training-zone settings; only
  the parent `App` has `profile` and `trainingZones`.
- Step rows are keyed by `${index}-${step.kind}`. `EditableNumberInput` holds
  its own draft text, so deleting a row lets React reuse the next row's input
  with the deleted row's draft. This is a latent bug today and would make any
  row-to-chart highlighting unreliable.
- `updateStep` spreads an untyped `Partial<WorkoutStep>` patch and casts the
  result, so nothing stops a `target` being written onto a free-ride step.
- `WorkoutBars` (library cards and the ride select list) is a rough sketch:
  bar height is `% FTP / 1.4`, ramps use their higher end as a flat bar, free
  ride is a fixed stub, every bar has a 3% minimum width, watt targets are
  treated as percentages, and there is no axis or zone colour.
- `WorkoutTimeline` (live ride) draws each compiled interval as a trapezoid
  with `clip-path`, scaled to peak watts. Its geometry math is inline and tied
  to ride progress state.
- `compileWorkoutIntervals(steps, ftpWatts)` in `src/types.ts` mirrors the
  Rust compiler in `src-tauri/src/domain.rs` and is covered by
  `types.test.ts`. It expands repeat groups but discards which step produced
  each interval, and the recursion is private to that function.
- Zone helpers exist (`effectivePowerZones`, `zoneIndex`), but the
  `zoneColors` palette lives in `App.tsx` and is not importable elsewhere.
- Repeat groups appear in the editor as a read-only "N× repeat group" row
  (they arrive from ZWO import). The editor cannot create or edit them.
- Component tests live beside their components (`WorkoutLibrary.test.tsx`,
  `ConfirmDialog.test.tsx`) using Testing Library and Vitest under jsdom.

## Principles

- **One expansion, one geometry, one renderer.** Every place that turns
  steps into intervals, intervals into shapes, or shapes into pixels goes
  through a single tested module. No parallel implementations, including the
  existing `WorkoutBars`, survive this change.
- **Identity by path, not by top-level index.** A step is addressed by its
  `number[]` path through nested repeat groups. Flat workouts have
  single-element paths. This is what makes repeat-group editing possible later
  without touching the preview.
- **Pure data first, DOM last.** Expansion, statistics, and geometry are pure
  functions with unit tests. The React component only maps geometry to SVG
  elements and wires events.
- **No visual hacks.** No `preserveAspectRatio="none"` (it distorts text and
  strokes), no minimum widths that break proportionality, no `clip-path`
  gradients. Measure the container and draw in pixel space.
- **Behaviour-preserving refactors land first,** each with the existing tests
  still green, before any new UI.

## Design

### 1. `src/workoutSteps.ts` — expansion (refactor of existing code)

```ts
export type StepPath = readonly number[];

export type LeafStep = Exclude<WorkoutStep, { kind: "repeat" }>;

export type ExpandedStep = {
  step: LeafStep;
  /** Index path through workout.steps, into nested repeat groups. */
  path: StepPath;
  /** For each repeat ancestor on the path, the 0-based iteration. */
  iterations: readonly number[];
  startSeconds: number;
  endSeconds: number;
};

export const expandWorkoutSteps = (steps: WorkoutStep[]): ExpandedStep[];
export const stepAtPath = (steps: WorkoutStep[], path: StepPath): WorkoutStep | undefined;
export const replaceStepAtPath = (steps: WorkoutStep[], path: StepPath, step: WorkoutStep): WorkoutStep[];
export const pathsEqual = (a: StepPath, b: StepPath): boolean;
export const pathStartsWith = (path: StepPath, prefix: StepPath): boolean;
```

- `compileWorkoutIntervals` and `workoutDuration` in `types.ts` are
  reimplemented as thin maps over `expandWorkoutSteps`. Their signatures and
  the existing tests do not change. The "mirrors the Rust compiler" guarantee
  now lives in one recursion.
- `replaceStepAtPath` is included now because the editor's `updateStep` will
  use it (see §4), and it is the primitive repeat-group editing will need.
- Tests: expansion order and counts for nested repeats, `path`/`iterations`
  correctness, `startSeconds` accumulation, and an invariant test that
  `compileWorkoutIntervals` output is unchanged for the fixtures already in
  `types.test.ts`.

### 2. `src/zones.ts` — palette

Move `zoneColors` out of `App.tsx` and add
`zoneColor(watts: number, zones: ZoneDefinition[]): string`, which applies
the same `zoneIndex` + modulo rule the time-in-zone chart uses today. `App.tsx`
imports from here; nothing else about zones changes.

### 3. `src/workoutProfileModel.ts` — statistics and geometry (pure)

```ts
export type ProfileSegment = ExpandedStep & {
  startWatts: number | null;   // null = free ride
  endWatts: number | null;
};

export type ProfileStats = {
  totalSeconds: number;
  freeRideSeconds: number;         // reported, never silently folded in
  averagePercentFtp: number | null; // time-weighted over targeted time; null if none
  estimatedStress: number | null;  // IF² × hours × 100 over targeted time
  peakWatts: number;
};

export type ProfileScale = {
  width: number;
  height: number;
  totalSeconds: number;
  maxWatts: number;   // max(peakWatts, 1.2 × ftp)
  x: (seconds: number) => number;
  y: (watts: number) => number;
};

export type ProfileShape = {
  path: StepPath;
  iterations: readonly number[];
  kind: LeafStep["kind"];
  points: readonly [number, number][];  // polygon in pixel space
  startWatts: number | null;
  endWatts: number | null;
};

export const profileSegments = (steps: WorkoutStep[], ftpWatts: number): ProfileSegment[];
export const profileStats = (segments: ProfileSegment[], ftpWatts: number): ProfileStats;
export const profileScale = (segments, ftpWatts, width, height): ProfileScale;
export const profileShapes = (segments: ProfileSegment[], scale: ProfileScale, gapPx: number): ProfileShape[];
export const timeAxisTicks = (totalSeconds: number, width: number): number[];
export const repeatSpans = (segments: ProfileSegment[]): { path: StepPath; repetitions: number; startSeconds: number; endSeconds: number }[];
```

- `profileSegments` maps each `ExpandedStep` through the same
  `targetWatts` the compiler uses (export it from `types.ts` rather than
  copying the clamp).
- `profileShapes` draws a gap only when the segment is wider than
  `2 × gapPx`; otherwise adjacent segments touch. Widths are never clamped.
- `timeAxisTicks` picks 1, 5, 10, or 15-minute spacing from the pixel width
  so labels never overlap.
- Tests: stats on mixed workouts (free ride excluded and reported), a 40 ×
  30/30 workout keeps proportional widths, ramps produce trapezoids with the
  correct corner heights, tick spacing at several widths, and an invariant
  that `profileSegments(...).map(s => ({...}))` deep-equals
  `compileWorkoutIntervals(...)`.

### 4. `src/WorkoutProfile.tsx` — the one renderer

```tsx
<WorkoutProfile
  steps={workout.steps}
  ftpWatts={ftp}
  powerZones={powerZones}          // already-resolved ZoneDefinition[]
  variant="editor" | "compact"
  highlightedPath={path | null}    // prefix match: a repeat highlights its children
  onHighlightPath={(path | null) => void}
  onSelectPath={(path) => void}
  segmentState={(shape) => ({ className?, progress? })}  // optional, for the live timeline later
  ariaLabel={string}               // caller supplies the summary sentence
/>
```

- Measures its own width with a small `useElementWidth` hook
  (`ResizeObserver`, falling back to a fixed width when unavailable). A
  `ResizeObserver` stub goes in `src/test-setup.ts`. Height comes from the
  variant via CSS and is read the same way.
- Renders `<figure role="img" aria-label>` containing a pixel-space
  `<svg width height>`. Shapes are `<polygon>` with `data-path` and
  `data-highlighted`. Ramps use a `<linearGradient>` from
  `zoneColor(startWatts)` to `zoneColor(endWatts)`, ids namespaced with
  `useId()`. Free ride uses a shared hatch `<pattern>`.
- Editor variant adds: dashed FTP line with a "FTP" label, time axis ticks,
  and a repeat bracket + "N×" label from `repeatSpans`. Compact variant draws
  shapes only.
- Empty steps render the same frame with a muted message so the layout does
  not jump when the first block is added.
- Memoised on `[steps, ftpWatts, powerZones, width, height]`.
- Tests: one polygon per segment, ramp gradient present, free-ride hatch,
  highlight prefix matching, `onSelectPath` fires with the correct path,
  empty state, and that an FTP change moves the FTP line.

### 5. Editor changes (`App.tsx` → `src/WorkoutEditor.tsx`)

Move `WorkoutEditor` and `TargetInput` into their own file and export them.

- **Props:** add `ftpWatts: number` and `powerZones: ZoneDefinition[]`. `App`
  already memoises `effectivePowerZones(trainingZones, profile.ftpWatts)`;
  pass that result rather than the settings object so the editor does not
  depend on `TrainingZoneSettings`.
- **Stable identities:** editor state becomes
  `{ meta: Omit<Workout, "steps">; rows: { id: string; step: WorkoutStep }[] }`
  with ids from `crypto.randomUUID()` assigned on open. Rows are keyed by
  `id`. On save the wrapper is stripped back to `Workout`. This fixes the
  stale-draft bug and gives highlighting a stable target. Top-level path
  `[i]` ↔ `rows[i].id` is the only mapping needed today; nested paths map
  through `stepAtPath` when repeat editing arrives.
- **Type-safe updates:** replace the cast-based `updateStep` with
  `updateRow(id, (step: WorkoutStep) => WorkoutStep)` and kind-narrowed
  helpers (`setDuration`, `setSteadyTarget`, `setRampTargets`). No `as
  WorkoutStep` remains.
- **Preview:** render `<WorkoutProfile variant="editor" …/>` between the
  description and `.step-list`, with `highlightedPath` derived from a
  `highlightedRowId` state.
- **Linking:** each row gets `onMouseEnter/Leave` and `onFocus/Blur` (via
  `focusin`/`focusout` on the row) to set `highlightedRowId`;
  `onHighlightPath` from the chart sets the same state;
  `onSelectPath` scrolls the row into view and focuses its first input via a
  `Map<id, HTMLElement>` of row refs.
- **Stats strip:** duration, average `% FTP`, estimated stress, and a
  "N min free ride excluded" note when `freeRideSeconds > 0`.
- Tests in `src/WorkoutEditor.test.tsx`: adding a block adds a polygon;
  changing a duration changes polygon width; deleting the first of two
  steady rows leaves the second row's input showing its own value
  (regression for the key bug); hovering a row highlights its polygon and
  vice versa; clicking a polygon focuses the row; stats update; save
  produces a `Workout` with no editor-only fields.

### 6. Replace every other drawing of a workout

- Library cards and the ride select list use
  `<WorkoutProfile variant="compact" …/>`. `WorkoutBars` and
  `flattenSteps` are deleted. `.workout-visual` / `tone-N` backgrounds stay.
- `WorkoutTimeline` keeps its DOM and progress overlay for now, but its
  inline trapezoid math is replaced by `profileScale` + `profileShapes` so
  there is no second copy of the geometry. `segmentState` on
  `WorkoutProfile` exists so the timeline can be migrated onto the component
  as a later, self-contained change; it is not required for this work.
- `WorkoutLibrary.test.tsx` and `Ride.test.tsx` are updated where they
  assert on the old bars.

### 7. CSS

- `.workout-profile` with `--profile-height` set per variant (150px editor,
  92px library card, 37px select list). Colours reference the existing greys
  for axis and FTP line; zone fills come from `zones.ts` via attributes.
- `[data-highlighted="true"]` on polygons and on `.step-editor` share one
  outline colour.
- `prefers-reduced-motion`: no transitions on shapes.

## Phases

Each phase is a commit that leaves `pnpm check` and `pnpm test` green. Phases
1–6 ship as one PR.

1. `workoutSteps.ts`: `expandWorkoutSteps` + path helpers; rebase
   `compileWorkoutIntervals` and `workoutDuration` onto it; export
   `targetWatts`. Tests.
2. `zones.ts`: move palette, add `zoneColor`. Tests.
3. `workoutProfileModel.ts`: segments, stats, scale, shapes, ticks, repeat spans.
   Tests, including the 30/30 proportionality case.
4. `WorkoutProfile.tsx` + `useElementWidth` + `ResizeObserver` stub + CSS.
   Component tests.
5. `WorkoutEditor.tsx`: extract, stable row ids, type-safe updates, preview,
   linking, stats. Editor tests.
6. Replace `WorkoutBars` everywhere; refactor `WorkoutTimeline` geometry;
   delete dead code; update affected tests.

## Verification

- `pnpm check` and `pnpm test` pass (run `nvm use` first; shells default to
  Node 16).
- Manual, in the running app:
  - New workout: empty frame, then one block per added Steady / Ramp / Free
    ride, each with the right shape, zone colour, and hatch.
  - Edit a duration and a `% FTP` value; the block resizes immediately and
    the stats strip changes.
  - Delete the first of two steady rows; the remaining row still shows its
    own values.
  - Import a ZWO with repeat groups; the repeat bracket, block count, and
    total duration match the ride screen's `WorkoutTimeline` when started.
  - Change FTP in Settings, reopen the editor; the FTP line and zone colours
    move, and library-card thumbnails match the editor's shapes.
  - Hover a row and a block in each direction; click a block and confirm the
    row scrolls into view and its duration input gets focus.
  - Resize the window below 620px; the preview stays proportional, text is
    not distorted, and the modal does not overflow.

## Extension points (designed in, not built)

- **Repeat-group editing:** paths, `stepAtPath`, `replaceStepAtPath`, and
  prefix-matched highlighting already handle nesting. Adding "Add repeat"
  and nested rows to the editor needs no change to expansion or rendering.
- **Drag editing in the chart:** shapes are in pixel space with `data-path`,
  so pointer hit-testing maps directly to a step path and
  `replaceStepAtPath`.
- **Live-ride timeline on the same component:** `segmentState` supplies
  per-segment class and progress; the timeline's scroll/zoom track and
  countdowns stay outside the component.
- **Other target types (cadence, heart rate):** `profileScale` is the only
  place that knows the y-axis is watts. A second axis would add a parallel
  scale, not a new renderer.

## Out of scope

- Editing repeat groups. The preview renders them correctly; editing is a
  separate feature enabled by this work.
- Drag-to-resize blocks in the chart.
- Migrating `WorkoutTimeline` fully onto `WorkoutProfile`. Only its geometry
  math is shared in this change.
- Any Rust change. The TypeScript compiler contract and its Rust mirror are
  untouched; only the TypeScript implementation is restructured.

## Risks

- **Refactoring the compiler path.** Rebasing `compileWorkoutIntervals` onto
  `expandWorkoutSteps` touches ride-critical code. Mitigation: existing
  fixtures stay, plus an added deep-equality invariant test between the old
  and new outputs before the old implementation is removed.
- **`ResizeObserver` in tests.** A stub that reports a fixed width keeps
  component tests deterministic; geometry correctness is covered by the pure
  tests, not by jsdom layout.
- **Custom zones with fewer than seven bands.** `zoneColor` uses the same
  modulo rule as the time-in-zone chart, and a test covers a 3-zone list.
- **Modal height.** Chart plus stats add ~190px inside a `max-height: 90vh`
  modal. The preview sits above the scrolling step list; if it should stay
  visible while scrolling, make it `position: sticky` within the modal.
- **Editor-only ids leaking into saved workouts.** The save path is a
  single function that strips the wrapper, and a test asserts the saved
  object deep-equals a plain `Workout`.
