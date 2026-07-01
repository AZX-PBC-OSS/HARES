"""Gymnasium environment for single-dwelling RL control."""

from __future__ import annotations

from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from datetime import timedelta
import math
import secrets
from typing import Any, TypedDict

import numpy as np

from ochre_next._hares import Dwelling as PyDwelling, SimulationConfig, ControlSignal as PyControlSignal

try:
    import gymnasium as gym
    from gymnasium import spaces

    _GYM_BASE = gym.Env
except ImportError:  # pragma: no cover - optional dependency
    gym = None
    spaces = None
    _GYM_BASE = object

_BROAD_LOW = -1.0e6
_BROAD_HIGH = 1.0e6


# ---------------------------------------------------------------------------
# Typed config
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class GymDwellingConfig:
    """Configuration for a single dwelling used by the gym environment.

    Use ``sim_config`` to pass simulation-level settings (time resolution,
    output verbosity, etc.) instead of the old untyped ``kwargs`` dict.
    """

    hpxml: str
    schedule: str
    weather: str
    defaults_path: str | None = None
    sim_config: SimulationConfig | None = None


# ---------------------------------------------------------------------------
# Typed return contracts
# ---------------------------------------------------------------------------


class StepInfo(TypedDict):
    seed: int
    timestep_index: int
    step: dict[str, Any]


class StepResult(TypedDict):
    obs: np.ndarray
    reward: float
    terminated: bool
    truncated: bool
    info: StepInfo


class RewardContext(TypedDict):
    step: dict[str, Any]
    telemetry_zone: dict[str, Any]
    telemetry_equipment: dict[str, Any]
    total_power_kw: float


# ---------------------------------------------------------------------------
# Internal helpers
# ---------------------------------------------------------------------------


def _coerce_seconds(value: Any) -> float:
    if isinstance(value, timedelta):
        return value.total_seconds()
    if isinstance(value, (int, float)):
        return float(value)
    return 60.0


def _normalize_config(config: GymDwellingConfig | Mapping[str, Any]) -> GymDwellingConfig:
    if isinstance(config, GymDwellingConfig):
        return config
    # Legacy dict path: extract known SimulationConfig fields into a
    # SimulationConfig object so the result is always typed.
    raw_time_res = config.get("time_res_s")
    raw_duration = config.get("duration_s")
    time_res_s: int | None = int(_coerce_seconds(raw_time_res)) if raw_time_res is not None else None
    duration_s: int | None = int(_coerce_seconds(raw_duration)) if raw_duration is not None else None
    sim_config = SimulationConfig(
        start_time=str(config["start_time"]) if "start_time" in config else None,
        duration_s=duration_s,
        time_res_s=time_res_s,
        output_verbosity=int(config["output_verbosity"]) if "output_verbosity" in config else None,
        output_path=str(config["output_path"]) if "output_path" in config else None,
        output_to_parquet=bool(config["output_to_parquet"]) if "output_to_parquet" in config else None,
        output_chunk_size=int(config["output_chunk_size"]) if "output_chunk_size" in config else None,
        master_seed=int(config["master_seed"]) if "master_seed" in config else None,
    )
    return GymDwellingConfig(
        hpxml=str(config["hpxml"]),
        schedule=str(config["schedule"]),
        weather=str(config["weather"]),
        defaults_path=str(config["defaults_path"]) if "defaults_path" in config else None,
        sim_config=sim_config,
    )


def _sim_config_to_kwargs(sim_config: SimulationConfig) -> dict[str, Any]:
    """Extract non-None fields from a SimulationConfig into a flat kwargs dict
    suitable for ``PyDwelling.from_hpxml``."""
    out: dict[str, Any] = {}
    if sim_config.start_time:
        out["start_time"] = sim_config.start_time
    if sim_config.duration_s:
        out["duration_s"] = sim_config.duration_s
    if sim_config.time_res_s:
        out["time_res_s"] = sim_config.time_res_s
    if sim_config.output_verbosity is not None:
        out["output_verbosity"] = sim_config.output_verbosity
    if sim_config.output_path is not None:
        out["output_path"] = sim_config.output_path
    out["output_to_parquet"] = sim_config.output_to_parquet
    out["output_chunk_size"] = sim_config.output_chunk_size
    if sim_config.master_seed:
        out["master_seed"] = sim_config.master_seed
    if sim_config.civil_timezone is not None:
        out["civil_timezone"] = sim_config.civil_timezone
    if sim_config.setpoint_deadband_c is not None:
        out["setpoint_deadband_c"] = sim_config.setpoint_deadband_c
    return out


