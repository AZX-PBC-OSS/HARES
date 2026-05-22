# PSM3 and TMY3 Sky Temperature Bypasses Compute Path, Ignores Potential IR Column

**Severity**: Medium
**Impact on annual kWh**: Medium
**Status**: Open
**Areas**: hares-io/psm3, hares-io/tmy3

## Problem

Both parsers call `clark_allen_sky_temp_c` directly rather than routing through the shared `compute_sky_temp_c` function that selects between Stefan-Boltzmann inversion (when measured IR is available) and Clark-Allen fallback. This means:

1. If a future PSM3 file includes a longwave IR column (`Dhi_Modeled`, `Lwdown`, or `Radiation Modeled`), the data will be silently ignored and Clark-Allen will still be used — producing ±5–10 °C sky temperature error relative to the Stefan-Boltzmann inversion.
2. `horizontal_infrared_w_m2` is hardcoded to all-zeros for both formats, making the correct IR-present path permanently unreachable for these parsers.

Clark-Allen (1978) is a last-resort empirical correlation with no IR quality gating. EnergyPlus `CalcSkyTemp` (WeatherManager.cc) uses Stefan-Boltzmann inversion when measured horizontal IR ≥ 50 W/m², and falls back to Clark-Allen only when IR is absent. `compute_sky_temp_c` in `hares-io/src/epw.rs` implements this same branching. The parsers must route through it.

NSRDB PSM3 does not include `Opaque Sky Cover`; `opaque_sky_cover = 0.0` is therefore correct for the Walton cloud-cover correction path. The `Cloud Type` column present in some PSM3 product variants is currently unparsed — that is a separate issue.

## Current Behavior

`crates/hares-io/src/psm3.rs:245`: `clark_allen_sky_temp_c(db, dp)` called directly for every timestep.

`crates/hares-io/src/psm3.rs:270`: `horizontal_infrared_w_m2 = vec![0.0; n]` hardcoded.

`crates/hares-io/src/tmy3.rs:173`: same direct `clark_allen_sky_temp_c` call.

`crates/hares-io/src/tmy3.rs:190`: `horizontal_infrared_w_m2 = vec![0.0_f64; n]` hardcoded.

## Required Behavior

1. PSM3 parser must detect an optional longwave IR column (`Dhi_Modeled`, `Lwdown`, or `Radiation Modeled`, case-insensitive) in the column header map. When present, populate `horizontal_infrared_w_m2` from it. When absent, leave as 0.0.
2. Both parsers must call `compute_sky_temp_c(ir, db, dp, 0.0)` instead of `clark_allen_sky_temp_c(db, dp)`. When IR = 0.0, `compute_sky_temp_c` falls through to Clark-Allen — numerically identical to current behavior, but the routing is correct and future IR-present files activate the right physics path automatically.
3. No silent substitution: if a detected IR column contains non-finite values, return a parse error identifying the row and column.

Primary citation: EnergyPlus Engineering Reference §2.7.1 "Sky Radiation Modeling" — Stefan-Boltzmann inversion when IR ≥ 50 W/m², Clark-Allen (1978) as fallback.

Secondary citations:
- Clark, G. and Allen, C. (1978), "The Estimation of Atmospheric Radiation for Clear and Cloudy Skies", Proc. 2nd National Passive Solar Conference
- Berdahl, P. and Martin, M. (1984), "Emissivity of clear skies", Solar Energy, 32(5), 663–664
- ASHRAE Handbook of Fundamentals 2021 Ch. 14 §14.3 "Longwave Radiation"

## Approach

In `psm3.rs`: add `lwdown: Option<usize>` to `Psm3ColumnMap`. In the header-parsing loop, match column names case-insensitively against `dhi_modeled`, `lwdown`, `radiation modeled`. In the data loop, read the value when the index is present, fall back to 0.0 otherwise. Replace `clark_allen_sky_temp_c` call with `compute_sky_temp_c(ir, db, dp, 0.0)`.

In `tmy3.rs`: replace the direct `clark_allen_sky_temp_c` call with `compute_sky_temp_c(0.0, db, dp, 0.0)`.

## Definition of Done

- [ ] PSM3 parser calls `compute_sky_temp_c` for all timesteps
- [ ] TMY3 parser calls `compute_sky_temp_c` for all timesteps
- [ ] PSM3 parser optionally reads and populates `horizontal_infrared_w_m2` when an IR column is detected
- [ ] When IR = 0.0 (no column present), sky temperature output is numerically identical to current behavior
- [ ] Non-finite IR values produce a parse error with row and column identification

