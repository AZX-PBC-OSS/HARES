# Kusuda-Achenbach Ground Model: Amplitude Fallback Uses Instantaneous Extremes, Not Monthly Means

**Severity**: Medium
**Status**: Open
**Areas**: hares-core/environment

## Problem

`compute_mains_inputs` at `crates/hares-core/src/environment.rs:683` derives the Kusuda-Achenbach
amplitude parameter by computing the range of monthly mean dry-bulb temperatures. Two defects
corrupt this derivation.

**Defect 1 — hardcoded non-leap February.**
At `environment.rs:694`, month day counts are hardcoded:
```
let month_days = [31usize, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
```
For a leap-year weather file (8784 hourly records), the February slice is one day short (28 × 24 =
672 samples instead of 696), causing February to be computed from 28 days of data while the
remaining 29 days are silently absorbed into the March slice. The February monthly mean is wrong and
the March mean is wrong, producing a corrupted `mains_dt_annual_range_c`.

**Defect 2 — fallback uses instantaneous hourly extremes.**
At `environment.rs:696–697`, when `temps.len() < year_samples`:
```
return (annual_avg_c, simple_range(temps));
```
`simple_range` returns the difference between the minimum and maximum value across all hourly
records (`environment.rs:722–728`). The Kusuda-Achenbach `t_amplitude_c` is defined as half the
range of monthly mean temperatures — not half the range of instantaneous hourly values. Per Kusuda
and Achenbach (1965) ASHRAE Transactions 71(1):61-74 and EnergyPlus Engineering Reference §3.1,
`T_amplitude = (T_max_monthly_mean - T_min_monthly_mean) / 2`. Instantaneous hourly extremes are
typically 5–15 °C wider than the monthly-mean range. The fallback path therefore over-estimates the
Kusuda amplitude by 30–70%, producing ground temperature swings that are far too large.

Note: this defect only affects the simulation once ticket 055 is resolved and
`kusuda_achenbach_temp` is called from the thermal solver hot path. The parameter derivation is
currently moot at runtime.

## Current Behavior

`crates/hares-core/src/environment.rs:694`:
```
let month_days = [31usize, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
```
Fixed non-leap February regardless of weather file length.

`crates/hares-core/src/environment.rs:696–697`:
```
if temps.len() < year_samples {
    return (annual_avg_c, simple_range(temps));
```
Returns instantaneous min/max range when fewer than 8760 hourly records are present.

## Required Behavior

1. Detect leap year from `weather.dry_bulb_c.len()`: if `len == 8784 * samples_per_hour` (or
   equivalently, if `len % (366 * samples_per_day) == 0` and `len / samples_per_day == 366`), use
   February day count of 29. Per EnergyPlus Engineering Reference §3.1, annual period is 365 or
   366 days depending on the weather file.
2. When `temps.len() < year_samples` (short or synthetic weather), derive the amplitude from the
   range of whatever monthly means can be computed from the available data, not from instantaneous
   extremes. Specifically: compute `month_means` for the months fully covered by `temps`, then
   apply `simple_range(&month_means)`. If fewer than 2 months of data are present, use 0.0 as
   the amplitude (constant ground temperature equal to mean), which is physically conservative and
   matches the synthetic weather case.

## Approach

1. In `compute_mains_inputs`, after computing `samples_per_day`, derive `is_leap` by checking
   whether `temps.len()` equals `366 * samples_per_day`. Pass `is_leap` to a `month_day_counts`
   helper (the equivalent already exists in `crates/hares-io/src/epw.rs:381` as
   `monthly_day_counts`; extract to a shared location or duplicate with a comment).
2. Replace the early-return fallback at line 696 with a partial-month-means approach: accumulate
   `month_means` for months where `cursor + month_samples <= temps.len()`, then call
   `simple_range(&month_means)` if `month_means.len() >= 2`, else use `0.0`.
