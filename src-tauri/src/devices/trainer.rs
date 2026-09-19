//! The trainer slot: FTMS discovery steps, ERG control and the Indoor Bike
//! Data notification worker, plus the built-in simulator.

use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU16, AtomicU32, AtomicU64, Ordering},
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
use tokio::sync::{Mutex, Notify, RwLock, broadcast};

use super::{
    Capability, DeviceInfo, DeviceRole, DeviceSlot, DeviceState,
    ble::{self, Ble, GATT_CONNECT_TIMEOUT, GATT_STEP_TIMEOUT, bluetooth_uuid, hex, with_timeout},
    fuser::{Reading, TelemetryFuser},
    spawn_link_worker,
};
use crate::{
    domain::Telemetry,
    ftms::{
        FITNESS_MACHINE_CONTROL_POINT, FITNESS_MACHINE_FEATURE, FITNESS_MACHINE_STATUS, FtmsError,
        INDOOR_BIKE_DATA, ResponseCode, SUPPORTED_POWER_RANGE, SpinDownStatus,
        parse_control_response, parse_indoor_bike_data, parse_spin_down_response,
        parse_spin_down_status, request_control, set_target_power, start_or_resume,
        start_spin_down, stop_or_pause, supports_spin_down,
    },
};

pub const SIMULATED_TRAINER_ID: &str = "simulated-trainer";
const CALIBRATION_TIMEOUT: Duration = Duration::from_secs(120);
/// Budget for the GATT write of one control command. Trainers answer in well
/// under a second; anything past this is a dead link, and a long wait only
/// holds `command_lock` hostage.
pub const CONTROL_WRITE_TIMEOUT: Duration = Duration::from_secs(4);
/// Budget for the trainer's indication after a successful write.
pub const CONTROL_ACK_TIMEOUT: Duration = Duration::from_secs(4);

/// Why a control-point command did not go through, classified so callers can
/// tell "the trainer is gone" from "the trainer said no" from "try later".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlError {
    /// No trainer, or its link is gone.
    NotConnected,
    /// The write went out (or was attempted) but nothing came back in time.
    Timeout,
    /// The trainer answered with an FTMS result code other than Success.
    Refused(ResponseCode),
    /// Transport failure: GATT write error, response stream closed, ...
    Gatt(String),
    /// A calibration is running; ERG control is on hold.
    Busy(String),
}

impl fmt::Display for ControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ControlError::NotConnected => f.write_str("Trainer is not connected"),
            ControlError::Timeout => f.write_str("Trainer control command timed out"),
            ControlError::Refused(code) => write!(f, "Trainer refused the command: {code:?}"),
            ControlError::Gatt(detail) => write!(f, "Trainer rejected control command: {detail}"),
            ControlError::Busy(detail) => f.write_str(detail),
        }
    }
}

impl std::error::Error for ControlError {}

impl From<ControlError> for String {
    fn from(error: ControlError) -> Self {
        error.to_string()
    }
}

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
        transport: Default::default(),
        simulated: true,
        rssi: Some(-30),
        capabilities: vec![Capability::Ftms],
    }]
}

