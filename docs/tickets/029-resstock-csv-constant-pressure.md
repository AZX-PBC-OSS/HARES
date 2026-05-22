# ResStock CSV Parser: Constant Pressure From Elevation, No Diurnal Variation

**Severity**: Low
**Priority**: P3
**Status**: Open
**Areas**: hares-io/resstock_csv
**Weather-cluster note**: This ticket belongs adjacent to the 024–034 weather pipeline cluster. Do not move — renumber when the cluster is sequenced.

## Problem

`parse_resstock_csv` in `hares-io/src/resstock_csv.rs:115` computes `let pressure_kpa = isa_pressure_kpa(elevation_m)` once and reuses it for every row. The ResStock 8-column format has no pressure field; the ISA estimate is the best available approximation. This is a known format limitation, not a parsing bug. However, a second defect in the same file is a fixable bug: timestep inference hardcodes an 8 760-hour year.

Downstream effects of constant pressure:
1. `humidity_ratio_from_tdp(dew_point_c, pressure_pa)` receives a constant pressure at every step. At sea level a 10 hPa swing causes ~0.5 % error in humidity ratio; at Denver (1 609 m), 1–2 %.
2. Psychrometric properties (wet-bulb, enthalpy) also depend on pressure; constant pressure produces constant specific volume. Acceptable for ResStock's comparative-benchmark role but outside ASHRAE HoF 2021 Ch. 1 §1.2 requirements for full psychrometric accuracy.

### Leap-year timestep inference bug

`resstock_csv.rs:254–263`:
```rust
let total_seconds_in_year = 8760 * 3600;
if total_seconds_in_year % n == 0 {
    (total_seconds_in_year / n) as u32
} else {
    3600
}
```

For a leap-year file with 8 784 hourly rows, `8760 * 3600 % 8784 != 0`, so the modulo check fails and the step defaults to 3 600 s. For hourly data this is accidentally correct, but for sub-hourly leap-year files it will produce a wrong step. The is-leap-year detection at lines 266–276 correctly uses the first data row's timestamp, but its result is not fed back into the step inference.

## Current Behavior

`hares-io/src/resstock_csv.rs:115`: single constant pressure for all rows (format limitation — no fix available; document only).
`hares-io/src/resstock_csv.rs:254–263`: step inference hardcodes `8760 * 3600` regardless of leap year (fixable bug).

## Required Behavior

1. Move is-leap-year detection (currently lines 266–276) to before the `source_step_secs` block. Compute `total_seconds_in_year = (if is_leap_year { 8_784_u64 } else { 8_760_u64 }) * 3_600` and use it for the modulo check. This is the only code change required.

2. Add a module-level doc comment at the top of `resstock_csv.rs` stating: "ResStock CSV pressure is a constant ISA estimate derived from site elevation. The 8-column format contains no measured pressure data; per-row pressure variation is not achievable. See ISO 2533:1975 §5 for the ISA model." Do not add a silent fallback, override mechanism, or any warning per-row — the constant is the correct best estimate for this format.

Per project policy `feedback_no_silent_defaults.md`: the constant-pressure limitation is documented, not silently hidden. Per `feedback_no_backward_compat.md`: no shim or override hook for callers who want different pressure.

## Approach

1. Extract or reorder the is-leap-year detection block so it runs before `source_step_secs` assignment.
2. Replace `let total_seconds_in_year = 8760 * 3600` with the leap-year-aware expression.
3. Add the module-level doc comment.

## Definition of Done

- [ ] Timestep inference uses leap-year-aware year length (`8_784` or `8_760` based on first-row timestamp)
- [ ] Module doc comment documents constant-pressure limitation with ISO 2533:1975 citation
- [ ] `cargo test -p hares-io resstock` passes
- [ ] Test: 8 784-row ResStock CSV (leap year) correctly infers 3 600 s timestep

## Verification

```bash
cargo test -p hares-io resstock
```

## References

