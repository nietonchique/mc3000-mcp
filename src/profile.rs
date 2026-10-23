//! Typed charging profiles, chemistry-aware defaults, 40-byte profile-frame
//! conversion (`protocol.build_profile`), and the four bundled safety profiles.
//!
//! The frame conversion reproduces the Android app byte for byte, including the
//! explicit-zero voltage behavior: omitted voltage fields fall back to
//! chemistry-appropriate defaults, while explicitly supplied zero values are written
//! verbatim (the charger silently ignored live `NiMH` profile writes when charge/keep
//! voltages were sent as zero, so the app always fills chemistry-appropriate
//! voltages before saving the `0x11` frame).

use crate::model::{Chemistry, Operation, RiskLevel, SlotMask};
use crate::protocol::{Opcode, PROFILE_LEN, put_u16_be, with_checksum_at};
use crate::{ProtocolError, lo8};
use serde::Serialize;

/// Charger mode code for plain charge (`protocol.MODE_CHARGE`).
pub const MODE_CHARGE: u8 = 0;
/// Charger mode code for refresh (`protocol.MODE_REFRESH`).
pub const MODE_REFRESH: u8 = 1;
/// Charger mode code for storage/break-in (`protocol.MODE_BREAKIN_OR_STORAGE`).
pub const MODE_BREAKIN_OR_STORAGE: u8 = 2;
/// Charger mode code for discharge (`protocol.MODE_DISCHARGE`).
pub const MODE_DISCHARGE: u8 = 3;
/// Charger mode code for custom cycles (`protocol.MODE_CYCLE`).
pub const MODE_CYCLE: u8 = 4;
/// Default charge current for mode-derived defaults (milliamps).
pub const DEFAULT_CHARGE_CURRENT_MA: i64 = 1000;
/// Default discharge current for mode-derived defaults (milliamps).
pub const DEFAULT_DISCHARGE_CURRENT_MA: i64 = 500;
/// Break-in charge current divisor: C/10 (`protocol.BREAKIN_CHARGE_DIVISOR`).
pub const BREAKIN_CHARGE_DIVISOR: i64 = 10;
/// Break-in discharge current divisor: C/5 (`protocol.BREAKIN_DISCHARGE_DIVISOR`).
pub const BREAKIN_DISCHARGE_DIVISOR: i64 = 5;

/// Battery-type codes treated as nickel chemistry for break-in and -deltaV defaults.
const NI_CHEMISTRY_CODES: [u8; 3] = [3, 4, 6];
/// Nickel-zinc battery-type code.
const NIZN_CODE: u8 = 5;
/// Battery-type codes sharing the NiZn/RAM voltage-default group.
const NIZN_RAM_CODES: [u8; 2] = [5, 7];

/// Cycle-mode selector (`protocol.CYCLE_MODE_TO_CODE` /
/// `protocol.BREAKIN_CYCLE_MODE_TO_CODE`).
///
/// Named variants carry their charger label; validity depends on whether the
/// profile is a break-in profile (the break-in table only knows `C>D>C` and
/// `D>C>D`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CycleMode {
    /// Numeric cycle-mode code, written to the frame verbatim.
    Code(u8),
    /// `C>D` (charge then discharge).
    ChargeToDischarge,
    /// `C>D>C` (charge, discharge, charge).
    ChargeDischargeCharge,
    /// `D>C` (discharge then charge).
    DischargeToCharge,
    /// `D>C>D` (discharge, charge, discharge).
    DischargeChargeDischarge,
}

