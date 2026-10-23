//! Safe SKYRC MC3000-compatible charger protocol, validation, and MCP server library.
//!
//! This crate is a typed Rust port of the recovered MC3000 BLE protocol (Android APK
//! 4.1.2) and of the enforced battery-safety validation layer. Everything in this
//! library is pure: frame builders produce exact byte sequences, parsers are total
//! over arbitrary bytes, and validation is fail-closed for chemistries without
//! enforced limits. All former Python exceptions (`ValueError`, `KeyError`,
//! `IndexError`) are represented as [`ProtocolError`].
//!
//! # Modules
//!
//! - [`model`] — domain types: [`Slot`](model::Slot), [`SlotMask`](model::SlotMask),
//!   [`ChargerModel`](model::ChargerModel), [`Chemistry`](model::Chemistry),
//!   [`Operation`](model::Operation), and the charger-facing name tables.
//! - [`protocol`] — pure 20-byte frame builders and total notification parsers
//!   (status, basic settings, version, voltage curve, MC5000 slot status).
//! - [`profile`] — typed charging profiles, chemistry-aware defaults, 40-byte
//!   profile-frame conversion, and the four bundled profiles.
//! - [`safety`] — enforced safety limits, profile validation, and the fail-closed
//!   profile-to-MC3000 argument conversion.
//! - [`transport`] — async BLE transport interface with the btleplug/BlueZ
//!   backend ([`transport::BleTransport`]) and the scriptable fake backend
//!   ([`transport::FakeTransport`]).
//! - [`client`] — serialized charger operations ([`client::ChargerClient`]),
//!   connection/model state, and read-only model enforcement.
//! - [`session`] — bounded, redacted in-memory audit log
//!   ([`session::SessionLog`]).
//! - [`mcp`] — the rmcp server surface ([`mcp::McpService`]): every
//!   production and legacy tool, resources, prompts, and the audit export.
//!
//! # Byte-compatibility
//!
//! Frame bytes, checksums, defaults, explicit-zero voltage behavior, parser names and
//! fields, the MC5000 parser, CSV output, safety limits, and the bundled profiles
//! match the Python implementation exactly. See `tests/protocol_fixtures.rs` and
//! `tests/safety_contract.rs` for the independently recorded golden fixtures.

#![warn(missing_docs)]

pub mod client;
pub mod mcp;
pub mod model;
pub mod profile;
pub mod protocol;
pub mod safety;
pub mod session;
pub mod transport;

/// Typed replacement for every Python exception raised by protocol, profile, and
/// safety code. `Display` strings preserve the exact human-readable messages of the
/// Python implementation wherever one existed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProtocolError {
    /// A slot outside `0..=3` was requested (Android app uses zero-based slots).
    #[error("slot must be 0..3 (Android app uses zero-based slots)")]
    InvalidSlot,
    /// A 20-byte frame payload exceeded the 18 available bytes.
    #[error("20-byte frame payload is too large")]
    PayloadTooLarge,
    /// A value outside `0..=65535` was written into a big-endian `u16` field.
    #[error("value for u16 out of range: {0}")]
    U16OutOfRange(i64),
    /// A profile slot mask outside `0..=0x0F` was requested.
    #[error("slot_mask must be a bitmask 0..15 (slot1=1, slot2=2, slot3=4, slot4=8)")]
    InvalidSlotMask,
    /// A profile buffer with a length other than 40 bytes was split or interpreted.
    #[error("profile must be exactly 40 bytes")]
    InvalidProfileLength,
    /// A voltage-curve notification was shorter than [`CURVE_LEN`](protocol::CURVE_LEN).
    #[error("voltage curve needs {expected} bytes, got {got}")]
    CurveTooShort {
        /// Minimum number of bytes required by the parser.
        expected: usize,
        /// Number of bytes actually supplied.
        got: usize,
    },
    /// A framed notification was shorter than the parser requires.
    #[error("expected at least {expected} bytes, got {got}")]
    FrameTooShort {
        /// Minimum number of bytes required by the parser.
        expected: usize,
        /// Number of bytes actually supplied.
        got: usize,
    },
    /// A checksum position outside the buffer was requested.
    #[error("checksum index {index} out of range for buffer of length {len}")]
    ChecksumIndexOutOfRange {
        /// Requested checksum byte position.
        index: usize,
        /// Length of the supplied buffer.
        len: usize,
    },
    /// A MAC address could not be parsed as six colon/hyphen-separated hex bytes.
    #[error("invalid MAC address: '{0}'")]
    InvalidMac(String),
    /// A hex string could not be decoded (invalid digit or odd length).
    #[error("invalid hex string: {0}")]
    InvalidHex(String),
    /// A cycle-mode name was not one of the charger's known cycle modes.
    #[error("unknown cycle mode: {0}")]
    UnknownCycleMode(String),
    /// A safety profile failed validation; the message lists every error joined by
    /// `"; "` exactly like the Python `ValueError`.
    #[error("invalid profile: {0}")]
    InvalidProfile(String),
}

/// Keep the low byte of an integer expression (`& 0xFF` semantics for the byte
/// writers).
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // masked to 0..=255 first
pub(crate) fn lo8(value: i64) -> u8 {
    (value & 0xFF) as u8
}

/// Truncate toward zero exactly like Python `int()` on a `float`; callers pass
/// finite values only (JSON numbers and byte-derived math).
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Python int() truncation semantics
pub(crate) fn py_int(value: f64) -> i64 {
    value as i64
}

/// Truncate toward zero into a `u32`; callers pass finite non-negative values only.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Python int() truncation semantics
pub(crate) fn f64_to_u32(value: f64) -> u32 {
    value as u32
}

/// Widen a sample index for timestamp math (Python `index * interval_seconds`).
#[must_use]
#[allow(clippy::cast_precision_loss)] // sample counts stay far below 2^53
pub(crate) fn index_to_f64(index: usize) -> f64 {
    index as f64
}