## Verification

```bash
cargo test -p hares-io psm3
cargo test -p hares-io tmy3
```

Tests must verify: (a) sky temp for a known db/dp pair matches `compute_sky_temp_c(0.0, db, dp, 0.0)` exactly, and (b) a synthetic PSM3 input with an `Lwdown` column produces sky temps matching `compute_sky_temp_c(lwdown_value, db, dp, 0.0)`.

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-20

### Code Confirmation
- [x] Referenced line numbers still match (or note corrected location)
  - `psm3.rs:245` — `clark_allen_sky_temp_c(db, dp)` confirmed at line 245.
  - `psm3.rs:270` — `horizontal_infrared_w_m2 = vec![0.0; n]` confirmed at line 270.
  - `tmy3.rs:173` — `clark_allen_sky_temp_c(db, dp)` confirmed at line 173.
  - `tmy3.rs:190` — `horizontal_infrared_w_m2 = vec![0.0_f64; n]` confirmed at line 190.
  - All four line numbers match exactly.
- [x] Described logic matches current implementation
  - Both parsers import and call `clark_allen_sky_temp_c` directly from `crate::epw`.
  - Neither parser imports `compute_sky_temp_c`.
  - `horizontal_infrared_w_m2` is hardcoded to zeros in both.
  - `Psm3ColumnMap` has no `lwdown` field; the `Lwdown` column is silently ignored.
- [x] OCHRE cross-check result: **diverges** — see below
  - `vendors/OCHRE/ochre/utils/schedule.py:182`: For EPW files, OCHRE computes
    sky temperature via Stefan-Boltzmann: `(df["ghi_infrared"].values / 5.6697e-8) ** 0.25`.
  - `vendors/OCHRE/ochre/utils/schedule.py:202`: For PSM3/CSV files, OCHRE sets
    `df["sky_temperature"] = np.nan` — **it does not call Clark-Allen at all**.
    The envelope code then falls back to ambient temperature when sky_temp is NaN
    (`Envelope.py:274-281`).
  - HARES diverges from OCHRE: HARES uses Clark-Allen fallback for PSM3 while
    OCHRE uses NaN (i.e. ignores sky radiation for PSM3). The ticket's approach
    (compute_sky_temp_c routing) is a reasonable and arguably correct improvement
    over both OCHRE's NaN and the current HARES Clark-Allen direct call.
