"""equipment_zip() must publish the coefficients that actually govern.

The ZIP-bearing equipment publishes its class row, and the row's real
coefficients scale that equipment's real power at the bus voltage: the
scheduled/event loads scale their scheduled draw; the typed physics
equipment (HVAC, water heaters, ventilation) scales its physics-computed
draw through the recorded deep-sag form. Only DER (battery, PV, EV) is
reactive-only: its real power is controller-driven and never ZIP-scaled,
and only its reactive coefficients apply.

These tests pin the observable contract:

1. Every ZIP dict carries an explicit ``real_power_zip_applies`` flag so a
   caller can tell "measured constant power" apart from "real power driven
   by a DER controller" for every equipment, not one named case.
2. The flag is True for scheduled/event loads and typed physics equipment
   (whose real coefficients do govern the real power at the bus voltage)
   and False for DER.
3. The deep-sag response is real and follows the recorded shape: the
   refrigerator class's held turn (monotone in voltage across the
   admitted band), the motor families' fitted elevation, and the premise
   aggregate reporting the assembled deep-sag response of the moment's
   composition.
"""

from pathlib import Path

import pytest

from conftest import HARES_DEFAULTS, HPXML, SCHEDULE, WEATHER, make_dwelling
from hares import SimulationConfig


def _f64_bits(x: float) -> int:
    """The float's IEEE-754 bits (Python floats carry no to_bits)."""
    import struct

    return struct.unpack("<Q", struct.pack("<d", x))[0]


def _deep_sag_real_multiplier(zip_dict, voltage_pu: float, as_fitted: bool) -> float:
    """Evaluate a published row's real coefficients at a pu voltage through
    the recorded deep-sag form.

    The form is the record's (``docs/PHYSICS_DECISIONS.md``, the deep-sag
    entry; ``ZipLoad::deep_sag_real_multiplier``): the row's real
    polynomial at ``voltage_pu / v0``, with every class outside the
    recorded motor families held at the row's in-band stationary point on
    the deeper-sag side (the evaluated response never rises as the
    voltage falls) and the recorded motor families evaluating the raw
    polynomial as fitted. The model's own evaluation is witnessed end to
    end by the dwelling witnesses below and the hares-types row tests;
    this evaluation pins the published coefficients to the same physics.
    """
    zp, ip, pp = zip_dict["zp"], zip_dict["ip"], zip_dict["pp"]
    v_norm = voltage_pu / zip_dict["v0"]
    if v_norm == 1.0:
        return 1.0
    if not as_fitted and v_norm < 1.0 and zp > 0.0:
        v_turn = -ip / (2.0 * zp)
        if 0.5 / zip_dict["v0"] <= v_turn < 1.0:
            held = v_turn if v_norm < v_turn else v_norm
            return zp * held * held + ip * held + pp
    return zp * v_norm * v_norm + ip * v_norm + pp

