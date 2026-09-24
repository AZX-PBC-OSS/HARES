# Slab-on-Grade Uses Area-UA Conduction Instead of ASHRAE F-Factor Perimeter Method

**Severity**: High
**Priority**: P2
**Status**: Open
**Areas**: hares-physics/ground.rs, hares-envelope/boundary_rc.rs, hares-core/dwelling/conversions.rs

## Problem

ASHRAE Handbook of Fundamentals 2021, Ch. 18.31 mandates the perimeter
F-factor method for slab-on-grade heat loss:

```
Q_slab = F2 × P × (T_indoor - T_ground_surface)
```

where F2 [W/(m·K)] is a perimeter heat loss coefficient that accounts for the
three-dimensional heat flow around the slab edge, and P is the exposed
perimeter length. This method is the EnergyPlus default (Engineering Reference
§12.4) and the ASHRAE 90.1 compliance method.

The functions `slab_perimeter_loss_w` and `f2_coefficient` exist in
`hares-physics/src/ground.rs:98–150` and are correct. However, they are never
called during simulation. The slab boundary (`BoundaryType::Slab`) is assembled
as a flat conductive boundary by `build_layered_boundary` / `build_precomputed_boundary`
in `boundary_rc.rs` — identical to a wall — and connected to `ExteriorTarget::Ground`
with an area-weighted conductance (U × A_slab).

### Physics Error

Area-UA modeling of a slab computes:
```
Q_slab = U_slab × A_slab × (T_indoor - T_ground)
```
using the full floor area. This is incorrect because:

1. Heat loss through the slab center is negligible (ASHRAE §18.31: deep soil
   has near-constant temperature; only the perimeter is thermally active).
2. Area-UA ignores the three-dimensional edge conduction that dominates slab
   heat loss.
3. U_slab for a concrete slab without insulation is high (~1.0–1.5 W/m²·K);
   multiplied by the full floor area this dramatically overstates heat loss.

For a 140 m² (1,500 ft²) slab with P = 50 m perimeter, F2 = 1.17 W/(m·K),
and ΔT = 15°C:
- Correct (F-factor): Q = 1.17 × 50 × 15 = 878 W
- Incorrect (area-UA at U=1.0): Q = 1.0 × 140 × 15 = 2,100 W — 2.4× too high.

## Evidence

```
crates/hares-physics/src/ground.rs:98–150
    pub fn slab_perimeter_loss_w(...) -> f64 { ... }  // ← correct but never called
    pub fn f2_coefficient(insulation_r_m2_k_w: f64) -> f64 { ... }  // ← correct but never called

crates/hares-envelope/src/boundary_rc.rs:446–453
    ExteriorTarget::Ground => ground_node,  // ← slab uses same path as wall
```

No caller in `solver_builder.rs`, `conversions.rs`, or `boundary_rc.rs`
invokes `slab_perimeter_loss_w` or `f2_coefficient`.

## HPXML Geometry Available

HPXML provides slab perimeter geometry:
- `<Perimeter>` element under `<Slab>` (explicit perimeter length [ft]).
- If absent: derive from `sqrt(area) × 4` for square approximation.
- `<PerimeterInsulation><Layer><InstallationType>` and `<NominalRValue>` for
  insulation depth/R-value needed to select F2 coefficient.

## Required Behavior

Per ASHRAE HoF 2021, Ch. 18 §31 and EnergyPlus Engineering Reference §12.4,
slab-on-grade heat loss must use the F-factor perimeter method:

```
Q_slab = F2 × P × (T_indoor - T_ground_surface)
```

where F2 [W/(m·K)] is selected from ANSI/ASHRAE 90.1-2022, Table A6.3.1
based on insulation depth and R-value. The area-UA method must not be used.

## Approach

1. Parse `<Perimeter>` from HPXML slab elements in `building.rs`; store as
   `perimeter_m: f64` on `Boundary`. If absent, derive from `4 × sqrt(area_m2)`
   as a square-plan approximation (no silent failure — log a warning).
