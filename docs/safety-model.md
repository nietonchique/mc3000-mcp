# Safety model

Safety is enforced in code first and documented second. Agent instructions are not a safety boundary.

## Enforced by code

- Profile validation requires explicit chemistry, capacity, cell count, operation, currents, cutoffs, temperature policy, profile id, source, and risk level.
- Unsupported chemistry or operation is rejected.
- Chemistry-specific conservative current, voltage, discharge cutoff, cell-count, and temperature limits are enforced in `mc3000_mcp.safety`.
- `charger.apply_profile` is dry-run by default.
- Live profile writes require `confirmation_token=APPLY_PROFILE_SLOT_<slot>`.
- `charger.start` requires `confirmation_token=START_SLOT_<slot>`.
- Emergency stop does not depend on profile validation.
- Session logs redact device addresses.
- App-compatible voltage-curve reads are read-only and do not require dangerous-action confirmation.

## Companion skill

`skill/SKILL.md` is an agent playbook, not a safety boundary. It teaches Hermes/LLM agents to ask for missing battery facts, prefer conservative profiles, show dry-run summaries, require explicit confirmation, monitor status after start, and use emergency stop on unsafe state. All hard limits and dangerous-action gates must remain enforced in MCP server code.

## Guidance only

- User supervision and visual inspection.
- Whether a cell wrapper/datasheet is authentic.
- Whether a cell is physically damaged, hot, leaking, or counterfeit.
- Whether an unknown clone interprets the BLE profile exactly like a tested MC3000.

## Non-goals

- The LLM must not infer missing chemistry or cell count.
- The MCP server does not make hidden network calls.
- The server does not upload telemetry.
- The server is not a replacement for the charger firmware's own protection circuits.

## Dangerous workflow

1. Scan/connect/read status.
2. Validate profile.
3. Apply profile in dry-run.
4. Show profile summary and required token.
5. Only after explicit human confirmation, apply profile live.
6. Only after a second explicit confirmation, start.
7. Monitor status after start.
8. Stop immediately if current, voltage, temperature, chemistry, or slot differs from expectation.
