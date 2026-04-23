# Window Glass-Absorbed Solar Uses RC Voltage-Divider Ratio Instead of N_i Inward Fraction

**Severity**: Low
**Priority**: P3
**Status**: Open
**Areas**: hares-envelope/thermal_solver/solar.rs, hares-envelope/thermal_solver/config.rs

## Problem

`apply_solar_inputs` at `crates/hares-envelope/src/thermal_solver/solar.rs:56` computes the inward-flowing fraction of glass-absorbed solar as:

```rust
let absorbed_inward = (shgc - transmittance).max(0.0) * win.radiation_frac;
```

`win.radiation_frac` is the RC network surface-film voltage-divider ratio defined in `WindowSolarProperties` at `crates/hares-envelope/src/thermal_solver/config.rs:173`. That ratio splits heat between the RC surface node and zone air for longwave radiation exchange — it is not the N_i inward fraction of glass-absorbed solar defined by the EnergyPlus window model.

The EnergyPlus window heat balance (Engineering Reference §14.7 "Window Heat Balance") defines:

```
Q_glass_inward = (SHGC - T) × N_i × A × E_poa
```

where N_i is the inward fraction of absorbed solar derived from interior and exterior film coefficients:

```
N_i = h_ci / (h_ci + h_co)
```

Under NFRC 100-2020 §6.3 rating conditions (h_ci = 8.3 W/m²·K, h_co = 34.0 W/m²·K):

```
N_i = 8.3 / (8.3 + 34.0) ≈ 0.196
```

The `radiation_frac` for a typical residential window RC network is 0.3–0.8. Using it in place of N_i overestimates the inward heat flux by a factor of 1.5–4×. For a south-facing window with 200 W of glass-absorbed solar during peak cooling:

```
error ≈ 200 × (radiation_frac - N_i) ≈ 200 × (0.5 - 0.196) ≈ 61 W
```

This spurious heat is added to the zone air node, inflating peak cooling loads.

`WindowSolarProperties` in `config.rs` has no `n_i_inward_fraction` field; no N_i calculation or storage exists anywhere in the window configuration path.

## Current Behavior

`crates/hares-envelope/src/thermal_solver/solar.rs:56-57`: `(shgc - transmittance).max(0.0) * win.radiation_frac` — `radiation_frac` used as N_i proxy.

`crates/hares-envelope/src/thermal_solver/config.rs:159-176`: `WindowSolarProperties` struct has no `n_i_inward_fraction` field.

OCHRE `Window.py` uses a fixed `radiation_frac = 0.5` for this calculation — also incorrect but a different value. Neither matches the EnergyPlus N_i formula. The EnergyPlus Engineering Reference §14.7 is the authoritative source.

## Required Behavior

1. Add `n_i_inward_fraction: f64` to `WindowSolarProperties` in `crates/hares-envelope/src/thermal_solver/config.rs`. This field represents N_i as defined in EnergyPlus Engineering Reference §14.7, not the RC voltage-divider ratio.

2. Replace the computation in `solar.rs:56` with:
   ```rust
   let absorbed_inward = (shgc - transmittance).max(0.0) * win.n_i_inward_fraction;
   ```

3. In the HPXML solver builder, derive `n_i_inward_fraction` from the window's interior and exterior film coefficients when available from the HPXML input. When not available, use 0.196 as the NFRC 100-2020 §6.3 standard rating condition default. This default must not be silent: the builder must emit `tracing::debug!` indicating that the NFRC default N_i = 0.196 is being used for the named window surface.

4. `radiation_frac` in `WindowSolarProperties` retains its existing role in the RC network voltage-divider split for longwave exchange. The two fields serve different physics; the field names must not be conflated in comments or documentation.

Primary citations:
- EnergyPlus Engineering Reference §14.7 "Window Heat Balance" — N_i inward fraction derivation from film coefficients
- NFRC 100-2020 §6.3 — standard rating condition film coefficients h_ci = 8.3 W/m²·K, h_co = 34.0 W/m²·K
- ASHRAE Handbook of Fundamentals 2021 Ch. 15 §15.27 "Window and Door Thermal Transmittance" — SHGC and transmittance relationship

## Definition of Done

- [ ] `n_i_inward_fraction: f64` field added to `WindowSolarProperties`
- [ ] `absorbed_inward` computed using `win.n_i_inward_fraction` in `solar.rs:56`
- [ ] HPXML builder derives `n_i_inward_fraction` from film coefficients when available, otherwise uses 0.196 with `tracing::debug!`
- [ ] Test: window with SHGC = 0.25, transmittance = 0.21, h_ci = 8.3, h_co = 34.0 — absorbed inward heat per unit area matches EnergyPlus §14.7 reference value within 5%
- [ ] Test: `n_i_inward_fraction` and `radiation_frac` differ for a representative window configuration (guards against future conflation)

## Verification

```bash
cargo test -p hares-envelope solar
cargo test -p hares-envelope window
```
