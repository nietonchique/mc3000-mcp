# Full Rust rewrite plan

## 1. Outcome

Replace the Python implementation with a production-quality Rust implementation of `mc3000-mcp`.
The Rust binary will be the only supported server path. The rewrite includes protocol and safety
hardening, real and fake BLE backends, complete MCP compatibility, repository automation, crates.io
packaging, and an Arch Linux AUR package.

Target release: **0.2.0**. The major MCP tool names and read-only response fields remain compatible;
intentional safety breaking changes are listed explicitly below.

## 2. Evidence and compatibility baseline

The Python contract is taken from `src/mc3000_mcp/` and its tests, not reconstructed from docs.

A live read-only baseline was captured from the powered charger before the rewrite:

- device name: `Charger`;
- firmware: `1.23`;
- hardware: `2.20`;
- slots 1, 2, and 3 reported approximately 1.32 V;
- every slot reported `standby` and zero current.

Battery chemistry and capacity are unknown. Voltage alone will never be used to infer chemistry or
authorize a write/start operation.

The following must remain compatible unless listed as an intentional change:

- all 27 `mc3000_*` and `charger.*` tool names;
- frame layouts, checksums, slot numbering, profile defaults, and notification parsing;
- profile safety limits and fail-closed behavior for unsupported chemistries;
- `charger.apply_profile` dry-run-by-default behavior and exact per-slot confirmation tokens;
- tokenless emergency stop operations;
- voltage-curve fragment reassembly and JSON/CSV export;
- resources, prompts, bundled profiles, and MCP stdio transport;
- zero-based API slots `0..3`, matching physical slots `1..4`.

## 3. Architecture

Use one Cargo package with a reusable library and one binary:

```text
src/
  lib.rs                 Public library surface and shared error types
  main.rs                CLI and stdio startup; stdout reserved for MCP
  model.rs               Slot, slot mask, charger model, chemistry, mode, status
  protocol.rs            Pure frame builders and total notification parsers
  profile.rs             Typed profiles, defaults, frame conversion, bundled profiles
  safety.rs              Pure validation and safe-operation conversion
  client.rs              Serialized charger operations and connection/model state
  transport/
    mod.rs               Async transport interface and scan data
    ble.rs               btleplug/BlueZ implementation
    fake.rs              Deterministic scriptable fake backend
  session.rs             Bounded, redacted in-memory audit log
  mcp/
    mod.rs               rmcp ServerHandler and service construction
    tools.rs             Production and legacy tool handlers
    resources.rs         JSON resources
    prompts.rs           Safe charging prompts
tests/
  protocol_fixtures.rs   Independently recorded Python/golden protocol fixtures
  safety_contract.rs     Consumer-visible validation boundaries
  mcp_contract.rs        rmcp client against the public server surface
  stdio_smoke.rs         Real binary over stdio using the fake backend
packaging/aur/           Reviewable PKGBUILD, .SRCINFO, and packaging license
```

### Design decisions

- **rmcp 3.7.0**: official Rust MCP SDK; stdio, tools, resources, prompts, schema support, and
  current protocol negotiation. MSRV is Rust 1.88.
- **btleplug 0.13.5**: async BLE central with Linux BlueZ support; no Python runtime.
- **Tokio**: one async runtime for BLE and MCP.
- **Serde + schemars**: typed tool arguments and generated JSON Schemas instead of hand-maintained
  loosely checked dictionaries.
- **thiserror** in the library; **anyhow** only at the binary boundary. No production `unwrap`,
  `expect`, or panic-based error handling.
- **Dynamic dispatch only at the backend boundary**: real BLE and fake backends vary at runtime;
  protocol, validation, and profile code remain static and allocation-conscious.
- **One in-flight device request**: serialize charger commands and correlate replies by opcode so a
  concurrent status notification cannot consume a profile ACK. Notification queues are bounded;
  timeout and disconnect paths clear pending state.
- **No global charger-model flag**: model is explicit per connection. Unknown models are read-only.
- **Stdout is protocol-only**: logs go to stderr through `tracing`; no debug prints can corrupt MCP.

## 4. Intentional safety improvements and breaking changes

1. Server version comes from `CARGO_PKG_VERSION`; no hardcoded version drift.
2. Confirmation tokens and device identifiers are redacted before audit-log storage. The log is a
   bounded queue rather than unlimited process-lifetime growth.
3. Every non-emergency write/start path, including legacy tools, requires the same explicit
   confirmation policy as `charger.*`. Emergency stop remains tokenless.
4. `mc3000_send_raw` no longer auto-detects a 40-byte frame and implicitly stops/starts slots. Raw
   writes require an explicit raw-command confirmation; factory/calibration restore opcodes remain
   blocked.
5. Invalid cycle modes, malformed MAC addresses, invalid slot masks, and out-of-range numeric values
   return typed, descriptive validation errors rather than incidental `KeyError`/tracebacks.
