//! Enforced battery-safety limits, profile validation, and the fail-closed
//! profile-to-MC3000 conversion (`safety.py`).
//!
//! Validation reproduces the Python error/warning strings and their order
//! exactly. Chemistries without enforced limits (`LiIo4.35`, `NiZn`, `RAM`, `LTO`)
//! fail closed: any profile using them is rejected before a backend call can
//! happen. All Python exceptions become [`ProtocolError`].

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::model::{Chemistry, Operation, Slot, SlotMask};
use crate::profile::{CycleMode, ProfileParams};
use crate::{ProtocolError, lo8, py_int};

/// Hard upper bound for `timeout_minutes` (`safety.TIMEOUT_MINUTES_MAX`).
pub const TIMEOUT_MINUTES_MAX: i64 = 1440;

/// Required profile fields in sorted order (the order the Python implementation
/// reports them in the `missing required fields` error).
pub const REQUIRED_PROFILE_FIELDS: [&str; 11] = [
    "capacity_mah",
    "cell_count",
    "charge_current_ma",
    "chemistry",
    "discharge_current_ma",
    "discharge_cutoff_mv",
    "operation",
    "profile_id",
    "risk_level",
    "source",
    "temperature_cutoff_c",
];

/// Enforced limits for one chemistry (`safety.SAFETY_LIMITS` entries).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChemistryLimits {
    /// Nominal voltage of one cell in millivolts.
    pub cell_nominal_mv: i64,
    /// Minimum supported series cell count.
    pub cell_count_min: i64,
    /// Maximum supported series cell count.
    pub cell_count_max: i64,
    /// Minimum charge current in milliamps.
    pub charge_current_ma_min: i64,
    /// Maximum charge current as a C-rate of the rated capacity.
    pub charge_current_c_max: f64,
    /// Minimum charge cutoff voltage in millivolts.
    pub charge_voltage_mv_min: i64,
    /// Maximum charge cutoff voltage in millivolts.
    pub charge_voltage_mv_max: i64,
    /// Minimum discharge current in milliamps.
    pub discharge_current_ma_min: i64,
    /// Maximum discharge current in milliamps.
    pub discharge_current_ma_max: i64,
    /// Minimum discharge cutoff voltage in millivolts.
    pub discharge_cutoff_mv_min: i64,
    /// Maximum discharge cutoff voltage in millivolts.
    pub discharge_cutoff_mv_max: i64,
    /// Minimum storage voltage in millivolts (Li chemistries only).
    pub storage_voltage_mv_min: Option<i64>,
    /// Maximum storage voltage in millivolts (Li chemistries only).
    pub storage_voltage_mv_max: Option<i64>,
    /// Minimum temperature cutoff in degrees Celsius.
    pub temperature_cutoff_c_min: i64,
    /// Maximum temperature cutoff in degrees Celsius.
    pub temperature_cutoff_c_max: i64,
    /// Operations this chemistry may run.
    pub operations: &'static [Operation],
}

/// Limits shared by the nickel chemistries (`NiMH`, `Eneloop`, `NiCd`).
const NICKEL_LIMITS: ChemistryLimits = ChemistryLimits {
    cell_nominal_mv: 1200,
    cell_count_min: 1,
    cell_count_max: 1,
    charge_current_ma_min: 50,
    charge_current_c_max: 1.0,
    charge_voltage_mv_min: 1400,
    charge_voltage_mv_max: 1800,
    discharge_current_ma_min: 50,
    discharge_current_ma_max: 2000,
    discharge_cutoff_mv_min: 900,
    discharge_cutoff_mv_max: 1100,
    storage_voltage_mv_min: None,
    storage_voltage_mv_max: None,
    temperature_cutoff_c_min: 20,
    temperature_cutoff_c_max: 60,
    operations: &[
        Operation::Charge,
        Operation::Refresh,
        Operation::BreakIn,
        Operation::Discharge,
        Operation::Cycle,
    ],
};

