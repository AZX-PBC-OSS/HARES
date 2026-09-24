# Envelope Boundaries.csv and Boundary Types.csv: zone labels, assembly R-values
**Review ID**: defdata-02
**Category**: defaults-data
**Date**: 2026-05-26

## Files Reviewed
defaults/envelope/Envelope Boundaries.csv defaults/envelope/Envelope Boundary Types.csv

## Vendor/Reference Files Consulted
None

## Findings
### Finding 1: [Severity: critical] Envelope Boundaries.csv is declared but never loaded by the Rust LUT parser
**Description**: The doc comment at `crates/hares-io/src/envelope_lut.rs:2-6` states that `EnvelopeLookup::load()` parses three CSVs including `Envelope Boundaries.csv`. However, `load()` at lines 108-126 only reads `Envelope Boundary Types.csv` and `Envelope Materials.csv`. The `Envelope Boundaries.csv` file is never opened by any Rust code path. Zone-to-boundary-name resolution is instead performed by the hardcoded `resolve_boundary_name()` function at `envelope_lut.rs:288-369`, which is a hand-maintained duplicate of the CSV's zone mappings.
**Code Location**: `crates/hares-io/src/envelope_lut.rs:108-126` (load omits Boundaries.csv) and `:288-369` (hardcoded resolve_boundary_name)
**Root Cause**: The CSV loader was implemented for two of three files; the third was deferred and replaced by the hardcoded function.
**Impact**: Any edit to `Envelope Boundaries.csv` (adding a boundary, changing a zone label, correcting a pairing) will have zero effect on the Rust solver. The CSV and the hardcoded function can silently diverge. If a Python OCHRE run uses the CSV but a Rust run uses the hardcoded mapping, results will differ with no warning. This was previously flagged in `docs/reviews/config-io/config-io-02-defaults-csv-loading.md`.

### Finding 2: [Severity: high] Window has no assembly entries in Envelope Boundary Types.csv
**Description**: `Envelope Boundaries.csv` line 5 defines `Window,WD,EXT,LIV`, establishing the zone adjacency. However, `Envelope Boundary Types.csv` (401 lines) contains zero rows with `Boundary Name = "Window"`. Every other envelope boundary name (Exterior Wall, Roof, Floor, Door, Foundation Wall, Rim Joist, etc.) has at least one default assembly row. The `resolve_boundary_name()` function returns `None` for Windows (line 367), so no LUT lookup occurs. Windows are handled through a separate code path using the `Window` struct's `u_factor_w_m2_k`. While functionally correct today, this is a data completeness gap: there are no lookup-able window assembly defaults in the CSV that could serve as fallbacks or validation references.
**Code Location**: `defaults/envelope/Envelope Boundary Types.csv` (entire file — no Window rows) and `crates/hares-io/src/envelope_lut.rs:367`
**Root Cause**: Window thermal performance is determined by NFRC-rated U-factor/SHGC values rather than layered material assemblies, so the OCHRE reference dataset does not include window construction variants in the Boundary Types CSV.
**Impact**: If a future code path attempts to look up a window boundary type in the LUT, it will receive `None` with no indication that this is expected. No fallback R-value exists in the CSV for windows (the code-level fallback is `DEFAULT_ASSEMBLY_R_M2_K_W = 2.5` at `boundary_rc.rs:32`, which is far better than any real window and would be physically wrong if ever applied).

