//! A sensor slot (heart rate, power meter, cadence sensor): connect, subscribe
//! to one measurement characteristic, decode notifications into `Reading`s for
//! the fuser, and, when the sensor kind has a control point, run procedures on
//! it (a power meter's zero offset). Each sensor kind supplies a `Decoder`;
//! everything else — GATT steps, logging, stats, the simulator loop, the
//! control-point plumbing — is shared here.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
    },
    time::Duration,
};

use btleplug::{
    api::{Characteristic, Peripheral as _, WriteType},
    platform::Peripheral,
};
use chrono::Utc;
use futures::StreamExt;
use tokio::sync::{Mutex, Notify, RwLock, broadcast};

use super::{
    DeviceInfo, DeviceRole, DeviceSlot, DeviceState,
    ble::{self, Ble, GATT_CONNECT_TIMEOUT, GATT_STEP_TIMEOUT, bluetooth_uuid, hex, with_timeout},
    control_point::{ControlError, run_procedure},
    fuser::{Reading, TelemetryFuser},
    spawn_ble_link_worker, spawn_link_worker,
};

/// How long the simulator takes to answer a procedure, so the dialog's
/// "hold still" phase is visible in developer mode.
const SIMULATED_PROCEDURE_DELAY: Duration = Duration::from_millis(1_500);

/// The control point a sensor kind takes commands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlPointSpec {
    /// The indicate characteristic commands are written to (16-bit UUID).
    pub characteristic: u16,
    /// The read-once feature characteristic that says which procedures the
    /// device supports, when the profile has one.
    pub feature: Option<u16>,
    pub name: &'static str,
}

/// Kind-specific knowledge for one sensor role.
pub trait Decoder: Send + Sync + 'static {
    /// The measurement characteristic to subscribe to (16-bit UUID).
    fn characteristic(&self) -> u16;
    /// Human name of that characteristic, for the log.
    fn characteristic_name(&self) -> &'static str;
    /// Parse one notification. `Ok(None)` means "valid but nothing to report
    /// yet" (e.g. a crank-only packet before two revolutions have passed).
    fn decode(&mut self, data: &[u8], now_ms: i64) -> Result<Option<Reading>, String>;
    /// One-line summary of a reading for the "first sample" log entry.
    fn describe(&self, reading: &Reading) -> String;
    /// Simulated reading; `tick` counts 500 ms steps since connect.
    fn simulate(&mut self, tick: u64, now_ms: i64) -> Reading;
    /// Called when a link starts so per-connection state (e.g. crank deltas)
    /// resets.
    fn reset(&mut self) {}
    /// The control point this sensor kind runs procedures on, if any.
    fn control_point(&self) -> Option<ControlPointSpec> {
        None
    }
    /// The feature characteristic's bytes, read once at connect. Returns a
    /// one-line summary for the log.
    fn features(&mut self, _bytes: &[u8]) -> Result<String, String> {
        Err("this sensor kind has no feature characteristic".into())
    }
    /// Whether the features read (or their absence) allow the calibration
    /// procedure. Only meaningful when `control_point` is `Some`.
    fn calibration_supported(&self) -> bool {
        false
    }
    /// Whether the latest measurement carried the device's own request to be
    /// calibrated.
    fn calibration_requested(&self) -> bool {
        false
    }
    /// The simulator's answer to a procedure request, carrying `result` as
    /// the response value (1 = success). `None` means the simulator never
    /// answers, which is what a timeout looks like.
    fn simulate_procedure(&mut self, _payload: &[u8], _result: u8) -> Option<Vec<u8>> {
        None
    }
}

/// Knobs for making a simulated sensor misbehave during procedures (unit
/// tests and the debug fault-injection command).
#[derive(Default)]
pub struct SensorFaults {
    /// Response value the next simulated procedure answer carries instead of
    /// success (Cycling Power: 2 = not supported, 4 = operation failed);
    /// consumed once.
    pub refuse_with: AtomicU8,
    /// Extra delay before the simulated answer, on top of the built-in one;
    /// past the procedure's timeout the answer arrives late.
    pub ack_delay_ms: AtomicU64,
}

type SharedDecoder = Arc<std::sync::Mutex<Box<dyn Decoder>>>;

