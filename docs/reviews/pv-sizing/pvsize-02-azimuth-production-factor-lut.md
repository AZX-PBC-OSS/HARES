# Azimuth production factor lookup table — South=1.0, East=0.80, North=0.45 at 45-degree snaps
**Review ID**: pvsize-02
**Category**: pv-sizing
**Date**: 2026-05-26

## Files Reviewed
crates/hares-physics/src/pv_sizing.rs

## Vendor/Reference Files Consulted
- vendors/EnergyPlus/src/EnergyPlus/PVWatts.cc (EnergyPlus PVWatts v5 wrapper around SAM SDK `pvwattsv5_1ts`)
- vendors/EnergyPlus/third_party/ssc/shared/lib_pvwatts.cpp (SAM PVWatts v5 DC/AC power model)
- vendors/EnergyPlus/third_party/ssc/shared/lib_irradproc.cpp (SAM irradiance transposition, Perez/Hay-Davies/isotropic sky models)
- vendors/OCHRE/ochre/Equipment/PV.py (OCHRE PV model using PySAM `Pvwattsv8`)

## Findings

### Finding 1: [Severity: high] LUT is latitude-independent; NREL SAM and OCHRE both apply latitude implicitly through solar geometry
**Description**: The `azimuth_production_factor_lut` function (lines 124-138) returns fixed production multipliers regardless of site latitude. A north-facing panel at 25°N (Miami) receives the same factor (0.45) as one at 48°N (Seattle), despite dramatically different relative production. NREL SAM PVWatts v5 and OCHRE do not use simplified azimuth lookup tables — both pass latitude to the solar position calculation (see `solarpos_spa` in lib_irradproc.cpp:2347 and `location["latitude"]` in PV.py:39), making azimuth derating inherently latitude-dependent through solar geometry and the Perez transposition model. PVWatts v5 never reduces azimuth to a fixed multiplier.
**Code Location**: `crates/hares-physics/src/pv_sizing.rs:124-138`
**Root Cause**: The lookup table is hardcoded with no latitude parameter. A separate latitude-dependent model (`plane_solar_score`, lines 144-151) exists but is only used for Gable/Flat plane selection and candidate ranking — the Hip roof aggregation path at line 282 exclusively uses the static LUT.
**Impact**: For a sizing model this may be acceptable as an approximation, but at high latitudes the error is substantial. At Seattle (48°N), north-facing panels produce ~30% of south-facing annual output, while the LUT gives 45% — a 50% relative overestimate. At Miami (25°N), north-facing panels produce ~65% of south-facing, while the LUT gives 45% — a 30% relative underestimate. When this factor feeds into Hip roof panel counting (line 282-283), it directly affects `max_capacity_kw`.

### Finding 2: [Severity: medium] LUT uses nearest-neighbor snapping at 45° intervals, introducing up to 22.5° quantization error and discontinuous 10% factor jumps
**Description**: The snapping formula `((az / 45.0).round() * 45.0)` at line 127 quantizes azimuth to the nearest 45° cardinal direction. A roof plane at 157° east of north gets snapped to 135° (SE, factor 0.90), while a plane at 158° snaps to 180° (S, factor 1.00). This creates a discontinuous 10% jump in the production factor at the 157.5° boundary. The maximum azimuth error is 22.5° (half the 45° step). At mid-latitudes (~35°N), a 20° azimuth error introduces approximately 2-3% annual production error, meaning the maximum quantization error is roughly 2.5-3.5%. Whether this is acceptable depends on the accuracy requirements of the sizing model.
**Code Location**: `crates/hares-physics/src/pv_sizing.rs:127`
**Root Cause**: Nearest-neighbor rounding with a coarse 45° step size.
**Impact**: For a rooftop sizing model, 3.5% capacity estimate error may be within acceptable bounds. However, the discontinuity at 157.5°/202.5° could cause a roof that is nearly-south to be classified as SE/SW, producing an unexpectedly low estimate. Linear interpolation between LUT entries would reduce this to <1% error.

