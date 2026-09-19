//! Bluetooth Heart Rate Service (0x180D): Heart Rate Measurement (0x2A37).

use super::{Capability, DeviceInfo, fuser::Reading, sensor::Decoder};

pub const HEART_RATE_MEASUREMENT: u16 = 0x2A37;
pub const SIMULATED_HEART_RATE_ID: &str = "simulated-heart-rate";

pub fn simulated_device() -> DeviceInfo {
    DeviceInfo {
        id: SIMULATED_HEART_RATE_ID.into(),
        name: "Simulated HR Strap".into(),
        transport: Default::default(),
        simulated: true,
        rssi: Some(-40),
        capabilities: vec![Capability::HeartRate],
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeartRateMeasurement {
    pub bpm: u16,
    /// `None` when the strap does not support contact detection.
    pub sensor_contact: Option<bool>,
    pub energy_expended_kj: Option<u16>,
    /// Number of RR intervals present (values are skipped).
    pub rr_intervals: usize,
}

/// Parse a Heart Rate Measurement notification.
///
/// Flags (byte 0): bit 0 = value is u16 (else u8); bits 1–2 = sensor contact
/// (bit 2 "supported", bit 1 "detected"); bit 3 = energy expended present;
/// bit 4 = RR intervals present.
pub fn parse_heart_rate_measurement(data: &[u8]) -> Result<HeartRateMeasurement, String> {
    let flags = *data.first().ok_or("empty heart rate packet")?;
    let mut offset = 1;
    let bpm = if flags & 0x01 != 0 {
        let bytes = data
            .get(offset..offset + 2)
            .ok_or("truncated 16-bit heart rate value")?;
        offset += 2;
        u16::from_le_bytes([bytes[0], bytes[1]])
    } else {
        let value = *data.get(offset).ok_or("truncated heart rate value")?;
        offset += 1;
        u16::from(value)
    };
    let sensor_contact = if flags & 0x04 != 0 {
        Some(flags & 0x02 != 0)
    } else {
        None
    };
    let energy_expended_kj = if flags & 0x08 != 0 {
        let bytes = data
            .get(offset..offset + 2)
            .ok_or("truncated energy expended field")?;
        offset += 2;
        Some(u16::from_le_bytes([bytes[0], bytes[1]]))
    } else {
        None
    };
    let rr_intervals = if flags & 0x10 != 0 {
        let remaining = data.len().saturating_sub(offset);
        if remaining % 2 != 0 {
            return Err("odd number of RR interval bytes".into());
        }
        remaining / 2
    } else {
        0
    };
    Ok(HeartRateMeasurement {
        bpm,
        sensor_contact,
        energy_expended_kj,
        rr_intervals,
    })
}

#[derive(Default)]
pub struct HeartRateDecoder;

pub fn decoder(_device: &DeviceInfo) -> Box<dyn Decoder> {
    Box::new(HeartRateDecoder)
}

impl Decoder for HeartRateDecoder {
    fn characteristic(&self) -> u16 {
        HEART_RATE_MEASUREMENT
    }

    fn characteristic_name(&self) -> &'static str {
        "Heart Rate Measurement"
    }

    fn decode(&mut self, data: &[u8], _now_ms: i64) -> Result<Option<Reading>, String> {
        let measurement = parse_heart_rate_measurement(data)?;
        Ok(Some(Reading::HeartRate {
            bpm: measurement.bpm,
            sensor_contact: measurement.sensor_contact,
        }))
    }

    fn describe(&self, reading: &Reading) -> String {
        match reading {
            Reading::HeartRate {
                bpm,
                sensor_contact,
            } => match sensor_contact {
                Some(true) => format!("{bpm} bpm · contact"),
                Some(false) => format!("{bpm} bpm · no contact"),
                None => format!("{bpm} bpm"),
            },
            other => format!("{other:?}"),
        }
    }

    fn simulate(&mut self, tick: u64, _now_ms: i64) -> Reading {
        // A slow warm-up from 95 to ~150 bpm with a little breathing wobble.
        let seconds = tick as f32 / 2.0;
        let warmup = 1.0 - (-seconds / 240.0).exp();
        let wobble = (seconds / 7.0).sin() * 2.0;
        Reading::HeartRate {
            bpm: (95.0 + 55.0 * warmup + wobble).round() as u16,
            sensor_contact: Some(true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_u8_value_without_flags() {
        let parsed = parse_heart_rate_measurement(&[0x00, 72]).unwrap();
        assert_eq!(parsed.bpm, 72);
        assert_eq!(parsed.sensor_contact, None);
        assert_eq!(parsed.energy_expended_kj, None);
        assert_eq!(parsed.rr_intervals, 0);
    }

    #[test]
    fn parses_u16_value_with_contact_energy_and_rr() {
        // flags: u16 value, contact supported+detected, energy, RR
        let parsed =
            parse_heart_rate_measurement(&[0x1F, 0x2C, 0x01, 0x10, 0x27, 0x00, 0x04, 0x10, 0x04])
                .unwrap();
        assert_eq!(parsed.bpm, 300);
        assert_eq!(parsed.sensor_contact, Some(true));
        assert_eq!(parsed.energy_expended_kj, Some(10_000));
        assert_eq!(parsed.rr_intervals, 2);
    }

    #[test]
    fn reports_no_contact_when_supported_but_not_detected() {
        let parsed = parse_heart_rate_measurement(&[0x04, 0]).unwrap();
        assert_eq!(parsed.sensor_contact, Some(false));
        assert_eq!(parsed.bpm, 0);
    }

    #[test]
    fn rejects_truncated_packets() {
        assert!(parse_heart_rate_measurement(&[]).is_err());
        assert!(parse_heart_rate_measurement(&[0x01, 0x40]).is_err());
        assert!(parse_heart_rate_measurement(&[0x08, 0x40, 0x01]).is_err());
        assert!(parse_heart_rate_measurement(&[0x10, 0x40, 0x01]).is_err());
    }

    #[test]
    fn decoder_produces_heart_rate_readings() {
        let mut decoder = HeartRateDecoder;
        let reading = decoder.decode(&[0x06, 140], 0).unwrap().unwrap();
        assert_eq!(
            reading,
            Reading::HeartRate {
                bpm: 140,
                sensor_contact: Some(true)
            }
        );
        assert_eq!(decoder.describe(&reading), "140 bpm · contact");
        assert_eq!(decoder.characteristic(), 0x2A37);
    }

    #[test]
    fn simulator_warms_up() {
        let mut decoder = HeartRateDecoder;
        let start = match decoder.simulate(0, 0) {
            Reading::HeartRate { bpm, .. } => bpm,
            _ => unreachable!(),
        };
        let later = match decoder.simulate(600, 0) {
            Reading::HeartRate { bpm, .. } => bpm,
            _ => unreachable!(),
        };
        assert!((90..=100).contains(&start));
        assert!(later > start && later <= 155);
    }
}
