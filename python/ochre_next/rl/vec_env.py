"""Vectorized Gymnasium environment for fleet-level RL."""

from __future__ import annotations

from collections.abc import Callable, Mapping, Sequence
from datetime import timedelta
import multiprocessing
import secrets
from typing import Any

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
    _sorted_action_layout,
    telemetry_to_observation,
)


class VecDwellingGymEnv:
    def __init__(
        self,
        dwellings: Sequence[PyDwelling],
        observation_fields: Sequence[str],
        action_space_config: Mapping[str, Sequence[str]],
        reward_fn: Callable[[RewardContext], float],
        episode_length: timedelta,
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
            self.observation_space = spaces.Box(
                low=np.full(len(observation_fields), -np.inf, dtype=np.float64),
                high=np.full(len(observation_fields), np.inf, dtype=np.float64),
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
        self._reward_fn = reward_fn
        self._steps_elapsed = np.zeros(self.num_envs, dtype=np.int64)
        self._max_steps = max(1, int(np.ceil(episode_length.total_seconds() / 60.0)))

    def _apply_controls(self, dwelling: PyDwelling, action_row: np.ndarray) -> None:
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
        infos = [{"seed": seeds[idx]} for idx in range(self.num_envs)]
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

        for idx, dwelling in enumerate(self._dwellings):
            self._apply_controls(dwelling, arr[idx])

        if rust_batch_step is not None:
            # Controls are already applied via _apply_controls above.
            # Pass empty action lists — Rust-side action mapping (H-7) is not yet implemented.
            empty_actions: list[list[float]] = [[] for _ in self._dwellings]
            raw = rust_batch_step(self._dwellings, empty_actions, self._observation_fields)
            obs = np.asarray([row["obs"] for row in raw], dtype=np.float64)
            rewards = np.asarray([float(row["reward"]) for row in raw], dtype=np.float64)
            dones = np.asarray([bool(row["terminated"]) for row in raw], dtype=np.bool_)
            truncs = np.asarray([bool(row["truncated"]) for row in raw], dtype=np.bool_)
            infos = [dict(row.get("info", {})) for row in raw]
        else:
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
                    total_power_kw=float(telemetry.total_power_kw()),
                )
                rewards.append(float(self._reward_fn(ctx)))
                dones.append(False)
                truncs.append(False)
                infos.append({"step": step_data})
            obs = np.asarray(obs_rows, dtype=np.float64)
            rewards = np.asarray(rewards, dtype=np.float64)
            dones = np.asarray(dones, dtype=np.bool_)
            truncs = np.asarray(truncs, dtype=np.bool_)

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
