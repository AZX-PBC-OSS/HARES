# Verification L5-d: consumer ledger and numeric fixes

Scope: `ledger/08-consumer.md` (CN-17 and the B0 re-check, then CN-0,
CN-16, B0 and every uncommitted CN entry, CN-1 to CN-15),
`ledger/09-numeric-fixes.md` (every entry, with B-14, B-15, B-16 and
B-17 in depth), and `plan.md` §2.8. Engine read at `7b7a6a7d` in the
worktree `.workstrees/audit-7b7a6a7d`; consumer read at `5c651bc` on
`i08-consumer`. Upstream OpenStudio-HPXML read at tag `v1.12.0`.

Counts: 1 blocker, 2 major, 8 minor.

Clean, with no finding: CN-0, CN-16, B0, the B0 re-check (row 16g), CN-1,
CN-2, CN-4 to CN-15, B-1a, B-1b, B-1c, B-2 to B-13. CN-17 and CN-3 have one
minor finding each. B-15 has one major finding, carried into B-17.

## Findings

### F1. Blocker. B-14 misses two in-tree readers of the renamed names

Location: `09-numeric-fixes.md` B-14, the census (2084-2109), the
boundary classes (2111-2187), the `si_census` acceptance step (2807-2820)
and the `si_guard` scope (2610-2630).

Evidence: two files at `7b7a6a7d` use names that B-14 removes, and none
of B-14's checks reaches them. B-14 does not mention either file.

- `notebooks/simulation_explorer.py`:
  - :431 and :443 call `PV("PV", <kW>, tilt, azimuth)` with a positional
    kW capacity.
  - :460 and :465 read `bat_obj.max_charge_kw` and `bat_obj.capacity_kwh`.
  - :473 calls `Battery(..., max_charge_kw=..., max_discharge_kw=...)`.
  - :492 passes `"max_charge_kw"` in a `BatteryManagement` actor
    parameter dict, a key that commit 4 lists in `REMOVED_KEYS`.
  - :511 calls `EV(..., capacity_kwh=..., max_charging_kw=...)`.
- `benches/rl_episode.rs:35,44-45` reads the observation keys
  `"total_power_kw"` and `"reactive_power_kvar"`. Commit 3 renames both,
  and under the old names they fail.

Neither file is covered by any check:
- The `si_census` pathspecs are `crates/*/src/**` and `python/hares/**`.
- The `si_guard` scope is the `src` of each workspace crate.
- pytest `testpaths` is `tests/python`, so it never imports `notebooks/`.
- `cargo nextest run --workspace` does not build `[[bench]]` targets.

So after commit 4 the claim at 2112-2113 is false: it says every census
line falls in a boundary class or a `REMOVED_KEYS` row, but these two
files are not listed at all. Both would break only when someone runs them
by hand.

Correction:
- Name both files as call sites. The notebook moves in commits 2 and 4,
  the bench in commit 3.
- Add `notebooks/**` and `benches/**` to the `si_census` pathspecs and to
  the `si_guard` scope.
- Add `cargo bench --no-run` (or `cargo build --benches`) to the
  verification of commit 3, so the bench is at least compiled.

### F2. Major. The `si_census` self-check cannot detect a dead Python pathspec

Location: `09-numeric-fixes.md:2807-2820`.

Evidence: the command, run exactly as written in the worktree (output
redirected elsewhere, since the worktree is read-only for this audit), gives
grep exit 0, self-check exit 0, 8655 lines, and 0 lines under `python/`.
The reason is that `python/hares/` does not exist yet; the package is
`python/ochre_next/` until PY-7.

B-14 runs after PY-7, so the pathspec is right for the tree it runs on.
But `git grep` does not fail on a pathspec that matches nothing. The
self-check line only proves the Rust half matched (`ELECTRIC_KW` in
`hares-types`). The entry says at 2818-2820 that the self-check fails "a
pathspec or pattern that matches nothing". That holds for the first
pathspec only.

The census in the evidence section (2086-2088) uses `python/ochre_next/**`
at `afa9a071` and prints 8857 lines, so the Python half contributes
lines. If PY-7 lands under another path, the acceptance census silently
drops the HELICS, gym and tariff-telemetry Python readers.

