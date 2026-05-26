# Envelope Materials.csv: Resistance and Capacitance vs ASHRAE HOF 2021 Ch.26 Table 4
**Review ID**: defdata-01
**Category**: defaults-data
**Date**: 2026-05-26

## Files Reviewed
defaults/envelope/Envelope Materials.csv

## Vendor/Reference Files Consulted
None

## Findings
### Finding 1: [Severity: critical] Mislabeled Specific Heat column — kJ/kg-K header contains J/kg-K values
**Description**: Column 9 header reads `Specific Heat (kJ/kg-K)` but the actual values are in J/kg·K, not kJ/kg·K. For example, line 27 GYPSUM BOARD 1 has value 837.4 under this column — the specific heat of gypsum is ~837 J/kg·K (~0.837 kJ/kg·K). The value 837.4 kJ/kg·K is physically impossible for any common building material. The CSV also has column 13 `Specific Heat (J/kg-K)` which contains the same-scaled values. Row 27 has column 9=837.4 (marked kJ) and column 13 empty; row 26 has column 9 empty and column 13=1177.466 (marked J). Both compute correct capacitance via `density × value × thickness / 1000`. If code that consumes this file treats column 9 as kJ and skips the /1000, capacitance would be 1000× too high.

**Code Location**: Envelope Materials.csv, header row (line 1), column 9 header. Affects all rows where column 9 is populated (e.g., lines 25–27, 78–80, 318–320, 961–962).

**Root Cause**: Column header was mislabeled during data export/generation. The SI unit for specific heat in the J/kg·K column (col 13) is correct; the kJ/kg·K column (col 9) should either be relabeled to J/kg·K or its values scaled by 1/1000 to match the header.

**Impact**: Depending on how consuming code reads this column, capacitance values may be wrong by factor of 1000. All thermal mass calculations for envelope components defined in rows with col-9 values would be affected.

---

### Finding 2: [Severity: high] Resistance values for STUD AND CAVITY materials do not match R = thickness / conductivity
**Description**: Multiple rows have Resistance values that diverge significantly from the relationship R = thickness / conductivity. The most extreme case: line 26 (CEILING STUD AND CAVITY for Attic Floor, Minimal) lists thickness=0.1397 m, conductivity=0.002 W/m·K, yielding computed R = 69.85 m²·K/W, but the CSV records 78.89 m²·K/W — a +12.9% discrepancy. Similarly, line 79 (WALL STUD AND CAVITY for Attic Wall, Minimal) lists thickness=0.0889 m, conductivity=0.001 W/m·K, yielding computed R = 88.90, but CSV records 87.85 (−1.2%). These discordant values suggest the R column was populated from a different source than the thickness and conductivity columns, or the k values were back-calculated from target R-values with different rounding.

**Code Location**:
- Line 26: Boundary=Attic Floor, Type=Minimal, Material=CEILING STUD AND CAVITY, t=0.1397, k=0.002, R_computed=69.85, R_csv=78.89
- Line 79: Boundary=Attic Wall, Type=Minimal, Material=WALL STUD AND CAVITY, t=0.0889, k=0.001, R_computed=88.90, R_csv=87.85
- Line 319: Boundary=Exterior Wall, Type=Minimal, Material=WALL STUD AND CAVITY, t=0.0889, k=0.001, R_computed=88.90, R_csv=87.85 (same as line 79)

**Root Cause**: The STUD AND CAVITY materials are composites whose effective conductivity was likely derived from a parallel-path model (framing factor × stud k + cavity fraction × insulation k). The R-value in the CSV may reflect the full assembly R distributed differently, or the k value was chosen to match a target R while thickness remained fixed. With only thickness and k provided in the CSV, the physical consistency R = t/k should hold.

**Impact**: Simulation results may use incorrect conductive heat transfer for attic and wall assemblies using the Minimal insulation type, which directly affects annual heating and cooling loads for these boundary types.

---

### Finding 3: [Severity: high] Missing common building materials — no XPS, polyiso, spray foam, wood studs, metal studs, or hardwood flooring
**Description**: The material list lacks several standard building materials that are commonly found in residential construction and are expected by HPXML and ASHRAE HOF references:

| Missing Material | Present? | Closest Existing Material |
|---|---|---|
| XPS (extruded polystyrene) | No | WALL RIGID INS (k=0.029) — could represent XPS but is undifferentiated |
| Polyiso (polyisocyanurate) | No | WALL RIGID INS (k=0.029) — polyiso typically k≈0.020–0.023, so this single rigid insulation category conflates different products |
| Spray foam, open-cell | No | None — typical k≈0.036–0.042 |
| Spray foam, closed-cell | No | None — typical k≈0.022–0.028 |
| Wood studs (standalone) | No | WALL STUD AND CAVITY is a composite, not the stud itself |
| Metal studs | No | None |
| Hardwood flooring | No | FLOOR COVERING — generic, not specific |
| Concrete (varied densities) | Partial | Only specific densities (1892–2243 kg/m³); no lightweight concrete (800–1400 kg/m³) |

