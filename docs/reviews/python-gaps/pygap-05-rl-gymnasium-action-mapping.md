# RL Gymnasium action mapping: PyNotImplementedError gap, what's needed for RL training completion
**Review ID**: pygap-05
**Category**: python-gaps
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-python/src/py_gym.rs` (172 lines)
- `python/ochre_next/rl/gym_env.py` (450 lines)
- `python/ochre_next/rl/vec_env.py` (204 lines)
- `crates/hares-types/src/control_signal.rs` (422 lines)
- `crates/hares-equipment/src/battery/mod.rs:1201-1249`
- `crates/hares-equipment/src/ev/mod.rs:880-1009`
- `crates/hares-equipment/src/hvac/thermostat.rs:445-479`
- `crates/hares-equipment/src/hvac/hvac_core.rs:665-672`
- `crates/hares-equipment/src/hvac/helpers.rs:176-217`
- `crates/hares-python/src/py_actor.rs:198-378`
- `tests/python/test_gym_env.py` (172 lines)

## Vendor/Reference Files Consulted
None

## Findings

### Finding 1: Entire action mapping pipeline is stubbed [Severity: critical]
**Description**: The Rust `batch_step` function at `crates/hares-python/src/py_gym.rs:68-73` rejects every non-empty action vector with `PyNotImplementedError`. The check is a blunt gate: `actions.iter().any(|a| !a.is_empty())` — any populated action vector immediately terminates the call with an error. This means the parallel Rayon stepping path is entirely unavailable for RL training; there is no fallback or partial handling.
**Code Location**: `crates/hares-python/src/py_gym.rs:68-73`
**Root Cause**: The action-to-ControlSignal mapping inside the GIL-free Rayon section was never implemented. The workaround (applying controls via `dwelling.apply_control()` in Python before calling `batch_step`, then passing empty action vectors) only works in the `VecDwellingGymEnv.step()` code path (`vec_env.py:138-139, 143-144`). No equivalent exists for `DwellingGymEnv.step()` because it calls `dwelling.step()` directly.
**Impact**: No RL agent can pass continuous action vectors through the Rust parallel stepping path. The vectorized environment can work around this by applying controls in Python, but the single-dwelling environment has no workaround. The Rust path is effectively dead code for RL training until this is resolved.

### Finding 2: `_apply_action()` has no action clipping or validation [Severity: high]
**Description**: In `gym_env.py:390-400`, the `_apply_action()` method passes raw neural network outputs directly to `_build_control_signal()` without any `np.clip(action, low, high)` call. The action space Box bounds defined at `gym_env.py:366-377` are declarative only — they inform the RL algorithm about the valid range but are never enforced at step time. The same gap exists in `VecDwellingGymEnv._apply_controls()` at `vec_env.py:99-108`.
**Code Location**: `gym_env.py:390-400`, `vec_env.py:99-108`
**Root Cause**: The action space `Box(low, high)` is used to inform the algorithm, but the step function trusts the caller to respect bounds. No defensive `np.clip` is applied.
**Impact**: An RL agent during exploration will send actions far outside the declared bounds (e.g., heating setpoint of 500°C, SOC target of -5). These values flow through unchecked into Rust `ControlSignal` constructors and then into equipment `apply_control_unchecked()` where most equipment performs zero numeric validation. This can produce physically impossible simulation states, NaN propagation, or silent corruption.

### Finding 3: `_field_bounds()` is incomplete — many fields fall to [-1e6, 1e6] [Severity: medium]
**Description**: The `_field_bounds()` function at `gym_env.py:181-191` classifies fields into four category-specific ranges: `[0,1]` for SOC/fraction, `[0,1]` for booleans, `[-50,80]` for temperatures, `[0,30]` for deadband. All other fields fall to `_BROAD_LOW.._BROAD_HIGH` = `[-1e6, 1e6]`. Fields that should have bounded ranges but fall into the broad bucket include:
- `active_power_kw`: should be bounded to equipment rated power (typical residential: `[-(5.._15), (5..15)]` kW)
- `max_power_kw`: should be `[0, rated_power]` kW
- `ramp_rate_kw_per_s`: should be `[0, max_ramp]`
- `power_factor`: should be `[0, 1]`
- `reactive_power_kvar`: should be bounded by inverter kVA rating
- `capacity_w`: should be `[0, rated_capacity]` W
- `departure_hour`: should be `[0, 24]` or `[0, 1]` as fraction-of-day
- `delay_s`: should be `[0, max_delay]`
- `p_setpoint_kw`, `kw`: fallback aliases with no bounds
**Code Location**: `gym_env.py:181-191`
**Root Cause**: `_field_bounds()` uses a name-based heuristic with an incomplete whitelist. Power-related fields, EV-specific fields, and capacity/ramp fields are not recognized.
**Impact**: RL exploration in these dimensions will sample from an enormous range, making training extremely sample-inefficient. The agent wastes steps exploring values that are physically meaningless.

### Finding 4: No cross-field constraint validation [Severity: high]
**Description**: The `ThermalSetpoint` variant carries three related fields: `heating_setpoint_c`, `cooling_setpoint_c`, and `deadband_c`. For physically valid operation, the constraint `cooling_setpoint_c >= heating_setpoint_c + deadband_c` must hold. Neither the Python action mapping (`gym_env.py:270-275`) nor the Rust thermostat (`thermostat.rs:445-479`) validates this. The thermostat `apply_thermal_setpoint_signal()` method at `thermostat.rs:448-457` stores `heating_c` and `cooling_c` independently into `RuntimeSetpointOverride` without any consistency check. The downstream `effective_setpoints()` may swap them or produce undefined behavior. Similarly, `SOCTarget` has `min_soc < target_soc < max_soc` ordering constraints that are not checked in the battery equipment at `battery/mod.rs:1210-1222`.
**Code Location**: `thermostat.rs:448-457`, `battery/mod.rs:1210-1222`
**Root Cause**: The thermostat FSM stores raw setpoint values without cross-checking. The battery stores `soc_target_min` and `soc_target_max` as independent `Option<f64>` fields.
**Impact**: An RL agent during exploration will frequently send `heating_c > cooling_c`, producing a physically degenerate thermostat state. The simulation will not panic, but the resulting HVAC behavior is undefined. SOC ordering violations (e.g., `min_soc=0.8, target_soc=0.3, max_soc=0.9`) cause the battery to operate with invalid constraints.

### Finding 5: No discretization strategy for boolean/integer action dimensions [Severity: medium]
**Description**: The action space is a continuous `Box` for all dimensions, but several `ControlSignal` variants have boolean or discrete components: `GridConnect { connected: bool }`, `SelfConsumption { enabled: bool, solar_only_charging: bool }`, `ModeOverride { mode: OperatingMode }`, `InverterPriorityMode { priority: InverterPriority }`, `DemandResponse { level: DRLevel }`, `DutyCycleComponent { component: ... }`. The `_build_control_signal()` function at `gym_env.py:308-309, 312-314` uses a threshold `>= 0.5` to convert continuous values to booleans for `GridConnect` and `SelfConsumption`, but this is applied at signal construction time, not in the action space definition. Other discrete types (mode enums, DR levels, inverter priority) have no mapping path from continuous actions to enum variants.
**Code Location**: `gym_env.py:308-309, 312-314`, `vec_env.py:99-108`
**Root Cause**: The Gymnasium `Box` action space is continuous, but the codebase has no systematic discretization layer. The boolean thresholding is ad-hoc in `_build_control_signal()`. Integer/enum-valued controls have no continuous-to-discrete mapping at all.
**Impact**: Multi-category discrete actions (OperatingMode with 11 variants, DRLevel with 5 variants) cannot be expressed in the current continuous action space. An RL agent that needs to set equipment mode or demand response level has no way to do so through the Gymnasium interface.

### Finding 6: Equipment-level numeric validation is inconsistent [Severity: high]
**Description**: The `ControlSignal::validate_numeric_bounds()` method does not exist. Validation across equipment `apply_control_unchecked()` implementations is haphazard:

| Equipment | File:Line | What is validated |
|-----------|-----------|-------------------|
| EV | `ev/mod.rs:890-928` | Finiteness of `PowerSetpoint`, `PowerLimit`, `SOCTarget`, `EvDrive`, `EvAwayCharge`; SOC in [0,1]; kwh >= 0 |
| Battery | `battery/mod.rs:1236` | Only `PowerLimit.max_power_kw.max(0.0)` |
| HVAC (heating) | `hvac/helpers.rs:189-193` | Only `deadband_c` finiteness and non-negative |
| HVAC (general) | `hvac_core.rs:670` | Only `MaxCapacityFraction.fraction.clamp(0,1)` |
| Water Heater (all types) | — | No numeric validation at all |
| PV | — | No numeric validation at all |
| Generator | — | No numeric validation at all |

The following physically invalid values pass through unchecked in all equipment except EV:
- Negative `PowerSetpoint.active_power_kw` on non-V2G battery (silently discharges)
- `LoadFraction.fraction = 5.0` (load 500% of rated)
- `DutyCycle.on_fraction = 2.0` (on 200% of the time)
- `SOCTarget.target_soc = -0.5` (negative SOC target on battery)
- `IdealCapacity.capacity_w = -1000.0` (negative heating capacity on HVAC)
- `ThermalSetpointDelta.heating_delta_c = 100.0` (100°C delta on furnace)

**Code Location**: Equipment `apply_control_unchecked()` in `battery/mod.rs:1201`, `ev/mod.rs:880`, `hvac/helpers.rs:178`, `hvac/hvac_core.rs:665`, water heater files, `pv/mod.rs`, `generator.rs`
**Root Cause**: The `Equipment` trait boundary at `hares-equipment/src/lib.rs:122-129` only validates capability bitflags (`ensure_signal_supported`), not numeric ranges. Each equipment is responsible for its own validation, but most skip it.
**Impact**: Random exploration by an RL agent will inject NaN, infinite, or out-of-range values into equipment state, causing cascading failures or silent simulation corruption. The EV module is the only equipment with adequate validation, demonstrating the pattern that should be applied uniformly.

### Finding 7: Rust `batch_step` cannot call Python reward function [Severity: medium]
**Description**: The Rust `batch_step` function at `py_gym.rs:122` hardcodes the reward as `-step.net_electric_power_kw`, ignoring the user-supplied `reward_fn` callback. In the Python `VecDwellingGymEnv.step()` at `vec_env.py:141-150`, when `rust_batch_step` is available, it is called with empty actions and the user's `reward_fn` is never invoked. The Python fallback path (line 151-174) correctly calls `self._reward_fn(ctx)`. This is not directly an action-mapping gap, but it makes the Rust batch path unsuitable for RL training even after action mapping is implemented, because arbitrary reward functions (comfort penalties, demand charge signals, carbon intensity, etc.) cannot be evaluated inside the GIL-free Rayon section.
**Code Location**: `py_gym.rs:122`, `vec_env.py:141-150`
**Root Cause**: Calling a Python callback from a GIL-free Rayon context requires either re-acquiring the GIL per dwelling (defeating parallelism) or computing the reward in Rust (requiring a pre-compiled reward function or post-hoc reward computation). Neither is implemented.
**Impact**: When action mapping is implemented, the Rust batch path will still compute the wrong reward. The Python fallback path must be used for correct rewards, losing the parallelism benefit. A design decision is needed: either accept that the Rust path has a restricted reward, or implement a mechanism for passing reward components back to Python for post-hoc aggregation.

### Finding 8: Existing test coverage is minimal and cannot exercise RL training [Severity: medium]
**Description**: The test file `tests/python/test_gym_env.py` has 6 test functions, of which 2 are `xfail` (load_state not implemented) and the remaining 4 test only shape/dtype/spaces, not functional RL stepping. The action configuration in tests (`_ACTION_CONFIG = {"Gas Furnace": ["heat_c"]}`) exercises only a single control dimension (ThermalSetpoint, heating setpoint only). No test runs:
- Multi-equipment action vectors (battery + HVAC + EV + water heater simultaneously)
- Random exploration across the full action space
- Boundary values (min, max, midpoint of each dimension)
- Invalid action injection and recovery
- 100-step episodes
- The Rust batch_step path (tests use Python fallback only)
**Code Location**: `tests/python/test_gym_env.py:34-35, 70-172`
**Root Cause**: Tests were written before the action mapping was implemented and focus on environment plumbing verification.
**Impact**: Once action mapping is implemented, there is no regression test to verify that random RL exploration does not crash the simulation. A 100-step random-policy test is essential.

## Summary
- Total findings: 8
- Critical: 1 (entire action mapping pipeline is stubbed)
- High: 3 (no action clipping, no cross-field constraint validation, inconsistent equipment-level numeric validation)
- Medium: 4 (incomplete field bounds, no discretization strategy, Rust reward bypass, minimal test coverage)

## Recommendations

### 1. Implement action-to-ControlSignal mapping in `py_gym.rs`
Replace `py_gym.rs:68-73` with an action mapping function that:
- Parses `Vec<f64>` actions into `Vec<ControlSignal>` per dwelling
- Uses the same `_action_layout` pattern (equipment → field mapping) as `gym_env.py`
- Clips each field to its valid range using the same `_field_bounds()` logic
- Constructs `ControlSignal` variants and calls `dwelling.apply_control()` before `step_core_string()`
- Returns a descriptive error (not a panic) on invalid actions

### 2. Add action clipping in Python step functions
Insert `np.clip(action, low, high)` at the top of `_apply_action()` (`gym_env.py:390`) and `_apply_controls()` (`vec_env.py:99`), using the action space bounds already defined at initialization.

### 3. Add numeric range validation to `ControlSignal`
Implement `ControlSignal::validate_numeric_bounds() -> Result<()>` in `crates/hares-types/src/control_signal.rs` with per-variant range checks:
- `ThermalSetpoint`: heating `(10, 30)`, cooling `(15, 35)`, deadband `(0.5, 5)`, heating + deadband < cooling
- `PowerSetpoint`: `active_power_kw` within equipment rated power
- `SOCTarget`: `(0, 1)`, ordering `min_soc < target_soc < max_soc`
- `LoadFraction`/`DutyCycle.on_fraction`: `(0, 1)`
- `MaxCapacityFraction`: `(0, 1)`
- `HumiditySetpoint`: `target_rh` in `(0, 1)`
- `CurtailmentPercent`: `(0, 100)`
- `PowerFactorSetpoint`: `(0, 1)`
- `EvDrive.kwh`/`EvAwayCharge.power_kw`: non-negative
- `EventDelay.delay_s`: non-negative
- `EvSetReadyBy.target_soc`: `(0, 1)`, `departure_hour`: `(0, 24)`

Call this from the `Equipment::apply_control()` trait boundary at `hares-equipment/src/lib.rs:128`.

### 4. Enforce cross-field constraints in equipment
- In `thermostat.rs:448-457`: after setting `RuntimeSetpointOverride`, verify `cooling_c > heating_c + deadband_c`. If violated, reject the signal or auto-correct (e.g., set cooling = heating + deadband + 1°C).
- In `battery/mod.rs:1210-1222`: verify `min_soc < target_soc < max_soc` ordering, defaulting missing bounds to reasonable values (min=0.1, max=0.9).

### 5. Define a discretization strategy
For the Gymnasium action space, document and implement one of:
- **Threshold discretization**: For boolean fields, `action > 0.5 → true`. Already partially implemented in `_build_control_signal()` (`gym_env.py:309, 314`). Extend to all boolean fields.
- **Integer discretization via Discrete space**: For mode/level fields (OperatingMode, DRLevel, InverterPriority, DutyCycleComponent), define a `gymnasium.spaces.Discrete` sub-space with the correct number of categories. This requires a composite `spaces.Dict` or `spaces.Tuple` action space.
- **Rounding**: For integer-constrained fields, `round(action)` to nearest valid integer.

### 6. Fix `_field_bounds()` coverage
In `gym_env.py:181-191`, add bounds for currently unclassified fields:
- `active_power_kw`, `kw`, `p_setpoint_kw`: `(-50.0, 50.0)` kW (conservative residential bound)
- `reactive_power_kvar`: `(-50.0, 50.0)` kVAR
- `max_power_kw`: `(0.0, 50.0)` kW
- `ramp_rate_kw_per_s`: `(0.0, 10.0)` kW/s
- `capacity_w`: `(0.0, 50000.0)` W
- `departure_hour`: `(0.0, 24.0)`
- `delay_s`: `(0.0, 86400.0)` (max 24h)

### 7. Design reward function bridge for Rust batch path
Options:
- **Option A (post-hoc)**: Return `RewardContext` components from Rust (net power, zone temps, comfort violations) as a dict, and have Python compute the reward after batch_step returns. This is the least invasive.
- **Option B (callback in GIL)**: Re-acquire GIL per dwelling batch, call `reward_fn`. Accept serialization overhead.
- **Option C (declarative reward)**: Let users define rewards as algebraic expressions of telemetry fields, evaluated in Rust. This is the most work but provides full parallelism.

### 8. Implement integration test
Add to `tests/python/test_gym_env.py`:
```python
def test_random_policy_hundred_steps_no_panic():
    """Run a random policy for 100 steps without panicking.
    Tests each action dimension at min, max, and midpoint."""
    env = _make_env(episode_length=timedelta(hours=2))
    env.reset(seed=0)
    low = env.action_space.low
    high = env.action_space.high
    mid = (low + high) / 2.0
    rng = np.random.default_rng(42)
    for t in range(100):
        # Cycle through min, mid, max, and random actions
        if t % 4 == 0:
            action = low.copy()
        elif t % 4 == 1:
            action = high.copy()
        elif t % 4 == 2:
            action = mid.copy()
        else:
            action = rng.uniform(low, high).astype(np.float64)
        obs, reward, terminated, truncated, info = env.step(action)
        assert obs.shape == env.observation_space.shape
        assert np.isfinite(obs).all(), f"NaN/Inf in obs at step {t}"
        assert np.isfinite(reward), f"NaN/Inf reward at step {t}"
        assert not terminated, f"unexpected termination at step {t}"
