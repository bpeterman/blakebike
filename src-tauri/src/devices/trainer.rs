//! The trainer slot: FTMS discovery steps, ERG control and the Indoor Bike
//! Data notification worker, plus the built-in simulator.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU16, Ordering},
    },
    time::Duration,
};

use btleplug::{
    api::{Characteristic, Peripheral as _, WriteType},
    platform::Peripheral,
};
use chrono::Utc;
use futures::StreamExt;
use serde::Serialize;
use tokio::sync::{Mutex, RwLock, broadcast};

use super::{
    Capability, DeviceInfo, DeviceRole, DeviceSlot, DeviceState,
    ble::{self, Ble, GATT_CONNECT_TIMEOUT, GATT_STEP_TIMEOUT, bluetooth_uuid, hex, with_timeout},
    fuser::{Reading, TelemetryFuser},
};
use crate::{
    domain::Telemetry,
    ftms::{
        FITNESS_MACHINE_CONTROL_POINT, FITNESS_MACHINE_FEATURE, FITNESS_MACHINE_STATUS,
        INDOOR_BIKE_DATA, SUPPORTED_POWER_RANGE, SpinDownStatus, parse_control_response,
        parse_indoor_bike_data, parse_spin_down_response, parse_spin_down_status, request_control,
        set_target_power, start_or_resume, start_spin_down, stop_or_pause, supports_spin_down,
    },
};

pub const SIMULATED_TRAINER_ID: &str = "simulated-trainer";
const CALIBRATION_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibrationProgress {
    pub phase: &'static str,
    pub target_low_kph: Option<f32>,
    pub target_high_kph: Option<f32>,
    pub message: Option<String>,
}

/// Simulated devices offered at the top of every scan.
pub fn simulated_devices() -> Vec<DeviceInfo> {
    vec![DeviceInfo {
        id: SIMULATED_TRAINER_ID.into(),
        name: "BlakeBike Simulator".into(),
        simulated: true,
        rssi: Some(-30),
        capabilities: vec![Capability::Ftms],
    }]
}

pub struct Trainer {
    slot: Arc<DeviceSlot>,
    ble: Arc<Ble>,
    fuser: Arc<TelemetryFuser>,
    peripheral: RwLock<Option<Peripheral>>,
    control_point: RwLock<Option<Characteristic>>,
    target_power: Arc<AtomicU16>,
    min_power: AtomicU16,
    max_power: AtomicU16,
    command_lock: Mutex<()>,
    control_responses: broadcast::Sender<Vec<u8>>,
    status_responses: broadcast::Sender<Vec<u8>>,
    calibration_cancel: broadcast::Sender<()>,
    calibration_supported: AtomicBool,
    calibrating: AtomicBool,
}

impl Trainer {
    pub fn new(slot: Arc<DeviceSlot>, ble: Arc<Ble>, fuser: Arc<TelemetryFuser>) -> Self {
        let (control_responses, _) = broadcast::channel(16);
        let (status_responses, _) = broadcast::channel(16);
        let (calibration_cancel, _) = broadcast::channel(4);
        Self {
            slot,
            ble,
            fuser,
            peripheral: RwLock::new(None),
            control_point: RwLock::new(None),
            target_power: Arc::new(AtomicU16::new(100)),
            min_power: AtomicU16::new(0),
            max_power: AtomicU16::new(2_000),
            command_lock: Mutex::new(()),
            control_responses,
            status_responses,
            calibration_cancel,
            calibration_supported: AtomicBool::new(false),
            calibrating: AtomicBool::new(false),
        }
    }

    pub fn slot(&self) -> &Arc<DeviceSlot> {
        &self.slot
    }

