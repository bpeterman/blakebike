import { useCallback, useEffect, useMemo, useState } from "react";
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
  Trash2,
  Zap,
} from "lucide-react";
import { api } from "./api";
import {
  deviceName,
  formatAge,
  formatClock,
  formatRelativeDate,
  formatUptime,
  isConnected,
  makeAndModel,
  transportLabel,
  metricsFedBy,
  readoutMark,
} from "./devices";
import type {
  DeviceLogLine,
  DeviceRole,
  DeviceSlot,
  DeviceState,
  DevicesSnapshot,
  AntAdapterStatus,
  KnownDevice,
  Metric,
  SourceChoice,
  TelemetrySources,
} from "./types";
import { deviceRoleLabel, deviceRoles } from "./types";
import { SourceSelect } from "./SourceSelect";
import { metricLabel } from "./sourcePreferences";

const roleIcon: Record<DeviceRole, typeof Activity> = {
  trainer: Bike,
  heartRate: HeartPulse,
  power: Zap,
  cadence: Gauge,
};

const roleBlurb: Record<DeviceRole, string> = {
  trainer: "FTMS smart trainer. Supplies power, cadence and speed, and takes ERG targets.",
  heartRate: "Bluetooth or ANT+ heart-rate strap or optical sensor.",
  power: "Crank, pedal or hub power meter. Most also report cadence from crank data.",
  cadence: "Dedicated cadence sensor, or a power meter used for cadence only.",
};

