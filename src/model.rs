//! Charger domain types: slots, slot masks, charger models, chemistries, operations,
//! and the charger-facing name tables used by the protocol parsers.
//!
//! The name tables replicate the Python `protocol.py` dictionaries byte for byte;
//! unknown codes resolve to the same fallback strings (`"unknown"`, `"error"`,
//! `"ERROR"`, `""`).

use serde::{Deserialize, Serialize};

use crate::ProtocolError;

/// Physical slot selector. The Android app uses zero-based slots `0..=3`, matching
/// physical slots `1..=4`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Slot(u8);

impl Slot {
    /// Slot bitmask value for this slot (`slot 2` -> `0b0100`).
    #[must_use]
    pub const fn mask_value(self) -> u8 {
        1 << self.0
    }

    /// Zero-based slot index.
    #[must_use]
    pub const fn value(self) -> u8 {
        self.0
    }
}

impl TryFrom<u8> for Slot {
    type Error = ProtocolError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        if value > 3 {
            return Err(ProtocolError::InvalidSlot);
        }
        Ok(Self(value))
    }
}

impl From<Slot> for u8 {
    fn from(value: Slot) -> Self {
        value.0
    }
}

/// Bitmask over the four charger slots (`slot1=1, slot2=2, slot3=4, slot4=8`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct SlotMask(u8);

impl SlotMask {
    /// Mask selecting exactly one slot.
    #[must_use]
    pub const fn from_slot(slot: Slot) -> Self {
        Self(slot.mask_value())
    }

    /// Raw bitmask value, guaranteed to be `0..=0x0F`.
    #[must_use]
    pub const fn value(self) -> u8 {
        self.0
    }
}

impl TryFrom<u8> for SlotMask {
    type Error = ProtocolError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        if value > 0x0F {
            return Err(ProtocolError::InvalidSlotMask);
        }
        Ok(Self(value))
    }
}

/// Charger family the library is connected to. The model is explicit per connection;
/// unknown models are read-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChargerModel {
    /// SKYRC MC3000 four-slot charger; full protocol support.
    Mc3000,
    /// SKYRC MC5000; read-only slot status support only.
    Mc5000,
}

impl ChargerModel {
    /// Whether the charger family accepts profile/write/start operations. The MC5000
    /// path is read-only until independent real-hardware evidence exists.
    #[must_use]
    pub const fn allows_writes(self) -> bool {
        matches!(self, Self::Mc3000)
    }
}

/// Battery chemistry names accepted by the safety layer (`safety.CHEMISTRY_CODES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Chemistry {
    /// Nickel-metal hydride.
    #[serde(rename = "NiMH")]
    Nimh,
    /// Nickel-cadmium.
    #[serde(rename = "NiCd")]
    Nicd,
    /// Panasonic Eneloop (low-self-discharge `NiMH`).
    #[serde(rename = "Eneloop")]
    Eneloop,
    /// Lithium-ion, 4.20 V charge.
    #[serde(rename = "LiIon")]
    LiIon,
    /// Lithium iron phosphate.
    #[serde(rename = "LiFe")]
    LiFe,
    /// Lithium-ion, 4.35 V charge.
    #[serde(rename = "LiIo4.35")]
    LiIo435,
    /// Nickel-zinc.
    #[serde(rename = "NiZn")]
    NiZn,
    /// Rechargeable alkaline.
    #[serde(rename = "RAM")]
    Ram,
    /// Lithium titanate.
    #[serde(rename = "LTO")]
    Lto,
}

impl Chemistry {
    /// All chemistries, in `CHEMISTRY_CODES` definition order.
    pub const ALL: [Chemistry; 9] = [
        Chemistry::LiIon,
        Chemistry::LiFe,
        Chemistry::LiIo435,
        Chemistry::Nimh,
        Chemistry::Nicd,
        Chemistry::NiZn,
        Chemistry::Eneloop,
        Chemistry::Ram,
        Chemistry::Lto,
    ];