# Physics-modeled (typed) equipment on the base.xml fixture: their class
# rows govern the voltage response of their physics-computed draws.
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
                f"indistinguishable from a reactive-only DER view"
            )
        assert seen > 0, "fixture must enumerate at least one ZIP-bearing equipment"

    def test_physics_equipment_declares_its_class_row_as_governing(self):
        """The typed equipment's published coefficients are its class row's
        own, and their evaluations at exact pu voltages are the physics the
        table declares.

        The row's real coefficients scale the equipment's physics-computed
        draw at the bus voltage (the recorded deep-sag decision), so the
        published ``(zp, ip, pp)`` are the class row's own and the flag
        declares them governing: the brownout round's F1/F2 defect was a
        structural ``(0,0,1)`` pin that reported constant power while the
        table scored the row an impedance or a motor response. That defect
        class is a real side re-pinned to ``(0, 0, 1)`` behind a ``True``
        flag, and a re-pin evaluates flat 1.0 at every voltage (the Ideal
        class's own constant-power row), so the pins below hold the class
        rows exactly and their hand-derived deep-sag evaluations, which a
        re-pin cannot satisfy.
        """
        dw = make_dwelling(duration_s=600, time_res_s=60)

        # The class rows, hand-pinned from the recorded literature table
        # (`zip_defaults_for_class` in crates/hares-types/src/zip.rs): the
        # gas furnace's electric draw is its blower fan motor (the OCHRE
        # fans row), the air conditioner's compressor the Hajagos & Danai
        # 1998 cooling row, the resistance water heater's element pure
        # impedance (the OCHRE CSV row 17). The boolean is the recorded
        # deep-sag shape policy: the compressor and fan families evaluate
        # as fitted, the resistance row through the monotone form.
        class_rows = {
            "Gas Furnace": ((0.26, 0.9, -0.16), True),
            "Air Conditioner": ((1.6, -2.69, 2.09), True),
            "Electric Resistance Water Heater": ((1.0, 0.0, 0.0), False),
        }
        zip_dicts: dict[str, dict] = {}
        for name, (row, as_fitted) in class_rows.items():
            zip_dict = dw.equipment_zip(name)
            assert zip_dict is not None, f"{name} must expose a ZIP dict"
            assert zip_dict["real_power_zip_applies"] is True, (
                f"{name}: its class row scales the physics-computed draw at "
                f"the bus voltage; the published coefficients are real and "
                f"governing"
            )
            assert (zip_dict["zp"], zip_dict["ip"], zip_dict["pp"]) == row, (
                f"{name}: the published real coefficients must be its class "
                f"row {row}, not a structural (0, 0, 1) pin behind a "
                f"governing flag"
            )
            zip_dicts[name] = zip_dict

        # The shares' evaluations at exact pu voltages, hand-derived from
        # the pinned rows through the recorded deep-sag form.
        water_heater = zip_dicts["Electric Resistance Water Heater"]
        # The pure-impedance row (1, 0, 0) evaluates V^2: the element draws
        # 0.49 of rated at 0.7 pu (0.7 squared) and exactly a quarter at
        # 0.5 pu (P = V^2/R: the element is a resistor, so the heat
        # follows the draw).
        assert _deep_sag_real_multiplier(water_heater, 0.7, as_fitted=False) == 0.7 * 0.7
        assert _deep_sag_real_multiplier(water_heater, 0.5, as_fitted=False) == 0.25

        # The compressor row (1.6, -2.69, 2.09) evaluates as fitted:
        # 1.6 x 0.25 - 2.69 x 0.5 + 2.09 = 0.4 - 1.345 + 2.09 = 1.145 at
        # 0.5 pu, the measured static draw of the family: the draw rises
        # as the voltage falls.
        conditioner = zip_dicts["Air Conditioner"]
        assert _deep_sag_real_multiplier(conditioner, 0.5, as_fitted=True) == 1.145

        # The blower fan row (0.26, 0.9, -0.16) evaluates as fitted:
        # 0.26 x 0.25 + 0.9 x 0.5 - 0.16 = 0.065 + 0.45 - 0.16 = 0.355 at
        # 0.5 pu.
        furnace = zip_dicts["Gas Furnace"]
        assert _deep_sag_real_multiplier(furnace, 0.5, as_fitted=True) == 0.355

        # The constant-power row (0, 0, 1), the Ideal class's own, is the
        # one row whose evaluation is flat 1.0: exactly the signature a
        # re-pinned real side reads at every voltage, and exactly what the
        # governing rows above must not read at a sag.
        ideal = {"zp": 0.0, "ip": 0.0, "pp": 1.0, "v0": 1.0}
        for v in (0.5, 0.7, 1.0):
            assert _deep_sag_real_multiplier(ideal, v, as_fitted=False) == 1.0

        # The refrigerator's row (5.03, -8.48, 4.45) turns at
        # 8.48 / (2 x 5.03) = 0.8429423459 pu, inside the admitted band,
        # so the monotone form holds the evaluation at the quadratic's
        # stationary value 4.45 - 8.48 squared / (4 x 5.03)
        # = 0.8759244532803181 for every voltage below the turn: 0.5 pu
        # and 0.7 pu agree bitwise, where the raw polynomial answers
        # 1.4675000000000002 and 0.9786999999999999 (the stalled-draw
        # class the record does not model).
        refrigerator = dw.equipment_zip("Refrigerator")
        assert refrigerator is not None, "the refrigerator must expose a ZIP dict"
        assert (refrigerator["zp"], refrigerator["ip"], refrigerator["pp"]) == (
            5.03, -8.48, 4.45
        ), (
            "the refrigerator's published coefficients must be the Bokhari "
            "et al. 2014 row (5.03, -8.48, 4.45)"
        )
        held_05 = _deep_sag_real_multiplier(refrigerator, 0.5, as_fitted=False)
        held_07 = _deep_sag_real_multiplier(refrigerator, 0.7, as_fitted=False)
        assert (
            _f64_bits(held_05)
            == _f64_bits(held_07)
            == _f64_bits(0.8759244532803181)
        ), (
            "the refrigerator's held evaluation must be the stationary "
            "value 0.8759244532803181 bitwise at 0.5 and 0.7 pu, got "
            f"{held_05} and {held_07}"
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

        The aggregate is formed from the ZIP-governing mix, so the
        equipment it excluded (DER, voltage-invariant by construction)
        must be named: a caller must never have to guess whether the
        equipment was silently dropped.
        """
        dw = make_dwelling(duration_s=600, time_res_s=60)
        premise = dw.premise_zip()
        assert premise is not None
        governing = {e["name"] for e in premise["governing_equipment"]}
        for name in SCHEDULED_LOADS + PHYSICS_EQUIPMENT:
            assert name in governing, (
                f"{name} is a ZIP-governing load and must be listed as "
                f"governing with its weighting draw"
            )
        constant_power = set(premise["constant_power_equipment"])
        assert governing.isdisjoint(constant_power)

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
        weighted: list[tuple[dict[str, float | bool], float]] = []
        for entry in premise["governing_equipment"]:
            mean_power = entry["mean_power_w"]
            if mean_power is None or mean_power <= 0:
                continue
            equipment_zip = dw.equipment_zip(entry["name"])
            assert equipment_zip is not None, (
                f"{entry['name']}: governing equipment must publish its ZIP dict"
            )
            weighted.append((equipment_zip, float(mean_power)))
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
        from hares import Dwelling

        lines = Path(SCHEDULE).read_text().splitlines()
        column_name = lines[0].split(",")[3]
        parts = lines[1].split(",")
        parts[3] = "nan"
        lines[1] = ",".join(parts)
        bad_schedule = tmp_path / "schedule_with_nan.csv"
        bad_schedule.write_text("\n".join(lines) + "\n")

        with pytest.raises(Exception) as excinfo:
            Dwelling.from_hpxml(HPXML, str(bad_schedule), WEATHER, defaults=str(HARES_DEFAULTS), bldg_id=42, config=SimulationConfig(start_time="2019-01-01T00:00:00", duration=600, time_res=60,master_seed=0))
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
        user-supplied ``defaults``) is the reachable non-finite
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

        from hares import Dwelling

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
            Dwelling.from_hpxml(HPXML, SCHEDULE, WEATHER, defaults=str(defaults), bldg_id=42, config=SimulationConfig(start_time="2019-01-01T00:00:00", duration=600, time_res=60,master_seed=0))
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
        from hares import Dwelling

        with pytest.raises(Exception) as excinfo:
            Dwelling.from_hpxml(HPXML, SCHEDULE, WEATHER, defaults=str(HARES_DEFAULTS), bldg_id=42, overrides={"Indoor Lighting": {"zip": {"v0": 1e-320}}}, config=SimulationConfig(start_time="2019-01-01T00:00:00", duration=600, time_res=60,master_seed=0))
        message = str(excinfo.value).lower()
        assert "v0" in message, (
            f"the rejection must name the offending field, got: {excinfo.value}"
        )


class TestDeepSagSteppedResponse:
    def test_scheduled_load_steps_at_the_true_deep_sag_pu(self):
        """A dwelling stepped through a 0.7 pu sag answers with the ZIP
        polynomial evaluated at 0.7 pu, end to end through a real step.

        The per-equipment ZIP path has never clamped (the aggregate
        solver scale was the clamp site; its pure-P default never fired
        it in the assembled dwelling), so this witness is green across
        the clamp's removal: it pins that the deep-sag response is real
        at the dwelling surface, not only in the solver unit. Two
        identically-seeded dwellings, one at 0.7 pu and one at 1.0 pu:
        the raw scheduled draw is identical (voltage feeds no RNG), so
        the governing load's power is the raw draw times the ZIP terms
        at the actual voltage, bitwise.
        """
        dw_nominal = make_dwelling(duration_s=600, time_res_s=60)
        dw_sag = make_dwelling(duration_s=600, time_res_s=60)
        dw_nominal.set_grid_voltage(1.0)
        dw_sag.set_grid_voltage(0.7)

        v = 0.7
        # The Indoor Lighting row (Bokhari et al. 2014): the same
        # coefficients equipment_zip() publishes.
        zip_dict = dw_nominal.equipment_zip("Indoor Lighting")
        assert zip_dict is not None and zip_dict["real_power_zip_applies"]
        zp, ip, pp = zip_dict["zp"], zip_dict["ip"], zip_dict["pp"]
        mult_nominal = zp * 1.0 * 1.0 + ip * 1.0 + pp
        mult_sag = zp * v * v + ip * v + pp

        witness_steps = 0
        for _ in range(10):
            r_nominal = dw_nominal.step()
            dw_sag.step()
            tel_nominal = dw_nominal.telemetry()
            tel_sag = dw_sag.telemetry()
            names = list(tel_nominal.equipment()["names"])
            powers_nominal = list(tel_nominal.equipment()["power_w"])
            powers_sag = list(tel_sag.equipment()["power_w"])
            for name, p_nominal, p_sag in zip(names, powers_nominal, powers_sag):
                if name != "Indoor Lighting" or p_nominal == 0.0:
                    continue
                # The same raw draw (bit-identical across the two runs),
                # scaled by the same polynomial at the two voltages.
                raw = p_nominal / mult_nominal
                expected_sag = raw * mult_sag
                assert _f64_bits(p_sag) == _f64_bits(expected_sag), (
                    f"step {r_nominal['timestep_index']}: Indoor Lighting at "
                    f"{v} pu must be the raw draw {raw} W times the ZIP terms "
                    f"at {v} pu ({expected_sag} W), got {p_sag} W"
                )
                witness_steps += 1
        assert witness_steps > 0, "the fixture must draw lighting power inside the window"


class TestDeepSagShapes:
    """The brownout round's deep-sag shapes, witnessed at the dwelling.

    F1: the resistive element draws V²/R at the actual voltage (the
    resistance water heater's element drew 110.348223229 W, bitwise, at
    1.0 and 0.5 pu at the parent). F2: the motor families respond per
    their recorded rows (the ASHP's compressor drew 0.03 percent flatter
    at 0.5 pu against the row's +15.5 percent). F3: the refrigerator's
    row no longer turns at 0.843 pu (the draw fell 1.4675x → 0.9787x of
    nominal from 0.5 pu to 0.7 pu: the stalled-draw class the record
    does not model).
    """

    def _equipment_powers(self, dw):
        tel = dw.telemetry().equipment()
        return dict(zip(list(tel["names"]), list(tel["power_w"])))

    def test_resistance_water_heater_element_draws_v_squared_over_r(self):
        """F1's witness: the element's draw evaluates its pure-impedance
        row at the actual voltage. Every firing step draws exactly the
        rated watts × V²: a quarter of the watts at half voltage, where
        the parent drew 110.348223229 W, bitwise, at 1.0 and 0.5 pu. The
        two runs' firing steps are NOT paired: the sag run's tank heats at
        a quarter rate, the thermostats answer, and the two control
        trajectories legitimately diverge: the response of each firing
        step is what the row governs."""
        dw_nominal = make_dwelling(duration_s=86400, time_res_s=60)
        dw_sag = make_dwelling(duration_s=86400, time_res_s=60)
        dw_nominal.set_grid_voltage(1.0)
        dw_sag.set_grid_voltage(0.5)
        name = "Electric Resistance Water Heater"
        nominal_draws = set()
        sag_draws = set()
        for _ in range(1440):
            dw_nominal.step()
            dw_sag.step()
            p_nominal = self._equipment_powers(dw_nominal)
            p_sag = self._equipment_powers(dw_sag)
            if p_nominal.get(name, 0.0) > 0.0:
                nominal_draws.add(p_nominal[name])
            if p_sag.get(name, 0.0) > 0.0:
                sag_draws.add(p_sag[name])
        assert nominal_draws, "the fixture's water heater must fire (nominal)"
        assert sag_draws, "the fixture's water heater must fire (sag)"
        # P = V²/R through the pure-impedance row: every firing draw at
        # 0.5 pu is exactly a quarter of some nominal firing draw (×0.25
        # is exact in f64).
        expected = {d * 0.25 for d in nominal_draws}
        assert sag_draws == expected, (
            f"every 0.5 pu firing draw must be rated × 0.25; sag draws "
            f"{sorted(sag_draws)} against the nominal set × 0.25 "
            f"{sorted(expected)}"
        )

    def test_refrigerator_deep_sag_response_is_monotone_in_voltage(self):
        """F3's witness: the held turn. The refrigerator's evaluated
        response is the row's stationary value at every voltage below the
        0.843 pu turn, so the 0.5 pu and 0.7 pu draws agree bitwise and
        both sit below the 0.85 pu draw (the raw row answered 1.4675x at
        0.5 pu and 0.9787x at 0.7 pu: the draw rose as the voltage fell)."""
        dwellings = {}
        for v in (0.5, 0.7, 0.85, 1.0):
            dw = make_dwelling(duration_s=600, time_res_s=60)
            dw.set_grid_voltage(v)
            dwellings[v] = dw
        draws = {v: [] for v in dwellings}
        for _ in range(10):
            for v, dw in dwellings.items():
                dw.step()
                p = self._equipment_powers(dw)
                if p.get("Refrigerator", 0.0) != 0.0:
                    draws[v].append(p["Refrigerator"])
        assert draws[0.5], "the fixture's refrigerator must run inside the window"
        assert len(draws[0.5]) == len(draws[0.7]) == len(draws[0.85]) == len(draws[1.0]), (
            "identically-seeded dwellings must run the refrigerator in the "
            "same steps"
        )
        for a, b, c, d in zip(draws[0.5], draws[0.7], draws[0.85], draws[1.0]):
            assert _f64_bits(a) == _f64_bits(b), (
                "the refrigerator's draws at 0.5 and 0.7 pu must agree "
                "bitwise (the row held at its stationary point below the "
                f"0.843 pu turn): {a} vs {b}"
            )
            assert b < c < d, (
                "the refrigerator's response must be monotone increasing in "
                f"voltage: {b} (0.7 pu) then {c} (0.85 pu) then {d} (1.0 pu)"
            )

    def test_premise_zip_reports_the_assembled_deep_sag_response(self):
        """F10's witness: the stepped dwelling's premise aggregate reports
        the assembled deep-sag response of the moment's composition, equal
        to the sag run's own scaled-draw/raw-draw ratio. At the parent the
        expected-mean weights (the dryer, the range: loads off at the
        moment) understated the assembled response by up to 24.8 percent
        at 0.5 pu, and no deep-sag scale was reported at all."""
        dw_sag = make_dwelling(duration_s=600, time_res_s=60)
        dw_sag.set_grid_voltage(0.5)
        for _ in range(10):
            dw_sag.step()
        premise_sag = dw_sag.premise_zip()
        assert premise_sag is not None
        # A stepped dwelling weights by the moment's raw draws.
        weights = {
            e["name"]: e["mean_power_w"]
            for e in premise_sag["governing_equipment"]
        }
        stepped = {
            name: w for name, w in weights.items() if w is not None and w > 0
        }
        assert stepped, "the stepped fixture must carry positive moment weights"
        p_sag = self._equipment_powers(dw_sag)
        total_raw = sum(weights[n] for n in stepped)
        total_scaled = sum(p_sag[n] for n in stepped)
        assembled = total_scaled / total_raw
        reported = premise_sag["deep_sag_scale"]
        assert reported is not None, "the stepped premise must report its deep-sag scale"
        assert reported == pytest.approx(assembled, rel=1e-9), (
            f"the premise aggregate must report the assembled deep-sag "
            f"response {assembled} (the moment's composition), got {reported}"
        )
        # The deep-sag scale is not the raw evaluation of the published
        # coefficient row: the held refrigerator turn is unreachable from
        # a single quadratic.
        raw_row = (
            premise_sag["zp"] * 0.25 + premise_sag["ip"] * 0.5 + premise_sag["pp"]
        )
        assert raw_row != pytest.approx(reported, rel=1e-6), (
            "the reported scale must carry the rows' deep-sag forms (the "
            f"held turn), not the raw polynomial's {raw_row}"
        )
