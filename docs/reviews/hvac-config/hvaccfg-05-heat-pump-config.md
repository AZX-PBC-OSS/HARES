# HeatPumpConfig: dual-mode independence, compressor lockout, defrost
**Review ID**: hvaccfg-05
**Category**: hvac-config
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-equipment/src/hvac/heat_pump_config.rs` (978 lines)
- `crates/hares-equipment/src/hvac/heat_pump/defrost.rs` (1003 lines)
- `crates/hares-equipment/src/hvac/heat_pump/constants.rs` (96 lines)
- `crates/hares-equipment/src/hvac/heat_pump/heater.rs` (relevant lockout/defrost sections)
- `crates/hares-equipment/src/hvac/heat_pump/cooler.rs` (relevant control sections)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/DXCoils.hh` — `DXCoilData` struct with `MinOATCompressor`, `MaxOATCompressor`, defrost fields
- `vendors/EnergyPlus/src/EnergyPlus/UnitarySystem.hh` — `UnitarySys` with separate `m_MinOATCompressorCooling` / `m_MinOATCompressorHeating`, `m_MaxOATSuppHeat`
- `vendors/EnergyPlus/src/EnergyPlus/VariableSpeedCoils.hh` — variable-speed defrost/compressor lockout fields
- `vendors/EnergyPlus/src/EnergyPlus/StandardRatings.hh` — `DefrostStrat` and `HPdefrostControl` enums
- `vendors/EnergyPlus/src/EnergyPlus/HVACMultiSpeedHeatPump.hh` — separate heating/cooling speeds and OAT lockouts per mode
- `vendors/EnergyPlus/src/EnergyPlus/UnitarySystem.cc` — lockout enforcement at lines 12157/12330/12778/14602

## Findings

### Finding 1: No minimum outdoor temperature lockout for cooling compressor [Severity: high]
**Description**: `HeatPumpCoolerConfig` has no field to specify a minimum outdoor temperature below which the cooling compressor should lock out. EnergyPlus tracks `m_MinOATCompressorCooling` in `UnitarySys` (collected from `DXCoils::GetMinOATCompressor` at `UnitarySystem.cc:6831-6843`, enforced at line 12778). Real heat pump controllers typically lock out cooling below 10–15°C when an economizer can provide free cooling. Without this parameter, the cooler will attempt to run the compressor at any outdoor temperature, which is unrealistic and can produce erroneous energy consumption during cold weather.
**Code Location**: `heat_pump_config.rs:383-400` (`HeatPumpCoolerConfig` struct — only `crankcase_heater_kw`, `crankcase_heater_threshold_c`, and `stage_shrs` are cooler-specific; no `min_oat_cooling_c` or equivalent)
**Root Cause**: The cooler's control is delegated entirely to `CentralAirConditioner` (via `typed_hp_to_central_ac_config`, `cooler.rs:110-186`), which also lacks any compressor lockout parameter. Neither the HP-specific nor the generic AC config path supports cooling compressor lockout.
**Impact**: Without a cooling lockout, the model may run the compressor when the outdoor temperature is too low for safe or efficient operation. In real systems, below ~10–15°C, an economizer handles cooling load without compressor operation. This leads to overestimation of compressor runtime and energy use in mild/cold weather, and misrepresents sequences where the economizer alone would suffice.

### Finding 2: No maximum outdoor temperature for heating compressor lockout [Severity: medium]
**Description**: The heating side has `hp_lockout_temp_c` (minimum OAT, defaults to −17.78°C) but no maximum OAT above which the heating compressor should lock out. EnergyPlus defines `MaxOATCompressor` on `DXCoilData` (`DXCoils.hh:239`) for VRF heat pumps. Practically, heat pump heating is not needed (and may be disabled by controls) above 15–20°C outdoor temperature. Without an upper bound, the model may run heating at any OAT, which is unrealistic during shoulder seasons or in climates with mild winters.
**Code Location**: `heat_pump_config.rs:179-217` (`HeatPumpHeaterConfig` — `hp_lockout_temp_c` (line 184) is present but no `max_oat_heating_c` counterpart)
**Root Cause**: The design focuses on the low-temperature limit (compressor lockout for cold weather) but omits the high-temperature limit where heating demand is zero or satisfied by minimal solar/internal gains.
**Impact**: In mild climates and shoulder seasons, the model may cycle the heat pump in heating mode at outdoor temperatures where real systems would remain off. This inflates heating energy consumption and can produce unrealistic cycling behavior.

