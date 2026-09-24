# RL observation space: state variables, physically correct bounds, no infinite ranges
**Review ID**: pygap-08
**Category**: python-gaps
**Date**: 2026-05-26

## Files Reviewed
- `python/ochre_next/rl/gym_env.py` — Single-dwelling Gymnasium `Env` (observation space definition)
- `python/ochre_next/rl/vec_env.py` — Vectorized environment (observation space replicated)
- `crates/hares-core/src/telemetry.rs` — Rust `DwellingTelemetry` and `to_observation_vec()`
- `crates/hares-python/src/py_gym.rs` — Rust bindings; batched observation construction
- `crates/hares-types/src/telemetry_keys.rs` — All telemetry key constants

## Vendor/Reference Files Consulted
None

## Findings

### Finding 1: Observation space bounds are unbounded (−inf..+inf) for all dimensions [Severity: critical]
**Description**: Both `DwellingGymEnv.__init__` (`gym_env.py:359-364`) and `VecDwellingGymEnv.__init__` (`vec_env.py:74-79`) construct the `gymnasium.spaces.Box` observation space with `low=-np.inf` and `high=np.inf` for every observation dimension. This is applied uniformly regardless of the variable semantics — outdoor temperature, zone temperature, SOC fractions, and power values all share the same unbounded range.

Physically correct bounds should be:
| Variable | Low | High |
|----------|-----|------|
| Outdoor temperature | −50°C | 55°C |
| Zone temperature | 0°C | 50°C |
| Relative humidity | 0.0 (fraction) | 1.0 (fraction) |
| Battery / EV SOC | 0.0 (fraction) | 1.0 (fraction) |
| Equipment power | 0.0 | Rated capacity + margin |
| Net load (total_power_kw) | −rated generation | Rated consumption |

The consequences of unbounded observations are severe:
- **RL algorithm breaks**: Gymnasium wrappers like `NormalizeObservation` (used by SB3) require finite bounds to compute running statistics for normalization. These wrappers will raise `ValueError` or silently produce NaN.
- **Exploration noise unbounded**: SAC, TD3, and other algorithms that clip exploration noise to observation bounds degenerate to unbounded exploration.
- **Normalization impossible**: Without known mins/maxes, any downstream normalization step cannot function correctly.
**Code Location**: `gym_env.py:359-364`, `vec_env.py:74-79`
**Root Cause**: No per-field bounds metadata exists. The `observation_fields` parameter is a flat list of strings with no associated range information. The code makes the simplest possible choice — `-inf..+inf` — rather than deferring to a field-to-bounds lookup table.
**Impact**: RL training fails or degrades with any algorithm that normalizes observations (essentially all modern DRL). The agent cannot be trained with SB3, RLlib, or any framework that uses observation normalization.

### Finding 2: Critical observation variables are missing from the telemetry dispatch [Severity: high]
**Description**: The `telemetry_to_observation` function (`gym_env.py:210-258`) supports only a fixed set of field keys: `outdoor_temp`/`outdoor_temp_c`, `outdoor_rh`, `total_power_kw`/`total_electric_kw`, per-zone temperatures and setpoints (`zone_temp[name]`, `setpoint_heat[name]`, `setpoint_cool[name]`), per-equipment SOC and power (`equipment_soc[name]`, `equipment_power[name]`), and flat aliases `battery_soc`/`ev_soc`. Unknown fields raise `KeyError` (`gym_env.py:257`).

The following variables are **absent** from the dispatch and cannot be added by users without modifying HARES source code:

