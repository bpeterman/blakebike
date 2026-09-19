use crate::domain::Telemetry;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const FITNESS_MACHINE_SERVICE: u16 = 0x1826;
pub const INDOOR_BIKE_DATA: u16 = 0x2AD2;
pub const FITNESS_MACHINE_FEATURE: u16 = 0x2ACC;
pub const SUPPORTED_POWER_RANGE: u16 = 0x2AD8;
pub const FITNESS_MACHINE_CONTROL_POINT: u16 = 0x2AD9;
pub const FITNESS_MACHINE_STATUS: u16 = 0x2ADA;

#[derive(Debug, Error, PartialEq)]
pub enum FtmsError {
    #[error("truncated FTMS packet")]
    Truncated,
    #[error("unexpected control-point response")]
    UnexpectedResponse,
    #[error("trainer rejected opcode {opcode:#04x}: {result:?}")]
    Rejected { opcode: u8, result: ResponseCode },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResponseCode {
    Success,
    OpcodeNotSupported,
    InvalidParameter,
    OperationFailed,
    ControlNotPermitted,
    Unknown(u8),
}

impl From<u8> for ResponseCode {
    fn from(value: u8) -> Self {
        match value {
            1 => Self::Success,
            2 => Self::OpcodeNotSupported,
            3 => Self::InvalidParameter,
            4 => Self::OperationFailed,
            5 => Self::ControlNotPermitted,
            other => Self::Unknown(other),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ControlOpcode {
    RequestControl = 0x00,
    Reset = 0x01,
    SetTargetResistance = 0x04,
    SetTargetPower = 0x05,
    StartOrResume = 0x07,
    StopOrPause = 0x08,
}

pub fn request_control() -> Vec<u8> {
    vec![ControlOpcode::RequestControl as u8]
}

pub fn start_or_resume() -> Vec<u8> {
    vec![ControlOpcode::StartOrResume as u8]
}

pub fn stop_or_pause(pause: bool) -> Vec<u8> {
    vec![ControlOpcode::StopOrPause as u8, if pause { 2 } else { 1 }]
}

pub fn set_target_power(watts: u16) -> Vec<u8> {
    let mut payload = vec![ControlOpcode::SetTargetPower as u8];
    payload.extend_from_slice(&(watts as i16).to_le_bytes());
    payload
}

pub fn parse_control_response(data: &[u8], expected_opcode: u8) -> Result<(), FtmsError> {
    if data.len() < 3 || data[0] != 0x80 || data[1] != expected_opcode {
        return Err(FtmsError::UnexpectedResponse);
    }
    let result = ResponseCode::from(data[2]);
    if result == ResponseCode::Success {
        Ok(())
    } else {
        Err(FtmsError::Rejected {
            opcode: expected_opcode,
            result,
        })
    }
}

pub fn parse_indoor_bike_data(data: &[u8], timestamp_ms: i64) -> Result<Telemetry, FtmsError> {
    let mut reader = Reader::new(data);
    let flags = reader.u16()?;
    let mut telemetry = Telemetry {
        timestamp_ms,
        ..Telemetry::default()
    };

    // "More Data" being clear means instantaneous speed is present.
    if flags & (1 << 0) == 0 {
        telemetry.speed_kph = Some(reader.u16()? as f32 / 100.0);
    }
    if flags & (1 << 1) != 0 {
        reader.skip(2)?; // average speed
    }
    if flags & (1 << 2) != 0 {
        telemetry.cadence_rpm = Some(reader.u16()? as f32 / 2.0);
    }
    if flags & (1 << 3) != 0 {
        reader.skip(2)?; // average cadence
    }
    if flags & (1 << 4) != 0 {
        reader.skip(3)?; // total distance
    }
    if flags & (1 << 5) != 0 {
        reader.skip(2)?; // resistance
    }
    if flags & (1 << 6) != 0 {
        telemetry.power_watts = reader.i16()?.max(0) as u16;
    }
    if flags & (1 << 7) != 0 {
        reader.skip(2)?; // average power
    }
    if flags & (1 << 8) != 0 {
        reader.skip(5)?; // total + per-hour energy
    }
    if flags & (1 << 9) != 0 {
        telemetry.heart_rate_bpm = Some(reader.u8()?);
    }
    Ok(telemetry)
}

struct Reader<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N], FtmsError> {
        let end = self.cursor + N;
        let slice = self
            .bytes
            .get(self.cursor..end)
            .ok_or(FtmsError::Truncated)?;
        self.cursor = end;
        Ok(slice.try_into().expect("slice length was checked"))
    }

    fn skip(&mut self, count: usize) -> Result<(), FtmsError> {
        if self.cursor + count > self.bytes.len() {
            return Err(FtmsError::Truncated);
        }
        self.cursor += count;
        Ok(())
    }

    fn u8(&mut self) -> Result<u8, FtmsError> {
        Ok(self.take::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16, FtmsError> {
        Ok(u16::from_le_bytes(self.take()?))
    }

    fn i16(&mut self) -> Result<i16, FtmsError> {
        Ok(i16::from_le_bytes(self.take()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_power_as_signed_little_endian() {
        assert_eq!(set_target_power(275), vec![0x05, 0x13, 0x01]);
    }

    #[test]
    fn parses_common_indoor_bike_packet() {
        // speed + cadence + power + heart rate
        let data = [
            0b0100_0100,
            0b0000_0010,
            0x2e,
            0x09, // 23.50 kph
            0xb4,
            0x00, // 90 rpm
            0xfa,
            0x00, // 250 W
            150,  // bpm
        ];
        let telemetry = parse_indoor_bike_data(&data, 123).unwrap();
        assert_eq!(telemetry.power_watts, 250);
        assert_eq!(telemetry.cadence_rpm, Some(90.0));
        assert_eq!(telemetry.speed_kph, Some(23.5));
        assert_eq!(telemetry.heart_rate_bpm, Some(150));
    }

    #[test]
    fn rejects_truncated_packets() {
        assert_eq!(
            parse_indoor_bike_data(&[0, 0], 0).unwrap_err(),
            FtmsError::Truncated
        );
    }
}
