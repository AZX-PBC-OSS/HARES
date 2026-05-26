# URDB JSON parser: test against fixtures, energyratestructure, demandratestructure, fixedchargefirstmeter
**Review ID**: tariffdp-04
**Category**: tariff-deep
**Date**: 2026-05-26

## Files Reviewed
crates/hares-tariff/src/urdb.rs

## Vendor/Reference Files Consulted
None

## Findings

### Finding 1: [Severity: high] demand_window_minutes hardcoded to 15; not extracted from URDB metadata
**Description**: The parser hardcodes `demand_window_minutes: 15` (line 610) and never attempts to extract this value from any URDB field. URDB tariff entries may specify non-standard demand averaging windows (30 or 60 minutes) through fields such as a `demandwindow` metadata entry or within rate-structure tier metadata. The 15-minute default is correct for standard US FERC/NERC residential practices, but a tariff silently using the wrong window length will produce incorrect rolling-average peak demand — and therefore incorrect demand charges.
**Code Location**: `urdb.rs:610` — `demand_window_minutes: 15,` (hardcoded constant in the `ElectricTariff` constructor)
**Root Cause**: No parsing logic exists to read a demand-window field from the JSON. The value is a literal constant.
**Impact**: Demand charges will be computed for the wrong window length when a tariff uses a non-15-minute window. The rolling-average in `DemandWindow` (billing.rs:17-63) computes a running average over `demand_window_minutes * 60 / interval_seconds` samples. A 60-minute window reduced to a 15-minute average would overstate demand volatility (higher peak), while expanding 15 minutes to 60 would understate it. The error magnitude depends on load profile granularity but can shift demand bills by tens of percent.

### Finding 2: [Severity: medium] Only primary `rate`+`adj` extracted from demand tiers; separate generation/transmission/distribution components silently dropped
**Description**: The `tier_rate` helper (line 319-322) sums only the `rate` and `adj` fields from a tier object. Some URDB tariffs encode demand as multiple additive components (generation demand, transmission demand, distribution demand) using separate fields within the same tier JSON (e.g., `genrate`, `transrate`, `distrate`). The parser treats every demand tier as a single combined rate and never looks for these additional component fields. If a tariff has separate components that sum to the total demand charge, only the primary `rate`+`adj` is captured.
**Code Location**: `urdb.rs:319-322` — `tier_rate` reads `rate` and `adj` only; `urdb.rs:496-522` and `urdb.rs:550-567` — demand rate construction uses `tier_rate`.
**Root Cause**: The parser assumes a single rate field per demand tier. It does not scan for or sum separate generation/transmission/distribution demand rate fields.
**Impact**: For tariffs with multi-component demand charges, the demand portion of the bill may be understated because only the primary rate component is captured. PG&E tariffs, for example, often split demand into generation and distribution components.

### Finding 3: [Severity: medium] No compound or seasonal fixed charge handling
**Description**: `parse_fixed_charges` (lines 353-372) reads a single `fixedchargefirstmeter` field and assigns it to **either** `daily_usd` **or** `monthly_usd` based on `fixedchargeunits`, not both. A tariff with both a daily access charge AND a monthly service fee (potentially stored in `fixedchargesecondmeter` or other fields) would only capture one component. Additionally, fixed charges that vary by season are not supported — the value is a single scalar.
**Code Location**: `urdb.rs:353-372` — `parse_fixed_charges` function.
**Root Cause**: The parser only reads `fixedchargefirstmeter` and treats daily/monthly as mutually exclusive. No parsing of `fixedchargesecondmeter` or seasonal fixed-charge structures.
**Impact**: Tariffs with multi-component or seasonal fixed charges (e.g., a monthly facilities charge plus a daily metering charge, or winter/summer differentials on the monthly service fee) will have understated fixed costs. The daily+monthly combination is common in some commercial and a few residential tariffs.

### Finding 4: [Severity: medium] flatdemandstructure season assignment relies on array position heuristics
**Description**: For `flatdemandstructure` (lines 496-522), the parser infers season from array position: 1-entry means `SeasonFilter::All`, 2-entry means `[0]=Summer, [1]=Winter`. This heuristic is not guaranteed by the URDB format. Some utilities order seasons differently, and the mapping could be inverted. For >2 entries, a warning is logged and all extra entries get `SeasonFilter::All`, which is a fallback rather than a resolution.
**Code Location**: `urdb.rs:500-513` — season matching logic for `flatdemandstructure`.
**Root Cause**: Season is inferred from the array index of `flatdemandstructure` rather than from explicit schedule metadata or a seasonal mapping field.
**Impact**: If a utility uses `[0]=Winter, [1]=Summer` ordering, summer and winter demand rates would be swapped, causing incorrect demand charges for both seasons. The >2-entry fallback to `All` may miss seasonal granularity entirely.

### Finding 5: [Severity: low] sell rates only extracted from the first tier of each energy period
**Description**: When building TOU export credits (lines 446-452), the parser only examines `tiers.first().and_then(tier_sell)` — the sell rate from the first tier only. If a URDB tariff has `sell` fields on higher tiers within the same period, those are ignored. In principle, a utility could specify different sell rates for different usage levels within a TOU period.
**Code Location**: `urdb.rs:446-452` — sell-credit extraction loop.
**Root Cause**: The code assumes sell rates are uniform across all tiers within a period, only checking the first tier.
**Impact**: If a tariff has tiered sell rates, only the first tier's rate is used for all exported energy in that period. This is unusual in residential tariffs (sell rates are typically flat) but could affect commercial or feed-in tariff scenarios.

