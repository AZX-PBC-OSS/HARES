"""Warning-surfacing contract tests for the pure-Python RL step paths.

The Rust ``batch_step`` fast path (covered in test_gym_batch_step.py) drains
``Dwelling.take_warnings()`` into ``info["warning_count"]`` (float, always
present) and ``info["warnings"]`` (list[str], only when non-zero). These tests
pin the identical contract onto the two pure-Python paths:
``DwellingGymEnv.step`` and the ``VecDwellingGymEnv`` fallback used when the
Rust entrypoint is unavailable.

``VecDwellingGymEnv`` imports without gymnasium (numpy only), so its tests run
everywhere. ``DwellingGymEnv`` requires gymnasium at construction time and its
tests skip where that optional dependency is absent.
"""

from __future__ import annotations

from datetime import timedelta

import pytest

np = pytest.importorskip("numpy")

from conftest import HARES_DEFAULTS, HPXML, SCHEDULE, WEATHER, make_dwelling

import ochre_next.rl.vec_env as vec_env_module
from ochre_next.rl.vec_env import VecDwellingGymEnv

_OBS_FIELDS = ["total_power_kw", "outdoor_temp"]

# heat_c = 21 degC is a valid setpoint: the clean-step baseline.
_CLEAN_ACTION_CONFIG = {"Gas Furnace": ["heat_c"]}
# cool_c = 70 degC is inside the gym clip range (-50, 80) but outside
# ControlSignal numeric bounds ([0, 60] degC), so it passes the queue-time
# capability check and is rejected by Equipment::apply_control during
# dispatch — the silent-warning path these tests guard.
_REJECTED_ACTION_CONFIG = {"Gas Furnace": ["cool_c"]}


# ---------------------------------------------------------------------------
# VecDwellingGymEnv — pure-Python fallback (rust_batch_step forced to None)
# ---------------------------------------------------------------------------


def _make_vec_env(action_config: dict[str, list[str]]) -> tuple[VecDwellingGymEnv, list]:
    dwellings = [make_dwelling(duration_s=600)]
    env = VecDwellingGymEnv(
        dwellings=dwellings,
        observation_fields=_OBS_FIELDS,
        action_space_config=action_config,
        reward_fn=lambda ctx: -ctx["total_power_kw"],
        episode_length=timedelta(minutes=5),
    )
    return env, dwellings


def test_vec_fallback_clean_step_reports_zero_warnings(monkeypatch: pytest.MonkeyPatch):
    """A clean fallback step surfaces warning_count == 0.0 and omits the list."""
    monkeypatch.setattr(vec_env_module, "rust_batch_step", None)
    env, _ = _make_vec_env(_CLEAN_ACTION_CONFIG)

    _, _, _, _, infos = env.step(np.full((1, 1), 21.0, dtype=np.float64))

    info = infos[0]
    assert info["warning_count"] == 0.0
    assert "warnings" not in info


def test_vec_fallback_surfaces_rejected_control_signal(monkeypatch: pytest.MonkeyPatch):
    """A dispatch-time control rejection shows up in the fallback step info,
    and the messages are drained exactly once."""
    monkeypatch.setattr(vec_env_module, "rust_batch_step", None)
    env, dwellings = _make_vec_env(_REJECTED_ACTION_CONFIG)

    _, _, _, _, infos = env.step(np.full((1, 1), 70.0, dtype=np.float64))

    info = infos[0]
    assert info["warning_count"] >= 1.0
    assert any("control apply failed" in w for w in info["warnings"])
    assert any("Gas Furnace" in w for w in info["warnings"])
    # The messages were drained into info — a subsequent poll is empty.
    assert dwellings[0].take_warnings() == []


def test_vec_rust_path_preserves_warning_count_key():
    """The Rust fast path plumbs warning_count through VecDwellingGymEnv infos,
    so downstream code sees the same contract regardless of path."""
    if vec_env_module.rust_batch_step is None:
        pytest.skip("Rust batch_step entrypoint unavailable")
    env, _ = _make_vec_env(_CLEAN_ACTION_CONFIG)

    _, _, _, _, infos = env.step(np.full((1, 1), 21.0, dtype=np.float64))

    info = infos[0]
    assert info["warning_count"] == 0.0
    assert "warnings" not in info


# ---------------------------------------------------------------------------
# DwellingGymEnv — requires gymnasium (skips where absent)
# ---------------------------------------------------------------------------


def _make_gym_env(action_config: dict[str, list[str]]):
    pytest.importorskip("gymnasium")
    from ochre_next.rl.gym_env import DwellingGymEnv

    return DwellingGymEnv(
        config={
            "hpxml": HPXML,
            "schedule": SCHEDULE,
            "weather": WEATHER,
            "defaults_path": str(HARES_DEFAULTS),
            "start_time": "2019-01-01T00:00:00",
            "duration_s": 600,
            "time_res_s": 60,
        },
        observation_fields=_OBS_FIELDS,
        action_space_config=action_config,
        reward_fn=lambda ctx: -ctx["total_power_kw"],
        episode_length=timedelta(minutes=5),
    )


def test_gym_env_clean_step_reports_zero_warnings():
    """A clean DwellingGymEnv step surfaces warning_count == 0.0, no list."""
    env = _make_gym_env(_CLEAN_ACTION_CONFIG)
    env.reset(seed=42)

    _, _, _, _, info = env.step(np.array([21.0], dtype=np.float64))

    assert info["warning_count"] == 0.0
    assert "warnings" not in info


def test_gym_env_surfaces_rejected_control_signal():
    """A dispatch-time control rejection shows up in DwellingGymEnv step info,
    and the messages are drained exactly once."""
    env = _make_gym_env(_REJECTED_ACTION_CONFIG)
    env.reset(seed=42)

    _, _, _, _, info = env.step(np.array([70.0], dtype=np.float64))

    assert info["warning_count"] >= 1.0
    assert any("control apply failed" in w for w in info["warnings"])
    assert any("Gas Furnace" in w for w in info["warnings"])
    # The messages were drained into info — a subsequent poll is empty.
    assert env._dwelling.take_warnings() == []
