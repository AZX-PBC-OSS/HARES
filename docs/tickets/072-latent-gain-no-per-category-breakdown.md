# ThermalAccumulator: No Per-Category Latent Breakdown

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-types, hares-envelope
**Note**: Land ticket 071 (adds `HvacDehumidification`) before this ticket so the new category is immediately covered in `latent_by_category`.

## Problem

`ThermalAccumulator` tracks per-category sensible and radiant heat in fixed-size arrays (`sensible_by_category`, `radiant_by_category`) but `latent_gain_w` is a single scalar with no per-category breakdown. This makes it impossible to separate HVAC latent extraction (negative, from AC or dehumidifier) from internal-gain latent additions (positive, from occupants or humidifiers) in zone energy-balance diagnostics.

Without a per-category latent breakdown:

1. The sensible/latent closure check — `Q_sens + Q_lat must equal total zone enthalpy change` — cannot be attributed by source. Energy balance violations (ticket 048) cannot be isolated to specific equipment.
2. It is impossible to verify that cooling-coil latent extraction and dehumidifier latent extraction sum correctly against the humidity solver's computed moisture removal.
3. Diagnostic output cannot separately report "HVAC latent removal" from "occupant latent addition", preventing EnergyPlus-parity reporting.

EnergyPlus Engineering Reference §3.2 "Zone Air Moisture Balance": the moisture balance equation distinguishes `ΣQlatent_HVAC` from `ΣQlatent_internal` as separate source terms. ASHRAE Handbook of Fundamentals 2021 Ch. 1 Eq. 41: latent load = `Σ(m_dot_i × h_fg × Δω_i)` per source term.

## Current Behavior

`hares-types/src/ports.rs:164–172`:

```rust
pub struct ThermalAccumulator {
    pub zone: ZoneId,
    pub sensible_gain_w: f64,
    pub radiant_gain_w: f64,
    pub latent_gain_w: f64,                              // scalar only
    pub sensible_by_category: [f64; THERMAL_CATEGORY_COUNT],
    pub radiant_by_category: [f64; THERMAL_CATEGORY_COUNT],
    // latent_by_category does not exist
}
```

`ports.rs:186–198` `add()` updates aggregate totals and per-category arrays for sensible and radiant, but only the aggregate total for latent. No `latent_by_category` update.

The humidity solver (`humidity_solver.rs:116–122`) consumes `latent_gain_w` as a single sum across all thermal accumulators for a zone. It cannot distinguish HVAC extraction from occupant addition when the net total is ambiguous.

## Required Behavior

Add `pub latent_by_category: [f64; THERMAL_CATEGORY_COUNT]` to `ThermalAccumulator`, updated in `add()` consistently with `sensible_by_category`. Add a `latent_for_category()` accessor parallel to `sensible_for_category()`.

Invariant: `sum(latent_by_category) == latent_gain_w` must hold after every `add()` call and after `zero()`. This invariant must be verified by a test.

`zero()` must reset the new array to `[0.0; THERMAL_CATEGORY_COUNT]`.

Reference: EnergyPlus Engineering Reference §3.2 "Zone Air Moisture Balance"; OCHRE results dictionary — per-end-use latent gain reporting (e.g., `"HVAC Cooling Latent Gains (W)"` separate from `"Internal Gains Latent Gains (W)"`).

## Approach

1. In `ports.rs`, add `pub latent_by_category: [f64; THERMAL_CATEGORY_COUNT]` to `ThermalAccumulator`.
2. Initialize to `[0.0; THERMAL_CATEGORY_COUNT]` in `ThermalAccumulator::new()` (or the equivalent constructor site).
3. In `add()`, append `self.latent_by_category[category.index()] += latent_gain_w;`.
4. In `zero()`, add `self.latent_by_category = [0.0; THERMAL_CATEGORY_COUNT];`.
5. Add `pub fn latent_for_category(&self, cat: ThermalCategory) -> f64 { self.latent_by_category[cat.index()] }`.
6. Update the struct doc comment to include the latent invariant.

## Definition of Done

- [ ] `ThermalAccumulator` has `pub latent_by_category: [f64; THERMAL_CATEGORY_COUNT]`
- [ ] `add()` updates `latent_by_category[category.index()]`
- [ ] `zero()` resets `latent_by_category` to all zeros
- [ ] `latent_for_category()` accessor exists and returns the correct bucket
- [ ] Test: `sum(latent_by_category) == latent_gain_w` after a sequence of mixed-category `add()` calls (numeric equality, not approximate)
- [ ] Test: `zero()` resets `latent_by_category` — every element is 0.0
- [ ] No existing tests broken