### Finding 6: [Severity: low] demandratestructure without demand schedule matrices loses seasonal information
**Description**: When `demandratestructure` is present but the `demandweekdayschedule` and/or `demandweekendschedule` matrices are absent, the parser assigns `SeasonFilter::All` to every demand rate (line 558). The energy schedules may contain seasonal information that could be used as a fallback, but it is not consulted.
**Code Location**: `urdb.rs:554-558` — fallback season logic in TOU demand rate construction.
**Root Cause**: The `else` branch defaults to `All` without attempting to derive season from the energy weekdayschedule/weekendschedule matrices for the corresponding period index.
**Impact**: Demand charges meant to be summer-only or winter-only would be applied year-round, inflating demand costs in the wrong season. Mitigated by the fact that when demand schedules ARE present, seasons are correctly derived (lines 554-556).

### Finding 7: [Severity: low] seasonal detection defaults to northern hemisphere (June-September summer)
**Description**: When `detect_summer_months` (lines 117-150) cannot identify a seasonal split — because all 12 months have identical schedule patterns — the default summer months are hardcoded as months 6-9 (June-September, northern hemisphere summer). The algorithm does handle southern hemisphere detection within its working logic (inverting when >6 months differ from December), but the hardcoded fallback always assumes northern hemisphere.
**Code Location**: `urdb.rs:400-403` — `let default_summer: BTreeSet<u8> = (6..=9).collect();`
**Root Cause**: A northern hemisphere constant is the fallback when seasonal detection produces no result.
**Impact**: Only affects tariffs where all months use the same schedule pattern (no seasonal TOU variation), which means seasonal split is irrelevant anyway. Impact is negligible in practice since the seasonal split would not be applied for all-season tariffs.

### Finding 8: [Severity: low] Insufficient fixture coverage; no cross-fixture annual bill comparison test
**Description**: Only 2 URDB fixture files exist (`flat_rate.json` and `pge_e_tou_c.json`). There is no test that parses all fixtures and compares annual bills computed by the evaluator against independently calculated reference bills (e.g., from the URDB API or a manual spreadsheet calculation). The review instructions explicitly request such a test.
**Code Location**: `urdb.rs:624-626` (fixture includes); `tests/fixtures/urdb/` directory (only 2 fixtures).
**Root Cause**: Test coverage for the URDB parser is limited to unit-level field extraction checks and basic round-trip parsing. No integration-level bill comparison tests exist.
**Impact**: URDB tariff structures not represented in the fixtures (e.g., 12-period TOU, seasonal demand only, complex tiered blocks, multiple demand components, compound fixed charges, net billing with TOU sell credits) may contain parsing bugs that would go undetected. The 2-fixture set exercises only flat-rate and 3-period TOU tariffs.

## Summary
- Total findings: 8
- Critical: 0
- High: 1
- Medium: 3
- Low: 4

## Recommendations

1. **Extract `demand_window_minutes` from URDB metadata.** Scan the JSON for a demand-window field (the URDB v7 format may store this as a `demandwindow` property on the tariff or within rate-structure metadata). If absent, default to 15 minutes with a `tracing::warn!`.

2. **Parse multi-component demand rates.** Extend `tier_rate` (or add a separate function) to sum all known demand-rate component fields (`genrate`, `transrate`, `distrate`, etc.) in addition to `rate`+`adj`.

3. **Support compound fixed charges.** Parse `fixedchargesecondmeter` if present, and sum daily + monthly components rather than treating them as exclusive. Consider supporting seasonally-varying fixed charges if the URDB format includes them.

4. **Improve `flatdemandstructure` season inference.** Instead of hardcoding `[0]=Summer, [1]=Winter`, use the detected summer months from the energy schedule to cross-reference which demand entry applies to which season. Alternatively, require the URDB fixture to include explicit season labels.

5. **Expand URDB fixture coverage.** Add fixtures for tariffs with: (a) no demand charges, (b) demand-only seasonal charges, (c) 4+ TOU periods, (d) tiered blocks across all seasons, (e) net billing with TOU sell credits, (f) daily fixed charges, (g) multiple `flatdemandstructure` seasons. Add a test that parses every fixture and runs a 1-year bill simulation against a reference load profile, comparing the output to independently calculated bills.

6. **Extract sell rates from all tiers within a period**, not just the first.

7. **Fall back to energy schedule seasonality** when demand schedules are absent but `demandratestructure` is present.

8. **Consider making the default summer months configurable or location-aware** rather than hardcoding June-September.

## References / Citations
- `urdb.rs:319-322` — `tier_rate` helper (rate + adj extraction)
- `urdb.rs:353-372` — `parse_fixed_charges` (single-field fixed charge)
- `urdb.rs:400-403` — northern hemisphere default summer months
- `urdb.rs:434-436` — period index → energy_structure mapping
- `urdb.rs:446-452` — sell rate extraction (first tier only)
- `urdb.rs:496-522` — `flatdemandstructure` season heuristic
- `urdb.rs:550-567` — TOU demand rate construction with season fallback
- `urdb.rs:610` — hardcoded `demand_window_minutes: 15`
- `urdb.rs:624-626` — fixture includes (only 2 fixtures)
- `billing.rs:17-63` — `DemandWindow` rolling-average demand implementation
- `types.rs:232` — `demand_window_minutes` field on `ElectricTariff`
