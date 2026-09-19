use std::time::Duration;

use reqwest::StatusCode;
use serde::Deserialize;

const BASE_URL: &str = "https://intervals.icu";

#[derive(Deserialize)]
struct MmpModel {
    ftp: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedCyclingZones {
    pub cycling_ftp_watts: Option<u16>,
    pub max_heart_rate_bpm: Option<u16>,
    pub heart_rate_boundaries: Vec<u16>,
    pub heart_rate_names: Vec<String>,
    pub power_percent_boundaries: Vec<u16>,
    pub power_names: Vec<String>,
}

#[derive(Deserialize)]
struct SportSettings {
    #[serde(default)]
    types: Vec<String>,
    ftp: Option<f64>,
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

pub async fn fetch_estimated_ftp(api_key: &str) -> Result<u16, String> {
    fetch_estimated_ftp_from(api_key, BASE_URL).await
}

pub async fn fetch_cycling_training_zones(
    api_key: &str,
) -> Result<Option<ImportedCyclingZones>, String> {
    fetch_cycling_training_zones_from(api_key, BASE_URL).await
}

async fn fetch_cycling_training_zones_from(
    api_key: &str,
    base_url: &str,
) -> Result<Option<ImportedCyclingZones>, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| format!("Could not prepare the Intervals.icu request: {error}"))?;
    let response = client
        .get(format!("{base_url}/api/v1/athlete/0/sport-settings"))
        .basic_auth("API_KEY", Some(api_key))
        .send()
        .await
        .map_err(|error| format!("Could not reach Intervals.icu: {error}"))?;
    if matches!(
        response.status(),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ) {
        return Err("Intervals.icu rejected the API key".into());
    }
    if !response.status().is_success() {
        return Err(format!(
            "Intervals.icu sport settings request failed (HTTP {})",
            response.status().as_u16()
        ));
    }
    let settings: Vec<SportSettings> = response
        .json()
        .await
        .map_err(|error| format!("Intervals.icu returned unreadable sport settings: {error}"))?;
    let Some(cycling) = settings.into_iter().find(|setting| {
        setting
            .types
            .iter()
            .any(|kind| kind.eq_ignore_ascii_case("ride"))
    }) else {
        return Ok(None);
    };
    normalized_cycling_zones(cycling).map(Some)
}

async fn fetch_estimated_ftp_from(api_key: &str, base_url: &str) -> Result<u16, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| format!("Could not prepare the Intervals.icu request: {error}"))?;
    let response = client
        .get(format!(
            "{base_url}/api/v1/athlete/0/mmp-model?type=Ride&fields=ftp"
        ))
        .basic_auth("API_KEY", Some(api_key))
        .send()
        .await
        .map_err(|error| format!("Could not reach Intervals.icu: {error}"))?;

    if matches!(
        response.status(),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ) {
        return Err("Intervals.icu rejected the API key".into());
    }
    if !response.status().is_success() {
        return Err(format!(
            "Intervals.icu request failed (HTTP {})",
            response.status().as_u16()
        ));
    }

    let model: MmpModel = response
        .json()
        .await
        .map_err(|error| format!("Intervals.icu returned an unreadable FTP response: {error}"))?;
    normalized_ftp(model.ftp)
}

