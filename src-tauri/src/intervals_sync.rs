//! Keeping the local mirrors of Intervals.icu current: the planned-workout
//! calendar cache and the library workouts. Sits between `IntervalsClient`
//! (network) and `Storage` (disk); the commands are thin wrappers over it.
//!
//! Both mirrors are read-only copies, safe to refresh on every launch. The
//! training-settings sync is deliberately not part of this: it rewrites the
//! rider's FTP and zones and stays behind its own button.

use chrono::{DateTime, Duration, Local, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    domain::{Workout, WorkoutOrigin},
    formats::import_zwo,
    intervals::{IntervalsAthlete, IntervalsClient, IntervalsError},
    storage::{IntervalsSyncSettings, Storage},
};

/// Days after today the calendar cache covers, so a day rollover while
/// offline still finds the next plan.
pub const CALENDAR_DAYS_AHEAD: i64 = 6;

pub const MIRRORED_WORKOUT_MESSAGE: &str =
    "This workout is mirrored from Intervals.icu; edit a copy or turn off library sync";

/// What the Settings card and the home screen show about the connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntervalsStatus {
    /// An API key is saved.
    pub configured: bool,
    pub athlete_id: Option<String>,
    pub athlete_name: Option<String>,
    pub settings: IntervalsSyncSettings,
    pub last_synced_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

pub fn status(storage: &Storage) -> Result<IntervalsStatus, String> {
    let athlete = storage.intervals_athlete()?;
    let sync = storage.intervals_sync_state()?;
    Ok(IntervalsStatus {
        configured: storage.intervals_api_key()?.is_some(),
        athlete_id: athlete.as_ref().map(|athlete| athlete.id.clone()),
        athlete_name: athlete.and_then(|athlete| athlete.name),
        settings: sync.settings,
        last_synced_at: sync.last_synced_at,
        last_error: sync.last_error,
    })
}

/// Change which mirrors are kept. Turning a mirror off removes its copies at
/// once, so nothing read-only lingers that the sync will no longer refresh;
/// the toggle copy promises exactly that.
pub fn update_settings(
    storage: &Storage,
    settings: IntervalsSyncSettings,
) -> Result<IntervalsStatus, String> {
    let mut state = storage.intervals_sync_state()?;
    let previous = state.settings;
    state.settings = settings;
    storage.save_intervals_sync_state(&state)?;
    if previous.library && !settings.library {
        let removed = storage.delete_mirrored_workouts_not_in(&[])?;
        tracing::info!(
            removed,
            "Library mirror turned off; mirrored workouts removed"
        );
    }
    if previous.calendar && !settings.calendar {
        storage.clear_planned_workouts()?;
        tracing::info!("Calendar mirror turned off; planned workouts cleared");
    }
    status(storage)
}

/// Mirrored workouts are refreshed by the sync and never edited or deleted
/// locally. `incoming` is the payload being saved, if any, so a client cannot
/// smuggle a mirror in by hand either.
pub fn ensure_editable(
    storage: &Storage,
    id: Uuid,
    incoming: Option<&Workout>,
) -> Result<(), String> {
    if incoming.is_some_and(Workout::is_mirrored)
        || storage
            .workout(id)?
            .is_some_and(|stored| stored.is_mirrored())
    {
        return Err(MIRRORED_WORKOUT_MESSAGE.into());
    }
    Ok(())
}

/// The athlete id every call needs. Stored → use it. Key but no athlete
/// (saved before ids were stored) → fetch once and keep it. No key → error.
pub async fn resolve_athlete(
    storage: &Storage,
    client: &IntervalsClient,
) -> Result<IntervalsAthlete, String> {
    if let Some(athlete) = storage.intervals_athlete()? {
        return Ok(athlete);
    }
    let athlete = client.athlete().await?;
    storage.save_intervals_athlete(&athlete)?;
    tracing::info!(athlete = %athlete.id, "Resolved Intervals.icu athlete for an existing key");
    Ok(athlete)
}