### Finding 3: [Severity: medium] East-West asymmetry gives West a 5% premium — inverted from the physical afternoon temperature penalty
**Description**: The LUT assigns East (90°) = 0.80 and West (270°) = 0.85. This gives west-facing panels a 6.25% relative premium over east-facing panels. In reality, afternoon ambient temperatures are typically higher than morning temperatures, which reduces PV efficiency (via the negative temperature coefficient, typically -0.3% to -0.5%/°C). At most US locations, this makes west-facing panels produce 1-3% LESS than east-facing annually (see Lave & Kleissl 2010). The HARES values invert this relationship. Notably, the SAM PVWatts v5 model at `lib_pvwatts.cpp:235` does not apply different derating factors for East vs. West — the temperature penalty emerges naturally from the cell temperature model (`pvwatts_celltemp` class, line 117-172) operating on hourly weather data.
**Code Location**: `crates/hares-physics/src/pv_sizing.rs:132-133`
**Root Cause**: The LUT values appear to be rough estimates rather than empirically derived from production data. The 5% premium for West may reflect a mistaken assumption that afternoon sun is more energetic, without accounting for temperature effects.
**Impact**: At the individual building level, a 5% capacity factor error is modest. However, the sign error means West-facing arrays are systematically overvalued relative to East-facing. This also interacts with the west-of-south tiebreaker in `resolve_azimuth` (line 181) and Hip plane selection (line 254), which compound the westward bias.

### Finding 4: [Severity: high] Inconsistency between LUT (Hip aggregation) and plane_solar_score (Gable/Flat selection); models disagree dramatically on East/West factors
**Description**: The codebase contains two fundamentally different azimuth production models:
- **LUT** (lines 124-138): South=1.00, East=0.80, West=0.85, North=0.45
- **plane_solar_score** (lines 144-151): `0.18 + 0.82 × cos(θ)^n` where θ is deviation from south

