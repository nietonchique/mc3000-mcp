//! Pure SKYRC MC3000 BLE protocol: 20-byte command frame builders and total
//! notification parsers, recovered from MC3000 Android APK 4.1.2.
//!
//! The app uses a single Nordic-UART-like GATT service/characteristic and sends
//! 20-byte command frames. Longer profile/curve payloads are split over multiple
//! 20-byte notifications/writes. All parsers are total: malformed or truncated
//! notifications produce structured errors or `unknown` values, never panics.

use serde::Serialize;

use crate::model::{self, Slot};
use crate::{ProtocolError, f64_to_u32, index_to_f64, lo8};

/// BLE device names the charger advertises (`protocol.DEVICE_NAMES`).
pub const DEVICE_NAMES: [&str; 3] = ["SimpleBLEPeripheral", "Charger", "HitecCharger"];
/// Nordic-UART-like GATT service UUID (`protocol.SERVICE_UUID`).
pub const SERVICE_UUID: &str = "0000FFE0-0000-1000-8000-00805f9b34fb";
/// Nordic-UART-like GATT characteristic UUID (`protocol.CHAR_UUID`).
pub const CHAR_UUID: &str = "0000FFE1-0000-1000-8000-00805f9b34fb";

/// Command frame length in bytes.
pub const FRAME_LEN: usize = 20;
/// Full profile frame length in bytes (two 20-byte chunks).
pub const PROFILE_LEN: usize = 40;
/// Complete voltage-curve notification length in bytes.
pub const CURVE_LEN: usize = 245;
/// Maximum value of a big-endian `u16` field.
pub const U16_MAX: u16 = 0xFFFF;
/// Maximum slot-mask value (`slot1=1, slot2=2, slot3=4, slot4=8`).
pub const SLOT_MASK_MAX: u8 = 0x0F;
/// Shortest notification from which an opcode can be read.
pub const MIN_OPCODE_FRAME_LEN: usize = 2;
/// Byte count of a MAC address.
pub const MAC_PARTS: usize = 6;

/// Command/notification opcode (`protocol.Opcode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Opcode {
    /// Start the program in a slot.
    Start = 0x05,
    /// Write a 40-byte profile (two chunks).
    SetProfile = 0x11,
    /// Slot status request (MC3000).
    Status = 0x55,
    /// Voltage-curve request.
    VoltageCurve = 0x56,
    /// Firmware/hardware version request.
    Version = 0x57,
    /// Basic settings request.
    GetBasic = 0x61,
    /// Basic settings write.
    SetBasic = 0x63,
    /// Restore factory settings (blocked in normal operation).
    RestoreFactory = 0x65,
    /// Restore calibration (blocked in normal operation).
    RestoreCalibration = 0x66,
    /// MC5000 per-slot status request.
    Mc5000SlotStatus = 0x91,
    /// Stop a slot.
    Stop = 0xFE,
}

impl Opcode {
    /// Numeric opcode byte.
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// Reverse mapping for [`Opcode::as_u8`].
    #[must_use]
    pub const fn from_u8(byte: u8) -> Option<Self> {
        match byte {
            0x05 => Some(Self::Start),
            0x11 => Some(Self::SetProfile),
            0x55 => Some(Self::Status),
            0x56 => Some(Self::VoltageCurve),
            0x57 => Some(Self::Version),
            0x61 => Some(Self::GetBasic),
            0x63 => Some(Self::SetBasic),
            0x65 => Some(Self::RestoreFactory),
            0x66 => Some(Self::RestoreCalibration),
            0x91 => Some(Self::Mc5000SlotStatus),
            0xFE => Some(Self::Stop),
            _ => None,
        }
    }
}

