# DEG_PER_DAY=0.986 source verification (Burch-Christensen 2007) and divergence analysis from 360/365.25=0.9856
**Review ID**: wm-01
**Category**: water-mains
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/water_mains.rs:16`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/utils/schedule.py:239`
- `vendors/EnergyPlus/src/EnergyPlus/WeatherManager.cc:7204`
- `vendors/EnergyPlus/doc/engineering-reference/src/simulation-models-encyclopedic-reference-005/water-systems.tex:28`

## Findings

### Finding 1: [Severity: low] HARES correctly matches the canonical 0.986 value used by both EnergyPlus and OCHRE

**Description**: The `DEG_PER_DAY` constant is set to `0.986` in HARES, matching the literal value hardcoded in EnergyPlus `WeatherManager.cc:7204` and OCHRE `schedule.py:239`. All three implementations agree. The comment at `water_mains.rs:14-15` correctly notes that this value is "not derived from 360/365.25 (≈ 0.9856)" and comes directly from the paper.

**Code Location**: `crates/hares-physics/src/water_mains.rs:16`

**Root Cause**: The constant `0.986` originates from the Hendron et al. (2004) SimBuild paper describing the correlation developed by Burch & Christensen, and is carried forward in the Burch-Christensen (2007) ASES paper. It likely derives from `360 / 365 = 0.986301…` rounded to 3 significant figures (`0.986`), representing a simple degrees-per-day approximation using the integer number of days in a solar year (non-leap). The value was not derived from `360 / 365.25` (the mean year length including leap years), which would give the slightly smaller `0.985626…`.

**Impact**: None on HARES implementation — the value is correct as written. The four reference sources (Hendron 2004, Burch-Christensen 2007, EnergyPlus, OCHRE) all use `0.986`.

---

### Finding 2: [Severity: low] 360/365.25 (= 0.9856) is not the "correct" value — it is a different approximation

**Description**: The framing "divergence from 360/365.25 = 0.9856" implicitly suggests that `0.9856` is the ground-truth value. In reality, the true daily mean Earth orbital angular speed is:

| Formula | Value | Derivation |
|---------|-------|-----------|
| `360 / 365` | 0.98630… | Integer days per non-leap year |
| **`0.986` (canonical)** | 0.986 | Likely 360/365 rounded to 3 SF |
| `360 / 365.242190` | 0.98565… | Tropical year (actual orbit) |
| `360 / 365.25` | 0.98563… | Julian year average |
| `360 / 365.2564` | 0.98560… | Sidereal year |

The `0.9856` value often cited as the "correct" physical number is neither the original paper's constant nor the most accurate astronomical value. It is simply `360 / 365.25` truncated to 4 decimal places — an alternative approximation with no claim to greater accuracy than `0.986`.

**Code Location**: The HARES comment at `water_mains.rs:14-15` correctly identifies that `0.986` is not derived from 360/365.25.

**Root Cause**: The Burch-Christensen 2007 paper was empirically calibrated to measured mains-water data from US sites. The exact degrees-per-day constant is subsumed by the empirical calibration of the ratio, lag, and offset parameters. The authors' specific choice of `0.986` (versus `0.9856` or `0.98565`) is irrelevant to model accuracy because the calibration compensates. Changing it now would break parity with the canonical implementations without improving physical fidelity.

**Impact**: None. The `0.986` value is the correct reference value; `0.9856` would be incorrect to use.

---

### Finding 3: [Severity: low] Annual temperature impact of the 0.986 vs 0.9856 difference is negligible

**Description**: Replacing `DEG_PER_DAY = 0.986` with `DEG_PER_DAY = 360.0 / 365.25` would shift the daily angular position by a cumulative 0.000374 degrees/day. The maximum impact on mains water temperature predictions was quantified:

| Metric | Value |
|--------|-------|
| Angular difference at mid-year (day 182.5) | 0.068° |
| sin(angular_diff) at worst case | 0.00119 |
| Max temp error with 10 °F amplitude | 0.012 °F ≈ 0.007 °C |
| Max temp error with 17.5 °F amplitude (extreme US) | 0.021 °F ≈ 0.012 °C |
| Water heating energy error (55°C setpoint, 10°C mains) | < 0.02% |
| Annual mean temperature impact | 0 (sine term averages to zero) |

**Code Location**: `water_mains.rs:108` — the `angl`e_deg computation where difference would manifest.

**Root Cause**: The sine function is smooth at the 0.068-degree scale, producing amplitude differences below 0.02 °F. This is two orders of magnitude below typical measurement uncertainty in weather data and below the precision floor of the empirical calibration.

**Impact**: None. The constant difference is far below any physically meaningful threshold for building energy simulation.

---

### Finding 4: [Severity: low] HARES comment could be clarified with the actual derivation

**Description**: The comment block at `water_mains.rs:13-16` states the value is from "the Burch-Christensen (2007) paper (0.986)" and adds a note about `360/365.25`. It does not explain *why* the paper chose 0.986, which would be helpful for reviewers asking the same question as this review.

**Code Location**: `crates/hares-physics/src/water_mains.rs:13-16`

**Suggested text**: The constant likely comes from `360/365` (degrees per day in a 365-day solar year) rounded to 3 significant figures — a simpler and no less defensible choice than the more precise astronomical value `360/365.2422 ≈ 0.98565`. Because the model was empirically calibrated using this value, changing it would break parity with the reference implementations (EnergyPlus, OCHRE) without improving accuracy.

**Impact**: Documentation quality only. No code impact.

## Summary
- **Total findings**: 4
- **Critical**: 0
- **High**: 0
- **Medium**: 0
- **Low**: 4

## Recommendations
1. **No code changes needed.** `DEG_PER_DAY = 0.986` is correct and matches EnergyPlus, OCHRE, and the original Hendron 2004 / Burch-Christensen 2007 papers.
2. Optionally improve the comment at `water_mains.rs:13-16` to explain that `0.986` likely derives from `360/365` rounded to 3 SF, and that the alternative `360/365.25 = 0.9856` is neither more nor less correct — both are approximations to the true `360 / 365.2422 ≈ 0.98565`, and the specific choice is immaterial because the model parameters were empirically calibrated alongside it.
3. The `360/365.25` framing in the review prompt represents an alternative approximation, not a more correct physical value. Any future reviews should use the tropical year value `360 / 365.242190` for physical comparison rather than `360 / 365.25`.

## References / Citations
- Hendron, R., Anderson, R., Christensen, C., Eastment, M., and Reeves, P. (2004). "Development of an Energy Savings Benchmark for All Residential End-Uses." *Proceedings of SimBuild 2004*, IBPSA-USA, Boulder, CO.
- Burch, J. and Christensen, C. (2007). "Towards Development of an Algorithm for Mains Water Temperature." *Proceedings of the 2007 ASES National Solar Conference*.
- EnergyPlus `WeatherManager.cc:7204` — bare literal `0.986` in the water mains temperature sine argument.
- OCHRE `schedule.py:239` — bare literal `0.986` in the water mains temperature formula.
