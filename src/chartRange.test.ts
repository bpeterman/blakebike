import { renderHook } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { normalizeRange, summarizeRange, useChartRangeDrag } from "./chartRange";
import { downsampleTelemetry } from "./types";

const sample = (activeElapsedMs: number, powerWatts: number, heartRateBpm: number | null = null) => ({
  timestampMs: activeElapsedMs,
  activeElapsedMs,
  powerWatts,
  heartRateBpm,
  cadenceRpm: null,
  speedKph: null,
  targetPowerWatts: null,
});

describe("summarizeRange", () => {
  const samples = [
    sample(0, 100, 120),
    sample(1000, 200, 130),
    sample(2000, 300, null),
    sample(3000, 400, 150),
    sample(4000, 500, 160),
  ];

  it("includes both boundaries and rounds the mean", () => {
    const summary = summarizeRange(samples, { startMs: 1000, endMs: 3000 });
    expect(summary.durationMs).toBe(2000);
    expect(summary.power).toEqual({ average: 300, max: 400, min: 200, count: 3 });
    expect(summary.heartRate).toEqual({ average: 140, max: 150, min: 130, count: 2 });
  });

  it("returns null series when nothing usable falls in the range", () => {
    const empty = summarizeRange(samples, { startMs: 4500, endMs: 9000 });
    expect(empty.power).toBeNull();
    expect(empty.heartRate).toBeNull();
    const noHeartRate = summarizeRange(samples, { startMs: 2000, endMs: 2000 });
    expect(noHeartRate.power).toEqual({ average: 300, max: 300, min: 300, count: 1 });
    expect(noHeartRate.heartRate).toBeNull();
  });

  it("sees peaks that downsampled chart data drops", () => {
    const full = Array.from({ length: 100 }, (_, index) => sample(index * 1000, index === 50 ? 900 : 200));
    const range = { startMs: 40_000, endMs: 60_000 };
    expect(summarizeRange(full, range).power?.max).toBe(900);
    expect(summarizeRange(downsampleTelemetry(full, 10), range).power?.max).toBe(200);
  });
});

describe("normalizeRange", () => {
  it("orders the endpoints", () => {
    expect(normalizeRange(5000, 1000)).toEqual({ startMs: 1000, endMs: 5000 });
    expect(normalizeRange(1000, 5000)).toEqual({ startMs: 1000, endMs: 5000 });
  });
});

describe("useChartRangeDrag", () => {
  const setup = () => {
    const onChange = vi.fn();
    const { result } = renderHook(() => useChartRangeDrag(onChange));
    return { onChange, drag: result.current };
  };

  it("emits a normalized range while dragging and keeps it on release", () => {
    const { onChange, drag } = setup();
    drag.onMouseDown({ activeLabel: 5000 });
    expect(onChange).not.toHaveBeenCalled();
    drag.onMouseMove({ activeLabel: 3000 });
    expect(onChange).toHaveBeenLastCalledWith({ startMs: 3000, endMs: 5000 });
    drag.onMouseMove({ activeLabel: 9000 });
    expect(onChange).toHaveBeenLastCalledWith({ startMs: 5000, endMs: 9000 });
    drag.onMouseUp({ activeLabel: 9000 });
    expect(onChange).toHaveBeenCalledTimes(2);
  });

  it("treats a release without meaningful movement as a click that clears", () => {
    const { onChange, drag } = setup();
    drag.onMouseDown({ activeLabel: 5000 });
    drag.onMouseMove({ activeLabel: 5500 });
    drag.onMouseUp({ activeLabel: 5500 });
    expect(onChange).toHaveBeenLastCalledWith(null);

    onChange.mockClear();
    drag.onMouseDown({ activeLabel: 5000 });
    drag.onMouseUp({ activeLabel: 5000 });
    expect(onChange).toHaveBeenCalledWith(null);
  });

  it("finishes the drag when the cursor leaves the chart", () => {
    const { onChange, drag } = setup();
    drag.onMouseDown({ activeLabel: 1000 });
    drag.onMouseMove({ activeLabel: 8000 });
    drag.onMouseLeave();
    expect(onChange).toHaveBeenCalledTimes(1);
    drag.onMouseMove({ activeLabel: 20_000 });
    expect(onChange).toHaveBeenCalledTimes(1);
  });

  it("anchors a press that had no position at the first move with the button held", () => {
    const { onChange, drag } = setup();
    drag.onMouseDown({ activeLabel: undefined });
    drag.onMouseMove({ activeLabel: 2000 }, { buttons: 0 });
    expect(onChange).not.toHaveBeenCalled();
    drag.onMouseMove({ activeLabel: 2000 }, { buttons: 1 });
    drag.onMouseMove({ activeLabel: 7000 }, { buttons: 1 });
    expect(onChange).toHaveBeenLastCalledWith({ startMs: 2000, endMs: 7000 });
    drag.onMouseUp({ activeLabel: 7000 });
    expect(onChange).toHaveBeenCalledTimes(1);
  });

  it("treats a press and release that never had a position as a click", () => {
    const { onChange, drag } = setup();
    drag.onMouseDown({ activeLabel: undefined });
    drag.onMouseUp({ activeLabel: undefined });
    expect(onChange).toHaveBeenCalledWith(null);
  });

  it("ignores events without a numeric position", () => {
    const { onChange, drag } = setup();
    drag.onMouseMove({ activeLabel: 4000 });
    drag.onMouseUp({});
    drag.onMouseDown({ activeLabel: undefined });
    drag.onMouseMove({ activeLabel: 4000 });
    expect(onChange).not.toHaveBeenCalled();

    drag.onMouseDown({ activeLabel: 1000 });
    drag.onMouseMove({ activeLabel: "Monday" });
    drag.onMouseMove({ activeLabel: 6000 });
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange).toHaveBeenLastCalledWith({ startMs: 1000, endMs: 6000 });
  });
});
