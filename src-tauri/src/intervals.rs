//! Intervals.icu API client.
//!
//! One `IntervalsClient` holds the API key, the base URL and a reused
//! `reqwest::Client`. Every call authenticates with HTTP basic auth, user
//! `API_KEY`, password the key, which is how Intervals.icu documents personal
//! API keys. `base_url` is a constructor parameter so tests can point the
//! client at a local listener serving canned JSON.
//!
//! Workout structure travels as ZWO on both the calendar and the library
//! path, so Intervals.icu resolves `%FTP` / `%MMP` targets with its own model
//! and `formats::import_zwo` reads the result.

use std::{fmt, time::Duration};

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use chrono::{NaiveDate, Utc};
use reqwest::{RequestBuilder, Response, StatusCode};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use uuid::Uuid;

use crate::{
    domain::{PlannedWorkout, Workout},
    formats::import_zwo,
};

const BASE_URL: &str = "https://intervals.icu";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Namespace for the v5 UUIDs given to planned workouts, so the same
/// Intervals.icu event always maps to the same local id.
const PLANNED_WORKOUT_NAMESPACE: Uuid = Uuid::from_u128(0x6b1d_2f3e_4a5c_4d7e_8f90_1a2b_3c4d_5e6f);

/// Why a request to Intervals.icu did not produce data. The command layer
/// turns these into the strings the UI shows; keeping them apart here means
/// "your key is wrong" and "you are offline" never blur into one message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntervalsError {
    /// 401 or 403: the key was rejected.
    Unauthorized,
    /// The request never got an answer (DNS, TCP, TLS, timeout).
    Network(String),
    /// Any other non-2xx status.
    Http { status: u16, what: &'static str },
    /// 2xx, but the body was not what we expected.
    Unreadable { what: &'static str, error: String },
}

impl fmt::Display for IntervalsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unauthorized => write!(f, "Intervals.icu rejected the API key"),
            Self::Network(error) => write!(f, "Could not reach Intervals.icu: {error}"),
            Self::Http { status, what } => {
                write!(f, "Intervals.icu {what} request failed (HTTP {status})")
            }
            Self::Unreadable { what, error } => {
                write!(f, "Intervals.icu returned unreadable {what}: {error}")
            }
        }
    }
}

impl std::error::Error for IntervalsError {}

impl From<IntervalsError> for String {
    fn from(error: IntervalsError) -> Self {
        error.to_string()
    }
}

/// The athlete a key belongs to, as `GET /athlete/0` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntervalsAthlete {
    /// Intervals.icu athlete id, e.g. `i12345`.
    pub id: String,
    pub name: Option<String>,
}

pub struct IntervalsClient {
    api_key: String,
    base_url: String,
    http: reqwest::Client,
}

impl IntervalsClient {
    pub fn new(api_key: &str) -> Result<Self, IntervalsError> {
        Self::with_base_url(api_key, BASE_URL)
    }

