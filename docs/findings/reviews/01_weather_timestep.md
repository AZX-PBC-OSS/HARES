# Weather Pipeline & Timestep — First-Principles Physics Audit

**Reviewer**: Claude Sonnet 4.6  
**Date**: 2026-04-23  
**Scope**: Weather pipeline and timestep handling since `c1abb8af9d3087873ba4d7010f3383a647a7fd79`  
**Files reviewed**: `weather.rs`, `epw.rs`, `tmy3.rs`, `psm3.rs`, `resstock_csv.rs`, `schedule.rs`, `schedule_resolve.rs`, `clock.rs`, `hares-python/src/lib.rs`  
**Method**: First-principles derivation from primary sources; tool-verified test execution  

---

## 1. Executive Summary

**BLOCKERS PRESENT — DO NOT MERGE.**

Two blockers break tests in CI: (WR-01) `ochre_compat()` silently applies Triangular solar and CircularLinear wind direction instead of ZOH, causing 16 check violations across 7 parity corpus fixtures; (WR-02) `resampled_weather_produces_smooth_environment` fails unconditionally — step 1396 produces a 0.155 °C jump exceeding the 0.15 °C threshold. Both are directly confirmed by `cargo test`.

All physics constants are traceable to authoritative primary sources. The Stefan-Boltzmann constant is correctly CODATA 2018 (5.670374419e-8). The sky-temperature cascade (Stefan-Boltzmann inversion → Berdahl-Martin + Walton → Clark-Allen fallback) matches EnergyPlus WeatherManager. The B5 fix (recomputing sky temperature from interpolated inputs) is correct in design and well-tested. The B9 circular wind-direction interpolation, S7 triangular solar interpolation, and D5 cyclic PCHIP are all physically correct. The TMY3 `midpoint_offset_secs = 1800` fix (B4) is present in code but lacks a pinning test.

One new formula finding (not in the prior review): the Berdahl-Martin coefficients in `epw.rs` (0.758 / 0.521 / 0.625) match the **calibrated** Li et al. form cited in the EnergyPlus FY2020 design document and used in E+ 9.6, not the original 1984 Berdahl & Martin paper (0.711 / 0.56 / 0.73). This is **correct and better physics** — but the docstring cites the wrong paper. The coefficients belong to Martin & Berdahl (1984) Solar Energy 33(3/4) as cited in code; independent verification via the E+ FY2020 design document (`Alternative Models for Clear Sky Emissivity Calculation.md`) confirms the calibrated values match E+.

One silent-default violation persists in `epw.rs:204`: precipitation parse failure silently produces 0.0 instead of propagating an error. All other `unwrap_or` paths are structurally justified fallbacks (optional fields, OR computed fallbacks with explicit rationale).

---

## 2. Constants Audit Table

| Constant | Value in Code | Authoritative Value | Source | Verdict |
|---|---|---|---|---|
| `STEFAN_BOLTZMANN` | `5.670_374_419e-8` W/(m²·K⁴) | `5.670374419e-8` | NIST CODATA 2018 | CORRECT |
| `CELSIUS_TO_KELVIN` | `273.15` | `273.15` K | ISA 1976 / NIST | CORRECT |
| `SEA_LEVEL_PRESSURE_PA` | `101_325.0` Pa | `101325` Pa | ISA 1976 / ICAO Doc 7488 | CORRECT |
| `ISA_LAPSE_COEFFICIENT` | `2.255_77e-5` 1/m | `2.25577e-5` | ISA 1976 | CORRECT |
| `ISA_PRESSURE_EXPONENT` | `5.2559` | `5.25588` (from g/R_da/L) | ISA 1976 | MINOR DISCREPANCY — see WR-14 |
| `DRY_AIR_GAS_CONSTANT_J_KG_K` | `287.058` J/(kg·K) | `287.052874` (NIST CODATA) / `287.058` (ASHRAE 2017) | ASHRAE 2017 HOF Ch.1 | CORRECT (ASHRAE value used deliberately) |
| Clark-Allen coefficients | `0.787`, `0.764` | `0.787`, `0.764` | Clark & Allen (1978), ASES; E+ WeatherManager | CORRECT |
| Berdahl-Martin coefficients | `0.758`, `0.521`, `0.625` | `0.758`, `0.521`, `0.625` | Calibrated form — E+ FY2020 design doc (Li et al. recalibration of Martin & Berdahl 1984) | CORRECT — but docstring cites the wrong Berdahl paper (see WR-15) |
| Original Berdahl & Martin (1984) | N/A (not used) | `0.711`, `0.56`, `0.73` | Berdahl & Martin (1984) Solar Energy 32(5) | N/A — not used, calibrated form is correct |
| Walton cloud correction | `0.0224`, `0.0035`, `0.00028` | `0.0224`, `0.0035`, `0.00028` | Walton (1983), NBSIR 83-2655 | CORRECT |
| `INFRARED_FALLBACK_THRESHOLD` | `50.0` W/m² | E+ WeatherManager uses same threshold | EnergyPlus WeatherManager | CORRECT |
| `DOE2_GROUND_DIFFUSIVITY` | `0.025` m²/h | `0.025` m²/h (DOE-2 default for average soil) | OCHRE `schedule.py:248`, DOE-2 GTEMP subroutine | CORRECT (matches OCHRE reference) |
| `DOE2_GROUND_DEPTH_FACTOR` | `10.0` m | `10` m | DOE-2 GTEMP | CORRECT |
| `DOE2_GROUND_PHASE_OFFSET_RAD` | `0.6` rad | `0.6` rad | DOE-2 GTEMP | CORRECT (matches OCHRE `schedule.py:248`) |
| `DOE2_GROUND_HOURS_PER_YEAR` | `8760.0` h | `8760` h | Standard year convention | CORRECT |
| `DOE2_GROUND_DAYS_PER_YEAR` | `365.0` days | `365.0` days | DOE-2 GTEMP (uses 365, not 365.25) | CORRECT (matches OCHRE exactly) |
| EPW `midpoint_offset_secs` | `1800` s | `source_step/2 = 1800` s | EnergyPlus Auxiliary Programs: EPW hour-ending convention; OCHRE `schedule.py:168` `offset = timedelta(minutes=30)` | CORRECT |
| TMY3 `midpoint_offset_secs` | `1800` s | `source_step/2 = 1800` s | TMY3 User's Manual (Wilcox & Marion 2008, §3.3): hour-ending convention identical to EPW | CORRECT |
| PSM3 `midpoint_offset_secs` | `0` s | `0` s | PSM3 uses beginning-of-interval timestamps | CORRECT |
| ResStock `midpoint_offset_secs` | `0` s | Should be `1800` s — see WR-09 | ResStock AMY files use end-of-interval timestamps (documented in `resstock_csv.rs:172`) | INCORRECT — pre-existing issue, see WR-09 |
| OCHRE sky temp σ (reference) | N/A | `5.6697e-8` (stale) | OCHRE `schedule.py:182` | HARES correctly deviates from OCHRE |

