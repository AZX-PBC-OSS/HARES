# TOML Resample Method Path Silently Discards Unknown Names

**Severity**: High
**Priority**: P1
**Status**: Open
**Areas**: hares-python, hares-io/weather

## Problem

`crates/hares-python/src/py_config.rs:579` parses TOML configuration for resample method names and silently discards unknown values, falling through to the default. The companion Python kwargs path correctly raises `PyValueError` on the same input. This inconsistency violates `feedback_no_silent_defaults`: a user typo or an API mismatch in the TOML config produces a silently-different simulation, while the same typo via Python kwargs raises a loud error.

Specifically: a TOML file with `resample_method = "tringular"` (typo of "triangular") parses without complaint and uses the default method (`Zoh` for solar after ticket 030). The user's intent — and the resulting simulation — diverge with no signal.

## Current Behavior

`crates/hares-python/src/py_config.rs:579` (TOML path):
```rust
let method = parse_resample_method(name).unwrap_or_default();  // silent fallback
```

(Exact code may differ; the defect is the silent fall-through to default on parse failure.)

By contrast, the Python kwargs path uses `parse_resample_method(name)?` and raises `PyValueError("unknown resample method: ...")` to the caller.

## Required Behavior

1. The TOML path must return `PyValueError` (or the equivalent Rust error variant that translates to `PyValueError`) when the resample method name is unknown.
2. The error message must list the valid method names (`"zoh"`, `"linear"`, `"triangular"`, etc.) so the user can fix the typo immediately.
3. Both TOML and Python kwargs paths must share a single `parse_resample_method` helper and the same error type.

## Approach

1. Locate `crates/hares-python/src/py_config.rs:579` and identify the TOML resample-method parsing block.
2. Replace the silent default fall-through with explicit error propagation. Use the same `parse_resample_method` helper used by the Python kwargs path (or extract one if the two paths use different parsers).
3. Surface the error as `PyValueError` via PyO3.
4. Ensure the error message includes the offending name and the list of valid names.
5. Add a Python integration test parsing a TOML file with an unknown method name and asserting `PyValueError` is raised.

## Definition of Done

- [ ] `parse_resample_method` returns `Result<ResampleMethod, ResampleMethodError>` with no silent default
- [ ] TOML and Python kwargs paths both consume the result and raise `PyValueError` on error
- [ ] Error message lists valid method names
- [ ] Python integration test: TOML with `resample_method = "tringular"` raises `PyValueError`
- [ ] Python integration test: TOML with `resample_method = "zoh"` parses successfully (regression)

## Verification

```bash
cargo test -p hares-python
uv run pytest python/tests/test_config_resample.py
```

## References

- Project policy `feedback_no_silent_defaults.md` — never silently substitute fallback values for missing/invalid input; error loudly.
- PyO3 documentation §"Error handling" — converting Rust errors to `PyValueError`.
- HARES `crates/hares-io/src/weather.rs` — `ResampleMethod` enum definition.

## Related Tickets

- 030-solar-upsampling-triangular-mean-error (default method for solar channels)
- 098-triangular-resample-docstring-or-mean-preserving (related Triangular method correction)

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-21

### Code Confirmation

- [x] Referenced line numbers still match — `if let Ok(m) = parse_resample_method(v)` is at line 579 of `crates/hares-python/src/py_config.rs` (confirmed).
- [x] Described logic matches current implementation — the `if let Ok` pattern at line 579 does silently discard `Err` from `parse_resample_method`, confirmed.
- [x] OCHRE cross-check result: N/A — there is no equivalent TOML or dict-based resample override path in OCHRE; HARES introduces this API surface entirely.
- [x] EnergyPlus cross-check result: N/A — this ticket concerns the Python API error-handling contract, not a physics algorithm. No EnergyPlus formula is involved.

**Key finding not in the ticket:** the silent-discard at line 579 is inside `PyDwellingConfig::to_dwelling_config()` (the Rust → Rust conversion method), **not** in a TOML parsing path. No TOML parsing of `DwellingConfig` exists anywhere in the codebase (`grep -r "DwellingConfig.*toml" crates/` returns no results).

**The silent discard is currently unreachable from user-facing code** because `PyDwellingConfig::new()` (lines 423–429 of the same file) validates the method value string at construction time and returns `PyValueError` on any unknown name before it can be stored in the struct's internal `HashMap<String, String>`. There is no setter for `resample_overrides` on `PyDwellingConfig`, and the `from_py_object` extraction path only clones an already-constructed (and thus already-validated) instance.

Empirical confirmation: `DwellingConfig(resample_overrides={"dry_bulb": "tringular"})` already raises `ValueError: invalid resample method 'tringular'` — verified by `tests/python/test_py_config.py::TestDwellingConfig::test_resample_overrides_typo_raises_value_error` (added by this audit; passes green).

