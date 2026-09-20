import { deviceRoleLabel, formatDuration } from "./types";
import {
  type Bin,
  type ComparedSource,
  type PowerComparison,
  type ReadyComparison,
  calibrationContext,
  describeLag,
  describeTrend,
  deviceDetail,
  deviceTitle,
  formatClockSeconds,
  formatSigned,
  formatWatts,
  headlineWindow,
} from "./dualPower";

/**
 * The trainer-versus-meter report as one drawing. The same `paintPowerComparison`
 * draws the canvas in the ride detail and the exported PNG, against a small
 * `Painter` so tests can read what was drawn without a 2D context.
 */

export type TextStyle = {
  size: number;
  weight?: number;
  color: string;
  align?: "left" | "center" | "right";
  baseline?: "top" | "middle" | "alphabetic";
};

export interface Painter {
  /** Called once with the finished size before anything is drawn. */
  begin(width: number, height: number): void;
  rect(x: number, y: number, width: number, height: number, fill: string, radius?: number): void;
  line(points: ReadonlyArray<readonly [number, number]>, stroke: string, width: number, dash?: readonly number[]): void;
  circle(x: number, y: number, radius: number, fill: string, stroke?: string, strokeWidth?: number): void;
  text(text: string, x: number, y: number, style: TextStyle): void;
  measureText(text: string, size: number, weight?: number): number;
}

/** Colours validated against the card surface for two series and dark text tokens. */
export const reportTheme = {
  page: "#10120f",
  panel: "#191c18",
  inset: "#12150f",
  border: "#262b24",
  text: "#f4f6f1",
  muted: "#8d9587",
  faint: "#5f675b",
  grid: "#242923",
  axis: "#3a4137",
  trainer: "#78a300",
  meter: "#3d8be0",
  mark: "#aeb7a8",
  markSoft: "rgba(174, 183, 168, 0.5)",
  accent: "#c8ff32",
  warmup: "rgba(200, 255, 50, 0.06)",
  font: 'Inter, ui-sans-serif, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif',
};

/** Logical width the exported image is laid out at. */
export const EXPORT_WIDTH = 1200;
/** Below this the two-up rows stack. */
const TWO_COLUMN_MIN_WIDTH = 900;
const PAD = 28;
const GAP = 16;

type Section = { height: number; draw: (y: number) => void };

export type PaintOptions = { width: number };

/** Draw the report at `width` logical pixels; returns the height it took. */
export function paintPowerComparison(painter: Painter, report: PowerComparison, options: PaintOptions): number {
  const width = Math.max(480, Math.round(options.width));
  const sections = report.status === "ready"
    ? planReady(painter, report, width)
    : planInsufficient(painter, report, width);
  const height = sections.reduce((total, section) => total + section.height, 0) + PAD;
  painter.begin(width, height);
  painter.rect(0, 0, width, height, reportTheme.page);
  let y = 0;
  for (const section of sections) {
    section.draw(y);
    y += section.height;
  }
  return height;
}

// ---- sections ---------------------------------------------------------------

function planReady(p: Painter, report: ReadyComparison, width: number): Section[] {
  const twoColumn = width >= TWO_COLUMN_MIN_WIDTH;
  const inner = width - PAD * 2;
  const half = (inner - GAP) / 2;
  const sections: Section[] = [
    header(p, report, width, `${report.workoutName} · ${formatDate(report.startedAt)} · ${formatDuration(report.elapsedSeconds)}`),
    headlineTiles(p, report, width),
    tracesChart(p, report, width),
    driftChart(p, report, width),
  ];
  const blandAltman = (x: number, w: number) => blandAltmanChart(p, report, x, w);
  const table = (x: number, w: number) => windowsTable(p, report, x, w);
  const power = (x: number, w: number) => binsChart(p, report.byPower, x, w, {
    title: "Difference by power level",
    subtitle: `${report.headlineWindowSeconds} s pairs · ${report.powerBinWatts} W bins · bins under ${report.binMinPairs} s hidden`,
    label: (bin) => `${bin.lower}–${bin.upper} W`,
    empty: "No power level was ridden long enough to bin.",
  });
  const cadence = (x: number, w: number) => binsChart(p, report.byCadence?.bins ?? [], x, w, {
    title: "Difference by cadence",
    subtitle: report.byCadence
      ? `${deviceRoleLabel[report.byCadence.source]}'s cadence · ${report.cadenceBinRpm} rpm bins · bins under ${report.binMinPairs} s hidden`
      : `${report.cadenceBinRpm} rpm bins`,
    label: (bin) => `${bin.lower}–${bin.upper} rpm`,
    empty: "Neither device reported cadence for at least half the compared time.",
  });
  const rightColumn = PAD + half + GAP;
  if (twoColumn) {
    sections.push(row([blandAltman(PAD, half), table(rightColumn, half)], 320));
    sections.push(row([power(PAD, half), cadence(rightColumn, half)], 250));
  } else {
    sections.push(row([blandAltman(PAD, inner)], 320));
    sections.push(row([table(PAD, inner)], 250));
    sections.push(row([power(PAD, inner)], 250));
    sections.push(row([cadence(PAD, inner)], 250));
  }
  sections.push(footer(p, report, width, readyFooterLines(report)));
  return sections;
}

