import type { WorkoutStep } from "./types";

/**
 * Index path from `workout.steps` down through nested repeat groups to a
 * step. A flat workout has single-element paths; `[2, 0]` is the first child
 * of the third top-level step.
 */
export type StepPath = readonly number[];

export type LeafStep = Exclude<WorkoutStep, { kind: "repeat" }>;
export type RepeatStep = Extract<WorkoutStep, { kind: "repeat" }>;

/** One executed block of a workout after every repeat group is unrolled. */
export type ExpandedStep = {
  step: LeafStep;
  /** Path to the step definition; identical for every iteration of a repeat. */
  path: StepPath;
  /** For each repeat ancestor on `path`, the 0-based iteration being run. */
  iterations: readonly number[];
  startSeconds: number;
  endSeconds: number;
};

/**
 * The single place that turns a step tree into the sequence the runner
 * executes. Everything that needs intervals, durations, or a picture of a
 * workout derives from this.
 */
export const expandWorkoutSteps = (steps: readonly WorkoutStep[]): ExpandedStep[] => {
  const expanded: ExpandedStep[] = [];
  let cursor = 0;
  const visit = (
    list: readonly WorkoutStep[],
    prefix: StepPath,
    iterations: readonly number[],
  ) => {
    list.forEach((step, index) => {
      const path = [...prefix, index];
      if (step.kind === "repeat") {
        for (let iteration = 0; iteration < step.repetitions; iteration += 1) {
          visit(step.steps, path, [...iterations, iteration]);
        }
        return;
      }
      const startSeconds = cursor;
      cursor += step.durationSeconds;
      expanded.push({ step, path, iterations, startSeconds, endSeconds: cursor });
    });
  };
  visit(steps, [], []);
  return expanded;
};

export const stepAtPath = (
  steps: readonly WorkoutStep[],
  path: StepPath,
): WorkoutStep | undefined => {
  let list: readonly WorkoutStep[] = steps;
  let found: WorkoutStep | undefined;
  for (const index of path) {
    found = list[index];
    if (found === undefined) return undefined;
    if (found.kind === "repeat") {
      list = found.steps;
    } else {
      list = [];
    }
  }
  return path.length === 0 ? undefined : found;
};

/**
 * Returns a new step tree with the step at `path` replaced. Untouched
 * branches keep their identity so memoised consumers do not re-render.
 * Returns the input unchanged when the path does not resolve.
 */
export const replaceStepAtPath = (
  steps: readonly WorkoutStep[],
  path: StepPath,
  replacement: WorkoutStep,
): WorkoutStep[] => {
  if (path.length === 0) return [...steps];
  const [index, ...rest] = path;
  const current = steps[index];
  if (current === undefined) return [...steps];
  if (rest.length === 0) {
    return steps.map((step, stepIndex) => (stepIndex === index ? replacement : step));
  }
  if (current.kind !== "repeat") return [...steps];
  const children = replaceStepAtPath(current.steps, rest, replacement);
  return steps.map((step, stepIndex) =>
    stepIndex === index ? { ...current, steps: children } : step,
  );
};

export const pathsEqual = (a: StepPath, b: StepPath): boolean =>
  a.length === b.length && a.every((index, position) => index === b[position]);

/** True when `path` is `prefix` or lies inside the group `prefix` names. */
export const pathStartsWith = (path: StepPath, prefix: StepPath): boolean =>
  prefix.length <= path.length &&
  prefix.every((index, position) => index === path[position]);
