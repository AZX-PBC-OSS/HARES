"""Gymnasium environment for single-dwelling RL control."""

from __future__ import annotations

from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from datetime import timedelta
import math
import secrets
from typing import TYPE_CHECKING, Any, TypedDict, cast
import warnings

import numpy as np

from hares._hares import (
    ControlSignal as PyControlSignal,
    Dwelling as PyDwelling,
    SimulationConfig,
    clip_action,
)

if TYPE_CHECKING:
    import gymnasium as gym
    from gymnasium import spaces

    # The extension's step() returns a plain dict at runtime; its declared
    # shape lives in hares._hares_types.
    from hares._hares_types import StepResult

    _GYM_BASE = gym.Env
else:
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

_warned_initial_nan: bool = False


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
    defaults: str | None = None
    sim_config: SimulationConfig | None = None


# ---------------------------------------------------------------------------
# Typed return contracts
# ---------------------------------------------------------------------------


class RewardContext(TypedDict):
    step: StepResult
    telemetry_zone: dict[str, Any]
    telemetry_equipment: dict[str, Any]
    total_power_w: float


# ---------------------------------------------------------------------------
# Internal helpers
# ---------------------------------------------------------------------------


def _normalize_config(config: object) -> GymDwellingConfig:
    """The gym accepts only its typed config: any other value, a mapping
    among them, is the caller's error, named by type."""
    if not isinstance(config, GymDwellingConfig):
        raise TypeError(
            f"config must be a GymDwellingConfig, got {type(config).__name__}"
        )
    return config


def validate_reward_fn(reward_fn: object) -> None:
    """The reward is the callable's own or, for None, the batch path's
    default; any other value is the caller's error, named by field and
    type, refused at construction instead of the first step."""
    if reward_fn is not None and not callable(reward_fn):
        raise TypeError(
            f"reward_fn must be callable or None, got {type(reward_fn).__name__}"
        )


def validated_observation_fields(observation_fields: object) -> list[str]:
    """Only a list or tuple of strings is accepted: a bare string would
    iterate per character, and a non-string element would fail later and
    far from the cause. The error names the field, the type and the
    value."""
    if not isinstance(observation_fields, (list, tuple)):
        raise TypeError(
            "observation_fields must be a list or tuple of strings, "
            f"got {type(observation_fields).__name__}: {observation_fields!r}"
        )
    fields = cast("list[object] | tuple[object, ...]", observation_fields)
    out: list[str] = []
    for field in fields:
        if not isinstance(field, str):
            raise TypeError(
                "observation_fields must be a list or tuple of strings, "
                f"got element {field!r} of type {type(field).__name__}"
            )
        out.append(field)
    return out


def validate_episode_length(episode_length: object) -> None:
    """An episode must be a positive timedelta: a non-positive one would
    otherwise coerce to a silent one-step episode, and any other type
    would fail on its first attribute access. The error names the field
    and the value."""
    if not isinstance(episode_length, timedelta):
        raise TypeError(
            f"episode_length must be a timedelta, got {type(episode_length).__name__}"
        )
    if episode_length.total_seconds() <= 0:
        raise ValueError(
            f"episode_length must be at least one step, got {episode_length}"
        )


def sorted_action_layout(
    action_space_config: Mapping[str, Sequence[str]],
) -> tuple[list[tuple[str, str]], dict[str, str]]:
    layout: list[tuple[str, str]] = []
    type_by_equipment: dict[str, str] = {}
    for equipment in sorted(action_space_config):
        fields = sorted(str(field) for field in action_space_config[equipment])
        if not fields:
            raise ValueError(f"action_space_config[{equipment!r}] must not be empty")
        sig_type = _infer_signal_type(fields)
        if sig_type == "ThermalSetpoint" and not {field.lower() for field in fields} & _SETPOINT_FIELDS:
            raise ValueError(
                f"action_space_config[{equipment!r}] has deadband_c but no setpoint field: "
                "a ThermalSetpoint deadband applies only with a named setpoint"
            )
        type_by_equipment[equipment] = sig_type
        for field in fields:
            layout.append((equipment, field))
    return layout, type_by_equipment


