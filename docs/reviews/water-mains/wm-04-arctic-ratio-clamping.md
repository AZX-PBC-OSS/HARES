# Ratio clamping at arctic temperatures — verify clamping produces flat behavior (not inverted seasons) when t_avg < -24°C
**Review ID**: wm-04
**Category**: water-mains
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-physics/src/water_mains.rs` (lines 8–114, 128–392)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/utils/schedule.py` (lines 222–241, Burch-Christensen mains temperature algorithm)
- `vendors/OCHRE/ochre/Models/Water.py` (mains temperature usage, no independent ratio clamping)

## Findings

### Finding 1: Comment on zero-crossing threshold is numerically wrong [Severity: low]
**Description**: The comment at `water_mains.rs:96–97` states:
> Clamp ratio to [0.0, 1.0] to prevent phase inversion for arctic climates (annual avg < −24 °C) where the unclamped formula goes negative.

The formula is `RATIO_BASE + RATIO_SLOPE * (t_avg_f - T_REF_F)` = `0.4 + 0.01 * (t_avg_f - 44.0)`. The unclamped ratio crosses zero at `t_avg_f = 4.0 °F` = `−15.56 °C`, **not** at −24 °C. The actual threshold where phase inversion begins is −15.56 °C, which is 8.4 °C warmer than the comment claims.

**Code Location**: `crates/hares-physics/src/water_mains.rs:96–97`
**Root Cause**: Likely a documentation error. The value −24 °C may have originated from converting 4 °F incorrectly (adding 32 instead of after the conversion), or from a sign error in an earlier calculation.
**Impact**: Low. The clamping logic is mathematically correct — it activates at the true zero-crossing of −15.56 °C via `f64::clamp(0.0, 1.0)`. However, a reader relying on the comment might incorrectly believe the clamping activates at a colder temperature than it actually does, potentially causing confusion during climate-range analysis or parameter sensitivity studies.

**Verification — zero-crossing calculation**:
```
t_avg_f_zero = T_REF_F - RATIO_BASE / RATIO_SLOPE
             = 44.0 - 0.4 / 0.01
             = 44.0 - 40.0
             = 4.0 °F
t_avg_c_zero = (4.0 - 32.0) × 5/9 = −28.0 × 5/9 = −15.556 °C
```

### Finding 2: Clamping correctly prevents inverted seasonality across the full test range [Severity: none — confirmation]
**Description**: The clamping logic `ratio.clamp(0.0, 1.0)` at `water_mains.rs:98` was tested analytically and via the existing test suite at the four requested temperatures. At −15.56 °C and colder, the unclamped ratio becomes negative, which would flip the sign of the seasonal sine-wave amplitude and produce inverted seasons (summer colder than winter). Clamping at 0.0 replaces this inverted curve with a flat, zero-amplitude output.

**Test results for requested t_avg values** (all at dt_annual_range = 30 °C, Northern Hemisphere):

| t_avg_c | t_avg_f | Unclamped ratio | Clamped ratio | Amplitude (°F) | Behavior |
|---------|---------|----------------:|--------------:|---------------:|----------|
| −20 °C  |  −4 °F  |          −0.080 |           0.0 |            0.0 | Flat |
| −24 °C  | −11.2 °F|          −0.152 |           0.0 |            0.0 | Flat |
| −30 °C  | −22 °F  |          −0.260 |           0.0 |            0.0 | Flat |
| −40 °C  | −40 °F  |          −0.440 |           0.0 |            0.0 | Flat |

At all four points, the clamped ratio is 0.0, amplitude is 0.0, and the output is a flat line at `t_avg_f + OFFSET_F` (6 °F above the annual average). No seasonal inversion occurs. The existing test `arctic_climate_neg40c_ratio_clamped_to_zero` (`water_mains.rs:201–212`) independently confirms this: all 5 sampled days produce the same value within 0.001 °C tolerance.

**Contrast without clamping**: At −40 °C with dt_annual_range = 30 °C (dt_f = 54 °F), the unclamped ratio of −0.44 would produce an amplitude of `−0.44 × 27 = −11.88 °F` (−6.6 °C half-swing). The sine wave would be phase-inverted, with the _minimum_ occurring in late summer (day ~230–260) and the _maximum_ in late winter (day ~40–60). This is physically wrong for water mains temperature.

**Code Location**: `crates/hares-physics/src/water_mains.rs:98` and `water_mains.rs:109`
**Root Cause**: The Burch-Christensen ratio formula `0.4 + 0.01 × (t_avg_f − 44)` was calibrated for contiguous US climates (annual average −5 °C to 30 °C). Below −15.56 °C, the linear extrapolation becomes unphysical. Clamping is correct.
**Impact**: None (defensive code is working as intended).

### Finding 3: OCHRE reference implementation does NOT clamp the ratio [Severity: medium]
**Description**: The OCHRE reference implementation at `vendors/OCHRE/ochre/utils/schedule.py:233–239` computes the ratio identically to HARES:
```python
tmains_ratio = 0.4 + 0.01 * (t_amb_avg - 44)
```
but applies **no clamping** before multiplying into the sine term:
```python
t_mains = t_amb_avg + 6 + tmains_ratio * dt_monthly * np.sin(...)
```
When `t_amb_avg < 4 °F (−15.56 °C)`, `tmains_ratio` goes negative and OCHRE produces an inverted seasonal curve. HARES's clamping to `[0.0, 1.0]` is an improvement over the vendor reference that prevents this non-physical behavior.

**Code Location**: `vendors/OCHRE/ochre/utils/schedule.py:233,239` (no clamping) vs `crates/hares-physics/src/water_mains.rs:98` (clamping present)
**Root Cause**: OCHRE's implementation is a direct transcription of the Burch-Christensen formula without extrapolation guards. The original paper calibrated only for US climates, so extreme extrapolation was not considered.
**Impact**: Medium. For OCHRE users simulating arctic/antarctic climates, the unclamped ratio would produce inverted water mains seasonality (warm mains in winter, cold in summer), leading to incorrect water heating energy predictions. This issue does not affect HARES because of the clamping. The HARES codebase should note this divergence in documentation.

## Summary
- Total findings: 3
- Critical: 0
- High: 0
- Medium: 1 (Finding 3 — OCHRE lacks clamping, HARES's clamping is an improvement)
- Low: 1 (Finding 1 — inaccurate zero-crossing comment)
- Confirmation: 1 (Finding 2 — clamping works correctly, no issues found)

## Recommendations
1. **Fix the comment** at `water_mains.rs:97`: change `−24 °C` to `−15.56 °C` (or `approximately −15.6 °C`) to accurately reflect the true zero-crossing threshold of the unclamped ratio.
2. **Document the OCHRE divergence** in the module-level doc comment (`water_mains.rs:8–10`) or in a design note, noting that HARES improves on the reference by clamping the ratio to prevent phase inversion beyond the model's calibration range.
3. **Consider a boundary-region test** at `t_avg_c` values just above and just below −15.56 °C to explicitly verify that clamping activates smoothly at the zero-crossing. For example: at −15 °C (ratio very small but positive), verify weak but correct seasonality; at −16 °C (ratio clamped to 0), verify flat output.

## References / Citations
- Burch, J. and Christensen, C. (2007). "Towards Development of an Algorithm for Mains Water Temperature." Proceedings of the 2007 ASES National Solar Conference.
- OCHRE mains temperature implementation: `vendors/OCHRE/ochre/utils/schedule.py:222–241`
- EnergyPlus Engineering Reference, Section "Site:WaterMainsTemperature" (Burch-Christensen model)