Correction: add a second self-check line that requires a known Python
line, for example
`grep -qE '^python/hares/.+:[0-9]+:' target/si-census.txt`, or better a
named line that is known to stay. Then each pathspec has its own witness.

### F3. Minor. plan.md §2.8 does not list B-15, B-16 or B-17

Location: `plan.md:1672-1721`.

Evidence: §2.8 says "The decision log names exactly these". It then lists
items 1 to 12, the precedence entry B-13, B-14 and the found defects
(FL-5, OD-9, OD-2 site 6, OD-10, OD-20, IN-8, IN-9). `grep` finds no
`B-15`, `B-16` or `B-17` anywhere in `plan.md` or `decision-log.md`.

The decisions do exist in `decision-log.md:245-255`, under
`dl-i08-architecture.unavailable_periods` and
`dl-i08-architecture.wet_appliance_hot_water`. The entries exist in
`09-numeric-fixes.md` (2975, 3169, 3219), and the schedule has them at
rows 94j to 94l. The decision log is authoritative, so this is a currency
gap, not a missing authorisation.

Correction: add to the found-defects paragraph of §2.8:
- B-15, unavailable periods (`dl-i08-architecture.unavailable_periods`);
- B-16, the wet-appliance triage, recorded and with no commit;
- B-17, its contingent numeric commit
  (`dl-i08-architecture.wet_appliance_hot_water`).

### F4. Minor. Two of the three reasons for not using `maturin develop` are wrong

Location: `08-consumer.md`, engine-under-test mechanism, about line 238.

Evidence: the entry says `maturin develop` "has no `--active` flag, the
consumer venv has no `pip`, and it also installs the engine's `dev`
dependency group". `maturin develop --help` for maturin 1.15.0 (the
engine's locked version) shows two of these are wrong:
- It installs "in the current virtualenv", meaning the one named by
  `VIRTUAL_ENV`, so an `--active` flag is not needed.
- It has `--uv` ("Use `uv` to install packages instead of `pip`"), so the
  missing `pip` does not block it.

The third reason (the `dev` group is installed) is stated as measured; I
did not re-measure it. The mechanism the ledger chose keeps the main tree
out of the consumer venv: `uv pip install --editable` of the worktree
path, with `--no-deps` and the engine's locked maturin as a build
constraint. It satisfies the engine-under-test rule, so the only defect
is the stated rationale.

Correction: state only the reason that holds, namely that `maturin
develop` installs the engine's `dev` group into the target venv and
changes the consumer's `ruff check` gate. Delete the other two.

### F5. Minor. CN-17's description of how `paths.py` resolves the engine omits a fallback

Location: `08-consumer.md`, CN-17, the description of
`calibration/paths.py:24-41`.

Evidence: at `5c651bc`, `paths.py` resolves `$HARES_REPO`, then
`./HARES`, then `../HARES` (commented as the legacy sibling checkout).
CN-17 describes only the first two.

The fix CN-17 specifies is unaffected: the contract check reads
`paths.HARES_REPO` instead of `DEFAULT_REPO`. But a gate that runs
without `HARES_REPO` set can resolve `../HARES`. That is
`/home/rich/src/HARES`, the main tree.

Correction: name all three tiers. State that every gate and capture sets
`HARES_REPO` to the worktree, so neither the `../HARES` fallback nor the
`./HARES` fallback can reach the main tree.

### F6. Minor. CN-3 cites `conversions.rs` without a path

Location: `08-consumer.md:1229`.

Evidence: "HARES `conversions.rs:147-151, 240-291`". Two files have that
name: `crates/hares-core/src/dwelling/conversions.rs`, which is the one
meant, and `crates/hares-python/src/conversions.rs`.

Correction: cite `crates/hares-core/src/dwelling/conversions.rs:147-151,
240-291` and `crates/hares-io/src/envelope_lut.rs:550-590`.

### F7. Minor. HARES's therm is 14.7 J off OpenStudio-HPXML's, and B-14 keeps the difference without saying so

Location: `09-numeric-fixes.md:2100-2109` and `:2411-2416`.

