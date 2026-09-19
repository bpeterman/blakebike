//! Merges readings from every connected device into the single `Telemetry`
//! stream the runner, recorder and ride screen consume.
//!
//! Each metric (power, cadence, heart rate) has a user preference: `Auto` or a
//! specific role. `Auto` means "dedicated sensor wins": a heart-rate strap over
//! the trainer's HR field, a power meter over the trainer, a cadence sensor
//! over a power meter's crank data over the trainer. Whatever is chosen, a
//! source whose last reading is stale falls back down the default order.

use std::{collections::HashMap, sync::Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::sync::broadcast;

use super::DeviceRole;
use crate::domain::Telemetry;

/// A metric can come from the device the user picked, or from the best
/// available one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "mode", content = "role", rename_all = "camelCase")]
pub enum SourceChoice {
    #[default]
    Auto,
    Role(DeviceRole),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct SourcePreferences {
    pub power: SourceChoice,
    pub cadence: SourceChoice,
    pub heart_rate: SourceChoice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Metric {
    Power,
    Cadence,
    HeartRate,
}

impl Metric {
    /// Default priority, best first.
    pub fn default_order(self) -> &'static [DeviceRole] {
        match self {
            Metric::Power => &[DeviceRole::Power, DeviceRole::Trainer],
            Metric::Cadence => &[DeviceRole::Cadence, DeviceRole::Power, DeviceRole::Trainer],
            Metric::HeartRate => &[DeviceRole::HeartRate, DeviceRole::Trainer],
        }
    }

    /// How old a reading may be before it is ignored and the next source
    /// takes over. Straps report about once a second; trainers 2–4 times.
    pub fn stale_after_ms(self) -> i64 {
        match self {
            Metric::HeartRate => 5_000,
            Metric::Power | Metric::Cadence => 3_000,
        }
    }

    fn choice(self, preferences: &SourcePreferences) -> SourceChoice {
        match self {
            Metric::Power => preferences.power,
            Metric::Cadence => preferences.cadence,
            Metric::HeartRate => preferences.heart_rate,
        }
    }
}

/// One device's contribution.
#[derive(Debug, Clone, PartialEq)]
pub enum Reading {
    /// Indoor Bike Data: may carry power, cadence, speed and heart rate.
    Trainer(Telemetry),
    HeartRate {
        bpm: u16,
        sensor_contact: Option<bool>,
    },
    Power {
        watts: u16,
        cadence_rpm: Option<f32>,
        /// Left-pedal share when the meter reports balance (display only).
        balance_left_percent: Option<f32>,
    },
    Cadence {
        rpm: f32,
    },
}

/// Which device supplied each fused value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub role: DeviceRole,
    /// True when the user's chosen source was unavailable or stale and this
    /// role stepped in.
    pub fallback: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TelemetrySources {
    pub power: Option<Source>,
    pub cadence: Option<Source>,
    pub heart_rate: Option<Source>,
}

/// What the UI receives on `trainer://telemetry`: the fused sample plus its
/// provenance. Flattened so the existing frontend `Telemetry` type keeps
/// working unchanged.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FusedTelemetry {
    #[serde(flatten)]
    pub telemetry: Telemetry,
    pub sources: TelemetrySources,
}

#[derive(Debug, Clone, Copy)]
struct Latest {
    value: f32,
    at_ms: i64,
}

#[derive(Default)]
struct Inner {
    preferences: SourcePreferences,
    latest: HashMap<(Metric, DeviceRole), Latest>,
    /// Speed and target only come from the trainer.
    trainer_speed_kph: Option<f32>,
    trainer_target_watts: Option<u16>,
    last_emit_ms: i64,
    last_sources: TelemetrySources,
}

/// Minimum spacing between fused samples sent downstream.
const MIN_EMIT_INTERVAL_MS: i64 = 200;

pub struct TelemetryFuser {
    app: Option<AppHandle>,
    inner: Mutex<Inner>,
    telemetry: broadcast::Sender<Telemetry>,
}

impl TelemetryFuser {
    pub fn new(app: Option<AppHandle>, telemetry: broadcast::Sender<Telemetry>) -> Self {
        Self {
            app,
            inner: Mutex::new(Inner::default()),
            telemetry,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn preferences(&self) -> SourcePreferences {
        self.lock().preferences
    }

    pub fn set_preferences(&self, preferences: SourcePreferences) {
        tracing::info!(?preferences, "Telemetry source preferences changed");
        self.lock().preferences = preferences;
    }

    /// The provenance of the most recently fused sample.
    pub fn sources(&self) -> TelemetrySources {
        self.lock().last_sources
    }

    /// Drop everything a device contributed, so nothing lingers after it
    /// disconnects (staleness would otherwise hide it for a few seconds).
    pub fn forget(&self, role: DeviceRole) {
        let mut inner = self.lock();
        inner.latest.retain(|(_, source), _| *source != role);
        if role == DeviceRole::Trainer {
            inner.trainer_speed_kph = None;
            inner.trainer_target_watts = None;
        }
    }

    /// Feed one reading. Returns the fused sample if one was published.
    pub fn ingest(
        &self,
        role: DeviceRole,
        reading: Reading,
        now_ms: i64,
    ) -> Option<FusedTelemetry> {
        let fused = {
            let mut inner = self.lock();
            match reading {
                Reading::Trainer(telemetry) => {
                    inner.record(Metric::Power, role, telemetry.power_watts as f32, now_ms);
                    if let Some(cadence) = telemetry.cadence_rpm {
                        inner.record(Metric::Cadence, role, cadence, now_ms);
                    }
                    if let Some(bpm) = telemetry.heart_rate_bpm.filter(|bpm| *bpm > 0) {
                        inner.record(Metric::HeartRate, role, bpm as f32, now_ms);
                    }
                    inner.trainer_speed_kph = telemetry.speed_kph;
                    inner.trainer_target_watts = telemetry.target_power_watts;
                }
                Reading::HeartRate { bpm, .. } => {
                    if bpm > 0 {
                        inner.record(Metric::HeartRate, role, bpm as f32, now_ms);
                    }
                }
                Reading::Power {
                    watts, cadence_rpm, ..
                } => {
                    inner.record(Metric::Power, role, watts as f32, now_ms);
                    if let Some(cadence) = cadence_rpm {
                        inner.record(Metric::Cadence, role, cadence, now_ms);
                    }
                }
                Reading::Cadence { rpm } => inner.record(Metric::Cadence, role, rpm, now_ms),
            }
            if now_ms - inner.last_emit_ms < MIN_EMIT_INTERVAL_MS {
                return None;
            }
            inner.last_emit_ms = now_ms;
            let fused = inner.fuse(now_ms);
            inner.last_sources = fused.sources;
            fused
        };
        let _ = self.telemetry.send(fused.telemetry.clone());
        if let Some(app) = &self.app
            && let Err(error) = app.emit("trainer://telemetry", fused.clone())
        {
            tracing::warn!(error = %error, "Could not emit telemetry to UI");
        }
        Some(fused)
    }

    /// Fuse without ingesting; used by tests and for a snapshot of the state.
    #[cfg(test)]
    pub fn current(&self, now_ms: i64) -> FusedTelemetry {
        self.lock().fuse(now_ms)
    }
}

impl Inner {
    fn record(&mut self, metric: Metric, role: DeviceRole, value: f32, at_ms: i64) {
        self.latest.insert((metric, role), Latest { value, at_ms });
    }

    fn fresh(&self, metric: Metric, role: DeviceRole, now_ms: i64) -> Option<f32> {
        self.latest
            .get(&(metric, role))
            .filter(|latest| now_ms - latest.at_ms <= metric.stale_after_ms())
            .map(|latest| latest.value)
    }

    fn pick(&self, metric: Metric, now_ms: i64) -> Option<(f32, Source)> {
        let chosen = match metric.choice(&self.preferences) {
            SourceChoice::Role(role) => Some(role),
            SourceChoice::Auto => None,
        };
        if let Some(role) = chosen
            && let Some(value) = self.fresh(metric, role, now_ms)
        {
            return Some((
                value,
                Source {
                    role,
                    fallback: false,
                },
            ));
        }
        for role in metric.default_order() {
            if let Some(value) = self.fresh(metric, *role, now_ms) {
                // In Auto mode the first fresh source in the default order is
                // the intended one, not a fallback.
                let fallback = chosen.is_some();
                return Some((
                    value,
                    Source {
                        role: *role,
                        fallback,
                    },
                ));
            }
        }
        None
    }

    fn fuse(&self, now_ms: i64) -> FusedTelemetry {
        let power = self.pick(Metric::Power, now_ms);
        let cadence = self.pick(Metric::Cadence, now_ms);
        let heart_rate = self.pick(Metric::HeartRate, now_ms);
        FusedTelemetry {
            telemetry: Telemetry {
                timestamp_ms: now_ms,
                power_watts: power
                    .map(|(value, _)| value.round().max(0.0) as u16)
                    .unwrap_or(0),
                cadence_rpm: cadence.map(|(value, _)| value),
                speed_kph: self.trainer_speed_kph,
                heart_rate_bpm: heart_rate.map(|(value, _)| value.round().clamp(0.0, 255.0) as u8),
                target_power_watts: self.trainer_target_watts,
            },
            sources: TelemetrySources {
                power: power.map(|(_, source)| source),
                cadence: cadence.map(|(_, source)| source),
                heart_rate: heart_rate.map(|(_, source)| source),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fuser() -> (TelemetryFuser, broadcast::Receiver<Telemetry>) {
        let (tx, rx) = broadcast::channel(16);
        (TelemetryFuser::new(None, tx), rx)
    }

    fn trainer_sample(power: u16, cadence: Option<f32>, hr: Option<u8>, at: i64) -> Reading {
        Reading::Trainer(Telemetry {
            timestamp_ms: at,
            power_watts: power,
            cadence_rpm: cadence,
            speed_kph: Some(30.0),
            heart_rate_bpm: hr,
            target_power_watts: Some(200),
        })
    }

    #[test]
    fn trainer_alone_passes_through() {
        let (fuser, mut rx) = fuser();
        let fused = fuser
            .ingest(
                DeviceRole::Trainer,
                trainer_sample(210, Some(90.0), Some(140), 1_000),
                1_000,
            )
            .unwrap();
        assert_eq!(fused.telemetry.power_watts, 210);
        assert_eq!(fused.telemetry.cadence_rpm, Some(90.0));
        assert_eq!(fused.telemetry.heart_rate_bpm, Some(140));
        assert_eq!(fused.telemetry.speed_kph, Some(30.0));
        assert_eq!(fused.telemetry.target_power_watts, Some(200));
        assert_eq!(
            fused.sources.heart_rate,
            Some(Source {
                role: DeviceRole::Trainer,
                fallback: false
            })
        );
        assert_eq!(rx.try_recv().unwrap().power_watts, 210);
    }

    #[test]
    fn strap_beats_trainer_heart_rate_in_auto() {
        let (fuser, _rx) = fuser();
        fuser.ingest(
            DeviceRole::Trainer,
            trainer_sample(200, None, Some(120), 1_000),
            1_000,
        );
        let fused = fuser
            .ingest(
                DeviceRole::HeartRate,
                Reading::HeartRate {
                    bpm: 155,
                    sensor_contact: Some(true),
                },
                1_500,
            )
            .unwrap();
        assert_eq!(fused.telemetry.heart_rate_bpm, Some(155));
        assert_eq!(
            fused.sources.heart_rate.unwrap().role,
            DeviceRole::HeartRate
        );
        assert!(!fused.sources.heart_rate.unwrap().fallback);
        assert_eq!(fused.telemetry.power_watts, 200);
    }

    #[test]
    fn stale_strap_falls_back_to_trainer() {
        let (fuser, _rx) = fuser();
        fuser.ingest(
            DeviceRole::HeartRate,
            Reading::HeartRate {
                bpm: 150,
                sensor_contact: None,
            },
            1_000,
        );
        // 6 s later only the trainer is talking.
        let fused = fuser
            .ingest(
                DeviceRole::Trainer,
                trainer_sample(200, None, Some(130), 7_000),
                7_000,
            )
            .unwrap();
        assert_eq!(fused.telemetry.heart_rate_bpm, Some(130));
        assert_eq!(fused.sources.heart_rate.unwrap().role, DeviceRole::Trainer);
    }

    #[test]
    fn explicit_choice_marks_fallback() {
        let (fuser, _rx) = fuser();
        fuser.set_preferences(SourcePreferences {
            heart_rate: SourceChoice::Role(DeviceRole::HeartRate),
            ..SourcePreferences::default()
        });
        let fused = fuser
            .ingest(
                DeviceRole::Trainer,
                trainer_sample(200, None, Some(130), 1_000),
                1_000,
            )
            .unwrap();
        let source = fused.sources.heart_rate.unwrap();
        assert_eq!(source.role, DeviceRole::Trainer);
        assert!(source.fallback);
    }

    #[test]
    fn user_can_prefer_trainer_over_strap() {
        let (fuser, _rx) = fuser();
        fuser.set_preferences(SourcePreferences {
            heart_rate: SourceChoice::Role(DeviceRole::Trainer),
            ..SourcePreferences::default()
        });
        fuser.ingest(
            DeviceRole::HeartRate,
            Reading::HeartRate {
                bpm: 155,
                sensor_contact: None,
            },
            1_000,
        );
        let fused = fuser
            .ingest(
                DeviceRole::Trainer,
                trainer_sample(200, None, Some(120), 1_400),
                1_400,
            )
            .unwrap();
        assert_eq!(fused.telemetry.heart_rate_bpm, Some(120));
        assert!(!fused.sources.heart_rate.unwrap().fallback);
    }

    #[test]
    fn coalesces_bursts() {
        let (fuser, _rx) = fuser();
        assert!(
            fuser
                .ingest(
                    DeviceRole::Trainer,
                    trainer_sample(1, None, None, 1_000),
                    1_000
                )
                .is_some()
        );
        assert!(
            fuser
                .ingest(
                    DeviceRole::Trainer,
                    trainer_sample(2, None, None, 1_050),
                    1_050
                )
                .is_none()
        );
        assert!(
            fuser
                .ingest(
                    DeviceRole::Trainer,
                    trainer_sample(3, None, None, 1_300),
                    1_300
                )
                .is_some()
        );
    }

    #[test]
    fn forget_clears_a_device() {
        let (fuser, _rx) = fuser();
        fuser.ingest(
            DeviceRole::HeartRate,
            Reading::HeartRate {
                bpm: 150,
                sensor_contact: None,
            },
            1_000,
        );
        fuser.forget(DeviceRole::HeartRate);
        assert_eq!(fuser.current(1_100).telemetry.heart_rate_bpm, None);
    }

    #[test]
    fn zero_heart_rate_is_not_a_reading() {
        let (fuser, _rx) = fuser();
        fuser.ingest(
            DeviceRole::Trainer,
            trainer_sample(200, None, Some(0), 1_000),
            1_000,
        );
        assert_eq!(fuser.current(1_000).telemetry.heart_rate_bpm, None);
    }

    #[test]
    fn power_meter_beats_trainer_and_supplies_cadence() {
        let (fuser, _rx) = fuser();
        fuser.ingest(
            DeviceRole::Trainer,
            trainer_sample(200, Some(80.0), None, 1_000),
            1_000,
        );
        let fused = fuser
            .ingest(
                DeviceRole::Power,
                Reading::Power {
                    watts: 215,
                    cadence_rpm: Some(88.0),
                    balance_left_percent: Some(50.0),
                },
                1_400,
            )
            .unwrap();
        assert_eq!(fused.telemetry.power_watts, 215);
        assert_eq!(fused.telemetry.cadence_rpm, Some(88.0));
        assert_eq!(fused.sources.power.unwrap().role, DeviceRole::Power);
        assert_eq!(fused.sources.cadence.unwrap().role, DeviceRole::Power);
        // Speed and target still come from the trainer.
        assert_eq!(fused.telemetry.speed_kph, Some(30.0));
        assert_eq!(fused.telemetry.target_power_watts, Some(200));
    }

    #[test]
    fn cadence_sensor_beats_power_meter_crank_data() {
        let (fuser, _rx) = fuser();
        fuser.ingest(
            DeviceRole::Power,
            Reading::Power {
                watts: 215,
                cadence_rpm: Some(88.0),
                balance_left_percent: None,
            },
            1_000,
        );
        let fused = fuser
            .ingest(DeviceRole::Cadence, Reading::Cadence { rpm: 91.0 }, 1_300)
            .unwrap();
        assert_eq!(fused.telemetry.cadence_rpm, Some(91.0));
        assert_eq!(fused.sources.cadence.unwrap().role, DeviceRole::Cadence);
        assert_eq!(fused.sources.power.unwrap().role, DeviceRole::Power);
    }

    #[test]
    fn power_meter_without_crank_data_leaves_cadence_to_trainer() {
        let (fuser, _rx) = fuser();
        fuser.ingest(
            DeviceRole::Trainer,
            trainer_sample(200, Some(80.0), None, 1_000),
            1_000,
        );
        let fused = fuser
            .ingest(
                DeviceRole::Power,
                Reading::Power {
                    watts: 215,
                    cadence_rpm: None,
                    balance_left_percent: None,
                },
                1_400,
            )
            .unwrap();
        assert_eq!(fused.telemetry.cadence_rpm, Some(80.0));
        assert_eq!(fused.sources.cadence.unwrap().role, DeviceRole::Trainer);
    }

    #[test]
    fn preferences_serialize_for_the_frontend() {
        let preferences = SourcePreferences {
            power: SourceChoice::Auto,
            cadence: SourceChoice::Role(DeviceRole::Power),
            heart_rate: SourceChoice::Auto,
        };
        let json = serde_json::to_string(&preferences).unwrap();
        assert_eq!(
            json,
            r#"{"power":{"mode":"auto"},"cadence":{"mode":"role","role":"power"},"heartRate":{"mode":"auto"}}"#
        );
        let parsed: SourcePreferences = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, preferences);
        let empty: SourcePreferences = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, SourcePreferences::default());
    }

    #[test]
    fn fused_telemetry_flattens_for_existing_ui_type() {
        let (fuser, _rx) = fuser();
        let fused = fuser
            .ingest(
                DeviceRole::Trainer,
                trainer_sample(200, Some(85.0), None, 1_000),
                1_000,
            )
            .unwrap();
        let json = serde_json::to_value(fused).unwrap();
        assert_eq!(json["powerWatts"], 200);
        assert_eq!(json["cadenceRpm"], 85.0);
        assert_eq!(json["sources"]["power"]["role"], "trainer");
        assert!(json["sources"]["heartRate"].is_null());
    }
}
