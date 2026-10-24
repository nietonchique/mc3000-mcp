//! Independently recorded Python/golden protocol fixtures.
//!
//! Every hex constant in this file was captured from the Python implementation
//! (`tests/test_protocol.py` and live runs of `src/mc3000_mcp/protocol.py`), not
//! from the Rust code under test. The Rust builders must reproduce these bytes
//! exactly.

use mc3000_mcp::ProtocolError;
use mc3000_mcp::model::{Slot, SlotMask};
use mc3000_mcp::profile::{CycleMode, ProfileParams, build_profile};
use mc3000_mcp::protocol::{
    BasicSettings, CURVE_LEN, Opcode, bytes_from_hex, checksum, command_get_basic,
    command_get_mc5000_status, command_get_status, command_get_version, command_get_voltage_curve,
    command_restore_calibration, command_restore_factory, command_set_basic, command_start,
    command_stop, mac_to_bytes, make_frame, parse_basic, parse_mc5000_status, parse_notification,
    parse_status, parse_version, parse_voltage_curve, split_profile, voltage_curve_time_series,
    voltage_curve_to_csv, with_checksum, with_checksum_at,
};
use proptest::prelude::*;
use serde_json::json;

/// Decode a fixture hex string; panics only on a corrupted literal.
fn hx(hex: &str) -> Vec<u8> {
    match bytes_from_hex(hex) {
        Ok(bytes) => bytes,
        Err(error) => panic!("corrupted fixture hex {hex}: {error}"),
    }
}

/// Fixture slot; panics only on a corrupted fixture value.
fn slot(value: u8) -> Slot {
    match Slot::try_from(value) {
        Ok(parsed) => parsed,
        Err(error) => panic!("corrupted fixture slot {value}: {error}"),
    }
}

/// Fixture slot mask; panics only on a corrupted fixture value.
fn mask(value: u8) -> SlotMask {
    match SlotMask::try_from(value) {
        Ok(parsed) => parsed,
        Err(error) => panic!("corrupted fixture mask {value}: {error}"),
    }
}

/// Unwrap a fixture `Result`, panicking with context on error (the crate denies
/// `expect` even in tests).
fn ok<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{what}: {error}"),
    }
}

/// Serialize a value to JSON, panicking on the (impossible) serialization failure.
fn value<T: serde::Serialize>(data: &T) -> serde_json::Value {
    match serde_json::to_value(data) {
        Ok(value) => value,
        Err(error) => panic!("fixture must serialize: {error}"),
    }
}

/// Upper-case hex of a byte sequence.
fn upper<const N: usize>(frame: [u8; N]) -> String {
    hex::encode_upper(frame)
}

/// Build a fixture profile frame, panicking on invalid fixture parameters.
fn frame(params: &ProfileParams) -> [u8; 40] {
    ok(build_profile(params), "fixture params fit u16 fields")
}

/// The Python `status_frame()` fixture with opcode 0x55, recorded as hex.
const STATUS_FRAME_HEX: &str = "0F550100000101007B106803E807D01900320168";

#[test]
fn simple_commands_match_apk_constants() {
    assert_eq!(
        upper(command_get_status(slot(0))),
        "0F55000000000000000000000000000000000064"
    );
    assert_eq!(
        upper(command_get_status(slot(3))),
        "0F55030000000000000000000000000000000067"
    );
    assert_eq!(
        upper(command_start(slot(2))),
        "0F05040000000000000000000000000000000018"
    );
    assert_eq!(
        upper(command_stop(slot(1))),
        "0FFE02000000000000000000000000000000000F"
    );
    assert_eq!(
        upper(command_get_basic()),
        "0F61000000000000000000000000000000000070"
    );
    assert_eq!(
        upper(command_get_voltage_curve(slot(1))),
        "0F56010000000000000000000000000000000066"
    );
    assert_eq!(
        upper(command_restore_factory()),
        "0F65000000000000000000000000000000000074"
    );
    assert_eq!(
        upper(command_restore_calibration()),
        "0F66000000000000000000000000000000000075"
    );
    assert_eq!(
        upper(ok(command_get_version(None), "no MAC to parse")),
        "0F57000000000000000000000000000000000066"
    );
}

