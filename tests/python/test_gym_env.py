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
        """Fake that mirrors the typed static constructors on the real ControlSignal."""

        def __init__(self, kind: str, **kwargs) -> None:
            self._kind = kind
            self._fields: dict = kwargs

        def get(self, key, default=None):
            if key == "type":
                return self._kind
            return self._fields.get(key, default)

        def __getitem__(self, key):
            if key == "type":
                return self._kind
            return self._fields[key]

        @staticmethod
        def thermal_setpoint(heat_c=None, cool_c=None, deadband_c=None):
            return FakePyControlSignal("ThermalSetpoint", heat_c=heat_c, cool_c=cool_c, deadband_c=deadband_c)

        @staticmethod
        def power_setpoint(kw=0.0, reactive_kvar=None):
            return FakePyControlSignal("PowerSetpoint", active_power_kw=kw, reactive_power_kvar=reactive_kvar)

        @staticmethod
        def power_limit(max_power_kw=0.0, ramp_rate_kw_per_s=None):
            return FakePyControlSignal("PowerLimit", max_power_kw=max_power_kw, ramp_rate_kw_per_s=ramp_rate_kw_per_s)

        @staticmethod
        def soc_target(target=0.0, min=None, max=None):
            return FakePyControlSignal("SOCTarget", target_soc=target, min_soc=min, max_soc=max)

        @staticmethod
        def load_fraction(fraction=0.0):
            return FakePyControlSignal("LoadFraction", fraction=fraction)

        @staticmethod
        def duty_cycle(on_fraction=0.0, period_s=None, component=None):
            return FakePyControlSignal("DutyCycle", on_fraction=on_fraction, period_s=period_s)

        @staticmethod
        def humidity_setpoint(target_rh=0.0, min_rh=None, max_rh=None):
            return FakePyControlSignal("HumiditySetpoint", target_rh=target_rh, min_rh=min_rh, max_rh=max_rh)

        @staticmethod
        def grid_connect(connected=False):
            return FakePyControlSignal("GridConnect", connected=connected)

        @staticmethod
        def self_consumption(enabled=False, solar_only_charging=False):
            return FakePyControlSignal("SelfConsumption", enabled=enabled, solar_only_charging=solar_only_charging)

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
                self.soc = float(signal.get("target_soc") or self.soc)

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

    class FakeSimulationConfig:
        def __init__(self, start_time=None, duration=None, time_res=None,
                     output_verbosity=None, output_path=None, output_to_parquet=None,
                     output_chunk_size=None, master_seed=None, civil_timezone=None,
                     setpoint_deadband_c=None):
            self.start_time = start_time or ""
            self.duration = duration or 0
            self.time_res = time_res or 60
            self.output_verbosity = output_verbosity or 0
            self.output_path = output_path
            self.output_to_parquet = output_to_parquet or False
            self.output_chunk_size = output_chunk_size or 8760
            self.master_seed = master_seed or 0
            self.civil_timezone = civil_timezone
            self.setpoint_deadband_c = setpoint_deadband_c

    # Build a module that satisfies all imports in ochre_next/__init__.py.
    # Unrecognised names fall back to a generic stub so the package loads.
    _sentinel = type("_Stub", (), {})()

    fake_mod = types.ModuleType("ochre_next._hares")
    fake_mod.PyControlSignal = FakePyControlSignal
    fake_mod.ControlSignal = FakePyControlSignal
    fake_mod.SimulationConfig = FakeSimulationConfig
    fake_mod.PyDwelling = FakePyDwelling
    fake_mod.PyFleet = type("FakePyFleet", (), {})
    fake_mod.batch_step = fake_batch_step
    # Provide stubs for every other name __init__.py tries to import.
    for _name in (
        "PyFleetResults", "DwellingConfig", "PyTelemetry",
        "Battery", "PV", "PvSoilingConfig", "EV",
        "Actor", "DispatchRequest", "Priority", "Signal",
        "EndUse", "FuelType", "OperatingMode", "Mode", "ExecutionStage",
        "FluidType", "InverterPriority", "DutyCycleComponent", "SimStatus",
        "AggregationResolution", "ResStockVersion", "ControlCapabilities",
        "LutType", "BatteryChemistry", "ChargingLevel", "DriverArchetype",
        "DRLevel", "EquipmentDescriptor", "TelemetryField",
        "PySimulationMetrics", "PyAnnualEnergyKwh", "PyPeakPowerKw",
        "PyRollingPeakKw", "PyGridInteractionMetrics",
        "PyEnvelopeComponentLoadsKwh", "PyEfficiencyMetrics",
        "PyGasEnergyMetrics",
        "RoofPlane", "PvCandidate", "PvSizingResult",
        "parse_weather", "parse_epw", "parse_psm3", "parse_tmy3",
        "parse_resstock_csv", "WeatherTimeSeries",
    ):
        if not hasattr(fake_mod, _name):
            setattr(fake_mod, _name, _sentinel)

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
