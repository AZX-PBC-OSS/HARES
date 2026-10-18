# Verification round L5-b

Scope: `plan.md` §2.1 to §2.3, §2.5 to §2.7; `ledger/03-output-reporting.md`
every entry not committed, with a dedicated pass over OR-18, plus OR-19
(added to scope after this round started); `ledger/05-python-surface.md`;
`ledger/12-inputs.md`. OR-11 (committed `b8195c83`) checked for its
**State** paragraph only. Order taken from `ledger/00-schedule.md`, rules
from `plan.md` §3. Checked against HEAD `7b7a6a7d` in the clean worktree
`/home/rich/src/HARES/.workstrees/audit-7b7a6a7d`; ledger and plan text
read from `/home/rich/src/HARES/.shipwright/initiatives/I-08/` (gitignored,
not present in the worktree). The OpenStudio-HPXML citations in OR-18 were
checked against `github.com/NREL/OpenStudio-HPXML` tag `v1.12.0`,
`HPXMLtoOpenStudio/resources/geometry.rb` and `defaults.rb`, fetched
directly. OR-19's redteam source (`verification/redteam-OR-11-SL-7.md`)
was read; its reproduction worktree was not needed since the code
citations were checked directly against `7b7a6a7d`.

Blockers: 2. Majors: 5. Minors: 19.

## Blocker findings

1. **`ledger/12-inputs.md` IN-2, stale citation anchoring the
   assembly/warm-up split.** The entry cites
   `crates/hares-core/src/dwelling/mod.rs:2740, run_warmup_converged(0.5, 25)`
   as the warm-up call inside `build()`. At `7b7a6a7d` line 2740 is
   unrelated output-path code (`default_output_path`); the actual
   `run_warmup_converged(0.5, 25)` call site is line 3000, and the
   function's definition is line 5360. Correction: re-pin to `:3000` (call
   site) and `:5360` (definition). The design itself (separating assembly
   from warm-up, `ResolvedClock`) is sound and its dependency ordering is
   correct; only the anchor is wrong, but it is wrong enough that an
   implementer following it lands on code that does not exist in the form
   described.

