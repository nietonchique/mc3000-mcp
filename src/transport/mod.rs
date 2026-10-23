//! Async charger transport interface shared by the real `BlueZ` backend and the
//! deterministic fake backend.
//!
//! A transport owns exactly one BLE link: connect, disconnect, write, and a
//! bounded queue of raw notification bytes. Higher-level request/response
//! matching, voltage-curve reassembly, and profile sequencing live in
//! [`crate::client`].

pub mod ble;
pub mod fake;

pub use self::ble::BleTransport;
pub use self::fake::FakeTransport;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use btleplug::api::bleuuid::uuid_from_u16;
use serde::Serialize;
use tokio::sync::Notify;

use crate::protocol::DEVICE_NAMES;

/// Capacity of the bounded per-connection notification queue. When the consumer
/// falls behind, the oldest notifications are dropped and counted instead of
/// growing without bound (the Python queue was unbounded).
pub const NOTIFICATION_QUEUE_CAPACITY: usize = 128;

/// GATT service UUID of MC3000-compatible chargers (`protocol.SERVICE_UUID`).
pub const SERVICE_UUID: uuid::Uuid = uuid_from_u16(0xFFE0);

/// GATT characteristic UUID used for writes and notifications
/// (`protocol.CHAR_UUID`).
pub const CHARACTERISTIC_UUID: uuid::Uuid = uuid_from_u16(0xFFE1);

/// A charger-like device found during a scan (`transport.FoundDevice`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScanDevice {
    /// Bluetooth address (`AA:BB:CC:DD:EE:FF`).
    pub address: String,
    /// Advertised local name, when the device advertises one.
    pub name: Option<String>,
    /// Signal strength in dBm, when advertised.
    pub rssi: Option<i16>,
}

/// Transport-level failures with actionable messages. Python raised bare
/// `RuntimeError`/`TimeoutError`; every case is typed here.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TransportError {
    /// The `BlueZ` adapter exists but is powered off or not ready.
    #[error(
        "Bluetooth adapter is powered off or not ready; run `rfkill unblock bluetooth` and \
         `bluetoothctl power on`, then retry"
    )]
    AdapterPoweredOff,
    /// No BLE adapter was found or bluetoothd is unavailable.
    #[error(
        "no Bluetooth adapter available; connect a BLE adapter and ensure bluetoothd is running"
    )]
    NoAdapter,
    /// A scan or connect window elapsed without finding the device.
    #[error("device '{address}' was not found within the scan window")]
    DeviceNotFound {
        /// Address that was searched for.
        address: String,
    },
    /// No charger-like device was found by the official names.
    #[error("no MC3000-like BLE device found by names {names:?}")]
    NoDeviceFound {
        /// The official device names used as scan filters.
        names: Vec<String>,
    },
    /// An operation requiring a live link was attempted while disconnected.
    #[error("MC3000 is not connected")]
    NotConnected,
    /// The connected device does not expose the FFE0/FFE1 GATT layout.
    #[error(
        "GATT characteristic {uuid} not found on the connected device; the device may not be \
         an MC3000-compatible charger"
    )]
    CharacteristicMissing {
        /// The missing characteristic UUID.
        uuid: uuid::Uuid,
    },
    /// The characteristic exists but lacks write-without-response or notify.
    #[error("characteristic does not support the required operation: {0}")]
    UnsupportedOperation(String),
    /// The backend rejected an address literal.
    #[error("invalid Bluetooth address '{0}'")]
    InvalidAddress(String),
    /// A reply wait exceeded its deadline.
    #[error("timeout waiting for opcode {0}")]
    ReplyTimeout(ReplyTarget),
    /// A timeout argument was not finite or was negative.
    #[error("invalid timeout {value}: timeout must be finite and non-negative")]
    InvalidTimeout {
        /// The rejected timeout value in seconds.
        value: f64,
    },
    /// Any other backend failure, with its message preserved.
    #[error("backend error: {0}")]
    Backend(String),
}

/// Opcode target of a timed-out wait, rendered into [`TransportError::ReplyTimeout`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplyTarget(pub Option<u8>);

impl std::fmt::Display for ReplyTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(opcode) => write!(formatter, "{opcode:#04x}"),
            None => write!(formatter, "any"),
        }
    }
}

/// Convert a Python-style timeout in seconds into a `Duration`, rejecting
/// negative, NaN, and infinite values with a typed error.
///
/// # Errors
///
/// [`TransportError::InvalidTimeout`] when `value` is negative, NaN, or infinite.
pub fn timeout_secs(value: f64) -> Result<Duration, TransportError> {
    if !value.is_finite() || value < 0.0 || value > Duration::MAX.as_secs_f64() {
        return Err(TransportError::InvalidTimeout { value });
    }
    Ok(Duration::from_secs_f64(value))
}

/// Whether an advertised name matches the official charger device names
/// (`DEVICE_NAMES`): exact membership or substring containment, byte-for-byte
/// the Python scan filter.
#[must_use]
pub fn name_matches_device(name: &str) -> bool {
    DEVICE_NAMES
        .iter()
        .any(|official| name == *official || name.contains(official))
}

/// Bounded, closable queue of raw notification bytes shared between the backend
/// notification pump and the client consumer.
///
/// When full, the oldest notification is dropped and counted; push after close
/// is ignored. `pop` returns `None` once the queue is closed and drained.
#[derive(Debug)]
pub(crate) struct NotificationQueue {
    capacity: usize,
    state: Mutex<QueueState>,
    available: Notify,
}

#[derive(Debug)]
struct QueueState {
    items: VecDeque<Vec<u8>>,
    closed: bool,
    dropped: u64,
}

