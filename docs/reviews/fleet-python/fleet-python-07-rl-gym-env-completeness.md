# RL Gymnasium environment completeness and observation space correctness
**Review ID**: fleet-python-07
**Category**: fleet-python
**Date**: 2026-05-26

## Files Reviewed
crates/hares-python/src/py_gym.rs python/ochre_next/rl/gym_env.py python/ochre_next/rl/vec_env.py

## Vendor/Reference Files Consulted
vendors/EnergyPlus/src/EnergyPlus/api/ (datatransfer.py, api.py)

EnergyPlus exposes a rich runtime query API (`getVariableValue`, `getMeterValue`, `dayOfWeek`, `hour`, `dayOfMonth`, etc.) that provides time-contextual state on demand. HARES adopts a batched-telemetry-snapshot model via `DwellingTelemetry` and `to_observation_vec()`, which is structurally different but sufficient for vectorized RL if all necessary channels are exposed.

## Findings

### Finding 1: [Severity: critical] Observation space bounds are -inf to +inf with no normalization/scaling
**Description**: Both `DwellingGymEnv` and `VecDwellingGymEnv` declare `observation_space` with `low=-np.inf, high=np.inf` for all observation fields. No normalization, standardization, or rescaling is applied before observations reach the agent. Physically meaningful bounds for every telemetry channel are known (e.g., zone temperatures in -50C to 80C, SOC in 0-1, power in kW range) but are not encoded in the space definition. This means the agent receives raw physical units with unbounded ranges, violating the expected Gymnasium contract of semantically bounded spaces. Deep RL algorithms that use observation normalization (e.g., PPO with `VecNormalize`) will receive physically meaningless statistics.
**Code Location**: `python/ochre_next/rl/gym_env.py:359-364`, `python/ochre_next/rl/vec_env.py:74-79`
**Root Cause**: Observation space is constructed with blanket `-np.inf / +np.inf` bounds for all fields regardless of what keys are in `observation_fields`.
**Impact**: RL agent may take longer to converge or fail entirely; normalized-space wrappers produce nonsensical statistics for channels that are physically bounded.

### Finding 2: [Severity: critical] Rust batch_step reward bypasses user-provided reward_fn
**Description**: In `VecDwellingGymEnv.step()`, when the Rust `batch_step` path is active (line 141-150), the per-dwelling reward is read directly from the Rust return value (`row["reward"]`), which in `py_gym.rs:122` is hardcoded to `-step.net_electric_power_kw`. The user-provided `self._reward_fn` callback is **never called** in this path. In the Python fallback path (line 167), `self._reward_fn(ctx)` IS correctly used. This creates a silent divergence: any RL algorithm using the vectorized Rust path with a custom reward function will receive a different reward signal than intended.
**Code Location**: `python/ochre_next/rl/vec_env.py:147` (bypass), `crates/hares-python/src/py_gym.rs:122` (hardcoded reward)
**Root Cause**: The Rust `batch_step_py` function was designed as a low-level primitive and computes its own reward for the vector observation path; the Python wrapper neglects to re-compute or pass the user's reward function downstream.
**Impact**: All RL training using `VecDwellingGymEnv` with the Rust batch path ignores the user's reward function. Only the fallback Python loop respects it.

### Finding 3: [Severity: high] Critical telemetry channels for RL control decisions are not observable
**Description**: The following telemetry values that are essential for making informed energy control decisions are either not present in `DwellingTelemetry`, not exposed via `to_observation_vec()`, or not plumbed through the observation key interface:

| Required Signal | Status | Detail |
|---|---|---|
| Zone temperatures (all zones) | **Present** | `zone_temp[ZoneName]` works |
| Outdoor temperature | **Present** | `outdoor_temp` / `outdoor_temp_c` works |
| Outdoor humidity | **Present but misleading** | `outdoor_rh` is labeled "RH" but populated from `weather.outdoor_humidity_ratio` (absolute ratio, not relative humidity). See Finding 4. |
| Indoor humidity | **Missing** | No indoor/zone RH in `DwellingTelemetry` struct at all |
| Occupancy status | **Missing** | `actor_telemetry` map is populated but has no observation-key handler in `to_observation_vec()` — occupancy data from actors is completely opaque to the RL agent |
| Time-of-day (sin/cos) | **Missing** | No cyclical time encoding anywhere |
| Day-of-week | **Missing** | Not exposed |
| Electricity price signal | **Missing** | `PriceSignal` exists on `EnvironmentState` but is not copied into `DwellingTelemetry` at `dwelling/mod.rs:1983-1985` |
| Equipment status flags (HVAC mode, WH mode) | **Missing** | `equipment_modes` vector exists in `DwellingTelemetry` but has no `equipment_mode[Name]` observation key handler in `to_observation_vec()` or `telemetry_to_observation()` |
| Battery charging/discharging/idle | **Partially present** | `equipment_power[Battery]` gives power magnitude/direction but no explicit state label |
| Battery SOC | **Present** | `equipment_soc[...]` and `battery_soc` alias work |
| EV SOC & connected status | **Partially present** | `ev_soc` alias works; EV `connection_state` (telemetry key `connection_state` in `telemetry_keys.rs:127`) is not exposed via any observation key |
| PV generation | **Missing** | Exists in `ElectricalSummary.pv_generation_kw` but not in `DwellingTelemetry` |
| Net electrical load | **Present** | `total_power_kw` covers this |

**Code Location**: `crates/hares-core/src/telemetry.rs:30-108` (observation key handler list), `crates/hares-core/src/dwelling/mod.rs:1899-1988` (telemetry() construction), `python/ochre_next/rl/gym_env.py:210-257` (Python observation fields list)
**Root Cause**: The `DwellingTelemetry` struct and `to_observation_vec()` method were designed for the initial narrow test use case (`["total_power_kw", "outdoor_temp_c"]`) and never extended to cover the full RL observation surface. Critical economic and temporal context (price, time) are absent from the telemetry data model.
**Impact**: RL agent has insufficient state information to make temporally-aware, economically-optimal control decisions. Missing occupancy means the agent cannot distinguish periods when comfort matters vs when the dwelling is empty. Missing price means no TOU-aware control.

### Finding 4: [Severity: high] outdoor_rh field contains humidity ratio, not relative humidity
**Description**: The `DwellingTelemetry.outdoor_rh` field is populated at `dwelling/mod.rs:1985` as:
```rust
outdoor_rh: self.latest_env.weather.outdoor_humidity_ratio,
```
The `outdoor_humidity_ratio` field on `WeatherState` is an absolute humidity ratio (kg water / kg dry air, typically 0.0-0.03) — **not** relative humidity (0-100%). The field name `outdoor_rh` with the suffix `_rh` implies relative humidity, and the Python `telemetry_to_observation()` maps the key `outdoor_rh` directly to this value without any conversion. An RL agent consuming this value as "relative humidity" will misinterpret the magnitude.
**Code Location**: `crates/hares-core/src/dwelling/mod.rs:1985`, `python/ochre_next/rl/gym_env.py:222-224`
**Root Cause**: Field naming mismatch — the telemetry struct uses `_rh` suffix but the underlying data source is `outdoor_humidity_ratio`, which is an absolute ratio, not relative humidity.
**Impact**: RL agent receives misleading humidity values; any reward function or policy that conditions on outdoor humidity will be incorrect.