2. **`ledger/12-inputs.md` IN-5, three of four function citations in
   `schedule_resolve.rs` are wrong, all shifted in the same direction.**
   - Cited `:655-693` for `inject_setpoint_schedules`; actual definition
     at line 701 (through ~740).
   - Cited `:695-713` for `set_typed_setpoint_source`; actual definition
     at line 741 (through ~789).
   - Cited `:743-790` for `inject_water_heater_schedule_columns`; actual
     definition at line 790; the cited range ends exactly where the real
     function starts.
   - The column-table citation `:176-190` is also off:
     `heating_setpoint`/`cooling_setpoint` are at 186/191,
     `water_heater_setpoint` at 196.
   This is the entry that resolves thermostat setpoint source by the
   OS-HPXML rule, a load-bearing physics change; all three function
   anchors must be corrected before implementation starts. `thermostat.rs:269-302`
   is correct. The OS-HPXML evidence (commit `57bd1b88`, `hvac.rb`,
   `schedules.rb`, a `test_validation.rb` case) was not independently
   re-fetched in this pass (out of this round's OS-HPXML verification
   scope, which was OR-18's rule) but is internally consistent with the
   rest of the entry and with IN-8's downstream Fahrenheit fix.

## Major findings

3. **`ledger/03-output-reporting.md` OR-18, the cited OpenStudio-HPXML
   evidence for "other exterior / outside" does not support the claim.**
   The entry's "Verified OS-HPXML rule" table states: "`defaults.rb:3065-3067`
   (verified below), not a scheduled-space average" as the citation for
   OS-HPXML treating `OtherExterior` the same as `Outside` (pure outdoor
   ambient, no averaging). Fetched `defaults.rb` at tag `v1.12.0`: lines
   3065-3067 are inside `apply_hvac_location`, accumulating supply-duct
   surface area by location for HVAC distribution-system location
   defaulting; entirely unrelated to ambient-temperature sourcing for
   water heaters. The claim is true, but the actual supporting code is
   `geometry.rb:1675-1678` (`get_space_or_schedule_from_location`), which
   returns `nil` (no space, no schedule, pure outdoor) for exactly
   `[HPXML::LocationOtherExterior, HPXML::LocationOutside, HPXML::LocationRoofDeck]`.
   Every other part of the table (indoor/outdoor/ground weights and the
   SI-converted floors for "other heated space", "other multifamily
   buffer space", "other non-freezing space", "other housing unit") was
   verified byte-for-byte against `geometry.rb:1399-1428`
   (`get_temperature_scheduled_space_values`) and is exactly right,
   including the °F→°C floor conversions (20.0, 10.0, 4.44 °C). Correction:
   replace the `defaults.rb:3065-3067` citation with `geometry.rb:1675-1678`.
   This does not change the physics design, which is correct; it is a
   citation-accuracy defect in a commit (commit 2) that explicitly adds
   physics no golden can prove, so the citation is the only thing standing
   behind the claim's correctness, and standing behind the wrong lines.

4. **`ledger/00-schedule.md`, duplicate row number `21a` used for two
   different entries.** Row `21a` names `OR-18` (`ledger/03-output-reporting.md`,
   "the three census rows OR-11 left as logs"). A second row, also
   labelled `21a`, names `RT-3a` ("one shared-term resolve per step for
   the ideal-capacity solves ... added 2026-10-02"), placed after `OR-19`'s
   row `21b`. The schedule's own stated rule is that "a row keeps its
   number when it moves, so references to it stay valid"; this requires
   every row number to be unique, which this is not. `RT-3a` was added
   after `OR-18` and `OR-19` already held `21a`/`21b`; it collides with
   `OR-18` instead of continuing the sequence (`21c`). Correction: renumber
   `RT-3a` to `21c`. This is a schedule-integrity defect, not a design
   defect in either entry; `RT-3a` belongs to `ledger/06-runtime-memory.md`,
   outside this round's primary scope, but the collision is visible in
   `00-schedule.md`, which this round is instructed to use for order.

5. **`ledger/03-output-reporting.md` and `ledger/05-python-surface.md`,
   `py_dwelling.rs` citations are stale across both files for the same
   reason: the file shifted after commits already landed on top of the
   citations' pin.** `plan.md` §1 requires evidence into a PR #27 file
   (`py_dwelling.rs` named explicitly) to be "re-verified at the Task 0
   head" before its entry starts; that re-verification was never done.
   PR #27 shifted everything past roughly line 700 by about +41 lines at
   the Task 0 head (`1d60f478`), and the profiler rows (14-18,
   `d7f0e381`/`21d90588`/`26626710`/`1d471677`/`afa9a071`) and `39488581`
   shifted it further, by roughly +50 to +60 lines past line 1000, so most
   citations now land 40 to 60 lines off, and several land on unrelated
   code entirely. Confirmed sites, `ledger/03-output-reporting.md`:
   OR-3 (`metrics()` raise, `rotation` keyword), OR-6 (`finalize_billing`),
   OR-7 (three naive-`datetime` sites), OR-8 (`metrics()`), OR-9
   (initialise/clear sites), OR-10 (`load_state`). Confirmed sites,
   `ledger/05-python-surface.md`: PY-1 (`simulate()`, `results()`,
   `metrics()`), PY-3 (`reset_with_seed`), PY-9 (`results()`, `metrics()`,
   `finalize_billing`, `dwelling_fatal`). None of these are design defects:
   the underlying claims (IPC clone, GIL-held construction, naive
   `datetime` production, metrics gating) are all still true at `7b7a6a7d`,
   but an implementer following any of these citations literally lands
   on unrelated code (for example OR-3's cited `py_dwelling.rs:1494-1497`
   is `has_equipment_lut` at the Task 0 head, not the `metrics()` raise).
   Correction: re-sweep every `py_dwelling.rs` citation in both files
   against the Task 0 head or against `7b7a6a7d` with the commit named, as
   `plan.md` §1 requires for this file; or add explicit "at `<sha>`"
   qualifiers the way `05-python-surface.md` PY-2 and
   `03-output-reporting.md` OR-13 already do. The same staleness pattern,
   to a lesser degree, affects `dwelling/mod.rs` and `equipment.rs`
   citations throughout `03-output-reporting.md` (OR-1, OR-4, OR-6, OR-10,
   OR-16), but those are correctly pinned to `ebdb1a17` by plan.md §1's
   general rule and were individually confirmed correct there; they are
   listed as minors below because the convention covers them, unlike the
   `py_dwelling.rs` sites, which plan.md §1 specifically calls out for
   re-verification that did not happen.

6. **`ledger/12-inputs.md` IN-4, citation points at test-only code, not
   the production path it claims to describe.** "Record index and
   interpolation at `environment.rs:529-536,811-849`"; lines 529-536
   match the weather lookup inside `update_in_place` correctly, but lines
   811-849 are inside `#[cfg(test)] fn compute_schedule_offset`, which is
   schedule-offset test scaffolding, not weather interpolation. This is
   not a stale-line issue; it names the wrong function in the wrong
   category (test vs. production). Correction: replace with the real
   interpolation site inside `update()` (defined at line 758) or the
   PCHIP logic the `weather_step_59_uses_pchip_interpolation` test
   exercises near line 1566. `site_location.rs:100` in the same entry is
   exact.

7. **`ledger/12-inputs.md` IN-6, the four cited use-site line numbers for
   `resample_overrides` in `dwelling/mod.rs` match nothing in the
   worktree.** Cited `:1813, 1896, 1952, 12295`; actual occurrences at
   `7b7a6a7d` are lines 2022, 2105, 2196 and 12991 (the field declaration
   itself, cited "221-222", is close at 223). None of the four cited
   numbers land on any real occurrence of the name. The "189 lines in 39
   files at `ebdb1a17`" census is pinned to a different commit and was not
   independently recomputed in this pass. Correction: regenerate the
   `dwelling/mod.rs` site list against `7b7a6a7d` or whichever commit the
   entry starts from. The design (OCHRE resampling logic deleted from
   `hares-io`/`hares-core`, moved to `python/hares/adapters/ochre.py`,
   with `REMOVED_KEYS` giving a loud error for the deleted keyword) is
   correct and matches the no-legacy-shim rule.

## Minor findings

8. **`ledger/03-output-reporting.md` OR-19, one acceptance test likely
   already passes before the change.**
   `post_step_conflicting_fluid_declaration_is_rejected` is listed as a
   test that "must fail against `7b7a6a7d` before the change." Reading
   `ensure_ports_satisfied` (`mod.rs:4090-4139`) and its callers inside
   `validate_candidate_equipment`/`validate_replacement_equipment`
   (`mod.rs:3732-3734,3774-3776`): once `clock.current_step > 0`, a
   candidate's `Fluid` port declaration is already checked against the
   frozen table by exact `(loop_id, fluid_type, node_id)` match, before
   the candidate is pushed. A candidate declaring a running loop with a
   different fluid type than the frozen table's existing accumulator
   already fails this match and is already rejected pre-push today, so
   the roster is already left unchanged on this path. This test may not
   actually distinguish pre-change from post-change behaviour, which
   would violate the stated "fails before the change" requirement for one
   of the five acceptance tests. This does not affect the correctness of
   OR-19's design, which otherwise plans every fallible derivation
   (including the output schema, name map, loop-type map and thermal
   consistency wiring) before any mutation, correctly closing both
   redteam blockers B1 and B2 and the architectural concern raised in
   redteam Minor M1. Recommend confirming this test actually fails at
   `7b7a6a7d` before relying on it, or replacing it with a case that
   genuinely only fails pre-change (for example the identical scenario
   before the first step, which is already covered by
   `removed_loop_declarer_frees_its_fluid_type_before_first_step`).

9. **`ledger/03-output-reporting.md` OR-19, roster-entrance coverage
   confirmed exhaustive; no gap found.** Checked explicitly because this
   round's scope asked for it: `grep -rn "refresh_equipment_caches\b"`
   across the workspace finds exactly the seven call sites OR-19 lists
   (`add_actor`, `auto_register_actors`, `add_equipment`, `clear_equipment`,
   `remove_equipment`, `remove_equipment_by_end_use`, `replace_equipment`),
   no others. `Dwelling::add_actor_by_name` (`mod.rs:3387-3394`, which the
   Python binding `py_dwelling.rs:907-924` calls) delegates to `self.add_actor`,
   so it is covered transitively. The Python blueprint-level
   `PyDwellingBlueprint::add_equipment`/`remove_equipment`/`remove_equipment_by_end_use`
   (`py_blueprint.rs:62-98`) and the underlying `DwellingBlueprint` methods
   (`blueprint.rs:194-233`) mutate a plain `Vec<EquipmentSpec>` before any
   `Dwelling` exists; no port table, no fluid-loop map, no output schema
   has been built yet, so the half-rebuilt-cache risk OR-19 fixes does not
   exist on that path, and its silence on it is correct, not a gap.
   Checkpoint restore (`checkpoint.rs`) restores per-equipment state into
   an already-matching live roster by name/id; it does not add, remove or
   replace equipment, so it is not a roster entrance either. All line
   citations in OR-19's Problem and Change sections
   (`mod.rs:3678-3714,3901-4036,4155-4244`,
   `fluid_solver.rs:209-216`) were checked and are exact at `7b7a6a7d`.

10. **`ledger/03-output-reporting.md` OR-1.** `equipment.rs:1921` for
    `EquipmentDescriptor.telemetry_fields` is line 1920 at `ebdb1a17` and
    1868 at `7b7a6a7d` (the same off-by-one recurs in `plan.md` §2.2 at
    its own line 467). `NoStateActor` cited as `ev_driver/mod.rs:6303` is
    6302 at `ebdb1a17`. Both trivial, both one line off.

11. **`ledger/03-output-reporting.md` OR-5 (committed `dc033cee`,
    `469ea955`), the entry has no `**State:**` paragraph.** Unlike OR-11
    (which has one at line 1447) and every not-started entry, OR-5's
    as-committed facts appear only inline in its Change list, and its
    `**Commit:**` line names only the first of its two commits. Correction:
    add a `**State:** committed dc033cee, 469ea955` paragraph naming both
    messages, matching the schedule's row 4. Separately, informational
    only: `dc033cee`'s own commit message says the clipped-action statics
    are deleted; they are still live at `py_gym.rs:208,212` at `7b7a6a7d`.
    The ledger's inline note is correct and OD-18 (row 30b) already owns
    the fix; this is not a defect in the ledger, only a note that the
    shipped commit message overstated what it did.

12. **`ledger/03-output-reporting.md` OR-6.** Reuses OR-4's
    `mod.rs:2805-2830` preamble citation, correct only at `ebdb1a17`
    (3097-3114 at `7b7a6a7d`); covered by finding 5's general
    `dwelling/mod.rs` pattern, listed separately because it is a citation
    OR-6 restates rather than owns.

13. **`ledger/03-output-reporting.md` OR-7.** `columns.rs:915-927` is
    911-923 at `7b7a6a7d` (4-line drift, within the general pattern).

14. **`ledger/03-output-reporting.md` OR-10.** `mod.rs:1517,1518,4575,4645-4814,4822`
    are correct at `ebdb1a17` and roughly 360 lines off at `7b7a6a7d`,
    consistent with the general `dwelling/mod.rs` drift pattern (finding 5);
    not a defect in the citation convention, which pins to `ebdb1a17`, but
    worth re-pinning before this row starts given how far the file has
    moved.

15. **`ledger/03-output-reporting.md` OR-16, a citation wrong even at its
    own explicitly named commit.** "`Dwelling::set_tariff` (`mod.rs:4259`
    at HEAD `21d90588`)"; at `21d90588` itself, `set_tariff` is at line
    4249, not 4259 (4475 at `7b7a6a7d`). This is the one citation in this
    round that names its own pin commit and is still wrong there, distinct
    from the general "correct at `ebdb1a17`, stale at `7b7a6a7d`" pattern.

16. **`ledger/03-output-reporting.md` OR-11 (out of this round's primary
    scope, State paragraph already checked clean against the audit file
    and `b8195c83`; noted in passing by one of the delegated passes).**
    `DHW_DEMAND_LOOP` is cited as `water_heater/mod.rs:128`; the constant
    is at line 79 in `b8195c83`. Belongs to the area another verification
    round owns; recorded here only because it surfaced during this round's
    work.

17. **`ledger/05-python-surface.md` PY-7.** `pyproject.toml:77` for
    `module-name = "ochre_next._hares"` is line 81 at `7b7a6a7d` (`name =
    "ochre_next"` at line 6 is correct).

18. **`ledger/05-python-surface.md` PY-8.** `PySimulationConfig` struct
    citation `py_config.rs:45-548` actually starts at line 47;
    `PyDwellingConfig` cited at `py_config.rs:550` is actually line 552.
    All other PY-8 citations in this file (`write_output: true` at 201,
    `retain_batches: true` at 211, `to_sim_config` at 469, `from_sim_config`
    at 518) are exact.

19. **`ledger/05-python-surface.md` PY-3.** `py_blueprint.rs:45` for
    `DwellingBlueprint.from_hpxml` is actually line 37 (8-line drift);
    `py_blueprint.rs:102-115` for `build` is exact.

20. **`ledger/05-python-surface.md` PY-11.** `py_actor.rs:49` for the
    `PyActor` struct is actually line 50 (trivial).

21. **`ledger/12-inputs.md` IN-7.** The claim "walls, roofs, floors and
    doors carry no insulation description, `building.rs:2215`" does not
    correspond to anything at that line (unrelated material-layer
    specific-heat validation code), and a repository-wide search for
    "InsulationDescription"/"insulation description" in the HPXML module
    returns nothing. This reads as a description of the HPXML schema
    itself (these element types lack an `InsulationDescription` child)
    rather than of HARES code, so the file:line citation is misattached.
    Correction: remove the citation or re-anchor it to wherever the
    parser's non-handling of this absent field is actually observable.
    The entry's other citations (`envelope_lut.rs` `lookup` and
    nearest-R logic) are accurate within a small, consistent comment-line
    offset.

22. **`ledger/12-inputs.md` IN-3, IN-9, trivial line offsets.** IN-3's
    `validation.rs` required-paths table is cited `:255-281`, actually
    `258-281`. IN-9's `resolve_der.rs:182-210` for `resolve_ev` is
    actually line 177 (3-line offset). Both within normal drift tolerance
    and do not misdirect an implementer to different code.

## Clean (citations and design verified, no issues)

`ledger/03-output-reporting.md`: OR-2, OR-4 (beyond the shared preamble
citation already counted under OR-6), OR-12, OR-13, OR-14, OR-15, OR-17.
`ledger/05-python-surface.md`: PY-2 (citations verified exact at its
explicitly named historical commit `872cf289`), PY-5, PY-6, PY-10, PY-4,
PY-12 (including the silent-default weight-lookup bug it targets, `weights.get(bid, 1.0)`,
confirmed present as described). `ledger/12-inputs.md`: IN-1, IN-8.

OR-11's **State** paragraph (committed `b8195c83`, "not done", naming the
three census rows left as debug-only logs and pointing to OR-18 as their
fix) was checked against `verification/audit-OR-11-b8195c83.md` and the
commit itself and is accurate.

