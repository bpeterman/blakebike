export type Profile = {
  id: string;
  name: string;
  ftpWatts: number;
  maxPowerWatts: number;
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

export type Capability = "ftms" | "heartRate" | "cyclingPower" | "csc";

export type DeviceInfo = {
  id: string;
  name: string;
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
  manufacturer: string | null;
  model: string | null;
  firmware: string | null;
  connectedSinceMs: number | null;
  drops: number;
  lastRawHex: string | null;
  /** Human summary of the latest decoded reading, e.g. "215 W · 88 rpm". */
  lastReading: string | null;
};

export type DeviceSlot = {
  role: DeviceRole;
  state: DeviceState;
  stats: SlotStats;
  log: DeviceLogLine[];
};

export type KnownDevice = {
  id: string;
  name: string;
  role: DeviceRole;
  capabilities: Capability[];
  simulated: boolean;
  manufacturer: string | null;
  model: string | null;
  /** RFC 3339 timestamp of the last successful connection. */
  lastConnectedAt: string;
};

export type SourceChoice = { mode: "auto" } | { mode: "role"; role: DeviceRole };

export type Metric = "power" | "cadence" | "heartRate";

export type SourcePreferences = Record<Metric, SourceChoice>;

export type TelemetrySource = { role: DeviceRole; fallback: boolean };

/** Which device supplied each fused telemetry value. */
export type TelemetrySources = Record<Metric, TelemetrySource | null>;

export type DevicesSnapshot = {
  scanning: boolean;
  scanError: { message: string; guidance: string } | null;
  slots: DeviceSlot[];
  sourcePreferences: SourcePreferences;
  sources: TelemetrySources;
};

export type ConnectProgress = {
  step: string;
  detail: string | null;
  level: "info" | "ok" | "warn";
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

export type RunnerState =
  | { status: "idle" }
  | { status: "countdown"; seconds: number; workoutName: string }
  | {
      status: "running";
      sessionId: string;
      workoutName: string;
      elapsedSeconds: number;
      totalSeconds: number | null;
      intervalIndex: number;
      intervalElapsedSeconds: number;
      targetPowerWatts: number | null;
      manualErg: boolean;
    }
  | {
      status: "paused";
      sessionId: string;
      workoutName: string;
      elapsedSeconds: number;
      totalSeconds: number | null;
      intervalIndex: number;
      targetPowerWatts: number | null;
      manualErg: boolean;
    }
  | { status: "finished"; sessionId: string; completed: boolean }
  | { status: "error"; message: string };

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
  completed: boolean;
};

export type SessionDetail = {
  summary: SessionSummary;
  samples: Telemetry[];
};

export const workoutDuration = (steps: WorkoutStep[]): number =>
  steps.reduce((total, step) => {
    if (step.kind === "repeat") {
      return total + step.repetitions * workoutDuration(step.steps);
    }
    return total + step.durationSeconds;
  }, 0);

export const formatDuration = (seconds: number): string => {
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const remaining = seconds % 60;
  return hours > 0
    ? `${hours}:${String(minutes).padStart(2, "0")}:${String(remaining).padStart(2, "0")}`
    : `${minutes}:${String(remaining).padStart(2, "0")}`;
};

export const manualPowerDeltaForKey = (
  key: string,
  repeat: boolean,
): number | null => {
  if (repeat) return null;
  if (key === "ArrowUp") return 5;
  if (key === "ArrowDown") return -5;
  return null;
};
