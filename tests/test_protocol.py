from __future__ import annotations

import pytest

from mc3000_mcp import protocol as p


def status_frame(opcode: int = p.Opcode.STATUS) -> bytes:
    frame = bytearray(20)
    frame[0] = 0x0F
    frame[1] = opcode
    frame[2] = 1
    frame[3] = 0
    frame[4] = 0
    frame[5] = 1
    frame[6] = 1
    frame[7] = 0
    frame[8] = 123
    frame[9] = 0x10
    frame[10] = 0x68  # 4200 mV
    frame[11] = 0x03
    frame[12] = 0xE8  # 1000 mA
    frame[13] = 0x07
    frame[14] = 0xD0  # 2000 mAh
    frame[15] = 25
    frame[16] = 0
    frame[17] = 50
    frame[18] = 1
    return p.with_checksum(frame)


def test_simple_commands_match_apk_constants() -> None:
    assert p.command_get_status(0).hex().upper() == "0F55000000000000000000000000000000000064"
    assert p.command_get_status(3).hex().upper() == "0F55030000000000000000000000000000000067"
    assert p.command_start(2).hex().upper() == "0F05040000000000000000000000000000000018"
    assert p.command_stop(1).hex().upper() == "0FFE02000000000000000000000000000000000F"
    assert p.command_get_basic().hex().upper() == "0F61000000000000000000000000000000000070"
    assert p.command_get_voltage_curve(1).startswith(b"\x0f\x56\x01")
    assert p.command_restore_factory().startswith(b"\x0f\x65")
    assert p.command_restore_calibration().startswith(b"\x0f\x66")
    assert p.command_get_version().startswith(b"\x0f\x57")


def test_basic_settings_frame() -> None:
    frame = p.command_set_basic(
        temp_unit=0,
        system_beep=True,
        display=5,
        screensaver=True,
        cooling_fan=3,
        input_mv=11000,
    )
    assert len(frame) == 20
    assert frame[:9].hex().upper() == "0F6300010501032AF8"
    assert frame[-1] == p.checksum(frame[:-1])


def test_parse_status() -> None:
    parsed = p.parse_status(status_frame())
    assert parsed.slot == 1
    assert parsed.battery_type == "LiIon"
    assert parsed.status == "charge"
    assert parsed.voltage_mv == 4200
    assert parsed.current_ma == 1000
    assert parsed.capacity_mah == 2000
    assert parsed.internal_resistance_mohm == 50
    assert parsed.to_dict()["slot"] == 1


def test_parse_status_unknowns_and_internal_none() -> None:
    frame = bytearray(status_frame())
    frame[3] = 99
    frame[4] = 99
    frame[6] = 99
    frame[16] = 0xFF
    frame[17] = 0xFF
    parsed = p.parse_status(p.with_checksum(frame))
    assert parsed.battery_type == "unknown"
    assert parsed.mode == "unknown"
    assert parsed.status == "error"
    assert parsed.internal_resistance_mohm is None
    assert parsed.is_working is False


def test_parse_basic_and_version() -> None:
    basic = p.command_set_basic(
        temp_unit=1,
        system_beep=True,
        display=5,
        screensaver=False,
        cooling_fan=8,
        input_mv=12000,
    )
    parsed_basic = p.parse_basic(basic)
    assert parsed_basic.temp_unit_name == "fahrenheit"
    assert parsed_basic.system_beep is True
    assert parsed_basic.input_mv == 12000
    assert parsed_basic.to_dict()["input_mv"] == 12000

    version = bytearray(p.command_get_version("AA:BB:CC:DD:EE:FF"))
    version[14] = 4
    version[15] = 12
    version[16] = 33
    parsed_version = p.parse_version(p.with_checksum(version))
    assert parsed_version["firmware"] == "4.12"
    assert parsed_version["hardware"] == "3.30"


def test_profile_layout_and_split() -> None:
    profile = p.build_profile(slot_mask=1, battery_type=0, mode=0, capacity_mah=2000)
    assert len(profile) == 40
    assert profile[0] == 0x0F
    assert profile[1] == p.Opcode.SET_PROFILE
    assert profile[2] == 1
    assert profile[5:7] == bytes([0x07, 0xD0])
    assert profile[39] == p.checksum(profile[:39])
    first, second = p.split_profile(profile)
    assert len(first) == len(second) == 20
    assert first + second == profile


def test_profile_cycle_modes_and_breakin() -> None:
    cycle = p.build_profile(slot_mask=1, cycle_mode="D>C>D")
    assert cycle[21] == 3
    breakin = p.build_profile(slot_mask=1, cycle_mode="D>C>D", breakin=True, time_limit_minutes=99)
    assert breakin[21] == 1
    assert breakin[27:29] == b"\x00\x00"


def test_voltage_curve_parser() -> None:
    curve = bytearray(245)
    curve[0] = 0x0F
    curve[1] = p.Opcode.VOLTAGE_CURVE
    curve[2] = 2
    curve[3] = 0
    curve[4] = 5
    for idx, mv in enumerate((4100, 4110, 4120), start=0):
        pos = 5 + idx * 2
        curve[pos] = mv // 256
        curve[pos + 1] = mv % 256
    parsed = p.parse_voltage_curve(bytes(curve))
    assert parsed["slot"] == 2
    assert parsed["interval_ms"] == 5000
    assert parsed["points_mv"][:3] == [4100, 4110, 4120]


def test_parse_notification_variants() -> None:
    assert p.parse_notification(b"\x0f")["kind"] == "unknown"
    assert p.parse_notification(status_frame())["kind"] == "status"
    assert p.parse_notification(p.command_set_basic())["kind"] == "ack"
    assert p.parse_notification(p.command_get_basic())["kind"] == "basic"

    version = bytearray(p.command_get_version())
    version[14] = 1
    version[15] = 23
    version[16] = 22
    assert p.parse_notification(p.with_checksum(version))["kind"] == "version"

    curve = bytearray(p.CURVE_LEN)
    curve[0] = 0x0F
    curve[1] = p.Opcode.VOLTAGE_CURVE
    assert p.parse_notification(bytes(curve))["kind"] == "voltage_curve"
    assert p.parse_notification(b"\x0f\x99")["opcode"] == 0x99


def test_invalid_inputs_rejected() -> None:
    with pytest.raises(ValueError, match="slot must be"):
        p.command_start(4)
    with pytest.raises(ValueError, match="payload"):
        p.make_frame(1, b"x" * 19)
    with pytest.raises(ValueError, match="u16"):
        p.command_set_basic(input_mv=70000)
    with pytest.raises(ValueError, match="slot_mask"):
        p.build_profile(slot_mask=16)
    with pytest.raises(ValueError, match="40 bytes"):
        p.split_profile(b"short")
    with pytest.raises(ValueError, match="voltage curve"):
        p.parse_voltage_curve(b"short")
    with pytest.raises(ValueError, match="invalid MAC"):
        p.mac_to_bytes("bad")
    with pytest.raises(ValueError, match="expected at least"):
        p.parse_status(b"short")
    assert p.bytes_from_hex("0F 05 00") == b"\x0f\x05\x00"