    pub async fn connect(&self, device: DeviceInfo) -> Result<(), String> {
        tracing::info!(id = %device.id, name = %device.name, simulated = device.simulated, rssi = ?device.rssi, "Connecting to trainer");
        let started = std::time::Instant::now();
        let result = self.connect_inner(device.clone()).await;
        match &result {
            Ok(()) => tracing::info!(
                name = %device.name,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "Trainer connected and control acquired"
            ),
            Err(error) => {
                tracing::error!(
                    id = %device.id,
                    name = %device.name,
                    error = %error,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "Trainer connection failed"
                );
                self.slot
                    .progress("error", "Connection failed", Some(error.clone()));
                // "Channel closed" means btleplug's CoreBluetooth/BlueZ event loop
                // has died; the cached adapter and its peripherals are useless
                // until recreated, so force a fresh one on the next scan.
                if error.contains("Channel closed") {
                    self.ble.reset().await;
                }
                self.slot
                    .set_state(DeviceState::Error {
                        message: error.clone(),
                        guidance: ble::platform_guidance(),
                    })
                    .await;
            }
        }
        result
    }

    async fn connect_inner(&self, device: DeviceInfo) -> Result<(), String> {
        self.disconnect().await;
        self.calibration_supported.store(false, Ordering::Relaxed);
        self.slot.set_calibration(Some(false), Some(false)).await;
        self.slot
            .set_state(DeviceState::Connecting {
                name: device.name.clone(),
            })
            .await;
        if device.simulated {
            self.calibration_supported.store(true, Ordering::Relaxed);
            self.slot.set_calibration(Some(true), None).await;
            self.slot
                .progress("info", "Spinning up virtual flywheel", None);
            tokio::time::sleep(Duration::from_millis(350)).await;
            self.slot.progress(
                "ok",
                "Simulated FTMS service online",
                Some("0 dBm · zero latency".into()),
            );
            self.connect_simulator(device).await;
            return Ok(());
        }
        self.slot.progress(
            "info",
            "Locating trainer from last scan",
            Some(device.id.clone()),
        );
        let peripheral = self
            .ble
            .peripheral(&device.id)
            .await
            .ok_or_else(|| "Trainer is no longer available; scan again".to_string())?;
        self.slot.progress(
            "info",
            "Opening GATT link",
            device.rssi.map(|rssi| format!("signal {rssi} dBm")),
        );
        let already_connected = peripheral.is_connected().await.ok();
        tracing::debug!(already_connected = ?already_connected, "GATT connect");
        with_timeout(GATT_CONNECT_TIMEOUT, "GATT connect", peripheral.connect())
            .await
            .map_err(|error| format!("Could not connect to trainer: {error}"))?;
        tracing::debug!("GATT connected; discovering services");
        self.slot.progress("ok", "Link established", None);
        self.slot
            .progress("info", "Discovering GATT services", None);
        with_timeout(
            GATT_STEP_TIMEOUT,
            "service discovery",
            peripheral.discover_services(),
        )
        .await
        .map_err(|error| format!("Could not discover trainer services: {error}"))?;
        let characteristics = peripheral.characteristics();
        let services = peripheral.services();
        tracing::debug!(
            services = ?services.iter().map(|s| s.uuid).collect::<Vec<_>>(),
            characteristics = ?characteristics.iter().map(|c| (c.uuid, c.properties)).collect::<Vec<_>>(),
            "Services discovered"
        );
        self.slot.progress(
            "ok",
            "Services discovered",
            Some(format!(
                "{} services · {} characteristics",
                services.len(),
                characteristics.len()
            )),
        );
        let indoor_data = characteristics
            .iter()
            .find(|characteristic| characteristic.uuid == bluetooth_uuid(INDOOR_BIKE_DATA))
            .cloned()
            .ok_or_else(|| "Trainer does not expose Indoor Bike Data".to_string())?;
        self.slot.progress(
            "ok",
            "Indoor Bike Data characteristic",
            Some("0x2AD2".into()),
        );
        let control = characteristics
            .iter()
            .find(|characteristic| {
                characteristic.uuid == bluetooth_uuid(FITNESS_MACHINE_CONTROL_POINT)
            })
            .cloned()
            .ok_or_else(|| "Trainer does not support FTMS control".to_string())?;
        self.slot
            .progress("ok", "Fitness Machine Control Point", Some("0x2AD9".into()));
        if let Some(features) = characteristics
            .iter()
            .find(|characteristic| characteristic.uuid == bluetooth_uuid(FITNESS_MACHINE_FEATURE))
        {
            // Read once so capability discovery failures surface during connection.
            let data = with_timeout(GATT_STEP_TIMEOUT, "feature read", peripheral.read(features))
                .await
                .map_err(|error| format!("Could not read trainer capabilities: {error}"))?;
            tracing::debug!(features = ?data, "Fitness Machine Feature read");
            let spin_down = supports_spin_down(&data).unwrap_or(false);
            self.calibration_supported
                .store(spin_down, Ordering::Relaxed);
            self.slot.set_calibration(Some(spin_down), None).await;
            self.slot.progress(
                "ok",
                "Read machine features",
                Some(format!(
                    "{} · spin-down {}",
                    hex(&data),
                    if spin_down {
                        "supported"
                    } else {
                        "unavailable"
                    }
                )),
            );
        } else {
            tracing::debug!("Trainer does not expose Fitness Machine Feature");
            self.slot
                .progress("warn", "No Fitness Machine Feature characteristic", None);
        }
        if let Some(range) = characteristics
            .iter()
            .find(|characteristic| characteristic.uuid == bluetooth_uuid(SUPPORTED_POWER_RANGE))
            && let Ok(data) = with_timeout(
                GATT_STEP_TIMEOUT,
                "power range read",
                peripheral.read(range),
            )
            .await
            && data.len() >= 4
        {
            self.min_power.store(
                i16::from_le_bytes([data[0], data[1]]).max(0) as u16,
                Ordering::Relaxed,
            );
            self.max_power.store(
                i16::from_le_bytes([data[2], data[3]]).max(0) as u16,
                Ordering::Relaxed,
            );
            let (min_watts, max_watts) = (
                self.min_power.load(Ordering::Relaxed),
                self.max_power.load(Ordering::Relaxed),
            );
            tracing::info!(min_watts, max_watts, "Supported power range read");
            self.slot.progress(
                "ok",
                "Supported power range",
                Some(format!("{min_watts}–{max_watts} W")),
            );
        } else {
            tracing::debug!("Supported Power Range unavailable; using defaults");
            self.slot.progress(
                "warn",
                "No power range advertised",
                Some("using 0–2000 W".into()),
            );
        }
        let details = ble::read_details(&peripheral).await;
        match details.summary() {
            Some(summary) => self
                .slot
                .progress("ok", "Device information", Some(summary)),
            None => self
                .slot
                .progress("warn", "No battery or device information", None),
        }
        tracing::debug!("Subscribing to Indoor Bike Data and Control Point notifications");
        self.slot
            .progress("info", "Subscribing to telemetry stream", None);
        with_timeout(
            GATT_STEP_TIMEOUT,
            "telemetry subscribe",
            peripheral.subscribe(&indoor_data),
        )
        .await
        .map_err(|error| format!("Could not subscribe to trainer data: {error}"))?;
        self.slot
            .progress("info", "Subscribing to control responses", None);
        with_timeout(
            GATT_STEP_TIMEOUT,
            "control subscribe",
            peripheral.subscribe(&control),
        )
        .await
        .map_err(|error| format!("Could not subscribe to trainer control: {error}"))?;
        self.slot.progress("ok", "Notifications armed", None);
        if let Some(status) = characteristics
            .iter()
            .find(|characteristic| characteristic.uuid == bluetooth_uuid(FITNESS_MACHINE_STATUS))
            && let Err(error) = peripheral.subscribe(status).await
        {
            tracing::debug!(error = %error, "Could not subscribe to Fitness Machine Status (non-fatal)");
        }
        *self.peripheral.write().await = Some(peripheral.clone());
        *self.control_point.write().await = Some(control);
        self.slot.record_connected(device.rssi, &details);
        let slot = self.slot.clone();
        let fuser = self.fuser.clone();
        let control_tx = self.control_responses.clone();
        let status_tx = self.status_responses.clone();
        let reconnect_name = device.name.clone();
        let worker = tokio::spawn(async move {
            let mut notifications = match peripheral.notifications().await {
                Ok(stream) => stream,
                Err(error) => {
                    tracing::error!(error = %error, "Could not open notification stream");
                    slot.note(
                        "error",
                        "Could not open notification stream",
                        Some(error.to_string()),
                    );
                    return;
                }
            };
            tracing::debug!("Notification worker started");
            let mut samples: u64 = 0;
            let mut last_summary = std::time::Instant::now();
            while let Some(notification) = notifications.next().await {
                if notification.uuid == bluetooth_uuid(INDOOR_BIKE_DATA) {
                    match parse_indoor_bike_data(&notification.value, Utc::now().timestamp_millis())
                    {
                        Ok(telemetry) => {
                            samples += 1;
                            slot.record_sample(Some(&notification.value));
                            if samples == 1 {
                                tracing::debug!(?telemetry, "First Indoor Bike Data sample");
                                slot.note(
                                    "ok",
                                    "First sample received",
                                    Some(format!(
                                        "{} W · {} rpm",
                                        telemetry.power_watts,
                                        telemetry
                                            .cadence_rpm
                                            .map(|c| format!("{c:.0}"))
                                            .unwrap_or_else(|| "—".into())
                                    )),
                                );
                            } else if samples.is_multiple_of(120) {
                                tracing::debug!(samples, ?telemetry, "Indoor Bike Data");
                            }
                            if last_summary.elapsed() >= Duration::from_secs(60) {
                                last_summary = std::time::Instant::now();
                                let stats = slot.stats();
                                slot.note(
                                    "info",
                                    "Telemetry flowing",
                                    Some(format!(
                                        "{} samples · {:.1} Hz · {} parse failures",
                                        stats.samples, stats.rate_hz, stats.parse_failures
                                    )),
                                );
                            }
                            slot.record_reading(describe(&telemetry));
                            fuser.ingest(
                                DeviceRole::Trainer,
                                Reading::Trainer(telemetry),
                                Utc::now().timestamp_millis(),
                            );
                        }
                        Err(error) => {
                            let failures = slot.record_parse_failure(&notification.value);
                            if failures <= 5 || failures.is_multiple_of(100) {
                                tracing::warn!(failures, error = %error, raw = ?notification.value, "Could not parse Indoor Bike Data");
                                slot.note(
                                    "warn",
                                    "Could not parse Indoor Bike Data",
                                    Some(format!("{error} · {}", hex(&notification.value))),
                                );
                            }
                        }
                    }
                } else if notification.uuid == bluetooth_uuid(FITNESS_MACHINE_CONTROL_POINT) {
                    tracing::debug!(raw = ?notification.value, "Control Point response");
                    let _ = control_tx.send(notification.value);
                } else if notification.uuid == bluetooth_uuid(FITNESS_MACHINE_STATUS) {
                    tracing::debug!(raw = ?notification.value, "Fitness Machine Status");
                    let _ = status_tx.send(notification.value);
                } else {
                    tracing::trace!(uuid = %notification.uuid, raw = ?notification.value, "Other notification");
                }
            }
            tracing::warn!(samples, "Notification stream ended; trainer link lost");
            slot.record_drop();
            fuser.forget(DeviceRole::Trainer);
            slot.note(
                "error",
                "Link lost",
                Some(format!("after {samples} samples")),
            );
            slot.set_state(DeviceState::Reconnecting {
                name: reconnect_name,
            })
            .await;
        });
        self.slot.set_worker(worker).await;
        tracing::debug!("Requesting FTMS control");
        self.slot.progress(
            "info",
            "Requesting control",
            Some("op 0x00 · Request Control".into()),
        );
        self.write_control(&request_control())
            .await
            .map_err(|error| format!("Could not request trainer control: {error}"))?;
        self.slot
            .progress("ok", "Control granted", Some("ERG mode available".into()));
        self.slot.set_state(DeviceState::Ready { device }).await;
        Ok(())
    }