pub struct Sensor {
    slot: Arc<DeviceSlot>,
    ble: Arc<Ble>,
    fuser: Arc<TelemetryFuser>,
    make_decoder: fn(&DeviceInfo) -> Box<dyn Decoder>,
    peripheral: RwLock<Option<Peripheral>>,
    /// Shared with the notification worker, which decodes under the lock,
    /// so procedures can consult the same state (features, simulator).
    decoder: RwLock<Option<SharedDecoder>>,
    control_point: RwLock<Option<Characteristic>>,
    /// Indications from the control point, matched to requests by
    /// `control_point::run_procedure`.
    procedure_responses: broadcast::Sender<Vec<u8>>,
    /// One procedure at a time per sensor.
    command_lock: Mutex<()>,
    procedure_cancel: Notify,
    /// A calibration is in progress (guards against a second one starting).
    calibrating: AtomicBool,
    simulated: AtomicBool,
    faults: Arc<SensorFaults>,
}

impl Sensor {
    pub fn new(
        slot: Arc<DeviceSlot>,
        ble: Arc<Ble>,
        fuser: Arc<TelemetryFuser>,
        make_decoder: fn(&DeviceInfo) -> Box<dyn Decoder>,
    ) -> Self {
        let (procedure_responses, _) = broadcast::channel(16);
        Self {
            slot,
            ble,
            fuser,
            make_decoder,
            peripheral: RwLock::new(None),
            decoder: RwLock::new(None),
            control_point: RwLock::new(None),
            procedure_responses,
            command_lock: Mutex::new(()),
            procedure_cancel: Notify::new(),
            calibrating: AtomicBool::new(false),
            simulated: AtomicBool::new(false),
            faults: Arc::new(SensorFaults::default()),
        }
    }

    pub fn slot(&self) -> &Arc<DeviceSlot> {
        &self.slot
    }

    /// Fault-injection knobs; only the simulator honors them.
    pub fn faults(&self) -> Arc<SensorFaults> {
        self.faults.clone()
    }

    fn role(&self) -> DeviceRole {
        self.slot.role
    }

