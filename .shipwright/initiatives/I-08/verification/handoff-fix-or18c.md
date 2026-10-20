# Handoff: fix-or18c round 3 (review-OR-18c-de561181.md)

## Where things are

- Handoff branch: `handoff/fix-or18c`, commit `e3abbe70`, checked out in
  worktree `/home/rich/src/HARES/.workstrees/fix-or18c`. The worktree is clean.
- Base: `db5efe80` (`feat/output-results`; unchanged when the work started).
- The old branch `fix-or18c` still points at `de561181` (38 commits on
  `2f85338d`). It has not been moved. Move it only when the rebase is finished.
- No uncommitted work. No `vendors/OCHRE` submodule change, and no submodule
  commit.
- The rebase was done by cherry-picking onto a detached `db5efe80`, not with
  `git rebase`, so nothing is in progress.
- 23 of the 31 non-golden commits are rebased (`2f85338d..15d1e93a` in order,
  minus the 7 golden captures, which are dropped and must be re-captured).
- One `fixup!` commit, `e55739e0`, belongs in `442e404d`. Squash it with
  `GIT_SEQUENCE_EDITOR=true git rebase -i --autosquash db5efe80`. It does
  two things:
  - It fixes `building.rs` attic `Storey` construction: with
    `floors_above_grade` now an f64, the code was `total_conditioned_floors.map(..)`.
  - Because the parser now requires `NumberofConditionedFloorsAboveGrade`, the
    hip-fallback test drops `NumberofConditionedFloors` instead, and the
    `zone_geometry.rs` warning text becomes "the number of conditioned floors
    is not given".

## Commits still to cherry-pick, in order

```
c70bf586 e42d87f3 c974f591 39b4045d a8396b7e 708b0071 8676916a 15d1e93a
```

Do not cherry-pick the old golden commits (`9afd978e..de561181`). Re-capture
the goldens at the end instead.

The order of these eight follows `git rev-list --reverse 2f85338d..15d1e93a`,
lines 24 to 31:
c70bf586, c974f591, 39b4045d, a8396b7e, e42d87f3, 708b0071, 8676916a,
15d1e93a.

## How each rebase conflict was resolved (verify with range-diff)

The head (14 commits since `2f85338d`) made several changes the branch also
made. The resolutions keep both sides:

- **`0e54c236` (was 460d89a4).** `generate_default_schedule` now takes
  `&DefaultProfiles` and returns a Result. That is the head's
  missing-profile errors combined with the branch's removal of the building
  param and the setpoint columns.
  - Setpoint defaulting uses the branch's OS-HPXML 68/78 F with a warning.
    The head's `DefaultProfiles::is_empty` is removed.
  - `hpxml_schedule_tests.rs` was rewritten as a merge of both.
  - The `schedule_path_contract.rs` digest is re-pinned to `a30b9593...`,
    with the comment explaining both moves.
- **Finding A (fixed in `0e54c236`).** The head's duplicate
  `IndirectTank`/`TanklessWaterHeater` stubs, with optional `zone_id`, are
  deleted. The branch's pair is kept: required keyword `zone_id`, with
  docstrings, which matches the pyo3 signatures in `py_water_heater.rs:540,681`.
  pyright has NOT been run yet.
- **`71d55df9` (was bca0651a).** The conditioned volume is read lazily, with
  the head's error message. The `solver_builder` test helper `zone_of_type`
  now gives the zone `volume_m3: Some(100.0)`.
- **`49099c02` (was cb21d63a).**
  - `conditioned_volume_m3` is an f64 (branch) and `ceiling_height_m` is an
    f64 (head). Every Building literal uses `400.0` / `2.5`.
  - The head's test `infiltration_without_conditioned_volume_is_an_error` is
    deleted, since it cannot be expressed now.
  - The autosize occupancy-only test uses `conditioned_volume_m3 = 0.0`.
- **`aa622046` (was 644a5b67).** `infiltration_heights_without_their_inputs_error`
  now expects the parse to fail on a missing floor count.