    async fn connect_simulator(&self, device: DeviceInfo) {
        tracing::info!("Starting simulated trainer");
        self.slot.record_connected(
            device.rssi,
            &ble::DeviceDetails {
                battery_percent: Some(100),
                manufacturer: Some("BlakeBike".into()),
                model: Some("Simulator".into()),
                firmware: Some(env!("CARGO_PKG_VERSION").into()),
            },
        );
        self.slot
            .set_state(DeviceState::Ready {
                device: device.clone(),
            })
            .await;
        let target = self.target_power.clone();
        let slot = self.slot.clone();
        let fuser = self.fuser.clone();
        let worker = tokio::spawn(async move {
            let mut power = 90.0_f32;
            let mut tick = tokio::time::interval(Duration::from_millis(500));
            let mut first = true;
            loop {
                tick.tick().await;
                let requested = target.load(Ordering::Relaxed) as f32;
                power += (requested - power) * 0.18;
                let elapsed = Utc::now().timestamp_millis() as f32 / 1_000.0;
                let wobble = elapsed.sin() * 3.0;
                let telemetry = Telemetry {
                    timestamp_ms: Utc::now().timestamp_millis(),
                    power_watts: (power + wobble).max(0.0) as u16,
                    cadence_rpm: Some(88.0 + wobble / 2.0),
                    speed_kph: Some(30.0 + wobble / 3.0),
                    heart_rate_bpm: Some((125.0 + requested / 20.0).min(185.0) as u8),
                    target_power_watts: Some(requested as u16),
                };
                slot.record_sample(None);
                if first {
                    first = false;
                    slot.note("ok", "First sample received", Some("simulated".into()));
                }
                slot.record_reading(describe(&telemetry));
                fuser.ingest(
                    DeviceRole::Trainer,
                    Reading::Trainer(telemetry),
                    Utc::now().timestamp_millis(),
                );
            }
        });
        self.slot.set_worker(worker).await;
    }

