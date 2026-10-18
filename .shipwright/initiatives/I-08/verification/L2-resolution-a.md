# L2 resolution a: plan.md §1, §3, §5; 07-profiler; committed opening rows

Inputs: `verify-L2-a.md` (all 34 findings) and the `verify-L2-e.md` findings
that concern `plan.md` §1, §3, §5, `ledger/07-profiler.md` or committed
opening rows. Binding: `decision-log.md`, including the `dl-i08-execution`
entries. Code read at `feat/output-results` HEAD `872cf289`; the committed
rows are `1d60f478` (Task 0) through `872cf289` (BR-6b).

Files changed by this pass: `plan.md` (§1, §3 only), `ledger/07-profiler.md`,
`ledger/01-baseline-repair.md` (new entries BR-10 to BR-15 appended after
BR-6d; nothing above them edited). Not touched: `implementation.md`,
`decision-log.md`, `ledger/00-schedule.md`, `ledger/02-measurement.md`, any
code.

Counts:

- `verify-L2-a.md`, 34 findings: 11 fixed in the plan or `07`; 7 still
  valid at HEAD and now covered by new entries (BR-10 to BR-15); 7 already
  fixed in committed code; 9 belong to other owners only; 0 rejected.
- `verify-L2-e.md`, 10 findings in this scope: 7 fixed; 1 still valid and
  covered by a new entry (BR-12); 2 belong to other owners.
- New ledger entries: 6 (BR-10, BR-11, BR-12, BR-13, BR-14, BR-15).
- Schedule changes: 7 rows (1 executed row recorded, 6 new), plus 4
  preamble or cell corrections.

## verify-L2-a.md

### Blockers

**1. BR-2 misses PR #27's two `ptr_arg` lints in `profile_sim.rs`.**
Already fixed at `fd23dd80`. `crates/hares-core/examples/profile_sim.rs`
now reads `fn config(days: i64, fixture: &Path, weather: &Path) -> DwellingConfig`,
gated `#[cfg(feature = "profiling")]` with its imports; the commit message
says "its PathBuf arguments are &Path". `[clippy]` is green at HEAD
(`implementation.md` block state). PR-1 still deletes the example.

