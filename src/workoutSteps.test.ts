import { describe, expect, it } from "vitest";
import type { WorkoutStep } from "./types";
import { compileWorkoutIntervals, workoutDuration } from "./types";
import {
  appendNode,
  canMoveNode,
  expandWorkoutSteps,
  formatStepPath,
  fromStepNodes,
  groupNodes,
  moveNode,
  nodeAtPath,
  nodePath,
  pathStartsWith,
  pathsEqual,
  removeNode,
  replaceStepAtPath,
  stepAtPath,
  stepsProblem,
  toStepNodes,
  ungroupNode,
  updateNode,
  type LeafStep,
  type StepNode,
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

describe("step nodes", () => {
  const ids = () => {
    let next = 0;
    return () => `n${(next += 1)}`;
  };
  // n1 steady, n2 repeat [n3 steady, n4 repeat [n5 free]], n6 ramp
  const tree = () => toStepNodes(nested, ids());
  const shape = (nodes: readonly StepNode[]): unknown[] =>
    nodes.map((node) => (node.kind === "repeat" ? { [node.id]: shape(node.children) } : node.id));

  it("keys every step and round-trips back to the same steps", () => {
    const nodes = tree();
    expect(shape(nodes)).toEqual(["n1", { n2: ["n3", { n4: ["n5"] }] }, "n6"]);
    expect(fromStepNodes(nodes)).toEqual(nested);
  });

  it("maps ids to paths and back at any depth", () => {
    const nodes = tree();
    expect(nodePath(nodes, "n5")).toEqual([1, 1, 0]);
    expect(nodePath(nodes, "n6")).toEqual([2]);
    expect(nodePath(nodes, "missing")).toBeNull();
    expect(nodeAtPath(nodes, [1, 1, 0])?.id).toBe("n5");
    expect(nodeAtPath(nodes, [1])?.id).toBe("n2");
    expect(nodeAtPath(nodes, [0, 0])).toBeUndefined();
    expect(nodeAtPath(nodes, [])).toBeUndefined();
  });

  it("updates and removes nested nodes while sharing untouched branches", () => {
    const nodes = tree();
    const replacement: LeafStep = { kind: "freeRide", durationSeconds: 45 };
    const updated = updateNode(nodes, "n3", (node) =>
      node.kind === "leaf" ? { ...node, step: replacement } : node,
    );
    expect(stepAtPath(fromStepNodes(updated), [1, 0])).toBe(replacement);
    expect(updated[0]).toBe(nodes[0]);
    expect(updated[2]).toBe(nodes[2]);
    expect(shape(removeNode(nodes, "n4"))).toEqual(["n1", { n2: ["n3"] }, "n6"]);
    expect(removeNode(nodes, "missing")).toBe(nodes);
  });

  it("appends at the top level or inside a repeat", () => {
    const nodes = tree();
    const leaf: StepNode = { id: "x", kind: "leaf", step: { kind: "freeRide", durationSeconds: 10 } };
    expect(shape(appendNode(nodes, null, leaf))).toEqual(["n1", { n2: ["n3", { n4: ["n5"] }] }, "n6", "x"]);
    expect(shape(appendNode(nodes, "n4", leaf))).toEqual(["n1", { n2: ["n3", { n4: ["n5", "x"] }] }, "n6"]);
    expect(appendNode(nodes, "missing", leaf)).toBe(nodes);
  });

  it("moves steps into, through, and out of repeats one slot at a time", () => {
    let nodes = tree();
    // A step next to a repeat steps into its near end.
    nodes = moveNode(nodes, "n6", -1);
    expect(shape(nodes)).toEqual(["n1", { n2: ["n3", { n4: ["n5"] }, "n6"] }]);
    // Repeats move as a unit and swap with a neighbouring step.
    nodes = moveNode(nodes, "n4", 1);
    expect(shape(nodes)).toEqual(["n1", { n2: ["n3", "n6", { n4: ["n5"] }] }]);
    // At the edge of a repeat a step steps out beside it.
    nodes = moveNode(nodes, "n3", -1);
    expect(shape(nodes)).toEqual(["n1", "n3", { n2: ["n6", { n4: ["n5"] }] }]);
    nodes = moveNode(nodes, "n5", 1);
    expect(shape(nodes)).toEqual(["n1", "n3", { n2: ["n6", { n4: [] }, "n5"] }]);
    expect(moveNode(nodes, "n1", -1)).toBe(nodes);
    expect(canMoveNode(nodes, "n1", -1)).toBe(false);
    expect(canMoveNode(nodes, "n2", 1)).toBe(false);
    expect(canMoveNode(nodes, "n5", 1)).toBe(true);
    expect(canMoveNode(nodes, "missing", 1)).toBe(false);
  });

  it("groups contiguous siblings in place and refuses anything else", () => {
    const nodes = tree();
    const group = { id: "g", repetitions: 3 };
    const grouped = groupNodes(nodes, ["n6", "n2"], group);
    expect(grouped && shape(grouped)).toEqual(["n1", { g: [{ n2: ["n3", { n4: ["n5"] }] }, "n6"] }]);
    expect(grouped?.[1]).toMatchObject({ kind: "repeat", repetitions: 3 });
    const inner = groupNodes(nodes, ["n3"], group);
    expect(inner && shape(inner)).toEqual(["n1", { n2: [{ g: ["n3"] }, { n4: ["n5"] }] }, "n6"]);
    expect(groupNodes(nodes, ["n1", "n6"], group)).toBeNull();
    expect(groupNodes(nodes, ["n1", "n3"], group)).toBeNull();
    expect(groupNodes(nodes, [], group)).toBeNull();
    expect(groupNodes(nodes, ["missing"], group)).toBeNull();
  });

  it("ungroups a repeat into its children in place", () => {
    const nodes = tree();
    expect(shape(ungroupNode(nodes, "n2"))).toEqual(["n1", "n3", { n4: ["n5"] }, "n6"]);
    expect(ungroupNode(nodes, "n1")).toBe(nodes);
  });
});

describe("stepsProblem", () => {
  const once = (repetitions: number, steps: WorkoutStep[]): WorkoutStep => ({ kind: "repeat", repetitions, steps });

  it("accepts valid trees and mirrors the backend's repeat rules", () => {
    expect(stepsProblem(nested)).toBeNull();
    expect(stepsProblem([once(1, [steady(10, 50)])])).toBe("Repeat 1 must run between 2 and 20 times");
    expect(stepsProblem([once(21, [steady(10, 50)])])).toMatch(/between 2 and 20/);
    expect(stepsProblem([once(2.5, [steady(10, 50)])])).toMatch(/between 2 and 20/);
    expect(stepsProblem([steady(10, 50), once(2, [])])).toBe("Repeat 2 needs at least one step");
    expect(stepsProblem([once(2, [steady(0, 50)])])).toBe("Block 1.1 needs a duration of at least 1 whole second");
  });

  it("formats paths for people", () => {
    expect(formatStepPath([1, 0])).toBe("2.1");
    expect(formatStepPath([0])).toBe("1");
  });
});
