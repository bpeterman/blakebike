use std::{fs, path::PathBuf, sync::Arc};

use chrono::Utc;
use tauri::{AppHandle, State};
use uuid::Uuid;

use crate::{
    AppState,
    devices::{
        DeviceInfo, DeviceLogLine, DeviceRole, DeviceState, DevicesSnapshot, KnownDevice,
        SourcePreferences,
    },
    domain::{Profile, SessionDetail, SessionSummary, Workout},
    fit::ensure_ride_file,
    formats::{export_zwo, import_zwo},
    runner::RunnerState,
};

// Trainer-only commands kept for the pre-hub UI; they delegate to the hub.

#[tauri::command]
pub async fn device_state(state: State<'_, AppState>) -> Result<DeviceState, String> {
    Ok(state.devices.state().await)
}

#[tauri::command]
pub async fn scan_trainers(state: State<'_, AppState>) -> Result<Vec<DeviceInfo>, String> {
    tracing::debug!("command scan_trainers");
    state.devices.scan_trainers().await
}

#[tauri::command]
pub async fn connect_trainer(state: State<'_, AppState>, device: DeviceInfo) -> Result<(), String> {
    tracing::debug!(device = ?device, "command connect_trainer");
    state.devices.connect(DeviceRole::Trainer, device).await?;
    remember(&state, DeviceRole::Trainer).await;
    Ok(())
}

/// Persist a just-connected device so it shows up under Known devices. Never
/// fails the connect: a storage hiccup is logged and the ride goes on.
async fn remember(state: &State<'_, AppState>, role: DeviceRole) {
    if let Some(device) = state.devices.remember(role).await
        && let Err(error) = state.storage.remember_device(&device)
    {
        tracing::warn!(?role, error = %error, "Could not remember device");
    }
}

#[tauri::command]
pub async fn disconnect_trainer(state: State<'_, AppState>) -> Result<(), String> {
    tracing::debug!("command disconnect_trainer");
    state.devices.disconnect_role(DeviceRole::Trainer).await;
    Ok(())
}

// Devices hub commands.

#[tauri::command]
pub async fn devices_snapshot(state: State<'_, AppState>) -> Result<DevicesSnapshot, String> {
    Ok(state.devices.snapshot().await)
}

#[tauri::command]
pub async fn scan_devices(state: State<'_, AppState>) -> Result<Vec<DeviceInfo>, String> {
    tracing::debug!("command scan_devices");
    state.devices.scan().await
}

#[tauri::command]
pub async fn connect_device(
    state: State<'_, AppState>,
    role: DeviceRole,
    device: DeviceInfo,
) -> Result<(), String> {
    tracing::debug!(?role, device = ?device, "command connect_device");
    state.devices.connect(role, device).await?;
    remember(&state, role).await;
    Ok(())
}

#[tauri::command]
pub fn known_devices(state: State<'_, AppState>) -> Result<Vec<KnownDevice>, String> {
    state.storage.known_devices()
}

#[tauri::command]
pub fn forget_device(state: State<'_, AppState>, id: String) -> Result<(), String> {
    tracing::info!(%id, "command forget_device");
    state.storage.forget_device(&id)
}

#[tauri::command]
pub fn forget_all_devices(state: State<'_, AppState>) -> Result<usize, String> {
    let removed = state.storage.forget_all_devices()?;
    tracing::info!(removed, "command forget_all_devices");
    Ok(removed)
}

#[tauri::command]
pub async fn disconnect_device(state: State<'_, AppState>, role: DeviceRole) -> Result<(), String> {
    tracing::debug!(?role, "command disconnect_device");
    state.devices.disconnect_role(role).await;
    Ok(())
}

#[tauri::command]
pub fn get_source_preferences(state: State<'_, AppState>) -> Result<SourcePreferences, String> {
    Ok(state.devices.source_preferences())
}

#[tauri::command]
pub fn set_source_preferences(
    state: State<'_, AppState>,
    preferences: SourcePreferences,
) -> Result<(), String> {
    tracing::debug!(?preferences, "command set_source_preferences");
    state.storage.save_source_preferences(&preferences)?;
    state.devices.set_source_preferences(preferences);
    Ok(())
}

