from __future__ import annotations

import asyncio
import importlib
from contextlib import suppress
from dataclasses import asdict, dataclass
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from types import ModuleType

from . import protocol

try:
    _bleak: ModuleType | None = importlib.import_module("bleak")
except ImportError:  # pragma: no cover - exercised only when bleak is absent
    _bleak = None

BleakClient: Any = None if _bleak is None else _bleak.BleakClient
BleakScanner: Any = None if _bleak is None else _bleak.BleakScanner


@dataclass(slots=True)
class FoundDevice:
    address: str
    name: str | None
    rssi: int | None
    details: str | None = None

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


class BleakMissingError(RuntimeError):
    pass


class MC3000Client:
    def __init__(self) -> None:
        if BleakClient is None or BleakScanner is None:
            raise BleakMissingError(
                "Python package 'bleak' is not installed. Run: "
                "python -m pip install -e '.[test]' or python -m pip install bleak",
            )
        self.client: Any | None = None
        self.address: str | None = None
        self._queue: asyncio.Queue[bytes] = asyncio.Queue()
        self._curve_buffer = bytearray()

    @staticmethod
    async def scan(timeout: float = 5.0) -> list[FoundDevice]:
        if BleakScanner is None:
            raise BleakMissingError("Python package 'bleak' is not installed")
        devices = await BleakScanner.discover(timeout=timeout, return_adv=False)
        found: list[FoundDevice] = []
        for dev in devices:
            name = getattr(dev, "name", None)
            if name in protocol.DEVICE_NAMES or (
                name and any(device_name in name for device_name in protocol.DEVICE_NAMES)
            ):
                found.append(
                    FoundDevice(
                        address=dev.address,
                        name=name,
                        rssi=getattr(dev, "rssi", None),
                        details=str(dev.details),
                    ),
                )
        return found

    async def connect(self, address: str) -> dict[str, Any]:
        await self.disconnect()
        self.address = address
        self.client = BleakClient(address)
        await self.client.connect()
        await self.client.start_notify(protocol.CHAR_UUID, self._on_notify)
        return {"connected": True, "address": address}

    async def disconnect(self) -> dict[str, Any]:
        if self.client is not None:
            try:
                if self.client.is_connected:
                    with suppress(Exception):
                        await self.client.stop_notify(protocol.CHAR_UUID)
                    await self.client.disconnect()
            finally:
                self.client = None
        return {"connected": False}

    def is_connected(self) -> bool:
        return bool(self.client and self.client.is_connected)

    async def write(self, data: bytes) -> None:
        if not self.client or not self.client.is_connected:
            raise RuntimeError("MC3000 is not connected")
        await self.client.write_gatt_char(protocol.CHAR_UUID, data, response=False)

    async def request(
        self,
        frame: bytes,
        expect_opcode: int | None = None,
        timeout: float = 3.0,
    ) -> dict[str, Any]:
        await self.write(frame)
        return await self.wait_for(
            expect_opcode=expect_opcode if expect_opcode is not None else frame[1],
            timeout=timeout,
        )

    async def wait_for(
        self,
        expect_opcode: int | None = None,
        timeout: float = 3.0,
    ) -> dict[str, Any]:
        deadline = asyncio.get_running_loop().time() + timeout
        while True:
            left = deadline - asyncio.get_running_loop().time()
            if left <= 0:
                raise TimeoutError(f"timeout waiting for opcode {expect_opcode}")
            try:
                data = await asyncio.wait_for(self._queue.get(), timeout=left)
            except TimeoutError as exc:
                raise TimeoutError(f"timeout waiting for opcode {expect_opcode}") from exc
            # Voltage curve is fragmented into 20-byte notifications until 245 bytes.
            if (
                len(data) >= protocol.MIN_OPCODE_FRAME_LEN
                and data[1] == protocol.Opcode.VOLTAGE_CURVE
            ):
                self._curve_buffer = bytearray(data)
                if len(self._curve_buffer) < protocol.CURVE_LEN:
                    continue
                parsed = protocol.parse_notification(
                    bytes(self._curve_buffer[: protocol.CURVE_LEN]),
                )
                self._curve_buffer.clear()
            elif self._curve_buffer:
                self._curve_buffer.extend(data)
                if len(self._curve_buffer) < protocol.CURVE_LEN:
                    continue
                parsed = protocol.parse_notification(
                    bytes(self._curve_buffer[: protocol.CURVE_LEN]),
                )
                self._curve_buffer.clear()
            else:
                parsed = protocol.parse_notification(data)
            if (
                expect_opcode is None
                or parsed.get("opcode") == expect_opcode
                or _kind_matches(expect_opcode, parsed)
            ):
                return parsed

    async def poll_status(self, slot: int, *, mc5000: bool = False) -> dict[str, Any]:
        if mc5000:
            return await self.request(
                protocol.command_get_mc5000_status(slot),
                protocol.Opcode.MC5000_SLOT_STATUS,
            )
        return await self.request(protocol.command_get_status(slot), protocol.Opcode.STATUS)

    async def poll_all_status(self) -> list[dict[str, Any]]:
        result = []
        for slot in range(4):
            result.append(await self.poll_status(slot))
            await asyncio.sleep(0.05)
        return result

    async def send_profile(self, profile: bytes, *, start: bool = False) -> dict[str, Any]:
        first, second = protocol.split_profile(profile)
        slot_mask = profile[2]
        # APK stops selected slot before applying the profile. It only uses the first
        # selected slot from parameter_1[2] in UI flows; here stop all selected slots.
        stopped = []
        for slot in range(4):
            if slot_mask & (1 << slot):
                await self.write(protocol.command_stop(slot))
                stopped.append(slot)
                await asyncio.sleep(0.5)
        await self.write(first)
        await asyncio.sleep(0.05)
        await self.write(second)
        if start:
            await asyncio.sleep(0.5)
            for slot in range(4):
                if slot_mask & (1 << slot):
                    await self.write(protocol.command_start(slot))
        return {
            "ok": True,
            "stopped_slots": stopped,
            "started": bool(start),
            "profile_hex": profile.hex().upper(),
        }

    def _on_notify(self, _sender: Any, data: bytearray) -> None:
        self._queue.put_nowait(bytes(data))


async def find_and_connect(
    client: MC3000Client,
    address: str | None = None,
    timeout: float = 5.0,
) -> dict[str, Any]:
    if address:
        return await client.connect(address)
    devices = await MC3000Client.scan(timeout=timeout)
    if not devices:
        raise RuntimeError(f"No MC3000-like BLE device found by names {protocol.DEVICE_NAMES}")
    return await client.connect(devices[0].address)


def _kind_matches(opcode: int, parsed: dict[str, Any]) -> bool:
    return (
        (opcode == protocol.Opcode.STATUS and parsed.get("kind") == "status")
        or (opcode == protocol.Opcode.GET_BASIC and parsed.get("kind") == "basic")
        or (opcode == protocol.Opcode.VERSION and parsed.get("kind") == "version")
        or (opcode == protocol.Opcode.VOLTAGE_CURVE and parsed.get("kind") == "voltage_curve")
        or (opcode == protocol.Opcode.MC5000_SLOT_STATUS and parsed.get("kind") == "status")
    )
