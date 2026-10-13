# Slop cleanup

Slop found in code this ledger touches: narrating or useless comments,
planning or ticket references in shipped code, em dashes, verbose hedging
documentation, dead code, silent fallbacks, duplicated logic and misleading
comments. Em dashes and other slop are fixed in the code each entry touches;
there is no separate global sweep (`dl-i08-architecture.slop_scope`). An
item is fixed in the commit of the entry named under "Fixed by"; an item
marked "own commit" has its own schedule row. Nothing here changes
behavior; SL-7 proves it with the golden fixtures.

## SL-1: the telemetry snapshot's "slot reuse" comment is false

**Evidence:** `crates/hares-core/src/dwelling/mod.rs:6961-6963` says
`clone_from` lets steady-state telemetry keys "reuse their f64 slots
without reallocating the inner map". `Telemetry` derives `Clone`
(`crates/hares-types/src/telemetry.rs:11-12`), so `clone_from` reallocates
the whole map. The restore path (`mod.rs:5278`) has the same call and
no comment. The doc of `EnvironmentState::upsert_domain_ref`
(`crates/hares-types/src/environment.rs:510-512`) makes the same false claim
for `DomainUpdate`.

**Fix:** the telemetry comment is rewritten to describe the in-place copy
RT-1 implements; `upsert_domain_ref` and its doc are deleted with RT-1's
fixed domain slots, whose `set_from` doc states that the copy reuses the
slot's vectors.

**Check:** RT-1's reviewer reads the rewritten comment against
`telemetry_clone_from_same_keys_allocates_nothing` and the `set_from` doc
against `domain_update_clone_from_reuses_allocations`; each comment states
what its test proves.

**Fixed by:** `06-runtime-memory.md` RT-1.

## SL-2: census of planning and review references in shipped code

**Evidence:** at `ebdb1a17`,
`git grep -nIE '\bT-[0-9]{3,4}\b|\bI-0[0-9]\b|\bTask [0-9]|\bFinding [0-9]|[Rr]eviewers?\b|\b[Tt]ickets?\b|\b[Rr]ound [0-9]' -- crates python tests benches notebooks scripts docs ':!docs/reviews' ':!docs/tickets' ':!docs/findings' ':!docs/superpowers' ':!scripts/reviews' ':!scripts/triage.py' ':!scripts/review-all.py' ':!scripts/review-all.sh' ':!scripts/verify-tickets.sh' ':!scripts/verify-ticket-prompt.md'`
returns 451 lines in 122 files: 431 lines in 112 files under the code trees
and 20 lines in 10 files under `scripts` and `docs`. Examples:
- ticket IDs such as `T-0226` (`crates/hares-core/src/dwelling/synthetic.rs:141`),
  `T-0034` (`crates/hares-envelope/src/state_space.rs:443`), and the block in
  `crates/hares-equipment/tests/si_guard.rs:636-697`;
- ticket references without an ID ("the ticket's completion bar",
  `crates/hares-core/src/actors/ev_driver/mod.rs:7209`; "ticket 040",
  `crates/hares-envelope/tests/radiant_gain_weights_zero_alloc.rs:1,229`;
  "docs/tickets/132", `ev_driver/mod.rs:7336`);
- initiative IDs such as `I-01` (`crates/hares-envelope/src/boundary_rc.rs:2495,2500`),
  `I-06` (`crates/hares-core/src/dwelling/mod.rs:5263`), `I-07`
  (`crates/hares-equipment/src/config.rs:472`,
  `tests/fixtures/parity/cz5a_ev_charging/config.toml:16`), `I-02`
  (`docs/alignment/DIVERGENCES.md:50,60,86`,
  `docs/physics/exterior-surface-balance.md:78`,
  `docs/physics/input-vector-anatomy.md:46`);
- review references: "Finding N" (`crates/hares-core/src/actors/ev_driver/departure.rs:280`,
  `crates/hares-equipment/src/water_heater/heat_pump_wh.rs:562`,
  `crates/hares-equipment/src/battery/degradation.rs:733`,
  `scripts/ochre_bestest_600.py:121`), "future reviewer"
  (`crates/hares-equipment/tests/hvac_tests.rs:3499`,
  `crates/hares-envelope/tests/interior_lwr.rs:50`,
  `crates/hares-equipment/tests/si_guard.rs:46`), "round 2" / "round 3"
  (`crates/hares-core/src/dwelling/conversions.rs:2587-2589`);
