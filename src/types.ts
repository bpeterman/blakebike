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

/**
 * Where a mirrored workout came from. Present only on workouts synced from
 * the Intervals.icu library; `source` stays the one-word label the card shows.
 * Mirrors `WorkoutOrigin` in domain.rs.
 */
export type WorkoutOrigin = {
  externalId: number;
  folderId: number | null;
  folder: string | null;
  updated: string;
  plannedLoad: number | null;
};

export type Workout = {
  id: string;
  name: string;
  description: string;
  /** `local`, `zwo` (file import) or `intervals` (mirrored from Intervals.icu). */
  source: string;
  version: number;
  steps: WorkoutStep[];
  createdAt: string;
  updatedAt: string;
  /** Set on mirrored workouts; absent or null otherwise. */
  origin?: WorkoutOrigin | null;
};

/** Mirrored from Intervals.icu: read-only locally, refreshed by sync. */
export const isMirrored = (workout: Workout): boolean => workout.origin != null;

/** A fresh local copy of a workout, for "Edit a copy" on a mirrored one. */
export const localCopyOf = (workout: Workout, now = new Date()): Workout => ({
  ...workout,
  id: crypto.randomUUID(),
  name: `${workout.name} (copy)`,
  source: "local",
  origin: null,
  createdAt: now.toISOString(),
  updatedAt: now.toISOString(),
});

/** One planned workout from the Intervals.icu calendar. Mirrors `PlannedWorkout` in domain.rs. */
export type PlannedWorkout = {
  eventId: number;
  /** Stable per event; what `startWorkout` takes. */
  workoutId: string;
  /** Local date, `YYYY-MM-DD`. */
  date: string;
  name: string;
  description: string;
  activityType: string;
  plannedLoad: number | null;
  durationSeconds: number | null;
  /** Structure, when Intervals.icu supplied a readable ZWO. */
  workout: Workout | null;
  parseError: string | null;
  updated: string | null;
  fetchedAt: string;
};

/** Today as `YYYY-MM-DD` in the machine's local time zone, matching `PlannedWorkout.date`. */
export const localDateString = (date = new Date()): string => {
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
};

export const plannedOn = (planned: PlannedWorkout[], date: string): PlannedWorkout[] =>
  planned.filter((entry) => entry.date === date);

/**
 * Which Intervals.icu number a training-settings sync takes the FTP from,
 * and which one it actually used. Mirrors `FtpSource` in intervals.rs.
 */
export type FtpSource = "indoorFtp" | "ftp" | "estimatedFtp";

export const ftpSourceLabel: Record<FtpSource, string> = {
  indoorFtp: "indoor FTP",
  ftp: "FTP",
  estimatedFtp: "eFTP",
};

/** Mirrors `IntervalsSyncSettings` in storage.rs. */
export type IntervalsSyncSettings = {
  calendar: boolean;
  library: boolean;
  ftpSource: FtpSource;
};

/** Mirrors `IntervalsStatus` in intervals_sync.rs. */
export type IntervalsStatus = {
  configured: boolean;
  athleteId: string | null;
  athleteName: string | null;
  settings: IntervalsSyncSettings;
  lastSyncedAt: string | null;
  lastError: string | null;
};

export const disconnectedIntervalsStatus: IntervalsStatus = {
  configured: false,
  athleteId: null,
  athleteName: null,
  settings: { calendar: true, library: true, ftpSource: "indoorFtp" },
  lastSyncedAt: null,
  lastError: null,
};

export type CalendarSyncReport = {
  fetched: number;
  unstructured: number;
  failed: number;
  skipped: number;
};

export type LibrarySyncReport = {
  added: number;
  updated: number;
  removed: number;
  unchanged: number;
  skipped: number;
  failed: string[];
};

export type SyncOutcome<T> =
  | { status: "off" }
  | { status: "done"; report: T }
  | { status: "failed"; error: string };

