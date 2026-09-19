use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use chrono::Utc;
use tokio::sync::Mutex;

use super::{Ant, SensorId};
use crate::devices::{
    DeviceInfo, DeviceRole, DeviceSlot, DeviceState,
    ble::DeviceDetails,
    fuser::{Reading, TelemetryFuser},
};

const POLL_INTERVAL: Duration = Duration::from_millis(500);
const LINK_TIMEOUT: Duration = Duration::from_secs(5);

pub struct HeartRateReceiver {
    slot: Arc<DeviceSlot>,
    ant: Ant,
    fuser: Arc<TelemetryFuser>,
    cancel: Mutex<Option<Arc<AtomicBool>>>,
}

impl HeartRateReceiver {
    pub fn new(slot: Arc<DeviceSlot>, ant: Ant, fuser: Arc<TelemetryFuser>) -> Self {
        Self {
            slot,
            ant,
            fuser,
            cancel: Mutex::new(None),
        }
    }

    pub async fn connect(&self, device: DeviceInfo) -> Result<(), String> {
        self.disconnect().await;
        let id = SensorId::parse(&device.id)?;
        self.slot
            .set_state(DeviceState::Connecting {
                name: device.name.clone(),
            })
            .await;
        self.slot.progress(
            "info",
            "Opening ANT USB stick",
            Some("ANT USBStick2 · 57600 baud".into()),
        );
        self.slot.progress(
            "info",
            "Configuring ANT+ heart-rate channel",
            Some(format!("device {}", id.device_number)),
        );
        let session = match self.ant.connect_heart_rate(id).await {
            Ok(session) => session,
            Err(error) => {
                self.slot
                    .progress("error", "ANT connection failed", Some(error.clone()));
                self.slot
                    .set_state(DeviceState::Error {
                        message: error.clone(),
                        guidance: ant_guidance(&error),
                    })
                    .await;
                return Err(error);
            }
        };
        self.slot.progress("ok", "ANT channel open", None);
        self.slot.record_connected(
            None,
            &DeviceDetails {
                battery_percent: None,
                manufacturer: None,
                model: Some(format!("ANT+ HR {}", id.device_number)),
                firmware: None,
            },
        );

        let cancel = Arc::new(AtomicBool::new(false));
        *self.cancel.lock().await = Some(cancel.clone());
        let worker_slot = self.slot.clone();
        let worker_fuser = self.fuser.clone();
        let reconnect_name = device.name.clone();
        let worker = tokio::spawn(async move {
            let blocking_slot = worker_slot.clone();
            let blocking_fuser = worker_fuser.clone();
            let outcome = tokio::task::spawn_blocking(move || {
                let mut session = session;
                let mut last_sample = Instant::now();
                let mut first = true;
                while !cancel.load(Ordering::Acquire) {
                    match session.next_heart_rate(POLL_INTERVAL) {
                        Ok(Some((measurement, raw))) => {
                            last_sample = Instant::now();
                            let valid_bpm = measurement.bpm.is_some();
                            record(&blocking_slot, &blocking_fuser, measurement, &raw, first);
                            if valid_bpm {
                                first = false;
                            }
                        }
                        Ok(None) if last_sample.elapsed() < LINK_TIMEOUT => {}
                        Ok(None) => return Err("No ANT heart-rate data for 5 seconds".to_string()),
                        Err(error) => return Err(error),
                    }
                }
                Ok(())
            })
            .await;

            let error = match outcome {
                Ok(Ok(())) => return,
                Ok(Err(error)) => error,
                Err(error) => format!("ANT receiver task failed: {error}"),
            };
            tracing::warn!(error = %error, "ANT heart-rate link lost");
            worker_slot.record_drop();
            worker_fuser.forget(DeviceRole::HeartRate);
            worker_slot.note("error", "ANT link lost", Some(error));
            worker_slot
                .set_state(DeviceState::Reconnecting {
                    name: reconnect_name,
                })
                .await;
        });
        self.slot.set_worker(worker).await;
        self.slot.set_state(DeviceState::Ready { device }).await;
        Ok(())
    }

    pub async fn disconnect(&self) {
        if let Some(cancel) = self.cancel.lock().await.take() {
            cancel.store(true, Ordering::Release);
            // The blocking reader wakes at least twice per second.
            tokio::time::sleep(Duration::from_millis(550)).await;
        }
        self.slot.abort_worker().await;
        self.fuser.forget(DeviceRole::HeartRate);
        let was_connected = self.slot.state().await.is_connected();
        if was_connected {
            self.slot.note("info", "Disconnected", None);
        }
        self.slot.record_disconnected();
        if !matches!(self.slot.state().await, DeviceState::Idle) {
            self.slot.set_state(DeviceState::Idle).await;
        }
    }
}

fn record(
    slot: &DeviceSlot,
    fuser: &TelemetryFuser,
    measurement: super::hrm::Measurement,
    raw: &[u8; 8],
    first: bool,
) {
    slot.record_sample(Some(raw));
    let Some(bpm) = measurement.bpm else {
        return;
    };
    let summary = format!("{bpm} bpm · ANT+");
    if first {
        slot.note("ok", "First ANT heart-rate sample", Some(summary.clone()));
    }
    slot.record_reading(summary);
    fuser.ingest(
        DeviceRole::HeartRate,
        Reading::HeartRate {
            bpm: u16::from(bpm),
            sensor_contact: None,
        },
        Utc::now().timestamp_millis(),
    );
}

pub fn ant_guidance(error: &str) -> String {
    let lower = error.to_lowercase();
    if cfg!(target_os = "linux") && (lower.contains("permission") || lower.contains("denied")) {
        "Install the BlakeBike ANT udev rule, reload udev, then unplug and reconnect the ANT USB stick."
            .into()
    } else if lower.contains("busy") || lower.contains("exclusive") {
        "Close Garmin Express and any other fitness app using the ANT USB stick, then try again."
            .into()
    } else {
        "Check that the ANT USBStick2 is attached, wear and wake the heart-rate strap, then try again."
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_guidance_is_actionable() {
        let text = ant_guidance("Permission denied");
        if cfg!(target_os = "linux") {
            assert!(text.contains("udev"));
        } else {
            assert!(text.contains("USBStick2"));
        }
    }

    #[test]
    fn busy_guidance_names_competing_apps() {
        assert!(ant_guidance("device busy").contains("Garmin Express"));
    }
}