- ticket IDs in the CI scripts SL-8 edits
  (`scripts/check-ashrae-chapters.sh:8,14-15`, `scripts/check-citations.sh:5,7`)
  and in user documentation (`docs/references/ashrae-hof-2021-chapters.md:5,52,60`,
  `docs/alignment/DIVERGENCES.md:74`, `docs/development.md:276`);
- task headers `// ---- Task 1: ...` and `// ---- Task 2: ...`
  (`crates/hares-io/src/hpxml/resolve_hvac.rs:6497,6536`).

None of the patterns has a legitimate use in these trees; citations such as
`NREL/TP-`, `FHWA-PL-` and `SHA-256` do not match them. The planning and
review records under `docs/` and `scripts/reviews/`, and the review tooling
SL-8 moves with them, leave the public tree in SL-8 and are excluded above.

**Fix:** each comment or passage states the defect class or contract it
protects in its own words, keeping its technical content and dropping the
reference. For example "the column was permuted to another depth's ground
node (I-01 defect)" becomes "the column was permuted to another depth's
ground node"; "Regression for ticket 051 Defect 2: ASHRAE HoF 2021 Ch. 27
requires ..." becomes "ASHRAE HoF 2021 Ch. 27 requires ..."; the task
headers become `// -- minisplit stage remapping --` and `// -- MSHP
resolver-level speed forcing --`. Identifiers named after tickets (for
example `ticket_n_i`, `crates/hares-envelope/src/thermal_solver/solar.rs:1119`)
are renamed after the quantity they hold.

**Fixed by:** SL-3's rule, in the commit of each entry that touches the
file.

## SL-3: planning references and em dashes are removed where an entry touches code

**Kind:** slop cleanup, a standing rule applied by every entry (no commit
of its own; `dl-i08-architecture.slop_scope`).

**Change:** an entry that edits a file rewords every SL-2 census match and
every em dash in the lines it touches and in the items (function, type,
test, doc block, script header, documentation section) those lines belong
to, as SL-2's fix describes. Matches in files no entry touches stay; there
is no separate global sweep.

**Acceptance test:** the `[writing]` lines of the verification block
(`plan.md` §3), whose pattern includes SL-2's, fail any commit that adds a
match or an em dash; each entry's reviewer checks that the items it touched
hold none.

**Verification:** `[writing]`.

**Commit:** none of its own; part of every entry's commit.

**Dependencies:** none.

## SL-8 (own commit): planning and review records leave the public tree

**Kind:** slop cleanup (`dl-i08-architecture.public_planning_docs`).

**Problem:** HARES is public, and its tracked tree carries dated planning and
review records that are not user documentation: `docs/reviews/`,
`docs/tickets/`, `docs/findings/`, `docs/superpowers/` and
`scripts/reviews/`. They name internal ticket and review IDs throughout and
the old package name (`ochre_next`), so the package rename census (PY-7)
would otherwise have to carve them out. Shipped code comments, two CI
script headers and two user documents point into them, and a third
document is tracked inside an ignored directory.

**Evidence:** at `ebdb1a17`, `git ls-files` lists 285 files under
`docs/reviews`, 114 under `docs/tickets`, 28 under `docs/findings`, 3 under
`docs/superpowers` and 40 under `scripts/reviews`; `.gitignore:63` ignores
`.shipwright` and `.gitignore:70` ignores `docs/reviews/_logs/`.
`git grep -n -E 'docs/(reviews|tickets|findings|superpowers)|scripts/reviews'`
outside the moved paths matches:
- code comments that cite a review or ticket file as the source of a value
  or a test:
  - `crates/hares-equipment/src/ev/catalog.rs:119, 323, 1168, 1229, 1242,
    1255, 1266` (EV specifications);
  - `crates/hares-equipment/src/hvac/coil_physics.rs:289, 1361` (the
    apparatus dew point guard);
  - `crates/hares-equipment/src/water_heater/heat_pump_wh.rs:562` (the UEF
    cross-fade);
  - `crates/hares-io/src/output/metrics.rs:2627` (nullable setpoint tests);
  - `crates/hares-core/src/dwelling/mod.rs:13021` (the BESTEST radiant
    fraction test);
  - `crates/hares-core/src/actors/ev_driver/mod.rs:7336` (the low-SOC gate
    test);