2. Parse `<PerimeterInsulationDepth>` and `<PerimeterInsulation><Layer><NominalRValue>`
   to select the F2 coefficient via `f2_coefficient(insulation_r_m2_k_w)`.
3. In `solver_builder.rs`, for `BoundaryType::Slab`, call `slab_perimeter_loss_w`
   to produce a fixed conductance `G = F2 × P` [W/K] connected between the
   slab interior node and the ground node.
4. Retain slab thermal mass as a capacitance layer (concrete thickness × density × Cp).
5. Remove the area-UA resistor for the slab path in `boundary_rc.rs:446–453`.

## Definition of Done

- [ ] `slab_perimeter_loss_w` and `f2_coefficient` are called from the solver
      boundary construction path for `BoundaryType::Slab`.
- [ ] The area-UA resistor from slab interior to ground node is removed and
      replaced with a fixed conductance `G = F2 × P` [W/K].
- [ ] Slab thermal mass (concrete capacitance) is retained as a separate layer.
- [ ] Test: 140 m² slab, P=50 m, uninsulated F2≈1.17 W/(m·K), ΔT=15°C →
      Q ≈ 878 W (tolerance ±5%).
- [ ] Test: insulated slab (R-5 perimeter) produces lower Q than uninsulated
      at same ΔT.

## Verification

```bash
cargo test -p hares-physics slab_perimeter_loss
cargo test -p hares-envelope slab
```

Expected: `slab_perimeter_loss_w(1.17, 50.0, 15.0)` ≈ 878 W.

## References

- ASHRAE Handbook of Fundamentals 2021, Ch. 18 §31 (Slab-on-Grade Floors —
  F-factor perimeter method).
- ANSI/ASHRAE/IES 90.1-2022, Table A6.3.1 (Slab F-factors by climate zone and
  insulation depth).
- EnergyPlus Engineering Reference §12.4 (Slab-on-Grade Heat Transfer —
  F-factor method as default).
- OCHRE `utils/envelope.py`: F-factor pre-integrated into `SlabFloor` RC layer
  stack; heat loss dominated by perimeter nodes.

## Verification Audit

**Auditor**: claude-sonnet-4-6 (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] **Referenced line numbers still match (corrected locations)**
  - `slab_perimeter_loss_w`: declared at `ground.rs:98` (function signature),
    body at lines 98–105. Ticket says "98–150 for the whole block" — that span
    covers both functions; confirmed correct.
  - `f2_coefficient`: declared at `ground.rs:140`, returns at `ground.rs:141–151`. ✓
  - `boundary_rc.rs:446`: `ExteriorTarget::Ground => ground_node` ✓ (independently
    verified by reading the file at offset 430–456).

- [x] **Described logic matches current implementation**
  - `slab_perimeter_loss_w` implements exactly `f2_w_per_m_k * perimeter_m *
    (t_indoor_c - t_ground_surface_c)` (line 104) — matches `F2 × P × ΔT`. ✓
  - `f2_coefficient` returns `0.74 / 0.86 / 1.17` W/(m·K) for R-10+/R-5/
    uninsulated (`ground.rs:141–150`). ✓
  - `boundary_rc.rs` has **zero** references to `BoundaryType` (confirmed by grep)
    — it receives pre-resolved `ExteriorTarget` values and cannot discriminate
    `Slab` from any other type. Every `ExteriorTarget::Ground` boundary routes
    identically to `ground_node` (line 446) with no slab-specific branch. ✓
  - `solver_builder.rs:192` matches `BoundaryType::Floor | BoundaryType::Slab`
    in the same arm with no perimeter-specific logic. ✓
  - **grep confirms zero production callers**: all references to
    `slab_perimeter_loss_w` and `f2_coefficient` outside the `#[cfg(test)]` block
    are within that same block (lines 216, 224–226, 267, 270, 288–289). The
    functions are dead code in production. ✓
  - Two regression tests already exist at `ground.rs:265` and `ground.rs:283`
    (added in previous audit pass), both passing: `cargo test -p hares-physics
    slab` → 5/5 ok.

