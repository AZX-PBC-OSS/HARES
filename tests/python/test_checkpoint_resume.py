"""OR-10: the checkpoint carries billing, metrics, reports, health and warnings.

A PV+battery dwelling with the PG&E E-TOU-C tariff runs across the July to
August billing boundary; checkpointed mid-period via ``save_state``, restored
into a fresh dwelling via ``load_state``, and run to the end, it finishes to
the uninterrupted run's metrics, billing summaries, health and warnings.
"""

from datetime import datetime
from pathlib import Path

import pytest
from hares import SimulationConfig

HARES_ROOT = Path(__file__).resolve().parent.parent.parent
EXAMPLES = HARES_ROOT / "data" / "examples"
FIXTURES = HARES_ROOT / "tests" / "fixtures"
DEFAULTS = HARES_ROOT / "python" / "hares" / "defaults"

HPXML = EXAMPLES / "pv_battery_example.xml"
SCHEDULE = EXAMPLES / "BEopt_example_schedule.csv"
WEATHER = EXAMPLES / "USA_CO_Denver.Intl.AP.725650_TMY3.epw"
URDB_TOU = FIXTURES / "urdb" / "pge_e_tou_c.json"

TIME_RES_S = 900
SIM_DURATION_S = 4 * 86400
SIM_START = datetime(2019, 7, 30, 0, 0)
# The July period closes at the step whose end reaches 2019-08-01T00:00
# (step 192); the checkpoint lands mid-August-period, after the close.
CHECKPOINT_STEP = 240

BMS_MODE = (
    '{"SelfConsumption": {"min_soc": 0.1, "max_soc": 1.0, '
    '"solar_only_charging": false, "surplus_deadband_kw": 0.0}}'
)
BATTERY_OVERRIDES = {"Battery": {"bms_mode": BMS_MODE}}


def _build_dwelling():
    from hares import Destination, Dwelling, ElectricTariff, ReportSpec, Selection

    dw = Dwelling.from_hpxml(
        str(HPXML),
        str(SCHEDULE),
        str(WEATHER),
        defaults=str(DEFAULTS),
        bldg_id=1,
        overrides=BATTERY_OVERRIDES,
        config=SimulationConfig(
            start_time="2019-07-30T00:00:00",
            duration=SIM_DURATION_S,
            time_res=TIME_RES_S,
            master_seed=42,
            metrics=True,
            reports=[
                ReportSpec("frame", Selection.verbosity(3), Destination.memory())
            ],
        ),
    )
    dw.initialize()
    dw.set_electric_tariff(ElectricTariff.from_urdb_json(str(URDB_TOU)))
    return dw


def _summaries(dw):
    return [s.to_dict() for s in dw.billing_summaries()]


@pytest.fixture(scope="module")
def runs():
    continuous = _build_dwelling()
    for _ in range(CHECKPOINT_STEP):
        continuous.step()
    state = bytes(continuous.save_state())
    continuous_result = continuous.simulate()

    interrupted = _build_dwelling()
    for _ in range(CHECKPOINT_STEP):
        interrupted.step()
    interrupted.load_state(state)
    resumed_result = interrupted.simulate()
    return continuous, continuous_result, interrupted, resumed_result


class TestCheckpointResume:
    def test_metrics_match(self, runs):
        _continuous, continuous_result, _interrupted, resumed_result = runs
        assert resumed_result.metrics is not None
        assert continuous_result.metrics is not None
        a = continuous_result.metrics
        b = resumed_result.metrics
        assert a.total_energy_kwh.net_energy_kwh == b.total_energy_kwh.net_energy_kwh
        assert (
            a.total_energy_kwh.gross_consumption_kwh
            == b.total_energy_kwh.gross_consumption_kwh
        )
        assert a.comfort_hours == b.comfort_hours
        assert a.unmet_load_hours == b.unmet_load_hours
        assert a.nan_step_count == b.nan_step_count
        assert a.simulation_duration_hours == b.simulation_duration_hours

    def test_billing_summaries_match(self, runs):
        continuous, _continuous_result, interrupted, _resumed_result = runs
        a = _summaries(continuous)
        b = _summaries(interrupted)
        assert a, "the uninterrupted run holds the closed July period"
        assert a == b

    def test_health_and_warnings_match(self, runs):
        _continuous, continuous_result, _interrupted, resumed_result = runs
        a = [(w.source, w.message, w.step_index) for w in continuous_result.warnings]
        b = [(w.source, w.message, w.step_index) for w in resumed_result.warnings]
        assert a == b
        assert continuous_result.warnings_dropped == resumed_result.warnings_dropped
        assert (
            continuous_result.health.port_rollbacks == resumed_result.health.port_rollbacks
        )
        assert (
            continuous_result.health.rejected_control_signals
            == resumed_result.health.rejected_control_signals
        )

    def test_frames_match(self, runs):
        _continuous, continuous_result, _interrupted, resumed_result = runs
        a = continuous_result.reports[0]
        b = resumed_result.reports[0]
        assert a.rows == b.rows
        assert a.batches_delivered == b.batches_delivered
