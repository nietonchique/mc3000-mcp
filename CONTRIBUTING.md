# Contributing

Thanks for helping improve `mc3000-mcp`. This is a safety-relevant hardware tool: correctness and
fail-closed behavior matter more than features.

## Setup

- Rust stable via [rustup](https://rustup.rs); the MSRV is **1.88** (see `Cargo.toml` and
  `clippy.toml`). CI tests both stable and 1.88.
- Commit `Cargo.lock` (this repository ships a binary).
- No hardware is required for development: use the fake backend (`--backend fake`) for all local
  testing.

## Quality gates

All of these must pass before a PR can merge; CI runs the same commands:

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

Rules:

- Fix Clippy warnings; do not suppress them globally or add `allow` attributes without a written
  justification in the PR.
- No production `unwrap`, `expect`, or panics on device-controlled data.
- No test may write to or start real hardware. Live-dangerous behavior is covered by manual,
  separately confirmed tests only.
- Keep stdout protocol-only; logs go to stderr.

## Commit and PR flow

1. Branch from `main`; keep commits focused (`feat:`, `fix:`, `docs:`, `test:`, `chore:` prefixes
   are appreciated but not enforced).
2. Open a PR and fill in the checklist from `.github/pull_request_template.md`.
3. Any change to write/start paths must state the safety impact explicitly: what is validated
   before the backend call, and what stays fail-closed.
4. Changes to packaging (`packaging/aur/`) must keep `.SRCINFO` synchronized:
   `makepkg --printsrcinfo > .SRCINFO` from `packaging/aur/`.

## Reporting

- Security or hardware-safety issues: see `SECURITY.md` (private reporting).
- Behavior bugs and feature requests: GitHub issue templates.
