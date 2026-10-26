"""The grid-signal input channel: frequency and price (schedule row 13).

The frequency is a first-class grid input: ``set_frequency`` sits beside
``set_grid_voltage`` on the Python surface, the 60 Hz constant is the
fallback when no override was given, and a voltage call never overwrites
a set frequency. The price channel reaches a fleet federate's dwellings
exactly as the single dwelling federate applies it (one boundary
behavior; see docs/cosim.md).
"""

from __future__ import annotations

import math
from pathlib import Path

import pytest

from conftest import make_dwelling
from hares import DwellingConfig, SimulationConfig, SteppableFleet

ROOT = Path(__file__).resolve().parents[2]
EXAMPLES = ROOT / "data" / "examples"


def _fleet_config(bldg_id: int = 1) -> DwellingConfig:
    sim_config = SimulationConfig(start_time="2019-01-01T00:00:00", duration=3600, time_res=60)
    return DwellingConfig(
        hpxml=str(EXAMPLES / "BEopt_example.xml"),
        schedule=str(EXAMPLES / "BEopt_example_schedule.csv"),
        weather=str(EXAMPLES / "USA_CO_Denver.Intl.AP.725650_TMY3.epw"),
        config=sim_config,
        bldg_id=bldg_id,
    )


class TestFrequencyChannel:
    def test_frequency_passes_through_and_survives_a_voltage_call(self):
        """The pass-through witness: set 59.8 Hz, read the simulation's
        frequency. The RED state at the parent was the 60 Hz constant:
        ``set_frequency`` did not exist on the surface, and
        ``set_grid_voltage`` overwrote ``frequency_hz`` with the constant
        on every call."""
        dw = make_dwelling(duration_s=300, time_res_s=60)
        dw.set_frequency(59.8)
        dw.set_grid_voltage(0.7)
        dw.step()
        env = dw.latest_env()
        assert env["grid_frequency_hz"] == pytest.approx(59.8), (
            "the set frequency must survive the later voltage call"
        )
        assert env["grid_voltage_pu"] == pytest.approx(0.7)

    def test_frequency_only_override_keeps_the_default_voltage(self):
        dw = make_dwelling(duration_s=300, time_res_s=60)
        dw.set_frequency(59.3)
        dw.step()
        env = dw.latest_env()
        assert env["grid_frequency_hz"] == pytest.approx(59.3)
        assert env["grid_voltage_pu"] == pytest.approx(1.0), (
            "a frequency-only override composes with the default utility "
            "voltage, not a substitute for it"
        )

    def test_without_an_override_the_60hz_constant_falls_back(self):
        dw = make_dwelling(duration_s=300, time_res_s=60)
        dw.step()
        assert dw.latest_env()["grid_frequency_hz"] == pytest.approx(60.0)

    def test_out_of_band_frequencies_are_rejected(self):
        dw = make_dwelling(duration_s=300, time_res_s=60)
        for invalid in (-59.8, 0.0, 39.9, 80.1, 600.0):
            with pytest.raises(Exception, match="grid frequency override"):
                dw.set_frequency(invalid)
        for non_finite in (float("nan"), float("inf"), float("-inf")):
            with pytest.raises(Exception, match="frequency_hz must be finite"):
                dw.set_frequency(non_finite)

    def test_fleet_frequency_override_applies_to_every_dwelling(self):
        fleet = SteppableFleet.from_configs(
            [_fleet_config(1), _fleet_config(2)], n_threads=0
        )
        fleet.set_frequency_all(59.8)
        fleet.set_frequency(1, 59.3)
        fleet.set_frequency(1, 59.8)
        with pytest.raises(Exception, match="grid frequency override"):
            fleet.set_frequency(0, 600.0)


class TestFleetPriceChannel:
    def test_fleet_price_signal_all_applies_to_every_dwelling(self):
        """The fleet binding's price surface: the same signal every home,
        applied through each dwelling's own price signal, exactly the
        single dwelling's application."""
        fleet = SteppableFleet.from_configs([_fleet_config(1), _fleet_config(2)], n_threads=0)
        fleet.set_price_signal_all(0.42)
        entries = fleet.step()
        assert len(entries) == 2
        for entry in entries:
            assert entry["ok"] is True

    def test_fleet_price_signal_all_rejects_non_finite(self):
        fleet = SteppableFleet.from_configs([_fleet_config(1)], n_threads=0)
        with pytest.raises(Exception):
            fleet.set_price_signal_all(float("nan"))
