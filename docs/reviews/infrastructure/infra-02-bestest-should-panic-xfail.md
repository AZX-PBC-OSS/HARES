# BESTEST `#[should_panic]` xfail pattern has soundness hazard

**Review ID**: infra-02
**Category**: infrastructure
**Date**: 2026-05-26

## Files Reviewed
- `tests/bestest/mod.rs`

## Vendor/Reference Files Consulted
- `docs/findings/consolidated.md` — error budget for 900FF, listing root causes (S4/S5, B1, B2, B6)
- `tests/bestest/reference_bands.rs` — ASHRAE 140 reference band definitions
- `tests/bestest/cases.rs` — BESTEST test case registry and fixture paths

## Findings

### Finding 1: [Severity: critical]

**Description**: The `bestest_case_900ff` test at `tests/bestest/mod.rs:142` uses `#[should_panic(expected = "metric=min_zone_temp_c")]` as an expected-failure (xfail) mechanism. While the Rust `expected` parameter does provide partial protection (the test FAILs if the panic message differs), the pattern remains a soundness hazard in the following ways:

1. **False PASS from coincidental panic messages**: If any code path in the simulation produces a panic message that happen to contain the substring `"metric=min_zone_temp_c"` — through a debug assertion, a formatting error, or a change to error-reporting code — the test PASSes even though the failure has nothing to do with the known min_zone_temp_c band violation.

2. **Does not verify the simulation actually completed**: The `#[should_panic]` only cares that a panic occurred with a matching message. It cannot distinguish between failure partway through the simulation (e.g., step 47 of 8760) and failure at the band-validation assertion.

3. **Does not verify the known failure's magnitude or direction**: The test cannot assert that the min_zone_temp_c value matches the documented outlier (+0.90°C with band upper bound of −1.6°C, outlier = +2.50°C). If a regression silently shifts the value to something entirely different (within or beyond the band), the test won't detect it.

4. **Allows non-min_zone_temp_c failures to go undetected**: The `expected = "metric=min_zone_temp_c"` substring matches as long as min_zone_temp_c is one of the failing metrics in the assertion block. If other metrics (e.g., peak_zone_temp_c) also fail, the test still PASSes — masking additional regressions.

**Code Location**: `tests/bestest/mod.rs:141-148`
```rust
#[test]
#[should_panic(expected = "metric=min_zone_temp_c")]
#[ignore = "pending remaining physics fixes: B1, S4/S5, and other consolidated.md items"]
fn bestest_case_900ff() {
    // Verifies the test still correctly detects the known min-temp exceedance.
    let case = core_cases().into_iter().find(|c| c.id == "900FF").unwrap();
    run_single_case(&case);
}
```

**Root Cause**: The `#[should_panic]` attribute was adopted as a shorthand xfail mechanism instead of writing an explicit assertion that checks for the known specific failure condition. The test currently carries BOTH `#[ignore]` and `#[should_panic]`, meaning the xfail semantics are active only when someone explicitly runs `cargo test bestest_case_900ff -- --ignored` (e.g., during debugging or physics validation). In that scenario, a user could be misled by a false PASS.

**Impact**:
- High risk of masking regressions during active physics debugging sessions (when `--ignored` tests are run).
- Low risk in CI, since the test is `#[ignore]`-gated and never executes in normal test runs.
- If the `#[ignore]` attribute were ever removed before fixing the underlying physics, the xfail pattern would become a permanent CI soundness hazard.

### Finding 2: [Severity: medium]

**Description**: The `run_single_case` helper (`tests/bestest/mod.rs:27-58`) has no mechanism to distinguish between different categories of test failure. It uniformly panics via an assertion on the full set of band violations, combining all failing metrics into a single panic message. This means the `#[should_panic]` expected string on line 142 only works because the Display impl of `BestestMetric::MinZoneTempC` renders as `"min_zone_temp_c"` (`reference_bands.rs:16`) and the failure format string (`mod.rs:47`) includes `metric={}`. These two independent implementation details are coupled by coincidence, not by design — changing either would silently break the xfail pattern.

**Code Location**:
- `tests/bestest/mod.rs:46-49` — failure message format: `"case={} metric={} value={:.6} outside [{:.6}, {:.6}]"`
- `tests/bestest/reference_bands.rs:12-22` — Display impl for `BestestMetric`, where `MinZoneTempC → "min_zone_temp_c"`

**Root Cause**: No structured error type separates known from unknown failures. The band-check assertion at `mod.rs:53-57` treats all violations uniformly rather than surfacing individual fail/pass statuses that a test could inspect programmatically.

### Finding 3: [Severity: low]

**Description**: The `#[ignore]` reason strings reference `"pending remaining physics fixes: B1, S4/S5, and other consolidated.md items"` (lines 77, 99, 120, 143, 167) but do not link to specific known discrepancies for each case. A developer seeing these tests has no way to understand, without reading `consolidated.md` in full, why each BESTEST case is disabled, what the expected vs. actual values are, or whether the case is close to passing.

**Code Location**: `tests/bestest/mod.rs:77,99,120,143,167`

### Finding 4: [Severity: low]

