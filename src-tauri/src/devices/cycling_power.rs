//! Bluetooth Cycling Power Service (0x1818): Cycling Power Measurement
//! (0x2A63) for watts and cadence, Cycling Power Feature (0x2A65) for what the
//! meter supports, and the Cycling Power Control Point (0x2A66) for the one
//! rider-facing procedure a strain-gauge meter has: offset compensation, the
//! "zero offset" every vendor asks for before a ride.

use std::{fmt, time::Duration};

use chrono::Utc;

use super::{
    CalibrationDetail, CalibrationKind, CalibrationProgress, CalibrationRecord, Capability,
    DeviceInfo, DeviceRole, DeviceState,
    control_point::ControlError,
    crank::CrankCadence,
    fuser::{Metric, Reading, TelemetryFuser},
    sensor::{ControlPointSpec, Decoder, Sensor},
};

pub const CYCLING_POWER_MEASUREMENT: u16 = 0x2A63;
pub const CYCLING_POWER_FEATURE: u16 = 0x2A65;
pub const CYCLING_POWER_CONTROL_POINT: u16 = 0x2A66;
pub const SIMULATED_POWER_METER_ID: &str = "simulated-power-meter";

/// The spec gives a meter 30 s to finish a control-point procedure. Pedals
/// answer offset compensation in a few seconds; crank meters can take longer.
pub const ZERO_OFFSET_TIMEOUT: Duration = Duration::from_secs(30);

pub fn simulated_device() -> DeviceInfo {
    DeviceInfo {
        id: SIMULATED_POWER_METER_ID.into(),
        name: "Simulated Power Meter".into(),
        transport: Default::default(),
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
    /// The meter is asking to be zeroed (flag bit 12, Offset Compensation
    /// Indicator).
    pub offset_compensation_requested: bool,
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
/// revolution data present, bit 5 crank revolution data present, bit 12
/// offset compensation indicator. Later flag bits describe fields that come
/// after the ones we read, so they are ignored.
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
        offset_compensation_requested: flags & (1 << 12) != 0,
    })
}

/// The Cycling Power Feature word (u32): what this meter can do. Only the
/// bits that change our behaviour are kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CyclingPowerFeature {
    /// Bit 8: the meter sets the indicator flag when it wants a zero.
    pub offset_compensation_indicator: bool,
    /// Bit 9: the meter accepts Start Offset Compensation.
    pub offset_compensation: bool,
    /// Bit 19: CPS 1.1 enhanced procedure with manufacturer data.
    pub enhanced_offset_compensation: bool,
}

impl CyclingPowerFeature {
    pub fn parse(data: &[u8]) -> Result<Self, CpsError> {
        if data.len() < 4 {
            return Err(CpsError::Truncated);
        }
        let word = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        Ok(Self {
            offset_compensation_indicator: word & (1 << 8) != 0,
            offset_compensation: word & (1 << 9) != 0,
            enhanced_offset_compensation: word & (1 << 19) != 0,
        })
    }

    /// One line for the connect log.
    pub fn summary(&self) -> String {
        let mut parts = vec![if self.offset_compensation {
            "offset compensation supported"
        } else {
            "offset compensation not advertised"
        }];
        if self.enhanced_offset_compensation {
            parts.push("enhanced");
        }
        if self.offset_compensation_indicator {
            parts.push("asks when it needs a zero");
        }
        parts.join(" · ")
    }
}

/// Control Point opcodes we use, out of the profile's fifteen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ControlOpcode {
    StartOffsetCompensation = 0x0C,
    ResponseCode = 0x20,
}

/// The response value byte of a control-point indication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseValue {
    Success,
    OpCodeNotSupported,
    InvalidParameter,
    OperationFailed,
    Unknown(u8),
}

impl From<u8> for ResponseValue {
    fn from(value: u8) -> Self {
        match value {
            0x01 => ResponseValue::Success,
            0x02 => ResponseValue::OpCodeNotSupported,
            0x03 => ResponseValue::InvalidParameter,
            0x04 => ResponseValue::OperationFailed,
            other => ResponseValue::Unknown(other),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CpsError {
    Truncated,
    /// Not a response indication, or one for a different request.
    UnexpectedResponse(Vec<u8>),
    Rejected {
        opcode: u8,
        result: ResponseValue,
    },
}

impl fmt::Display for CpsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CpsError::Truncated => f.write_str("truncated Cycling Power packet"),
            CpsError::UnexpectedResponse(raw) => {
                write!(
                    f,
                    "unexpected control point response {}",
                    super::ble::hex(raw)
                )
            }
            CpsError::Rejected { opcode, result } => {
                write!(f, "meter rejected op 0x{opcode:02x}: {result:?}")
            }
        }
    }
}

