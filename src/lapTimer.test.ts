import { describe, expect, it } from "vitest";
import {
  defaultLapTimerSettings,
  drillPhase,
  lapStatus,
  normalizeLapTimerSettings,
  type LapTimerSettings,
} from "./lapTimer";
import type { Telemetry } from "./types";

/** One sample a second: 150 W / 80 rpm for the first minute, 250 W / 100 rpm after. */
const history: Telemetry[] = Array.from({ length: 121 }, (_, second) => ({
  timestampMs: second * 1_000,
  powerWatts: second < 60 ? 150 : 250,
  cadenceRpm: second < 60 ? 80 : 100,
  speedKph: 30,
  heartRateBpm: 140,
  targetPowerWatts: null,
}));

const drill = (workSeconds: number, restSeconds: number): LapTimerSettings => ({
  mode: "countdown",
  workSeconds,
  restSeconds,
});

describe("lap timer settings", () => {
  it("keeps whole seconds inside the bounds and a known mode", () => {
    expect(normalizeLapTimerSettings(defaultLapTimerSettings)).toEqual(defaultLapTimerSettings);
    expect(normalizeLapTimerSettings({ mode: "countdown", workSeconds: 12.6, restSeconds: 99_999 })).toEqual({
      mode: "countdown",
      workSeconds: 13,
      restSeconds: 3_600,
    });
    expect(
      normalizeLapTimerSettings({
        ...defaultLapTimerSettings,
        workSeconds: Number.NaN,
        restSeconds: -5,
      }),
    ).toEqual({ ...defaultLapTimerSettings, restSeconds: 0 });
  });
});

describe("lap status", () => {
  it("treats the whole ride as the first lap until a lap is marked", () => {
    const status = lapStatus([], 42, history.slice(0, 43));
    expect(status).toMatchObject({ number: 1, seconds: 42, lastSeconds: null, last: null });
    expect(status.current.powerWatts).toBe(150);
  });

  it("times laps on the ride clock and averages them over their own samples", () => {
    // Marks carry both clocks: 30 s of riding between them, 40 s of wall
    // time because the rider paused for ten seconds in that lap.
    const status = lapStatus(
      [
        { elapsedSeconds: 50, timestampMs: 50_000 },
        { elapsedSeconds: 80, timestampMs: 90_000 },
      ],
      100,
      history,
    );
    expect(status.number).toBe(3);
    expect(status.seconds).toBe(20);
    expect(status.lastSeconds).toBe(30);
    expect(status.current).toEqual({ powerWatts: 250, cadenceRpm: 100 });
    // The samples between the marks: 10 s at 150 W, then 29 s at 250 W.
    expect(status.last?.powerWatts).toBeCloseTo(8_750 / 39);
    expect(status.last?.cadenceRpm).toBeCloseTo(3_700 / 39);
  });

  it("says nothing about averages a lap has no samples for yet", () => {
    const status = lapStatus([{ elapsedSeconds: 120, timestampMs: 120_500 }], 120, history);
    expect(status.seconds).toBe(0);
    expect(status.current).toEqual({ powerWatts: null, cadenceRpm: null });
  });
});

describe("countdown drill", () => {
  it("alternates work and rest and counts each part down", () => {
    expect(drillPhase(drill(30, 15), 0)).toEqual({ kind: "work", repeat: 1, remainingSeconds: 30, index: 0 });
    expect(drillPhase(drill(30, 15), 29)).toEqual({ kind: "work", repeat: 1, remainingSeconds: 1, index: 0 });
    expect(drillPhase(drill(30, 15), 30)).toEqual({ kind: "rest", repeat: 1, remainingSeconds: 15, index: 1 });
    expect(drillPhase(drill(30, 15), 45)).toEqual({ kind: "work", repeat: 2, remainingSeconds: 30, index: 2 });
  });

  it("repeats the work part back to back without a rest", () => {
    expect(drillPhase(drill(20, 0), 19)).toEqual({ kind: "work", repeat: 1, remainingSeconds: 1, index: 0 });
    expect(drillPhase(drill(20, 0), 20)).toEqual({ kind: "work", repeat: 2, remainingSeconds: 20, index: 1 });
  });
});
