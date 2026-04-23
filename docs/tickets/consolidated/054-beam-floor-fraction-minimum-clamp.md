# `beam_floor_fraction` Minimum Clamp of 0.3 Is Physically Wrong at Low Solar Altitude

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-envelope/thermal_solver/solar.rs

## Problem

`beam_floor_fraction` (solar.rs:10–12) computes the fraction of transmitted
beam solar radiation that strikes the floor vs. the walls:

```rust
fn beam_floor_fraction(solar_altitude_deg: f64) -> f64 {
    solar_altitude_deg.to_radians().sin().clamp(0.3, 0.9)
}
```

The lower clamp of 0.3 is physically wrong. At low solar altitude angles
(sunrise, sunset, winter morning/evening) the beam enters through windows
nearly horizontally. A horizontal beam strikes vertical walls, not the floor;
the true floor fraction approaches zero as altitude → 0°.

`sin(0°) = 0.0` is the physically correct floor fraction at solar altitude 0°.
The clamp forces a minimum of 0.3, assigning 30% of all low-angle beam to the
floor even at sunrise. This phantom floor gain is distributed per the floor's
`area × absorptance` weight and deposited to the floor RC node, bypassing the
wall nodes where the energy physically arrives.

The upper clamp of 0.9 is arguably justified — the sun is rarely directly
overhead in residential buildings (overhead sun at high altitude strikes mainly
the floor through skylights, but glazing on vertical walls means some wall
absorption always occurs). A ceiling of 0.9 is defensible.

The `sin(altitude)` base formula itself is an approximation. EnergyPlus uses
ASHRAE interior solar distribution with view-factor-based geometry (Engineering
Reference §14.5). For the approximation to be acceptable, it must at least be
physically monotone and pass through zero.

## Evidence

```
crates/hares-envelope/src/thermal_solver/solar.rs:10–12
fn beam_floor_fraction(solar_altitude_deg: f64) -> f64 {
    solar_altitude_deg.to_radians().sin().clamp(0.3, 0.9)
}
```

Called at solar.rs:116 and solar.rs:149 for both ScriptF (LWR) and StarMesh
solar distribution paths.

## Annual kWh Impact

**Medium.** The error is concentrated in winter mornings and evenings when
solar altitude is low and beam solar is non-trivial. In heating-dominated
climates (CZ 5–7), this artificially cools walls (less solar gain to wall
nodes) and heats the floor (excess floor gain), resulting in some cancellation.
The net effect on zone air temperature is moderated by the interior LWR
exchange, but the spatial distribution of absorbed solar heat is wrong. For
BESTEST Case 600 the error is visible in morning peak zone temperatures.

## Required Fix

1. Remove the lower clamp of 0.3. The physically correct lower bound is 0.0.
2. Retain the upper clamp of 0.9 (or make it configurable per ASHRAE interior
   solar distribution coefficients).
3. Corrected function:
   ```rust
   fn beam_floor_fraction(solar_altitude_deg: f64) -> f64 {
       solar_altitude_deg.to_radians().sin().clamp(0.0, 0.9)
   }
   ```
4. Add a unit test verifying:
   - `beam_floor_fraction(0.0) == 0.0` (sunrise: beam hits walls, not floor)
   - `beam_floor_fraction(90.0) <= 0.9` (overhead: mostly floor but clipped)
   - `beam_floor_fraction(30.0) ≈ 0.5` (mid-altitude: ~half floor)
5. Long-term: replace the sinusoidal approximation with ASHRAE SHGC-weighted
   area fractions per surface orientation (EnergyPlus §14.5).

## References

- EnergyPlus Engineering Reference §14.5 (Interior Solar Distribution).
- ASHRAE Handbook of Fundamentals 2021, Ch. 15, §5 (Solar Heat Gain through
  Fenestration — interior distribution).
- BESTEST ASHRAE Standard 140-2017, Case 600 (interior solar gain distribution
  validation).
