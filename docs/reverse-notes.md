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

## Voltage curve / app graph

The Android detail screen graph is not a generic telemetry plot. It is an app-compatible voltage curve:

- `DetailActivity` requests message `1018`, which sends `Config.getVoltageCurve(slot)`.
- `Config.getVoltageCurve(slot)` builds opcode `0x56` with a zero-based slot.
- `BleThread` collects the fragmented `0x56` response into a 245-byte buffer.
- Bytes `3..4` are the sample interval in seconds; the app multiplies by `1000` into `Config.getTime[slot]`.
- Bytes starting at `5..6` are big-endian millivolt samples, up to 120 points.
- Zero means unused tail. During a running charge/discharge, the app appends the current status voltage into the first zero slot.
- If the 120-point array fills, the app keeps every second sample (120 -> 60) and continues appending.
- Export writes date/slot/type/mode/final voltage/temp/capacity summary plus a two-column time/voltage table.

Current MCP mapping:

- `mc3000_get_voltage_curve` exposes the low-level command.
- `charger.get_voltage_curve` exposes the production-safe app-compatible time-series.
- `charger.export_voltage_curve` exports the same data as JSON or CSV.

The charger/app do not provide historical current/capacity/temperature series through this curve command; those are current status fields only.

## Live profile-write finding

The Android app's profile builder always fills chemistry-appropriate voltage fields before saving
`parameter_1`/`parameter_2`. A NiMH profile with `charge_stop_voltage=0` and `keep_voltage=0`
was accepted at the BLE write layer but ignored by the charger: starting the slot continued the
previous/default Charge program at about 200 mA.

After matching the Android/manual NiMH defaults (`charge_stop_voltage=1650mV`,
`discharge_stop_voltage=1000mV`, `charge_stop_current=50mA`, `keep_voltage=1000mV`), the same
`0x11` two-chunk write took effect on real hardware. Verified on physical slot 4 / API slot 3:

- Charge profile with `charge_current=500mA` started as `mode=charge`, `status=charge`,
  `current_ma=501`.
- Discharge profile with `discharge_current=200mA` started as `mode=discharge`,
  `status=discharge`, `current_ma=200`.

Conclusion: stop-before-write is necessary, but not sufficient. The profile must also contain
valid non-zero chemistry voltage defaults for fields the charger validates even if the UI hides them
for the selected mode.

A follow-up live probe confirmed the packet layout stays the same across modes, while mode-specific
semantics/defaults matter. Physical slot 4 / API slot 3 accepted and ran these NiMH profiles:

- Refresh: `mode_code=1`, `mode=refresh`, `status=charge`, `current_ma=260`.
- Break-in: `mode_code=2`, `mode=storage_or_breakin`, `status=discharge`, `current_ma=519`
  for a 2600mAh profile (`C/5` discharge phase).
- Cycle: `mode_code=4`, `mode=cycle`, `status=discharge`, `current_ma=200`.

Each probe was stopped afterwards and verified as `status=standby`, `current_ma=0`.

## Firmware dependency of the profile write

