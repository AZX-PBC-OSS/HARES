HARES Heat Pump & Minisplit Quality Assessment Report
Executive Summary
The HARES heat pump implementation is a well-structured, physically grounded model that covers the core residential heat pump behaviors with strong OCHRE parity. The code is well-factored, extensively documented with provenance references, and has substantial unit test coverage. However, several significant physics gaps remain relative to EnergyPlus fidelity, particularly around variable-speed inverter compressor physics, heating-side SHR/latent modeling, and condensate/frost accumulation dynamics.
Overall Rating: B+ — Good for residential energy simulations; needs targeted improvements for EnergyPlus-grade accuracy.
---
Area 1: Heat Pump Implementation
1.1 COP Variation with Outdoor Temperature, Indoor Conditions, and Compressor Speed
Strengths:
- Biquadratic curves are correctly implemented for both capacity and EIR as functions of (indoor WB, outdoor DB), matching EnergyPlus DX coil convention (heater.rs:866-883, heater.rs:917-934).
- Inter-stage biquadratic interpolation is properly handled for multi-speed/variable-speed (heater.rs:873-883, heater.rs:923-934), interpolating both capacity and EIR curves between bracket stages.
- Part-load factor (PLF) correctly degrades cycling efficiency per AHRI 210/240: PLF = 1 - Cd*(1-PLR) (staging.rs:296-298), with configurable Cd and PLF floor clamping (staging.rs:309).
- Variable-speed MSHP gets SpeedControlMode::VariableSpeedIdeal which forces PLF=1.0 (no cycling penalty — correct for inverter-driven equipment) (staging.rs:274-279).
- Startup capacity degradation uses the Winkler (2009) exponential ramp model (speed_control.rs:71-97), with c_d=0.0 for variable-speed MSHP (no startup ramp — correct) (cooler.rs:429-430). Winkler, J.M. (2009). "Development of a Component Based Simulation Tool for the Steady State and Transient Analysis of Vapor Compression Systems." Ph.D. dissertation, University of Maryland. https://drum.lib.umd.edu/handle/1903/9493
Gaps:
- ⚠️ FINDING MEDIUM: The EIR biquadratic curves use identity defaults [1,0,0,0,0,0] when no CSV data is provided (hvac_core.rs:23). This means COP is constant regardless of outdoor temperature unless the user supplies curve data. EnergyPlus always uses manufacturer-specific or AHRI-class biquadratic curves. The identity default gives physically unrealistic constant COP at all temperatures. File: hvac_core.rs:23, heater.rs:866
- ⚠️ FINDING LOW: The capacity biquadratic also defaults to identity, meaning rated capacity is preserved at all temperatures. Real HP capacity drops ~30-50% from 47°F to 17°F. Users must supply curves for physical results. File: hvac_core.rs:23
- ⚠️ FINDING LOW: No explicit low-temperature cutoff curve exists beyond the hard lockout at hp_lockout_temp_c (-17.78°C default). EnergyPlus models a gradual capacity/EIR degradation curve that naturally reduces COP toward zero near the lockout, rather than an abrupt cliff. File: constants.rs:34
1.2 Supplementary/Emergency Heat Staging
Strengths:
- Three distinct HP operating modes: HeatingHP, HeatingER, HeatingHPAndER — correctly models HP-only, backup-only, and combined operation (heater.rs:676-681).
- ER lockout hierarchy is comprehensive with three gates:
  1. Temperature gate: er_lockout_temp_c (OCHRE default 4.44°C) — ER blocked above this (heater.rs:1249)
  2. EnergyPlus supplemental cap: max_oat_supplemental_c (hard cap 21°C) — ER blocked above this even if OCHRE threshold permits it (heater.rs:1250, constants.rs:47)
  3. ER hard lockout after setpoint increase (heater.rs:1175-1183) with configurable duration
  4. ER soft lockout while zone temp is rising (heater.rs:1188-1210)
