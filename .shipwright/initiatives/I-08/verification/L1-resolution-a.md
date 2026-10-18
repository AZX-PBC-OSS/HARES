# L1 resolution a: plan.md §1 and §3, 01-baseline-repair, 02-measurement, 07-profiler

Scope: every finding in `verify-L1-a.md`, and the findings in `verify-L1-e.md`
and `verify-L1-d.md` that concern these files. Binding input: `decision-log.md`,
including the rewritten `dl-i08-architecture.dependencies` (every dependency
and tool to its latest release, rust-version as required, advisory check with
no ignores, no license policy).

Code claims were re-checked at `ebdb1a17`, against PR #27
(`origin/fix/data-host-renamed-nlr`, merge base `2ee83a86`), crates.io and
PyPI, and the consumer at `c71c542`.

## verify-L1-a

| ID | Result |
|---|---|
| B1 | fixed: `plan.md` §3 "Opening sequence" + `01` BR-5. The release-only failures are repaired by OR-5, run in BR-5's slot before any entry that runs the block: "Task 0, M-0, BR-1, BR-3, BR-2, OR-5 (BR-5's repair ...), BR-4, BR-7, BR-8, M-1, M-2, BR-6a, BR-6b, BR-6c, BR-6d". BR-5: "**Commit:** none; OR-5's commit, landing in this position." |
| B2 | fixed: `01` order "BR-1, BR-3, BR-2"; BR-2 "**Dependencies:** BR-1, BR-3." |
| B3 | fixed: `plan.md` §3 block: "`uv run --locked --all-extras --all-groups maturin develop --release --uv`" and "`uv run --all-extras --all-groups --no-sync pytest -m "" -rs -p no:cacheprovider`", with "`-m ""` replaces the `-m 'not slow and not ochre'` in `pyproject.toml` `addopts`". BR-6c uses `[python-build]`/`[pytest]`. |
| B4 | fixed: `07` PR-3: "Delete the `debug_assert_eq!`: violations are counted and reported, never a panic"; per-thread counter; test `profiled_steps_count_allocations_without_panicking`. |
| B5 | already fixed in §2.2 by its owner (`BatchCallback` trait in `hares-io`, `Callback` marker variant, `hares-python` implements it); verified present. |
| M1 | fixed: `plan.md` §3 Task 0 steps 1-2 and `00-schedule` contract below. "a fast-forward yields exactly the recreated branch without deleting anything". The proposal to reword `dl-i08-process.branch` to "fast-forwarded" is rejected: a fast-forward of a branch with no commits of its own is the recreation the decision describes. |
| M2 | rejected: `dl-i08-architecture.dependencies` says "no license policy is added"; `[advisories]` runs cargo-deny's advisory check only, no `deny.toml`. |
| M3 | fixed: `01` BR-6a "raise rust-version to 1.89 and collapse nested ifs into let chains" (own commit, before the crate upgrade); BR-6b names `crates/hares-python/build.rs:15` and `crates/hares-io/src/hpxml/building.rs:363-406`; "Fix every compile error and every new warning or lint properly; suppress none." |
| M4 | fixed: M-1 and M-2 precede BR-6a; BR-6b "`frame_golden delta` runs on every M-2 fixture before re-capture; any change ... is recorded per fixture and per column". |
| M5 | fixed: `01` BR-7 evidence lists all fourteen files including `crates/hares-core/dwelling_100_diagnostics.csv`, root `dwelling_1_diagnostics.csv`, `tests/python/dwelling_42.csv`; census covers `tests/python`; Task 0 lists them with the exact `git status ... | grep -E` command. The claim that `benches/common.rs:127` writes into the tree is wrong (it writes to `unique_temp_path`, `:126`); BR-7 says so. |
| M6 | fixed: `plan.md` §3 `[stray]` with exclusions for `target`, `.venv`, `__pycache__`, `.pytest_cache`, `.ruff_cache`, the built `_hares*.so`; "(`target/` and `.venv/` also on a clean checkout)"; `01` BR-7 adds CI steps to the `test` and `python-test` jobs. |
| M7 | fixed (per `verify-L1-e` 13, which overrides this finding's "keep ignored with a reason"): `01` BR-8 "eleven tests are `#[ignore]`d", all print-only, deleted; acceptance "`git grep -nE '#\[ignore' -- '*.rs'` prints nothing"; the two `solar_parity.rs` comments deleted. |
| M8 | fixed: `02` M-1 "*Manifest.*" (inputs, `[simulation]`, features, defaults, overrides) and sidecar; validity compared before `to_bits()`. |
| M9 | fixed: fleet manifest kind, `--columns`, schema change rule, `weather_wrap_900s` in M-2. The "pytest-side capture helper" is rejected in favour of dropping the Python-driven claim from `09` B-3 (contract 6 below): B-3's effect is a default value, reproduced by a manifest. |
| M10 | fixed: `[golden]` lines in §3; `02` M-1 acceptance tests `compare_identical_batches_passes`, `compare_reports_first_mismatch_row_and_column`, `compare_treats_nan_payloads_bitwise`, `compare_rejects_validity_mismatch`, and more; "listed in `[workspace] members`". |
| M11 | fixed: `02` M-2 table with inputs, start, duration, step, features, rows ("35 040", "480", "384"); "a year at 900 s has 35 040 rows with or without DST"; leap fixture "pins today's handling of a 2024 run over such a file, which ... B-1 changes". |
| M12 | fixed: `02` M-3 uses `consumer_shape_900s` (in-repo `bldg0176227`) through `frame_golden run`; "**Commit:** none (`ledger/` is not part of the repository)". |
| M13 | fixed: `07` PR-1 phase clock; "the phases partition the step exactly and there is no residual"; `water_heater`, `hvac`, `other` deleted; `DwellingProfilingSummary` named. |
| M14 | fixed: `[test-profiling]` in §3; `01` BR-4 adds `-F profiling` to CI. |
| M15 | fixed differently: `07` PR-2 removes the per-step reads entirely and reads once per `profiling_summary()` call (the peak is monotone), `Option<u64>`, `None` off Linux, no new dependency; test `profiling_summary_reports_process_high_water_mark`. |
| M16 | fixed: `07` PR-3 `crates/hares-types/src/alloc_count.rs`, thread-local counter, each binary installs `#[global_allocator]`, the three existing copies reuse it, exact-count test. |
| M17 | already fixed in §2 by its owner (`RunMetrics`, `HealthPolicy::strict()`, `InputSource`, actor identity by name); verified present. |
| minor 1 | fixed: BR-1 "`check_invariants` alone compiles, because `hares-core`'s `check_invariants` feature does not enable `hares-equipment/observe`". |
| minor 2 | fixed: BR-1 "read from `self.rainflow.sum_squared_dod_daily()` ... before `self.rainflow.reset_daily()` (`battery/mod.rs:1494`); `update_daily` only borrows the counter". |
| minor 3 | fixed: BR-2 "clippy fails under `observe`"; "default-feature clippy is clean". |
| minor 4 | fixed: BR-3 renamed `spring_forward_day_of_year_at_hourly_boundary_steps`; "Delete the `T-0212` comments and the comment that replaces an assertion". |
| minor 5 | fixed: BR-3 "The standing guard against nested tests is the rustc lint `unnameable_test_items`". |
| minor 6 | fixed: BR-3 "`#[test]` at 607 and starts at 608; the nested function occupies 650-727 (the file ends at 728)". |
| minor 7 | fixed: BR-4 evidence and change: "`ci.yml:36` becomes the `[clippy]` line", `--profile ci`. |
| minor 8 | fixed: the interim acceptance is gone; OR-5 lands before any entry runs `[test-release]`. |
| minor 9 | fixed: BR-6b "each exit 101 with `package ID specification ... did not match any packages`". |
| minor 10 | already fixed in §2.7 by its owner (`crates/hares-equipment/src/hvac/heat_pump_config.rs:262,297,483`). |
| minor 11 | already fixed in §2.2 by its owner (`conversions.rs:590`). |
| minor 12 | fixed: §3 "the 'reuse their f64 slots without reallocating' comment at `crates/hares-core/src/dwelling/mod.rs:6431-6433`". |
| minor 13 | fixed: §3 "7.0 GB across 61 top-level directories at `c71c542`, re-measured at execution". |
| minor 14 | fixed: §3 `[writing]` commands over the commit message and added lines; `[bench]` is `cargo bench --workspace --no-run`. |
| minor 15 | fixed: no `review.md` citation remains in 01, 02, 07; PR-4 cites `context.md` item 8; the noise rule is stated in M-3. |
| minor 16 | fixed: PR-4 "add `finish: StdDuration`" and `finish_s` to the binding; new `07` PR-5 "the non-`profiling` build raises `NotImplementedError`". |

## verify-L1-e (findings concerning these files)

| ID | Result |
|---|---|
| 3 (nalgebra before the golden) | fixed: M-1, M-2 before BR-6a/6b. The rest of 3 belongs to `08`/`09`/schedule. |
| 13 | fixed: `01` BR-8, all eleven removed, "prints nothing". |
| 15 | fixed: `[stray]` in the block; pytest line replaced; census diff; `gymnasium` installed via `--all-extras`; BR-7 covers `tests/python`. |
| 16 (history and dangling cites in these files) | fixed: BR-7 "earlier draft" gone; PR-2, PR-4, M-3 `review.md` cites gone. §2.10 belongs to §2's owner. |
| 18 | fixed: §3 Task 0 (row 0) and "**Closing sequence.** ... the operator pushes `feat/output-results` and opens a pull request into `initial-implementation`". |
| 19 (as stated in §3) | fixed: §3 "Consumer engine under test", "Consumer gate" ("its skipped cases are exactly M-0's base skips"), "Consumer pin" (`Engine:` trailer, one operator-approved `checkout --detach`, `run_all --report`, AGENTS rule 5). CN-9 re-pinning and the DE-trace payload belong to `08`. |
| 22 item 15 | fixed: PR-1 partition, `profiling_phases_partition_the_step`; PR-2 measurement against the release wall time. |
| 29 (golden defaults path) | fixed: manifest `defaults = "defaults"` "resolved to an absolute path", recorded in the sidecar. |
| 35 | fixed: M-3 item 5 (repeated construct-and-simulate, 10^6 steps, 100 stepped episodes). The RL-3 entry belongs to `05`/`06`. |
| 36 | fixed: metrics, billing and health products; adapter updated by OR-3/OR-6 (contract 4); row criterion; `consumer_shape_900s`; golden before the upgrade. |
| 37 (PR-2, PR-3, PR-4) | fixed: each decided; PR-4 evidence `mod.rs:2869`. |
| 39 | fixed: opening sequence; `[test-profiling]`. |
| 43 | fixed: BR-6b "no ignore list. A new advisory that appears later is fixed in the entry that surfaces it"; CI `advisories` job. |
| 44 (BR-5 row) | fixed in `01`; schedule change in contract 2. |
| 52 | fixed: BR-4 (clippy line, debug default, dst, profiling, plain release), BR-6b (advisories), BR-7 (stray). |
| 55 | rejected: a public changelog per numeric commit is not in the decision log and adds a shipped artefact outside I-08's purpose; deltas are recorded in the measurement log. |

## verify-L1-d (findings concerning these files)

| ID | Result |
|---|---|
| 1 | fixed: §3 "Consumer engine under test": `uv sync --locked --extra engine --no-install-package ochre-next`; `VIRTUAL_ENV=... uvx --from 'maturin>=1.15,<2' maturin develop --release --uv`; `HARES_REPO=~/src/HARES uv run --no-sync <command>`; `paths.summary()` recorded. `08` must match (contract 3). |
| 2 | fixed for these files: §3 "Consumer gate" (`run_all`, `build_bundle.py`, `ruff check`, skip and failure lists) and `02` M-0 item 3 (base lists at `c71c542`). Per-CN verification text belongs to `08`. |
| 3 (fixture) | fixed: `02` M-2 `consumer_shape_900s`. B0 timing belongs to `08`/schedule. |
| 16 | fixed: §3 "Consumer pin" (one final bump, operator checkpoint, `--report`, `build_bundle.py`, push rule). |
| 18 | fixed: as L1-a B1. |
| 22 | fixed: §3 operator checkpoint names `sweep_scratch` as not to be used and states 61 directories. The CN-8 text belongs to `08`. |
| 36 (BR-5 row) | fixed in `01`; schedule change in contract 2. |

## Contract additions for the integrator

1. **plan.md §2.7, dependency paragraph.** Replace "The binding and columnar
   stack is pyo3 0.29 ... in `ledger/01-baseline-repair.md` BR-6. The four are
   mutually compatible (a throwaway compile check ...)" with the decided
   upgrade: every Rust dependency to its latest release (arrow and parquet at
   59, the latest that pyo3-arrow 0.19 accepts), rust-version 1.89, both
   repositories' Python lockfiles, maturin; `ledger/01-baseline-repair.md`
   BR-6a to BR-6d; advisory check with no ignores. The current text carries
   history ("throwaway compile check") and the superseded scope.
2. **00-schedule.** Rows, in order: Task 0 (operator: PR #27 merge,
   fast-forward, `i08-consumer`, strays), M-0 (no commit), BR-1, BR-3, BR-2,
   OR-5 (closes BR-5; BR-5 has no row of its own), BR-4, BR-7, BR-8, M-1,
   M-2, BR-6a, BR-6b, BR-6c, BR-6d (consumer repository), M-3 (no commit),
   then the rest; PR-1, PR-2, PR-3 after M-3; new PR-5 after PR-1; PR-4
   after OR-6; B0 captured after BR-6d; closing rows (push, pull request,
   consumer pin bump). Commit messages as now written in `01`, `02`, `07`.
3. **08-consumer.** The preamble's engine-under-test commands, every CN
   entry's verification, and the pin rule are §3's text verbatim (no
   per-commit pin advance; `Engine: <sha>` trailer; one final bump). B0 is
   captured after BR-6d. `consumer_shape_900s` is the engine-side fixture
   whose `[golden]` result shows engine rows are consumer-neutral.
4. **03-output-reporting.** OR-5 runs in the opening sequence after BR-2.
   Two corrections it needs: its change should list the signature ripple of
   a returned drift error (`refresh_equipment_caches`, `mod.rs:3794`, is
   called by `auto_register_actors` `:3167`, `clear_equipment` `:3574`,
   `remove_equipment_by_end_use` `:3605`, all infallible today, with callers
   in `hares-python` and four test files); and
   `soc_out_of_bounds_fails_in_release` cannot work as written, because
   `check_soc` only warns and clamps (`invariants.rs:313-329`) and `Soc`
   rejects 1.5 at construction (`equipment_id_regressions.rs:404`). OR-3 and
   OR-6 update `tools/frame_golden/src/adapter.rs` in their commits; OR-5 or
   OR-12 adds the `health` product; OR-7 follows M-1's schema change rule.
   The `frame_golden capture schema_v0..v8` reference in OR-1 needs one
   manifest per verbosity under M-1's manifest format.
5. **06-runtime-memory.** RT-1 and RT-3 install
   `hares_types::alloc_count::CountingAllocator` in their own test binaries
   and assert `Some(0)`; "a weather-wrap fixture" is M-2 `weather_wrap_900s`.
6. **09-numeric-fixes.** Drop "re-captured for every Python-driven golden
   fixture" from B-3; numeric entries record with `frame_golden delta`, then
   `capture`; fixture names as in M-2.
7. **05-python-surface / package rename.** The rename entry updates the
   engine distribution name in §3's first consumer command; PR-5 edits
   `_hares.pyi`.

## Operator questions

1. HARES#31 also asks to surface the `actor_profiling` per-actor timings
   through the Python bindings. The decision log does not mention it, so no
   entry adds it. Should I-08 include it?
2. M-2 commits six golden fixtures as ZSTD Parquet to the public repository,
   three of them full-year 900 s frames at verbosity 2 (sizes are recorded
   when captured). Is that acceptable in the public tree, or should goldens
   be kept smaller (for example fewer days or a lower verbosity)?

## Counts

- verify-L1-a: 38 findings: 33 fixed (M1, M7, M9 and M15 by a different
  mechanism than the one proposed, as stated in their rows), 4 already fixed
  by the §2 owner (B5, M17, minors 10 and 11), 1 rejected (M2).
- verify-L1-e: 16 findings concerning these files: 15 fixed, 1 rejected (55).
- verify-L1-d: 7 findings concerning these files: 7 fixed.
- Totals: 61 handled: 55 fixed, 4 already fixed, 2 rejected; 7 integrator
  contract items; 2 operator questions.
