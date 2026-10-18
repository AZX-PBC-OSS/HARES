# L6 resolution: re-pin audit at 7b7a6a7d

Fixes every citation `verify-L6-a2.md` confirmed stale, plus the sweep its
instructions required: every citation of `crates/hares-core/src/dwelling/mod.rs`
(and bare `mod.rs:` meaning it), `dwelling/blueprint.rs`, `crates/hares-core/src/environment.rs`
and `crates/hares-envelope/src/thermal_solver/*.rs` in every ledger entry and
in `plan.md` that is not a committed row (`ledger/00-schedule.md`'s State
column). Committed rows' records are untouched. RT-3 (row 21, in progress) is
untouched; its stale lines are noted at the end, not fixed. All line numbers
below are at `7b7a6a7d` unless stated otherwise, checked by reading the code
at `/home/rich/src/HARES/.workstrees/audit-7b7a6a7d`.

## The 19 confirmed citations (ledger/03-output-reporting.md)

- OR-1, `build_equipment_column_map`: `mod.rs:376` to `mod.rs:540`.
- OR-1, `build_actor_column_map` (+ doc comment): `mod.rs:741-764` to `mod.rs:778-804`.
- OR-1, `record_step`'s core electric/gas/SOC/mode columns: `mod.rs:6636-6800`
  to `mod.rs:7165-7236`.
- OR-1, `zone_display_name` (was pinned "at afa9a071"): `mod.rs:7790` to
  `mod.rs:7980`, unpinned.
- OR-1/OR-2, `record_step`'s span and its `output_column_index` lookups:
  `mod.rs:6606-~7020` / `mod.rs:6618-6634` and `6825-6972` to
  `mod.rs:7165-7540` / `mod.rs:7169-7204` and `7347-7493`.
- OR-1/OR-2, `build_equipment_column_map` / `resolve_col`: `mod.rs:376` /
  `mod.rs:645-665` to `mod.rs:540` / `mod.rs:814`.
- OR-2, per-equipment string-keyed telemetry reads inside `record_step`:
  `mod.rs:6753-6791` to `mod.rs:7271-7307`.
- OR-3, the `write_output = false` schema-construction gate: `mod.rs:2476-2480`
  to `mod.rs:2747`.
- OR-3, `default_output_path`'s call site, and the function itself:
  `mod.rs:2474` to `mod.rs:2741`; `conversions.rs:590` to `conversions.rs:595`
  (the minor finding).
- OR-3/OR-6, `flush_and_close()`'s call site: `mod.rs:2869` to `mod.rs:3146`.
- OR-3, the diagnostics CSV writer gate (was pinned "at 872cf289", checked
  there and correct): `mod.rs:2819-2830` to `mod.rs:3055-3064`, unpinned.
- OR-3, `failed_run_leaves_no_partial_file`'s precedent: `mod.rs:13847` named
  no step()-returning-`Err` equipment at either baseline or head; rewritten to
  name the actual nearby precedent, `FailingPortEquipment` at `mod.rs:14548`.
- OR-6, `Dwelling::simulate()`: `mod.rs:2849-2877` to `mod.rs:3126-3154`.
- OR-6, `step()`'s unbounded-history return: `mod.rs:2966-2967` to `mod.rs:3244`.
- OR-6 (+ `plan.md` §2.1), `SimulationResults.steps`: `mod.rs:860-862` to
  `mod.rs:1034-1035`.
- OR-6, the step-history push and clear sites: `mod.rs:6583` / `5045,5064` to
  `mod.rs:7136` / `5481,5509`.
- OR-7/OR-13 (+ `plan.md` §2.5), the chrono `"%+"` write (OR-13's was pinned
  "at ebdb1a17", checked there and correct): `mod.rs:7009-7010` to `mod.rs:7532`,
  unpinned.

19 individual citations, all corrected.

## plan.md, beyond §2.1 and §2.5

- §2.1, `StepResult` (comment anchor): `mod.rs:846` to `mod.rs:1020`.
- §2.2, `zone_display_name` (duplicate of OR-1's, was pinned "at afa9a071",
  correct there): `mod.rs:7790` to `mod.rs:7980`, unpinned.
