import { describe, expect, it } from "vitest";
import { readyComparisonFixture } from "./dualPowerFixture";
import type { PowerComparison } from "./dualPower";
import {
  EXPORT_WIDTH,
  RecordingPainter,
  niceTicks,
  paintPowerComparison,
  reportTheme,
  timeTicks,
  wrap,
} from "./powerComparisonReport";

describe("power comparison report", () => {
  it("paints the ride, both devices, every headline number and the rules it followed", () => {
    const report = readyComparisonFixture();
    const painter = new RecordingPainter();
    const height = paintPowerComparison(painter, report, { width: EXPORT_WIDTH });
    expect(painter.size).toEqual({ width: EXPORT_WIDTH, height });
    expect(height).toBeGreaterThan(1_000);
    const text = painter.prose;
    // Ride and devices, firmware included.
    expect(text).toContain("Threshold");
    expect(text).toContain("KICKR CORE");
    expect(text).toContain("fw 1.2.3");
    expect(text).toContain("Assioma DUO");
    expect(text).toContain("fw 5.14");
    // Headline numbers with their sign, and the sign convention itself.
    expect(text).toContain("+5.1 W");
    expect(text).toContain("+2.4% · trainer − meter");
    expect(text).toContain(report.signConvention);
    // Lag, exclusion rule, coverage and calibration context are printed.
    expect(text).toContain("1.75 s");
    expect(text).toContain("trainer trails the meter");
    expect(text).toContain("under 30 W");
    expect(text).toContain("more than 50 W in a second");
    expect(text).toContain("2:48 excluded (1:36 coasting, 1:12 steps); 21:52 compared.");
    expect(text).toContain("Meter zeroed 1.5 h before the ride · offset 1019 (−4 vs the zero before).");
    // Every surviving bin prints its count; the warm-up summary is worded.
    for (const bin of report.byPower) expect(text).toContain(`${bin.count} s`);
    expect(text).toContain("150–200 W");
    expect(text).toContain("80–90 rpm");
    expect(text).toContain("first 10 min +3.6%, after that +1.8%");
    // The three windows are tabulated and the fit is described.
    expect(text).toContain("30 s");
    expect(text).toContain("+2.1 W flat, plus 1.4% of power");
    // The mark.
    expect(text).toContain("blake.bike");
    // Both traces are drawn in their own colours, 2 px, and gaps break them.
    const trainerLines = painter.lines.filter((line) => line.stroke === reportTheme.trainer);
    const meterLines = painter.lines.filter((line) => line.stroke === reportTheme.meter);
    expect(trainerLines.length).toBeGreaterThanOrEqual(2);
    expect(meterLines.length).toBe(trainerLines.length);
    expect(trainerLines.every((line) => line.width === 2)).toBe(true);
    // Bland-Altman: one mean line and two dashed limits.
    expect(painter.lines.filter((line) => line.dash.length === 2 && line.dash[0] === 5)).toHaveLength(2);
    expect(painter.circles.length).toBeGreaterThan(report.blandAltman.length);
  });

  it("stacks the two-up rows on a narrow canvas and keeps everything legible", () => {
    const report = readyComparisonFixture();
    const wide = new RecordingPainter();
    const narrow = new RecordingPainter();
    const wideHeight = paintPowerComparison(wide, report, { width: EXPORT_WIDTH });
    const narrowHeight = paintPowerComparison(narrow, report, { width: 700 });
    expect(narrowHeight).toBeGreaterThan(wideHeight + 400);
    expect(narrow.prose).toContain("Bland-Altman");
    expect(narrow.prose).toContain("Difference by cadence");
    // Text never grows or shrinks with the width.
    const sizes = (painter: RecordingPainter) => new Set(painter.texts.map((entry) => entry.style.size));
    expect(sizes(narrow)).toEqual(sizes(wide));
  });

  it("says when a suppressed axis has nothing to show", () => {
    const report = readyComparisonFixture({ byCadence: null, byPower: [], warmup: null, byMinute: [] });
    const painter = new RecordingPainter();
    paintPowerComparison(painter, report, { width: EXPORT_WIDTH });
    expect(painter.prose).toContain("Neither device reported cadence for at least half the compared time.");
    expect(painter.prose).toContain("No power level was ridden long enough to bin.");
    expect(painter.prose).toContain("No minute had enough compared seconds.");
    expect(painter.prose).not.toContain("first 10 min");
  });

  it("paints an insufficient report as a reason, the coverage and the devices", () => {
    const ready = readyComparisonFixture();
    const report: PowerComparison = {
      status: "insufficient",
      sessionId: ready.sessionId,
      reason: "Both devices reported at the same time for only 40 s; at least 60 s are needed.",
      minOverlapSeconds: 60,
      coverage: { ...ready.coverage, overlapSeconds: 40, overlapStartSeconds: 1_452, overlapEndSeconds: 1_492, comparedSeconds: 0, excludedSeconds: 0, coastingSeconds: 0, transientSeconds: 0 },
      a: ready.a,
      b: ready.b,
      signConvention: ready.signConvention,
    };
    const painter = new RecordingPainter();
    const height = paintPowerComparison(painter, report, { width: EXPORT_WIDTH });
    expect(height).toBeLessThan(600);
    expect(painter.prose).toContain("NOT ENOUGH OVERLAP");
    expect(painter.prose).toContain("only 40 s");
    expect(painter.prose).toContain("from 24:12 to 24:52 of a 25:00 ride");
    expect(painter.prose).toContain("Assioma DUO");
    expect(painter.prose).toContain(ready.signConvention);
    expect(painter.prose).not.toContain("Bland-Altman");
  });

  it("has axis helpers that cover the range on round steps", () => {
    expect(niceTicks(0, 412, 5)).toEqual([0, 100, 200, 300, 400, 500]);
    expect(niceTicks(-3.2, 3.2, 5)).toEqual([-4, -2, 0, 2, 4]);
    expect(timeTicks([0, 1500])).toEqual([0, 300, 600, 900, 1200, 1500]);
    expect(timeTicks([12, 100])).toEqual([15, 30, 45, 60, 75, 90]);
    const painter = new RecordingPainter();
    expect(wrap(painter, "one two three four", 10, 60)).toEqual(["one two", "three four"]);
  });
});
