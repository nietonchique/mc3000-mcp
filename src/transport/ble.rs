//! btleplug/`BlueZ` transport backend for real MC3000 hardware.
//!
//! Preserves the Python transport semantics: official name filtering, FFE0/FFE1
//! discovery, write-without-response on FFE1, and notifications into a bounded
//! queue. Adapter-level failures (powered off, missing adapter) surface as
//! actionable [`TransportError`] values instead of bare runtime errors.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use btleplug::Error as BtleError;
use btleplug::api::{
    BDAddr, Central, CharPropFlags, Characteristic, Manager, Peripheral as _, ScanFilter, WriteType,
};
use btleplug::platform::{Adapter, Manager as PlatformManager, Peripheral};
use futures::StreamExt;
use futures::future::join_all;
use tracing::warn;

use super::{
    NOTIFICATION_QUEUE_CAPACITY, NotificationQueue, ScanDevice, Transport, TransportError,
};

/// How long `connect` keeps scanning for the target address before giving up.
const CONNECT_SCAN_WINDOW: Duration = Duration::from_secs(5);
/// Delay between address polls during the connect discovery window.
const CONNECT_POLL_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug, Default)]
struct BleState {
    peripheral: Option<Peripheral>,
    write_characteristic: Option<Characteristic>,
    notify_characteristic: Option<Characteristic>,
    pump: Option<tokio::task::JoinHandle<()>>,
}

#[derive(Debug)]
struct BleInner {
    state: Mutex<BleState>,
    queue: Arc<NotificationQueue>,
}

/// Real BLE transport over btleplug (`BlueZ` on Linux).
#[derive(Debug)]
pub struct BleTransport {
    manager: PlatformManager,
    inner: Arc<BleInner>,
}

impl BleTransport {
    /// Create the transport by opening the `BlueZ` manager.
    ///
    /// # Errors
    ///
    /// [`TransportError::NoAdapter`] when no adapter exists,
    /// [`TransportError::AdapterPoweredOff`] when `BlueZ` reports the adapter is
    /// not ready, and [`TransportError::Backend`] for other failures.
    pub async fn new() -> Result<Self, TransportError> {
        let manager = PlatformManager::new().await.map_err(classify_btleplug)?;
        Ok(Self {
            manager,
            inner: Arc::new(BleInner {
                state: Mutex::new(BleState::default()),
                queue: NotificationQueue::new(NOTIFICATION_QUEUE_CAPACITY),
            }),
        })
    }

    async fn adapters(&self) -> Result<Vec<Adapter>, TransportError> {
        let adapters = self.manager.adapters().await.map_err(classify_btleplug)?;
        Ok(adapters)
    }
}

/// Map a btleplug error to the most actionable transport error, preserving the
/// backend message otherwise.
pub(crate) fn classify_btleplug(error: BtleError) -> TransportError {
    match error {
        BtleError::NoAdapterAvailable => TransportError::NoAdapter,
        BtleError::NotConnected => TransportError::NotConnected,
        BtleError::DeviceNotFound => TransportError::Backend("BLE device not found".to_owned()),
        other => {
            let message = other.to_string();
            if message.contains("PoweredOff") || message.contains("NotReady") {
                TransportError::AdapterPoweredOff
            } else if message.contains("No such device") || message.contains("does not exist") {
                TransportError::NoAdapter
            } else {
                TransportError::Backend(message)
            }
        }
    }
}

/// Scan one adapter for the scan window and collect charger-like devices.
async fn scan_adapter(
    adapter: &Adapter,
    timeout: Duration,
) -> Result<Vec<ScanDevice>, TransportError> {
    adapter
        .start_scan(ScanFilter::default())
        .await
        .map_err(classify_btleplug)?;
    tokio::time::sleep(timeout).await;
    let _ = adapter.stop_scan().await;
    let mut found = Vec::new();
    for peripheral in adapter.peripherals().await.map_err(classify_btleplug)? {
        let Ok(Some(properties)) = peripheral.properties().await else {
            continue;
        };
        let Some(name) = properties.local_name else {
            continue;
        };
        if !super::name_matches_device(&name) {
            continue;
        }
        found.push(ScanDevice {
            address: properties.address.to_string(),
            name: Some(name),
            rssi: properties.rssi,
        });
    }
    Ok(found)
}

