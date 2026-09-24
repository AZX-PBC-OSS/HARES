# HPXML construction material layers to RC network node mapping correctness
**Review ID**: rc-mat-01
**Category**: envelope-rc-mat
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-io/src/hpxml/building.rs` — HPXML parsing, material layer extraction, unit conversion dispatch
- `crates/hares-envelope/src/boundary_rc.rs` — RC graph construction, layer splitting, window fallback path
- `crates/hares-envelope/src/rc_network.rs` — RC network matrix assembly, floating-node elimination
- `crates/hares-physics/src/units.rs` — IP-to-SI conversion factors
- `crates/hares-physics/src/solar.rs` — EnergyPlus window model (Steps 1, 4, 5, 7)
- `crates/hares-core/src/dwelling/conversions.rs` — Building-to-BoundaryInput wiring, window U-factor decomposition

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/utils/envelope.py` — `create_rc_data()`, `calculate_window_parameters()`, `get_boundary_rc_values()`
- `vendors/OCHRE/ochre/Models/RCModel.py` — `create_rc_matrices()` star-mesh transform, RC state-space construction
- `vendors/EnergyPlus/src/EnergyPlus/HeatBalanceSurfaceManager.cc` — window layer heat balance, solar distribution

## Findings

### Finding 1: [Severity: low] Diurnal penetration depth criterion uses half-penetration-depth for conservative spatial resolution
**Description**: The `split_layer_count()` function at `boundary_rc.rs:81-96` computes the sub-layer count as `n = ceil(thickness / Λ)` where `Λ = √(α·P/(4π))` with `P = 86400 s`. This is the half-penetration-depth (δ_p/2) where the full penetration depth δ_p = √(α·P/π) per ISO 13786:2007 §6.2 and Incropera & DeWitt §5.8. Using half-depth ensures at least two RC sub-layers per full diurnal penetration depth for adequate wave-shape resolution.
**Code Location**: `crates/hares-envelope/src/boundary_rc.rs:81-96`
**Root Cause**: Deliberate design choice, correctly documented in the doc comment at lines 62-80.
**Impact**: Provides conservative spatial resolution (more nodes than strictly required by ISO 13786). This is not a defect — it improves transient accuracy at the cost of additional states. The angular frequency ω = 2π/86400 is correctly implicit in the derivation. The threshold check at lines 1209-1211 correctly gates splitting: only material with `density > 100 kg/m³ AND conductivity > 0.1 W/(m·K)` is considered for splitting.

### Finding 2: [Severity: low] Insulation correctly modeled as single-node; zero-capacitance layers properly pruned
**Description**: Pure insulation layers (very low conductivity, negligible density) are correctly excluded from the splitting gate at `boundary_rc.rs:1209-1211` by the `conductivity > SPLIT_MIN_CONDUCTIVITY (0.1 W/(m·K))` check. Typical insulation conductivities (0.02-0.05 W/(m·K)) fail this test and remain single-node. For the precomputed LUT path, zero-capacitance insulation layers in `build_precomputed_boundary()` are merged into adjacent resistors at lines 1392-1407, converting them to pure thermal resistances with no capacitance node. This matches OCHRE's `create_rc_data()` zero-cap pruning at `envelope.py:326-332`.
**Code Location**: `crates/hares-envelope/src/boundary_rc.rs:1209-1221` (split gate), `1392-1407` (zero-cap pruning)
**Root Cause**: Correct implementation matching OCHRE methodology.
**Impact**: No unnecessary RC nodes created for insulation. Minor edge case: materials with density > 100 kg/m³ but conductivity < 0.1 W/(m·K) (unusual combination, e.g. some aerogel insulation at ~120 kg/m³, k=0.015) would be excluded from splitting, which is still physically correct since such materials' diurnal wave penetrates extremely slowly.

