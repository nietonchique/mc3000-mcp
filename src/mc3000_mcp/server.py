from __future__ import annotations

import asyncio
import json
import sys
import traceback
from collections.abc import Awaitable, Callable, Mapping
from typing import Any

from . import protocol, safety
from .transport import BleakMissingError, MC3000Client, find_and_connect

_client: MC3000Client | None = None
_session_log: list[dict[str, Any]] = []

ToolHandler = Callable[[dict[str, Any]], Awaitable[Any]]


def _log_event(command: str, args: dict[str, Any], result: Mapping[str, Any]) -> None:
    redacted_args = dict(args)
    if "address" in redacted_args:
        redacted_args["address"] = "<redacted-device-id>"
    _session_log.append({"command": command, "arguments": redacted_args, "result": dict(result)})


def _json_result(value: Any) -> dict[str, Any]:
    return {"content": [{"type": "text", "text": json.dumps(value, ensure_ascii=False, indent=2)}]}


def _text_result(value: str) -> dict[str, Any]:
    return {"content": [{"type": "text", "text": value}]}


def _ensure_client() -> MC3000Client:
    global _client
    if _client is None:
        _client = MC3000Client()
    return _client


async def tool_scan(args: dict[str, Any]) -> Any:
    timeout = float(args.get("timeout", 5.0))
    devices = await MC3000Client.scan(timeout=timeout)
    return {"devices": [d.to_dict() for d in devices], "matched_names": protocol.DEVICE_NAMES}


async def tool_connect(args: dict[str, Any]) -> Any:
    client = _ensure_client()
    return await find_and_connect(
        client, address=args.get("address"), timeout=float(args.get("timeout", 5.0))
    )


async def tool_disconnect(args: dict[str, Any]) -> Any:
    _ = args
    if _client is None:
        return {"connected": False}
    return await _client.disconnect()


async def tool_status(args: dict[str, Any]) -> Any:
    client = _ensure_client()
    if "slot" in args and args["slot"] is not None:
        return await client.poll_status(int(args["slot"]))
    return {"slots": await client.poll_all_status()}


async def tool_start(args: dict[str, Any]) -> Any:
    client = _ensure_client()
    slot = int(args["slot"])
    await client.write(protocol.command_start(slot))
    return {"sent": "start", "slot": slot}


async def tool_stop(args: dict[str, Any]) -> Any:
    client = _ensure_client()
    slot = int(args["slot"])
    await client.write(protocol.command_stop(slot))
    return {"sent": "stop", "slot": slot}


async def tool_get_version(args: dict[str, Any]) -> Any:
    client = _ensure_client()
    return await client.request(
        protocol.command_get_version(args.get("mac")), protocol.Opcode.VERSION
    )


async def tool_get_basic(args: dict[str, Any]) -> Any:
    _ = args
    client = _ensure_client()
    return await client.request(protocol.command_get_basic(), protocol.Opcode.GET_BASIC)


async def tool_set_basic(args: dict[str, Any]) -> Any:
    client = _ensure_client()
    frame = protocol.command_set_basic(
        temp_unit=int(args.get("temp_unit", 0)),
        system_beep=bool(args.get("system_beep", False)),
        display=int(args.get("display", 0)),
        screensaver=bool(args.get("screensaver", False)),
        cooling_fan=int(args.get("cooling_fan", 0)),
        input_mv=int(args.get("input_mv", 11000)),
    )
    return await client.request(frame, protocol.Opcode.SET_BASIC)


async def tool_get_voltage_curve(args: dict[str, Any]) -> Any:
    client = _ensure_client()
    slot = int(args["slot"])
    return await client.request(
        protocol.command_get_voltage_curve(slot),
        protocol.Opcode.VOLTAGE_CURVE,
        timeout=float(args.get("timeout", 8.0)),
    )


async def tool_build_profile(args: dict[str, Any]) -> Any:
    profile = protocol.build_profile(**args)
    first, second = protocol.split_profile(profile)
    return {
        "profile_hex": profile.hex().upper(),
        "chunk_1_hex": first.hex().upper(),
        "chunk_2_hex": second.hex().upper(),
    }


async def tool_apply_profile(args: dict[str, Any]) -> Any:
    client = _ensure_client()
    if "profile_hex" in args:
        profile = protocol.bytes_from_hex(str(args["profile_hex"]))
    else:
        build_args = dict(args)
        build_args.pop("start", None)
        profile = protocol.build_profile(**build_args)
    return await client.send_profile(profile, start=bool(args.get("start", False)))


