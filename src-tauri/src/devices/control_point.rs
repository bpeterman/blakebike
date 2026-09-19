//! Control-point procedures shared by every GATT device that takes commands:
//! write one request to an indicate characteristic and wait for the indication
//! that answers it. FTMS (trainer) and the Cycling Power Service (power meter)
//! both work this way and both echo the request opcode at byte 1 of the
//! response, so the matching, the stale-answer drain, the timeouts and the
//! "give up the moment the link is gone" rule live here once.

use std::{fmt, future::Future, time::Duration};

use tokio::sync::broadcast;

use super::{DeviceSlot, DeviceState};
use crate::ftms::ResponseCode;

/// Budget for the GATT write of one control command. Devices answer in well
/// under a second; anything past this is a dead link, and a long wait only
/// holds the caller's command lock hostage.
pub const CONTROL_WRITE_TIMEOUT: Duration = Duration::from_secs(4);
/// Budget for the device's indication after a successful write. Procedures
/// that legitimately take longer (a power meter's offset compensation) pass
/// their own.
pub const CONTROL_ACK_TIMEOUT: Duration = Duration::from_secs(4);

/// Why a control-point command did not go through, classified so callers can
/// tell "the device is gone" from "the device said no" from "try later".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlError {
    /// No device, or its link is gone.
    NotConnected,
    /// The write went out (or was attempted) but nothing came back in time.
    Timeout,
    /// The trainer answered with an FTMS result code other than Success.
    /// (Cycling Power refusals are classified by the power-meter procedure
    /// itself, which knows its own result codes.)
    Refused(ResponseCode),
    /// Transport failure: GATT write error, response stream closed, ...
    Gatt(String),
    /// A calibration is running; ERG control is on hold.
    Busy(String),
    /// The device exposes no control point, or none we know how to drive.
    Unsupported(String),
}

impl fmt::Display for ControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ControlError::NotConnected => f.write_str("Device is not connected"),
            ControlError::Timeout => f.write_str("Control command timed out"),
            ControlError::Refused(code) => write!(f, "Device refused the command: {code:?}"),
            ControlError::Gatt(detail) => write!(f, "Control command rejected: {detail}"),
            ControlError::Busy(detail) | ControlError::Unsupported(detail) => f.write_str(detail),
        }
    }
}

impl std::error::Error for ControlError {}

impl From<ControlError> for String {
    fn from(error: ControlError) -> Self {
        error.to_string()
    }
}

