# Heat pump defrost cycle tracker edge cases
**Review ID**: equip-hvac-03
**Category**: equipment-hvac
**Date**: 2026-05-26

## Files Reviewed
crates/hares-equipment/src/hvac/heat_pump/defrost.rs crates/hares-equipment/src/hvac/heat_pump/heater.rs crates/hares-equipment/src/hvac/heat_pump.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/Equipment/HVAC.py

## Findings
### Finding 1: [Severity: high] No frost decay/reset during extended off-periods — immediate defrost after shoulder season
**Description**: The `DefrostCycleTracker` preserves `accumulated_frost_s` across HP off-cycles with no time-based decay or OAT-threshold reset. After a week of spring/summer inactivity (no heating call, outdoor coil warm), the first heating call triggers immediate defrost because the accumulator still holds its frozen value from the last heating season.
**Code Location**: `defrost.rs:269-283` (`advance` method); `defrost.rs:289-291` (reset only on Defrosting→Accumulating transition)
**Root Cause**: The `advance` method in the Accumulating state only has one update path — increment frost when conditions favor frost (line 274); if conditions do not favor frost (line 273 guard fails), `accumulated_frost_s` is left unchanged, never decayed. The only reset paths are via the Defrosting→Accumulating transition (line 290) or initial construction (line 253). There is no mechanism that decays accumulated frost based on elapsed real time or OAT-above-freezing duration.
**Impact**: First heating call after a seasonal hiatus spuriously enters defrost mode, reporting zero zone capacity for ~210 s (ReverseCycle) or drawing resistive defrost power. This distorts seasonal energy use and peak power estimates. OCHRE avoids this entirely because `HeatPumpHeater.update_capacity()` (HVAC.py:1141) uses a simple `self.defrost = t_ext_db < 4.4445` — no persistent accumulator state exists to go stale.

### Finding 2: [Severity: medium] Step-local interval calculation breaks long-run defrost fraction when OAT oscillates around threshold
**Description**: The inter-defrost interval formula `interval_s = cycle_duration_s / time_fraction` at `defrost.rs:278` uses the *current step's* `time_fraction`, not a running average. When OAT oscillates around `DEFROST_ENABLE_TEMP_C` (4.4445°C), the continuously-varying `time_fraction` causes `interval_s` to vary widely step-to-step, and accumulated frost that was gathered under one `time_fraction` regime triggers defrost under a different regime's interval threshold. The long-run average time-in-defrost may diverge from the continuous model's predicted fraction.
**Code Location**: `defrost.rs:273-283`
**Root Cause**: The derivation of `interval_s` (line 278) assumes constant `time_fraction` across the entire accumulation period, which holds only when weather conditions are steady. Under oscillating OAT, `time_fraction` from `evaluate_defrost` fluctuates as coil temperature, delta-humidity, and the activation threshold interact, but the accumulated frost `dt_s * time_fraction` was collected under potentially different `time_fraction` values.
**Impact**: In mild-climate ramping seasons where outdoor temperature frequently crosses 4.5°C, the discrete FSM's effective defrost duty cycle drifts from the intended continuous-model average. OCHRE has no equivalent issue since each step independently computes defrost penalties from instantaneous ambient conditions (HVAC.py:1141-1166).

### Finding 3: [Severity: medium] No defrost activation hysteresis in continuous evaluation — rapid toggle at threshold
**Description**: `evaluate_defrost()` at `defrost.rs:369-374` returns inactive when `outdoor_db_c >= config.max_oat_defrost_c` (default 4.4445°C). There is no hysteresis band. When outdoor temperature oscillates within ±0.5°C of the threshold, the continuous model rapidly alternates between active and inactive, causing the `DefrostCycleTracker` Accumulating phase to jerkily start/stop accumulation. OCHRE has the identical behavior (HVAC.py:1141: `self.defrost = t_ext_db < 4.4445`), so this is parity behavior, but both models share the deficiency.
**Code Location**: `defrost.rs:369-374`; OCHRE `HVAC.py:1141`
**Root Cause**: A single hard threshold with zero hysteresis. The discrete FSM's internal state provides partial mitigation (once in Defrosting, the cycle runs to completion regardless of OAT at lines 285-293), but the Accumulating phase has no such protection.
**Impact**: In borderline-temperature conditions, defrost calculations and telemetry values fluctuate rapidly step-to-step. The discrete FSM's built-in cycle duration damps the most visible effects (defrost events still run their full duration), but the accumulation rate jitter affects the timing of the next defrost trigger.

