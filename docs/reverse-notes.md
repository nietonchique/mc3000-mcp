# MC3000 reverse-engineering notes

These notes document the BLE protocol recovered from the official SKYRC MC3000 Android application version 4.1.2.

The repository does not include the APK or decompiled application code. Keep APK/decompiler outputs local under `reverse/`; that directory is ignored by git.

## BLE transport

The app uses FastBle over a single GATT service/characteristic pair:

- Service UUID: `0000FFE0-0000-1000-8000-00805f9b34fb`
- Write/notify characteristic UUID: `0000FFE1-0000-1000-8000-00805f9b34fb`
- Scan names: `SimpleBLEPeripheral`, `Charger`, `HitecCharger`

The same characteristic is used for writes and notifications.

## Frame format

Most commands are 20 bytes:

| Offset | Meaning |
| --- | --- |
| 0 | Constant `0x0f` |
| 1 | Opcode |
| 2..18 | Payload |
| 19 | Checksum |

Checksum is `sum(frame with checksum byte set to 0) & 0xff`.

Profile/program commands are 40 bytes:

| Offset | Meaning |
| --- | --- |
| 0 | Constant `0x0f` |
| 1 | Opcode `0x11` |
| 2..29 | Profile payload |
| 30..38 | Reserved/zero in observed app output |
| 39 | Checksum over bytes `0..38` |

The Android app writes the 40-byte profile as two 20-byte BLE writes with about 50 ms delay.

## Opcodes

| Opcode | Direction | Meaning |
| --- | --- | --- |
| `0x05` | write/notify | Start selected slot / start ACK |
| `0xfe` | write | Stop selected slot |
| `0x11` | write/notify | Set profile / profile ACK |
| `0x55` | write/notify | Get slot status / slot status response |
| `0x56` | write/notify | Get voltage curve / 245-byte curve response |
| `0x57` | write/notify | Get firmware/hardware version / version response |
| `0x61` | write/notify | Get basic settings / basic settings response |
| `0x63` | write/notify | Set basic settings / ACK |
| `0x65` | write/notify | Restore factory settings / ACK |
| `0x66` | write/notify | Restore calibration data / ACK |

Factory and calibration reset opcodes are documented but not exposed as first-class MCP tools for safety.

## Slot status response (`0x55`, 20 bytes)

| Offset | Meaning |
| --- | --- |
| 2 | Slot, zero-based `0..3` |
| 3 | Battery type code |
| 4 | Mode code |
| 5 | Count |
| 6 | Status code |
| 7..8 | Time seconds, big-endian u16 |
| 9..10 | Voltage mV, big-endian u16 |
| 11..12 | Current mA, big-endian u16 |
| 13..14 | Capacity mAh, big-endian u16 |
| 15 | Temperature |
| 16..17 | Internal resistance mΩ, big-endian u16 (`0`, `1`, `65535` = N/A) |
| 18 | LED status |

## Battery type codes

| Code | Type |
| --- | --- |
| 0 | LiIon |
| 1 | LiFe |
| 2 | LiIo4.35 |
| 3 | NiMH |
| 4 | NiCd |
| 5 | NiZn |
| 6 | Eneloop |
| 7 | RAM |
| 8 | LTO |
| 9 | Na-Lion |

## Mode codes

| Code | Mode |
| --- | --- |
| 0 | Charge |
| 1 | Refresh |
| 2 | Storage for lithium chemistries, Break-in for NiMH/NiCd-like chemistries |
| 3 | Discharge |
| 4 | Cycle |

## Status codes

| Code | Status |
| --- | --- |
| 0 | Standby |
| 1 | Charge |
| 2 | Discharge |
| 3 | Pause |
| 4 | Completed |
| 128 | Input voltage low |
| 129 | Input voltage high |
| 130 | MCP3424-1 error |
| 131 | MCP3424-2 error |
| 132 | Connect break |
| 133 | Check voltage |
| 134 | Capacity cutoff |
| 135 | Time cutoff |
| 136 | System temperature high |
| 137 | Battery temperature cutoff |
| 138 | Short circuit |
| 139 | Polarity error |

## Basic settings response (`0x61`) / write (`0x63`)

| Offset | Meaning |
| --- | --- |
| 2 | Temperature unit: `0` Celsius, `1` Fahrenheit |
| 3 | System beep: `0/1` |
| 4 | Display mode index |
| 5 | Screensaver: `0/1` |
| 6 | Cooling fan index |
| 7..8 | Input voltage mV, big-endian u16 |

## Version response (`0x57`)

The app parses:

- Firmware: `(byte14 * 100 + byte15) / 100`
- Hardware: `(byte16 * 10) / 100`

## Voltage curve response (`0x56`, 245 bytes)

| Offset | Meaning |
| --- | --- |
| 2 | Slot, zero-based |
| 3..4 | Sample interval seconds, big-endian u16; app multiplies by 1000 |
| 5..244 | 120 voltage points, mV, big-endian u16 |

## Profile payload (`0x11`, 40 bytes)

| Offset | Meaning |
| --- | --- |
| 2 | Slot mask: slot1=`1`, slot2=`2`, slot3=`4`, slot4=`8` |
| 3 | Battery type code |
| 4 | Mode code |
| 5..6 | Capacity mAh |
| 7..8 | Charge current mA |
| 9..10 | Discharge current mA |
| 11..12 | Charge stop voltage mV |
| 13..14 | Discharge stop voltage mV |
| 15..16 | Charge stop current mA |
| 17..18 | Discharge stop current mA |
| 19 | Charge rest minutes |
| 20 | Cycle count |
| 21 | Cycle mode |
| 22 | Negative voltage delta mV |
| 23 | Trickle/eddy current in 10 mA units |
| 24..25 | Keep voltage mV |
| 26 | Temperature cutoff |
| 27..28 | Time limit minutes; zero for break-in |
| 29 | Discharge rest minutes |
| 39 | Checksum |

Cycle mode codes for normal cycle:

- `0`: `C>D`
- `1`: `C>D>C`
- `2`: `D>C`
- `3`: `D>C>D`

Cycle mode codes for break-in:

- `0`: `C>D>C`
- `1`: `D>C>D`