## Verification

```bash
cargo test -p hares-types
cargo test -p hares-envelope
```

## References

- EnergyPlus Engineering Reference §3.2 "Zone Air Moisture Balance" — per-source latent term attribution
- ASHRAE Handbook of Fundamentals 2021 Ch. 1 Eq. 41 — latent load = Σ(m_dot_i × h_fg × Δω_i) per source term
- OCHRE results dictionary — per-end-use latent gain reporting
- `hares-types/src/ports.rs:164–205` — `ThermalAccumulator` struct and `add()` / `zero()`

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match (struct definition: lines 165–172; `add()`: lines 186–198; `zero()`: lines 200–206 — ticket cited 164–172 and 186–198, off by one due to doc-comment lines, but describes the correct location)
- [x] Described logic matches current implementation: `latent_gain_w` is a single `f64` scalar; `latent_by_category` does not exist; `add()` updates `self.latent_gain_w` but does NOT update any per-category latent array; `zero()` resets `latent_gain_w` but there is no `latent_by_category` to reset.
- [x] OCHRE cross-check: **Partially matches OCHRE, but one specific claim is inaccurate.** OCHRE tracks HVAC latent and internal latent gains as *separate internal variables* (`hvac_latent_gain` and `internal_latent_gain`) during calculation (Envelope.py lines 1294–1297) but reports them only as: (a) `"HVAC <Heating or Cooling> Latent Gains (W)"` per HVAC end-use (Variable names and units.csv line 22), and (b) `"Net Latent Heat Gain - Indoor (W)"` as a zone aggregate (CSV line 149). The ticket asserts OCHRE reports a key called `"Internal Gains Latent Gains (W)"` — **this key does not exist** anywhere in the OCHRE codebase or its output variable CSV.
- [x] EnergyPlus cross-check: **Substantively matches.** The EnergyPlus Moisture Predictor-Corrector (bigladdersoftware.com/epx/docs/8-7/engineering-reference/moisture-predictor-corrector.html and 25-2 equivalent) presents the zone moisture balance as: `ρ_air·V_z·C_W·dW_z/dt = Σkg_mass_sched_load + Σ(A_i·h_m_i·ρ·(W_surf−W_tz)) + Σṁ_i·(W_zi−W_tz) + ṁ_inf·(W_∞−W_tz) + ṁ_sys·(W_sup−W_tz)`. Internal scheduled latent loads (`Σkg_mass_sched_load`) and the HVAC system supply term (`ṁ_sys·(W_sup−W_tz)`) are distinct summed components. This confirms the ticket's core claim that EnergyPlus treats these as separate source terms, validating the need for per-category attribution.

### Web-Verified Citations

**Citation 1**