### Finding 4: [Severity: low] Defrosting timer progresses even when HP is off — state/physics mismatch
**Description**: In the `DefrostCycleState::Defrosting` arm of `advance()` (lines 285-293), `defrost_elapsed_s` increments unconditionally regardless of whether `conditions_favor_frost` is true or the compressor is actually running. If the thermostat satisfies mid-defrost, the HP shuts off but the timer continues to count. When the HP restarts, the tracker may already have transitioned to Accumulating (frost reset to zero on line 290), even though the physical coil never completed defrost.
**Code Location**: `defrost.rs:285-293`; `heater.rs:1392` (capacity override guard)
**Root Cause**: The state machine assumes time-in-state is always real elapsed time, but HP-off periods during defrost represent time where the defrost cycle is effectively paused, not progressing.
**Impact**: The capacity/power override at `heater.rs:1392` guards against phantom defrost power (`is_discrete_defrosting` requires `hp_on`), so incorrect state does not propagate to zone energy balances. However, telemetry fields `DEFROST_CYCLE_STATE`, `DEFROST_ACCUMULATED_FROST_S`, and `DEFROST_ELAPSED_S` (`heater.rs:1127-1135`) report physically inconsistent values after interrupted defrost cycles. OCHRE has no equivalent issue because it lacks discrete state.

## Summary
- Total findings: 4
- Critical: 0 / High: 1 / Medium: 2 / Low: 1

## Recommendations
1. **Add frost decay during extended off-periods** (Finding 1). When `conditions_favor_frost` is false and the HP is off, decay `accumulated_frost_s` toward zero with a time constant derived from OAT (e.g., exponential decay when OAT > 0°C, no decay when OAT ≤ 0°C). Alternatively, reset `accumulated_frost_s` to zero when OAT exceeds a configurable "frost-clear" temperature (e.g., 7°C) for a configurable duration.

2. **Use a trailing average `time_fraction` for interval calculation** (Finding 2). Compute `interval_s` from an EWMA of `time_fraction` rather than the instantaneous value, so the accumulation/defrost ratio converges to the continuous model's long-run average under variable weather.

3. **Add hysteresis to `evaluate_defrost` activation threshold** (Finding 3). Apply a 0.5–1.0°C hysteresis band around `max_oat_defrost_c` so that once defrost activates, it stays active until OAT rises past `max_oat_defrost_c + hysteresis_c`, and once inactive, stays inactive until OAT falls past `max_oat_defrost_c - hysteresis_c`.

4. **Pause the Defrosting timer when the compressor is off** (Finding 4). In the `Defrosting` arm of `advance()`, only increment `defrost_elapsed_s` when `conditions_favor_frost` is true (i.e., the compressor is actually running). This aligns `defrost_elapsed_s` with actual defrost-cycle runtime.

## References / Citations
- OCHRE `HeatPumpHeater.update_capacity()`: HVAC.py lines 1128–1166 (continuous defrost model — no state, no accumulation, no discrete cycling)
- HARES `DefrostCycleTracker::advance()`: defrost.rs lines 260–303
- HARES `DefrostCycleTracker` construction: defrost.rs lines 247–258
- HARES step integration (pre-step FSM advance): heater.rs lines 969–1009
- HARES discrete defrost capacity/power override: heater.rs lines 1386–1446
- EnergyPlus Engineering Reference §15.2.11.4 (Defrost Operation) — source of the continuous defrost formulas implemented in HARES `evaluate_defrost`
