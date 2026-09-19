import { useEffect, useMemo, useState } from "react";
import {
  Activity,
  BatteryMedium,
  Bike,
  Bluetooth,
  ChevronDown,
  ChevronUp,
  Gauge,
  HeartPulse,
  Radio,
  Zap,
} from "lucide-react";
import { api } from "./api";
import {
  deviceName,
  formatAge,
  formatClock,
  formatUptime,
  isConnected,
  metricsFedBy,
  readoutMark,
} from "./devices";
import type {
  DeviceLogLine,
  DeviceRole,
  DeviceSlot,
  DeviceState,
  DevicesSnapshot,
  Metric,
  SourceChoice,
  SourcePreferences,
  TelemetrySources,
} from "./types";
import { deviceRoleLabel, deviceRoles } from "./types";

const roleIcon: Record<DeviceRole, typeof Activity> = {
  trainer: Bike,
  heartRate: HeartPulse,
  power: Zap,
  cadence: Gauge,
};

const roleBlurb: Record<DeviceRole, string> = {
  trainer: "FTMS smart trainer. Supplies power, cadence and speed, and takes ERG targets.",
  heartRate: "Bluetooth heart-rate strap or optical sensor.",
  power: "Crank, pedal or hub power meter. Most also report cadence from crank data.",
  cadence: "Dedicated cadence sensor, or a power meter used for cadence only.",
};

const metricLabel: Record<Metric, string> = {
  power: "Power",
  cadence: "Cadence",
  heartRate: "Heart rate",
};

/** Roles that can supply each metric, in the backend's default priority order. */
const metricRoles: Record<Metric, DeviceRole[]> = {
  power: ["power", "trainer"],
  cadence: ["cadence", "power", "trainer"],
  heartRate: ["heartRate", "trainer"],
};

export function DevicesPage({
  hub,
  sources,
  onConnect,
  perform,
}: {
  hub: DevicesSnapshot | null;
  sources: TelemetrySources | undefined;
  onConnect: (role: DeviceRole) => void;
  perform: (action: () => Promise<unknown>, label?: string) => Promise<void>;
}) {
  // A one-second clock so uptime and "last sample" ages tick without new events.
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);

  const slots = useMemo(() => {
    const byRole = new Map((hub?.slots ?? []).map((slot) => [slot.role, slot]));
    return deviceRoles.map((role) => byRole.get(role) ?? emptySlot(role));
  }, [hub]);
  const connectedCount = slots.filter((slot) => isConnected(slot.state)).length;
  const liveSources = sources ?? hub?.sources;

  const savePreferences = (preferences: SourcePreferences) =>
    void perform(() => api.saveSourcePreferences(preferences), "save source preferences");

  return (
    <>
      <header className="page-header">
        <div>
          <span>BLUETOOTH SENSORS</span>
          <h1>Devices</h1>
        </div>
        <div className="header-actions">
          <span className="data-badge">
            {connectedCount} of {deviceRoles.length} connected
          </span>
        </div>
      </header>

      {hub?.scanError && (
        <div className="inline-error devices-scan-error">
          <strong>{hub.scanError.message}</strong>
          <span>{hub.scanError.guidance}</span>
        </div>
      )}

      <div className="device-grid">
        {slots.map((slot) => (
          <DeviceCard
            key={slot.role}
            slot={slot}
            now={now}
            feeding={metricsFedBy(slot.role, liveSources)}
            onConnect={() => onConnect(slot.role)}
            onDisconnect={() => void perform(() => api.disconnectDevice(slot.role), `disconnect ${slot.role}`)}
          />
        ))}
      </div>

      <section className="card sources-card">
        <div>
          <span className="label">SOURCES</span>
          <h2>Which device feeds each metric</h2>
          <p>
            Auto prefers a dedicated sensor over the trainer. If the chosen device goes quiet for a few seconds,
            the next one in line takes over automatically and the metric is marked as falling back.
          </p>
        </div>
        <div className="sources-list">
          {(Object.keys(metricLabel) as Metric[]).map((metric) => (
            <SourceRow
              key={metric}
              metric={metric}
              choice={hub?.sourcePreferences[metric] ?? { mode: "auto" }}
              active={liveSources?.[metric] ?? null}
              slots={slots}
              onChange={(choice) => {
                const current = hub?.sourcePreferences ?? {
                  power: { mode: "auto" },
                  cadence: { mode: "auto" },
                  heartRate: { mode: "auto" },
                };
                savePreferences({ ...current, [metric]: choice });
              }}
            />
          ))}
        </div>
      </section>
    </>
  );
}

