# Summary

<!-- One or two sentences: what changes and why. -->

## Behavior

<!-- What user-visible behavior changes? If tool names, read-only response fields, slot
numbering, or protocol handling change, call it out explicitly. -->

## Safety

- [ ] No write/start path reaches the backend before validation passes.
- [ ] Fail-closed behavior for unsupported chemistries and unknown models is preserved.
- [ ] Every non-emergency write/start still requires an explicit confirmation; emergency stop
      stays tokenless.
- [ ] Stdout remains protocol-only (logs to stderr).

## Tests

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --all-targets --all-features --locked -- -D warnings`
- [ ] `cargo test --all-features --locked`
- [ ] New/changed behavior is covered by tests; no test touches real hardware.

## Documentation

- [ ] README / docs updated for user-visible changes.
- [ ] `CHANGELOG.md` has an entry.
- [ ] `packaging/aur/.SRCINFO` regenerated if packaging inputs changed.

## Hardware evidence

<!-- Required only for PRs that change protocol, transport, or write/start behavior validated
against a powered charger. Leave "not applicable" otherwise. Live write/start tests are manual and
separately confirmed; read-only evidence must record slot, chemistry, capacity, and charger model. -->

- [ ] Not applicable.
