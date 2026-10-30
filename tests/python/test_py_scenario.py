"""The scenario spec and the sampler: the declared spec's surface, the
sampler's determinism and spec conformance, and the overrides round-trip
proving each sampled value reached the typed config it names.

The binding under test is ``hares.ScenarioSpec``: a spec declared as a
dict (the parameters' distributions, units, targets and correlation
groups), ``sample(master_seed, scenario_id, bldg_id)`` returning the
home's sampled parameter record, and the overrides form the dwelling
constructor accepts. Every witness is deterministic: the same (seed,
scenario, building) reproduces, a different scenario or building draws
differently, and the marginals conform to the spec's distributions.
"""

import statistics

import pytest

from hares import ScenarioSpec


def load_spec() -> ScenarioSpec:
    """A two-parameter spec: one overridable equipment target (the water
    heater's setpoint, which the dwelling's override path accepts) and one
    patch target."""
    return ScenarioSpec(
        {
            "name": "test-scenario",
            "parameters": [
                {
                    "name": "wh_setpoint",
                    "unit": "C",
                    "target": {
                        "override": {
                            "equipment": "Electric Resistance Water Heater",
                            "field": "setpoint_c",
                        }
                    },
                    "distribution": {"uniform": {"low": 45.0, "high": 60.0}},
                },
                {
                    "name": "bedrooms",
                    "unit": "count",
                    "target": {"patch": {"field": "number_of_bedrooms"}},
                    "distribution": {"triangular": {"min": 1.0, "mode": 3.0, "max": 5.0}},
                },
            ],
        }
    )


class TestSamplerDeterminism:
    """The sampler's determinism contract."""

    def test_same_inputs_reproduce_the_same_parameters(self):
        spec = load_spec()
        a = spec.sample(master_seed=42, scenario_id=1, bldg_id=7)
        b = spec.sample(master_seed=42, scenario_id=1, bldg_id=7)
        assert a == b

    def test_another_scenario_or_building_draws_differently(self):
        spec = load_spec()
        base = spec.sample(master_seed=42, scenario_id=1, bldg_id=7)
        other_scenario = spec.sample(master_seed=42, scenario_id=2, bldg_id=7)
        other_building = spec.sample(master_seed=42, scenario_id=1, bldg_id=8)
        assert base != other_scenario
        assert base != other_building
        # Declared scenario 0 is not the absent scenario's stream either:
        # the sampler always declares its scenario.
        zero = spec.sample(master_seed=42, scenario_id=0, bldg_id=7)
        one = spec.sample(master_seed=42, scenario_id=1, bldg_id=7)
        assert zero != one