/** Mirrors `IntervalsSyncReport` in intervals_sync.rs. */
export type IntervalsSyncReport = {
  syncedAt: string;
  calendar: SyncOutcome<CalendarSyncReport>;
  library: SyncOutcome<LibrarySyncReport>;
};

const plural = (count: number, noun: string) => `${count} ${noun}${count === 1 ? "" : "s"}`;

/** One line per half of a mirror sync, for the Settings card. */
export const describeIntervalsSync = (report: IntervalsSyncReport): string => {
  const clauses: string[] = [];
  switch (report.calendar.status) {
    case "off":
      clauses.push("Calendar sync is off");
      break;
    case "failed":
      clauses.push(`Calendar: ${report.calendar.error}`);
      break;
    case "done": {
      const { fetched, unstructured, failed } = report.calendar.report;
      const details = [
        unstructured > 0 ? `${unstructured} without structure` : null,
        failed > 0 ? `${failed} unreadable` : null,
      ].filter((detail) => detail !== null);
      clauses.push(
        `${plural(fetched, "planned ride")} in the next 7 days${details.length ? ` (${details.join(", ")})` : ""}`,
      );
      break;
    }
  }
  switch (report.library.status) {
    case "off":
      clauses.push("Library sync is off");
      break;
    case "failed":
      clauses.push(`Library: ${report.library.error}`);
      break;
    case "done": {
      const { added, updated, removed, failed } = report.library.report;
      const changes = [
        added > 0 ? `${added} added` : null,
        updated > 0 ? `${updated} updated` : null,
        removed > 0 ? `${removed} removed` : null,
      ].filter((change) => change !== null);
      let line = changes.length ? `Library: ${changes.join(", ")}` : "Library up to date";
      if (failed.length > 0) line += ` (${plural(failed.length, "workout")} could not be read)`;
      clauses.push(line);
      break;
    }
  }
  return clauses.join(" · ");
};

