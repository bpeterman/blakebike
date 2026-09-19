//! The devices hub: one slot per device role (trainer, heart rate, power,
//! cadence), each with its own connection state, live statistics and log.
//! Everything downstream (workout runner, ride recorder, ride screen) consumes
//! a single fused `Telemetry` stream published by the hub.

pub mod ant;
pub mod ble;
pub mod cadence;
pub mod crank;
pub mod cycling_power;
pub mod fuser;
pub mod heart_rate;
pub mod log;
pub mod sensor;
pub mod trainer;

use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    panic::AssertUnwindSafe,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use btleplug::{api::Peripheral as _, platform::Peripheral};
use futures::FutureExt;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::{
    sync::{Mutex, broadcast, watch},
    task::JoinHandle,
};

pub use ble::{Capability, DeviceInfo, DeviceTransport};
pub use fuser::{SourcePreferences, TelemetrySources};
pub use log::DeviceLogLine;
pub use trainer::ControlError;

use crate::domain::Telemetry;
use fuser::TelemetryFuser;
use log::DeviceLog;

/// The job a connected device does. One device per role at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeviceRole {
    Trainer,
    HeartRate,
    Power,
    Cadence,
}

impl DeviceRole {
    pub const ALL: [DeviceRole; 4] = [
        DeviceRole::Trainer,
        DeviceRole::HeartRate,
        DeviceRole::Power,
        DeviceRole::Cadence,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DeviceRole::Trainer => "Trainer",
            DeviceRole::HeartRate => "Heart rate",
            DeviceRole::Power => "Power meter",
            DeviceRole::Cadence => "Cadence sensor",
        }
    }

    /// Which advertised services qualify a device for this role.
    pub fn accepts(self, capability: Capability) -> bool {
        matches!(
            (self, capability),
            (DeviceRole::Trainer, Capability::Ftms)
                | (DeviceRole::HeartRate, Capability::HeartRate)
                | (DeviceRole::Power, Capability::CyclingPower)
                | (
                    DeviceRole::Cadence,
                    Capability::CyclingPower | Capability::Csc
                )
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum DeviceState {
    Idle,
    Scanning,
    Connecting { name: String },
    Ready { device: DeviceInfo },
    Controlling { device: DeviceInfo },
    Reconnecting { name: String },
    Error { message: String, guidance: String },
}

impl DeviceState {
    pub fn device(&self) -> Option<&DeviceInfo> {
        match self {
            DeviceState::Ready { device } | DeviceState::Controlling { device } => Some(device),
            _ => None,
        }
    }

    pub fn is_connected(&self) -> bool {
        self.device().is_some()
    }

    /// True when there is no link to wait on: nothing connected, the link was
    /// lost, or the last attempt failed. `Connecting` is not "down": commands
    /// are exchanged during connection.
    pub fn link_is_down(&self) -> bool {
        matches!(
            self,
            DeviceState::Idle | DeviceState::Reconnecting { .. } | DeviceState::Error { .. }
        )
    }
}

/// One line of the live connection readout shown in the trainer dialog.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectProgress {
    pub step: String,
    pub detail: Option<String>,
    pub level: &'static str,
}

/// High-level numbers about a connected device, for the hub cards.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SlotStats {
    pub samples: u64,
    pub parse_failures: u64,
    pub last_sample_ms: Option<i64>,
    /// Notifications per second over the last few seconds.
    pub rate_hz: f32,
    pub rssi: Option<i16>,
    pub battery_percent: Option<u8>,
    pub battery_status: Option<String>,
    pub battery_voltage: Option<f32>,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub firmware: Option<String>,
    pub connected_since_ms: Option<i64>,
    /// Link losses since the app started.
    pub drops: u32,
    /// Which automatic reconnect attempt is in progress (0 when none).
    pub reconnect_attempt: u32,
    pub last_raw_hex: Option<String>,
    /// Human summary of the latest decoded reading, e.g. "215 W · 88 rpm".
    pub last_reading: Option<String>,
    /// Whether this trainer advertises the FTMS Spin Down Control feature.
    pub calibration_supported: bool,
    /// Whether a trainer spin-down procedure is currently running.
    pub calibrating: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotSnapshot {
    pub role: DeviceRole,
    pub state: DeviceState,
    pub stats: SlotStats,
    pub log: Vec<DeviceLogLine>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevicesSnapshot {
    pub scanning: bool,
    pub scan_error: Option<ScanError>,
    pub ant_adapter: ant::AdapterStatus,
    pub slots: Vec<SlotSnapshot>,
    pub source_preferences: SourcePreferences,
    pub sources: TelemetrySources,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanError {
    pub message: String,
    pub guidance: String,
}

/// A device the user connected before, kept so it can be reconnected with one
/// click and without a full scan.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KnownDevice {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub transport: DeviceTransport,
    pub role: DeviceRole,
    #[serde(default)]
    pub capabilities: Vec<Capability>,
    pub simulated: bool,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub last_connected_at: chrono::DateTime<Utc>,
}

impl KnownDevice {
    /// The device as a connect needs it. A remembered device has no current
    /// advertisement, so its signal strength is unknown.
    pub fn to_device_info(&self) -> DeviceInfo {
        DeviceInfo {
            id: self.id.clone(),
            name: self.name.clone(),
            transport: self.transport,
            simulated: self.simulated,
            rssi: None,
            capabilities: self.capabilities.clone(),
        }
    }
}

/// What happened to one remembered device during a connect-all.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KnownConnectOutcome {
    pub role: DeviceRole,
    pub name: String,
    pub status: KnownConnectStatus,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum KnownConnectStatus {
    Connected,
    /// The role already had a device connected or connecting; left alone.
    Skipped,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceLogEvent {
    pub role: DeviceRole,
    pub line: DeviceLogLine,
}

const RATE_WINDOW_MS: i64 = 5_000;
const STATS_EMIT_INTERVAL_MS: i64 = 1_000;

#[derive(Default)]
struct StatsInner {
    stats: SlotStats,
    window: VecDeque<i64>,
    last_emit_ms: i64,
}

/// Connection state, statistics, log and worker for one role.
pub struct DeviceSlot {
    pub role: DeviceRole,
    app: Option<AppHandle>,
    state: watch::Sender<DeviceState>,
    stats: std::sync::Mutex<StatsInner>,
    log: DeviceLog,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl DeviceSlot {
    fn new(role: DeviceRole, app: Option<AppHandle>) -> Arc<Self> {
        Arc::new(Self {
            role,
            app,
            state: watch::Sender::new(DeviceState::Idle),
            stats: std::sync::Mutex::new(StatsInner::default()),
            log: DeviceLog::default(),
            worker: Mutex::new(None),
        })
    }

    /// Send an event to the UI. Returns false only when a window exists and the
    /// emit failed (the UI is gone); headless tests report success.
    pub fn emit<T: Serialize + Clone>(&self, event: &str, payload: T) -> bool {
        match &self.app {
            Some(app) => match app.emit(event, payload) {
                Ok(()) => true,
                Err(error) => {
                    tracing::warn!(event, error = %error, "Could not emit event to UI");
                    false
                }
            },
            None => true,
        }
    }

    pub async fn state(&self) -> DeviceState {
        self.state.borrow().clone()
    }

    pub async fn set_state(&self, next: DeviceState) {
        let previous = self.state.send_replace(next.clone());
        tracing::debug!(role = ?self.role, from = ?previous, to = ?next, "Device state changed");
        self.emit("devices://slot", self.snapshot().await);
    }

    /// Follow this slot's connection state; wakes on every change.
    pub fn subscribe_state(&self) -> watch::Receiver<DeviceState> {
        self.state.subscribe()
    }

    pub fn stats(&self) -> SlotStats {
        let mut inner = self
            .stats
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        refresh_rate(&mut inner, Utc::now().timestamp_millis());
        inner.stats.clone()
    }

    pub fn log_lines(&self) -> Vec<DeviceLogLine> {
        self.log.all()
    }

    pub async fn snapshot(&self) -> SlotSnapshot {
        SlotSnapshot {
            role: self.role,
            state: self.state().await,
            stats: self.stats(),
            log: self.log.tail(log::TAIL),
        }
    }

    /// A connection-phase line: kept in the log, sent to the hub, and (for the
    /// trainer) mirrored to the connect dialog's readout.
    pub fn progress(&self, level: &'static str, step: &str, detail: Option<String>) {
        self.record(level, step, detail, true);
    }

    /// A runtime line (after connection): kept in the log and sent to the hub.
    pub fn note(&self, level: &'static str, step: &str, detail: Option<String>) {
        self.record(level, step, detail, false);
    }

    fn record(&self, level: &'static str, step: &str, detail: Option<String>, connect: bool) {
        tracing::debug!(role = ?self.role, level, step, detail = ?detail, "Device log");
        let line = self.log.push(level, step, detail, connect);
        if connect && self.role == DeviceRole::Trainer {
            self.emit(
                "trainer://connect-progress",
                ConnectProgress {
                    step: line.step.clone(),
                    detail: line.detail.clone(),
                    level,
                },
            );
        }
        self.emit(
            "devices://log",
            DeviceLogEvent {
                role: self.role,
                line,
            },
        );
    }

    /// Called by the notification worker for every parsed sample.
    pub fn record_sample(&self, raw: Option<&[u8]>) {
        let now = Utc::now().timestamp_millis();
        let due = {
            let mut inner = self
                .stats
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            inner.stats.samples += 1;
            inner.stats.last_sample_ms = Some(now);
            if let Some(raw) = raw {
                inner.stats.last_raw_hex = Some(ble::hex(raw));
            }
            inner.window.push_back(now);
            refresh_rate(&mut inner, now);
            if now - inner.last_emit_ms >= STATS_EMIT_INTERVAL_MS {
                inner.last_emit_ms = now;
                true
            } else {
                false
            }
        };
        if due && let Some(app) = &self.app {
            // Stats are pushed at most once a second per role.
            let app = app.clone();
            let role = self.role;
            let stats = self.stats();
            let state = self.state.borrow().clone();
            {
                let _ = app.emit(
                    "devices://slot",
                    SlotSnapshot {
                        role,
                        state,
                        stats,
                        log: self.log.tail(log::TAIL),
                    },
                );
            }
        }
    }

    pub fn record_reading(&self, summary: String) {
        let mut inner = self
            .stats
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.stats.last_reading = Some(summary);
    }

    /// Update battery information delivered after connection (for example,
    /// the optional ANT+ HRM page 7) and immediately refresh the device card.
    /// Returns true when the displayed value changed.
    pub fn record_battery(
        &self,
        battery_percent: Option<u8>,
        battery_status: Option<String>,
        battery_voltage: Option<f32>,
    ) -> bool {
        let changed = {
            let mut inner = self
                .stats
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if inner.stats.battery_percent == battery_percent
                && inner.stats.battery_status == battery_status
                && inner.stats.battery_voltage == battery_voltage
            {
                false
            } else {
                inner.stats.battery_percent = battery_percent;
                inner.stats.battery_status = battery_status;
                inner.stats.battery_voltage = battery_voltage;
                true
            }
        };
        if changed {
            self.emit(
                "devices://slot",
                SlotSnapshot {
                    role: self.role,
                    state: self.state.borrow().clone(),
                    stats: self.stats(),
                    log: self.log.tail(log::TAIL),
                },
            );
        }
        changed
    }

    pub async fn set_calibration(&self, supported: Option<bool>, calibrating: Option<bool>) {
        {
            let mut inner = self
                .stats
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(supported) = supported {
                inner.stats.calibration_supported = supported;
            }
            if let Some(calibrating) = calibrating {
                inner.stats.calibrating = calibrating;
            }
        }
        self.emit("devices://slot", self.snapshot().await);
    }

    pub fn record_parse_failure(&self, raw: &[u8]) -> u64 {
        let mut inner = self
            .stats
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.stats.parse_failures += 1;
        inner.stats.last_raw_hex = Some(ble::hex(raw));
        inner.stats.parse_failures
    }

    pub fn record_connected(&self, rssi: Option<i16>, details: &ble::DeviceDetails) {
        let mut inner = self
            .stats
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.stats.connected_since_ms = Some(Utc::now().timestamp_millis());
        inner.stats.rssi = rssi;
        inner.stats.battery_percent = details.battery_percent;
        inner.stats.battery_status = None;
        inner.stats.battery_voltage = None;
        inner.stats.manufacturer = details.manufacturer.clone();
        inner.stats.model = details.model.clone();
        inner.stats.firmware = details.firmware.clone();
        inner.stats.samples = 0;
        inner.stats.parse_failures = 0;
        inner.stats.last_sample_ms = None;
        inner.stats.last_raw_hex = None;
        inner.window.clear();
    }

    pub async fn set_reconnect_attempt(&self, attempt: u32) {
        {
            let mut inner = self
                .stats
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            inner.stats.reconnect_attempt = attempt;
        }
        self.emit("devices://slot", self.snapshot().await);
    }

    pub fn record_drop(&self) {
        let mut inner = self
            .stats
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.stats.drops += 1;
        inner.stats.connected_since_ms = None;
        inner.window.clear();
        inner.stats.rate_hz = 0.0;
    }

    fn record_disconnected(&self) {
        let mut inner = self
            .stats
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.stats.connected_since_ms = None;
        inner.window.clear();
        inner.stats.rate_hz = 0.0;
    }

    pub async fn set_worker(&self, worker: JoinHandle<()>) {
        if let Some(previous) = self.worker.lock().await.replace(worker) {
            previous.abort();
        }
    }

    pub async fn abort_worker(&self) -> bool {
        match self.worker.lock().await.take() {
            Some(worker) => {
                worker.abort();
                true
            }
            None => false,
        }
    }
}

fn refresh_rate(inner: &mut StatsInner, now: i64) {
    while inner
        .window
        .front()
        .is_some_and(|at| now - *at > RATE_WINDOW_MS)
    {
        inner.window.pop_front();
    }
    inner.stats.rate_hz = match (inner.window.front(), inner.window.len()) {
        (_, 0) => 0.0,
        (Some(first), count) => {
            let span_ms = (now - *first).max(500) as f32;
            count as f32 * 1_000.0 / span_ms
        }
        (None, _) => 0.0,
    };
}

/// Run a device's notification loop as a task. Whatever ends it, including a
/// panic, is treated as a link loss so the slot ends up in `Reconnecting` and
/// the supervisor takes over. `body` returns the number of samples it saw.
pub(crate) fn spawn_link_worker(
    slot: Arc<DeviceSlot>,
    fuser: Arc<TelemetryFuser>,
    role: DeviceRole,
    name: String,
    body: impl Future<Output = u64> + Send + 'static,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let samples = match AssertUnwindSafe(body).catch_unwind().await {
            Ok(samples) => samples,
            Err(_) => {
                tracing::error!(?role, "Device worker panicked; treating it as a link loss");
                slot.note("error", "Device worker crashed", None);
                0
            }
        };
        link_lost(&slot, &fuser, role, name, samples).await;
    })
}

/// Some Bluetooth backends leave notification streams open after disconnect.
/// Check the transport independently; a quiet but connected sensor is healthy.
async fn monitor_connection<F, Fut>(mut connected: F) -> String
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<bool, String>>,
{
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        match tokio::time::timeout(Duration::from_secs(4), connected()).await {
            Ok(Ok(true)) => {}
            Ok(Ok(false)) => return "Bluetooth connection closed".into(),
            Ok(Err(error)) => return format!("Bluetooth connection check failed: {error}"),
            Err(_) => return "Bluetooth connection check timed out".into(),
        }
    }
}