def _sorted_action_layout(
    action_space_config: Mapping[str, Sequence[str]],
) -> tuple[list[tuple[str, str]], dict[str, str]]:
    layout: list[tuple[str, str]] = []
    type_by_equipment: dict[str, str] = {}
    for equipment in sorted(action_space_config):
        fields = sorted(str(field) for field in action_space_config[equipment])
        if not fields:
            raise ValueError(f"action_space_config[{equipment!r}] must not be empty")
        sig_type = _infer_signal_type(fields)
        type_by_equipment[equipment] = sig_type
        for field in fields:
            layout.append((equipment, field))
    return layout, type_by_equipment


def _infer_signal_type(fields: Sequence[str]) -> str:
    normalized = {field.lower() for field in fields}
    if normalized & {"heating_setpoint_c", "cooling_setpoint_c", "deadband_c", "heat_c", "cool_c", "setpoint_c"}:
        return "ThermalSetpoint"
    if normalized & {"active_power_kw", "reactive_power_kvar", "p_setpoint_kw", "kw"}:
        return "PowerSetpoint"
    if normalized & {"max_power_kw", "ramp_rate_kw_per_s"}:
        return "PowerLimit"
    if normalized & {"target_soc", "soc", "min_soc", "max_soc"}:
        return "SOCTarget"
    if normalized & {"fraction", "load_fraction"}:
        return "LoadFraction"
    if normalized & {"on_fraction", "duty_cycle", "period_s"}:
        return "DutyCycle"
    if normalized & {"target_rh", "min_rh", "max_rh"}:
        return "HumiditySetpoint"
    if normalized & {"connected"}:
        return "GridConnect"
    if normalized & {"enabled", "solar_only_charging"}:
        return "SelfConsumption"
    raise ValueError(f"unsupported action field set: {sorted(fields)}")


def _field_bounds(field: str) -> tuple[float, float]:
    name = field.lower()
    if name in {"soc", "target_soc", "min_soc", "max_soc", "fraction", "load_fraction", "on_fraction", "duty_cycle", "target_rh", "min_rh", "max_rh"}:
        return (0.0, 1.0)
    if name in {"connected", "enabled", "solar_only_charging"}:
        return (0.0, 1.0)
    if name in {"setpoint_c", "heat_c", "cool_c", "heating_setpoint_c", "cooling_setpoint_c"}:
        return (-50.0, 80.0)
    if name in {"deadband_c"}:
        return (0.0, 30.0)
    return (_BROAD_LOW, _BROAD_HIGH)


def _bracket_name(field: str, prefix: str) -> str | None:
    head = f"{prefix}["
    if not field.startswith(head) or not field.endswith("]"):
        return None
    return field[len(head) : -1]


def _index_map(names: Sequence[str]) -> dict[str, int]:
    return {name.strip().lower(): idx for idx, name in enumerate(names)}


# ---------------------------------------------------------------------------
# Public helpers
# ---------------------------------------------------------------------------