| Missing variable | Why it matters |
|-----------------|----------------|
| **Zone indoor humidity** / humidity ratio | `outdoor_rh` is available but no per-zone `zone_rh[name]` exists. Comfort-aware RL policies need indoor humidity to avoid condensation/mold risk. The `DwellingTelemetry` struct has no zone humidity vector. |
| **Electricity price** (current + forecast) | Critical for load-shifting and economic dispatch. Without price, the agent cannot decide *when* to consume. |
| **PV power output** as a first-class field | Only available via `equipment_power[PV]` if the dwelling happens to have equipment named "PV". No dedicated `pv_power` alias exists (unlike `battery_soc`/`ev_soc`). |
| **Net load decomposition** (grid import vs. export) | `total_power_kw` is signed net power. The agent cannot distinguish importing cheap power from exporting PV surplus. |
| **Time encoding** (sin/cos of hour-of-day) | No built-in `time_sin`, `time_cos` or `day_of_week` fields. The agent is temporally blind — it cannot learn diurnal patterns (pre-cooling at night, pre-heating before morning peak). |
| **DR event flags** | No demand-response event indicator. The agent cannot condition behavior on grid stress signals. |
| **Battery SOC as percentage** | SOC is returned as fraction 0–1. Converting to 0–100% for display is a minor concern, but the review spec calls for SOC in %. |
| **Occupancy** | No occupancy-derived field (people count, metabolic heat, CO₂). Occupancy-driven setback strategies cannot be learned. |

The `DwellingTelemetry` struct (`telemetry.rs:11-27`) contains `actor_telemetry: HashMap<String, HashMap<String, f64>>` — a general-purpose per-actor telemetry channel. However, neither the Rust `to_observation_vec` (`telemetry.rs:32-108`) nor the Python `telemetry_to_observation` (`gym_env.py:210-258`) resolve fields against `actor_telemetry`. This infrastructure exists but is **not wired into observation construction**.
**Code Location**: `gym_env.py:210-258` (dispatch chain), `telemetry.rs:32-108` (Rust dispatch), `telemetry.rs:27` (unused `actor_telemetry`)
**Root Cause**: The observation field dispatch is a hardcoded if/elif chain in both Python and Rust, not a schema-driven lookup. Adding a new field requires modifying both code paths. The `actor_telemetry` channel — which would be the natural place for custom fields like price and DR flags — is entirely unused for observation resolution.
**Impact**: Agents make control decisions with incomplete information. Time-blind agents cannot shift load across hours. Price-blind agents cannot optimize cost. Humidity-blind agents risk condensation events. These are not theoretical — every residential energy management RL paper includes time encoding, price, and humidity in the observation vector.

### Finding 3: No observation normalization infrastructure [Severity: high]
**Description**: Observations are returned as raw physical values (temperatures in °C, power in kW, humidity as 0–1 fraction). There is no mechanism — not even an optional one — to normalize the observation vector to approximately [0, 1] or [−1, 1] before the agent receives it.

The data flow is:
```
dwelling.telemetry() → telemetry_to_observation() → raw numpy array → agent
```

Specifically:
- No `normalize=True` constructor flag.
- No normalization constants (mean/std or min/max) are stored, documented, or exposed.
- No separate accessor for raw (unnormalized) observations for debugging.
- The `_observation()` wrapper in `DwellingGymEnv` (`gym_env.py:386-388`) is a pass-through that adds only `np.ascontiguousarray`.

This forces users to wrap the environment with `gymnasium.wrappers.NormalizeObservation`, which requires **finite observation space bounds** (see Finding 1). Since bounds are −inf..+inf, that wrapper fails. Users must write custom wrapper code outside HARES.
**Code Location**: `gym_env.py:386-388` (`_observation()`), `gym_env.py:359-364` (space definition)
**Root Cause**: The observation resolution function produces raw engineering-unit values. No design for a two-tier (raw/normalized) observation pipeline exists.
**Impact**: Neural network RL agents receive inputs with disparate scales (e.g., 20°C temperature vs 5000W power). Gradient descent slows drastically. Users must implement their own normalization outside HARES, increasing integration friction.

### Finding 4: No temporal window, history stacking, or forecast horizon [Severity: high]
**Description**: The observation vector is a single-timestep snapshot. Each `step()` returns one observation corresponding to that timestep only. There is:

