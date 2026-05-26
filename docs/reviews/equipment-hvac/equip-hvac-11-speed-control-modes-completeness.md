# Speed control modes completeness: staging, variable-speed, coil selection
**Review ID**: equip-hvac-11
**Category**: equipment-hvac
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-equipment/src/hvac/speed_control.rs`
- `crates/hares-equipment/src/hvac/staging.rs`
- `crates/hares-equipment/src/hvac/heat_pump_config.rs`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Equipment/HVAC.py`

## Findings
### Finding 1: [Severity: medium] VariableSpeedIdeal and MultiSpeedInterpolated share identical speed-stage interpolation — no true ideal-capacity path
**Description**: `VariableSpeedIdeal` delegates to the exact same `select_multi_speed()` method as `MultiSpeedInterpolated` (`staging.rs:118-119`). Both call `interpolate_speed_stages()` (`speed_control.rs:159-204`), producing `SpeedSelection` with `speed_frac` for linear interpolation between adjacent discrete stages. The two modes differ only in side effects: PLF always 1.0 for VariableSpeedIdeal (`staging.rs:257-263`) and startup Cd=0 (`heat_pump_config.rs:437`), but the core speed-selection algorithm is identical.

In OCHRE (`HVAC.py:990-1020`), variable-speed equipment with `n_speeds >= 4` uses the ideal-capacity path — the exact required capacity is solved from the building thermal model (`solve_ideal_capacity`), then mapped to a fractional `speed_idx` via `np.searchsorted` interpolation between stage capacities. HARES `VariableSpeedIdeal` instead receives a pre-computed `load_fraction` and interpolates identically to `MultiSpeedInterpolated`, effectively making it a multi-speed mode with reduced degradation.

**Code Location**: `staging.rs:118-119`, `staging.rs:218-231`, `speed_control.rs:159-204`

**Root Cause**: The `VariableSpeedIdeal` variant was added to the enum (`speed_control.rs:23`) without a distinct capacity-determination algorithm. Its name implies ideal-capacity behavior (solve envelope model for exact capacity), but its implementation is identical to multi-speed interpolation.

**Impact**: No simulation error — both paths produce correct speed interpolation between discrete stages. However, `VariableSpeedIdeal` does not model true variable-speed behavior (continuously modulated capacity solved from thermal demand). For mini-split equipment forced to 4 speeds via `cooling_speed_control_mode` (`heat_pump_config.rs:421-428`), the capacity is bounded to the discrete stage levels with linear interpolation between them, not truly continuous. This may under-represent the load-following capability of inverter-driven compressors.

### Finding 2: [Severity: high] TwoSpeedAlternating lacks minimum-time-per-speed guard, risking oscillation
**Description**: `TwoSpeedAlternating` mode (`staging.rs:98-116`) applies no minimum-time-speed enforcement, unlike `TwoSpeedSetpoint` (`staging.rs:137-164`) and `TwoSpeedTime` (`staging.rs:166-216`), both of which check `time_at_current_speed_s < min_time_per_speed_s` and lock the speed if too little time has elapsed. In OCHRE (`HVAC.py:868-916`), the `run_two_speed_control` method applies the `min_time_in_speed` guard to all two-speed control types (Time, Time2, Setpoint) via the shared check `if self.time_in_speed < self.min_time_in_speed[prev_speed_idx - 1]: speed = prev_speed_idx`.

`TwoSpeedAlternating` is documented as "always runs at high speed when on. Equivalent to cycling high/low each on-event" (`speed_control.rs:17`), which means it toggles each on-event. But after `apply_disabled_speeds_two_speed` redirects the desired index (e.g., when high speed is disabled), the alternating logic could cause rapid switching between enabled speeds without a minimum-dwell constraint.

**Code Location**: `staging.rs:98-116`

**Root Cause**: The minimum-time guard was added to `TwoSpeedSetpoint` and `TwoSpeedTime` but omitted from `TwoSpeedAlternating`.

**Impact**: If an external controller rapidly toggles `disabled_speeds` or if the `TwoSpeedAlternating` pattern interacts with disabled-speed fallback, the equipment could switch speeds faster than the compressor's physical minimum cycle time. May produce unrealistic short-cycling in simulations with dynamic speed disabling.

