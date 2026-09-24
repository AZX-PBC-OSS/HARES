# Ground Temperature Uses Surface-Layer DOE-2 Value; Kusuda-Achenbach Is Dead Code at Runtime

**Severity**: Critical
**Status**: Open
**Areas**: hares-envelope/thermal_solver, hares-io/epw, hares-core/environment, hares-physics/ground

## Problem

Every foundation wall, slab boundary, and ground-connected node receives
`env.weather.ground_temp_c` as its driving temperature. This scalar comes from the DOE-2 surface
ground temperature model (`interpolate_ground_temp_c`, `crates/hares-io/src/epw.rs:607`), which is
a sinusoidal fit to monthly mean ambient temperatures at the ground surface (depth ≈ 0 m, with the
10 m depth factor as implemented — see ticket 024 for that sub-defect). It carries no depth
attenuation or phase lag relative to the boundary centroid depth.

The Kusuda-Achenbach model — which correctly computes depth-attenuated, phase-shifted ground
temperature at any depth — exists in `crates/hares-physics/src/ground.rs:63–80` and is accessible
via `EnvironmentManager::ground_temp_at_depth_c` (`crates/hares-core/src/environment.rs:339–348`).
Neither is called during a simulation timestep. The thermal solver and solver_builder use only the
single scalar `env.weather.ground_temp_c`.

Three call sites consume the surface value uncorrected:
- `crates/hares-envelope/src/thermal_solver/mod.rs:701`: `u[idx] = env.weather.ground_temp_c`
- `crates/hares-envelope/src/thermal_solver/stepping.rs:179`:
  `DrivingTemp::Ground => self.cached_ground_temp_c`
- `crates/hares-envelope/src/thermal_solver/longwave.rs:259`:
  `DrivingTemp::Ground => env.weather.ground_temp_c`

A second defect feeds into this: `crates/hares-core/src/environment.rs:253–254` derives the
Kusuda-Achenbach parameters as:
```
ground_t_mean_c: mains_t_annual_avg_c,
ground_t_amplitude_c: mains_dt_annual_range_c / 2.0,
```
reusing the Burch-Christensen mains water temperature inputs (from `compute_mains_inputs` at
`environment.rs:683`). The reuse is physically reasonable when the monthly-mean path executes, but
the DOE-2 surface temperature (`ground_temp_c` in `WeatherTimeSeries`) is applied to all below-grade
boundaries instead of the Kusuda result, making the parameter derivation moot at runtime.

## Physics Error

Below-grade heat transfer depends on temperature at the centroid depth of the foundation element,
not the surface. Per Kusuda and Achenbach (1965) ASHRAE Transactions 71(1):61-74 and EnergyPlus
Engineering Reference §12.6:

- Full basement (2.4 m below grade), Minneapolis climate, January:
  DOE-2 surface ground temp ≈ -5 °C (tracks outdoor air). Kusuda-Achenbach at 2.4 m ≈ +4 °C
  (attenuated, ~30-day phase lag). Boundary condition error: 9 °C.
- For a 1,500 ft² conditioned basement at U ≈ 0.35 W/m²·K, a 9 °C error in January produces
  roughly 1,000 kWh of excess heating energy in that month alone.
- In summer the error reverses sign: the DOE-2 surface is warm, but at 2.4 m the soil is cool,
  reversing the computed heat flow direction.

ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.31 (Below-Grade Heat Transfer) specifies that
undisturbed ground temperature at the relevant foundation depth — not surface temperature — is the
correct thermal boundary condition for basement walls, slab-on-grade, and crawlspace floors.

## Required Behavior

1. Add `foundation_depth_m: f64` to the RC node metadata for every `ExteriorTarget::Ground` node
   so the solver knows the centroid depth of each below-grade boundary.
