# Review of 1bb045c6 (OR-19 roster plan/install), range 1c18543a..1bb045c6

Reviewer: independent; worktree .workstrees/review-or19 detached at 1bb045c6 (vendors/OCHRE initialized). Commits: 12f753be, a3a21a92, 1bb045c6.

## Verdict: CHANGES-REQUIRED

Counts: blocker 0, major 1, minor 6, nit 1. No behavior defect found. The one major finding is a plan-block gate that fails on HEAD (`[writing]`), fixable by editing four strings. Every earlier finding is verified fixed and pinned by a test that fails without the fix.

## Findings

### M1 (major, gate failure): `[writing]` fails on HEAD, four em dashes added in 1bb045c6

Evidence: the plan.md section 3 `writing .` check (HEAD commit message clean; added-line grep) hits four added lines in crates/hares-core/src/dwelling/roster.rs:534, :610, :628, :653. All four are user-visible error strings moved verbatim from mod.rs ("a surviving equipment — a replacement", "no free equipment id — the never-reused", "on that id — ids are", "still reports {:?}) — check its"). Because 1bb045c6 is where the code moves into roster.rs, they count as added lines in that commit; 12f753be and a3a21a92 have zero hits. Reproduced by running the exact `writing()` grep against HEAD.
Fix: replace each em dash with a colon or comma (and in the same pass the moved comments are already clean). Re-run `[writing]`. No test pins these strings.

### m1 (minor, testing gap): the EV driver RNG cursor is not pinned across roster changes

Evidence: roster.rs:345 commits `next_ev_driver_stream` after a roster change. Spot check: deleting that single line in my worktree leaves the whole `cargo nextest run -p hares-core` run green (1313 passed). No test builds an EV driver through `add_equipment` after assembly and asserts it gets a stream distinct from the drivers already live, nor that a rejected add leaves the cursor unchanged, nor that `auto_register_actors`/`set_tariff` restart at 0. The in-call increment is covered (mod.rs `built_in_actors` tests); the cross-call state, the new decision, is not.
Related, not blocking: (a) the cursor is not in `DwellingCheckpoint` (mod.rs:3769). That is safe for restored actors, whose RNG state travels in the actor blob, and `derive_sub_rng` (rng.rs:43) depends only on seed and stream, so a driver built mid-run is position-independent and reproducible. But a restored dwelling's cursor equals the number of drivers built at assembly, not the original's history, so an EV added after restore may take a different stream than it would have in the uninterrupted run. (b) stream = `RNG_STREAM_EV_DRIVER_BASE (1) + cursor` and `RNG_STREAM_EVENT_LOAD_BASE` is 1000 (rng.rs:19,23); about 1000 add/remove cycles of built-in EV drivers without a `set_tariff` would alias the event-load streams. The per-call debug collision check (mod.rs:7033, `seen_rng_pairs` local to the call) no longer sees cross-call pairs. Unlikely in practice; a cheap guard (assert or error when cursor reaches the next base) or documenting the bound would close it.
"Matches old numbering in common cases": confirmed by code reading and identical goldens. Old numbering was the index among EV drivers built in one `build_actors_from_seeds` call, and every rebuild (assembly, `set_tariff`, `auto_register_actors`) reset it (roster.rs:363 resets the cursor to 0, a skipped seed consumes none, same as the old `continue` before the increment). Uncommon cases that differ: an EV added through `add_equipment`/`add_ev` after assembly (old Python path ran a full rebuild, which renumbered and re-seeded every live built-in actor and so reset their state; new path keeps existing drivers untouched and gives the newcomer the next stream), and remove-then-add (new stream never reuses a freed one). Both are deterministic and better than before (no live built-in drivers share a stream); not wrong.
Fix: add tests (add EV via `add_equipment`, assert distinct streams for two live drivers; rejected add leaves the cursor; `set_tariff` restarts it) and decide on the checkpoint/overflow points.

### m2 (minor, testing gap): `RejectedEquipment` to Python exception mapping is untested; error shape is conditional

Evidence: py_dwelling.rs:2594-2625 classifies by `reason` (an `Equipment` reason maps to `HaresEquipmentError`, `Io`/`Dwelling`/`Envelope` to `HaresConfigError`, others to `HaresSimulationError`). The logic is correct on reading, but nothing exercises it: no hares-python Rust test reaches `to_py_err` with a `RejectedEquipment`, and no file under tests/python changed in the range or references the wrapper. The only test is the serde round trip in hares-types/src/error.rs.
Shape: roster.rs:172 wraps only when the candidate raised warnings (`drained.is_empty()` returns the bare error), so the same rejection is `HaresError::Equipment` for one candidate and `RejectedEquipment{Equipment}` for another. Rust callers matching `HaresError::Equipment(_)` on `add_equipment`'s result see the wrapper only sometimes. The variant also allows an empty `warnings` vector whose Display ends in "(the rejected equipment warned: )". Suggest either always returning the warnings (struct always carries them) or exposing a `root_cause()` accessor, plus a pytest or Rust test per mapped class (duplicate-name add gives `HaresEquipmentError` with the warning text in the message; fluid conflict gives `HaresConfigError`).

