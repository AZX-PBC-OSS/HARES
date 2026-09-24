# Consolidate `h_rad = 4εσT³` Linearisation Into Shared Helper

**Severity**: Low
**Priority**: P3
**Status**: Open
**Areas**: hares-physics, hares-envelope

## Problem

The linearised radiation conductance `h_rad = 4·ε·σ·T³` (Stefan-Boltzmann linearisation) is computed inline at three locations:

- `crates/hares-physics/src/film_coefficients.rs:186-189`
- `crates/hares-envelope/src/boundary_rc.rs:696-697`
- `crates/hares-envelope/src/rc_network.rs:871-872`

DRY violation. A future change to ε, σ, or the linearisation pivot temperature must touch three files. The constant 4 (the derivative coefficient) and the σ value (Stefan-Boltzmann constant) are at risk of subtle drift.

## Current Behavior

Three independent inline computations of `4 * emissivity * STEFAN_BOLTZMANN * t.powi(3)`. Each site uses its own σ symbol (one uses `STEFAN_BOLTZMANN_W_M2_K4`, another uses `SIGMA`, the third may use a literal). Verifying agreement requires reading three files.

## Required Behavior

Add a single shared helper in `hares-physics`:

```rust
/// Linearised radiation conductance `h_rad = 4·ε·σ·T³` (Stefan-Boltzmann derivative).
///
/// `t_kelvin` is the linearisation pivot temperature in K (typically the mean of the
/// two surface temperatures involved in the radiation exchange).
#[inline]
pub fn linearised_h_rad(emissivity: f64, t_kelvin: f64) -> f64 {
    4.0 * emissivity * STEFAN_BOLTZMANN_W_M2_K4 * t_kelvin.powi(3)
}
```

All three callsites consume this helper. The σ constant has a single canonical definition in `hares-physics/src/constants.rs` (verify the existing constant matches NIST CODATA 2018: σ = 5.670374419e-8 W·m⁻²·K⁻⁴).

## Approach

1. Add `linearised_h_rad` to `crates/hares-physics/src/film_coefficients.rs` (or a new `radiation.rs` if more appropriate).
2. Replace the inline computations at:
   - `crates/hares-physics/src/film_coefficients.rs:186-189`
   - `crates/hares-envelope/src/boundary_rc.rs:696-697`
   - `crates/hares-envelope/src/rc_network.rs:871-872`
3. Verify the σ constant matches NIST CODATA 2018; update if necessary.
4. Add a unit test for `linearised_h_rad` at a representative T (e.g. T=295 K, ε=0.9): expected ~5.79 W/(m²·K).
5. Verify no behavioural change via existing `cargo test -p hares-envelope` and `cargo test -p hares-physics`.

## Definition of Done

- [ ] `linearised_h_rad` helper exists in `hares-physics`
- [ ] Three callsites consume the helper; no inline `4 * .. * sigma * t.powi(3)` pattern remains
- [ ] σ constant verified against NIST CODATA 2018
- [ ] Unit test for `linearised_h_rad` at T=295 K, ε=0.9 passes
- [ ] All existing tests pass with no behavioural change

## Verification

```bash
cargo test -p hares-physics
cargo test -p hares-envelope
rg "4\.0 \* .* STEFAN" crates/   # should return zero hits after fix
```

## References

- NIST CODATA 2018: Stefan-Boltzmann constant σ = 5.670374419 × 10⁻⁸ W·m⁻²·K⁻⁴.
- Incropera, DeWitt, Bergman, Lavine *Fundamentals of Heat and Mass Transfer* 7th ed. §1.2.3 — Stefan-Boltzmann law and linearisation about a pivot temperature.
- ASHRAE Handbook of Fundamentals 2021 Ch. 4 §4.3 "Radiation Heat Transfer" — linearised h_rad formulation.

## Related Tickets

