import { memo, useId, useMemo } from "react";
import {
  DEFAULT_BIAS_PERCENT,
  formatDuration,
  type WorkoutStep,
  type ZoneDefinition,
} from "./types";
import { pathStartsWith, type StepPath } from "./workoutSteps";
import {
  LABEL_SEPARATOR,
  profileLabel,
  profileScale,
  profileSegments,
  profileShapes,
  profileStats,
  profileSummary,
  repeatSpans,
  timeAxisTicks,
  type ProfileShape,
} from "./workoutProfileModel";
import { useElementSize } from "./useElementSize";
import { zoneColor } from "./zones";

export type WorkoutProfileVariant = "editor" | "compact";

/** Per-block presentation supplied by the host, e.g. ride progress. */
export type SegmentState = {
  className?: string;
  /** Free-form state name exposed as `data-state` for styling and tests. */
  state?: string;
  /** 0–1 fraction of the block completed; drawn as an overlay clipped to the shape. */
  progress?: number;
};

export type WorkoutProfileProps = {
  steps: readonly WorkoutStep[];
  ftpWatts: number;
  /** Ride bias applied to every target, so the drawing matches what the trainer is asked for. */
  biasPercent?: number;
  powerZones: readonly ZoneDefinition[];
  /** `editor` adds the FTP line, time axis, and repeat brackets. `compact` draws shapes only. */
  variant?: WorkoutProfileVariant;
  /** Write each block's target and duration on it wherever there is room. */
  labels?: boolean;
  /** Blocks at or inside this path are emphasised and the rest dimmed. */
  highlightedPath?: StepPath | null;
  onHighlightPath?: (path: StepPath | null) => void;
  onSelectPath?: (path: StepPath) => void;
  segmentState?: (shape: ProfileShape, index: number) => SegmentState | undefined;
  emptyMessage?: string;
  className?: string;
};

const AXIS_HEIGHT = 16;
const REPEAT_BAND_HEIGHT = 14;
const LABEL_FONT_PX = 10;
const FALLBACK_SIZE = { width: 600, height: 120 };

const pathKey = (path: StepPath) => path.join(".");

const pointsAttribute = (points: ProfileShape["points"]) =>
  points.map(([x, y]) => `${round(x)},${round(y)}`).join(" ");

const round = (value: number) => Math.round(value * 100) / 100;

/**
 * The one drawing of a workout. Everything it shows comes from the pure
 * profile model in pixel space, so text and strokes are never distorted and
 * pointer positions map straight back to step paths.
 */
