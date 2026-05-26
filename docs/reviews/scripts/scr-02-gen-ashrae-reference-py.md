# gen_ashrae_reference.py: every formula verified against ASHRAE HOF 2021
**Review ID**: scr-02
**Category**: scripts
**Date**: 2026-05-26

## Files Reviewed
- `scripts/gen_ashrae_reference.py` (1023 lines)

## Vendor/Reference Files Consulted
- `tests/fixtures/parity/ashrae_rc_reference.json` (generated output fixture)
- `tests/structural_envelope_oracle.rs` (parity test harness consuming the fixture)
- `crates/hares-physics/src/film_coefficients.rs` (HARES Rust film resistance implementation)
- `crates/hares-core/src/dwelling/conversions.rs` (HARES Rust boundary/slab construction)
- `crates/hares-envelope/src/boundary_rc.rs` (HARES RC node construction)

## Findings

### Finding 1: [Severity: critical]
**Description**: The JSON fixture (`ashrae_rc_reference.json`) is desynchronized from the current Python source. The fixture was generated from a significantly older version of `gen_ashrae_reference.py` and does not reflect values the current source code would produce. Every parity test that consumes this fixture is testing against stale data.
**Code Location**: `scripts/gen_ashrae_reference.py` vs `tests/fixtures/parity/ashrae_rc_reference.json` (entire file)
**Root Cause**: Multiple code changes were committed to the Python source without re-running the script to regenerate the fixture. The fixture became a "historical artifact" rather than a living reference.
**Impact**: **All parity tests** that consume this fixture (`structural_envelope_oracle.rs:695-700`) are testing against potentially incorrect reference values. If the script were re-run today, it would produce a different JSON file, causing parity tests to fail — either because the new values are more correct (exposing HARES bugs) or because the Python source itself has regressed (false test failures).

**Specific divergences found between source and fixture**:

| Parameter | Source value (current) | Fixture value (stale) | Source line |
|---|---|---|---|
| `MIN_DELTA_T_TARP_NATURAL_K` | 0.1 K | 12.9 K (implicit) | `gen_ashrae_reference.py:90` |
| Exterior Wall `r_film_int_m2_k_w` | ~0.446 (TARP at ΔT=5K) | 0.3255 (TARP at ΔT=12.9K) | line 221 |
| Attic Wall `r_film_int_m2_k_w` | ~0.644 (TARP at ΔT=1.67K) | 0.3255 (TARP at ΔT=12.9K) | line 221 |
| Attic Floor `r_film_int_m2_k_w` | ~0.305 (TARP at ΔT=5K) | 0.5611 (TARP at ΔT=12.9K) | line 719 |
| Floor `r_film_int_m2_k_w` | ~0.305 (TARP at ΔT=10K) | 0.2475 | line 743 |
| Floor `r_layer_m2_k_w` | ~0.97 (from `_FLOOR_LAYERS`) | 2.009 (from F-factor) | line 742 |
| Floor `n_nodes` | 3 (`ASSEMBLY_N_NODES`) | 1 (stale) | line 417 |
| Floor slab citation text | "BEopt Residential Construction Ref" | "ASHRAE HoF Ch. 18.31 F-factor" | lines 959-964 |
| Floor slab methodology | 4-layer ASHRAE Table 4 stack | F-factor perimeter method | lines 339-347 |

---

### Finding 2: [Severity: critical]
**Description**: Interior film resistance algorithm mismatch between the reference script and HARES. The reference script uses **TARP natural convection** (`tarp_h_natural`, variable with ΔT), while HARES uses **ASHRAE Simple** (`ashrae_simple_interior_h_conv`, fixed values by orientation). These are fundamentally different physics models and produce different results at all ΔT values except for vertical surfaces at the accidental 12.9K convergence point.
**Code Location**: 
- Reference script: `gen_ashrae_reference.py:105-121` (`tarp_h_natural`)
- HARES: `crates/hares-physics/src/film_coefficients.rs:209-225` (`ashrae_simple_interior_h_conv`)
**Root Cause**: The reference script was designed to mimic OCHRE's TARP-based approach, but HARES has since been refactored to use ASHRAE Simple (EnergyPlus's default interior convection algorithm). The two algorithms coincidentally produce nearly identical results for vertical surfaces only when ΔT is clamped to 12.9K (to within 0.12%, as noted at `film_coefficients.rs:268-269`), but diverge at all other ΔT values.
**Impact**: Even after regenerating the fixture, the reference script's TARP-based film resistances will systematically differ from HARES's ASHRAE Simple values for all non-vertical surfaces and all surfaces at small ΔT. The parity test would fail for attic floor (heat flow up at small ΔT), roof (pitched), and slab (horizontal). The reference script cannot serve as an oracle for HARES's film resistance computations because it uses a different convection model.

