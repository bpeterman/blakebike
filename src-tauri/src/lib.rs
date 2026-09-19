mod commands;
mod default_workouts;
mod devices;
mod distance;
mod domain;
mod fit;
mod formats;
mod ftms;
mod intervals;
mod runner;
mod storage;

use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use devices::DeviceHub;
use runner::WorkoutRunner;
use storage::Storage;
use tauri::{AppHandle, Manager, RunEvent};
use tracing_subscriber::EnvFilter;

/// How long a quit waits for the ride to finalize and the trainer to be
/// released before the process exits regardless.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);
/// Daily log files kept before the oldest is pruned.
const LOG_FILES_KEPT: usize = 14;

pub struct AppState {
    devices: Arc<DeviceHub>,
    runner: Arc<WorkoutRunner>,
    storage: Arc<Storage>,
    log_dir: PathBuf,
    ride_files_dir: PathBuf,
    _log_guard: tracing_appender::non_blocking::WorkerGuard,
}

impl AppState {
    /// Today's log file. The appender rotates daily on UTC dates.
    pub fn log_path(&self) -> PathBuf {
        self.log_dir.join(format!(
            "blakebike.{}.log",
            chrono::Utc::now().format("%Y-%m-%d")
        ))
    }
}

static SHUTDOWN_STARTED: AtomicBool = AtomicBool::new(false);