**2. PR-6's acceptance grep cannot pass at row 18.** Fixed:
`ledger/07-profiler.md` PR-6: "`git grep -n 'actor_profiling' -- '*.rs'
'*.toml' '*.md' '*.py' '*.pyi' ':(exclude)docs/reviews'` returns nothing.
The two `docs/reviews/` planning records that mention the feature are not
edited (they are review history, and `10-slop-cleanup.md` SL-8 moves them
out of the tree at a later row)." Checked at HEAD: the only other matches
are `crates/hares-core/Cargo.toml:10`, `dwelling/mod.rs` and
`docs/development.md`, all of which PR-6 changes.

### Majors

**3. BR-7 leaves the `dwelling_*_diagnostics.csv` strays.** Already fixed at
`81cc6d85` for the strays: every test at verbosity 4 and above now passes a
temporary `output_path` (for example
`crates/hares-core/tests/ev_thermal_regressions.rs:469`,
`output_path: Some(tmp.path().join("ev_regr_diag.csv"))`), the commit
message states the diagnostics rule, and `[stray]` is empty at HEAD. The
engine behaviour itself still holds at HEAD
(`crates/hares-core/src/dwelling/mod.rs:2818-2824`: the writer is created
whenever `output_verbosity >= 4` and its path derives from `output_path`);
`dl-i08-execution.progress` assigns it to OR-3 and OR-4. Item for the
`03-output-reporting.md` owner below.

**4. BR-4's CI never runs on this initiative's branches.** Still valid at
HEAD (`.github/workflows/ci.yml:3-8` lists only `main`; `main` holds only
`.gitignore`, `LICENSE`, `README.md`) -> new entry BR-10. BR-10 also fixes
the `bench` job, which hard-codes `main` (`ci.yml:271, 281-283`) and would
fail on the first pull request into `initial-implementation`. No
feature-branch name is added to the triggers (consistent with
`verify-L2-e.md` 41).

**5. PR-1 puts actor `decide()` time in `control`; PR-6 says
`actor_dispatch`.** Fixed: `ledger/07-profiler.md` PR-1 now defines the
phases in code order, with summed spans: "`control`: Steps 1b-1d ... and,
as a second span, Step 2a"; "`ideal_capacity`: each
`ExecutionPhase::SolverFeedback` entry of the scheduler loop, Steps 1e-1f";
"`actors`: each `ExecutionPhase::ActorDecide` entry of the scheduler loop:
the interest filter, `decide()` when the filter calls it, and the health
check after it"; "`dispatch`: Steps 2 and 2b". The acceptance test sums
eleven named phases. PR-6: "`total` is the actor's scheduler entry span as
PR-1's phase clock measures it ... so the actors' totals sum exactly to
PR-1's `actors` phase." The solver feedback gets its own phase rather than
joining `actors` as the finding proposed, because the ideal-capacity solve
is a thermal solve, not actor cost, and folding it in would hide the cost
HARES#31 asks about. `plan.md` §2.9's field list is outside this scope:
item for the §2 owner below.

**6. `initialization_duration_s = 0` runs a warm-up.** Still valid at HEAD:
`tools/frame_golden/src/adapter.rs:174` and `:274-276` build
`Some(StdDuration::from_secs(home.initialization_duration_s))`;
`dwelling/mod.rs:2753` warms up for any `Some`;
`consumer_shape_900s.golden.json` records `"warmup": {"Ran": {"converged":
true, "days_run": 2, ...}}`, so the fixture does not mirror the consumer
-> new entry BR-11 (0 means no warm-up; the other manifests move to 604800
so their goldens are unchanged and B-2 keeps fixtures with a warm-up;
`consumer_shape_900s` re-captured with its delta recorded). Scheduled
before M-3 because M-3 measures that fixture.

**7. Fleet manifest weight and resolution, fleet products.** Partly
already fixed at `979b8773`: the adapter defines the HEAD products
(`aggregate` = `aggregate_timeseries`; `weights`, one row per home, as the
covered-weight stand-in, `adapter.rs:349-370`; `homes` with `bldg_id`,
energy, peak, weight, status, failed, `adapter.rs:372-430`). Still valid:
the weight comes from `Fleet::from_buildings` and the resolution from a
rule on the step (`adapter.rs:282`, `:304-310`), neither stated in the
manifest -> new entry BR-15. The M-1 text still says "covered weights":
item for the `02` owner.

**8. `fleet_mixed_offset` pins an empty aggregate.** The fact is recorded in
the code commit (`7643b4e7`: "the fleet aggregate row count is pinned at
0") and in `implementation.md`. The ledger text is not: other owners only
(no code change needed in a committed row). Items for the `02`, `04` and
schedule owners below. The optional second fleet fixture with identical
column sets across offsets goes to the `04` owner with FL-4's acceptance,
which is where alignment is first exercised.

**9. No golden carries billing.** Still valid at HEAD
(`adapter.rs:216-223`: "no manifest reaches today"; `manifest.rs:53-67`
has no tariff key) -> new entry BR-14 (`[tariff]` table with a JSON
`ElectricTariff` file and an IANA zone; fixture
`month_boundary_billing_3600s`, cz2a inputs, 2023-01-25, 12 d at 3600 s).
The time-of-use fixture `verify-L2-e.md` 6 asks for belongs to OR-10:
item for the `03` owner.

**10. The defaults digest "in load order" cannot be computed.** Already
fixed at `979b8773`: `tools/frame_golden/src/defaults.rs:71-82` digests
every file of the tree the run used, sorted by relative path, `0x00`,
bytes, following the symlinked replacement tree; `implementation.md`
records the choice. The M-1 text still says "every defaults file the run
read ... in load order": item for the `02` owner. Whether `compare` fails
on a mismatch is minor 7 (BR-13).

**11. BR-8 edited a never-compiled file; seven shadowed files; wrong
count.** Still valid at HEAD: `cargo metadata --no-deps` lists no target
for `crates/hares-core/tests/{parity,bestest,conditioned_oracle,
freefloat_oracle,structural_envelope_oracle,warmup_regression,
resstock_smoke}.rs`; every other `crates/*/tests/*.rs` and
`tools/*/tests/*.rs` file is a target. -> new entry BR-12 (delete the six
shims, move the four seasonal 72 h tests into the compiled
`tests/resstock_smoke.rs` with assertions matching their doc comments,
delete the shadowed file, acceptance by `cargo metadata` comparison). The
count claim is moot at HEAD: `git grep -nE '#\[ignore' -- '*.rs'` prints
nothing, so no ignored instance remains; BR-8's text is not edited (append
only). `12-inputs.md:382-387` targets the shadowed file: item for the `12`
owner.

**12. PR-6's allocation test cannot isolate what it claims.** Fixed:
`ledger/07-profiler.md` PR-6 adds the `pub` store `ActorTimings` and the
test `actor_timings_record_without_allocating`: "an `ActorTimings` built
for 2 slots, then 96 x 2 `record` calls; the `thread_allocations()` delta
across the calls is `Some(0)`. Whole-step `Some(0)` is not asserted here
... `06-runtime-memory.md` RT-1 and RT-3 own that assertion."

### Minors

1. `[test-observe-profiling]` missing from the failing list. Fixed in
   `plan.md` §3: "At the Task 0 head the block fails on `[clippy]`,
   `[test-observe]`, `[test-observe-profiling]`, `[test-release]`,
   `[advisories]` and `[stray]`". BR-1 (committed) did turn it green
   (`implementation.md`: `[test-observe-profiling]` (1331) green), so BR-1's
   text needs no change. The schedule preamble's list: schedule item below.
2. Schedule row range "rows 1 to 10". Schedule owner (below).
3. M-1 cites `.gitignore:50`; `*.parquet` is line 49 at HEAD. `02` owner.
4. M-1 "from RT-4" gap. `02` owner.
5. Metrics flattening unspecified. Already fixed at `979b8773`:
   `tools/frame_golden/src/golden.rs:87` `flatten_metrics` writes dotted
   field paths (`"total_energy_kwh.net_energy_kwh"`), `None` as
   `{"bits": null, "value": null}`, enums as text (`"coverage":
   "PartialYear"`). The M-1 text: `02` owner.
6. Health layout unspecified. Already fixed at `979b8773`: the dwelling
   golden carries `health` as the serialised `RunHealth` (counters plus the
   tagged `WarmupOutcome`); fleet health is `null` (`golden.rs:58-60`). The
   M-1 text: `02` owner.
7. Behaviour on a defaults mismatch unspecified. Still valid at HEAD:
   `defaults_digest` is written (`capture.rs:43-44`) and never read by
   `compare` (`compare.rs:172-200`) -> new entry BR-13 (`compare` fails on a
   mismatch naming both digests; `delta` reports it).
8. `[simulation]` not strict before OD-2. Already fixed at `979b8773`:
   `tools/frame_golden/src/manifest.rs:72-87` `SIMULATION_KEYS` and
   `validate()` reject any `[simulation]` key outside the
   `SimulationConfig` field list. The M-1 text: `02` owner.
9. EPW source years. `02` owner.
10. M-3 item 4 defaults replacement. M-3 is not committed: `02` owner.
11. M-3 item 1 measures the tool's writer. M-3 is not committed: `02` owner.
12. PR-3's rayon bench under-counts. Fixed in `07` PR-3: "`benches/rl_episode.rs`'s
    vectorised group ... takes its allocations-per-episode figure from a
    separate counting pass that steps the same dwellings sequentially with
    `iter_mut()` on the bench thread, outside the timed loop".
13. PR-3 before and after on different binaries. Fixed in `07` PR-3:
    "`frame_golden` installs it only when built with `-F profiling` ... the
    plain release binary whose wall times M-3 recorded keeps the system
    allocator", with the test
    `frame_golden_installs_the_allocator_only_under_profiling`.
14. PR-2 and PR-4 test files unnamed. Fixed in `07`: both name
    `crates/hares-core/tests/profiling.rs` and the cz2a fixture.
15. PR-5 stub missing. Fixed in `07` PR-5: "this entry adds it: a
    `ProfilingSummary` `TypedDict` ... `def profiling_summary(self) ->
    ProfilingSummary`, and a docstring stating the `NotImplementedError`";
    PR-4 and PR-6 extend it.
16. PR-6 entry count depends on built-in actors. Fixed in `07` PR-6: "The
    assertions name the two entries rather than count the list, because
    `auto_register_actors` may register built-in actors first". The probes
    declare no interests, because at HEAD the interest filter can skip
    `decide()` (`dwelling/mod.rs` `should_call`), which would break
    `calls == 96`.
17. BR-6c "remove each upper cap" ambiguous. BR-6c is the row in progress
    and its text is not editable here. Item for the BR-6c implementer: the
    working tree already drops `maturin<2`, `pandas<3.0` and `pint<0.25`;
    `requires-python = ">=3.12,<3.14"` is not a dependency cap and is out of
    the rule's scope (the CI matrix tests 3.12 and 3.13).
18. BR-6d quotes the consumer source wrongly. Not yet run; text not editable
    here. Item for the BR-6d implementer: the consumer's line is
    `ochre-next = { path = "HARES", editable = true }` (consumer
    `pyproject.toml:32` on `i08-consumer`).
19. BR-7 census misses TOML-built configs. Already fixed at `81cc6d85`: "Fixture-config-driven
    construction paths sanitize the parsed config (the fixture TOMLs carry
    no output settings, so the defaults wrote to a working-directory-relative
    path)"; `[stray]` empty at HEAD.
20. CI misses `[test-observe-profiling]` and `[golden]`. Still valid
    (`ci.yml:150-156`) -> BR-10 adds both.
21. Tool versions not recorded. Fixed in `plan.md` §3: block line
    `{ rustc -V; cargo nextest --version; cargo deny --version; uv --version; } | tee target/tools.txt   # [tools]`
    and the `[tools]` rule ("a version that differs from the previous row's
    is stated with the row that first ran under it").
22. `[stray]` does not exclude `.shipwright/`. Fixed in `plan.md` §3: the
    `stray()` function adds `':(exclude).shipwright' ':(exclude).workstrees'
    ':(exclude).claude'`. Checked at HEAD: all three directories are ignored
    and change while a block runs (ledger files, engine-under-test
    worktrees and their build directories, editor-session state), so each
    new file there would fail `[stray]`.

### Notes outside the scope of `verify-L2-a.md`

- `decision-log.md` `dl-i08-process.readiness` (lines 299, 300, 312) holds
  three em dashes and the verdict "ledger cleared for execution". Operator
  item (this pass never edits the decision log).
- `plan.md` §2 em dashes (now lines 264 and 590): §2 owner.
- `12-inputs.md:382-387`: `12` owner (above).

### `verify-L1-a` table

No action: every row is marked fixed or points to a finding above.

## verify-L2-e.md (findings in this scope)

**9 (major), `07` part.** Fixed: PR-1's acceptance now names the inputs and
settings ("`building.xml`, `schedule.csv`, `weather.epw`; start
`2023-01-01T00:00:00-07:00`, no warm-up, `write_output: false`") and calls
them "the cz2a fixture"; PR-2, PR-3, PR-4, PR-5 and PR-6 tests use it, and
PR-6 names its two probe actors. The other areas' tests: their owners.

**14 (major).** Fixed: `plan.md` §3 **Writing** now separates what ships
from these documents: "Anything that ships also carries no planning ID
(ledger entry IDs, requirement IDs, review IDs, ticket IDs), no reference to
reviewers, review rounds or agents. These documents may name the readiness,
review and red-team process the decision log defines, and may cite, as
cross-references, the ledger's entry IDs and the identifiers of the source
documents in this directory ... Where a requirement ID is spelled like a
ledger entry ID ..., the ledger writes it with its source ("requirement
PY-1")." `plan.md` §1 now states the readiness rule as a current gate plus
the rule for later findings. `07` cites no requirement ID. Applying the
"requirement PY-n" spelling in other ledger files: their owners.

**21 (minor).** Fixed: PR-4's `finish` is `Option<StdDuration>`, "`None`
until `finish()` has run"; the test asserts `None` before and `Some` after
and that the file exists: "The duration's value is not asserted (no timing
assertion, `plan.md` §3); it is recorded in the measurement below." The
§2.9 field type: §2 owner.

**25 (minor).** Still valid (the `02` log holds no `--run-ignored only`
result) -> BR-12: "In an archive copy of `b674a767^` under `target/` ...
run `cargo nextest run --workspace --all-features --run-ignored only
--no-fail-fast` and record each of the then-ignored tests' result ... A test
that failed there is a defect report".

**28 (minor).** "when this plan was written" (BR-6b) and "at plan time"
(BR-6c) are in committed or in-progress entries of `01`, which this pass may
only append to. Item for whoever may edit `01` in place: replace both with
"at 2026-09-29".

**30 (minor).** Fixed: `plan.md` §3 operator checkpoints add "the index
changes of `ledger/10-slop-cleanup.md` SL-8: each `git rm -r --cached` that
stops tracking the planning and review records, and the untracking of
`.shipwright/constitution.md`". The same list now names the
engine-under-test worktree path of `dl-i08-execution.worktrees`
(`/home/rich/src/HARES/.workstrees/`) instead of `~/src/hares-eut/`.

**31 (minor).** Fixed: §3 **Writing**: "A public GitHub issue reference
(`HARES#28`, `HARES#31`) may appear in a commit message, where it links the
commit to the public issue it closes; never in code, comments or docs."

**41 (minor).** `02` owner (M-4a). BR-10 makes CI run on pull requests
into `initial-implementation`; M-4a's workflow should use the same
triggers (`workflow_dispatch` and `pull_request` into
`initial-implementation`), not a push to `feat/output-results`.

**44 (minor).** Fixed in `plan.md` §3: "updates the tool's engine adapter in
the entry's commit, so every commit compiles and passes `[golden]`." The
same sentence in M-1: `02` owner.

**46 (minor).** Fixed in `plan.md` §3: the `census()` function strips the
line number from both the run's skips and the census file before `diff`,
so the census file needs no rewrite and the in-progress row is not
disturbed; the `[pytest]` note says so. M-0's "Each census line names a
file, a line and a reason": `02` owner (add that the comparison ignores the
line).

Other `verify-L2-e.md` findings concern `plan.md` §2, `review.md`,
`initiative.md`, `exploration.md` and ledger files `02` to `06` and `08` to
`12`; they are not handled here. Finding 17 (factorization count) touches
§2.9: §2 owner.

## Schedule changes (`ledger/00-schedule.md`, for the integration pass)

Rows:

| Position | Entry | Title | File | Commit | Consumer check |
|---|---|---|---|---|---|
| between row 3 (BR-2) and row 4 (OR-5), as executed (`21d64b7b`) | BR-9 | bound nextest parallelism | 01 | `fix: bound test parallelism with a committed nextest profile` | B0 (CN-16) |
| between row 13 (BR-6d) and row 13a (M-3) | BR-11 | golden manifests: 0 means no warm-up | 01 | `fix: a golden manifest with no initialization duration runs no warm-up` | B0 (CN-16) |
| directly after row 13a (M-3), first | BR-10 | CI runs on `initial-implementation` and its pull requests | 01 | `ci: run on initial-implementation and its pull requests, and add the observe-profiling and golden lines` | B0 (CN-16) |
| after BR-10 | BR-12 | seasonal ResStock tests compiled; shadowed test files deleted | 01 | `test: compile the seasonal ResStock tests and delete test files no target builds` | B0 (CN-16) |
| after BR-12 | BR-13 | `compare` fails on a defaults-digest mismatch | 01 | `fix: golden comparison fails when the defaults tree differs from the capture` | B0 (CN-16) |
| after BR-13 | BR-14 | tariff in golden manifests; month-boundary billing fixture | 01 | `test: add a tariff to golden manifests and a month-boundary billing fixture` | B0 (CN-16) |
| after BR-14, before row 14 (PR-1) | BR-15 | fleet manifests state weights and resolution | 01 | `fix: fleet golden manifests state each home's weight and the aggregation resolution` | B0 (CN-16) |

None of the six new rows changes the engine, so each B0 check is expected
identical. M-3 keeps the label 13a (the decision log cites "rows 10 to
13a"); the integrator chooses the new rows' labels (for example 3a for
BR-9, and 13b to 13f for the post-M-3 rows), with BR-11 placed between
rows 13 and 13a.

Preamble and cells:

1. Opening sequence (rules list, first bullet): add BR-9 after BR-2 and
   BR-11 after BR-6d, matching `plan.md` §3.
2. "At the Task 0 head the block cannot be green (it fails on ...)": add
   `[test-observe-profiling]`; BR-1 turns it green with `[test-observe]`.
3. "rows 1 to 10 ... From row 11 (BR-6b) on": the range runs through BR-6b,
   row 11 (verify-L2-a minor 2), shifted by the BR-9 row the integrator
   inserts.
4. Row 51 (FL-2), consumer check: "golden, except `fleet_mixed_offset`
   re-captured" once the `04` owner confirms FL-2 is the entry that makes
   its aggregate non-empty (verify-L2-a major 8).

## Items for other owners

- `plan.md` §2 owner: §2.9 field list becomes `environment, control,
  ideal_capacity, actors, dispatch, equipment, envelope, invariants,
  state_snapshot, output, accounting, step_total (StdDuration each); finish:
  Option<StdDuration>; memory_high_water_kb: Option<u64>;
  hot_path_allocations: Option<u64>; hot_path_alloc_violations:
  Option<u64>; actors: Vec<ActorTiming>`, and gains `pub struct
  ActorTimings` (profiling feature, `07` PR-6). Em dashes at §2 lines 264
  and 590. `verify-L2-e.md` 17 (factorization count) and 27 (§2.10 filler).
- `ledger/02-measurement.md` owner (M-1, M-2, M-0, M-3, M-4a):
  - M-1 manifest: `initialization_duration_s = 0` means no warm-up, a
    positive value warms up (BR-11); fleet `weight` and `resolution` keys
    (BR-15); optional `[tariff]` table (BR-14); `compare` fails on a
    defaults-digest mismatch (BR-13).
  - M-1 golden text: the fleet products are `aggregate`, `weights` (one row
    per home until FL-1) and `homes`; the defaults digest covers every file
    of the tree the run used, sorted by relative path (not "the run read
    ... in load order"); the metrics flattening (dotted paths, `None` as
    null, enums by name) and the health layout as implemented; `[simulation]`
    keys checked against `SIMULATION_KEYS`; `.gitignore:49`; "until RT-4";
    "in the entry's commit" (`verify-L2-e.md` 44).
  - M-2: `initialization_duration_s` per fixture after BR-11 (604800, and 0
    for `consumer_shape_900s`); `fleet_mixed_offset`'s HEAD aggregate is
    empty because its homes' column sets differ, and the entry that changes
    it (per the `04` owner) re-captures it under the schema change rule;
    `month_boundary_billing_3600s` (BR-14) in the fixture table; the EPW
    description ("TMY3 rows from several source years, none a Feb 29").
  - M-0: the census comparison ignores line numbers (`verify-L2-e.md` 46).
  - M-3: items 1 and 4 (`verify-L2-a.md` minors 10 and 11).
  - M-4a: triggers per `verify-L2-e.md` 41, consistent with BR-10.
- `ledger/03-output-reporting.md` owner: OR-3/OR-4 remove the diagnostics
  writer's verbosity-only gate and its derivation from `output_path`
  (`dwelling/mod.rs:2818-2824`), as `dl-i08-execution.progress` assigns;
  OR-10's time-of-use fixture (`verify-L2-e.md` 6) can reuse BR-14's
  `[tariff]` table.
- `ledger/04-fleet.md` owner: name the entry (FL-2 per `verify-L2-a.md`
  major 8) that makes `fleet_mixed_offset`'s aggregate non-empty, have it
  re-capture under the schema change rule, and change its consumer check;
  FL-4's acceptance needs a fixture whose homes share a column set across
  offsets; FL-1 migrates BR-15's `resolution` key to the fleet report's
  frequency.
- `ledger/05-python-surface.md` and `ledger/09-numeric-fixes.md` owners:
  at HEAD the TOML path maps `initialization_duration_s = 0` to `Some(0 s)`
  and warms up (`dwelling/mod.rs:1935-1937`) while the Python binding maps
  `0` to no warm-up (`py_dwelling.rs:2473-2479`); PY-8 (one config type) or
  B-2/B-3 (warm-up) must make `0` mean the same everywhere. B-2's manifest
  migration (`warmup = "disabled"`) maps `initialization_duration_s = 0`
  to disabled. B-4's dependency names `month_boundary_billing_3600s`
  (BR-14).
- `ledger/12-inputs.md` owner: `:382-387` change
  `crates/hares-core/tests/resstock_smoke.rs`, which BR-12 deletes; retarget
  to `tests/resstock_smoke.rs`.
- In-place edit of `ledger/01-baseline-repair.md`, deferred to the row's
  own commit: BR-6c minor 17 and BR-6d minor 18 notes above; "when this plan
  was written" and "at plan time" become "at 2026-09-29"
  (`verify-L2-e.md` 28).
- Operator: `decision-log.md` `dl-i08-process.readiness` em dashes and its
  verdict wording against the L2 findings.
