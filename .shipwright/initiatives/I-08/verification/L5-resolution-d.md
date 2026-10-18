# L5-d resolution: consumer ledger and numeric fixes

Resolves every finding of `verification/verify-L5-d.md` (1 blocker, 2
major, 8 minor). Edits are confined to `ledger/08-consumer.md`,
`ledger/09-numeric-fixes.md` and `plan.md` §2.8. Engine checked in
`.workstrees/audit-7b7a6a7d` (`7b7a6a7d`); consumer checked at
`/home/rich/src/HARES_CALIBRATION_SYSTEM` (read only); upstream
OpenStudio-HPXML checked at tag `v1.12.0` by direct fetch of the source
files, not inference.

## Census coverage proof (F1)

Ran the census pathspecs against `git ls-files` in the worktree. Outside
`crates/*/src/**`, `python/hares/**` (not yet created; see F2), the
directories named in the task are:

| Directory | Tracked files | Contain a census-pattern hit? |
|---|---|---|
| `notebooks/` | 1 (`simulation_explorer.py`) | yes, 48 lines, including live calls to renamed constructors/keys |
| `benches/` | 6 | yes in 2 (`aggregation.rs`, `rl_episode.rs`), 14 lines |
| `examples/` | 0 (directory does not exist) | n/a |
| `tools/` | 20, all under `tools/frame_golden` (a workspace member, `Cargo.toml` `members`) | yes in 4 files, 57 lines, every one a boundary/reporting name already in "Boundaries that keep their units" (`TotalEnergyKwh`, `PeakPowerKw`, `peak_import_kw`, `peak_15min_kw`, `envelope_loads_kwh`, …) — confirmed by reading every hit; none is a removed key |
| `scripts/` | 65 (`.py`, `.sh`, `.json`, 1 `.txt`) | yes in 5 `.py`/`.json` files, 19 lines, all local variable/dict-key names (`p_kw`, `annual_heating_load_kwh`) or JSON review manifests, none calling a HARES renamed API — confirmed by reading every hit |
| `docs/` | 353 `.md` (+1 `.py` test helper) | yes, 738 lines across ~140 files, all English prose in review/ticket/architecture writing, not executed or compiled code |

Corrections made:
- `notebooks/**` and `benches/**` are added to the `si_census` pathspecs
  and the `si_guard` scope, each with its own self-check witness line
  (`09-numeric-fixes.md`, the `si_census` acceptance block and the
  `si_guard` paragraph). Ran the rewritten census command at `7b7a6a7d`:
  8717 lines, and all three witnesses that can match at this pre-PY-7
  commit matched (`crates` witness, `notebooks` witness, `benches`
  witness); the `python/hares/**` witness correctly did not match,
  because `python/hares/` does not exist until PY-7 — proving the witness
  design actually distinguishes a dead pathspec from a live one instead of
  passing silently either way.
- `cargo bench --no-run` is added to commit 3's Verification, so
  `benches/rl_episode.rs` (which reads the renamed `total_power_kw` and
  `reactive_power_kvar`) is compiled by a step the verification block
  runs, not only by a developer running benches by hand.
- `tools/frame_golden` needs no new pathspec: it is a workspace member
  (`Cargo.toml` `members`), already covered when `si_guard`'s scope widens
  to "the `src` of every workspace crate" (unchanged by this resolution),
  and its `tests/` directory is compiled and run by `cargo nextest run
  --workspace`. Every hit in it is a boundary name today, stated and
  cited in the resolution, not a finding.
- `scripts/*.py` and `scripts/reviews/*.json` are excluded with the
  stated reason above: read every hit; none calls a renamed HARES API, so
  none breaks on a rename. Scripts that do call the engine (none do,
  among the hits) would need the same treatment as notebooks if that ever
  changes; this is noted in-place rather than spuriously widening the
  scan to data manifests.
- `docs/**` is excluded: it is prose, not parsed by `si_guard` (`syn`) and
  not executed, so it cannot break; it is handled by the per-commit
  "Numeric changes" / `docs/python.md` process already in the ledger, a
  different mechanism for a different failure mode (stale documentation,
  not a silent runtime break).

## Findings

