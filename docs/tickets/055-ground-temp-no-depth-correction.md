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
