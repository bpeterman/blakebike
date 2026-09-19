import {
  compileWorkoutInterval,
  DEFAULT_BIAS_PERCENT,
  formatDuration,
  formatIntervalTarget,
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
  /** Exactly what the runner will execute for this block, bias included. */
  interval: WorkoutInterval;
};

export const profileSegments = (
  steps: readonly WorkoutStep[],
  ftpWatts: number,
  biasPercent = DEFAULT_BIAS_PERCENT,
): ProfileSegment[] =>
  expandWorkoutSteps(steps).map((expanded) => ({
    ...expanded,
    interval: compileWorkoutInterval(expanded.step, ftpWatts, biasPercent),
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

export type ProfileLabel = {
  /** Horizontal centre of the block, in pixels. */
  x: number;
  /**
   * `inside`/`above`: baseline of the target line; the duration sits one
   * `lineHeight` below. `vertical`: the anchor at the block's baseline that
   * the single line reads upward from.
   */
  y: number;
  lineHeight: number;
  /**
   * Two horizontal lines inside the block just above the baseline, or
   * floating above a block too short to hold them; or one line stood on end
   * in a block too narrow for horizontal text.
   */
  placement: "inside" | "above" | "vertical";
  target: string;
  /** Null when only the target fits. */
  duration: string | null;
};

/** The two type styles a label is set in; the duration and separator share the lighter one. */
export type LabelRole = "target" | "duration";

/** Rendered width in pixels of `text` set in the style for `role`. */
export type LabelMeasure = (text: string, role: LabelRole) => number;

/** Width of a glyph relative to the font size, generous for a sans face with tabular digits. */
const LABEL_GLYPH_RATIO = 0.62;
/** Clearance between a label and the block's baseline or top. */
const LABEL_PADDING_PX = 4;
/** Clearance between a horizontal label and each side of its column; neighbours end up twice this apart. */
const LABEL_GUTTER_PX = 2;
/** Room above the first line's baseline for its cap height. */
const LABEL_CAP_RATIO = 0.75;
/** Separator between target and duration when they share one line. */
export const LABEL_SEPARATOR = " · ";

/**
 * Fallback measurer for environments without text metrics. It overestimates
 * on purpose so a label is never drawn where it might not fit.
 */
export const estimateLabelWidth =
  (fontSize: number): LabelMeasure =>
  (text) =>
    text.length * fontSize * LABEL_GLYPH_RATIO;

/**
 * Where to write a block's target and duration, or null when the block has
 * room for neither. Wide blocks get two horizontal lines; narrow but tall
 * blocks get a single vertical line, dropping the duration if even that is
 * too long. Labels never spill outside their own column, so neighbours
 * cannot collide.
 */
export const profileLabel = (
  shape: ProfileShape,
  scale: ProfileScale,
  fontSize: number,
  measure: LabelMeasure = estimateLabelWidth(fontSize),
): ProfileLabel | null => {
  const target = formatIntervalTarget(shape.startWatts, shape.endWatts);
  const duration = formatDuration(shape.endSeconds - shape.startSeconds);
  const lineHeight = fontSize * 1.25;
  const [[, startY], [, endY]] = shape.points;
  const lowerTop = Math.max(startY, endY);
  const upperTop = Math.min(startY, endY);
  const label = { x: shape.x + shape.width / 2, lineHeight, target };
  const targetWidth = measure(target, "target");

  const widthNeeded = Math.max(targetWidth, measure(duration, "duration")) + LABEL_GUTTER_PX * 2;
  if (shape.width >= widthNeeded) {
    const heightNeeded = lineHeight * 2 + fontSize * LABEL_CAP_RATIO + LABEL_PADDING_PX * 2;
    if (scale.height - lowerTop >= heightNeeded) {
      return { ...label, duration, placement: "inside", y: scale.height - LABEL_PADDING_PX - lineHeight };
    }
    if (upperTop >= heightNeeded) {
      return { ...label, duration, placement: "above", y: upperTop - LABEL_PADDING_PX - lineHeight };
    }
  }

  // Stood on end, the line needs a glyph's height of width and reads upward from the baseline.
  if (shape.width >= fontSize + 2) {
    const room = scale.height - lowerTop - LABEL_PADDING_PX * 2;
    const vertical = { ...label, placement: "vertical" as const, y: scale.height - LABEL_PADDING_PX };
    if (targetWidth + measure(`${LABEL_SEPARATOR}${duration}`, "duration") <= room) return { ...vertical, duration };
    if (targetWidth <= room) return { ...vertical, duration: null };
  }
  return null;
};

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
