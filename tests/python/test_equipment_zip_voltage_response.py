"""equipment_zip() must not present structurally pinned coefficients as real.

Rule R1 pins the real-power ZIP of every physics-modeled equipment class
(HVAC, water heaters, ventilation fans) to (zp, ip, pp) = (0, 0, 1) so that
real power stays bit-identical to the physics model at any voltage. The
published dict is then indistinguishable from a genuinely constant-power
model: anything computed from the published real coefficients — predicted
voltage sensitivity, an aggregate premise ZIP — silently returns the wrong
answer for that equipment, with no signal to the caller that they are
looking at a reactive-only view.

These tests pin the observable contract that closes that gap:

1. Every ZIP dict carries an explicit ``real_power_zip_applies`` flag so a
   caller can tell "measured constant power" apart from "real-power ZIP
   inapplicable by construction" — for every equipment, not one named case.
2. The flag is True for scheduled/event loads (whose real coefficients do
   govern real power) and False for Rule-R1 physics equipment.
3. A premise-level query exposes the dwelling's real aggregate voltage
   response, which is measurably non-zero even though every physics-typed
   equipment individually reports constant power.
"""

from pathlib import Path

import pytest

from conftest import HARES_DEFAULTS, HPXML, SCHEDULE, WEATHER, make_dwelling

# Physics-modeled (Rule R1) equipment on the base.xml fixture.
PHYSICS_EQUIPMENT = [
    "Gas Furnace",
    "Air Conditioner",
    "Electric Resistance Water Heater",
]
# Scheduled/event loads on the same fixture, whose real ZIP governs P.
SCHEDULED_LOADS = ["Indoor Lighting", "MELs", "Refrigerator"]


class TestRealPowerZipApplicability:
    def test_every_zip_dict_declares_whether_real_power_zip_applies(self):
        dw = make_dwelling(duration_s=600, time_res_s=60)
        seen = 0
        for name in dw.equipment_names():
            zip_dict = dw.equipment_zip(name)
            if zip_dict is None:
                continue  # no electrical ZIP concept (e.g. generator)
            seen += 1
            assert "real_power_zip_applies" in zip_dict, (
                f"equipment_zip({name!r}) must declare whether its real-power "
                f"coefficients govern real power; a bare (0,0,1) is "
                f"indistinguishable from a reactive-only Rule R1 pin"
            )
        assert seen > 0, "fixture must enumerate at least one ZIP-bearing equipment"

    def test_physics_equipment_declared_real_power_zip_inapplicable(self):
        dw = make_dwelling(duration_s=600, time_res_s=60)
        for name in PHYSICS_EQUIPMENT:
            zip_dict = dw.equipment_zip(name)
            assert zip_dict is not None, f"{name} must expose a ZIP dict"
            assert (zip_dict["zp"], zip_dict["ip"], zip_dict["pp"]) == (
                0.0,
                0.0,
                1.0,
            ), f"{name}: Rule R1 pins the real side to constant power"
            assert zip_dict["real_power_zip_applies"] is False, (
                f"{name}: real power comes from the physics model, not the ZIP; "
                f"the published (0,0,1) must be marked inapplicable, not read "
                f"as a measured constant-power response"
            )

    def test_scheduled_loads_declared_real_power_zip_applies(self):
        dw = make_dwelling(duration_s=600, time_res_s=60)
        for name in SCHEDULED_LOADS:
            zip_dict = dw.equipment_zip(name)
            assert zip_dict is not None, f"{name} must expose a ZIP dict"
            assert zip_dict["real_power_zip_applies"] is True, (
                f"{name}: scheduled load real power is driven by its ZIP at "
                f"the bus voltage, so the published coefficients are real"
            )


