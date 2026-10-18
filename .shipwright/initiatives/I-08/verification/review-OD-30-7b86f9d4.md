# Review and red team: OD-30 at `7b86f9d4` (branch `fix-od30`, 23 commits on `1b708ccb`)

Reviewer did not write any of it. Two scratch worktrees under
`.workstrees/scratch-review-od30-*` (a: the block at `7b86f9d4`; b: mutations and
the fold attempt), private script and logs under the session scratchpad
(`review-od30/`), `CARGO_TARGET_DIR` and `TMPDIR` inside it. Everything created is
deleted (worktrees removed after `submodule deinit`, target dirs and logs removed).
Nothing on the branch, on any other worktree or on the main tree was modified.

## Verdict: CHANGES-REQUIRED

No high finding. The branch is correct and green: the whole `plan.md` section 3
block passes on `7b86f9d4`, goldens are bitwise, every mutation of the override
validator, the wildcard resolver and the zone behaviour is caught by a test. Two
items are required before the fold, and the fold itself is not mechanical:

1. The debug-build check that a raw read off the list panics is not pinned by any
   test (finding M1, reproduced: disabling it fails zero tests).
2. The silent skip of a schedule column that creates no equipment is only a
   `tracing::debug!` at the tip (finding M2). The head already has the visible
   form (`warn!`) in `7ac4d4d9`; the fold must take it, and `fix-headbreaks`
   wants it as a dwelling construction warning.

The fold onto `db5efe80` needs 9 hand-resolved conflicts and 5 call-site fixes
(section 9); it compiles and passes 5468 tests after those.

## Findings

### Medium

**M1. "A debug read off the list panics" has no test.**
`crates/hares-equipment/src/config.rs:395-404` (`raw_value`, the `assert!(listed == Some(kind), ...)`).
The list-completeness side is pinned only by the existing load tests reaching each
read (removing `cooldown_duration_s` from `EVENT_LOAD_PARAMS` fails 20+ event-load
tests). The assertion itself is not: replacing the condition with
`true || listed == Some(kind)` leaves hares-equipment lib (2402), hares-core
`conversions` (75), `appliance_zone_gains` (14) and hares-io lib (980) all green.
No `#[should_panic]` exists for it (grep over `crates/hares-equipment`). A later
edit that weakens or deletes the check passes the suite; and a reader that adds a
new key on a path no test executes is not caught either. Fix: a debug-only
`#[should_panic(expected = "does not name")]` test per kind (a key off the list; a
listed key read as the wrong kind), plus one test that a listed key read as its
kind does not panic. Confidence: high (reproduced by mutation).

**M2. A schedule column that creates no equipment is logged at `debug` only.**
`crates/hares-io/src/schedule_resolve.rs:1523-1528`. A `lighting_garage`,
`lighting_basement` or `clothes_dryer` column for a home without that equipment now
creates nothing and says so only at `tracing::debug!` (the previous round's M2.7,
still open at the tip). The branch's rule (create only when the equipment has a
default energy) is the same rule the head reached in `7ac4d4d9`, whose version logs
`warn!`. At the fold take the head's `warn!`; `fix-headbreaks` `c71a360b` goes
further and puts the turn-away into the dwelling's construction warning list, which
is the only channel a Python user sees. Pin it: the branch's test
`columns_for_equipment_without_energy_create_no_equipment` (`schedule_resolve.rs`
tests) asserts only that no spec is created. Confidence: high.

**M3. Fold readiness: not a clean rebase, and two ledger statements are stale.** See
section 9 for the list. The head's `fix-headbreaks` branch (`cd1b5972`) conflicts
with this branch in 5 source files and the 7 goldens; its 7 golden recaptures and
this branch's recaptures cannot both stand, so the folded goldens must be recaptured
once from the clean folded tree.

### Low

**L1. A negative month multiplier is silently clamped to zero.**
`crates/hares-equipment/src/schedule_helpers.rs:181` (`*slot = val.max(0.0)`). The
override list admits any number for `month_multiplier_<n>`, and the new override
channel is the first way a user reaches it; `-1.0` becomes "load off that month" with
no error (NaN and infinity are rejected, negative is not). Same class as the no
silent substitution rule. Fix: reject a negative multiplier with the key named.
Pre-existing code, newly reachable. Confidence: high (by reading; not run).

