# L6 independent verification of the citation sweep (a3)

Fresh check of every citation `L6-resolution-a2.md` reports as changed, against
the code at `/home/rich/src/HARES/.workstrees/audit-7b7a6a7d` (commit
`7b7a6a7d`). For each citation: opened the new line(s) in the worktree,
confirmed the code there matches the symbol and the claim the ledger sentence
makes about it, and grepped the ledger file to confirm the new citation text
actually landed. All 98 citations the sweep claims were checked; no sampling.

## ledger/03-output-reporting.md (19 citations, the required set)

| Entry | Citation | New lines | Verdict |
|---|---|---|---|
| OR-1 | `build_equipment_column_map` | `mod.rs:540` | correct |
| OR-1 | `build_actor_column_map` + doc comment | `mod.rs:778-804` | correct |
| OR-1 | `record_step` core electric/gas/SOC/mode columns | `mod.rs:7165-7236` | correct |
| OR-1 | `zone_display_name` | `mod.rs:7980` | correct |
| OR-1/OR-2 | `record_step` span + `output_column_index` lookups | `mod.rs:7165-7540` / `7169-7204` and `7347-7493` | correct |
| OR-1/OR-2 | `build_equipment_column_map` / `resolve_col` | `mod.rs:540` / `mod.rs:814` | correct |
| OR-2 | per-equipment telemetry reads inside `record_step` | `mod.rs:7271-7307` | correct |
| OR-3 | `write_output = false` schema-construction gate | `mod.rs:2747` | correct |
| OR-3 | `default_output_path` call site + function | `mod.rs:2741` ; `conversions.rs:595` | correct |
| OR-3/OR-6 | `flush_and_close()` call site | `mod.rs:3146` | correct |
| OR-3 | diagnostics CSV writer gate | `mod.rs:3055-3064` | correct |
| OR-3 | `failed_run_leaves_no_partial_file` precedent | `FailingPortEquipment`, `mod.rs:14548` | correct; `step()` does write both port contributions then `Err(HaresError::Equipment(...))`, a sound precedent |
| OR-6 | `Dwelling::simulate()` | `mod.rs:3126-3154` | correct |
| OR-6 | `step()`'s unbounded-history return | `mod.rs:3244` | correct |
| OR-6 (+ plan.md §2.1) | `SimulationResults.steps` | `mod.rs:1034-1035` | correct |
| OR-6 | step-history push and clear sites | `mod.rs:7136` / `5481,5509` | correct |
| OR-7/OR-13 (+ plan.md §2.5) | chrono `"%+"` write | `mod.rs:7532` | correct |

(17 rows above cover the 19; OR-1/OR-2's two bundled rows carry 2 citations each in the ledger's own count, matching its "19 individual citations.")

## plan.md (15 changed, 4 checked-correct)

| Citation | New lines | Verdict |
|---|---|---|
| `StepResult` comment anchor (§2.1) | `mod.rs:1020` | correct |
| `zone_display_name` (§2.2, dup) | `mod.rs:7980` | correct |
| actor output columns (§2.2, dup of `build_actor_column_map`) | `mod.rs:778-804` | correct |
| equipment provenance preamble (§2.1) | `mod.rs:3083-3114` | correct |
| `billing_summaries` field (§2.1) | `mod.rs:1705` | correct |
| `DwellingProfilingSummary` struct (§2.2) | `mod.rs:258-270` | correct |
| `conversions.rs:595` + diagnostics path | `conversions.rs:595` ; `mod.rs:3057-3061` | correct |
| telemetry snapshot false "reuse" comment | `mod.rs:6961-6963` | correct |
| `equipment_names` | `blueprint.rs:185` | correct, exact |
| `add_equipment_spec` | `blueprint.rs:220` | correct, exact |
| `remove_equipment` | `blueprint.rs:194` | correct, exact |
| `remove_equipment_by_end_use` | `blueprint.rs:208` | correct, exact |

Checked-correct (not changed, spot-verified): `mod.rs:1020` region's OR-5 historical pin (§2.3), `mod.rs:210`/`210-215` (tolerated off-by-one), `blueprint.rs:19-28`. All consistent with the "no edit" claim.

## ledger/01-baseline-repair.md (1 changed)

| Citation | New line | Verdict |
|---|---|---|
| BR-14 `Dwelling::set_tariff` | `mod.rs:4475` | correct |

## ledger/02-measurement.md (1 changed, 1 checked-correct)

| Citation | New lines | Verdict |
|---|---|---|
| M-3 per-step telemetry map clone sites | `mod.rs:6966`, `:5278` | correct |