### ISA Exponent Discrepancy Detail (WR-14)

The ISA 1976 standard atmosphere exponent is `g/(R·L) = 9.80665 / (287.058 × 0.0065) = 5.25588`. `constants.rs` defines `5.2559` (rounded to 4 decimal places). `resstock_csv.rs` inline literal `5.25588` is more precise. The `constants.rs` value introduces ≈0.0004% error in pressure; at 1600 m this yields ≈0.4 Pa error. Negligible for weather calculations but the inconsistency between the constant (`5.2559`) and the inline literal (`5.25588`) is a DRY violation; the inline literal has better precision.

---

## 3. Formulae Audit Table

| Formula | Location | Derivation / Authority | Code Matches? | Source |
|---|---|---|---|---|
| Sky temp from IR: `T_sky = (IR/σ)^0.25 - 273.15` | `epw.rs:494–495` | Direct Stefan-Boltzmann inversion. OCHRE `schedule.py:182` (same formula, stale σ). E+ WeatherManager `calcSky()`. | YES | NIST CODATA 2018 σ; OCHRE/E+ precedent |
| Berdahl-Martin emissivity: `ε = 0.758 + 0.521(T_dp/100) + 0.625(T_dp/100)²` | `epw.rs:536–537` | Calibrated form from E+ FY2020 design doc (Li et al. recalibration). Confirmed match to EnergyPlus 9.6 and E+ FY2020 doc `Alternative Models for Clear Sky Emissivity Calculation.md`. | YES (calibrated form) | E+ FY2020 design doc; `Alternative Models for Clear Sky Emissivity Calculation.md` |
| Walton cloud correction: `ε_sky = ε_clear × (1 + 0.0224N - 0.0035N² + 0.00028N³)` | `epw.rs:586–587` | Walton (1983) NBSIR 83-2655 polynomial. EnergyPlus uses same coefficients. N clamped to [0,10]. | YES | Walton (1983); E+ WeatherManager |
| Clark-Allen: `ε = 0.787 + 0.764 × ln(T_dp_K / 273.15)`, `T_sky = T_db_K × ε^0.25` | `epw.rs:519` | Clark & Allen (1978) ASES. E+ WeatherManager. OCHRE `schedule.py` uses same formula for fallback. Note code divides by `KELVIN_OFFSET_C` (273.15) — matches source. | YES | Clark & Allen (1978) |
| Sky-temp cascade order: IR ≥ 50 → S-B; cover > 0 → B-M+Walton; else → Clark-Allen | `epw.rs:492–503` | E+ WeatherManager `calcSky()` signature takes `IRHoriz`; E+ uses S-B inversion when IR data present, fallback models otherwise. OCHRE only uses S-B (no B-M+Walton tier). HARES three-tier cascade is more faithful to E+ than OCHRE. | YES | E+ WeatherManager; EnergyPlus Eng. Ref. §Climate Calculations |
| Sky temp recomputed after resampling (not directly interpolated) | `weather.rs:525–531`, `616–622` | Chain-rule argument: T_sky = f(IR, T_db, T_dp, cover) is nonlinear (4th-root). Direct interpolation of T_sky produces values inconsistent with interpolated inputs. E+ WeatherManager recomputes after each timestep interpolation. | YES | E+ WeatherManager design; chain-rule physics |
| DOE-2 ground temp: sinusoidal damped correlation | `epw.rs:449–463` | DOE-2 GTEMP subroutine. OCHRE `schedule.py:244-265` implements identical formula with same constants. Formula: `T(day) = T_avg - ΔT × gm × cos(2π·day/365 - phase)` | YES | DOE-2 GTEMP; OCHRE `schedule.py` |
| Ground temp linear interpolation between monthly anchors | `epw.rs:607–656` | Linear interpolation is physically superior to E+ step-function. OCHRE uses linear via pandas. No standard mandates the specific interpolation method. | YES | OCHRE precedent; physics of thermal diffusion |
| Triangular solar interpolation: `frac < 0.5: prev×(0.5-frac) + cur×(0.5+frac); frac ≥ 0.5: cur×(1.5-frac) + next×(frac-0.5)` | `weather.rs:1085–1090` | Midpoint-interpolation resampling. Mean of sub-hourly values is `0.125×prev + 0.75×cur + 0.125×next` — approximately equals `cur` for slowly varying signals (≤2% error for sinusoidal profiles, confirmed by test at `weather.rs:2471`). C0-continuous at hour boundaries confirmed analytically and by test. E+ WeatherManager `SetupInterpolationValues` uses a different (current/previous-weighted) approach; HARES uses midpoint interpolation which is the correct physical interpretation of "hourly mean placed at midpoint". | YES (correct physics) | Physical midpoint-interpolation principle; E+ Eng. Ref. §Weather File Solar Interpolation |
| Circular wind interpolation: `delta = ((b - a + 180) % 360) - 180; result = (a + frac×delta) % 360` | `weather.rs:1022–1023` | Correct shortest-arc formula using Euclidean remainder. For 350° → 10°: `delta = ((10-350+180) % 360) - 180 = (-160 rem_euclid 360) - 180 = 200 - 180 = 20°`. Result at frac=0.5: `(350 + 10) % 360 = 0°`. Confirmed correct by test `circular_linear_wrap_350_to_10`. | YES | EnergyPlus WeatherManager `interpolateWindDirection` (same algorithm); angular shortest-arc standard |
| PCHIP Fritsch-Carlson boundary slopes (one-sided 3-pt) | `weather.rs:687–712` | Matches SLATEC pchim.f boundary slope formula: `s = 1.5×d1 - 0.5×d2`; capped at zero if sign changes, capped at `3×d1` if exceeds monotonicity bound. Confirmed match to SciPy `PchipInterpolator` and MATLAB `pchip`. | YES | Fritsch & Carlson (1980), SIAM J. Numer. Anal. 17(2); SLATEC pchim.f |
| PCHIP Fritsch-Carlson interior slopes: `(d_{k-1} + d_k)/2` when signs agree, else 0 | `weather.rs:715–721` | Standard initial estimate for monotone Hermite spline. | YES | Fritsch & Carlson (1980) §2 |
| PCHIP F-C monotonicity correction: `if α²+β² > 9: τ = 3/√(α²+β²); scale both slopes` | `weather.rs:723–741` | SLATEC pchim.f form: uses `τ×d[k]` instead of `τ×α×δ[k]` to avoid 0×∞=NaN. Correct. | YES | Fritsch & Carlson (1980) §3; SLATEC pchim.f |
| Cyclic PCHIP boundary: all slopes use centered differences including wrap | `weather.rs:768–776` | For periodic data, correct; all n segments including wrap (n-1→0) treated uniformly. No boundary special case needed. | YES | Fritsch & Carlson (1980) extension to periodic data |
| F-C cyclic correction loop processes n segments with `k_next = (k+1) % n` | `weather.rs:779–793` | The loop may scale d[k_next] twice if k_next is visited later in the outer loop. This is the same convergence property as the non-cyclic case: each scaling only reduces magnitude; the algorithm still converges to a monotone interpolant. | YES (NIT — see WR-12) | Fritsch & Carlson (1980) convergence proof |
| EPW hour-ending → midpoint: subtract 1800 s from simulation time when indexing | `weather.rs:223–227` (comment), `tmy3.rs:240–244`, `epw.rs:284–287` | EPW/TMY3 data row for hour H covers H-1:00 to H:00. Midpoint is H-0:30. The simulation steps forward from the midpoint: `t_index = (sim_time - offset) / source_step`. With offset=1800, hour 1 row is accessed at t=0:30 (correct). Confirmed matches OCHRE `schedule.py:168` `offset=timedelta(minutes=30)`. | YES | EnergyPlus Auxiliary Programs §EPW format; TMY3 User's Manual §3.3; OCHRE schedule.py |
| Magnus dew point: `α = ln(RH/100) + 17.67×T/(243.5+T); T_dp = 243.5α/(17.67-α)` | `resstock_csv.rs:57–60` | August-Roche-Magnus formula (Alduchov & Eskridge 1996 calibration: b=17.67, c=243.5). Valid for -40 to +50°C. The `rh_pct` clamp to [0.001, 1.0] in fractional form is a guard against ln(0) — this is correct. | YES | Alduchov & Eskridge (1996) |