/// Merge multi-adapter scan results into one device per Bluetooth address.
///
/// Both local adapters can observe the same charger; the Python backend naturally returned one
/// device, so preserve first-seen order and only fill in a missing name/RSSI from a later adapter.
fn merge_scan_devices(devices: Vec<ScanDevice>) -> Vec<ScanDevice> {
    let mut merged: Vec<ScanDevice> = Vec::new();
    for device in devices {
        if let Some(existing) = merged
            .iter_mut()
            .find(|candidate| candidate.address == device.address)
        {
            if existing.name.is_none() {
                existing.name = device.name;
            }
            if existing.rssi.is_none() {
                existing.rssi = device.rssi;
            }
        } else {
            merged.push(device);
        }
    }
    merged
}

/// Poll one adapter until `target` appears (or the window closes).
async fn discover_on_adapter(
    adapter: &Adapter,
    target: BDAddr,
    address: &str,
) -> Result<Peripheral, TransportError> {
    adapter
        .start_scan(ScanFilter::default())
        .await
        .map_err(classify_btleplug)?;
    let deadline = tokio::time::Instant::now() + CONNECT_SCAN_WINDOW;
    let mut found = None;
    while found.is_none() {
        for peripheral in adapter.peripherals().await.map_err(classify_btleplug)? {
            if peripheral.address() == target {
                found = Some(peripheral);
                break;
            }
        }
        if found.is_none() {
            let Some(remaining) = deadline.checked_duration_since(tokio::time::Instant::now())
            else {
                break;
            };
            tokio::time::sleep(remaining.min(CONNECT_POLL_INTERVAL)).await;
        }
    }
    let _ = adapter.stop_scan().await;
    found.ok_or(TransportError::DeviceNotFound {
        address: address.to_owned(),
    })
}

#[async_trait]
impl Transport for BleTransport {
    async fn scan(&self, timeout: Duration) -> Result<Vec<ScanDevice>, TransportError> {
        let adapters = self.adapters().await?;
        if adapters.is_empty() {
            return Err(TransportError::NoAdapter);
        }

        // Scan every local adapter concurrently. A two-adapter machine must not consume the
        // entire timeout on the first adapter only; the GP AA charger may advertise through
        // either controller.
        let results = join_all(
            adapters
                .iter()
                .map(|adapter| scan_adapter(adapter, timeout)),
        )
        .await;
        let mut devices = Vec::new();
        let mut failure: Option<TransportError> = None;
        for result in results {
            match result {
                Ok(mut found) => devices.append(&mut found),
                Err(error) => {
                    if failure.is_none() {
                        failure = Some(error);
                    }
                }
            }
        }
        if devices.is_empty()
            && let Some(error) = failure
        {
            return Err(error);
        }
        Ok(merge_scan_devices(devices))
    }

    async fn connect(&self, address: &str) -> Result<(), TransportError> {
        let target = BDAddr::from_str_delim(address)
            .map_err(|_| TransportError::InvalidAddress(address.to_owned()))?;
        // The Python transport drops any previous link before connecting.
        self.disconnect().await?;
        let adapters = self.adapters().await?;
        if adapters.is_empty() {
            return Err(TransportError::NoAdapter);
        }
        let mut peripheral: Option<Peripheral> = None;
        let mut failure: Option<TransportError> = None;
        for adapter in &adapters {
            match discover_on_adapter(adapter, target, address).await {
                Ok(found) => {
                    peripheral = Some(found);
                    break;
                }
                Err(error) => {
                    if failure.is_none() {
                        failure = Some(error);
                    }
                }
            }
        }
        let Some(peripheral) = peripheral else {
            return Err(failure.unwrap_or(TransportError::DeviceNotFound {
                address: address.to_owned(),
            }));
        };
        peripheral.connect().await.map_err(classify_btleplug)?;
        peripheral
            .discover_services()
            .await
            .map_err(classify_btleplug)?;
        let service = peripheral
            .services()
            .into_iter()
            .find(|service| service.uuid == super::SERVICE_UUID)
            .ok_or(TransportError::CharacteristicMissing {
                uuid: super::CHARACTERISTIC_UUID,
            })?;
        let characteristic = service
            .characteristics
            .iter()
            .find(|candidate| candidate.uuid == super::CHARACTERISTIC_UUID)
            .ok_or(TransportError::CharacteristicMissing {
                uuid: super::CHARACTERISTIC_UUID,
            })?;
        if !characteristic
            .properties
            .contains(CharPropFlags::WRITE_WITHOUT_RESPONSE)
        {
            return Err(TransportError::UnsupportedOperation(
                "FFE1 does not support write-without-response".to_owned(),
            ));
        }
        if !characteristic.properties.contains(CharPropFlags::NOTIFY) {
            return Err(TransportError::UnsupportedOperation(
                "FFE1 does not support notifications".to_owned(),
            ));
        }
        peripheral
            .subscribe(characteristic)
            .await
            .map_err(classify_btleplug)?;
        let stream = peripheral
            .notifications()
            .await
            .map_err(classify_btleplug)?;
        // The pre-connect `disconnect` closed the shared queue; every later
        // connect must reopen it, or the pump's pushes are ignored while
        // closed and every request times out (real-BLE reconnect regression).
        self.inner.queue.clear();
        self.inner.queue.reopen();
        let queue = Arc::clone(&self.inner.queue);
        let notify_uuid = characteristic.uuid;
        let pump = tokio::spawn(async move {
            let mut stream = stream;
            while let Some(notification) = stream.next().await {
                if notification.uuid == notify_uuid {
                    queue.push(notification.value);
                }
            }
            // Stream ended: the remote link is gone.
            queue.close();
        });
        let mut state = lock_state(&self.inner.state);
        state.peripheral = Some(peripheral);
        state.write_characteristic = Some(characteristic.clone());
        state.notify_characteristic = Some(characteristic.clone());
        state.pump = Some(pump);
        Ok(())
    }