impl CycleMode {
    /// Charger label for the named variants (`None` for [`CycleMode::Code`]).
    #[must_use]
    pub const fn name(self) -> Option<&'static str> {
        match self {
            CycleMode::Code(_) => None,
            CycleMode::ChargeToDischarge => Some("C>D"),
            CycleMode::ChargeDischargeCharge => Some("C>D>C"),
            CycleMode::DischargeToCharge => Some("D>C"),
            CycleMode::DischargeChargeDischarge => Some("D>C>D"),
        }
    }

    /// Parse a charger label (`protocol._cycle_mode_code` string form).
    ///
    /// # Errors
    ///
    /// [`ProtocolError::UnknownCycleMode`] when the label is not one of `C>D`,
    /// `C>D>C`, `D>C`, `D>C>D`.
    pub fn from_name(name: &str) -> Result<Self, ProtocolError> {
        match name {
            "C>D" => Ok(Self::ChargeToDischarge),
            "C>D>C" => Ok(Self::ChargeDischargeCharge),
            "D>C" => Ok(Self::DischargeToCharge),
            "D>C>D" => Ok(Self::DischargeChargeDischarge),
            other => Err(ProtocolError::UnknownCycleMode(other.to_string())),
        }
    }

    /// Resolve the numeric cycle-mode byte for the frame
    /// (`protocol._cycle_mode_code`); the break-in table accepts only `C>D>C` and
    /// `D>C>D`.
    ///
    /// # Errors
    ///
    /// [`ProtocolError::UnknownCycleMode`] when a named variant is not valid for the
    /// requested table (the Python implementation raised `KeyError`).
    pub fn to_code(self, breakin: bool) -> Result<u8, ProtocolError> {
        match self {
            CycleMode::Code(code) => Ok(code),
            CycleMode::ChargeToDischarge => {
                if breakin {
                    Err(ProtocolError::UnknownCycleMode("C>D".to_string()))
                } else {
                    Ok(0)
                }
            }
            // `C>D>C` is code 0 in the break-in table, code 1 in the normal table.
            CycleMode::ChargeDischargeCharge => Ok(u8::from(!breakin)),
            CycleMode::DischargeToCharge => {
                if breakin {
                    Err(ProtocolError::UnknownCycleMode("D>C".to_string()))
                } else {
                    Ok(2)
                }
            }
            CycleMode::DischargeChargeDischarge => Ok(if breakin { 1 } else { 3 }),
        }
    }
}

impl Serialize for CycleMode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match *self {
            CycleMode::Code(code) => serializer.serialize_u8(code),
            CycleMode::ChargeToDischarge => serializer.serialize_str("C>D"),
            CycleMode::ChargeDischargeCharge => serializer.serialize_str("C>D>C"),
            CycleMode::DischargeToCharge => serializer.serialize_str("D>C"),
            CycleMode::DischargeChargeDischarge => serializer.serialize_str("D>C>D"),
        }
    }
}

/// Arguments for [`build_profile`], mirroring `protocol.build_profile` keyword
/// arguments and their defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileParams {
    /// Target slot bitmask (0..=0x0F).
    pub slot_mask: SlotMask,
    /// Battery-type code (`0` = `LiIon`).
    pub battery_type: u8,
    /// Charger mode code (`0` = charge).
    pub mode: u8,
    /// Rated capacity in milliamp-hours (default 2000).
    pub capacity_mah: i64,
    /// Charge current; `None` derives the mode default.
    pub charge_current_ma: Option<i64>,
    /// Discharge current; `None` derives the mode default.
    pub discharge_current_ma: Option<i64>,
    /// Charge stop voltage; `None` derives the chemistry default, explicit zero is
    /// written verbatim.
    pub charge_stop_voltage_mv: Option<i64>,
    /// Discharge stop voltage; `None` derives the chemistry default.
    pub discharge_stop_voltage_mv: Option<i64>,
    /// Charge stop current; `None` derives the mode/chemistry default.
    pub charge_stop_current_ma: Option<i64>,
    /// Discharge stop current; `None` follows the discharge current.
    pub discharge_stop_current_ma: Option<i64>,
    /// Rest minutes after charge; `None` derives the mode default.
    pub charge_rest_minutes: Option<i64>,
    /// Cycle count; `None` defaults to 1.
    pub cycle_count: Option<i64>,
    /// Cycle mode; defaults to numeric code 0.
    pub cycle_mode: CycleMode,
    /// Negative delta-V cutoff; `None` derives the nickel default (3 mV).
    pub negative_delta_mv: Option<i64>,
    /// Trickle current; `None` derives the nickel default (10 mA).
    pub trickle_current_ma: Option<i64>,
    /// Keep/storage voltage; `None` derives the chemistry default.
    pub keep_voltage_mv: Option<i64>,
    /// Temperature cutoff in degrees Celsius (default 45).
    pub temp_cutoff: i64,
    /// Time limit in minutes; `None` defaults to 0 (disabled) and is forced to 0 for
    /// break-in profiles.
    pub time_limit_minutes: Option<i64>,
    /// Rest minutes after discharge; `None` derives the mode default.
    pub discharge_rest_minutes: Option<i64>,
    /// Explicit break-in flag; also implied by nickel chemistries in mode 2.
    pub breakin: bool,
}

