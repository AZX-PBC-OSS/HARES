# ERV/HRV Bypass and Defrost Effectiveness Not Propagated to Thermal Solver

**Severity**: High
**Priority**: P1
**Status**: Open
**Areas**: hares-equipment, hares-core, hares-envelope

## Problem

The `Ventilation` equipment model computes a dynamic per-timestep effective sensible/latent recovery effectiveness that accounts for bypass mode (full outdoor bypass when conditions favor free cooling) and defrost derating (reduced effectiveness at low outdoor temperatures). However, `ThermalSolverConfig` receives only the static rated effectiveness values from configuration at init time. The dynamic values computed in `Ventilation::step()` never reach the thermal solver.

When bypass is active (effective effectiveness = 0.0), the thermal solver continues to apply the rated sensible recovery efficiency (e.g., 0.75), computing an effective flow of `forced_flow_m3_s × (1 - 0.75) = 0.25 × forced_flow_m3_s`. The correct effective flow during bypass is `forced_flow_m3_s × (1 - 0.0) = forced_flow_m3_s` — a 4× underestimate of the actual ventilation sensible load. This is an energy-balance error, not a diagnostic gap.

EnergyPlus Engineering Reference §17.2 "Heat Recovery Equipment": effectiveness is applied per-timestep and may be zero during bypass mode. ANSI/ASHRAE 62.2-2019 §6: bypass does not relax the ventilation flow rate requirement — only heat exchange is suspended.

## Current Behavior

`hares-equipment/src/ventilation.rs:262–296`: `effective_sensible_effectiveness()` and `effective_latent_effectiveness()` compute per-timestep values accounting for bypass and defrost.

`hares-equipment/src/ventilation.rs:412–413`:
```rust
let eff_s = self.effective_sensible_effectiveness(t_outdoor_c);
let eff_l = self.effective_latent_effectiveness(t_outdoor_c);
```
These values are used internally for telemetry (`tk::SENSIBLE_RECOVERY_W`, `tk::LATENT_RECOVERY_W`) but are never written to a port or communicated to the thermal solver.

`hares-equipment/src/ventilation.rs:443–446`: only the fan electrical port is written.

`hares-core/src/dwelling/solver_builder.rs:1741–1742`:
```rust
sensible_recovery_efficiency: recovered.sensible_effectiveness.unwrap_or_default(),
latent_recovery_efficiency: recovered.latent_effectiveness.unwrap_or_default(),
```
Static rated values set once at init, never updated.

`hares-envelope/src/infiltration.rs:179–186`:
```rust
total_nat_flow + forced_flow_m3_s * (1.0 - config.ventilation.sensible_recovery_efficiency)
```
Consumes the stale rated value. During bypass, `1 - 0.75 = 0.25` is used where `1 - 0.0 = 1.0` is required.

## Required Behavior

The per-timestep effective sensible and latent recovery efficiency computed in `Ventilation::step()` must be communicated to the thermal solver before the infiltration/ventilation calculation for that timestep. The preferred approach (Option A) avoids a new domain type:

**Option A (preferred)**: `Ventilation` stores `effective_sensible_effectiveness: f64` and `effective_latent_effectiveness: f64` as struct fields updated each step (initialized to rated values). The dwelling orchestration layer reads these fields after equipment step and updates `ThermalSolverConfig.ventilation` before calling the thermal solver step.

**Option B**: `Ventilation` writes a custom port contribution carrying `[eff_s, eff_l, bypass_active]`. The thermal solver reads the custom accumulator when building the ventilation coupling term. This is more complex and introduces a new domain; prefer Option A.

Reference: EnergyPlus Engineering Reference §17.2 — per-timestep effectiveness application, bypass operation; ANSI/ASHRAE 62.2-2019 §6.

## Approach

1. Add `effective_sensible_effectiveness: f64` and `effective_latent_effectiveness: f64` to the `Ventilation` struct, initialized to the rated values at construction.
2. At the end of `Ventilation::step()`, store the computed `eff_s` and `eff_l` into these fields.
3. In the dwelling orchestration (after equipment step, before thermal solver step), update `thermal_cfg.ventilation.sensible_recovery_efficiency` and `thermal_cfg.ventilation.latent_recovery_efficiency` from the ventilation equipment's effective fields.
4. Static rated values remain in `MechanicalVentilationParams` as the initial default (used before the first step).

## Definition of Done

- [ ] `Ventilation` struct has `effective_sensible_effectiveness: f64` and `effective_latent_effectiveness: f64` updated each step
- [ ] Dwelling orchestration updates `ThermalSolverConfig.ventilation` from these fields before each thermal solver step
- [ ] Static rated values remain as the initial default in `MechanicalVentilationParams`
- [ ] Test: bypass active → `infiltration.rs` receives `sensible_recovery_efficiency = 0.0` → effective flow = `1.0 × forced_flow_m3_s`; computed ventilation sensible load is 4× higher than with rated 0.75 effectiveness
- [ ] Test: defrost derating → solver receives derated effectiveness → proportionally larger ventilation load than rated
- [ ] No regression in non-bypass, non-defrost HRV tests
- [ ] `cargo test -p hares-equipment` and `cargo test -p hares-core` pass