#[test]
fn make_frame_matches_command_frames() {
    assert_eq!(
        make_frame(Opcode::Status, &[0]).map(upper).as_deref(),
        Ok("0F55000000000000000000000000000000000064"),
    );
    let oversized = [b'x'; 19];
    assert_eq!(
        make_frame(Opcode::Start, &oversized),
        Err(ProtocolError::PayloadTooLarge)
    );
}

#[test]
fn mc5000_command_matches_apk_bytes() {
    let command = command_get_mc5000_status(slot(2));
    assert_eq!(&command[..4], hx("0F039104").as_slice());
    assert_eq!(command.len(), 5);
    assert_eq!(command[4], checksum(&command[2..4]));
    assert_eq!(command, hx("0F03910495").as_slice());
}

#[test]
fn basic_settings_frame() {
    let frame = ok(
        command_set_basic(&BasicSettings {
            temp_unit: 0,
            system_beep: true,
            display: 5,
            screensaver: true,
            cooling_fan: 3,
            input_mv: 11_000,
        }),
        "11000 mV fits u16",
    );
    assert_eq!(frame.len(), 20);
    assert_eq!(hex::encode_upper(&frame[..9]), "0F6300010501032AF8");
    assert_eq!(
        hex::encode_upper(frame),
        "0F6300010501032AF8000000000000000000009E"
    );
    assert_eq!(frame[19], checksum(&frame[..19]));
}

#[test]
fn parse_status_fixture() {
    let parsed = ok(
        parse_status(&hx(STATUS_FRAME_HEX)),
        "fixture frame is long enough",
    );
    assert_eq!(parsed.slot, 1);
    assert_eq!(parsed.battery_type, "LiIon");
    assert_eq!(parsed.status, "charge");
    assert_eq!(parsed.voltage_mv, 4200);
    assert_eq!(parsed.current_ma, 1000);
    assert_eq!(parsed.capacity_mah, 2000);
    assert_eq!(parsed.internal_resistance_mohm, Some(50));
    assert!(parsed.is_working);
    // Python `parsed.to_dict()["slot"] == 1` via serde.
    let dict = value(&parsed);
    assert_eq!(dict["slot"], 1);
    assert_eq!(dict["time_seconds"], 123);
    assert_eq!(dict["led"], 1);
    assert_eq!(dict["internal_resistance_mohm"], 50);
}

#[test]
fn parse_status_unknowns_and_internal_none() {
    let mut frame = hx(STATUS_FRAME_HEX);
    frame[3] = 99;
    frame[4] = 99;
    frame[6] = 99;
    frame[16] = 0xFF;
    frame[17] = 0xFF;
    let parsed = ok(
        parse_status(&with_checksum(&frame)),
        "fixture frame is long enough",
    );
    assert_eq!(parsed.battery_type, "unknown");
    assert_eq!(parsed.mode, "unknown");
    assert_eq!(parsed.status, "error");
    assert_eq!(parsed.internal_resistance_mohm, None);
    assert!(!parsed.is_working);

    // Reserved resistance markers 0 and 1 also parse as None (Python parity).
    let mut zeros = hx(STATUS_FRAME_HEX);
    zeros[16] = 0;
    zeros[17] = 1;
    let parsed = ok(parse_status(&zeros), "fixture frame is long enough");
    assert_eq!(parsed.internal_resistance_mohm, None);
    // 0xFFFE is a real value.
    let mut real = hx(STATUS_FRAME_HEX);
    real[16] = 0xFF;
    real[17] = 0xFE;
    let parsed = ok(parse_status(&real), "fixture frame is long enough");
    assert_eq!(parsed.internal_resistance_mohm, Some(0xFFFE));
}