**L2. The resolver's own override pass bypasses the list.**
`crates/hares-io/src/hpxml/equipment.rs:163-181` (`apply_overrides`, called from
`resolve_equipment` at `:69`). It applies the whole wildcard to every spec's bag and
an equipment's own entry unchecked. The dwelling passes `{}` here
(`crates/hares-core/src/dwelling/blueprint.rs:137-146`), so production is not
affected, but `resolve_equipment` is public and the commit message says the
wildcard has one resolver; only the layer lookup is shared. Either drop the
parameter from `resolve_equipment` (greenfield) or route it through the list.

**L3. Appliance zone id is the first matching zone, the zone map is the last.**
`crates/hares-io/src/hpxml/resolve_loads.rs:1166` (`.position(...)`, first zone of
the type) against `crates/hares-core/src/dwelling/mod.rs:2477-2505` (`map.insert`,
the last zone of a role wins). With two zones of one type (two garages, or a
foundation zone and a crawlspace zone, which share the `Basement` and `Crawlspace`
roles) a garage appliance and a garage-named light land in different zones. Not
reproduced on any fixture (no corpus building has two zones of one type); by
reading. Fix: one rule, or an error on a duplicate type.

**L4. The zone-ordering pin is split across crates.** Mutation of
`appliance_site` (`idx + 1` to `idx`) is caught by two hares-io unit tests
(`appliance_sites_follow_openstudio_hpxml_locations`,
`appliances_give_their_heat_where_they_stand`) but by no hares-core test: the
`appliance_zone_gains.rs` fixtures have no appliance outside the conditioned zone,
so a dwelling-level break of "garage appliance heat lands in the garage" is
unpinned end to end. Add a garage-refrigerator dwelling test.

**L5. Silent paths the branch touches or sits beside (item 10), none new.**
- `crates/hares-io/src/hpxml/resolve_hvac.rs:327-370` (`rebuild_hvac_typed_config`,
  `.ok().flatten()` on every `try_build_*`), `:1151`, `:1305`, `:1336` (`.ok()?` on
  `EquipmentConfig::from_typed` for Ideal HVAC, Room AC, Dehumidifier): any build
  error becomes `None`, the caller `crates/hares-core/src/dwelling/autosize.rs:590-591`
  assigns it to `spec.typed_config`, and the spec falls to the raw path. The build then
  fails at init with `requires typed config (was Raw)`
  (`crates/hares-equipment/src/config.rs:373-377`), so the failure is loud but names the
  wrong cause and the serde error is lost. Make the builders return `Result` and
  propagate.
- `crates/hares-io/src/hpxml/equipment.rs:244-250` (`build_typed_spec`): a
  serialization failure becomes empty parameters with a warning.
- `crates/hares-equipment/src/event_load.rs:625-628, 1191, 1635-1636`: absent
  `active_power_kw` (1 kW), `active_duration_s` (900 s), `n_units` (1) default
  silently for a load built without them.
- `crates/hares-equipment/src/load_zone.rs:36`: an exterior or outdoor named load
  with zone heat and no `zone_id` drops its heat by design (documented in
  `docs/equipment/loads.md`); an override of its sensible fraction therefore does
  nothing, with no message.
- From the previous review and still open at the tip (not touched by this branch):
  Occupancy invariants gated by `cfg(debug_assertions)`, the `MetricsCalculator`
  init failure that zeroes metrics (`crates/hares-core/src/dwelling/roster.rs:1156`),
  `find_zone_idx` falling to zone 0, `autosize.rs` mains-temperature and bedroom
  defaults.

**L6. One comment misstates OCHRE.** `crates/hares-equipment/src/scheduled_load.rs:246-247`
cites `ScheduledLoad.py:38-41` for "month multipliers scale whatever schedule it has".
That code only zeroes the schedule in months whose multiplier is exactly 0
(`vendors/OCHRE/ochre/Equipment/ScheduledLoad.py:38-41`; the old comment at `:146` says
so). The HARES behaviour (scaling) is a deliberate and correct extension, but the
citation reads as OCHRE parity. Reword, and since it differs from OCHRE record it
beside D-005 to D-007 or in `docs/equipment/loads.md` (which does state the behaviour).

