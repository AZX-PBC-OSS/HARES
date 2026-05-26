# Startup capacity degradation ramp (Winkler 2011) implementation
**Review ID**: equip-hvac-14
**Category**: equipment-hvac
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-equipment/src/hvac/air_conditioner.rs`
- `crates/hares-equipment/src/hvac/heat_pump/heater.rs`
- `crates/hares-equipment/src/hvac/heat_pump/cooler.rs`
- `crates/hares-equipment/src/hvac/hvac_core.rs`
- `crates/hares-equipment/src/hvac/speed_control.rs` (StartupConfig)
- `crates/hares-equipment/src/hvac/staging.rs` (apply_startup_capacity_degradation)
- `crates/hares-equipment/src/hvac/cooling_config.rs` (derived_cooling_startup_cd)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Equipment/HVAC.py`

## Findings

### Finding 1: [Severity: critical]
**Description**: HARES applies startup capacity degradation to air conditioners and HP coolers; OCHRE restricts it to heat pump heating mode only. OCHRE's `calc_startup_capacity_degredation` (HVAC.py:977) gates the entire ramp on `"HP" in self.mode`. Because `AirConditioner.domain` returns `"On"/"Off"` and `ASHPCooler`/`MinisplitAHSPCooler` inherit that behaviour, the ramp multiplier is always `1.0` for cooling-only equipment in OCHRE. HARES calls `apply_startup_capacity_degradation` unconditionally from `AirConditioner::calculate_performance` (air_conditioner.rs:1228) and `HeatPumpHeaterCore::compute_step` (heater.rs:1362), applying a non-unity multiplier to every DX cooling cycle.

**Code Location**:
- OCHRE gate: `HVAC.py:977` (`if "HP" in self.mode:`)
- HARES cooling call site: `air_conditioner.rs:1228` (`.apply_startup_capacity_degradation(steady_capacity_w, dt_min)`)
- HARES heating call site: `heater.rs:1362` (`.apply_startup_capacity_degradation(steady_capacity_w, dt_min)`)

**Root Cause**: HARES' `apply_startup_capacity_degradation` (staging.rs:298–316) has no equipment-type guard. The `on_now` signal is derived from generic `duty_cycle > 0.0` (staging.rs:303) rather than a compressor-actually-running check.

**Impact**: Single-speed and two-speed air conditioners (central AC, room AC, ASHP cooler) with non-zero Cd (0.07 – 0.22 by default) experience 2–5 minutes of artificially reduced capacity on every thermostat cycle, even though the Winkler 2011 model was developed for heat pump compressor startups and OCHRE explicitly excludes AC equipment. This depresses seasonal cooling energy efficiency metrics relative to OCHRE, particularly for tight-cycling equipment in moderate climates.

---

### Finding 2: [Severity: high]
**Description**: OCHRE's default `c_d` is `0.0` (startup degradation OFF), while HARES defaults `c_d` to `DEFAULT_PLF_DEGRADATION_COEFF = 0.25` (ON) for non-mini-split equipment. Although HARES refines Cd via `derived_cooling_startup_cd()` (cooling_config.rs:127–137), the ramp is always active (Cd ≥ 0.07 for SEER ≥ 13 equipment), whereas OCHRE disables it unless the user explicitly supplies `"Startup Capacity Degradation (-)"` in the configuration.

**Code Location**:
- OCHRE default: `HVAC.py:765` (`self.c_d = kwargs.get("Startup Capacity Degradation (-)", 0.0)`)
- HARES default: `staging.rs:20` (`pub(super) const DEFAULT_PLF_DEGRADATION_COEFF: f64 = 0.25;`)
- HARES init: `hvac_core.rs:492–495` (`self.runtime.startup = StartupConfig { c_d: cd, time_since_start_min: 0.0 }`)

**Root Cause**: HARES reuses the same Cd constant for both PLF cycling degradation (AHRI 210/240) and startup capacity ramp (Winkler 2011), conflating two distinct physical phenomena with different defaults. OCHRE keeps them separate and defaults the startup ramp to 0.0.

**Impact**: Equipment that would have no startup penalty in OCHRE experiences a permanent capacity derate in HARES. The energy impact is most pronounced for single-speed ACs with SEER < 13 (Cd=0.20 in HARES, 0.0 in OCHRE).

---

