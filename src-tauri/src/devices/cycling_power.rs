//! Bluetooth Cycling Power Service (0x1818): Cycling Power Measurement (0x2A63).

use super::{Capability, DeviceInfo, crank::CrankCadence, fuser::Reading, sensor::Decoder};

pub const CYCLING_POWER_MEASUREMENT: u16 = 0x2A63;
pub const SIMULATED_POWER_METER_ID: &str = "simulated-power-meter";

pub fn simulated_device() -> DeviceInfo {
    DeviceInfo {
        id: SIMULATED_POWER_METER_ID.into(),
        name: "Simulated Power Meter".into(),
        simulated: true,
        rssi: Some(-45),
        capabilities: vec![Capability::CyclingPower],
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CyclingPowerMeasurement {
    pub instantaneous_power_watts: i16,
    /// Left-pedal share in percent when the meter reports balance.
    pub pedal_balance_left_percent: Option<f32>,
    pub crank: Option<CrankData>,
    pub wheel: Option<WheelData>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrankData {
    pub cumulative_revolutions: u16,
    /// 1/1024 s units.
    pub last_event_time: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WheelData {
    pub cumulative_revolutions: u32,
    /// 1/2048 s units.
    pub last_event_time: u16,
}

/// Parse a Cycling Power Measurement notification.
///
/// Flags (u16, bytes 0–1): bit 0 pedal power balance present, bit 1 balance
/// reference (1 = left), bit 2 accumulated torque present, bit 4 wheel
/// revolution data present, bit 5 crank revolution data present. Later flag
/// bits describe fields that come after the ones we read, so they are ignored.
pub fn parse_cycling_power_measurement(data: &[u8]) -> Result<CyclingPowerMeasurement, String> {
    let mut reader = Reader::new(data);
    let flags = reader.u16()?;
    let instantaneous_power_watts = reader.i16()?;
    let pedal_balance_left_percent = if flags & (1 << 0) != 0 {
        // Bit 1 says whether the value refers to the left pedal; meters that
        // leave it clear ("unknown") still mean left in practice.
        Some(f32::from(reader.u8()?) / 2.0)
    } else {
        None
    };
    if flags & (1 << 2) != 0 {
        reader.skip(2)?; // accumulated torque
    }
    let wheel = if flags & (1 << 4) != 0 {
        Some(WheelData {
            cumulative_revolutions: reader.u32()?,
            last_event_time: reader.u16()?,
        })
    } else {
        None
    };
    let crank = if flags & (1 << 5) != 0 {
        Some(CrankData {
            cumulative_revolutions: reader.u16()?,
            last_event_time: reader.u16()?,
        })
    } else {
        None
    };
    Ok(CyclingPowerMeasurement {
        instantaneous_power_watts,
        pedal_balance_left_percent,
        crank,
        wheel,
    })
}

/// Little-endian cursor shared by the cycling parsers.
pub(super) struct Reader<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        let end = self.offset + count;
        let slice = self.data.get(self.offset..end).ok_or_else(|| {
            format!(
                "truncated packet: needed {end} bytes, got {}",
                self.data.len()
            )
        })?;
        self.offset = end;
        Ok(slice)
    }

    pub fn skip(&mut self, count: usize) -> Result<(), String> {
        self.take(count).map(|_| ())
    }

    pub fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16, String> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    pub fn i16(&mut self) -> Result<i16, String> {
        let bytes = self.take(2)?;
        Ok(i16::from_le_bytes([bytes[0], bytes[1]]))
    }

    pub fn u32(&mut self) -> Result<u32, String> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }
}

/// Power meter as the Power role: watts plus cadence when crank data exists.
#[derive(Default)]
pub struct PowerDecoder {
    crank: CrankCadence,
    saw_crank_data: bool,
}

pub fn decoder(_device: &DeviceInfo) -> Box<dyn Decoder> {
    Box::new(PowerDecoder::default())
}

impl Decoder for PowerDecoder {
    fn characteristic(&self) -> u16 {
        CYCLING_POWER_MEASUREMENT
    }

    fn characteristic_name(&self) -> &'static str {
        "Cycling Power Measurement"
    }

    fn reset(&mut self) {
        self.crank.reset();
        self.saw_crank_data = false;
    }

    fn decode(&mut self, data: &[u8], now_ms: i64) -> Result<Option<Reading>, String> {
        let measurement = parse_cycling_power_measurement(data)?;
        let cadence_rpm = measurement.crank.and_then(|crank| {
            self.saw_crank_data = true;
            self.crank
                .update(crank.cumulative_revolutions, crank.last_event_time, now_ms)
        });
        Ok(Some(Reading::Power {
            watts: measurement.instantaneous_power_watts.max(0) as u16,
            cadence_rpm,
            balance_left_percent: measurement.pedal_balance_left_percent,
        }))
    }

    fn describe(&self, reading: &Reading) -> String {
        match reading {
            Reading::Power {
                watts,
                cadence_rpm,
                balance_left_percent,
            } => {
                let mut parts = vec![format!("{watts} W")];
                match cadence_rpm {
                    Some(rpm) => parts.push(format!("{rpm:.0} rpm")),
                    None if self.saw_crank_data => parts.push("crank data".into()),
                    None => parts.push("no crank data".into()),
                }
                if let Some(left) = balance_left_percent {
                    parts.push(format!("L {left:.0}% / R {:.0}%", 100.0 - left));
                }
                parts.join(" · ")
            }
            other => format!("{other:?}"),
        }
    }

    fn simulate(&mut self, tick: u64, now_ms: i64) -> Reading {
        // ~180 W with a slow surge; cadence goes through the real crank math so
        // the hold/coast path is exercised by the simulator too.
        let seconds = tick as f32 / 2.0;
        let watts = 180.0 + 25.0 * (seconds / 20.0).sin();
        let coasting = (tick / 2) % 60 >= 52; // coast for 8 s of every minute
        let (revolutions, event_time) = if coasting {
            (self.crank.last_revolutions(), self.crank.last_event_time())
        } else {
            simulated_crank(tick, 92.0)
        };
        let cadence_rpm = self.crank.update(revolutions, event_time, now_ms);
        Reading::Power {
            watts: if coasting { 0 } else { watts.round() as u16 },
            cadence_rpm,
            balance_left_percent: Some(49.0 + (seconds / 11.0).sin()),
        }
    }
}