Evidence: B-14 defines `therm` as 100 000 of `uom`'s `btu_it`.
- uom 0.38.0 defines `btu_it` as 1.055056e3 J (`src/si/energy.rs`), so
  the therm is 105 505 600 J.
- OpenStudio-HPXML `v1.12.0` `HPXMLtoOpenStudio/resources/unit_conversions.rb`
  defines `btu` to `j` as 1055.05585262 and `therm` to `btu` as 100000.0,
  so its therm is 105 505 585.262 J.
- The difference is 14.738 J per therm, 1.4e-7 relative. The same
  applies to `btu_it_per_hour` (0.29307107 W against 1055.05585262 / 3600
  = 0.2930710701722 W).

The entry identifies the convention correctly: it is the IT Btu, not
`therm_us` (1.054804e8 J) or `therm_ec` (1.05506e8 J). Commit 3 is a
numeric commit with a recorded delta, so it can take the exact constant
at no extra process cost. The entry instead keeps today's rounded value
"so the definitions move no value beyond its last bit", and does not say
that the result differs from OpenStudio-HPXML.

Correction: define both units from 1055.05585262 J per Btu with `unit!`
in `si_units.rs`, in commit 3, and record the delta there. The other
option is to keep the rounded value and state the 1.4e-7 difference from
OpenStudio-HPXML in the entry and in the `docs/python.md` line, with the
reason.

### F8. Minor. B-16 cites `water_heater/mod.rs:131` with no commit, and the function is now at :82

Location: `09-numeric-fixes.md:3202`.

Evidence: `read_dhw_demand_kg_s` is at line 131 at `afa9a071`,
`1d471677` and `26626710`. It is at line 82 at `b8195c83` and
`7b7a6a7d`. Every other HARES citation in B-15 to B-17 names its commit.

Correction: cite `water_heater/mod.rs:82` at `7b7a6a7d`, or keep :131 and
pin it to `afa9a071`.

### F9. Minor. B-16's figures already exceed its triage threshold

Location: `09-numeric-fixes.md:3169-3217`.

Evidence: B-16 runs B-17 when "any appliance's realised daily volume
differs from the OpenStudio-HPXML volume by more than 1%, or differs
between 900 s and 3600 s by more than 1%". This is a numeric threshold
against a formula-defined reference
(`hotwater_appliances.rb:607-675, 824-888` at `v1.12.0`, both confirmed),
so the condition on B-17 can be decided.

The entry's own figures already exceed it:

| Comparison | Values (L/d) | Difference |
|---|---|---|
| Washer, HARES 900 s vs OpenStudio-HPXML | 17.11 vs 28.19 | 39% |
| Washer, 900 s vs 3600 s | 17.11 vs 39.35 | 130% |
| Dishwasher, HARES 900 s vs OpenStudio-HPXML | 31.17 vs 14.05 | 122% |

B-17's own Problem section confirms the defects in the code at
`afa9a071`:
- `resolve_loads.rs:290` uses `scy = 164.0 + n_bedrooms * 46.5`.
- `resolve_loads.rs:463` uses `dwcpy = (88.4 + 34.9 * n_bedrooms) * ...`.
- `resolve_loads.rs:226-297` applies the `AdjustedAnnualkWh` adjustment
  twice.

The schedule calls B-17 "contingent", but the triage outcome is not in
doubt.

Correction: state in B-16 that the planning figures already exceed the
threshold, and that the recorded measurement's job is to confirm them.
Alternatively, drop the contingency and schedule B-17 as a numeric entry
of its own, keeping B-16 as its recorded pre-change measurement.

### F10. Major. B-15's zeroing mechanism has no table row for the two hot-water appliance columns B-17 activates, so a vacant home would still draw appliance hot water

Location: `09-numeric-fixes.md` B-15 (`2975-3167`), "Schedule-driven end
uses" paragraph, and B-17 (`3219-3286`), "the volume is drawn" paragraph.