pub(crate) fn spawn_ble_link_worker(
    slot: Arc<DeviceSlot>,
    fuser: Arc<TelemetryFuser>,
    role: DeviceRole,
    name: String,
    peripheral: Peripheral,
    body: impl Future<Output = u64> + Send + 'static,
) -> JoinHandle<()> {
    spawn_link_worker(slot.clone(), fuser, role, name, async move {
        tokio::select! {
            samples = body => samples,
            reason = monitor_connection(|| async {
                peripheral.is_connected().await.map_err(|error| error.to_string())
            }) => {
                slot.note("error", "Bluetooth connection lost", Some(reason));
                slot.stats().samples
            }
        }
    })
}

/// What every device worker does when its stream ends: record the drop, stop
/// feeding the fuser, and mark the slot as needing a reconnect.
pub(crate) async fn link_lost(
    slot: &DeviceSlot,
    fuser: &TelemetryFuser,
    role: DeviceRole,
    name: String,
    samples: u64,
) {
    tracing::warn!(?role, samples, "Notification stream ended; link lost");
    slot.record_drop();
    fuser.forget(role);
    slot.note(
        "error",
        "Link lost",
        Some(format!("after {samples} samples")),
    );
    slot.set_state(DeviceState::Reconnecting { name }).await;
}

