"""Tests for SimulationConfig and DwellingConfig Python bindings."""

import pytest

from ochre_next import SimulationConfig, DwellingConfig


class TestSimulationConfig:
    """Test SimulationConfig Python type."""

    def test_default_construction_yields_expected_defaults(self):
        """Verify SimulationConfig() default construction yields expected default values."""
        cfg = SimulationConfig()
        assert cfg.start_time == "2019-01-01T00:00:00+00:00"
        assert cfg.duration_s == 86400
        assert cfg.time_res_s == 60
        assert cfg.output_verbosity == 0
        assert cfg.output_path is None
        assert cfg.output_to_parquet is False
        assert cfg.output_chunk_size == 10000
        assert cfg.master_seed == 0
        assert cfg.civil_timezone is None
        assert cfg.setpoint_deadband_c is None

    def test_construction_with_kwargs(self):
        """Verify SimulationConfig accepts kwargs in constructor."""
        cfg = SimulationConfig(duration_s=3600, time_res_s=60, master_seed=42)
        assert cfg.duration_s == 3600
        assert cfg.time_res_s == 60
        assert cfg.master_seed == 42
        assert cfg.start_time == "2019-01-01T00:00:00+00:00"
        assert cfg.output_verbosity == 0

    def test_custom_values_accepted_and_round_trip(self):
        """Verify custom values are accepted and round-trip through getters."""
        cfg = SimulationConfig()
        cfg.start_time = "2024-06-15T12:00:00Z"
        cfg.duration_s = 7200
        cfg.time_res_s = 300
        cfg.output_verbosity = 5
        cfg.output_path = "/tmp/output.csv"
        cfg.output_to_parquet = True
        cfg.output_chunk_size = 5000
        cfg.master_seed = 42
        cfg.civil_timezone = "America/Denver"
        cfg.setpoint_deadband_c = 1.5

        assert cfg.start_time == "2024-06-15T12:00:00+00:00"
        assert cfg.duration_s == 7200
        assert cfg.time_res_s == 300
        assert cfg.output_verbosity == 5
        assert cfg.output_path == "/tmp/output.csv"
        assert cfg.output_to_parquet is True
        assert cfg.output_chunk_size == 5000
        assert cfg.master_seed == 42
        assert cfg.civil_timezone == "America/Denver"
        assert cfg.setpoint_deadband_c == 1.5

    def test_repr_includes_key_info(self):
        """Verify __repr__ includes key info."""
        cfg = SimulationConfig()
        cfg.master_seed = 42
        r = repr(cfg)
        assert "SimulationConfig" in r
        assert "2019-01-01" in r
        assert "86400" in r
        assert "master_seed=42" in r

    def test_invalid_start_time_string_raises_value_error(self):
        """Verify invalid start_time string raises ValueError."""
        cfg = SimulationConfig()
        with pytest.raises(ValueError, match="invalid start_time format"):
            cfg.start_time = "not-a-valid-date"

    def test_invalid_duration_raises_value_error(self):
        """Verify invalid duration_s raises ValueError."""
        cfg = SimulationConfig()
        with pytest.raises(ValueError, match="duration_s must be positive"):
            cfg.duration_s = 0

    def test_invalid_time_res_raises_value_error(self):
        """Verify invalid time_res_s raises ValueError."""
        cfg = SimulationConfig()
        with pytest.raises(ValueError, match="time_res_s must be positive"):
            cfg.time_res_s = 0

    def test_duration_not_divisible_by_time_res_raises_in_conversion(self):
        """Verify duration_s not divisible by time_res_s raises when converting to Rust config."""
        cfg = SimulationConfig()
        cfg.duration_s = 100
        cfg.time_res_s = 60
        # Validation happens in to_sim_config(), not in setters.
        # This test verifies it raises when you try to use the config.
        with pytest.raises(ValueError, match="divisible by time_res_s"):
            cfg2 = SimulationConfig(duration_s=100, time_res_s=60)

    def test_invalid_output_verbosity_raises_value_error(self):
        """Verify output_verbosity > 8 raises ValueError."""
        cfg = SimulationConfig()
        with pytest.raises(ValueError, match="output_verbosity must be 0-8"):
            cfg.output_verbosity = 9

    def test_invalid_setpoint_deadband_raises_value_error(self):
        """Verify invalid setpoint_deadband_c raises ValueError."""
        cfg = SimulationConfig()
        with pytest.raises(ValueError, match="setpoint_deadband_c must be finite"):
            cfg.setpoint_deadband_c = -1.0