    /// Protocol battery-type code (`safety.CHEMISTRY_CODES`).
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Chemistry::LiIon => 0,
            Chemistry::LiFe => 1,
            Chemistry::LiIo435 => 2,
            Chemistry::Nimh => 3,
            Chemistry::Nicd => 4,
            Chemistry::NiZn => 5,
            Chemistry::Eneloop => 6,
            Chemistry::Ram => 7,
            Chemistry::Lto => 8,
        }
    }

    /// Reverse mapping for [`Chemistry::code`].
    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Chemistry::LiIon),
            1 => Some(Chemistry::LiFe),
            2 => Some(Chemistry::LiIo435),
            3 => Some(Chemistry::Nimh),
            4 => Some(Chemistry::Nicd),
            5 => Some(Chemistry::NiZn),
            6 => Some(Chemistry::Eneloop),
            7 => Some(Chemistry::Ram),
            8 => Some(Chemistry::Lto),
            _ => None,
        }
    }

    /// Canonical chemistry name (`NiMH`, `LiIo4.35`, ...).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Chemistry::LiIon => "LiIon",
            Chemistry::LiFe => "LiFe",
            Chemistry::LiIo435 => "LiIo4.35",
            Chemistry::Nimh => "NiMH",
            Chemistry::Nicd => "NiCd",
            Chemistry::NiZn => "NiZn",
            Chemistry::Eneloop => "Eneloop",
            Chemistry::Ram => "RAM",
            Chemistry::Lto => "LTO",
        }
    }

    /// Exact-name lookup mirroring the Python dictionary keys.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|chemistry| chemistry.name() == name)
    }
}

/// Charging operation accepted by the safety layer (`safety.OPERATION_MODES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    /// Plain charge.
    Charge,
    /// Charge/discharge refresh cycle.
    Refresh,
    /// Break-in formation cycle.
    BreakIn,
    /// Storage-voltage conditioning.
    Storage,
    /// Plain discharge.
    Discharge,
    /// User-configured charge/discharge cycles.
    Cycle,
}

impl Operation {
    /// All operations, in `OPERATION_MODES` definition order.
    pub const ALL: [Operation; 6] = [
        Operation::Charge,
        Operation::Refresh,
        Operation::BreakIn,
        Operation::Storage,
        Operation::Discharge,
        Operation::Cycle,
    ];

    /// Charger mode code written into profile frames.
    #[must_use]
    pub const fn mode_code(self) -> u8 {
        match self {
            Operation::Charge => 0,
            Operation::Refresh => 1,
            Operation::BreakIn | Operation::Storage => 2,
            Operation::Discharge => 3,
            Operation::Cycle => 4,
        }
    }

    /// Canonical `snake_case` name (`break_in`, ...).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Operation::Charge => "charge",
            Operation::Refresh => "refresh",
            Operation::BreakIn => "break_in",
            Operation::Storage => "storage",
            Operation::Discharge => "discharge",
            Operation::Cycle => "cycle",
        }
    }

    /// Exact-name lookup mirroring the Python dictionary keys.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|operation| operation.name() == name)
    }
}

/// Editorial risk classification carried by battery profiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    /// Routine, well-understood parameters.
    Low,
    /// Needs attention during the run.
    Medium,
    /// Requires extra human review and supervision.
    High,
}

/// MC3000 status-frame battery-type names (`protocol.BATTERY_TYPES`).
pub const BATTERY_TYPES: [&str; 10] = [
    "LiIon", "LiFe", "LiIo4.35", "NiMH", "NiCd", "NiZn", "Eneloop", "RAM", "LTO", "Na-Lion",
];

/// MC5000 status-frame battery-type names (`protocol.BATTERY_TYPES_MC5000`).
pub const BATTERY_TYPES_MC5000: [&str; 10] = [
    "LiIon", "LiIon_HV", "LiFePO4", "NiMH", "NiCd", "Eneloop", "NiZn", "RAM", "LTO", "NaIon",
];

