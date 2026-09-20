import { offsetDrift } from "./devices";
import type { CalibrationRecord, DeviceRole, RideDevice, Telemetry } from "./types";

/**
 * The trainer-versus-power-meter report, as `get_power_comparison` returns it
 * (mirrors `dual_power.rs`). The difference is always trainer minus meter;
 * positive means the trainer reads higher.
 */
export type PowerComparison = InsufficientComparison | ReadyComparison;

export type InsufficientComparison = {
  status: "insufficient";
  sessionId: string;
  reason: string;
  minOverlapSeconds: number;
  coverage: Coverage;
  a: ComparedSource;
  b: ComparedSource;
  signConvention: string;
};

export type ReadyComparison = {
  status: "ready";
  sessionId: string;
  workoutName: string;
  startedAt: string;
  elapsedSeconds: number;
  /** The trainer. */
  a: ComparedSource;
  /** The power meter. */
  b: ComparedSource;
  signConvention: string;
  coverage: Coverage;
  exclusion: Exclusion;
  lag: Lag;
  windows: WindowStats[];
  headlineWindowSeconds: number;
  trend: Trend;
  warmup: Warmup | null;
  /** Per elapsed minute (`lower`/`upper` in minutes). */
  byMinute: Bin[];
  minuteBinMinPairs: number;
  /** Per power level (`lower`/`upper` in watts). */
  byPower: Bin[];
  powerBinWatts: number;
  byCadence: CadenceBins | null;
  cadenceBinRpm: number;
  binMinPairs: number;
  traces: TracePoint[];
  /** `[mean, difference]` pairs. */
  blandAltman: Array<[number, number]>;
};

export type ComparedSource = {
  role: DeviceRole;
  device: RideDevice | null;
  readings: number;
  rateHz: number;
  seconds: number;
  meanWatts: number | null;
  maxWatts: number | null;
};

export type Coverage = {
  rideSeconds: number;
  overlapSeconds: number;
  overlapStartSeconds: number;
  overlapEndSeconds: number;
  comparedSeconds: number;
  excludedSeconds: number;
  coastingSeconds: number;
  transientSeconds: number;
};

export type Exclusion = { minWatts: number; transientWattsPerSecond: number; guardSeconds: number };

export type Lag = {
  seconds: number;
  correlation: number;
  confident: boolean;
  searchSeconds: number;
  note: string;
};

export type WindowStats = {
  windowSeconds: number;
  count: number;
  meanWatts: number;
  meanPercent: number;
  sdWatts: number;
  loaLowWatts: number;
  loaHighWatts: number;
  p95AbsWatts: number;
  p95AbsPercent: number;
  maxAbsWatts: number;
  maxAbsPercent: number;
};

export type Trend = { interceptWatts: number; slopePercent: number };

export type Warmup = {
  minutes: number;
  firstMeanPercent: number;
  restMeanPercent: number;
  firstCount: number;
  restCount: number;
};

export type Bin = {
  lower: number;
  upper: number;
  count: number;
  meanWatts: number;
  meanPercent: number;
  sdWatts: number;
};

export type CadenceBins = { source: DeviceRole; bins: Bin[] };

export type TracePoint = { elapsedSeconds: number; a: number | null; b: number | null };

/** The short form of the sign convention, for labels next to a number. */
export const DELTA_LABEL = "trainer − meter";

/** Below this the live percent is meaningless (mirrors the analysis' coasting rule). */
export const LIVE_DELTA_MIN_WATTS = 30;

/** "+7 W", "−2.8%", "±0 W": an explicit sign on every difference. */
export function formatSigned(value: number, digits: number, unit: string): string {
  const rounded = Number(value.toFixed(digits));
  const sign = rounded > 0 ? "+" : rounded < 0 ? "−" : "±";
  const space = unit === "%" ? "" : " ";
  return `${sign}${Math.abs(rounded).toFixed(digits)}${space}${unit}`;
}

export function formatWatts(value: number | null | undefined, digits = 0): string {
  return value === null || value === undefined ? "—" : `${value.toFixed(digits)} W`;
}

/** "12:34" or "1:02:03" from seconds. */
export function formatClockSeconds(totalSeconds: number): string {
  const seconds = Math.max(0, Math.round(totalSeconds));
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const rest = seconds % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  return hours > 0 ? `${hours}:${pad(minutes)}:${pad(rest)}` : `${minutes}:${pad(rest)}`;
}

export type LiveDelta = {
  trainerWatts: number;
  meterWatts: number;
  /** Trainer minus meter. */
  watts: number;
  /** Relative to the mean of the two; null while either coasts. */
  percent: number | null;
};

