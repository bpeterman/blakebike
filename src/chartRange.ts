import { useCallback, useRef } from "react";
import type { Telemetry } from "./types";

/** A highlighted stretch of a ride chart, in active (pause-free) elapsed ms. */
export type ChartRange = { startMs: number; endMs: number };

export type SeriesSummary = { average: number; max: number; min: number; count: number };

export type RangeSummary = {
  range: ChartRange;
  durationMs: number;
  power: SeriesSummary | null;
  heartRate: SeriesSummary | null;
};

/** Drags shorter than this count as a click, which clears the selection. */
export const MIN_RANGE_MS = 1_000;

export const normalizeRange = (a: number, b: number): ChartRange => ({
  startMs: Math.min(a, b),
  endMs: Math.max(a, b),
});

const summarizeSeries = (values: number[]): SeriesSummary | null => {
  if (values.length === 0) return null;
  let total = 0;
  let max = -Infinity;
  let min = Infinity;
  for (const value of values) {
    total += value;
    if (value > max) max = value;
    if (value < min) min = value;
  }
  return {
    average: Math.round(total / values.length),
    max: Math.round(max),
    min: Math.round(min),
    count: values.length,
  };
};

/**
 * Average, maximum, and minimum power and heart rate for the samples inside
 * `range` (inclusive). Pass full-resolution samples, not the downsampled or
 * smoothed chart data, so peaks between chart points are not lost.
 */
export const summarizeRange = <T extends Pick<Telemetry, "powerWatts" | "heartRateBpm"> & { activeElapsedMs: number }>(
  samples: T[],
  range: ChartRange,
): RangeSummary => {
  const power: number[] = [];
  const heartRate: number[] = [];
  for (const sample of samples) {
    if (sample.activeElapsedMs < range.startMs || sample.activeElapsedMs > range.endMs) continue;
    power.push(sample.powerWatts);
    if (sample.heartRateBpm !== null) heartRate.push(sample.heartRateBpm);
  }
  return {
    range,
    durationMs: range.endMs - range.startMs,
    power: summarizeSeries(power),
    heartRate: summarizeSeries(heartRate),
  };
};

/** The subset of Recharts' chart mouse-event state the drag logic reads. */
export type ChartPointer = { activeLabel?: string | number | undefined };

/** The subset of the DOM mouse event the drag logic reads. */
export type ChartPointerEvent = { buttons?: number };

const labelMs = (pointer: ChartPointer): number | null =>
  typeof pointer.activeLabel === "number" && Number.isFinite(pointer.activeLabel)
    ? pointer.activeLabel
    : null;

const primaryButtonHeld = (event: ChartPointerEvent | undefined) =>
  event?.buttons !== undefined && (event.buttons & 1) === 1;

/**
 * Press-and-drag range selection over a Recharts chart with a numeric X axis.
 * Recharts reports the X value under the cursor as `activeLabel`; this hook
 * turns press, move, and release into a normalized `ChartRange`. Releasing
 * after moving less than `minRangeMs` is treated as a click and clears the
 * selection. The handlers are stable, so a memoized chart does not rerender
 * because of them.
 *
 * Recharts derives `activeLabel` from its hover state rather than from the
 * press itself, so a press that lands before any hover has no position. Such
 * a press is remembered as pending and anchored at the first move made with
 * the button still held.
 */
export function useChartRangeDrag(
  onSelectionChange: (range: ChartRange | null) => void,
  minRangeMs = MIN_RANGE_MS,
) {
  const anchorRef = useRef<number | null>(null);
  const latestRef = useRef<number | null>(null);
  const pendingRef = useRef(false);
  const changeRef = useRef(onSelectionChange);
  changeRef.current = onSelectionChange;

  const finish = useCallback(() => {
    pendingRef.current = false;
    const anchor = anchorRef.current;
    if (anchor === null) return;
    const latest = latestRef.current ?? anchor;
    anchorRef.current = null;
    latestRef.current = null;
    const range = normalizeRange(anchor, latest);
    if (range.endMs - range.startMs < minRangeMs) changeRef.current(null);
  }, [minRangeMs]);

  const onMouseDown = useCallback((pointer: ChartPointer) => {
    const ms = labelMs(pointer);
    pendingRef.current = ms === null;
    anchorRef.current = ms;
    latestRef.current = ms;
  }, []);

  const onMouseMove = useCallback((pointer: ChartPointer, event?: ChartPointerEvent) => {
    const ms = labelMs(pointer);
    if (ms === null) return;
    if (anchorRef.current === null) {
      if (!pendingRef.current || !primaryButtonHeld(event)) return;
      pendingRef.current = false;
      anchorRef.current = ms;
      latestRef.current = ms;
      return;
    }
    latestRef.current = ms;
    changeRef.current(normalizeRange(anchorRef.current, ms));
  }, []);

  const onMouseUp = useCallback((pointer: ChartPointer) => {
    if (pendingRef.current) {
      // Pressed and released without ever getting a position: a plain click.
      pendingRef.current = false;
      changeRef.current(null);
      return;
    }
    if (anchorRef.current === null) return;
    const ms = labelMs(pointer);
    if (ms !== null) latestRef.current = ms;
    finish();
  }, [finish]);

  return { onMouseDown, onMouseMove, onMouseUp, onMouseLeave: finish };
}