async def tool_send_raw(args: dict[str, Any]) -> Any:
    client = _ensure_client()
    data = protocol.bytes_from_hex(str(args["hex"] if "hex" in args else args["frame_hex"]))
    if len(data) == protocol.PROFILE_LEN:
        return await client.send_profile(data, start=bool(args.get("start", False)))
    await client.write(data)
    if args.get("wait", False):
        return await client.wait_for(timeout=float(args.get("timeout", 3.0)))
    return {"sent_hex": data.hex().upper()}


async def tool_list_profiles(args: dict[str, Any]) -> Any:
    _ = args
    return {"profiles": safety.profiles()}


async def tool_validate_profile(args: dict[str, Any]) -> Any:
    profile = _profile_from_args(args)
    result = safety.validate_profile(profile)
    _log_event("charger.validate_profile", args, result)
    return result


async def tool_charger_apply_profile(args: dict[str, Any]) -> Any:
    dry_run = bool(args.get("dry_run", True))
    slot = int(args["slot"])
    profile = _profile_from_args(args)
    validation = safety.validate_profile(profile)
    if not validation["ok"]:
        result = {"dry_run": dry_run, "applied": False, "validation": validation}
        _log_event("charger.apply_profile", args, result)
        return result

    mc3000_args = safety.profile_to_mc3000_args(profile, slot=slot)
    built = await tool_build_profile(mc3000_args)
    if dry_run:
        result = {"dry_run": True, "applied": False, "validation": validation, **built}
        _log_event("charger.apply_profile", args, result)
        return result
    if args.get("confirmation_token") != f"APPLY_PROFILE_SLOT_{slot}":
        raise ValueError(f"dangerous write requires confirmation_token=APPLY_PROFILE_SLOT_{slot}")
    client = _ensure_client()
    profile_bytes = protocol.build_profile(**mc3000_args)
    result = await client.send_profile(profile_bytes, start=False)
    result = {"dry_run": False, "applied": True, "validation": validation, **result}
    _log_event("charger.apply_profile", args, result)
    return result


async def tool_charger_start(args: dict[str, Any]) -> Any:
    slot = int(args["slot"])
    if args.get("confirmation_token") != f"START_SLOT_{slot}":
        raise ValueError(f"starting hardware requires confirmation_token=START_SLOT_{slot}")
    result = await tool_start({"slot": slot})
    _log_event("charger.start", args, result)
    return result


async def tool_stop_all(args: dict[str, Any]) -> Any:
    _ = args
    client = _ensure_client()
    stopped: list[int] = []
    for slot in range(4):
        await client.write(protocol.command_stop(slot))
        stopped.append(slot)
    result = {"sent": "stop_all", "slots": stopped}
    _log_event("charger.stop_all", {}, result)
    return result


async def tool_export_session_log(args: dict[str, Any]) -> Any:
    _ = args
    return {"events": list(_session_log)}


def _profile_from_args(args: dict[str, Any]) -> dict[str, Any]:
    if "profile" in args and isinstance(args["profile"], dict):
        return dict(args["profile"])
    known_profiles = safety.profiles()
    if "profile_id" in args and str(args["profile_id"]) in known_profiles:
        return known_profiles[str(args["profile_id"])]
    excluded = {"slot", "dry_run", "confirmation_token"}
    return {key: value for key, value in args.items() if key not in excluded}


