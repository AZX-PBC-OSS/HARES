# M0 Block Results — feat/output-results @ 1d60f478 (2026-09-30)

Measurement run. Each line run separately with `bash -c` from repo root, in order.

## 1. [fmt]
- Command: `cargo fmt --check --all`
- Exit code: **0**
- Elapsed: 2s

## 2. [clippy]
- Command: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Exit code: **101** (FAILED)
- Elapsed: 12s
- First error:
  ```
  error[E0425]: cannot find value `sum_sq_dod` in this scope
      --> crates/hares-equipment/src/battery/mod.rs:1468:21
       |
  1468 |                     sum_sq_dod,
       |                     ^^^^^^^^^^ not found in this scope
  error: could not compile `hares-equipment` (lib) due to 1 previous error
  error: could not compile `hares-equipment` (lib test) due to 1 previous error
  ```

## 3. [test-debug]
- Command: `cargo nextest run --workspace --no-fail-fast`
- Exit code: **0**
- Elapsed: 117s

## 4. [test-observe]
- Command: `cargo nextest run --workspace --no-fail-fast -F observe`
- Exit code: **101** (FAILED — compile error, no tests ran)
- Elapsed: 14s
- First error:
  ```
  error[E0425]: cannot find value `sum_sq_dod` in this scope
      --> crates/hares-equipment/src/battery/mod.rs:1468:21
  error: could not compile `hares-equipment` (lib) due to 1 previous error
  error: could not compile `hares-equipment` (lib test) due to 1 previous error
  ```

## 5. [test-dst]
- Command: `cargo nextest run --workspace --no-fail-fast -F dst`
- Exit code: **0**
- Elapsed: 142s

## 6. [test-profiling]
- Command: `cargo nextest run --workspace --no-fail-fast -F profiling`
- Exit code: **0**
- Elapsed: 186s

## 7. [test-observe-profiling]
- Command: `cargo nextest run -p hares-core --no-fail-fast -F observe,profiling`
- Exit code: **101** (FAILED — compile error, no tests ran)
- Elapsed: 7s
- First error:
  ```
  error[E0425]: cannot find value `sum_sq_dod` in this scope
      --> crates/hares-equipment/src/battery/mod.rs:1468:21
  error: could not compile `hares-equipment` (lib) due to 1 previous error
  ```

## 8. [test-release-invariants]
- Command: `cargo nextest run --workspace --release --no-fail-fast -F check_invariants`
- Exit code: **0**
- Elapsed: 90s

## 9. [test-release]
- Command: `cargo nextest run --workspace --release --no-fail-fast`
- Exit code: **100** (FAILED)
- Elapsed: 80s
- Summary: 6957 tests run: 6954 passed, 3 failed, 12 skipped
- Failing test names (full):
  1. `hares-core dwelling::tests::build_equipment_column_map_still_panics_on_schema_drift_for_known_equipment` (should_panic test)
  2. `hares-core::equipment_id_regressions ev_capacity_check_fails_loudly_when_ev_telemetry_keys_are_absent`
  3. `hares-core::equipment_id_regressions ev_soc_check_fails_loudly_when_core_output_soc_is_absent`

## 10. [bench]
- Command: `cargo bench --workspace --no-run`
- Exit code: **0**
- Elapsed: 3s

## 11. [advisories]
- Command: `cargo deny --all-features check advisories`
- Exit code: **1** (FAILED)
- Elapsed: 1s
- 6 advisory errors:
  1. RUSTSEC-2023-0089 — `atomic-polyfill 1.0.3` (unmaintained)
  2. RUSTSEC-2024-0436 — `paste 1.0.15` (unmaintained)
  3. RUSTSEC-2026-0176 — `pyo3 0.28.3` (vulnerability: Out-of-bounds read in `nth`/`nth_back` for `PyList`/`PyTuple` iterators)
  4. RUSTSEC-2026-0177 — `pyo3 0.28.3` (vulnerability: Missing `Sync` bound on `PyCFunction::new_closure` closures)
  5. RUSTSEC-2026-0194 — `quick-xml 0.39.4` (vulnerability: Quadratic run time on duplicate attribute names)
  6. RUSTSEC-2026-0195 — `quick-xml 0.39.4` (vulnerability: Unbounded namespace-declaration allocation / DoS)

