# Chart Range Selection — Plan

## Goal

Let the rider press and drag horizontally across the Power and Heart rate
charts to highlight a section of the ride, then read the average, maximum, and
minimum for that section without leaving the chart. The selection stays put
after the mouse is released so the numbers can be read and compared, and it is
cleared with a click, a Clear button, or Escape.

The feature applies to both places `SessionAreaChart` is rendered: the ride
history detail modal (`RideDetailModal`) and the live ride screen (`Ride`).
Because both charts on a screen share one sample array and one time axis, a
range dragged on the power chart is highlighted on the heart-rate chart too, and
one summary strip reports power and heart rate together.

## Current state

- Charts live in `src/App.tsx`. `SessionAreaChart` is a memoized Recharts 3.10
  `AreaChart` with an `activeElapsedMs` numeric X axis, a default `Tooltip`, and
  no interaction beyond hover.
- Sample preparation is already factored into pure helpers in `src/types.ts`:
  `withActiveElapsed` (gap-aware elapsed time, rescaled to the reported
  duration) and `downsampleTelemetry` (caps chart data at 1,200 points, keeping
  first and last). `timeInZones` shows the existing convention for iterating
  samples with a 5 s gap cutoff.
- The history modal computes `samples` once per session. The live ride recomputes
  `chartHistory` every second because `elapsed` changes, so both charts already
  re-render on every tick.
- Whole-ride average and maximum power already appear as `Metric` tiles; there
  is no per-range statistic anywhere and no `ReferenceArea` or `Brush` usage.
- Recharts exposes `onMouseDown`, `onMouseMove`, `onMouseUp`, and `onMouseLeave`
  on `AreaChart`. Each handler receives a `MouseHandlerDataParam` whose
  `activeLabel` is the X value under the cursor (here, `activeElapsedMs`), which
  is all the drag logic needs. `ReferenceArea` with numeric `x1`/`x2` and
  `ifOverflow="discard"` draws the highlight.
- Tests use Vitest and Testing Library in jsdom. Existing tests render `Ride`
  with charts and pass, but `ResponsiveContainer` has zero size in jsdom, so
  Recharts never produces `activeLabel` from synthetic mouse events. Drag logic
  therefore needs to be testable without a real layout.

## Decisions

1. **Free time range, not interval snapping.** "Zones" in the request is read
   as arbitrary sections of the graph. Snapping the selection to workout
   interval boundaries is a listed follow-up, not part of the first release.
2. **Statistics come from full-resolution samples.** Averages and extremes are
   computed on the un-downsampled, un-smoothed samples restricted to the
   selected `activeElapsedMs` window. Downsampling drops two thirds of a
   one-hour ride, and a 30 s power-smoothing window hides true peaks, so
   computing on chart data would give wrong numbers. The chart only decides
   *which* window; a pure helper does the math.
3. **Selection state is lifted to the screen, not kept in the chart.** `Ride`
   and `RideDetailModal` own one `ChartRange | null` and pass it to both
   `SessionAreaChart` instances as a controlled prop. That is what makes the
   shared highlight and the single summary strip possible.
4. **Sticky selection with a drag threshold.** Mouse up ends the drag but keeps
   the range. A press-and-release that moves less than 1 s of ride time counts
   as a click and clears any existing range. Escape also clears.
5. **Mouse first.** BlakeBike is a Tauri desktop app driven by a mouse or
   trackpad. Touch drag is a follow-up.
6. **No backend changes.** Everything is derived in the frontend from samples
   the app already has.

## Phase 1 — Pure range summary helper

Add to `src/types.ts`:

```ts
export type ChartRange = { startMs: number; endMs: number }; // normalized, start <= end

export type RangeSummary = {
  durationMs: number;
  power: { average: number; max: number; min: number; count: number } | null;
  heartRate: { average: number; max: number; min: number; count: number } | null;
};

export const summarizeRange = <T extends Telemetry & { activeElapsedMs: number }>(
  samples: T[],
  range: ChartRange,
): RangeSummary
```

- Include samples with `startMs <= activeElapsedMs <= endMs`.
- Skip `null` heart rate. Return `null` for a series with zero usable samples.
- Average is the arithmetic mean of samples, rounded to a whole number,
  consistent with how `averagePowerWatts` is presented elsewhere.