- §2.2, actor output columns (duplicate of OR-1's `build_actor_column_map`):
  `mod.rs:741-764` to `mod.rs:778-804`.
- §2.1, the equipment provenance preamble `Provenance.equipment` is built
  from: `mod.rs:2805-2830` to `mod.rs:3083-3114`.
- §2.1, `billing_summaries` field: `mod.rs:1518` to `mod.rs:1705`.
- §2.2, `DwellingProfilingSummary`'s eleven-phase struct: `mod.rs:242-253` to
  `mod.rs:258-270`.
- `conversions.rs:590` to `conversions.rs:595`, and the diagnostics path
  derived from it: `mod.rs:2790-2796` to `mod.rs:3057-3061`.
- The telemetry snapshot's false "reuse their f64 slots" comment: `mod.rs:6431-6433`
  to `mod.rs:6961-6963`.
- `DwellingBlueprintBuilder`'s mutators, `plan.md`'s own API sketch:
  `equipment_names` `blueprint.rs:174` to `blueprint.rs:185`;
  `add_equipment_spec` `blueprint.rs:209` to `blueprint.rs:220`;
  `remove_equipment` `blueprint.rs:183` to `blueprint.rs:194`;
  `remove_equipment_by_end_use` `blueprint.rs:197` to `blueprint.rs:208`.

Checked and already correct, no edit: §2.3's OR-5 citation (`mod.rs:1177-1195,1228-1233,1262-1279`
at `ebdb1a17`, historical: describes the state OR-5, a committed row, fixed);
§2.7's `DwellingConfig` field citation (`mod.rs:210`, off by one, tolerated);
§2.7's `mod.rs:210-215` field list (same tolerance); §2.7's
`blueprint.rs:19-28` public-field list (exact). 4 citations checked correct.

15 citations changed in `plan.md`, 4 checked correct.

## ledger/01-baseline-repair.md

Only BR-10, BR-12, BR-13, BR-14, BR-15 and BR-20 are not committed
(`00-schedule.md` rows 16b-16f, 16a1). Every other row in this file (BR-1
through BR-9, BR-11, BR-16 through BR-19, BR-6a through BR-6d) is committed;
their citations at lines 156, 202, 719 stay as written.

- BR-14, `Dwelling::set_tariff` (was pinned "at 872cf289", checked there and
  correct): `mod.rs:4152` to `mod.rs:4475`, unpinned.

1 citation changed. The rest of BR-10/12/13/15/20 cite no `dwelling/mod.rs`,
`blueprint.rs`, `environment.rs` or `thermal_solver/*.rs` lines.

## ledger/02-measurement.md

M-0 through M-3 and M-4a/M-4b are measurement log entries, recorded or
reusable procedure text, not implementation rows; `00-schedule.md` carries no
commit for them. One citation is a live procedure (reused by not-yet-run
rows such as RT-1/RT-2, which cite "M-3 item 5" by name) and was corrected;
two are self-labelled historical snapshots ("the plan's line numbers have
shifted") and were left as the record they are; one was checked and is
exact.

- M-3's "function shares of one run" procedure, the per-step telemetry map
  clone sites: `mod.rs:6436, :4924` to `mod.rs:6966, :5278`.

Left as written (self-dated historical record, not a live citation): the
`run_timestep`/`record_step`/`check_invariants` symbol map at M-3's
"3. Perf record" item, which states its own lines "have shifted."

Checked and correct: `ProfilePhase::Envelope` phase-clock entry,
`dwelling/mod.rs:6552`, in the Measurement log's envelope-phase note (states
"unchanged by SL-7's diff" and is).

1 citation changed, 1 checked correct, 1 left as a dated record.

## ledger/04-fleet.md

- The file intro, the recorder's unfilled-cell zero default: `mod.rs:6607`
  to `mod.rs:7166` (`self.record_scratch.fill(0.0)`).

Checked and correct: FL-4's site-offset resolution citation,
`blueprint.rs:75-98` (`DwellingBlueprint::from_parts`, exact to within a few
lines).

1 citation changed, 1 checked correct.

## ledger/05-python-surface.md

Checked and correct, no edit: PY-7's `DwellingConfig` citation (`mod.rs:210`,
tolerated); PY-2's `py_blueprint.rs:37` and `:102-115` (`from_hpxml`/`build`,
exact). These are `hares-python` files, outside the mandated sweep scope, but
cheap to confirm since they sit beside blueprint citations.

