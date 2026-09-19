use std::{
    fs,
    path::Path,
    sync::{Mutex, MutexGuard},
};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use crate::{
    devices::{KnownDevice, SourcePreferences},
    domain::{
        DistanceSource, DistanceUnit, PowerTarget, Profile, SessionDetail, SessionSummary,
        Telemetry, WeightUnit, Workout, WorkoutStep,
    },
};

const SOURCE_PREFERENCES_KEY: &str = "source_preferences";
const POWER_SMOOTHING_KEY: &str = "power_smoothing";
const INTERVALS_API_KEY: &str = "intervals_api_key";
const TRAINING_ZONES_KEY: &str = "training_zones";
const RIDE_DISPLAY_PREFERENCES_KEY: &str = "ride_display_preferences";

/// How the live power readout is averaged on the ride screens. Stored so the
/// rider's last choice comes back on the next launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum PowerSmoothing {
    #[default]
    #[serde(rename = "instant")]
    Instant,
    #[serde(rename = "3s")]
    ThreeSeconds,
    #[serde(rename = "5s")]
    FiveSeconds,
    #[serde(rename = "10s")]
    TenSeconds,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZoneDefinition {
    pub name: String,
    pub upper_bound: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ZoneMode {
    #[default]
    Derived,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TrainingZoneSettings {
    pub version: u8,
    pub sync_power_zones_from_intervals: bool,
    pub power_mode: ZoneMode,
    pub power_zones: Vec<ZoneDefinition>,
    pub heart_rate_mode: ZoneMode,
    pub heart_rate_zones: Vec<ZoneDefinition>,
}

impl Default for TrainingZoneSettings {
    fn default() -> Self {
        Self {
            version: 1,
            sync_power_zones_from_intervals: false,
            power_mode: ZoneMode::Derived,
            power_zones: Vec::new(),
            heart_rate_mode: ZoneMode::Derived,
            heart_rate_zones: Vec::new(),
        }
    }
}

impl TrainingZoneSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err("Unsupported training-zone settings version".into());
        }
        validate_zones("power", self.power_mode, &self.power_zones, 1, 3_000)?;
        validate_zones(
            "heart-rate",
            self.heart_rate_mode,
            &self.heart_rate_zones,
            30,
            250,
        )
    }
}