**Illustrative ΔT = 5K comparison (LIV=20°C, EXT=15°C)**:
| Orientation | TARP (reference) | ASHRAE Simple (HARES) | Δ% |
|---|---|---|---|
| Vertical (90°) | h=2.24, r=0.446 | h=3.076, r=0.325 | +37% |
| Horizontal enhanced | h=2.60, r=0.385 | h=4.040, r=0.248 | +55% |
| Horizontal reduced | h=0.50, r=2.01 | h=0.948, r=1.055 | +90% |

Note: With the old 12.9K clamp, vertical TARP matches ASHRAE Simple to 0.12%. The MIN_DELTA_T change from 12.9K to 0.1K (line 90) breaks this accidental convergence.

---

### Finding 3: [Severity: high]
**Description**: The `MIN_DELTA_T` change from 12.9K to 0.1K (line 90) is based on a misinterpretation. The source comment claims the old 12.9K value "caused R_film to be 17–27% too low," but this is incorrect. The 12.9K TARP clamp produces values that match EnergyPlus's **default** interior convection algorithm (ASHRAE Simple), which is what HARES and most E+ models actually use. Changing to 0.1K makes the reference script produce film resistances that diverge from both HARES and EnergyPlus defaults.
**Code Location**: `gen_ashrae_reference.py:86-90`
**Root Cause**: The TARP algorithm is sensitive to ΔT. Clamping to 12.9K was an OCHRE convention that happened to give reasonable film resistances across all surface orientations. **EnergyPlus's `MIN_DELTA_T = 0.1` is the TARP-specific floor**, but EnergyPlus's default algorithm is ASHRAE Simple (fixed h_conv), not TARP. The comment at lines 88-89 incorrectly implies that E+ uses TARP with 0.1K, when E+ defaults to ASHRAE Simple which is ΔT-independent.
**Impact**: The MIN_DELTA_T change makes the reference script diverge from HARES for all surfaces, not just non-vertical ones. The comment about fixing "17-27% too low" film resistance is misleading — the HARES values ARE the ASHRAE Simple values, which match the old 12.9K TARP clamp.

---

### Finding 4: [Severity: high]
**Description**: Floor (slab-on-grade) construction methodology mismatch. The current Python source (`_FLOOR_LAYERS`, lines 339-347) builds the slab from a 4-layer ASHRAE Table 4 stack (fictitious F-factor resistor + 12" soil + 4" concrete + carpet). However, HARES's Rust implementation (`conversions.rs:166-209`) uses the **F-factor perimeter method** (ASHRAE 90.1 §5.5.3.1 / HoF Ch. 18.31): `UA = F2 × P` where `F2 = 1.17 W/(m·K)`, producing a single `PrecomputedRCLayer` with R-value derived from the perimeter conductance. These are structurally different:
- **Reference script (current source)**: Multi-layer R-stack, R ≈ 0.97 m²·K/W (very thin, dominated by fictitious layer)
- **HARES Rust**: F-factor perimeter method, R = area/F2/P − r_film_int ≈ 2.01 m²·K/W
- The fixture has R = 2.009, matching HARES's F-factor approach, but the current source would generate R ≈ 0.97.
**Code Location**: `gen_ashrae_reference.py:339-347` (`_FLOOR_LAYERS`) vs `conversions.rs:166-209`
**Root Cause**: The `_FLOOR_LAYERS` multi-layer stack was intended to replicate the ASHRAE Table 4 build-up for documentation, but HARES uses the F-factor method which produces a completely different thermal resistance. The reference script needs to model slabs the same way HARES does.
**Impact**: The slab boundary would have wildly different UA in the reference vs HARES output. The parity test would fail for Floor UA by ~50% (2.009 vs 0.97 for r_layer).