### m3 (minor, testing gap): new `add_equipment` seeding and `add_equipment_with_actors` behaviour change is thin on tests and Python docs

Evidence: `add_equipment` now seeds the candidate's built-in actor; Python `add_ev` no longer calls `auto_register_actors` (py_dwelling.rs:1233) and `add_ev_with_driver` joins equipment and driver atomically (py_dwelling.rs:1344). Covered: re-add restores the built-in actor, replace under a new name evicts, existing Python tests at tests/python/test_ev_tariff_integration.py:453-579 pass. Not covered: `add_equipment_with_actors` atomicity (a supplied actor whose name is already registered must leave equipment, actors and id counter unchanged; a supplied actor named like the built-in replaces it and consumes no stream; the only caller test is mod.rs:8832 for the happy path). No Python test pins the behaviour change that `add_ev` no longer resets other built-in actors' state. The Rust doc comments describe the behaviour (roster.rs `add_equipment`, `add_equipment_with_actors`); the Python `add_ev`/`replace_equipment` docstrings do not mention that replace under a new name now evicts actors bound to the old name.

### m4 (minor, coverage lost): fluid-type consistency tests deleted without direct replacements

Evidence: the four deleted `validate_fluid_type_consistency_*` tests (ports.rs) covered (1) single loop passes, (2) mixed types rejected with the message naming the loop and both fluid types, (3) different loops pass, (4) non-Fluid ports and identical duplicates ignored. Remaining: `constructor_rejects_conflicting_fluid_types_for_same_loop` (fluid_solver.rs:1301), solver_builder.rs:4717 (message contains "conflicting fluid types") and the OR-19 dwelling tests. (1) and (3) are covered implicitly by every assembled dwelling; (2) is covered for rejection but nothing asserts the message names the loop id and both types any more (solver_builder.rs only checks the phrase); (4) is not pinned: `fluid_loop_declarations` (ports.rs:818) and `plan_loop_types` (fluid_solver.rs:104, now `pub`) have no unit tests, so the `port_type != Fluid` skip and the dedup could regress unseen. Also the error variant changed from `Dwelling("fluid type consistency validation failed: ...")` to `Envelope(...)` at assembly; both map to `HaresConfigError` in Python, so no user-visible class change.
Fix: add a `plan_loop_types` test for the message content and a `fluid_loop_declarations` test with a thermal declaration carrying a loop id and an identical duplicate fluid declaration.

### m5 (minor, testing gap): rejection is pinned for add, replace, add_actor and set_tariff only

Evidence: tests exist for those four entrances (mod.rs `rejected_add_leaves_roster_unchanged`, `rejected_replace_keeps_the_original`, `rejected_actor_add_leaves_roster_unchanged`, `rejected_set_tariff_leaves_no_tariff_and_no_parse_warnings`, plus the id-counter and warning tests). `remove_equipment`, `remove_equipment_by_end_use`, `clear_equipment` and `auto_register_actors` (all with the same plan-then-commit shape, roster.rs:~365, ~494, ~517, ~563) have no rejection test, and the actor test checks the actor list but not the schedule. The previous red team verified them by scratch fingerprint; nothing in the tree does. Likewise nothing pins that a rejected plan does not touch the live output file: the recorder is created as the plan's last fallible step (roster.rs plan_fresh_schema), so rejections before it (fluid conflict, column-map drift) leave the file alone, but that ordering is only a code-order property. Add: a drift fixture driven through each remaining entrance, and a test that a fluid-conflict rejection with output enabled leaves the existing output file bytes unchanged.

### m6 (minor, doc): ledger entry is stale

Evidence: ledger/03-output-reporting.md OR-19 still says "State: not started" and promises "bit-for-bit as it was", while the implementation adds `RejectedEquipment` (candidate warnings returned in the error), recorder creation inside the plan, `set_tariff` atomicity, `add_equipment_with_actors`, built-in seeding on add, the EV stream cursor and eviction on rename-replace. None of those decisions is recorded. Update the entry in place.

### n1 (nit): overlapping recorder handles during a pre-step re-plan

Evidence: `plan_fresh_schema` creates the new recorder (File::create truncates the output path) while the previous recorder, which holds 0 rows, is still alive until install assigns `self.recorder` (roster.rs install, FreshSchema arm). The old recorder's drop can flush a buffered Parquet magic header (`PAR1`) into the same file at offset 0; the bytes equal what the new writer writes, so output is correct, and the pre-OR-19 code had the same order. Fragile only if the writer ever buffers more before the first row.

## Verification of the claimed fixes (reverts applied in the review worktree only; worktree restored, `git status` clean)

