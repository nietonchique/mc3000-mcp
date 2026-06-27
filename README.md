# mc3000-mcp

[![CI](https://github.com/nietonchique/mc3000-mcp/actions/workflows/ci.yml/badge.svg)](https://github.com/nietonchique/mc3000-mcp/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![MCP](https://img.shields.io/badge/MCP-stdio-green.svg)](server.json)

MCP server for monitoring and controlling SKYRC MC3000 battery chargers over Bluetooth Low Energy.

The BLE protocol was recovered from the official Android app (`MC3000` 4.1.2). The server exposes charger operations as MCP tools so an agent can inspect slot state, read voltage curves, stop/start slots, and apply charge/discharge profiles.

Status: alpha. Protocol framing and offline MCP behavior are tested; real hardware testing depends on having the charger powered on and advertising over BLE.

## Safety warning

This software can control a physical battery charger. Wrong chemistry, current, voltage, or slot selection can damage cells or create a fire risk.

- Verify battery chemistry and limits before applying any profile.
- API slot numbers are zero-based (`0..3`), matching the Android app internals.
- Keep the charger in sight while testing.
- Do not expose this MCP server to untrusted clients.
- Factory/calibration reset opcodes are documented for research but intentionally not exposed as first-class tools.

## Features

- BLE discovery using the same advertised names as the Android app.
- Connect/disconnect to the charger over BLE.
- Poll one slot or all four slots.
- Parse slot status: chemistry, mode, status, time, voltage, current, capacity, temperature, internal resistance, LED state.
- Read firmware/hardware version.
- Read and write basic device settings.
- Read 245-byte voltage curves.
- Build/apply 40-byte MC3000 charge/discharge profiles.
- Raw frame escape hatch for protocol experiments.
- No Android device required.

## Install

```bash
git clone https://github.com/nietonchique/mc3000-mcp.git
cd mc3000-mcp
python -m venv .venv
. .venv/bin/activate
python -m pip install -e '.[test]'
```

Linux BLE access requires BlueZ/DBus. Confirm the adapter is powered:

```bash
bluetoothctl show
```

## Run as an MCP server

```bash
mc3000-mcp
```

Hermes example:

```bash
hermes mcp add mc3000 --command "$(pwd)/.venv/bin/mc3000-mcp"
hermes mcp test mc3000
```

## MCP tools

Preferred production tools are namespaced as `charger.*` and enforce dry-run/confirmation on dangerous actions:

| Tool | Purpose |
| --- | --- |
| `charger.scan_devices` | Read-only BLE scan. |
| `charger.connect` | Connect to a selected device. |
| `charger.get_status` | Read current charger status. |
| `charger.read_slots` | Read one slot or all slots. |
| `charger.get_voltage_curve` | Read charger-stored voltage time-series (`0x56`), app-compatible and no polling. |
| `charger.export_voltage_curve` | Export that voltage curve as JSON or CSV. |
| `charger.list_profiles` | List bundled example profiles. |
| `charger.validate_profile` | Validate a battery profile against enforced limits. |
| `charger.apply_profile` | Validate and apply a profile; dry-run by default, live write requires `APPLY_PROFILE_SLOT_<slot>`. |
| `charger.start` | Start a slot; requires `START_SLOT_<slot>`. |
| `charger.stop_slot` | Emergency stop for one slot. |
| `charger.stop_all` | Emergency stop for all slots. |
| `charger.export_session_log` | Export redacted in-memory session log. |

Legacy `mc3000_*` tools remain for low-level protocol work and backwards compatibility.

## Example MCP calls

Build a conservative LiIon charge profile for slot 1 (API slot mask bit 0):

```json
{
  "name": "mc3000_build_profile",
  "arguments": {
    "slot_mask": 1,
    "battery_type": 0,
    "mode": 0,
    "capacity_mah": 2000,
    "charge_current_ma": 1000,
    "charge_stop_voltage_mv": 4200,
    "charge_stop_current_ma": 100,
    "temp_cutoff": 45
  }
}
```

Poll all slots:

```json
{"name": "mc3000_status", "arguments": {}}
```

Stop physical slot 1:

```json
{"name": "mc3000_stop", "arguments": {"slot": 0}}
```

## Protocol notes

See `docs/reverse-notes.md` and `docs/ble-protocol-notes.md` for the recovered frame layout, opcodes, and payload mappings.

Additional docs:

- `SECURITY.md` — hardware safety and disclosure policy.
- `docs/supported-devices.md` — confirmed devices and compatibility policy.
- `docs/safety-model.md` — what is enforced in code vs guidance.
- `docs/profile-schema.md` — battery profile schema and examples.
- `docs/examples.md` — safe dry-run/apply/start/stop examples.
- `docs/publishing.md` — release/listing descriptions and manual publishing checklist.
- `skill/SKILL.md` — agent playbook for safe charger operation.
- `server.json` — MCP registry metadata draft.

Short version:

- Service UUID: `0000FFE0-0000-1000-8000-00805f9b34fb`
- Write/notify characteristic UUID: `0000FFE1-0000-1000-8000-00805f9b34fb`
- Device names: `SimpleBLEPeripheral`, `Charger`, `HitecCharger`
- Normal command: 20 bytes, checksum in byte 19.
- Profile command: 40 bytes, checksum in byte 39, written as two 20-byte chunks.

## Development

```bash
python -m pip install -e '.[test]'
python -m pytest -q
```

Local E2E smoke test without hardware:

```bash
python -m mc3000_mcp.server <<'EOF'
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}
{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"mc3000_build_profile","arguments":{"slot_mask":1}}}
EOF
```

Hardware smoke test:

```bash
mc3000-mcp
# then call mc3000_scan, mc3000_connect, mc3000_status
```

If `mc3000_scan` returns no devices, make sure the charger is powered on, not already connected to the phone app, and close enough to the PC BLE adapter.

## Repository hygiene

The repository intentionally does not include the APK, decompiled Java, or local reverse-engineering scratch outputs. Those are excluded by `.gitignore`.

## License

Apache-2.0. See `LICENSE`.
