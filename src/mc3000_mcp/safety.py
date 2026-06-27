from __future__ import annotations

import re
from copy import deepcopy
from typing import Any, Literal, TypedDict

Chemistry = Literal["NiMH", "NiCd", "Eneloop", "LiIon", "LiFe", "LiIo4.35", "NiZn", "RAM", "LTO"]
Operation = Literal["charge", "refresh", "break_in", "discharge", "cycle", "storage"]
RiskLevel = Literal["low", "medium", "high"]
TIMEOUT_MINUTES_MAX = 1440

REQUIRED_PROFILE_FIELDS = {
    "profile_id",
    "chemistry",
    "capacity_mah",
    "cell_count",
    "operation",
    "charge_current_ma",
    "discharge_current_ma",
    "discharge_cutoff_mv",
    "temperature_cutoff_c",
    "source",
    "risk_level",
}

CHEMISTRY_CODES: dict[str, int] = {
    "LiIon": 0,
    "LiFe": 1,
    "LiIo4.35": 2,
    "NiMH": 3,
    "NiCd": 4,
    "NiZn": 5,
    "Eneloop": 6,
    "RAM": 7,
    "LTO": 8,
}

OPERATION_MODES: dict[str, int] = {
    "charge": 0,
    "refresh": 1,
    "break_in": 2,
    "storage": 2,
    "discharge": 3,
    "cycle": 4,
}

SAFETY_LIMITS: dict[str, dict[str, Any]] = {
    "NiMH": {
        "cell_nominal_mv": 1200,
        "cell_count_min": 1,
        "cell_count_max": 1,
        "charge_current_ma_min": 50,
        "charge_current_c_max": 1.0,
        "charge_voltage_mv_min": 1400,
        "charge_voltage_mv_max": 1800,
        "discharge_current_ma_min": 50,
        "discharge_current_ma_max": 2000,
        "discharge_cutoff_mv_min": 900,
        "discharge_cutoff_mv_max": 1100,
        "temperature_cutoff_c_min": 20,
        "temperature_cutoff_c_max": 60,
        "operations": ["charge", "refresh", "break_in", "discharge", "cycle"],
    },
    "Eneloop": {
        "cell_nominal_mv": 1200,
        "cell_count_min": 1,
        "cell_count_max": 1,
        "charge_current_ma_min": 50,
        "charge_current_c_max": 1.0,
        "charge_voltage_mv_min": 1400,
        "charge_voltage_mv_max": 1800,
        "discharge_current_ma_min": 50,
        "discharge_current_ma_max": 2000,
        "discharge_cutoff_mv_min": 900,
        "discharge_cutoff_mv_max": 1100,
        "temperature_cutoff_c_min": 20,
        "temperature_cutoff_c_max": 60,
        "operations": ["charge", "refresh", "break_in", "discharge", "cycle"],
    },
    "NiCd": {
        "cell_nominal_mv": 1200,
        "cell_count_min": 1,
        "cell_count_max": 1,
        "charge_current_ma_min": 50,
        "charge_current_c_max": 1.0,
        "charge_voltage_mv_min": 1400,
        "charge_voltage_mv_max": 1800,
        "discharge_current_ma_min": 50,
        "discharge_current_ma_max": 2000,
        "discharge_cutoff_mv_min": 900,
        "discharge_cutoff_mv_max": 1100,
        "temperature_cutoff_c_min": 20,
        "temperature_cutoff_c_max": 60,
        "operations": ["charge", "refresh", "break_in", "discharge", "cycle"],
    },
    "LiIon": {
        "cell_nominal_mv": 3700,
        "cell_count_min": 1,
        "cell_count_max": 1,
        "charge_current_ma_min": 50,
        "charge_current_c_max": 1.0,
        "charge_voltage_mv_min": 4100,
        "charge_voltage_mv_max": 4200,
        "discharge_current_ma_min": 50,
        "discharge_current_ma_max": 2000,
        "discharge_cutoff_mv_min": 2800,
        "discharge_cutoff_mv_max": 3300,
        "storage_voltage_mv_min": 3700,
        "storage_voltage_mv_max": 3900,
        "temperature_cutoff_c_min": 20,
        "temperature_cutoff_c_max": 55,
        "operations": ["charge", "storage", "discharge"],
    },
    "LiFe": {
        "cell_nominal_mv": 3200,
        "cell_count_min": 1,
        "cell_count_max": 1,
        "charge_current_ma_min": 50,
        "charge_current_c_max": 1.0,
        "charge_voltage_mv_min": 3500,
        "charge_voltage_mv_max": 3600,
        "discharge_current_ma_min": 50,
        "discharge_current_ma_max": 2000,
        "discharge_cutoff_mv_min": 2400,
        "discharge_cutoff_mv_max": 2900,
        "storage_voltage_mv_min": 3200,
        "storage_voltage_mv_max": 3400,
        "temperature_cutoff_c_min": 20,
        "temperature_cutoff_c_max": 55,
        "operations": ["charge", "storage", "discharge"],
    },
}