_SETPOINT_FIELDS = {"heating_setpoint_c", "cooling_setpoint_c", "heat_c", "cool_c", "setpoint_c"}


def _infer_signal_type(fields: Sequence[str]) -> str:
    normalized = {field.lower() for field in fields}
    if normalized & (_SETPOINT_FIELDS | {"deadband_c"}):
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


# The Python-side action-field bounds table: the same fields with the same
# bounds as the engine's exposed table, pinned in both directions by
# test_python_action_bounds_match_the_engine.
_ACTION_FIELD_BOUNDS: dict[str, tuple[float, float]] = {
    "connected": (0.0, 1.0),
    "cool_c": (-50.0, 80.0),
    "cooling_setpoint_c": (-50.0, 80.0),
    "duty_cycle": (0.0, 1.0),
    "enabled": (0.0, 1.0),
    "fraction": (0.0, 1.0),
    "heat_c": (-50.0, 80.0),
    "heating_setpoint_c": (-50.0, 80.0),
    "load_fraction": (0.0, 1.0),
    "max_rh": (0.0, 1.0),
    "max_soc": (0.0, 1.0),
    "min_rh": (0.0, 1.0),
    "min_soc": (0.0, 1.0),
    "on_fraction": (0.0, 1.0),
    "setpoint_c": (-50.0, 80.0),
    "soc": (0.0, 1.0),
    "solar_only_charging": (0.0, 1.0),
    "target_rh": (0.0, 1.0),
    "target_soc": (0.0, 1.0),
}


def field_bounds(field: str) -> tuple[float, float]:
    name = field.lower()
    if name in _ACTION_FIELD_BOUNDS:
        return _ACTION_FIELD_BOUNDS[name]
    return (_BROAD_LOW, _BROAD_HIGH)


def action_bounds(dwelling: PyDwelling, equipment: str, field: str) -> tuple[float, float]:
    """A deadband is held to the range of the target's thermostat class."""
    if field.lower() == "deadband_c":
        band = dwelling.thermostat_band_range(equipment)
        if band is None:
            raise ValueError(
                f"equipment {equipment!r} has no thermostat band for a deadband_c action"
            )
        return band
    return field_bounds(field)


