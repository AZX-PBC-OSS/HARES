from __future__ import annotations

from datetime import timedelta
import importlib
import json
import sys
import types

import pytest

np = pytest.importorskip("numpy")
pytest.importorskip("gymnasium")


def _install_fake_hares(monkeypatch: pytest.MonkeyPatch):
    class FakePyControlSignal:
        @classmethod
        def from_dict(cls, d):
            return d

    class FakeTelemetry:
        def __init__(self, total_power_kw: float, soc: float) -> None:
            self._total_power_kw = total_power_kw
            self._soc = soc

        def zone(self):
            return {
                "names": ["Indoor"],
                "temperature_c": [21.0],
                "setpoint_heat_c": [20.0],
                "setpoint_cool_c": [24.0],
                "outdoor_temp_c": 10.0,
                "outdoor_rh": 0.45,
            }

        def equipment(self):
            return {
                "names": ["Battery"],
                "modes": [0.0],
                "states": [0.0],
                "soc": [self._soc],
                "power_kw": [self._total_power_kw],
            }

        def total_power_kw(self):
            return self._total_power_kw

    class FakePyDwelling:
        _batch_calls = 0

        def __init__(self) -> None:
            self.apply_calls = []
            self.seed = 0
            self.step_idx = 0
            self.soc = 0.5
            self._initialized = False

        @classmethod
        def from_hpxml(cls, hpxml, schedule, weather, **kwargs):
            _ = (hpxml, schedule, weather, kwargs)
            return cls()

        def initialize(self):
            self._initialized = True

        def save_state(self):
            return json.dumps({"seed": self.seed, "step": self.step_idx, "soc": self.soc}).encode()

        def load_state(self, state: bytes):
            data = json.loads(state.decode())
            self.seed = int(data["seed"])
            self.step_idx = int(data["step"])
            self.soc = float(data["soc"])

        def reset_with_seed(self, seed: int):
            self.seed = int(seed)
            self.step_idx = 0
            self.soc = 0.5

        def apply_control(self, name, signal):
            self.apply_calls.append((name, signal))
            if signal.get("type") == "SOCTarget":
                self.soc = float(signal.get("target_soc", self.soc))

        def step(self):
            self.step_idx += 1
            total = (self.seed % 10) * 0.1 + self.step_idx + self.soc
            return {"net_electric_power_kw": total, "time": f"t{self.step_idx}"}

        def telemetry(self):
            total = (self.seed % 10) * 0.1 + self.step_idx + self.soc
            return FakeTelemetry(total_power_kw=total, soc=self.soc)

    def fake_batch_step(dwellings, actions, observation_fields):
        _ = actions
        FakePyDwelling._batch_calls += 1
        out = []
        for dwelling in dwellings:
            step = dwelling.step()
            total = float(step["net_electric_power_kw"])
            obs = [total for _ in observation_fields]
            out.append(
                {
                    "obs": obs,
                    "reward": -total,
                    "terminated": False,
                    "truncated": False,
                    "info": {"net_electric_power_kw": total},
                }
            )
        return out

    fake_mod = types.ModuleType("ochre_next._hares")
    fake_mod.PyControlSignal = FakePyControlSignal
    fake_mod.PyDwelling = FakePyDwelling
    fake_mod.PyFleet = type("FakePyFleet", (), {})
    fake_mod.batch_step = fake_batch_step

    monkeypatch.setitem(sys.modules, "ochre_next._hares", fake_mod)
    for module in [
        "ochre_next",
        "ochre_next.rl",
        "ochre_next.rl.gym_env",
        "ochre_next.rl.vec_env",
    ]:
        sys.modules.pop(module, None)

    import ochre_next.rl as rl_mod

    return importlib.reload(rl_mod), FakePyDwelling