- 089-radiation-frac-starmesh-rederivation
- 044-lwr-fallback-linearised-not-scriptf
- 101-interior-film-coefficients-recompute-per-step

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] **Referenced line numbers are INCORRECT** — all three cited locations are wrong:
  - `film_coefficients.rs:186-189` — line 187 is the `film_resistances` function signature, not an h_rad computation. The actual inline formula occurrences in this file are at **lines 289 and 333**, both inside `#[cfg(test)]` blocks (test functions `film_resistances_typical_wall_outdoor` and `interior_film_resistance_is_convection_only`).
  - `boundary_rc.rs:696-697` — line 696 is a `const T_REF_K` definition. The actual inline formula occurrences are at **line 705** (window glass decomposition, production code) and **line 920** (star-mesh conductance, production code).
  - `rc_network.rs:871-872` — line 870-871 is a test function body. The actual occurrence is at **line 870** inside `fn linearized_h_rad_sensitivity_at_reference_temperature()`, which is `#[cfg(test)]`.
- [x] **Described logic matches current implementation** — the formula `4.0 * ε * σ * T.powi(3)` is indeed present at the above corrected locations, and σ symbol names differ across callsites:
  - `film_coefficients.rs:289,333`: uses `crate::constants::STEFAN_BOLTZMANN`
  - `boundary_rc.rs:705,920`: uses local `const SIGMA: f64 = crate::longwave_radiation::STEFAN_BOLTZMANN`
  - `boundary_rc.rs:2765`: uses `STEFAN_BOLTZMANN` (direct import)
  - `rc_network.rs:693-695,870`: uses local `const SIGMA: f64 = 5.670374e-8` (hardcoded)
  - `interior_lwr.rs:432`: uses local `const SIGMA` (also hardcoded)
- [x] **A helper already exists** — `linearised_h_r(emissivity: f64, t_avg_c: f64) -> f64` is defined at `crates/hares-envelope/src/longwave_radiation.rs:239-240` and exported from `hares-envelope`. The ticket does not mention this existing helper. The proposed helper `linearised_h_rad` in `hares-physics` would duplicate it with a different argument convention (Kelvin vs Celsius) and a different home crate.
- [x] **OCHRE cross-check**: OCHRE implements the same linearisation at `vendors/OCHRE/ochre/Models/Envelope.py`:
  - Line 238: `self.e_factor = self.emissivity * 5.6704e-8 * self.area` (σ = 5.6704e-8)
  - Line 1055: `res_name: 1 / (4 * surface.e_factor * t_ref**3)` where `t_ref = 20 + 273.15 = 293.15 K`
  - OCHRE uses σ = 5.6704e-8 (6-significant-figure truncation). HARES `STEFAN_BOLTZMANN` is 5.670374419e-8 (NIST CODATA 2018). The relative difference is 0.0005%, negligible for building simulation but a real DRY violation exists: `rc_network.rs` hardcodes `5.670374e-8` (7 digits), while `longwave_radiation.rs` uses the constant from `hares-physics`. All values agree to within 1 ppm.
- [x] **EnergyPlus cross-check**: EnergyPlus Engineering Reference (bigladdersoftware.com, versions 8.0–9.6) defines `hr` as the linearised radiative heat transfer coefficient with units W/(m²·K), and uses the form `hr = εσ(T⁴_surf − T⁴_x)/(T_surf − T_x)` for exterior surfaces — not the Taylor-expanded `4εσT³` directly. For interior surfaces (Inside Heat Balance), EnergyPlus uses the non-linearised ScriptF / CarrollMRT methods. The `4εσT³` form is the first-order Taylor expansion of the general form about the operating point T, valid when ΔT ≪ T. HARES's use of the Taylor form for the star-mesh conductance is consistent with OCHRE's approach and with the `mechanics-and-machines.com` reference which states: "the effective heat transfer coefficient is h_rad = 4σεT_char³ where T_char is a characteristic temperature."

### Web-Verified Citations

**Citation 1**: NIST CODATA 2018: σ = 5.670374419 × 10⁻⁸ W·m⁻²·K⁻⁴