**L7. Two behaviour changes are not stated where a user would look.**
(a) The unvented electric clothes dryer's heat is now the resolver's split (0.90
sensible, 0.10 latent, the OS-HPXML rule) instead of the head's step override that made
an `unvented_condenser` all sensible; removed with the dryer label in `ab38c046`, whose
message does not say the numbers move. (b) `gas_schedule_is_w` accepts only a boolean
(`0`/`1` removed) and `cycle_phase_<n>_*` is gone; both are in the `7f59167d` message
and pinned (`values_of(Bool)` rejects `1.0` in `conversions.rs` tests;
`parse_cycle_phases_reads_one_spelling`), neither is in `docs/equipment/loads.md`.

**L8. The measurement log does not give a cause for every moved leaf.**
`ledger/02-measurement.md` OD-30 table lists energy and peak leaves with the
internal-gain cause, but not the leaves that move because event replay now follows the
schedule date (`a71f0af8`): `fleet_mixed_offset` dwelling 0 `cooking` 7.93 to 15.90 kWh
and `other` 52.52 to 29.14 kWh (gross -12.83 %), `leap_year_february_900s` and
`weather_wrap_900s` water heating +39 % and +31 % (4.23 to 5.89 and 3.64 to 4.76 kWh).
Signs and orders are plausible (the replayed day changes which cycles run), the cause is
in the `a71f0af8` message, but the log's table omits `cooking` and `other` and attributes
nothing to the replay. Add one sentence per golden.

### Info

- No test applies a wildcard override across the committed corpus (ochre samples and
  ResStock fixtures): `a_wildcard_reaches_the_loads_that_read_it` runs on cz2a only.
  "Nothing on a valid corpus input is newly rejected" holds for every input the
  existing census tests build (below) and for overrides no test sends; it is a testing
  gap, not run ad hoc.
- `unvented_fuel_fired_dryer_is_rejected` is the one rule kept from the dryer label.

## Checks

### Block on `7b86f9d4` (plan.md section 3, my own runner, scratch tree, `TMPDIR` set)

| Line | Result |
|---|---|
| `[fmt]` | clean |
| clippy: all-features, default, dst, profiling, observe, observe+profiling, release, release+dst, release+observe (9 lines, `-D warnings`) | all clean |
| `[test-debug]` nextest workspace | 7408 passed, 0 failed, 0 skipped |
| `[test-observe]` | 7467 passed |
| `[test-dst]` | 7421 passed |
| `[test-profiling]` | 7414 passed |
| `[test-observe-profiling]` (hares-core) | 1492 passed |
| `[test-release]` | 7365 passed |
| `[bench]` `--no-run` | builds |
| `[advisories]` | ok |
| `[golden]` default and `-F dst` | `consumer_shape_900s`, `fleet_mixed_offset`, `full_year_hourly`, `leap_year_february_900s`, `month_boundary_billing_3600s`, `weather_wrap_900s`, and `full_year_900s_dst`: all identical (7 of 7) |
| `[python-build]` maturin release | ok |
| `[pytest]` `-m ""` | 1417 passed, 4 xfailed, 0 skipped, 0 failed |
| skip census diff | 0 skips, census empty, diff empty |
| pyright | 0 errors, 0 warnings |
| `[stray]` new paths | none |
| `[stray]` TMPDIR | fails: 6690 leftover entries (see below) |

The corpus builds: `every_fixture_builds_every_equipment` (hares-core,
`equipment_init_failure_assembly.rs`, the 14 `ochre_samples` HPXMLs),
`census_specs_without_any_schedule_source_across_checked_in_hpxmls` (hares-io),
`resstock_smoke` (2024.2: 11 buildings, 2025.1; smoke, summer 72 h, winter 72 h,
manifest hashes) and `parity_outputs_against_reference_corpus` all pass.