/// Limits for `LiIon`.
const LIION_LIMITS: ChemistryLimits = ChemistryLimits {
    cell_nominal_mv: 3700,
    cell_count_min: 1,
    cell_count_max: 1,
    charge_current_ma_min: 50,
    charge_current_c_max: 1.0,
    charge_voltage_mv_min: 4100,
    charge_voltage_mv_max: 4200,
    discharge_current_ma_min: 50,
    discharge_current_ma_max: 2000,
    discharge_cutoff_mv_min: 2800,
    discharge_cutoff_mv_max: 3300,
    storage_voltage_mv_min: Some(3700),
    storage_voltage_mv_max: Some(3900),
    temperature_cutoff_c_min: 20,
    temperature_cutoff_c_max: 55,
    operations: &[Operation::Charge, Operation::Storage, Operation::Discharge],
};

/// Limits for `LiFe`.
const LIFE_LIMITS: ChemistryLimits = ChemistryLimits {
    cell_nominal_mv: 3200,
    cell_count_min: 1,
    cell_count_max: 1,
    charge_current_ma_min: 50,
    charge_current_c_max: 1.0,
    charge_voltage_mv_min: 3500,
    charge_voltage_mv_max: 3600,
    discharge_current_ma_min: 50,
    discharge_current_ma_max: 2000,
    discharge_cutoff_mv_min: 2400,
    discharge_cutoff_mv_max: 2900,
    storage_voltage_mv_min: Some(3200),
    storage_voltage_mv_max: Some(3400),
    temperature_cutoff_c_min: 20,
    temperature_cutoff_c_max: 55,
    operations: &[Operation::Charge, Operation::Storage, Operation::Discharge],
};

/// `safety.CHEMISTRY_CODES[name]`: protocol battery-type code for a chemistry name.
#[must_use]
pub fn chemistry_code(name: &str) -> Option<u8> {
    Chemistry::from_name(name).map(Chemistry::code)
}

/// `safety.OPERATION_MODES[name]`: charger mode code for an operation name.
#[must_use]
pub fn operation_mode(name: &str) -> Option<u8> {
    Operation::from_name(name).map(Operation::mode_code)
}

/// Enforced limits for a chemistry; `None` means the chemistry fails closed.
#[must_use]
pub const fn limits_for(chemistry: Chemistry) -> Option<&'static ChemistryLimits> {
    match chemistry {
        Chemistry::Nimh | Chemistry::Eneloop | Chemistry::Nicd => Some(&NICKEL_LIMITS),
        Chemistry::LiIon => Some(&LIION_LIMITS),
        Chemistry::LiFe => Some(&LIFE_LIMITS),
        Chemistry::LiIo435 | Chemistry::NiZn | Chemistry::Ram | Chemistry::Lto => None,
    }
}

/// Consumer-visible validation result (`safety.ValidationResult`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ValidationResult {
    /// Whether the profile passed every enforced check.
    pub ok: bool,
    /// Blocking errors, in the Python emission order.
    pub errors: Vec<String>,
    /// Non-blocking warnings.
    pub warnings: Vec<String>,
    /// Verbatim copy of the validated profile.
    pub normalized_profile: Value,
}

/// The charger-profile JSON schema (`safety.PROFILE_SCHEMA`).
#[must_use]
pub fn profile_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://github.com/nietonchique/mc3000-mcp/schema/charger-profile.json",
        "title": "Charger Battery Profile",
        "type": "object",
        "additionalProperties": false,
        "required": REQUIRED_PROFILE_FIELDS,
        "properties": {
            "profile_id": {"type": "string", "pattern": "^[a-z0-9][a-z0-9_.-]{2,80}$"},
            "chemistry": {"type": "string", "enum": sorted_chemistry_names()},
            "capacity_mah": {"type": "integer", "minimum": 1, "maximum": 50000},
            "cell_count": {"type": "integer", "minimum": 1, "maximum": 12},
            "nominal_voltage_mv": {"type": "integer", "minimum": 500, "maximum": 60000},
            "operation": {"type": "string", "enum": sorted_operation_names()},
            "charge_current_ma": {"type": "integer", "minimum": 0, "maximum": 10000},
            "discharge_current_ma": {"type": "integer", "minimum": 0, "maximum": 10000},
            "charge_voltage_mv": {"type": "integer", "minimum": 0, "maximum": 60000},
            "discharge_cutoff_mv": {"type": "integer", "minimum": 0, "maximum": 60000},
            "timeout_minutes": {"type": "integer", "minimum": 0, "maximum": 1440},
            "temperature_cutoff_c": {"type": "integer", "minimum": 0, "maximum": 80},
            "temperature_policy": {"type": "string", "minLength": 3},
            "notes": {"type": "string"},
            "source": {"type": "string", "minLength": 3},
            "risk_level": {"type": "string", "enum": ["low", "medium", "high"]},
            "cycle_count": {"type": "integer", "minimum": 1, "maximum": 99},
            "cycle_mode": {"type": "string", "enum": ["C>D", "C>D>C", "D>C", "D>C>D"]},
            "negative_delta_mv": {"type": "integer", "minimum": 0, "maximum": 20},
            "trickle_current_ma": {"type": "integer", "minimum": 0, "maximum": 300}
        }
    })
}

