# EV degradation model sharing Battery degradation pipeline
**Review ID**: equip-der-04
**Category**: equipment-der
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-equipment/src/ev/mod.rs`
- `crates/hares-equipment/src/battery/degradation.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Equipment/EV.py`

## Findings

### Finding 1: [Severity: medium]
**Description**: EV does not apply SOH degradation to its effective battery capacity. The Battery model updates `capacity_kwh_nominal = capacity_kwh_rated * SOH` at every day boundary (`battery/mod.rs:1037-1038`), so the Battery's SOC tracking accounts for capacity loss. The EV never reduces its `battery_capacity_kwh` field—the field is held constant from initialization. SOC updates during driving (`ev/mod.rs:973`) and charging (`ev/mod.rs:647`) use the original capacity divisor, causing the EV's reported SOC to drift relative to the physically degraded cell. After 10% capacity fade the EV would report 1 Wh of usable capacity where the degraded cell only holds 0.9 Wh.
**Code Location**: `crates/hares-equipment/src/ev/mod.rs:577-596` (update_degradation method, missing SOH→capacity link), contrasted with `crates/hares-equipment/src/battery/mod.rs:1037-1038` (Battery's working SOH linkage)
**Root Cause**: The `update_degradation` method calls `update_daily` which computes `capacity_fade` (line 591), but the result is never fed back into `self.battery_capacity_kwh`. The `capacity_fade_pct` telemetry field is emitted (`ev/mod.rs:567`), and the LUT lookup uses `soh` for the charging curve (`ev/mod.rs:421`), but the core SOC-accounting arithmetic is untouched.
**Impact**: Over multi-year simulations the EV's SOC diverges from physical reality. For a battery with 15% capacity fade and an originally 80 kWh pack, a 10 kWh driving trip moves SOC by 10/80 = 12.5% instead of the physically correct 10/(80×0.85) = 14.7%. The degradation metrics are correctly tracked, but the simulation's runtime behavior (SOC, range, charging duration) does not reflect the degraded state of the pack.

### Finding 2: [Severity: medium]
**Description**: Pre-step vs post-step SOC inconsistency in degradation `accumulate()` calls. The Battery explicitly saves `soc_before = self.soc` before applying charge/discharge deltas, then passes `soc_before` and `v_oc_before` to `accumulate()` with the rationale: "degradation sees the SOC the cell was at *before* the charge/discharge delta, matching the physical voltage the cell experienced during the interval" (`battery/mod.rs:1019-1027`). The EV calls `update_degradation` *after* `apply_soc_and_thermal()` has already updated `self.soc`, so `accumulate()` receives post-step SOC and post-step OCV for mechanism 3's Tafel correction (`ev/mod.rs:577-582`).
**Code Location**:
- Battery correct pattern: `crates/hares-equipment/src/battery/mod.rs:1019-1027`
- EV inconsistent pattern: `crates/hares-equipment/src/ev/mod.rs:577-582`
**Root Cause**: The EV's `step()` method calls `apply_soc_and_thermal()` (which mutates `self.soc`) before `update_degradation()` is reached at line 745. The degradation accumulation sees the SOC *after* the timestep's energy transfer, while the Battery deliberately preserves the pre-step snapshot.
**Impact**: The mechanism 3 (BOL transient) Tafel term `exp(α_b3·F/R · (V_oc/T − V_ref/T_ref))` is evaluated at a voltage that corresponds to the post-step SOC rather than the average voltage over the interval. Over a single 5-minute timestep the SOC delta is small (sub-0.1%), so the error is negligible in practice, but the model intent diverges between the two equipment types.

### Finding 3: [Severity: low]
**Description**: Day-0 off-by-one in the discrete integrator for calendar aging (mechanism 1). In `degradation.rs:293-300`, when `day_age == 0` and `q_li1 == 0`, the `dq_li1` computation hits the `else { 0.0 }` branch, skipping the first day's calendar aging contribution entirely. The first non-zero increment occurs at `day_age == 1` (the second midnight). This means the cumulative `q_li1` is always one day behind the analytic `b1_eff * sqrt(t_day)` curve.
**Code Location**: `crates/hares-equipment/src/battery/degradation.rs:293-300`
**Root Cause**: The branch structure uses `day_age` (pre-increment) in the condition `self.day_age > 0`, but `day_age` starts at 0 and only reaches 1 *after* the first `update_daily` completes. The `abs() < 1e-5` check on `q_li1` is also true on day 0. Both conditions together route to the no-op branch.
**Impact**: The first sim-day (0→1 boundary) contributes zero calendar aging. For a 365-day simulation the impact is ~1/365 ≈ 0.3% of the calendar contribution. Negligible for multi-year simulations but introduces a small systematic under-prediction of SEI growth. Affects both EV and Battery equally since they share the same `DegradationState`.

### Finding 4: [Severity: low]
**Description**: No degradation-specific tests in the EV test suite. The only EV degradation tests check initialization (`degradation_starts_at_zero`), telemetry presence (`capacity_fade_fraction_in_telemetry`), and checkpoint survival (`degradation_state_survives_checkpoint`)—none validate that the shared pipeline produces correct degradation when fed EV-specific SOC trajectories (deep DoD driving cycles, multi-day idle periods, away charging sessions). In contrast, `degradation.rs` has extensive physics tests covering Arrhenius factors, cycling aging, calendar aging, and multi-day accumulation (`degradation.rs:349-1099`).
**Code Location**: `crates/hares-equipment/src/ev/tests.rs:949-980`
**Root Cause**: The tests validate that the data structures exist and survive serialization but not that the model produces physically correct output for EV patterns. This is a testing gap rather than a code defect.
**Impact**: No automated verification that an EV completing a 60% DOD drive-then-charge cycle degrades at a rate consistent with the Smith 2017 model. The shared `degradation.rs` unit tests mitigate this somewhat since they test the pipeline in isolation, but EV-specific integration tests are absent.

### Finding 5: [Severity: low]
**Description**: OCHRE's EV.py contains no battery degradation model whatsoever. The `ElectricVehicle` class handles SOC tracking, charging power, and event-based scheduling but never computes capacity fade. The comment at `vendors/OCHRE/ochre/Equipment/EV.py:291` ("this is copied from the battery model, but they are not linked at all") refers to the power calculation, not degradation—no degradation pipeline exists in OCHRE's EV. HARES has added degradation tracking that OCHRE lacks, which is a net improvement. However, there is no vendor reference implementation to validate the EV-specific degradation wiring against.
**Code Location**: `vendors/OCHRE/ochre/Equipment/EV.py:1-371` (entire file, no degradation)
**Root Cause**: OCHRE's EV was designed as a simple charging model; battery degradation for EVs was not in its scope.
**Impact**: Low (informational). HARES's EV degradation model is an extension beyond the vendor reference and appears physically sound for EV cycling patterns (the rainflow counter captures deep DoD half-cycles, and the DOD² weighting correctly differentiates EV deep-cycling from stationary battery micro-cycling). Cross-validation against published EV degradation literature would strengthen confidence.

## Summary
- Total findings: 5
- Critical: 0 / High: 0 / Medium: 2 / Low: 3

## Assessment per Concern

### Rainflow counter for EV deep DoD cycles
**Verdict: Passes.** The rainflow counter correctly captures large-DoD half-cycles from EV driving-then-charging patterns. The 3-point algorithm with persistent reversal buffer handles multi-day idle periods correctly (SOC doesn't change during idle, so no spurious reversals are generated). The ASTM E1049-85 half-cycle counting with DOD² damage weighting is physically sound for differentiating EV deep cycles from stationary battery micro-cycles.

### Smith2017 cycle-life curve at correct DoD values
**Verdict: Passes.** The mechanism 2 formula `dq_li2 = B2_REF * b2_accum * sqrt(Σ count_i × DOD_i²)` uses DOD values directly without extrapolation. Smith 2017 fits DOD up to 100%, and typical EV day-level DODs (40-80%) fall within the fitted range. The DOD² weighting via the rainflow counter's `sum_squared_dod_daily()` ensures that an EV's single 80% DOD cycle (0.64 damage units) is weighted more heavily than many small cycles—physically matching the convex relationship between DOD and cycle-life degradation.

### Calendar aging during EV idle periods
**Verdict: Passes with minor note.** The EV calls `update_degradation` every timestep regardless of connection state (`ev/mod.rs:745`). During `Disconnected` periods (vehicle parked/not charging), SOC is stable (no power flow at line 738-741) and only ambient-driven thermal drift occurs. Mechanism 1 (calendar SEI) accumulates based on temperature via Arrhenius, while mechanism 2 (cycling) contributes zero because no cycles are extracted. This is the correct physical behavior—calendar aging continues regardless of whether the vehicle is driven. No differentiation from the stationary battery is needed since the same physics governs both.

## Recommendations

1. **Wire SOH into EV effective capacity.** After `update_daily`, apply the same pattern as Battery: `self.battery_capacity_kwh = initial_capacity * (1.0 - self.degradation.capacity_fade_fraction())`. Store the initial (rated) capacity separately as `battery_capacity_kwh_rated` as Battery does.

2. **Align EV degradation accumulation with Battery's pre-step SOC pattern.** Capture `soc_before = self.soc` before `apply_soc_and_thermal()`, then pass `soc_before` (and `v_oc_before = ocv_table.voltage_at_soc(soc_before)`) to `accumulate()`. Keep the rainflow push using post-step SOC since rainflow tracks the endpoint trajectory.

3. **Consider a rate-limited SOH update** to avoid coupling the degradation's daily cadence to every timestep capacity calculation. The Battery pattern (daily SOH recompute) is appropriate for both equipment types.

4. **Add an EV-specific integration test** that simulates a multi-day driving pattern (e.g., day 1: drive 60% DOD, charge to full; day 2: idle; day 3: drive 40% DOD) and asserts that the cumulative capacity fade matches predictions from the Smith 2017 analytic formulas, and that the rainflow counter extracts the expected number of half-cycles with correct DOD values.

5. **Fix the day-0 calendar aging off-by-one** in `degradation.rs:293-300` by using `self.day_age.max(1)` in the denominator or initializing `day_age` to 1 instead of 0.

## References / Citations

- Smith, K., Saxon, A., Keyser, M., Lundstrom, B., Cao, Z., & Roc, A. (2017). "Life prediction model for grid-connected Li-ion battery energy storage system." *2017 IEEE 7963578*. American Control Conference (ACC), Seattle, WA.
- ASTM E1049-85 (2017). "Standard Practices for Cycle Counting in Fatigue Analysis." ASTM International.
- Schmalstieg, J., et al. (2014). "A holistic aging model for Li(NiMnCo)O2 based 18650 lithium-ion batteries." *Journal of Power Sources*, 257, 325-334.
- OCHRE (`vendors/OCHRE/ochre/Equipment/EV.py`): No degradation model present in vendor reference; HARES degradation model is a superset.
