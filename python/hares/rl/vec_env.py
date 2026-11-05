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
    from gymnasium.vector import VectorEnv as _VectorEnvBaseRuntime
except ImportError:  # pragma: no cover - optional dependency
    spaces = None
    _VectorEnvBaseRuntime = object  # type: ignore[assignment, misc]

if TYPE_CHECKING:
    # The checker sees the real base class, parameterized by the env's
    # types (the observations, rewards and actions are float64 ndarrays);
    # at runtime the guarded import above decides.
    from gymnasium.vector import VectorEnv as _GymVectorEnv

    _VectorEnvBase = _GymVectorEnv[np.ndarray, np.ndarray, np.ndarray]
else:
    _VectorEnvBase = _VectorEnvBaseRuntime  # type: ignore[assignment, misc]

# The Gymnasium presence: the base class is the real VectorEnv only when
# Gymnasium is installed (the fallback keeps the importable surface).
_HAS_VECTOR_BASE = _VectorEnvBaseRuntime is not object

from .gym_env import (
    RewardContext,
    action_bounds,
    build_control_signal,
    drain_step_warnings,
    field_bounds,
    observation_field_bounds,
    sorted_action_layout,
    validate_episode_length,
    validated_auto_reset,
    validated_observation_fields,
    validated_reset_seed,
    validated_seed_sequence,
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
        total_power_w=float(telemetry.total_power_w),
    )