/** The live trainer-versus-meter difference from one fused sample, or null without both. */
export function liveDelta(powerBySource: Telemetry["powerBySource"]): LiveDelta | null {
  const trainerWatts = powerBySource?.trainer;
  const meterWatts = powerBySource?.power;
  if (trainerWatts === undefined || meterWatts === undefined) return null;
  const watts = trainerWatts - meterWatts;
  const mean = (trainerWatts + meterWatts) / 2;
  const coasting = trainerWatts < LIVE_DELTA_MIN_WATTS || meterWatts < LIVE_DELTA_MIN_WATTS;
  return { trainerWatts, meterWatts, watts, percent: coasting ? null : (watts / mean) * 100 };
}

export type RollingDelta = { watts: number; percent: number; count: number };

/**
 * A rolling window of live deltas so the nerd card can show a steadier number
 * next to the instantaneous one. Fed from live samples; samples while either
 * device coasts are kept out, like the ride analysis does.
 */
export class DeltaWindow {
  private readonly entries: Array<{ atMs: number; trainer: number; meter: number }> = [];

  constructor(private readonly windowMs = 30_000) {}

  push(sample: Pick<Telemetry, "timestampMs" | "powerBySource">): void {
    const delta = liveDelta(sample.powerBySource);
    if (!delta || delta.percent === null) return;
    const last = this.entries[this.entries.length - 1];
    if (last && last.atMs === sample.timestampMs) return;
    this.entries.push({ atMs: sample.timestampMs, trainer: delta.trainerWatts, meter: delta.meterWatts });
    this.trim(sample.timestampMs);
  }

  /** Mean difference over the window ending at `nowMs`, as watts and ratio of sums. */
  mean(nowMs: number): RollingDelta | null {
    this.trim(nowMs);
    if (this.entries.length === 0) return null;
    let trainer = 0;
    let meter = 0;
    for (const entry of this.entries) {
      trainer += entry.trainer;
      meter += entry.meter;
    }
    return {
      watts: (trainer - meter) / this.entries.length,
      percent: (trainer / meter - 1) * 100,
      count: this.entries.length,
    };
  }

  private trim(nowMs: number): void {
    while (this.entries.length > 0 && nowMs - this.entries[0].atMs > this.windowMs) {
      this.entries.shift();
    }
  }
}

export function deviceTitle(device: RideDevice | null, fallback: string): string {
  return device?.name ?? fallback;
}

/** "Wahoo KICKR CORE · fw 1.2.3 · simulated" under a device name. */
export function deviceDetail(device: RideDevice | null): string {
  if (!device) return "device not on record";
  const parts: string[] = [];
  const make = [device.manufacturer, device.model].filter((part): part is string => !!part).join(" ");
  if (make && make !== device.name) parts.push(make);
  if (device.firmware) parts.push(`fw ${device.firmware}`);
  if (device.simulated) parts.push("simulated");
  return parts.join(" · ");
}

/** How the meter's last zero relates to the ride, for the report footer. */
export function calibrationContext(
  record: CalibrationRecord | null | undefined,
  rideStartedAt: string,
): string | null {
  if (!record || record.kind !== "zeroOffset") return null;
  const at = Date.parse(record.at);
  const start = Date.parse(rideStartedAt);
  if (Number.isNaN(at) || Number.isNaN(start)) return null;
  const hours = (start - at) / 3_600_000;
  const when = hours < 0
    ? "after the ride"
    : hours < 1
      ? `${Math.max(1, Math.round(hours * 60))} min before the ride`
      : hours < 48
        ? `${hours.toFixed(hours < 10 ? 1 : 0)} h before the ride`
        : `${Math.round(hours / 24)} days before the ride`;
  if (record.offsetRaw === null) return `Meter zeroed ${when}.`;
  const drift = offsetDrift(record.offsetRaw, record.previousOffsetRaw);
  const driftText = drift
    ? ` (${drift.delta >= 0 ? "+" : "−"}${Math.abs(drift.delta)} vs the zero before)`
    : "";
  return `Meter zeroed ${when} · offset ${record.offsetRaw}${driftText}.`;
}

/** "+2.1 W flat plus 1.8% of power": additive versus proportional error in words. */
export function describeTrend(trend: Trend): string {
  const flat = `${formatSigned(trend.interceptWatts, 1, "W")} flat`;
  if (Math.abs(trend.slopePercent) < 0.05) return `${flat}, no change with power`;
  return `${flat}, ${trend.slopePercent > 0 ? "plus" : "minus"} ${Math.abs(trend.slopePercent).toFixed(1)}% of power`;
}

export function headlineWindow(report: ReadyComparison): WindowStats {
  return (
    report.windows.find((window) => window.windowSeconds === report.headlineWindowSeconds)
    ?? report.windows[0]
  );
}

/** What the lag line should say, sign convention included. */
export function describeLag(lag: Lag): string {
  if (!lag.confident) return `Lag: not estimated (${lag.note.replace(/\.$/, "")}).`;
  if (Math.abs(lag.seconds) < 0.125) return "Lag: none; the two streams were already aligned.";
  const who = lag.seconds > 0 ? "trainer trails the meter" : "meter trails the trainer";
  return `Lag: ${Math.abs(lag.seconds).toFixed(2)} s (${who}), corrected before comparing.`;
}