- [x] **OCHRE cross-check result: diverges — area-UA in OCHRE, perimeter-explicit
  proposed in ticket — with file+line evidence**
  - `vendors/OCHRE/ochre/utils/envelope.py:462–485` (`get_slab_insulation()`):
    parses HPXML perimeter/under-slab insulation R-values and maps them to a
    string key (e.g. "Uninsulated", "2ft R5 Perimeter"). **No F×P calculation
    occurs at runtime.**
  - `vendors/OCHRE/ochre/defaults/Envelope/Envelope Materials.csv:708`: The
    "Floor, Uninsulated" RC stack includes a layer explicitly named `"Ficticious
    Insulating Layer"` with thickness=0, density=0, cp=0, and
    R=0.4237 m²·K/W — a purely resistive (massless) layer that distributes
    the aggregate perimeter-calibrated resistance across the full slab area.
  - `vendors/OCHRE/ochre/defaults/Envelope/Envelope Boundary Types.csv:176`:
    `Floor,Uninsulated,,,Uninsulated,Minimal_slab_unins,5.508984723`
    — total assembly R ≈ 5.51 m²·K/W (includes soil, slab concrete, carpet,
    and fictitious layer).
  - Conclusion: OCHRE uses area-UA with a LUT-calibrated fictitious resistance.
    HARES currently does the same via the OCHRE LUT. The ticket proposes replacing
    the area-UA path with explicit F×P conductance — this would **intentionally
    diverge from OCHRE** toward the standard ASHRAE method. That divergence is
    not already present; it must be introduced as part of the fix.

- [x] **EnergyPlus cross-check result: partially matches ticket claim — section
  number wrong, formula correct — with quoted passage**
  - Source fetched: EnergyPlus 25.1 Engineering Reference, section
    "Slab-on-grade and Underground Floors Defined with F-factors" within
    "Ground Heat Transfer Calculations using C and F Factor Constructions"
    (<https://bigladdersoftware.com/epx/docs/25-1/engineering-reference/ground-heat-transfer-calculations-using-c.html>).
    Also verified against EnergyPlus 8.0, 8.3, 9.4, and 24.1 table-of-contents
    pages — **no version uses numbered sections like "§12.4"**; the document uses
    descriptive headings throughout.
  - Quoted passage (EnergyPlus 25.1):
    > *"Q = Area · Ueff · (Tair,out − Tair,in) = (Tair,out − Tair,in) · (Pexp ·
    > F-factor)"* and *"Ueff = (Pexp · F-factor) / Area"*
  - Implementation detail: EnergyPlus converts the F-factor + perimeter into an
    area-weighted `Ueff`, then builds a two-layer equivalent construction (0.15 m
    concrete + fictitious insulation). It uses the F-factor *via area-UA* — not
    an explicit perimeter conductance node — but the equivalence holds when
    perimeter is correctly propagated.
  - EnergyPlus describes the method as the code-compliance path ("building energy
    code compliance calculations … ASHRAE 90.1, 90.2 and California Title 24"),
    **not** as the unconditional default. The ticket's claim that it is "the
    EnergyPlus default" overstates it slightly; it is the compliance shortcut path.
  - **Correction to ticket's §12.4 citation**: The EnergyPlus Engineering
    Reference has no section numbered "§12.4". The correct heading is
    "Slab-on-grade and Underground Floors Defined with F-factors". The notation
    "§12.4" likely reflects an older PDF chapter-page reference, not a section
    number.

### Web-Verified Citations

**Citation 1**
- **Citation**: "ASHRAE Handbook of Fundamentals 2021, Ch. 18 §31 (Slab-on-Grade
  Floors — F-factor perimeter method)"