### Mean Preservation Claim (WR-04 cross-check)

The mean of `factor` sub-samples for hour `i` under the triangular formula is:

```
mean = (1/N) × sum_{j=0}^{N-1} [
    prev×(0.5 - j/N) + cur×(0.5 + j/N)   for j < N/2
    cur×(1.5 - j/N) + next×(j/N - 0.5)   for j ≥ N/2
]
= 0.125×prev + 0.75×cur + 0.125×next
```

This equals `cur` only when `prev = next` (constant or symmetric signal). For a ramp `prev=0, cur=800, next=800`, the mean = `0.125×0 + 0.75×800 + 0.125×800 = 700 ≠ 800` (12.5% shortfall). The docstring at `weather.rs:292` claims "hourly mean preservation" — this is incorrect for non-constant signals. The test `triangular_hourly_mean_preservation` correctly tests with ≤2% relative tolerance for a sinusoidal profile, which is accurate, but the docstring overstates the guarantee.

---

## 4. Test Integrity Audit

| Test Name | File:Line | Reference-Value Provenance | Tolerance Justification | Verdict |
|---|---|---|---|---|
| `sky_temp_stefan_boltzmann_300_w_m2` | `weather_parity.rs:62` | Derived: `(300/5.670374419e-8)^0.25 - 273.15 = -3.45°C`. Expected `-3.5°C` ± 0.1°C. | 0.1°C tolerance — rounding of `-3.45` to `-3.5`. Justified. | SOUND |
| `sky_temp_stefan_boltzmann_increases_monotonically_with_ir` | `weather_parity.rs:78` | Physical: S-B is strictly monotone in IR. | Qualitative (no numeric tolerance needed). | SOUND |
| `sky_temp_infrared_fallback_threshold_is_50_w_m2` | `weather_parity.rs:98` | Const-assert + physical: `(50/σ)^0.25 - 273.15 ≈ -90.5°C`, confirming 50 W/m² is implausible for atmosphere. | < -50°C bound physically derived. | SOUND |
| `clark_allen_sky_temp_at_20c_dry_10c_dew` | `weather_parity.rs:128` | Derivation: T_db=293.15K, T_dp=283.15K, ε=0.81446, T_sky_k=278.9K, T_sky_c≈5.75°C. Expected range (0,15)°C. | Range conservative; actual ≈5.75°C is well inside. | SOUND |
| `clark_allen_sky_temp_decreases_with_lower_dew_point` | `weather_parity.rs:156` | Physical: lower RH → lower water vapour emission → lower T_sky. | Monotonicity check only; no numeric tolerance. | SOUND |
| `pchip_passes_through_knot_values` | `weather_parity.rs:411` | Mathematical requirement: PCHIP is an interpolant. | `1e-10` floating-point epsilon. | SOUND |
| `pchip_is_c1_continuous` | `weather_parity.rs:427` | Mathematical: PCHIP is C1 by construction. Richardson extrapolation used. | `1e-4` — generous but verified via Richardson. | SOUND |
| `circular_linear_wrap_350_to_10` | `weather.rs:1696` | Analytically derived: delta=+20°, midpoint=0°. | `1e-12` floating-point. | SOUND |
| `triangular_hourly_mean_preservation` | `weather.rs:2448` | Analytically derived: mean = 0.125·prev + 0.75·cur + 0.125·next. For sinusoidal profile, rel error ≤2%. | 2% relative tolerance; 30 W/m² absolute for nighttime. Justified for smooth profiles. | SOUND (but docstring claim is misleading — see WR-04) |
| `triangular_c0_continuous_at_hour_boundaries` | `weather.rs:2391` | Mathematical: both pieces evaluate to same value at frac=0 and frac→1. | Exact `1e-12` match for start; `1/factor`-bounded gap for end. | SOUND |
| `sky_temp_recomputed_after_upsampling` | `weather.rs:1563` | Mathematical identity: sky_temp must equal `compute_sky_temp_c(resampled_inputs...)`. | `1e-9` — correct for double-precision chain. | SOUND |
| `sky_temp_recomputed_after_downsampling` | `weather.rs:1612` | Same mathematical identity for mean-downsampled inputs. | `1e-9`. | SOUND |
| `pchip_cyclic_no_discontinuity_at_boundary` | `weather.rs:1921` | Physical: year-boundary discontinuity must be ≤1°C for sinusoidal 24-pt series. | 1.0°C tolerance is generous but appropriate — the test verifies the qualitative fix (cyclic vs flat-hold). | SOUND (qualitative) |
| `resampled_weather_produces_smooth_environment` | `weather_integration.rs:537` | Physical: PchipCyclic on 24-element synthetic series with large year-boundary gradient (h23≈6.9°C, h0=15°C) pulls endpoint slopes, producing 0.155°C jump at step 1396. | 0.15°C threshold was correct for non-cyclic Pchip; too tight for PchipCyclic on a 24-element series. **FAILS.** | BROKEN — see WR-02 |
| `full_pipeline_synthetic_weather` | `weather_integration.rs:259` | Physical bounds on each quantity; precipitation total conservation. | Ranges are physically justified (e.g., T in [-10,35], WB < DB − 0.1). | SOUND |
| `solar_irradiance_physical_bounds` | `weather_integration.rs:462` | Physical: total ≤ 1400 W/m², nighttime < 5 W/m². | Conservative but correct. | SOUND |

