"""Tests for DwellingGymEnv and VecDwellingGymEnv."""

from __future__ import annotations

from datetime import timedelta
from pathlib import Path

import pytest

np = pytest.importorskip("numpy")
pytest.importorskip("gymnasium")

from ochre_next._hares import Dwelling as PyDwelling
from ochre_next.rl.gym_env import DwellingGymEnv, _observation_field_bounds, _sorted_action_layout, telemetry_to_observation
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
_OBS_FIELDS = ["total_power_kw", "outdoor_temp"]


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


def test_apply_action_clips_at_bounds():
    """_apply_action clips action values to action_space.low/high
    before passing them to _build_control_signal."""
    env = _make_env()
    env.reset(seed=42)
    low = env.action_space.low[0]
    high = env.action_space.high[0]

    from ochre_next.rl import gym_env
    original = gym_env._build_control_signal
    captured = []

    def _spy(signal_type, values):
        captured.append(dict(values))
        return original(signal_type, values)

    gym_env._build_control_signal = _spy
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
        gym_env._build_control_signal = original


def test_vec_gym_out_of_bounds_actions_no_nan():
    """100-step random-policy test with actions 10x the declared range.

    Forces the Python fallback path (rust_batch_step = None) so that
    _apply_controls — the method this ticket patches — is actually exercised.
    Also spies on _build_control_signal to verify _apply_controls clips
    action values to the declared action space bounds before forwarding them.
    """
    import ochre_next.rl.vec_env as ve

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
    from ochre_next import Battery
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

    low = env.action_space.low
    high = env.action_space.high
    scale = 10.0
    rng = np.random.default_rng(42)

    # Resolve action layout indices once — _sorted_action_layout sorts
    # equipment names and fields alphabetically, so for the config above
    # the layout is [("Batt", "active_power_kw"), ("Gas Furnace", "heat_c")].
    heat_idx = next(i for i, (_, f) in enumerate(env._action_layout) if f == "heat_c")
    power_idx = next(i for i, (_, f) in enumerate(env._action_layout) if f == "active_power_kw")

    captured_signals = []
    original_build = ve._build_control_signal

    def _spy(signal_type, values):
        captured_signals.append((signal_type, dict(values)))
        return original_build(signal_type, values)

    ve._build_control_signal = _spy
    old_batch_step = ve.rust_batch_step
    ve.rust_batch_step = None
    try:
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
    finally:
        ve.rust_batch_step = old_batch_step
        ve._build_control_signal = original_build

    # Verify all captured action values are within action space bounds.
    assert captured_signals, "spy should have captured at least one signal"
    for _signal_type, values in captured_signals:
        if "heat_c" in values:
            val = float(values["heat_c"])
            assert low[heat_idx] <= val <= high[heat_idx], (
                f"heat_c {val} should be in [{low[heat_idx]}, {high[heat_idx]}]"
            )
        if "active_power_kw" in values:
            val = float(values["active_power_kw"])
            assert low[power_idx] <= val <= high[power_idx], (
                f"active_power_kw {val} should be in [{low[power_idx]}, {high[power_idx]}]"
            )


# ---------------------------------------------------------------------------
# Observation bounds tests (T-0339)
# ---------------------------------------------------------------------------


def test_observation_field_bounds_all_finite():
    """Every known observation key (and the fallback) returns finite bounds."""
    from ochre_next.rl.gym_env import _observation_field_bounds

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
        low, high = _observation_field_bounds(field)
        assert np.isfinite(low), f"low bound {low} is not finite for field {field!r}"
        assert np.isfinite(high), f"high bound {high} is not finite for field {field!r}"
        assert not np.isnan(low), f"low bound is NaN for field {field!r}"
        assert not np.isnan(high), f"high bound is NaN for field {field!r}"
        assert low < high, f"bounds inverted for {field!r}: {low} >= {high}"


def test_observation_space_bounds_all_finite():
    """Observation space arrays must contain no -inf or +inf values."""
    env = _make_env()
    low = np.asarray(env.observation_space.low, dtype=np.float64)
    high = np.asarray(env.observation_space.high, dtype=np.float64)
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
    low = env.observation_space.low
    high = env.observation_space.high
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
        low, high = _observation_field_bounds(field)
        assert low == expected[0] and high == expected[1], (
            f"{field!r}: expected {expected}, got ({low}, {high})"
        )


def test_observation_field_bounds_broad_fallback_is_finite():
    """Unrecognised fields return the broad but finite fallback, not ±inf."""
    for field in ("giraffe_temp_c", "  fluffy_rh  "):
        low, high = _observation_field_bounds(field)
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
    low = np.asarray(env.observation_space.low, dtype=np.float64)
    high = np.asarray(env.observation_space.high, dtype=np.float64)
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
    low, high = _observation_field_bounds("time_sin")
    assert low == -1.0 and high == 1.0, f"time_sin: ({low}, {high})"
    low, high = _observation_field_bounds("time_cos")
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
    low = np.asarray(env.observation_space.low, dtype=np.float64)
    high = np.asarray(env.observation_space.high, dtype=np.float64)
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
    import ochre_next.rl.gym_env as gym_env_mod
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
    import ochre_next.rl.vec_env as ve

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
            assert mask is not None, "reset info must include initial_observation_mask"
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
