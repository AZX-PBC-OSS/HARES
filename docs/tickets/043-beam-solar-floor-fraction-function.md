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

---

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] **Referenced line numbers still match.** `beam_floor_fraction` is at lines 10–12 of `crates/hares-envelope/src/thermal_solver/solar.rs`; calls are at lines 116 and 149 exactly as cited. `compute_solar_distribution_into` spans lines 231–293; `compute_solar_distribution_into_solar` spans lines 319–377.
- [x] **Described logic matches current implementation.** The function body is exactly `solar_altitude_deg.to_radians().sin().clamp(0.3, 0.9)`. The lower clamp of 0.3 is present. The two distribution functions are structurally identical byte-for-byte in their physics logic, differing only in the surface-info struct type (`InteriorSurfaceInfo` vs `InteriorSolarSurfaceInfo`), confirming the duplication claim.
- [x] **Bug is present (not already fixed).** The existing test `beam_floor_fraction_boundaries` at line 713 explicitly asserts `beam_floor_fraction(0.0) == 0.3`, confirming the 0.3 lower clamp is active and passing. The new regression test `beam_floor_fraction_low_altitude_walls_dominate` **fails** with the current code (`got 0.3` instead of `≈ sin(5°) ≈ 0.087`).
- [x] **OCHRE cross-check: diverges from OCHRE, but OCHRE does not use a fixed 0.6 per-altitude beam fraction either.**

  OCHRE (`vendors/OCHRE/ochre/Models/Envelope.py` lines 548–553) distributes all transmitted window solar — beam and diffuse alike — uniformly by `area × absorptivity / Σ(area × absorptivity)` across **all** interior surfaces without distinguishing floors from walls, and without any altitude dependence. There is no fixed 0.6 floor fraction anywhere in OCHRE's Envelope.py. The 0.6 literal that appears in Envelope.py line 48 is a natural-ventilation open-window-area multiplier, unrelated to solar distribution. OCHRE's approach is equivalent to EnergyPlus's MinimalShadowing/FullExterior diffuse distribution applied to everything (including beam), which is a different simplification from HARES's sine heuristic.

  The ticket's claim that "OCHRE `SolarModel.py` uses a fixed beam fraction of 0.6 for all altitudes" is **inaccurate**: there is no `SolarModel.py` in the vendored OCHRE, and no fixed 0.6 beam floor fraction in any OCHRE Python file. The 0.6 may originate from an older OCHRE version or a separate project; it is not present in the vendored code.

- [x] **EnergyPlus cross-check: HARES diverges from EnergyPlus. The sin(altitude) heuristic has no EnergyPlus precedent.**

  EnergyPlus Engineering Reference (Shading Module, confirmed via BigLadder docs for versions 8.0–24.1) describes two approaches: (a) MinimalShadowing/FullExterior — "all beam solar radiation entering the zone is assumed to fall on the floor, where it is absorbed according to the floor's solar absorptance" (100% to floor, none altitude-dependent); (b) FullInteriorAndExterior — geometric projection of sun rays through windows onto interior surfaces, computing overlap areas per surface, with no altitude heuristic. Neither uses a `sin(altitude)` formula or any numeric floor fraction between 0 and 1. There is no "§14.5 Table 14.2" with pre-tabulated distribution fractions in the current EnergyPlus Engineering Reference; the section cited in the ticket does not exist by that number in any version consulted.

---

### Web-Verified Citations

**Citation 1**: "EnergyPlus Engineering Reference §14.5 'Beam Solar Radiation Distribution' — geometry-based distribution fractions, Table 14.2"