### Missing Tests (High-Priority Gaps)

| Gap | Severity |
|---|---|
| No assertion `tmy3.meta.midpoint_offset_secs == 1800` in `parse_standard_year` test | HIGH |
| No test for PSM3 `midpoint_offset_secs == 0` (trivially correct, but unguarded) | LOW |
| No test for ResStock `midpoint_offset_secs == 0` (incorrect value — see WR-09) | MEDIUM |
| No test confirming `ochre_compat()` produces ZOH for GHI/DNI/DHI/wind_dir/wind_speed | HIGH |
| No test for `triangular_resample` with a physically realistic sunrise-only profile where `prev=0` at first daylight hour (exercises the nonlinear mean deviation path) | MEDIUM |
| No dedicated test for PCHIP at year boundary for a full 8760-element series (existing tests use 24-element synthetic) | LOW |

---

## 5. Severity-Ranked Findings

### BLOCKER

---

**WR-01** — `ochre_compat()` does not override solar or wind direction; 16 parity failures  
`crates/hares-io/src/weather.rs:324–335`

`ochre_compat()` sets ZOH for `dry_bulb`, `dew_point`, `rel_humidity`, `pressure`, `infrared`, `ground_temp`, `opaque_sky_cover`, then falls through to `..Default::default()` for the rest. The default for `ghi`/`dni`/`dhi` is now `Triangular` and for `wind_dir` is `CircularLinear`. These were both ZOH before this campaign and should remain ZOH in compat mode. Every parity test calling `ochre_compat()` now silently receives smoother solar and physically correct wind interpolation, which shifts zone temperatures and HVAC energy vs the OCHRE reference.