def _profile_schema(
    extra: dict[str, Any] | None = None,
    required: list[str] | None = None,
) -> dict[str, Any]:
    props: dict[str, Any] = {
        "slot_mask": {
            "type": "integer",
            "minimum": 1,
            "maximum": 15,
            "description": "slot bitmask: slot1=1, slot2=2, slot3=4, slot4=8",
        },
        "battery_type": {
            "type": "integer",
            "default": 0,
            "description": (
                "0 LiIon, 1 LiFe, 2 LiIo4.35, 3 NiMH, 4 NiCd, 5 NiZn, "
                "6 Eneloop, 7 RAM, 8 LTO, 9 Na-Lion"
            ),
        },
        "mode": {
            "type": "integer",
            "default": 0,
            "description": "0 charge, 1 refresh, 2 storage/break-in, 3 discharge, 4 cycle",
        },
        "capacity_mah": {"type": "integer", "default": 2000},
        "charge_current_ma": {"type": "integer", "default": 1000},
        "discharge_current_ma": {"type": "integer", "default": 500},
        "charge_stop_voltage_mv": {
            "type": "integer",
            "description": "Omit for chemistry default; NiMH/NiCd/Eneloop default is 1650mV.",
        },
        "discharge_stop_voltage_mv": {
            "type": "integer",
            "description": "Omit for chemistry default; NiMH/NiCd/Eneloop default is 1000mV.",
        },
        "charge_stop_current_ma": {
            "type": "integer",
            "description": "Omit for chemistry default; NiMH/NiCd/Eneloop default is 50mA.",
        },
        "discharge_stop_current_ma": {"type": "integer", "default": 500},
        "charge_rest_minutes": {"type": "integer", "default": 0},
        "cycle_count": {"type": "integer", "default": 1},
        "cycle_mode": {"oneOf": [{"type": "integer"}, {"type": "string"}], "default": 0},
        "negative_delta_mv": {"type": "integer", "default": 0},
        "trickle_current_ma": {"type": "integer", "default": 0},
        "keep_voltage_mv": {
            "type": "integer",
            "description": "Omit for chemistry default; NiMH/NiCd/Eneloop default is 1000mV.",
        },
        "temp_cutoff": {"type": "integer", "default": 45},
        "time_limit_minutes": {"type": "integer", "default": 0},
        "discharge_rest_minutes": {"type": "integer", "default": 0},
        "breakin": {"type": "boolean", "default": False},
    }
    if extra:
        props.update(extra)
    return {
        "type": "object",
        "properties": props,
        "required": ["slot_mask"] if required is None else required,
    }