    async fn disconnect(&self) -> Result<(), TransportError> {
        let (pump, peripheral, notify_characteristic) = {
            let mut state = lock_state(&self.inner.state);
            (
                state.pump.take(),
                state.peripheral.take(),
                state.notify_characteristic.take(),
            )
        };
        if let Some(handle) = pump {
            handle.abort();
        }
        if let Some(peripheral) = peripheral
            && peripheral.is_connected().await.unwrap_or(false)
        {
            if let Some(characteristic) = notify_characteristic {
                // Unsubscribe failures are suppressed, exactly like the
                // Python transport's stop_notify handling.
                if let Err(error) = peripheral.unsubscribe(&characteristic).await {
                    warn!(%error, "FFE1 unsubscribe failed; suppressed");
                }
            }
            if let Err(error) = peripheral.disconnect().await {
                warn!(%error, "peripheral disconnect failed; suppressed");
            }
        }
        self.inner.queue.clear();
        self.inner.queue.close();
        Ok(())
    }

    async fn is_connected(&self) -> bool {
        let peripheral = lock_state(&self.inner.state).peripheral.clone();
        match peripheral {
            Some(peripheral) => peripheral.is_connected().await.unwrap_or(false),
            None => false,
        }
    }

    async fn write(&self, data: &[u8]) -> Result<(), TransportError> {
        let link = {
            let state = lock_state(&self.inner.state);
            state
                .peripheral
                .clone()
                .zip(state.write_characteristic.clone())
        };
        let Some((peripheral, characteristic)) = link else {
            return Err(TransportError::NotConnected);
        };
        if !peripheral.is_connected().await.unwrap_or(false) {
            return Err(TransportError::NotConnected);
        }
        peripheral
            .write(&characteristic, data, WriteType::WithoutResponse)
            .await
            .map_err(classify_btleplug)
    }

    async fn next_notification(&self) -> Option<Vec<u8>> {
        self.inner.queue.pop().await
    }

    fn clear_pending(&self) {
        self.inner.queue.clear();
    }

    fn notification_dropped_count(&self) -> u64 {
        self.inner.queue.dropped()
    }
}

fn lock_state(state: &Mutex<BleState>) -> std::sync::MutexGuard<'_, BleState> {
    match state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_scan_devices_keeps_one_entry_per_address() {
        let merged = merge_scan_devices(vec![
            ScanDevice {
                address: "F8:2E:0C:B8:A6:15".to_owned(),
                name: Some("Charger".to_owned()),
                rssi: None,
            },
            ScanDevice {
                address: "F8:2E:0C:B8:A6:15".to_owned(),
                name: None,
                rssi: Some(-76),
            },
            ScanDevice {
                address: "AA:BB:CC:DD:EE:FF".to_owned(),
                name: Some("SimpleBLEPeripheral".to_owned()),
                rssi: Some(-90),
            },
        ]);

        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].name.as_deref(), Some("Charger"));
        assert_eq!(merged[0].rssi, Some(-76));
        assert_eq!(merged[1].address, "AA:BB:CC:DD:EE:FF");
    }

    #[test]
    fn merge_scan_devices_preserves_distinct_addresses() {
        let merged = merge_scan_devices(vec![
            ScanDevice {
                address: "AA:BB:CC:DD:EE:01".to_owned(),
                name: Some("Charger".to_owned()),
                rssi: Some(-70),
            },
            ScanDevice {
                address: "AA:BB:CC:DD:EE:02".to_owned(),
                name: Some("HitecCharger".to_owned()),
                rssi: Some(-71),
            },
        ]);

        assert_eq!(merged.len(), 2);
        assert_eq!(merged[1].name.as_deref(), Some("HitecCharger"));
    }
}