    pub fn with_base_url(api_key: &str, base_url: &str) -> Result<Self, IntervalsError> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|error| {
                IntervalsError::Network(format!("could not prepare the request: {error}"))
            })?;
        Ok(Self {
            api_key: api_key.to_owned(),
            base_url: base_url.trim_end_matches('/').to_owned(),
            http,
        })
    }

    fn get(&self, path: &str) -> RequestBuilder {
        self.http.get(format!("{}{path}", self.base_url))
    }

    fn post(&self, path: &str) -> RequestBuilder {
        self.http.post(format!("{}{path}", self.base_url))
    }

    async fn send(
        &self,
        request: RequestBuilder,
        what: &'static str,
    ) -> Result<Response, IntervalsError> {
        let response = request
            .basic_auth("API_KEY", Some(&self.api_key))
            .send()
            .await
            .map_err(|error| IntervalsError::Network(error.to_string()))?;
        let status = response.status();
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            return Err(IntervalsError::Unauthorized);
        }
        if !status.is_success() {
            return Err(IntervalsError::Http {
                status: status.as_u16(),
                what,
            });
        }
        Ok(response)
    }

    async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        what: &'static str,
    ) -> Result<T, IntervalsError> {
        self.send(self.get(path), what)
            .await?
            .json()
            .await
            .map_err(|error| IntervalsError::Unreadable {
                what,
                error: error.to_string(),
            })
    }

    /// The athlete the key belongs to.
    pub async fn athlete(&self) -> Result<IntervalsAthlete, IntervalsError> {
        #[derive(Deserialize)]
        struct Athlete {
            id: String,
            name: Option<String>,
        }
        let athlete: Athlete = self.get_json("/api/v1/athlete/0", "athlete").await?;
        if athlete.id.trim().is_empty() {
            return Err(IntervalsError::Unreadable {
                what: "athlete",
                error: "missing id".into(),
            });
        }
        Ok(IntervalsAthlete {
            id: athlete.id,
            name: athlete.name.filter(|name| !name.trim().is_empty()),
        })
    }

    /// The athlete's cycling ("Ride") sport settings, normalized. `Ok(None)`
    /// means the athlete has no Ride settings at all.
    pub async fn cycling_settings(
        &self,
        athlete_id: &str,
    ) -> Result<Option<CyclingSettings>, IntervalsError> {
        let settings: Vec<SportSettings> = self
            .get_json(
                &format!("/api/v1/athlete/{athlete_id}/sport-settings"),
                "sport settings",
            )
            .await?;
        Ok(settings
            .into_iter()
            .find(|setting| setting.types.iter().any(|kind| is_ride(kind)))
            .map(normalized_cycling_settings))
    }

    /// Planned cycling workouts between two local dates, inclusive, with
    /// their structure decoded from the ZWO Intervals.icu attaches when asked
    /// with `ext=zwo`. A ZWO that cannot be read is a per-event failure.
    pub async fn planned_workouts(
        &self,
        athlete_id: &str,
        oldest: NaiveDate,
        newest: NaiveDate,
    ) -> Result<CalendarFetch, IntervalsError> {
        let events: Vec<Event> = self
            .get_json(
                &format!(
                    "/api/v1/athlete/{athlete_id}/events?oldest={oldest}&newest={newest}&category=WORKOUT&ext=zwo"
                ),
                "calendar",
            )
            .await?;
        let fetched_at = Utc::now();
        let mut fetch = CalendarFetch::default();
        for event in events {
            let Some(planned) = planned_workout_from_event(event, fetched_at) else {
                fetch.skipped += 1;
                continue;
            };
            fetch.planned.push(planned);
        }
        Ok(fetch)
    }

    /// The cycling workouts in the athlete's library with their folder names.
    /// Structure is not included; fetch it per workout with `workout_zwo`.
    pub async fn library(&self, athlete_id: &str) -> Result<LibraryFetch, IntervalsError> {
        let raw: Vec<serde_json::Value> = self
            .get_json(
                &format!("/api/v1/athlete/{athlete_id}/workouts"),
                "workout library",
            )
            .await?;
        let folders: Vec<Folder> = self
            .get_json(&format!("/api/v1/athlete/{athlete_id}/folders"), "folders")
            .await?;
        let mut fetch = LibraryFetch::default();
        for value in raw {
            let workout: LibraryWorkout =
                serde_json::from_value(value.clone()).map_err(|error| {
                    IntervalsError::Unreadable {
                        what: "workout library",
                        error: error.to_string(),
                    }
                })?;
            let activity_type = workout.activity_type.unwrap_or_default();
            if !is_ride(&activity_type) {
                fetch.skipped += 1;
                continue;
            }
            let folder = workout.folder_id.and_then(|id| {
                folders
                    .iter()
                    .find(|folder| folder.id == id)
                    .and_then(|folder| folder.name.clone())
            });
            fetch.entries.push(LibraryEntry {
                external_id: workout.id,
                name: non_empty(workout.name).unwrap_or_else(|| "Untitled workout".into()),
                description: workout.description.unwrap_or_default(),
                activity_type,
                folder_id: workout.folder_id,
                folder,
                updated: workout.updated.unwrap_or_default(),
                planned_load: to_u16(workout.icu_training_load),
                duration_seconds: to_u32(workout.moving_time),
                raw: value,
            });
        }
        Ok(fetch)
    }

    /// One library workout as ZWO, converted by Intervals.icu from the
    /// workout JSON it gave us in the listing.
    pub async fn workout_zwo(
        &self,
        athlete_id: &str,
        entry: &LibraryEntry,
    ) -> Result<String, IntervalsError> {
        let what = "workout download";
        self.send(
            self.post(&format!(
                "/api/v1/athlete/{athlete_id}/download-workout.zwo"
            ))
            .json(&entry.raw),
            what,
        )
        .await?
        .text()
        .await
        .map_err(|error| IntervalsError::Unreadable {
            what,
            error: error.to_string(),
        })
    }
}