- [x] EnergyPlus cross-check result: **partially matches** — see Web-Verified Citations below
  - The EnergyPlus Engineering Reference (verified via bigladdersoftware.com)
    confirms: `Tsky = (IRH/σ)^0.25 − 273.15` as the default sky temperature
    formula when horizontal IR data is present.
  - Clark-Allen formula (`ε_sky,clear = 0.787 + 0.764·ln(T_dp/273)`) is used by
    EnergyPlus for **computing horizontal IR from sky cover data** when IR is
    missing from the weather file — not as a direct sky-temp fallback. The ticket
    slightly mischaracterises EnergyPlus's use of Clark-Allen (see citation notes below).
  - The 50 W/m² threshold used in HARES `INFRARED_FALLBACK_THRESHOLD` (epw.rs:471)
    originates from HARES code commentary ("physically implausible for atmospheric
    downwelling longwave radiation") and is **not directly attributed to EnergyPlus
    source code** — web searches for this specific threshold in EnergyPlus
    WeatherManager.cc found no publicly visible confirmation. The threshold is
    physically reasonable (atmospheric downwelling LW is typically 200–450 W/m²).

### Web-Verified Citations

**Citation 1 — Primary: EnergyPlus Engineering Reference §2.7.1 "Sky Radiation Modeling"**
- **Source found**: EnergyPlus Engineering Reference, "Climate Calculations" section,
  BigLadder Software mirrors (versions 8.3–23.2 checked):
  https://bigladdersoftware.com/epx/docs/9-3/engineering-reference/climate-calculations.html
- **Quoted passage** (from EnergyPlus 9.3 Engineering Reference, "Sky Radiation Modeling"):
  > "EnergyPlus calculates the Horizontal Infrared Radiation Intensity (IRH), if it is
  > missing in the weather file or for design days, from the Dry Bulb Temperature, Dewpoint
  > Temperature or Partial Pressure of Water Vapor, and Opaque Cloud Cover as described below."
  >
  > Clark & Allen emissivity: `ε_sky = 0.787 + 0.764·ln(T_dp/273)`
  >
  > "EnergyPlus Sky Temperature Calculation: By default the Sky Temperature (Tsky) is
  > calculated from the Horizontal Infrared Radiation Intensity (IRH):
  > Tsky = (IRH/σ)^0.25 − 273.15"
- **Verdict**: **Partially correct**. The Stefan-Boltzmann inversion formula is confirmed.
  However, the section number "§2.7.1" is not verifiable from the online documentation
  (the online version has no numbered subsections). More importantly, in EnergyPlus,
  Clark-Allen is used to *compute the horizontal IR* from sky-cover data (when IR is
  absent from the EPW file), not as a direct sky-temperature fallback formula. The ticket
  conflates these two roles but the practical implication (Clark-Allen is used when IR is
  unavailable) is correct. No 50 W/m² threshold appears in any EnergyPlus documentation
  page found; the threshold in HARES is an internal design choice.

**Citation 2 — Clark, G. and Allen, C. (1978)**
- **Source found**: Multiple secondary confirmations via EnergyPlus Engineering Reference
  and academic surveys. E.g., Sky Temperature Estimation survey at
  https://publications.ibpsa.org/proceedings/bs/2017/papers/BS2017_569.pdf
- **Quoted passage**: EnergyPlus Engineering Reference reproduces the Clark & Allen formula:
  > `ε_sky,clear = 0.787 + 0.764·ln(T_dp/273)` (Clark & Allen 1978)
  The HARES implementation (epw.rs:519): `0.787 + 0.764 * (dew_point_k / KELVIN_OFFSET_C).ln()`
  matches this exactly (note: `dew_point_k / KELVIN_OFFSET_C = T_dp_K / 273.15 ≈ T_dp_K / 273`).
- **Verdict**: **Confirmed**. The formula coefficients 0.787 and 0.764 are correct.
  The paper citation (Proc. 2nd National Passive Solar Conference, 1978, pp. 675–678)
  is consistent with all secondary references found.

**Citation 3 — Berdahl, P. and Martin, M. (1984), Solar Energy, 32(5), 663–664**
- **Source found**: Semantic Scholar and ADS abstract:
  https://ui.adsabs.harvard.edu/abs/1984SoEn...32..663B/abstract
  https://www.sciepub.com/reference/37305
- **Quoted passage**: The paper "Emissivity of clear skies," *Solar Energy* vol. 32 no. 5
  pp. 663–664 (1984) by Berdahl and Martin is confirmed to exist and to deal with
  clear-sky emissivity. The ticket cites this as a secondary source alongside Clark-Allen.
  Note: the ticket distinguishes Berdahl & Martin (1984) from Martin & Berdahl (1984)
  Solar Energy 33(3/4) 321–336 — the former is a 2-page note, the latter is the full
  paper with the quadratic emissivity formula (0.758 + 0.521x + 0.625x²) used by
  EnergyPlus 9.6 and implemented in `berdahl_martin_sky_emissivity` in epw.rs:537.
  The short Berdahl & Martin note (32(5)) uses a simpler linear relation; the HARES code
  comment at epw.rs:529 correctly distinguishes these two papers.
- **Verdict**: **Confirmed** as an existing publication. The specific coefficients
  cited belong to the companion paper Martin & Berdahl 1984 (Solar Energy 33), not
  this 2-page note — both are correct supporting references.

**Citation 4 — ASHRAE Handbook of Fundamentals 2021 Ch. 14 §14.3 "Longwave Radiation"**
- **Source found**: ASHRAE official ToC:
  https://www.ashrae.org/technical-resources/ashrae-handbook/table-of-contents-2021-ashrae-handbook-fundamentals
  and ASHRAE handbook portal:
  https://handbook.ashrae.org/Handbooks/F17/IP/f17_ch14/f17_ch14_ip.aspx
- **Quoted passage**: The 2021 ASHRAE Handbook—Fundamentals Chapter 14 is titled
  "**Climatic Design Information**" and covers climatic design conditions, clear-sky
  solar radiation, degree-days, and related design data. It does **not** contain a
  section on longwave radiation or sky emissivity. Longwave radiation heat transfer
  is covered in Chapter 4 (Heat Transfer) and the sol-air temperature / outside
  surface heat balance discussion in Chapter 18 (Nonresidential Cooling and Heating
  Load Calculations).
- **Verdict**: **Incorrect**. The ticket's citation "ASHRAE HoF 2021 Ch. 14 §14.3
  'Longwave Radiation'" does not correspond to a real section. Chapter 14 is
  "Climatic Design Information" with no subsection on longwave radiation. The
  correct chapter for atmospheric radiation and sky temperature in ASHRAE Fundamentals
  would be Chapter 4 (Heat Transfer) or Chapter 18, depending on the edition.
  This citation should be removed or corrected; it does not undermine the core
  technical argument of the ticket, which is supported by the EnergyPlus and primary
  literature citations.

### Legitimacy
- **Verdict**: **Partially Legitimate**
- **Rationale**: The core bug is real and confirmed: both `psm3.rs:245` and `tmy3.rs:173`
  call `clark_allen_sky_temp_c` directly, bypassing `compute_sky_temp_c`. The
  `horizontal_infrared_w_m2` fields are hardcoded to zero in both parsers, making
  the Stefan-Boltzmann inversion path permanently unreachable for these formats.
  The regression test `psm3_lwdown_column_activates_stefan_boltzmann_path` was written
  and **fails on current code**, demonstrating an ~8.8 °C error (5.34 °C Clark-Allen
  vs −3.45 °C Stefan-Boltzmann for 300 W/m² IR). The OCHRE cross-check confirms
  PSM3 files have no standard longwave IR column, so this is a latent/future-facing
  issue rather than a currently active physics error for any real PSM3 file in
  circulation today. The ±5–10 °C error magnitude claim is confirmed for the case
  where a future IR-capable PSM3 file is used. However, two details need refinement:
  (1) the ASHRAE Ch. 14 §14.3 citation is incorrect (no such section exists — the
  citation should be removed or corrected to Ch. 4 or the EnergyPlus reference alone);
  (2) the claim that EnergyPlus `CalcSkyTemp` uses Clark-Allen as a fallback for
  IR ≥ 50 W/m² is a mischaracterisation — EnergyPlus uses Clark-Allen only to
  *compute horizontal IR from sky cover*, not as a direct sky-temp fallback. The
  routing fix through `compute_sky_temp_c` is still correct.

### Proposed Fix Summary
1. **psm3.rs**: Add `lwdown: Option<usize>` to `Psm3ColumnMap`. In
   `build_column_map`, match column names case-insensitively against
   `"dhi_modeled"`, `"lwdown"`, `"radiation modeled"`. In the data loop, read
   `lwdown_val` when the index is present (fail with `WeatherError::Validation`
   if non-finite), fall back to `0.0` otherwise. Replace `clark_allen_sky_temp_c`
   call with `compute_sky_temp_c(ir, db, dp, 0.0)`. Update the `use crate::epw`
   import to include `compute_sky_temp_c`.
2. **tmy3.rs**: Replace `clark_allen_sky_temp_c(db, dp)` with
   `compute_sky_temp_c(0.0, db, dp, 0.0)`. Update the `use crate::epw` import.
   No structural changes needed (TMY3 has no optional IR column).
3. Both parsers: the `horizontal_infrared_w_m2` zero-fill is correct when no IR
   column is present; PSM3 should populate it from the column when present.

### Test Written
- **File**: `crates/hares-io/src/psm3.rs` (within `#[cfg(test)] mod tests`)
  - `psm3_sky_temp_zero_ir_matches_clark_allen` — guards that
    `compute_sky_temp_c(0.0, db, dp, 0.0)` == `clark_allen_sky_temp_c(db, dp)`.
    **Currently passes** (numerical identity holds). Must continue to pass after fix.
  - `psm3_lwdown_column_activates_stefan_boltzmann_path` — synthetic PSM3 with
    `Lwdown=300 W/m²` column; asserts sky temp == Stefan-Boltzmann inversion and
    `horizontal_infrared_w_m2[0]` == 300.0. **Currently FAILS** (bug confirmed).
    Must pass after fix.
- **File**: `crates/hares-io/src/tmy3.rs` (within `#[cfg(test)] mod tests`)
  - `tmy3_sky_temp_zero_ir_matches_clark_allen` — same numerical identity guard.
    **Currently passes**.
  - `tmy3_sky_temp_routes_through_compute_sky_temp_c` — verifies the equivalence
    of `compute_sky_temp_c(0.0, …)` and `clark_allen`. **Currently passes**.