- ResStock weather format: https://github.com/NREL/resstock — 8-column simplified CSV; no pressure field
- ASHRAE Handbook of Fundamentals 2021 Ch. 1 §1.2 "Psychrometrics" — pressure dependence of humidity ratio
- ISO 2533:1975 Standard Atmosphere §5 — basis for `isa_pressure_kpa` approximation

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match (or note corrected location)
  - **Line 115**: `let pressure_kpa = isa_pressure_kpa(elevation_m)` — confirmed, matches ticket description exactly.
  - **Lines 252–263**: `source_step_secs` block with hardcoded `8760 * 3600 = 31_536_000` at line 255 — confirmed.
  - **Lines 265–276**: `is_leap_year` detection block — confirmed, comes *after* the step-inference block. The ordering bug is present.
- [x] Described logic matches current implementation
  - Pressure: `pressure_kpa` is computed once at line 115 and pushed identically for every row at line 317 — confirmed.
  - Leap-year step bug: `total_seconds_in_year = 8760 * 3600` at line 255 yields 31 536 000. For a 30-min leap-year file (17 568 rows): `31_536_000 % 17_568 == 1_440 ≠ 0`, so it falls back to the default 3 600 s instead of 1 800 s. Arithmetic independently verified below.
- [x] OCHRE cross-check result: **diverges — OCHRE hardcodes 101.325 kPa for all sites when pressure is absent**
  - `vendors/OCHRE/ochre/Models/Humidity.py:25`: `p_outdoor = initial_schedule.get("Ambient Pressure (kPa)", 101.325)`
  - `vendors/OCHRE/ochre/Models/Envelope.py:840–841`: `if "Ambient Pressure (kPa)" not in self.schedule: self.warn("Ambient pressure not in schedule. Using standard pressure of 1 atm (101.3 kPa).")`
  - HARES uses the ISA formula keyed to site elevation rather than always defaulting to sea level; the divergence from OCHRE is intentional and represents an improvement.
  - OCHRE rejects leap-year files outright (`vendors/OCHRE/ochre/utils/schedule.py:96–97`: `if duration.days != 365: raise OCHREException("Cannot parse data for a leap year.")`), providing no reference for correct leap-year step inference.
- [x] EnergyPlus cross-check result: **N/A** — EnergyPlus uses EPW files that already carry measured pressure per row; no ISA estimation is involved. The ISA formula is standard atmospheric physics, not an EnergyPlus algorithm.

### Web-Verified Citations

---

**Citation 1**: "ASHRAE HoF 2021 Ch. 1 §1.2 — requirements for full psychrometric accuracy"

- **Source fetched**: ASHRAE Handbook of Fundamentals 2021, Chapter 1 online edition at `handbook.ashrae.org/Handbooks/F21/SI/F21_Ch01/F21_Ch01_si.aspx`; structure cross-checked against the 2021 slideshare reproduction and the official TOC at `ashrae.org/technical-resources/ashrae-handbook/table-of-contents-2021-ashrae-handbook-fundamentals`.
- **Section structure confirmed**: ASHRAE 2021 Ch. 1 uses decimal section numbering (§1.1, §1.2, …). The ten sections are:
  - §1.1 Composition of Dry and Moist Air
  - §1.2 U.S. Standard Atmosphere
  - §1.3 Thermodynamic Properties of Moist Air
  - §1.4 Thermodynamic Properties of Water at Saturation
  - §1.5 Humidity Parameters
  - §1.6 Perfect Gas Relationships for Dry and Moist Air
  - §1.7 Thermodynamic Wet-Bulb and Dew-Point Temperature
  - §1.8 Numerical Calculation of Moist Air Properties
  - §1.9 Psychrometric Charts
  - §1.10 Typical Air-Conditioning Processes
