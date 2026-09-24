# All 16 Ashrae152ZoneType variants — default insulation, leakage, and seasonal parameters
**Review ID**: dse-deep-02
**Category**: ashrae152-deep
**Date**: 2026-05-26

## Files Reviewed
crates/hares-physics/src/ashrae152.rs

## Vendor/Reference Files Consulted
vendors/EnergyPlus/src/EnergyPlus/

The EnergyPlus source tree does not reference ASHRAE Standard 152 by name. It models the same physical phenomena (duct conduction, duct leakage, zone distribution effectiveness, buried pipe heat transfer) through AirflowNetwork-based duct loss objects (`Duct:Loss:Conduction`, `Duct:Loss:Leakage`, `Duct:Loss:MakeupAir`) and zone air distribution effectiveness parameters (`ZoneADEffCooling`/`ZoneADEffHeating`), but does not implement the simplified tabular approach of ASHRAE 152. No seasonal multiplier or per-zone-type insulation default tables comparable to ASHRAE 152 Tables 5-A through 5-D were found.

## Findings

### Finding 1: [Severity: high]
**Description**: All 16 zone type variants share a single, flat uninsulated R-value default (R-1.7 IP, ≈ R-0.3 m²·K/W) regardless of zone type. ASHRAE 152 Tables 5-A through 5-D define distinct default insulation R-values that vary by zone type, climate zone, and season. For example, ducts in unconditioned vented attics, unvented attics, crawlspaces, and basements each have different tabulated default insulation levels depending on whether the building is in a heating-dominated or cooling-dominated climate. The current implementation collapses all of these into a single value.
**Code Location**: `crates/hares-physics/src/ashrae152.rs:362-371`
**Root Cause**: The `supply_r` and `return_r` local variables apply a hardcoded 1.7 IP fallback when the user-provided nominal R-value is ≤ 0. The `Ashrae152ZoneType` parameter carried in `input.zone_type` is not consulted during R-value selection: lines 387-396 pass `zone_type` only to `zone_temps()`, and lines 362-371 compute the R-value transforms from `supply_nom_r_ip`/`return_nom_r_ip` alone, without any zone-type dispatch.
**Impact**: Buildings in cold climates with ducts in unconditioned spaces default to unrealistically low insulation when the user omits explicit R-values. This inflates duct conduction losses and depresses DSE below what the ASHRAE 152 reference method would produce. Conversely, ducts in mild climates may receive the same conservative default when a lower insulation value would be appropriate. Every building that relies on the `≤ 0 → uninsulated default` path (lines 362, 367) is affected.

### Finding 2: [Severity: high]
**Description**: The `DuctDseInput` struct exposes `supply_leakage_frac` and `return_leakage_frac` as direct 0–1 fraction inputs (lines 56–63) with no abstraction for ASHRAE 152 duct leakage classes. The standard defines a table of default leakage classes — e.g., "well-sealed" (~2 CFM/100 ft² at 25 Pa), "sealed" (~6 CFM/100 ft²), "unsealed" (~12 CFM/100 ft²), etc. — and these vary by whether the duct is in a vented attic, unvented attic, crawlspace, basement, or under slab. The leakage fraction should be derived from leakage class, duct surface area, and fan flow, but instead the code requires the caller to supply a raw fraction. No zone-type-dependent leakage class default is ever inferred.
**Code Location**: `crates/hares-physics/src/ashrae152.rs:56-63` (struct fields); lines 446–448 (leakage used as-is without zone-type dispatch)
**Root Cause**: The `Ashrae152ZoneType` enum carries no leakage-class association. The inputs `supply_leakage_frac` and `return_leakage_frac` are treated as fully opaque user values, with no internal mapping from zone type → leakage class → leakage area per unit surface area → leakage fraction.
**Impact**: Leakage fractions must be supplied externally (HPXML or other tool) or the DSE calculation defaults to zero leakage, giving an unrealistically optimistic result. There is no guardrail preventing a user from assigning a blower-door leakage fraction appropriate for an attic to a slab installation. The absence of the leakage-class abstraction also means the split between leakage-to-outside (loss) vs. leakage-to-zone (neutral/partial regain) is not separately parameterised per zone type, which ASHRAE 152 distinguishes.

### Finding 3: [Severity: medium]
**Description**: No seasonal delivery effectiveness multipliers are applied anywhere in `calculate_dse`. The function computes an uncorrected delivery effectiveness from steady-state heat transfer principles (lines 497–524), then adjusts it for thermal regain (lines 563–566), but does not apply the tabulated heating-season and cooling-season delivery effectiveness multipliers specified in ASHRAE 152 Tables 5-A through 5-D. These multipliers are empirical corrections that account for cyclic losses, part-load effects, and air distribution patterns not captured by the steady-state NTU-effectiveness model, and they vary by zone type, equipment type, and season.
**Code Location**: `crates/hares-physics/src/ashrae152.rs:497-566`
**Root Cause**: The calculation path goes directly from `seas_uncorr_de` → `seas_de` (with regain) → `seas_dse` (with equipment, load factor, and cycle loss) without an intermediate tabulated multiplier lookup step. There is no data table for these multipliers anywhere in the crate.
**Impact**: The computed DSE may systematically diverge from ASHRAE 152 reference values, particularly for zone types where the empirical multiplier differs significantly from 1.0. For example, ASHRAE 152 heating-season multipliers for ducts in unconditioned attics are typically in the 0.80–0.95 range.

