# Review 1/6: Core physics + conservation (hares-core step loop, hares-physics, hares-control)

Read-only deep review, post-I-08. Scope read: `docs/PHYSICS_DECISIONS.md`, `docs/invariants-and-observability.md`, `hares-core/src/dwelling/` (step loop 6554-8096, invariants 8586-9006, conversions, autosize), all of `hares-physics/src/`, all of `hares-control/src/`. Cross-checked against vendored EnergyPlus/OCHRE sources where the code cites them. Verified clean: natural-ventilation Cw curve matches the vendored `ZoneEquipmentManager.cc` single-segment formula; warm-up load residual matches `HeatBalanceManager.cc:3350-3364` including the 100 W floor semantics; autosize zero-capacity path fails loudly at the typed-config rebuild via `MissingField`.

## MAJOR

**M1. The thermal-balance conservation invariant is silently skipped when the RC model has no node capacitances.**
- `crates/hares-core/src/dwelling/mod.rs:8762-8786` - `if !q_gains.is_empty() { ... check_thermal(...) }`, comment admits "the thermal check is silently skipped" for capacitance-less fixtures.
- A wiring/assembly defect that drains `node_capacitances` disables exactly the check that would catch it. Production dwellings always have zone-air capacitance (`derive_zone_capacitances`, `boundary_rc.rs:527`), so an empty vector in a real run is itself a defect.
- Fix: fail construction (or the step, naming the model) when a production dwelling has zero node capacitances; keep the skip only behind `#[cfg(test)]` / an explicit fixture flag. At minimum, count it in `RunHealth` per step.

**M2. A tolerated equipment-step rollback keeps the equipment's committed internal state but discards its energy - a one-step conservation hole.**
- `crates/hares-core/src/dwelling/mod.rs:6469-6540` (`record_equipment_step_failure`/`rollback_failed_equipment_ports`): ports restored, warnings pushed, run continues. Per `docs/invariants-and-observability.md` §Equipment Step Failures, state columns "keep its committed state" and the consistency checks skip it that step.
- A thermal device that mutated its own state before erroring (tank node heated by its element, battery SoC advanced) keeps that state while the port draw is rolled back. Step-level energy balance violated by construction; invariants skip the failed equipment so nothing can see it. Bounded by `max_consecutive_step_failures` (default 1) but isolated recurring failures each carry the hole silently into annual totals.
- Fix: snapshot the failed equipment's committed `CoreOutput`/state before `step()` and restore on rollback (checkpointing.rs pattern), or record a `discarded_state_energy_wh` health counter.

**M3. Control-signal value validation is `debug_assert`-only, and the unvalidated external path bypasses even that.**
- `hares-control/src/signal.rs:57-61` (heat < cool), `:70-101` (finite power), `:103-117` (SOC ordering) - all `debug_assert!`. `hares-core/src/dwelling/mod.rs:4654-4662` - `apply_control()` queues any signal with no validation; `apply_control_validated` (`:4698`) validates only equipment acceptance. `hares-control/src/compat.rs:24-88` routes OCHRE-key signals through the constructors.
- Signals are externally produced (Python actors, `apply_control` API, compat bridge). In a release fleet run an inverted setpoint, NaN power, or `min_soc > max_soc` propagates into thermostat FSMs and BMS logic unguarded.
- Fix: unconditional `validate()` on `ControlSignal` called at both queue sites (`control_dispatcher.queue` and `apply_control`), returning `HaresError::InvalidState` naming the field.

**M4. Warm-up day arithmetic truncates to whole seconds and silently degenerates to zero warm-up for sub-second resolutions.**
- `mod.rs:6151-6157` - `steps_per_day = (24*3600).checked_div(time_res.num_seconds()).unwrap_or(0)`; `if steps_per_day == 0 { return Ok(1); }`.
- (a) sub-second `time_res` (`chrono_to_std_duration`, `conversions.rs:622-632`, admits >= 1 ms) silently runs zero warm-up days; (b) non-integral-second resolution (1500 ms) truncates to 1 s -> 86 400 "steps/day" - day-over-day residuals compare misaligned windows.
- Fix: compute from `num_milliseconds()` division; non-integral or zero result is a construction error naming the resolution.

**M5. The natural-ventilation module is an undocumented hybrid of the EnergyPlus model it cites and OCHRE's comfort gating - the discharge coefficient deviates structurally from both.**
- `hares-physics/src/infiltration.rs:379-444`. E+ computes `Cd = 0.40 + 0.0045*|T_ext - T_mat|` (`ZoneEquipmentManager.cc:6040`); HARES uses fixed `Cd = 0.20` single-sided / `0.60` cross (`:54-59`, `:420`). The `adj = (T_zone-T_base)/(T_zone-T_outdoor)` factor (`:433`) and the three gating conditions (`:397-403`) are OCHRE's. Docstring claims "Implements the EnergyPlus `ZoneVentilation:WindandStackOpenArea` model".
- At 5-10 K dT the E+ auto-Cd is 0.42-0.45, so single-sided stack flow is ~2x lower in HARES than in the cited model.
- Fix: adopt E+'s auto-Cd or record the fixed-Cd choice (ASHRAE 2009 Ch.16.14) in PHYSICS_DECISIONS.md and correct the docstring.

