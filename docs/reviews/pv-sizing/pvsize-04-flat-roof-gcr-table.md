# Flat roof GCR latitude-dependent table (0.35/0.40/0.50)
**Review ID**: pvsize-04
**Category**: pv-sizing
**Date**: 2026-05-26

## Files Reviewed
crates/hares-physics/src/pv_sizing.rs

## Vendor/Reference Files Consulted
vendors/EnergyPlus/src/EnergyPlus/PVWatts.hh, PVWatts.cc, Photovoltaics.cc
vendors/EnergyPlus/third_party/ssc/ssc/cmod_pvwattsv5.cpp
vendors/EnergyPlus/third_party/ssc/shared/lib_pvshade.cpp, lib_irradproc.cpp
vendors/EnergyPlus/idd/versions/V9-6-0-Energy+.idd
vendors/OCHRE/ochre/Equipment/PV.py

## Findings

### Finding 1: GCR-tilt coupling inconsistency at high latitudes [Severity: high]
**Description**: The flat roof tilt is capped at 25° (`latitude.min(25.0)` at line 323), while the GCR values decrease with latitude (0.50 → 0.40 → 0.35). These two trends are physically contradictory: a lower tilt produces less row-to-row shading, enabling a *higher* GCR. Yet at high latitudes where the tilt cap binds (lat ≥ 25 always gets 25° tilt), HARES assigns the *lowest* GCR of 0.35.

Verified geometrically: GCR = 1 / (cos(tilt) + sin(tilt) / tan(solar_elevation)). At lat 40° with tilt 25°, HARES GCR=0.35 corresponds to shading-free operation above ~12.2° solar elevation — roughly an 8:30 AM winter-solstice window, which is reasonable. However, at lat 50° with the same 25° tilt, the same GCR=0.35 corresponds to the sun being at only ~5.5° elevation — implying almost year-round shading. If the tilt at lat 50° were latitude-correct (40-45°), a still-lower GCR of ~0.22 would be needed; conversely, if the tilt truly is 25° (ballasted commercial racking), a GCR of 0.40-0.45 would be feasible because low tilt reduces shading.

**Code Location**: `crates/hares-physics/src/pv_sizing.rs:154-161` (GCR table) and `:321-327` (tilt capping)

**Root Cause**: The tilt cap `latitude.min(25.0)` was designed to model shallow commercial flat-roof racking, but the GCR table was parameterized independently without re-computing the GCR values for the capped tilt. The two parameters were derived from different mental models.

**Impact**: At latitudes above 40°, the system underestimates flat-roof capacity for buildings that actually use shallow-tilt commercial racking (25° or less), and may be correct-to-optimistic for residential flat roofs using steeper tilts. Capacity error magnitude: for a 100 m² roof at lat 45°, HARES estimates ~11.8 kW (28 panels × 420 W); a shallow-tilt-compatible GCR of 0.40 would yield ~13.4 kW — a 14% undercount.

### Finding 2: Step-function latitude bands create sharp capacity discontinuities [Severity: medium]
**Description**: The `flat_roof_gcr` function uses hard thresholds at 30.0° and 40.0° latitude. Across the boundary at exactly 40.001°N, GCR jumps from 0.40 to 0.35 — a 12.5% reduction in GCR that maps to a 14.3% increase in panel footprint and corresponding 12.5% reduction in system capacity. Two identical buildings at latitudes 39.999°N and 40.001°N would estimate ~13.4 kW vs ~11.8 kW — a non-physical cliff.

For comparison, NREL SAM (via the SSC `sssky_diffuse_table`) computes GCR-based shading as a continuous function via integration over sky bins, not as a step lookup. EnergyPlus/PVWatts uses a flat GCR=0.4 default with no latitude dependence for fixed-tilt arrays.

**Code Location**: `crates/hares-physics/src/pv_sizing.rs:154-161`

**Root Cause**: The three-band step table was chosen for simplicity, but the physical GCR-latitude relationship is continuous (driven by the solar altitude geometry).

**Impact**: Buildings near latitude-band boundaries get up to 14% over/under-estimated capacity. In a batch processing pipeline, this creates artificial geographic clustering in capacity estimates.

### Finding 3: No differentiation between commercial and residential flat-roof racking [Severity: medium]
**Description**: The GCR table is applied uniformly regardless of building type, yet commercial flat roofs (ballasted racking, 5-10° tilt) and residential flat roofs (penetrating racking, 15-25° tilt) have fundamentally different row-spacing requirements. A ballasted 5° tilt commercial system can achieve GCR ~0.55-0.70 with modest winter shading, while a 20° residential flat-roof tilt requires GCR ~0.25-0.35 for similar shading tolerance. HARES uses a single GCR table and tilt cap that spans this range without distinguishing the use case.