---

### Finding 5: [Severity: high]
**Description**: The `FICTITIOUS INSULATING LAYER` tuple (line 343) misuses its fields: `thickness=0.4237` is actually an R-value (m²·K/W) stored in the thickness slot, with `k=1.0` to make R = thickness/k = 0.4237. The density and specific heat are set to zero. This is a non-obvious encoding convention that could easily be misinterpreted by future maintainers.
**Code Location**: `gen_ashrae_reference.py:343`
**Root Cause**: The `_Material` tuple format `(name, thickness_m, conductivity, density, cp)` has no way to represent a pure resistance (no mass). The workaround uses thickness-as-R with a dummy k=1.0.
**Impact**: If anyone changes the fictitious layer thinking "thickness" means physical thickness, they'd break the slab R-value. The `_assembly_r_capacitance_kj` function at line 388-392 handles this correctly (0.4237 / 1.0 = 0.4237 resistance, 0.4237 * 0.0 * 0.0 / 1000 = 0 capacitance), but the intent is obscured.

---

### Finding 6: [Severity: medium]
**Description**: The `interior_film_r` citation in the `_provenance` block (line 932-935) is factually misleading. It claims: "ASHRAE Handbook of Fundamentals 2021, Ch. 26 Table 1 (still-air + radiation, eps=0.9, T=293.15 K); EnergyPlus Engineering Reference v25.1.0 §9.4 (TARP)." But:
1. ASHRAE Ch. 26 Table 1 gives **combined** (convection + radiation) film resistances: 0.12 vertical, 0.10 upward, 0.16 downward — none of which are used in this script.
2. The script uses TARP convection-only film resistance (1/h_conv), explicitly excluding radiation.
3. The citation implies the ASHRAE tabulated values are the basis, but they are not.
**Code Location**: `gen_ashrae_reference.py:932-935`
**Root Cause**: The citation appears to describe what the reviewer *expected* to see rather than what the code actually computes.
**Impact**: Misleads future readers about the provenance of the film resistance values. The script does not use ASHRAE Ch. 26 Table 1 values.

---

### Finding 7: [Severity: medium]
**Description**: The reference script's `film_resistances` function uses the same `h_conv` (interior TARP) for the Ground exterior film resistance (line 228: `r_ext = 1.0 / h_conv`). For ground-contact boundaries, ASHRAE HOF Ch. 26 Table 1 does not prescribe a film resistance — the exterior is a fixed temperature node. HARES's Rust code correctly zeros out `r_film_exterior` for slabs (`conversions.rs:294-296`). The reference script computes a non-zero exterior film for ground, which diverges from HARES.
**Code Location**: `gen_ashrae_reference.py:227-228`; `conversions.rs:294-296`
**Root Cause**: The `film_resistances` function applies interior TARP h_conv to ground contact as a fallback, but slabs should have zero exterior film.
**Impact**: The Floor boundary total R would include an erroneous exterior film contribution, making the reference UA lower than HARES. The fixture output shows `r_film_ext_m2_k_w = 0.0` for Floor (line 76 of JSON), meaning the old script did zero it — but the current source code would NOT zero it.

---