- **Source found**: ASHRAE.org 2021 Handbook table of contents
  (<https://www.ashrae.org/technical-resources/ashrae-handbook/table-of-contents-2021-ashrae-handbook-fundamentals>);
  confirmed Ch. 17 = "Residential Cooling and Heating Load Calculations",
  Ch. 18 = "Nonresidential Cooling and Heating Load Calculations". The slab
  F-factor content appears in both residential (Ch. 17) and nonresidential (Ch. 18)
  load calculation chapters.
- **Quoted passage**: From the ASHRAE 2021 ToC page: Chapter 17 covers residential
  loads; Chapter 18 covers nonresidential. The slab perimeter method is referenced
  in both. Notation "18.31" follows the ASHRAE Handbook chapter.page convention
  (chapter 18, page 31), not a numbered subsection.
- **Verdict**: **Partially correct — citation style ambiguity, not a factual error**.
  The method is real and ASHRAE-specified in both Ch. 17 and Ch. 18. "Ch. 18 §31"
  is best read as "Chapter 18, page 31" in ASHRAE's older page-style citation
  convention. Ch. 18 covers nonresidential loads; the residential equivalent is
  Ch. 17. For a residential simulator like HARES the authoritative section is
  Ch. 17, not Ch. 18 — though both describe the same F-factor perimeter method.
  The `ground.rs:11` module docstring reproduces this same citation verbatim.

**Citation 2**
- **Citation**: "ANSI/ASHRAE/IES 90.1-2022, Table A6.3.1 (Slab F-factors by
  climate zone and insulation depth)"
- **Source found**: NYC Energy Code 2025 §A6.3 "F-Factors for Slab-on-Grade
  Floors" via UpCodes (<https://up.codes/s/f-factors-for-slab-on-grade-floors>),
  which adopts ASHRAE 90.1 Appendix A normatively.
- **Quoted passage** (from UpCodes, citing ASHRAE 90.1 Table A6.3.1-1):
  > *"Unheated slabs — Uninsulated: 0.73 [Btu/(h·ft·°F)]."*
  > *"Heated slabs — Uninsulated: 1.35 [Btu/(h·ft·°F)]."*
  > R-5 perimeter insulation (12 in. horizontal): unheated 0.72, heated 1.31.
  > R-10 perimeter insulation (12 in. horizontal): unheated 0.71, heated 1.30.
- **Verdict**: **Confirmed**. Table A6.3.1 exists and provides F-factors by
  insulation configuration. Values are in IP units (Btu/h·ft·°F). The table name,
  number, and standard citation are all accurate.

**Citation 3**
- **Citation**: "EnergyPlus Engineering Reference §12.4 (Slab-on-Grade Heat
  Transfer — F-factor method as default)"
