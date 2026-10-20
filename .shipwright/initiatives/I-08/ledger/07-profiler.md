# Profiler (HARES#31)

The `profiling` feature is how the engine-performance entries
(`06-runtime-memory.md`) decide what to fix and how allocation counts are
asserted. HARES#31 reports that it distorts what it measures and leaves most
of the step unattributed. These entries make it accurate, cheap, and honest
about what it cannot measure, and deliver HARES#31 in full, including the
per-actor timings through the Python bindings
(`dl-i08-architecture.profiler_scope`). Every entry's Rust tests run under
the `[test-profiling]` block line (`plan.md` §3).

PR-1 and PR-2 cite their evidence at `ebdb1a17` plus PR #27 (Task 0 head):
PR #27 adds `crates/hares-core/examples/profile_sim.rs`, the `profiling`
feature of `crates/hares-python/Cargo.toml`, and
`PyDwelling::profiling_summary()` in `crates/hares-python/src/py_dwelling.rs`
(after `from_hpxml`, before `simulate`). PR-3 to PR-6 cite theirs at
`21d90588` (PR-2's commit), after `01-baseline-repair.md` BR-6a's let-chain
refactor and PR-1 moved the `dwelling/mod.rs` lines.

Committed on `feat/output-results`: PR-1 `d7f0e381`, PR-2 `21d90588`,
PR-3 `26626710`, PR-5 `1d471677` and PR-6 `afa9a071`, in that order
(`00-schedule.md` rows 14 to 18). None has a recorded B0 check; all five
are checked in the B0 re-check step (row 16g), each on a worktree of its
commit. That step runs after `03-output-reporting.md` OR-11 (row 19)
commits and `08-consumer.md` CN-17 (row 16a) lands gated on OR-11's
commit. PR-4 runs at row 44, after `03-output-reporting.md` OR-6 and
`05-python-surface.md` PY-9, once a verification round over rows 20
onward records zero blockers and zero majors.

## PR-1: phase timers overlap, leave the step unattributed, and one is never written

**Kind:** fix.

**Problem:** `DwellingProfilingSummary` (`crates/hares-core/src/dwelling/mod.rs:242-253`)
does not partition `run_timestep`:

- `water_heater` has no write site; it always reads zero.
- `hvac` (`mod.rs:5702-5703` start, `5957-5966` stop) wraps the whole unified
  equipment step (Step 3), water heaters included, so a water-heater timer
  inside it would count the same time twice.
- `schedule_load` (`mod.rs:5316-5317` start, `5619-5628` stop) spans the
  environment update, internal gains, `update_control`, the ideal-capacity
  solve and actor dispatch, so its name describes a fraction of it.
- The span from the end of the envelope solve (`mod.rs:6163`) to `record_step`
  (`:6483`), which holds the invariant checks and the per-step telemetry
  snapshot (`:6389-6440`), is timed by nothing, and billing plus the
  electrical summary after `:6520` falls outside `step_started` accounting
  altogether. `other` (`mod.rs:6499-6508`) is a residual that absorbs all of
  it and every instrument cost.
- `crates/hares-core/examples/profile_sim.rs` duplicates the timing driver
  that `02-measurement.md` M-1's `frame_golden run -F profiling` provides, and
  runs `bldg0000007` (Honolulu, UTC-10, county G1500030) against
  `G0100590_2018.csv` with a `-07:00` start.

**Change:**

- Replace the timers with one phase clock: `Instant::now()` is read once at
  entry to `run_timestep` and once at each boundary below, and each reading
  closes the previous span and adds it to that span's phase, so the phases
  partition the step exactly and there is no residual. A phase may be
  closed more than once per step (its spans are summed); the code order of
  the spans is: `environment`, `control`, the scheduler loop (`ideal_capacity`
  and `actors` interleaved in plan order), `dispatch`, `control` again
  (Step 2a), `equipment`, `envelope`, `invariants`, `state_snapshot`,
  `output`, `accounting`. Fields of `DwellingProfilingSummary`
  (`StdDuration` each), with their spans in `run_timestep`'s own step
  comments:
  - `environment`: entry through Step 1a' (environment update, queued control
    dispatch);
  - `control`: Steps 1b-1d (internal gains, `update_control`, input build)
    and, as a second span, Step 2a (`update_control` re-run after dispatch);
  - `ideal_capacity`: each `ExecutionPhase::SolverFeedback` entry of the
    scheduler loop, Steps 1e-1f (`collect_and_solve`, the solver-feedback
    `decide`, queueing its signals);
  - `actors`: each `ExecutionPhase::ActorDecide` entry of the scheduler
    loop: the interest filter, `decide()` when the filter calls it, and the
    health check after it;
  - `dispatch`: Steps 2 and 2b (actor signals and derived equipment
    signals dispatched);
  - `equipment`: Steps 3 and 3c;
  - `envelope`: Step 4 and its diagnostic capture;
  - `invariants`: the always-on checks and `check_invariants`;
  - `state_snapshot`: observer push, diagnostic accumulation, the
    end-of-step equipment snapshot into `latest_env`, the step result;
  - `output`: Step 5 (`record_step`);
  - `accounting`: billing, the electrical summary and clock advance, to return;
  - `step_total`: from entry to return.