- **`6b036ad2` (was 18195aea), reworded.** The head's parser already rejects
  an unknown SiteType, so this commit only cites `defaults.rb` 817-818 and
  pins the `None -> suburban` case.
- **`a10d1a4e` (was 70fa06e2), reworded.** The parse errors quote the file's
  own text.
- **`45e52830` (was 398097d2), reworded.** The commit message no longer says
  "ratchets", and "triaged under OD-56" is gone from D-010.
  - The head's typed `ShieldingOfHome` enum is kept. The parser now reads the
    schema spelling `ShieldingofHome`.
  - `shielding_to_class(Option<&ShieldingOfHome>, facility_type)` carries the
    facility-type default.
  - `envelope_input_warnings` reports a missing SiteType or shielding, and
    D-008 says "with a warning in the dwelling's log".
  - Test XML in `hpxml/mod.rs` and `validation.rs` and the head test
    `unknown_shielding_or_site_type_is_a_parse_error` use `ShieldingofHome`.
- **`e3abbe70` (was c8c6245c).** `building_ceiling_height_m` validates that
  the f64 is finite and positive. The synthetic building states its ceiling
  as volume over floor area.

Last test run, after `e3abbe70`: hares-core, hares-io and hares-envelope all
pass (3245/3245). The full workspace passed at `45e52830` (7347/7347).
Clippy and fmt have not been run on the rebased tree.

## Findings

- **A: done, test pending.** Fixed in `0e54c236`, in
  `python/ochre_next/_hares.pyi`. pyright and maturin are not yet run.
- **B (gable guard): not started.** `crates/hares-io/src/hpxml/zone_geometry.rs`,
  `end_area_m2 = gable_area_m2 / 2.0` (about line 355) and the window at
  385-395. The plan:
  1. Count the outside gable walls.
  2. With one wall, the end is its full area.
  3. With two, require ends of equal size.
  4. Tighten `GABLE_END_RATIO_MIN`/max. Alternatively, take the span from
     the party-wall geometry.

  Cite OS-HPXML v1.12.0 `geometry.rb`, not the vendored dev copy.

  Test on `vendors/OCHRE/test/OS-HPXML Sample Files/base-bldgtype-attached.xml`
  and on its `-2stories` and `-infil-compartmentalization-test` variants:
  - span 36.7 ft;
  - attic about 117 m3 (the current code gives 78.02 m3);
  - rise 9.18 ft.

  Re-scan all 425 committed HPXML files. Per the review: 29 gable accepted,
  12 hip fallback, 1 with no gable walls. Fix D-009's sentence "every gable
  ... 0.9993 to 1.0008". Then run a mutation: revert the fix and show the
  test fails.
- **C (parity residuals): not started, no evidence gathered.**
  - `cz6b_resistance_res_wh` is about 22 %, and `cz4a_ashp_hpwh` about 22.6 %.
  - The suspected cz4a cause: OCHRE fork `vendors/OCHRE/ochre/Equipment/WaterHeater.py:456`
    reads `self.hp_cop` before `:476` sets it, so the fixture cannot
    regenerate with the pinned start 18.083026715940175 C.
  - Fix it as a local commit on submodule branch `handoff/or18c-ochre`.
    Do not push. Publishing the fork commit is the operator's decision.
  - Then pin `[ochre] initial_temp_setpoint_c` in
    `tests/fixtures/parity/cz4a_ashp_hpwh/config.toml`, and remove the
    unpinned note there and in `tests/fixtures/parity/README.md`.
  - Seed numpy in `tests/python/generate_parity_reference.py`, and regenerate
    twice to show identical output.
  - Triage cz6b_resistance by end use and step, classes (a) to (d).
  - Set each fixture's band in `tests/parity/mod.rs` `fixture_override` to
    its proven residual. A class (a) deviation also gets a DIVERGENCES entry
    and a pinning test.
  - Update ledger OD-61.
  - Mutation: revert the fix and show a test fails.