/// One parsed MC3000 slot-status notification (`SlotStatus.to_dict()`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SlotStatus {
    /// Zero-based slot index.
    pub slot: u8,
    /// Raw battery-type code.
    pub battery_type_code: u8,
    /// Battery-type name (`"unknown"` for unknown codes).
    pub battery_type: &'static str,
    /// Raw mode code.
    pub mode_code: u8,
    /// Mode name (`"unknown"` for unknown codes).
    pub mode: &'static str,
    /// Cycle counter byte.
    pub count: u8,
    /// Raw status code.
    pub status_code: u8,
    /// Status name (`"error"` for unknown codes).
    pub status: &'static str,
    /// Seconds elapsed in the current run.
    pub time_seconds: u16,
    /// Last measured cell voltage in millivolts.
    pub voltage_mv: u16,
    /// Instantaneous current in milliamps.
    pub current_ma: u16,
    /// Accumulated capacity in milliamp-hours.
    pub capacity_mah: u16,
    /// Measured temperature in degrees Celsius.
    pub temperature: u8,
    /// Internal resistance in milliohms; `None` for the reserved markers 0, 1, and
    /// 0xFFFF.
    pub internal_resistance_mohm: Option<u16>,
    /// Raw LED byte.
    pub led: u8,
    /// Whether the slot is actively charging/discharging/paused.
    pub is_working: bool,
}

/// Parsed basic-settings notification (`BasicData.to_dict()`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BasicData {
    /// Raw temperature-unit byte.
    pub temp_unit: u8,
    /// `"celsius"` for unit byte 0, otherwise `"fahrenheit"`.
    pub temp_unit_name: &'static str,
    /// Whether the key beep is enabled.
    pub system_beep: bool,
    /// Raw display byte.
    pub display: u8,
    /// Whether the screensaver is enabled.
    pub screensaver: bool,
    /// Raw cooling-fan byte.
    pub cooling_fan: u8,
    /// Reported input voltage in millivolts.
    pub input_mv: u16,
}

/// Parsed firmware/hardware version (`parse_version`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VersionData {
    /// Firmware version formatted with two decimals (e.g. `"1.23"`).
    pub firmware: String,
    /// Hardware version formatted with two decimals (e.g. `"2.20"`).
    pub hardware: String,
    /// Upper-case hex of the full source frame.
    pub raw_hex: String,
}

/// One timestamped voltage-curve sample (`time_series` entry).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct CurvePoint {
    /// Zero-based sample index within the stored array.
    pub index: usize,
    /// Seconds from the start of the run, rounded to three decimals.
    pub time_seconds: f64,
    /// Sample voltage in millivolts.
    pub voltage_mv: u16,
}

/// Parsed voltage-curve notification (`parse_voltage_curve`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VoltageCurve {
    /// Zero-based slot the curve was captured on.
    pub slot: u8,
    /// Charger-provided spacing between stored samples in milliseconds.
    pub interval_ms: u32,
    /// All 120 raw sample values (zero marks the empty tail).
    pub points_mv: Vec<u16>,
    /// Number of leading non-zero samples.
    pub point_count: usize,
    /// Duration of the last sample, or `0.0` for an empty series.
    pub duration_seconds: f64,
    /// Timestamped samples up to the first zero value.
    pub time_series: Vec<CurvePoint>,
    /// Upper-case hex of the first 245 source bytes.
    pub raw_hex: String,
}

/// Parsed MC5000 slot status (`parse_mc5000_status`). Total over arbitrary bytes:
/// short frames yield zeroed/default fields exactly like the Python implementation.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Mc5000Status {
    /// Slot derived from the channel byte; `None` when the byte is not a single-bit
    /// channel mask or the frame is too short.
    pub slot: Option<u8>,
    /// Raw battery-type code (0 for short frames).
    pub battery_type_code: u8,
    /// Battery-type name (`"unknown"` for unknown codes).
    pub battery_type: &'static str,
    /// Raw mode code.
    pub mode_code: u8,
    /// Mode name resolved through the battery-type mode group.
    pub mode: &'static str,
    /// Cycle counter; always 0 for the MC5000 status frame.
    pub count: u8,
    /// Raw status code.
    pub status_code: u8,
    /// Error name when an error code is present, otherwise the status name.
    pub status: &'static str,
    /// Seconds elapsed in the current run.
    pub time_seconds: u32,
    /// Last measured voltage in millivolts.
    pub voltage_mv: u32,
    /// Instantaneous current in milliamps.
    pub current_ma: u32,
    /// Accumulated capacity in milliamp-hours.
    pub capacity_mah: u16,
    /// Measured temperature in degrees Celsius (truncated).
    pub temperature: u32,
    /// Internal resistance in milliohms; `None` for the reserved markers 0, 1, and
    /// 0xFFFF.
    pub internal_resistance_mohm: Option<u16>,
    /// Derived LED byte: 1 while working, 16 once finished.
    pub led: u8,
    /// Whether status codes 1..=4 apply (active work).
    pub is_working: bool,
}

