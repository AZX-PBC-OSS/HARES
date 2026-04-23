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