/// Validate a key by asking who it belongs to, then store both. A network
/// failure says so and leaves the old key alone, so an offline rider does not
/// conclude the key is wrong.
pub async fn save_api_key(storage: &Storage, api_key: &str) -> Result<IntervalsAthlete, String> {
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return Err("Enter an Intervals.icu API key".into());
    }
    save_api_key_with(storage, api_key, &IntervalsClient::new(api_key)?).await
}

async fn save_api_key_with(
    storage: &Storage,
    api_key: &str,
    client: &IntervalsClient,
) -> Result<IntervalsAthlete, String> {
    let athlete = client.athlete().await.map_err(|error| match error {
        IntervalsError::Network(_) => format!("{error}; the key was not saved"),
        other => other.to_string(),
    })?;
    storage.save_intervals_api_key(api_key)?;
    storage.save_intervals_athlete(&athlete)?;
    Ok(athlete)
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarSyncReport {
    /// Cycling workouts in the fetched range.
    pub fetched: usize,
    /// Of those, planned without structure (free-form or notes only).
    pub unstructured: usize,
    /// Of those, with structure Intervals.icu sent but we could not read.
    pub failed: usize,
    /// Events skipped because they were not cycling workouts.
    pub skipped: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibrarySyncReport {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub unchanged: usize,
    /// Library workouts skipped because they were not cycling workouts.
    pub skipped: usize,
    /// "name: reason" for workouts whose structure could not be fetched or read.
    pub failed: Vec<String>,
}

/// One half of a sync: not enabled, done with its report, or failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum SyncOutcome<T> {
    Off,
    Done { report: T },
    Failed { error: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntervalsSyncReport {
    pub synced_at: DateTime<Utc>,
    pub calendar: SyncOutcome<CalendarSyncReport>,
    pub library: SyncOutcome<LibrarySyncReport>,
}

/// Refresh the enabled mirrors. A failure in one half does not stop the
/// other; the state on disk records the time of the attempt and the first
/// error, which the status shows until the next successful sync.
pub async fn sync_intervals(storage: &Storage) -> Result<IntervalsSyncReport, String> {
    let api_key = storage
        .intervals_api_key()?
        .ok_or_else(|| "Save an Intervals.icu API key first".to_string())?;
    sync_with_client(storage, &IntervalsClient::new(&api_key)?).await
}

async fn sync_with_client(
    storage: &Storage,
    client: &IntervalsClient,
) -> Result<IntervalsSyncReport, String> {
    let mut state = storage.intervals_sync_state()?;
    let athlete = match resolve_athlete(storage, client).await {
        Ok(athlete) => athlete,
        Err(error) => {
            state.last_error = Some(error.clone());
            storage.save_intervals_sync_state(&state)?;
            return Err(error);
        }
    };
    let calendar = if state.settings.calendar {
        match sync_calendar(storage, client, &athlete.id).await {
            Ok(report) => SyncOutcome::Done { report },
            Err(error) => SyncOutcome::Failed { error },
        }
    } else {
        SyncOutcome::Off
    };
    let library = if state.settings.library {
        match sync_library(storage, client, &athlete.id).await {
            Ok(report) => SyncOutcome::Done { report },
            Err(error) => SyncOutcome::Failed { error },
        }
    } else {
        SyncOutcome::Off
    };
    let synced_at = Utc::now();
    let first_error = [outcome_error(&calendar), outcome_error(&library)]
        .into_iter()
        .flatten()
        .next();
    if matches!(calendar, SyncOutcome::Done { .. }) || matches!(library, SyncOutcome::Done { .. }) {
        state.last_synced_at = Some(synced_at);
    }
    state.last_error = first_error;
    storage.save_intervals_sync_state(&state)?;
    tracing::info!(?calendar, ?library, "Intervals.icu mirrors synced");
    Ok(IntervalsSyncReport {
        synced_at,
        calendar,
        library,
    })
}

fn outcome_error<T>(outcome: &SyncOutcome<T>) -> Option<String> {
    match outcome {
        SyncOutcome::Failed { error } => Some(error.clone()),
        _ => None,
    }
}

async fn sync_calendar(
    storage: &Storage,
    client: &IntervalsClient,
    athlete_id: &str,
) -> Result<CalendarSyncReport, String> {
    let today = Local::now().date_naive();
    let fetch = client
        .planned_workouts(
            athlete_id,
            today,
            today + Duration::days(CALENDAR_DAYS_AHEAD),
        )
        .await?;
    let report = CalendarSyncReport {
        fetched: fetch.planned.len(),
        unstructured: fetch
            .planned
            .iter()
            .filter(|planned| planned.workout.is_none() && planned.parse_error.is_none())
            .count(),
        failed: fetch
            .planned
            .iter()
            .filter(|planned| planned.parse_error.is_some())
            .count(),
        skipped: fetch.skipped,
    };
    storage.replace_planned_workouts(&fetch.planned)?;
    Ok(report)
}

/// Mirror the cycling library: one listing, then a ZWO fetch only for
/// workouts that are new or changed (by `updated` or folder), and removal of
/// mirrors whose source is gone. Local workouts are never touched.
async fn sync_library(
    storage: &Storage,
    client: &IntervalsClient,
    athlete_id: &str,
) -> Result<LibrarySyncReport, String> {
    let fetch = client.library(athlete_id).await?;
    let mut report = LibrarySyncReport {
        skipped: fetch.skipped,
        ..LibrarySyncReport::default()
    };
    let mut keep = Vec::with_capacity(fetch.entries.len());
    for entry in &fetch.entries {
        let origin = WorkoutOrigin {
            external_id: entry.external_id,
            folder_id: entry.folder_id,
            folder: entry.folder.clone(),
            updated: entry.updated.clone(),
            planned_load: entry.planned_load,
        };
        let key = origin.external_key();
        let existing = storage.workout_by_external_key(&key)?;
        if existing.as_ref().is_some_and(|workout| {
            workout.origin.as_ref() == Some(&origin) && workout.name == entry.name
        }) {
            keep.push(key);
            report.unchanged += 1;
            continue;
        }
        let structure = match client.workout_zwo(athlete_id, entry).await {
            Ok(zwo) => import_zwo(&zwo),
            Err(error) => Err(error.to_string()),
        };
        let parsed = match structure {
            Ok(parsed) => parsed,
            Err(error) => {
                report.failed.push(format!("{}: {error}", entry.name));
                // Keep the previous copy rather than losing a workout over a
                // transient failure; it is refreshed next time.
                if existing.is_some() {
                    keep.push(key);
                }
                continue;
            }
        };
        let now = Utc::now();
        let workout = Workout {
            id: existing
                .as_ref()
                .map_or_else(Uuid::new_v4, |workout| workout.id),
            name: entry.name.clone(),
            description: if parsed.description.trim().is_empty() {
                entry.description.clone()
            } else {
                parsed.description
            },
            source: "intervals".into(),
            version: 1,
            steps: parsed.steps,
            created_at: existing.as_ref().map_or(now, |workout| workout.created_at),
            updated_at: now,
            origin: Some(origin),
        };
        storage.save_workout(&workout)?;
        keep.push(key);
        if existing.is_some() {
            report.updated += 1;
        } else {
            report.added += 1;
        }
    }
    report.removed = storage.delete_mirrored_workouts_not_in(&keep)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        intervals::test_support::{Route, json, serve_routes},
        storage::IntervalsSyncState,
    };
    use base64::{Engine, engine::general_purpose::STANDARD as BASE64};

    const ZWO: &str = r#"<workout_file><name>Sweet Spot</name><description>3x12</description><workout>
      <Warmup Duration="600" PowerLow="0.5" PowerHigh="0.7"/>
      <IntervalsT Repeat="3" OnDuration="720" OffDuration="240" OnPower="0.9" OffPower="0.55"/>
    </workout></workout_file>"#;

    fn today() -> String {
        Local::now().date_naive().to_string()
    }

    fn calendar_body() -> String {
        format!(
            r#"[{{"id":501,"start_date_local":"{today}T00:00:00","name":"Sweet Spot 3x12","type":"Ride",
                "category":"WORKOUT","icu_training_load":68,"workout_doc":{{}},"workout_file_base64":"{zwo}"}},
               {{"id":502,"start_date_local":"{today}T00:00:00","name":"Easy spin","type":"Ride","category":"WORKOUT",
                "icu_training_load":25,"moving_time":2700}}]"#,
            today = today(),
            zwo = BASE64.encode(ZWO.as_bytes()),
        )
    }

    fn library_body(threshold_updated: &str, folder_id: i64) -> String {
        format!(
            r#"[{{"id":7,"name":"Threshold 2x20","description":"- 2x 20m 98%","type":"Ride","folder_id":{folder_id},
                "updated":"{threshold_updated}","icu_training_load":92,"moving_time":4500}},
               {{"id":9,"name":"Openers","type":"VirtualRide","folder_id":null,"updated":"2026-09-02T08:00:00"}},
               {{"id":8,"name":"Tempo run","type":"Run","folder_id":3,"updated":"2026-09-01T08:00:00"}}]"#
        )
    }

    const FOLDERS: &str =
        r#"[{"id":3,"name":"Base","type":"FOLDER"},{"id":4,"name":"Build","type":"PLAN"}]"#;

    /// The threshold workout alone, moved to the "Build" plan.
    const THRESHOLD_MOVED: &str = r#"[{"id":7,"name":"Threshold 2x20","type":"Ride","folder_id":4,"updated":"2026-09-01T08:00:00"}]"#;

    fn routes(library: &str) -> Vec<Route> {
        vec![
            json("GET /api/v1/athlete/0 ", r#"{"id":"i1","name":"Blake"}"#),
            json("GET /api/v1/athlete/i1/events?", &calendar_body()),
            json("GET /api/v1/athlete/i1/workouts ", library),
            json("GET /api/v1/athlete/i1/folders ", FOLDERS),
            Route {
                prefix: "POST /api/v1/athlete/i1/download-workout.zwo ",
                status: "200 OK",
                content_type: "application/xml",
                body: ZWO.to_string(),
            },
        ]
    }

    #[tokio::test]
    async fn resolves_and_stores_the_athlete_for_a_key_saved_before_ids_existed() {
        let storage = Storage::in_memory().unwrap();
        storage.save_intervals_api_key("secret").unwrap();
        assert_eq!(storage.intervals_athlete().unwrap(), None);
        let (base_url, server) = serve_routes(routes(""), 1);
        let client = IntervalsClient::with_base_url("secret", &base_url).unwrap();

        let athlete = resolve_athlete(&storage, &client).await.unwrap();
        assert_eq!(athlete.id, "i1");
        assert_eq!(storage.intervals_athlete().unwrap(), Some(athlete.clone()));
        // Second call answers from storage: no request.
        assert_eq!(resolve_athlete(&storage, &client).await.unwrap(), athlete);
        assert_eq!(server.join().unwrap().len(), 1);

        let status = status(&storage).unwrap();
        assert!(status.configured);
        assert_eq!(status.athlete_name.as_deref(), Some("Blake"));
        assert_eq!(status.settings, IntervalsSyncSettings::default());
    }

    #[tokio::test]
    async fn saving_a_key_validates_it_and_a_bad_key_is_not_stored() {
        let storage = Storage::in_memory().unwrap();
        // No listener: network failure, key not saved, message says so.
        let error = save_api_key_at(&storage, "secret", "http://127.0.0.1:9")
            .await
            .unwrap_err();
        assert!(error.contains("the key was not saved"), "{error}");
        assert_eq!(storage.intervals_api_key().unwrap(), None);

        let (base_url, server) = serve_routes(
            vec![Route {
                prefix: "GET /api/v1/athlete/0 ",
                status: "401 Unauthorized",
                content_type: "application/json",
                body: "{}".into(),
            }],
            1,
        );
        let error = save_api_key_at(&storage, "bad", &base_url)
            .await
            .unwrap_err();
        assert_eq!(error, "Intervals.icu rejected the API key");
        server.join().unwrap();
        assert_eq!(storage.intervals_api_key().unwrap(), None);

        let (base_url, server) = serve_routes(routes(""), 1);
        let athlete = save_api_key_at(&storage, "  secret ", &base_url)
            .await
            .unwrap();
        server.join().unwrap();
        assert_eq!(athlete.id, "i1");
        assert_eq!(
            storage.intervals_api_key().unwrap().as_deref(),
            Some("secret")
        );
        assert_eq!(storage.intervals_athlete().unwrap(), Some(athlete));
    }

    /// `save_api_key` against a test listener.
    async fn save_api_key_at(
        storage: &Storage,
        api_key: &str,
        base_url: &str,
    ) -> Result<IntervalsAthlete, String> {
        let api_key = api_key.trim();
        let client = IntervalsClient::with_base_url(api_key, base_url)?;
        save_api_key_with(storage, api_key, &client).await
    }

    #[tokio::test]
    async fn mirrors_calendar_and_library_then_only_refetches_changes() {
        let storage = Storage::in_memory().unwrap();
        storage.save_intervals_api_key("secret").unwrap();
        let local_count = storage.workouts().unwrap().len();

        // First sync: athlete, calendar, library listing, folders, two ZWO posts.
        let (base_url, server) = serve_routes(routes(&library_body("2026-09-01T08:00:00", 3)), 6);
        let client = IntervalsClient::with_base_url("secret", &base_url).unwrap();
        let athlete = resolve_athlete(&storage, &client).await.unwrap();
        let calendar = sync_calendar(&storage, &client, &athlete.id).await.unwrap();
        let library = sync_library(&storage, &client, &athlete.id).await.unwrap();
        assert_eq!(server.join().unwrap().len(), 6);

        assert_eq!(
            calendar,
            CalendarSyncReport {
                fetched: 2,
                unstructured: 1,
                failed: 0,
                skipped: 0
            }
        );
        let planned = storage.planned_workouts().unwrap();
        assert_eq!(planned.len(), 2);
        assert_eq!(planned[0].name, "Sweet Spot 3x12");
        assert_eq!(planned[0].planned_load, Some(68));
        assert_eq!(planned[0].workout.as_ref().unwrap().steps.len(), 2);
        assert_eq!(planned[1].workout, None);
        assert_eq!(
            storage
                .planned_workout(planned[0].workout_id)
                .unwrap()
                .unwrap()
                .event_id,
            501
        );

        assert_eq!(
            library,
            LibrarySyncReport {
                added: 2,
                skipped: 1,
                ..LibrarySyncReport::default()
            }
        );
        let mirrored = storage.mirrored_workouts().unwrap();
        assert_eq!(mirrored.len(), 2);
        assert_eq!(storage.workouts().unwrap().len(), local_count + 2);
        let threshold = storage
            .workout_by_external_key("intervals:7")
            .unwrap()
            .unwrap();
        assert_eq!(threshold.source, "intervals");
        assert!(threshold.is_mirrored());
        let origin = threshold.origin.clone().unwrap();
        assert_eq!(origin.folder.as_deref(), Some("Base"));
        assert_eq!(origin.planned_load, Some(92));
        assert_eq!(threshold.description, "3x12");

        // Second sync with the threshold workout moved to another folder and
        // "Openers" gone: one listing, folders, one ZWO post; Openers removed;
        // the local uuid survives.
        let (base_url, server) = serve_routes(routes(THRESHOLD_MOVED), 3);
        let client = IntervalsClient::with_base_url("secret", &base_url).unwrap();
        let library = sync_library(&storage, &client, &athlete.id).await.unwrap();
        assert_eq!(server.join().unwrap().len(), 3);
        assert_eq!(
            library,
            LibrarySyncReport {
                updated: 1,
                removed: 1,
                ..LibrarySyncReport::default()
            }
        );
        let moved = storage
            .workout_by_external_key("intervals:7")
            .unwrap()
            .unwrap();
        assert_eq!(moved.id, threshold.id);
        assert_eq!(moved.origin.unwrap().folder.as_deref(), Some("Build"));
        assert_eq!(
            storage.workout_by_external_key("intervals:9").unwrap(),
            None
        );
        assert_eq!(storage.workouts().unwrap().len(), local_count + 1);

        // Third sync, nothing changed: listing and folders only.
        let (base_url, server) = serve_routes(routes(THRESHOLD_MOVED), 2);
        let client = IntervalsClient::with_base_url("secret", &base_url).unwrap();
        let library = sync_library(&storage, &client, &athlete.id).await.unwrap();
        assert_eq!(server.join().unwrap().len(), 2);
        assert_eq!(library.unchanged, 1);
        assert_eq!(library.added + library.updated + library.removed, 0);

        // Clearing the key purges everything Intervals.icu.
        assert_eq!(storage.clear_intervals().unwrap(), 1);
        assert_eq!(storage.workouts().unwrap().len(), local_count);
        assert_eq!(storage.planned_workouts().unwrap(), vec![]);
        assert_eq!(storage.intervals_athlete().unwrap(), None);
        assert_eq!(storage.intervals_api_key().unwrap(), None);
        assert_eq!(
            storage.intervals_sync_state().unwrap(),
            IntervalsSyncState::default()
        );
    }

    #[tokio::test]
    async fn a_failed_zwo_fetch_keeps_the_previous_copy() {
        let storage = Storage::in_memory().unwrap();
        let (base_url, server) = serve_routes(routes(&library_body("2026-09-01T08:00:00", 3)), 4);
        let client = IntervalsClient::with_base_url("secret", &base_url).unwrap();
        sync_library(&storage, &client, "i1").await.unwrap();
        server.join().unwrap();
        let before = storage
            .workout_by_external_key("intervals:7")
            .unwrap()
            .unwrap();

        let (base_url, server) = serve_routes(
            vec![
                json(
                    "GET /api/v1/athlete/i1/workouts ",
                    &library_body("2026-09-05T08:00:00", 3),
                ),
                json("GET /api/v1/athlete/i1/folders ", FOLDERS),
                Route {
                    prefix: "POST /api/v1/athlete/i1/download-workout.zwo ",
                    status: "500 Internal Server Error",
                    content_type: "text/plain",
                    body: "boom".into(),
                },
            ],
            3,
        );
        let client = IntervalsClient::with_base_url("secret", &base_url).unwrap();
        let report = sync_library(&storage, &client, "i1").await.unwrap();
        server.join().unwrap();
        // Only the changed workout was fetched; "Openers" was unchanged.
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.unchanged, 1);
        assert!(report.failed[0].starts_with("Threshold 2x20: "));
        assert_eq!(report.removed, 0);
        let after = storage
            .workout_by_external_key("intervals:7")
            .unwrap()
            .unwrap();
        assert_eq!(after, before);
    }

    #[tokio::test]
    async fn turning_a_mirror_off_removes_its_copies_and_mirrors_stay_read_only() {
        let storage = Storage::in_memory().unwrap();
        storage.save_intervals_api_key("secret").unwrap();
        let local_count = storage.workouts().unwrap().len();
        let (base_url, server) = serve_routes(routes(&library_body("2026-09-01T08:00:00", 3)), 6);
        let client = IntervalsClient::with_base_url("secret", &base_url).unwrap();
        let athlete = resolve_athlete(&storage, &client).await.unwrap();
        sync_calendar(&storage, &client, &athlete.id).await.unwrap();
        sync_library(&storage, &client, &athlete.id).await.unwrap();
        server.join().unwrap();
        let mirrored = storage
            .workout_by_external_key("intervals:7")
            .unwrap()
            .unwrap();
        let local = storage
            .workouts()
            .unwrap()
            .into_iter()
            .find(|workout| !workout.is_mirrored())
            .unwrap();

        assert_eq!(
            ensure_editable(&storage, mirrored.id, None).unwrap_err(),
            MIRRORED_WORKOUT_MESSAGE
        );
        assert_eq!(
            ensure_editable(&storage, local.id, Some(&mirrored)).unwrap_err(),
            MIRRORED_WORKOUT_MESSAGE
        );
        ensure_editable(&storage, local.id, Some(&local)).unwrap();
        ensure_editable(&storage, Uuid::new_v4(), None).unwrap();

        let status = update_settings(
            &storage,
            IntervalsSyncSettings {
                calendar: true,
                library: false,
            },
        )
        .unwrap();
        assert!(!status.settings.library);
        assert_eq!(storage.mirrored_workouts().unwrap(), vec![]);
        assert_eq!(storage.workouts().unwrap().len(), local_count);
        assert_eq!(storage.planned_workouts().unwrap().len(), 2);

        update_settings(
            &storage,
            IntervalsSyncSettings {
                calendar: false,
                library: false,
            },
        )
        .unwrap();
        assert_eq!(storage.planned_workouts().unwrap(), vec![]);
        // Turning a mirror back on purges nothing; the next sync refills it.
        let status = update_settings(&storage, IntervalsSyncSettings::default()).unwrap();
        assert_eq!(status.settings, IntervalsSyncSettings::default());
    }

    #[tokio::test]
    async fn sync_intervals_respects_the_toggles_and_records_state() {
        let storage = Storage::in_memory().unwrap();
        storage.save_intervals_api_key("secret").unwrap();
        storage
            .save_intervals_athlete(&IntervalsAthlete {
                id: "i1".into(),
                name: None,
            })
            .unwrap();
        let mut state = storage.intervals_sync_state().unwrap();
        state.settings.library = false;
        storage.save_intervals_sync_state(&state).unwrap();

        // Only the calendar half runs, and it fails: the state records the error.
        let (base_url, server) = serve_routes(
            vec![Route {
                prefix: "GET /api/v1/athlete/i1/events?",
                status: "500 Internal Server Error",
                content_type: "text/plain",
                body: "down".into(),
            }],
            1,
        );
        let report = sync_intervals_at(&storage, &base_url).await.unwrap();
        server.join().unwrap();
        assert_eq!(report.library, SyncOutcome::Off);
        assert!(matches!(report.calendar, SyncOutcome::Failed { .. }));
        let state = storage.intervals_sync_state().unwrap();
        assert_eq!(state.last_synced_at, None);
        assert!(state.last_error.unwrap().contains("HTTP 500"));

        let (base_url, server) = serve_routes(routes(""), 1);
        let report = sync_intervals_at(&storage, &base_url).await.unwrap();
        server.join().unwrap();
        assert!(matches!(report.calendar, SyncOutcome::Done { .. }));
        let state = storage.intervals_sync_state().unwrap();
        assert_eq!(state.last_synced_at, Some(report.synced_at));
        assert_eq!(state.last_error, None);
    }

    /// `sync_intervals` against a test listener.
    async fn sync_intervals_at(
        storage: &Storage,
        base_url: &str,
    ) -> Result<IntervalsSyncReport, String> {
        let api_key = storage.intervals_api_key()?.unwrap();
        let client = IntervalsClient::with_base_url(&api_key, base_url)?;
        sync_with_client(storage, &client).await
    }
}