Evidence: `HPXMLtoOpenStudio/resources/data/unavailable_periods.csv` at
`v1.12.0` has rows for `clothes_washer` (line 12) and `dishwasher` (line
11), both `1` under `Vacancy` and `Power Outage`. It has no row named
`hot_water_clothes_washer` or `hot_water_dishwasher`. OpenStudio-HPXML
zeroes those two draw columns anyway, but not by a table row: in
`schedules.rb` `set_unavailable_periods` (`:1421-1462`), the `case`
statement at `:1433-1442` remaps `HotWaterDishwasher` to the `Dishwasher`
schedule name and `HotWaterClothesWasher` to `ClothesWasher` before
testing `Schedule.unavailable_period_applies(schedule_name2, column_name)`,
and only then multiplies the hot-water column by `(1.0 -
@tmp_schedules[column_name][i])`. The hot-water draw column's zeroing
rides on the appliance's own energy-use row, not a row of its own.

B-15 describes its mechanism as: "`inject_schedule_into_specs` multiplies
each schedule column whose table row a period applies to by 0". Taken as
written, this reaches `clothes_washer` and `dishwasher` (they have table
rows) but not `hot_water_clothes_washer` and `hot_water_dishwasher`
(they have none), because at B-15's commit those two columns are still
`NotUsed` and nothing in B-15 adds a remapping for them. B-17 later turns
them into read, demand-driving columns (`08-consumer.md:1270-1271`) but
its own text only says the columns "stop being `NotUsed`" and describes
the peak-flow and draw-temperature conversion; it does not add the
`HotWaterDishwasher -> dishwasher`, `HotWaterClothesWasher ->
clothes_washer` remapping.

Net effect if built as described: `bldg0527060` (`Vacancy`, all year, no
schedule file, B-15's own fixture) would have its `clothes_washer` and
`dishwasher` energy-use rows zeroed by B-15, but after B-17 lands, its
generated `hot_water_clothes_washer` and `hot_water_dishwasher` columns
would carry the normal (non-zeroed) appliance-schedule shape, so the
vacant home keeps drawing wash and dishwasher hot water for a full year.
This is the same class of defect B-15 exists to fix, reintroduced by B-17
for the two columns B-17 itself activates.

Correction: add the remapping explicitly to B-15 (so it is in place
before B-17 needs it, matching the dependency order B-17 already states)
or to B-17's change list: when zeroing schedule columns under a period,
`hot_water_clothes_washer` follows `clothes_washer`'s applicability and
`hot_water_dishwasher` follows `dishwasher`'s, the same two-step lookup
OpenStudio-HPXML uses. Add an acceptance test on `bldg0527060` once B-17
lands: both hot-water draw columns are 0 for the full vacancy period.

### F11. Minor. B-17's "default appliance hot-water profiles" overstates what OpenStudio-HPXML has

Location: `09-numeric-fixes.md:3240-3242`.

Evidence: B-17 says the schedule generator produces the two columns "from
OpenStudio-HPXML's default appliance hot-water profiles." OpenStudio-HPXML
has no separate hot-water profile table for these appliances. Without a
detailed schedule file, `hotwater_appliances.rb` builds one
`MonthWeekdayWeekendSchedule` per appliance from the appliance's own
`weekday_fractions`/`weekend_fractions`/`monthly_multipliers` (defaulted
in `resources/defaults.rb` when absent from the HPXML), uses it to size
the electric design level from the daily kWh (`:63-65`, `:155-157`), and
reuses the same schedule object's `calc_design_level_from_daily_gpm` to
size the water draw from the daily gallons (`:480-482, 517-519`). The
shape is one schedule, shared between the appliance's energy and water
draw; there is no distinct "hot-water profile."

Correction: state that the generated `hot_water_clothes_washer` and
`hot_water_dishwasher` columns reuse the same weekday/weekend/monthly
fractions already generated for `clothes_washer` and `dishwasher`, scaled
to the appliance's daily water volume instead of its daily energy, not a
separate default profile.

## Verified without finding

CN-17 and the B0 re-check:
- Every cited consumer line matches at the commit it names
  (`hares_contract_check.py:27,30-32,55-58,142-144`,
  `build_bundle.py:173-174` at `aec2e3f`, `build_bundle.py:344,384-391`,
  `suite_calibration.py:264-278`, `suite_coverage.py:23-31`,
  `schema.py:508-550`).
- The uv flags exist in uv 0.12.8.
- The eight re-check commits exist, are ancestors of `7b7a6a7d`, and are
  in row order.
- `75a182a0..8b09ac4a` is exactly one commit, so the bisection range
  `1d60f478..75a182a0` covers rows 1 to 12.
- Row 16g, CN-17 and `plan.md` §3 describe the same commit list.
- CN-17's gate-engine time matches the `7b7a6a7d` commit time.
- B0's tree ids and the `0ec757cb`..`1d60f478` diff stat (2 files,
  +18/-4) reproduce.

