"""Tests for SimulationConfig, SiteLocationOverride and DwellingConfig bindings.

Row 34: one config type across TOML, synthetic TOML and Python. The
constructor's keywords are the Rust field names, its defaults the Rust serde
defaults, and the validation the TOML path runs runs here too.
"""

import inspect
import re
import tomllib
from typing import Any, cast

import pytest

from conftest import REMOVED_SITE_KEYWORD_MESSAGES
from hares import (
    Destination,
    Dwelling,
    DwellingConfig,
    Frequency,
    ReportSpec,
    Selection,
    SimulationConfig,
    SiteLocationOverride,
)

START = "2024-01-01T00:00:00Z"


def required_kwargs() -> dict[str, Any]:
    return {"start_time": START, "duration": 3600}


class TestSimulationConfig:
    """Test SimulationConfig Python type."""

    def test_python_and_toml_defaults_come_from_one_table(self):
        """A config built in Python with only the required arguments and a
        config parsed from the same two required keys resolve field for
        field equal: the defaults come from the one serde table."""
        toml_text = f'start_time = "{START}"\nduration = 3600\n'
        toml_cfg = SimulationConfig.from_toml(toml_text)
        py_cfg = SimulationConfig(**required_kwargs())
        assert py_cfg.start_time == toml_cfg.start_time
        assert py_cfg.duration == toml_cfg.duration
        assert py_cfg.time_res == toml_cfg.time_res
        assert py_cfg.reports == toml_cfg.reports
        assert py_cfg.output_chunk_size == toml_cfg.output_chunk_size
        assert py_cfg.setpoint_deadband_c == toml_cfg.setpoint_deadband_c
        assert py_cfg.master_seed == toml_cfg.master_seed
        assert py_cfg.civil_timezone == toml_cfg.civil_timezone
        assert py_cfg.max_consecutive_step_failures == toml_cfg.max_consecutive_step_failures
        assert py_cfg.site_location.latitude_deg == toml_cfg.site_location.latitude_deg
        assert py_cfg.site_location.longitude_deg == toml_cfg.site_location.longitude_deg
        assert py_cfg.site_location.elevation_m == toml_cfg.site_location.elevation_m
        assert py_cfg.site_location.utc_offset_h == toml_cfg.site_location.utc_offset_h
        # And the Python-built config serialises to exactly what from_toml
        # parses back to the same config.
        assert SimulationConfig.from_toml(py_cfg.to_toml()) == py_cfg or (
            py_cfg.to_toml() == toml_cfg.to_toml()
        )

    def test_construction_with_kwargs(self):
        cfg = SimulationConfig(start_time=START, duration=3600, time_res=60, master_seed=42)
        assert cfg.duration == 3600
        assert cfg.time_res == 60
        assert cfg.master_seed == 42
        assert cfg.start_time == "2024-01-01T00:00:00+00:00"
        assert cfg.reports == []

    def test_duration_and_time_res_accept_timedelta(self):
        from datetime import timedelta

        cfg = SimulationConfig(
            start_time=START, duration=timedelta(hours=1), time_res=timedelta(minutes=1)
        )
        assert cfg.duration == 3600
        assert cfg.time_res == 60
        whole = timedelta(hours=1, milliseconds=500)
        with pytest.raises(ValueError, match="whole number of seconds"):
            SimulationConfig(start_time=START, duration=whole)

    def test_custom_values_accepted_and_round_trip(self):
        reports = [
            ReportSpec("frame", Selection.verbosity(5), Destination.memory()),
            ReportSpec(
                "archive",
                Selection.named(["Total Electric Power (kW)"]),
                Destination.parquet("/tmp/output.parquet", overwrite=True),
            ),
        ]
        cfg = SimulationConfig(
            start_time="2024-06-15T12:00:00Z",
            duration=7200,
            time_res=300,
            reports=reports,
            output_chunk_size=5000,
            master_seed=42,
            civil_timezone="America/Denver",
            setpoint_deadband_c=1.5,
        )
        assert cfg.start_time == "2024-06-15T12:00:00+00:00"
        assert cfg.duration == 7200
        assert cfg.time_res == 300
        assert len(cfg.reports) == 2
        assert cfg.reports[0].name == "frame"
        assert repr(cfg.reports[0].destination) == "Destination.memory()"
        assert repr(cfg.reports[1].destination) == (
            "Destination.parquet(/tmp/output.parquet, overwrite=true)"
        )
        assert cfg.output_chunk_size == 5000
        assert cfg.master_seed == 42
        assert cfg.civil_timezone == "America/Denver"
        assert cfg.setpoint_deadband_c == 1.5

    def test_repr_includes_key_info(self):
        cfg = SimulationConfig(start_time=START, duration=3600, master_seed=42)
        r = repr(cfg)
        assert "SimulationConfig" in r
        assert "2024-01-01" in r
        assert "3600" in r
        assert "master_seed=42" in r

    def test_invalid_start_time_string_raises_value_error(self):
        with pytest.raises(ValueError, match="invalid start_time format"):
            SimulationConfig(start_time="not-a-valid-date", duration=3600)

    def test_invalid_duration_raises_value_error(self):
        with pytest.raises(ValueError, match="duration must be positive"):
            SimulationConfig(start_time=START, duration=0)

    def test_invalid_time_res_raises_value_error(self):
        with pytest.raises(ValueError, match="time_res must be positive"):
            SimulationConfig(start_time=START, duration=3600, time_res=0)

    def test_duration_not_divisible_by_time_res_raises_value_error(self):
        with pytest.raises(ValueError, match="divisible"):
            SimulationConfig(start_time=START, duration=100, time_res=60)

    def test_invalid_report_verbosity_raises_value_error(self):
        with pytest.raises(ValueError, match="verbosity must be 0-8"):
            SimulationConfig(
                start_time=START,
                duration=3600,
                reports=[ReportSpec("frame", Selection.verbosity(9), Destination.memory())],
            )

    def test_invalid_setpoint_deadband_raises_value_error(self):
        with pytest.raises(ValueError, match="setpoint_deadband_c must be finite"):
            SimulationConfig(start_time=START, duration=3600, setpoint_deadband_c=-1.0)

    def test_step_failure_budget_is_configurable(self):
        cfg = SimulationConfig(start_time=START, duration=3600, max_consecutive_step_failures=0)
        assert cfg.max_consecutive_step_failures == 0
        cfg.max_consecutive_step_failures = 5
        assert cfg.max_consecutive_step_failures == 5

    def test_duration_above_chrono_bound_raises_value_error(self):
        # i64::MAX / 1_000 ~ 9.22e15; 10**18 exceeds it.
        with pytest.raises(ValueError, match="too large for chrono Duration"):
            SimulationConfig(start_time=START, duration=10**18, time_res=60)

    def test_time_res_above_chrono_bound_raises_value_error(self):
        with pytest.raises(ValueError, match="too large for chrono Duration"):
            SimulationConfig(start_time=START, duration=3600, time_res=10**18)

    def test_removed_keyword_names_replacement(self):
        """The Python-only removed keywords name their replacement: the four
        site keywords raise the removed-keys table's full message (the one
        shared constant, the same string the Rust pins assert), and
        `output_to_parquet` names `SimulationConfig.reports`."""
        for key, message in REMOVED_SITE_KEYWORD_MESSAGES.items():
            kwargs = required_kwargs() | {key: 1.0}
            with pytest.raises(TypeError, match=re.escape(message)):
                # The constructor rejects the key before any value
                # extraction, so the value's type never matters.
                SimulationConfig(**cast("dict[str, Any]", kwargs))

        for key, replacement in [("output_to_parquet", "reports")]:
            kwargs = required_kwargs() | {key: 1.0}
            with pytest.raises(TypeError, match=f"'{key}'.*{replacement}|{replacement}.*'{key}'"):
                SimulationConfig(**cast("dict[str, Any]", kwargs))

    def test_removed_simulation_keyword_names_replacement(self):
        """The removed [simulation] keys name their replacements here too:
        the output booleans name the report list."""
        for key, replacement in [
            ("duration_s", "duration"),
            ("time_res_s", "time_res"),
            ("write_output", "reports"),
            ("output_format", "reports"),
            ("output_path", "reports"),
            ("output_verbosity", "reports"),
            ("retain_batches", "reports"),
            ("rotation", "reports"),
        ]:
            kwargs = required_kwargs() | {key: 3600}
            with pytest.raises(TypeError, match=f"'{key}'.*{replacement}|{replacement}.*'{key}'"):
                SimulationConfig(**cast("dict[str, Any]", kwargs))

    def test_unknown_keyword_is_type_error(self):
        kwargs = required_kwargs() | {"not_a_real_keyword": 1}
        with pytest.raises(TypeError, match="not_a_real_keyword"):
            SimulationConfig(**cast("dict[str, Any]", kwargs))

    def test_python_keyword_names_equal_rust_field_names(self):
        """The constructor's keyword names (pyo3 generates the constructor's
        signature, skipping the VAR_KEYWORD parameter `unexpected`) equal the
        keys `to_toml()` writes after every optional field is set to a
        non-default value; the test holds no list of field names."""
        sig = inspect.signature(SimulationConfig)
        keyword_names = [
            name
            for name, param in sig.parameters.items()
            if param.kind is not param.kind.VAR_KEYWORD
        ]
        cfg = SimulationConfig(
            start_time="2024-06-15T12:00:00Z",
            duration=7200,
            time_res=300,
            reports=[ReportSpec("frame", Selection.verbosity(5), Destination.memory())],
            diagnostics=True,
            output_chunk_size=5000,
            setpoint_deadband_c=1.5,
            master_seed=42,
            civil_timezone="America/Denver",
            site_location=SiteLocationOverride(latitude_deg=40.0),
            max_consecutive_step_failures=3,
        )
        assert cfg.diagnostics is True
        toml_keys = tomllib.loads(cfg.to_toml()).keys()
        assert sorted(keyword_names) == sorted(toml_keys), (
            "the keyword surface and the TOML keys must be the same name map"
        )
        parsed = SimulationConfig.from_toml(cfg.to_toml())
        assert parsed.diagnostics is True

    def test_diagnostics_defaults_false_and_round_trips(self):
        """The diagnostics request defaults to false, the getter and setter
        round-trip it, and the TOML path carries the same default."""
        cfg = SimulationConfig(**required_kwargs())
        assert cfg.diagnostics is False
        cfg.diagnostics = True
        assert cfg.diagnostics is True
        assert SimulationConfig.from_toml(cfg.to_toml()).diagnostics is True
        assert SimulationConfig.from_toml(
            f'start_time = "{START}"\nduration = 3600\n'
        ).diagnostics is False

    def test_site_location_override_round_trips(self):
        """`SiteLocationOverride(latitude_deg=40.0)` through the config,
        `to_toml()` and `from_toml()` is equal, and an out-of-range latitude
        is a `ValueError` naming the field."""
        site = SiteLocationOverride(latitude_deg=40.0)
        cfg = SimulationConfig(start_time=START, duration=3600, site_location=site)
        assert cfg.site_location.latitude_deg == pytest.approx(40.0)
        round_tripped = SimulationConfig.from_toml(cfg.to_toml())
        assert round_tripped.site_location.latitude_deg == pytest.approx(40.0)
        assert round_tripped.site_location.longitude_deg is None
        with pytest.raises(ValueError, match="latitude"):
            SiteLocationOverride(latitude_deg=95.0)

    def test_toml_site_location_out_of_range_is_a_validate_error(self):
        """A TOML config and a Python-built config with the same
        out-of-range site coordinate fail with the message the constructor
        raises: the one validate() check."""
        toml_text = (
            f'start_time = "{START}"\nduration = 3600\n\n[site_location]\nlatitude_deg = 95.0\n'
        )
        with pytest.raises(ValueError) as toml_exc:
            SimulationConfig.from_toml(toml_text)
        with pytest.raises(ValueError) as py_exc:
            SiteLocationOverride(latitude_deg=95.0)
        assert str(py_exc.value) in str(toml_exc.value) or (
            str(toml_exc.value) in str(py_exc.value)
            or "latitude must be finite and within [-90, 90]" in str(toml_exc.value)
        )

    def test_validate_method_runs_the_same_checks(self):
        cfg = SimulationConfig(start_time=START, duration=3600)
        cfg.validate()
        # A setter that would leave the config invalid is refused by the
        # same validation, so a config object is valid after every
        # successful mutation.
        with pytest.raises(ValueError, match="divisible"):
            cfg.time_res = 500
        cfg.validate()
        cfg.time_res = 600
        cfg.validate()


