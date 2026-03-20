"""Gymnasium environment for single-dwelling RL control."""

from __future__ import annotations

from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass, field as dataclass_field
from datetime import timedelta
import math
import secrets
from typing import Any

import numpy as np

from ochre_next._hares import PyDwelling

try:
    from ochre_next._hares import PyControlSignal
except ImportError:
    from ochre_next._hares import ControlSignal as PyControlSignal

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


@dataclass(frozen=True)
class DwellingConfig:
    hpxml: str
    schedule: str
    weather: str
    kwargs: dict[str, Any] = dataclass_field(default_factory=dict)


def _coerce_seconds(value: Any) -> float:
    if isinstance(value, timedelta):
        return value.total_seconds()
    if isinstance(value, (int, float)):
        return float(value)
    return 60.0


def _normalize_config(config: DwellingConfig | Mapping[str, Any]) -> DwellingConfig:
    if isinstance(config, DwellingConfig):
        return config
    kwargs = dict(config.get("kwargs", {}))
    for key in (
        "start_time",
        "time_res",
        "duration",
        "initialization_duration",
        "output_to_parquet",
        "output_path",
        "output_verbosity",
        "output_chunk_size",
        "bldg_id",
        "master_seed",
    ):
        if key in config and key not in kwargs:
            kwargs[key] = config[key]
    return DwellingConfig(
        hpxml=str(config["hpxml"]),
        schedule=str(config["schedule"]),
        weather=str(config["weather"]),
        kwargs=kwargs,
    )


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
        if key in {"outdoor_rh"}:
            out.append(float(zone.get("outdoor_rh", 0.0)))
            continue
        if key in {"total_power_kw", "total_electric_kw"}:
            out.append(float(telemetry.total_power_kw()))
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


def action_payload(signal_type: str, values: Mapping[str, float]) -> dict[str, Any]:
    lower = {k.lower(): float(v) for k, v in values.items()}
    payload: dict[str, Any] = {"type": signal_type}
    if signal_type == "ThermalSetpoint":
        payload["heating_setpoint_c"] = lower.get("heating_setpoint_c", lower.get("heat_c", lower.get("setpoint_c")))
        payload["cooling_setpoint_c"] = lower.get("cooling_setpoint_c", lower.get("cool_c"))
        payload["deadband_c"] = lower.get("deadband_c")
    elif signal_type == "PowerSetpoint":
        payload["active_power_kw"] = lower.get("active_power_kw", lower.get("p_setpoint_kw", lower.get("kw", 0.0)))
        payload["reactive_power_kvar"] = lower.get("reactive_power_kvar")
    elif signal_type == "PowerLimit":
        payload["max_power_kw"] = lower.get("max_power_kw", 0.0)
        payload["ramp_rate_kw_per_s"] = lower.get("ramp_rate_kw_per_s")
    elif signal_type == "SOCTarget":
        payload["target_soc"] = lower.get("target_soc", lower.get("soc", 0.0))
        payload["min_soc"] = lower.get("min_soc")
        payload["max_soc"] = lower.get("max_soc")
    elif signal_type == "LoadFraction":
        payload["fraction"] = lower.get("fraction", lower.get("load_fraction", 0.0))
    elif signal_type == "DutyCycle":
        payload["on_fraction"] = lower.get("on_fraction", lower.get("duty_cycle", 0.0))
        payload["period_s"] = lower.get("period_s")
    elif signal_type == "HumiditySetpoint":
        payload["target_rh"] = lower.get("target_rh", 0.0)
        payload["min_rh"] = lower.get("min_rh")
        payload["max_rh"] = lower.get("max_rh")
    elif signal_type == "GridConnect":
        payload["connected"] = lower.get("connected", 0.0) >= 0.5
    elif signal_type == "SelfConsumption":
        payload["enabled"] = lower.get("enabled", 0.0) >= 0.5
        payload["solar_only_charging"] = lower.get("solar_only_charging", 0.0) >= 0.5
    return payload


class DwellingGymEnv(_GYM_BASE):
    def __init__(
        self,
        config: DwellingConfig | Mapping[str, Any],
        observation_fields: Sequence[str],
        action_space_config: Mapping[str, Sequence[str]],
        reward_fn: Callable[[dict[str, Any]], float],
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

        self._dwelling = PyDwelling.from_hpxml(
            hpxml=self._config.hpxml,
            schedule=self._config.schedule,
            weather=self._config.weather,
            **self._config.kwargs,
        )
        self._dwelling.initialize()
        self._initial_snapshot = bytes(self._dwelling.save_state())

        self.observation_space = spaces.Box(
            low=np.full(len(self._observation_fields), -np.inf, dtype=np.float64),
            high=np.full(len(self._observation_fields), np.inf, dtype=np.float64),
            shape=(len(self._observation_fields),),
            dtype=np.float64,
        )

        action_low = []
        action_high = []
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

        time_res_seconds = _coerce_seconds(self._config.kwargs.get("time_res", 60.0))
        self._max_steps = max(1, int(math.ceil(episode_length.total_seconds() / time_res_seconds)))
        self._steps_elapsed = 0
        self._active_seed: int | None = None

    def _observation(self) -> np.ndarray:
        obs = telemetry_to_observation(self._dwelling.telemetry(), self._observation_fields)
        return np.ascontiguousarray(obs, dtype=np.float64)

    def _apply_action(self, action: np.ndarray) -> None:
        action_values: dict[str, dict[str, float]] = {}
        for idx, (equipment, field) in enumerate(self._action_layout):
            action_values.setdefault(equipment, {})[field] = float(action[idx])

        for equipment in sorted(action_values):
            payload = action_payload(
                self._signal_type_by_equipment[equipment],
                action_values[equipment],
            )
            signal = PyControlSignal.from_dict(payload)
            self._dwelling.apply_control(equipment, signal)

    def reset(self, *, seed: int | None = None, options: dict[str, Any] | None = None) -> tuple[np.ndarray, dict[str, Any]]:
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

    def step(self, action: np.ndarray) -> tuple[np.ndarray, float, bool, bool, dict[str, Any]]:
        arr = np.asarray(action, dtype=np.float64)
        arr = np.ascontiguousarray(arr.reshape(-1))
        if arr.shape != (len(self._action_layout),):
            raise ValueError(
                f"expected action shape {(len(self._action_layout),)}, got {arr.shape}"
            )

        self._apply_action(arr)
        step_data = self._dwelling.step()
        telemetry = self._dwelling.telemetry()
        obs = telemetry_to_observation(telemetry, self._observation_fields)
        self._steps_elapsed += 1
        truncated = self._steps_elapsed >= self._max_steps

        reward_context = {
            "step": step_data,
            "telemetry": {
                "zone": telemetry.zone(),
                "equipment": telemetry.equipment(),
                "total_power_kw": telemetry.total_power_kw(),
            },
        }
        reward = float(self._reward_fn(reward_context))
        info = {
            "step": step_data,
            "seed": self._active_seed,
            "timestep_index": self._steps_elapsed,
        }
        return obs, reward, False, truncated, info