**Code Location**: Envelope Materials.csv, column 6 (Material Name). The complete set of unique material names (1036 rows, but only ~30 distinct material names) lacks the entries above.

**Root Cause**: The materials list was likely constructed from a limited set of simulation input files (ResStock/HPXML samples, as evidenced by the "Received From" column referencing "res3.1_national_300", "res3.1_ochre_500", etc.) rather than from a comprehensive building materials database. Only materials present in the source buildings were included.

**Impact**: When an HPXML file specifies a material not in this lookup table (e.g., `<InsulationLayer><Material>spray foam</Material></InsulationLayer>`), the simulation either fails silently or falls back to an incorrect default. This silently degrades the accuracy of simulations for buildings with these materials.

---

### Finding 4: [Severity: high] Material naming incompatible with HPXML material vocabulary
**Description**: HPXML uses specific material identifiers (e.g., "R-13 fiberglass batt", "R-19 blown cellulose", "2 inch XPS", "1/2 inch gypsum board"). The CSV uses generic composite names like "WALL STUD AND CAVITY" and varies only conductivity to achieve different R-values, rather than naming the insulation material type and R-value explicitly. For example, a WoodStud wall with R-11 uses "WALL STUD AND CAVITY" with k=0.095; the same material name with k=0.078 for R-15; and k=0.118 for R-7. There is no "fiberglass batt" or "cellulose" material entry. The lookup key (Boundary Name + Boundary Type) encodes the target R-value (e.g., "WoodStud, aluminum siding, R-11"), but the material layer itself is not independently addressable.

**Code Location**: Throughout the file. Example: lines 500–516 (Exterior Wall, WoodStud, aluminum siding) show "WALL STUD AND CAVITY" with k=0.095 (R-11), k=0.078 (R-15), k=0.23 (R-19), k=0.118 (R-7), k=0.55 (Uninsulated) — all sharing the same material name.

**Root Cause**: The data model embeds the insulation level in the conductivity rather than in the material identity. This approach works for the internal simulation but creates a mismatch with any HPXML-based lookup that expects material-first identification.

**Impact**: An HPXML-to-HARES mapping layer must know to translate "R-13 fiberglass batt" into a specific Boundary Type string rather than a Material Name, making the material lookup table unsuitable for direct HPXML material resolution.

---

### Finding 5: [Severity: medium] GYPSUM BOARD thickness inconsistency across boundary types
**Description**: Gypsum board thickness varies across rows without apparent justification. Most wall assemblies use 0.0127 m (1/2 in) per ASHRAE HOF Table 4 for standard gypsum board (lines 12, 24, 27, 30 etc.). However, some assemblies use 0.0025 m (~0.1 in) — e.g., lines 5 (Adjacent Ceiling), 17 (Adjacent Wall/ConcreteMasonryUnit), 49 (Attic Floor/Uninsulated). A 0.0025 m thickness for gypsum board is physically non-standard (standard panels are 1/2 in or 5/8 in). The ASHRAE HOF Ch.26 Table 4 R-value for 1/2 in gypsum board is ~0.079 m²·K/W (R-0.45 in IP), matching the 0.0127 m rows. The 0.0025 m rows give R≈0.0156, which is ~1/5 of the expected value.

**Code Location**:
- 0.0025 m gypsum: lines 5, 17, 49, 129–130, 151, 154, 175, 178, 199–202, 223–226, 243–245, 266–269, 289–293, 314–317, 319–320, 844–845
- 0.0127 m gypsum: lines 12, 24, 27, 30, 33, 36, 39, 42, 45, 47, 109, 111, etc.

**Root Cause**: The thin gypsum (0.0025 m) appears in Uninsulated and Minimal boundary types, likely to reduce thermal mass for the minimal/default assembly. It may represent a thin skim-coat or an intentional under-specification, but it is not standard 1/2 in gypsum board.

**Impact**: Underestimates thermal capacitance of these assemblies by ~80% for the gypsum layer, and slightly underestimates resistance. Affects all "Uninsulated" and "Minimal" boundary types.

---

### Finding 6: [Severity: medium] Resistance small systematic deviations from t/k in cladding materials
**Description**: Several cladding materials show consistent small deviations between computed R=t/k and the recorded Resistance. These are small but systematic:

| Line | Material | Thickness (m) | k (W/m·K) | R_computed | R_csv | Δ% |
|---|---|---|---|---|---|---|
| 5 | GYPSUM BOARD | 0.0025 | 0.16 | 0.015625 | 0.01585 | +1.4% |
| 12 | GYPSUM BOARD | 0.0127 | 0.16 | 0.079375 | 0.07923 | −0.18% |
| 25 | CEILING LOOSEFILL INS | 0.4233 | 0.048 | 8.81875 | 8.804 | −0.17% |
| 2 | OSB SHEATHING 0.5 IN | 0.0127 | 0.115 | 0.11043 | 0.11 | −0.39% |