The roof-shape inference logic (`infer_roof_shape`, line 476) does route "apartment"/"5+" facility types to `RoofShape::Flat`, but once classified as flat, the same GCR table applies to both a multifamily high-rise and a single-family detached dwelling with a flat roof.

EnergyPlus makes no such distinction — GCR is a single user-specified input per `Generator:PVWatts` object. OCHRE has no GCR or flat-roof logic at all.

**Code Location**: `crates/hares-physics/src/pv_sizing.rs:154-161` (GCR table), `:312-316` (panel_footprint), `:476-521` (infer_roof_shape)

**Root Cause**: The GCR table was designed with a "one size fits flat" assumption without considering that different flat-roof PV mounting systems produce different achievable GCR ranges.

**Impact**: Residential flat roofs (steeper tilt) are over-capacitized by the model; commercial flat roofs (shallow tilt) are under-capacitized. Both errors are bounded but systematic.

### Finding 4: East-west dual-tilt flat roof configuration is unmodeled [Severity: medium]
**Description**: The HARES model only considers south-facing rows on flat roofs. East-west (dual-tilt/sawtooth) configurations — where adjacent rows face east and west — are a common flat-roof PV design, especially in northern Europe and for maximizing roof-area utilization. These systems achieve GCR values of 0.60-0.90 because the geometry forms a continuous envelope with no horizontal gaps between rows. By not modeling east-west configurations, HARES may significantly undercount capacity for flat-roof buildings using this layout.

EnergyPlus supports arbitrary azimuth and tilt inputs per PV array; the duality is handled by defining two separate `Generator:PVWatts` objects (one east, one west). OCHRE does not model east-west at all. Neither provides an automated GCR adjustment for east-west layouts.

**Code Location**: `crates/hares-physics/src/pv_sizing.rs` — no east-west logic exists. Azimuth resolution (line 165) and north-facing filter (line 117) both assume south-preferring arrays.

**Root Cause**: The model was designed around the conventional residential assumption of south-facing tilted arrays with no consideration for the east-west flat-roof paradigm.

**Impact**: East-west flat-roof systems would be modeled as south-facing with standard GCR penalty, underestimating module count by potentially 30-60%.

### Finding 5: GCR values compared against NREL SAM / EnergyPlus / OCHRE benchmarks [Severity: low]
**Description**: HARES is unique among the three implementations in providing a latitude-dependent GCR lookup for flat roofs. Comparison:

| Implementation | GCR for fixed-tilt flat roof | Latitude-dependent |
|---|---|---|
| HARES | 0.35-0.50 (step table) | Yes (3 bands) |
| EnergyPlus/PVWatts | 0.40 (default, user overridable) | No |
| SAM/SSC cmod_pvwattsv5 | 0.40 (default) | No |
| OCHRE PV.py | Not implemented | N/A |

The HARES 0.40 default (at 30-40°N, and for `None` latitude) aligns exactly with the EnergyPlus/SSC default of 0.40. However, EnergyPlus's IDD notes that GCR applies only to 1-axis tracking arrays (`\note Applies only to arrays with one-axis tracking`), not to fixed-tilt — meaning EnergyPlus treats GCR as irrelevant for fixed-tilt arrays. HARES's use of GCR for fixed-tilt flat roofs extends beyond EnergyPlus's intended scope.

**Code Location**: `crates/hares-physics/src/pv_sizing.rs:154-161`; EnergyPlus IDD V9-6-0 line ~88070; SSC cmod_pvwattsv5.cpp line ~211

**Root Cause**: HARES innovates beyond the vendor reference models by applying GCR to a scenario (fixed-tilt flat roof) where the reference models either don't apply GCR or leave it as user input.

**Impact**: This is not a correctness issue — the HARES approach is a defensible extension. However, the model should document that it diverges from EnergyPlus/SAM convention, and that the GCR values are HARES-specific calibrations not sourced from the reference implementations.

### Finding 6: Usable-fraction × GCR interaction may double-count spacing [Severity: low]
**Description**: For flat roofs, the usable area calculation applies two multiplicative factors: `usable_fraction(Flat) = 0.70` (fire-code setbacks + obstructions, line 102) and then `panel_footprint = panel_area / GCR` (row spacing, line 313). The comment at line 96-97 states "Flat roofs handle row-spacing separately via GCR" — indicating these are intended to be independent factors. However, the 0.70 usable_fraction for flat roofs is close to the 0.75 used for gable roofs, which suggests it's accounting for similar setback/obstruction margins. The effective panel coverage for a flat roof is `usable_fraction × GCR = 0.70 × 0.35 = 0.245` at high latitudes, vs. `0.75` for a gable roof's south face. This ~3:1 ratio of gable-to-flat panel density seems physically plausible for strongly tilted residential roofs, but the factor composition should be explicitly validated against field data.