- **Citation**: "EnergyPlus Engineering Reference §3.2 'Zone Air Moisture Balance'"
- **Source found**: EnergyPlus Engineering Reference 25.2, Big Ladder Software (https://bigladdersoftware.com/epx/docs/25-2/engineering-reference/moisture-predictor-corrector.html); also version 8.7 at https://bigladdersoftware.com/epx/docs/8-7/engineering-reference/moisture-predictor-corrector.html
- **Quoted passage**: "The transient air mass balance equation for the change in the zone humidity ratio = sum of internal scheduled latent loads + infiltration + system + multizone airflows + convection to the zone surfaces." The section is titled **"Moisture Predictor-Corrector"** and does not carry the section number "3.2". The EnergyPlus Engineering Reference does not use numbered sections; the moisture balance content lives under "Integrated Solution Manager → Moisture Predictor-Corrector", not a standalone "§3.2".
- **Verdict**: **Partially correct.** The substance (EnergyPlus distinguishes internal latent loads from HVAC system supply as separate terms) is accurate. The section reference "§3.2 'Zone Air Moisture Balance'" is inaccurate — no such section number or title exists in any version of the EnergyPlus Engineering Reference reviewed (8.1, 8.7, 25.2).

**Citation 2**

- **Citation**: "ASHRAE Handbook of Fundamentals 2021 Ch. 1 Eq. 41 — latent load = Σ(m_dot_i × h_fg × Δω_i) per source term"
- **Source found**: ASHRAE Handbook of Fundamentals 2021 SI, Chapter 1 "Psychrometrics" (https://handbook.ashrae.org/Handbooks/F21/SI/F21_Ch01/F21_Ch01_si.aspx)
- **Quoted passage**: Equation 41 in Ch. 1 is **"h = hda + W·hg"** — specific enthalpy of moist air expressed using degree of saturation. It is an enthalpy identity, not a per-source latent load summation formula. The per-source latent load relationship (`Q̇_lat = ṁ_da·(W₁−W₂)·h_w₂`) appears at **Equation 45**, not Equation 41.
- **Verdict**: **Incorrect.** ASHRAE HoF 2021 Ch. 1 Eq. 41 is `h = hda + W·hg`, an enthalpy definition. The formula the ticket attributes to it (`Σ(m_dot_i × h_fg × Δω_i)`) does not correspond to Eq. 41. The conceptual claim (latent load can be decomposed as a sum of per-source moisture-flux terms weighted by h_fg) is physically sound, but the citation is wrong.

**Citation 3**

- **Citation**: "OCHRE results dictionary — per-end-use latent gain reporting (e.g., `'HVAC Cooling Latent Gains (W)'` separate from `'Internal Gains Latent Gains (W)'`)"
- **Source found**: OCHRE Variable names and units.csv (vendors/OCHRE/ochre/defaults/Variable names and units.csv) and OCHRE documentation at https://ochre-nrel.readthedocs.io/en/stable/Outputs.html
- **Quoted passage** (from CSV): Line 22: `"HVAC <Heating or Cooling> Latent Gains (W), W, ..., HVAC latent heat gain delivered to indoor zone"`. Line 149: `"Net Latent Heat Gain - Indoor (W), W, ..., Net latent heat injected into zone (Includes heat gains from infiltration, ventilation, HVAC, other equipment, and occupants)"`. No entry named `"Internal Gains Latent Gains (W)"` exists anywhere in the CSV or in any OCHRE Python source file.
- **Verdict**: **Partially correct.** OCHRE does report `"HVAC Cooling Latent Gains (W)"` separately from the net total. However, the ticket's claim that OCHRE reports a key `"Internal Gains Latent Gains (W)"` is false — this output variable does not exist in OCHRE. The ticket misrepresents OCHRE's output structure as having a symmetric per-category breakdown that it does not actually have.

### Legitimacy

- **Verdict**: **Partially Legitimate**
- **Rationale**: The core bug is real and confirmed: `ThermalAccumulator.latent_gain_w` is a scalar with no per-category array, `add()` does not route latent gains to any per-category bucket, and `latent_for_category()` does not exist — all exactly as described. The asymmetry with `sensible_by_category` and `radiant_by_category` is genuine and the proposed fix is structurally sound. However, two of the three citations contain errors: (1) EnergyPlus §3.2 "Zone Air Moisture Balance" is not a real section reference — the relevant content is titled "Moisture Predictor-Corrector" with no numeric section label; (2) ASHRAE HoF 2021 Ch. 1 Eq. 41 is `h = hda + W·hg`, not a per-source latent summation formula — the cited equation number is wrong. (3) OCHRE does not report `"Internal Gains Latent Gains (W)"` as a named output. The physics motivation in the ticket is correct, but the specific citations need correction before publication.

### Proposed Fix Summary

In `crates/hares-types/src/ports.rs`:
1. Add `pub latent_by_category: [f64; THERMAL_CATEGORY_COUNT]` to `ThermalAccumulator`.
2. Initialize to `[0.0; THERMAL_CATEGORY_COUNT]` in `new()`.
3. In `add()`, append `self.latent_by_category[category.index()] += latent_gain_w;` alongside the existing scalar update.
4. In `zero()`, add `self.latent_by_category = [0.0; THERMAL_CATEGORY_COUNT];`.
5. Add `pub fn latent_for_category(&self, cat: ThermalCategory) -> f64 { self.latent_by_category[cat.index()] }`.
6. Update the struct doc comment to document the invariant `sum(latent_by_category) == latent_gain_w`.
No changes needed in `humidity_solver.rs` — it correctly consumes `latent_gain_w` as the aggregate total.

### Test Written

- **File**: `crates/hares-types/tests/type_interop.rs` (appended at end of file)
- **Tests added**:
  - `latent_category_sum_matches_total` — verifies `sum(latent_by_category) == latent_gain_w` after mixed-category `add()` calls including negative HVAC cooling and positive internal-gain latent values; also checks per-category subtotals individually.
  - `zero_resets_latent_by_category` — verifies `zero()` resets every element of `latent_by_category` to 0.0.
- **Current status**: Both tests **fail to compile** with `error[E0599]: no method named 'latent_for_category' found for reference '&ThermalAccumulator'`, confirming the bug is present and unresolved. They will pass once the fix in ticket 072 is implemented.
