"""SKYRC MC3000 BLE protocol recovered from MC3000 Android APK 4.1.2.

The app uses a single Nordic-UART-like GATT service/characteristic and sends
20-byte command frames. Longer profile/curve payloads are split over multiple
20-byte notifications/writes.
"""

from __future__ import annotations

from dataclasses import asdict, dataclass
from enum import IntEnum
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from collections.abc import Iterable

DEVICE_NAMES = ("SimpleBLEPeripheral", "Charger", "HitecCharger")
SERVICE_UUID = "0000FFE0-0000-1000-8000-00805f9b34fb"
CHAR_UUID = "0000FFE1-0000-1000-8000-00805f9b34fb"

FRAME_LEN = 20
PROFILE_LEN = 40
CURVE_LEN = 245
U16_MAX = 0xFFFF
SLOT_MASK_MAX = 0x0F
MIN_OPCODE_FRAME_LEN = 2
MAC_PARTS = 6


class Opcode(IntEnum):
    START = 0x05
    SET_PROFILE = 0x11
    STATUS = 0x55
    VOLTAGE_CURVE = 0x56
    VERSION = 0x57
    GET_BASIC = 0x61
    SET_BASIC = 0x63
    RESTORE_FACTORY = 0x65
    RESTORE_CALIBRATION = 0x66
    STOP = 0xFE


BATTERY_TYPES = (
    "LiIon",
    "LiFe",
    "LiIo4.35",
    "NiMH",
    "NiCd",
    "NiZn",
    "Eneloop",
    "RAM",
    "LTO",
    "Na-Lion",
)

MODE_NAMES = {
    0: "charge",
    1: "refresh",
    2: "storage_or_breakin",
    3: "discharge",
    4: "cycle",
}

STATUS_NAMES = {
    0: "standby",
    1: "charge",
    2: "discharge",
    3: "pause",
    4: "completed",
    128: "input_volt_low",
    129: "input_volt_high",
    130: "MCP3424-1 Err",
    131: "MCP3424-2 Err",
    132: "connect_break",
    133: "check_volt",
    134: "cap_cut",
    135: "time_cut",
    136: "sys_temp_high",
    137: "battery_temp_cut",
    138: "short_circuit",
    139: "polarity",
}

CYCLE_MODE_TO_CODE = {"C>D": 0, "C>D>C": 1, "D>C": 2, "D>C>D": 3}
BREAKIN_CYCLE_MODE_TO_CODE = {"C>D>C": 0, "D>C>D": 1}
MODE_CHARGE = 0
MODE_REFRESH = 1
MODE_BREAKIN_OR_STORAGE = 2
MODE_DISCHARGE = 3
MODE_CYCLE = 4
NI_CHEMISTRY_CODES = {3, 4, 6}
NIZN_CODE = 5
NIZN_RAM_CODES = {5, 7}
DEFAULT_CHARGE_CURRENT_MA = 1000
DEFAULT_DISCHARGE_CURRENT_MA = 500
BREAKIN_CHARGE_DIVISOR = 10
BREAKIN_DISCHARGE_DIVISOR = 5


@dataclass(slots=True)
class SlotStatus:
    slot: int
    battery_type_code: int
    battery_type: str
    mode_code: int
    mode: str
    count: int
    status_code: int
    status: str
    time_seconds: int
    voltage_mv: int
    current_ma: int
    capacity_mah: int
    temperature: int
    internal_resistance_mohm: int | None
    led: int
    is_working: bool

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


@dataclass(slots=True)
class BasicData:
    temp_unit: int
    temp_unit_name: str
    system_beep: bool
    display: int
    screensaver: bool
    cooling_fan: int
    input_mv: int

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


def _u16_be(hi: int, lo: int) -> int:
    return ((hi & 0xFF) << 8) | (lo & 0xFF)


