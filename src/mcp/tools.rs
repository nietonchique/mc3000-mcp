//! Production and legacy MCP tool handlers (`server.TOOLS`).
//!
//! Every Python tool name, description, and schema/default is reproduced; the
//! section-4 hardening applies to the legacy `mc3000_*` write paths: they
//! require confirmation tokens exactly like the `charger.*` tools, emergency
//! stops stay tokenless, and `mc3000_send_raw` is never profile-interpreted
//! and blocks the restore factory/calibration opcodes.

use std::time::Duration;

use rmcp::ErrorData;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::client::ChargerError;
use crate::mcp::McpService;
use crate::model::{ChargerModel, Slot, SlotMask};
use crate::profile::{CycleMode, ProfileParams, build_profile};
use crate::protocol::{
    self, BasicSettings, Notification, Opcode, VoltageCurve, bytes_from_hex, split_profile,
    voltage_curve_to_csv,
};
use crate::safety;
use crate::transport::TransportError;
use crate::{ProtocolError, lo8};

/// Confirmation token required by the hardened `mc3000_set_basic`.
pub const SET_BASIC_TOKEN: &str = "SET_BASIC";
/// Confirmation token required by the hardened `mc3000_send_raw`.
pub const SEND_RAW_TOKEN: &str = "SEND_RAW";

/// Render a charger model as the lowercase wire name used in connect results.
#[must_use]
pub fn model_name(model: ChargerModel) -> &'static str {
    match model {
        ChargerModel::Mc3000 => "mc3000",
        ChargerModel::Mc5000 => "mc5000",
    }
}

/// Python-shaped internal error (`-32603`) carrying the human-readable message.
fn internal_error(error: impl std::fmt::Display) -> ErrorData {
    ErrorData::internal_error(error.to_string(), None)
}

/// Parse a zero-based slot number.
fn slot_from(value: u8) -> Result<Slot, ErrorData> {
    Slot::try_from(value).map_err(internal_error)
}

/// Parse a slot-mask argument with the exact Python error message.
fn mask_from(value: i64) -> Result<SlotMask, ErrorData> {
    let byte = u8::try_from(value)
        .map_err(|_| internal_error(ProtocolError::InvalidSlotMask.to_string()))?;
    SlotMask::try_from(byte).map_err(internal_error)
}

/// Serialize a tool result as the single pretty-JSON text content the Python
/// server produced (`_json_result`).
fn json_result(value: &impl serde::Serialize) -> Result<CallToolResult, ErrorData> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|error| internal_error(format!("result serialization failed: {error}")))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

/// Serialize a parsed device notification.
fn notification_value(notification: &Notification) -> Result<Value, ErrorData> {
    serde_json::to_value(notification)
        .map_err(|error| internal_error(format!("notification serialization failed: {error}")))
}

/// Require an exact confirmation token (Python `ValueError` message shape).
fn require_token(
    provided: Option<&str>,
    expected: &str,
    requirement: &str,
) -> Result<(), ErrorData> {
    if provided == Some(expected) {
        return Ok(());
    }
    Err(internal_error(format!(
        "{requirement} requires confirmation_token={expected}"
    )))
}

/// Convert a Python-style timeout (seconds) with typed validation.
fn timeout_or_error(value: f64) -> Result<Duration, ErrorData> {
    crate::transport::timeout_secs(value).map_err(internal_error)
}

/// Extract the parsed [`VoltageCurve`] from a curve reply.
fn curve_from(notification: Notification) -> Result<VoltageCurve, ErrorData> {
    match notification {
        Notification::VoltageCurve {
            slot,
            interval_ms,
            points_mv,
            point_count,
            duration_seconds,
            time_series,
            raw_hex,
        } => Ok(VoltageCurve {
            slot,
            interval_ms,
            points_mv,
            point_count,
            duration_seconds,
            time_series,
            raw_hex,
        }),
        other => Err(internal_error(format!(
            "expected a voltage curve reply, got: {other:?}"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Argument types (schemas generated with schemars)
// ---------------------------------------------------------------------------

/// Scan-window default (`timeout=5`).
fn default_scan_timeout() -> f64 {
    5.0
}

/// Voltage-curve timeout default (`timeout=8`).
fn default_curve_timeout() -> f64 {
    8.0
}

/// Raw-command wait timeout default (`timeout=3`).
fn default_raw_timeout() -> f64 {
    3.0
}

/// Input-voltage default of `mc3000_set_basic`.
fn default_input_mv() -> i64 {
    11_000
}

/// Charger family selected on connect; anything but `mc3000` is read-only.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ConnectModel {
    /// SKYRC MC3000; full protocol support (the default).
    #[default]
    Mc3000,
    /// SKYRC MC5000; read-only slot status support.
    Mc5000,
}

impl From<ConnectModel> for ChargerModel {
    fn from(value: ConnectModel) -> Self {
        match value {
            ConnectModel::Mc3000 => ChargerModel::Mc3000,
            ConnectModel::Mc5000 => ChargerModel::Mc5000,
        }
    }
}

/// Arguments of `mc3000_scan` and `charger.scan_devices`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ScanArgs {
    /// Scan window in seconds.
    #[serde(default = "default_scan_timeout")]
    pub timeout: f64,
}

/// Arguments of `mc3000_connect` and `charger.connect`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ConnectArgs {
    /// BLE address to connect to; omit to scan and connect to the first match.
    #[serde(default)]
    pub address: Option<String>,
    /// Scan window in seconds.
    #[serde(default = "default_scan_timeout")]
    pub timeout: f64,
    /// Charger model of this connection; writes require an MC3000. Defaults to
    /// `mc3000`.
    #[serde(default)]
    pub model: ConnectModel,
}

/// Arguments of `mc3000_status` and `charger.read_slots`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct OptionalSlotArgs {
    /// Zero-based slot 0..3; omit to poll all four.
    #[serde(default)]
    #[schemars(range(min = 0, max = 3))]
    pub slot: Option<u8>,
}

/// Arguments of `mc3000_start` (hardened: requires the start token).
#[derive(Debug, Deserialize, JsonSchema)]
pub struct LegacyStartArgs {
    /// Zero-based slot 0..3.
    #[schemars(range(min = 0, max = 3))]
    pub slot: u8,
    /// Required confirmation token `START_SLOT_<slot>`.
    #[serde(default)]
    pub confirmation_token: Option<String>,
}

/// Arguments of `mc3000_stop` and `charger.stop_slot` (tokenless).
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SlotArgs {
    /// Zero-based slot 0..3.
    #[schemars(range(min = 0, max = 3))]
    pub slot: u8,
}

/// Arguments of `mc3000_get_version`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct VersionArgs {
    /// Optional charger MAC to include exactly like the Android app.
    #[serde(default)]
    pub mac: Option<String>,
}

