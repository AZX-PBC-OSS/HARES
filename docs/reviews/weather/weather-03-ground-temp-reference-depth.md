# Ground temperature reference depth mismatch vs OCHRE (0.5m vs 2m)
**Review ID**: weather-03
**Category**: weather
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/ground.rs` — Kusuda-Achenbach model, slab perimeter (F2) & foundation wall heat loss
- `crates/hares-io/src/epw.rs` — EPW parsing, EPW header ground-temp extraction, DOE-2 fallback model

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/utils/schedule.py` — DOE-2 GTEMP ground-temperature correlation (lines 243–254)
- `vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc` — EPW header ground-temp processing (lines 7810–7849), ground-temp type dispatch (lines 6894–6918)
- `vendors/EnergyPlus/src/EnergyPlus/GroundTemperatureModeling/SiteShallowGroundTemperatures.cc` — Shallow vs Deep ground-temp separation
- `vendors/EnergyPlus/src/EnergyPlus/GroundTemperatureModeling/KusudaAchenbachGroundTemperatureModel.cc` — Kusuda-Achenbach model parameter derivation from Shallow ground temperatures

## Findings

### Finding 1: [Severity: high] DOE-2 fallback depth is 0.5 m; OCHRE uses ~3.0 m (10 ft)

**Description**: The DOE-2 fallback ground-temperature model in `epw.rs` uses a reference depth of 0.5 m (`DOE2_GROUND_REFERENCE_DEPTH_M`), while OCHRE (faithfully following the original DOE-2 GTEMP subroutine) uses a depth of 10 ft (~3.048 m). At 0.5 m the DOE-2 amplitude-damping factor (`gm`) is approximately 0.97 — negligible seasonal attenuation — whereas at the original 10 ft depth the same model produces `gm ≈ 0.55`.

**Code Location**:
- HARES: `crates/hares-io/src/epw.rs:437` — `DOE2_GROUND_REFERENCE_DEPTH_M: f64 = 0.5`
- HARES: `crates/hares-io/src/epw.rs:523` — `beta` computed as `sqrt(π / (8760 * 0.025)) * DOE2_GROUND_REFERENCE_DEPTH_M`
- OCHRE: `vendors/OCHRE/ochre/utils/schedule.py:248` — `beta = (np.pi / (8760 * 0.025)) ** 0.5 * 10`

**Root Cause**: The depth of 0.5 m was selected to match the EPW header `GROUND TEMPERATURES` reference depth for `GroundTemperatures:Surface` (EPW Data Dictionary v9.6 §3), which is correct for the EPW header parsing path. However, the *same* constant is reused for the DOE-2 fallback model without adjusting for the fact that the DOE-2 GTEMP correlation was calibrated at 10 ft below grade, not at the shallow surface boundary. The comment on line 433 falsely states "The original DOE-2 GTEMP code used 5 ft (approximately 1.524 m)" — the actual value in both the DOE-2 source and OCHRE is 10 ft.

**Impact**:
- When EPW files lack ground-temperature header data (`GROUND TEMPERATURES,0`), the hourly ground temperature field (`weather.ground_temp_c`) is populated from the DOE-2 fallback at 0.5 m with effectively no seasonal amplitude damping.
- This produces ground temperatures that closely track outdoor air temperatures (97% of seasonal amplitude) rather than the damped soil temperatures expected at foundation depth.
- Downstream consumers of `weather.ground_temp_c` include the dwelling module (`crates/hares-core/src/dwelling/mod.rs:3123`) where it is exposed as a scalar ground temperature. The thermal solver in `crates/hares-envelope/src/thermal_solver/mod.rs` uses a separate Kusuda-Achenbach computation with per-boundary depths for most ground-coupled surfaces, but the scalar `ground_temp_c` is used for water heater standing losses (water heater ambient), pipe heat loss estimates, and diagnostics.
- In climates with large seasonal temperature swings, this can produce physically implausible ground-temperature extremes — e.g., ground temperatures below freezing in sub-arctic winters or above 30 °C in hot summers, where the real ground at foundation depth would be much more moderate.

### Finding 2: [Severity: medium] Incorrect claim about original DOE-2 depth in comment

**Description**: The doc comment on line 433–436 of `epw.rs` states: "The original DOE-2 GTEMP code used 5 ft (approximately 1.524 m); the choice of 0.5 m here is the physically motivated depth for slab/crawlspace foundations." Both claims are incorrect: (a) the DOE-2 GTEMP code uses 10 ft (not 5 ft), as verified in OCHRE's preservation of the same correlation; (b) 0.5 m is not physically motivated for slab/crawlspace foundations — ASHRAE Handbook of Fundamentals Ch. 27 models below-grade heat transfer using soil temperatures at the average depth of the below-grade surface, which for basement walls is typically 1–2 m and for slabs is on the order of 2–4 m where thermal mass dominates.

**Code Location**: `crates/hares-io/src/epw.rs:433-436`

**Impact**: Misleading documentation could lead a future maintainer to trust the 0.5 m value as validated against reference implementations when it is not.

### Finding 3: [Severity: low] EPW header path correctly picks 0.5 m, but no fallback to deeper depths

