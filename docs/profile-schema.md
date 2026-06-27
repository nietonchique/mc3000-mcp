# Profile schema

The canonical JSON Schema is exposed at MCP resource `charger://profile-schema` and implemented in `mc3000_mcp.safety.PROFILE_SCHEMA`.

Profiles describe battery facts and intended operation. They are validated before any live write. The MCP server, not the LLM, enforces hard limits.

## Required fields

- `profile_id`
- `chemistry`
- `capacity_mah`
- `cell_count`
- `operation`
- `charge_current_ma`
- `discharge_current_ma`
- `discharge_cutoff_mv`
- `temperature_cutoff_c`
- `temperature_policy`
- `source`
- `risk_level`

## Common optional fields

- `nominal_voltage_mv`
- `charge_voltage_mv`
- `timeout_minutes`
- `cycle_count`
- `cycle_mode`
- `negative_delta_mv`
- `trickle_current_ma`
- `notes`

## Supported operations

The current MC3000 mapping supports:

- `charge`
- `discharge`
- `refresh`
- `break_in` for NiMH/NiCd/Eneloop-style cells
- `cycle`
- `storage` for supported lithium chemistries

Unsupported chemistry/operation combinations are rejected by `charger.validate_profile`.

## Example: conservative NiMH AA charge

```json
{
  "profile_id": "nimh-aa-conservative-charge",
  "chemistry": "NiMH",
  "capacity_mah": 2600,
  "cell_count": 1,
  "nominal_voltage_mv": 1200,
  "operation": "charge",
  "charge_current_ma": 260,
  "discharge_current_ma": 200,
  "charge_voltage_mv": 1650,
  "discharge_cutoff_mv": 1000,
  "timeout_minutes": 960,
  "temperature_cutoff_c": 45,
  "temperature_policy": "stop at cutoff; keep charger supervised",
  "negative_delta_mv": 3,
  "trickle_current_ma": 0,
  "source": "cell label + MC3000 manual",
  "risk_level": "low",
  "notes": "Slow supervised charge."
}
```

## Example validation call

```json
{
  "name": "charger.validate_profile",
  "arguments": {
    "profile_id": "nimh-aa-conservative-charge"
  }
}
```

## Safety notes

- Never infer chemistry from physical size. AA/AAA can be NiMH, NiCd, NiZn, RAM, alkaline primary, lithium primary, or Li-ion in a similar form factor.
- MC3000 slots are single-cell slots. Multi-cell packs require a separate supported workflow and should not be treated as a single slot profile.
- Primary/non-rechargeable cells must not be charged.
- `risk_level=high` is allowed as data but should trigger extra human review and supervision.

Run `charger.validate_profile` before `charger.apply_profile`; run `charger.apply_profile` dry-run before any live write.