function planInsufficient(p: Painter, report: Extract<PowerComparison, { status: "insufficient" }>, width: number): Section[] {
  const inner = width - PAD * 2;
  const lines = wrap(p, report.reason, 15, inner - 32);
  const coverage = wrap(
    p,
    `Both devices recorded for ${formatClockSeconds(report.coverage.overlapSeconds)} together, from ${formatClockSeconds(report.coverage.overlapStartSeconds)} to ${formatClockSeconds(report.coverage.overlapEndSeconds)} of a ${formatClockSeconds(report.coverage.rideSeconds)} ride. At least ${formatClockSeconds(report.minOverlapSeconds)} of overlap is needed for a comparison.`,
    12,
    inner - 32,
  );
  const note: Section = {
    height: 40 + lines.length * 22 + coverage.length * 18 + 24,
    draw: (y) => {
      p.rect(PAD, y, inner, 24 + lines.length * 22 + coverage.length * 18 + 16, reportTheme.panel, 12);
      let ty = y + 20;
      p.text("NOT ENOUGH OVERLAP", PAD + 16, ty, label());
      ty += 20;
      for (const line of lines) {
        p.text(line, PAD + 16, ty, { size: 15, weight: 600, color: reportTheme.text, baseline: "top" });
        ty += 22;
      }
      for (const line of coverage) {
        p.text(line, PAD + 16, ty, { size: 12, color: reportTheme.muted, baseline: "top" });
        ty += 18;
      }
    },
  };
  return [
    header(p, report, width, "Ride recorded both power devices"),
    note,
    footer(p, report, width, [report.signConvention, "Neither device is treated as the reference."]),
  ];
}

function header(p: Painter, report: PowerComparison, width: number, subtitle: string): Section {
  const inner = width - PAD * 2;
  const half = (inner - GAP) / 2;
  const panelHeight = 96;
  return {
    height: PAD + 30 + 24 + 12 + panelHeight,
    draw: (y) => {
      const top = y + PAD;
      p.text("Trainer vs power meter", PAD, top, { size: 26, weight: 750, color: reportTheme.text, baseline: "top" });
      p.text("blake.bike", width - PAD, top + 4, { size: 15, weight: 750, color: reportTheme.accent, align: "right", baseline: "top" });
      p.text(subtitle, PAD, top + 34, { size: 13, color: reportTheme.muted, baseline: "top" });
      const panelTop = top + 66;
      devicePanel(p, report.a, "A · TRAINER", reportTheme.trainer, PAD, panelTop, half, panelHeight);
      devicePanel(p, report.b, "B · POWER METER", reportTheme.meter, PAD + half + GAP, panelTop, half, panelHeight);
    },
  };
}

function devicePanel(p: Painter, source: ComparedSource, caption: string, color: string, x: number, y: number, w: number, h: number) {
  p.rect(x, y, w, h, reportTheme.panel, 12);
  p.circle(x + 20, y + 21, 5, color);
  p.text(caption, x + 32, y + 15, label());
  p.text(ellipsize(p, deviceTitle(source.device, deviceRoleLabel[source.role]), 17, 700, w - 32), x + 16, y + 34, {
    size: 17, weight: 700, color: reportTheme.text, baseline: "top",
  });
  p.text(ellipsize(p, deviceDetail(source.device), 12, 400, w - 32), x + 16, y + 57, { size: 12, color: reportTheme.muted, baseline: "top" });
  const stats = [
    `${source.rateHz.toFixed(1)} Hz`,
    `${source.readings.toLocaleString()} readings`,
    `mean ${formatWatts(source.meanWatts)}`,
    `max ${formatWatts(source.maxWatts)}`,
  ].join(" · ");
  p.text(ellipsize(p, stats, 11, 400, w - 32), x + 16, y + 75, { size: 11, color: reportTheme.faint, baseline: "top" });
}