### Finding 3: `cycle_duration_s` and `max_defrost_duration_s` are not user-configurable from `DefrostConfig` [Severity: medium]
**Description**: The `DefrostCycleTracker` (the discrete defrost ON/OFF cycle FSM, `defrost.rs:233-245`) initializes `cycle_duration_s` and `max_defrost_duration_s` from hardcoded constants (`DEFAULT_DEFROST_CYCLE_DURATION_S = 210.0 s` and `MAX_DEFROST_CYCLE_DURATION_S = 600.0 s`, `constants.rs:93-96`). These values are never sourced from `DefrostConfig`. The `DefrostConfig` struct (`defrost.rs:55-102`) has no corresponding fields. Users cannot adjust the defrost cycle duration (e.g., to 5 minutes for a unit with a 300 s defrost cycle) or the hard safety cap (e.g., to 8 minutes instead of 10).
**Code Location**: 
- `defrost.rs:242-244` — `cycle_duration_s` and `max_defrost_duration_s` default to constants
- `defrost.rs:247-258` — `DefrostCycleTracker::new()` hardcodes defaults
- `defrost.rs:55-102` — `DefrostConfig` lacks `defrost_cycle_duration_s` and `defrost_max_duration_s`
**Root Cause**: The discrete defrost FSM was added as a later enhancement over the original continuous model. The cycle timing parameters were placed on the runtime tracker (`DefrostCycleTracker`) rather than the config struct (`DefrostConfig`), making them effectively hardcoded.
**Impact**: All heat pump models use the same 210 s defrost cycle and 600 s max, regardless of manufacturer specifications. Some residential units have shorter or longer defrost cycles (2–10 min range). The 600 s safety cap is reasonable for most equipment, but non-configurability reduces model flexibility for atypical equipment.

### Finding 4: No configurable defrost interval (compressor runtime accumulator) [Severity: low]
**Description**: In real heat pump controllers, the inter-defrost interval — accumulated compressor runtime before initiating the next defrost cycle — is a settable parameter (typically 30, 60, or 90 minutes). HARES derives this interval implicitly from `cycle_duration_s / time_fraction` in the discrete tracker (`defrost.rs:278`). There is no user-facing field to directly set "defrost every 60 minutes of compressor runtime." This is a convenience gap — users can indirectly control the interval by adjusting `defrost_time_fraction`, but the relationship is non-obvious.
**Code Location**: `defrost.rs:278` — `let interval_s = self.cycle_duration_s / time_fraction;` (derived, not configurable)
**Root Cause**: The HARES discrete model maps the continuous time fraction model onto discrete cycles by inverting the fraction. The interval is a derived quantity, not a primary parameter.
**Impact**: Low. Users familiar with the EnergyPlus fractional-time model will understand the relationship. However, users accustomed to setting defrost intervals in minutes on thermostats may be confused. The derived interval is mathematically equivalent to the continuous model.

### Finding 5: Airflow, fan power, and number of speeds are shared between heating and cooling [Severity: medium]
**Description**: `HeatPumpCommonConfig` defines single values for `airflow_m3_s_per_w` (line 63), `fan_power_w` (line 59), `fan_power_w_per_cfm` (line 61), and `number_of_speeds` (line 53). These are used identically for both heating and cooling operation. In contrast, EnergyPlus treats heating and cooling coils as separate objects with independent airflow rates, fan powers, and numbers of speeds (`NumOfSpeedHeating` vs `NumOfSpeedCooling` in `MSHeatPumpData`, `HVACMultiSpeedHeatPump.hh:100-107`). Real heat pumps often have different numbers of compressor stages for heating vs cooling (e.g., 2-stage cooling, 3-stage heating), and heating airflow is typically lower than cooling airflow (350–400 CFM/ton for heating vs 400–450 CFM/ton for cooling).
**Code Location**: `heat_pump_config.rs:53-63` (`number_of_speeds`, `fan_power_w`, `fan_power_w_per_cfm`, `airflow_m3_s_per_w` in `HeatPumpCommonConfig`)
**Root Cause**: The `HeatPumpCommonConfig` was designed as a flattened shared config, sacrificing mode-independence for serialization simplicity (serde `flatten` constraints).
**Impact**: A heat pump with 3 heating stages and 2 cooling stages cannot be accurately represented. Airflow-dependent performance (capacity and EIR corrections via biquadratic curves) will be identical for both modes even when physical airflows differ. For most residential single-speed units this is acceptable, but for multi-stage and mini-split equipment it introduces error.

### Finding 6: `shr` (sensible heat ratio) is cooling-only but placed on common config [Severity: low]
**Description**: `HeatPumpCommonConfig` includes `shr` (line 57, sensible heat ratio), which is a cooling-mode parameter. The heater side has its own `heating_shr` (line 202) on `HeatPumpHeaterConfig`, which correctly defaults to 1.0 for all-sensible heating. The common `shr` is ambiguous and could be misinterpreted as applying to heating as well. In practice, the cooler's `typed_hp_to_central_ac_config` maps `shr` for cooling only (`cooler.rs:147`), and the heater ignores it. But the field's location on the common config is misleading.
**Code Location**: `heat_pump_config.rs:57` (`shr` on `HeatPumpCommonConfig`)
**Root Cause**: `shr` predates the `heating_shr` field and was placed on common to serve the cooling air conditioner mapping path. When `heating_shr` was added later, `shr` was not moved to the cooler-specific struct.
**Impact**: Low. The field is correctly consumed only on the cooling path. But the naming is ambiguous — a user might set `shr` expecting it to affect heating latent loads, which it does not. Documentation clarity would suffice.