## Cross-cutting checks that passed

- **Dependency ordering:** every entry's dependencies land on an earlier
  `00-schedule.md` row than the entry itself, across all three ledger
  files, including every `IN-*` row checked individually (IN-1 through
  IN-9 against RT-4, RT-2, RT-6, OR-1, OR-6, PY-9, PY-4, PY-7, PY-8, OD-2,
  OD-9, OD-10, OD-17, OD-19, B-1a, B-14) and every `PY-*` row (PY-7, PY-5,
  PY-12, PY-8, PY-2, OR-1 through OR-6, PY-9, PY-1, PY-3, PY-4, PY-6,
  PY-10, PY-11 against their row numbers). OR-19's sole dependency (OR-11)
  is an earlier row; it does not need to depend on OR-18, since the two
  entries' code changes are disjoint (OR-18 touches `water_heater/*.rs`,
  `hvac/dehumidifier.rs`, `hvac/helpers.rs`, `synthetic.rs` and
  `hvac/ideal_hvac.rs`'s end-use assignment; OR-19 touches the roster
  entrances and port-slot/fluid-loop-map derivation in `dwelling/mod.rs`).
- **SI units, no silent defaults, local time only, no legacy shims, DRY:**
  no entry in any of the three files violates these. IN-6 is the entry
  that removes OCHRE compatibility logic from the Rust core rather than
  duplicating it; PY-8 deletes the hand-rolled Python mirror structs
  rather than keeping two config types; OR-3 removes the silent default
  output path; OR-18 and OR-19 both raise typed errors instead of
  silently substituting a wrong zone, a wrong fluid type or a half-mutated
  roster.