/// Crank revolution data a sensor spinning at `rpm` would have sent by
/// `tick` (500 ms steps): the count of completed revolutions and the time of
/// the last one in 1/1024 s, both wrapping at 16 bits like the real thing.
pub(super) fn simulated_crank(tick: u64, rpm: f32) -> (u16, u16) {
    let seconds = tick as f64 / 2.0;
    let revolutions = (seconds * f64::from(rpm) / 60.0).floor();
    let last_event_seconds = revolutions * 60.0 / f64::from(rpm);
    (
        (revolutions as u64 & 0xFFFF) as u16,
        ((last_event_seconds * 1_024.0).round() as u64 & 0xFFFF) as u16,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_power_only() {
        let parsed = parse_cycling_power_measurement(&[0x00, 0x00, 0xC8, 0x00]).unwrap();
        assert_eq!(parsed.instantaneous_power_watts, 200);
        assert_eq!(parsed.crank, None);
        assert_eq!(parsed.wheel, None);
        assert_eq!(parsed.pedal_balance_left_percent, None);
    }

    #[test]
    fn parses_balance_torque_wheel_and_crank() {
        // flags: balance(0), reference left(1), torque(2), wheel(4), crank(5)
        let flags: u16 = 0b0011_0111;
        let mut data = flags.to_le_bytes().to_vec();
        data.extend_from_slice(&250i16.to_le_bytes()); // power
        data.push(104); // balance 52.0 %
        data.extend_from_slice(&[0x10, 0x00]); // torque
        data.extend_from_slice(&1_000u32.to_le_bytes()); // wheel revs
        data.extend_from_slice(&2_000u16.to_le_bytes()); // wheel time
        data.extend_from_slice(&321u16.to_le_bytes()); // crank revs
        data.extend_from_slice(&4_321u16.to_le_bytes()); // crank time
        let parsed = parse_cycling_power_measurement(&data).unwrap();
        assert_eq!(parsed.instantaneous_power_watts, 250);
        assert_eq!(parsed.pedal_balance_left_percent, Some(52.0));
        assert_eq!(
            parsed.wheel,
            Some(WheelData {
                cumulative_revolutions: 1_000,
                last_event_time: 2_000
            })
        );
        assert_eq!(
            parsed.crank,
            Some(CrankData {
                cumulative_revolutions: 321,
                last_event_time: 4_321
            })
        );
    }

    #[test]
    fn rejects_truncated_packets() {
        assert!(parse_cycling_power_measurement(&[0x00]).is_err());
        assert!(parse_cycling_power_measurement(&[0x00, 0x00, 0x10]).is_err());
        assert!(parse_cycling_power_measurement(&[0x20, 0x00, 0x10, 0x00, 0x01]).is_err());
    }

    #[test]
    fn decoder_yields_cadence_after_two_crank_packets() {
        let mut decoder = PowerDecoder::default();
        let packet = |revs: u16, time: u16| {
            let mut data = 0x20u16.to_le_bytes().to_vec();
            data.extend_from_slice(&200i16.to_le_bytes());
            data.extend_from_slice(&revs.to_le_bytes());
            data.extend_from_slice(&time.to_le_bytes());
            data
        };
        let first = decoder.decode(&packet(10, 0), 0).unwrap().unwrap();
        assert_eq!(
            first,
            Reading::Power {
                watts: 200,
                cadence_rpm: None,
                balance_left_percent: None
            }
        );
        assert_eq!(decoder.describe(&first), "200 W · crank data");
        let second = decoder.decode(&packet(13, 2_048), 2_000).unwrap().unwrap();
        match second {
            Reading::Power {
                cadence_rpm: Some(rpm),
                ..
            } => assert!((rpm - 90.0).abs() < 0.5),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn negative_power_clamps_to_zero() {
        let mut decoder = PowerDecoder::default();
        let mut data = 0u16.to_le_bytes().to_vec();
        data.extend_from_slice(&(-5i16).to_le_bytes());
        let reading = decoder.decode(&data, 0).unwrap().unwrap();
        assert!(matches!(reading, Reading::Power { watts: 0, .. }));
        assert_eq!(decoder.describe(&reading), "0 W · no crank data");
    }

    #[test]
    fn simulator_pedals_and_coasts() {
        let mut decoder = PowerDecoder::default();
        let mut saw_cadence = false;
        let mut saw_zero = false;
        for tick in 0..240u64 {
            match decoder.simulate(tick, tick as i64 * 500) {
                Reading::Power {
                    cadence_rpm: Some(rpm),
                    ..
                } if rpm > 80.0 && rpm < 100.0 => saw_cadence = true,
                Reading::Power {
                    cadence_rpm: Some(0.0),
                    watts: 0,
                    ..
                } => saw_zero = true,
                _ => {}
            }
        }
        assert!(
            saw_cadence,
            "simulator should report ~92 rpm while pedalling"
        );
        assert!(saw_zero, "simulator should report 0 rpm while coasting");
    }
}
