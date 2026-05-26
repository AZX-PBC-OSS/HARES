# RL Gymnasium environment: obs/action spaces, reward, vec env correctness
**Review ID**: py-companion-04
**Category**: py-companion
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-python/src/py_gym.rs` — Rust Python bindings for batched RL stepping
- `python/ochre_next/rl/gym_env.py` — Single-dwelling Gymnasium `Env`
- `python/ochre_next/rl/vec_env.py` — Vectorized fleet-level environment wrapper

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/api/datatransfer.py` — EnergyPlus DataExchange API: sensor/actuator handles, weather data queries (hours, day, etc.), bounded variable definitions
- `vendors/EnergyPlus/src/EnergyPlus/api/runtime.py` — EnergyPlus Runtime API: simulation lifecycle, callback registration, state management
- `vendors/EnergyPlus/src/EnergyPlus/api/state.py` — EnergyPlus StateManager: `new_state()`, `reset_state()`, `delete_state()`
- `vendors/EnergyPlus/src/EnergyPlus/api/plugin.py` — EnergyPlusPlugin base class with `on_*` simulation hook callbacks
- `vendors/EnergyPlus/src/EnergyPlus/api/api.py` — EnergyPlusAPI top-level class; state lifecycle, version checks

---

## Findings

### Finding 1: Observation space uses `-inf..+inf` bounds instead of physically-meaningful ranges
**Severity**: high
**Description**: Both `DwellingGymEnv.__init__` (`gym_env.py:359-364`) and `VecDwellingGymEnv.__init__` (`vec_env.py:74-79`) construct a `gym.spaces.Box` observation space with `low=-np.inf` and `high=np.inf` for every observation dimension. This undermines RL algorithms that rely on observation-space bounds for:
- Observation normalization (e.g., `NormalizeObservation` wrapper expects finite bounds)
- Exploration noise clipping (e.g., SAC/TD3 clip observation to space bounds)
- Automatic feature scaling in SB3 / RLlib

Zone temperatures should be bounded to something like 10–50°C, SOC values to 0–1, equipment power to rated capacity ± margin, etc. The EnergyPlus API defines typed variables with known engineering units and physically plausible ranges; HARES should do the same.
**Code Location**: `gym_env.py:359-364`, `vec_env.py:74-79`
**Root Cause**: `telemetry_to_observation` computes per-field values but does not export per-field metadata (low/high) for the callers. The `observation_fields` argument is a flat list of strings with no associated bounds map.
**Impact**: RL agents receive unbounded observation signals; exploration noise may produce out-of-distribution values; SB3-compatible wrappers cannot properly normalize observations.

### Finding 2: Rust `batch_step` reward hardcodes `-net_electric_power_kw`, ignoring user-provided `reward_fn`
**Severity**: critical
**Description**: In `py_gym.rs:122`, the Rust `batch_step_py` function computes reward as:
```rust
let reward = -step.net_electric_power_kw;
```
This entirely ignores the user-supplied `reward_fn` callback that is passed to and stored by the Python `VecDwellingGymEnv` constructor. Meanwhile, the pure-Python fallback path at `vec_env.py:167` correctly calls `float(self._reward_fn(ctx))`. This means the same `VecDwellingGymEnv` instance produces **different rewards** depending on whether the `rust_batch_step` native extension is available at import time.
**Code Location**: `py_gym.rs:122` (hardcoded reward), `vec_env.py:141-183` (parallel vs. fallback divergence)
**Root Cause**: The `batch_step_py` function signature (`py_gym.rs:50-51`) accepts `action` and `observation_fields` but does not accept a `reward_fn` callback. The Rust layer has no mechanism to invoke a Python callable inside the GIL-free parallel section.
**Impact**: Reward function is silently ignored in production (native path). Any cost/comfort trade-off encoded in the user's `reward_fn` is lost. If a team tunes the reward function in single-dwelling mode (`DwellingGymEnv`) and then scales to fleet mode with `rust_batch_step`, RL policies will train against the wrong objective.

### Finding 3: `terminated` is always `False` — episodes never signal natural completion
**Severity**: high
**Description**: In `DwellingGymEnv.step()` (`gym_env.py:450`), `terminated` is unconditionally `False`. The only termination is `truncated` via `self._steps_elapsed >= self._max_steps` (line 436). There is no mechanism for the underlying simulation to signal that it has exhausted weather data, reached end-of-simulation, or encountered an unrecoverable error. In the Rust layer (`py_gym.rs:130`), `terminated` is also always `False` (except on panic, where it becomes `True`, line 140-144).
**Code Location**: `gym_env.py:450`, `py_gym.rs:130`, `vec_env.py:168`
**Root Cause**: `Dwelling.step()` returns `step_data` but the environment does not inspect it for an end-of-simulation flag. The simulation may report `{"done": true}` or similar, but this is never propagated to the Gymnasium `terminated` flag.
**Impact**: The RL agent never learns to handle terminal states naturally. If weather data runs out before the `episode_length`, the simulation steps on stale/missing weather data silently. SB3 training loops that differentiate `terminated` (episode end) from `truncated` (time limit) will misbehave.

