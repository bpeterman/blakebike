use std::{io::ErrorKind, time::Duration};

use serialport::{SerialPort, SerialPortType};

pub const DYNASTREAM_VENDOR_ID: u16 = 0x0fcf;
pub const USB_STICK2_PRODUCT_ID: u16 = 0x1008;

#[derive(Debug, Clone)]
pub struct PortInfo {
    pub path: String,
    pub label: String,
}

pub fn candidate_ports() -> Result<Vec<PortInfo>, String> {
    let ports = serialport::available_ports()
        .map_err(|error| format!("Could not enumerate serial devices: {error}"))?;
    Ok(ports
        .into_iter()
        .filter_map(|port| {
            let supported_usb = match &port.port_type {
                SerialPortType::UsbPort(info) => {
                    info.vid == DYNASTREAM_VENDOR_ID && info.pid == USB_STICK2_PRODUCT_ID
                }
                _ => false,
            };
            // The no-libudev Linux enumerator reads VID/PID from sysfs, so do
            // not probe unrelated CP210x/ttyUSB devices by path alone.
            if !supported_usb {
                return None;
            }
            let label = match port.port_type {
                SerialPortType::UsbPort(info) => {
                    info.product.unwrap_or_else(|| "ANT USBStick2".to_string())
                }
                _ => "ANT USBStick2".to_string(),
            };
            Some(PortInfo {
                path: port.port_name,
                label,
            })
        })
        .collect())
}

pub fn open(port: &PortInfo) -> Result<Box<dyn SerialPort>, String> {
    serialport::new(&port.path, 57_600)
        .timeout(Duration::from_millis(250))
        .open()
        .map_err(|error| format!("Could not open {}: {error}", port.path))
}

pub fn is_timeout(error: &std::io::Error) -> bool {
    error.kind() == ErrorKind::TimedOut
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_ids_are_the_observed_stick2() {
        assert_eq!(DYNASTREAM_VENDOR_ID, 0x0fcf);
        assert_eq!(USB_STICK2_PRODUCT_ID, 0x1008);
    }
}
