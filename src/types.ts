import { expandWorkoutSteps, type LeafStep } from "./workoutSteps";

export type Profile = {
  id: string;
  name: string;
  ftpWatts: number;
  maxPowerWatts: number;
  maxHeartRateBpm: number;
  riderWeightKg: number;
  bikeWeightKg: number;
  weightUnit: "kg" | "lb";
  distanceUnit: "km" | "mi";
};

export type PowerTarget =
  | { unit: "watts"; value: number }
  | { unit: "percentFtp"; value: number };

export type WorkoutStep =
  | { kind: "steady"; durationSeconds: number; target: PowerTarget }
  | {
      kind: "ramp";
      durationSeconds: number;
      start: PowerTarget;
      end: PowerTarget;
    }
  | { kind: "freeRide"; durationSeconds: number }
  | { kind: "repeat"; repetitions: number; steps: WorkoutStep[] };

export type Workout = {
  id: string;
  name: string;
  description: string;
  source: string;
  version: number;
  steps: WorkoutStep[];
  createdAt: string;
  updatedAt: string;
};

export type WorkoutInterval = {
  kind: Exclude<WorkoutStep["kind"], "repeat">;
  durationSeconds: number;
  startWatts: number | null;
  endWatts: number | null;
  freeRide: boolean;
};

export type Capability = "ftms" | "heartRate" | "cyclingPower" | "csc";

export type DeviceTransport = "ble" | "ant";

export type DeviceInfo = {
  id: string;
  name: string;
  /** Missing only in records written before transports were explicit. */
  transport?: DeviceTransport;
  simulated: boolean;
  rssi: number | null;
  capabilities?: Capability[];
};

export type DeviceRole = "trainer" | "heartRate" | "power" | "cadence";

export const deviceRoles: DeviceRole[] = ["trainer", "heartRate", "power", "cadence"];

export const deviceRoleLabel: Record<DeviceRole, string> = {
  trainer: "Trainer",
  heartRate: "Heart rate",
  power: "Power meter",
  cadence: "Cadence sensor",
};

export type DeviceLogLine = {
  atMs: number;
  level: "info" | "ok" | "warn" | "error";
  step: string;
  detail: string | null;
  connect: boolean;
};

export type SlotStats = {
  samples: number;
  parseFailures: number;
  lastSampleMs: number | null;
  rateHz: number;
  rssi: number | null;
  batteryPercent: number | null;
  batteryStatus: string | null;
  batteryVoltage: number | null;
  manufacturer: string | null;
  model: string | null;
  firmware: string | null;
  connectedSinceMs: number | null;
  drops: number;
  /** Automatic reconnect attempt in progress (0 or absent when none). */
  reconnectAttempt?: number;
  lastRawHex: string | null;
  /** Human summary of the latest decoded reading, e.g. "215 W · 88 rpm". */
  lastReading: string | null;
  /** The device offers a procedure we can drive: spin-down or zero offset. */
  calibrationSupported: boolean;
  calibrating: boolean;
  /** The device itself is asking to be calibrated (power meter indicator flag). */
  calibrationRequested?: boolean;
  /** Last calibration on record for this device, if any. */
  lastCalibration?: CalibrationRecord | null;
};

export type CalibrationKind = "spinDown" | "zeroOffset";

/** One completed calibration. `offsetRaw` is the meter's zero in its own units. */
export type CalibrationRecord = {
  /** RFC 3339 timestamp. */
  at: string;
  kind: CalibrationKind;
  offsetRaw: number | null;
};

/** Roles whose device can offer a calibration procedure. */
export const calibratableRoles: DeviceRole[] = ["trainer", "power"];

export type DeviceSlot = {
  role: DeviceRole;
  state: DeviceState;
  stats: SlotStats;
  log: DeviceLogLine[];
};

export type KnownDevice = {
  id: string;
  name: string;
  /** Missing only in records written before transports were explicit. */
  transport?: DeviceTransport;
  role: DeviceRole;
  capabilities: Capability[];
  simulated: boolean;
  manufacturer: string | null;
  model: string | null;
  /** RFC 3339 timestamp of the last successful connection. */
  lastConnectedAt: string;
  lastCalibration?: CalibrationRecord | null;
};

