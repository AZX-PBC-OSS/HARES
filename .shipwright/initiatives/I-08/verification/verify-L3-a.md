# Verification L3-a: plan.md sections 1 and 3, the opening follow-ups, the profiler, the schedule

Scope: `plan.md` sections 1 and 3; `ledger/01-baseline-repair.md` BR-10,
BR-12, BR-13, BR-14, BR-15; `ledger/08-consumer.md` CN-17;
`ledger/07-profiler.md`; `ledger/00-schedule.md` as a whole.

Code state checked: engine `8b09ac4a` (archived into the scratch directory,
builds and probes run there only), consumer `5c651bc`, plus the engine
commits that landed while this round ran (`d7f0e381`, `21d90588`) and the
main tree's uncommitted work, read only.

Counts: 2 blockers, 3 majors, 14 minors.

## Findings

### 1. Blocker: execution has left the schedule and the readiness gate

**Entry:** `00-schedule.md` rows 13-1, 13a to 13g, 14 to 16; `plan.md` §3
opening sequence; `decision-log.md` `dl-i08-process.readiness`.

**Evidence:**
- Engine log: `8b09ac4a` (BR-11, row 13a), then `d7f0e381` "fix: partition
  the profiled step into phases that sum to the step time" (PR-1, row 14)
  and `21d90588` "perf: read the memory high-water mark once per summary"
  (PR-2, row 15). The main tree holds PR-3 (row 16) uncommitted: new
  `crates/hares-types/src/alloc_count.rs`, `crates/hares-core/tests/profiling_alloc.rs`,
  `tools/frame_golden/tests/global_allocator.rs`, and edits to
  `benches/rl_episode.rs` and the two envelope zero-allocation tests.
- Rows 13-1 (CN-17) and 13c to 13g (BR-10, BR-12, BR-13, BR-14, BR-15)
  have not run. The consumer's `i08-consumer` head is still `5c651bc`
  (BR-6d), and there is no `compare_ignores_the_header` case in it.
- The schedule says "a row starts only when the row above is committed,
  reviewed, red-teamed and green". The table places CN-17 (13-1) before
  BR-11 (13a), and BR-10 to BR-15 (13c to 13g) before PR-1 (14).
- `dl-i08-process.readiness`: "Rows 14 onward are cleared only when a
  fresh verification round over them records zero blockers and zero
  majors in plan-critique.md". `plan-critique.md` reads "Current status:
  NOT READY", and round L3, this round, is still running.
- The consumer-check labels no longer hold. The schedule preamble says
  rows 13a to 13g are checked "on the consumer after CN-17", but BR-11's
  B0 check ran on `5c651bc`. Rows 14 and 15 are labelled "B0 (CN-17)", and
  no CN-17 consumer exists. No engine worktree exists for `8b09ac4a`,
  `d7f0e381` or `21d90588` under `.workstrees/` (finding 5).

**Correction:**
- Stop the row-16 work in the main tree until this round clears.
- Add an execution record to `decision-log.md`. It states the order that
  actually ran: BR-11, M-3, PR-1, PR-2.
- Rewrite `00-schedule.md` and `plan.md` §3 to that order, with
  CN-17, BR-10, BR-12, BR-13, BR-14 and BR-15 as the next rows, ahead of
  PR-3, and a note that rows 13a, 14 and 15 ran before CN-17.
- After CN-17 lands, re-run the B0 check on the CN-17 consumer against
  worktrees of `8b09ac4a`, `d7f0e381` and `21d90588` through the
  mechanism, and record each result.
- Any L3 finding against PR-1 or PR-2 becomes a new fix entry, as `plan.md`
  §1 requires for committed rows. No destructive git operation is involved.

### 2. Blocker: BR-10's acceptance cannot pass, because the coverage job needs a Codecov token the repository does not have

