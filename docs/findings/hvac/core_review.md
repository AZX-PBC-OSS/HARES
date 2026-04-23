HARES HVAC Implementation Quality Assessment
Executive Summary
The HVAC subsystem is architecturally sound with physically grounded models and extensive test coverage. The main risks are from significant DRY violations in thermostat/control logic (duplicated between HvacEquipment and IdealHvac), struct bloat in HvacEquipment (~40 public fields), and a duplicated speed-selection algorithm in the AC cooler. The control crate (hares-control) is cleanly layered and well-factored. Config structs are well-typed with serde validation and sensible defaults backed by AHRI/OCHRE references.
---
Area 1: Core HVAC Structure and Control Logic
Rating: Adequate
Strengths:
- Thermostat FSM is physically correct with hysteresis, asymmetric deadband offset, and cutout ratio (thermostat.rs:18-30, hvac_core.rs:666-722)
- Part-load factor follows AHRI 210/240 with proper floor clamping (staging.rs:268-312)
- Winkler 2011 startup ramp is correctly implemented with mid-step initialization (speed_control.rs:71-97)
- Min-cycle and compressor on/off time guards prevent unrealistic short-cycling (hvac_core.rs:724-809)
- Two-speed time mode correctly detects "moving wrong way" conditions (staging.rs:150-200)
- Well-separated files: thermostat.rs (184 lines), speed_control.rs (206 lines), staging.rs (422 lines)
Findings:
#	Severity	File:Line	Finding
F1	High	hvac_core.rs:654-733 vs ideal_hvac.rs:268-364	Thermostat FSM logic is fully duplicated. HvacEquipment::update_mode and IdealHvac::update_mode implement the identical FSM with the same offset/cutout branching (~100 lines). Similarly can_transition_mode (hvac_core.rs:790-809 vs ideal_hvac.rs:251-266), set_mode (hvac_core.rs:770-776 vs ideal_hvac.rs:240-249), and resolve_profile_setpoints/resolve_schedule_setpoints (hvac_core.rs:629-652 vs ideal_hvac.rs:204-227) are duplicated. This creates a maintenance risk: a bug fix in one path may not propagate to the other.
F2	High	hvac_core.rs:122-249	HvacEquipment is a god struct with ~40 public fields mixing configuration (thermostat, biquadratic_coeffs), mutable runtime state (mode, duty_cycle, plf_state), physics parameters (shr, airflow_m3_s_per_w), and control signal state (max_capacity_fraction, disabled_speeds). This makes it difficult to reason about field lifetimes and invariants. The invariant comment on mode_start_at (hvac_core.rs:138-143) documents the coupling but the struct does not enforce it.
F3	Medium	hvac_core.rs:572-608 vs ideal_hvac.rs:654-720	Control signal handling is partially duplicated. Both HvacEquipment::apply_control_signal and IdealHvac::apply_control_unchecked implement ThermalSetpoint and ThermalSetpointDelta dispatch with the same idempotent delta logic. The delta anchor logic (base = static + schedule) is identical.
F4	Medium	hvac_core.rs:328-566	HvacEquipment::init is 240 lines handling ~30 config keys with extract_numeric/extract_bool chains. This is fragile: adding a new key requires careful placement within the sequence, and there's no compile-time guarantee that a typed config field is consumed here.
F5	Low	core_config.rs:73-110	cfg(test) polymorphic extract helpers mean test builds resolve keys from both typed and raw paths, but production builds only check typed. This is intentional for backward compatibility but creates a subtle behavioral divergence between test and prod.
Control Logic Correctness:
- The thermostat FSM correctly models on/off cycling with hysteresis. The dual-mode (offset > 0 vs offset == 0) branching in update_mode accurately captures asymmetric vs symmetric deadband behavior (hvac_core.rs:666-722).
- The staging system correctly interpolates between speed stages for MultiSpeedInterpolated (staging.rs:202-249).
- PLF floor clamping is physically correct: max(plf_min, PLR) ensures PLF never drops below actual runtime fraction, preventing the model from claiming more efficiency than physically possible at very low PLR (staging.rs:309).
---
Area 2: Air Conditioner and Baseboard/Boiler/Furnace Implementations
Rating: Good
Strengths:
- CoolingCore properly extracts shared logic between AirConditioner and RoomAC (air_conditioner.rs:44-99)
- Biquadratic curve evaluation with flow-fraction correction is physically grounded (air_conditioner.rs:968-1060)
- Crankcase heater correctly accounts for companion heating coil RTF in HP systems (air_conditioner.rs:897-923)
- Gas boiler condensing/non-condensing EIR polynomial matches OCHRE reference (boiler.rs:369-396)
- Jacket loss energy conservation is correct: fuel_input + electric - thermal_output (boiler.rs:514)
- Heating equipment consistently delegates to shared helpers (helpers::update_heating_control, apply_heating_control_unchecked)
- Fuel independence from duct DSE is correctly modeled: furnaces burn the same gas regardless of duct losses (furnace.rs:374-381)
Findings:
#	Severity	File:Line	Finding
F6	High	air_conditioner.rs:335-397 vs staging.rs:202-249	Duplicated variable-speed interpolation. CoolingCore::select_variable_speed_cooling reimplements the same capacity-fraction bracket interpolation already in HvacEquipment::select_multi_speed. The partition_point logic, span calculation, and SpeedSelection construction are identical. This should call hvac.select_speed() or share a common function.
F7	Medium	air_conditioner.rs:280-325	RoomAC does not implement ideal_target(). The Equipment impl for RoomAC at line 280 lacks an ideal_target method, meaning RoomAC cannot participate in solver-driven ideal-capacity computation even though it inherits the use_ideal flag from CoolingCore.
F8	Medium	furnace.rs:160-174 vs boiler.rs:204-260	Heating step logic is nearly identical across equipment types. Electric furnace, electric boiler, and baseboard all follow the pattern: thermal = capacity * duty; electric = thermal * eir / 1000. The differences are minor (duct DSE, fluid port vs thermal port, fan heat). A shared SimpleHeatingStep helper could reduce this.
F9	Low	cooling_config.rs:10-12 vs heat_pump_config.rs:10-12	default_one() is defined independently in two config files with identical implementation. Should be a shared helper.
F10	Low	air_conditioner.rs:1062-1090	Ideal capacity re-evaluation in calculate_performance is complex: when use_ideal && ideal_capacity_w != 0, it re-runs variable speed selection and curve inputs inside the performance calculation. This is correct but hard to follow — the ideal path effectively overrides the duty cycle set by update_control. A cleaner approach would be to finalize speed selection in update_control so calculate_performance is a pure function of current state.
F11	Low	furnace.rs:189	Telemetry thermal_output_w includes duct DSE (total_sensible_w * dse), but write_zone_thermal_contributions already distributes by zone_heat_fractions (which encodes DSE). This means the telemetry value reports DSE-adjusted output, while the port gets gross output distributed by DSE. The two are consistent but the naming is potentially confusing — telemetry says "thermal output" but it's net-delivered, not gross.
---
Area 3: Ideal Capacity Implementation
Rating: Adequate
Strengths:
- IdealHvac correctly models unlimited capacity by accepting solver-provided IdealCapacity signals (ideal_hvac.rs:656-658)
- The ideal_target() method properly returns (zone_id, current_target_c) only when active and in ideal mode (ideal_hvac.rs:722-727)
- Cooling SHR splitting into sensible/latent is physically correct (ideal_hvac.rs:555-559)
- Fan power ratio calculation matches OCHRE convention: fan_power = |capacity| * eir * fan_power_ratio (ideal_hvac.rs:545-549)
- Minimum capacity threshold prevents unrealistic near-zero operation (ideal_hvac.rs:531-535)
- IdealCapacityMode::Auto correctly engages at coarse timesteps (≥300s) or with variable-speed equipment (ideal_hvac.rs:229-238)
- State save/restore via postcard serialization is complete and tested
Findings:
#	Severity	File:Line	Finding
F12	High	ideal_hvac.rs:268-364	Full FSM duplication (see F1). The IdealHvac thermostat logic is an exact copy of HvacEquipment's. This is the single largest DRY violation in the HVAC subsystem. A ThermostatFsm struct that both HvacEquipment and IdealHvac delegate to would eliminate ~150 lines of duplication and the associated divergence risk.
F13	Medium	ideal_hvac.rs:518-528	Non-ideal step uses rated capacity without biquadratic correction. When use_ideal_cached == false, the step falls back to rated_capacity_w (heating) or cooling_capacity_w (cooling) at line 526-527. This means at fine timesteps with IdealCapacityMode::Off, the equipment delivers full rated capacity regardless of outdoor temperature, ignoring the biquadratic performance curves that real equipment would have. This is physically inaccurate for heat pumps whose capacity degrades at low outdoor temperatures.
F14	Medium	ideal_hvac.rs:563-571	Fan motor heat always added to sensible gain. Line 567 adds fan_power_w to sensible_gain_w. This is correct for heating (fan heat adds to warming), but in cooling mode the fan heat partially offsets cooling — the code adds fan heat to the (already negative) sensible cooling, which is correct per OCHRE convention, but there is no explicit comment acknowledging this physical subtlety.
F15	Low	ideal_hvac.rs:275-281	Deadband target is the midpoint of heating/cooling setpoints. This is an arbitrary choice — in deadband, the "target" isn't physically meaningful. The solver should already know not to dispatch ideal capacity when mode is Deadband (since ideal_target() returns None), so this field is vestigial in deadband.
---
Area 4: Separation of Concerns
Rating: Adequate
Architecture diagram:
hares-control          hares-equipment/hvac
  dispatch.rs           hvac_core.rs (HvacEquipment)
  capabilities.rs         ├── thermostat.rs
  signal.rs               ├── speed_control.rs
  compat.rs               ├── staging.rs
  types.rs                ├── core_config.rs
                          ├── ac_config.rs / cooling_config.rs / heating_config.rs / heat_pump_config.rs
                          ├── air_conditioner.rs (CoolingCore)
                          ├── baseboard.rs / boiler.rs / furnace.rs
                          ├── ideal_hvac.rs
                          └── helpers.rs
