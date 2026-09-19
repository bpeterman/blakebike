import { describe, expect, it } from "vitest";
import type { WorkoutStep } from "./types";
import { compileWorkoutIntervals, workoutDuration } from "./types";
import {
  expandWorkoutSteps,
  pathStartsWith,
  pathsEqual,
  replaceStepAtPath,
  stepAtPath,
} from "./workoutSteps";

const steady = (durationSeconds: number, value: number): WorkoutStep => ({
  kind: "steady",
  durationSeconds,
  target: { unit: "percentFtp", value },
});

const nested: WorkoutStep[] = [
  steady(60, 50),
  {
    kind: "repeat",
    repetitions: 2,
    steps: [
      steady(30, 120),
      {
        kind: "repeat",
        repetitions: 2,
        steps: [{ kind: "freeRide", durationSeconds: 5 }],
      },
    ],
  },
  { kind: "ramp", durationSeconds: 20, start: { unit: "watts", value: 100 }, end: { unit: "watts", value: 200 } },
];

describe("expandWorkoutSteps", () => {
  it("unrolls nested repeats in execution order with paths and iterations", () => {
    const expanded = expandWorkoutSteps(nested);
    expect(expanded.map((entry) => [entry.step.kind, [...entry.path], [...entry.iterations]])).toEqual([
      ["steady", [0], []],
      ["steady", [1, 0], [0]],
      ["freeRide", [1, 1, 0], [0, 0]],
      ["freeRide", [1, 1, 0], [0, 1]],
      ["steady", [1, 0], [1]],
      ["freeRide", [1, 1, 0], [1, 0]],
      ["freeRide", [1, 1, 0], [1, 1]],
      ["ramp", [2], []],
    ]);
  });

  it("accumulates start and end seconds without gaps", () => {
    const expanded = expandWorkoutSteps(nested);
    expect(expanded[0]).toMatchObject({ startSeconds: 0, endSeconds: 60 });
    expect(expanded[1]).toMatchObject({ startSeconds: 60, endSeconds: 90 });
    expect(expanded[2]).toMatchObject({ startSeconds: 90, endSeconds: 95 });
    for (let index = 1; index < expanded.length; index += 1) {
      expect(expanded[index].startSeconds).toBe(expanded[index - 1].endSeconds);
    }
    expect(expanded[expanded.length - 1].endSeconds).toBe(workoutDuration(nested));
  });

  it("yields nothing for empty workouts or zero-repetition groups", () => {
    expect(expandWorkoutSteps([])).toEqual([]);
    expect(expandWorkoutSteps([{ kind: "repeat", repetitions: 0, steps: [steady(10, 50)] }])).toEqual([]);
  });

  it("is the source of truth for the runner-aligned compiler", () => {
    const ftp = 201;
    const intervals = compileWorkoutIntervals(nested, ftp);
    const expanded = expandWorkoutSteps(nested);
    expect(intervals).toHaveLength(expanded.length);
    intervals.forEach((interval, index) => {
      expect(interval.kind).toBe(expanded[index].step.kind);
      expect(interval.durationSeconds).toBe(expanded[index].step.durationSeconds);
    });
  });
});

describe("step paths", () => {
  it("resolves steps by path", () => {
    expect(stepAtPath(nested, [0])).toBe(nested[0]);
    expect(stepAtPath(nested, [1, 1, 0])).toEqual({ kind: "freeRide", durationSeconds: 5 });
    expect(stepAtPath(nested, [])).toBeUndefined();
    expect(stepAtPath(nested, [5])).toBeUndefined();
    expect(stepAtPath(nested, [0, 0])).toBeUndefined();
  });

  it("replaces a nested step immutably and preserves untouched branches", () => {
    const replacement: WorkoutStep = { kind: "freeRide", durationSeconds: 99 };
    const next = replaceStepAtPath(nested, [1, 1, 0], replacement);
    expect(next).not.toBe(nested);
    expect(stepAtPath(next, [1, 1, 0])).toBe(replacement);
    expect(stepAtPath(nested, [1, 1, 0])).toEqual({ kind: "freeRide", durationSeconds: 5 });
    expect(next[0]).toBe(nested[0]);
    expect(next[2]).toBe(nested[2]);
  });

  it("replaces a top-level step", () => {
    const replacement = steady(1, 1);
    const next = replaceStepAtPath(nested, [2], replacement);
    expect(next[2]).toBe(replacement);
    expect(next[1]).toBe(nested[1]);
  });

  it("leaves the tree alone for paths that do not resolve", () => {
    expect(replaceStepAtPath(nested, [9], steady(1, 1))).toEqual(nested);
    expect(replaceStepAtPath(nested, [0, 0], steady(1, 1))).toEqual(nested);
    expect(replaceStepAtPath(nested, [], steady(1, 1))).toEqual(nested);
  });

  it("compares and prefix-matches paths", () => {
    expect(pathsEqual([1, 2], [1, 2])).toBe(true);
    expect(pathsEqual([1, 2], [1])).toBe(false);
    expect(pathsEqual([], [])).toBe(true);
    expect(pathStartsWith([1, 2, 0], [1])).toBe(true);
    expect(pathStartsWith([1, 2, 0], [1, 2, 0])).toBe(true);
    expect(pathStartsWith([1], [1, 2])).toBe(false);
    expect(pathStartsWith([2, 0], [1])).toBe(false);
  });
});
