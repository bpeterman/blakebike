import { describe, expect, it } from "vitest";
import {
  formatDuration,
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
});
