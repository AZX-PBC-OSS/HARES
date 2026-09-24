# Python utils parse_datetime_str: RFC3339, ISO 8601 T-separated, date-only, naive assumes UTC, DST consistency
**Review ID**: pygap-03
**Category**: python-gaps
**Date**: 2026-05-26

## Files Reviewed
crates/hares-python/src/utils.rs

## Vendor/Reference Files Consulted
None

## Findings

### Finding 1: RFC3339 offset preserved — inconsistent with naive branches that always use UTC [Severity: medium]
**Description**: The function has three parse paths, and they return `DateTime<FixedOffset>` values with different offsets depending on the input format:
- RFC3339 (e.g. `"2024-01-15T14:30:00+05:00"`) → offset `+05:00` is preserved via `DateTime::parse_from_rfc3339`
- ISO 8601 T-separated without timezone (e.g. `"2024-01-15T14:30:00"`) → offset `+00:00` (UTC)
- Date-only (e.g. `"2024-01-15"`) → offset `+00:00` (UTC)

The doc comment only states "assume UTC" for the T-separated path and does not mention how RFC3339 offsets are handled. A caller inspecting `.offset()` sees `+05:00` from an RFC3339 input but `+00:00` from equivalent naive input representing the same wall-clock time.
**Code Location**: `crates/hares-python/src/utils.rs:34-42`
**Root Cause**: `DateTime::parse_from_rfc3339` returns the `DateTime<FixedOffset>` with the original offset intact. The other branches explicitly construct `FixedOffset::east_opt(0)` (UTC). No normalization step unifies the offsets.
**Impact**: Under HARES's local-time convention (documented in `crates/hares-core/src/clock.rs:1-11` and `crates/hares-io/src/config.rs:36-45`), wall-clock digits are what matters — the offset is "carried but ignored unless `civil_timezone` is set." So this inconsistency is **semantically harmless** for schedule evaluation and daily profiles. However, it is a latent trap for any future code that compares `DateTime<FixedOffset>` values by structural equality (offset must match) rather than timestamp equality, or code that serializes offsets assuming uniformity. The doc comment should be updated to explain the three-path behavior.

### Finding 2: Sub-second precision lost for naive T-separated and date-only formats [Severity: medium]
**Description**: The naive fallback uses `NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S")`, which rejects fractional seconds. The date-only branch constructs a string with `T00:00:00` and uses the same format, likewise lacking sub-second support. ISO 8601 explicitly permits fractional seconds in all datetime representations.
- `"2024-01-15T14:30:00.123456Z"` → RFC3339 branch: **succeeds**, sub-seconds preserved
- `"2024-01-15T14:30:00.123456"` (no timezone) → RFC3339 fails, NaiveDateTime fails on `.123456`, date-only fails → **error**
- `"2024-01-15T14:30:00.5+05:00"` → RFC3339 branch: **succeeds**, sub-seconds preserved
- `"2024-01-15T14:30:00.5"` (no timezone) → **error**