/// Knobs for making the simulated trainer misbehave on purpose, so link loss
/// and control failures can be exercised without hardware (unit tests and the
/// debug fault-injection command).
#[derive(Default)]
pub struct SimFaults {
    /// The next N control writes fail as if the GATT write errored
    /// (`u32::MAX` means every write).
    pub fail_writes: AtomicU32,
    /// How long the simulated GATT write takes; past `CONTROL_WRITE_TIMEOUT`
    /// it times out like a dead link.
    pub write_delay_ms: AtomicU64,
    /// How long the simulator waits before acknowledging a command; past
    /// `CONTROL_ACK_TIMEOUT` the command times out and the ack arrives late.
    pub ack_delay_ms: AtomicU64,
    /// FTMS result code the next acknowledgement carries instead of Success
    /// (4 = OperationFailed, 5 = ControlNotPermitted); consumed once.
    pub refuse_with: AtomicU8,
    /// The next N simulated connects fail.
    pub fail_connects: AtomicU32,
    /// Makes the simulator's worker task panic on its next tick (once).
    pub panic_worker: AtomicBool,
    /// Ends the simulator's telemetry loop, which is exactly what a real link
    /// loss looks like from the inside.
    pub drop_link: Notify,
    /// Every control payload the simulator has received, oldest first.
    pub commands: std::sync::Mutex<Vec<(tokio::time::Instant, Vec<u8>)>>,
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
    /// True while the built-in simulator is the connected trainer. Without it
    /// "no peripheral" would look like a trainer that accepts every command.
    simulated: Arc<AtomicBool>,
    faults: Arc<SimFaults>,
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
            simulated: Arc::new(AtomicBool::new(false)),
            faults: Arc::new(SimFaults::default()),
        }
    }

    pub fn slot(&self) -> &Arc<DeviceSlot> {
        &self.slot
    }

    /// Fault-injection knobs; only the simulator honors them.
    pub fn faults(&self) -> Arc<SimFaults> {
        self.faults.clone()
    }

    /// What `set_target_power` would send for `requested`, without sending it.
    pub fn clamp_target(&self, requested: u16, rider_max: u16) -> u16 {
        requested.clamp(
            self.min_power.load(Ordering::Relaxed),
            self.max_power.load(Ordering::Relaxed).min(rider_max),
        )
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
            let failures = self.faults.fail_connects.load(Ordering::Relaxed);
            if failures > 0 {
                self.faults
                    .fail_connects
                    .store(failures - 1, Ordering::Relaxed);
                return Err("Simulated connect failure".into());
            }
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
        let worker = spawn_link_worker(
            slot.clone(),
            fuser.clone(),
            DeviceRole::Trainer,
            reconnect_name,
            async move {
                let mut notifications = match peripheral.notifications().await {
                    Ok(stream) => stream,
                    Err(error) => {
                        tracing::error!(error = %error, "Could not open notification stream");
                        slot.note(
                            "error",
                            "Could not open notification stream",
                            Some(error.to_string()),
                        );
                        return 0;
                    }
                };
                tracing::debug!("Notification worker started");
                let mut samples: u64 = 0;
                let mut last_summary = std::time::Instant::now();
                while let Some(notification) = notifications.next().await {
                    if notification.uuid == bluetooth_uuid(INDOOR_BIKE_DATA) {
                        match parse_indoor_bike_data(
                            &notification.value,
                            Utc::now().timestamp_millis(),
                        ) {
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
                samples
            },
        );
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
        self.simulated.store(true, Ordering::Relaxed);
        self.slot
            .set_state(DeviceState::Ready {
                device: device.clone(),
            })
            .await;
        let target = self.target_power.clone();
        let slot = self.slot.clone();
        let fuser = self.fuser.clone();
        let faults = self.faults.clone();
        let simulated = self.simulated.clone();
        let name = device.name.clone();
        let worker = spawn_link_worker(
            slot.clone(),
            fuser.clone(),
            DeviceRole::Trainer,
            name,
            async move {
                let mut power = 90.0_f32;
                let mut tick = tokio::time::interval(Duration::from_millis(500));
                let mut first = true;
                let mut samples: u64 = 0;
                loop {
                    tokio::select! {
                        _ = tick.tick() => {}
                        _ = faults.drop_link.notified() => break,
                    }
                    samples += 1;
                    if faults.panic_worker.swap(false, Ordering::Relaxed) {
                        panic!("simulated trainer worker panic");
                    }
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
                // A dropped simulator is as gone as a dropped trainer.
                simulated.store(false, Ordering::Relaxed);
                samples
            },
        );
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

    pub async fn begin_control(&self) -> Result<(), ControlError> {
        if self.calibrating.load(Ordering::Relaxed) {
            return Err(ControlError::Busy(
                "Wait for trainer calibration to finish".into(),
            ));
        }
        let device = match self.slot.state().await {
            DeviceState::Ready { device } | DeviceState::Controlling { device } => device,
            other => {
                tracing::warn!(state = ?other, "begin_control called without a connected trainer");
                return Err(ControlError::NotConnected);
            }
        };
        tracing::info!(name = %device.name, "Starting/resuming trainer control");
        self.write_control(&start_or_resume()).await?;
        self.slot
            .set_state(DeviceState::Controlling { device })
            .await;
        Ok(())
    }

    /// Request Control + Start/Resume again. Needed when the trainer answers
    /// ControlNotPermitted mid-ride: another app or a firmware reset took
    /// control while the link stayed up.
    pub async fn reacquire_control(&self) -> Result<(), ControlError> {
        tracing::info!("Re-acquiring trainer control");
        self.write_control(&request_control()).await?;
        self.write_control(&start_or_resume()).await
    }

    pub async fn set_target_power(
        &self,
        requested: u16,
        rider_max: u16,
    ) -> Result<u16, ControlError> {
        if self.calibrating.load(Ordering::Relaxed) {
            return Err(ControlError::Busy(
                "Cannot change ERG power during trainer calibration".into(),
            ));
        }
        let clamped = self.clamp_target(requested, rider_max);
        if clamped != requested {
            tracing::debug!(requested, clamped, rider_max, "Target power clamped");
        }
        self.target_power.store(clamped, Ordering::Relaxed);
        self.write_control(&set_target_power(clamped)).await?;
        Ok(clamped)
    }

    pub async fn pause(&self) -> Result<(), ControlError> {
        tracing::info!("Pausing trainer");
        self.write_control(&stop_or_pause(true)).await
    }

    pub async fn stop(&self) -> Result<(), ControlError> {
        tracing::info!("Stopping trainer");
        self.target_power.store(0, Ordering::Relaxed);
        match self.write_control(&stop_or_pause(false)).await {
            // An already-stopped trainer answers OperationFailed or
            // ControlNotPermitted to Stop; that is the state we want.
            Err(ControlError::Refused(
                ResponseCode::OperationFailed | ResponseCode::ControlNotPermitted,
            )) => {
                tracing::debug!("Trainer was already stopped");
                Ok(())
            }
            other => other,
        }
    }

    async fn write_control(&self, payload: &[u8]) -> Result<(), ControlError> {
        let _guard = self.command_lock.lock().await;
        let response = self.write_control_locked(payload).await?;
        let expected_opcode = payload[0];
        match parse_control_response(&response, expected_opcode) {
            Ok(()) => Ok(()),
            Err(error) => {
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
                Err(match error {
                    FtmsError::Rejected { result, .. } => ControlError::Refused(result),
                    other => ControlError::Gatt(other.to_string()),
                })
            }
        }
    }

    /// Write one FTMS command while the caller holds `command_lock` and wait
    /// for the trainer's indication for it. Bounded by the control timeouts,
    /// and abandoned at once if the link goes down meanwhile.
    async fn write_control_locked(&self, payload: &[u8]) -> Result<Vec<u8>, ControlError> {
        let expected_opcode = payload
            .first()
            .copied()
            .ok_or_else(|| ControlError::Gatt("Cannot send an empty trainer command".into()))?;
        let mut state_rx = self.slot.subscribe_state();
        if state_rx.borrow().link_is_down() {
            return Err(ControlError::NotConnected);
        }
        let peripheral = self.peripheral.read().await.clone();
        let control = self.control_point.read().await.clone();
        let simulated = peripheral.is_none();
        if simulated && !self.simulated.load(Ordering::Relaxed) {
            return Err(ControlError::NotConnected);
        }
        let mut responses = self.control_responses.subscribe();
        tracing::debug!(opcode = format_args!("0x{expected_opcode:02x}"), payload = ?payload, "Writing control command");
        let started = tokio::time::Instant::now();

        let write = async {
            match (&peripheral, &control) {
                (Some(peripheral), Some(control)) => peripheral
                    .write(control, payload, WriteType::WithResponse)
                    .await
                    .map_err(|error| ControlError::Gatt(error.to_string())),
                _ => self.simulated_write(expected_opcode, payload).await,
            }
        };
        let written = tokio::select! {
            result = tokio::time::timeout(CONTROL_WRITE_TIMEOUT, write) => match result {
                Ok(result) => result,
                Err(_) => Err(ControlError::Timeout),
            },
            _ = state_rx.wait_for(DeviceState::link_is_down) => Err(ControlError::NotConnected),
        };
        if let Err(error) = written {
            tracing::error!(opcode = format_args!("0x{expected_opcode:02x}"), error = %error, "Control write failed");
            self.slot.note(
                "error",
                "Control write failed",
                Some(format!("op 0x{expected_opcode:02x} · {error}")),
            );
            return Err(error);
        }
        // FTMS sends the indication for a procedure after the ATT write
        // response, so anything already queued is a late answer to an earlier
        // (timed-out) command and must not be mistaken for this one's.
        loop {
            match responses.try_recv() {
                Ok(stale) => tracing::debug!(raw = ?stale, "Discarding stale control response"),
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
        if simulated {
            self.schedule_simulated_ack(expected_opcode);
        }
        let wait_for_ack = async {
            loop {
                match responses.recv().await {
                    Ok(response) if response.get(1) == Some(&expected_opcode) => {
                        return Ok(response);
                    }
                    Ok(other) => {
                        tracing::trace!(raw = ?other, "Ignoring response for another opcode")
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => {
                        return Err(ControlError::Gatt("Lost trainer response stream".into()));
                    }
                }
            }
        };
        let acknowledgement = tokio::select! {
            result = tokio::time::timeout(CONTROL_ACK_TIMEOUT, wait_for_ack) => match result {
                Ok(result) => result,
                Err(_) => Err(ControlError::Timeout),
            },
            _ = state_rx.wait_for(DeviceState::link_is_down) => Err(ControlError::NotConnected),
        };
        match &acknowledgement {
            Ok(_) => tracing::debug!(
                opcode = format_args!("0x{expected_opcode:02x}"),
                elapsed_ms = started.elapsed().as_millis() as u64,
                "Control command acknowledged"
            ),
            Err(ControlError::Timeout) => {
                tracing::error!(
                    opcode = format_args!("0x{expected_opcode:02x}"),
                    timeout_secs = CONTROL_ACK_TIMEOUT.as_secs(),
                    "No control response in time"
                );
                self.slot.note(
                    "error",
                    "Control command timed out",
                    Some(format!(
                        "op 0x{expected_opcode:02x} · no response in {} s",
                        CONTROL_ACK_TIMEOUT.as_secs()
                    )),
                );
            }
            Err(error) => tracing::warn!(
                opcode = format_args!("0x{expected_opcode:02x}"),
                error = %error,
                "Control command abandoned"
            ),
        }
        acknowledgement
    }

    /// The simulator's side of a control write: remember the command and
    /// honor the fault knobs. The acknowledgement is posted separately so it
    /// travels through the same response matching as a real trainer's.
    async fn simulated_write(&self, opcode: u8, payload: &[u8]) -> Result<(), ControlError> {
        self.faults
            .commands
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((tokio::time::Instant::now(), payload.to_vec()));
        let delay = self.faults.write_delay_ms.load(Ordering::Relaxed);
        if delay > 0 {
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }
        let remaining = self.faults.fail_writes.load(Ordering::Relaxed);
        if remaining > 0 {
            if remaining != u32::MAX {
                self.faults
                    .fail_writes
                    .store(remaining - 1, Ordering::Relaxed);
            }
            tracing::warn!(
                opcode = format_args!("0x{opcode:02x}"),
                "Simulated control write failure"
            );
            return Err(ControlError::Gatt("simulated write failure".into()));
        }
        Ok(())
    }

    fn schedule_simulated_ack(&self, opcode: u8) {
        let delay = Duration::from_millis(self.faults.ack_delay_ms.load(Ordering::Relaxed));
        let result = match self.faults.refuse_with.swap(0, Ordering::Relaxed) {
            0 => 0x01,
            code => code,
        };
        let responses = self.control_responses.clone();
        tokio::spawn(async move {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            let _ = responses.send(vec![0x80, opcode, result]);
        });
    }

    pub async fn disconnect(&self) {
        let _ = self.calibration_cancel.send(());
        let had_peripheral = self.peripheral.read().await.is_some();
        let was_connected = self.slot.state().await.is_connected();
        if had_peripheral {
            tracing::info!("Disconnecting trainer");
        }
        if self.slot.abort_worker().await {
            tracing::debug!("Trainer worker aborted");
        }
        // Only a live trainer is told to stop; a lost one cannot hear it and
        // waiting on it would only delay the reconnect.
        if was_connected
            && let Ok(Err(error)) = tokio::time::timeout(Duration::from_secs(3), self.stop()).await
        {
            tracing::debug!(error = %error, "Stop before disconnect failed (ignored)");
        }
        self.simulated.store(false, Ordering::Relaxed);
        if let Some(peripheral) = self.peripheral.write().await.take() {
            match tokio::time::timeout(Duration::from_secs(3), peripheral.disconnect()).await {
                Ok(Ok(())) => tracing::debug!("GATT disconnected"),
                Ok(Err(error)) => tracing::warn!(error = %error, "GATT disconnect failed"),
                Err(_) => tracing::warn!("GATT disconnect timed out"),
            }
        }
        *self.control_point.write().await = None;
        self.calibration_supported.store(false, Ordering::Relaxed);
        self.calibrating.store(false, Ordering::Relaxed);
        self.slot.set_calibration(Some(false), Some(false)).await;
        self.fuser.forget(DeviceRole::Trainer);
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
        // Nothing connected: commands must fail, not silently "succeed".
        assert_eq!(
            trainer.set_target_power(100, 800).await.unwrap_err(),
            ControlError::NotConnected
        );
        assert_eq!(trainer.clamp_target(900, 800), 800);
        trainer
            .connect(simulated_devices().remove(0))
            .await
            .unwrap();
        assert_eq!(trainer.set_target_power(100, 800).await.unwrap(), 100);
        assert_eq!(trainer.set_target_power(900, 800).await.unwrap(), 800);
        assert_eq!(trainer.target_power.load(Ordering::Relaxed), 800);
        trainer.disconnect().await;
    }

    #[tokio::test]
    async fn simulated_faults_fail_writes_and_drop_the_link() {
        let hub = super::super::DeviceHub::default();
        hub.connect(DeviceRole::Trainer, simulated_devices().remove(0))
            .await
            .unwrap();
        let faults = hub.simulated_faults();
        faults.fail_writes.store(2, Ordering::Relaxed);
        assert!(hub.set_target_power(150, 800).await.is_err());
        assert!(hub.set_target_power(150, 800).await.is_err());
        assert_eq!(hub.set_target_power(150, 800).await.unwrap(), 150);
        assert_eq!(
            faults
                .commands
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, payload)| payload.first() == Some(&0x05))
                .count(),
            3
        );
        faults.drop_link.notify_one();
        for _ in 0..40 {
            if matches!(
                hub.slot(DeviceRole::Trainer).state().await,
                DeviceState::Reconnecting { .. }
            ) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(matches!(
            hub.slot(DeviceRole::Trainer).state().await,
            DeviceState::Reconnecting { .. }
        ));
        assert_eq!(hub.slot(DeviceRole::Trainer).stats().drops, 1);
        // The link is gone, so control writes fail instead of pretending.
        assert_eq!(
            hub.set_target_power(150, 800).await.unwrap_err(),
            ControlError::NotConnected
        );
        hub.disconnect().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_late_ack_is_not_mistaken_for_the_next_commands_answer() {
        let hub = super::super::DeviceHub::default();
        hub.connect(DeviceRole::Trainer, simulated_devices().remove(0))
            .await
            .unwrap();
        let faults = hub.simulated_faults();
        // Command 1: the trainer answers only after the ack budget.
        faults.ack_delay_ms.store(6_000, Ordering::Relaxed);
        let started = tokio::time::Instant::now();
        assert_eq!(
            hub.set_target_power(200, 800).await.unwrap_err(),
            ControlError::Timeout
        );
        let waited = started.elapsed();
        assert!(
            waited >= CONTROL_ACK_TIMEOUT
                && waited < CONTROL_ACK_TIMEOUT + Duration::from_millis(100),
            "{waited:?}"
        );
        // Command 2 goes out while that late success is still on its way (its
        // write takes 3 s) and the trainer refuses it. Without the stale-ack
        // drain the late success would be taken as command 2's answer.
        faults.ack_delay_ms.store(0, Ordering::Relaxed);
        faults.write_delay_ms.store(3_000, Ordering::Relaxed);
        faults.refuse_with.store(4, Ordering::Relaxed);
        assert_eq!(
            hub.set_target_power(205, 800).await.unwrap_err(),
            ControlError::Refused(ResponseCode::OperationFailed)
        );
        faults.write_delay_ms.store(0, Ordering::Relaxed);
        hub.disconnect().await;
    }

    #[tokio::test(start_paused = true)]
    async fn link_loss_abandons_an_in_flight_write_at_once() {
        let hub = Arc::new(super::super::DeviceHub::default());
        hub.connect(DeviceRole::Trainer, simulated_devices().remove(0))
            .await
            .unwrap();
        let faults = hub.simulated_faults();
        faults.write_delay_ms.store(3_000, Ordering::Relaxed);
        let writer = {
            let hub = hub.clone();
            tokio::spawn(async move {
                let started = tokio::time::Instant::now();
                (hub.set_target_power(200, 800).await, started.elapsed())
            })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        faults.drop_link.notify_one();
        let (result, elapsed) = writer.await.unwrap();
        assert_eq!(result.unwrap_err(), ControlError::NotConnected);
        assert!(elapsed < Duration::from_secs(1), "{elapsed:?}");
        hub.disconnect().await;
    }

    #[tokio::test]
    async fn a_worker_panic_is_handled_as_a_link_loss() {
        let hub = super::super::DeviceHub::default();
        hub.connect(DeviceRole::Trainer, simulated_devices().remove(0))
            .await
            .unwrap();
        hub.simulated_faults()
            .panic_worker
            .store(true, Ordering::Relaxed);
        for _ in 0..60 {
            if matches!(
                hub.slot(DeviceRole::Trainer).state().await,
                DeviceState::Reconnecting { .. }
            ) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(matches!(
            hub.slot(DeviceRole::Trainer).state().await,
            DeviceState::Reconnecting { .. }
        ));
        assert!(
            hub.slot(DeviceRole::Trainer)
                .log_lines()
                .iter()
                .any(|line| line.step == "Device worker crashed")
        );
        hub.disconnect().await;
    }

    #[tokio::test]
    async fn refusals_are_classified_and_stop_tolerates_already_stopped() {
        let hub = super::super::DeviceHub::default();
        hub.connect(DeviceRole::Trainer, simulated_devices().remove(0))
            .await
            .unwrap();
        let faults = hub.simulated_faults();
        faults.refuse_with.store(5, Ordering::Relaxed);
        assert_eq!(
            hub.set_target_power(200, 800).await.unwrap_err(),
            ControlError::Refused(ResponseCode::ControlNotPermitted)
        );
        faults.refuse_with.store(4, Ordering::Relaxed);
        hub.stop().await.unwrap();
        hub.reacquire_control().await.unwrap();
        let opcodes: Vec<u8> = faults
            .commands
            .lock()
            .unwrap()
            .iter()
            .map(|(_, payload)| payload[0])
            .collect();
        assert_eq!(&opcodes[opcodes.len() - 2..], &[0x00, 0x07]);
        hub.disconnect().await;
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