PROFILE_SCHEMA: dict[str, Any] = {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "$id": "https://github.com/nietonchique/mc3000-mcp/schema/charger-profile.json",
    "title": "Charger Battery Profile",
    "type": "object",
    "additionalProperties": False,
    "required": sorted(REQUIRED_PROFILE_FIELDS),
    "properties": {
        "profile_id": {"type": "string", "pattern": "^[a-z0-9][a-z0-9_.-]{2,80}$"},
        "chemistry": {"type": "string", "enum": sorted(CHEMISTRY_CODES)},
        "capacity_mah": {"type": "integer", "minimum": 1, "maximum": 50000},
        "cell_count": {"type": "integer", "minimum": 1, "maximum": 12},
        "nominal_voltage_mv": {"type": "integer", "minimum": 500, "maximum": 60000},
        "operation": {"type": "string", "enum": sorted(OPERATION_MODES)},
        "charge_current_ma": {"type": "integer", "minimum": 0, "maximum": 10000},
        "discharge_current_ma": {"type": "integer", "minimum": 0, "maximum": 10000},
        "charge_voltage_mv": {"type": "integer", "minimum": 0, "maximum": 60000},
        "discharge_cutoff_mv": {"type": "integer", "minimum": 0, "maximum": 60000},
        "timeout_minutes": {"type": "integer", "minimum": 0, "maximum": 1440},
        "temperature_cutoff_c": {"type": "integer", "minimum": 0, "maximum": 80},
        "temperature_policy": {"type": "string", "minLength": 3},
        "notes": {"type": "string"},
        "source": {"type": "string", "minLength": 3},
        "risk_level": {"type": "string", "enum": ["low", "medium", "high"]},
        "cycle_count": {"type": "integer", "minimum": 1, "maximum": 99},
        "cycle_mode": {"type": "string", "enum": ["C>D", "C>D>C", "D>C", "D>C>D"]},
        "negative_delta_mv": {"type": "integer", "minimum": 0, "maximum": 20},
        "trickle_current_ma": {"type": "integer", "minimum": 0, "maximum": 300},
    },
}

EXAMPLE_PROFILES: dict[str, dict[str, Any]] = {
    "nimh-aa-conservative-charge": {
        "profile_id": "nimh-aa-conservative-charge",
        "chemistry": "NiMH",
        "capacity_mah": 2600,
        "cell_count": 1,
        "nominal_voltage_mv": 1200,
        "operation": "charge",
        "charge_current_ma": 260,
        "discharge_current_ma": 200,
        "charge_voltage_mv": 1650,
        "discharge_cutoff_mv": 1000,
        "timeout_minutes": 960,
        "temperature_cutoff_c": 45,
        "temperature_policy": "stop at cutoff; keep charger supervised",
        "negative_delta_mv": 3,
        "trickle_current_ma": 0,
        "source": "GP 2700 AA label + SKYRC MC3000 manual safe defaults",
        "risk_level": "low",
        "notes": "Slow conservative NiMH AA charge; user must confirm chemistry and capacity.",
    },
    "nimh-aa-refresh": {
        "profile_id": "nimh-aa-refresh",
        "chemistry": "NiMH",
        "capacity_mah": 2600,
        "cell_count": 1,
        "nominal_voltage_mv": 1200,
        "operation": "refresh",
        "charge_current_ma": 260,
        "discharge_current_ma": 200,
        "charge_voltage_mv": 1650,
        "discharge_cutoff_mv": 1000,
        "timeout_minutes": 1440,
        "temperature_cutoff_c": 45,
        "temperature_policy": "stop at cutoff; monitor first cycle",
        "negative_delta_mv": 3,
        "trickle_current_ma": 0,
        "source": "SKYRC MC3000 manual; live-tested BLE semantics",
        "risk_level": "medium",
        "notes": "Refresh measures capacity; do not leave unknown cells unattended.",
    },
    "nimh-aa-break-in": {
        "profile_id": "nimh-aa-break-in",
        "chemistry": "NiMH",
        "capacity_mah": 2600,
        "cell_count": 1,
        "nominal_voltage_mv": 1200,
        "operation": "break_in",
        "charge_current_ma": 260,
        "discharge_current_ma": 520,
        "charge_voltage_mv": 1650,
        "discharge_cutoff_mv": 1000,
        "timeout_minutes": 0,
        "temperature_cutoff_c": 45,
        "temperature_policy": "stop at cutoff; supervise because break-in is long",
        "cycle_mode": "D>C>D",
        "negative_delta_mv": 3,
        "trickle_current_ma": 0,
        "source": "SKYRC MC3000 manual; C/10 charge and C/5 discharge convention",
        "risk_level": "medium",
        "notes": "For long-stored/deactivated NiMH only; regular cycle is usually gentler.",
    },
    "liion-1s-storage": {
        "profile_id": "liion-1s-storage",
        "chemistry": "LiIon",
        "capacity_mah": 2000,
        "cell_count": 1,
        "nominal_voltage_mv": 3700,
        "operation": "storage",
        "charge_current_ma": 500,
        "discharge_current_ma": 500,
        "charge_voltage_mv": 3800,
        "discharge_cutoff_mv": 3000,
        "timeout_minutes": 240,
        "temperature_cutoff_c": 45,
        "temperature_policy": "stop at cutoff; use fire-safe area",
        "source": "Conservative generic 1S Li-ion storage profile",
        "risk_level": "high",
        "notes": (
            "MC3000 is single-cell-per-slot; do not use for multi-cell packs "
            "without per-cell support."
        ),
    },
}