- ER thermostat hysteresis with distinct turn-on/turn-off thresholds prevents short-cycling (heater.rs:1257-1263).
- Fuel backup (gas/propane/oil) correctly routes to fuel ports with appropriate EIR (heater.rs:39-44, heater.rs:1055-1060).
- Power limit shedding correctly sheds ER first (not modulatable) before scaling HP proportionally (heater.rs:1097-1129).
Gaps:
- ⚠️ FINDING LOW: ER capacity is modulated by PLR when in non-ideal mode (heater.rs:1039), but electric resistance heat is inherently on/off — partial modulation is unrealistic for strip heat. The er_capacity_w = backup_capacity_w * plr formula at heater.rs:1039 implies variable ER output, which is only possible with multi-stage strip heat (rare in residential) or triac-based SCR control. EnergyPlus models ER as strictly binary unless explicitly configured for staged operation.
1.3 Defrost Logic (Timing, Demand, Energy Penalty)
Strengths:
- Two defrost control modes: OnDemand (humidity-based, OCHRE default) and Timed (DOE-2), both implemented with correct EnergyPlus-derived formulas (defrost.rs:17-24).
- OnDemand defrost computes time_fraction from outdoor coil moisture accumulation (defrost.rs:137-138), using the EnergyPlus formula 1 / (1 + 0.01446/delta_omega).
- Timed defrost uses DOE-2 capacity/power multiplier formulas with humidity-dependent slopes (defrost.rs:180-185), clamped to 0,1 for high-humidity conditions.
- Two defrost strategies: ReverseCycle (common modern HPs) and Resistive (defrost.rs:28-33), each with distinct energy accounting.
- ReverseCycle correctly produces both q_defrost_w (capacity loss) and extra_power_w (compressor work + EIR modifier) (defrost.rs:188-213).
- Resistive correctly has zero q_defrost_w but positive extra_power_w from the heater element (defrost.rs:215-221).
- Optional biquadratic EIR curve for Timed+ReverseCycle mode with 15.555°C input floor (defrost.rs:194-205), matching EnergyPlus.
- Runtime fraction scaling: Defrost power is proportional to compressor RTF (defrost.rs:207-211), physically correct.
- 15 focused unit tests covering both modes, edge cases, clamping, and regression values (defrost.rs:236-607).
Gaps:
- ⚠️ FINDING MEDIUM: No explicit defrost accumulator with state — the model is stateless per timestep. EnergyPlus tracks accumulated frost and transitions into/out of defrost cycles with hysteresis (defrost duration, recovery period). HARES computes a time_fraction but doesn't model discrete defrost on/off cycles. The defrost_accumulator_s field in HeaterCore (heater.rs:96) exists but is only used for telemetry, not for cycle control. File: heater.rs:96, defrost.rs:112-233
- ⚠️ FINDING LOW: No demand-defrost with temperature-based trigger. EnergyPlus also offers a temperature-based demand defrost mode where defrost triggers when the outdoor coil temperature drops below a threshold (more common than humidity-based in real controllers). HARES only has OnDemand (humidity) and Timed. File: defrost.rs:17-24
1.4 Minisplit Variable-Speed/Inverter Behavior
Strengths:
- MSHP speed model: VariableSpeedIdeal correctly models inverter-driven variable-speed operation with PLF=1.0 (no cycling penalty), Cd=0.0 (no startup ramp), and 4-stage capacity interpolation (heater.rs:517-525, cooler.rs:39-49).
- MSHP-specific defaults: Pan heater (150W @ 0°C), crankcase heater (15W @ 0°C), DSE=1.0 (ductless), zero backup by default (constants.rs:59-62, cooler.rs:19-22).
- Companion RTF coordination: Heating coil RTF is passed to the cooler for crankcase power accounting (cooler.rs:29-32, heater.rs:253-255).
- Ductless DSE=1.0 correctly applied for MSHP (heater.rs:526).
Gaps:
- ⚠️ FINDING HIGH: No inverter minimum/modulation limit. Real MSHPs have a minimum compressor speed (~25-40% of rated) below which they cannot operate. The HARES model allows the interpolated capacity to go as low as 25% of rated (the lowest of the 4 synthetic stages at heater.rs:523), but below that it cycles on/off at the lowest stage. This is actually somewhat realistic for units with a 25% minimum, but the minimum is hardcoded at 25% rather than configurable. EnergyPlus allows explicit Minimum Outdoor Dry-Bulb Temperature for Compressor Operation and minimum flow fraction. File: heater.rs:522-524
- ⚠️ FINDING MEDIUM: No MSHP cooling-side SHR model. The cooling coil's SHR for MSHP is handled identically to central AC (via the parent AirConditioner), but MSHPs have different SHR characteristics at low speeds (SHR increases at lower compressor speeds due to reduced latent capacity). EnergyPlus models this via speed-dependent SHR curves. The stage_shrs config field exists (heat_pump_config.rs:338-339) but MSHP auto-generates 4 identical EIR stages from a single EIR value (heater.rs:524), meaning SHR doesn't vary with speed. File: heater.rs:522-525
- ⚠️ FINDING LOW: No MSHP heating-side SHR/sensible model. HP heating is treated as 100% sensible (heater.rs:698-700 with latent_gain_w=0.0). While heating coils are predominantly sensible, there is a small latent component from outdoor coil condensation that EnergyPlus models. File: heater.rs:696-701
---
Area 2: Coil Physics (coil_physics.rs)
Strengths:
- Correct dry-air mass flow basis (coil_physics.rs:425-441). The comment explicitly documents the OCHRE deviation and references ASHRAE HOF Ch.1 §1.8. This is a legitimate physics improvement over OCHRE.
- Enthalpy-based bypass factor (coil_physics.rs:396-402). Uses the enthalpy form BF = (h_out - h_ADP)/(h_in - h_ADP) rather than the temperature form, correctly accounting for latent heat transfer. This matches EnergyPlus and is physically superior.
- ADP iterative solver uses bisection-halving with sign change detection (coil_physics.rs:352-375), matching OCHRE/EnergyPlus.
- SHR round-trip consistency: coil_ao_factor → calculate_shr recovers the input SHR within ±0.002 (coil_physics.rs:986-995).
- Bypass factor floor at 0.01 prevents negative/zero BF (coil_physics.rs:195, coil_physics.rs:383-384).
- Robust edge-case handling: dry coil (W < 1e-7), zero capacity, zero airflow all return sensible defaults.
Gaps:
- ⚠️ FINDING LOW: The iterate() root-finder (coil_physics.rs:443-548) is a complex Muller/secant hybrid ported from OCHRE with subtle edge cases (division by zero guards, mode fallbacks). While functional, it has a 50-iteration limit and could benefit from a Brent's method implementation for more robust convergence. No correctness issue observed, but complexity is high.
---
Area 3: Latent/Dehumidification
3.1 Henderson-Rengarajan Latent Degradation (coil_physics.rs:96-194)
Strengths:
- Full H-R 1996 model implemented: twet_rated, gamma_rated, max_cycling_rate, latent_time_constant — all four required parameters (coil_physics.rs:41-54).
- Correct scaling of twet and gamma to actual conditions via rated/actual latent ratio and DB-WB depression ratio (coil_physics.rs:122-132).
- Companion heating RTF correctly shortens effective off-time when heating runs during cooling off-cycles (coil_physics.rs:161-165), reducing degradation — physically correct per H-R.
- To fixed-point solver with convergence check (coil_physics.rs:172-179), 20-iteration limit.
- SHR clamped to steady_state_shr, 1.0 — degradation can only increase SHR (reduce latent removal), never below steady-state (coil_physics.rs:193).
- 12 comprehensive tests including regression against hand calculations (coil_physics.rs:894-909).
Gaps:
- ⚠️ FINDING MEDIUM: The latent degradation model is only applied to the cooling coil (via AirConditioner). The heating coil has no latent modeling at all — heating output is treated as 100% sensible (heater.rs:698-700). EnergyPlus Coil:Heating:DX models heating-side SHR (typically 0.95-1.0) and includes latent effects from outdoor coil frosting. For cold-climate heat pumps, the outdoor coil frost/defrost latent energy exchange can represent 5-10% of heating energy. File: heater.rs:696-701
3.2 Standalone Dehumidifier (dehumidifier.rs)
Strengths:
- Complete model with biquadratic water-removal and energy-factor curves, RH-based deadband control, and proper energy balance (sensible_gain = latent_removal + electric_power, dehumidifier.rs:186).
- Correct sign convention: latent removal is negative (removes moisture from zone), sensible gain is positive (adds heat to zone) (dehumidifier.rs:332-339).
- 9 comprehensive tests covering off/on/energy-balance/state-round-trip/control-signals (dehumidifier.rs:564-867).
Gaps:
- ⚠️ FINDING LOW: Default biquadratic curves are identity (dehumidifier.rs:38), meaning water removal and energy factor are constant regardless of temperature/RH. Real dehumidifier performance varies significantly with conditions. This is a configuration gap, not a physics gap.
---
Area 4: Duct Distribution (duct_distribution.rs)
Strengths:
- ASHRAE 152 DSE calculation integrated via hares_physics::ashrae152::calculate_dse (helpers.rs:265), with full zone-type enumeration (16 types) and all duct parameters.
- Multi-zone routing correctly distributes conditioned zone, basement zone, and duct zone fractions that sum to 1.0 (duct_distribution.rs:22-63).
- Zone deduplication when basement_zone == duct_zone (duct_distribution.rs:68-81).
- DuctLoss thermal category distinguishes delivered heat from duct losses in diagnostics (duct_distribution.rs:107-119).
- Ducts-in-conditioned-space correctly forces DSE=1.0 with a warning (duct_distribution.rs:25-36).
- 11 targeted tests with precise expected values.
No significant gaps identified. This is a mature, well-tested module.
---
Area 5: Equivalent Battery Model (equivalent_battery.rs)
Strengths:
- Correct OCHRE parity: E_dot = eta * P - P_b with eta=1.0, P_b=0.0 for HVAC (equivalent_battery.rs:9).
- Deadband fill fraction derived from zone temperature position within the thermostat deadband (equivalent_battery.rs:55-69).
- Both heating and cooling modes correctly handled with opposite fill directions (equivalent_battery.rs:44-74).
Gaps:
- ⚠️ FINDING LOW: The model uses max_energy_kwh = max_power_kw * time_res_s / 3600 which is the energy deliverable in one timestep, not the actual thermal storage capacity of the building. EnergyPlus uses the building thermal mass (from the zone model) for the EBM, which can be 10-100× larger than the single-timestep energy. This limits the EBM's usefulness for demand-response optimization. File: equivalent_battery.rs:54
---
Area 6: Helpers (helpers.rs)
Strengths:
- Well-organized with clear config access policy documented (helpers.rs:1-8).
- Fuel type parsing covers all variants with case-insensitive matching (helpers.rs:95-104).
- Duct DSE resolution with ASHRAE 152 fallback is properly layered (helpers.rs:210-266).
- Shared heating control logic prevents duplication across furnace/boiler/baseboard (helpers.rs:131-189).
No significant gaps.
---
Area 7: Configuration (heat_pump_config.rs)
Strengths:
- Comprehensive typed config with serde(deny_unknown_fields) preventing typos (heat_pump_config.rs:19).
- Validation for positive/finite values on all capacity and efficiency fields (heat_pump_config.rs:205-299).
- MSHP speed forcing properly deferred to equipment init (not config) so config records user intent (heat_pump_config.rs:17).
- Biquadratic bounds (x1/x2 min/max) and flow-fraction/PLF bounds exposed for curve evaluation control (heat_pump_config.rs:115-138).
- 7 config tests including round-trip, validation, and effective-speed logic.
Gaps:
- ⚠️ FINDING MEDIUM: No defrost configuration fields in HeatPumpHeaterConfig. The defrost mode, strategy, time fraction, and EIR curve must be configured via raw config keys rather than typed fields. This creates an inconsistency where some HP parameters are typed and validated while defrost parameters are not. File: heat_pump_config.rs — entire struct has no defrost fields
- ⚠️ FINDING LOW: No stage_heating_shrs field exists (unlike stage_cooling_shrs on the cooler config). Heating-side SHR per stage cannot be configured. File: heat_pump_config.rs vs heat_pump_config.rs:338-339
---
Area 8: Test Coverage
Unit Tests (Inline)
Module	Test Count	Quality
heater.rs	~30	Excellent — ER lockout, defrost, MSHP, DR, mode override, state round-trip
cooler.rs	~12	Good — MSHP crankcase, variable speed, deadband, typed config
defrost.rs	15	Excellent — both modes, edge cases, clamping, regression
coil_physics.rs	~25	Excellent — ADP, BF, SHR round-trip, latent degradation
duct_distribution.rs	11	Excellent — multi-zone, dedup, DSE, thermal categories
dehumidifier.rs	9	Good — off/on, energy balance, state round-trip
speed_control.rs	7	Good — Winkler ramp, PLR
staging.rs	~15	Excellent — PLF, interpolation, two-speed, multi-speed
heat_pump_config.rs	7	Good — round-trip, validation, MSHP speeds
Integration Tests
File	Coverage
hvac_tests.rs	ASHP COP, MSHP defaults/pan heater, sub-consumption telemetry, bang-bang cycling, 2-speed ASHP escalation
hvac_parity.rs	ASHP COP vs OAT, defrost COP drop, AC first-law, gas furnace AFUE/DSE, 2-speed runtime, MSHP startup penalty
Gaps:
- ⚠️ FINDING HIGH: No multi-step transient test that runs a heat pump through a full heating season day (e.g., 24 hours with varying OAT) and checks cumulative energy, defrost cycling, and mode transitions. All tests are 1-30 step snapshots.
- ⚠️ FINDING MEDIUM: No test for MSHP heating in integration tests. hvac_tests.rs and hvac_parity.rs have MSHP cooling tests but not MSHP heating with variable-speed behavior.
- ⚠️ FINDING MEDIUM: No test for combined HP+ER mode (HeatingHPAndER) in integration tests. This is a critical operating mode for cold-climate ASHPs.
---
Top 5 Most Impactful Gaps Relative to EnergyPlus
1. 🔴 No Default Biquadratic Performance Curves (Impact: HIGH)
Files: hvac_core.rs:23, heater.rs:866-934
EnergyPlus requires manufacturer-specific or AHRI-class biquadratic curves for every DX coil. HARES defaults to identity curves [1,0,0,0,0,0], meaning COP and capacity are constant regardless of outdoor temperature. This produces:
- COP = 1/EIR at all temperatures (no degradation at low OAT)
- No capacity loss at low OAT (real HPs lose 30-50% capacity at 17°F vs 47°F)
- Defrost penalty is the only mechanism that reduces COP at low OAT
Impact on residential simulations: Annual energy consumption errors of 20-40% for heating in cold climates without user-supplied curves.
Recommendation: Ship AHRI-class default curves (e.g., DOE-2 type curves from ResStock/OCHRE CSV files) for common HP categories.
2. 🔴 No Discrete Defrost Cycle Modeling (Impact: HIGH)
File: defrost.rs:112-233
HARES computes a continuous time_fraction each timestep but never transitions the equipment into a discrete defrost state with distinct capacity/EIR behavior. EnergyPlus models explicit defrost ON/OFF cycles with:
- Defrost ON: compressor reverses, capacity drops to zero or near-zero, extra power consumed
- Defrost recovery: brief capacity boost as coil clears
- Accumulation period: frost builds gradually between defrost cycles
HARES's continuous multiplier approach averages the defrost penalty across the timestep rather than modeling the transient. This underestimates the peak power draw during defrost and overestimates the average capacity during frost accumulation.
Impact: 5-15% error in peak demand during defrost, 2-5% in annual heating energy in cold-humid climates.
3. 🟡 No Heating-Side SHR/Latent Model (Impact: MEDIUM)
File: heater.rs:696-701
HP heating is modeled as 100% sensible. EnergyPlus Coil:Heating:DX includes a heating SHR (typically 0.90-1.0) that accounts for:
- Condensation/frost on the outdoor coil consuming latent energy
- Moisture carried from outdoor to indoor during defrost recovery
- Small latent effects from supply-air temperature differences
For cold-climate ASHPs, the outdoor coil frost/defrost latent energy exchange can represent 5-10% of heating energy, affecting both humidity and energy balance.
4. 🟡 No Configurable MSHP Minimum Compressor Speed (Impact: MEDIUM)
File: heater.rs:522-524
Real MSHPs have a minimum compressor speed (~25-40% of rated). HARES hardcodes 4 stages at 25%/50%/75%/100% of rated capacity. Below 25% load, the unit cycles at the 25% stage. EnergyPlus allows explicit Minimum Flow Fraction and Number of Speeds with per-speed capacity/EIR. The hardcoded 25% minimum may not match specific equipment.
5. 🟡 No Defrost Configuration in Typed Config (Impact: MEDIUM)
File: heat_pump_config.rs
Defrost parameters (mode, strategy, time fraction, EIR curve) must be configured via raw key-value pairs rather than typed struct fields. This creates:
- No compile-time validation of defrost settings
- Inconsistency with other HP parameters that are typed and validated
- Risk of misconfiguration (e.g., specifying defrost_control as "ondemand" vs "OnDemand")
---
Code Quality Assessment
Aspect	Rating	Notes
Structure & Modularity	A	Excellent separation: heater/cooler/defrost/constants/config as separate modules
DRY	A-	Minor duplication between HeatPumpHeaterConfig and HeatPumpCoolerConfig (38 shared fields)
Provenance Documentation	A+	Every formula references OCHRE line numbers, EnergyPlus docs, or ASHRAE sections
Error Handling	A	Comprehensive validation with descriptive error messages
State Management	A-	Postcard serialization with careful INFINITY handling (heater.rs:1443-1447)
Naming Conventions	A	Clear SI-unit suffixes (_c, _w, _m3_s, _pa, _kw)
Type Safety	A	Typed configs with serde(deny_unknown_fields), exhaustive match arms
---
Summary Table
Area	Physics Correctness	Code Quality	EnergyPlus Parity	Test Coverage
HP Heater (heating)	B+	A	B-	B+
HP Cooler (cooling)	A-	A	B	B+
Defrost	B	A+	B-	A
MSHP Variable-Speed	B	A	C+	B
Coil Physics	A	A-	A-	A
Latent Degradation	A	A	A	A
Dehumidifier	B+	A	B	B+
Duct Distribution	A	A+	A	A+
Equivalent Battery	B	A	C	B
Configuration	B	A	B	B+
Confidence level: High for structure and code quality; Medium for specific EnergyPlus numerical parity (would need side-by-side annual simulation comparison to confirm).