### Finding 5: [Severity: high] Episode termination not detected — agent can overstep simulation end
**Description**: Both `DwellingGymEnv.step()` and `VecDwellingGymEnv.step()` unconditionally return `terminated=False` (Python path) or `terminated: false` (Rust path). The `StepResult` struct has no simulation-complete flag, and the `step()` method on `Dwelling` has no end-detection logic. If the agent calls `step()` beyond the configured simulation duration, the dwelling clock will advance past the weather file / schedule endpoint without signaling termination. The only stopping mechanism is the `_max_steps`-based truncation, which may not align with the actual simulation end.
**Code Location**: `python/ochre_next/rl/gym_env.py:450`, `python/ochre_next/rl/vec_env.py:168`, `crates/hares-python/src/py_gym.rs:130`
**Root Cause**: No end-of-simulation detection exists in either the Rust dwelling core or the Python gym wrapper. The simulation engine silently runs past its intended duration.
**Impact**: Episodes continue past the simulation horizon, producing physically invalid states (no weather data, no schedule data) that contaminate the RL experience buffer.

### Finding 6: [Severity: medium] No action clipping applied before simulation ingestion
**Description**: Actions are passed directly from the RL agent through `_apply_action()` → `_build_control_signal()` → `dwelling.apply_control()` without any clipping or clamping to the declared action space bounds. The `_field_bounds()` function defines physically-valid ranges for action fields, but these bounds are only used to construct the `action_space` specification — they are never enforced at step time. An RL agent in early training can emit action values outside `[-50.0, 80.0]` for heating setpoints, which would silently propagate to the simulation as physically impossible thermostat settings.
**Code Location**: `python/ochre_next/rl/gym_env.py:390-400`, `python/ochre_next/rl/vec_env.py:99-108`
**Root Cause**: `_apply_action()` does not call `np.clip(action, action_space.low, action_space.high)` before constructing control signals.
**Impact**: Invalid actions produce invalid simulation states without error; exploration noise can corrupt the environment state silently.

### Finding 7: [Severity: medium] RewardContext lacks fields needed for comfort and demand-charge penalty computation
**Description**: The `RewardContext` TypedDict provides only `step` (the `StepResult`), `telemetry_zone`, `telemetry_equipment`, and `total_power_kw`. To compute a comfort penalty (deviation from setpoint), the user needs access to heating/cooling setpoints per zone. The `setpoint_heat_c` and `setpoint_cool_c` arrays exist in `DwellingTelemetry` but the `telemetry_zone` dict does not forward them — only `zone_names`, `temperature_c`, `setpoint_heat_c`, and `setpoint_cool_c` are in the Rust struct but `telemetry.zone()` in Python may not include all. Additionally, demand charge peak tracking and equipment cycling penalty require per-timestep history that `RewardContext` does not supply. The `StepResult` struct has `hvac_heating_w` and `hvac_cooling_w` but no setpoints for deviation computation.
**Code Location**: `python/ochre_next/rl/gym_env.py:69-73`, `python/ochre_next/rl/gym_env.py:438-443`
**Root Cause**: `RewardContext` was designed for the simplest test case (`-total_power_kw`) and not extended for comfort-weighted or demand-charge-aware reward functions.
**Impact**: Users must implement their own comfort/compliance reward components with limited context, or rely on external wrappers.

### Finding 8: [Severity: medium] VecDwellingGymEnv does not implement standard Gymnasium VecEnv interface
**Description**: `VecDwellingGymEnv` is a custom class, not a subclass of `gymnasium.vector.VectorEnv` or `SubprocVecEnv`. It lacks `step_async()`/`step_wait()` methods, which are the standard Gymnasium interface for parallel vectorized environments. Instead, it uses a synchronous `step()` that calls the Rust `batch_step` (Rayon-parallelized) or a Python for-loop. This means standard wrappers like `VecNormalize`, `VecMonitor`, and algorithms like Stable-Baselines3 that expect `step_async`/`step_wait` cannot use this environment without an adapter.
**Code Location**: `python/ochre_next/rl/vec_env.py:34` (class definition), entire file
**Root Cause**: The environment was designed as a standalone custom interface rather than adhering to the Gymnasium VecEnv abstract base class contract.
**Impact**: Incompatibility with standard RL training frameworks. Users must write custom wrappers.