Sub-second precision is only available when the input is RFC3339-compliant (i.e., includes a timezone suffix). Naive and date-only inputs silently drop sub-second information and return integer-second precision. There is no warning or error distinction — the sub-second portion simply causes parse failure.
**Code Location**: `crates/hares-python/src/utils.rs:39`
**Root Cause**: The `%Y-%m-%dT%H:%M:%S` format string does not include `%.f`, `%.3f`, `%.6f`, or `%.9f` to capture fractional seconds. chrono uses `%.f` for fractional-second parsing.
**Impact**: Users passing sub-second timestamps without a timezone suffix (e.g., from Python's `datetime.isoformat()` on a naive datetime with microseconds) receive a cryptic error instead of a correctly parsed high-precision timestamp.

### Finding 3: `extract_datetime` breaks on Python naive datetimes with microseconds [Severity: medium]
**Description**: `extract_datetime` (line 61–68) calls `obj.call_method0("isoformat")` on Python datetime objects and feeds the result to `parse_datetime_str`. Python's `datetime.isoformat()` on a naive datetime includes microseconds by default (e.g., `"2024-01-15T14:30:00.123456"`). This string lacks a timezone suffix, so every branch in `parse_datetime_str` fails (RFC3339 needs a timezone, NaiveDateTime parse doesn't handle `.123456`, date-only doesn't match). The result is a `PyValueError` with a misleading message about an "invalid start_time format," even though the datetime itself is perfectly valid.
**Code Location**: `crates/hares-python/src/utils.rs:66-67`
**Root Cause**: Downstream consequence of Finding 2. The `isoformat()` output includes microsecond precision but no timezone (for naive datetimes), and no parse path in `parse_datetime_str` can handle this combination.
**Impact**: Users who construct Python `datetime` objects with nonzero microseconds and pass them via `extract_datetime` (e.g., as `start_time` kwargs) will get a parse error. The workaround is to pass a string with a timezone suffix. Alternatively, users can truncate microseconds before passing the datetime.

### Finding 4: Leap seconds rejected with generic error message [Severity: low]
**Description**: `"2016-12-31T23:59:60Z"` is a valid RFC3339 timestamp (a leap second). chrono 0.4.44 does not support leap seconds, so `DateTime::parse_from_rfc3339` rejects it. The input then falls through all branches and produces the generic error: `invalid start_time format: '2016-12-31T23:59:60Z'. Expected ISO 8601 format (e.g., '2019-01-01T00:00:00Z')`. The error message gives no indication that the input is syntactically valid but semantically unsupported due to a leap second.
**Code Location**: `crates/hares-python/src/utils.rs:34,54-57`
**Root Cause**: chrono's `parse_from_rfc3339` does not implement leap-second handling. The error message at line 54–57 is a generic catch-all.
**Impact**: Low — leap seconds are vanishingly rare in energy simulation inputs. The only harm is a confusing error message when it does occur.

### Finding 5: Date-only parse path is untested [Severity: low]
**Description**: The date-only branch at lines 46–52 (`"%Y-%m-%d"` → construct `T00:00:00` → parse) has no direct unit test. The existing tests cover RFC3339 with Z suffix, RFC3339 with positive/negative offsets, naive fallback, and rejection of invalid strings — but not the date-only format.
**Code Location**: Tests in `crates/hares-python/src/py_dwelling.rs:2062-2116`; missing test.
**Impact**: A regression in the date-only path (e.g., a logic error in the `format!` call, or a change in chrono's parsing behavior) would go undetected by the test suite.

### Finding 6: Duplicated UTC-offset construction [Severity: low]
**Description**: The pattern `FixedOffset::east_opt(0).ok_or_else(|| PyValueError::new_err("invalid UTC offset"))?.from_utc_datetime(&naive)` appears verbatim at lines 39–42 (naive T-separated fallback) and 49–51 (date-only fallback). The `east_opt(0)` call with `ok_or_else` is dead code — `FixedOffset::east_opt(0)` can never return `None` for the value `0`. This pattern could be extracted into a helper function.
**Code Location**: `crates/hares-python/src/utils.rs:40-42` and `49-51`
**Impact**: Minor code duplication. The dead `ok_or_else` branch is harmless defensive code but clutters the logic.

## Items Verified as Correct

**(a) RFC3339 offset math**: `DateTime::parse_from_rfc3339("2024-01-15T14:30:00+05:00")` correctly parses the offset. The resulting `DateTime<FixedOffset>` stores local time `14:30:00` with offset `+05:00`. The underlying Unix timestamp is correctly computed. No sign error exists — chrono 0.4.44's RFC3339 parser is well-tested.

**(b) ISO 8601 T-separated without timezone**: Treated as UTC via `from_utc_datetime` with `FixedOffset::east(0)`. Not treated as local machine time. Correct.

**(c) Date-only**: Parses as midnight UTC via the format string `"{date}T00:00:00"` combined with `from_utc_datetime`. Correct.

**(d) Naive assumes UTC**: Applied consistently to the naive and date-only branches. The RFC3339 branch preserves the original offset rather than assuming UTC, which is intentional under HARES's local-time convention but results in heterogeneous offsets (Finding 1).

**(e) DST transitions**: The parser handles explicit numeric offsets only (RFC3339) or UTC fallback. No IANA timezone resolution occurs at this layer. DST gaps and ambiguous periods do not apply. The `civil_timezone` field in `SimulationConfig` handles DST-aware reinterpretation elsewhere. Safe.

**(f) Leap seconds**: Not supported (Finding 4).

**(g) Sub-second precision**: Supported only via the RFC3339 branch. chrono preserves nanosecond precision for valid RFC3339 inputs with fractional seconds.

**(h) Malformed strings**: All tested malformed inputs (`"not-a-date"`, `"2019-13-01T00:00:00"`, `"2024/01/15"`, `"15-01-2024"`, empty string, garbage) fall through to a `PyValueError`. No panics, no default values. Error message is generic but clear.

**(i) Year bounds**: chrono's `NaiveDate` range (`-262144-01-01` to `+262143-12-31`) far exceeds simulation needs. No integer overflow or panic risk for any practical date.

## Summary
- **Total findings**: 6
- **Critical**: 0
- **High**: 0
- **Medium**: 3
- **Low**: 3

## Recommendations

1. **Document the three-path behavior and offset inconsistency** (Finding 1). Update the doc comment on `parse_datetime_str` to explicitly state that RFC3339 inputs preserve their offset, while naive and date-only inputs are assigned UTC offset. Clarify that this is correct under HARES's local wall-clock time convention.

2. **Add sub-second support to naive and date-only parse paths** (Finding 2). Change the `NaiveDateTime::parse_from_str` format from `"%Y-%m-%dT%H:%M:%S"` to `"%Y-%m-%dT%H:%M:%S%.f"` (chrono's `%.f` captures optional fractional seconds). This would allow `"2024-01-15T14:30:00.123456"` and the date-only equivalent to parse correctly.

3. **Add `.%.f` to the date-only format string** (Finding 2, satellite fix). Since the date-only branch constructs `T00:00:00`, sub-second precision is irrelevant for that path, but adding `%.f` makes the parse format consistent with the naive branch after fixing Finding 2.

4. **Consider using `sigma` or `nanoseconds` in Python's `isoformat()` call** (Finding 3). After fixing Finding 2, `extract_datetime` would correctly handle naive datetimes with microseconds. As an additional safety net, consider calling `isoformat(timespec='seconds')` to truncate sub-seconds when they aren't needed, or call `isoformat(timespec='microseconds')` to ensure a consistent format.

5. **Add tests for the date-only parse path** (Finding 5). Test inputs like `"2024-01-15"`, `"2024-12-31"`, and an edge case like `"2024-02-29"` (leap day). Verify the returned wall-clock time is `00:00:00` with UTC offset.

6. **Extract the UTC conversion into a private helper** (Finding 6). Replace the duplicated `FixedOffset::east_opt(0)...from_utc_datetime` pattern with a `fn assume_utc(naive: NaiveDateTime) -> DateTime<FixedOffset>` helper.

7. **Add leap-second and sub-second test cases** to document expected behavior. Even if leap seconds are rejected, a test makes the behavior explicit and prevents silent regressions.

## References / Citations
- `crates/hares-core/src/clock.rs:1-11` — HARES local wall-clock time convention documentation
- `crates/hares-io/src/config.rs:36-45` — `SimulationConfig.start_time` field documentation
- `crates/hares-python/src/py_dwelling.rs:2062-2116` — existing tests for `parse_datetime_str`
- chrono 0.4.44 `DateTime::parse_from_rfc3339` — RFC3339 parsing preserving offset
- chrono 0.4.44 `FixedOffset::from_utc_datetime` — UTC-to-offset conversion (identity for offset 0)
- chrono 0.4.44 `%.f` format specifier — fractional second parsing in strftime-style format strings
