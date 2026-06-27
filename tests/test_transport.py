from __future__ import annotations

import asyncio
from typing import TYPE_CHECKING, Any, cast

import pytest

from mc3000_mcp import protocol, transport

if TYPE_CHECKING:
    from collections.abc import Callable


class FakeBleDevice:
    def __init__(self, address: str, name: str, rssi: int = -42) -> None:
        self.address = address
        self.name = name
        self.rssi = rssi
        self.details = {"fake": True}


class FakeScanner:
    @staticmethod
    async def discover(timeout: float = 5.0, return_adv: bool = False) -> list[FakeBleDevice]:
        _ = (timeout, return_adv)
        return [
            FakeBleDevice("AA:BB:CC:DD:EE:01", "Keyboard"),
            FakeBleDevice("AA:BB:CC:DD:EE:02", "Charger"),
        ]


class EmptyScanner:
    @staticmethod
    async def discover(timeout: float = 5.0, return_adv: bool = False) -> list[FakeBleDevice]:
        _ = (timeout, return_adv)
        return []


class FakeBleakClient:
    def __init__(self, address: str) -> None:
        self.address = address
        self.is_connected = False
        self.writes: list[bytes] = []
        self.notify_uuid: str | None = None
        self.callback: Callable[[Any, bytearray], None] | None = None

    async def connect(self) -> None:
        self.is_connected = True

    async def disconnect(self) -> None:
        self.is_connected = False

    async def start_notify(
        self,
        uuid: str,
        callback: Callable[[Any, bytearray], None],
    ) -> None:
        self.notify_uuid = uuid
        self.callback = callback

    async def stop_notify(self, uuid: str) -> None:
        _ = uuid
        self.notify_uuid = None

    async def write_gatt_char(self, uuid: str, data: bytes, response: bool = False) -> None:
        _ = (uuid, response)
        self.writes.append(bytes(data))


class StopNotifyFailClient(FakeBleakClient):
    async def stop_notify(self, uuid: str) -> None:
        _ = uuid
        raise RuntimeError("notify already stopped")


