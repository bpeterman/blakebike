import { useEffect, useState } from "react";
import { AlertTriangle, CheckCircle2, Gauge, X, Zap } from "lucide-react";
import { api } from "./api";
import { calibrationVerb, driftLabel, offsetDrift } from "./devices";
import type { CalibrationDetail, CalibrationPhase, CalibrationProgress, DeviceRole, DeviceSlot } from "./types";
import { useDialog } from "./useDialog";

type Phase = "idle" | CalibrationPhase;

const activePhases: Phase[] = ["preparing", "accelerate", "stopPedaling", "holdStill"];

/**
 * One dialog for every calibration the hub offers. The shell (icon, phase
 * heading, message, actions, close rules) is shared; the copy, the live
 * panel and the cancel semantics come from the role: a trainer spin-down
 * holds device control and can only be cancelled by disconnecting, a power
 * meter's zero offset is just a wait that can be given up.
 */
export function CalibrationModal({
  role,
  slot,
  speedKph,
  close,
}: {
  role: DeviceRole;
  slot: DeviceSlot | undefined;
  speedKph: number | null;
  close: () => void;
}) {
  const [phase, setPhase] = useState<Phase>("idle");
  const [detail, setDetail] = useState<CalibrationDetail | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [listening, setListening] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const active = activePhases.includes(phase);
  const dialogRef = useDialog(close, !active);
  const isPower = role === "power";
  const verb = calibrationVerb[role];

  useEffect(() => {
    let mounted = true;
    let unlisten: (() => void) | undefined;
    void api.onCalibrationProgress((progress: CalibrationProgress) => {
      // A trainer spin-down and a meter zero must not cross-talk.
      if (progress.role !== role) return;
      setPhase(progress.phase);
      setMessage(progress.message);
      if (progress.detail) setDetail(progress.detail);
    }).then((off) => {
      unlisten = off;
      if (mounted) setListening(true);
    });
    return () => {
      mounted = false;
      unlisten?.();
    };
  }, [role]);

  const start = async () => {
    setPhase("preparing");
    setDetail(null);
    setMessage(isPower ? "Unclip and keep the bike still." : "Preparing the trainer…");
    try {
      await api.calibrateDevice(role);
    } catch (cause) {
      const cause_ = messageOf(cause);
      setPhase("error");
      setMessage(cause_);
      void api.reportError(`${role} calibration`, cause_).catch(() => undefined);
    }
  };

  const cancel = async () => {
    setCancelling(true);
    try {
      if (isPower) {
        await api.cancelCalibration(role);
      } else {
        await api.disconnectDevice(role);
      }
      close();
    } catch (cause) {
      setMessage(messageOf(cause));
      setPhase("error");
      setCancelling(false);
    }
  };

  const heading = headingFor(role, phase, verb);
  const zero = detail?.kind === "zeroOffset" ? detail : null;
  const spinDown = detail?.kind === "spinDown" ? detail : null;
  const drift = phase === "success" && zero ? offsetDrift(zero.offsetRaw, zero.previousOffsetRaw) : null;
  const Icon = isPower ? Zap : Gauge;

  return (
    <div className="modal-backdrop dialog-enter" role="presentation" onMouseDown={(event) => {
      if (event.target === event.currentTarget && !active) close();
    }}>
      <section ref={dialogRef} className="modal calibration-modal" role="dialog" aria-modal="true" aria-labelledby="calibration-title" tabIndex={-1}>
        <button className="modal-close" onClick={close} disabled={active} aria-label="Close calibration">
          <X size={18} />
        </button>
        <div className={`modal-icon ${phase === "error" ? "error" : phase === "success" ? "success" : ""}`}>
          {phase === "success" ? <CheckCircle2 size={24} /> : phase === "error" ? <AlertTriangle size={24} /> : <Icon size={24} />}
        </div>
        <span className="label">{isPower ? "ZERO OFFSET" : "FTMS SPIN-DOWN"}</span>
        <h2 id="calibration-title">{heading}</h2>

        {phase === "idle" ? (
          isPower ? (
            <div className="calibration-copy">
              <p>
                Zeroing tells the meter what &ldquo;no load&rdquo; reads right now. Do it once the meter has reached room
                temperature, and again after a battery change. A zero taken with weight on the pedals biases every watt afterwards.
              </p>
              <ol>
                <li>Unclip and stand the bike upright and still.</li>
                <li>Nothing touching the pedals or cranks. Crank-based meters: arm hanging straight down.</li>
                <li>Press begin and keep your hands off for a few seconds.</li>
              </ol>
              {slot?.stats.lastCalibration?.offsetRaw != null && (
                <p className="calibration-previous">Last zero on record: offset {slot.stats.lastCalibration.offsetRaw}. The new value is compared against it.</p>
              )}
            </div>
          ) : (
            <div className="calibration-copy">
              <p>Place the bike securely, keep people clear of the drivetrain, and warm the trainer up before starting.</p>
              <ol>
                <li>Pedal to the speed range provided by your trainer.</li>
                <li>When prompted, stop pedaling completely.</li>
                <li>Wait while the flywheel coasts down.</li>
              </ol>
            </div>
          )
        ) : (
          <>
            {isPower ? (
              phase === "success" && zero ? (
                <div className="calibration-gauge zero-result">
                  <span>Offset</span>
                  <strong>{zero.offsetRaw}</strong>
                  {drift ? (
                    <em className={drift.tone === "large" ? "drift large" : "drift"}>
                      was {zero.previousOffsetRaw} · drift {driftLabel(drift)}
                      {drift.tone === "large" ? " · large change" : " · steady"}
                    </em>
                  ) : (
                    <em>first zero on record</em>
                  )}
                </div>
              ) : (
                <div className="calibration-gauge meter-reading">
                  <span>Meter reading</span>
                  <strong>{slot?.stats.lastReading ?? "—"}</strong>
                  <em>Should read 0 W and 0 rpm while it zeros</em>
                </div>
              )
            ) : (
              <div className="calibration-gauge">
                <span>Current speed</span>
                <strong>{speedKph === null ? "—" : speedKph.toFixed(1)} <small>km/h</small></strong>
                {spinDown && <em>Target {spinDown.targetLowKph.toFixed(1)}–{spinDown.targetHighKph.toFixed(1)} km/h</em>}
              </div>
            )}
            {drift?.tone === "large" && (
              <p className="calibration-message warn">
                That is a big jump from last time. Check for a cleat or shoe touching the pedal, a loose crank or a low battery, then zero again.
              </p>
            )}
            {message && <p className={phase === "error" ? "calibration-message error" : "calibration-message"}>{message}</p>}
          </>
        )}

        <div className="card-actions calibration-actions">
          {phase === "idle" && <button className="primary" disabled={!listening} onClick={() => void start()}>{isPower ? "Begin zero offset" : "Begin calibration"}</button>}
          {phase === "error" && <button className="primary" onClick={() => void start()}>Try again</button>}
          {phase === "success" && isPower && <button className="secondary" onClick={() => void start()}>Zero again</button>}
          {(phase === "success" || phase === "error") && <button className="secondary" onClick={close}>Close</button>}
          {active && (
            <button className="danger-button" disabled={cancelling} onClick={() => void cancel()}>
              {isPower ? (cancelling ? "Cancelling…" : "Cancel") : cancelling ? "Disconnecting…" : "Cancel and disconnect"}
            </button>
          )}
          {active && !isPower && <small>Calibration holds exclusive trainer control until it finishes or is cancelled.</small>}
        </div>
      </section>
    </div>
  );
}

function headingFor(role: DeviceRole, phase: Phase, verb: string): string {
  switch (phase) {
    case "accelerate":
      return "Pedal smoothly into the target range";
    case "stopPedaling":
      return "Stop pedaling and let the flywheel coast";
    case "holdStill":
      return "Hold still";
    case "preparing":
      return role === "power" ? "Preparing to zero" : "Preparing spin-down calibration";
    case "success":
      return role === "power" ? "Zero offset complete" : "Calibration complete";
    case "error":
      return role === "power" ? "Zero offset did not complete" : "Calibration did not complete";
    default:
      return role === "power" ? "Zero the power meter" : `${verb} trainer`;
  }
}

function messageOf(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause);
}
