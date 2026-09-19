//! Minimal ANT serial host for an ANT USBStick2 and the ANT+ HRM profile.

pub mod frame;
pub mod hrm;
mod io;
pub mod receiver;

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use serde::Serialize;
use serialport::{ClearBuffer, SerialPort};

use super::{Capability, DeviceInfo, DeviceTransport};
use frame::{Frame, Parser};

const CHANNEL: u8 = 0;
const ANT_PLUS_NETWORK_KEY: [u8; 8] = [0xB9, 0xA5, 0x21, 0xFB, 0xBD, 0x72, 0xC3, 0x45];

const MESG_RESPONSE_EVENT: u8 = 0x40;
const MESG_UNASSIGN_CHANNEL: u8 = 0x41;
const MESG_ASSIGN_CHANNEL: u8 = 0x42;
const MESG_CHANNEL_PERIOD: u8 = 0x43;
const MESG_SEARCH_TIMEOUT: u8 = 0x44;
const MESG_CHANNEL_RADIO_FREQ: u8 = 0x45;
const MESG_NETWORK_KEY: u8 = 0x46;
const MESG_SYSTEM_RESET: u8 = 0x4A;
const MESG_OPEN_CHANNEL: u8 = 0x4B;
const MESG_CLOSE_CHANNEL: u8 = 0x4C;
const MESG_REQUEST: u8 = 0x4D;
const MESG_BROADCAST_DATA: u8 = 0x4E;
const MESG_CHANNEL_ID: u8 = 0x51;
const MESG_STARTUP: u8 = 0x6F;