/// A parsed device notification (`parse_notification`).
///
/// Serialized with a `"kind"` tag so the JSON matches the Python dictionaries
/// exactly (`{"kind": "status", "status": {...}}`, ...).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Notification {
    /// Unrecognized or too-short notification.
    Unknown {
        /// Opcode byte when the frame carried at least two bytes.
        #[serde(skip_serializing_if = "Option::is_none")]
        opcode: Option<u8>,
        /// Upper-case hex of the full payload.
        raw_hex: String,
    },
    /// Slot status (MC3000 or MC5000).
    Status {
        /// Parsed status payload.
        status: StatusPayload,
    },
    /// Basic settings report.
    Basic {
        /// Parsed settings.
        basic: BasicData,
    },
    /// Firmware/hardware version report.
    Version {
        /// Firmware version string.
        firmware: String,
        /// Hardware version string.
        hardware: String,
        /// Upper-case hex of the full payload.
        raw_hex: String,
    },
    /// Complete voltage-curve report.
    VoltageCurve {
        /// Zero-based slot.
        slot: u8,
        /// Sample spacing in milliseconds.
        interval_ms: u32,
        /// Raw samples including the zero tail.
        points_mv: Vec<u16>,
        /// Number of leading non-zero samples.
        point_count: usize,
        /// Duration of the last sample (0.0 when empty).
        duration_seconds: f64,
        /// Timestamped samples.
        time_series: Vec<CurvePoint>,
        /// Upper-case hex of the first 245 bytes.
        raw_hex: String,
    },
    /// Write/start acknowledgement.
    Ack {
        /// Acknowledged opcode byte.
        opcode: u8,
        /// Whether the charger reported success (payload byte `1`).
        ok: bool,
        /// Upper-case hex of the full payload.
        raw_hex: String,
    },
}

/// Status payload of [`Notification::Status`]: either an MC3000 slot status or an
/// MC5000 slot status. Untagged, so the serialized JSON is the flat status
/// dictionary exactly as in Python.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum StatusPayload {
    /// MC3000 20-byte slot status.
    Mc3000(SlotStatus),
    /// MC5000 extended slot status.
    Mc5000(Mc5000Status),
}

/// Sum of all bytes, truncated to one byte (`protocol.checksum`).
#[must_use]
#[allow(clippy::cast_possible_truncation)] // masked to 0..=255 first
pub fn checksum(data: &[u8]) -> u8 {
    let sum: u32 = data.iter().map(|&byte| u32::from(byte)).sum();
    (sum & 0xFF) as u8
}

/// Copy `buf` with the last byte replaced by the checksum over the whole buffer
/// (`protocol.with_checksum` with the default index). An empty buffer yields an
/// empty result.
#[must_use]
pub fn with_checksum(buf: &[u8]) -> Vec<u8> {
    let mut out = buf.to_vec();
    let Some(last) = out.len().checked_sub(1) else {
        return out;
    };
    out[last] = 0;
    let sum = checksum(&out);
    out[last] = sum;
    out
}

/// Copy `buf` with `index` replaced by the checksum over the whole buffer
/// (`protocol.with_checksum` with an explicit index).
///
/// # Errors
///
/// [`ProtocolError::ChecksumIndexOutOfRange`] when `index` is outside the buffer.
pub fn with_checksum_at(buf: &[u8], index: usize) -> Result<Vec<u8>, ProtocolError> {
    if index >= buf.len() {
        return Err(ProtocolError::ChecksumIndexOutOfRange {
            index,
            len: buf.len(),
        });
    }
    let mut out = buf.to_vec();
    out[index] = 0;
    let sum = checksum(&out);
    out[index] = sum;
    Ok(out)
}