    pub async fn connect(&self, device: DeviceInfo) -> Result<(), String> {
        let role = self.role();
        tracing::info!(?role, id = %device.id, name = %device.name, simulated = device.simulated, "Connecting sensor");
        let started = std::time::Instant::now();
        let result = self.connect_inner(device.clone()).await;
        match &result {
            Ok(()) => tracing::info!(
                ?role,
                name = %device.name,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "Sensor connected"
            ),
            Err(error) => {
                tracing::error!(?role, name = %device.name, error = %error, "Sensor connection failed");
                self.slot
                    .progress("error", "Connection failed", Some(error.clone()));
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
        self.slot
            .set_state(DeviceState::Connecting {
                name: device.name.clone(),
            })
            .await;
        let mut decoder = (self.make_decoder)(&device);
        decoder.reset();
        let spec = decoder.control_point();
        if device.simulated {
            self.slot
                .progress("info", "Starting simulated sensor", None);
            tokio::time::sleep(Duration::from_millis(250)).await;
            self.slot.progress(
                "ok",
                "Simulated sensor online",
                Some(decoder.characteristic_name().into()),
            );
            self.simulated.store(true, Ordering::Relaxed);
            self.slot
                .set_calibration(Some(spec.is_some()), Some(false))
                .await;
            self.start_simulator(device, decoder).await;
            return Ok(());
        }
        let label = self.role().label().to_lowercase();
        self.slot.progress(
            "info",
            "Locating device from last scan",
            Some(device.id.clone()),
        );
        let peripheral = self
            .ble
            .peripheral(&device.id)
            .await
            .ok_or_else(|| format!("{} is no longer available; scan again", device.name))?;
        self.slot.progress(
            "info",
            "Opening GATT link",
            device.rssi.map(|rssi| format!("signal {rssi} dBm")),
        );
        with_timeout(GATT_CONNECT_TIMEOUT, "GATT connect", peripheral.connect())
            .await
            .map_err(|error| format!("Could not connect to {label}: {error}"))?;
        self.slot.progress("ok", "Link established", None);
        self.slot
            .progress("info", "Discovering GATT services", None);
        with_timeout(
            GATT_STEP_TIMEOUT,
            "service discovery",
            peripheral.discover_services(),
        )
        .await
        .map_err(|error| format!("Could not discover {label} services: {error}"))?;
        let characteristics = peripheral.characteristics();
        tracing::debug!(?characteristics, "Services discovered");
        self.slot.progress(
            "ok",
            "Services discovered",
            Some(format!(
                "{} services · {} characteristics",
                peripheral.services().len(),
                characteristics.len()
            )),
        );
        let find = |short: u16| {
            characteristics
                .iter()
                .find(|characteristic| characteristic.uuid == bluetooth_uuid(short))
                .cloned()
        };
        let measurement = find(decoder.characteristic()).ok_or_else(|| {
            format!(
                "{} does not expose {}",
                device.name,
                decoder.characteristic_name()
            )
        })?;
        self.slot.progress(
            "ok",
            decoder.characteristic_name(),
            Some(format!("0x{:04X}", decoder.characteristic())),
        );
        let details = ble::read_details(&peripheral).await;
        match details.summary() {
            Some(summary) => self
                .slot
                .progress("ok", "Device information", Some(summary)),
            None => self
                .slot
                .progress("warn", "No battery or device information", None),
        }

        // Control point: read the features, arm its indications.
        let control = match spec {
            Some(spec) => match find(spec.characteristic) {
                Some(control) => {
                    self.slot.progress(
                        "ok",
                        spec.name,
                        Some(format!("0x{:04X}", spec.characteristic)),
                    );
                    self.read_features(&peripheral, &spec, &find, &mut decoder)
                        .await;
                    with_timeout(
                        GATT_STEP_TIMEOUT,
                        "control point subscribe",
                        peripheral.subscribe(&control),
                    )
                    .await
                    .map_err(|error| format!("Could not arm the {label} control point: {error}"))?;
                    self.slot.progress("ok", "Control point armed", None);
                    Some(control)
                }
                None => {
                    self.slot.progress(
                        "warn",
                        "No control point",
                        Some("calibration unavailable".into()),
                    );
                    None
                }
            },
            None => None,
        };
        let calibration_supported = control.is_some() && decoder.calibration_supported();

        self.slot
            .progress("info", "Subscribing to measurements", None);
        with_timeout(
            GATT_STEP_TIMEOUT,
            "measurement subscribe",
            peripheral.subscribe(&measurement),
        )
        .await
        .map_err(|error| format!("Could not subscribe to {label}: {error}"))?;
        self.slot.progress("ok", "Notifications armed", None);
        *self.peripheral.write().await = Some(peripheral.clone());
        let control_uuid = control.as_ref().map(|control| control.uuid);
        *self.control_point.write().await = control;
        let decoder: SharedDecoder = Arc::new(std::sync::Mutex::new(decoder));
        *self.decoder.write().await = Some(decoder.clone());
        self.slot.record_connected(device.rssi, &details);
        self.slot
            .set_calibration(Some(calibration_supported), Some(false))
            .await;

        let slot = self.slot.clone();
        let fuser = self.fuser.clone();
        let role = self.role();
        let reconnect_name = device.name.clone();
        let measurement_uuid = measurement.uuid;
        let responses = self.procedure_responses.clone();
        let worker = spawn_ble_link_worker(
            slot.clone(),
            fuser.clone(),
            role,
            reconnect_name,
            peripheral.clone(),
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
                let mut samples: u64 = 0;
                let mut last_summary = std::time::Instant::now();
                while let Some(notification) = notifications.next().await {
                    if Some(notification.uuid) == control_uuid {
                        tracing::debug!(raw = ?notification.value, "Control point response");
                        let _ = responses.send(notification.value);
                        continue;
                    }
                    if notification.uuid != measurement_uuid {
                        tracing::trace!(uuid = %notification.uuid, raw = ?notification.value, "Other notification");
                        continue;
                    }
                    let now = Utc::now().timestamp_millis();
                    let decoded = {
                        let mut decoder = lock_decoder(&decoder);
                        decoder.decode(&notification.value, now).map(|reading| {
                            let summary = reading.as_ref().map(|reading| decoder.describe(reading));
                            (reading, summary, decoder.calibration_requested())
                        })
                    };
                    match decoded {
                        Ok((reading, summary, requested)) => {
                            samples += 1;
                            slot.record_sample(Some(&notification.value));
                            if slot.set_calibration_requested(requested) && requested {
                                slot.note("warn", "Device requests calibration", None);
                            }
                            if let (Some(reading), Some(summary)) = (reading, summary) {
                                if samples == 1 {
                                    slot.note("ok", "First sample received", Some(summary.clone()));
                                }
                                slot.record_reading(summary);
                                fuser.ingest(role, reading, now);
                            }
                            if last_summary.elapsed() >= Duration::from_secs(60) {
                                last_summary = std::time::Instant::now();
                                let stats = slot.stats();
                                slot.note(
                                    "info",
                                    "Measurements flowing",
                                    Some(format!(
                                        "{} samples · {:.1} Hz · {} parse failures",
                                        stats.samples, stats.rate_hz, stats.parse_failures
                                    )),
                                );
                            }
                        }
                        Err(error) => {
                            let failures = slot.record_parse_failure(&notification.value);
                            if failures <= 5 || failures.is_multiple_of(100) {
                                tracing::warn!(?role, failures, error = %error, raw = ?notification.value, "Could not parse measurement");
                                slot.note(
                                    "warn",
                                    "Could not parse measurement",
                                    Some(format!("{error} · {}", hex(&notification.value))),
                                );
                            }
                        }
                    }
                }
                samples
            },
        );
        self.slot.set_worker(worker).await;
        self.slot.set_state(DeviceState::Ready { device }).await;
        Ok(())
    }

    /// Read the profile's feature characteristic and hand it to the decoder.
    /// Never fails the connect: an unreadable feature word is logged and the
    /// decoder decides what to assume.
    async fn read_features(
        &self,
        peripheral: &Peripheral,
        spec: &ControlPointSpec,
        find: &impl Fn(u16) -> Option<Characteristic>,
        decoder: &mut Box<dyn Decoder>,
    ) {
        let Some(feature_uuid) = spec.feature else {
            return;
        };
        let Some(feature) = find(feature_uuid) else {
            self.slot.progress(
                "warn",
                "No feature characteristic",
                Some(format!("0x{feature_uuid:04X} missing")),
            );
            return;
        };
        match with_timeout(GATT_STEP_TIMEOUT, "feature read", peripheral.read(&feature)).await {
            Ok(bytes) => match decoder.features(&bytes) {
                Ok(summary) => self.slot.progress("ok", "Features", Some(summary)),
                Err(error) => self.slot.progress(
                    "warn",
                    "Features unreadable",
                    Some(format!("{error} · {}", hex(&bytes))),
                ),
            },
            Err(error) => self
                .slot
                .progress("warn", "Feature read failed", Some(error)),
        }
    }

    async fn start_simulator(&self, device: DeviceInfo, decoder: Box<dyn Decoder>) {
        self.slot.record_connected(
            device.rssi,
            &ble::DeviceDetails {
                battery_percent: Some(100),
                manufacturer: Some("BlakeBike".into()),
                model: Some("Simulator".into()),
                firmware: Some(env!("CARGO_PKG_VERSION").into()),
            },
        );
        let decoder: SharedDecoder = Arc::new(std::sync::Mutex::new(decoder));
        *self.decoder.write().await = Some(decoder.clone());
        self.slot
            .set_state(DeviceState::Ready {
                device: device.clone(),
            })
            .await;
        let slot = self.slot.clone();
        let fuser = self.fuser.clone();
        let role = self.role();
        let worker = spawn_link_worker(
            slot.clone(),
            fuser.clone(),
            role,
            device.name.clone(),
            async move {
                let mut tick = tokio::time::interval(Duration::from_millis(500));
                let mut count: u64 = 0;
                loop {
                    tick.tick().await;
                    let now = Utc::now().timestamp_millis();
                    let (reading, summary, requested) = {
                        let mut decoder = lock_decoder(&decoder);
                        let reading = decoder.simulate(count, now);
                        let summary = decoder.describe(&reading);
                        (reading, summary, decoder.calibration_requested())
                    };
                    slot.record_sample(None);
                    slot.set_calibration_requested(requested);
                    if count == 0 {
                        slot.note(
                            "ok",
                            "First sample received",
                            Some(format!("simulated · {summary}")),
                        );
                    }
                    slot.record_reading(summary);
                    fuser.ingest(role, reading, now);
                    count += 1;
                }
            },
        );
        self.slot.set_worker(worker).await;
    }

    /// Claim the calibration slot; false when one is already running.
    pub fn begin_calibration(&self) -> bool {
        self.calibrating
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
            .is_ok()
    }

    pub fn end_calibration(&self) {
        self.calibrating.store(false, Ordering::SeqCst);
    }

    /// Stop waiting on the procedure in flight, if any. The device's late
    /// answer is discarded as stale by the next procedure.
    pub fn cancel_procedure(&self) {
        self.procedure_cancel.notify_waiters();
    }

    /// Resolves when `cancel_procedure` is called while awaited.
    pub async fn cancelled(&self) {
        self.procedure_cancel.notified().await;
    }

    /// Write one command to the sensor's control point and wait up to
    /// `ack_timeout` for the indication answering it. Serialised per sensor.
    pub async fn procedure(
        &self,
        payload: &[u8],
        ack_timeout: Duration,
    ) -> Result<Vec<u8>, ControlError> {
        let opcode = payload
            .first()
            .copied()
            .ok_or_else(|| ControlError::Gatt("Cannot send an empty command".into()))?;
        let _guard = self.command_lock.lock().await;
        let has_spec = match self.decoder.read().await.as_ref() {
            Some(decoder) => lock_decoder(decoder).control_point().is_some(),
            None => return Err(ControlError::NotConnected),
        };
        if !has_spec {
            return Err(ControlError::Unsupported(format!(
                "{} has no control point",
                self.role().label()
            )));
        }
        let peripheral = self.peripheral.read().await.clone();
        let control = self.control_point.read().await.clone();
        let simulated = self.simulated.load(Ordering::Relaxed);
        if !simulated {
            if peripheral.is_none() {
                return Err(ControlError::NotConnected);
            }
            if control.is_none() {
                return Err(ControlError::Unsupported(format!(
                    "This {} exposes no control point",
                    self.role().label().to_lowercase()
                )));
            }
        }
        let write = async {
            match (&peripheral, &control) {
                (Some(peripheral), Some(control)) => peripheral
                    .write(control, payload, WriteType::WithResponse)
                    .await
                    .map_err(|error| ControlError::Gatt(error.to_string())),
                _ => Ok(()),
            }
        };
        run_procedure(
            &self.slot,
            &self.procedure_responses,
            opcode,
            ack_timeout,
            write,
            || {
                if simulated {
                    self.schedule_simulated_response(payload);
                }
            },
        )
        .await
    }

    /// The simulator's side of a procedure: ask the decoder for its answer
    /// (honoring the refusal knob) and post it after a delay, so it travels
    /// through the same response matching as a real device's indication.
    fn schedule_simulated_response(&self, payload: &[u8]) {
        let result = match self.faults.refuse_with.swap(0, Ordering::Relaxed) {
            0 => 0x01,
            code => code,
        };
        let delay = SIMULATED_PROCEDURE_DELAY
            + Duration::from_millis(self.faults.ack_delay_ms.load(Ordering::Relaxed));
        let response = self.decoder.try_read().ok().and_then(|decoder| {
            decoder
                .as_ref()
                .and_then(|decoder| lock_decoder(decoder).simulate_procedure(payload, result))
        });
        let Some(response) = response else {
            tracing::debug!(payload = ?payload, "Simulator does not answer this procedure");
            return;
        };
        let responses = self.procedure_responses.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let _ = responses.send(response);
        });
    }

    pub async fn disconnect(&self) {
        let had_peripheral = self.peripheral.read().await.is_some();
        if self.slot.abort_worker().await {
            tracing::debug!(role = ?self.role(), "Sensor worker aborted");
        }
        if let Some(peripheral) = self.peripheral.write().await.take() {
            match peripheral.disconnect().await {
                Ok(()) => tracing::debug!("GATT disconnected"),
                Err(error) => tracing::warn!(error = %error, "GATT disconnect failed"),
            }
        }
        *self.control_point.write().await = None;
        *self.decoder.write().await = None;
        self.simulated.store(false, Ordering::Relaxed);
        self.fuser.forget(self.role());
        let was_connected = self.slot.state().await.is_connected();
        if had_peripheral || was_connected {
            self.slot.note("info", "Disconnected", None);
        }
        self.slot.record_disconnected();
        if self.slot.stats().calibration_supported || self.slot.stats().calibrating {
            self.slot.set_calibration(Some(false), Some(false)).await;
        }
        self.slot.set_calibration_requested(false);
        if !matches!(self.slot.state().await, DeviceState::Idle) {
            self.slot.set_state(DeviceState::Idle).await;
        }
    }
}

fn lock_decoder(decoder: &SharedDecoder) -> std::sync::MutexGuard<'_, Box<dyn Decoder>> {
    decoder
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
