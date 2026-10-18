# Final verification block — I-08

- Repo: /home/rich/src/HARES @ 7643b4e7 (feat/output-results)
- Consumer: /home/rich/src/HARES_CALIBRATION_SYSTEM @ aec2e3f6 (i08-consumer)
- Started: epoch 1790879894
- Constraint honored: -j 8 / --build-jobs 8, sequential; nextest parallelism from .config/nextest.toml (cap 8)
- Tracked-file integrity at end: no modified tracked files in either repo

## Results table

| # | command | exit | elapsed | counts |
|---|---------|------|---------|--------|
| 1 | `cargo fmt --check --all` | 0 | 2s | clean |
| 2 | `cargo clippy --workspace --all-targets --all-features -j 8 -- -D warnings` | 0 | 12s | 0 warnings, 0 errors |
| 3 | `cargo nextest run --workspace --no-fail-fast --build-jobs 8` | 0 | 120s | 7083 run: 7083 passed, 0 failed, 0 skipped |
| 4 | `cargo nextest run --workspace --no-fail-fast --build-jobs 8 -F observe` | 0 | 145s | 7142 run: 7142 passed, 0 failed, 0 skipped |
| 5 | `cargo nextest run --workspace --no-fail-fast --build-jobs 8 -F dst` | **100** | 123s | 7096 run: 7094 passed, **2 FAILED**, 0 skipped — see notes |
| 6 | `cargo nextest run --workspace --no-fail-fast --build-jobs 8 -F profiling` | 0 | 143s | 7083 run: 7083 passed, 0 failed, 0 skipped |
| 7 | `cargo nextest run -p hares-core --no-fail-fast --build-jobs 8 -F observe,profiling` | 0 | 147s | 1331 run: 1331 passed, 0 failed, 0 skipped |
| 8 | `cargo nextest run --workspace --release --no-fail-fast --build-jobs 8 -F check_invariants` | 0 | 76s | 7038 run: 7038 passed, 0 failed, 0 skipped |
| 9 | `cargo nextest run --workspace --release --no-fail-fast --build-jobs 8` | 0 | 69s | 7003 run: 7003 passed, 0 failed, 0 skipped |
| 10 | `cargo bench --workspace --no-run -j 8` | 0 | 3s | 18 bench executables built; 2 warnings (dead_code `UnhealthyStubActor`, unused imports `BoundaryCategory`/`BoundaryDiagnosticInfo`) |
| 11 | `cargo deny --all-features check advisories` | **1 (EXPECTED FAIL)** | 0s | exactly the 6 expected advisories (below) |
| 12a | `cargo run --release -p frame_golden -- compare-all` | 0 | 43s (12a+12b) | 5/5 identical: consumer_shape_900s, fleet_mixed_offset, full_year_hourly, leap_year_february_900s, weather_wrap_900s |
| 12b | `cargo run --release -p frame_golden -F dst -- compare-all` | 0 | (with 12a) | 1/1 identical: full_year_900s_dst |
| 13 | `uv run --locked --all-extras --all-groups maturin develop --release --uv` | 0 | 34s | ochre_next-0.1.0 built for CPython 3.13, installed editable |
| 14 | `uv run --all-extras --all-groups --no-sync pytest -m "" -rs -p no:cacheprovider` | 0 | 291s | 1383 items: 1377 passed, 9 skipped, 0 failed, 19 warnings; census diff **identical** |
| 15 | stray check `git status --porcelain ... \| grep -E 'dwelling_[0-9]+(_diagnostics)?\.csv'` | no match (grep 1) | 0s | **EMPTY** — no stray dwelling CSVs (BR-7 clean) |
| 16a | `git log -1 --format=%B \| grep -nP "\x{2014}"` | nothing (grep 1) | 0s | clean |
| 16b | `git show --format= --unified=0 HEAD \| grep -nP "^\+(?!\+\+).*\x{2014}"` | nothing (grep 1) | 0s | clean |
| 17 | consumer `uv pip install --python .venv/bin/python --no-deps --reinstall-package ochre-next --editable /home/rich/src/HARES` (CARGO_TARGET_DIR=/home/rich/src/HARES/.workstrees/target) | 0 | 37s | ochre-next 0.1.0 reinstalled editable from HARES |
| 18 | `capture_de_trace.py capture --out /tmp/opencode/i08/b0-final.json` then `compare .../B0-de-trace.json /tmp/opencode/i08/b0-final.json` | 0 / 0 | 172s | capture wrote 4 premises; compare printed **identical** |

