mod commands;
mod devices;
mod distance;
mod domain;
mod fit;
mod formats;
mod ftms;
mod runner;
mod storage;

use std::{fs, path::PathBuf, sync::Arc};

use devices::DeviceHub;
use runner::WorkoutRunner;
use storage::Storage;
use tauri::Manager;
use tracing_subscriber::EnvFilter;

pub struct AppState {
    devices: Arc<DeviceHub>,
    runner: Arc<WorkoutRunner>,
    storage: Arc<Storage>,
    log_path: PathBuf,
    ride_files_dir: PathBuf,
    _log_guard: tracing_appender::non_blocking::WorkerGuard,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let log_dir = app.path().app_log_dir()?;
            fs::create_dir_all(&log_dir)?;
            let log_path = log_dir.join("blakebike.log");
            let appender = tracing_appender::rolling::never(&log_dir, "blakebike.log");
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
                log_file = %log_path.display(),
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
            app.manage(AppState {
                devices: Arc::new(devices),
                runner: Arc::new(WorkoutRunner::default()),
                storage: Arc::new(storage),
                log_path,
                ride_files_dir,
                _log_guard: log_guard,
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                tracing::info!("Window destroyed; stopping workout and disconnecting devices");
                let state = window.state::<AppState>();
                let devices = Arc::clone(&state.devices);
                let runner = Arc::clone(&state.runner);
                tauri::async_runtime::spawn(async move {
                    let _ = runner.stop(&devices).await;
                    devices.disconnect().await;
                });
            }
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
            commands::device_log,
            commands::get_source_preferences,
            commands::set_source_preferences,
            commands::known_devices,
            commands::forget_device,
            commands::forget_all_devices,
            commands::get_profile,
            commands::save_profile,
            commands::list_workouts,
            commands::get_workout,
            commands::save_workout,
            commands::delete_workout,
            commands::import_zwo_workout,
            commands::export_zwo_workout,
            commands::runner_state,
            commands::start_workout,
            commands::start_free_ride,
            commands::adjust_manual_power,
            commands::set_manual_power,
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