- Add a `normalizeRange(a, b)` helper so callers can pass drag endpoints in
  either order.

Tests in `src/types.test.ts`: empty selection, inclusive boundaries, reversed
endpoints, null heart rate throughout, mixed nulls, and a check that summarizing
full-resolution samples differs from summarizing their downsampled version when
the peak falls between kept points.

Size: S (half a day including tests).

## Phase 2 — Drag state and highlight in `SessionAreaChart`

- New props on `SessionAreaChart`: `selection: ChartRange | null`,
  `onSelectionChange: (range: ChartRange | null) => void`.
- Local `dragStartMs` state (or `useRef`) records the `activeLabel` on
  `onMouseDown`. `onMouseMove` while dragging calls `onSelectionChange` with the
  normalized range. `onMouseUp` and `onMouseLeave` end the drag; if the range is
  shorter than the 1 s threshold, call `onSelectionChange(null)`.
- Ignore events whose `activeLabel` is `undefined` (cursor outside the plot).
- Render `<ReferenceArea x1={selection.startMs} x2={selection.endMs}
  ifOverflow="discard" fill={color} fillOpacity={0.18} stroke={color} />`
  when a selection exists. Keep the existing tooltip active during a drag.
- Set `cursor: crosshair` on the chart wrapper and `user-select: none` while
  dragging so text in the modal does not get highlighted.
- Extract the drag decisions into a small pure reducer or hook
  (`useChartRangeDrag`) whose handlers accept `{ activeLabel }` objects. That
  unit is testable in jsdom without layout; the Recharts wiring stays thin.

Tests: reducer/hook tests for start, extend, release above and below threshold,
leave-while-dragging, and ignored undefined labels. A render test that a
`ReferenceArea` is present when `selection` is supplied (Recharts renders an
SVG `rect` with the `recharts-reference-area` class).

Size: M (one day).

## Phase 3 — Ride history detail modal

- `RideDetailModal` owns `const [range, setRange] = useState<ChartRange | null>(null)`
  and passes it to both charts.
- Compute `fullSamples = withActiveElapsed(session.samples, elapsedSeconds)`
  once (the chart already computes this before downsampling; split the memo so
  both the chart data and the full-resolution data come from one pass).
- `summary = useMemo(() => range && summarizeRange(fullSamples, range))`.
- New `RangeSummaryStrip` component rendered between the metrics and the
  charts: time span (`0:12:30 – 0:17:30 · 5:00`), power avg/max/min in W, heart
  rate avg/max/min in bpm, and a Clear button. Reuse the `Metric` tile and the
  `.detail-metrics` grid so it matches the existing header. When no range is
  selected, show a one-line hint: "Drag across a chart to summarize a section."
- Escape clears the range when one exists instead of closing the modal; a
  second Escape closes it. This needs a small change to how `useDialog` handles
  the key, or a capturing handler on the modal that stops propagation when a
  range is set.
- Mark the strip `role="status"` so screen readers announce the numbers when a
  selection changes.

Tests in `src/PostRide.test.tsx` or a new `RideDetail.test.tsx`: render with a
session, assert the hint is shown, then drive the selection through the exposed
handlers (render `SessionAreaChart` directly with a stub `onSelectionChange`, or
expose a test-only initial range prop on the modal) and assert the strip text.

Size: M (one day).

## Phase 4 — Live ride screen

- Same lifting into `Ride`, passing `range` to the power and heart-rate charts.
- Statistics use `withActiveElapsed(telemetryHistory, elapsed)` (raw power, not
  `smoothedHistory`), memoized on `telemetryHistory` and `elapsed`. The chart
  keeps showing the smoothed series; the strip states "Raw power" in its label
  when smoothing is on so the numbers are not mistaken for smoothed values.
- The X domain grows every second; a fixed range in ms stays anchored to the
  same section of the ride, which is the desired behaviour. Verify that
  `withActiveElapsed`'s rescale to `elapsed` does not visibly walk the highlight
  during a paused ride. If it does, compute the range in raw elapsed and let the
  chart translate.
- Both live chart cards are collapsible. Collapsing a chart hides its highlight
  but must not clear the shared range. Place the summary strip inside the
  power card under the chart, next to the existing progress meta.
- Clear the range when a new ride starts (`runner.sessionId` changes).