export const WorkoutProfile = memo(function WorkoutProfile({
  steps,
  ftpWatts,
  biasPercent = DEFAULT_BIAS_PERCENT,
  powerZones,
  variant = "compact",
  labels = false,
  highlightedPath = null,
  onHighlightPath,
  onSelectPath,
  segmentState,
  emptyMessage = "Add a block to see the shape of your workout",
  className,
}: WorkoutProfileProps) {
  const [canvasRef, size] = useElementSize<HTMLDivElement>(FALLBACK_SIZE);
  const idPrefix = useId().replace(/[^a-zA-Z0-9_-]/g, "");
  const editor = variant === "editor";

  const segments = useMemo(
    () => profileSegments(steps, ftpWatts, biasPercent),
    [steps, ftpWatts, biasPercent],
  );
  const stats = useMemo(() => profileStats(segments, ftpWatts), [segments, ftpWatts]);
  const spans = useMemo(() => (editor ? repeatSpans(steps, segments) : []), [editor, steps, segments]);

  const topBand = editor && spans.length > 0 ? REPEAT_BAND_HEIGHT : 0;
  const bottomBand = editor ? AXIS_HEIGHT : 0;
  const plotHeight = Math.max(1, size.height - topBand - bottomBand);

  const scale = useMemo(
    () => profileScale(segments, { width: size.width, height: plotHeight, ftpWatts }),
    [segments, size.width, plotHeight, ftpWatts],
  );
  const shapes = useMemo(() => profileShapes(segments, scale, 1), [segments, scale]);
  const ticks = useMemo(
    () => (editor ? timeAxisTicks(scale.totalSeconds, scale.width) : []),
    [editor, scale.totalSeconds, scale.width],
  );

  const interactive = Boolean(onHighlightPath || onSelectPath);
  const summary = profileSummary(stats);

  return (
    <figure
      className={["workout-profile", className].filter(Boolean).join(" ")}
      data-variant={variant}
      data-interactive={interactive ? "true" : undefined}
      role="img"
      aria-label={summary}
    >
      <div className="workout-profile-canvas" ref={canvasRef}>
        {shapes.length === 0 ? (
          editor ? <div className="workout-profile-empty">{emptyMessage}</div> : null
        ) : (
          <svg
            width={size.width}
            height={size.height}
            viewBox={`0 0 ${size.width} ${size.height}`}
            aria-hidden="true"
            focusable="false"
            onMouseLeave={onHighlightPath ? () => onHighlightPath(null) : undefined}
          >
            <defs>
              <pattern
                id={`${idPrefix}-hatch`}
                width="6"
                height="6"
                patternUnits="userSpaceOnUse"
                patternTransform="rotate(45)"
              >
                <line className="workout-profile-hatch" x1="0" y1="0" x2="0" y2="6" strokeWidth="1.5" />
              </pattern>
              {shapes.map((shape, index) =>
                shape.kind === "ramp" && shape.startWatts !== null && shape.endWatts !== null ? (
                  <linearGradient
                    key={index}
                    id={`${idPrefix}-ramp-${index}`}
                    gradientUnits="objectBoundingBox"
                    x1="0"
                    y1="0"
                    x2="1"
                    y2="0"
                  >
                    <stop offset="0" stopColor={zoneColor(shape.startWatts, powerZones)} />
                    <stop offset="1" stopColor={zoneColor(shape.endWatts, powerZones)} />
                  </linearGradient>
                ) : null,
              )}
              {segmentState &&
                shapes.map((shape, index) => (
                  <clipPath key={index} id={`${idPrefix}-clip-${index}`}>
                    <polygon points={pointsAttribute(shape.points)} />
                  </clipPath>
                ))}
            </defs>
            <g transform={`translate(0 ${topBand})`}>
              {editor && (
                <line
                  className="workout-profile-baseline"
                  x1={0}
                  x2={scale.width}
                  y1={scale.height}
                  y2={scale.height}
                />
              )}
              {shapes.map((shape, index) => {
                const state = segmentState?.(shape, index);
                const highlighted = highlightedPath !== null && pathStartsWith(shape.path, highlightedPath);
                const dimmed = highlightedPath !== null && !highlighted;
                const fill =
                  shape.startWatts === null || shape.endWatts === null
                    ? `url(#${idPrefix}-hatch)`
                    : shape.kind === "ramp"
                      ? `url(#${idPrefix}-ramp-${index})`
                      : zoneColor(shape.startWatts, powerZones);
                const label = labels ? profileLabel(shape, scale, LABEL_FONT_PX) : null;
                return (
                  <g key={`${pathKey(shape.path)}:${shape.iterations.join(".")}`}>
                    <polygon
                      className={["workout-profile-block", state?.className].filter(Boolean).join(" ")}
                      data-path={pathKey(shape.path)}
                      data-kind={shape.kind}
                      data-state={state?.state}
                      data-highlighted={highlighted ? "true" : undefined}
                      data-dimmed={dimmed ? "true" : undefined}
                      points={pointsAttribute(shape.points)}
                      fill={fill}
                      onMouseEnter={onHighlightPath ? () => onHighlightPath(shape.path) : undefined}
                      onClick={onSelectPath ? () => onSelectPath(shape.path) : undefined}
                    />
                    {state?.progress !== undefined && state.progress > 0 && (
                      <rect
                        className="workout-profile-progress"
                        data-progress-for={pathKey(shape.path)}
                        clipPath={`url(#${idPrefix}-clip-${index})`}
                        x={round(shape.x)}
                        y={0}
                        width={round(shape.width * Math.min(1, state.progress))}
                        height={scale.height}
                      />
                    )}
                    {label && (
                      <text
                        className="workout-profile-block-label"
                        data-label-for={pathKey(shape.path)}
                        data-placement={label.placement}
                        data-kind={shape.kind}
                        data-state={state?.state}
                        x={round(label.x)}
                        y={round(label.y)}
                        fontSize={LABEL_FONT_PX}
                        textAnchor={label.placement === "vertical" ? "start" : "middle"}
                        dominantBaseline={label.placement === "vertical" ? "central" : undefined}
                        transform={
                          label.placement === "vertical"
                            ? `rotate(-90 ${round(label.x)} ${round(label.y)})`
                            : undefined
                        }
                      >
                        <tspan className="workout-profile-block-target">{label.target}</tspan>
                        {label.duration !== null && label.placement === "vertical" && (
                          <>
                            <tspan className="workout-profile-block-separator">{LABEL_SEPARATOR}</tspan>
                            <tspan className="workout-profile-block-duration">{label.duration}</tspan>
                          </>
                        )}
                        {label.duration !== null && label.placement !== "vertical" && (
                          <tspan className="workout-profile-block-duration" x={round(label.x)} dy={round(label.lineHeight)}>
                            {label.duration}
                          </tspan>
                        )}
                      </text>
                    )}
                  </g>
                );
              })}
              {editor && ftpWatts > 0 && (
                <>
                  <line
                    className="workout-profile-ftp"
                    data-ftp-line
                    x1={0}
                    x2={scale.width}
                    y1={round(scale.y(ftpWatts))}
                    y2={round(scale.y(ftpWatts))}
                  />
                  <text
                    className="workout-profile-label"
                    x={scale.width - 3}
                    y={round(scale.y(ftpWatts)) - 3}
                    textAnchor="end"
                  >
                    FTP
                  </text>
                </>
              )}
            </g>
            {editor && spans.length > 0 && (
              <g className="workout-profile-repeats">
                {spans.map((span) => {
                  const left = round(scale.x(span.startSeconds)) + 1;
                  const right = round(scale.x(span.endSeconds)) - 1;
                  const y = topBand - 4 - span.depth * 3;
                  return (
                    <g key={pathKey(span.path)} data-repeat={pathKey(span.path)}>
                      <path
                        className="workout-profile-repeat"
                        d={`M${left} ${y - 4} V${y} H${right} V${y - 4}`}
                      />
                      <text
                        className="workout-profile-repeat-label"
                        x={(left + right) / 2}
                        y={y - 5}
                        textAnchor="middle"
                      >
                        {span.repetitions}×
                      </text>
                    </g>
                  );
                })}
              </g>
            )}
            {editor && (
              <g className="workout-profile-axis" transform={`translate(0 ${topBand + scale.height})`}>
                {ticks.map((seconds) => {
                  const x = round(scale.x(seconds));
                  return (
                    <g key={seconds}>
                      <line className="workout-profile-tick" x1={x} x2={x} y1={0} y2={3} />
                      <text className="workout-profile-tick-label" x={x} y={AXIS_HEIGHT - 3} textAnchor="middle">
                        {formatDuration(seconds)}
                      </text>
                    </g>
                  );
                })}
              </g>
            )}
          </svg>
        )}
      </div>
    </figure>
  );
});
