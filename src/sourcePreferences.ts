import type { DeviceRole, Metric, SourceChoice, SourcePreferences } from "./types";

export const metricLabel: Record<Metric, string> = {
  power: "Power",
  cadence: "Cadence",
  heartRate: "Heart rate",
};

/** Roles that can supply each metric, in the backend's default priority order. */
export const metricRoles: Record<Metric, DeviceRole[]> = {
  power: ["power", "trainer"],
  cadence: ["cadence", "power", "trainer"],
  heartRate: ["heartRate", "trainer"],
};

export const defaultSourcePreferences: SourcePreferences = {
  power: { mode: "auto" },
  cadence: { mode: "auto" },
  heartRate: { mode: "auto" },
};

export function withSourcePreference(
  preferences: SourcePreferences,
  metric: Metric,
  choice: SourceChoice,
): SourcePreferences {
  return { ...preferences, [metric]: choice };
}
