use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU16, Ordering},
    },
    time::Duration,
};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::{sync::RwLock, task::JoinHandle};
use uuid::Uuid;

use crate::{
    devices::DeviceHub,
    domain::{Interval, SessionSummary, Workout},
    storage::Storage,
};

const RUNNING: u8 = 0;
const PAUSED: u8 = 1;
const SKIP: u8 = 2;
const STOPPED: u8 = 3;
const MANUAL_START_WATTS: u16 = 100;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RunnerState {
    Idle,
    Countdown {
        seconds: u8,
        workout_name: String,
    },
    Running {
        session_id: Uuid,
        workout_name: String,
        elapsed_seconds: u32,
        total_seconds: Option<u32>,
        interval_index: usize,
        interval_elapsed_seconds: u32,
        target_power_watts: Option<u16>,
        manual_erg: bool,
    },
    Paused {
        session_id: Uuid,
        workout_name: String,
        elapsed_seconds: u32,
        total_seconds: Option<u32>,
        interval_index: usize,
        target_power_watts: Option<u16>,
        manual_erg: bool,
    },
    Finished {
        session_id: Uuid,
        completed: bool,
    },
    Error {
        message: String,
    },
}

pub struct WorkoutRunner {
    state: Arc<RwLock<RunnerState>>,
    control: Arc<AtomicU8>,
    manual_target: Arc<AtomicU16>,
    manual_active: Arc<AtomicBool>,
    standalone: Arc<AtomicBool>,
    manual_adjustment_lock: tokio::sync::Mutex<()>,
    worker: tokio::sync::Mutex<Option<JoinHandle<()>>>,
}

impl Default for WorkoutRunner {
    fn default() -> Self {
        Self {
            state: Arc::new(RwLock::new(RunnerState::Idle)),
            control: Arc::new(AtomicU8::new(STOPPED)),
            manual_target: Arc::new(AtomicU16::new(MANUAL_START_WATTS)),
            manual_active: Arc::new(AtomicBool::new(false)),
            standalone: Arc::new(AtomicBool::new(false)),
            manual_adjustment_lock: tokio::sync::Mutex::new(()),
            worker: tokio::sync::Mutex::new(None),
        }
    }
}

impl WorkoutRunner {
    pub async fn state(&self) -> RunnerState {
        self.state.read().await.clone()
    }

    pub async fn start(
        &self,
        app: AppHandle,
        workout: Workout,
        ftp: u16,
        rider_max: u16,
        devices: Arc<DeviceHub>,
        storage: Arc<Storage>,
    ) -> Result<Uuid, String> {
        workout.validate().map_err(|error| {
            tracing::warn!(workout = %workout.name, error = %error, "Workout failed validation");
            error
        })?;
        let intervals = workout.compile(ftp);
        let total_seconds = intervals.iter().map(|step| step.duration_seconds).sum();
        self.start_ride(
            app,
            workout.name,
            Some(workout.id),
            intervals,
            Some(total_seconds),
            false,
            rider_max,
            devices,
            storage,
        )
        .await
    }