function headlineTiles(p: Painter, report: ReadyComparison, width: number): Section {
  const inner = width - PAD * 2;
  const stats = headlineWindow(report);
  const tiles: Array<[string, string, string]> = [
    [`MEAN OFFSET · ${stats.windowSeconds} s`, formatSigned(stats.meanWatts, 1, "W"), `${formatSigned(stats.meanPercent, 1, "%")} · trainer − meter`],
    ["SPREAD · SD", `±${stats.sdWatts.toFixed(1)} W`, `95% of pairs within ${formatSigned(stats.loaLowWatts, 0, "")}…${formatSigned(stats.loaHighWatts, 0, "W")}`],
    ["P95 · MAX |Δ|", `${stats.p95AbsWatts.toFixed(0)} W · ${stats.maxAbsWatts.toFixed(0)} W`, `${stats.p95AbsPercent.toFixed(1)}% · ${stats.maxAbsPercent.toFixed(1)}%`],
    ["LAG", report.lag.confident ? `${Math.abs(report.lag.seconds).toFixed(2)} s` : "n/a", report.lag.confident ? (report.lag.seconds >= 0 ? "trainer trails the meter" : "meter trails the trainer") : "not enough variation"],
    ["COMPARED", formatClockSeconds(report.coverage.comparedSeconds), `${formatClockSeconds(report.coverage.excludedSeconds)} excluded of ${formatClockSeconds(report.coverage.overlapSeconds)}`],
  ];
  const columns = width >= TWO_COLUMN_MIN_WIDTH ? 5 : 3;
  const rows = Math.ceil(tiles.length / columns);
  const tileHeight = 92;
  const tileWidth = (inner - GAP * (columns - 1)) / columns;
  return {
    height: GAP + rows * (tileHeight + GAP) - GAP + GAP,
    draw: (y) => {
      tiles.forEach(([caption, value, note], index) => {
        const x = PAD + (index % columns) * (tileWidth + GAP);
        const ty = y + GAP + Math.floor(index / columns) * (tileHeight + GAP);
        p.rect(x, ty, tileWidth, tileHeight, reportTheme.panel, 12);
        p.text(caption, x + 14, ty + 14, label());
        p.text(ellipsize(p, value, 22, 750, tileWidth - 28), x + 14, ty + 32, { size: 22, weight: 750, color: reportTheme.text, baseline: "top" });
        p.text(ellipsize(p, note, 11, 400, tileWidth - 28), x + 14, ty + 64, { size: 11, color: reportTheme.muted, baseline: "top" });
      });
    },
  };
}

// ---- charts ----------------------------------------------------------------

type Plot = { x: number; y: number; w: number; h: number };

function panelWithTitle(p: Painter, x: number, y: number, w: number, h: number, title: string, subtitle: string): Plot {
  p.rect(x, y, w, h, reportTheme.panel, 12);
  p.text(title, x + 16, y + 16, { size: 14, weight: 700, color: reportTheme.text, baseline: "top" });
  p.text(ellipsize(p, subtitle, 11, 400, w - 32), x + 16, y + 36, { size: 11, color: reportTheme.muted, baseline: "top" });
  return { x: x + 16 + 44, y: y + 62, w: w - 32 - 44, h: h - 62 - 40 };
}

function tracesChart(p: Painter, report: ReadyComparison, width: number): Section {
  const inner = width - PAD * 2;
  const height = 300;
  return {
    height: height + GAP,
    draw: (y) => {
      const top = y + GAP;
      const plot = panelWithTitle(p, PAD, top, inner, height, `Both devices, ${report.headlineWindowSeconds} s smoothed`, "Elapsed time from ride start · the whole overlap, excluded seconds included");
      const points = report.traces;
      const values = points.flatMap((point) => [point.a, point.b]).filter((v): v is number => v !== null);
      if (points.length < 2 || values.length === 0) {
        p.text("Not enough data to draw.", plot.x, plot.y + plot.h / 2, { size: 12, color: reportTheme.muted });
        return;
      }
      const xDomain: [number, number] = [points[0].elapsedSeconds, points[points.length - 1].elapsedSeconds];
      const yTicks = niceTicks(0, Math.max(...values) * 1.05, 5);
      const yDomain: [number, number] = [yTicks[0], yTicks[yTicks.length - 1]];
      const sx = linear(xDomain, [plot.x, plot.x + plot.w]);
      const sy = linear(yDomain, [plot.y + plot.h, plot.y]);
      gridAndAxes(p, plot, timeTicks(xDomain), yTicks, sx, sy, formatClockSeconds, (v) => `${v} W`);
      series(p, points.map((pt) => [pt.elapsedSeconds, pt.a] as const), sx, sy, reportTheme.trainer);
      series(p, points.map((pt) => [pt.elapsedSeconds, pt.b] as const), sx, sy, reportTheme.meter);
      legend(p, plot.x + plot.w, top + 16, [
        [deviceTitle(report.a.device, "Trainer"), reportTheme.trainer],
        [deviceTitle(report.b.device, "Power meter"), reportTheme.meter],
      ]);
    },
  };
}

