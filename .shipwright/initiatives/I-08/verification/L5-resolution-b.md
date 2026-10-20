# L5-b resolution

Scope: every finding of `verification/verify-L5-b.md` except the
`00-schedule.md` duplicate-row-number finding (owned by another resolver)
and the OR-11 census table's disposition cells (owned by another
resolver; OR-11's other citations were in scope). Every corrected
citation below was checked against the committed HEAD `7b7a6a7d` in
`/home/rich/src/HARES/.workstrees/audit-7b7a6a7d` by grep and direct read,
except OR-18's OS-HPXML citation, checked by fetching
`github.com/NREL/OpenStudio-HPXML` tag `v1.12.0`. No ledger file outside
`ledger/03-output-reporting.md`, `ledger/05-python-surface.md` and
`ledger/12-inputs.md` was edited. No repository code was touched.

## Blockers

1. **`ledger/12-inputs.md` IN-2, stale warm-up citation.** Confirmed at
   `7b7a6a7d`: line 2740 is `default_output_path`, unrelated; the
   `run_warmup_converged(0.5, 25)` call site is line 3000 and its
   definition is line 5360. Changed the citation from `:2740` to `:3000`
   and added the definition site `:5360`. The entry's design claim
   (assembly/warm-up split, `ResolvedClock` dependency ordering) still
   holds; only the anchor was wrong.

2. **`ledger/12-inputs.md` IN-5, three `schedule_resolve.rs` function
   citations.** Confirmed at `7b7a6a7d`: `inject_setpoint_schedules` is
   defined at line 701 (through 740, not 655-693); `set_typed_setpoint_source`
   at line 741 (through 789, not 695-713); `inject_water_heater_schedule_columns`
   at line 790 (through 837, not 743-790 -- the old citation's end is
   exactly where the real function starts). Fixed both occurrences of each
   in the Evidence and Change sections. The column-table citation `:176-190`
   is also wrong: `heating_setpoint` is at 186 only within a block that
   runs 184-199 (`cooling_setpoint` 191, `water_heater_setpoint` 196);
   changed to `:184-199`, and the separate `water_heater_setpoint`-only
   citation (`:186-189`) to its exact line, `:196`. `thermostat.rs:269-302`
   was confirmed correct and left unchanged. The rule itself (schedule
   column wins, else HPXML, else default; OS-HPXML precedence match) still
   holds; only the anchors were wrong. The same stale `:176-190` citation
   also appears, unflagged, in IN-8's Problem paragraph; corrected it to
   `:184-199` too, since it is the identical error.

## Majors

3. **`ledger/03-output-reporting.md` OR-18, OS-HPXML citation for "other
   exterior / outside".** Fetched `defaults.rb` and `geometry.rb` at tag
   `v1.12.0`. `defaults.rb:3065-3067` is inside `apply_hvac_location`,
   accumulating supply-duct area by location for HVAC distribution
   defaulting, unrelated to water-heater ambient sourcing. The real
   support is `geometry.rb:1675-1678`, inside
   `get_space_or_schedule_from_location`:
   ```
   return if [HPXML::LocationOtherExterior,
              HPXML::LocationOutside,
              HPXML::LocationRoofDeck].include? location
   ```
   which returns `nil` (no space, no schedule) for exactly those three
   locations, confirming "other exterior" and "outside" both resolve to
   pure outdoor ambient with no averaging. Replaced the citation in both
   places it appears (the Evidence paragraph and the rule table's last
   row) and added the function name and the three-constant list to the
   table row so the claim is self-supporting. The physics claim itself is
   correct; only the citation was wrong.

4. **`ledger/00-schedule.md` duplicate row `21a`.** Out of scope for this
   pass; owned by another resolver. Not touched.