    fn emit_calibration(
        &self,
        phase: &'static str,
        target: Option<(f32, f32)>,
        message: Option<String>,
    ) {
        self.slot.emit(
            "trainer://calibration",
            CalibrationProgress {
                phase,
                target_low_kph: target.map(|value| value.0),
                target_high_kph: target.map(|value| value.1),
                message,
            },
        );
    }

    pub async fn calibrate(&self) -> Result<(), String> {
        let device = match self.slot.state().await {
            DeviceState::Ready { device } => device,
            DeviceState::Controlling { .. } => {
                return Err("Trainer calibration is unavailable during a workout".into());
            }
            _ => return Err("Connect a trainer before calibrating".into()),
        };
        if !self.calibration_supported.load(Ordering::Relaxed) {
            return Err("This trainer does not advertise FTMS spin-down calibration".into());
        }
        if self
            .calibrating
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
            .is_err()
        {
            return Err("Trainer calibration is already running".into());
        }

        self.slot.set_calibration(None, Some(true)).await;
        self.slot.note("info", "Calibration started", None);
        self.emit_calibration(
            "preparing",
            None,
            Some("Keep pedaling while the trainer prepares.".into()),
        );

        let result = self.calibrate_inner(device.simulated).await;
        self.calibrating.store(false, Ordering::SeqCst);
        self.slot.set_calibration(None, Some(false)).await;
        match &result {
            Ok(()) => {
                self.slot.note("ok", "Calibration complete", None);
                self.emit_calibration(
                    "success",
                    None,
                    Some("Trainer calibration completed successfully.".into()),
                );
            }
            Err(error) => {
                self.slot
                    .note("error", "Calibration failed", Some(error.clone()));
                self.emit_calibration("error", None, Some(error.clone()));
            }
        }
        result
    }