    pub async fn start_free_ride(
        &self,
        app: AppHandle,
        rider_max: u16,
        devices: Arc<DeviceHub>,
        storage: Arc<Storage>,
    ) -> Result<Uuid, String> {
        self.start_ride(
            app,
            "Free Ride".into(),
            None,
            vec![Interval {
                duration_seconds: u32::MAX,
                start_watts: None,
                end_watts: None,
                free_ride: true,
            }],
            None,
            true,
            rider_max,
            devices,
            storage,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn start_ride(
        &self,
        app: AppHandle,
        ride_name: String,
        workout_id: Option<Uuid>,
        intervals: Vec<Interval>,
        total_seconds: Option<u32>,
        standalone: bool,
        rider_max: u16,
        devices: Arc<DeviceHub>,
        storage: Arc<Storage>,
    ) -> Result<Uuid, String> {
        let current = self.state.read().await.clone();
        if !matches!(
            current,
            RunnerState::Idle | RunnerState::Finished { .. } | RunnerState::Error { .. }
        ) {
            tracing::warn!(state = ?current, "start ride rejected: already active");
            return Err("A ride is already active".into());
        }
        devices.begin_control().await?;
        let session = storage.start_session(workout_id, &ride_name)?;
        let session_id = session.id;
        tracing::info!(
            session_id = %session_id,
            workout_id = ?workout_id,
            ride = %ride_name,
            rider_max,
            intervals = intervals.len(),
            total_seconds = ?total_seconds,
            standalone,
            "Ride starting"
        );
        self.control.store(RUNNING, Ordering::Relaxed);
        self.manual_active.store(false, Ordering::Relaxed);
        self.standalone.store(standalone, Ordering::Relaxed);
        *self.state.write().await = RunnerState::Countdown {
            seconds: 3,
            workout_name: ride_name.clone(),
        };
        emit_state(&app, &self.state).await;

        let state = self.state.clone();
        let control = self.control.clone();
        let manual_target = self.manual_target.clone();
        let manual_active = self.manual_active.clone();
        let worker = tokio::spawn(async move {
            for remaining in (1..=3).rev() {
                *state.write().await = RunnerState::Countdown {
                    seconds: remaining,
                    workout_name: ride_name.clone(),
                };
                emit_state(&app, &state).await;
                tokio::time::sleep(Duration::from_secs(1)).await;
                if control.load(Ordering::Relaxed) == STOPPED {
                    let _ = finish(
                        &app,
                        &storage,
                        &state,
                        session.clone(),
                        0,
                        0,
                        0,
                        0.0,
                        0,
                        0,
                        false,
                    )
                    .await;
                    return;
                }
            }
            let result = run_timeline(
                &app,
                &ride_name,
                intervals,
                total_seconds,
                standalone,
                rider_max,
                session,
                devices.clone(),
                storage.clone(),
                state.clone(),
                control.clone(),
                manual_target,
                manual_active.clone(),
            )
            .await;
            manual_active.store(false, Ordering::Relaxed);
            if let Err(message) = result {
                tracing::error!(session_id = %session_id, error = %message, "Workout aborted with error");
                if let Err(error) = devices.stop().await {
                    tracing::warn!(error = %error, "Could not stop trainer after workout error");
                }
                *state.write().await = RunnerState::Error { message };
                emit_state(&app, &state).await;
            }
        });
        *self.worker.lock().await = Some(worker);
        Ok(session_id)
    }

    pub async fn adjust_manual_power(
        &self,
        app: &AppHandle,
        delta: i16,
        rider_max: u16,
        devices: &DeviceHub,
    ) -> Result<u16, String> {
        if !matches!(delta, -5 | 5) {
            return Err("Power adjustments must be 5 watts".into());
        }
        let _adjustment_guard = self.manual_adjustment_lock.lock().await;
        if !manual_adjustment_allowed(
            self.control.load(Ordering::Relaxed),
            self.manual_active.load(Ordering::Relaxed),
        ) {
            return Err("Manual ERG control is not active".into());
        }
        let current = self.manual_target.load(Ordering::Relaxed);
        let requested = adjusted_target(current, delta);
        self.apply_manual_power(app, requested, rider_max, devices)
            .await
    }

    pub async fn set_manual_power(
        &self,
        app: &AppHandle,
        watts: u16,
        rider_max: u16,
        devices: &DeviceHub,
    ) -> Result<u16, String> {
        if watts == 0 {
            return Err("Target power must be greater than zero".into());
        }
        let _adjustment_guard = self.manual_adjustment_lock.lock().await;
        if !manual_adjustment_allowed(
            self.control.load(Ordering::Relaxed),
            self.manual_active.load(Ordering::Relaxed),
        ) {
            return Err("Manual ERG control is not active".into());
        }
        self.apply_manual_power(app, watts, rider_max, devices)
            .await
    }

    async fn apply_manual_power(
        &self,
        app: &AppHandle,
        requested: u16,
        rider_max: u16,
        devices: &DeviceHub,
    ) -> Result<u16, String> {
        let clamped = devices.set_target_power(requested, rider_max).await?;
        self.manual_target.store(clamped, Ordering::Relaxed);
        {
            let mut state = self.state.write().await;
            let RunnerState::Running {
                target_power_watts,
                manual_erg: true,
                ..
            } = &mut *state
            else {
                return Err("Manual ERG control is not active".into());
            };
            *target_power_watts = Some(clamped);
        }
        emit_state(app, &self.state).await;
        Ok(clamped)
    }

    pub async fn pause_or_resume(&self, devices: &DeviceHub) -> Result<(), String> {
        let current = self.control.load(Ordering::Relaxed);
        if current == RUNNING {
            tracing::info!("Workout paused");
            devices.pause().await?;
            self.control.store(PAUSED, Ordering::Relaxed);
        } else if current == PAUSED {
            tracing::info!("Workout resumed");
            devices.begin_control().await?;
            self.control.store(RUNNING, Ordering::Relaxed);
        } else {
            tracing::warn!(control = current, "pause_or_resume with no active workout");
            return Err("No workout can be paused or resumed".into());
        }
        Ok(())
    }

    pub fn skip(&self) -> Result<(), String> {
        if self.standalone.load(Ordering::Relaxed) {
            return Err("Free rides do not have intervals to skip".into());
        }
        if self.control.load(Ordering::Relaxed) <= PAUSED {
            tracing::info!("Interval skip requested");
            self.control.store(SKIP, Ordering::Relaxed);
            Ok(())
        } else {
            Err("No active interval to skip".into())
        }
    }

    pub async fn stop(&self, devices: &DeviceHub) -> Result<(), String> {
        tracing::info!("Workout stop requested");
        self.control.store(STOPPED, Ordering::Relaxed);
        self.manual_active.store(false, Ordering::Relaxed);
        devices.stop().await?;
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_timeline(
    app: &AppHandle,
    ride_name: &str,
    intervals: Vec<Interval>,
    total_seconds: Option<u32>,
    standalone: bool,
    rider_max: u16,
    summary: SessionSummary,
    devices: Arc<DeviceHub>,
    storage: Arc<Storage>,
    state: Arc<RwLock<RunnerState>>,
    control: Arc<AtomicU8>,
    manual_target: Arc<AtomicU16>,
    manual_active: Arc<AtomicBool>,
) -> Result<(), String> {
    let mut telemetry_rx = devices.subscribe();
    let mut elapsed = 0_u32;
    let mut power_total = 0_u64;
    let mut cadence_total = 0.0_f64;
    let mut cadence_samples = 0_u32;
    let mut samples = 0_u32;
    let mut max_power = 0_u16;

    for (interval_index, interval) in intervals.iter().enumerate() {
        if interval.free_ride {
            let clamped = devices
                .set_target_power(MANUAL_START_WATTS, rider_max)
                .await?;
            manual_target.store(clamped, Ordering::Relaxed);
            manual_active.store(true, Ordering::Relaxed);
        } else {
            manual_active.store(false, Ordering::Relaxed);
        }
        tracing::info!(
            session_id = %summary.id,
            interval_index,
            duration_seconds = interval.duration_seconds,
            elapsed,
            "Interval started"
        );
        let mut interval_elapsed = 0_u32;
        while interval_elapsed < interval.duration_seconds {
            match control.load(Ordering::Relaxed) {
                STOPPED => {
                    finish(
                        app,
                        &storage,
                        &state,
                        summary,
                        elapsed,
                        power_total,
                        max_power,
                        cadence_total,
                        cadence_samples,
                        samples,
                        standalone,
                    )
                    .await?;
                    return Ok(());
                }
                SKIP => {
                    manual_active.store(false, Ordering::Relaxed);
                    control.store(RUNNING, Ordering::Relaxed);
                    break;
                }
                PAUSED => {
                    *state.write().await = RunnerState::Paused {
                        session_id: summary.id,
                        workout_name: ride_name.to_string(),
                        elapsed_seconds: elapsed,
                        total_seconds,
                        interval_index,
                        target_power_watts: if interval.free_ride {
                            Some(manual_target.load(Ordering::Relaxed))
                        } else {
                            target_at(interval, interval_elapsed)
                        },
                        manual_erg: interval.free_ride,
                    };
                    emit_state(app, &state).await;
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    continue;
                }
                _ => {}
            }

            let mut target = if interval.free_ride {
                Some(manual_target.load(Ordering::Relaxed))
            } else {
                target_at(interval, interval_elapsed)
            };
            if let Some(watts) = target {
                let clamped = devices.set_target_power(watts, rider_max).await?;
                if interval.free_ride {
                    manual_target.store(clamped, Ordering::Relaxed);
                    target = Some(clamped);
                }
            }
            *state.write().await = RunnerState::Running {
                session_id: summary.id,
                workout_name: ride_name.to_string(),
                elapsed_seconds: elapsed,
                total_seconds,
                interval_index,
                interval_elapsed_seconds: interval_elapsed,
                target_power_watts: target,
                manual_erg: interval.free_ride,
            };
            emit_state(app, &state).await;

            let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
            while tokio::time::Instant::now() < deadline {
                let wait = deadline.saturating_duration_since(tokio::time::Instant::now());
                match tokio::time::timeout(wait, telemetry_rx.recv()).await {
                    Ok(Ok(mut sample)) => {
                        sample.target_power_watts = target;
                        storage.record_sample(summary.id, &sample)?;
                        samples += 1;
                        power_total += u64::from(sample.power_watts);
                        max_power = max_power.max(sample.power_watts);
                        if let Some(cadence) = sample.cadence_rpm {
                            cadence_total += f64::from(cadence);
                            cadence_samples += 1;
                        }
                    }
                    Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
                    _ => break,
                }
            }
            elapsed += 1;
            interval_elapsed += 1;
        }
        manual_active.store(false, Ordering::Relaxed);
    }
    devices.stop().await?;
    finish(
        app,
        &storage,
        &state,
        summary,
        elapsed,
        power_total,
        max_power,
        cadence_total,
        cadence_samples,
        samples,
        true,
    )
    .await
}

fn adjusted_target(current: u16, delta: i16) -> u16 {
    if delta.is_negative() {
        current.saturating_sub(delta.unsigned_abs())
    } else {
        current.saturating_add(delta as u16)
    }
}

fn manual_adjustment_allowed(control: u8, manual_active: bool) -> bool {
    control == RUNNING && manual_active
}

fn target_at(interval: &Interval, elapsed: u32) -> Option<u16> {
    let start = interval.start_watts?;
    let end = interval.end_watts?;
    if interval.duration_seconds <= 1 || start == end {
        return Some(start);
    }
    let fraction = elapsed as f32 / (interval.duration_seconds - 1) as f32;
    Some((start as f32 + (end as f32 - start as f32) * fraction).round() as u16)
}

#[allow(clippy::too_many_arguments)]
async fn finish(
    app: &AppHandle,
    storage: &Storage,
    state: &Arc<RwLock<RunnerState>>,
    mut summary: SessionSummary,
    elapsed: u32,
    power_total: u64,
    max_power: u16,
    cadence_total: f64,
    cadence_samples: u32,
    samples: u32,
    completed: bool,
) -> Result<(), String> {
    summary.ended_at = Some(Utc::now());
    summary.elapsed_seconds = elapsed;
    summary.average_power_watts = if samples == 0 {
        0
    } else {
        (power_total / u64::from(samples)) as u16
    };
    summary.max_power_watts = max_power;
    summary.average_cadence_rpm =
        (cadence_samples > 0).then_some((cadence_total / f64::from(cadence_samples)) as f32);
    summary.completed = completed;
    tracing::info!(
        session_id = %summary.id,
        completed,
        elapsed_seconds = elapsed,
        samples,
        average_power_watts = summary.average_power_watts,
        max_power_watts = summary.max_power_watts,
        "Workout finished"
    );
    storage.finish_session(&summary).map_err(|error| {
        tracing::error!(session_id = %summary.id, error = %error, "Could not persist session summary");
        error
    })?;
    *state.write().await = RunnerState::Finished {
        session_id: summary.id,
        completed,
    };
    emit_state(app, state).await;
    Ok(())
}

async fn emit_state(app: &AppHandle, state: &RwLock<RunnerState>) {
    let _ = app.emit("workout://state", state.read().await.clone());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolates_ramp_targets() {
        let interval = Interval {
            duration_seconds: 3,
            start_watts: Some(100),
            end_watts: Some(200),
            free_ride: false,
        };
        assert_eq!(target_at(&interval, 0), Some(100));
        assert_eq!(target_at(&interval, 1), Some(150));
        assert_eq!(target_at(&interval, 2), Some(200));
    }

    #[test]
    fn adjusts_manual_targets_in_five_watt_steps() {
        let runner = WorkoutRunner::default();
        assert_eq!(
            runner.manual_target.load(Ordering::Relaxed),
            MANUAL_START_WATTS
        );
        assert_eq!(adjusted_target(MANUAL_START_WATTS, 5), 105);
        assert_eq!(adjusted_target(MANUAL_START_WATTS, -5), 95);
    }

    #[test]
    fn manual_target_adjustment_saturates_at_numeric_bounds() {
        assert_eq!(adjusted_target(2, -5), 0);
        assert_eq!(adjusted_target(u16::MAX - 2, 5), u16::MAX);
    }

    #[test]
    fn manual_adjustment_requires_a_running_manual_interval() {
        assert!(manual_adjustment_allowed(RUNNING, true));
        assert!(!manual_adjustment_allowed(RUNNING, false));
        assert!(!manual_adjustment_allowed(PAUSED, true));
        assert!(!manual_adjustment_allowed(STOPPED, true));
    }

    #[test]
    fn runner_state_fields_are_camel_case_for_the_frontend() {
        let state = RunnerState::Running {
            session_id: Uuid::nil(),
            workout_name: "Free Ride".into(),
            elapsed_seconds: 12,
            total_seconds: None,
            interval_index: 0,
            interval_elapsed_seconds: 12,
            target_power_watts: Some(100),
            manual_erg: true,
        };
        let json = serde_json::to_value(state).unwrap();

        assert_eq!(json["workoutName"], "Free Ride");
        assert_eq!(json["targetPowerWatts"], 100);
        assert_eq!(json["manualErg"], true);
        assert!(json.get("workout_name").is_none());
    }
}