/** What happened to one remembered device during "Connect all". */
export type KnownConnectOutcome = {
  role: DeviceRole;
  name: string;
  /** `skipped` means the role already had a device connected or connecting. */
  status: "connected" | "skipped" | "failed";
  error: string | null;
};

export type SourceChoice = { mode: "auto" } | { mode: "role"; role: DeviceRole };

export type Metric = "power" | "cadence" | "heartRate";

export type SourcePreferences = Record<Metric, SourceChoice>;

export type TelemetrySource = { role: DeviceRole; fallback: boolean };

/** Which device supplied each fused telemetry value. */
export type TelemetrySources = Record<Metric, TelemetrySource | null>;

export type AntAdapterStatus =
  | { status: "notAttached" }
  | { status: "ready"; name: string }
  | { status: "permissionDenied" | "busy" | "error"; message: string };

export type DevicesSnapshot = {
  scanning: boolean;
  scanError: { message: string; guidance: string } | null;
  antAdapter: AntAdapterStatus;
  slots: DeviceSlot[];
  sourcePreferences: SourcePreferences;
  sources: TelemetrySources;
};

export type ConnectProgress = {
  step: string;
  detail: string | null;
  level: "info" | "ok" | "warn";
};

export type CalibrationPhase =
  | "preparing"
  /** Spin-down: pedal into the target range. */
  | "accelerate"
  /** Spin-down: stop and let the flywheel coast. */
  | "stopPedaling"
  /** Zero offset: the meter is measuring; keep the bike still. */
  | "holdStill"
  | "success"
  | "error";

export type CalibrationDetail =
  | { kind: "spinDown"; targetLowKph: number; targetHighKph: number }
  | { kind: "zeroOffset"; offsetRaw: number; previousOffsetRaw: number | null };

/** One step of a calibration, on `devices://calibration`. */
export type CalibrationProgress = {
  role: DeviceRole;
  phase: CalibrationPhase;
  message: string | null;
  detail: CalibrationDetail | null;
};

export type DeviceState =
  | { status: "idle" | "scanning" }
  | { status: "connecting" | "reconnecting"; name: string }
  | { status: "ready" | "controlling"; device: DeviceInfo }
  | { status: "error"; message: string; guidance: string };

export type Telemetry = {
  timestampMs: number;
  powerWatts: number;
  cadenceRpm: number | null;
  speedKph: number | null;
  heartRateBpm: number | null;
  targetPowerWatts: number | null;
  /** Present on live samples from the devices hub; absent on recorded history. */
  sources?: TelemetrySources;
};

/** How the trainer is keeping up with the ride (absent on older payloads). */
export type ControlStatus = "ok" | "degraded" | "lost";

export type RunnerState =
  | { status: "idle" }
  | {
      status: "running";
      sessionId: string;
      workoutName: string;
      elapsedSeconds: number;
      totalSeconds: number | null;
      intervalIndex: number;
      intervalElapsedSeconds: number;
      /** Target sent to the trainer right now (after bias and any override). */
      targetPowerWatts: number | null;
      /** What the workout plan asks for, before bias/override; null on free ride. */
      plannedTargetWatts: number | null;
      /** A free-ride interval is running (manual ERG). */
      manualErg: boolean;
      /** The rider has overridden this interval's target. */
      overrideActive: boolean;
      biasPercent: number;
      control?: ControlStatus;
      recordingWarning?: string | null;
    }
  | {
      status: "paused";
      sessionId: string;
      workoutName: string;
      elapsedSeconds: number;
      totalSeconds: number | null;
      intervalIndex: number;
      intervalElapsedSeconds: number;
      targetPowerWatts: number | null;
      plannedTargetWatts: number | null;
      manualErg: boolean;
      overrideActive: boolean;
      biasPercent: number;
      control?: ControlStatus;
      recordingWarning?: string | null;
    }
  | { status: "finished"; sessionId: string; completed: boolean; saveWarning?: string | null }
  | { status: "error"; message: string; sessionId: string | null };