### Finding 3: [Severity: high] Non-monotonic stud cavity R-values in Foundation Ceiling, Garage Interior Ceiling, and Raised Floor assemblies
**Description**: In `Envelope Materials.csv`, the `FLOOR STUD AND CAVITY` layer for Foundation Ceiling (lines 712-730), Garage Interior Ceiling (lines 991-1009), and Raised Floor (lines 1094-1114) exhibits non-monotonic thermal resistance: the R-38 stud cavity has *lower* R-value than the Uninsulated variant.
```
Variant        | Stud Cavity R (m²K/W) | Stud Cavity k (W/m-K)
Uninsulated    | 0.1512                | 0.924
R-13           | 0.5915                | 0.236
R-19           | 1.4370                | 0.097
R-30           | 1.0320                | 0.135  ← decreases from R-19
R-38           | 0.1160                | 1.204  ← lower than Uninsulated
```
The R-38 stud cavity conducts approximately 13x more heat than the R-19 stud cavity, and even 30% more than the uninsulated cavity. The R-38 ceiling assembly's total R-value (5.86 m²K/W ≈ R-33.3 imperial) is approximately 12% below its nominal R-38 target.
**Code Location**: `defaults/envelope/Envelope Materials.csv:712-730` (Foundation Ceiling), `:991-1009` (Garage Interior Ceiling), `:1094-1114` (Raised Floor)
**Root Cause**: These values appear to be back-calculated from target assembly totals where rigid insulation contributes the bulk of the resistance, forcing the stud cavity layer conductivity to an artificially high value to balance the assembly. At R-38, the cavity layer becomes a near-zero-resistance air gap.
**Impact**: If these material layers are ever used to construct an RC network from first principles (rather than using the pre-computed assembly R-value), the thermal dynamics will be physically wrong — the stud cavity will act as a short-circuit. The assembly-level R-values stored in `Envelope Boundary Types.csv` are correct (they match the nominal targets), but the per-layer breakdown is physically dubious.

### Finding 4: [Severity: medium] No climate zone-based window U-factor or SHGC defaults
**Description**: The HARES codebase contains no IECC climate zone lookup for default window U-factor or SHGC values. Windows must provide `<UFactor>` and `<SHGC>` explicitly in the HPXML file, or the solver returns a hard error (`crates/hares-core/src/dwelling/solver_builder.rs:291-304`). There are no fallback defaults keyed by climate zone, building vintage, or IECC code cycle. The previous silent defaults (U=5.0 W/m²·K, SHGC=0.40) were replaced with loud errors in a prior fix, but the replacement was removal rather than substitution with IECC-appropriate defaults.
**Code Location**: `crates/hares-core/src/dwelling/solver_builder.rs:287-304` (U/SHGC required), `crates/hares-io/src/hpxml/validation.rs:313-339` (validation checks)
**Root Cause**: The silent defaults were identified as inappropriate (representing 1970s single-pane aluminum) and removed, but no climate zone lookup table was added as a replacement.
**Impact**: Users providing HPXML files without explicit window U-factor/SHGC will receive a hard error rather than a reasonable default. This is safer than a silent wrong answer, but less user-friendly than a climate-aware default. SHGC ranges per IECC (0.25-0.40 for cold climates, 0.20-0.30 for hot climates) are not enforced or suggested.

### Finding 5: [Severity: medium] Skylight HPXML boundary type is completely unsupported
**Description**: HPXML defines `Skylight` as a valid envelope boundary type, but the HARES `BoundaryType` enum (`crates/hares-io/src/hpxml/building.rs:49-60`) has no `Skylight` variant. The HPXML boundary spec list at lines 942-957 does not include skylights. The `parse_windows()` function at lines 975-1094 handles only windows. Neither `Envelope Boundaries.csv` nor `Envelope Boundary Types.csv` contains a Skylight entry. A skylight in an HPXML file would be silently ignored (never parsed), causing missing heat transfer and solar gain.
**Code Location**: `crates/hares-io/src/hpxml/building.rs:49-60` (enum definition), `:942-957` (boundary spec list), `:975-1094` (window parsing)
**Root Cause**: Skylights were not prioritized during the initial HPXML parser implementation.
**Impact**: Dwellings with skylights will undercount envelope heat loss and solar gain. This could be significant in buildings with large skylight areas (e.g., contemporary residential architecture).