fn validate_zones(
    label: &str,
    mode: ZoneMode,
    zones: &[ZoneDefinition],
    minimum: u16,
    maximum: u16,
) -> Result<(), String> {
    if mode == ZoneMode::Derived {
        return Ok(());
    }
    if !(2..=10).contains(&zones.len()) {
        return Err(format!("Custom {label} zones require 2–10 zones"));
    }
    let mut previous = 0;
    for (index, zone) in zones.iter().enumerate() {
        if zone.name.trim().is_empty() {
            return Err(format!("{label} zone names cannot be empty"));
        }
        match zone.upper_bound {
            Some(_) if index + 1 == zones.len() => {
                return Err(format!("The final {label} zone must be open-ended"));
            }
            Some(bound) if bound < minimum || bound > maximum || bound <= previous => {
                return Err(format!(
                    "{label} zone boundaries must increase and stay between {minimum} and {maximum}"
                ));
            }
            Some(bound) => previous = bound,
            None if index + 1 != zones.len() => {
                return Err(format!("Only the final {label} zone can be open-ended"));
            }
            None => {}
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RideDisplayPreferences {
    pub show_time_in_zone: bool,
}

impl Default for RideDisplayPreferences {
    fn default() -> Self {
        Self {
            show_time_in_zone: false,
        }
    }
}

pub struct Storage {
    connection: Mutex<Connection>,
}

impl Storage {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let connection = Connection::open(path).map_err(|error| error.to_string())?;
        connection
            .execute_batch(
                "
                PRAGMA foreign_keys = ON;
                PRAGMA journal_mode = WAL;
                CREATE TABLE IF NOT EXISTS profiles (
                  id TEXT PRIMARY KEY,
                  name TEXT NOT NULL,
                  ftp_watts INTEGER NOT NULL,
                  max_power_watts INTEGER NOT NULL,
                  active INTEGER NOT NULL DEFAULT 1
                );
                CREATE TABLE IF NOT EXISTS workouts (
                  id TEXT PRIMARY KEY,
                  name TEXT NOT NULL,
                  source TEXT NOT NULL,
                  version INTEGER NOT NULL,
                  payload_json TEXT NOT NULL,
                  created_at TEXT NOT NULL,
                  updated_at TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS sessions (
                  id TEXT PRIMARY KEY,
                  workout_id TEXT,
                  workout_name TEXT NOT NULL,
                  started_at TEXT NOT NULL,
                  ended_at TEXT,
                  elapsed_seconds INTEGER NOT NULL DEFAULT 0,
                  average_power_watts INTEGER NOT NULL DEFAULT 0,
                  max_power_watts INTEGER NOT NULL DEFAULT 0,
                  average_cadence_rpm REAL,
                  completed INTEGER NOT NULL DEFAULT 0
                );
                CREATE TABLE IF NOT EXISTS telemetry_samples (
                  session_id TEXT NOT NULL,
                  timestamp_ms INTEGER NOT NULL,
                  payload_json TEXT NOT NULL,
                  PRIMARY KEY(session_id, timestamp_ms),
                  FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
                );
                CREATE INDEX IF NOT EXISTS telemetry_session_idx
                  ON telemetry_samples(session_id, timestamp_ms);
                CREATE TABLE IF NOT EXISTS settings (
                  key TEXT PRIMARY KEY,
                  value_json TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS known_devices (
                  id TEXT PRIMARY KEY,
                  payload_json TEXT NOT NULL,
                  last_connected_at TEXT NOT NULL
                );
                ",
            )
            .map_err(|error| error.to_string())?;
        ensure_column(
            &connection,
            "profiles",
            "rider_weight_kg",
            "REAL NOT NULL DEFAULT 75.0",
        )?;
        ensure_column(
            &connection,
            "profiles",
            "bike_weight_kg",
            "REAL NOT NULL DEFAULT 9.0",
        )?;
        ensure_column(
            &connection,
            "profiles",
            "weight_unit",
            "TEXT NOT NULL DEFAULT 'kg'",
        )?;
        ensure_column(
            &connection,
            "profiles",
            "distance_unit",
            "TEXT NOT NULL DEFAULT 'km'",
        )?;
        ensure_column(
            &connection,
            "profiles",
            "max_heart_rate_bpm",
            "INTEGER NOT NULL DEFAULT 190",
        )?;
        ensure_column(
            &connection,
            "sessions",
            "estimated_distance_meters",
            "REAL NOT NULL DEFAULT 0.0",
        )?;
        ensure_column(&connection, "sessions", "distance_source", "TEXT")?;
        ensure_column(
            &connection,
            "sessions",
            "distance_weight_kg",
            "REAL NOT NULL DEFAULT 84.0",
        )?;
        let storage = Self {
            connection: Mutex::new(connection),
        };
        storage.seed()?;
        Ok(storage)
    }

    #[cfg(test)]
    pub fn in_memory() -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!("blakebike-{}.sqlite", Uuid::new_v4()));
        Self::open(&path)
    }

    fn connection(&self) -> Result<MutexGuard<'_, Connection>, String> {
        self.connection
            .lock()
            .map_err(|_| "Database lock was poisoned".to_string())
    }

    fn seed(&self) -> Result<(), String> {
        let connection = self.connection()?;
        let profile_count: u32 = connection
            .query_row("SELECT COUNT(*) FROM profiles", [], |row| row.get(0))
            .map_err(|error| error.to_string())?;
        if profile_count == 0 {
            let profile = Profile::default();
            connection
                .execute(
                    "INSERT INTO profiles(
                       id, name, ftp_watts, max_power_watts, rider_weight_kg, bike_weight_kg,
                       weight_unit, distance_unit, max_heart_rate_bpm
                     ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        profile.id.to_string(),
                        profile.name,
                        profile.ftp_watts,
                        profile.max_power_watts,
                        profile.rider_weight_kg,
                        profile.bike_weight_kg,
                        weight_unit_value(profile.weight_unit),
                        distance_unit_value(profile.distance_unit),
                        profile.max_heart_rate_bpm,
                    ],
                )
                .map_err(|error| error.to_string())?;
        }
        let workout_count: u32 = connection
            .query_row("SELECT COUNT(*) FROM workouts", [], |row| row.get(0))
            .map_err(|error| error.to_string())?;
        drop(connection);
        if workout_count == 0 {
            self.save_workout(&sample_workout())?;
        }
        Ok(())
    }

    pub fn profile(&self) -> Result<Profile, String> {
        self.connection()?
            .query_row(
                "SELECT id, name, ftp_watts, max_power_watts, rider_weight_kg, bike_weight_kg,
                 weight_unit, distance_unit, max_heart_rate_bpm
                 FROM profiles WHERE active = 1 LIMIT 1",
                [],
                |row| {
                    Ok(Profile {
                        id: parse_uuid(row.get::<_, String>(0)?)?,
                        name: row.get(1)?,
                        ftp_watts: row.get(2)?,
                        max_power_watts: row.get(3)?,
                        rider_weight_kg: row.get(4)?,
                        bike_weight_kg: row.get(5)?,
                        weight_unit: parse_weight_unit(&row.get::<_, String>(6)?),
                        distance_unit: parse_distance_unit(&row.get::<_, String>(7)?),
                        max_heart_rate_bpm: row.get(8)?,
                    })
                },
            )
            .map_err(|error| error.to_string())
    }

    pub fn save_profile(&self, profile: &Profile) -> Result<(), String> {
        if profile.name.trim().is_empty() || !(50..=500).contains(&profile.ftp_watts) {
            return Err("Enter a name and an FTP between 50 and 500 watts".into());
        }
        if !(100..=230).contains(&profile.max_heart_rate_bpm) {
            return Err("Enter a maximum heart rate between 100 and 230 bpm".into());
        }
        if !(100..=2_500).contains(&profile.max_power_watts) {
            return Err("Enter a safety power limit between 100 and 2500 watts".into());
        }
        if !(30.0..=250.0).contains(&profile.rider_weight_kg)
            || !(3.0..=40.0).contains(&profile.bike_weight_kg)
        {
            return Err("Enter a rider weight from 30–250 kg and bike weight from 3–40 kg".into());
        }
        self.connection()?
            .execute(
                "INSERT INTO profiles(
                   id, name, ftp_watts, max_power_watts, rider_weight_kg, bike_weight_kg,
                   weight_unit, distance_unit, max_heart_rate_bpm, active
                 ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1)
                 ON CONFLICT(id) DO UPDATE SET name = excluded.name,
                   ftp_watts = excluded.ftp_watts, max_power_watts = excluded.max_power_watts,
                   rider_weight_kg = excluded.rider_weight_kg,
                   bike_weight_kg = excluded.bike_weight_kg,
                   weight_unit = excluded.weight_unit,
                   distance_unit = excluded.distance_unit,
                   max_heart_rate_bpm = excluded.max_heart_rate_bpm",
                params![
                    profile.id.to_string(),
                    profile.name,
                    profile.ftp_watts,
                    profile.max_power_watts,
                    profile.rider_weight_kg,
                    profile.bike_weight_kg,
                    weight_unit_value(profile.weight_unit),
                    distance_unit_value(profile.distance_unit),
                    profile.max_heart_rate_bpm,
                ],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    fn setting<T: serde::de::DeserializeOwned>(&self, key: &str) -> Result<Option<T>, String> {
        let json: Option<String> = self
            .connection()?
            .query_row(
                "SELECT value_json FROM settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        match json {
            Some(json) => serde_json::from_str(&json)
                .map(Some)
                .map_err(|error| format!("Stored setting {key} is unreadable: {error}")),
            None => Ok(None),
        }
    }

    fn save_setting<T: serde::Serialize>(&self, key: &str, value: &T) -> Result<(), String> {
        let json = serde_json::to_string(value).map_err(|error| error.to_string())?;
        self.connection()?
            .execute(
                "INSERT INTO settings(key, value_json) VALUES(?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
                params![key, json],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn intervals_api_key(&self) -> Result<Option<String>, String> {
        self.setting(INTERVALS_API_KEY)
    }

    pub fn save_intervals_api_key(&self, api_key: &str) -> Result<(), String> {
        let api_key = api_key.trim();
        if api_key.is_empty() {
            return Err("Enter an Intervals.icu API key".into());
        }
        self.save_setting(INTERVALS_API_KEY, &api_key)
    }

    pub fn clear_intervals_api_key(&self) -> Result<(), String> {
        self.connection()?
            .execute(
                "DELETE FROM settings WHERE key = ?1",
                params![INTERVALS_API_KEY],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// Which device feeds each telemetry metric; `Auto` everywhere by default.
    pub fn source_preferences(&self) -> Result<SourcePreferences, String> {
        Ok(self.setting(SOURCE_PREFERENCES_KEY)?.unwrap_or_default())
    }

    pub fn save_source_preferences(&self, preferences: &SourcePreferences) -> Result<(), String> {
        self.save_setting(SOURCE_PREFERENCES_KEY, preferences)
    }

    /// Live power averaging window for the ride screens; instant by default.
    pub fn power_smoothing(&self) -> Result<PowerSmoothing, String> {
        Ok(self.setting(POWER_SMOOTHING_KEY)?.unwrap_or_default())
    }

    pub fn save_power_smoothing(&self, smoothing: PowerSmoothing) -> Result<(), String> {
        self.save_setting(POWER_SMOOTHING_KEY, &smoothing)
    }

    pub fn training_zones(&self) -> Result<TrainingZoneSettings, String> {
        Ok(self.setting(TRAINING_ZONES_KEY)?.unwrap_or_default())
    }

    pub fn save_training_zones(&self, zones: &TrainingZoneSettings) -> Result<(), String> {
        zones.validate()?;
        self.save_setting(TRAINING_ZONES_KEY, zones)
    }

    pub fn ride_display_preferences(&self) -> Result<RideDisplayPreferences, String> {
        Ok(self
            .setting(RIDE_DISPLAY_PREFERENCES_KEY)?
            .unwrap_or_default())
    }

    pub fn save_ride_display_preferences(
        &self,
        preferences: &RideDisplayPreferences,
    ) -> Result<(), String> {
        self.save_setting(RIDE_DISPLAY_PREFERENCES_KEY, preferences)
    }

    /// Remember (or refresh) a device after a successful connection.
    pub fn remember_device(&self, device: &KnownDevice) -> Result<(), String> {
        let json = serde_json::to_string(device).map_err(|error| error.to_string())?;
        self.connection()?
            .execute(
                "INSERT INTO known_devices(id, payload_json, last_connected_at) VALUES(?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET payload_json = excluded.payload_json,
                                               last_connected_at = excluded.last_connected_at",
                params![device.id, json, device.last_connected_at.to_rfc3339()],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// Remembered devices, most recently connected first.
    pub fn known_devices(&self) -> Result<Vec<KnownDevice>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT payload_json FROM known_devices ORDER BY last_connected_at DESC")
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?;
        let mut devices = Vec::new();
        for row in rows {
            let json = row.map_err(|error| error.to_string())?;
            match serde_json::from_str::<KnownDevice>(&json) {
                Ok(device) => devices.push(device),
                Err(error) => tracing::warn!(error = %error, "Skipping unreadable known device"),
            }
        }
        Ok(devices)
    }

    pub fn forget_device(&self, id: &str) -> Result<(), String> {
        self.connection()?
            .execute("DELETE FROM known_devices WHERE id = ?1", params![id])
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn forget_all_devices(&self) -> Result<usize, String> {
        self.connection()?
            .execute("DELETE FROM known_devices", [])
            .map_err(|error| error.to_string())
    }

    pub fn workouts(&self) -> Result<Vec<Workout>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT payload_json FROM workouts ORDER BY updated_at DESC")
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?;
        rows.map(|row| {
            serde_json::from_str(&row.map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())
        })
        .collect()
    }

    pub fn workout(&self, id: Uuid) -> Result<Option<Workout>, String> {
        let payload = self
            .connection()?
            .query_row(
                "SELECT payload_json FROM workouts WHERE id = ?1",
                [id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        payload
            .map(|json| serde_json::from_str(&json).map_err(|error| error.to_string()))
            .transpose()
    }

    pub fn save_workout(&self, workout: &Workout) -> Result<(), String> {
        workout.validate()?;
        let payload = serde_json::to_string(workout).map_err(|error| error.to_string())?;
        self.connection()?
            .execute(
                "INSERT INTO workouts(id, name, source, version, payload_json, created_at, updated_at)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(id) DO UPDATE SET name = excluded.name, source = excluded.source,
                   version = excluded.version, payload_json = excluded.payload_json,
                   updated_at = excluded.updated_at",
                params![
                    workout.id.to_string(),
                    workout.name,
                    workout.source,
                    workout.version,
                    payload,
                    workout.created_at.to_rfc3339(),
                    workout.updated_at.to_rfc3339(),
                ],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn delete_workout(&self, id: Uuid) -> Result<(), String> {
        self.connection()?
            .execute("DELETE FROM workouts WHERE id = ?1", [id.to_string()])
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn start_session(
        &self,
        workout_id: Option<Uuid>,
        workout_name: &str,
        distance_weight_kg: f32,
    ) -> Result<SessionSummary, String> {
        let summary = SessionSummary {
            id: Uuid::new_v4(),
            workout_id,
            workout_name: workout_name.to_string(),
            started_at: Utc::now(),
            ended_at: None,
            elapsed_seconds: 0,
            average_power_watts: 0,
            max_power_watts: 0,
            average_cadence_rpm: None,
            estimated_distance_meters: 0.0,
            distance_source: None,
            distance_weight_kg,
            completed: false,
        };
        self.connection()?
            .execute(
                "INSERT INTO sessions(id, workout_id, workout_name, started_at, distance_weight_kg)
                 VALUES(?1, ?2, ?3, ?4, ?5)",
                params![
                    summary.id.to_string(),
                    summary.workout_id.map(|id| id.to_string()),
                    summary.workout_name,
                    summary.started_at.to_rfc3339(),
                    summary.distance_weight_kg,
                ],
            )
            .map_err(|error| error.to_string())?;
        Ok(summary)
    }

    pub fn record_sample(&self, session_id: Uuid, sample: &Telemetry) -> Result<(), String> {
        let json = serde_json::to_string(sample).map_err(|error| error.to_string())?;
        self.connection()?
            .execute(
                "INSERT OR REPLACE INTO telemetry_samples(session_id, timestamp_ms, payload_json)
                 VALUES(?1, ?2, ?3)",
                params![session_id.to_string(), sample.timestamp_ms, json],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn finish_session(&self, summary: &SessionSummary) -> Result<(), String> {
        self.connection()?
            .execute(
                "UPDATE sessions SET ended_at = ?2, elapsed_seconds = ?3,
                  average_power_watts = ?4, max_power_watts = ?5,
                  average_cadence_rpm = ?6, completed = ?7,
                  estimated_distance_meters = ?8, distance_source = ?9,
                  distance_weight_kg = ?10 WHERE id = ?1",
                params![
                    summary.id.to_string(),
                    summary.ended_at.map(|date| date.to_rfc3339()),
                    summary.elapsed_seconds,
                    summary.average_power_watts,
                    summary.max_power_watts,
                    summary.average_cadence_rpm,
                    summary.completed,
                    summary.estimated_distance_meters,
                    summary.distance_source.map(distance_source_value),
                    summary.distance_weight_kg,
                ],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn update_session_distance(
        &self,
        id: Uuid,
        estimated_distance_meters: f64,
        source: Option<DistanceSource>,
    ) -> Result<(), String> {
        self.connection()?
            .execute(
                "UPDATE sessions SET estimated_distance_meters = ?2, distance_source = ?3
                 WHERE id = ?1",
                params![
                    id.to_string(),
                    estimated_distance_meters,
                    source.map(distance_source_value),
                ],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn sessions(&self) -> Result<Vec<SessionSummary>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, workout_id, workout_name, started_at, ended_at, elapsed_seconds,
                 average_power_watts, max_power_watts, average_cadence_rpm, completed,
                 estimated_distance_meters, distance_source, distance_weight_kg
                 FROM sessions ORDER BY started_at DESC",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], row_to_session)
            .map_err(|error| error.to_string())?;
        rows.map(|row| row.map_err(|error| error.to_string()))
            .collect()
    }

    pub fn session(&self, id: Uuid) -> Result<Option<SessionDetail>, String> {
        let connection = self.connection()?;
        let summary = connection
            .query_row(
                "SELECT id, workout_id, workout_name, started_at, ended_at, elapsed_seconds,
                 average_power_watts, max_power_watts, average_cadence_rpm, completed,
                 estimated_distance_meters, distance_source, distance_weight_kg
                 FROM sessions WHERE id = ?1",
                [id.to_string()],
                row_to_session,
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let Some(summary) = summary else {
            return Ok(None);
        };
        let mut statement = connection
            .prepare(
                "SELECT payload_json FROM telemetry_samples
                 WHERE session_id = ?1 ORDER BY timestamp_ms",
            )
            .map_err(|error| error.to_string())?;
        let samples = statement
            .query_map([id.to_string()], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?
            .map(|row| {
                serde_json::from_str(&row.map_err(|error| error.to_string())?)
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<Telemetry>, String>>()?;
        Ok(Some(SessionDetail { summary, samples }))
    }
}

fn row_to_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionSummary> {
    let started: String = row.get(3)?;
    let ended: Option<String> = row.get(4)?;
    Ok(SessionSummary {
        id: parse_uuid(row.get::<_, String>(0)?)?,
        workout_id: row
            .get::<_, Option<String>>(1)?
            .map(parse_uuid)
            .transpose()?,
        workout_name: row.get(2)?,
        started_at: parse_date(started)?,
        ended_at: ended.map(parse_date).transpose()?,
        elapsed_seconds: row.get(5)?,
        average_power_watts: row.get(6)?,
        max_power_watts: row.get(7)?,
        average_cadence_rpm: row.get(8)?,
        completed: row.get(9)?,
        estimated_distance_meters: row.get(10)?,
        distance_source: row
            .get::<_, Option<String>>(11)?
            .as_deref()
            .and_then(parse_distance_source),
        distance_weight_kg: row.get(12)?,
    })
}

fn ensure_column(
    connection: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), String> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|error| error.to_string())?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    drop(statement);
    if !columns.iter().any(|existing| existing == column) {
        connection
            .execute(
                &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
                [],
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn weight_unit_value(unit: WeightUnit) -> &'static str {
    match unit {
        WeightUnit::Kg => "kg",
        WeightUnit::Lb => "lb",
    }
}

fn parse_weight_unit(value: &str) -> WeightUnit {
    match value {
        "lb" => WeightUnit::Lb,
        _ => WeightUnit::Kg,
    }
}

fn distance_unit_value(unit: DistanceUnit) -> &'static str {
    match unit {
        DistanceUnit::Km => "km",
        DistanceUnit::Mi => "mi",
    }
}

fn parse_distance_unit(value: &str) -> DistanceUnit {
    match value {
        "mi" => DistanceUnit::Mi,
        _ => DistanceUnit::Km,
    }
}

fn distance_source_value(source: DistanceSource) -> &'static str {
    match source {
        DistanceSource::Trainer => "trainer",
        DistanceSource::Power => "power",
        DistanceSource::Mixed => "mixed",
    }
}

fn parse_distance_source(value: &str) -> Option<DistanceSource> {
    match value {
        "trainer" => Some(DistanceSource::Trainer),
        "power" => Some(DistanceSource::Power),
        "mixed" => Some(DistanceSource::Mixed),
        _ => None,
    }
}

fn parse_uuid(value: String) -> rusqlite::Result<Uuid> {
    Uuid::parse_str(&value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            value.len(),
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn parse_date(value: String) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value)
        .map(|date| date.with_timezone(&Utc))
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                value.len(),
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
}

fn sample_workout() -> Workout {
    let mut workout = Workout::new(
        "FTP Builder",
        vec![
            WorkoutStep::Ramp {
                duration_seconds: 300,
                start: PowerTarget::PercentFtp(45),
                end: PowerTarget::PercentFtp(70),
            },
            WorkoutStep::Repeat {
                repetitions: 3,
                steps: vec![
                    WorkoutStep::Steady {
                        duration_seconds: 180,
                        target: PowerTarget::PercentFtp(100),
                    },
                    WorkoutStep::Steady {
                        duration_seconds: 120,
                        target: PowerTarget::PercentFtp(55),
                    },
                ],
            },
            WorkoutStep::Ramp {
                duration_seconds: 300,
                start: PowerTarget::PercentFtp(65),
                end: PowerTarget::PercentFtp(40),
            },
        ],
    );
    workout.description = "A short progressive workout with three threshold efforts.".into();
    workout
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initializes_and_round_trips_workouts() {
        let storage = Storage::in_memory().unwrap();
        let workouts = storage.workouts().unwrap();
        assert_eq!(workouts.len(), 1);
        assert!(storage.workout(workouts[0].id).unwrap().is_some());
    }

    #[test]
    fn source_preferences_default_to_auto_and_round_trip() {
        use crate::devices::{DeviceRole, fuser::SourceChoice};
        let storage = Storage::in_memory().unwrap();
        assert_eq!(
            storage.source_preferences().unwrap(),
            SourcePreferences::default()
        );
        let preferences = SourcePreferences {
            heart_rate: SourceChoice::Role(DeviceRole::HeartRate),
            ..SourcePreferences::default()
        };
        storage.save_source_preferences(&preferences).unwrap();
        assert_eq!(storage.source_preferences().unwrap(), preferences);
        let updated = SourcePreferences::default();
        storage.save_source_preferences(&updated).unwrap();
        assert_eq!(storage.source_preferences().unwrap(), updated);
    }

    #[test]
    fn power_smoothing_defaults_to_instant_and_round_trips() {
        let storage = Storage::in_memory().unwrap();
        assert_eq!(storage.power_smoothing().unwrap(), PowerSmoothing::Instant);
        storage
            .save_power_smoothing(PowerSmoothing::TenSeconds)
            .unwrap();
        assert_eq!(
            storage.power_smoothing().unwrap(),
            PowerSmoothing::TenSeconds
        );
        storage
            .save_power_smoothing(PowerSmoothing::ThreeSeconds)
            .unwrap();
        assert_eq!(
            storage.power_smoothing().unwrap(),
            PowerSmoothing::ThreeSeconds
        );
        // The frontend uses the same short names.
        assert_eq!(
            serde_json::to_string(&PowerSmoothing::FiveSeconds).unwrap(),
            "\"5s\""
        );
    }

    #[test]
    fn training_zones_and_ride_display_default_validate_and_round_trip() {
        let storage = Storage::in_memory().unwrap();
        assert_eq!(
            storage.training_zones().unwrap(),
            TrainingZoneSettings::default()
        );
        assert_eq!(
            storage.ride_display_preferences().unwrap(),
            RideDisplayPreferences::default()
        );
        let zones = TrainingZoneSettings {
            sync_power_zones_from_intervals: true,
            heart_rate_mode: ZoneMode::Custom,
            heart_rate_zones: vec![
                ZoneDefinition {
                    name: "Easy".into(),
                    upper_bound: Some(140),
                },
                ZoneDefinition {
                    name: "Hard".into(),
                    upper_bound: None,
                },
            ],
            ..TrainingZoneSettings::default()
        };
        storage.save_training_zones(&zones).unwrap();
        assert_eq!(storage.training_zones().unwrap(), zones);
        let display = RideDisplayPreferences {
            show_time_in_zone: true,
        };
        storage.save_ride_display_preferences(&display).unwrap();
        assert_eq!(storage.ride_display_preferences().unwrap(), display);
    }

    #[test]
    fn rejects_invalid_custom_zone_boundaries() {
        let invalid = TrainingZoneSettings {
            power_mode: ZoneMode::Custom,
            power_zones: vec![
                ZoneDefinition {
                    name: "One".into(),
                    upper_bound: Some(200),
                },
                ZoneDefinition {
                    name: "Two".into(),
                    upper_bound: Some(150),
                },
            ],
            ..TrainingZoneSettings::default()
        };
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn intervals_api_key_round_trips_and_clears() {
        let storage = Storage::in_memory().unwrap();
        assert_eq!(storage.intervals_api_key().unwrap(), None);

        storage
            .save_intervals_api_key("  secret-api-key  ")
            .unwrap();
        assert_eq!(
            storage.intervals_api_key().unwrap().as_deref(),
            Some("secret-api-key")
        );

        storage.clear_intervals_api_key().unwrap();
        assert_eq!(storage.intervals_api_key().unwrap(), None);
        assert!(storage.save_intervals_api_key("  ").is_err());
    }

    #[test]
    fn remembers_and_forgets_devices() {
        use crate::devices::{Capability, DeviceRole};
        let storage = Storage::in_memory().unwrap();
        assert!(storage.known_devices().unwrap().is_empty());
        let strap = KnownDevice {
            id: "strap".into(),
            name: "HRM-Pro".into(),
            role: DeviceRole::HeartRate,
            capabilities: vec![Capability::HeartRate],
            simulated: false,
            manufacturer: Some("Garmin".into()),
            model: None,
            last_connected_at: Utc::now() - chrono::Duration::minutes(5),
        };
        let trainer = KnownDevice {
            id: "kickr".into(),
            name: "KICKR CORE".into(),
            role: DeviceRole::Trainer,
            capabilities: vec![Capability::Ftms, Capability::CyclingPower],
            simulated: false,
            manufacturer: Some("Wahoo".into()),
            model: Some("KICKR CORE".into()),
            last_connected_at: Utc::now(),
        };
        storage.remember_device(&strap).unwrap();
        storage.remember_device(&trainer).unwrap();
        let known = storage.known_devices().unwrap();
        assert_eq!(known.len(), 2);
        assert_eq!(known[0].id, "kickr", "most recent first");
        assert_eq!(known[1].manufacturer.as_deref(), Some("Garmin"));

        // Reconnecting refreshes the row instead of duplicating it.
        let refreshed = KnownDevice {
            last_connected_at: Utc::now() + chrono::Duration::minutes(1),
            model: Some("HRM-Pro Plus".into()),
            ..strap.clone()
        };
        storage.remember_device(&refreshed).unwrap();
        let known = storage.known_devices().unwrap();
        assert_eq!(known.len(), 2);
        assert_eq!(known[0].id, "strap");
        assert_eq!(known[0].model.as_deref(), Some("HRM-Pro Plus"));

        storage.forget_device("strap").unwrap();
        assert_eq!(storage.known_devices().unwrap().len(), 1);
        assert_eq!(storage.forget_all_devices().unwrap(), 1);
        assert!(storage.known_devices().unwrap().is_empty());
    }

    #[test]
    fn stores_free_ride_without_a_workout() {
        let storage = Storage::in_memory().unwrap();
        let mut session = storage.start_session(None, "Free Ride", 84.0).unwrap();
        session.estimated_distance_meters = 12_345.6;
        session.distance_source = Some(DistanceSource::Mixed);
        storage.finish_session(&session).unwrap();
        let stored = storage.session(session.id).unwrap().unwrap().summary;

        assert_eq!(stored.workout_id, None);
        assert_eq!(stored.workout_name, "Free Ride");
        assert_eq!(stored.estimated_distance_meters, 12_345.6);
        assert_eq!(stored.distance_source, Some(DistanceSource::Mixed));
        assert_eq!(stored.distance_weight_kg, 84.0);
    }

    #[test]
    fn profile_units_round_trip_while_weights_remain_metric() {
        let storage = Storage::in_memory().unwrap();
        let mut profile = storage.profile().unwrap();
        profile.rider_weight_kg = 81.25;
        profile.bike_weight_kg = 10.5;
        profile.weight_unit = WeightUnit::Lb;
        profile.distance_unit = DistanceUnit::Mi;
        storage.save_profile(&profile).unwrap();

        let stored = storage.profile().unwrap();
        assert_eq!(stored.rider_weight_kg, 81.25);
        assert_eq!(stored.bike_weight_kg, 10.5);
        assert_eq!(stored.weight_unit, WeightUnit::Lb);
        assert_eq!(stored.distance_unit, DistanceUnit::Mi);
    }

    #[test]
    fn migrates_existing_profile_and_session_tables() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("legacy.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "
                CREATE TABLE profiles (
                  id TEXT PRIMARY KEY, name TEXT NOT NULL, ftp_watts INTEGER NOT NULL,
                  max_power_watts INTEGER NOT NULL, active INTEGER NOT NULL DEFAULT 1
                );
                CREATE TABLE sessions (
                  id TEXT PRIMARY KEY, workout_id TEXT, workout_name TEXT NOT NULL,
                  started_at TEXT NOT NULL, ended_at TEXT, elapsed_seconds INTEGER NOT NULL DEFAULT 0,
                  average_power_watts INTEGER NOT NULL DEFAULT 0,
                  max_power_watts INTEGER NOT NULL DEFAULT 0, average_cadence_rpm REAL,
                  completed INTEGER NOT NULL DEFAULT 0
                );
                ",
            )
            .unwrap();
        let profile_id = Uuid::new_v4();
        connection
            .execute(
                "INSERT INTO profiles(id, name, ftp_watts, max_power_watts)
                 VALUES(?1, 'Legacy', 220, 900)",
                [profile_id.to_string()],
            )
            .unwrap();
        drop(connection);

        let storage = Storage::open(&path).unwrap();
        let profile = storage.profile().unwrap();
        assert_eq!(profile.rider_weight_kg, 75.0);
        assert_eq!(profile.bike_weight_kg, 9.0);
        assert_eq!(profile.weight_unit, WeightUnit::Kg);
        assert_eq!(profile.distance_unit, DistanceUnit::Km);
        assert_eq!(profile.max_heart_rate_bpm, 190);
        let session = storage.start_session(None, "Migrated", 84.0).unwrap();
        assert_eq!(
            storage
                .session(session.id)
                .unwrap()
                .unwrap()
                .summary
                .estimated_distance_meters,
            0.0
        );
    }
}
