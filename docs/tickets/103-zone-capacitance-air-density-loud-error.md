# `derive_zone_capacitances` Must Error When `site_pressure_pa <= 0`

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-envelope/boundary_rc

## Problem

`derive_zone_capacitances` at `crates/hares-envelope/src/boundary_rc.rs:362-375` silently substitutes the sea-level air density `AIR_DENSITY_KG_M3 = 1.2041` when `site_pressure_pa <= 0`. A non-positive pressure is unambiguously a configuration or parsing error — there is no physical site at or below zero absolute pressure. Substituting a sea-level constant masks the error and produces zone capacitances that are wrong for any non-sea-level site.

## Current Behavior

`crates/hares-envelope/src/boundary_rc.rs:362-375`:
```rust
let rho_air = if site_pressure_pa > 0.0 {
    compute_air_density(site_pressure_pa, ...)
} else {
    AIR_DENSITY_KG_M3  // silent sea-level fallback
};
```

A misconfigured site (e.g. weather file with missing pressure column, EPW field 9 sentinel 999999) silently uses 1.2041 kg/m³ regardless of actual elevation. At a 1500 m site (Denver, e.g.) actual rho_air is ~1.05 kg/m³ — a 14% bias in zone air capacitance.

## Required Behavior

1. If `site_pressure_pa <= 0`, return `Err(BoundaryRcError::InvalidSitePressure { value })` with the offending value.
2. Do not substitute any fallback density.
3. The error propagates to the caller and surfaces as a hard initialisation failure.
4. The companion weather-loader path that produces `site_pressure_pa` must also surface the issue at parse time (separate ticket if not already addressed by ticket 029 / EPW pressure handling).

## Approach

1. Open `crates/hares-envelope/src/boundary_rc.rs:362-375` and replace the `else` branch with `return Err(...)`.
2. Add the error variant to the local error enum.
3. Plumb the error through the caller (`Dwelling::new` or solver builder).
4. Add a unit test asserting the error fires for `site_pressure_pa = 0.0` and `-1.0`.
5. Add a sanity test asserting valid pressures still produce non-zero capacitances.

## Definition of Done

- [ ] Silent fallback to `AIR_DENSITY_KG_M3` at `boundary_rc.rs:362-375` removed
- [ ] New error variant `BoundaryRcError::InvalidSitePressure` carries the offending value
- [ ] Error propagates to caller
- [ ] Unit tests cover `site_pressure_pa = 0.0`, `< 0.0`, and a representative valid value
- [ ] No other silent default in `derive_zone_capacitances` (audit complete)

## Verification

```bash
cargo test -p hares-envelope boundary_rc
cargo test -p hares-core dwelling
```

## References

- ASHRAE Handbook of Fundamentals 2021 Ch. 1 §1.2 "Atmospheric Pressure" — pressure is strictly positive; standard ISA formula `p(h) = 101325 * (1 - 2.25577e-5 * h)^5.2559`.
- EnergyPlus Engineering Reference §1 "Site:Location" — site pressure derived from elevation when not explicitly provided; never zero or negative.
- Project policy `feedback_no_silent_defaults.md`.

## Related Tickets