## Line 5 failure detail (test-dst)

Two failures, both in `tools/frame_golden/tests/golden.rs`, both with the same signature:

```
FAIL frame_golden::golden golden_json_is_byte_stable
  panicked at tools/frame_golden/tests/golden.rs:63:84:
  called `Result::unwrap()` on an `Err` value:
  FeatureSetMismatch { manifest: "/tmp/.tmpFqeher/one_day.toml", required: [], running: ["dst"] }

FAIL frame_golden::golden capture_then_compare_round_trips
  panicked at tools/frame_golden/tests/golden.rs:90:6:
  called `Result::unwrap()` on an `Err` value:
  FeatureSetMismatch { manifest: "/tmp/.tmpmTdLQ2/one_day.toml", required: [], running: ["dst"] }
```

Interpretation (evidence-based): the golden harness's own FeatureSetMismatch guard rejects these two unit tests when the suite runs with `-F dst` because their generated fixture manifests declare `required: []`. It is a harness/fixture declaration mismatch, not a numerical golden mismatch — the `frame_golden -F dst -- compare-all` CLI (line 12b) passes with identical golden data. No files were modified per the run's no-mutation constraint. Logged at /tmp/opencode/i08/nextest-5.log.

## Line 11 advisories (expected, BR-6b entry)

1. RUSTSEC-2023-0089 — atomic-polyfill 1.0.3 unmaintained (via heapless → postcard)
2. RUSTSEC-2024-0436 — paste 1.0.15 unmaintained (via parquet, simba)
3. RUSTSEC-2026-0176 — pyo3 0.28.3 OOB read in nth/nth_back (upgrade ≥0.29.0)
4. RUSTSEC-2026-0177 — pyo3 0.28.3 missing Sync bound on new_closure (upgrade ≥0.29.0)
5. RUSTSEC-2026-0194 — quick-xml 0.39.4 quadratic duplicate-attribute check (upgrade ≥0.41.0)
6. RUSTSEC-2026-0195 — quick-xml 0.39.4 unbounded namespace-declaration allocation (upgrade ≥0.41.0)

## Census diff (line 14)

`/tmp/opencode/i08/census-final.txt` vs `.shipwright/initiatives/I-08/ledger/pytest-skip-census.txt`: **identical**, 9/9 lines (3× "OCHRE not installed", 6× "ResStock test fixtures not available").

## Stray check (line 15)

Empty. No `dwelling_*.csv` / `dwelling_*_diagnostics.csv` matches in git status (untracked/ignored), BR-7 landed.

## Golden (line 12)

- plain: `compared consumer_shape_900s: identical`, `compared fleet_mixed_offset: identical`, `compared full_year_hourly: identical`, `compared leap_year_february_900s: identical`, `compared weather_wrap_900s: identical` — exit 0
- dst: `compared full_year_900s_dst: identical` — exit 0

## B0 verdict (line 18)

`capture_de_trace.py compare B0-de-trace.json b0-final.json` → `identical`, exit 0 (4 premises). Rows since the last B0 check are neutral for the consumer.

## Totals

- Total elapsed: 1558s (~26.0 min)
- Line status: 17/18 exit 0; line 11 expected-fail (exit 1, six advisories as recorded); **line 5 unexpected-fail (exit 100, 2 golden unit tests under -F dst)**
- No commits, no tracked-file modifications in either repo
