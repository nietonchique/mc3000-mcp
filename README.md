# mc3000-mcp

<!-- mcp-name: io.github.nietonchique/mc3000-mcp -->

[![CI](https://github.com/nietonchique/mc3000-mcp/actions/workflows/ci.yml/badge.svg)](https://github.com/nietonchique/mc3000-mcp/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![MCP](https://img.shields.io/badge/MCP-stdio-green.svg)](server.json)

**Safe local MCP control for SKYRC MC3000-compatible chargers.**

`mc3000-mcp` connects an AI agent to a battery charger without handing it an unsupervised
hardware API. BLE control is local. Charging is dry-run by default. Live writes and starts need
explicit per-slot confirmation. Emergency stop is always one call away.

```text
Agent  ──MCP stdio──  mc3000-mcp  ──BLE FFE0/FFE1──  SKYRC MC3000
                          │
                          ├─ profile validation and chemistry limits
                          ├─ dry-run profile rendering
                          ├─ confirmation tokens for live writes/starts
                          └─ bounded, redacted audit log
```

| | |
|---|---|
| Implementation | Rust binary; no Python runtime |
| MCP | stdio, 27 tools, resources and prompts |
| Safety | code-enforced validation, dry-run first, explicit confirmation |
| Hardware | MC3000 read/write; MC5000 read-only |
| Live-tested | MC3000 firmware `1.23`, hardware `2.20`, NiMH modes |
| Registry | crates.io, MCP Registry MCPB, AUR |

> **Hardware warning.** Wrong chemistry, current, voltage, slot, or unsupervised charging can
> damage cells or start a fire. The MCP server is the safety boundary, not the LLM. Keep the
> charger in sight. See [SECURITY.md](SECURITY.md).

## Install

```bash
# crates.io
cargo install mc3000-mcp --locked

# Arch Linux
paru -S mc3000-mcp

# released tag, if you prefer installing from source
cargo install --git https://github.com/nietonchique/mc3000-mcp.git --tag v0.2.1 --locked
```

A Linux x86_64 MCPB bundle is attached to each release for MCPB-compatible clients.

Linux BLE requires BlueZ/D-Bus:

```bash
bluetoothctl show
# If the adapter is off:
sudo rfkill unblock bluetooth && bluetoothctl power on
```

## Connect an MCP client

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

Client-specific examples are in [`examples/`](examples).

Try it without hardware:

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

## Agent skill

`charger-agent` uses the portable [Agent Skills](https://agentskills.io) layout: one folder with a
`SKILL.md` manifest. No vendor-specific installer is required.

Install it for Codex:

```bash
mkdir -p ~/.codex/skills
curl -fsSL https://github.com/nietonchique/mc3000-mcp/releases/download/v0.2.1/mc3000-mcp-skill-v0.2.1.tar.gz \
  | tar -xz -C ~/.codex/skills
```

For OMP, use `~/.omp/agent/skills` as the destination. For Claude-compatible clients, use
`~/.claude/skills`. Hermes can also install the raw file, but it is just one compatible client—not
the distribution format.

The folder contains the charger workflow plus a pre-start checklist and example profile. It is
guidance only; the Rust MCP server remains the enforced safety boundary.

## CLI

The default command starts MCP stdio. Read-only diagnostics are also available directly:

```bash
mc3000-mcp --version
mc3000-mcp scan --timeout 8
mc3000-mcp status --address AA:BB:CC:DD:EE:FF
mc3000-mcp --backend fake scan
```

The CLI can scan and read slots. It cannot write profiles, change settings, or start charging.

## Safe workflow

1. Read slot status.
2. Validate a battery profile.
3. Render it as a dry run.
4. Apply it with `confirmation_token=APPLY_PROFILE_SLOT_<slot>`.
5. Start it with a separate `confirmation_token=START_SLOT_<slot>`.
6. Monitor voltage, current, temperature, and status.
7. Use `charger.stop_slot` / `charger.stop_all` immediately if anything looks wrong.

Example dry run for physical slot 4 / API slot 3:

```json
{
  "name": "charger.apply_profile",
  "arguments": {
    "slot": 3,
    "profile_id": "nimh-aa-conservative-charge"
  }
}
```

Start requires a second explicit confirmation:

```json
{
  "name": "charger.start",
  "arguments": {
    "slot": 3,
    "confirmation_token": "START_SLOT_3"
  }
}
```

Emergency stop is deliberately tokenless:

```json
{"name": "charger.stop_all", "arguments": {}}
```

## Tools

Production surface is namespaced as `charger.*`:

| Tool | Purpose |
| --- | --- |
| `scan_devices` | Read-only BLE scan. |
| `connect` / `disconnect` | Manage the charger connection. |
| `get_status`, `read_slots` | Slot chemistry, mode, voltage/current, temperature, state. |
| `get_voltage_curve`, `export_voltage_curve` | Charger-stored voltage series as JSON/CSV. |
| `list_profiles`, `validate_profile` | Bundled profiles and enforced safety checks. |
| `apply_profile` | Dry-run by default; live write requires its slot token. |
| `start` | Start a prepared slot; requires a separate token. |
| `stop_slot`, `stop_all` | Immediate emergency stop. |
| `export_session_log` | Bounded, redacted audit log. |

Fourteen legacy `mc3000_*` tools remain for protocol/debugging work. They use the same
confirmation policy for non-emergency writes. Raw writes cannot implicitly start slots and always
block factory/calibration restore opcodes.

## Safety model

- API slots are zero-based `0..3`; physical slots are `1..4`.
- `apply_profile` defaults to `dry_run=true`.
- Live profile writes require exactly `APPLY_PROFILE_SLOT_<slot>`.
- Starts require exactly `START_SLOT_<slot>`.
- Safety checks run before any backend call; unsupported chemistry fails closed.
- NiMH/Eneloop/NiCd, LiIon, and LiFe have enforced limits.
- Unknown charger models and MC5000 connections are read-only.
- Session logs redact addresses and confirmation tokens and are bounded in memory.

Full model: [`docs/safety-model.md`](docs/safety-model.md).
Profile schema and bundled examples: [`docs/profile-schema.md`](docs/profile-schema.md).

## Compatibility

Confirmed live: SKYRC MC3000 firmware `1.23`, hardware `2.20`.

Protocol details are documented from the official Android app and hardware testing in
[`docs/ble-protocol-notes.md`](docs/ble-protocol-notes.md) and
[`docs/reverse-notes.md`](docs/reverse-notes.md). MC5000 support is intentionally read-only until
writes are verified on real hardware.

Charger firmware behavior can differ. A profile write acknowledgement is not proof that the charger
applied it—start the slot and verify mode/current/status.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-features --locked
cargo audit && cargo deny check
```

The fake backend keeps hardware out of normal tests and CI. See
[`CONTRIBUTING.md`](CONTRIBUTING.md) for the full workflow and hardware-safety rule.

## Documentation

- Security and disclosure: [`SECURITY.md`](SECURITY.md)
- Safety model: [`docs/safety-model.md`](docs/safety-model.md)
- Supported devices: [`docs/supported-devices.md`](docs/supported-devices.md)
- Examples: [`docs/examples.md`](docs/examples.md)
- Release/publishing checklist: [`docs/publishing.md`](docs/publishing.md)
- Agent skill: [`skill/SKILL.md`](skill/SKILL.md)

## License

Apache-2.0. See [LICENSE](LICENSE).
