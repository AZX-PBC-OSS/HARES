# Garage Infiltration Uses `ach50 / 20` Rule-of-Thumb Instead of AIM-2 Physics

**Severity**: High
**Priority**: P2
**Status**: Open
**Areas**: hares-core/dwelling/solver_builder.rs

## Problem

Garage zone infiltration is computed as `building.infiltration_ach50 / 20.0`
with `unwrap_or(0.5)` fallback (`crates/hares-core/src/dwelling/solver_builder.rs:941–947`).
This has three independent physics errors:

**1. Fixed N=20 divisor ignores climate.** The AIM-2 N-factor (ACH50 → natural ACH)
depends on stack and wind coefficients, which are functions of building geometry,
terrain, and shielding class. Per Walker & Wilson 1998, N ranges from ~10 in
windy southern climates (CZ1) to ~22 in sheltered northern climates (CZ7). A
fixed divisor of 20 introduces 67–100% error in natural ACH for CZ1 buildings.

**2. Building ACH50 applied to garage.** `building.infiltration_ach50` is measured
for the conditioned envelope. The garage is a separate, typically less airtight
zone with its own leakage characteristics. Applying the conditioned-zone ACH50 to
the garage conflates two distinct air barriers.

**3. `InfiltrationMethod::Ach` used.** The result is a constant annual natural ACH,
bypassing wind and stack dynamics entirely. All other zones use physics-based methods
(`AshraeWindStack`, `Ela`) that vary by timestep.

Note: ticket 052 documents that `building.infiltration_ach50` may already be wrong
(CFM50 parsed as ACH50). Fix ticket 052 first; this ticket assumes correct ACH50
is available after that fix.

## Current Behavior

`crates/hares-core/src/dwelling/solver_builder.rs:941–947`:

```rust
ZoneType::Garage => {
    let garage_ach = building
        .infiltration_ach50
        .map(|ach50| ach50 / 20.0)  // fixed N, wrong zone leakage source
        .unwrap_or(0.5);
    InfiltrationMethod::Ach { ach: garage_ach }
}
```

The conditioned zone (same file, ~line 870–928) calls `aim2_coefficients_from_ach50`
with height, shielding, and terrain parameters. The garage zone does not.

OCHRE assigns garage `ach50 / n_factor` where `n_factor` is climate-specific,
not a fixed 20.

## Required Behavior

Per Walker & Wilson 1998 (AIM-2) and ASHRAE HoF 2021, Ch. 16 §4.3, the
N-factor must be derived from the AIM-2 stack and wind coefficients for the
zone. Per EnergyPlus Engineering Reference §27.4.3, the ELA-based model
produces time-varying infiltration from SLA and zone geometry. Both are
superior to a constant ACH.

When no garage-specific leakage measurement is available, ASHRAE 152-2004
(Residential HVAC Performance) and the EnergyPlus residential template specify
`SLA = 3.0 × 10⁻⁴` for an attached unconditioned garage. There is no silent
`unwrap_or(0.5)` fallback — if no leakage data is available and the ASHRAE
default is not applied, the error must surface loudly.

## Approach

1. Implement `garage_infiltration_method(building, zone, building_height_m, terrain_class) -> Result<InfiltrationMethod, HaresError>`
   analogous to `foundation_infiltration_method` and `attic_infiltration_method`.
2. If `infiltration_cfm50` is available for the garage zone (or as a building-level
   measurement with explicit garage attribution from HPXML), derive ELA and
   compute AIM-2 coefficients using garage floor area and height.
3. If no garage-specific leakage measurement is available, apply the ASHRAE 152
   default `SLA = 3.0e-4` with a `tracing::warn!` naming the source standard.
   Convert to ELA: `ela_m2 = SLA × garage_floor_area_m2`. Use
   `InfiltrationMethod::Ela` with attic-style coefficients appropriate to garage
   height.
4. Remove `ach50 / 20.0` and `unwrap_or(0.5)` entirely. If the ASHRAE 152
   default is also unacceptable for a given run configuration, error loudly
   referencing the missing HPXML element.
5. Use `InfiltrationMethod::Ela` throughout; not `InfiltrationMethod::Ach`.

## Definition of Done

