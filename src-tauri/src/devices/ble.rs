//! Bluetooth plumbing shared by every device role: adapter lifetime, scanning,
//! timeouts, UUID helpers and the standard Battery / Device Information reads.

use std::{collections::HashMap, time::Duration};

use btleplug::{
    api::{Central, Characteristic, Manager as _, Peripheral as _, ScanFilter},
    platform::{Adapter, Manager, Peripheral},
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::ftms::FITNESS_MACHINE_SERVICE;

pub const HEART_RATE_SERVICE: u16 = 0x180D;
pub const CYCLING_POWER_SERVICE: u16 = 0x1818;
pub const CYCLING_SPEED_CADENCE_SERVICE: u16 = 0x1816;
pub const BATTERY_LEVEL: u16 = 0x2A19;
pub const MANUFACTURER_NAME: u16 = 0x2A29;
pub const MODEL_NUMBER: u16 = 0x2A24;
pub const FIRMWARE_REVISION: u16 = 0x2A26;

pub const GATT_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
pub const GATT_STEP_TIMEOUT: Duration = Duration::from_secs(10);
const SCAN_WINDOW: Duration = Duration::from_secs(3);
/// How long a targeted scan waits for one specific device to show up.
pub const FIND_TIMEOUT: Duration = Duration::from_secs(10);

/// A GATT service a discovered device advertises that we know how to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Capability {
    Ftms,
    HeartRate,
    CyclingPower,
    Csc,
}

impl Capability {
    pub const ALL: [Capability; 4] = [
        Capability::Ftms,
        Capability::HeartRate,
        Capability::CyclingPower,
        Capability::Csc,
    ];

    pub fn service(self) -> u16 {
        match self {
            Capability::Ftms => FITNESS_MACHINE_SERVICE,
            Capability::HeartRate => HEART_RATE_SERVICE,
            Capability::CyclingPower => CYCLING_POWER_SERVICE,
            Capability::Csc => CYCLING_SPEED_CADENCE_SERVICE,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub simulated: bool,
    pub rssi: Option<i16>,
    #[serde(default)]
    pub capabilities: Vec<Capability>,
}

impl DeviceInfo {
    pub fn supports(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }
}

/// Standard, optional, read-once characteristics most sensors expose.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeviceDetails {
    pub battery_percent: Option<u8>,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub firmware: Option<String>,
}

impl DeviceDetails {
    pub fn summary(&self) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(manufacturer) = &self.manufacturer {
            parts.push(manufacturer.clone());
        }
        if let Some(model) = &self.model {
            parts.push(model.clone());
        }
        if let Some(firmware) = &self.firmware {
            parts.push(format!("fw {firmware}"));
        }
        if let Some(battery) = self.battery_percent {
            parts.push(format!("battery {battery}%"));
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    }
}

/// Owns the adapter and the peripherals seen in the last scan.
pub struct Ble {
    adapter: Mutex<Option<Adapter>>,
    discovered: Mutex<HashMap<String, Peripheral>>,
    /// When false, `adapter()` fails without ever touching the OS Bluetooth
    /// stack. Headless hubs (unit tests) use this: CoreBluetooth aborts an
    /// unsigned test binary on macOS, and BlueZ may be absent on CI.
    enabled: bool,
}

impl Default for Ble {
    fn default() -> Self {
        Self::new(true)
    }
}

impl Ble {
    pub fn new(enabled: bool) -> Self {
        Self {
            adapter: Mutex::new(None),
            discovered: Mutex::new(HashMap::new()),
            enabled,
        }
    }

    /// A Bluetooth layer with no adapter; only simulated devices work.
    pub fn disabled() -> Self {
        Self::new(false)
    }

    pub async fn adapter(&self) -> Result<Adapter, String> {
        if !self.enabled {
            return Err("Bluetooth is disabled in this environment".into());
        }
        if let Some(adapter) = self.adapter.lock().await.clone() {
            return Ok(adapter);
        }
        tracing::debug!("Initializing Bluetooth manager");
        let manager = Manager::new()
            .await
            .map_err(|error| format!("Bluetooth is unavailable: {error}"))?;
        let adapters = manager
            .adapters()
            .await
            .map_err(|error| format!("Could not enumerate Bluetooth adapters: {error}"))?;
        tracing::debug!(count = adapters.len(), "Bluetooth adapters enumerated");
        let adapter = adapters
            .into_iter()
            .next()
            .ok_or_else(|| "No Bluetooth adapter was found".to_string())?;
        let adapter_info = adapter.adapter_info().await.ok();
        tracing::info!(info = ?adapter_info, "Using Bluetooth adapter");
        *self.adapter.lock().await = Some(adapter.clone());
        Ok(adapter)
    }