/// Arguments of `mc3000_set_basic` (hardened: requires `SET_BASIC`).
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct SetBasicArgs {
    /// `0=celsius, 1=fahrenheit`.
    #[serde(default)]
    pub temp_unit: Option<i64>,
    /// Key beep enabled.
    #[serde(default)]
    pub system_beep: Option<bool>,
    /// Display mode byte.
    #[serde(default)]
    pub display: Option<i64>,
    /// Screensaver enabled.
    #[serde(default)]
    pub screensaver: Option<bool>,
    /// Cooling-fan byte.
    #[serde(default)]
    pub cooling_fan: Option<i64>,
    /// Input voltage in millivolts.
    #[serde(default)]
    pub input_mv: Option<i64>,
    /// Required confirmation token `SET_BASIC`.
    #[serde(default)]
    pub confirmation_token: Option<String>,
}

/// Arguments of `mc3000_get_voltage_curve` and `charger.get_voltage_curve`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CurveArgs {
    /// Zero-based slot 0..3.
    #[schemars(range(min = 0, max = 3))]
    pub slot: u8,
    /// Reply timeout in seconds.
    #[serde(default = "default_curve_timeout")]
    pub timeout: f64,
}

/// Arguments of `charger.export_voltage_curve`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CurveExportArgs {
    /// Zero-based slot 0..3.
    #[schemars(range(min = 0, max = 3))]
    pub slot: u8,
    /// Export format.
    #[serde(default)]
    pub format: Option<String>,
    /// Reply timeout in seconds.
    #[serde(default = "default_curve_timeout")]
    pub timeout: f64,
}

/// Arguments of `mc3000_mc5000_status`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct Mc5000StatusArgs {
    /// Zero-based slot 0..3.
    #[schemars(range(min = 0, max = 3))]
    pub slot: u8,
}

/// Cycle-mode argument: a numeric charger code or a label such as `"D>C"`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum CycleModeArg {
    /// Numeric charger cycle-mode code.
    Code(i64),
    /// Cycle-mode label (`"C>D"`, `"C>D>C"`, `"D>C"`, `"D>C>D"`).
    Name(String),
}

/// The profile/program fields shared by `mc3000_build_profile` and
/// `mc3000_apply_profile` (Python `_profile_schema` properties and defaults).
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ProfileFields {
    /// slot bitmask: slot1=1, slot2=2, slot3=4, slot4=8.
    #[schemars(range(min = 1, max = 15), required)]
    pub slot_mask: Option<i64>,
    /// Battery-type code; the JSON schema lists the exact mapping.
    #[schemars(
        description = "0 LiIon, 1 LiFe, 2 LiIo4.35, 3 NiMH, 4 NiCd, 5 NiZn, 6 Eneloop, 7 RAM, 8 LTO, 9 Na-Lion",
        extend("default" = 0)
    )]
    pub battery_type: Option<i64>,
    /// Charger operation code; the JSON schema lists the exact mapping.
    #[schemars(
        description = "0 charge, 1 refresh, 2 storage/break-in, 3 discharge, 4 cycle",
        extend("default" = 0)
    )]
    pub mode: Option<i64>,
    /// Rated capacity in milliamp-hours.
    #[schemars(extend("default" = 2000))]
    pub capacity_mah: Option<i64>,
    /// Charge current in milliamps; omitted derives the mode default.
    #[schemars(extend("default" = 1000))]
    pub charge_current_ma: Option<i64>,
    /// Discharge current in milliamps; omitted derives the mode default.
    #[schemars(extend("default" = 500))]
    pub discharge_current_ma: Option<i64>,
    /// Omit for chemistry default; NiMH/NiCd/Eneloop default is 1650mV.
    pub charge_stop_voltage_mv: Option<i64>,
    /// Omit for chemistry default; NiMH/NiCd/Eneloop default is 1000mV.
    pub discharge_stop_voltage_mv: Option<i64>,
    /// Omit for chemistry default; NiMH/NiCd/Eneloop default is 50mA.
    pub charge_stop_current_ma: Option<i64>,
    /// Discharge stop current in milliamps.
    #[schemars(extend("default" = 500))]
    pub discharge_stop_current_ma: Option<i64>,
    /// Rest minutes after charge.
    #[schemars(extend("default" = 0))]
    pub charge_rest_minutes: Option<i64>,
    /// Cycle count.
    #[schemars(extend("default" = 1))]
    pub cycle_count: Option<i64>,
    /// Cycle mode as numeric code or label.
    #[schemars(extend("default" = 0))]
    pub cycle_mode: Option<CycleModeArg>,
    /// Negative delta-V cutoff in millivolts.
    #[schemars(extend("default" = 0))]
    pub negative_delta_mv: Option<i64>,
    /// Trickle current in milliamps.
    #[schemars(extend("default" = 0))]
    pub trickle_current_ma: Option<i64>,
    /// Omit for chemistry default; NiMH/NiCd/Eneloop default is 1000mV.
    pub keep_voltage_mv: Option<i64>,
    /// Temperature cutoff in degrees Celsius.
    #[schemars(extend("default" = 45))]
    pub temp_cutoff: Option<i64>,
    /// Hard time limit in minutes (0 disables).
    #[schemars(extend("default" = 0))]
    pub time_limit_minutes: Option<i64>,
    /// Rest minutes after discharge.
    #[schemars(extend("default" = 0))]
    pub discharge_rest_minutes: Option<i64>,
    /// Explicit break-in flag.
    #[schemars(extend("default" = false))]
    pub breakin: Option<bool>,
}

