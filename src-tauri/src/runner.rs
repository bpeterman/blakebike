use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU16, Ordering},
    },
    time::Duration,
};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio::{sync::RwLock, task::JoinHandle};
use uuid::Uuid;

use crate::{
    AppState,
    devices::DeviceHub,
    distance::estimate_distance,
    domain::{Interval, SessionSummary, Workout},
    fit::ensure_ride_file,
    storage::Storage,
};

const RUNNING: u8 = 0;
const PAUSED: u8 = 1;
const SKIP: u8 = 2;
const STOPPED: u8 = 3;
const MANUAL_START_WATTS: u16 = 100;
/// Overall workout bias (percent of the planned target) and its allowed range.
pub const DEFAULT_BIAS_PERCENT: u16 = 100;
pub const MIN_BIAS_PERCENT: u16 = 50;
pub const MAX_BIAS_PERCENT: u16 = 150;
/// `override_target == NO_OVERRIDE` means the planned target is in force.
const NO_OVERRIDE: u16 = 0;

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
        /// Target currently sent to the trainer (after bias and any override).
        target_power_watts: Option<u16>,
        /// Target the workout plan asks for right now, before bias/override.
        planned_target_watts: Option<u16>,
        /// True while a free-ride interval is running (manual ERG).
        manual_erg: bool,
        /// True while the rider has overridden this interval's target.
        override_active: bool,
        bias_percent: u16,
    },
    Paused {
        session_id: Uuid,
        workout_name: String,
        elapsed_seconds: u32,
        total_seconds: Option<u32>,
        interval_index: usize,
        interval_elapsed_seconds: u32,
        target_power_watts: Option<u16>,
        planned_target_watts: Option<u16>,
        manual_erg: bool,
        override_active: bool,
        bias_percent: u16,
    },
    Finished {
        session_id: Uuid,
        completed: bool,
    },
    Error {
        message: String,
    },
}

/// Lock-free knobs shared between the command handlers and the timeline task.
#[derive(Clone)]
struct Controls {
    control: Arc<AtomicU8>,
    /// Target of the running free-ride interval (manual ERG).
    manual_target: Arc<AtomicU16>,
    /// A free-ride interval is running.
    manual_active: Arc<AtomicBool>,
    /// A planned (non-free-ride) interval with a target is running.
    planned_active: Arc<AtomicBool>,
    /// Rider override for the current planned interval, `NO_OVERRIDE` when unset.
    override_target: Arc<AtomicU16>,
    /// The last target actually sent to the trainer; ± steps start from here.
    current_target: Arc<AtomicU16>,
    /// Overall bias applied to every planned target for this ride.
    bias_percent: Arc<AtomicU16>,
}

impl Default for Controls {
    fn default() -> Self {
        Self {
            control: Arc::new(AtomicU8::new(STOPPED)),
            manual_target: Arc::new(AtomicU16::new(MANUAL_START_WATTS)),
            manual_active: Arc::new(AtomicBool::new(false)),
            planned_active: Arc::new(AtomicBool::new(false)),
            override_target: Arc::new(AtomicU16::new(NO_OVERRIDE)),
            current_target: Arc::new(AtomicU16::new(0)),
            bias_percent: Arc::new(AtomicU16::new(DEFAULT_BIAS_PERCENT)),
        }
    }
}

impl Controls {
    fn bias(&self) -> u16 {
        self.bias_percent.load(Ordering::Relaxed)
    }

    fn override_target(&self) -> Option<u16> {
        match self.override_target.load(Ordering::Relaxed) {
            NO_OVERRIDE => None,
            watts => Some(watts),
        }
    }

    fn clear_override(&self) {
        self.override_target.store(NO_OVERRIDE, Ordering::Relaxed);
    }

    /// Target to send for a planned interval: the rider's override if set,
    /// otherwise the plan scaled by the bias.
    fn planned_effective(&self, planned: Option<u16>) -> Option<u16> {
        self.override_target()
            .or_else(|| planned.map(|watts| biased_target(watts, self.bias())))
    }
}

pub struct WorkoutRunner {
    state: Arc<RwLock<RunnerState>>,
    controls: Controls,
    standalone: Arc<AtomicBool>,
    manual_adjustment_lock: tokio::sync::Mutex<()>,
    worker: tokio::sync::Mutex<Option<JoinHandle<()>>>,
}

