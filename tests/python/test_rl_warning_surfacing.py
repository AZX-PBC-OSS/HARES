"""Warning-surfacing contract tests for the RL step paths.

The Rust ``batch_step`` entrypoint (covered in test_gym_batch_step.py) drains
``Dwelling.take_warnings()`` into ``info["warning_count"]`` (float, always
present) and ``info["warnings"]`` (list[str], only when non-zero). These tests
pin that contract onto ``VecDwellingGymEnv.step``, which runs on the extension's
``batch_step`` alone, and onto ``DwellingGymEnv.step``, which drives one
dwelling from Python.

``VecDwellingGymEnv`` imports without gymnasium (numpy only), so its tests run
everywhere. ``DwellingGymEnv`` requires gymnasium at construction time and its
tests skip where that optional dependency is absent.
"""

from __future__ import annotations

from datetime import timedelta
from pathlib import Path

import numpy as np
import pytest
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
# VecDwellingGymEnv -- the extension's batch_step
# ---------------------------------------------------------------------------


def _make_vec_env(action_config: dict[str, list[str]]) -> tuple[VecDwellingGymEnv, list]:
    dwellings = [make_dwelling(duration_s=600)]
    # The fixture's construction notices (base.xml gives no ShieldingofHome,
    # which takes OS-HPXML's default) are not a step's warnings.
    for dwelling in dwellings:
        dwelling.take_warnings()
    env = VecDwellingGymEnv(
        dwellings=dwellings,
        observation_fields=_OBS_FIELDS,
        action_space_config=action_config,
        reward_fn=lambda ctx: -ctx["total_power_kw"],
        episode_length=timedelta(minutes=5),
    )
    return env, dwellings


def test_vec_rust_path_preserves_warning_count_key():
    """The extension's batch_step plumbs warning_count through
    VecDwellingGymEnv infos."""
    assert vec_env_module.rust_batch_step is not None, (
        "the extension registers batch_step; without it the module does not import"
    )
    env, _ = _make_vec_env(_CLEAN_ACTION_CONFIG)

    _, _, _, _, infos = env.step(np.full((1, 1), 21.0, dtype=np.float64))

    info = infos[0]
    assert info["warning_count"] == 0.0
    assert "warnings" not in info


def test_vec_env_refuses_to_import_without_the_extension_entrypoint(
    monkeypatch: pytest.MonkeyPatch,
):
    """A missing ``batch_step`` is an import error, not a silent Python
    fallback: an extension that does not register the entrypoint fails the
    module's import."""
    import importlib
    import importlib.util
    import sys
    import types

    from ochre_next import _hares as real_extension

    stub = types.ModuleType("ochre_next._hares")
    for name in dir(real_extension):
        if name != "batch_step" and not name.startswith("__"):
            setattr(stub, name, getattr(real_extension, name))

    real_module = importlib.import_module("ochre_next.rl.vec_env")
    module_file = real_module.__file__
    assert module_file is not None, "the vec env module is loaded from a file"
    spec = importlib.util.spec_from_file_location(
        "ochre_next.rl.vec_env_without_entrypoint",
        Path(module_file),
    )
    assert spec is not None and spec.loader is not None
    probe = importlib.util.module_from_spec(spec)

    monkeypatch.setitem(sys.modules, "ochre_next._hares", stub)
    with pytest.raises(ImportError, match="batch_step"):
        spec.loader.exec_module(probe)


# ---------------------------------------------------------------------------
# DwellingGymEnv — requires gymnasium (skips where absent)
# ---------------------------------------------------------------------------


def _make_gym_env(action_config: dict[str, list[str]], output_dir: Path):
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
            # The gym env's dict path cannot disable write_output; the recorder
            # lands in the test's own directory.
            "output_path": str(output_dir / "dwelling_0.csv"),
        },
        observation_fields=_OBS_FIELDS,
        action_space_config=action_config,
        reward_fn=lambda ctx: -ctx["total_power_kw"],
        episode_length=timedelta(minutes=5),
    )


def test_gym_env_clean_step_reports_zero_warnings(tmp_path: Path):
    """A clean DwellingGymEnv step surfaces warning_count == 0.0, no list."""
    env = _make_gym_env(_CLEAN_ACTION_CONFIG, tmp_path)
    env.reset(seed=42)
    # The first step's info carries the fixture's construction notices
    # (base.xml gives no ShieldingofHome); the next clean step has none.
    _, _, _, _, first = env.step(np.array([21.0], dtype=np.float64))
    assert all("ShieldingofHome" in w for w in first.get("warnings", []))

    _, _, _, _, info = env.step(np.array([21.0], dtype=np.float64))

    assert info["warning_count"] == 0.0
    assert "warnings" not in info


def test_gym_env_surfaces_rejected_control_signal(tmp_path: Path):
    """A dispatch-time control rejection shows up in DwellingGymEnv step info,
    and the messages are drained exactly once."""
    env = _make_gym_env(_REJECTED_ACTION_CONFIG, tmp_path)
    env.reset(seed=42)

    _, _, _, _, info = env.step(np.array([70.0], dtype=np.float64))

    assert info["warning_count"] >= 1.0
    assert any("control apply failed" in w for w in info["warnings"])
    assert any("Gas Furnace" in w for w in info["warnings"])
    # The messages were drained into info — a subsequent poll is empty.
    assert env._dwelling.take_warnings() == []