class TestSpecConformance:
    """The sampled marginals conform to the spec's distributions."""

    def test_uniform_moments_and_range(self):
        spec = ScenarioSpec(
            {
                "name": "uniform",
                "parameters": [
                    {
                        "name": "p",
                        "unit": "count",
                        "target": {"patch": {"field": "number_of_bedrooms"}},
                        "distribution": {"uniform": {"low": 1.0, "high": 5.0}},
                    }
                ],
            }
        )
        values = [
            spec.sample(master_seed=42, scenario_id=s, bldg_id=s)["values"][0]["value"]
            for s in range(4000)
        ]
        assert all(1.0 <= v <= 5.0 for v in values)
        assert statistics.fmean(values) == pytest.approx(3.0, abs=0.08)
        assert statistics.stdev(values) == pytest.approx(4.0 / 12**0.5, rel=0.05)

    def test_normal_moments(self):
        spec = ScenarioSpec(
            {
                "name": "normal",
                "parameters": [
                    {
                        "name": "p",
                        "unit": "C",
                        "target": {
                            "override": {
                                "equipment": "Electric Resistance Water Heater",
                                "field": "setpoint_c",
                            }
                        },
                        # The water heater's setpoint validates in [40, 85]
                        # (the typed config's check_range), so the fixture's
                        # mean sits inside the band its target accepts: the
                        # 4,000-draw range at sigma 1.5 stays inside it.
                        "distribution": {"normal": {"mean": 50.0, "std_dev": 1.5}},
                    }
                ],
            }
        )
        values = [
            spec.sample(master_seed=7, scenario_id=s, bldg_id=0)["values"][0]["value"]
            for s in range(4000)
        ]
        assert statistics.fmean(values) == pytest.approx(50.0, abs=0.08)
        assert statistics.stdev(values) == pytest.approx(1.5, rel=0.05)

    def test_lognormal_moments(self):
        spec = ScenarioSpec(
            {
                "name": "lognormal",
                "parameters": [
                    {
                        "name": "p",
                        "unit": "fraction",
                        "target": {
                            "override": {"equipment": "MELs", "field": "usage_multiplier"}
                        },
                        "distribution": {"lognormal": {"mu": 0.0, "sigma": 0.25}},
                    }
                ],
            }
        )
        values = [
            spec.sample(master_seed=9, scenario_id=s, bldg_id=0)["values"][0]["value"]
            for s in range(4000)
        ]
        # The analytical mean exp(mu + sigma^2/2) and std; the moments
        # within the sampling error of 4,000 draws.
        import math

        expected_mean = math.exp(0.0 + 0.25**2 / 2)
        assert statistics.fmean(values) == pytest.approx(expected_mean, rel=0.05)
        assert all(v > 0.0 for v in values)

    def test_triangular_moments_and_range(self):
        spec = ScenarioSpec(
            {
                "name": "triangular",
                "parameters": [
                    {
                        "name": "p",
                        "unit": "count",
                        "target": {"patch": {"field": "number_of_bedrooms"}},
                        "distribution": {"triangular": {"min": 1.0, "mode": 1.2, "max": 5.0}},
                    }
                ],
            }
        )
        values = [
            spec.sample(master_seed=3, scenario_id=s, bldg_id=0)["values"][0]["value"]
            for s in range(4000)
        ]
        assert all(1.0 <= v <= 5.0 for v in values)
        # The triangular mean is (min + mode + max) / 3, not the mode: an
        # asymmetric triangle (min 1.0, mode 1.2, max 5.0) has mean 2.4,
        # and the mean's standard error at 4,000 draws is about 0.015.
        assert statistics.fmean(values) == pytest.approx(2.4, abs=0.05)

    def test_discrete_values_only(self):
        spec = ScenarioSpec(
            {
                "name": "discrete",
                "parameters": [
                    {
                        "name": "p",
                        "unit": "count",
                        "target": {"patch": {"field": "number_of_bedrooms"}},
                        "distribution": {
                            "discrete": {"values": [2.0, 3.0, 5.0], "weights": [0.2, 0.5, 0.3]}
                        },
                    }
                ],
            }
        )
        values = [
            spec.sample(master_seed=11, scenario_id=s, bldg_id=0)["values"][0]["value"]
            for s in range(4000)
        ]
        assert set(values) <= {2.0, 3.0, 5.0}
        share_three = sum(1 for v in values if v == 3.0) / len(values)
        assert share_three == pytest.approx(0.5, abs=0.03)

    def test_correlated_group_members_move_together(self):
        spec = ScenarioSpec(
            {
                "name": "correlated",
                "parameters": [
                    {
                        "name": "a",
                        "unit": "fraction",
                        "target": {
                            "override": {"equipment": "MELs", "field": "usage_multiplier"}
                        },
                        "distribution": {"uniform": {"low": 0.0, "high": 1.0}},
                        "correlation": {"group": "load", "rho": 0.9},
                    },
                    {
                        "name": "b",
                        "unit": "fraction",
                        "target": {
                            "override": {"equipment": "TV", "field": "usage_multiplier"}
                        },
                        "distribution": {"uniform": {"low": 0.0, "high": 1.0}},
                        "correlation": {"group": "load", "rho": 0.9},
                    },
                ],
            }
        )
        pairs = [
            tuple(
                v["value"]
                for v in spec.sample(master_seed=5, scenario_id=s, bldg_id=0)["values"]
            )
            for s in range(2000)
        ]
        a = [p[0] for p in pairs]
        b = [p[1] for p in pairs]
        mean_a, mean_b = statistics.fmean(a), statistics.fmean(b)
        cov = sum((x - mean_a) * (y - mean_b) for x, y in zip(a, b)) / len(a)
        rho = cov / (statistics.pstdev(a) * statistics.pstdev(b))
        assert rho > 0.5, f"the group's members must move together, got rho {rho}"