3. Remove the `simple_range(temps)` call entirely from `compute_mains_inputs`; it must not be
   reachable after this change.

## Definition of Done

- `compute_mains_inputs` uses 29-day February when `temps.len()` corresponds to a leap year.
- The short-weather fallback path never calls `simple_range` on the raw `temps` slice.
- Test: a leap-year weather series (8784 hourly records) produces a February mean computed from
  29 days of data.
- Test: a weather series with only 2 months of data produces amplitude = `simple_range` of those
  2 monthly means, not the hourly min/max range.

## Verification

```
cargo test -p hares-core environment
cargo test -p hares-physics ground
```

The environment test suite must include:
- A synthetic 8784-record series confirming 29-day February averaging.
- A 48-record series (2 days) confirming amplitude is derived from available monthly means,
  not from `simple_range` of all 48 hourly values.

## References

- Kusuda, T. and Achenbach, P.R. (1965), ASHRAE Transactions Vol. 71(1), pp. 61-74 —
  `T_amplitude = (T_max_monthly_mean - T_min_monthly_mean) / 2`
- Burch, J. and Christensen, C. (2007), NREL/CP-550-41263 — monthly-mean range for `ΔT`
- EnergyPlus Engineering Reference §3.1 "Ground Heat Transfer" — `T_amplitude` from monthly-mean
  temperature extremes, annual period 365 or 366 days

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match (confirmed: `compute_mains_inputs` at line 683;
  `month_days` hardcoded at line 694; early-return `simple_range(temps)` at lines 696–697;
  `simple_range` definition at lines 722–728)
- [x] Described logic matches current implementation — both defects are present exactly as
  described; the `month_days` array is `[31usize, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]`
  with no leap-year detection; the fallback at line 697 calls `simple_range(temps)` on the raw
  hourly slice
- [x] OCHRE cross-check result: **diverges from OCHRE (in the fallback path); OCHRE never uses
  instantaneous extremes**. `vendors/OCHRE/ochre/utils/schedule.py` line 171–173 removes Feb 29
  from leap-year EPW files rather than using a 29-day slice; line 227–228 computes:
  ```python
  t_monthly_avg = t_amb.groupby(df.index.month).mean()
  dt_monthly = (t_monthly_avg.max() - t_monthly_avg.min()) / 2
  ```
  OCHRE always derives the amplitude from monthly means, never from instantaneous extremes.
  HARES has the correct intent (monthly means in the normal path) but the leap-year count is
  wrong (Defect 1) and the fallback path uses instantaneous extremes (Defect 2). Note OCHRE
  skips the fallback entirely (line 223: only computes mains/ground temp when all 12 months
  are present); HARES's fallback path has no OCHRE equivalent.