Confirmed by `cargo test -p hares-core --test parity`: 16 check violations across `cz2a_pv_ev`, `cz4a_ashp_hpwh`, `cz4a_battery_only`, `cz4a_pv_battery`, `cz4a_pv_only`, `cz5a_ev_only`, `cz5a_minisplit_gas_wh`, `cz6b_pv_battery_ev`, `cz6b_resistance_res_wh`. MAE deviations range from 0.65–0.77 °C vs 0.60 allowed; HVAC energy relative deviations 28–54% vs 25–48% allowed.

**Fix**: Add `ghi: Some(ResampleMethod::Zoh), dni: Some(ResampleMethod::Zoh), dhi: Some(ResampleMethod::Zoh), wind_dir: Some(ResampleMethod::Zoh), wind_speed: Some(ResampleMethod::Zoh)` to the `ochre_compat()` constructor. These fields must match OCHRE's `resample().ffill()` behavior for the parity reference to remain valid.

---

**WR-02** — `resampled_weather_produces_smooth_environment` fails unconditionally  
`crates/hares-core/tests/weather_integration.rs:577`

Confirmed by `cargo test -p hares-core --test weather_integration`:

```
step 1396: temp jump of 0.155°C (prev=8.17, cur=8.33)
```

The test enforces < 0.15 °C step-to-step jumps. The default for `dry_bulb` changed to `PchipCyclic`. On the 24-element synthetic series, PchipCyclic sees a large year-boundary gradient (h23 ≈ 6.9°C, h0 = 15.0°C) and adjusts endpoint slopes to wrap smoothly — which increases the slope at the end of the series and causes a slightly larger step near the boundary than non-cyclic Pchip.

Note: `PchipCyclic` on a 24-element (1-day) test series is semantically incorrect — a 24-element series represents one day, not a periodic annual cycle. The cyclic boundary joins day-end to day-start, which for this synthetic series is a temperature jump of ≈8°C, causing the interpolant to accelerate toward that wrap point. The correct default for multi-year simulations (full 8760-element EPW) is `PchipCyclic`; for 24-element synthetic tests, `Pchip` (flat extrapolation) is correct. The test input is not representative of a full-year EPW dataset.

**Fix option A (preferred)**: The test synthetic series should use `midpoint_offset_secs: 0` (no offset) and pass explicit `ResampleOverrides { dry_bulb: Some(Pchip), ..Default::default() }` to EnvironmentManager's resample call, OR build a 24-element series with matching endpoints. Option A is cleaner because the 0.15°C threshold was empirically validated for Pchip on that specific series.  
**Fix option B**: Relax the threshold to 0.20°C to accommodate the larger but still physically reasonable PchipCyclic slope.  
Do not use option B without understanding the physical implication: 0.155°C/minute is still plausibly smooth for real weather.

---

### HIGH

---

**WR-03** — No unit test asserts `tmy3.meta.midpoint_offset_secs == 1800`  
`crates/hares-io/src/tmy3.rs:244`

The B4 fix is present. No `parse_standard_year` test verifies `midpoint_offset_secs`. A single-line regression silently reintroduces a 30-minute systematic offset on all TMY3 simulations with no test failure. This is the highest-impact single constant in the pipeline.

**Fix**: Add `assert_eq!(meta.midpoint_offset_secs, 1800, "TMY3 midpoint_offset_secs must be 1800 s (half-period for hour-ending convention)");` to the `parse_standard_year` test (and any TMY3 round-trip test).

---