def observation_field_bounds(field: str) -> tuple[float, float]:
    key = field.strip().lower()
    if key in {"outdoor_temp", "outdoor_temp_c"}:
        return (-50.0, 55.0)
    if key == "outdoor_rh":
        return (0.0, 1.0)
    if key == "outdoor_humidity_ratio":
        return (0.0, 0.1)
    if key == "total_power_w":
        return (-100_000.0, 100_000.0)
    if key == "reactive_power_var":
        return (-100_000.0, 100_000.0)
    if _bracket_name(key, "zone_temp") is not None:
        return (0.0, 50.0)
    if _bracket_name(key, "setpoint_heat") is not None:
        return (0.0, 50.0)
    if _bracket_name(key, "setpoint_cool") is not None:
        return (0.0, 50.0)
    if _bracket_name(key, "zone_energy_balance") is not None:
        return (_BROAD_LOW, _BROAD_HIGH)
    if _bracket_name(key, "equipment_soc") is not None:
        return (0.0, 1.0)
    if _bracket_name(key, "equipment_power") is not None:
        return (0.0, 100_000.0)
    if key in {"battery_soc", "ev_soc"}:
        return (0.0, 1.0)
    if key in {"time_sin", "time_cos"}:
        return (-1.0, 1.0)
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
    initialized = getattr(telemetry, "initialized", True)
    if not initialized:
        out_arr = np.full(len(observation_fields), np.nan, dtype=np.float64)
        return np.ascontiguousarray(out_arr)

    zone = telemetry.zone()
    equipment = telemetry.equipment()
    zone_idx = _index_map(list(zone.get("names", [])))
    equip_idx = _index_map(list(equipment.get("names", [])))

    out: list[float] = []
    for field in observation_fields:
        key = str(field)
        if key in {"outdoor_temp", "outdoor_temp_c"}:
            out.append(float(zone.get("outdoor_temp_c", float("nan"))))
            continue
        if key in {"outdoor_humidity_ratio"}:
            out.append(float(zone.get("outdoor_humidity_ratio", float("nan"))))
            continue
        if key == "total_power_w":
            out.append(float(telemetry.total_power_w))
            continue
        if key == "reactive_power_var":
            out.append(float(telemetry.reactive_power_var))
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
        zone_name = _bracket_name(key, "zone_energy_balance")
        if zone_name is not None:
            out.append(float(zone["energy_balance_residuals"][zone_idx[zone_name.strip().lower()]]))
            continue
        equip_name = _bracket_name(key, "equipment_soc")
        if equip_name is not None:
            out.append(float(equipment["soc"][equip_idx[equip_name.strip().lower()]]))
            continue
        equip_name = _bracket_name(key, "equipment_power")
        if equip_name is not None:
            out.append(float(equipment["power_w"][equip_idx[equip_name.strip().lower()]]))
            continue

        if key == "battery_soc":
            out.append(float(equipment["soc"][equip_idx["battery"]]))
            continue
        if key == "ev_soc":
            out.append(float(equipment["soc"][equip_idx["electric vehicle"]]))
            continue

        # Temporal encoding: sin/cos of fractional hour for diurnal RL patterns.
        if key == "time_sin":
            dt = telemetry.current_time
            fhour = float(dt.hour) + float(dt.minute) / 60.0 + float(dt.second) / 3600.0
            out.append(math.sin(2.0 * math.pi * fhour / 24.0))
            continue
        if key == "time_cos":
            dt = telemetry.current_time
            fhour = float(dt.hour) + float(dt.minute) / 60.0 + float(dt.second) / 3600.0
            out.append(math.cos(2.0 * math.pi * fhour / 24.0))
            continue

        # Actor telemetry fallback: dot-separated <actor>.<channel>.
        found_in_actors = False
        if "." in key:
            actor_name, channel_name = key.split(".", 1)
            actors = telemetry.actors()
            if actor_name in actors and channel_name in actors[actor_name]:
                out.append(float(actors[actor_name][channel_name]))
                found_in_actors = True
        # Bare field name: search all actors (non-deterministic on duplicates).
        if not found_in_actors:
            actors = telemetry.actors()
            for actor_channels in actors.values():
                if key in actor_channels:
                    out.append(float(actor_channels[key]))
                    found_in_actors = True
                    break
        if found_in_actors:
            continue

        raise KeyError(f"unknown observation field `{key}`")

    obs = np.asarray(out, dtype=np.float64)
    return np.ascontiguousarray(obs)


def build_control_signal(signal_type: str, values: Mapping[str, float]) -> Any:
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


def drain_step_warnings(dwelling: Any) -> tuple[float, list[str]]:
    """Drain warnings accumulated on ``dwelling`` during the step just taken.

    Returns ``(warning_count, messages)`` matching the Rust ``batch_step``
    info contract: the count is a float published under
    ``info["warning_count"]`` on every step, while the messages go under
    ``info["warnings"]`` only when the count is non-zero.
    ``Dwelling.take_warnings()`` drains, so each warning is surfaced exactly
    once.
    """
    messages = [str(message) for message in dwelling.take_warnings()]
    return float(len(messages)), messages


# ---------------------------------------------------------------------------
# Gymnasium environment
# ---------------------------------------------------------------------------