Tests in `src/Ride.test.tsx`: range survives a rerender with new telemetry; range
resets on session change; collapsed heart-rate chart does not clear the range.

Size: M (one day).

## Phase 5 — Polish and verification

- CSS: highlight colour per chart, crosshair cursor, strip layout at the modal's
  narrow breakpoint (`.zone-chart-grid` already collapses to one column).
- Double-check `memo` on `SessionAreaChart` still prevents rerenders when only
  unrelated props change; `onSelectionChange` must be a stable callback.
- Manual verification with the simulated trainer (`pnpm tauri dev`, developer
  mode): drag on power, confirm the same range on heart rate, read the strip,
  click to clear, Escape to clear, drag off the plot edge, drag right-to-left,
  short click does nothing, selection persists while the live chart grows.
- Run `pnpm check` and `pnpm test`.
- README: add a bullet under Features for section statistics on ride charts.

Size: S (half a day).

## Test strategy

- Pure helpers (`summarizeRange`, `normalizeRange`, drag reducer) carry the
  correctness burden and are fully unit tested.
- Component tests assert rendering given a selection and that lifted state
  behaves across rerenders. They do not attempt to simulate Recharts hit
  testing in jsdom.
- Manual checklist in Phase 5 covers the pointer path that jsdom cannot.

## Delivery slices

1. Phase 1 + Phase 2 as one PR: helper, drag hook, highlight, no visible UI
   change yet beyond the highlight when a parent supplies a range.
2. Phase 3 as one PR: history modal end to end. This is the first user-visible
   slice and the one to demo.
3. Phase 4 + Phase 5 as one PR: live ride, polish, README.

Total: roughly four working days.

## Definition of done

- Dragging on either chart in history or live ride highlights the same time
  range on both charts and shows power and heart-rate average, max, and min for
  full-resolution samples in that range.
- Click, Clear, and Escape remove the selection. Sub-second drags do nothing.
- No regression in existing chart rendering, tooltips, or collapse behaviour.
- `pnpm check` and `pnpm test` pass; new helpers have unit tests.

## Follow-ups, not part of the first release

- Snap the selection to workout interval boundaries, or offer "select this
  interval" on click, if the request's "zones" meant intervals.
- Restrict the Time-in-zone bar charts to the selected range.
- Add normalized power, cadence, and speed to the summary strip.
- Touch and trackpad gesture support.
- Keyboard-driven range adjustment for accessibility.

## Open questions

- Was "zones" meant as free time ranges (assumed here) or workout intervals?
- Is raw power the right basis for the live-ride numbers while the chart shows
  smoothed power, or should the strip follow the smoothing setting?

## Status

Implemented on `feature/graph-range-select` across all phases, in one change.

Phase 4 (live ride) was implemented, verified, and then removed at Blake's
request on 2026-09-19: the selection was not judged useful mid-ride. The chart
component keeps `selection`/`onSelectionChange` as optional props, so the live
charts render exactly as before and the history modal is the only place that
opts in. Everything below about the live screen describes the removed slice.

- `src/chartRange.ts` holds the pure pieces: `summarizeRange`,
  `normalizeRange`, and the `useChartRangeDrag` hook. `src/chartRange.test.ts`
  covers them. `RangeSummaryStrip` and the wiring live in `src/App.tsx`, with
  `src/RangeSummary.test.tsx` covering the strip and the modal's initial state.
- Recharts derives the handler payload from its hover state, not from the
  press. A press that lands before any hover therefore has no position. The hook
  treats such a press as pending and anchors it at the first move made with the
  primary button still held, read from the DOM event Recharts passes alongside.
- Recharts focuses the chart on every press for its keyboard layer, which drew
  the browser's focus ring on each click. The ring is suppressed for pointer
  focus only (`:focus:not(:focus-visible)`), so keyboard focus still shows.
- Escape clears the selection in the history modal before it closes the modal.
- Extremes are rounded for display like the averages; whole-ride metrics are
  shown as integers elsewhere.
- Verified in a browser harness that renders `RideDetailModal` and `Ride` from
  the worktree with generated 1 Hz data: drag in both directions, shared
  highlight across both charts, click and Escape clearing, and the layout at
  800 px and 1300 px. The harness lives outside the repo and is not part of the change.
