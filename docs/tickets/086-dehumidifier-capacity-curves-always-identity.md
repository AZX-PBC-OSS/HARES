# Dehumidifier Capacity and Energy Factor Curves Always Identity — No Temperature Dependence

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-equipment

## Problem

`Dehumidifier::init_from_typed` always sets both biquadratic curves to the identity
constant `[1.0, 0.0, 0.0, 0.0, 0.0, 0.0]`, regardless of any curve data that might
be available. This means dehumidifier water removal capacity and energy factor are
constant across all dry-bulb temperatures and relative humidity levels. In reality,
dehumidifier performance varies significantly with conditions: at 60°F/60% RH (AHAM
rated conditions) a unit may remove 30 L/day, but at 50°F it may remove only 20 L/day
(-33%) due to reduced refrigerant cycle effectiveness.

EnergyPlus models this with two biquadratic curves (capacity-vs-conditions,
energy-factor-vs-conditions). HARES provides the curve infrastructure
(`BiquadraticCurve`) but hardcodes identity, masking all temperature/humidity
dependence.

## Evidence

`dehumidifier.rs:241–252`:

```
self.water_removal_curve = BiquadraticCurve {
    coeffs: DEFAULT_NORMALIZED_CURVE,
    x1_bounds: DEFAULT_DB_BOUNDS_C,
    x2_bounds: DEFAULT_RH_BOUNDS,
};
self.energy_factor_curve = BiquadraticCurve {
    coeffs: DEFAULT_NORMALIZED_CURVE,
    x1_bounds: DEFAULT_DB_BOUNDS_C,
    x2_bounds: DEFAULT_RH_BOUNDS,
};
self.water_removal_curve_rated_value = 1.0;
self.energy_factor_curve_rated_value = 1.0;
```

Where `DEFAULT_NORMALIZED_CURVE: [f64; 6] = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0]`.

This is hard-coded even when the legacy (non-typed) init path may have loaded
real curves. The typed-config path unconditionally overwrites with identity.

## OCHRE Cross-check

OCHRE `Equipment/Dehumidifier.py` loads biquadratic curves for water removal and
energy factor as a function of dry-bulb temperature and relative humidity from
default CSV files. The curves are normalised to the AHAM DH-1 rated condition
(60°F / 60% RH for DH-1-2009). HARES has the BiquadraticCurve struct but no
default coefficients for dehumidifiers.

## Cross-Reference

Ticket 010 (default biquadratic performance curves) covers the same defect shape
for HVAC cooling coils. That ticket's solution for loading default curves from a
constants module or CSV data file should be adopted here for dehumidifier curves.
Do not merge — different equipment, different curve variables (dehumidifier curves
use dry-bulb °C and RH fraction; cooling coils use WB and OAT).

Ticket 082 (dehumidifier pint conversion) establishes the rated reference condition
that the normalization value must match.

## Required Behavior

1. Source default biquadratic curve coefficients for dehumidifier water removal
   and energy factor as a function of dry-bulb temperature (°C) and relative humidity
   (fraction). Primary reference: EnergyPlus `ZoneHVAC:Dehumidifier:DX` example file
   coefficients (`WaterRemovalCurve` and `EnergyFactorCurve` from `ExampleFiles/`).
2. Store default coefficients in a constants module inside `hares-equipment/src/hvac/`
   (matching the pattern for cooling coil defaults).
3. Load these defaults in `dehumidifier.rs:init_from_typed` (line 241 area) when no
   explicit curve data is provided in the typed config.
4. Retain identity as a last-resort fallback only.
5. Set `water_removal_curve_rated_value` and `energy_factor_curve_rated_value` to the
   curve output evaluated at the AHAM DH-1-2009 rated condition (15.56°C, 0.60 RH),
   so that `rated_capacity_liters_per_day` passes through unchanged at rated conditions.

## Approach

- In `hares-equipment/src/hvac/dehumidifier_defaults.rs` (new file or addition to existing defaults module), define:
  ```rust
  pub const DEFAULT_WATER_REMOVAL_CURVE: [f64; 6] = [ ... ]; // from EnergyPlus DX dehumidifier dataset
  pub const DEFAULT_ENERGY_FACTOR_CURVE: [f64; 6] = [ ... ];
  pub const AHAM_DH1_2009_RATED_DB_C: f64 = 15.56;
  pub const AHAM_DH1_2009_RATED_RH: f64 = 0.60;
  ```
