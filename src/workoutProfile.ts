import {
  compileWorkoutInterval,
  formatDuration,
  type WorkoutInterval,
  type WorkoutStep,
} from "./types";
import {
  expandWorkoutSteps,
  pathStartsWith,
  stepAtPath,
  type ExpandedStep,
  type LeafStep,
  type StepPath,
} from "./workoutSteps";

/**
 * Pure model behind every drawing of a workout: the executed blocks with their
 * compiled watt targets, summary statistics, a pixel scale, and the polygons
 * to draw. Rendering code maps these to SVG and does no math of its own.
 */

export type ProfileSegment = ExpandedStep & {
  /** Exactly what the runner will execute for this block. */
  interval: WorkoutInterval;
};

export const profileSegments = (
  steps: readonly WorkoutStep[],
  ftpWatts: number,
): ProfileSegment[] =>
  expandWorkoutSteps(steps).map((expanded) => ({
    ...expanded,
    interval: compileWorkoutInterval(expanded.step, ftpWatts),
  }));

export type ProfileStats = {
  totalSeconds: number;
  /** Time with no power target. Reported separately, never folded into averages. */
  freeRideSeconds: number;
  blockCount: number;
  /** Time-weighted mean target over targeted time, or null when nothing is targeted. */
  averagePercentFtp: number | null;
  /** Σ (target / FTP)² × hours × 100 over targeted time, or null when nothing is targeted. */
  estimatedStress: number | null;
  peakWatts: number;
  peakPercentFtp: number | null;
};

/** Mean of a linear ramp from `a` to `b`. */
const rampMean = (a: number, b: number) => (a + b) / 2;
/** Mean of the square of a linear ramp from `a` to `b`. */
const rampMeanSquare = (a: number, b: number) => (a * a + a * b + b * b) / 3;

export const profileStats = (
  segments: readonly ProfileSegment[],
  ftpWatts: number,
): ProfileStats => {
  let totalSeconds = 0;
  let freeRideSeconds = 0;
  let targetedSeconds = 0;
  let wattSeconds = 0;
  let squaredWattSeconds = 0;
  let peakWatts = 0;
  for (const { interval } of segments) {
    totalSeconds += interval.durationSeconds;
    if (interval.startWatts === null || interval.endWatts === null) {
      freeRideSeconds += interval.durationSeconds;
      continue;
    }
    targetedSeconds += interval.durationSeconds;
    wattSeconds += rampMean(interval.startWatts, interval.endWatts) * interval.durationSeconds;
    squaredWattSeconds +=
      rampMeanSquare(interval.startWatts, interval.endWatts) * interval.durationSeconds;
    peakWatts = Math.max(peakWatts, interval.startWatts, interval.endWatts);
  }
  const targeted = targetedSeconds > 0 && ftpWatts > 0;
  return {
    totalSeconds,
    freeRideSeconds,
    blockCount: segments.length,
    averagePercentFtp: targeted ? (wattSeconds / targetedSeconds / ftpWatts) * 100 : null,
    estimatedStress: targeted
      ? (squaredWattSeconds / (ftpWatts * ftpWatts) / 3600) * 100
      : null,
    peakWatts,
    peakPercentFtp: targeted ? (peakWatts / ftpWatts) * 100 : null,
  };
};

export type ProfileScale = {
  width: number;
  height: number;
  totalSeconds: number;
  ftpWatts: number;
  /** Top of the y axis. Never below `ftpWatts × headroom`, so easy workouts read as easy. */
  maxWatts: number;
  /** Placeholder height for blocks with no target. */
  freeRideWatts: number;
  x: (seconds: number) => number;
  y: (watts: number) => number;
};

export const profileScale = (
  segments: readonly ProfileSegment[],
  options: { width: number; height: number; ftpWatts: number; headroom?: number },
): ProfileScale => {
  const { width, height, ftpWatts } = options;
  const headroom = options.headroom ?? 1.2;
  const stats = profileStats(segments, ftpWatts);
  const totalSeconds = stats.totalSeconds;
  const maxWatts = Math.max(1, stats.peakWatts, ftpWatts * headroom);
  return {
    width,
    height,
    totalSeconds,
    ftpWatts,
    maxWatts,
    freeRideWatts: ftpWatts * 0.5,
    x: (seconds) => (totalSeconds === 0 ? 0 : (seconds / totalSeconds) * width),
    y: (watts) => height - (Math.min(Math.max(watts, 0), maxWatts) / maxWatts) * height,
  };
};

export type ProfilePoint = readonly [number, number];