def telemetry_to_observation(telemetry: Any, observation_fields: Sequence[str]) -> np.ndarray:
    zone = telemetry.zone()
    equipment = telemetry.equipment()
    zone_idx = _index_map(list(zone.get("names", [])))
    equip_idx = _index_map(list(equipment.get("names", [])))

    out: list[float] = []
    for field in observation_fields:
        key = str(field)
        if key in {"outdoor_temp", "outdoor_temp_c"}:
            out.append(float(zone.get("outdoor_temp_c", 0.0)))
            continue
        if key in {"outdoor_humidity_ratio"}:
            out.append(float(zone.get("outdoor_humidity_ratio", 0.0)))
            continue
        if key in {"total_power_kw", "total_electric_kw"}:
            out.append(float(telemetry.total_power_kw))
            continue

        zone_name = _bracket_name(key, "zone_temp")
        if zone_name is not None:
            out.append(float(zone["temperature_c"][zone_idx[zone_name.strip().lower()]]))
            continue
        zone_name = _bracket_name(key, "setpoint_heat")
        if zone_name is not None:
            out.append(float(zone["setpoint_heat_c"][zone_idx[zone_name.strip().lower()]]))
            continue
        zone_name = _bracket_name(key, "setpoint_cool")
        if zone_name is not None:
            out.append(float(zone["setpoint_cool_c"][zone_idx[zone_name.strip().lower()]]))
            continue
        equip_name = _bracket_name(key, "equipment_soc")
        if equip_name is not None:
            out.append(float(equipment["soc"][equip_idx[equip_name.strip().lower()]]))
            continue
        equip_name = _bracket_name(key, "equipment_power")
        if equip_name is not None:
            out.append(float(equipment["power_kw"][equip_idx[equip_name.strip().lower()]]))
            continue

        if key == "battery_soc":
            out.append(float(equipment["soc"][equip_idx["battery"]]))
            continue
        if key == "ev_soc":
            out.append(float(equipment["soc"][equip_idx["electric vehicle"]]))
            continue

        raise KeyError(f"unknown observation field `{key}`")

    obs = np.asarray(out, dtype=np.float64)
    return np.ascontiguousarray(obs)


def _build_control_signal(signal_type: str, values: Mapping[str, float]) -> Any:
    """Build a typed ``PyControlSignal`` from a signal type name and field values.

    All branches call the typed static constructors on ``PyControlSignal``
    rather than building untyped dicts.
    """
    lower = {k.lower(): float(v) for k, v in values.items()}
    if signal_type == "ThermalSetpoint":
        return PyControlSignal.thermal_setpoint(
            heat_c=lower.get("heating_setpoint_c", lower.get("heat_c", lower.get("setpoint_c"))),
            cool_c=lower.get("cooling_setpoint_c", lower.get("cool_c")),
            deadband_c=lower.get("deadband_c"),
        )
    if signal_type == "PowerSetpoint":
        return PyControlSignal.power_setpoint(
            kw=lower.get("active_power_kw", lower.get("p_setpoint_kw", lower.get("kw", 0.0))),
            reactive_kvar=lower.get("reactive_power_kvar"),
        )
    if signal_type == "PowerLimit":
        return PyControlSignal.power_limit(
            max_power_kw=lower.get("max_power_kw", 0.0),
            ramp_rate_kw_per_s=lower.get("ramp_rate_kw_per_s"),
        )
    if signal_type == "SOCTarget":
        return PyControlSignal.soc_target(
            target=lower.get("target_soc", lower.get("soc", 0.0)),
            min=lower.get("min_soc"),
            max=lower.get("max_soc"),
        )
    if signal_type == "LoadFraction":
        return PyControlSignal.load_fraction(
            fraction=lower.get("fraction", lower.get("load_fraction", 0.0)),
        )
    if signal_type == "DutyCycle":
        return PyControlSignal.duty_cycle(
            on_fraction=lower.get("on_fraction", lower.get("duty_cycle", 0.0)),
            period_s=lower.get("period_s"),
        )
    if signal_type == "HumiditySetpoint":
        return PyControlSignal.humidity_setpoint(
            target_rh=lower.get("target_rh", 0.0),
            min_rh=lower.get("min_rh"),
            max_rh=lower.get("max_rh"),
        )
    if signal_type == "GridConnect":
        return PyControlSignal.grid_connect(
            connected=lower.get("connected", 0.0) >= 0.5,
        )
    if signal_type == "SelfConsumption":
        return PyControlSignal.self_consumption(
            enabled=lower.get("enabled", 0.0) >= 0.5,
            solar_only_charging=lower.get("solar_only_charging", 0.0) >= 0.5,
        )
    raise ValueError(f"unsupported signal type: {signal_type!r}")


# ---------------------------------------------------------------------------
# Gymnasium environment
# ---------------------------------------------------------------------------