class TestOverridesRoundTrip:
    """Each sampled value reaches the typed config it names."""

    def test_sample_overrides_form_and_patch_form(self):
        spec = load_spec()
        sample = spec.sample(master_seed=42, scenario_id=1, bldg_id=7)
        overrides = spec.sample_overrides(master_seed=42, scenario_id=1, bldg_id=7)
        # The overrides form: the equipment key with the field key.
        assert "Electric Resistance Water Heater" in overrides
        assert "setpoint_c" in overrides["Electric Resistance Water Heater"]
        sampled_value = {v["name"]: v["value"] for v in sample["values"]}
        assert (
            overrides["Electric Resistance Water Heater"]["setpoint_c"]
            == sampled_value["wh_setpoint"]
        )
        # The sample's record: the names, the values, the units.
        assert sample["scenario_id"] == 1
        assert sample["bldg_id"] == 7
        assert {v["name"] for v in sample["values"]} == {"wh_setpoint", "bedrooms"}
        assert {v["unit"] for v in sample["values"]} <= {"C", "count"}

    def test_the_sample_record_carries_the_declared_targets(self):
        """The record's ``targets`` map is the application payload a manifest
        consumer reads: every sampled value's name maps to the target the
        spec declared, in the serde shape (``override``/``patch``). The
        record's top-level keys and the target shapes are the contract; a
        rename would break a consumer while the sampler's own values stayed
        the same."""
        spec = load_spec()
        sample = spec.sample(master_seed=42, scenario_id=1, bldg_id=7)
        assert set(sample) == {"scenario_id", "bldg_id", "values", "targets"}
        assert sample["targets"] == {
            "wh_setpoint": {
                "override": {
                    "equipment": "Electric Resistance Water Heater",
                    "field": "setpoint_c",
                }
            },
            "bedrooms": {"patch": {"field": "number_of_bedrooms"}},
        }

    def test_sampled_override_reaches_the_typed_config(self):
        """The round-trip: sample, apply through the dwelling constructor's
        overrides, and the built dwelling runs the sampled value (read
        back from the resolved model's equipment read-back, bitwise)."""
        from conftest import HARES_DEFAULTS, HPXML, WEATHER
        from hares import Dwelling, SimulationConfig

        spec = ScenarioSpec(
            {
                "name": "round-trip",
                "parameters": [
                    {
                        "name": "wh_setpoint",
                        "unit": "C",
                        "target": {
                            "override": {
                                "equipment": "Electric Resistance Water Heater",
                                "field": "setpoint_c",
                            }
                        },
                        "distribution": {"uniform": {"low": 45.0, "high": 60.0}},
                    }
                ],
            }
        )
        overrides = spec.sample_overrides(master_seed=42, scenario_id=3, bldg_id=7)
        sampled = spec.sample(master_seed=42, scenario_id=3, bldg_id=7)["values"][0]["value"]
        config = SimulationConfig(
            start_time="2019-01-01T00:00:00",
            duration=300,
            time_res=60,
        )
        dw = Dwelling.from_hpxml(
            HPXML,
            None,
            WEATHER,
            config=config,
            defaults=str(HARES_DEFAULTS),
            bldg_id=7,
            overrides=overrides,
        )
        dw.initialize()
        # The sampled value reached the typed config it names: the water
        # heater's resolved setpoint is the sampled value bit for bit.
        resolved = dw.resolved_model()
        wh = next(
            eq
            for eq in resolved["equipment"]
            if eq["name"] == "Electric Resistance Water Heater"
        )
        setpoint = next(p for p in wh["parameters"] if p["name"] == "setpoint_c")
        assert setpoint["value"] == sampled
        assert setpoint["unit"] == "C"


