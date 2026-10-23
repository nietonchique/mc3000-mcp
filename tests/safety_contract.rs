//! Consumer-visible safety validation boundaries.
//!
//! Every expected error/warning string in this file was captured from live runs of
//! the Python implementation (`src/mc3000_mcp/safety.py` + `tests/test_safety.py`),
//! so the Rust port must match them verbatim, including order.

use mc3000_mcp::ProtocolError;
use mc3000_mcp::model::{Chemistry, Operation, Slot};
use mc3000_mcp::profile::{CycleMode, bundled_profile, bundled_profiles};
use mc3000_mcp::safety::{
    chemistry_code, limits_for, operation_mode, profile_schema, profile_to_mc3000_args, profiles,
    safety_limits, validate_profile,
};
use serde_json::{Value, json};

/// Unwrap a fixture `Result`, panicking with context on error (the crate denies
/// `expect` even in tests).
fn ok<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{what}: {error}"),
    }
}

/// Unwrap a fixture `Result` expecting failure, panicking otherwise.
fn err<T: std::fmt::Debug, E>(result: Result<T, E>, what: &str) -> E {
    match result {
        Ok(value) => panic!("{what}: unexpectedly {value:?}"),
        Err(error) => error,
    }
}

/// Serialize a value to JSON, panicking on the (impossible) serialization failure.
fn value<T: serde::Serialize>(data: &T) -> serde_json::Value {
    match serde_json::to_value(data) {
        Ok(value) => value,
        Err(error) => panic!("fixture must serialize: {error}"),
    }
}

/// Serialize a bundled profile into the raw JSON shape `validate_profile` consumes.
fn profile_value(profile_id: &str) -> Value {
    let profile = bundled_profile(profile_id).unwrap_or_else(|| {
        panic!("bundled profile {profile_id} must exist");
    });
    match serde_json::to_value(&profile) {
        Ok(value) => value,
        Err(error) => panic!("profile {profile_id} must serialize: {error}"),
    }
}

/// Fixture slot; panics only on a corrupted fixture value.
fn slot(value: u8) -> Slot {
    match Slot::try_from(value) {
        Ok(parsed) => parsed,
        Err(error) => panic!("corrupted fixture slot {value}: {error}"),
    }
}

#[test]
fn bundled_profiles_validate() {
    assert_eq!(bundled_profiles().len(), 4);
    for profile in bundled_profiles() {
        let value = match serde_json::to_value(&profile) {
            Ok(value) => value,
            Err(error) => panic!("profile {} must serialize: {error}", profile.profile_id),
        };
        let result = validate_profile(&value);
        assert!(
            result.ok,
            "profile {} must validate: {result:?}",
            profile.profile_id
        );
    }
}

#[test]
fn rejects_missing_and_unknown_chemistry() {
    let result = validate_profile(&json!({"profile_id": "bad", "chemistry": "Mystery"}));
    assert!(!result.ok);
    // Full recorded output from the Python implementation, including order.
    assert_eq!(
        value(&result),
        json!({
            "ok": false,
            "errors": [
                "missing required fields: capacity_mah, cell_count, charge_current_ma, discharge_current_ma, discharge_cutoff_mv, operation, risk_level, source, temperature_cutoff_c",
                "unsupported chemistry: Mystery",
            ],
            "warnings": [],
            "normalized_profile": {"profile_id": "bad", "chemistry": "Mystery"},
        })
    );
    // Enforced limits expose the recorded NiMH values.
    let limits = safety_limits();
    assert_eq!(limits["NiMH"]["cell_nominal_mv"], 1200);
    assert_eq!(limits["Eneloop"]["cell_nominal_mv"], 1200);
    assert_eq!(limits["NiCd"]["charge_current_c_max"], 1.0);
}

#[test]
fn rejects_bad_scalar_values() {
    let mut profile = profiles()["nimh-aa-conservative-charge"].clone();
    profile["profile_id"] = json!("Bad ID");
    profile["capacity_mah"] = json!(0);
    profile["nominal_voltage_mv"] = json!(7400);
    profile["timeout_minutes"] = json!(2000);
    profile["temperature_cutoff_c"] = json!("hot");
    profile["operation"] = json!("storage");
    let result = validate_profile(&profile);
    assert!(!result.ok);
    // Exact recorded Python errors, in order.
    assert_eq!(
        result.errors,
        vec![
            "profile_id must match ^[a-z0-9][a-z0-9_.-]{2,80}$".to_string(),
            "operation storage is not supported for NiMH".to_string(),
            "temperature_cutoff_c must be an integer".to_string(),
            "capacity_mah must be positive".to_string(),
            "nominal_voltage_mv 7400 does not match NiMH x 1".to_string(),
            "charge_current_ma=260 outside enforced range 50..0".to_string(),
            "timeout_minutes must be 0..1440".to_string(),
        ]
    );
    assert!(result.warnings.is_empty());
    // normalized_profile keeps the raw (unnormalized) input values.
    assert_eq!(result.normalized_profile["capacity_mah"], 0);
    assert_eq!(result.normalized_profile["temperature_cutoff_c"], "hot");
}