### Finding 3: [Severity: high]
**Description**: In the ASHP heater path, HARES advances the startup timer when the equipment duty cycle is non-zero, even in ER-only mode where the heat pump compressor is not running. `apply_startup_capacity_degradation` uses `duty_cycle > 0.0` as `on_now` (staging.rs:303), but the heater can have a positive `duty_cycle` while only the backup resistance element is active (`HeatingER` mode). OCHRE's `calc_startup_capacity_degredation` only advances `time_from_start` when `"HP" in self.mode` (HVAC.py:977, 985), which correctly excludes ER-only operation.

**Code Location**:
- HARES: `staging.rs:303` (`let on_now = self.runtime.duty_cycle > 0.0;`)
- HARES heater: `heater.rs:1360–1362` (unconditional call)
- OCHRE guard: `HVAC.py:977` (`if "HP" in self.mode:`), timer advance: `HVAC.py:985` (`self.time_from_start += self.time_res`)

**Root Cause**: The shared `duty_cycle` field conflates HP compressor runtime with ER backup runtime. The startup ramp component (`StartupConfig`) has no awareness of whether the actual compressor is energised.

**Impact**: If an ASHP runs in ER-only mode for several minutes (e.g., during a cold snap below the HP lockout), and then the outdoor temperature rises and the HP compressor engages, the startup timer may already be past `t_full`, causing the HP compressor to skip its startup ramp entirely. This overestimates HP capacity in the first few minutes after the mode switch back to HP.

---

### Finding 4: [Severity: medium]
**Description**: HARES resets `time_since_start_min = 0.0` on **every** off-step, regardless of off-duration. OCHRE resets `time_from_start = 0.5 * time_res` only on a mode transition from non-"HP" to "HP". Both approaches result in full-restart behaviour after any compressor-off interval, but HARES' mechanism is triggered by PLR cycling at the sub-timestep level, potentially causing multiple ramp resets within a single simulation timestep when `part_load_ratio < 1.0`.

**Code Location**:
- HARES reset: `speed_control.rs:72–73` (`if !on_now { self.time_since_start_min = 0.0; return 1.0; }`)
- OCHRE reset: `HVAC.py:978–979` (`if "HP" not in self.mode_prev: self.time_from_start = 0.5 * self.time_res`)

**Root Cause**: HARES' `capacity_multiplier` method is driven by a per-step boolean rather than a mode-transition edge detector. For equipment with PLR < 1.0 where the compressor physically cycles on/off within a single timestep, each sub-cycle would trigger a fresh ramp. The intent expressed in the code comment at `air_conditioner.rs:830` (`// apply_startup_capacity_degradation resets the ramp timer when off.`) acknowledges but does not mitigate this.

**Impact**: For equipment operating at part load with fractional PLR, capacity is doubly penalised (once by PLR, once by repeated startup ramps). The magnitude depends on timestep resolution and cycling frequency.

---

### Finding 5: [Severity: medium]
**Description**: Neither HARES nor OCHRE models off-time-proportional recovery. A brief off period (e.g., 1 minute) triggers the same full restart degradation as an overnight shutdown. The Winkler 2011 model is a compressor transient model (thermal mass and pressure equalisation), so short off periods should logically produce less severe restarts than cold starts. The review question — "does a brief off period trigger a full restart degradation, or is the degradation proportional to off-time?" — is answered affirmatively: both implementations use full restart degradation regardless of off-duration.

**Code Location**:
- HARES: `speed_control.rs:86` (first on-step initialises `time_since_start_min = 0.5 * dt_min` — no memory of prior off-duration)
- OCHRE: `HVAC.py:979` (`self.time_from_start = 0.5 * self.time_res` — same pattern)

**Root Cause**: The Winkler exponential formula has no off-time parameter. The model assumes a cold compressor start every time. This is a limitation of the reference model, not an implementation defect.

**Impact**: Over-punishes tight thermostat hysteresis configurations or intermittent cloud-cover cycling, where the compressor turns off for only a minute or two before restarting.

---

### Finding 6: [Severity: low]
**Description**: Variable-speed/inverter equipment correctly bypasses the startup ramp when `c_d == 0.0` (speed_control.rs:78–80), and `derived_cooling_startup_cd()` returns `Some(0.0)` for `VariableSpeedIdeal` mode (cooling_config.rs:129). However, an explicit `startup_cd` config override with a non-zero value would be accepted for variable-speed equipment and would apply the ramp, which is physically incorrect for inverter-driven systems where capacity modulation is continuous and there is no discrete compressor start transient.