export type ProfileShape = {
  path: StepPath;
  iterations: readonly number[];
  kind: LeafStep["kind"];
  startSeconds: number;
  endSeconds: number;
  startWatts: number | null;
  endWatts: number | null;
  /** Left edge, right edge (after any gap), in pixels. */
  x: number;
  width: number;
  /** Clockwise from the top-left: start target, end target, baseline right, baseline left. */
  points: readonly [ProfilePoint, ProfilePoint, ProfilePoint, ProfilePoint];
};

/**
 * One polygon per executed block. A gap is left between neighbours only when
 * the block is wide enough to afford it; narrow blocks touch so a long set of
 * short intervals stays proportional.
 */
export const profileShapes = (
  segments: readonly ProfileSegment[],
  scale: ProfileScale,
  gapPx = 1,
): ProfileShape[] =>
  segments.map((segment, index) => {
    const left = scale.x(segment.startSeconds);
    const rightEdge = scale.x(segment.endSeconds);
    const isLast = index === segments.length - 1;
    const gap = !isLast && rightEdge - left > gapPx * 3 ? gapPx : 0;
    const right = rightEdge - gap;
    const { startWatts, endWatts } = segment.interval;
    const startY = scale.y(startWatts ?? scale.freeRideWatts);
    const endY = scale.y(endWatts ?? scale.freeRideWatts);
    return {
      path: segment.path,
      iterations: segment.iterations,
      kind: segment.step.kind,
      startSeconds: segment.startSeconds,
      endSeconds: segment.endSeconds,
      startWatts,
      endWatts,
      x: left,
      width: right - left,
      points: [
        [left, startY],
        [right, endY],
        [right, scale.height],
        [left, scale.height],
      ],
    };
  });

const tickSpacingsSeconds = [60, 300, 600, 900, 1800, 3600] as const;

/** Interior tick positions in seconds, spaced so labels never crowd. */
export const timeAxisTicks = (
  totalSeconds: number,
  width: number,
  minLabelPx = 56,
): number[] => {
  if (totalSeconds <= 0 || width <= 0) return [];
  const spacing =
    tickSpacingsSeconds.find((candidate) => (candidate / totalSeconds) * width >= minLabelPx) ??
    tickSpacingsSeconds[tickSpacingsSeconds.length - 1];
  const ticks: number[] = [];
  for (let seconds = spacing; seconds < totalSeconds; seconds += spacing) ticks.push(seconds);
  return ticks;
};

export type RepeatSpan = {
  path: StepPath;
  /** Nesting depth; top-level groups are 0. */
  depth: number;
  repetitions: number;
  startSeconds: number;
  endSeconds: number;
};

/** Time spans covered by each repeat group that produced at least one block. */
export const repeatSpans = (
  steps: readonly WorkoutStep[],
  segments: readonly ProfileSegment[],
): RepeatSpan[] => {
  const spans = new Map<string, RepeatSpan>();
  for (const segment of segments) {
    for (let length = 1; length < segment.path.length; length += 1) {
      const groupPath = segment.path.slice(0, length);
      const key = groupPath.join(".");
      const existing = spans.get(key);
      if (existing) {
        existing.startSeconds = Math.min(existing.startSeconds, segment.startSeconds);
        existing.endSeconds = Math.max(existing.endSeconds, segment.endSeconds);
        continue;
      }
      const group = stepAtPath(steps, groupPath);
      if (group?.kind !== "repeat") continue;
      spans.set(key, {
        path: groupPath,
        depth: length - 1,
        repetitions: group.repetitions,
        startSeconds: segment.startSeconds,
        endSeconds: segment.endSeconds,
      });
    }
  }
  return [...spans.values()].sort(
    (a, b) => a.startSeconds - b.startSeconds || a.depth - b.depth,
  );
};

/** Shapes whose step lies at or inside `path`; used for highlighting. */
export const shapesWithin = <T extends { path: StepPath }>(
  shapes: readonly T[],
  path: StepPath,
): T[] => shapes.filter((shape) => pathStartsWith(shape.path, path));

/** Plain-language summary for assistive technology and tooltips. */
export const profileSummary = (stats: ProfileStats): string => {
  if (stats.blockCount === 0) return "Empty workout";
  const parts = [
    `${formatDuration(stats.totalSeconds)} workout`,
    `${stats.blockCount} ${stats.blockCount === 1 ? "block" : "blocks"}`,
  ];
  if (stats.peakPercentFtp !== null) parts.push(`peak ${Math.round(stats.peakPercentFtp)}% FTP`);
  if (stats.freeRideSeconds > 0) parts.push(`${formatDuration(stats.freeRideSeconds)} free ride`);
  return parts.join(", ");
};