- Recorder in plan (previous M1/M2): `construction_fails_when_the_output_file_cannot_be_created` and `a_pre_step_add_is_rejected_when_the_recorder_cannot_be_recreated` (tests/roster_recorder_failure.rs) both FAIL when recorder creation errors are swallowed (spot revert: fallback to a temp path), and pass on HEAD. Code read: `install_roster_caches` has no fallible call; every `?` is in the plan; nothing fallible runs after the recorder is created, so a rejected plan never creates or truncates a file; a failed creation keeps the live recorder (the second test asserts schema width, roster, warnings, and that the dwelling still steps).
- `set_tariff` atomicity (previous m-1): reordering the evaluator and parse warnings ahead of the plan makes `rejected_set_tariff_leaves_no_tariff_and_no_parse_warnings` FAIL; passes on HEAD.
- Candidate warnings (previous m-2): forcing `reject_candidate` to return the bare error makes both `rejected_*_returns_the_candidate_warnings_and_leaves_the_log_unchanged` tests FAIL; passes on HEAD. Behavior: the rejected candidate's warnings come back in the error, the run log is unchanged (the ledger's "bit-for-bit" is now true).
- Loop types from live declarations (OR-19 change 3): restoring fluid retention in `rebuild_port_slots` plus retained pairs in the plan makes `removed_loop_declarer_frees_its_fluid_type_before_first_step` and `runtime_added_equipment_declares_its_loops` FAIL. Note a partial revert (retained pairs only) does not fail the dwelling unit test because the rebuild already drops fluid accumulators; the two together are what the tests pin.
- Stale `refresh_equipment_caches` references: gone from crates (only historical docs/reviews mention it). The `AsRef` impls and generic planner (previous m3/m4) are gone: the planner takes `&[&dyn Equipment]`/`&[&dyn Actor]`. `RosterCaches` is a `Default` struct (previous m2).
- id counter, drift-schema and parity tests from the earlier round run and pass in the full suites below.

## Answers to the scrutiny questions

- `RejectedEquipment { reason, warnings }`: shape acceptable (boxed reason, pre-formatted warnings, serde round trip tested) but see m2 for the conditional wrapping; Python mapping is correct by reading, untested.
- Python `add_ev`: all-or-nothing now (equipment plus driver in one change); behaviour change (no full rebuild, no state reset of other built-in actors, driver survives `set_tariff` via the canonical name) is documented in Rust doc comments and a code comment, partly tested (m3).
- Cursor: deterministic, seed-and-stream derived, not checkpointed, safe for restored actor state; see m1 for the uncommon cases and the unpinned commit.
- Deletions: `rebuild_loop_types`, `validate_fluid_type_consistency` and its tests: logic fully subsumed by `plan_loop_types` (same rejection, same map), but direct test coverage shrank (m4).
- `RosterCaches::default()`: every field defaults to empty/zero, which is the correct value for output-disabled dwellings and is replaced by the first plan, installed at the end of assembly (`auto_register_actors` runs before `build_from_blueprint_inner` returns; failure drops the dwelling). No reachable silent wrong value; a step before install would iterate empty zone caches silently, but no code path steps before install.
- install infallible, plan pure: install is infallible; plan is pure except for creating the recorder file as its last fallible step (documented in the doc comment); rejected plans leave no stray or truncated output file (see above).
- Debug thermal-declarant warning removal: justified. The old debug-only `tracing::warn!` checked that every accumulator in a table built from `PortSlots::from_declarations(&declarations)` had a declarant in the same `declarations`, which cannot fail; after OR-19 the pre-step table intentionally retains zone accumulators with no declarant, so the check would be wrong if kept.

## Verification run (worktree at 1bb045c6, CARGO_TARGET_DIR=.workstrees/target-review-or19, -j 8)

- `cargo fmt --check --all`: pass.
- clippy `-D warnings` (all targets): `--all-features`, default, `-F dst`, `-F profiling`, `-F observe`, `-F observe,profiling`, `--release`: all pass (the pre-existing release-profile clippy failure in thermal_solver is fixed by this range).
- nextest: default 7168 passed; `-F observe` 7227; `-F dst` 7181; `-F profiling` 7174; `-p hares-core -F observe,profiling` 1368; `--release` 7125; 0 failed, 0 skipped in each.
- `cargo bench --workspace --no-run`: pass.
- `frame_golden compare-all` default, `-F dst`, `-F profiling`: every fixture identical.
- `maturin develop --release --uv` into the worktree venv then pytest `-m ""`: 1378 passed, 0 failed, 9 skipped; the skip census matches ledger/pytest-skip-census.txt exactly (skips: OCHRE python package not installed in the uv env, ResStock fixtures not available).
- `[writing]`: FAIL (M1).
- Not run: `cargo deny` advisories, `[stray]` bracket, `cargo doc`.

## Files

- /home/rich/src/HARES/crates/hares-core/src/dwelling/roster.rs
- /home/rich/src/HARES/crates/hares-core/src/dwelling/mod.rs
- /home/rich/src/HARES/crates/hares-python/src/py_dwelling.rs
- /home/rich/src/HARES/crates/hares-types/src/error.rs
- /home/rich/src/HARES/crates/hares-types/src/ports.rs
- /home/rich/src/HARES/crates/hares-envelope/src/fluid_solver.rs
- /home/rich/src/HARES/crates/hares-core/tests/roster_recorder_failure.rs