**Code Location**: `cooling_config.rs:128` (`self.startup_cd.or(match self.cooling_speed_control_mode() {`) — the explicit override takes priority over the derived default.

**Root Cause**: The `or` pattern always prefers user override over derived defaults. There is no validation rejecting `startup_cd > 0.0` for `VariableSpeedIdeal` mode.

**Impact**: Minor — relies on user error. If a user explicitly sets `startup_cd = 0.15` on a variable-speed system, the ramp would be incorrectly applied.

---

### Finding 7: [Severity: low]
**Description**: Room AC units use a hardcoded `startup_cd` default of `0.22` (air_conditioner.rs:528), which is significantly higher than the SEER-based derived defaults for central AC (0.07 for SEER ≥ 13). This value appears to come from the general PLF degradation coefficient rather than a room-AC-specific startup study, and OCHRE's `RoomAC` class (HVAC.py:1080–1086) does not override `c_d`, leaving it at the OCHRE global default of `0.0`.

**Code Location**: `air_conditioner.rs:528` (`let cd = cfg.startup_cd.unwrap_or(0.22);`)

**Root Cause**: Room AC path uses a different Cd resolution path than central AC (`startup_cd.unwrap_or(0.22)` vs `derived_cooling_startup_cd()`). The 0.22 value appears to be the legacy PLF degradation default, not a room-AC-specific startup parameter.

**Impact**: Room ACs experience a startup ramp lasting `20 * 0.22 + 0.4 = 4.8` minutes per cycle, while OCHRE's equivalent has no startup ramp at all. This disproportionately affects window/room AC models.

---

## Summary
- **Total findings**: 7
- **Critical**: 1 (AC/HP-cooler startup ramp not gated like OCHRE)
- **High**: 2 (default Cd divergence, ER-only timer advancement)
- **Medium**: 2 (per-off-step reset vs mode-transition reset, no off-time recovery)
- **Low**: 2 (variable-speed config override, room AC Cd constant)

## Recommendations
1. **Add an equipment-type guard** to `apply_startup_capacity_degradation` (or its call sites) so that only heat pump compressor operation activates the ramp, matching OCHRE's `"HP" in self.mode` gate. Air conditioners and HP coolers should either bypass the ramp entirely or use `c_d = 0.0` by default.
2. **Reconcile default Cd values**: Either adopt OCHRE's default of `0.0` for startup degradation and require explicit user input to enable it, or document the divergence clearly. The current conflation of PLF cycling Cd with startup ramp Cd is the root cause of the permanently-active ramp.
3. **Make the startup timer compressor-aware** in the ASHP heater path: gate `on_now` on `hp_on_control` rather than generic `duty_cycle > 0.0`. This prevents the timer from advancing during ER-only operation.
4. **Add off-time-proportional recovery** to the startup model if short-cycling accuracy is a concern. The simplest approach: scale the initial `time_since_start_min` based on off-duration (e.g., `time_since_start_min = max(0, t_full - off_min * recovery_rate)`).
5. **Validate or reject** explicit `startup_cd` overrides for variable-speed equipment — either emit a warning or return an error when `startup_cd > 0.0` and the speed control mode is `VariableSpeedIdeal`.
6. **Review the Room AC Cd default** of 0.22 against the SEER-based central-AC defaults; if Room AC startup physics differ materially from central AC, cite the reference.

## References / Citations
- Winkler, J. 2011. Startup capacity degradation model: exponential ramp with `t_full = 20 * Cd + 0.4` minutes, `mult = clamp(0, 1, -1.025 * exp(-3.79936 * t / t_full) + 1.025)`. Referenced in OCHRE `calc_startup_capacity_degredation` (HVAC.py:971–988) and HARES `StartupConfig::capacity_multiplier` (speed_control.rs:71–97).
- AHRI 210/240-2023: PLF degradation coefficient Cd (0.25 default), used by HARES as the root Cd for both PLF and startup models.
- Cutler, D. et al. 2013. "Improved Modeling of Residential Air Conditioners and Heat Pumps for Energy Calculations." NREL. Section 2.2.1 references the biquadratic model but does not specify startup ramp application scope.
