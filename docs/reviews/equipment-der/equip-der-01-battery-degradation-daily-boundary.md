# Battery degradation daily update at midnight boundary
**Review ID**: equip-der-01
**Category**: equipment-der
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-equipment/src/battery/degradation.rs`
- `crates/hares-equipment/src/battery/mod.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Equipment/Battery.py`

## Findings

### Finding 1: [Severity: high]
**Description**: The per-step degradation accumulation (`accumulate()`) and rainflow SOC push (`rainflow.push()`) are executed *before* the day-boundary check in `Battery::step()`. This causes the first timestep of a new simulation day to contaminate the previous day's degradation state, and the new day loses that first timestep's accumulation entirely.

**Code Location**: `crates/hares-equipment/src/battery/mod.rs:1017-1047`

**Root Cause**: The `step()` method places the rainflow push and degradation accumulate call site *above* the daily-update gate:

```rust
// Line 1017: SOC of new day's first step pushed before boundary check
self.rainflow.push(self.soc);

// Lines 1023-1028: Accumulation for new day's first step runs before boundary check
{
    let cell_temp_k = self.cell_temp_c + 273.15;
    let v_oc_before = self.ocv_table.voltage_at_soc(soc_before);
    self.degradation
        .accumulate(dt_s, cell_temp_k, v_oc_before, soc_before);
}

