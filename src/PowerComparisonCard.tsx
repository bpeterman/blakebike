import { useEffect, useRef, useState } from "react";
import { Download } from "lucide-react";
import {
  type PowerComparison,
  describeLag,
  describeTrend,
  formatClockSeconds,
  formatSigned,
  headlineWindow,
} from "./dualPower";
import { drawReportToCanvas } from "./powerComparisonReport";
import { useElementSize } from "./useElementSize";

/**
 * The trainer-versus-power-meter section of a ride's detail: the headline
 * numbers as text, the full report drawn on a canvas (the same drawing the
 * PNG export produces), and the export button. Rendered only for rides that
 * recorded both devices.
 */
export function PowerComparisonCard({
  comparison,
  onExport,
}: {
  comparison: PowerComparison;
  /** Render and save the PNG; the caller owns the dialog and error reporting. */
  onExport: () => Promise<unknown>;
}) {
  const [frameRef, size] = useElementSize<HTMLDivElement>({ width: 700, height: 0 });
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [canDraw, setCanDraw] = useState(true);
  const [exporting, setExporting] = useState(false);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const height = drawReportToCanvas(canvas, comparison, size.width, window.devicePixelRatio || 1);
    setCanDraw(height !== null);
  }, [comparison, size.width]);

  const exportPng = async () => {
    setExporting(true);
    try {
      await onExport();
    } finally {
      setExporting(false);
    }
  };

  return (
    <section className="card power-comparison" aria-labelledby="power-comparison-title">
      <div className="chart-heading">
        <div>
          <span className="label">TRAINER VS POWER METER</span>
          <h3 id="power-comparison-title">Power accuracy comparison</h3>
        </div>
        {comparison.status === "ready" && (
          <button type="button" className="secondary" onClick={() => void exportPng()} disabled={exporting}>
            <Download size={16} /> {exporting ? "Exporting…" : "Export PNG"}
          </button>
        )}
      </div>
      {comparison.status === "ready" ? <ReadySummary comparison={comparison} /> : (
        <p className="power-comparison-note" role="status">
          {comparison.reason} Both devices overlapped for {formatClockSeconds(comparison.coverage.overlapSeconds)} of a {formatClockSeconds(comparison.coverage.rideSeconds)} ride.
        </p>
      )}
      <div ref={frameRef}>
        {canDraw ? (
          <canvas
            ref={canvasRef}
            className="power-comparison-canvas"
            role="img"
            aria-label="Trainer versus power meter comparison: both traces, drift per minute, Bland-Altman plot, and differences by power level and cadence"
          />
        ) : (
          <p className="power-comparison-note">The comparison image cannot be drawn on this platform.</p>
        )}
      </div>
      <p className="power-comparison-note">
        {comparison.signConvention} Neither device is treated as the reference; coasting and power steps are left out, and the warm-up axis is elapsed time, not temperature.
      </p>
    </section>
  );
}

function ReadySummary({ comparison }: { comparison: Extract<PowerComparison, { status: "ready" }> }) {
  const stats = headlineWindow(comparison);
  const { coverage } = comparison;
  return (
    <dl className="power-comparison-summary">
      <div>
        <dt>Mean offset · {stats.windowSeconds} s</dt>
        <dd>
          {formatSigned(stats.meanWatts, 1, "W")}
          <small>{formatSigned(stats.meanPercent, 1, "%")} · trainer − meter</small>
        </dd>
      </div>
      <div>
        <dt>Spread</dt>
        <dd>
          ±{stats.sdWatts.toFixed(1)} W
          <small>95% within {formatSigned(stats.loaLowWatts, 0, "")}…{formatSigned(stats.loaHighWatts, 0, "W")}</small>
        </dd>
      </div>
      <div>
        <dt>Worst</dt>
        <dd>
          {stats.p95AbsWatts.toFixed(0)} W · {stats.maxAbsWatts.toFixed(0)} W
          <small>P95 · max, {stats.maxAbsPercent.toFixed(1)}% at most</small>
        </dd>
      </div>
      <div>
        <dt>Fit</dt>
        <dd>
          {formatSigned(comparison.trend.interceptWatts, 1, "W")} {comparison.trend.slopePercent >= 0 ? "+" : "−"} {Math.abs(comparison.trend.slopePercent).toFixed(1)}%
          <small>{describeTrend(comparison.trend)}</small>
        </dd>
      </div>
      <div>
        <dt>Lag</dt>
        <dd>
          {comparison.lag.confident ? `${Math.abs(comparison.lag.seconds).toFixed(2)} s` : "n/a"}
          <small>{describeLag(comparison.lag).replace(/^Lag: /, "")}</small>
        </dd>
      </div>
      <div>
        <dt>Compared</dt>
        <dd>
          {formatClockSeconds(coverage.comparedSeconds)}
          <small>{formatClockSeconds(coverage.excludedSeconds)} excluded of {formatClockSeconds(coverage.overlapSeconds)} together</small>
        </dd>
      </div>
    </dl>
  );
}
