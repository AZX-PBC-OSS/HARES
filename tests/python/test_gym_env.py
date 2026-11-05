"""Tests for DwellingGymEnv and VecDwellingGymEnv."""

from __future__ import annotations

from datetime import timedelta
import struct

from hares import Destination, ReportSpec, Selection, SimulationConfig, SiteLocationOverride
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
from hares._hares import batch_step as extension_batch_step
from hares import Actor
from hares.rl.gym_env import (
    DwellingGymEnv,
    GymDwellingConfig,
    RewardContext,
    field_bounds,
    observation_field_bounds,
    sorted_action_layout,
    validated_config_actors,
)
from hares.rl.vec_env import VecDwellingGymEnv

ROOT = Path(__file__).resolve().parents[2]
HARES_DEFAULTS = ROOT / "python" / "hares" / "defaults"
HPXML = str(ROOT / "tests/fixtures/hpxml/ochre_samples/base.xml")
WEATHER = str(ROOT / "data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw")
SCHEDULE = str(ROOT / "data/examples/BEopt_example_schedule.csv")


@pytest.fixture(autouse=True)
def _unresolved_start_method():
    """The fork guard reads the process's resolved start method, and any
    sibling suite's multiprocessing use resolves the Linux default (fork)
    into the process state the guard then reads as explicit policy: the
    guard cannot tell a resolved default from a set one. The vec tests
    reset the resolution before each test, so the guard reads the
    process's own policy alone; the fork test's monkeypatched reader is
    untouched by this."""
    import multiprocessing

    multiprocessing.set_start_method(None, force=True)
    yield

_DWELLING_CONFIG = GymDwellingConfig(
    hpxml=HPXML,
    schedule=SCHEDULE,
    weather=WEATHER,
    defaults=str(HARES_DEFAULTS),
    sim_config=SimulationConfig(
        start_time="2019-01-01T00:00:00",
        duration=600,
        time_res=60,
    ),
)


def _obs_box(env: DwellingGymEnv | VecDwellingGymEnv) -> spaces.Box:
    """The observation space as a Box (gymnasium is an importorskip here)."""
    space = env.observation_space
    assert isinstance(space, spaces.Box), "observation_space must be a Box"
    return space

# Gas Furnace is present in the base HPXML; heat_c resolves to ThermalSetpoint.
_ACTION_CONFIG: dict[str, list[str]] = {"Gas Furnace": ["heat_c"]}
_OBS_FIELDS = ["total_power_w", "outdoor_temp"]


def _make_env(episode_length: timedelta = timedelta(minutes=5)) -> DwellingGymEnv:
    return DwellingGymEnv(
        config=_DWELLING_CONFIG,
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_w"],
        episode_length=episode_length,
    )


def _make_dwelling(seed: int = 0, duration_s: int = 600) -> PyDwelling:
    return PyDwelling.from_hpxml(hpxml=HPXML, schedule=SCHEDULE, weather=WEATHER, defaults=str(HARES_DEFAULTS), config=SimulationConfig(start_time="2019-01-01T00:00:00", duration=duration_s, time_res=60, master_seed=seed))


def test_gym_reset_restores_snapshot_and_reseeds():
    """The gym's reset restores the warmed-up state and reseeds the streams
    without rebuilding: two reset(seed=7) calls, each followed by the same
    96 actions, give bitwise-equal observations and rewards, and the
    restored first observation is the same whatever the episode's seed."""
    fixture = ROOT / "tests/fixtures/parity/cz5a_ev_charging"
    dwelling = PyDwelling.from_hpxml(
        hpxml=str(fixture / "building.xml"),
        schedule=str(fixture / "schedule.csv"),
        weather=str(fixture / "weather.epw"),
        defaults=str(HARES_DEFAULTS),
        config=SimulationConfig(
            start_time="2023-01-01T00:00:00-07:00",
            duration=2 * 86_400,
            time_res=900,
            master_seed=42,
        ),
    )
    env = VecDwellingGymEnv(
        dwellings=[dwelling],
        observation_fields=[
            "total_power_w",
            "outdoor_temp",
            "zone_temp[Indoor]",
            "time_sin",
            "time_cos",
        ],
        action_space_config={"ASHP Heater": ["heat_c"]},
        reward_fn=lambda ctx: -ctx["total_power_w"],
        episode_length=timedelta(days=2),
    )

    def run_episode(seed: int):
        obs0, _info = env.reset(seed=seed)
        observations = [obs0.copy()]
        rewards = []
        action = np.full((1, env._action_dim), 21.0, dtype=np.float64)
        for _ in range(96):
            obs, reward, _terminated, _truncated, _info = env.step(action)
            observations.append(obs.copy())
            rewards.append(reward.copy())
        return observations, rewards

    first_obs, first_rewards = run_episode(7)
    second_obs, second_rewards = run_episode(7)
    assert len(first_obs) == 97
    for left, right in zip(first_obs, second_obs, strict=True):
        assert left.tobytes() == right.tobytes(), "bitwise-equal observations"
    for left, right in zip(first_rewards, second_rewards, strict=True):
        assert left.tobytes() == right.tobytes(), "bitwise-equal rewards"

    # The restored state is the observation's source: the first
    # observation is the same whatever the episode's seed.
    obs7, _info7 = env.reset(seed=7)
    obs8, _info8 = env.reset(seed=8)
    assert obs7.tobytes() == obs8.tobytes(), (
        "the reset restores the warmed-up state, the same first "
        "observation under any seed"
    )


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
        reward_fn=lambda ctx: -ctx["total_power_w"],
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
            reward_fn=lambda ctx: -ctx["total_power_w"],
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
        reward_fn=lambda ctx: -ctx["total_power_w"],
        episode_length=timedelta(minutes=5),
    )
    column = env._action_layout.index(("Gas Furnace", "deadband_c"))
    assert env._action_bounds[column] == (MIN_THERMOSTAT_BAND_C, MAX_HVAC_THERMOSTAT_BAND_C)
    assert [field for _, field in env._action_layout] == ["deadband_c", "heat_c"]
    _, _, _, _, infos = env.step(np.asarray([[30.0, 21.0], [30.0, 21.0]]))
    assert [info["warning_count"] for info in infos] == [0, 0], "the band is clipped"


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
            reward_fn=lambda ctx: -ctx["total_power_w"],
            episode_length=timedelta(minutes=5),
        )


def test_deadband_action_without_a_setpoint_field_is_rejected():
    with pytest.raises(ValueError, match="no setpoint field"):
        sorted_action_layout({"HVAC": ["deadband_c"]})
    layout, _ = sorted_action_layout({"HVAC": ["deadband_c", "heat_c"]})
    assert [f for _, f in layout] == ["deadband_c", "heat_c"]


def test_sorted_action_layout_preserves_equipment_sort_order():
    layout, type_by_equip = sorted_action_layout(
        {"B": ["reactive_power_var", "active_power_w"], "A": ["active_power_w"]}
    )

    equipment_names = [eq for eq, _ in layout]
    assert equipment_names[0] == "A"
    assert equipment_names[1] == "B"
    assert equipment_names[2] == "B"

    a_fields = [f for eq, f in layout if eq == "A"]
    assert a_fields == ["active_power_w"]

    b_fields = [f for eq, f in layout if eq == "B"]
    assert b_fields == sorted(["reactive_power_var", "active_power_w"])

    assert type_by_equip["A"] == "PowerSetpoint"
    assert type_by_equip["B"] == "PowerSetpoint"