#[test]
fn parse_basic_and_version() {
    let basic = ok(
        command_set_basic(&BasicSettings {
            temp_unit: 1,
            system_beep: true,
            display: 5,
            screensaver: false,
            cooling_fan: 8,
            input_mv: 12_000,
        }),
        "12000 mV fits u16",
    );
    let parsed_basic = ok(parse_basic(&basic), "command frame is long enough");
    assert_eq!(parsed_basic.temp_unit_name, "fahrenheit");
    assert!(parsed_basic.system_beep);
    assert_eq!(parsed_basic.input_mv, 12_000);
    let dict = value(&parsed_basic);
    assert_eq!(dict["input_mv"], 12000);

    let version = with_checksum(&[
        0x0F, 0x57, 0x00, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04,
        0x0C, 0x21, 0x00, 0x00, 0x00,
    ]);
    assert_eq!(
        hex::encode_upper(&version),
        "0F5700AABBCCDDEEFF0000000000040C21000092"
    );
    let parsed_version = ok(parse_version(&version), "fixture frame is long enough");
    assert_eq!(parsed_version.firmware, "4.12");
    assert_eq!(parsed_version.hardware, "3.30");
    assert_eq!(
        parsed_version.raw_hex,
        "0F5700AABBCCDDEEFF0000000000040C21000092"
    );

    // command_get_version embeds the MAC at bytes 3..9.
    let with_mac = ok(
        command_get_version(Some("AA:BB:CC:DD:EE:FF")),
        "valid fixture MAC",
    );
    assert_eq!(upper(with_mac), "0F5700AABBCCDDEEFF0000000000000000000061");
}

#[test]
fn profile_layout_and_split() {
    let params = ProfileParams {
        slot_mask: mask(1),
        battery_type: 0,
        mode: 0,
        capacity_mah: 2000,
        ..ProfileParams::default()
    };
    let profile = frame(&params);
    assert_eq!(profile.len(), 40);
    assert_eq!(profile[0], 0x0F);
    assert_eq!(profile[1], Opcode::SetProfile.as_u8());
    assert_eq!(profile[2], 1);
    assert_eq!(&profile[5..7], &[0x07, 0xD0]);
    assert_eq!(profile[39], checksum(&profile[..39]));
    // Golden bytes recorded from the Python implementation.
    assert_eq!(
        hex::encode_upper(profile),
        "0F1101000007D003E801F410680BB8006401F4000100000010362D000000000000000000000000E0"
    );
    let (first, second) = ok(split_profile(&profile), "profile is 40 bytes");
    assert_eq!(first.len(), 20);
    assert_eq!(second.len(), 20);
    assert_eq!(&profile[..20], &first);
    assert_eq!(&profile[20..], &second);
    // Python raises ValueError for wrong-length profiles.
    assert_eq!(
        split_profile(b"short"),
        Err(ProtocolError::InvalidProfileLength)
    );
}

