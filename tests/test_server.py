from __future__ import annotations

import asyncio
import io
import json
import runpy
import subprocess
import sys
import warnings
from typing import TYPE_CHECKING, Any, cast

if TYPE_CHECKING:
    import pytest

from mc3000_mcp import protocol, server
from mc3000_mcp.transport import BleakMissingError

JsonDict = dict[str, Any]


def call_tool(name: str, arguments: JsonDict | None = None) -> JsonDict:
    req: JsonDict = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": name, "arguments": arguments or {}},
    }
    response = asyncio.run(server.handle(req))
    assert response is not None
    return response


def result_text(response: JsonDict) -> str:
    return cast("str", response["result"]["content"][0]["text"])


class FakeServerClient:
    def __init__(self) -> None:
        self.writes: list[bytes] = []
        self.disconnected = False
        self.sent_profiles: list[tuple[bytes, bool]] = []

    async def disconnect(self) -> dict[str, Any]:
        self.disconnected = True
        return {"connected": False}

    async def poll_status(self, slot: int) -> dict[str, Any]:
        return {"kind": "status", "status": {"slot": slot}}

    async def poll_all_status(self) -> list[dict[str, Any]]:
        return [await self.poll_status(slot) for slot in range(4)]

    async def write(self, data: bytes) -> None:
        self.writes.append(data)

    async def request(
        self,
        frame: bytes,
        expect_opcode: int | None = None,
        timeout: float = 3.0,
    ) -> dict[str, Any]:
        _ = timeout
        if expect_opcode == protocol.Opcode.VOLTAGE_CURVE:
            curve = bytearray(protocol.CURVE_LEN)
            curve[0] = 0x0F
            curve[1] = protocol.Opcode.VOLTAGE_CURVE
            curve[2] = frame[2]
            curve[3] = 0
            curve[4] = 5
            for idx, mv in enumerate((4100, 4110, 4120)):
                pos = 5 + idx * 2
                curve[pos] = mv // 256
                curve[pos + 1] = mv % 256
            return protocol.parse_voltage_curve(bytes(curve))
        return {
            "opcode": expect_opcode if expect_opcode is not None else frame[1],
            "frame": frame.hex(),
        }

    async def wait_for(
        self,
        expect_opcode: int | None = None,
        timeout: float = 3.0,
    ) -> dict[str, Any]:
        return {"opcode": expect_opcode, "timeout": timeout}

    async def send_profile(self, profile: bytes, *, start: bool = False) -> dict[str, Any]:
        self.sent_profiles.append((profile, start))
        return {"ok": True, "profile_hex": profile.hex().upper(), "started": start}