- **No history stacking**: No option to concatenate the last N observations into a single vector (common in DRL: `FrameStack(4)`).
- **No forecast horizon**: No electricity price forecast fields (next N hours), no outdoor temperature forecast, no PV generation forecast. These are critical for model-predictive-control-style RL.
- **No statefulness in the observation pipeline**: The `telemetry_to_observation` function is stateless — it has no access to prior observations.

The `VecDwellingGymEnv` shares the same limitation (`vec_env.py:120-122` in `reset()`, lines 146/160 in `step()`).

The `DwellingTelemetry` struct has `timestep_index` and `current_time` (`telemetry.rs:12-13`) but these are not surfaced as observation fields. No time fields are available, not even raw hour-of-day.
**Code Location**: `gym_env.py:386-388` (`_observation` returns single observed vector), `gym_env.py:434` (`step()` returns single vector), `vec_env.py:120-122`
**Root Cause**: The observation pipeline is designed as a stateless per-step transformation. There is no ring buffer, no window configuration, and no forecast data source. The `DwellingTelemetry` struct carries no forecast data — just current-step values.
**Impact**: RL algorithms that benefit from temporal context (PPO with LSTMs, Dreamer, TD-MPC) are handicapped. Price-aware load shifting requires price forecasts — without them, the agent must learn an implicit world model of price dynamics, dramatically increasing sample complexity.

### Finding 5: Observation dimensionality is fixed per zone/equipment count — no padding or generalization [Severity: medium]
**Description**: The observation vector length is determined by `len(self._observation_fields)` and is fixed at construction time (`gym_env.py:338`, `vec_env.py:94`). Each field string must resolve to a valid zone or equipment name present in the dwelling model.

If a dwelling has different zone names or a different equipment inventory, the field resolution fails:
- `zone_temp[LivingRoom]` requires a zone named "LivingRoom" in the dwelling. If absent, `KeyError` (`gym_env.py:257`) or `HaresError::Control` (`telemetry.rs:56`).
- `battery_soc` requires an equipment named "Battery" (`gym_env.py:251`, `telemetry.rs:92-93`). If absent, error.
- `ev_soc` requires an equipment named "Electric Vehicle" (`gym_env.py:253`).

There is **no padding mechanism** for partial zone/equipment coverage. A dwelling with 3 zones cannot use an observation space designed for 2 zones, and vice versa. There is **no variable-length observation** support (the `Box` space requires fixed `shape`). There is **no separate environment-per-zone-count** factory.
**Code Location**: `gym_env.py:229-248` (zone/equipment resolution by exact name match), `gym_env.py:250-254` (flat aliases with hardcoded equipment names)
**Root Cause**: The observation field dispatch uses exact ASCII-normalized name matching against the dwelling's zone and equipment names. There is no "best effort" resolution with zero-fill for missing zones, and no dynamic observation dimension.
**Impact**: A fleet of heterogeneous dwellings (different zone counts, different equipment names) requires a separate `DwellingGymEnv` per dwelling configuration. Cannot train a single RL policy across diverse building stock without external mapping/padding.

### Finding 6: First-timestep observation defaults are ambiguous and incomplete [Severity: medium]
**Description**: At `reset()` (or directly after construction + `initialize()` in `__init__`), the observation is taken before any simulation step has run. The observed values depend on what the Rust `Dwelling` initializes in its telemetry.

In `telemetry_to_observation`:
- `outdoor_temp` / `outdoor_temp_c` falls back to `0.0` if the zone data key is missing (`gym_env.py:220`). A 0.0°C outdoor temperature is physically meaningful and could be confused with a valid measurement.
- `outdoor_rh` falls back to `0.0` (`gym_env.py:223`). 0% RH is unrealistic for ambient air.
- Zone temperatures do **no fallback** — they index directly into `zone["temperature_c"]` arrays (`gym_env.py:231`). If the array is uninitialized or defaulted to 0.0 by Rust, the agent sees 0.0°C as a zone temperature.
- Equipment SOC does no fallback — direct array indexing (`gym_env.py:243`).
- `total_power_kw` uses the telemetry value directly (`gym_env.py:226`) — no fallback.