- [x] EnergyPlus cross-check result: **confirms monthly-mean amplitude; τ = 365 only**.
  EnergyPlus Engineering Reference v22.2, Water Systems chapter
  (https://bigladdersoftware.com/epx/docs/22-2/engineering-reference/water-systems.html) states:
  > "ΔTout,maxdiff is the maximum difference in monthly average outdoor air temperatures (°F)"
  in the Burch-Christensen mains water correlation. The Kusuda-Achenbach model documentation
  (v8.4–v22.1, https://bigladdersoftware.com/epx/docs/22-1/engineering-reference/undisturbed-ground-temperature-model-kusuda.html)
  specifies `τ = 365` with no mention of 366 for leap years. The amplitude is unambiguously
  from monthly means, not instantaneous hourly extremes. The ticket's claim that EnergyPlus
  supports "365 or 366 days" is not supported by any fetched EnergyPlus page.

### Web-Verified Citations

**Citation 1**: Kusuda, T. and Achenbach, P.R. (1965), ASHRAE Transactions Vol. 71(1), pp. 61-74

- **Sources found**:
  - NIST Publications page — https://www.nist.gov/publications/earth-temperature-and-thermal-diffusivity-selected-stations-united-states
  - EnergyPlus Engineering Reference v8.4 and v9.3 (Big Ladder Software) —
    https://bigladdersoftware.com/epx/docs/8-4/engineering-reference/undisturbed-ground-temperature-model-kusuda.html
  - Semantic Scholar — https://www.semanticscholar.org/paper/EARTH-TEMPERATURE-AND-THERMAL-DIFFUSIVITY-AT-IN-THE-Kusuda-Achenbach/fe1b3ec9c47d2bc09059f6aea282f8cd55d77064
- **Quoted passage** (NIST): "Kusuda, T. and Achenbach, P. (1965), Earth temperature and
  thermal diffusivity at selected stations in the United States, NBS report 8972,
  https://doi.org/10.6028/NBS.RPT.8972"
- **Quoted passage** (EnergyPlus v8.4 Engineering Reference): "Kusuda, T. and P.R. Achenbach.
  1965. 'Earth Temperatures and Thermal Diffusivity at Selected Stations in the United States.'
  ASHRAE Transactions. 71(1): 61-74."
- **Verdict**: **Confirmed**. The work was published in dual venues: it appears in ASHRAE
  Transactions 71(1):61-74 (as cited by EnergyPlus and the ticket) **and** simultaneously as
  NBS Report 8972 (a US government technical report). EnergyPlus explicitly cites the ASHRAE
  Transactions version with the volume/issue/page numbers that match the ticket. The previous
  audit's "partially correct" verdict was too conservative — the ASHRAE citation is correct.

**Citation 2**: Burch, J. and Christensen, C. (2007), NREL/CP-550-41263

- **Source found**: OSTI entry — https://www.osti.gov/biblio/981988
- **Quoted passage** (OSTI): "Title: Towards Development of an Algorithm for Mains Water
  Temperature. Authors: Burch, J and Christensen, C. Year: 2007. Publisher: American Solar
  Energy Society (ASES), Boulder, CO. OSTI ID: 981988."
- **Verdict**: **Partially correct**. The paper is real (Burch and Christensen, 2007, ASES
  conference). The OSTI record does not list the report number "NREL/CP-550-41263"; this
  number circulates in EnergyPlus source and related literature as the NREL internal conference
  paper number and cannot be independently confirmed from the OSTI record alone. The core
  claim — that the amplitude parameter `ΔT` is the range of **monthly mean** temperatures
  (not instantaneous extremes) — is confirmed by EnergyPlus Water Systems documentation (see
  Citation 3) and by OCHRE's direct implementation.

**Citation 3**: EnergyPlus Engineering Reference §3.1 "Ground Heat Transfer" — `T_amplitude`
from monthly-mean temperature extremes, annual period 365 or 366 days

- **Sources found**:
  - EnergyPlus Engineering Reference v22.2, Water Systems —
    https://bigladdersoftware.com/epx/docs/22-2/engineering-reference/water-systems.html
  - EnergyPlus Engineering Reference v22.1, Kusuda-Achenbach model —
    https://bigladdersoftware.com/epx/docs/22-1/engineering-reference/undisturbed-ground-temperature-model-kusuda.html
- **Quoted passage** (Water Systems, v22.2): "ΔTout,maxdiff is the maximum difference in
  monthly average outdoor air temperatures (°F)" and the formula
  "Tmains = (Tout,avg + 6) + ratio × (ΔTout,maxdiff / 2) × SIN(0.986 × (day − 15 − lag) − 90)"
- **Quoted passage** (Kusuda-Achenbach, v22.1): "τ is time constant, 365."
- **Verdict**: **Partially correct — two errors in the section reference and the τ claim**.
  (a) The section reference "§3.1 Ground Heat Transfer" is incorrect. EnergyPlus organizes
  the Burch-Christensen mains formula under "Water Systems" and the Kusuda-Achenbach model
  under "Undisturbed Ground Temperature Model: Kusuda-Achenbach" — no "§3.1 Ground Heat
  Transfer" section was found in any fetched version. (b) The claim "annual period 365 or
  366 days" is unsupported: EnergyPlus specifies `τ = 365` with no leap-year branch in any
  fetched version (v8.4 through v22.1). The core claim that the amplitude is derived from
  monthly-mean temperature extremes is confirmed.

### Legitimacy

- **Verdict**: **Legitimate** (with minor citation inaccuracies that do not affect the bugs)
- **Rationale**: Both defects are present in the current code exactly as described. Defect 1
  (hardcoded 28-day February) is confirmed at `environment.rs:694` — the `month_days` array
  uses `28` regardless of whether the weather series has 8760 or 8784 records. A
  `monthly_day_counts` helper that handles leap years already exists at `hares-io/src/epw.rs:381`
  and is not used here. Defect 2 (fallback uses instantaneous extremes) is confirmed at
  `environment.rs:696–697` — `simple_range(temps)` operates on the entire raw hourly slice.
  EnergyPlus Engineering Reference v22.2 Water Systems explicitly defines the mains amplitude
  parameter as "the maximum difference in **monthly average** outdoor air temperatures",
  confirming Defect 2 is a real physics bug. OCHRE (`schedule.py:227–228`) independently
  confirms this: it computes `dt_monthly = (t_monthly_avg.max() - t_monthly_avg.min()) / 2`
  from monthly groupby means and never uses instantaneous extremes. The two regression tests
  are syntactically correct and demonstrate the failures — they cannot currently be executed
  because of a pre-existing, unrelated compile error in `synthetic.rs:1151` (`ground_temp_c`
  field absent from `SyntheticWeatherConfig`). Their math is independently verified: the
  buggy March mean of 96.774°C = (30×24×100) / (31×24) is arithmetically correct. Citation
  inaccuracies (section reference §3.1; τ claim of "365 or 366"; NREL report number not found
  in OSTI record) are minor and do not affect the validity of the bug reports.

### Proposed Fix Summary

1. **Defect 1**: Replace the hardcoded `month_days` array at line 694 with a call to
   `hares_io::epw::monthly_day_counts(temps.len() == 366 * samples_per_day)` (the helper
   already exists at `hares-io/src/epw.rs:381`). The visibility of that function may need to
   be widened from `pub(crate)` to `pub` or relocated to a shared crate.
2. **Defect 2**: Replace the early-return `simple_range(temps)` at line 697 with a
   partial-monthly-means approach: accumulate month means only for months fully covered by
   the available data (`cursor + month_samples <= temps.len()`), then return
   `simple_range(&month_means)` if `month_means.len() >= 2`, else `0.0`. Remove the
   `simple_range(temps)` call entirely from the short-weather branch of `compute_mains_inputs`.

### Test Written

- **File**: `crates/hares-core/src/environment.rs` (inline `#[cfg(test)] mod tests` block,
  lines 1919–2120, added in a prior audit pass)
- **Execution status**: Tests cannot run currently due to a pre-existing compile error in
  `crates/hares-core/src/dwelling/synthetic.rs:1151` (`ground_temp_c` field missing from
  `SyntheticWeatherConfig`). This is unrelated to ticket 034.
- **Tests**:
  - `leap_year_february_uses_29_days_for_monthly_mean` (line 1943) — expected to FAIL with
    current code (Defect 1): fixture sets March to 100°C in a 8784-hour series; buggy code
    returns range ≈ 96.77°C because one Feb-29 zero-degree day leaks into March's slice,
    lowering its mean from 100.0 to (30×100)/31 ≈ 96.77; correct code must return 100.0.
  - `short_weather_fallback_uses_monthly_means_not_instantaneous_extremes` (line 2070) —
    expected to FAIL with current code (Defect 2): 48-hour alternating −20°C/+20°C series
    gives `simple_range(temps) = 40.0`; correct code must return 0.0 (no full month available).