```

## Dependencies and ordering
1. `ControlSignal::validate_numeric_bounds()` must be implemented and called from `Equipment::apply_control()` before action mapping can be safely tested (Finding 3).
2. `_field_bounds()` must be made complete before action clipping can be correct (Finding 6).
3. Action clipping in Python `_apply_action()`/`_apply_controls()` should be implemented first as a defense-in-depth measure (Finding 2).
4. The Rust action mapping in `py_gym.rs` (Finding 1) depends on 1, 2, and 3 above.
5. Cross-field constraint validation (Finding 4) depends on 1 but is a refinement.
6. Discretization strategy (Finding 5) is a design decision that affects the action space type (Box vs. Tuple/Dict).
7. The reward bridge (Finding 7) is a separate concern from action mapping but must be solved for usable RL training with the Rust path.
8. Integration test (Finding 8) should be the final deliverable, running after all other fixes.

## References / Citations
- `crates/hares-python/src/py_gym.rs:68-73` — PyNotImplementedError gate
- `crates/hares-types/src/control_signal.rs:37-146` — ControlSignal enum (25 variants)
- `python/ochre_next/rl/gym_env.py:181-191` — `_field_bounds()`
- `python/ochre_next/rl/gym_env.py:263-316` — `_build_control_signal()` (9 signal types)
- `python/ochre_next/rl/gym_env.py:390-400` — `_apply_action()` (no clipping)
- `python/ochre_next/rl/vec_env.py:99-108` — `_apply_controls()` (no clipping)
- `crates/hares-equipment/src/battery/mod.rs:1201-1249` — battery apply_control_unchecked (minimal validation)
- `crates/hares-equipment/src/ev/mod.rs:880-1009` — EV apply_control_unchecked (thorough validation, reference pattern)
- `crates/hares-equipment/src/hvac/thermostat.rs:445-479` — thermostat setpoint application (no cross-field check)
- `crates/hares-equipment/src/hvac/helpers.rs:176-217` — HVAC apply_heating_control_unchecked (deadband only)
- `crates/hares-equipment/src/hvac/hvac_core.rs:665-672` — HvacEquipment::apply_control_signal (setpoint + capacity fraction clamp)
- `crates/hares-python/src/py_actor.rs:198-256` — PySignal enum (15 variants, 10 ControlSignal variants missing)
- `tests/python/test_gym_env.py:34-35` — minimal action config: single heating setpoint
- `crates/hares-equipment/src/lib.rs:122-129` — Equipment trait boundary (capability validation only, no range validation)
