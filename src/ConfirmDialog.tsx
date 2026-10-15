import { AlertTriangle, X } from "lucide-react";
import { useDialog } from "./useDialog";

export function ConfirmDialog({
  title,
  description,
  confirmLabel,
  busyLabel,
  busy = false,
  onConfirm,
  onClose,
}: {
  title: string;
  description: string;
  confirmLabel: string;
  busyLabel?: string;
  busy?: boolean;
  onConfirm: () => void;
  onClose: () => void;
}) {
  const dialogRef = useDialog(onClose, !busy);
  return (
    <div className="modal-backdrop dialog-enter" role="presentation" onMouseDown={(event) => {
      if (event.target === event.currentTarget && !busy) onClose();
    }}>
      <section
        ref={dialogRef}
        className="modal confirm-modal"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="confirm-title"
        aria-describedby="confirm-description"
        tabIndex={-1}
      >
        <button className="modal-close" disabled={busy} onClick={onClose} aria-label="Cancel"><X size={18} /></button>
        <div className="modal-icon caution"><AlertTriangle size={23} /></div>
        <h2 id="confirm-title">{title}</h2>
        <p id="confirm-description">{description}</p>
        <div className="modal-actions confirm-actions">
          <button className="secondary" disabled={busy} onClick={onClose}>Keep it</button>
          <button className="danger-button" disabled={busy} onClick={onConfirm}>
            {busy ? (busyLabel ?? "Working…") : confirmLabel}
          </button>
        </div>
      </section>
    </div>
  );
}