### Finding 7: Supplemental heat does not prevent simultaneous HP+ER operation at moderate OAT [Severity: low]
**Description**: The supplemental ER control (`heater.rs:1862-1866`) allows simultaneous heat pump and ER operation whenever the ER thermostat call is active and ER is not temperature-blocked. Real controllers typically prevent simultaneous HP+ER operation above the thermal balance point (the OAT where the heat pump capacity equals the building load). Above this point, the heat pump alone can satisfy the load, so ER should not run. HARES does have `er_lockout_temp_c` (4.44°C default) and `max_oat_supplemental_c` (21°C default), which cap ER operation to cold weather. However, there is no explicit "thermal balance point" parameter that models the load-vs-capacity crossover. The current two-threshold approach (OCHRE aggressive 4.44°C + EnergyPlus safety cap 21°C) approximates this but lacks the load-dependent nuance.
**Code Location**: `heater.rs:1814-1815` (`er_allowed_by_temp` check) and `heater.rs:1830-1866` (HP+ER simultaneous logic)
**Root Cause**: The OCHRE heritage model uses a simple OAT-based ER lockout rather than a load-based thermal balance point. This is a simplification that works well for most residential applications but may allow ER operation in mild weather if `er_lockout_temp_c` is set too high.
**Impact**: Low at default settings (ER locked out above 4.44°C). But if a user sets `er_lockout_temp_c` to a higher value (e.g., 15°C), the ER may fire simultaneously with the HP in moderate weather where the HP alone would suffice, overestimating energy consumption.

## Summary
- **Total findings**: 7
- **Critical**: 0
- **High**: 1 (no cooling compressor lockout)
- **Medium**: 3 (no max OAT for heating lockout, defrost timing not configurable, shared airflow/speeds)
- **Low**: 3 (no configurable defrost interval, shr on common config, no thermal balance point)

## Recommendations
1. **Add `min_oat_cooling_c` to `HeatPumpCoolerConfig`** (mirroring the heater's `hp_lockout_temp_c`) with a sensible default (e.g., 10°C per ASHRAE 90.1 economizer changeover). Wire it through to the `CentralAirConditioner` control path so the cooling compressor is disabled below this outdoor temperature. EnergyPlus reference: `m_MinOATCompressorCooling` in `UnitarySys` (collected from `DXCoils::GetMinOATCompressor`).

2. **Add `max_oat_heating_c` to `HeatPumpHeaterConfig`** to model the upper OAT bound above which heat pump heating is disabled. Default to the ASHP supply air temperature formula's implied cutoff (approximately 20°C) or make it user-configurable to match specific thermostat models. EnergyPlus reference: `MaxOATCompressor` on `DXCoilData`.

3. **Add `defrost_cycle_duration_s` and `defrost_max_duration_s` to `DefrostConfig`** so users can customize the discrete defrost cycle timing. Default to the current constants (210 s / 600 s). Wire these into `DefrostCycleTracker` initialization in `heater.rs`.

4. **Consider splitting `number_of_speeds`, `airflow_m3_s_per_w`, and `fan_power_w` into heating and cooling variants** in `HeatPumpCommonConfig` (e.g., `number_of_heating_speeds`, `number_of_cooling_speeds`; `heating_airflow_m3_s_per_w`, `cooling_airflow_m3_s_per_w`). Fall back to the current shared values when mode-specific values are absent for backward compatibility. EnergyPlus reference: `m_NumOfSpeedHeating` / `m_NumOfSpeedCooling` in `UnitarySys`.

5. **Move `shr` from `HeatPumpCommonConfig` to `HeatPumpCoolerConfig`** (alongside `stage_shrs`) and document that `heating_shr` on `HeatPumpHeaterConfig` is the heating-side counterpart. This clarifies that SHR is mode-specific.

## References / Citations
- EnergyPlus `UnitarySystem.cc:12157` — cooling compressor allowed only when `OutsideDryBulbTemp > m_MinOATCompressorCooling`
- EnergyPlus `UnitarySystem.cc:14602` — heating compressor allowed only when `OutdoorDryBulb < m_MinOATCompressorHeating` (note: EnergyPlus uses > for cooling and < for heating due to different MinOAT semantics)
- EnergyPlus `DXCoils.hh:230-251` — defrost fields: `MaxOATDefrost`, `DefrostTime`, `DefrostCapacity`, `HPCompressorRuntime`
- EnergyPlus `HVACMultiSpeedHeatPump.hh:100-107` — separate `NumOfSpeedHeating` / `NumOfSpeedCooling` for MSHP
- EnergyPlus `UnitarySystem.hh:313-314` — `m_MinOATCompressorCooling` and `m_MinOATCompressorHeating` as separate lockout thresholds
- EnergyPlus `StandardRatings.hh` — `DefrostStrat` (ReverseCycle/Resistive) and `HPdefrostControl` (Timed/OnDemand) enums, directly mirrored in HARES `DefrostControl` and `DefrostStrategy`
- OCHRE `HVAC.py:1208` — `-17.78` default for compressor lockout temperature, source of HARES `DEFAULT_HP_LOCKOUT_TEMP_C`
- ASHRAE HVAC Systems and Equipment 2020, Chapter 9 — typical residential heat pump defrost cycle parameters (3–10 min duration, 30/60/90 min intervals)