0 citations changed, 2 checked correct.

## ledger/06-runtime-memory.md

RT-1's whole problem statement was written "at ebdb1a17" and is the single
largest concentration of drift outside file 03, because RT-1 targets exactly
the regions OR-5, OR-11 and the PR rows touched (the equipment snapshot and
domain-update code in `run_timestep`). Re-pinned to `7b7a6a7d`:

- `HashSet` rebuild of equipment ids: `mod.rs:6389-6399` to `mod.rs:6919-6929`.
- `retain` against it: `mod.rs:6400-6402` to `mod.rs:6930-6932`.
- `existing.clone_from(telemetry)` and its false-reuse comment:
  `mod.rs:6431-6436` / `:6432-6433` to `mod.rs:6961-6966` / `:6961-6963`.
- the dwelling's unknown-key latch enforcement: `mod.rs:6479` to `mod.rs:6478`.
- `snapshot_equipment_state` (the checkpoint-restore path): `mod.rs:4892-4930`
  to `mod.rs:5246-5285`; its `clone_from` site `:4922-4924` to `:5273-5278`.
- `EnvironmentManager::update_in_place`'s domain-entry drain and the
  schedule/mains pushes back: `environment.rs:705` to `:678`; `:728, 733` to
  `:701, 706`.
- the thermal/humidity/electrical/fluid/custom `upsert_domain_ref` calls:
  `mod.rs:6010, 6120-6123, 6138` to `mod.rs:6569, 6679-6682, 6697`.
- the custom-update-buffer growth loop: `mod.rs:6126` to `mod.rs:6685-6686`.
- the four `custom_domains` readers: `mod.rs:4191, 4959, 7286, 7313` to
  `mod.rs:4556, 5313, 7784, 7808`.
- `custom_domain_solvers` starting empty at construction (was pinned "at
  21d90588"; checked there, the pin itself was already 10 lines off, the
  field write is at `2750` at that commit, not `2760`): corrected and
  unpinned to `mod.rs:2883`.
- the two custom-solver unit tests and their pushes: `mod.rs:15237` / `:15323`
  and `:15349` to `mod.rs:15816` / `:15903` and `:15929`.

RT-2 (unpinned, must be correct at head; largely was not, same `update_in_place`
region):

- `update_in_place`'s span: `environment.rs:524-782` to `:510-755`.
- humidity/wet-bulb/enthalpy: `:548-552` to `:534-537`.
- civil day of year: `:562-568` to `:543-554`.
- solar position call: `:570-575` to `:556-561`.
- mains temperature: `:605-610` to `:581-585`.
- `compute_annual_offset`'s body (the `ordinal0()`/`hour()`/`minute()` read):
  `:823-846` to `:796-808`.
- the mains-inputs call site: `:243` to `:242-243`.
- the civil-timezone-under-`dst` debug check: `:562-566` to `:568-574`.
- the per-surface Perez/incidence loop: `:621-664` to `:599-637`.
- the PV roof-shading block after it: `:667-676` to `:640-649`.
- `run_warmup_converged`'s day reset and loop (was pinned "at 21d90588"; the
  pin itself was wrong, landing on residual-tracking code, not the reset):
  `mod.rs:5207-5209` to `mod.rs:5396-5398`, unpinned.
- the `solar_override` check inside `update_in_place`: `:621-623` to `:594`.

RT-5's static-disposition table:

- `RANGE_REJECTION_COUNT`'s replacement, `validate_core_contract`'s `?`
  propagation: `mod.rs:5760-5763` to `mod.rs:6270-6273`.
- `EMPTY_TELEMETRY`: `mod.rs:9038` to `mod.rs:9445`.
- `ISLAND_UNSERVED_WARNED`: `mod.rs:144` to `mod.rs:145`.

RT-7's evidence:

- `Dwelling::telemetry()`: `mod.rs:4159-4185` to `mod.rs:4525-4551`.
- event-load equipment's derived seed at construction: `mod.rs:2349-2356` to
  `mod.rs:2562-2587`.
- actors drawing from the dwelling's RNG stream: `mod.rs:3223-3230` to
  `mod.rs:8146-8147`.