#[test]
fn fail_closed_chemistries_without_limits() {
    // The Python test injected a "Ghost" chemistry; the real fail-closed surface is
    // the chemistry group with no enforced limits: NiZn, RAM, LTO, LiIo4.35.
    for chemistry in ["NiZn", "RAM", "LTO", "LiIo4.35"] {
        let mut profile = profiles()["nimh-aa-conservative-charge"].clone();
        profile["chemistry"] = json!(chemistry);
        let result = validate_profile(&profile);
        assert!(!result.ok, "{chemistry} must fail closed");
        assert_eq!(
            result.errors,
            vec![format!(
                "no enforced safety limits for chemistry: {chemistry}"
            )]
        );
    }
    // These chemistries are typed but carry no limits.
    assert!(limits_for(Chemistry::NiZn).is_none());
    assert!(limits_for(Chemistry::Ram).is_none());
    assert!(limits_for(Chemistry::Lto).is_none());
    assert!(limits_for(Chemistry::LiIo435).is_none());
}

#[test]
fn rejects_suspicious_nimh_current_and_cutoff() {
    let mut profile = profiles()["nimh-aa-conservative-charge"].clone();
    profile["charge_current_ma"] = json!(5000);
    profile["discharge_cutoff_mv"] = json!(500);
    let result = validate_profile(&profile);
    assert!(!result.ok);
    assert!(
        result
            .errors
            .iter()
            .any(|error| error.contains("charge_current_ma"))
    );
    assert!(
        result
            .errors
            .iter()
            .any(|error| error.contains("discharge_cutoff_mv"))
    );
    assert_eq!(
        result.errors,
        vec![
            "charge_current_ma=5000 outside enforced range 50..2600".to_string(),
            "discharge_cutoff_mv=500 outside enforced range 900..1100".to_string(),
        ]
    );
}

#[test]
fn rejects_unknown_lipo_multi_cell() {
    let mut profile = profiles()["liion-1s-storage"].clone();
    profile["cell_count"] = json!(2);
    profile["nominal_voltage_mv"] = json!(7400);
    let result = validate_profile(&profile);
    assert!(!result.ok);
    assert!(
        result
            .errors
            .iter()
            .any(|error| error.contains("cell_count"))
    );
    assert_eq!(
        result.errors,
        vec!["cell_count=2 outside enforced range 1..1".to_string()]
    );
    assert_eq!(
        result.warnings,
        vec!["high-risk profile: require extra human review and supervision".to_string()]
    );
}

#[test]
fn profile_to_mc3000_args_maps_slot_and_operation() {
    let args = ok(
        profile_to_mc3000_args(&profile_value("nimh-aa-refresh"), slot(3)),
        "bundled refresh profile is valid",
    );
    assert_eq!(args.slot_mask.value(), 8);
    assert_eq!(args.battery_type, chemistry_code("NiMH").unwrap_or(255));
    assert_eq!(args.mode, operation_mode("refresh").unwrap_or(255));
    // Full recorded conversion from the Python implementation.
    assert_eq!(args.capacity_mah, 2600);
    assert_eq!(args.charge_current_ma, Some(260));
    assert_eq!(args.discharge_current_ma, Some(200));
    assert_eq!(args.charge_stop_voltage_mv, Some(1650));
    assert_eq!(args.discharge_stop_voltage_mv, Some(1000));
    assert_eq!(args.discharge_stop_current_ma, Some(200));
    assert_eq!(args.temp_cutoff, 45);
    assert_eq!(args.time_limit_minutes, Some(1440));
    assert_eq!(args.cycle_count, Some(1));
    assert_eq!(args.cycle_mode, CycleMode::DischargeToCharge);
    assert_eq!(args.negative_delta_mv, Some(3));
    assert_eq!(args.trickle_current_ma, Some(0));
    assert!(!args.breakin);
}

#[test]
fn profile_to_mc3000_args_storage_and_breakin() {
    let storage = ok(
        profile_to_mc3000_args(&profile_value("liion-1s-storage"), slot(0)),
        "bundled storage profile is valid",
    );
    assert_eq!(storage.slot_mask.value(), 1);
    assert_eq!(storage.battery_type, 0);
    assert_eq!(storage.mode, 2);
    assert_eq!(storage.charge_stop_voltage_mv, Some(3800));
    assert_eq!(storage.time_limit_minutes, Some(240));
    assert!(!storage.breakin);

    let breakin = ok(
        profile_to_mc3000_args(&profile_value("nimh-aa-break-in"), slot(1)),
        "bundled break-in profile is valid",
    );
    assert_eq!(breakin.slot_mask.value(), 2);
    assert_eq!(breakin.battery_type, 3);
    assert_eq!(breakin.mode, 2);
    assert_eq!(breakin.discharge_stop_current_ma, Some(520));
    assert_eq!(breakin.time_limit_minutes, Some(0));
    assert_eq!(breakin.cycle_mode, CycleMode::DischargeChargeDischarge);
    assert!(breakin.breakin);
}

