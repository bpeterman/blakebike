import type {
  CalibrationRecord,
  Capability,
  DeviceInfo,
  DeviceLogLine,
  DeviceRole,
  DeviceSlot,
  DeviceState,
  KnownConnectOutcome,
  KnownDevice,
  TelemetrySources,
  DeviceTransport,
} from "./types";
import { deviceRoleLabel } from "./types";

/** What the calibration button and dialog call the procedure for a role. */
export const calibrationVerb: Record<DeviceRole, string> = {
  trainer: "Calibrate",
  power: "Zero offset",
  heartRate: "Calibrate",
  cadence: "Calibrate",
};

/** Why a role's calibration button is disabled, or null when it is usable. */
export function calibrationUnavailable(
  role: DeviceRole,
  state: DeviceState,
  stats: { calibrationSupported: boolean; calibrating: boolean },
  rideActive = false,
): string | null {
  if (state.status === "controlling") return "Calibration is unavailable during a workout";
  if (state.status !== "ready") return null;
  if (!stats.calibrationSupported) {
    return role === "power"
      ? "This power meter does not advertise offset compensation"
      : "This trainer does not advertise FTMS spin-down calibration";
  }
  if (rideActive) {
    return role === "power" ? "Zero offset is unavailable during a ride" : "Calibration is unavailable during a workout";
  }
  return null;
}

/** "Zeroed 2 h ago · offset 1023" / "Spin-down 3 days ago" for a card line. */
export function calibrationSummary(record: CalibrationRecord | null | undefined, now = Date.now()): string | null {
  if (!record) return null;
  const when = formatRelativeDate(record.at, now);
  if (record.kind === "zeroOffset") {
    return record.offsetRaw === null ? `Zeroed ${when}` : `Zeroed ${when} · offset ${record.offsetRaw}`;
  }
  return `Spin-down ${when}`;
}

export type Drift = {
  delta: number;
  /** Share of the previous value, absolute. */
  ratio: number;
  /** Small drift is normal; a large one points at a cleat, crank or battery problem. */
  tone: "steady" | "large";
};

/**
 * How far a new zero moved from the last one. Vendors treat a few percent as
 * ordinary temperature drift; beyond that they tell you to check the
 * hardware and zero again. The absolute value means nothing on its own.
 */
export function offsetDrift(offsetRaw: number, previousOffsetRaw: number | null | undefined): Drift | null {
  if (previousOffsetRaw === null || previousOffsetRaw === undefined) return null;
  const delta = offsetRaw - previousOffsetRaw;
  const ratio = previousOffsetRaw === 0 ? (delta === 0 ? 0 : Infinity) : Math.abs(delta / previousOffsetRaw);
  return { delta, ratio, tone: ratio > 0.02 ? "large" : "steady" };
}

export function driftLabel(drift: Drift): string {
  const sign = drift.delta > 0 ? "+" : drift.delta < 0 ? "−" : "±";
  return `${sign}${Math.abs(drift.delta)}`;
}

/** Whether a power meter should be nudged towards a zero before riding. */
export function zeroOffsetIsDue(record: CalibrationRecord | null | undefined, now = Date.now(), maxAgeMs = 24 * 3_600_000): boolean {
  if (!record) return true;
  const then = Date.parse(record.at);
  return Number.isNaN(then) || now - then > maxAgeMs;
}

/** Which advertised services qualify a device for a role (mirrors the backend). */
export const roleCapabilities: Record<DeviceRole, Capability[]> = {
  trainer: ["ftms"],
  heartRate: ["heartRate"],
  power: ["cyclingPower"],
  cadence: ["csc", "cyclingPower"],
};

export const capabilityLabel: Record<Capability, string> = {
  ftms: "FTMS",
  heartRate: "Heart rate",
  cyclingPower: "Power",
  csc: "Cadence",
};

export function deviceTransport(device: { transport?: DeviceTransport }): DeviceTransport {
  return device.transport ?? "ble";
}

export function transportLabel(device: { transport?: DeviceTransport }): string {
  return deviceTransport(device) === "ant" ? "ANT+" : "BLE";
}

export function deviceFitsRole(device: DeviceInfo, role: DeviceRole): boolean {
  const capabilities = device.capabilities ?? [];
  // Pre-hub scan results carried no capability list and were trainers only.
  if (capabilities.length === 0) return role === "trainer";
  return capabilities.some((capability) => roleCapabilities[role].includes(capability));
}

export function readoutMark(level: DeviceLogLine["level"]): string {
  switch (level) {
    case "ok":
      return "✓";
    case "warn":
      return "!";
    case "error":
      return "✕";
    default:
      return "›";
  }
}

export function isConnected(state: DeviceState | undefined): boolean {
  return state?.status === "ready" || state?.status === "controlling";
}

export function deviceName(state: DeviceState | undefined): string | null {
  return state && (state.status === "ready" || state.status === "controlling") ? state.device.name : null;
}

/** "via heart rate" / "fallback: trainer" badge text for a fused metric. */
export function sourceNote(source: { role: DeviceRole; fallback: boolean } | null | undefined): string | null {
  if (!source) return null;
  const label = deviceRoleLabel[source.role].toLowerCase();
  return source.fallback ? `fallback: ${label}` : `via ${label}`;
}

