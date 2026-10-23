# mc3000-mcp

<!-- mcp-name: io.github.nietonchique/mc3000-mcp -->

[![CI](https://github.com/nietonchique/mc3000-mcp/actions/workflows/ci.yml/badge.svg)](https://github.com/nietonchique/mc3000-mcp/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![MCP](https://img.shields.io/badge/MCP-stdio-green.svg)](server.json)

Safe MCP server and companion agent skill for monitoring and controlling SKYRC
MC3000-compatible battery chargers over Bluetooth Low Energy.

The Rust server exposes `charger.*` production tools for safe charger operations: inspect slot state,
read charger-stored voltage curves, validate battery profiles, dry-run profile writes, require explicit
confirmation for live writes and starts, and stop slots or the whole charger immediately. Legacy
`mc3000_*` tools remain for low-level protocol and debugging work and follow the same confirmation
policy for non-emergency writes.

Status: alpha. Protocol framing, profile construction, safety validation, notification parsing, request
sequencing, and fake-client flows are tested automatically. Live hardware checks have confirmed MC3000
BLE operation on NiMH Charge, Discharge, Refresh, Break-in, and Cycle modes. Hardware behavior still
depends on charger firmware/revision and BLE availability.

## Safety warning

This software can control a physical battery charger. Wrong chemistry, current, voltage, or slot
selection can damage cells or create a fire risk.

- The LLM/agent is not the safety layer. Limits and dangerous-action gates are enforced in the server.
- Verify battery chemistry, capacity, cell count, voltage/cutoff, temperature policy, and slot before
  applying a profile.
- API slot numbers are zero-based (`0..3`), matching Android app internals. Physical slot 1 is API slot
  `0`.
- `charger.apply_profile` is dry-run by default. Live writes require `confirmation_token=APPLY_PROFILE_SLOT_<slot>`.
- `charger.start` requires a separate `confirmation_token=START_SLOT_<slot>`.
- Emergency stop tools are intentionally tokenless: use them immediately when anything looks wrong.
- Keep the charger in sight while testing. Do not expose this MCP server to untrusted clients.
- Factory and calibration reset opcodes are documented for research but are blocked by the server.

Read `SECURITY.md` and `docs/safety-model.md` before using live hardware.

## Features

- BLE discovery using the advertised names recognized by the official Android app.
- Connect and disconnect over the Nordic-UART-like FFE0/FFE1 GATT layout.
- Read one slot or all four slots.
- Parse slot status: chemistry, mode, status, time, voltage, current, capacity, temperature, internal
  resistance, and LED state.
- Read firmware/hardware version and basic device settings.
- Read and export the app-compatible charger-stored voltage time-series (`0x56`) as JSON or CSV.
- Validate battery profiles against conservative chemistry/device limits before any backend call.
- Build and apply 40-byte MC3000 profiles with chemistry- and mode-aware defaults.
- Serialized device requests and bounded notification queues, so concurrent polls cannot consume a
  profile acknowledgement or grow memory without bound.
- Explicit charger-model state: MC5000 and unknown devices are read-only.
- Deterministic fake backend for hardware-free development and MCP client testing.
- Companion `charger-agent` skill for Hermes Agent workflows.
- Legacy `mc3000_*` low-level tools for protocol/debug work, with the same write confirmation policy.

### MC3000 vs MC5000 selection

Model selection is based on the charger, not the inserted battery. Select the model explicitly when
connecting; the default is MC3000. MC5000 exposes read-only slot status through opcode `0x91`. Profile
writes and start operations remain MC3000-only until they are verified on real MC5000 hardware. Unknown
devices must not receive profile writes or start commands.

## Install

From source:

```bash
git clone https://github.com/nietonchique/mc3000-mcp.git
cd mc3000-mcp
cargo install --path . --locked
```

From crates.io (after the 0.2.0 release):

```bash
cargo install mc3000-mcp --locked
```

On Arch Linux:

```bash
paru -S mc3000-mcp
```

The official MCP Registry distribution is an MCPB bundle attached to each GitHub release. Download the
Linux x86_64 `.mcpb` file and install it in an MCPB-compatible client, or use the native binary
installation above.

Linux BLE access requires BlueZ over D-Bus. Confirm that the adapter is powered:

```bash
bluetoothctl show
```

If scanning reports a powered-off adapter:

```bash
sudo rfkill unblock bluetooth
bluetoothctl power on
```

If scans return no devices, make sure the charger is powered on, is not connected to the phone app, and
is close enough to the computer's BLE adapter.

## Command line

Starting without a subcommand runs the MCP stdio server:

```bash
mc3000-mcp
mc3000-mcp --backend fake --help
```

Read-only hardware diagnostics are available directly:

```bash
mc3000-mcp scan --timeout 8
mc3000-mcp status --address F8:2E:0C:B8:A6:15
```

`scan` lists charger-like devices. `status` connects to an address (or the first discovered charger),
reads all four slots, prints JSON, and disconnects. The CLI never writes a profile, changes settings, or
starts a slot.

## Configure as an MCP server

Native binary:

```json
{
  "mcpServers": {
    "mc3000": {
      "command": "mc3000-mcp",
      "args": []
    }
  }
}
```

Hardware-free client setup:

```json
{
  "mcpServers": {
    "mc3000": {
      "command": "mc3000-mcp",
      "args": ["--backend", "fake"]
    }
  }
}
```

Client-specific examples are in `examples/claude-desktop.json`, `examples/codex.json`, and
`examples/cursor.json`.

For Hermes:

```bash
hermes mcp add mc3000 --command mc3000-mcp
hermes mcp test mc3000
```

## Install the companion Hermes skill

The MCP server enforces safety. The skill teaches the agent the safe workflow: ask for missing battery
facts, choose conservative profiles, show dry-run summaries, require explicit confirmation, monitor after
start, and stop on unsafe state.

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

The raw `SKILL.md` is intentionally self-contained. Files under `skill/profiles/` and `skill/checklists/`
are examples for humans and future bundle-style registries; server-side safety rules live in the Rust
`safety` module.

## Production MCP tools

| Tool | Purpose |
| --- | --- |
| `charger.scan_devices` | Read-only BLE scan. |
| `charger.connect` | Connect to a selected device with an explicit model. |
| `charger.get_status` | Read current charger status for all slots. |
| `charger.read_slots` | Read one slot or all slots. |
| `charger.get_voltage_curve` | Read the charger-stored voltage time-series (`0x56`). |
| `charger.export_voltage_curve` | Export that curve as JSON or CSV. |
| `charger.list_profiles` | List bundled example profiles. |
| `charger.validate_profile` | Validate a battery profile against enforced limits. |
| `charger.apply_profile` | Validate and apply a profile; dry-run by default, live write requires `APPLY_PROFILE_SLOT_<slot>`. |
| `charger.start` | Start a slot; requires `START_SLOT_<slot>`. |
| `charger.stop_slot` | Emergency stop for one slot. |
| `charger.stop_all` | Emergency stop for all slots. |
| `charger.export_session_log` | Export the bounded, redacted in-memory audit log. |

Legacy `mc3000_*` tools cover the same protocol surface for debugging. Non-emergency legacy writes
require the same style of explicit confirmation token; raw writes additionally block factory and
calibration restore opcodes.

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

The Android app graph is a voltage curve, not a generic telemetry logger. The server requests opcode
`0x56`, receives the charger-stored sample interval plus up to 120 millivolt points, and exports time
versus voltage. Current, capacity, temperature, and resistance are status fields, not historical curve
series. Fragmented notifications are reassembled into the complete 245-byte curve before parsing.

## Documentation map

- `SECURITY.md` — hardware safety and disclosure policy.
- `docs/supported-devices.md` — confirmed devices and compatibility policy.
- `docs/safety-model.md` — what is enforced in code versus guidance.
- `docs/profile-schema.md` — battery profile schema and examples.
- `docs/ble-protocol-notes.md` — GATT/framing/opcode notes.
- `docs/reverse-notes.md` — detailed reverse-engineering notes.
- `docs/examples.md` — safe dry-run/apply/start/stop/curve examples.
- `docs/publishing.md` — release, registry, skill, and AUR checklist.
- `docs/rust-rewrite-plan.md` — architecture and acceptance plan for the Rust rewrite.
- `skill/SKILL.md` — companion agent playbook.
- `server.json` — Official MCP Registry metadata.

## Protocol short version

- Service UUID: `0000FFE0-0000-1000-8000-00805f9b34fb`
- Write/notify characteristic UUID: `0000FFE1-0000-1000-8000-00805f9b34fb`
- Device names: `SimpleBLEPeripheral`, `Charger`, `HitecCharger`
- Normal command: 20 bytes, checksum in byte 19.
- Profile command (`0x11`): 40 bytes, checksum in byte 39, written as two 20-byte chunks.
- Voltage curve command (`0x56`): charger-stored voltage curve for a zero-based slot.
- Status/curve slot arguments are zero-based; start/stop use slot bitmasks.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-features --locked
cargo doc --all-features --no-deps --locked
cargo audit
cargo deny check
```

The fake backend keeps hardware out of ordinary development and CI. See `CONTRIBUTING.md` for the full
workflow, including the rule that automated tests never write to or start real hardware.

## License

Apache License 2.0. See [LICENSE](LICENSE).