#[test]
fn profile_uses_chemistry_voltage_defaults() {
    let liion = frame(&ProfileParams {
        slot_mask: mask(1),
        ..ProfileParams::default()
    });
    assert_eq!(
        hex::encode_upper(liion),
        "0F1101000007D003E801F410680BB8006401F4000100000010362D000000000000000000000000E0"
    );

    let nimh = frame(&ProfileParams {
        slot_mask: mask(8),
        battery_type: 3,
        mode: 3,
        capacity_mah: 2600,
        ..ProfileParams::default()
    });
    assert_eq!(&nimh[11..13], &[0x06, 0x72]);
    assert_eq!(&nimh[13..15], &[0x03, 0xE8]);
    assert_eq!(&nimh[15..17], &[0x00, 0x32]);
    assert_eq!(&nimh[24..26], &[0x03, 0xE8]);
    assert_eq!(
        hex::encode_upper(nimh),
        "0F110803030A2803E801F4067203E8003201F4000100030103E82D000000000000000000000000E7"
    );

    let nizn = frame(&ProfileParams {
        slot_mask: mask(1),
        battery_type: 5,
        ..ProfileParams::default()
    });
    assert_eq!(&nizn[11..13], &[0x07, 0x6C]);
    assert_eq!(&nizn[13..15], &[0x05, 0xDC]);
    assert_eq!(
        hex::encode_upper(nizn),
        "0F1101050007D003E801F4076C05DC003201F4000100000004B02D0000000000000000000000003A"
    );

    let ram = frame(&ProfileParams {
        slot_mask: mask(1),
        battery_type: 7,
        ..ProfileParams::default()
    });
    assert_eq!(&ram[11..13], &[0x06, 0x72]);
    assert_eq!(&ram[13..15], &[0x03, 0x84]);
    assert_eq!(
        hex::encode_upper(ram),
        "0F1101070007D003E801F406720384003201F4000100000004B02D000000000000000000000000E7"
    );

    // Explicit zeros are written verbatim; only omitted fields get defaults.
    let explicit_off = frame(&ProfileParams {
        slot_mask: mask(8),
        battery_type: 3,
        charge_stop_voltage_mv: Some(0),
        keep_voltage_mv: Some(0),
        negative_delta_mv: Some(0),
        trickle_current_ma: Some(0),
        ..ProfileParams::default()
    });
    assert_eq!(&explicit_off[11..13], &[0x00, 0x00]);
    assert_eq!(explicit_off[22], 0);
    assert_eq!(explicit_off[23], 0);
    assert_eq!(&explicit_off[24..26], &[0x00, 0x00]);
    assert_eq!(
        hex::encode_upper(explicit_off),
        "0F1108030007D003E801F4000003E8003201F4000100000000002D00000000000000000000000022"
    );
}

#[test]
fn profile_mode_specific_defaults() {
    let refresh = frame(&ProfileParams {
        slot_mask: mask(8),
        battery_type: 3,
        mode: 1,
        ..ProfileParams::default()
    });
    assert_eq!(&refresh[15..17], &[0x03, 0xE8]);
    assert_eq!(refresh[19], 30);
    assert_eq!(refresh[20], 1);
    assert_eq!(refresh[22], 3);
    assert_eq!(refresh[23], 1);
    assert_eq!(refresh[29], 60);
    assert_eq!(
        hex::encode_upper(refresh),
        "0F1108030107D003E801F4067203E803E801F41E0100030103E82D00003C0000000000000000009D"
    );

    let cycle = frame(&ProfileParams {
        slot_mask: mask(8),
        battery_type: 3,
        mode: 4,
        ..ProfileParams::default()
    });
    assert_eq!(&cycle[15..17], &[0x03, 0xE8]);
    assert_eq!(cycle[19], 20);
    assert_eq!(cycle[29], 10);
    assert_eq!(
        hex::encode_upper(cycle),
        "0F1108030407D003E801F4067203E803E801F4140100030103E82D00000A00000000000000000064"
    );

    let breakin = frame(&ProfileParams {
        slot_mask: mask(8),
        battery_type: 3,
        mode: 2,
        capacity_mah: 2600,
        ..ProfileParams::default()
    });
    assert_eq!(&breakin[7..9], &[0x01, 0x04]);
    assert_eq!(&breakin[9..11], &[0x02, 0x08]);
    assert_eq!(breakin[19], 60);
    assert_eq!(&breakin[27..29], &[0x00, 0x00]);
    assert_eq!(breakin[29], 60);
    assert_eq!(
        hex::encode_upper(breakin),
        "0F110803020A2801040208067203E8003202083C0100030103E82D00003C000000000000000000A2"
    );
}

