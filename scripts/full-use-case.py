#!/usr/bin/env python3
"""Run the complete supervised MC3000 GP AA charging use case.

The service must only run when `/var/lib/mc3000-full-usecase.armed` exists.  The arm
file is consumed at startup, so an accidental later reboot cannot start charging.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from collections.abc import Iterator
from typing import Any

SERVER = "/usr/bin/mc3000-mcp"
ARM_FILE = "/var/lib/mc3000-full-usecase.armed"
LOG_FILE = "/var/log/mc3000-full-usecase.log"
JSONL_FILE = "/var/log/mc3000-full-usecase.jsonl"
ADDR = "F8:2E:0C:B8:A6:15"
PROFILE_ID = "nimh-aa-conservative-charge"


def log(message: str, **fields: Any) -> None:
    stamp = time.strftime("%Y-%m-%dT%H:%M:%S")
    suffix = "".join(f" {key}={value}" for key, value in fields.items())
    line = f"{stamp} {message}{suffix}"
    print(line, flush=True)
    with open(LOG_FILE, "a", encoding="utf-8") as handle:
        handle.write(f"{line}\n")


def event(name: str, payload: dict[str, Any]) -> None:
    record = {"at": time.time(), "event": name, **payload}
    with open(JSONL_FILE, "a", encoding="utf-8") as handle:
        handle.write(json.dumps(record, ensure_ascii=False) + "\n")
    log(name, **payload)


class Mcp:
    def __init__(self) -> None:
        self.process = subprocess.Popen(
            [SERVER],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
        )
        self.next_id = 1

    def _request(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        request_id = self.next_id
        self.next_id += 1
        request = {"jsonrpc": "2.0", "id": request_id, "method": method}
        if params is not None:
            request["params"] = params
        assert self.process.stdin is not None
        assert self.process.stdout is not None
        self.process.stdin.write(json.dumps(request) + "\n")
        self.process.stdin.flush()
        line = self.process.stdout.readline()
        if not line:
            raise RuntimeError("MCP server closed stdout")
        response = json.loads(line)
        if "error" in response:
            raise RuntimeError(f"MCP error: {response['error']}")
        return response["result"]

    def initialize(self) -> dict[str, Any]:
        result = self._request(
            "initialize",
            {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "mc3000-full-usecase", "version": "1"},
            },
        )
        assert self.process.stdin is not None
        self.process.stdin.write(
            json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n"
        )
        self.process.stdin.flush()
        return result

    def tool(self, name: str, arguments: dict[str, Any] | None = None) -> dict[str, Any]:
        result = self._request("tools/call", {"name": name, "arguments": arguments or {}})
        if result.get("isError"):
            raise RuntimeError(f"{name} failed: {result.get('content')}")
        return json.loads(result["content"][0]["text"])

    def close(self) -> None:
        if self.process.poll() is None:
            assert self.process.stdin is not None
            self.process.stdin.close()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()


def wait_for_charger(deadline: float = 240.0) -> list[dict[str, Any]]:
    end = time.monotonic() + deadline
    attempt = 0
    while time.monotonic() < end:
        attempt += 1
        client = Mcp()
        try:
            client.initialize()
            result = client.tool("charger.scan_devices", {"timeout": 12})
            devices = result.get("devices", [])
            if devices:
                event("charger_discovered", devices=devices, attempt=attempt)
                return devices
            event("charger_not_seen", attempt=attempt)
        finally:
            client.close()
        time.sleep(5)
    raise RuntimeError("MC3000 did not advertise within the scan deadline")


def connect(preferred: str | None) -> dict[str, Any]:
    client = Mcp()
    client.initialize()
    if preferred:
        connected = client.tool(
            "charger.connect", {"address": preferred, "timeout": 30, "model": "mc3000"}
        )
    else:
        connected = client.tool("charger.connect", {"timeout": 30, "model": "mc3000"})
    if not connected.get("connected"):
        raise RuntimeError(f"connect failed: {connected}")
    return client


def slot_statuses(client: Mcp) -> dict[int, dict[str, Any]]:
    result = client.tool("charger.get_status")
    return {int(item["status"]["slot"]): item["status"] for item in result["slots"]}


def stop_all(client: Mcp | None) -> None:
    if client is None:
        return
    try:
        client.tool("charger.stop_all")
        event("emergency_stop_sent")
    except Exception as error:
        event("emergency_stop_failed", error=str(error))


def main() -> int:
    if not os.path.exists(ARM_FILE):
        log("not armed; exiting")
        return 0
    os.unlink(ARM_FILE)
    subprocess.run(["systemctl", "disable", "mc3000-full-usecase.service"], check=False)

    devices = wait_for_charger()
    preferred = ADDR
    addresses = {str(item.get("address")) for item in devices}
    if preferred not in addresses:
        preferred = next(
            (
                str(item["address"])
                for item in devices
                if any(
                    name in str(item.get("name", ""))
                    for name in ("Charger", "SimpleBLEPeripheral", "HitecCharger")
                )
            ),
            None,
        )

    client: Mcp | None = None
    try:
        client = connect(preferred)
        event("connected", result="ok")

        version = client.tool("mc3000_get_version")
        basic = client.tool("mc3000_get_basic")
        statuses = slot_statuses(client)
        event("baseline", version=version, basic=basic, slots=statuses)

        active = [
            slot
            for slot, status in statuses.items()
            if 500 <= int(status.get("voltage_mv", 0)) <= 1650
            and status.get("status") == "standby"
        ]
        if not active:
            raise RuntimeError(f"no GP AA slots detected: {statuses}")
        if len(active) > 3:
            raise RuntimeError(f"too many active slots: {active}")
        event("selected_slots", slots=active)

        # Complete dry runs before any hardware mutation.
        for slot in active:
            dry = client.tool(
                "charger.apply_profile", {"slot": slot, "profile_id": PROFILE_ID}
            )
            if dry.get("dry_run") is not True or dry.get("applied") is not False:
                raise RuntimeError(f"unexpected dry-run result for slot {slot}: {dry}")
            if not dry.get("validation", {}).get("ok"):
                raise RuntimeError(f"validation failed for slot {slot}: {dry}")
            event("dry_run_passed", slot=slot, profile_hex=dry.get("profile_hex"))

        # Apply each slot only after every dry run passed.
        for slot in active:
            applied = client.tool(
                "charger.apply_profile",
                {
                    "slot": slot,
                    "profile_id": PROFILE_ID,
                    "dry_run": False,
                    "confirmation_token": f"APPLY_PROFILE_SLOT_{slot}",
                },
            )
            if applied.get("applied") is not True:
                raise RuntimeError(f"live apply failed for slot {slot}: {applied}")
            event("profile_applied", slot=slot, profile_hex=applied.get("profile_hex"))

        for slot in active:
            started = client.tool(
                "charger.start",
                {"slot": slot, "confirmation_token": f"START_SLOT_{slot}"},
            )
            if started.get("sent") != "start":
                raise RuntimeError(f"start failed for slot {slot}: {started}")
            event("slot_started", slot=slot)
            time.sleep(1)

        checks = 0
        while checks < 8:
            checks += 1
            time.sleep(20)
            statuses = slot_statuses(client)
            sample = {slot: statuses.get(slot) for slot in active}
            event("monitor", check=checks, slots=sample)

            failures = []
            for slot in active:
                status = sample[slot]
                if not status:
                    failures.append(f"slot {slot}: missing")
                    continue
                current = int(status.get("current_ma", 0))
                if status.get("battery_type") != "NiMH":
                    failures.append(f"slot {slot}: chemistry={status.get('battery_type')}")
                if not status.get("is_working"):
                    failures.append(f"slot {slot}: is_working=false")
                if status.get("mode") != "charge":
                    failures.append(f"slot {slot}: mode={status.get('mode')}")
                if not 50 <= current <= 500:
                    failures.append(f"slot {slot}: current={current}mA")
                if int(status.get("voltage_mv", 0)) > 1650:
                    failures.append(f"slot {slot}: voltage={status.get('voltage_mv')}mV")
                if int(status.get("temperature", 0)) > 45:
                    failures.append(f"slot {slot}: temperature={status.get('temperature')}")

            if not failures:
                event("use_case_passed", slots=active, checks=checks)
                log("FULL USE CASE PASSED", slots=active, check=checks)
                return 0
            if failures and checks >= 4:
                raise RuntimeError(f"monitoring failed: {failures}")
            log("monitor retry", check=checks, failures=failures)

        raise RuntimeError("charging did not stabilize within the monitoring window")
    except Exception as error:
        event("use_case_failed", error=str(error))
        log("FULL USE CASE FAILED", error=str(error))
        stop_all(client)
        return 1
    finally:
        client.close()


if __name__ == "__main__":
    raise SystemExit(main())