export function DevicesPage({
  hub,
  sources,
  onConnect,
  onCalibrate,
  onSourcePreference,
  onOfferUndo = () => undefined,
  perform,
}: {
  hub: DevicesSnapshot | null;
  sources: TelemetrySources | undefined;
  onConnect: (role: DeviceRole) => void;
  onCalibrate: () => void;
  onSourcePreference: (metric: Metric, choice: SourceChoice) => void;
  onOfferUndo?: (message: string, action: () => Promise<void>) => void;
  perform: (action: () => Promise<unknown>, label?: string) => Promise<void>;
}) {
  const [known, setKnown] = useState<KnownDevice[]>([]);
  const refreshKnown = useCallback(() => {
    api.knownDevices().then(setKnown).catch(() => undefined);
  }, []);
  // Reload whenever a slot's connection state changes: a fresh connect adds
  // or refreshes a row.
  const connectionKey = (hub?.slots ?? []).map((slot) => slot.state.status).join("|");
  useEffect(() => {
    refreshKnown();
  }, [refreshKnown, connectionKey]);

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
  const antReady = hub?.antAdapter.status === "ready";

  return (
    <>
      <header className="page-header">
        <div>
          <span>BIKE SENSORS</span>
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

      {known.length > 0 && (
        <section className="card known-devices">
          <div className="known-head">
            <div>
              <span className="label">KNOWN DEVICES</span>
              <h2>Connect again with one click</h2>
            </div>
            <p>Remembered from earlier sessions. A device that drops out reconnects on its own: for as long as a ride is running, otherwise for about a minute.</p>
          </div>
          <div className="known-list">
            {known.map((device) => {
              const slot = slots.find((candidate) => candidate.role === device.role);
              const connectedHere = slotDeviceId(slot) === device.id;
              const busy = slot?.state.status === "connecting";
              const Icon = roleIcon[device.role];
              return (
                <div key={device.id} className="known-row">
                  <span className="device-icon"><Icon size={18} /></span>
                  <div className="known-title">
                    <strong>{device.name}</strong>
                    <span>
                      {deviceRoleLabel[device.role]}
                      {!device.simulated && ` · ${transportLabel(device)}`}
                      {makeAndModel(device.manufacturer, device.model) && ` · ${makeAndModel(device.manufacturer, device.model)}`}
                      {device.simulated && " · simulated"}
                    </span>
                  </div>
                  <span className="known-when">{connectedHere ? "connected now" : `last used ${formatRelativeDate(device.lastConnectedAt, now)}`}</span>
                  {connectedHere ? (
                    <button className="secondary" onClick={() => void perform(() => api.disconnectDevice(device.role), `disconnect ${device.role}`)}>
                      Disconnect
                    </button>
                  ) : (
                    <button
                      className="primary"
                      disabled={busy}
                      onClick={() => void perform(() => api.connectDevice(device.role, {
                        id: device.id,
                        name: device.name,
                        transport: device.transport,
                        simulated: device.simulated,
                        rssi: null,
                        capabilities: device.capabilities,
                      }), `connect known ${device.role}: ${device.name}`)}
                    >
                      {busy ? "Connecting…" : "Connect"}
                    </button>
                  )}
                  <button
                    className="icon-button danger"
                    aria-label={`Forget ${device.name}`}
                    title="Forget this device"
                    onClick={() => void perform(async () => {
                      await api.forgetDevice(device.id);
                      refreshKnown();
                      onOfferUndo(`“${device.name}” forgotten.`, async () => {
                        await api.restoreKnownDevices([device]);
                        refreshKnown();
                      });
                    }, "forget device")}
                  >
                    <Trash2 size={16} />
                  </button>
                </div>
              );
            })}
          </div>
        </section>
      )}

      <div className="device-grid">
        {slots.map((slot) => (
          <DeviceCard
            key={slot.role}
            slot={slot}
            now={now}
            feeding={metricsFedBy(slot.role, liveSources)}
            onConnect={() => onConnect(slot.role)}
            onCalibrate={onCalibrate}
            onDisconnect={() => void perform(() => api.disconnectDevice(slot.role), `disconnect ${slot.role}`)}
            supportsAnt={slot.role === "heartRate" && antReady}
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
              onChange={(choice) => onSourcePreference(metric, choice)}
            />
          ))}
        </div>
      </section>

      {hub && hub.antAdapter.status !== "notAttached" && (
        <AntAdapterCard adapter={hub.antAdapter} />
      )}
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
  const nameOf = (role: DeviceRole) => {
    const slot = slots.find((candidate) => candidate.role === role);
    return deviceName(slot?.state) ?? deviceRoleLabel[role];
  };
  return (
    <div className="source-row">
      <span className="source-metric">{metricLabel[metric]}</span>
      <SourceSelect
        metric={metric}
        choice={choice}
        autoLabel="Auto (dedicated sensor first)"
        roleLabel={(role) =>
          `${deviceRoleLabel[role]}${
            isConnected(slots.find((slot) => slot.role === role)?.state)
              ? ` · ${nameOf(role)}`
              : " · not connected"
          }`
        }
        onChange={onChange}
      />
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
  onCalibrate,
  onDisconnect,
  supportsAnt,
}: {
  slot: DeviceSlot;
  now: number;
  feeding: Metric[];
  onConnect: () => void;
  onCalibrate: () => void;
  onDisconnect: () => void;
  supportsAnt: boolean;
}) {
  const [showLog, setShowLog] = useState(false);
  const Icon = roleIcon[slot.role];
  const connected = isConnected(slot.state);
  const activeDevice = connected && (slot.state.status === "ready" || slot.state.status === "controlling")
    ? slot.state.device
    : null;
  const name = deviceName(slot.state);
  const { stats } = slot;
  const status = statusOf(slot.state, stats.reconnectAttempt ?? 0);
  const lastAge = stats.lastSampleMs ? now - stats.lastSampleMs : null;
  const stale = connected && lastAge !== null && lastAge > 5000;
  const ConnectIcon = supportsAnt ? Radio : Bluetooth;
  const battery = stats.batteryPercent !== null
    ? `${stats.batteryPercent}%`
    : stats.batteryStatus ?? (stats.batteryVoltage !== null ? `${stats.batteryVoltage.toFixed(2)} V` : "—");

  return (
    <section className={`card device-card ${status.tone}`}>
      <header className="device-card-head">
        <span className={`card-icon ${connected ? "" : "subtle"}`}>
          <Icon size={22} />
        </span>
        <div className="device-card-title">
          <span className="label">{deviceRoleLabel[slot.role].toUpperCase()}</span>
          <h3>{name ?? (slot.state.status === "connecting" || slot.state.status === "reconnecting" ? slot.state.name : "Not connected")}</h3>
          {activeDevice && (
            <span className="device-make">
              {activeDevice.simulated ? "Simulated" : transportLabel(activeDevice)}
              {makeAndModel(stats.manufacturer, stats.model) && ` · ${makeAndModel(stats.manufacturer, stats.model)}`}
            </span>
          )}
        </div>
        <span className={`status-chip ${status.tone}`}>{status.text}</span>
      </header>

      <div className="device-strip">
        <Stat icon={BatteryMedium} label="Battery" value={battery} />
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
            {slot.role === "trainer" && (
              <button
                className="secondary"
                onClick={onCalibrate}
                disabled={
                  slot.state.status !== "ready" ||
                  !slot.stats.calibrationSupported ||
                  slot.stats.calibrating
                }
                title={
                  slot.state.status === "controlling"
                    ? "Calibration is unavailable during a workout"
                    : !slot.stats.calibrationSupported
                      ? "This trainer does not advertise FTMS spin-down calibration"
                      : undefined
                }
              >
                <Gauge size={15} /> {slot.stats.calibrating ? "Calibrating…" : "Calibrate"}
              </button>
            )}
            <button className="secondary" onClick={onConnect}>Change</button>
            <button className="danger-button" onClick={onDisconnect}>Disconnect</button>
          </>
        ) : (
          <button
            className="primary"
            onClick={onConnect}
            disabled={slot.state.status === "connecting"}
            title={supportsAnt ? "Connect via Bluetooth or ANT+" : "Connect via Bluetooth"}
          >
            <ConnectIcon size={15} /> {slot.state.status === "reconnecting" ? "Reconnect" : "Connect"}
          </button>
        )}
        <button className="text-button log-toggle" onClick={() => setShowLog((open) => !open)}>
          {showLog ? <ChevronUp size={14} /> : <ChevronDown size={14} />} Log
          {slot.log.length > 0 && <span className="log-count">{slot.log.length}</span>}
        </button>
      </div>
      {connected && slot.role === "trainer" && slot.state.status === "controlling" && (
        <small className="calibration-unavailable">Calibration is unavailable during a workout.</small>
      )}
      {connected && slot.role === "trainer" && slot.state.status === "ready" && !slot.stats.calibrationSupported && (
        <small className="calibration-unavailable">This trainer does not advertise FTMS spin-down calibration.</small>
      )}

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

function AntAdapterCard({ adapter }: { adapter: Exclude<AntAdapterStatus, { status: "notAttached" }> }) {
  const ready = adapter.status === "ready";
  const detail = ready
    ? `${adapter.name} is ready for ANT+ heart-rate sensors.`
    : adapter.status === "permissionDenied"
      ? `Permission denied · ${adapter.message}. Run scripts/install-ant-udev.sh, then replug the stick.`
      : adapter.status === "busy"
        ? `Stick busy · ${adapter.message}. Close other fitness apps using it.`
        : adapter.message;
  return (
    <section className={`card ant-adapter-card ${adapter.status}`}>
      <span className={`card-icon ${ready ? "" : "subtle"}`}><Radio size={22} /></span>
      <div>
        <span className="label">ANT+ ADAPTER</span>
        <h2>ANT+ receiver</h2>
        <p>{detail}</p>
      </div>
      <span className={`status-chip ${ready ? "online" : "error"}`}>{ready ? "Ready" : "Needs attention"}</span>
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

function slotDeviceId(slot: DeviceSlot | undefined): string | null {
  const state = slot?.state;
  return state && (state.status === "ready" || state.status === "controlling") ? state.device.id : null;
}

function statusOf(state: DeviceState, reconnectAttempt: number): { text: string; tone: "online" | "busy" | "off" | "error" } {
  switch (state.status) {
    case "ready":
      return { text: "Connected", tone: "online" };
    case "controlling":
      return { text: "ERG control", tone: "online" };
    case "connecting":
      return { text: "Connecting…", tone: "busy" };
    case "reconnecting":
      return {
        text: reconnectAttempt > 0 ? `Link lost · reconnecting (attempt ${reconnectAttempt})` : "Link lost",
        tone: "error",
      };
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
      batteryStatus: null,
      batteryVoltage: null,
      manufacturer: null,
      model: null,
      firmware: null,
      connectedSinceMs: null,
      drops: 0,
      reconnectAttempt: 0,
      lastRawHex: null,
      lastReading: null,
      calibrationSupported: false,
      calibrating: false,
    },
    log: [],
  };
}