class DwellingGymEnv(_GYM_BASE):
    def __init__(
        self,
        config: GymDwellingConfig | Mapping[str, Any],
        observation_fields: Sequence[str],
        action_space_config: Mapping[str, Sequence[str]],
        reward_fn: Callable[[RewardContext], float],
        episode_length: timedelta,
    ) -> None:
        if gym is None or spaces is None:
            raise ImportError("gymnasium is required for RL environments")

        self._config = _normalize_config(config)
        self._observation_fields = [str(field) for field in observation_fields]
        self._reward_fn = reward_fn
        self._episode_length = episode_length
        self._action_layout, self._signal_type_by_equipment = _sorted_action_layout(
            action_space_config
        )

        hpxml_kwargs: dict[str, Any] = {}
        if self._config.defaults_path is not None:
            hpxml_kwargs["defaults_path"] = self._config.defaults_path
        if self._config.sim_config is not None:
            hpxml_kwargs.update(_sim_config_to_kwargs(self._config.sim_config))

        self._dwelling = PyDwelling.from_hpxml(
            hpxml=self._config.hpxml,
            schedule=self._config.schedule,
            weather=self._config.weather,
            **hpxml_kwargs,
        )
        self._dwelling.initialize()
        self._initial_snapshot = bytes(self._dwelling.save_state())

        self.observation_space = spaces.Box(
            low=np.full(len(self._observation_fields), -np.inf, dtype=np.float64),
            high=np.full(len(self._observation_fields), np.inf, dtype=np.float64),
            shape=(len(self._observation_fields),),
            dtype=np.float64,
        )

        action_low: list[float] = []
        action_high: list[float] = []
        for _, field in self._action_layout:
            low, high = _field_bounds(field)
            action_low.append(low)
            action_high.append(high)
        self.action_space = spaces.Box(
            low=np.asarray(action_low, dtype=np.float64),
            high=np.asarray(action_high, dtype=np.float64),
            shape=(len(self._action_layout),),
            dtype=np.float64,
        )

        time_res_s: float = 60.0
        if self._config.sim_config is not None and self._config.sim_config.time_res_s:
            time_res_s = float(self._config.sim_config.time_res_s)
        self._max_steps = max(1, int(math.ceil(episode_length.total_seconds() / time_res_s)))
        self._steps_elapsed: int = 0
        self._active_seed: int | None = None

    def _observation(self) -> np.ndarray:
        obs = telemetry_to_observation(self._dwelling.telemetry(), self._observation_fields)
        return np.ascontiguousarray(obs, dtype=np.float64)

    def _apply_action(self, action: np.ndarray) -> None:
        low = self.action_space.low
        high = self.action_space.high
        action = np.clip(action, low, high)
        action_values: dict[str, dict[str, float]] = {}
        for idx, (equipment, field) in enumerate(self._action_layout):
            action_values.setdefault(equipment, {})[field] = float(action[idx])

        for equipment in sorted(action_values):
            signal = _build_control_signal(
                self._signal_type_by_equipment[equipment],
                action_values[equipment],
            )
            self._dwelling.apply_control(equipment, signal)

    def reset(
        self,
        *,
        seed: int | None = None,
        options: dict[str, Any] | None = None,
    ) -> tuple[np.ndarray, dict[str, Any]]:
        del options
        if gym is not None:
            super().reset(seed=seed)

        self._dwelling.load_state(self._initial_snapshot)
        applied_seed = int(seed) if seed is not None else secrets.randbits(63)
        self._dwelling.reset_with_seed(applied_seed)
        self._steps_elapsed = 0
        self._active_seed = applied_seed

        obs = self._observation()
        return obs, {"seed": applied_seed}

    def step(
        self, action: np.ndarray
    ) -> tuple[np.ndarray, float, bool, bool, StepInfo]:
        arr = np.asarray(action, dtype=np.float64)
        arr = np.ascontiguousarray(arr.reshape(-1))
        if arr.shape != (len(self._action_layout),):
            raise ValueError(
                f"expected action shape {(len(self._action_layout),)}, got {arr.shape}"
            )

        self._apply_action(arr)
        step_data: dict[str, Any] = self._dwelling.step()
        telemetry = self._dwelling.telemetry()
        obs = telemetry_to_observation(telemetry, self._observation_fields)
        self._steps_elapsed += 1
        truncated = self._steps_elapsed >= self._max_steps

        reward_context = RewardContext(
            step=step_data,
            telemetry_zone=telemetry.zone(),
            telemetry_equipment=telemetry.equipment(),
            total_power_kw=float(telemetry.total_power_kw),
        )
        reward = float(self._reward_fn(reward_context))
        info = StepInfo(
            step=step_data,
            seed=self._active_seed if self._active_seed is not None else 0,
            timestep_index=self._steps_elapsed,
        )
        return obs, reward, False, truncated, info
