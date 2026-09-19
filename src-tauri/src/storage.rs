use std::{
    fs,
    path::Path,
    sync::{Mutex, MutexGuard},
};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use crate::domain::{
    PowerTarget, Profile, SessionDetail, SessionSummary, Telemetry, Workout, WorkoutStep,
};

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
                ",
            )
            .map_err(|error| error.to_string())?;
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
                    "INSERT INTO profiles(id, name, ftp_watts, max_power_watts) VALUES(?1, ?2, ?3, ?4)",
                    params![
                        profile.id.to_string(),
                        profile.name,
                        profile.ftp_watts,
                        profile.max_power_watts
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
                "SELECT id, name, ftp_watts, max_power_watts FROM profiles WHERE active = 1 LIMIT 1",
                [],
                |row| {
                    Ok(Profile {
                        id: parse_uuid(row.get::<_, String>(0)?)?,
                        name: row.get(1)?,
                        ftp_watts: row.get(2)?,
                        max_power_watts: row.get(3)?,
                    })
                },
            )
            .map_err(|error| error.to_string())
    }

    pub fn save_profile(&self, profile: &Profile) -> Result<(), String> {
        if profile.name.trim().is_empty() || !(50..=500).contains(&profile.ftp_watts) {
            return Err("Enter a name and an FTP between 50 and 500 watts".into());
        }
        self.connection()?
            .execute(
                "INSERT INTO profiles(id, name, ftp_watts, max_power_watts, active)
                 VALUES(?1, ?2, ?3, ?4, 1)
                 ON CONFLICT(id) DO UPDATE SET name = excluded.name,
                   ftp_watts = excluded.ftp_watts, max_power_watts = excluded.max_power_watts",
                params![
                    profile.id.to_string(),
                    profile.name,
                    profile.ftp_watts,
                    profile.max_power_watts
                ],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
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

    pub fn start_session(&self, workout: &Workout) -> Result<SessionSummary, String> {
        let summary = SessionSummary {
            id: Uuid::new_v4(),
            workout_id: Some(workout.id),
            workout_name: workout.name.clone(),
            started_at: Utc::now(),
            ended_at: None,
            elapsed_seconds: 0,
            average_power_watts: 0,
            max_power_watts: 0,
            average_cadence_rpm: None,
            completed: false,
        };
        self.connection()?
            .execute(
                "INSERT INTO sessions(id, workout_id, workout_name, started_at)
                 VALUES(?1, ?2, ?3, ?4)",
                params![
                    summary.id.to_string(),
                    workout.id.to_string(),
                    summary.workout_name,
                    summary.started_at.to_rfc3339()
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
                  average_cadence_rpm = ?6, completed = ?7 WHERE id = ?1",
                params![
                    summary.id.to_string(),
                    summary.ended_at.map(|date| date.to_rfc3339()),
                    summary.elapsed_seconds,
                    summary.average_power_watts,
                    summary.max_power_watts,
                    summary.average_cadence_rpm,
                    summary.completed,
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
                 average_power_watts, max_power_watts, average_cadence_rpm, completed
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
                 average_power_watts, max_power_watts, average_cadence_rpm, completed
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
    })
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
}