fn normalized_ftp(ftp: Option<f64>) -> Result<u16, String> {
    let ftp = ftp.ok_or_else(|| "Intervals.icu did not return an estimated FTP".to_string())?;
    if !ftp.is_finite() || !(50.0..=500.0).contains(&ftp) {
        return Err("Intervals.icu returned an FTP outside the supported 50–500 W range".into());
    }
    Ok(ftp.round() as u16)
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

fn normalized_cycling_zones(settings: SportSettings) -> Result<ImportedCyclingZones, String> {
    let cycling_ftp_watts = settings
        .ftp
        .filter(|value| value.is_finite() && (50.0..=500.0).contains(value))
        .map(|value| value.round() as u16);
    let max_heart_rate_bpm = settings
        .max_hr
        .filter(|value| value.is_finite() && (100.0..=230.0).contains(value))
        .map(|value| value.round() as u16);
    let heart_rate_boundaries =
        normalized_boundaries(settings.hr_zones.unwrap_or_default(), 30.0, 250.0, "HR")?;
    if !heart_rate_boundaries.is_empty() && max_heart_rate_bpm.is_none() {
        return Err(
            "Intervals.icu cycling settings include HR zones without a valid max HR".into(),
        );
    }
    let mut power_percent_boundaries = normalized_boundaries(
        settings.power_zones.unwrap_or_default(),
        1.0,
        999.0,
        "power",
    )?;
    if power_percent_boundaries
        .last()
        .is_some_and(|bound| *bound >= 999)
    {
        power_percent_boundaries.pop();
    }
    Ok(ImportedCyclingZones {
        cycling_ftp_watts,
        max_heart_rate_bpm,
        heart_rate_boundaries,
        heart_rate_names: settings.hr_zone_names.unwrap_or_default(),
        power_percent_boundaries,
        power_names: settings.power_zone_names.unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    use super::*;

    fn serve_once(status: &str, body: &str) -> (String, thread::JoinHandle<String>) {
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

    #[test]
    fn normalizes_valid_ftp_and_rejects_bad_values() {
        assert_eq!(normalized_ftp(Some(247.6)).unwrap(), 248);
        assert!(normalized_ftp(None).is_err());
        assert!(normalized_ftp(Some(49.9)).is_err());
        assert!(normalized_ftp(Some(500.1)).is_err());
        assert!(normalized_ftp(Some(f64::NAN)).is_err());
    }

    #[test]
    fn normalizes_cycling_zones_and_rejects_bad_order() {
        let zones = normalized_cycling_zones(SportSettings {
            types: vec!["Ride".into()],
            ftp: Some(280.0),
            max_hr: Some(192.2),
            hr_zones: Some(vec![120.0, 145.4, 166.0, 182.0, 192.0]),
            hr_zone_names: Some(vec!["Recovery".into(), "Endurance".into()]),
            power_zones: Some(vec![55.0, 75.0, 90.0, 105.0, 120.0, 150.0, 999.0]),
            power_zone_names: Some(vec!["Recovery".into(), "Endurance".into()]),
        })
        .unwrap();
        assert_eq!(zones.cycling_ftp_watts, Some(280));
        assert_eq!(zones.max_heart_rate_bpm, Some(192));
        assert_eq!(zones.heart_rate_boundaries, vec![120, 145, 166, 182, 192]);
        assert_eq!(
            zones.power_percent_boundaries,
            vec![55, 75, 90, 105, 120, 150]
        );
        assert!(normalized_boundaries(vec![140.0, 130.0], 30.0, 250.0, "HR").is_err());
    }

    #[tokio::test]
    async fn fetches_ftp_with_api_key_basic_auth() {
        let (base_url, server) = serve_once("200 OK", r#"{"ftp":263.4}"#);
        assert_eq!(
            fetch_estimated_ftp_from("secret", &base_url).await.unwrap(),
            263
        );
        let request = server.join().unwrap();
        assert!(request.starts_with("GET /api/v1/athlete/0/mmp-model?type=Ride&fields=ftp "));
        assert!(request.contains("authorization: Basic QVBJX0tFWTpzZWNyZXQ="));
    }

    #[tokio::test]
    async fn fetches_cycling_training_zone_sport_settings() {
        let body = r#"[
          {"types":["Run"],"ftp":null,"max_hr":188,"hr_zones":[130,150,170,188],
           "power_zones":null,"power_zone_names":null},
          {"types":["Ride"],"ftp":280,"max_hr":194,"hr_zones":[125,146,165,182,194],
           "hr_zone_names":["Easy","Endurance","Tempo","Threshold","Maximum"],
           "power_zones":[55,75,90,105,120,150,999],
           "power_zone_names":["Recovery","Endurance","Tempo","Threshold","VO2","Anaerobic","Neuromuscular"]}
        ]"#;
        let (base_url, server) = serve_once("200 OK", body);
        let imported = fetch_cycling_training_zones_from("secret", &base_url)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(imported.cycling_ftp_watts, Some(280));
        assert_eq!(imported.max_heart_rate_bpm, Some(194));
        assert_eq!(
            imported.heart_rate_boundaries,
            vec![125, 146, 165, 182, 194]
        );
        assert_eq!(imported.heart_rate_names[1], "Endurance");
        assert_eq!(
            imported.power_percent_boundaries,
            vec![55, 75, 90, 105, 120, 150]
        );
        assert_eq!(imported.power_names[6], "Neuromuscular");
        let request = server.join().unwrap();
        assert!(request.starts_with("GET /api/v1/athlete/0/sport-settings "));
    }

    #[tokio::test]
    async fn maps_auth_and_response_errors() {
        let (base_url, server) = serve_once("401 Unauthorized", "{}");
        assert_eq!(
            fetch_estimated_ftp_from("bad", &base_url)
                .await
                .unwrap_err(),
            "Intervals.icu rejected the API key"
        );
        server.join().unwrap();

        let (base_url, server) = serve_once("200 OK", "{}");
        assert_eq!(
            fetch_estimated_ftp_from("secret", &base_url)
                .await
                .unwrap_err(),
            "Intervals.icu did not return an estimated FTP"
        );
        server.join().unwrap();
    }
}