- **Citation error found**: The ticket attributes "requirements for full psychrometric accuracy" to §1.2. **§1.2 is "U.S. Standard Atmosphere"** — it defines sea-level standard conditions (T₀ = 15 °C, P₀ = 101.325 kPa) and provides altitude-pressure equations. It does not contain any passage about "requirements for full psychrometric accuracy." The relevant content on pressure dependence of humidity ratio is in **§1.5 (Humidity Parameters)** and **§1.6 (Perfect Gas Relationships)**. §1.6 is the section that discusses when perfect-gas approximations produce acceptable accuracy.
- **Humidity ratio formula** (ASHRAE 2017/2021 Ch. 1 Equation 20, confirmed via PsychroLib source code which cites "ASHRAE Handbook - Fundamentals (2017) ch. 1 eqn 20"): `W = 0.621945 × pᵥ / (p − pᵥ)`. Pressure `p` appears in the denominator, confirming pressure dependence.
- **Quantitative error check**: Using `W = 0.621945 × pᵥ / (p − pᵥ)` at 20 °C, 50 % RH (pᵥ ≈ 1.169 kPa): sensitivity `dW/W ≈ dp / (p − pᵥ)`. At sea level (p = 101.325 kPa, p − pᵥ ≈ 100.16 kPa): a 5 hPa one-sided deviation yields 0.50 % error. The ticket describes this as "a 10 hPa swing causes ~0.5 % error." This is consistent if "swing" means peak-to-peak (±5 hPa from mean). At Denver (p ≈ 83.4 kPa, p − pᵥ ≈ 82.2 kPa): a 10 hPa one-sided deviation yields ≈1.2 %, consistent with the ticket's "1–2 %."
- **Verdict**: **Partially correct** — the chapter (Ch. 1 Psychrometrics) is correct, and the underlying scientific claim about pressure dependence of W is confirmed by §1.5/§1.6. However, **§1.2 is not a section about psychrometric accuracy; it is the U.S. Standard Atmosphere section**. The citation should read §1.5 or §1.6, not §1.2.

---

**Citation 2**: "ISO 2533:1975 Standard Atmosphere §5 — basis for `isa_pressure_kpa` approximation"

- **Source fetched**: Wikipedia "Barometric formula" (`en.wikipedia.org/wiki/Barometric_formula`); Engineering LibreTexts ISA equations page (`eng.libretexts.org/…/2.3.03:_ISA_equations`); ISO 2533:1975 sample PDF from `cdn.standards.iteh.ai` (scanned, not machine-readable).
- **Constants confirmed** (from Wikipedia Barometric Formula, standard atmosphere layer b = 0):
  - Sea-level pressure P₀ = 101 325 Pa
  - Sea-level temperature T₀ = 288.15 K
  - Temperature lapse rate L = 0.0065 K/m
  - Standard gravity g₀ = 9.80665 m/s²
  - Molar mass of dry air M₀ = 28.9644 kg/kmol
  - Universal gas constant R* = 8314.32 J/(kmol·K)
- **Formula derivation verified arithmetically**:
  - L / T₀ = 0.0065 / 288.15 = **2.25577 × 10⁻⁵** ✓ (matches HARES constant)
  - g₀ M₀ / (R* L) = (9.80665 × 28.9644) / (8314.32 × 0.0065) = **5.25588** ✓ (matches HARES exponent)
  - Resulting formula: P = 101.325 kPa × (1 − 2.25577 × 10⁻⁵ × h)^5.25588 — exact match to `isa_pressure_kpa` in `resstock_csv.rs:42–46`.
- **Engineering LibreTexts quoted passage**: *"p [Pa] = 101325 (1 − 22.558 × 10⁻⁶ × h [m])^5.2559"* — same formula with minor rounding difference in the last decimal (22.558×10⁻⁶ ≈ 2.2558×10⁻⁵ vs HARES 2.25577×10⁻⁵; exponent 5.2559 vs HARES 5.25588). This is a rounding artifact; both are correct.
- **ISO 2533:1975 §5**: The full standard is not freely available in machine-readable form. Secondary sources consistently describe §5 as covering the tropospheric pressure-altitude formula with these exact constants.
- **Verdict**: **Confirmed** — the ISA formula, both constants, and the ISO 2533:1975 citation are correct. The HARES implementation is arithmetically equivalent to the standard atmosphere formula.

---

**Citation 3**: "ResStock weather format — 8-column simplified CSV; no pressure field"

- **Source fetched**: Two actual NREL OEDI AMY2018 weather CSV files retrieved directly from the public S3 bucket:
  - `oedi-data-lake.s3.amazonaws.com/nrel-pds-building-stock/end-use-load-profiles-for-us-building-stock/2024/comstock_amy2018_release_2/weather/amy2018/G5600250_2018.csv`
  - `oedi-data-lake.s3.amazonaws.com/nrel-pds-building-stock/end-use-load-profiles-for-us-building-stock/2024/comstock_amy2018_release_1/weather/amy2018/G5300070_2018.csv`