function SourceRow({
  metric,
  choice,
  active,
  slots,
  onChange,
}: {
  metric: Metric;
  choice: SourceChoice;
  active: { role: DeviceRole; fallback: boolean } | null;
  slots: DeviceSlot[];
  onChange: (choice: SourceChoice) => void;
}) {
  const value = choice.mode === "auto" ? "auto" : choice.role;
  const nameOf = (role: DeviceRole) => {
    const slot = slots.find((candidate) => candidate.role === role);
    return deviceName(slot?.state) ?? deviceRoleLabel[role];
  };
  return (
    <div className="source-row">
      <span className="source-metric">{metricLabel[metric]}</span>
      <select
        value={value}
        onChange={(event) =>
          onChange(
            event.target.value === "auto"
              ? { mode: "auto" }
              : { mode: "role", role: event.target.value as DeviceRole },
          )
        }
      >
        <option value="auto">Auto (dedicated sensor first)</option>
        {metricRoles[metric].map((role) => (
          <option key={role} value={role}>
            {deviceRoleLabel[role]}
            {isConnected(slots.find((slot) => slot.role === role)?.state) ? ` · ${nameOf(role)}` : " · not connected"}
          </option>
        ))}
      </select>
      <span className={active ? (active.fallback ? "source-active fallback" : "source-active") : "source-active none"}>
        {active
          ? active.fallback
            ? `falling back to ${nameOf(active.role)}`
            : `from ${nameOf(active.role)}`
          : "no data"}
      </span>
    </div>
  );
}