    async fn calibrate_inner(&self, simulated: bool) -> Result<(), String> {
        let _guard = self.command_lock.lock().await;
        if simulated {
            self.emit_calibration(
                "accelerate",
                Some((30.0, 35.0)),
                Some("Pedal into the target speed range.".into()),
            );
            tokio::time::sleep(Duration::from_millis(250)).await;
            self.emit_calibration(
                "stopPedaling",
                Some((30.0, 35.0)),
                Some("Stop pedaling and let the flywheel coast down.".into()),
            );
            tokio::time::sleep(Duration::from_millis(250)).await;
            return Ok(());
        }

        let mut statuses = self.status_responses.subscribe();
        let mut cancelled = self.calibration_cancel.subscribe();
        let response = self.write_control_locked(&start_spin_down()).await?;
        let target = parse_spin_down_response(&response).map_err(|error| error.to_string())?;
        let target_range = (target.low_kph, target.high_kph);
        self.emit_calibration(
            "accelerate",
            Some(target_range),
            Some(format!(
                "Pedal between {:.1} and {:.1} km/h.",
                target.low_kph, target.high_kph
            )),
        );

        tokio::time::timeout(CALIBRATION_TIMEOUT, async {
            loop {
                tokio::select! {
                    _ = cancelled.recv() => {
                        return Err("Trainer calibration was cancelled by disconnect".to_string());
                    }
                    received = statuses.recv() => {
                        let packet = received.map_err(|error| format!("Lost calibration status: {error}"))?;
                        match parse_spin_down_status(&packet).map_err(|error| error.to_string())? {
                            None | Some(SpinDownStatus::Requested) | Some(SpinDownStatus::Unknown(_)) => {}
                            Some(SpinDownStatus::StopPedaling) => {
                                self.emit_calibration(
                                    "stopPedaling",
                                    Some(target_range),
                                    Some("Stop pedaling and let the flywheel coast down.".into()),
                                );
                            }
                            Some(SpinDownStatus::Success) => return Ok(()),
                            Some(SpinDownStatus::Error) => {
                                return Err("Trainer reported a spin-down calibration error".into());
                            }
                        }
                    }
                }
            }
        })
        .await
        .map_err(|_| "Trainer calibration timed out after 2 minutes".to_string())?
    }