export type SessionSummary = {
  id: string;
  workoutId: string | null;
  workoutName: string;
  startedAt: string;
  endedAt: string | null;
  elapsedSeconds: number;
  averagePowerWatts: number;
  maxPowerWatts: number;
  averageCadenceRpm: number | null;
  estimatedDistanceMeters: number;
  distanceSource: "trainer" | "power" | "mixed" | null;
  distanceWeightKg: number;
  completed: boolean;
  recordingWarning?: string | null;
};

export type SessionDetail = {
  summary: SessionSummary;
  samples: Telemetry[];
};

export type ZoneDefinition = {
  name: string;
  upperBound: number | null;
};

/**
 * Where a zone set comes from: following FTP / max HR, edited by hand, or
 * imported from Intervals.icu. Mirrors `ZoneMode` in storage.rs.
 */
export type ZoneMode = "derived" | "custom" | "intervals";

export type TrainingZoneSettings = {
  version: 1;
  syncPowerZonesFromIntervals: boolean;
  syncHeartRateZonesFromIntervals: boolean;
  powerMode: ZoneMode;
  powerZones: ZoneDefinition[];
  heartRateMode: ZoneMode;
  heartRateZones: ZoneDefinition[];
};

export const rideCardIds = [
  "power",
  "cadence",
  "speed",
  "heartRate",
  "workoutTimeline",
  "targetAndBias",
  "powerChart",
  "heartRateChart",
  "timeInZone",
  "deviceStats",
] as const;

export type RideCardId = (typeof rideCardIds)[number];

/**
 * Whether a card is shown to a rider who has not chosen for themselves.
 * Diagnostics cards default to hidden. Mirrors `RIDE_CARDS` in storage.rs.
 */
export const rideCardDefaultVisible: Record<RideCardId, boolean> = {
  power: true,
  cadence: true,
  speed: true,
  heartRate: true,
  workoutTimeline: true,
  targetAndBias: true,
  powerChart: true,
  heartRateChart: true,
  timeInZone: true,
  deviceStats: false,
};

export type RideCardPreference = {
  id: RideCardId;
  visible: boolean;
};

export type RideDisplayPreferences = {
  version: 2;
  cards: RideCardPreference[];
};

export const defaultTrainingZoneSettings: TrainingZoneSettings = {
  version: 1,
  syncPowerZonesFromIntervals: false,
  syncHeartRateZonesFromIntervals: false,
  powerMode: "derived",
  powerZones: [],
  heartRateMode: "derived",
  heartRateZones: [],
};

export const defaultRideDisplayPreferences: RideDisplayPreferences = {
  version: 2,
  cards: rideCardIds.map((id) => ({ id, visible: rideCardDefaultVisible[id] })),
};

export function normalizeRideDisplayPreferences(
  preferences: RideDisplayPreferences,
): RideDisplayPreferences {
  const known = new Set<RideCardId>(rideCardIds);
  const seen = new Set<RideCardId>();
  const cards = preferences.cards.filter((card) => {
    if (!known.has(card.id) || seen.has(card.id)) return false;
    seen.add(card.id);
    return true;
  });
  for (const id of rideCardIds) {
    if (!seen.has(id)) cards.push({ id, visible: rideCardDefaultVisible[id] });
  }
  return { version: 2, cards };
}

const zoneNames = (count: number) =>
  Array.from({ length: count }, (_, index) => `Zone ${index + 1}`);

export const derivedPowerZones = (ftpWatts: number): ZoneDefinition[] => {
  const percentages = [55, 75, 90, 105, 120, 150];
  return [
    ...percentages.map((percent, index) => ({
      name: zoneNames(7)[index],
      upperBound: Math.round((ftpWatts * percent) / 100),
    })),
    { name: "Zone 7", upperBound: null },
  ];
};

export const derivedHeartRateZones = (maxHeartRateBpm: number): ZoneDefinition[] => {
  const percentages = [60, 70, 80, 90];
  return [
    ...percentages.map((percent, index) => ({
      name: zoneNames(5)[index],
      upperBound: Math.round((maxHeartRateBpm * percent) / 100),
    })),
    { name: "Zone 5", upperBound: null },
  ];
};