Strengths:
- hares-control is cleanly separated: it defines types and routing, not equipment behavior
- staging.rs and speed_control.rs are properly extracted from hvac_core.rs
- CoolingCore encapsulates AC-specific logic behind AirConditioner and RoomAC facades
- Config structs are in separate files with serde(deny_unknown_fields) for safety
- The helpers module centralizes heating control dispatch (update_heating_control, apply_heating_control_unchecked)
Findings:
#	Severity	File:Line	Finding
F16	High	hvac_core.rs + ideal_hvac.rs	Thermostat abstraction is leaky. The thermostat FSM logic lives on both HvacEquipment and IdealHvac as inline methods rather than being a reusable ThermostatFsm component. This is the root cause of findings F1, F3, F12. A ThermostatFsm struct containing mode, mode_start_at, last_mode_switch_at, thermostat, setpoints, and update_mode() would be composable by both equipment types.
F17	Medium	hvac_core.rs:122-249	HvacEquipment conflates configuration and runtime state. Fields like biquadratic_coeffs (config) sit alongside plf_state (runtime) and disabled_speeds (control signal state). These have different lifecycles: config is set once at init, runtime changes every step, control state changes on signal receipt. The struct would benefit from grouping these into sub-structs (e.g., HvacConfig, HvacRuntimeState, HvacControlState).
F18	Medium	air_conditioner.rs:44-99	CoolingCore embeds HvacEquipment but also adds its own control signal state (ctrl_duty_cycle, ctrl_power_limit_kw, ctrl_mode_override, DR state). This means the "control" layer is split between HvacEquipment (which handles setpoint signals) and CoolingCore (which handles duty cycle, power limit, DR). The responsibility split is unclear — why does HvacEquipment handle MaxCapacityFraction but CoolingCore handles DutyCycle?
F19	Low	core_config.rs:60-65	typed_value only reads from the typed payload, not from raw extras. In production builds, the extract_* functions only consult the typed JSON data. In test builds they fall back to raw extras. This means production code cannot use the raw key-value fallback path at all, which is correct for the migration strategy but could surprise someone debugging a config issue.
---
Area 5: Configuration
Rating: Good
Strengths:
- All config structs use serde(deny_unknown_fields) — typos in config JSON produce clear errors (cooling_config.rs:528-545)
- Defaults are documented with AHRI/OCHRE/DOE references (e.g., hvac_core.rs:25-29 RESNET fan power, hvac_core.rs:486-515 AHRI Cd lookup table)
- Config validation is thorough: capacity > 0, EIR > 0, stage counts match, SHR in 0,1, airflow > 0 (cooling_config.rs:138-270)
- DuctConfig is shared across heating/cooling via #[serde(flatten)] (heating_config.rs:37, heat_pump_config.rs:113)
- The typed config path is strongly preferred: require_typed::<T>() returns a descriptive error when raw config is used (baseboard.rs:366-378)
- Schedule setpoint sources support three levels: CSV column, daily profile, constant (core_config.rs:112-133)
- Speed control mode parsing accepts many aliases for user convenience (core_config.rs:145-175)
Findings:
#	Severity	File:Line	Finding
F20	Medium	cooling_config.rs:10-12 + heat_pump_config.rs:10-12	default_one() duplicated. Two independent definitions of the same function. Should be in core_config.rs or a shared module.
F21	Medium	hvac_core.rs:396-407	Cd is set three times from overlapping config keys. First at line 396-398 (cooling_cd → cd → fallback), then at 399-402 (startup_cd → cooling_cd → cd → fallback), and finally at 487-515 (equipment-type derived defaults). The final block at 487-515 correctly checks explicit_cd.is_none() before overriding, but the earlier assignments at 396-406 may have already set plf_cooling_degradation_coeff and startup.c_d to different values, creating an intermediate inconsistency that's resolved only by the later override. This is correct but fragile.
F22	Low	heat_pump_config.rs:302-393	HeatPumpCoolerConfig is nearly identical to HeatPumpHeaterConfig (~90 fields, ~80% overlap) with the addition of stage_shrs and minor validation differences. A shared HeatPumpCommonConfig flattened into both would reduce the maintenance burden.
F23	Low	cooling_config.rs:108-112 + heat_pump_config.rs:395-399	Equipment type name registration is inconsistent. CentralAirConditionerConfig registers as "Central AC", HeatPumpHeaterConfig as "ASHP Heater", HeatPumpCoolerConfig as "ASHP Cooler". These names must match exactly in EquipmentConfig::from_typed() calls scattered across init methods. A typo in any of these string literals produces a runtime error with no compile-time check.
---
Summary Ratings
Area	Rating	Rationale
Core HVAC structure & control logic	Adequate	Physically correct models with strong test coverage, but significant DRY violations in thermostat FSM and a god-struct problem in HvacEquipment
AC / baseboard / boiler / furnace	Good	Well-factored CoolingCore, correct physics, proper energy conservation; main issue is duplicated variable-speed interpolation
Ideal capacity implementation	Adequate	Correct solver interface and physically reasonable model, but full FSM duplication from hvac_core and missing biquadratic correction in non-ideal fallback
Separation of concerns	Adequate	Clean file-level decomposition and proper control/equipment layering, but thermostat FSM is not factored into a reusable component and HvacEquipment conflates config/runtime/control state
Configuration	Good	Well-typed, validated, documented with standards references; minor duplication in helper functions and config structs
Recommended Actions (Priority Order)
1. Extract ThermostatFsm component — Factor the duplicated FSM logic (update_mode, set_mode, can_transition_mode, schedule resolution, effective_setpoints) from both HvacEquipment and IdealHvac into a shared struct. This eliminates findings F1, F3, F12, F16 and is the highest-impact refactor.
2. Unify variable-speed selection — Make CoolingCore::select_variable_speed_cooling delegate to HvacEquipment::select_speed() or extract a shared interpolate_speed_stages() function. Eliminates F6.
3. Decompose HvacEquipment — Group fields into HvacConfig, HvacRuntimeState, and HvacControlState sub-structs. This makes invariants (like the mode/mode_start_at coupling at F2) enforceable at the type level. Addresses F2, F17.
4. Add ideal_target() to RoomAC — The missing method means RoomAC cannot participate in ideal-capacity solver dispatch. Addresses F7.
5. Consolidate default_one() and shared config helpers — Move to core_config.rs. Addresses F20, F22.