### Finding 9: [Severity: medium] time_res_s hardcoded to 60.0 in VecDwellingGymEnv but configurable in DwellingGymEnv
**Description**: `VecDwellingGymEnv._max_steps` is computed using a hardcoded time resolution of 60.0 seconds (`vec_env.py:97`), while `DwellingGymEnv._max_steps` reads `time_res_s` from the `SimulationConfig` (with a fallback of 60.0 at `gym_env.py:379-381`). If a user configures dwellings with a non-60s time resolution, the vectorized environment will compute incorrect `_max_steps`, causing premature or delayed truncation.
**Code Location**: `python/ochre_next/rl/vec_env.py:97` vs `python/ochre_next/rl/gym_env.py:379-382`
**Root Cause**: `VecDwellingGymEnv` does not receive a `GymDwellingConfig` or `SimulationConfig` — it receives pre-constructed `PyDwelling` objects and cannot introspect their time resolution. The `episode_length` to max-steps conversion uses a hardcoded 60s denominator.
**Impact**: Episodes may truncate at the wrong timestep count when time_res_s != 60.

### Finding 10: [Severity: low] `to_observation_vec` Rust path and `telemetry_to_observation` Python path differ slightly
**Description**: The Rust `to_observation_vec()` supports `"reactive_power_kvar"` as an observation key, but the Python `telemetry_to_observation()` in `gym_env.py` has no handler for it. Conversely, the Rust path does not support `"outdoor_temp_c"` (only `"outdoor_temp"` with no `_c` suffix), but the Python path supports both forms. The Rust path checks `total_electric_kw` and `total_power_kw` as aliases; the Python path does the same. These minor asymmetries mean a field list that works in `DwellingGymEnv` (Python path) might fail in `VecDwellingGymEnv` (Rust path) and vice versa.
**Code Location**: `crates/hares-core/src/telemetry.rs:38-39` vs `python/ochre_next/rl/gym_env.py:219-220`, `crates/hares-core/src/telemetry.rs:50-52` (Rust-only `reactive_power_kvar`)
**Root Cause**: Two independent implementations of the same observation-field parser with no shared test suite ensuring parity.
**Impact**: Field lists are not portable between single and vectorized envs.

### Finding 11: [Severity: low] Setpoint telemetry falls back to current temperature when no equipment in zone
**Description**: In `dwelling/mod.rs:1924-1925`, `setpoint_heat_c` and `setpoint_cool_c` are initialized as copies of the current zone temperatures (`zone_temperatures_c.clone()`), then only overridden for zones where equipment with explicit setpoints exists (lines 1938-1954). For zones with no HVAC equipment or no active setpoint, the telemetry reports the zone's current temperature as its setpoint. This is misleading: the agent sees a "setpoint" value equal to the current temperature, which appears as if perfect setpoint tracking is happening, when in reality the zone has no active HVAC control at all.
**Code Location**: `crates/hares-core/src/dwelling/mod.rs:1924-1925, 1938-1954`
**Root Cause**: Initialization from current temperature as default, rather than using a sentinel like NaN or omitting the value.
**Impact**: RL agent receives false information about HVAC control targets in uncontrolled zones.

### Finding 12: [Severity: low] Observation error fallback produces empty observation vector
**Description**: In `py_gym.rs:115-121`, when `observation_for_fields()` fails, the code falls back to `dwelling.observation()` which calls `to_observation_vec(&["total_power_kw", "outdoor_temp", "outdoor_rh"])` — a hardcoded 3-element observation that likely has a different dimensionality than the user's `observation_fields`. If the user's observation vector is e.g., 15 elements, the fallback produces a 3-element vector, causing a shape mismatch downstream when the batch result is stacked.
**Code Location**: `crates/hares-python/src/py_gym.rs:115-121`, `crates/hares-python/src/py_dwelling.rs:1351-1357`
**Root Cause**: The fallback `observation()` method uses a fixed field list that does not match the caller's `observation_fields`.
**Impact**: Shape mismatch in batched observations when telemetry fetch errors occur; could crash RL training or produce silently corrupted observation arrays.