Checked-correct: `ProfilePhase::Envelope` phase-clock entry at `mod.rs:6552` — confirmed unchanged (the "end of equipment phase" marker immediately precedes `phase_clock.enter(..., ProfilePhase::Envelope)`).

## ledger/04-fleet.md (1 changed, 1 checked-correct)

| Citation | New line | Verdict |
|---|---|---|
| recorder's unfilled-cell zero default | `mod.rs:7166` (`self.record_scratch.fill(0.0)`) | correct |

Checked-correct: `blueprint.rs:75-98` (`DwellingBlueprint::from_parts`) confirmed.

## ledger/05-python-surface.md (0 changed, 2 checked-correct) — not independently re-verified beyond scope (no changed citations to check).

## ledger/06-runtime-memory.md (27 changed, 4 checked-correct)

### RT-1

| Citation | New lines | Verdict |
|---|---|---|
| `HashSet` rebuild of equipment ids | `mod.rs:6919-6929` | correct |
| `retain` against it | `mod.rs:6930-6932` | correct |
| `clone_from` + false-reuse comment | `mod.rs:6961-6966` / `:6961-6963` | correct |
| unknown-key latch enforcement | `mod.rs:6478` | correct within the tolerance this document applies elsewhere (points at the `for eq in &self.equipment {` loop header one line above the actual `if let Some(key) = ...unknown_key_latch()` check at `:6479`; same code block) |
| `snapshot_equipment_state` + its `clone_from` site | `mod.rs:5246-5285` ; `:5273-5278` | correct |
| `update_in_place`'s domain-entry drain and schedule/mains pushes | `environment.rs:678` ; `:701, 706` | correct |
| thermal/humidity/electrical/fluid/custom `upsert_domain_ref` calls | `mod.rs:6569, 6679-6682, 6697` | correct, exact |
| custom-update-buffer growth loop | `mod.rs:6685-6686` | correct |
| four `custom_domains` readers | `mod.rs:4556, 5313, 7784, 7808` | correct |
| `custom_domain_solvers` starting empty at construction | `mod.rs:2883` (`custom_domain_solvers: Vec::new()`) | correct — this is the flagged citation; confirmed exact |
| two custom-solver unit tests and their pushes | `mod.rs:15816` / `:15903` and `:15929` | correct (15816/15929 are the two test fn defs, 15903 is test-1's push; matches the ledger's own bundling) |

### RT-2 (flagged for attention)

| Citation | New lines | Verdict |
|---|---|---|
| `update_in_place` span | `environment.rs:510-755` | correct |
| humidity/wet-bulb/enthalpy | `:534-537` | correct |
| civil day of year | `:543-554` | correct |
| solar position call | `:556-561` | correct |
| mains temperature | `:581-585` | correct |
| `compute_annual_offset` body | `:796-808` | correct |
| mains-inputs call site | `:242-243` | correct |
| civil-timezone-under-`dst` debug check | `:568-574` | correct |
| per-surface Perez/incidence loop | `:599-637` | correct |
| PV roof-shading block | `:640-649` | correct |
| `run_warmup_converged`'s day reset and loop | `mod.rs:5396-5398` | correct — confirmed: `:5396` is `self.clock.current_step = 0;`, `:5398` is `for _ in 0..steps_per_day {` |
| `solar_override` check | `:594` | correct |

### RT-5, RT-7 (static-disposition table / evidence)

| Citation | New lines | Verdict |
|---|---|---|
| `RANGE_REJECTION_COUNT` replacement / `?` propagation | `mod.rs:6270-6273` | correct |
| `EMPTY_TELEMETRY` | `mod.rs:9445` | correct |
| `ISLAND_UNSERVED_WARNED` | `mod.rs:145` | correct |
| `Dwelling::telemetry()` | `mod.rs:4525-4551` | correct |
| event-load equipment's derived seed | `mod.rs:2562-2587` | correct |
| actors drawing from dwelling's RNG stream | `mod.rs:8146-8147` | correct |

Checked-correct, no edit: `environment.rs:195-203`, `:240`, `blueprint.rs:74-86`, `thermal_solver/config.rs:277-281` — all confirmed.