**`[stray]` TMPDIR.** The 6690 entries are 6096 `hares-regr-output-*.csv`
(`tests/regression/helpers.rs`, from `a0fb8cf2`), 203 `hares-fleet-*`
(`crates/hares-fleet/src/fleet.rs:1059`), 18 `hares-core-engine*`,
`hares-test-batchstep-*.toml`, `pv_lut_*.parquet` and `.csv` (hares-equipment PV
tests), two `hares-*.xml` HPXML test files, and Python `tmp*` directories. None of the
sources is in `1b708ccb..7b86f9d4`. Proof for the branch's tests: with a fresh empty
`TMPDIR`, the branch-added or branch-edited Rust test sets leave 0 entries: hares-core
`appliance_zone_gains`, `assembly_errors`, `equipment_init_failure_assembly`,
`equipment_identity_boundaries`, `equipment_id_regressions`, `port_radiant_diag` (67
tests, 0 leaks); `frame_golden` and hares-types (482 tests, 0); hares-core lib (1081,
0); hares-io (1390, 0). Running hares-equipment adds the `pv_lut_*` files (pre-existing;
the branch touches `pv/mod.rs` by one `schedule_row: None` line). Python: the branch
removed two `mkdtemp` users (`test_ochre_parity.py`, `test_thermal_trace.py`, now
`tmp_path_factory`) and added none; the other two Python files it touched add no
temporary path (`parity_diagnostics.py` one key, `test_py_fleet.py` the defaults path).

### 1. Override list

One list per kind (`raw_params.rs`: `SCHEDULED_LOAD`, `EVENT_LOAD`, `WET_APPLIANCE`,
`ALL`), used by the readers' check (`config.rs:390-408`), the dwelling's kind check on
resolved parameters (`conversions.rs:696-708`), the own-entry check
(`conversions.rs:1257-1271`), and the wildcard resolver. I listed every `get_f64`,
`get_str`, `get_bool`, `get_f64_array` and `raw_value` in `event_load.rs`,
`scheduled_load.rs`, `schedule_helpers.rs`, `gain_fractions.rs`, `load_zone.rs`:
every key is on its kind's list with the kind it is read as (phase keys by
`phase_len`; `fuel_type` on the event kinds only; `usage_multiplier` on scheduled
only). The remaining raw reads in the crate belong to unlisted classes (typed
configs; the raw EV and similar blueprint-added specs, documented as taking the
whole wildcard unchecked). No reader bypasses the list. The only bypass is on the write
side (L2). The debug assertion is real but unpinned (M1). Mutations (scratch b):

| Mutation | Result |
|---|---|
| remove one key from `EVENT_LOAD_PARAMS` (`cooldown_duration_s`) | `hares-equipment` lib: 20+ event-load tests FAIL (read off the list) |
| disable the assertion in `raw_value` | no test fails (M1) |
| skip the own-entry key check (`raw_load_parameters`) | `a_load_takes_an_override_of_exactly_the_parameters_it_reads`, `a_rejected_override_lists_what_the_load_reads`, `an_override_the_load_cannot_take_fails_the_build` fail |
| numbered-key range check always true (`raw_params.rs`) | `a_numbered_family_admits_members_below_its_count`, `a_wildcard_numbered_key_reaches_a_load_within_its_count`, `a_wildcard_parameter_must_be_read_by_some_equipment` fail |

### 2. Wildcards

`wildcard_override` and `override_layers` (`equipment.rs:111-157`) are the one
resolver; both spellings is an error; a non-object layer is an error; numbered keys are
range-checked by the family count; `validate_wildcard_override` rejects a key no
population member reads. Mutations: drop the both-spellings error fails
`override_layers_resolve_one_wildcard` and `both_wildcard_spellings_are_an_error`;
`read = true` in the population loop fails `a_wildcard_parameter_must_be_read_by_some_equipment`
and `a_wildcard_no_equipment_can_take_fails_the_build`. The corpus tests above pass;
the corpus has no wildcard input (grep: every `all`/`*` override in the repo is a test),
see the Info item.

### 3. Behaviour changes

- `month_multiplier_<n>` composing with a daily profile's month factors: correct
  (the profile's factors are its shape, the multipliers scale it; before, an override
  of `month_multiplier_n` on a load with a default profile did nothing). Nothing in the
  engine writes the keys (grep), so no double scaling of resolved inputs; the expected
  mean multiplies month by month (`scheduled_load.rs`, `month_multipliers_scale_a_daily_profile`,
  and the dwelling-level `a_scheduled_load_takes_the_parameters_the_resolver_left_out`).
  Documented in `docs/equipment/loads.md`. Citation issue L6; negative clamp L1.