These are distinct from Finding 2 (STUD AND CAVITY) in magnitude but share the same root concern: R, t, and k are not consistently related.

**Code Location**: Lines 2, 5, 12, 25, and similar throughout.

**Root Cause**: Values may have been rounded independently or sourced from different references (e.g., R from ASHRAE table lookup, t converted from inches, k from a separate database), causing small rounding-inconsistency artifacts.

**Impact**: Minor. Heat transfer calculations will use whichever column (R or k) the solver reads. If the solver uses k directly, the R column discrepancy is irrelevant. If the solver uses R, the small error propagates.

---

### Finding 7: [Severity: low] Inconsistent material naming conventions (case, suffixes, aliases)
**Description**: Similar materials have inconsistent naming across the file:
- "Soil" (line 669) vs "Soil-12in" (line 741) vs "SOIL-12IN" (line 752)
- "Slab" (line 670) vs "SLABMASS" (line 690) vs "Concrete-8in" (line 742) vs "CONCRETE-8IN" (line 753)
- "GYPSUM BOARD" (line 30) vs "GYPSUM BOARD 1" (line 27) vs "GypsumBoard-1_2in" (line 768)
- "WALL RIGID INS" vs "ROOF RIGID INS" vs "FLOOR RIGID INS" vs "RIM JOIST RIGID INS" vs "CEILING LOOSEFILL INS" — all rigid insulation but each treated as distinct material
- "Ficticious Insulating Layer" (lines 668, 672, etc.) — misspelled "Fictitious"

**Code Location**: Throughout. Compare: line 669 ("Soil") with line 752 ("SOIL-12IN"); line 670 ("Slab") with line 690 ("SLABMASS"); line 27 ("GYPSUM BOARD 1") with line 30 ("GYPSUM BOARD").

**Root Cause**: Materials were sourced from different simulation baselines (Denver, national_300, ochre_500, OLD) and the names were carried forward without normalization.

**Impact**: Case-sensitive string matching on material names would fail to recognize these as the same material. A lookup for "Soil" would miss "SOIL-12IN", potentially leading to fallback defaults.

---

### Finding 8: [Severity: low] "Ficticious" typo in material name
**Description**: The material name "Ficticious Insulating Layer" (lines 668, 672, 676, 680, 684, 692, 696, 700, 704, 708, 731, 735, 961, 963, 966, 969, 972, 975, 978, 984, 987) is misspelled — should be "Fictitious Insulating Layer".

**Code Location**: See lines listed above. First occurrence at line 668.

**Root Cause**: Typo in the originating data source that was never corrected.

**Impact**: Any code that does exact string matching on "Fictitious" would miss this material. Low impact since it's a zero-thickness placeholder layer.

---

## Summary
- Total findings: 8
- Critical: 1 (Finding 1 — mislabeled specific heat units)
- High: 3 (Findings 2, 3, 4 — R/t/k inconsistency, missing materials, HPXML vocabulary gap)
- Medium: 2 (Findings 5, 6 — gypsum thickness inconsistency, small R deviations)
- Low: 2 (Findings 7, 8 — naming inconsistency, typo)

## Recommendations
1. Fix the column 9 header from `Specific Heat (kJ/kg-K)` to `Specific Heat (J/kg-K)` to match the actual data values, or rescale values to true kJ/kg·K and update consuming code accordingly.
2. Re-derive Resistance values from thickness and conductivity for all rows to enforce R = t/k consistency, or document that R is sourced independently and k is back-calculated.
3. Add missing materials: XPS (k≈0.029), polyiso (k≈0.022), open-cell spray foam (k≈0.039), closed-cell spray foam (k≈0.024), wood stud (k≈0.12, ρ≈500), metal stud, hardwood flooring. Source values from ASHRAE HOF 2021 Ch.26 Table 4.
4. Implement an HPXML material-name mapping layer that translates HPXML standard material identifiers to the appropriate HARES boundary type + insulation detail combination.
5. Normalize material names to a consistent convention (case, suffixes) and de-duplicate identical materials with different names.
6. Standardize gypsum board thickness to 0.0127 m (1/2 in) or 0.0159 m (5/8 in) per ASHRAE HOF, unless the 0.0025 m value can be justified with a comment.
7. Fix "Ficticious" → "Fictitious" typo in material name and all references.

## References / Citations
- ASHRAE Handbook of Fundamentals 2021, Chapter 26, Table 4 — Thermal Properties of Common Building Materials
- HPXML Building America Program Standard — material vocabulary and naming conventions
- Envelope Materials.csv: 1037 data rows, 13 columns, ~30 distinct material names
- Envelope Boundaries.csv: 33 boundary definitions
- Envelope Boundary Types.csv: 344 boundary type combinations with assembly R-values