/// Build a 20-byte frame: `0x0F`, opcode, payload, trailing checksum
/// (`protocol.make_frame`).
///
/// # Errors
///
/// [`ProtocolError::PayloadTooLarge`] when `payload` exceeds 18 bytes.
pub fn make_frame(opcode: Opcode, payload: &[u8]) -> Result<[u8; FRAME_LEN], ProtocolError> {
    if payload.len() > FRAME_LEN - 2 {
        return Err(ProtocolError::PayloadTooLarge);
    }
    let mut buf = [0u8; FRAME_LEN];
    buf[0] = 0x0F;
    buf[1] = opcode.as_u8();
    buf[2..2 + payload.len()].copy_from_slice(payload);
    buf[FRAME_LEN - 1] = checksum(&buf[..FRAME_LEN - 1]);
    Ok(buf)
}

/// Fixed-payload frame builder for the command constructors whose payload length is
/// statically within the 18-byte limit; byte-identical to [`make_frame`].
fn fixed_frame(opcode: Opcode, payload: &[u8]) -> [u8; FRAME_LEN] {
    let mut buf = [0u8; FRAME_LEN];
    buf[0] = 0x0F;
    buf[1] = opcode.as_u8();
    buf[2..2 + payload.len()].copy_from_slice(payload);
    buf[FRAME_LEN - 1] = checksum(&buf[..FRAME_LEN - 1]);
    buf
}

/// Request the MC3000 slot-status frame (`protocol.command_get_status`); the APK
/// shortcut writes `[0f, 55, slot, zeros..., slot + 100]`.
#[must_use]
pub fn command_get_status(slot: Slot) -> [u8; FRAME_LEN] {
    fixed_frame(Opcode::Status, &[slot.value()])
}

/// Request the MC5000 per-slot status (`protocol.command_get_mc5000_status`), a
/// 5-byte frame `[0f, 03, 91, channel, checksum(bytes 2..4)]`.
#[must_use]
pub fn command_get_mc5000_status(slot: Slot) -> [u8; 5] {
    let mut buf = [0u8; 5];
    buf[0] = 0x0F;
    buf[1] = 0x03;
    buf[2] = Opcode::Mc5000SlotStatus.as_u8();
    buf[3] = slot.mask_value();
    buf[4] = checksum(&buf[2..4]);
    buf
}

/// Start the program in a slot (`protocol.command_start`).
#[must_use]
pub fn command_start(slot: Slot) -> [u8; FRAME_LEN] {
    fixed_frame(Opcode::Start, &[slot.mask_value()])
}

/// Stop a slot (`protocol.command_stop`).
#[must_use]
pub fn command_stop(slot: Slot) -> [u8; FRAME_LEN] {
    fixed_frame(Opcode::Stop, &[slot.mask_value()])
}

/// Request the basic-settings frame (`protocol.command_get_basic`).
#[must_use]
pub fn command_get_basic() -> [u8; FRAME_LEN] {
    fixed_frame(Opcode::GetBasic, &[])
}

/// Request the voltage-curve frame (`protocol.command_get_voltage_curve`).
#[must_use]
pub fn command_get_voltage_curve(slot: Slot) -> [u8; FRAME_LEN] {
    fixed_frame(Opcode::VoltageCurve, &[slot.value()])
}

/// Request the firmware/hardware version frame (`protocol.command_get_version`),
/// optionally embedding a MAC address at bytes 3..9.
///
/// # Errors
///
/// [`ProtocolError::InvalidMac`] when `mac` is `Some` and unparsable.
pub fn command_get_version(mac: Option<&str>) -> Result<[u8; FRAME_LEN], ProtocolError> {
    let mut buf = [0u8; FRAME_LEN];
    buf[0] = 0x0F;
    buf[1] = Opcode::Version.as_u8();
    if let Some(mac) = mac {
        buf[3..9].copy_from_slice(&mac_to_bytes(mac)?);
    }
    buf[FRAME_LEN - 1] = checksum(&buf[..FRAME_LEN - 1]);
    Ok(buf)
}

