# Direct Solar Beam Floor Fraction: Physically Wrong Clamp and Duplicated Logic

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-envelope/thermal_solver/solar.rs

## Problem

`beam_floor_fraction` at `crates/hares-envelope/src/thermal_solver/solar.rs:10-12` computes the fraction of transmitted beam solar that strikes the floor:

```rust
fn beam_floor_fraction(solar_altitude_deg: f64) -> f64 {
    solar_altitude_deg.to_radians().sin().clamp(0.3, 0.9)
}
```

The lower clamp of 0.3 is physically wrong. At low solar altitude angles (sunrise, sunset, winter mornings), beam enters through windows nearly horizontally and strikes vertical walls, not the floor. The physically correct floor fraction approaches zero as altitude approaches 0°. `sin(0°) = 0.0` is correct; clamping to 0.3 assigns 30% of all low-angle beam to the floor, depositing phantom heat to floor RC nodes and bypassing wall nodes where that energy physically arrives.

The upper clamp of 0.9 is defensible: overhead sun illuminates the floor predominantly through vertical glazing, but some wall absorption always occurs.

The `sin(altitude)` base formula has no citation and is not derived from any published standard. EnergyPlus (Engineering Reference §14.5 "Beam Solar Radiation Distribution") uses a geometry-based view-factor approach; ASHRAE Fundamentals 2021 Ch. 18 §18.47 uses area-weighted distribution fractions computed from room geometry. The heuristic is an approximation that must at minimum be physically monotone and pass through zero.

Second defect: `compute_solar_distribution_into` (solar.rs:231-293, ScriptF/LWR path) and `compute_solar_distribution_into_solar` (solar.rs:319-377, StarMesh path) duplicate approximately 60 lines of identical beam-splitting and diffuse-distribution logic, differing only in the struct type (`InteriorSurfaceInfo` vs `InteriorSolarSurfaceInfo`). Any physics fix to one path silently misses the other.

## Current Behavior

`crates/hares-envelope/src/thermal_solver/solar.rs:10-12`: lower clamp of 0.3 forces minimum 30% floor fraction regardless of solar altitude.

`solar.rs:116` and `solar.rs:149`: `beam_floor_fraction` called identically in both distribution paths.

`solar.rs:231-293` and `solar.rs:319-377`: full beam-splitting and diffuse loop duplicated verbatim across `compute_solar_distribution_into` and `compute_solar_distribution_into_solar`.

OCHRE `SolarModel.py` uses a fixed beam fraction of 0.6 for all altitudes (BESTEST assumption), which is wrong at low altitudes but does not introduce the direction-reversal error that the lower clamp does. BESTEST Case 600 specifies 0.6 as a fixed distribution coefficient for validation only, not as a physics model.

## Required Behavior

1. Remove the lower clamp of 0.3. The physically correct lower bound is 0.0 (`sin(0°) = 0.0` means beam arriving at zero altitude angle does not illuminate the floor). Retain the upper clamp of 0.9. Corrected function:

   ```rust
   fn beam_floor_fraction(solar_altitude_deg: f64) -> f64 {
       solar_altitude_deg.to_radians().sin().clamp(0.0, 0.9)
   }
   ```

2. The long-term target is the ASHRAE/EnergyPlus interior solar distribution model (EnergyPlus Engineering Reference §14.5, Table 14.2): beam solar assigned by room geometry and surface absorptance fractions, not a heuristic sine. This is a separate, larger refactor; the clamp fix is the immediate deliverable.

3. Eliminate the duplication between `compute_solar_distribution_into` and `compute_solar_distribution_into_solar`. The beam-splitting and diffuse-distribution loops are identical; extract the shared logic into a generic function or a trait so that any future physics change applies to both ScriptF and StarMesh paths simultaneously.

## Definition of Done

- [ ] `beam_floor_fraction` lower clamp changed from 0.3 to 0.0
- [ ] Unit test: `beam_floor_fraction(0.0) == 0.0`
- [ ] Unit test: `beam_floor_fraction(90.0) <= 0.9`
- [ ] Unit test: `beam_floor_fraction(30.0)` is approximately `sin(30°) = 0.5`
- [ ] Unit test: at solar altitude 5°, the non-floor fraction of distributed beam exceeds the floor fraction (regression guard for the direction-reversal error)
- [ ] `compute_solar_distribution_into` and `compute_solar_distribution_into_solar` share a common implementation; duplication eliminated

## Verification

```bash
cargo test -p hares-envelope solar
```

## References

- EnergyPlus Engineering Reference §14.5 "Beam Solar Radiation Distribution" — geometry-based distribution fractions, Table 14.2
- ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.47 "Solar Radiation Through Fenestration" — simplified distribution factors by surface area and absorptance
- ASHRAE Standard 140-2017, Case 600 — fixed 0.6 floor fraction for BESTEST validation only, not a physics model
