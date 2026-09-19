//! A notify-only sensor slot (heart rate, power meter, cadence sensor): connect,
//! subscribe to one measurement characteristic, decode notifications into
//! `Reading`s for the fuser. Each sensor kind supplies a `Decoder`; everything
//! else — GATT steps, logging, stats, the simulator loop — is shared here.

use std::{sync::Arc, time::Duration};

use btleplug::{api::Peripheral as _, platform::Peripheral};
use chrono::Utc;
use futures::StreamExt;
use tokio::sync::RwLock;

use super::{
    DeviceInfo, DeviceRole, DeviceSlot, DeviceState,
    ble::{self, Ble, GATT_CONNECT_TIMEOUT, GATT_STEP_TIMEOUT, bluetooth_uuid, hex, with_timeout},
    fuser::{Reading, TelemetryFuser},
    spawn_link_worker,
};

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
}

pub struct Sensor {
    slot: Arc<DeviceSlot>,
    ble: Arc<Ble>,
    fuser: Arc<TelemetryFuser>,
    make_decoder: fn(&DeviceInfo) -> Box<dyn Decoder>,
    peripheral: RwLock<Option<Peripheral>>,
}

impl Sensor {
    pub fn new(
        slot: Arc<DeviceSlot>,
        ble: Arc<Ble>,
        fuser: Arc<TelemetryFuser>,
        make_decoder: fn(&DeviceInfo) -> Box<dyn Decoder>,
    ) -> Self {
        Self {
            slot,
            ble,
            fuser,
            make_decoder,
            peripheral: RwLock::new(None),
        }
    }

    pub fn slot(&self) -> &Arc<DeviceSlot> {
        &self.slot
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
        if device.simulated {
            self.slot
                .progress("info", "Starting simulated sensor", None);
            tokio::time::sleep(Duration::from_millis(250)).await;
            self.slot.progress(
                "ok",
                "Simulated sensor online",
                Some(decoder.characteristic_name().into()),
            );
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
        let measurement = characteristics
            .iter()
            .find(|characteristic| characteristic.uuid == bluetooth_uuid(decoder.characteristic()))
            .cloned()
            .ok_or_else(|| {
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
        self.slot.record_connected(device.rssi, &details);

        let slot = self.slot.clone();
        let fuser = self.fuser.clone();
        let role = self.role();
        let reconnect_name = device.name.clone();
        let measurement_uuid = measurement.uuid;
        let worker = spawn_link_worker(
            slot.clone(),
            fuser.clone(),
            role,
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
                let mut samples: u64 = 0;
                let mut last_summary = std::time::Instant::now();
                while let Some(notification) = notifications.next().await {
                    if notification.uuid != measurement_uuid {
                        tracing::trace!(uuid = %notification.uuid, raw = ?notification.value, "Other notification");
                        continue;
                    }
                    let now = Utc::now().timestamp_millis();
                    match decoder.decode(&notification.value, now) {
                        Ok(reading) => {
                            samples += 1;
                            slot.record_sample(Some(&notification.value));
                            if let Some(reading) = reading {
                                let summary = decoder.describe(&reading);
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

    async fn start_simulator(&self, device: DeviceInfo, mut decoder: Box<dyn Decoder>) {
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
                    let reading = decoder.simulate(count, now);
                    slot.record_sample(None);
                    let summary = decoder.describe(&reading);
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
        self.fuser.forget(self.role());
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
