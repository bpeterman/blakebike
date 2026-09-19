//! ANT+ Heart Rate Monitor profile fields common to every broadcast data page.

pub const DEVICE_TYPE: u8 = 120;
pub const CHANNEL_PERIOD: u16 = 8_070;
pub const RF_FREQUENCY: u8 = 57;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Measurement {
    pub page: u8,
    pub heartbeat_event_time: u16,
    pub heartbeat_count: u8,
    pub bpm: Option<u8>,
    /// Optional remaining level from HRM data page 7. Values above 100 are
    /// reserved by the profile; 0xFF means the sensor does not report it.
    pub battery_percent: Option<u8>,
}

pub fn parse(data: &[u8]) -> Result<Measurement, String> {
    if data.len() != 8 {
        return Err(format!("ANT HR page must be 8 bytes, got {}", data.len()));
    }
    let page = data[0] & 0x7f;
    Ok(Measurement {
        // Bit 7 is the page-change toggle, not part of the page number.
        page,
        heartbeat_event_time: u16::from_le_bytes([data[4], data[5]]),
        heartbeat_count: data[6],
        bpm: (data[7] != 0).then_some(data[7]),
        battery_percent: (page == 7 && data[1] <= 100).then_some(data[1]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_fields_and_toggle() {
        let measurement = parse(&[0x84, 0xff, 0xff, 0xff, 0x34, 0x12, 9, 147]).unwrap();
        assert_eq!(measurement.page, 4);
        assert_eq!(measurement.heartbeat_event_time, 0x1234);
        assert_eq!(measurement.heartbeat_count, 9);
        assert_eq!(measurement.bpm, Some(147));
        assert_eq!(measurement.battery_percent, None);
    }

    #[test]
    fn parses_page_seven_battery_percentage() {
        let measurement = parse(&[0x87, 82, 0x80, 0x22, 0x34, 0x12, 9, 147]).unwrap();
        assert_eq!(measurement.page, 7);
        assert_eq!(measurement.battery_percent, Some(82));
        assert_eq!(measurement.bpm, Some(147));
    }

    #[test]
    fn ignores_missing_reserved_and_non_battery_levels() {
        assert_eq!(
            parse(&[7, 0xff, 0, 0, 0, 0, 0, 70])
                .unwrap()
                .battery_percent,
            None
        );
        assert_eq!(
            parse(&[7, 101, 0, 0, 0, 0, 0, 70]).unwrap().battery_percent,
            None
        );
        assert_eq!(
            parse(&[6, 82, 0, 0, 0, 0, 0, 70]).unwrap().battery_percent,
            None
        );
    }

    #[test]
    fn zero_bpm_is_missing() {
        assert_eq!(parse(&[0; 8]).unwrap().bpm, None);
    }

    #[test]
    fn rejects_wrong_size() {
        assert!(parse(&[0; 7]).is_err());
        assert!(parse(&[0; 9]).is_err());
    }
}