#[tauri::command]
pub async fn device_log(
    state: State<'_, AppState>,
    role: DeviceRole,
) -> Result<Vec<DeviceLogLine>, String> {
    Ok(state.devices.slot(role).log_lines())
}

#[tauri::command]
pub fn get_profile(state: State<'_, AppState>) -> Result<Profile, String> {
    state.storage.profile()
}

#[tauri::command]
pub fn save_profile(state: State<'_, AppState>, profile: Profile) -> Result<(), String> {
    tracing::info!(
        ftp = profile.ftp_watts,
        max_power = profile.max_power_watts,
        "Saving profile"
    );
    state.storage.save_profile(&profile)
}

#[tauri::command]
pub fn list_workouts(state: State<'_, AppState>) -> Result<Vec<Workout>, String> {
    state.storage.workouts()
}

#[tauri::command]
pub fn get_workout(state: State<'_, AppState>, id: Uuid) -> Result<Option<Workout>, String> {
    state.storage.workout(id)
}

#[tauri::command]
pub fn save_workout(state: State<'_, AppState>, mut workout: Workout) -> Result<Workout, String> {
    workout.updated_at = Utc::now();
    if let Err(error) = state.storage.save_workout(&workout) {
        tracing::error!(
            workout_id = %workout.id,
            workout_name = %workout.name,
            error = %error,
            "Could not save workout"
        );
        return Err(error);
    }
    tracing::info!(workout_id = %workout.id, "Workout saved");
    Ok(workout)
}

#[tauri::command]
pub fn delete_workout(state: State<'_, AppState>, id: Uuid) -> Result<(), String> {
    tracing::info!(workout_id = %id, "Deleting workout");
    state.storage.delete_workout(id)
}

#[tauri::command]
pub fn import_zwo_workout(state: State<'_, AppState>, contents: String) -> Result<Workout, String> {
    let workout = import_zwo(&contents).map_err(|error| {
        tracing::warn!(error = %error, bytes = contents.len(), "ZWO import failed");
        error
    })?;
    tracing::info!(workout_id = %workout.id, workout = %workout.name, "ZWO imported");
    state.storage.save_workout(&workout)?;
    Ok(workout)
}

#[tauri::command]
pub fn export_zwo_workout(
    state: State<'_, AppState>,
    workout_id: Uuid,
    path: PathBuf,
) -> Result<(), String> {
    let workout = state
        .storage
        .workout(workout_id)?
        .ok_or_else(|| "Workout not found".to_string())?;
    let profile = state.storage.profile()?;
    fs::write(path, export_zwo(&workout, profile.ftp_watts))
        .map_err(|error| format!("Could not export workout: {error}"))
}

#[tauri::command]
pub async fn runner_state(state: State<'_, AppState>) -> Result<RunnerState, String> {
    Ok(state.runner.state().await)
}

#[tauri::command]
pub async fn start_workout(
    app: AppHandle,
    state: State<'_, AppState>,
    workout_id: Uuid,
) -> Result<Uuid, String> {
    tracing::debug!(workout_id = %workout_id, "command start_workout");
    let workout = state
        .storage
        .workout(workout_id)?
        .ok_or_else(|| "Workout not found".to_string())?;
    let profile = state.storage.profile()?;
    state
        .runner
        .start(
            app,
            workout,
            profile.ftp_watts,
            profile.max_power_watts,
            Arc::clone(&state.devices),
            Arc::clone(&state.storage),
        )
        .await
}

#[tauri::command]
pub async fn start_free_ride(app: AppHandle, state: State<'_, AppState>) -> Result<Uuid, String> {
    tracing::debug!("command start_free_ride");
    let profile = state.storage.profile()?;
    state
        .runner
        .start_free_ride(
            app,
            profile.max_power_watts,
            Arc::clone(&state.devices),
            Arc::clone(&state.storage),
        )
        .await
}

#[tauri::command]
pub async fn adjust_manual_power(
    app: AppHandle,
    state: State<'_, AppState>,
    delta: i16,
) -> Result<u16, String> {
    let profile = state.storage.profile()?;
    state
        .runner
        .adjust_manual_power(&app, delta, profile.max_power_watts, &state.devices)
        .await
}

