# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- `send_profile` and `charger.stop_all` now wait for firmware acknowledgements instead of using
  fixed inter-write sleeps: each `0xFE` stop awaits its ACK (500 ms timeout, best-effort), and
  the `0x11` profile write awaits its ACK with a 2 s timeout. Profile writes without a start
  return as soon as the firmware acknowledges them instead of always waiting the 5 s settle
  delay; the settle delay remains before start writes because firmware 1.23 ignored starts
  issued immediately after an acknowledged profile.
- Firmware 1.13 (which never acknowledges `0x11`) falls back to the settle delay and caches the
  behaviour per connection, so later profile writes skip the acknowledgement wait.

### Fixed

- `charger.stop_all` now aborts with the model-gate error on read-only models instead of
  swallowing it as a per-slot acknowledgement timeout.

## [0.2.3] - 2026-10-25

### Fixed

- Pace `charger.stop_all` writes with 500 ms between slots; MC3000 firmware 1.23 dropped
  back-to-back stop frames, leaving some slots charging after an emergency stop.

## [0.2.2] - 2026-10-24

### Fixed

- Scan all local BLE adapters concurrently instead of spending the whole timeout on the first
  adapter; this fixes discovery on multi-adapter hosts.
- Wait five seconds after profile chunk writes before starting slots; MC3000 firmware 1.23 ignored
  immediate starts despite accepting the profile.
- Tolerate transient BlueZ startup races while waiting for the charger to advertise.

### Added

- Supervised full-use-case runner for read-only checks, profile dry-runs, live writes, starts, and
  charging monitoring.
- Standalone Agent Skill archive in release assets.

### Changed

- Corrected the bundled conservative NiMH profile source metadata to GP AA 2600 mAh.
- Disabled Dependabot version updates for this repository.

## [0.2.1] - 2026-10-23

### Fixed

- Serialize concurrent MCP tool calls so pipelined `connect`, read, write, and `disconnect`
  requests cannot race shared charger connection state.
- Add a deterministic fake-backend connect delay and regression coverage for concurrent
  connection lifecycle requests.


## [0.2.0] - 2026-10-23

Full rewrite of the server in Rust. The major MCP tool names, read-only response fields, protocol
behavior, and zero-based API slots `0..3` (physical slots `1..4`) are unchanged.

### Migration notes

- The Rust binary is the only supported server path. The Python package, its `pyproject.toml`
  entry points, and all pip/pipx/uvx installation instructions are removed.
- Install from the `v0.2.0` Git tag, from a release archive, or via the AUR package
  `mc3000-mcp`. Update MCP client configs to launch the `mc3000-mcp` binary instead of the Python
  module; see the README for examples.
- The server version now comes from the crate version instead of a hardcoded Python constant.
- Invalid input (unknown chemistry, invalid cycle modes, malformed MAC addresses, invalid slot
  masks, out-of-range numbers) now returns typed, descriptive validation errors instead of Python
  tracebacks.

### Added

- Typed profiles, safety limits, and bundled profiles validated before any backend call.
- Deterministic `--backend fake` BLE backend for hardware-free development and testing.
- Read-only CLI commands `scan` and `status`; default invocation starts the MCP stdio server.
- Bounded, redacted in-memory audit log; confirmation tokens and device identifiers are redacted
  before storage.
- Confirmation policy applied to every non-emergency write/start path, including legacy tools;
  emergency stop remains tokenless.

### Changed

- `mc3000_status` uses the selected connection model instead of global state; unknown models are
  read-only.
- All notification parsers are total over arbitrary bytes: malformed or truncated frames produce
  structured errors or `unknown` values instead of panics or unbounded allocations.
- Timeouts are finite and non-negative; backend errors carry actionable context without exposing
  host tracebacks to MCP clients.
- Stdout is reserved for the MCP protocol; logs go to stderr via `tracing`.

### Safety (breaking)

- `mc3000_send_raw` no longer auto-detects a 40-byte frame and no longer implicitly stops or starts
  slots. Raw writes require an explicit raw-command confirmation; factory/calibration restore
  opcodes remain blocked.

### Removed

- The Python implementation, pytest configuration, and the Python CI/release workflows.

Releases before 0.2.0 shipped the Python implementation and predate this changelog.

[Unreleased]: https://github.com/nietonchique/mc3000-mcp/compare/v0.2.2...HEAD
[0.2.3]: https://github.com/nietonchique/mc3000-mcp/releases/tag/v0.2.3
[0.2.2]: https://github.com/nietonchique/mc3000-mcp/releases/tag/v0.2.2
[0.2.1]: https://github.com/nietonchique/mc3000-mcp/releases/tag/v0.2.1
[0.2.0]: https://github.com/nietonchique/mc3000-mcp/releases/tag/v0.2.0