- 029-resstock-csv-constant-pressure (related ResStock pressure handling)
- 102-thermal-solver-init-indoor-zone-loud-error (parallel silent-default fix)

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match — `derive_zone_capacitances` is at `boundary_rc.rs:350-376`; the silent fallback branch is at lines 360-364. The ticket cites "362-375" which is slightly off (the function starts at 350, the `else` branch is at 360-364), but the code described is present and correct.
- [x] Described logic matches current implementation — `if site_pressure_pa > 0.0` uses ideal gas law; `else` silently returns `AIR_DENSITY_KG_M3 = 1.2041`. The function signature is `pub fn derive_zone_capacitances(zones: &[ZoneInput], site_pressure_pa: f64) -> Vec<f64>` — it returns `Vec<f64>`, not `Result`.
- [x] Bug is not yet fixed — confirmed by two passing regression tests (`ticket_103_zero_pressure_silently_uses_sea_level_density_not_an_error`, `ticket_103_negative_pressure_silently_uses_sea_level_density_not_an_error`).
- [x] OCHRE cross-check: **OCHRE intentionally uses a hardcoded sea-level density for zone capacitance**. `vendors/OCHRE/ochre/Models/Envelope.py:11` defines `rho_air = 1.2041  # kg/m^3, used for determining capacitance only` and uses it unconditionally at line 446: `self.capacitance = self.volume * rho_air * cp_air * capacitance_multiplier`. OCHRE's own comment ("used for determining capacitance only") acknowledges this as a deliberate simplification. HARES's fallback reproduces this OCHRE behaviour but the ticket is correct that it should be a hard error — the OCHRE simplification is a known limitation HARES is intentionally improving upon.
- [x] EnergyPlus cross-check: EnergyPlus `PsyRhoAirFnPbTdbW` (verified at `github.com/NREL/EnergyPlus` Psychrometrics.hh) computes density as `pb / (287.0 × (tdb + KelvinConv) × (1 + 1.6077687 × max(w, 1e-5)))` — it checks the *result* for negativity but does not guard the input pressure against zero or negative values. EnergyPlus derives site pressure from elevation using `P = 101325 × (1 − Z × 2.25577×10⁻⁵)^5.2559` (ASHRAE 1997 HOF), which is always positive for realistic altitudes. EnergyPlus never passes zero or negative pressure to its density functions in normal operation — the pressure is always elevation-derived. HARES matches EnergyPlus in the ideal-gas formula; the divergence is HARES allowing `site_pressure_pa = 0` as input at all.
- [x] EPW sentinel cross-check: EPW field 9 (Atmospheric Station Pressure) uses `999999` as its missing-value sentinel (confirmed: EnergyPlus Auxiliary Programs 23.2 docs — "Missing value for this field is 999999"). However, `crates/hares-io/src/epw.rs:161-164` already validates pressure into the range `[60, 110] kPa` and hard-errors on violation; `999999 Pa = ~1000 kPa` would be caught and rejected before reaching `derive_zone_capacitances`. So the EPW-sentinel path cited in the ticket does **not** actually produce `site_pressure_pa = 0`. The realistic zero-pressure path is from `ResStock CSV` or other callers that construct `WeatherData` without setting pressure (ticket 029).

### Web-Verified Citations

**Citation 1**: "ASHRAE Handbook of Fundamentals 2021 Ch. 1 §1.2 'Atmospheric Pressure' — pressure is strictly positive; standard ISA formula `p(h) = 101325 * (1 - 2.25577e-5 * h)^5.2559`"

- **Source found**: ASHRAE HoF 2021 Ch. 1 (SI) via `handbook.ashrae.org/Handbooks/F21/SI/F21_Ch01/F21_Ch01_si.aspx`
- **Quoted passage**: "At sea level, standard temperature is 15°C; standard barometric pressure is 101.325 kPa. … Pressure values in Table 1 may be calculated from [Eq. (3)]: `p = 101.325(1 − 0.0065Z/288.15)^5.255`" where Z is altitude in metres. The document states the equation is "accurate from −5000 m to 11,000 m."
- **Verdict**: **Partially correct.** The formula and constant 101325 are real and correct in spirit, but the ticket cites "§1.2" and the exact coefficients `2.25577e-5` and `5.2559` that appear in ISA 1976 / ICAO Doc 7488 and EnergyPlus, not verbatim in ASHRAE §1.2 which uses slightly different rounding (`0.0065/288.15 = 2.2577×10⁻⁵` and exponent `5.255`). The HARES constants (`ISA_LAPSE_COEFFICIENT = 2.255_77e-5`, `ISA_PRESSURE_EXPONENT = 5.2559`) match ISA 1976 / ICAO Doc 7488, not ASHRAE §1.2 exactly. This is a minor discrepancy in the citation. The claim that pressure is strictly positive at physical altitudes is correct — the formula yields positive values for all altitudes below ~44 km.
- **Claim that pressure is "strictly positive"**: ASHRAE §1.2 does not use those words, but the physics is unambiguous. No physical site on Earth has zero or negative absolute pressure.

**Citation 2**: "EnergyPlus Engineering Reference §1 'Site:Location' — site pressure derived from elevation when not explicitly provided; never zero or negative"

- **Source found**: EnergyPlus Auxiliary Programs 23.2 (`bigladdersoftware.com/epx/docs/23-2/auxiliary-programs/energyplus-weather-file-epw-data-dictionary.html`); EnergyPlus Engineering Reference 9.5 climate calculations; Unmet Hours Q&A on site air density.
- **Quoted passage**: "The `Site:Location` input object includes parameters (Latitude, Longitude, Elevation, Timezone) that allow EnergyPlus to calculate the solar position … as well as supply the standard barometric pressure (using elevation)." Formula: `P = 101325 × (1.0 − Z × 0.0000225577)^5.2559` where Z = elevation in metres. From Unmet Hours: "R = 287.05 J/kg-K … T = 293.15 K … density = P / (R × T)."
- **Verdict**: **Confirmed in substance.** EnergyPlus does derive site pressure from elevation, the formula always produces a positive value for realistic elevations, and EnergyPlus never passes zero or negative pressure to density functions in normal simulation flow. The section reference "§1" is vague (EnergyPlus Engineering Reference has no simple §1 heading for Site:Location), but the claim is accurate.