/// The bundled profiles as a JSON object keyed by `profile_id`
/// (`safety.profiles()`); every call returns fresh copies.
#[must_use]
pub fn profiles() -> Map<String, Value> {
    let mut out = Map::new();
    for profile in crate::profile::bundled_profiles() {
        let value = match serde_json::to_value(&profile) {
            Ok(value) => value,
            // Serialization of this plain-data struct cannot fail; the fallback
            // keeps the function total without panicking.
            Err(_) => Value::Null,
        };
        out.insert(profile.profile_id.clone(), value);
    }
    out
}

/// The enforced safety limits as JSON (`safety.safety_limits()`).
#[must_use]
pub fn safety_limits() -> Value {
    json!({
        "NiMH": nickel_limits_json(),
        "Eneloop": nickel_limits_json(),
        "NiCd": nickel_limits_json(),
        "LiIon": liion_limits_json(),
        "LiFe": liion_limits_json(),
    })
}

fn nickel_limits_json() -> Value {
    json!({
        "cell_nominal_mv": 1200,
        "cell_count_min": 1,
        "cell_count_max": 1,
        "charge_current_ma_min": 50,
        "charge_current_c_max": 1.0,
        "charge_voltage_mv_min": 1400,
        "charge_voltage_mv_max": 1800,
        "discharge_current_ma_min": 50,
        "discharge_current_ma_max": 2000,
        "discharge_cutoff_mv_min": 900,
        "discharge_cutoff_mv_max": 1100,
        "temperature_cutoff_c_min": 20,
        "temperature_cutoff_c_max": 60,
        "operations": ["charge", "refresh", "break_in", "discharge", "cycle"],
    })
}

fn liion_limits_json() -> Value {
    json!({
        "cell_nominal_mv": 3700,
        "cell_count_min": 1,
        "cell_count_max": 1,
        "charge_current_ma_min": 50,
        "charge_current_c_max": 1.0,
        "charge_voltage_mv_min": 4100,
        "charge_voltage_mv_max": 4200,
        "discharge_current_ma_min": 50,
        "discharge_current_ma_max": 2000,
        "discharge_cutoff_mv_min": 2800,
        "discharge_cutoff_mv_max": 3300,
        "storage_voltage_mv_min": 3700,
        "storage_voltage_mv_max": 3900,
        "temperature_cutoff_c_min": 20,
        "temperature_cutoff_c_max": 55,
        "operations": ["charge", "storage", "discharge"],
    })
}

fn sorted_chemistry_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = Chemistry::ALL.into_iter().map(Chemistry::name).collect();
    names.sort_unstable();
    names
}

fn sorted_operation_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = Operation::ALL.into_iter().map(Operation::name).collect();
    names.sort_unstable();
    names
}

/// Integer fields extracted from the raw profile (`safety._as_int` results).
struct CheckedScalars {
    capacity: Option<i64>,
    cell_count: Option<i64>,
    nominal_mv: Option<i64>,
    charge_current: Option<i64>,
    discharge_current: Option<i64>,
    charge_voltage: Option<i64>,
    discharge_cutoff: Option<i64>,
    temp_cutoff: Option<i64>,
    timeout: Option<i64>,
}

/// Extract the integer fields in the Python emission order.
fn extract_scalars(profile: &Value, errors: &mut Vec<String>) -> CheckedScalars {
    CheckedScalars {
        capacity: as_int(profile, "capacity_mah", errors, true),
        cell_count: as_int(profile, "cell_count", errors, true),
        nominal_mv: as_int(profile, "nominal_voltage_mv", errors, false),
        charge_current: as_int(profile, "charge_current_ma", errors, true),
        discharge_current: as_int(profile, "discharge_current_ma", errors, true),
        charge_voltage: as_int(profile, "charge_voltage_mv", errors, false),
        discharge_cutoff: as_int(profile, "discharge_cutoff_mv", errors, true),
        temp_cutoff: as_int(profile, "temperature_cutoff_c", errors, true),
        timeout: as_int(profile, "timeout_minutes", errors, false),
    }
}