/// Intervals.icu activity types are Strava's: `Ride`, `VirtualRide`,
/// `GravelRide`, `MountainBikeRide`, `EBikeRide`. All of them are rides.
fn is_ride(activity_type: &str) -> bool {
    activity_type.to_ascii_lowercase().contains("ride")
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

fn to_u16(value: Option<f64>) -> Option<u16> {
    value
        .filter(|value| value.is_finite() && *value >= 0.0 && *value <= f64::from(u16::MAX))
        .map(|value| value.round() as u16)
}

fn to_u32(value: Option<f64>) -> Option<u32> {
    value
        .filter(|value| value.is_finite() && *value >= 0.0 && *value <= f64::from(u32::MAX))
        .map(|value| value.round() as u32)
}

/// Which Intervals.icu field an FTP came from. blake.bike is an indoor app,
/// so a configured indoor FTP wins over the general one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FtpSource {
    IndoorFtp,
    Ftp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CyclingFtp {
    pub watts: u16,
    pub source: FtpSource,
}

/// One zone set as Intervals.icu stores it: increasing upper bounds (bpm for
/// HR, percent of FTP for power) and optional names, index-aligned. Empty
/// boundaries mean the set is not configured.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ZoneBoundaries {
    pub boundaries: Vec<u16>,
    pub names: Vec<String>,
}

/// The cycling sport settings, each zone set normalized on its own so a bad
/// HR set cannot spoil the FTP or the power zones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CyclingSettings {
    pub ftp: Option<CyclingFtp>,
    pub max_heart_rate_bpm: Option<u16>,
    pub heart_rate_zones: Result<ZoneBoundaries, String>,
    pub power_zones: Result<ZoneBoundaries, String>,
}

#[derive(Deserialize)]
struct SportSettings {
    #[serde(default)]
    types: Vec<String>,
    ftp: Option<f64>,
    indoor_ftp: Option<f64>,
    max_hr: Option<f64>,
    #[serde(default)]
    hr_zones: Option<Vec<f64>>,
    #[serde(default)]
    hr_zone_names: Option<Vec<String>>,
    #[serde(default)]
    power_zones: Option<Vec<f64>>,
    #[serde(default)]
    power_zone_names: Option<Vec<String>>,
}

/// A calendar event as `GET /events?ext=zwo` returns it; only the fields we
/// keep. `workout_file_base64` is present when the event has structure.
#[derive(Deserialize)]
struct Event {
    id: i64,
    start_date_local: Option<String>,
    name: Option<String>,
    description: Option<String>,
    #[serde(rename = "type")]
    activity_type: Option<String>,
    category: Option<String>,
    icu_training_load: Option<f64>,
    moving_time: Option<f64>,
    updated: Option<String>,
    workout_doc: Option<serde_json::Value>,
    workout_file_base64: Option<String>,
}

#[derive(Debug, Default)]
pub struct CalendarFetch {
    pub planned: Vec<PlannedWorkout>,
    /// Events that were not cycling workouts.
    pub skipped: usize,
}

/// `None` when the event is not a cycling workout.
fn planned_workout_from_event(
    event: Event,
    fetched_at: chrono::DateTime<Utc>,
) -> Option<PlannedWorkout> {
    if event
        .category
        .as_deref()
        .is_some_and(|category| category != "WORKOUT")
    {
        return None;
    }
    let activity_type = event.activity_type.unwrap_or_default();
    if !is_ride(&activity_type) {
        return None;
    }
    let date = event
        .start_date_local
        .as_deref()
        .and_then(|stamp| NaiveDate::parse_from_str(stamp.get(..10)?, "%Y-%m-%d").ok())?;
    let name = non_empty(event.name).unwrap_or_else(|| "Planned workout".into());
    let workout_id = planned_workout_uuid(event.id);
    let (workout, parse_error) = match (event.workout_file_base64, event.workout_doc) {
        (Some(encoded), _) => match decode_zwo(&encoded).and_then(|zwo| import_zwo(&zwo)) {
            Ok(mut parsed) => {
                parsed.id = workout_id;
                parsed.name = name.clone();
                parsed.source = "intervals".into();
                (Some(parsed), None)
            }
            Err(error) => (None, Some(error)),
        },
        (None, Some(_)) => (
            None,
            Some("Intervals.icu did not attach a ZWO file for this workout".into()),
        ),
        (None, None) => (None, None),
    };
    Some(PlannedWorkout {
        event_id: event.id,
        workout_id,
        date,
        name,
        description: event.description.unwrap_or_default(),
        activity_type,
        planned_load: to_u16(event.icu_training_load),
        duration_seconds: workout
            .as_ref()
            .map(Workout::duration_seconds)
            .or_else(|| to_u32(event.moving_time)),
        workout,
        parse_error,
        updated: event.updated,
        fetched_at,
    })
}