**Code Location**: `crates/hares-physics/src/pv_sizing.rs:102` (usable_fraction) and `:313` (panel_footprint)

**Root Cause**: The separation-of-concerns (setbacks vs. row-spacing) is logically correct, but the numerical interaction has not been calibrated against measured installations.

**Impact**: If actual flat-roof installations achieve higher-than-modeled GCR after setbacks (e.g., via edge-to-edge row layouts), HARES could systematically undercount flat-roof PV potential.

### Finding 7: Missing GCR adjustment for non-optimal azimuth on flat roofs [Severity: low]
**Description**: The GCR table assumes rows are oriented perpendicular to the sun's path (south-facing in northern hemisphere). If a flat-roof array must be rotated to match a non-ideal building orientation (e.g., building faces 135° or 225°), the effective row spacing requirement changes because the shadow geometry rotates relative to the row. HARES does not adjust GCR for azimuth deviation from south on flat roofs — the same GCR is applied regardless of the resolved azimuth.

The SSC diffuse_reduce and shadeFraction1x functions explicitly account for azimuth in their self-shading calculations via `solazi`, `azimuth`, and the full Appelbaum shadow geometry. HARES's simpler model does not capture this effect.

**Code Location**: `crates/hares-physics/src/pv_sizing.rs:312-316` — `panel_footprint` depends only on `flat_roof_gcr(latitude)`, not on azimuth.

**Root Cause**: The GCR model treats inter-row spacing as purely latitude-dependent without accounting for the orientation of the rows relative to the sun path.

**Impact**: For buildings oriented significantly east or west of south (e.g., 135° or 225°), the effective GCR should be lower than shown due to increased morning/afternoon row shading. The overestimate is modest (~5-10% capacity error at 45° azimuth deviation).

## Summary
- Total findings: 7
- Critical: 0
- High: 1 (GCR-tilt coupling)
- Medium: 3 (step-function bands, commercial/residential differentiation, east-west configuration)
- Low: 3 (SAM benchmark comparison, usable-fraction interaction, azimuth-GCR coupling)

## Recommendations
1. **Decouple tilt cap from GCR computation** or compute GCR dynamically from tilt and latitude: e.g., `gcr = 1 / (cos(tilt) + sin(tilt) / tan(min_solar_elevation))` with a design-point solar elevation appropriate to the latitude band. This would automatically self-consistently adjust GCR based on the actual installed tilt, eliminating the contradiction at high latitudes.

2. **Smooth the GCR-latitude function**: Replace the step table with a continuous function, e.g., `gcr = 0.52 - 0.005 × latitude` (clamped to [0.30, 0.55]) or a piecewise-linear interpolation that removes the 14% capacity cliff at band boundaries.

3. **Add a commercial/residential parameter** that adjusts both tilt cap and GCR range: commercial = max 15° tilt, GCR base +0.05-0.10; residential = max 35° tilt, GCR base -0.05.

4. **Add east-west dual-tilt flat roof support**: When the best plane is flat and the building is not strongly asymmetric, offer an east-west candidate with azimuth pairs (90°/270°) and an elevated GCR of ~0.70+ to represent the sawtooth configuration.

5. **Validate the `usable_fraction(Flat) = 0.70` × GCR product** against field survey data for known flat-roof installations, to ensure the composite effective coverage is empirically grounded.

## References / Citations
- **NREL SAM / SSC**: `cmod_pvwattsv5.cpp:211` — default GCR = 0.4, applied only to self-shaded 1-axis tracking arrays.
- **EnergyPlus IDD V9-6-0**: `Generator:PVWatts` field N5 — GCR default 0.4, documented as "applies only to arrays with one-axis tracking."
- **Appelbaum & Bany, 1979**: "Shadow effect of adjacent solar collectors in large scale systems," *Solar Energy* Vol 23 No. 6 — basis for SSC `lib_pvshade.cpp` row-spacing model (`ss_exec` at line 329).
- **SSC `diffuse_reduce`**: `lib_pvshade.cpp:158-212` — derives row spacing R = B/GCR from GCR, confirming the HARES convention is consistent.
- **OCHRE PV.py**: No flat-roof, GCR, or row-spacing logic; tilt/azimuth auto-detected from envelope model or defaulted to latitude and south.
- **HARES `pv_sizing.rs:154-161`**: Current flat_roof_gcr implementation with three-band lookup.
- **HARES `pv_sizing.rs:322-323`**: Tilt cap `latitude.min(25.0)` — constrains flat roof tilt independent of GCR computation.