class TestDwellingConfig:
    """Test DwellingConfig Python type."""

    def test_minimal_construction_works(self):
        cfg = DwellingConfig(
            hpxml="path/to/building.xml",
            schedule="path/to/schedules.csv",
            weather="path/to/weather.epw",
            config=SimulationConfig(start_time=START, duration=3600),
        )
        assert cfg.hpxml == "path/to/building.xml"
        assert cfg.schedule == "path/to/schedules.csv"
        assert cfg.weather == "path/to/weather.epw"
        assert cfg.bldg_id == 0
        assert cfg.config.duration == 3600
        assert cfg.defaults_path is None

    def test_initialization_duration_round_trips(self):
        cfg = DwellingConfig(
            hpxml="path/to/building.xml",
            schedule="path/to/schedules.csv",
            weather="path/to/weather.epw",
            config=SimulationConfig(start_time=START, duration=3600),
            initialization_duration=3600,
        )
        assert cfg.initialization_duration == 3600

    def test_removed_key_resample_overrides_names_the_adapter(self):
        """The removed `resample_overrides` keyword errors with the
        removed-keys table's message, which points at the Python adapter
        that holds the weather now. The class is `TypeError`, the same
        `DwellingConfig(...)` raises for an unexpected keyword."""
        removed: dict[str, Any] = {
            "hpxml": "path/to/building.xml",
            "schedule": "path/to/schedules.csv",
            "weather": "path/to/weather.epw",
            "config": SimulationConfig(start_time=START, duration=3600),
            "resample_overrides": {"dry_bulb": "zoh"},
        }
        with pytest.raises(TypeError, match="hares\\.adapters\\.ochre\\.hold_weather"):
            DwellingConfig(**cast("dict[str, Any]", removed))

    def test_removed_key_resample_overrides_on_from_hpxml_names_the_adapter(self):
        """The same removed key on `Dwelling.from_hpxml` errors the same way:
        the message names the adapter, before any input file is read."""
        removed: dict[str, Any] = {
            "resample_overrides": {"dry_bulb": "zoh"},
        }
        with pytest.raises(TypeError, match="hares\\.adapters\\.ochre\\.hold_weather"):
            Dwelling.from_hpxml(
                "path/to/building.xml",
                "path/to/schedules.csv",
                "path/to/weather.epw",
                SimulationConfig(start_time=START, duration=3600),
                **removed,
            )

    def test_removed_simulation_key_names_replacement_on_every_dwelling_constructor(self):
        """A removed [simulation] key on the dwelling constructors names its
        replacement: the SimulationConfig-scope rows fire on every
        constructor, not only on `SimulationConfig` itself. One case per
        scope per constructor pins the scope list."""
        for key, replacement in [
            ("duration_s", "pass duration"),
            ("initialization_duration_s", "top level"),
        ]:
            kwargs: dict[str, Any] = {
                "hpxml": "path/to/building.xml",
                "schedule": "path/to/schedules.csv",
                "weather": "path/to/weather.epw",
                "config": SimulationConfig(start_time=START, duration=3600),
                key: 3600,
            }
            with pytest.raises(TypeError, match=key):
                DwellingConfig(**cast("dict[str, Any]", kwargs))
            with pytest.raises(TypeError, match=replacement):
                DwellingConfig(**cast("dict[str, Any]", kwargs))
            with pytest.raises(TypeError, match=key):
                Dwelling.from_hpxml(**cast("dict[str, Any]", kwargs))

    def test_removed_python_keyword_names_replacement_on_every_dwelling_constructor(self):
        """A removed Python keyword on the dwelling constructors names its
        replacement: the PythonKeyword-scope rows fire on every constructor
        too. One case per scope per constructor pins the scope list."""
        kwargs: dict[str, Any] = {
            "hpxml": "path/to/building.xml",
            "schedule": "path/to/schedules.csv",
            "weather": "path/to/weather.epw",
            "config": SimulationConfig(start_time=START, duration=3600),
            "latitude": 40.0,
        }
        with pytest.raises(TypeError, match="site_location"):
            DwellingConfig(**cast("dict[str, Any]", kwargs))
        with pytest.raises(TypeError, match="site_location"):
            Dwelling.from_hpxml(**cast("dict[str, Any]", kwargs))

    def test_dwelling_config_unknown_keyword_is_type_error(self):
        """Another unknown keyword on `DwellingConfig(...)` raises `TypeError`
        naming it, as Python's own signature check would."""
        unknown: dict[str, Any] = {
            "hpxml": "path/to/building.xml",
            "schedule": "path/to/schedules.csv",
            "weather": "path/to/weather.epw",
            "config": SimulationConfig(start_time=START, duration=3600),
            "not_a_real_keyword": 1,
        }
        with pytest.raises(TypeError, match="not_a_real_keyword"):
            DwellingConfig(**cast("dict[str, Any]", unknown))

    def test_non_finite_overrides_value_fails_loudly(self):
        """A NaN/±inf float inside a DwellingConfig `overrides` object
        must fail loudly at construction. The shared Python→JSON boundary
        converter guards this channel (the config-object entry point for
        the same equipment-override semantics
        `Dwelling.from_hpxml(overrides=...)` carries): a non-finite value
        it let through would be written as `null` by serde_json (JSON
        cannot represent non-finite floats), silently becoming the
        field's documented default — the silent-substitution class the
        sibling entry points gate (`Dwelling.from_hpxml`'s overrides,
        the tariff dicts) and the spec channel's `make_spec` rejects.
        A separate gate from the dwelling-level one because the channel
        is a separate entry point.

        The huge-int face (a Python int too large for i64, e.g. 10**400)
        is asserted only as "an exception is raised", never which layer
        raised it — the boundary-test convention: it is rejected loudly
        by the converter's unsupported-type catch-all ("cannot convert
        Python object of type 'int' to JSON"), which is the same
        loud-never-silent contract by a different arm than the
        non-finite-float rejection.
        """
        base: dict[str, Any] = {
            "hpxml": "path/to/building.xml",
            "schedule": "path/to/schedules.csv",
            "weather": "path/to/weather.epw",
            "config": SimulationConfig(start_time=START, duration=3600),
        }
        # Control: a finite override value crosses cleanly and constructs.
        cfg = DwellingConfig(**cast("dict[str, Any]", base | {"overrides": {"EV": {"battery_temp_c": 15.0}}}))
        assert cfg.overrides is not None

        # The defect face: non-finite float values must raise the
        # converter's non-finite rejection, not silently default.
        for bad in (float("nan"), float("inf"), float("-inf")):
            with pytest.raises(ValueError, match="non-finite"):
                DwellingConfig(**cast("dict[str, Any]", base | {"overrides": {"EV": {"battery_temp_c": bad}}}))

        # The huge-int face: too large for i64, rejected loudly by the
        # converter's unsupported-type arm — never silently defaulted.
        with pytest.raises((ValueError, TypeError)):
            DwellingConfig(**cast("dict[str, Any]", base | {"overrides": {"EV": {"battery_temp_c": 10**400}}}))

    def test_non_string_overrides_key_fails_loudly(self):
        """A non-string key inside a DwellingConfig `overrides` dict must
        fail loudly, never be silently skipped — a skipped key would
        silently drop that equipment's overrides with no error anywhere,
        the same silent-substitution class the value guard rejects.

        The two key faces are pinned per layer, both loud: a non-string
        key *nested* inside a value dict is rejected by the shared
        converter's dict arm ("dict keys must be strings"); a non-string
        *top-level* key is rejected at pyo3's string extraction in the
        config channel's own key loop — asserted only as "an exception
        is raised", never which layer raised it (the boundary-test
        convention).
        """
        base: dict[str, Any] = {
            "hpxml": "path/to/building.xml",
            "schedule": "path/to/schedules.csv",
            "weather": "path/to/weather.epw",
            "config": SimulationConfig(start_time=START, duration=3600),
        }
        # Control: string keys cross cleanly and construct.
        cfg = DwellingConfig(**cast("dict[str, Any]", base | {"overrides": {"EV": {"battery_temp_c": 15.0}}}))
        assert cfg.overrides is not None

        # Nested face: the converter's own dict-arm rejection.
        with pytest.raises(ValueError, match="dict keys must be strings"):
            DwellingConfig(**cast("dict[str, Any]", base | {"overrides": {"EV": {1: 1.0}}}))

        # Top-level face: rejected loudly at the channel's key extraction
        # (pyo3's own TypeError), never silently skipped.
        with pytest.raises((ValueError, TypeError)):
            DwellingConfig(
                **cast("dict[str, Any]", base | {"overrides": {1: 1.0}}),
            )

    def test_with_simulation_config_round_trips(self):
        sim_cfg = SimulationConfig(start_time=START, duration=7200)
        dw_cfg = DwellingConfig(
            hpxml="path/to/building.xml",
            schedule="path/to/schedules.csv",
            weather="path/to/weather.epw",
            config=sim_cfg,
        )
        assert dw_cfg.config.duration == 7200

    def test_missing_required_fields_raises_type_error(self):
        kwargs: dict[str, Any] = {"hpxml": "path/to/building.xml"}
        with pytest.raises(TypeError):
            DwellingConfig(**cast("dict[str, Any]", kwargs))

    def test_overrides_round_trips_as_dict(self):
        overrides = {"hvac": {"capacity_kw": 5.0}}
        cfg = DwellingConfig(
            hpxml="path/to/building.xml",
            schedule="path/to/schedules.csv",
            weather="path/to/weather.epw",
            config=SimulationConfig(start_time=START, duration=3600),
            overrides=overrides,
        )
        result = cfg.overrides
        assert result is not None
        assert "hvac" in result
        assert result["hvac"]["capacity_kw"] == 5.0