2. At solver initialisation, pre-compute Kusuda-Achenbach parameters (`t_mean_c`, `t_amplitude_c`,
   `phase_day`) from the weather series. These already exist as `Environment::ground_t_mean_c`,
   `ground_t_amplitude_c` on `environment.rs:253–254`; expose them in `EnvironmentState` so the
   thermal solver can read them without calling back into `EnvironmentManager`.
3. At each timestep, for each below-grade boundary node, call `kusuda_achenbach_temp` with the
   node's `foundation_depth_m` and current day-of-year. Store the result as a per-node driving
   temperature instead of the shared scalar `ground_temp_c`.
4. Retain `env.weather.ground_temp_c` (DOE-2 surface model) only for boundaries that genuinely
   contact the grade surface (e.g., slab perimeter at grade level per ASHRAE HoF 2021 Ch. 18 §31
   perimeter F2 method). Document the distinction in a module-level comment on `thermal_solver/mod.rs`.
5. The DOE-2 surface model output and the Kusuda-Achenbach model must not share a field name or
   be mixed without explicit annotation of which applies to which boundary type.

## Approach

1. In `crates/hares-envelope/src/thermal_solver/config.rs`, add `foundation_depth_m: f64` to the
   struct that describes each ground-input node (wherever `ExteriorTarget::Ground` is represented).
   Default to 0.0 m (surface) so existing tests remain valid until per-boundary depths are wired.
2. Add `ground_t_mean_c: f64`, `ground_t_amplitude_c: f64`, `ground_phase_day: f64` to
   `EnvironmentState` (the read-only snapshot passed to the thermal solver), populated from
   `EnvironmentManager` fields at each `update_in_place` call.
3. In `ThermalSolver::prepare_inputs_inner` (`mod.rs`), replace the single
   `u[idx] = env.weather.ground_temp_c` loop with a per-node call to
   `kusuda_achenbach_temp(node.foundation_depth_m, env.day_of_year, ...)` using the parameters
   from step 2.
4. Apply the same replacement in `stepping.rs:179` and `longwave.rs:259` — all three sites must
   use the depth-corrected value.
5. Add an integration test (`cargo test -p hares-envelope bestest`) that verifies: for a basement
   zone in Minneapolis climate (annual mean 7 °C, amplitude 14 °C), the January ground boundary
   temperature at 2.4 m depth is within 1 °C of the Kusuda-Achenbach analytical result.

## Definition of Done

- No call site in `thermal_solver/` reads `env.weather.ground_temp_c` for a below-grade boundary
  without depth correction.
- `kusuda_achenbach_temp` is called from at least one hot-path call site per simulation timestep.
- `EnvironmentState` carries the three Kusuda parameters.
- Integration test passes for the Minneapolis basement case.
- Module-level comment on `thermal_solver/mod.rs` documents which ground temperature applies to
  which boundary category.

## Verification

```
cargo test -p hares-physics ground
cargo test -p hares-envelope bestest
cargo test -p hares-core environment
```

The `bestest` suite must include a case with a below-grade boundary and assert that the driving
temperature at depth 2.4 m differs from `ground_temp_c` (surface) by at least 3 °C in January
for a cold-climate site.

## References

- Kusuda, T. and Achenbach, P.R. (1965), ASHRAE Transactions Vol. 71(1), pp. 61-74 — depth
  attenuation and phase-lag derivation
- EnergyPlus Engineering Reference §12.6 "Ground Heat Transfer — Kusuda-Achenbach Undisturbed
  Ground Temperature Model"
- ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.31 "Below-Grade Heat Transfer" — boundary
  condition specification for basement walls, slab-on-grade, crawlspace

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match (with corrections noted below)
- [x] Described logic matches current implementation
- [x] OCHRE cross-check result: diverges — see detail below
- [x] EnergyPlus cross-check result: matches — see quoted passage below

**Line number corrections:**

