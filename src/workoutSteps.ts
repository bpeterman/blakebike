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

/** 1-based, dotted position of a step for people: `[1, 0]` is block "2.1". */
export const formatStepPath = (path: StepPath): string => path.map((index) => index + 1).join(".");

/** Repeat counts the backend accepts; mirrors `WorkoutStep::validate` in domain.rs. */
export const MIN_REPETITIONS = 2;
export const MAX_REPETITIONS = 20;

/**
 * The first reason the backend would refuse these steps, phrased for the
 * rider, or null when they are valid. Mirrors `WorkoutStep::validate` so the
 * editor can explain a problem before it tries to save.
 */
export const stepsProblem = (steps: readonly WorkoutStep[]): string | null => {
  const visit = (list: readonly WorkoutStep[], prefix: StepPath): string | null => {
    for (const [index, step] of list.entries()) {
      const path = [...prefix, index];
      const label = formatStepPath(path);
      if (step.kind !== "repeat") {
        if (!Number.isInteger(step.durationSeconds) || step.durationSeconds < 1) {
          return `Block ${label} needs a duration of at least 1 whole second`;
        }
        continue;
      }
      if (
        !Number.isInteger(step.repetitions) ||
        step.repetitions < MIN_REPETITIONS ||
        step.repetitions > MAX_REPETITIONS
      ) {
        return `Repeat ${label} must run between ${MIN_REPETITIONS} and ${MAX_REPETITIONS} times`;
      }
      if (step.steps.length === 0) return `Repeat ${label} needs at least one step`;
      const nested = visit(step.steps, path);
      if (nested !== null) return nested;
    }
    return null;
  };
  return visit(steps, []);
};

/**
 * A step tree where every step, leaf or repeat, carries an id that survives
 * edits, reordering, grouping, and nesting. Paths are positions and change
 * whenever the tree does; ids are what stateful UI keys on. Convert at the
 * edges with `toStepNodes` / `fromStepNodes`.
 */
export type StepNode =
  | { id: string; kind: "leaf"; step: LeafStep }
  | { id: string; kind: "repeat"; repetitions: number; children: StepNode[] };

export const toStepNodes = (steps: readonly WorkoutStep[], newId: () => string): StepNode[] =>
  steps.map((step) =>
    step.kind === "repeat"
      ? { id: newId(), kind: "repeat", repetitions: step.repetitions, children: toStepNodes(step.steps, newId) }
      : { id: newId(), kind: "leaf", step },
  );

export const fromStepNodes = (nodes: readonly StepNode[]): WorkoutStep[] =>
  nodes.map((node) =>
    node.kind === "repeat"
      ? { kind: "repeat", repetitions: node.repetitions, steps: fromStepNodes(node.children) }
      : node.step,
  );

/** Where a node sits: the repeat holding it (null at the top level) and its index there. */
type NodeLocation = { parentId: string | null; index: number; siblings: readonly StepNode[] };

const locateNode = (
  nodes: readonly StepNode[],
  id: string,
  parentId: string | null = null,
): NodeLocation | null => {
  for (const [index, node] of nodes.entries()) {
    if (node.id === id) return { parentId, index, siblings: nodes };
    if (node.kind === "repeat") {
      const found = locateNode(node.children, id, node.id);
      if (found !== null) return found;
    }
  }
  return null;
};

export const nodePath = (nodes: readonly StepNode[], id: string): StepPath | null => {
  for (const [index, node] of nodes.entries()) {
    if (node.id === id) return [index];
    if (node.kind === "repeat") {
      const nested = nodePath(node.children, id);
      if (nested !== null) return [index, ...nested];
    }
  }
  return null;
};

export const nodeAtPath = (nodes: readonly StepNode[], path: StepPath): StepNode | undefined => {
  let list = nodes;
  let found: StepNode | undefined;
  for (const index of path) {
    found = list[index];
    if (found === undefined) return undefined;
    list = found.kind === "repeat" ? found.children : [];
  }
  return found;
};

/**
 * Rewrites the child list of `parentId` (the top level when null). Branches
 * that do not contain the parent keep their identity.
 */
const editChildren = (
  nodes: readonly StepNode[],
  parentId: string | null,
  edit: (children: readonly StepNode[]) => StepNode[],
): StepNode[] => {
  if (parentId === null) return edit(nodes);
  return nodes.map((node) => {
    if (node.kind !== "repeat") return node;
    if (node.id === parentId) return { ...node, children: edit(node.children) };
    const children = editChildren(node.children, parentId, edit);
    return children.every((child, index) => child === node.children[index]) ? node : { ...node, children };
  });
};