    pub async fn begin_control(&self) -> Result<(), String> {
        if self.calibrating.load(Ordering::Relaxed) {
            return Err("Wait for trainer calibration to finish".into());
        }
        let device = match self.slot.state().await {
            DeviceState::Ready { device } | DeviceState::Controlling { device } => device,
            other => {
                tracing::warn!(state = ?other, "begin_control called without a connected trainer");
                return Err("Connect a trainer before starting a workout".into());
            }
        };
        tracing::info!(name = %device.name, "Starting/resuming trainer control");
        self.write_control(&start_or_resume()).await?;
        self.slot
            .set_state(DeviceState::Controlling { device })
            .await;
        Ok(())
    }

    pub async fn set_target_power(&self, requested: u16, rider_max: u16) -> Result<u16, String> {
        if self.calibrating.load(Ordering::Relaxed) {
            return Err("Cannot change ERG power during trainer calibration".into());
        }
        let clamped = requested.clamp(
            self.min_power.load(Ordering::Relaxed),
            self.max_power.load(Ordering::Relaxed).min(rider_max),
        );
        if clamped != requested {
            tracing::debug!(requested, clamped, rider_max, "Target power clamped");
        }
        self.target_power.store(clamped, Ordering::Relaxed);
        self.write_control(&set_target_power(clamped)).await?;
        Ok(clamped)
    }

    pub async fn pause(&self) -> Result<(), String> {
        tracing::info!("Pausing trainer");
        self.write_control(&stop_or_pause(true)).await
    }

    pub async fn stop(&self) -> Result<(), String> {
        tracing::info!("Stopping trainer");
        self.target_power.store(0, Ordering::Relaxed);
        self.write_control(&stop_or_pause(false)).await
    }

    async fn write_control(&self, payload: &[u8]) -> Result<(), String> {
        let _guard = self.command_lock.lock().await;
        let response = self.write_control_locked(payload).await?;
        let expected_opcode = payload[0];
        parse_control_response(&response, expected_opcode).map_err(|error| {
            tracing::error!(
                opcode = format_args!("0x{expected_opcode:02x}"),
                error = %error,
                "Trainer refused control command"
            );
            self.slot.note(
                "error",
                "Trainer refused control command",
                Some(format!("op 0x{expected_opcode:02x} · {error}")),
            );
            error.to_string()
        })
    }