impl Default for ProfileParams {
    fn default() -> Self {
        Self {
            slot_mask: SlotMask::default(),
            battery_type: 0,
            mode: 0,
            capacity_mah: 2000,
            charge_current_ma: None,
            discharge_current_ma: None,
            charge_stop_voltage_mv: None,
            discharge_stop_voltage_mv: None,
            charge_stop_current_ma: None,
            discharge_stop_current_ma: None,
            charge_rest_minutes: None,
            cycle_count: None,
            cycle_mode: CycleMode::Code(0),
            negative_delta_mv: None,
            trickle_current_ma: None,
            keep_voltage_mv: None,
            temp_cutoff: 45,
            time_limit_minutes: None,
            discharge_rest_minutes: None,
            breakin: false,
        }
    }
}

/// Mode-derived current/rest defaults (`protocol._profile_mode_defaults`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ModeDefaults {
    charge_current_ma: i64,
    discharge_current_ma: i64,
    charge_rest_minutes: i64,
    discharge_rest_minutes: i64,
}

/// Voltage defaults for a chemistry (`protocol._profile_voltage_defaults`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VoltageDefaults {
    charge_stop_voltage_mv: i64,
    discharge_stop_voltage_mv: i64,
    charge_stop_current_ma: i64,
    keep_voltage_mv: i64,
}

/// Resolved voltage fields written into the profile frame
/// (`protocol._profile_voltage_fields`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VoltageFields {
    charge_stop_voltage_mv: i64,
    discharge_stop_voltage_mv: i64,
    charge_stop_current_ma: i64,
    keep_voltage_mv: i64,
}

/// Resolved current/rest fields written into the profile frame
/// (`protocol._profile_current_defaults`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CurrentFields {
    charge_current_ma: i64,
    discharge_current_ma: i64,
    discharge_stop_current_ma: i64,
    charge_rest_minutes: i64,
    discharge_rest_minutes: i64,
}

