import { describe, expect, it } from "vitest";
import {
  formatDistance,
  formatDuration,
  formatSpeed,
  manualPowerDeltaForKey,
  rideKeyAction,
  clampBias,
  withSmoothedPower,
  workoutDuration,
  type WorkoutStep,
} from "./types";

describe("workout helpers", () => {
  it("totals nested repeats", () => {
    const steps: WorkoutStep[] = [
      {
        kind: "repeat",
        repetitions: 3,
        steps: [
          {
            kind: "steady",
            durationSeconds: 30,
            target: { unit: "percentFtp", value: 120 },
          },
          { kind: "freeRide", durationSeconds: 30 },
        ],
      },
    ];
    expect(workoutDuration(steps)).toBe(180);
  });

  it("formats short and long durations", () => {
    expect(formatDuration(65)).toBe("1:05");
    expect(formatDuration(3661)).toBe("1:01:01");
  });

  it("maps non-repeating arrow keys to manual power changes", () => {
    expect(manualPowerDeltaForKey("ArrowUp", false)).toBe(5);
    expect(manualPowerDeltaForKey("ArrowDown", false)).toBe(-5);
    expect(manualPowerDeltaForKey("ArrowUp", true)).toBeNull();
    expect(manualPowerDeltaForKey("Enter", false)).toBeNull();
  });

  it("formats metric-backed distance and speed in either display unit", () => {
    expect(formatDistance(10_000, "km")).toEqual({ value: "10.0", unit: "km" });
    expect(formatDistance(1609.344, "mi")).toEqual({ value: "1.00", unit: "mi" });
    expect(formatSpeed(32.18688, "mi")).toEqual({ value: "20.0", unit: "mph" });
    expect(formatSpeed(32.18688, "km")).toEqual({ value: "32.2", unit: "km/h" });
  });

  it("routes Shift+arrows to the bias and plain arrows to the target", () => {
    expect(rideKeyAction("ArrowUp", false, false)).toEqual({ kind: "power", delta: 5 });
    expect(rideKeyAction("ArrowDown", true, false)).toEqual({ kind: "bias", delta: -1 });
    expect(rideKeyAction("ArrowUp", true, true)).toBeNull();
    expect(rideKeyAction("a", true, false)).toBeNull();
  });

  it("keeps the bias inside 50–150 %", () => {
    expect(clampBias(100.4)).toBe(100);
    expect(clampBias(10)).toBe(50);
    expect(clampBias(999)).toBe(150);
  });

  it("averages power over a trailing time window", () => {
    const history = [0, 1, 2, 3, 4, 5].map((second) => ({
      timestampMs: second * 1000,
      powerWatts: second * 100,
    }));
    expect(withSmoothedPower(history, "instant").map((s) => s.displayPowerWatts)).toEqual([
      0, 100, 200, 300, 400, 500,
    ]);
    // 3 s window: at t=5 the samples at t=3,4,5 are included (t=2 is 3 s old and drops out).
    expect(withSmoothedPower(history, "3s").map((s) => s.displayPowerWatts)).toEqual([
      0, 50, 100, 200, 300, 400,
    ]);
    // 5 s window: at t=5 the samples at t=1..5 are included.
    expect(withSmoothedPower(history, "5s")[5].displayPowerWatts).toBe(300);
    // 10 s window covers everything here.
    expect(withSmoothedPower(history, "10s")[5].displayPowerWatts).toBe(250);
    expect(withSmoothedPower([], "10s")).toEqual([]);
  });
});