// Lines 1031-1047: NOW the day boundary is detected and previous day's
// degradation is computed — using accumulators that have already been
// incremented by the new day's first timestep.
let current_day = Self::day_ordinal(env);
if current_day != self.last_daily_update_day {
    let sum_sq_dod = self.rainflow.sum_squared_dod_daily();      // includes cycles from line 1017
    self.degradation.update_daily(..., sum_sq_dod);              // uses contaminated accumulators
    // ...
    self.degradation.reset_day_tracking(self.soc);
    self.rainflow.reset_daily();
    self.last_daily_update_day = current_day;
}
```

Consequences of the ordering:

1. **b1/b2/b3 accumulators**: The first timestep of day N+1 is accumulated into b1_accum/b2_accum/b3_accum *before* `update_daily()` processes day N. Day N's degradation update therefore sees `(day N's accumulation) + (1 timestep from day N+1)`. After `update_daily()` resets accumulators to zero (`degradation.rs:341-343`), the remaining timesteps of day N+1 accumulate normally, but the first timestep's contribution is lost. This results in a persistent 1/N_steps drift (e.g., ~0.35% for 288 steps/day).

2. **Rainflow DOD sum**: `rainflow.push()` at line 1017 may trigger a reversal and cycle extraction if the new day's SOC point changes direction. Any newly extracted cycle DOD is added to `daily_cycle_dods` and read by `sum_squared_dod_daily()` at line 1034, which feeds into the previous day's `update_daily()`. The cycle is correctly counted in total, but attributed to the wrong day.

3. **Tafel correction temperature**: `update_daily()` at line 1035 is called with `cell_temp_c + 273.15` from the *current* (new day's) cell temperature, which is used for the Tafel correction via `t_day` in the `tafel_b1` computation (`degradation.rs:287`). The previous day's degradation should use a representative temperature for that day, not the temperature at the first timestep of the following day.

**OCHRE comparison**: In `vendors/OCHRE/ochre/Equipment/Battery.py:315-346`, OCHRE executes the degradation update (`calculate_degradation()` at line 318) *before* appending the current timestep's SOC/temperature to the degradation data buffer (`degradation_data.append()` at line 346). This ensures the midnight timestep belongs to the *next* day's degradation window. HARES inverts this ordering.

**Impact**: The error in calendar degradation (mechanism 1) per day is approximately `1 / steps_per_day` of the daily increment. For a 5-minute timestep (288 steps/day), this is ~0.35%. For cycle degradation (mechanism 2), the DOD sum error depends on whether a cycle straddles midnight, which varies by control strategy but is typically infrequent. Over multi-year simulations the systematic bias in mechanism 1 could accumulate to a non-negligible calendar aging error. The mechanism 3 BOL transient is also affected. All three mechanisms are impacted because the accumulation-ordering bug affects `b1_accum`, `b2_accum`, and `b3_accum` identically.

### Finding 2: [Severity: medium]
**Description**: `reset_daily()` correctly preserves the reversal buffer but clears the daily DOD list. However, no test verifies that a half-cycle started before midnight survives the reset and is correctly attributed to the appropriate day's degradation.

**Code Location**: 
- `crates/hares-equipment/src/battery/degradation.rs:115-120` (reset_daily implementation)
- `crates/hares-equipment/src/battery/degradation.rs:349-881` (test module — no midnight straddling test)

**Root Cause**: The `reset_daily()` method (`degradation.rs:115-120`) has an explicit comment documenting the intent:

```rust
pub(crate) fn reset_daily(&mut self) {
    // Keep the full reversal buffer. Residential charge/discharge cycles can
    // straddle midnight, so discarding the buffer would lose partial cycles.
    // Clear daily DOD list; it is consumed by the degradation model at midnight.
    self.daily_cycle_dods.clear();
}
```

This design is correct — the `reversals` buffer and `cycle_count` are preserved. However, there is no test that exercises this behavior. A test should:

1. Push a sequence that starts a charging ramp just before midnight (e.g., `[0.2, 0.5, 0.8]`)
2. Call `reset_daily()` (simulating midnight)
3. Continue the sequence after midnight (e.g., `[0.6, 0.3]`)
4. Verify that the complete cycle (DOD ≈ 0.5) is extracted after midnight and attributed to the next day's `sum_squared_dod_daily()`
5. Also verify that any half-cycle extracted *before* midnight (in step 1) is attributed to the pre-midnight day's DOD sum

**OCHRE comparison**: OCHRE `calculate_degradation()` (line 365-441) accumulates all SOC/temperature data in a Python list and runs `rainflow.extract_cycles()` on the entire accumulated buffer at midnight, then clears it. A half-cycle that started at 23:55 and completed at 00:05 is split: the 23:55 point is in day N's degradation data, the 00:00 and 00:05 points are in day N+1's data. The rainflow library may not extract the cycle correctly from split data. HARES's incremental approach with a preserved reversal buffer is architecturally superior for this case, but the behavior is unverified by tests.

**Impact**: Medium — the code structure is architecturally correct, but without test coverage, future refactors could accidentally clear the reversal buffer or change the reset semantics, causing partial cycle loss at midnight crossings. Residential PV self-consumption micro-cycles commonly straddle midnight (e.g., evening discharge transitioning to morning charge), making this a real operational scenario.

### Finding 3: [Severity: low]
**Description**: `DegradationState::update_daily()` is only called at the day boundary (`mod.rs:1035`), and `capacity_fade` is only computed within `update_daily()` (`degradation.rs:338`). The `accumulate()` method called every timestep (`mod.rs:1027`) only updates per-day tracking fields (`b1_accum`, `b2_accum`, `b3_accum`, SOC extremes) and never modifies `q_li1`, `q_li2`, `q_li3`, or `capacity_fade`. However, the `capacity_fade_fraction()` value is reported to telemetry every timestep (`mod.rs:1067`) — it simply returns a *stale* (previous day's) value between daily updates. While this is behaviorally correct (no premature aging), the telemetry field name `CAPACITY_FADE_PCT` may mislead consumers into thinking it is a live value updated every step.

**Code Location**:
- `crates/hares-equipment/src/battery/mod.rs:1065-1068` (telemetry write)
- `crates/hares-equipment/src/battery/degradation.rs:216-218` (capacity_fade_fraction getter)

**Root Cause**: The degradation design intentionally gates all lithium-loss updates behind `update_daily()`, which is correct per the Smith 2017 calendar model. However, no documentation or telemetry field metadata indicates that the value is "daily" or "stale between midnight updates."

**OCHRE comparison**: OCHRE `calculate_power_and_heat()` (line 317) gates degradation behind `self.current_time.time() == dt.time(0, 0)` — the same daily cadence. OCHRE reports `Nominal Capacity (kWh)` and degradation states only at verbosity level 7+ (line 448-454), and these are written at each timestep using whatever value was set at the last midnight update. OCHRE exhibits the same "stale-between-updates" behavior.

**Impact**: Low — the telemetry value reflects the most recent degradation state correctly. No premature aging occurs. The concern is documentation, not correctness.

## Summary
- **Total findings**: 3
- **Critical**: 0
- **High**: 1
- **Medium**: 1
- **Low**: 1

## Recommendations

1. **Restructure the midnight boundary ordering in `Battery::step()` (Finding 1)**:
   Move the day-boundary check and daily degradation update to *precede* the per-step rainflow push and degradation accumulation for the current timestep. The corrected ordering should be:
   ```
   (a) Check day boundary
   (b) If day changed: compute sum_sq_dod, call update_daily(), reset tracking, advance last_daily_update_day
   (c) Push SOC to rainflow (for current step, which belongs to the appropriate day)
   (d) Accumulate degradation terms (for current step)
   ```
   This mirrors OCHRE's ordering where `calculate_degradation()` runs before `degradation_data.append()`. The `cell_temp_k` passed to `update_daily()` should also be captured from a representative daily value rather than the current (first-of-new-day) temperature.

2. **Add a midnight-boundary straddling test for `RainflowCounter::reset_daily()` (Finding 2)**:
   Add a test in `degradation.rs` that:
   - Pushes a partial reversal sequence (e.g., `[0.2, 0.5, 0.8]`)
   - Verifies pre-reset `sum_squared_dod_daily()` if a half-cycle was extracted
   - Calls `reset_daily()`
   - Verifies `reversals` buffer is preserved (`reversals.len() > 0`)
   - Verifies `sum_squared_dod_daily()` is zero after reset
   - Continues the SOC sequence post-midnight (`[0.6, 0.3]`)
   - Verifies the completed cycle appears in the new day's `sum_squared_dod_daily()`

3. **Document the daily cadence of capacity fade (Finding 3)**:
   Add a comment on the `CAPACITY_FADE_PCT` telemetry field description in `battery_telemetry_fields()` (`mod.rs:1374-1377`) noting that the value is updated once per day at midnight and reflects the previous day's cumulative degradation.

## References / Citations
- Smith, K., et al. "Life prediction model for grid-connected Li-ion battery energy storage system." *IEEE Transactions on Industry Applications* (2017), IEEE 7963578.
- ASTM E1049-85 (2017), "Standard Practices for Cycle Counting in Fatigue Analysis."
- Schimpe, M., et al. "Comprehensive Modeling of Temperature-Dependent Degradation Mechanisms in Lithium Iron Phosphate Batteries." *NREL/TP-5400-70616* (2018).
- Schmalstieg, J. et al. "A holistic aging model for Li(NiMnCo)O2 based 18650 lithium-ion batteries." *Journal of Power Sources* 257 (2014): 325-334.
- Xu, B. et al. "Modeling of lithium-ion battery degradation for cell life assessment." *IEEE Transactions on Smart Grid* 9.2 (2018): 1131-1140.
- OCHRE v1 Battery model: `vendors/OCHRE/ochre/Equipment/Battery.py`