## MINOR

- **m1.** Duplicated lower-precision `BTU_PER_HR_PER_W` in autosize: `autosize.rs:123` defines `3.412`; `hares-physics/src/constants.rs:181` defines `3.412_141_633`. Import; delete the local.
- **m2.** AIM-2 pressure exponent silently clamped: `infiltration.rs:171`, `:653` - `n_i.clamp(N_I_MIN, N_I_MAX)` no warning/counter. Validate at spec resolution; keep the clamp only as a non-finite guard.
- **m3.** `expect` on the physics step path: `mod.rs:6078-6091` `apply_occupancy_gains` `.expect("SCHEDULE_DOMAIN_ID payload absent at step time...")`; `borehole.rs:540-543`, `:641` `.expect("borehole history RwLock poisoned")`. Typed `HaresError::InvalidState` at both sites.
- **m4.** Supersaturation silently clamped: `psychrometrics.rs:119-124` `(p_v/p_sat).clamp(0.0, 1.0)`. Document, or return `is_supersaturated` for the caller that needs it.
- **m5.** ASHRAE 152 duct model ships documented placeholders biasing DSE optimistic: `ashrae152.rs:106-115` (`default_insulation_r_ip`), `:163-175` (`seasonal_multiplier` = 1.0), `:383-402` (`soil_temp_at_burial_depth_f`, self-declared placeholder). Surface a `duct_dse_placeholder` warning once per build.
- **m6.** Two water-density conventions: `constants.rs:187-193` (nominal 1000 kg/m3) vs `constants.rs:345-352` (`water_density_kg_m3`, Kell polynomial, validity 0-80 C) - called with water-heater supply temperature; 85 C setpoint silently extrapolates. Route the GPM threshold through `water_density_kg_m3(setpoint)` or document; clamp/error outside 0-100 C.
- **m7.** `Dwelling` is a god object; `run_timestep_inner` is a 1540-line function (`mod.rs:6554-8096`) sequencing nine phases the profiling clock already names, with ordering contracts carried as prose comments (`:8051-8058` "ports.zero() must come AFTER check_step_invariants"). Extract a `StepPipeline` so ordering is structural.

## Top 5 rearchitecture opportunities

1. **Make the step pipeline structural** - extract the nine named phases of `run_timestep_inner` into a `StepPipeline`; the profiling `PhaseClock` boundaries are already the seam. Effort M.
2. **Split `Dwelling` into physics core + satellites** - metering/billing (`:7979-8039`), output/recording (`record_step`, `:8136+`), warnings/health, dispatch into owned sub-structs. Effort L.
3. **Single moisture-accounting channel** - the moisture invariant re-derives the humidity solver's algebra at `mod.rs:8927-8971` (second implementation that will drift; a past tautology bug is already documented there). Move the expected-balance derivation into the humidity solver. Effort M.
4. **Enumerate silent clamps** - generalize `curve_index_clamps` into a `ClampSite` taxonomy (n_i range, RH <= 1, natural-vent adj <= 1, 20-ACH cap, AIM-2 clamps, film floors) drained per step. Effort S.
5. **One home for conversion factors** - a source-scan test (precedent `hares-equipment/tests/si_guard.rs:549`) forbidding redefinitions of names that exist in `hares-physics::constants`. Effort S.

## Validation gaps

- Run-level energy closure (not just per-step): the per-step tolerance `max(1 W, 1e-6 gross)` (`invariants.rs:87`) passes a systematic 0.5 W bias that integrates to ~4.4 kWh/yr. Aggregate residuals per zone, report in kWh and ppm.
- BESTEST in CI with ASHRAE 140 bands for 600/900 plus the L-series low-mass pair (stresses the capacitance wiring M1's skip hides) and unconditioned-zone criteria.
- Rollback divergence metric: forced mid-`step()` water-heater failure leaves tank energy change equal to the metered draw.
- Warm-up arithmetic fixtures: `steps_per_day` at 60/900/3600 s; construction error at sub-second and non-integral resolutions.
- Natural ventilation golden vectors vs a port of `ZoneEquipmentManager.cc:5988-6060`.
- Missing unit tests: `aim2_coefficients_from_ach50` vs a Walker & Wilson (1998) worked example; `duct_leakage_infiltration_m3_s` at the `imb == infil_fan_off` boundary; `soil_temp_at_burial_depth_f` monotonicity; autosize with deliberately zero design load fails loudly; `moist_air_density_kg_m3`/`water_density_kg_m3` domain edges.
- Fleet telemetry: distribution (mean/p99) of thermal- and moisture-balance residuals and warm-up days-run - a drifting mean residual under tolerance is the earliest warning the invariant system can give.
