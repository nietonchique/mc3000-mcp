//! Serialized charger operations and connection/model state.
//!
//! [`ChargerClient`] preserves the Python `MC3000Client` semantics on top of a
//! [`Transport`]: opcode-matched request waits, 245-byte voltage-curve
//! reassembly, sequential slot polling with a 50 ms gap, and the exact APK
//! profile write order (stop each masked slot, 500 ms, chunk 1, 50 ms, chunk 2,
//! optional 500 ms then starts). Robustness improvements over the Python
//! client: bounded notification routing, disconnect/connect clears pending
//! state, a request lock that keeps concurrent operations from stealing each
//! other's replies, and an explicit per-connection [`ChargerModel`] with
//! read-only enforcement for models that do not allow writes.

use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use serde::Serialize;

use crate::ProtocolError;
use crate::model::{ChargerModel, Slot};
use crate::protocol::{
    self, BasicSettings, Notification, Opcode, command_get_basic, command_get_mc5000_status,
    command_get_status, command_get_voltage_curve, command_set_basic, command_start, command_stop,
    parse_notification, split_profile,
};
use crate::transport::{ReplyTarget, ScanDevice, Transport, TransportError};

/// Default reply wait for status/version/basic requests (Python `timeout=3.0`).
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
/// Default reply wait for a full voltage curve (Python server `timeout=8.0`).
pub const CURVE_TIMEOUT: Duration = Duration::from_secs(8);
/// Default scan window (Python `timeout=5.0`).
pub const SCAN_TIMEOUT: Duration = Duration::from_secs(5);
/// Gap between sequential slot polls (Python `asyncio.sleep(0.05)`).
pub const SLOT_POLL_GAP: Duration = Duration::from_millis(50);
/// Delay after each profile stop write (Python `asyncio.sleep(0.5)`).
pub const PROFILE_STOP_DELAY: Duration = Duration::from_millis(500);
/// Delay between the two profile chunk writes (Python `asyncio.sleep(0.05)`).
pub const PROFILE_CHUNK_GAP: Duration = Duration::from_millis(50);
/// Delay between the profile chunks and the start writes
/// (Python `asyncio.sleep(0.5)`).
pub const PROFILE_START_DELAY: Duration = Duration::from_millis(500);

/// Result of a successful connect. (Not `Serialize`: the model type lives in
/// [`crate::model`] without serde derives; the MCP layer renders its own JSON.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectInfo {
    /// Always `true` on success.
    pub connected: bool,
    /// Connected address.
    pub address: String,
    /// Model explicitly selected for this connection.
    pub model: ChargerModel,
}

/// Result of [`ChargerClient::send_profile`] (`MC3000Client.send_profile`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SendProfileReport {
    /// Always `true` when the sequence completed.
    pub ok: bool,
    /// Zero-based slots that were stopped before the write.
    pub stopped_slots: Vec<u8>,
    /// Whether start writes were issued after the profile.
    pub started: bool,
    /// Upper-case hex of the full 40-byte profile.
    pub profile_hex: String,
}

/// Client-level failure: transport and protocol errors plus model enforcement.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ChargerError {
    /// Transport-level failure.
    #[error(transparent)]
    Transport(#[from] TransportError),
    /// Frame/profile construction failure.
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    /// The connected model does not accept writes; reads remain allowed.
    #[error(
        "charger model {model:?} is read-only; write, start, stop, and profile operations \
         require an MC3000"
    )]
    ModelReadOnly {
        /// The connected model that rejected the write.
        model: ChargerModel,
    },
    /// No model is selected (never connected, or after disconnect): writes are
    /// refused until `connect` provides an explicit model.
    #[error("no charger model is selected; connect with an explicit ChargerModel before writing")]
    ModelUnknown,
}

#[derive(Debug, Default)]
struct ClientState {
    model: Option<ChargerModel>,
    address: Option<String>,
    curve_buffer: Vec<u8>,
}

/// Serialized charger client over a [`Transport`].
///
/// All operations take `&self` and are internally serialized by one request
/// lock, so a concurrent status poll can never consume a profile
/// acknowledgement.
pub struct ChargerClient<T: Transport> {
    transport: T,
    state: Mutex<ClientState>,
    request_lock: tokio::sync::Mutex<()>,
}