impl ProfileFields {
    /// Convert into typed [`ProfileParams`] with the Python coercion rules.
    ///
    /// # Errors
    ///
    /// [`ProtocolError`] messages for invalid masks and unknown cycle-mode
    /// labels.
    pub fn params(&self) -> Result<ProfileParams, ErrorData> {
        let slot_mask = self
            .slot_mask
            .ok_or_else(|| internal_error("slot_mask is required to build a profile frame"))?;
        let cycle_mode = match &self.cycle_mode {
            None => CycleMode::Code(0),
            Some(CycleModeArg::Code(code)) => CycleMode::Code(lo8(*code)),
            Some(CycleModeArg::Name(name)) => CycleMode::from_name(name).map_err(internal_error)?,
        };
        Ok(ProfileParams {
            slot_mask: mask_from(slot_mask)?,
            battery_type: self.battery_type.map_or(0, lo8),
            mode: self.mode.map_or(0, lo8),
            capacity_mah: self.capacity_mah.unwrap_or(2_000),
            charge_current_ma: self.charge_current_ma,
            discharge_current_ma: self.discharge_current_ma,
            charge_stop_voltage_mv: self.charge_stop_voltage_mv,
            discharge_stop_voltage_mv: self.discharge_stop_voltage_mv,
            discharge_stop_current_ma: self.discharge_stop_current_ma,
            charge_stop_current_ma: self.charge_stop_current_ma,
            charge_rest_minutes: self.charge_rest_minutes,
            cycle_count: self.cycle_count,
            cycle_mode,
            negative_delta_mv: self.negative_delta_mv,
            trickle_current_ma: self.trickle_current_ma,
            keep_voltage_mv: self.keep_voltage_mv,
            temp_cutoff: self.temp_cutoff.unwrap_or(45),
            time_limit_minutes: self.time_limit_minutes,
            discharge_rest_minutes: self.discharge_rest_minutes,
            breakin: self.breakin.unwrap_or(false),
        })
    }
}