/// Build the 40-byte profile frame used by the Android app
/// (`protocol.build_profile`).
///
/// The APK saves it as two 20-byte chunks (`parameter_1`, `parameter_2`), writes
/// chunk 1, waits ~50 ms, then writes chunk 2. The checksum is byte 39.
///
/// # Errors
///
/// - [`ProtocolError::U16OutOfRange`] when any `u16` field falls outside
///   `0..=65535` (checked before any byte is produced, like the Python version).
/// - [`ProtocolError::UnknownCycleMode`] when the cycle-mode label is not valid for
///   the resolved break-in table.
pub fn build_profile(params: &ProfileParams) -> Result<[u8; PROFILE_LEN], ProtocolError> {
    let breakin = is_breakin_mode(params.battery_type, params.mode, params.breakin);
    let currents = profile_current_fields(params, breakin);
    let voltages = profile_voltage_fields(
        params.battery_type,
        params.mode,
        currents.charge_current_ma,
        params.charge_stop_voltage_mv,
        params.discharge_stop_voltage_mv,
        params.charge_stop_current_ma,
        params.keep_voltage_mv,
    );
    let (negative_delta_mv_default, trickle_current_ma_default) = profile_ni_defaults(
        params.battery_type,
        params.negative_delta_mv,
        params.trickle_current_ma,
    );
    let cycle_mode = params.cycle_mode.to_code(breakin)?;
    let cycle_count = params.cycle_count.unwrap_or(1);
    let time_limit_minutes = params.time_limit_minutes.unwrap_or(0);

    let mut buf = [0u8; PROFILE_LEN];
    buf[0] = 0x0F;
    buf[1] = Opcode::SetProfile.as_u8();
    buf[2] = params.slot_mask.value();
    buf[3] = params.battery_type;
    buf[4] = params.mode;
    put_u16_be(&mut buf, 5, params.capacity_mah)?;
    put_u16_be(&mut buf, 7, currents.charge_current_ma)?;
    put_u16_be(&mut buf, 9, currents.discharge_current_ma)?;
    put_u16_be(&mut buf, 11, voltages.charge_stop_voltage_mv)?;
    put_u16_be(&mut buf, 13, voltages.discharge_stop_voltage_mv)?;
    put_u16_be(&mut buf, 15, voltages.charge_stop_current_ma)?;
    put_u16_be(&mut buf, 17, currents.discharge_stop_current_ma)?;
    buf[19] = lo8(currents.charge_rest_minutes);
    buf[20] = lo8(cycle_count);
    buf[21] = cycle_mode;
    buf[22] = lo8(negative_delta_mv_default);
    // The APK stores eddy/trickle current in 10 mA units.
    buf[23] = lo8(trickle_current_ma_default.div_euclid(10));
    put_u16_be(&mut buf, 24, voltages.keep_voltage_mv)?;
    buf[26] = lo8(params.temp_cutoff);
    put_u16_be(&mut buf, 27, if breakin { 0 } else { time_limit_minutes })?;
    buf[29] = lo8(currents.discharge_rest_minutes);
    let full = with_checksum_at(&buf, 39)?;
    let mut out = [0u8; PROFILE_LEN];
    out.copy_from_slice(&full);
    Ok(out)
}

/// Battery profile in the safety-layer shape (`safety.EXAMPLE_PROFILES` entries);
/// serialized JSON matches the Python profile dictionaries.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BatteryProfile {
    /// Stable profile identifier (`^[a-z0-9][a-z0-9_.-]{2,80}$`).
    pub profile_id: String,
    /// Battery chemistry.
    pub chemistry: Chemistry,
    /// Rated capacity in milliamp-hours.
    pub capacity_mah: i64,
    /// Number of cells in series.
    pub cell_count: i64,
    /// Nominal pack voltage in millivolts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nominal_voltage_mv: Option<i64>,
    /// Charging operation.
    pub operation: Operation,
    /// Charge current in milliamps.
    pub charge_current_ma: i64,
    /// Discharge current in milliamps.
    pub discharge_current_ma: i64,
    /// Charge cutoff voltage in millivolts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub charge_voltage_mv: Option<i64>,
    /// Discharge cutoff voltage in millivolts.
    pub discharge_cutoff_mv: i64,
    /// Hard time limit in minutes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_minutes: Option<i64>,
    /// Temperature cutoff in degrees Celsius.
    pub temperature_cutoff_c: i64,
    /// Human-readable temperature supervision policy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature_policy: Option<String>,
    /// Cycle-mode label for cycle operations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cycle_mode: Option<CycleMode>,
    /// Negative delta-V cutoff in millivolts (nickel chemistries).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub negative_delta_mv: Option<i64>,
    /// Trickle current in milliamps (nickel chemistries).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trickle_current_ma: Option<i64>,
    /// Where the parameters come from (manual, datasheet, label).
    pub source: String,
    /// Editorial risk classification.
    pub risk_level: RiskLevel,
    /// Free-form notes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// The four bundled profiles (`safety.EXAMPLE_PROFILES`), in definition order.