- Delete `water_heater`, `hvac`, `schedule_load`, `io`, `envelope_solve` and
  `other`. Update `emit_dwelling_profiling_summary`
  (`crates/hares-core/src/engine.rs:365-391`) and the PR #27 Python binding
  keys (`environment_s`, `control_s`, ... `step_total_s`).
- Delete `crates/hares-core/examples/profile_sim.rs`; `frame_golden run`
  built with `-F profiling` prints the summary for any manifest.

**Acceptance test:** `profiling_phases_partition_the_step` in
`crates/hares-core/tests/profiling.rs` (`#![cfg(feature = "profiling")]`):
after 96 hourly steps of the
`tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh/` inputs (`building.xml`,
`schedule.csv`, `weather.epw`; start `2023-01-01T00:00:00-07:00`, no
warm-up, `write_output: false`), the sum of the eleven phase fields
(`environment`, `control`, `ideal_capacity`, `actors`, `dispatch`,
`equipment`, `envelope`, `invariants`, `state_snapshot`, `output`,
`accounting`) equals `step_total` exactly (`Duration` equality: the phases
are differences of the same `Instant` readings, so the check does not
depend on timing). Every fixture-driven test in this file uses these inputs
and settings and is named "the cz2a fixture" below.

**Verification:** `[test-profiling]`; `[clippy]` (the Python binding compiles
under `--all-features`).

**Commit:** `fix: partition the profiled step into phases that sum to the step time`

**Dependencies:** Task 0 (PR #27), M-3 (the baseline is taken before the
profiler changes), M-1 (`frame_golden run` replaces the example).

**Measurement:** the M-3 perf shares and the profiled phase shares for
`consumer_shape_900s`, side by side, in the `02-measurement.md` log.

## PR-2: the memory high-water mark is read from `/proc` four times per step