class ValidationResult(TypedDict):
    ok: bool
    errors: list[str]
    warnings: list[str]
    normalized_profile: dict[str, Any]


def profile_schema() -> dict[str, Any]:
    return deepcopy(PROFILE_SCHEMA)


def profiles() -> dict[str, dict[str, Any]]:
    return deepcopy(EXAMPLE_PROFILES)


def safety_limits() -> dict[str, dict[str, Any]]:
    return deepcopy(SAFETY_LIMITS)


def validate_profile(profile: dict[str, Any]) -> ValidationResult:  # noqa: C901, PLR0912, PLR0915
    normalized = dict(profile)
    errors: list[str] = []
    warnings: list[str] = []

    missing = sorted(REQUIRED_PROFILE_FIELDS - set(normalized))
    if missing:
        errors.append(f"missing required fields: {', '.join(missing)}")

    profile_id = str(normalized.get("profile_id", ""))
    if profile_id and re.fullmatch(r"[a-z0-9][a-z0-9_.-]{2,80}", profile_id) is None:
        errors.append("profile_id must match ^[a-z0-9][a-z0-9_.-]{2,80}$")

    chemistry = str(normalized.get("chemistry", ""))
    if chemistry not in CHEMISTRY_CODES:
        errors.append(f"unsupported chemistry: {chemistry or '<missing>'}")
        return _validation_result(ok=False, errors=errors, warnings=warnings, normalized=normalized)

    limits = SAFETY_LIMITS.get(chemistry)
    if limits is None:
        errors.append(f"no enforced safety limits for chemistry: {chemistry}")
        return _validation_result(ok=False, errors=errors, warnings=warnings, normalized=normalized)

    operation = str(normalized.get("operation", ""))
    if operation not in limits["operations"]:
        errors.append(f"operation {operation or '<missing>'} is not supported for {chemistry}")

    capacity = _as_int(normalized, "capacity_mah", errors)
    cell_count = _as_int(normalized, "cell_count", errors)
    nominal_mv = _as_int(normalized, "nominal_voltage_mv", errors, required=False)
    charge_current = _as_int(normalized, "charge_current_ma", errors)
    discharge_current = _as_int(normalized, "discharge_current_ma", errors)
    charge_voltage = _as_int(normalized, "charge_voltage_mv", errors, required=False)
    discharge_cutoff = _as_int(normalized, "discharge_cutoff_mv", errors)
    temp_cutoff = _as_int(normalized, "temperature_cutoff_c", errors)
    timeout = _as_int(normalized, "timeout_minutes", errors, required=False)

    if capacity is not None and capacity <= 0:
        errors.append("capacity_mah must be positive")
    if cell_count is not None:
        _check_range(
            "cell_count",
            cell_count,
            limits["cell_count_min"],
            limits["cell_count_max"],
            errors,
        )
    if nominal_mv is not None:
        expected = int(limits["cell_nominal_mv"]) * int(cell_count or 1)
        if abs(nominal_mv - expected) > max(150, expected // 10):
            errors.append(
                f"nominal_voltage_mv {nominal_mv} does not match {chemistry} x {cell_count}",
            )
    if charge_current is not None and capacity is not None:
        max_charge = int(float(limits["charge_current_c_max"]) * capacity)
        _check_range(
            "charge_current_ma",
            charge_current,
            limits["charge_current_ma_min"],
            max_charge,
            errors,
        )
    if discharge_current is not None:
        _check_range(
            "discharge_current_ma",
            discharge_current,
            limits["discharge_current_ma_min"],
            limits["discharge_current_ma_max"],
            errors,
        )
    if (
        charge_voltage is not None
        and operation in limits["operations"]
        and operation in {"charge", "refresh", "cycle", "storage"}
    ):
        key_min = "storage_voltage_mv_min" if operation == "storage" else "charge_voltage_mv_min"
        key_max = "storage_voltage_mv_max" if operation == "storage" else "charge_voltage_mv_max"
        _check_range("charge_voltage_mv", charge_voltage, limits[key_min], limits[key_max], errors)
    if discharge_cutoff is not None:
        _check_range(
            "discharge_cutoff_mv",
            discharge_cutoff,
            limits["discharge_cutoff_mv_min"],
            limits["discharge_cutoff_mv_max"],
            errors,
        )
    if temp_cutoff is not None:
        _check_range(
            "temperature_cutoff_c",
            temp_cutoff,
            limits["temperature_cutoff_c_min"],
            limits["temperature_cutoff_c_max"],
            errors,
        )
    if timeout is not None and not 0 <= timeout <= TIMEOUT_MINUTES_MAX:
        errors.append("timeout_minutes must be 0..1440")
    if str(normalized.get("risk_level", "")) == "high":
        warnings.append("high-risk profile: require extra human review and supervision")

    return _validation_result(
        ok=not errors,
        errors=errors,
        warnings=warnings,
        normalized=normalized,
    )


def _validation_result(
    *,
    ok: bool,
    errors: list[str],
    warnings: list[str],
    normalized: dict[str, Any],
) -> ValidationResult:
    return {"ok": ok, "errors": errors, "warnings": warnings, "normalized_profile": normalized}


def profile_to_mc3000_args(profile: dict[str, Any], *, slot: int) -> dict[str, Any]:
    validation = validate_profile(profile)
    if not validation["ok"]:
        raise ValueError("invalid profile: " + "; ".join(validation["errors"]))
    normalized = validation["normalized_profile"]
    operation = str(normalized["operation"])
    return {
        "slot_mask": 1 << slot,
        "battery_type": CHEMISTRY_CODES[str(normalized["chemistry"])],
        "mode": OPERATION_MODES[operation],
        "capacity_mah": int(normalized["capacity_mah"]),
        "charge_current_ma": int(normalized["charge_current_ma"]),
        "discharge_current_ma": int(normalized["discharge_current_ma"]),
        "charge_stop_voltage_mv": int(normalized.get("charge_voltage_mv", 0) or 0) or None,
        "discharge_stop_voltage_mv": int(normalized["discharge_cutoff_mv"]),
        "discharge_stop_current_ma": int(normalized["discharge_current_ma"]),
        "temp_cutoff": int(normalized["temperature_cutoff_c"]),
        "time_limit_minutes": int(normalized.get("timeout_minutes", 0) or 0),
        "cycle_count": int(normalized.get("cycle_count", 1) or 1),
        "cycle_mode": normalized.get("cycle_mode", "D>C"),
        "negative_delta_mv": int(normalized.get("negative_delta_mv", 3) or 0),
        "trickle_current_ma": int(normalized.get("trickle_current_ma", 0) or 0),
        "breakin": operation == "break_in",
    }


def _as_int(
    profile: dict[str, Any],
    key: str,
    errors: list[str],
    *,
    required: bool = True,
) -> int | None:
    value = profile.get(key)
    if value is None:
        if required:
            errors.append(f"{key} is required")
        return None
    try:
        return int(value)
    except (TypeError, ValueError):
        errors.append(f"{key} must be an integer")
        return None


def _check_range(
    name: str,
    value: int,
    minimum: float,
    maximum: float,
    errors: list[str],
) -> None:
    if not int(minimum) <= value <= int(maximum):
        errors.append(f"{name}={value} outside enforced range {int(minimum)}..{int(maximum)}")