- [ ] `garage_infiltration_method` function exists, returns `InfiltrationMethod::Ela`.
- [ ] `ach50 / 20.0` and `unwrap_or(0.5)` paths removed.
- [ ] ASHRAE 152 SLA default applied with `tracing::warn!` when no zone-level
      leakage data is present.
- [ ] Test: CZ1 climate (N≈10) garage produces ≈2× higher natural ACH than a
      fixed-N=20 calculation would give.
- [ ] Test: missing leakage data → ASHRAE 152 default applied → ELA non-zero.

## Verification

```bash
cargo test -p hares-core garage_infiltration
cargo test -p hares-physics aim2_coefficients_from_ach50
```

## References

- Walker, I.S. and Wilson, D.J. (1998), "Field Validation of Algebraic Equations
  for Stack and Wind Driven Air Infiltration Calculations," HVAC&R Research 4(2):
  119–139. (AIM-2 model; N-factor climate dependence).
- ASHRAE Handbook of Fundamentals 2021, Ch. 16 §4.3 (Residential infiltration
  models — LBL model N-factor limitations).
- ASHRAE Standard 152-2004 §6.2 (Unconditioned attached garage default SLA).
- EnergyPlus Engineering Reference §27.4.3 (Effective Leakage Area model).
- `crates/hares-physics/src/infiltration.rs` — `aim2_coefficients_from_ach50`
  provides correct N-factor computation.

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match — `ZoneType::Garage` arm confirmed at
  `solver_builder.rs:941–947`. The `ach50 / 20.0` and `unwrap_or(0.5)` code is
  present exactly as described.
- [x] Described logic matches current implementation — the conditioned zone
  (lines 896–932) calls `aim2_coefficients_from_ach50` with full physics;
  the garage zone does not, using a flat constant instead.
- [x] `foundation_infiltration_method` (lines 1137–1178) and
  `attic_infiltration_method` (lines 1180–1221) both exist as referenced,
  using `InfiltrationMethod::Ela` with SLA-derived ELA.
- [x] `garage_ela_coefficients` already exists in
  `crates/hares-physics/src/infiltration.rs:655–663`, implementing
  `hor_lk_frac = 0.4` from Walker & Wilson (1998) Table 2. The building
  blocks for the fix are already implemented.

**OCHRE cross-check result: DIVERGES — and the ticket's description of OCHRE
is factually incorrect.**

In `vendors/OCHRE/ochre/utils/hpxml.py:736–742`, OCHRE assigns garage
infiltration as:

```python
indoor_ach = indoor_infiltration["BuildingAirLeakage"]["AirLeakage"]  # ACH50
zones["Garage"] = {
    "Infiltration Method": "ACH",
    "Air Changes (1/hour)": indoor_ach,  # raw ACH50, no division
}
```

OCHRE applies the *raw ACH50* (no division by any N-factor, not even 20) as a
constant ACH to the garage. The code even has a `# FUTURE: convert to ELA?`
comment at line 729 acknowledging the limitation. The ticket claims "OCHRE
assigns garage `ach50 / n_factor` where `n_factor` is climate-specific" — this
is **wrong**. OCHRE applies `ACH50` directly, which is even more physically
incorrect than HARES's `ach50 / 20`. HARES is partially better than OCHRE on
this point (it at least converts to a rough natural ACH rather than using
blower-door pressure as a steady-state rate). OCHRE does implement
`calculate_ela_coefficients` for garages (`envelope.py:636–686`) with
`hor_lk_frac = 0.4`, but this path is not reached for garages — the `"ACH"`
method is used instead.

**EnergyPlus cross-check result: PARTIALLY MATCHES**

EnergyPlus Engineering Reference (bigladdersoftware.com, v23.2, Infiltration/
Ventilation chapter) documents both:
- The **Effective Leakage Area (Sherman-Grimsrud) model**: formula
  `Infiltration = AL/1000 × √(Cs·ΔT + Cw·WindSpeed²)`, sourced from ASHRAE
  HoF 2001 Ch. 26 / 2005 Ch. 27, referred to as the "Basic" model.
- The **Flow Coefficient (AIM-2) model**: sourced from Walker & Wilson (1998),
  referred to as the "Enhanced" model in the same ASHRAE chapters.