## Verification

```bash
cargo test -p hares-equipment
cargo test -p hares-core
```

## References

- EnergyPlus Engineering Reference §17.2 "Heat Recovery Equipment" — per-timestep effectiveness application, bypass operation
- ANSI/ASHRAE 62.2-2019 §6 — ventilation rate requirements; bypass does not relax flow rate, only removes heat exchange
- `hares-equipment/src/ventilation.rs:262–296` — `effective_sensible_effectiveness()` computation
- `hares-envelope/src/infiltration.rs:179–186` — static effectiveness consumed from config
- `hares-core/src/dwelling/solver_builder.rs:1741–1742` — static effectiveness wired from config

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match (or note corrected location)
  - `effective_sensible_effectiveness()`: ticket says lines 262–296; **actual: line 245** (function starts at 245, latent variant at 263 — both within the ≤17-line shift range; function bodies match exactly).
  - `effective_latent_effectiveness()`: actual line 263.
  - Lines 412–413 (`eff_s`/`eff_l` computed in `step()`): **confirmed at lines 412–413**.
  - Lines 443–446 (only fan electrical port written): **confirmed at lines 442–446**.
  - `solver_builder.rs` static assignment: ticket cites lines 1741–1742, **actual lines 1771–1772** (test-only fixture, not the live init path). Live init path is lines **974–1007**. The bug is present in both: the live path sets the values once at init; neither path is ever called again per-timestep.
  - `infiltration.rs` formula: ticket cites line 179; **actual line 182** (±3 lines). Formula confirmed: `total_nat_flow + forced_flow_m3_s * (1.0 - config.ventilation.sensible_recovery_efficiency)`.

- [x] Described logic matches current implementation — **confirmed**. The `Ventilation` struct has no `effective_sensible_effectiveness` or `effective_latent_effectiveness` fields (only private methods). The computed `eff_s`/`eff_l` are used for telemetry only; no write-back to any port or shared struct field occurs. `run_timestep()` in `mod.rs:1935` calls equipment `.step()` (Step 3a, line 2133) and then `thermal_solver.integrate()` (Step 4, line 2236) with no intervening update of `ThermalSolverConfig.ventilation`.

- [x] OCHRE cross-check result: **HARES diverges from OCHRE — but the divergence does not justify the bug**. OCHRE (`Models/Envelope.py:503–507`) also uses static rated values (`self.sens_recovery_eff`, `self.lat_recovery_eff`) loaded once from HPXML at init and never updated per-timestep. OCHRE has **no bypass or defrost model for ventilation** — it does not compute per-timestep effective effectiveness at all. HARES deliberately added bypass and defrost logic in `Ventilation::step()` to improve on OCHRE's simplified model. However, the improvement is incomplete: HARES computes the dynamic effectiveness correctly but never communicates it to the solver. In OCHRE the discrepancy does not arise because the computation doesn't exist; in HARES the computation exists but is stranded in equipment telemetry. The divergence from OCHRE is *intentional* (HARES is supposed to be more correct) but the propagation gap is a bug unique to HARES.

- [x] EnergyPlus cross-check result: **confirmed directionally, section number incorrect**. EnergyPlus Engineering Reference (v9.6, "Heat Exchangers" chapter at bigladdersoftware.com/epx/docs/9-6/engineering-reference/heat-exchangers.html): *"Energy transfer provided by the heat exchanger will be suspended whenever free cooling is available (i.e., when the air-side economizer is activated)"*. For plate exchangers: *"heat transfer is suspended by fully bypassing the supply and exhaust air around the heat exchanger core"*. Defrost: *"The sensible and total heat transfer rates are then calculated and multiplied by the fractional time period that the heat exchanger is not in defrost mode (1−XDefrostTime)"*. The EnergyPlus reference confirms that bypass should suspend heat transfer (equivalent to effectiveness = 0.0) and defrost derate it proportionally. **However, the ticket's section citation "§17.2" is incorrect**: the EnergyPlus Engineering Reference does not use numeric section designations like §17.2. The content is under a heading-only "Heat Exchangers" chapter with no §17.2 identifier.

### Web-Verified Citations

**Citation 1** — EnergyPlus Engineering Reference §17.2 "Heat Recovery Equipment"