**Kind:** fix (also removes most of the profiling build's own overhead).

**Problem:** `current_process_hwm_kb()`
(`crates/hares-core/src/dwelling/synthetic.rs:2110-2121`) reads and parses
`/proc/self/status` on every call, and `run_timestep` calls it four times per
step (`mod.rs:5624-5627`, `5962-5965`, `6160-6163`, `6493-6496`). HARES#31
measured the profiling build at about 3x the release wall time with the
reads accounting for most of it. The peak resident size is monotone, so the
per-step reads add nothing a single read at the end does not give. On any
platform without `/proc/self/status`, the function returns 0, which reads as
a measurement.

**Change:** remove the four per-step reads. `DwellingProfilingSummary`
carries `memory_high_water_kb: Option<u64>`, filled once when
`profiling_summary()` is called: `Some(VmHWM)` on Linux, `None` elsewhere
(`#[cfg(target_os = "linux")]`), and a read or parse failure on Linux is
logged with `tracing::warn!` naming the error and yields `None`, never 0.
`current_process_hwm_kb` becomes that single reader. The Python binding
returns `None` as Python `None`.

**Acceptance test:** `profiling_summary_reports_process_high_water_mark`
in `crates/hares-core/tests/profiling.rs` (`#[cfg(target_os = "linux")]`):
after 96 steps of the cz2a fixture (PR-1), the summary's
`memory_high_water_kb` is `Some(v)` with `v > 0` and `v` at most a `VmHWM`
read taken immediately afterwards.

**Verification:** `[test-profiling]`.

**Commit:** `perf: read the memory high-water mark once per summary, not four times per step`

**Dependencies:** PR-1.

**Measurement:** profiled `step_total` for `consumer_shape_900s` against the
unprofiled release wall time from M-3, before and after, in the log.

## PR-3: the hot-path allocation counter is a stub, and a real one would panic

**Kind:** fix.

**Problem:** `hot_path_alloc_counter()` (`synthetic.rs:2153`) returns 0,
so `hot_path_alloc_violations` can never be non-zero and the allocation-free
claims of `06-runtime-memory.md` RT-1 and RT-3 cannot be tested. Behind it,
`run_timestep` does `debug_assert_eq!(alloc_after, alloc_before, ...)`
(`mod.rs:6786-6795`): with a real counter, every debug `-F profiling` step
that allocates would panic, and steps allocate today (the telemetry map
`clone_from`/`insert` at `mod.rs:5079` and `:6722`, recorder buffer growth).
The workspace also holds three copies of a process-global counting
allocator: `crates/hares-envelope/tests/synthetic_box_zero_alloc.rs:12-26`,
`crates/hares-envelope/tests/radiant_gain_weights_zero_alloc.rs:34-48` and
`benches/rl_episode.rs:19-38`. A process-global count includes other
threads' allocations.

**Change:**

- Add `crates/hares-types/src/alloc_count.rs`, one counting allocator for the
  workspace: `pub struct CountingAllocator;` implementing `GlobalAlloc` over
  `std::alloc::System`, incrementing a `const`-initialised
  `thread_local!` `Cell<u64>` (accessed with `try_with`, so a thread in
  teardown is never a panic) on `alloc`, `alloc_zeroed` and `realloc`, and
  setting a process-wide `AtomicBool` the first time it runs; and
  `pub fn thread_allocations() -> Option<u64>` returning `None` when that
  flag is unset, meaning no binary installed the allocator. A library only
  defines the type; each binary that wants counts installs it with
  `#[global_allocator]`.
- `hot_path_alloc_counter()` becomes `hares_types::alloc_count::thread_allocations()`.
  `DwellingProfilingSummary` carries `hot_path_allocations: Option<u64>` (sum
  of per-step deltas) and `hot_path_alloc_violations: Option<u64>` (steps with
  a non-zero delta); both are `None` when the allocator is not installed.
  Delete the `debug_assert_eq!`: violations are counted and reported, never a
  panic.
- The three existing copies install `hares_types::alloc_count::CountingAllocator`
  and read `thread_allocations()` instead of their own statics. A
  per-thread count does not see allocations on rayon worker threads, so
  `benches/rl_episode.rs`'s vectorised group (`dwellings.par_iter_mut()`,
  `:280-306`) takes its allocations-per-episode figure from a separate
  counting pass that steps the same dwellings sequentially with
  `iter_mut()` on the bench thread, outside the timed loop; the timed loop
  stays parallel and uncounted, and the printed label says the count is
  from the sequential pass.
- `frame_golden` installs it only when built with `-F profiling`
  (`#[cfg(feature = "profiling")] #[global_allocator]` in
  `tools/frame_golden/src/lib.rs`, which the binary and the tool's test
  binaries link; nothing depends on the tool), so `run
  -F profiling` reports real counts while the plain release binary whose
  wall times M-3 recorded keeps the system allocator unchanged and the
  before and after timings come from the same binary. The Python extension
  does not install it, so its summary reports `None`; the binding returns
  Python `None`.

**Acceptance test:**

- `crates/hares-types/tests/alloc_count.rs` (installs the allocator):
  `thread_allocations_counts_exactly_this_threads_allocations`: between two
  reads on one thread, 7 `black_box(Box::new(black_box(n)))` allocations
  give a delta of exactly 7 (the outer `black_box` makes each box escape, so
  the compiler may not elide the allocation, which `GlobalAlloc` permits for
  a box dropped at once). A spawned thread and the main thread meet at a
  `std::sync::Barrier`; the spawned thread then makes 5 allocations between
  its own two reads while the main thread makes 1 000, and the spawned
  thread's delta is exactly 5.
- `crates/hares-types/tests/alloc_count_absent.rs` (does not install it):
  `thread_allocations_is_none_without_the_allocator`.
- `crates/hares-core/tests/profiling_alloc.rs`
  (`#![cfg(feature = "profiling")]`, installs the allocator):
  `profiled_steps_count_allocations_without_panicking`: 96 debug-build steps
  of the cz2a fixture (PR-1) complete, and both counters are `Some`, with
  `hot_path_allocations` greater than or equal to
  `hot_path_alloc_violations`.
- `frame_golden_installs_the_allocator_only_under_profiling`
  (`tools/frame_golden/tests/`): `thread_allocations()` is `Some` in a
  `-F profiling` build of the tool's test binary and `None` otherwise
  (`#[cfg]` on each branch).