6. `mc3000_status` uses the selected connection model instead of dead global state. An unknown model
   permits read-only probes only.
7. All parsers are total over arbitrary bytes: malformed/truncated notifications produce structured
   errors or `unknown` values, never panics or unbounded allocations.
8. Timeouts must be finite and non-negative; backend errors preserve actionable context without
   returning host tracebacks to MCP clients.

These changes require release notes and migration examples. No silent compatibility aliases or
parallel Python implementation will be retained.

## 5. Implementation sequence and acceptance gates

### Phase A — Contract and crate foundation

- Create `Cargo.toml`, committed `Cargo.lock`, `rustfmt.toml`, `clippy.toml`, `.editorconfig`, and
  crate metadata.
- Set `rust-version = "1.88"`; use stable in CI and test MSRV separately. CI pins explicit
  toolchains per job (stable and 1.88) via `RUSTUP_TOOLCHAIN`; distribution builds (AUR) use the
  system Rust toolchain. No `rust-toolchain.toml` is committed.
- Port independent protocol fixtures before implementation: frame bytes, checksums, profiles,
  status/version/basic/curve parsing, MC5000 status, and CSV output.
- Add property tests for arbitrary notification bytes and numeric boundaries.

**Gate:** the same independently recorded inputs produce the Python-compatible parsed values; all
invalid inputs fail without panic.

### Phase B — Typed protocol, profiles, and safety

- Implement `Slot` and `SlotMask` validation at the type boundary.
- Implement every existing opcode builder used by the server, including MC5000 read-only status.
- Implement all chemistry/mode defaults, including explicit-zero voltage behavior and break-in
  timing rules.
- Port all enforced safety limits and the four bundled profiles.
- Validate every public operation before any backend call.

**Gate:** protocol/safety tests cover consumer-visible boundaries, invalid chemistry fails closed,
all bundled profiles validate, and no dangerous backend call occurs on invalid input.

### Phase C — BLE and fake backends

- Implement powered-adapter discovery with actionable BlueZ/rfkill errors.
- Preserve official name filters, FFE0/FFE1 discovery, write-without-response, notifications,
  disconnect cleanup, and per-slot stop/write delays.
- Implement bounded notification routing and 245-byte curve reassembly.
- Implement a deterministic fake backend supporting scan, connect, requests, fragments, timeouts,
  disconnects, and write-order assertions.
- Add read-only CLI commands `scan` and `status`; default invocation starts MCP stdio. A explicit
  `--backend fake` mode supports development without hardware.

**Gate:** fake tests prove write ordering, timeout behavior, fragment reassembly, emergency-stop
availability, and disconnect cleanup without requiring a charger.

### Phase D — Complete MCP surface

- Implement all production and legacy tools with typed schemas.
- Implement resources, prompts, bundled-profile listing, JSON/CSV curve export, and audit export.
- Preserve dry-run and token behavior; apply the hardening rules from section 4.
- Add rmcp client integration tests and real-child-process stdio tests using the fake backend.

**Gate:** initialize, tools/list, every tool class, resources/list/read, prompts/list/get, malformed
requests, dry-run, missing/wrong/correct tokens, and emergency stop are exercised end to end.

### Phase E — Clean cutover and documentation

- Remove `pyproject.toml`, Python modules, pytest configuration, and Python CI/release paths.
- Update README, examples, companion skill references where necessary, safety docs, publishing docs,
  and migration notes.
- Document Cargo, crates.io, binary, and AUR installation; no pip/pipx/uvx instructions remain.
- Keep reverse-engineering research docs, but exclude scratch/reverse artifacts from the crate.

**Gate:** a fresh clone builds and runs using only documented Rust commands; example MCP client
configs launch the Rust binary.

## 6. Testing and quality policy

