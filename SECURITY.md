# Security Policy

This project controls physical battery-charging hardware. Treat every write/start action as potentially dangerous.

## Hardware safety

- Keep cells and charger in sight during testing.
- Never guess chemistry, capacity, cell count, or voltage limits.
- Refuse to charge damaged, hot, swollen, leaking, unwrapped, unknown, or non-rechargeable cells.
- Use `charger.validate_profile` before any profile write.
- Use `charger.apply_profile` in dry-run mode before any live profile write.
- `charger.apply_profile` defaults to dry-run and live writes require `confirmation_token=APPLY_PROFILE_SLOT_<slot>`.
- `charger.start` requires a separate `confirmation_token=START_SLOT_<slot>`.
- Emergency stop tools (`charger.stop_slot`, `charger.stop_all`) intentionally bypass profile validation.
- Verify the actual slot status after live writes/starts; ACKs alone are not proof of safe operation.

## Supported use

The current implementation is focused on SKYRC MC3000-compatible BLE devices. Unsupported firmware or unknown clones may reject writes or interpret profiles differently.

The server exposes app-compatible voltage curves from the charger. It does not collect telemetry or make hidden network calls.

## Reporting vulnerabilities

Please report safety/security issues privately before public disclosure. Include:

- device model and firmware/hardware version;
- MCP server version/commit;
- exact tool call and profile payload, with personal identifiers redacted;
- observed charger state before and after.

## Telemetry and network

The server performs no hidden network calls and has no telemetry. BLE discovery/control and local stdio MCP are the only default runtime channels. Session logs are in-memory and redact device addresses where possible.
