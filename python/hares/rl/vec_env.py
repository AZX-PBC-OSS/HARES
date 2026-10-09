"""Vectorized Gymnasium environment for fleet-level RL."""

from __future__ import annotations

from collections.abc import Callable, Mapping, Sequence
from datetime import timedelta
import logging
import math
import multiprocessing
import secrets
from typing import TYPE_CHECKING, Any
import warnings

import numpy as np

from hares._hares import Dwelling as PyDwelling
from hares._hares import batch_step as _extension_batch_step

try:
    from gymnasium import spaces
except ImportError:  # pragma: no cover - optional dependency
    spaces = None

from .gym_env import (
    RewardContext,
    action_bounds,
    build_control_signal,
    drain_step_warnings,
    field_bounds,
    observation_field_bounds,
    sorted_action_layout,
    telemetry_to_observation,
    validate_episode_length,
    validated_observation_fields,
)

if TYPE_CHECKING:
    from hares._hares_types import StepResult

__all__ = [
    "RewardContext",
    "VecDwellingGymEnv",
    "action_bounds",
    "build_control_signal",
    "drain_step_warnings",
    "field_bounds",
    "observation_field_bounds",
    "rust_batch_step",
    "sorted_action_layout",
    "telemetry_to_observation",
]

# The extension's batch entrypoint, held as an Optional module global so
# step() has two branches to serve and a caller can force the pure-Python
# one. The extension always registers it (a module without it fails to
# import), so this never reads None unless forced.
rust_batch_step: Callable[..., list[dict[str, Any]]] | None = _extension_batch_step

_warned_initial_nan: bool = False


def _reward_context(step_data: StepResult, dwelling: PyDwelling) -> RewardContext:
    """The ``RewardContext`` both step paths reward on: the step dict and the
    dwelling's post-step telemetry, built once so the batch path and a single
    dwelling's step hand ``reward_fn`` identical contexts."""
    telemetry = dwelling.telemetry()
    return RewardContext(
        step=step_data,
        telemetry_zone=telemetry.zone(),
        telemetry_equipment=telemetry.equipment(),
        total_power_kw=float(telemetry.total_power_kw),
    )


