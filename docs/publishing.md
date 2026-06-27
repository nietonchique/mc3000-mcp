# Publishing notes

## Short descriptions

- GitHub: Safe MCP server and agent skill for SKYRC MC3000-compatible BLE battery chargers, with code-enforced profile validation, dry-run defaults, and emergency stop tools.
- PyPI: MCP stdio server for safely controlling SKYRC MC3000 battery chargers over Bluetooth Low Energy.
- Official MCP Registry: Battery charger MCP server exposing safe `charger.*` tools, profile validation resources, and prompts for supervised charging workflows.
- Glama/PulseMCP/Smithery/mcp.so: Safe BLE battery charger control for MC3000-compatible devices; dry-run by default, explicit confirmation required for writes/start.
- Awesome MCP Servers: Hardware-control MCP server for SKYRC MC3000 BLE chargers with strict safety validation and charger operation skill.

## Manual release steps

1. Ensure working tree is clean and CI passes.
2. Bump version in `pyproject.toml` and `server.json`.
3. Tag release: `git tag vX.Y.Z && git push origin vX.Y.Z`.
4. Download GitHub Actions release artifacts or build locally with `python -m build`.
5. Upload to PyPI only after inspecting artifacts: `python -m twine upload dist/*`.
6. Submit/update `server.json` to the Official MCP Registry following the registry's current process.
7. Add listings to Glama, PulseMCP, Smithery, mcp.so, and Awesome MCP Servers using the descriptions above.

## Do not publish yet if

- Any test/lint/type/security gate fails.
- A supported chemistry has no enforced limits.
- A dangerous tool can write/start without explicit confirmation.
- The README or SECURITY warning is missing.