CN-1 to CN-15:
- Citations match at `c71c542` and `5c651bc`.
- Every dependency sits in an earlier schedule row.
- Acceptance tests name a home, a seed and a grid, or are pure arithmetic
  or monkeypatched.
- CN-15 is the one entry gated on the pushed pin, by its own statement.
  Every other gate goes through the worktree mechanism.

B-1a to B-13: the citations sampled match the worktree; line drift
against `ebdb1a17` is expected by the ledger's citation rule. Each entry
has its own commit and row, a recorded delta and a `docs/python.md`
line. Acceptance tests are built from explicit series or named fixtures.
No entry targets an OCHRE defect.

B-14, apart from F1, F2 and F7:
- The fields cited at 7b7a6a7d still hold their old names
  (`pv/config.rs:36,50`, `battery/config.rs:20-22`,
  `ev/config.rs:381,385`), and `units.rs:118-165` matches the cited
  constants.
- The uom 0.38.0 contents are as stated: no plain therm, no Btu/h, no
  apparent or reactive power quantity, and `unit!` available.
- Every engine-facing reader of a renamed quantity in the consumer is
  named in CN-3: `hares_der_remeasure.py:85`, `der_inject.py:157-180`,
  `ami2/dr_actuation.py:257`, `cosim.py:81,94,107`,
  `coupling.py:393,412`, `coverage.py:106` and `hares_contract.py`.
  The consumer's own kW dataclasses stay as reporting types.

B-15, apart from F10:
- Every cited upstream line was confirmed at `v1.12.0`:
  `unavailable_periods.csv:1-40`, `schedules.rb:700-724, 815, 1375-1408,
  1438-1461`, `hvac.rb:42-71, 4536-4573`, `waterheater.rb`,
  `hpxml.rb:1365-1448`, `defaults.rb:229-241` and
  `workflow_inputs.rst:282-312`. These cover:
  - the end-use set of each period type;
  - the closed-day fill and year wrap;
  - the water heater held at 2 C;
  - setpoints untouched, load fractions zeroed;
  - unknown columns failing the run.
- Fixture bldg0176227 matches the ledger:
  - `home.xml:309-320` has one `No Space Heating` period, 10/4 hour 0 to
    10/4 hour 24;
  - `in.schedules.csv` column `No Space Heating` has exactly 96 flagged
    rows, 0-indexed 26496 to 26591;
  - `NumberofResidents` is 5.0.
- The five fixture homes with periods match a `grep` of
  `tests/fixtures/resstock/2025.1`.
- The HARES citations at `afa9a071` and `b8195c83` match.

B-17, apart from F10 and F11:
- The cited upstream lines were confirmed at `v1.12.0`:
  `hotwater_appliances.rb:627-637, 842-850`,
  `defaults.rb:5523-5530, 5586-5594` and `schedules.rb:1353-1365`.
- The acceptance fixtures exist and carry both appliances.

## Testing gaps

- B-17's four expected daily volumes have not been recomputed from the
  fixtures' inputs: `cz4a_ashp_hpwh` washer 2.584 and dishwasher
  2.563 gal/d, `bldg0176227` washer 7.448 and dishwasher 3.711 gal/d.
  The formulas and defaults they come from are confirmed; the numbers are
  not.
- `vendors/OCHRE/ochre/utils/schedule.py:57-69` (OCHRE ignores the four
  period types, cited in B-15) has not been read.
- No consumer gate or capture was run. CN-17 and the B0 re-check are not
  yet executed, and this verification checked their commands and
  citations only.
