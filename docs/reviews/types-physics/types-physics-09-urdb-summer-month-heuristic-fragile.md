# URDB summer-month heuristic fragile
**Review ID**: types-physics-09
**Category**: types-physics
**Date**: 2026-05-26

## Files Reviewed
crates/hares-tariff/src/urdb.rs
crates/hares-types/src/schedule.rs

## Vendor/Reference Files Consulted
vendors/OCHRE/ochre/utils/hpxml.py — No direct comparison available. OCHRE's HPXML parser deals with building envelope geometry, not tariff rate schedules. It contains no seasonal tariff logic relevant to this review.

## Findings

### Finding 1: [Severity: critical] `SeasonFilter::contains_month()` hardcodes June–September summer; `SeasonalSplit` is never consulted at runtime

**Description**: The `SeasonFilter::contains_month()` method defines summer as hardcoded months 6–9 (June–September) and winter as everything else. The evaluator and billing code use this method exclusively to determine which season's rates apply. Meanwhile, `SeasonalSplit`—which supports configurable summer month ranges including wrapping for Southern Hemisphere—is parsed, stored on `ElectricTariff`, but never passed to any runtime evaluation path. This means the URDB parser's hemisphere detection and custom seasonal split are dead code: even when the parser correctly identifies Southern Hemisphere summer months, the runtime still applies the Northern Hemisphere June–September definition.

**Code Location**: 
- `crates/hares-types/src/schedule.rs:878-888` — `SeasonFilter::contains_month()` hardcodes `(6..=9).contains(&month)` for summer
- `crates/hares-types/src/schedule.rs:933-943` — `SeasonalSplit::is_summer()` exists but is unused
- `crates/hares-tariff/src/evaluator.rs:98` — `period.season.contains_month(month)` (no `SeasonalSplit` parameter)
- `crates/hares-tariff/src/evaluator.rs:117,134,150,273` — same pattern repeated for energy rates, export credits, demand TOU, and tiered blocks
- `crates/hares-tariff/src/evaluator.rs:369` — `dr.season.contains_month(month)` for demand charges
- `crates/hares-tariff/src/billing.rs:288` — `b.season.contains_month(month)` for tiered energy cost
- `crates/hares-tariff/src/urdb.rs:591-595` — seasonal_split is computed and stored but the evaluator never reads it

**Root Cause**: The `SeasonFilter` enum was designed with a fixed geographic assumption baked into its `contains_month()` method. The `SeasonalSplit` struct was added later as a mechanism to override that assumption, but the runtime evaluation code was never updated to accept and consult a `SeasonalSplit`. The two mechanisms (`SeasonFilter` for labeling periods, `SeasonalSplit` for defining the actual summer range) are decoupled.

**Impact**: 
- **Southern Hemisphere tariffs produce incorrect results**: Even though `detect_summer_months()` in `urdb.rs:117-150` attempts to detect Southern Hemisphere summer months (Dec–Feb), the rates will be applied as if summer is June–September.
- **Non-standard North American tariffs produce incorrect results**: Utilities with summer rates covering May–October or April–October will be mis-evaluated.
- **The hemisphere detection and custom split infrastructure is dead code**: The parser correctly computes a `SeasonalSplit` at `urdb.rs:591-595`, but no consumer reads it.

---

### Finding 2: [Severity: high] `SeasonFilter` enum lacks `Shoulder`/`Intermediate` variant

**Description**: The `SeasonFilter` enum (`schedule.rs:862-867`) only has three variants: `All`, `Summer`, and `Winter`. There is no representation for shoulder or intermediate seasons. The `season_for_months()` function in `urdb.rs:202-210` uses a binary classification: if any month in a period's active months is in the detected summer set, that period gets `SeasonFilter::Summer`; if none are, it gets `Winter`; if both summer and winter months are present, it gets `All`. This means a utility that defines three distinct rates for summer (Jun–Sep), winter (Nov–Mar), and shoulder (Apr–May, Oct) cannot be correctly modeled—shoulder periods will be silently misclassified as either summer or winter.

