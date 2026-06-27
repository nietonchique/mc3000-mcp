# Examples

All examples use production `charger.*` tools. Legacy `mc3000_*` tools are still available for protocol debugging, but normal agent workflows should prefer the safe namespace.

Slot numbers are zero-based: physical slot 1 is `slot: 0`, physical slot 4 is `slot: 3`.

## Scan, connect, and read status

```json
{"name": "charger.scan_devices", "arguments": {"timeout": 5}}
```

```json
{
  "name": "charger.connect",
  "arguments": {
    "address": "AA:BB:CC:DD:EE:FF"
  }
}
```

```json
{"name": "charger.read_slots", "arguments": {}}
```

## Validate a custom profile

```json
{
  "name": "charger.validate_profile",
  "arguments": {
    "profile": {
      "profile_id": "nimh-aa-example",
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
      "temperature_policy": "stop at cutoff; supervised",
      "negative_delta_mv": 3,
      "trickle_current_ma": 0,
      "source": "cell wrapper",
      "risk_level": "low"
    }
  }
}
```

## Dry-run a bundled profile

```json
{
  "name": "charger.apply_profile",
  "arguments": {
    "slot": 3,
    "profile_id": "nimh-aa-conservative-charge"
  }
}
```

The result includes validation and a built profile frame but does not write to hardware.

## Apply after explicit confirmation

```json
{
  "name": "charger.apply_profile",
  "arguments": {
    "slot": 3,
    "profile_id": "nimh-aa-conservative-charge",
    "dry_run": false,
    "confirmation_token": "APPLY_PROFILE_SLOT_3"
  }
}
```

## Start after separate explicit confirmation

```json
{
  "name": "charger.start",
  "arguments": {
    "slot": 3,
    "confirmation_token": "START_SLOT_3"
  }
}
```

## Export the app-compatible voltage curve

```json
{
  "name": "charger.export_voltage_curve",
  "arguments": {
    "slot": 3,
    "format": "csv"
  }
}
```

This uses the charger's own `0x56` curve command, the same mechanism the Android app uses. It is not a polling telemetry logger; the exported time-series contains voltage samples only.

CSV shape:

```csv
index,time_seconds,voltage_mv,voltage_v
0,0.0,4100,4.100
1,5.0,4110,4.110
```

## Emergency stop

Stop one slot:

```json
{"name": "charger.stop_slot", "arguments": {"slot": 3}}
```

Stop all slots:

```json
{"name": "charger.stop_all", "arguments": {}}
```

Emergency stop tools intentionally bypass profile validation.