The ticket's citation of "§27.4.3" is not a verifiable section number. The
EnergyPlus Engineering Reference does not use numbered subsections in the
online HTML documentation; the infiltration models are addressed under the
"Infiltration/Ventilation" chapter without sub-numbering like §27.4.3. The
conceptual reference to the ELA model is correct; the section number citation
cannot be confirmed.

### Web-Verified Citations

**Citation 1**: Walker & Wilson (1998), HVAC&R Research 4(2):119–139 —
N-factor climate dependence.

- **Source found**: LBNL-42361.pdf (AIVC mirror); AIVC resource page
  (aivc.org/resource/field-validation-algebraic-equations-stack-and-wind-driven-air-infiltration-calculations);
  LBL Buildings group publication page (buildings.lbl.gov).
- **Quoted passage**: The paper is confirmed to exist and is correctly cited.
  The EnergyPlus documentation confirms: "The Flow Coefficient model is based
  on Walker and Wilson (1998) and the model formulation used in EnergyPlus is
  from the ASHRAE Handbook of Fundamentals where it is referred to as the
  'Enhanced' or 'AIM-2' model." (bigladdersoftware.com/epx/docs/23-2/)
- **N-factor range verification**: The LBL N-factor concept (ACHnat = ACH50 /
  N) is confirmed by multiple independent sources. GreenBuildingAdvisor states:
  "It varies from 9.8 to 29.4" and "It cannot be determined by climate zone
  alone; it also depends on the number of stories in the building under
  consideration." The Building Performance Association Journal confirms: example
  N-factor of 9.8 for a zone-1, exposed, 3-story building. However, the
  specific claim that "N ranges from ~10 in windy southern climates (CZ1) to
  ~22 in sheltered northern climates (CZ7)" uses DOE climate zones (CZ1–CZ7),
  which are distinct from the LBL model's zones (Zone 1–5 based on Heating
  Degree Days). The ticket conflates these zone numbering systems. The
  directional claim (windier/warmer → lower N) is correct; the specific CZ
  labels and the upper bound of 22 are approximations not directly sourced
  from the Walker & Wilson paper.
- **Verdict**: Partially correct. The paper exists and the N-factor
  climate-dependence concept is sound and well-documented. The specific CZ1/CZ7
  labels and the claim that "N ranges from ~10 to ~22" are imprecise (the full
  LBL table range is 9.8–29.4); the directional error claim (67–100%) overstates
  certainty for a single-climate example.

**Citation 2**: ASHRAE Handbook of Fundamentals 2021, Ch. 16 §4.3.

- **Source found**: ASHRAE official ToC page (ashrae.org/technical-resources/
  ashrae-handbook/table-of-contents-2021-ashrae-handbook-fundamentals).
- **Quoted passage**: Chapter 16 is confirmed as "Ventilation and Infiltration"
  (2021 edition). The full chapter content is paywalled; sub-section §4.3
  cannot be independently verified from public sources.
- **Verdict**: Partially correct. Chapter 16 is correctly identified as
  "Ventilation and Infiltration." The sub-section citation "§4.3" cannot be
  confirmed from publicly available sources. The 2017 edition reference in
  `infiltration.rs` line 27 uses "Chapter 16" without sub-section numbering.

**Citation 3**: ASHRAE Standard 152-2004 §6.2 — Unconditioned attached garage
default SLA = 3.0 × 10⁻⁴.

- **Source found**: Multiple web searches for this specific value in ASHRAE 152
  returned no public documentation. ASHRAE Standard 152-2004 is titled "Method
  of Test for Determining the Design and Seasonal Efficiencies of Residential
  Thermal Distribution Systems" — it is primarily a duct efficiency test
  standard, not an infiltration reference standard.
- **Quoted passage**: None found. Searches for "ASHRAE 152" consistently return
  references to duct leakage and distribution efficiency, not zone infiltration
  defaults.
- **Verdict**: Cannot verify. ASHRAE 152 covers duct system efficiency, not
  building envelope air leakage defaults. The SLA=3.0×10⁻⁴ value may be
  correct (it appears in ResStock/OpenStudio-HPXML for attics and similar
  unconditioned spaces), but attributing it specifically to ASHRAE 152-2004
  §6.2 could not be confirmed. The correct source for residential default SLA
  values is more likely ANSI/RESNET/ICC 301 or OpenStudio-HPXML defaults.

**Citation 4**: EnergyPlus Engineering Reference §27.4.3 — ELA model.