### Finding 3: [Severity: medium] Three-speed equipment falls to SingleSpeed in cooler control mode derivation
**Description**: `HeatPumpCoolerConfig::cooling_speed_control_mode()` (`heat_pump_config.rs:421-428`) maps speed counts to control modes:
```rust
match n {
    1 => SpeedControlMode::SingleSpeed,
    2 => SpeedControlMode::TwoSpeedSetpoint,
    n if n >= 4 => SpeedControlMode::VariableSpeedIdeal,
    _ => SpeedControlMode::SingleSpeed,   // n == 3 falls here
}
```
Three-speed equipment (uncommon but valid in `MultiSpeedInterpolated` via the `SpeedControlMode` enum) gets classified as `SingleSpeed`, losing multi-stage interpolation capability. OCHRE's `SPEED_TYPES` dictionary only defines keys `{1, 2, 4}` (`HVAC.py:13-18`), and `DynamicHVAC.run_thermostat_control` raises an exception for any `n_speeds` other than 1 or 2 (`HVAC.py:927`). HARES has broader enum support but the cooler derivation function doesn't map 3-speed correctly.

**Code Location**: `heat_pump_config.rs:421-428`

**Root Cause**: The match arms cover `1`, `2`, and `>= 4` but have no explicit arm for `3`; the catch-all `_` arm maps to `SingleSpeed` instead of `MultiSpeedInterpolated`.

**Impact**: A 3-speed ASHP cooler configured with `number_of_speeds: 3` and explicit stage capacities would silently degrade to single-speed operation via the typed config path. Since 3-speed heat pumps are rare (most are 1, 2, or 4-speed), the practical exposure is low.

### Finding 4: [Severity: medium] No heating-side speed control mode derivation on HeatPumpHeaterConfig
**Description**: `HeatPumpCoolerConfig` has `cooling_speed_control_mode()` (`heat_pump_config.rs:421`) and `derived_cooling_startup_cd()` (`heat_pump_config.rs:435`), but `HeatPumpHeaterConfig` has no corresponding `heating_speed_control_mode()` or `derived_heating_startup_cd()` methods. The ASHP heating path must rely on the generic `HvacEquipmentConfig.speed_control_mode` field being set correctly by an upstream init function, without typed-config-based derivation.

**Code Location**: `heat_pump_config.rs:421-443` (cooler methods present), `heat_pump_config.rs:178-371` (heater has no corresponding methods)

**Root Cause**: Asymmetric implementation — the typed config architecture was built out for the cooling path first. The heating path works but lacks the same typed derivation helpers.

**Impact**: Any init code for `HeatPumpHeaterConfig` must manually set `speed_control_mode` on the underlying `HvacEquipment` rather than calling a `heating_speed_control_mode()` method. This creates a maintenance risk where heating and cooling speed modes could diverge (e.g., a 2-speed heater config gets SingleSpeed mode because the init path missed the 2-speed mapping).

### Finding 5: [Severity: low] er_stages config field has no corresponding staged-backup runtime logic
**Description**: `HeatPumpCommonConfig.er_stages` (`heat_pump_config.rs:113`) accepts 1–4 electric-resistance backup stages, validated at `heat_pump_config.rs:360-365`. However, there is no staging logic in `speed_control.rs`, `staging.rs`, or `heat_pump_config.rs` that progressively engages backup stages based on runtime conditions. OCHRE has the same limitation — the `staged_backup` method in `ASHPHeater` is entirely commented out as a TODO (`HVAC.py:1371-1394`).

**Code Location**: `heat_pump_config.rs:113` (declaration), `heat_pump_config.rs:360-365` (validation)

**Root Cause**: Staged backup was deferred as future work in both codebases. The config field exists to accept HPXML/ResStock data but has no runtime effect beyond `er_stages == 1` (binary on/off).

**Impact**: For buildings with multi-stage electric resistance backup (2–3 sequenced strips), the simulation treats all backup as a single element. This may overestimate backup power in mild conditions where only a partial strip would activate. The validation at config load prevents invalid values from reaching runtime, so there's no crash risk — just a fidelity gap.

### Finding 6: [Severity: low] Fresh-cycle detection in TwoSpeedTime relies on prev_zone_temp_c being None after off events
**Description**: `select_two_speed_time` (`staging.rs:172-189`) detects a fresh cycle when either `zone_temp_c` or `prev_zone_temp_c` is `None`. The fresh-cycle branch starts at speed 0 (low speed). However, the caller is responsible for calling `update_prev_zone_temp(None)` when the unit turns off (per the doc comment at `staging.rs:63-66`). If the caller fails to do this, a subsequent on-cycle could inherit the previous `prev_zone_temp_c` and incorrectly classify the initial temperature trend, potentially skipping the low-speed start.

In OCHRE, the fresh-cycle detection is based on `self.mode == "Off"` (`HVAC.py:870`) — the mode is set to "Off" by thermostat control directly, so there's no dependency on a separate update call. OCHRE's approach is more robust because it's state-driven rather than parameter-passing-driven.