- `cycle_phase_<n>_*` removed: nothing wrote it (grep over crates, python, defaults,
  docs, tests), pinned by `parse_cycle_phases_reads_one_spelling`; in the commit
  message only (L7).
- `gas_schedule_is_w` boolean only: correct (a number is now a typed parameter error
  through the kind check), nothing writes it; commit message only (L7).

### 4. Rebase decisions against `1b708ccb`

- Init failure: `crates/hares-core/src/dwelling/mod.rs:2638-2650` is the head's
  `8c034e4f` code plus one arm that keeps `InvalidEquipmentParameter` typed. The branch's
  `80f6315e` is gone from the history (`22bd7171` carries only the share resolution,
  the schedule-column rule and the duct and basement-copy errors). One implementation;
  grep finds no skip-and-warn path left in dwelling assembly (the remaining "non-critical"
  hits are comments and `defaults.rs` row fallbacks).
- Head's garage gate and undeclared-dryer default are removed: `inject_schedule_into_specs`
  lost `foundation_name` and `garage_modeled` (`schedule_resolve.rs`), the schedule-column
  rule replaces them; `models_garage()` stays for the HPXML lighting resolver. The head's
  basement-lighting and crawlspace tests collapse into
  `columns_for_equipment_without_energy_create_no_equipment`, which also covers
  `clothes_dryer`; the head's own later commit `7ac4d4d9` reached the same rule (M2).
- `xs:boolean`: one reader remains, `xml_helpers.rs:60-101` (`xs_boolean`, `child_bool`),
  same lexical space as the head's `parse_xsd_boolean` (`true|false|1|0`, case-sensitive)
  with a typed `InvalidField` error instead of `Parse`; the head's every-site test is kept
  and renamed (`every_xsd_boolean_site_uses_the_xs_boolean_reader`); no other boolean parse
  in `hares-io/src`.
