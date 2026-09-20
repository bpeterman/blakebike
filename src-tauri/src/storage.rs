use std::{
    fs,
    path::Path,
    sync::{Mutex, MutexGuard},
};

use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use crate::{
    default_workouts::{DEFAULT_WORKOUTS_VERSION, default_workouts, is_legacy_sample},
    devices::{CalibrationRecord, KnownDevice, SourcePreferences},
    distance::estimate_distance,
    domain::{
        DistanceSource, DistanceUnit, PlannedWorkout, Profile, SessionDetail, SessionSummary,
        Telemetry, WeightUnit, Workout, WorkoutOrigin,
    },
    intervals::IntervalsAthlete,
};

const SOURCE_PREFERENCES_KEY: &str = "source_preferences";
const POWER_SMOOTHING_KEY: &str = "power_smoothing";
const INTERVALS_API_KEY: &str = "intervals_api_key";
const TRAINING_ZONES_KEY: &str = "training_zones";
const RIDE_DISPLAY_PREFERENCES_KEY: &str = "ride_display_preferences";
const DEV_MODE_KEY: &str = "dev_mode";
const DEFAULT_WORKOUTS_KEY: &str = "default_workouts_version";
const INTERVALS_ATHLETE_KEY: &str = "intervals_athlete";
const INTERVALS_SYNC_KEY: &str = "intervals_sync";

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