- **D (minisplit orphan ducts): not started.**
  `tests/fixtures/parity/cz5a_minisplit_gas_wh/building.xml:788-835`.
  1. Remove the `AirDistributionSystem` that no HVAC references.
  2. Regenerate the reference.
  3. Drop the 34.6 / 33.3 overrides and the class (a) comment in
     `tests/parity/mod.rs` (lines 47-66 at de561181), or set them to the new
     residual.
  4. Update OD-61 text. No DIVERGENCES entry currently rests on it; grep
     found none.
- **Lows: not started.**
  - OD-53 text: `ledger/11-other-defects.md:2845-2867`. Use "EnergyPlus
    dataset records" and "OS-HPXML's default material", not "Engineering
    Reference" or "Rough".
  - The 398097d2 reword is done in `45e52830`.
  - Remove the `T-0075` IDs from `tests/bestest/mod.rs` and rename the
    ratchet wording to drift baseline. `tests/parity/tolerance.rs` also
    carries `T-0075` and `T-0123`.
  - `crates/hares-core/tests/orchestration_parity.rs:223` still says "7x
    interior mass multiplier".

## Remaining steps in order

1. Cherry-pick the eight commits above, resolving conflicts. Expect
   `tests/parity/mod.rs` and possibly `DIVERGENCES.md`. Check that each
   commit compiles.
2. Autosquash `e55739e0` into `442e404d`. Then check every commit with
   `git rebase --exec 'cargo check --workspace --all-targets' db5efe80`.
3. Make the B, C, D and low fixes as separate commits, with tests and
   mutations.
4. Re-capture the seven goldens on the final code with `git_dirty` false,
   one commit each, explaining every delta.
5. Run `git range-diff 2f85338d..de561181 db5efe80..HEAD`, plus a line-level
   check of every file both sides touched.
6. Run the full plan.md §3 block, including:
   - clippy, all lines;
   - pytest `-m ""` with an empty skip census;
   - pyright with 0 errors;
   - `[stray]`: list every leaked TMPDIR path. Head tests leak
     `hares-regr-output-*.csv`, `hares-test-batchstep-*.toml`,
     `hares-core-engine-*.csv`, `hares-autosized-boiler-indirect-tank.xml`
     and `hares-unwired-indirect-tank.xml`. Show that this branch adds none.
7. Move `fix-or18c` with `git branch -f fix-or18c HEAD` from a detached HEAD
   or the handoff branch. Never push.

## Traps

- The sccache client fails with "path must be shorter than SUN_LEN" when
  TMPDIR is the long scratch path. Start the server first:
  `TMPDIR=/tmp/claude-1000 sccache --start-server`.
- The scratchpad directory is shared with the parent session and was wiped
  once mid-run. Keep nothing important there.
- The head made `ceiling_height_m`, `floors_above_grade` and the site enums
  required or typed. The branch commits assume Option types and string
  shielding. Auto-merged code can compile-fail several commits later; `zip`
  on an f64 is one example. Compile after every pick.
- The head reads the misspelt `ShieldingOfHome`. The schema is
  `ShieldingofHome` (HPXML.xsd 4479). Keep the schema spelling.
- The head deliberately removed tracing warnings for a missing SiteType.
  Warnings now go to the dwelling log through `envelope_input_warnings`
  instead.
- OCHRE reads `HasFlueOrChimneyInConditionedSpace` from the AirInfiltration
  extension (`hpxml.py:510`), so the head's BuildingConstruction rename in
  the parity fixtures does not change OCHRE references.
- Parity reference regeneration overwrites
  `tests/fixtures/parity/*/reference_output.parquet`.

## Unverified

- pyright, clippy, fmt, the pytest slow and ochre groups, and the goldens on
  the rebased tree.
- Per-commit compile of the rebased commits. Only the tips at each conflict
  were checked.
- Everything in B, C and D.