#[test]
fn profile_cycle_modes_and_breakin() {
    let cycle = frame(&ProfileParams {
        slot_mask: mask(1),
        cycle_mode: CycleMode::DischargeChargeDischarge,
        ..ProfileParams::default()
    });
    assert_eq!(cycle[21], 3);
    assert_eq!(
        hex::encode_upper(cycle),
        "0F1101000007D003E801F410680BB8006401F4000103000010362D000000000000000000000000E3"
    );

    let breakin = frame(&ProfileParams {
        slot_mask: mask(1),
        cycle_mode: CycleMode::DischargeChargeDischarge,
        breakin: true,
        time_limit_minutes: Some(99),
        ..ProfileParams::default()
    });
    assert_eq!(breakin[21], 1);
    assert_eq!(&breakin[27..29], &[0x00, 0x00]);
    assert_eq!(
        hex::encode_upper(breakin),
        "0F1101000007D000C8019010680BB8006401903C0101000010362D00003C0000000000000000006E"
    );

    // C>D is not a valid break-in cycle mode (Python raised KeyError).
    assert_eq!(
        build_profile(&ProfileParams {
            slot_mask: mask(1),
            cycle_mode: CycleMode::ChargeToDischarge,
            breakin: true,
            ..ProfileParams::default()
        }),
        Err(ProtocolError::UnknownCycleMode("C>D".to_string()))
    );
}

#[test]
fn voltage_curve_parser() {
    let mut curve = vec![0u8; CURVE_LEN];
    curve[0] = 0x0F;
    curve[1] = Opcode::VoltageCurve.as_u8();
    curve[2] = 2;
    curve[3] = 0;
    curve[4] = 5;
    for (index, mv) in [4100u16, 4110, 4120].into_iter().enumerate() {
        let pos = 5 + index * 2;
        curve[pos] = u8::try_from(mv / 256).unwrap_or_default();
        curve[pos + 1] = u8::try_from(mv % 256).unwrap_or_default();
    }
    let parsed = ok(parse_voltage_curve(&curve), "fixture curve is complete");
    assert_eq!(parsed.slot, 2);
    assert_eq!(parsed.interval_ms, 5000);
    assert_eq!(&parsed.points_mv[..3], &[4100, 4110, 4120]);
    assert_eq!(parsed.point_count, 3);
    assert_eq!(parsed.points_mv.len(), 120);
    let dict = value(&parsed);
    assert_eq!(dict["duration_seconds"], 10.0);
    assert_eq!(
        dict["time_series"][2],
        json!({"index": 2, "time_seconds": 10.0, "voltage_mv": 4120})
    );
    assert_eq!(
        voltage_curve_to_csv(&parsed),
        "index,time_seconds,voltage_mv,voltage_v\n0,0.0,4100,4.100\n1,5.0,4110,4.110\n2,10.0,4120,4.120\n"
    );
    // Python raises ValueError for short payloads.
    assert_eq!(
        parse_voltage_curve(b"short"),
        Err(ProtocolError::CurveTooShort {
            expected: 245,
            got: 5
        })
    );
}

#[test]
fn voltage_curve_time_series_stops_at_zero() {
    let series = voltage_curve_time_series(&[4100, 0, 4120], 5000);
    assert_eq!(series.len(), 1);
    assert_eq!(
        value(&series),
        json!([{"index": 0, "time_seconds": 0.0, "voltage_mv": 4100}])
    );
}

#[test]
fn parse_notification_variants() {
    let as_json = |data: &[u8]| value(&parse_notification(data));
    assert_eq!(
        as_json(&[0x0f]),
        json!({"kind": "unknown", "raw_hex": "0F"})
    );

    let status = as_json(&hx(STATUS_FRAME_HEX));
    assert_eq!(status["kind"], "status");
    assert_eq!(status["status"]["slot"], 1);
    assert_eq!(status["status"]["battery_type"], "LiIon");

    let set_basic = ok(
        command_set_basic(&BasicSettings::default()),
        "defaults fit u16",
    );
    assert_eq!(
        as_json(&set_basic),
        json!({
            "kind": "ack",
            "opcode": 99,
            "ok": false,
            "raw_hex": "0F6300000000002AF80000000000000000000094",
        })
    );
    assert_eq!(as_json(&command_get_basic())["kind"], "basic");

    let version = with_checksum(&[
        0x0F, 0x57, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
        0x17, 0x16, 0x00, 0x00, 0x00,
    ]);
    assert_eq!(as_json(&version)["kind"], "version");
    assert_eq!(as_json(&version)["firmware"], "1.23");
    assert_eq!(as_json(&version)["hardware"], "2.20");

    let mut curve = vec![0u8; CURVE_LEN];
    curve[0] = 0x0F;
    curve[1] = Opcode::VoltageCurve.as_u8();
    assert_eq!(as_json(&curve)["kind"], "voltage_curve");

    // Unknown opcodes keep the opcode byte.
    assert_eq!(
        as_json(&[0x0F, 0x99]),
        json!({"kind": "unknown", "opcode": 153, "raw_hex": "0F99"})
    );
}