/// Build the restore-factory frame (`protocol.command_restore_factory`). Blocked at
/// the command layer in normal operation; kept for protocol completeness.
#[must_use]
pub fn command_restore_factory() -> [u8; FRAME_LEN] {
    fixed_frame(Opcode::RestoreFactory, &[])
}

/// Build the restore-calibration frame (`protocol.command_restore_calibration`).
/// Blocked at the command layer in normal operation; kept for protocol
/// completeness.
#[must_use]
pub fn command_restore_calibration() -> [u8; FRAME_LEN] {
    fixed_frame(Opcode::RestoreCalibration, &[])
}

/// Basic-settings payload for [`command_set_basic`], mirroring the Python keyword
/// defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BasicSettings {
    /// Temperature-unit byte (`0` = Celsius).
    pub temp_unit: u8,
    /// Key beep enabled.
    pub system_beep: bool,
    /// Display byte.
    pub display: u8,
    /// Screensaver enabled.
    pub screensaver: bool,
    /// Cooling-fan byte.
    pub cooling_fan: u8,
    /// Input voltage in millivolts.
    pub input_mv: u32,
}

impl Default for BasicSettings {
    fn default() -> Self {
        Self {
            temp_unit: 0,
            system_beep: false,
            display: 0,
            screensaver: false,
            cooling_fan: 0,
            input_mv: 11_000,
        }
    }
}

/// Build the basic-settings write frame (`protocol.command_set_basic`).
///
/// # Errors
///
/// [`ProtocolError::U16OutOfRange`] when `input_mv` exceeds 65535.
pub fn command_set_basic(settings: &BasicSettings) -> Result<[u8; FRAME_LEN], ProtocolError> {
    let mut buf = [0u8; FRAME_LEN];
    buf[0] = 0x0F;
    buf[1] = Opcode::SetBasic.as_u8();
    buf[2] = settings.temp_unit;
    buf[3] = u8::from(settings.system_beep);
    buf[4] = settings.display;
    buf[5] = u8::from(settings.screensaver);
    buf[6] = settings.cooling_fan;
    put_u16_be(&mut buf, 7, i64::from(settings.input_mv))?;
    buf[FRAME_LEN - 1] = checksum(&buf[..FRAME_LEN - 1]);
    Ok(buf)
}

/// Write `value` as a big-endian `u16` at `buf[pos..pos + 2]`, rejecting values
/// outside `0..=65535` exactly like the Python `_put_u16_be`.
///
/// # Errors
///
/// [`ProtocolError::U16OutOfRange`] when `value` is outside `0..=65535`.
pub fn put_u16_be(buf: &mut [u8], pos: usize, value: i64) -> Result<(), ProtocolError> {
    if !(0..=i64::from(U16_MAX)).contains(&value) {
        return Err(ProtocolError::U16OutOfRange(value));
    }
    buf[pos] = lo8(value >> 8);
    buf[pos + 1] = lo8(value);
    Ok(())
}

/// Split a 40-byte profile frame into its two 20-byte write chunks
/// (`protocol.split_profile`).
///
/// # Errors
///
/// [`ProtocolError::InvalidProfileLength`] when `profile` is not exactly 40 bytes.
pub fn split_profile(profile: &[u8]) -> Result<([u8; FRAME_LEN], [u8; FRAME_LEN]), ProtocolError> {
    if profile.len() != PROFILE_LEN {
        return Err(ProtocolError::InvalidProfileLength);
    }
    let mut first = [0u8; FRAME_LEN];
    let mut second = [0u8; FRAME_LEN];
    first.copy_from_slice(&profile[..FRAME_LEN]);
    second.copy_from_slice(&profile[FRAME_LEN..]);
    Ok((first, second))
}