| Ticket cites | Actual location | Notes |
|---|---|---|
| `crates/hares-io/src/epw.rs:607` (`interpolate_ground_temp_c`) | `epw.rs:607` | Confirmed exact match |
| `crates/hares-core/src/environment.rs:339–348` (`ground_temp_at_depth_c`) | `environment.rs:339–348` | Confirmed exact match |
| `crates/hares-core/src/environment.rs:253–254` (Kusuda parameter initialisation) | `environment.rs:253–254` | Confirmed exact match |
| `crates/hares-envelope/src/thermal_solver/mod.rs:701` (`u[idx] = env.weather.ground_temp_c`) | `mod.rs:701` | Confirmed exact match |
| `crates/hares-envelope/src/thermal_solver/stepping.rs:179` (`DrivingTemp::Ground => self.cached_ground_temp_c`) | `stepping.rs:179` | Confirmed exact match |
| `crates/hares-envelope/src/thermal_solver/longwave.rs:259` (`DrivingTemp::Ground => env.weather.ground_temp_c`) | `longwave.rs:259` | Confirmed exact match |

**OCHRE cross-check** (`vendors/OCHRE/ochre/`):

OCHRE (`ochre/utils/schedule.py:243–255`) implements the DOE-2 GTEMP subroutine ground
temperature formula — a damped harmonic oscillator based on monthly mean ambient temperatures
— which produces a single depth-implicit ground temperature series. The formula applies a
depth factor `beta = (π / (8760 × 0.025))^0.5 × 10` equivalent to a fixed penetration depth
of ~10 m, producing amplitude damping but **no per-boundary depth variation**. OCHRE then
feeds this single `Ground Temperature (C)` time series to the "Ground" exterior zone
(`Envelope.py:1260–1261`) as a constant boundary condition applied uniformly to all below-grade
surfaces. HARES's use of `env.weather.ground_temp_c` from the EPW's own monthly surface
temperatures matches OCHRE's approach, but both inherit the same physical limitation: no
per-node depth correction. The HARES Kusuda-Achenbach implementation is an intentional
improvement over OCHRE's approach that exists as dead code at runtime.

**EnergyPlus cross-check:**

EnergyPlus Engineering Reference (v8.4 and v22.1, BigLadder Software) states under
"Undisturbed Ground Temperature Model: Kusuda-Achenbach":

> *"T(z,t) = T̄ₛ − ΔT̄ₛ · e^(−z·√(π/ατ)) · cos(2πt/τ − θ)"*
>
> Where z = depth below surface, α = soil thermal diffusivity, τ = 365 days, θ = day of
> minimum surface temperature.
>
> Citation: "Kusuda, T. and P.R. Achenbach. 1965. 'Earth Temperatures and Thermal Diffusivity
> at Selected Stations in the United States.' ASHRAE Transactions. 71(1): 61-74."

The HARES `kusuda_achenbach_temp` function in `crates/hares-physics/src/ground.rs:63–80`
implements this formula exactly (confirmed by code inspection). EnergyPlus uses this model for
far-field boundary temperatures in its `Site:GroundDomain:Basement` object
(EnergyPlus Engineering Reference, "Ground Heat Transfer Calculations using
Site:GroundDomain:Basement"): *"The ground temperature profile at the domain sides and lower
surface are taken from Kusuda & Achenbach 1965."* The HARES thermal solver does not call this
model at runtime, diverging from EnergyPlus behaviour.

### Web-Verified Citations

**Citation 1:**
- **Citation**: Kusuda, T. and Achenbach, P.R. (1965), ASHRAE Transactions Vol. 71(1), pp. 61-74
- **Source found**: NIST Publications record; Semantic Scholar entry; cited in EnergyPlus
  Engineering Reference v8.4 and v22.1 at
  https://bigladdersoftware.com/epx/docs/8-4/engineering-reference/undisturbed-ground-temperature-model-kusuda.html
- **Quoted passage**: EnergyPlus cites: *"Kusuda, T. and P.R. Achenbach. 1965. 'Earth
  Temperatures and Thermal Diffusivity at Selected Stations in the United States.' ASHRAE
  Transactions. 71(1): 61-74."* (page numbers vary: 61–74 in some records, 61–75 in others;
  the paper spans that range.)
