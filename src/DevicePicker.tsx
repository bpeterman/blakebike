import { useCallback, useEffect, useRef, useState } from "react";
import { Activity, Bluetooth, ChevronRight, Radio, X } from "lucide-react";
import { api } from "./api";
import type { DeviceInfo, DeviceLogLine, DeviceRole, DeviceSlot } from "./types";
import { deviceRoleLabel } from "./types";
import { capabilityLabel, deviceFitsRole, readoutMark } from "./devices";

const roleHint: Record<DeviceRole, string> = {
  trainer: "Make sure your trainer is awake and not paired with another app.",
  heartRate: "Wet the strap contacts and wear it so it starts broadcasting.",
  power: "Spin the crank to wake the power meter before scanning.",
  cadence: "Spin the crank to wake the cadence sensor before scanning.",
};

type ReadoutLine = DeviceLogLine & { at: number; failed?: boolean };

export function DevicePicker({
  role,
  slot,
  scanError,
  close,
  perform,
}: {
  role: DeviceRole;
  slot: DeviceSlot | undefined;
  scanError: { message: string; guidance: string } | null;
  close: () => void;
  perform: (action: () => Promise<unknown>, label?: string) => Promise<void>;
}) {
  const [devices, setDevices] = useState<DeviceInfo[]>([]);
  const [scanning, setScanning] = useState(false);
  const [connecting, setConnecting] = useState<DeviceInfo | null>(null);
  const [readout, setReadout] = useState<ReadoutLine[]>([]);
  const [outcome, setOutcome] = useState<"connected" | "failed" | null>(null);
  const startedAt = useRef(0);
  const scannedOnce = useRef(false);
  const label = deviceRoleLabel[role];

  const scan = useCallback(async () => {
    setScanning(true);
    await perform(async () => {
      const found = await api.scanDevices();
      setDevices(found.filter((device) => deviceFitsRole(device, role)));
    }, `scan devices for ${role}`);
    setScanning(false);
  }, [perform, role]);

  useEffect(() => {
    if (!scannedOnce.current) {
      scannedOnce.current = true;
      void scan();
    }
  }, [scan]);

  useEffect(() => {
    let off: (() => void) | undefined;
    void api
      .onDeviceLog((event) => {
        if (event.role !== role || !event.line.connect) return;
        setReadout((lines) => [...lines, { ...event.line, at: performance.now() - startedAt.current }]);
      })
      .then((unlisten) => {
        off = unlisten;
      });
    return () => off?.();
  }, [role]);

  const connect = async (device: DeviceInfo) => {
    startedAt.current = performance.now();
    setReadout([]);
    setOutcome(null);
    setConnecting(device);
    void api.reportEvent(`connect ${role}`, `${device.name} (${device.id})`).catch(() => undefined);
    let ok = false;
    await perform(async () => {
      await api.connectDevice(role, device);
      ok = true;
    }, `connect ${role}: ${device.name}`);
    if (ok) {
      setOutcome("connected");
      // Let the last line land before the modal disappears.
      await new Promise((resolve) => setTimeout(resolve, 900));
      setConnecting(null);
      close();
    } else {
      setOutcome("failed");
      setReadout((lines) =>
        lines.length ? [...lines.slice(0, -1), { ...lines[lines.length - 1], failed: true }] : lines,
      );
    }
  };

  const state = slot?.state;
  const connected = state?.status === "ready" || state?.status === "controlling";
  const busy = connecting !== null && outcome === null;
  const error =
    state?.status === "error" ? { message: state.message, guidance: state.guidance } : scanError;

  return (
    <div className="modal-backdrop">
      <div className="modal device-modal">
        <button className="modal-close" disabled={busy} onClick={close} aria-label="Close">
          <X />
        </button>
        <div className="modal-icon">{busy ? <span className="spinner" /> : <Bluetooth />}</div>
        <span className="label">BLUETOOTH · {label.toUpperCase()}</span>
        <h2>
          {connecting
            ? outcome === "connected"
              ? `${label} connected`
              : outcome === "failed"
                ? "Connection failed"
                : `Connecting to ${connecting.name}…`
            : connected
              ? `${label} connected`
              : `Choose a ${label.toLowerCase()}`}
        </h2>
        {!connecting && <p>{roleHint[role]}</p>}
        {connecting ? (
          <div className={`readout ${outcome ?? "busy"}`}>
            {readout.map((line, index) => (
              <div key={index} className={`readout-line ${line.failed ? "fail" : line.level}`}>
                <span className="readout-time">{(line.at / 1000).toFixed(2)}s</span>
                <span className="readout-mark">{readoutMark(line.failed ? "error" : line.level)}</span>
                <span className="readout-step">{line.step}</span>
                {line.detail && <span className="readout-detail">{line.detail}</span>}
              </div>
            ))}
            {busy && (
              <div className="readout-line info cursor">
                <span className="readout-time" />
                <span className="readout-mark">›</span>
                <span className="readout-step">
                  working<span className="dots" />
                </span>
              </div>
            )}
            {outcome === "connected" && (
              <div className="readout-line ok">
                <span className="readout-time">{((performance.now() - startedAt.current) / 1000).toFixed(2)}s</span>
                <span className="readout-mark">✓</span>
                <span className="readout-step">Ready to ride</span>
              </div>
            )}
          </div>
        ) : (
          <>
            {error && (
              <div className="inline-error">
                <strong>{error.message}</strong>
                <span>{error.guidance}</span>
              </div>
            )}
            <div className="device-list">
              {devices.map((device) => (
                <button key={device.id} onClick={() => void connect(device)}>
                  <span className="device-icon">{device.simulated ? <Activity /> : <Radio />}</span>
                  <span>
                    <strong>{device.name}</strong>
                    <small>
                      {device.simulated ? "No hardware required" : `${device.rssi ?? "—"} dBm`}
                      {(device.capabilities ?? []).length > 0 && (
                        <span className="capability-chips">
                          {(device.capabilities ?? []).map((capability) => (
                            <i key={capability}>{capabilityLabel[capability]}</i>
                          ))}
                        </span>
                      )}
                    </small>
                  </span>
                  <ChevronRight />
                </button>
              ))}
              {!scanning && devices.length === 0 && (
                <div className="device-list-empty">No {label.toLowerCase()} found. Wake the device and scan again.</div>
              )}
            </div>
          </>
        )}
        {outcome === "failed" && error && (
          <div className="inline-error">
            <strong>{error.message}</strong>
            <span>{error.guidance}</span>
          </div>
        )}
        <div className="modal-actions">
          {outcome === "failed" ? (
            <>
              <button className="secondary" onClick={() => { setConnecting(null); setOutcome(null); }}>
                Back
              </button>
              <button className="primary" onClick={() => connecting && void connect(connecting)}>
                Retry
              </button>
            </>
          ) : (
            !connecting && (
              <button className="secondary" disabled={scanning} onClick={() => void scan()}>
                {scanning ? "Scanning…" : "Scan again"}
              </button>
            )
          )}
          {connected && !connecting && (
            <button
              className="danger-button"
              onClick={() => void perform(() => api.disconnectDevice(role), `disconnect ${role}`)}
            >
              Disconnect
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