At latitude 35° (n=1.0), plane_solar_score gives:
- East/West (θ=90°): 0.18 (vs LUT's 0.80-0.85 — a 4.5× difference)
- SE/SW (θ=45°): 0.76 (vs LUT's 0.90)

The `compute_usable_area` function uses the LUT for Hip roof aggregation (line 282) but `plane_solar_score` for Gable/Flat plane selection (line 265). `enumerate_pv_candidates` uses `plane_solar_score` for all roof shapes (line 431). This means a Hip roof may receive different panel counts depending on which code path is called, and the ranking from `enumerate_pv_candidates` may not match the plane selected by `compute_usable_area` for Hip roofs.
**Code Location**: `crates/hares-physics/src/pv_sizing.rs:124-138` (LUT) vs `144-151` (plane_solar_score); used differently at lines 282 and 265/431.
**Root Cause**: The LUT and continuous model were developed independently with different assumptions about the proportion of direct vs. diffuse radiation contributing to off-south panel production.
**Impact**: The East/West factor discrepancy is extreme (0.18 vs 0.80). If the plane_solar_score model is correct, the LUT severely overestimates East/West production; if the LUT is correct, plane_solar_score underestimates. Neither extreme is consistent with empirical data (East/West panels typically produce 75-85% of south-facing). The plane_solar_score model incorrectly assumes all beam radiation arrives from due south, which is physically wrong — morning sun strikes east-facing panels directly.

### Finding 5: [Severity: medium] LUT values are undocumented and lack empirical derivation; no dedicated unit tests
**Description**: The LUT values (0.45, 0.65, 0.80, 0.85, 0.90, 1.00) appear nowhere in the file's comments or documentation. There is no unit test that directly exercises `azimuth_production_factor_lut` — all existing tests (lines 537-746) validate `compute_usable_area` end-to-end, but none assert that specific azimuth inputs produce specific LUT outputs. The only way to verify the LUT values through tests is indirectly via the Hip roof aggregation test at line 610, which checks that `max_panels > 0` without validating the count.
**Code Location**: `crates/hares-physics/src/pv_sizing.rs:124-138` (no source citation for values); `523-746` (no direct LUT test)
**Root Cause**: The function comment at lines 122-123 describes the snapping behavior but not the provenance of the factor values.
**Impact**: Without provenance, it is impossible to determine whether the factors come from NREL SAM, PVWatts, published literature, or engineering judgment. Future maintainers cannot assess whether these values need revision for different geographic regions or panel technologies.

### Finding 6: [Severity: low] Default match arm is dead code
**Description**: The match expression at lines 128-137 covers all eight 45° cardinal values from 0° to 315° (0, 45, 90, 135, 180, 225, 270, 315). The `round()` function always produces one of these eight values after snapping and modulo 360. The default arm `_ => 0.75` at line 136 can never be reached, making it dead code.
**Code Location**: `crates/hares-physics/src/pv_sizing.rs:136`
**Root Cause**: Over-defensive coding. All possible snapped values are explicitly matched.
**Impact**: Low — no runtime bug, but the dead code and arbitrary fallback value (0.75) could mislead maintainers. The compiler will not warn since `u32` is not an exhaustive type in a Rust match.

### Finding 7: [Severity: low] No vendor reference implementation uses a simplified azimuth-only lookup table
**Description**: EnergyPlus PVWatts (PVWatts.cc:88-90) instantiates the SAM SDK module `pvwattsv5_1ts`, which performs per-timestep solar position calculation (latitude-dependent), incidence angle computation, and Perez-model transposition of beam/diffuse/ground-reflected irradiance. OCHRE PV.py:53 calls `pvwatts.default("PVWattsNone")` which runs the full PVWatts v8 model with identical methodology. Neither reference implementation reduces azimuth to a static multiplier — orientation effects emerge from the full irradiance model operating on hourly solar geometry. PVWatts v5 also integrates a thermal model (`pvwatts_celltemp`, lib_pvwatts.cpp:117-172) that captures the temperature penalty on west-facing panels naturally, without explicit azimuth-asymmetric factors.
**Code Location**: `vendors/EnergyPlus/src/EnergyPlus/PVWatts.cc:88-90`, `vendors/OCHRE/ochre/Equipment/PV.py:53-63`, `vendors/EnergyPlus/third_party/ssc/shared/lib_pvwatts.cpp:117-172`
**Impact**: This finding is informational. For a sizing model (as opposed to an hourly energy model), a simplified lookup approach is a valid engineering trade-off. The existence of this review confirms that the simplification is deliberate and the accuracy trade-offs should be understood.

## Summary
- Total findings: 7
- Critical: 0
- High: 2 (Findings 1, 4)
- Medium: 3 (Findings 2, 3, 5)
- Low: 2 (Findings 6, 7)

## Recommendations
1. **Add latitude dependence to the LUT or unify models** (Finding 1, 4). Replace the static LUT with a latitude-parameterized model. The existing `plane_solar_score` function could be adapted for this purpose, but it needs correction for East/West orientations — consider a model that accounts for morning/afternoon direct beam on off-south panels, e.g., by integrating hourly solar geometry or using latitude-aware derating factors from NREL PVWatts documentation (Table 4 in Dobos 2014, NREL/TP-6A20-62641).
2. **Correct East-West asymmetry** (Finding 3). Switch the East and West factors so that East ≥ West (e.g., East=0.85, West=0.80, or symmetric 0.83). The physical temperature penalty makes East slightly better at most US locations.
3. **Use linear interpolation instead of nearest-neighbor** (Finding 2). Replace `round()` with floor/ceil fractional weighting to eliminate the discontinuous jump at the 157.5°/202.5° boundary and reduce quantization error by ~3×.
4. **Add unit tests for the LUT** (Finding 5). Test each cardinal direction (0°, 45°, 90°, 135°, 180°, 225°, 270°, 315°), boundary transitions (±22.5° from each snap), and edge cases (0°, 360°, negative angles). Verify LUT values against published NREL SAM PVWatts orientation factor tables for at least two latitudes (25°N and 48°N).
5. **Remove or document the dead code** (Finding 6). Either remove the `_ => 0.75` default arm or annotate it with `#[allow(unreachable_patterns)]` and a comment explaining it's a safety fallback.
6. **Document source of LUT values** (Finding 5). Add a comment citing the derivation method (e.g., "Values from NREL PVWatts v5 annual orientation factor table for fixed-tilt arrays at latitude tilt, US average," or "Empirically estimated from 100 US TMY3 sites").

## References / Citations
- Dobos, A.P. (2014). "PVWatts Version 5 Manual." NREL/TP-6A20-62641. National Renewable Energy Laboratory. (Documents that PVWatts v5 does not use azimuth lookup tables — production is computed from hourly solar geometry.)
- Lave, M. & Kleissl, J. (2010). "Optimum fixed orientations and benefits of tracking for capturing solar radiation in the continental United States." Renewable Energy, 36(3), 1145-1152. (Documents East-West production asymmetry due to afternoon temperature effects.)
- SAM SSC source: `vendors/EnergyPlus/third_party/ssc/shared/lib_irradproc.cpp` — `incidence()` (line 1476), `perez()` (line 1907), and `isotropic()` (line 1860) transposition models. All are latitude-dependent through solar position.
- SAM SSC source: `vendors/EnergyPlus/third_party/ssc/shared/lib_pvwatts.cpp` — `dcpowr()` (line 235) applies the temperature coefficient (`pwrdgr`) to cell temperature, creating the East-West production asymmetry.
- EnergyPlus source: `vendors/EnergyPlus/src/EnergyPlus/PVWatts.cc:88-90` — calls `ssc_module_create("pvwattsv5_1ts")`, which uses the full SAM PVWatts v5 pipeline with latitude-dependent solar geometry.
- OCHRE source: `vendors/OCHRE/ochre/Equipment/PV.py:53-63` — uses PySAM `Pvwattsv8` with full hourly simulation including latitude via `location["latitude"]`.
