from __future__ import annotations

import pytest

from mc3000_mcp import safety


def test_bundled_profiles_validate() -> None:
    for profile in safety.profiles().values():
        result = safety.validate_profile(profile)
        assert result["ok"], result


def test_rejects_missing_and_unknown_chemistry() -> None:
    result = safety.validate_profile({"profile_id": "bad", "chemistry": "Mystery"})
    assert result["ok"] is False
    assert any("missing required" in error for error in result["errors"])
    assert any("unsupported chemistry" in error for error in result["errors"])
    assert safety.safety_limits()["NiMH"]["cell_nominal_mv"] == 1200


def test_rejects_bad_scalar_values_and_unknown_limit(monkeypatch: pytest.MonkeyPatch) -> None:
    profile = safety.profiles()["nimh-aa-conservative-charge"]
    profile["profile_id"] = "Bad ID"
    profile["capacity_mah"] = 0
    profile["nominal_voltage_mv"] = 7400
    profile["timeout_minutes"] = 2000
    profile["temperature_cutoff_c"] = "hot"
    profile["operation"] = "storage"
    result = safety.validate_profile(profile)
    assert result["ok"] is False
    assert any("profile_id" in error for error in result["errors"])
    assert any("capacity_mah" in error for error in result["errors"])
    assert any("nominal_voltage_mv" in error for error in result["errors"])
    assert any("timeout_minutes" in error for error in result["errors"])
    assert any("temperature_cutoff_c" in error for error in result["errors"])
    assert any("operation storage" in error for error in result["errors"])

    monkeypatch.setitem(safety.CHEMISTRY_CODES, "Ghost", 99)
    ghost = safety.profiles()["nimh-aa-conservative-charge"]
    ghost["chemistry"] = "Ghost"
    ghost_result = safety.validate_profile(ghost)
    assert any("no enforced safety limits" in error for error in ghost_result["errors"])


def test_rejects_suspicious_nimh_current_and_cutoff() -> None:
    profile = safety.profiles()["nimh-aa-conservative-charge"]
    profile["charge_current_ma"] = 5000
    profile["discharge_cutoff_mv"] = 500
    result = safety.validate_profile(profile)
    assert result["ok"] is False
    assert any("charge_current_ma" in error for error in result["errors"])
    assert any("discharge_cutoff_mv" in error for error in result["errors"])


def test_profile_to_mc3000_args_maps_slot_and_operation() -> None:
    profile = safety.profiles()["nimh-aa-refresh"]
    args = safety.profile_to_mc3000_args(profile, slot=3)
    assert args["slot_mask"] == 8
    assert args["battery_type"] == safety.CHEMISTRY_CODES["NiMH"]
    assert args["mode"] == safety.OPERATION_MODES["refresh"]


def test_unknown_lipo_multi_cell_is_rejected() -> None:
    profile = safety.profiles()["liion-1s-storage"]
    profile["cell_count"] = 2
    profile["nominal_voltage_mv"] = 7400
    result = safety.validate_profile(profile)
    assert result["ok"] is False
    assert any("cell_count" in error for error in result["errors"])


def test_invalid_profile_to_mc3000_args_raises() -> None:
    with pytest.raises(ValueError, match="invalid profile"):
        safety.profile_to_mc3000_args({"chemistry": "NiMH"}, slot=0)