**Description**: BESTEST cases 910, 920, 930FF, 940, 950, and 960 from the ASHRAE 140-2017 standard have not been implemented. No fixtures exist for these cases, and no test functions reference them. The `bestest_extended_cases_are_tracked` test (`mod.rs:174-187`) only validates that extended-case fixture files exist on disk — it does not run simulations against those cases.

## BESTEST Case Status Matrix

| Case | Fixture | Test | Status | Known Discrepancy |
|------|---------|------|--------|-------------------|
| 600  | Yes     | `#[ignore]` | Pending physics fixes | Depends on B1 (internal gain split), S4/S5 (warmup). Awaiting Tier 1 fixes from `consolidated.md`. |
| 600FF | Yes   | `#[ignore]` | Pending physics fixes | Depends on B1, S4/S5. Free-float case — sensitive to initialization. |
| 640  | Yes     | `#[ignore]` | Pending physics fixes | Setback thermostat. Depends on B1, S4/S5. |
| 900  | Yes     | `#[ignore]` | Pending physics fixes | High-mass conditioned. Depends on B1, S4/S5, B6 (window exterior LWR). Floor slab τ≈33 days. |
| 900FF | Yes    | `#[ignore]` + `#[should_panic]` | Known failure | **min_zone_temp_c = +0.90°C (ASHRAE band: [-6.4, −1.6]°C, outlier = +2.50°C).** Root causes: S4/S5 (warmup, −2.4°C measured), B1 (internal gain 100% convective, −0.22°C estimated at correct 30% radiant), B6 (window exterior LWR skipped, est. −1.0 to −2.5°C), B2 (window interior LWR dropped, +0.5 to +1.0°C partial offset). Peak zone temp likely passes. Sources: `docs/findings/consolidated.md` §"Revised 900FF error budget". |
| 610  | Yes     | None     | Uninvestigated | No simulation test defined. Only fixture existence is verified. |
| 620  | Yes     | None     | Uninvestigated | No simulation test defined. Only fixture existence is verified. |
| CE100 | Yes    | None     | Uninvestigated | No simulation test defined. Only fixture existence is verified. |
| CE200 | Yes    | None     | Uninvestigated | No simulation test defined. Only fixture existence is verified. |
| S5.4-HP | Yes  | None     | Uninvestigated | No simulation test defined. Only fixture existence is verified. |
| 910  | No      | None     | Not implemented | ASHRAE 140 sunspace south-facing, no fixture or test. |
| 920  | No      | None     | Not implemented | ASHRAE 140 sunspace east/west, no fixture or test. |
| 930FF | No     | None     | Not implemented | ASHRAE 140 sunspace free-float, no fixture or test. |
| 940  | No      | None     | Not implemented | ASHRAE 140 basement case, no fixture or test. |
| 950  | No      | None     | Not implemented | ASHRAE 140 slab-on-grade, no fixture or test. |
| 960  | No      | None     | Not implemented | ASHRAE 140 crawlspace, no fixture or test. |

## Summary

- **Total findings**: 4
- **Critical**: 1 (F1: `#[should_panic]` xfail soundness hazard)
- **High**: 0
- **Medium**: 1 (F2: implicit coupling between Display impl and xfail pattern)
- **Low**: 2 (F3: vague ignore reasons; F4: unimplemented BESTEST cases)

## Recommendations

1. **Replace `#[should_panic]` with explicit assertions** on `bestest_case_900ff` (F1). The test should:
   - Run the simulation to completion (catching any panic from `run_case`).
   - Evaluate all bands.
   - Assert that at least one failure exists (i.e., the known issue is still present).
   - Assert that `BestestMetric::MinZoneTempC` is among the failures.
   - Verify the min_zone_temp_c value is within a plausible range around the documented outlier (e.g., +0.5°C to +1.5°C, tolerating small shifts from unrelated changes).
   - Assert that no other metric is failing (to detect new regressions).
   - Preserve the `#[ignore]` attribute.

2. **Introduce a structured `BandResult` type** (F2) that a test can inspect programmatically rather than relying on a formatted panic string. This should include per-metric pass/fail status and the computed value, enabling tests to assert on specific known failures without `#[should_panic]`.

3. **Document per-case known-discrepancy details in the `#[ignore]` reason strings** (F3). Each ignore attribute should briefly state the expected vs. actual value and the primary root cause identifier (e.g., `"900FF min_zone_temp_c = +0.90°C exceeds ASHRAE band [-6.4,-1.6]°C; root causes S4+S5 warmup"`). This removes the indirection through `consolidated.md` and makes test status immediately readable.

4. **Track missing BESTEST cases as issues/tickets** (F4). Cases 910–960 and the extended cases (610, 620, CE100, CE200, S5.4-HP) that lack simulation tests should have corresponding tracking tickets with priority assigned based on relevance to HARES's residential energy simulation scope.

## References / Citations

- Rust Reference §11.4.3 `#[should_panic]` — "If the test function panics, the test passes. If the expected parameter is used, the panic message must contain the provided string."
- ASHRAE 140-2017 Tables B8-2, B8-3a — reference bands for BESTEST cases 600, 600FF, 900, 900FF, 640
- `docs/findings/consolidated.md` — Issue register and 900FF error budget with measured/estimated contributions from S4/S5, B1, B2, B6
- EnergyPlus Engineering Reference §1.2 — warmup convergence requirement for buildings with significant thermal mass