class TestSpecSerializationRoundTrip:
    """The Python dict IS the spec's serialized form: a spec written out to
    JSON text and read back parses to the same spec and samples the same
    values bit for bit, so a study can record and reload its declaration.
    Every family is declared, with a correlated pair and a patch target,
    and the keys are reordered on the way through."""

    def test_a_json_round_tripped_spec_samples_bitwise_the_same(self):
        import json

        # Each family pairs with a target whose units and validated domain
        # the family's draws carry: the capacity at the water heater's
        # public capacity field (W), the setpoint inside the typed config's
        # [40, 85] C validation, the lognormal and triangular members at
        # the two loads' usage multipliers they are the class of
        # (fraction), the discrete at the integer bedroom counts its patch
        # field holds (count). The pair's two targets are distinct
        # equipment: a wildcard on a field a named entry already writes
        # would be shadowed by that entry at the only equipment reading
        # it.
        override = {
            "override": {
                "equipment": "Electric Resistance Water Heater",
                "field": "heating_capacity_w",
            }
        }
        second_override = {
            "override": {
                "equipment": "Electric Resistance Water Heater",
                "field": "setpoint_c",
            }
        }
        multiplier_override = {
            "override": {"equipment": "MELs", "field": "usage_multiplier"}
        }
        tv_multiplier_override = {
            "override": {"equipment": "TV", "field": "usage_multiplier"}
        }
        patch = {"patch": {"field": "number_of_bedrooms"}}
        spec_dict = {
            "name": "round-trip",
            "parameters": [
                {
                    "name": "u",
                    "unit": "W",
                    "target": override,
                    "distribution": {"uniform": {"low": 4000.0, "high": 6000.0}},
                },
                {
                    "name": "n",
                    "unit": "C",
                    "target": second_override,
                    "distribution": {"normal": {"mean": 50.5, "std_dev": 1.5}},
                },
                {
                    "name": "l",
                    "unit": "fraction",
                    "target": multiplier_override,
                    "distribution": {"lognormal": {"mu": 0.0, "sigma": 0.25}},
                    "correlation": {"group": "load", "rho": 0.9},
                },
                {
                    "name": "t",
                    "unit": "fraction",
                    "target": tv_multiplier_override,
                    "distribution": {"triangular": {"min": 0.5, "mode": 1.0, "max": 1.5}},
                    "correlation": {"group": "load", "rho": 0.9},
                },
                {
                    "name": "d",
                    "unit": "count",
                    "target": patch,
                    "distribution": {"discrete": {"values": [2.0, 3.0, 5.0], "weights": [0.2, 0.5, 0.3]}},
                },
            ],
        }
        original = ScenarioSpec(spec_dict)
        reloaded = ScenarioSpec(json.loads(json.dumps(spec_dict, sort_keys=True)))
        for master_seed, scenario_id, bldg_id in [(42, 1, 7), (0, 0, 0), (7, 2**63, -3)]:
            assert reloaded.sample(master_seed, scenario_id, bldg_id) == original.sample(
                master_seed, scenario_id, bldg_id
            )
            assert reloaded.sample_overrides(
                master_seed, scenario_id, bldg_id
            ) == original.sample_overrides(master_seed, scenario_id, bldg_id)