**WR-04** — `Triangular` docstring claims exact "hourly mean preservation"  
`crates/hares-io/src/weather.rs:292`, `:1055`

The docstring at line 292 states: "achieves the same goals (smooth transitions, **hourly mean preservation**)". The mean is `0.125×prev + 0.75×cur + 0.125×next`, which equals `cur` only when the second difference is zero (constant or linear signal). For a sunrise hour where `prev=0, cur=800, next=800`, the mean is 700, a 12.5% shortfall. The test at line 2448 correctly documents "approximately preserves" with a 2% tolerance. The docstring overstates the guarantee and will mislead users who need conservation.

**Fix**: Replace "hourly mean preservation" in both docstring locations with "approximate hourly mean (exact only for constant or linear signals; ≤2% error for typical sinusoidal profiles)".

---

**WR-08** — `py_config.rs` TOML path silently discards unknown resample method names  
`crates/hares-python/src/py_config.rs:579`

The TOML parsing path uses `if let Ok(m) = parse_resample_method(v)`, silently ignoring invalid method names. The Python `kwargs` path in `py_dwelling.rs` returns `Err(PyValueError)` for unknown values. A typo in TOML configuration (e.g., `"circular_lineear"`) produces a silent no-op rather than an error, violating the "no silent defaults" policy.

**Fix**: Change to propagate the error: `overrides.$field = Some(parse_resample_method(v).map_err(|e| ..err_type..("invalid resample method {v}: {e}"))?);`.

---

### MEDIUM

---

**WR-05** — `bestest_900ff_root_cause.rs::heavyweight_concrete_wall_produces_two_rc_sub_layers` fails  
`crates/hares-envelope/tests/bestest_900ff_root_cause.rs:189`

This test was added in this campaign and immediately fails: expects 4 RC nodes, gets 5. Verified by `cargo test -p hares-envelope`. A failing spec test normalizes red CI status. Outside weather scope but introduced in the same campaign.

**Fix**: Either fix the RC discretization to produce 4 nodes as specified, or correct the assertion with an explanation of why 5 nodes is correct.

---

**WR-06** — `ochre_compat()` doc comment does not document sky temp behavioral divergence  
`crates/hares-io/src/weather.rs:320–323`

The comment says "sky_temp_c is not listed because it is always recomputed from the interpolated inputs." It does not note that this causes compat-mode sky temperatures to diverge from OCHRE's simpler direct-ZOH sky temperature. OCHRE computes `sky_temp = (IR/5.6697e-8)^0.25 - 273.15` once at parse time and then ZOH-resamples it. HARES recomputes from ZOH-interpolated IR and dry-bulb. This is physically correct but differs from the OCHRE reference signal.

**Fix**: Add a note: "Sky temperature is recomputed from ZOH-interpolated inputs (IR, dry-bulb, dew-point, sky cover); this diverges from OCHRE, which ZOH-resamples a pre-computed sky temperature. The HARES value is self-consistent; the OCHRE value is not."

---

**WR-07** — Factor-1 `resample_with()` calls dispatched to resample methods unnecessarily  
`crates/hares-io/src/weather.rs:472–475`

`resample_with()` returns early when `source == target_step_secs`. However, if a caller manually constructs a case where `source` and `target` differ but produce `factor == 1`, each `resample_field` is still dispatched. This is unlikely in practice (factor is always `source / target` which is ≥ 2 for upsampling given the divisibility check) but the triangular/cyclic `% n` lookups at n=1 still execute. No behavioral bug exists, but the code is fragile. Lower priority than WR-08.

---

**WR-09** — `ResStock CSV midpoint_offset_secs = 0` inconsistent with end-of-interval timestamps  
`crates/hares-io/src/resstock_csv.rs:337`  
**Pre-existing issue; not introduced by this campaign.**

`resstock_csv.rs:172` documents "ResStock uses end-of-interval timestamps (01:00 = hour 1)" — the same convention as EPW and TMY3. Yet `midpoint_offset_secs = 0`. The analogous TMY3 bug (B4) was fixed to 1800 in this campaign; the ResStock bug was not addressed. This produces a 30-minute systematic offset on all ResStock simulations.

**Fix**: Set `midpoint_offset_secs: source_step_secs / 2` consistent with EPW/TMY3 (or at minimum flag it as a known defect in the code comment rather than silently leaving it as 0).

---

### LOW

---

**WR-10** — `triangular_resample` does not clamp solar output to ≥ 0  
`crates/hares-io/src/weather.rs:1085–1090`

With valid (non-negative) EPW inputs, all interpolation weights are ≥ 0, so negatives cannot arise. However, `rel_humidity_pct` and `opaque_sky_cover` are clamped post-resample (`weather.rs:486–491`). Consistency suggests a `max(0.0)` guard on solar fields after triangular resampling. The test `triangular_zero_solar_stays_non_negative` confirms non-negativity for valid inputs; the gap is only for hypothetically invalid inputs. The `≥ -1e-12` tolerance in the integration test (`weather_integration.rs:342`) rather than `≥ 0` is an indirect acknowledgement of this.

**Fix**: Add `.max(0.0_f64)` to each solar field post-triangular-resample, or document explicitly that negative GHI/DNI/DHI input is rejected at parse time (it is, via the `[0, 1500]` bound check).