The Rust path (`telemetry.rs:32-108`) has **no fallback values at all** — it directly reads struct fields and vector elements, which will contain whatever Rust initializes them to (typically 0.0 for `f64` defaults unless explicitly set during `initialize()`).

There is no explicit "first timestep" sentinel or flag. The agent cannot distinguish a valid reading from an uninitialized default.
**Code Location**: `gym_env.py:220,223` (fallbacks for outdoor temp/RH), `gym_env.py:231,243` (no fallback for zone temp/SOC), `telemetry.rs:38-44` (Rust path: no fallbacks)
**Root Cause**: The observation resolution treats all steps identically. No special-case logic exists for the post-initialization / post-reset state where the simulation has not yet advanced. The Rust `DwellingTelemetry` is populated from the dwelling's internal state, which may contain default/zero values before the first real timestep.
**Impact**: The initial observation may contain misleading values (0.0°C outdoor temperature, 0.0 SOC, 0.0 zone temperature). If an RL algorithm uses the initial observation to condition exploration, this injects systematic bias into early training. The symptom is difficult to diagnose because subsequent timesteps produce correct values.

### Finding 7: Rust and Python observation dispatch are not functionally equivalent [Severity: medium]
**Description**: The Rust `to_observation_vec` (`telemetry.rs:32-108`) and Python `telemetry_to_observation` (`gym_env.py:210-258`) are supposed to be functionally equivalent, but they are not:

1. **`reactive_power_kvar` is supported in Rust but not in Python**: Rust resolves `reactive_power_kvar` at `telemetry.rs:50-52`. The Python dispatch has no corresponding branch. Any observation field list containing `reactive_power_kvar` works in Rust but raises `KeyError` in Python.

2. **Fallback behavior differs**: Python provides defaults (`outdoor_temp` → 0.0, `outdoor_rh` → 0.0). Rust provides no defaults — it reads struct fields directly. If the Rust `DwellingTelemetry` has NaN or unset values, the Python path (via `PyTelemetry.zone()`) may or may not see the same value.

3. **Index map construction differs**: Python uses direct name matching via `_bracket_name` and `_index_map` helpers. Rust uses `normalize_ascii()` in its index maps. If `normalize_ascii` does more than `str.strip().lower()` (which is what Python does), the two paths may resolve names differently.

This matters because `VecDwellingGymEnv` can take either the Rust `batch_step` path or the Python fallback path (`vec_env.py:141-183`), producing potentially different observation vectors.
**Code Location**: `telemetry.rs:50-52` (Rust-only `reactive_power_kvar`), `gym_env.py:210-258` (Python-only dispatch)
**Root Cause**: Two independently maintained dispatch chains. No shared schema or code generation ensures equivalence. The Rust `reactive_power_kvar` field appears to have been added to the Rust side without a corresponding Python update.
**Impact**: Silent observation vector divergence between single-dwelling mode and Rust-batched fleet mode. RL policies trained in one mode may behave unexpectedly in the other.

### Finding 8: `step()` and `reset()` use inconsistent code paths for observation construction [Severity: low]
**Description**: `DwellingGymEnv.reset()` calls `self._observation()` (`gym_env.py:418`), which wraps the result in `np.ascontiguousarray`. `DwellingGymEnv.step()` calls `telemetry_to_observation()` directly (`gym_env.py:434`), bypassing `_observation()`.

Currently `_observation()` is a trivial wrapper, so there is no behavioral difference. However, if `_observation()` is later extended with normalization, history stacking, or clipping, the discrepancy will become a real bug: `reset()` observations would be processed differently from `step()` observations.
**Code Location**: `gym_env.py:418` (reset uses `_observation()`), `gym_env.py:434` (step uses `telemetry_to_observation` directly)
**Root Cause**: `_observation()` was defined as a helper but inconsistently adopted. Only `reset()` calls it.
**Impact**: Currently none — but very high fragility. Any future change to `_observation()` creates a silent training-time bug. Low-severity finding because it only affects future development risk.