**Description**: The EPW header parsing at `parse_ground_temperatures()` correctly selects the depth entry closest to 0.5 m, matching EnergyPlus's documented behavior (WeatherManager.cc:7812: "Assume the 0.5 m set of ground temperatures"). However, when the EPW header does contain multiple depth levels (e.g., 0.5 m, 2 m, 4 m as seen in the Denver TMY3 fixture), HARES discards the deeper data entirely and uses only the 0.5 m set for all ground-temperature-dependent calculations. EnergyPlus separates ground temperatures by application type (`Site:GroundTemperature:Shallow` at 0.5 m, `Site:GroundTemperature:BuildingSurface` for envelope ground contact, `Site:GroundTemperature:Deep`, and `Site:GroundTemperature:FCFactorMethod`), each serving a different purpose.

**Code Location**: `crates/hares-io/src/epw.rs:366` — `TARGET_DEPTH_M: f64 = 0.5` in `parse_ground_temperatures()`

**Impact**: Even when the EPW file provides deeper ground-temperature data that would be more appropriate for foundation heat-transfer boundary conditions, HARES cannot use it. Foundation wall and slab heat transfer models are forced to use the shallowest available ground temperature or, in the thermal solver, rely on a separate Kusuda-Achenbach computation.

### Finding 4: [Severity: medium] Depth is hard-coded and not user-configurable

**Description**: Neither the EPW header depth selection target (0.5 m) nor the DOE-2 fallback reference depth (0.5 m) is exposed as a user-configurable parameter. The only depth-exposed path is the `SourceTemperature::KusudaAchenbach { borehole_depth_m }` variant in `ground.rs:195`, which is specific to ground-source heat pump boreholes. For building envelope ground coupling, the depth is either hard-coded in the DO2 fallback constants or implicitly determined by the solver wiring (`ground_temp_input_depths_m`).

**Code Location**:
- `crates/hares-io/src/epw.rs:366` — hard-coded 0.5 m for EPW header depth selection
- `crates/hares-io/src/epw.rs:437` — hard-coded 0.5 m for DOE-2 fallback depth
- `crates/hares-physics/src/ground.rs:195-201` — only GSHP borehole depth is configurable via `SourceTemperature::KusudaAchenbach`

**Impact**: Users cannot select a deeper reference depth for ground-temperature extraction from EPW files, nor adjust the DOE-2 fallback depth to match local soil conditions or foundation design. Different foundation types (shallow slab vs full basement) could benefit from different reference depths.

## Summary
- Total findings: 4
- High: 1
- Medium: 2
- Low: 1

## Recommendations

1. **Fix the DOE-2 fallback depth.** Change `DOE2_GROUND_REFERENCE_DEPTH_M` from 0.5 to a depth consistent with the original DOE-2 GTEMP model (10 ft ≈ 3.048 m), or at minimum to a depth physically appropriate for foundation heat transfer (1.5–3.0 m). The amplitude-damping analysis shows:

   | Depth | `gm` factor | Amplitude damping |
   |-------|-------------|-------------------|
   | 0.5 m (current HARES) | ~0.97 | ~3% |
   | 1.5 m | ~0.70 | ~30% |
   | 3.048 m (10 ft, OEM DOE-2) | ~0.55 | ~45% |

   If 3 m depth is adopted, the DOE-2 fallback will produce ground temperatures with seasonal swings roughly half of the outdoor air swing, matching the physical expectation for foundation-level soil.

2. **Make the reference depth configurable.** Expose `TARGET_DEPTH_M` in the EPW header parser and `DOE2_GROUND_REFERENCE_DEPTH_M` in the fallback as parameters settable via a configuration file or simulation options. This allows users of different foundation types (slab-on-grade vs deep basement) to select appropriate depths.

3. **Preserve deeper EPW ground-temperature data.** When an EPW file contains multiple depth levels, store all of them and provide an API to query ground temperature at a given depth. This would enable foundation heat-transfer models to use deeper (more damped) ground temperatures where appropriate, matching EnergyPlus's multi-type ground-temperature architecture.

4. **Correct the misleading comment** on `epw.rs:433-436` to reflect that the original DOE-2 GTEMP code uses 10 ft (not 5 ft), and that any deviation from that is a design choice rather than alignment with the reference.

## References / Citations

- **DOE-2 GTEMP subroutine**: OCHRE preserves the original correlation at `vendors/OCHRE/ochre/utils/schedule.py:243-254`, using 10 ft depth. Comment on line 244 states: "same correlation as DOE-2's src\WTH.f file, subroutine GTEMP."
- **EnergyPlus EPW header processing**: `vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc:7810-7849` — assumes 0.5 m ground temperatures for FC factor method, parsing the first set from each depth entry.
- **EnergyPlus ground-temperature type separation**: `WeatherManager.cc:6894-6918` initializes four separate ground-temperature objects (BuildingSurface, FCFactorMethod, Shallow, Deep), each serving a different boundary-condition purpose.
- **EPW Data Dictionary v9.6**: The GROUND TEMPERATURES header field stores monthly ground temperatures at user-specified depths; 0.5 m is the recommended reference depth for surface boundary conditions (`GroundTemperatures:Surface`).
- **Kusuda, T. and Achenbach, P.R. (1965)**: "Earth Temperatures and Thermal Diffusivity at Selected Stations in the United States", ASHRAE Transactions, Vol. 71(1), pp. 61-74. The undisturbed ground-temperature model used in `crates/hares-physics/src/ground.rs:46-89`.
- **ASHRAE Handbook of Fundamentals 2021, Ch. 27**: Below-grade heat transfer boundary conditions for basement walls, slab-on-grade, and crawlspace floors. Foundation heat transfer uses soil temperatures at average below-grade depth, typically 1–4 m depending on foundation geometry.