TOOLS: dict[str, tuple[str, dict[str, Any], ToolHandler]] = {
    "mc3000_scan": (
        "Scan BLE devices and return devices whose advertised name matches the official "
        "MC3000 app filters.",
        {"type": "object", "properties": {"timeout": {"type": "number", "default": 5}}},
        tool_scan,
    ),
    "mc3000_connect": (
        "Connect to MC3000 by BLE address, or scan and connect to the first matching "
        "charger if address is omitted.",
        {
            "type": "object",
            "properties": {
                "address": {"type": "string"},
                "timeout": {"type": "number", "default": 5},
            },
        },
        tool_connect,
    ),
    "mc3000_disconnect": (
        "Disconnect from charger.",
        {"type": "object", "properties": {}},
        tool_disconnect,
    ),
    "mc3000_status": (
        "Poll slot status. Slot is zero-based 0..3; omit slot to poll all four.",
        {"type": "object", "properties": {"slot": {"type": "integer", "minimum": 0, "maximum": 3}}},
        tool_status,
    ),
    "mc3000_start": (
        "Start charging/program on a zero-based slot 0..3.",
        {
            "type": "object",
            "properties": {"slot": {"type": "integer", "minimum": 0, "maximum": 3}},
            "required": ["slot"],
        },
        tool_start,
    ),
    "mc3000_stop": (
        "Stop a zero-based slot 0..3.",
        {
            "type": "object",
            "properties": {"slot": {"type": "integer", "minimum": 0, "maximum": 3}},
            "required": ["slot"],
        },
        tool_stop,
    ),
    "mc3000_get_version": (
        "Read firmware/hardware version.",
        {
            "type": "object",
            "properties": {
                "mac": {
                    "type": "string",
                    "description": "Optional charger MAC to include exactly like the Android app.",
                }
            },
        },
        tool_get_version,
    ),
    "mc3000_get_basic": (
        "Read basic device settings.",
        {"type": "object", "properties": {}},
        tool_get_basic,
    ),
    "mc3000_set_basic": (
        "Set basic device settings.",
        {
            "type": "object",
            "properties": {
                "temp_unit": {
                    "type": "integer",
                    "description": "0=celsius, 1=fahrenheit",
                    "default": 0,
                },
                "system_beep": {"type": "boolean", "default": False},
                "display": {"type": "integer", "default": 0},
                "screensaver": {"type": "boolean", "default": False},
                "cooling_fan": {"type": "integer", "default": 0},
                "input_mv": {"type": "integer", "default": 11000},
            },
        },
        tool_set_basic,
    ),
    "mc3000_get_voltage_curve": (
        "Read voltage curve for a zero-based slot. Response is parsed into millivolt points.",
        {
            "type": "object",
            "properties": {
                "slot": {"type": "integer", "minimum": 0, "maximum": 3},
                "timeout": {"type": "number", "default": 8},
            },
            "required": ["slot"],
        },
        tool_get_voltage_curve,
    ),
    "mc3000_build_profile": (
        "Build a 40-byte MC3000 profile/program frame without sending it.",
        _profile_schema(),
        tool_build_profile,
    ),
    "mc3000_apply_profile": (
        "Apply a profile/program. Pass profile_hex from mc3000_build_profile or pass "
        "the same profile fields directly. Optionally start selected slots.",
        _profile_schema(
            extra={
                "profile_hex": {"type": "string"},
                "start": {"type": "boolean", "default": False},
            },
            required=[],
        ),
        tool_apply_profile,
    ),
    "mc3000_send_raw": (
        "Send a raw hex frame. Escape hatch for protocol experiments.",
        {
            "type": "object",
            "properties": {
                "hex": {"type": "string"},
                "frame_hex": {"type": "string"},
                "wait": {"type": "boolean", "default": False},
                "timeout": {"type": "number", "default": 3},
                "start": {"type": "boolean", "default": False},
            },
        },
        tool_send_raw,
    ),
    "charger.scan_devices": (
        "Read-only BLE scan for supported chargers.",
        {"type": "object", "properties": {"timeout": {"type": "number", "default": 5}}},
        tool_scan,
    ),
    "charger.connect": (
        "Connect to a selected charger. Prefer an explicit address from scan_devices.",
        {
            "type": "object",
            "properties": {
                "address": {"type": "string"},
                "timeout": {"type": "number", "default": 5},
            },
        },
        tool_connect,
    ),
    "charger.get_status": (
        "Read current charger status for all slots.",
        {"type": "object", "properties": {}},
        tool_status,
    ),
    "charger.read_slots": (
        "Read one slot or all slot statuses. Slot is zero-based 0..3.",
        {"type": "object", "properties": {"slot": {"type": "integer", "minimum": 0, "maximum": 3}}},
        tool_status,
    ),
    "charger.list_profiles": (
        "List bundled safe example profiles.",
        {"type": "object", "properties": {}},
        tool_list_profiles,
    ),
    "charger.validate_profile": (
        "Validate a battery profile against enforced chemistry/device/safety limits.",
        {
            "type": "object",
            "properties": {
                "profile": safety.profile_schema(),
                "profile_id": {"type": "string"},
            },
        },
        tool_validate_profile,
    ),
    "charger.apply_profile": (
        "Validate and apply a profile to a slot. Dry-run is default; live write requires token.",
        {
            "type": "object",
            "properties": {
                "slot": {"type": "integer", "minimum": 0, "maximum": 3},
                "profile": safety.profile_schema(),
                "profile_id": {"type": "string"},
                "dry_run": {"type": "boolean", "default": True},
                "confirmation_token": {"type": "string"},
            },
            "required": ["slot"],
        },
        tool_charger_apply_profile,
    ),
    "charger.start": (
        "Start the already-applied operation on a slot. Requires START_SLOT_<slot> token.",
        {
            "type": "object",
            "properties": {
                "slot": {"type": "integer", "minimum": 0, "maximum": 3},
                "confirmation_token": {"type": "string"},
            },
            "required": ["slot", "confirmation_token"],
        },
        tool_charger_start,
    ),
    "charger.stop_slot": (
        "Emergency stop for one slot; bypasses profile validation.",
        {
            "type": "object",
            "properties": {"slot": {"type": "integer", "minimum": 0, "maximum": 3}},
            "required": ["slot"],
        },
        tool_stop,
    ),
    "charger.stop_all": (
        "Emergency stop for all slots; bypasses profile validation.",
        {"type": "object", "properties": {}},
        tool_stop_all,
    ),
    "charger.export_session_log": (
        "Export in-memory session log with redacted device identifiers.",
        {"type": "object", "properties": {}},
        tool_export_session_log,
    ),
}


RESOURCES: dict[str, Callable[[], Any]] = {
    "charger://profiles": safety.profiles,
    "charger://profile-schema": safety.profile_schema,
    "charger://safety-limits": safety.safety_limits,
    "charger://device-capabilities": lambda: {
        "devices": ["SKYRC MC3000", "BLE-compatible MC3000 clones/rebrands"],
        "slots": 4,
        "transport": "Bluetooth Low Energy Nordic-UART-like FFE0/FFE1",
        "dangerous_tools_require_confirmation": True,
    },
    "charger://session-log/latest": lambda: {"events": list(_session_log)},
}