### Finding 6: [Severity: medium] Foundation Wall to Outdoor zone mapping falls back to incorrect boundary name
**Description**: `resolve_boundary_name()` at `envelope_lut.rs:341` maps `(ZoneType::Foundation, ZoneType::Outdoor)` for `BoundaryType::Wall` to `"Exterior Wall"`. This represents a walkout basement wall (foundation zone exposed to ambient). However:
1. `Envelope Boundaries.csv` has no equivalent entry — Foundation Wall is defined only as `GND→FND` (line 10), not `EXT→FND`
2. An Exterior Wall assembly (typically wood-framed with siding and cavity insulation above grade) is physically different from a walkout foundation wall (typically concrete/CMU with interior insulation)
3. The Rim Joist boundary (EXT→FND, line 11) exists but covers only the band joist area, not the full foundation wall height
**Code Location**: `crates/hares-io/src/envelope_lut.rs:341` (fallback match arm)
**Root Cause**: The match wildcard at line 342 maps all unrecognized (ZoneType × ZoneType) pairs for walls to "Exterior Wall" as a catch-all.
**Impact**: A walkout basement wall will be assigned the thermal properties of an above-grade wood-stud exterior wall instead of a foundation wall assembly, misrepresenting its thermal mass and conductance.

### Finding 7: [Severity: medium] Garage Attached Wall variants omit exterior siding layer visible in Exterior Wall equivalents
**Description**: All 30+ Garage Attached Wall material assemblies in `Envelope Materials.csv` (lines 815-960) omit the exterior siding/finish layer even when the Boundary Type name explicitly references it. For example, `"ConcreteMasonryUnit, vinyl siding, 6-in Hollow, R-19"` (line 835) has layers `WALL RIGID INS → OSB → CONCRETE BLOCK → GYPSUM BOARD` — missing the `VINYL SIDING` layer present in the corresponding Exterior Wall variant. The same pattern holds for brick veneer, aluminum siding, wood siding, and all other finish types.
**Code Location**: `defaults/envelope/Envelope Materials.csv:815-960` (all Garage Attached Wall variants)
**Root Cause**: This may be intentional — the garage interior face of a shared house/garage wall would not have weather-resistant siding; the sheathing (OSB) faces the unconditioned garage. However, the Boundary Type naming is misleading and the R-value contribution of the siding layer (~R-0.04 to R-0.11 m²K/W) is lost.
**Impact**: Minor understatement of thermal resistance for garage-attached walls. The naming convention could cause confusion when users search for "vinyl siding" garage walls and get assemblies that lack siding.

### Finding 8: [Severity: low] Floor boundary with (Conditioned, Ground) adjacency maps to wrong name
**Description**: In `resolve_boundary_name()`, a `BoundaryType::Floor` with `(ZoneType::Conditioned, ZoneType::Ground)` adjacency falls through to the wildcard at line 356, returning `"Attic Floor"` instead of `"Floor"`. The default exterior zone for Floor is `Attic` (line 301), but when explicitly set to Ground, there is no matching arm. In practice, a floor-to-ground boundary should be a `BoundaryType::Slab`, not `Floor`, so this path may never be exercised with valid HPXML. However, `BoundaryType::Slab` with `(Conditioned, _)` correctly returns `"Floor"` at line 360.
**Code Location**: `crates/hares-io/src/envelope_lut.rs:350-356` (Floor match arms) and `:358-363` (Slab match arms)
**Root Cause**: The Floor match arms were written to handle attic, foundation, garage, and outdoor adjacencies but not ground adjacency, since slab-on-grade is expected to use the Slab HPXML element type.
**Impact**: Low — requires malformed HPXML where a slab-on-grade is tagged as a Floor rather than a Slab. If triggered, the slab would be assigned attic floor thermal properties, which would be wrong.

### Finding 9: [Severity: low] Envelope Materials.csv is loaded but its per-layer R/C values are not used for assembly R-value verification
**Description**: `EnvelopeLookup::load()` at `envelope_lut.rs:108-126` loads `Envelope Materials.csv` but the per-layer resistance and capacitance values are used only for constructing pre-computed RC lookup tables. There is no cross-validation step that sums the layer resistances from `Envelope Materials.csv` and compares them against the `Assembly R Value` column in `Envelope Boundary Types.csv`. Discrepancies like Finding 3 (non-monotonic stud cavity values) would be caught by such a check.
**Code Location**: `crates/hares-io/src/envelope_lut.rs:108-126` (load) and `:131-200` (lookup)
**Root Cause**: The two CSVs are treated as independent data sources rather than mutually verifying datasets.
**Impact**: No automated detection of data integrity issues between the materials definition and the assembly R-value claims.