impl<T: Transport> ChargerClient<T> {
    /// Wrap a transport backend.
    #[must_use]
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            state: Mutex::new(ClientState::default()),
            request_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// Access the underlying transport (scan, fake-scripting, diagnostics).
    #[must_use]
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// Model explicitly selected for the current connection; `None` when never
    /// connected or after disconnect.
    #[must_use]
    pub fn model(&self) -> Option<ChargerModel> {
        Self::lock(&self.state).model
    }

    /// Address of the current connection; `None` when never connected or after
    /// disconnect.
    #[must_use]
    pub fn address(&self) -> Option<String> {
        Self::lock(&self.state).address.clone()
    }

    /// Whether the transport reports a live link.
    pub async fn is_connected(&self) -> bool {
        self.transport.is_connected().await
    }

    /// Total notifications dropped by the bounded notification queue.
    #[must_use]
    pub fn dropped_notifications(&self) -> u64 {
        self.transport.notification_dropped_count()
    }

    /// Scan for charger-like devices whose advertised names match
    /// [`crate::protocol::DEVICE_NAMES`] exactly or by substring. The filter
    /// applies regardless of backend, mirroring the Python client scan.
    ///
    /// # Errors
    ///
    /// Propagates [`TransportError`] from the backend.
    pub async fn scan(&self, timeout: Duration) -> Result<Vec<ScanDevice>, ChargerError> {
        let devices = self.transport.scan(timeout).await?;
        Ok(devices
            .into_iter()
            .filter(|device| {
                device
                    .name
                    .as_deref()
                    .is_some_and(crate::transport::name_matches_device)
            })
            .collect())
    }

    /// Connect to `address` with an explicit charger model. Any previous link
    /// is dropped first and all pending state (queued notifications, partial
    /// curve buffer) is cleared.
    ///
    /// # Errors
    ///
    /// Propagates [`TransportError`] from the backend.
    pub async fn connect(
        &self,
        address: &str,
        model: ChargerModel,
    ) -> Result<ConnectInfo, ChargerError> {
        let _guard = self.request_lock.lock().await;
        if let Err(error) = self.transport.disconnect().await {
            tracing::warn!(%error, "previous-link disconnect error suppressed");
        }
        self.transport.clear_pending();
        {
            let mut state = Self::lock(&self.state);
            state.curve_buffer.clear();
            state.address = None;
            state.model = None;
        }
        self.transport.connect(address).await?;
        let mut state = Self::lock(&self.state);
        state.address = Some(address.to_owned());
        state.model = Some(model);
        Ok(ConnectInfo {
            connected: true,
            address: address.to_owned(),
            model,
        })
    }

    /// Drop the link and clear all pending state, including the selected
    /// model (a fresh connect must choose it explicitly again).
    ///
    /// # Errors
    ///
    /// Link teardown failures are suppressed like the Python transport; the
    /// `Result` only surfaces unexpected internal errors.
    pub async fn disconnect(&self) -> Result<(), ChargerError> {
        let _guard = self.request_lock.lock().await;
        if let Err(error) = self.transport.disconnect().await {
            tracing::warn!(%error, "disconnect error suppressed");
        }
        self.transport.clear_pending();
        let mut state = Self::lock(&self.state);
        state.curve_buffer.clear();
        state.address = None;
        state.model = None;
        Ok(())
    }

    /// Connect by address, or scan and connect to the first charger-like
    /// device (Python `find_and_connect`).
    ///
    /// # Errors
    ///
    /// [`ChargerError::Transport`] with
    /// [`TransportError::NoDeviceFound`] when scanning found no charger-like
    /// device; otherwise propagates [`ChargerError::Transport`].
    pub async fn find_and_connect(
        &self,
        address: Option<&str>,
        timeout: Duration,
        model: ChargerModel,
    ) -> Result<ConnectInfo, ChargerError> {
        if let Some(address) = address {
            return self.connect(address, model).await;
        }
        let devices = self.scan(timeout).await?;
        let Some(first) = devices.first() else {
            return Err(TransportError::NoDeviceFound {
                names: protocol::DEVICE_NAMES
                    .iter()
                    .map(|name| (*name).to_owned())
                    .collect(),
            }
            .into());
        };
        let address = first.address.clone();
        self.connect(&address, model).await
    }

