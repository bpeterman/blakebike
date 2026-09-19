use std::time::Duration;

use reqwest::StatusCode;
use serde::Deserialize;

const BASE_URL: &str = "https://intervals.icu";

#[derive(Deserialize)]
struct MmpModel {
    ftp: Option<f64>,
}

pub async fn fetch_estimated_ftp(api_key: &str) -> Result<u16, String> {
    fetch_estimated_ftp_from(api_key, BASE_URL).await
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