/// Parse any device notification (`protocol.parse_notification`). Total: every
/// input maps to a [`Notification`].
#[must_use]
pub fn parse_notification(data: &[u8]) -> Notification {
    if data.len() < MIN_OPCODE_FRAME_LEN {
        return Notification::Unknown {
            opcode: None,
            raw_hex: hex::encode_upper(data),
        };
    }
    let op = data[1];
    if op == Opcode::Status.as_u8()
        && data.len() >= FRAME_LEN
        && let Ok(status) = parse_status(data)
    {
        return Notification::Status {
            status: StatusPayload::Mc3000(status),
        };
    }
    if op == Opcode::GetBasic.as_u8()
        && data.len() >= FRAME_LEN
        && let Ok(basic) = parse_basic(data)
    {
        return Notification::Basic { basic };
    }
    if op == Opcode::Version.as_u8()
        && data.len() >= FRAME_LEN
        && let Ok(version) = parse_version(data)
    {
        return Notification::Version {
            firmware: version.firmware,
            hardware: version.hardware,
            raw_hex: version.raw_hex,
        };
    }
    if op == Opcode::VoltageCurve.as_u8()
        && data.len() >= CURVE_LEN
        && let Ok(curve) = parse_voltage_curve(data)
    {
        return Notification::VoltageCurve {
            slot: curve.slot,
            interval_ms: curve.interval_ms,
            points_mv: curve.points_mv,
            point_count: curve.point_count,
            duration_seconds: curve.duration_seconds,
            time_series: curve.time_series,
            raw_hex: curve.raw_hex,
        };
    }
    if data.len() >= 5 && data[0] == 0x0F && data[2] == Opcode::Mc5000SlotStatus.as_u8() {
        return Notification::Status {
            status: StatusPayload::Mc5000(parse_mc5000_status(data)),
        };
    }
    let is_ack_opcode = Opcode::from_u8(op).is_some_and(|code| {
        matches!(
            code,
            Opcode::Start
                | Opcode::SetProfile
                | Opcode::SetBasic
                | Opcode::RestoreFactory
                | Opcode::RestoreCalibration
        )
    });
    if is_ack_opcode {
        return Notification::Ack {
            opcode: op,
            ok: data.len() > MIN_OPCODE_FRAME_LEN && data[2] == 1,
            raw_hex: hex::encode_upper(data),
        };
    }
    Notification::Unknown {
        opcode: Some(op),
        raw_hex: hex::encode_upper(data),
    }
}

/// Parse the 20-byte MC3000 slot-status frame (`protocol.parse_status`).
///
/// # Errors
///
/// [`ProtocolError::FrameTooShort`] for frames shorter than 20 bytes.
pub fn parse_status(frame: &[u8]) -> Result<SlotStatus, ProtocolError> {
    require_len(frame, FRAME_LEN)?;
    let battery_type_code = frame[3];
    let mode_code = frame[4];
    let status_code = frame[6];
    let internal = u16::from_be_bytes([frame[16], frame[17]]);
    Ok(SlotStatus {
        slot: frame[2],
        battery_type_code,
        battery_type: model::battery_type_name(battery_type_code),
        mode_code,
        mode: model::mode_name(mode_code),
        count: frame[5],
        status_code,
        status: model::status_name(status_code),
        time_seconds: u16::from_be_bytes([frame[7], frame[8]]),
        voltage_mv: u16::from_be_bytes([frame[9], frame[10]]),
        current_ma: u16::from_be_bytes([frame[11], frame[12]]),
        capacity_mah: u16::from_be_bytes([frame[13], frame[14]]),
        temperature: frame[15],
        internal_resistance_mohm: marker_or_value(internal),
        led: frame[18],
        is_working: matches!(status_code, 1..=3),
    })
}

/// Parse the 20-byte basic-settings frame (`protocol.parse_basic`).
///
/// # Errors
///
/// [`ProtocolError::FrameTooShort`] for frames shorter than 20 bytes.
pub fn parse_basic(frame: &[u8]) -> Result<BasicData, ProtocolError> {
    require_len(frame, FRAME_LEN)?;
    let temp_unit = frame[2];
    Ok(BasicData {
        temp_unit,
        temp_unit_name: if temp_unit == 0 {
            "celsius"
        } else {
            "fahrenheit"
        },
        system_beep: frame[3] == 1,
        display: frame[4],
        screensaver: frame[5] == 1,
        cooling_fan: frame[6],
        input_mv: u16::from_be_bytes([frame[7], frame[8]]),
    })
}