    /// Write raw bytes to FFE1 without response. Requires a write-capable
    /// model: unknown and read-only models are refused before any bytes move.
    ///
    /// # Errors
    ///
    /// [`ChargerError::ModelUnknown`]/[`ChargerError::ModelReadOnly`] from the
    /// model gate and [`ChargerError::Transport`] from the backend.
    pub async fn write(&self, data: &[u8]) -> Result<(), ChargerError> {
        self.require_write_model()?;
        let _guard = self.request_lock.lock().await;
        Ok(self.transport.write(data).await?)
    }

    /// Start the program in `slot` (write-only, no reply wait, like the APK).
    ///
    /// # Errors
    ///
    /// Same as [`ChargerClient::write`].
    pub async fn start(&self, slot: Slot) -> Result<(), ChargerError> {
        self.require_write_model()?;
        let _guard = self.request_lock.lock().await;
        Ok(self.transport.write(&command_start(slot)).await?)
    }

    /// Stop the program in `slot` (write-only, no reply wait).
    ///
    /// # Errors
    ///
    /// Same as [`ChargerClient::write`].
    pub async fn stop(&self, slot: Slot) -> Result<(), ChargerError> {
        self.require_write_model()?;
        let _guard = self.request_lock.lock().await;
        Ok(self.transport.write(&command_stop(slot)).await?)
    }

    /// Write a frame and wait for the matching parsed reply. The expected
    /// opcode defaults to the frame's opcode byte. Frames whose opcode is a
    /// write acknowledgement (start, set-profile, set-basic, stop, restore)
    /// are refused for unknown/read-only models.
    ///
    /// # Errors
    ///
    /// Model gate errors, [`ChargerError::Transport`] from the backend, and
    /// [`TransportError::ReplyTimeout`] when no matching reply arrives in
    /// `timeout`.
    pub async fn request(
        &self,
        frame: &[u8],
        expect_opcode: Option<Opcode>,
        timeout: Duration,
    ) -> Result<Notification, ChargerError> {
        self.ensure_command_allowed(frame)?;
        let _guard = self.request_lock.lock().await;
        self.transport.write(frame).await?;
        let expect = expect_opcode.or_else(|| frame_opcode(frame));
        self.wait_for_locked(expect, timeout).await
    }

    /// Wait for the next matching parsed notification, discarding non-matching
    /// ones and reassembling voltage-curve fragments (Python `wait_for`).
    ///
    /// # Errors
    ///
    /// [`TransportError::ReplyTimeout`] when nothing matches within `timeout`
    /// and [`TransportError::NotConnected`] when the link ends first.
    pub async fn wait_for(
        &self,
        expect_opcode: Option<Opcode>,
        timeout: Duration,
    ) -> Result<Notification, ChargerError> {
        let _guard = self.request_lock.lock().await;
        self.wait_for_locked(expect_opcode, timeout).await
    }

    /// Poll the MC3000 status of one slot.
    ///
    /// # Errors
    ///
    /// Same as [`ChargerClient::request`].
    pub async fn status(&self, slot: Slot) -> Result<Notification, ChargerError> {
        self.request(
            &command_get_status(slot),
            Some(Opcode::Status),
            REQUEST_TIMEOUT,
        )
        .await
    }

    /// Poll the extended MC5000 status of one slot (read-only path).
    ///
    /// # Errors
    ///
    /// Same as [`ChargerClient::request`].
    pub async fn mc5000_status(&self, slot: Slot) -> Result<Notification, ChargerError> {
        self.request(
            &command_get_mc5000_status(slot),
            Some(Opcode::Mc5000SlotStatus),
            REQUEST_TIMEOUT,
        )
        .await
    }

    /// Poll all four MC3000 slots sequentially with the Python 50 ms gap.
    ///
    /// # Errors
    ///
    /// Same as [`ChargerClient::status`].
    pub async fn poll_all_status(&self) -> Result<Vec<Notification>, ChargerError> {
        let mut result = Vec::with_capacity(4);
        for value in 0..=3u8 {
            let slot = Slot::try_from(value).map_err(ChargerError::Protocol)?;
            result.push(self.status(slot).await?);
            tokio::time::sleep(SLOT_POLL_GAP).await;
        }
        Ok(result)
    }