**Code Location**: 
- `crates/hares-types/src/schedule.rs:862-867` — enum has no `Shoulder` variant
- `crates/hares-tariff/src/urdb.rs:202-210` — binary summer/winter classification

**Root Cause**: The season model is fundamentally binary (summer/winter) and does not accommodate the three-season rate structures common in real-world utility tariffs. The URDB format itself supports multi-period seasonal schedules through the 12×24 matrix encoding, but the HARES type system cannot represent more than two seasons plus "all."

**Impact**: Shoulder-season rates are silently merged into either summer or winter, producing inaccurate cost calculations for tariffs with three distinct seasonal rate tiers. Many U.S. utilities (e.g., SCE, SDG&E, APS) use three-season structures.

---

### Finding 3: [Severity: medium] `detect_summer_months` uses December as fixed reference month

**Description**: The `detect_summer_months()` function at `urdb.rs:117-150` always uses December (month 12, index 11) as the reference "winter" month against which all other months are compared. This is a Northern Hemisphere assumption. The hemisphere inversion logic (>6 months differ → invert) is a heuristic that assumes symmetrical six-month seasons, which is not always the case. Additionally, the equality comparison (`wd != dec_wd || we != dec_we`) is sensitive to undetected small schedule variations.

**Code Location**: `crates/hares-tariff/src/urdb.rs:117-150`

**Root Cause**: The detection algorithm makes a geometric assumption about the hemisphere based on schedule pattern differences from December, rather than consulting geographic metadata or explicit season fields that may exist in the URDB JSON.

**Impact**: For tariffs where the schedule pattern difference from December is exactly 6 months (borderline case), the hemisphere classification is indeterminate and depends on the `> 6` threshold (not `>= 6`). A Southern Hemisphere tariff with exactly 6 months differing from December would not be inverted and would be misclassified as Northern Hemisphere.

---

### Finding 4: [Severity: medium] Fallback to June–September for non-contiguous months

**Description**: When `seasonal_split_from_months()` in `urdb.rs:152-200` encounters a set of summer months that is non-contiguous even with year-end wrapping (e.g., months {5, 7, 9, 11}), it logs a warning and falls back to hardcoded June–September. This silently substitutes an incorrect default for a legitimate (though unusual) tariff structure. The function also returns `None` (which causes `HasSeasonal` check to be false, resulting in no seasonal split) rather than using the fallback when `summer_months` is empty.

**Code Location**: `crates/hares-tariff/src/urdb.rs:194-199`

**Root Cause**: The `SeasonalSplit` type models seasons as a single contiguous (possibly wrapping) range of months. It cannot represent disjoint sets of summer months. This is a modeling limitation that translates into data loss during parsing.

**Impact**: Tariffs with non-contiguous summer month assignments produce a warning and silently revert to an incorrect June–September default.

---

### Finding 5: [Severity: low] `flatdemandstructure` season assignment uses array index convention

**Description**: The flat demand structure parser at `urdb.rs:496-513` assigns `SeasonFilter::Summer` to index 0 and `SeasonFilter::Winter` to index 1 of the `flatdemandstructure` array, based solely on array position. This convention is not validated against the schedule data or the detected summer months. It also uses the hardcoded `SeasonFilter` rather than consulting the detected `summer_months` set.

**Code Location**: `crates/hares-tariff/src/urdb.rs:496-513`

**Root Cause**: The flat demand rates have no associated schedule matrix, so there is no data-driven way to determine which array index corresponds to which season. The index-based convention (0=summer, 1=winter) is a reasonable guess but is unverified.

**Impact**: If a URDB flat demand structure uses a different season-to-index mapping (or has only one entry that should be season-specific), the demand rates will be applied to the wrong season.

---

### Finding 6: [Severity: low] URDB parser does not consult explicit season-definition fields