- user documentation prose: `docs/eplus/section-mapping.md:27-28` and
  `docs/references/ashrae-hof-2021-chapters.md:67`;
- the CI scripts' headers and exclusion lists:
  `scripts/check-ashrae-chapters.sh:13-15, 30-35`,
  `scripts/check-citations.sh:12, 23-27`,
  `scripts/check-energyplus-sections.sh:19-22`, and the message naming
  `findings` in `scripts/check-ochre-commit.sh:44`;
- `.gitignore:70`.
`git ls-files -ci --exclude-standard` lists two tracked files that the
ignore rules cover: `.shipwright/constitution.md` and
`docs/eplus/section-mapping.md` (`.gitignore:65` ignores `docs/eplus/`,
whose only file is that HARES citation guide).

**Change:**

- `mkdir -p` the target parents, then `git mv` each of the five
  directories to `.shipwright/archive/<same relative path>` (the moved
  paths stay on disk, under the ignored `.shipwright/`), then `git rm -r
  --cached .shipwright/archive`, so the commit removes them from the
  tracked tree while nothing is deleted locally. Measured on a scratch
  repository: `git mv` into a missing directory fails ("No such file or
  directory"); into an existing ignored directory it succeeds and the files
  stay tracked at the new path (ignore rules do not apply to tracked
  files); `git rm -r --cached` then untracks them and leaves them on disk.
- The review tooling that exists only to produce or audit these records
  moves with them by the same commands: `scripts/triage.py` (reads
  `docs/reviews`, `:26, 80`), `scripts/review-all.py` and
  `scripts/review-all.sh` (write `docs/reviews`, `.py:10`, `.sh:17, 23`),
  `scripts/verify-tickets.sh` (audits `docs/tickets`, `:33, 39`) and
  `scripts/verify-ticket-prompt.md`.
- `.shipwright/constitution.md` is untracked with `git rm --cached` and
  stays on disk. The `git rm --cached` steps change the index of a shared
  working tree, so they run at an operator checkpoint (`plan.md` §3).
- Each code comment above keeps the primary source the review file
  recorded and drops the path and any "Finding N":
  - the EV catalog cites fueleconomy.gov for fuel economy and range, the
    Nissan, Ford and Toyota specifications (with the EV-Database.org and
    Wikipedia pages already named) for on-board charger power, and the
    misspelt `puedeconomy.gov` in the same test (`catalog.rs:1174`) reads
    `fueleconomy.gov`;
  - the two coil comments keep their EnergyPlus `WaterCoils.cc` citation
    and name the guard in `coil_bypass_factor` by function, not by line;
  - the HPWH comment states its reason: the cross-fade over UEF 4.8 to 5.0
    is HARES's own choice, to avoid a step change in COP and capacity at
    OCHRE's low-power threshold of UEF 4.9
    (`vendors/OCHRE/ochre/Equipment/WaterHeater.py:444`);
  - the metrics test comment states the contract it pins (a missing
    setpoint skips only the comfort and unmet-hours accumulation);
  - the BESTEST test doc keeps the EnergyPlus Input Output Reference and
    BESTEST IDF citation and says the 60% figure applies to occupant
    sensible gains, not to `OtherEquipment`, without the audit history;
  - the EV driver test doc says that a `LowSoc` threshold of 0.15 sits
    below the anxiety band, so the override governs it.
- The CI checks that prune the moved directories drop the dead entries from
  their exclusion lists (`check-ashrae-chapters.sh:30-35`,
  `check-citations.sh:23-27`, `check-energyplus-sections.sh:19-22`), the
  message in `check-ochre-commit.sh:44` drops the moved file, and the
  headers of `check-ashrae-chapters.sh` and `check-citations.sh` state what
  each check enforces (the ASHRAE HoF 2021 chapter for each topic; the
  corrected Cutler et al. (2013) and Winkler (2009) citations) with no
  ticket ID and no exemption for the moved directories.
- `.gitignore:70` (`docs/reviews/_logs/`) is deleted. `.gitignore:65`
  (`docs/eplus/`) becomes `docs/eplus/*` with `!docs/eplus/section-mapping.md`,
  so the tracked citation guide is no longer inside an ignored path and any
  local EnergyPlus reference copies placed there stay ignored.
- `docs/eplus/section-mapping.md` loses its "Legacy § references in
  historical documents" section (`:25-31`), which describes only the moved
  records. `docs/references/ashrae-hof-2021-chapters.md` keeps its
  corrected-citation list as current facts: the heading
  `## Known Citation Errors (Fixed in T-0311)` (`:52`) becomes
  `## Corrected citations`, and the ticket and review references at `:5`,
  `:60` and `:67` are dropped, each item keeping its code locations.
- User-facing documentation under `docs/` stays, and any link from it into
  a moved directory is removed or pointed at the section of user
  documentation that states the same fact.
- The planning references of SL-11 are reworded as SL-11 states, in this
  commit.

**Acceptance test:**
- `git ls-files docs/reviews docs/tickets docs/findings docs/superpowers
  scripts/reviews scripts/triage.py scripts/review-all.py
  scripts/review-all.sh scripts/verify-tickets.sh
  scripts/verify-ticket-prompt.md .shipwright` prints nothing;
- the moved paths exist under `.shipwright/archive/` on disk with identical
  content (a `sha256sum` manifest taken before the move equals the one
  taken after);
- `git grep -n -E 'docs/(reviews|tickets|findings|superpowers)|scripts/reviews|scripts/(triage\.py|review-all|verify-tickets|verify-ticket-prompt)'`
  prints nothing (the tracked tree no longer holds `.gitignore:70`, the code
  comments, the script headers or the two documentation passages);
- `git ls-files -ci --exclude-standard` prints nothing;
- SL-11's check prints nothing;
- the four CI scripts pass (`bash scripts/check-energyplus-sections.sh`,
  `check-citations.sh`, `check-ashrae-chapters.sh`, `check-ochre-commit.sh`).

**Verification:** whole block.

**Commit:** `chore: move planning and review records out of the public tree`

**Dependencies:** M-3. Lands before `05-python-surface.md` PY-7, whose
rename census then needs no exclusion list.

## SL-4: the calibration fit's "Latin-hypercube" comment is false

**Evidence:** `~/src/HARES_CALIBRATION_SYSTEM/calibration/fit.py:319-321`:
"Seed the population with nominal and the warm start, then Latin-hypercube
the rest", above `X = self.lo + (self.hi - self.lo) * rng.random((P, n))`,
which is independent uniform sampling, not a Latin-hypercube design.

**Fix:** the comment is reworded to say the population is filled uniformly
over the box and the nominal and warm-start candidates overwrite the first
two rows. The sampling is not changed: changing it would change every fit.

**Check:** CN-6's reviewer confirms the reworded comment against the code
below it, and CN-6's B0 gate shows the sampling unchanged.

**Fixed by:** `08-consumer.md` CN-6, which edits the same function.

## SL-5: the worker-pool shrink comment describes code that never ran

**Evidence:** `calibration/scale.py:630-637` describes the inner pools
shrinking to `cores // fleet_workers`, but `workers` is defaulted at `:630`
before the `is None` guard at `:633`, so the shrink never runs.

**Fix:** none beyond CN-5: once CN-5 applies the shrink, the comment is
accurate.

**Check:** CN-5's test of the shrink (the inner pool size it asserts) is the
check that the comment now describes code that runs; CN-5's reviewer
confirms the comment matches the fixed control flow.

**Fixed by:** `08-consumer.md` CN-5.

**State:** closed as `60ccfbc` (consumer branch `row/cn5-pool-sizing`): the
shrink runs before the default inside `scale._pool_sizes`, and the
docstring's promise, the inner pools at `cores // fleet_workers` when the
caller passed no `workers`, is the code's behaviour on every path. The
shrink's test (`fleet_pool_shrinks_inner_workers_when_workers_unset`,
`_pool_sizes(4, None, 4, 28) == (4, 7)`) is the check that the comment now
describes code that runs; the fixed control flow and the comment were read
against each other in the row's review, which is this entry's check.

