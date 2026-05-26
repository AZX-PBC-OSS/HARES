# check-si-guard.sh: grep patterns and false positive handling for IP units
**Review ID**: scr-05
**Category**: scripts
**Date**: 2026-05-26

## Files Reviewed
- `scripts/check-si-guard.sh`
- `crates/hares-equipment/tests/si_guard.rs` (actual guard logic)

## Vendor/Reference Files Consulted
None

## Findings

### Finding 1: [Severity: critical] Guard uses substring matching instead of regex
**Description**: The guard is not a grep-based script at all. `check-si-guard.sh` (line 5) delegates to `cargo test -p hares-equipment --test si_guard`, and the Rust test at `crates/hares-equipment/tests/si_guard.rs:55` uses `content.contains(marker)` — bare substring matching with no word boundaries, no surrounding-whitespace qualifiers, and no contextual anchors.
**Code Location**: `crates/hares-equipment/tests/si_guard.rs:55`
**Root Cause**: `content.contains(marker)` is a literal substring check. It cannot distinguish `"BTU"` the unit from `"BTU"` inside `"BTU_PER_HR_PER_W"`, nor `"ft"` the unit from `"ft"` inside `"left"`, `"after"`, or `"shift"`. This fails review criteria (b) — the patterns lack the specificity required to avoid false positives and are not expressed as `\b`-anchored regex.
**Impact**: If expanded to cover common short unit strings (`ft`, `in`, `lb`, `F`, `gal`), substring matching would produce overwhelming false positives on variable names, error messages, and hex literals, making the guard unusable. Conversely, if kept restricted to long unique markers to avoid false positives, it misses all other IP unit strings — the current state.

### Finding 2: [Severity: critical] Guard markers reference non-existent code
**Description**: The test defines 4 forbidden markers (lines 32–43) — `BTU_PER_HR_PER_W`, `HeatingEfficiencyUnit::Seer`, `HeatingEfficiencyUnit::Eer`, `HeatingEfficiencyUnit::Hspf` — none of which appear anywhere in the `hares-equipment/src/` tree. A full recursive search (`rg -rn` across the entire crate and workspace) confirmed zero matches for all four markers.
**Code Location**: `crates/hares-equipment/tests/si_guard.rs:32-43`
**Root Cause**: These markers likely named code that was renamed, removed, or never committed. The allowlist is now dead.
**Impact**: The guard passes vacuously — it tests for markers that don't exist. Any new IP unit can be introduced without triggering a violation. The guard provides no protection.

### Finding 3: [Severity: critical] Pattern coverage is near-zero
**Description**: Only 4 markers are checked. The review criteria list required coverage for BTU, F (Fahrenheit), lb (pound-mass), ft (feet), inch/in, mph, psf, psi, gallon/gal, cfm, and other IP units. The current guard covers none of these. Real IP-unit references found in the codebase that slip through:
- `crates/hares-equipment/src/water_heater/mod.rs:1` — comment `"10 hr·ft²·°F/n — a"` (feet, Fahrenheit)
- `crates/hares-equipment/src/water_heater/resistance.rs:1` — comment `"10 hr·ft²·°F/n"` (feet, Fahrenheit)
- `crates/hares-equipment/src/scheduled_load.rs:1` — comment `"1 US therm = 100,000 n_IT"` (therm)
- `crates/hares-equipment/src/hvac/heat_pump_config.rs:61` — field name `fan_power_w_per_cfm` (cfm)
- `crates/hares-io/src/hpxml/resolve_hvac.rs` — extensive SEER/EER/HSPF conversion code (outside scan scope but semantically IP)
**Code Location**: `crates/hares-equipment/tests/si_guard.rs:31-43`
**Impact**: IP units can enter the codebase undetected. The guard's value proposition is negated.

### Finding 4: [Severity: high] No comment/string-literal exclusion
**Description**: The guard does not distinguish between IP units in executable code vs. explanatory comments or string literals. Review criterion (c) notes that IP units in comments (e.g., `// convert from BTU to J`) are arguably benign since they don't affect simulation math, yet the guard has no mechanism to exclude them. If markers were added for strings like `"BTU"`, every doc comment referencing the conversion would be flagged. Conversely, the current lack of markers means even doc comments with IP units are silently allowed — a false sense of security.
**Code Location**: `crates/hares-equipment/tests/si_guard.rs:52-60`
**Root Cause**: The design reads the entire file as a single string (line 52: `fs::read_to_string`) and applies `contains()` without any AST-aware or comment-aware filtering.
**Impact**: The guard cannot be tuned to balance comment-tolerance with code-level enforcement, and lacks documentation of which approach is intended.

### Finding 5: [Severity: high] No inline-annotation allowlist mechanism
**Description**: Review criterion (d) requires an allowlist mechanism by inline annotation (e.g., `// SI-GUARD-IGNORE: EnergyPlus reference value`). The current guard has only a static, hardcoded allowlist in the test source (`crates/hares-equipment/tests/si_guard.rs:31-43`) keyed by file-relative path. There is no per-line or per-block annotation that developers can use to suppress false positives in legacy migration code or unit-conversion tables.
**Code Location**: `crates/hares-equipment/tests/si_guard.rs:31-43`
**Root Cause**: The allowlist is a compile-time array of `(&str, &[&str])` tuples in test code — no runtime annotation parsing exists.
**Impact**: Any file that legitimately needs IP-unit references (e.g., HPXML parsing in `crates/hares-io/src/hpxml/resolve_hvac.rs`) cannot be handled by the guard without adding it to the test's hardcoded allowlist — but these files are outside the guard's scan scope anyway. If the scope were expanded, the static allowlist approach would not scale.