- **Acceptance-test determinism and "fails before the change":** no named
  test from any entry checked in this round (OR-1 through OR-17 excepting
  the committed OR-5 and OR-11, OR-18, OR-19, every PY-* entry, every IN-*
  entry) already exists in the worktree, and none depends on wall-clock
  timing or an unseeded RNG. The one exception is OR-19's
  `post_step_conflicting_fluid_declaration_is_rejected` (finding 8), which
  may already pass today for reasons unrelated to the fix.
  `tests/python/test_py_config.py` already exists but correctly contains
  none of PY-8's newly named test functions, so PY-8 adds to it rather
  than duplicating it.
  `vendors/OCHRE/test/OS-HPXML Sample Files/base-bldgtype-multifamily-adjacent-to-other-heated-space.xml`
  and the other four OS-HPXML sample files OR-18's acceptance tests cite
  are confirmed vendored in the repository.
- **Writing rules:** no em dashes and no reference to an agent, LLM,
  Claude, subagent, review round or reviewer were found in
  `03-output-reporting.md`, `05-python-surface.md` or `12-inputs.md`
  (entry-ID self-references such as "OR-18", "PY-8", "IN-5" are the
  permitted carve-out plan.md §3 states, and are the only such tokens
  present). `HARES#28` appears only inside OR-3's quoted commit message,
  which the Writing rule allows.