## SL-6: `check_losses.py`'s docstring claims a check the tool does not perform

**Evidence:** `tools/check_losses.py`'s docstring calls the tool "the
cheapest possible answer to did that change move the physics", but
`main()` always returns 0 (`check_losses.py:44`).

**Fix:** once CN-9 makes the exit code reflect a moved loss, the docstring is
true; CN-9 adds one sentence stating the comparison it performs.

**Check:** CN-9's test that a moved loss gives a non-zero exit code is the
check that the docstring's claim holds.

**Fixed by:** `08-consumer.md` CN-9.

## SL-7 (own commit): remove the dead coupled-LU path and the identity solves

**Kind:** slop cleanup.

**Evidence:** `StateSpaceModel` has one implicit matrix M, and both
constructors set it to the identity (`crates/hares-envelope/src/state_space.rs:201-228`,
`:403-425`, `m_is_identity: true`); nothing ever sets `m_is_identity` to
`false`. Therefore:
- `build_coupled_lu` (`:695-708`), `step_with_coupled_lu_into` (`:553`),
  `step_with_coupled_lu_into_with_forcing` (`:572`),
  `solve_for_scalar_input_coupled` (`:718`) and `_into` (`:907`), `CouplingData`
  (`:38`, re-exported at `crates/hares-envelope/src/lib.rs:34`),
  `CoupledState::LU` (`thermal_solver/mod.rs:74-75`) with its arms
  (`thermal_solver/stepping.rs:375-388, 678-686, 719-736`), the
  energy-balance branch on `m_is_identity()` (`stepping.rs:1056`) and its
  non-identity arm that calls `build_coupled_lu` (`:1149`), and
  `ThermalSolver.m_scratch` (`thermal_solver/mod.rs:93-94`) are unreachable
  from any model;