**Code Location**: `staging.rs:63-66` (doc comment), `staging.rs:172-173` (fresh-cycle check)

**Root Cause**: The fresh-cycle reset is a side-effect of an external call (`update_prev_zone_temp(None)`) rather than an implicit consequence of the equipment turning off. This is a fragile API contract.

**Impact**: If the caller (thermostat/control layer) fails to call `update_prev_zone_temp(None)` before the next on-cycle, TwoSpeedTime may start at high speed instead of low speed. In practice this only affects the first timestep of a heating/cooling cycle. The impact is minor because the timer guard would prevent speed changes for `min_time_per_speed_s` anyway.

### Finding 7: [Severity: low] capacity_fractions_for returns empty Vec when maximum capacity is zero, but callers may not handle empty
**Description**: `capacity_fractions_for` (`speed_control.rs:137-143`) returns an empty `Vec` when `max_cap <= 0.0`. `interpolate_speed_stages` (`speed_control.rs:159-204`) handles empty input gracefully (returns zero `SpeedSelection`). However, `select_multi_speed` (`staging.rs:218-231`) calls `capacity_fractions_for` and passes the result to `interpolate_speed_stages` — this is safe. `capacity_fractions` (`staging.rs:351-367`) also returns the result of `capacity_fractions_for` and callers could iterate over an empty `Vec`. While no crash path exists, the silent zero-capacity behavior makes it difficult to diagnose misconfigured equipment (zero or negative rated capacities in the input file).

**Code Location**: `speed_control.rs:137-143`

**Root Cause**: Defensive coding that prevents division-by-zero or NaN propagation but leaves no diagnostic trace when equipment has zero rated capacity.

**Impact**: Equipment with accidentally zero rated capacity would produce zero output without any warning or error at the speed-selection level. Validation at config load (e.g., `heat_pump_config.rs:247-249`) catches some cases but the `capacities_w` vectors come from a different configuration path and may not be validated.

## Summary
- Total findings: 7
- High: 1 (TwoSpeedAlternating missing minimum-time guard)
- Medium: 3 (VariableSpeedIdeal/MultiSpeedInterpolated convergence, 3-speed mapping gap, missing heating-side mode derivation)
- Low: 3 (er_stages orphaned config, fragile fresh-cycle reset, silent zero-capacity edge case)

## Recommendations
1. **Add minimum-time-per-speed guard to TwoSpeedAlternating** (`staging.rs:98-116`). Apply the same `time_at_current_speed_s < min_time_per_speed_s` lock that exists for TwoSpeedSetpoint and TwoSpeedTime. Match OCHRE's unified guard in `run_two_speed_control` (`HVAC.py:903-904`).

2. **Map 3-speed equipment to MultiSpeedInterpolated** in `cooling_speed_control_mode()` (`heat_pump_config.rs:426`). Change the catch-all arm from `SpeedControlMode::SingleSpeed` to `SpeedControlMode::MultiSpeedInterpolated` to correctly handle `n == 3`.

3. **Add `heating_speed_control_mode()` to `HeatPumpHeaterConfig`** symmetric with the cooler path, or document that heating speed mode is always set by the generic init path. If the generic path is the intended design, remove the asymmetry to avoid maintenance confusion.

4. **Implement or deprecate `er_stages` staged backup**. Either add progressive ER staging logic (time-sequenced strip activation matching ecobee/Nest thermostat behavior) or reduce the config to a boolean `has_backup` until staging is implemented. OCHRE has the same gap; a shared design decision is acceptable.

5. **Make fresh-cycle detection in TwoSpeedTime state-driven** by comparing `last_speed_index` and tracking equipment-on state internally (similar to OCHRE's `self.mode == "Off"`). This eliminates the fragile `prev_zone_temp_c = None` contract.

6. **Add a diagnostic trace when `capacity_fractions_for` returns empty**, including the raw capacity values, to help debug misconfigured equipment.

## References / Citations
- OCHRE `HVAC.py:868-916` — `DynamicHVAC.run_two_speed_control`: unified minimum-time guard for all 2-speed types
- OCHRE `HVAC.py:990-1020` — `DynamicHVAC.update_capacity`: variable-speed interpolation via `np.searchsorted` and fractional `speed_idx`
- OCHRE `HVAC.py:13-18` — `SPEED_TYPES` accepts only `{1, 2, 4}` speeds
- OCHRE `HVAC.py:1371-1394` — TODO for staged backup heat (same gap as HARES)
- Cutler et al. (2013) "Improved Modeling of Residential Air Conditioners and Heat Pumps for Energy Calculations" — biquadratic speed model foundation
- AHRI 210/240-2023 S6.6.3 — default PLF degradation coefficient Cd=0.25