- **Source found**: NIST CODATA Fundamental Constants database (physics.nist.gov/cgi-bin/cuu/Value?sigma), confirmed via multiple secondary sources including Wikipedia (Stefan–Boltzmann constant article), and a CODATA search result stating "The recommended numerical value of the Stefan-Boltzmann constant based on the 2018 CODATA adjustment is 5.670374419 × 10⁻⁸ W m⁻² K⁻⁴, with a relative standard uncertainty of 1.5 × 10⁻⁶. The more extended value is (5.67037441918442...)×10⁻⁸ W/m²/K⁴."
- **Quoted passage**: "σ = 5.670374419...×10⁻⁸ W⋅m⁻²⋅K⁻⁴. As of the 2019 revision of the SI, which establishes exact fixed values for k, h, and c, the Stefan–Boltzmann constant is exactly this value." — Wikipedia, Stefan–Boltzmann constant (citing NIST CODATA 2018/2019 SI).
- **Verdict**: **Confirmed**. The value in `hares-physics/src/constants.rs:149` (`5.670_374_419e-8`) matches NIST CODATA 2018 exactly.

---

**Citation 2**: Incropera, DeWitt, Bergman, Lavine *Fundamentals of Heat and Mass Transfer* 7th ed. §1.2.3 — Stefan-Boltzmann law and linearisation about a pivot temperature

- **Source found**: Multiple secondary sources confirming the textbook's Equations 1.8 and 1.9. From Vaia.com (worked solution for Bergman/Incropera 7th ed. Ch. 1 Problem 33) and Chegg homework help referencing "Eq. 1.9" and a web article (numberanalytics.com) citing the general form.
- **Quoted passage**: Equation 1.9 in the 7th edition defines `hr ≡ εσ(Ts + Tsur)(Ts² + Tsur²)`. When `Ts ≈ Tsur ≡ T̄`, this simplifies to `h_r,a = 4εσT̄³` where `T̄ = (Ts + Tsur)/2`. From the Vaia solution: "When surface temperature approximately equals surroundings temperature, the formula simplifies to h_{r,a} = 4εσT̄³."
- **Verdict**: **Confirmed** with one caveat. The citation is accurate for the general concept, and the formula `4εσT³` is the recognised approximation when `Ts ≈ Tsur`. However, §1.2.3 in the 7th edition is titled "Thermal Radiation" and introduces the Stefan-Boltzmann *law* (not the linearisation per se) — the linearised coefficient `hr` and Eq. 1.9 are introduced in §1.2.3 of the 7th edition. The ticket's description of this section as covering "linearisation about a pivot temperature" is consistent with the content.

---

**Citation 3**: ASHRAE Handbook of Fundamentals 2021 Ch. 4 §4.3 "Radiation Heat Transfer" — linearised h_rad formulation

- **Source found**: ASHRAE Handbook of Fundamentals 2021 Chapter 4 table of contents confirmed at handbook.ashrae.org/Handbooks/F21/IP/F21_Ch04/F21_Ch04_ip.aspx. The chapter covers heat transfer including radiation.
- **Quoted passage**: From the ASHRAE HoF 2021 Ch. 4 page: "In cases like this [combined convection + radiation], it is often useful to express net radiation as q_net = h_r A_s (t_s − t_surr), where [equation image] is often called a **radiation heat transfer coefficient**. The disadvantage of this form is that hr depends on ts, which is often the desired result of the calculation." The specific `4εσT³` form appears in the thermal comfort chapter (Ch. 9) as `hr = 4εσ(Ar/AD)[(tcl + t̄r)/2 + 273.2]³` — confirming that ASHRAE does use this linearised form in the HoF. The section number §4.3 could not be independently confirmed as the exact section that presents the linearised h_rad formula (the chapter may present it without that specific subsection label).
- **Verdict**: **Partially confirmed**. The ASHRAE HoF 2021 Ch. 4 does cover the linearised radiation coefficient concept. The ticket's claim that §4.3 specifically presents the linearised h_rad formulation is plausible but could not be confirmed precisely from freely accessible content (the full chapter text requires a subscription). The `4εσT³` form is definitively present elsewhere in the HoF (thermal comfort chapter, confirmed via ASHRAE sources).