**Verification:** `[test-profiling]`; the envelope and bench copies keep
passing under `[test-debug]` and `[bench]`.

**Commit:** `fix: count hot-path allocations per thread with one shared counting allocator`

**Dependencies:** PR-1. Blocks the allocation assertions of
`06-runtime-memory.md` RT-1 and RT-3, which install the same allocator in
their own test binaries and assert `Some(0)`.

## PR-4: output finalisation is not timed

**Kind:** fix.

**Problem:** the profiled summary has no phase for flushing and closing
output at the end of a run, so its cost (about 9 ms per full-year run at
verbosity 2, `context.md` item 8) appears nowhere.

**Evidence** (at `7b7a6a7d`): `Dwelling::simulate` calls
`flush_and_close()` at `mod.rs:3146` with no timer; after `03-output-reporting.md` OR-6 the call
moves into `Dwelling::finish()`.

**Change:** add `finish: Option<StdDuration>` to `DwellingProfilingSummary`,
timed around the first, finalising call of `finish()` (flush, close,
rename, billing finalisation), outside `step_total`. Later calls return
the same `Arc<RunResult>` without finalising again (`plan.md` §2.1) and
leave `finish` as the first call measured it. It is `None` until
`finish()` has run, so a summary taken mid-run never shows a zero that
reads as a measurement. Add `finish_s` (`None` until finished) to the
Python binding, to its `_hares.pyi` summary type and to
`emit_dwelling_profiling_summary`. `frame_golden`'s adapter reads the
profiling summary after it calls `finish()` (today it reads it after the
run, `tools/frame_golden/src/adapter.rs:254` at `afa9a071`), and the
tool's per-repetition profiling line (`tools/frame_golden/src/cli.rs:695`
at `afa9a071`) prints `finish`, so the Measurement below comes from
`frame_golden run -F profiling`.

**Acceptance test:**

- `profiling_finish_timer_covers_file_finalisation` in
  `crates/hares-core/tests/profiling.rs`: 96 steps of the cz2a fixture
  (PR-1) with a Parquet destination in a `tempfile` directory; before
  `finish()` the summary's `finish` is `None`; after it, `finish` is `Some`
  and the Parquet file exists at its final name; after a second
  `finish()`, the summary's `finish` equals the value after the first. The
  duration's value is not asserted (no timing assertion, `plan.md` §3); it
  is recorded in the measurement below.
- `profiling_summary_reports_finish` (a `hares-python` Rust test, compiled
  under `[test-profiling]`, built as PR-6's `profiling_summary_lists_actors`
  is): a `PyDwelling` from the cz2a fixture stepped 96 times returns
  `finish_s` as Python `None` from `profiling_summary()`, and a float after
  `finish()`.

**Verification:** `[test-profiling]`; `[clippy]` (the binding compiles
under `--all-features`); `[pytest]` (the `_hares.pyi` stub).

**Commit:** `feat: time run finalisation in the profiling summary`

**Dependencies:** PR-1, `03-output-reporting.md` OR-6,
`05-python-surface.md` PY-9 (the Python `finish()`).

**Measurement:** `finish` for `consumer_shape_900s` with a Parquet
destination, in the `02-measurement.md` log.