/// Backoff between automatic reconnect attempts; the last entry repeats.
const RECONNECT_BACKOFF: [Duration; 6] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(20),
    Duration::from_secs(30),
];
/// Attempts made when no ride is active before giving up and leaving the
/// slot in an error state with a Reconnect button. During a ride the
/// supervisor never gives up.
const RECONNECT_IDLE_ATTEMPTS: u32 = 6;

/// What the reconnect supervisor knows about one role: the device to go back
/// to, and a generation counter that a manual connect or disconnect bumps to
/// cancel any reconnect episode in progress.
struct RoleLink {
    last_device: std::sync::Mutex<Option<DeviceInfo>>,
    generation: watch::Sender<u64>,
}

impl RoleLink {
    fn new() -> Self {
        Self {
            last_device: std::sync::Mutex::new(None),
            generation: watch::Sender::new(0),
        }
    }

    fn last_device(&self) -> Option<DeviceInfo> {
        self.last_device
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn set_last_device(&self, device: Option<DeviceInfo>) {
        *self
            .last_device
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = device;
    }

    fn bump(&self) {
        self.generation.send_modify(|generation| *generation += 1);
    }
}

/// Owns every slot, the Bluetooth adapter and the fused telemetry stream.
pub struct DeviceHub {
    ble: Arc<ble::Ble>,
    ant: ant::Ant,
    scanning: AtomicBool,
    scan_error: std::sync::Mutex<Option<ScanError>>,
    telemetry: broadcast::Sender<Telemetry>,
    fuser: Arc<fuser::TelemetryFuser>,
    trainer: trainer::Trainer,
    heart_rate: sensor::Sensor,
    ant_heart_rate: ant::receiver::HeartRateReceiver,
    power: sensor::Sensor,
    cadence: sensor::Sensor,
    /// Serializes connects: BlueZ misbehaves when several GATT connections
    /// start at once or while a scan is running.
    connect_lock: Mutex<()>,
    links: HashMap<DeviceRole, RoleLink>,
    /// Set by the runner while a ride is in progress; the reconnect
    /// supervisor never gives up while it is true.
    ride_active: AtomicBool,
}

impl Default for DeviceHub {
    fn default() -> Self {
        Self::build(None)
    }
}

impl DeviceHub {
    pub fn new(app: AppHandle) -> Self {
        Self::build(Some(app))
    }

    fn build(app: Option<AppHandle>) -> Self {
        let (telemetry, _) = broadcast::channel(256);
        // A hub without a window (tests) never touches the OS Bluetooth stack.
        let ble = Arc::new(if app.is_some() {
            ble::Ble::default()
        } else {
            ble::Ble::disabled()
        });
        let fuser = Arc::new(fuser::TelemetryFuser::new(app.clone(), telemetry.clone()));
        let ant = if app.is_some() {
            ant::Ant::new(true)
        } else {
            ant::Ant::disabled()
        };
        let trainer_slot = DeviceSlot::new(DeviceRole::Trainer, app.clone());
        let heart_rate_slot = DeviceSlot::new(DeviceRole::HeartRate, app.clone());
        let power_slot = DeviceSlot::new(DeviceRole::Power, app.clone());
        let cadence_slot = DeviceSlot::new(DeviceRole::Cadence, app.clone());
        Self {
            trainer: trainer::Trainer::new(trainer_slot, ble.clone(), fuser.clone()),
            heart_rate: sensor::Sensor::new(
                heart_rate_slot.clone(),
                ble.clone(),
                fuser.clone(),
                heart_rate::decoder,
            ),
            ant_heart_rate: ant::receiver::HeartRateReceiver::new(
                heart_rate_slot,
                ant.clone(),
                fuser.clone(),
            ),
            power: sensor::Sensor::new(
                power_slot,
                ble.clone(),
                fuser.clone(),
                cycling_power::decoder,
            ),
            cadence: sensor::Sensor::new(
                cadence_slot,
                ble.clone(),
                fuser.clone(),
                cadence::decoder,
            ),
            ble,
            ant,
            scanning: AtomicBool::new(false),
            scan_error: std::sync::Mutex::new(None),
            telemetry,
            fuser,
            connect_lock: Mutex::new(()),
            links: DeviceRole::ALL
                .into_iter()
                .map(|role| (role, RoleLink::new()))
                .collect(),
            ride_active: AtomicBool::new(false),
        }
    }

