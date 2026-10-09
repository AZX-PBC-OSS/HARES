"""Tests for DwellingGymEnv and VecDwellingGymEnv."""

from __future__ import annotations

from datetime import timedelta

from hares import SimulationConfig, SiteLocationOverride
import math
from pathlib import Path
from typing import Any, cast

import pytest

# gymnasium ships only in the optional rl extra.
pytest.importorskip("gymnasium")

import numpy as np
from gymnasium import spaces

from hares._hares import Dwelling as PyDwelling
from hares._hares import SimulationConfig
from hares.rl.gym_env import (
    DwellingGymEnv,
    GymDwellingConfig,
    RewardContext,
    field_bounds,
    observation_field_bounds,
    sorted_action_layout,
    telemetry_to_observation,
)
from hares.rl.vec_env import VecDwellingGymEnv

ROOT = Path(__file__).resolve().parents[2]
HARES_DEFAULTS = ROOT / "defaults"
HPXML = str(ROOT / "tests/fixtures/hpxml/ochre_samples/base.xml")
WEATHER = str(ROOT / "data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw")
SCHEDULE = str(ROOT / "data/examples/BEopt_example_schedule.csv")

_DWELLING_CONFIG = GymDwellingConfig(
    hpxml=HPXML,
    schedule=SCHEDULE,
    weather=WEATHER,
    defaults_path=str(HARES_DEFAULTS),
    sim_config=SimulationConfig(
        start_time="2019-01-01T00:00:00",
        duration=600,
        time_res=60,
        write_output=False,
    ),
)


def _obs_box(env: DwellingGymEnv | VecDwellingGymEnv) -> spaces.Box:
    """The observation space as a Box (gymnasium is an importorskip here)."""
    space = env.observation_space
    assert isinstance(space, spaces.Box), "observation_space must be a Box"
    return space

# Gas Furnace is present in the base HPXML; heat_c resolves to ThermalSetpoint.
_ACTION_CONFIG: dict[str, list[str]] = {"Gas Furnace": ["heat_c"]}
_OBS_FIELDS = ["total_power_kw", "outdoor_temp"]


def _make_env(episode_length: timedelta = timedelta(minutes=5)) -> DwellingGymEnv:
    return DwellingGymEnv(
        config=_DWELLING_CONFIG,
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_kw"],
        episode_length=episode_length,
    )


def _make_dwelling(seed: int = 0, duration_s: int = 600) -> PyDwelling:
    return PyDwelling.from_hpxml(hpxml=HPXML, schedule=SCHEDULE, weather=WEATHER, defaults_path=str(HARES_DEFAULTS), config=SimulationConfig(start_time="2019-01-01T00:00:00", duration=duration_s, time_res=60, master_seed=seed, write_output=False))


def test_dwelling_gym_reset_seed_reproducible():
    env = _make_env()

    env.reset(seed=42)
    seq1 = [env.step(np.array([21.0], dtype=np.float64))[0].copy() for _ in range(3)]
    env.reset(seed=42)
    seq2 = [env.step(np.array([21.0], dtype=np.float64))[0].copy() for _ in range(3)]

    for left, right in zip(seq1, seq2, strict=True):
        assert np.allclose(left, right)


def test_dwelling_gym_spaces_and_mapping():
    env = _make_env()

    assert env.observation_space.shape == (len(_OBS_FIELDS),)
    assert env.action_space.shape == (1,)

    # Step directly after construction -- initialize() is called inside __init__.
    step_obs, reward, terminated, truncated, info = env.step(
        np.array([21.0], dtype=np.float64)
    )
    assert step_obs.shape == (len(_OBS_FIELDS),)
    assert step_obs.dtype == np.float64
    assert env.observation_space.contains(step_obs)
    assert isinstance(reward, float)
    assert terminated is False
    assert isinstance(truncated, bool)


def _water_heater_name(dwelling: PyDwelling) -> str:
    (name,) = [n for n in dwelling.equipment_names() if "Water Heater" in n]
    return name


def test_deadband_action_bounds_follow_the_target_thermostat_class():
    from hares._hares import (
        MAX_HVAC_THERMOSTAT_BAND_C,
        MAX_TANK_THERMOSTAT_BAND_C,
        MIN_THERMOSTAT_BAND_C,
    )

    water_heater = _water_heater_name(_make_dwelling())
    env = DwellingGymEnv(
        config=_DWELLING_CONFIG,
        observation_fields=_OBS_FIELDS,
        action_space_config={
            "Gas Furnace": ["heat_c", "deadband_c"],
            water_heater: ["setpoint_c", "deadband_c"],
        },
        reward_fn=lambda ctx: -ctx["total_power_kw"],
        episode_length=timedelta(minutes=5),
    )
    space = env.action_space
    assert isinstance(space, spaces.Box)
    bounds = {
        (equipment, field): (low, high)
        for (equipment, field), low, high in zip(env._action_layout, space.low, space.high)
    }
    assert bounds[("Gas Furnace", "deadband_c")] == (
        MIN_THERMOSTAT_BAND_C,
        MAX_HVAC_THERMOSTAT_BAND_C,
    )
    assert bounds[(water_heater, "deadband_c")] == (
        MIN_THERMOSTAT_BAND_C,
        MAX_TANK_THERMOSTAT_BAND_C,
    )


def test_thermostat_band_range_is_the_target_class():
    from hares._hares import MAX_HVAC_THERMOSTAT_BAND_C, MIN_THERMOSTAT_BAND_C

    dwelling = _make_dwelling()
    assert dwelling.thermostat_band_range("Gas Furnace") == (
        MIN_THERMOSTAT_BAND_C,
        MAX_HVAC_THERMOSTAT_BAND_C,
    )
    non_thermostats = [
        n for n in dwelling.equipment_names() if n != "Gas Furnace" and "Water" not in n
        and dwelling.thermostat_band_range(n) is None
    ]
    assert non_thermostats, "some equipment takes no band"


def test_a_deadband_action_on_equipment_without_a_band_is_rejected():
    dwelling = _make_dwelling()
    no_band = next(n for n in dwelling.equipment_names() if dwelling.thermostat_band_range(n) is None)
    with pytest.raises(ValueError, match="no thermostat band"):
        DwellingGymEnv(
            config=_DWELLING_CONFIG,
            observation_fields=_OBS_FIELDS,
            action_space_config={"Gas Furnace": ["heat_c"], no_band: ["setpoint_c", "deadband_c"]},
            reward_fn=lambda ctx: -ctx["total_power_kw"],
            episode_length=timedelta(minutes=5),
        )


def test_band_range_and_zip_each_carry_their_own_doc():
    band_doc = PyDwelling.thermostat_band_range.__doc__
    zip_doc = PyDwelling.equipment_zip.__doc__
    assert band_doc is not None and zip_doc is not None
    assert "deadband" in band_doc
    assert "ZIP" in zip_doc
    assert "deadband" not in zip_doc


def test_vec_env_bounds_a_deadband_by_each_dwelling_s_target_class():
    from hares._hares import MAX_HVAC_THERMOSTAT_BAND_C, MIN_THERMOSTAT_BAND_C

    dwellings = [_make_dwelling(seed=i) for i in range(2)]
    # The fixture's construction notices (base.xml gives no ShieldingofHome,
    # which takes OS-HPXML's default) are not a step's warnings.
    for dwelling in dwellings:
        dwelling.take_warnings()
    env = VecDwellingGymEnv(
        dwellings=dwellings,
        observation_fields=_OBS_FIELDS,
        action_space_config={"Gas Furnace": ["heat_c", "deadband_c"]},
        reward_fn=lambda ctx: -ctx["total_power_kw"],
        episode_length=timedelta(minutes=5),
    )
    column = env._action_layout.index(("Gas Furnace", "deadband_c"))
    assert env._action_bounds[column] == (MIN_THERMOSTAT_BAND_C, MAX_HVAC_THERMOSTAT_BAND_C)
    assert [field for _, field in env._action_layout] == ["deadband_c", "heat_c"]
    _, _, _, _, infos = env.step(np.asarray([[30.0, 21.0], [30.0, 21.0]]))
    assert [info["warning_count"] for info in infos] == [0.0, 0.0], "the band is clipped"


class _DwellingWithBand:
    """A dwelling whose furnace reports another thermostat class's range."""

    def __init__(self, dwelling: PyDwelling, band: tuple[float, float]) -> None:
        self._dwelling = dwelling
        self._band = band

    def thermostat_band_range(self, name: str) -> tuple[float, float]:
        return self._band

    def __getattr__(self, attr: str):
        return getattr(self._dwelling, attr)