/// MC3000 status-frame mode names (`protocol.MODE_NAMES`); unknown codes fall back
/// to `"unknown"`.
#[must_use]
pub const fn mode_name(code: u8) -> &'static str {
    match code {
        0 => "charge",
        1 => "refresh",
        2 => "storage_or_breakin",
        3 => "discharge",
        4 => "cycle",
        _ => "unknown",
    }
}

/// MC3000 status-frame status names (`protocol.STATUS_NAMES`); unknown codes fall
/// back to `"error"`.
#[must_use]
pub const fn status_name(code: u8) -> &'static str {
    match code {
        0 => "standby",
        1 => "charge",
        2 => "discharge",
        3 => "pause",
        4 => "completed",
        128 => "input_volt_low",
        129 => "input_volt_high",
        130 => "MCP3424-1 Err",
        131 => "MCP3424-2 Err",
        132 => "connect_break",
        133 => "check_volt",
        134 => "cap_cut",
        135 => "time_cut",
        136 => "sys_temp_high",
        137 => "battery_temp_cut",
        138 => "short_circuit",
        139 => "polarity",
        _ => "error",
    }
}

/// MC3000 battery-type name; unknown codes fall back to `"unknown"`.
#[must_use]
pub const fn battery_type_name(code: u8) -> &'static str {
    if let Some(index) = checked_index(code) {
        return BATTERY_TYPES[index];
    }
    "unknown"
}

/// MC5000 battery-type name; unknown codes fall back to `"unknown"`.
#[must_use]
pub const fn mc5000_battery_type_name(code: u8) -> &'static str {
    if let Some(index) = checked_index(code) {
        return BATTERY_TYPES_MC5000[index];
    }
    "unknown"
}

/// MC5000 mode name resolved through the battery-type mode group
/// (`MC5000_MODE_GROUP_FOR_TYPE` / `MC5000_MODE_GROUPS`); unknown codes fall back to
/// `"unknown"`.
#[must_use]
pub const fn mc5000_mode_name(battery_type_code: u8, mode_code: u8) -> &'static str {
    match battery_type_code {
        0 | 1 | 2 | 8 | 9 => match mode_code {
            0 => "charge",
            1 => "storage",
            2 => "discharge",
            3 => "cycle",
            _ => "unknown",
        },
        3..=5 => match mode_code {
            0 => "charge",
            1 => "refresh",
            2 => "break_in",
            3 => "discharge",
            4 => "cycle",
            _ => "unknown",
        },
        6..=7 => match mode_code {
            0 => "charge",
            1 => "discharge",
            2 => "cycle",
            _ => "unknown",
        },
        _ => "unknown",
    }
}

/// MC5000 status name (`MC5000_STATUSES`); unknown codes fall back to `"error"`.
#[must_use]
pub const fn mc5000_status_name(code: u8) -> &'static str {
    match code {
        0 => "standby",
        1 => "processing",
        2 => "charging",
        3 => "discharging",
        4 => "resting",
        5 | 6 => "completed",
        _ => "error",
    }
}

/// MC5000 error name (`MC5000_ERRORS`): empty string for code 0, the error name for
/// known codes, and `"ERROR"` for unknown non-zero codes.
#[must_use]
pub const fn mc5000_error_name(code: u8) -> &'static str {
    match code {
        0 => "",
        1 => "input_volt_low",
        2 => "input_volt_high",
        3 | 9 => "connection_break",
        4 => "cap_cut",
        5 => "time_cut",
        6 => "sys_temp_high",
        7 => "calibration_failed",
        8 => "high_internal_resistance",
        10 => "battery_type_error",
        11 => "overload_protection",
        12 => "reversed_polarity",
        13 => "fully_charged",
        _ => "ERROR",
    }
}

/// Table index for a code, if the code is inside the 10-entry battery-type tables.
const fn checked_index(code: u8) -> Option<usize> {
    if code < 10 { Some(code as usize) } else { None }
}
