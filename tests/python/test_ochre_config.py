"""Tests for ochre_next.compat.ochre_config."""

from __future__ import annotations

import pytest

from ochre_next.compat import migrate_ochre_equipment_config, ochre_generator_config


class TestMigrateOchreEquipmentConfig:
    def test_generator_ramp_rate_converted_to_kw_per_s(self):
        ochre_params = {"ramp_rate": 60.0}  # 60 kW/min = 1 kW/s
        result = migrate_ochre_equipment_config(ochre_params, "Generator")
        assert result["delta_kw_per_s"] == 1.0

    def test_generator_ramp_rate_none_preserved(self):
        ochre_params = {"ramp_rate": None}
        result = migrate_ochre_equipment_config(ochre_params, "Generator")
        assert result["delta_kw_per_s"] is None

    def test_generator_capacity_mapped(self):
        ochre_params = {"capacity": 10.0}
        result = migrate_ochre_equipment_config(ochre_params, "Generator")
        assert result["rated_power_kw"] == 10.0

    def test_generator_capacity_min_mapped(self):
        ochre_params = {"capacity_min": 2.0}
        result = migrate_ochre_equipment_config(ochre_params, "Generator")
        assert result["capacity_min_kw"] == 2.0

    def test_generator_efficiency_mapped(self):
        ochre_params = {"efficiency": 0.35}
        result = migrate_ochre_equipment_config(ochre_params, "Generator")
        assert result["eta_electric"] == 0.35

    def test_generator_efficiency_chp_mapped_to_eta_thermal(self):
        ochre_params = {"efficiency_chp": 0.5}
        result = migrate_ochre_equipment_config(ochre_params, "Generator")
        assert result["eta_thermal"] == 0.5

    def test_generator_import_limit_mapped(self):
        ochre_params = {"import_limit": 2.0}
        result = migrate_ochre_equipment_config(ochre_params, "Generator")
        assert result["grid_import_limit_kw"] == 2.0

    def test_generator_export_limit_mapped(self):
        ochre_params = {"export_limit": 1.0}
        result = migrate_ochre_equipment_config(ochre_params, "Generator")
        assert result["export_limit_kw"] == 1.0

    def test_unknown_keys_passed_through(self):
        ochre_params = {"custom_key": "custom_value", "other": 123}
        result = migrate_ochre_equipment_config(ochre_params, "Generator")
        assert result["custom_key"] == "custom_value"
        assert result["other"] == 123


class TestOchreGeneratorConfig:
    def test_basic_generates_correct_config(self):
        result = ochre_generator_config(capacity=10.0)
        assert result["rated_power_kw"] == 10.0
        assert result["eta_electric"] == 0.30
        assert result["delta_kw_per_s"] == 0.1 / 60.0  # OCHRE default 0.1 kW/min

    def test_custom_ramp_rate_converted(self):
        result = ochre_generator_config(capacity=10.0, ramp_rate=6.0)
        assert result["delta_kw_per_s"] == 0.1  # 6/60 = 0.1

    def test_none_ramp_rate_preserved(self):
        result = ochre_generator_config(capacity=10.0, ramp_rate=None)
        assert result["delta_kw_per_s"] is None

    def test_capacity_min_included(self):
        result = ochre_generator_config(capacity=10.0, capacity_min=2.0)
        assert result["capacity_min_kw"] == 2.0

    def test_import_limit_included(self):
        result = ochre_generator_config(capacity=10.0, import_limit=5.0)
        assert result["grid_import_limit_kw"] == 5.0

    def test_export_limit_included(self):
        result = ochre_generator_config(capacity=10.0, export_limit=3.0)
        assert result["export_limit_kw"] == 3.0

    def test_additional_kwargs_passed(self):
        result = ochre_generator_config(capacity=10.0, custom_field="value")
        assert result["custom_field"] == "value"

    def test_octre_1kw_per_min_converts_correctly(self):
        result = ochre_generator_config(capacity=10.0, ramp_rate=1.0)
        expected_delta = 1.0 / 60.0
        assert abs(result["delta_kw_per_s"] - expected_delta) < 1e-10


class TestRoundTripVerification:
    def test_10kw_generator_1kw_per_min_full_load_time(self):
        config = ochre_generator_config(capacity=10.0, ramp_rate=1.0)
        delta_kw_per_s = config["delta_kw_per_s"]
        seconds_to_full = 10.0 / delta_kw_per_s
        assert seconds_to_full == 600.0  # 10 minutes = OCHRE behavior