PROMPTS: dict[str, str] = {
    "charger_regular_charge": (
        "Collect chemistry, capacity_mAh, slot, nominal voltage, charge current, cutoff voltage, "
        "temperature policy, and source. Run charger.validate_profile, show dry-run, then ask "
        "for explicit START_SLOT_<slot> confirmation before charger.start."
    ),
    "charger_refresh_nimh": (
        "For NiMH refresh: verify NiMH chemistry/capacity/slot, use conservative currents, "
        "validate, dry-run, require explicit confirmation, then monitor status and temperature."
    ),
    "charger_break_in_nimh": (
        "Use only for known NiMH/NiCd cells that need forming. Ask why break-in is needed, "
        "validate C/10 charge and C/5 discharge, dry-run, confirm, monitor."
    ),
    "charger_lipo_storage": (
        "Refuse multi-cell packs unless the charger supports per-cell/balance handling. Validate "
        "cell count and storage voltage, dry-run by default, require explicit confirmation."
    ),
    "charger_emergency_stop": (
        "Call charger.stop_slot or charger.stop_all immediately; do not wait for "
        "profile validation."
    ),
}


async def handle(req: dict[str, Any]) -> dict[str, Any] | None:  # noqa: C901, PLR0912
    if "id" not in req:
        return None
    try:
        method = req.get("method")
        params = req.get("params") or {}
        result: dict[str, Any]
        if method == "initialize":
            result = {
                "protocolVersion": params.get("protocolVersion", "2024-11-05"),
                "capabilities": {"tools": {}, "resources": {}, "prompts": {}},
                "serverInfo": {"name": "mc3000-mcp", "version": "0.1.0"},
            }
        elif method == "tools/list":
            result = {
                "tools": [
                    {"name": name, "description": desc, "inputSchema": schema}
                    for name, (desc, schema, _) in TOOLS.items()
                ]
            }
        elif method == "tools/call":
            name = params.get("name")
            args = params.get("arguments") or {}
            if name not in TOOLS:
                raise ValueError(f"unknown tool: {name}")
            value = await TOOLS[name][2](args)
            result = _json_result(value)
        elif method == "resources/list":
            result = {
                "resources": [
                    {
                        "uri": uri,
                        "name": uri.removeprefix("charger://"),
                        "mimeType": "application/json",
                    }
                    for uri in RESOURCES
                ]
            }
        elif method == "resources/read":
            uri = str(params.get("uri", ""))
            if uri not in RESOURCES:
                raise ValueError(f"unknown resource: {uri}")
            result = {
                "contents": [
                    {
                        "uri": uri,
                        "mimeType": "application/json",
                        "text": json.dumps(RESOURCES[uri](), ensure_ascii=False, indent=2),
                    }
                ]
            }
        elif method == "prompts/list":
            result = {
                "prompts": [{"name": name, "description": text} for name, text in PROMPTS.items()]
            }
        elif method == "prompts/get":
            name = str(params.get("name", ""))
            if name not in PROMPTS:
                raise ValueError(f"unknown prompt: {name}")
            result = {
                "messages": [{"role": "user", "content": {"type": "text", "text": PROMPTS[name]}}]
            }
        elif method in ("ping", "notifications/initialized"):
            result = {}
        else:
            raise ValueError(f"unsupported method: {method}")
        return {"jsonrpc": "2.0", "id": req["id"], "result": result}
    except BleakMissingError as exc:
        return {"jsonrpc": "2.0", "id": req["id"], "error": {"code": -32000, "message": str(exc)}}
    except Exception as exc:
        return {
            "jsonrpc": "2.0",
            "id": req["id"],
            "error": {"code": -32603, "message": str(exc), "data": traceback.format_exc()},
        }


async def amain() -> None:
    while True:
        line = await asyncio.to_thread(sys.stdin.readline)
        if line == "":
            break
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except json.JSONDecodeError as exc:
            resp: dict[str, Any] | None = {
                "jsonrpc": "2.0",
                "id": None,
                "error": {"code": -32700, "message": str(exc)},
            }
        else:
            resp = await handle(req)
        if resp is not None:
            print(json.dumps(resp, ensure_ascii=False), flush=True)


def main() -> None:
    asyncio.run(amain())


if __name__ == "__main__":
    main()