/// The meter's answer to Start Offset Compensation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZeroOffset {
    /// In the meter's own units (ADC counts for most pedals, Nm/32 for some
    /// cranks). Only its drift between zeros means anything to the rider.
    pub raw: i16,
}

pub fn start_offset_compensation() -> Vec<u8> {
    vec![ControlOpcode::StartOffsetCompensation as u8]
}

/// Parse the indication answering Start Offset Compensation:
/// `[0x20, 0x0C, result, offset_lo, offset_hi]`.
pub fn parse_offset_compensation_response(data: &[u8]) -> Result<ZeroOffset, CpsError> {
    let opcode = ControlOpcode::StartOffsetCompensation as u8;
    if data.len() < 3 {
        return Err(CpsError::Truncated);
    }
    if data[0] != ControlOpcode::ResponseCode as u8 || data[1] != opcode {
        return Err(CpsError::UnexpectedResponse(data.to_vec()));
    }
    let result = ResponseValue::from(data[2]);
    if result != ResponseValue::Success {
        return Err(CpsError::Rejected { opcode, result });
    }
    if data.len() < 5 {
        return Err(CpsError::Truncated);
    }
    Ok(ZeroOffset {
        raw: i16::from_le_bytes([data[3], data[4]]),
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

/// Power meter as the Power role: watts plus cadence when crank data exists,
/// and the offset-compensation procedure on its control point.
#[derive(Default)]
pub struct PowerDecoder {
    crank: CrankCadence,
    saw_crank_data: bool,
    /// `None` until the feature characteristic has been read.
    features: Option<CyclingPowerFeature>,
    /// Latest value of the measurement's offset compensation indicator.
    calibration_requested: bool,
    /// Simulator: the zero it will report next, walked a little each time so
    /// the drift line has something to show.
    simulated_offset: i16,
    simulated_zeros: u32,
}

pub fn decoder(_device: &DeviceInfo) -> Box<dyn Decoder> {
    Box::new(PowerDecoder {
        simulated_offset: 1_020,
        // A freshly connected simulator asks for a zero until it gets one.
        calibration_requested: true,
        ..PowerDecoder::default()
    })
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
        self.features = None;
    }

    fn decode(&mut self, data: &[u8], now_ms: i64) -> Result<Option<Reading>, String> {
        let measurement = parse_cycling_power_measurement(data)?;
        let cadence_rpm = measurement.crank.and_then(|crank| {
            self.saw_crank_data = true;
            self.crank
                .update(crank.cumulative_revolutions, crank.last_event_time, now_ms)
        });
        self.calibration_requested = measurement.offset_compensation_requested;
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

    fn control_point(&self) -> Option<ControlPointSpec> {
        Some(ControlPointSpec {
            characteristic: CYCLING_POWER_CONTROL_POINT,
            feature: Some(CYCLING_POWER_FEATURE),
            name: "Cycling Power Control Point",
        })
    }

    fn features(&mut self, bytes: &[u8]) -> Result<String, String> {
        let features = CyclingPowerFeature::parse(bytes).map_err(|error| error.to_string())?;
        self.features = Some(features);
        Ok(features.summary())
    }

    /// A clear "not supported" bit is believed; an unreadable or missing
    /// feature word is not a reason to hide the button (the procedure's own
    /// answer settles it).
    fn calibration_supported(&self) -> bool {
        self.features
            .is_none_or(|features| features.offset_compensation)
    }

    fn calibration_requested(&self) -> bool {
        self.calibration_requested
    }

    fn simulate_procedure(&mut self, payload: &[u8], result: u8) -> Option<Vec<u8>> {
        let opcode = *payload.first()?;
        let mut response = vec![ControlOpcode::ResponseCode as u8, opcode];
        if opcode != ControlOpcode::StartOffsetCompensation as u8 {
            response.push(0x02); // op code not supported
            return Some(response);
        }
        response.push(result);
        if result == 0x01 {
            // Walk the zero by a few counts per call: -2, +3, -1, +2, …
            self.simulated_zeros += 1;
            let step = [-2i16, 3, -1, 2][(self.simulated_zeros % 4) as usize];
            self.simulated_offset += step;
            self.calibration_requested = false;
            response.extend_from_slice(&self.simulated_offset.to_le_bytes());
        }
        Some(response)
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

/// Why a zero must not start right now, from the meter's own latest readings.
/// A zero taken under load "succeeds" and biases every watt afterwards, so a
/// moving crank or a non-zero reading is refused outright. Meters that read
/// 0 W while coasting pass; this guards against the common mistake, not
/// against every possible load.
pub fn movement_blocker(watts: Option<f32>, cadence_rpm: Option<f32>) -> Option<String> {
    match (cadence_rpm, watts) {
        (Some(rpm), _) if rpm > 0.0 => Some(format!(
            "The meter is still moving ({rpm:.0} rpm). Unclip, stop the cranks and try again."
        )),
        (_, Some(watts)) if watts > 0.0 => Some(format!(
            "The meter still reads {watts:.0} W. Take your weight off the pedals and try again."
        )),
        _ => None,
    }
}

fn emit(sensor: &Sensor, phase: &'static str, message: &str, detail: Option<CalibrationDetail>) {
    sensor.slot().emit_calibration(CalibrationProgress {
        role: DeviceRole::Power,
        phase,
        message: Some(message.into()),
        detail,
    });
}

/// Zero a power meter: Start Offset Compensation over its control point,
/// with progress on `devices://calibration` and the answer recorded on the
/// slot. The hub has already refused this during a ride.
pub async fn zero_offset(
    sensor: &Sensor,
    fuser: &TelemetryFuser,
) -> Result<CalibrationRecord, String> {
    let slot = sensor.slot();
    let device = match slot.state().await {
        DeviceState::Ready { device } | DeviceState::Controlling { device } => device,
        _ => return Err("Connect a power meter before zeroing it".into()),
    };
    if !slot.stats().calibration_supported {
        return Err("This power meter does not advertise offset compensation".into());
    }
    if !sensor.begin_calibration() {
        return Err("Zero offset is already running".into());
    }
    if !device.simulated {
        let now = Utc::now().timestamp_millis();
        if let Some(reason) = movement_blocker(
            fuser.latest(Metric::Power, DeviceRole::Power, now),
            fuser.latest(Metric::Cadence, DeviceRole::Power, now),
        ) {
            sensor.end_calibration();
            slot.note("warn", "Zero offset refused", Some(reason.clone()));
            return Err(reason);
        }
    }

    slot.set_calibration(None, Some(true)).await;
    slot.note("info", "Zero offset started", None);
    emit(sensor, "preparing", "Unclip and keep the bike still.", None);
    let previous = slot.last_calibration().and_then(|record| record.offset_raw);
    let result = run(sensor, previous).await;
    sensor.end_calibration();
    slot.set_calibration(None, Some(false)).await;
    match result {
        Ok(offset) => {
            let record = CalibrationRecord {
                at: Utc::now(),
                kind: CalibrationKind::ZeroOffset,
                offset_raw: Some(offset.raw),
                previous_offset_raw: previous,
            };
            slot.record_calibration(Some(record.clone())).await;
            slot.set_calibration_requested(false);
            slot.note(
                "ok",
                "Zero offset complete",
                Some(match previous {
                    Some(previous) => format!(
                        "{} (was {previous}, drift {:+})",
                        offset.raw,
                        i32::from(offset.raw) - i32::from(previous)
                    ),
                    None => format!("{} (first zero on record)", offset.raw),
                }),
            );
            emit(
                sensor,
                "success",
                "Zero offset complete.",
                Some(CalibrationDetail::ZeroOffset {
                    offset_raw: offset.raw,
                    previous_offset_raw: previous,
                }),
            );
            Ok(record)
        }
        Err(error) => {
            slot.note("error", "Zero offset failed", Some(error.clone()));
            emit(sensor, "error", &error, None);
            Err(error)
        }
    }
}

async fn run(sensor: &Sensor, previous: Option<i16>) -> Result<ZeroOffset, String> {
    emit(
        sensor,
        "holdStill",
        "Keep the bike still. The meter is measuring its zero.",
        previous.map(|previous| CalibrationDetail::ZeroOffset {
            offset_raw: previous,
            previous_offset_raw: Some(previous),
        }),
    );
    let request = start_offset_compensation();
    let response = tokio::select! {
        response = sensor.procedure(&request, ZERO_OFFSET_TIMEOUT) => response,
        _ = sensor.cancelled() => return Err("Zeroing was cancelled.".into()),
    };
    let response = response.map_err(|error| match error {
        ControlError::NotConnected => {
            "The power meter disconnected before it answered.".to_string()
        }
        ControlError::Timeout => format!(
            "The meter did not answer within {} s. Wake it with one turn of the cranks, let it settle, and try again.",
            ZERO_OFFSET_TIMEOUT.as_secs()
        ),
        ControlError::Gatt(detail) => format!("Bluetooth error while zeroing: {detail}"),
        other => other.to_string(),
    })?;
    parse_offset_compensation_response(&response).map_err(|error| match error {
        CpsError::Rejected {
            result: ResponseValue::OperationFailed,
            ..
        } => "The meter refused the zero. Make sure nothing is touching the pedals or cranks and try again.".to_string(),
        CpsError::Rejected {
            result: ResponseValue::OpCodeNotSupported,
            ..
        } => "This meter does not support offset compensation over Bluetooth.".to_string(),
        CpsError::Rejected { result, .. } => {
            format!("The meter rejected the request ({result:?}).")
        }
        other => format!("The meter answered with something unexpected: {other}."),
    })
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
        assert!(!parsed.offset_compensation_requested);
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
    fn reads_the_offset_compensation_indicator() {
        let flags: u16 = 1 << 12;
        let mut data = flags.to_le_bytes().to_vec();
        data.extend_from_slice(&0i16.to_le_bytes());
        let parsed = parse_cycling_power_measurement(&data).unwrap();
        assert!(parsed.offset_compensation_requested);
        let mut decoder = PowerDecoder::default();
        decoder.decode(&data, 0).unwrap();
        assert!(decoder.calibration_requested());
        decoder.decode(&[0x00, 0x00, 0x00, 0x00], 500).unwrap();
        assert!(!decoder.calibration_requested());
    }

    #[test]
    fn rejects_truncated_packets() {
        assert!(parse_cycling_power_measurement(&[0x00]).is_err());
        assert!(parse_cycling_power_measurement(&[0x00, 0x00, 0x10]).is_err());
        assert!(parse_cycling_power_measurement(&[0x20, 0x00, 0x10, 0x00, 0x01]).is_err());
    }

    #[test]
    fn parses_the_feature_word() {
        assert_eq!(
            CyclingPowerFeature::parse(&[0, 0, 0]),
            Err(CpsError::Truncated)
        );
        let none = CyclingPowerFeature::parse(&[0x0B, 0x00, 0x00, 0x00]).unwrap();
        assert_eq!(none, CyclingPowerFeature::default());
        assert_eq!(none.summary(), "offset compensation not advertised");
        let word: u32 = (1 << 8) | (1 << 9) | (1 << 19);
        let all = CyclingPowerFeature::parse(&word.to_le_bytes()).unwrap();
        assert!(all.offset_compensation);
        assert!(all.offset_compensation_indicator);
        assert!(all.enhanced_offset_compensation);
        assert_eq!(
            all.summary(),
            "offset compensation supported · enhanced · asks when it needs a zero"
        );
    }

    #[test]
    fn decoder_believes_a_clear_feature_bit_but_assumes_when_unread() {
        let mut decoder = PowerDecoder::default();
        assert!(decoder.calibration_supported(), "unread features: assume");
        assert!(decoder.features(&[0x00, 0x00, 0x00, 0x00]).is_ok());
        assert!(!decoder.calibration_supported());
        decoder.features(&(1u32 << 9).to_le_bytes()).unwrap();
        assert!(decoder.calibration_supported());
        assert!(decoder.features(&[0x01]).is_err());
        assert!(decoder.control_point().is_some());
    }

    #[test]
    fn parses_offset_compensation_responses() {
        assert_eq!(start_offset_compensation(), vec![0x0C]);
        assert_eq!(
            parse_offset_compensation_response(&[0x20, 0x0C, 0x01, 0xFF, 0x03]),
            Ok(ZeroOffset { raw: 1_023 })
        );
        assert_eq!(
            parse_offset_compensation_response(&[0x20, 0x0C, 0x01, 0xFE, 0xFF]),
            Ok(ZeroOffset { raw: -2 })
        );
        assert_eq!(
            parse_offset_compensation_response(&[0x20, 0x0C, 0x04]),
            Err(CpsError::Rejected {
                opcode: 0x0C,
                result: ResponseValue::OperationFailed
            })
        );
        assert_eq!(
            parse_offset_compensation_response(&[0x20, 0x0C, 0x02]),
            Err(CpsError::Rejected {
                opcode: 0x0C,
                result: ResponseValue::OpCodeNotSupported
            })
        );
        assert_eq!(
            parse_offset_compensation_response(&[0x20, 0x0C, 0x09]),
            Err(CpsError::Rejected {
                opcode: 0x0C,
                result: ResponseValue::Unknown(9)
            })
        );
        assert_eq!(
            parse_offset_compensation_response(&[0x20, 0x05, 0x01]),
            Err(CpsError::UnexpectedResponse(vec![0x20, 0x05, 0x01]))
        );
        assert_eq!(
            parse_offset_compensation_response(&[0x80, 0x0C, 0x01, 0, 0]),
            Err(CpsError::UnexpectedResponse(vec![0x80, 0x0C, 0x01, 0, 0]))
        );
        assert_eq!(
            parse_offset_compensation_response(&[0x20, 0x0C]),
            Err(CpsError::Truncated)
        );
        assert_eq!(
            parse_offset_compensation_response(&[0x20, 0x0C, 0x01, 0xFF]),
            Err(CpsError::Truncated)
        );
    }

    #[test]
    fn movement_guard_refuses_a_turning_or_loaded_meter() {
        assert_eq!(movement_blocker(None, None), None);
        assert_eq!(movement_blocker(Some(0.0), Some(0.0)), None);
        assert!(
            movement_blocker(Some(0.0), Some(84.4))
                .unwrap()
                .contains("84 rpm")
        );
        assert!(movement_blocker(Some(12.0), None).unwrap().contains("12 W"));
        // Cadence is the clearer signal when both are present.
        assert!(
            movement_blocker(Some(12.0), Some(60.0))
                .unwrap()
                .contains("rpm")
        );
    }

    #[test]
    fn simulator_answers_zero_offset_and_refuses_other_opcodes() {
        let mut decoder = PowerDecoder {
            simulated_offset: 1_020,
            ..PowerDecoder::default()
        };
        let first = decoder.simulate_procedure(&[0x0C], 0x01).unwrap();
        let first = parse_offset_compensation_response(&first).unwrap();
        let second = decoder.simulate_procedure(&[0x0C], 0x01).unwrap();
        let second = parse_offset_compensation_response(&second).unwrap();
        assert_ne!(first.raw, second.raw, "the simulated zero drifts");
        assert!((first.raw - 1_020).abs() <= 3 && (second.raw - first.raw).abs() <= 3);
        assert_eq!(
            decoder.simulate_procedure(&[0x0C], 0x04),
            Some(vec![0x20, 0x0C, 0x04])
        );
        assert_eq!(
            decoder.simulate_procedure(&[0x04, 0x00], 0x01),
            Some(vec![0x20, 0x04, 0x02])
        );
        assert_eq!(decoder.simulate_procedure(&[], 0x01), None);
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

    mod hub {
        use std::{sync::atomic::Ordering, time::Duration};

        use super::super::*;
        use crate::devices::{DeviceHub, DeviceRole, DeviceState};

        async fn connected_hub() -> DeviceHub {
            let hub = DeviceHub::default();
            hub.connect(DeviceRole::Power, simulated_device())
                .await
                .unwrap();
            hub
        }

        #[tokio::test(start_paused = true)]
        async fn simulated_meter_zeros_and_remembers_the_drift() {
            let hub = connected_hub().await;
            let slot = hub.slot(DeviceRole::Power);
            assert!(slot.stats().calibration_supported);
            // The fresh simulator asks to be zeroed until it is.
            for _ in 0..20 {
                if slot.stats().calibration_requested {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            assert!(slot.stats().calibration_requested);

            let first = hub.calibrate(DeviceRole::Power).await.unwrap();
            assert_eq!(first.kind, CalibrationKind::ZeroOffset);
            let first_offset = first.offset_raw.unwrap();
            assert!((first_offset - 1_020).abs() <= 3);
            let stats = slot.stats();
            assert!(!stats.calibrating);
            assert!(!stats.calibration_requested);
            assert_eq!(stats.last_calibration, Some(first.clone()));

            let second = hub.calibrate(DeviceRole::Power).await.unwrap();
            assert_ne!(second.offset_raw, first.offset_raw);
            let log = slot.log_lines();
            let complete: Vec<_> = log
                .iter()
                .filter(|line| line.step == "Zero offset complete")
                .collect();
            assert_eq!(complete.len(), 2);
            assert!(
                complete[0]
                    .detail
                    .as_deref()
                    .unwrap()
                    .contains("first zero on record")
            );
            assert!(
                complete[1]
                    .detail
                    .as_deref()
                    .unwrap()
                    .contains(&format!("was {first_offset}"))
            );
            hub.disconnect().await;
        }

        #[tokio::test(start_paused = true)]
        async fn refusals_timeouts_and_cancels_are_reported() {
            let hub = std::sync::Arc::new(connected_hub().await);
            let faults = hub.simulated_sensor_faults(DeviceRole::Power).unwrap();
            faults.refuse_with.store(0x04, Ordering::Relaxed);
            let error = hub.calibrate(DeviceRole::Power).await.unwrap_err();
            assert!(error.contains("refused the zero"), "{error}");
            faults.refuse_with.store(0x02, Ordering::Relaxed);
            let error = hub.calibrate(DeviceRole::Power).await.unwrap_err();
            assert!(error.contains("does not support"), "{error}");

            faults.ack_delay_ms.store(
                ZERO_OFFSET_TIMEOUT.as_millis() as u64 + 5_000,
                Ordering::Relaxed,
            );
            let error = hub.calibrate(DeviceRole::Power).await.unwrap_err();
            assert!(error.contains("did not answer within 30 s"), "{error}");
            faults.ack_delay_ms.store(0, Ordering::Relaxed);
            // The late answer must not be taken as the next zero's.
            let record = hub.calibrate(DeviceRole::Power).await.unwrap();
            assert!(record.offset_raw.is_some());

            faults.ack_delay_ms.store(10_000, Ordering::Relaxed);
            let runner = {
                let hub = hub.clone();
                tokio::spawn(async move { hub.calibrate(DeviceRole::Power).await })
            };
            tokio::time::sleep(Duration::from_millis(500)).await;
            assert!(hub.slot(DeviceRole::Power).stats().calibrating);
            hub.cancel_calibration(DeviceRole::Power).unwrap();
            let error = runner.await.unwrap().unwrap_err();
            assert!(error.contains("cancelled"), "{error}");
            assert!(!hub.slot(DeviceRole::Power).stats().calibrating);
            hub.disconnect().await;
        }

        #[tokio::test(start_paused = true)]
        async fn disconnect_mid_procedure_ends_it() {
            let hub = std::sync::Arc::new(connected_hub().await);
            let faults = hub.simulated_sensor_faults(DeviceRole::Power).unwrap();
            faults.ack_delay_ms.store(10_000, Ordering::Relaxed);
            let runner = {
                let hub = hub.clone();
                tokio::spawn(async move { hub.calibrate(DeviceRole::Power).await })
            };
            tokio::time::sleep(Duration::from_millis(500)).await;
            hub.disconnect_role(DeviceRole::Power).await;
            let error = runner.await.unwrap().unwrap_err();
            assert!(error.contains("disconnected"), "{error}");
            assert!(matches!(
                hub.slot(DeviceRole::Power).state().await,
                DeviceState::Idle
            ));
            assert!(!hub.slot(DeviceRole::Power).stats().calibration_supported);
        }

        #[tokio::test]
        async fn zeroing_is_refused_without_a_meter_during_a_ride_or_for_other_roles() {
            let hub = DeviceHub::default();
            assert_eq!(
                hub.calibrate(DeviceRole::Power).await.unwrap_err(),
                "Connect a power meter before zeroing it"
            );
            assert_eq!(
                hub.calibrate(DeviceRole::HeartRate).await.unwrap_err(),
                "Heart rate has no calibration procedure"
            );
            assert!(hub.cancel_calibration(DeviceRole::Trainer).is_err());
            hub.connect(DeviceRole::Power, simulated_device())
                .await
                .unwrap();
            hub.set_ride_active(true);
            assert_eq!(
                hub.calibrate(DeviceRole::Power).await.unwrap_err(),
                "Zero offset is unavailable during a ride"
            );
            hub.set_ride_active(false);
            hub.disconnect().await;
        }
    }
}
