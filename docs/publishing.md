# Publishing notes

This project has three distributable pieces:

1. the Rust MCP server binary/crate (`mc3000-mcp`), distributed via crates.io, GitHub release archives,
   the MCPB bundle, and the Arch Linux AUR package;
2. the Official MCP Registry metadata (`server.json`), which points at the signed/attested MCPB release
   asset;
3. the companion Hermes skill (`skill/SKILL.md`), distributed via the Hermes skill install URL.

They are advertised together but installed through different mechanisms.

## Short descriptions

- GitHub: Safe MCP server and agent skill for SKYRC MC3000-compatible BLE battery chargers, with
  code-enforced profile validation, dry-run defaults, app-compatible voltage curves, and emergency stop
  tools.
- crates.io: MCP stdio server for safely controlling SKYRC MC3000 battery chargers over Bluetooth Low
  Energy.
- Official MCP Registry: Battery charger MCP server exposing safe `charger.*` tools, profile validation
  resources, charger-stored voltage curve export, and prompts for supervised charging workflows.
- Hermes skill: `charger-agent` playbook for safe battery-profile selection and charger operation through
  the `mc3000-mcp` MCP server.
- AUR: Safe MCP server for SKYRC MC3000-compatible battery chargers over Bluetooth LE.

## End-user install snippet

```bash
# MCP server
cargo install mc3000-mcp --locked

# Arch Linux
paru -S mc3000-mcp

# Companion Agent Skill for Codex; replace the destination for another client
mkdir -p ~/.codex/skills
curl -fsSL \
  https://github.com/nietonchique/mc3000-mcp/releases/download/v0.2.4/mc3000-mcp-skill-v0.2.4.tar.gz \
  | tar -xz -C ~/.codex/skills
```

Register the stdio server with your MCP client using the `mcpServers` example above.

Hermes users may alternatively run:

```bash
hermes mcp add mc3000 --command mc3000-mcp
hermes skills install \
  https://raw.githubusercontent.com/nietonchique/mc3000-mcp/v0.2.4/skill/SKILL.md \
  --name charger-agent
```

## Release checklist

1. Ensure the working tree is clean and `Cargo.lock` is committed.
2. Run every local quality gate from `CONTRIBUTING.md`:
   - `cargo fmt --all -- --check`
   - `cargo clippy --all-targets --all-features --locked -- -D warnings`
   - `cargo test --all-features --locked`
   - `cargo doc --all-features --no-deps --locked`
   - `cargo audit`
   - `cargo deny check`
   - `cargo build --release --locked`
   - `cargo package --locked`
3. Bump the version together in:
   - `Cargo.toml`
   - `packaging/mcpb/manifest.json`
   - `packaging/mcpb/server.json.template`
   - `packaging/aur/PKGBUILD` and regenerate `packaging/aur/.SRCINFO`
   - root `server.json` only when updating registry metadata between releases
4. Update `CHANGELOG.md` and user-facing documentation.
5. Commit the version bump, then tag and push:

   ```bash
   git tag vX.Y.Z
   git push origin main vX.Y.Z
   ```

6. Wait for CI and the tag-driven Release workflow. It verifies the tag/version match, tests, builds,
   packages, creates the Linux x86_64 archive, deterministic MCPB bundle, standalone Agent Skill
   archive, and reproducible source archive; renders release registry metadata with the real
   SHA-256, writes BLAKE2b checksums, and attaches build provenance.
7. Inspect the GitHub release artifacts and attestations.
8. Publish the crate only after artifact inspection:

   ```bash
   cargo publish --locked
   ```

   Prefer configured crates.io trusted publishing once it is enabled for this repository.
9. Submit/update the rendered release `server.json` with the Official MCP Registry following the
   registry's current process. Do not submit placeholder checksums.
10. Publish the AUR update:
    - download the release assets `mc3000-mcp-<version>-source.tar.gz` and
      `mc3000-mcp-<version>-source.tar.gz.b2`; replace the `b2sums` entry after
      `b2sum -c --strict`;
    - run `makepkg --printsrcinfo > .SRCINFO`;
    - build in a clean chroot and run `namcap`;
    - install the built package and smoke-test `mc3000-mcp --version` plus `mc3000-mcp --backend fake`;
    - commit `PKGBUILD` and `.SRCINFO` to the AUR `master` branch and push.
11. Publish/list the companion Agent Skill:
    - verify the `mc3000-mcp-skill-<version>.tar.gz` release asset and its BLAKE2b checksum;
    - clients install the folder using their normal SKILL.md directory, for example
      `~/.codex/skills`, `~/.omp/agent/skills`, or `~/.claude/skills`;
    - the `hermes skills install` command remains optional for Hermes users.

## Do not publish yet if

- Any fmt/clippy/test/doc/audit/deny gate fails.
- A supported chemistry has no enforced limits.
- A non-emergency tool can write/start without its explicit confirmation token.
- The MCPB manifest tool list does not match runtime `tools/list`.
- The AUR `b2sums` entry is still the marked placeholder.
- Build artifacts contain reverse-engineering scratch files, APKs, target directories, or local caches.

## Notes for skill distribution

`skill/SKILL.md` is intentionally self-contained so raw URL installation works. Files in
`skill/profiles/` and `skill/checklists/` are examples/checklists for humans and future bundle
registries; safety enforcement lives in the Rust server, not in those files.