impl NotificationQueue {
    /// Create a shared queue with the given positive capacity.
    pub(crate) fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            capacity: capacity.max(1),
            state: Mutex::new(QueueState {
                items: VecDeque::new(),
                closed: false,
                dropped: 0,
            }),
            available: Notify::new(),
        })
    }

    /// Enqueue raw notification bytes, dropping the oldest entry when full.
    /// Ignored while the queue is closed.
    pub(crate) fn push(&self, data: Vec<u8>) {
        let mut state = Self::lock(&self.state);
        if state.closed {
            return;
        }
        if state.items.len() >= self.capacity {
            state.items.pop_front();
            state.dropped += 1;
        }
        state.items.push_back(data);
        drop(state);
        self.available.notify_one();
    }

    /// Close the queue: pending items stay readable, `pop` yields `None` once
    /// drained, later pushes are ignored.
    pub(crate) fn close(&self) {
        {
            let mut state = Self::lock(&self.state);
            state.closed = true;
        }
        self.available.notify_one();
    }

    /// Reopen a closed queue so a fresh link can use it.
    pub(crate) fn reopen(&self) {
        let mut state = Self::lock(&self.state);
        state.closed = false;
    }

    /// Drop every queued notification (pending-state cleanup).
    pub(crate) fn clear(&self) {
        Self::lock(&self.state).items.clear();
    }

    /// Total notifications dropped for being over capacity.
    pub(crate) fn dropped(&self) -> u64 {
        Self::lock(&self.state).dropped
    }

    /// Await the next notification; `None` once the queue is closed and drained.
    pub(crate) async fn pop(&self) -> Option<Vec<u8>> {
        loop {
            {
                let mut state = Self::lock(&self.state);
                if let Some(item) = state.items.pop_front() {
                    return Some(item);
                }
                if state.closed {
                    return None;
                }
            }
            self.available.notified().await;
        }
    }

    fn lock(state: &Mutex<QueueState>) -> MutexGuard<'_, QueueState> {
        match state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// One live charger link: scan, connect, write, notifications.
///
/// Implementations are cheap to clone when `Arc`-backed and must be usable from
/// multiple tasks; all state is internal.
#[async_trait]
pub trait Transport: Send + Sync {
    /// Scan for charger-like devices for `timeout` and return the matches.
    ///
    /// # Errors
    ///
    /// [`TransportError::NoAdapter`] when no adapter exists,
    /// [`TransportError::AdapterPoweredOff`] when the adapter is not ready, and
    /// [`TransportError::Backend`] for other backend failures.
    async fn scan(&self, timeout: Duration) -> Result<Vec<ScanDevice>, TransportError>;

    /// Connect to `address` and subscribe to FFE1 notifications. Any previous
    /// link is dropped first.
    ///
    /// # Errors
    ///
    /// [`TransportError::DeviceNotFound`] when the address is not discovered,
    /// [`TransportError::AdapterPoweredOff`], [`TransportError::NoAdapter`],
    /// [`TransportError::CharacteristicMissing`],
    /// [`TransportError::UnsupportedOperation`], and
    /// [`TransportError::Backend`] for connection failures.
    async fn connect(&self, address: &str) -> Result<(), TransportError>;

    /// Drop the link best-effort (unsubscribe failures are suppressed like the
    /// Python transport) and clear pending notifications.
    ///
    /// # Errors
    ///
    /// Implementations currently suppress link teardown errors and return
    /// `Ok`; the `Result` is reserved for backends that can report teardown
    /// failures.
    async fn disconnect(&self) -> Result<(), TransportError>;

    /// Whether a live link exists right now.
    async fn is_connected(&self) -> bool;

    /// Write bytes to FFE1 without response.
    ///
    /// # Errors
    ///
    /// [`TransportError::NotConnected`] without a live link and
    /// [`TransportError::Backend`] when the write is rejected.
    async fn write(&self, data: &[u8]) -> Result<(), TransportError>;

    /// Await the next raw notification; `None` once the link is gone and the
    /// queue is drained.
    async fn next_notification(&self) -> Option<Vec<u8>>;

    /// Drop every queued notification (pending-state cleanup).
    fn clear_pending(&self);

    /// Total notifications dropped for being over capacity.
    fn notification_dropped_count(&self) -> u64;
}

#[cfg(test)]
mod tests {
    use super::NotificationQueue;

    /// Regression for the real-BLE reconnect bug: `disconnect` closes the
    /// shared queue, so a backend whose `connect` forgets `reopen` leaves the
    /// pump pushing into a closed queue — every push is silently ignored and
    /// all requests time out. `FakeTransport::connect` reopens internally, so
    /// this transition can only be pinned by a direct queue-level test.
    #[tokio::test]
    async fn reopen_restores_delivery_after_close() {
        let queue = NotificationQueue::new(4);
        queue.push(b"first".to_vec());
        queue.close();
        // A closed queue still drains what arrived before the close.
        assert_eq!(
            queue.pop().await.as_deref(),
            Some(b"first".as_slice()),
            "pre-close notification must remain readable"
        );
        assert_eq!(
            queue.pop().await,
            None,
            "drained closed queue ends the stream"
        );
        // The bug shape: pushes while closed are ignored.
        queue.push(b"ignored".to_vec());
        // What every backend's connect must do (mirrors BleTransport::connect).
        queue.clear();
        queue.reopen();
        queue.push(b"delivered".to_vec());
        assert_eq!(
            queue.pop().await.as_deref(),
            Some(b"delivered".as_slice()),
            "after reopen the pump's pushes must be delivered; the pre-reopen \
             push must have been dropped"
        );
    }
}
