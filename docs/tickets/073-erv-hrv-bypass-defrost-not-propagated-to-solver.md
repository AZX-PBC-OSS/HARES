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