Checked and correct, no edit: `environment.rs:195-203` (resample/`time_res`);
`environment.rs:240` (`weather_start_offset`); `blueprint.rs:74-86`
(`from_parts` writing the resolved site); `thermal_solver/config.rs:277-281`
(window U-factor/SHGC fields, exact).

Not fixed, noted only (RT-3, in progress, out of scope): `thermal_solver/stepping.rs:672-674`
(exterior-surface-temp clone/restore) is now at `:653-655` at `7b7a6a7d`, a
19-line drift; `mod.rs:1549` (`refresh_solar_slot_map`) is now at `:1560`, an
11-line drift; `stepping.rs:366` (`solve_ideal_capacity_for_target`) is now at
`:369`, a 3-line drift. RT-3a's own citation is explicitly pinned "at the RT-3
head" and cannot be checked before RT-3 lands; left as scoped.

27 citations changed, 4 checked correct.

## ledger/07-profiler.md

PR-1, PR-2, PR-3, PR-5 and PR-6 are committed; their citations (lines 34,
352, and the `mod.rs:5702-5703` family) stay as written. PR-4 (row 44, not
committed) is already pinned "at `7b7a6a7d`" and its one citation,
`flush_and_close()` at `mod.rs:3146`, is exact.

0 citations changed, 1 checked correct.

## ledger/08-consumer.md