class VecDwellingGymEnv:
    def __init__(
        self,
        dwellings: Sequence[PyDwelling],
        observation_fields: Sequence[str],
        action_space_config: Mapping[str, Sequence[str]],
        reward_fn: Callable[[RewardContext], float],
        episode_length: timedelta,
        time_res_s: float | None = None,
        field_bounds_overrides: Mapping[str, tuple[float, float]] | None = None,
        verify_observation_equivalence: bool = False,
    ) -> None:
        self._observation_fields = validated_observation_fields(observation_fields)
        if not callable(reward_fn):
            raise TypeError(
                f"reward_fn must be callable, got {type(reward_fn).__name__}"
            )
        validate_episode_length(episode_length)

        start_method = multiprocessing.get_start_method(allow_none=True)
        if start_method == "fork":
            raise RuntimeError(
                "VecDwellingGymEnv does not support multiprocessing start method "
                "'fork' because fork + Rayon can deadlock. "
                "Use DummyVecEnv or a non-fork start method."
            )

        self._dwellings = list(dwellings)
        if not self._dwellings:
            raise ValueError("dwellings must not be empty")

        for dwelling in self._dwellings:
            dwelling.initialize()

        self._initial_snapshots = [bytes(dwelling.save_state()) for dwelling in self._dwellings]
        self.num_envs = len(self._dwellings)

        self._action_layout, self._signal_type_by_equipment = sorted_action_layout(
            action_space_config
        )
        action_low: list[float] = []
        action_high: list[float] = []
        for equipment, field in self._action_layout:
            per_dwelling = {
                action_bounds(dwelling, equipment, field) for dwelling in self._dwellings
            }
            if len(per_dwelling) != 1:
                raise ValueError(
                    f"{equipment}.{field} has different bounds across the dwellings: "
                    f"{sorted(per_dwelling)}; one action space cannot hold them"
                )
            ((low, high),) = per_dwelling
            action_low.append(low)
            action_high.append(high)
        self._action_bounds = list(zip(action_low, action_high))

        action_low_arr = np.asarray(action_low, dtype=np.float64)
        action_high_arr = np.asarray(action_high, dtype=np.float64)
        bounds = [
            observation_field_bounds(f) for f in self._observation_fields
        ]
        if field_bounds_overrides:
            for idx, field in enumerate(self._observation_fields):
                override = field_bounds_overrides.get(field)
                if override is not None:
                    bounds[idx] = (float(override[0]), float(override[1]))
        # The bounds are kept directly on the instance so step() needs no
        # gymnastics over the optional Gymnasium space
        # objects (absent when gymnasium is not installed).
        self._action_low = action_low_arr
        self._action_high = action_high_arr
        self._action_dim = len(self._action_layout)
        if spaces is not None:
            obs_low = np.asarray([b[0] for b in bounds], dtype=np.float64)
            obs_high = np.asarray([b[1] for b in bounds], dtype=np.float64)
            self.observation_space = spaces.Box(
                low=obs_low,
                high=obs_high,
                shape=(len(self._observation_fields),),
                dtype=np.float64,
            )
            self.action_space = spaces.Box(
                low=action_low_arr,
                high=action_high_arr,
                shape=(len(self._action_layout),),
                dtype=np.float64,
            )
        else:
            self.observation_space = {
                "shape": (len(self._observation_fields),),
                "dtype": np.float64,
            }
            self.action_space = {
                "shape": (len(self._action_layout),),
                "dtype": np.float64,
                "low": action_low_arr,
                "high": action_high_arr,
            }
        self._observation_bounds: dict[str, tuple[float, float]] = {
            field: (float(low), float(high))
            for field, (low, high) in zip(self._observation_fields, bounds)
        }
        self._reward_fn = reward_fn
        self._verify_observation_equivalence = verify_observation_equivalence
        self._steps_elapsed = np.zeros(self.num_envs, dtype=np.int64)

        _time_res: float
        _fallback: bool = False
        if time_res_s is not None:
            if time_res_s <= 0:
                raise ValueError(
                    f"time_res_s must be positive, got {time_res_s}"
                )
            _time_res = time_res_s
        else:
            try:
                _cfg_time_res = self._dwellings[0].config().time_res
                if _cfg_time_res and _cfg_time_res > 0:
                    _time_res = float(_cfg_time_res)
                else:
                    _time_res = 60.0
                    _fallback = True
            except Exception:
                _time_res = 60.0
                _fallback = True

        if _fallback:
            warnings.warn(
                "Cannot determine time_res_s from dwelling config; falling back to 60.0 s. "
                "Pass time_res_s explicitly to VecDwellingGymEnv or ensure the first "
                "dwelling has a valid sim config.",
                stacklevel=2,
            )

        self._max_steps = int(math.ceil(episode_length.total_seconds() / _time_res))
        self._time_res_s = _time_res

        _logger = logging.getLogger(__name__)
        _logger.info(
            "VecDwellingGymEnv constructed: _max_steps=%d, time_res_s=%.1f, num_envs=%d",
            self._max_steps,
            _time_res,
            self.num_envs,
        )

    def reset(self, *, seed: int | None = None, options: dict[str, Any] | None = None):
        del options
        seeds: list[int] = []
        for idx, dwelling in enumerate(self._dwellings):
            dwelling.load_state(self._initial_snapshots[idx])
            applied_seed = (int(seed) + idx) if seed is not None else secrets.randbits(63)
            dwelling.reset_with_seed(int(applied_seed))
            seeds.append(int(applied_seed))
        self._steps_elapsed[:] = 0

        obs = np.vstack(
            [telemetry_to_observation(d.telemetry(), self._observation_fields) for d in self._dwellings]
        )

        row_has_nan = ~np.all(np.isfinite(obs), axis=1)
        global _warned_initial_nan
        if np.any(row_has_nan) and not _warned_initial_nan:
            _warned_initial_nan = True
            warnings.warn(
                "Initial observation contains NaN for uninitialized fields "
                "— replace with 0.0 or mask before training.",
                stacklevel=2,
            )

        infos: list[dict[str, Any]] = [{"seed": seeds[idx]} for idx in range(self.num_envs)]
        for idx in range(self.num_envs):
            infos[idx]["initial_observation_mask"] = ~np.isfinite(obs[idx])
        return obs.astype(np.float64, copy=False), infos

    def _apply_actions(self, dwelling: PyDwelling, action: np.ndarray) -> None:
        """Clip one dwelling's action row to the layout's per-column bounds
        and apply it as control signals, equipment by sorted name: the same
        clip-and-dispatch the extension's ``batch_step`` runs."""
        clipped = np.clip(action, self._action_low, self._action_high)
        action_values: dict[str, dict[str, float]] = {}
        for col, (equipment, field) in enumerate(self._action_layout):
            action_values.setdefault(equipment, {})[field] = float(clipped[col])
        for equipment in sorted(action_values):
            dwelling.apply_control(
                equipment,
                build_control_signal(
                    self._signal_type_by_equipment[equipment],
                    action_values[equipment],
                ),
            )

    def step(self, actions: np.ndarray):
        """Advance every dwelling one timestep.

        Each entry of ``infos`` carries the same shape on both step paths:
        ``"step"`` (the step dict the reward reads, ``None`` when that
        dwelling's step failed), ``"warning_count"`` (float, always
        present) and ``"warnings"`` (list[str], present only when the count
        is non-zero, the warnings drained from that dwelling during the
        step, e.g. control signals rejected at dispatch time). Both step
        paths clip actions to the declared bounds and call ``reward_fn``
        per environment on the same ``RewardContext`` built from the step
        dict and the dwelling's telemetry: one reward contract.
        """
        arr = np.asarray(actions, dtype=np.float64)
        arr = np.ascontiguousarray(arr)
        expected = (self.num_envs, self._action_dim)
        if arr.shape != expected:
            raise ValueError(f"expected actions shape {expected}, got {arr.shape}")

        obs_rows: list[Any] = []
        rewards: list[float] = []
        dones: list[bool] = []
        truncs: list[bool] = []
        infos: list[dict[str, Any]] = []
        if rust_batch_step is not None:
            actions_list = arr.tolist()
            raw = rust_batch_step(
                self._dwellings,
                actions_list,
                self._observation_fields,
                self._action_layout,
                self._signal_type_by_equipment,
                self._action_bounds,
            )
            for dwelling, row in zip(self._dwellings, raw, strict=True):
                step_data = row["step"]
                if step_data is None:
                    # A failed step terminates the episode and carries no
                    # step dict to build a context from; its reward stays
                    # the 0.0 a failed step has always reported.
                    rewards.append(0.0)
                else:
                    rewards.append(
                        float(self._reward_fn(_reward_context(step_data, dwelling)))
                    )
                obs_rows.append(row["obs"])
                dones.append(bool(row["terminated"]))
                truncs.append(bool(row["truncated"]))
                info = dict(row.get("info", {}))
                info["step"] = step_data
                infos.append(info)
        else:
            for dwelling, action_row in zip(self._dwellings, arr, strict=True):
                self._apply_actions(dwelling, action_row)
                step_data = dwelling.step()
                telemetry = dwelling.telemetry()
                obs_rows.append(
                    telemetry_to_observation(telemetry, self._observation_fields)
                )
                rewards.append(float(self._reward_fn(_reward_context(step_data, dwelling))))
                dones.append(False)
                truncs.append(False)
                warning_count, warning_messages = drain_step_warnings(dwelling)
                info: dict[str, Any] = {"step": step_data, "warning_count": warning_count}
                if warning_messages:
                    info["warnings"] = warning_messages
                infos.append(info)

        obs = np.asarray(obs_rows, dtype=np.float64)
        reward_arr = np.asarray(rewards, dtype=np.float64)
        done_arr = np.asarray(dones, dtype=np.bool_)
        trunc_arr = np.asarray(truncs, dtype=np.bool_)

        if self._verify_observation_equivalence:
            for i, dwelling in enumerate(self._dwellings):
                py_obs = telemetry_to_observation(dwelling.telemetry(), self._observation_fields)
                py_obs = np.asarray(py_obs, dtype=np.float64)
                if not np.allclose(obs[i], py_obs, atol=1e-9, rtol=1e-5, equal_nan=True):
                    raw_diff = np.abs(obs[i] - py_obs)
                    nan_mismatch = np.isnan(obs[i]) != np.isnan(py_obs)
                    diff = np.where(nan_mismatch, np.inf, raw_diff)
                    max_idx = int(np.argmax(diff))
                    max_diff = float(diff[max_idx])
                    max_diff_str = (
                        "NaN mismatch" if np.isinf(max_diff) else f"{max_diff:.9g}"
                    )
                    warnings.warn(
                        f"Rust-Python observation mismatch for dwelling {i} at field "
                        f"{self._observation_fields[max_idx]!r}: "
                        f"Rust={obs[i][max_idx]:.9g}, Python={py_obs[max_idx]:.9g}, "
                        f"max absolute diff={max_diff_str}",
                        stacklevel=2,
                    )

        for info in infos:
            for field, (low, high) in self._observation_bounds.items():
                info[f"obs_{field}_low"] = float(low)
                info[f"obs_{field}_high"] = float(high)

        self._steps_elapsed += 1
        auto_trunc = self._steps_elapsed >= self._max_steps
        trunc_arr = np.logical_or(trunc_arr, auto_trunc)
        for idx, trunc in enumerate(auto_trunc.tolist()):
            if trunc:
                infos[idx]["truncated"] = True

        return obs, reward_arr, done_arr, trunc_arr, infos

    @property
    def unwrapped(self) -> "VecDwellingGymEnv":
        return self

    def close(self) -> None:
        return None

    def env_method(
        self,
        method_name: str,
        *args: Any,
        indices: Sequence[int] | None = None,
        **kwargs: Any,
    ) -> list[Any]:
        if indices is None:
            indices = range(self.num_envs)
        out: list[Any] = []
        for idx in indices:
            out.append(getattr(self._dwellings[idx], method_name)(*args, **kwargs))
        return out
