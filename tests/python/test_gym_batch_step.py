"""Tests for the Rust ``batch_step`` RL entrypoint (py_gym.rs).

Lives outside test_gym_env.py because ``batch_step`` needs neither numpy nor
gymnasium — these tests must run even where those optional deps are absent.
"""

from __future__ import annotations

import pytest
from conftest import make_dwelling

_OBS_FIELDS = ["total_power_kw", "outdoor_temp"]
_SETPOINT_BOUNDS = [(-50.0, 80.0)]


def test_batch_step_clips_a_deadband_to_the_bounds_it_is_given():
    """The environment resolves the furnace's band range once; a 30 C band
    clipped to it reaches the furnace as a valid band, so nothing is
    rejected."""
    from ochre_next._hares import batch_step

    dwelling = make_dwelling(duration_s=600)
    band = dwelling.thermostat_band_range("Gas Furnace")
    assert band is not None
    results = batch_step(
        [dwelling],
        [[30.0, 21.0]],
        _OBS_FIELDS,
        [("Gas Furnace", "deadband_c"), ("Gas Furnace", "heat_c")],
        {"Gas Furnace": "ThermalSetpoint"},
        [band, (-50.0, 80.0)],
    )
    assert results[0]["info"]["warning_count"] == 0.0


def test_batch_step_refuses_bounds_that_do_not_match_the_layout():
    from ochre_next._hares import batch_step

    dwelling = make_dwelling(duration_s=600)
    with pytest.raises(ValueError, match="action_bounds length"):
        batch_step(
            [dwelling],
            [[21.0]],
            _OBS_FIELDS,
            [("Gas Furnace", "heat_c")],
            {"Gas Furnace": "ThermalSetpoint"},
            [],
        )


@pytest.mark.parametrize(
    ("pair", "reason"),
    [
        ((30.0, 10.0), "low above high"),
        ((float("nan"), 80.0), "not finite"),
        ((-50.0, float("inf")), "not finite"),
    ],
)
def test_batch_step_refuses_a_bad_bounds_pair(pair, reason):
    from ochre_next._hares import batch_step

    dwelling = make_dwelling(duration_s=600)
    with pytest.raises(ValueError, match=reason):
        batch_step(
            [dwelling],
            [[21.0]],
            _OBS_FIELDS,
            [("Gas Furnace", "heat_c")],
            {"Gas Furnace": "ThermalSetpoint"},
            [pair],
        )


def test_batch_step_refuses_deadband_bounds_wider_than_a_dwellings_band():
    """The bounds are per call: a storage-tank band range on a furnace's
    deadband column is wider than the furnace holds, so it is refused."""
    from ochre_next._hares import MAX_TANK_THERMOSTAT_BAND_C, batch_step

    dwelling = make_dwelling(duration_s=600)
    band = dwelling.thermostat_band_range("Gas Furnace")
    assert band is not None
    low, _ = band
    with pytest.raises(ValueError, match="band range of its thermostat"):
        batch_step(
            [dwelling],
            [[1.0, 21.0]],
            _OBS_FIELDS,
            [("Gas Furnace", "deadband_c"), ("Gas Furnace", "heat_c")],
            {"Gas Furnace": "ThermalSetpoint"},
            [(low, MAX_TANK_THERMOSTAT_BAND_C), (-50.0, 80.0)],
        )


def test_batch_step_refusal_reaches_no_dwelling():
    """A refused action in the second dwelling leaves the first untouched:
    its out-of-range setpoint is never queued, so its next step rejects
    nothing."""
    from ochre_next._hares import batch_step

    first, second = make_dwelling(duration_s=600), make_dwelling(duration_s=600)
    with pytest.raises(ValueError, match="not finite"):
        batch_step(
            [first, second],
            [[70.0], [float("nan")]],
            _OBS_FIELDS,
            [("Gas Furnace", "cool_c")],
            {"Gas Furnace": "ThermalSetpoint"},
            _SETPOINT_BOUNDS,
        )
    first.step()
    assert not any("control apply failed" in w for w in first.take_warnings())


def test_batch_step_with_no_actions_steps_every_dwelling():
    from ochre_next._hares import batch_step

    results = batch_step([make_dwelling(duration_s=600)], [], _OBS_FIELDS, [], {}, [])
    assert len(results) == 1


def test_batch_step_info_reports_zero_warnings_on_clean_step():
    """A clean step surfaces warning_count == 0 and omits the messages list."""
    from ochre_next._hares import batch_step

    dwelling = make_dwelling(duration_s=600)
    # The fixture's construction notices (base.xml gives no ShieldingofHome,
    # which takes OS-HPXML's default) are not a step's warnings.
    dwelling.take_warnings()
    results = batch_step(
        [dwelling],
        [[21.0]],
        _OBS_FIELDS,
        [("Gas Furnace", "heat_c")],
        {"Gas Furnace": "ThermalSetpoint"},
        _SETPOINT_BOUNDS,
    )
    info = results[0]["info"]
    assert info["warning_count"] == 0.0
    assert "warnings" not in info


def test_batch_step_surfaces_rejected_control_signal_in_info():
    """A control signal that passes queue-time capability validation but is
    rejected at dispatch time (numeric bounds) must show up in step info so
    RL loops see it without polling take_warnings()."""
    from ochre_next._hares import batch_step

    dwelling = make_dwelling(duration_s=600)
    # cooling_setpoint_c = 70 is inside the gym clip range (-50, 80) but
    # outside ControlSignal numeric bounds ([0, 60] degC), so it passes the
    # queue-time capability check and is rejected by Equipment::apply_control
    # during dispatch -- the silent-warning path this test guards.
    results = batch_step(
        [dwelling],
        [[70.0]],
        _OBS_FIELDS,
        [("Gas Furnace", "cool_c")],
        {"Gas Furnace": "ThermalSetpoint"},
        _SETPOINT_BOUNDS,
    )
    info = results[0]["info"]
    assert info["warning_count"] >= 1.0
    assert any("control apply failed" in w for w in info["warnings"])
    assert any("Gas Furnace" in w for w in info["warnings"])
    # The messages were drained into info -- a subsequent poll is empty.
    assert dwelling.take_warnings() == []
