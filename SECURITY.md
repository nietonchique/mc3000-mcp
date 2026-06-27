# Security Policy

This project controls physical battery-charging hardware. Treat every write/start action as potentially dangerous.

## Hardware safety

- Keep cells and charger in sight during testing.
- Never guess chemistry, capacity, cell count, or voltage limits.
- Use `charger.validate_profile` before any profile write.
- `charger.apply_profile` defaults to dry-run and live writes require `APPLY_PROFILE_SLOT_<slot>`.
- `charger.start` requires `START_SLOT_<slot>`.
- Emergency stop tools (`charger.stop_slot`, `charger.stop_all`) intentionally bypass profile validation.

## Supported use

The current implementation is focused on SKYRC MC3000-compatible BLE devices. Unsupported firmware or unknown clones may reject writes or interpret profiles differently.

## Reporting vulnerabilities

Please report safety/security issues privately before public disclosure. Include:

- device model and firmware/hardware version;
- MCP server version/commit;
- exact tool call and profile payload, with personal identifiers redacted;
- observed charger state before and after.

## Telemetry and network

The server performs no hidden network calls and has no telemetry. BLE discovery and local stdio MCP are the only default runtime channels.