function driftChart(p: Painter, report: ReadyComparison, width: number): Section {
  const inner = width - PAD * 2;
  const height = 260;
  return {
    height: height + GAP,
    draw: (y) => {
      const top = y + GAP;
      const warmup = report.warmup;
      const subtitle = warmup
        ? `Mean difference per elapsed minute · first ${warmup.minutes} min ${formatSigned(warmup.firstMeanPercent, 1, "%")}, after that ${formatSigned(warmup.restMeanPercent, 1, "%")} · time, not temperature, is the axis`
        : `Mean difference per elapsed minute · minutes with under ${report.minuteBinMinPairs} compared seconds hidden · time, not temperature, is the axis`;
      const plot = panelWithTitle(p, PAD, top, inner, height, "Drift as the ride goes on", subtitle);
      const bins = report.byMinute;
      if (bins.length === 0) {
        p.text("No minute had enough compared seconds.", plot.x, plot.y + plot.h / 2, { size: 12, color: reportTheme.muted });
        return;
      }
      const xMax = Math.max(bins[bins.length - 1].upper, report.elapsedSeconds / 60);
      const extent = Math.max(1, ...bins.map((bin) => Math.abs(bin.meanPercent))) * 1.15;
      const yTicks = niceTicks(-extent, extent, 5);
      const sx = linear([0, xMax], [plot.x, plot.x + plot.w]);
      const sy = linear([yTicks[0], yTicks[yTicks.length - 1]], [plot.y + plot.h, plot.y]);
      if (warmup) {
        const to = Math.min(warmup.minutes, xMax);
        p.rect(sx(0), plot.y, sx(to) - sx(0), plot.h, reportTheme.warmup);
        p.text(`first ${warmup.minutes} min`, sx(to) - 6, plot.y + 6, { size: 10, color: reportTheme.faint, align: "right", baseline: "top" });
      }
      gridAndAxes(p, plot, niceTicks(0, xMax, 6), yTicks, sx, sy, (v) => `${v} min`, (v) => formatSigned(v, 0, "%"));
      p.line([[plot.x, sy(0)], [plot.x + plot.w, sy(0)]], reportTheme.muted, 1);
      const points = bins.map((bin) => [(bin.lower + bin.upper) / 2, bin.meanPercent] as const);
      series(p, points, sx, sy, reportTheme.mark);
      for (const [x, v] of points) p.circle(sx(x), sy(v), 3.5, reportTheme.mark, reportTheme.panel, 1.5);
    },
  };
}

function blandAltmanChart(p: Painter, report: ReadyComparison, x: number, w: number) {
  return (y: number, h: number) => {
    const stats = headlineWindow(report);
    const plot = panelWithTitle(p, x, y, w, h, "Bland-Altman", `Difference against the mean of both · ${stats.count.toLocaleString()} pairs · solid mean, dashed 95% limits, dotted trend`);
    const points = report.blandAltman;
    if (points.length === 0) {
      p.text("No compared pairs.", plot.x, plot.y + plot.h / 2, { size: 12, color: reportTheme.muted });
      return;
    }
    const xs = points.map((pt) => pt[0]);
    const ys = points.map((pt) => pt[1]);
    const xTicks = niceTicks(Math.min(...xs), Math.max(...xs), 5);
    const yExtent = Math.max(Math.abs(stats.loaLowWatts), Math.abs(stats.loaHighWatts), ...ys.map(Math.abs)) * 1.1;
    const yTicks = niceTicks(-yExtent, yExtent, 5);
    const sx = linear([xTicks[0], xTicks[xTicks.length - 1]], [plot.x, plot.x + plot.w]);
    const sy = linear([yTicks[0], yTicks[yTicks.length - 1]], [plot.y + plot.h, plot.y]);
    gridAndAxes(p, plot, xTicks, yTicks, sx, sy, (v) => `${v} W`, (v) => formatSigned(v, 0, "W"));
    p.line([[plot.x, sy(0)], [plot.x + plot.w, sy(0)]], reportTheme.axis, 1);
    for (const [mean, diff] of points) p.circle(sx(mean), sy(diff), 2.2, reportTheme.markSoft);
    const right = plot.x + plot.w;
    const lineAt = (value: number, dash?: readonly number[]) => p.line([[plot.x, sy(value)], [right, sy(value)]], reportTheme.text, 1.25, dash);
    lineAt(stats.meanWatts);
    lineAt(stats.loaLowWatts, [5, 4]);
    lineAt(stats.loaHighWatts, [5, 4]);
    const x0 = xTicks[0];
    const x1 = xTicks[xTicks.length - 1];
    const trendAt = (mean: number) => report.trend.interceptWatts + (report.trend.slopePercent / 100) * mean;
    p.line([[sx(x0), sy(clamp(trendAt(x0), yTicks[0], yTicks[yTicks.length - 1]))], [sx(x1), sy(clamp(trendAt(x1), yTicks[0], yTicks[yTicks.length - 1]))]], reportTheme.muted, 1.25, [2, 3]);
    p.text(formatSigned(stats.meanWatts, 1, "W"), right - 4, sy(stats.meanWatts) - 4, { size: 10, color: reportTheme.text, align: "right" });
  };
}

