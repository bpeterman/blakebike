//! Intervals.icu API client.
//!
//! One `IntervalsClient` holds the API key, the base URL and a reused
//! `reqwest::Client`. Every call authenticates with HTTP basic auth, user
//! `API_KEY`, password the key, which is how Intervals.icu documents personal
//! API keys. `base_url` is a constructor parameter so tests can point the
//! client at a local listener serving canned JSON.

use std::{fmt, time::Duration};

use reqwest::StatusCode;
use serde::{Deserialize, de::DeserializeOwned};

const BASE_URL: &str = "https://intervals.icu";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

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
    /// 2xx, but the body was not the JSON we expected.
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

    async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        what: &'static str,
    ) -> Result<T, IntervalsError> {
        let response = self
            .http
            .get(format!("{}{path}", self.base_url))
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
        response
            .json()
            .await
            .map_err(|error| IntervalsError::Unreadable {
                what,
                error: error.to_string(),
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
            .find(|setting| {
                setting
                    .types
                    .iter()
                    .any(|kind| kind.eq_ignore_ascii_case("ride"))
            })
            .map(normalized_cycling_settings))
    }
}

/// Which Intervals.icu field an FTP came from. blake.bike is an indoor app,
/// so a configured indoor FTP wins over the general one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
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
    //! A one-shot HTTP listener serving canned JSON. The joined handle
    //! returns the raw request so tests can assert on path and headers.

    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    pub fn serve_once(status: &str, body: &str) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let status = status.to_string();
        let body = body.to_string();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let bytes = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..bytes]).into_owned();
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            request
        });
        (format!("http://{address}"), handle)
    }
}

#[cfg(test)]
mod tests {
    use super::{test_support::serve_once, *};

    fn ride(json: &str) -> SportSettings {
        serde_json::from_str(json).unwrap()
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
}