- In `dehumidifier.rs:init_from_typed` (line 241), replace hardcoded identity:
  ```rust
  self.water_removal_curve = BiquadraticCurve {
      coeffs: DEFAULT_WATER_REMOVAL_CURVE,
      x1_bounds: DEFAULT_DB_BOUNDS_C,
      x2_bounds: DEFAULT_RH_BOUNDS,
  };
  // compute rated value at (AHAM_DH1_2009_RATED_DB_C, AHAM_DH1_2009_RATED_RH)
  self.water_removal_curve_rated_value =
      self.water_removal_curve.evaluate(AHAM_DH1_2009_RATED_DB_C, AHAM_DH1_2009_RATED_RH);
  ```
  Same for `energy_factor_curve`.

## Citation

- EnergyPlus Engineering Reference §16.9: `ZoneHVAC:Dehumidifier:DX` — two
  biquadratic curves (water removal and energy factor vs. dry-bulb and RH)
- AHAM DH-1-2009: rated at 60°F/60% RH; DH-1-2021: 65°F/60% RH
- EnergyPlus example curves for residential dehumidifiers:
  `EnergyPlus/DataSets/DX_Coil_BiQuadratic.csv`

## Annual kWh Impact Rank

**Medium.** In mixed-climate zones (2–4) where dehumidifiers run in shoulder seasons
at below-rated temperatures, identity curves overstate water removal and understate
energy consumption. For a 30 L/day rated unit running in a 60°F basement, real
capacity may be 20 L/day — the identity curve overstates by 50%, causing the
simulation to end dehumidification cycles earlier and undercount runtime.

## Definition of Done

- [ ] Default biquadratic coefficients sourced from EnergyPlus `ZoneHVAC:Dehumidifier:DX` dataset and stored in a named constants module inside `hares-equipment/src/hvac/`
- [ ] `init_from_typed` at `dehumidifier.rs:241` loads defaults instead of identity when no explicit curve is provided
- [ ] `water_removal_curve_rated_value` and `energy_factor_curve_rated_value` computed at AHAM DH-1-2009 rated condition (15.56°C, 0.60 RH), not hardcoded to 1.0
- [ ] Identity retained as fallback only for the case where no defaults module is available
- [ ] Test: at (15.56°C, 0.60 RH), `water_removal_curve.evaluate(...)` × `water_removal_curve_rated_value` normalizes to 1.0 (rated output)
- [ ] Test: at (10.0°C, 0.60 RH), curve output factor < 1.0 (capacity decreases below rated at lower temperature)

## Verification

```bash
cargo test -p hares-equipment -- hvac::dehumidifier
```

Compare capacity prediction at 10°C against EnergyPlus reference output for `ZoneHVAC:Dehumidifier:DX` using the same curve coefficients.

---

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation
- [x] Referenced line numbers still match — `init_from_typed` identity assignment is at lines 241–252 (exact match)
- [x] Described logic matches current implementation — `DEFAULT_NORMALIZED_CURVE: [f64; 6] = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0]` confirmed at line 38; unconditionally assigned at lines 241–252; `water_removal_curve_rated_value` hardcoded to `1.0` at line 251
- [x] OCHRE cross-check result: **N/A** — OCHRE has no dehumidifier equipment class. `vendors/OCHRE/ochre/Equipment/HVAC.py` contains no dehumidifier; `ochre/Models/Humidity.py:44` contains only a `# FUTURE: Dehumidifier?` comment; `ochre/utils/hpxml.py` references dehumidifier only in parsing. OCHRE does not implement a biquadratic dehumidifier model to cross-check against.
- [x] EnergyPlus cross-check result: **diverges** — EnergyPlus `ZoneHVAC:Dehumidifier:DX` requires two biquadratic curves (water removal and energy factor) normalized to 1.0 at rated conditions. The Fortran source (`ZoneDehumidifier.f90`) defines `RatedInletAirTemp = 26.7d0` and `RatedInletAirRH = 60.0d0` and validates that curves return ≈1.0 at these conditions. HARES provides `BiquadraticCurve` infrastructure but hard-codes identity coefficients, giving no temperature or RH dependence whatsoever — a clear functional divergence.

### Web-Verified Citations

