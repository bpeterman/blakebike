import { useEffect, useRef } from "react";
import { Activity } from "lucide-react";
import { DELTA_LABEL, DeltaWindow, formatSigned, liveDelta } from "./dualPower";
import {
  deviceName,
  emptySlot,
  formatAge,
  formatUptime,
  isConnected,
  makeAndModel,
  statusOf,
  transportLabel,
} from "./devices";
import { useNowTick } from "./useNowTick";
import { metricLabel } from "./sourcePreferences";
import type {
  ControlStatus,
  DeviceRole,
  DeviceSlot,
  DevicesSnapshot,
  Metric,
  PowerSmoothing,
  SlotStats,
  Telemetry,
  TelemetrySources,
} from "./types";
import { deviceRoleLabel, deviceRoles, powerSmoothingLabel } from "./types";

export function Stat({ icon: Icon, label, value }: { icon: typeof Activity; label: string; value: string }) {
  return (
    <div className="device-stat">
      <span><Icon size={13} /> {label}</span>
      <strong>{value}</strong>
    </div>
  );
}

const metrics: Metric[] = ["power", "cadence", "heartRate"];

const controlLabel: Record<ControlStatus, string> = {
  ok: "acknowledging targets",
  degraded: "not acknowledging targets",
  lost: "link lost",
};

/** A slot worth a row: it has a device, or had one and is trying to get it back. */
function hasDevice(slot: DeviceSlot): boolean {
  return slot.state.status !== "idle" && slot.state.status !== "scanning";
}

function batteryOf(stats: SlotStats): string {
  if (stats.batteryPercent !== null) return `${stats.batteryPercent}%`;
  if (stats.batteryStatus !== null) return stats.batteryStatus;
  if (stats.batteryVoltage !== null) return `${stats.batteryVoltage.toFixed(2)} V`;
  return "—";
}

function metricValue(metric: Metric, telemetry: Telemetry): string {
  switch (metric) {
    case "power":
      return `${Math.round(telemetry.powerWatts)} W`;
    case "cadence":
      return telemetry.cadenceRpm === null ? "—" : `${Math.round(telemetry.cadenceRpm)} rpm`;
    case "heartRate":
      return telemetry.heartRateBpm === null ? "—" : `${telemetry.heartRateBpm} bpm`;
  }
}

function sourceText(sources: TelemetrySources | undefined, metric: Metric): string {
  const source = sources?.[metric];
  if (!source) return "no source";
  const label = deviceRoleLabel[source.role].toLowerCase();
  return source.fallback ? `${label} (fallback)` : label;
}

/**
 * Everything every device is reporting, live: per-device link health and raw
 * packets, plus which device is feeding each fused metric. Off by default;
 * riders turn it on in Settings → Live ride cards.
 */
export function DeviceStatsCard({
  hub,
  telemetry,
  displayPowerWatts,
  powerSmoothing,
  historySampleCount,
  control,
}: {
  hub: DevicesSnapshot | null;
  telemetry: Telemetry;
  /** Power as the ride cards show it, after smoothing. */
  displayPowerWatts: number;
  powerSmoothing: PowerSmoothing;
  historySampleCount: number;
  control: ControlStatus | undefined;
}) {
  const now = useNowTick();
  // Trainer versus meter, live: the instantaneous pair plus a 30 s mean from
  // the samples this card has seen (not ride history, which is empty before a
  // ride starts, when riders compare the most).
  const deltaWindow = useRef(new DeltaWindow());
  useEffect(() => {
    deltaWindow.current.push(telemetry);
  }, [telemetry]);
  const delta = liveDelta(telemetry.powerBySource);
  const rolling = delta ? deltaWindow.current.mean(now) : null;
  // Every role, in the hub's order, so a role that has never been connected is
  // still named rather than silently missing.
  const slots = deviceRoles.map(
    (role) => hub?.slots.find((slot) => slot.role === role) ?? emptySlot(role),
  );
  // Live samples carry the fusion record; between samples the hub snapshot has
  // the last one.
  const sources = telemetry.sources ?? hub?.sources;
  const withDevice = slots.filter(hasDevice);
  const withoutDevice = slots.filter((slot) => !hasDevice(slot));
  const telemetryAge = telemetry.timestampMs > 0 ? now - telemetry.timestampMs : null;

  return (
    <div className="card nerd-stats">
      <div className="chart-heading">
        <div><span className="label">STATS FOR NERDS</span><h3>Device data</h3></div>
        <span className="nerd-stats-note">
          {withDevice.filter((slot) => isConnected(slot.state)).length} of {slots.length} connected
          {hub?.scanning && " · scanning"}
        </span>
      </div>

      <div className="nerd-fusion">
        {metrics.map((metric) => (
          <div className="nerd-fusion-metric" key={metric}>
            <span className="label">{metricLabel[metric].toUpperCase()}</span>
            <strong>{metricValue(metric, telemetry)}</strong>
            <small>{sourceText(sources, metric)}</small>
          </div>
        ))}
        <dl className="device-details">
          <dt>Power shown</dt>
          <dd>
            {Math.round(displayPowerWatts)} W
            {powerSmoothing !== "instant" && ` · ${powerSmoothingLabel[powerSmoothing]} · raw ${Math.round(telemetry.powerWatts)} W`}
          </dd>
          <dt>Target</dt>
          <dd>
            {telemetry.targetPowerWatts === null ? "free" : `${telemetry.targetPowerWatts} W`}
            {control && ` · trainer ${controlLabel[control]}`}
          </dd>
          <dt>Last sample</dt>
          <dd>{telemetryAge === null ? "—" : formatAge(telemetryAge)} · {historySampleCount} recorded</dd>
        </dl>
        {delta && (
          <div className="nerd-delta" data-nerd-delta>
            <span className="label">TRAINER VS METER</span>
            <strong>
              Trainer {delta.trainerWatts} W · Meter {delta.meterWatts} W · {formatSigned(delta.watts, 0, "W")}
              {delta.percent !== null && ` (${formatSigned(delta.percent, 1, "%")})`}
            </strong>
            <small>
              {DELTA_LABEL}
              {delta.percent === null && " · no percent while coasting"}
              {rolling && ` · 30 s mean ${formatSigned(rolling.watts, 0, "W")} (${formatSigned(rolling.percent, 1, "%")}) over ${rolling.count} samples`}
            </small>
          </div>
        )}
      </div>

      <div className="nerd-devices">
        {withDevice.map((slot) => (
          <NerdDeviceRow key={slot.role} slot={slot} now={now} sources={sources} />
        ))}
        {withDevice.length === 0 && <p className="nerd-empty">No devices connected.</p>}
      </div>

      {withoutDevice.length > 0 && (
        <p className="nerd-empty">
          Not connected: {withoutDevice.map((slot) => deviceRoleLabel[slot.role].toLowerCase()).join(", ")}.
        </p>
      )}

      {hub && hub.antAdapter.status !== "notAttached" && (
        <p className="nerd-empty">
          ANT+ adapter: {hub.antAdapter.status === "ready" ? hub.antAdapter.name : `${hub.antAdapter.status} · ${hub.antAdapter.message}`}
        </p>
      )}
    </div>
  );
}