class TestPremiseVoltageResponse:
    def test_premise_zip_reflects_real_voltage_sensitivity(self):
        """A supported, no-simulation route to whole-premise voltage response.

        Driving the dwelling with set_grid_voltage measurably changes
        consumption (~0.25 %P/%V annually), yet summing per-equipment real
        coefficients from the typed equipment alone predicts zero. The
        premise-level aggregate must expose the real, non-constant-power
        response of the whole load mix.
        """
        dw = make_dwelling(duration_s=600, time_res_s=60)
        premise = dw.premise_zip()
        assert premise is not None, "premise-level ZIP query must exist"
        assert (premise["zp"], premise["ip"], premise["pp"]) != (0.0, 0.0, 1.0), (
            "the dwelling measurably responds to service voltage; a pure "
            "constant-power premise aggregate contradicts the engine's own "
            "simulated response"
        )

    def test_premise_zip_rosters_equipment_by_real_power_regime(self):
        """The aggregate says exactly what it does and does not cover.

        The aggregate is formed from the ZIP-governed mix only, so the
        equipment it excluded (physics/DER-driven, voltage-invariant by
        construction) must be named — a caller must never have to guess
        whether the HVAC was silently dropped.
        """
        dw = make_dwelling(duration_s=600, time_res_s=60)
        premise = dw.premise_zip()
        assert premise is not None
        governing = {e["name"] for e in premise["governing_equipment"]}
        for name in SCHEDULED_LOADS:
            assert name in governing, (
                f"{name} is a ZIP-governed load and must be listed as "
                f"governing with its expected mean power"
            )
        constant_power = set(premise["constant_power_equipment"])
        for name in PHYSICS_EQUIPMENT:
            assert name in constant_power, (
                f"{name} is Rule R1 physics equipment; the premise aggregate "
                f"must name it as excluded rather than silently dropping it"
            )
        # Every governing entry carries its weight so a caller can audit
        # the aggregation.
        for entry in premise["governing_equipment"]:
            assert "mean_power_kw" in entry, (
                f"governing entry {entry['name']!r} must report its "
                f"expected mean power"
            )

    def test_premise_roster_partitions_all_zip_equipment(self):
        """The rosters and the per-equipment flag must tell one story.

        Every ZIP-bearing equipment appears in exactly one roster, on the
        side its own ``real_power_zip_applies`` flag declares; equipment
        with no ZIP concept at all appears in neither. A flag/roster
        disagreement on any equipment class — not just the ones named
        above — would let a caller reconcile the two APIs into a wrong
        premise answer.
        """
        dw = make_dwelling(duration_s=600, time_res_s=60)
        premise = dw.premise_zip()
        assert premise is not None
        governing = {e["name"] for e in premise["governing_equipment"]}
        constant_power = set(premise["constant_power_equipment"])
        assert governing.isdisjoint(constant_power), (
            f"equipment in both rosters: {governing & constant_power}"
        )
        for name in dw.equipment_names():
            zip_dict = dw.equipment_zip(name)
            if zip_dict is None:
                assert name not in governing and name not in constant_power, (
                    f"{name}: no ZIP concept, so it must appear in neither "
                    f"premise roster"
                )
            elif zip_dict["real_power_zip_applies"]:
                assert name in governing, (
                    f"{name}: flag says its real ZIP governs, but the premise "
                    f"aggregate does not list it as governing"
                )
            else:
                assert name in constant_power, (
                    f"{name}: flag says its real ZIP is a structural pin, but "
                    f"the premise aggregate does not list it as constant power"
                )
        rostered = governing | constant_power
        for name in dw.equipment_names():
            if dw.equipment_zip(name) is not None:
                assert name in rostered, f"{name}: ZIP-bearing but unrostered"

    def test_premise_aggregate_is_weighted_mean_of_governing_coefficients(self):
        """The published aggregate is the documented power-weighted mean.

        Recomputing the weighted mean from the per-equipment dicts and the
        rostered weights must reproduce the aggregate coefficients exactly:
        a caller auditing the premise answer against the per-equipment API
        is exercising the documented contract, and any divergence means the
        aggregate silently used different coefficients or weights than it
        published.
        """
        dw = make_dwelling(duration_s=600, time_res_s=60)
        premise = dw.premise_zip()
        assert premise is not None
        assert premise["real_power_zip_applies"] is True, (
            "the aggregate describes the ZIP-governed mix by construction"
        )
        weighted = [
            (dw.equipment_zip(e["name"]), e["mean_power_kw"])
            for e in premise["governing_equipment"]
            if e["mean_power_kw"] is not None and e["mean_power_kw"] > 0
        ]
        assert weighted, "fixture must have positively weighted governing loads"
        total = sum(w for _, w in weighted)
        for key in ("zp", "ip", "pp", "zq", "iq", "pq", "v0"):
            expected = sum(z[key] * w for z, w in weighted) / total
            assert premise[key] == pytest.approx(expected, rel=1e-12, abs=1e-15), (
                f"premise {key}: published aggregate diverges from the "
                f"power-weighted mean of the published per-equipment values"
            )
        # Each governing row is init-validated to sum to 1, so the weighted
        # mean does too — a caller can rely on closure.
        assert premise["zp"] + premise["ip"] + premise["pp"] == pytest.approx(1.0)
        # pf aggregates through the power-weighted mean of tan(acos(pf)):
        # reactive power is proportional to tan(phi), and
        # pf = cos(atan(x)) = 1/sqrt(1 + x^2) recovers the magnitude.
        import math

        tan_phi = sum(math.tan(math.acos(z["pf"])) * w for z, w in weighted) / total
        expected_pf = 1.0 / math.sqrt(1.0 + tan_phi * tan_phi)
        assert premise["pf"] == pytest.approx(expected_pf, rel=1e-12), (
            "premise pf: published aggregate diverges from the documented "
            "tan(acos(pf)) weighted mean of the governing equipment"
        )
        assert 0.0 < premise["pf"] <= 1.0


