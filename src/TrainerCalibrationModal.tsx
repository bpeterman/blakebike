import { useEffect, useState } from "react";
import { AlertTriangle, CheckCircle2, Gauge, X } from "lucide-react";
import { api } from "./api";
import type { CalibrationProgress } from "./types";
import { useDialog } from "./useDialog";

type Phase = "idle" | CalibrationProgress["phase"];

const activePhases: Phase[] = ["preparing", "accelerate", "stopPedaling"];

export function TrainerCalibrationModal({
  speedKph,
  close,
}: {
  speedKph: number | null;
  close: () => void;
}) {
  const [phase, setPhase] = useState<Phase>("idle");
  const [target, setTarget] = useState<{ low: number; high: number } | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [listening, setListening] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const active = activePhases.includes(phase);
  const dialogRef = useDialog(close, !active);

  useEffect(() => {
    let mounted = true;
    let unlisten: (() => void) | undefined;
    void api.onCalibrationProgress((progress) => {
      setPhase(progress.phase);
      setMessage(progress.message);
      if (progress.targetLowKph !== null && progress.targetHighKph !== null) {
        setTarget({ low: progress.targetLowKph, high: progress.targetHighKph });
      }
    }).then((off) => {
      unlisten = off;
      if (mounted) setListening(true);
    });
    return () => {
      mounted = false;
      unlisten?.();
    };
  }, []);

  const start = async () => {
    setPhase("preparing");
    setTarget(null);
    setMessage("Preparing the trainer…");
    try {
      await api.calibrateTrainer();
    } catch (cause) {
      const detail = messageOf(cause);
      setPhase("error");
      setMessage(detail);
      void api.reportError("trainer calibration", detail).catch(() => undefined);
    }
  };

  const cancel = async () => {
    setCancelling(true);
    try {
      await api.disconnectDevice("trainer");
      close();
    } catch (cause) {
      setMessage(messageOf(cause));
      setPhase("error");
      setCancelling(false);
    }
  };

  const instruction =
    phase === "accelerate"
      ? "Pedal smoothly into the target range"
      : phase === "stopPedaling"
        ? "Stop pedaling and let the flywheel coast"
        : phase === "preparing"
          ? "Preparing spin-down calibration"
          : phase === "success"
            ? "Calibration complete"
            : phase === "error"
              ? "Calibration did not complete"
              : "Calibrate trainer";

  return (
    <div className="modal-backdrop dialog-enter" role="presentation" onMouseDown={(event) => {
      if (event.target === event.currentTarget && !active) close();
    }}>
      <section ref={dialogRef} className="modal calibration-modal" role="dialog" aria-modal="true" aria-labelledby="calibration-title" tabIndex={-1}>
        <button className="modal-close" onClick={close} disabled={active} aria-label="Close calibration">
          <X size={18} />
        </button>
        <div className={`modal-icon ${phase === "error" ? "error" : phase === "success" ? "success" : ""}`}>
          {phase === "success" ? <CheckCircle2 size={24} /> : phase === "error" ? <AlertTriangle size={24} /> : <Gauge size={24} />}
        </div>
        <span className="label">FTMS SPIN-DOWN</span>
        <h2 id="calibration-title">{instruction}</h2>

        {phase === "idle" ? (
          <div className="calibration-copy">
            <p>Place the bike securely, keep people clear of the drivetrain, and warm the trainer up before starting.</p>
            <ol>
              <li>Pedal to the speed range provided by your trainer.</li>
              <li>When prompted, stop pedaling completely.</li>
              <li>Wait while the flywheel coasts down.</li>
            </ol>
          </div>
        ) : (
          <>
            <div className="calibration-gauge">
              <span>Current speed</span>
              <strong>{speedKph === null ? "—" : speedKph.toFixed(1)} <small>km/h</small></strong>
              {target && <em>Target {target.low.toFixed(1)}–{target.high.toFixed(1)} km/h</em>}
            </div>
            {message && <p className={phase === "error" ? "calibration-message error" : "calibration-message"}>{message}</p>}
          </>
        )}

        <div className="card-actions calibration-actions">
          {phase === "idle" && <button className="primary" disabled={!listening} onClick={() => void start()}>Begin calibration</button>}
          {phase === "error" && <button className="primary" onClick={() => void start()}>Try again</button>}
          {(phase === "success" || phase === "error") && <button className="secondary" onClick={close}>Close</button>}
          {active && <button className="danger-button" disabled={cancelling} onClick={() => void cancel()}>{cancelling ? "Disconnecting…" : "Cancel and disconnect"}</button>}
          {active && <small>Calibration holds exclusive trainer control until it finishes or is cancelled.</small>}
        </div>
      </section>
    </div>
  );
}

function messageOf(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause);
}