## 12. [golden]
- Command: `cargo run --release -p frame_golden -- compare-all`
- Exit code: **101** (FAILED — expected: crate does not exist yet)
- Elapsed: 0s
- Error:
  ```
  error: package(s) `frame_golden` not found in workspace `/home/rich/src/HARES`
  ```

## 13. [python-build]
- Command: `uv run --locked --all-extras --all-groups maturin develop --release --uv`
- Exit code: **0**
- Elapsed: 41s

## 14. [pytest]
- Command: `uv run --all-extras --all-groups --no-sync pytest -m "" -rs -p no:cacheprovider | tee target/pytest.log ; then { grep '^SKIPPED' target/pytest.log || true; } | sort > target/pytest-skip-census.txt`
- Exit code: **0**
- Elapsed: 274s
- Result: 1377 passed, 9 skipped, 17 warnings
- Skip census (target/pytest-skip-census.txt, also copied to run dir as pytest-skip-census.txt):
  ```
  SKIPPED [1] tests/python/test_observer_diagnostics.py:86: OCHRE not installed
  SKIPPED [1] tests/python/test_ochre_parity.py:185: OCHRE not installed
  SKIPPED [1] tests/python/test_py_conversions.py:163: ResStock test fixtures not available
  SKIPPED [1] tests/python/test_py_conversions.py:171: ResStock test fixtures not available
  SKIPPED [1] tests/python/test_py_conversions.py:179: ResStock test fixtures not available
  SKIPPED [1] tests/python/test_py_conversions.py:187: ResStock test fixtures not available
  SKIPPED [1] tests/python/test_py_conversions.py:195: ResStock test fixtures not available
  SKIPPED [1] tests/python/test_py_conversions.py:213: ResStock test fixtures not available
  SKIPPED [1] tests/python/test_thermal_trace.py:44: OCHRE not installed
  ```

## 15. [stray]
- Command: `git status --porcelain --ignored --untracked-files=all -- . ':(exclude)target' ':(exclude).venv' ':(exclude)*__pycache__*' ':(exclude)*.pytest_cache*' ':(exclude)*.ruff_cache*' ':(exclude)python/*/_hares*.so' | grep -E 'dwelling_[0-9]+(_diagnostics)?\.csv'`
- Grep exit code: **0** — MATCHED
- 12 stray dwelling CSVs found, all ignored (`!!`), none untracked (`??`):
  - `!! crates/hares-core/dwelling_0.csv`
  - `!! crates/hares-core/dwelling_1.csv`
  - `!! crates/hares-core/dwelling_100_diagnostics.csv`
  - `!! dwelling_0.csv`
  - `!! dwelling_0_diagnostics.csv`
  - `!! dwelling_1.csv`
  - `!! dwelling_2.csv`
  - `!! dwelling_3.csv`
  - `!! dwelling_42.csv`
  - `!! dwelling_42_diagnostics.csv`
  - `!! dwelling_88.csv`
  - `!! dwelling_99.csv`

## Summary

| # | Line | Exit | Elapsed | Notes |
|---|------|------|---------|-------|
| 1 | fmt | 0 | 2s | PASS |
| 2 | clippy | 101 | 12s | FAIL — E0425 `sum_sq_dod` (battery/mod.rs:1468) |
| 3 | test-debug | 0 | 117s | PASS |
| 4 | test-observe | 101 | 14s | FAIL — same E0425 compile error |
| 5 | test-dst | 0 | 142s | PASS |
| 6 | test-profiling | 0 | 186s | PASS |
| 7 | test-observe-profiling | 101 | 7s | FAIL — same E0425 compile error |
| 8 | test-release-invariants | 0 | 90s | PASS |
| 9 | test-release | 100 | 80s | FAIL — 3 tests (see section 9) |
| 10 | bench | 0 | 3s | PASS |
| 11 | advisories | 1 | 1s | FAIL — 6 RUSTSEC advisories |
| 12 | golden | 101 | 0s | FAIL — expected (crate not yet created) |
| 13 | python-build | 0 | 41s | PASS |
| 14 | pytest | 0 | 274s | PASS — 1377 passed, 9 skipped |
| 15 | stray | 0 | 0s | MATCHED — 12 ignored dwelling CSVs |








