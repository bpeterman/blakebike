//! The Cadence role: a dedicated Cycling Speed and Cadence sensor (0x1816,
//! CSC Measurement 0x2A5B), or a power meter's crank data used for cadence
//! only when the user assigns a power meter to this role.

use super::{
    Capability, DeviceInfo,
    crank::CrankCadence,
    cycling_power::{
        CYCLING_POWER_MEASUREMENT, Reader, parse_cycling_power_measurement, simulated_crank,
    },
    fuser::Reading,
    sensor::Decoder,
};

pub const CSC_MEASUREMENT: u16 = 0x2A5B;
pub const SIMULATED_CADENCE_ID: &str = "simulated-cadence";

pub fn simulated_device() -> DeviceInfo {
    DeviceInfo {
        id: SIMULATED_CADENCE_ID.into(),
        name: "Simulated Cadence Sensor".into(),
        transport: Default::default(),
        simulated: true,
        rssi: Some(-50),
        capabilities: vec![Capability::Csc],
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CscMeasurement {
    pub wheel: Option<(u32, u16)>,
    /// Cumulative crank revolutions and last crank event time (1/1024 s).
    pub crank: Option<(u16, u16)>,
}

/// Parse a CSC Measurement notification. Flags (u8): bit 0 wheel revolution
/// data present (u32 revolutions + u16 event time in 1/1024 s), bit 1 crank
/// revolution data present (u16 revolutions + u16 event time).
pub fn parse_csc_measurement(data: &[u8]) -> Result<CscMeasurement, String> {
    let mut reader = Reader::new(data);
    let flags = reader.u8()?;
    let wheel = if flags & 0x01 != 0 {
        Some((reader.u32()?, reader.u16()?))
    } else {
        None
    };
    let crank = if flags & 0x02 != 0 {
        Some((reader.u16()?, reader.u16()?))
    } else {
        None
    };
    Ok(CscMeasurement { wheel, crank })
}

/// Pick the decoder for a device assigned to the Cadence role.
pub fn decoder(device: &DeviceInfo) -> Box<dyn Decoder> {
    if !device.supports(Capability::Csc) && device.supports(Capability::CyclingPower) {
        Box::new(CadenceDecoder::from_power_meter())
    } else {
        Box::new(CadenceDecoder::default())
    }
}

#[derive(Default)]
pub struct CadenceDecoder {
    crank: CrankCadence,
    from_power_meter: bool,
}

impl CadenceDecoder {
    pub fn from_power_meter() -> Self {
        Self {
            crank: CrankCadence::default(),
            from_power_meter: true,
        }
    }

    fn crank_data(&self, data: &[u8]) -> Result<Option<(u16, u16)>, String> {
        if self.from_power_meter {
            Ok(parse_cycling_power_measurement(data)?
                .crank
                .map(|crank| (crank.cumulative_revolutions, crank.last_event_time)))
        } else {
            Ok(parse_csc_measurement(data)?.crank)
        }
    }
}

impl Decoder for CadenceDecoder {
    fn characteristic(&self) -> u16 {
        if self.from_power_meter {
            CYCLING_POWER_MEASUREMENT
        } else {
            CSC_MEASUREMENT
        }
    }

    fn characteristic_name(&self) -> &'static str {
        if self.from_power_meter {
            "Cycling Power Measurement (crank data)"
        } else {
            "CSC Measurement"
        }
    }

    fn reset(&mut self) {
        self.crank.reset();
    }

    fn decode(&mut self, data: &[u8], now_ms: i64) -> Result<Option<Reading>, String> {
        let (revolutions, event_time) = self
            .crank_data(data)?
            .ok_or("packet carries no crank revolution data")?;
        Ok(self
            .crank
            .update(revolutions, event_time, now_ms)
            .map(|rpm| Reading::Cadence { rpm }))
    }

    fn describe(&self, reading: &Reading) -> String {
        match reading {
            Reading::Cadence { rpm } if *rpm == 0.0 => "0 rpm · coasting".into(),
            Reading::Cadence { rpm } => format!("{rpm:.0} rpm"),
            other => format!("{other:?}"),
        }
    }

    fn simulate(&mut self, tick: u64, now_ms: i64) -> Reading {
        let coasting = (tick / 2) % 45 >= 40; // coast 5 s of every 45 s
        let (revolutions, event_time) = if coasting {
            (self.crank.last_revolutions(), self.crank.last_event_time())
        } else {
            simulated_crank(tick, 85.0)
        };
        Reading::Cadence {
            rpm: self
                .crank
                .update(revolutions, event_time, now_ms)
                .unwrap_or(0.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_crank_only_packet() {
        let parsed = parse_csc_measurement(&[0x02, 0x10, 0x00, 0x00, 0x04]).unwrap();
        assert_eq!(parsed.crank, Some((16, 1_024)));
        assert_eq!(parsed.wheel, None);
    }

    #[test]
    fn parses_wheel_and_crank_packet() {
        let parsed = parse_csc_measurement(&[
            0x03, 0xE8, 0x03, 0x00, 0x00, 0x00, 0x08, 0x05, 0x00, 0x00, 0x02,
        ])
        .unwrap();
        assert_eq!(parsed.wheel, Some((1_000, 2_048)));
        assert_eq!(parsed.crank, Some((5, 512)));
    }

    #[test]
    fn rejects_truncated_packets() {
        assert!(parse_csc_measurement(&[]).is_err());
        assert!(parse_csc_measurement(&[0x02, 0x01]).is_err());
        assert!(parse_csc_measurement(&[0x01, 0x01, 0x02]).is_err());
    }

    #[test]
    fn csc_decoder_reports_cadence_and_coasting() {
        let mut decoder = CadenceDecoder::default();
        let packet = |revs: u16, time: u16| {
            let mut data = vec![0x02];
            data.extend_from_slice(&revs.to_le_bytes());
            data.extend_from_slice(&time.to_le_bytes());
            data
        };
        assert_eq!(decoder.decode(&packet(0, 0), 0).unwrap(), None);
        let reading = decoder.decode(&packet(3, 2_048), 2_000).unwrap().unwrap();
        match reading {
            Reading::Cadence { rpm } => assert!((rpm - 90.0).abs() < 0.5),
            other => panic!("{other:?}"),
        }
        assert_eq!(decoder.describe(&reading), "90 rpm");
        let stalled = decoder.decode(&packet(3, 2_048), 5_000).unwrap().unwrap();
        assert_eq!(stalled, Reading::Cadence { rpm: 0.0 });
        assert_eq!(decoder.describe(&stalled), "0 rpm · coasting");
    }

    #[test]
    fn wheel_only_sensor_is_an_error_not_silence() {
        let mut decoder = CadenceDecoder::default();
        let error = decoder
            .decode(&[0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x04], 0)
            .unwrap_err();
        assert!(error.contains("no crank revolution data"));
    }

    #[test]
    fn power_meter_can_serve_as_cadence_sensor() {
        let power_meter = DeviceInfo {
            id: "pm".into(),
            name: "Crank".into(),
            transport: Default::default(),
            simulated: false,
            rssi: None,
            capabilities: vec![Capability::CyclingPower],
        };
        let mut decoder = decoder(&power_meter);
        assert_eq!(decoder.characteristic(), CYCLING_POWER_MEASUREMENT);
        let packet = |revs: u16, time: u16| {
            let mut data = 0x20u16.to_le_bytes().to_vec();
            data.extend_from_slice(&150i16.to_le_bytes());
            data.extend_from_slice(&revs.to_le_bytes());
            data.extend_from_slice(&time.to_le_bytes());
            data
        };
        assert_eq!(decoder.decode(&packet(10, 1_024), 0).unwrap(), None);
        let reading = decoder.decode(&packet(11, 2_048), 1_000).unwrap().unwrap();
        assert!(matches!(reading, Reading::Cadence { rpm } if (rpm - 60.0).abs() < 0.5));
        let csc = decoder_for_csc();
        assert_eq!(csc.characteristic(), CSC_MEASUREMENT);
    }

    fn decoder_for_csc() -> Box<dyn Decoder> {
        decoder(&simulated_device())
    }

    #[test]
    fn simulator_pedals_at_85_and_coasts() {
        let mut decoder = CadenceDecoder::default();
        let mut steady = 0;
        let mut coasting = 0;
        for tick in 1..180u64 {
            match decoder.simulate(tick, tick as i64 * 500) {
                Reading::Cadence { rpm } if (rpm - 85.0).abs() < 1.0 => steady += 1,
                Reading::Cadence { rpm: 0.0 } => coasting += 1,
                _ => {}
            }
        }
        assert!(steady > 100, "steady ticks: {steady}");
        assert!(coasting > 5, "coasting ticks: {coasting}");
    }
}