- `m_lu` is the LU of the identity, and `step_into` (`state_space.rs:434`),
  `solve_for_scalar_input` (`:882, 888`, behind `solve_for_output_input`,
  `:499`) and `verify_stability` (`:482-484`) run triangular solves against
  it: work that returns its input except, at most, the sign of a zero;
- `solve_for_output_input_into` (`:974`) and the `SolverScratch` (`:23-36`)
  and `SolveTarget` (`:44-48`) it takes, re-exported at `lib.rs:34`, have no
  caller (`git grep` at `afa9a071` finds only their definitions and the
  re-export); their `gemv` accumulation with `beta = 1` adds `B_eff·u` into
  `N·x` term by term, which is not the operation order of the solve the
  solver calls, so they could not serve as its allocation-free form
  (`06-runtime-memory.md` RT-3 gives that solve its own scratch);
- the doc comment of `ThermalSolver::solve_ideal_capacity_for_target`
  (`thermal_solver/stepping.rs:354-355`) says "the coupled LU is cached",
  describing the dead arm;
- the `debug_assert`s comparing the freshly built identity with another
  identity (`:211-218`, `:407-414`) cannot fail;
- the docs saying M is caller-supplied for `from_discrete` (`:107`, `:266`)
  or non-identity in tests (`:672-674`) are false;
- `step_into_with_forcing` (`:445-462`) is `#[allow(dead_code)]` with no
  caller;
- the bench `crates/hares-envelope/benches/rc_solver.rs` measures the dead
  path in `bench_coupled_step_lu` (`:281-306`), and
  `bench_coupled_step_identity` asserts `m_is_identity()` (`:264`).