pub fn planned_workout_uuid(event_id: i64) -> Uuid {
    Uuid::new_v5(&PLANNED_WORKOUT_NAMESPACE, event_id.to_string().as_bytes())
}

fn decode_zwo(encoded: &str) -> Result<String, String> {
    let bytes = BASE64
        .decode(encoded.trim())
        .map_err(|error| format!("Intervals.icu sent an unreadable workout file: {error}"))?;
    String::from_utf8(bytes)
        .map_err(|error| format!("Intervals.icu sent a workout file that is not UTF-8: {error}"))
}

#[derive(Deserialize)]
struct LibraryWorkout {
    id: i64,
    name: Option<String>,
    description: Option<String>,
    #[serde(rename = "type")]
    activity_type: Option<String>,
    folder_id: Option<i64>,
    updated: Option<String>,
    icu_training_load: Option<f64>,
    moving_time: Option<f64>,
}

#[derive(Deserialize)]
struct Folder {
    id: i64,
    name: Option<String>,
}

/// One cycling workout in the library, without structure.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryEntry {
    pub external_id: i64,
    pub name: String,
    pub description: String,
    pub activity_type: String,
    pub folder_id: Option<i64>,
    pub folder: Option<String>,
    pub updated: String,
    pub planned_load: Option<u16>,
    pub duration_seconds: Option<u32>,
    /// The workout exactly as listed, posted back for ZWO conversion.
    raw: serde_json::Value,
}

#[derive(Debug, Default)]
pub struct LibraryFetch {
    pub entries: Vec<LibraryEntry>,
    /// Library workouts that were not cycling workouts.
    pub skipped: usize,
}

fn in_range(value: Option<f64>, minimum: f64, maximum: f64) -> Option<u16> {
    value
        .filter(|value| value.is_finite() && (minimum..=maximum).contains(value))
        .map(|value| value.round() as u16)
}

fn normalized_boundaries(
    values: Vec<f64>,
    minimum: f64,
    maximum: f64,
    label: &str,
) -> Result<Vec<u16>, String> {
    let mut boundaries = Vec::new();
    for value in values {
        if !value.is_finite() || !(minimum..=maximum).contains(&value) {
            return Err(format!(
                "Intervals.icu returned an invalid {label} zone boundary"
            ));
        }
        let value = value.round() as u16;
        if boundaries.last().is_some_and(|previous| value <= *previous) {
            return Err(format!(
                "Intervals.icu returned {label} zones that do not increase"
            ));
        }
        boundaries.push(value);
    }
    if !boundaries.is_empty() && boundaries.len() < 2 {
        return Err(format!(
            "Intervals.icu cycling settings do not include usable {label} zones"
        ));
    }
    Ok(boundaries)
}