def test_scan_filters_apk_device_names(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(transport, "BleakScanner", FakeScanner)
    found = asyncio.run(transport.MC3000Client.scan(timeout=0.01))
    assert len(found) == 1
    assert found[0].name == "Charger"
    assert found[0].to_dict()["address"] == "AA:BB:CC:DD:EE:02"


def test_missing_bleak_errors(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(transport, "BleakClient", None)
    monkeypatch.setattr(transport, "BleakScanner", None)
    with pytest.raises(transport.BleakMissingError, match="bleak"):
        transport.MC3000Client()
    with pytest.raises(transport.BleakMissingError, match="bleak"):
        asyncio.run(transport.MC3000Client.scan())


def test_connect_starts_notify(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(transport, "BleakClient", FakeBleakClient)
    monkeypatch.setattr(transport, "BleakScanner", FakeScanner)
    client = transport.MC3000Client()
    assert client.is_connected() is False
    result = asyncio.run(client.connect("AA:BB:CC:DD:EE:02"))
    fake_client = cast("FakeBleakClient", client.client)
    assert result["connected"] is True
    assert client.is_connected() is True
    assert fake_client.notify_uuid == protocol.CHAR_UUID
    assert asyncio.run(client.disconnect()) == {"connected": False}


def test_find_and_connect_scans_or_raises(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(transport, "BleakClient", FakeBleakClient)
    monkeypatch.setattr(transport, "BleakScanner", FakeScanner)
    client = transport.MC3000Client()
    result = asyncio.run(transport.find_and_connect(client))
    assert result["address"] == "AA:BB:CC:DD:EE:02"

    monkeypatch.setattr(transport, "BleakScanner", EmptyScanner)
    with pytest.raises(RuntimeError, match="No MC3000-like"):
        asyncio.run(transport.find_and_connect(transport.MC3000Client()))


def test_find_and_connect_with_explicit_address(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(transport, "BleakClient", FakeBleakClient)
    monkeypatch.setattr(transport, "BleakScanner", FakeScanner)
    client = transport.MC3000Client()
    result = asyncio.run(transport.find_and_connect(client, address="AA:BB"))
    assert result["address"] == "AA:BB"


def test_write_requires_connection(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(transport, "BleakClient", FakeBleakClient)
    monkeypatch.setattr(transport, "BleakScanner", FakeScanner)
    client = transport.MC3000Client()
    with pytest.raises(RuntimeError, match="not connected"):
        asyncio.run(client.write(b"x"))


def test_disconnect_suppresses_stop_notify_error(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(transport, "BleakClient", StopNotifyFailClient)
    monkeypatch.setattr(transport, "BleakScanner", FakeScanner)
    client = transport.MC3000Client()
    asyncio.run(client.connect("AA:BB:CC:DD:EE:02"))
    assert asyncio.run(client.disconnect()) == {"connected": False}


def test_wait_for_status_and_timeout(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(transport, "BleakClient", FakeBleakClient)
    monkeypatch.setattr(transport, "BleakScanner", FakeScanner)
    client = transport.MC3000Client()
    frame = bytearray(protocol.command_get_status(0))
    frame[6] = 1
    client._on_notify(None, bytearray(protocol.with_checksum(frame)))  # noqa: SLF001
    parsed = asyncio.run(client.wait_for(protocol.Opcode.STATUS, timeout=0.01))
    assert parsed["kind"] == "status"

    with pytest.raises(TimeoutError, match="timeout waiting"):
        asyncio.run(client.wait_for(protocol.Opcode.STATUS, timeout=0.001))

    with pytest.raises(TimeoutError, match="timeout waiting"):
        asyncio.run(client.wait_for(protocol.Opcode.STATUS, timeout=-1.0))


def test_request_poll_and_kind_mismatch_paths(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(transport, "BleakClient", FakeBleakClient)
    monkeypatch.setattr(transport, "BleakScanner", FakeScanner)
    client = transport.MC3000Client()
    asyncio.run(client.connect("AA:BB:CC:DD:EE:02"))
    fake_client = cast("FakeBleakClient", client.client)

    client._on_notify(None, bytearray(protocol.command_get_basic()))  # noqa: SLF001
    parsed = asyncio.run(client.request(protocol.command_get_basic()))
    assert parsed["kind"] == "basic"
    assert fake_client.writes[-1] == protocol.command_get_basic()

    client._on_notify(None, bytearray(b"\x0f\x99"))  # noqa: SLF001
    with pytest.raises(TimeoutError, match="timeout waiting"):
        asyncio.run(client.wait_for(protocol.Opcode.STATUS, timeout=0.01))

    client._on_notify(None, bytearray(b"\x0f\x99"))  # noqa: SLF001
    assert asyncio.run(client.wait_for(None, timeout=0.01))["kind"] == "unknown"

    curve_client = transport.MC3000Client()
    curve = bytearray(protocol.CURVE_LEN)
    curve[0] = 0x0F
    curve[1] = protocol.Opcode.VOLTAGE_CURVE
    curve_client._on_notify(None, bytearray(curve[:20]))  # noqa: SLF001
    curve_client._on_notify(None, bytearray(curve[20:40]))  # noqa: SLF001
    with pytest.raises(TimeoutError, match="timeout waiting"):
        asyncio.run(curve_client.wait_for(protocol.Opcode.VOLTAGE_CURVE, timeout=0.01))


def test_poll_status_and_all_status(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(transport, "BleakClient", FakeBleakClient)
    monkeypatch.setattr(transport, "BleakScanner", FakeScanner)
    client = transport.MC3000Client()
    asyncio.run(client.connect("AA:BB:CC:DD:EE:02"))
    for slot in range(4):
        frame = bytearray(protocol.command_get_status(slot))
        frame[2] = slot
        frame[6] = 1
        client._on_notify(None, bytearray(protocol.with_checksum(frame)))  # noqa: SLF001
    result = asyncio.run(client.poll_all_status())
    assert [item["status"]["slot"] for item in result] == [0, 1, 2, 3]


def test_wait_for_voltage_curve_fragments(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(transport, "BleakClient", FakeBleakClient)
    monkeypatch.setattr(transport, "BleakScanner", FakeScanner)
    client = transport.MC3000Client()
    curve = bytearray(protocol.CURVE_LEN)
    curve[0] = 0x0F
    curve[1] = protocol.Opcode.VOLTAGE_CURVE
    curve[2] = 3
    curve[3] = 0
    curve[4] = 1
    curve[5] = 0x04
    curve[6] = 0xD2
    client._on_notify(None, bytearray(curve[:20]))  # noqa: SLF001
    client._on_notify(None, bytearray(curve[20:]))  # noqa: SLF001
    parsed = asyncio.run(client.wait_for(protocol.Opcode.VOLTAGE_CURVE, timeout=0.01))
    assert parsed["kind"] == "voltage_curve"
    assert parsed["slot"] == 3
    assert parsed["points_mv"][0] == 1234


def test_wait_for_voltage_curve_single_full_notification(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(transport, "BleakClient", FakeBleakClient)
    monkeypatch.setattr(transport, "BleakScanner", FakeScanner)
    client = transport.MC3000Client()
    curve = bytearray(protocol.CURVE_LEN)
    curve[0] = 0x0F
    curve[1] = protocol.Opcode.VOLTAGE_CURVE
    curve[2] = 1
    client._on_notify(None, bytearray(curve))  # noqa: SLF001
    parsed = asyncio.run(client.wait_for(protocol.Opcode.VOLTAGE_CURVE, timeout=0.01))
    assert parsed["kind"] == "voltage_curve"


def test_send_profile_e2e_write_sequence(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(transport, "BleakClient", FakeBleakClient)
    monkeypatch.setattr(transport, "BleakScanner", FakeScanner)
    client = transport.MC3000Client()
    asyncio.run(client.connect("AA:BB:CC:DD:EE:02"))
    profile = protocol.build_profile(slot_mask=0b0011)
    result = asyncio.run(client.send_profile(profile, start=True))
    fake_client = cast("FakeBleakClient", client.client)
    assert result["stopped_slots"] == [0, 1]
    assert result["started"] is True
    writes = fake_client.writes
    assert writes[0] == protocol.command_stop(0)
    assert writes[1] == protocol.command_stop(1)
    assert writes[2] == profile[:20]
    assert writes[3] == profile[20:]
    assert writes[4] == protocol.command_start(0)
    assert writes[5] == protocol.command_start(1)
