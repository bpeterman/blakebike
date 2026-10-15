import type { TooltipContentProps, TooltipPayloadEntry } from "recharts";
import { chartTooltipColors } from "./chartTooltipTheme";

/** One line of a chart tooltip: a series swatch, its name, and its value. */
export type ChartTooltipRow = { color: string; name: string; value: string };

type ChartLabel = string | number | undefined;

const surfaceStyle = {
  background: chartTooltipColors.background,
  borderColor: chartTooltipColors.border,
  color: chartTooltipColors.text,
} as const;
const mutedStyle = { color: chartTooltipColors.muted } as const;

/**
 * Hover readout shared by every Recharts chart. Pass it as an element to
 * `<Tooltip content={...} />`; Recharts fills in `active`, `payload`, and
 * `label`.
 *
 * Text is always drawn in the high-contrast tooltip colours. A series' colour
 * only appears as a swatch, so a dim or saturated series colour never decides
 * whether the value can be read.
 */
export function ChartTooltip({
  active = false,
  payload = [],
  label,
  formatTitle = String,
  formatRow,
}: Partial<Pick<TooltipContentProps, "active" | "payload" | "label">> & {
  formatTitle?: (label: string | number) => string;
  formatRow: (entry: TooltipPayloadEntry, label: ChartLabel) => ChartTooltipRow;
}) {
  if (!active || payload.length === 0) return null;
  return (
    <div className="chart-tooltip" style={surfaceStyle}>
      {label !== undefined && <div className="chart-tooltip-title" style={mutedStyle}>{formatTitle(label)}</div>}
      {payload.map((entry) => {
        const row = formatRow(entry, label);
        return (
          <div className="chart-tooltip-row" key={entry.graphicalItemId}>
            <span className="chart-tooltip-swatch" style={{ background: row.color }} aria-hidden="true" />
            <span className="chart-tooltip-name" style={mutedStyle}>{row.name}</span>
            <span className="chart-tooltip-value">{row.value}</span>
          </div>
        );
      })}
    </div>
  );
}