## Summary
- Total findings: 12
- Critical: 2 (Findings 1, 2)
- High: 3 (Findings 3, 4, 5)
- Medium: 4 (Findings 6, 7, 8, 9)
- Low: 3 (Findings 10, 11, 12)

## Recommendations
1. **Observation bounds**: Compute per-field low/high from known physical ranges in `DwellingTelemetry` (e.g., zone temps [-50, 80], SOC [0, 1], power [-100, 100] kW). Apply these bounds to `observation_space.Box` and optionally add a `VecNormalize`-compatible interface.
2. **Fix reward bypass**: In `VecDwellingGymEnv.step()`, after the Rust `batch_step` call, re-compute rewards using `self._reward_fn(ctx)` to replace the Rust-computed reward. Alternatively, support passing the reward function to the Rust layer.
3. **Extend telemetry exposure**: Add observation key handlers for `equipment_mode[Name]`, `price_signal_electricity`, `pv_generation_kw`, `ev_connected`, and a `time_sin`/`time_cos` encoding in `to_observation_vec()` and `telemetry_to_observation()`. Plumb `PriceSignal` and `ElectricalSummary.pv_generation_kw` into `DwellingTelemetry`.
4. **Fix outdoor_rh**: Either rename the field to `outdoor_humidity_ratio` or compute actual relative humidity from the humidity ratio and outdoor temperature.
5. **Episode termination**: Add a `is_finished` or `at_end` flag to the `Dwelling` step logic that returns `True` when the simulation has exhausted its configured duration/weather/schedule. Return `terminated=True` in the gym env.
6. **Action clipping**: Add `np.clip(action, self.action_space.low, self.action_space.high)` in `_apply_action()` and in `VecDwellingGymEnv._apply_controls()`.
7. **Extend RewardContext**: Include zone setpoints, price signal, and equipment modes in `RewardContext` so users can implement comfort and demand-charge penalties.
8. **VecEnv compliance**: Implement `step_async()`/`step_wait()` using the Rust batch_step dispatch mechanism, or at minimum document the deviation from Gymnasium standard.
9. **Fix VecDwellingGymEnv time_res**: Accept time_res_s as a constructor parameter or introspect it from the first dwelling instance.
10. **Unify Rust/Python observation parsers**: Delegate observation construction to a single implementation (Rust) and remove the duplicate Python parser, or at minimum add cross-tests ensuring parity.

## References / Citations
- `DwellingTelemetry` struct: `crates/hares-core/src/telemetry.rs:9-28`
- `to_observation_vec()`: `crates/hares-core/src/telemetry.rs:30-108`
- `telemetry()` construction: `crates/hares-core/src/dwelling/mod.rs:1899-1988`
- `StepResult` struct: `crates/hares-core/src/dwelling/mod.rs:326-337`
- `PriceSignal` struct: `crates/hares-types/src/environment.rs:27-32`
- `ElectricalSummary` struct (pv_generation_kw): `crates/hares-types/src/environment.rs:40-52`
- `WeatherState.outdoor_humidity_ratio`: `crates/hares-types/src/environment.rs:121-192`
- Telemetry key constants: `crates/hares-types/src/telemetry_keys.rs` (all keys)
- Rust batch_step reward: `crates/hares-python/src/py_gym.rs:122`
- Vec env Rust reward bypass: `python/ochre_next/rl/vec_env.py:147`
- Vec env time_res hardcode: `python/ochre_next/rl/vec_env.py:97`
- Observation space -inf bounds: `python/ochre_next/rl/gym_env.py:359-364`, `python/ochre_next/rl/vec_env.py:74-79`
- No termination detection: `python/ochre_next/rl/gym_env.py:450`, `crates/hares-python/src/py_gym.rs:130`
- EnergyPlus API time/state queries: `vendors/EnergyPlus/src/EnergyPlus/api/datatransfer.py:173-302`
- Test file: `tests/python/test_gym_env.py`
