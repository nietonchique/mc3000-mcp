# BLE protocol notes

See `docs/reverse-notes.md` for recovered frame details.

## Known GATT

- Service UUID: `0000FFE0-0000-1000-8000-00805f9b34fb`
- Write/notify characteristic UUID: `0000FFE1-0000-1000-8000-00805f9b34fb`
- Advertised names accepted by the official app: `SimpleBLEPeripheral`, `Charger`, `HitecCharger`

## Framing

- Normal command: 20 bytes.
- Profile command (`0x11`): 40 bytes, written as two 20-byte chunks.
- Checksum: sum of bytes with checksum byte zeroed, masked to `0xff`.
- Status/curve slot arguments are zero-based indexes.
- Start/stop frames use slot bitmasks.

## Safety caveat

A write ACK is not proof that a profile is active. Verify physical state by reading status after reconnect if necessary.

## References

- Official Android app reverse notes in `docs/reverse-notes.md`.
- Open-source reference used for cross-checking start/stop bitmasks: `kroimon/skyrc-ble`.