### Finding 3: [Severity: low] Specific heat IP-to-SI conversion uses thermochemical BTU (4183.9987) instead of IT BTU (4186.8)
**Description**: The `specific_heat_btu_lb_f_to_j_kg_k()` conversion at `units.rs:158-161` delegates to the `uom` crate's `btu_per_pound_degree_fahrenheit` unit, which is based on the thermochemical BTU definition (~1054.35 J/BTU) yielding 4183.9987 J/(kg·K). The IT (International Table) BTU definition (1055.05585262 J/BTU) gives 4186.8 J/(kg·K), which is the standard for building energy simulation per ASHRAE HoF. The absolute difference is 0.067%, which is well below material property variability.
**Code Location**: `crates/hares-physics/src/units.rs:158-161`; test at lines 312-318 documents the discrepancy
**Root Cause**: The `uom` crate's `btu_per_pound_degree_fahrenheit` unit definition chooses the thermochemical rather than IT BTU as its energy base. The test comment at line 313 incorrectly states "uom uses the IT BTU" — it actually uses the thermochemical one.
**Impact**: Negligible (< 0.07%). Specific heat is used to compute layer capacitance (C = ρ·cp·thickness·area). For a typical 4-inch concrete slab (ρ=2400, cp≈880), the error in thermal mass is ~0.07% — dwarfed by material property uncertainty (±10%).

### Finding 4: [Severity: medium] EnergyPlus window model Step 1 is correctly implemented with an improvement over OCHRE
**Description**: HARES implements Steps 1, 4, and 5 of the E+ Simple Window Model. Step 1 (glass-to-glass resistance) at `solar.rs:688-706` correctly decomposes U-factor using the E+ polynomial: R_int from the piecewise ln/linear function, R_ext from Ro,w correlation, and R_glass = 1/U - R_int - R_ext. HARES diverges from OCHRE `envelope.py:302` which sets `res_ext_w = 0`, absorbing Ro,w into r_window. HARES separates Ro,w for accurate solar parameter computation in Step 5 where the glass-only R is needed for the `radiation_frac` voltage divider at `solar.rs:787-794`.

Step 4 (transmittance from SHGC) at `solar.rs:735-754` uses the E+ piecewise polynomial with interpolation in the 3.4-4.5 W/(m²·K) band — an improvement over OCHRE which hard-cuts at 3.95 with no interpolation.

Step 5 (absorbed solar split) at `solar.rs:756-794` computes interior/exterior solar film resistances, then `radiation_frac = (R_ext_s + R_glass/2) / (R_ext_s + R_glass + R_int_s)`, matching the E+ engineering reference voltage-divider formula. The StarMesh window decomposition at `boundary_rc.rs:755-862` correctly splits the interior film into convective and radiative components, with `h_conv = h_si - h_rad(ε=0.84)` and a floating window_node for star-mesh radiation participation.

Step 5 of the review query (model convective/radiative heat transfer at each glass surface) is handled in the StarMesh path: zone_air ↔ window_node via R_conv (convection-only), and window_node ↔ star_node via R_rad (linearized T⁴). This matches E+ "Option 2", TRNSYS Type 56, and ESP-r.
**Code Location**: Step 1: `crates/hares-physics/src/solar.rs:688-706`; Step 4: `solar.rs:735-754`; Step 5: `solar.rs:756-794`, `crates/hares-envelope/src/boundary_rc.rs:755-862`
**Root Cause**: Correct E+ implementation.
**Impact**: Correct window thermal behavior. The HARES window model is more rigorous than OCHRE's (which omits the separate exterior film R). No window layers exist in the HPXML material sense (windows use U-factor and SHGC with the E+ simple model decomposition), so per-layer absorbed solar distribution is inapplicable — the single-layer model with `radiation_frac` split is correct per E+ Step 5.