    fn link(&self, role: DeviceRole) -> &RoleLink {
        &self.links[&role]
    }

    pub fn set_ride_active(&self, active: bool) {
        self.ride_active.store(active, Ordering::Relaxed);
    }

    pub fn ride_active(&self) -> bool {
        self.ride_active.load(Ordering::Relaxed)
    }

    /// Stop every automatic reconnect (app shutdown).
    pub fn cancel_reconnects(&self) {
        for link in self.links.values() {
            link.set_last_device(None);
            link.bump();
        }
    }

    /// Start one reconnect supervisor per role. They live as long as the
    /// runtime and re-arm on every successful connect made in this session.
    pub fn spawn_reconnect_supervisors(hub: &Arc<Self>) {
        for role in DeviceRole::ALL {
            let hub = hub.clone();
            tokio::spawn(async move { reconnect_supervisor(hub, role).await });
        }
    }

    pub fn slot(&self, role: DeviceRole) -> &Arc<DeviceSlot> {
        match role {
            DeviceRole::Trainer => self.trainer.slot(),
            DeviceRole::HeartRate => self.heart_rate.slot(),
            DeviceRole::Power => self.power.slot(),
            DeviceRole::Cadence => self.cadence.slot(),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Telemetry> {
        self.telemetry.subscribe()
    }

    pub fn source_preferences(&self) -> SourcePreferences {
        self.fuser.preferences()
    }

    pub fn set_source_preferences(&self, preferences: SourcePreferences) {
        self.fuser.set_preferences(preferences);
    }

    /// Which device is currently feeding each metric.
    pub fn telemetry_sources(&self) -> TelemetrySources {
        self.fuser.sources()
    }

    pub fn is_scanning(&self) -> bool {
        self.scanning.load(Ordering::Relaxed)
    }

    fn scan_error(&self) -> Option<ScanError> {
        self.scan_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn set_scan_error(&self, error: Option<ScanError>) {
        *self
            .scan_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = error;
    }

    pub async fn snapshot(&self) -> DevicesSnapshot {
        let mut slots = Vec::with_capacity(DeviceRole::ALL.len());
        for role in DeviceRole::ALL {
            slots.push(self.slot(role).snapshot().await);
        }
        DevicesSnapshot {
            scanning: self.is_scanning(),
            scan_error: self.scan_error(),
            ant_adapter: self.ant.status(),
            slots,
            source_preferences: self.source_preferences(),
            sources: self.telemetry_sources(),
        }
    }

    /// The trainer's state, as the pre-hub UI expects it: scanning and scan
    /// failures show up here when no trainer is connected.
    pub async fn state(&self) -> DeviceState {
        let state = self.trainer.slot().state().await;
        match state {
            DeviceState::Idle if self.is_scanning() => DeviceState::Scanning,
            DeviceState::Idle => match self.scan_error() {
                Some(ScanError { message, guidance }) => DeviceState::Error { message, guidance },
                None => DeviceState::Idle,
            },
            other => other,
        }
    }

    /// Scan for every kind of sensor. With `include_simulated` (developer
    /// mode) the simulators are listed first; otherwise only real hardware
    /// is offered.
    pub async fn scan(&self, include_simulated: bool) -> Result<Vec<DeviceInfo>, String> {
        tracing::info!("Scanning for Bluetooth and ANT+ sensors");
        self.scanning.store(true, Ordering::Relaxed);
        self.set_scan_error(None);
        let started = std::time::Instant::now();
        let (ble_result, ant_result) = tokio::join!(self.ble.scan(), self.ant.scan_heart_rate());
        self.scanning.store(false, Ordering::Relaxed);
        let ant_found = match ant_result {
            Ok(found) => found,
            Err(error) => {
                tracing::warn!(error = %error, "ANT scan unavailable");
                Vec::new()
            }
        };
        match ble_result {
            Err(message) if ant_found.is_empty() => {
                tracing::error!(error = %message, "Scan failed");
                self.set_scan_error(Some(ScanError {
                    message: message.clone(),
                    guidance: ble::platform_guidance(),
                }));
                Err(message)
            }
            ble_result => {
                let found = match ble_result {
                    Ok(found) => found,
                    Err(message) => {
                        tracing::warn!(error = %message, "Bluetooth scan unavailable; keeping ANT results");
                        self.set_scan_error(Some(ScanError {
                            message,
                            guidance: ble::platform_guidance(),
                        }));
                        Vec::new()
                    }
                };
                tracing::info!(
                    ble_found = found.len(),
                    ant_found = ant_found.len(),
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "Scan finished"
                );
                let mut devices = if include_simulated {
                    let mut simulators = trainer::simulated_devices();
                    simulators.push(heart_rate::simulated_device());
                    simulators.push(cycling_power::simulated_device());
                    simulators.push(cadence::simulated_device());
                    simulators
                } else {
                    Vec::new()
                };
                devices.extend(found);
                devices.extend(ant_found);
                Ok(devices)
            }
        }
    }

    /// Scan and keep only devices usable as a trainer (pre-hub API).
    pub async fn scan_trainers(&self, include_simulated: bool) -> Result<Vec<DeviceInfo>, String> {
        Ok(self
            .scan(include_simulated)
            .await?
            .into_iter()
            .filter(|device| device.supports(Capability::Ftms))
            .collect())
    }

    pub async fn connect(&self, role: DeviceRole, device: DeviceInfo) -> Result<(), String> {
        // A manual connect supersedes any automatic reconnect in progress.
        self.link(role).bump();
        self.connect_inner(role, device).await
    }

    async fn connect_inner(&self, role: DeviceRole, device: DeviceInfo) -> Result<(), String> {
        let _serialized = self.connect_lock.lock().await;
        if self
            .slot(role)
            .state()
            .await
            .device()
            .is_some_and(|connected| connected.id == device.id)
        {
            tracing::info!(?role, name = %device.name, "Already connected; nothing to do");
            return Ok(());
        }
        if !device.simulated
            && !device.capabilities.is_empty()
            && !device
                .capabilities
                .iter()
                .any(|capability| role.accepts(*capability))
        {
            return Err(format!(
                "{} does not advertise a service usable as {}",
                device.name,
                role.label().to_lowercase()
            ));
        }
        for other in DeviceRole::ALL.into_iter().filter(|other| *other != role) {
            if self
                .slot(other)
                .state()
                .await
                .device()
                .is_some_and(|connected| connected.id == device.id && !device.simulated)
            {
                return Err(format!(
                    "{} is already connected as {}",
                    device.name,
                    other.label().to_lowercase()
                ));
            }
        }
        // A remembered device was not part of the last scan: look for it now.
        let device = if !device.simulated
            && device.transport == DeviceTransport::Ble
            && self.ble.peripheral(&device.id).await.is_none()
        {
            let slot = self.slot(role);
            slot.set_state(DeviceState::Connecting {
                name: device.name.clone(),
            })
            .await;
            slot.progress(
                "info",
                "Looking for remembered device",
                Some(format!(
                    "{} · up to {}s",
                    device.name,
                    ble::FIND_TIMEOUT.as_secs()
                )),
            );
            match self.ble.find(&device.id).await {
                Ok(seen) => {
                    slot.progress(
                        "ok",
                        "Device found",
                        seen.rssi.map(|rssi| format!("signal {rssi} dBm")),
                    );
                    DeviceInfo {
                        // Keep the remembered name if the advertisement had none.
                        name: if seen.name.is_empty() {
                            device.name
                        } else {
                            seen.name
                        },
                        ..seen
                    }
                }
                Err(error) => {
                    slot.progress("error", "Device not found", Some(error.clone()));
                    slot.set_state(DeviceState::Error {
                        message: error.clone(),
                        guidance: "Wake the device (spin the crank, wear the strap, pedal the trainer) and try again.".into(),
                    })
                    .await;
                    return Err(error);
                }
            }
        } else {
            device
        };
        let result = match role {
            DeviceRole::Trainer => self.trainer.connect(device.clone()).await,
            DeviceRole::HeartRate if device.transport == DeviceTransport::Ant => {
                self.heart_rate.disconnect().await;
                self.ant_heart_rate.connect(device.clone()).await
            }
            DeviceRole::HeartRate => {
                self.ant_heart_rate.disconnect().await;
                self.heart_rate.connect(device.clone()).await
            }
            DeviceRole::Power => self.power.connect(device.clone()).await,
            DeviceRole::Cadence => self.cadence.connect(device.clone()).await,
        };
        if result.is_ok() {
            // Remember what to go back to if the link drops.
            self.link(role).set_last_device(Some(device));
        }
        result
    }

    /// Connect the most recently used remembered device of every role that
    /// has nothing connected or connecting, in `DeviceRole::ALL` order. A
    /// device that does not answer never stops the others; each outcome is
    /// reported and the caller decides how loudly to surface it.
    pub async fn connect_known(&self, known: &[KnownDevice]) -> Vec<KnownConnectOutcome> {
        let mut outcomes = Vec::new();
        for role in DeviceRole::ALL {
            let Some(device) = known
                .iter()
                .filter(|device| device.role == role)
                .max_by_key(|device| device.last_connected_at)
            else {
                continue;
            };
            let state = self.slot(role).state().await;
            if state.is_connected() || matches!(state, DeviceState::Connecting { .. }) {
                tracing::info!(?role, name = %device.name, "Connect all: role busy; skipped");
                outcomes.push(KnownConnectOutcome {
                    role,
                    name: device.name.clone(),
                    status: KnownConnectStatus::Skipped,
                    error: None,
                });
                continue;
            }
            let (status, error) = match self.connect(role, device.to_device_info()).await {
                Ok(()) => (KnownConnectStatus::Connected, None),
                Err(error) => {
                    tracing::warn!(?role, name = %device.name, error = %error, "Connect all: device did not answer");
                    (KnownConnectStatus::Failed, Some(error))
                }
            };
            outcomes.push(KnownConnectOutcome {
                role,
                name: device.name.clone(),
                status,
                error,
            });
        }
        outcomes
    }

    /// Build the record to remember after a successful connect: the device as
    /// connected plus whatever Device Information it exposed.
    pub async fn remember(&self, role: DeviceRole) -> Option<KnownDevice> {
        let slot = self.slot(role);
        let device = slot.state().await.device()?.clone();
        let stats = slot.stats();
        Some(KnownDevice {
            id: device.id,
            name: device.name,
            transport: device.transport,
            role,
            capabilities: device.capabilities,
            simulated: device.simulated,
            manufacturer: stats.manufacturer,
            model: stats.model,
            last_connected_at: Utc::now(),
        })
    }

    pub async fn disconnect_role(&self, role: DeviceRole) {
        // Cancel any reconnect episode, forget the device, and let an
        // in-flight attempt finish before tearing the link down (aborting a
        // connect midway can leave a GATT link nobody owns).
        self.link(role).bump();
        self.link(role).set_last_device(None);
        let _serialized = self.connect_lock.lock().await;
        match role {
            DeviceRole::Trainer => self.trainer.disconnect().await,
            DeviceRole::HeartRate => {
                self.ant_heart_rate.disconnect().await;
                self.heart_rate.disconnect().await;
            }
            DeviceRole::Power => self.power.disconnect().await,
            DeviceRole::Cadence => self.cadence.disconnect().await,
        }
    }

    /// Disconnect everything (app quit).
    pub async fn disconnect(&self) {
        for role in DeviceRole::ALL {
            self.disconnect_role(role).await;
        }
    }

    // Trainer control, used by the workout runner.

    pub async fn calibrate_trainer(&self) -> Result<(), String> {
        self.trainer.calibrate().await
    }

    /// The trainer's clamp for a requested target, without sending anything.
    pub fn clamp_target(&self, requested: u16, rider_max: u16) -> u16 {
        self.trainer.clamp_target(requested, rider_max)
    }

    /// Fault-injection knobs of the simulated trainer.
    pub fn simulated_faults(&self) -> Arc<trainer::SimFaults> {
        self.trainer.faults()
    }

    pub async fn begin_control(&self) -> Result<(), ControlError> {
        self.trainer.begin_control().await
    }

    /// Persistent command failures must re-open the transport, not just retry
    /// commands on a dead handle. Do not supersede a manual connect/disconnect.
    pub async fn reconnect_trainer(&self) {
        let Ok(_serialized) = self.connect_lock.try_lock() else {
            return;
        };
        if self.link(DeviceRole::Trainer).last_device().is_none() {
            return;
        }
        let slot = self.slot(DeviceRole::Trainer);
        let Some(device) = slot.state().await.device().cloned() else {
            return;
        };
        slot.abort_worker().await;
        link_lost(
            slot,
            &self.fuser,
            DeviceRole::Trainer,
            device.name,
            slot.stats().samples,
        )
        .await;
    }

    /// Request Control + Start/Resume again, for a trainer that answered
    /// ControlNotPermitted mid-ride.
    pub async fn reacquire_control(&self) -> Result<(), ControlError> {
        self.trainer.reacquire_control().await
    }

    pub async fn request_control(&self) -> Result<(), ControlError> {
        self.trainer.request_control().await
    }

    pub async fn set_target_power(
        &self,
        requested: u16,
        rider_max: u16,
    ) -> Result<u16, ControlError> {
        self.trainer.set_target_power(requested, rider_max).await
    }

    pub async fn pause(&self) -> Result<(), ControlError> {
        self.trainer.pause().await
    }

    pub async fn stop(&self) -> Result<(), ControlError> {
        self.trainer.stop().await
    }

    /// Follow the trainer slot's connection state.
    pub fn subscribe_trainer_state(&self) -> watch::Receiver<DeviceState> {
        self.trainer.slot().subscribe_state()
    }
}

/// Watches one role for link loss and brings the device back: fresh targeted
/// scan, growing backoff, forever while a ride is active and for a handful of
/// attempts otherwise. A manual connect or disconnect cancels it.
async fn reconnect_supervisor(hub: Arc<DeviceHub>, role: DeviceRole) {
    let slot = hub.slot(role).clone();
    let mut state_rx = slot.subscribe_state();
    loop {
        if state_rx
            .wait_for(|state| matches!(state, DeviceState::Reconnecting { .. }))
            .await
            .is_err()
        {
            return;
        }
        let Some(device) = hub.link(role).last_device() else {
            // A manual disconnect can cancel recovery while the slot still
            // briefly says Reconnecting. Wait instead of spinning on it.
            if state_rx.changed().await.is_err() {
                return;
            }
            continue;
        };
        let generation = *hub.link(role).generation.borrow();
        let mut cancel_rx = hub.link(role).generation.subscribe();
        tracing::info!(?role, name = %device.name, "Link lost; automatic reconnect armed");
        hub.ble.forget_peripheral(&device.id).await;
        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            if !hub.ride_active() && attempt > RECONNECT_IDLE_ATTEMPTS {
                tracing::warn!(
                    ?role,
                    attempts = attempt - 1,
                    "Giving up on automatic reconnect"
                );
                slot.note("warn", "Automatic reconnect gave up", None);
                slot.set_state(DeviceState::Error {
                    message: format!("{} link lost", device.name),
                    guidance: "Wake the device and press Reconnect.".into(),
                })
                .await;
                break;
            }
            let delay = RECONNECT_BACKOFF[(attempt as usize - 1).min(RECONNECT_BACKOFF.len() - 1)];
            slot.set_reconnect_attempt(attempt).await;
            slot.note(
                "info",
                "Reconnect scheduled",
                Some(format!("attempt #{attempt} in {}s", delay.as_secs())),
            );
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = cancel_rx.wait_for(|current| *current != generation) => {}
            }
            if *hub.link(role).generation.borrow() != generation {
                tracing::info!(?role, "Automatic reconnect cancelled");
                break;
            }
            slot.note("info", "Reconnect attempt", Some(format!("#{attempt}")));
            match hub.connect_inner(role, device.clone()).await {
                Ok(()) => {
                    tracing::info!(?role, name = %device.name, attempt, "Automatic reconnect succeeded");
                    break;
                }
                Err(error) => {
                    tracing::warn!(?role, attempt, error = %error, "Automatic reconnect attempt failed");
                    if *hub.link(role).generation.borrow() != generation {
                        break;
                    }
                    // Keep the card on "Link lost" between attempts.
                    slot.set_state(DeviceState::Reconnecting {
                        name: device.name.clone(),
                    })
                    .await;
                }
            }
        }
        slot.set_reconnect_attempt(0).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn wait_until(check: impl AsyncFn() -> bool) {
        for _ in 0..6_000 {
            if check().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("condition never became true");
    }

    fn reconnect_attempts(hub: &DeviceHub, role: DeviceRole) -> usize {
        hub.slot(role)
            .log_lines()
            .iter()
            .filter(|line| line.step == "Reconnect attempt")
            .count()
    }

    #[tokio::test]
    async fn simulators_are_offered_only_in_developer_mode() {
        let hub = DeviceHub::default();
        let offered = hub.scan(true).await.unwrap();
        assert!(
            offered.iter().any(|device| device.simulated),
            "developer mode should offer the simulators"
        );
        assert!(
            hub.scan(false).await.unwrap().is_empty(),
            "without developer mode only real hardware is offered"
        );
        assert!(
            hub.scan_trainers(false).await.unwrap().is_empty(),
            "the trainer scan hides simulators too"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_dropped_simulator_reconnects_on_its_own() {
        let hub = Arc::new(DeviceHub::default());
        DeviceHub::spawn_reconnect_supervisors(&hub);
        hub.connect(DeviceRole::Trainer, trainer::simulated_devices().remove(0))
            .await
            .unwrap();
        hub.simulated_faults().drop_link.notify_one();
        wait_until(async || {
            matches!(
                hub.slot(DeviceRole::Trainer).state().await,
                DeviceState::Reconnecting { .. }
            )
        })
        .await;
        wait_until(async || hub.slot(DeviceRole::Trainer).state().await.is_connected()).await;
        assert_eq!(reconnect_attempts(&hub, DeviceRole::Trainer), 1);
        assert_eq!(hub.slot(DeviceRole::Trainer).stats().reconnect_attempt, 0);
        hub.disconnect().await;
    }

    #[tokio::test(start_paused = true)]
    async fn manual_disconnect_cancels_an_automatic_reconnect() {
        let hub = Arc::new(DeviceHub::default());
        DeviceHub::spawn_reconnect_supervisors(&hub);
        hub.connect(DeviceRole::Trainer, trainer::simulated_devices().remove(0))
            .await
            .unwrap();
        let faults = hub.simulated_faults();
        faults.fail_connects.store(u32::MAX, Ordering::Relaxed);
        faults.drop_link.notify_one();
        wait_until(async || reconnect_attempts(&hub, DeviceRole::Trainer) >= 2).await;
        hub.disconnect_role(DeviceRole::Trainer).await;
        assert!(matches!(
            hub.slot(DeviceRole::Trainer).state().await,
            DeviceState::Idle
        ));
        let attempts = reconnect_attempts(&hub, DeviceRole::Trainer);
        tokio::time::sleep(Duration::from_secs(600)).await;
        assert_eq!(reconnect_attempts(&hub, DeviceRole::Trainer), attempts);
        assert!(matches!(
            hub.slot(DeviceRole::Trainer).state().await,
            DeviceState::Idle
        ));
        faults.fail_connects.store(0, Ordering::Relaxed);
    }

    #[tokio::test(start_paused = true)]
    async fn reconnect_gives_up_when_idle_but_not_during_a_ride() {
        let hub = Arc::new(DeviceHub::default());
        DeviceHub::spawn_reconnect_supervisors(&hub);
        hub.connect(DeviceRole::Trainer, trainer::simulated_devices().remove(0))
            .await
            .unwrap();
        let faults = hub.simulated_faults();
        faults.fail_connects.store(u32::MAX, Ordering::Relaxed);
        // Idle: a handful of attempts, then an error with a Reconnect button.
        faults.drop_link.notify_one();
        wait_until(async || {
            matches!(
                hub.slot(DeviceRole::Trainer).state().await,
                DeviceState::Error { .. }
            )
        })
        .await;
        assert_eq!(
            reconnect_attempts(&hub, DeviceRole::Trainer) as u32,
            RECONNECT_IDLE_ATTEMPTS
        );
        // During a ride: keeps trying well past that.
        faults.fail_connects.store(0, Ordering::Relaxed);
        hub.connect(DeviceRole::Trainer, trainer::simulated_devices().remove(0))
            .await
            .unwrap();
        hub.set_ride_active(true);
        faults.fail_connects.store(u32::MAX, Ordering::Relaxed);
        faults.drop_link.notify_one();
        wait_until(async || {
            reconnect_attempts(&hub, DeviceRole::Trainer) as u32 >= RECONNECT_IDLE_ATTEMPTS + 3
        })
        .await;
        assert!(matches!(
            hub.slot(DeviceRole::Trainer).state().await,
            DeviceState::Reconnecting { .. }
        ));
        // The trainer comes back: the next attempt succeeds.
        faults.fail_connects.store(0, Ordering::Relaxed);
        wait_until(async || hub.slot(DeviceRole::Trainer).state().await.is_connected()).await;
        hub.set_ride_active(false);
        hub.disconnect().await;
    }

    #[tokio::test]
    async fn starts_idle_everywhere() {
        let hub = DeviceHub::default();
        assert!(matches!(hub.state().await, DeviceState::Idle));
        let snapshot = hub.snapshot().await;
        assert_eq!(snapshot.slots.len(), 4);
        assert!(!snapshot.scanning);
        assert!(
            snapshot
                .slots
                .iter()
                .all(|slot| matches!(slot.state, DeviceState::Idle))
        );
        assert_eq!(snapshot.slots[0].role, DeviceRole::Trainer);
    }

    #[tokio::test]
    async fn slot_log_and_stats() {
        let slot = DeviceSlot::new(DeviceRole::HeartRate, None);
        slot.progress("info", "Opening", Some("x".into()));
        slot.note("ok", "First sample", None);
        let lines = slot.log_lines();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].connect);
        assert!(!lines[1].connect);
        slot.record_sample(Some(&[0x10, 0x5a]));
        slot.record_sample(None);
        let stats = slot.stats();
        assert_eq!(stats.samples, 2);
        assert_eq!(stats.last_raw_hex.as_deref(), Some("10 5a"));
        assert!(stats.rate_hz > 0.0);
        assert!(slot.record_battery(Some(82), Some("Good".into()), Some(2.5)));
        assert!(!slot.record_battery(Some(82), Some("Good".into()), Some(2.5)));
        assert_eq!(slot.stats().battery_percent, Some(82));
        assert_eq!(slot.stats().battery_status.as_deref(), Some("Good"));
        assert_eq!(slot.stats().battery_voltage, Some(2.5));
        slot.record_drop();
        assert_eq!(slot.stats().drops, 1);
        assert_eq!(slot.stats().rate_hz, 0.0);
    }

    #[test]
    fn rate_uses_recent_window_only() {
        let mut inner = StatsInner::default();
        for at in [0, 1_000, 2_000, 3_000, 4_000] {
            inner.window.push_back(at);
        }
        refresh_rate(&mut inner, 4_000);
        assert!((inner.stats.rate_hz - 1.25).abs() < 0.01);
        refresh_rate(&mut inner, 20_000);
        assert_eq!(inner.stats.rate_hz, 0.0);
        assert!(inner.window.is_empty());
    }

    #[test]
    fn roles_accept_matching_capabilities() {
        assert!(DeviceRole::Trainer.accepts(Capability::Ftms));
        assert!(!DeviceRole::Trainer.accepts(Capability::CyclingPower));
        assert!(DeviceRole::Cadence.accepts(Capability::CyclingPower));
        assert!(DeviceRole::Cadence.accepts(Capability::Csc));
        assert!(!DeviceRole::HeartRate.accepts(Capability::Csc));
    }

    #[tokio::test]
    async fn mismatched_capabilities_are_rejected() {
        let hub = DeviceHub::default();
        let strap = DeviceInfo {
            id: "hr".into(),
            name: "Strap".into(),
            transport: Default::default(),
            simulated: false,
            rssi: None,
            capabilities: vec![Capability::HeartRate],
        };
        let wrong = hub
            .connect(DeviceRole::Trainer, strap.clone())
            .await
            .unwrap_err();
        assert!(wrong.contains("does not advertise"));
        let wrong = hub.connect(DeviceRole::Cadence, strap).await.unwrap_err();
        assert!(wrong.contains("does not advertise"));
        // A power meter qualifies for both Power and Cadence.
        let meter = DeviceInfo {
            id: "pm".into(),
            name: "Crank".into(),
            transport: Default::default(),
            simulated: false,
            rssi: None,
            capabilities: vec![Capability::CyclingPower],
        };
        // Not in the last scan, so the hub tries a targeted scan for it. That
        // fails here (no adapter in CI, or the device is not seen), but only
        // after the capability check passed, and the attempt is logged.
        let missing = hub.connect(DeviceRole::Cadence, meter).await.unwrap_err();
        assert!(!missing.contains("does not advertise"), "{missing}");
        let log = hub.slot(DeviceRole::Cadence).log_lines();
        assert!(
            log.iter()
                .any(|line| line.step == "Looking for remembered device")
        );
        assert!(log.iter().any(|line| line.step == "Device not found"));
        assert!(matches!(
            hub.slot(DeviceRole::Cadence).state().await,
            DeviceState::Error { .. }
        ));
    }

    #[tokio::test]
    async fn remember_captures_connected_device_and_details() {
        let hub = DeviceHub::default();
        assert!(hub.remember(DeviceRole::Trainer).await.is_none());
        hub.connect(DeviceRole::Trainer, trainer::simulated_devices().remove(0))
            .await
            .unwrap();
        let known = hub.remember(DeviceRole::Trainer).await.unwrap();
        assert_eq!(known.id, trainer::SIMULATED_TRAINER_ID);
        assert_eq!(known.role, DeviceRole::Trainer);
        assert!(known.simulated);
        assert_eq!(known.manufacturer.as_deref(), Some("BlakeBike"));
        assert_eq!(known.model.as_deref(), Some("Simulator"));
        assert_eq!(known.capabilities, vec![Capability::Ftms]);
        hub.disconnect().await;
    }

    #[tokio::test]
    async fn connect_known_takes_the_latest_per_role_and_carries_on_past_failures() {
        let hub = DeviceHub::default();
        let remembered = |device: DeviceInfo, role: DeviceRole, age_secs: i64| KnownDevice {
            id: device.id,
            name: device.name,
            transport: device.transport,
            role,
            capabilities: device.capabilities,
            simulated: device.simulated,
            manufacturer: None,
            model: None,
            last_connected_at: Utc::now() - chrono::Duration::seconds(age_secs),
        };
        let real = |id: &str, name: &str, capability: Capability| DeviceInfo {
            id: id.into(),
            name: name.into(),
            transport: DeviceTransport::Ble,
            simulated: false,
            rssi: None,
            capabilities: vec![capability],
        };
        // The strap is already connected: connect-all must leave it alone.
        hub.connect(DeviceRole::HeartRate, heart_rate::simulated_device())
            .await
            .unwrap();
        let known = vec![
            // An older real trainer listed first; the newer simulator must win.
            remembered(
                real("old-kickr", "Old KICKR", Capability::Ftms),
                DeviceRole::Trainer,
                3_600,
            ),
            remembered(
                trainer::simulated_devices().remove(0),
                DeviceRole::Trainer,
                60,
            ),
            remembered(heart_rate::simulated_device(), DeviceRole::HeartRate, 60),
            // Not reachable here (no adapter): fails, but the cadence sensor after it still connects.
            remembered(
                real("pm", "Assioma", Capability::CyclingPower),
                DeviceRole::Power,
                60,
            ),
            remembered(cadence::simulated_device(), DeviceRole::Cadence, 60),
        ];
        let outcomes = hub.connect_known(&known).await;
        let statuses: Vec<_> = outcomes
            .iter()
            .map(|outcome| (outcome.role, outcome.status))
            .collect();
        assert_eq!(
            statuses,
            vec![
                (DeviceRole::Trainer, KnownConnectStatus::Connected),
                (DeviceRole::HeartRate, KnownConnectStatus::Skipped),
                (DeviceRole::Power, KnownConnectStatus::Failed),
                (DeviceRole::Cadence, KnownConnectStatus::Connected),
            ]
        );
        assert_eq!(outcomes[0].name, "BlakeBike Simulator");
        assert!(outcomes[2].error.is_some());
        assert_eq!(
            hub.slot(DeviceRole::Trainer)
                .state()
                .await
                .device()
                .unwrap()
                .id,
            trainer::SIMULATED_TRAINER_ID
        );
        assert!(hub.slot(DeviceRole::Cadence).state().await.is_connected());
        // Nothing remembered for a role is simply not mentioned; nothing
        // connected means nothing to do.
        hub.disconnect().await;
        assert!(hub.connect_known(&[]).await.is_empty());
    }

    #[tokio::test]
    async fn all_four_simulators_fuse_with_dedicated_sensors_winning() {
        let hub = DeviceHub::default();
        let mut telemetry = hub.subscribe();
        hub.connect(DeviceRole::Trainer, trainer::simulated_devices().remove(0))
            .await
            .unwrap();
        hub.connect(DeviceRole::HeartRate, heart_rate::simulated_device())
            .await
            .unwrap();
        hub.connect(DeviceRole::Power, cycling_power::simulated_device())
            .await
            .unwrap();
        hub.connect(DeviceRole::Cadence, cadence::simulated_device())
            .await
            .unwrap();
        let mut settled = false;
        for _ in 0..40 {
            tokio::time::timeout(std::time::Duration::from_secs(3), telemetry.recv())
                .await
                .expect("telemetry flows")
                .unwrap();
            let sources = hub.telemetry_sources();
            if sources.power.is_some_and(|s| s.role == DeviceRole::Power)
                && sources
                    .cadence
                    .is_some_and(|s| s.role == DeviceRole::Cadence)
                && sources
                    .heart_rate
                    .is_some_and(|s| s.role == DeviceRole::HeartRate)
            {
                settled = true;
                break;
            }
        }
        assert!(
            settled,
            "every metric should come from its dedicated sensor"
        );
        let snapshot = hub.snapshot().await;
        assert!(snapshot.slots.iter().all(|slot| slot.state.is_connected()));
        assert!(snapshot.slots[2].stats.last_reading.is_some());
        hub.disconnect().await;
        assert!(
            hub.snapshot()
                .await
                .slots
                .iter()
                .all(|slot| matches!(slot.state, DeviceState::Idle))
        );
    }

    #[tokio::test]
    async fn simulated_strap_and_trainer_fuse_heart_rate() {
        let hub = DeviceHub::default();
        let mut telemetry = hub.subscribe();
        hub.connect(DeviceRole::Trainer, trainer::simulated_devices().remove(0))
            .await
            .unwrap();
        hub.connect(DeviceRole::HeartRate, heart_rate::simulated_device())
            .await
            .unwrap();
        // Wait until both sources have reported at least once.
        let mut fused = None;
        for _ in 0..12 {
            let sample = tokio::time::timeout(std::time::Duration::from_secs(3), telemetry.recv())
                .await
                .expect("telemetry flows")
                .unwrap();
            let sources = hub.telemetry_sources();
            if sources
                .heart_rate
                .is_some_and(|s| s.role == DeviceRole::HeartRate)
                && sources.power.is_some_and(|s| s.role == DeviceRole::Trainer)
            {
                fused = Some(sample);
                break;
            }
        }
        let sample = fused.expect("strap supplies heart rate, trainer supplies power");
        // Simulated strap starts near 95 bpm; the simulated trainer's own HR field is ~130.
        assert!(sample.heart_rate_bpm.unwrap() < 120);
        let snapshot = hub.snapshot().await;
        assert!(snapshot.slots[1].state.is_connected());
        assert_eq!(snapshot.source_preferences, SourcePreferences::default());
        hub.disconnect_role(DeviceRole::HeartRate).await;
        assert!(matches!(
            hub.slot(DeviceRole::HeartRate).state().await,
            DeviceState::Idle
        ));
        assert!(
            hub.slot(DeviceRole::HeartRate)
                .log_lines()
                .iter()
                .any(|line| line.step == "Disconnected")
        );
        hub.disconnect().await;
    }

    #[test]
    fn state_serializes_with_status_tag() {
        let json = serde_json::to_string(&DeviceState::Connecting { name: "K".into() }).unwrap();
        assert_eq!(json, r#"{"status":"connecting","name":"K"}"#);
    }

    #[test]
    fn old_device_json_defaults_to_ble_transport() {
        let device: DeviceInfo = serde_json::from_str(
            r#"{"id":"legacy","name":"Old strap","simulated":false,"rssi":null,"capabilities":["heartRate"]}"#,
        )
        .unwrap();
        assert_eq!(device.transport, DeviceTransport::Ble);
    }

    #[tokio::test(start_paused = true)]
    async fn a_silent_disconnected_transport_enters_normal_recovery() {
        let hub = Arc::new(DeviceHub::default());
        DeviceHub::spawn_reconnect_supervisors(&hub);
        hub.connect(DeviceRole::Trainer, trainer::simulated_devices().remove(0))
            .await
            .unwrap();
        let slot = hub.slot(DeviceRole::Trainer).clone();
        slot.abort_worker().await;
        let worker = spawn_link_worker(
            slot.clone(),
            hub.fuser.clone(),
            DeviceRole::Trainer,
            "Silent trainer".into(),
            async {
                // The notification stream never produces EOF. The transport check
                // must be sufficient to hand it to the reconnect supervisor.
                tokio::select! {
                    n = std::future::pending::<u64>() => n,
                    _ = monitor_connection(|| async { Ok(false) }) => 0,
                }
            },
        );
        slot.set_worker(worker).await;
        wait_until(async || slot.stats().drops > 0).await;
        wait_until(async || slot.state().await.is_connected()).await;
        assert_eq!(reconnect_attempts(&hub, DeviceRole::Trainer), 1);
        hub.disconnect().await;
    }

    #[tokio::test(start_paused = true)]
    async fn quiet_connected_sensors_are_not_reconnected() {
        assert!(
            tokio::time::timeout(
                Duration::from_secs(30),
                monitor_connection(|| async { Ok(true) })
            )
            .await
            .is_err()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_stalled_connection_check_is_bounded() {
        let reason = tokio::time::timeout(
            Duration::from_secs(10),
            monitor_connection(std::future::pending::<Result<bool, String>>),
        )
        .await
        .unwrap();
        assert!(reason.contains("timed out"));
    }

    #[tokio::test(start_paused = true)]
    async fn connection_check_errors_are_treated_as_link_loss() {
        let reason = monitor_connection(|| async { Err("adapter unavailable".into()) }).await;
        assert!(reason.contains("adapter unavailable"));
    }
}