/** Returns the tree unchanged when `id` is not in it. */
export const updateNode = (
  nodes: StepNode[],
  id: string,
  update: (node: StepNode) => StepNode,
): StepNode[] => {
  const at = locateNode(nodes, id);
  if (at === null) return nodes;
  return editChildren(nodes, at.parentId, (list) => list.map((node) => (node.id === id ? update(node) : node)));
};

export const removeNode = (nodes: StepNode[], id: string): StepNode[] => {
  const at = locateNode(nodes, id);
  if (at === null) return nodes;
  return editChildren(nodes, at.parentId, (list) => list.filter((node) => node.id !== id));
};

/** Adds `node` at the end of the repeat `parentId`, or of the workout when null. */
export const appendNode = (nodes: StepNode[], parentId: string | null, node: StepNode): StepNode[] => {
  if (parentId !== null && locateNode(nodes, parentId) === null) return nodes;
  return editChildren(nodes, parentId, (list) => [...list, node]);
};

export type MoveDirection = -1 | 1;

/**
 * Moves a node one slot through the outline. It swaps with its neighbour,
 * except that a step next to a repeat steps into it at the near end, and a
 * node at the edge of a repeat steps out to sit beside it. Repeats move as a
 * unit and never enter one another, so moving cannot nest groups by accident.
 * Returns the tree unchanged when the node is already first or last overall.
 */
export const moveNode = (nodes: StepNode[], id: string, direction: MoveDirection): StepNode[] => {
  const at = locateNode(nodes, id);
  if (at === null) return nodes;
  const node = at.siblings[at.index];
  const neighbour = at.siblings[at.index + direction];
  const withoutNode = () => editChildren(nodes, at.parentId, (list) => list.filter((entry) => entry.id !== id));
  if (neighbour !== undefined) {
    if (node.kind === "leaf" && neighbour.kind === "repeat") {
      return editChildren(withoutNode(), neighbour.id, (children) =>
        direction === 1 ? [node, ...children] : [...children, node],
      );
    }
    return editChildren(nodes, at.parentId, (list) =>
      list.map((entry, index) =>
        index === at.index ? neighbour : index === at.index + direction ? node : entry,
      ),
    );
  }
  if (at.parentId === null) return nodes;
  const remaining = withoutNode();
  const parent = locateNode(remaining, at.parentId);
  if (parent === null) return nodes;
  const landing = parent.index + (direction === 1 ? 1 : 0);
  return editChildren(remaining, parent.parentId, (list) => [
    ...list.slice(0, landing),
    node,
    ...list.slice(landing),
  ]);
};

/** False exactly when `moveNode` would leave the tree unchanged. */
export const canMoveNode = (nodes: readonly StepNode[], id: string, direction: MoveDirection): boolean => {
  const at = locateNode(nodes, id);
  if (at === null) return false;
  return at.parentId !== null || at.siblings[at.index + direction] !== undefined;
};

/**
 * Wraps the nodes `ids` in a new repeat, in place. They must be contiguous
 * siblings (given in any order); otherwise there is no single place for the
 * repeat to go and the result is null.
 */
export const groupNodes = (
  nodes: StepNode[],
  ids: readonly string[],
  group: { id: string; repetitions: number },
): StepNode[] | null => {
  const locations = [...new Set(ids)].map((id) => locateNode(nodes, id));
  const [first] = locations;
  if (first === undefined || first === null) return null;
  const indices: number[] = [];
  for (const location of locations) {
    if (location === null || location.parentId !== first.parentId) return null;
    indices.push(location.index);
  }
  indices.sort((a, b) => a - b);
  const start = indices[0];
  if (indices.some((index, position) => index !== start + position)) return null;
  const end = start + indices.length;
  return editChildren(nodes, first.parentId, (list) => [
    ...list.slice(0, start),
    { id: group.id, kind: "repeat", repetitions: group.repetitions, children: list.slice(start, end) },
    ...list.slice(end),
  ]);
};

/** Replaces a repeat with its children, run once. Leaves any other node alone. */
export const ungroupNode = (nodes: StepNode[], id: string): StepNode[] => {
  const at = locateNode(nodes, id);
  if (at === null) return nodes;
  const node = at.siblings[at.index];
  if (node.kind !== "repeat") return nodes;
  return editChildren(nodes, at.parentId, (list) => [
    ...list.slice(0, at.index),
    ...node.children,
    ...list.slice(at.index + 1),
  ]);
};