    /// Request firmware/hardware version.
    ///
    /// # Errors
    ///
    /// [`ChargerError::Protocol`] for an unparsable `mac` plus the
    /// [`ChargerClient::request`] errors.
    pub async fn version(&self, mac: Option<&str>) -> Result<Notification, ChargerError> {
        let frame = protocol::command_get_version(mac)?;
        self.request(&frame, Some(Opcode::Version), REQUEST_TIMEOUT)
            .await
    }

    /// Request the basic-settings report.
    ///
    /// # Errors
    ///
    /// Same as [`ChargerClient::request`].
    pub async fn basic(&self) -> Result<Notification, ChargerError> {
        self.request(
            &command_get_basic(),
            Some(Opcode::GetBasic),
            REQUEST_TIMEOUT,
        )
        .await
    }

    /// Request the voltage curve of one slot; the 245-byte payload is
    /// reassembled from 20-byte notifications when fragmented.
    ///
    /// # Errors
    ///
    /// Same as [`ChargerClient::request`].
    pub async fn curve(&self, slot: Slot, timeout: Duration) -> Result<Notification, ChargerError> {
        self.request(
            &command_get_voltage_curve(slot),
            Some(Opcode::VoltageCurve),
            timeout,
        )
        .await
    }

    /// Write basic settings and await the acknowledgement. Requires a
    /// write-capable model.
    ///
    /// # Errors
    ///
    /// Model gate errors, [`ChargerError::Protocol`] for out-of-range values,
    /// plus the [`ChargerClient::request`] errors.
    pub async fn set_basic(&self, settings: &BasicSettings) -> Result<Notification, ChargerError> {
        self.require_write_model()?;
        let frame = command_set_basic(settings)?;
        self.request(&frame, Some(Opcode::SetBasic), REQUEST_TIMEOUT)
            .await
    }

    /// Apply a 40-byte profile with the exact APK write order: stop every
    /// masked slot (500 ms after each), write chunk 1, wait 50 ms, write
    /// chunk 2; when `start`, wait 500 ms and start every masked slot.
    /// Requires a write-capable model.
    ///
    /// # Errors
    ///
    /// [`ChargerError::Protocol`] for a profile that is not 40 bytes, model
    /// gate errors, and [`ChargerError::Transport`] from the backend.
    pub async fn send_profile(
        &self,
        profile: &[u8],
        start: bool,
    ) -> Result<SendProfileReport, ChargerError> {
        self.require_write_model()?;
        let (first, second) = split_profile(profile)?;
        let slot_mask = profile[2];
        let _guard = self.request_lock.lock().await;
        let mut stopped = Vec::new();
        for value in 0..=3u8 {
            if slot_mask & (1 << value) == 0 {
                continue;
            }
            let slot = Slot::try_from(value).map_err(ChargerError::Protocol)?;
            self.transport.write(&command_stop(slot)).await?;
            stopped.push(value);
            tokio::time::sleep(PROFILE_STOP_DELAY).await;
        }
        self.transport.write(&first).await?;
        tokio::time::sleep(PROFILE_CHUNK_GAP).await;
        self.transport.write(&second).await?;
        if start {
            tokio::time::sleep(PROFILE_START_DELAY).await;
            for value in 0..=3u8 {
                if slot_mask & (1 << value) == 0 {
                    continue;
                }
                let slot = Slot::try_from(value).map_err(ChargerError::Protocol)?;
                self.transport.write(&command_start(slot)).await?;
            }
        }
        Ok(SendProfileReport {
            ok: true,
            stopped_slots: stopped,
            started: start,
            profile_hex: hex::encode_upper(profile),
        })
    }