    /// Forget the adapter and everything seen through it. Used when btleplug's
    /// event loop has died ("Channel closed"): the cached handles are useless
    /// until recreated by the next scan.
    pub async fn reset(&self) {
        tracing::warn!("Discarding cached Bluetooth adapter; the next scan recreates it");
        *self.adapter.lock().await = None;
        self.discovered.lock().await.clear();
    }

    pub async fn peripheral(&self, id: &str) -> Option<Peripheral> {
        self.discovered.lock().await.get(id).cloned()
    }

    /// Scan until the peripheral with `id` is seen (or `FIND_TIMEOUT` passes),
    /// remembering it so a following connect can use it. Returns the device as
    /// currently advertised.
    pub async fn find(&self, id: &str) -> Result<DeviceInfo, String> {
        let adapter = self.adapter().await?;
        adapter
            .start_scan(ScanFilter::default())
            .await
            .map_err(|error| format!("Could not start Bluetooth scan: {error}"))?;
        let deadline = tokio::time::Instant::now() + FIND_TIMEOUT;
        let mut found = None;
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(400)).await;
            let peripherals = adapter
                .peripherals()
                .await
                .map_err(|error| format!("Could not read Bluetooth devices: {error}"))?;
            if let Some(peripheral) = peripherals
                .into_iter()
                .find(|peripheral| peripheral.id().to_string() == id)
            {
                found = Some(peripheral);
                break;
            }
        }
        if let Err(error) = adapter.stop_scan().await {
            tracing::debug!(error = %error, "Could not stop scan (ignored)");
        }
        let peripheral = found.ok_or_else(|| {
            format!(
                "Device was not seen within {}s; make sure it is awake and nearby",
                FIND_TIMEOUT.as_secs()
            )
        })?;
        let properties = peripheral
            .properties()
            .await
            .map_err(|error| format!("Could not read device properties: {error}"))?
            .ok_or_else(|| "Device has not advertised its properties yet".to_string())?;
        let capabilities: Vec<Capability> = Capability::ALL
            .into_iter()
            .filter(|capability| {
                properties
                    .services
                    .iter()
                    .any(|uuid| *uuid == bluetooth_uuid(capability.service()))
            })
            .collect();
        self.discovered
            .lock()
            .await
            .insert(id.to_string(), peripheral);
        Ok(DeviceInfo {
            id: id.to_string(),
            name: properties
                .local_name
                .unwrap_or_else(|| default_name(&capabilities).into()),
            simulated: false,
            rssi: properties.rssi,
            capabilities,
        })
    }

    /// One scan window; returns every peripheral advertising at least one
    /// capability we understand. Returns an empty list (not an error) when the
    /// machine has no Bluetooth adapter so the simulator can still be offered.
    pub async fn scan(&self) -> Result<Vec<DeviceInfo>, String> {
        let adapter = match self.adapter().await {
            Ok(adapter) => adapter,
            Err(error) => {
                tracing::warn!(error = %error, "No Bluetooth adapter; offering simulators only");
                return Ok(Vec::new());
            }
        };
        // Linux/BlueZ merges filters from all clients, so filter results ourselves below.
        adapter
            .start_scan(ScanFilter::default())
            .await
            .map_err(|error| format!("Could not start Bluetooth scan: {error}"))?;
        tokio::time::sleep(SCAN_WINDOW).await;
        let peripherals = adapter
            .peripherals()
            .await
            .map_err(|error| format!("Could not read Bluetooth devices: {error}"))?;
        if let Err(error) = adapter.stop_scan().await {
            tracing::debug!(error = %error, "Could not stop scan (ignored)");
        }
        tracing::debug!(
            peripherals = peripherals.len(),
            "Bluetooth scan window closed"
        );
        let mut devices = Vec::new();
        let mut discovered = self.discovered.lock().await;
        discovered.clear();
        for peripheral in peripherals {
            let properties = match peripheral.properties().await {
                Ok(Some(properties)) => properties,
                Ok(None) => {
                    tracing::trace!(id = %peripheral.id(), "Peripheral has no properties yet");
                    continue;
                }
                Err(error) => {
                    tracing::debug!(id = %peripheral.id(), error = %error, "Could not read peripheral properties");
                    continue;
                }
            };
            let capabilities: Vec<Capability> = Capability::ALL
                .into_iter()
                .filter(|capability| {
                    properties
                        .services
                        .iter()
                        .any(|uuid| *uuid == bluetooth_uuid(capability.service()))
                })
                .collect();
            tracing::trace!(
                id = %peripheral.id(),
                name = ?properties.local_name,
                rssi = ?properties.rssi,
                services = ?properties.services,
                ?capabilities,
                "Saw peripheral"
            );
            if capabilities.is_empty() {
                continue;
            }
            let id = peripheral.id().to_string();
            tracing::info!(
                id = %id,
                name = ?properties.local_name,
                rssi = ?properties.rssi,
                ?capabilities,
                "Found sensor"
            );
            devices.push(DeviceInfo {
                id: id.clone(),
                name: properties
                    .local_name
                    .unwrap_or_else(|| default_name(&capabilities).into()),
                simulated: false,
                rssi: properties.rssi,
                capabilities,
            });
            discovered.insert(id, peripheral);
        }
        Ok(devices)
    }
}

