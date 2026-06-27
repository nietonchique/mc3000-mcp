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

## Choosing MC3000 vs MC5000

Do not choose the protocol by battery chemistry, capacity, slot number, or the fact that the charger has four slots. MC3000 and MC5000 can both charge common AA/AAA NiMH cells; the device/protocol model is a charger property, not a battery property.

Selection rule for agents and integrations:

1. If the user explicitly says the physical charger is MC5000, use the MC5000 adapter/read path.
2. If the user explicitly says MC3000, use the MC3000 adapter/read path.
3. If the model is unknown, connect read-only and probe in this order:
   - try the MC3000 status frame (`0x55`) for one slot;
   - if that times out or parses as unknown, try the MC5000 slot status frame (`0x91`) for the same slot;
   - cache the detected model for the session and include it in status output.
4. If neither read-only probe succeeds, do not send profile/start/write commands. Report the device as unsupported/unknown.

Safety rule: MC5000 support must stay read-only until live hardware testing proves the profile/write/start frame layout. MC3000 profile writes are not automatically safe for MC5000.

Known protocol distinction:

- MC3000 status: opcode `0x55`, slot is a zero-based index (`0..3`).
- MC5000 slot status: opcode `0x91`, channel is a bitmask (`1 << slot`) and the response uses a different field layout.

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
- MC5000 live writes/profile apply/start. MC5000 status parsing is based on the public `kolinger/skyrc-mc3000` implementation and must be live-verified before enabling dangerous operations.
- Historical non-voltage telemetry from the charger; implement explicit polling only if a future use case requires it.

## Compatibility policy

The server refuses profiles whose chemistry has no enforced limits. New devices should be added by implementing a device adapter, capability object, protocol fixtures, fake-device tests, docs, and safety limits before exposing dangerous writes.