function feedingText(role: DeviceRole, sources: TelemetrySources | undefined): string | null {
  const fed = metrics.filter((metric) => sources?.[metric]?.role === role);
  if (fed.length === 0) return null;
  return `feeding ${fed.map((metric) => metricLabel[metric].toLowerCase()).join(", ")}`;
}

function NerdDeviceRow({
  slot,
  now,
  sources,
}: {
  slot: DeviceSlot;
  now: number;
  sources: TelemetrySources | undefined;
}) {
  const { stats } = slot;
  const status = statusOf(slot.state, stats.reconnectAttempt ?? 0);
  const connected = isConnected(slot.state);
  const activeDevice = slot.state.status === "ready" || slot.state.status === "controlling"
    ? slot.state.device
    : null;
  const name = deviceName(slot.state)
    ?? (slot.state.status === "connecting" || slot.state.status === "reconnecting" ? slot.state.name : deviceRoleLabel[slot.role]);
  const lastAge = stats.lastSampleMs === null ? null : now - stats.lastSampleMs;
  const feeding = feedingText(slot.role, sources);

  return (
    <section className={`nerd-device ${status.tone}`} data-nerd-device={slot.role}>
      <header>
        <div>
          <span className="label">{deviceRoleLabel[slot.role].toUpperCase()}</span>
          <strong>{name}</strong>
          <small>
            {activeDevice && (activeDevice.simulated ? "Simulated" : transportLabel(activeDevice))}
            {makeAndModel(stats.manufacturer, stats.model) && ` · ${makeAndModel(stats.manufacturer, stats.model)}`}
            {stats.firmware && ` · fw ${stats.firmware}`}
          </small>
        </div>
        <span className={`status-chip ${status.tone}`}>{status.text}</span>
      </header>

      <div className="nerd-reading">
        <strong>{connected ? stats.lastReading ?? "waiting for data…" : "—"}</strong>
        {feeding && <span className="feeding">{feeding}</span>}
      </div>

      <dl className="device-details">
        <dt>Rate</dt>
        <dd>{stats.rateHz > 0 ? `${stats.rateHz.toFixed(1)} Hz` : "—"}</dd>
        <dt>Last sample</dt>
        <dd>{lastAge === null ? "—" : formatAge(lastAge)}</dd>
        <dt>Samples</dt>
        <dd>{stats.samples}</dd>
        <dt>Bad packets</dt>
        <dd>{stats.parseFailures}</dd>
        <dt>Drops</dt>
        <dd>{stats.drops}</dd>
        <dt>Uptime</dt>
        <dd>{stats.connectedSinceMs === null ? "—" : formatUptime(now - stats.connectedSinceMs)}</dd>
        <dt>Signal</dt>
        <dd>{stats.rssi === null ? "—" : `${stats.rssi} dBm`}</dd>
        <dt>Battery</dt>
        <dd>{batteryOf(stats)}</dd>
      </dl>

      {stats.lastRawHex && (
        <p className="nerd-packet">
          <span>Last packet</span>
          <code>{stats.lastRawHex}</code>
        </p>
      )}
      {slot.state.status === "error" && <p className="nerd-error">{slot.state.message}</p>}
    </section>
  );
}