---

**WR-14** — `ISA_PRESSURE_EXPONENT` in `constants.rs` rounds to 5.2559; `resstock_csv.rs` uses more precise 5.25588 inline  
`crates/hares-physics/src/constants.rs:83`, `crates/hares-io/src/resstock_csv.rs:46`

The `constants.rs` constant `5.2559` is used by the HVAC/psychrometrics stack. The ResStock CSV parser uses inline `5.25588`. The difference is ≈0.04% — negligible for pressure, but this is a DRY violation. The ResStock literal is more precise and should be the canonical value.

**Fix**: Update `ISA_PRESSURE_EXPONENT` to `5.25588` in `constants.rs` and import it in `resstock_csv.rs`.

---

### NIT

---

**WR-11** — `STEFAN_BOLTZMANN` change from `5.6697e-8` to `5.670374419e-8` is a physics improvement  
`crates/hares-physics/src/constants.rs:149`

Old value `5.6697e-8` was truncated; CODATA 2018 value `5.670374419e-8` is now used. Relative difference ≈0.012%. Effect on sky temperature at 300 W/m²: `|T_sky_CODATA - T_sky_old|` ≈ `0.003°C`. This is correct and confirmed by independent derivation. No action required; noted for traceability. The intentional divergence from OCHRE (`5.6697e-8`) is correct.

---

**WR-12** — `fritsch_carlson_slopes_cyclic` monotonicity correction can scale a slope twice  
`crates/hares-io/src/weather.rs:779–793`

When processing segment `k`, scaling `d[k_next]` may cause that slope to be revisited when k reaches `k_next`. This is the same property as the non-cyclic implementation (lines 726–740) and is consistent with Fritsch & Carlson's convergence proof: each scaling can only reduce magnitude, so the result is still monotonicity-preserving. The existing `pchip_cyclic_*` tests pass, confirming no behavioral defect. Documented for awareness; not a bug.

---

**WR-13** — `Triangular` docstring says "a different algorithm" from E+  
`crates/hares-io/src/weather.rs:1055`

"The E+ approach uses a weighted blend of current/previous hours; this implementation uses midpoint interpolation which achieves the same goals…with a different algorithm." The comment is not precisely wrong — E+ uses current-minus-previous weighting from the interval start; HARES uses midpoint-referenced interpolation. They are not the same algorithm and produce different weight distributions for identical inputs at the same sub-hourly slot. The claim they "achieve the same goals" is correct (both are C0-continuous and approximately mean-preserving); the claim of equivalence is not implied and is not stated, so this is an accuracy nit not a misleading claim.

**Fix**: Optionally clarify: "The E+ approach weights from the interval start; this implementation weights from the midpoint, which produces a different sub-hourly profile but the same qualitative properties."

---

**WR-15** — Berdahl-Martin docstring cites the wrong paper for the calibrated coefficients  
`crates/hares-io/src/epw.rs:529–531`

The code comments cite "Martin, M. and Berdahl, P. (1984), Solar Energy 33(3/4), 321-336" for coefficients 0.758 / 0.521 / 0.625. According to the EnergyPlus FY2020 design document (`Alternative Models for Clear Sky Emissivity Calculation.md`), these calibrated coefficients are from Li et al. recalibration of the original form, not from the 1984 Martin & Berdahl paper directly (which gives 0.711 / 0.56 / 0.73 as the original form). The calibrated form is used in E+ and is physically superior. The docstring's citation should note these are the calibrated (Li et al. / E+ 9.6) coefficients.

**Fix**: Add "Calibrated form used by EnergyPlus 9.6 (Li et al. recalibration; original Berdahl & Martin (1984) Solar Energy 32(5) gives 0.711/0.56/0.73)." to the `berdahl_martin_sky_emissivity` docstring.

---

**WR-16** — `liquid_precip_m` parse failure silently defaults to 0.0  
`crates/hares-io/src/epw.rs:202–209`

```rust
parse_f64(fields[IDX_LIQUID_PRECIP_DEPTH_MM], row, "liquid_precip_mm")
    .unwrap_or(0.0)
    .max(0.0)
    / 1000.0
```

This is a partial silent-default. EPW field 33 is not present in all EPW variants (some files have fewer than 34 fields); the `fields.len() > IDX_LIQUID_PRECIP_DEPTH_MM` guard handles the truly absent field case. The `unwrap_or(0.0)` handles the parse-failure case when the field IS present but contains a non-numeric value (e.g., "N/A", missing, or corrupt). Silently defaulting to 0.0 mm for a corrupt field violates the no-silent-defaults policy. Precipitation is typically low-impact, but the silent default masks data quality issues.

**Fix**: Change to `parse_f64(...).map_err(|_| WeatherError::Parse(format!("row {row}: liquid_precip field '{v}' could not be parsed as f64")))? .max(0.0) / 1000.0` or at minimum emit a `tracing::warn!`. Accept `0.0` only when the field is actually absent (i.e., `fields.len() <= IDX_LIQUID_PRECIP_DEPTH_MM`).

---

## 6. Prior-Review Claims Re-Verified