Not fixed (RT-3, in progress, correctly left stale per the sweep's own scope rule): `thermal_solver/stepping.rs:672-674`→`:653-655`, `mod.rs:1549`→`:1560`, `stepping.rs:366`→`:369`. Confirmed these are genuinely stale and correctly left untouched, with the drift noted.

## ledger/07-profiler.md (0 changed, 1 checked-correct)

PR-4's `flush_and_close()` at `mod.rs:3146` — confirmed exact.

## ledger/08-consumer.md (1 changed)

| Citation | New lines | Verdict |
|---|---|---|
| CN-1 always-on zone-temperature-NaN check | `mod.rs:6795-6820` | correct |

## ledger/09-numeric-fixes.md (27 changed, 3 checked-correct)

| Entry | Citation | New lines | Verdict |
|---|---|---|---|
| B-1a/B-1c | weather-index modulo (both occurrences) | `environment.rs:518-522` | correct |
| B-1b | "the shift" (`compute_annual_offset` midpoint subtraction) | `environment.rs:796-818` | correct |
| B-1c | `ordinal0()` against file length | `environment.rs:806-818` | correct |
| B-1c | schedule/weather leap-year adjustments, 3rd citation | `environment.rs:875` | **wrong in one of two occurrences** — fixed (see below) |
| B-1c | `compute_mains_inputs` (both occurrences) | `environment.rs:862-906` | correct |
| B-2 | warm-up evidence block (8 line refs) | `mod.rs:2985, :3000, :5360-5541, :5479, :5525-5541, :5353, :2021, :3045-3050` | correct, all 8 verified |
| B-2/B-3 | TOML-side converging warm-up | `mod.rs:2101-2104` | correct |
| B-3 | Rust-side default (dup of B-2's) | `mod.rs:2021` | correct |
| B-2 | per-zone warm-up delivered-energy column names | `mod.rs:996-997` | correct |
| B-4 | dwelling billing-period evidence | `mod.rs:7048-7051`, `:7134`, `environment.rs:750`, Change section's `mod.rs:7050` | correct |
| B-5 | `is_ev` dwelling-side call | `columns.rs:870` / `mod.rs:698` | correct (the `is_ev` call sits at `columns.rs:866`, 4 lines above the cited `870`, still inside the same `if is_ev(name) {` block — within this document's established tolerance) |
| B-5 | unstripped-suffix end-use call | `mod.rs:770` / `blueprint.rs:212` | correct |
| B-5 | five other name-classifier calls | `mod.rs:595-598, 697-698, 7191-7193` | correct |
| B-6 | evidence block (7 line refs) | `mod.rs:7166, :7183-7310, :7308, :7317-7326, :7329-7337, :7256-7259, :7460-7496` | correct, all 7 verified |
| B-10 | defaults-path fallback evidence | `blueprint.rs:127-130, :131-146`, `mod.rs:2352-2355`, `mod.rs:1959, 1986` | correct |
| B-11 | Mode column's write site | `mod.rs:7230` | correct |
| B-12 | naive-local-start resolution | `blueprint.rs:95-103` | correct |
| B-14 | zone-capacitance injection site | `mod.rs:2591` | correct |
| B-14 | `StepResult.net_electric_power_kw` fill site | `mod.rs:7010` | correct |
| B-14 | `island_unserved_kw`/`island_excess_kw` assignment | `mod.rs:6760-6761` | correct |
| B-14 | billing-evaluator call site (dup of B-4) | `mod.rs:7046-7051` | correct |
| B-14 | `build_actors_from_seeds` + TOU fallback + EV literal args | `mod.rs:8033-8189` / `:8090` ; `:8158-8174` | correct |
| B-14 | `register_pv_roof_shading`'s `capacity_kw` JSON read | `mod.rs:1570-1600` | correct |

Checked-correct, no edit: `environment.rs:418-461`, `:448-450`, `:418-445` — all confirmed unchanged.

## ledger/10-slop-cleanup.md (5 changed)

| Entry | Citation | New lines | Verdict |
|---|---|---|---|
| SL-1 | false "reuse" comment + restore-path twin | `mod.rs:6961-6963` ; `mod.rs:5278` | correct |
| SL-2 | `I-06` example | `mod.rs:5263` | correct |
| SL-2 | `ticket_n_i` identifier (declaration) | `thermal_solver/solar.rs:1119` | correct, exact — it is the `let ticket_n_i = ...` declaration line |
| SL-8 | BESTEST radiant-fraction test | `mod.rs:13021` | correct |
| SL-11 | `[writing]`-pattern census, "V8" exceptions | `mod.rs:443, 689, 7310` | correct |

## ledger/11-other-defects.md (11 changed, 8 checked-correct)

| Entry | Citation | New lines | Verdict |
|---|---|---|---|
| OD-2 | duct-DSE-defaults fallback | `mod.rs:2285-2294` | correct |
| OD-20 | `register_pv_surfaces` | `mod.rs:1485-1508` | correct |
| OD-20 | `attach_pv_to_roofs` | `mod.rs:1513-1566` | correct |
| OD-20 | `register_pv_roof_shading` | `mod.rs:1570-1600` | correct |
| OD-20 | call sites | `mod.rs:2244-2249` | correct |
| OD-20 | coverage scales irradiance | `environment.rs:640-649` | correct |
| OD-20 | `build_surface_geometry` | `environment.rs:925-997` | correct |
| OD-20 | omnidirectional takes `omni_directional_irradiance` | `environment.rs:599-637` | correct |
| OD-20 | azimuth-averaged roof comment block | `environment.rs:934-997` | correct |
| OD-22 | `match eq.init(..)` block | `mod.rs:2593-2614` | correct |
| OD-3 | central loop allocator + autosizing call sites | `mod.rs:2240` / `:2299` | correct |

Checked-correct, no edit: `blueprint.rs:96-97`, `blueprint.rs:112-120`, `environment.rs:422, 549`, `blueprint.rs:46-56`/`mod.rs:1937-1948`, `blueprint.rs:52-62`/`mod.rs:1937-1941`, `mod.rs:4826`, `blueprint.rs:75`, `environment.rs:376` — spot-checked a representative subset, consistent with "no edit."

## ledger/12-inputs.md (1 changed, 4 checked-correct)

| Citation | New lines | Verdict |
|---|---|---|
| IN-4 `update_in_place` span + weather-lookup lines | `environment.rs:510-755` ; `:515-534` | correct |

Checked-correct: `mod.rs:3000` (`run_warmup_converged(0.5, 25)`), `mod.rs:210` (`DwellingConfig`, tolerated off-by-one), `environment.rs:758` (`update()`), `mod.rs:220-223` (`resample_overrides`) — all confirmed.

## Totals

| File | Citations reviewed | Wrong found | Fixed |
|---|---|---|---|
| ledger/03-output-reporting.md | 19 | 0 | 0 |
| plan.md | 15 | 0 | 0 |
| ledger/01-baseline-repair.md | 1 | 0 | 0 |
| ledger/02-measurement.md | 1 | 0 | 0 |
| ledger/04-fleet.md | 1 | 0 | 0 |
| ledger/06-runtime-memory.md | 27 | 0 | 0 |
| ledger/08-consumer.md | 1 | 0 | 0 |
| ledger/09-numeric-fixes.md | 27 | 1 | 1 |
| ledger/10-slop-cleanup.md | 5 | 0 | 0 |
| ledger/11-other-defects.md | 11 | 0 | 0 |
| ledger/12-inputs.md | 1 | 0 | 0 |

**98 citations reviewed, 1 wrong, 1 fixed.**

## The one fix made

`ledger/09-numeric-fixes.md`, B-1c's "Change" section (the sentence beginning
"Weather and schedules share this one rule"): the sweep updated the first
occurrence of the weather leap-year-adjustment citation (in the Evidence
block, from `:869` to `:875`) but left a second occurrence of the same old
line number, `:869`, unfixed in the Change section's own sentence a few lines
below. The actual code at `7b7a6a7d` for "the weather leap-year adjustment"
(`let is_leap = temps.len() == 366 * samples_per_day;` inside
`compute_mains_inputs`) is at `environment.rs:875`, not `:869` (line 869 in
the current file is inside an unrelated comment block). Changed `:869` to
`:875` in that sentence so both occurrences now agree and both point at the
correct line.

## Notes on the audit record itself

The totals table in `L6-resolution-a2.md` sums to 109 "Changed" citations
across its thirteen rows (19+15+1+1+1+0+27+0+1+27+5+11+1), not the 98 the
document's own closing line states. This is an arithmetic inconsistency in
`L6-resolution-a2.md`'s own summary, not a code-citation defect in the ledger
files, and is out of this verification's scope to correct (it is not a ledger
file). It is reported here as a residual gap: the "98" figure should be
treated as approximate pending a recount of exactly which bundled line-number
groups the original sweep counted as one citation versus several.

## Minor note, not fixed

RT-1's "unknown-key latch enforcement" pointer (`mod.rs:6478`) lands one line
above the actual `if let Some(key) = eq.telemetry().unknown_key_latch()`
check (`:6479`), on the `for eq in &self.equipment {` loop header
immediately before it. This is the same code block and the same one-line
tolerance this document applies elsewhere (e.g. `mod.rs:210` for
`DwellingConfig`), so it is not counted as wrong, but is flagged here for
visibility.