#[test]
fn invalid_profile_to_mc3000_args_fails() {
    let error = err(
        profile_to_mc3000_args(&json!({"chemistry": "NiMH"}), slot(0)),
        "invalid profile must fail",
    );
    assert!(
        error.to_string().starts_with("invalid profile: "),
        "unexpected message: {error}"
    );
    assert!(matches!(error, ProtocolError::InvalidProfile(_)));
}

#[test]
fn charge_voltage_default_and_zero_handling_matches_python() {
    // Python validates charge_voltage_mv=0 as out of range for NiMH (1400..1800):
    // an explicit zero never reaches the frame through the safety conversion.
    let mut zero = profiles()["nimh-aa-refresh"].clone();
    zero["charge_voltage_mv"] = json!(0);
    let error = err(
        profile_to_mc3000_args(&zero, slot(0)),
        "zero voltage must fail",
    );
    assert_eq!(
        error.to_string(),
        "invalid profile: charge_voltage_mv=0 outside enforced range 1400..1800"
    );

    // An omitted charge voltage converts to None, so build_profile applies the
    // chemistry default (NiMH 1650 mV), never LiIon voltages or zero.
    let mut omitted = profiles()["nimh-aa-refresh"].clone();
    match omitted.as_object_mut() {
        Some(object) => {
            object.remove("charge_voltage_mv");
        }
        None => panic!("profile is an object"),
    }
    let args = ok(
        profile_to_mc3000_args(&omitted, slot(0)),
        "profile is valid",
    );
    assert_eq!(args.charge_stop_voltage_mv, None);
    let frame = ok(
        mc3000_mcp::profile::build_profile(&args),
        "args fit u16 fields",
    );
    assert_eq!(hex::encode_upper(&frame[11..13]), "0672");
}

#[test]
fn timeout_boundaries() {
    let mut at_limit = profiles()["nimh-aa-refresh"].clone();
    at_limit["timeout_minutes"] = json!(1440);
    assert!(validate_profile(&at_limit).ok);

    let mut over_limit = at_limit.clone();
    over_limit["timeout_minutes"] = json!(1441);
    let result = validate_profile(&over_limit);
    assert!(!result.ok);
    assert!(
        result
            .errors
            .iter()
            .any(|error| error == "timeout_minutes must be 0..1440")
    );
}

#[test]
fn profiles_and_schema_shapes() {
    // profiles() is keyed by profile_id with the Python dict values.
    let bundled = profiles();
    assert_eq!(bundled.len(), 4);
    assert!(bundled.contains_key("nimh-aa-conservative-charge"));
    assert!(bundled.contains_key("nimh-aa-refresh"));
    assert!(bundled.contains_key("nimh-aa-break-in"));
    assert!(bundled.contains_key("liion-1s-storage"));

    let schema = profile_schema();
    assert_eq!(
        schema["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(
        schema["required"],
        json!([
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
        ])
    );
    assert_eq!(
        schema["properties"]["chemistry"]["enum"],
        // Recorded from Python sorted(CHEMISTRY_CODES): byte order, uppercase first.
        json!([
            "Eneloop", "LTO", "LiFe", "LiIo4.35", "LiIon", "NiCd", "NiMH", "NiZn", "RAM"
        ])
    );
    assert_eq!(
        schema["properties"]["operation"]["enum"],
        json!([
            "break_in",
            "charge",
            "cycle",
            "discharge",
            "refresh",
            "storage"
        ])
    );
}

#[test]
fn operation_and_chemistry_codes_match_python_tables() {
    // OPERATION_MODES
    for (name, mode) in [
        ("charge", 0u8),
        ("refresh", 1),
        ("break_in", 2),
        ("storage", 2),
        ("discharge", 3),
        ("cycle", 4),
    ] {
        assert_eq!(operation_mode(name), Some(mode), "OPERATION_MODES[{name}]");
    }
    // CHEMISTRY_CODES
    for (name, code) in [
        ("LiIon", 0u8),
        ("LiFe", 1),
        ("LiIo4.35", 2),
        ("NiMH", 3),
        ("NiCd", 4),
        ("NiZn", 5),
        ("Eneloop", 6),
        ("RAM", 7),
        ("LTO", 8),
    ] {
        assert_eq!(chemistry_code(name), Some(code), "CHEMISTRY_CODES[{name}]");
    }
    // Allowed nickel operations match the recorded table.
    let Some(limits) = limits_for(Chemistry::Nimh) else {
        panic!("NiMH has enforced limits");
    };
    assert_eq!(
        limits.operations,
        [
            Operation::Charge,
            Operation::Refresh,
            Operation::BreakIn,
            Operation::Discharge,
            Operation::Cycle,
        ]
    );
    let Some(liion) = limits_for(Chemistry::LiIon) else {
        panic!("LiIon has enforced limits");
    };
    assert_eq!(
        liion.operations,
        [Operation::Charge, Operation::Storage, Operation::Discharge]
    );
    assert_eq!(liion.storage_voltage_mv_min, Some(3700));
    assert_eq!(liion.storage_voltage_mv_max, Some(3900));
}