export type Metric = keyof TelemetrySources;

export function metricsFedBy(role: DeviceRole, sources: TelemetrySources | undefined): Metric[] {
  if (!sources) return [];
  return (["power", "cadence", "heartRate"] as Metric[]).filter((metric) => sources[metric]?.role === role);
}

/** A remembered device as a connect needs it; its current signal is unknown. */
export function knownToDeviceInfo(device: KnownDevice): DeviceInfo {
  return {
    id: device.id,
    name: device.name,
    transport: device.transport,
    simulated: device.simulated,
    rssi: null,
    capabilities: device.capabilities,
  };
}

/** "trainer, heart rate and power meter" */
function listInProse(items: string[]): string {
  if (items.length <= 1) return items.join("");
  return `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

/**
 * The calm one-liner shown after "Connect all" when at least one device did
 * not answer. Null when nothing failed: the ordinary success toast covers
 * that. Never quotes the underlying error; the device card's log has it.
 */
export function connectAllSummary(outcomes: KnownConnectOutcome[]): string | null {
  const failed = outcomes.filter((outcome) => outcome.status === "failed");
  if (failed.length === 0) return null;
  const connected = outcomes.filter((outcome) => outcome.status === "connected");
  const roleName = (outcome: KnownConnectOutcome) => deviceRoleLabel[outcome.role].toLowerCase();
  const sentence = (text: string) => `${text.charAt(0).toUpperCase()}${text.slice(1)}`;
  const parts: string[] = [];
  if (connected.length > 0) {
    parts.push(sentence(`${listInProse(connected.map(roleName))} connected.`));
  }
  const quiet = listInProse(failed.map((outcome) => `${roleName(outcome)} (${outcome.name})`));
  const one = failed.length === 1;
  parts.push(sentence(`${quiet} didn’t answer, probably asleep. Wake ${one ? "it" : "them"} and press Connect on ${one ? "its" : "their"} card.`));
  return parts.join(" ");
}

export function formatUptime(ms: number): string {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const rest = seconds % 60;
  if (hours > 0) return `${hours}h ${String(minutes).padStart(2, "0")}m`;
  if (minutes > 0) return `${minutes}m ${String(rest).padStart(2, "0")}s`;
  return `${rest}s`;
}

export function formatAge(ms: number): string {
  if (ms < 1000) return "now";
  if (ms < 60_000) return `${Math.floor(ms / 1000)}s ago`;
  return `${Math.floor(ms / 60_000)}m ago`;
}

/** "Wahoo · KICKR CORE" from whatever Device Information a device exposed. */
export function makeAndModel(manufacturer: string | null | undefined, model: string | null | undefined): string | null {
  const parts = [manufacturer, model].filter((part): part is string => !!part && part.trim().length > 0);
  if (parts.length === 0) return null;
  // Avoid "Wahoo · Wahoo KICKR" style repetition.
  if (parts.length === 2 && parts[1].toLowerCase().startsWith(parts[0].toLowerCase())) return parts[1];
  return parts.join(" · ");
}

export function formatRelativeDate(iso: string, now = Date.now()): string {
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return "unknown";
  const diff = now - then;
  if (diff < 60_000) return "just now";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} min ago`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} h ago`;
  const days = Math.floor(diff / 86_400_000);
  return days === 1 ? "yesterday" : `${days} days ago`;
}

export function formatClock(atMs: number): string {
  const date = new Date(atMs);
  return [date.getHours(), date.getMinutes(), date.getSeconds()]
    .map((part) => String(part).padStart(2, "0"))
    .join(":");
}

export function statusOf(state: DeviceState, reconnectAttempt: number): { text: string; tone: "online" | "busy" | "off" | "error" } {
  switch (state.status) {
    case "ready":
      return { text: "Connected", tone: "online" };
    case "controlling":
      return { text: "ERG control", tone: "online" };
    case "connecting":
      return { text: "Connecting…", tone: "busy" };
    case "reconnecting":
      return {
        text: reconnectAttempt > 0 ? `Link lost · reconnecting (attempt ${reconnectAttempt})` : "Link lost",
        tone: "error",
      };
    case "scanning":
      return { text: "Scanning…", tone: "busy" };
    case "error":
      return { text: "Error", tone: "error" };
    default:
      return { text: "Idle", tone: "off" };
  }
}

/** A slot with no device yet, so every role has a card even before a scan. */
export function emptySlot(role: DeviceRole): DeviceSlot {
  return {
    role,
    state: { status: "idle" },
    stats: {
      samples: 0,
      parseFailures: 0,
      lastSampleMs: null,
      rateHz: 0,
      rssi: null,
      batteryPercent: null,
      batteryStatus: null,
      batteryVoltage: null,
      manufacturer: null,
      model: null,
      firmware: null,
      connectedSinceMs: null,
      drops: 0,
      reconnectAttempt: 0,
      lastRawHex: null,
      lastReading: null,
      calibrationSupported: false,
      calibrating: false,
      calibrationRequested: false,
      lastCalibration: null,
    },
    log: [],
  };
}
