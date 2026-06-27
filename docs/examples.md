# Examples

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

## Start after explicit confirmation

```json
{
  "name": "charger.start",
  "arguments": {
    "slot": 3,
    "confirmation_token": "START_SLOT_3"
  }
}
```

## Emergency stop

```json
{"name": "charger.stop_all", "arguments": {}}
```