class VecDwellingGymEnv(_VectorEnvBase):
    """The fleet's vector environment, a Gymnasium `VectorEnv` when
    Gymnasium is installed: the standard vector wrappers accept it, the
    per-env spaces ride `single_observation_space` /
    `single_action_space` (the fleet steps one shared space), and the
    base's `close` bookkeeping drives `close_extras`."""
    def __init__(
        self,
        dwellings: Sequence[PyDwelling],
        observation_fields: Sequence[str],
        action_space_config: Mapping[str, Sequence[str]],
        reward_fn: Callable[[RewardContext], float],
        episode_length: timedelta,
        time_res_s: float | None = None,
        field_bounds_overrides: Mapping[str, tuple[float, float]] | None = None,
        copy: bool = True,
        auto_reset: object = False,
        seed_sequence: object = None,
    ) -> None:
        self._observation_fields = validated_observation_fields(observation_fields)
        if not callable(reward_fn):
            raise TypeError(
                f"reward_fn must be callable, got {type(reward_fn).__name__}"
            )
        validate_episode_length(episode_length)
        self._auto_reset = validated_auto_reset(auto_reset)
        seed_sequence = validated_seed_sequence(seed_sequence)

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
        # The two lifecycle arguments: `auto_reset` resets an env whose
        # episode ended alone at the step's end (see step()), and
        # `seed_sequence` is the caller SeedSequence the per-env episode
        # seeds derive from -- one child per env, each of the env's
        # episodes drawing its seed from its child's next spawn, so the
        # seeds are deterministic in (caller SeedSequence, env index,
        # episode count) and independent across envs. `None` draws each
        # episode's seed from OS entropy instead, like reset(seed=None).
        self._seed_children: list[np.random.SeedSequence] | None = (
            list(seed_sequence.spawn(self.num_envs))
            if seed_sequence is not None
            else None
        )
        # Each env's current episode's applied seed, reported on every
        # info row: an unmasked env keeps its running episode's seed, so
        # the report is the seed the env's steps actually run under.
        self._applied_seeds: list[int | None] = [None] * self.num_envs

        # The frozen observation plans, one per dwelling, built at reset()
        # against the built dwelling (lazily here for a step before any
        # reset). Every step reads its observation through the plan into
        # the buffer below, in place: no per-row lists and no Python
        # object per field.
        self._plans: list[Any] | None = None
        # The batch's observation buffer, owned by this environment: the
        # extension fills each environment's row in place through its
        # buffer protocol.
        self._obs_buffer = np.empty(
            (self.num_envs, len(self._observation_fields)), dtype=np.float64
        )
        # step() returns a copy of the buffer by default, so an observation
        # a caller keeps across steps is never overwritten; copy=False
        # returns the buffer itself, overwritten by the next step() or
        # reset(), for callers that consume each observation before
        # stepping again (Gymnasium's SyncVectorEnv(copy=...) has the same
        # contract).
        self._copy = copy
        # The Gymnasium VectorEnv's bookkeeping flag: `close()` sets it
        # once and a second close reads it short.
        self.closed = False

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
            # The per-env spaces: the fleet steps one shared observation
            # and action space (the constructor refuses bounds that differ
            # across the dwellings), so the single env's spaces are these.
            self.single_observation_space = self.observation_space
            self.single_action_space = self.action_space
        else:
            # The no-Gymnasium surface keeps its dict-shaped stand-ins
            # (the base class's space attributes do not exist to type
            # against).
            self.observation_space = {  # type: ignore[reportAttributeAccessIssue]
                "shape": (len(self._observation_fields),),
                "dtype": np.float64,
            }
            self.action_space = {  # type: ignore[reportAttributeAccessIssue]
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
        # Each dwelling's finished run result, `None` until the episode's
        # truncating step finalises the run through ``finish()``.
        self._run_results: list[Any] = [None] * self.num_envs

        _logger = logging.getLogger(__name__)
        _logger.info(
            "VecDwellingGymEnv constructed: _max_steps=%d, time_res_s=%.1f, num_envs=%d",
            self._max_steps,
            _time_res,
            self.num_envs,
        )

    def _build_plans(self) -> None:
        """Freezes one observation plan per dwelling, built at reset against
        the built dwelling. An unknown field fails here, naming it."""
        self._plans = [
            dwelling.observation_plan(self._observation_fields)
            for dwelling in self._dwellings
        ]

    def _env_plans(self) -> list[Any]:
        """The plans, built at the first use if no reset has run yet."""
        if self._plans is None:
            self._build_plans()
        assert self._plans is not None
        return self._plans

    def _next_episode_seed(self, idx: int) -> int:
        """The next episode seed for env ``idx``: the caller SeedSequence's
        child for this env spawns it, so the env's episodes seed
        deterministically and independently of the other envs'; with no
        caller SeedSequence the seed is OS entropy, like
        ``reset(seed=None)``."""
        if self._seed_children is not None:
            child = self._seed_children[idx].spawn(1)[0]
            return int(child.generate_state(1, dtype=np.uint64)[0])
        return secrets.randbits(63)

    def _apply_seed(self, seed: int | np.random.SeedSequence | None, idx: int) -> int:
        """The applied seed for env ``idx``'s reset: an explicit int keeps
        the documented ``seed + env index`` form; ``None`` draws from the
        env's own stream (the caller SeedSequence's child for this env
        when one was given, OS entropy otherwise)."""
        if isinstance(seed, int):
            return int(seed) + idx
        return self._next_episode_seed(idx)

    # The infos slot is the documented divergence from the base class's
    # batch-dict shape: every row is one env's own dict, the shape the
    # extension's info contract publishes.
    def reset(  # type: ignore[reportIncompatibleMethodOverride]
        self,
        *,
        seed: object = None,
        options: dict[str, Any] | None = None,
        mask: np.ndarray | None = None,
    ):
        """Reset the masked envs (every env by default) and return the batch's
        observations.

        ``seed`` is an integer (the documented ``seed + env index`` form), a
        ``numpy.random.SeedSequence`` (the per-env children re-spawn from it),
        or ``None`` (each env's own stream, OS entropy when the constructor
        received no ``seed_sequence``). Any other type is refused naming it.
        An unmasked env's state, clock, RNG and applied episode seed are left
        untouched.
        """
        del options
        seed = validated_reset_seed(seed)
        if mask is None:
            mask = np.ones(self.num_envs, dtype=bool)
        else:
            mask = np.asarray(mask, dtype=bool)
            if mask.shape != (self.num_envs,):
                raise ValueError(
                    f"mask must hold one bool per env, shape {(self.num_envs,)}, got {mask.shape}"
                )
        if isinstance(seed, np.random.SeedSequence):
            # The caller's SeedSequence (re)spawns this environment's
            # per-env children: the envs' episode seeds derive from it
            # from this reset on.
            self._seed_children = list(seed.spawn(self.num_envs))
        for idx, dwelling in enumerate(self._dwellings):
            if not mask[idx]:
                continue
            applied = self._apply_seed(seed, idx)
            dwelling.restore_state(self._initial_snapshots[idx], applied)
            # The reset restores the warmed-up state and reseeds the
            # streams, without rebuilding: the snapshot's converged
            # initial condition is kept, the episode's random streams are
            # the seed's, and no warm-up runs again. The explicit full
            # rebuild is `reset_with_seed`, which this path does not call.
            # The mask leaves the unmasked envs' state, clock and RNG
            # untouched: their next step is bitwise the run's own.
            self._applied_seeds[idx] = applied
        self._steps_elapsed = np.where(mask, 0, self._steps_elapsed)
        self._run_results = [
            None if mask[idx] else self._run_results[idx] for idx in range(self.num_envs)
        ]

        # The plans are frozen at reset; the initial observation is read
        # through them into the buffer, then returned as a fresh copy.
        self._build_plans()
        plans = self._env_plans()
        for idx, dwelling in enumerate(self._dwellings):
            dwelling.observe_into(plans[idx], self._obs_buffer[idx])
        obs = self._obs_buffer.copy()

        row_has_nan = ~np.all(np.isfinite(obs), axis=1)
        global _warned_initial_nan
        if np.any(row_has_nan) and not _warned_initial_nan:
            _warned_initial_nan = True
            warnings.warn(
                "Initial observation contains NaN for uninitialized fields "
                "— replace with 0.0 or mask before training.",
                stacklevel=2,
            )

        infos: list[dict[str, Any]] = [
            {"seed": self._applied_seeds[idx]} for idx in range(self.num_envs)
        ]
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

    # The infos slot is the documented divergence from the base class's
    # batch-dict shape: every row is one env's own dict, the shape the
    # extension's info contract publishes.
    def step(self, actions: np.ndarray):  # type: ignore[reportIncompatibleMethodOverride]
        """Advance every dwelling one timestep.

        ``actions`` is the ``(n_envs, n_actions)`` float64 array, passed to
        the extension as its own C-contiguous buffer: read in place, one
        row per dwelling, like the observation write.

        Each entry of ``infos`` carries the same shape on both step paths:
        ``"step"`` (the step dict the reward reads, ``None`` when that
        dwelling's step failed), ``"warning_count"`` (int, always
        present) and ``"warnings"`` (list[str], present only when the count
        is non-zero, the warnings drained from that dwelling during the
        step, e.g. control signals rejected at dispatch time). Both step
        paths clip actions to the declared bounds and call ``reward_fn``
        per environment on the same ``RewardContext`` built from the step
        dict and the dwelling's telemetry: one reward contract.

        With ``auto_reset`` set, an env whose episode ended (terminated or
        truncated) resets alone before the step returns: its observation
        is the new episode's first one, the ended episode's last
        observation rides ``info["final_observation"]``, and its
        ``info["seed"]`` is the new episode's applied seed (drawn from the
        env's own stream of the constructor's ``seed_sequence``, OS
        entropy when none was given). The other envs' dwellings are not
        touched by the masked reset: their next steps are bitwise the
        un-reset run's.
        """
        arr = np.asarray(actions, dtype=np.float64)
        arr = np.ascontiguousarray(arr)
        expected = (self.num_envs, self._action_dim)
        if arr.shape != expected:
            raise ValueError(f"expected actions shape {expected}, got {arr.shape}")

        rewards: list[float] = []
        dones: list[bool] = []
        truncs: list[bool] = []
        infos: list[dict[str, Any]] = []
        plans = self._env_plans()
        if rust_batch_step is not None:
            # include_step_dict=True: the reward reads each environment's
            # step dict, so this caller asks for it; the default spares
            # consumers that read the buffer alone.
            raw = rust_batch_step(
                self._dwellings,
                arr,
                plans,
                self._action_layout,
                self._signal_type_by_equipment,
                self._action_bounds,
                self._obs_buffer,
                True,
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
                dones.append(bool(row["terminated"]))
                truncs.append(bool(row["truncated"]))
                info = dict(row.get("info", {}))
                info["step"] = step_data
                infos.append(info)
        else:
            for idx, (dwelling, action_row) in enumerate(
                zip(self._dwellings, arr, strict=True)
            ):
                self._apply_actions(dwelling, action_row)
                step_data = dwelling.step()
                # The same plan the batch path reads through, into the
                # same buffer: the two paths read the same values by
                # construction.
                dwelling.observe_into(plans[idx], self._obs_buffer[idx])
                rewards.append(float(self._reward_fn(_reward_context(step_data, dwelling))))
                dones.append(False)
                truncs.append(False)
                warning_count, warning_messages = drain_step_warnings(dwelling)
                info: dict[str, Any] = {"step": step_data, "warning_count": warning_count}
                if warning_messages:
                    info["warnings"] = warning_messages
                infos.append(info)

        # Every step row reports the env's current episode seed, the same
        # field (and the same omission before the env's first reset) the
        # single-dwelling environment carries. The seed the row's step ran
        # under is the one this env's state is seeded with.
        for idx, info in enumerate(infos):
            if self._applied_seeds[idx] is not None:
                info["seed"] = self._applied_seeds[idx]

        # One contiguous n_envs x n_fields allocation per step by default:
        # an observation a caller keeps across steps is never overwritten.
        # copy=False returns the buffer itself, overwritten by the next
        # step() or reset().
        obs = self._obs_buffer.copy() if self._copy else self._obs_buffer
        reward_arr = np.asarray(rewards, dtype=np.float64)
        done_arr = np.asarray(dones, dtype=np.bool_)
        trunc_arr = np.asarray(truncs, dtype=np.bool_)

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
                # The episode's end finalises the dwelling's run: the
                # finished RunResult is the episode's report surface.
                self._run_results[idx] = self._dwellings[idx].finish()

        if self._auto_reset:
            self._auto_reset_ended(obs, done_arr, trunc_arr, infos)

        return obs, reward_arr, done_arr, trunc_arr, infos

    def _auto_reset_ended(
        self,
        obs: np.ndarray,
        done_arr: np.ndarray,
        trunc_arr: np.ndarray,
        infos: list[dict[str, Any]],
    ) -> None:
        """The masked auto-reset: every env whose episode ended
        (terminated or truncated) resets alone, from the per-env seed its
        stream spawns next; the other envs' dwellings are not touched, so
        their next steps are bitwise the un-reset run's. The ended env's
        returned observation is the new episode's first one, with the
        ended episode's last observation under
        ``info["final_observation"]``; its ``info["seed"]`` is the new
        episode's applied seed, and the finished episode's ``RunResult``
        stays on ``run_results`` until the next truncation replaces it."""
        plans = self._env_plans()
        for idx in range(self.num_envs):
            if not (done_arr[idx] or trunc_arr[idx]):
                continue
            infos[idx]["final_observation"] = obs[idx].copy()
            applied_seed = self._next_episode_seed(idx)
            self._dwellings[idx].restore_state(
                self._initial_snapshots[idx], applied_seed
            )
            self._applied_seeds[idx] = applied_seed
            infos[idx]["seed"] = applied_seed
            self._steps_elapsed[idx] = 0
            self._dwellings[idx].observe_into(plans[idx], self._obs_buffer[idx])
            obs[idx] = self._obs_buffer[idx].copy()

    @property
    def unwrapped(self) -> "VecDwellingGymEnv":
        return self

    @property
    def run_results(self) -> list[Any]:
        """Each dwelling's finished run ``RunResult``, once its episode has
        run to truncation (``None`` before that, cleared on ``reset()``)."""
        return self._run_results

    @property
    def envs(self) -> list[Any]:
        """The underlying dwellings, one per environment: their live
        ``health()`` reads at any step."""
        return self._dwellings

    def close(self, **kwargs: Any) -> None:
        """The base `VectorEnv.close`'s bookkeeping when Gymnasium is
        installed (`closed` set once, `close_extras` called); the
        dwellings hold no resources of their own to release."""
        del kwargs
        # The runtime base is the real VectorEnv whenever Gymnasium is
        # installed; the fallback's close is this branch's own.
        if _HAS_VECTOR_BASE:
            _VectorEnvBaseRuntime.close(self)  # type: ignore[reportUnknownMemberType, reportAttributeAccessIssue]
        else:
            self.closed = True

    def close_extras(self, **kwargs: Any) -> None:
        """The fleet's dwellings need no external resource release."""
        del kwargs

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