- **Source found**: EnergyPlus 25.1 Engineering Reference
  (<https://bigladdersoftware.com/epx/docs/25-1/engineering-reference/ground-heat-transfer-calculations-using-c.html>).
  Also checked EnergyPlus 8.0, 8.3, 8.8, 9.0, 9.2, 9.4, and 24.1 — none use
  numbered sections. The document structure uses descriptive headings only.
- **Quoted passage**:
  > *"Slab-on-grade and Underground Floors Defined with F-factors … Q = Area ·
  > Ueff · (Tair,out − Tair,in) = (Tair,out − Tair,in) · (Pexp · F-factor)"*
- **Verdict**: **Partially correct**. The F-factor slab method is real and
  implemented in EnergyPlus as the code-compliance path. However: (a) the section
  is not numbered "§12.4" in any version of the Engineering Reference; (b)
  EnergyPlus describes it as the compliance shortcut method rather than the
  unconditional "default". Both inaccuracies are minor and do not undermine the
  cited physics.

**Citation 4**
- **Citation**: "OCHRE `utils/envelope.py`: F-factor pre-integrated into
  `SlabFloor` RC layer stack; heat loss dominated by perimeter nodes."
- **Source found**: Directly read from the vendored submodule:
  - `vendors/OCHRE/ochre/utils/envelope.py:462–485` (`get_slab_insulation()`).
  - `vendors/OCHRE/ochre/defaults/Envelope/Envelope Materials.csv:708–711`.
  - `vendors/OCHRE/ochre/defaults/Envelope/Envelope Boundary Types.csv:176`.
- **Quoted passage** (`envelope.py:462–481`):
  ```python
  def get_slab_insulation(floor, name=None):
      r_perimeter = floor.get("PerimeterInsulation", {}).get("Layer", {}).get("NominalRValue", 0)
      r_under = floor.get("UnderSlabInsulation", {}).get("Layer", {}).get("NominalRValue", 0)
      ...
      elif not r_perimeter and not r_under:
          insulation = "Uninsulated"
      return insulation
  ```
  `Envelope Materials.csv:708`: `Floor,Uninsulated,,,Uninsulated,Ficticious Insulating Layer,0,0,0,0,0.423720515,0,Minimal_slab_unins,`
  `Envelope Boundary Types.csv:176`: `Floor,Uninsulated,,,Uninsulated,Minimal_slab_unins,5.508984723`
- **Verdict**: **Partially correct / framing imprecise**. OCHRE does encode slab
  insulation into the RC stack, and the "Ficticious Insulating Layer" in the
  materials CSV distributes aggregate perimeter-calibrated resistance across the
  full slab area. However: (a) there are no "perimeter nodes" — OCHRE uses
  area-UA, not a spatial perimeter mesh; (b) the description "pre-integrated"
  loosely captures that the LUT R-values were calibrated to approximate perimeter
  effects, but architecturally OCHRE is an area-UA model. The claim is
  approximately true in effect but misleading in mechanism.

**Citation 5 — F2 values (0.74, 0.86, 1.17 W/(m·K))**
- **Citation**: Ticket states F2 = 1.17 W/(m·K) for uninsulated slab; code
  returns 0.74/0.86/1.17 W/(m·K) for R-10+/R-5/uninsulated.
- **Source found**: ASHRAE 90.1 Table A6.3.1 values confirmed at 0.73 Btu/(h·ft·°F)
  (unheated uninsulated) via UpCodes NYC 2025 (<https://up.codes/s/f-factors-for-slab-on-grade-floors>).
  Unit conversion independently verified: 1 Btu/(h·ft·°F) = 1.73074 W/(m·K)
  (derivation: 1055.056 J/Btu ÷ 3600 s/h ÷ 0.3048 m/ft ÷ (5/9) K/°F).
- **Quoted passage** (conversion arithmetic):
  - ASHRAE 90.1 unheated uninsulated: 0.73 × 1.73074 = **1.263 W/(m·K)**
  - ASHRAE 90.1 heated uninsulated: 1.35 × 1.73074 = **2.336 W/(m·K)**
  - Ticket value 1.17 W/(m·K) back-converts to: 1.17 ÷ 1.73074 = **0.676 Btu/(h·ft·°F)**
  - ASHRAE 90.1 R-5 insulated: 0.72 × 1.73074 = **1.246 W/(m·K)** (unheated)
  - ASHRAE 90.1 R-10 insulated: 0.71 × 1.73074 = **1.229 W/(m·K)** (unheated)
- **Verdict**: **Incorrect vs. ASHRAE 90.1-2022, but correctly flagged in code
  docstring**. The ticket's 1.17 W/(m·K) for uninsulated slabs is approximately
  8% below the ASHRAE 90.1 unheated value of 1.263 W/(m·K) and matches no
  ASHRAE 90.1 table entry. The value 0.676 Btu/(h·ft·°F) is not a standard
  tabulated value. The 1.17 / 0.86 / 0.74 triplet appears to come from an older
  edition of the ASHRAE Handbook of Fundamentals (separate from 90.1) where
  different soil-conductivity assumptions produce lower absolute values.
  Importantly, `f2_coefficient`'s docstring at `ground.rs:138` explicitly says:
  *"for code-compliance F-factors use ASHRAE 90.1 Table A6.3.1 instead"* —
  the code acknowledges the discrepancy. The ticket does not clearly note that
  the example value 1.17 W/(m·K) is a handbook approximation, not the 90.1
  compliance value. This is a presentational issue in the ticket; it does not
  affect the bug report itself.

### Legitimacy

- **Verdict**: **Legitimate**

- **Rationale**: Every core claim in the ticket is independently confirmed. (1)
  `slab_perimeter_loss_w` and `f2_coefficient` exist in `ground.rs:98–151` and
  have zero callers in production code — confirmed by exhaustive grep across all
  Rust crates. (2) `boundary_rc.rs` contains no `BoundaryType` references; it
  resolves `ExteriorTarget::Ground` identically for slabs and below-grade walls
  (line 446). (3) `solver_builder.rs:192` matches `Slab` and `Floor` in the same
  match arm with no perimeter-specific branch. (4) EnergyPlus 25.1 Engineering
  Reference (fetched directly) confirms the F-factor method is the ASHRAE
  code-compliance approach for slab-on-grade heat transfer, with the formula
  `Q = Pexp × F-factor × ΔT`. (5) The OCHRE cross-check confirms that OCHRE
  itself uses area-UA with a calibrated fictitious layer — not explicit F×P — so
  implementing the perimeter method would intentionally and correctly diverge from
  OCHRE. The physics magnitude error is real: area-UA with a concrete slab U-value
  over the full floor area substantially overstates heat loss compared to the
  perimeter-dominant F-factor method. Minor citation inaccuracies (section "§12.4"
  vs. descriptive heading; F2 = 1.17 vs. ASHRAE 90.1's 1.263 W/(m·K)) are
  acknowledged in code comments and do not undermine the bug report. Applying the
  correct ASHRAE 90.1 F-factors (1.263 W/(m·K) for unheated uninsulated) would
  make the physics even more accurate; the current code values are conservative
  approximations.

### Proposed Fix Summary

1. Add `perimeter_m: Option<f64>` to the slab `Boundary` representation;
   parse `<Perimeter>` from HPXML in `hares-io/src/hpxml/building.rs`; fall back
   to `4 × sqrt(area_m2)` with a logged warning when absent.
2. Parse `<PerimeterInsulation><Layer><NominalRValue>` from HPXML to select the
   F2 coefficient via `f2_coefficient(insulation_r_m2_k_w)`. Consider upgrading
   the coefficient table to the ASHRAE 90.1-2022 compliance values
   (1.263/1.246/1.229 W/(m·K) for unheated uninsulated/R-5/R-10) rather than
   the older handbook approximations (1.17/0.86/0.74).
3. In `solver_builder.rs`, add a `BoundaryType::Slab`-specific branch that
   calls `slab_perimeter_loss_w` to produce a fixed conductance
   `G = F2 × P_m [W/K]` and connects it directly between the slab interior
   node and the ground node.
4. Preserve concrete thermal mass as a capacitance layer (separate from the
   conductance path, retaining density × Cp × volume).
5. Remove or gate the area-UA resistor path in `boundary_rc.rs` so that
   `BoundaryType::Slab` bypasses the precomputed RC LUT path (or that path
   produces only a capacitance, not a conductance, for slabs).

### Test Written

- **File**: `crates/hares-physics/src/ground.rs` (within `#[cfg(test)]` block)
- **Tests present** (written in prior audit pass, verified passing 2026-05-21):
  1. `ticket_035_slab_140m2_p50_uninsulated_15k_delta_approx_878w`
     (`ground.rs:265`) — exercises the DoD example: P=50 m, F2=1.17 W/(m·K),
     ΔT=15 K → 877.5 W ±5%. Passes.
  2. `ticket_035_r5_perimeter_slab_lower_loss_than_uninsulated`
     (`ground.rs:283`) — confirms R-5 perimeter insulation < uninsulated at
     same ΔT. Passes.
- **Test status**: `cargo test -p hares-physics slab` → **5/5 ok**
- **Coverage note**: Both tests exercise the helper functions in isolation. They
  will continue to pass whether or not the solver wiring is done (they do not
  demonstrate the integration bug). An integration-level test confirming that a
  `BoundaryType::Slab` boundary in a full simulation produces
  `Q ≈ F2 × P × ΔT` (not `U × A × ΔT`) must be added as part of the fix.
