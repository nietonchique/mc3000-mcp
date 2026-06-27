# mc3000-mcp

[![CI](https://github.com/nietonchique/mc3000-mcp/actions/workflows/ci.yml/badge.svg)](https://github.com/nietonchique/mc3000-mcp/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![MCP](https://img.shields.io/badge/MCP-stdio-green.svg)](server.json)

Safe MCP server and companion agent skill for monitoring and controlling SKYRC MC3000-compatible battery chargers over Bluetooth Low Energy.

The BLE protocol was recovered from the official Android app (`MC3000` 4.1.2). The production `charger.*` MCP tools expose safe charger operations: inspect slot state, read charger-stored voltage curves, validate battery profiles, dry-run profile writes, require explicit confirmation for live writes/starts, and provide emergency stop tools.

Status: alpha. Protocol framing, MCP stdio behavior, safety validation, and fake-client flows are tested. Live testing has confirmed MC3000 BLE operation on NiMH Charge, Discharge, Refresh, Break-in, and Cycle modes. Hardware behavior still depends on charger firmware/revision and BLE availability.

## Safety warning

This software can control a physical battery charger. Wrong chemistry, current, voltage, or slot selection can damage cells or create a fire risk.

- The LLM/agent is not the safety layer; limits and dangerous-action gates are enforced in the MCP server.
- Verify battery chemistry, capacity, cell count, voltage/cutoff, temperature policy, and slot before applying any profile.
- API slot numbers are zero-based (`0..3`), matching Android app internals. Physical slot 1 is API slot `0`.
- `charger.apply_profile` is dry-run by default. Live writes require `confirmation_token=APPLY_PROFILE_SLOT_<slot>`.
- `charger.start` requires a separate `confirmation_token=START_SLOT_<slot>`.
- Keep the charger in sight while testing. Do not expose this MCP server to untrusted clients.
- Factory/calibration reset opcodes are documented for research but intentionally not exposed as production tools.

See `SECURITY.md` and `docs/safety-model.md` before using live hardware.

## Features

- BLE discovery using the same advertised names as the Android app.
- Connect/disconnect over BLE FFE0/FFE1.
- Read one slot or all four slots.
- Parse slot status: chemistry, mode, status, time, voltage, current, capacity, temperature, internal resistance, LED state.
- Read firmware/hardware version and basic device settings.
- Read the app-compatible charger-stored voltage time-series (`0x56`) and export it as JSON/CSV.
- Validate battery profiles against conservative chemistry/device limits.
- Build/apply 40-byte MC3000 profiles with chemistry and mode-aware defaults.
- Dry-run-by-default production tools plus explicit confirmation tokens for writes/starts.
- Emergency stop for one slot or all slots.
- Companion `charger-agent` skill for Hermes Agent workflows.
- Legacy `mc3000_*` low-level tools for protocol/debug work.

## Install

From source:

```bash
git clone https://github.com/nietonchique/mc3000-mcp.git
cd mc3000-mcp
python -m venv .venv
. .venv/bin/activate
python -m pip install -e '.[test]'
```

Once published to PyPI, install/run with one of:

```bash
pipx install mc3000-mcp
pipx run mc3000-mcp
# or
uvx mc3000-mcp
```

Linux BLE access requires BlueZ/DBus. Confirm the adapter is powered:

```bash
bluetoothctl show
```

If scans return no devices, make sure the charger is powered on, not connected to the phone app, and close enough to the PC BLE adapter.

## Configure as an MCP server

Local source checkout:

```bash
hermes mcp add mc3000 --command "$(pwd)/.venv/bin/mc3000-mcp"
hermes mcp test mc3000
```

Published package / pipx:

```bash
hermes mcp add mc3000 --command "pipx run mc3000-mcp"
hermes mcp test mc3000
```

Common client config examples are in `examples/claude-desktop.json`, `examples/codex.json`, and `examples/cursor.json`.

## Install the companion Hermes skill

The MCP server enforces safety. The skill teaches the agent the safe workflow: ask for missing battery facts, choose conservative profiles, show dry-run summaries, require explicit confirmation, monitor after start, and stop on unsafe state.

Install from this repository:

```bash
hermes skills install \
  https://raw.githubusercontent.com/nietonchique/mc3000-mcp/main/skill/SKILL.md \
  --name charger-agent
```

Use it explicitly:

```bash
hermes -s charger-agent
# or inside a running session:
# /skill charger-agent
```

The raw `SKILL.md` is intentionally self-contained. Additional repository files under `skill/profiles/` and `skill/checklists/` are examples/checklists for humans and future bundle-style registries; the server-side safety rules remain in `mc3000_mcp.safety`.

## Production MCP tools

Preferred tools are namespaced as `charger.*` and enforce dry-run/confirmation on dangerous actions:

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
| `charger.export_session_log` | Export redacted in-memory command/session log. |

Legacy `mc3000_*` tools remain for low-level protocol work and backwards compatibility.

## Safe workflow examples

Dry-run a bundled conservative NiMH AA profile for physical slot 4 / API slot 3:

```json
{
  "name": "charger.apply_profile",
  "arguments": {
    "slot": 3,
    "profile_id": "nimh-aa-conservative-charge"
  }
}
```

Apply after explicit confirmation:

```json
{
  "name": "charger.apply_profile",
  "arguments": {
    "slot": 3,
    "profile_id": "nimh-aa-conservative-charge",
    "dry_run": false,
    "confirmation_token": "APPLY_PROFILE_SLOT_3"
  }
}
```

Start after a separate explicit confirmation:

```json
{
  "name": "charger.start",
  "arguments": {
    "slot": 3,
    "confirmation_token": "START_SLOT_3"
  }
}
```

Export the charger-stored voltage curve as CSV:

```json
{
  "name": "charger.export_voltage_curve",
  "arguments": {
    "slot": 3,
    "format": "csv"
  }
}
```

Emergency stop all slots:

```json
{"name": "charger.stop_all", "arguments": {}}
```

More examples: `docs/examples.md`.

## Voltage curves

The Android app's graph is a voltage curve, not a generic telemetry logger. It requests opcode `0x56`, receives a charger-stored sample interval plus up to 120 millivolt points, and plots/exports time vs voltage. Current, capacity, temperature, and resistance are status fields, not historical curve series.

This project exposes that same data as:

- `mc3000_get_voltage_curve` — low-level protocol/debug tool.
- `charger.get_voltage_curve` — production-safe JSON time-series.
- `charger.export_voltage_curve` — JSON/CSV export.

No background polling is required for the app-compatible voltage curve.

## Documentation map

- `SECURITY.md` — hardware safety and disclosure policy.
- `docs/supported-devices.md` — confirmed devices and compatibility policy.
- `docs/safety-model.md` — what is enforced in code vs guidance.
- `docs/profile-schema.md` — battery profile schema and examples.
- `docs/ble-protocol-notes.md` — GATT/framing/opcode notes.
- `docs/reverse-notes.md` — detailed reverse-engineering notes.
- `docs/examples.md` — safe dry-run/apply/start/stop/curve examples.
- `docs/publishing.md` — release/listing/skill distribution checklist.
- `skill/SKILL.md` — companion agent playbook.
- `server.json` — draft Official MCP Registry metadata.

## Protocol short version

- Service UUID: `0000FFE0-0000-1000-8000-00805f9b34fb`
- Write/notify characteristic UUID: `0000FFE1-0000-1000-8000-00805f9b34fb`
- Device names: `SimpleBLEPeripheral`, `Charger`, `HitecCharger`
- Normal command: 20 bytes, checksum in byte 19.
- Profile command (`0x11`): 40 bytes, checksum in byte 39, written as two 20-byte chunks.
- Voltage curve command (`0x56`): charger-stored voltage curve for a zero-based slot.
- Status/curve slot args are zero-based; start/stop use slot bitmasks.

## Development

```bash
python -m pip install -e '.[test]'
python -m pytest -q
python -m ruff check .
python -m ruff format --check .
python -m mypy src tests
```

Local MCP stdio smoke test without hardware:

```bash
python -m mc3000_mcp.server <<'EOF'
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}
{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"charger.list_profiles","arguments":{}}}
EOF
```

Build/publish checks:

```bash
python -m build
python -m twine check dist/*
```

## Repository hygiene

The repository intentionally does not include the APK, decompiled Java, local reverse-engineering scratch outputs, virtualenvs, or build caches. Those are excluded by `.gitignore`.

## License

Apache-2.0. See `LICENSE`.
