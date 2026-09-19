import type {
  Capability,
  DeviceInfo,
  DeviceLogLine,
  DeviceRole,
  DeviceState,
  TelemetrySources,
  DeviceTransport,
} from "./types";
import { deviceRoleLabel } from "./types";

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