function DeviceCard({
  slot,
  now,
  feeding,
  onConnect,
  onDisconnect,
}: {
  slot: DeviceSlot;
  now: number;
  feeding: Metric[];
  onConnect: () => void;
  onDisconnect: () => void;
}) {
  const [showLog, setShowLog] = useState(false);
  const Icon = roleIcon[slot.role];
  const connected = isConnected(slot.state);
  const name = deviceName(slot.state);
  const { stats } = slot;
  const status = statusOf(slot.state);
  const lastAge = stats.lastSampleMs ? now - stats.lastSampleMs : null;
  const stale = connected && lastAge !== null && lastAge > 5000;

  return (
    <section className={`card device-card ${status.tone}`}>
      <header className="device-card-head">
        <span className={`card-icon ${connected ? "" : "subtle"}`}>
          <Icon size={22} />
        </span>
        <div>
          <span className="label">{deviceRoleLabel[slot.role].toUpperCase()}</span>
          <h3>{name ?? (slot.state.status === "connecting" || slot.state.status === "reconnecting" ? slot.state.name : "Not connected")}</h3>
        </div>
        <span className={`status-chip ${status.tone}`}>{status.text}</span>
      </header>

      <div className="device-strip">
        <Stat icon={BatteryMedium} label="Battery" value={stats.batteryPercent !== null ? `${stats.batteryPercent}%` : "—"} />
        <Stat icon={Radio} label="Signal" value={stats.rssi !== null ? `${stats.rssi} dBm` : "—"} />
        <Stat icon={Activity} label="Uptime" value={stats.connectedSinceMs ? formatUptime(now - stats.connectedSinceMs) : "—"} />
      </div>

      <div className={`device-reading ${connected ? "" : "muted"} ${stale ? "stale" : ""}`}>
        {connected ? (
          <>
            <strong>{stats.lastReading ?? "waiting for data…"}</strong>
            <small>
              {stats.rateHz > 0 ? `${stats.rateHz.toFixed(1)} Hz` : "no samples yet"}
              {lastAge !== null && ` · last ${formatAge(lastAge)}`}
              {stats.drops > 0 && ` · ${stats.drops} drop${stats.drops === 1 ? "" : "s"}`}
              {stats.parseFailures > 0 && ` · ${stats.parseFailures} bad packet${stats.parseFailures === 1 ? "" : "s"}`}
            </small>
            {feeding.length > 0 && (
              <span className="feeding">
                feeding {feeding.map((metric) => metricLabel[metric].toLowerCase()).join(", ")}
              </span>
            )}
          </>
        ) : (
          <>
            <strong>{roleBlurb[slot.role]}</strong>
            {stats.drops > 0 && <small>{stats.drops} drop{stats.drops === 1 ? "" : "s"} this session</small>}
          </>
        )}
      </div>

      <div className="card-actions device-actions">
        {connected ? (
          <>
            <button className="secondary" onClick={onConnect}>Change</button>
            <button className="danger-button" onClick={onDisconnect}>Disconnect</button>
          </>
        ) : (
          <button className="primary" onClick={onConnect} disabled={slot.state.status === "connecting"}>
            <Bluetooth size={15} /> {slot.state.status === "reconnecting" ? "Reconnect" : "Connect"}
          </button>
        )}
        <button className="text-button log-toggle" onClick={() => setShowLog((open) => !open)}>
          {showLog ? <ChevronUp size={14} /> : <ChevronDown size={14} />} Log
          {slot.log.length > 0 && <span className="log-count">{slot.log.length}</span>}
        </button>
      </div>

      {showLog && (
        <div className="device-log">
          {(stats.manufacturer || stats.model || stats.firmware || stats.lastRawHex) && (
            <dl className="device-details">
              {stats.manufacturer && (<><dt>Make</dt><dd>{stats.manufacturer}</dd></>)}
              {stats.model && (<><dt>Model</dt><dd>{stats.model}</dd></>)}
              {stats.firmware && (<><dt>Firmware</dt><dd>{stats.firmware}</dd></>)}
              {stats.lastRawHex && (<><dt>Last packet</dt><dd><code>{stats.lastRawHex}</code></dd></>)}
              <dt>Samples</dt><dd>{stats.samples}</dd>
            </dl>
          )}
          <LogReadout lines={slot.log} />
        </div>
      )}
    </section>
  );
}

function Stat({ icon: Icon, label, value }: { icon: typeof Activity; label: string; value: string }) {
  return (
    <div className="device-stat">
      <span><Icon size={13} /> {label}</span>
      <strong>{value}</strong>
    </div>
  );
}

function LogReadout({ lines }: { lines: DeviceLogLine[] }) {
  if (lines.length === 0) {
    return <div className="readout empty">No activity yet.</div>;
  }
  return (
    <div className="readout hub-log">
      {lines.map((line, index) => (
        <div key={`${line.atMs}-${index}`} className={`readout-line ${line.level === "error" ? "fail" : line.level}`}>
          <span className="readout-time">{formatClock(line.atMs)}</span>
          <span className="readout-mark">{readoutMark(line.level)}</span>
          <span className="readout-step">{line.step}</span>
          {line.detail && <span className="readout-detail" title={line.detail}>{line.detail}</span>}
        </div>
      ))}
    </div>
  );
}

function statusOf(state: DeviceState): { text: string; tone: "online" | "busy" | "off" | "error" } {
  switch (state.status) {
    case "ready":
      return { text: "Connected", tone: "online" };
    case "controlling":
      return { text: "ERG control", tone: "online" };
    case "connecting":
      return { text: "Connecting…", tone: "busy" };
    case "reconnecting":
      return { text: "Link lost", tone: "error" };
    case "scanning":
      return { text: "Scanning…", tone: "busy" };
    case "error":
      return { text: "Error", tone: "error" };
    default:
      return { text: "Idle", tone: "off" };
  }
}

function emptySlot(role: DeviceRole): DeviceSlot {
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
      manufacturer: null,
      model: null,
      firmware: null,
      connectedSinceMs: null,
      drops: 0,
      lastRawHex: null,
      lastReading: null,
    },
    log: [],
  };
}