def test_tools_list_contains_core_tools() -> None:
    response = asyncio.run(
        server.handle(
            {"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}},
        ),
    )
    assert response is not None
    names = {tool["name"] for tool in response["result"]["tools"]}
    assert "mc3000_status" in names
    assert "mc3000_build_profile" in names
    assert "mc3000_apply_profile" in names
    assert "charger.validate_profile" in names
    assert "charger.apply_profile" in names
    assert "charger.stop_all" in names


def test_build_profile_tool_is_json_serializable() -> None:
    response = call_tool("mc3000_build_profile", {"slot_mask": 1})
    payload = json.loads(result_text(response))
    assert payload["profile_hex"].startswith("0F11")
    assert len(payload["chunk_1_hex"]) == 40
    assert len(payload["chunk_2_hex"]) == 40


def test_unknown_tool_returns_jsonrpc_error() -> None:
    response = call_tool("does_not_exist")
    assert response["error"]["code"] == -32603
    assert "unknown tool" in response["error"]["message"]


def test_handle_initialize_ping_notification_and_bad_method() -> None:
    init = asyncio.run(
        server.handle({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
    )
    assert init is not None
    assert init["result"]["serverInfo"]["name"] == "mc3000-mcp"
    ping = asyncio.run(server.handle({"jsonrpc": "2.0", "id": 2, "method": "ping"}))
    assert ping is not None
    assert ping["result"] == {}
    notify = asyncio.run(server.handle({"jsonrpc": "2.0", "method": "ping"}))
    assert notify is None
    bad = asyncio.run(server.handle({"jsonrpc": "2.0", "id": 3, "method": "nope"}))
    assert bad is not None
    assert bad["error"]["code"] == -32603


def test_bleak_missing_error_is_mapped(monkeypatch: pytest.MonkeyPatch) -> None:
    def fail_client() -> None:
        raise BleakMissingError("missing")

    monkeypatch.setattr(server, "_client", None)
    monkeypatch.setattr(server, "MC3000Client", fail_client)
    response = call_tool("mc3000_status")
    assert response["error"]["code"] == -32000


def test_all_tool_handlers_with_fake_client(monkeypatch: pytest.MonkeyPatch) -> None:
    fake = FakeServerClient()
    monkeypatch.setattr(server, "_client", fake)

    payload = json.loads(result_text(call_tool("mc3000_status", {"slot": 2})))
    assert payload["status"]["slot"] == 2
    payload = json.loads(result_text(call_tool("mc3000_status")))
    assert len(payload["slots"]) == 4

    assert json.loads(result_text(call_tool("mc3000_start", {"slot": 1})))["sent"] == "start"
    assert json.loads(result_text(call_tool("mc3000_stop", {"slot": 1})))["sent"] == "stop"
    assert (
        json.loads(result_text(call_tool("mc3000_get_version", {"mac": "AA:BB:CC:DD:EE:FF"})))[
            "opcode"
        ]
        == protocol.Opcode.VERSION
    )
    assert (
        json.loads(result_text(call_tool("mc3000_get_basic")))["opcode"]
        == protocol.Opcode.GET_BASIC
    )
    assert (
        json.loads(result_text(call_tool("mc3000_set_basic", {"system_beep": True})))["opcode"]
        == protocol.Opcode.SET_BASIC
    )
    assert json.loads(result_text(call_tool("mc3000_get_voltage_curve", {"slot": 0})))["slot"] == 0
    charger_curve = json.loads(result_text(call_tool("charger.get_voltage_curve", {"slot": 0})))
    assert charger_curve["source"] == "charger_voltage_curve_command_0x56"
    assert charger_curve["time_series"][1] == {"index": 1, "time_seconds": 5.0, "voltage_mv": 4110}
    curve_csv = json.loads(
        result_text(call_tool("charger.export_voltage_curve", {"slot": 0, "format": "csv"})),
    )
    assert curve_csv["content"].splitlines()[0] == "index,time_seconds,voltage_mv,voltage_v"
    assert "1,5.0,4110,4.110" in curve_csv["content"]
    curve_json = json.loads(
        result_text(call_tool("charger.export_voltage_curve", {"slot": 0, "format": "json"})),
    )
    assert curve_json["content"]["point_count"] == 3
    bad_curve_format = call_tool("charger.export_voltage_curve", {"slot": 0, "format": "xlsx"})
    assert bad_curve_format["error"]["code"] == -32603

    profile = protocol.build_profile(slot_mask=1)
    apply_payload = json.loads(
        result_text(
            call_tool("mc3000_apply_profile", {"profile_hex": profile.hex(), "start": True}),
        ),
    )
    assert apply_payload["started"] is True
    built_apply = json.loads(result_text(call_tool("mc3000_apply_profile", {"slot_mask": 1})))
    assert built_apply["ok"] is True
    raw_profile = json.loads(result_text(call_tool("mc3000_send_raw", {"hex": profile.hex()})))
    assert raw_profile["ok"] is True
    raw_frame = json.loads(
        result_text(call_tool("mc3000_send_raw", {"hex": "0F0500", "wait": False})),
    )
    assert raw_frame["sent_hex"] == "0F0500"
    raw_wait = json.loads(
        result_text(call_tool("mc3000_send_raw", {"hex": "0F0500", "wait": True})),
    )
    assert raw_wait["timeout"] == 3.0
    assert asyncio.run(server.tool_disconnect({})) == {"connected": False}
    assert fake.disconnected is True


def test_safe_charger_tools_with_fake_client(monkeypatch: pytest.MonkeyPatch) -> None:
    fake = FakeServerClient()
    monkeypatch.setattr(server, "_client", fake)
    profile_id = "nimh-aa-conservative-charge"

    profiles = json.loads(result_text(call_tool("charger.list_profiles")))
    assert profile_id in profiles["profiles"]

    validation = json.loads(
        result_text(call_tool("charger.validate_profile", {"profile_id": profile_id})),
    )
    assert validation["ok"] is True

    dry_run = json.loads(
        result_text(call_tool("charger.apply_profile", {"slot": 3, "profile_id": profile_id})),
    )
    assert dry_run["dry_run"] is True
    assert dry_run["applied"] is False
    assert "profile_hex" in dry_run

    missing_token = call_tool(
        "charger.apply_profile",
        {"slot": 3, "profile_id": profile_id, "dry_run": False},
    )
    assert missing_token["error"]["code"] == -32603
    live = json.loads(
        result_text(
            call_tool(
                "charger.apply_profile",
                {
                    "slot": 3,
                    "profile_id": profile_id,
                    "dry_run": False,
                    "confirmation_token": "APPLY_PROFILE_SLOT_3",
                },
            ),
        ),
    )
    assert live["applied"] is True

    start_missing_token = call_tool("charger.start", {"slot": 3})
    assert start_missing_token["error"]["code"] == -32603
    started = json.loads(
        result_text(call_tool("charger.start", {"slot": 3, "confirmation_token": "START_SLOT_3"})),
    )
    assert started["sent"] == "start"
    assert json.loads(result_text(call_tool("charger.stop_all")))["slots"] == [0, 1, 2, 3]
    assert json.loads(result_text(call_tool("charger.export_session_log")))["events"]

    async def raw_curve(args: JsonDict) -> JsonDict:
        _ = args
        return {"opcode": int(protocol.Opcode.VOLTAGE_CURVE)}

    monkeypatch.setattr(server, "tool_get_voltage_curve", raw_curve)
    fallback_curve = asyncio.run(server.tool_charger_get_voltage_curve({"slot": 0}))
    assert fallback_curve == {"opcode": int(protocol.Opcode.VOLTAGE_CURVE)}

    bad_apply = json.loads(
        result_text(
            call_tool(
                "charger.apply_profile",
                {"slot": 3, "profile": {"profile_id": "bad", "chemistry": "Mystery"}},
            ),
        ),
    )
    assert bad_apply["validation"]["ok"] is False
    raw_profile = json.loads(
        result_text(
            call_tool(
                "charger.validate_profile",
                safety_profile_without_wrapper(),
            ),
        ),
    )
    assert raw_profile["ok"] is True


def safety_profile_without_wrapper() -> JsonDict:
    return {
        "profile_id": "nimh-aa-direct",
        "chemistry": "NiMH",
        "capacity_mah": 2600,
        "cell_count": 1,
        "nominal_voltage_mv": 1200,
        "operation": "charge",
        "charge_current_ma": 260,
        "discharge_current_ma": 200,
        "charge_voltage_mv": 1650,
        "discharge_cutoff_mv": 1000,
        "timeout_minutes": 960,
        "temperature_cutoff_c": 45,
        "temperature_policy": "supervised",
        "source": "test",
        "risk_level": "low",
    }


def test_resources_and_prompts() -> None:
    resources = asyncio.run(
        server.handle({"jsonrpc": "2.0", "id": 1, "method": "resources/list", "params": {}}),
    )
    assert resources is not None
    assert any(
        item["uri"] == "charger://profile-schema" for item in resources["result"]["resources"]
    )
    profile_schema = asyncio.run(
        server.handle(
            {
                "jsonrpc": "2.0",
                "id": 2,
                "method": "resources/read",
                "params": {"uri": "charger://profile-schema"},
            },
        ),
    )
    assert profile_schema is not None
    assert "Charger Battery Profile" in profile_schema["result"]["contents"][0]["text"]

    prompts = asyncio.run(
        server.handle({"jsonrpc": "2.0", "id": 3, "method": "prompts/list", "params": {}}),
    )
    assert prompts is not None
    assert any(item["name"] == "charger_emergency_stop" for item in prompts["result"]["prompts"])
    prompt = asyncio.run(
        server.handle(
            {
                "jsonrpc": "2.0",
                "id": 4,
                "method": "prompts/get",
                "params": {"name": "charger_emergency_stop"},
            },
        ),
    )
    assert prompt is not None
    assert "stop_all" in prompt["result"]["messages"][0]["content"]["text"]

    bad_resource = asyncio.run(
        server.handle(
            {
                "jsonrpc": "2.0",
                "id": 5,
                "method": "resources/read",
                "params": {"uri": "charger://missing"},
            },
        ),
    )
    assert bad_resource is not None
    assert bad_resource["error"]["code"] == -32603
    bad_prompt = asyncio.run(
        server.handle(
            {
                "jsonrpc": "2.0",
                "id": 6,
                "method": "prompts/get",
                "params": {"name": "missing"},
            },
        ),
    )
    assert bad_prompt is not None
    assert bad_prompt["error"]["code"] == -32603


def test_disconnect_without_client(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(server, "_client", None)
    assert asyncio.run(server.tool_disconnect({})) == {"connected": False}


def test_tool_scan_and_connect(monkeypatch: pytest.MonkeyPatch) -> None:
    class ScanClient:
        @staticmethod
        async def scan(timeout: float = 5.0) -> list[Any]:
            _ = timeout
            return []

    async def fake_find(
        client: Any,
        address: str | None = None,
        timeout: float = 5.0,
    ) -> dict[str, Any]:
        _ = (client, timeout)
        return {"connected": True, "address": address}

    monkeypatch.setattr(server, "MC3000Client", ScanClient)
    assert asyncio.run(server.tool_scan({"timeout": 0.1}))["devices"] == []
    monkeypatch.setattr(server, "_client", FakeServerClient())
    monkeypatch.setattr(server, "find_and_connect", fake_find)
    assert asyncio.run(server.tool_connect({"address": "AA"}))["address"] == "AA"


def test_text_result() -> None:
    assert server._text_result("ok")["content"][0]["text"] == "ok"  # noqa: SLF001
    server._log_event("x", {"address": "AA:BB"}, {"ok": True})  # noqa: SLF001
    assert server._session_log[-1]["arguments"]["address"] == "<redacted-device-id>"  # noqa: SLF001


def test_stdio_e2e_initialize_list_and_build_profile() -> None:
    input_data = "\n".join(
        [
            json.dumps(
                {
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {"protocolVersion": "2024-11-05"},
                },
            ),
            json.dumps({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
            json.dumps(
                {
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "tools/call",
                    "params": {"name": "mc3000_build_profile", "arguments": {"slot_mask": 1}},
                },
            ),
            "not-json",
            "",
        ],
    )
    proc = subprocess.run(
        [sys.executable, "-m", "mc3000_mcp.server"],
        input=input_data,
        text=True,
        capture_output=True,
        timeout=10,
        check=True,
    )
    lines = [json.loads(line) for line in proc.stdout.splitlines()]
    assert lines[0]["result"]["serverInfo"]["name"] == "mc3000-mcp"
    assert any(tool["name"] == "mc3000_status" for tool in lines[1]["result"]["tools"])
    assert "profile_hex" in lines[2]["result"]["content"][0]["text"]
    assert lines[3]["error"]["code"] == -32700


def test_amain_direct(monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]) -> None:
    monkeypatch.setattr(sys, "stdin", io.StringIO('{"jsonrpc":"2.0","id":1,"method":"ping"}\n\n'))
    asyncio.run(server.amain())
    assert json.loads(capsys.readouterr().out)["result"] == {}


def test_amain_invalid_json_direct(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    monkeypatch.setattr(sys, "stdin", io.StringIO("not-json\n"))
    asyncio.run(server.amain())
    assert json.loads(capsys.readouterr().out)["error"]["code"] == -32700


def test_main(monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]) -> None:
    monkeypatch.setattr(sys, "stdin", io.StringIO('{"jsonrpc":"2.0","id":1,"method":"ping"}\n'))
    server.main()
    assert json.loads(capsys.readouterr().out)["id"] == 1


def test_module_entrypoint(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    monkeypatch.setattr(sys, "stdin", io.StringIO('{"jsonrpc":"2.0","id":1,"method":"ping"}\n'))
    with warnings.catch_warnings():
        warnings.filterwarnings(
            "ignore", message=".*found in sys.modules.*", category=RuntimeWarning
        )
        runpy.run_module("mc3000_mcp.server", run_name="__main__")
    assert json.loads(capsys.readouterr().out)["id"] == 1
