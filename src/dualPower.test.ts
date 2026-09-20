import { describe, expect, it } from "vitest";
import {
  DeltaWindow,
  calibrationContext,
  describeLag,
  describeTrend,
  deviceDetail,
  formatClockSeconds,
  formatSigned,
  headlineWindow,
  liveDelta,
} from "./dualPower";
import { readyComparisonFixture } from "./dualPowerFixture";

describe("dual power helpers", () => {
  it("puts an explicit sign on every difference", () => {
    expect(formatSigned(7, 0, "W")).toBe("+7 W");
    expect(formatSigned(-2.84, 1, "%")).toBe("−2.8%");
    expect(formatSigned(0.04, 1, "W")).toBe("±0.0 W");
    expect(formatClockSeconds(65)).toBe("1:05");
    expect(formatClockSeconds(3_725)).toBe("1:02:05");
  });

  it("computes the live delta as trainer minus meter and withholds percent while coasting", () => {
    expect(liveDelta(undefined)).toBeNull();
    expect(liveDelta({ trainer: 248 })).toBeNull();
    const delta = liveDelta({ trainer: 248, power: 241 });
    expect(delta).toMatchObject({ trainerWatts: 248, meterWatts: 241, watts: 7 });
    expect(delta?.percent).toBeCloseTo(2.86, 1);
    expect(liveDelta({ trainer: 12, power: 0 })).toMatchObject({ watts: 12, percent: null });
  });

  it("keeps a rolling mean over the window and drops coasting samples", () => {
    const window = new DeltaWindow(30_000);
    expect(window.mean(0)).toBeNull();
    for (let second = 0; second < 40; second += 1) {
      window.push({ timestampMs: second * 1_000, powerBySource: { trainer: 210, power: 200 } });
    }
    // Duplicated timestamps and coasting samples are ignored.
    window.push({ timestampMs: 39_000, powerBySource: { trainer: 500, power: 100 } });
    window.push({ timestampMs: 39_500, powerBySource: { trainer: 0, power: 0 } });
    // 39.5 s back 30 s reaches 9.5 s: seconds 10 through 39 remain.
    const mean = window.mean(39_500);
    expect(mean?.count).toBe(30);
    expect(mean?.watts).toBe(10);
    expect(mean?.percent).toBeCloseTo(5, 5);
    // Everything ages out eventually.
    expect(window.mean(100_000)).toBeNull();
  });

  it("describes devices, lag, trend and the meter's zero relative to the ride", () => {
    const report = readyComparisonFixture();
    expect(deviceDetail(report.a.device)).toBe("Wahoo KICKR CORE · fw 1.2.3");
    expect(deviceDetail(report.b.device)).toBe("Favero · fw 5.14");
    expect(deviceDetail(null)).toBe("device not on record");
    expect(describeLag(report.lag)).toBe("Lag: 1.75 s (trainer trails the meter), corrected before comparing.");
    expect(describeLag({ ...report.lag, seconds: -0.5 })).toContain("meter trails the trainer");
    expect(describeLag({ ...report.lag, confident: false, seconds: 0, note: "Power barely varied." })).toBe(
      "Lag: not estimated (Power barely varied).",
    );
    expect(describeTrend(report.trend)).toBe("+2.1 W flat, plus 1.4% of power");
    expect(describeTrend({ interceptWatts: -3, slopePercent: 0 })).toBe("−3.0 W flat, no change with power");
    expect(calibrationContext(report.b.device?.lastCalibration, report.startedAt)).toBe(
      "Meter zeroed 1.5 h before the ride · offset 1019 (−4 vs the zero before).",
    );
    expect(calibrationContext({ at: "2026-09-19T11:50:00Z", kind: "zeroOffset", offsetRaw: 1019 }, report.startedAt)).toBe(
      "Meter zeroed 10 min before the ride · offset 1019.",
    );
    expect(calibrationContext({ at: "2026-09-10T11:50:00Z", kind: "spinDown", offsetRaw: null }, report.startedAt)).toBeNull();
    expect(headlineWindow(report).windowSeconds).toBe(3);
  });
});