**Description**: The parser never looks for or reads any explicit season-related fields from the URDB JSON, such as `season`, `season_mapping`, or season name/description fields. All seasonal inference is derived heuristically from the schedule matrices.

**Code Location**: `crates/hares-tariff/src/urdb.rs:386-618` (the `parse` function does not reference any season-definition field)

**Root Cause**: The URDB data format and the specific fields it provides for season definitions are unknown to the parser. All seasonal logic is reverse-engineered from schedule patterns.

**Impact**: If the URDB JSON includes authoritative season metadata (e.g., explicit month ranges for each season with labels like "SUMMER", "WINTER", "SPRING", "FALL"), this information is discarded.

## Summary
- Total findings: 6
- Critical: 1 (Finding 1)
- High: 1 (Finding 2)
- Medium: 2 (Findings 3, 4)
- Low: 2 (Findings 5, 6)

## Recommendations

1. **Fix the `SeasonFilter::contains_month()` / evaluator disconnect (Finding 1)**: The evaluator and billing code should accept an optional `SeasonalSplit` parameter and use `SeasonalSplit::is_summer()` instead of the hardcoded `SeasonFilter::contains_month()`. If no `SeasonalSplit` is provided, fall back to June–September as the default. This alone would make the hemisphere detection and custom seasonal split infrastructure functional.

2. **Add `Shoulder` support to `SeasonFilter` (Finding 2)**: Add a `SeasonFilter::Shoulder` variant. The `season_for_months()` function in the URDB parser would need a more nuanced classification strategy (e.g., three-way split, or configurable season boundaries). The `SeasonalSplit` type would need to be extended (or complemented) to represent three season boundaries.

3. **Improve hemisphere detection robustness (Finding 3)**: Instead of using December as the fixed reference month, consider using the schedule data to find the most "extreme" or "distinct" month patterns and derive season boundaries from those. Alternatively, consult geographic metadata (latitude, country) if available in the URDB JSON to set hemisphere expectations.

4. **Replace fallback defaults with errors or config overrides (Finding 4)**: When summer months are non-contiguous, the parser should either (a) reject the tariff with a clear error indicating limited season model support, or (b) support multiple disjoint seasonal split ranges. Silently falling back to June–September is hazardous.

5. **Cross-validate flat demand season assignments (Finding 5)**: Where possible, cross-check the flat demand structure index convention against the energy schedule's detected season months. If they conflict, log a warning.

6. **Parse explicit URDB season metadata if available (Finding 6)**: Investigate the URDB v7 schema for season-definition fields (e.g., `season`, `season_mapping`, or named period-to-season labels). If such fields exist, prefer them over the heuristic schedule comparison.

## References / Citations
- `crates/hares-types/src/schedule.rs:862-867` — `SeasonFilter` enum (All, Summer, Winter only)
- `crates/hares-types/src/schedule.rs:878-888` — hardcoded June–September in `contains_month()`
- `crates/hares-types/src/schedule.rs:895-943` — `SeasonalSplit` struct and `is_summer()` (unused at runtime)
- `crates/hares-tariff/src/urdb.rs:117-150` — `detect_summer_months()` with December reference
- `crates/hares-tariff/src/urdb.rs:152-200` — `seasonal_split_from_months()` with June–September fallback
- `crates/hares-tariff/src/urdb.rs:202-210` — `season_for_months()` binary classification
- `crates/hares-tariff/src/urdb.rs:401-403` — default summer months hardcoded as June–September
- `crates/hares-tariff/src/urdb.rs:500-513` — `flatdemandstructure` season-by-index convention
- `crates/hares-tariff/src/urdb.rs:591-595` — `seasonal_split` computed but never consumed by evaluator
- `crates/hares-tariff/src/evaluator.rs:98,117,134,150,273,369` — all evaluator paths use hardcoded `contains_month()`
- `crates/hares-tariff/src/billing.rs:288` — billing uses hardcoded `contains_month()`