5. **`py_dwelling.rs` citations stale across `03-output-reporting.md` and
   `05-python-surface.md`.** Re-swept every confirmed-stale site against
   `7b7a6a7d`:
   - `03-output-reporting.md` OR-3: `metrics()` raise `:1494-1497` to
     `:1545-1551`; `rotation` keyword `:2164` to `:2221`.
   - OR-6: `finalize_billing` `:1827` to `:1882`.
   - OR-7: the three `chrono_to_py_datetime` call sites cited as
     `:808,1927,2131` are actually `:863,1984,2188` (the `step()`
     timestamp, `current_time` and the timesteps iterator).
   - OR-8: `metrics()` `:1490-1516` to `:1545-1577`.
   - OR-9: full resweep -- `initialize()` function `:661-676` to
     `:659-673`; field declared `:624` to `:622`; initialised `:652` and
     `:1954` to `:650` and `:2011`; tested `:662` to `:660`; written `:673`
     to `:671`; cleared `:1528` to `:1587`.
   - OR-10: `load_state` `:1541-1545` to `:1600-1603`.
   - `05-python-surface.md` PY-1: `simulate()`/`results()` clone sites
     `:743-746,1484` to `:797-798,1539-1540`; `metrics()` `:1490-1516` to
     `:1545-1577`.
   - PY-3: `from_hpxml` `:636-658` to `:634-656`, its `Dwelling::from_config`
     call `:645` to `:643`; `reset_with_seed` `:1518-1532` to `:1577-1590`,
     its `from_config` call `:1522` to `:1581`; `DwellingBlueprint.from_hpxml`
     (`py_blueprint.rs:45`) to `:37` (`DwellingBlueprint.build` at
     `py_blueprint.rs:102-115` was already correct).
   - PY-9: `results()` `:1482` to `:1537`; `metrics()` `:1490` to `:1545`;
     `finalize_billing()` `:1827` to `:1882`; `dwelling_fatal` `:996-999`
     at `21d90588` re-pinned to `7b7a6a7d` as `:999-1002` (the `21d90588`
     pin was itself correct, but current-code pins read better for an
     implementer).

   Also fixed under the same general `dwelling/mod.rs`/`equipment.rs`
   drift this finding names as covering OR-1, OR-4, OR-6, OR-10 and OR-16
   (listed as minors 10, 12, 14, 15 below) and PY-7, PY-8 and PY-11's own
   struct/module citations (`05-python-surface.md`, listed as minors 17,
   18, 20 below, plus PY-7's `pyproject.toml:77` to `:81`, flagged
   separately by the coordinator mid-pass and confirmed the same way).
   None of these changes the underlying claims (IPC clone, GIL-held
   construction, naive-`datetime` production, metrics gating, package
   rename, config mirroring); every claim still holds at `7b7a6a7d`.