#[test]
fn parse_notification_mc5000_status() {
    let mut frame = vec![0u8; 40];
    frame[0] = 0x0F;
    frame[1] = 0x03;
    frame[2] = 0x91;
    frame[3] = 0x04;
    frame[4..6].copy_from_slice(&5000u16.to_be_bytes());
    frame[6..8].copy_from_slice(&4100u16.to_be_bytes());
    frame[8..10].copy_from_slice(&24500u16.to_be_bytes());
    frame[10..12].copy_from_slice(&1200u16.to_be_bytes());
    frame[12..16].copy_from_slice(&3600u32.to_be_bytes());
    frame[16..18].copy_from_slice(&48u16.to_be_bytes());
    frame[18] = 2;
    frame[19] = 3;
    frame[20] = 0;
    frame[21] = 3;
    let parsed = parse_mc5000_status(&frame);
    assert_eq!(parsed.slot, Some(2));
    assert_eq!(parsed.battery_type, "NiMH");
    assert_eq!(parsed.mode, "discharge");
    assert_eq!(parsed.status, "charging");
    assert_eq!(parsed.voltage_mv, 4100);
    assert_eq!(parsed.current_ma, 5000);
    assert_eq!(parsed.temperature, 24);
    assert!(parsed.is_working);
    assert_eq!(parsed.internal_resistance_mohm, Some(48));
    assert_eq!(parsed.led, 1);

    let dict = value(&parse_notification(&frame));
    assert_eq!(dict["kind"], "status");
    assert_eq!(dict["status"]["battery_type"], "NiMH");
    assert_eq!(dict["status"]["mode"], "discharge");
    assert_eq!(dict["status"]["count"], 0);
    assert_eq!(dict["status"]["capacity_mah"], 1200);
    assert_eq!(dict["status"]["time_seconds"], 3600);

    let mut completed_frame = frame.clone();
    completed_frame[18] = 5;
    let completed = parse_mc5000_status(&completed_frame);
    assert_eq!(completed.status, "completed");
    assert!(!completed.is_working);
    assert_eq!(completed.led, 16);

    // Error code overrides the status name; unknown codes render as ERROR.
    let mut error_frame = frame.clone();
    error_frame[20] = 13;
    assert_eq!(parse_mc5000_status(&error_frame).status, "fully_charged");
    error_frame[20] = 99;
    assert_eq!(parse_mc5000_status(&error_frame).status, "ERROR");

    // Short frames are total: defaults with a null slot.
    let short = parse_mc5000_status(&hx("0F039104"));
    assert_eq!(short.slot, None);
    assert_eq!(short.battery_type, "LiIon");
    assert_eq!(short.mode, "charge");
    assert_eq!(short.status, "standby");
    assert_eq!(short.voltage_mv, 0);
    assert_eq!(short.internal_resistance_mohm, None);

    // Non-single-bit channel bytes yield a null slot.
    let weird = parse_mc5000_status(&hx("0F039110"));
    assert_eq!(weird.slot, None);

    // Unknown battery type makes the mode unknown too.
    let mut odd_type = frame.clone();
    odd_type[21] = 11;
    let parsed = parse_mc5000_status(&odd_type);
    assert_eq!(parsed.battery_type, "unknown");
    assert_eq!(parsed.mode, "unknown");
}