- **Source found**: bigladdersoftware.com/epx/docs/23-2/engineering-reference/
  infiltration-ventilation.html (fetched and read).
- **Quoted passage**: "Infiltration = (FSchedule) AL/1000 √(Cs·ΔT + Cw·(WindSpeed)²)"
  where "AL is the effective air leakage area in cm² that corresponds to a 4 Pa
  pressure differential." The document references "ASHRAE Handbook of
  Fundamentals (2001 Chapter 26; 2005 Chapter 27)" as the source. The section
  is titled "Infiltration by Effective Leakage Area" — no sub-section number
  27.4.3 appears anywhere in the online documentation.
- **Verdict**: Partially correct. The ELA model exists in EnergyPlus as
  described, the formula is correctly characterised, and the model IS superior
  to a constant ACH. The section number "§27.4.3" does not exist in the
  EnergyPlus Engineering Reference as a verifiable citation; the online HTML
  docs use prose headings without hierarchical numbering.

### Legitimacy

- **Verdict**: Partially Legitimate

- **Rationale**: The core bug described in the ticket is **real and confirmed**:
  `solver_builder.rs:941–947` uses `ach50 / 20.0` with `unwrap_or(0.5)` for
  garage infiltration, while the conditioned zone uses full AIM-2 physics and
  other unconditioned zones (attic, foundation) use ELA with SLA. The three
  physics errors described (fixed N, wrong ACH50 source, constant-ACH method)
  are all genuine. The proposed fix direction — a `garage_infiltration_method`
  using `InfiltrationMethod::Ela` — is architecturally correct and the physics
  infrastructure (`garage_ela_coefficients` in `infiltration.rs:655–663`) is
  already implemented. However, three secondary claims need refinement:
  (1) The OCHRE cross-reference is factually wrong — OCHRE applies raw ACH50
  (not `ach50 / n_factor`) to the garage, which is worse than HARES's current
  code, not better; (2) the specific ASHRAE 152-2004 §6.2 citation for
  SLA=3.0×10⁻⁴ cannot be verified — ASHRAE 152 is a duct efficiency standard;
  (3) the EnergyPlus §27.4.3 section number does not exist in the public
  documentation.

### Proposed Fix Summary

1. Add `garage_infiltration_method(building, zone, building_height_m) ->
   Result<InfiltrationMethod>` in `solver_builder.rs`, analogous to
   `attic_infiltration_method`.
2. If zone-level `ventilation_sla` or `ventilation_ach` data is present, use
   it directly (same pattern as `foundation_infiltration_method`).
3. If no zone-level data is present, apply a documented default SLA (verify
   the correct source — likely ANSI/RESNET/ICC 301 or OpenStudio-HPXML
   defaults rather than ASHRAE 152) and emit `tracing::warn!`.
4. Use `garage_ela_coefficients` (already in `infiltration.rs:655`) to get
   the stack/wind coefficients; compute `ela_m2 = sla * garage_floor_area_m2`.
5. Return `InfiltrationMethod::Ela { ela_m2, stack_coeff, wind_coeff }`.
6. Remove `ach50 / 20.0` and `unwrap_or(0.5)` entirely.
7. DO NOT implement fix: only test and audit work was done here.

### Test Written

- **File**: `crates/hares-physics/tests/physics_validation_tests.rs`
- **Tests**:
  1. `ticket_053_aim2_flow_varies_with_climate_unlike_fixed_n20` — verifies
     that AIM-2 produces meaningfully different flows under cold/calm vs
     warm/windy conditions (ratio differs from 1.0 by >30%), demonstrating
     that a constant `ach50/N` model loses time-varying climate information.
     Both tests pass (physics layer is correct; the bug is in solver_builder).
  2. `ticket_053_ela_model_varies_with_conditions_unlike_constant_ach` —
     verifies that `garage_ela_coefficients` returns positive non-degenerate
     coefficients, that SLA=3.0e-4 × garage_floor_area produces non-zero ELA
     (the required fallback), and that the resulting ELA flow varies with both
     wind speed and ΔT unlike a constant-ACH model.
- Both tests **pass** — they exercise the correct physics infrastructure, not
  the broken `solver_builder.rs` path. The integration-level regression test
  (verifying the solver_builder path itself) should go in
  `crates/hares-core/tests/` once the fix is implemented.