impl Default for WorkoutRunner {
    fn default() -> Self {
        Self {
            state: Arc::new(RwLock::new(RunnerState::Idle)),
            controls: Controls::default(),
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

    #[allow(clippy::too_many_arguments)]
    pub async fn start(
        &self,
        app: AppHandle,
        workout: Workout,
        ftp: u16,
        rider_max: u16,
        distance_weight_kg: f32,
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
            distance_weight_kg,
            devices,
            storage,
        )
        .await
    }

    pub async fn start_free_ride(
        &self,
        app: AppHandle,
        rider_max: u16,
        distance_weight_kg: f32,
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
            distance_weight_kg,
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
        distance_weight_kg: f32,
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
        let session = storage.start_session(workout_id, &ride_name, distance_weight_kg)?;
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
        self.controls.control.store(RUNNING, Ordering::Relaxed);
        self.controls.manual_active.store(false, Ordering::Relaxed);
        self.controls.planned_active.store(false, Ordering::Relaxed);
        self.controls.clear_override();
        self.controls
            .bias_percent
            .store(DEFAULT_BIAS_PERCENT, Ordering::Relaxed);
        self.standalone.store(standalone, Ordering::Relaxed);
        *self.state.write().await = RunnerState::Countdown {
            seconds: 3,
            workout_name: ride_name.clone(),
        };
        emit_state(&app, &self.state).await;

        let state = self.state.clone();
        let controls = self.controls.clone();
        let worker = tokio::spawn(async move {
            for remaining in (1..=3).rev() {
                *state.write().await = RunnerState::Countdown {
                    seconds: remaining,
                    workout_name: ride_name.clone(),
                };
                emit_state(&app, &state).await;
                tokio::time::sleep(Duration::from_secs(1)).await;
                if controls.control.load(Ordering::Relaxed) == STOPPED {
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
                controls.clone(),
            )
            .await;
            controls.manual_active.store(false, Ordering::Relaxed);
            controls.planned_active.store(false, Ordering::Relaxed);
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

    /// Nudge the target by ±5 W. In a free-ride interval this moves the manual
    /// ERG target; in a planned interval it overrides that interval's target
    /// until the next one starts.
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
        let mode = self.adjustment_mode()?;
        let current = match mode {
            Adjustment::Manual => self.controls.manual_target.load(Ordering::Relaxed),
            Adjustment::Override => self.controls.current_target.load(Ordering::Relaxed),
        };
        let requested = adjusted_target(current, delta);
        self.apply_target(app, requested, rider_max, devices, mode)
            .await
    }

    /// Set the target to an exact wattage; same free-ride / planned split as
    /// [`Self::adjust_manual_power`].
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
        let mode = self.adjustment_mode()?;
        self.apply_target(app, watts, rider_max, devices, mode)
            .await
    }

    /// Drop the rider's override for the current interval and go back to the
    /// (biased) plan. Returns the target now in force.
    pub async fn clear_target_override(
        &self,
        app: &AppHandle,
        rider_max: u16,
        devices: &DeviceHub,
    ) -> Result<Option<u16>, String> {
        let _adjustment_guard = self.manual_adjustment_lock.lock().await;
        if !manual_adjustment_allowed(
            self.controls.control.load(Ordering::Relaxed),
            self.controls.planned_active.load(Ordering::Relaxed),
        ) {
            return Err("No workout interval is running".into());
        }
        self.controls.clear_override();
        self.reapply_plan(app, rider_max, devices).await
    }

    /// Scale every planned target for the rest of this ride. Applies at once
    /// unless the current interval is overridden or free ride.
    pub async fn set_bias_percent(
        &self,
        app: &AppHandle,
        percent: u16,
        rider_max: u16,
        devices: &DeviceHub,
    ) -> Result<u16, String> {
        let percent = percent.clamp(MIN_BIAS_PERCENT, MAX_BIAS_PERCENT);
        let _adjustment_guard = self.manual_adjustment_lock.lock().await;
        if self.controls.control.load(Ordering::Relaxed) > PAUSED {
            return Err("No ride is active".into());
        }
        self.controls.bias_percent.store(percent, Ordering::Relaxed);
        tracing::info!(percent, "Workout bias changed");
        {
            let mut state = self.state.write().await;
            match &mut *state {
                RunnerState::Running { bias_percent, .. }
                | RunnerState::Paused { bias_percent, .. } => *bias_percent = percent,
                _ => {}
            }
        }
        let planned_running = self.controls.control.load(Ordering::Relaxed) == RUNNING
            && self.controls.planned_active.load(Ordering::Relaxed)
            && self.controls.override_target().is_none();
        if planned_running {
            self.reapply_plan(app, rider_max, devices).await?;
        } else {
            emit_state(app, &self.state).await;
        }
        Ok(percent)
    }

    fn adjustment_mode(&self) -> Result<Adjustment, String> {
        let control = self.controls.control.load(Ordering::Relaxed);
        if manual_adjustment_allowed(control, self.controls.manual_active.load(Ordering::Relaxed)) {
            Ok(Adjustment::Manual)
        } else if manual_adjustment_allowed(
            control,
            self.controls.planned_active.load(Ordering::Relaxed),
        ) {
            Ok(Adjustment::Override)
        } else {
            Err("Target power can only be changed while riding".into())
        }
    }

    async fn apply_target(
        &self,
        app: &AppHandle,
        requested: u16,
        rider_max: u16,
        devices: &DeviceHub,
        mode: Adjustment,
    ) -> Result<u16, String> {
        let clamped = devices.set_target_power(requested, rider_max).await?;
        self.controls
            .current_target
            .store(clamped, Ordering::Relaxed);
        match mode {
            Adjustment::Manual => self
                .controls
                .manual_target
                .store(clamped, Ordering::Relaxed),
            Adjustment::Override => {
                self.controls
                    .override_target
                    .store(clamped, Ordering::Relaxed);
                tracing::info!(watts = clamped, "Interval target overridden");
            }
        }
        {
            let mut state = self.state.write().await;
            let RunnerState::Running {
                target_power_watts,
                manual_erg,
                override_active,
                ..
            } = &mut *state
            else {
                return Err("Target power can only be changed while riding".into());
            };
            match mode {
                Adjustment::Manual if !*manual_erg => {
                    return Err("Manual ERG control is not active".into());
                }
                Adjustment::Manual => {}
                Adjustment::Override => *override_active = true,
            }
            *target_power_watts = Some(clamped);
        }
        emit_state(app, &self.state).await;
        Ok(clamped)
    }

    /// Send the biased planned target for the current interval right away
    /// rather than waiting for the next one-second tick.
    async fn reapply_plan(
        &self,
        app: &AppHandle,
        rider_max: u16,
        devices: &DeviceHub,
    ) -> Result<Option<u16>, String> {
        let planned = match &*self.state.read().await {
            RunnerState::Running {
                planned_target_watts,
                ..
            } => *planned_target_watts,
            _ => None,
        };
        let mut applied = None;
        if let Some(watts) = self.controls.planned_effective(planned) {
            let clamped = devices.set_target_power(watts, rider_max).await?;
            self.controls
                .current_target
                .store(clamped, Ordering::Relaxed);
            applied = Some(clamped);
        }
        {
            let mut state = self.state.write().await;
            if let RunnerState::Running {
                target_power_watts,
                override_active,
                ..
            } = &mut *state
            {
                *override_active = self.controls.override_target().is_some();
                if applied.is_some() {
                    *target_power_watts = applied;
                }
            }
        }
        emit_state(app, &self.state).await;
        Ok(applied)
    }

    pub async fn pause_or_resume(&self, devices: &DeviceHub) -> Result<(), String> {
        let current = self.controls.control.load(Ordering::Relaxed);
        if current == RUNNING {
            tracing::info!("Workout paused");
            devices.pause().await?;
            self.controls.control.store(PAUSED, Ordering::Relaxed);
        } else if current == PAUSED {
            tracing::info!("Workout resumed");
            devices.begin_control().await?;
            self.controls.control.store(RUNNING, Ordering::Relaxed);
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
        if self.controls.control.load(Ordering::Relaxed) <= PAUSED {
            tracing::info!("Interval skip requested");
            self.controls.control.store(SKIP, Ordering::Relaxed);
            Ok(())
        } else {
            Err("No active interval to skip".into())
        }
    }

    pub async fn stop(&self, devices: &DeviceHub) -> Result<(), String> {
        tracing::info!("Workout stop requested");
        self.controls.control.store(STOPPED, Ordering::Relaxed);
        self.controls.manual_active.store(false, Ordering::Relaxed);
        self.controls.planned_active.store(false, Ordering::Relaxed);
        self.controls.clear_override();
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
    controls: Controls,
) -> Result<(), String> {
    let Controls {
        control,
        manual_target,
        manual_active,
        planned_active,
        current_target,
        ..
    } = controls.clone();
    let mut telemetry_rx = devices.subscribe();
    let mut elapsed = 0_u32;
    let mut power_total = 0_u64;
    let mut cadence_total = 0.0_f64;
    let mut cadence_samples = 0_u32;
    let mut samples = 0_u32;
    let mut max_power = 0_u16;

    for (interval_index, interval) in intervals.iter().enumerate() {
        // Overrides belong to one interval only; the bias carries across.
        controls.clear_override();
        if interval.free_ride {
            let clamped = devices
                .set_target_power(MANUAL_START_WATTS, rider_max)
                .await?;
            manual_target.store(clamped, Ordering::Relaxed);
            current_target.store(clamped, Ordering::Relaxed);
            manual_active.store(true, Ordering::Relaxed);
            planned_active.store(false, Ordering::Relaxed);
        } else {
            manual_active.store(false, Ordering::Relaxed);
            planned_active.store(interval.start_watts.is_some(), Ordering::Relaxed);
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
                    planned_active.store(false, Ordering::Relaxed);
                    control.store(RUNNING, Ordering::Relaxed);
                    break;
                }
                PAUSED => {
                    let planned = target_at(interval, interval_elapsed);
                    *state.write().await = RunnerState::Paused {
                        session_id: summary.id,
                        workout_name: ride_name.to_string(),
                        elapsed_seconds: elapsed,
                        total_seconds,
                        interval_index,
                        interval_elapsed_seconds: interval_elapsed,
                        target_power_watts: if interval.free_ride {
                            Some(manual_target.load(Ordering::Relaxed))
                        } else {
                            controls.planned_effective(planned)
                        },
                        planned_target_watts: if interval.free_ride { None } else { planned },
                        manual_erg: interval.free_ride,
                        override_active: !interval.free_ride
                            && controls.override_target().is_some(),
                        bias_percent: controls.bias(),
                    };
                    emit_state(app, &state).await;
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    continue;
                }
                _ => {}
            }

            let planned = if interval.free_ride {
                None
            } else {
                target_at(interval, interval_elapsed)
            };
            let mut target = if interval.free_ride {
                Some(manual_target.load(Ordering::Relaxed))
            } else {
                controls.planned_effective(planned)
            };
            if let Some(watts) = target {
                let clamped = devices.set_target_power(watts, rider_max).await?;
                current_target.store(clamped, Ordering::Relaxed);
                if interval.free_ride {
                    manual_target.store(clamped, Ordering::Relaxed);
                }
                target = Some(clamped);
            }
            *state.write().await = RunnerState::Running {
                session_id: summary.id,
                workout_name: ride_name.to_string(),
                elapsed_seconds: elapsed,
                total_seconds,
                interval_index,
                interval_elapsed_seconds: interval_elapsed,
                target_power_watts: target,
                planned_target_watts: planned,
                manual_erg: interval.free_ride,
                override_active: !interval.free_ride && controls.override_target().is_some(),
                bias_percent: controls.bias(),
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
        planned_active.store(false, Ordering::Relaxed);
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

/// Scale a planned target by the ride's bias, rounding to the nearest watt
/// and never dropping to zero (ERG mode treats 0 W as "no target").
fn biased_target(planned: u16, bias_percent: u16) -> u16 {
    let bias = bias_percent.clamp(MIN_BIAS_PERCENT, MAX_BIAS_PERCENT);
    let scaled = (u32::from(planned) * u32::from(bias) + 50) / 100;
    scaled.clamp(1, u32::from(u16::MAX)) as u16
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Adjustment {
    /// Free-ride interval: move the manual ERG target.
    Manual,
    /// Planned interval: override its target until the next interval.
    Override,
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
    if let Some(detail) = storage.session(summary.id)? {
        let estimate = estimate_distance(&detail.samples, summary.distance_weight_kg);
        summary.estimated_distance_meters = estimate.total_meters;
        summary.distance_source = estimate.source;
    }
    summary.completed = completed;
    tracing::info!(
        session_id = %summary.id,
        completed,
        elapsed_seconds = elapsed,
        samples,
        average_power_watts = summary.average_power_watts,
        max_power_watts = summary.max_power_watts,
        estimated_distance_meters = summary.estimated_distance_meters,
        distance_source = ?summary.distance_source,
        "Workout finished"
    );
    storage.finish_session(&summary).map_err(|error| {
        tracing::error!(session_id = %summary.id, error = %error, "Could not persist session summary");
        error
    })?;
    let ride_files_dir = app.state::<AppState>().ride_files_dir.clone();
    match storage.session(summary.id) {
        Ok(Some(detail)) => match ensure_ride_file(&ride_files_dir, &detail) {
            Ok(path) => {
                tracing::info!(session_id = %summary.id, file = %path.display(), "Ride FIT file saved")
            }
            Err(error) => {
                tracing::warn!(session_id = %summary.id, %error, "Could not save ride FIT file; startup will retry")
            }
        },
        Ok(None) => {
            tracing::warn!(session_id = %summary.id, "Ride disappeared before FIT generation")
        }
        Err(error) => {
            tracing::warn!(session_id = %summary.id, %error, "Could not reload ride for FIT generation")
        }
    }
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
            runner.controls.manual_target.load(Ordering::Relaxed),
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
            planned_target_watts: None,
            manual_erg: true,
            override_active: false,
            bias_percent: DEFAULT_BIAS_PERCENT,
        };
        let json = serde_json::to_value(state).unwrap();

        assert_eq!(json["workoutName"], "Free Ride");
        assert_eq!(json["targetPowerWatts"], 100);
        assert_eq!(json["manualErg"], true);
        assert_eq!(json["biasPercent"], 100);
        assert_eq!(json["overrideActive"], false);
        assert!(json["plannedTargetWatts"].is_null());
        assert!(json.get("workout_name").is_none());
    }

    #[test]
    fn paused_state_keeps_interval_elapsed_time() {
        let state = RunnerState::Paused {
            session_id: Uuid::nil(),
            workout_name: "Intervals".into(),
            elapsed_seconds: 75,
            total_seconds: Some(300),
            interval_index: 1,
            interval_elapsed_seconds: 15,
            target_power_watts: Some(200),
            planned_target_watts: Some(200),
            manual_erg: false,
            override_active: false,
            bias_percent: DEFAULT_BIAS_PERCENT,
        };
        let json = serde_json::to_value(state).unwrap();

        assert_eq!(json["intervalIndex"], 1);
        assert_eq!(json["intervalElapsedSeconds"], 15);
    }

    #[test]
    fn bias_scales_planned_targets_and_rounds_to_the_nearest_watt() {
        assert_eq!(biased_target(200, 100), 200);
        assert_eq!(biased_target(200, 105), 210);
        assert_eq!(biased_target(201, 95), 191); // 190.95 rounds up
        assert_eq!(biased_target(1, 50), 1); // never zero
        assert_eq!(biased_target(200, 500), 300); // clamped to MAX_BIAS_PERCENT
        assert_eq!(biased_target(200, 10), 100); // clamped to MIN_BIAS_PERCENT
    }

    #[test]
    fn override_wins_over_bias_until_cleared() {
        let controls = Controls::default();
        controls.bias_percent.store(110, Ordering::Relaxed);
        assert_eq!(controls.planned_effective(Some(200)), Some(220));
        assert_eq!(controls.planned_effective(None), None);

        controls.override_target.store(250, Ordering::Relaxed);
        assert_eq!(controls.planned_effective(Some(200)), Some(250));
        assert_eq!(controls.planned_effective(None), Some(250));

        controls.clear_override();
        assert_eq!(controls.override_target(), None);
        assert_eq!(controls.planned_effective(Some(200)), Some(220));
    }

    #[test]
    fn adjustment_mode_follows_the_running_interval_kind() {
        let runner = WorkoutRunner::default();
        assert!(runner.adjustment_mode().is_err()); // stopped

        runner.controls.control.store(RUNNING, Ordering::Relaxed);
        assert!(runner.adjustment_mode().is_err()); // no interval with a target yet

        runner
            .controls
            .planned_active
            .store(true, Ordering::Relaxed);
        assert_eq!(runner.adjustment_mode().unwrap(), Adjustment::Override);

        runner
            .controls
            .planned_active
            .store(false, Ordering::Relaxed);
        runner.controls.manual_active.store(true, Ordering::Relaxed);
        assert_eq!(runner.adjustment_mode().unwrap(), Adjustment::Manual);

        runner.controls.control.store(PAUSED, Ordering::Relaxed);
        assert!(runner.adjustment_mode().is_err());
    }
}