#[test]
fn invalid_inputs_rejected() {
    assert!(matches!(Slot::try_from(4), Err(ProtocolError::InvalidSlot)));
    assert!(matches!(
        SlotMask::try_from(16),
        Err(ProtocolError::InvalidSlotMask)
    ));
    // Firmware-side basic-settings bounds (reverse-engineered, fw 1.27):
    // temp_unit < 2, display < 6, cooling_fan < 10, input_mv 10000..=13000.
    assert_eq!(
        command_set_basic(&BasicSettings {
            input_mv: 70_000,
            ..BasicSettings::default()
        }),
        Err(ProtocolError::BasicValueOutOfRange {
            field: "input_mv",
            min: 10_000,
            max: 13_000,
            value: 70_000
        })
    );
    assert_eq!(
        command_set_basic(&BasicSettings {
            temp_unit: 2,
            ..BasicSettings::default()
        }),
        Err(ProtocolError::BasicValueOutOfRange {
            field: "temp_unit",
            min: 0,
            max: 1,
            value: 2
        })
    );
    assert_eq!(
        command_set_basic(&BasicSettings {
            display: 6,
            ..BasicSettings::default()
        }),
        Err(ProtocolError::BasicValueOutOfRange {
            field: "display",
            min: 0,
            max: 5,
            value: 6
        })
    );
    assert_eq!(
        command_set_basic(&BasicSettings {
            cooling_fan: 10,
            ..BasicSettings::default()
        }),
        Err(ProtocolError::BasicValueOutOfRange {
            field: "cooling_fan",
            min: 0,
            max: 9,
            value: 10
        })
    );
    assert_eq!(
        command_set_basic(&BasicSettings {
            input_mv: 9_999,
            ..BasicSettings::default()
        }),
        Err(ProtocolError::BasicValueOutOfRange {
            field: "input_mv",
            min: 10_000,
            max: 13_000,
            value: 9_999
        })
    );
    // Boundary values are accepted and produce the firmware frame.
    let boundary = match command_set_basic(&BasicSettings {
        temp_unit: 1,
        display: 5,
        cooling_fan: 9,
        input_mv: 13_000,
        ..BasicSettings::default()
    }) {
        Ok(frame) => frame,
        Err(error) => panic!("boundary values rejected: {error}"),
    };
    assert_eq!(boundary[7], 0x32, "13000 mV big-endian high byte");
    assert_eq!(
        parse_status(b"short"),
        Err(ProtocolError::FrameTooShort {
            expected: 20,
            got: 5
        })
    );
    assert_eq!(
        mac_to_bytes("bad"),
        Err(ProtocolError::InvalidMac("bad".to_string()))
    );
    assert_eq!(
        mac_to_bytes("AA:BB:CC:DD:EE:FF"),
        Ok([0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF])
    );
    assert_eq!(
        mac_to_bytes("AA-BB-CC-DD-EE-FF"),
        Ok([0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF])
    );
    assert_eq!(bytes_from_hex("0F 05 00"), Ok(vec![0x0f, 0x05, 0x00]));
    assert!(matches!(
        bytes_from_hex("0F 0"),
        Err(ProtocolError::InvalidHex(_))
    ));
}