///
/// Like the Python `safety.profiles()`, every call returns fresh copies.
#[must_use]
pub fn bundled_profiles() -> Vec<BatteryProfile> {
    vec![
        nimh_aa_conservative_charge(),
        nimh_aa_refresh(),
        nimh_aa_break_in(),
        liion_1s_storage(),
    ]
}

/// Look up a bundled profile by `profile_id`.
#[must_use]
pub fn bundled_profile(profile_id: &str) -> Option<BatteryProfile> {
    bundled_profiles()
        .into_iter()
        .find(|profile| profile.profile_id == profile_id)
}

fn nimh_aa_conservative_charge() -> BatteryProfile {
    BatteryProfile {
        profile_id: "nimh-aa-conservative-charge".to_string(),
        chemistry: Chemistry::Nimh,
        capacity_mah: 2600,
        cell_count: 1,
        nominal_voltage_mv: Some(1200),
        operation: Operation::Charge,
        charge_current_ma: 260,
        discharge_current_ma: 200,
        charge_voltage_mv: Some(1650),
        discharge_cutoff_mv: 1000,
        timeout_minutes: Some(960),
        temperature_cutoff_c: 45,
        temperature_policy: Some("stop at cutoff; keep charger supervised".to_string()),
        cycle_mode: None,
        negative_delta_mv: Some(3),
        trickle_current_ma: Some(0),
        source: "GP AA NiMH label (2600 mAh) + SKYRC MC3000 manual safe defaults".to_string(),
        risk_level: RiskLevel::Low,
        notes: Some(
            "Slow conservative charge for GP AA NiMH 2600 mAh cells; user must confirm chemistry and capacity."
                .to_string(),
        ),
    }
}

fn nimh_aa_refresh() -> BatteryProfile {
    BatteryProfile {
        profile_id: "nimh-aa-refresh".to_string(),
        chemistry: Chemistry::Nimh,
        capacity_mah: 2600,
        cell_count: 1,
        nominal_voltage_mv: Some(1200),
        operation: Operation::Refresh,
        charge_current_ma: 260,
        discharge_current_ma: 200,
        charge_voltage_mv: Some(1650),
        discharge_cutoff_mv: 1000,
        timeout_minutes: Some(1440),
        temperature_cutoff_c: 45,
        temperature_policy: Some("stop at cutoff; monitor first cycle".to_string()),
        cycle_mode: None,
        negative_delta_mv: Some(3),
        trickle_current_ma: Some(0),
        source: "SKYRC MC3000 manual; live-tested BLE semantics".to_string(),
        risk_level: RiskLevel::Medium,
        notes: Some(
            "Refresh measures capacity; do not leave unknown cells unattended.".to_string(),
        ),
    }
}

fn nimh_aa_break_in() -> BatteryProfile {
    BatteryProfile {
        profile_id: "nimh-aa-break-in".to_string(),
        chemistry: Chemistry::Nimh,
        capacity_mah: 2600,
        cell_count: 1,
        nominal_voltage_mv: Some(1200),
        operation: Operation::BreakIn,
        charge_current_ma: 260,
        discharge_current_ma: 520,
        charge_voltage_mv: Some(1650),
        discharge_cutoff_mv: 1000,
        timeout_minutes: Some(0),
        temperature_cutoff_c: 45,
        temperature_policy: Some("stop at cutoff; supervise because break-in is long".to_string()),
        cycle_mode: Some(CycleMode::DischargeChargeDischarge),
        negative_delta_mv: Some(3),
        trickle_current_ma: Some(0),
        source: "SKYRC MC3000 manual; C/10 charge and C/5 discharge convention".to_string(),
        risk_level: RiskLevel::Medium,
        notes: Some(
            "For long-stored/deactivated NiMH only; regular cycle is usually gentler.".to_string(),
        ),
    }
}

