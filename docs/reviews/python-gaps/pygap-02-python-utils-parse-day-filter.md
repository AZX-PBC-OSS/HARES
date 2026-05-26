# Python utils parse_day_filter: all 9 filter strings, case insensitivity, error messages
**Review ID**: pygap-02
**Category**: python-gaps
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-python/src/utils.rs` (parse_day_filter, lines 10–26)
- `crates/hares-types/src/schedule.rs` (DayFilter enum definition, lines 12–22)
- `crates/hares-python/src/py_enums.rs` (callers at lines 2268–2269, 2292, 2363)
- `crates/hares-python/src/py_tariff.rs` (parse_season for comparison, lines 11–20)
- `docs/schedule-sources.md` (DayFilter documentation, lines 113–121)

## Vendor/Reference Files Consulted
None

## Findings

### Finding 1: [Severity: medium]
**Description**: The parser uses the string `"any"` to represent the universal filter, but both the review specification and a sibling parser in the same codebase use `"all"` for the equivalent concept. In `py_tariff.rs:12–13`, `parse_season` maps `"all"` → `SeasonFilter::All`. However, `parse_day_filter` maps `"any"` → `DayFilter::Any`. A user who naturally writes `"all"` for a day filter receives an error message that mentions `'any'` but never explains that `"all"` is not valid. Additionally, serde deserialization of `DayFilter` uses the capitalized variant name `"Any"`, creating a third naming convention for the same concept across parse paths.

**Code Location**: `crates/hares-python/src/utils.rs:11–12` — `"any" => Ok(DayFilter::Any)`
**Root Cause**: The `DayFilter` enum uses `Any` as its variant name, and the parser mirrors this with `"any"` rather than using the more conventional `"all"`. No alias or documentation bridges the gap.
**Impact**: Users providing `"all"` from configuration files get a cryptic error: `"unknown day filter 'all', expected 'any', 'weekdays', 'weekends', or a day name"`. The confusion is compounded by `parse_season` in the same codebase accepting `"all"`.

### Finding 2: [Severity: low]
**Description**: The parser does not strip leading or trailing whitespace before matching. A filter string like `" weekdays"` (space-prefixed) or `"weekdays "` (trailing space) passes through `to_lowercase()` with whitespace intact and fails to match any variant, producing an error. This is common when configuration files are hand-edited or when values are extracted from YAML/TOML with inconsistent whitespace handling.

**Code Location**: `crates/hares-python/src/utils.rs:11` — `match s.to_lowercase().as_str()`
**Root Cause**: No `.trim()` call is applied before the match.
**Impact**: Configuration typos that include accidental whitespace result in confusing errors that do not indicate the whitespace as the problem. Users must inspect the raw string to spot the issue.

### Finding 3: [Severity: medium]
**Description**: The error message for unrecognized filters does not enumerate all valid string values. It states `"expected 'any', 'weekdays', 'weekends', or a day name"` but `"a day name"` is vague — it does not list `"monday"` through `"sunday"`. Users must guess or read source code to discover the exact day name strings. Compare with `parse_resample_method` at `utils.rs:84–87`, which lists all six valid method names explicitly.

**Code Location**: `crates/hares-python/src/utils.rs:22–24`
**Root Cause**: The error message uses a category description instead of listing concrete values.
**Impact**: Increased friction for users debugging configuration errors. A minor code change (listing the seven day names) would eliminate this entirely.

### Finding 4: [Severity: low]
**Description**: The parser has no support for combined or compound day filters (e.g., `"weekdays+peak_events"`). The `DayFilter` enum in `hares-types/src/schedule.rs:13–22` contains only `Any`, `Weekdays`, `Weekends`, and `Day(Weekday)`, with no `Combined(Vec<DayFilter>)` or intersection variant. This is a design limitation of the type system, not a parser oversight, but the parser neither documents the limitation nor provides a hint in the error message that combinations are unsupported.

**Code Location**: `crates/hares-types/src/schedule.rs:13–22` (DayFilter enum), `crates/hares-python/src/utils.rs:10–26` (parser)
**Root Cause**: The `DayFilter` enum is intentionally simple (no combination support). The parser faithfully reflects this but provides no guidance when a user attempts a combined string.
**Impact**: Users familiar with scheduling systems that support compound filters will discover the limitation only through trial and error.

### Finding 5: [Severity: medium]
**Description**: `parse_day_filter` has zero test coverage. The test module in `utils.rs:91–142` contains four tests, all covering `parse_resample_method` exclusively. There are no tests for valid filter parsing, case-insensitive matching (`"WEEKDAYS"`, `"Weekdays"`), error messages for invalid strings, empty strings, whitespace-only strings, or Unicode input.

**Code Location**: `crates/hares-python/src/utils.rs:91–142` (test module, no `parse_day_filter` tests)
**Root Cause**: Tests were written for `parse_resample_method` but `parse_day_filter` was left untested.
**Impact**: Refactoring the parser or error messages could silently break case insensitivity or valid filter recognition without any test failure to catch it. This is a regression risk for all Python-visible APIs that depend on `parse_day_filter` (`BmsScheduleWindow`, `DepartureConstraint`, tariff windows).

### Finding 6: [Severity: low] (positive)
**Description**: Case insensitivity is correctly implemented via `s.to_lowercase()` at `utils.rs:11`. Rust's `str::to_lowercase()` handles both ASCII and Unicode correctly, so `"WEEKDAYS"`, `"Weekdays"`, and `"weekdays"` all parse to `DayFilter::Weekdays`.

**Code Location**: `crates/hares-python/src/utils.rs:11`
**Impact**: None — this is working correctly.

### Finding 7: [Severity: low] (positive)
**Description**: The return type `DayFilter` is the exact enum used by the scheduling engine. `TimeWindow.day` at `schedule.rs:57` is a `DayFilter`, and `DayFilter::matches()` at `schedule.rs:26–33` is the engine's matching function. No conversion or lookup layer is needed between the parser output and runtime use.

**Code Location**: `crates/hares-python/src/utils.rs:10` — return type `PyResult<DayFilter>`; `crates/hares-types/src/schedule.rs:57` — `TimeWindow.day: DayFilter`
**Impact**: None — the abstraction is sound.

### Finding 8: [Severity: low] (positive)
**Description**: Zero-length strings (`""`) fall through to the `_` catch-all arm and produce a `PyValueError` with the message `"unknown day filter '', expected 'any', ..."`. This is a proper error, not a panic or a silent default. However, the error message could be friendlier for the empty-string case.

**Code Location**: `crates/hares-python/src/utils.rs:22–24` (catch-all arm)
**Impact**: No crash. User sees an error message, albeit one that includes a confusing pair of empty quotes.

### Finding 9: [Severity: low] (positive)
**Description**: Unicode and non-ASCII characters (e.g., `"mönday"` from a mis-encoded config file) pass through `to_lowercase()` (which is Unicode-aware) and do not match any variant, falling to the error branch. No panic or undefined behavior occurs.

**Code Location**: `crates/hares-python/src/utils.rs:11,22`
**Impact**: Non-ASCII inputs are rejected gracefully with an error message.

### Finding 10: [Severity: medium]
**Description**: The `DayFilter` enum lacks variants for `holidays`, `peak_events`, `non_peak_events`, `first_of_month`, and `last_of_month`. These 5 of the 9 filter categories listed in the reference specification are absent from both the type system and the parser. This was previously identified in the schedule-helpers review (`equip-util-02`), which noted the absence of holiday support specifically. The parser faithfully reflects the type system's capabilities — it is not missing strings that the type system would accept — but the absence means users have no way to express these scheduling patterns.

**Code Location**: `crates/hares-types/src/schedule.rs:13–22` (enum definition), `crates/hares-python/src/utils.rs:10–26` (parser)
**Root Cause**: Design scope limitation. The `DayFilter` was designed for basic day-of-week scheduling, not for calendar-aware or event-driven scheduling.
**Impact**: Schedules cannot differentiate holidays from regular weekdays, or peak events from normal days. Simulations that overlap with holidays or DR events apply the same schedule as the base day type. For residential energy models this is typically minor, but it limits the expressiveness of the scheduling system.

## Summary
- Total findings: 10
- Critical: 0
- High: 0
- Medium: 4 (Findings 1, 3, 5, 10)
- Low: 6 (Findings 2, 4, 6–9)

## Recommendations
1. **Add `"all"` as an alias** for `"any"` in `parse_day_filter` to match user expectations and maintain consistency with `parse_season`. Alternatively, rename the `DayFilter::Any` variant or add a documentation alias.
2. **Trim whitespace** from the input string with `.trim()` before lowering and matching.
3. **Enumerate all valid day names** in the error message: change `"or a day name"` to `"or 'monday'..'sunday'"`.
4. **Add comprehensive tests** for `parse_day_filter` covering: all 10 valid strings, case insensitivity (mixed/caps), invalid strings, empty strings, whitespace-only strings, Unicode input, and error message content validation.
5. **Document the combination limitation** in the error message or documentation. If the type system is expanded to support compound filters, update the parser accordingly.
6. **Consider extending `DayFilter`** with `Holidays` and event-driven variants if holiday-aware or DR-event-driven scheduling becomes a requirement. This has been flagged in a prior review (`equip-util-02`).

## References / Citations
- `crates/hares-python/src/utils.rs:10–26` — `parse_day_filter` implementation
- `crates/hares-types/src/schedule.rs:12–22` — `DayFilter` enum definition
- `crates/hares-types/src/schedule.rs:24–33` — `DayFilter::matches()` method
- `crates/hares-python/src/py_tariff.rs:11–20` — `parse_season` using `"all"` (inconsistency)
- `docs/schedule-sources.md:113–121` — official DayFilter documentation
- `docs/reviews/equipment-util/equip-util-02-schedule-helpers.md` — prior review noting holiday support gap