#[test]
fn checksum_helpers_match_python() {
    assert_eq!(checksum(b"\x0f\x55"), 0x64);
    let frame = with_checksum(&hx(STATUS_FRAME_HEX));
    assert_eq!(hex::encode_upper(frame), STATUS_FRAME_HEX);
    let profile = [0u8; 40];
    let stamped = ok(with_checksum_at(&profile, 39), "39 < 40");
    assert_eq!(stamped[39], checksum(&stamped[..39]));
    assert!(matches!(
        with_checksum_at(&profile, 40),
        Err(ProtocolError::ChecksumIndexOutOfRange { index: 40, len: 40 })
    ));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn parse_notification_never_panics(data in proptest::collection::vec(any::<u8>(), 0..600)) {
        match parse_notification(&data) {
            mc3000_mcp::protocol::Notification::Unknown { raw_hex, .. }
            | mc3000_mcp::protocol::Notification::Ack { raw_hex, .. }
            | mc3000_mcp::protocol::Notification::Version { raw_hex, .. } => {
                prop_assert_eq!(raw_hex, hex::encode_upper(&data));
            }
            mc3000_mcp::protocol::Notification::VoltageCurve { raw_hex, .. } => {
                let curve_len = CURVE_LEN.min(data.len());
                prop_assert_eq!(raw_hex, hex::encode_upper(&data[..curve_len]));
            }
            _ => {}
        }
    }

    #[test]
    fn parsers_never_panic(data in proptest::collection::vec(any::<u8>(), 0..300)) {
        let _ = parse_status(&data);
        let _ = parse_basic(&data);
        let _ = parse_version(&data);
        let _ = parse_voltage_curve(&data);
        let _ = parse_mc5000_status(&data);
        let _ = split_profile(&data);
    }

    #[test]
    fn time_series_never_panics_and_stops_at_zero(
        points in proptest::collection::vec(any::<u16>(), 0..130),
        interval_ms in any::<u32>(),
    ) {
        let series = voltage_curve_time_series(&points, interval_ms);
        prop_assert!(series.len() <= points.len());
        // The series stops at the first zero (or runs to the end).
        let stop = series.len();
        if stop < points.len() && stop > 0 {
            prop_assert_eq!(points[stop], 0);
        }
        if stop > 0 {
            prop_assert_eq!(series[stop - 1].index, stop - 1);
        }
    }

    #[test]
    fn build_profile_never_panics(
        slot_mask in any::<u8>(),
        battery_type in any::<u8>(),
        mode in any::<u8>(),
        capacity in 0i64..200_000,
        charge_current in proptest::option::of(0i64..70_000),
        discharge_current in proptest::option::of(0i64..70_000),
        charge_stop_voltage in proptest::option::of(0i64..70_000),
        discharge_stop_voltage in proptest::option::of(0i64..70_000),
        keep_voltage in proptest::option::of(0i64..70_000),
        temp_cutoff in any::<u8>(),
        time_limit in proptest::option::of(0i64..70_000),
        cycle_mode in 0u8..255,
        breakin in any::<bool>(),
    ) {
        let params = ProfileParams {
            slot_mask: SlotMask::try_from(slot_mask).unwrap_or_default(),
            battery_type,
            mode,
            capacity_mah: capacity,
            charge_current_ma: charge_current,
            discharge_current_ma: discharge_current,
            charge_stop_voltage_mv: charge_stop_voltage,
            discharge_stop_voltage_mv: discharge_stop_voltage,
            charge_stop_current_ma: None,
            discharge_stop_current_ma: None,
            charge_rest_minutes: None,
            discharge_rest_minutes: None,
            cycle_count: None,
            cycle_mode: CycleMode::Code(cycle_mode),
            negative_delta_mv: None,
            trickle_current_ma: None,
            keep_voltage_mv: keep_voltage,
            temp_cutoff: i64::from(temp_cutoff),
            time_limit_minutes: time_limit,
            breakin,
        };
        if let Ok(profile) = build_profile(&params) {
            prop_assert_eq!(profile.len(), 40);
            prop_assert_eq!(profile[0], 0x0F);
            prop_assert_eq!(profile[39], checksum(&profile[..39]));
        }
    }

    #[test]
    fn with_checksum_always_yields_valid_checksum(data in proptest::collection::vec(any::<u8>(), 1..300)) {
        let stamped = with_checksum(&data);
        prop_assert_eq!(stamped.len(), data.len());
        prop_assert_eq!(stamped[stamped.len() - 1], checksum(&stamped[..stamped.len() - 1]));
    }
}