6. **`ledger/12-inputs.md` IN-4, test-only citation.** `environment.rs:529-536`
   (the weather record index/lookup inside `update_in_place`) is correct
   and unchanged. `:811-849` is `#[cfg(test)] fn compute_schedule_offset`,
   schedule-offset test scaffolding, not weather interpolation; there is
   no separate named "interpolation" function in `environment.rs` outside
   that test and `update_in_place`'s lookup itself (actual PCHIP
   resampling happens once, at weather-file load, in
   `hares-io/src/weather.rs`, not per step). Replaced the citation with a
   pointer to `update()` at `:758`, the public per-step entry point that
   calls `update_in_place`. The design claim (one function factored out,
   reused by `resample_weather` and by B-2's warm-up day repeat) is
   unaffected.

7. **`ledger/12-inputs.md` IN-6, `resample_overrides` use sites.** Cited
   `:1813, 1896, 1952, 12295`; none match at `7b7a6a7d`. Actual occurrences:
   `:2022, 2105, 2196, 12991`. Field declaration and its doc comment: cited
   `:221-222`, actually `:220-223` (doc comment 220-222, field 223).
   Changed both. The design (OCHRE resampling deleted from the Rust core,
   moved to `python/hares/adapters/ochre.py`) is unaffected; only the
   census needed regenerating against the current commit.

## Minors

8. **`ledger/03-output-reporting.md` OR-19,
   `post_step_conflicting_fluid_declaration_is_rejected` may already
   pass pre-change.** Added a caveat to the acceptance test entry: confirm
   the test actually fails at `7b7a6a7d` before relying on it, since
   `ensure_ports_satisfied` and its callers already reject a conflicting
   candidate by exact `(loop_id, fluid_type, node_id)` match once
   `clock.current_step > 0`, independent of this fix; if it does not fail
   pre-change, `removed_loop_declarer_frees_its_fluid_type_before_first_step`
   already covers a genuinely pre-change-failing case. Did not delete or
   replace the test, since only the operator can confirm the run-time
   behaviour; the entry now states the risk instead of asserting it away.

9. **`ledger/03-output-reporting.md` OR-19, roster-entrance coverage.** No
   change: the finding confirmed the coverage exhaustive and found no gap.

10. **`ledger/03-output-reporting.md` OR-1.** `equipment.rs:1921` to
    `:1868`; `ev_driver/mod.rs:6303` to `:6337` (both re-pinned to
    `7b7a6a7d` rather than corrected at the `ebdb1a17` pin, per the
    preference for citations an implementer can read directly).

11. **`ledger/03-output-reporting.md` OR-5, missing State paragraph.**
    Added `**State:** committed \`dc033cee\`, \`469ea955\`.` after the
    Measurement line, naming both commits
    (`fix: record health and enforce physics invariants in every build
    profile`; `fix: gate the stage snapshot to the builds that write it
    and finish the doc sweep`), matching OR-5's schedule row and the style
    of OR-11's own State paragraph.

12. **`ledger/03-output-reporting.md` OR-6.** The restated OR-4 preamble
    citation `mod.rs:2805-2830` (correct only at `ebdb1a17`) changed to
    `:3097-3114` at `7b7a6a7d` (the `write_equipment_init` call and its
    `equipment_zones`/`ocv_sources`/`provenance_lines` builders). Also
    fixed the same citation at its source in OR-4's own Problem paragraph,
    since OR-4 is one of the entries the parent finding (5) names.

13. **`ledger/03-output-reporting.md` OR-7.** `columns.rs:915-927` to
    `:911-924` (the four `metadata.insert` calls:
    `hares_verbosity`, `hares_mode_map`, `basement_lighting_created`,
    `garage_lighting_created`).

14. **`ledger/03-output-reporting.md` OR-10.** Re-pinned every mod.rs
    citation to `7b7a6a7d`: `tariff_evaluator` field `:1517` to `:1704`;
    `billing_summaries()` `:1518` to `:4495`; `save_checkpoint` `:4575` to
    `:4935`; `load_checkpoint` `:4645-4814` to `:5005-5168`;
    `restore_building_state` `:4822` to `:5176`.

15. **`ledger/03-output-reporting.md` OR-16.** `set_tariff` was cited as
    `mod.rs:4259` "at HEAD `21d90588`" but is actually `:4249` there
    (confirmed by `git show 21d90588:...`); at `7b7a6a7d` it is `:4475`.
    Re-pinned to `7b7a6a7d:4475` and dropped the wrong `21d90588` label
    rather than correcting it to `:4249`, since the entry gains nothing
    from a historical pin here and the current line is unambiguous.

16. **`ledger/03-output-reporting.md` OR-11.** `DHW_DEMAND_LOOP` cited as
    `water_heater/mod.rs:128`; the constant is at line 79 at `7b7a6a7d`
    (and, per the finding, at line 79 in `b8195c83` too). Fixed the
    citation inside the Fluid-loop declarations prose; the census table's
    disposition cells were not touched.

17. **`ledger/05-python-surface.md` PY-7.** `pyproject.toml:77` to `:81`
    (`module-name = "ochre_next._hares"`; `name = "ochre_next"` at line 6
    was already correct and unchanged).

18. **`ledger/05-python-surface.md` PY-8.** `py_config.rs:45-548` to
    `:47-550` (the struct-to-impl-end span shifted by a uniform +2 lines);
    `py_config.rs:550` (`PyDwellingConfig`) to `:552`. Confirmed
    `write_output: true`/`retain_batches: true` at `:201`/`:211` and
    `to_sim_config`/`from_sim_config` at `:469`/`:518` are still exact at
    `7b7a6a7d` and left unchanged.

19. **`ledger/05-python-surface.md` PY-3.** `py_blueprint.rs:45` to `:37`
    for `DwellingBlueprint.from_hpxml`; `py_blueprint.rs:102-115` for
    `build` confirmed exact, left unchanged.

20. **`ledger/05-python-surface.md` PY-11.** `py_actor.rs:49` to `:50`.

21. **`ledger/12-inputs.md` IN-7.** "Walls, roofs, floors and doors carry
    no insulation description" was cited at `building.rs:2215` (unrelated
    material-layer specific-heat validation code at that line); a
    repository-wide search for `InsulationDescription` under
    `crates/hares-io/src/hpxml/` found nothing. This is a fact about the
    HPXML schema, not something observable at a HARES source line.
    Removed the file:line citation and reworded the sentence to name the
    HPXML elements directly (`Wall`, `Roof`, `Floor`, `Door`) rather than
    attach it to code that doesn't establish the claim. The entry's other
    citations (`envelope_lut.rs:507-603`, nearest-R at `:583-590`) were
    confirmed close enough (actual `lookup` is at line 510, within the
    finding's "small, consistent offset") and left unchanged.

22. **`ledger/12-inputs.md` IN-3, IN-9.** IN-3's `validation.rs` required-paths
    table cited `:255-281`; actual span at `7b7a6a7d` is `:259-282`.
    IN-9's `resolve_der.rs:182-210` for `resolve_ev`; actual definition
    line is `:177`. Both changed.

## Counts

Blockers resolved: 2 of 2.
Majors resolved: 4 of 5 (1 explicitly out of scope: the `00-schedule.md`
duplicate row, owned by another resolver).
Minors resolved: 19 of 19 (one, OR-19's roster-entrance coverage, required
no change; one, OR-19's acceptance-test risk, resolved by adding a caveat
rather than asserting a test result no one has run).
Findings touched outside the three owned files: none.
Findings touched inside OR-11's census table disposition cells: none.