export const effectivePowerZones = (
  settings: TrainingZoneSettings,
  ftpWatts: number,
): ZoneDefinition[] =>
  settings.powerMode !== "derived" && settings.powerZones.length > 1
    ? settings.powerZones
    : derivedPowerZones(ftpWatts);

export const effectiveHeartRateZones = (
  settings: TrainingZoneSettings,
  maxHeartRateBpm: number,
): ZoneDefinition[] =>
  settings.heartRateMode !== "derived" && settings.heartRateZones.length > 1
    ? settings.heartRateZones
    : derivedHeartRateZones(maxHeartRateBpm);

export const zoneModeLabel: Record<ZoneMode, string> = {
  derived: "Derived",
  custom: "Custom",
  intervals: "From Intervals.icu",
};

/** One zone set's fate in a training-settings sync. Mirrors `ZoneSetOutcome` in commands.rs. */
export type ZoneSetOutcome =
  | { status: "imported" | "unchanged" | "syncOff" | "notConfigured" }
  | { status: "invalid"; reason: string };

/** Mirrors `TrainingSyncResult` in commands.rs. */
export type TrainingSyncResult = {
  profile: Profile;
  zones: TrainingZoneSettings;
  ftp: { watts: number; previousWatts: number; source: "indoorFtp" | "ftp" };
  /** null when Intervals.icu has no max HR and the local value was kept. */
  maxHeartRate: { bpm: number; previousBpm: number } | null;
  powerZones: ZoneSetOutcome;
  heartRateZones: ZoneSetOutcome;
};

const describeZoneSet = (label: string, outcome: ZoneSetOutcome): string => {
  switch (outcome.status) {
    case "imported":
      return `${label} zones imported`;
    case "unchanged":
      return `${label} zones already up to date`;
    case "syncOff":
      return `${label} zones not imported (import is off)`;
    case "notConfigured":
      return `Intervals.icu has no ${label.toLowerCase()} zones`;
    case "invalid":
      return `${label} zones not imported: ${outcome.reason}`;
  }
};

/** The status line shown after a training-settings sync, one clause per item. */
export const describeTrainingSync = (result: TrainingSyncResult): string => {
  const ftpSource = result.ftp.source === "indoorFtp" ? "indoor FTP" : "FTP";
  const ftpChange =
    result.ftp.watts === result.ftp.previousWatts
      ? "unchanged"
      : `was ${result.ftp.previousWatts} W`;
  const clauses = [
    `FTP ${result.ftp.watts} W from your Intervals.icu ${ftpSource} (${ftpChange})`,
  ];
  if (result.maxHeartRate === null) {
    clauses.push("Intervals.icu has no max HR; yours is unchanged");
  } else {
    const change =
      result.maxHeartRate.bpm === result.maxHeartRate.previousBpm
        ? "unchanged"
        : `was ${result.maxHeartRate.previousBpm} bpm`;
    clauses.push(`Max HR ${result.maxHeartRate.bpm} bpm (${change})`);
  }
  clauses.push(describeZoneSet("Power", result.powerZones));
  clauses.push(describeZoneSet("Heart-rate", result.heartRateZones));
  return clauses.join(" · ");
};

export const zoneIndex = (value: number, zones: readonly ZoneDefinition[]): number =>
  zones.findIndex((zone) => zone.upperBound === null || value <= zone.upperBound);

export const timeInZones = (
  samples: Telemetry[],
  zones: ZoneDefinition[],
  metric: "power" | "heartRate",
  maxGapMs = 5_000,
): number[] => {
  const totals = zones.map(() => 0);
  for (let index = 0; index < samples.length - 1; index += 1) {
    const sample = samples[index];
    const durationMs = samples[index + 1].timestampMs - sample.timestampMs;
    if (durationMs <= 0 || durationMs > maxGapMs) continue;
    const value = metric === "power" ? sample.powerWatts : sample.heartRateBpm;
    if (value === null) continue;
    const found = zoneIndex(value, zones);
    if (found >= 0) totals[found] += durationMs / 1000;
  }
  return totals;
};

