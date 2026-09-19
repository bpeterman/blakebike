//! The workout runner.
//!
//! Three cooperating pieces per ride:
//! - the **timeline** task keeps the clock on wall time, decides which
//!   interval and target are current, records telemetry totals and is the only
//!   writer of [`RunnerState`];
//! - the **target writer** task is the only thing that talks to the trainer
//!   during a ride: it follows the timeline's intent (running / paused /
//!   stopped plus the current target), retries with backoff and reports a
//!   [`ControlStatus`] instead of ever failing the ride;
//! - the **recorder** thread batches telemetry into SQLite so a slow or
//!   failing write never stalls the ride.
//!
//! Trainer trouble degrades a ride; it never ends it. The session is finalized
//! on every exit path.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU16, Ordering},
        mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel},
    },
    time::Duration,
};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::{
    sync::{Notify, RwLock, broadcast, watch},
    task::JoinHandle,
    time::{Instant, MissedTickBehavior},
};
use uuid::Uuid;

use crate::{
    devices::{ControlError, DeviceHub, DeviceState},
    distance::estimate_distance,
    domain::{Interval, SessionSummary, Telemetry, Workout},
    fit::ensure_ride_file,
    ftms::ResponseCode,
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
/// Re-send the current target this often when nothing changed, so a trainer
/// that quietly dropped it gets it again.
const KEEPALIVE: Duration = Duration::from_secs(5);
/// After a failed command, try again this soon (each attempt is itself bounded
/// by the control timeouts).
const RETRY_AFTER: Duration = Duration::from_secs(2);
/// Consecutive command failures before a nominally connected trainer counts
/// as lost.
const FAILURES_BEFORE_LOST: u8 = 3;
/// How long the writer waits for the trainer to acknowledge Stop at the end.
const STOP_GRACE: Duration = Duration::from_secs(3);
/// How long the ride waits for the recorder to write the last samples.
const FLUSH_GRACE: Duration = Duration::from_secs(5);
/// Samples the recorder keeps while the database stays unwritable (about ten
/// minutes at 5 Hz); older ones are dropped first.
const MAX_PENDING_SAMPLES: usize = 3_000;

/// How the trainer is keeping up with the ride, shown on the ride screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ControlStatus {
    /// Commands are acknowledged.
    #[default]
    Ok,
    /// A recent command failed or was refused; the writer keeps retrying.
    Degraded,
    /// No usable link; the target is held until the trainer is back.
    Lost,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RunnerState {
    Idle,
    Running {
        session_id: Uuid,
        workout_name: String,
        elapsed_seconds: u32,
        total_seconds: Option<u32>,
        interval_index: usize,
        interval_elapsed_seconds: u32,
        /// Target the ride wants on the trainer right now (after bias and any
        /// override). Whether the trainer has it is `control`.
        target_power_watts: Option<u16>,
        /// Target the workout plan asks for right now, before bias/override.
        planned_target_watts: Option<u16>,
        /// True while a free-ride interval is running (manual ERG).
        manual_erg: bool,
        /// True while the rider has overridden this interval's target.
        override_active: bool,
        bias_percent: u16,
        control: ControlStatus,
        #[serde(default)]
        recording_warning: Option<String>,
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
        control: ControlStatus,
        #[serde(default)]
        recording_warning: Option<String>,
    },
    Finished {
        session_id: Uuid,
        completed: bool,
        #[serde(default)]
        save_warning: Option<String>,
    },
    Error {
        message: String,
        /// The ride that was being recorded; it is finalized and in History.
        session_id: Option<Uuid>,
    },
}

/// Running totals kept by the timeline and folded into the session summary.
#[derive(Debug, Default, Clone, Copy)]
struct RideStats {
    elapsed: u32,
    power_total: u64,
    max_power: u16,
    cadence_total: f64,
    cadence_samples: u32,
    samples: u32,
}

impl RideStats {
    fn record(&mut self, sample: &Telemetry) {
        self.samples += 1;
        self.power_total += u64::from(sample.power_watts);
        self.max_power = self.max_power.max(sample.power_watts);
        if let Some(cadence) = sample.cadence_rpm {
            self.cadence_total += f64::from(cadence);
            self.cadence_samples += 1;
        }
    }

    fn apply(&self, summary: &mut SessionSummary) {
        summary.elapsed_seconds = self.elapsed;
        summary.average_power_watts = if self.samples == 0 {
            0
        } else {
            (self.power_total / u64::from(self.samples)) as u16
        };
        summary.max_power_watts = self.max_power;
        summary.average_cadence_rpm = (self.cadence_samples > 0)
            .then_some((self.cadence_total / f64::from(self.cadence_samples)) as f32);
    }
}

/// What the ride wants the trainer to be doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Running,
    Paused,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TrainerIntent {
    phase: Phase,
    /// Desired ERG target, already clamped to the trainer and rider limits.
    target: Option<u16>,
}

/// Lock-free knobs shared between the command handlers and the ride tasks.
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
    /// The target the ride wants applied right now; ± steps start from here.
    current_target: Arc<AtomicU16>,
    /// Overall bias applied to every planned target for this ride.
    bias_percent: Arc<AtomicU16>,
    /// What the trainer should be doing; the writer task follows it.
    intent: Arc<watch::Sender<TrainerIntent>>,
    /// Wakes the timeline so stop, skip, pause and resume act at once.
    wake: Arc<Notify>,
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
            intent: Arc::new(watch::Sender::new(TrainerIntent {
                phase: Phase::Idle,
                target: None,
            })),
            wake: Arc::new(Notify::new()),
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

    fn set_phase(&self, phase: Phase) {
        self.intent.send_if_modified(|intent| {
            if intent.phase == phase {
                false
            } else {
                intent.phase = phase;
                true
            }
        });
    }

    fn set_target(&self, target: Option<u16>) {
        self.intent.send_if_modified(|intent| {
            if intent.target == target {
                false
            } else {
                intent.target = target;
                true
            }
        });
    }
}

pub struct WorkoutRunner {
    state: Arc<RwLock<RunnerState>>,
    controls: Controls,
    standalone: Arc<AtomicBool>,
    manual_adjustment_lock: tokio::sync::Mutex<()>,
    worker: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    /// Where finalized rides are written as FIT files.
    ride_files_dir: PathBuf,
    /// Keeps the machine and display awake while a ride is active.
    awake: Arc<std::sync::Mutex<Option<keepawake::KeepAwake>>>,
}

impl Default for WorkoutRunner {
    fn default() -> Self {
        Self::new(std::env::temp_dir().join("blakebike-ride-files"))
    }
}

