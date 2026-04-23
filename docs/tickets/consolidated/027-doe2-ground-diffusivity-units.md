# DOE-2 Ground Temperature Model: Diffusivity in m²/hour vs m²/s Inconsistency

**Severity**: High
**Impact on annual kWh**: High
**Status**: Open
**Areas**: hares-io/epw

## Problem

The DOE-2 ground temperature model in `hares-io/src/epw.rs` uses `DOE2_GROUND_DIFFUSIVITY = 0.025` declared as `[m²/hour]` (line 368–369), and the beta parameter computation at line 449 divides by `DOE2_GROUND_HOURS_PER_YEAR = 8760.0`:

```rust
let beta = (std::f64::consts::PI / (DOE2_GROUND_HOURS_PER_YEAR * DOE2_GROUND_DIFFUSIVITY))
    .sqrt()
    * DOE2_GROUND_DEPTH_FACTOR;
```

The formula `β = D × √(π / (α × Y))` (where Y is the period and α is diffusivity) requires consistent units for α and Y. If α = 0.025 m²/h and Y = 8760 h, then α × Y = 219 m², and β is dimensionless — this is mathematically consistent for the DOE-2 model.

However, OCHRE's `get_ground_temp` (`ochre/models/weather.py`) uses the same constants (0.025 m²/h, 8760 h, depth 10 m) and produces a damping coefficient β ≈ 4.77. For a typical cold-climate site (annual average 8 °C, amplitude 12 °C), the January ground temperature prediction from this formula is approximately 2.5 °C.

The `hares-physics/src/ground.rs` Kusuda-Achenbach model at line 22 uses `DEFAULT_SOIL_DIFFUSIVITY_M2_PER_DAY = 0.05` (m²/day), which is a different model for a different purpose (deep-soil foundation boundary conditions vs. DOE-2 surface ground temperature). These two models coexist in HARES but are not cross-validated. The DOE-2 model produces ground surface temperatures; the Kusuda-Achenbach model produces below-grade temperatures. Both are used in the same simulation: DOE-2 output goes into `WeatherTimeSeries.ground_temp_c`, while Kusuda-Achenbach is called from `EnvironmentManager::ground_temp_at_depth_c`.

The `WeatherTimeSeries.ground_temp_c` (DOE-2 surface) is used by the thermal solver as the below-grade boundary temperature for slab-on-grade and crawlspace zone boundaries — but surface ground temperature (DOE-2) is not the same as undisturbed soil temperature at 0.3–1 m depth. EnergyPlus Engineering Reference §3.1 states that surface ground temperature should only be used for very shallow depths (< 0.1 m); for slab and crawlspace, the Kusuda-Achenbach temperature at the relevant depth is required.

## Current Behavior

`hares-envelope/src/thermal_solver/config.rs` and `hares-envelope/src/thermal_solver/stepping.rs`: the `ground_temp_c` from `WeatherState` (which comes from the DOE-2 surface model) is applied as the boundary temperature for below-grade surfaces.

`hares-core/src/environment.rs:253`: `ground_t_mean_c = mains_t_annual_avg_c` and `ground_t_amplitude_c = mains_dt_annual_range_c / 2.0` — the Kusuda model parameters are derived from the weather file stats, but the Kusuda model is only accessible via `EnvironmentManager::ground_temp_at_depth_c`, which is not called during the normal `update_in_place` step.

OCHRE uses DOE-2 surface ground temperature (equivalent to HARES `ground_temp_c`) for all ground-contact boundaries without depth correction.

## Required Behavior

1. Document explicitly which ground temperature model applies to which boundary type.
2. For below-grade walls and slabs (typical crawlspace depth 0.3–1 m), `EnvironmentManager::update_in_place` must call `ground_temp_at_depth_c` with the appropriate depth and expose the result in `WeatherState` (or via a separate field) so the thermal solver can use it.
3. The DOE-2 surface temperature should remain available for shallow-contact boundaries (e.g., slab floor surface boundary).
4. Alternatively (if matching OCHRE exactly is the priority), document that HARES uses DOE-2 surface temperature for all boundaries — matching OCHRE's behavior — and track this as a known limitation.

## Approach

Add `ground_temp_at_depth_c` as a field in `WeatherState` (e.g., `below_grade_ground_temp_c`) populated each step by `EnvironmentManager` at a configurable depth (e.g., 0.5 m default per ASHRAE HoF Ch. 18). Wire this field to the thermal solver's below-grade boundary conditions. The existing `ground_temp_c` (DOE-2 surface) remains for surface-adjacent boundaries.

## Definition of Done

- [ ] `WeatherState` has `below_grade_ground_temp_c` populated from Kusuda-Achenbach at configurable depth
- [ ] Thermal solver uses `below_grade_ground_temp_c` for slab and crawlspace zone boundaries
- [ ] Unit test confirms Kusuda-Achenbach at 0.5 m depth differs from DOE-2 surface temp by < 3 °C for a representative site
- [ ] Both ground temperature models documented in module-level comments with applicable depth ranges

## Verification

```bash
cargo test -p hares-physics ground
cargo test -p hares-envelope bestest
```

## References

- EnergyPlus Engineering Reference §3.1 "Ground Heat Transfer" — Kusuda-Achenbach for below-grade, not surface DOE-2
- ASHRAE Handbook of Fundamentals 2021 Ch. 18 "Nonresidential Cooling and Heating Load Calculations" §18.31 (Below-Grade Heat Transfer)
- Kusuda, T. and Achenbach, P.R. (1965), ASHRAE Transactions 71(1), pp. 61-74
- OCHRE `models/envelope.py`: uses DOE-2 surface temperature for all ground boundaries (acknowledged simplification)