**Additional asymmetry identified:** the `py_dwelling.rs` `build_config()` function (used by `from_hpxml` and related entry points) has its own inline `parse_method` closure at lines 1657–1669 that also raises `PyValueError` correctly on unknown method names. It does **not** share `parse_resample_method` from `py_config.rs`. The ticket's requirement 3 (shared helper) is a legitimate code-quality concern: two independent parsers with slightly different error message wording (`"invalid resample method: '{}'. Expected ..."` vs `"unknown resample method '{}' for field '{}'; valid methods: ..."`) could drift out of sync.

### Web-Verified Citations

- **Citation**: PyO3 documentation §"Error handling" — converting Rust errors to `PyValueError`.
  - **Source found**: <https://pyo3.rs/v0.22.5/function/error-handling>
  - **Quoted passage**: "All built-in Python exception types are defined in the `pyo3::exceptions` module … these types feature a `new_err` constructor … Passing Python exceptions through Rust code then uses all the 'normal' techniques such as the `?` operator, with `PyErr` as the error type." The guide explicitly contrasts `?` (propagates the error to Python) with patterns that silently drop the `Err` arm.
  - **Verdict**: confirmed — the `if let Ok(m) = parse_resample_method(v)` pattern at line 579 is the anti-pattern the documentation warns against; `parse_resample_method(v)?` is the idiomatic fix.

- **Citation**: `feedback_no_silent_defaults.md` project policy.
  - **Source found**: Not found in the repository (no file matching `feedback_no_silent_defaults*` exists under `/Users/rich/source/HARES`).
  - **Verdict**: Cannot confirm the policy document itself, but the principle it describes (error loudly rather than silently substituting defaults) is enforced by the existing `PyDwellingConfig::new()` code, so the spirit of the policy is already met for user-facing input.

- **Citation**: `HARES crates/hares-io/src/weather.rs — ResampleMethod enum definition`.
  - **Source found**: `/Users/rich/source/HARES/crates/hares-io/src/weather.rs` line 232.
  - **Quoted passage**: `pub enum ResampleMethod { #[default] Pchip, PchipCyclic, Zoh, Linear, CircularLinear, Triangular, }` — the `#[default]` is `Pchip`, **not** `Zoh`.
  - **Verdict**: partially incorrect — the ticket states "uses the default method (`Zoh` for solar after ticket 030)", but the `#[derive(Default)]`-derived default for `ResampleMethod` is `Pchip` (the `#[default]` attribute). The `Zoh` default mentioned in the ticket refers to per-field defaults set elsewhere (e.g., in `ResampleOverrides::default()` at lines 326–337, where every field is `Some(ResampleMethod::Zoh)`), not the enum's own `Default` impl. This is a description inaccuracy in the ticket, not a code defect.

### Legitimacy

- **Verdict**: Partially Legitimate
- **Rationale**: The core concern — that a silent `if let Ok` discard in `to_dwelling_config()` could let a typo propagate undetected — is a real latent code defect (line 579 of `py_config.rs`). However, the bug is **not user-facing today**: `PyDwellingConfig::new()` validates the method name at construction time (lines 423–429), so the `Err` branch at line 579 is currently unreachable via the public Python API. The ticket mislabels the affected code path as "TOML" when no TOML parsing exists for `DwellingConfig`. It also misstates the default fallback enum value (the ticket says `Zoh`; the enum's `Default` impl yields `Pchip`). The secondary code-quality concern (two independent `parse_method` implementations in `py_config.rs` and `py_dwelling.rs` with slightly different error messages) is valid and worth fixing regardless of the reachability question. A focused fix should: (1) eliminate the dead `if let Ok` at line 579 by replacing it with `?`-propagation, and (2) extract a single shared `parse_resample_method` helper consumed by both files.

### Proposed Fix Summary

1. Change `to_dwelling_config()` at line 579 from:
   ```rust
   if let Ok(m) = parse_resample_method(v) {
       overrides.$field = Some(m);
   }
   ```
   to:
   ```rust
   overrides.$field = Some(parse_resample_method(v)?);
   ```
   and make the containing closure return `PyResult<ResampleOverrides>`, propagating the error through `.map(|map| { ... })` → `.transpose()`.

2. Move `parse_resample_method` to a shared location (e.g., `crates/hares-python/src/utils.rs` or a new `resample.rs`) and replace the inline `parse_method` closure in `py_dwelling.rs:1657–1669` with a call to the shared helper.

3. No production-code change is needed to make invalid-method typos raise `ValueError` for the end user — that already works via `PyDwellingConfig::new()`. The fix is defensive hardening of the conversion path.

### Test Written

- File: `tests/python/test_py_config.py`
- What it tests:
  - `test_resample_overrides_typo_raises_value_error` — asserts that `DwellingConfig(resample_overrides={"dry_bulb": "tringular"})` raises `ValueError` matching `"tringular"`. **Currently passes** (confirming the constructor-level guard is effective). Would become a failing test if the constructor guard were removed without fixing line 579.
  - `test_resample_overrides_all_valid_methods_accepted` — regression guard confirming all six valid method names are accepted without error.