### Finding 4: `VecDwellingGymEnv` hardcodes 60-second timestep for truncation, inconsistent with `DwellingGymEnv`
**Severity**: medium
**Description**: `DwellingGymEnv` reads time resolution from `sim_config.time_res_s` (`gym_env.py:380-381`) to compute `_max_steps`. `VecDwellingGymEnv` hardcodes `60.0` seconds at `vec_env.py:97`:
```python
self._max_steps = max(1, int(np.ceil(episode_length.total_seconds() / 60.0)))
```
If a dwelling uses a non-60-second timestep, the vectorized environment truncates at the wrong number of steps.
**Code Location**: `vec_env.py:97` vs `gym_env.py:380-381`
**Root Cause**: `VecDwellingGymEnv` does not receive a `SimulationConfig` — it receives pre-constructed `PyDwelling` objects instead of `GymDwellingConfig` objects, so time resolution information is lost.
**Impact**: Mismatched episode lengths when timestep != 60s. Subtle silent errors in multi-dwelling fleets with heterogeneous configurations.

### Finding 5: `close()` is a no-op — dwelling resources are never released
**Severity**: medium
**Description**: `VecDwellingGymEnv.close()` (`vec_env.py:189-190`) returns `None` immediately without calling any cleanup on the dwellings. The dwelling objects hold Rust-side simulation state, memory allocations, and potentially file handles. An equivalent `close()` is also absent from `DwellingGymEnv`.
**Code Location**: `vec_env.py:189-190`; `DwellingGymEnv` has no `close()` at all
**Root Cause**: The `PyDwelling` Python wrapper may not expose a `close()` or destructor method, and the environment authors may be relying on Python GC. However, Gymnasium environments are typically long-lived in training loops, and explicit resource management is expected.
**Impact**: Memory bloat in long-running RL training. If `PyDwelling` holds OS resources (shared memory, file handles), these leak until GC runs.

### Finding 6: No `step_async()` / `step_wait()` pattern — lacks SubprocVecEnv-like parallelism
**Severity**: medium
**Description**: The review instructions call for verification of the `step_async()`/`step_wait()` pattern that enables parallel environment stepping. `VecDwellingGymEnv` has a single synchronous `step()` method (`vec_env.py:126-183`). It does not conform to the Gymnasium `AsyncVectorEnv` / `SubprocVecEnv` interface. The Rust path uses Rayon thread parallelism (many dwellings in one process), which is a different concurrency model from subprocess-based parallelism and cannot scale beyond one machine.
**Code Location**: `vec_env.py:126` (single `step()` method, no async methods)
**Root Cause**: The design uses in-process threading (Rayon in Rust, sequential loop in Python fallback) rather than the multiprocessing-based `step_async`/`step_wait` pattern. The fork-safety check at `vec_env.py:43-49` actively prevents `fork` start-method, which is what `SubprocVecEnv` typically uses.
**Impact**: Cannot scale fleet size beyond a single process's memory/CPU. Integration with SB3 `SubprocVecEnv`, `AsyncVectorEnv`, or `VecNormalize` wrappers will not work without an adapter.

### Finding 7: No time-of-day encoding, occupancy, or electricity price in observation space
**Severity**: medium
**Description**: The review instructions specify that the observation space should include "time-of-day encoding, electricity price, occupancy". The `telemetry_to_observation` helper (`gym_env.py:210-257`) supports only:
- `outdoor_temp` / `outdoor_rh`
- `total_power_kw`
- `zone_temp[<name>]` / `setpoint_heat[<name>]` / `setpoint_cool[<name>]`
- `equipment_soc[<name>]` / `equipment_power[<name>]`
- `battery_soc` / `ev_soc`

There is no built-in time-of-day encoding (e.g., `sin(hour * 2π/24)`, `cos(hour * 2π/24)`), no occupancy field, no electricity price field. Users would need to embed these as custom telemetry keys. The EnergyPlus API actively exposes `hour`, `dayOfWeek`, `dayOfYear`, `currentTime`, `sunIsUp`, `isRaining`, and weather forecast data (`datatransfer.py:173-199`, `200-300`) — which HARES's `telemetry_to_observation` has no equivalent for.
**Code Location**: `gym_env.py:210-257`
**Root Cause**: The observation field dispatch is implemented as a hardcoded if/elif chain rather than a declarative schema. Adding new telemetry categories requires modifying the helper function.
**Impact**: RL agents cannot learn time-dependent behaviors (pre-cooling at night, pre-heating before morning) without manually adding time features. Occupancy-driven control policies cannot be trained.