**State:** committed `0f1e459b` on branch `row/r42-or12-pr4` (run after
row 42, before the finalize entry this row's dependence list names: the
`finish()` row and the Python `RunResult` row have not run, so the
entry's shape is adapted to today's surface and the account is recorded
here). The finalising call today is the end of `Dwelling::simulate()`:
the billing finalisation, the observe post-hoc diagnostic checks and
every recorder's flush, close and publish, now extracted as one
idempotent `finalise_outputs()` (the billing finalisation is documented
idempotent, the checks read accumulated observer data, and a closed
recorder re-flushes and re-publishes nothing, proven by the second-call
assertions). The timer wraps that section whole:
`DwellingProfilingSummary.finish` is `Option<StdDuration>`, `None` until
the first finalising call has run (a summary taken mid-run never shows a
zero that reads as a measurement), `Some` after it, outside
`step_total`, and a later finalising call leaves the first call's
measurement. The Python summary's `finish_s` (None until finished, a
float after), the `ProfilingSummary` TypedDict's field, the engine's
emitted stats line and the golden tool's per-repetition profiling line
(after `step_total`: `finish 0.003 s`, or `unavailable` when None) carry
it. The tool's adapter reads the profiling summary after the run
returns, which is already after the finalising call, so the entry's
adapter move is vacuous here and the printed line is the measurement
source. The acceptance tests drive the finalising call through
`finish()`, which does not exist yet; here they drive it through
`simulate()`: `profiling_finish_timer_covers_file_finalisation` in
`crates/hares-core/tests/profiling.rs` (the cz2a fixture, 96 hourly
steps, one Parquet destination in a `tempfile` directory: `finish` is
None before, Some with the Parquet file at its final name after, and
equality across a second finalising call; the duration's value not
asserted) and `profiling_summary_reports_finish` (a `hares-python` Rust
test compiled under `[test-profiling]`, built as PR-6's
`profiling_summary_lists_actors` is): `finish_s` is Python None from
`profiling_summary()` after the 96 steps and a float after the
finalising call, which runs on the inner `Dwelling` so the test
interpreter need not carry polars. RED first: both tests failed to
compile on the missing field, then the None/Some assertions failed
before the timer existed, then green. Gates: fmt clean; clippy
`--workspace --all-targets --all-features -D warnings` clean; nextest
`--all-features -j 4` 7896 passed, 1 skipped (the parent's ignored
kind-pin generator), `-F dst` 7826 passed, `-F profiling` 7821 passed;
the eight golden fixtures bitwise identical in both feature sets
(verified no golden carries the profiling summary: the fixtures' JSON
holds metrics, frame digests and health only, so the new field moves
nothing); pytest unchanged (1648 passed with the base's 3 failures and
4 setup errors, base-verified on a worktree of the base); pyright 0
errors.

## PR-5: the Python binding reports zeros when profiling is compiled out

**Kind:** fix.

**Problem:** without the `profiling` feature, the PR #27 binding returns a
dict of zero timings with a `note` key, so a caller that does not read the
note receives numbers that look like a measurement.

**Evidence:** `PyDwelling::profiling_summary`, the
`#[cfg(not(feature = "profiling"))]` branch
(`crates/hares-python/src/py_dwelling.rs:722-742`, the block that sets every
`*_s` key to `0.0`).

**Change:** the non-`profiling` build raises `NotImplementedError` naming the
`profiling` feature and the build command
(`maturin develop --release --features profiling`); the `note` key goes from
both branches. `python/ochre_next/_hares.pyi` has no `profiling_summary`
stub at the Task 0 head; this entry adds it: a `ProfilingSummary`
`TypedDict` with one key per summary field as PR-1 to PR-3 define them
(`environment_s` to `step_total_s` as `float`, `memory_high_water_kb`,
`hot_path_allocations` and `hot_path_alloc_violations` as `int | None`),
`def profiling_summary(self) -> ProfilingSummary`, and a docstring stating
the `NotImplementedError` and the build command. PR-4 and PR-6 extend the
`TypedDict` with their keys.

**Acceptance test:** `tests/python/test_profiling_summary.py::test_profiling_summary_raises_without_the_feature`,
run by `[pytest]` against the default (no-feature) build, on a dwelling
built from the cz2a fixture (PR-1).

**Verification:** `[pytest]`.

**Commit:** `fix: raise instead of returning zero timings when profiling is not compiled in`

**Dependencies:** PR-1.

## PR-6: per-actor timings keep only the last step and cannot reach Python

**Kind:** fix (`dl-i08-architecture.profiler_scope`: HARES#31 in full,
including the per-actor profiling Python binding).

**Problem:** the `actor_profiling` feature records how long each actor's
`decide()` takes, but:

- `per_actor_timing` is cleared at the start of every step, so
  `Dwelling::actor_timing()` returns the last step's timings, not the
  run's;
- `actor_timing()` builds a new `Vec<(String, StdDuration)>` and clones
  every actor name on each call;
