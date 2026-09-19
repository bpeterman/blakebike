import { describe, expect, it } from "vitest";
import {
  formatDistance,
  formatDuration,
  formatSpeed,
  manualPowerDeltaForKey,
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
});