**F1 (blocker).** Fixed. `09-numeric-fixes.md`: named both call sites
(`notebooks/simulation_explorer.py`, `benches/rl_episode.rs`) right after
the census command's explanation; added `':(glob)notebooks/**'` and
`':(glob)benches/**'` to the `si_census` command; widened the `si_guard`
scope paragraph to include `notebooks/` and `benches/`; added `cargo bench
--no-run` to commit 3's Verification line. Quote (new): "Two in-tree
readers of a renamed name sit outside `crates/*/src` and `python/hares`
and are not reached by this census, `si_guard`, pytest's `testpaths`
... or `cargo nextest run --workspace` ... notebooks/simulation_explorer.py
... and benches/rl_episode.rs."

**F2 (major).** Fixed. Added one witness line per pathspec to the
`si_census` acceptance step (`grep -qE` for a known line under
`python/hares/`, `notebooks/` and `benches/`, beside the existing
`crates` witness), and rewrote the surrounding prose to state the general
rule ("a pathspec that matches nothing... passes the step silently
instead of failing it") rather than the narrower claim the verification
found false. Stated explicitly which Python pathspec applies when: the
`python/ochre_next/**` measurement in the evidence paragraph predates PY-7
(schedule row 26) and is only illustrative; B-14's own acceptance step
runs at row 94c or later, after PY-7, so `python/hares/**` is the live
pathspec when the step actually executes. Confirmed the self-check design
against the real tree (see census proof above): at `7b7a6a7d` the
`python/hares/**` witness correctly fails to match (package not yet
renamed) while the other three witnesses match, which is exactly the
discriminating behavior F2 required and the old single self-check could
not provide.

**F3 (minor).** Fixed. `plan.md` §2.8's found-defects sentence now names
B-15, B-16 and B-17 with their decision-log keys, matching
`decision-log.md:245-255` and the schedule rows 94j-94l.

**F4 (minor).** Fixed. `08-consumer.md`'s `maturin develop` rationale now
states only the true reason (the engine's `dev` dependency group installs
into the consumer venv and changes its `ruff check` gate) and records,
parenthetically, that `--help` at 1.15.0 shows "the current virtualenv"
(the one `VIRTUAL_ENV` names, no `--active` needed) and a `--uv` flag (no
`pip` needed), so neither is a real blocker. Checked against a locally
installed `maturin` (1.12.6, not 1.15.0, but the flag set and "current
virtualenv" wording match what the finding reports for 1.15.0): `maturin
develop --help` confirms no `--active` flag and a `--uv` flag exist.

**F5 (minor).** Fixed. Read `calibration/paths.py:24-41` directly: the
resolution order is `$HARES_REPO`, then `./HARES` (`REPO_ROOT / "HARES"`
if it has a `Cargo.toml`), then `REPO_ROOT.parent / "HARES"` — on this
host, `/home/rich/src/HARES`, the main tree. `08-consumer.md`'s CN-17
problem statement and the mechanism's Step 5 rationale now name all three
tiers and state that every gate and capture step sets `HARES_REPO` to the
worktree, so neither fallback is reached.

**F6 (minor).** Fixed. CN-3's citation is now
"`crates/hares-core/src/dwelling/conversions.rs:147-151, 240-291` (not
`crates/hares-python/src/conversions.rs`, a different file of the same
name)". Verified both files exist at those paths in the worktree.

**F7 (minor).** Fixed, keeping the rounded value (the option the
constitution's bitwise-neutrality requirement for the SI migration
commits picks: B-14's commit 3 is a naming/unit-representation commit, not
one of the project's declared numeric commits). `09-numeric-fixes.md` now
states the exact delta (HARES's therm 105 505 600 J against
OpenStudio-HPXML v1.12.0's 105 505 585.262 J, `unit_conversions.rb`,
100 000 × 1055.05585262 J; 14.738 J, 1.4e-7 relative; the same ratio for
`btu_it_per_hour`), why HARES keeps today's factor in this commit
(bitwise neutrality), and that adopting the exact upstream constant is a
numeric change requiring its own commit. The `docs/python.md` "Numeric
changes" line requirement for commit 3 now names this disclosure
explicitly.

**F8 (minor).** Fixed. Checked `water_heater/mod.rs` across the cited
commits: `read_dhw_demand_kg_s` is at line 131 through `26626710` and at
line 82 from `b8195c83` on, including `7b7a6a7d`. B-16's citation now
reads "`water_heater/mod.rs:82` at `7b7a6a7d`".

**F9 (minor).** Fixed with the "state it" option (the "drop the
contingency" alternative would mean renumbering `00-schedule.md`, outside
this resolution's file scope). B-16 now states the planning deltas as
percentages (39%, 130%, 122%, 208%, 180%, all well past the 1% triage
threshold) and says plainly that the recorded measurement's job is to
confirm an already-expected outcome, not to decide a close call.

**F10 (major).** Fixed. Confirmed upstream's exact mechanism by fetching
`HPXMLtoOpenStudio/resources/schedules.rb` at `v1.12.0` directly (not from
memory): `set_unavailable_periods` (method body 1421-1462, `case`
statement 1433-1442) remaps `HotWaterDishwasher` to `Dishwasher` and
`HotWaterClothesWasher` to `ClothesWasher` before testing
`unavailable_period_applies` on the remapped name, then multiplies the
*original* (hot-water) column by `(1.0 - flag)`. Added this exact
mechanism to B-17's "The volume is drawn" paragraph (the natural home,
since the two columns are `NotUsed` until B-17 activates them — B-15
cannot usefully carry a remapping for columns it does not yet read) and
added the acceptance test `vacant_home_draws_no_appliance_hot_water` on
`bldg0527060` (B-15's own vacancy fixture): both hot-water columns are 0
for the full run and the water heater's realised draw from both
appliances is 0.

**F11 (minor).** Fixed in the same edit as F10. B-17 no longer claims
"OpenStudio-HPXML's default appliance hot-water profiles"; it now states
that the generated columns reuse the same `MonthWeekdayWeekendSchedule`
object already built for `clothes_washer`/`dishwasher` from their own
`weekday_fractions`/`weekend_fractions`/`monthly_multipliers`
(`hotwater_appliances.rb:63-65, 155-157` size the electric design level
from daily kWh; `:480-482, 517-519` reuse the same object's
`calc_design_level_from_daily_gpm` to size the water draw from daily
gallons), scaled to water volume instead of energy — confirmed these four
line ranges against the fetched upstream source; there is no separate
profile table.

## Testing gaps closed

- **B-17's four daily-volume constants**: recomputed by hand from the
  fixtures and the fetched upstream formulas
  (`calc_clothes_washer_energy_gpd`/`calc_dishwasher_energy_gpd`,
  `hotwater_appliances.rb:607-675, 824-888`). Both fixtures give
  `NumberofResidents` explicitly, so the operational branch applies
  (`scy = 123 + 61·n_occ` washer, `91 + 30·n_occ` dishwasher). Result:
  `cz4a_ashp_hpwh` washer ≈ 2.583-2.584 gal/d, dishwasher ≈ 2.563 gal/d;
  `bldg0176227` washer ≈ 7.449 gal/d (×1.2 `UsageMultiplier`), dishwasher
  ≈ 3.711-3.712 gal/d (×1.2). All four match the ledger's stated
  constants (2.584, 2.563, 7.448, 3.711) to the printed digits — no
  correction was needed to the numbers themselves, only to the
  surrounding text (F10, F11). B-16's L/d planning figures (28.19, 14.05,
  9.78…) are the same gpd figures times 3.78541 L/gal and also check out.
- **OCHRE's `schedule.py:57-69`**: read directly. Confirmed: under
  `SCHEDULE_NAMES["Ignore"]`, `"Vacancy"`, `"Power Outage"`, `"No Space
  Heating"` and `"No Space Cooling"` all map to `None`, i.e. OCHRE drops
  all four period types, matching B-15's claim exactly.

## Counts

11 findings resolved (1 blocker, 2 major, 8 minor). 2 testing gaps closed
(B-17's constants recomputed and confirmed correct; OCHRE's
`schedule.py:57-69` read and confirmed). 1 testing gap remains open and
out of this resolution's scope: "No consumer gate or capture was run" —
running `CN-17`/the B0 re-check requires executing the consumer gate
mechanism, not a ledger edit.

Files edited: `ledger/08-consumer.md`, `ledger/09-numeric-fixes.md`,
`plan.md` (§2.8 only). No repository code was edited; no destructive git
operation was run.
