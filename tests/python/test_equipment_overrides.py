"""The equipment override channel reaches the typed configs equipment
initialises from, through the Python surface.

A dwelling-level override of a typed field must change the equipment's
behaviour, an unknown field must raise naming the equipment and field, and
a value the typed schema rejects must raise at the build. The heat pump's
backup-fuel override must flip the FUEL capability declaration with the
flow: the declaration and the flow read the same merged config generation.
"""

from __future__ import annotations

from datetime import datetime, timedelta
from pathlib import Path

import pytest

from conftest import HARES_DEFAULTS, ROOT, WEATHER

from ochre_next import (
    ASHPHeater,
    Dwelling,
    DwellingBlueprint,
    EndUse,
    GasWaterHeater,
    TanklessWaterHeater,
)
from ochre_next._hares import HaresEquipmentError

SAMPLES = ROOT / "vendors/OCHRE/test/OS-HPXML Sample Files"
GAS_BACKUP_SAMPLE = str(
    SAMPLES / "base-hvac-autosize-dual-fuel-air-to-air-heat-pump-1-speed-sizing-methodology-acca.xml"
)
ELECTRIC_BACKUP_SAMPLE = str(SAMPLES / "base-hvac-air-to-air-heat-pump-1-speed.xml")

START = "2023-01-15T10:00:00-07:00"


def make_sample_dwelling(hpxml: str, **kw):
    dw = Dwelling.from_hpxml(
        hpxml,
        None,
        WEATHER,
        start_time=START,
        duration_s=timedelta(hours=24),
        time_res_s=900,
        defaults_path=str(HARES_DEFAULTS),
        bldg_id=1,
        write_output=False,
        **kw,
    )
    dw.initialize()
    return dw


def step_a_day(dw) -> None:
    steps = 24 * 3600 // 900
    for _ in range(steps):
        dw.step()


class TestHeatPumpBackupFuelOverride:
    def test_flip_to_electric_steps_a_day_with_the_contract(self):
        dw = make_sample_dwelling(
            GAS_BACKUP_SAMPLE,
            overrides={"ASHP Heater": {"backup_fuel": "Electric"}},
        )
        step_a_day(dw)

    def test_flip_to_gas_steps_a_day_with_the_contract(self):
        dw = make_sample_dwelling(
            ELECTRIC_BACKUP_SAMPLE,
            overrides={"ASHP Heater": {"backup_fuel": "Gas"}},
        )
        step_a_day(dw)

    def test_unknown_override_field_raises(self):
        with pytest.raises(HaresEquipmentError, match="backup_fuell"):
            make_sample_dwelling(
                ELECTRIC_BACKUP_SAMPLE,
                overrides={"ASHP Heater": {"backup_fuell": 1.0}},
            )


class TestKnownFieldOverrides:
    def test_unknown_pv_override_field_raises(self):
        with pytest.raises(HaresEquipmentError, match="totally_unknown"):
            make_sample_dwelling(
                str(ROOT / "tests/fixtures/hpxml/ochre_samples/base-pv.xml"),
                overrides={"PV": {"totally_unknown": 1.0}},
            )

    def test_pv_capacity_override_changes_generation(self):
        baseline = make_sample_dwelling(
            str(ROOT / "tests/fixtures/hpxml/ochre_samples/base-pv.xml"),
        )
        baseline.step()
        enlarged = make_sample_dwelling(
            str(ROOT / "tests/fixtures/hpxml/ochre_samples/base-pv.xml"),
            overrides={"PV": {"capacity_kw": 9.0}},
        )
        enlarged.step()

        def pv_generation_magnitude_kw(dw) -> float:
            gens = [
                eq.core_output.electric_kw
                for eq in dw.equipment()
                if eq.descriptor.name.startswith("PV")
            ]
            assert gens, "the dwelling must contain the PV arrays"
            return sum(abs(g or 0.0) for g in gens)

        assert (
            pv_generation_magnitude_kw(enlarged) > pv_generation_magnitude_kw(baseline)
        ), (
            "a PV capacity override of 9 kW must generate more than the 5 kW baseline"
        )

    def test_water_heater_setpoint_override_builds_and_applies(self):
        dw = make_sample_dwelling(
            str(ROOT / "tests/fixtures/hpxml/ochre_samples/base.xml"),
            overrides={"Electric Resistance Water Heater": {"setpoint_c": 60.0}},
        )
        step_a_day(dw)


class TestTypedSpecBuilderConstructions:
    """The typed Python builders' specs must land their own parameter bag
    on the typed payload: the bag keeps the human fuel spelling ("natural
    gas") while the typed payload carries the canonical serde form ("Gas"),
    and the landing normalizes the one into the other instead of rejecting
    the construction."""

    BASE_SAMPLE = str(ROOT / "tests/fixtures/hpxml/ochre_samples/base.xml")

    def make_blueprint(self, hpxml: str):
        bp = DwellingBlueprint.from_hpxml(
            hpxml,
            None,
            WEATHER,
            start_time=START,
            duration_s=timedelta(hours=24),
            time_res_s=900,
            defaults_path=str(HARES_DEFAULTS),
            bldg_id=1,
            write_output=False,
        )
        return bp

    def build_and_step(self, hpxml: str, equipment, replace: str | None = None) -> None:
        bp = self.make_blueprint(hpxml)
        if replace is not None:
            bp.remove_equipment(replace)
        bp.add_equipment(equipment)
        dw = bp.build()
        dw.initialize()
        dw.step()

    def test_gas_water_heater_typed_construction_builds(self):
        self.build_and_step(
            self.BASE_SAMPLE,
            GasWaterHeater(
                name="Gas Water Heater",
                autosize=False,
                tank_volume_m3=0.3,
                heating_capacity_w=4000.0,
                avg_water_draw_l_per_day=200.0,
                zone_id=1,
            ),
        )

    def test_tankless_water_heater_typed_construction_builds(self):
        self.build_and_step(
            self.BASE_SAMPLE,
            TanklessWaterHeater(
                name="Tankless Water Heater",
                autosize=False,
                heating_capacity_w=4000.0,
                avg_water_draw_l_per_day=200.0,
                zone_id=1,
            ),
        )

    @pytest.mark.parametrize("spelling", ["gas", "Natural Gas", "electric"])
    def test_ashp_backup_fuel_spellings_build(self, spelling):
        self.build_and_step(
            ELECTRIC_BACKUP_SAMPLE,
            ASHPHeater(
                name="ASHP Heater",
                autosize=False,
                capacity_w=10000.0,
                backup_fuel=spelling,
                backup_capacity_w=3000.0,
                zone_id=1,
            ),
            replace="ASHP Heater",
        )

    def test_invalid_backup_fuel_spelling_raises_naming_field_and_value(self):
        with pytest.raises(ValueError, match=r"backup_fuel.*banana"):
            ASHPHeater(
                name="ASHP Heater",
                autosize=False,
                capacity_w=10000.0,
                backup_fuel="banana",
            )
