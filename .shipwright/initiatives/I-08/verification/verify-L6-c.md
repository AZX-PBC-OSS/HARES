# Verification L6-c: consumer ledger and numeric fixes (B-14/B-15/B-16/B-17)

Scope: `ledger/08-consumer.md` (every uncommitted CN entry: CN-17 and the
B0 re-check first, then CN-1 through CN-15; CN-0 and CN-16 are committed
and read for context only) and `ledger/09-numeric-fixes.md` (B-14 in full
depth, including its SI migration census and unit constants; B-15, B-16,
B-17 in depth), against `plan.md` §2.8 and §3, in `00-schedule.md` order.

Entries read in full: CN-0, CN-16, CN-17, B0, B0 re-check, CN-1 through
CN-15 (all of `08-consumer.md`); B-14 (all four commits), B-15, B-16, B-17
(all of the relevant section of `09-numeric-fixes.md`). `plan.md` §3
(execution rules, verification block, no-regressions rules) and
`00-schedule.md`'s ordering text were read for the governing rules.

Counts: 2 committed CN entries read for context (CN-0, CN-16); 16
uncommitted consumer-ledger steps read in full depth (CN-17, the B0
re-check, CN-1 to CN-15); 4 numeric entries read in full depth (B-14's
four commits, B-15, B-16, B-17).

Verification performed: read every line of both ledger files in the
stated scope; cross-checked commit hashes against both repositories'
actual history (`git cat-file -e`, `git log`); re-ran B-14's commit-2 SI
census command verbatim against the HARES commit it is evidenced at
(`afa9a071`, via a worktree); inspected the pinned `uom` 0.38.0 source and
HARES's `crates/hares-physics/src/units.rs` for the Btu/therm constants;
fetched OpenStudio-HPXML v1.12.0's `unit_conversions.rb`, `schedules.rb`
and `hotwater_appliances.rb` from GitHub to check B-14's, B-15's and
B-17's cited factors, line behaviour and formulas; inspected the
consumer's `hares_contract_check.py`, `paths.py` and `build_bundle.py` at
HEAD (`5c651bc`) to check CN-17's problem statement; grepped the HARES
tree for stale documentation and for readers outside B-14's census
pathspecs.

Not independently re-run: the full `plan.md` §3 verification block
(`cargo nextest`, `cargo clippy`, `cargo deny`, `frame_golden
compare-all`, the consumer gate, the DE-trace capture/compare) — these
require building the Rust workspace and the consumer's Python
environment, which was out of scope for a static ledger review of
uncommitted entries (there is no commit yet to run the block against for
CN-17 onward or for B-14/B-15/B-16/B-17). The four B-17 daily hot-water
volume constants (2.584, 2.563, 7.448, 3.711 gal/d) were not
independently recomputed from the cited fixtures and OpenStudio-HPXML
formulas; this is reported as a gap below, not a finding.

## Findings

### Major: B-14 omits live engine documentation from its doc-update list, which the SI rename leaves wrong

Entry/evidence: `ledger/09-numeric-fixes.md` B-14, "Change, commit 2" and
"commit 3" doc-update bullets, which name only `docs/equipment/*.md`,
`docs/PHYSICS_DECISIONS.md` and `docs/python.md`.

`docs/architecture.md:160,177-178` (at `/home/rich/src/HARES/.workstrees/audit-7b7a6a7d`)
documents the control-signal enum with the pre-migration field names:

```
Electrical { active_power_kw, reactive_power_kvar },
...
PowerSetpoint     { active_power_kw: f64, reactive_power_kvar: Option<f64> },
PowerLimit        { max_power_kw: f64, ramp_rate_kw_per_s: Option<f64> },
```

`docs/invariants-and-observability.md:58,70,134-135,258,264,305-306,312-313,317,388`
documents `electrical_solver.net_active_kw()`, `net_reactive_kvar()`,
`electrical_net_kw`/`electrical_net_kvar` CSV headers, and
`electrical_load_kw`/`electrical_gen_kw` observer fields — exactly the
names and the specific field B-14 itself calls out as "wrong today" (the
observer fields that hold W under kW names) and renames in commit 3.
`docs/outage-behavior.md:117-119,131-132,134` documents
`island_unserved_kw`/`island_excess_kw`, which B-14 commit 3 renames to
`island_unserved_w`/`island_excess_w`.

