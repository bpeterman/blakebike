import type { DeviceRole, Metric, SourceChoice } from "./types";
import { deviceRoleLabel } from "./types";
import { metricLabel, metricRoles } from "./sourcePreferences";

export function SourceSelect({
  metric,
  choice,
  label = `${metricLabel[metric]} source`,
  autoLabel = "Auto",
  roleLabel = (role) => deviceRoleLabel[role],
  onChange,
}: {
  metric: Metric;
  choice: SourceChoice;
  label?: string;
  autoLabel?: string;
  roleLabel?: (role: DeviceRole) => string;
  onChange: (choice: SourceChoice) => void;
}) {
  const value = choice.mode === "auto" ? "auto" : choice.role;

  return (
    <label className="source-choice-select">
      <span className="sr-only">{label}</span>
      <select
        aria-label={label}
        value={value}
        onChange={(event) => {
          if (event.target.value === "auto") {
            onChange({ mode: "auto" });
            return;
          }
          // Resolve the option back to one of this metric's roles instead of trusting the raw string.
          const role = metricRoles[metric].find((candidate) => candidate === event.target.value);
          if (role) onChange({ mode: "role", role });
        }}
      >
        <option value="auto">{autoLabel}</option>
        {metricRoles[metric].map((role) => (
          <option key={role} value={role}>{roleLabel(role)}</option>
        ))}
      </select>
    </label>
  );
}