- the feature exists only on `hares-core`, so no Python build can enable
  it, and nothing in the bindings exposes it (HARES#31 asks for both);
- `docs/development.md` describes it as "Per-equipment step timing" and
  says the Python extension has only the `observe` feature, both untrue
  once PR #27 has landed.

**Evidence:** `crates/hares-core/Cargo.toml:10` (`actor_profiling = []`);
`crates/hares-core/src/dwelling/mod.rs:1828-1831` (fields), `:5726-5730`
(clear and reserve per step), `:5795-5796` and `:5811-5812` (the timing
around `decide()`), `:4215-4231` (`actor_timing()`); `docs/development.md:239-261`.

**Change:**

- The `actor_profiling` feature is folded into `profiling` (one feature,
  one block line): `DwellingProfilingSummary` gains
  `per_actor: Vec<ActorTiming>` (not `actors`, which is PR-1's phase
  field), `ActorTiming { name: Arc<str>, total:
  StdDuration, calls: u64 }` in `hares-core`, one entry per registered
  actor in registration order. The run's store is a small `pub` type in
  `hares-core`'s profiling module, `ActorTimings` (opaque, `plan.md` §2.9),
  holding one `ActorTiming` per actor slot. Its public surface is
  `pub fn new(names: impl IntoIterator<Item = Arc<str>>) -> Self` (one slot
  per name, in iteration order, totals zero) and
  `pub fn record(&mut self, slot: usize, elapsed: StdDuration, called: bool)`,
  which adds to that slot in place, so accumulating allocates nothing, and
  `pub fn timings(&self) -> &[ActorTiming]`, which the summary reads. The
  dwelling builds it with `new` when actors are registered (in
  `refresh_equipment_caches`), from the registered actors' names. `total` is the
  actor's scheduler entry span as PR-1's phase clock measures it (the
  interest filter, `decide()` and the health check; no extra `Instant`
  read), and `calls` counts the `decide()` calls the filter made, so the
  actors' totals sum exactly to PR-1's `actors` phase. `actor_timing()` and
  the two per-step fields are deleted; the summary is the one accessor.
- The Python `profiling_summary()` gains a `per_actor` key: a list of
  `{"name", "total_s", "calls"}` in registration order (PR-1's phase key
  is `actors_s`); the non-`profiling` build raises as PR-5 states.
  `_hares.pyi`'s `ProfilingSummary` gains the `per_actor` key with an
  `ActorTimingSummary` `TypedDict`.
- `emit_dwelling_profiling_summary` (`engine.rs:365-391`) logs the per-actor
  totals.
- `docs/development.md` describes `profiling` as the one timing feature
  (phases, finalisation, per-actor totals) and lists the features the
  Python extension offers.

**Acceptance test:**

- `actor_timings_accumulate_over_the_run` (`crates/hares-core/tests/profiling.rs`):
  the cz2a fixture (PR-1) with two test actors added after construction,
  `"probe_a"` then `"probe_b"`, each declaring no interests (so the filter
  calls `decide()` every step) and dispatching nothing, stepped 96 times.
  The assertions name the two entries rather than count the list, because
  `auto_register_actors` may register built-in actors first: the summary's
  `per_actor` holds `"probe_a"` before `"probe_b"`, each with `calls == 96`,
  and the sum of every entry's `total` equals the `actors` phase exactly
  (`Duration` equality, as in PR-1).
- `actor_timings_record_without_allocating`
  (`crates/hares-core/tests/profiling_alloc.rs`, PR-3's allocator):
  `ActorTimings::new(["probe_a", "probe_b"].map(Arc::<str>::from))`, then
  96 x 2 `record` calls; the `thread_allocations()` delta across the calls
  (read after construction) is `Some(0)`. Whole-step
  `Some(0)` is not asserted here: steps still allocate at this row
  (the telemetry map), and `06-runtime-memory.md` RT-1 and RT-3 own that
  assertion.
- `git grep -n 'actor_profiling' -- '*.rs' '*.toml' '*.md' '*.py' '*.pyi' ':(exclude)docs/reviews'`
  returns nothing. The two `docs/reviews/` planning records that mention the
  feature are not edited (they are review history, and
  `10-slop-cleanup.md` SL-8 moves them out of the tree at a later row).
- `profiling_summary_lists_actors` (a `hares-python` Rust test, compiled
  under `[test-profiling]`, which enables the crate's `profiling` feature):
  the test's own `#[cfg(test)]` module in `hares-python` defines two
  `Actor` implementations named `"probe_a"` and `"probe_b"` with no
  interests that dispatch nothing (the `hares-core` probes are test items
  another crate cannot import), builds a `PyDwelling` from the cz2a
  fixture, adds them through its inner `Dwelling` in that order, steps it
  96 times, and calls `profiling_summary()` under the interpreter that
  `pyo3`'s `auto-initialize` feature (already enabled for the crate's
  tests) provides: the returned dict has a `per_actor` list holding
  `"probe_a"` before `"probe_b"`, each with `calls == 96`.

**Verification:** `[test-profiling]`; `[clippy]`; `[pytest]`.

**Commit:** `fix: accumulate per-actor timings over the run and report them in the profiling summary`

**Dependencies:** PR-1, PR-3, PR-5.