**Change:** the model is `x[k+1] = N·x[k] + B·u[k]` with the coupled step's
closed-form diagonal scaling. `m_mat`, `m_lu`, `m_is_identity`, the LU path,
`CouplingData` and its re-export, `CoupledState::LU`, the energy-balance
branch on `m_is_identity()` (its identity arm becomes the only code),
`m_scratch`, the identity solves, the vacuous asserts, the dead forcing
variants, `solve_for_output_input_into` with `SolverScratch`, `SolveTarget`
and their re-exports, and the false docs (the LU sentence of
`solve_ideal_capacity_for_target`'s doc included; RT-3 rewrites its
"Zero allocation" claim once the test proves it) are removed; `verify_stability` takes the
eigenvalues of N. In `benches/rc_solver.rs`, `bench_coupled_step_lu` and its
`criterion_group!` entry (`:316`) are removed, and
`bench_coupled_step_identity` drops its `m_is_identity()` assertion. The
tests that used `build_coupled_lu` as an oracle for the closed form
(`state_space.rs:2017-2420`, the calls at `:2039, 2196, 2256, 2338, 2420`)
keep that oracle by building and factoring `(I + D)` inside the test.

**Acceptance test:** M-2's golden fixtures compare bitwise equal (the only
arithmetic an identity solve can change is the sign of a zero, so any
difference fails the entry and is investigated); the closed-form oracle
tests pass; `grep -rn 'm_lu\|m_is_identity\|build_coupled_lu\|CoupledState::LU\|m_scratch\|CouplingData\|SolverScratch\|SolveTarget\|_input_into\|coupled_into' crates/hares-envelope`
returns nothing.

**Verification:** `cargo nextest run -p hares-envelope`; `cargo bench
--no-run`; `frame_golden compare-all`.

**Commit:** `refactor: remove the unreachable coupled-LU path and identity solves from the state-space model`

**Dependencies:** M-1, M-2. Lands before RT-3.

**Measurement:** share of `simulate()` in `step_into` and
`solve_for_output_input` before and after.

**State:** committed `7b7a6a7d`. Independent audit
`verification/audit-SL-7-7b7a6a7d.md`: no blocker; the Measurement above
was not recorded at commit and is taken as an A/B of `b8195c83` against
`7b7a6a7d`, with the physics suites, in `02-measurement.md`'s SL-7
section. The stale stability-check comment it found is fixed by RT-3.

## SL-9: `profile_sim` example carries dead code under observe-only builds

**Evidence:** `crates/hares-core/examples/profile_sim.rs` gates its real
`main` on `#[cfg(feature = "profiling")]` (`:49`) but leaves `config` (`:20`),
`use std::time::Instant` (`:15`) and `use hares_core::dwelling::Dwelling`
(`:17`) ungated, so under `-F observe` without `profiling` the example
compiles three rustc warnings (`unused_imports` twice, `dead_code` for
`config`). No verification-block line builds examples with profiling off
(`[clippy]` runs `--all-features`, where `profiling` is on; the nextest lines
do not build examples), so this never gates; it surfaces in ad-hoc
observe-only checks.

**Fixed by:** `01-baseline-repair.md` BR-2 (`fd23dd80` gates the imports
and `config` on `profiling`, so every feature set compiles warning-free);
`07-profiler.md` PR-1 (`d7f0e381`) then deletes the example.

## SL-10: `SteppableFleet`'s per-home accessors hide failures

**State:** fixed by row 52 (FL-3, `455eb8a3`): the accessors return
`Result<_, FleetError>` (`NoSuchHome`, `Telemetry`), Python maps each
variant to its exception through one conversion function, a failed
snapshot in `step()` is that home's `ok: false` item, and the building
ids come from an infallible iterator; the check below prints nothing.