**Citation 3 (implicit)**: EPW field 9 sentinel "999999" can result in `site_pressure_pa = 0` reaching `derive_zone_capacitances`

- **Source found**: EnergyPlus Auxiliary Programs 23.2, EPW Data Dictionary.
- **Quoted passage**: "This is the station pressure in Pa at the time indicated. Valid values range from 31,000 to 120,000. … Missing value for this field is 999999."
- **Verdict**: **Incorrect as a pathway to the bug.** The EPW parser at `crates/hares-io/src/epw.rs:160-165` validates pressure in the range `[60, 110] kPa` and returns a hard `WeatherError::Validation` if out of range. A sentinel `999999 Pa (~999 kPa)` would be caught and rejected there — it cannot reach `derive_zone_capacitances`. The realistic zero/invalid-pressure path is through ResStock CSV or callers that skip weather pressure.

**Project policy citation**: `feedback_no_silent_defaults.md`

- **Source found**: Referenced in 36 ticket and review files across the project (confirmed via codebase search).
- **Verdict**: **Confirmed** as a real project policy document referenced throughout the codebase.

### Legitimacy

- **Verdict**: **Legitimate** (with a minor inaccuracy on EPW sentinel pathway)

- **Rationale**: The core bug is real and confirmed: `derive_zone_capacitances` at `boundary_rc.rs:356-364` silently substitutes `AIR_DENSITY_KG_M3 = 1.2041` when `site_pressure_pa <= 0`, instead of returning an error. This is confirmed by code inspection and demonstrated by two regression tests that currently pass. OCHRE intentionally uses a hardcoded sea-level density for zone capacitance (a known OCHRE simplification that HARES is upgrading), so the fallback was deliberately modelled on OCHRE, but the ticket is correct that HARES should error rather than silently perpetuate that limitation. The EPW sentinel claim is slightly inaccurate — the EPW parser already hard-errors on out-of-range pressure before it could reach this function — but this does not undermine the core bug. The line-number citation ("362-375") is slightly off (the actual `else` branch is at lines 360-364; the function body spans 356-375) but the described code is present and correct. The ASHRAE formula citation uses ISA 1976 coefficients that are physically equivalent to the ASHRAE equation but from a different source. The ticket's proposed fix (add `BoundaryRcError::InvalidSitePressure` and return `Err`) is sound and consistent with project policy.

### Proposed Fix Summary

1. Add `BoundaryRcError` enum to `boundary_rc.rs` (or re-use an existing error type in the crate) with a variant `InvalidSitePressure { value: f64 }`.
2. Change `derive_zone_capacitances` signature from `-> Vec<f64>` to `-> Result<Vec<f64>, BoundaryRcError>`.
3. Replace the `else` branch (lines 360-364) with `return Err(BoundaryRcError::InvalidSitePressure { value: site_pressure_pa })`.
4. Update all callers (currently ~35 call sites in `boundary_rc.rs` tests and crate integration tests) to handle the `Result`.
5. Remove `zone_capacitance_zero_pressure_uses_fallback` (documents the now-fixed bug) and update the two ticket-103 regression tests to assert the `Err` path.
6. Do NOT remove the `AIR_DENSITY_KG_M3` constant — it is still used in BESTEST tests.

### Test Written

- **File**: `crates/hares-envelope/src/boundary_rc.rs` (within the existing `#[cfg(test)]` module at end of file)
- **Tests added**:
  - `ticket_103_zero_pressure_silently_uses_sea_level_density_not_an_error` — currently **passes** (demonstrates the bug: zero pressure silently produces sea-level-density capacitance instead of an error)
  - `ticket_103_negative_pressure_silently_uses_sea_level_density_not_an_error` — currently **passes** (same bug for negative input)
- **What they test**: Both tests call `derive_zone_capacitances` with an invalid pressure (0.0 and −1.0 Pa respectively) and assert that the *current wrong behavior* (returning sea-level capacitance) occurs. They will break (or must be rewritten) once the fix returns `Err(BoundaryRcError::InvalidSitePressure)` instead.
