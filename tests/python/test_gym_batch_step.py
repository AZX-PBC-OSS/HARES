"""Tests for the Rust ``batch_step`` RL entrypoint (py_gym.rs).

Lives outside test_gym_env.py because ``batch_step`` needs neither numpy nor
gymnasium — these tests must run even where those optional deps are absent.
"""

from __future__ import annotations

from conftest import make_dwelling

_OBS_FIELDS = ["total_power_kw", "outdoor_temp"]


def test_batch_step_info_reports_zero_warnings_on_clean_step():
    """A clean step surfaces warning_count == 0 and omits the messages list."""
    from ochre_next._hares import batch_step

    dwelling = make_dwelling(duration_s=600)
    results = batch_step(
        [dwelling],
        [[21.0]],
        _OBS_FIELDS,
        [("Gas Furnace", "heat_c")],
        {"Gas Furnace": "ThermalSetpoint"},
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
    )
    info = results[0]["info"]
    assert info["warning_count"] >= 1.0
    assert any("control apply failed" in w for w in info["warnings"])
    assert any("Gas Furnace" in w for w in info["warnings"])
    # The messages were drained into info -- a subsequent poll is empty.
    assert dwelling.take_warnings() == []