function windowsTable(p: Painter, report: ReadyComparison, x: number, w: number) {
  return (y: number, h: number) => {
    p.rect(x, y, w, h, reportTheme.panel, 12);
    p.text("By smoothing window", x + 16, y + 16, { size: 14, weight: 700, color: reportTheme.text, baseline: "top" });
    p.text("1 s is the noise floor · 3 s the headline · 30 s what a ride summary sees", x + 16, y + 36, { size: 11, color: reportTheme.muted, baseline: "top" });
    // Weighted columns: the mean carries watts and percent, the window just a number.
    const columns: Array<[string, number]> = [["Window", 0.7], ["Mean", 1.7], ["SD", 0.8], ["95% limits", 1.3], ["P95 |Δ|", 0.8], ["Max |Δ|", 0.8]];
    const unit = (w - 32) / columns.reduce((total, [, weight]) => total + weight, 0);
    const columnX = columns.map((_, index) => x + 16 + columns.slice(0, index).reduce((total, [, weight]) => total + weight * unit, 0));
    const columnWidth = columns.map(([, weight]) => weight * unit);
    let ty = y + 68;
    columns.forEach(([column], index) => {
      p.text(column.toUpperCase(), columnX[index], ty, label());
    });
    ty += 22;
    for (const stats of report.windows) {
      const cells = [
        `${stats.windowSeconds} s`,
        `${formatSigned(stats.meanWatts, 1, "W")} · ${formatSigned(stats.meanPercent, 1, "%")}`,
        `${stats.sdWatts.toFixed(1)} W`,
        `${formatSigned(stats.loaLowWatts, 0, "")}…${formatSigned(stats.loaHighWatts, 0, "W")}`,
        `${stats.p95AbsWatts.toFixed(0)} W`,
        `${stats.maxAbsWatts.toFixed(0)} W`,
      ];
      cells.forEach((cell, index) => {
        p.text(ellipsize(p, cell, 12, index === 0 ? 700 : 400, columnWidth[index] - 8), columnX[index], ty, {
          size: 12, weight: index === 0 ? 700 : 400, color: reportTheme.text, baseline: "top",
        });
      });
      p.line([[x + 16, ty + 24], [x + w - 16, ty + 24]], reportTheme.border, 1);
      ty += 32;
    }
    ty += 6;
    for (const line of wrap(p, `Fit over mean power: ${describeTrend(report.trend)}. A flat offset points at a zero or calibration; a change with power at a scale error.`, 11, w - 32)) {
      if (ty > y + h - 16) break;
      p.text(line, x + 16, ty, { size: 11, color: reportTheme.muted, baseline: "top" });
      ty += 16;
    }
  };
}

function binsChart(p: Painter, bins: Bin[], x: number, w: number, options: {
  title: string; subtitle: string; label: (bin: Bin) => string; empty: string;
}) {
  return (y: number, h: number) => {
    const plot = panelWithTitle(p, x, y, w, h, options.title, options.subtitle);
    if (bins.length === 0) {
      for (const [index, line] of wrap(p, options.empty, 12, plot.w).entries()) {
        p.text(line, plot.x, plot.y + plot.h / 2 + index * 18, { size: 12, color: reportTheme.muted });
      }
      return;
    }
    const extent = Math.max(1, ...bins.map((bin) => Math.abs(bin.meanPercent))) * 1.3;
    const yTicks = niceTicks(-extent, extent, 5);
    const sy = linear([yTicks[0], yTicks[yTicks.length - 1]], [plot.y + plot.h, plot.y]);
    const slot = plot.w / bins.length;
    const barWidth = Math.min(48, slot * 0.6);
    for (const tick of yTicks) {
      p.line([[plot.x, sy(tick)], [plot.x + plot.w, sy(tick)]], reportTheme.grid, 1);
      p.text(formatSigned(tick, 0, "%"), plot.x - 8, sy(tick), { size: 10, color: reportTheme.faint, align: "right", baseline: "middle" });
    }
    p.line([[plot.x, sy(0)], [plot.x + plot.w, sy(0)]], reportTheme.muted, 1);
    bins.forEach((bin, index) => {
      const cx = plot.x + slot * index + slot / 2;
      const top = Math.min(sy(0), sy(bin.meanPercent));
      const height = Math.max(1, Math.abs(sy(bin.meanPercent) - sy(0)));
      p.rect(cx - barWidth / 2, top, barWidth, height, reportTheme.mark, 3);
      const labelY = bin.meanPercent >= 0 ? top - 4 : top + height + 12;
      p.text(formatSigned(bin.meanPercent, 1, "%"), cx, labelY, { size: 10, weight: 600, color: reportTheme.text, align: "center" });
      p.text(options.label(bin), cx, plot.y + plot.h + 14, { size: 10, color: reportTheme.muted, align: "center" });
      p.text(`${bin.count} s`, cx, plot.y + plot.h + 27, { size: 9, color: reportTheme.faint, align: "center" });
    });
  };
}

