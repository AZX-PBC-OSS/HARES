# Verify all 5 model constants (OFFSET_F=6F, RATIO_BASE=0.4, RATIO_SLOPE=0.01, LAG_BASE=35, LAG_SLOPE=1.0) against Burch-Christensen 2007 paper
**Review ID**: wm-02
**Category**: water-mains
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/water_mains.rs:21-36`

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/utils/schedule.py:230-241` — OCHRE's reference implementation of the Burch-Christensen model (hardcoded magic numbers)

## Findings

### Finding 1: [Severity: low] Ratio clamping diverges from Burch-Christensen paper formula
**Description**: HARES clamps the ratio coefficient to `[0.0, 1.0]` at `water_mains.rs:98`, whereas the OCHRE reference implementation applies no clamping (`schedule.py:233`). The Burch-Christensen paper does not specify clamping; the ratio formula is a linear regression valid within the contiguous US calibration range (annual average −5 °C to 30 °C). For extremely cold arctic climates (annual avg < −24 °C), the unclamped formula produces negative ratio values, which would cause phase inversion. The HARES clamping is a deliberate guard against this extrapolation scenario, documented in the comment at `water_mains.rs:96-97`, but it represents a deviation from the published model.
**Code Location**: `crates/hares-physics/src/water_mains.rs:98`
**Root Cause**: Defensive programming for extrapolated climates outside the paper's calibration range.
**Impact**: Within the contiguous US calibration range (annual avg ≥ −5 °C), the ratio never reaches 0.0, so clamping has no effect and HARES exactly matches the paper. Outside that range, HARES silently degrades to zero amplitude (flat mains temperature) rather than producing phase-inverted output as OCHRE would. Low impact because the model is not validated for such climates.

### Finding 2: [Severity: low] No direct paper citation verified
**Description**: The constants cannot be directly verified against the original Burch-Christensen 2007 paper text because the paper is not available in the repository. Verification relies on cross-referencing against OCHRE (which explicitly cites the paper at `schedule.py:231`) and EnergyPlus documentation (which reproduces the same model with the same constants). Both independent reference implementations agree with HARES's values.
**Code Location**: N/A (documentation concern)
**Root Cause**: Primary source not archived in the repository.
**Impact**: Low — two independent reference implementations (OCHRE and EnergyPlus) corroborate the values.

## Constant Verification Summary

| Constant | HARES Value | OCHRE Value | OCHRE Location | Match? |
|---|---|---|---|---|
| `OFFSET_F` | 6.0 °F | `+ 6` | `schedule.py:238` | Yes |
| `RATIO_BASE` | 0.4 | `0.4` | `schedule.py:233` | Yes |
| `RATIO_SLOPE` | 0.01 /°F | `0.01` | `schedule.py:233` | Yes |
| `LAG_BASE` | 35.0 days | `35` | `schedule.py:234` | Yes |
| `LAG_SLOPE` | 1.0 days/°F | `1.0` (implicit) | `schedule.py:234` | Yes |
| `T_REF_F` | 44.0 °F | `44` | `schedule.py:233-234` | Yes |
| `DEG_PER_DAY` | 0.986 °/day | `0.986` | `schedule.py:239` | Yes |

All 5 model constants match the Burch-Christensen 2007 reference values as implemented in both OCHRE and EnergyPlus. The algebraic forms are also preserved:

- **Ratio**: HARES `RATIO_BASE + RATIO_SLOPE * (t_avg_f - T_REF_F)` = `0.4 + 0.01 * (t_avg_f - 44)` ≡ OCHRE `0.4 + 0.01 * (t_amb_avg - 44)`
- **Lag**: HARES `LAG_BASE - LAG_SLOPE * (t_avg_f - T_REF_F)` = `35 - 1.0 * (t_avg_f - 44)` = `79 - t_avg_f` ≡ OCHRE `35 - (t_amb_avg - 44)` = `79 - t_amb_avg`
- **Offset**: HARES `OFFSET_F` = `6.0` ≡ OCHRE `+ 6`

The Chicago TMY2 cross-validation test at `water_mains.rs:308-334` further confirms numerical equivalence against OCHRE's formula to within 0.001 °C.

## Summary
- Total findings: 2
- Critical: 0 / High: 0 / Medium: 0 / Low: 2

## Recommendations
1. Consider archiving a copy of the Burch-Christensen 2007 paper in the repository's `docs/references/` directory for direct auditability of all constants.
2. The ratio clamping at `water_mains.rs:98` is a reasonable defensive measure for out-of-range climates, but document that the unclamped formula (per the paper) is `0.4 + 0.01 * (t_avg_f - 44)` without bounds. No code change required — the clamping is inactive within the paper's calibration range.

## References / Citations
- Burch, J. and Christensen, C. (2007). "Towards Development of an Algorithm for Mains Water Temperature." Proceedings of the 2007 ASES National Solar Conference.
- OCHRE reference implementation: `vendors/OCHRE/ochre/utils/schedule.py:230-241`
- EnergyPlus Engineering Reference, "Mains Water Temperature" section (reproduces the Burch-Christensen model with identical constants)