/// Run the value range checks in the Python emission order.
fn check_scalars(
    scalars: &CheckedScalars,
    limits: &ChemistryLimits,
    operation: Option<Operation>,
    operation_supported: bool,
    chemistry_name: &str,
    errors: &mut Vec<String>,
) {
    if scalars.capacity.is_some_and(|value| value <= 0) {
        errors.push("capacity_mah must be positive".to_string());
    }
    if let Some(cells) = scalars.cell_count {
        check_range(
            "cell_count",
            cells,
            limits.cell_count_min,
            limits.cell_count_max,
            errors,
        );
    }
    if let Some(nominal) = scalars.nominal_mv {
        let cells_for_nominal = scalars.cell_count.filter(|&cells| cells != 0).unwrap_or(1);
        let expected = limits.cell_nominal_mv * cells_for_nominal;
        let tolerance = std::cmp::max(150, expected / 10);
        if (nominal - expected).abs() > tolerance {
            let cell_count_label = scalars
                .cell_count
                .map_or_else(|| "None".to_string(), |cells| cells.to_string());
            errors.push(format!(
                "nominal_voltage_mv {nominal} does not match {chemistry_name} x {cell_count_label}"
            ));
        }
    }
    if let (Some(charge), Some(capacity_value)) = (scalars.charge_current, scalars.capacity) {
        let max_charge = py_int(limits.charge_current_c_max * i64_to_f64(capacity_value));
        check_range(
            "charge_current_ma",
            charge,
            limits.charge_current_ma_min,
            max_charge,
            errors,
        );
    }
    if let Some(discharge) = scalars.discharge_current {
        check_range(
            "discharge_current_ma",
            discharge,
            limits.discharge_current_ma_min,
            limits.discharge_current_ma_max,
            errors,
        );
    }
    if let Some(charge_voltage_value) = scalars.charge_voltage {
        // Python guards with `operation in limits["operations"]` first, then checks
        // the voltage only for charge/refresh/cycle/storage operations.
        let storage_operation = matches!(operation, Some(Operation::Storage));
        if operation_supported
            && matches!(
                operation,
                Some(
                    Operation::Charge | Operation::Refresh | Operation::Cycle | Operation::Storage
                )
            )
        {
            let (min, max) = if storage_operation {
                (limits.storage_voltage_mv_min, limits.storage_voltage_mv_max)
            } else {
                (
                    Some(limits.charge_voltage_mv_min),
                    Some(limits.charge_voltage_mv_max),
                )
            };
            if let (Some(min), Some(max)) = (min, max) {
                check_range("charge_voltage_mv", charge_voltage_value, min, max, errors);
            }
        }
    }
    if let Some(cutoff) = scalars.discharge_cutoff {
        check_range(
            "discharge_cutoff_mv",
            cutoff,
            limits.discharge_cutoff_mv_min,
            limits.discharge_cutoff_mv_max,
            errors,
        );
    }
    if let Some(temp) = scalars.temp_cutoff {
        check_range(
            "temperature_cutoff_c",
            temp,
            limits.temperature_cutoff_c_min,
            limits.temperature_cutoff_c_max,
            errors,
        );
    }
    if scalars
        .timeout
        .is_some_and(|value| !(0..=TIMEOUT_MINUTES_MAX).contains(&value))
    {
        errors.push("timeout_minutes must be 0..1440".to_string());
    }
}