- Nothing of the head lost: `git range-diff` of the old tip (`8ded384b` on `38398bd4`)
  against `1b708ccb..7b86f9d4` shows the expected differences; a line-level check of the 60
  files both sides touched (head-added lines the branch removes, `38398bd4..1b708ccb`
  against `1b708ccb..7b86f9d4`) leaves only: `parse_xsd_boolean` call sites (replaced by
  `child_bool`), the head's garage gate and dryer default (replaced, above), the typed-config
  `check::<...>` match lines in `config.rs` (reformatted by the branch's registry change),
  and the head's old gain-fraction comments. Test functions removed from the head: 13, each
  accounted for (dryer-label tests with `ab38c046`, convective-fraction test with the removed
  key, the four basement tests as above, `is_conditioned_location...` with
  `appliance_site`, `parse_cycle_phases_alternate_key_format` with `cycle_phase_`, the
  renamed boolean test).

### 5. Goldens

All seven carry `git_head` `320f85c2` (an ancestor of the tip, branch commit "give lighting
its radiant and visible parts", the parent of the last recapture `b41221f6`) and
`git_dirty` false; defaults digests `3f9c3a95` (six) and `5339acb2`
(`consumer_shape_900s`), which is the digest the head's own goldens carry for those two
fixtures, and `defaults/` is unchanged from `1b708ccb` to `db5efe80` and `cd1b5972`.
Bitwise at the tip (block, 7 of 7). Moved numbers: the ledger table has a physical cause
for the internal-gain leaves (heating down, cooling up, in sign and size: consumer shape
heating -13.2 %, cooling +7.4 %; full year heating -2.7 %, cooling +4.4 %); the
replay-date leaves are L8. Parity MAE moves are in D-005 to D-007 with the three
fixtures that move away from OCHRE named as class (a).

### 6. Zone-ordering and validator mutations (scratch b at `7b86f9d4`)

| Mutation | Tests that fail |
|---|---|
| zone map: `ZoneType::Attic` also claims `ZoneRole::Indoor` (`mod.rs:2494`) | `appliance_zone_gains`: `hpxml_appliance_gains_reach_the_conditioned_zone`, `hpxml_lighting_splits_convective_radiant_and_visible`, `overridden_sensible_fraction_keeps_the_radiant_and_visible_shares`, `every_gain_override_changes_the_split` (4 of 14) |
| `appliance_site` zone index `idx + 1` to `idx` (`resolve_loads.rs:1168`) | hares-io: `appliance_sites_follow_openstudio_hpxml_locations`, `appliances_give_their_heat_where_they_stand` (2); hares-core integration: none (L4) |
| validator (own-entry check off) | 3 (section 1) |
| wildcard resolver (both spellings) | 3 (section 2) |
| wildcard population check always satisfied | 2 |
| numbered-key range | 3 |
| debug assertion off | 0 (M1) |

All mutations reverted; the scratch worktree is deleted.

### 8. Shipped text

For each of the 23 commits (`1b708ccb..7b86f9d4`), the message and added lines (excluding
fixtures and lockfiles) were scanned with the block's `ids` regex plus U+2013, U+2014,
`agent`, `HARES#`, and a second pass for `red team`, `ledger`, `shipwright`, `found
defect`, `previous round`, `the review`, `auditor`. 0 matches in messages, 0 in added
lines. The `[writing]` block line would pass for every commit.

### 9. Fold onto `db5efe80` (scratch merge in worktree b; a commit-by-commit rebase was
not run)

Head since `1b708ccb`: 8 commits (`dc4a456d` fleet error, `a416b901`, `11ca3bf3`,
`1eba7760` EV mapping errors, `7ac4d4d9` schedule power errors, `9a087efa`, `f4e4ff9a`,
`db5efe80`), 22 files.

- Textual conflicts (9): `crates/hares-io/src/schedule_resolve.rs` (3 hunks),
  `crates/hares-io/tests/schedule_integration.rs`, and the 7 goldens (head's change to
  each is the digest and `git_head` lines only, and `git_dirty: true`).
- Semantic conflicts (5, compile errors, no text conflict): the head's new tests in
  `schedule_resolve.rs` (lines 2141, 2182, 2212, 2272, 2300) call
  `inject_schedule_into_specs` with the removed `foundation_name` and `garage_modeled`
  arguments (E0061). Removing the two arguments fixes all five.
- After resolving (keep the branch's schedule-column rule, take the head's `warn!` for
  the skip, drop the stale `None, false`), `cargo clippy --workspace --all-targets
  --all-features -D warnings` is clean (so `max_consecutive_step_failures`, the fleet
  error variant, the `py_dwelling.rs` and `py_equipment.rs` changes and the EV catalogue
  changes compile), and `cargo nextest run -p hares-io -p hares-core -p hares-equipment`
  passes 5468 of 5468. Not run on the merged tree: Python, goldens.
- Goldens: defaults unchanged, so the branch's numbers stand; `git_head`, `git_dirty` and
  the head's dirty-capture state must be redone by one clean recapture on the folded tree.
- `952e5486` defaults-path rule: the tip already obeys it (`defaults_path: None` gives no
  profiles, `schedule_resolve.rs:642-645`; `7b86f9d4` gives the four fleet tests the
  defaults directory because they failed at `1b708ccb`).
- `fix-headbreaks` (`cd1b5972`, 19 commits over `db5efe80`): merging it with the branch
  conflicts in `conversions.rs`, `equipment_id_regressions.rs`, `equipment.rs`,
  `schedule_resolve.rs`, `schedule_integration.rs` and the 7 goldens. Its `c71a360b`
  (garage and basement columns become warnings under the head's gate) and `215398f3`
  (unset defaults directory named in the error) collide with the branch's removal of
  the gate: re-express `c71a360b` for the branch's rule (M2). I did not compile that
  combination.

### 10. Silent paths

See L5 (the `.ok()?` and `.ok().flatten()` builders at `resolve_hvac.rs:327-370`,
`:1151`, `:1305`, `:1336` and their only caller `autosize.rs:590-591`), M2, L1, L3.

## Files

- Report: `/home/rich/src/HARES/.shipwright/initiatives/I-08/verification/review-OD-30-7b86f9d4.md`
- Code under review: `/home/rich/src/HARES/.workstrees/fix-od30` (read only)