/// Where a zone set comes from. `Derived` follows FTP / max HR, `Custom` was
/// edited by hand, `Intervals` was imported from Intervals.icu. Keeping the
/// last two apart is what lets a sync avoid overwriting hand-tuned zones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ZoneMode {
    #[default]
    Derived,
    Custom,
    Intervals,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TrainingZoneSettings {
    pub version: u8,
    pub sync_power_zones_from_intervals: bool,
    pub sync_heart_rate_zones_from_intervals: bool,
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
            sync_heart_rate_zones_from_intervals: false,
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

pub(crate) fn validate_zones(
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

/// The panels a ride screen can hold — the cards that are not a single
/// number. Mirrors `ridePanelIds` in `src/rideFields.ts`.
const RIDE_PANELS: [&str; 6] = [
    "workoutTimeline",
    "targetAndBias",
    "powerChart",
    "heartRateChart",
    "timeInZone",
    "deviceStats",
];

/// Every field a rider can put on a screen, as `metric:scope:aggregate`.
/// Mirrors the table built in `src/rideFields.ts`; a field missing from here
/// is dropped on load, so the two lists have to agree.
const RIDE_FIELDS: [&str; 33] = [
    "power:ride:current",
    "power:ride:average",
    "power:ride:max",
    "power:interval:average",
    "power:interval:max",
    "cadence:ride:current",
    "cadence:ride:average",
    "cadence:ride:max",
    "cadence:interval:average",
    "cadence:interval:max",
    "heartRate:ride:current",
    "heartRate:ride:average",
    "heartRate:ride:max",
    "heartRate:interval:average",
    "heartRate:interval:max",
    "speed:ride:current",
    "speed:ride:average",
    "speed:ride:max",
    "speed:interval:average",
    "speed:interval:max",
    "wattsPerKilogram:ride:current",
    "wattsPerKilogram:ride:average",
    "wattsPerKilogram:interval:average",
    "energy:ride:total",
    "energy:interval:total",
    "calories:ride:total",
    "calories:interval:total",
    "time:ride:total",
    "time:ride:remaining",
    "time:interval:total",
    "time:interval:remaining",
    "distance:ride:total",
    "targetPower:ride:current",
];

const MAX_RIDE_SCREENS: usize = 8;
const MAX_RIDE_SCREEN_ITEMS: usize = 24;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RideScreenItem {
    #[serde(rename_all = "camelCase")]
    Field {
        field: RideFieldSpec,
        #[serde(default = "one_span")]
        span: u8,
    },
    #[serde(rename_all = "camelCase")]
    Panel { panel: String },
}

fn one_span() -> u8 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RideFieldSpec {
    pub metric: String,
    pub scope: String,
    pub aggregate: String,
}

impl RideFieldSpec {
    fn key(&self) -> String {
        format!("{}:{}:{}", self.metric, self.scope, self.aggregate)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RideScreen {
    pub id: String,
    pub name: String,
    pub items: Vec<RideScreenItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RideDisplayPreferences {
    pub version: u8,
    pub screens: Vec<RideScreen>,
}

/// One screen's worth of the default layout: `(metric, scope, aggregate, span)`
/// fields and panels, in order. Mirrors `defaultRideDisplayPreferences`.
fn default_screens() -> Vec<RideScreen> {
    let field = |key: &str, span: u8| {
        let mut parts = key.split(':');
        RideScreenItem::Field {
            field: RideFieldSpec {
                metric: parts.next().unwrap_or_default().to_owned(),
                scope: parts.next().unwrap_or_default().to_owned(),
                aggregate: parts.next().unwrap_or_default().to_owned(),
            },
            span,
        }
    };
    let panel = |id: &str| RideScreenItem::Panel {
        panel: id.to_owned(),
    };
    vec![
        RideScreen {
            id: "ride".into(),
            name: "Ride".into(),
            items: vec![
                field("power:ride:current", 2),
                field("cadence:ride:current", 1),
                field("heartRate:ride:current", 1),
                field("speed:ride:current", 1),
                field("power:ride:average", 1),
                field("wattsPerKilogram:ride:current", 1),
                field("distance:ride:total", 1),
                field("time:ride:total", 1),
                field("time:ride:remaining", 1),
                panel("workoutTimeline"),
                panel("targetAndBias"),
                panel("powerChart"),
            ],
        },
        RideScreen {
            id: "detail".into(),
            name: "Detail".into(),
            items: vec![
                field("energy:ride:total", 1),
                field("calories:ride:total", 1),
                field("power:interval:average", 1),
                field("heartRate:ride:average", 1),
                panel("heartRateChart"),
                panel("timeInZone"),
            ],
        },
    ]
}

impl Default for RideDisplayPreferences {
    fn default() -> Self {
        Self {
            version: 3,
            screens: default_screens(),
        }
    }
}

/// The v2 cards, each as the field or panel that replaced it.
fn v2_item(id: &str) -> Option<RideScreenItem> {
    let field = |key: &str, span: u8| {
        let mut parts = key.split(':');
        Some(RideScreenItem::Field {
            field: RideFieldSpec {
                metric: parts.next().unwrap_or_default().to_owned(),
                scope: parts.next().unwrap_or_default().to_owned(),
                aggregate: parts.next().unwrap_or_default().to_owned(),
            },
            span,
        })
    };
    match id {
        "power" => field("power:ride:current", 2),
        "cadence" => field("cadence:ride:current", 1),
        "speed" => field("speed:ride:current", 1),
        "heartRate" => field("heartRate:ride:current", 1),
        "energy" => field("energy:ride:total", 1),
        "averagePower" => field("power:ride:average", 1),
        "wattsPerKilogram" => field("wattsPerKilogram:ride:current", 1),
        "distance" => field("distance:ride:total", 1),
        "elapsedTime" => field("time:ride:total", 1),
        "remainingTime" => field("time:ride:remaining", 1),
        other if RIDE_PANELS.contains(&other) => Some(RideScreenItem::Panel {
            panel: other.to_owned(),
        }),
        _ => None,
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct V2Card {
    id: String,
    visible: bool,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct V2Preferences {
    cards: Vec<V2Card>,
}

impl RideDisplayPreferences {
    /// Reads whatever is stored: a v3 document, a v2 card list (migrated to a
    /// single screen holding the cards the rider had switched on), or anything
    /// else, which falls back to the defaults.
    fn from_stored(value: serde_json::Value) -> Self {
        // `screens` has to be present: the struct's serde default would
        // otherwise turn a v2 card list into the default layout and lose it.
        if value.get("screens").is_some()
            && let Ok(preferences) = serde_json::from_value::<Self>(value.clone())
        {
            return preferences.normalized();
        }
        if let Ok(v2) = serde_json::from_value::<V2Preferences>(value) {
            let items = v2
                .cards
                .iter()
                .filter(|card| card.visible)
                .filter_map(|card| v2_item(&card.id))
                .collect();
            return Self {
                version: 3,
                screens: vec![RideScreen {
                    id: "ride".into(),
                    name: "Ride".into(),
                    items,
                }],
            }
            .normalized();
        }
        Self::default()
    }

    /// Drops fields and panels this version does not know, screens with
    /// nothing left on them, and anything past the caps. Mirrors
    /// `normalizeRideDisplayPreferences` in `src/rideScreens.ts`.
    fn normalized(self) -> Self {
        let mut screens: Vec<RideScreen> = Vec::new();
        let mut ids: Vec<String> = Vec::new();
        for screen in self.screens {
            if screens.len() == MAX_RIDE_SCREENS {
                break;
            }
            let mut items: Vec<RideScreenItem> = Vec::new();
            let mut keys: Vec<String> = Vec::new();
            for item in screen.items {
                if items.len() == MAX_RIDE_SCREEN_ITEMS {
                    break;
                }
                let key = match &item {
                    RideScreenItem::Field { field, .. } => {
                        if !RIDE_FIELDS.contains(&field.key().as_str()) {
                            continue;
                        }
                        format!("field:{}", field.key())
                    }
                    RideScreenItem::Panel { panel } => {
                        if !RIDE_PANELS.contains(&panel.as_str()) {
                            continue;
                        }
                        format!("panel:{panel}")
                    }
                };
                if keys.contains(&key) {
                    continue;
                }
                keys.push(key);
                items.push(match item {
                    RideScreenItem::Field { field, span } => RideScreenItem::Field {
                        field,
                        span: if [1, 2, 4].contains(&span) { span } else { 1 },
                    },
                    panel => panel,
                });
            }
            if items.is_empty() {
                continue;
            }
            let mut id = if screen.id.trim().is_empty() {
                format!("screen-{}", screens.len() + 1)
            } else {
                screen.id
            };
            while ids.contains(&id) {
                id = format!("{id}-{}", screens.len() + 1);
            }
            ids.push(id.clone());
            let name = if screen.name.trim().is_empty() {
                format!("Screen {}", screens.len() + 1)
            } else {
                screen.name
            };
            screens.push(RideScreen { id, name, items });
        }
        if screens.is_empty() {
            screens = default_screens();
        }
        Self {
            version: 3,
            screens,
        }
    }
}

/// Which Intervals.icu mirrors run on launch and on "Sync now". Both on by
/// default once a key is saved; the Settings card shows and edits them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct IntervalsSyncSettings {
    pub calendar: bool,
    pub library: bool,
}

impl Default for IntervalsSyncSettings {
    fn default() -> Self {
        Self {
            calendar: true,
            library: true,
        }
    }
}

/// The sync settings plus what the last sync did, so the UI can say "last
/// synced 2 h ago" and show the last error without another request.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct IntervalsSyncState {
    pub settings: IntervalsSyncSettings,
    pub last_synced_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

/// Sample gaps longer than this are pauses (or a dead app) and do not count
/// as riding time. Mirrors `withActiveElapsed` in the ride charts.
const MAX_ACTIVE_GAP_MS: i64 = 5_000;

/// Riding time in whole seconds, from the spacing of the recorded samples.
pub fn active_seconds(samples: &[Telemetry]) -> u32 {
    let active_ms: i64 = samples
        .windows(2)
        .map(|pair| pair[1].timestamp_ms - pair[0].timestamp_ms)
        .filter(|delta| *delta > 0 && *delta <= MAX_ACTIVE_GAP_MS)
        .sum();
    ((active_ms + 500) / 1_000).max(0) as u32
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recovered {
    /// The session got a summary computed from its samples.
    Finalized,
    /// The session had no samples and was removed.
    Deleted,
    /// The session was already finished or does not exist.
    Untouched,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryReport {
    pub finalized: usize,
    pub deleted: usize,
    pub failed: usize,
}

pub struct Storage {
    /// The writer. Every mutation goes through here.
    connection: Mutex<Connection>,
    /// A second connection for reads: WAL lets it run alongside the writer,
    /// so a long History query never holds up the ride recorder.
    reader: Mutex<Connection>,
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
                CREATE TABLE IF NOT EXISTS planned_workouts (
                  event_id INTEGER PRIMARY KEY,
                  workout_uuid TEXT NOT NULL,
                  date TEXT NOT NULL,
                  name TEXT NOT NULL,
                  description TEXT NOT NULL,
                  activity_type TEXT NOT NULL,
                  planned_load INTEGER,
                  duration_seconds INTEGER,
                  workout_json TEXT,
                  parse_error TEXT,
                  intervals_updated TEXT,
                  fetched_at TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS planned_workouts_date_idx
                  ON planned_workouts(date);
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
        ensure_column(&connection, "sessions", "recording_warning", "TEXT")?;
        // Its own column, so a reconnect's payload upsert cannot wipe it.
        ensure_column(
            &connection,
            "known_devices",
            "last_calibration_json",
            "TEXT",
        )?;
        // Mirrored workouts are found again by their Intervals.icu id; local
        // workouts leave it NULL, which a unique index permits any number of.
        ensure_column(&connection, "workouts", "external_id", "TEXT")?;
        connection
            .execute_batch(
                "CREATE UNIQUE INDEX IF NOT EXISTS workouts_external_id_idx
                   ON workouts(external_id);",
            )
            .map_err(|error| error.to_string())?;
        let reader = Connection::open(path).map_err(|error| error.to_string())?;
        let storage = Self {
            connection: Mutex::new(connection),
            reader: Mutex::new(reader),
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

    fn reader(&self) -> Result<MutexGuard<'_, Connection>, String> {
        self.reader
            .lock()
            .map_err(|_| "Database read lock was poisoned".to_string())
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
        drop(connection);
        self.seed_default_workouts()?;
        Ok(())
    }

    /// Installs the built-in library, once per version of it. Existing riders
    /// get the additions too, dated below whatever they already have so their
    /// own workouts keep the top of the list.
    fn seed_default_workouts(&self) -> Result<(), String> {
        let seeded: u32 = self.setting(DEFAULT_WORKOUTS_KEY)?.unwrap_or(0);
        if seeded >= DEFAULT_WORKOUTS_VERSION {
            return Ok(());
        }
        let existing = self.workouts()?;
        for workout in existing.iter().filter(|it| is_legacy_sample(it)) {
            self.delete_workout(workout.id)?;
        }
        let taken: Vec<&str> = existing
            .iter()
            .filter(|it| !is_legacy_sample(it))
            .map(|it| it.name.as_str())
            .collect();
        let newest = existing
            .iter()
            .filter(|it| !is_legacy_sample(it))
            .map(|it| it.updated_at)
            .min()
            .map(|oldest| oldest - Duration::seconds(1))
            .unwrap_or_else(Utc::now);
        for workout in default_workouts(newest) {
            // A rider who already has a workout by this name keeps theirs.
            // Matching on the name is deliberate: rename a default and a later
            // version seeds a fresh copy alongside yours, which is the wanted
            // behaviour — the renamed one is now the rider's own workout.
            if taken.contains(&workout.name.as_str()) {
                continue;
            }
            self.save_workout(&workout)?;
        }
        self.save_setting(DEFAULT_WORKOUTS_KEY, &DEFAULT_WORKOUTS_VERSION)
    }

    pub fn profile(&self) -> Result<Profile, String> {
        self.reader()?
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
        validate_profile(profile)?;
        let connection = self.connection()?;
        write_profile(&connection, profile)
    }

    /// Profile and training zones written together, so an FTP and the zones
    /// scaled from it can never land half-way (a sync from Intervals.icu
    /// changes both at once).
    pub fn save_training_settings(
        &self,
        profile: &Profile,
        zones: &TrainingZoneSettings,
    ) -> Result<(), String> {
        validate_profile(profile)?;
        zones.validate()?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        write_profile(&transaction, profile)?;
        write_setting(&transaction, TRAINING_ZONES_KEY, zones)?;
        transaction.commit().map_err(|error| error.to_string())
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
        let connection = self.connection()?;
        write_setting(&connection, key, value)
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

    /// Forget everything Intervals.icu: the key, the athlete, the sync state,
    /// the calendar cache and the mirrored library (none of it can be kept
    /// current without the key). Returns how many mirrored workouts went.
    pub fn clear_intervals(&self) -> Result<usize, String> {
        self.delete_setting(INTERVALS_API_KEY)?;
        self.delete_setting(INTERVALS_ATHLETE_KEY)?;
        self.delete_setting(INTERVALS_SYNC_KEY)?;
        self.clear_planned_workouts()?;
        self.delete_mirrored_workouts_not_in(&[])
    }

    fn delete_setting(&self, key: &str) -> Result<(), String> {
        self.connection()?
            .execute("DELETE FROM settings WHERE key = ?1", params![key])
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn intervals_athlete(&self) -> Result<Option<IntervalsAthlete>, String> {
        self.setting(INTERVALS_ATHLETE_KEY)
    }

    pub fn save_intervals_athlete(&self, athlete: &IntervalsAthlete) -> Result<(), String> {
        self.save_setting(INTERVALS_ATHLETE_KEY, athlete)
    }

    pub fn intervals_sync_state(&self) -> Result<IntervalsSyncState, String> {
        Ok(self.setting(INTERVALS_SYNC_KEY)?.unwrap_or_default())
    }

    pub fn save_intervals_sync_state(&self, state: &IntervalsSyncState) -> Result<(), String> {
        self.save_setting(INTERVALS_SYNC_KEY, state)
    }

    /// Replace the whole calendar cache with a fresh fetch, atomically, so a
    /// reader never sees half of two syncs.
    pub fn replace_planned_workouts(&self, planned: &[PlannedWorkout]) -> Result<(), String> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        transaction
            .execute("DELETE FROM planned_workouts", [])
            .map_err(|error| error.to_string())?;
        for row in planned {
            let workout_json = row
                .workout
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|error| error.to_string())?;
            transaction
                .execute(
                    "INSERT INTO planned_workouts(
                       event_id, workout_uuid, date, name, description, activity_type,
                       planned_load, duration_seconds, workout_json, parse_error,
                       intervals_updated, fetched_at
                     ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    params![
                        row.event_id,
                        row.workout_id.to_string(),
                        row.date.to_string(),
                        row.name,
                        row.description,
                        row.activity_type,
                        row.planned_load,
                        row.duration_seconds,
                        workout_json,
                        row.parse_error,
                        row.updated,
                        row.fetched_at.to_rfc3339(),
                    ],
                )
                .map_err(|error| error.to_string())?;
        }
        transaction.commit().map_err(|error| error.to_string())
    }

    /// The cached calendar, oldest first. The UI picks today by local date.
    pub fn planned_workouts(&self) -> Result<Vec<PlannedWorkout>, String> {
        let connection = self.reader()?;
        let mut statement = connection
            .prepare(&format!(
                "SELECT {PLANNED_WORKOUT_COLUMNS} FROM planned_workouts ORDER BY date, event_id"
            ))
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], planned_workout_from_row)
            .map_err(|error| error.to_string())?;
        rows.map(|row| row.map_err(|error| error.to_string()))
            .collect()
    }

    /// A planned workout by the stable id the UI and the runner use.
    pub fn planned_workout(&self, workout_id: Uuid) -> Result<Option<PlannedWorkout>, String> {
        self.reader()?
            .query_row(
                &format!(
                    "SELECT {PLANNED_WORKOUT_COLUMNS} FROM planned_workouts WHERE workout_uuid = ?1"
                ),
                [workout_id.to_string()],
                planned_workout_from_row,
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    pub fn clear_planned_workouts(&self) -> Result<(), String> {
        self.connection()?
            .execute("DELETE FROM planned_workouts", [])
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// The mirrored workout for an Intervals.icu id (`WorkoutOrigin::external_key`).
    pub fn workout_by_external_key(&self, key: &str) -> Result<Option<Workout>, String> {
        let payload = self
            .reader()?
            .query_row(
                "SELECT payload_json FROM workouts WHERE external_id = ?1",
                [key],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        payload
            .map(|json| serde_json::from_str(&json).map_err(|error| error.to_string()))
            .transpose()
    }

    /// Every workout mirrored from Intervals.icu.
    pub fn mirrored_workouts(&self) -> Result<Vec<Workout>, String> {
        let connection = self.reader()?;
        let mut statement = connection
            .prepare("SELECT payload_json FROM workouts WHERE external_id IS NOT NULL")
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

    /// Drop mirrored workouts that are no longer in the Intervals.icu library.
    /// Local workouts are never touched. Returns how many were removed.
    pub fn delete_mirrored_workouts_not_in(&self, keep: &[String]) -> Result<usize, String> {
        let mut removed = 0;
        for workout in self.mirrored_workouts()? {
            let key = workout
                .origin
                .as_ref()
                .map(WorkoutOrigin::external_key)
                .unwrap_or_default();
            if !keep.contains(&key) {
                self.delete_workout(workout.id)?;
                removed += 1;
            }
        }
        Ok(removed)
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

    /// Developer mode. Off by default; while it is on the simulated sensors
    /// are offered alongside real hardware.
    pub fn dev_mode(&self) -> Result<bool, String> {
        Ok(self.setting(DEV_MODE_KEY)?.unwrap_or(false))
    }

    pub fn save_dev_mode(&self, enabled: bool) -> Result<(), String> {
        self.save_setting(DEV_MODE_KEY, &enabled)
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
            .setting::<serde_json::Value>(RIDE_DISPLAY_PREFERENCES_KEY)?
            .map(RideDisplayPreferences::from_stored)
            .unwrap_or_default())
    }

    pub fn save_ride_display_preferences(
        &self,
        preferences: &RideDisplayPreferences,
    ) -> Result<(), String> {
        self.save_setting(
            RIDE_DISPLAY_PREFERENCES_KEY,
            &preferences.clone().normalized(),
        )
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

    /// Record a device's latest calibration. Only touches its own column, so
    /// `remember_device` can keep replacing the payload on every connect.
    /// A device that was never remembered has nowhere to keep it; that is
    /// reported so the caller can log it, not an error worth failing the
    /// calibration over.
    pub fn record_calibration(&self, id: &str, record: &CalibrationRecord) -> Result<bool, String> {
        let json = serde_json::to_string(record).map_err(|error| error.to_string())?;
        let updated = self
            .connection()?
            .execute(
                "UPDATE known_devices SET last_calibration_json = ?2 WHERE id = ?1",
                params![id, json],
            )
            .map_err(|error| error.to_string())?;
        Ok(updated > 0)
    }

    /// Remembered devices, most recently connected first.
    pub fn known_devices(&self) -> Result<Vec<KnownDevice>, String> {
        let connection = self.reader()?;
        let mut statement = connection
            .prepare(
                "SELECT payload_json, last_calibration_json FROM known_devices
                 ORDER BY last_connected_at DESC",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })
            .map_err(|error| error.to_string())?;
        let mut devices = Vec::new();
        for row in rows {
            let (json, calibration_json) = row.map_err(|error| error.to_string())?;
            match serde_json::from_str::<KnownDevice>(&json) {
                Ok(mut device) => {
                    // The column is the source of truth; the payload's copy is
                    // whatever the hub knew when it last connected.
                    device.last_calibration = calibration_json.and_then(|json| {
                        serde_json::from_str::<CalibrationRecord>(&json)
                            .inspect_err(|error| {
                                tracing::warn!(id = %device.id, error = %error, "Skipping unreadable calibration record")
                            })
                            .ok()
                    });
                    devices.push(device);
                }
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
        let connection = self.reader()?;
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
        self.reader()?
            .execute(
                "INSERT INTO workouts(id, name, source, version, payload_json, created_at, updated_at, external_id)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(id) DO UPDATE SET name = excluded.name, source = excluded.source,
                   version = excluded.version, payload_json = excluded.payload_json,
                   updated_at = excluded.updated_at, external_id = excluded.external_id",
                params![
                    workout.id.to_string(),
                    workout.name,
                    workout.source,
                    workout.version,
                    payload,
                    workout.created_at.to_rfc3339(),
                    workout.updated_at.to_rfc3339(),
                    workout.origin.as_ref().map(WorkoutOrigin::external_key),
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
            recording_warning: None,
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

    /// One sample; rides use `record_samples` in batches.
    #[cfg(test)]
    pub fn record_sample(&self, session_id: Uuid, sample: &Telemetry) -> Result<(), String> {
        self.record_samples(session_id, std::slice::from_ref(sample))
    }

    /// Insert a batch of samples in one transaction (the ride recorder writes
    /// about once a second). Same upsert semantics as `record_sample`.
    pub fn record_samples(&self, session_id: Uuid, samples: &[Telemetry]) -> Result<(), String> {
        if samples.is_empty() {
            return Ok(());
        }
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        {
            let mut statement = transaction
                .prepare_cached(
                    "INSERT OR REPLACE INTO telemetry_samples(session_id, timestamp_ms, payload_json)
                     VALUES(?1, ?2, ?3)",
                )
                .map_err(|error| error.to_string())?;
            let session = session_id.to_string();
            for sample in samples {
                let json = serde_json::to_string(sample).map_err(|error| error.to_string())?;
                statement
                    .execute(params![session, sample.timestamp_ms, json])
                    .map_err(|error| error.to_string())?;
            }
        }
        transaction.commit().map_err(|error| error.to_string())
    }

    pub fn finish_session(&self, summary: &SessionSummary) -> Result<(), String> {
        self.connection()?
            .execute(
                "UPDATE sessions SET ended_at = ?2, elapsed_seconds = ?3,
                  average_power_watts = ?4, max_power_watts = ?5,
                  average_cadence_rpm = ?6, completed = ?7,
                  estimated_distance_meters = ?8, distance_source = ?9,
                  distance_weight_kg = ?10, recording_warning = ?11 WHERE id = ?1",
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
                    summary.recording_warning,
                ],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// Retain a save problem in History, including failures after finalization.
    pub fn save_recording_warning(&self, id: Uuid, warning: Option<&str>) -> Result<(), String> {
        self.connection()?
            .execute(
                "UPDATE sessions SET recording_warning = ?2 WHERE id = ?1",
                params![id.to_string(), warning],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// Sessions that never got a summary: the app died, or a ride aborted
    /// before it was closed out.
    pub fn unfinished_sessions(&self) -> Result<Vec<Uuid>, String> {
        let connection = self.reader()?;
        let mut statement = connection
            .prepare("SELECT id FROM sessions WHERE ended_at IS NULL ORDER BY started_at")
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?;
        let mut ids = Vec::new();
        for row in rows {
            let id = row.map_err(|error| error.to_string())?;
            match Uuid::parse_str(&id) {
                Ok(id) => ids.push(id),
                Err(error) => tracing::warn!(%id, %error, "Skipping session with an unreadable id"),
            }
        }
        Ok(ids)
    }

    /// Close every unfinished session from its recorded samples (startup
    /// crash recovery). Never fails as a whole; per-session errors are counted.
    pub fn finalize_orphaned_sessions(&self) -> Result<RecoveryReport, String> {
        let mut report = RecoveryReport::default();
        for id in self.unfinished_sessions()? {
            match self.finalize_orphaned_session(id) {
                Ok(Recovered::Finalized) => report.finalized += 1,
                Ok(Recovered::Deleted) => report.deleted += 1,
                Ok(Recovered::Untouched) => {}
                Err(error) => {
                    tracing::warn!(session_id = %id, %error, "Could not recover unfinished session");
                    report.failed += 1;
                }
            }
        }
        Ok(report)
    }

    /// Give one unfinished session the summary `finish_session` would have
    /// written: riding time from the samples, averages, distance, and
    /// `ended_at` at the last sample. Sessions without samples are deleted;
    /// finished sessions are left alone.
    pub fn finalize_orphaned_session(&self, id: Uuid) -> Result<Recovered, String> {
        let Some(detail) = self.session(id)? else {
            return Ok(Recovered::Untouched);
        };
        if detail.summary.ended_at.is_some() {
            return Ok(Recovered::Untouched);
        }
        if detail.samples.is_empty() {
            self.delete_session(id)?;
            tracing::info!(session_id = %id, "Deleted unfinished session without samples");
            return Ok(Recovered::Deleted);
        }
        let samples = &detail.samples;
        let mut summary = detail.summary;
        summary.elapsed_seconds = active_seconds(samples);
        summary.average_power_watts = (samples
            .iter()
            .map(|sample| u64::from(sample.power_watts))
            .sum::<u64>()
            / samples.len() as u64) as u16;
        summary.max_power_watts = samples
            .iter()
            .map(|sample| sample.power_watts)
            .max()
            .unwrap_or(0);
        let cadences: Vec<f64> = samples
            .iter()
            .filter_map(|sample| sample.cadence_rpm)
            .map(f64::from)
            .collect();
        summary.average_cadence_rpm = (!cadences.is_empty())
            .then(|| (cadences.iter().sum::<f64>() / cadences.len() as f64) as f32);
        let estimate = estimate_distance(samples, summary.distance_weight_kg);
        summary.estimated_distance_meters = estimate.total_meters;
        summary.distance_source = estimate.source;
        let last_ms = samples
            .last()
            .map(|sample| sample.timestamp_ms)
            .unwrap_or_default();
        let ended_at = Utc
            .timestamp_millis_opt(last_ms)
            .single()
            .unwrap_or(summary.started_at)
            .max(summary.started_at);
        summary.ended_at = Some(ended_at);
        summary.completed = false;
        self.finish_session(&summary)?;
        tracing::info!(
            session_id = %id,
            elapsed_seconds = summary.elapsed_seconds,
            samples = samples.len(),
            "Recovered unfinished session"
        );
        Ok(Recovered::Finalized)
    }

    pub fn delete_session(&self, id: Uuid) -> Result<(), String> {
        self.connection()?
            .execute("DELETE FROM sessions WHERE id = ?1", [id.to_string()])
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
        let connection = self.reader()?;
        let mut statement = connection
            .prepare(
                "SELECT id, workout_id, workout_name, started_at, ended_at, elapsed_seconds,
                 average_power_watts, max_power_watts, average_cadence_rpm, completed,
                 estimated_distance_meters, distance_source, distance_weight_kg, recording_warning
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
        let connection = self.reader()?;
        let summary = connection
            .query_row(
                "SELECT id, workout_id, workout_name, started_at, ended_at, elapsed_seconds,
                 average_power_watts, max_power_watts, average_cadence_rpm, completed,
                 estimated_distance_meters, distance_source, distance_weight_kg, recording_warning
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
        recording_warning: row.get(13)?,
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

fn validate_profile(profile: &Profile) -> Result<(), String> {
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
    Ok(())
}

/// Upsert the active profile on any connection or transaction.
fn write_profile(connection: &Connection, profile: &Profile) -> Result<(), String> {
    connection
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

/// Upsert one JSON setting on any connection or transaction.
fn write_setting<T: serde::Serialize>(
    connection: &Connection,
    key: &str,
    value: &T,
) -> Result<(), String> {
    let json = serde_json::to_string(value).map_err(|error| error.to_string())?;
    connection
        .execute(
            "INSERT INTO settings(key, value_json) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
            params![key, json],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

const PLANNED_WORKOUT_COLUMNS: &str = "event_id, workout_uuid, date, name, description, activity_type, planned_load, \
     duration_seconds, workout_json, parse_error, intervals_updated, fetched_at";

fn planned_workout_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PlannedWorkout> {
    let workout_json: Option<String> = row.get(8)?;
    let workout = workout_json
        .map(|json| {
            serde_json::from_str::<Workout>(&json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    8,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })
        })
        .transpose()?;
    let date: String = row.get(2)?;
    let date = NaiveDate::parse_from_str(&date, "%Y-%m-%d").map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(PlannedWorkout {
        event_id: row.get(0)?,
        workout_id: parse_uuid(row.get::<_, String>(1)?)?,
        date,
        name: row.get(3)?,
        description: row.get(4)?,
        activity_type: row.get(5)?,
        planned_load: row.get(6)?,
        duration_seconds: row.get(7)?,
        workout,
        parse_error: row.get(9)?,
        updated: row.get(10)?,
        fetched_at: parse_timestamp(row.get::<_, String>(11)?)?,
    })
}

fn parse_timestamp(value: String) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value)
        .map(|stamp| stamp.with_timezone(&Utc))
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                value.len(),
                rusqlite::types::Type::Text,
                Box::new(error),
            )
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::default_workouts::legacy_sample_steps;
    use crate::domain::{PowerTarget, WorkoutStep};

    fn free_ride(duration_seconds: u32) -> WorkoutStep {
        WorkoutStep::FreeRide { duration_seconds }
    }

    #[test]
    fn initializes_and_round_trips_workouts() {
        let storage = Storage::in_memory().unwrap();
        let workouts = storage.workouts().unwrap();
        assert_eq!(workouts.len(), default_workouts(Utc::now()).len());
        assert!(storage.workout(workouts[0].id).unwrap().is_some());
    }

    /// The library list and the home screen both read `updated_at DESC`.
    #[test]
    fn a_fresh_library_leads_with_the_ftp_test() {
        let storage = Storage::in_memory().unwrap();
        let names: Vec<String> = storage
            .workouts()
            .unwrap()
            .into_iter()
            .map(|workout| workout.name)
            .collect();
        let expected: Vec<String> = default_workouts(Utc::now())
            .into_iter()
            .map(|workout| workout.name)
            .collect();
        assert_eq!(names, expected);
    }

    #[test]
    fn seeding_the_defaults_happens_only_once() {
        let path = std::env::temp_dir().join(format!("blakebike-{}.sqlite", Uuid::new_v4()));
        let storage = Storage::open(&path).unwrap();
        let seeded = storage.workouts().unwrap();
        let removed = seeded
            .iter()
            .find(|workout| workout.name == "Recovery Spin")
            .unwrap();
        storage.delete_workout(removed.id).unwrap();
        drop(storage);

        let reopened = Storage::open(&path).unwrap();
        assert_eq!(reopened.workouts().unwrap().len(), seeded.len() - 1);
    }

    #[test]
    fn upgrading_keeps_the_riders_workouts_on_top_and_drops_the_old_sample() {
        let path = std::env::temp_dir().join(format!("blakebike-{}.sqlite", Uuid::new_v4()));
        let storage = Storage::open(&path).unwrap();
        for workout in storage.workouts().unwrap() {
            storage.delete_workout(workout.id).unwrap();
        }
        storage
            .save_workout(&Workout::new("FTP Builder", legacy_sample_steps()))
            .unwrap();
        storage
            .save_workout(&Workout::new("Recovery Spin", vec![free_ride(600)]))
            .unwrap();
        let mine = Workout::new("Tuesday nights", vec![free_ride(1800)]);
        storage.save_workout(&mine).unwrap();
        // Pretend this database predates the built-in library.
        storage
            .connection()
            .unwrap()
            .execute(
                "DELETE FROM settings WHERE key = ?1",
                params![DEFAULT_WORKOUTS_KEY],
            )
            .unwrap();
        drop(storage);

        let reopened = Storage::open(&path).unwrap();
        let names: Vec<String> = reopened
            .workouts()
            .unwrap()
            .into_iter()
            .map(|workout| workout.name)
            .collect();
        assert!(!names.contains(&"FTP Builder".to_string()));
        assert_eq!(&names[..2], ["Tuesday nights", "Recovery Spin"]);
        assert_eq!(names[2], "FTP Test (20 min)");
        // The rider's own "Recovery Spin" was not duplicated by the default.
        assert_eq!(
            names.iter().filter(|name| *name == "Recovery Spin").count(),
            1
        );
        // Both of the rider's workouts, plus every default but the skipped one.
        assert_eq!(names.len(), default_workouts(Utc::now()).len() + 1);
    }

    /// Renaming a default makes it the rider's own, so the next version of the
    /// library is free to seed a fresh copy under the original name.
    #[test]
    fn a_renamed_default_does_not_block_a_later_reseed() {
        let path = std::env::temp_dir().join(format!("blakebike-{}.sqlite", Uuid::new_v4()));
        let storage = Storage::open(&path).unwrap();
        let mut renamed = storage
            .workouts()
            .unwrap()
            .into_iter()
            .find(|workout| workout.name == "Recovery Spin")
            .unwrap();
        renamed.name = "My easy spin".into();
        renamed.updated_at = Utc::now();
        storage.save_workout(&renamed).unwrap();
        // Stand in for a future DEFAULT_WORKOUTS_VERSION bump.
        storage.save_setting(DEFAULT_WORKOUTS_KEY, &0u32).unwrap();

        storage.seed_default_workouts().unwrap();

        let names: Vec<String> = storage
            .workouts()
            .unwrap()
            .into_iter()
            .map(|workout| workout.name)
            .collect();
        assert!(names.contains(&"My easy spin".to_string()));
        assert!(names.contains(&"Recovery Spin".to_string()));
        assert_eq!(names.len(), default_workouts(Utc::now()).len() + 1);
    }

    /// An edited copy of the old sample is the rider's work, not ours.
    #[test]
    fn upgrading_leaves_an_edited_ftp_builder_alone() {
        let path = std::env::temp_dir().join(format!("blakebike-{}.sqlite", Uuid::new_v4()));
        let storage = Storage::open(&path).unwrap();
        let mut edited = Workout::new("FTP Builder", legacy_sample_steps());
        edited.steps.push(free_ride(300));
        storage.save_workout(&edited).unwrap();
        storage
            .connection()
            .unwrap()
            .execute(
                "DELETE FROM settings WHERE key = ?1",
                params![DEFAULT_WORKOUTS_KEY],
            )
            .unwrap();
        drop(storage);

        let reopened = Storage::open(&path).unwrap();
        assert!(reopened.workout(edited.id).unwrap().is_some());
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
    fn dev_mode_defaults_to_off_and_round_trips() {
        let storage = Storage::in_memory().unwrap();
        assert!(!storage.dev_mode().unwrap());
        storage.save_dev_mode(true).unwrap();
        assert!(storage.dev_mode().unwrap());
        storage.save_dev_mode(false).unwrap();
        assert!(!storage.dev_mode().unwrap());
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
        assert!(!TrainingZoneSettings::default().sync_power_zones_from_intervals);
        assert!(!TrainingZoneSettings::default().sync_heart_rate_zones_from_intervals);
        assert_eq!(
            storage.ride_display_preferences().unwrap(),
            RideDisplayPreferences::default()
        );
        let zones = TrainingZoneSettings {
            sync_power_zones_from_intervals: true,
            sync_heart_rate_zones_from_intervals: true,
            power_mode: ZoneMode::Intervals,
            power_zones: vec![
                ZoneDefinition {
                    name: "Endurance".into(),
                    upper_bound: Some(200),
                },
                ZoneDefinition {
                    name: "Threshold".into(),
                    upper_bound: None,
                },
            ],
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
        // Imported zones are validated like custom ones.
        let bad_import = TrainingZoneSettings {
            power_mode: ZoneMode::Intervals,
            power_zones: vec![ZoneDefinition {
                name: "Only".into(),
                upper_bound: None,
            }],
            ..TrainingZoneSettings::default()
        };
        assert!(storage.save_training_zones(&bad_import).is_err());
        // Settings written before the heart-rate toggle existed load with it off.
        storage
            .save_setting(
                TRAINING_ZONES_KEY,
                &serde_json::json!({
                    "version": 1,
                    "syncPowerZonesFromIntervals": true,
                    "powerMode": "derived",
                    "powerZones": [],
                    "heartRateMode": "derived",
                    "heartRateZones": []
                }),
            )
            .unwrap();
        let loaded = storage.training_zones().unwrap();
        assert!(loaded.sync_power_zones_from_intervals);
        assert!(!loaded.sync_heart_rate_zones_from_intervals);
        let mut display = RideDisplayPreferences::default();
        display.screens.swap(0, 1);
        display.screens[0].items.truncate(2);
        storage.save_ride_display_preferences(&display).unwrap();
        assert_eq!(storage.ride_display_preferences().unwrap(), display);
    }

    #[test]
    fn ride_display_preferences_fall_back_and_normalize() {
        let storage = Storage::in_memory().unwrap();
        storage
            .save_setting(
                RIDE_DISPLAY_PREFERENCES_KEY,
                &serde_json::json!({ "showTimeInZone": false }),
            )
            .unwrap();
        assert_eq!(
            storage.ride_display_preferences().unwrap(),
            RideDisplayPreferences::default()
        );

        // A field this build does not know, a duplicate, an impossible span
        // and a screen left with nothing on it are all cleaned up on load.
        storage
            .save_setting(
                RIDE_DISPLAY_PREFERENCES_KEY,
                &serde_json::json!({
                    "version": 3,
                    "screens": [
                        {
                            "id": "main",
                            "name": "",
                            "items": [
                                { "kind": "field", "field": { "metric": "power", "scope": "ride", "aggregate": "current" }, "span": 3 },
                                { "kind": "field", "field": { "metric": "power", "scope": "ride", "aggregate": "current" }, "span": 1 },
                                { "kind": "field", "field": { "metric": "vo2max", "scope": "ride", "aggregate": "current" }, "span": 1 },
                                { "kind": "panel", "panel": "spaceship" }
                            ]
                        },
                        { "id": "empty", "name": "Empty", "items": [] }
                    ]
                }),
            )
            .unwrap();
        let loaded = storage.ride_display_preferences().unwrap();
        assert_eq!(loaded.version, 3);
        assert_eq!(loaded.screens.len(), 1);
        assert_eq!(loaded.screens[0].name, "Screen 1");
        assert_eq!(
            loaded.screens[0].items,
            vec![RideScreenItem::Field {
                field: RideFieldSpec {
                    metric: "power".into(),
                    scope: "ride".into(),
                    aggregate: "current".into(),
                },
                span: 1,
            }]
        );
    }

    #[test]
    fn a_v2_card_layout_becomes_one_ride_screen() {
        let storage = Storage::in_memory().unwrap();
        storage
            .save_setting(
                RIDE_DISPLAY_PREFERENCES_KEY,
                &serde_json::json!({
                    "version": 2,
                    "cards": [
                        { "id": "speed", "visible": true },
                        { "id": "power", "visible": true },
                        { "id": "deviceStats", "visible": false },
                        { "id": "timeInZone", "visible": true },
                        { "id": "unknown", "visible": true }
                    ]
                }),
            )
            .unwrap();
        let migrated = storage.ride_display_preferences().unwrap();
        assert_eq!(migrated.version, 3);
        assert_eq!(migrated.screens.len(), 1);
        assert_eq!(migrated.screens[0].name, "Ride");
        // The rider's order is kept, hidden cards are dropped, and the power
        // card keeps the double width it has on the ride screen.
        assert_eq!(
            migrated.screens[0].items,
            vec![
                RideScreenItem::Field {
                    field: RideFieldSpec {
                        metric: "speed".into(),
                        scope: "ride".into(),
                        aggregate: "current".into(),
                    },
                    span: 1,
                },
                RideScreenItem::Field {
                    field: RideFieldSpec {
                        metric: "power".into(),
                        scope: "ride".into(),
                        aggregate: "current".into(),
                    },
                    span: 2,
                },
                RideScreenItem::Panel {
                    panel: "timeInZone".into(),
                },
            ]
        );
    }

    #[test]
    fn ride_screens_round_trip() {
        let storage = Storage::in_memory().unwrap();
        let mut chosen = RideDisplayPreferences::default();
        chosen.screens.truncate(1);
        chosen.screens[0].items.push(RideScreenItem::Panel {
            panel: "deviceStats".into(),
        });
        storage.save_ride_display_preferences(&chosen).unwrap();
        assert_eq!(storage.ride_display_preferences().unwrap(), chosen);
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

        assert_eq!(storage.clear_intervals().unwrap(), 0);
        assert_eq!(storage.intervals_api_key().unwrap(), None);
        assert!(storage.save_intervals_api_key("  ").is_err());
    }

    #[test]
    fn planned_workouts_round_trip_and_replace_atomically() {
        let storage = Storage::in_memory().unwrap();
        assert_eq!(storage.planned_workouts().unwrap(), vec![]);
        let structured = Workout::new(
            "Sweet Spot",
            vec![WorkoutStep::Steady {
                duration_seconds: 600,
                target: PowerTarget::PercentFtp(88),
            }],
        );
        let date = NaiveDate::from_ymd_opt(2026, 9, 21).unwrap();
        let fetched_at = Utc.with_ymd_and_hms(2026, 9, 20, 12, 0, 0).unwrap();
        let rows = vec![
            PlannedWorkout {
                event_id: 501,
                workout_id: structured.id,
                date,
                name: "Sweet Spot".into(),
                description: "3x12".into(),
                activity_type: "Ride".into(),
                planned_load: Some(68),
                duration_seconds: Some(600),
                workout: Some(structured.clone()),
                parse_error: None,
                updated: Some("2026-09-20T10:00:00".into()),
                fetched_at,
            },
            PlannedWorkout {
                event_id: 502,
                workout_id: Uuid::new_v4(),
                date: date.succ_opt().unwrap(),
                name: "Easy".into(),
                description: String::new(),
                activity_type: "VirtualRide".into(),
                planned_load: None,
                duration_seconds: None,
                workout: None,
                parse_error: Some("Unsupported workout element <Sprint>".into()),
                updated: None,
                fetched_at,
            },
        ];
        storage.replace_planned_workouts(&rows).unwrap();
        assert_eq!(storage.planned_workouts().unwrap(), rows);
        assert_eq!(
            storage.planned_workout(structured.id).unwrap(),
            Some(rows[0].clone())
        );
        assert_eq!(storage.planned_workout(Uuid::new_v4()).unwrap(), None);

        // A replace drops what is no longer planned.
        storage.replace_planned_workouts(&rows[1..]).unwrap();
        assert_eq!(storage.planned_workouts().unwrap(), rows[1..].to_vec());
        storage.clear_planned_workouts().unwrap();
        assert_eq!(storage.planned_workouts().unwrap(), vec![]);
    }

    #[test]
    fn mirrored_workouts_are_found_by_external_id_and_local_ones_are_left_alone() {
        let storage = Storage::in_memory().unwrap();
        let local_count = storage.workouts().unwrap().len();
        let mut mirrored = Workout::new(
            "Threshold 2x20",
            vec![WorkoutStep::Steady {
                duration_seconds: 1200,
                target: PowerTarget::PercentFtp(98),
            }],
        );
        mirrored.source = "intervals".into();
        mirrored.origin = Some(WorkoutOrigin {
            external_id: 7,
            folder_id: Some(3),
            folder: Some("Base".into()),
            updated: "2026-09-01T08:00:00".into(),
            planned_load: Some(92),
        });
        storage.save_workout(&mirrored).unwrap();
        assert_eq!(
            storage.workout_by_external_key("intervals:7").unwrap(),
            Some(mirrored.clone())
        );
        assert_eq!(
            storage.workout_by_external_key("intervals:8").unwrap(),
            None
        );
        assert_eq!(storage.mirrored_workouts().unwrap(), vec![mirrored.clone()]);

        // Re-saving under the same external id keeps the local uuid.
        let mut refreshed = mirrored.clone();
        refreshed.name = "Threshold 2×20".into();
        storage.save_workout(&refreshed).unwrap();
        assert_eq!(
            storage
                .workout_by_external_key("intervals:7")
                .unwrap()
                .unwrap()
                .id,
            mirrored.id
        );

        // Two mirrors cannot share an external id.
        let mut duplicate = mirrored.clone();
        duplicate.id = Uuid::new_v4();
        assert!(storage.save_workout(&duplicate).is_err());

        assert_eq!(
            storage
                .delete_mirrored_workouts_not_in(&["intervals:7".into()])
                .unwrap(),
            0
        );
        assert_eq!(storage.delete_mirrored_workouts_not_in(&[]).unwrap(), 1);
        assert_eq!(storage.workouts().unwrap().len(), local_count);
        // Payloads saved before `origin` existed still load.
        assert!(
            storage
                .workouts()
                .unwrap()
                .iter()
                .all(|workout| !workout.is_mirrored())
        );
    }

    #[test]
    fn training_settings_save_together_or_not_at_all() {
        let storage = Storage::in_memory().unwrap();
        let mut profile = storage.profile().unwrap();
        profile.ftp_watts = 265;
        let zones = TrainingZoneSettings {
            power_mode: ZoneMode::Intervals,
            power_zones: vec![
                ZoneDefinition {
                    name: "Endurance".into(),
                    upper_bound: Some(199),
                },
                ZoneDefinition {
                    name: "Threshold".into(),
                    upper_bound: None,
                },
            ],
            ..TrainingZoneSettings::default()
        };
        storage.save_training_settings(&profile, &zones).unwrap();
        assert_eq!(storage.profile().unwrap().ftp_watts, 265);
        assert_eq!(storage.training_zones().unwrap(), zones);

        // An invalid profile rejects the whole save; the zones stay as they were.
        let mut bad_profile = profile.clone();
        bad_profile.ftp_watts = 20;
        assert!(
            storage
                .save_training_settings(&bad_profile, &TrainingZoneSettings::default())
                .is_err()
        );
        assert_eq!(storage.profile().unwrap().ftp_watts, 265);
        assert_eq!(storage.training_zones().unwrap(), zones);
    }

    #[test]
    fn remembers_and_forgets_devices() {
        use crate::devices::{Capability, DeviceRole};
        let storage = Storage::in_memory().unwrap();
        assert!(storage.known_devices().unwrap().is_empty());
        let strap = KnownDevice {
            id: "strap".into(),
            name: "HRM-Pro".into(),
            transport: Default::default(),
            role: DeviceRole::HeartRate,
            capabilities: vec![Capability::HeartRate],
            simulated: false,
            manufacturer: Some("Garmin".into()),
            model: None,
            last_connected_at: Utc::now() - chrono::Duration::minutes(5),
            last_calibration: None,
        };
        let trainer = KnownDevice {
            id: "kickr".into(),
            name: "KICKR CORE".into(),
            transport: Default::default(),
            role: DeviceRole::Trainer,
            capabilities: vec![Capability::Ftms, Capability::CyclingPower],
            simulated: false,
            manufacturer: Some("Wahoo".into()),
            model: Some("KICKR CORE".into()),
            last_connected_at: Utc::now(),
            last_calibration: None,
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
    fn calibration_records_survive_reconnects_and_only_attach_to_remembered_devices() {
        use crate::devices::{CalibrationKind, Capability, DeviceRole};
        let storage = Storage::in_memory().unwrap();
        let meter = KnownDevice {
            id: "pm".into(),
            name: "Assioma".into(),
            transport: Default::default(),
            role: DeviceRole::Power,
            capabilities: vec![Capability::CyclingPower],
            simulated: false,
            manufacturer: Some("Favero".into()),
            model: None,
            last_connected_at: Utc::now(),
            last_calibration: None,
        };
        let record = CalibrationRecord {
            at: Utc::now(),
            kind: CalibrationKind::ZeroOffset,
            offset_raw: Some(1_023),
        };
        // Nothing remembered yet: nowhere to keep it, reported as such.
        assert!(!storage.record_calibration("pm", &record).unwrap());
        storage.remember_device(&meter).unwrap();
        assert!(storage.record_calibration("pm", &record).unwrap());
        assert_eq!(
            storage.known_devices().unwrap()[0].last_calibration,
            Some(record.clone())
        );
        // Reconnecting rewrites the payload (with whatever the hub knew, here
        // nothing) but must not lose the record.
        storage
            .remember_device(&KnownDevice {
                last_connected_at: Utc::now() + Duration::minutes(1),
                model: Some("Assioma DUO".into()),
                ..meter.clone()
            })
            .unwrap();
        let known = storage.known_devices().unwrap();
        assert_eq!(known[0].model.as_deref(), Some("Assioma DUO"));
        assert_eq!(known[0].last_calibration, Some(record.clone()));
        // A later zero replaces it.
        let newer = CalibrationRecord {
            offset_raw: Some(1_019),
            ..record.clone()
        };
        storage.record_calibration("pm", &newer).unwrap();
        assert_eq!(
            storage.known_devices().unwrap()[0]
                .last_calibration
                .as_ref()
                .and_then(|record| record.offset_raw),
            Some(1_019)
        );
        // Forgetting drops it with the device.
        storage.forget_device("pm").unwrap();
        storage.remember_device(&meter).unwrap();
        assert_eq!(storage.known_devices().unwrap()[0].last_calibration, None);
    }

    #[test]
    fn records_sample_batches_in_one_transaction() {
        let storage = Storage::in_memory().unwrap();
        let session = storage.start_session(None, "Batch", 84.0).unwrap();
        let samples: Vec<Telemetry> = (0..5)
            .map(|index| Telemetry {
                timestamp_ms: 1_700_000_000_000 + index * 200,
                power_watts: 100 + index as u16,
                ..Telemetry::default()
            })
            .collect();
        storage.record_samples(session.id, &samples).unwrap();
        storage.record_samples(session.id, &[]).unwrap();
        // Re-writing the same timestamps replaces rather than duplicates.
        storage.record_samples(session.id, &samples[..2]).unwrap();
        let stored = storage.session(session.id).unwrap().unwrap().samples;
        assert_eq!(stored.len(), 5);
        assert_eq!(stored[4].power_watts, 104);
    }

    #[test]
    fn active_seconds_ignores_pause_sized_gaps() {
        let sample = |timestamp_ms| Telemetry {
            timestamp_ms,
            power_watts: 100,
            ..Telemetry::default()
        };
        assert_eq!(active_seconds(&[]), 0);
        assert_eq!(
            active_seconds(&[sample(0), sample(1_000), sample(2_000)]),
            2
        );
        // A minute-long hole (a pause, or the app being dead) does not count.
        assert_eq!(
            active_seconds(&[sample(0), sample(1_000), sample(61_000), sample(62_000)]),
            2
        );
    }

    #[test]
    fn unfinished_sessions_are_closed_from_samples_and_empty_ones_removed() {
        let storage = Storage::in_memory().unwrap();
        let mut finished = storage.start_session(None, "Done", 84.0).unwrap();
        finished.ended_at = Some(Utc::now());
        finished.elapsed_seconds = 7;
        storage.finish_session(&finished).unwrap();
        let orphan = storage.start_session(None, "Died", 84.0).unwrap();
        for (index, watts) in [100_u16, 200, 300].into_iter().enumerate() {
            storage
                .record_sample(
                    orphan.id,
                    &Telemetry {
                        timestamp_ms: 1_700_000_000_000 + 1_000 * index as i64,
                        power_watts: watts,
                        cadence_rpm: Some(90.0),
                        speed_kph: Some(30.0),
                        heart_rate_bpm: None,
                        target_power_watts: None,
                    },
                )
                .unwrap();
        }
        let empty = storage.start_session(None, "Empty", 84.0).unwrap();
        assert_eq!(storage.unfinished_sessions().unwrap().len(), 2);

        let report = storage.finalize_orphaned_sessions().unwrap();
        assert_eq!(
            report,
            RecoveryReport {
                finalized: 1,
                deleted: 1,
                failed: 0
            }
        );
        let recovered = storage.session(orphan.id).unwrap().unwrap().summary;
        assert!(recovered.ended_at.is_some());
        assert!(!recovered.completed);
        assert_eq!(recovered.elapsed_seconds, 2);
        assert_eq!(recovered.average_power_watts, 200);
        assert_eq!(recovered.max_power_watts, 300);
        assert_eq!(recovered.average_cadence_rpm, Some(90.0));
        assert!(recovered.estimated_distance_meters > 0.0);
        assert!(storage.session(empty.id).unwrap().is_none());
        // Finished rides are never touched.
        assert_eq!(
            storage
                .session(finished.id)
                .unwrap()
                .unwrap()
                .summary
                .elapsed_seconds,
            7
        );
        assert!(storage.unfinished_sessions().unwrap().is_empty());
        assert_eq!(
            storage.finalize_orphaned_sessions().unwrap(),
            RecoveryReport::default()
        );
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