class TestScheduleDataIntegrity:
    def test_nan_schedule_value_fails_loudly_with_cause(self, tmp_path):
        """Corrupt schedule data must never silently skew premise weights.

        The premise aggregate weights equipment by expected mean draw over
        the loaded schedule; a NaN admitted at load (``str::parse::<f64>``
        accepts ``"nan"``) would degrade that weight with no signal — the
        same failure class as the Rule R1 conflation, one layer down. The
        loader rejects non-finite values at the parse boundary, and the
        error must carry its cause (which value, which column) to the
        caller through the bindings.
        """
        from ochre_next import Dwelling

        lines = Path(SCHEDULE).read_text().splitlines()
        column_name = lines[0].split(",")[3]
        parts = lines[1].split(",")
        parts[3] = "nan"
        lines[1] = ",".join(parts)
        bad_schedule = tmp_path / "schedule_with_nan.csv"
        bad_schedule.write_text("\n".join(lines) + "\n")

        with pytest.raises(Exception) as excinfo:
            Dwelling.from_hpxml(
                HPXML,
                str(bad_schedule),
                WEATHER,
                start_time="2019-01-01T00:00:00",
                duration_s=600,
                time_res_s=60,
                defaults_path=str(HARES_DEFAULTS),
                bldg_id=42,
                master_seed=0,
            )
        message = str(excinfo.value)
        assert "non-finite" in message, (
            f"the rejection must name the defect, got: {message}"
        )
        assert column_name in message, (
            f"the rejection must name the offending column {column_name!r}, "
            f"got: {message}"
        )


class TestZipDataIntegrity:
    def test_non_finite_zip_sidecar_fails_loudly_with_cause(self, tmp_path):
        """Corrupt ZIP coefficients must never reach the published dicts.

        The defaults sidecar (``zip_parameters.toml`` under the
        user-supplied ``defaults_path``) is the reachable non-finite
        channel: TOML parses ``nan`` natively, the loader does no
        finiteness check, and the sum-based init validation passes NaN
        (NaN comparisons are false). The corrupt coefficient then flows
        into ``equipment_zip()`` and — power-weighted over a finite,
        positive-weight roster — into ``premise_zip()``, which publishes a
        structurally normal aggregate with ``nan`` where a coefficient
        belongs: the same silent-degradation class as the Rule R1
        conflation, one channel over. The build must reject it with its
        cause (the defect and the offending equipment row) instead.
        """
        import re

        from ochre_next import Dwelling

        defaults = tmp_path / "defaults"
        defaults.mkdir()
        for child in HARES_DEFAULTS.iterdir():
            if child.name == "zip_parameters.toml":
                continue
            (defaults / child.name).symlink_to(child)
        zip_toml = defaults / "zip_parameters.toml"
        text = (HARES_DEFAULTS / "zip_parameters.toml").read_text()
        corrupted, n = re.subn(
            r"(\[mels\][^\[]*?zp\s*=\s*)[-0-9.]+", r"\1nan", text, count=1
        )
        assert n == 1, "fixture: [mels] zp must exist in zip_parameters.toml"
        zip_toml.write_text(corrupted)

        with pytest.raises(Exception) as excinfo:
            Dwelling.from_hpxml(
                HPXML,
                SCHEDULE,
                WEATHER,
                start_time="2019-01-01T00:00:00",
                duration_s=600,
                time_res_s=60,
                defaults_path=str(defaults),
                bldg_id=42,
                master_seed=0,
            )
        message = str(excinfo.value).lower()
        assert "non-finite" in message, (
            f"the rejection must name the defect, got: {excinfo.value}"
        )
        assert "mels" in message, (
            f"the rejection must name the offending equipment row, "
            f"got: {excinfo.value}"
        )

    def test_tiny_v0_zip_override_fails_loudly_with_cause(self):
        """A v0 that overflows the normalized voltage must fail at build.

        The ``"zip"`` override channel accepts any finite f64, and
        ``v0 > 0`` validation is necessary but not sufficient: a positive
        v0 small enough that ``v / v0`` overflows makes the ZIP multiplier
        NaN (subnormal v0: ``0 * inf``) or infinite (tiny v0: ``v_norm**2``
        past ``f64::MAX``). Executed on this checkout, a subnormal v0
        override builds and initializes cleanly — every guard passes — and
        then panics the simulation mid-step (release builds: silent NaN in
        every electric power value). A config typo must be rejected at the
        boundary with its cause, not deferred to a mid-simulation crash.
        """
        from ochre_next import Dwelling

        with pytest.raises(Exception) as excinfo:
            Dwelling.from_hpxml(
                HPXML,
                SCHEDULE,
                WEATHER,
                start_time="2019-01-01T00:00:00",
                duration_s=600,
                time_res_s=60,
                defaults_path=str(HARES_DEFAULTS),
                bldg_id=42,
                master_seed=0,
                overrides={"Indoor Lighting": {"zip": {"v0": 1e-320}}},
            )
        message = str(excinfo.value).lower()
        assert "v0" in message, (
            f"the rejection must name the offending field, got: {excinfo.value}"
        )
