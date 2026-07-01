"""Tests for DwellingGymEnv and VecDwellingGymEnv."""

from __future__ import annotations

from datetime import timedelta
from pathlib import Path

import pytest

np = pytest.importorskip("numpy")
pytest.importorskip("gymnasium")

from ochre_next._hares import Dwelling as PyDwelling
from ochre_next.rl.gym_env import DwellingGymEnv, _sorted_action_layout
from ochre_next.rl.vec_env import VecDwellingGymEnv

ROOT = Path(__file__).resolve().parents[2]
HARES_DEFAULTS = ROOT / "defaults"
HPXML = str(ROOT / "tests/fixtures/hpxml/ochre_samples/base.xml")
WEATHER = str(ROOT / "data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw")
SCHEDULE = str(ROOT / "data/examples/BEopt_example_schedule.csv")

_DWELLING_CONFIG = {
    "hpxml": HPXML,
    "schedule": SCHEDULE,
    "weather": WEATHER,
    "defaults_path": str(HARES_DEFAULTS),
    "start_time": "2019-01-01T00:00:00",
    "duration_s": 600,
    "time_res_s": 60,
}

# Gas Furnace is present in the base HPXML; heat_c resolves to ThermalSetpoint.
_ACTION_CONFIG: dict[str, list[str]] = {"Gas Furnace": ["heat_c"]}
_OBS_FIELDS = ["total_power_kw", "outdoor_temp_c"]


def _make_env(episode_length: timedelta = timedelta(minutes=5)) -> DwellingGymEnv:
    return DwellingGymEnv(
        config=_DWELLING_CONFIG,
        observation_fields=_OBS_FIELDS,
        action_space_config=_ACTION_CONFIG,
        reward_fn=lambda ctx: -ctx["total_power_kw"],
        episode_length=episode_length,
    )


def _make_dwelling(seed: int = 0) -> PyDwelling:
    return PyDwelling.from_hpxml(
        hpxml=HPXML,
        schedule=SCHEDULE,
        weather=WEATHER,
        defaults_path=str(HARES_DEFAULTS),
        start_time="2019-01-01T00:00:00",
        duration_s=600,
        time_res_s=60,
        master_seed=seed,
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


def test_sorted_action_layout_preserves_equipment_sort_order():
    layout, type_by_equip = _sorted_action_layout(
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
    from ochre_next import Battery
    from ochre_next._hares import Dwelling as PyDwelling

    n_dwel = 2
    obs_fields = ["total_power_kw", "outdoor_temp"]
    dwellings = [
        PyDwelling.from_hpxml(
            hpxml=HPXML,
            schedule=SCHEDULE,
            weather=WEATHER,
            defaults_path=str(HARES_DEFAULTS),
            start_time="2019-01-01T00:00:00",
            duration_s=6000,
            time_res_s=60,
            master_seed=i,
        )
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