def test_vec_env_refuses_a_fleet_whose_deadband_bounds_differ():
    from hares._hares import MAX_TANK_THERMOSTAT_BAND_C, MIN_THERMOSTAT_BAND_C

    other_class = _DwellingWithBand(
        _make_dwelling(seed=1), (MIN_THERMOSTAT_BAND_C, MAX_TANK_THERMOSTAT_BAND_C)
    )
    with pytest.raises(ValueError, match="different bounds across the dwellings"):
        VecDwellingGymEnv(
            dwellings=[_make_dwelling(seed=0), cast(PyDwelling, other_class)],
            observation_fields=_OBS_FIELDS,
            action_space_config={"Gas Furnace": ["heat_c", "deadband_c"]},
            reward_fn=lambda ctx: -ctx["total_power_kw"],
            episode_length=timedelta(minutes=5),
        )


def test_deadband_action_without_a_setpoint_field_is_rejected():
    with pytest.raises(ValueError, match="no setpoint field"):
        sorted_action_layout({"HVAC": ["deadband_c"]})
    layout, _ = sorted_action_layout({"HVAC": ["deadband_c", "heat_c"]})
    assert [f for _, f in layout] == ["deadband_c", "heat_c"]


def test_sorted_action_layout_preserves_equipment_sort_order():
    layout, type_by_equip = sorted_action_layout(
        {"B": ["reactive_power_kvar", "active_power_kw"], "A": ["active_power_kw"]}
    )

    equipment_names = [eq for eq, _ in layout]
    assert equipment_names[0] == "A"
    assert equipment_names[1] == "B"
    assert equipment_names[2] == "B"

    a_fields = [f for eq, f in layout if eq == "A"]
    assert a_fields == ["active_power_kw"]

    b_fields = [f for eq, f in layout if eq == "B"]
    assert b_fields == sorted(["reactive_power_kvar", "active_power_kw"])

    assert type_by_equip["A"] == "PowerSetpoint"
    assert type_by_equip["B"] == "PowerSetpoint"


def test_vec_gym_step_batch_shape():
    dwellings = [_make_dwelling(seed=i) for i in range(4)]
    env = VecDwellingGymEnv(
        dwellings=dwellings,
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_kw"],
        episode_length=timedelta(minutes=5),
    )

    assert env.num_envs == 4

    step_obs, rewards, dones, truncs, infos = env.step(
        np.full((4, 1), 21.0, dtype=np.float64)
    )
    assert step_obs.ndim == 2
    assert step_obs.shape[0] == 4
    assert step_obs.dtype == np.float64
    assert rewards.shape == (4,)
    assert dones.shape == (4,)
    assert truncs.shape == (4,)
    assert len(infos) == 4
    for info in infos:
        assert "obs_total_power_kw_low" in info
        assert "obs_total_power_kw_high" in info
        assert "obs_outdoor_temp_low" in info
        assert "obs_outdoor_temp_high" in info
        assert np.isfinite(info["obs_total_power_kw_low"])
        assert np.isfinite(info["obs_total_power_kw_high"])


def test_vec_gym_reset_seed_batch_shape():
    dwellings = [_make_dwelling(seed=i) for i in range(4)]
    env = VecDwellingGymEnv(
        dwellings=dwellings,
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_kw"],
        episode_length=timedelta(minutes=5),
    )

    obs, infos = env.reset(seed=123)
    assert obs.shape == (4, len(_OBS_FIELDS))
    assert obs.dtype == np.float64
    assert len(infos) == 4


