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

## Battery-type decision notes

Use these as workflow guidance only; the MCP server's validation is the enforcement layer.

### NiMH / Eneloop AA/AAA

Typical rechargeable household cells. Nominal voltage is 1.2V per cell.

- If the wrapper gives a standard charge current/time, prefer that over generic C-rate advice.
- Conservative charge: about 0.1C when the user wants gentle charging, or the wrapper's stated standard charge.
- Fast charge is possible only when the cell/datasheet supports it and the user accepts supervision.
- Discharge cutoff: prefer 1.00V/cell for conservative testing; do not go low just to extract capacity.
- Temperature cutoff: use a conservative cutoff such as 45°C unless the user provides a better value.
- Delta peak/trickle are Ni-only termination helpers. Do not raise delta peak or enable trickle casually.
- `Refresh`: use when the user wants to measure/restore capacity or sort cells. Compare measured capacity; cells in a pack should be close, e.g. within roughly 5%.
- `Break-in`: use for new, long-stored, or suspected underperforming NiMH/NiCd cells when the user explicitly wants forming. Prefer slow C/10 charge and C/5 discharge behavior.
- `Cycle`: use for diagnostics/capacity measurement; avoid endless cycling as a default maintenance habit.

### NiCd

Older rechargeable chemistry with 1.2V nominal voltage.

- Similar workflow to NiMH, but ask explicitly because NiCd is uncommon and may be confused with NiMH.
- Use conservative currents unless the label/datasheet says otherwise.
- `Refresh`/`Cycle` can be useful for old cells, but stop if they heat or behave inconsistently.

### NiZn / RAM

Less common 1-cell chemistries with different voltage limits.

- Never treat NiZn/RAM as NiMH just because the form factor is AA/AAA.
- Require an explicit chemistry label and capacity.
- Use only profiles whose chemistry is exactly NiZn/RAM and let `charger.validate_profile` enforce voltage limits.
- If the label is unclear, refuse charging until identified.

### Li-ion cylindrical cells, e.g. 10440/14500/18650/21700

Same physical size as AA/AAA does not mean same chemistry. Li-ion cells have much higher voltage and stricter safety requirements.

- Require chemistry, nominal voltage, max charge voltage, capacity, and whether the cell is protected/unprotected.
- Standard Li-ion is usually 4.20V max; LiIo4.35 is 4.35V max; LiFePO4 is lower voltage. Do not mix them.
- Use conservative current, typically <=0.5C unless the datasheet supports more.
- Storage mode is appropriate for cells that will not be used soon.
- Refuse swollen, hot, damaged, unwrapped, unknown, or salvaged cells unless the user is doing a clearly bounded diagnostic discharge with supervision.
- MC3000 slots are single-cell; do not treat a multi-cell pack as supported unless each cell is handled individually and safely.

### Primary/non-rechargeable cells

- Refuse charging alkaline, lithium primary, zinc-carbon, silver oxide, or any cell marked non-rechargeable.
- Offer only safe disposal/recycling guidance or, if supported and safe, a diagnostic voltage read/discharge plan without charging.

### Mixed sets and packs

- Do not start a multi-slot program when slots contain mixed chemistries or unknown cells.
- For cells used together in a device, prefer testing/refreshing individually, then grouping by similar measured capacity and behavior.
- Stop if one cell heats faster, has much lower capacity, or voltage behavior diverges strongly.

## Mode selection quick rules

- Simple known-good rechargeable cell: `charge`.
- Unknown remaining capacity but known safe chemistry: status/read first; then conservative `charge` or `discharge` only if requested.
- Capacity measurement / matching cells: `refresh` or `cycle`, with conservative current and rest periods.
- Long-stored NiMH/NiCd forming: `break_in`, after explicit user request.
- Lithium cell not used soon: `storage` when supported for that chemistry.
- Emergency / unexpected heat/current/mode: `charger.stop_slot` or `charger.stop_all` immediately.

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