    /// Write one FTMS command while the caller holds `command_lock`.
    async fn write_control_locked(&self, payload: &[u8]) -> Result<Vec<u8>, String> {
        let peripheral = self.peripheral.read().await.clone();
        let control = self.control_point.read().await.clone();
        if let (Some(peripheral), Some(control)) = (peripheral, control) {
            let expected_opcode = payload
                .first()
                .copied()
                .ok_or_else(|| "Cannot send an empty trainer command".to_string())?;
            let mut responses = self.control_responses.subscribe();
            tracing::debug!(opcode = format_args!("0x{expected_opcode:02x}"), payload = ?payload, "Writing control command");
            let started = std::time::Instant::now();
            with_timeout(
                GATT_STEP_TIMEOUT,
                "control write",
                peripheral.write(&control, payload, WriteType::WithResponse),
            )
            .await
            .map_err(|error| {
                tracing::error!(opcode = format_args!("0x{expected_opcode:02x}"), error = %error, "Control write failed");
                self.slot.note(
                    "error",
                    "Control write failed",
                    Some(format!("op 0x{expected_opcode:02x} · {error}")),
                );
                format!("Trainer rejected control command: {error}")
            })?;
            let acknowledgement = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let response = responses
                        .recv()
                        .await
                        .map_err(|error| format!("Lost trainer response: {error}"))?;
                    if response.get(1) == Some(&expected_opcode) {
                        return Ok(response);
                    }
                    tracing::trace!(raw = ?response, "Ignoring response for another opcode");
                }
            })
            .await
            .map_err(|_| {
                tracing::error!(
                    opcode = format_args!("0x{expected_opcode:02x}"),
                    "No control response within 5s"
                );
                self.slot.note(
                    "error",
                    "Control command timed out",
                    Some(format!("op 0x{expected_opcode:02x} · no response in 5 s")),
                );
                "Trainer control command timed out".to_string()
            })?;
            match &acknowledgement {
                Ok(_) => tracing::debug!(
                    opcode = format_args!("0x{expected_opcode:02x}"),
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "Control command acknowledged"
                ),
                Err(error) => tracing::error!(
                    opcode = format_args!("0x{expected_opcode:02x}"),
                    error = %error,
                    "Trainer control response failed"
                ),
            }
            return acknowledgement;
        }
        // Simulated trainers need no GATT write.
        Ok(vec![
            0x80,
            payload.first().copied().unwrap_or_default(),
            0x01,
        ])
    }

    pub async fn disconnect(&self) {
        let _ = self.calibration_cancel.send(());
        let had_peripheral = self.peripheral.read().await.is_some();
        if had_peripheral {
            tracing::info!("Disconnecting trainer");
        }
        if let Err(error) = self.stop().await {
            tracing::debug!(error = %error, "Stop before disconnect failed (ignored)");
        }
        if self.slot.abort_worker().await {
            tracing::debug!("Trainer worker aborted");
        }
        if let Some(peripheral) = self.peripheral.write().await.take() {
            match peripheral.disconnect().await {
                Ok(()) => tracing::debug!("GATT disconnected"),
                Err(error) => tracing::warn!(error = %error, "GATT disconnect failed"),
            }
        }
        *self.control_point.write().await = None;
        self.calibration_supported.store(false, Ordering::Relaxed);
        self.calibrating.store(false, Ordering::Relaxed);
        self.slot.set_calibration(Some(false), Some(false)).await;
        self.fuser.forget(DeviceRole::Trainer);
        let was_connected = self.slot.state().await.is_connected();
        if had_peripheral || was_connected {
            self.slot.note("info", "Disconnected", None);
        }
        self.slot.record_disconnected();
        if !matches!(self.slot.state().await, DeviceState::Idle) {
            self.slot.set_state(DeviceState::Idle).await;
        }
    }
}

