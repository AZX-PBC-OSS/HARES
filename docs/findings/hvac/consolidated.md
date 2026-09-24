HARES HVAC Quality Assessment — Consolidated Report
Overall Ratings
Domain	Rating	Summary
Core HVAC structure & control	Adequate	Physically correct, but thermostat FSM duplicated across HvacEquipment and IdealHvac (~150 lines DRY violation); HvacEquipment is a god-struct with ~49 fields
AC / furnace / boiler / baseboard	Good	CoolingCore properly shared, energy conservation correct, biquadratic curves well-validated
Ideal capacity	Adequate	Solver interface correct, but non-ideal fallback ignores biquadratic correction (delivers rated capacity regardless of outdoor temp — wrong for heat pumps)
Heat pump / minisplit	Adequate	Defrost, backup ER lockout, and SHR modeled; missing discrete defrost cycle, heating-side SHR, default biquadratic curves, and variable-speed compressor map
Physics & envelope coupling	B+	Solid ASHRAE-aligned physics, but h_fg inconsistency across modules (2,454,000 vs 2,501,000 J/kg) creates systematic energy imbalance, and no humidity port variant means latent→humidity coupling may be inconsistent
Telemetry & debuggability	Adequate/Needs Improvement	Good telemetry infrastructure, but thermostat FSM produces zero tracing; 6 OCHRE-equivalent output columns missing; no decision logging
Top 5 Critical Findings
1. Thermostat FSM Duplication (DRY violation, maintenance risk)
hvac_core.rs:654-733 duplicates ideal_hvac.rs:268-364 — identical update_mode, can_transition_mode, set_mode, and setpoint resolution. A ThermostatFsm shared component would eliminate ~150 lines and divergence risk.
2. h_fg Inconsistency / Missing Humidity Port (energy conservation)
Dehumidifier hardcodes h_fg = 2,454,000 (20°C), thermal solver uses 2,501,000 (0°C via ASHRAE). The PortContribution enum has no Humidity variant — equipment writes latent energy (W) but not humidity ratio change, so the humidity solver must derive kg/s using potentially different h_fg. ~1.9% systematic latent energy imbalance.
3. Thermostat Decision Invisibility
Zero tracing calls in thermostat FSM. Cannot diagnose why mode transitions happen (or are blocked by min-cycle-time). Rating: Poor for control decision visibility.
4. Ideal HVAC Non-Ideal Fallback Ignores Biquadratic Curves
ideal_hvac.rs:518-528 — when use_ideal_cached == false, falls back to rated capacity ignoring outdoor temperature effects. Heat pumps delivering full rated capacity at -15°C outdoor is physically wrong.
5. Duplicated Variable-Speed Selection
air_conditioner.rs:335-397 reimplements the same speed-stage interpolation as staging.rs:202-249.
Top 5 Gaps vs EnergyPlus
1. No discrete defrost cycle — HARES uses OCHRE's steady-state average-effect model; E+ models the actual reverse-cycle melt process
2. No heating-side SHR — heating coil SHR assumed = 1.0; E+ computes heating bypass factor for dehumidification during heating
3. No supply air temperature model — heat injected directly to zone node; no air-side mixing
4. No dynamic duct model — DSE is static (computed once at init); E+ varies with zone temps
5. No variable-speed compressor map — variable-speed equipment uses ideal capacity mode despite having EIR curves
Key OCHRE Patterns to Adopt
- hvac_mult sign-unification for deadband/thermostat (halves branching)
- ADP iteration for SHR (EnergyPlus standard, OCHRE has JIT-compiled version)
- Winkler startup degradation model (already partially present)
- ER lockout with hard/soft timers (production-quality staging)
- Zone fraction distribution (duct losses go to duct zone, not outdoor)
Key OCHRE Mistakes to Avoid
- Mixed int/float speed_idx — HARES correctly separates speed_index: u8 + speed_frac: f64
- Static DSE — HARES should implement dynamic duct model
- One-step lag in ideal capacity back-solve — HARES should use current-step estimates
- Silent biquadratic clipping at extremes — HARES should warn
- Crankcase/pan heater power not adding to zone heat — energy accounting gap
Recommended Priority Actions
Priority	Action
P0	Extract ThermostatFsm shared component
P0	Unify h_fg constant; add Humidity port variant
P0	Add thermostat decision tracing::debug! logging
P1	Fix ideal HVAC non-ideal fallback to use biquadratic curves
P1	Add missing output columns (SHR, speed, component power, duct losses, RTF) at v7
P2	Decompose HvacEquipment into config/runtime/control sub-structs
P2	Promote key HVAC fields from telemetry to CoreOutput
P3	Implement dynamic duct model
P3	Add discrete defrost cycle state machine
The heat pump/minisplit agent also noted that test coverage for HVAC is present but no tests verify telemetry→output wiring correctness, and the observe feature (richest debugging tool) is gated off by default.