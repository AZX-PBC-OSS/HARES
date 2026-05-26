# Parity test tolerance definitions: physically justified or hiding defects?
**Review ID**: test-03
**Category**: tests
**Date**: 2026-05-26

## Files Reviewed
- `tests/parity/tolerance.rs` (134 lines, all tolerance constants and comparison helpers)
- `tests/parity/mod.rs` (809 lines, parity test driver, fixture override table, metric aggregation)
- `tests/parity/corpus.rs` (94 lines, fixture discovery and required-files list)
- `tests/conditioned_oracle.rs` (1042 lines, sibling 48–72 h conditioned-oracle test, compare/contrast)
- `tests/fixtures/parity/README.md` (52 lines, fixture corpus documentation)
- `tests/python/generate_parity_reference.py` (191 lines, OCHRE reference generation script)

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Dwelling.py` — OCHRE simulation entry point; timestep constraints (line 154: non-ideal equipment requires `time_res < 15 min`)
- `vendors/OCHRE/ochre/Models/RCModel.py` — OCHRE RC state-space model; `transform_floating_node` star-mesh (line 15–50); `create_rc_matrices` (line 84–109)
- `vendors/OCHRE/ochre/Models/Envelope.py` — OCHRE envelope construction; film resistances frozen at init (line 348–349); linearized interior LWR (line 1048–1061)
- `vendors/OCHRE/ochre/utils/envelope.py` — OCHRE `create_rc_data()` boundary RC chain assembly (line 294–339)
- `vendors/OCHRE/ochre/utils/schedule.py` — OCHRE weather resampling: `df.resample().ffill()` (ZOH) at line 550–553
- `vendors/EnergyPlus/src/EnergyPlus/ConvectionCoefficients.cc` — EnergyPlus recomputes `h_conv` every timestep via TARP (unlike HARES/OCHRE which freeze at init)
- `vendors/EnergyPlus/third_party/kiva/test/unit/foundation.unit.cpp` — Kiva BESTEST foundation test cases (GC30a/b/c, GC60b, GC65b)
- `tests/fixtures/parity/ochre_rc_reference.json` — OCHRE RC network reference data extracted by `extract_ochre_rc.py`
- `tests/fixtures/parity/ashrae_rc_reference.json` — Independently derived ASHRAE HoF RC reference values
- `docs/reviews/envelope/envelope-12-bestest-tolerance-widening.md` — Prior finding: BESTEST 2R1C model bypasses production RC pipeline; ~10% tolerance used as per-component guardrail
- `docs/reviews/equipment-hvac/equip-hvac-09-ideal-hvac-back-solve.md` — Prior finding: ideal capacity back-solve correctness review
- `docs/findings/reviews/02_rc_envelope_solver.md` — Prior finding: 80% peak power tolerance (now 20%) was normalizing a known defect
- `crates/hares-io/src/weather.rs` — `ResampleOverrides::ochre_compat()` (line 412–427); `ResampleMethod` enum (line 296–373)
- `crates/hares-envelope/src/thermal_solver/stepping.rs` — `autosize_capacity()` DC-gain fix (line 24–66), T-0123 reference

## Findings

### Finding 1: [Severity: critical] Parity fixtures are all 1-hour simulations, yet tolerances are named and described as if applied to annual/long-duration data
**Description**: All 11 parity fixture `config.toml` files specify `duration = 3600` (1 hour) at `time_res = 60` (1-minute resolution). This means every tolerance defined in `tolerance.rs` operates on at most 60 data points covering a single hour of a single day. The tolerances are described using language implying meaningful energy comparisons: "annual water heater energy" (line 18), "peak HVAC power" (line 20), "total site energy" (line 19). In reality, a 1-hour window captures at most one compressor cycle (typical residential duty cycle ~50%, with compressor minimum run-time of 3–10 minutes), and the energy values being compared are the integral of a single on-cycle or fractional on-cycle. The tolerances are therefore not testing energy prediction accuracy — they are testing cycle-level phase alignment between two simulators whose thermostat logic differs.

**Code Location**: `tests/parity/tolerance.rs:9–29` (all constants); `tests/fixtures/parity/*/config.toml` (all specify `duration = 3600`); `tests/parity/mod.rs:472–533` (energy comparisons calling `annual_energy_for_prefixes` on 60-step series)

**Root Cause**: The fixture corpus was scoped to 1-hour "short-window" dynamic-cycling tests (as documented in tolerance.rs:1 — "1-hour dynamic cycling windows" and the README.md:50–51 — "Short-window (≤1 h) bands are intentionally looser than the 24–72 h conditioned-oracle suite"). The energy comparison metric names (`annual_energy_for_prefixes`, `ANNUAL_WATER_HEATER_ENERGY_REL_PCT_MAX`) were never renamed when the fixtures were reduced to 1-hour runs. Only one fixture (`resstock_bldg0112631_24h`) has a 24-hour duration, but it too has no `reference_output.parquet` file.

**Impact**: Users reading the tolerance definitions will believe HARES achieves 0.5% parity on annual water heater energy, 25% on HVAC energy, and 20% on peak power. None of these are comparable to annual energy error norms used in ASHRAE 140 (which specify ±10% on annual load, ±15% on peak, ±1°C on annual mean temperature over a full-year simulation). The tolerances are defending cycle-level timing artifacts, not model accuracy. This creates a false sense of validation completeness and masks the fact that no multi-day or annual parity tests exist.

**Recommendation**: Either (a) extend fixtures to meaningful durations (≥24 h for energy, ≥1 year for annual metrics) and adjust tolerances to reflect published validation norms, or (b) rename all constants and metrics to unambiguously indicate the short-window scope (e.g., `SINGLE_CYCLE_HVAC_ENERGY_PHASE_OFFSET_REL_PCT_MAX`), and add a prominent warning that these are NOT accuracy tolerances.

### Finding 2: [Severity: critical] No `reference_output.parquet` files exist — parity tests never run
**Description**: `tests/parity/corpus.rs:5–11` defines `REQUIRED_FILES` including `"reference_output.parquet"`. Fixture discovery at `corpus.rs:64–67` checks for all required files. All 11 fixture directories under `tests/fixtures/parity/` are missing `reference_output.parquet`. At `tests/parity/mod.rs:97` incomplete fixtures are skipped with an `eprintln!`. The parity test body at line 110–113 will print "no complete fixtures found" and return without testing anything. This means the entire parity test suite — with all its tolerances — is dead code in practice.

**Code Location**: `tests/parity/corpus.rs:5–11` (REQUIRED_FILES); `tests/parity/mod.rs:97–113` (incomplete fixture skip logic); all 11 fixture directories under `tests/fixtures/parity/`

**Root Cause**: Reference data must be regenerated by running `uv run --group ochre python tests/python/generate_parity_reference.py`. The script exists and is functional, but reference data has not been committed or regenerated. This may be intentional (the fixtures require OCHRE as a dependency, and OCHRE may fail on some fixtures) or an oversight.

**Impact**: The parity test suite is a ghost — all tolerance definitions, the fixture override mechanism, the 8-metric comparison pipeline, and 800+ lines of comparison logic in `tests/parity/mod.rs` are completely untested. The CI pipeline at `.github/workflows/ci.yml` does not even run `cargo test`, so even if reference data existed, no CI would exercise these tolerances. There is zero automated feedback on whether HARES is getting closer to or further from OCHRE outputs.

**Recommendation**: Either regenerate and commit reference data, or remove the dead parity test scaffold and acknowledge that short-window OCHRE comparison is not yet operational. If keeping, add `reference_output.parquet` files to the repository (or a documented regeneration step in CI).

### Finding 3: [Severity: high] 25% short-window HVAC and site energy tolerances are explicitly masking single-cycle phase offsets, not model errors
**Description**: The docstring at `tolerance.rs:11–16` states: "Single-cycle phase offsets between HARES and OCHRE routinely shift integrated energy by 5–25 % for fixtures whose duration barely exceeds the on-time of one compressor cycle. Tighter bands require ≥24 h windows." This admission that the 25% tolerance is sized to accommodate phase offsets means the tolerance is masking a timing/control difference, not measuring physical model accuracy. For a 1-hour fixture, whether the compressor happened to be ON at minute 1–10 or OFF at minute 1–10 (due to different thermostat initializations or on/off logic) can change the 1-hour energy by the entire cycle energy — not just 25% but potentially 100%.

The identical 25% tolerance for `SHORT_WINDOW_TOTAL_SITE_ENERGY_REL_PCT_MAX` (line 19) propagates this same accommodation to the total electric power metric, which is dominated by HVAC in most fixtures. This means a systematic 20–24% energy error in the thermal solver would pass unnoticed.

The `fixture_override` at `tests/parity/mod.rs:52–53` widens tolerances further for `cz2a_pv_ev` to HVAC=48% and total site=43%, explicitly documenting that the step-0 back-solve defect drives the divergence. The comment at line 39–42 states "Once that back-solve is aligned the overrides should drop back to the defaults" — but there is no detection mechanism for when alignment occurs (see Finding 4).

**Code Location**: `tests/parity/tolerance.rs:11–17` (SHORT_WINDOW_HVAC_ENERGY); `tests/parity/tolerance.rs:19` (SHORT_WINDOW_TOTAL_SITE_ENERGY); `tests/parity/mod.rs:36–55` (fixture_override expanding HVAC to 48%, site to 43%)

**Root Cause**: The parity test design chose 1-hour fixtures despite the documented reality that single-cycle phase offsets dominate 1-hour energy integrals. Rather than extending fixture duration to ≥24 h (as the conditioned_oracle does with its 72 h spring test at line 654, which achieves 15% HVAC energy tolerance), the tolerances were inflated to accommodate the short windows.

**Impact**: These tolerances cannot detect systematic equipment modeling errors smaller than 25% (or 48% for cz2a_pv_ev). A regression that introduces a 20% bias in HVAC capacity would pass. A bug that causes the water heater to run 15% longer would pass. The test provides near-zero discrimination for energy accuracy.

**Recommendation**: Replace short-window energy tolerances with longer-duration fixtures (≥24 h) and tighter percentage bounds (≤15%), as the conditioned_oracle test already demonstrates is feasible. Keep short-window fixtures only for cycle-counting and phase-offset analysis, not energy accuracy.

### Finding 4: [Severity: high] No mechanism exists to detect parity improvement and tighten tolerances
**Description**: The review instructions ask: "As defects are fixed, tolerances should narrow — but is there a mechanism to detect that parity improved and tighten the tolerance accordingly?" The answer is no. There is:

1. No CI job that runs parity tests (`.github/workflows/ci.yml` only runs `cargo fmt` and `cargo clippy` — lines 1–35)
2. No tolerance value history or regression comparison
3. No automated script to tighten tolerances when actual deviations drop below a threshold
4. No `#[test]` that asserts tolerance values are decreasing over time
5. No versioned tolerance snapshots
6. No "golden file" approach that would automatically detect when tolerances can be narrowed

The `fixture_override` table at `tests/parity/mod.rs:43–55` is the closest thing to a tolerance-narrowing mechanism: it has a docstring saying to drop overrides "once the back-solve is aligned." This is a manual, developer-memory-dependent process. There is no test that fails if an override's tolerance exceeds the default tolerance — i.e., the codebase cannot alert developers that overrides still exist and need to be removed.

**Code Location**: `.github/workflows/ci.yml:1–35` (no test execution); `tests/parity/mod.rs:36–56` (manual fixture_override table); `tests/parity/tolerance.rs:1–29` (static constants with no comparison baseline)

**Root Cause**: The parity test infrastructure was built as a manual developer tool, not as an automated regression suite. Tolerance values are hand-maintained constants with comments describing when they should change, but no automated enforcement of those comments.

**Impact**: Known defects (the step-0 back-solve, RC node ordering, film coefficient differences) that are currently masked by wide tolerances will remain masked indefinitely. When developers fix these defects, the tolerances will not automatically narrow — the improved accuracy will be invisible to the test harness unless a developer manually opens `tolerance.rs` and edits the constant. This creates a "set and forget" dynamic where tolerances ratchet up (via fixture overrides for known regressions) but never ratchet down.

**Recommendation**:
1. Add CI test execution (at minimum `cargo test --test parity` when reference data exists).
2. Store tolerance values in the fixture config (not global constants) so each fixture can state its expected improvement trajectory.
3. Add a CI check that fails if actual deviations are well within tolerance bounds (e.g., < 20% of allowed tolerance), suggesting the tolerance should be tightened.
4. Add a check that all fixture_override values are strictly ≤ the corresponding default tolerance value — this would fail immediately if an override is wider than the default, alerting developers to the outstanding defect.
5. Write a regression test that records the current deviation values and fails if they increase, providing a "ratchet" on accuracy.

### Finding 5: [Severity: high] `relative_percent_deviation` has a near-zero denominator with `f64::EPSILON` threshold, allowing spurious deviations for trivially small reference values
**Description**: The `relative_percent_deviation` function at `tolerance.rs:122–133` uses `if denom <= f64::EPSILON` to guard against division by zero. `f64::EPSILON` is ~2.2e-16. This means if the reference value is, say, `5e-10` (which is > f64::EPSILON and passes the guard), the relative deviation becomes enormous due to numerical noise in the HARES output. For example, if OCHRE reports 5e-15 kWh of water heater energy (essentially zero because the water heater didn't fire), and HARES reports 1e-13 kWh (also essentially zero), the relative deviation is `|1e-13 - 5e-15| / 5e-15 * 100 = 1,900%` — clearly spurious.

By contrast, the `conditioned_oracle.rs` comparison at line 226 uses a physically meaningful absolute threshold: `if ochre_mean.abs() > 1.0 { ... } else { f64::INFINITY }`. This threshold (1.0 W) prevents noise-level values from producing meaningless percentage deviations.

The `relative_percent_deviation` function is used by three metrics:
- `SHORT_WINDOW_HVAC_ENERGY` (line 499–505): HVAC energy summed over 1-hour
- `ANNUAL_WATER_HEATER_ENERGY` (line 513–519): water heater energy summed over 1-hour
- `SHORT_WINDOW_TOTAL_SITE_ENERGY` (line 527–533): total site energy summed over 1-hour
- `PEAK_HVAC_POWER` (line 538–544): peak HVAC kW
- `EQUIPMENT_MODE_CYCLES` (line 565–572): integer cycle count cast to f64

Of these, water heater energy and equipment mode cycles are most vulnerable: a 1-hour fixture in spring may have zero water heater firing, producing energy values at or near zero in both models, yet the current threshold would let a meaningless relative deviation through.

**Code Location**: `tests/parity/tolerance.rs:122–133` (relative_percent_deviation function); `tests/parity/tolerance.rs:124` (f64::EPSILON threshold); `tests/conditioned_oracle.rs:226` (better approach using absolute > 1.0 W threshold)

**Root Cause**: The guard was written with a purely numerical concern (avoiding division by zero) rather than a physical concern (avoiding comparisons where the signal is below the noise floor). There is no check that the reference value is physically meaningful before computing a percentage.

**Impact**: Water heater energy and equipment cycle comparisons can report spurious failures or passes depending on whether minute-to-minute numerical noise happens to hit the right side of `f64::EPSILON`. Since no reference data exists (Finding 2), this has not been observed, but it will appear as "flaky" test results when data is generated.

**Recommendation**: Replace `f64::EPSILON` with a physically meaningful threshold. For energy metrics, use 1e-6 kWh (0.001 Wh, well below any equipment minimum output). For cycle counts, use 1.0 (at least one cycle must have occurred). Alternatively, adopt the `conditioned_oracle.rs` pattern of an absolute tolerance floor + a percentage tolerance ceiling, with relative comparison only kicking in above an absolute magnitude threshold.

### Finding 6: [Severity: medium] `ANNUAL_WATER_HEATER_ENERGY_REL_PCT_MAX` (0.5%) is named as "annual" but applied to 1-hour fixture data
**Description**: The constant at `tolerance.rs:18` is named `ANNUAL_WATER_HEATER_ENERGY_REL_PCT_MAX` and set to 0.5%. A 0.5% bound would be extremely tight for annual energy comparison (ASHRAE 140 allows ~10–15% on annual load). The metric is applied through `annual_energy_for_prefixes()` at `tests/parity/mod.rs:508` which sums the water heater power series over the available timesteps (60 steps for a 1-hour fixture) and multiplies by `MINUTE_STEP_HOURS = 1/60`. For a 1-hour fixture, the result is the kWh consumed in that single hour — not annual energy. The function name `annual_energy_for_prefixes` is misleading because it does not annualize; it merely integrates over whatever window the data covers.

A water heater in a 1-hour fixture may not fire at all (producing zero or near-zero energy in both models, see Finding 5), or may fire once (producing a single cycle's worth of energy). The 0.5% tolerance is so tight that any phase offset of even a few seconds in the water heater thermostat would cause a failure — yet phase offsets are exactly what the HVAC energy tolerances are designed to accommodate (see Finding 3).

**Code Location**: `tests/parity/tolerance.rs:18` (constant definition); `tests/parity/mod.rs:508–519` (water heater comparison calling `annual_energy_for_prefixes`); `tests/parity/mod.rs:621–650` (annual_energy_for_prefixes implementation, which does NOT annualize)

**Root Cause**: The constant name implies annual scope but the fixture design is 1-hour. Either the intent was to have annual fixtures that were never created, or the naming convention was adopted without adjustment when fixture duration was shortened.

**Impact**: The 0.5% tolerance is simultaneously too tight (for 1-hour cycle-level comparison where phase offsets dominate) and meaningless (because it's not comparing annual energy). No credible energy model achieves 0.5% annual water heater parity against any reference. The constant creates confusion about what is actually being validated.

**Recommendation**: Rename to `SHORT_WINDOW_WATER_HEATER_ENERGY_REL_PCT_MAX` or similar. Set the value to reflect the actual comparison window scope (e.g., 25% to match the HVAC window tolerance, since water heater cycling has the same phase-offset problem). Alternatively, extend fixtures to cover full days and apply an annual-calibrated tolerance.

### Finding 7: [Severity: medium] `EQUIPMENT_MODE_CYCLE_COUNT_REL_PCT_MAX` (5.0%) is applied to integer counts over a 1-hour window
**Description**: The equipment mode cycle count at `tests/parity/mod.rs:691–703` counts transitions from mode 0 to non-zero mode across all equipment columns, and the result is a small integer (typically 0–3 cycles per equipment in a 1-hour window). The relative percentage deviation of this integer count is compared against `EQUIPMENT_MODE_CYCLE_COUNT_REL_PCT_MAX = 5.0%` at `tolerance.rs:29`. A 5% relative deviation on a small integer is mathematically impossible for most values:
- 0 cycles in both: 0% deviation (pass)
- 1 cycle vs 0 cycles: `|1-0|/0 * 100 = INFINITY` (fail), or 0 vs 1 same
- 1 cycle vs 1 cycle: 0% (pass)
- 2 vs 2: 0% (pass)
- 2 vs 1: `|2-1|/1 * 100 = 100%` (fail)
- 3 vs 2: `|3-2|/2 * 100 = 50%` (fail)

The only values that can pass a 5% relative tolerance are identical counts or zero/zero. Any off-by-one cycle count fails. This makes the tolerance a de facto equality check, but with a percentage label that suggests meaningful discrimination. There is no justification comment.

**Code Location**: `tests/parity/tolerance.rs:29` (5% constant); `tests/parity/mod.rs:691–714` (cycle counting logic, `count_mode_cycles` counts 0→nonzero transitions); `tests/parity/mod.rs:565–572` (comparison casting u64 to f64)

**Root Cause**: The relative-percent framework was designed for continuous energy values and was applied to discrete cycle counts without considering that relative percentages on small integers produce binary pass/fail behavior indistinguishable from equality.

**Impact**: The test will flag any difference in cycle count as a failure regardless of physical significance. A 1-cycle difference (e.g., thermostat deadband causing one extra or fewer cycle) fails the test, while the same deadband difference can cause a 25% energy deviation that passes the energy tolerance. This inconsistency means the cycle-count test is simultaneously too strict (any difference fails) and too permissive (it cannot detect systematic problems like double-counting or missed cycles because it checks only total count, not timing).

**Recommendation**: Replace the relative percentage tolerance with an absolute cycle-count tolerance (e.g., ±2 cycles for 1-hour window, ±5 for 24-hour). This acknowledges the discrete nature of the metric and provides a meaningful bound. Alternatively, compute cycle count correlation (do cycles occur at the same minutes?) rather than aggregate count.

### Finding 8: [Severity: medium] `BATTERY_SOC_MAE_ABS_MAX` (0.01) has no justification comment
**Description**: The constant at `tolerance.rs:28` is `BATTERY_SOC_MAE_ABS_MAX: f64 = 0.01` with no doc comment or justification. A 1% SOC MAE is reasonable for battery state-of-charge tracking — most battery models can maintain <1% drift over short durations — but there is no citation to battery modeling validation norms, no reference to the charge-counting accuracy of the underlying coulomb-counting or voltage-based SOC estimation, and no discussion of acceptable drift rates.

**Code Location**: `tests/parity/tolerance.rs:28`

**Root Cause**: Inherited from a presumably informed but undocumented engineering judgment.

**Impact**: Low for correctness (1% SOC is a defensibly tight bound). The absence of justification documentation makes it unclear whether this tolerance is grounded in sensor accuracy (commercial BMS SOC accuracy is typically ±5%), modeling accuracy (coulomb counting can achieve <1%), or engineering judgment. Future developers cannot assess whether tightening to 0.5% or loosening to 5% is appropriate.

**Recommendation**: Add a doc comment citing the physical justification (e.g., "Coulomb-counting SOC drift over 1 hour at 1C rate is <0.1% for high-precision ADCs; 0.01 is a conservative 100× margin" or similar).

### Finding 9: [Severity: low] Weather sky temperature diverges between HARES and OCHRE even in `ochre_compat()` mode
**Description**: The `ResampleOverrides::ochre_compat()` doc comment at `crates/hares-io/src/weather.rs:399–411` documents that "sky_temp_c will still diverge" because HARES recomputes sky temperature from interpolated weather inputs (dry-bulb, dew-point, opaque sky cover) while OCHRE forward-fills the source EPW file column directly. This is a known, documented difference. However, the parity test tolerances make no allowance for this divergence — sky temperature affects longwave radiation exchange with the sky, which influences zone temperatures, heating loads, and cooling loads. The resulting energy error is folded into the 25% blanket HVAC and site energy tolerances without specific accounting.

**Code Location**: `crates/hares-io/src/weather.rs:399–411` (sky temp divergence note); `crates/hares-io/src/weather.rs:412–427` (ochre_compat applies ZOH to all fields except sky_temp which is not overridable); `docs/tickets/126-ochre-compat-doc-sky-temp-divergence-note.md` (dedicated ticket)

**Root Cause**: HARES's weather pipeline always recomputes sky temperature from physical first principles (Walton model), while OCHRE uses the sky-temperature column directly from the EPW file (which may come from a different model, e.g., the original TMY3 derivation). The `ResampleOverrides` struct has no `sky_temp` field (line 380–381 comment: "sky_temp_c — NOT overridable, always recomputed from interpolated inputs").

**Impact**: The sky temperature divergence introduces an unquantified systematic bias in the parity comparison that is folded into the broad energy tolerances. Since the tolerances are already 25% and can't discriminate model improvements, this specific divergence is not independently detectable. If energy tolerances were tightened per Finding 3, sky-temperature divergence would become a measurable failure mode requiring either a sky-temp override or an acceptance of the residual.

**Recommendation**: Add a `sky_temp` field to `ResampleOverrides` with ZOH as the OCHRE-compat option, or accept that sky temperature will always diverge and document the expected magnitude (typically <2°C for clear-sky conditions per TMY3 vs Walton model comparison).

### Finding 10: [Severity: low] `fixture_override` tolerance widening for `cz2a_pv_ev` relies on developer memory, not automated regression
**Description**: The `fixture_override` table at `tests/parity/mod.rs:43–55` contains two entries for `cz2a_pv_ev` widening HVAC energy to 48% and total site to 43%. The comment at line 37–42 states: "Once that back-solve is aligned the overrides should drop back to the defaults." There is no test that asserts the overrides are not wider than defaults, no CI failure if the override exists, and no periodic audit that checks whether the deviation has shrunk enough to remove the override. The values 48% and 43% were set to "observed residual plus a 1% margin (no headroom beyond evidence)" — meaning they are exactly calibrated to make the current (broken) code pass. Any improvement in the back-solve will go unnoticed because the override will still pass with a wider margin. Any regression in the back-solve will also go unnoticed until it exceeds 48%/43%.

**Code Location**: `tests/parity/mod.rs:36–56` (fixture_override function and docstring)

**Root Cause**: The override mechanism has no "maximum override" bound and no alert when an override is active. It's a pure opt-out mechanism with no counter-pressure.

**Impact**: The fixture_override for cz2a_pv_ev can persist indefinitely even after the root cause is fixed. The 48% HVAC energy tolerance masks an order-of-magnitude defect in the step-0 back-solve (documented as 11× inflation of autosizing capacity in `autosize.rs:1196`).

**Recommendation**: Add a CI regression check that logs (or fails on) any active fixture_override, similar to `#[allow(dead_code)]` warnings. Add a maximum-permitted-override cap (e.g., override cannot exceed 2× default) to prevent unbounded tolerance inflation. Consider converting fixture overrides to `#[ignore]` annotations on individual fixtures rather than tolerance widening — this makes the skipped comparison visible in test output.

## Summary
- Total findings: 10
- Critical: 2 (Findings 1, 2)
- High: 3 (Findings 3, 4, 5)
- Medium: 4 (Findings 6, 7, 8, 9)
- Low: 1 (Finding 10)

## Recommendations

1. **Regenerate or acknowledge dead parity tests**: The parity test suite is non-functional because no `reference_output.parquet` files exist. Either regenerate them (via `generate_parity_reference.py`), commit them, and enable CI execution, or remove the scaffold and document that short-window OCHRE parity is aspirational.

2. **Extend fixture duration**: Replace 1-hour fixtures with ≥24-hour fixtures for energy metrics. The conditioned_oracle 72-hour test already demonstrates this is feasible and achieves 15% HVAC energy tolerance — 10× tighter than the parity test's 25%.

3. **Rename energy metrics to reflect actual scope**: `ANNUAL_WATER_HEATER_ENERGY_REL_PCT_MAX` must not be named "annual" when applied to 1-hour data. `annual_energy_for_prefixes()` must either annualize or be renamed.

4. **Implement automated tolerance tightening**: Add a CI step that records current deviations and alerts when they drop below 20% of the allowed tolerance. Add a pre-commit hook or CI check that fails when a `fixture_override` value exceeds the corresponding default tolerance.

5. **Fix the `relative_percent_deviation` near-zero guard**: Replace `f64::EPSILON` with physically meaningful absolute thresholds (≥1e-6 kWh for energy, ≥1.0 for cycle counts). Adopt the `conditioned_oracle.rs` dual-threshold approach (absolute floor + percentage ceiling).

6. **Add physical justification documentation**: Every tolerance constant that lacks a comment (`ZONE_TEMP_UNCONDITIONED_C_MAE_MAX`, `ANNUAL_WATER_HEATER_ENERGY_REL_PCT_MAX`, `BATTERY_SOC_MAE_ABS_MAX`, `EQUIPMENT_MODE_CYCLE_COUNT_REL_PCT_MAX`) must be documented with the physical basis for its value.

7. **Replace discrete cycle-count relative tolerance with absolute bound**: Integer cycle counts over 1-hour windows produce binary pass/fail at 5% relative tolerance. Switch to absolute tolerance (±N cycles) or timing correlation.

8. **Add sky temperature override to `ResampleOverrides`**: The documented sky-temp divergence between HARES and OCHRE should have a configurable override path so parity tests can match OCHRE's forward-filled sky temp when desired.

## References / Citations
- ASHRAE 140-2017, Table B8-2/B8-3a: ±1°C annual mean zone temperature, ±10% annual heating/cooling loads, ±15% peak loads for BESTEST Cases 600/900
- `tests/parity/tolerance.rs:9` — cites BESTEST ±1°C for zone temperature justification
- `tests/parity/tolerance.rs:13–16` — cites single-cycle phase offsets for 25% HVAC energy tolerance
- `tests/parity/mod.rs:37–42` — fixture_override doc cites step-0 ideal-capacity back-solve as the root cause of cz2a_pv_ev divergences
- `docs/findings/reviews/02_rc_envelope_solver.md:121–131` — classifies 80% peak power tolerance as normalizing known defect
- `crates/hares-core/src/dwelling/autosize.rs:1196` — T-0123 fix: "The cold-start back-solve bug... would dramatically inflate autosize capacity 11×"
- `crates/hares-io/src/weather.rs:399–411` — documented sky-temperature divergence in ochre_compat mode
- `docs/reviews/envelope/envelope-12-bestest-tolerance-widening.md` — BESTEST tolerance used as per-component guardrail for ~10% LWR error
- `tests/fixtures/parity/README.md:1–7` — "ballpark comparison point for HARES, not as a correctness oracle"
