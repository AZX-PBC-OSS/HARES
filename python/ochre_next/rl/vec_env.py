"""Vectorized Gymnasium environment for fleet-level RL."""

from __future__ import annotations

from collections.abc import Callable, Mapping, Sequence
from datetime import timedelta
import multiprocessing
import secrets
from typing import Any
import warnings

import numpy as np

from ochre_next._hares import Dwelling as PyDwelling

try:
    from ochre_next._hares import batch_step as rust_batch_step
except ImportError:  # pragma: no cover - only when extension lacks RL entrypoint
    rust_batch_step = None

try:
    from gymnasium import spaces
except ImportError:  # pragma: no cover - optional dependency
    spaces = None

from .gym_env import (
    RewardContext,
    _build_control_signal,
    _field_bounds,
    _observation_field_bounds,
    _sorted_action_layout,
    telemetry_to_observation,
)

_warned_initial_nan: bool = False


class VecDwellingGymEnv:
    def __init__(
        self,
        dwellings: Sequence[PyDwelling],
        observation_fields: Sequence[str],
        action_space_config: Mapping[str, Sequence[str]],
        reward_fn: Callable[[RewardContext], float],
        episode_length: timedelta,
        field_bounds_overrides: Mapping[str, tuple[float, float]] | None = None,
        verify_observation_equivalence: bool = False,
    ) -> None:
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

        self._action_layout, self._signal_type_by_equipment = _sorted_action_layout(
            action_space_config
        )
        action_low = []
        action_high = []
        for _, field in self._action_layout:
            low, high = _field_bounds(field)
            action_low.append(low)
            action_high.append(high)

        action_low_arr = np.asarray(action_low, dtype=np.float64)
        action_high_arr = np.asarray(action_high, dtype=np.float64)
        if spaces is not None:
            bounds = [
                _observation_field_bounds(f) for f in observation_fields
            ]
            if field_bounds_overrides:
                for idx, field in enumerate(observation_fields):
                    override = field_bounds_overrides.get(field)
                    if override is not None:
                        bounds[idx] = (float(override[0]), float(override[1]))
            obs_low = np.asarray([b[0] for b in bounds], dtype=np.float64)
            obs_high = np.asarray([b[1] for b in bounds], dtype=np.float64)
            self.observation_space = spaces.Box(
                low=obs_low,
                high=obs_high,
                shape=(len(observation_fields),),
                dtype=np.float64,
            )
            self.action_space = spaces.Box(
                low=action_low_arr,
                high=action_high_arr,
                shape=(len(self._action_layout),),
                dtype=np.float64,
            )
        else:
            self.observation_space = {"shape": (len(observation_fields),), "dtype": np.float64}
            self.action_space = {
                "shape": (len(self._action_layout),),
                "dtype": np.float64,
                "low": action_low_arr,
                "high": action_high_arr,
            }
        self._observation_fields = list(observation_fields)
        self._observation_bounds: dict[str, tuple[float, float]] = {
            field: (float(low), float(high))
            for field, (low, high) in zip(observation_fields, bounds)
        }
        self._reward_fn = reward_fn
        self._verify_observation_equivalence = verify_observation_equivalence
        self._steps_elapsed = np.zeros(self.num_envs, dtype=np.int64)
        self._max_steps = max(1, int(np.ceil(episode_length.total_seconds() / 60.0)))

    def _apply_controls(self, dwelling: PyDwelling, action_row: np.ndarray) -> None:
        low = self.action_space.low if hasattr(self.action_space, "low") else self.action_space["low"]
        high = self.action_space.high if hasattr(self.action_space, "high") else self.action_space["high"]
        action_row = np.clip(action_row, low, high)
        action_values: dict[str, dict[str, float]] = {}
        for idx, (equipment, field) in enumerate(self._action_layout):
            action_values.setdefault(equipment, {})[field] = float(action_row[idx])
        for equipment in sorted(action_values):
            signal = _build_control_signal(
                self._signal_type_by_equipment[equipment],
                action_values[equipment],
            )
            dwelling.apply_control(equipment, signal)

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

        infos = [{"seed": seeds[idx]} for idx in range(self.num_envs)]
        for idx in range(self.num_envs):
            infos[idx]["initial_observation_mask"] = ~np.isfinite(obs[idx])
        return obs.astype(np.float64, copy=False), infos

    def step(self, actions: np.ndarray):
        arr = np.asarray(actions, dtype=np.float64)
        arr = np.ascontiguousarray(arr)
        action_dim = (
            self.action_space.shape[0]
            if hasattr(self.action_space, "shape")
            else self.action_space["shape"][0]
        )
        expected = (self.num_envs, action_dim)
        if arr.shape != expected:
            raise ValueError(f"expected actions shape {expected}, got {arr.shape}")

        if rust_batch_step is not None:
            actions_list = arr.tolist()
            raw = rust_batch_step(
                self._dwellings,
                actions_list,
                self._observation_fields,
                self._action_layout,
                self._signal_type_by_equipment,
            )
            obs = np.asarray([row["obs"] for row in raw], dtype=np.float64)
            rewards = np.asarray([float(row["reward"]) for row in raw], dtype=np.float64)
            dones = np.asarray([bool(row["terminated"]) for row in raw], dtype=np.bool_)
            truncs = np.asarray([bool(row["truncated"]) for row in raw], dtype=np.bool_)
            infos = [dict(row.get("info", {})) for row in raw]

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
        else:
            for idx, dwelling in enumerate(self._dwellings):
                self._apply_controls(dwelling, arr[idx])
            obs_rows = []
            rewards = []
            dones = []
            truncs = []
            infos = []
            for dwelling in self._dwellings:
                step_data = dwelling.step()
                telemetry = dwelling.telemetry()
                obs_rows.append(telemetry_to_observation(telemetry, self._observation_fields))
                ctx = RewardContext(
                    step=step_data,
                    telemetry_zone=telemetry.zone(),
                    telemetry_equipment=telemetry.equipment(),
                    total_power_kw=float(telemetry.total_power_kw),
                )
                rewards.append(float(self._reward_fn(ctx)))
                dones.append(False)
                truncs.append(False)
                infos.append({"step": step_data})
            obs = np.asarray(obs_rows, dtype=np.float64)
            rewards = np.asarray(rewards, dtype=np.float64)
            dones = np.asarray(dones, dtype=np.bool_)
            truncs = np.asarray(truncs, dtype=np.bool_)

        for info in infos:
            for field, (low, high) in self._observation_bounds.items():
                info[f"obs_{field}_low"] = float(low)
                info[f"obs_{field}_high"] = float(high)

        self._steps_elapsed += 1
        auto_trunc = self._steps_elapsed >= self._max_steps
        truncs = np.logical_or(truncs, auto_trunc)
        for idx, trunc in enumerate(auto_trunc.tolist()):
            if trunc:
                infos[idx]["truncated"] = True

        return obs, rewards, dones, truncs, infos

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
        out = []
        for idx in indices:
            out.append(getattr(self._dwellings[idx], method_name)(*args, **kwargs))
        return out