/** One or more panels drawn side by side at the same height. */
function row(panels: Array<(y: number, h: number) => void>, height: number): Section {
  return {
    height: height + GAP,
    draw: (y) => {
      for (const panel of panels) panel(y + GAP, height);
    },
  };
}

function readyFooterLines(report: ReadyComparison): string[] {
  const { coverage, exclusion } = report;
  const lines = [
    report.signConvention,
    `Excluded: seconds where either device read under ${exclusion.minWatts} W (coasting) or moved more than ${exclusion.transientWattsPerSecond} W in a second (a power step), plus ${exclusion.guardSeconds} s on each side. ${formatClockSeconds(coverage.excludedSeconds)} excluded (${formatClockSeconds(coverage.coastingSeconds)} coasting, ${formatClockSeconds(coverage.transientSeconds)} steps); ${formatClockSeconds(coverage.comparedSeconds)} compared.`,
    `Both devices recorded together from ${formatClockSeconds(coverage.overlapStartSeconds)} to ${formatClockSeconds(coverage.overlapEndSeconds)} of a ${formatClockSeconds(coverage.rideSeconds)} ride. Both were resampled to 1 Hz; percentages are the ratio of sums over the compared seconds.`,
    describeLag(report.lag),
  ];
  const calibration = calibrationContext(report.b.device?.lastCalibration, report.startedAt);
  if (calibration) lines.push(calibration);
  lines.push("Neither device is treated as the reference. Timestamps are Bluetooth arrival times; the axis is elapsed time, not temperature.");
  return lines;
}

function footer(p: Painter, _report: PowerComparison, width: number, lines: string[]): Section {
  const inner = width - PAD * 2;
  const wrapped = lines.flatMap((line) => wrap(p, line, 11, inner - 32));
  const height = 16 + wrapped.length * 16 + 34;
  return {
    height: height + GAP,
    draw: (y) => {
      const top = y + GAP;
      p.rect(PAD, top, inner, height, reportTheme.inset, 12);
      let ty = top + 14;
      for (const line of wrapped) {
        p.text(line, PAD + 16, ty, { size: 11, color: reportTheme.muted, baseline: "top" });
        ty += 16;
      }
      p.text("Made with blake.bike · trainer versus power meter comparison", PAD + 16, top + height - 14, { size: 10, color: reportTheme.faint, baseline: "middle" });
      p.text("blake.bike", PAD + inner - 16, top + height - 14, { size: 12, weight: 750, color: reportTheme.accent, align: "right", baseline: "middle" });
    },
  };
}

// ---- drawing helpers ------------------------------------------------------------

function label(): TextStyle {
  return { size: 10, weight: 700, color: reportTheme.muted, baseline: "top" };
}

function linear(domain: readonly [number, number], range: readonly [number, number]): (value: number) => number {
  const span = domain[1] - domain[0] || 1;
  return (value) => range[0] + ((value - domain[0]) / span) * (range[1] - range[0]);
}

function clamp(value: number, low: number, high: number): number {
  return Math.min(high, Math.max(low, value));
}

/** Tick values on a nice step that cover [min, max]. */
export function niceTicks(min: number, max: number, count: number): number[] {
  if (!Number.isFinite(min) || !Number.isFinite(max)) return [0, 1];
  if (max <= min) max = min + 1;
  const rough = (max - min) / Math.max(1, count);
  const magnitude = 10 ** Math.floor(Math.log10(rough));
  const residual = rough / magnitude;
  const step = (residual >= 5 ? 10 : residual >= 2 ? 5 : residual >= 1 ? 2 : 1) * magnitude;
  const start = Math.floor(min / step) * step;
  const end = Math.ceil(max / step) * step;
  const ticks: number[] = [];
  for (let value = start; value <= end + step / 2; value += step) ticks.push(Number(value.toFixed(6)));
  return ticks;
}