- **Verification blocks:** every entry cites `plan.md` §3's block labels
  correctly ([golden], [clippy], etc.), and OR-18's four-commit split
  correctly scopes rule 1 ("no regressions") to commits 1, 3 and 4 and
  rule 2 (physics evidence standard) to commit 2, which is the one commit
  that is not bitwise-neutral by its own stated reasoning.
- **OR-18 physics content** (the only entry this round fetched external
  source for): every weight, floor and its °F→°C conversion in the
  "Verified OS-HPXML rule" table matches `geometry.rb`'s
  `get_temperature_scheduled_space_values` exactly (finding 3 covers the
  one wrong citation inside this otherwise-correct table). The six
  synthetic-fixture names in commit 3
  (`window_area_deducted_from_host_wall`,
  `zero_conductivity_with_positive_thickness_returned_as_error`,
  `zero_density_with_positive_thickness_succeeds`,
  `zero_specific_heat_with_positive_thickness_succeeds`,
  `negative_density_returned_as_error`, `SYNTHETIC_RC_DWELLING_TOML`) all
  exist at the cited files. `EnvironmentManager::update`/`update_in_place`
  line citations (`environment.rs:510,758`) for commit 2's per-step hook
  are exact, and the proposed computation (three scalar multiplies and a
  `.max()`, no new allocation) matches the claim of no hot-path
  allocation. The `conditioned_zone_id`/`zone_sort_key`/`parse_zone_label`/
  `zone_id_for_location` citations for commit 1 and commit 2's zone
  resolution are all exact.