/** Keep chart rendering bounded while preserving the first and last samples. */
export const withActiveElapsed = <T extends Pick<Telemetry, "timestampMs">>(
  samples: T[],
  expectedElapsedSeconds?: number,
  maxGapMs = 5_000,
): Array<T & { activeElapsedMs: number }> => {
  let elapsedMs = 0;
  const result = samples.map((sample, index) => {
    if (index > 0) {
      const delta = sample.timestampMs - samples[index - 1].timestampMs;
      if (delta > 0 && delta <= maxGapMs) elapsedMs += delta;
    }
    return { ...sample, activeElapsedMs: elapsedMs };
  });
  const expectedMs = expectedElapsedSeconds === undefined
    ? elapsedMs
    : Math.max(0, expectedElapsedSeconds * 1000);
  if (elapsedMs > 0 && expectedMs !== elapsedMs) {
    return result.map((sample) => ({
      ...sample,
      activeElapsedMs: Math.round((sample.activeElapsedMs / elapsedMs) * expectedMs),
    }));
  }
  return result;
};

export const downsampleTelemetry = <T extends Telemetry>(
  samples: T[],
  maximum = 1_200,
): T[] => {
  if (samples.length <= maximum || maximum < 2) return samples;
  const result = [samples[0]];
  const step = (samples.length - 1) / (maximum - 1);
  for (let index = 1; index < maximum - 1; index += 1) {
    result.push(samples[Math.round(index * step)]);
  }
  result.push(samples[samples.length - 1]);
  return result;
};

export const workoutDuration = (steps: readonly WorkoutStep[]): number => {
  const expanded = expandWorkoutSteps(steps);
  return expanded.length === 0 ? 0 : expanded[expanded.length - 1].endSeconds;
};

/** Mirrors the Rust target resolution, including its u16 clamp on `% FTP` targets. */
export const targetWatts = (target: PowerTarget, ftpWatts: number): number =>
  target.unit === "watts"
    ? target.value
    : Math.min(65_535, Math.floor((ftpWatts * target.value) / 100));

export const BIAS_STEP_PERCENT = 1;
export const MIN_BIAS_PERCENT = 50;
export const MAX_BIAS_PERCENT = 150;
export const DEFAULT_BIAS_PERCENT = 100;

export const clampBias = (percent: number): number =>
  Math.min(MAX_BIAS_PERCENT, Math.max(MIN_BIAS_PERCENT, Math.round(percent)));

/**
 * Scales a planned target by the ride's bias. Mirrors the Rust runner's
 * `biased_target`: nearest watt, never zero (ERG treats 0 W as "no target").
 */
export const biasedWatts = (plannedWatts: number, biasPercent: number): number =>
  Math.min(65_535, Math.max(1, Math.floor((plannedWatts * clampBias(biasPercent) + 50) / 100)));

/**
 * Compiles one executed block, with the ride's bias applied to its targets.
 * Mirrors the Rust workout compiler's per-step output and the runner's bias.
 */
export const compileWorkoutInterval = (
  step: LeafStep,
  ftpWatts: number,
  biasPercent = DEFAULT_BIAS_PERCENT,
): WorkoutInterval => {
  const biased = (target: PowerTarget) => biasedWatts(targetWatts(target, ftpWatts), biasPercent);
  if (step.kind === "steady") {
    const watts = biased(step.target);
    return {
      kind: step.kind,
      durationSeconds: step.durationSeconds,
      startWatts: watts,
      endWatts: watts,
      freeRide: false,
    };
  }
  if (step.kind === "ramp") {
    return {
      kind: step.kind,
      durationSeconds: step.durationSeconds,
      startWatts: biased(step.start),
      endWatts: biased(step.end),
      freeRide: false,
    };
  }
  return {
    kind: step.kind,
    durationSeconds: step.durationSeconds,
    startWatts: null,
    endWatts: null,
    freeRide: true,
  };
};