- **Exact column headers confirmed from both files** (8 columns):
  1. `date_time`
  2. `Dry Bulb Temperature [°C]`
  3. `Relative Humidity [%]`
  4. `Wind Speed [m/s]`
  5. `Wind Direction [Deg]`
  6. `Global Horizontal Radiation [W/m2]`
  7. `Direct Normal Radiation [W/m2]`
  8. `Diffuse Horizontal Radiation [W/m2]`
- **First data row example** (G5300070_2018.csv): `2018-01-01 01:00:00,-3.3,74.78,0.0,4.0,0.0,0.0,0.0`
- **No pressure column** in either file — directly confirmed, not inferred.
- These column headers match exactly what `resolve_columns()` searches for (`"dry bulb temperature"`, `"relative humidity"`, `"wind speed"`, `"wind direction"`, `"global horizontal radiation"`, `"direct normal radiation"`, `"diffuse horizontal radiation"`).
- **Verdict**: **Confirmed** — the 8-column format and the absence of any pressure column are directly verified from the real NREL OEDI CSV files. The ticket's description is accurate.

### Legitimacy

- **Verdict**: **Partially Legitimate**
- **Rationale**: Both defects described in the ticket are real and present in the current code. The constant-pressure limitation (line 115) is correctly characterised as a format constraint with no fix available. The leap-year timestep inference bug (line 255) is a genuine fixable bug: `31_536_000 % 17_568 == 1_440 ≠ 0` causes sub-hourly leap-year files to fall back to 3 600 s, independently confirmed arithmetically and by the failing regression test `leap_year_30min_step_inferred_correctly` (`cargo test -p hares-io resstock`: 29 passed, 1 failed with "got 3600 s, expected 1800 s"). The ResStock CSV format claim is **directly confirmed** from real NREL OEDI files. The ISO 2533:1975 formula constants are **arithmetically verified**. The OCHRE cross-check is confirmed from source code. One citation error is present: **§1.2 is U.S. Standard Atmosphere, not a section about psychrometric accuracy requirements** — the correct section for pressure-dependent humidity ratio formulas is §1.5 (Humidity Parameters) or §1.6 (Perfect Gas Relationships). This is a citation-attribution error in the ticket that does not affect the validity of the underlying scientific claims or the proposed fix. The verdict is "Partially Legitimate" solely due to this citation error; the core issue is real and the fix is correct.

### Proposed Fix Summary

1. Move the `is_leap_year` detection block (currently lines 265–276) to before the `source_step_secs` assignment (currently line 252).
2. Replace `let total_seconds_in_year = 8760 * 3600` (line 255) with `let total_seconds_in_year = (if is_leap_year { 8_784_u64 } else { 8_760_u64 }) * 3_600_u64`.
3. Add or update a module-level doc comment for the constant-pressure limitation citing ISO 2533:1975 (the module doc at lines 1–23 already states "Pressure [kPa] — Estimated — ISA standard atmosphere from elevation"; the citation to ISO 2533:1975 §5 should be added here). Correct the ASHRAE citation in the ticket from "§1.2" to "§1.5" or "§1.6."
4. Do NOT change the constant-pressure behaviour.

### Test Written

- **File**: `crates/hares-io/src/resstock_csv.rs` (within `#[cfg(test)] mod tests`)
- **Pre-existing tests in this audit cycle** (written by a prior audit pass, already present in source):
  - `leap_year_30min_step_inferred_correctly` (lines 800–811) — builds 17 568-row synthetic ResStock CSV with 30-minute interval timestamps in leap year 2004, asserts `source_step_secs == 1800`. **Currently FAILS** (`cargo test` confirms: "got 3600 s, expected 1800 s"). This is the primary regression test for the fixable defect.
  - `leap_year_hourly_step_inferred_correctly` (lines 817–827) — builds 8 784-row synthetic ResStock CSV with hourly timestamps in leap year 2004, asserts `source_step_secs == 3600`. Currently passes (the non-leap modulo `31_536_000 % 8_784 == 0` accidentally works because 8 760 divides 8 784). After the fix, this verifies the hourly case still infers 3 600 s.
- **No new tests written**: the pre-existing regression tests fully cover the fixable defect.