type OpenPort = (io::PortInfo, Box<dyn SerialPort>);

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum AdapterStatus {
    #[default]
    NotAttached,
    Ready {
        name: String,
    },
    PermissionDenied {
        message: String,
    },
    Busy {
        message: String,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SensorId {
    pub device_number: u16,
    pub transmission_type: u8,
}

impl SensorId {
    pub fn key(self) -> String {
        format!("ant:hrm:{}:{}", self.device_number, self.transmission_type)
    }

    pub fn parse(key: &str) -> Result<Self, String> {
        let mut parts = key.split(':');
        if parts.next() != Some("ant") || parts.next() != Some("hrm") {
            return Err("Invalid ANT heart-rate device id".into());
        }
        let device_number = parts
            .next()
            .ok_or("Missing ANT device number")?
            .parse()
            .map_err(|_| "Invalid ANT device number")?;
        let transmission_type = parts
            .next()
            .ok_or("Missing ANT transmission type")?
            .parse()
            .map_err(|_| "Invalid ANT transmission type")?;
        if parts.next().is_some() {
            return Err("Invalid ANT heart-rate device id".into());
        }
        Ok(Self {
            device_number,
            transmission_type,
        })
    }
}

#[derive(Clone)]
pub struct Ant {
    inner: Arc<Inner>,
}

struct Inner {
    enabled: bool,
    busy: AtomicBool,
    status: Mutex<AdapterStatus>,
}

struct AccessPermit {
    inner: Arc<Inner>,
}

impl Drop for AccessPermit {
    fn drop(&mut self) {
        self.inner.busy.store(false, Ordering::Release);
    }
}

impl Ant {
    pub fn new(enabled: bool) -> Self {
        Self {
            inner: Arc::new(Inner {
                enabled,
                busy: AtomicBool::new(false),
                status: Mutex::new(AdapterStatus::NotAttached),
            }),
        }
    }

    pub fn disabled() -> Self {
        Self::new(false)
    }

    pub fn status(&self) -> AdapterStatus {
        self.inner
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn set_status(&self, status: AdapterStatus) {
        *self
            .inner
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = status;
    }

    pub async fn scan_heart_rate(&self) -> Result<Vec<DeviceInfo>, String> {
        if !self.inner.enabled {
            return Ok(Vec::new());
        }
        let ant = self.clone();
        tokio::task::spawn_blocking(move || ant.scan_blocking())
            .await
            .map_err(|error| format!("ANT scan task failed: {error}"))?
    }

    fn scan_blocking(&self) -> Result<Vec<DeviceInfo>, String> {
        let _access = self.acquire()?;
        let Some((port_info, mut port)) = self.open_first()? else {
            self.set_status(AdapterStatus::NotAttached);
            return Ok(Vec::new());
        };
        self.set_status(AdapterStatus::Ready {
            name: port_info.label.clone(),
        });
        let mut protocol = Protocol::default();
        protocol.initialize(&mut *port)?;
        protocol.open_hr_channel(&mut *port, None)?;
        let (_measurement, raw) = protocol.wait_for_hr(&mut *port, Duration::from_secs(10))?;
        let id = protocol.request_channel_id(&mut *port, Duration::from_secs(2))?;
        protocol.close(&mut *port);
        tracing::debug!(device_number = id.device_number, raw = ?raw, "ANT HR sensor discovered");
        Ok(vec![device_info(id)])
    }

    pub async fn connect_heart_rate(&self, id: SensorId) -> Result<Session, String> {
        if !self.inner.enabled {
            return Err("ANT is disabled in this environment".into());
        }
        let ant = self.clone();
        tokio::task::spawn_blocking(move || ant.connect_blocking(id))
            .await
            .map_err(|error| format!("ANT connect task failed: {error}"))?
    }

    fn connect_blocking(&self, id: SensorId) -> Result<Session, String> {
        let access = self.acquire()?;
        let (port_info, mut port) = self
            .open_first()?
            .ok_or("No supported ANT USB stick is attached")?;
        self.set_status(AdapterStatus::Ready {
            name: port_info.label,
        });
        let mut protocol = Protocol::default();
        protocol.initialize(&mut *port)?;
        protocol.open_hr_channel(&mut *port, Some(id))?;
        Ok(Session {
            // Keep the global access guard for the life of the session. The
            // manager has one serial owner and cannot scan while connected.
            _access: access,
            port,
            protocol,
        })
    }

    fn open_first(&self) -> Result<Option<OpenPort>, String> {
        let candidates = io::candidate_ports()?;
        let Some(info) = candidates.into_iter().next() else {
            return Ok(None);
        };
        match io::open(&info) {
            Ok(port) => Ok(Some((info, port))),
            Err(message) => {
                let lower = message.to_lowercase();
                let status = if lower.contains("permission") || lower.contains("denied") {
                    AdapterStatus::PermissionDenied {
                        message: message.clone(),
                    }
                } else if lower.contains("busy") || lower.contains("exclusive") {
                    AdapterStatus::Busy {
                        message: message.clone(),
                    }
                } else {
                    AdapterStatus::Error {
                        message: message.clone(),
                    }
                };
                self.set_status(status);
                Err(message)
            }
        }
    }

    fn acquire(&self) -> Result<AccessPermit, String> {
        self.inner
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "ANT USB stick is busy with another BlakeBike operation".to_string())?;
        Ok(AccessPermit {
            inner: self.inner.clone(),
        })
    }
}

pub struct Session {
    _access: AccessPermit,
    port: Box<dyn SerialPort>,
    protocol: Protocol,
}

impl Session {
    /// Poll for one HR page. A quiet half-second is normal and returns None;
    /// transport/protocol failures are errors.
    pub fn next_heart_rate(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<(hrm::Measurement, [u8; 8])>, String> {
        match self.protocol.poll_hr(&mut *self.port, timeout) {
            Ok(value) => Ok(Some(value)),
            Err(error) if error.starts_with("timed out") => Ok(None),
            Err(error) => Err(error),
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.protocol.close(&mut *self.port);
    }
}

fn device_info(id: SensorId) -> DeviceInfo {
    DeviceInfo {
        id: id.key(),
        name: format!("ANT+ HR {}", id.device_number),
        transport: DeviceTransport::Ant,
        simulated: false,
        rssi: None,
        capabilities: vec![Capability::HeartRate],
    }
}

#[derive(Default)]
struct Protocol {
    parser: Parser,
}

impl Protocol {
    fn initialize(&mut self, port: &mut dyn SerialPort) -> Result<(), String> {
        let _ = port.clear(ClearBuffer::All);
        self.write(port, MESG_SYSTEM_RESET, &[0])?;
        self.wait_for(port, Duration::from_secs(2), |frame| {
            (frame.id == MESG_STARTUP).then_some(())
        })?;
        let mut network = vec![0];
        network.extend_from_slice(&ANT_PLUS_NETWORK_KEY);
        self.command(port, MESG_NETWORK_KEY, &network)?;
        Ok(())
    }

    fn open_hr_channel(
        &mut self,
        port: &mut dyn SerialPort,
        sensor: Option<SensorId>,
    ) -> Result<(), String> {
        self.command(port, MESG_ASSIGN_CHANNEL, &[CHANNEL, 0, 0])?;
        let id = sensor.unwrap_or(SensorId {
            device_number: 0,
            transmission_type: 0,
        });
        let [low, high] = id.device_number.to_le_bytes();
        self.command(
            port,
            MESG_CHANNEL_ID,
            &[CHANNEL, low, high, hrm::DEVICE_TYPE, id.transmission_type],
        )?;
        let [low, high] = hrm::CHANNEL_PERIOD.to_le_bytes();
        self.command(port, MESG_CHANNEL_PERIOD, &[CHANNEL, low, high])?;
        self.command(port, MESG_CHANNEL_RADIO_FREQ, &[CHANNEL, hrm::RF_FREQUENCY])?;
        self.command(port, MESG_SEARCH_TIMEOUT, &[CHANNEL, 4])?;
        self.command(port, MESG_OPEN_CHANNEL, &[CHANNEL])?;
        Ok(())
    }

    fn wait_for_hr(
        &mut self,
        port: &mut dyn SerialPort,
        timeout: Duration,
    ) -> Result<(hrm::Measurement, [u8; 8]), String> {
        self.wait_for(port, timeout, |frame| {
            if frame.id != MESG_BROADCAST_DATA || frame.payload.len() < 9 {
                return None;
            }
            let raw: [u8; 8] = frame.payload[1..9].try_into().ok()?;
            hrm::parse(&raw).ok().map(|measurement| (measurement, raw))
        })
        .map_err(|error| format!("No ANT+ heart-rate broadcast received: {error}"))
    }

    fn poll_hr(
        &mut self,
        port: &mut dyn SerialPort,
        timeout: Duration,
    ) -> Result<(hrm::Measurement, [u8; 8]), String> {
        self.wait_for(port, timeout, |frame| {
            if frame.id != MESG_BROADCAST_DATA || frame.payload.len() < 9 {
                return None;
            }
            let raw: [u8; 8] = frame.payload[1..9].try_into().ok()?;
            hrm::parse(&raw).ok().map(|measurement| (measurement, raw))
        })
    }

    fn request_channel_id(
        &mut self,
        port: &mut dyn SerialPort,
        timeout: Duration,
    ) -> Result<SensorId, String> {
        self.write(port, MESG_REQUEST, &[CHANNEL, MESG_CHANNEL_ID])?;
        self.wait_for(port, timeout, |frame| {
            if frame.id != MESG_CHANNEL_ID || frame.payload.len() < 5 {
                return None;
            }
            Some(SensorId {
                device_number: u16::from_le_bytes([frame.payload[1], frame.payload[2]]),
                transmission_type: frame.payload[4],
            })
        })
    }

    fn close(&mut self, port: &mut dyn SerialPort) {
        let _ = self.command(port, MESG_CLOSE_CHANNEL, &[CHANNEL]);
        let _ = self.command(port, MESG_UNASSIGN_CHANNEL, &[CHANNEL]);
    }

    fn command(&mut self, port: &mut dyn SerialPort, id: u8, payload: &[u8]) -> Result<(), String> {
        self.write(port, id, payload)?;
        self.wait_for(port, Duration::from_secs(2), |frame| {
            if frame.id != MESG_RESPONSE_EVENT || frame.payload.len() < 3 || frame.payload[1] != id
            {
                return None;
            }
            Some(frame.payload[2])
        })
        .and_then(|code| {
            if code == 0 {
                Ok(())
            } else {
                Err(format!(
                    "ANT command 0x{id:02X} failed with code 0x{code:02X}"
                ))
            }
        })
    }

    fn write(&self, port: &mut dyn SerialPort, id: u8, payload: &[u8]) -> Result<(), String> {
        port.write_all(
            &Frame {
                id,
                payload: payload.to_vec(),
            }
            .encode(),
        )
        .map_err(|error| format!("ANT write failed: {error}"))
    }

    fn wait_for<T>(
        &mut self,
        port: &mut dyn SerialPort,
        timeout: Duration,
        mut match_frame: impl FnMut(&Frame) -> Option<T>,
    ) -> Result<T, String> {
        let deadline = Instant::now() + timeout;
        let mut bytes = [0; 256];
        while Instant::now() < deadline {
            while let Some(frame) = self.parser.next() {
                if let Some(value) = match_frame(&frame) {
                    return Ok(value);
                }
            }
            match port.read(&mut bytes) {
                Ok(count) if count > 0 => self.parser.push(&bytes[..count]),
                Ok(_) => {}
                Err(error) if io::is_timeout(&error) => {}
                Err(error) => return Err(format!("ANT read failed: {error}")),
            }
        }
        Err(format!("timed out after {:.1}s", timeout.as_secs_f32()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensor_id_round_trips() {
        let id = SensorId {
            device_number: 12345,
            transmission_type: 5,
        };
        assert_eq!(SensorId::parse(&id.key()).unwrap(), id);
        assert!(SensorId::parse("ble:123").is_err());
    }
}
