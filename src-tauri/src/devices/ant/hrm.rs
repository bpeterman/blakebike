//! ANT+ Heart Rate Monitor profile fields common to every broadcast data page.

pub const DEVICE_TYPE: u8 = 120;
pub const CHANNEL_PERIOD: u16 = 8_070;
pub const RF_FREQUENCY: u8 = 57;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryStatus {
    New,
    Good,
    Ok,
    Low,
    Critical,
}

impl BatteryStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Good => "Good",
            Self::Ok => "OK",
            Self::Low => "Low",
            Self::Critical => "Critical",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Battery {
    pub percent: Option<u8>,
    /// Voltage in 1/256 V, avoiding floating-point comparisons in the decoder.
    pub voltage_256ths: Option<u16>,
    pub status: Option<BatteryStatus>,
}

impl Battery {
    pub fn voltage(self) -> Option<f32> {
        self.voltage_256ths.map(|value| f32::from(value) / 256.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Measurement {
    pub page: u8,
    pub heartbeat_event_time: u16,
    pub heartbeat_count: u8,
    pub bpm: Option<u8>,
    /// Optional battery fields carried by HRM data page 7.
    pub battery: Option<Battery>,
}

pub fn parse(data: &[u8]) -> Result<Measurement, String> {
    if data.len() != 8 {
        return Err(format!("ANT HR page must be 8 bytes, got {}", data.len()));
    }
    let page = data[0] & 0x7f;
    let battery = (page == 7).then(|| {
        let descriptive = data[3];
        let coarse_voltage = descriptive & 0x0f;
        let status = match (descriptive >> 4) & 0x07 {
            1 => Some(BatteryStatus::New),
            2 => Some(BatteryStatus::Good),
            3 => Some(BatteryStatus::Ok),
            4 => Some(BatteryStatus::Low),
            5 => Some(BatteryStatus::Critical),
            _ => None,
        };
        Battery {
            // 0x65-0xFE are reserved; 0xFF means not used.
            percent: (data[1] <= 100).then_some(data[1]),
            // A coarse value of 0x0F marks the complete voltage field invalid.
            voltage_256ths: (coarse_voltage != 0x0f)
                .then_some(u16::from(coarse_voltage) * 256 + u16::from(data[2])),
            status,
        }
    });
    Ok(Measurement {
        // Bit 7 is the page-change toggle, not part of the page number.
        page,
        heartbeat_event_time: u16::from_le_bytes([data[4], data[5]]),
        heartbeat_count: data[6],
        bpm: (data[7] != 0).then_some(data[7]),
        battery,
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
        assert_eq!(measurement.battery, None);
    }

    #[test]
    fn parses_page_seven_battery_percentage() {
        let measurement = parse(&[0x87, 82, 0x80, 0x22, 0x34, 0x12, 9, 147]).unwrap();
        assert_eq!(measurement.page, 7);
        assert_eq!(
            measurement.battery,
            Some(Battery {
                percent: Some(82),
                voltage_256ths: Some(2 * 256 + 128),
                status: Some(BatteryStatus::Good),
            })
        );
        assert_eq!(measurement.battery.unwrap().voltage(), Some(2.5));
        assert_eq!(measurement.bpm, Some(147));
    }

    #[test]
    fn ignores_missing_reserved_and_non_battery_levels() {
        let missing = parse(&[7, 0xff, 0xff, 0x7f, 0, 0, 0, 70])
            .unwrap()
            .battery
            .unwrap();
        assert_eq!(missing.percent, None);
        assert_eq!(missing.voltage_256ths, None);
        assert_eq!(missing.status, None);
        assert_eq!(
            parse(&[7, 101, 0, 0, 0, 0, 0, 70])
                .unwrap()
                .battery
                .unwrap()
                .percent,
            None
        );
        assert_eq!(parse(&[6, 82, 0, 0, 0, 0, 0, 70]).unwrap().battery, None);
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