/// Write one command to a control point and wait for the indication that
/// answers it. The caller holds whatever lock serializes its commands, has
/// already resolved the characteristic, and supplies:
///
/// - `write`: the GATT write itself (or the simulator's stand-in), bounded
///   here by `CONTROL_WRITE_TIMEOUT`;
/// - `after_write`: run once the write succeeded and stale answers have been
///   drained; the simulators use it to post their acknowledgement so it
///   travels through the same matching as a real device's;
/// - `ack_timeout`: how long the device may take to answer.
///
/// Both protocols send the indication for a procedure after the ATT write
/// response, so anything already queued in `responses` is a late answer to an
/// earlier (timed-out) command and must not be mistaken for this one's. The
/// answer is the first indication whose byte 1 echoes `opcode`. The whole
/// exchange is abandoned at once if the slot reports the link is down.
pub async fn run_procedure<W>(
    slot: &DeviceSlot,
    responses: &broadcast::Sender<Vec<u8>>,
    opcode: u8,
    ack_timeout: Duration,
    write: W,
    after_write: impl FnOnce(),
) -> Result<Vec<u8>, ControlError>
where
    W: Future<Output = Result<(), ControlError>>,
{
    let mut state_rx = slot.subscribe_state();
    if state_rx.borrow().link_is_down() {
        return Err(ControlError::NotConnected);
    }
    let mut responses = responses.subscribe();
    tracing::debug!(role = ?slot.role, opcode = format_args!("0x{opcode:02x}"), "Writing control command");
    let started = tokio::time::Instant::now();

    let written = tokio::select! {
        result = tokio::time::timeout(CONTROL_WRITE_TIMEOUT, write) => match result {
            Ok(result) => result,
            Err(_) => Err(ControlError::Timeout),
        },
        _ = state_rx.wait_for(DeviceState::link_is_down) => Err(ControlError::NotConnected),
    };
    if let Err(error) = written {
        tracing::error!(role = ?slot.role, opcode = format_args!("0x{opcode:02x}"), error = %error, "Control write failed");
        slot.note(
            "error",
            "Control write failed",
            Some(format!("op 0x{opcode:02x} · {error}")),
        );
        return Err(error);
    }
    loop {
        match responses.try_recv() {
            Ok(stale) => tracing::debug!(raw = ?stale, "Discarding stale control response"),
            Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
            Err(_) => break,
        }
    }
    after_write();
    let wait_for_ack = async {
        loop {
            match responses.recv().await {
                Ok(response) if response.get(1) == Some(&opcode) => return Ok(response),
                Ok(other) => {
                    tracing::trace!(raw = ?other, "Ignoring response for another opcode")
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(ControlError::Gatt("Lost control response stream".into()));
                }
            }
        }
    };
    let acknowledgement = tokio::select! {
        result = tokio::time::timeout(ack_timeout, wait_for_ack) => match result {
            Ok(result) => result,
            Err(_) => Err(ControlError::Timeout),
        },
        _ = state_rx.wait_for(DeviceState::link_is_down) => Err(ControlError::NotConnected),
    };
    match &acknowledgement {
        Ok(_) => tracing::debug!(
            role = ?slot.role,
            opcode = format_args!("0x{opcode:02x}"),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "Control command acknowledged"
        ),
        Err(ControlError::Timeout) => {
            tracing::error!(
                role = ?slot.role,
                opcode = format_args!("0x{opcode:02x}"),
                timeout_secs = ack_timeout.as_secs(),
                "No control response in time"
            );
            slot.note(
                "error",
                "Control command timed out",
                Some(format!(
                    "op 0x{opcode:02x} · no response in {} s",
                    ack_timeout.as_secs()
                )),
            );
        }
        Err(error) => tracing::warn!(
            role = ?slot.role,
            opcode = format_args!("0x{opcode:02x}"),
            error = %error,
            "Control command abandoned"
        ),
    }
    acknowledgement
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::{DeviceInfo, DeviceRole};

    fn ready_slot() -> std::sync::Arc<DeviceSlot> {
        let slot = DeviceSlot::new(DeviceRole::Power, None);
        slot.state.send_replace(DeviceState::Ready {
            device: DeviceInfo {
                id: "pm".into(),
                name: "Meter".into(),
                transport: Default::default(),
                simulated: true,
                rssi: None,
                capabilities: vec![],
            },
        });
        slot
    }

    #[tokio::test]
    async fn matches_the_answer_to_its_opcode_and_skips_stale_ones() {
        let slot = ready_slot();
        let (responses, _keep) = broadcast::channel::<Vec<u8>>(8);
        // A late answer to an earlier command is already queued.
        let stale_tx = responses.clone();
        let answer_tx = responses.clone();
        let result = run_procedure(
            &slot,
            &responses,
            0x0C,
            Duration::from_secs(1),
            async {
                let _ = stale_tx.send(vec![0x20, 0x0C, 0x01, 0x11, 0x11]);
                Ok(())
            },
            move || {
                tokio::spawn(async move {
                    let _ = answer_tx.send(vec![0x20, 0x05, 0x01]); // someone else's
                    let _ = answer_tx.send(vec![0x20, 0x0C, 0x01, 0x22, 0x22]);
                });
            },
        )
        .await
        .unwrap();
        assert_eq!(result, vec![0x20, 0x0C, 0x01, 0x22, 0x22]);
    }

    #[tokio::test(start_paused = true)]
    async fn times_out_when_nothing_answers() {
        let slot = ready_slot();
        let (responses, _keep) = broadcast::channel::<Vec<u8>>(8);
        let result = run_procedure(
            &slot,
            &responses,
            0x0C,
            Duration::from_secs(2),
            async { Ok(()) },
            || {},
        )
        .await;
        assert_eq!(result, Err(ControlError::Timeout));
        assert!(
            slot.log_lines()
                .iter()
                .any(|line| line.step == "Control command timed out")
        );
    }

    #[tokio::test]
    async fn a_failed_write_is_reported_and_logged() {
        let slot = ready_slot();
        let (responses, _keep) = broadcast::channel::<Vec<u8>>(8);
        let result = run_procedure(
            &slot,
            &responses,
            0x0C,
            Duration::from_secs(1),
            async { Err(ControlError::Gatt("boom".into())) },
            || panic!("after_write must not run when the write failed"),
        )
        .await;
        assert_eq!(result, Err(ControlError::Gatt("boom".into())));
        assert!(
            slot.log_lines()
                .iter()
                .any(|line| line.step == "Control write failed")
        );
    }

    #[tokio::test]
    async fn link_loss_abandons_the_wait() {
        let slot = ready_slot();
        let (responses, _keep) = broadcast::channel::<Vec<u8>>(8);
        let dropper = slot.clone();
        let result = run_procedure(
            &slot,
            &responses,
            0x0C,
            Duration::from_secs(30),
            async { Ok(()) },
            move || {
                tokio::spawn(async move {
                    dropper.state.send_replace(DeviceState::Reconnecting {
                        name: "Meter".into(),
                    });
                });
            },
        )
        .await;
        assert_eq!(result, Err(ControlError::NotConnected));
    }

    #[tokio::test]
    async fn refuses_to_write_to_a_down_link() {
        let slot = DeviceSlot::new(DeviceRole::Power, None);
        let (responses, _keep) = broadcast::channel::<Vec<u8>>(8);
        let result = run_procedure(
            &slot,
            &responses,
            0x0C,
            Duration::from_secs(1),
            async { panic!("must not write") },
            || {},
        )
        .await;
        assert_eq!(result, Err(ControlError::NotConnected));
    }
}