### Finding 8: Action space bounds for thermal setpoints are too broad; no support for normalized action scaling
**Severity**: low
**Description**: In `_field_bounds` (`gym_env.py:187-188`), HVAC setpoint fields return `(-50.0, 80.0)`. While these encompass all physically possible indoor temperatures, they are overly broad for RL exploration (an agent may sample 70°C heating setpoints). The review instructions suggest normalized actions (e.g., -1.0 to 1.0 for setpoint offsets). The current action space exposes raw physical units directly; there is no `action = offset * scale + baseline` normalization layer.
**Code Location**: `gym_env.py:187-188`, `gym_env.py:366-377`
**Root Cause**: Actions are passed directly to equipment `ControlSignal` constructors with no normalization/denormalization wrapper. The design assumes the RL agent outputs physical units.
**Impact**: RL algorithms must learn to produce actions in physical units (e.g., °C, kW), which is harder than learning normalized offsets. Exploration noise (e.g., Gaussian with σ=0.1) has no consistent interpretation across action dimensions.

### Finding 9: VE fallback `dones` and `truncs` are unconditionally reset before truncation check, masking simulation termination
**Severity**: low
**Description**: In the Python fallback path of `VecDwellingGymEnv.step()`, `dones` and `truncs` are set to `False` for every dwelling (`vec_env.py:168-169`), then auto-truncation is OR'd on top (`vec_env.py:177-178`). If a dwelling's `.step()` call signals natural termination (e.g., weather exhaustion), that signal is discarded because the fallback path doesn't inspect `step_data` for a done flag.
**Code Location**: `vec_env.py:168-169`
**Root Cause**: Same as Finding 3 — no inspection of simulation-level termination signals.
**Impact**: Late-stage RL training may continue stepping dead environments, producing zero-reward garbage transitions.

### Finding 10: Rust `observation()` fallback returns default empty vec on observation error
**Severity**: low
**Description**: In `py_gym.rs:118-120`, if `observation_for_fields` fails, the code falls back to `dwelling.observation().unwrap_or_default()` — which returns an empty `Vec`. The resulting observation dimension mismatch relative to what the Python environment expects would cause a downstream crash with a confusing error.
**Code Location**: `py_gym.rs:118-120`
**Root Cause**: The observation error path (`obs_err`) is recorded but the `obs` vec is replaced with a potentially empty fallback rather than zeros of the expected length.
**Impact**: Silent dimension mismatch that crashes in `np.asarray` or `np.vstack` in the Python calling code. Hard to debug in fleet training.

---

## Summary
- **Total findings**: 10
- **Critical**: 1 (Finding 2 — Rust reward hardcode ignores reward_fn)
- **High**: 2 (Findings 1, 3 — observation bounds unbounded, terminated never True)
- **Medium**: 4 (Findings 4, 5, 6, 7 — timestep inconsistency, no close(), no step_async/step_wait, missing telemetry keys)
- **Low**: 3 (Findings 8, 9, 10 — action bounds too broad, fallback masks termination, obs fallback empties)

## Recommendations

1. **Fix the Rust reward** (Critical): Extend `batch_step_py` to accept a `reward_fn` parameter. Since Python callables cannot be invoked GIL-free, either (a) compute rewards in Python after the parallel step, or (b) accept a parametric reward config (weights for cost, comfort, demand) and compute reward inside Rust. Until fixed, the pure-Python fallback path is the only correct one.

2. **Add per-field observation bounds**: Create a `telemetry_field_bounds()` helper (mirroring `_field_bounds` for actions) so both `DwellingGymEnv` and `VecDwellingGymEnv` can set `observation_space.low/high` to meaningful finite values for each field.

3. **Propagate simulation termination**: Inspect `step_data` returned by `dwelling.step()` for an end-of-simulation flag and set `terminated=True` when the simulation has exhausted its weather data or completed the configured duration.

4. **Fix timestep inconsistency**: Pass `time_res_s` into `VecDwellingGymEnv` (extract from dwelling config or accept as a parameter) instead of hardcoding 60s.

5. **Implement proper resource cleanup**: Add a `close()` method to `DwellingGymEnv` that calls dwelling cleanup. Make `VecDwellingGymEnv.close()` iterate and close all dwellings.

6. **Add time-of-day, occupancy, and price telemetry**: Extend `telemetry_to_observation` with fields for `time_sin`, `time_cos` (cyclic hour encoding), `occupancy` (from schedule/sensor), and `electricity_price` (from tariff/rate schedule).

7. **Consider normalized action interface**: Add an optional `action_normalization="normalized"` mode where actions are in [-1, 1] and mapped to physical units via `scale * action + offset` before being passed to equipment.

## References / Citations

- EnergyPlus `DataExchange` weather/time query methods: `datatransfer.py:173-199` (hour, day, month, sunIsUp, currentSimTime), `datatransfer.py:200-300` (todayWeather*, tomorrowWeather* forecast functions)
- EnergyPlus `StateManager.new_state()`, `delete_state()` lifecycle: `state.py:82-105`
- EnergyPlus callback registration at timestep boundaries: `runtime.py:107-143` (19 distinct simulation hook points)
- Gymnasium `Box` space documentation: expects finite `low`/`high` for observation normalization wrappers
- SB3 `NormalizeObservation` wrapper: requires bounded observation space to compute running statistics