/// Arguments of `mc3000_build_profile`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct BuildProfileArgs {
    /// Profile/program fields.
    #[serde(flatten)]
    pub profile: ProfileFields,
    /// Arguments that are not profile fields (rejected like Python's
    /// unexpected-keyword `TypeError`).
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Arguments of `mc3000_apply_profile` (hardened: dry-run by default, one
/// selected slot, `APPLY_PROFILE_SLOT_<slot>` token, no implicit start).
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ApplyProfileArgs {
    /// Profile/program fields (used when `profile_hex` is omitted).
    #[serde(flatten)]
    pub profile: ProfileFields,
    /// Pre-built 40-byte profile frame from `mc3000_build_profile`.
    #[serde(default)]
    pub profile_hex: Option<String>,
    /// Rejected: start the applied profile with `mc3000_start` instead.
    #[serde(default)]
    pub start: Option<bool>,
    /// Build and report without writing to the charger (the default).
    #[schemars(extend("default" = true))]
    pub dry_run: Option<bool>,
    /// Required for live writes: `APPLY_PROFILE_SLOT_<slot>`.
    #[serde(default)]
    pub confirmation_token: Option<String>,
    /// Arguments that are neither profile nor control fields (rejected like
    /// Python's unexpected-keyword `TypeError`).
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Arguments of `mc3000_send_raw` (hardened: `SEND_RAW` token, no profile
/// interpretation, restore opcodes blocked).
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct SendRawArgs {
    /// Raw hex frame to send (preferred spelling).
    #[serde(default)]
    pub hex: Option<String>,
    /// Raw hex frame to send (legacy spelling).
    #[serde(default)]
    pub frame_hex: Option<String>,
    /// Wait for the next notification after the write.
    #[serde(default)]
    pub wait: Option<bool>,
    /// Reply wait in seconds.
    #[serde(default = "default_raw_timeout")]
    pub timeout: f64,
    /// Rejected: profiles are applied with `mc3000_apply_profile`.
    #[serde(default)]
    pub start: Option<bool>,
    /// Required confirmation token `SEND_RAW`.
    #[serde(default)]
    pub confirmation_token: Option<String>,
}

/// Arguments of `charger.validate_profile` (profile precedence: `profile` >
/// known `profile_id` > raw fields).
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ValidateProfileArgs {
    /// Safety-profile object to validate.
    #[serde(default)]
    pub profile: Option<Value>,
    /// Bundled profile identifier to validate.
    #[serde(default)]
    pub profile_id: Option<String>,
    /// Raw profile fields (used when `profile`/`profile_id` are omitted).
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Arguments of `charger.apply_profile`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ChargerApplyArgs {
    /// Zero-based target slot 0..3.
    #[schemars(range(min = 0, max = 3))]
    pub slot: u8,
    /// Safety-profile object to apply.
    #[serde(default)]
    pub profile: Option<Value>,
    /// Bundled profile identifier to apply.
    #[serde(default)]
    pub profile_id: Option<String>,
    /// Dry-run by default; live writes need the confirmation token.
    #[schemars(extend("default" = true))]
    pub dry_run: Option<bool>,
    /// Required for live writes: `APPLY_PROFILE_SLOT_<slot>`.
    #[serde(default)]
    pub confirmation_token: Option<String>,
    /// Raw profile fields (used when `profile`/`profile_id` are omitted).
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Arguments of `charger.start` (hardened write; token required).
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct ChargerStartArgs {
    /// Zero-based slot 0..3.
    #[schemars(range(min = 0, max = 3))]
    pub slot: u8,
    /// Required confirmation token `START_SLOT_<slot>`.
    pub confirmation_token: String,
}

/// Python-side key exclusions for raw profile fields
/// (`_profile_from_args`).
const RAW_PROFILE_EXCLUDED: [&str; 3] = ["slot", "dry_run", "confirmation_token"];

/// Render the raw-fields fallback of `_profile_from_args` from flattened
/// arguments. `profile_id` was consumed by the typed field but belongs in the
/// raw profile exactly like the Python exclusion set (`slot`, `dry_run`,
/// `confirmation_token` are the only excluded keys).
fn raw_profile_from(extra: &Map<String, Value>, profile_id: Option<&str>) -> Value {
    let mut out = Map::new();
    if let Some(profile_id) = profile_id {
        out.insert("profile_id".to_owned(), json!(profile_id));
    }
    for (key, value) in extra {
        if !RAW_PROFILE_EXCLUDED.contains(&key.as_str()) {
            out.insert(key.clone(), value.clone());
        }
    }
    Value::Object(out)
}

/// Reject flattened arguments that are not profile fields, exactly like
/// Python's `build_profile() got an unexpected keyword argument`.
fn reject_unknown(extra: &Map<String, Value>) -> Result<(), ErrorData> {
    if let Some((key, _)) = extra.iter().next() {
        return Err(internal_error(format!(
            "unexpected profile argument '{key}'"
        )));
    }
    Ok(())
}

/// The chosen profile source of `_profile_from_args`.
enum ProfileSource {
    /// Explicit `profile` object.
    Object(Value),
    /// Bundled profile copied by `profile_id`.
    Bundled(Value),
    /// Remaining raw fields.
    Raw(Value),
}

/// Profile precedence (`profile` > known `profile_id` > raw fields).
fn profile_from_args(
    profile: Option<&Value>,
    profile_id: Option<&str>,
    extra: &Map<String, Value>,
) -> ProfileSource {
    if let Some(profile) = profile.filter(|profile| profile.is_object()) {
        return ProfileSource::Object(profile.clone());
    }
    if let Some(profile_id) = profile_id
        && let Some(bundled) = safety::profiles().get(profile_id)
    {
        return ProfileSource::Bundled(bundled.clone());
    }
    ProfileSource::Raw(raw_profile_from(extra, profile_id))
}

impl ProfileSource {
    /// The JSON profile passed to the safety layer.
    fn value(&self) -> Value {
        match self {
            ProfileSource::Object(value)
            | ProfileSource::Bundled(value)
            | ProfileSource::Raw(value) => value.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tool implementations
// ---------------------------------------------------------------------------

/// Description constants keep the exact Python tool descriptions.
mod descriptions {
    /// `mc3000_scan`.
    pub const MC3000_SCAN: &str = "Scan BLE devices and return devices whose advertised name \
         matches the official MC3000 app filters.";
    /// `mc3000_connect`.
    pub const MC3000_CONNECT: &str = "Connect to MC3000 by BLE address, or scan and connect to \
         the first matching charger if address is omitted.";
    /// `mc3000_disconnect`.
    pub const MC3000_DISCONNECT: &str = "Disconnect from charger.";
    /// `mc3000_status`.
    pub const MC3000_STATUS: &str = "Poll slot status. Slot is zero-based 0..3; omit slot to \
         poll all four.";
    /// `mc3000_start`.
    pub const MC3000_START: &str = "Start charging/program on a zero-based slot 0..3.";
    /// `mc3000_stop`.
    pub const MC3000_STOP: &str = "Stop a zero-based slot 0..3.";
    /// `mc3000_get_version`.
    pub const MC3000_GET_VERSION: &str = "Read firmware/hardware version.";
    /// `mc3000_get_basic`.
    pub const MC3000_GET_BASIC: &str = "Read basic device settings.";
    /// `mc3000_set_basic`.
    pub const MC3000_SET_BASIC: &str = "Set basic device settings.";
    /// `mc3000_get_voltage_curve`.
    pub const MC3000_GET_VOLTAGE_CURVE: &str = "Read voltage curve for a zero-based slot. \
         Response is parsed into millivolt points.";
    /// `mc3000_mc5000_status`.
    pub const MC3000_MC5000_STATUS: &str = "Read slot status using MC5000 frame layout \
         (opcode 0x91).";
    /// `mc3000_build_profile`.
    pub const MC3000_BUILD_PROFILE: &str =
        "Build a 40-byte MC3000 profile/program frame without sending it.";
    /// `mc3000_apply_profile`.
    pub const MC3000_APPLY_PROFILE: &str = "Apply a profile/program. Pass profile_hex from \
         mc3000_build_profile or pass the same profile fields directly. Dry-run is the default; \
         live writes support exactly one selected slot and need its \
         APPLY_PROFILE_SLOT_<slot> token.";
    /// `mc3000_send_raw`.
    pub const MC3000_SEND_RAW: &str = "Send a raw hex frame. Escape hatch for protocol \
         experiments; never interpreted as a profile.";
    /// `charger.scan_devices`.
    pub const CHARGER_SCAN_DEVICES: &str = "Read-only BLE scan for supported chargers.";
    /// `charger.connect`.
    pub const CHARGER_CONNECT: &str =
        "Connect to a selected charger. Prefer an explicit address from scan_devices.";
    /// `charger.get_status`.
    pub const CHARGER_GET_STATUS: &str = "Read current charger status for all slots.";
    /// `charger.read_slots`.
    pub const CHARGER_READ_SLOTS: &str =
        "Read one slot or all slot statuses. Slot is zero-based 0..3.";
    /// `charger.get_voltage_curve`.
    pub const CHARGER_GET_VOLTAGE_CURVE: &str =
        "Read the charger-stored app-style voltage time-series for one slot. No polling.";
    /// `charger.export_voltage_curve`.
    pub const CHARGER_EXPORT_VOLTAGE_CURVE: &str =
        "Export the charger-stored voltage curve as JSON or CSV.";
    /// `charger.list_profiles`.
    pub const CHARGER_LIST_PROFILES: &str = "List bundled safe example profiles.";
    /// `charger.validate_profile`.
    pub const CHARGER_VALIDATE_PROFILE: &str =
        "Validate a battery profile against enforced chemistry/device/safety limits.";
    /// `charger.apply_profile`.
    pub const CHARGER_APPLY_PROFILE: &str =
        "Validate and apply a profile to a slot. Dry-run is default; live write requires token.";
    /// `charger.start`.
    pub const CHARGER_START: &str =
        "Start the already-applied operation on a slot. Requires START_SLOT_<slot> token.";
    /// `charger.stop_slot`.
    pub const CHARGER_STOP_SLOT: &str = "Emergency stop for one slot; bypasses profile \
         validation.";
    /// `charger.stop_all`.
    pub const CHARGER_STOP_ALL: &str = "Emergency stop for all slots; bypasses profile \
         validation.";
    /// `charger.export_session_log`.
    pub const CHARGER_EXPORT_SESSION_LOG: &str =
        "Export in-memory session log with redacted device identifiers.";
}

/// Serialize tool arguments for the session log.
fn args_value<T: Serialize>(args: &T) -> Result<Value, ErrorData> {
    serde_json::to_value(args)
        .map_err(|error| internal_error(format!("argument serialization failed: {error}")))
}

impl McpService {
    /// Shared scan implementation (`tool_scan`).
    async fn run_scan(&self, timeout: Duration) -> Result<CallToolResult, ErrorData> {
        let devices = self.client.scan(timeout).await.map_err(internal_error)?;
        let mut list = Vec::with_capacity(devices.len());
        for device in &devices {
            list.push(serde_json::to_value(device).map_err(internal_error)?);
        }
        json_result(&json!({
            "devices": list,
            "matched_names": protocol::DEVICE_NAMES,
        }))
    }

    /// Shared connect implementation (`tool_connect`).
    async fn run_connect(&self, args: ConnectArgs) -> Result<CallToolResult, ErrorData> {
        let timeout = timeout_or_error(args.timeout)?;
        let info = self
            .client
            .find_and_connect(args.address.as_deref(), timeout, args.model.into())
            .await
            .map_err(internal_error)?;
        json_result(&json!({
            "connected": info.connected,
            "address": info.address,
            "model": model_name(info.model),
        }))
    }

    /// Shared status implementation (`tool_status`).
    async fn run_status(&self, args: OptionalSlotArgs) -> Result<CallToolResult, ErrorData> {
        let result = if let Some(slot) = args.slot {
            let notification = self
                .client
                .status(slot_from(slot)?)
                .await
                .map_err(internal_error)?;
            notification_value(&notification)?
        } else {
            let all = self
                .client
                .poll_all_status()
                .await
                .map_err(internal_error)?;
            let mut slots = Vec::with_capacity(all.len());
            for notification in &all {
                slots.push(notification_value(notification)?);
            }
            json!({"slots": slots})
        };
        json_result(&result)
    }

    /// Shared single-slot stop (`tool_stop`); an emergency stop, tokenless.
    async fn run_stop(&self, args: SlotArgs) -> Result<CallToolResult, ErrorData> {
        let slot = slot_from(args.slot)?;
        self.client.stop(slot).await.map_err(internal_error)?;
        json_result(&json!({"sent": "stop", "slot": args.slot}))
    }

    /// Shared voltage-curve read (`tool_get_voltage_curve`).
    async fn run_curve(&self, slot: u8, timeout: f64) -> Result<Notification, ErrorData> {
        let timeout = timeout_or_error(timeout)?;
        self.client
            .curve(slot_from(slot)?, timeout)
            .await
            .map_err(internal_error)
    }
}

#[tool_router(vis = "pub(crate)")]
impl McpService {
    /// Read-only BLE scan (`mc3000_scan`).
    #[tool(name = "mc3000_scan", description = descriptions::MC3000_SCAN)]
    pub async fn mc3000_scan(
        &self,
        Parameters(args): Parameters<ScanArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let timeout = timeout_or_error(args.timeout)?;
        self.run_scan(timeout).await
    }

    /// Connect with an explicit charger model (`mc3000_connect`).
    #[tool(name = "mc3000_connect", description = descriptions::MC3000_CONNECT)]
    pub async fn mc3000_connect(
        &self,
        Parameters(args): Parameters<ConnectArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_connect(args).await
    }

    /// Disconnect (`mc3000_disconnect`).
    #[tool(
        name = "mc3000_disconnect",
        description = descriptions::MC3000_DISCONNECT
    )]
    pub async fn mc3000_disconnect(&self) -> Result<CallToolResult, ErrorData> {
        self.client.disconnect().await.map_err(internal_error)?;
        json_result(&json!({"connected": false}))
    }

    /// Poll slot status (`mc3000_status`).
    #[tool(name = "mc3000_status", description = descriptions::MC3000_STATUS)]
    pub async fn mc3000_status(
        &self,
        Parameters(args): Parameters<OptionalSlotArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_status(args).await
    }

    /// Start a slot with its confirmation token (`mc3000_start`).
    #[tool(name = "mc3000_start", description = descriptions::MC3000_START)]
    pub async fn mc3000_start(
        &self,
        Parameters(args): Parameters<LegacyStartArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let slot = slot_from(args.slot)?;
        let expected = format!("START_SLOT_{}", args.slot);
        require_token(
            args.confirmation_token.as_deref(),
            &expected,
            "starting hardware",
        )?;
        self.client.start(slot).await.map_err(internal_error)?;
        json_result(&json!({"sent": "start", "slot": args.slot}))
    }

    /// Stop a slot; tokenless emergency stop (`mc3000_stop`).
    #[tool(name = "mc3000_stop", description = descriptions::MC3000_STOP)]
    pub async fn mc3000_stop(
        &self,
        Parameters(args): Parameters<SlotArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_stop(args).await
    }

    /// Read firmware/hardware version (`mc3000_get_version`).
    #[tool(
        name = "mc3000_get_version",
        description = descriptions::MC3000_GET_VERSION
    )]
    pub async fn mc3000_get_version(
        &self,
        Parameters(args): Parameters<VersionArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let notification = self
            .client
            .version(args.mac.as_deref())
            .await
            .map_err(internal_error)?;
        json_result(&notification_value(&notification)?)
    }

    /// Read basic settings (`mc3000_get_basic`).
    #[tool(
        name = "mc3000_get_basic",
        description = descriptions::MC3000_GET_BASIC
    )]
    pub async fn mc3000_get_basic(&self) -> Result<CallToolResult, ErrorData> {
        let notification = self.client.basic().await.map_err(internal_error)?;
        json_result(&notification_value(&notification)?)
    }

    /// Write basic settings with the `SET_BASIC` token (`mc3000_set_basic`).
    #[tool(
        name = "mc3000_set_basic",
        description = descriptions::MC3000_SET_BASIC
    )]
    pub async fn mc3000_set_basic(
        &self,
        Parameters(args): Parameters<SetBasicArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        require_token(
            args.confirmation_token.as_deref(),
            SET_BASIC_TOKEN,
            "device settings write",
        )?;
        let input_mv_raw = args.input_mv.unwrap_or_else(default_input_mv);
        let settings = BasicSettings {
            temp_unit: lo8(args.temp_unit.unwrap_or_default()),
            system_beep: args.system_beep.unwrap_or_default(),
            display: lo8(args.display.unwrap_or_default()),
            screensaver: args.screensaver.unwrap_or_default(),
            cooling_fan: lo8(args.cooling_fan.unwrap_or_default()),
            input_mv: u32::try_from(input_mv_raw)
                .map_err(|_| internal_error(ProtocolError::U16OutOfRange(input_mv_raw)))?,
        };
        let notification = self
            .client
            .set_basic(&settings)
            .await
            .map_err(internal_error)?;
        json_result(&notification_value(&notification)?)
    }

    /// Read the voltage curve of one slot (`mc3000_get_voltage_curve`).
    #[tool(
        name = "mc3000_get_voltage_curve",
        description = descriptions::MC3000_GET_VOLTAGE_CURVE
    )]
    pub async fn mc3000_get_voltage_curve(
        &self,
        Parameters(args): Parameters<CurveArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let notification = self.run_curve(args.slot, args.timeout).await?;
        json_result(&notification_value(&notification)?)
    }

    /// Read MC5000-layout slot status (`mc3000_mc5000_status`).
    #[tool(
        name = "mc3000_mc5000_status",
        description = descriptions::MC3000_MC5000_STATUS
    )]
    pub async fn mc3000_mc5000_status(
        &self,
        Parameters(args): Parameters<Mc5000StatusArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let notification = self
            .client
            .mc5000_status(slot_from(args.slot)?)
            .await
            .map_err(internal_error)?;
        json_result(&notification_value(&notification)?)
    }

    /// Build a 40-byte profile frame without sending it
    /// (`mc3000_build_profile`).
    #[tool(
        name = "mc3000_build_profile",
        description = descriptions::MC3000_BUILD_PROFILE
    )]
    pub async fn mc3000_build_profile(
        &self,
        Parameters(args): Parameters<BuildProfileArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        reject_unknown(&args.extra)?;
        let frame = build_profile(&args.profile.params()?).map_err(internal_error)?;
        profile_hex_result(&frame)
    }

    /// Apply a profile with dry-run-by-default hardening
    /// (`mc3000_apply_profile`).
    #[tool(
        name = "mc3000_apply_profile",
        description = descriptions::MC3000_APPLY_PROFILE
    )]
    pub async fn mc3000_apply_profile(
        &self,
        Parameters(args): Parameters<ApplyProfileArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        if args.start == Some(true) {
            return Err(internal_error(
                "mc3000_apply_profile does not start slots; apply the profile, then start it \
                 with mc3000_start",
            ));
        }
        let dry_run = args.dry_run.unwrap_or(true);
        let profile = if let Some(profile_hex) = &args.profile_hex {
            bytes_from_hex(profile_hex).map_err(internal_error)?
        } else {
            reject_unknown(&args.extra)?;
            build_profile(&args.profile.params()?)
                .map_err(internal_error)?
                .to_vec()
        };
        if profile.len() != protocol::PROFILE_LEN {
            return Err(internal_error(ProtocolError::InvalidProfileLength));
        }
        // Hardening: a live write targets exactly one slot, named by its token.
        let mask = profile[2];
        let selected = mask.count_ones();
        if mask == 0 || selected != 1 {
            return Err(internal_error(format!(
                "mc3000_apply_profile supports exactly one selected slot; slot_mask {mask} \
                 selects {selected} slots"
            )));
        }
        let slot_index = mask.trailing_zeros();
        let (first, second) = split_profile(&profile).map_err(internal_error)?;
        if dry_run {
            return json_result(&json!({
                "dry_run": true,
                "applied": false,
                "profile_hex": hex::encode_upper(&profile),
                "chunk_1_hex": hex::encode_upper(first),
                "chunk_2_hex": hex::encode_upper(second),
            }));
        }
        let expected = format!("APPLY_PROFILE_SLOT_{slot_index}");
        require_token(
            args.confirmation_token.as_deref(),
            &expected,
            "dangerous write",
        )?;
        let report = self
            .client
            .send_profile(&profile, false)
            .await
            .map_err(internal_error)?;
        let mut result = serde_json::to_value(&report).map_err(internal_error)?;
        if let Some(object) = result.as_object_mut() {
            object.insert("dry_run".to_owned(), json!(false));
            object.insert("applied".to_owned(), json!(true));
        }
        json_result(&result)
    }

    /// Send a raw frame with the `SEND_RAW` token (`mc3000_send_raw`).
    #[tool(
        name = "mc3000_send_raw",
        description = descriptions::MC3000_SEND_RAW
    )]
    pub async fn mc3000_send_raw(
        &self,
        Parameters(args): Parameters<SendRawArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        if args.start == Some(true) {
            return Err(internal_error(
                "mc3000_send_raw no longer interprets profiles or starts slots; use \
                 mc3000_apply_profile and mc3000_start",
            ));
        }
        require_token(
            args.confirmation_token.as_deref(),
            SEND_RAW_TOKEN,
            "raw write",
        )?;
        let hex_string = args
            .hex
            .clone()
            .or(args.frame_hex)
            .ok_or_else(|| internal_error("mc3000_send_raw requires 'hex' or 'frame_hex'"))?;
        let data = bytes_from_hex(&hex_string).map_err(internal_error)?;
        if data.len() >= protocol::MIN_OPCODE_FRAME_LEN
            && matches!(
                data[1],
                opcode if opcode == Opcode::RestoreFactory.as_u8()
                    || opcode == Opcode::RestoreCalibration.as_u8()
            )
        {
            return Err(internal_error(
                "restore factory/calibration opcodes are blocked",
            ));
        }
        self.client.write(&data).await.map_err(internal_error)?;
        if args.wait.unwrap_or_default() {
            let timeout = timeout_or_error(args.timeout)?;
            let notification = self
                .client
                .wait_for(None, timeout)
                .await
                .map_err(internal_error)?;
            json_result(&notification_value(&notification)?)
        } else {
            json_result(&json!({"sent_hex": hex::encode_upper(&data)}))
        }
    }

    /// Read-only BLE scan (`charger.scan_devices`).
    #[tool(
        name = "charger.scan_devices",
        description = descriptions::CHARGER_SCAN_DEVICES
    )]
    pub async fn charger_scan_devices(
        &self,
        Parameters(args): Parameters<ScanArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let timeout = timeout_or_error(args.timeout)?;
        self.run_scan(timeout).await
    }

    /// Connect to a selected charger (`charger.connect`).
    #[tool(name = "charger.connect", description = descriptions::CHARGER_CONNECT)]
    pub async fn charger_connect(
        &self,
        Parameters(args): Parameters<ConnectArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_connect(args).await
    }

    /// Read all slot statuses (`charger.get_status`).
    #[tool(
        name = "charger.get_status",
        description = descriptions::CHARGER_GET_STATUS
    )]
    pub async fn charger_get_status(&self) -> Result<CallToolResult, ErrorData> {
        self.run_status(OptionalSlotArgs::default()).await
    }

    /// Read one or all slot statuses (`charger.read_slots`).
    #[tool(
        name = "charger.read_slots",
        description = descriptions::CHARGER_READ_SLOTS
    )]
    pub async fn charger_read_slots(
        &self,
        Parameters(args): Parameters<OptionalSlotArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_status(args).await
    }

    /// Read the app-style voltage time-series (`charger.get_voltage_curve`).
    #[tool(
        name = "charger.get_voltage_curve",
        description = descriptions::CHARGER_GET_VOLTAGE_CURVE
    )]
    pub async fn charger_get_voltage_curve(
        &self,
        Parameters(args): Parameters<CurveArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let notification = self.run_curve(args.slot, args.timeout).await?;
        let mut value = notification_value(&notification)?;
        if value.get("points_mv").is_some()
            && let Some(object) = value.as_object_mut()
        {
            object
                .entry("kind".to_owned())
                .or_insert_with(|| json!("voltage_curve"));
            object.insert(
                "source".to_owned(),
                json!("charger_voltage_curve_command_0x56"),
            );
        }
        json_result(&value)
    }

    /// Export the voltage curve as JSON or CSV (`charger.export_voltage_curve`).
    #[tool(
        name = "charger.export_voltage_curve",
        description = descriptions::CHARGER_EXPORT_VOLTAGE_CURVE
    )]
    pub async fn charger_export_voltage_curve(
        &self,
        Parameters(args): Parameters<CurveExportArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let curve = curve_from(self.run_curve(args.slot, args.timeout).await?)?;
        let mut charger_curve = serde_json::to_value(&curve).map_err(internal_error)?;
        if let Some(object) = charger_curve.as_object_mut() {
            object.insert("kind".to_owned(), json!("voltage_curve"));
            object.insert(
                "source".to_owned(),
                json!("charger_voltage_curve_command_0x56"),
            );
        }
        match args
            .format
            .as_deref()
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("csv") => json_result(&json!({
                "slot": curve.slot,
                "format": "csv",
                "content": voltage_curve_to_csv(&curve),
            })),
            Some("json") | None => json_result(&json!({
                "slot": curve.slot,
                "format": "json",
                "content": charger_curve,
            })),
            Some(other) => Err(internal_error(format!(
                "format must be 'json' or 'csv' (got '{other}')"
            ))),
        }
    }

    /// List bundled safe example profiles (`charger.list_profiles`).
    #[tool(
        name = "charger.list_profiles",
        description = descriptions::CHARGER_LIST_PROFILES
    )]
    pub async fn charger_list_profiles(&self) -> Result<CallToolResult, ErrorData> {
        json_result(&json!({"profiles": safety::profiles()}))
    }

    /// Validate a safety profile (`charger.validate_profile`).
    #[tool(
        name = "charger.validate_profile",
        description = descriptions::CHARGER_VALIDATE_PROFILE
    )]
    pub async fn charger_validate_profile(
        &self,
        Parameters(args): Parameters<ValidateProfileArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let profile = profile_from_args(
            args.profile.as_ref(),
            args.profile_id.as_deref(),
            &args.extra,
        )
        .value();
        let validation = safety::validate_profile(&profile);
        let result = serde_json::to_value(&validation).map_err(internal_error)?;
        self.session
            .record("charger.validate_profile", &args_value(&args)?, &result);
        json_result(&result)
    }

    /// Validate and apply a profile with dry-run default
    /// (`charger.apply_profile`).
    #[tool(
        name = "charger.apply_profile",
        description = descriptions::CHARGER_APPLY_PROFILE
    )]
    pub async fn charger_apply_profile(
        &self,
        Parameters(args): Parameters<ChargerApplyArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let dry_run = args.dry_run.unwrap_or(true);
        let slot = slot_from(args.slot)?;
        let logged_args = args_value(&args)?;
        let profile = profile_from_args(
            args.profile.as_ref(),
            args.profile_id.as_deref(),
            &args.extra,
        )
        .value();
        let validation = safety::validate_profile(&profile);
        if !validation.ok {
            let result = json!({
                "dry_run": dry_run,
                "applied": false,
                "validation": serde_json::to_value(&validation).map_err(internal_error)?,
            });
            self.session
                .record("charger.apply_profile", &logged_args, &result);
            return json_result(&result);
        }
        let params = safety::profile_to_mc3000_args(&profile, slot).map_err(internal_error)?;
        let frame = build_profile(&params).map_err(internal_error)?;
        let (first, second) = split_profile(&frame).map_err(internal_error)?;
        if dry_run {
            let result = json!({
                "dry_run": true,
                "applied": false,
                "validation": serde_json::to_value(&validation).map_err(internal_error)?,
                "profile_hex": hex::encode_upper(frame),
                "chunk_1_hex": hex::encode_upper(first),
                "chunk_2_hex": hex::encode_upper(second),
            });
            self.session
                .record("charger.apply_profile", &logged_args, &result);
            return json_result(&result);
        }
        let expected = format!("APPLY_PROFILE_SLOT_{}", args.slot);
        require_token(
            args.confirmation_token.as_deref(),
            &expected,
            "dangerous write",
        )?;
        let report = self
            .client
            .send_profile(&frame, false)
            .await
            .map_err(internal_error)?;
        let mut result = serde_json::to_value(&report).map_err(internal_error)?;
        if let Some(object) = result.as_object_mut() {
            object.insert("dry_run".to_owned(), json!(false));
            object.insert("applied".to_owned(), json!(true));
            object.insert(
                "validation".to_owned(),
                serde_json::to_value(&validation).map_err(internal_error)?,
            );
        }
        self.session
            .record("charger.apply_profile", &logged_args, &result);
        json_result(&result)
    }

    /// Start the applied operation with its token (`charger.start`).
    #[tool(name = "charger.start", description = descriptions::CHARGER_START)]
    pub async fn charger_start(
        &self,
        Parameters(args): Parameters<ChargerStartArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let slot = slot_from(args.slot)?;
        let logged_args = args_value(&args)?;
        let expected = format!("START_SLOT_{}", args.slot);
        require_token(
            Some(&args.confirmation_token),
            &expected,
            "starting hardware",
        )?;
        self.client.start(slot).await.map_err(internal_error)?;
        let result = json!({"sent": "start", "slot": args.slot});
        self.session.record("charger.start", &logged_args, &result);
        json_result(&result)
    }

    /// Tokenless emergency stop of one slot (`charger.stop_slot`).
    #[tool(
        name = "charger.stop_slot",
        description = descriptions::CHARGER_STOP_SLOT
    )]
    pub async fn charger_stop_slot(
        &self,
        Parameters(args): Parameters<SlotArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_stop(args).await
    }

    /// Tokenless emergency stop of all slots (`charger.stop_all`).
    ///
    /// Each stop waits for the `0xFE` acknowledgement (500 ms timeout,
    /// best-effort: a timeout logs and continues to the next slot). The ACK
    /// wait paces the writes — firmware 1.23 dropped stop frames sent
    /// back-to-back — and confirms each stop reached the charger.
    #[tool(name = "charger.stop_all", description = descriptions::CHARGER_STOP_ALL)]
    pub async fn charger_stop_all(&self) -> Result<CallToolResult, ErrorData> {
        let mut stopped = Vec::new();
        for value in 0..=3u8 {
            let slot = slot_from(value)?;
            if let Err(error) = self.client.stop_acked(slot).await {
                // Only acknowledgement timeouts are best-effort; model gates
                // and transport failures abort the emergency stop.
                if matches!(
                    error,
                    ChargerError::Transport(TransportError::ReplyTimeout(_))
                ) {
                    tracing::warn!(slot = value, %error, "stop_all ack timeout; continuing");
                } else {
                    return Err(internal_error(error));
                }
            }
            stopped.push(value);
        }
        let result = json!({"sent": "stop_all", "slots": stopped});
        self.session.record("charger.stop_all", &json!({}), &result);
        json_result(&result)
    }

    /// Export the bounded session log (`charger.export_session_log`).
    #[tool(
        name = "charger.export_session_log",
        description = descriptions::CHARGER_EXPORT_SESSION_LOG
    )]
    pub async fn charger_export_session_log(&self) -> Result<CallToolResult, ErrorData> {
        json_result(&json!({"events": self.session.events()}))
    }
}

/// Build and report the `mc3000_build_profile` payload.
fn profile_hex_result(frame: &[u8; protocol::PROFILE_LEN]) -> Result<CallToolResult, ErrorData> {
    let (first, second) = split_profile(frame).map_err(internal_error)?;
    json_result(&json!({
        "profile_hex": hex::encode_upper(frame),
        "chunk_1_hex": hex::encode_upper(first),
        "chunk_2_hex": hex::encode_upper(second),
    }))
}