def test_dwelling_gym_reset_seed_reproducible(monkeypatch: pytest.MonkeyPatch):
    rl_mod, _ = _install_fake_hares(monkeypatch)

    env = rl_mod.DwellingGymEnv(
        config={"hpxml": "in.xml", "schedule": "s.csv", "weather": "w.epw"},
        observation_fields=["total_power_kw", "battery_soc"],
        action_space_config={"Battery": ["soc"]},
        reward_fn=lambda ctx: -float(ctx["step"]["net_electric_power_kw"]),
        episode_length=timedelta(minutes=10),
    )

    env.reset(seed=42)
    seq1 = [env.step(np.array([0.6], dtype=np.float64))[0].copy() for _ in range(3)]
    env.reset(seed=42)
    seq2 = [env.step(np.array([0.6], dtype=np.float64))[0].copy() for _ in range(3)]

    for left, right in zip(seq1, seq2, strict=True):
        assert np.allclose(left, right)


def test_dwelling_gym_spaces_and_mapping(monkeypatch: pytest.MonkeyPatch):
    rl_mod, _ = _install_fake_hares(monkeypatch)

    env = rl_mod.DwellingGymEnv(
        config={"hpxml": "in.xml", "schedule": "s.csv", "weather": "w.epw"},
        observation_fields=["total_power_kw", "battery_soc"],
        action_space_config={"B": ["reactive_power_kvar", "active_power_kw"], "A": ["active_power_kw"]},
        reward_fn=lambda ctx: -float(ctx["step"]["net_electric_power_kw"]),
        episode_length=timedelta(minutes=10),
    )

    env.reset(seed=1)
    obs, _, _, _, _ = env.step(np.array([1.0, 2.0, 3.0], dtype=np.float64))
    assert obs.shape == (2,)
    assert obs.dtype == np.float64
    assert env.observation_space.contains(obs)

    apply_calls = env._dwelling.apply_calls
    assert apply_calls[0][0] == "A"
    assert apply_calls[0][1]["active_power_kw"] == 1.0
    assert apply_calls[1][0] == "B"
    assert apply_calls[1][1]["active_power_kw"] == 2.0
    assert apply_calls[1][1]["reactive_power_kvar"] == 3.0


def test_vec_gym_step_batch_shape(monkeypatch: pytest.MonkeyPatch):
    rl_mod, fake_dwelling_cls = _install_fake_hares(monkeypatch)

    dwellings = [fake_dwelling_cls.from_hpxml("a", "b", "c") for _ in range(4)]
    env = rl_mod.VecDwellingGymEnv(
        dwellings=dwellings,
        observation_fields=["total_power_kw", "battery_soc"],
        action_space_config={"Battery": ["soc"]},
        reward_fn=lambda ctx: -float(ctx["step"]["net_electric_power_kw"]),
        episode_length=timedelta(minutes=10),
    )

    obs, _ = env.reset(seed=123)
    assert obs.shape == (4, 2)

    step_obs, rewards, dones, truncs, infos = env.step(np.full((4, 1), 0.7, dtype=np.float64))
    assert step_obs.shape == (4, 2)
    assert rewards.shape == (4,)
    assert dones.shape == (4,)
    assert truncs.shape == (4,)
    assert len(infos) == 4
    assert fake_dwelling_cls._batch_calls == 1


def test_vec_gym_rejects_fork_start_method(monkeypatch: pytest.MonkeyPatch):
    rl_mod, fake_dwelling_cls = _install_fake_hares(monkeypatch)
    monkeypatch.setattr("multiprocessing.get_start_method", lambda allow_none=True: "fork")

    dwellings = [fake_dwelling_cls.from_hpxml("a", "b", "c")]
    with pytest.raises(RuntimeError, match=r"fork \+ Rayon"):
        rl_mod.VecDwellingGymEnv(
            dwellings=dwellings,
            observation_fields=["total_power_kw"],
            action_space_config={"Battery": ["soc"]},
            reward_fn=lambda _: 0.0,
            episode_length=timedelta(minutes=1),
        )
