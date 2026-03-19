---
id: HARES-052
title: "Pure Python — RL Gymnasium Interface"
kind: implement
depends_on: [HARES-049, HARES-045]
files_to_touch:
  - python/ochre_next/rl/__init__.py
  - python/ochre_next/rl/gym_env.py
  - python/ochre_next/rl/vec_env.py
  - crates/hares-python/src/py_gym.rs
references:
  - docs/architecture/03-control-interfaces.md
verification:
  - uv run pytest tests/python/ -v -k "gym"
---

## Background/Context
Reinforcement learning researchers need a standard `gymnasium.Env` wrapping a single dwelling and a vectorised wrapper for parallel training. Reproducibility requires that `reset(seed=N)` always produces the same trajectory. `VecDwellingGymEnv` uses the Rust-side Rayon parallel step rather than Python `multiprocessing` because `fork` + Rayon deadlocks; it is compatible with Stable-Baselines3 `DummyVecEnv` but not `SubprocVecEnv`.

## Work to Do
- [ ] Implement `gym_env.py`: `DwellingGymEnv(gymnasium.Env)`
  - [ ] Constructor: `config: DwellingConfig`, `observation_fields: list[str]`, `action_space_config: dict`, `reward_fn: Callable[[dict], float]`, `episode_length: timedelta`
    - [ ] The env internally constructs a `PyDwelling` from `config` and calls `initialize()` on it; users do NOT call `initialize()` themselves — the env handles it. This ensures `reset()` always restores from a valid post-initialization state.
    - [ ] At construction time, after internal initialization, save an in-memory snapshot of the dwelling state using `PyDwelling.save_state()` from HARES-049; this snapshot is the initial checkpoint used by `reset()`
    - [ ] Derive `observation_space` (`gymnasium.spaces.Box`, dtype `float64`, shape `(len(observation_fields),)`) and `action_space` from `action_space_config` at construction time; both must be valid `gymnasium.Space` instances checkable with `env.observation_space.contains(obs)`
  - [ ] `reset(seed: int | None = None) -> tuple[np.ndarray, dict]` — calls `PyDwelling.load_state(initial_snapshot)` (from HARES-049) to restore from the in-memory initial checkpoint (no disk I/O), then calls `PyDwelling.reset_with_seed(seed)` if seed is provided; the same seed always produces an identical subsequent trajectory; `seed=None` uses a non-deterministic seed
  - [ ] `step(action: np.ndarray) -> tuple[np.ndarray, float, bool, bool, dict]` — maps the N-dimensional `float64` action array to individual `ControlSignal` values using `action_space_config`; action space config format aligns with the architecture (`03-control-interfaces.md`): `{'equipment_name': ['signal_field', ...]}` maps each equipment to its controllable signal field names; bounds for each field are obtained from the equipment's descriptor (e.g., `equipment.bounds()`), not inlined in the config; flat action array index is derived from sorted equipment names, then sorted signal fields within each — Python 3.7+ dict insertion order is NOT relied upon for index stability; calls `Dwelling.step()`, builds observation from telemetry fields, computes reward via `reward_fn`
  - [ ] Observations: contiguous `float64` numpy array from telemetry, shape `(len(observation_fields),)`
  - [ ] `observation_space` and `action_space` are `gymnasium.Space` instances, set at construction time
- [ ] Implement `vec_env.py`: `VecDwellingGymEnv`
  - [ ] Wraps `N` independent pre-constructed `PyDwelling` instances; parallelism is achieved through `batch_step()` from HARES-049 (not a custom Rust function in this ticket); calls the Rust batch entry point with GIL released — NOT N sequential Python calls to `PyDwelling.step()`
  - [ ] `py_gym.rs` stub in `crates/hares-python/src/py_gym.rs` contains the Rust-side `batch_step` entry point (defined in HARES-049)
  - [ ] `step(actions: np.ndarray)` — shape `(N, action_dim)`; passes all actions to `batch_step()` from HARES-049 with GIL released; returns `(obs, rewards, dones, truncs, infos)` with batch dimension N
  - [ ] Compatible with SB3 `DummyVecEnv` interface; NOT compatible with `SubprocVecEnv` — add a runtime assertion at construction time that raises `RuntimeError` if `multiprocessing` fork start method is active, with a message explaining that fork + Rayon deadlocks
- [ ] `python/ochre_next/rl/__init__.py`: export `DwellingGymEnv`, `VecDwellingGymEnv`

## Files to Touch
- `python/ochre_next/rl/__init__.py`: subpackage exports
- `python/ochre_next/rl/gym_env.py`: `DwellingGymEnv` implementation
- `python/ochre_next/rl/vec_env.py`: `VecDwellingGymEnv` implementation
- `crates/hares-python/src/py_gym.rs`: Rust-side `batch_step` entry point (defined in HARES-049)

## Measures of Success
- [ ] `DwellingGymEnv.reset(seed=42)` followed by 10 `step()` calls produces the same observation sequence when repeated with `seed=42`
- [ ] `step()` observation shape matches `(len(observation_fields),)` and dtype is `float64`
- [ ] `env.observation_space` and `env.action_space` are valid `gymnasium.Space` instances; `env.observation_space.contains(obs)` returns `True` for observations returned by `step()`
- [ ] Action array correctly maps to `ControlSignal` values according to `action_space_config` field order
- [ ] `VecDwellingGymEnv` with `N=4` returns observation array of shape `(4, obs_dim)` from a single `step` call, dispatched through the Rust batch entry point
- [ ] `VecDwellingGymEnv` is constructible and runnable from SB3 training loop without modification
- [ ] Constructing `VecDwellingGymEnv` when `multiprocessing` fork start method is active raises `RuntimeError` with an explanatory message

## Verification
- [ ] `uv run pytest tests/python/ -v -k "gym"` passes
