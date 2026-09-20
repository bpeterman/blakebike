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
        CalibrationRecord, DeviceInfo, DeviceLogLine, DeviceRole, DeviceState, DevicesSnapshot,
        KnownConnectOutcome, KnownConnectStatus, KnownDevice, SourcePreferences,
    },
    domain::{PlannedWorkout, Profile, SessionDetail, SessionSummary, Workout},
    fit::ensure_ride_file,
    formats::{export_zwo, import_zwo},
    intervals::{CyclingSettings, FtpSource, IntervalsClient, ZoneBoundaries},
    intervals_sync::{self, IntervalsStatus, IntervalsSyncReport, resolve_athlete},
    runner::RunnerState,
    storage::{
        IntervalsSyncSettings, PowerSmoothing, RideDisplayPreferences, Storage,
        TrainingZoneSettings, ZoneDefinition, ZoneMode, validate_zones,
    },
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

/// What one "Sync from Intervals.icu" did, item by item, so the UI can say
/// exactly which numbers moved and which zone sets were left alone and why.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrainingSyncResult {
    pub profile: Profile,
    pub zones: TrainingZoneSettings,
    pub ftp: FtpOutcome,
    /// `None` when Intervals.icu has no max HR; the local value is kept.
    pub max_heart_rate: Option<MaxHeartRateOutcome>,
    pub power_zones: ZoneSetOutcome,
    pub heart_rate_zones: ZoneSetOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FtpOutcome {
    pub watts: u16,
    pub previous_watts: u16,
    pub source: FtpSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaxHeartRateOutcome {
    pub bpm: u16,
    pub previous_bpm: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ZoneSetOutcome {
    /// The set was replaced with the Intervals.icu zones.
    Imported,
    /// The set already held exactly these Intervals.icu zones.
    Unchanged,
    /// The rider has not turned import on for this set.
    SyncOff,
    /// Intervals.icu has no zones configured for this set.
    NotConfigured,
    /// Intervals.icu has zones, but they are unusable.
    Invalid { reason: String },
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

/// Persist a just-connected device so it shows up under Known devices, and
/// hand the slot the calibration record kept for it so the card can say when
/// it was last zeroed. Never fails the connect: a storage hiccup is logged
/// and the ride goes on.
async fn remember(state: &State<'_, AppState>, role: DeviceRole) {
    let Some(device) = state.devices.remember(role).await else {
        return;
    };
    if let Err(error) = state.storage.remember_device(&device) {
        tracing::warn!(?role, error = %error, "Could not remember device");
    }
    match state.storage.known_devices() {
        Ok(known) => {
            let record = known
                .into_iter()
                .find(|candidate| candidate.id == device.id)
                .and_then(|candidate| candidate.last_calibration);
            state.devices.slot(role).record_calibration(record).await;
        }
        Err(error) => tracing::warn!(?role, error = %error, "Could not load calibration record"),
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

/// The remembered devices the rider can see right now. A simulator remembered
/// from a developer-mode session should not offer itself once developer mode
/// is off.
fn visible_known_devices(state: &State<'_, AppState>) -> Result<Vec<KnownDevice>, String> {
    let dev_mode = state.storage.dev_mode()?;
    Ok(state
        .storage
        .known_devices()?
        .into_iter()
        .filter(|device| dev_mode || !device.simulated)
        .collect())
}

#[tauri::command]
pub fn known_devices(state: State<'_, AppState>) -> Result<Vec<KnownDevice>, String> {
    visible_known_devices(&state)
}

/// "Connect all": bring back every remembered device whose role is free.
/// Succeeds even when some devices do not answer; each outcome is in the
/// result so the UI can stay calm about a sleeping sensor.
#[tauri::command]
pub async fn connect_known_devices(
    state: State<'_, AppState>,
) -> Result<Vec<KnownConnectOutcome>, String> {
    tracing::info!("command connect_known_devices");
    let known = visible_known_devices(&state)?;
    let outcomes = state.devices.connect_known(&known).await;
    for outcome in &outcomes {
        if outcome.status == KnownConnectStatus::Connected {
            remember(&state, outcome.role).await;
        }
    }
    Ok(outcomes)
}

#[tauri::command]
pub fn forget_device(state: State<'_, AppState>, id: String) -> Result<(), String> {
    tracing::info!(%id, "command forget_device");
    state.storage.forget_device(&id)
}

#[tauri::command]
pub fn forget_all_devices(state: State<'_, AppState>) -> Result<usize, String> {
    // Only forget what the rider can see: with developer mode off the
    // remembered simulators are hidden, so they must survive a "forget all"
    // that the undo could not put back.
    let removed = if state.storage.dev_mode()? {
        state.storage.forget_all_devices()?
    } else {
        let mut removed = 0;
        for device in state.storage.known_devices()? {
            if !device.simulated {
                state.storage.forget_device(&device.id)?;
                removed += 1;
            }
        }
        removed
    };
    tracing::info!(removed, "command forget_all_devices");
    Ok(removed)
}

#[tauri::command]
pub fn restore_known_devices(
    state: State<'_, AppState>,
    devices: Vec<KnownDevice>,
) -> Result<(), String> {
    for device in devices {
        state.storage.remember_device(&device)?;
        if let Some(record) = &device.last_calibration {
            state.storage.record_calibration(&device.id, record)?;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn disconnect_device(state: State<'_, AppState>, role: DeviceRole) -> Result<(), String> {
    tracing::debug!(?role, "command disconnect_device");
    state.devices.disconnect_role(role).await;
    Ok(())
}

/// Run the calibration a role's device offers (trainer spin-down, power
/// meter zero offset) and keep the result with the remembered device. The
/// record comes back so the dialog can show it without waiting on events.
#[tauri::command]
pub async fn calibrate_device(
    state: State<'_, AppState>,
    role: DeviceRole,
) -> Result<CalibrationRecord, String> {
    tracing::info!(?role, "command calibrate_device");
    let record = state.devices.calibrate(role).await?;
    if let Some(device) = state.devices.slot(role).state().await.device() {
        match state.storage.record_calibration(&device.id, &record) {
            Ok(true) => {}
            Ok(false) => {
                tracing::warn!(?role, id = %device.id, "Calibrated a device that is not remembered; record not kept")
            }
            Err(error) => {
                tracing::warn!(?role, error = %error, "Could not store calibration record")
            }
        }
    }
    Ok(record)
}

#[tauri::command]
pub fn cancel_calibration(state: State<'_, AppState>, role: DeviceRole) -> Result<(), String> {
    tracing::info!(?role, "command cancel_calibration");
    state.devices.cancel_calibration(role)
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
pub fn intervals_status(state: State<'_, AppState>) -> Result<IntervalsStatus, String> {
    intervals_sync::status(&state.storage)
}

/// Validate the key against Intervals.icu (which also tells us the athlete
/// id every later call needs), then store both.
#[tauri::command]
pub async fn save_intervals_api_key(
    state: State<'_, AppState>,
    api_key: String,
) -> Result<IntervalsStatus, String> {
    let athlete = intervals_sync::save_api_key(&state.storage, &api_key).await?;
    tracing::info!(athlete = %athlete.id, "Intervals.icu API key saved");
    intervals_sync::status(&state.storage)
}

/// Forget the key and everything that depends on it: athlete, sync state,
/// the calendar cache and the mirrored library workouts.
#[tauri::command]
pub fn clear_intervals_api_key(state: State<'_, AppState>) -> Result<IntervalsStatus, String> {
    let removed = state.storage.clear_intervals()?;
    tracing::info!(
        removed_mirrored_workouts = removed,
        "Intervals.icu key cleared"
    );
    intervals_sync::status(&state.storage)
}

#[tauri::command]
pub fn set_intervals_sync_settings(
    state: State<'_, AppState>,
    settings: IntervalsSyncSettings,
) -> Result<IntervalsStatus, String> {
    tracing::info!(?settings, "command set_intervals_sync_settings");
    intervals_sync::update_settings(&state.storage, settings)
}

/// Refresh the calendar cache and the mirrored library, whichever are on.
#[tauri::command]
pub async fn sync_intervals(state: State<'_, AppState>) -> Result<IntervalsSyncReport, String> {
    tracing::info!("command sync_intervals");
    intervals_sync::sync_intervals(&state.storage).await
}

#[tauri::command]
pub async fn planned_workouts(state: State<'_, AppState>) -> Result<Vec<PlannedWorkout>, String> {
    let storage = Arc::clone(&state.storage);
    blocking(move || storage.planned_workouts()).await
}

/// Pull FTP, max HR and (when the rider has asked for them) the power and
/// heart-rate zones from the athlete's Intervals.icu cycling settings. One
/// request; if it fails nothing is written.
#[tauri::command]
pub async fn sync_training_settings(
    state: State<'_, AppState>,
) -> Result<TrainingSyncResult, String> {
    let api_key = state
        .storage
        .intervals_api_key()?
        .ok_or_else(|| "Save an Intervals.icu API key before syncing".to_string())?;
    let client = IntervalsClient::new(&api_key)?;
    let athlete = resolve_athlete(&state.storage, &client).await?;
    let settings = client.cycling_settings(&athlete.id).await?.ok_or_else(|| {
        "Intervals.icu has no cycling (Ride) settings for this athlete".to_string()
    })?;
    let profile = state.storage.profile()?;
    let zones = state.storage.training_zones()?;
    // Only the eFTP costs a second request, so only fetch it when it is what
    // the rider asked for.
    let preferred = state.storage.intervals_sync_state()?.settings.ftp_source;
    let estimated = if preferred == FtpSource::EstimatedFtp {
        client.estimated_ftp(&athlete.id).await?
    } else {
        None
    };
    let result = apply_cycling_settings(profile, zones, settings, preferred, estimated)?;
    state
        .storage
        .save_training_settings(&result.profile, &result.zones)?;
    tracing::info!(
        ftp = result.ftp.watts,
        ftp_source = ?result.ftp.source,
        max_hr = result.profile.max_heart_rate_bpm,
        power_zones = ?result.power_zones,
        heart_rate_zones = ?result.heart_rate_zones,
        "Synced training settings from Intervals.icu"
    );
    Ok(result)
}

/// Fold fetched cycling settings into the rider's profile and zones. Pure, so
/// the rules are testable without a server: FTP and max HR always update,
/// each zone set only when its toggle is on, and an imported set is marked
/// `ZoneMode::Intervals` so a hand-edited (`Custom`) set stays recognizable.
/// `preferred` is the rider's FTP source; `estimated` the modeled eFTP when
/// it was fetched.
fn apply_cycling_settings(
    mut profile: Profile,
    mut zones: TrainingZoneSettings,
    settings: CyclingSettings,
    preferred: FtpSource,
    estimated: Option<u16>,
) -> Result<TrainingSyncResult, String> {
    let ftp = settings.resolve_ftp(preferred, estimated).ok_or_else(|| {
        "Intervals.icu has no FTP between 50 and 500 W for this athlete".to_string()
    })?;
    let ftp_outcome = FtpOutcome {
        watts: ftp.watts,
        previous_watts: profile.ftp_watts,
        source: ftp.source,
    };
    profile.ftp_watts = ftp.watts;
    let max_heart_rate = settings.max_heart_rate_bpm.map(|bpm| {
        let previous_bpm = profile.max_heart_rate_bpm;
        profile.max_heart_rate_bpm = bpm;
        MaxHeartRateOutcome { bpm, previous_bpm }
    });
    let max_hr = profile.max_heart_rate_bpm;
    let heart_rate_zones = import_zone_set(
        zones.sync_heart_rate_zones_from_intervals,
        settings.heart_rate_zones,
        |set| {
            // Intervals.icu closes the top HR zone at max HR; ours is open-ended.
            let mut boundaries = set.boundaries;
            if boundaries.last().is_some_and(|bound| *bound >= max_hr) {
                boundaries.pop();
            }
            zone_definitions(&boundaries, &set.names)
        },
        |definitions| validate_zones("heart-rate", ZoneMode::Intervals, definitions, 30, 250),
        &mut zones.heart_rate_mode,
        &mut zones.heart_rate_zones,
    );
    let power_zones = import_zone_set(
        zones.sync_power_zones_from_intervals,
        settings.power_zones,
        |set| {
            zone_definitions(
                &power_zone_boundaries(ftp.watts, &set.boundaries),
                &set.names,
            )
        },
        |definitions| validate_zones("power", ZoneMode::Intervals, definitions, 1, 3_000),
        &mut zones.power_mode,
        &mut zones.power_zones,
    );
    Ok(TrainingSyncResult {
        profile,
        zones,
        ftp: ftp_outcome,
        max_heart_rate,
        power_zones,
        heart_rate_zones,
    })
}

/// `validate` is the same check storage applies on save, run here so an
/// unusable import is reported as that set's outcome instead of failing the
/// whole sync after the other items were already decided.
fn import_zone_set(
    enabled: bool,
    fetched: Result<ZoneBoundaries, String>,
    definitions: impl FnOnce(ZoneBoundaries) -> Vec<ZoneDefinition>,
    validate: impl FnOnce(&[ZoneDefinition]) -> Result<(), String>,
    mode: &mut ZoneMode,
    current: &mut Vec<ZoneDefinition>,
) -> ZoneSetOutcome {
    if !enabled {
        return ZoneSetOutcome::SyncOff;
    }
    let set = match fetched {
        Ok(set) => set,
        Err(reason) => return ZoneSetOutcome::Invalid { reason },
    };
    if set.boundaries.is_empty() {
        return ZoneSetOutcome::NotConfigured;
    }
    let imported = definitions(set);
    if let Err(reason) = validate(&imported) {
        return ZoneSetOutcome::Invalid {
            reason: format!("Intervals.icu zones are unusable: {reason}"),
        };
    }
    if *mode == ZoneMode::Intervals && *current == imported {
        return ZoneSetOutcome::Unchanged;
    }
    *mode = ZoneMode::Intervals;
    *current = imported;
    ZoneSetOutcome::Imported
}

fn zone_definitions(boundaries: &[u16], names: &[String]) -> Vec<ZoneDefinition> {
    (0..=boundaries.len())
        .map(|index| ZoneDefinition {
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
    use super::*;

    /// `apply_cycling_settings` with the default FTP preference (indoor) and
    /// no eFTP fetched, which is what most of these cases are about.
    fn applied(
        profile: Profile,
        zones: TrainingZoneSettings,
        settings: CyclingSettings,
    ) -> Result<TrainingSyncResult, String> {
        apply_cycling_settings(profile, zones, settings, FtpSource::default(), None)
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| (*name).to_string()).collect()
    }

    fn settings() -> CyclingSettings {
        CyclingSettings {
            indoor_ftp: Some(280),
            ftp: Some(305),
            max_heart_rate_bpm: Some(192),
            heart_rate_zones: Ok(ZoneBoundaries {
                boundaries: vec![120, 145, 166, 182, 192],
                names: names(&["Recovery", "Endurance", "Tempo", "Threshold", "Max"]),
            }),
            power_zones: Ok(ZoneBoundaries {
                boundaries: vec![55, 75, 90, 105, 120, 150],
                names: names(&[
                    "Recovery",
                    "Endurance",
                    "Tempo",
                    "Threshold",
                    "VO2",
                    "Anaerobic",
                    "Neuro",
                ]),
            }),
        }
    }

    fn both_on() -> TrainingZoneSettings {
        TrainingZoneSettings {
            sync_power_zones_from_intervals: true,
            sync_heart_rate_zones_from_intervals: true,
            ..TrainingZoneSettings::default()
        }
    }

    fn bounds(zones: &[ZoneDefinition]) -> Vec<Option<u16>> {
        zones.iter().map(|zone| zone.upper_bound).collect()
    }

    #[test]
    fn converts_intervals_power_zones_with_cycling_profile_ftp() {
        assert_eq!(
            power_zone_boundaries(280, &[55, 75, 90, 105, 120, 150]),
            vec![154, 210, 252, 294, 336, 420]
        );
    }

    #[test]
    fn toggles_off_update_anchors_and_leave_zones_alone() {
        let profile = Profile {
            ftp_watts: 250,
            max_heart_rate_bpm: 185,
            ..Profile::default()
        };
        let result = applied(profile, TrainingZoneSettings::default(), settings()).unwrap();
        assert_eq!(
            result.ftp,
            FtpOutcome {
                watts: 280,
                previous_watts: 250,
                source: FtpSource::IndoorFtp
            }
        );
        assert_eq!(result.profile.ftp_watts, 280);
        assert_eq!(
            result.max_heart_rate,
            Some(MaxHeartRateOutcome {
                bpm: 192,
                previous_bpm: 185
            })
        );
        assert_eq!(result.profile.max_heart_rate_bpm, 192);
        assert_eq!(result.power_zones, ZoneSetOutcome::SyncOff);
        assert_eq!(result.heart_rate_zones, ZoneSetOutcome::SyncOff);
        assert_eq!(result.zones, TrainingZoneSettings::default());
    }

    #[test]
    fn the_rider_preference_chooses_which_ftp_the_sync_uses() {
        for (preferred, estimated, expected) in [
            (FtpSource::IndoorFtp, None, (280, FtpSource::IndoorFtp)),
            (FtpSource::Ftp, None, (305, FtpSource::Ftp)),
            (
                FtpSource::EstimatedFtp,
                Some(318),
                (318, FtpSource::EstimatedFtp),
            ),
            // No eFTP modeled yet: the sync still lands a value and says so.
            (FtpSource::EstimatedFtp, None, (280, FtpSource::IndoorFtp)),
        ] {
            let result = apply_cycling_settings(
                Profile::default(),
                both_on(),
                settings(),
                preferred,
                estimated,
            )
            .unwrap();
            assert_eq!(result.ftp.watts, expected.0);
            assert_eq!(result.ftp.source, expected.1);
            assert_eq!(result.profile.ftp_watts, expected.0);
            // Power zones scale from whichever FTP was used.
            assert_eq!(
                result.zones.power_zones[0].upper_bound,
                Some(expected.0 * 55 / 100)
            );
        }
    }

    #[test]
    fn toggles_on_import_both_sets_scaled_from_the_chosen_ftp() {
        let result = applied(Profile::default(), both_on(), settings()).unwrap();
        assert_eq!(result.power_zones, ZoneSetOutcome::Imported);
        assert_eq!(result.heart_rate_zones, ZoneSetOutcome::Imported);
        assert_eq!(result.zones.power_mode, ZoneMode::Intervals);
        assert_eq!(result.zones.heart_rate_mode, ZoneMode::Intervals);
        assert_eq!(
            bounds(&result.zones.power_zones),
            vec![
                Some(154),
                Some(210),
                Some(252),
                Some(294),
                Some(336),
                Some(420),
                None
            ]
        );
        assert_eq!(result.zones.power_zones[6].name, "Neuro");
        // The 192 bpm top boundary equals max HR and becomes the open-ended zone.
        assert_eq!(
            bounds(&result.zones.heart_rate_zones),
            vec![Some(120), Some(145), Some(166), Some(182), None]
        );
        assert_eq!(result.zones.heart_rate_zones[4].name, "Max");
        result.zones.validate().unwrap();
    }

    #[test]
    fn identical_imported_zones_are_unchanged_but_identical_custom_zones_are_imported() {
        let first = applied(Profile::default(), both_on(), settings()).unwrap();
        let again = applied(first.profile.clone(), first.zones.clone(), settings()).unwrap();
        assert_eq!(again.power_zones, ZoneSetOutcome::Unchanged);
        assert_eq!(again.heart_rate_zones, ZoneSetOutcome::Unchanged);
        assert_eq!(again.ftp.previous_watts, 280);

        let mut custom = first.zones.clone();
        custom.power_mode = ZoneMode::Custom;
        let flipped = applied(first.profile, custom, settings()).unwrap();
        assert_eq!(flipped.power_zones, ZoneSetOutcome::Imported);
        assert_eq!(flipped.zones.power_mode, ZoneMode::Intervals);
    }

    #[test]
    fn empty_and_invalid_sets_report_without_spoiling_the_rest() {
        let mut fetched = settings();
        fetched.heart_rate_zones = Ok(ZoneBoundaries::default());
        fetched.power_zones = Err("Intervals.icu returned power zones that do not increase".into());
        let result = applied(Profile::default(), both_on(), fetched).unwrap();
        assert_eq!(result.heart_rate_zones, ZoneSetOutcome::NotConfigured);
        assert_eq!(
            result.power_zones,
            ZoneSetOutcome::Invalid {
                reason: "Intervals.icu returned power zones that do not increase".into()
            }
        );
        assert_eq!(result.profile.ftp_watts, 280);
        assert_eq!(result.zones.power_mode, ZoneMode::Derived);
        assert_eq!(result.zones.heart_rate_mode, ZoneMode::Derived);

        // A lone HR boundary at max HR would leave one zone: invalid, not imported.
        let mut lone = settings();
        lone.heart_rate_zones = Ok(ZoneBoundaries {
            boundaries: vec![192],
            names: Vec::new(),
        });
        let result = applied(Profile::default(), both_on(), lone).unwrap();
        assert!(matches!(
            result.heart_rate_zones,
            ZoneSetOutcome::Invalid { .. }
        ));
        assert_eq!(result.power_zones, ZoneSetOutcome::Imported);

        // Power percentages that collapse to the same watt at a low FTP fail
        // storage's validation; that is this set's outcome, not a command error.
        let mut collapsing = settings();
        collapsing.indoor_ftp = Some(50);
        collapsing.power_zones = Ok(ZoneBoundaries {
            boundaries: vec![55, 56, 57, 90],
            names: Vec::new(),
        });
        let result = applied(Profile::default(), both_on(), collapsing).unwrap();
        assert!(matches!(result.power_zones, ZoneSetOutcome::Invalid { .. }));
        assert_eq!(result.heart_rate_zones, ZoneSetOutcome::Imported);
        assert_eq!(result.profile.ftp_watts, 50);
        result.zones.validate().unwrap();
    }

    #[test]
    fn drops_a_top_hr_boundary_above_max_hr_too() {
        let mut fetched = settings();
        fetched.heart_rate_zones = Ok(ZoneBoundaries {
            boundaries: vec![120, 145, 166, 182, 200],
            names: Vec::new(),
        });
        let result = applied(Profile::default(), both_on(), fetched).unwrap();
        assert_eq!(
            bounds(&result.zones.heart_rate_zones),
            vec![Some(120), Some(145), Some(166), Some(182), None]
        );
    }

    #[test]
    fn serializes_the_shape_the_frontend_types_mirror() {
        let mut fetched = settings();
        fetched.heart_rate_zones = Err("bad".into());
        fetched.max_heart_rate_bpm = None;
        let result = applied(Profile::default(), both_on(), fetched).unwrap();
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["ftp"]["source"], "indoorFtp");
        assert_eq!(json["ftp"]["previousWatts"], 200);
        assert_eq!(json["maxHeartRate"], serde_json::Value::Null);
        assert_eq!(
            json["powerZones"],
            serde_json::json!({ "status": "imported" })
        );
        assert_eq!(
            json["heartRateZones"],
            serde_json::json!({ "status": "invalid", "reason": "bad" })
        );
        assert_eq!(json["zones"]["powerMode"], "intervals");
        assert_eq!(json["zones"]["syncHeartRateZonesFromIntervals"], true);

        let off = applied(
            Profile::default(),
            TrainingZoneSettings::default(),
            settings(),
        )
        .unwrap();
        let json = serde_json::to_value(&off).unwrap();
        assert_eq!(
            json["powerZones"],
            serde_json::json!({ "status": "syncOff" })
        );
        assert_eq!(json["maxHeartRate"]["previousBpm"], 190);
    }

    #[test]
    fn missing_max_hr_keeps_the_local_value() {
        let mut fetched = settings();
        fetched.max_heart_rate_bpm = None;
        fetched.heart_rate_zones = Ok(ZoneBoundaries::default());
        let profile = Profile {
            max_heart_rate_bpm: 177,
            ..Profile::default()
        };
        let result = applied(profile, both_on(), fetched).unwrap();
        assert_eq!(result.max_heart_rate, None);
        assert_eq!(result.profile.max_heart_rate_bpm, 177);
    }

    #[test]
    fn no_usable_ftp_is_an_error_before_anything_changes() {
        let mut fetched = settings();
        fetched.indoor_ftp = None;
        fetched.ftp = None;
        let error = applied(Profile::default(), both_on(), fetched).unwrap_err();
        assert!(error.contains("no FTP"));
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
    blocking(move || {
        intervals_sync::ensure_editable(&storage, id, None)?;
        storage.delete_workout(id)
    })
    .await
}

#[tauri::command]
pub async fn save_workout(
    state: State<'_, AppState>,
    mut workout: Workout,
) -> Result<Workout, String> {
    workout.updated_at = Utc::now();
    let storage = Arc::clone(&state.storage);
    blocking(move || {
        intervals_sync::ensure_editable(&storage, workout.id, Some(&workout))?;
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
        .rideable_workout(workout_id)?
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

/// Make a simulated device fail on purpose (debug builds only), so link loss,
/// control failures and calibration refusals can be rehearsed in the running
/// app. Trainer: `failWrites`, `writeDelayMs`, `ackDelayMs`, `refuseWith`,
/// `failConnects`, `dropLink`. Sensors (power meter, heart rate, cadence):
/// `refuseWith` (Cycling Power result code, 4 = operation failed) and
/// `ackDelayMs` for the next procedure. `role` defaults to the trainer.
#[tauri::command]
pub fn debug_inject_device_fault(
    state: State<'_, AppState>,
    role: Option<DeviceRole>,
    kind: String,
    count: Option<u32>,
) -> Result<(), String> {
    if !cfg!(debug_assertions) {
        return Err("Fault injection is only available in debug builds".into());
    }
    let role = role.unwrap_or(DeviceRole::Trainer);
    match role {
        DeviceRole::Trainer => {
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
        }
        sensor => {
            let faults = state
                .devices
                .simulated_sensor_faults(sensor)
                .ok_or_else(|| format!("{} has no simulated faults", sensor.label()))?;
            match kind.as_str() {
                "refuseWith" => faults
                    .refuse_with
                    .store(count.unwrap_or(4).min(255) as u8, Ordering::Relaxed),
                "ackDelayMs" => faults
                    .ack_delay_ms
                    .store(u64::from(count.unwrap_or(0)), Ordering::Relaxed),
                other => return Err(format!("Unknown sensor fault: {other}")),
            }
        }
    }
    tracing::warn!(?role, kind, count, "Injected simulated device fault");
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