- **Verdict**: Confirmed. Title, journal, volume, year all match. Page range 61-74 vs 61-75 is
  a minor variant (pagination inclusive vs exclusive of last page); no material difference.

**Citation 2:**
- **Citation**: EnergyPlus Engineering Reference §12.6 "Ground Heat Transfer — Kusuda-Achenbach
  Undisturbed Ground Temperature Model"
- **Source found**: https://bigladdersoftware.com/epx/docs/22-1/engineering-reference/undisturbed-ground-temperature-model-kusuda.html
  (EnergyPlus v22.1 Engineering Reference)
- **Quoted passage**: *"T(z,t) = T̄ₛ − ΔT̄ₛ·e^(−z·√(π/ατ))·cos(2πt/τ − θ)"*, where T̄ₛ
  is average annual soil surface temperature, ΔT̄ₛ is amplitude of soil temperature change,
  θ is phase shift (day of minimum surface temperature), α is thermal diffusivity, τ = 365.
- **Verdict**: Confirmed for the formula and content. **Partially incorrect** on section number:
  the EnergyPlus Engineering Reference does not use a "§12.6" numeric section numbering scheme
  in any public version; the section is titled "Undisturbed Ground Temperature Model:
  Kusuda-Achenbach" and appears under the ground heat transfer chapter. The "§12.6" reference
  in the ticket does not correspond to any retrievable section heading and appears to be an
  approximate internal numbering that is not exposed in the published HTML documentation.
  The formula and physical content are correct.

**Citation 3:**
- **Citation**: ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.31 "Below-Grade Heat Transfer"
  — boundary condition specification for basement walls, slab-on-grade, crawlspace
- **Source found**: ASHRAE Handbook Table of Contents (ashrae.org);
  https://handbook.ashrae.org/Handbooks/F17/IP/f17_ch18/f17_ch18_ip.aspx (Chapter 18 content)
- **Quoted passage**: Chapter 18 of the 2017/2021 ASHRAE Handbook—Fundamentals covers
  "Nonresidential Cooling and Heating Load Calculations." The chapter does not contain a
  §18.31 "Below-Grade Heat Transfer" section; it focuses on internal loads, fenestration, and
  above-grade envelope. The `crates/hares-physics/src/ground.rs` module-level comment itself
  cites "Ch. 18.31 (Below-Grade Heat Transfer)" suggesting the code author placed this content
  in a different chapter than actually exists in the 2021 edition.

  Below-grade heat transfer in ASHRAE Fundamentals is addressed in Chapter 27 (Heat, Air, and
  Moisture Control in Building Assemblies — Examples) or earlier editions' Chapter 25/26. The
  physical principle cited by the ticket — that undisturbed ground temperature at foundation
  depth is the correct boundary condition — is scientifically sound and consistent with
  EnergyPlus guidance, even if the specific ASHRAE chapter/section reference is inaccurate.
- **Verdict**: **Incorrect** chapter/section reference. Ch. 18 §18.31 does not exist in the
  2021 ASHRAE Handbook—Fundamentals as a below-grade heat transfer section. However, the
  underlying physics claim (depth-dependent ground temperature as boundary condition) is
  correct and supported by EnergyPlus documentation and the Kusuda & Achenbach (1965) paper.

### Legitimacy

- **Verdict**: Partially Legitimate

