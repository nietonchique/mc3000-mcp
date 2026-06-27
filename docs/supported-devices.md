# Supported devices

## Confirmed

### SKYRC MC3000-compatible BLE charger

Observed device identity:

- BLE advertised name: `Charger`
- Service UUID: `0000FFE0-0000-1000-8000-00805f9b34fb`
- Characteristic UUID: `0000FFE1-0000-1000-8000-00805f9b34fb`
- Slot count: 4
- Status slots: zero-based `0..3`
- Start/stop protocol: slot bitmask

Live-tested operations on NiMH in physical slot 4 / API slot 3:

- Charge
- Discharge
- Refresh
- Break-in
- Cycle

## Not yet supported

- Multi-cell balance charging packs.
- USB firmware update mode.
- Chargers that do not expose the MC3000 BLE protocol.

## Compatibility policy

The server refuses profiles whose chemistry has no enforced limits. New devices should be added by implementing a device adapter, capability object, fixtures, and fake-device tests before exposing dangerous writes.
