"""The island's grid-interchange publication (schedule row 14).

The physically correct grid-facing publication: during an island the
service connection carries no power (the breaker is open), so the
published grid interchange is exactly 0.0 while the premise balance
continues and the imbalance surfaces as the island's internal response
(``island_unserved_w`` / ``island_excess_w``). The RED state at the
parent: the interchange fields did not exist, and the only published
power (``total_power_w``, the premise balance) read the island's
internal imbalance, a phantom load the feeder federate would read as
demand exactly during the disturbances the study exists to examine.
"""

from __future__ import annotations

import pytest
from conftest import make_dwelling
from hares import Battery


def _battery_dwelling():
    dw = make_dwelling(duration_s=600, time_res_s=60)
    dw.add_battery(
        Battery("Battery", capacity_j=10.0 * 3.6e6, max_charge_w=5000.0, max_discharge_w=500.0, initial_soc=0.6)
    )
    return dw


class TestIslandInterchangePublication:
    def test_islanded_interchange_is_exactly_zero_with_the_internal_response(self):
        dw = _battery_dwelling()
        dw.step()  # nominal, grid-connected
        dw.set_grid_voltage(0.0)  # the outage: island formation
        dw.step()
        t = dw.telemetry()
        assert t.islanded, "the battery-backed home must island on the outage"
        assert t.bus_voltage_pu == pytest.approx(1.0), "the island bus is nominal"
        assert t.grid_interchange_w == 0.0, (
            "the published grid interchange during an island is exactly zero: "
            "the breaker is open"
        )
        # The scenario's discharge limit (500 W) cannot cover the premise's
        # load, so the internal imbalance is real and non-zero: it surfaces
        # as the island's unserved load, never as a phantom grid import.
        assert t.island_unserved_w > 0.0, (
            "the scenario's imbalance is real (the battery's discharge limit "
            "cannot cover the load), so the witness is non-vacuous"
        )
        assert t.total_power_w == t.island_unserved_w, (
            "the premise balance reads the island's own imbalance, not a "
            "grid quantity"
        )

        dw.set_grid_voltage(1.0)  # restoration
        dw.step()
        t = dw.telemetry()
        assert not t.islanded
        assert t.grid_interchange_w == t.total_power_w, (
            "grid-connected, the interchange is the meter's net flow"
        )

    def test_island_channels_agree_with_the_environment_view(self):
        dw = _battery_dwelling()
        dw.step()
        dw.set_grid_voltage(0.0)
        dw.step()
        env = dw.latest_env()
        t = dw.telemetry()
        assert env["grid_islanded"] == t.islanded
        assert env["grid_bus_voltage_pu"] == t.bus_voltage_pu
        assert env["island_unserved_w"] == t.island_unserved_w
        assert env["island_excess_w"] == t.island_excess_w