/// Validate a profile against the enforced safety limits
/// (`safety.validate_profile`).
///
/// The input is the raw JSON profile object, so missing fields, wrong types, and
/// unknown chemistries are reported exactly like the Python implementation.
#[must_use]
pub fn validate_profile(profile: &Value) -> ValidationResult {
    let normalized = profile.clone();
    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    let missing: Vec<&str> = REQUIRED_PROFILE_FIELDS
        .iter()
        .copied()
        .filter(|field| profile.get(*field).is_none())
        .collect();
    if !missing.is_empty() {
        errors.push(format!("missing required fields: {}", missing.join(", ")));
    }

    let profile_id = py_str(profile.get("profile_id"));
    if !profile_id.is_empty() && !is_valid_profile_id(&profile_id) {
        errors.push("profile_id must match ^[a-z0-9][a-z0-9_.-]{2,80}$".to_string());
    }

    let chemistry_name = py_str(profile.get("chemistry"));
    let Some(chemistry) = Chemistry::from_name(&chemistry_name) else {
        errors.push(format!(
            "unsupported chemistry: {}",
            if chemistry_name.is_empty() {
                "<missing>"
            } else {
                chemistry_name.as_str()
            }
        ));
        return ValidationResult {
            ok: false,
            errors,
            warnings,
            normalized_profile: normalized,
        };
    };

    let Some(limits) = limits_for(chemistry) else {
        errors.push(format!(
            "no enforced safety limits for chemistry: {chemistry_name}"
        ));
        return ValidationResult {
            ok: false,
            errors,
            warnings,
            normalized_profile: normalized,
        };
    };

    let operation_name = py_str(profile.get("operation"));
    let operation = Operation::from_name(&operation_name);
    let operation_supported = operation.is_some_and(|op| limits.operations.contains(&op));
    if !operation_supported {
        errors.push(format!(
            "operation {} is not supported for {chemistry_name}",
            if operation_name.is_empty() {
                "<missing>"
            } else {
                operation_name.as_str()
            }
        ));
    }

    let scalars = extract_scalars(profile, &mut errors);
    check_scalars(
        &scalars,
        limits,
        operation,
        operation_supported,
        &chemistry_name,
        &mut errors,
    );

    if py_str(profile.get("risk_level")) == "high" {
        warnings.push("high-risk profile: require extra human review and supervision".to_string());
    }

    ValidationResult {
        ok: errors.is_empty(),
        errors,
        warnings,
        normalized_profile: normalized,
    }
}

/// Convert a validated safety profile into MC3000 profile arguments
/// (`safety.profile_to_mc3000_args`).
///
/// # Errors
///
/// [`ProtocolError::InvalidProfile`] when validation fails; the message lists every
/// error joined by `"; "`.
/// [`ProtocolError::UnknownCycleMode`] when `cycle_mode` is present but not a valid
/// cycle-mode label or numeric code.
pub fn profile_to_mc3000_args(profile: &Value, slot: Slot) -> Result<ProfileParams, ProtocolError> {
    let validation = validate_profile(profile);
    if !validation.ok {
        return Err(ProtocolError::InvalidProfile(validation.errors.join("; ")));
    }
    let normalized = &validation.normalized_profile;
    let operation_name = py_str(normalized.get("operation"));
    let operation = Operation::from_name(&operation_name).ok_or_else(|| {
        ProtocolError::InvalidProfile(format!("unsupported operation: {operation_name}"))
    })?;
    let chemistry = Chemistry::from_name(&py_str(normalized.get("chemistry")))
        .ok_or_else(|| ProtocolError::InvalidProfile("unsupported chemistry".to_string()))?;

    // Python: int(normalized.get("charge_voltage_mv", 0) or 0) or None
    let charge_stop_voltage_mv = normalized
        .get("charge_voltage_mv")
        .and_then(json_to_int)
        .filter(|&value| value != 0);

    // Python: int(normalized.get("cycle_count", 1) or 1)
    let cycle_count = normalized
        .get("cycle_count")
        .and_then(json_to_int)
        .filter(|&value| value != 0)
        .unwrap_or(1);

    // Python: normalized.get("cycle_mode", "D>C")
    let cycle_mode = match normalized.get("cycle_mode") {
        None => CycleMode::DischargeToCharge,
        Some(Value::String(name)) => CycleMode::from_name(name)?,
        Some(Value::Number(number)) => {
            let raw = number.as_i64().unwrap_or(0);
            CycleMode::Code(lo8(raw))
        }
        Some(other) => {
            return Err(ProtocolError::UnknownCycleMode(py_str(Some(other))));
        }
    };

    // Python: int(normalized.get("negative_delta_mv", 3) or 0): missing -> 3,
    // present (even zero/null) -> the value, or 0 when falsy.
    let negative_delta_mv = match normalized.get("negative_delta_mv") {
        None => 3,
        Some(_) => or_zero_int(normalized, "negative_delta_mv").unwrap_or(0),
    };

    Ok(ProfileParams {
        slot_mask: SlotMask::from_slot(slot),
        battery_type: chemistry.code(),
        mode: operation.mode_code(),
        capacity_mah: required_int(normalized, "capacity_mah"),
        charge_current_ma: Some(required_int(normalized, "charge_current_ma")),
        discharge_current_ma: Some(required_int(normalized, "discharge_current_ma")),
        charge_stop_voltage_mv,
        discharge_stop_voltage_mv: Some(required_int(normalized, "discharge_cutoff_mv")),
        discharge_stop_current_ma: Some(required_int(normalized, "discharge_current_ma")),
        charge_stop_current_ma: None,
        charge_rest_minutes: None,
        discharge_rest_minutes: None,
        cycle_count: Some(cycle_count),
        cycle_mode,
        negative_delta_mv: Some(negative_delta_mv),
        // Python: int(normalized.get("trickle_current_ma", 0) or 0)
        trickle_current_ma: Some(or_zero_int(normalized, "trickle_current_ma").unwrap_or(0)),
        keep_voltage_mv: None,
        temp_cutoff: required_int(normalized, "temperature_cutoff_c"),
        // Python: int(normalized.get("timeout_minutes", 0) or 0)
        time_limit_minutes: Some(or_zero_int(normalized, "timeout_minutes").unwrap_or(0)),
        breakin: operation == Operation::BreakIn,
    })
}