fn liion_1s_storage() -> BatteryProfile {
    BatteryProfile {
        profile_id: "liion-1s-storage".to_string(),
        chemistry: Chemistry::LiIon,
        capacity_mah: 2000,
        cell_count: 1,
        nominal_voltage_mv: Some(3700),
        operation: Operation::Storage,
        charge_current_ma: 500,
        discharge_current_ma: 500,
        charge_voltage_mv: Some(3800),
        discharge_cutoff_mv: 3000,
        timeout_minutes: Some(240),
        temperature_cutoff_c: 45,
        temperature_policy: Some("stop at cutoff; use fire-safe area".to_string()),
        cycle_mode: None,
        negative_delta_mv: None,
        trickle_current_ma: None,
        source: "Conservative generic 1S Li-ion storage profile".to_string(),
        risk_level: RiskLevel::High,
        notes: Some(
            "MC3000 is single-cell-per-slot; do not use for multi-cell packs without per-cell support."
                .to_string(),
        ),
    }
}

/// Whether the profile resolves to a break-in program
/// (`protocol._is_breakin_mode`): the explicit flag, or a nickel chemistry in
/// storage/break-in mode 2.
fn is_breakin_mode(battery_type: u8, mode: u8, explicit_breakin: bool) -> bool {
    explicit_breakin
        || (NI_CHEMISTRY_CODES.contains(&battery_type) && mode == MODE_BREAKIN_OR_STORAGE)
}

/// Resolve the current/rest fields for the frame
/// (`protocol._profile_current_defaults`).
#[must_use]
fn profile_current_fields(params: &ProfileParams, breakin: bool) -> CurrentFields {
    let defaults = profile_mode_defaults(params.mode, params.capacity_mah, breakin);
    let charge_current = params
        .charge_current_ma
        .unwrap_or(defaults.charge_current_ma);
    let discharge_current = params
        .discharge_current_ma
        .unwrap_or(defaults.discharge_current_ma);
    CurrentFields {
        charge_current_ma: charge_current,
        discharge_current_ma: discharge_current,
        discharge_stop_current_ma: params
            .discharge_stop_current_ma
            .unwrap_or(discharge_current),
        charge_rest_minutes: params
            .charge_rest_minutes
            .unwrap_or(defaults.charge_rest_minutes),
        discharge_rest_minutes: params
            .discharge_rest_minutes
            .unwrap_or(defaults.discharge_rest_minutes),
    }
}

/// Resolve the voltage fields for the frame
/// (`protocol._profile_voltage_fields`).
#[must_use]
fn profile_voltage_fields(
    battery_type: u8,
    mode: u8,
    charge_current_ma: i64,
    charge_stop_voltage_mv: Option<i64>,
    discharge_stop_voltage_mv: Option<i64>,
    charge_stop_current_ma: Option<i64>,
    keep_voltage_mv: Option<i64>,
) -> VoltageFields {
    let defaults = profile_voltage_defaults(battery_type);
    let charge_stop_current_default = if mode == MODE_REFRESH || mode == MODE_CYCLE {
        charge_current_ma
    } else {
        defaults.charge_stop_current_ma
    };
    VoltageFields {
        charge_stop_voltage_mv: charge_stop_voltage_mv.unwrap_or(defaults.charge_stop_voltage_mv),
        discharge_stop_voltage_mv: discharge_stop_voltage_mv
            .unwrap_or(defaults.discharge_stop_voltage_mv),
        charge_stop_current_ma: charge_stop_current_ma.unwrap_or(charge_stop_current_default),
        keep_voltage_mv: keep_voltage_mv.unwrap_or(defaults.keep_voltage_mv),
    }
}

