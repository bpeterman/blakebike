use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU16, Ordering},
    },
    time::Duration,
};

use btleplug::{
    api::{
        Central, Characteristic, Manager as _, Peripheral as _, ScanFilter, WriteType,
        bleuuid::BleUuid,
    },
    platform::{Adapter, Manager, Peripheral},
};
use chrono::Utc;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::{
    sync::{Mutex, RwLock},
    task::JoinHandle,
};
use uuid::Uuid;

use crate::{
    domain::Telemetry,
    ftms::{
        FITNESS_MACHINE_CONTROL_POINT, FITNESS_MACHINE_SERVICE, INDOOR_BIKE_DATA,
        ControlOpcode, parse_indoor_bike_data, request_control, set_target_power, start_or_resume,
        stop_or_pause,
    },
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub simulated: bool,
    pub rssi: Option<i16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum DeviceState {
    Idle,
    Scanning,
    Connecting { name: String },
    Ready { device: DeviceInfo },
    Controlling { device: DeviceInfo },
    Reconnecting { name: String },
    Error { message: String, guidance: String },
}

pub struct DeviceManager {
    state: Arc<RwLock<DeviceState>>,
    adapter: Mutex<Option<Adapter>>,
    discovered: Mutex<HashMap<String, Peripheral>>,
    peripheral: Arc<RwLock<Option<Peripheral>>>,
    control_point: Arc<RwLock<Option<Characteristic>>>,
    target_power: Arc<AtomicU16>,
    min_power: AtomicU16,
    max_power: AtomicU16,
    worker: Mutex<Option<JoinHandle<()>>>,
    command_lock: Mutex<()>,
}

impl Default for DeviceManager {
    fn default() -> Self {
        Self {
            state: Arc::new(RwLock::new(DeviceState::Idle)),
            adapter: Mutex::new(None),
            discovered: Mutex::new(HashMap::new()),
            peripheral: Arc::new(RwLock::new(None)),
            control_point: Arc::new(RwLock::new(None)),
            target_power: Arc::new(AtomicU16::new(100)),
            min_power: AtomicU16::new(0),
            max_power: AtomicU16::new(2_000),
            worker: Mutex::new(None),
            command_lock: Mutex::new(()),
        }
    }
}

impl DeviceManager {
    pub async fn state(&self) -> DeviceState {
        self.state.read().await.clone()
    }

    pub async fn scan(&self) -> Result<Vec<DeviceInfo>, String> {
        *self.state.write().await = DeviceState::Scanning;
        let result = self.scan_inner().await;
        if result.is_err() {
            *self.state.write().await = DeviceState::Error {
                message: result.as_ref().unwrap_err().clone(),
                guidance: platform_guidance(),
            };
        } else {
            *self.state.write().await = DeviceState::Idle;
        }
        result
    }

    async fn scan_inner(&self) -> Result<Vec<DeviceInfo>, String> {
        let adapter = self.adapter().await?;
        // Linux/BlueZ merges filters from all clients, so filter results ourselves below.
        adapter
            .start_scan(ScanFilter::default())
            .await
            .map_err(|error| format!("Could not start Bluetooth scan: {error}"))?;
        tokio::time::sleep(Duration::from_secs(3)).await;
        let peripherals = adapter
            .peripherals()
            .await
            .map_err(|error| format!("Could not read Bluetooth devices: {error}"))?;
        let mut devices = vec![DeviceInfo {
            id: "simulated-trainer".into(),
            name: "BlakeBike Simulator".into(),
            simulated: true,
            rssi: Some(-30),
        }];
        let mut discovered = self.discovered.lock().await;
        discovered.clear();
        for peripheral in peripherals {
            let Ok(Some(properties)) = peripheral.properties().await else {
                continue;
            };
            let is_ftms = properties
                .services
                .iter()
                .any(|uuid| *uuid == Uuid::from_u16(FITNESS_MACHINE_SERVICE));
            if !is_ftms {
                continue;
            }
            let id = peripheral.id().to_string();
            devices.push(DeviceInfo {
                id: id.clone(),
                name: properties
                    .local_name
                    .unwrap_or_else(|| "FTMS trainer".into()),
                simulated: false,
                rssi: properties.rssi,
            });
            discovered.insert(id, peripheral);
        }
        Ok(devices)
    }

    async fn adapter(&self) -> Result<Adapter, String> {
        if let Some(adapter) = self.adapter.lock().await.clone() {
            return Ok(adapter);
        }
        let manager = Manager::new()
            .await
            .map_err(|error| format!("Bluetooth is unavailable: {error}"))?;
        let adapter = manager
            .adapters()
            .await
            .map_err(|error| format!("Could not enumerate Bluetooth adapters: {error}"))?
            .into_iter()
            .next()
            .ok_or_else(|| "No Bluetooth adapter was found".to_string())?;
        *self.adapter.lock().await = Some(adapter.clone());
        Ok(adapter)
    }

    pub async fn connect(&self, app: AppHandle, device: DeviceInfo) -> Result<(), String> {
        self.disconnect().await;
        *self.state.write().await = DeviceState::Connecting {
            name: device.name.clone(),
        };
        if device.simulated {
            self.connect_simulator(app, device).await;
            return Ok(());
        }
        let peripheral = self
            .discovered
            .lock()
            .await
            .get(&device.id)
            .cloned()
            .ok_or_else(|| "Trainer is no longer available; scan again".to_string())?;
        peripheral
            .connect()
            .await
            .map_err(|error| format!("Could not connect to trainer: {error}"))?;
        peripheral
            .discover_services()
            .await
            .map_err(|error| format!("Could not discover trainer services: {error}"))?;
        let characteristics = peripheral.characteristics();
        let indoor_data = characteristics
            .iter()
            .find(|characteristic| characteristic.uuid == Uuid::from_u16(INDOOR_BIKE_DATA))
            .cloned()
            .ok_or_else(|| "Trainer does not expose Indoor Bike Data".to_string())?;
        let control = characteristics
            .iter()
            .find(|characteristic| {
                characteristic.uuid == Uuid::from_u16(FITNESS_MACHINE_CONTROL_POINT)
            })
            .cloned()
            .ok_or_else(|| "Trainer does not support FTMS control".to_string())?;
        peripheral
            .subscribe(&indoor_data)
            .await
            .map_err(|error| format!("Could not subscribe to trainer data: {error}"))?;
        peripheral
            .subscribe(&control)
            .await
            .map_err(|error| format!("Could not subscribe to trainer control: {error}"))?;
        peripheral
            .write(&control, &request_control(), WriteType::WithResponse)
            .await
            .map_err(|error| format!("Could not request trainer control: {error}"))?;
        *self.peripheral.write().await = Some(peripheral.clone());
        *self.control_point.write().await = Some(control);
        *self.state.write().await = DeviceState::Ready {
            device: device.clone(),
        };
        let state = self.state.clone();
        let worker = tokio::spawn(async move {
            let Ok(mut notifications) = peripheral.notifications().await else {
                return;
            };
            while let Some(notification) = notifications.next().await {
                if notification.uuid == Uuid::from_u16(INDOOR_BIKE_DATA) {
                    if let Ok(telemetry) =
                        parse_indoor_bike_data(&notification.value, Utc::now().timestamp_millis())
                    {
                        let _ = app.emit("trainer://telemetry", telemetry);
                    }
                }
            }
            *state.write().await = DeviceState::Reconnecting {
                name: device.name,
            };
        });
        *self.worker.lock().await = Some(worker);
        Ok(())
    }

    async fn connect_simulator(&self, app: AppHandle, device: DeviceInfo) {
        *self.state.write().await = DeviceState::Ready {
            device: device.clone(),
        };
        let target = self.target_power.clone();
        let state = self.state.clone();
        let worker = tokio::spawn(async move {
            let mut power = 90.0_f32;
            let mut tick = tokio::time::interval(Duration::from_millis(500));
            loop {
                tick.tick().await;
                let requested = target.load(Ordering::Relaxed) as f32;
                power += (requested - power) * 0.18;
                let elapsed = Utc::now().timestamp_millis() as f32 / 1_000.0;
                let wobble = elapsed.sin() * 3.0;
                let telemetry = Telemetry {
                    timestamp_ms: Utc::now().timestamp_millis(),
                    power_watts: (power + wobble).max(0.0) as u16,
                    cadence_rpm: Some(88.0 + wobble / 2.0),
                    speed_kph: Some(30.0 + wobble / 3.0),
                    heart_rate_bpm: Some((125.0 + requested / 20.0).min(185.0) as u8),
                    target_power_watts: Some(requested as u16),
                };
                if app.emit("trainer://telemetry", telemetry).is_err() {
                    break;
                }
            }
            *state.write().await = DeviceState::Idle;
        });
        *self.worker.lock().await = Some(worker);
    }

    pub async fn begin_control(&self) -> Result<(), String> {
        let device = match self.state.read().await.clone() {
            DeviceState::Ready { device } | DeviceState::Controlling { device } => device,
            _ => return Err("Connect a trainer before starting a workout".into()),
        };
        self.write_control(&start_or_resume()).await?;
        *self.state.write().await = DeviceState::Controlling { device };
        Ok(())
    }

    pub async fn set_target_power(&self, requested: u16, rider_max: u16) -> Result<u16, String> {
        let clamped = requested.clamp(
            self.min_power.load(Ordering::Relaxed),
            self.max_power.load(Ordering::Relaxed).min(rider_max),
        );
        self.target_power.store(clamped, Ordering::Relaxed);
        self.write_control(&set_target_power(clamped)).await?;
        Ok(clamped)
    }

    pub async fn pause(&self) -> Result<(), String> {
        self.write_control(&stop_or_pause(true)).await
    }

    pub async fn stop(&self) -> Result<(), String> {
        self.target_power.store(0, Ordering::Relaxed);
        self.write_control(&stop_or_pause(false)).await
    }

    async fn write_control(&self, payload: &[u8]) -> Result<(), String> {
        let _guard = self.command_lock.lock().await;
        let peripheral = self.peripheral.read().await.clone();
        let control = self.control_point.read().await.clone();
        if let (Some(peripheral), Some(control)) = (peripheral, control) {
            peripheral
                .write(&control, payload, WriteType::WithResponse)
                .await
                .map_err(|error| format!("Trainer rejected control command: {error}"))?;
        }
        // Simulated trainers need no GATT write.
        Ok(())
    }

    pub async fn disconnect(&self) {
        let _ = self.stop().await;
        if let Some(worker) = self.worker.lock().await.take() {
            worker.abort();
        }
        if let Some(peripheral) = self.peripheral.write().await.take() {
            let _ = peripheral.disconnect().await;
        }
        *self.control_point.write().await = None;
        *self.state.write().await = DeviceState::Idle;
    }
}

fn platform_guidance() -> String {
    if cfg!(target_os = "linux") {
        "Ensure BlueZ is running and your user can access the system D-Bus Bluetooth service."
            .into()
    } else if cfg!(target_os = "macos") {
        "Allow BlakeBike to use Bluetooth in System Settings → Privacy & Security → Bluetooth."
            .into()
    } else {
        "Check that Bluetooth is enabled and the trainer is not connected to another app.".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn starts_idle() {
        assert!(matches!(
            DeviceManager::default().state().await,
            DeviceState::Idle
        ));
    }

    #[test]
    fn opcodes_match_ftms() {
        assert_eq!(ControlOpcode::SetTargetPower as u8, 0x05);
    }
}