### Finding 8: [Severity: medium]
**Description**: `ASSEMBLY_N_NODES["floor"]` is hardcoded to 3 (line 417), but the current `_FLOOR_LAYERS` has 4 layers with 2 being real material layers (soil + concrete) and 1 being zero-mass (carpet — density is 1.214 kg/m³ which is effectively zero). The comment says "3 nodes (interior, mid, outer)" but the node count should be derived from the actual layer stack using the same splitting rules as HARES's `split_layer_count` (`boundary_rc.rs:81-96`), not hardcoded.
**Code Location**: `gen_ashrae_reference.py:416-417`; `boundary_rc.rs:81-96`
**Root Cause**: Node counts are manually specified rather than computed from layer properties (conductivity, density, specific heat, thickness) using the diurnal penetration depth criterion that HARES applies. If any layer property changes, the manual count will be wrong.
**Impact**: Minor — the fixture currently has n_nodes=1 for Floor (matching HARES's F-factor single-layer approach), but the source code says 3. This is part of the broader fixture/source desync issue.

---

### Finding 9: [Severity: medium]
**Description**: The specific-heat value for "CARPET + FIBROUS PAD" is 40050 J/(kg·K) (line 346). This is two orders of magnitude higher than any building material in ASHRAE Ch. 26 Table 4 (typical range: 800–1500 J/(kg·K)). The value appears to be an error or a "fake thermal mass" encoding that shouldn't be in a script that claims to use ASHRAE Table 4 values.
**Code Location**: `gen_ashrae_reference.py:346`
**Root Cause**: ASHRAE Ch. 26 Table 4 lists typical carpet/fibrous-pad specific heat as ~1.38 kJ/(kg·K), not 40 kJ/(kg·K). This value may come from a non-standard source or be a deliberate "lumped mass" encoding.
**Impact**: The carpet layer capacitance would be: `0.0254 × 1.21 × 40050 / 1000 = 1.23 kJ/(m²·K)`. At 40050 J/(kg·K), this is a very small contribution (density is 1.21 kg/m³). But the value itself is physically unrealistic and undermines the script's claim of ASHRAE provenance.

---

### Finding 10: [Severity: medium]
**Description**: The `h_radiation_interior` function (lines 124-134) computes `h_rad = 4 × ε × σ × T³` and is used only in `film_resistances_combined` (line 249) for the `r_interior_combined` output. However, `r_interior_combined` is never used in the boundary construction code — only `r_conv` and `r_ext` are consumed. The `_film_combined` closure captures all three return values but only uses `r_fi` (conv-only) and `r_fe`. `r_fi_eff` (combined) is discarded at every call site (lines 664, 690, 719, et al.).
**Code Location**: `gen_ashrae_reference.py:124-134`, `gen_ashrae_reference.py:235-251`, and all calls at lines 664, 690, 719, 743, 769, 820, 850, 884.
**Root Cause**: The combined resistance was likely intended for a previous version of the RC topology that merged convection and radiation into a single film coefficient. Since the code now exports convection-only R_film and handles LWR separately, the combined value is dead code.
**Impact**: Dead code that could confuse maintainers. If someone accidentally uses `r_fi_eff` instead of `r_fi`, they'd get film resistances ~30-40% lower than intended.

---

### Finding 11: [Severity: medium]
**Description**: The window U-factor decomposition (`simple_glazing_interior_film_r`, lines 259-272) returns an interior film resistance that includes both convection AND radiation. This is correct per EnergyPlus's Simple Window Model (the decomposition includes h_rad in the film coefficient). However, the HARES RC model's opaque surfaces use convection-only film resistances (see Finding 2). This means the window r_film_int (0.1386 in the fixture) is a *combined* coefficient while wall r_film_int (0.3255) is *convection-only*. This asymmetry is correct per E+ but may surprise readers since both use the same JSON field name (`r_film_int_m2_k_w`).
**Code Location**: `gen_ashrae_reference.py:259-272`; JSON fixture lines 33-34 vs 102-103
**Root Cause**: EnergyPlus legitimately treats windows and opaque surfaces differently. Windows use the Simple Glazing Model which bakes radiation into the interior film, while opaque surfaces use convection-only film with explicit LWR handling.
**Impact**: If a test author naively expects all `r_film_int_m2_k_w` values to have the same physical meaning, they might misinterpret window vs wall film resistances. This is a documentation/schema clarity issue.

---

### Finding 12: [Severity: low]
**Description**: The citation for the EnergyPlus Simple Glazing Model references "Engineering Reference v25.1.0 §3.2" (line 29). In EnergyPlus Engineering Reference documentation, the Simple Glazing Model is typically in the "Window Calculation Module" section or "Envelope" chapter, not necessarily §3.2. The §3.2 numbering may be from a different edition or document. The regression coefficients (0.359073, 6.949915, 1.788041, 2.886625) come from Arasteh et al. (LBNL, 2009) and are correct, but the section number citation is questionable.
**Code Location**: `gen_ashrae_reference.py:29, 946-947`
**Root Cause**: Section numbers in EnergyPlus documentation change across versions and document types (Engineering Reference vs Input Output Reference).
**Impact**: Low — the actual numerical coefficients are correct. The section number is only for provenance tracking.

---

### Finding 13: [Severity: low]
**Description**: Material property citations claim all values come from "ASHRAE Handbook of Fundamentals 2021 Ch. 26 Table 4" (lines 283-286), but several materials in the ASSEMBLY_LAYERS are not in that table:
- **Vinyl siding** (k=0.089): ASHRAE Ch. 26 Table 4 does not list vinyl siding. This value is from BEopt's Residential Construction Reference (NREL/TP-5500-64459).
- **Wall stud and cavity with effective k**: The effective cavity conductivity (0.078 for R-15 wall) is a BEopt parallel-path framing correction, not a Table 4 value.
- **Carpet + fibrous pad** (k=0.0867, cp=40050): Neither value matches any Table 4 entry.
**Code Location**: `gen_ashrae_reference.py:304, 316, 346`
**Root Cause**: The comment at lines 283-288 correctly notes that "BEopt's Residential Construction Reference... recommends as the canonical ASHRAE-derived input" but the inline citation at line 950-957 should cite both ASHRAE Table 4 AND BEopt, not claim all values are from Table 4.
**Impact**: Low — the effective R-values are physically reasonable as engineering approximations. The citations are slightly inaccurate.

---

### Finding 14: [Severity: low]
**Description**: The window reference citation (line 947) says "Arasteh et al., LBNL 2009" but the correct citation for the Simple Window Model regression is Arasteh, D., Kohler, C., Griffith, B. (2009). "Modeling Windows in EnergyPlus with Simple Performance Indices." LBNL-2804E. The "LBNL 2009" shorthand is ambiguous.
**Code Location**: `gen_ashrae_reference.py:947`
**Root Cause**: The inline comment is too terse for a proper academic citation.
**Impact**: Very low — the numerical values are correct, but a proper LBNL report number would aid traceability.

---

### Finding 15: [Severity: low]
**Description**: The `_round` function (lines 611-614) uses Python f-string formatting (`f"{x:.{n}f}"`) which applies banker's rounding (round-half-to-even). However, `round()` in Python also uses banker's rounding. The f-string approach converts through a string, which can introduce subtle floating-point representation changes. A simple `round(x, n)` would be equivalent and avoid the string round-trip.
**Code Location**: `gen_ashrae_reference.py:611-614`
**Root Cause**: Use of string formatting for rounding is less conventional and slightly slower than `round()`.
**Impact**: Low — deterministic behavior is preserved, but the pattern is unusual.

---

### Finding 16: [Severity: low]
**Description**: Floor boundary uses `_round(r_slab_layer, 6)` (line 751) while all other boundaries use `_round(r_layer, 5)` or `_round(r_layer, 4)`. The inconsistent precision could cause test failures if a consumer expects consistent decimal places.
**Code Location**: `gen_ashrae_reference.py:751` vs lines 672, 698, 727, 829
**Root Cause**: The Floor slab build-up produces small individual layer R-values, so higher precision was likely added during debugging.
**Impact**: Very low — the JSON schema is schema-less (no fixed precision spec), so consumers handle arbitrary decimals. But it suggests the slab computation was problematic enough to need extra digits.

---

### Finding 17: [Severity: low]
**Description**: The roof tilt angle is computed as `atan(rise/12)` at line 768, but the TARP h_natural formula requires tilt in degrees from horizontal. The `math.degrees(math.atan(...))` is correct, but the `FilmInputs` object has `tilt_deg` which is passed directly. If `roof_pitch_rise_12` is missing from HPXML, `parse_hpxml` will panic at line 575 (`roofs[0]["pitch_rise_12"]`) — there's no error handling for missing pitch data.
**Code Location**: `gen_ashrae_reference.py:575, 768`
**Root Cause**: The parser assumes the first roof always has a `Pitch` element. HPXML allows `Pitch` to be optional.
**Impact**: Low — the script is designed for a specific fixture file (`BEopt_example.xml`) which has `Pitch`. If run against a different HPXML file without pitch data, the script would crash with a non-obvious error.

---

## Summary
- Total findings: 17
- Critical: 2 (findings 1, 2)
- High: 4 (findings 3, 4, 5, 6)
- Medium: 5 (findings 7, 8, 9, 10, 11)
- Low: 6 (findings 12, 13, 14, 15, 16, 17)

## Recommendations

1. **Regenerate the fixture immediately**: Run `uv run python scripts/gen_ashrae_reference.py` to produce a current `ashrae_rc_reference.json`. Then run the structure oracle tests to see which tests break. The breakage will reveal where HARES and the reference diverge.

2. **Align the interior film resistance algorithm**: The most fundamental fix is to make the reference script use the same ASHRAE Simple algorithm that HARES uses (`ashrae_simple_interior_h_conv` in `film_coefficients.rs:209-225`), or vice versa. Having two different convection models for a parity test will never produce consistent results except at the accidental 12.9K convergence point. The Python script should call `film_coefficients.ashrae_simple_interior_h_conv()` from a shared physics module or replicate the fixed h_conv values.

3. **Use F-factor for slabs consistently**: Either:
   - Make the reference script use F-factor perimeter method (matching HARES)
   - Or refactor HARES to use the multi-layer approach (matching the script)
   The current split causes the Floor boundary to have structurally different thermal resistances.

4. **Derive node counts, don't hardcode them**: Implement `split_layer_count()` from `boundary_rc.rs:81-96` in the Python script so node counts are computed from layer properties (thickness, conductivity, density, specific heat) using the diurnal penetration depth criterion. This ensures the reference node count always matches HARES.

5. **Fix the `interior_film_r` citation**: Replace "ASHRAE HOF 2021 Ch. 26 Table 1 (still-air + radiation)" with the actual algorithm used. If the code uses TARP, cite EnergyPlus §9.4 TARP. If it switches to ASHRAE Simple, cite ASHRAE 1985 Table 1 per EnergyPlus ConvectionCoefficients.cc.

6. **Fix the carpet specific heat**: 40050 J/(kg·K) is unrealistic. Either use a real ASHRAE Table 4 value (~1380 J/(kg·K) for carpet) or document that this is a non-standard engineering approximation.

7. **Add a CI guard**: Add a check in CI that compares the generated fixture against a golden copy, or at minimum verifies that `gen_ashrae_reference.py` runs successfully and produces valid JSON. Currently there is no enforcement that the fixture is up-to-date.

8. **Remove dead code**: `r_interior_combined` from `film_resistances_combined` is computed but never used in boundary construction. Either integrate it or remove the combined resistance computation.

## References / Citations
- ASHRAE Handbook of Fundamentals 2021, Ch. 26, Tables 1 and 4
- EnergyPlus Engineering Reference v25.1.0, §9.4 (TARP), §9.5 (DOE-2), Simple Glazing Model
- ASHRAE Standard 90.1-2022, Appendix A (unit conversions), §5.5.3.1 (slab F-factor)
- Arasteh, D., Kohler, C., Griffith, B. (2009). "Modeling Windows in EnergyPlus with Simple Performance Indices." LBNL-2804E.
- BEopt Residential Construction Reference, NREL/TP-5500-64459, April 2016
- Walton, G. N. 1983. TARP Reference Manual, NBSSIR 83-2655
- ASHRAE Handbook of Fundamentals 1985, p. 23.2, Table 1
- Incropera & DeWitt, *Fundamentals of Heat and Mass Transfer* §5.8
- ISO 13786:2007 §6.2