/// Finish the ride (session summary, FIT file), release the trainer and stop
/// reconnecting. Idempotent and bounded by the caller.
async fn shutdown(app: &AppHandle) {
    let state = app.state::<AppState>();
    let devices = Arc::clone(&state.devices);
    let runner = Arc::clone(&state.runner);
    devices.cancel_reconnects();
    runner.stop_and_wait(SHUTDOWN_GRACE).await;
    devices.disconnect().await;
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let log_dir = app.path().app_log_dir()?;
            fs::create_dir_all(&log_dir)?;
            let appender = tracing_appender::rolling::Builder::new()
                .rotation(tracing_appender::rolling::Rotation::DAILY)
                .filename_prefix("blakebike")
                .filename_suffix("log")
                .max_log_files(LOG_FILES_KEPT)
                .build(&log_dir)?;
            let (log_writer, log_guard) = tracing_appender::non_blocking(appender);
            // Default: our own crate at debug (every BLE step, state change and
            // command), btleplug's internals at debug (they use the `log` crate
            // and are bridged in by tracing-subscriber's log feature), everything
            // else at info. Override with e.g. RUST_LOG=trace for a run.
            let filter = EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,blakebike_lib=debug,btleplug=debug"));
            let _ = tracing_subscriber::fmt()
                .with_ansi(false)
                .with_target(true)
                .with_file(true)
                .with_line_number(true)
                .with_thread_ids(true)
                .with_writer(log_writer)
                .with_env_filter(filter)
                .try_init();
            // Panics anywhere (including inside Tokio tasks) end up in the log
            // instead of only on a stderr nobody is watching.
            std::panic::set_hook(Box::new(|info| {
                let location = info
                    .location()
                    .map(|l| format!("{}:{}", l.file(), l.line()))
                    .unwrap_or_default();
                tracing::error!(location = %location, panic = %info, "Panic");
            }));
            tracing::info!(
                version = env!("CARGO_PKG_VERSION"),
                os = std::env::consts::OS,
                arch = std::env::consts::ARCH,
                debug_build = cfg!(debug_assertions),
                log_dir = %log_dir.display(),
                "blake.bike started"
            );
            let app_data_dir = app.path().app_data_dir()?;
            let database_path = app_data_dir.join("blakebike.sqlite3");
            let ride_files_dir = app_data_dir.join("Ride Files");
            fs::create_dir_all(&ride_files_dir)?;
            tracing::info!(database = %database_path.display(), "Opening local storage");
            let storage = Storage::open(&database_path).map_err(|error| {
                tracing::error!(error = %error, database = %database_path.display(), "Storage init failed");
                format!("Could not initialize local storage: {error}")
            })?;
            // Rides the previous run never closed (crash, kill, power loss) get
            // their summary from the samples that did make it to disk.
            match storage.finalize_orphaned_sessions() {
                Ok(report) => tracing::info!(
                    finalized = report.finalized,
                    deleted = report.deleted,
                    failed = report.failed,
                    "Unfinished sessions recovered"
                ),
                Err(error) => {
                    tracing::warn!(error = %error, "Could not recover unfinished sessions")
                }
            }
            let reconciliation = fit::reconcile_ride_files(&ride_files_dir, &storage);
            tracing::info!(
                generated = reconciliation.generated,
                existing = reconciliation.existing,
                failures = reconciliation.failures.len(),
                directory = %ride_files_dir.display(),
                "Ride Files reconciled"
            );
            for (session_id, error) in reconciliation.failures {
                tracing::warn!(%session_id, %error, "Could not backfill ride FIT file");
            }
            let devices = DeviceHub::new(app.handle().clone());
            match storage.source_preferences() {
                Ok(preferences) => devices.set_source_preferences(preferences),
                Err(error) => {
                    tracing::warn!(error = %error, "Could not load telemetry source preferences; using Auto")
                }
            }
            let devices = Arc::new(devices);
            {
                // Supervisors need a runtime; Tauri's is a tokio runtime.
                let hub = devices.clone();
                tauri::async_runtime::spawn(async move {
                    DeviceHub::spawn_reconnect_supervisors(&hub);
                });
            }
            app.manage(AppState {
                devices,
                runner: Arc::new(WorkoutRunner::new(ride_files_dir.clone())),
                storage: Arc::new(storage),
                log_dir,
                ride_files_dir,
                _log_guard: log_guard,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::device_state,
            commands::scan_trainers,
            commands::connect_trainer,
            commands::disconnect_trainer,
            commands::devices_snapshot,
            commands::scan_devices,
            commands::connect_device,
            commands::disconnect_device,
            commands::calibrate_trainer,
            commands::device_log,
            commands::get_source_preferences,
            commands::set_source_preferences,
            commands::known_devices,
            commands::forget_device,
            commands::forget_all_devices,
            commands::restore_known_devices,
            commands::get_profile,
            commands::save_profile,
            commands::intervals_api_key_configured,
            commands::save_intervals_api_key,
            commands::clear_intervals_api_key,
            commands::refresh_estimated_ftp,
            commands::list_workouts,
            commands::get_workout,
            commands::save_workout,
            commands::delete_workout,
            commands::import_zwo_workout,
            commands::export_zwo_workout,
            commands::export_all_zwo_workouts,
            commands::runner_state,
            commands::start_workout,
            commands::start_free_ride,
            commands::adjust_manual_power,
            commands::set_manual_power,
            commands::clear_target_override,
            commands::set_bias_percent,
            commands::get_power_smoothing,
            commands::set_power_smoothing,
            commands::get_dev_mode,
            commands::set_dev_mode,
            commands::get_training_zones,
            commands::set_training_zones,
            commands::get_ride_display_preferences,
            commands::set_ride_display_preferences,
            commands::pause_or_resume_workout,
            commands::skip_interval,
            commands::stop_workout,
            commands::list_sessions,
            commands::get_session,
            commands::export_session_csv,
            commands::export_session_fit,
            commands::prepare_garmin_upload,
            commands::get_ride_files_path,
            commands::reveal_ride_files,
            commands::get_log_file_path,
            commands::reveal_log_file,
            commands::report_client_error,
            commands::report_client_event,
            commands::debug_inject_trainer_fault,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // Closing the last window or quitting asks to exit; hold that
            // until the ride is finalized and the trainer released, then exit
            // for real (the second request passes straight through).
            if let RunEvent::ExitRequested { api, code, .. } = &event
                && !SHUTDOWN_STARTED.swap(true, Ordering::SeqCst)
            {
                tracing::info!(code = ?code, "Exit requested; finishing the ride first");
                api.prevent_exit();
                let app = app.clone();
                let code = code.unwrap_or(0);
                tauri::async_runtime::spawn(async move {
                    if tokio::time::timeout(SHUTDOWN_GRACE + Duration::from_secs(2), shutdown(&app))
                        .await
                        .is_err()
                    {
                        tracing::warn!("Shutdown did not complete in time; exiting anyway");
                    }
                    tracing::info!("Shutdown complete");
                    app.exit(code);
                });
            }
        });
}