**Evidence** (at `21d90588`): `SteppableFleet::telemetry`
(`crates/hares-fleet/src/fleet.rs:638-659`, added by
`03-output-reporting.md` OR-5's `dc033cee`) maps a telemetry snapshot that
cannot be built to `None`, "the same signal as an out-of-bounds index",
and logs it; Python then reports that failure as an index out of range
(`crates/hares-python/src/py_fleet.rs:551-562`), and `step()` turns it into
a silent `None` reactive power (`py_fleet.rs:449`). `set_grid_voltage` and
`apply_control` log an out-of-range index and return (`fleet.rs:612-636`).
`py_fleet.rs:446` maps every index in `0..fleet.len()` through
`bldg_id(idx).unwrap_or(-1)`, a fallback that cannot be reached.

**Fix:** the accessors return `Result<_, FleetError>` (`NoSuchHome`,
`Telemetry`), Python maps each variant to its exception, a failed snapshot
in `step()` is that home's error, and the building ids come from an
infallible iterator, as `04-fleet.md` FL-3 states.

**Check:** FL-3's `steppable_fleet_accessors_reject_a_missing_home` and
`fleet_error_maps_to_python_exception`; FL-3's reviewer confirms that
`git grep -n 'unwrap_or(-1)' -- crates/hares-python/src/py_fleet.rs` prints
nothing.

**Fixed by:** `04-fleet.md` FL-3, which edits `SteppableFleet` and its
binding.

## SL-11: planning references left in lines a committed entry touched, and ones SL-2's pattern does not match

**Evidence** (at `afa9a071`):
- `crates/hares-core/src/actors/ev_driver/mod.rs:6759`, "// Ticket
  tolerance is <1% drift; reconciliation is exact, so 1e-6 is the
  meaningful bound." `01-baseline-repair.md` BR-6a (`39488581`) re-indented
  this line for its let chains and left the reference, against SL-3's
  rule. The same file holds fourteen more SL-2 matches: "the ticket names"
  (`:916`, `:4479`), "Finding 5" to "Finding 8" and "Finding 1
  (integration)" (`:5415`, `:5471`, `:5497`, `:5533`, `:5600`), "the
  ticket's" (`:6864`, `:7075`, `:7210`, `:7321`, `:7335`), "the ticket
  asks" (`:7373`), and the `docs/tickets/132` path SL-8 already rewrites
  (`:7337`).
- The `[writing]` pattern of `plan.md` §3 is broader than SL-2's. Run over
  the same trees at `afa9a071`, it matches 138 lines SL-2's pattern does
  not. All but four groups are legitimate: R-values and slab labels (`R10`,
  `Half R5`), circuit resistor names and diagram node labels (`R1`, `R2`,
  `R12`, `docs/architecture.md:579-584`), EnergyPlus IDD
  versions (`V26-1-0`, `V9-6-0`), ASHRAE 169 "Table B-1", the verbosity-8
  column comments (`V8`, `dwelling/mod.rs:443, 689, 7310`), and "Rule R1",
  which `docs/equipment/power-factor.md:37` defines for users, and `V0`
  for nominal voltage (`docs/equipment/power-factor.md:271`). The four:
  the module docstrings `tests/python/test_py_fleet.py:1` ("(PY-009)"),
  `tests/python/test_py_lut_injection.py:1` ("PY-013:") and
  `tests/python/test_py_stubs.py:1` ("PY-015:"), all old ticket IDs; and
  "R3" in `crates/hares-equipment/src/hvac/ideal_hvac.rs:666, 2648, 2669,
  2670, 2688`, the number of a rule list that exists nowhere in the tree.

**Fix:** each comment states its content without the reference, as SL-2's
fix describes: the tolerance comment reads "Reconciliation is exact, so
1e-6 is the meaningful bound."; each
"Finding N" heading names the behaviour its tests pin; "the ticket names"
and "the ticket's completion bar" name the requirement itself (the
range-anxiety backstop; the override's effect on the charging profile);
the three docstrings drop the ID; "R3" becomes "the minimum-capacity rule".

**Check:** `git grep -nP '\b[Tt]ickets?\b|\bFinding [0-9]|\bPY-0[0-9]{2}\b|\bR3\b' --
crates/hares-core/src/actors/ev_driver/mod.rs crates/hares-equipment/src/hvac/ideal_hvac.rs
tests/python/test_py_fleet.py tests/python/test_py_lut_injection.py tests/python/test_py_stubs.py`
prints nothing.

**Fixed by:** SL-8 (`00-schedule.md` row 25), which already edits
`ev_driver/mod.rs` and is the commit that takes planning references out
of the public tree; the edits are comment-only, so the golden fixtures
compare bitwise.