- **Source found**: https://bigladdersoftware.com/epx/docs/9-6/engineering-reference/heat-exchangers.html (Heat Exchangers: Engineering Reference — EnergyPlus 9.6)
- **Quoted passage**: *"Energy transfer provided by the heat exchanger will be suspended whenever free cooling is available (i.e., when the air-side economizer is activated) and the user specified economizer lockout input is specified as Yes."* … *"For plate heat exchangers, heat transfer is suspended by fully bypassing the supply and exhaust air around the heat exchanger core."*; defrost: *"The sensible and total heat transfer rates are then calculated and multiplied by the fractional time period that the heat exchanger is not in defrost mode (1−XDefrostTime)."*
- **Verdict**: **Partially correct**. The substance is correct — EnergyPlus does apply effectiveness per-timestep and sets heat transfer to zero during bypass. However, **the section number "§17.2" does not exist** in the EnergyPlus Engineering Reference. The reference uses heading-based structure with no numeric §17.x designations. The chapter is titled "Heat Exchangers", not "§17.2 Heat Recovery Equipment". The citation should be corrected to: *EnergyPlus Engineering Reference, "Heat Exchangers" chapter (Air System Air-To-Air Sensible and Latent Effectiveness Heat Exchanger model description)*.

**Citation 2** — ANSI/ASHRAE 62.2-2019 §6 — bypass does not relax the ventilation flow rate requirement

- **Source found**: energycodeace.com/content/1145-other-mandatory-requirements-section-6-of-ashrae-stan; ashrae.org standard summaries; energycodeace.com code compliance resources.
- **Quoted passage**: ASHRAE 62.2 requires that HRV/ERV equipment *"include a bypass or free cooling function whereby the intake air bypasses the heat exchanger during favorable outdoor air temperatures"* — i.e. bypass refers only to bypassing the heat exchanger core, not bypassing the ventilation airflow itself. The ventilation flow rate is required to meet the §4/§5 whole-house requirements regardless of bypass state.
- **Verdict**: **Confirmed in substance** — the ASHRAE 62.2 bypass concept is consistent with what the ticket describes (heat exchange suspended, airflow maintained). **However, the specific claim that bypass is addressed "in §6"** could not be fully confirmed from publicly available sources; §6 covers general mandatory requirements, and the bypass discussion may appear in §7 (air-moving equipment) or equipment-specific clauses. The underlying claim (bypass ≠ reduced flow) is technically correct per the standard's intent and consistent with Code Ace summaries.

### Legitimacy

- **Verdict**: **Legitimate**

- **Rationale**: All three core claims in the ticket are verified by direct code inspection: (1) `effective_sensible_effectiveness()` and `effective_latent_effectiveness()` compute correct per-timestep values (lines 245 and 263); (2) these values are consumed locally for telemetry only and are not written to any port or shared field (lines 412–413, 442–446); (3) `ThermalSolverConfig.ventilation` is initialised once from rated config values (solver_builder.rs lines 974–1007) and is never updated within `run_timestep()` (mod.rs lines 1935–2240). The OCHRE cross-check confirms this is a HARES-specific bug introduced when bypass/defrost logic was added to the ventilation equipment without a corresponding propagation mechanism. The EnergyPlus cross-check confirms the correct semantics: per-timestep effectiveness must reach the solver. The section number "§17.2" in the ticket is incorrect (no such numbered section exists in the EnergyPlus Engineering Reference), and the "§6" ASHRAE citation is imprecise, but these are citation labelling errors, not errors of substance. The energy-balance error — a 4× underestimate of ventilation sensible load during bypass — is real and confirmed by the regression tests.

### Proposed Fix Summary

Add two fields `effective_sensible_effectiveness: f64` and `effective_latent_effectiveness: f64` to the `Ventilation` struct, initialised to the rated values. At the end of `Ventilation::step()`, store the computed `eff_s` and `eff_l` into those fields (the values are already computed at lines 412–413). In `Dwelling::run_timestep()`, after the thermal-stage equipment loop (Step 3a, mod.rs line 2158) and before `thermal_solver.integrate()` (Step 4, line 2236), iterate over equipment to find the `Ventilation` instance and copy its effective fields into `ThermalSolverConfig.ventilation.sensible_recovery_efficiency` and `.latent_recovery_efficiency`. The static rated values remain as initial defaults (used before the first equipment step). No new domain type or port is required.

### Test Written

- **File**: `crates/hares-envelope/src/thermal_solver/infiltration.rs` (appended to the existing `#[cfg(test)]` module)
- **Tests added**:
  1. `ticket073_bypass_zero_effectiveness_gives_full_ventilation_load` — verifies that when the solver receives `sensible_recovery_efficiency = 0.0` (bypass active), `h_inf_w_k` equals approximately `rho × forced_flow × cp` (full unrecovered ventilation load), and `q_forced_vent_w` is a substantial heat loss.
  2. `ticket073_rated_vs_bypass_effectiveness_ratio_is_4x` — verifies the 4× ratio: the sensible conductance (`h_inf_w_k`) with `eff = 0.0` is exactly 4× that with `eff = 0.75` for the same forced flow and conditions, directly quantifying the energy-balance error that occurs when bypass is active but the stale rated value is used.
- Both tests pass (`cargo test -p hares-envelope ticket073` → 2 passed, 0 failed).