    async fn wait_for_locked(
        &self,
        expect_opcode: Option<Opcode>,
        timeout: Duration,
    ) -> Result<Notification, ChargerError> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let Some(remaining) = deadline.checked_duration_since(tokio::time::Instant::now())
            else {
                return Err(TransportError::ReplyTimeout(ReplyTarget(
                    expect_opcode.map(Opcode::as_u8),
                ))
                .into());
            };
            let data =
                match tokio::time::timeout(remaining, self.transport.next_notification()).await {
                    Ok(Some(data)) => data,
                    // The notification stream ended: the link is gone.
                    Ok(None) => return Err(TransportError::NotConnected.into()),
                    Err(_elapsed) => {
                        return Err(TransportError::ReplyTimeout(ReplyTarget(
                            expect_opcode.map(Opcode::as_u8),
                        ))
                        .into());
                    }
                };
            // Voltage curves arrive as 20-byte fragments until 245 bytes;
            // a fresh curve opcode restarts the buffer (Python semantics).
            let completed = if data.len() >= protocol::MIN_OPCODE_FRAME_LEN
                && data[1] == Opcode::VoltageCurve.as_u8()
            {
                let mut state = Self::lock(&self.state);
                state.curve_buffer.clear();
                state.curve_buffer.extend_from_slice(&data);
                complete_curve(&mut state.curve_buffer)
            } else {
                let mut state = Self::lock(&self.state);
                if state.curve_buffer.is_empty() {
                    Some(parse_notification(&data))
                } else {
                    state.curve_buffer.extend_from_slice(&data);
                    complete_curve(&mut state.curve_buffer)
                }
            };
            let Some(parsed) = completed else {
                continue;
            };
            if notification_matches(expect_opcode, &parsed) {
                return Ok(parsed);
            }
            // Non-matching notifications are discarded, exactly like Python.
        }
    }

    fn require_write_model(&self) -> Result<ChargerModel, ChargerError> {
        match Self::lock(&self.state).model {
            Some(model) if model.allows_writes() => Ok(model),
            Some(model) => Err(ChargerError::ModelReadOnly { model }),
            None => Err(ChargerError::ModelUnknown),
        }
    }

    fn ensure_command_allowed(&self, frame: &[u8]) -> Result<(), ChargerError> {
        if frame.len() >= protocol::MIN_OPCODE_FRAME_LEN
            && let Some(opcode) = Opcode::from_u8(frame[1])
            && is_write_opcode(opcode)
        {
            self.require_write_model()?;
        }
        Ok(())
    }

    fn lock(state: &Mutex<ClientState>) -> MutexGuard<'_, ClientState> {
        match state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl<T: Transport> std::fmt::Debug for ChargerClient<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = ChargerClient::<T>::lock(&self.state);
        formatter
            .debug_struct("ChargerClient")
            .field("model", &state.model)
            .field("address", &state.address)
            .field("curve_buffer_len", &state.curve_buffer.len())
            .finish_non_exhaustive()
    }
}

/// Opcode byte of a command frame, when the frame is long enough.
fn frame_opcode(frame: &[u8]) -> Option<Opcode> {
    if frame.len() >= protocol::MIN_OPCODE_FRAME_LEN {
        Opcode::from_u8(frame[1])
    } else {
        None
    }
}

/// Whether an opcode class mutates charger state and is therefore gated for
/// read-only models.
fn is_write_opcode(opcode: Opcode) -> bool {
    matches!(
        opcode,
        Opcode::Start
            | Opcode::SetProfile
            | Opcode::SetBasic
            | Opcode::Stop
            | Opcode::RestoreFactory
            | Opcode::RestoreCalibration
    )
}

/// Finish a curve buffer once it reaches [`protocol::CURVE_LEN`]: parse the
/// first 245 bytes and clear the buffer. `None` keeps accumulating.
fn complete_curve(buffer: &mut Vec<u8>) -> Option<Notification> {
    if buffer.len() < protocol::CURVE_LEN {
        return None;
    }
    let parsed = parse_notification(&buffer[..protocol::CURVE_LEN]);
    buffer.clear();
    Some(parsed)
}

/// Whether a parsed notification satisfies an expected opcode, mirroring the
/// Python opcode-or-kind match: status kinds answer both status opcodes,
/// acks and unknown frames compare their opcode byte.
fn notification_matches(expect: Option<Opcode>, parsed: &Notification) -> bool {
    let Some(expect) = expect else {
        return true;
    };
    let code = expect.as_u8();
    match parsed {
        Notification::Unknown { opcode, .. } => *opcode == Some(code),
        Notification::Status { .. } => {
            matches!(expect, Opcode::Status | Opcode::Mc5000SlotStatus)
        }
        Notification::Basic { .. } => expect == Opcode::GetBasic,
        Notification::Version { .. } => expect == Opcode::Version,
        Notification::VoltageCurve { .. } => expect == Opcode::VoltageCurve,
        Notification::Ack { opcode, .. } => *opcode == code,
    }
}