### Finding 6: [Severity: high] Not integrated into CI
**Description**: The CI configuration at `.github/workflows/ci.yml` runs only `cargo fmt --check` (line 25) and `cargo clippy --workspace -- -D warnings` (line 35). There is no `cargo test` step and no invocation of `scripts/check-si-guard.sh`. The script is not referenced from the `Makefile` either (`Makefile:67` only has `check: check-no-magic-config`).
**Code Location**: `.github/workflows/ci.yml:17-35`, `scripts/check-si-guard.sh:5`
**Root Cause**: The CI pipeline was never updated to include the guard. The script exists but is orphaned.
**Impact**: Even if the guard were fixed to have meaningful patterns, violations would never be caught in CI. Developers receive no automated feedback on IP-unit introductions.

### Finding 7: [Severity: medium] Violation messages lack line numbers and are not CI-annotatable
**Description**: Violation messages (line 56–58) use the format `"forbidden imperial marker '{marker}' found in {rel}"`. This omits the file line number, making it difficult for developers to locate the offending code. Additionally, the output does not use GitHub Actions workflow command syntax (`::error file=FILENAME,line=LINE::MESSAGE`) for inline annotations.
**Code Location**: `crates/hares-equipment/tests/si_guard.rs:56-58`
**Root Cause**: `fs::read_to_string` reads the file as a blob — the test never scans line-by-line, so line numbers are unavailable. The assertion error output (line 63-67) uses plain `assert!` with joined strings.
**Impact**: Developer must manually search the file for the marker substring. In CI, the failure appears as a generic test failure with no file-level annotation in the PR diff view.

### Finding 8: [Severity: medium] Scan scope limited to a single crate
**Description**: The guard only scans `crates/hares-equipment/src/` (line 25). Other crates that could introduce IP units — `hares-io` (which already contains SEER/EER/HSPF conversion code at `crates/hares-io/src/hpxml/resolve_hvac.rs`), `hares-physics`, `hares-types`, etc. — are not covered.
**Code Location**: `crates/hares-equipment/tests/si_guard.rs:25`
**Impact**: The workspace-level protection the script's name implies is not achieved. IP units in IO/periphery code are beyond the guard's reach.

### Finding 9: [Severity: low] `set -euo pipefail` but no `cargo test` invocation in CI
**Description**: The shell script correctly uses `set -euo pipefail` (line 2) so that `cargo test` exit codes propagate. This is good. However, the `cargo test` command passes no `--` filter (line 5), so it runs *all* tests in the `hares-equipment` crate, not just `si_guard`. This is inefficient: an unrelated test failure would block the guard results, and the full test suite may be slow.
**Code Location**: `scripts/check-si-guard.sh:5`
**Root Cause**: Missing `-- si_guard` argument to restrict to the specific test.
**Impact**: Guard execution is unnecessarily coupled to the full test suite. In CI, this would waste compute time and conflate failures.

## Summary
- Total findings: 9
- Critical: 3 (Findings 1, 2, 3)
- High: 3 (Findings 4, 5, 6)
- Medium: 2 (Findings 7, 8)
- Low: 1 (Finding 9)

## Recommendations
1. **Replace substring matching with line-by-line regex scanning.** Use Rust's `regex` crate with `\b`-anchored patterns for each IP unit string (`\bBTU\b`, `\b°?F\b`, `\blb[s]?\b`, `\bft\b`, `\bin(ch)?\b`, `\bmph\b`, `\bps[fi]\b`, `\bgallon\b`, `\bgal\b`, `\bcfm\b`, etc.). This resolves Findings 1, 2, and 3 simultaneously.
2. **Add a pre-filter to skip comment lines and string literals** or document that the guard intentionally flags comments to prevent drift. If comments are excluded, strip Rust line comments (`//...`) and block comments (`/*...*/`) before scanning. Resolves Finding 4.
3. **Add inline-annotation support.** Recognize `// si-guard-ignore` or `#[allow(si_guard)]` on the line above or same line as the IP unit reference. Resolves Finding 5.
4. **Integrate into CI.** Add a `cargo test -p hares-equipment --test si_guard` step to `.github/workflows/ci.yml`. Resolves Finding 6.
5. **Add line numbers and CI annotation output.** Scan line-by-line so line numbers are available. Format violations as `::error file=FILENAME,line=N::forbidden imperial marker: PATTERN`. Resolves Finding 7.
6. **Expand scan scope** beyond `hares-equipment/src/` to cover the full workspace, or at minimum `hares-io`, `hares-physics`, and `hares-types`. Resolves Finding 8.
7. **Narrow the cargo test invocation** to `cargo test -p hares-equipment --test si_guard -- si_guard` to run only the relevant test. Resolves Finding 9.

## References / Citations
- `crates/hares-equipment/tests/si_guard.rs` — actual guard logic
- `scripts/check-si-guard.sh` — thin shell wrapper
- `.github/workflows/ci.yml` — CI pipeline (no guard invocation)
- `Makefile` — no guard target
- Review criteria from `scripts/reviews/scripts.json:39-43` specifying grep-pattern and false-positive requirements
