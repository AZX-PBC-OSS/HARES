"""Tests for the Rust ``batch_step`` RL entrypoint (py_gym.rs).

Lives outside test_gym_env.py because ``batch_step`` needs neither numpy nor
gymnasium — these tests must run even where those optional deps are absent.
The batch's observations are read into the caller's own buffer through its
buffer protocol, so every call here passes a stdlib ``array.array('d')``
buffer: the same protocol a numpy array exposes, without numpy. The actions
are the same channel in reverse: a flat ``array.array('d')`` of one
``(n_envs, n_actions)`` C-contiguous float64 row set, or ``None`` for no
actions.
"""

from __future__ import annotations

from array import array

import pytest
from conftest import make_dwelling

_OBS_FIELDS = ["total_power_w", "outdoor_temp"]
_SETPOINT_BOUNDS = [(-50.0, 80.0)]


def _zero_buffer(n_rows: int, width: int) -> array:
    """A zeroed float64 buffer for ``n_rows`` environments of ``width``
    observation fields: any writable C-contiguous float64 buffer protocol
    view is accepted, a numpy array among them."""
    return array("d", bytes(8 * width * n_rows))


def _plan(dwelling, fields=_OBS_FIELDS):
    return dwelling.observation_plan(fields)


def test_batch_step_clips_a_deadband_to_the_bounds_it_is_given():
    """The environment resolves the furnace's band range once; a 30 C band
    clipped to it reaches the furnace as a valid band, so nothing is
    rejected."""
    from hares._hares import batch_step

    dwelling = make_dwelling(duration_s=600)
    # The fixture's construction notices (base.xml gives no ShieldingofHome,
    # which takes OS-HPXML's default) are not a step's warnings.
    dwelling.take_warnings()
    band = dwelling.thermostat_band_range("Gas Furnace")
    assert band is not None
    results = batch_step(
        [dwelling],
        array("d", [30.0, 21.0]),
        [_plan(dwelling)],
        [("Gas Furnace", "deadband_c"), ("Gas Furnace", "heat_c")],
        {"Gas Furnace": "ThermalSetpoint"},
        [band, (-50.0, 80.0)],
        _zero_buffer(1, len(_OBS_FIELDS)),
    )
    assert results[0]["info"]["warning_count"] == 0