fn normalized_cycling_settings(settings: SportSettings) -> CyclingSettings {
    let ftp = in_range(settings.indoor_ftp, 50.0, 500.0)
        .map(|watts| CyclingFtp {
            watts,
            source: FtpSource::IndoorFtp,
        })
        .or_else(|| {
            in_range(settings.ftp, 50.0, 500.0).map(|watts| CyclingFtp {
                watts,
                source: FtpSource::Ftp,
            })
        });
    let max_heart_rate_bpm = in_range(settings.max_hr, 100.0, 230.0);
    let heart_rate_zones =
        normalized_boundaries(settings.hr_zones.unwrap_or_default(), 30.0, 250.0, "HR").and_then(
            |boundaries| {
                if !boundaries.is_empty() && max_heart_rate_bpm.is_none() {
                    return Err(
                        "Intervals.icu cycling settings include HR zones without a valid max HR"
                            .to_string(),
                    );
                }
                Ok(ZoneBoundaries {
                    boundaries,
                    names: settings.hr_zone_names.unwrap_or_default(),
                })
            },
        );
    let power_zones = normalized_boundaries(
        settings.power_zones.unwrap_or_default(),
        1.0,
        999.0,
        "power",
    )
    .map(|mut boundaries| {
        // Intervals.icu closes the last zone with a 999 % sentinel; ours is
        // open-ended.
        if boundaries.last().is_some_and(|bound| *bound >= 999) {
            boundaries.pop();
        }
        ZoneBoundaries {
            boundaries,
            names: settings.power_zone_names.unwrap_or_default(),
        }
    });
    CyclingSettings {
        ftp,
        max_heart_rate_bpm,
        heart_rate_zones,
        power_zones,
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Local HTTP listeners serving canned responses. The joined handle
    //! returns the raw requests so tests can assert on path, headers and body.

    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        thread,
    };

    /// One canned answer, matched by `METHOD /path` prefix.
    pub struct Route {
        pub prefix: &'static str,
        pub status: &'static str,
        pub content_type: &'static str,
        pub body: String,
    }

    pub fn json(prefix: &'static str, body: &str) -> Route {
        Route {
            prefix,
            status: "200 OK",
            content_type: "application/json",
            body: body.to_string(),
        }
    }

    fn read_request(stream: &mut TcpStream) -> String {
        let mut buffer = Vec::new();
        let mut chunk = [0_u8; 4096];
        loop {
            let bytes = stream.read(&mut chunk).unwrap();
            if bytes == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..bytes]);
            let text = String::from_utf8_lossy(&buffer);
            if let Some(end) = text.find("\r\n\r\n") {
                let content_length = text[..end]
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                if buffer.len() >= end + 4 + content_length {
                    break;
                }
            }
        }
        String::from_utf8_lossy(&buffer).into_owned()
    }

    fn respond(stream: &mut TcpStream, status: &str, content_type: &str, body: &str) {
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    }

    /// Serve exactly one request.
    pub fn serve_once(status: &str, body: &str) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let status = status.to_string();
        let body = body.to_string();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            respond(&mut stream, &status, "application/json", &body);
            request
        });
        (format!("http://{address}"), handle)
    }

    /// Serve `count` requests, each answered by the first route whose prefix
    /// matches `METHOD /path`; an unmatched request gets a 404 so the test
    /// fails loudly instead of hanging.
    pub fn serve_routes(
        routes: Vec<Route>,
        count: usize,
    ) -> (String, thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let mut requests = Vec::new();
            for _ in 0..count {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_request(&mut stream);
                match routes
                    .iter()
                    .find(|route| request.starts_with(route.prefix))
                {
                    Some(route) => {
                        respond(&mut stream, route.status, route.content_type, &route.body)
                    }
                    None => respond(&mut stream, "404 Not Found", "text/plain", "no route"),
                }
                requests.push(request);
            }
            requests
        });
        (format!("http://{address}"), handle)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        test_support::{Route, json, serve_once, serve_routes},
        *,
    };
    use crate::domain::WorkoutStep;

    fn ride(json: &str) -> SportSettings {
        serde_json::from_str(json).unwrap()
    }

    const ZWO: &str = r#"<workout_file><name>Sweet Spot</name><description>3x12</description><workout>
      <Warmup Duration="600" PowerLow="0.5" PowerHigh="0.7"/>
      <IntervalsT Repeat="3" OnDuration="720" OffDuration="240" OnPower="0.9" OffPower="0.55"/>
      <Cooldown Duration="120" PowerLow="0.55" PowerHigh="0.4"/>
    </workout></workout_file>"#;

    fn encoded(zwo: &str) -> String {
        BASE64.encode(zwo.as_bytes())
    }

    #[test]
    fn prefers_indoor_ftp_and_falls_back_to_ftp() {
        let both =
            normalized_cycling_settings(ride(r#"{"types":["Ride"],"ftp":280,"indoor_ftp":265.4}"#));
        assert_eq!(
            both.ftp,
            Some(CyclingFtp {
                watts: 265,
                source: FtpSource::IndoorFtp
            })
        );
        let only_ftp = normalized_cycling_settings(ride(r#"{"types":["Ride"],"ftp":280}"#));
        assert_eq!(
            only_ftp.ftp,
            Some(CyclingFtp {
                watts: 280,
                source: FtpSource::Ftp
            })
        );
        let out_of_range =
            normalized_cycling_settings(ride(r#"{"types":["Ride"],"ftp":49,"indoor_ftp":501}"#));
        assert_eq!(out_of_range.ftp, None);
        let nothing = normalized_cycling_settings(ride(r#"{"types":["Ride"]}"#));
        assert_eq!(nothing.ftp, None);
        assert_eq!(nothing.max_heart_rate_bpm, None);
        assert_eq!(nothing.heart_rate_zones, Ok(ZoneBoundaries::default()));
        assert_eq!(nothing.power_zones, Ok(ZoneBoundaries::default()));
    }

    #[test]
    fn normalizes_each_zone_set_on_its_own() {
        let settings = normalized_cycling_settings(SportSettings {
            types: vec!["Ride".into()],
            ftp: Some(280.0),
            indoor_ftp: None,
            max_hr: Some(192.2),
            hr_zones: Some(vec![120.0, 145.4, 166.0, 182.0, 192.0]),
            hr_zone_names: Some(vec!["Recovery".into(), "Endurance".into()]),
            power_zones: Some(vec![55.0, 75.0, 90.0, 105.0, 120.0, 150.0, 999.0]),
            power_zone_names: Some(vec!["Recovery".into(), "Endurance".into()]),
        });
        assert_eq!(settings.max_heart_rate_bpm, Some(192));
        let hr = settings.heart_rate_zones.unwrap();
        assert_eq!(hr.boundaries, vec![120, 145, 166, 182, 192]);
        assert_eq!(hr.names[1], "Endurance");
        let power = settings.power_zones.unwrap();
        assert_eq!(power.boundaries, vec![55, 75, 90, 105, 120, 150]);

        // A bad HR set does not touch the FTP or the power set.
        let bad_hr = normalized_cycling_settings(ride(
            r#"{"types":["Ride"],"ftp":280,"max_hr":190,"hr_zones":[140,130],
                "power_zones":[55,75,90,105,120,150,999]}"#,
        ));
        assert_eq!(bad_hr.ftp.map(|ftp| ftp.watts), Some(280));
        assert_eq!(
            bad_hr.heart_rate_zones.unwrap_err(),
            "Intervals.icu returned HR zones that do not increase"
        );
        assert_eq!(bad_hr.power_zones.unwrap().boundaries.len(), 6);

        let no_max_hr = normalized_cycling_settings(ride(
            r#"{"types":["Ride"],"ftp":280,"hr_zones":[130,150,170]}"#,
        ));
        assert!(
            no_max_hr
                .heart_rate_zones
                .unwrap_err()
                .contains("without a valid max HR")
        );
        let lone_bound =
            normalized_cycling_settings(ride(r#"{"types":["Ride"],"ftp":280,"power_zones":[75]}"#));
        assert!(
            lone_bound
                .power_zones
                .unwrap_err()
                .contains("usable power zones")
        );
    }

    #[tokio::test]
    async fn fetches_cycling_sport_settings_with_api_key_basic_auth() {
        let body = r#"[
          {"types":["Run"],"ftp":null,"max_hr":188,"hr_zones":[130,150,170,188],
           "power_zones":null,"power_zone_names":null},
          {"types":["Ride"],"ftp":280,"indoor_ftp":270,"max_hr":194,"hr_zones":[125,146,165,182,194],
           "hr_zone_names":["Easy","Endurance","Tempo","Threshold","Maximum"],
           "power_zones":[55,75,90,105,120,150,999],
           "power_zone_names":["Recovery","Endurance","Tempo","Threshold","VO2","Anaerobic","Neuromuscular"]}
        ]"#;
        let (base_url, server) = serve_once("200 OK", body);
        let client = IntervalsClient::with_base_url("secret", &base_url).unwrap();
        let settings = client.cycling_settings("i123").await.unwrap().unwrap();
        assert_eq!(
            settings.ftp,
            Some(CyclingFtp {
                watts: 270,
                source: FtpSource::IndoorFtp
            })
        );
        assert_eq!(settings.max_heart_rate_bpm, Some(194));
        assert_eq!(
            settings.heart_rate_zones.unwrap().boundaries,
            vec![125, 146, 165, 182, 194]
        );
        let power = settings.power_zones.unwrap();
        assert_eq!(power.boundaries, vec![55, 75, 90, 105, 120, 150]);
        assert_eq!(power.names[6], "Neuromuscular");
        let request = server.join().unwrap();
        assert!(request.starts_with("GET /api/v1/athlete/i123/sport-settings "));
        assert!(request.contains("authorization: Basic QVBJX0tFWTpzZWNyZXQ="));
    }

    #[tokio::test]
    async fn no_ride_settings_is_none_not_an_error() {
        let (base_url, server) = serve_once(
            "200 OK",
            r#"[{"types":["Run"],"ftp":null,"max_hr":188,"hr_zones":[130,150,170,188]}]"#,
        );
        let client = IntervalsClient::with_base_url("secret", &base_url).unwrap();
        assert_eq!(client.cycling_settings("0").await.unwrap(), None);
        server.join().unwrap();
    }

    #[tokio::test]
    async fn maps_auth_http_and_body_errors() {
        let (base_url, server) = serve_once("401 Unauthorized", "{}");
        let client = IntervalsClient::with_base_url("bad", &base_url).unwrap();
        assert_eq!(
            client.cycling_settings("0").await.unwrap_err(),
            IntervalsError::Unauthorized
        );
        server.join().unwrap();

        let (base_url, server) = serve_once("500 Internal Server Error", "{}");
        let client = IntervalsClient::with_base_url("secret", &base_url).unwrap();
        assert_eq!(
            client.cycling_settings("0").await.unwrap_err().to_string(),
            "Intervals.icu sport settings request failed (HTTP 500)"
        );
        server.join().unwrap();

        let (base_url, server) = serve_once("200 OK", "not json");
        let client = IntervalsClient::with_base_url("secret", &base_url).unwrap();
        assert!(matches!(
            client.cycling_settings("0").await.unwrap_err(),
            IntervalsError::Unreadable {
                what: "sport settings",
                ..
            }
        ));
        server.join().unwrap();

        // Nothing listening: a network error, not an auth error.
        let client = IntervalsClient::with_base_url("secret", "http://127.0.0.1:9").unwrap();
        assert!(matches!(
            client.cycling_settings("0").await.unwrap_err(),
            IntervalsError::Network(_)
        ));
    }

    #[tokio::test]
    async fn resolves_the_athlete_behind_the_key() {
        let (base_url, server) = serve_once(
            "200 OK",
            r#"{"id":"i98765","name":"Blake P","email":"x@y.z","sportSettings":[]}"#,
        );
        let client = IntervalsClient::with_base_url("secret", &base_url).unwrap();
        assert_eq!(
            client.athlete().await.unwrap(),
            IntervalsAthlete {
                id: "i98765".into(),
                name: Some("Blake P".into())
            }
        );
        assert!(server.join().unwrap().starts_with("GET /api/v1/athlete/0 "));

        let (base_url, server) = serve_once("200 OK", r#"{"id":"i1","name":""}"#);
        let client = IntervalsClient::with_base_url("secret", &base_url).unwrap();
        assert_eq!(client.athlete().await.unwrap().name, None);
        server.join().unwrap();
    }

    #[tokio::test]
    async fn decodes_planned_cycling_workouts_from_the_calendar() {
        let body = format!(
            r#"[
              {{"id":501,"start_date_local":"2026-09-21T00:00:00","name":"Sweet Spot 3x12","description":"- 10m 50-70%",
                "type":"Ride","category":"WORKOUT","icu_training_load":68.4,"moving_time":3720,"updated":"2026-09-20T10:00:00",
                "workout_doc":{{"steps":[]}},"workout_filename":"Sweet_Spot.zwo","workout_file_base64":"{zwo}"}},
              {{"id":502,"start_date_local":"2026-09-22T00:00:00","name":"Easy spin","type":"VirtualRide","category":"WORKOUT",
                "icu_training_load":25,"moving_time":2700}},
              {{"id":503,"start_date_local":"2026-09-22T00:00:00","name":"Long run","type":"Run","category":"WORKOUT",
                "workout_file_base64":"{zwo}"}},
              {{"id":504,"start_date_local":"2026-09-23T00:00:00","name":"Broken","type":"Ride","category":"WORKOUT",
                "workout_doc":{{}},"workout_file_base64":"{broken}"}}
            ]"#,
            zwo = encoded(ZWO),
            broken = encoded(
                "<workout_file><workout><Sprint Duration=\"10\"/></workout></workout_file>"
            ),
        );
        let (base_url, server) = serve_once("200 OK", &body);
        let client = IntervalsClient::with_base_url("secret", &base_url).unwrap();
        let oldest = NaiveDate::from_ymd_opt(2026, 9, 21).unwrap();
        let newest = NaiveDate::from_ymd_opt(2026, 9, 27).unwrap();
        let fetch = client.planned_workouts("i1", oldest, newest).await.unwrap();
        let request = server.join().unwrap();
        assert!(request.starts_with(
            "GET /api/v1/athlete/i1/events?oldest=2026-09-21&newest=2026-09-27&category=WORKOUT&ext=zwo "
        ));
        assert_eq!(fetch.skipped, 1, "the run is not a cycling workout");
        assert_eq!(fetch.planned.len(), 3);

        let structured = &fetch.planned[0];
        assert_eq!(structured.event_id, 501);
        assert_eq!(structured.workout_id, planned_workout_uuid(501));
        assert_eq!(structured.date, oldest);
        assert_eq!(structured.name, "Sweet Spot 3x12");
        assert_eq!(structured.planned_load, Some(68));
        assert_eq!(structured.duration_seconds, Some(3600));
        assert_eq!(structured.parse_error, None);
        let workout = structured.workout.as_ref().unwrap();
        assert_eq!(workout.id, structured.workout_id);
        assert_eq!(workout.name, "Sweet Spot 3x12");
        assert_eq!(workout.source, "intervals");
        assert_eq!(workout.steps.len(), 3);
        assert!(matches!(
            workout.steps[1],
            WorkoutStep::Repeat { repetitions: 3, .. }
        ));

        let unstructured = &fetch.planned[1];
        assert_eq!(unstructured.workout, None);
        assert_eq!(unstructured.parse_error, None);
        assert_eq!(unstructured.duration_seconds, Some(2700));
        assert_eq!(unstructured.planned_load, Some(25));

        let broken = &fetch.planned[2];
        assert_eq!(broken.workout, None);
        assert_eq!(
            broken.parse_error.as_deref(),
            Some("Unsupported workout element <Sprint>")
        );
    }

    #[test]
    fn planned_workout_ids_are_stable_per_event() {
        assert_eq!(planned_workout_uuid(42), planned_workout_uuid(42));
        assert_ne!(planned_workout_uuid(42), planned_workout_uuid(43));
    }

    #[tokio::test]
    async fn lists_the_cycling_library_with_folder_names_and_converts_one_to_zwo() {
        let workouts = r#"[
          {"id":7,"name":"Threshold 2x20","description":"- 2x 20m 98%","type":"Ride","folder_id":3,
           "updated":"2026-09-01T08:00:00","icu_training_load":92,"moving_time":4500},
          {"id":8,"name":"Tempo run","type":"Run","folder_id":3,"updated":"2026-09-01T08:00:00"},
          {"id":9,"name":"Openers","type":"VirtualRide","folder_id":null,"updated":"2026-09-02T08:00:00"}
        ]"#;
        let folders = r#"[{"id":3,"name":"Base","type":"FOLDER","children":[]}]"#;
        let (base_url, server) = serve_routes(
            vec![
                json("GET /api/v1/athlete/i1/workouts ", workouts),
                json("GET /api/v1/athlete/i1/folders ", folders),
                Route {
                    prefix: "POST /api/v1/athlete/i1/download-workout.zwo ",
                    status: "200 OK",
                    content_type: "application/xml",
                    body: ZWO.to_string(),
                },
            ],
            3,
        );
        let client = IntervalsClient::with_base_url("secret", &base_url).unwrap();
        let library = client.library("i1").await.unwrap();
        assert_eq!(library.skipped, 1);
        assert_eq!(library.entries.len(), 2);
        let threshold = &library.entries[0];
        assert_eq!(threshold.external_id, 7);
        assert_eq!(threshold.folder.as_deref(), Some("Base"));
        assert_eq!(threshold.folder_id, Some(3));
        assert_eq!(threshold.planned_load, Some(92));
        assert_eq!(threshold.duration_seconds, Some(4500));
        assert_eq!(threshold.updated, "2026-09-01T08:00:00");
        assert_eq!(library.entries[1].folder, None);

        let zwo = client.workout_zwo("i1", threshold).await.unwrap();
        assert_eq!(import_zwo(&zwo).unwrap().steps.len(), 3);

        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(requests[2].contains("content-type: application/json"));
        assert!(requests[2].ends_with(r#"}"#));
        assert!(requests[2].contains(r#""id":7"#));
    }
}