def _put_u16_be(buf: bytearray, pos: int, value: int) -> None:
    if not 0 <= int(value) <= U16_MAX:
        raise ValueError(f"value for u16 out of range: {value}")
    value = int(value)
    buf[pos] = (value // 256) & 0xFF
    buf[pos + 1] = value % 256


def checksum(data: Iterable[int]) -> int:
    return sum(int(x) & 0xFF for x in data) & 0xFF


def with_checksum(buf: bytearray | bytes, checksum_index: int | None = None) -> bytes:
    out = bytearray(buf)
    if checksum_index is None:
        checksum_index = len(out) - 1
    out[checksum_index] = 0
    out[checksum_index] = checksum(out)
    return bytes(out)


def make_frame(opcode: int, payload: bytes = b"") -> bytes:
    if len(payload) > FRAME_LEN - 2:
        raise ValueError("20-byte frame payload is too large")
    buf = bytearray(FRAME_LEN)
    buf[0] = 0x0F
    buf[1] = opcode & 0xFF
    buf[2 : 2 + len(payload)] = payload
    return with_checksum(buf)


def command_get_status(slot: int) -> bytes:
    _validate_slot(slot)
    # APK shortcut: [0f, 55, slot, zeros..., slot + 100]
    return make_frame(Opcode.STATUS, bytes([slot]))


def command_start(slot: int) -> bytes:
    _validate_slot(slot)
    return make_frame(Opcode.START, bytes([1 << slot]))


def command_stop(slot: int) -> bytes:
    _validate_slot(slot)
    return make_frame(Opcode.STOP, bytes([1 << slot]))


def command_get_basic() -> bytes:
    return make_frame(Opcode.GET_BASIC)


def command_get_voltage_curve(slot: int) -> bytes:
    _validate_slot(slot)
    return make_frame(Opcode.VOLTAGE_CURVE, bytes([slot]))


def command_get_version(mac: str | None = None) -> bytes:
    buf = bytearray(FRAME_LEN)
    buf[0] = 0x0F
    buf[1] = Opcode.VERSION
    if mac:
        buf[3:9] = mac_to_bytes(mac)
    return with_checksum(buf)


def command_restore_factory() -> bytes:
    return make_frame(Opcode.RESTORE_FACTORY)


def command_restore_calibration() -> bytes:
    return make_frame(Opcode.RESTORE_CALIBRATION)


def command_set_basic(
    *,
    temp_unit: int = 0,
    system_beep: bool = False,
    display: int = 0,
    screensaver: bool = False,
    cooling_fan: int = 0,
    input_mv: int = 11000,
) -> bytes:
    buf = bytearray(FRAME_LEN)
    buf[0] = 0x0F
    buf[1] = Opcode.SET_BASIC
    buf[2] = int(temp_unit) & 0xFF
    buf[3] = 1 if system_beep else 0
    buf[4] = int(display) & 0xFF
    buf[5] = 1 if screensaver else 0
    buf[6] = int(cooling_fan) & 0xFF
    _put_u16_be(buf, 7, input_mv)
    return with_checksum(buf)


def build_profile(
    *,
    slot_mask: int,
    battery_type: int = 0,
    mode: int = 0,
    capacity_mah: int = 2000,
    charge_current_ma: int | None = None,
    discharge_current_ma: int | None = None,
    charge_stop_voltage_mv: int | None = None,
    discharge_stop_voltage_mv: int | None = None,
    charge_stop_current_ma: int | None = None,
    discharge_stop_current_ma: int | None = None,
    charge_rest_minutes: int | None = None,
    cycle_count: int | None = None,
    cycle_mode: int | str = 0,
    negative_delta_mv: int | None = None,
    trickle_current_ma: int | None = None,
    keep_voltage_mv: int | None = None,
    temp_cutoff: int = 45,
    time_limit_minutes: int | None = None,
    discharge_rest_minutes: int | None = None,
    breakin: bool = False,
) -> bytes:
    """Build the 40-byte profile frame used by the Android app.

    The APK saves it as two 20-byte chunks (`parameter_1`, `parameter_2`),
    writes chunk #1, waits ~50 ms, then writes chunk #2. Checksum is byte 39.
    """
    if not 0 <= slot_mask <= SLOT_MASK_MAX:
        raise ValueError(
            "slot_mask must be a bitmask 0..15 (slot1=1, slot2=2, slot3=4, slot4=8)",
        )
    battery_type = int(battery_type)
    mode = int(mode)
    capacity_mah = int(capacity_mah)
    breakin = _is_breakin_mode(battery_type, mode, explicit_breakin=breakin)
    currents = _profile_current_defaults(
        mode=mode,
        capacity_mah=capacity_mah,
        breakin=breakin,
        charge_current_ma=charge_current_ma,
        discharge_current_ma=discharge_current_ma,
        discharge_stop_current_ma=discharge_stop_current_ma,
        charge_rest_minutes=charge_rest_minutes,
        discharge_rest_minutes=discharge_rest_minutes,
    )
    voltages = _profile_voltage_fields(
        battery_type=battery_type,
        mode=mode,
        charge_current_ma=currents["charge_current_ma"],
        charge_stop_voltage_mv=charge_stop_voltage_mv,
        discharge_stop_voltage_mv=discharge_stop_voltage_mv,
        charge_stop_current_ma=charge_stop_current_ma,
        keep_voltage_mv=keep_voltage_mv,
    )
    ni_defaults = _profile_ni_defaults(
        battery_type,
        negative_delta_mv=negative_delta_mv,
        trickle_current_ma=trickle_current_ma,
    )
    cycle_mode = _cycle_mode_code(cycle_mode, breakin=breakin)
    cycle_count = 1 if cycle_count is None else cycle_count
    time_limit_minutes = 0 if time_limit_minutes is None else time_limit_minutes

    buf = bytearray(PROFILE_LEN)
    buf[0] = 0x0F
    buf[1] = Opcode.SET_PROFILE
    buf[2] = slot_mask
    buf[3] = int(battery_type) & 0xFF
    buf[4] = int(mode) & 0xFF
    _put_u16_be(buf, 5, capacity_mah)
    _put_u16_be(buf, 7, currents["charge_current_ma"])
    _put_u16_be(buf, 9, currents["discharge_current_ma"])
    _put_u16_be(buf, 11, voltages["charge_stop_voltage_mv"])
    _put_u16_be(buf, 13, voltages["discharge_stop_voltage_mv"])
    _put_u16_be(buf, 15, voltages["charge_stop_current_ma"])
    _put_u16_be(buf, 17, currents["discharge_stop_current_ma"])
    buf[19] = currents["charge_rest_minutes"] & 0xFF
    buf[20] = int(cycle_count) & 0xFF
    buf[21] = int(cycle_mode) & 0xFF
    buf[22] = ni_defaults["negative_delta_mv"] & 0xFF
    # APK stores eddy/trickle current in 10 mA units.
    buf[23] = int(ni_defaults["trickle_current_ma"] // 10) & 0xFF
    _put_u16_be(buf, 24, voltages["keep_voltage_mv"])
    buf[26] = int(temp_cutoff) & 0xFF
    _put_u16_be(buf, 27, 0 if breakin else time_limit_minutes)
    buf[29] = currents["discharge_rest_minutes"] & 0xFF
    return with_checksum(buf, 39)


def _is_breakin_mode(battery_type: int, mode: int, *, explicit_breakin: bool) -> bool:
    return explicit_breakin or (
        battery_type in NI_CHEMISTRY_CODES and mode == MODE_BREAKIN_OR_STORAGE
    )


def _profile_current_defaults(
    *,
    mode: int,
    capacity_mah: int,
    breakin: bool,
    charge_current_ma: int | None,
    discharge_current_ma: int | None,
    discharge_stop_current_ma: int | None,
    charge_rest_minutes: int | None,
    discharge_rest_minutes: int | None,
) -> dict[str, int]:
    defaults = _profile_mode_defaults(mode=mode, capacity_mah=capacity_mah, breakin=breakin)
    charge_current = _default_int(charge_current_ma, defaults["charge_current_ma"])
    discharge_current = _default_int(discharge_current_ma, defaults["discharge_current_ma"])
    return {
        "charge_current_ma": charge_current,
        "discharge_current_ma": discharge_current,
        "discharge_stop_current_ma": _default_int(discharge_stop_current_ma, discharge_current),
        "charge_rest_minutes": _default_int(charge_rest_minutes, defaults["charge_rest_minutes"]),
        "discharge_rest_minutes": _default_int(
            discharge_rest_minutes,
            defaults["discharge_rest_minutes"],
        ),
    }


def _profile_voltage_fields(
    *,
    battery_type: int,
    mode: int,
    charge_current_ma: int,
    charge_stop_voltage_mv: int | None,
    discharge_stop_voltage_mv: int | None,
    charge_stop_current_ma: int | None,
    keep_voltage_mv: int | None,
) -> dict[str, int]:
    defaults = _profile_voltage_defaults(battery_type)
    charge_stop_current_default = (
        charge_current_ma
        if mode in (MODE_REFRESH, MODE_CYCLE)
        else defaults["charge_stop_current_ma"]
    )
    return {
        "charge_stop_voltage_mv": _default_int(
            charge_stop_voltage_mv,
            defaults["charge_stop_voltage_mv"],
        ),
        "discharge_stop_voltage_mv": _default_int(
            discharge_stop_voltage_mv,
            defaults["discharge_stop_voltage_mv"],
        ),
        "charge_stop_current_ma": _default_int(charge_stop_current_ma, charge_stop_current_default),
        "keep_voltage_mv": _default_int(keep_voltage_mv, defaults["keep_voltage_mv"]),
    }


def _profile_ni_defaults(
    battery_type: int,
    *,
    negative_delta_mv: int | None,
    trickle_current_ma: int | None,
) -> dict[str, int]:
    return {
        "negative_delta_mv": _default_int(
            negative_delta_mv,
            3 if battery_type in NI_CHEMISTRY_CODES else 0,
        ),
        "trickle_current_ma": _default_int(
            trickle_current_ma,
            10 if battery_type in NI_CHEMISTRY_CODES else 0,
        ),
    }


def _cycle_mode_code(cycle_mode: int | str, *, breakin: bool) -> int:
    if not isinstance(cycle_mode, str):
        return int(cycle_mode)
    table = BREAKIN_CYCLE_MODE_TO_CODE if breakin else CYCLE_MODE_TO_CODE
    return table[cycle_mode]


def _profile_mode_defaults(*, mode: int, capacity_mah: int, breakin: bool) -> dict[str, int]:
    if breakin:
        return {
            "charge_current_ma": _clamp_current(capacity_mah // BREAKIN_CHARGE_DIVISOR, 10, 3000),
            "discharge_current_ma": _clamp_current(
                capacity_mah // BREAKIN_DISCHARGE_DIVISOR,
                10,
                2000,
            ),
            "charge_rest_minutes": 60,
            "discharge_rest_minutes": 60,
        }
    if mode == MODE_REFRESH:
        return {
            "charge_current_ma": DEFAULT_CHARGE_CURRENT_MA,
            "discharge_current_ma": DEFAULT_DISCHARGE_CURRENT_MA,
            "charge_rest_minutes": 30,
            "discharge_rest_minutes": 60,
        }
    if mode == MODE_CYCLE:
        return {
            "charge_current_ma": DEFAULT_CHARGE_CURRENT_MA,
            "discharge_current_ma": DEFAULT_DISCHARGE_CURRENT_MA,
            "charge_rest_minutes": 20,
            "discharge_rest_minutes": 10,
        }
    return {
        "charge_current_ma": DEFAULT_CHARGE_CURRENT_MA,
        "discharge_current_ma": DEFAULT_DISCHARGE_CURRENT_MA,
        "charge_rest_minutes": 0,
        "discharge_rest_minutes": 0,
    }


def _clamp_current(value: int, minimum: int, maximum: int) -> int:
    return max(minimum, min(maximum, value))


def _default_int(value: int | None, default: int) -> int:
    return int(default if value is None else value)


def _profile_voltage_defaults(battery_type: int) -> dict[str, int]:
    """Return app/manual-compatible voltage defaults for omitted profile fields.

    The charger silently ignored live NiMH profile writes when charge/keep voltages
    were sent as zero. The Android app's UI always fills chemistry-appropriate
    voltages before saving the 0x11 frame, even when a field is not relevant to the
    visible mode. Keep explicit zero values from callers, but never default Ni-based
    profiles to LiIon voltages or to zero.
    """
    if battery_type in NI_CHEMISTRY_CODES:
        return {
            "charge_stop_voltage_mv": 1650,
            "discharge_stop_voltage_mv": 1000,
            "charge_stop_current_ma": 50,
            "keep_voltage_mv": 1000,
        }
    if battery_type in NIZN_RAM_CODES:
        return {
            "charge_stop_voltage_mv": 1900 if battery_type == NIZN_CODE else 1650,
            "discharge_stop_voltage_mv": 1500 if battery_type == NIZN_CODE else 900,
            "charge_stop_current_ma": 50,
            "keep_voltage_mv": 1200,
        }
    return {
        "charge_stop_voltage_mv": 4200,
        "discharge_stop_voltage_mv": 3000,
        "charge_stop_current_ma": 100,
        "keep_voltage_mv": 4150,
    }


def split_profile(profile: bytes) -> tuple[bytes, bytes]:
    if len(profile) != PROFILE_LEN:
        raise ValueError("profile must be exactly 40 bytes")
    return profile[:20], profile[20:]


def parse_notification(data: bytes) -> dict[str, Any]:
    if len(data) < MIN_OPCODE_FRAME_LEN:
        return {"kind": "unknown", "raw_hex": data.hex().upper()}
    op = data[1]
    if op == Opcode.STATUS and len(data) >= FRAME_LEN:
        return {"kind": "status", "status": parse_status(data).to_dict()}
    if op == Opcode.GET_BASIC and len(data) >= FRAME_LEN:
        return {"kind": "basic", "basic": parse_basic(data).to_dict()}
    if op == Opcode.VERSION and len(data) >= FRAME_LEN:
        return {"kind": "version", **parse_version(data)}
    if op == Opcode.VOLTAGE_CURVE and len(data) >= CURVE_LEN:
        return {"kind": "voltage_curve", **parse_voltage_curve(data)}
    if op in (
        Opcode.START,
        Opcode.SET_PROFILE,
        Opcode.SET_BASIC,
        Opcode.RESTORE_FACTORY,
        Opcode.RESTORE_CALIBRATION,
    ):
        return {
            "kind": "ack",
            "opcode": op,
            "ok": len(data) > MIN_OPCODE_FRAME_LEN and data[2] == 1,
            "raw_hex": data.hex().upper(),
        }
    return {"kind": "unknown", "opcode": op, "raw_hex": data.hex().upper()}


def parse_status(frame: bytes) -> SlotStatus:
    _require_len(frame, FRAME_LEN)
    slot = frame[2] & 0xFF
    battery_type_code = frame[3] & 0xFF
    mode_code = frame[4] & 0xFF
    status_code = frame[6] & 0xFF
    internal = _u16_be(frame[16], frame[17])
    return SlotStatus(
        slot=slot,
        battery_type_code=battery_type_code,
        battery_type=(
            BATTERY_TYPES[battery_type_code]
            if battery_type_code < len(BATTERY_TYPES)
            else "unknown"
        ),
        mode_code=mode_code,
        mode=MODE_NAMES.get(mode_code, "unknown"),
        count=frame[5] & 0xFF,
        status_code=status_code,
        status=STATUS_NAMES.get(status_code, "error"),
        time_seconds=_u16_be(frame[7], frame[8]),
        voltage_mv=_u16_be(frame[9], frame[10]),
        current_ma=_u16_be(frame[11], frame[12]),
        capacity_mah=_u16_be(frame[13], frame[14]),
        temperature=frame[15] & 0xFF,
        internal_resistance_mohm=None if internal in (0, 1, 0xFFFF) else internal,
        led=frame[18] & 0xFF,
        is_working=status_code in (1, 2, 3),
    )


def parse_basic(frame: bytes) -> BasicData:
    _require_len(frame, FRAME_LEN)
    temp_unit = frame[2] & 0xFF
    return BasicData(
        temp_unit=temp_unit,
        temp_unit_name="celsius" if temp_unit == 0 else "fahrenheit",
        system_beep=(frame[3] & 0xFF) == 1,
        display=frame[4] & 0xFF,
        screensaver=(frame[5] & 0xFF) == 1,
        cooling_fan=frame[6] & 0xFF,
        input_mv=_u16_be(frame[7], frame[8]),
    )


def parse_version(frame: bytes) -> dict[str, str]:
    _require_len(frame, FRAME_LEN)
    firmware = ((frame[14] & 0xFF) * 100 + (frame[15] & 0xFF)) / 100.0
    hardware = ((frame[16] & 0xFF) * 10) / 100.0
    return {
        "firmware": f"{firmware:.2f}",
        "hardware": f"{hardware:.2f}",
        "raw_hex": frame.hex().upper(),
    }


def parse_voltage_curve(data: bytes) -> dict[str, Any]:
    if len(data) < CURVE_LEN:
        raise ValueError(f"voltage curve needs {CURVE_LEN} bytes, got {len(data)}")
    slot = data[2] & 0xFF
    interval_ms = _u16_be(data[3], data[4]) * 1000
    points = [_u16_be(data[i - 1], data[i]) for i in range(6, CURVE_LEN, 2)]
    return {
        "slot": slot,
        "interval_ms": interval_ms,
        "points_mv": points,
        "raw_hex": data[:CURVE_LEN].hex().upper(),
    }


def mac_to_bytes(mac: str) -> bytes:
    parts = mac.replace("-", ":").split(":")
    if len(parts) != MAC_PARTS:
        raise ValueError(f"invalid MAC address: {mac!r}")
    return bytes(int(p, 16) for p in parts)


def bytes_from_hex(hex_string: str) -> bytes:
    return bytes.fromhex("".join(hex_string.split()))


def _validate_slot(slot: int) -> None:
    if slot not in (0, 1, 2, 3):
        raise ValueError("slot must be 0..3 (Android app uses zero-based slots)")


def _require_len(data: bytes, n: int) -> None:
    if len(data) < n:
        raise ValueError(f"expected at least {n} bytes, got {len(data)}")
