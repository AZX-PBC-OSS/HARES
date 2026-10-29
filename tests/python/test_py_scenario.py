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
    """A two-parameter spec: one override target, one patch target."""
    return ScenarioSpec(
        {
            "name": "test-scenario",
            "parameters": [
                {
                    "name": "occupant_load_w",
                    "unit": "W",
                    "target": {"override": {"equipment": "Occupancy", "field": "number_of_occupants"}},
                    "distribution": {"uniform": {"low": 1.0, "high": 5.0}},
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
                        "unit": "W",
                        "target": {"override": {"equipment": "Occupancy", "field": "number_of_occupants"}},
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
                        "target": {"override": {"equipment": "Occupancy", "field": "number_of_occupants"}},
                        "distribution": {"normal": {"mean": 20.0, "std_dev": 1.5}},
                    }
                ],
            }
        )
        values = [
            spec.sample(master_seed=7, scenario_id=s, bldg_id=0)["values"][0]["value"]
            for s in range(4000)
        ]
        assert statistics.fmean(values) == pytest.approx(20.0, abs=0.08)
        assert statistics.stdev(values) == pytest.approx(1.5, rel=0.05)

    def test_lognormal_moments(self):
        spec = ScenarioSpec(
            {
                "name": "lognormal",
                "parameters": [
                    {
                        "name": "p",
                        "unit": "W",
                        "target": {"override": {"equipment": "Occupancy", "field": "number_of_occupants"}},
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
                        "unit": "W",
                        "target": {"override": {"equipment": "Occupancy", "field": "number_of_occupants"}},
                        "distribution": {"uniform": {"low": 0.0, "high": 1.0}},
                        "correlation": {"group": "load", "rho": 0.9},
                    },
                    {
                        "name": "b",
                        "unit": "W",
                        "target": {"patch": {"field": "number_of_bedrooms"}},
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
        assert "Occupancy" in overrides
        assert "number_of_occupants" in overrides["Occupancy"]
        sampled_value = {v["name"]: v["value"] for v in sample["values"]}
        assert overrides["Occupancy"]["number_of_occupants"] == sampled_value["occupant_load_w"]
        # The sample's record: the names, the values, the units.
        assert sample["scenario_id"] == 1
        assert sample["bldg_id"] == 7
        assert {v["name"] for v in sample["values"]} == {"occupant_load_w", "bedrooms"}
        assert {v["unit"] for v in sample["values"]} <= {"W", "count"}

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

        override = {"override": {"equipment": "Occupancy", "field": "number_of_occupants"}}
        patch = {"patch": {"field": "number_of_bedrooms"}}
        spec_dict = {
            "name": "round-trip",
            "parameters": [
                {
                    "name": "u",
                    "unit": "W",
                    "target": override,
                    "distribution": {"uniform": {"low": 1.0, "high": 5.0}},
                },
                {
                    "name": "n",
                    "unit": "C",
                    "target": override,
                    "distribution": {"normal": {"mean": 20.5, "std_dev": 1.5}},
                },
                {
                    "name": "l",
                    "unit": "W",
                    "target": patch,
                    "distribution": {"lognormal": {"mu": 0.0, "sigma": 0.25}},
                    "correlation": {"group": "load", "rho": 0.9},
                },
                {
                    "name": "t",
                    "unit": "count",
                    "target": patch,
                    "distribution": {"triangular": {"min": 1.0, "mode": 1.2, "max": 5.0}},
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
        with pytest.raises(ValueError, match="name"):
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
        with pytest.raises(ValueError, match="patch"):
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