/** "Synced just now" / "Synced 2 h ago" / "Never synced", for status lines. */
export const describeLastSynced = (lastSyncedAt: string | null, now = new Date()): string => {
  if (!lastSyncedAt) return "Never synced";
  const ageMs = Math.max(0, now.getTime() - new Date(lastSyncedAt).getTime());
  const minutes = Math.round(ageMs / 60_000);
  if (minutes < 1) return "Synced just now";
  if (minutes < 60) return `Synced ${plural(minutes, "minute")} ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return `Synced ${plural(hours, "hour")} ago`;
  return `Synced ${plural(Math.round(hours / 24), "day")} ago`;
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
  /** The zero this one replaced; missing on records written before drift was kept. */
  previousOffsetRaw?: number | null;
};

/** A device as it was during a ride, stored with the session. */
export type RideDevice = {
  role: DeviceRole;
  id: string;
  name: string;
  transport?: DeviceTransport;
  simulated: boolean;
  manufacturer: string | null;
  model: string | null;
  firmware: string | null;
  lastCalibration?: CalibrationRecord | null;
};

/** One device's own reading before fusion (the dual-power recording). */
export type SourceSample = {
  role: DeviceRole;
  timestampMs: number;
  powerWatts: number | null;
  cadenceRpm: number | null;
  balanceLeftPercent: number | null;
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
  /** Every power-capable device's fresh reading, on live samples only. */
  powerBySource?: Partial<Record<DeviceRole, number>>;
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
      /** Distance ridden so far, from the same estimator as the saved ride. */
      distanceMeters: number;
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
      distanceMeters: number;
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

export const defaultTrainingZoneSettings: TrainingZoneSettings = {
  version: 1,
  syncPowerZonesFromIntervals: false,
  syncHeartRateZonesFromIntervals: false,
  powerMode: "derived",
  powerZones: [],
  heartRateMode: "derived",
  heartRateZones: [],
};


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
  ftp: { watts: number; previousWatts: number; source: FtpSource };
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
  const ftpSource = ftpSourceLabel[result.ftp.source];
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

/**
 * Visits every gap-valid step between samples: the sample the step starts at
 * and how long it lasts. Steps longer than `maxGapMs` are skipped, so a
 * dropout is billed as neither work nor riding time. This gap rule is shared
 * by everything that totals or averages a ride, including `timeInZones`.
 */
const eachStep = (
  samples: readonly Pick<Telemetry, "timestampMs">[],
  maxGapMs: number,
  visit: (index: number, durationMs: number) => void,
): void => {
  for (let index = 0; index < samples.length - 1; index += 1) {
    const durationMs = samples[index + 1].timestampMs - samples[index].timestampMs;
    if (durationMs <= 0 || durationMs > maxGapMs) continue;
    visit(index, durationMs);
  }
};

/** Mechanical work in kilojoules over a sample series. */
export const workKilojoules = (
  samples: readonly Pick<Telemetry, "timestampMs" | "powerWatts">[],
  maxGapMs = 5_000,
): number => {
  let joules = 0;
  eachStep(samples, maxGapMs, (index, durationMs) => {
    joules += (samples[index].powerWatts * durationMs) / 1000;
  });
  return joules / 1000;
};

/**
 * Mean of `value` weighted by the time each sample was on screen, or null when
 * no step carried a value. Weighted by time rather than sample count so an
 * uneven sample rate cannot skew it.
 */
export const timeWeightedMean = <T extends Pick<Telemetry, "timestampMs">>(
  samples: readonly T[],
  value: (sample: T) => number | null,
  maxGapMs = 5_000,
): number | null => {
  let total = 0;
  let weightMs = 0;
  eachStep(samples, maxGapMs, (index, durationMs) => {
    const sampled = value(samples[index]);
    if (sampled === null) return;
    total += sampled * durationMs;
    weightMs += durationMs;
  });
  return weightMs === 0 ? null : total / weightMs;
};

/** Time-weighted mean power over a sample series, or null before there is any. */
export const averagePowerWatts = (
  samples: readonly Pick<Telemetry, "timestampMs" | "powerWatts">[],
  maxGapMs = 5_000,
): number | null => timeWeightedMean(samples, (sample) => sample.powerWatts, maxGapMs);

/** Largest reading of `value` in the series, or null when nothing was read. */
export const peak = <T>(samples: readonly T[], value: (sample: T) => number | null): number | null => {
  let highest: number | null = null;
  for (const sample of samples) {
    const sampled = value(sample);
    if (sampled === null) continue;
    highest = highest === null ? sampled : Math.max(highest, sampled);
  }
  return highest;
};

/**
 * Gross mechanical efficiency of a cyclist: the share of the energy burned
 * that reaches the pedals. Measured values sit near a quarter, which is why
 * kilojoules of work and dietary calories burned come out close to 1:1.
 */
export const GROSS_EFFICIENCY = 0.24;

const KILOJOULES_PER_KILOCALORIE = 4.184;

/** Dietary calories (kcal) burned to produce `kilojoules` of work at the pedals. */
export const kilojoulesToKilocalories = (kilojoules: number): number =>
  kilojoules / (KILOJOULES_PER_KILOCALORIE * GROSS_EFFICIENCY);

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
  | { kind: "bias"; delta: number }
  | { kind: "screen"; delta: number }
  | { kind: "pause" }
  | { kind: "skip" }
  | { kind: "end" };

/**
 * Keyboard shortcuts on the live ride view: ↑/↓ nudge the target by 5 W,
 * Shift+↑/↓ nudge the workout bias by 1 %, ←/→ page between ride screens,
 * Space pauses/resumes, S skips the current workout block, and Escape
 * brings up the end-ride confirmation. Held keys do not repeat.
 */
export const rideKeyAction = (
  key: string,
  shift: boolean,
  repeat: boolean,
): RideKeyAction | null => {
  if (!repeat) {
    if (key === "ArrowLeft" || key === "ArrowRight") {
      return { kind: "screen", delta: key === "ArrowLeft" ? -1 : 1 };
    }
    if (key === " ") return { kind: "pause" };
    if (key === "s" || key === "S") return { kind: "skip" };
    if (key === "Escape") return { kind: "end" };
  }
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