/** Mirrors the Rust workout compiler so interval indexes align with the runner. */
export const compileWorkoutIntervals = (
  steps: readonly WorkoutStep[],
  ftpWatts: number,
  biasPercent = DEFAULT_BIAS_PERCENT,
): WorkoutInterval[] =>
  expandWorkoutSteps(steps).map(({ step }) => compileWorkoutInterval(step, ftpWatts, biasPercent));

/** "Free ride", "200 W", or "150–220 W" for a ramp. */
export const formatIntervalTarget = (
  startWatts: number | null,
  endWatts: number | null,
): string =>
  startWatts === null || endWatts === null
    ? "Free ride"
    : startWatts === endWatts
      ? `${startWatts} W`
      : `${startWatts}–${endWatts} W`;

export const formatDuration = (seconds: number): string => {
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const remaining = seconds % 60;
  return hours > 0
    ? `${hours}:${String(minutes).padStart(2, "0")}:${String(remaining).padStart(2, "0")}`
    : `${minutes}:${String(remaining).padStart(2, "0")}`;
};

export const formatDistance = (
  meters: number,
  unit: Profile["distanceUnit"],
): { value: string; unit: string } => {
  const distance = unit === "mi" ? meters / 1609.344 : meters / 1000;
  return {
    value: distance.toFixed(distance < 10 ? 2 : 1),
    unit,
  };
};

export const formatSpeed = (
  speedKph: number,
  unit: Profile["distanceUnit"],
): { value: string; unit: string } => ({
  value: (unit === "mi" ? speedKph / 1.609344 : speedKph).toFixed(1),
  unit: unit === "mi" ? "mph" : "km/h",
});

export const manualPowerDeltaForKey = (
  key: string,
  repeat: boolean,
): number | null => {
  if (repeat) return null;
  if (key === "ArrowUp") return 5;
  if (key === "ArrowDown") return -5;
  return null;
};

export type RideKeyAction =
  | { kind: "power"; delta: number }
  | { kind: "bias"; delta: number };

/**
 * Keyboard shortcuts on the live ride view: ↑/↓ nudge the target by 5 W,
 * Shift+↑/↓ nudge the workout bias by 1 %. Held keys do not repeat.
 */
export const rideKeyAction = (
  key: string,
  shift: boolean,
  repeat: boolean,
): RideKeyAction | null => {
  const delta = manualPowerDeltaForKey(key, repeat);
  if (delta === null) return null;
  return shift
    ? { kind: "bias", delta: Math.sign(delta) * BIAS_STEP_PERCENT }
    : { kind: "power", delta };
};

/** Live power averaging window on the ride screens. Mirrors the Rust enum. */
export type PowerSmoothing = "instant" | "3s" | "5s" | "10s";

export const powerSmoothingOptions: PowerSmoothing[] = ["instant", "3s", "5s", "10s"];

export const powerSmoothingLabel: Record<PowerSmoothing, string> = {
  instant: "Instant",
  "3s": "3 s avg",
  "5s": "5 s avg",
  "10s": "10 s avg",
};

export const powerSmoothingWindowMs: Record<PowerSmoothing, number> = {
  instant: 0,
  "3s": 3_000,
  "5s": 5_000,
  "10s": 10_000,
};

/**
 * Add a `displayPowerWatts` field to each sample: the mean of `powerWatts`
 * over the samples in the trailing window ending at that sample (by
 * timestamp). With the instant setting it is simply `powerWatts`.
 */
export const withSmoothedPower = <T extends Pick<Telemetry, "timestampMs" | "powerWatts">>(
  history: T[],
  smoothing: PowerSmoothing,
): Array<T & { displayPowerWatts: number }> => {
  const windowMs = powerSmoothingWindowMs[smoothing];
  if (windowMs === 0) {
    return history.map((sample) => ({ ...sample, displayPowerWatts: sample.powerWatts }));
  }
  let start = 0;
  let sum = 0;
  return history.map((sample, index) => {
    sum += sample.powerWatts;
    while (history[start].timestampMs <= sample.timestampMs - windowMs) {
      sum -= history[start].powerWatts;
      start += 1;
    }
    return { ...sample, displayPowerWatts: Math.round(sum / (index - start + 1)) };
  });
};