def test_batch_step_refuses_bounds_that_do_not_match_the_layout():
    from hares._hares import batch_step

    dwelling = make_dwelling(duration_s=600)
    with pytest.raises(ValueError, match="action_bounds length"):
        batch_step(
            [dwelling],
            array("d", [21.0]),
            [_plan(dwelling)],
            [("Gas Furnace", "heat_c")],
            {"Gas Furnace": "ThermalSetpoint"},
            [],
            _zero_buffer(1, len(_OBS_FIELDS)),
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
    from hares._hares import batch_step

    dwelling = make_dwelling(duration_s=600)
    with pytest.raises(ValueError, match=reason):
        batch_step(
            [dwelling],
            array("d", [21.0]),
            [_plan(dwelling)],
            [("Gas Furnace", "heat_c")],
            {"Gas Furnace": "ThermalSetpoint"},
            [pair],
            _zero_buffer(1, len(_OBS_FIELDS)),
        )


def test_batch_step_refuses_deadband_bounds_wider_than_a_dwellings_band():
    """The bounds are per call: a storage-tank band range on a furnace's
    deadband column is wider than the furnace holds, so it is refused."""
    from hares._hares import MAX_TANK_THERMOSTAT_BAND_C, batch_step

    dwelling = make_dwelling(duration_s=600)
    band = dwelling.thermostat_band_range("Gas Furnace")
    assert band is not None
    low, _ = band
    with pytest.raises(ValueError, match="band range of its thermostat"):
        batch_step(
            [dwelling],
            array("d", [1.0, 21.0]),
            [_plan(dwelling)],
            [("Gas Furnace", "deadband_c"), ("Gas Furnace", "heat_c")],
            {"Gas Furnace": "ThermalSetpoint"},
            [(low, MAX_TANK_THERMOSTAT_BAND_C), (-50.0, 80.0)],
            _zero_buffer(1, len(_OBS_FIELDS)),
        )


def test_clip_action_refuses_a_non_finite_action():
    """The single-dwelling clip path refuses a non-finite action the way the
    batch path does: a rejected call that records nothing, not a clamp."""
    from hares._hares import clip_action

    dwelling = make_dwelling(duration_s=600)
    with pytest.raises(ValueError, match="not finite"):
        clip_action(dwelling, [float("nan")], [("Gas Furnace", "heat_c")])
    with pytest.raises(ValueError, match="not finite"):
        clip_action(dwelling, [float("inf")], [("Gas Furnace", "heat_c")])


def test_batch_step_refusal_reaches_no_dwelling():
    """A refused action in the second dwelling leaves the first untouched:
    its out-of-range setpoint is never queued, so its next step rejects
    nothing."""
    from hares._hares import batch_step

    first, second = make_dwelling(duration_s=600), make_dwelling(duration_s=600)
    with pytest.raises(ValueError, match="not finite"):
        batch_step(
            [first, second],
            array("d", [70.0, float("nan")]),
            [_plan(first), _plan(second)],
            [("Gas Furnace", "cool_c")],
            {"Gas Furnace": "ThermalSetpoint"},
            _SETPOINT_BOUNDS,
            _zero_buffer(2, len(_OBS_FIELDS)),
        )
    first.step()
    assert not any("control apply failed" in w for w in first.take_warnings())


def _furnace_heating_setpoint(dwelling) -> float:
    furnace = next(eq for eq in dwelling.equipment() if eq.name == "Gas Furnace")
    return furnace.telemetry()["heating_setpoint_c"]


def test_batch_step_refuses_a_missing_unit_before_any_dwelling_changes():
    """The second home has no furnace, so its action is refused, and the
    first home's furnace never takes the 27 C setpoint: after a step it
    heats to the same setpoint as a home that was never sent anything."""
    from hares._hares import batch_step

    first, second, untouched = (make_dwelling(duration_s=600) for _ in range(3))
    second.remove_equipment("Gas Furnace")
    with pytest.raises(ValueError, match="not found"):
        batch_step(
            [first, second],
            array("d", [27.0, 27.0]),
            [_plan(first), _plan(second)],
            [("Gas Furnace", "heat_c")],
            {"Gas Furnace": "ThermalSetpoint"},
            _SETPOINT_BOUNDS,
            _zero_buffer(2, len(_OBS_FIELDS)),
        )
    first.step()
    untouched.step()
    assert _furnace_heating_setpoint(first) == _furnace_heating_setpoint(untouched)
    assert _furnace_heating_setpoint(first) != 27.0


def test_batch_step_with_no_actions_steps_every_dwelling():
    from hares._hares import batch_step

    dwelling = make_dwelling(duration_s=600)
    results = batch_step(
        [dwelling], None, [_plan(dwelling)], [], {}, [], _zero_buffer(1, len(_OBS_FIELDS))
    )
    assert len(results) == 1


def test_a_failed_row_carries_the_last_observation():
    """A failed step's row carries the dwelling's last observation, the same
    width as a living row's: the batch stays rectangular and the failed
    step's reward contract stays reachable in a multi-dwelling fleet."""
    from hares._hares import batch_step

    short, living = make_dwelling(duration_s=120), make_dwelling(duration_s=600)
    plans = [_plan(short), _plan(living)]
    buffer = _zero_buffer(2, len(_OBS_FIELDS))
    args = (
        plans,
        [("Gas Furnace", "heat_c")],
        {"Gas Furnace": "ThermalSetpoint"},
        _SETPOINT_BOUNDS,
    )
    actions = array("d", [21.0, 21.0])

    # The short home's sim holds two steps of 60 s; both homes step them.
    batch_step([short, living], actions, *args, buffer, True)
    second = batch_step([short, living], actions, *args, buffer, True)
    assert second[0]["step"] is not None
    last_obs = bytes(buffer)

    # The third step fails the short home (past its end) while the living
    # home steps on.
    third = batch_step([short, living], actions, *args, buffer, True)
    failed, ok = third
    assert failed["step"] is None
    assert failed["terminated"] is True
    assert ok["step"] is not None
    assert ok["terminated"] is False
    width = len(_OBS_FIELDS)
    failed_row = buffer[:width]
    second_row = array("d", last_obs[: 8 * width])
    assert failed_row == second_row, (
        "the failed row's observation must be the dwelling's last "
        "observation: the failed step changed nothing"
    )


def test_batch_step_rows_carry_no_reward_key():
    """A row carries the step dict when it is asked for, never a
    Rust-computed reward: the reward is the environment's ``reward_fn``
    decision, on one contract for both step paths."""
    from hares._hares import batch_step

    dwelling = make_dwelling(duration_s=600)
    dwelling.take_warnings()
    results = batch_step(
        [dwelling],
        array("d", [21.0]),
        [_plan(dwelling)],
        [("Gas Furnace", "heat_c")],
        {"Gas Furnace": "ThermalSetpoint"},
        _SETPOINT_BOUNDS,
        _zero_buffer(1, len(_OBS_FIELDS)),
        True,
    )
    row = results[0]
    assert "reward" not in row
    assert "step" in row


def test_batch_step_builds_no_step_dict_unless_asked():
    """The default batch step pays for no dicts: the row carries
    ``terminated``, ``truncated`` and ``info`` and no ``step`` dict, and
    ``include_step_dict=True`` builds it. The info's ``warning_count`` is
    an int on both paths through the row."""
    from hares._hares import batch_step

    dwelling = make_dwelling(duration_s=600)
    dwelling.take_warnings()
    unasked = batch_step(
        [dwelling],
        array("d", [21.0]),
        [_plan(dwelling)],
        [("Gas Furnace", "heat_c")],
        {"Gas Furnace": "ThermalSetpoint"},
        _SETPOINT_BOUNDS,
        _zero_buffer(1, len(_OBS_FIELDS)),
    )
    row = unasked[0]
    assert "step" not in row
    assert row["terminated"] is False
    assert row["truncated"] is False
    assert isinstance(row["info"]["warning_count"], int)
    assert row["info"]["warning_count"] == 0

    asked = batch_step(
        [dwelling],
        array("d", [21.0]),
        [_plan(dwelling)],
        [("Gas Furnace", "heat_c")],
        {"Gas Furnace": "ThermalSetpoint"},
        _SETPOINT_BOUNDS,
        _zero_buffer(1, len(_OBS_FIELDS)),
        True,
    )
    assert asked[0]["step"] is not None


def test_batch_step_dict_content_matches_the_single_step_dict():
    """The content pin: when the batch is asked for the step dict, the
    row's dict is the single-step path's own dict. Both dwellings take the
    same action and step once from the same fixture state; the pin holds
    the keys, the values and the key order identical, which the one shared
    conversion both paths build through gives them."""
    from hares._hares import batch_step
    from hares.rl.gym_env import build_control_signal

    single, batched = make_dwelling(duration_s=600), make_dwelling(duration_s=600)
    signal = build_control_signal("ThermalSetpoint", {"heat_c": 21.0})
    single.apply_control("Gas Furnace", signal)
    single_dict = single.step()

    results = batch_step(
        [batched],
        array("d", [21.0]),
        [_plan(batched)],
        [("Gas Furnace", "heat_c")],
        {"Gas Furnace": "ThermalSetpoint"},
        _SETPOINT_BOUNDS,
        _zero_buffer(1, len(_OBS_FIELDS)),
        True,
    )
    batch_dict = results[0]["step"]

    assert list(single_dict.keys()) == list(batch_dict.keys()), (
        "the same keys in the same order"
    )
    assert list(single_dict.items()) == list(batch_dict.items()), (
        "the same key-value pairs in the same order"
    )
    assert repr(single_dict) == repr(batch_dict), (
        "the step dicts are byte-identical: keys, values and order"
    )


def test_batch_step_info_reports_zero_warnings_on_clean_step():
    """A clean step surfaces warning_count == 0 and omits the messages list."""
    from hares._hares import batch_step

    dwelling = make_dwelling(duration_s=600)
    # The fixture's construction notices (base.xml gives no ShieldingofHome,
    # which takes OS-HPXML's default) are not a step's warnings.
    dwelling.take_warnings()
    results = batch_step(
        [dwelling],
        array("d", [21.0]),
        [_plan(dwelling)],
        [("Gas Furnace", "heat_c")],
        {"Gas Furnace": "ThermalSetpoint"},
        _SETPOINT_BOUNDS,
        _zero_buffer(1, len(_OBS_FIELDS)),
    )
    info = results[0]["info"]
    assert info["warning_count"] == 0
    assert "warnings" not in info


def test_batch_step_surfaces_rejected_control_signal_in_info():
    """A control signal that passes queue-time capability validation but is
    rejected at dispatch time (numeric bounds) must show up in step info so
    RL loops see it without polling take_warnings()."""
    from hares._hares import batch_step

    dwelling = make_dwelling(duration_s=600)
    # cooling_setpoint_c = 70 is inside the gym clip range (-50, 80) but
    # outside ControlSignal numeric bounds ([0, 60] degC), so it passes the
    # queue-time capability check and is rejected by Equipment::apply_control
    # during dispatch -- the silent-warning path this test guards.
    results = batch_step(
        [dwelling],
        array("d", [70.0]),
        [_plan(dwelling)],
        [("Gas Furnace", "cool_c")],
        {"Gas Furnace": "ThermalSetpoint"},
        _SETPOINT_BOUNDS,
        _zero_buffer(1, len(_OBS_FIELDS)),
    )
    info = results[0]["info"]
    assert info["warning_count"] >= 1
    assert any("control apply failed" in w for w in info["warnings"])
    assert any("Gas Furnace" in w for w in info["warnings"])
    # The messages were drained into info -- a subsequent poll is empty.
    assert dwelling.take_warnings() == []