#[tauri::command]
pub async fn set_manual_power(
    app: AppHandle,
    state: State<'_, AppState>,
    watts: u16,
) -> Result<u16, String> {
    let profile = state.storage.profile()?;
    state
        .runner
        .set_manual_power(&app, watts, profile.max_power_watts, &state.devices)
        .await
}

#[tauri::command]
pub async fn pause_or_resume_workout(state: State<'_, AppState>) -> Result<(), String> {
    state.runner.pause_or_resume(&state.devices).await
}

#[tauri::command]
pub fn skip_interval(state: State<'_, AppState>) -> Result<(), String> {
    state.runner.skip()
}

#[tauri::command]
pub async fn stop_workout(state: State<'_, AppState>) -> Result<(), String> {
    state.runner.stop(&state.devices).await
}

#[tauri::command]
pub fn list_sessions(state: State<'_, AppState>) -> Result<Vec<SessionSummary>, String> {
    state.storage.sessions()
}

#[tauri::command]
pub fn get_session(state: State<'_, AppState>, id: Uuid) -> Result<Option<SessionDetail>, String> {
    state.storage.session(id)
}

#[tauri::command]
pub fn export_session_csv(
    state: State<'_, AppState>,
    session_id: Uuid,
    path: PathBuf,
) -> Result<(), String> {
    let detail = state
        .storage
        .session(session_id)?
        .ok_or_else(|| "Ride not found".to_string())?;
    let mut writer = csv::Writer::from_path(path)
        .map_err(|error| format!("Could not create CSV file: {error}"))?;
    for sample in detail.samples {
        writer
            .serialize(sample)
            .map_err(|error| format!("Could not write CSV data: {error}"))?;
    }
    writer
        .flush()
        .map_err(|error| format!("Could not finish CSV export: {error}"))
}

#[tauri::command]
pub fn export_session_fit(
    state: State<'_, AppState>,
    session_id: Uuid,
    path: PathBuf,
) -> Result<(), String> {
    let source = ensure_session_fit(&state, session_id)?;
    if source == path {
        return Ok(());
    }
    fs::copy(&source, &path)
        .map(|_| ())
        .map_err(|error| format!("Could not export FIT file: {error}"))
}

#[tauri::command]
pub fn prepare_garmin_upload(
    state: State<'_, AppState>,
    session_id: Uuid,
) -> Result<String, String> {
    let path = ensure_session_fit(&state, session_id)?;
    tauri_plugin_opener::open_url(
        "https://connect.garmin.com/modern/import-data",
        None::<&str>,
    )
    .map_err(|error| format!("Could not open Garmin Connect: {error}"))?;
    tauri_plugin_opener::reveal_item_in_dir(&path)
        .map_err(|error| format!("Could not reveal FIT file: {error}"))?;
    Ok(path.display().to_string())
}

#[tauri::command]
pub fn get_ride_files_path(state: State<'_, AppState>) -> String {
    state.ride_files_dir.display().to_string()
}

#[tauri::command]
pub fn reveal_ride_files(state: State<'_, AppState>) -> Result<(), String> {
    tauri_plugin_opener::open_path(&state.ride_files_dir, None::<&str>)
        .map_err(|error| format!("Could not open Ride Files: {error}"))
}

fn ensure_session_fit(state: &AppState, session_id: Uuid) -> Result<PathBuf, String> {
    let detail = state
        .storage
        .session(session_id)?
        .ok_or_else(|| "Ride not found".to_string())?;
    ensure_ride_file(&state.ride_files_dir, &detail)
}

#[tauri::command]
pub fn get_log_file_path(state: State<'_, AppState>) -> String {
    state.log_path.display().to_string()
}

/// Reveal the log file in Finder / the system file manager.
#[tauri::command]
pub fn reveal_log_file(state: State<'_, AppState>) -> Result<(), String> {
    tracing::info!("Revealing log file");
    tauri_plugin_opener::reveal_item_in_dir(&state.log_path)
        .map_err(|error| format!("Could not open log location: {error}"))
}

#[tauri::command]
pub fn report_client_error(context: String, message: String) {
    tracing::error!(context = %context, error = %message, "Frontend operation failed");
}

/// Informational breadcrumbs from the UI (navigation, button presses) so the
/// log shows what the user did right before an error.
#[tauri::command]
pub fn report_client_event(context: String, message: String) {
    tracing::info!(context = %context, detail = %message, "Frontend event");
}