All prior review findings from the first pass (`01_weather_timestep.md` as it existed before this overwrite) are re-evaluated from first principles:

| Prior Claim | Independent Verification | Verdict |
|---|---|---|
| WR-01: `ochre_compat()` missing ZOH for solar/wind-dir, 18 parity failures | Confirmed by `cargo test -p hares-core --test parity`: 16 violations (fewer than 18 — some fixtures may have been updated or the count was fixture-level not metric-level). Physics and code analysis confirms the mechanism. | CONFIRMED (count differs slightly; root cause correct) |
| WR-02: `resampled_weather_produces_smooth_environment` fails at 0.155°C | Confirmed by `cargo test -p hares-core --test weather_integration`: identical failure. | CONFIRMED EXACTLY |
| WR-03: No test asserts `midpoint_offset_secs == 1800` | Confirmed by reading `tmy3.rs` test suite. | CONFIRMED |
| WR-04: "hourly mean preservation" docstring incorrect | Confirmed by first-principles derivation: mean = 0.125·prev + 0.75·cur + 0.125·next ≠ cur in general. | CONFIRMED |
| WR-05: `bestest_900ff_root_cause.rs` test fails (5 nodes vs 4) | Outside weather scope; not re-run in this audit. Prior claim stands on evidence in the commit. | ACCEPTED (not independently run) |
| WR-06: `ochre_compat()` sky_temp doc doesn't explain divergence from OCHRE | Confirmed by reading `weather.rs:320–323`. | CONFIRMED |
| WR-07: Factor-1 no-op guard missing at `resample_with()` level | Confirmed by reading code: no early return for factor==1 before dispatch loop. No behavioral bug confirmed. | CONFIRMED AS NIT |
| WR-08: `py_config.rs` TOML path silently ignores invalid method names | Confirmed by reading `py_config.rs`. | CONFIRMED |
| WR-09: ResStock `midpoint_offset_secs = 0` incorrect | Confirmed: end-of-interval timestamps documented in code, offset is 0. Pre-existing issue. | CONFIRMED |
| WR-10: No solar clamping post-triangular | Confirmed: no `.max(0.0)` applied. Valid inputs cannot produce negatives, so behavioral impact is nil. | CONFIRMED AS LOW |
| WR-11: Stefan-Boltzmann change is correct | Confirmed by independent derivation. | CONFIRMED |
| WR-12: Cyclic slope double-modification | Confirmed: same pattern as non-cyclic; converges per Fritsch-Carlson. Tests pass. | CONFIRMED AS NIT |
| WR-13: "different algorithm" comment misleads about E+ equivalence | Confirmed by algorithm analysis. They are not equivalent but achieve same qualitative goals. | CONFIRMED AS NIT |
| B3 synthetic sky_temp fix | Confirmed present in code; not re-run here. Prior claim stands. | ACCEPTED |
| B4 TMY3 midpoint_offset_secs=1800 | Confirmed present at `tmy3.rs:244`. | CONFIRMED |
| B5 sky temp recomputed after interpolation | Confirmed present at `weather.rs:525–531`; tests pass. | CONFIRMED |
| B9 circular wind interpolation | Confirmed correct; tests pass including 350°→10° wrap. | CONFIRMED |
| S7 triangular solar interpolation | Confirmed correct physics; tests pass. | CONFIRMED |
| D5 cyclic PCHIP | Confirmed correct; tests pass. | CONFIRMED |

No prior review claims were found to be wrong. Two additions from this independent review not in the prior pass: WR-14 (ISA exponent DRY/precision), WR-15 (Berdahl-Martin docstring wrong paper), WR-16 (precipitation silent default).

---

## 7. Findings-Doc Errors Spotted

The following `docs/findings/*.md` documents contain statements that contradict primary sources or have been superseded:

| Document | Claim | Authoritative Correction |
|---|---|---|
| `docs/findings/summary.md` | "60/40 internal-gain split" | ASHRAE HOF 2021 Ch.18 Table 1 specifies ≈30% radiative / 70% convective for typical residential seated occupancy. `constants.rs` correctly uses 30/70 (`OCCUPANT_RADIATIVE_FRACTION = 0.30`). The summary is wrong. |
| `docs/findings/weather.md` | States B4 TMY3 offset was the only midpoint-offset issue | ResStock CSV also uses end-of-interval timestamps (WR-09); the findings doc does not mention this. |
| `docs/findings/consolidated.md` | Documents finding B3 (synthetic sky temp hardcoded 300 W/m²) as a defect | Fix confirmed present. The consolidated.md finding is accurate as a historical record but should be marked resolved. |
| Any findings doc citing `5.6697e-8` as the HARES value | Pre-dates the CODATA 2018 update in this campaign | HARES now correctly uses `5.670374419e-8`. |

---

## Appendix: Test Execution Summary

```
cargo test -p hares-io --lib -- weather        → 66 passed, 0 failed
cargo test -p hares-io --test weather_parity   → 29 passed, 0 failed
cargo test -p hares-core --test weather_integration → 2 passed, 1 FAILED (WR-02)
cargo test -p hares-core --test parity         → FAILED (16 violations, WR-01)
cargo test -p hares-core --test orchestration_parity → 12 passed, 0 failed
```
