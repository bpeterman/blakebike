/**
 * Colours for the shared chart hover readout (ChartTooltip). Text colours are
 * held to WCAG AA (4.5:1) against `background` by chartTooltipTheme.test.ts.
 * The background sits a step above the modal and card surfaces so the tooltip
 * never blends into the chart behind it.
 */
export const chartTooltipColors = {
  background: "#262b24",
  border: "#4a5246",
  /** Values. */
  text: "#f4f6f1",
  /** Titles and series names. */
  muted: "#c9d0c3",
} as const;

/** Hairline that follows the pointer across line and area charts. */
export const chartTooltipLineCursor = { stroke: "#6b7566", strokeWidth: 1 } as const;
/** Band behind the hovered bar on bar charts. */
export const chartTooltipBarCursor = { fill: "rgba(255,255,255,.06)" } as const;

/** WCAG 2.x contrast ratio between two `#rrggbb` colours, from 1 to 21. */
export function contrastRatio(a: string, b: string): number {
  const [lighter, darker] = [relativeLuminance(a), relativeLuminance(b)].sort((x, y) => y - x);
  return (lighter + 0.05) / (darker + 0.05);
}

function relativeLuminance(hex: string): number {
  const match = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(hex);
  if (!match) throw new Error(`Expected a #rrggbb colour, got ${hex}`);
  const [r, g, b] = match.slice(1).map((pair) => {
    const channel = parseInt(pair, 16) / 255;
    return channel <= 0.03928 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}
