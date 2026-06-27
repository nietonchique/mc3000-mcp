---
name: charger-agent
description: Safely operate BLE battery chargers through MCP tools. Requires explicit battery facts, dry-run validation, and human confirmation before starting hardware.
---

# Charger Agent Skill

Use this skill when a user asks to charge, discharge, refresh, break-in, store, stop, inspect, or profile batteries with the charger MCP server.

## Safety principle

The LLM is not the safety layer. The MCP server enforces hard limits, but you must still collect facts and refuse unsafe ambiguity.

## Required facts before dangerous actions

Ask for missing facts before applying or starting a profile:

- charger slot / physical slot mapping;
- battery chemistry;
- capacity in mAh;
- cell count or nominal voltage;
- operation: charge, discharge, refresh, break-in, storage, cycle;
- requested current;
- voltage/cutoff values;
- temperature cutoff/policy;
- profile id/source/risk level;
- whether the user can supervise the charger.

Never guess chemistry, cell count, capacity, or a Li-ion/LiPo pack configuration.

## Normal workflow

1. Read status:
   - `charger.scan_devices`
   - `charger.connect`
   - `charger.get_status` or `charger.read_slots`
2. Choose a conservative bundled profile with `charger.list_profiles`, or build a full explicit profile from user facts.
3. Validate:
   - `charger.validate_profile`
4. Dry-run apply:
   - `charger.apply_profile` with `dry_run` omitted or `true`.
5. Show the dry-run summary: slot, chemistry, capacity, operation, currents, voltage/cutoff, timeout, temperature cutoff, risk, validation warnings, required confirmation token.
6. Only if the user explicitly confirms, apply live:
   - `charger.apply_profile` with `dry_run=false` and `confirmation_token=APPLY_PROFILE_SLOT_<slot>`.
7. Require a separate explicit confirmation before start:
   - `charger.start` with `confirmation_token=START_SLOT_<slot>`.
8. Monitor after start with `charger.read_slots`.
9. Stop if status, chemistry, current, voltage, temperature, or slot does not match expectation.

## Refuse or stop when

- Chemistry/cell count/capacity is unknown.
- The user asks to charge damaged, hot, swollen, leaking, or unidentifiable cells.
- A multi-cell LiPo/Li-ion pack is requested without supported per-cell/balance handling.
- Validation fails.
- The charger reports unexpected current, mode, or temperature.

## Emergency stop

For unsafe state or user asks to stop:

- `charger.stop_slot` for one slot.
- `charger.stop_all` for all slots.

Do not wait for profile validation before emergency stop.

## Final checklist

- [ ] Read current status.
- [ ] Required facts collected.
- [ ] Profile validated.
- [ ] Dry-run shown to user.
- [ ] Live apply confirmed separately.
- [ ] Start confirmed separately.
- [ ] Status monitored after start.
- [ ] Safe final state reported.