Only CN-0 and CN-16 are committed; every other row, including CN-1, is not.
CN-16's citations (the identity-assignment fix at `dwelling/mod.rs:1477, 2414,
3490` "at 0ec757cb") stay as written.

- CN-1, the always-on zone-temperature-NaN check that already fails a run as
  `InvariantViolation`: `mod.rs:6260-6283` to `mod.rs:6795-6820`.

1 citation changed.

## ledger/09-numeric-fixes.md

No row in this file is committed. Fixes, grouped by entry:

- B-1a/B-1c, the weather-index modulo: `environment.rs:532-536` to `:518-522`
  (both occurrences).
- B-1b, "the shift" (`compute_annual_offset`'s midpoint-offset subtraction):
  `environment.rs:811-849` to `:796-818`.
- B-1c, `ordinal0()` against the file length: `:823-847` to `:806-818`.
- B-1c, the schedule/weather leap-year adjustments' third citation:
  `:869` to `:875`.
- B-1c, `compute_mains_inputs` (both occurrences): `:889-932` to `:862-906`.
- B-2, the warm-up evidence block: `mod.rs:2725` (`if let Some(_init_dur)`)
  to `:2985`; `:2740` (`run_warmup_converged(0.5, 25)`) to `:3000`;
  `:4992-5073` (`run_warmup_converged`'s body) to `:5360-5541`; `:5039-5043`
  (the `fold`) to `:5479`; `:5066-5071` (the non-convergence warning) to
  `:5525-5541`; `:4998` (the incorrect doc comment) to `:5353`; `:1812` (the
  Rust convenience default) to `:2021`; `:2780-2787` (the warm-up-off
  warning) to `:3045-3050`.
- B-2/B-3, the TOML-side converging warm-up (was pinned "at 872cf289",
  checked there and correct): `mod.rs:1935-1937` to `mod.rs:2101-2104`,
  unpinned.
- B-3, the Rust-side default (duplicate of B-2's): `mod.rs:1812` to `mod.rs:2021`.
- B-2, the per-zone warm-up delivered-energy column names: `mod.rs:822-823`
  to `mod.rs:996-997`.
- B-4, the dwelling's billing-period evidence: `mod.rs:6520-6526` (passing
  `self.latest_env.current_time`) to `mod.rs:7048-7051`; `mod.rs:6581`
  (`clock.next()`) to `mod.rs:7134`; `environment.rs:777`
  (`clock.current_time()`) to `:750`; the **Change** section's own
  `mod.rs:6525` to `mod.rs:7050`.
- B-5, `is_ev`'s dwelling-side call: `columns.rs:870` / `mod.rs:534` to
  `columns.rs:870` / `mod.rs:698`; the unstripped-suffix end-use call:
  `mod.rs:606` / `blueprint.rs:201` to `mod.rs:770` / `blueprint.rs:212`; the
  five other name-classifier calls (was "at 21d90588", a stray pin on text
  that was never specific to that commit, dropped): `mod.rs:523-526, 625-626,
  6914-6916, 7191` to `mod.rs:595-598, 697-698, 7191-7193`.
- B-6, the evidence block: `mod.rs:6606-6607` (`record_scratch.fill(0.0)`) to
  `:7166`; `:6700-6791` (per-equipment cells) to `:7183-7310`; `:6786`
  (summed duct-loss cell) to `:7308`; `:6795-6803` (end-use aggregates) to
  `:7317-7326`; `:6806-6815` (actor telemetry) to `:7329-7337`; `:6735-6737`
  (power-factor-1.0 case) to `:7256-7259`; `:6940-6976` (setpoint sentinel)
  to `:7460-7496`.
- B-10, the defaults-path fallback evidence: `blueprint.rs:120-123` to
  `:127-130`; `:124-139` (malformed-TOML handling) to `:131-146`; the second
  relative fallback (was pinned "at ebdb1a17" with an alternate "at
  21d90588" number, both superseded) `mod.rs:2104-2110` / `2244-2249` to
  `mod.rs:2352-2355`, unpinned; the convenience constructors `mod.rs:1808,
  1888` to `mod.rs:1959, 1986`.
- B-11, the Mode column's write site: `mod.rs:6707-6709` to `mod.rs:7230`.
- B-12, the naive-local-start resolution: `blueprint.rs:92-98` to `:95-103`.
- B-14, the zone-capacitance injection site: `mod.rs:2480` to `mod.rs:2591`.
- B-14, `StepResult.net_electric_power_kw`'s fill site: `mod.rs:6841` to
  `mod.rs:7010`.
- B-14, `island_unserved_kw`/`island_excess_kw`'s assignment: `mod.rs:3570,
  3578` to `mod.rs:6760-6761`.
- B-14, the billing-evaluator call site (duplicate of B-4's region):
  `mod.rs:6879-6887` to `mod.rs:7046-7051`.
- B-14, `build_actors_from_seeds` and its TOU-fallback default: `mod.rs:7843-7990`
  / `:7901` to `mod.rs:8033-8189` / `:8090`; the EV-driver literal
  arguments: `mod.rs:7968-7986` to `mod.rs:8158-8174`.
- B-14, `register_pv_roof_shading`'s `capacity_kw` JSON read (was pinned "at
  afa9a071", checked there and correct): `mod.rs:1580-1609` to `mod.rs:1570-1600`,
  unpinned.

Checked and correct, no edit: `environment.rs:418-461` (`compute_schedule_idx`,
cited twice, unchanged); `environment.rs:448-450` (the fallback leap-year
adjustment, unchanged); `environment.rs:418-445` (the half-open rule,
unchanged). 3 citations checked correct (one cited twice).

27 citations changed (several bundling multiple line numbers from one
evidence block), 3 checked correct.

## ledger/10-slop-cleanup.md

SL-7 (row 20, committed as `7b7a6a7d` itself) is the only committed row in
this file; its citations (the dead coupled-LU path, lines 301-319) stay as
written, consistent with being that exact commit's own diff.

- SL-1, the false "reuse their f64 slots" comment and its restore-path
  twin: `mod.rs:6431-6433` to `mod.rs:6961-6963`; `mod.rs:4922-4924` to
  `mod.rs:5278`.
- SL-2's planning-reference census, the `I-06` example: `mod.rs:4909` to
  `mod.rs:5263`.
- SL-2's census, the `ticket_n_i` identifier: `thermal_solver/solar.rs:1122`
  to `:1119` (the declaration, not a later usage).
- SL-8, the BESTEST radiant-fraction test: `mod.rs:12323` to `mod.rs:13021`.
- SL-11, the `[writing]`-pattern census's "V8" column-comment exceptions
  (was pinned "at afa9a071", checked there and correct): `mod.rs:442, 688,
  7114` to `mod.rs:443, 689, 7310`, unpinned.

5 citations changed (2 of them carrying 2-3 line numbers each).

## ledger/11-other-defects.md

No row in this file is committed. Fixes:

- OD-2, the duct-DSE-defaults fallback: `mod.rs:2039-2049` to `mod.rs:2285-2294`.
- OD-20, `register_pv_surfaces`: `mod.rs:1495-1518` (was pinned "at
  afa9a071", checked there and correct) to `mod.rs:1485-1508`, unpinned;
  `attach_pv_to_roofs`: `:1523-1577` to `:1513-1566`; `register_pv_roof_shading`:
  `:1580-1609` to `:1570-1600`; their call sites: `:2210-2215` to `:2244-2249`;
  `environment.rs:667-676` (coverage scales irradiance) to `:640-649`;
  `:952-1019` (`build_surface_geometry`) to `:925-997`; `:627-640`
  (omnidirectional takes `omni_directional_irradiance`) to `:599-637`.
- OD-20, the azimuth-averaged omnidirectional-roof comment block:
  `environment.rs:961-1019` to `:934-997`.
- OD-22, the `match eq.init(..)` block (was pinned "at afa9a071", checked
  there and correct; the code has since grown a warning-drain step before
  the match): `mod.rs:2554-2575` to `mod.rs:2593-2614`, unpinned.
- OD-3, the central loop allocator's call site and the autosizing call
  site after it: `mod.rs:2144` / `:2213` to `mod.rs:2240` / `:2299`.

Checked and correct, no edit: OD-2's `blueprint.rs:96-97` (the non-finite
tz-offset rounding) and `blueprint.rs:112-120` (the civil-timezone parse,
within a line); `environment.rs:422, 549` (civil-time reads); OD-6's
`blueprint.rs:46-56` and `mod.rs:1937-1948` (the schedule-generation
duplication, exact); OD-17's `blueprint.rs:52-62` and `mod.rs:1937-1941`
(the same callers, exact); OD-18's `mod.rs:4826` (`record_clamped_actions`,
already pinned "at 7b7a6a7d" and exact); OD-21's `blueprint.rs:75`
(`from_parts`, exact); OD-20's `environment.rs:376` (`set_pv_roof_coverage`,
exact, cited twice).

11 citations changed, 8 checked correct.

## ledger/12-inputs.md

No row in this file is committed. Fixes:

- IN-4, `update_in_place`'s span and its weather-lookup lines:
  `environment.rs:524-700` to `:510-755`; `:529-536` to `:515-534`.

Checked and correct, no edit: IN-2's `mod.rs:3000` (`run_warmup_converged(0.5,
25)` call site); IN-3's `mod.rs:210` (`DwellingConfig`); IN-4's `:758`
(`update()`, exact); IN-7's `mod.rs:220-223` (the `resample_overrides`
field and doc).

1 citation changed (2 line numbers), 4 checked correct.

## Totals

| File | Changed | Checked correct |
|---|---|---|
| ledger/03-output-reporting.md | 19 | (covered by the audit's own count) |
| plan.md | 15 | 4 |
| ledger/01-baseline-repair.md | 1 | 0 (committed rows skipped by rule) |
| ledger/02-measurement.md | 1 | 1 |
| ledger/04-fleet.md | 1 | 1 |
| ledger/05-python-surface.md | 0 | 2 |
| ledger/06-runtime-memory.md | 27 | 4 |
| ledger/07-profiler.md | 0 | 1 |
| ledger/08-consumer.md | 1 | 0 |
| ledger/09-numeric-fixes.md | 27 | 3 |
| ledger/10-slop-cleanup.md | 5 | 0 |
| ledger/11-other-defects.md | 11 | 8 |
| ledger/12-inputs.md | 1 | 4 |

98 citations changed across the sweep (19 of them the required set in file
03), 28 explicitly checked and confirmed correct beyond what the audit had
already verified.

## RT-3: stale lines found, not fixed (in progress, out of scope)

`ledger/06-runtime-memory.md` RT-3's problem statement is written "at
afa9a071" and describes the thermal solver's allocation sites. Checked
against `7b7a6a7d` (SL-7's commit, the last one before RT-3's own
in-progress work):

- `thermal_solver/stepping.rs:672-674` (the exterior-surface-temperature
  clone and restore around `build_input_vector`) is now at `:653-655`, a
  19-line drift.
- `mod.rs:1549` (`refresh_solar_slot_map`) is now at `:1560`, an 11-line
  drift.
- `stepping.rs:366` (`solve_ideal_capacity_for_target`) is now at `:369`, a
  3-line drift.

These should be re-derived against the commit RT-3 actually lands as, not
against `afa9a071` or `7b7a6a7d`, once that commit exists.