### Finding 4: [Severity: medium]
**Description**: The `UnderSlab` variant (line 313) approximates the duct zone temperature as the mean ground temperature (`gnd`) with regain factors of 0.2. ASHRAE 152 provides buried-duct correction factors that depend on burial depth, soil thermal conductivity, duct diameter, and duct insulation R-value. These tabulated factors modify the effective zone temperature to account for the thermal mass and moderating effect of the surrounding soil, which reduces both heating and cooling losses compared to above-ground duct runs. The current implementation omits burial depth and soil conductivity entirely.
**Code Location**: `crates/hares-physics/src/ashrae152.rs:313`
**Root Cause**: The `zone_temps()` function signature (line 192–199) accepts only `zone`, four temperature values, and `gnd` (ground temperature). There is no parameter for burial depth or soil conductivity. The `DuctDseInput` struct (lines 48–82) likewise has no fields for these buried-duct properties.
**Impact**: Under-slab duct losses are modelled using an uncorrected ground temperature, which may overestimate summer losses (soil is cooler than ambient air at depth) and underestimate winter losses depending on burial depth. The DSE for buildings with slab-embedded ducts may not match ASHRAE 152 reference calculations.

### Finding 5: [Severity: low]
**Description**: The 16 variants cover all ASHRAE 152 exterior/interstitial zone types: 4 attic (vented, vented with radiant barrier, unvented, unvented with radiant barrier), 1 garage, 6 crawlspace (3 vented × 3 unvented insulation configurations), 3 basement (uninsulated, insulated walls, insulated ceiling), 1 under-slab, and 1 exterior-wall. However, there is no variant for ducts fully within the conditioned space. The standard treats conditioned-space ducts as having DSE = 1.0 with a neutral leakage effect (leakage recirculates within the conditioned envelope and is not an energy loss). The HARES code handles this upstream by returning `DuctConfig::default()` (DSE = 1.0) when `zone_type_str` is `None` (`resolve_hvac.rs:343-345`). This is functionally correct but not implemented as an enum variant.
**Code Location**: `crates/hares-physics/src/ashrae152.rs:26-43` (enum definition); `crates/hares-io/src/hpxml/resolve_hvac.rs:343-345` (upstream short-circuit)
**Root Cause**: Design choice — the conditioned-space case is handled as a caller-level short-circuit rather than a first-class zone type.
**Impact**: Low. The caller-level guard (returning DSE = 1.0 when no zone type string is present) produces the correct result. However, any code path that constructs a `DuctDseInput` with a zone type intended to represent conditioned space would produce a non-unity DSE, since no variant maps to "no losses."

### Finding 6: [Severity: low]
**Description**: Naming inconsistency in crawlspace variant identifiers. The enum uses `UnventUninsulatedCrawlspace` (line 32) and `VentUninsulatedCrawlspace` (line 35) — dropping the "-ed" suffix from "Unvented" and "Vented" respectively. The string keys in `resolve_hvac.rs` (lines 353, 356) use consistent short forms (`"unvent_unins_crawlspace"`, `"vent_unins_crawlspace"`), and `hvac/helpers.rs` (lines 304, 307) mirrors these. The full ASHRAE 152 convention would use "Unvented" and "Vented."
**Code Location**: `crates/hares-physics/src/ashrae152.rs:32,35` (enum); `crates/hares-io/src/hpxml/resolve_hvac.rs:353,356` (string keys); `crates/hares-equipment/src/hvac/helpers.rs:304,307` (string keys)
**Root Cause**: Inconsistent truncation of the "-ed" suffix during naming.
**Impact**: Cosmetic. All three locations agree internally, so serialisation/deserialisation is consistent. The issue is primarily one of documentation clarity and alignment with the ASHRAE 152 taxonomy.

### Supplementary Note: Zone temperature formulas and regain factors

For completeness, the zone temperature formulas in `zone_temps()` (lines 192–323) and the supply/return regain factors were reviewed against known ASHRAE 152 Table 5 patterns:

| Variant (lines) | Heating formula (design) | Cooling formula (design) | Supply regain | Return regain |
|---|---|---|---|---|
| AtticVented (201–208) | h_des + 10 | c_des + 22 | 0.1 | 0.1 |
| AtticVentedRadiantBarrier (209–216) | h_des + 10 | 0.65×(c_des+22) + 0.35×78 | 0.1 | 0.1 |
| AtticUnvented (217–224) | h_des + 10 | c_des + 36 | 0.1 | 0.1 |
| AtticUnventedRadiantBarrier (225–232) | h_des + 10 | 0.65×(c_des+36) + 0.35×78 | 0.1 | 0.1 |
| Garage (233–240) | h_des + 13 | c_des + 7 | 0.1 | 0.1 |
| UnventUninsulatedCrawlspace (241–248) | (2×h_des + 3×68)/5 | (2×c_des + 3×78)/5 | 0.6 | 0.6 |
| UnventCrawlspaceInsFloorWall (249–256) | (3×h_des + 68)/4 | (3×c_des + 78)/4 | 0.6 | 0.6 |
| UnventCrawlspaceInsFloor (257–264) | (5×h_des + 68)/6 | (5×c_des + 78)/6 | 0.3 | 0.3 |
| VentUninsulatedCrawlspace (265–272) | (h_des + 68)/2 | (c_des + 78)/2 | 0.6 | 0.6 |
| VentCrawlspaceInsFloorWall (273–280) | (5×h_des + 68)/6 | (5×c_des + 78)/6 | 0.63 | 0.63 |
| VentCrawlspaceInsFloor (281–288) | (8×h_des + 68)/9 | (8×c_des + 78)/9 | 0.3 | 0.3 |
| UninsulatedBasement (289–296) | (5×gnd + 2×h_des + 3×68)/10 | (5×gnd + 2×c_des + 3×78)/10 | 0.5 | 0.5 |
| BasementInsWalls (297–304) | (gnd + 68)/2 | (8×gnd + c_des + 78)/10 | 0.6 | 0.6 |
| BasementInsCeiling (305–312) | (3×gnd + h_des)/4 | (3×gnd + c_des)/4 | 0.6 | 0.6 |
| UnderSlab (313) | gnd | gnd | 0.2 | 0.2 |
| ExteriorWalls (313–321) | (h_des + 68)/2 | (c_des + 78)/2 | 0.2 | 0.2 |

These temperature formulas and regain factors are internally consistent and follow the weighted-average patterns prescribed by ASHRAE 152. The regain factors correctly reflect the thermal coupling hierarchy: crawlspaces and basements (0.3–0.63) have higher regain than attics (0.1) and under-slab/exterior (0.2). No deviations from expected values were identified in the temperature or regain-factor components.

## Summary
- Total findings: 6
- Critical: 0
- High: 2
- Medium: 2
- Low: 2

## Recommendations
1. **Add per-zone-type insulation R-value defaults**: Extend the `Ashrae152ZoneType` enum (or a companion table) with the default insulation R-values from ASHRAE 152 Tables 5-A through 5-D, keyed by zone type and optionally by climate zone or heating/cooling mode. The flat R-1.7 fallback (lines 362, 367) should dispatch to a zone-type-specific default when the zone type is known.
2. **Implement ASHRAE 152 leakage class abstraction**: Define a `DuctLeakageClass` enum (well-sealed, sealed, unsealed, and the specific CFM/100ft²-at-25Pa values from Table 5) and associate each `Ashrae152ZoneType` with a default leakage class. Derive `supply_leakage_frac` and `return_leakage_frac` from the leakage class, duct surface area, and fan airflow rather than requiring the caller to provide a raw fraction.
3. **Add seasonal delivery effectiveness multiplier lookup**: Embed the tabulated heating-season and cooling-season multipliers from ASHRAE 152 Tables 5-A through 5-D and apply them as a correction factor to `seas_uncorr_de` or `seas_de` before the final DSE computation. These should be dispatched on `zone_type`, `is_heating`, and optionally `is_heat_pump`.
4. **Add buried-duct correction parameters**: Extend `DuctDseInput` with optional `burial_depth_m` and `soil_conductivity_w_m_k` fields. For the `UnderSlab` variant, apply the ASHRAE 152 buried-duct correction table to adjust the effective zone temperature instead of using the raw ground temperature.
5. **Consider adding a `ConditionedSpace` variant** to make the DSE = 1.0 path explicit within the physics layer rather than relying on upstream short-circuit logic.
6. **Normalise variant naming** to use full ASHRAE 152 convention (e.g., `UnventedUninsulatedCrawlspace`) for alignment with the standard, while maintaining backward-compatible string parsing for the short forms.

## References / Citations
- ASHRAE Standard 152-2014, "Method of Test for Determining the Design and Seasonal Efficiencies of Residential Thermal Distribution Systems," Tables 5-A through 5-D (zone temperature formulas, default insulation R-values, duct leakage classes, seasonal delivery effectiveness multipliers, and buried-duct correction factors)
- HARES source: `crates/hares-physics/src/ashrae152.rs` — `Ashrae152ZoneType` enum (lines 26–43), `DuctDseInput` struct (lines 48–82), R-value default fallback (lines 362–371), delivery effectiveness computation (lines 497–566)
- HARES source: `crates/hares-io/src/hpxml/resolve_hvac.rs` — zone type string→enum mapping (lines 347–364), conditioned-space DSE short-circuit (lines 343–345)
- HARES source: `crates/hares-equipment/src/hvac/helpers.rs` — parallel zone type string→enum mapping (lines 296–317)
- EnergyPlus reference: No structurally comparable ASHRAE 152 default tables found. EnergyPlus uses a different modelling approach (AirflowNetwork + explicit duct loss objects) rather than the simplified standardised tables.