def test_vec_gym_rejects_fork_start_method(monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setattr("multiprocessing.get_start_method", lambda allow_none=True: "fork")

    dwellings = [_make_dwelling()]
    with pytest.raises(RuntimeError, match=r"fork \+ Rayon"):
        VecDwellingGymEnv(
            dwellings=dwellings,
            observation_fields=_OBS_FIELDS,
            action_space_config=_ACTION_CONFIG,
            reward_fn=lambda _: 0.0,
            episode_length=timedelta(minutes=1),
        )


def test_vec_gym_random_policy_100_steps():
    """100-step random-policy integration test exercises Rust batch_step with >=2 signal types."""
    from hares import Battery
    from hares._hares import Dwelling as PyDwelling

    n_dwel = 2
    obs_fields = ["total_power_kw", "outdoor_temp"]
    dwellings = [
        PyDwelling.from_hpxml(hpxml=HPXML, schedule=SCHEDULE, weather=WEATHER, defaults_path=str(HARES_DEFAULTS), config=SimulationConfig(start_time="2019-01-01T00:00:00", duration=6000, time_res=60, master_seed=i, write_output=False))
        for i in range(n_dwel)
    ]
    for dw in dwellings:
        dw.add_battery(Battery("Batt", 10.0, max_charge_kw=5.0, max_discharge_kw=5.0))

    action_config = {
        "Gas Furnace": ["heat_c"],
        "Batt": ["active_power_kw"],
    }

    env = VecDwellingGymEnv(
        dwellings=dwellings,
        observation_fields=obs_fields,
        action_space_config=action_config,
        reward_fn=lambda ctx: -ctx["total_power_kw"],
        episode_length=timedelta(seconds=6000),
    )

    rng = np.random.default_rng(42)
    obs_fields = ["total_power_kw", "outdoor_temp"]
    for step_idx in range(100):
        actions = rng.uniform(-1.0, 1.0, size=(n_dwel, 2)).astype(np.float64)
        step_obs, rewards, dones, truncs, infos = env.step(actions)
        assert step_obs.shape == (n_dwel, len(obs_fields)), (
            f"obs shape mismatch at step {step_idx}"
        )
        assert np.all(np.isfinite(step_obs)), (
            f"Non-finite observation at step {step_idx}: {step_obs}"
        )
        assert np.all(np.isfinite(rewards)), (
            f"Non-finite reward at step {step_idx}: {rewards}"
        )


def test_apply_action_clips_at_bounds():
    """_apply_action clips action values to action_space.low/high
    before passing them to _build_control_signal."""
    env = _make_env()
    env.reset(seed=42)
    action_space = env.action_space
    assert isinstance(action_space, spaces.Box)
    low = action_space.low[0]
    high = action_space.high[0]

    from hares.rl import gym_env
    original = gym_env.build_control_signal
    captured = []

    def _spy(signal_type, values):
        captured.append(dict(values))
        return original(signal_type, values)

    gym_env.build_control_signal = _spy
    try:
        # Above upper bound is clipped to high.
        env._apply_action(np.array([high + 100.0], dtype=np.float64))
        assert captured
        val = captured[0]["heat_c"]
        assert low <= val <= high, f"heat_c {val} should be in [{low}, {high}]"

        # Below lower bound is clipped to low.
        captured.clear()
        env._apply_action(np.array([low - 100.0], dtype=np.float64))
        val = captured[0]["heat_c"]
        assert low <= val <= high, f"heat_c {val} should be in [{low}, {high}]"

        # At bounds are passed through unchanged.
        for bound in (float(low), float(high)):
            captured.clear()
            env._apply_action(np.array([bound], dtype=np.float64))
            val = captured[0]["heat_c"]
            assert low <= val <= high, f"heat_c {val} should be in [{low}, {high}]"
    finally:
        gym_env.build_control_signal = original


def test_gym_step_refuses_a_non_finite_action():
    """A non-finite action is a refused call on the single-dwelling path
    too: the clip binding rejects it the way the batch path does, instead
    of counting it as one clamped value."""
    env = _make_env()
    with pytest.raises(ValueError, match="not finite"):
        env.step(np.array([float("nan")], dtype=np.float64))


def test_rust_and_python_paths_give_identical_rewards(
    monkeypatch: pytest.MonkeyPatch,
):
    """One reward contract: over a fixed action sequence, the same two
    dwellings stepped by the extension's batch_step and stepped from Python
    (batch_step forced off) give equal rewards per environment and step,
    exactly, for a reward_fn that reads the zone telemetry and
    total_power_kw."""
    import hares.rl.vec_env as vec_env_module

    def zone_power_reward(ctx: RewardContext) -> float:
        zone_temps = ctx["telemetry_zone"]["temperature_c"]
        return ctx["total_power_kw"] - 0.01 * float(zone_temps[0])

    envs: list[VecDwellingGymEnv] = []
    for _ in range(2):
        # 1800 s of sim time: the 20 fixed steps all land inside the run.
        dwellings = [_make_dwelling(seed=i, duration_s=1800) for i in range(2)]
        env = VecDwellingGymEnv(
            dwellings=dwellings,
            observation_fields=_OBS_FIELDS,
            action_space_config=_ACTION_CONFIG,
            reward_fn=zone_power_reward,
            episode_length=timedelta(minutes=20),
        )
        env.reset(seed=7)
        envs.append(env)
    batch_env, python_env = envs

    rng = np.random.default_rng(3)
    # The heat_c bounds are (-50, 80): the range reaches past them, so the
    # pin exercises the clip path, not just in-bounds actions.
    action_sequence = [np.asarray(a) for a in rng.uniform(-90.0, 130.0, size=(20, 2, 1))]
    low, high = batch_env._action_bounds[0]
    clipped_actions = [
        actions for actions in action_sequence if np.any(actions < low) or np.any(actions > high)
    ]
    assert clipped_actions, "the pin must exercise actions that clip to the declared bounds"
    batch_rewards = [batch_env.step(actions)[1] for actions in action_sequence]

    monkeypatch.setattr(vec_env_module, "rust_batch_step", None)
    python_rewards = [python_env.step(actions)[1] for actions in action_sequence]

    for step_idx, (batch, python) in enumerate(zip(batch_rewards, python_rewards, strict=True)):
        assert batch.tobytes() == python.tobytes(), (
            f"the batch and Python paths disagree on the reward_fn's rewards "
            f"at step {step_idx}: {batch} vs {python}"
        )


def test_vec_env_rejects_a_non_callable_reward_fn_at_construction():
    """A reward_fn that is not callable fails at construction, naming the
    field, before any dwelling is initialized or advanced."""
    with pytest.raises(TypeError, match="reward_fn"):
        VecDwellingGymEnv(
            dwellings=[_make_dwelling()],
            observation_fields=_OBS_FIELDS,
            action_space_config=_ACTION_CONFIG,
            reward_fn=cast(Any, None),
            episode_length=timedelta(minutes=5),
        )


def test_both_step_paths_build_the_same_info_shape(monkeypatch: pytest.MonkeyPatch):
    """One info shape: on a clean step, both paths carry "step" (the step
    dict the reward reads) and the same keys, so an RL caller reads one
    surface whichever path stepped the fleet."""
    import hares.rl.vec_env as vec_env_module

    envs: list[VecDwellingGymEnv] = []
    for _ in range(2):
        dwellings = [_make_dwelling(seed=i) for i in range(2)]
        # The fixture's construction notices are not a step's warnings.
        for dwelling in dwellings:
            dwelling.take_warnings()
        env = VecDwellingGymEnv(
            dwellings=dwellings,
            observation_fields=_OBS_FIELDS,
            action_space_config=_ACTION_CONFIG,
            reward_fn=lambda ctx: -ctx["total_power_kw"],
            episode_length=timedelta(minutes=5),
        )
        env.reset(seed=7)
        envs.append(env)
    batch_env, python_env = envs

    actions = np.full((2, 1), 21.0, dtype=np.float64)
    _, _, _, _, batch_infos = batch_env.step(actions)
    monkeypatch.setattr(vec_env_module, "rust_batch_step", None)
    _, _, _, _, python_infos = python_env.step(actions)

    for batch_info, python_info in zip(batch_infos, python_infos, strict=True):
        assert set(batch_info) == set(python_info), (
            f"the batch and Python paths must build the same info shape: "
            f"{sorted(batch_info)} vs {sorted(python_info)}"
        )
        assert "step" in batch_info
        assert "net_electric_power_kw" in batch_info["step"]
        assert "warning_count" in batch_info


def test_a_failed_env_keeps_the_batch_rectangular():
    """One env failing mid-run returns the batch: the failed env's row
    carries its last observation, reward 0.0 and terminated, next to the
    living env's step, instead of a ragged obs list that never assembles."""
    dwellings = [
        _make_dwelling(seed=0, duration_s=600),
        _make_dwelling(seed=1, duration_s=120),
    ]
    env = VecDwellingGymEnv(
        dwellings=dwellings,
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_kw"],
        episode_length=timedelta(minutes=20),
    )
    env.reset(seed=7)
    actions = np.zeros((2, 1), dtype=np.float64)

    # The short home's sim holds two steps of 60 s; both homes step them.
    env.step(actions)
    prev_obs, _, _, _, _ = env.step(actions)

    # The third step fails the short home (past its end) while the living
    # home steps on.
    obs, rewards, dones, _truncs, infos = env.step(actions)

    assert obs.shape == (2, len(_OBS_FIELDS))
    assert obs[1].tobytes() == prev_obs[1].tobytes(), (
        "the failed env's row must carry its last observation: the failed "
        "step changed nothing"
    )
    assert rewards[1] == 0.0
    assert dones[1]
    assert not dones[0]
    assert infos[1]["error"] == 1.0


def test_vec_gym_out_of_bounds_actions_no_nan():
    """100-step random-policy test with actions 10x the declared range.

    Runs on the extension's batch_step, which clips every action value to
    the declared action-space bounds before applying it, so the
    observations and rewards stay finite.
    """
    import hares.rl.vec_env as ve

    assert ve.rust_batch_step is not None, (
        "the extension registers batch_step; without it the vec env does not import"
    )

    n_dwel = 2
    obs_fields = ["total_power_kw", "outdoor_temp"]
    dwellings = [
        PyDwelling.from_hpxml(hpxml=HPXML, schedule=SCHEDULE, weather=WEATHER, defaults_path=str(HARES_DEFAULTS), config=SimulationConfig(start_time="2019-01-01T00:00:00", duration=6000, time_res=60, master_seed=i, write_output=False))
        for i in range(n_dwel)
    ]
    from hares import Battery
    for dw in dwellings:
        dw.add_battery(Battery("Batt", 10.0, max_charge_kw=5.0, max_discharge_kw=5.0))

    action_config = {
        "Gas Furnace": ["heat_c"],
        "Batt": ["active_power_kw"],
    }

    env = VecDwellingGymEnv(
        dwellings=dwellings,
        observation_fields=obs_fields,
        action_space_config=action_config,
        reward_fn=lambda ctx: -ctx["total_power_kw"],
        episode_length=timedelta(seconds=6000),
    )

    low = env._action_low
    high = env._action_high
    scale = 10.0
    rng = np.random.default_rng(42)

    for step_idx in range(100):
        actions = rng.uniform(low * scale, high * scale, size=(n_dwel, len(env._action_layout))).astype(np.float64)
        step_obs, rewards, _dones, _truncs, _infos = env.step(actions)
        assert step_obs.shape == (n_dwel, len(obs_fields)), (
            f"obs shape mismatch at step {step_idx}"
        )
        assert np.all(np.isfinite(step_obs)), (
            f"Non-finite observation at step {step_idx}: {step_obs}"
        )
        assert np.all(np.isfinite(rewards)), (
            f"Non-finite reward at step {step_idx}: {rewards}"
        )


# ---------------------------------------------------------------------------
# Observation bounds tests (T-0339)
# ---------------------------------------------------------------------------


def test_observation_field_bounds_all_finite():
    """Every known observation key (and the fallback) returns finite bounds."""
    from hares.rl.gym_env import observation_field_bounds

    fields = [
        "outdoor_temp",
        "outdoor_temp_c",
        "outdoor_rh",
        "outdoor_humidity_ratio",
        "total_power_kw",
        "total_electric_kw",
        "zone_temp[Living Room]",
        "zone_temp[Building]",
        "setpoint_heat[Main]",
        "setpoint_cool[Main]",
        "equipment_soc[Battery]",
        "equipment_power[Gas Furnace]",
        "battery_soc",
        "ev_soc",
        "time_sin",
        "time_cos",
        "unknown_field",
    ]
    for field in fields:
        low, high = observation_field_bounds(field)
        assert np.isfinite(low), f"low bound {low} is not finite for field {field!r}"
        assert np.isfinite(high), f"high bound {high} is not finite for field {field!r}"
        assert not np.isnan(low), f"low bound is NaN for field {field!r}"
        assert not np.isnan(high), f"high bound is NaN for field {field!r}"
        assert low < high, f"bounds inverted for {field!r}: {low} >= {high}"


def test_observation_space_bounds_all_finite():
    """Observation space arrays must contain no -inf or +inf values."""
    env = _make_env()
    low = np.asarray(_obs_box(env).low, dtype=np.float64)
    high = np.asarray(_obs_box(env).high, dtype=np.float64)
    assert np.all(np.isfinite(low)), f"non-finite low bounds: {low}"
    assert np.all(np.isfinite(high)), f"non-finite high bounds: {high}"


def test_observation_bounds_in_step_info():
    """step() info dict must include per-field observation bounds."""
    env = _make_env()
    _, _, _, _, info = env.step(np.array([21.0], dtype=np.float64))
    assert "observation_bounds" in info, "StepInfo missing observation_bounds key"
    bounds = info["observation_bounds"]
    assert isinstance(bounds, dict)
    for field in _OBS_FIELDS:
        assert field in bounds, f"field {field!r} not in observation_bounds"
        low, high = bounds[field]
        assert np.isfinite(low)
        assert np.isfinite(high)
        assert low < high


def test_normalize_observation_wrapper_no_crash_or_shape_mismatch():
    """NormalizeObservation wrapper does not crash on reset()/step().
    
    gymnasium.wrappers.NormalizeObservation is a documented Known Limitation:
    its obs_rms is contaminated by NaN reset observations. This test exercises
    the wrapper's reset()/step() protocol to catch regressions that would
    produce shape mismatches, dtype errors, or exceptions — failures that
    are silent because no other test touches NormalizeObservation.
    """
    import gymnasium as gym

    env = _make_env()
    wrapped = gym.wrappers.NormalizeObservation(env)

    obs, info = wrapped.reset(seed=42)
    assert obs.shape == (len(_OBS_FIELDS),)
    assert np.issubdtype(obs.dtype, np.floating)

    step_obs, reward, terminated, truncated, step_info = wrapped.step(
        np.array([21.0], dtype=np.float64)
    )
    assert step_obs.shape == (len(_OBS_FIELDS),)
    assert np.issubdtype(step_obs.dtype, np.floating)
    assert isinstance(reward, float)
    assert terminated is False
    assert isinstance(truncated, bool)


def test_field_bounds_overrides_apply():
    """field_bounds_overrides replace default bounds for specified fields."""
    env = DwellingGymEnv(
        config=_DWELLING_CONFIG,
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_kw"],
        episode_length=timedelta(minutes=5),
        field_bounds_overrides={"total_power_kw": (-200.0, 200.0)},
    )
    low = _obs_box(env).low
    high = _obs_box(env).high
    total_power_idx = _OBS_FIELDS.index("total_power_kw")
    assert np.isclose(float(low[total_power_idx]), -200.0), f"override low not applied: {low}"
    assert np.isclose(float(high[total_power_idx]), 200.0), f"override high not applied: {high}"


def test_observation_field_bounds_case_insensitive():
    """Bracket-prefixed fields match regardless of case or leading whitespace."""
    tests = [
        ("zone_temp[Living Room]", (0.0, 50.0)),
        ("Zone_Temp[Living Room]", (0.0, 50.0)),
        (" ZONE_TEMP[Kitchen]", (0.0, 50.0)),
        ("setpoint_heat[Main]", (0.0, 50.0)),
        ("Setpoint_Heat[Main]", (0.0, 50.0)),
        (" setpoint_cool[Upstairs]", (0.0, 50.0)),
        ("equipment_soc[Battery]", (0.0, 1.0)),
        ("Equipment_Soc[Battery]", (0.0, 1.0)),
        (" equipment_power[Gas Furnace]", (0.0, 100.0)),
    ]
    for field, expected in tests:
        low, high = observation_field_bounds(field)
        assert low == expected[0] and high == expected[1], (
            f"{field!r}: expected {expected}, got ({low}, {high})"
        )


def test_observation_field_bounds_broad_fallback_is_finite():
    """Unrecognised fields return the broad but finite fallback, not ±inf."""
    for field in ("giraffe_temp_c", "  fluffy_rh  "):
        low, high = observation_field_bounds(field)
        assert np.isfinite(low), f"fallback low {low} not finite for {field!r}"
        assert np.isfinite(high), f"fallback high {high} not finite for {field!r}"
        assert low < high, f"fallback bounds inverted for {field!r}"


def test_vec_gym_observation_space_bounds_finite():
    """VecDwellingGymEnv observation space must contain no -inf or +inf values."""
    dwellings = [_make_dwelling(seed=i) for i in range(2)]
    env = VecDwellingGymEnv(
        dwellings=dwellings,
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_kw"],
        episode_length=timedelta(minutes=5),
    )
    low = np.asarray(_obs_box(env).low, dtype=np.float64)
    high = np.asarray(_obs_box(env).high, dtype=np.float64)
    assert np.all(np.isfinite(low)), f"non-finite low bounds in VecDwellingGymEnv: {low}"
    assert np.all(np.isfinite(high)), f"non-finite high bounds in VecDwellingGymEnv: {high}"


def test_vec_gym_field_bounds_overrides_in_info():
    """VecDwellingGymEnv step() info dicts must use override-aware bounds,
    not stale defaults that silently disagree with observation_space."""
    dwellings = [_make_dwelling(seed=i) for i in range(2)]
    env = VecDwellingGymEnv(
        dwellings=dwellings,
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_kw"],
        episode_length=timedelta(minutes=5),
        field_bounds_overrides={"total_power_kw": (-200.0, 200.0)},
    )
    _, _, _, _, infos = env.step(
        np.full((2, 1), 21.0, dtype=np.float64)
    )
    for info in infos:
        assert np.isclose(
            float(info["obs_total_power_kw_low"]), -200.0
        ), f"override low not in info: {info}"
        assert np.isclose(
            float(info["obs_total_power_kw_high"]), 200.0
        ), f"override high not in info: {info}"


# ---------------------------------------------------------------------------
# time_sin / time_cos bounds and integration tests (T-0340)
# ---------------------------------------------------------------------------


def test_observation_field_bounds_time_sin_cos():
    """time_sin and time_cos have bounds [-1, 1]."""
    low, high = observation_field_bounds("time_sin")
    assert low == -1.0 and high == 1.0, f"time_sin: ({low}, {high})"
    low, high = observation_field_bounds("time_cos")
    assert low == -1.0 and high == 1.0, f"time_cos: ({low}, {high})"


def test_time_sin_cos_fields_in_observation():
    """time_sin and time_cos produce finite, in-bounds values via telemetry_to_observation."""
    env = DwellingGymEnv(
        config=_DWELLING_CONFIG,
        observation_fields=["time_sin", "time_cos"],
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_kw"],
        episode_length=timedelta(minutes=5),
    )
    env.reset(seed=42)
    obs, _, _, _, _ = env.step(np.array([21.0], dtype=np.float64))
    assert obs.shape == (2,)
    assert np.all(np.isfinite(obs)), f"non-finite observation: {obs}"
    assert -1.0 <= obs[0] <= 1.0, f"time_sin out of bounds: {obs[0]}"
    assert -1.0 <= obs[1] <= 1.0, f"time_cos out of bounds: {obs[1]}"


def test_time_sin_cos_observation_space_finite():
    """Observation space with time_sin/time_cos has only finite bounds."""
    env = DwellingGymEnv(
        config=_DWELLING_CONFIG,
        observation_fields=["time_sin", "time_cos", "total_power_kw"],
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_kw"],
        episode_length=timedelta(minutes=5),
    )
    low = np.asarray(_obs_box(env).low, dtype=np.float64)
    high = np.asarray(_obs_box(env).high, dtype=np.float64)
    assert np.all(np.isfinite(low)), f"non-finite low: {low}"
    assert np.all(np.isfinite(high)), f"non-finite high: {high}"
    assert np.isclose(float(low[0]), -1.0)
    assert np.isclose(float(high[0]), 1.0)


# ---------------------------------------------------------------------------
# actor_telemetry resolution tests (T-0340)
# ---------------------------------------------------------------------------


def test_telemetry_to_observation_actor_telemetry_dot_notation():
    """telemetry_to_observation resolves actor_telemetry fields via <actor>.<channel> dot notation."""
    dwelling = _make_dwelling()
    dwelling.initialize()
    dwelling.add_actor_by_name("DrCompliance", "DRAgent", None)
    dwelling.step()
    tel = dwelling.telemetry()

    obs = telemetry_to_observation(tel, ["DRAgent.dr_level"])
    assert obs.shape == (1,)
    assert obs[0] == 0.0, f"dr_level should be 0.0 for Normal DR level, got {obs[0]}"


def test_telemetry_to_observation_actor_telemetry_bare_name():
    """telemetry_to_observation resolves actor_telemetry fields via bare-name search across all actors."""
    dwelling = _make_dwelling()
    dwelling.initialize()
    dwelling.add_actor_by_name("DrCompliance", "DRAgent", None)
    dwelling.step()
    tel = dwelling.telemetry()

    obs = telemetry_to_observation(tel, ["dr_active"])
    assert obs.shape == (1,)
    assert obs[0] == 0.0, f"dr_active should be 0.0 when no DR event, got {obs[0]}"


def test_telemetry_to_observation_builtin_wins_over_actor_fallback():
    """Built-in telemetry fields take priority over actor_telemetry for the same key name."""
    dwelling = _make_dwelling()
    dwelling.initialize()
    dwelling.add_actor_by_name("DrCompliance", "DRAgent", None)
    dwelling.step()
    tel = dwelling.telemetry()

    # total_power_kw is a built-in field; actor_telemetry must not override it.
    obs = telemetry_to_observation(tel, ["total_power_kw"])
    assert obs.shape == (1,)
    assert obs[0] != 0.0, "built-in total_power_kw should not fall through to actor_telemetry"


def test_actor_telemetry_fields_in_dwelling_gym_observation():
    """End-to-end: actor-injected fields appear in the observation vector via telemetry_to_observation.

    Creates a dwelling, adds a DrCompliance actor, steps, and verifies that
    actor_telemetry fields resolve correctly in the observation output.
    This exercises the full actor → dwelling.step → telemetry → telemetry_to_observation
    pipeline without going through DwellingGymEnv's reset cycle (which recreates the
    dwelling and would discard dynamically-added actors).
    """
    dwelling = _make_dwelling()
    dwelling.initialize()
    dwelling.add_actor_by_name("DrCompliance", "DRAgent", None)
    dwelling.step()
    tel = dwelling.telemetry()

    # Dot-notation: <actor>.<channel>
    obs = telemetry_to_observation(tel, ["DRAgent.dr_level"])
    assert obs.shape == (1,)
    assert obs[0] == 0.0, f"dr_level should be 0.0 for Normal DR level, got {obs[0]}"

    # Bare-name search
    obs = telemetry_to_observation(tel, ["dr_active"])
    assert obs.shape == (1,)
    assert obs[0] == 0.0, f"dr_active should be 0.0 when no DR event, got {obs[0]}"

    # Combined: built-in + actor_telemetry fields in the same observation vector
    obs = telemetry_to_observation(tel, ["total_power_kw", "DRAgent.dr_level", "DRAgent.dr_active"])
    assert obs.shape == (3,)
    assert np.all(np.isfinite(obs)), f"non-finite observation: {obs}"
    assert obs[0] != 0.0, "built-in total_power_kw should be non-zero after step"
    assert obs[1] == 0.0
    assert obs[2] == 0.0


# ---------------------------------------------------------------------------
# Initial observation NaN & initialized flag tests (T-0341)
# ---------------------------------------------------------------------------


class _MockTelemetryMissingOutdoor:
    """Mock telemetry where zone() dict lacks outdoor_temp_c and outdoor_humidity_ratio."""

    def __init__(self):
        pass

    @property
    def initialized(self):
        return True

    @property
    def total_power_kw(self):
        return 0.0

    @property
    def current_time(self):
        import datetime
        return datetime.datetime(2019, 1, 1, 0, 0, 0)

    def zone(self):
        return {"names": [], "temperature_c": [], "setpoint_heat_c": [], "setpoint_cool_c": []}

    def equipment(self):
        return {"names": [], "modes": [], "soc": [], "power_kw": []}

    def actors(self):
        return {}


def test_telemetry_to_observation_nan_fallback_for_missing_outdoor_temp():
    """When zone dict lacks outdoor_temp_c / outdoor_humidity_ratio, fallback is NaN not 0.0."""
    mock = _MockTelemetryMissingOutdoor()
    obs = telemetry_to_observation(mock, ["outdoor_temp"])
    assert obs.shape == (1,)
    assert np.isnan(obs[0]), f"expected NaN for missing outdoor_temp_c, got {obs[0]}"


def test_telemetry_to_observation_nan_fallback_for_missing_outdoor_rh():
    """When zone dict lacks outdoor_humidity_ratio, fallback is NaN not 0.0."""
    mock = _MockTelemetryMissingOutdoor()
    obs = telemetry_to_observation(mock, ["outdoor_humidity_ratio"])
    assert obs.shape == (1,)
    assert np.isnan(obs[0]), f"expected NaN for missing outdoor_humidity_ratio, got {obs[0]}"


def test_reset_observation_contains_nan_for_uninitialized():
    """After reset(), pre-first-step observation contains NaN for uninitialized fields."""
    env = DwellingGymEnv(
        config=_DWELLING_CONFIG,
        observation_fields=["total_power_kw", "outdoor_temp", "outdoor_humidity_ratio", "time_sin", "time_cos", "equipment_soc[Gas Furnace]"],
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_kw"],
        episode_length=timedelta(minutes=5),
    )
    obs, info = env.reset(seed=42)
    assert obs.shape == (6,)
    # All fields should be NaN because initialized == False at reset()
    assert np.all(np.isnan(obs)), f"expected all NaN on reset, got {obs}"

    mask = info.get("initial_observation_mask")
    assert mask is not None, "reset info must include initial_observation_mask"
    assert mask.shape == obs.shape
    assert np.all(mask), "all fields should be masked (NaN) on initial observation"

    assert "seed" in info


def test_step_observation_all_finite_after_first_step():
    """After the first step(), all observation dimensions must be finite (no lingering NaN)."""
    env = DwellingGymEnv(
        config=_DWELLING_CONFIG,
        observation_fields=["total_power_kw", "outdoor_temp", "outdoor_humidity_ratio", "time_sin", "time_cos", "equipment_soc[Gas Furnace]"],
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_kw"],
        episode_length=timedelta(minutes=5),
    )
    env.reset(seed=42)
    step_obs, reward, terminated, truncated, info = env.step(np.array([21.0], dtype=np.float64))
    assert step_obs.shape == (6,)
    assert np.all(np.isfinite(step_obs)), f"expected all finite after step(), got {step_obs}"

    # initial_observation_mask should be None after step (no longer initial)
    assert info.get("initial_observation_mask") is None, (
        "initial_observation_mask should be None after step()"
    )


def test_telemetry_to_observation_all_nan_when_uninitialized():
    """telemetry_to_observation returns all NaN when telemetry.initialized is False."""
    # Use a real dwelling post-construction (before any step)
    dwelling = _make_dwelling()
    dwelling.initialize()
    tel = dwelling.telemetry()
    assert tel.initialized is False, "telemetry should be uninitialized before first step"

    obs = telemetry_to_observation(
        tel, ["total_power_kw", "outdoor_temp", "outdoor_humidity_ratio", "time_sin"]
    )
    assert obs.shape == (4,)
    assert np.all(np.isnan(obs)), f"expected all NaN when uninitialized, got {obs}"


def test_telemetry_to_observation_finite_after_step():
    """telemetry_to_observation returns finite values after a simulation step."""
    dwelling = _make_dwelling()
    dwelling.initialize()
    dwelling.step()
    tel = dwelling.telemetry()
    assert tel.initialized is True, "telemetry should be initialized after step()"

    obs = telemetry_to_observation(
        tel, ["total_power_kw", "outdoor_temp", "outdoor_humidity_ratio", "time_sin"]
    )
    assert obs.shape == (4,)
    assert np.all(np.isfinite(obs)), f"expected all finite after step(), got {obs}"


def test_initialized_flag_present_on_telemetry():
    """Telemetry object exposes the initialized boolean property."""
    dwelling = _make_dwelling()
    dwelling.initialize()
    tel = dwelling.telemetry()
    assert hasattr(tel, "initialized")
    assert isinstance(tel.initialized, bool)
    assert tel.initialized is False

    dwelling.step()
    tel = dwelling.telemetry()
    assert tel.initialized is True


def test_initial_observation_warning_emitted_once():
    """One-time warning is emitted on first reset() when observation contains NaN."""
    import warnings

    env = DwellingGymEnv(
        config=_DWELLING_CONFIG,
        observation_fields=["total_power_kw", "outdoor_temp", "outdoor_humidity_ratio", "reactive_power_kvar"],
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_kw"],
        episode_length=timedelta(minutes=5),
    )

    # Reset the module-level flag so we get a clean test
    import hares.rl.gym_env as gym_env_mod
    gym_env_mod._warned_initial_nan = False

    with warnings.catch_warnings(record=True) as w:
        warnings.simplefilter("always")
        env.reset(seed=1)
        assert len(w) == 1, f"expected exactly 1 warning, got {len(w)}"
        assert "NaN" in str(w[0].message), f"warning message should mention NaN, got {w[0].message!r}"
        assert issubclass(w[0].category, UserWarning)

    # Second reset should NOT emit a second warning
    with warnings.catch_warnings(record=True) as w:
        warnings.simplefilter("always")
        env.reset(seed=2)
        assert len(w) == 0, f"expected no warning on second reset, got {len(w)}"


def test_vec_gym_reset_observation_nan_and_mask():
    """VecDwellingGymEnv.reset() returns NaN obs with per-env mask and one-time warning."""
    import warnings
    import hares.rl.vec_env as ve

    dwellings = [_make_dwelling(seed=i) for i in range(4)]
    env = VecDwellingGymEnv(
        dwellings=dwellings,
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_kw"],
        episode_length=timedelta(minutes=5),
    )

    ve._warned_initial_nan = False

    with warnings.catch_warnings(record=True) as w:
        warnings.simplefilter("always")
        obs, infos = env.reset(seed=123)

        assert obs.shape == (4, len(_OBS_FIELDS))
        assert obs.dtype == np.float64
        assert np.all(np.isnan(obs)), f"expected all NaN on reset, got {obs}"

        assert len(infos) == 4
        for info in infos:
            mask = info.get("initial_observation_mask")
            assert isinstance(mask, np.ndarray), "reset info must include initial_observation_mask"
            assert mask.shape == (len(_OBS_FIELDS),)
            assert np.all(mask), "all fields should be masked (NaN) on initial observation"

        assert len(w) == 1, f"expected exactly 1 warning, got {len(w)}"
        assert "NaN" in str(w[0].message)
        assert issubclass(w[0].category, UserWarning)

    # Second reset must not emit a second warning.
    ve._warned_initial_nan = True  # already warned during first reset above
    with warnings.catch_warnings(record=True) as w:
        warnings.simplefilter("always")
        env.reset(seed=456)
        assert len(w) == 0, f"expected no warning on second reset, got {len(w)}"


# ---------------------------------------------------------------------------
# Field dispatch regression tests (T-0342)
# ---------------------------------------------------------------------------


class _FakeTelemetry:
    """Minimal fake telemetry for unit-testing telemetry_to_observation field dispatch.

    Requires no native _hares module, no HPXML fixtures, and no Dwelling construction.
    """

    initialized = True
    total_power_kw = 5.0
    reactive_power_kvar = 1.5

    def __init__(self):
        import datetime
        self.current_time = datetime.datetime(2019, 1, 1, 12, 0, 0)

    def zone(self):
        return {
            "names": ["Living Room", "Kitchen"],
            "temperature_c": [22.0, 23.5],
            "setpoint_heat_c": [20.0, 19.0],
            "setpoint_cool_c": [24.0, 25.0],
            "energy_balance_residuals": [10.0, -5.0],
            "outdoor_temp_c": 5.0,
            "outdoor_humidity_ratio": 0.004,
        }

    def equipment(self):
        return {
            "names": ["Battery", "Gas Furnace", "electric vehicle"],
            "soc": [0.5, 0.0, 0.8],
            "power_kw": [1.0, 2.0, 0.0],
        }

    def actors(self):
        return {}


def test_telemetry_to_observation_reactive_power_kvar_no_keyerror():
    """telemetry_to_observation handles reactive_power_kvar without raising KeyError."""
    obs = telemetry_to_observation(_FakeTelemetry(), ["reactive_power_kvar"])
    assert obs.shape == (1,)
    assert np.isfinite(obs[0])
    assert obs[0] == 1.5


def test_telemetry_to_observation_reactive_power_kvar_in_mixed_fields():
    """reactive_power_kvar coexists with other fields in a single observation vector."""
    obs = telemetry_to_observation(
        _FakeTelemetry(),
        ["total_power_kw", "reactive_power_kvar", "outdoor_temp"],
    )
    assert obs.shape == (3,)
    assert np.all(np.isfinite(obs))
    assert obs[1] == 1.5


def test_telemetry_to_observation_zone_energy_balance_no_keyerror():
    """telemetry_to_observation handles zone_energy_balance[name] without raising KeyError."""
    obs = telemetry_to_observation(_FakeTelemetry(), ["zone_energy_balance[Living Room]"])
    assert obs.shape == (1,)
    assert np.isfinite(obs[0])


def test_telemetry_to_observation_unknown_field_keyerror():
    """telemetry_to_observation raises KeyError for truly unrecognised fields."""
    with pytest.raises(KeyError, match="unknown observation field"):
        telemetry_to_observation(_FakeTelemetry(), ["nonexistent_field_name"])


def test_telemetry_to_observation_all_known_scalar_keys_no_keyerror():
    """All known scalar (non-bracket, non-actor) observation keys resolve without KeyError."""
    fake = _FakeTelemetry()
    scalar_fields = [
        "outdoor_temp",
        "outdoor_temp_c",
        "outdoor_humidity_ratio",
        "total_power_kw",
        "total_electric_kw",
        "reactive_power_kvar",
        "battery_soc",
        "ev_soc",
        "time_sin",
        "time_cos",
    ]
    obs = telemetry_to_observation(fake, scalar_fields)
    assert obs.shape == (len(scalar_fields),)
    assert np.all(np.isfinite(obs))


def test_telemetry_to_observation_bracket_keys_no_keyerror():
    """All bracket-prefixed observation keys resolve without KeyError."""
    fake = _FakeTelemetry()
    bracket_fields = [
        "zone_temp[Living Room]",
        "zone_temp[Kitchen]",
        "setpoint_heat[Living Room]",
        "setpoint_cool[Living Room]",
        "zone_energy_balance[Living Room]",
        "equipment_soc[Battery]",
        "equipment_power[Gas Furnace]",
    ]
    obs = telemetry_to_observation(fake, bracket_fields)
    assert obs.shape == (len(bracket_fields),)
    assert np.all(np.isfinite(obs))


# ---------------------------------------------------------------------------
# Dual-path observation equivalence verification (T-0342)
# ---------------------------------------------------------------------------


def test_vec_gym_verify_observation_equivalence_no_crash():
    """VecDwellingGymEnv with verify_observation_equivalence=True steps without crashing.

    When Rust batch_step and Python telemetry_to_observation produce equivalent
    observations, the comparison should emit no false-positive warnings.
    """
    import warnings

    dwellings = [_make_dwelling(seed=i) for i in range(2)]
    env = VecDwellingGymEnv(
        dwellings=dwellings,
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_kw"],
        episode_length=timedelta(minutes=5),
        verify_observation_equivalence=True,
    )

    with warnings.catch_warnings(record=True) as w:
        warnings.simplefilter("always")
        for _step_index in range(3):
            env.step(np.full((2, 1), 21.0, dtype=np.float64))
        equivalence_warnings = [
            _w for _w in w
            if "Rust-Python observation mismatch" in str(_w.message)
        ]
        assert len(equivalence_warnings) == 0, (
            f"unexpected mismatch warnings: {[str(_w.message) for _w in equivalence_warnings]}"
        )


def test_vec_gym_verify_observation_equivalence_flag_defaults_off():
    """VecDwellingGymEnv does not run the verification when flag is not set."""
    import warnings

    dwellings = [_make_dwelling(seed=i) for i in range(2)]
    env = VecDwellingGymEnv(
        dwellings=dwellings,
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_kw"],
        episode_length=timedelta(minutes=5),
    )

    with warnings.catch_warnings(record=True) as w:
        warnings.simplefilter("always")
        for _step_index in range(3):
            env.step(np.full((2, 1), 21.0, dtype=np.float64))
        equivalence_warnings = [
            _w for _w in w
            if "Rust-Python observation mismatch" in str(_w.message)
        ]
        assert len(equivalence_warnings) == 0, (
            f"verification warnings emitted with flag off: "
            f"{[str(_w.message) for _w in equivalence_warnings]}"
        )


def test_vec_gym_verify_observation_equivalence_reports_nan_mismatch():
    """NaN mismatch diagnostic names the divergent field, not a non-divergent one."""
    import warnings
    from unittest.mock import patch

    import hares.rl.vec_env as vec_env_module

    dwellings = [_make_dwelling(seed=0) for _ in range(1)]
    env = VecDwellingGymEnv(
        dwellings=dwellings,
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_kw"],
        episode_length=timedelta(minutes=5),
        verify_observation_equivalence=True,
    )

    # Inject a one-sided NaN at index 0 ("total_power_kw") in the Python path.
    def divergent_telemetry(telemetry, fields):
        result = np.asarray(
            telemetry_to_observation(telemetry, fields), dtype=np.float64
        )
        result[0] = np.nan
        return result

    with warnings.catch_warnings(record=True) as w:
        warnings.simplefilter("always")
        with patch.object(
            vec_env_module, "telemetry_to_observation",
            new=divergent_telemetry,
        ):
            env.step(np.full((1, 1), 21.0, dtype=np.float64))
        mismatch_warnings = [
            _w for _w in w
            if "Rust-Python observation mismatch" in str(_w.message)
        ]
    assert len(mismatch_warnings) == 1, (
        f"expected exactly one mismatch warning, got {len(mismatch_warnings)}: "
        f"{[str(_w.message) for _w in mismatch_warnings]}"
    )
    warning_text = str(mismatch_warnings[0].message)
    assert "total_power_kw" in warning_text, (
        f"warning should name 'total_power_kw', got: {warning_text}"
    )
    assert "NaN mismatch" in warning_text, (
        f"warning should report 'NaN mismatch', got: {warning_text}"
    )


# ---------------------------------------------------------------------------
# Configuration strictness: the gym rejects bad values with typed errors
# instead of coercing them, and forwards the whole typed config.
# ---------------------------------------------------------------------------


def _gym_config(sim_config: SimulationConfig | None = None) -> GymDwellingConfig:
    return GymDwellingConfig(
        hpxml=HPXML,
        schedule=SCHEDULE,
        weather=WEATHER,
        defaults_path=str(HARES_DEFAULTS),
        sim_config=sim_config,
    )


def test_gym_rejects_mapping_config():
    """A mapping config is a TypeError: the gym accepts only its typed config."""
    mapping = cast(GymDwellingConfig, {"hpxml": HPXML, "schedule": SCHEDULE, "weather": WEATHER})
    with pytest.raises(TypeError, match="GymDwellingConfig"):
        DwellingGymEnv(
            config=mapping,
            observation_fields=_OBS_FIELDS,
            action_space_config=_ACTION_CONFIG,
            reward_fn=lambda ctx: -ctx["total_power_kw"],
            episode_length=timedelta(minutes=5),
        )


def test_gym_rejects_non_numeric_time_res():
    """A time_res that is not a number is a TypeError, never a silent
    60 s default."""
    mapping = cast(
        GymDwellingConfig,
        {
            "hpxml": HPXML,
            "schedule": SCHEDULE,
            "weather": WEATHER,
            "defaults_path": str(HARES_DEFAULTS),
            "time_res_s": "600",
        },
    )
    with pytest.raises(TypeError, match="GymDwellingConfig"):
        DwellingGymEnv(
            config=mapping,
            observation_fields=_OBS_FIELDS,
            action_space_config=_ACTION_CONFIG,
            reward_fn=lambda ctx: -ctx["total_power_kw"],
            episode_length=timedelta(minutes=5),
        )


def test_gym_zero_time_res_reaches_validation():
    """A zero resolution raises the engine's resolution error instead of
    being replaced by the default."""
    with pytest.raises(ValueError, match="time_res must be positive"):
        DwellingGymEnv(
            config=_gym_config(SimulationConfig(start_time="2019-01-01T00:00:00", duration=600, time_res=0)),
            observation_fields=_OBS_FIELDS,
            action_space_config=_ACTION_CONFIG,
            reward_fn=lambda ctx: -ctx["total_power_kw"],
            episode_length=timedelta(minutes=5),
        )


def test_gym_zero_master_seed_reaches_the_dwelling():
    """A master_seed of 0 is the seed the built dwelling's config reports,
    not a value the wrapper drops for its default."""
    env = DwellingGymEnv(
        config=_gym_config(
            SimulationConfig(
                start_time="2019-01-01T00:00:00",
                duration=600,
                time_res=60,
                write_output=False,
                master_seed=0,
            )
        ),
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_kw"],
        episode_length=timedelta(minutes=5),
    )
    assert env._dwelling.config().master_seed == 0


def test_gym_rejects_non_positive_episode_length():
    """A non-positive episode_length is a ValueError naming the field and
    the value at construction, never a silent one-step episode."""
    for length in (timedelta(0), timedelta(minutes=-5)):
        with pytest.raises(ValueError, match="episode_length") as caught:
            _make_env(episode_length=length)
        assert str(length) in str(caught.value)


def test_gym_rejects_non_timedelta_episode_length():
    """An episode_length that is not a timedelta is a TypeError naming the
    field and the type, not an attribute error from deep inside."""
    with pytest.raises(TypeError, match="episode_length.*got int"):
        _make_env(episode_length=cast(Any, 300))


def test_gym_rejects_uncallable_reward_fn():
    """A reward_fn that is not callable is a TypeError at construction
    naming the field and the type, not a crash at the first step."""
    with pytest.raises(TypeError, match="reward_fn.*got str"):
        DwellingGymEnv(
            config=_DWELLING_CONFIG,
            observation_fields=_OBS_FIELDS,
            action_space_config=_ACTION_CONFIG,
            reward_fn=cast(Any, "not callable"),
            episode_length=timedelta(minutes=5),
        )


def test_gym_accepts_a_none_reward_fn_with_the_batch_default():
    """A None reward_fn is accepted and steps with the batch path's default
    reward: the negative net electric power of the step."""
    env = DwellingGymEnv(
        config=_DWELLING_CONFIG,
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=None,
        episode_length=timedelta(minutes=5),
    )
    _obs, reward, _terminated, _truncated, info = env.step(
        np.array([21.0], dtype=np.float64)
    )
    assert reward == -info["step"]["net_electric_power_kw"]


def test_gym_rejects_bare_string_observation_fields():
    """A bare string observation_fields would iterate per character: it is
    a TypeError naming the field and the value."""
    with pytest.raises(TypeError, match="observation_fields.*got str") as caught:
        DwellingGymEnv(
            config=_DWELLING_CONFIG,
            observation_fields=cast(Any, "total_power_kw"),
            action_space_config=_ACTION_CONFIG,
            reward_fn=lambda ctx: -ctx["total_power_kw"],
            episode_length=timedelta(minutes=5),
        )
    assert "total_power_kw" in str(caught.value)


def test_gym_rejects_a_non_string_observation_field():
    """An observation_fields element that is not a string is a TypeError
    naming the field and the element."""
    with pytest.raises(TypeError, match="observation_fields") as caught:
        DwellingGymEnv(
            config=_DWELLING_CONFIG,
            observation_fields=cast(Any, ["total_power_kw", 3]),
            action_space_config=_ACTION_CONFIG,
            reward_fn=lambda ctx: -ctx["total_power_kw"],
            episode_length=timedelta(minutes=5),
        )
    assert "got element 3 of type int" in str(caught.value)


def test_gym_forwards_every_sim_config_field(tmp_path: Path):
    """A gym built with a SimulationConfig whose every field differs from its
    default, write_output=False among them, records no output, and the config
    read back from the built dwelling equals the one passed on every field
    the class exposes.

    civil_timezone stays at its default here: the engine honours it only in
    a dst build, and the gate's extension is a plain build. Its getter is
    pinned at the config surface in test_py_config.py.
    """
    sim_config = SimulationConfig(
        start_time="2019-03-04T05:06:07",
        duration=7200,
        time_res=900,
        output_verbosity=2,
        output_path=str(tmp_path / "gym_output.csv"),
        write_output=False,
        output_format="parquet",
        output_chunk_size=1234,
        master_seed=7,
        setpoint_deadband_c=1.5,
        site_location=SiteLocationOverride(
            latitude_deg=40.0,
            longitude_deg=-105.0,
            elevation_m=1600.0,
            utc_offset_h=-7.0,
        ),
        retain_batches=False,
        rotation="daily",
        max_consecutive_step_failures=3,
    )
    env = DwellingGymEnv(
        config=_gym_config(sim_config),
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_kw"],
        episode_length=timedelta(minutes=5),
    )
    built = env._dwelling.config()
    for field in (
        "start_time",
        "duration",
        "time_res",
        "output_verbosity",
        "output_path",
        "write_output",
        "output_format",
        "output_chunk_size",
        "master_seed",
        "civil_timezone",
        "setpoint_deadband_c",
        "retain_batches",
        "rotation",
        "max_consecutive_step_failures",
    ):
        passed = getattr(sim_config, field)
        reported = getattr(built, field)
        assert reported == passed, (
            f"{field}: passed {passed!r}, the dwelling reports {reported!r}"
        )
    # The site override rides as the one field, every component reported.
    for field in ("latitude_deg", "longitude_deg", "elevation_m", "utc_offset_h"):
        passed = getattr(sim_config.site_location, field)
        reported = getattr(built.site_location, field)
        assert reported == passed, (
            f"site_location.{field}: passed {passed!r}, the dwelling reports {reported!r}"
        )
    assert not (tmp_path / "gym_output.csv").exists(), (
        "the gym recorded output with write_output=False"
    )


# ---------------------------------------------------------------------------
# time_res_s configurability (T-0518)
# ---------------------------------------------------------------------------


def test_vec_gym_time_res_s_explicit_30():
    """VecDwellingGymEnv with time_res_s=30 computes _max_steps accordingly."""
    dwellings = [_make_dwelling(seed=0) for _ in range(2)]
    episode_len = timedelta(minutes=5)
    env = VecDwellingGymEnv(
        dwellings=dwellings,
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_kw"],
        episode_length=episode_len,
        time_res_s=30.0,
    )
    expected = max(1, int(math.ceil(episode_len.total_seconds() / 30.0)))
    assert env._max_steps == expected, (
        f"expected _max_steps={expected}, got {env._max_steps}"
    )
    assert env._time_res_s == 30.0


def test_vec_gym_time_res_s_explicit_60_regression():
    """VecDwellingGymEnv with time_res_s=60 matches old hardcoded behavior."""
    dwellings = [_make_dwelling(seed=0) for _ in range(2)]
    episode_len = timedelta(minutes=5)
    env = VecDwellingGymEnv(
        dwellings=dwellings,
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_kw"],
        episode_length=episode_len,
        time_res_s=60.0,
    )
    expected = max(1, int(math.ceil(episode_len.total_seconds() / 60.0)))
    assert env._max_steps == expected
    assert env._time_res_s == 60.0


def test_vec_gym_time_res_s_none_uses_dwelling_config():
    """time_res_s=None reads from the first dwelling's config without warning."""
    import warnings

    dwellings = [_make_dwelling(seed=0) for _ in range(2)]
    episode_len = timedelta(minutes=5)

    with warnings.catch_warnings(record=True) as w:
        warnings.simplefilter("always")
        env = VecDwellingGymEnv(
            dwellings=dwellings,
            observation_fields=_OBS_FIELDS,
            action_space_config=_ACTION_CONFIG,
            reward_fn=lambda ctx: -ctx["total_power_kw"],
            episode_length=episode_len,
            time_res_s=None,
        )

    # _make_dwelling passes time_res_s=60, so the dwelling config should have it.
    assert env._time_res_s == 60.0
    expected = max(1, int(math.ceil(episode_len.total_seconds() / 60.0)))
    assert env._max_steps == expected

    fallback_warnings = [
        x for x in w
        if issubclass(x.category, UserWarning) and "time_res_s" in str(x.message)
    ]
    assert len(fallback_warnings) == 0, (
        f"expected no fallback warning, got: {[str(x.message) for x in fallback_warnings]}"
    )


def test_vec_gym_time_res_s_none_fallback_warning():
    """time_res_s=None with an object lacking config() produces fallback warning."""
    import warnings

    class _DwellingLike:
        def initialize(self) -> None:
            pass

        def save_state(self) -> bytes:
            return b"{}"

    dwellings: list = [_DwellingLike() for _ in range(2)]
    episode_len = timedelta(minutes=5)

    with warnings.catch_warnings(record=True) as w:
        warnings.simplefilter("always")
        env = VecDwellingGymEnv(
            dwellings=dwellings,
            observation_fields=_OBS_FIELDS,
            action_space_config=_ACTION_CONFIG,
            reward_fn=lambda ctx: -ctx["total_power_kw"],
            episode_length=episode_len,
            time_res_s=None,
        )

    fallback_warnings = [
        x for x in w
        if issubclass(x.category, UserWarning) and "time_res_s" in str(x.message)
    ]
    assert len(fallback_warnings) == 1, (
        f"expected exactly one fallback warning, got {len(fallback_warnings)}: "
        f"{[str(x.message) for x in fallback_warnings]}"
    )
    assert env._time_res_s == 60.0
    expected = max(1, int(math.ceil(episode_len.total_seconds() / 60.0)))
    assert env._max_steps == expected


def test_vec_gym_time_res_s_zero_raises():
    """time_res_s=0.0 raises ValueError instead of ZeroDivisionError."""
    dwellings = [_make_dwelling(seed=0) for _ in range(2)]
    with pytest.raises(ValueError, match=r"time_res_s must be positive"):
        VecDwellingGymEnv(
            dwellings=dwellings,
            observation_fields=_OBS_FIELDS,
            action_space_config=_ACTION_CONFIG,
            reward_fn=lambda ctx: -ctx["total_power_kw"],
            episode_length=timedelta(minutes=5),
            time_res_s=0.0,
        )


def test_vec_gym_time_res_s_negative_raises():
    """time_res_s=-30.0 raises ValueError instead of silent wrong _max_steps."""
    dwellings = [_make_dwelling(seed=0) for _ in range(2)]
    with pytest.raises(ValueError, match=r"time_res_s must be positive"):
        VecDwellingGymEnv(
            dwellings=dwellings,
            observation_fields=_OBS_FIELDS,
            action_space_config=_ACTION_CONFIG,
            reward_fn=lambda ctx: -ctx["total_power_kw"],
            episode_length=timedelta(minutes=5),
            time_res_s=-30.0,
        )


def test_vec_gym_rejects_non_positive_episode_length():
    """A non-positive episode_length is a ValueError naming the field and
    the value at construction, never a silent one-step episode."""
    dwellings = [_make_dwelling(seed=0) for _ in range(2)]
    for length in (timedelta(0), timedelta(minutes=-5)):
        with pytest.raises(ValueError, match="episode_length") as caught:
            VecDwellingGymEnv(
                dwellings=dwellings,
                observation_fields=_OBS_FIELDS,
                action_space_config=_ACTION_CONFIG,
                reward_fn=lambda ctx: -ctx["total_power_kw"],
                episode_length=length,
            )
        assert str(length) in str(caught.value)


def test_vec_gym_rejects_uncallable_reward_fn():
    """A reward_fn that is not callable is a TypeError at construction
    naming the field and the type."""
    dwellings = [_make_dwelling(seed=0) for _ in range(2)]
    with pytest.raises(TypeError, match="reward_fn.*got str"):
        VecDwellingGymEnv(
            dwellings=dwellings,
            observation_fields=_OBS_FIELDS,
            action_space_config=_ACTION_CONFIG,
            reward_fn=cast(Any, "not callable"),
            episode_length=timedelta(minutes=5),
        )


def test_vec_gym_rejects_bare_string_observation_fields():
    """A bare string observation_fields would iterate per character: it is
    a TypeError naming the field and the value."""
    dwellings = [_make_dwelling(seed=0) for _ in range(2)]
    with pytest.raises(TypeError, match="observation_fields.*got str") as caught:
        VecDwellingGymEnv(
            dwellings=dwellings,
            observation_fields=cast(Any, "total_power_kw"),
            action_space_config=_ACTION_CONFIG,
            reward_fn=lambda ctx: -ctx["total_power_kw"],
            episode_length=timedelta(minutes=5),
        )
    assert "total_power_kw" in str(caught.value)


# ---------------------------------------------------------------------------
# Action bounds: the engine's table is the canonical one, the Python-side
# table pins it, and clamped actions are counted in every build.
# ---------------------------------------------------------------------------


def test_python_action_bounds_match_the_engine():
    """The engine's exposed bounds table and the Python-side table name the
    same fields with the same bounds in both directions, so growth on
    either side fails the pin, and both fall through to the same broad
    bounds for a field neither names, case-insensitively."""
    from hares._hares import field_bounds as rust_field_bounds
    from hares._hares import field_bounds_table as rust_field_bounds_table
    from hares.rl.gym_env import _ACTION_FIELD_BOUNDS

    engine_table = rust_field_bounds_table()
    assert engine_table, "the engine's table must not be empty"
    for field, engine in engine_table.items():
        python = field_bounds(field)
        assert python == engine, f"{field}: python {python}, engine {engine}"
    for field, python in _ACTION_FIELD_BOUNDS.items():
        engine = rust_field_bounds(field)
        assert engine == python, f"{field}: engine {engine}, python {python}"
    for name in ("HEAT_C", "Mystery_Field"):
        assert field_bounds(name) == rust_field_bounds(name), name