class TestDwellingConfig:
    """Test DwellingConfig Python type."""

    def test_minimal_construction_works(self):
        """Verify DwellingConfig(hpxml=..., schedule=..., weather=...) minimal construction."""
        cfg = DwellingConfig(
            hpxml="path/to/building.xml",
            schedule="path/to/schedules.csv",
            weather="path/to/weather.epw",
        )
        assert cfg.hpxml == "path/to/building.xml"
        assert cfg.schedule == "path/to/schedules.csv"
        assert cfg.weather == "path/to/weather.epw"
        assert cfg.bldg_id == 0
        assert cfg.config is None
        assert cfg.defaults_path is None

    def test_initialization_duration_round_trips(self):
        """Verify DwellingConfig with initialization_duration=3600 round-trips."""
        cfg = DwellingConfig(
            hpxml="path/to/building.xml",
            schedule="path/to/schedules.csv",
            weather="path/to/weather.epw",
            initialization_duration=3600,
        )
        assert cfg.initialization_duration == 3600

    def test_resample_overrides_round_trips(self):
        """Verify DwellingConfig with resample_overrides round-trips."""
        cfg = DwellingConfig(
            hpxml="path/to/building.xml",
            schedule="path/to/schedules.csv",
            weather="path/to/weather.epw",
            resample_overrides={"dry_bulb": "zoh"},
        )
        assert cfg.resample_overrides is not None
        assert cfg.resample_overrides.get("dry_bulb") == "zoh"

    def test_resample_overrides_typo_raises_value_error(self):
        """Regression guard: typo in resample method must raise ValueError,
        not silently fall back to a default."""
        with pytest.raises(ValueError, match="tringular"):
            DwellingConfig(
                hpxml="path/to/building.xml",
                schedule="path/to/schedules.csv",
                weather="path/to/weather.epw",
                resample_overrides={"dry_bulb": "tringular"},
            )

    def test_resample_overrides_all_valid_methods_accepted(self):
        """Regression guard: each valid method name must be accepted without error."""
        valid_methods = ["zoh", "pchip", "pchip_cyclic", "linear", "circular_linear", "triangular"]
        for method in valid_methods:
            cfg = DwellingConfig(
                hpxml="path/to/building.xml",
                schedule="path/to/schedules.csv",
                weather="path/to/weather.epw",
                resample_overrides={"dry_bulb": method},
            )
            assert cfg.resample_overrides is not None
            assert cfg.resample_overrides.get("dry_bulb") == method

    def test_with_simulation_config_round_trips(self):
        """Verify DwellingConfig with config parameter round-trips."""
        sim_cfg = SimulationConfig()
        sim_cfg.duration_s = 7200

        dw_cfg = DwellingConfig(
            hpxml="path/to/building.xml",
            schedule="path/to/schedules.csv",
            weather="path/to/weather.epw",
            config=sim_cfg,
        )

        assert dw_cfg.config is not None
        assert dw_cfg.config.duration_s == 7200

    def test_missing_required_fields_raises_type_error(self):
        """Verify DwellingConfig with missing required fields raises TypeError."""
        with pytest.raises(TypeError):
            DwellingConfig(hpxml="path/to/building.xml")

    def test_overrides_round_trips_as_dict(self):
        """Verify DwellingConfig with overrides round-trips through getter as dict."""
        overrides = {"hvac": {"capacity_kw": 5.0}}
        cfg = DwellingConfig(
            hpxml="path/to/building.xml",
            schedule="path/to/schedules.csv",
            weather="path/to/weather.epw",
            overrides=overrides,
        )
        result = cfg.overrides
        assert result is not None
        assert "hvac" in result
        assert result["hvac"]["capacity_kw"] == 5.0


class TestDwellingConfigIntegration:
    """Integration tests for DwellingConfig with SimulationConfig."""

    def test_simulation_config_can_be_constructed_for_from_hpxml(self):
        """Verify SimulationConfig can be constructed for use with from_hpxml."""
        # Note: This test would require actual HPXML/weather files to run from_hpxml.
        # We just verify the config object can be created and manipulated.
        config = SimulationConfig()
        config.duration_s = 3600
        config.time_res_s = 60
        config.master_seed = 42

        # The config object can be used as kwarg
        assert config.duration_s == 3600
        assert config.master_seed == 42
