# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/nietonchique/mc3000-mcp/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/nietonchique/mc3000-mcp/releases/tag/v0.2.0