**Citation 1**
- **Citation**: Ticket claims "EnergyPlus Engineering Reference §16.9: `ZoneHVAC:Dehumidifier:DX` — two biquadratic curves (water removal and energy factor vs. dry-bulb and RH)"
- **Source found**: Big Ladder Software EnergyPlus 8.2 Engineering Reference — [Zone Equipment and Zone Forced Air Units](https://bigladdersoftware.com/epx/docs/8-2/engineering-reference/zone-equipment-and-zone-forced-air-units.html#zone-air-dx-dehumidifier)
- **Quoted passage**: "Three performance curves must also be specified to characterize the change in water removal and energy consumption at part-load conditions... `WaterRemovalModFac = a + b(Tin) + c(Tin)² + d(RHin) + e(RHin)² + f(Tin)(RHin)`... `EFModFac = a + b(Tin) + c(Tin)² + d(RHin) + e(RHin)² + f(Tin)(RHin)`... `Tin` = dry-bulb temperature entering the dehumidifier, °C; `RHin` = relative humidity of the air entering the dehumidifier, % (0–100)"
- **Verdict**: **Partially correct** — the two-curve model is confirmed. The section number "§16.9" could not be independently verified (the online HTML version does not carry section numbers), but the substance is correct. **Important additional finding**: EnergyPlus defines RHin as **percent (0–100)**, not fraction (0–1). HARES uses `DEFAULT_RH_BOUNDS: (0.0, 1.0)` (fraction). Any future default coefficients sourced from EnergyPlus must be re-fitted or the x2 variable scaled to match the fraction domain used internally.

**Citation 2**
- **Citation**: Ticket claims "AHAM DH-1-2009: rated at 60°F/60% RH; DH-1-2021: 65°F/60% RH"
- **Source found**: Multiple sources: EnergyPlus Fortran source `ZoneDehumidifier.f90` (GitHub: nrgsim/EnergyPlus-Fortran); [Building Simulation Data IDD Explorer](https://www.building-simulation-data.com/IDD-explorer/class/ZONEHVAC_DEHUMIDIFIER_DX); [Simergy Zone Dehumidifier docs](https://d-alchemy.com/html/helpdocs/Simergy/Content/HVAC_Systems/ZHG_Components/Zone_Dehumidifier.htm); [TURBRO AHAM explainer](https://www.turbro.com/blogs/blogs/understanding-capacity-in-dehumidifiers-aham-vs-saturation); AHAM Verification Program Procedural Guide.
- **Quoted passage**: From `ZoneDehumidifier.f90`: `REAL(r64), PARAMETER :: RatedInletAirTemp = 26.7d0` and `REAL(r64), PARAMETER :: RatedInletAirRH = 60.0d0`. From Big Ladder EnergyPlus 8.2 Engineering Reference: "user inputs are based on rated conditions (26.7°C, 60% RH)" and "normalized to have the value of 1.0 at the rating point (air entering the dehumidifier at 26.7°C [80°F] dry-bulb and 60% relative humidity)." From TURBRO and AHAM program sources: "Performance at AHAM conditions is 80°F / 60% Relative Humidity (RH)." From DOE Federal Register 2023: "The DOE test procedure specifies a different dry-bulb temperature (65°F for portable dehumidifiers) than ANSI/AHAM DH-1-2008."
- **Verdict**: **Incorrect** — The ticket states the AHAM DH-1-2009 rated condition is **60°F (15.56°C)** / 60% RH. This is wrong. The AHAM DH-1 rated condition — as used in EnergyPlus since its introduction and corroborated by every available source — is **26.7°C (80°F) / 60% RH**. The DOE's 2023 updated test procedure (for portable units) uses 65°F/60% RH, not 60°F. There is no evidence that any version of AHAM DH-1 used 60°F/60%RH as the rated condition; 60°F may be a "cold basement" scenario cited as a stress-test condition rather than a rating point. The ticket's `AHAM_DH1_2009_RATED_DB_C: f64 = 15.56` constant in the proposed fix would be incorrect. The correct rated temperature is `26.7°C` (or equivalently `26.6667°C` = exact 80°F conversion), matching `RatedInletAirTemp = 26.7d0` in EnergyPlus.

**Citation 3**
- **Citation**: Ticket references "EnergyPlus example curves for residential dehumidifiers: `EnergyPlus/DataSets/DX_Coil_BiQuadratic.csv`"
- **Source found**: Searched EnergyPlus GitHub and documentation. The EnergyPlus repository has `datasets/` directory with curve CSV files, but no file named `DX_Coil_BiQuadratic.csv` was verifiable from public search results. The EnergyPlus test IDF files for `ZoneHVAC:Dehumidifier:DX` were not found in public search results with explicit coefficient values.
- **Verdict**: **Cannot fully verify** — The file path cited (`EnergyPlus/DataSets/DX_Coil_BiQuadratic.csv`) is plausible but unconfirmed. EnergyPlus datasets directory exists (verified via search), but whether this specific CSV contains dehumidifier (rather than cooling coil) curves is uncertain. The ticket's requirement to source coefficients from this file remains valid in principle but the specific file reference needs confirmation against the actual EnergyPlus installation.

### Legitimacy
- **Verdict**: **Partially Legitimate**
- **Rationale**: The core bug is real and confirmed: `init_from_typed` unconditionally assigns identity biquadratic coefficients (`[1.0, 0.0, 0.0, 0.0, 0.0, 0.0]`) at lines 241–252, making dehumidifier capacity and energy factor identical at all temperatures and humidity levels. OCHRE provides no dehumidifier model to cross-reference, so that comparison is N/A. EnergyPlus `ZoneHVAC:Dehumidifier:DX` unambiguously uses two biquadratic curves and the divergence from HARES is confirmed. The ticket's impact assessment (capacity overstated at low temperatures, runtime undercounted) is physically correct. However, **the ticket contains a critical factual error**: it states the AHAM DH-1-2009 rated condition is 60°F (15.56°C) / 60% RH, but every verifiable source — including the EnergyPlus Fortran source constant `RatedInletAirTemp = 26.7d0` and all AHAM documentation — confirms the correct rated condition is **26.7°C (80°F) / 60% RH**. The proposed constant `AHAM_DH1_2009_RATED_DB_C: f64 = 15.56` in the fix approach would be wrong. Additionally, EnergyPlus biquadratic curves for this object use RH as percent (0–100), while HARES uses fraction (0–1); any coefficients copied directly from EnergyPlus must be adapted for this domain difference. The proposed solution structure is otherwise sound.

### Proposed Fix Summary
1. Source biquadratic coefficients for water removal and energy factor from the EnergyPlus `ZoneHVAC:Dehumidifier:DX` dataset (verify the `DataSets/` CSV path) or fit from published data.
2. **Correct the rated condition**: use `AHAM_RATED_DB_C: f64 = 26.666_666_666_7` (26.6̄°C = exactly 80°F) and `AHAM_RATED_RH: f64 = 0.60`, not `15.56` as stated in the ticket.
3. When adapting EnergyPlus coefficients, note that EnergyPlus encodes RH as percent (0–100). HARES uses fraction (0–1). Coefficients from EnergyPlus must be re-expressed so that `f(T, rh_fraction)` = `f_ep(T, rh_fraction * 100)`. This rescaling affects coefficients `d`, `e`, and `f`: `d_new = d_ep * 100`, `e_new = e_ep * 10000`, `f_new = f_ep * 100`.
4. In `init_from_typed` (lines 241–252) replace identity coefficients with the sourced defaults.
5. Set `water_removal_curve_rated_value = curve.evaluate(26.666_666_666_7, 0.60)` (not `1.0`) so the normalization self-corrects for any minor deviation of the default curve at rated conditions.
6. Retain identity as final fallback only.

### Test Written
- **File**: `crates/hares-equipment/src/hvac/dehumidifier.rs` (in the existing `#[cfg(test)]` module)
- **Tests added**:
  1. `water_removal_decreases_below_rated_temperature` — **FAILS** until fix applied. Runs the dehumidifier at 26.7°C/60%RH and 10°C/60%RH; asserts that capacity at 10°C is strictly less than at rated. With identity curves both return `33.1224 L/day` (identical), failing the assertion.
  2. `water_removal_at_rated_condition_equals_rated_capacity` — passes with identity curves (trivially, since multiplier = 1.0) AND must continue to pass after the fix (normalization round-trip). Asserts that `water_removal_l_day` at 26.7°C/60%RH equals `rated_capacity_liters_per_day` to within 1e-9 relative error.