### Numerical Value Error in Ticket

The ticket states: "Add a unit test for `linearised_h_rad` at a representative T (e.g. T=295 K, ε=0.9): expected ~5.79 W/(m²·K)."

**This value is incorrect.** The correct value is:

`h_rad = 4 × 0.9 × 5.670374419×10⁻⁸ × 295³ = 5.241 W/(m²·K)`

The value ~5.79 corresponds to either:
- ε = 1.0, T = 295 K → 5.823 W/(m²·K), or
- ε = 0.9, T ≈ 305 K → 5.792 W/(m²·K)

The correct canonical test point is T = 293.15 K (20°C), ε = 0.9 → **5.143 W/(m²·K)**.

### Legitimacy

- **Verdict**: **Partially Legitimate**

- **Rationale**: The core DRY concern is real and present in the codebase: the formula `4.0 * ε * σ * T.powi(3)` appears at multiple locations in `boundary_rc.rs` (production code at lines 705 and 920) and in test code in `rc_network.rs` (lines 693-695, 870) and `film_coefficients.rs` (lines 289, 333). The σ constant is expressed three different ways (`STEFAN_BOLTZMANN`, a re-exported alias, and a hardcoded literal `5.670374e-8`), confirming the drift risk the ticket identifies. However, the ticket has five material errors: (1) all three cited line numbers are wrong; (2) a helper `linearised_h_r` already exists in `hares-envelope/src/longwave_radiation.rs:239` — the ticket's proposed helper would duplicate it; (3) the unit test expected value (~5.79 W/(m²·K) at T=295 K, ε=0.9) is wrong by 10.6% — the correct value is 5.241; (4) the production code occurrences in `boundary_rc.rs` include an area multiplier `a` in the conductance (G = 4εσAT³), making the scalar helper `linearised_h_rad(ε, T) → f64` a partial solution that still leaves `* a` inline; (5) the existing `linearised_h_r` uses Celsius input while the proposed helper uses Kelvin — the ticket does not address this interface discrepancy.

### Proposed Fix Summary

Do NOT implement this. A correct implementation should:
1. Evaluate whether to place the helper in `hares-physics` or `hares-envelope` given that `linearised_h_r` already exists in `hares-envelope`; the two helpers should be unified rather than duplicated.
2. Unify `linearised_h_r` (Celsius, per-unit-area) and the proposed `linearised_h_rad` (Kelvin, per-unit-area) into a single canonical function — choose one calling convention consistently.
3. Correct the expected test value from ~5.79 to 5.241 W/(m²·K) at T=295 K, ε=0.9, or use the better canonical point T=293.15 K → 5.143 W/(m²·K).
4. The hardcoded `SIGMA = 5.670374e-8` in `rc_network.rs` test code should reference `hares_physics::constants::STEFAN_BOLTZMANN` — this is the primary σ drift risk.
5. The production callsites in `boundary_rc.rs` (lines 705, 920) are the only non-test occurrences and are the genuine DRY risk; the test-code occurrences are low priority.

### Test Written

- **File**: `crates/hares-envelope/src/longwave_radiation.rs` (appended to existing `#[cfg(test)]` module)
- **Function**: `ticket_115_linearised_h_r_value_and_sigma_correctness`
- **What it tests**:
  1. `STEFAN_BOLTZMANN` constant matches NIST CODATA 2018 (5.670374419×10⁻⁸) to full precision.
  2. `linearised_h_r(0.9, 20.0)` returns the mathematically correct value 5.143 W/(m²·K).
  3. `linearised_h_r(0.9, 21.85°C)` returns 5.241 W/(m²·K) at T=295 K — refuting the ticket's incorrect ~5.79 claim.
  4. The truncated σ = 5.670374e-8 used in `rc_network.rs` tests is within 1 ppm of NIST (confirming no numerical hazard today, but documenting the DRY risk).