Observed on a second MC3000 (version reply: firmware 1.13, hardware 1.3; later updated to firmware
1.27 with SkyRC's updater), using the two-chunk `0x11` frames described above, with the NiMH
voltage defaults filled in and the slot stopped first:

- Firmware 1.13 never acknowledged the `0x11` write, sent either as write-without-response or as
  write-with-response (both ATT writes succeeded). The `0xFE` stop on the same connection was
  acknowledged. A USB read of the stored slot programs about 35 minutes later still showed the old
  programs.
- Firmware 1.27 acknowledges the write (`0f 11 <slot mask> f0 ff ff ...`, the same shape as the
  `0xFE` stop ACK) and applies it. All four slots written with a NiMH Cycle profile (100 mA
  discharge first) started in Cycle mode, discharging at about 100 mA.
- On 1.27, `0x55` keeps reporting the slot's previous mode after an acknowledged write. The mode
  changes only when the slot starts, so reading status right after a write can't confirm it;
  starting the slot and checking mode and current can.

Firmware versions between 1.13 and 1.27 were not tested.

## Firmware image analysis (V1.27)

The official `MC3000_Firmware_Update__V1.27.exe` is a .NET assembly that embeds two Intel HEX
resources. Extraction: unzip → read the .NET managed resources (search for contiguous Intel HEX
record runs) → convert with `intelhex.tobinfile()`.

- `UpgradeFirmware.rom.hex` → STM32F103 main firmware, ARM Cortex-M3, 88396 bytes @ `0x08004000`.
  Handles charging logic, display, BLE protocol parsing.
- `UpgradeFirmware.loader.hex` → C8051F380 USB-interface MCU, 8051, 45584 bytes @ `0x00000000`.
  Handles USB HID only; irrelevant for BLE protocol.

### BLE command dispatcher (`FUN_08015b48` @ `0x08015b48`)

Single entry point for all 20-byte frames. Parses `byte[1]` as opcode, `byte[2]` as slot mask
or slot index depending on command.

### ACK frame format (`FUN_080164e0`)

Every acknowledged command gets a 20-byte reply:

| Offset | Value |
| --- | --- |
| 0 | `0x0F` |
| 1 | original opcode (echo) |
| 2 | result/slot mask (usually echo of the request byte 2) |
| 3 | `0xF0` |
| 4 | `0xFF` |
| 5 | `0xFF` |
| 6–18 | `0x00` |
| 19 | checksum (sum of bytes 0–18) |

This is the `0f <opcode> <mask> f0 ff ff …` shape observed live. A reliable write-confirmation
path is: send command → wait for ACK with matching byte 1 and byte 2.

### Stop command (`0xFE`) — two semantically different paths

- **mask `0x0F` (all four bits)**: does NOT stop individual slots. Sets three global display/UI
  state flags (`[+0x0a]=2, [+0x0b]=2, [+0x0d]=0x32`). This is a "reset to overview screen" action,
  not a charge stop. Sending a single `0F FE 0F <checksum>` frame does not stop a running slot.
- **individual bit masks (`0x01`–`0x0E`)**: iterates slots 0–3; for each bit set: if the slot is
  in remote-control mode, clears the remote flag and triggers the stop; otherwise clears the
  slot-active flag. If all four slots were non-remote, also sets the display-reset flag.

Consequence: the server correctly uses per-slot stop frames. A hypothetical "stop all with one
frame" using mask `0x0F` is not viable.

### Profile write (`0x11`) — per-slot handler (`FUN_08016524`)

The dispatcher calls `FUN_08016524(slot)` for each bit set in `byte[2]`. The 40-byte frame payload
is copied from a staging buffer into per-slot storage (stride `0x22`) with this mapping:

| Frame byte(s) | Storage offset | Meaning |
| --- | --- | --- |
| 3 | +0 | mode code |
| 4 | +1 | sub-mode/secondary code |
| 5–14 (5× u16 LE) | +4…+0x0a | voltage/current program parameters |
| 15–28 (7× u16 LE) | +0x0c…+0x1e | additional parameters incl. cutoff-δV, temperature |
| 0x13 (frame byte 19) | +0x0f | flag byte |
| 0x14 | +0x0e | flag byte |
| 0x15 | +0x11 | flag byte |
| 0x16 | +0x12 | u8 value |
| 0x17 | +0x14 | u8 × 10 |
| 0x18–0x19 | +0x1c | u16 LE |
| 0x1a | +0x1e | u8 × 10 |
| 0x1b–0x1c | +0x20 | u16 LE |
| 0x1d | +0x10 | flag byte |

After copying, the dispatcher calls `FUN_080134b4(slot)` to stage the profile into the "active
program" slot structure (stride `0x26`), computing voltage defaults per chemistry (mode 0/1/2
from global settings; mode 5 → 1000 mV; mode 7 → 1200 mV; mode 8 → 1500 mV; mode 9 → 2000 mV)
and a cutoff voltage offset of `-0x14` (−0.20 V).

### Internal BLE-module ↔ STM32 UART protocol (`FUN_08016988`)

A second dispatcher handles frames between the STM32 and the BLE radio module over UART
(byte-by-byte polled send/receive in `FUN_080167f0` / `FUN_0801613e`). Opcodes include `0x5A`
(response wrapper), `0x5F` (read profile from charger memory — returns a 64-byte per-slot block),
`0x88` (enter firmware upgrade mode and reboot), plus `0x05`/`0x11`/`0x55`/`0xFE` mirrors of the
BLE-facing commands. This layer is not reachable from a BLE client; it exists between the two MCUs
on the charger's own PCB.

### Firmware upgrade entry (`opcode 0x88`, internal)

Shows "Upgrade Firmware… LCD May Not Display… Please Wait…" then reboots into the bootloader.
Not exposed over BLE GATT and not implemented in this server.