Required local and CI gates:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-features --locked
cargo doc --all-features --no-deps --locked
cargo audit
cargo deny check
cargo build --release --locked
cargo package --locked
```

- Test stable and Rust 1.88 MSRV; `Cargo.lock` remains committed for the binary.
- Enable strict Clippy lints and fix warnings rather than globally suppressing them.
- Use `cargo llvm-cov` as a coverage report with a 90% line floor; safety and protocol behavior is
  governed by explicit boundary tests, not coverage percentage alone.
- Use property tests for arbitrary device data and profile numeric boundaries.
- No test may write to or start real hardware. Live-dangerous tests are manual and separately
  confirmed.
- Run `cargo test` before packaging and test the actual packaged binary with `--version`, `scan`
  failure/success behavior, and fake stdio MCP initialization.

## 7. Repository quality and automation

Add or update only files with a concrete maintenance role:

- `README.md`: features, safety, install, configuration, MCP examples, development, support matrix;
- `CHANGELOG.md`: Keep a Changelog format and 0.2.0 migration notes;
- `CONTRIBUTING.md`: setup, gates, commit/PR flow, no-real-hardware test rule;
- `SECURITY.md`: hardware safety and private vulnerability reporting;
- `CODE_OF_CONDUCT.md`: Contributor Covenant;
- `.github/ISSUE_TEMPLATE/`: bug report and feature request forms;
- `.github/pull_request_template.md`: behavior, safety, tests, docs, and hardware-evidence checklist;
- `.github/CODEOWNERS`: repository owner review;
- `.github/dependabot.yml`: Cargo and GitHub Actions updates;
- CI: format, clippy, tests, MSRV, docs, audit/deny, fake stdio smoke;
- release: tag-driven build, test, crate/package verification, Linux artifacts, deterministic
  MCPB bundle with rendered `server.json`, checksums, artifact attestation, crates.io trusted
  publishing, GitHub Release notes, and AUR metadata verification;
- actions pinned to immutable revisions and granted only required permissions.

Do not add bots or status badges that are not connected to real workflows.

## 8. crates.io and MCP Registry

- Publish the binary/library crate as `mc3000-mcp` with `cargo publish` after `cargo package` and
  packaged-crate tests pass.
- Include README, LICENSE, repository, keywords, categories, MSRV, and explicit `include` rules.
- crates.io is not an MCP Registry `registryType`. MCP Registry distribution uses deterministic
  Linux x86_64 MCPB bundles published on GitHub Releases: `registryType: "mcpb"`, the release asset
  URL as the package identifier, and the required SHA-256 `fileSha256` of the bundle.
- The release pipeline packs the bundle (`packaging/mcpb/pack.sh`) and renders `server.json` from
  `packaging/mcpb/server.json.template` (`packaging/mcpb/render-server-json.sh`) against the
  official 2025-12-11 `server.schema.json`.
- Publish the companion skill through the same existing independent mechanism.

## 9. AUR package

Submit a source-built stable package named **`mc3000-mcp`** (no `-bin` or `-git` suffix).

`packaging/aur/` will contain:

- `PKGBUILD` with maintainer header, `pkgver=0.2.0`, `pkgrel=1`, SPDX `Apache-2.0`, `x86_64`,
  `makedepends=(cargo)`, and real dependencies discovered from the built binary;
- a tagged upstream source archive with a real `b2sums` entry;
- `cargo fetch --locked --target host-tuple` in `prepare()`;
- offline `cargo build --frozen --release` and `cargo test --frozen` in `build()`/`check()`;
- direct installation of `target/release/mc3000-mcp` into `/usr/bin` in `package()`;
- installed license and companion documentation/skill files where useful;
- generated `.SRCINFO` and a 0BSD license for the packaging sources.

Verification before submission:

1. Check Arch official repositories and AUR for name collisions.
2. Run `makepkg --printsrcinfo` and confirm `.SRCINFO` is synchronized.
3. Run `namcap` on `PKGBUILD` and the built package.
4. Build in a clean Arch chroot, install the package, and verify `/usr/bin/mc3000-mcp --version` plus
   fake-backend MCP initialization.
5. Verify AUR SSH authentication and review the exact package-only commit.
6. Publish to `ssh://aur@aur.archlinux.org/mc3000-mcp.git` on `master` after the 0.2.0 upstream tag
   and release artifacts exist.
7. Verify the AUR page and a fresh `makepkg -si` installation from the published AUR repository.

The stable AUR package is the required deliverable. A separate `-git` package is out of scope unless
explicitly requested later.

## 10. Live hardware verification

Read-only verification may proceed as soon as the Rust backend passes fake tests:

1. power/adapter diagnostics;
2. scan and select the expected charger;
3. connect and discover FFE0/FFE1;
4. read firmware/hardware and basic settings;
5. read all four slots and compare shape/meaning with the Python baseline;
6. read and export one voltage curve;
7. disconnect and confirm reconnect works.

No profile write, settings write, raw command, or slot start is part of implicit authorization.
Before any such test, record the exact physical slot, confirmed chemistry, capacity, cell count,
operation, currents, voltage limits, temperature cutoff, and confirmation token. Keep the charger
under observation and verify stop first. Firmware update and calibration/factory operations are not
test targets.

## 11. Explicit non-goals

- No simultaneous Python and Rust implementations after cutover.
- No HTTP/SSE server, remote exposure, or multi-user service.
- No automatic chemistry detection or relaxed limits for unvalidated chemistries.
- No MC5000 write/start support without independent real-hardware evidence.
- No firmware update, factory reset, or calibration-reset implementation.
- No unverified claims that a profile ACK proves the charger applied a profile.

## 12. Completion criteria

The rewrite is complete only when the Python path is removed, every MCP capability is implemented,
all quality gates pass from a fresh checkout, fake and real read-only paths are observed, release
artifacts are reproducible from the tag, crates.io packaging is valid, and the stable AUR package is
built, installed, published, and verified. Any live write/start remains a separate explicitly
confirmed hardware operation, not an implicit completion shortcut.
