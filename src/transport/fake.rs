//! Deterministic, scriptable fake transport for tests and `--backend fake`
//! development without hardware.
//!
//! The fake records every write with its (tokio-clock) timestamp, injects
//! scripted notifications into the same bounded queue the real backend feeds,
//! and can simulate remote link loss, failed unsubscribes, and failed
//! scan/connect attempts.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use super::{
    NOTIFICATION_QUEUE_CAPACITY, NotificationQueue, ScanDevice, Transport, TransportError,
};

/// One recorded write: the bytes and the tokio-clock instant they were sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeWrite {
    /// Written bytes.
    pub data: Vec<u8>,
    /// Tokio-clock instant of the write (respects paused test time).
    pub at: tokio::time::Instant,
}

#[derive(Debug, Default)]
struct FakeState {
    connected: bool,
    address: Option<String>,
    subscribed: bool,
    stop_notify_fails: bool,
    writes: Vec<FakeWrite>,
}

#[derive(Debug)]
struct FakeInner {
    queue: Arc<NotificationQueue>,
    state: Mutex<FakeState>,
    scan_devices: Mutex<Vec<ScanDevice>>,
    scan_failure: Mutex<Option<TransportError>>,
    connect_failure: Mutex<Option<TransportError>>,
}

/// Scriptable fake [`Transport`]; clone freely, all clones share state.
#[derive(Debug, Clone)]
pub struct FakeTransport {
    inner: Arc<FakeInner>,
}

impl FakeTransport {
    /// Create an empty fake with no scan results and no failures.
    #[must_use]
    pub fn new() -> Self {
        Self::with_scan_devices(Vec::new())
    }

    /// Create a fake whose `scan` returns the given devices.
    #[must_use]
    pub fn with_scan_devices(devices: Vec<ScanDevice>) -> Self {
        Self {
            inner: Arc::new(FakeInner {
                queue: NotificationQueue::new(NOTIFICATION_QUEUE_CAPACITY),
                state: Mutex::new(FakeState::default()),
                scan_devices: Mutex::new(devices),
                scan_failure: Mutex::new(None),
                connect_failure: Mutex::new(None),
            }),
        }
    }

    /// Replace the scan results.
    pub fn set_scan_devices(&self, devices: Vec<ScanDevice>) {
        *Self::lock(&self.inner.scan_devices) = devices;
    }

    /// Script a `scan` failure; `None` clears the script.
    pub fn set_scan_failure(&self, failure: Option<TransportError>) {
        *Self::lock(&self.inner.scan_failure) = failure;
    }

    /// Script a `connect` failure; `None` clears the script.
    pub fn set_connect_failure(&self, failure: Option<TransportError>) {
        *Self::lock(&self.inner.connect_failure) = failure;
    }

    /// Make `stop_notify` fail during `disconnect` (suppressed, like the Python
    /// `StopNotifyFailClient` fixture).
    pub fn set_stop_notify_failure(&self, fails: bool) {
        Self::lock(&self.inner.state).stop_notify_fails = fails;
    }

    /// Inject raw notification bytes into the bounded queue, exactly like a
    /// device notification arriving over the air. Drops the oldest entry when
    /// the queue is full.
    pub fn inject(&self, data: &[u8]) {
        self.inner.queue.push(data.to_vec());
    }

    /// Simulate the remote end disappearing without a clean client-initiated
    /// disconnect: the link goes down and the notification stream ends after
    /// already-buffered notifications were delivered.
    pub fn drop_link(&self) {
        let mut state = Self::lock(&self.inner.state);
        state.connected = false;
        drop(state);
        self.inner.queue.close();
    }

    /// Recorded writes in send order.
    #[must_use]
    pub fn writes(&self) -> Vec<Vec<u8>> {
        Self::lock(&self.inner.state)
            .writes
            .iter()
            .map(|write| write.data.clone())
            .collect()
    }

    /// Tokio-clock instants of the recorded writes, in send order.
    #[must_use]
    pub fn write_instants(&self) -> Vec<tokio::time::Instant> {
        Self::lock(&self.inner.state)
            .writes
            .iter()
            .map(|write| write.at)
            .collect()
    }

    /// Forget the recorded writes (timestamps reset with them).
    pub fn clear_writes(&self) {
        Self::lock(&self.inner.state).writes.clear();
    }

    /// Whether FFE1 notifications are subscribed on the current link.
    #[must_use]
    pub fn subscribed(&self) -> bool {
        Self::lock(&self.inner.state).subscribed
    }

    /// Address of the current link, if any.
    #[must_use]
    pub fn current_address(&self) -> Option<String> {
        Self::lock(&self.inner.state).address.clone()
    }

    fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl Default for FakeTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Transport for FakeTransport {
    async fn scan(&self, timeout: Duration) -> Result<Vec<ScanDevice>, TransportError> {
        let _ = timeout;
        if let Some(failure) = Self::lock(&self.inner.scan_failure).clone() {
            return Err(failure);
        }
        Ok(Self::lock(&self.inner.scan_devices).clone())
    }

    async fn connect(&self, address: &str) -> Result<(), TransportError> {
        if let Some(failure) = Self::lock(&self.inner.connect_failure).clone() {
            return Err(failure);
        }
        let mut state = Self::lock(&self.inner.state);
        state.connected = true;
        state.address = Some(address.to_owned());
        state.subscribed = true;
        drop(state);
        self.inner.queue.reopen();
        self.inner.queue.clear();
        Ok(())
    }

    async fn disconnect(&self) -> Result<(), TransportError> {
        let mut state = Self::lock(&self.inner.state);
        // A failing unsubscribe is suppressed, mirroring the Python transport;
        // the subscription marker survives to reflect the failed stop_notify.
        state.subscribed = state.stop_notify_fails;
        state.connected = false;
        state.address = None;
        drop(state);
        self.inner.queue.clear();
        self.inner.queue.close();
        Ok(())
    }

    async fn is_connected(&self) -> bool {
        Self::lock(&self.inner.state).connected
    }

    async fn write(&self, data: &[u8]) -> Result<(), TransportError> {
        let mut state = Self::lock(&self.inner.state);
        if !state.connected {
            return Err(TransportError::NotConnected);
        }
        state.writes.push(FakeWrite {
            data: data.to_vec(),
            at: tokio::time::Instant::now(),
        });
        Ok(())
    }

    async fn next_notification(&self) -> Option<Vec<u8>> {
        // Mirrors the real pump: buffered notifications are delivered until the
        // stream closes, then `None`.
        self.inner.queue.pop().await
    }

    fn clear_pending(&self) {
        self.inner.queue.clear();
    }

    fn notification_dropped_count(&self) -> u64 {
        self.inner.queue.dropped()
    }
}
