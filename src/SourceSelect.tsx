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
        onChange={(event) =>
          onChange(
            event.target.value === "auto"
              ? { mode: "auto" }
              : { mode: "role", role: event.target.value as DeviceRole },
          )
        }
      >
        <option value="auto">{autoLabel}</option>
        {metricRoles[metric].map((role) => (
          <option key={role} value={role}>{roleLabel(role)}</option>
        ))}
      </select>
    </label>
  );
}