### Finding 5: [Severity: low] Framing factor default (0.25) slightly overstates wood framing effect
**Description**: The `parse_framing_factor()` function at `building.rs:1294-1297` returns a default of 0.25 for `WoodStud` construction when no explicit `<StudSpacing>`, `<StudWidth>`, or `<FramingFactor>` are provided. When stud geometry IS provided, `assembly_framing_factor()` at lines 1193-1215 correctly computes 0.23 for 2×4 at 16" OC (matching ASHRAE HoF 2021 Ch. 27 Table 6). The 0.25 default is ~9% higher than the ASHRAE assembly value of 0.23, producing a slight conservative overstatement of thermal bridging.
**Code Location**: `crates/hares-io/src/hpxml/building.rs:1294-1297` (default), `1193-1215` (computed)
**Root Cause**: The default is a standalone constant not derived from the same ASHRAE Table 6 values used in `assembly_framing_factor()`. When stud geometry is unspecified, the code assumes standard framing conditions; 0.23 would be more consistent with the calibrated computation. The `SteelFrame` construction type is correctly excluded from the default path (line 1296: `_ => None`) to prevent silent misapplication of the wood-based parallel-path method.
**Impact**: Minor (≤ 9% overstatement of framing fraction in the default case when HPXML stud geometry is absent). The effective R-value decrease from this is small since the framing path conductivity is only 0.144 W/(m·K). For most HPXML inputs that specify stud geometry, the computed value is correct.

### Finding 6: [Severity: low] Window E+ Step 2 (frame edge effects) not separately applied
**Description**: EnergyPlus Step 2 computes an overall window U-factor from separate center-of-glass and frame edge contributions. HARES does not implement this decomposition because HPXML provides the whole-window assembly U-factor directly — no separate glass-vs-frame decomposition is needed. The window `u_factor_w_m2_k` from HPXML is used directly in Step 1 decomposition. This is correct when the HPXML U-factor already represents the full assembly value, but users supplying center-of-glass U-factors (without frame correction) would need to manually apply frame correction before entering into HPXML.
**Code Location**: `crates/hares-core/src/dwelling/conversions.rs:269-287` (window boundary wiring)
**Root Cause**: Architectural simplification — HPXML schema provides one U-factor per window, not separate glass/frame components. The E+ Step 2 decomposition is designed for raw construction data (layer-by-layer glass properties + frame geometry), not for the simplified HPXML input format.
**Impact**: No error for standard HPXML inputs where U-factors represent the whole-window assembly. Edge case: center-of-glass U-factors entered into HPXML would produce slightly optimistic (lower-than-correct) U-factor, underestimating window conduction loss by 5-10% depending on frame fraction.

## Summary
- **Total findings**: 6
- **Critical**: 0
- **High**: 0
- **Medium**: 1 (Finding 4 — but this is a positive finding: HARES exceeds OCHRE in correctness)
- **Low**: 5 (Findings 1, 2, 3, 5, 6)

## Recommendations
1. **Fix test comment at `units.rs:313`**: The comment states "uom uses the IT BTU → 4183.9987 J/(kg·K)" but the value 4183.9987 is actually the thermochemical BTU conversion. IT BTU gives 4186.8. Correct the comment to avoid confusion.
2. **Align framing factor default with ASHRAE Table 6**: Change the WoodStud default from 0.25 to 0.23 at `building.rs:1295` to match the ASHRAE HoF 2021 Ch. 27 Table 6 assembly value for standard 2×4 at 16" OC wood-framed walls.
3. **Consider adding HPXML documentation note**: The E+ Step 2 clarification (Finding 6) is not a code defect, but users should be aware that HPXML U-factors must represent whole-window assembly values, not center-of-glass only.

## References / Citations
- ISO 13786:2007 §6.2 — Dynamic thermal characteristics, periodic penetration depth
- Incropera & DeWitt, *Fundamentals of Heat and Mass Transfer* §5.8 — Penetration depth for semi-infinite solid
- ASHRAE HoF 2021 Ch. 27 Table 6 — Assembly framing fractions for wood-stud walls
- ASHRAE HoF 2021 Ch. 27 §3.2 — Parallel-path method for wood framing
- ASHRAE HoF 2021 Ch. 33 Table 1 — Concrete material properties
- EnergyPlus Engineering Reference §Window Calculation Module — Steps 1–7
- EnergyPlus Engineering Reference §Inside Surface Heat Balance — TARP algorithm, Option 2
- OCHRE `envelope.py:294-339` — `create_rc_data()` zero-capacitance pruning
- OCHRE `envelope.py:405-431` — `calculate_window_parameters()` (Steps 4–5)
- OCHRE `RCModel.py:176-217` — `create_rc_matrices()`, state-space construction