/** Elapsed-time ticks on a round step, about six across the range. */
export function timeTicks([from, to]: readonly [number, number]): number[] {
  const steps = [15, 30, 60, 120, 300, 600, 900, 1200, 1800, 3600];
  const step = steps.find((candidate) => (to - from) / candidate <= 7) ?? 7200;
  const ticks: number[] = [];
  for (let value = Math.ceil(from / step) * step; value <= to; value += step) ticks.push(value);
  return ticks;
}

function gridAndAxes(
  p: Painter, plot: Plot, xTicks: number[], yTicks: number[],
  sx: (v: number) => number, sy: (v: number) => number,
  formatX: (v: number) => string, formatY: (v: number) => string,
) {
  for (const tick of yTicks) {
    p.line([[plot.x, sy(tick)], [plot.x + plot.w, sy(tick)]], reportTheme.grid, 1);
    p.text(formatY(tick), plot.x - 8, sy(tick), { size: 10, color: reportTheme.faint, align: "right", baseline: "middle" });
  }
  p.line([[plot.x, plot.y + plot.h], [plot.x + plot.w, plot.y + plot.h]], reportTheme.axis, 1);
  for (const tick of xTicks) {
    p.text(formatX(tick), sx(tick), plot.y + plot.h + 16, { size: 10, color: reportTheme.faint, align: "center" });
  }
}

/** A 2 px line that breaks at gaps. */
function series(p: Painter, points: ReadonlyArray<readonly [number, number | null]>, sx: (v: number) => number, sy: (v: number) => number, color: string) {
  let run: Array<[number, number]> = [];
  const flush = () => {
    if (run.length >= 2) p.line(run, color, 2);
    else if (run.length === 1) p.circle(run[0][0], run[0][1], 1.5, color);
    run = [];
  };
  for (const [x, value] of points) {
    if (value === null) flush();
    else run.push([sx(x), sy(value)]);
  }
  flush();
}

function legend(p: Painter, right: number, y: number, entries: Array<[string, string]>) {
  let x = right;
  for (const [text, color] of [...entries].reverse()) {
    const width = p.measureText(text, 11, 600);
    x -= width;
    p.text(text, x, y + 6, { size: 11, weight: 600, color: reportTheme.text, baseline: "middle" });
    x -= 14;
    p.circle(x + 4, y + 6, 4.5, color);
    x -= 18;
  }
}

/** Greedy word wrap using the painter's own measurements. */
export function wrap(p: Painter, text: string, size: number, maxWidth: number): string[] {
  const words = text.split(/\s+/).filter(Boolean);
  const lines: string[] = [];
  let current = "";
  for (const word of words) {
    const candidate = current ? `${current} ${word}` : word;
    if (current && p.measureText(candidate, size) > maxWidth) {
      lines.push(current);
      current = word;
    } else {
      current = candidate;
    }
  }
  if (current) lines.push(current);
  return lines;
}

function ellipsize(p: Painter, text: string, size: number, weight: number, maxWidth: number): string {
  if (p.measureText(text, size, weight) <= maxWidth) return text;
  let cut = text;
  while (cut.length > 1 && p.measureText(`${cut}…`, size, weight) > maxWidth) cut = cut.slice(0, -1);
  return `${cut.trimEnd()}…`;
}

function formatDate(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
}

// ---- painters -------------------------------------------------------------------

/** Draws onto a canvas at `scale` device pixels per logical pixel. */
export class CanvasPainter implements Painter {
  constructor(
    private readonly canvas: HTMLCanvasElement,
    private readonly context: CanvasRenderingContext2D,
    private readonly scale: number,
    private readonly cssWidth: number | null = null,
  ) {}

  begin(width: number, height: number): void {
    this.canvas.width = Math.round(width * this.scale);
    this.canvas.height = Math.round(height * this.scale);
    if (this.cssWidth !== null) {
      this.canvas.style.width = `${this.cssWidth}px`;
      this.canvas.style.height = `${(height * this.cssWidth) / width}px`;
    }
    this.context.setTransform(this.scale, 0, 0, this.scale, 0, 0);
  }

  rect(x: number, y: number, width: number, height: number, fill: string, radius = 0): void {
    const ctx = this.context;
    ctx.fillStyle = fill;
    if (radius > 0) {
      ctx.beginPath();
      roundedRectPath(ctx, x, y, width, height, radius);
      ctx.fill();
    } else {
      ctx.fillRect(x, y, width, height);
    }
  }

