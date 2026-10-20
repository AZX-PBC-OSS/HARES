# OR-5 Verification Block Results

- Repo: /home/rich/src/HARES @ dc033ceeb9d607960dd977b9e2cd26e5603a598b (branch feat/output-results)
- Consumer: /home/rich/src/HARES_CALIBRATION_SYSTEM @ aec2e3f6 (branch i08-consumer)
- Date: 2026-09-30
- Constraints: cargo `-j 8` / nextest `--build-jobs 8`, all commands sequential.

| # | Line | Exit | Elapsed | Notes |
|---|------|------|---------|-------|
| 1 | fmt | 0 | 1s | ok |
| 2 | clippy | 0 | 9s | ok |
| 3 | test-debug | 0 | 115s | 7065 run: 7065 passed, 12 skipped |
| 4 | test-observe | 0 | 121s | 7124 run: 7124 passed, 16 skipped |
| 5 | test-dst | 0 | 104s | 7078 run: 7078 passed, 12 skipped |
| 6 | test-profiling | 0 | 134s | 7065 run: 7065 passed, 12 skipped |
| 7 | test-observe-profiling | 0 | 147s | hares-core: 1331 run: 1331 passed, 15 skipped |
| 8 | test-release-invariants | **101** | 30s | **FAIL (compile)** — first error: `error[E0425]: cannot find type StageSnapshot in this scope` at crates/hares-core/src/dwelling/mod.rs:1558. Root cause: `struct StageSnapshot` is gated `#[cfg(debug_assertions)]` (mod.rs:228) while the field `stage_snapshot` is gated `#[cfg(any(debug_assertions, feature = "check_invariants"))]` (mod.rs:1558); in a release build with `-F check_invariants` (debug_assertions off) the type does not exist. Pre-existing feature-gating mismatch; recorded only, not fixed. |
| 9 | test-release | 0 | 64s | 6985 run: 6985 passed, 12 skipped |
| 10 | bench | 0 | 3s | ok (benches compiled, --no-run) |
| 11 | advisories | **1** | 1s | **EXPECTED FAIL** — exactly six RUSTSEC advisories (to be fixed by BR-6b): RUSTSEC-2023-0089 (atomic-polyfill unmaintained), RUSTSEC-2024-0436 (paste unmaintained), RUSTSEC-2026-0176 (pyo3 OOB read), RUSTSEC-2026-0177 (pyo3 PyCFunction Sync), RUSTSEC-2026-0194 (quick-xml quadratic dup attributes), RUSTSEC-2026-0195 (quick-xml NsReader unbounded allocation) |
| 12 | golden | **101** | 0s | **EXPECTED FAIL** — `error: package(s) frame_golden not found in workspace` (crate does not exist yet) |
| 13 | python-build | 0 | 34s | ok — ochre_next 0.1.0 wheel built for CPython 3.13, installed editable |
| 14 | pytest | 0 | 154s | 1377 passed, 9 skipped, 18 warnings. Skip census diff vs ledger: **IDENTICAL** (diff exit 0, 9 lines) |
| 15 | stray | matched | — | 12 matches, ALL gitignored (`!!`) recreations from test runs — expected pre-BR-7: `crates/hares-core/dwelling_0.csv`, `crates/hares-core/dwelling_1.csv`, `crates/hares-core/dwelling_100_diagnostics.csv`, `dwelling_0.csv`, `dwelling_0_diagnostics.csv`, `dwelling_1.csv`, `dwelling_2.csv`, `dwelling_3.csv`, `dwelling_42.csv`, `dwelling_42_diagnostics.csv`, `dwelling_88.csv`, `dwelling_99.csv`. No untracked (non-ignored) stray files. (Remaining non-CVS ignored entries are `.workstrees/target` build artifacts, not strays.) |
| 16 | writing | **1 (cmd2)** | 0s | cmd1 (commit message em-dash): exit 1, no match — OK. **cmd2 (added lines in HEAD diff): exit 0, FOUND 5 lines with U+2014 em-dash in added `///` doc comments** of HEAD commit dc033cee ("fix: record health and enforce physics invariants in every build profile"): diff lines 1877, 1937, 2733, 2734, 2760 (doc comments in crates/hares-core/src/dwelling/mod.rs). MUST-FIND-NOTHING check FAILED. |

## PART 2 — B0 check (consumer repo, i08-consumer)

| # | Line | Exit | Elapsed | Notes |
|---|------|------|---------|-------|
| 17 | engine rebuild into consumer venv | 0 | 34s | ok — ochre-next reinstalled editable from /home/rich/src/HARES working tree (HEAD dc033cee) |
| 18 | B0 capture + compare | 0 | 157s + 0s | capture: wrote /tmp/opencode/i08/b0-check-or5.json (4 premises). compare vs B0-de-trace.json: **identical**, exit 0. (One pre-existing UserWarning re: building 63724 sample_weight, unrelated.) |
| 19 | check_losses | 0 | 498s | verdicts: `ashp IDENTICAL`, `elecfurn IDENTICAL`, `gasfurn IDENTICAL`; `RESULT: engine change did NOT move calibration` |

## Verdict summary

- **Green (16/17 must-be-green lines):** fmt, clippy, test-debug, test-observe, test-dst, test-profiling, test-observe-profiling, test-release, bench, python-build, pytest (+census identical), stray (only expected gitignored recreations), writing cmd1, engine rebuild, B0 compare (identical), check_losses (all IDENTICAL).
- **Expected failures (recorded, no action):** line 11 advisories (six RUSTSEC, BR-6b), line 12 golden (crate not yet created).
- **UNEXPECTED FAILURE line 8 (test-release-invariants):** compile error E0425 — `StageSnapshot` gated `#[cfg(debug_assertions)]` at crates/hares-core/src/dwelling/mod.rs:228 but field gated `#[cfg(any(debug_assertions, feature = "check_invariants"))]` at mod.rs:1558. Release + `-F check_invariants` therefore does not compile. Pre-existing on this branch; recorded only, nothing modified.
- **UNEXPECTED FAILURE line 16 cmd2 (writing):** HEAD commit dc033cee adds five `///` doc-comment lines containing U+2014 em-dash (crates/hares-core/src/dwelling/mod.rs, diff lines 1877, 1937, 2733, 2734, 2760). Commit message itself is clean.
- Nothing committed; no tracked file modified anywhere.
