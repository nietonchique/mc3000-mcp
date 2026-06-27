# Profile schema

The canonical JSON Schema is exposed at MCP resource `charger://profile-schema` and implemented in `mc3000_mcp.safety.PROFILE_SCHEMA`.

Required fields:

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

Common optional fields:

- `nominal_voltage_mv`
- `charge_voltage_mv`
- `timeout_minutes`
- `cycle_count`
- `cycle_mode`
- `negative_delta_mv`
- `trickle_current_ma`
- `notes`

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

Run `charger.validate_profile` before `charger.apply_profile`.