/// Parse the 20-byte version frame (`protocol.parse_version`).
///
/// # Errors
///
/// [`ProtocolError::FrameTooShort`] for frames shorter than 20 bytes.
pub fn parse_version(frame: &[u8]) -> Result<VersionData, ProtocolError> {
    require_len(frame, FRAME_LEN)?;
    let firmware = f64::from(u16::from(frame[14]) * 100 + u16::from(frame[15])) / 100.0;
    let hardware = f64::from(u16::from(frame[16]) * 10) / 100.0;
    Ok(VersionData {
        firmware: format!("{firmware:.2}"),
        hardware: format!("{hardware:.2}"),
        raw_hex: hex::encode_upper(frame),
    })
}

/// Parse the 245-byte voltage-curve notification (`protocol.parse_voltage_curve`).
///
/// # Errors
///
/// [`ProtocolError::CurveTooShort`] for payloads shorter than 245 bytes.
pub fn parse_voltage_curve(data: &[u8]) -> Result<VoltageCurve, ProtocolError> {
    if data.len() < CURVE_LEN {
        return Err(ProtocolError::CurveTooShort {
            expected: CURVE_LEN,
            got: data.len(),
        });
    }
    let slot = data[2];
    let interval_ms = u32::from(u16::from_be_bytes([data[3], data[4]])) * 1000;
    let points_mv: Vec<u16> = data[5..CURVE_LEN]
        .chunks_exact(2)
        .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
        .collect();
    let time_series = voltage_curve_time_series(&points_mv, interval_ms);
    let duration_seconds = time_series.last().map_or(0.0, |point| point.time_seconds);
    Ok(VoltageCurve {
        slot,
        interval_ms,
        points_mv,
        point_count: time_series.len(),
        duration_seconds,
        time_series,
        raw_hex: hex::encode_upper(&data[..CURVE_LEN]),
    })
}

/// Convert the MC3000 app-style voltage curve array into timestamped points
/// (`protocol.voltage_curve_time_series`).
///
/// The Android app stores up to 120 voltage samples and treats zero as empty tail.
/// When the array fills during a long run, the app downsamples in-place and keeps
/// using the same 120-point buffer. The charger-provided interval is therefore the
/// authoritative spacing between adjacent stored samples.
#[must_use]
pub fn voltage_curve_time_series(points_mv: &[u16], interval_ms: u32) -> Vec<CurvePoint> {
    let interval_seconds = f64::from(interval_ms) / 1000.0;
    let mut series = Vec::new();
    for (index, &voltage_mv) in points_mv.iter().enumerate() {
        if voltage_mv == 0 {
            break;
        }
        series.push(CurvePoint {
            index,
            time_seconds: round3(index_to_f64(index) * interval_seconds),
            voltage_mv,
        });
    }
    series
}

/// Render a parsed voltage curve as CSV (`protocol.voltage_curve_to_csv`).
#[must_use]
pub fn voltage_curve_to_csv(curve: &VoltageCurve) -> String {
    let mut rows = vec!["index,time_seconds,voltage_mv,voltage_v".to_string()];
    for point in &curve.time_series {
        let voltage_mv = point.voltage_mv;
        rows.push(format!(
            "{},{},{},{:.3}",
            point.index,
            python_float(point.time_seconds),
            voltage_mv,
            f64::from(voltage_mv) / 1000.0
        ));
    }
    rows.join("\n") + "\n"
}