## Summary
- Total findings: 9
- Critical: 1 (Envelope Boundaries.csv never loaded)
- High: 2 (Window missing from Boundary Types CSV, non-monotonic stud cavity R-values)
- Medium: 4 (no climate zone window defaults, Skylight unsupported, Foundation/Outdoor wall mapping, Garage Attached Wall missing siding)
- Low: 2 (Floor/Ground adjacency mapping gap, no cross-validation between materials and assembly R-values)

## Recommendations
1. Either load `Envelope Boundaries.csv` in `EnvelopeLookup::load()` and derive boundary names from it instead of the hardcoded `resolve_boundary_name()`, or remove the CSV and document that the hardcoded function is the single source of truth. The current split-authority pattern is fragile.
2. Add climate zone-based window U-factor and SHGC defaults keyed to IECC 2021 Table R402.1.2, with SHGC ranges of 0.25–0.40 for cold climates (CZ 5–8, passive solar benefit) and 0.20–0.30 for hot climates (CZ 1–3, cooling-load reduction).
3. Add a `BoundaryType::Skylight` variant to the Rust enum and implement HPXML parsing for `<Skylight>` elements (they share the same schema as `<Window>` in HPXML).
4. Add a `Foundation Wall (Walkout)` boundary type in both CSVs for the `EXT→FND` adjacency or ensure the existing Rim Joist/Foundation Wall types handle the full foundation wall height, not just the band joist.
5. Add cross-validation in `EnvelopeLookup::load()` that verifies the sum of per-layer resistances from `Envelope Materials.csv` equals the `Assembly R Value` in `Envelope Boundary Types.csv` within a small tolerance (e.g., 1%). Flag assemblies where the difference exceeds the tolerance.
6. Investigate and fix the back-calculation methodology that produces non-monotonic stud cavity R-values (Finding 3). If these are intentional to hit assembly targets, document the methodology and consider flagging layers where the computed conductivity exceeds the conductivity of still air (0.026 W/m·K).
7. Remove the siding material names from Garage Attached Wall Boundary Type labels, or add a note in the CSV header documenting that these assemblies intentionally omit the exterior siding layer because the garage face of the shared wall does not require weather-resistant cladding.
8. Add a `(ZoneType::Conditioned, ZoneType::Ground)` match arm to the `BoundaryType::Floor` branch in `resolve_boundary_name()` returning `"Floor"` to handle the slab-on-grade-via-Floor edge case, even if it should never occur in well-formed HPXML.

## References / Citations
- ASHRAE Handbook of Fundamentals 2021, Chapter 27, Table 6: framing factor for 2x4 wood stud at 16" O.C. is 0.23 (HARES uses 0.25 default at `crates/hares-io/src/hpxml/building.rs:1295`).
- ASHRAE Handbook of Fundamentals 2021, Chapter 27, Section 3.2: zone method required for steel studs (correctly implemented at `crates/hares-envelope/src/boundary_rc.rs:1557-1579`).
- IECC 2021 Table R402.1.2: fenestration U-factor and SHGC requirements by climate zone.
- HPXML v4.0: boundary type enumeration includes Wall, Roof, Floor, Foundation Wall, Slab, Rim Joist, Door, Window, Skylight.
- `crates/hares-io/src/envelope_lut.rs:288-369` — `resolve_boundary_name()` hardcoded boundary-to-name mapping.
- `crates/hares-io/src/envelope_lut.rs:108-126` — `EnvelopeLookup::load()` loads only 2 of 3 CSVs.
- `crates/hares-core/src/dwelling/solver_builder.rs:287-304` — Window U-factor/SHGC required (loud error).
- `vendors/OCHRE/ochre/utils/envelope.py:9-10` — OCHRE zone label dictionaries (ZONES, EXT_ZONES).