These three files are live reference documentation, not dated audit
records: `docs/architecture.md` opens with a present-tense description of
the workspace, and `docs/invariants-and-observability.md` and
`docs/outage-behavior.md` describe current runtime behaviour with no
date stamp, in contrast to `docs/findings/*.md` and
`docs/reviews/agents/*.md`, which are dated point-in-time review records
(for example `docs/reviews/agents/agents-05-dr-compliance-actor.md:4`,
"**Date**: 2026-05-26") and are reasonably out of scope, the same way
`08-consumer.md` CN-13 exempts dated records of what was true when
written.

The task's instruction is explicit on this point: B-14 "excludes docs/ as
prose," but user-facing documentation that names a renamed parameter or
unit and would be left wrong after the migration is a gap the entry must
close. These three files meet that bar and are not in B-14's Change
section for any of its three numeric commits. Correction: B-14 commit 3
(run state) should add `docs/architecture.md` and
`docs/invariants-and-observability.md` to its doc-update bullet (the
renamed enum fields, the solver getters, the CSV headers, the observer
field names); commit 3 should also update `docs/outage-behavior.md`'s
`island_unserved_kw`/`island_excess_kw` references. (`docs/architecture.md`'s
`PowerLimit.ramp_rate_kw_per_s` is renamed by B-14 commit 4, so that one
line also needs commit 4's doc bullet.)

### Major: B-14's claim that the Btu/h constant carries "the same relative difference" from OpenStudio-HPXML as the therm constant is numerically wrong

Entry/evidence: `ledger/09-numeric-fixes.md` B-14, "Evidence" paragraph
("HARES's therm ... is 14.738 J larger than OpenStudio-HPXML v1.12.0's
... 1.4e-7 relative; `btu_it_per_hour` carries the same relative
difference (0.29307107 W against 0.2930710701722 W)") and the repeated
claim under commit 3's acceptance tests
("`therm_and_btu_per_hour_units_keep_the_parent_factors`... HARES's therm
and `btu_it_per_hour` are 1.4e-7 relative larger than OpenStudio-HPXML
v1.12.0's").

Verified constants: OpenStudio-HPXML v1.12.0
`HPXMLtoOpenStudio/resources/unit_conversions.rb` gives `['btu', 'j'] =>
1055.05585262` and `['btu/hr', 'w'] => 0.2930710701722222`. HARES's
current constant (`crates/hares-physics/src/units.rs:118`, confirmed in
the audit worktree) is `const BTU_PER_HOUR_TO_WATT: f64 = 0.293_071_07;`.

The therm delta is correctly computed: HARES's therm (100 000 x uom's
rounded `btu_it`, 1055.056 J, confirmed in `uom-0.38.0/src/si/energy.rs:62`,
`@btu_it: 1.055_056_E3`) is 105 505 600 J against OpenStudio-HPXML's
105 505 585.262 J, a difference of 14.738 J, 14.738 / 105 505 585.262 =
1.397e-7 relative — this matches the entry's "1.4e-7" claim exactly.

The Btu/h delta does not match: 0.2930710701722 - 0.29307107 =
1.722e-10, and 1.722e-10 / 0.2930710701722 = 5.88e-10 relative — about
237 times smaller than the therm's 1.4e-7, not "the same relative
difference." HARES's existing `BTU_PER_HOUR_TO_WATT` constant is in fact
already within 6e-10 of OpenStudio-HPXML's exact value (it was evidently
hand-tuned to the correct value independently of uom's rounded `btu_it`,
unlike the therm, which is 100 000x uom's rounded constant). This does
not change any test outcome (the cited acceptance test only checks that
the new `btu_it_per_hour` unit reproduces today's hardcoded constant
bit-for-bit, which it does by construction, not that the constant is a
fixed distance from OpenStudio-HPXML), but it is a specific, checkable
numeric claim the entry gets wrong, and it is slated to be written
verbatim into `docs/PHYSICS_DECISIONS.md` ("this commit's line ... also
states that HARES's therm and `btu_it_per_hour` are 1.4e-7 relative
larger than OpenStudio-HPXML v1.12.0's"). Correction: state the two
deltas separately — therm +14.738 J (1.4e-7 relative), `btu_it_per_hour`
+1.722e-10 W (5.9e-10 relative) — in both the ledger entry and the
`docs/PHYSICS_DECISIONS.md` text it specifies.

### Minor: CN-17's "a different ruff" measurement is stale against the current locks

Entry/evidence: `ledger/08-consumer.md` CN-17, mechanism step 3
commentary ("measured: pytest, maturin, psychrolib and a different
ruff"). At the current locks, HARES's `dev` dependency group
(`uv.lock` at the audit worktree) resolves `ruff` to `0.16.10`
(`extra == 'dev'`, specifier `>=1.15.0`... `ruff >= 0.16.10`), and the
consumer's own lock (`HARES_CALIBRATION_SYSTEM/uv.lock`) also resolves
`ruff` to `0.16.10` (its own specifier is `>=0.16.7`). The two locks
presently name the same ruff release, so "a different ruff" is not
currently true. This is a measurement that drifts with both repositories'
lockfiles and does not undermine the mechanism's actual reason for
avoiding `maturin develop` (it still installs `pytest`, `maturin` and
`psychrolib` into the consumer venv, which the consumer's own lock does
not want), so it is reported as a staleness note, not a defect in the
mechanism itself. Correction: reword the parenthetical to state the
reason (it installs the engine's whole `dev` group, not just the engine)
without naming a version relationship that can silently stop being true.

## Confirmed correct (high confidence)

- CN-0 and CN-16 commit hashes (`824805e`, `a6d1345`) and CN-17's named
  follow-up commits (`f7131d7`, `aec2e3f`) all exist in
  `~/src/HARES_CALIBRATION_SYSTEM` with commit messages matching the
  ledger's description; the consumer HEAD (`5c651bc`) matches the state
  `08-consumer.md` describes consumer entries as building on.
- Every HARES commit hash the consumer entries and `00-schedule.md`'s row
  order name (`1d60f478`, `0ec757cb`, `b8195c83`, `7b7a6a7d`, `afa9a071`,
  `26626710`, `21d90588`, `d7f0e381`, `1d471677`, `8b09ac4a`, `75a182a0`)
  exists in `~/src/HARES` and the audit worktree's HEAD (`7b7a6a7d`)
  matches the ledger's stated SL-7 commit.
- The maturin version pin: both `~/src/HARES`'s `uv.lock` (worktree and
  main tree) and `~/src/HARES_CALIBRATION_SYSTEM`'s `uv.lock` pin maturin
  `1.15.0`, matching CN-17's mechanism step 3 and the task's note about
  "maturin 1.15.0's `develop`."
- `maturin develop` pulling in the engine's whole `dev` dependency group
  (`pytest`, `maturin`, `psychrolib`, `pytest-cov`, `pytest-timeout`,
  `pytest-xdist`, `ruff`) is confirmed from `uv.lock`'s
  `[package.dev-dependencies]` table for `ochre-next` at the audit
  worktree, supporting the mechanism's reason for using
  `uv pip install --editable` instead.
- CN-17's problem statement about `hares_contract_check.py` is confirmed
  verbatim against the consumer source at HEAD: `DEFAULT_REPO =
  "/home/claude/hares_src/HARES-initial-implementation"`, `_repo()`
  returns `None` when that path's `crates` directory is absent, and
  `check()` prints `[hares-contract] SKIP ...` and returns 0 in that case
  — exactly as cited. `calibration/paths.py`'s resolution order
  ($HARES_REPO, then the pinned submodule, then the legacy sibling
  `../HARES`) also matches the citation exactly.
- CN-17's claim that `build_bundle.py`'s `main()` takes no arguments and
  always writes the zip beside the repository root with a date-stamped
  name, removing any existing file of that name, is confirmed at the
  cited lines.
- B-14 commit-2's SI census command, run verbatim in a worktree of
  `afa9a071` (`git grep -niE "$P" afa9a071 -- ':(glob)crates/*/src/**'
  ':(glob)python/ochre_next/**'`), prints exactly 8857 lines across
  exactly 132 distinct files, matching the entry's stated counts exactly,
  and the witness line
  (`crates/hares-types/src/telemetry_keys.rs:22: ... ELECTRIC_KW ...`) is
  present as claimed.
- B-14's unit constants: uom 0.38.0 defines `btu_it` as `1.055_056_E3` J
  (confirmed in `uom-0.38.0/src/si/energy.rs:62`), matching the entry's
  "a rounded btu_it (1055.056 J)"; HARES's current
  `BTU_PER_HOUR_TO_WATT = 0.293_071_07` (confirmed in
  `crates/hares-physics/src/units.rs:118`) matches the entry's stated
  value.
- B-15's unavailable-periods remap: OpenStudio-HPXML v1.12.0
  `HPXMLtoOpenStudio/resources/schedules.rb`'s `set_unavailable_periods`
  (around lines 1397-1448) contains the exact `case` statement B-17 cites
  for the two hot-water columns (`HotWaterDishwasher` -> `Dishwasher`,
  `HotWaterClothesWasher` -> `ClothesWasher`) before testing
  `unavailable_period_applies` on the remapped name.
- B-15's table citation: `HPXMLtoOpenStudio/resources/data/unavailable_periods.csv`
  at v1.12.0 is exactly 40 lines (1 header + 39 data rows), matching the
  entry's `unavailable_periods.csv:1-40` line-range citation; the
  Vacancy/Power Outage flag sets the entry summarizes (occupants and
  general water use excluded from Power Outage; refrigerators, freezers,
  space heating/cooling, the water heater, the dehumidifier, mechanical
  ventilation and battery charging/discharging added to Power Outage
  beyond Vacancy) match the fetched CSV's columns row by row.
- B-17's operational cycles-per-year formulas: OpenStudio-HPXML v1.12.0
  `HPXMLtoOpenStudio/resources/hotwater_appliances.rb`'s operational
  branches give `scy = 123.0 + 61.0 * n_occ` (clothes washer) and
  `scy = 91.0 + 30.0 * n_occ` (dishwasher), matching B-17's
  "`123 + 61 n_occ`" and "`91 + 30 n_occ`" citation exactly.
- B-14's census pathspec scoping is sound where checked: the top-level
  `tools/frame_golden` reads of `kw`/`kwh`-suffixed names (57 hits, not
  covered by the census's `crates/*/src/**` pathspec) are all reads of
  `RunMetrics` output fields (`total_energy_kwh`, `peak_power_kw`,
  `grid_interaction_metrics.peak_import_kw`, `envelope_loads_kwh` and
  its per-component fields), which B-14's own "Boundaries that keep
  their units" rule explicitly exempts as converted-once output records;
  this is not a gap. `scripts/`'s matches are an unrelated battery-cell
  script and an ASHRAE 140 reference script with their own local naming,
  not readers of HARES's renamed quantities. `crates/hares-core/examples/profile_sim.rs`,
  which CN-17 names, was deleted by `d7f0e381` (before `afa9a071`) and so
  correctly does not appear in the commit-2 census.

## Residual risk / scope not completed

- The consumer-ledger entries CN-1 through CN-15 were read in full and
  cross-checked for internal consistency and against a sample of their
  cited consumer-source line ranges (the ones above), but not every
  individual `file:line` citation across all fifteen entries (several
  hundred citations) was independently re-verified against
  `~/src/HARES_CALIBRATION_SYSTEM` source at `5c651bc`. No contradiction
  was found in the citations spot-checked.
- The four B-17 daily hot-water volume constants (2.584 gal/d and
  2.563 gal/d for `cz4a_ashp_hpwh`; 7.448 gal/d and 3.711 gal/d for
  `bldg0176227`) were not independently recomputed end-to-end from the
  fixture HPXML files and OpenStudio-HPXML's full `calc_clothes_washer_energy_gpd`
  / `calc_dishwasher_energy_gpd` formulas (which branch on IMEF/MEF,
  rated annual kWh, capacity, label usage and energy/gas rates in ways
  that need the fixtures' full appliance element sets to evaluate). This
  is a verification gap, not a finding against the entry: the formulas
  and the operational cycles-per-year equations that feed them were
  confirmed accurate (above), but the four printed digits are unverified.
- B-16 is a measurement/triage entry whose own text marks its planning
  figures "to be re-measured"; it was read in full but its arithmetic was
  not re-derived, consistent with its own stated status.
- `plan.md` §3's verification block (fmt, clippy, nextest across every
  feature combination, `cargo deny`, `frame_golden compare-all`, the
  consumer gate and DE-trace gate) was not run for any of the uncommitted
  entries in scope, since none of them has landed a commit yet; there is
  nothing to gate against beyond the static checks performed above.

## Summary

Entries read: 2 committed (context only) + 16 uncommitted consumer-ledger
steps + 4 numeric entries, full depth as scoped.

Findings: 2 major, 1 minor, 0 blockers.

- Major: B-14 leaves `docs/architecture.md`, `docs/invariants-and-observability.md`
  and `docs/outage-behavior.md` (live, undated reference documentation
  naming renamed fields, getters and CSV headers) off its doc-update
  list, so these three files are wrong after the migration lands.
- Major: B-14's claim that its Btu/h unit constant "carries the same
  relative difference" from OpenStudio-HPXML as its therm constant is
  numerically incorrect by a factor of about 237 (5.9e-10 relative
  against the claimed 1.4e-7); the therm's own 1.4e-7 figure is correct.
- Minor: CN-17's "a different ruff" measurement no longer holds against
  the current lockfiles on either side (both presently resolve
  `ruff==0.16.10`); the mechanism's underlying reason to avoid `maturin
  develop` still holds without that detail.

No blockers found. Everything independently checked against external
sources (OpenStudio-HPXML v1.12.0, `uom` 0.38.0, both repositories' git
history and lockfiles, and a live re-run of B-14's SI census command)
matched the ledger's citations exactly except the two major findings
above.