  line(points: ReadonlyArray<readonly [number, number]>, stroke: string, width: number, dash: readonly number[] = []): void {
    if (points.length < 2) return;
    const ctx = this.context;
    ctx.beginPath();
    ctx.moveTo(points[0][0], points[0][1]);
    for (let index = 1; index < points.length; index += 1) ctx.lineTo(points[index][0], points[index][1]);
    ctx.setLineDash([...dash]);
    ctx.lineJoin = "round";
    ctx.lineCap = "round";
    ctx.strokeStyle = stroke;
    ctx.lineWidth = width;
    ctx.stroke();
    ctx.setLineDash([]);
  }

  circle(x: number, y: number, radius: number, fill: string, stroke?: string, strokeWidth = 1): void {
    const ctx = this.context;
    ctx.beginPath();
    ctx.arc(x, y, radius, 0, Math.PI * 2);
    ctx.fillStyle = fill;
    ctx.fill();
    if (stroke) {
      ctx.strokeStyle = stroke;
      ctx.lineWidth = strokeWidth;
      ctx.stroke();
    }
  }

  text(text: string, x: number, y: number, style: TextStyle): void {
    const ctx = this.context;
    ctx.font = `${style.weight ?? 400} ${style.size}px ${reportTheme.font}`;
    ctx.fillStyle = style.color;
    ctx.textAlign = style.align ?? "left";
    ctx.textBaseline = style.baseline ?? "alphabetic";
    ctx.fillText(text, x, y);
  }

  measureText(text: string, size: number, weight = 400): number {
    this.context.font = `${weight} ${size}px ${reportTheme.font}`;
    return this.context.measureText(text).width;
  }
}

function roundedRectPath(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number) {
  const radius = Math.min(r, w / 2, h / 2);
  ctx.moveTo(x + radius, y);
  ctx.arcTo(x + w, y, x + w, y + h, radius);
  ctx.arcTo(x + w, y + h, x, y + h, radius);
  ctx.arcTo(x, y + h, x, y, radius);
  ctx.arcTo(x, y, x + w, y, radius);
  ctx.closePath();
}

/** Records what was drawn, for tests and for environments without a 2D context. */
export class RecordingPainter implements Painter {
  size: { width: number; height: number } | null = null;
  texts: Array<{ text: string; x: number; y: number; style: TextStyle }> = [];
  rects: Array<{ x: number; y: number; width: number; height: number; fill: string }> = [];
  lines: Array<{ points: Array<[number, number]>; stroke: string; width: number; dash: number[] }> = [];
  circles: Array<{ x: number; y: number; radius: number; fill: string }> = [];

  begin(width: number, height: number): void {
    this.size = { width, height };
  }

  rect(x: number, y: number, width: number, height: number, fill: string): void {
    this.rects.push({ x, y, width, height, fill });
  }

  line(points: ReadonlyArray<readonly [number, number]>, stroke: string, width: number, dash: readonly number[] = []): void {
    this.lines.push({ points: points.map(([x, y]) => [x, y]), stroke, width, dash: [...dash] });
  }

  circle(x: number, y: number, radius: number, fill: string): void {
    this.circles.push({ x, y, radius, fill });
  }

  text(text: string, x: number, y: number, style: TextStyle): void {
    this.texts.push({ text, x, y, style });
  }

  measureText(text: string, size: number): number {
    return text.length * size * 0.55;
  }

  /** Everything drawn as text, in drawing order, one entry per line. */
  get transcript(): string {
    return this.texts.map((entry) => entry.text).join("\n");
  }

  /** The text as sentences: wrapped lines rejoined, so a footer sentence can be searched for. */
  get prose(): string {
    return this.texts.map((entry) => entry.text).join(" ");
  }
}

/** Draw the report into `canvas` for the screen. Returns the logical height, or null without a 2D context. */
export function drawReportToCanvas(canvas: HTMLCanvasElement, report: PowerComparison, cssWidth: number, devicePixelRatio: number): number | null {
  const context = canvas.getContext("2d");
  if (!context) return null;
  const painter = new CanvasPainter(canvas, context, devicePixelRatio, cssWidth);
  return paintPowerComparison(painter, report, { width: cssWidth });
}

/** Render the report at export size and encode it as PNG bytes. */
export async function renderReportPng(report: PowerComparison, scale = 2): Promise<Uint8Array> {
  if (typeof document !== "undefined" && "fonts" in document) {
    await document.fonts.ready;
  }
  const canvas = document.createElement("canvas");
  const context = canvas.getContext("2d");
  if (!context) throw new Error("This platform cannot draw the comparison image");
  paintPowerComparison(new CanvasPainter(canvas, context, scale), report, { width: EXPORT_WIDTH });
  const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/png"));
  if (!blob) throw new Error("The comparison image could not be encoded");
  return new Uint8Array(await blob.arrayBuffer());
}
