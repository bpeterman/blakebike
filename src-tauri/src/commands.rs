use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering},
};

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
    intervals::{fetch_cycling_training_zones, fetch_estimated_ftp},
    runner::RunnerState,
    storage::{PowerSmoothing, RideDisplayPreferences, Storage, TrainingZoneSettings},
};

/// Run storage or file work on the blocking pool. Non-async commands run on
/// the main thread, which is also the UI thread; reading a long ride there
/// would freeze the window.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| format!("Background work failed: {error}"))?
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrainingSyncResult {
    profile: Profile,
    zones: TrainingZoneSettings,
    power_zones_imported: bool,
    heart_rate_zones_imported: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkoutExportResult {
    exported_count: usize,
    directory: String,
}

// Trainer-only commands kept for the pre-hub UI; they delegate to the hub.

#[tauri::command]
pub async fn device_state(state: State<'_, AppState>) -> Result<DeviceState, String> {
    Ok(state.devices.state().await)
}

#[tauri::command]
pub async fn scan_trainers(state: State<'_, AppState>) -> Result<Vec<DeviceInfo>, String> {
    tracing::debug!("command scan_trainers");
    let dev_mode = state.storage.dev_mode()?;
    state.devices.scan_trainers(dev_mode).await
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
    let dev_mode = state.storage.dev_mode()?;
    state.devices.scan(dev_mode).await
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
    let dev_mode = state.storage.dev_mode()?;
    Ok(state
        .storage
        .known_devices()?
        .into_iter()
        // A simulator remembered from a developer-mode session should not
        // offer itself once developer mode is off.
        .filter(|device| dev_mode || !device.simulated)
        .collect())
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
pub async fn calibrate_trainer(state: State<'_, AppState>) -> Result<(), String> {
    tracing::info!("command calibrate_trainer");
    state.devices.calibrate_trainer().await
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
pub fn intervals_api_key_configured(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state.storage.intervals_api_key()?.is_some())
}

#[tauri::command]
pub fn save_intervals_api_key(state: State<'_, AppState>, api_key: String) -> Result<(), String> {
    state.storage.save_intervals_api_key(&api_key)
}

#[tauri::command]
pub fn clear_intervals_api_key(state: State<'_, AppState>) -> Result<(), String> {
    state.storage.clear_intervals_api_key()
}

#[tauri::command]
pub async fn refresh_estimated_ftp(
    state: State<'_, AppState>,
) -> Result<TrainingSyncResult, String> {
    let api_key = state
        .storage
        .intervals_api_key()?
        .ok_or_else(|| "Save an Intervals.icu API key before refreshing FTP".to_string())?;
    let ftp = fetch_estimated_ftp(&api_key).await?;
    let mut profile = state.storage.profile()?;
    profile.ftp_watts = ftp;
    let mut zones = state.storage.training_zones()?;
    let mut power_zones_imported = false;
    let mut heart_rate_zones_imported = false;
    let mut power_zone_ftp = None;
    match fetch_cycling_training_zones(&api_key).await {
        Ok(Some(imported)) => {
            if let Some(max_hr) = imported.max_heart_rate_bpm {
                profile.max_heart_rate_bpm = max_hr;
            }
            if !imported.heart_rate_boundaries.is_empty() {
                let mut boundaries = imported.heart_rate_boundaries;
                if boundaries
                    .last()
                    .is_some_and(|bound| *bound >= profile.max_heart_rate_bpm)
                {
                    boundaries.pop();
                }
                zones.heart_rate_mode = crate::storage::ZoneMode::Custom;
                zones.heart_rate_zones = zone_definitions(&boundaries, &imported.heart_rate_names);
                heart_rate_zones_imported = true;
            }
            if zones.sync_power_zones_from_intervals
                && !imported.power_percent_boundaries.is_empty()
            {
                let zone_ftp = imported.cycling_ftp_watts.unwrap_or(ftp);
                let boundaries =
                    power_zone_boundaries(zone_ftp, &imported.power_percent_boundaries);
                zones.power_mode = crate::storage::ZoneMode::Custom;
                zones.power_zones = zone_definitions(&boundaries, &imported.power_names);
                power_zones_imported = true;
                power_zone_ftp = Some(zone_ftp);
            }
            state.storage.save_training_zones(&zones)?;
        }
        Ok(None) => {
            tracing::warn!("Intervals.icu has no cycling sport settings; keeping derived HR zones");
        }
        Err(error) => {
            tracing::warn!(%error, "Could not import Intervals.icu training zones; keeping current zones");
        }
    }
    state.storage.save_profile(&profile)?;
    tracing::info!(
        ftp,
        max_hr = profile.max_heart_rate_bpm,
        power_zone_ftp,
        power_zones_imported,
        heart_rate_zones_imported,
        "Updated training settings from Intervals.icu"
    );
    Ok(TrainingSyncResult {
        profile,
        zones,
        power_zones_imported,
        heart_rate_zones_imported,
    })
}

fn zone_definitions(boundaries: &[u16], names: &[String]) -> Vec<crate::storage::ZoneDefinition> {
    (0..=boundaries.len())
        .map(|index| crate::storage::ZoneDefinition {
            name: names
                .get(index)
                .filter(|name| !name.trim().is_empty())
                .cloned()
                .unwrap_or_else(|| format!("Zone {}", index + 1)),
            upper_bound: boundaries.get(index).copied(),
        })
        .collect()
}

fn power_zone_boundaries(ftp: u16, percentages: &[u16]) -> Vec<u16> {
    percentages
        .iter()
        .map(|percent| {
            ((u32::from(ftp) * u32::from(*percent)) / 100).min(u32::from(u16::MAX)) as u16
        })
        .collect()
}

#[cfg(test)]
mod training_sync_tests {
    use super::power_zone_boundaries;

    #[test]
    fn converts_intervals_power_zones_with_cycling_profile_ftp() {
        assert_eq!(
            power_zone_boundaries(280, &[55, 75, 90, 105, 120, 150]),
            vec![154, 210, 252, 294, 336, 420]
        );
    }
}

#[tauri::command]
pub async fn list_workouts(state: State<'_, AppState>) -> Result<Vec<Workout>, String> {
    let storage = Arc::clone(&state.storage);
    blocking(move || storage.workouts()).await
}

#[tauri::command]
pub async fn get_workout(state: State<'_, AppState>, id: Uuid) -> Result<Option<Workout>, String> {
    let storage = Arc::clone(&state.storage);
    blocking(move || storage.workout(id)).await
}

#[tauri::command]
pub async fn delete_workout(state: State<'_, AppState>, id: Uuid) -> Result<(), String> {
    tracing::info!(workout_id = %id, "Deleting workout");
    let storage = Arc::clone(&state.storage);
    blocking(move || storage.delete_workout(id)).await
}

#[tauri::command]
pub async fn save_workout(
    state: State<'_, AppState>,
    mut workout: Workout,
) -> Result<Workout, String> {
    workout.updated_at = Utc::now();
    let storage = Arc::clone(&state.storage);
    blocking(move || {
        if let Err(error) = storage.save_workout(&workout) {
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
    })
    .await
}

#[tauri::command]
pub async fn import_zwo_workout(
    state: State<'_, AppState>,
    contents: String,
) -> Result<Workout, String> {
    let storage = Arc::clone(&state.storage);
    blocking(move || {
        let workout = import_zwo(&contents).map_err(|error| {
            tracing::warn!(error = %error, bytes = contents.len(), "ZWO import failed");
            error
        })?;
        tracing::info!(workout_id = %workout.id, workout = %workout.name, "ZWO imported");
        storage.save_workout(&workout)?;
        Ok(workout)
    })
    .await
}

#[tauri::command]
pub async fn export_zwo_workout(
    state: State<'_, AppState>,
    workout_id: Uuid,
    path: PathBuf,
) -> Result<(), String> {
    let storage = Arc::clone(&state.storage);
    blocking(move || {
        let workout = storage
            .workout(workout_id)?
            .ok_or_else(|| "Workout not found".to_string())?;
        let profile = storage.profile()?;
        fs::write(path, export_zwo(&workout, profile.ftp_watts))
            .map_err(|error| format!("Could not export workout: {error}"))
    })
    .await
}

#[tauri::command]
pub async fn export_all_zwo_workouts(
    state: State<'_, AppState>,
    directory: PathBuf,
) -> Result<WorkoutExportResult, String> {
    let storage = Arc::clone(&state.storage);
    blocking(move || {
        let workouts = storage.workouts()?;
        let profile = storage.profile()?;
        let exported_count =
            export_workouts_to_directory(&directory, &workouts, profile.ftp_watts)?;
        tracing::info!(
            exported_count,
            directory = %directory.display(),
            "Workout library exported"
        );
        Ok(WorkoutExportResult {
            exported_count,
            directory: directory.display().to_string(),
        })
    })
    .await
}

fn export_workouts_to_directory(
    directory: &Path,
    workouts: &[Workout],
    ftp: u16,
) -> Result<usize, String> {
    if !directory.is_dir() {
        return Err("Choose an existing folder for the workout export".into());
    }

    let bases: Vec<String> = workouts
        .iter()
        .map(|workout| safe_workout_filename(&workout.name, workout.id))
        .collect();
    let mut totals = HashMap::<String, usize>::new();
    for base in &bases {
        *totals.entry(base.clone()).or_default() += 1;
    }

    for (workout, base) in workouts.iter().zip(bases) {
        let filename = if totals.get(&base).copied().unwrap_or_default() > 1 {
            format!("{base}-{}.zwo", short_id(workout.id))
        } else {
            format!("{base}.zwo")
        };
        let path = directory.join(filename);
        fs::write(&path, export_zwo(workout, ftp))
            .map_err(|error| format!("Could not export {}: {error}", workout.name))?;
    }
    Ok(workouts.len())
}

fn safe_workout_filename(name: &str, id: Uuid) -> String {
    let cleaned = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    let slug = cleaned
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        format!("workout-{}", short_id(id))
    } else {
        slug
    }
}

fn short_id(id: Uuid) -> String {
    id.simple().to_string()[..8].to_string()
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
            Some(app),
            workout,
            profile.ftp_watts,
            profile.max_power_watts,
            profile.rider_weight_kg + profile.bike_weight_kg,
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
            Some(app),
            profile.max_power_watts,
            profile.rider_weight_kg + profile.bike_weight_kg,
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
        .adjust_manual_power(Some(&app), delta, profile.max_power_watts, &state.devices)
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
        .set_manual_power(Some(&app), watts, profile.max_power_watts, &state.devices)
        .await
}

#[tauri::command]
pub async fn clear_target_override(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<u16>, String> {
    tracing::debug!("command clear_target_override");
    let profile = state.storage.profile()?;
    state
        .runner
        .clear_target_override(Some(&app), profile.max_power_watts, &state.devices)
        .await
}

#[tauri::command]
pub async fn set_bias_percent(
    app: AppHandle,
    state: State<'_, AppState>,
    percent: u16,
) -> Result<u16, String> {
    tracing::debug!(percent, "command set_bias_percent");
    let profile = state.storage.profile()?;
    state
        .runner
        .set_bias_percent(Some(&app), percent, profile.max_power_watts, &state.devices)
        .await
}

#[tauri::command]
pub fn get_power_smoothing(state: State<'_, AppState>) -> Result<PowerSmoothing, String> {
    state.storage.power_smoothing()
}

#[tauri::command]
pub fn set_power_smoothing(
    state: State<'_, AppState>,
    smoothing: PowerSmoothing,
) -> Result<(), String> {
    tracing::debug!(?smoothing, "command set_power_smoothing");
    state.storage.save_power_smoothing(smoothing)
}

#[tauri::command]
pub fn get_dev_mode(state: State<'_, AppState>) -> Result<bool, String> {
    state.storage.dev_mode()
}

#[tauri::command]
pub fn set_dev_mode(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    tracing::info!(enabled, "command set_dev_mode");
    state.storage.save_dev_mode(enabled)
}

#[tauri::command]
pub fn get_training_zones(state: State<'_, AppState>) -> Result<TrainingZoneSettings, String> {
    state.storage.training_zones()
}

#[tauri::command]
pub fn set_training_zones(
    state: State<'_, AppState>,
    zones: TrainingZoneSettings,
) -> Result<(), String> {
    state.storage.save_training_zones(&zones)
}

#[tauri::command]
pub fn get_ride_display_preferences(
    state: State<'_, AppState>,
) -> Result<RideDisplayPreferences, String> {
    state.storage.ride_display_preferences()
}

#[tauri::command]
pub fn set_ride_display_preferences(
    state: State<'_, AppState>,
    preferences: RideDisplayPreferences,
) -> Result<(), String> {
    state.storage.save_ride_display_preferences(&preferences)
}

#[tauri::command]
pub async fn pause_or_resume_workout(state: State<'_, AppState>) -> Result<(), String> {
    state.runner.pause_or_resume().await
}

#[tauri::command]
pub fn skip_interval(state: State<'_, AppState>) -> Result<(), String> {
    state.runner.skip()
}

#[tauri::command]
pub async fn stop_workout(state: State<'_, AppState>) -> Result<(), String> {
    state.runner.stop().await
}

#[tauri::command]
pub async fn list_sessions(state: State<'_, AppState>) -> Result<Vec<SessionSummary>, String> {
    let storage = Arc::clone(&state.storage);
    blocking(move || storage.sessions()).await
}

#[tauri::command]
pub async fn get_session(
    state: State<'_, AppState>,
    id: Uuid,
) -> Result<Option<SessionDetail>, String> {
    let storage = Arc::clone(&state.storage);
    blocking(move || storage.session(id)).await
}

#[tauri::command]
pub async fn export_session_csv(
    state: State<'_, AppState>,
    session_id: Uuid,
    path: PathBuf,
) -> Result<(), String> {
    let storage = Arc::clone(&state.storage);
    blocking(move || {
        let detail = storage
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
    })
    .await
}

#[tauri::command]
pub async fn export_session_fit(
    state: State<'_, AppState>,
    session_id: Uuid,
    path: PathBuf,
) -> Result<(), String> {
    let storage = Arc::clone(&state.storage);
    let ride_files_dir = state.ride_files_dir.clone();
    blocking(move || {
        let source = ensure_session_fit(&storage, &ride_files_dir, session_id)?;
        if source == path {
            return Ok(());
        }
        fs::copy(&source, &path)
            .map(|_| ())
            .map_err(|error| format!("Could not export FIT file: {error}"))
    })
    .await
}

#[tauri::command]
pub async fn prepare_garmin_upload(
    state: State<'_, AppState>,
    session_id: Uuid,
) -> Result<String, String> {
    let storage = Arc::clone(&state.storage);
    let ride_files_dir = state.ride_files_dir.clone();
    let path = blocking(move || ensure_session_fit(&storage, &ride_files_dir, session_id)).await?;
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

fn ensure_session_fit(
    storage: &Storage,
    ride_files_dir: &Path,
    session_id: Uuid,
) -> Result<PathBuf, String> {
    let detail = storage
        .session(session_id)?
        .ok_or_else(|| "Ride not found".to_string())?;
    ensure_ride_file(ride_files_dir, &detail)
}

#[tauri::command]
pub fn get_log_file_path(state: State<'_, AppState>) -> String {
    state.log_path().display().to_string()
}

/// Reveal the log file in Finder / the system file manager.
#[tauri::command]
pub fn reveal_log_file(state: State<'_, AppState>) -> Result<(), String> {
    tracing::info!("Revealing log file");
    let path = state.log_path();
    tauri_plugin_opener::reveal_item_in_dir(&path)
        .or_else(|_| tauri_plugin_opener::open_path(&state.log_dir, None::<&str>))
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

/// Make the simulated trainer fail on purpose (debug builds only), so link
/// loss and control failures can be rehearsed in the running app:
/// `failWrites` with a count, or `dropLink`.
#[tauri::command]
pub fn debug_inject_trainer_fault(
    state: State<'_, AppState>,
    kind: String,
    count: Option<u32>,
) -> Result<(), String> {
    if !cfg!(debug_assertions) {
        return Err("Fault injection is only available in debug builds".into());
    }
    let faults = state.devices.simulated_faults();
    match kind.as_str() {
        "failWrites" => faults
            .fail_writes
            .store(count.unwrap_or(1), Ordering::Relaxed),
        "writeDelayMs" => faults
            .write_delay_ms
            .store(u64::from(count.unwrap_or(0)), Ordering::Relaxed),
        "ackDelayMs" => faults
            .ack_delay_ms
            .store(u64::from(count.unwrap_or(0)), Ordering::Relaxed),
        "refuseWith" => faults
            .refuse_with
            .store(count.unwrap_or(4).min(255) as u8, Ordering::Relaxed),
        "failConnects" => faults
            .fail_connects
            .store(count.unwrap_or(1), Ordering::Relaxed),
        "dropLink" => faults.drop_link.notify_one(),
        other => return Err(format!("Unknown trainer fault: {other}")),
    }
    tracing::warn!(kind, count, "Injected simulated trainer fault");
    Ok(())
}

#[cfg(test)]
mod workout_export_tests {
    use super::*;
    use crate::domain::{PowerTarget, WorkoutStep};

    fn workout(id: &str, name: &str) -> Workout {
        let mut workout = Workout::new(
            name,
            vec![WorkoutStep::Steady {
                duration_seconds: 60,
                target: PowerTarget::PercentFtp(75),
            }],
        );
        workout.id = Uuid::parse_str(id).unwrap();
        workout
    }

    #[test]
    fn exports_every_workout_and_disambiguates_duplicate_names() {
        let directory = tempfile::tempdir().unwrap();
        let workouts = vec![
            workout("11111111-1111-4111-8111-111111111111", "Tempo / Ride"),
            workout("22222222-2222-4222-8222-222222222222", "Tempo / Ride"),
            workout("33333333-3333-4333-8333-333333333333", "Threshold"),
        ];

        assert_eq!(
            export_workouts_to_directory(directory.path(), &workouts, 200).unwrap(),
            3
        );
        assert!(directory.path().join("tempo-ride-11111111.zwo").is_file());
        assert!(directory.path().join("tempo-ride-22222222.zwo").is_file());
        assert!(directory.path().join("threshold.zwo").is_file());
    }

    #[test]
    fn gives_empty_names_a_safe_filename() {
        let id = Uuid::parse_str("44444444-4444-4444-8444-444444444444").unwrap();
        assert_eq!(safe_workout_filename("///", id), "workout-44444444");
    }

    #[test]
    fn rejects_a_destination_that_is_not_a_directory() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("not-a-folder");
        fs::write(&file, "blocked").unwrap();
        let error = export_workouts_to_directory(&file, &[], 200).unwrap_err();
        assert!(error.contains("existing folder"));
    }
}