class TestSpecValidation:
    """The declared spec's loud errors: a typed ``ValueError`` from the
    binding, never a Rust panic crossing (a panic surfaces as pyo3's
    ``PanicException``, a ``BaseException``) and never a silently repaired
    spec."""

    def test_unknown_distribution_is_an_error(self):
        with pytest.raises(ValueError, match="unknown variant"):
            ScenarioSpec(
                {
                    "name": "bad",
                    "parameters": [
                        {
                            "name": "p",
                            "unit": "W",
                            "target": {"override": {"equipment": "x", "field": "y"}},
                            "distribution": {"zipf": {}},
                        }
                    ],
                }
            )

    def test_duplicate_names_are_an_error(self):
        with pytest.raises(ValueError, match="repeats the parameter name 'p'.*unique"):
            ScenarioSpec(
                {
                    "name": "dup",
                    "parameters": [
                        {
                            "name": "p",
                            "unit": "W",
                            "target": {"override": {"equipment": "x", "field": "y"}},
                            "distribution": {"uniform": {"low": 0.0, "high": 1.0}},
                        },
                        {
                            "name": "p",
                            "unit": "W",
                            "target": {"override": {"equipment": "x", "field": "z"}},
                            "distribution": {"uniform": {"low": 0.0, "high": 1.0}},
                        },
                    ],
                }
            )

    def test_unknown_patch_field_is_an_error(self):
        with pytest.raises(ValueError, match="'p'.*not_a_patch_field"):
            ScenarioSpec(
                {
                    "name": "bad-patch",
                    "parameters": [
                        {
                            "name": "p",
                            "unit": "count",
                            "target": {"patch": {"field": "not_a_patch_field"}},
                            "distribution": {"uniform": {"low": 0.0, "high": 1.0}},
                        }
                    ],
                }
            )

    def test_a_malformed_distribution_names_its_parameter(self):
        """The spec's validation names the parameter carrying a malformed
        distribution, so a multi-parameter spec's author fixes the named
        one; the family's own reason rides after it. The refusal is the
        binding's ``ValueError``, not a panic crossing."""
        with pytest.raises(ValueError, match="'inverted'.*uniform"):
            ScenarioSpec(
                {
                    "name": "malformed-distribution",
                    "parameters": [
                        {
                            "name": "ok",
                            "unit": "fraction",
                            "target": {"override": {"equipment": "MELs", "field": "usage_multiplier"}},
                            "distribution": {"uniform": {"low": 0.0, "high": 1.0}},
                        },
                        {
                            "name": "inverted",
                            "unit": "fraction",
                            "target": {"override": {"equipment": "TV", "field": "usage_multiplier"}},
                            "distribution": {"uniform": {"low": 5.0, "high": 1.0}},
                        },
                    ],
                }
            )

    def test_a_spec_that_is_not_a_mapping_is_an_error(self):
        with pytest.raises(ValueError, match="does not parse"):
            ScenarioSpec("not a spec")  # type: ignore[arg-type]

    def test_a_non_finite_parameter_is_an_error(self):
        with pytest.raises(ValueError, match="non-finite"):
            ScenarioSpec(
                {
                    "name": "nan",
                    "parameters": [
                        {
                            "name": "p",
                            "unit": "W",
                            "target": {"override": {"equipment": "x", "field": "y"}},
                            "distribution": {"uniform": {"low": float("nan"), "high": 1.0}},
                        }
                    ],
                }
            )

    def test_two_parameters_at_one_target_are_an_error(self):
        """A target is the application's map key: two parameters writing one
        field would keep the later value and drop the earlier silently, so
        the spec refuses the authoring shape naming both parameters."""
        target = {"override": {"equipment": "MELs", "field": "usage_multiplier"}}
        with pytest.raises(ValueError, match="'a'.*'b'"):
            ScenarioSpec(
                {
                    "name": "same-target",
                    "parameters": [
                        {
                            "name": "a",
                            "unit": "fraction",
                            "target": target,
                            "distribution": {"uniform": {"low": 1.0, "high": 2.0}},
                        },
                        {
                            "name": "b",
                            "unit": "fraction",
                            "target": target,
                            "distribution": {"uniform": {"low": 2.0, "high": 3.0}},
                        },
                    ],
                }
            )

    def test_an_overflowing_closed_form_is_a_value_error_naming_the_parameter(self):
        """A distribution whose parameters validate but whose closed form
        leaves the finite f64 range refuses at the draw, and the binding
        carries the refusal as a ``ValueError`` naming the parameter and
        the family: never a panic crossing (a ``PanicException`` is a
        ``BaseException``, so it fails this pin) and never a non-finite
        value in the record. The spec itself constructs: ``mu`` 800 is a
        finite parameter, and only the exponentiation overflows."""
        spec = ScenarioSpec(
            {
                "name": "overflow",
                "parameters": [
                    {
                        "name": "huge_capacity",
                        "unit": "W",
                        "target": {
                            "override": {
                                "equipment": "Electric Resistance Water Heater",
                                "field": "heating_capacity_w",
                            }
                        },
                        "distribution": {"lognormal": {"mu": 800.0, "sigma": 1.0}},
                    }
                ],
            }
        )
        for call in (spec.sample, spec.sample_overrides):
            with pytest.raises(ValueError, match="huge_capacity.*lognormal"):
                call(master_seed=1, scenario_id=1, bldg_id=1)