/// Parse the MC5000 slot-status frame (`protocol.parse_mc5000_status`). Total over
/// arbitrary bytes: short frames yield zeroed/default fields exactly like the
/// Python implementation.
#[must_use]
pub fn parse_mc5000_status(frame: &[u8]) -> Mc5000Status {
    let slot = (frame.len() >= 5)
        .then(|| frame[3])
        .and_then(|channel| (0u8..4).find(|&s| (1 << s) == channel));
    let battery_type_code = frame.get(21).copied().unwrap_or(0);
    let mode_code = frame.get(19).copied().unwrap_or(0);
    let status_code = frame.get(18).copied().unwrap_or(0);
    let error_code = frame.get(20).copied().unwrap_or(0);
    let status_base = model::mc5000_status_name(status_code);
    let error = model::mc5000_error_name(error_code);
    let status = if error.is_empty() { status_base } else { error };
    let read2 = |start: usize| -> u16 {
        if frame.len() >= start + 2 {
            u16::from_be_bytes([frame[start], frame[start + 1]])
        } else {
            0
        }
    };
    let read4 = |start: usize| -> u32 {
        if frame.len() >= start + 4 {
            u32::from_be_bytes([
                frame[start],
                frame[start + 1],
                frame[start + 2],
                frame[start + 3],
            ])
        } else {
            0
        }
    };
    let resistance_raw = read2(16);
    let led = if (1..=4).contains(&status_code) {
        1
    } else if status_code >= 5 {
        16
    } else {
        0
    };
    Mc5000Status {
        slot,
        battery_type_code,
        battery_type: model::mc5000_battery_type_name(battery_type_code),
        mode_code,
        mode: model::mc5000_mode_name(battery_type_code, mode_code),
        count: 0,
        status_code,
        status,
        time_seconds: read4(12),
        voltage_mv: f64_to_u32(f64::from(read2(6)) / 1000.0 * 1000.0),
        current_ma: f64_to_u32(f64::from(read2(4)) / 1000.0 * 1000.0),
        capacity_mah: read2(10),
        temperature: f64_to_u32(f64::from(read2(8)) / 1000.0),
        internal_resistance_mohm: marker_or_value(resistance_raw),
        led,
        is_working: (1..=4).contains(&status_code),
    }
}

/// Parse a `AA:BB:CC:DD:EE:FF` / `AA-BB-CC-DD-EE-FF` MAC address
/// (`protocol.mac_to_bytes`).
///
/// # Errors
///
/// [`ProtocolError::InvalidMac`] when the address does not split into exactly six
/// hex bytes.
pub fn mac_to_bytes(mac: &str) -> Result<[u8; MAC_PARTS], ProtocolError> {
    let normalized = mac.replace('-', ":");
    let parts: Vec<&str> = normalized.split(':').collect();
    if parts.len() != MAC_PARTS {
        return Err(ProtocolError::InvalidMac(mac.to_string()));
    }
    let mut out = [0u8; MAC_PARTS];
    for (slot, part) in parts.iter().enumerate() {
        let byte = u8::from_str_radix(part.trim(), 16)
            .map_err(|_| ProtocolError::InvalidMac(mac.to_string()))?;
        out[slot] = byte;
    }
    Ok(out)
}

/// Decode a hex string, ignoring all whitespace (`protocol.bytes_from_hex`).
///
/// # Errors
///
/// [`ProtocolError::InvalidHex`] on invalid digits or odd length.
pub fn bytes_from_hex(hex_string: &str) -> Result<Vec<u8>, ProtocolError> {
    let cleaned: String = hex_string.split_whitespace().collect();
    hex::decode(cleaned.as_bytes()).map_err(|_| ProtocolError::InvalidHex(hex_string.to_string()))
}

fn require_len(data: &[u8], expected: usize) -> Result<(), ProtocolError> {
    if data.len() < expected {
        return Err(ProtocolError::FrameTooShort {
            expected,
            got: data.len(),
        });
    }
    Ok(())
}

/// Reserved internal-resistance markers 0, 1, and 0xFFFF map to `None`.
fn marker_or_value(raw: u16) -> Option<u16> {
    if raw == 0 || raw == 1 || raw == u16::MAX {
        None
    } else {
        Some(raw)
    }
}

/// Round to three decimals exactly like Python `round(value, 3)` (correct decimal
/// rounding of the binary value, half to even).
fn round3(value: f64) -> f64 {
    format!("{value:.3}").parse().unwrap_or(value)
}

/// Render an `f64` the way Python `str()` does: integral floats keep a `.0` suffix.
#[allow(clippy::float_cmp)] // exact zero/integral check is the point of this formatter
fn python_float(value: f64) -> String {
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0.0".to_string()
        } else {
            "0.0".to_string()
        };
    }
    if value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        format!("{value}")
    }
}
