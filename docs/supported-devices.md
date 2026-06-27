# Supported devices

## Confirmed

### SKYRC MC3000-compatible BLE charger

Observed device identity:

- BLE advertised name: `Charger`
- Service UUID: `0000FFE0-0000-1000-8000-00805f9b34fb`
- Characteristic UUID: `0000FFE1-0000-1000-8000-00805f9b34fb`
- Slot count: 4
- Status slots: zero-based `0..3`
- Start/stop protocol: slot bitmask (`1 << slot`)
- Voltage curve protocol: opcode `0x56`, zero-based slot argument

Live-tested operations on NiMH in physical slot 4 / API slot 3:

- Charge
- Discharge
- Refresh
- Break-in
- Cycle
- App-compatible voltage curve read/export

## Known caveats

- USB/microUSB firmware update mode is not implemented by this MCP server. BLE control can work even when the charger does not enumerate as a USB device on a PC.
- App-compatible curves are voltage-only historical data. Current/capacity/temperature/resistance are current status fields, not charger-provided historical series.
- A profile write ACK is not proof the charger accepted the profile. Verify by reading slot status and checking mode/current/status.
- Unknown firmware or clones may interpret profile fields differently. Keep live writes conservative.

## Not yet supported

- Multi-cell balance charging packs.
- USB firmware update mode.
- Chargers that do not expose the MC3000 BLE protocol.
- Historical non-voltage telemetry from the charger; implement explicit polling only if a future use case requires it.

## Compatibility policy

The server refuses profiles whose chemistry has no enforced limits. New devices should be added by implementing a device adapter, capability object, protocol fixtures, fake-device tests, docs, and safety limits before exposing dangerous writes.