class DwellingGymEnv(_GYM_BASE):
    def __init__(
        self,
        config: GymDwellingConfig,
        observation_fields: Sequence[str],
        action_space_config: Mapping[str, Sequence[str]],
        reward_fn: Callable[[RewardContext], float] | None,
        episode_length: timedelta,
        field_bounds_overrides: Mapping[str, tuple[float, float]] | None = None,
    ) -> None:
        if gym is None or spaces is None:
            raise ImportError("gymnasium is required for RL environments")

        self._config = _normalize_config(config)
        self._observation_fields = validated_observation_fields(observation_fields)
        validate_reward_fn(reward_fn)
        validate_episode_length(episode_length)
        self._reward_fn = reward_fn
        self._episode_length = episode_length
        self._action_layout, self._signal_type_by_equipment = sorted_action_layout(
            action_space_config
        )

        hpxml_kwargs: dict[str, Any] = {}
        if self._config.defaults is not None:
            hpxml_kwargs["defaults"] = self._config.defaults
        if self._config.sim_config is not None:
            # The whole object, so every field reaches the dwelling by
            # construction and the engine validates it.
            hpxml_kwargs["config"] = self._config.sim_config

        self._dwelling = PyDwelling.from_hpxml(
            hpxml=self._config.hpxml,
            schedule=self._config.schedule,
            weather=self._config.weather,
            **hpxml_kwargs,
        )
        self._dwelling.initialize()
        self._initial_snapshot = bytes(self._dwelling.save_state())
        # The episode's finished run result, `None` until a truncating step
        # finalises the run through ``finish()``.
        self._run_result: Any = None
        # The frozen observation plan, built at reset() against the built
        # dwelling (lazily here for a step before any reset): every step
        # and reset observation is read through it.
        self._plan: Any = None

        bounds = [
            observation_field_bounds(f) for f in self._observation_fields
        ]
        if field_bounds_overrides:
            for idx, field in enumerate(self._observation_fields):
                override = field_bounds_overrides.get(field)
                if override is not None:
                    bounds[idx] = (float(override[0]), float(override[1]))
        self._observation_bounds = {
            field: (float(low), float(high))
            for field, (low, high) in zip(self._observation_fields, bounds)
        }
        obs_low = np.asarray([b[0] for b in bounds], dtype=np.float64)
        obs_high = np.asarray([b[1] for b in bounds], dtype=np.float64)
        self.observation_space = spaces.Box(
            low=obs_low,
            high=obs_high,
            shape=(len(self._observation_fields),),
            dtype=np.float64,
        )
        if not np.all(np.isfinite(obs_low)) or not np.all(np.isfinite(obs_high)):
            warnings.warn(
                "Observation space contains infinite bounds for fields: "
                + ", ".join(sorted(set(
                    f for f, (low, high) in self._observation_bounds.items()
                    if not (np.isfinite(low) and np.isfinite(high))
                ))),
                stacklevel=2,
            )

        action_low: list[float] = []
        action_high: list[float] = []
        for equipment, field in self._action_layout:
            low, high = action_bounds(self._dwelling, equipment, field)
            action_low.append(low)
            action_high.append(high)
        # Kept on the instance so _apply_action() does not reach through the
        # optional Gymnasium space object.
        self._action_low_arr = np.asarray(action_low, dtype=np.float64)
        self._action_high_arr = np.asarray(action_high, dtype=np.float64)
        self.action_space = spaces.Box(
            low=self._action_low_arr,
            high=self._action_high_arr,
            shape=(len(self._action_layout),),
            dtype=np.float64,
        )

        time_res_s: float = 60.0
        if self._config.sim_config is not None:
            time_res_s = float(self._config.sim_config.time_res)
        # The episode covers at least one step: the constructor refused a
        # non-positive episode_length, so the ceiling is never below one.
        self._max_steps = int(math.ceil(self._episode_length.total_seconds() / time_res_s))
        self._steps_elapsed: int = 0
        self._active_seed: int | None = None

    @property
    def dwelling(self) -> PyDwelling:
        """The environment's underlying dwelling: its live ``health()``
        reads at any step, its ``finish()`` result at the episode's end."""
        return self._dwelling

    @property
    def run_result(self) -> Any:
        """The finished run's ``RunResult``, once an episode has run to its
        truncation (``None`` before that, cleared on ``reset()``)."""
        return self._run_result

    def _plan_or_build(self) -> Any:
        """The frozen observation plan, built once against the built
        dwelling; an unknown field fails at the build, naming it."""
        if self._plan is None:
            self._plan = self._dwelling.observation_plan(self._observation_fields)
        return self._plan

    def _observation(self) -> np.ndarray:
        """The observation row, read through the frozen plan into a fresh
        array: the same values the batch path reads, on bits."""
        row = np.empty(len(self._observation_fields), dtype=np.float64)
        self._dwelling.observe_into(self._plan_or_build(), row)
        return row

    def _apply_action(self, action: np.ndarray) -> None:
        # The extension clamps each value to the declared bounds and counts
        # the clamped values on the dwelling's run health, in every build:
        # the same record the batch path's steps carry.
        values = clip_action(
            self._dwelling,
            [float(value) for value in action],
            self._action_layout,
        )
        action_values: dict[str, dict[str, float]] = {}
        for (equipment, field), value in zip(self._action_layout, values, strict=True):
            action_values.setdefault(equipment, {})[field] = value

        for equipment in sorted(action_values):
            signal = build_control_signal(
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
        # The reset restores the warmed-up state and reseeds the streams,
        # without rebuilding: the snapshot's converged initial condition is
        # kept, the episode's random streams are the seed's, and no
        # warm-up runs again. The explicit full rebuild is
        # `reset_with_seed`, which this path does not call.
        self._dwelling.reseed(applied_seed)
        self._steps_elapsed = 0
        self._active_seed = applied_seed
        self._run_result = None

        # The plans are frozen at reset: rebuilt against the restored
        # dwelling, so an unknown field fails here, naming it.
        self._plan = self._dwelling.observation_plan(self._observation_fields)
        obs = self._observation()
        obs_mask = ~np.isfinite(obs)
        obs_valid = np.all(np.isfinite(obs))

        global _warned_initial_nan
        if not obs_valid and not _warned_initial_nan:
            _warned_initial_nan = True
            warnings.warn(
                "Initial observation contains NaN for uninitialized fields "
                "— replace with 0.0 or mask before training.",
                stacklevel=2,
            )

        return obs, {
            "seed": applied_seed,
            "initial_observation_mask": obs_mask,
        }

    def step(
        self, action: np.ndarray
    ) -> tuple[np.ndarray, float, bool, bool, dict[str, Any]]:
        """Apply ``action``, advance one timestep, and return
        ``(obs, reward, terminated, truncated, info)``.

        ``info`` carries the same contract as the Rust ``batch_step`` fast
        path: ``"step"`` is the dwelling's ``StepResult`` for the step,
        ``"warning_count"`` (float) is always present and counts the warnings
        drained from the dwelling during the step (e.g. control signals
        rejected at dispatch time); ``"warnings"`` (list[str]) is present only
        when the count is non-zero and holds the drained messages. Warnings
        are drained exactly once per step, so a subsequent
        ``Dwelling.take_warnings()`` will not return them again.
        """
        arr = np.asarray(action, dtype=np.float64)
        arr = np.ascontiguousarray(arr.reshape(-1))
        if arr.shape != (len(self._action_layout),):
            raise ValueError(
                f"expected action shape {(len(self._action_layout),)}, got {arr.shape}"
            )

        self._apply_action(arr)
        step_data: StepResult = self._dwelling.step()
        telemetry = self._dwelling.telemetry()
        obs = self._observation()
        self._steps_elapsed += 1
        truncated = self._steps_elapsed >= self._max_steps

        # The episode's end finalises the run: the finished RunResult is
        # the episode's report surface (the health totals ride the
        # dwelling's live health()).
        if truncated and self._run_result is None:
            self._run_result = self._dwelling.finish()

        if self._reward_fn is None:
            # The batch path's default reward: the negative net electric
            # power of the step in kW, converted once from the step's
            # watts, so a trained policy's reward scale does not change.
            reward = -float(step_data["net_electric_power_w"]) / 1000.0
        else:
            reward_context = RewardContext(
                step=step_data,
                telemetry_zone=telemetry.zone(),
                telemetry_equipment=telemetry.equipment(),
                total_power_w=float(telemetry.total_power_w),
            )
            reward = float(self._reward_fn(reward_context))
        warning_count, warning_messages = drain_step_warnings(self._dwelling)
        info: dict[str, Any] = {
            "step": step_data,
            "seed": self._active_seed if self._active_seed is not None else 0,
            "timestep_index": self._steps_elapsed,
            "observation_bounds": self._observation_bounds,
            "initial_observation_mask": None,
            "warning_count": warning_count,
        }
        if warning_messages:
            info["warnings"] = warning_messages
        return obs, reward, False, truncated, info