- **Rationale**: The core bug is real and confirmed by direct code inspection. Three independent
  call sites — `mod.rs:701`, `stepping.rs:179`, and `longwave.rs:259` — all substitute the
  scalar `env.weather.ground_temp_c` (EPW monthly surface interpolation, depth ≈ 0 m) as the
  driving temperature for `ExteriorTarget::Ground` boundaries, with no depth correction.
  The `kusuda_achenbach_temp` function and the `ground_temp_at_depth_c` wrapper both exist but
  are never called during a simulation timestep. The Kusuda & Achenbach (1965) source is
  correctly identified and its formula is faithfully implemented. The quantitative physics
  example (9 °C January error for a Minneapolis basement at 2.4 m) is plausible: regression
  tests written as part of this audit confirm a ~12 °C difference at depth 2.4 m on day 15
  using Minneapolis parameters. The OCHRE cross-check shows OCHRE has the same limitation
  (single DOE-2 surface temperature applied to all below-grade boundaries), so HARES diverges
  from OCHRE only in having a dead-code Kusuda implementation rather than none.

  The details that reduce this from "Legitimate" to "Partially Legitimate":
  1. **Section number is not verifiable**: The EnergyPlus "§12.6" reference cannot be
     confirmed in any publicly available version of the Engineering Reference.
  2. **ASHRAE chapter reference is incorrect**: "ASHRAE HoF 2021 Ch. 18 §18.31
     Below-Grade Heat Transfer" does not correspond to any section in Chapter 18 of the
     2021 Fundamentals handbook. Chapter 18 covers nonresidential load calculations.
     Below-grade content appears in Chapter 27 (Building Assemblies — Examples) or related
     chapters. The same incorrect reference also appears in `ground.rs` module-level comments
     and `slab_perimeter_loss_w` documentation.
  3. **Parameter reuse is acknowledged but uncritical**: The ticket notes that
     `ground_t_amplitude_c` is derived from `mains_dt_annual_range_c / 2.0`, which is the
     half-range of monthly mean dry-bulb temperatures — physically the correct input for
     Kusuda-Achenbach surface amplitude. The reuse is sound, not a defect.

### Proposed Fix Summary

No production code changed (per audit scope). The minimal fix is:

1. Add `foundation_depth_m: f64` to the RC node metadata for `ExteriorTarget::Ground` nodes
   (default 0.0 m preserves current behaviour for non-below-grade boundaries).
2. Expose `ground_t_mean_c`, `ground_t_amplitude_c`, and `ground_phase_day` on
   `EnvironmentState` (the read-only snapshot the thermal solver receives), populated from
   `EnvironmentManager` at each `update_in_place`.
3. In `ThermalSolver::apply_outdoor_inputs` (`mod.rs:699–703`), replace the loop body with a
   per-node call to `kusuda_achenbach_temp(node.foundation_depth_m, env.day_of_year, ...)`.
4. Apply the same replacement in `stepping.rs:179` and `longwave.rs:259`.
5. Correct the ASHRAE chapter reference in `ground.rs` module doc and `slab_perimeter_loss_w`
   docstring: replace "Ch. 18.31" with the correct chapter (likely Ch. 27 in 2021 edition).

### Test Written

- **File**: `crates/hares-physics/tests/physics_validation_tests.rs`
- **Tests added**:
  - `ticket_055_kusuda_depth_correction_exceeds_3c_vs_surface_in_january_minneapolis` —
    asserts that `kusuda_achenbach_temp` at 2.4 m on day 15 (Minneapolis, t_mean=7°C,
    amplitude=14°C) is at least 3 °C warmer than at depth 0 m. Confirms the physics function
    is correct and quantifies the boundary-condition error the solver bug introduces.
  - `ticket_055_kusuda_depth_correction_sign_reversal_in_summer_minneapolis` —
    asserts that in late July the surface is warmer than the basement depth, demonstrating
    the seasonal sign-reversal described in the ticket.
- Both tests pass (`cargo test -p hares-physics ticket_055` → 2 passed, 0 failed). They
  exercise the physics function only; the solver wiring bug is not tested here because the
  solver currently does not call `kusuda_achenbach_temp` at all.