/// One-line summary of an Indoor Bike Data sample for the hub card.
fn describe(telemetry: &Telemetry) -> String {
    let mut parts = vec![format!("{} W", telemetry.power_watts)];
    if let Some(cadence) = telemetry.cadence_rpm {
        parts.push(format!("{cadence:.0} rpm"));
    }
    if let Some(speed) = telemetry.speed_kph {
        parts.push(format!("{speed:.1} km/h"));
    }
    if let Some(target) = telemetry.target_power_watts {
        parts.push(format!("target {target} W"));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opcodes_match_ftms() {
        assert_eq!(crate::ftms::ControlOpcode::SetTargetPower as u8, 0x05);
    }

    #[test]
    fn simulator_is_a_trainer() {
        let devices = simulated_devices();
        assert_eq!(devices.len(), 1);
        assert!(devices[0].simulated);
        assert!(devices[0].supports(Capability::Ftms));
    }

    #[tokio::test]
    async fn simulated_target_power_respects_rider_limit() {
        let (telemetry, _) = broadcast::channel(4);
        let trainer = Trainer::new(
            DeviceSlot::new(DeviceRole::Trainer, None),
            Arc::new(Ble::disabled()),
            Arc::new(TelemetryFuser::new(None, telemetry)),
        );
        assert_eq!(trainer.set_target_power(100, 800).await.unwrap(), 100);
        assert_eq!(trainer.set_target_power(900, 800).await.unwrap(), 800);
        assert_eq!(trainer.target_power.load(Ordering::Relaxed), 800);
    }

    #[tokio::test]
    async fn simulator_connects_without_hardware() {
        let hub = super::super::DeviceHub::default();
        let mut telemetry = hub.subscribe();
        let device = simulated_devices().remove(0);
        hub.connect(DeviceRole::Trainer, device).await.unwrap();
        assert!(hub.state().await.is_connected());
        let sample = tokio::time::timeout(Duration::from_secs(3), telemetry.recv())
            .await
            .expect("simulator emits telemetry")
            .unwrap();
        assert!(sample.cadence_rpm.is_some());
        hub.begin_control().await.unwrap();
        assert!(matches!(hub.state().await, DeviceState::Controlling { .. }));
        assert_eq!(hub.set_target_power(250, 400).await.unwrap(), 250);
        assert_eq!(hub.set_target_power(900, 400).await.unwrap(), 400);
        hub.disconnect().await;
        assert!(matches!(hub.state().await, DeviceState::Idle));
        let log = hub.slot(DeviceRole::Trainer).log_lines();
        assert!(
            log.iter()
                .any(|line| line.step == "Simulated FTMS service online")
        );
        assert!(log.iter().any(|line| line.step == "Disconnected"));
    }

    #[tokio::test]
    async fn simulator_calibrates_and_clears_progress_state() {
        let hub = super::super::DeviceHub::default();
        hub.connect(DeviceRole::Trainer, simulated_devices().remove(0))
            .await
            .unwrap();
        hub.calibrate_trainer().await.unwrap();
        let stats = hub.slot(DeviceRole::Trainer).stats();
        assert!(stats.calibration_supported);
        assert!(!stats.calibrating);
        assert!(
            hub.slot(DeviceRole::Trainer)
                .log_lines()
                .iter()
                .any(|line| line.step == "Calibration complete")
        );
    }

    #[tokio::test]
    async fn calibration_is_blocked_during_erg_control() {
        let hub = super::super::DeviceHub::default();
        hub.connect(DeviceRole::Trainer, simulated_devices().remove(0))
            .await
            .unwrap();
        hub.begin_control().await.unwrap();
        assert_eq!(
            hub.calibrate_trainer().await.unwrap_err(),
            "Trainer calibration is unavailable during a workout"
        );
    }

    #[tokio::test]
    async fn calibration_rejects_disconnected_and_unsupported_trainers() {
        let hub = super::super::DeviceHub::default();
        assert_eq!(
            hub.calibrate_trainer().await.unwrap_err(),
            "Connect a trainer before calibrating"
        );
        hub.connect(DeviceRole::Trainer, simulated_devices().remove(0))
            .await
            .unwrap();
        hub.trainer
            .calibration_supported
            .store(false, Ordering::Relaxed);
        assert_eq!(
            hub.calibrate_trainer().await.unwrap_err(),
            "This trainer does not advertise FTMS spin-down calibration"
        );
    }
}