/// Nickel -deltaV/trickle defaults (`protocol._profile_ni_defaults`).
#[must_use]
fn profile_ni_defaults(
    battery_type: u8,
    negative_delta_mv: Option<i64>,
    trickle_current_ma: Option<i64>,
) -> (i64, i64) {
    let nickel = NI_CHEMISTRY_CODES.contains(&battery_type);
    (
        negative_delta_mv.unwrap_or(if nickel { 3 } else { 0 }),
        trickle_current_ma.unwrap_or(if nickel { 10 } else { 0 }),
    )
}

/// Mode-derived current/rest defaults (`protocol._profile_mode_defaults`).
#[must_use]
fn profile_mode_defaults(mode: u8, capacity_mah: i64, breakin: bool) -> ModeDefaults {
    if breakin {
        return ModeDefaults {
            charge_current_ma: clamp_current(
                capacity_mah.div_euclid(BREAKIN_CHARGE_DIVISOR),
                10,
                3000,
            ),
            discharge_current_ma: clamp_current(
                capacity_mah.div_euclid(BREAKIN_DISCHARGE_DIVISOR),
                10,
                2000,
            ),
            charge_rest_minutes: 60,
            discharge_rest_minutes: 60,
        };
    }
    if mode == MODE_REFRESH {
        return ModeDefaults {
            charge_current_ma: DEFAULT_CHARGE_CURRENT_MA,
            discharge_current_ma: DEFAULT_DISCHARGE_CURRENT_MA,
            charge_rest_minutes: 30,
            discharge_rest_minutes: 60,
        };
    }
    if mode == MODE_CYCLE {
        return ModeDefaults {
            charge_current_ma: DEFAULT_CHARGE_CURRENT_MA,
            discharge_current_ma: DEFAULT_DISCHARGE_CURRENT_MA,
            charge_rest_minutes: 20,
            discharge_rest_minutes: 10,
        };
    }
    ModeDefaults {
        charge_current_ma: DEFAULT_CHARGE_CURRENT_MA,
        discharge_current_ma: DEFAULT_DISCHARGE_CURRENT_MA,
        charge_rest_minutes: 0,
        discharge_rest_minutes: 0,
    }
}

/// Clamp a current into `[minimum, maximum]` (`protocol._clamp_current`).
#[must_use]
fn clamp_current(value: i64, minimum: i64, maximum: i64) -> i64 {
    value.clamp(minimum, maximum)
}

/// App/manual-compatible voltage defaults for omitted profile fields
/// (`protocol._profile_voltage_defaults`).
///
/// The charger silently ignored live `NiMH` profile writes when charge/keep voltages
/// were sent as zero. The Android app's UI always fills chemistry-appropriate
/// voltages before saving the `0x11` frame, even when a field is not relevant to
/// the visible mode. Explicit zero values from callers are kept verbatim, but
/// Ni-based profiles are never defaulted to `LiIon` voltages or to zero.
#[must_use]
fn profile_voltage_defaults(battery_type: u8) -> VoltageDefaults {
    if NI_CHEMISTRY_CODES.contains(&battery_type) {
        return VoltageDefaults {
            charge_stop_voltage_mv: 1650,
            discharge_stop_voltage_mv: 1000,
            charge_stop_current_ma: 50,
            keep_voltage_mv: 1000,
        };
    }
    if NIZN_RAM_CODES.contains(&battery_type) {
        return VoltageDefaults {
            charge_stop_voltage_mv: if battery_type == NIZN_CODE {
                1900
            } else {
                1650
            },
            discharge_stop_voltage_mv: if battery_type == NIZN_CODE { 1500 } else { 900 },
            charge_stop_current_ma: 50,
            keep_voltage_mv: 1200,
        };
    }
    VoltageDefaults {
        charge_stop_voltage_mv: 4200,
        discharge_stop_voltage_mv: 3000,
        charge_stop_current_ma: 100,
        keep_voltage_mv: 4150,
    }
}