**Entry:** BR-10, acceptance ("the first CI run ... starts and passes every
job").

**Evidence:** The last CI run on PR #27 is run 36028888222. Its `coverage`
job failed with `Commit creating failed: {"message":"Token required because
branch is protected"}`. The repository has no Actions secrets
(`gh api repos/:owner/:repo/actions/secrets` returns `total_count` 0).
`ci.yml:249-255` uploads with `codecov/codecov-action@v4` and
`fail_ci_if_error: true` and passes no token. The closing sequence's pull
request comes from a branch of the same repository, so tokenless upload
does not apply to it. No ledger entry touches this job.

That run's other failures were checked at `8b09ac4a`:
- The two docs-citation scripts exit 0.
- The `profile_sim` warning failure of the nextest job was fixed by BR-2.
- The bench `main` checkout failure is BR-10's own target.

**Correction:**
- BR-10 passes `token: ${{ secrets.CODECOV_TOKEN }}` to the Codecov step.
- `plan.md` §3 gains an operator checkpoint: add the `CODECOV_TOKEN`
  repository secret before the closing sequence's first push.
- If the operator declines, BR-10 instead removes the Codecov upload and
  keeps `cargo llvm-cov` as a job that must pass.
- BR-10's acceptance names the coverage job.

### 3. Major: BR-10 runs the bitwise golden comparison on moving runner and toolchain labels

**Entry:** BR-10, change ("The committed goldens are Linux x86_64
captures, which is the runner's platform").

**Evidence:**
- The goldens are per-column bit digests captured on Ubuntu 24.04 (glibc
  2.39) with rustc 1.94.1 (this host).
- The `test` job runs on `ubuntu-latest` with
  `dtolnay/rust-toolchain@stable` (`ci.yml:134`, `:141`). Both labels
  move: `ubuntu-latest` changes its Ubuntu release, and with it the glibc
  libm whose transcendental results the physics uses, and `stable` changes
  rustc.
- `02-measurement.md` M-4a pins `ubuntu-24.04` for exactly this reason,
  and expects cross-platform differences.
- Bit-exact agreement across libm or LLVM versions is not guaranteed, so
  `[golden]` in CI can fail for reasons unrelated to the change under
  test.

**Correction:**
- BR-10 runs the two `[golden]` steps on `runs-on: ubuntu-24.04`, the
  platform the goldens were captured on.
- It pins the toolchain for those steps to the version the capture used:
  either a `rust-toolchain.toml`, or `dtolnay/rust-toolchain@<version>`
  read from the M-2 `[tools]` record.
- It states that any toolchain move re-runs `compare-all` locally first,
  with the delta procedure of M-1.

### 4. Major: BR-12 tells the entry to fix any number-moving defect inside row 13d, ahead of the consumer migration

**Entry:** BR-12, change (ignored-test run: "if the cause lives in code
that still exists at this entry's parent, it is fixed and pinned by a test
in this entry") and acceptance (a seasonal-test failure "is a physics
defect investigated and fixed").

**Evidence:**
- `dl-i08-architecture.consumer`: numeric fixes land after the mechanical
  consumer migration.
- `00-schedule.md` rules: each numeric fix has its own row, and all of them
  follow the consumer block (row 78a).
- Row 13d is before both, so a physics fix found there has no legal place
  in BR-12, and the implementer has to invent one.

A scratch probe at `8b09ac4a` (release, the four seasonal configurations,
72 h at 3600 s, one warm-up day) shows that the four seasonal assertions
pass today:

| Release | Building | Season | End use | kWh |
|---|---|---|---|---|
| 2025.1 | bldg0000004 | summer | `hvac_cooling` | 24.5 |
| 2024.2 | bldg0000004 | summer | `hvac_cooling` | 43.6 |
| 2025.1 | bldg0000002 | winter | `hvac_heating` | 7.62 |
| 2024.2 | bldg0000002 | winter | `hvac_heating` | 2.37 |

The ignored-test run at `b674a767^` has not been done, so its outcome is
unknown.

**Correction:** BR-12 states the rule:
- A defect found by either run whose fix is bitwise-neutral (`[golden]`
  unchanged and B0 equal) is fixed in BR-12.
- A fix that moves numbers becomes a new `09-numeric-fixes.md` entry,
  scheduled after row 78a with its delta recorded.
- The failing test stays in BR-12 as a reproduction that is expected to
  fail. It is not ignored: it asserts the current wrong value and links to
  the new entry, which inverts the assertion.

### 5. Major: BR-6d's acceptance is unrecorded and was not run through the engine-under-test mechanism, and CN-17 builds a rule on that missing record

**Entry:** CN-17, change ("The base on a regenerated schema is the BR-6d
gate's if its record in `02-measurement.md` shows the schema was
regenerated ..."); BR-6d, acceptance; `00-schedule.md` row 13 "gate+B0".

**Evidence:**
- `02-measurement.md` has no BR-6d record, and
  `.shipwright/runs/v2-implement-*/` holds none either.
- The consumer venv's engine is
  `ochre_next-0.1.0.dist-info/direct_url.json` =
  `{"url":"file:///home/rich/src/HARES","dir_info":{"editable":true}}`.
  That is an editable install of the main engine tree, which is where rows
  are being implemented, not a `.workstrees/<sha>` worktree.
- `.workstrees/` holds `1d60f478`, `fd23dd80` and build directories only.
- The only capture after BR-6c is `scratch/i08/br11-redteam/capture.json`.
  Its header reads `hares_repo` `/home/rich/src/HARES`, `engine_so`
  `/home/rich/src/HARES/python/ochre_next/_hares...so`, engine
  `75a182a0`, consumer `5c651bc`. It compares `identical` to B0 (re-run
  here).
- `scratch/i08/872cf289/` is empty, and no `gate.txt` exists for BR-6d.
- `results/engine_schema.json` (06:01, `engine_sha256` `0e673f6cd097168b`)
  was emitted against that main-tree editable install.

**Correction:**
- CN-17 drops the conditional. Its gate, run through the full mechanism on
  a worktree of the engine commit it gates on, records the base skip set
  on a regenerated schema.
- It also records BR-6d's acceptance, which it now establishes: the
  consumer gate and a DE-trace equal to B0 for the upgraded lock. If the
  trace differs, BR-6d's split contingency in the `08-consumer.md`
  preamble applies.
- The B0 checks of rows 13a, 14 and 15 are re-run through the mechanism
  (finding 1).
- `08-consumer.md` adds one line: no gate or capture may run against an
  editable install of `/home/rich/src/HARES`. Step 4's dry-run line, which
  names `.workstrees/<sha>`, is the check, and it is recorded with every
  run.

### 6. Minor: BR-10 makes the bench job compare a pushed commit with itself

**Entry:** BR-10, change.

**Evidence:** On a push, `github.ref_name` names the pushed branch itself,
so the baseline and the measured commit are the same commit.

**Correction:** On a push, BR-10 takes the baseline from
`github.event.before`. On a pull request it keeps `github.base_ref`.

### 7. Minor: the `test` job uploads the nextest JUnit file from the wrong path

**Entry:** BR-10 (the `test` job it edits).

**Evidence:** `ci.yml:167-171` uploads `nextest-results.xml` from the
repository root. nextest writes `junit.path` relative to
`target/nextest/<profile>/`, and run 36028888222 logged "No files were
found with the provided path". The stray-check exclusion of a root
`nextest-results.xml` is moot for the same reason.

**Correction:** In BR-10, upload `target/nextest/ci/nextest-results.xml` and
drop the root exclusion.

### 8. Minor: BR-10's acceptance runs only at row 99a and says nothing about a failure there

**Entry:** BR-10, acceptance.

**Evidence:** The acceptance is the first CI run after the closing push,
long after BR-10's commit, and the entry gives no handling for a failure at
that point.

**Correction:** Add: "a job that fails at row 99a becomes a fix entry
committed before M-4b, and the operator pushes again."

### 9. Minor: BR-12's heating assertion for a gas-heated home names the wrong quantity

**Entry:** BR-12, seasonal assertions.

**Evidence:**
- `bldg0000002` heats with gas: a boiler in 2025.1 and a furnace in 2024.2.
  `total_energy_kwh.per_end_use` is electric only, so "heating runs" is
  measured through boiler auxiliary or fan electricity.
- The assertion passes (finding 4's probe), but it states a proxy.
- The probe also shows 2024.2 `bldg0000002` booking 3.45 kWh to
  `hvac_cooling` over 72 h from 2018-01-15 in Connecticut. That looks like
  a standing draw booked as cooling and is unexplained.

**Correction:**
- Name the keys `hvac_cooling` and `hvac_heating` in BR-12.
- For the winter cases, also assert that the run's gas energy is above
  zero, which is what the doc comment claims.
- Record the January cooling energy as a finding for triage, in
  `11-other-defects.md` or as a new entry.

### 10. Minor: BR-12 runs the ignored tests with every feature enabled at once

**Entry:** BR-12, ignored-test run.

**Evidence:** `--all-features` enables `dst`, `observe`,
`check_invariants`, `profiling` (with the zero-returning allocation stub)
and `actor_profiling` together. A failure could come from the feature
combination rather than from the code under test.

**Correction:** Run with default features, and add a second run under each
feature a test's own `cfg` names.

### 11. Minor: BR-12's target census leaves out the repository-level `tests/` files

**Entry:** BR-12, acceptance census.

**Evidence:** The census covers `crates/*/tests/*.rs` and
`tools/*/tests/*.rs` only. Repository-level `tests/*.rs` files are compiled
only through explicit `[[test]]` targets or `#[path]` shims. For example,
`tests/envelope_oracle.rs` is reached only through
`crates/hares-core/tests/envelope_oracle.rs`.

**Correction:** Extend the census: every `tests/*.rs` is the `src_path` of a
target or the `#[path]` of a compiled shim.

### 12. Minor: BR-13 leaves the directory comparison and `materialize` unspecified

**Entry:** BR-13, change.

**Evidence:**
- "compares it, with the defaults directory" does not say whether the
  `defaults_dir` string is compared or only reported.
- `materialize` (`capture.rs:110-145`) checks products, metrics and health,
  but not the defaults digest.

**Correction:** State that the directory string is reported, not compared
(the digest already covers content). Make `materialize` refuse a
defaults-digest mismatch as well, with the same message.

### 13. Minor: BR-14 leaves out a dependency, strict tariff parsing and the error tests

**Entry:** BR-14.

**Evidence:**
- The tool's `Cargo.toml` has no `chrono-tz`.
- `ElectricTariff` does not deny unknown fields, so a misspelled key in
  `flat_tariff.json` is silently dropped. That conflicts with the manifest's
  strictness rule.
- The acceptance lists no test for a bad file or a failed `validate()`.

**Correction:**
- Add `chrono-tz = { workspace = true }` to `tools/frame_golden`.
- Read the tariff file strictly: deserialize through `serde_ignored`, or
  compare the re-serialized key set, and fail naming the key.
- Add `manifest_tariff_rejects_invalid_file` and
  `manifest_tariff_rejects_failed_validation`.

### 14. Minor: BR-15's test fixture has no start time

**Entry:** BR-15, test `fleet_manifest_weights_reach_the_weights_product`.

**Evidence:** The test fixture gives no start time.

**Correction:** Use `2018-01-01T00:00:00-05:00`, the site offset of
`G0900090`.

### 15. Minor: CN-17's drift census overlaps CN-13's range

**Entry:** CN-17, change ("contract-drift census for engine rows 1 to 12"),
and the `08-consumer.md` preamble's assignment ("CN-13 for the engine rows
from BR-11 to row 26").

**Evidence:** CN-17 now gates on an engine at or after `21d90588`, so its
census covers BR-11, PR-1 and PR-2 as well, and those rows also fall in
CN-13's range.

**Correction:** CN-17's census covers every engine row up to the commit it
gates on, and CN-13's range starts after that commit.

### 16. Minor: two output-changing rows land before the consumer block without being named as exceptions

**Entry:** `00-schedule.md` rules: "Each numeric fix ... all of them follow
the consumer block".

**Evidence:**
- Row 31, OD-2 site 6, is a commit with a recorded delta.
- Row 51, FL-2, re-captures `fleet_mixed_offset`.
- Both change outputs before row 72. Their entries argue, correctly, that
  neither reaches B0 or `consumer_shape_900s`.

**Correction:** Name both in the rules as stated exceptions: output changes
that no consumer capture or consumer-shaped golden can see.

### 17. Minor: the profiler entries cite lines that BR-6a has since shifted

**Entry:** `07-profiler.md` PR-3 to PR-6 evidence.

**Evidence:** BR-6a's let-chain collapse moved `dwelling/mod.rs` lines by
up to about 190 before these rows run. At `8b09ac4a`:
- `actor_timing()` is at 4118, cited as 4089.
- The per-step clear is at 5626, cited as 5462.
- The `debug_assert_eq!` is at 6698-6705, cited as 6510-6517.
- The telemetry clone is at 6624, cited as 6436 (M-3's log notes this).

**Correction:** Re-cite these at `8b09ac4a` in the profiler file, as
`plan.md` §1 allows for entries that start after the opening sequence.

### 18. Minor: PR-3's allocation test relies on allocations the compiler is allowed to remove

**Entry:** PR-3, test
`thread_allocations_counts_exactly_this_threads_allocations`.

**Evidence:** `Box::new(black_box(n))` boxes are dropped at once, and the
documentation of `GlobalAlloc` allows the compiler to elide such
allocations. A scratch probe on rustc 1.94.1 counts 7 in both debug and
release, so it holds today but is not guaranteed.

**Correction:** Write `black_box(Box::new(n))`, so each box escapes.

### 19. Minor: PR-6's Python-side test does not say where its probe actors come from

**Entry:** PR-6, test `profiling_summary_lists_actors`.

**Evidence:** The probe actors are hares-core test items, which a
`hares-python` test cannot import.

**Correction:** State that the test defines its own two no-interest
`Actor` implementations in `hares-python`'s `#[cfg(test)]` module and
adds them through `PyDwelling`'s inner `Dwelling` (pyo3 `auto-initialize`
is already enabled).

## Verified correct

- BR-10 evidence:
  - `ci.yml:3-8` triggers `main` only, `:150-156` lists the nextest runs
    and `:271`, `:281-283` hold the hard-coded `main`.
  - `origin/main` holds only the initial commit, and the bench job failed
    on PR #27 with "could not find `Cargo.toml`".
  - PR #27's base change to `initial-implementation` came after its last
    run.
  - The added steps are valid nextest and cargo invocations, the golden
    fixtures are tracked plain files (no LFS), and `compare-all` writes
    only under `target/`.
- BR-12 evidence: `cargo metadata` at `8b09ac4a` lists exactly the seven
  named files as uncompiled. The six shims are two-line `#[path]` modules.
  The seasonal tests and their inputs are at the cited lines. The census
  command, run at `8b09ac4a`, prints exactly those seven paths, and the
  `src_path` against `realpath` comparison works. Debug runtime is small.
- BR-13 evidence:
  - `GoldenDoc.defaults_digest` is written at `capture.rs:44` and read
    nowhere else.
  - `digest_tree` hashes every file of the tree in sorted order.
  - `compare_products` takes no digest.
  - A temporary-manifest test is executable: the golden is written next to
    the manifest and replacement paths may be absolute.
- BR-14 evidence:
  - The adapter comment says no manifest reaches `set_tariff`.
  - `set_tariff(ElectricTariff, Tz)` is the only entry point.
  - `ElectricTariff` derives `Deserialize` and has `validate()`.
  - `simulate()` calls `finalize_billing()`, so the January and February
    rows are produced.
  - Jan 25 to Feb 6 in MST is fixed-offset `-07:00`.
  - It is ordered before B-4 (row 85) and OR-10 (row 48).
- BR-15 evidence:
  - `Fleet::from_buildings` assigns weight 1.0.
  - `with_sample_weights(Vec<f64>) -> Result` rejects NaN, infinite and
    negative weights.
  - The resolution rule is at `adapter.rs:315-321`.
  - `fleet_mixed_offset` at 3600 s maps to `Hourly`, so its golden is
    unchanged.
  - Duplicate building ids are not rejected, so the two-`bldg0000002` test
    is constructible.
- CN-17:
  - `cmd_compare` diffs only `premises`, and a test pinning that is
    missing.
  - The header fields the test varies are exactly `build_header`'s.
  - `calibration.schema --emit --all-cached` exists, and the skip in
    `suite_coverage.py` is as cited.
  - The commit message passes `[writing]`.
- `07-profiler.md` at `8b09ac4a`:
  - `DwellingProfilingSummary`'s fields are as stated, and `water_heater`
    has no write site.
  - There are four `current_process_hwm_kb()` reads per step, and
    `hot_path_alloc_counter()` returns 0 under a panicking
    `debug_assert_eq!`.
  - Three process-global counting allocators exist (the envelope tests and
    `benches/rl_episode.rs`), and only `bench_episode_vec` counts across
    `par_iter_mut`.
  - The non-`profiling` binding returns zeros with a `note`, and
    `_hares.pyi` has no stub.
  - `actor_profiling` is at `Cargo.toml:10` with per-step clear and push,
    and the `docs/development.md` text is as cited.
  - The `git grep` exclusion matches exactly two `docs/reviews` files.
  - Empty interests mean `decide()` is called every step.
  - `add_actor` calls `refresh_equipment_caches`.
  - `hares-python` has `pyo3` `auto-initialize`.
  - PR-1's phase order matches `run_timestep`'s step comments, and
    `Duration` sums of differences of the same `Instant` readings make the
    partition test exact.
  - Dependencies are in order: PR-4 after OR-6, PR-5 before PR-6, and PR-3
    before RT-3 and RT-1.
- `00-schedule.md` coverage: every entry heading in ledger files 01 to 12
  appears exactly once. BR-5, SL-1, SL-4, SL-5, SL-6 and SL-9 are closed
  inside named rows, SL-2 and SL-3 are standing rules, and M-4a and M-4b
  are separate rows.
- No stated dependency is ordered after its dependent. A scripted check of
  every Dependencies paragraph finds only "lands before" mentions.
- Consumer ordering matches `dl-i08-architecture.consumer`:
  - CN-13 directly after PY-7.
  - The mechanical migration (CN-1, CN-5, CN-9, CN-7, CN-6, CN-10, CN-11)
    at rows 72 to 78, before B-1a at row 79.
  - CN-14 after the weather fixes, then CN-3, CN-2, the one re-baseline
    CN-4, CN-12.
  - CN-15, the one pin advance, after the operator push.
  - Every 09 numeric fix, FL-5, OD-9, IN-8 and IN-9 in its own row after
    row 78a.
- `plan.md` §3 block commands are valid for the installed tools (each line
  has run green since BR-6b). The `[writing]` regex accepts every commit
  message of the entries in scope.