impl WorkoutRunner {
    pub fn new(ride_files_dir: PathBuf) -> Self {
        Self {
            state: Arc::new(RwLock::new(RunnerState::Idle)),
            controls: Controls::default(),
            standalone: Arc::new(AtomicBool::new(false)),
            manual_adjustment_lock: tokio::sync::Mutex::new(()),
            worker: tokio::sync::Mutex::new(None),
            ride_files_dir,
            awake: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    pub async fn state(&self) -> RunnerState {
        self.state.read().await.clone()
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn start(
        &self,
        app: Option<AppHandle>,
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
        app: Option<AppHandle>,
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
        app: Option<AppHandle>,
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
        // Let the previous ride's tasks wind down first: its final Stop must
        // not land after this ride's Start.
        if let Some(previous) = self.worker.lock().await.take()
            && tokio::time::timeout(Duration::from_secs(5), previous)
                .await
                .is_err()
        {
            tracing::warn!("Previous ride did not wind down in time; starting anyway");
        }
        // Control is acquired synchronously so a missing trainer is reported
        // to the rider right away; from here on the writer task owns the link.
        devices.begin_control().await.map_err(|error| match error {
            ControlError::NotConnected => "Connect a trainer before starting a workout".to_string(),
            other => other.to_string(),
        })?;
        let session = storage.start_session(workout_id, &ride_name, distance_weight_kg)?;
        let session_id = session.id;
        devices.set_ride_active(true);
        // A ride is not a good time for the Mac to go to sleep. Headless runs
        // (tests) leave power management alone.
        if app.is_some() {
            let assertion = keepawake::Builder::default()
                .display(true)
                .idle(true)
                .reason("Ride in progress")
                .app_name("blake.bike")
                .app_reverse_domain("com.bpeterman.blakebike")
                .create();
            match assertion {
                Ok(assertion) => {
                    tracing::info!("Sleep prevented for the duration of the ride");
                    *self
                        .awake
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(assertion);
                }
                Err(error) => {
                    tracing::warn!(error = %error, "Could not prevent sleep; the ride runs anyway")
                }
            }
        }
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
        self.controls.current_target.store(0, Ordering::Relaxed);
        self.controls.intent.send_replace(TrainerIntent {
            phase: Phase::Running,
            target: None,
        });
        self.standalone.store(standalone, Ordering::Relaxed);

        let (status_tx, status_rx) = watch::channel(ControlStatus::Ok);
        let writer = tokio::spawn(target_writer(
            devices.clone(),
            self.controls.intent.subscribe(),
            status_tx,
            devices.subscribe_trainer_state(),
            rider_max,
        ));
        let mut ride = Ride {
            app: app.clone(),
            name: ride_name,
            intervals,
            total_seconds,
            standalone,
            rider_max,
            session_id,
            devices,
            state: self.state.clone(),
            controls: self.controls.clone(),
            status_rx,
            recorder: Recorder::start(storage.clone(), session_id),
        };
        let ride_files_dir = self.ride_files_dir.clone();
        let awake = self.awake.clone();
        let supervisor_app = app;
        let supervisor_state = self.state.clone();
        let supervisor_storage = storage.clone();
        let ride_task = tokio::spawn(async move {
            let mut stats = RideStats::default();
            let completed = run_timeline(&mut ride, &mut stats).await;
            // Tell the trainer first; closing the session takes a moment.
            ride.controls.set_phase(Phase::Stopped);
            ride.controls.manual_active.store(false, Ordering::Relaxed);
            ride.controls.planned_active.store(false, Ordering::Relaxed);
            let flush_result = ride.recorder.flush(FLUSH_GRACE).await;
            let flushed = flush_result.is_ok();
            let mut save_warning = flush_result.err().or_else(|| ride.recorder.warning());
            let dropped = ride.recorder.dropped();
            if dropped > 0 {
                tracing::warn!(session_id = %session_id, dropped, "Telemetry samples were dropped during the ride");
            }
            // Whatever happened, the ride is closed out and lands in History.
            match finish(
                &storage,
                &ride_files_dir,
                session,
                &stats,
                completed,
                save_warning.clone(),
                flushed,
            ) {
                Ok(warning) => save_warning = warning,
                Err(error) => {
                    tracing::error!(%session_id, %error, "Ride finalization failed");
                    save_warning = Some(match save_warning {
                        Some(warning) => format!("{warning} {error}"),
                        None => error,
                    });
                    if let Err(error) =
                        storage.save_recording_warning(session_id, save_warning.as_deref())
                    {
                        tracing::error!(%session_id, %error, "Could not persist recording warning");
                    }
                }
            }
            if tokio::time::timeout(STOP_GRACE + Duration::from_secs(1), writer)
                .await
                .is_err()
            {
                tracing::warn!("Trainer writer did not wind down in time");
            }
            ride.devices.set_ride_active(false);
            if awake
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
                .is_some()
            {
                tracing::info!("Sleep allowed again");
            }
            *ride.state.write().await = RunnerState::Finished {
                session_id,
                completed,
                save_warning,
            };
            emit_state(ride.app.as_ref(), &ride.state).await;
        });
        // If the ride task itself dies (a panic), the session is still closed
        // from whatever samples were recorded and the UI is told.
        let worker = tokio::spawn(async move {
            if let Err(error) = ride_task.await
                && error.is_panic()
            {
                tracing::error!(session_id = %session_id, "Ride task panicked; finalizing from recorded samples");
                if let Err(error) = supervisor_storage.finalize_orphaned_session(session_id) {
                    tracing::error!(session_id = %session_id, error = %error, "Could not finalize ride after panic");
                }
                *supervisor_state.write().await = RunnerState::Error {
                    message:
                        "The ride stopped unexpectedly; it was saved from its recorded samples"
                            .into(),
                    session_id: Some(session_id),
                };
                emit_state(supervisor_app.as_ref(), &supervisor_state).await;
            }
        });
        *self.worker.lock().await = Some(worker);
        Ok(session_id)
    }

    /// Nudge the target by ±5 W. In a free-ride interval this moves the manual
    /// ERG target; in a planned interval it overrides that interval's target
    /// until the next one starts. Returns at once; the writer task applies it.
    pub async fn adjust_manual_power(
        &self,
        app: Option<&AppHandle>,
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
        app: Option<&AppHandle>,
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
        app: Option<&AppHandle>,
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
        Ok(self.reapply_plan(app, rider_max, devices).await)
    }

    /// Scale every planned target for the rest of this ride. Applies at once
    /// unless the current interval is overridden or free ride.
    pub async fn set_bias_percent(
        &self,
        app: Option<&AppHandle>,
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
            self.reapply_plan(app, rider_max, devices).await;
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
        app: Option<&AppHandle>,
        requested: u16,
        rider_max: u16,
        devices: &DeviceHub,
        mode: Adjustment,
    ) -> Result<u16, String> {
        let clamped = devices.clamp_target(requested, rider_max);
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
        self.controls.set_target(Some(clamped));
        emit_state(app, &self.state).await;
        Ok(clamped)
    }

    /// Publish the biased planned target for the current interval right away
    /// rather than waiting for the next one-second tick.
    async fn reapply_plan(
        &self,
        app: Option<&AppHandle>,
        rider_max: u16,
        devices: &DeviceHub,
    ) -> Option<u16> {
        let planned = match &*self.state.read().await {
            RunnerState::Running {
                planned_target_watts,
                ..
            } => *planned_target_watts,
            _ => None,
        };
        let applied = self
            .controls
            .planned_effective(planned)
            .map(|watts| devices.clamp_target(watts, rider_max));
        if let Some(clamped) = applied {
            self.controls
                .current_target
                .store(clamped, Ordering::Relaxed);
            self.controls.set_target(Some(clamped));
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
        applied
    }

    /// Pause or resume the ride clock. The trainer is told by the writer
    /// task; a trainer that cannot be reached does not stop the rider from
    /// pausing.
    pub async fn pause_or_resume(&self) -> Result<(), String> {
        let current = self.controls.control.load(Ordering::Relaxed);
        if current == RUNNING {
            tracing::info!("Workout paused");
            self.controls.control.store(PAUSED, Ordering::Relaxed);
        } else if current == PAUSED {
            tracing::info!("Workout resumed");
            self.controls.control.store(RUNNING, Ordering::Relaxed);
        } else {
            tracing::warn!(control = current, "pause_or_resume with no active workout");
            return Err("No workout can be paused or resumed".into());
        }
        self.controls.wake.notify_one();
        Ok(())
    }

    pub fn skip(&self) -> Result<(), String> {
        if self.standalone.load(Ordering::Relaxed) {
            return Err("Free rides do not have intervals to skip".into());
        }
        if self.controls.control.load(Ordering::Relaxed) <= PAUSED {
            tracing::info!("Interval skip requested");
            self.controls.control.store(SKIP, Ordering::Relaxed);
            self.controls.wake.notify_one();
            Ok(())
        } else {
            Err("No active interval to skip".into())
        }
    }

    /// Stop the ride (if any) and wait, bounded, until it is finalized: the
    /// session summary and FIT file written and the trainer told to stop.
    /// Used on app exit so the process does not die mid-finalization.
    pub async fn stop_and_wait(&self, grace: Duration) {
        let active = self.controls.control.load(Ordering::Relaxed) <= PAUSED;
        if active {
            let _ = self.stop().await;
        }
        let worker = self.worker.lock().await.take();
        if let Some(worker) = worker
            && tokio::time::timeout(grace, worker).await.is_err()
        {
            tracing::warn!("Ride did not finalize within the shutdown grace period");
        }
    }

    /// End the ride. The timeline finalizes the session and the writer task
    /// tells the trainer to stop; neither can fail from here.
    pub async fn stop(&self) -> Result<(), String> {
        tracing::info!("Workout stop requested");
        self.controls.control.store(STOPPED, Ordering::Relaxed);
        self.controls.manual_active.store(false, Ordering::Relaxed);
        self.controls.planned_active.store(false, Ordering::Relaxed);
        self.controls.clear_override();
        self.controls.wake.notify_one();
        Ok(())
    }
}

/// Riding time on a monotonic clock: wall time minus pauses, plus whatever
/// skipped intervals jumped over.
#[derive(Debug, Clone, Copy)]
struct RideClock {
    started: Instant,
    paused_total: Duration,
    pause_started: Option<Instant>,
    skipped: Duration,
}

impl RideClock {
    fn new(now: Instant) -> Self {
        Self {
            started: now,
            paused_total: Duration::ZERO,
            pause_started: None,
            skipped: Duration::ZERO,
        }
    }

    fn is_paused(&self) -> bool {
        self.pause_started.is_some()
    }

    fn pause(&mut self, now: Instant) {
        if self.pause_started.is_none() {
            self.pause_started = Some(now);
        }
    }

    fn resume(&mut self, now: Instant) {
        if let Some(at) = self.pause_started.take() {
            self.paused_total += now.saturating_duration_since(at);
        }
    }

    fn skip(&mut self, remainder: Duration) {
        self.skipped += remainder;
    }

    fn elapsed(&self, now: Instant) -> Duration {
        let paused = self.paused_total
            + self
                .pause_started
                .map_or(Duration::ZERO, |at| now.saturating_duration_since(at));
        now.saturating_duration_since(self.started)
            .saturating_sub(paused)
            + self.skipped
    }

    fn elapsed_seconds(&self, now: Instant) -> u64 {
        self.elapsed(now).as_secs()
    }
}

/// Everything the timeline needs for one ride.
struct Ride {
    app: Option<AppHandle>,
    name: String,
    intervals: Vec<Interval>,
    total_seconds: Option<u32>,
    standalone: bool,
    rider_max: u16,
    session_id: Uuid,
    devices: Arc<DeviceHub>,
    state: Arc<RwLock<RunnerState>>,
    controls: Controls,
    status_rx: watch::Receiver<ControlStatus>,
    recorder: Recorder,
}

/// Drive the ride to its end on wall time. Never fails: trainer and storage
/// trouble are handled by the writer and the recorder. Returns whether the
/// ride counts as completed (ran to the end, or a free ride the rider ended).
async fn run_timeline(ride: &mut Ride, stats: &mut RideStats) -> bool {
    let controls = ride.controls.clone();
    let mut telemetry_rx = ride.devices.subscribe();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut clock = RideClock::new(Instant::now());
    let mut interval_index = 0_usize;
    let mut interval_start: u64 = 0;
    let mut current_target: Option<u16> = None;
    let mut lag_logged = false;
    let mut telemetry_open = true;
    let mut writer_alive = true;
    enter_interval(ride, 0, 0);

    loop {
        tokio::select! {
            biased;
            _ = controls.wake.notified() => {}
            _ = tick.tick() => {}
            received = telemetry_rx.recv(), if telemetry_open => {
                match received {
                    Ok(mut sample) => {
                        sample.target_power_watts = current_target;
                        stats.record(&sample);
                        ride.recorder.push(sample);
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        if !lag_logged {
                            tracing::warn!(skipped, "Telemetry consumer lagged; some samples were not recorded");
                            lag_logged = true;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => telemetry_open = false,
                }
                continue;
            }
            changed = ride.status_rx.changed(), if writer_alive => {
                if changed.is_err() {
                    writer_alive = false;
                }
            }
        }

        let now = Instant::now();
        match controls.control.load(Ordering::Relaxed) {
            STOPPED => {
                stats.elapsed = clock.elapsed_seconds(now) as u32;
                // A stopped free ride still counts as completed: it has no end.
                return ride.standalone;
            }
            PAUSED => {
                if !clock.is_paused() {
                    clock.pause(now);
                    controls.set_phase(Phase::Paused);
                    tracing::info!("Ride clock paused");
                }
                let elapsed = clock.elapsed_seconds(now);
                stats.elapsed = elapsed as u32;
                publish(
                    ride,
                    true,
                    elapsed,
                    interval_index,
                    interval_start,
                    current_target,
                )
                .await;
                continue;
            }
            SKIP => {
                let elapsed = clock.elapsed_seconds(now);
                let end =
                    interval_start + u64::from(ride.intervals[interval_index].duration_seconds);
                clock.skip(Duration::from_secs(end.saturating_sub(elapsed)));
                controls.control.store(RUNNING, Ordering::Relaxed);
                tracing::info!(interval_index, "Interval skipped");
            }
            _ => {}
        }
        if clock.is_paused() {
            clock.resume(now);
            controls.set_phase(Phase::Running);
            tracing::info!("Ride clock resumed");
        }

        let elapsed = clock.elapsed_seconds(now);
        stats.elapsed = elapsed as u32;
        // Advance through every interval that has ended; a skip or a long
        // stall can cross several short ones at once.
        loop {
            let duration = u64::from(ride.intervals[interval_index].duration_seconds);
            if elapsed < interval_start + duration {
                break;
            }
            interval_start += duration;
            interval_index += 1;
            if interval_index == ride.intervals.len() {
                return true;
            }
            enter_interval(ride, interval_index, elapsed);
        }

        let interval = &ride.intervals[interval_index];
        let interval_elapsed = (elapsed - interval_start) as u32;
        let planned = if interval.free_ride {
            None
        } else {
            target_at(interval, interval_elapsed)
        };
        let mut target = if interval.free_ride {
            Some(controls.manual_target.load(Ordering::Relaxed))
        } else {
            controls.planned_effective(planned)
        };
        if let Some(watts) = target {
            let clamped = ride.devices.clamp_target(watts, ride.rider_max);
            controls.current_target.store(clamped, Ordering::Relaxed);
            if interval.free_ride {
                controls.manual_target.store(clamped, Ordering::Relaxed);
            }
            target = Some(clamped);
        }
        controls.set_target(target);
        current_target = target;
        publish(ride, false, elapsed, interval_index, interval_start, target).await;
    }
}

fn enter_interval(ride: &Ride, index: usize, elapsed: u64) {
    let interval = &ride.intervals[index];
    // Overrides belong to one interval only; the bias carries across.
    ride.controls.clear_override();
    if interval.free_ride {
        ride.controls
            .manual_target
            .store(MANUAL_START_WATTS, Ordering::Relaxed);
        ride.controls.manual_active.store(true, Ordering::Relaxed);
        ride.controls.planned_active.store(false, Ordering::Relaxed);
    } else {
        ride.controls.manual_active.store(false, Ordering::Relaxed);
        ride.controls
            .planned_active
            .store(interval.start_watts.is_some(), Ordering::Relaxed);
    }
    tracing::info!(
        session_id = %ride.session_id,
        interval_index = index,
        duration_seconds = interval.duration_seconds,
        elapsed,
        "Interval started"
    );
}

/// Write the current ride position into `RunnerState` and tell the UI.
async fn publish(
    ride: &Ride,
    paused: bool,
    elapsed: u64,
    interval_index: usize,
    interval_start: u64,
    target: Option<u16>,
) {
    let interval = &ride.intervals[interval_index];
    let interval_elapsed = elapsed.saturating_sub(interval_start) as u32;
    let planned = if interval.free_ride {
        None
    } else {
        target_at(interval, interval_elapsed)
    };
    let control = *ride.status_rx.borrow();
    let next = if paused {
        RunnerState::Paused {
            session_id: ride.session_id,
            workout_name: ride.name.clone(),
            elapsed_seconds: elapsed as u32,
            total_seconds: ride.total_seconds,
            interval_index,
            interval_elapsed_seconds: interval_elapsed,
            target_power_watts: target,
            planned_target_watts: planned,
            manual_erg: interval.free_ride,
            override_active: !interval.free_ride && ride.controls.override_target().is_some(),
            bias_percent: ride.controls.bias(),
            control,
            recording_warning: ride.recorder.warning(),
        }
    } else {
        RunnerState::Running {
            session_id: ride.session_id,
            workout_name: ride.name.clone(),
            elapsed_seconds: elapsed as u32,
            total_seconds: ride.total_seconds,
            interval_index,
            interval_elapsed_seconds: interval_elapsed,
            target_power_watts: target,
            planned_target_watts: planned,
            manual_erg: interval.free_ride,
            override_active: !interval.free_ride && ride.controls.override_target().is_some(),
            bias_percent: ride.controls.bias(),
            control,
            recording_warning: ride.recorder.warning(),
        }
    };
    *ride.state.write().await = next;
    emit_state(ride.app.as_ref(), &ride.state).await;
}

/// The only task that talks to the trainer during a ride. Follows the intent
/// published by the timeline and the command handlers, re-sends the target as
/// a keepalive, retries failures with backoff and reports a `ControlStatus`.
/// It never returns an error to anyone: a trainer that is gone just means the
/// status is `Lost` until it is back.
async fn target_writer(
    devices: Arc<DeviceHub>,
    mut intent_rx: watch::Receiver<TrainerIntent>,
    status_tx: watch::Sender<ControlStatus>,
    mut trainer_state: watch::Receiver<DeviceState>,
    rider_max: u16,
) {
    // Control was acquired synchronously when the ride started.
    let mut started = true;
    let mut pause_acknowledged = false;
    let mut failures: u8 = 0;
    let mut last_sent: Option<u16> = None;
    let mut last_attempt = Instant::now();
    let mut status = ControlStatus::Ok;
    loop {
        let intent = *intent_rx.borrow_and_update();
        if intent.phase == Phase::Stopped {
            if tokio::time::timeout(STOP_GRACE, devices.stop())
                .await
                .is_err()
            {
                tracing::warn!("Trainer did not acknowledge stop in time");
            }
            return;
        }
        let connected = trainer_state.borrow_and_update().is_connected();
        if !connected {
            if status != ControlStatus::Lost {
                tracing::warn!("Trainer link is down; holding the target until it is back");
            }
            status = ControlStatus::Lost;
            started = false;
            pause_acknowledged = false;
            last_sent = None;
            failures = 0;
        } else {
            let mut attempted = false;
            let mut result: Result<(), ControlError> = Ok(());
            match intent.phase {
                Phase::Paused => {
                    if !pause_acknowledged {
                        attempted = true;
                        // Until the acknowledgement arrives, the trainer may
                        // still be applying the previous resistance.
                        status_tx.send_replace(ControlStatus::Degraded);
                        result = devices.pause().await;
                        if result.is_ok() {
                            pause_acknowledged = true;
                            started = false;
                        }
                    }
                }
                Phase::Running => {
                    pause_acknowledged = false;
                    if !started {
                        attempted = true;
                        result = devices.begin_control().await;
                        if result.is_ok() {
                            started = true;
                            last_sent = None;
                        }
                    }
                    if result.is_ok()
                        && started
                        && let Some(target) = intent.target
                        && (last_sent != Some(target) || last_attempt.elapsed() >= KEEPALIVE)
                    {
                        attempted = true;
                        result = devices
                            .set_target_power(target, rider_max)
                            .await
                            .map(|_| ());
                        if result.is_ok() {
                            last_sent = Some(target);
                        }
                    }
                }
                Phase::Idle | Phase::Stopped => {}
            }
            if attempted {
                last_attempt = Instant::now();
                status = match result {
                    Ok(()) => {
                        failures = 0;
                        ControlStatus::Ok
                    }
                    Err(error) => {
                        failures = failures.saturating_add(1);
                        last_sent = None;
                        tracing::warn!(error = %error, failures, "Trainer command failed; ride continues");
                        classify_failure(&devices, error, failures, &mut started, intent.phase)
                            .await
                    }
                };
            }
        }
        status_tx.send_if_modified(|current| {
            if *current == status {
                false
            } else {
                tracing::info!(from = ?*current, to = ?status, "Trainer control status changed");
                *current = status;
                true
            }
        });

        // Wait for a reason to act again: a new intent, a link state change,
        // or the retry / keepalive timer.
        let timer = if !connected {
            None
        } else if failures > 0 {
            Some(last_attempt + RETRY_AFTER)
        } else if intent.phase == Phase::Running && intent.target.is_some() {
            Some(last_attempt + KEEPALIVE)
        } else {
            None
        };
        tokio::select! {
            changed = intent_rx.changed() => {
                if changed.is_err() {
                    return;
                }
            }
            changed = trainer_state.changed() => {
                if changed.is_err() {
                    return;
                }
            }
            _ = async {
                match timer {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                // Force a (re)write on the next pass.
                last_sent = None;
            }
        }
    }
}

async fn classify_failure(
    devices: &DeviceHub,
    error: ControlError,
    failures: u8,
    started: &mut bool,
    phase: Phase,
) -> ControlStatus {
    match error {
        ControlError::NotConnected => {
            *started = false;
            devices.reconnect_trainer().await;
            ControlStatus::Lost
        }
        ControlError::Refused(ResponseCode::ControlNotPermitted) => {
            tracing::warn!("Trainer says control is not permitted; re-acquiring");
            let result = if phase == Phase::Paused {
                devices.request_control().await
            } else {
                devices.reacquire_control().await
            };
            match result {
                Ok(()) => *started = phase == Phase::Running,
                Err(error) => {
                    tracing::warn!(error = %error, "Could not re-acquire trainer control");
                    *started = false;
                }
            }
            ControlStatus::Degraded
        }
        ControlError::Refused(_) | ControlError::Busy(_) | ControlError::Unsupported(_) => {
            ControlStatus::Degraded
        }
        ControlError::Timeout | ControlError::Gatt(_) => {
            if failures >= FAILURES_BEFORE_LOST {
                *started = false;
                devices.reconnect_trainer().await;
                ControlStatus::Lost
            } else {
                ControlStatus::Degraded
            }
        }
    }
}

/// Where recorded telemetry goes. `Storage` is the real one; tests inject
/// failing sinks.
pub trait SampleSink: Send + Sync {
    fn write_samples(&self, session_id: Uuid, samples: &[Telemetry]) -> Result<(), String>;
}

impl SampleSink for Storage {
    fn write_samples(&self, session_id: Uuid, samples: &[Telemetry]) -> Result<(), String> {
        self.record_samples(session_id, samples)
    }
}

enum RecorderMessage {
    Sample(Telemetry),
    Flush(std::sync::mpsc::Sender<Result<(), String>>),
}

#[derive(Default)]
struct RecordingHealth {
    dropped: u32,
    write_failed: bool,
    stopped: bool,
}

impl RecordingHealth {
    fn warning(&self) -> Option<String> {
        if self.stopped {
            Some("Ride recording stopped. Some or all ride data may be missing.".into())
        } else if self.write_failed {
            Some("Ride data is not being saved. Retrying; check available disk space and keep the app open.".into())
        } else if self.dropped > 0 {
            Some(format!(
                "Ride recording is incomplete: {} measurements could not be saved.",
                self.dropped
            ))
        } else {
            None
        }
    }
}

type SharedRecordingHealth = Arc<std::sync::Mutex<RecordingHealth>>;

fn recording_health(health: &SharedRecordingHealth) -> std::sync::MutexGuard<'_, RecordingHealth> {
    health
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Persists telemetry independently of the ride clock. Health is shared with
/// the timeline so disk failures are visible even while it keeps running.
struct Recorder {
    tx: Option<SyncSender<RecorderMessage>>,
    health: SharedRecordingHealth,
}

impl Recorder {
    fn start(sink: Arc<dyn SampleSink>, session_id: Uuid) -> Self {
        let (tx, rx) = sync_channel(256);
        let health = Arc::new(std::sync::Mutex::new(RecordingHealth::default()));
        let worker_health = health.clone();
        let spawned = std::thread::Builder::new()
            .name("ride-recorder".into())
            .spawn(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    recorder_loop(rx, sink, session_id, &worker_health);
                }));
                if outcome.is_err() {
                    recording_health(&worker_health).stopped = true;
                    tracing::error!(%session_id, "Ride recorder panicked");
                }
            });
        match spawned {
            Ok(_) => Self {
                tx: Some(tx),
                health,
            },
            Err(error) => {
                tracing::error!(error = %error, "Could not start the ride recorder");
                recording_health(&health).stopped = true;
                Self { tx: None, health }
            }
        }
    }

    fn push(&self, sample: Telemetry) {
        let Some(tx) = &self.tx else {
            recording_health(&self.health).dropped += 1;
            return;
        };
        if let Err(error) = tx.try_send(RecorderMessage::Sample(sample)) {
            let mut health = recording_health(&self.health);
            health.dropped += 1;
            health.stopped |= matches!(error, TrySendError::Disconnected(_));
            if health.dropped == 1 || health.dropped.is_multiple_of(100) {
                tracing::warn!(
                    dropped = health.dropped,
                    "Recorder dropped telemetry samples"
                );
            }
        }
    }

    /// One deadline covers both queueing and acknowledgement. Neither can
    /// block the async runtime; a failed write is never acknowledged as saved.
    async fn flush(&self, grace: Duration) -> Result<(), String> {
        let tx = self.tx.clone().ok_or("Ride recorder is unavailable")?;
        tokio::task::spawn_blocking(move || {
            let deadline = std::time::Instant::now() + grace;
            let (ack_tx, ack_rx) = std::sync::mpsc::channel();
            let mut message = RecorderMessage::Flush(ack_tx);
            loop {
                if std::time::Instant::now() >= deadline {
                    return Err("Timed out waiting for ride data to be saved".into());
                }
                match tx.try_send(message) {
                    Ok(()) => break,
                    Err(TrySendError::Full(returned)) => {
                        message = returned;
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(TrySendError::Disconnected(_)) => {
                        return Err("Ride recorder stopped".into());
                    }
                }
            }
            ack_rx
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                .map_err(|_| {
                    "Ride data could not be confirmed saved before the deadline".to_string()
                })?
        })
        .await
        .map_err(|error| format!("Could not wait for ride recording: {error}"))?
    }

    fn warning(&self) -> Option<String> {
        recording_health(&self.health).warning()
    }

    fn dropped(&self) -> u32 {
        recording_health(&self.health).dropped
    }
}

fn recorder_loop(
    rx: Receiver<RecorderMessage>,
    sink: Arc<dyn SampleSink>,
    session_id: Uuid,
    health: &SharedRecordingHealth,
) {
    let mut pending: Vec<Telemetry> = Vec::new();
    let mut oldest: Option<std::time::Instant> = None;
    loop {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(RecorderMessage::Sample(sample)) => {
                pending.push(sample);
                oldest.get_or_insert_with(std::time::Instant::now);
                if pending.len() > MAX_PENDING_SAMPLES {
                    let excess = pending.len() - MAX_PENDING_SAMPLES;
                    pending.drain(..excess);
                    recording_health(health).dropped += excess as u32;
                    tracing::warn!(excess, "Recorder backlog full; oldest samples dropped");
                }
            }
            Ok(RecorderMessage::Flush(ack)) => {
                let result = write_with_retries(&*sink, session_id, &mut pending, 8, health);
                // Keep retrying retained samples if the flush failed.
                oldest = (!pending.is_empty()).then(std::time::Instant::now);
                let _ = ack.send(result);
                continue;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                let _ = write_with_retries(&*sink, session_id, &mut pending, 8, health);
                return;
            }
        }
        if !pending.is_empty() && oldest.is_some_and(|at| at.elapsed() >= Duration::from_secs(1)) {
            let _ = write_with_retries(&*sink, session_id, &mut pending, 1, health);
            // Back off without sleeping the consumer: drain the queue while
            // waiting so a disk failure does not itself cause queue overflow.
            oldest = if pending.is_empty() {
                None
            } else {
                Some(std::time::Instant::now())
            };
        }
    }
}

fn write_with_retries(
    sink: &dyn SampleSink,
    session_id: Uuid,
    pending: &mut Vec<Telemetry>,
    attempts: u32,
    health: &SharedRecordingHealth,
) -> Result<(), String> {
    for attempt in 1..=attempts {
        if pending.is_empty() {
            return Ok(());
        }
        match sink.write_samples(session_id, pending) {
            Ok(()) => {
                let mut health = recording_health(health);
                if health.write_failed {
                    tracing::info!("Telemetry writes recovered");
                }
                health.write_failed = false;
                pending.clear();
                return Ok(());
            }
            Err(error) => {
                let mut health = recording_health(health);
                if !health.write_failed {
                    tracing::error!(%error, "Could not save telemetry; retaining samples for retry");
                }
                health.write_failed = true;
            }
        }
        if attempt < attempts {
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    Err("Some ride measurements could not be saved. Check available disk space; the recording may be incomplete.".into())
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

/// Close the session out: summary row, distance estimate and FIT file. Never
/// fails; every problem is logged and the startup reconciliation retries the
/// FIT file.
fn finish(
    storage: &Storage,
    ride_files_dir: &Path,
    mut summary: SessionSummary,
    stats: &RideStats,
    completed: bool,
    mut warning: Option<String>,
    flushed: bool,
) -> Result<Option<String>, String> {
    summary.ended_at = Some(Utc::now());
    stats.apply(&mut summary);
    let detail = storage
        .session(summary.id)
        .map_err(|error| format!("Could not verify saved ride data: {error}"))?
        .ok_or("The ride could not be found in History")?;
    if detail.samples.is_empty() {
        warning = Some(
            "No ride measurements were saved. Check available disk space and device connections."
                .into(),
        );
    }
    let estimate = estimate_distance(&detail.samples, summary.distance_weight_kg);
    summary.estimated_distance_meters = estimate.total_meters;
    summary.distance_source = estimate.source;
    summary.completed = completed;
    summary.recording_warning = warning.clone();
    storage.finish_session(&summary).map_err(|error| {
        format!("The ride summary could not be saved; restart recovery will retry: {error}")
    })?;
    tracing::info!(session_id = %summary.id, completed, samples = detail.samples.len(), warning = ?warning, "Ride finalized");
    // A timed-out recorder may still drain buffered samples. Let startup
    // reconciliation create the FIT from the eventual recording, not a stale
    // snapshot that would subsequently be mistaken for a complete export.
    if flushed {
        let detail = crate::domain::SessionDetail {
            summary,
            samples: detail.samples,
        };
        ensure_ride_file(ride_files_dir, &detail).map_err(|error| {
            format!("Ride data is in History, but its FIT file could not be saved: {error}")
        })?;
    }
    Ok(warning)
}

async fn emit_state(app: Option<&AppHandle>, state: &RwLock<RunnerState>) {
    if let Some(app) = app {
        let _ = app.emit("workout://state", state.read().await.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        devices::{DeviceRole, trainer::simulated_devices},
        domain::{PowerTarget, WorkoutStep},
    };

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
            control: ControlStatus::Lost,
            recording_warning: None,
        };
        let json = serde_json::to_value(state).unwrap();

        assert_eq!(json["workoutName"], "Free Ride");
        assert_eq!(json["targetPowerWatts"], 100);
        assert_eq!(json["manualErg"], true);
        assert_eq!(json["biasPercent"], 100);
        assert_eq!(json["overrideActive"], false);
        assert_eq!(json["control"], "lost");
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
            control: ControlStatus::Ok,
            recording_warning: None,
        };
        let json = serde_json::to_value(state).unwrap();

        assert_eq!(json["intervalIndex"], 1);
        assert_eq!(json["intervalElapsedSeconds"], 15);
        assert_eq!(json["control"], "ok");
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

    #[tokio::test(start_paused = true)]
    async fn ride_clock_excludes_pauses_and_jumps_on_skip() {
        let start = Instant::now();
        let mut clock = RideClock::new(start);
        tokio::time::advance(Duration::from_secs(10)).await;
        assert_eq!(clock.elapsed_seconds(Instant::now()), 10);
        clock.pause(Instant::now());
        tokio::time::advance(Duration::from_secs(30)).await;
        assert!(clock.is_paused());
        assert_eq!(clock.elapsed_seconds(Instant::now()), 10);
        clock.resume(Instant::now());
        tokio::time::advance(Duration::from_secs(5)).await;
        assert_eq!(clock.elapsed_seconds(Instant::now()), 15);
        clock.skip(Duration::from_secs(45));
        assert_eq!(clock.elapsed_seconds(Instant::now()), 60);
        // Pausing twice or resuming twice is harmless.
        clock.pause(Instant::now());
        clock.pause(Instant::now());
        clock.resume(Instant::now());
        clock.resume(Instant::now());
        assert_eq!(clock.elapsed_seconds(Instant::now()), 60);
    }

    // ---- ride integration tests on the simulated trainer ------------------

    struct Rig {
        hub: Arc<DeviceHub>,
        storage: Arc<Storage>,
        runner: WorkoutRunner,
        _ride_files: tempfile::TempDir,
    }

    async fn rig() -> Rig {
        let hub = Arc::new(DeviceHub::default());
        DeviceHub::spawn_reconnect_supervisors(&hub);
        hub.connect(DeviceRole::Trainer, simulated_devices().remove(0))
            .await
            .unwrap();
        let ride_files = tempfile::tempdir().unwrap();
        Rig {
            hub,
            storage: Arc::new(Storage::in_memory().unwrap()),
            runner: WorkoutRunner::new(ride_files.path().to_path_buf()),
            _ride_files: ride_files,
        }
    }

    async fn wait_for(
        runner: &WorkoutRunner,
        accept: impl Fn(&RunnerState) -> bool,
    ) -> RunnerState {
        for _ in 0..1_200 {
            let state = runner.state().await;
            if accept(&state) {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!(
            "runner never reached the expected state: {:?}",
            runner.state().await
        );
    }

    fn control_of(state: &RunnerState) -> Option<ControlStatus> {
        match state {
            RunnerState::Running { control, .. } | RunnerState::Paused { control, .. } => {
                Some(*control)
            }
            _ => None,
        }
    }

    fn set_target_writes(hub: &DeviceHub) -> Vec<u16> {
        hub.simulated_faults()
            .commands
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, payload)| payload.first() == Some(&0x05) && payload.len() == 3)
            .map(|(_, payload)| u16::from_le_bytes([payload[1], payload[2]]))
            .collect()
    }

    #[tokio::test]
    async fn a_stopped_free_ride_is_finished_and_saved() {
        let rig = rig().await;
        let session_id = rig
            .runner
            .start_free_ride(None, 800, 84.0, rig.hub.clone(), rig.storage.clone())
            .await
            .unwrap();
        wait_for(&rig.runner, |state| {
            matches!(state, RunnerState::Running { .. })
        })
        .await;
        // Real time here: the simulator reports every 500 ms and the fuser
        // rate-limits on the wall clock, so ride long enough to record.
        tokio::time::sleep(Duration::from_millis(1_300)).await;
        rig.runner.stop().await.unwrap();
        let state = wait_for(&rig.runner, |state| {
            matches!(state, RunnerState::Finished { .. })
        })
        .await;
        assert!(matches!(
            state,
            RunnerState::Finished {
                completed: true,
                save_warning: None,
                ..
            }
        ));
        let stored = rig.storage.session(session_id).unwrap().unwrap();
        assert!(
            stored.samples.len() >= 2,
            "telemetry was recorded and flushed"
        );
        let summary = stored.summary;
        assert!(summary.ended_at.is_some());
        assert!(summary.completed);
        assert!(summary.recording_warning.is_none());
        assert!(summary.average_power_watts > 0);
        assert_eq!(
            std::fs::read_dir(rig._ride_files.path()).unwrap().count(),
            1,
            "a FIT file was written"
        );
        // The trainer was told to stop, last.
        let last = rig
            .hub
            .simulated_faults()
            .commands
            .lock()
            .unwrap()
            .last()
            .map(|(_, payload)| payload.clone());
        assert_eq!(last, Some(vec![0x08, 0x01]));
        rig.hub.disconnect().await;
    }

    #[tokio::test(start_paused = true)]
    async fn the_clock_runs_on_wall_time_even_when_the_trainer_acks_slowly() {
        let rig = rig().await;
        // Every ack takes 950 ms, like the Hammer in the field logs.
        rig.hub
            .simulated_faults()
            .ack_delay_ms
            .store(950, Ordering::Relaxed);
        let workout = Workout::new(
            "Ramp then hold",
            vec![
                WorkoutStep::Ramp {
                    duration_seconds: 3,
                    start: PowerTarget::Watts(100),
                    end: PowerTarget::Watts(200),
                },
                WorkoutStep::Steady {
                    duration_seconds: 3,
                    target: PowerTarget::Watts(200),
                },
            ],
        );
        let session_id = rig
            .runner
            .start(
                None,
                workout,
                200,
                800,
                84.0,
                rig.hub.clone(),
                rig.storage.clone(),
            )
            .await
            .unwrap();
        // Time the ride itself, from the first Running state to Finished. The
        // start handshake before it and the Stop acknowledgement after it each
        // cost one slow ack and are not workout time.
        wait_for(&rig.runner, |state| {
            matches!(state, RunnerState::Running { .. })
        })
        .await;
        let started = Instant::now();
        let state = wait_for(&rig.runner, |state| {
            matches!(state, RunnerState::Finished { .. })
        })
        .await;
        let took = started.elapsed();
        assert!(matches!(
            state,
            RunnerState::Finished {
                completed: true,
                ..
            }
        ));
        assert!(
            took >= Duration::from_secs(6) && took < Duration::from_millis(7_500),
            "a 6 s workout took {took:?} (6 s plus one Stop ack)"
        );
        let stored = rig.storage.session(session_id).unwrap().unwrap().summary;
        assert_eq!(stored.elapsed_seconds, 6);
        // Ramp: 100, 150, 200. The steady block asks for 200 again, which is
        // not re-sent; only a keepalive could add a write.
        let writes = set_target_writes(&rig.hub);
        assert!(
            writes.starts_with(&[100, 150, 200]) && writes.len() <= 4,
            "{writes:?}"
        );
        rig.hub.disconnect().await;
    }

    #[tokio::test(start_paused = true)]
    async fn command_failures_degrade_the_ride_and_recover_without_aborting() {
        let rig = rig().await;
        let session_id = rig
            .runner
            .start_free_ride(None, 800, 84.0, rig.hub.clone(), rig.storage.clone())
            .await
            .unwrap();
        rig.hub
            .simulated_faults()
            .fail_writes
            .store(3, Ordering::Relaxed);
        let degraded = wait_for(&rig.runner, |state| {
            control_of(state).is_some_and(|control| control != ControlStatus::Ok)
        })
        .await;
        assert!(
            matches!(degraded, RunnerState::Running { .. }),
            "{degraded:?}"
        );
        let recovered = wait_for(&rig.runner, |state| {
            control_of(state) == Some(ControlStatus::Ok)
        })
        .await;
        assert!(matches!(recovered, RunnerState::Running { .. }));
        rig.runner.stop().await.unwrap();
        wait_for(&rig.runner, |state| {
            matches!(
                state,
                RunnerState::Finished {
                    completed: true,
                    ..
                }
            )
        })
        .await;
        let stored = rig.storage.session(session_id).unwrap().unwrap().summary;
        assert!(stored.ended_at.is_some());
        rig.hub.disconnect().await;
    }

    #[tokio::test(start_paused = true)]
    async fn link_loss_keeps_the_ride_running_and_controls_stay_responsive() {
        let rig = rig().await;
        rig.runner
            .start_free_ride(None, 800, 84.0, rig.hub.clone(), rig.storage.clone())
            .await
            .unwrap();
        wait_for(&rig.runner, |state| {
            matches!(state, RunnerState::Running { .. })
        })
        .await;
        // The first three reconnect attempts (at 1 s, 3 s and 8 s) fail, so the
        // link stays down long enough to exercise riding without a trainer.
        rig.hub
            .simulated_faults()
            .fail_connects
            .store(3, Ordering::Relaxed);
        rig.hub.simulated_faults().drop_link.notify_one();
        let lost = wait_for(&rig.runner, |state| {
            control_of(state) == Some(ControlStatus::Lost)
        })
        .await;
        let RunnerState::Running {
            elapsed_seconds: at_loss,
            ..
        } = lost
        else {
            panic!("{lost:?}");
        };
        // The clock keeps going without a trainer.
        tokio::time::sleep(Duration::from_secs(3)).await;
        let later = wait_for(&rig.runner, |state| {
            matches!(state, RunnerState::Running { elapsed_seconds, .. } if *elapsed_seconds >= at_loss + 3)
        })
        .await;
        assert_eq!(control_of(&later), Some(ControlStatus::Lost));
        // Manual adjustments answer immediately and are queued for the trainer.
        let before = Instant::now();
        let applied = rig
            .runner
            .adjust_manual_power(None, 5, 800, &rig.hub)
            .await
            .unwrap();
        assert_eq!(applied, 105);
        assert!(before.elapsed() < Duration::from_millis(50));
        assert_eq!(rig.runner.controls.intent.borrow().target, Some(105));
        // The supervisor brings the simulator back; the writer re-acquires
        // control and applies the target chosen while the link was down.
        let restored = wait_for(&rig.runner, |state| {
            control_of(state) == Some(ControlStatus::Ok)
        })
        .await;
        assert!(
            matches!(
                restored,
                RunnerState::Running {
                    target_power_watts: Some(105),
                    ..
                }
            ),
            "{restored:?}"
        );
        let opcodes: Vec<Vec<u8>> = rig
            .hub
            .simulated_faults()
            .commands
            .lock()
            .unwrap()
            .iter()
            .map(|(_, payload)| payload.clone())
            .collect();
        let start_index = opcodes
            .iter()
            .rposition(|payload| payload == &[0x07])
            .expect("Start/Resume after reconnect");
        assert!(
            opcodes[start_index..]
                .iter()
                .any(|payload| payload == &[0x05, 105, 0]),
            "target re-applied after Start/Resume: {opcodes:?}"
        );
        // Pausing works without a trainer, and the clock freezes.
        rig.hub.simulated_faults().drop_link.notify_one();
        wait_for(&rig.runner, |state| {
            control_of(state) == Some(ControlStatus::Lost)
        })
        .await;
        rig.runner.pause_or_resume().await.unwrap();
        let paused = wait_for(&rig.runner, |state| {
            matches!(state, RunnerState::Paused { .. })
        })
        .await;
        let RunnerState::Paused {
            elapsed_seconds: at_pause,
            ..
        } = paused
        else {
            panic!("{paused:?}");
        };
        tokio::time::sleep(Duration::from_secs(5)).await;
        let still = rig.runner.state().await;
        assert!(
            matches!(still, RunnerState::Paused { elapsed_seconds, .. } if elapsed_seconds == at_pause)
        );
        rig.runner.pause_or_resume().await.unwrap();
        rig.runner.stop().await.unwrap();
        wait_for(&rig.runner, |state| {
            matches!(state, RunnerState::Finished { .. })
        })
        .await;
        rig.hub.disconnect().await;
    }

    #[tokio::test(start_paused = true)]
    async fn skipping_advances_the_workout_on_the_clock() {
        let rig = rig().await;
        let workout = Workout::new(
            "Two blocks",
            vec![
                WorkoutStep::Steady {
                    duration_seconds: 600,
                    target: PowerTarget::Watts(120),
                },
                WorkoutStep::Steady {
                    duration_seconds: 2,
                    target: PowerTarget::Watts(180),
                },
            ],
        );
        let started = Instant::now();
        rig.runner
            .start(
                None,
                workout,
                200,
                800,
                84.0,
                rig.hub.clone(),
                rig.storage.clone(),
            )
            .await
            .unwrap();
        wait_for(&rig.runner, |state| {
            matches!(state, RunnerState::Running { .. })
        })
        .await;
        rig.runner.skip().unwrap();
        let second = wait_for(&rig.runner, |state| {
            matches!(
                state,
                RunnerState::Running {
                    interval_index: 1,
                    ..
                }
            )
        })
        .await;
        assert!(
            matches!(
                second,
                RunnerState::Running {
                    elapsed_seconds: 600,
                    target_power_watts: Some(180),
                    ..
                }
            ),
            "{second:?}"
        );
        wait_for(&rig.runner, |state| {
            matches!(
                state,
                RunnerState::Finished {
                    completed: true,
                    ..
                }
            )
        })
        .await;
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(set_target_writes(&rig.hub), vec![120, 180]);
        rig.hub.disconnect().await;
    }

    #[tokio::test]
    async fn stop_and_wait_returns_with_the_session_finalized() {
        let rig = rig().await;
        let session_id = rig
            .runner
            .start_free_ride(None, 800, 84.0, rig.hub.clone(), rig.storage.clone())
            .await
            .unwrap();
        wait_for(&rig.runner, |state| {
            matches!(state, RunnerState::Running { .. })
        })
        .await;
        rig.runner.stop_and_wait(Duration::from_secs(5)).await;
        // No polling: by the time it returns, the ride is closed.
        assert!(matches!(
            rig.runner.state().await,
            RunnerState::Finished { .. }
        ));
        assert!(
            rig.storage
                .session(session_id)
                .unwrap()
                .unwrap()
                .summary
                .ended_at
                .is_some()
        );
        // Calling it again with nothing active is harmless.
        rig.runner.stop_and_wait(Duration::from_secs(1)).await;
        rig.hub.disconnect().await;
    }

    struct FlakySink {
        inner: Arc<Storage>,
        remaining_failures: std::sync::Mutex<u32>,
        calls: std::sync::Mutex<u32>,
    }

    impl SampleSink for FlakySink {
        fn write_samples(&self, session_id: Uuid, samples: &[Telemetry]) -> Result<(), String> {
            *self.calls.lock().unwrap() += 1;
            let mut remaining = self.remaining_failures.lock().unwrap();
            if *remaining > 0 {
                *remaining -= 1;
                return Err("disk on fire".into());
            }
            self.inner.write_samples(session_id, samples)
        }
    }

    #[tokio::test]
    async fn the_recorder_retries_failed_writes_and_flushes_everything() {
        let storage = Arc::new(Storage::in_memory().unwrap());
        let session = storage.start_session(None, "Flaky", 84.0).unwrap();
        let sink = Arc::new(FlakySink {
            inner: storage.clone(),
            remaining_failures: std::sync::Mutex::new(2),
            calls: std::sync::Mutex::new(0),
        });
        let recorder = Recorder::start(sink.clone(), session.id);
        for index in 0..25_i64 {
            recorder.push(Telemetry {
                timestamp_ms: 1_700_000_000_000 + index * 200,
                power_watts: 150,
                ..Telemetry::default()
            });
        }
        recorder.flush(Duration::from_secs(10)).await.unwrap();
        assert_eq!(recorder.dropped(), 0);
        assert!(*sink.calls.lock().unwrap() >= 3);
        assert_eq!(
            storage.session(session.id).unwrap().unwrap().samples.len(),
            25
        );
    }

    #[tokio::test(start_paused = true)]
    async fn silent_link_and_repeated_write_failures_trigger_reconnect() {
        let rig = rig().await;
        rig.runner
            .start_free_ride(None, 800, 84.0, rig.hub.clone(), rig.storage.clone())
            .await
            .unwrap();
        wait_for(&rig.runner, |s| matches!(s, RunnerState::Running { .. })).await;
        rig.hub.slot(DeviceRole::Trainer).abort_worker().await;
        rig.hub
            .simulated_faults()
            .fail_writes
            .store(3, Ordering::Relaxed);
        wait_for(&rig.runner, |s| control_of(s) == Some(ControlStatus::Lost)).await;
        wait_for(&rig.runner, |s| control_of(s) == Some(ControlStatus::Ok)).await;
        assert!(rig.hub.slot(DeviceRole::Trainer).stats().drops >= 1);
        assert!(rig.hub.slot(DeviceRole::Trainer).stats().samples > 0);
        assert!(rig.hub.state().await.is_connected());
        assert!(matches!(
            rig.runner.state().await,
            RunnerState::Running { .. }
        ));
        rig.runner.stop_and_wait(Duration::from_secs(5)).await;
        rig.hub.disconnect().await;
    }

    async fn paused_recovery(failures: u32, refusal: u8) {
        let rig = rig().await;
        rig.runner
            .start_free_ride(None, 800, 84.0, rig.hub.clone(), rig.storage.clone())
            .await
            .unwrap();
        wait_for(&rig.runner, |s| matches!(s, RunnerState::Running { .. })).await;
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        let faults = rig.hub.simulated_faults();
        let command_count = faults.commands.lock().unwrap().len();
        faults.fail_writes.store(failures, Ordering::Relaxed);
        faults.refuse_with.store(refusal, Ordering::Relaxed);
        rig.runner.pause_or_resume().await.unwrap();
        wait_for(&rig.runner, |s| {
            matches!(
                s,
                RunnerState::Paused {
                    control: ControlStatus::Degraded | ControlStatus::Lost,
                    ..
                }
            )
        })
        .await;
        wait_for(&rig.runner, |s| {
            matches!(
                s,
                RunnerState::Paused {
                    control: ControlStatus::Ok,
                    ..
                }
            )
        })
        .await;
        {
            let commands = faults.commands.lock().unwrap();
            let after_pause = &commands[command_count..];
            assert!(
                after_pause
                    .iter()
                    .filter(|(_, p)| p.as_slice() == [0x08, 0x02])
                    .count()
                    >= 2
            );
            assert!(
                !after_pause.iter().any(|(_, p)| p.as_slice() == [0x07]),
                "A paused ride must never be started by recovery"
            );
        }
        if failures >= 3 {
            assert!(rig.hub.slot(DeviceRole::Trainer).stats().drops > 0);
        }
        rig.runner.stop_and_wait(Duration::from_secs(5)).await;
        rig.hub.disconnect().await;
    }

    #[tokio::test(start_paused = true)]
    async fn failed_pause_is_retried_until_acknowledged() {
        paused_recovery(1, 0).await;
    }

    #[tokio::test(start_paused = true)]
    async fn reconnect_while_paused_confirms_pause_without_resuming() {
        paused_recovery(3, 0).await;
    }

    #[tokio::test(start_paused = true)]
    async fn pause_reacquires_permission_without_starting_trainer() {
        paused_recovery(0, 5).await;
    }

    #[tokio::test]
    async fn recording_failure_is_visible_and_persisted_without_stopping_ride() {
        let mut rig = rig().await;
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("failed-recording.sqlite");
        rig.storage = Arc::new(Storage::open(&path).unwrap());
        let fault = rusqlite::Connection::open(&path).unwrap();
        fault.execute_batch("CREATE TRIGGER fail_samples BEFORE INSERT ON telemetry_samples BEGIN SELECT RAISE(FAIL, 'injected write failure'); END;").unwrap();
        let id = rig
            .runner
            .start_free_ride(None, 800, 84.0, rig.hub.clone(), rig.storage.clone())
            .await
            .unwrap();
        wait_for(&rig.runner, |s| {
            matches!(
                s,
                RunnerState::Running {
                    recording_warning: Some(_),
                    ..
                }
            )
        })
        .await;
        rig.runner.stop_and_wait(Duration::from_secs(10)).await;
        let finished = rig.runner.state().await;
        assert!(
            matches!(
                finished,
                RunnerState::Finished {
                    save_warning: Some(_),
                    ..
                }
            ),
            "{finished:?}"
        );
        let reopened = Storage::open(&path).unwrap();
        let detail = reopened.session(id).unwrap().unwrap();
        assert!(detail.samples.is_empty());
        assert!(
            detail
                .summary
                .recording_warning
                .as_deref()
                .unwrap()
                .contains("No ride measurements")
        );
        assert!(reopened.sessions().unwrap()[0].recording_warning.is_some());
        // Failed flushes must not create an apparently complete, stale FIT.
        assert_eq!(
            std::fs::read_dir(rig._ride_files.path()).unwrap().count(),
            0
        );
        fault.execute_batch("DROP TRIGGER fail_samples;").unwrap();
        rig.hub.disconnect().await;
    }

    #[tokio::test]
    async fn a_temporary_recording_failure_clears_after_all_samples_are_saved() {
        let storage = Arc::new(Storage::in_memory().unwrap());
        let session = storage.start_session(None, "Retry", 84.0).unwrap();
        let sink = Arc::new(FlakySink {
            inner: storage.clone(),
            remaining_failures: std::sync::Mutex::new(100),
            calls: std::sync::Mutex::new(0),
        });
        let recorder = Recorder::start(sink.clone(), session.id);
        for i in 0..20 {
            recorder.push(Telemetry {
                timestamp_ms: 1700000000000 + i * 200,
                ..Telemetry::default()
            });
        }
        for _ in 0..100 {
            if recorder.warning().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(recorder.warning().is_some());
        *sink.remaining_failures.lock().unwrap() = 0;
        recorder.flush(Duration::from_secs(5)).await.unwrap();
        assert!(recorder.warning().is_none());
        assert_eq!(
            storage.session(session.id).unwrap().unwrap().samples.len(),
            20
        );
    }

    #[tokio::test]
    async fn flush_deadline_includes_a_full_queue() {
        let (tx, rx) = sync_channel(1);
        tx.send(RecorderMessage::Sample(Telemetry::default()))
            .unwrap();
        let consumer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            rx.recv().unwrap();
        });
        let recorder = Recorder {
            tx: Some(tx),
            health: Arc::new(std::sync::Mutex::new(RecordingHealth::default())),
        };
        assert!(recorder.flush(Duration::from_millis(20)).await.is_err());
        assert!(
            !consumer.is_finished(),
            "Flush waited for the blocked consumer instead of its deadline"
        );
        consumer.join().unwrap();
    }

    #[tokio::test]
    async fn dropped_samples_remain_a_warning_after_successful_flush() {
        let (tx, rx) = sync_channel(1);
        let recorder = Recorder {
            tx: Some(tx),
            health: Arc::new(std::sync::Mutex::new(RecordingHealth::default())),
        };
        recorder.push(Telemetry::default());
        recorder.push(Telemetry::default()); // Full queue: this sample is lost.
        assert_eq!(recorder.dropped(), 1);
        let consumer = std::thread::spawn(move || {
            rx.recv().unwrap();
            if let RecorderMessage::Flush(ack) = rx.recv().unwrap() {
                ack.send(Ok(())).unwrap();
            }
        });
        recorder.flush(Duration::from_secs(5)).await.unwrap();
        assert!(recorder.warning().unwrap().contains("1 measurements"));
        consumer.join().unwrap();
    }

    #[tokio::test]
    async fn recorder_worker_death_is_visible_without_another_sample() {
        struct PanickingSink;
        impl SampleSink for PanickingSink {
            fn write_samples(&self, _: Uuid, _: &[Telemetry]) -> Result<(), String> {
                panic!("injected recorder failure")
            }
        }
        let recorder = Recorder::start(Arc::new(PanickingSink), Uuid::new_v4());
        recorder.push(Telemetry::default());
        assert!(recorder.flush(Duration::from_secs(2)).await.is_err());
        for _ in 0..100 {
            if recorder.warning().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(recorder.warning().unwrap().contains("recording stopped"));
    }

    #[tokio::test]
    async fn summary_save_failure_is_reported_even_when_samples_were_saved() {
        let mut rig = rig().await;
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("summary-failure.sqlite");
        rig.storage = Arc::new(Storage::open(&path).unwrap());
        let fault = rusqlite::Connection::open(&path).unwrap();
        fault.execute_batch("CREATE TRIGGER fail_summary BEFORE UPDATE OF ended_at ON sessions BEGIN SELECT RAISE(FAIL, 'injected summary failure'); END;").unwrap();
        let id = rig
            .runner
            .start_free_ride(None, 800, 84.0, rig.hub.clone(), rig.storage.clone())
            .await
            .unwrap();
        wait_for(&rig.runner, |s| matches!(s, RunnerState::Running { .. })).await;
        tokio::time::sleep(Duration::from_millis(600)).await;
        rig.runner.stop_and_wait(Duration::from_secs(5)).await;
        assert!(matches!(
            rig.runner.state().await,
            RunnerState::Finished {
                save_warning: Some(_),
                ..
            }
        ));
        let detail = rig.storage.session(id).unwrap().unwrap();
        assert!(!detail.samples.is_empty());
        assert!(detail.summary.ended_at.is_none());
        assert!(
            detail
                .summary
                .recording_warning
                .unwrap()
                .contains("summary could not be saved")
        );
        fault.execute_batch("DROP TRIGGER fail_summary;").unwrap();
        rig.hub.disconnect().await;
    }
}
