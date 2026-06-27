# Publishing notes

This project has two distributable pieces:

1. the MCP server package (`mc3000-mcp`), distributed via PyPI/pipx/uvx and MCP catalogs;
2. the companion Hermes skill (`skill/SKILL.md`), distributed via Hermes skill install URL/registry/tap.

They should be advertised together, but they are installed through different mechanisms. `server.json` describes the MCP server only; it does not automatically install the skill.

## Short descriptions

- GitHub: Safe MCP server and agent skill for SKYRC MC3000-compatible BLE battery chargers, with code-enforced profile validation, dry-run defaults, app-compatible voltage curves, and emergency stop tools.
- PyPI: MCP stdio server for safely controlling SKYRC MC3000 battery chargers over Bluetooth Low Energy.
- Official MCP Registry: Battery charger MCP server exposing safe `charger.*` tools, profile validation resources, charger-stored voltage curve export, and prompts for supervised charging workflows.
- Hermes skill: `charger-agent` playbook for safe battery-profile selection and charger operation through the `mc3000-mcp` MCP server.
- Glama/PulseMCP/Smithery/mcp.so: Safe BLE battery charger control for MC3000-compatible devices; dry-run by default, explicit confirmation required for writes/start.
- Awesome MCP Servers: Hardware-control MCP server for SKYRC MC3000 BLE chargers with strict safety validation and companion agent skill.

## End-user install snippet

```bash
# MCP server
pipx install mc3000-mcp
hermes mcp add mc3000 --command "pipx run mc3000-mcp"
hermes mcp test mc3000

# Companion skill
hermes skills install \
  https://raw.githubusercontent.com/nietonchique/mc3000-mcp/main/skill/SKILL.md \
  --name charger-agent
```

Users can then start Hermes with:

```bash
hermes -s charger-agent
```

or load the skill in-session with `/skill charger-agent`.

## Release checklist

1. Ensure working tree is clean.
2. Run local quality gates:
   - `python -m pytest -q`
   - `python -m ruff check .`
   - `python -m ruff format --check .`
   - `python -m mypy src tests`
   - `python -m bandit -q -c pyproject.toml -r src tests`
   - `python -m pip_audit --progress-spinner off --strict .`
3. Bump version in:
   - `pyproject.toml`
   - `server.json`
4. Build and inspect artifacts:
   - `python -m build`
   - `python -m twine check dist/*`
5. Commit the version bump and tag release:
   - `git tag vX.Y.Z`
   - `git push origin main vX.Y.Z`
6. Wait for GitHub Actions release workflow to pass.
7. Upload to PyPI only after inspecting artifacts:
   - `python -m twine upload dist/*`
8. Submit/update `server.json` to the Official MCP Registry following the registry's current process.
9. Publish/list the companion skill:
   - raw install URL is already usable;
   - if publishing to Hermes skill registry, run the registry's current `hermes skills publish ...` flow with `skill/` as the source bundle if supported.
10. Add listings to Glama, PulseMCP, Smithery, mcp.so, and Awesome MCP Servers using the descriptions above.

## Do not publish yet if

- Any test/lint/type/security gate fails.
- A supported chemistry has no enforced limits.
- A dangerous tool can write/start without explicit confirmation.
- The README, SECURITY warning, `server.json`, or companion skill install instructions are missing.
- Build artifacts include reverse-engineering scratch files, APKs, venvs, or local caches.

## Notes for skill distribution

`skill/SKILL.md` is intentionally self-contained so raw URL installation works. The files in `skill/profiles/` and `skill/checklists/` are examples/checklists for humans and future bundle registries; safety enforcement lives in MCP server code, not in those files.