def test_vec_gym_step_batch_shape():
    dwellings = [_make_dwelling(seed=i) for i in range(4)]
    env = VecDwellingGymEnv(
        dwellings=dwellings,
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_w"],
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
        assert "obs_total_power_w_low" in info
        assert "obs_total_power_w_high" in info
        assert "obs_outdoor_temp_low" in info
        assert "obs_outdoor_temp_high" in info
        assert np.isfinite(info["obs_total_power_w_low"])
        assert np.isfinite(info["obs_total_power_w_high"])


def test_vec_gym_reset_seed_batch_shape():
    dwellings = [_make_dwelling(seed=i) for i in range(4)]
    env = VecDwellingGymEnv(
        dwellings=dwellings,
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_w"],
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
    obs_fields = ["total_power_w", "outdoor_temp"]
    dwellings = [
        PyDwelling.from_hpxml(hpxml=HPXML, schedule=SCHEDULE, weather=WEATHER, defaults=str(HARES_DEFAULTS), config=SimulationConfig(start_time="2019-01-01T00:00:00", duration=6000, time_res=60, master_seed=i))
        for i in range(n_dwel)
    ]
    for dw in dwellings:
        dw.add_battery(Battery("Batt", capacity_j=36000000, max_charge_w=5000, max_discharge_w=5000))

    action_config = {
        "Gas Furnace": ["heat_c"],
        "Batt": ["active_power_w"],
    }

    env = VecDwellingGymEnv(
        dwellings=dwellings,
        observation_fields=obs_fields,
        action_space_config=action_config,
        reward_fn=lambda ctx: -ctx["total_power_w"],
        episode_length=timedelta(seconds=6000),
    )

    rng = np.random.default_rng(42)
    obs_fields = ["total_power_w", "outdoor_temp"]
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
    total_power_w."""
    import hares.rl.vec_env as vec_env_module

    def zone_power_reward(ctx: RewardContext) -> float:
        zone_temps = ctx["telemetry_zone"]["temperature_c"]
        return ctx["total_power_w"] - 0.01 * float(zone_temps[0])

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
    """A reward_fn that is not callable and not None fails at
    construction, naming the field, before any dwelling is initialized or
    advanced; None is the default reward's own form now (the same default
    the single env pays, pinned bitwise equal across the paths)."""
    with pytest.raises(TypeError, match="reward_fn"):
        VecDwellingGymEnv(
            dwellings=[_make_dwelling()],
            observation_fields=_OBS_FIELDS,
            action_space_config=_ACTION_CONFIG,
            reward_fn=cast(Any, "not a reward"),
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
            reward_fn=lambda ctx: -ctx["total_power_w"],
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
        assert "net_electric_power_w" in batch_info["step"]
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
        reward_fn=lambda ctx: -ctx["total_power_w"],
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
    obs_fields = ["total_power_w", "outdoor_temp"]
    dwellings = [
        PyDwelling.from_hpxml(hpxml=HPXML, schedule=SCHEDULE, weather=WEATHER, defaults=str(HARES_DEFAULTS), config=SimulationConfig(start_time="2019-01-01T00:00:00", duration=6000, time_res=60, master_seed=i))
        for i in range(n_dwel)
    ]
    from hares import Battery
    for dw in dwellings:
        dw.add_battery(Battery("Batt", capacity_j=36000000, max_charge_w=5000, max_discharge_w=5000))

    action_config = {
        "Gas Furnace": ["heat_c"],
        "Batt": ["active_power_w"],
    }

    env = VecDwellingGymEnv(
        dwellings=dwellings,
        observation_fields=obs_fields,
        action_space_config=action_config,
        reward_fn=lambda ctx: -ctx["total_power_w"],
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
        "outdoor_humidity_ratio",
        "total_power_w",
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
    produce shape mismatches, dtype errors, or exceptions: failures that
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
        reward_fn=lambda ctx: -ctx["total_power_w"],
        episode_length=timedelta(minutes=5),
        field_bounds_overrides={"total_power_w": (-200.0, 200.0)},
    )
    low = _obs_box(env).low
    high = _obs_box(env).high
    total_power_idx = _OBS_FIELDS.index("total_power_w")
    assert np.isclose(float(low[total_power_idx]), -200.0), f"override low not applied: {low}"
    assert np.isclose(float(high[total_power_idx]), 200.0), f"override high not applied: {high}"


def test_observation_field_bounds_case_insensitive():
    """Bracket-prefixed fields match regardless of case or leading whitespace."""
    tests = [
        ("zone_temp[Living Room]", (-80.0, 50.0)),
        ("Zone_Temp[Living Room]", (-80.0, 50.0)),
        (" ZONE_TEMP[Kitchen]", (-80.0, 50.0)),
        ("setpoint_heat[Main]", (0.0, 50.0)),
        ("Setpoint_Heat[Main]", (0.0, 50.0)),
        (" setpoint_cool[Upstairs]", (0.0, 50.0)),
        ("equipment_soc[Battery]", (0.0, 1.0)),
        ("Equipment_Soc[Battery]", (0.0, 1.0)),
        (" equipment_power[Gas Furnace]", (-100000.0, 100000.0)),
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
        reward_fn=lambda ctx: -ctx["total_power_w"],
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
        reward_fn=lambda ctx: -ctx["total_power_w"],
        episode_length=timedelta(minutes=5),
        field_bounds_overrides={"total_power_w": (-200.0, 200.0)},
    )
    _, _, _, _, infos = env.step(
        np.full((2, 1), 21.0, dtype=np.float64)
    )
    for info in infos:
        assert np.isclose(
            float(info["obs_total_power_w_low"]), -200.0
        ), f"override low not in info: {info}"
        assert np.isclose(
            float(info["obs_total_power_w_high"]), 200.0
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
    """time_sin and time_cos produce finite, in-bounds values through the env's frozen plan."""
    env = DwellingGymEnv(
        config=_DWELLING_CONFIG,
        observation_fields=["time_sin", "time_cos"],
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_w"],
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
        observation_fields=["time_sin", "time_cos", "total_power_w"],
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_w"],
        episode_length=timedelta(minutes=5),
    )
    low = np.asarray(_obs_box(env).low, dtype=np.float64)
    high = np.asarray(_obs_box(env).high, dtype=np.float64)
    assert np.all(np.isfinite(low)), f"non-finite low: {low}"
    assert np.all(np.isfinite(high)), f"non-finite high: {high}"
    assert np.isclose(float(low[0]), -1.0)
    assert np.isclose(float(high[0]), 1.0)


# ---------------------------------------------------------------------------
# actor_telemetry resolution through the env's frozen plan: the plan is
# the one observation vocabulary, so the actor channels are pinned through
# both environments, dot and bare name, with the built-ins winning.
# ---------------------------------------------------------------------------


def test_actor_channels_resolve_through_both_envs_dot_and_bare():
    """An actor's published channels resolve through the env's plan on
    both gym environments: the dot form names the actor, the bare form
    searches the actors in sorted-name order."""
    for env in _both_envs(
        observation_fields=[cast(Any, "DRAgent.dr_level"), cast(Any, "dr_active")],
        actors=[{"actor_type": "DrCompliance", "name": "DRAgent"}],
    ):
        env.reset(seed=0)
        obs, _r, _term, _trunc, _info = env.step(_no_action(env))
        row = obs[0] if obs.ndim == 2 else obs
        assert row.shape == (2,)
        assert row[0] == 0.0, f"dr_level should be 0.0 for Normal DR level, got {row[0]}"
        assert row[1] == 0.0, f"dr_active should be 0.0 when no DR event, got {row[1]}"


def test_env_builtin_field_wins_over_an_actor_channel_of_the_same_name():
    """A built-in field takes priority over an actor channel of the same
    name: the plan resolves the engine's own source, and the actor's
    publication of that name never reaches the observation."""
    from hares import OutputVariable

    class PowerPublisher(Actor):
        def __init__(self):
            super().__init__()
            self.name = "PowerPublisher"

        def output_variables(self):
            return [OutputVariable("total_power_w", "W", "power")]

        def decide(self, env):
            self.set_output("total_power_w", 7.5)
            return []

    for env in _both_envs(
        observation_fields=[cast(Any, "total_power_w")],
        actors=[PowerPublisher()],
    ):
        env.reset(seed=0)
        obs, _r, _term, _trunc, _info = env.step(_no_action(env))
        row = obs[0] if obs.ndim == 2 else obs
        assert row.shape == (1,)
        assert row[0] != 7.5, "the built-in total_power_w must not read the actor's channel"
        assert row[0] != 0.0, "the built-in total_power_w reads the engine's own value"


# ---------------------------------------------------------------------------
# Initial observation NaN & initialized flag tests (T-0341)
# ---------------------------------------------------------------------------


def test_reset_observation_contains_nan_for_uninitialized():
    """After reset(), pre-first-step observation contains NaN for uninitialized fields."""
    env = DwellingGymEnv(
        config=_DWELLING_CONFIG,
        observation_fields=["total_power_w", "outdoor_temp", "outdoor_humidity_ratio", "time_sin", "time_cos", "equipment_soc[Gas Furnace]"],
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_w"],
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


def test_step_before_reset_omits_the_seed():
    """A step taken before the first ``reset`` carries no ``seed``: the
    episode's seed exists once a reset assigns one. Every post-reset step
    carries it (``reset(seed=None)`` draws one too), so this omission is
    the pre-reset branch's own, and the warning count rides the same info
    as an int."""
    env = _make_env()
    _, _, _, _, info = env.step(np.array([21.0], dtype=np.float64))
    assert "seed" not in info, "a pre-reset step has no episode seed to report"
    assert isinstance(info["warning_count"], int)


def test_reset_without_a_seed_reports_the_drawn_seed_on_every_info():
    """``reset(seed=None)`` draws the episode's seed, so the reset info
    carries it and the next step's info carries the same one."""
    env = _make_env()
    _, reset_info = env.reset()
    assert isinstance(reset_info["seed"], int)
    _, _, _, _, step_info = env.step(np.array([21.0], dtype=np.float64))
    assert step_info["seed"] == reset_info["seed"]


def test_step_observation_all_finite_after_first_step():
    """After the first step(), all observation dimensions must be finite (no lingering NaN)."""
    env = DwellingGymEnv(
        config=_DWELLING_CONFIG,
        observation_fields=["total_power_w", "outdoor_temp", "outdoor_humidity_ratio", "time_sin", "time_cos", "equipment_soc[Gas Furnace]"],
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_w"],
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
        observation_fields=["total_power_w", "outdoor_temp", "outdoor_humidity_ratio", "reactive_power_var"],
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_w"],
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
        reward_fn=lambda ctx: -ctx["total_power_w"],
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
# The frozen observation plan: the batch's observations are read into the
# environment's own buffer through the plan, in place. The plan is the one
# observation vocabulary; every key the bounds table recognizes is pinned
# through both environments here.
# ---------------------------------------------------------------------------


def _no_action(env) -> np.ndarray:
    """The env's zero-width action: the single env steps on a `(0,)`
    array, the vec env on one row per env, `(n_envs, 0)`."""
    n = getattr(env, "num_envs", 1)
    return np.zeros((n, 0), dtype=np.float64) if n > 1 or isinstance(env, VecDwellingGymEnv) else np.zeros(0, dtype=np.float64)


def _both_envs(observation_fields: list[Any], actors: list[Any] | None = None):
    """The single env and a one-env vec env over the same fixture: the
    same field list resolves and reads through both."""
    single = DwellingGymEnv(
        config=_config_with_actors(actors),
        observation_fields=observation_fields,
        action_space_config={},
        reward_fn=lambda ctx: -ctx["total_power_w"],
        episode_length=timedelta(minutes=5),
    )
    dwelling = _make_dwelling()
    dwelling.initialize()
    for entry in validated_config_actors(actors):
        if entry[0] == "registry":
            _kind, actor_type, name, parameters = entry
            dwelling.add_actor_by_name(actor_type, name, parameters)
        else:
            dwelling.add_actor(entry[1])
    vector = VecDwellingGymEnv(
        dwellings=[dwelling],
        observation_fields=observation_fields,
        action_space_config={},
        reward_fn=lambda ctx: -ctx["total_power_w"],
        episode_length=timedelta(minutes=5),
    )
    return single, vector


def test_env_resolves_every_bounds_table_key():
    """Every scalar key the bounds table recognizes resolves through the
    plan and reads a finite value on both gym environments, the alias and
    the actor channels included."""
    fields = [
        "outdoor_temp",
        "outdoor_temp_c",
        "outdoor_humidity_ratio",
        "total_power_w",
        "reactive_power_var",
        "grid_interchange_w",
        "islanded",
        "bus_voltage_pu",
        "zone_temp[Indoor]",
        "setpoint_heat[Indoor]",
        "setpoint_cool[Indoor]",
        "zone_energy_balance[Indoor]",
        "time_sin",
        "time_cos",
        "schedule_row",
        "occupants_present",
        "clock_hour",
        "clock_day",
        "clock_step",
        "DRAgent.dr_level",
        "dr_active",
    ]
    for env in _both_envs(
        [cast(Any, f) for f in fields],
        actors=[{"actor_type": "DrCompliance", "name": "DRAgent"}],
    ):
        env.reset(seed=0)
        obs, _r, _term, _trunc, _info = env.step(_no_action(env))
        row = obs[0] if obs.ndim == 2 else obs
        assert row.shape == (len(fields),)
        assert np.all(np.isfinite(row)), f"non-finite observation: {row}"


def test_env_equipment_keys_resolve_for_every_roster_unit():
    """The equipment bracket keys resolve for every unit the dwelling
    carries, on both gym environments: each planned field reads that
    unit's own telemetry power and SOC."""
    probe = _make_dwelling()
    probe.initialize()
    names = list(probe.telemetry().equipment()["names"])
    fields = [cast(Any, f"equipment_power[{name}]") for name in names]
    fields += [cast(Any, f"equipment_soc[{name}]") for name in names]
    for env in _both_envs(fields):
        env.reset(seed=0)
        obs, _r, _term, _trunc, _info = env.step(_no_action(env))
        row = obs[0] if obs.ndim == 2 else obs
        assert row.shape == (len(fields),)
        assert np.all(np.isfinite(row)), f"non-finite observation: {row}"
        unit = env.dwelling if isinstance(env, DwellingGymEnv) else env.envs[0]
        equipment = unit.telemetry().equipment()
        powers = list(equipment["power_w"])
        for idx, name in enumerate(names):
            expected = powers[equipment["names"].index(name)]
            assert struct.pack("<d", row[idx]) == struct.pack("<d", float(expected)), (
                f"equipment_power[{name}] must read the unit's own telemetry power"
            )


def test_outdoor_temp_c_alias_reads_the_canonical_value():
    """The telemetry dict's `outdoor_temp_c` spelling resolves to the same
    source the plan's canonical `outdoor_temp` reads: one value, on
    bits, through both environments."""
    for env in _both_envs([cast(Any, "outdoor_temp"), cast(Any, "outdoor_temp_c")]):
        env.reset(seed=0)
        obs, _r, _term, _trunc, _info = env.step(_no_action(env))
        row = obs[0] if obs.ndim == 2 else obs
        assert struct.pack("<d", row[0]) == struct.pack("<d", row[1]), (
            "the alias must read the same value as the canonical key"
        )


def test_outdoor_rh_is_retired_from_the_vocabulary():
    """`outdoor_rh` is retired everywhere: the plan refuses it on both gym
    environments, naming the key, as the Rust telemetry's own rejection
    does."""
    with pytest.raises(Exception, match="outdoor_rh"):
        DwellingGymEnv(
            config=_DWELLING_CONFIG,
            observation_fields=[cast(Any, "outdoor_rh")],
            action_space_config={},
            reward_fn=lambda ctx: -ctx["total_power_w"],
            episode_length=timedelta(minutes=5),
        ).reset(seed=0)
    with pytest.raises(Exception, match="outdoor_rh"):
        VecDwellingGymEnv(
            dwellings=[_make_dwelling()],
            observation_fields=[cast(Any, "outdoor_rh")],
            action_space_config={},
            reward_fn=lambda ctx: -ctx["total_power_w"],
            episode_length=timedelta(minutes=5),
        ).reset(seed=0)


def test_unknown_observation_field_fails_at_reset():
    """An unknown observation field fails the plan's build at reset,
    naming the field, on both gym environments: the field list is frozen
    per episode, so a name the dwelling cannot answer never reaches a
    step."""
    dwellings = [_make_dwelling(seed=0)]
    env = VecDwellingGymEnv(
        dwellings=dwellings,
        observation_fields=["total_power_w", "bogus_field"],
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_w"],
        episode_length=timedelta(minutes=5),
    )
    with pytest.raises(Exception, match="bogus_field"):
        env.reset(seed=0)

    single = DwellingGymEnv(
        config=_DWELLING_CONFIG,
        observation_fields=["total_power_w", "bogus_field"],
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_w"],
        episode_length=timedelta(minutes=5),
    )
    with pytest.raises(Exception, match="bogus_field"):
        single.reset(seed=0)


def _roster(dwelling: PyDwelling) -> tuple[list[str], list[float]]:
    """The dwelling's equipment names and their power readings, in the
    dwelling order the observation plan and the telemetry share."""
    equipment = dwelling.telemetry().equipment()
    return list(equipment["names"]), list(equipment["power_w"])


def test_single_env_observation_reads_a_surviving_unit_under_its_own_name():
    """The frozen plan binds equipment by its never-reused id: removing a
    unit ahead of a planned one leaves the planned field on the surviving
    unit's own value, never on whatever took its roster slot."""
    env = DwellingGymEnv(
        config=_DWELLING_CONFIG,
        observation_fields=[cast(Any, "equipment_power[Gas Furnace]")],
        action_space_config={},
        reward_fn=lambda ctx: -ctx["total_power_w"],
        episode_length=timedelta(minutes=5),
    )
    env.reset(seed=0)
    names, _powers = _roster(env.dwelling)
    # A plan on the second unit, the removal of the first: the plan froze
    # the second unit's identity, so the removal must not touch its read.
    env = DwellingGymEnv(
        config=_DWELLING_CONFIG,
        observation_fields=[cast(Any, f"equipment_power[{names[1]}]")],
        action_space_config={},
        reward_fn=lambda ctx: -ctx["total_power_w"],
        episode_length=timedelta(minutes=5),
    )
    env.reset(seed=0)
    env.dwelling.remove_equipment(names[0])

    obs, _reward, _term, _trunc, _info = env.step(np.zeros(0, dtype=np.float64))
    names_after, powers_after = _roster(env.dwelling)
    assert names_after[0] == names[1], "the surviving unit keeps its name"
    # Bit-for-bit the unit's own value: struct-level equality, so even a
    # signed-zero or NaN difference cannot pass.
    assert struct.pack("<d", obs[0]) == struct.pack("<d", float(powers_after[0])), (
        f"equipment_power[{names[1]}] must read the surviving unit's own "
        f"power ({powers_after[0]}), not the shifted roster's"
    )


def test_single_env_observation_refuses_a_removed_equipment_field():
    """A unit the plan named that has left the roster is a typed error
    naming the field and the id, on the single env's step: never a panic
    out of bounds, never a silent read of the shifted unit."""
    env = DwellingGymEnv(
        config=_DWELLING_CONFIG,
        observation_fields=[cast(Any, "equipment_power[Gas Furnace]")],
        action_space_config={},
        reward_fn=lambda ctx: -ctx["total_power_w"],
        episode_length=timedelta(minutes=5),
    )
    env.reset(seed=0)
    names, _powers = _roster(env.dwelling)
    env = DwellingGymEnv(
        config=_DWELLING_CONFIG,
        observation_fields=[
            cast(Any, f"equipment_power[{names[0]}]"),
            cast(Any, f"equipment_power[{names[2]}]"),
        ],
        action_space_config={},
        reward_fn=lambda ctx: -ctx["total_power_w"],
        episode_length=timedelta(minutes=5),
    )
    env.reset(seed=0)
    env.dwelling.remove_equipment(names[0])

    with pytest.raises(Exception, match="no longer in the dwelling's equipment roster"):
        env.step(np.zeros(0, dtype=np.float64))


def test_batch_step_refuses_a_removed_equipment_field_as_a_row_error():
    """The batch's observation read is protected: a roster change between
    the plan's build and the read ends that environment with its error
    (`terminated=True`, the field named in `info["error_msg"]`) instead of
    unwinding the batch."""
    dwellings = [_make_dwelling(seed=0)]
    probe = VecDwellingGymEnv(
        dwellings=[PyDwelling.from_hpxml(
            hpxml=HPXML, schedule=SCHEDULE, weather=WEATHER,
            defaults=str(HARES_DEFAULTS),
            config=SimulationConfig(
                start_time="2019-01-01T00:00:00", duration=600, time_res=60, master_seed=0,
            ),
        )],
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_w"],
        episode_length=timedelta(minutes=5),
    )
    probe.reset(seed=0)
    names, _powers = _roster(probe.envs[0])
    env = VecDwellingGymEnv(
        dwellings=dwellings,
        observation_fields=[
            cast(Any, f"equipment_power[{names[0]}]"),
            cast(Any, f"equipment_power[{names[2]}]"),
        ],
        action_space_config={},
        reward_fn=lambda ctx: -ctx["total_power_w"],
        episode_length=timedelta(minutes=5),
    )
    env.reset(seed=0)
    env.envs[0].remove_equipment(names[0])

    _obs, _rewards, terminated, _trunc, infos = env.step(np.zeros((1, 0), dtype=np.float64))
    assert terminated[0], "the drifted roster ends that environment"
    assert "no longer in the dwelling's equipment roster" in infos[0]["error_msg"]
    assert f"equipment_power[{names[0]}]" in infos[0]["error_msg"]


# ---------------------------------------------------------------------------
# The episode lifecycle's actor surface: the env config registers actors
# before the construction snapshot, and a Python actor's state rides the
# checkpoint (and is cleared by a reset) through the wrapper's hooks.
# ---------------------------------------------------------------------------


class _StatefulActor(Actor):
    """A Python actor whose mutable state is a plain Python field, declared
    through the checkpoint hooks: the state the wrapper must carry across
    a restore and a reset."""

    def __init__(self):
        super().__init__()
        self.name = "Stateful"
        self.steps = 0

    def decide(self, env):
        self.steps += 1
        return []

    def save_state(self):
        return int(self.steps).to_bytes(8, "little")

    def load_state(self, state):
        self.steps = int.from_bytes(state, "little")


def _config_with_actors(actors: list[Any] | None) -> GymDwellingConfig:
    return GymDwellingConfig(
        hpxml=HPXML,
        schedule=SCHEDULE,
        weather=WEATHER,
        defaults=str(HARES_DEFAULTS),
        sim_config=SimulationConfig(
            start_time="2019-01-01T00:00:00", duration=600, time_res=60,
        ),
        actors=actors,
    )


def test_python_actor_state_rides_the_checkpoint_round_trip():
    """A stateful Python actor's fields restore to the checkpoint's value:
    `save_state` serializes them through the wrapper's hooks and
    `restore_state` reloads them, so a resumed episode continues the
    actor's own counts."""
    dwelling = _make_dwelling()
    actor = _StatefulActor()
    dwelling.add_actor(actor)
    dwelling.initialize()

    for _ in range(3):
        dwelling.step()
    assert actor.steps == 3, "the counter advances one per decide"
    blob = bytes(dwelling.save_state())
    dwelling.step()
    dwelling.step()
    assert actor.steps == 5

    dwelling.restore_state(blob)
    assert actor.steps == 3, (
        f"the checkpoint's actor state must restore (3), got {actor.steps}: "
        "the state carried across the restore instead"
    )


def test_single_env_config_actor_runs_two_episodes():
    """An actor declared in the env config registers before the
    construction snapshot, so the env's reset restores a checkpoint that
    carries it: two episodes on one env, the actor's channel in the
    observation both times."""
    env = DwellingGymEnv(
        config=_config_with_actors(
            [{"actor_type": "DrCompliance", "name": "DRAgent"}]
        ),
        observation_fields=[cast(Any, "DRAgent.dr_level")],
        action_space_config={},
        reward_fn=lambda ctx: -ctx["total_power_w"],
        episode_length=timedelta(minutes=5),
    )
    for episode in range(2):
        # The reset itself is the witness: the construction snapshot
        # carries the registered actor, so the restore never fails the
        # actor-count check.
        env.reset(seed=0)
        obs, _r, _term, _trunc, _info = env.step(np.zeros(0, dtype=np.float64))
        assert obs.shape == (1,), "the actor channel's observation resolves"
        assert np.isfinite(obs[0]), f"episode {episode}: the channel reads a value"


def test_single_env_reset_clears_a_stateful_python_actor():
    """A stateful Python actor declared in the env config restarts each
    episode at its construction state: the reset restores the checkpoint
    saved at construction, and the wrapper's load_state hook reloads the
    actor's own saved state."""
    actor = _StatefulActor()
    env = DwellingGymEnv(
        config=_config_with_actors([actor]),
        observation_fields=[cast(Any, "total_power_w")],
        action_space_config={},
        reward_fn=lambda ctx: -ctx["total_power_w"],
        episode_length=timedelta(minutes=5),
    )
    env.reset(seed=0)
    for _ in range(3):
        env.step(np.zeros(0, dtype=np.float64))
    assert actor.steps == 3, "the counter advances one per decide"

    env.reset(seed=0)
    assert actor.steps == 0, (
        f"the reset must restore the actor's construction state (0), "
        f"got {actor.steps}: the state carried across the reset instead"
    )


def test_batch_step_returns_a_fresh_observation_by_default():
    """The default step returns a fresh array per step: two consecutive
    steps give different objects, the first is unchanged by the second
    step, and each equals the pure-Python path's observation on bits."""
    dwellings = [_make_dwelling(seed=0) for _ in range(2)]
    env = VecDwellingGymEnv(
        dwellings=dwellings,
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_w"],
        episode_length=timedelta(minutes=5),
    )
    env.reset(seed=0)

    action = np.full((2, 1), 21.0, dtype=np.float64)
    first, _reward1, _done1, _trunc1, _info1 = env.step(action)
    plans = env._env_plans()
    direct1 = np.empty((2, len(_OBS_FIELDS)), dtype=np.float64)
    for idx, dwelling in enumerate(dwellings):
        dwelling.observe_into(plans[idx], direct1[idx])
    assert first is not direct1
    assert first.tobytes() == direct1.tobytes(), (
        "the returned observation must equal the pure-Python path's "
        "observation on bits"
    )
    first_snapshot = first.copy()

    second, _reward2, _done2, _trunc2, _info2 = env.step(action)
    assert second is not first, "two consecutive steps give different objects"
    assert first.tobytes() == first_snapshot.tobytes(), (
        "an observation a caller keeps is never overwritten by the next step"
    )
    direct2 = np.empty((2, len(_OBS_FIELDS)), dtype=np.float64)
    for idx, dwelling in enumerate(dwellings):
        dwelling.observe_into(plans[idx], direct2[idx])
    assert second.tobytes() == direct2.tobytes()


def test_batch_step_copy_false_returns_the_shared_buffer():
    """With copy=False two consecutive steps return the same object: the
    environment's own buffer, overwritten by the next step or reset."""
    dwellings = [_make_dwelling(seed=0) for _ in range(2)]
    env = VecDwellingGymEnv(
        dwellings=dwellings,
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_w"],
        episode_length=timedelta(minutes=5),
        copy=False,
    )
    env.reset(seed=0)

    action = np.full((2, 1), 21.0, dtype=np.float64)
    first, _r1, _d1, _t1, _i1 = env.step(action)
    second, _r2, _d2, _t2, _i2 = env.step(action)
    assert first is second, "copy=False returns the environment's own buffer"
    assert first is env._obs_buffer


# ---------------------------------------------------------------------------
# Configuration strictness: the gym rejects bad values with typed errors
# instead of coercing them, and forwards the whole typed config.
# ---------------------------------------------------------------------------


def _gym_config(sim_config: SimulationConfig | None = None) -> GymDwellingConfig:
    return GymDwellingConfig(
        hpxml=HPXML,
        schedule=SCHEDULE,
        weather=WEATHER,
        defaults=str(HARES_DEFAULTS),
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
            reward_fn=lambda ctx: -ctx["total_power_w"],
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
            "defaults": str(HARES_DEFAULTS),
            "time_res_s": "600",
        },
    )
    with pytest.raises(TypeError, match="GymDwellingConfig"):
        DwellingGymEnv(
            config=mapping,
            observation_fields=_OBS_FIELDS,
            action_space_config=_ACTION_CONFIG,
            reward_fn=lambda ctx: -ctx["total_power_w"],
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
            reward_fn=lambda ctx: -ctx["total_power_w"],
            episode_length=timedelta(minutes=5),
        )


def test_gym_default_writes_no_file(tmp_path: Path):
    """A DwellingGymEnv built with a default-valued SimulationConfig (only
    its required fields set) over the conftest inputs, run for one episode
    of 20 steps, leaves the working directory empty: nothing is recorded
    that no report requests, and a default config requests no report. (At
    the old booleans' defaults the episode wrote a csv into the working
    directory.)"""
    import os

    from hares.rl.gym_env import DwellingGymEnv as Env

    env = Env(
        config=GymDwellingConfig(
            hpxml=HPXML,
            schedule=SCHEDULE,
            weather=WEATHER,
            defaults=str(HARES_DEFAULTS),
            sim_config=SimulationConfig(
                start_time="2019-01-01T00:00:00",
                duration=1200,
                time_res=60,
            ),
        ),
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_w"],
        episode_length=timedelta(minutes=20),
    )
    cwd = os.getcwd()
    os.chdir(tmp_path)
    try:
        observation, _ = env.reset()
        assert observation is not None
        action = np.zeros(len(_ACTION_CONFIG["Gas Furnace"]), dtype=np.float64)
        for _ in range(20):
            observation, *_ = env.step(action)
    finally:
        os.chdir(cwd)
    assert list(tmp_path.iterdir()) == [], (
        "a default gym episode writes no file into the working directory"
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
                master_seed=0,
            )
        ),
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_w"],
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
    assert reward == -info["step"]["net_electric_power_w"] / 1000.0


def test_gym_rejects_bare_string_observation_fields():
    """A bare string observation_fields would iterate per character: it is
    a TypeError naming the field and the value."""
    with pytest.raises(TypeError, match="observation_fields.*got str") as caught:
        DwellingGymEnv(
            config=_DWELLING_CONFIG,
            observation_fields=cast(Any, "total_power_w"),
            action_space_config=_ACTION_CONFIG,
            reward_fn=lambda ctx: -ctx["total_power_w"],
            episode_length=timedelta(minutes=5),
        )
    assert "total_power_w" in str(caught.value)


def test_gym_rejects_a_non_string_observation_field():
    """An observation_fields element that is not a string is a TypeError
    naming the field and the element."""
    with pytest.raises(TypeError, match="observation_fields") as caught:
        DwellingGymEnv(
            config=_DWELLING_CONFIG,
            observation_fields=cast(Any, ["total_power_w", 3]),
            action_space_config=_ACTION_CONFIG,
            reward_fn=lambda ctx: -ctx["total_power_w"],
            episode_length=timedelta(minutes=5),
        )
    assert "got element 3 of type int" in str(caught.value)


def test_gym_forwards_every_sim_config_field(tmp_path: Path):
    """A gym built with a SimulationConfig whose every field differs from its
    default among them, records only what its reports request, and the config
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
        reports=[
            ReportSpec("frame", Selection.verbosity(2), Destination.memory()),
            ReportSpec(
                "file",
                Selection.verbosity(2),
                Destination.parquet(str(tmp_path / "gym_output.parquet"), overwrite=True),
            ),
        ],
        output_chunk_size=1234,
        master_seed=7,
        setpoint_deadband_c=1.5,
        site_location=SiteLocationOverride(
            latitude_deg=40.0,
            longitude_deg=-105.0,
            elevation_m=1600.0,
            utc_offset_h=-7.0,
        ),
        max_consecutive_step_failures=3,
    )
    env = DwellingGymEnv(
        config=_gym_config(sim_config),
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_w"],
        episode_length=timedelta(minutes=5),
    )
    built = env._dwelling.config()
    for field in (
        "start_time",
        "duration",
        "time_res",
        "output_chunk_size",
        "master_seed",
        "civil_timezone",
        "setpoint_deadband_c",
        "max_consecutive_step_failures",
    ):
        passed = getattr(sim_config, field)
        reported = getattr(built, field)
        assert reported == passed, (
            f"{field}: passed {passed!r}, the dwelling reports {reported!r}"
        )
    # The reports ride the config whole: repr is the one equality the
    # pyclasses carry across the boundary.
    assert [repr(r) for r in built.reports] == [repr(r) for r in sim_config.reports]
    # The site override rides as the one field, every component reported.
    for field in ("latitude_deg", "longitude_deg", "elevation_m", "utc_offset_h"):
        passed = getattr(sim_config.site_location, field)
        reported = getattr(built.site_location, field)
        assert reported == passed, (
            f"site_location.{field}: passed {passed!r}, the dwelling reports {reported!r}"
        )
    assert len(built.reports) == 2, "the reports ride the config whole"
    assert not (tmp_path / "gym_output.parquet").exists(), (
        "the gym's stepped episode publishes no file (publishing happens at simulate's end)"
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
        reward_fn=lambda ctx: -ctx["total_power_w"],
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
        reward_fn=lambda ctx: -ctx["total_power_w"],
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
            reward_fn=lambda ctx: -ctx["total_power_w"],
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
            reward_fn=lambda ctx: -ctx["total_power_w"],
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
            reward_fn=lambda ctx: -ctx["total_power_w"],
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
            reward_fn=lambda ctx: -ctx["total_power_w"],
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
                reward_fn=lambda ctx: -ctx["total_power_w"],
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
            observation_fields=cast(Any, "total_power_w"),
            action_space_config=_ACTION_CONFIG,
            reward_fn=lambda ctx: -ctx["total_power_w"],
            episode_length=timedelta(minutes=5),
        )
    assert "total_power_w" in str(caught.value)


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


_REPORTED_CONFIG = GymDwellingConfig(
    hpxml=HPXML,
    schedule=SCHEDULE,
    weather=WEATHER,
    defaults=str(HARES_DEFAULTS),
    sim_config=SimulationConfig(
        start_time="2019-01-01T00:00:00",
        duration=600,
        time_res=60,
        reports=[ReportSpec("frame", Selection.verbosity(1), Destination.memory())],
    ),
)


def test_gym_env_exposes_the_finished_run_result():
    """The truncating step finalises the run and ``run_result`` reads the
    finished ``RunResult``: ``None`` before it, the report surface after,
    cleared on ``reset()``."""
    env = DwellingGymEnv(
        config=_REPORTED_CONFIG,
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_w"],
        episode_length=timedelta(minutes=5),
    )
    env.reset(seed=0)
    before = env.run_result
    assert before is None, "no result before the truncating step"

    for _ in range(4):
        env.step(np.array([21.0], dtype=np.float64))
    running = env.run_result
    assert running is None, "no result while the episode runs"

    obs, reward, terminated, truncated, info = env.step(
        np.array([21.0], dtype=np.float64)
    )
    assert truncated is True, "the fifth step truncates the episode"
    result = env.run_result
    assert result is not None, "the truncating step finalises the run"
    reports: list[Any] = list(result.reports)
    assert [report.name for report in reports] == ["frame"]
    assert reports[0].rows == 5, "the report holds the episode's five steps"
    assert result.health.clamped_actions == 0

    env.reset(seed=0)
    cleared = env.run_result
    assert cleared is None, "reset() clears the finished result"


def test_vec_gym_exposes_each_dwelling_run_result():
    """``run_results`` reads each dwelling's finished ``RunResult`` off the
    vectorised wrapper: ``None`` per env before truncation, the report
    surface after, cleared on ``reset()``."""
    env = VecDwellingGymEnv(
        dwellings=[_make_dwelling_for_reports(seed=i) for i in range(2)],
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_w"],
        episode_length=timedelta(minutes=5),
    )
    env.reset(seed=0)
    assert all(result is None for result in env.run_results)

    for _ in range(4):
        env.step(np.full((2, 1), 21.0, dtype=np.float64))
    assert all(result is None for result in env.run_results)

    obs, rewards, dones, truncs, infos = env.step(
        np.full((2, 1), 21.0, dtype=np.float64)
    )
    assert truncs.tolist() == [True, True], "the fifth step truncates both"
    for result in env.run_results:
        assert result is not None, "the truncating step finalises each run"
        assert [report.name for report in result.reports] == ["frame"]
        assert result.reports[0].rows == 5

    env.reset(seed=0)
    assert all(result is None for result in env.run_results), "reset() clears"


def _make_dwelling_for_reports(seed: int = 0) -> PyDwelling:
    config = SimulationConfig(
        start_time="2019-01-01T00:00:00",
        duration=600,
        time_res=60,
        master_seed=seed,
        reports=[ReportSpec("frame", Selection.verbosity(1), Destination.memory())],
    )
    return PyDwelling.from_hpxml(
        hpxml=HPXML,
        schedule=SCHEDULE,
        weather=WEATHER,
        defaults=str(HARES_DEFAULTS),
        config=config,
    )


def test_gym_observation_bounds_are_in_watts():
    """The default observation space's `total_power_w` bounds are
    (-100 000, 100 000), `reactive_power_var`'s the same and each
    equipment `power_w` the signed W range (generation is negative by
    contract, so a PV home's producing step is inside its Box); one
    episode keeps every observation inside its Box, the PV's own
    equipment column included; the built-in reward is minus the step's
    net power in kW. The home is the der_pv_battery_900s inputs' PV home
    without the battery: the gym config carries no override surface, and
    the bounds and the reward need no battery."""
    from hares import SimulationConfig

    config = GymDwellingConfig(
        hpxml=str(ROOT / "tests/fixtures/hpxml/ochre_samples/base-pv.xml"),
        schedule=SCHEDULE,
        weather=WEATHER,
        defaults=str(HARES_DEFAULTS),
        sim_config=SimulationConfig(
            start_time="2023-06-01T00:00:00-07:00",
            duration=14400,
            time_res=900,
            master_seed=0,
        ),
    )
    fields = ["total_power_w", "outdoor_temp"]
    env = DwellingGymEnv(
        config=config,
        observation_fields=cast(Any, fields),
        action_space_config={},
        reward_fn=None,
        episode_length=timedelta(hours=4),
    )
    obs, _ = env.reset(seed=0)
    for _ in range(10):
        obs, reward, _term, _trunc, info = env.step(np.zeros(0, dtype=np.float64))
        for i, field in enumerate(fields):
            low, high = observation_field_bounds(field)
            assert low <= obs[i] <= high, f"{field} = {obs[i]} outside ({low}, {high})"
        # The built-in reward: minus the step's net power in kW, converted
        # once from the step's watts.
        assert reward == -info["step"]["net_electric_power_w"] / 1000.0
    # The bounds are the W-scale ones, the equipment power signed.
    assert observation_field_bounds("total_power_w") == (-100_000.0, 100_000.0)
    assert observation_field_bounds("reactive_power_var") == (-100_000.0, 100_000.0)
    assert observation_field_bounds("equipment_power[Gas Furnace]") == (
        -100_000.0,
        100_000.0,
    )
    assert observation_field_bounds("zone_temp[Indoor]") == (-80.0, 50.0)


def test_pv_production_sits_inside_the_observation_box():
    """A PV home's producing step publishes a negative `equipment_power`
    for the PV (generation is negative watts by contract) and the whole
    observation stays inside the Box the bounds table declared: the
    containment Gymnasium's passive checker warns on, and `check_env`
    would refuse."""
    from hares import SimulationConfig

    sim_config = SimulationConfig(
        start_time="2023-06-01T10:00:00-07:00",
        duration=14400,
        time_res=900,
        master_seed=0,
    )
    config = GymDwellingConfig(
        hpxml=str(ROOT / "tests/fixtures/hpxml/ochre_samples/base-pv.xml"),
        schedule=SCHEDULE,
        weather=WEATHER,
        defaults=str(HARES_DEFAULTS),
        sim_config=sim_config,
    )
    # The PV unit's own name, read off the built home's roster.
    probe = PyDwelling.from_hpxml(
        hpxml=config.hpxml,
        schedule=config.schedule,
        weather=config.weather,
        defaults=config.defaults,
        config=sim_config,
    )
    probe.initialize()
    names = list(probe.telemetry().equipment()["names"])
    pv_names = [name for name in names if "pv" in name.lower()]
    assert pv_names, f"the PV fixture must carry a PV unit, got {names}"
    pv_field = cast(Any, f"equipment_power[{pv_names[0]}]")

    env = DwellingGymEnv(
        config=config,
        observation_fields=[pv_field],
        action_space_config={},
        reward_fn=None,
        episode_length=timedelta(hours=4),
    )
    env.reset(seed=0)
    produced = False
    for _ in range(10):
        obs, _r, _term, _trunc, _info = env.step(np.zeros(0, dtype=np.float64))
        assert env.observation_space.contains(obs), (
            f"{pv_field} = {obs[0]} must sit inside the declared Box"
        )
        produced = produced or obs[0] < 0.0
    assert produced, (
        "the PV home must produce within the episode: a negative "
        f"{pv_field} step is the containment witness"
    )


# ---------------------------------------------------------------------------
# Episode lifecycle: snapshot/restore/pickle, masked auto-reset, and the
# schedule/occupancy/clock observation fields (the lifecycle row's faces).
# ---------------------------------------------------------------------------

_LIFECYCLE_FIELDS = [
    "schedule_row",
    "occupants_present",
    "clock_hour",
    "clock_day",
    "clock_step",
]


def test_pickle_round_trip_continues_the_episode_bitwise():
    """A pickled Dwelling unpickles to a dwelling whose next step is the
    uninterrupted run's next step, bitwise: the pickle carries the
    episode's checkpoint blob, and the continuation after unpickling
    equals the same run's continuation."""
    import pickle

    def build():
        return PyDwelling.from_hpxml(
            hpxml=HPXML,
            schedule=SCHEDULE,
            weather=WEATHER,
            defaults=str(HARES_DEFAULTS),
            config=SimulationConfig(
                start_time="2019-01-01T00:00:00",
                duration=3600,
                time_res=60,
                master_seed=7,
            ),
        )

    run = build()
    run.initialize()
    twin = build()
    twin.initialize()

    # Ten uncontrolled steps put the run mid-episode; the pickle crosses
    # the boundary there.
    for _ in range(10):
        run.step()
        twin.step()

    pickled = pickle.dumps(run)
    resumed = pickle.loads(pickled)

    fields = ["total_power_w", "clock_hour", "clock_step"]
    plan = resumed.observation_plan(fields)
    row = np.empty(len(fields), dtype=np.float64)
    resumed.observe_into(plan, row)

    plan_ref = twin.observation_plan(fields)
    row_ref = np.empty(len(fields), dtype=np.float64)
    twin.observe_into(plan_ref, row_ref)
    assert row.tobytes() == row_ref.tobytes(), (
        "the unpickled dwelling's observation must be the uninterrupted "
        "run's observation, bitwise"
    )

    step_resumed = resumed.step()
    step_run = twin.step()
    plan2 = resumed.observation_plan(fields)
    row2 = np.empty(len(fields), dtype=np.float64)
    resumed.observe_into(plan2, row2)
    plan2_ref = twin.observation_plan(fields)
    row2_ref = np.empty(len(fields), dtype=np.float64)
    twin.observe_into(plan2_ref, row2_ref)
    assert row2.tobytes() == row2_ref.tobytes(), (
        "the episode's continuation after unpickling must equal the "
        "uninterrupted run's next step, bitwise"
    )
    assert step_resumed["timestamp"] == step_run["timestamp"]
    assert (
        step_resumed["net_electric_power_w"].hex()
        == step_run["net_electric_power_w"].hex()
    ), "the step's power must be bit-identical across the pickle boundary"


def test_the_pickle_refuses_actors_it_cannot_rebuild():
    """The pickle does not carry actors added to the dwelling after
    construction: the unpickle's rebuild cannot make them, and it refuses
    naming the counts rather than resume without them."""
    import pickle

    dwelling = _make_dwelling(seed=0, duration_s=600)
    dwelling.add_actor_by_name("IdealThermostat", "probe", {"target": "21.0"})
    blob = pickle.dumps(dwelling)
    with pytest.raises(ValueError, match="actors"):
        pickle.loads(blob)


def test_snapshot_state_rejects_a_foreign_checkpoint_version():
    """restore_state's blob crosses the checkpoint's version gate: a blob
    whose version tag names another version is refused naming both
    versions, never parsed as this version's schema."""
    import json as _json

    dwelling = _make_dwelling(seed=0, duration_s=600)
    dwelling.initialize()
    blob = _json.loads(bytes(dwelling.snapshot_state()))
    assert blob["format_version"] >= 1, "the snapshot carries the checkpoint's version tag"
    blob["format_version"] = blob["format_version"] - 1
    tampered = _json.dumps(blob).encode()
    with pytest.raises(Exception, match="version mismatch"):
        dwelling.restore_state(tampered)


def test_masked_auto_reset_resets_the_ended_env_alone():
    """The masked auto-reset resets only the env whose episode ended, and it
    does reset that env: the returned row is the new episode's first
    observation (NaN before its first step), the ended episode's last
    observation rides `final_observation`, the row's seed is the env's next
    child spawn, and the next step is the new episode's step 1. The other
    env's next steps are bitwise the no-reset run's. The short home (a
    two-step sim) fails its third step; the long home never moves."""
    fields = _OBS_FIELDS + ["clock_step"]

    def run(auto_reset: bool):
        dwellings = [
            _make_dwelling(seed=0, duration_s=120),
            _make_dwelling(seed=1, duration_s=600),
        ]
        env = VecDwellingGymEnv(
            dwellings=dwellings,
            observation_fields=fields,
            action_space_config=_ACTION_CONFIG,
            reward_fn=lambda ctx: -ctx["total_power_w"],
            episode_length=timedelta(minutes=20),
            auto_reset=auto_reset,
            seed_sequence=np.random.SeedSequence(2024),
        )
        env.reset(seed=7)
        actions = np.zeros((2, 1), dtype=np.float64)
        short_rows = []
        living_rows = []
        seeds = []
        ended = None
        for step_index in range(8):
            obs, _reward, done, trunc, info = env.step(actions)
            short_rows.append(obs[0].copy())
            living_rows.append(obs[1].copy())
            seeds.append((info[0]["seed"], info[1]["seed"]))
            if auto_reset and ended is None and (done[0] or trunc[0]):
                ended = {
                    "step": step_index,
                    "reset_obs": obs[0].copy(),
                    "final_observation": info[0]["final_observation"].copy(),
                    "seed": info[0]["seed"],
                }
        return short_rows, living_rows, seeds, ended

    with_reset, with_living, with_seeds, ended = run(True)
    without_reset, without_living, without_seeds, _ = run(False)

    for step_index, (on, off) in enumerate(zip(with_living, without_living, strict=True)):
        assert on.tobytes() == off.tobytes(), (
            f"the living env's step {step_index + 1} must be bitwise the "
            "no-reset run's: the masked reset touched only the ended env"
        )

    # The unmasked env keeps its running episode's seed on every step row:
    # reset(seed=7) gives `seed + env index`, so env 1 runs under 8.
    assert [seed1 for _seed0, seed1 in with_seeds] == [8] * 8
    assert [seed1 for _seed0, seed1 in without_seeds] == [8] * 8

    assert ended is not None, "the short home must end its episode in the window"
    ended_step = ended["step"]
    assert np.all(np.isnan(ended["reset_obs"])), (
        "the auto-reset row is the new episode's first observation, which is "
        "NaN in every field before the first step"
    )
    assert ended["final_observation"].tobytes() == without_reset[ended_step].tobytes(), (
        "the ended episode's last observation is the no-reset run's failed-step row"
    )
    expected_seed = int(
        np.random.SeedSequence(2024)
        .spawn(2)[0]
        .spawn(1)[0]
        .generate_state(1, dtype=np.uint64)[0]
    )
    assert ended["seed"] == expected_seed, (
        "the new episode's seed is the env's child spawn from the caller "
        "SeedSequence: (caller sequence, env index 0, episode 2)"
    )
    assert with_reset[ended_step + 1][-1] == 1.0, (
        "the env resets its step counter: the post-reset step is the new "
        "episode's step 1, not the ended episode's next count"
    )


def test_episode_seeds_derive_from_the_caller_sequence_per_env_and_episode():
    """The per-env episode seeds are deterministic in (caller SeedSequence,
    env index, episode count): two batches built from the same caller
    sequence value derive the same seeds, and one env's resets never
    advance another env's stream (each env draws from its own child)."""
    fields = _OBS_FIELDS

    def dwellings():
        return [
            _make_dwelling(seed=0, duration_s=1800),
            _make_dwelling(seed=1, duration_s=1800),
        ]

    def make_env(seed_sequence):
        return VecDwellingGymEnv(
            dwellings=dwellings(),
            observation_fields=fields,
            action_space_config=_ACTION_CONFIG,
            reward_fn=lambda ctx: -ctx["total_power_w"],
            episode_length=timedelta(minutes=20),
            seed_sequence=seed_sequence,
        )

    def seeds(env, **kwargs):
        _obs, infos = env.reset(**kwargs)
        return [info["seed"] for info in infos]

    first = make_env(np.random.SeedSequence(2024))
    second = make_env(np.random.SeedSequence(2024))
    first_batch = seeds(first)
    assert first_batch == seeds(second), (
        "the same caller sequence value derives the same per-env first-episode seeds"
    )
    second_batch = seeds(first)
    assert second_batch == seeds(second), (
        "the second episode's seeds are deterministic in the episode count"
    )

    # One env's resets advance only its own child: env 1's second episode
    # seed is the same whether or not env 0 reset in between.
    independent = make_env(np.random.SeedSequence(2024))
    seeds(independent)  # episode 1 on both children
    seeds(independent, mask=np.array([True, False]))  # only env 0's child moves
    seeds(independent, mask=np.array([True, False]))
    env1_second = seeds(independent, mask=np.array([False, True]))[1]
    assert env1_second == second_batch[1], (
        "env 0's masked resets must not advance env 1's child stream: each "
        "env's episode count derives from its own child"
    )

    # A caller SeedSequence on reset re-spawns the per-env children and the
    # masked env draws that child's next spawn.
    caller = np.random.SeedSequence(99)
    _obs, infos = independent.reset(seed=caller)
    expected = [
        int(child.spawn(1)[0].generate_state(1, dtype=np.uint64)[0])
        for child in np.random.SeedSequence(99).spawn(2)
    ]
    assert [info["seed"] for info in infos] == expected


def test_reset_seed_is_an_integer_or_a_seed_sequence_and_never_dropped():
    """An explicit integer seed (a numpy integer included) keeps the
    documented ``seed + env index`` form; a seed of any other type is
    refused naming the type before a dwelling moves, instead of being
    silently replaced by an OS-entropy draw."""
    env = VecDwellingGymEnv(
        dwellings=[_make_dwelling(seed=0, duration_s=1800)],
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_w"],
        episode_length=timedelta(minutes=20),
        seed_sequence=np.random.SeedSequence(2024),
    )
    _obs, infos = env.reset(seed=np.int64(7))
    assert infos[0]["seed"] == 7, "a numpy integer is the seed + env index form"

    applied = list(env._applied_seeds)
    # The invalid types are the caller errors the binding refuses; the casts
    # keep the static signature honest at the call.
    with pytest.raises(TypeError, match="float64"):
        env.reset(seed=cast(Any, np.float64(7.0)))
    with pytest.raises(TypeError, match="str"):
        env.reset(seed=cast(Any, "7"))
    assert env._applied_seeds == applied, (
        "a refused seed leaves every env's applied seed unchanged: validation "
        "runs before the reset touches a dwelling"
    )


def test_observation_reports_the_engine_clock_schedule_and_occupancy():
    """The new observation fields read the engine's own state at the same
    step: the schedule row is the environment's published row, the
    occupants-present count is the occupancy fraction the gains read
    times the resident count, and the clock fields are the step's local
    time and step index."""
    fixture = ROOT / "tests/fixtures/hpxml/base-all-north.xml"
    # 3.0 residents in the fixture's BuildingOccupancy; with no schedule
    # CSV the generated schedule's occupancy column evaluates the shipped
    # default profile, whose weekday fraction at hours 0 through 4 is
    # 0.035 ("Default Schedule Parameters.csv", the occupants row's
    # weekday_fractions), and the run's first hours are weekday night
    # (the run starts Tuesday 2019-01-01).
    residents = 3.0
    night_fraction = 0.035
    dwelling = PyDwelling.from_hpxml(
        hpxml=str(fixture),
        schedule=None,
        weather=WEATHER,
        defaults=str(HARES_DEFAULTS),
        config=SimulationConfig(
            start_time="2019-01-01T00:00:00",
            duration=6 * 3600,
            time_res=60,
            master_seed=0,
            # Diversity off: the generated occupancy column carries the
            # raw profile fraction, so the expectation is the shipped
            # profile's own value times the resident count, with no
            # diversity draw in the product.
            schedule_diversity="off",
        ),
    )
    dwelling.initialize()

    plan = dwelling.observation_plan(_LIFECYCLE_FIELDS)
    row = np.empty(len(_LIFECYCLE_FIELDS), dtype=np.float64)
    dwelling.observe_into(plan, row)
    assert np.all(np.isnan(row)), "before the first step every field is NaN"

    step_dict = dwelling.step()
    dwelling.observe_into(plan, row)
    timestamp = step_dict["timestamp"]
    assert row[2] == timestamp.hour + timestamp.minute / 60.0 + timestamp.second / 3600.0, (
        "clock_hour is the step's own start instant's fractional local hour"
    )
    expected_ordinal = (
        timestamp.replace(tzinfo=None) - timestamp.replace(tzinfo=None, month=1, day=1)
    ).days + 1
    assert row[3] == float(expected_ordinal), "clock_day is the step's day of year"
    assert row[4] == 1.0, "clock_step is the dwelling's step index"
    assert row[0] == dwelling.latest_env()["schedule_row"], (
        "schedule_row is the environment's published schedule row"
    )
    assert row[1] == residents * night_fraction, (
        "occupants_present is the schedule's occupancy fraction times the "
        "resident count, the product the occupancy gains read"
    )


# ---------------------------------------------------------------------------
# The Gymnasium vector surface: the class is a VectorEnv, the standard
# wrappers accept it, and the per-env spaces ride the single_* attributes.
# ---------------------------------------------------------------------------


def test_vec_env_is_a_gymnasium_vector_env_with_single_spaces():
    """`VecDwellingGymEnv` subclasses `gymnasium.vector.VectorEnv`: the
    standard wrappers accept it, `single_observation_space` /
    `single_action_space` are the per-env spaces the base class expects,
    and `unwrapped` exposes the environment itself."""
    import gymnasium

    dwellings = [_make_dwelling(seed=0)]
    env = VecDwellingGymEnv(
        dwellings=dwellings,
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_w"],
        episode_length=timedelta(minutes=5),
    )
    assert isinstance(env, gymnasium.vector.VectorEnv)
    assert env.single_observation_space == env.observation_space
    assert env.single_action_space == env.action_space
    assert env.observation_space.shape == (len(_OBS_FIELDS),)
    assert env.unwrapped is env

    obs, infos = env.reset(seed=0)
    assert obs.shape == (1, len(_OBS_FIELDS))
    obs, _r, _d, _t, _i = env.step(np.full((1, 1), 21.0, dtype=np.float64))
    assert env.single_observation_space.contains(obs[0]), (
        "the step observation sits inside the declared per-env space"
    )
    env.close()
    assert env.closed


def test_vec_env_accepts_the_standard_episode_statistics_wrapper():
    """The standard vector wrapper composes: the wrapped env steps with
    the action clipped to the declared action space, the info rows keep
    the documented per-env shape, and the base class's bookkeeping
    (`closed`, `unwrapped`) follows the wrapped chain."""
    import gymnasium

    dwellings = [_make_dwelling(seed=0) for _ in range(2)]
    env = gymnasium.wrappers.vector.ClipAction(
        VecDwellingGymEnv(
            dwellings=dwellings,
            observation_fields=_OBS_FIELDS,
            action_space_config=_ACTION_CONFIG,
            reward_fn=lambda ctx: -ctx["total_power_w"],
            episode_length=timedelta(minutes=5),
        )
    )
    env.reset(seed=0)
    # An action outside the declared Box: the wrapper clips it to the
    # space before the environment sees it, and the step reads like the
    # in-bounds action's own.
    _obs, rewards, _dones, _truncs, infos = env.step(
        np.full((2, 1), 10.0, dtype=np.float64)
    )
    assert len(infos) == 2, "one info row per env, the documented shape"
    for info in infos:
        assert isinstance(info, dict) and "warning_count" in info
    env.close()
    assert env.unwrapped.closed


# ---------------------------------------------------------------------------
# The parity contract of the two step paths: the clamp accounting, the run
# health, the refused action and the default reward are one contract on
# both paths; the bounds overrides and the reset seed are validated before
# anything is touched.
# ---------------------------------------------------------------------------


def test_both_step_paths_share_the_clamp_and_health_contract():
    """The Python fallback's clamp accounting and queue-time acceptance are
    the batch path's: an out-of-bounds action is clipped and counted on the
    acting dwelling's run health, and a signal the equipment would refuse
    is refused before anything applies, on both paths."""
    actions_in_band = np.full((2, 1), 200.0, dtype=np.float64)  # far above the band
    clamped_counts = []
    rejected = []
    for rust_path in (True, False):
        import hares.rl.vec_env as vec_module

        previous = vec_module.rust_batch_step
        vec_module.rust_batch_step = (
            extension_batch_step if rust_path else None
        )
        try:
            dwellings = [_make_dwelling(seed=0), _make_dwelling(seed=0)]
            env = VecDwellingGymEnv(
                dwellings=dwellings,
                observation_fields=_OBS_FIELDS,
                action_space_config=_ACTION_CONFIG,
                reward_fn=lambda ctx: -ctx["total_power_w"],
                episode_length=timedelta(minutes=5),
            )
            env.reset(seed=0)
            _obs, _rewards, _d, _t, _i = env.step(actions_in_band)
            counts = [
                dwelling.health().clamped_actions for dwelling in dwellings
            ]
            clamped_counts.append(counts)
        finally:
            vec_module.rust_batch_step = previous
    assert clamped_counts[0] == clamped_counts[1], (
        f"the clamp count is the same on both paths: {clamped_counts}"
    )
    assert clamped_counts[0][0] == 1, (
        f"the out-of-band action is counted once on the acting dwelling: {clamped_counts[0]}"
    )

    # A signal the equipment would refuse: the refusal is the caller's
    # error on both paths, before any dwelling is touched.
    for rust_path in (True, False):
        import hares.rl.vec_env as vec_module

        previous = vec_module.rust_batch_step
        vec_module.rust_batch_step = (
            extension_batch_step if rust_path else None
        )
        try:
            dwellings = [_make_dwelling(seed=0), _make_dwelling(seed=0)]
            env = VecDwellingGymEnv(
                dwellings=dwellings,
                observation_fields=_OBS_FIELDS,
                action_space_config=_ACTION_CONFIG,
                reward_fn=lambda ctx: -ctx["total_power_w"],
                episode_length=timedelta(minutes=5),
            )
            env.reset(seed=0)
            # A missing target equipment: the signal names an equipment
            # the dwelling does not carry.
            env._action_layout = [("No Such Equipment", "heat_c")]
            env._signal_type_by_equipment = {"No Such Equipment": "ThermalSetpoint"}
            env._action_dim = 1
            with pytest.raises(Exception, match="No Such Equipment"):
                env.step(np.full((2, 1), 21.0, dtype=np.float64))
            rejected.append(
                [dwelling.health().rejected_control_signals for dwelling in dwellings]
            )
        finally:
            vec_module.rust_batch_step = previous
    assert rejected[0] == rejected[1] == [0, 0], (
        f"the refused action reaches no dwelling on either path: {rejected}"
    )


def test_both_step_paths_refuse_a_later_envs_action_before_stepping_any():
    """A refused action in a later environment leaves every dwelling
    unstepped on both step paths: the batch path maps and accepts every
    row before it applies one, and the fallback validates every row
    before it touches one. RED at the fallback's per-row dispatch, where
    the first environment stepped before the second row was refused."""
    for rust_path in (True, False):
        import hares.rl.vec_env as vec_module

        previous = vec_module.rust_batch_step
        vec_module.rust_batch_step = extension_batch_step if rust_path else None
        try:
            dwellings = [_make_dwelling(seed=0), _make_dwelling(seed=0)]
            env = VecDwellingGymEnv(
                dwellings=dwellings,
                observation_fields=_OBS_FIELDS,
                action_space_config=_ACTION_CONFIG,
                reward_fn=lambda ctx: -ctx["total_power_w"],
                episode_length=timedelta(minutes=5),
            )
            env.reset(seed=0)
            # The second dwelling loses the action's target after the
            # layout is frozen on both: its row's signal is refused while
            # the first dwelling's row is valid.
            env.envs[1].remove_equipment("Gas Furnace")
            before = env.envs[0].telemetry().timestep_index
            with pytest.raises(Exception, match="Gas Furnace"):
                env.step(np.full((2, 1), 21.0, dtype=np.float64))
            assert env.envs[0].telemetry().timestep_index == before, (
                f"rust_path={rust_path}: the refused row must not step the "
                "first environment"
            )
        finally:
            vec_module.rust_batch_step = previous


def test_the_default_reward_is_bitwise_equal_across_paths_and_envs():
    """`reward_fn=None` pays the same default on the single env and on
    both vec paths: minus the step's net power in kW, on bits, one
    fixture."""
    dwellings = [_make_dwelling(seed=0), _make_dwelling(seed=0)]
    vec_rewards = []
    for rust_path in (True, False):
        import hares.rl.vec_env as vec_module

        previous = vec_module.rust_batch_step
        vec_module.rust_batch_step = (
            extension_batch_step if rust_path else None
        )
        try:
            env = VecDwellingGymEnv(
                dwellings=[_make_dwelling(seed=0), _make_dwelling(seed=0)],
                observation_fields=_OBS_FIELDS,
                action_space_config=_ACTION_CONFIG,
                reward_fn=None,
                episode_length=timedelta(minutes=5),
            )
            env.reset(seed=0)
            _obs, rewards, _d, _t, infos = env.step(np.full((2, 1), 21.0, dtype=np.float64))
            vec_rewards.append(
                [
                    struct.pack("<d", rewards[i]) == struct.pack(
                        "<d", -infos[i]["step"]["net_electric_power_w"] / 1000.0
                    )
                    for i in range(2)
                ]
            )
        finally:
            vec_module.rust_batch_step = previous
    assert vec_rewards[0] == [True, True] and vec_rewards[1] == [True, True], (
        f"the vec default is minus the step's net kW on both paths: {vec_rewards}"
    )

    single = DwellingGymEnv(
        config=_DWELLING_CONFIG,
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=None,
        episode_length=timedelta(minutes=5),
    )
    single.reset(seed=0)
    _obs, reward, _term, _trunc, info = single.step(np.array([21.0], dtype=np.float64))
    assert struct.pack("<d", reward) == struct.pack(
        "<d", -info["step"]["net_electric_power_w"] / 1000.0
    ), "the single env's default is the same conversion"
    # The vec and single defaults agree on the same fixture: the vec env's
    # two dwellings were stepped once with the same action, so their
    # rewards equal the single env's bitwise.
    assert vec_rewards[0][0] and vec_rewards[0][1]


def test_field_bounds_overrides_are_validated_on_both_envs():
    """An unknown override key (the typo) and a malformed pair (inverted,
    non-finite) are refused naming the key and the declared fields, on
    both gym environments, at construction."""

    def build_single(**overrides: Any) -> DwellingGymEnv:
        return DwellingGymEnv(
            config=_DWELLING_CONFIG,
            observation_fields=[cast(Any, "total_power_w")],
            action_space_config={},
            reward_fn=lambda ctx: -ctx["total_power_w"],
            episode_length=timedelta(minutes=5),
            **overrides,
        )

    def build_vec(**overrides: Any) -> VecDwellingGymEnv:
        return VecDwellingGymEnv(
            dwellings=[_make_dwelling()],
            observation_fields=[cast(Any, "total_power_w")],
            action_space_config={},
            reward_fn=lambda ctx: -ctx["total_power_w"],
            episode_length=timedelta(minutes=5),
            **overrides,
        )

    for build in (build_single, build_vec):
        with pytest.raises(ValueError, match="total_pwr_w.*total_power_w"):
            build(field_bounds_overrides={"total_pwr_w": (0.0, 1.0)})
        with pytest.raises(ValueError, match="low < high"):
            build(field_bounds_overrides={"total_power_w": (10.0, -10.0)})
        with pytest.raises(ValueError, match="finite"):
            build(field_bounds_overrides={"total_power_w": (float("nan"), 10.0)})


def test_reset_seed_is_range_validated_before_anything_is_touched():
    """A float, a negative and an over-u64 seed are refused naming the
    value or the type, and the env's state is exactly as it was: the
    episode counters never move and the next step is bitwise the
    control run's own."""
    control = DwellingGymEnv(
        config=_DWELLING_CONFIG,
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_w"],
        episode_length=timedelta(minutes=5),
    )
    control.reset(seed=0)
    control.step(np.array([21.0], dtype=np.float64))
    control.step(np.array([21.0], dtype=np.float64))
    control_obs, _r, _term, _trunc, _info = control.step(np.array([21.0], dtype=np.float64))

    env = DwellingGymEnv(
        config=_DWELLING_CONFIG,
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_w"],
        episode_length=timedelta(minutes=5),
    )
    env.reset(seed=0)
    env.step(np.array([21.0], dtype=np.float64))
    env.step(np.array([21.0], dtype=np.float64))
    for bad_seed in (1.5, -1, 2**64, "42"):
        with pytest.raises((TypeError, ValueError)):
            env.reset(seed=bad_seed)
        # The refusal touched nothing: the episode counters hold.
        assert env._steps_elapsed == 2, (
            f"a refused seed leaves the episode counters alone, got {env._steps_elapsed}"
        )
    # The env's next step is the control run's own third step, bitwise.
    refused_obs, _r2, _term2, _trunc2, _info2 = env.step(np.array([21.0], dtype=np.float64))
    assert struct.pack("<2d", *refused_obs) == struct.pack("<2d", *control_obs), (
        "the refused resets left the episode exactly on its own course"
    )