fn default_name(capabilities: &[Capability]) -> &'static str {
    match capabilities.first() {
        Some(Capability::Ftms) => "FTMS trainer",
        Some(Capability::HeartRate) => "Heart rate monitor",
        Some(Capability::CyclingPower) => "Power meter",
        Some(Capability::Csc) => "Cadence sensor",
        None => "Bluetooth sensor",
    }
}

/// Read Battery Level and Device Information if the peripheral exposes them.
/// Never fails: every read is optional and bounded by a timeout.
pub async fn read_details(peripheral: &Peripheral) -> DeviceDetails {
    let characteristics = peripheral.characteristics();
    let find = |short: u16| {
        characteristics
            .iter()
            .find(|characteristic| characteristic.uuid == bluetooth_uuid(short))
            .cloned()
    };
    let read_string = |characteristic: Option<Characteristic>| async {
        let characteristic = characteristic?;
        match with_timeout(
            GATT_STEP_TIMEOUT,
            "device info read",
            peripheral.read(&characteristic),
        )
        .await
        {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes)
                    .trim_end_matches('\0')
                    .trim()
                    .to_string();
                (!text.is_empty()).then_some(text)
            }
            Err(error) => {
                tracing::debug!(uuid = %characteristic.uuid, error = %error, "Optional read failed");
                None
            }
        }
    };
    let battery_percent = match find(BATTERY_LEVEL) {
        Some(characteristic) => {
            match with_timeout(
                GATT_STEP_TIMEOUT,
                "battery read",
                peripheral.read(&characteristic),
            )
            .await
            {
                Ok(bytes) => bytes.first().copied().filter(|level| *level <= 100),
                Err(error) => {
                    tracing::debug!(error = %error, "Battery read failed");
                    None
                }
            }
        }
        None => None,
    };
    DeviceDetails {
        battery_percent,
        manufacturer: read_string(find(MANUFACTURER_NAME)).await,
        model: read_string(find(MODEL_NUMBER)).await,
        firmware: read_string(find(FIRMWARE_REVISION)).await,
    }
}

pub fn platform_guidance() -> String {
    if cfg!(target_os = "linux") {
        "Ensure BlueZ is running and your user can access the system D-Bus Bluetooth service."
            .into()
    } else if cfg!(target_os = "macos") {
        "Allow BlakeBike to use Bluetooth in System Settings → Privacy & Security → Bluetooth."
            .into()
    } else {
        "Check that Bluetooth is enabled and the device is not connected to another app.".into()
    }
}

/// Bound a BLE operation so a silent stack never leaves the UI spinning forever.
pub async fn with_timeout<T>(
    limit: Duration,
    what: &str,
    operation: impl std::future::Future<Output = btleplug::Result<T>>,
) -> Result<T, String> {
    match tokio::time::timeout(limit, operation).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => {
            tracing::error!(
                step = what,
                timeout_secs = limit.as_secs(),
                "BLE operation timed out"
            );
            Err(format!("{what} timed out after {}s", limit.as_secs()))
        }
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn bluetooth_uuid(short: u16) -> Uuid {
    Uuid::from_u128((u128::from(short) << 96) | 0x0000_0000_0000_1000_8000_0080_5f9b_34fb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_uuid_expands_to_base() {
        assert_eq!(
            bluetooth_uuid(0x1826).to_string(),
            "00001826-0000-1000-8000-00805f9b34fb"
        );
    }

    #[test]
    fn hex_formats_bytes() {
        assert_eq!(hex(&[0x00, 0xab, 0x10]), "00 ab 10");
        assert_eq!(hex(&[]), "");
    }

    #[test]
    fn details_summary() {
        assert_eq!(DeviceDetails::default().summary(), None);
        let details = DeviceDetails {
            battery_percent: Some(80),
            manufacturer: Some("Wahoo".into()),
            model: None,
            firmware: Some("4.2".into()),
        };
        assert_eq!(
            details.summary().as_deref(),
            Some("Wahoo · fw 4.2 · battery 80%")
        );
    }

    #[test]
    fn device_info_deserializes_without_capabilities() {
        let info: DeviceInfo =
            serde_json::from_str(r#"{"id":"x","name":"Trainer","simulated":false,"rssi":-60}"#)
                .unwrap();
        assert!(info.capabilities.is_empty());
        assert!(!info.supports(Capability::Ftms));
    }
}
