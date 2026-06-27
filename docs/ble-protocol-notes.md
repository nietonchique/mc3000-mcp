# BLE protocol notes

See `docs/reverse-notes.md` for detailed recovered frame notes. This file is the concise protocol reference for implementers.

## Known GATT

- Service UUID: `0000FFE0-0000-1000-8000-00805f9b34fb`
- Write/notify characteristic UUID: `0000FFE1-0000-1000-8000-00805f9b34fb`
- Advertised names accepted by the official app: `SimpleBLEPeripheral`, `Charger`, `HitecCharger`

## Framing

- Normal command: 20 bytes.
- Profile command (`0x11`): 40 bytes, written as two 20-byte chunks.
- Voltage curve command (`0x56`): charger-stored app-compatible voltage history for one slot.
- Voltage curve response contains slot, sample interval, and up to 120 voltage points; zero marks unused tail.
- Checksum: sum of bytes with checksum byte zeroed, masked to `0xff`.
- Status/curve slot arguments are zero-based indexes (`0..3`).
- Start/stop frames use slot bitmasks (`1 << slot`).

## Core opcodes

| Opcode | Direction | Meaning |
| --- | --- | --- |
| `0x05` | write | start slot bitmask |
| `0x11` | write | set profile/program, 40-byte payload |
| `0x55` | read/notify | slot status |
| `0x56` | read/notify | voltage curve |
| `0x57` | read/notify | firmware/hardware version |
| `0x61` | read/notify | basic settings |
| `0x63` | write/ack | set basic settings |
| `0xFE` | write | stop slot bitmask |

## MC3000 vs MC5000 status selection

The server must not infer charger model from inserted batteries. Detect or select the model using read-only charger/protocol behavior:

- MC3000 status read uses opcode `0x55` with a zero-based slot argument.
- MC5000 slot status read uses opcode `0x91` with a channel bitmask (`1 << slot`) and a different response layout.
- If the user/device config says MC5000, use the `0x91` path.
- If the model is unknown, probe `0x55` first and fall back to `0x91` only after timeout/unknown parse.
- Do not send profile writes, starts, or other mutating commands to a charger whose model was not detected.

The MC5000 status-layout facts were cross-checked against `kolinger/skyrc-mc3000`; do not copy GPL code into this Apache-2.0 project. Use independently written parsers and cite protocol facts only.

## Status fields

Status frame (`0x55`) includes:

- slot
- battery type
- mode
- cycle count
- status
- elapsed time
- voltage mV
- current mA
- capacity mAh
- temperature
- internal resistance mΩ
- LED state

## Voltage curve fields

Voltage curve frame (`0x56`) includes:

- byte 2: zero-based slot
- bytes 3..4: sample interval in seconds; the Android app multiplies by `1000`
- bytes 5..244: big-endian millivolt samples, up to 120 points
- zero sample means unused tail

The Android app appends current status voltage to the curve while charging/discharging. If the 120-point array fills, it keeps every second sample and continues appending. The MCP server exposes the charger-provided curve as `charger.get_voltage_curve` and `charger.export_voltage_curve`; it does not run a polling telemetry logger by default.

## Safety caveat

A write ACK is not proof that a profile is active. Verify physical state by reading status after reconnect if necessary. In live testing, invalid profile voltage defaults could be silently ignored while the previous program remained active.

## References

- Official Android app reverse notes in `docs/reverse-notes.md`.
- Open-source reference used for cross-checking start/stop bitmasks: `kroimon/skyrc-ble`.
