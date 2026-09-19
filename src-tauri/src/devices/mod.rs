//! The devices hub: one slot per device role (trainer, heart rate, power,
//! cadence), each with its own connection state, live statistics and log.
//! Everything downstream (workout runner, ride recorder, ride screen) consumes
//! a single fused `Telemetry` stream published by the hub.

pub mod ble;
pub mod log;
pub mod trainer;

use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::{
    sync::{Mutex, RwLock, broadcast},
    task::JoinHandle,
};

pub use ble::{Capability, DeviceInfo};
pub use log::DeviceLogLine;

use crate::domain::Telemetry;
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
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub firmware: Option<String>,
    pub connected_since_ms: Option<i64>,
    /// Link losses since the app started.
    pub drops: u32,
    pub last_raw_hex: Option<String>,
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
    pub slots: Vec<SlotSnapshot>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanError {
    pub message: String,
    pub guidance: String,
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
    state: RwLock<DeviceState>,
    stats: std::sync::Mutex<StatsInner>,
    log: DeviceLog,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl DeviceSlot {
    fn new(role: DeviceRole, app: Option<AppHandle>) -> Arc<Self> {
        Arc::new(Self {
            role,
            app,
            state: RwLock::new(DeviceState::Idle),
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
        self.state.read().await.clone()
    }

    pub async fn set_state(&self, next: DeviceState) {
        {
            let mut guard = self.state.write().await;
            tracing::debug!(role = ?self.role, from = ?*guard, to = ?next, "Device state changed");
            *guard = next;
        }
        self.emit("devices://slot", self.snapshot().await);
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
            let state = self.state.try_read().map(|guard| guard.clone());
            if let Ok(state) = state {
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
        inner.stats.manufacturer = details.manufacturer.clone();
        inner.stats.model = details.model.clone();
        inner.stats.firmware = details.firmware.clone();
        inner.stats.samples = 0;
        inner.stats.parse_failures = 0;
        inner.stats.last_sample_ms = None;
        inner.stats.last_raw_hex = None;
        inner.window.clear();
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

/// Owns every slot, the Bluetooth adapter and the fused telemetry stream.
pub struct DeviceHub {
    ble: Arc<ble::Ble>,
    scanning: AtomicBool,
    scan_error: std::sync::Mutex<Option<ScanError>>,
    telemetry: broadcast::Sender<Telemetry>,
    trainer: trainer::Trainer,
    heart_rate: Arc<DeviceSlot>,
    power: Arc<DeviceSlot>,
    cadence: Arc<DeviceSlot>,
    /// Serializes connects: BlueZ misbehaves when several GATT connections
    /// start at once or while a scan is running.
    connect_lock: Mutex<()>,
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
        let (telemetry, _) = broadcast::channel(64);
        let ble = Arc::new(ble::Ble::default());
        Self {
            trainer: trainer::Trainer::new(
                DeviceSlot::new(DeviceRole::Trainer, app.clone()),
                ble.clone(),
                telemetry.clone(),
            ),
            heart_rate: DeviceSlot::new(DeviceRole::HeartRate, app.clone()),
            power: DeviceSlot::new(DeviceRole::Power, app.clone()),
            cadence: DeviceSlot::new(DeviceRole::Cadence, app.clone()),
            ble,
            scanning: AtomicBool::new(false),
            scan_error: std::sync::Mutex::new(None),
            telemetry,
            connect_lock: Mutex::new(()),
        }
    }

    pub fn slot(&self, role: DeviceRole) -> &Arc<DeviceSlot> {
        match role {
            DeviceRole::Trainer => self.trainer.slot(),
            DeviceRole::HeartRate => &self.heart_rate,
            DeviceRole::Power => &self.power,
            DeviceRole::Cadence => &self.cadence,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Telemetry> {
        self.telemetry.subscribe()
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
            slots,
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

    /// Scan for every kind of sensor. Simulators are listed first.
    pub async fn scan(&self) -> Result<Vec<DeviceInfo>, String> {
        tracing::info!("Scanning for Bluetooth sensors");
        self.scanning.store(true, Ordering::Relaxed);
        self.set_scan_error(None);
        let started = std::time::Instant::now();
        let result = self.ble.scan().await;
        self.scanning.store(false, Ordering::Relaxed);
        match result {
            Err(message) => {
                tracing::error!(error = %message, "Scan failed");
                self.set_scan_error(Some(ScanError {
                    message: message.clone(),
                    guidance: ble::platform_guidance(),
                }));
                Err(message)
            }
            Ok(found) => {
                tracing::info!(
                    found = found.len(),
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "Scan finished"
                );
                let mut devices = trainer::simulated_devices();
                devices.extend(found);
                Ok(devices)
            }
        }
    }

    /// Scan and keep only devices usable as a trainer (pre-hub API).
    pub async fn scan_trainers(&self) -> Result<Vec<DeviceInfo>, String> {
        Ok(self
            .scan()
            .await?
            .into_iter()
            .filter(|device| device.simulated || device.supports(Capability::Ftms))
            .collect())
    }

    pub async fn connect(&self, role: DeviceRole, device: DeviceInfo) -> Result<(), String> {
        let _serialized = self.connect_lock.lock().await;
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
        match role {
            DeviceRole::Trainer => self.trainer.connect(device).await,
            other => {
                let slot = self.slot(other);
                slot.progress("warn", "Role not supported yet", Some(other.label().into()));
                Err(format!(
                    "{} support is coming in a later update",
                    other.label()
                ))
            }
        }
    }

    pub async fn disconnect_role(&self, role: DeviceRole) {
        match role {
            DeviceRole::Trainer => self.trainer.disconnect().await,
            other => {
                let slot = self.slot(other);
                slot.abort_worker().await;
                slot.record_disconnected();
                if !matches!(slot.state().await, DeviceState::Idle) {
                    slot.set_state(DeviceState::Idle).await;
                }
            }
        }
    }

    /// Disconnect everything (app quit).
    pub async fn disconnect(&self) {
        for role in DeviceRole::ALL {
            self.disconnect_role(role).await;
        }
    }

    // Trainer control, used by the workout runner.

    pub async fn begin_control(&self) -> Result<(), String> {
        self.trainer.begin_control().await
    }

    pub async fn set_target_power(&self, requested: u16, rider_max: u16) -> Result<u16, String> {
        self.trainer.set_target_power(requested, rider_max).await
    }

    pub async fn pause(&self) -> Result<(), String> {
        self.trainer.pause().await
    }

    pub async fn stop(&self) -> Result<(), String> {
        self.trainer.stop().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    async fn unsupported_role_is_rejected_and_logged() {
        let hub = DeviceHub::default();
        let device = DeviceInfo {
            id: "hr".into(),
            name: "Strap".into(),
            simulated: false,
            rssi: None,
            capabilities: vec![Capability::HeartRate],
        };
        assert!(
            hub.connect(DeviceRole::HeartRate, device.clone())
                .await
                .is_err()
        );
        assert_eq!(hub.slot(DeviceRole::HeartRate).log_lines().len(), 1);
        let wrong = hub.connect(DeviceRole::Trainer, device).await.unwrap_err();
        assert!(wrong.contains("does not advertise"));
    }

    #[test]
    fn state_serializes_with_status_tag() {
        let json = serde_json::to_string(&DeviceState::Connecting { name: "K".into() }).unwrap();
        assert_eq!(json, r#"{"status":"connecting","name":"K"}"#);
    }
}
