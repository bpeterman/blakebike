/**
 * The lap timer card: laps the rider marks by hand (the card's Lap button or
 * the L key) and, optionally, a repeating countdown drill that restarts with
 * every lap.
 *
 * Laps belong to the runner, not to this card: it owns the ride clock (so a
 * lap stops while the ride is paused) and records each mark with the ride so
 * the FIT file carries them as lap messages. This module only reads the marks
 * back and works out what the card shows.
 */
import { timeWeightedMean, type LapMark, type Telemetry } from "./types";

export const lapTimerModes = ["manual", "countdown"] as const;

export type LapTimerMode = (typeof lapTimerModes)[number];

export const lapTimerModeLabels: Record<LapTimerMode, string> = {
  manual: "Manual laps",
  countdown: "Countdown drill",
};

export const isLapTimerMode = (value: unknown): value is LapTimerMode =>
  lapTimerModes.some((mode) => mode === value);

/** How a lap timer card is set up. Persisted with the ride-screen layout. */
export type LapTimerSettings = {
  mode: LapTimerMode;
  /** Countdown drill: the work part of each repeat. */
  workSeconds: number;
  /** Countdown drill: recovery after each work part; 0 repeats the work part back to back. */
  restSeconds: number;
};

/** Bounds on the drill lengths. Mirrored by `LapTimerSettings` in `storage.rs`. */
export const LAP_TIMER_WORK_SECONDS = { min: 5, max: 3_600 } as const;
export const LAP_TIMER_REST_SECONDS = { min: 0, max: 3_600 } as const;

export const defaultLapTimerSettings: LapTimerSettings = {
  mode: "manual",
  workSeconds: 30,
  restSeconds: 30,
};

const wholeSecondsIn = (
  value: number,
  bounds: { min: number; max: number },
  fallback: number,
): number =>
  Number.isFinite(value) ? Math.min(bounds.max, Math.max(bounds.min, Math.round(value))) : fallback;

/** Whole seconds inside the bounds, and a mode this version knows. */
export const normalizeLapTimerSettings = (settings: LapTimerSettings): LapTimerSettings => ({
  mode: isLapTimerMode(settings.mode) ? settings.mode : defaultLapTimerSettings.mode,
  workSeconds: wholeSecondsIn(
    settings.workSeconds,
    LAP_TIMER_WORK_SECONDS,
    defaultLapTimerSettings.workSeconds,
  ),
  restSeconds: wholeSecondsIn(
    settings.restSeconds,
    LAP_TIMER_REST_SECONDS,
    defaultLapTimerSettings.restSeconds,
  ),
});

export type LapAverages = {
  powerWatts: number | null;
  cadenceRpm: number | null;
};

export type LapStatus = {
  /** The lap being ridden, counting from 1. */
  number: number;
  /** Riding time into the current lap. */
  seconds: number;
  /** How long the previous lap took; null during the first. */
  lastSeconds: number | null;
  current: LapAverages;
  last: LapAverages | null;
};

const averages = (samples: readonly Telemetry[]): LapAverages => ({
  powerWatts: timeWeightedMean(samples, (sample) => sample.powerWatts),
  cadenceRpm: timeWeightedMean(samples, (sample) => sample.cadenceRpm),
});

/**
 * Where the rider is in their laps. Times come from the ride clock; the
 * averages come from the samples recorded between the marks' wall times, so
 * a pause inside a lap neither counts towards it nor skews its averages.
 */
export const lapStatus = (
  laps: readonly LapMark[],
  elapsedSeconds: number,
  history: readonly Telemetry[],
): LapStatus => {
  const latest: LapMark | undefined = laps[laps.length - 1];
  if (latest === undefined) {
    return {
      number: 1,
      seconds: elapsedSeconds,
      lastSeconds: null,
      current: averages(history),
      last: null,
    };
  }
  const previous: LapMark | undefined = laps[laps.length - 2];
  const lastStartMs = previous?.timestampMs ?? Number.NEGATIVE_INFINITY;
  return {
    number: laps.length + 1,
    seconds: Math.max(0, elapsedSeconds - latest.elapsedSeconds),
    lastSeconds: latest.elapsedSeconds - (previous?.elapsedSeconds ?? 0),
    current: averages(history.filter((sample) => sample.timestampMs >= latest.timestampMs)),
    last: averages(
      history.filter(
        (sample) => sample.timestampMs >= lastStartMs && sample.timestampMs < latest.timestampMs,
      ),
    ),
  };
};

export type DrillPhase = {
  kind: "work" | "rest";
  /** The repeat being ridden, counting from 1. */
  repeat: number;
  /** Seconds left in this part, counting down to 1. */
  remainingSeconds: number;
  /** Counts every rollover since the lap began; a change is the cue to beep. */
  index: number;
};

/** Where a lap is in its countdown drill, `lapSeconds` into the lap. */
export const drillPhase = (settings: LapTimerSettings, lapSeconds: number): DrillPhase => {
  const { workSeconds, restSeconds } = settings;
  const cycle = workSeconds + restSeconds;
  const into = Math.max(0, Math.floor(lapSeconds));
  const repeatIndex = Math.floor(into / cycle);
  const within = into % cycle;
  const resting = within >= workSeconds;
  const phasesPerRepeat = restSeconds > 0 ? 2 : 1;
  return {
    kind: resting ? "rest" : "work",
    repeat: repeatIndex + 1,
    remainingSeconds: resting ? cycle - within : workSeconds - within,
    index: repeatIndex * phasesPerRepeat + (resting ? 1 : 0),
  };
};