/// Value of a validated required integer field; validation already accepted it, so
/// an unparsable value here is impossible and falls back to 0 without panicking.
fn required_int(normalized: &Value, key: &str) -> i64 {
    normalized.get(key).and_then(json_to_int).unwrap_or(0)
}

/// Python `int(normalized.get(key, default) or 0)`: `Some` only for truthy
/// (non-zero) values.
fn or_zero_int(normalized: &Value, key: &str) -> Option<i64> {
    normalized
        .get(key)
        .and_then(json_to_int)
        .filter(|&value| value != 0)
}

/// Python `str(normalized.get(key, ""))` over JSON values.
fn py_str(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Bool(true)) => "True".to_string(),
        Some(Value::Bool(false)) => "False".to_string(),
        Some(Value::Number(number)) => number.to_string(),
        Some(other) => serde_json::to_string(other).unwrap_or_else(|_| "{}".to_string()),
    }
}

/// Python `int(value)` over JSON values; `None` reproduces the Python failure
/// cases (`TypeError`/`ValueError`).
fn json_to_int(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64().or_else(|| number.as_f64().map(py_int)),
        Value::Bool(flag) => Some(i64::from(*flag)),
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// Python `_as_int`: required/optional integer extraction with the exact error
/// messages.
fn as_int(profile: &Value, key: &str, errors: &mut Vec<String>, required: bool) -> Option<i64> {
    let value = profile.get(key);
    if value.is_none_or(Value::is_null) {
        if required {
            errors.push(format!("{key} is required"));
        }
        return None;
    }
    let parsed = value.and_then(json_to_int);
    if parsed.is_none() {
        errors.push(format!("{key} must be an integer"));
    }
    parsed
}

/// Python `_check_range` with the exact error message.
fn check_range(name: &str, value: i64, minimum: i64, maximum: i64, errors: &mut Vec<String>) {
    if !(minimum..=maximum).contains(&value) {
        errors.push(format!(
            "{name}={value} outside enforced range {minimum}..{maximum}"
        ));
    }
}

/// `re.fullmatch(r"[a-z0-9][a-z0-9_.-]{2,80}", profile_id)` equivalent.
fn is_valid_profile_id(profile_id: &str) -> bool {
    let bytes = profile_id.as_bytes();
    if bytes.len() < 3 || bytes.len() > 81 {
        return false;
    }
    let is_start = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    let is_rest = |byte: u8| is_start(byte) || matches!(byte, b'_' | b'.' | b'-');
    is_start(bytes[0]) && bytes[1..].iter().all(|&byte| is_rest(byte))
}

/// `float(capacity)` for the C-rate limit multiplication.
#[must_use]
#[allow(clippy::cast_precision_loss)] // capacities far below 2^53
fn i64_to_f64(value: i64) -> f64 {
    value as f64
}