- **Source found**: EnergyPlus Engineering Reference (BigLadder Software), versions 8.0–24.1 Shading Module; Table of Contents for EnergyPlus 24.1 Engineering Reference (https://bigladdersoftware.com/epx/docs/24-1/engineering-reference/)
- **Quoted passage**: From the Shading Module documentation (versions 9.4–9.6 confirmed): "FullInteriorAndExterior: This is the same as FullExterior except that instead of assuming all transmitted beam solar falls on the floor the program calculates the amount of beam radiation falling on each surface in the zone, including floor, walls and windows." The interior beam distribution formula is: `AISurf(SurfNum) = AbsIntSurf(SurfNum)/A(SurfNum) × Σ TBmi × Aoverlapi(SurfNum) × CosInci` — a geometric overlap-area calculation, not a table lookup.
- **Verdict**: **Incorrect.** No section numbered "§14.5" titled "Beam Solar Radiation Distribution" exists in the EnergyPlus Engineering Reference; the interior solar distribution content is in the Shading Module chapter (unnumbered or differently numbered). More importantly, EnergyPlus does **not** use altitude-dependent distribution fractions or a "Table 14.2" with pre-tabulated floor fraction values. The geometry-based approach cited is real (it is the FullInteriorAndExterior polygon-overlap method), but the section number and table are fabricated. The ticket's intent — that EnergyPlus uses something better than a sine heuristic — is correct, but the specific citation is wrong.

**Citation 2**: "ASHRAE Handbook of Fundamentals 2021 Ch. 18 §18.47 'Solar Radiation Through Fenestration' — simplified distribution factors by surface area and absorptance"

- **Source found**: ASHRAE Handbook of Fundamentals 2021 Table of Contents (https://www.ashrae.org/technical-resources/ashrae-handbook/table-of-contents-2021-ashrae-handbook-fundamentals); ASHRAE HoF 2017 Chapter 18 "Nonresidential Cooling and Heating Load Calculations" (https://handbook.ashrae.org/Handbooks/F17/IP/f17_ch18/f17_ch18_ip.aspx); ASHRAE HoF 2017 Chapter 15 "Fenestration" (https://handbook.ashrae.org/handbooks/F17/SI/f17_ch15/f17_ch15_si.aspx)
- **Quoted passage**: Chapter 18 of the ASHRAE HoF covers "Nonresidential Cooling and Heating Load Calculations" and does include fenestration heat gain, but contains no section numbered §18.47. Chapter 15 ("Fenestration") covers U-factors, SHGC, solar transmittance, and glazing properties but does not contain a section on how transmitted solar distributes to interior surfaces. Neither chapter contains simplified interior solar distribution fractions by surface area and absorptance.
- **Verdict**: **Incorrect.** The section number §18.47 does not exist in ASHRAE HoF Chapters 15 or 18 (2017 or 2021 editions). The description "simplified distribution factors by surface area and absorptance" likely paraphrases OCHRE's own area×absorptivity normalization or EnergyPlus's diffuse distribution algorithm, neither of which is specified in ASHRAE HoF Ch. 18. The ASHRAE citation is fabricated; no publicly accessible ASHRAE source confirms this section or these values.

**Citation 3**: "ASHRAE Standard 140-2017, Case 600 — fixed 0.6 floor fraction for BESTEST validation only, not a physics model"

- **Source found**: ASHRAE Standard 140 / BESTEST documentation reviewed via multiple sources: NREL reports (fy08osti/43827, fy10osti/47427), IEA BESTEST report (equaonline.com/iceuser/validation/old_stuff/BESTEST_Report.pdf), BigLadder Buildings library Modelica BESTEST Cases6xx (https://simulationresearch.lbl.gov/modelica/releases/v7.0.0/help/Buildings_ThermalZones_Detailed_Validation_BESTEST_Cases6xx.html)
- **Quoted passage**: The BESTEST documentation reviewed specifies Case 600 as "a light-weight building with room temperature control set to 20°C for heating and 27°C for cooling." None of the publicly accessible BESTEST or ASHRAE Standard 140-2017 documents confirm a "fixed 0.6 floor fraction" as a Case 600 specification. The 0.6 value does not appear in any cited source as a mandated beam solar floor fraction for Case 600. (The 0.6 literal in OCHRE's Envelope.py line 222 is a default solar absorptivity for non-attic surfaces — unrelated to beam floor fraction; the line 48 value 0.6 is a window opening-area factor.)
- **Verdict**: **Cannot verify.** No publicly accessible version of ASHRAE Standard 140-2017 or the IEA BESTEST original specification confirms a "0.6 floor fraction" as a Case 600 parameter. The claim may be based on the original 1995 IEA BESTEST report (Judkoff & Neymark, NREL/TP-472-6231), which is not freely available. The ticket's characterization — that 0.6 is a validation-only assumption, not a physics model — is plausible given EnergyPlus's MinimalShadowing behavior (which assigns 100% to floor, not 60%), but the specific 0.6 value in Standard 140-2017 Case 600 cannot be confirmed or denied from available sources.

---

### Legitimacy

- **Verdict**: **Partially Legitimate**

- **Rationale**: The core defect — a lower clamp of 0.3 that prevents `beam_floor_fraction` from reaching zero at sunrise/sunset altitude angles — is real, present in the code, and physically wrong. `sin(0°) = 0.0` is the correct lower bound; clamping to 0.3 deposits phantom heat to floor RC nodes at low solar angles. This is confirmed directly by reading the source (lines 10–12), by the existing test at line 714 which asserts the broken value (`beam_floor_fraction(0.0) == 0.3`), and by the new failing regression test. The duplication of ~60 lines across `compute_solar_distribution_into` and `compute_solar_distribution_into_solar` is also real and confirmed by inspection. However, two of the three citations are incorrect: the EnergyPlus section/table numbers are fabricated (no "§14.5 Table 14.2" exists), and the ASHRAE HoF §18.47 section does not exist. The claim about OCHRE using a fixed 0.6 floor fraction cannot be reproduced from the vendored OCHRE code — OCHRE distributes all solar uniformly by area×absorptivity with no beam/diffuse distinction and no floor-vs-wall split. The ticket's physics reasoning is sound and the fix is correct; the standards citations are unreliable and should be replaced with accurate references.

---

### Proposed Fix Summary

In `beam_floor_fraction` (solar.rs:10–12), change `.clamp(0.3, 0.9)` to `.clamp(0.0, 0.9)`. No other production-code change is required for the immediate deliverable. The duplication between `compute_solar_distribution_into` and `compute_solar_distribution_into_solar` is a separate refactor (extract shared beam-splitting and diffuse-loop logic into a generic function parameterized over the surface-info type).

---

### Test Written

- **File**: `crates/hares-envelope/src/thermal_solver/solar.rs` (within existing `#[cfg(test)] mod tests`)
- **Tests added**:
  - `beam_floor_fraction_zero_altitude_must_be_zero` (`#[should_panic]`): asserts `beam_floor_fraction(0.0) == 0.0`; currently the inner assert panics (because the value is 0.3), so the `#[should_panic]` wrapper passes — confirming the bug is present. After the fix the inner assert will no longer panic, and `#[should_panic]` should be removed.
  - `beam_floor_fraction_low_altitude_walls_dominate`: asserts `beam_floor_fraction(5°) < 0.15` (near sin(5°) ≈ 0.087); **currently FAILS** with `got 0.3` due to the lower clamp. Will pass after the fix.

Both tests were verified to exercise the actual bug (`cargo test -p hares-envelope solar` shows `beam_floor_fraction_low_altitude_walls_dominate FAILED: got 0.3`).
