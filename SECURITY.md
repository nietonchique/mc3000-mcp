# Security and safety

This project can control a physical battery charger. Treat every write operation as safety-critical.

Safety rules:

- Verify battery chemistry, slot number, current, voltage, and temperature limits before applying a profile.
- Slot numbers in the API are zero-based (`0..3`), matching the official Android app internals.
- Do not expose this MCP server to untrusted clients or networks.
- Keep the charger in sight when testing new profiles.
- Factory reset and calibration reset opcodes are documented but not exposed as first-class MCP tools.

If you find a protocol bug that could cause unsafe charging behavior, open a private security advisory or contact the maintainer before public disclosure.