---

## Summary
- **Total findings**: 8
- **Critical**: 1 (Finding 1 — infinite observation bounds)
- **High**: 3 (Finding 2 — missing observation variables; Finding 3 — no normalization; Finding 4 — no history/forecast)
- **Medium**: 3 (Finding 5 — fixed zone count; Finding 6 — first-timestep defaults; Finding 7 — Rust/Python dispatch divergence)
- **Low**: 1 (Finding 8 — inconsistent code paths)

## Recommendations

1. **Add per-field observation bounds** (Critical): Create a `_observation_field_bounds(field: str) -> tuple[float, float]` helper (analogous to `_field_bounds` for actions) that returns physically correct bounds for each known field. Use these bounds in both `DwellingGymEnv` and `VecDwellingGymEnv` observation space construction.

2. **Wire `actor_telemetry` into observation resolution** (High): The `DwellingTelemetry.actor_telemetry` map exists but is unused. Extend both Rust and Python dispatch to resolve fields not matching built-in keys against `actor_telemetry`. This would allow users to inject electricity price, DR flags, occupancy, and time encoding fields without modifying HARES source.

3. **Add built-in temporal observation fields** (High): Expose `current_time` from `DwellingTelemetry` as `time_sin` and `time_cos` (hour-of-day cyclic encoding) and `day_of_week`. This is low-effort (the `DateTime<FixedOffset>` value is already present in the struct) and high-value for RL.

4. **Add zone indoor humidity to telemetry** (High): Extend `DwellingTelemetry` with a `zone_humidity` vector (or `zone_humidity_ratio`). This requires the humidity solver to publish per-zone values into the telemetry struct.

5. **Provide optional normalization wrapper** (High): Add a `normalize_obs=True` flag to the environment constructor. When enabled, wrap observations with a configurable `NormalizeObservation`-style running-statistics or min-max scaler. Expose raw observations via `env.unwrapped._raw_observation()` for debugging.

6. **Support observation frame stacking** (High): Add an `obs_history: int = 1` parameter and maintain a ring buffer of the last N observations. Stack them into a single output vector.

7. **Add padding/zero-fill for missing zones/equipment** (Medium): Instead of raising `KeyError` for unknown zone names, accept a `default_value: float = 0.0` parameter and fill missing zones with that value. This would allow a single observation space to work across heterogeneous dwellings.

8. **Unify Rust and Python observation dispatch** (Medium): Generate the dispatch from a shared schema (e.g., a JSON registry of supported field keys and their resolution expressions) to prevent the `reactive_power_kvar` discrepancy from recurring. At minimum, add `reactive_power_kvar` to the Python dispatch.

9. **Use `_observation()` consistently** (Low): In `step()`, replace the direct `telemetry_to_observation` call with `self._observation()` so that both `reset()` and `step()` go through the same code path.

## References / Citations
- Gymnasium `Box` space docs: finite `low`/`high` required for `NormalizeObservation` wrapper
- Stable-Baselines3 `VecNormalize`: requires bounded observation space for running-mean/std computation
- SB3 `NormalizeObservation`: raises `ValueError("Cannot compute empirical normalization for infinite bounds")` on unbounded spaces
- Typical RL observation for building control (e.g., CityLearn, Sinergym): includes time encoding, electricity price, indoor temp, outdoor temp, SOC, net load, and PV generation — all with finite bounds
- `actor_telemetry` infrastructure in `DwellingTelemetry` (`telemetry.rs:27`): exists but is unused for observation resolution
- Rust-only `reactive_power_kvar` field: `telemetry.rs:50-52`, absent from `gym_env.py:210-258`
