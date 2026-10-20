# Review and red team: fix-or18c (9ba22b42, 680b98ca, afcd981a, b68a8aca on 5d66dafc)

Verdict: CHANGES-REQUIRED

Counts: 0 blockers, 0 majors, 3 mediums, 7 lows, 6 infos (plus 2
pre-existing defects found on the way, listed separately, not counted).

Every finding of `review-OR-18-7aaaf53e.md` (Part A) and OD-31 (Part B) is
fixed in code, and every hunk with a behaviour of its own fails a test
when it is reverted, except the derivation rules in M1 and the two lows
L1 and L2. The physics holds against OS-HPXML v1.12.0 and EnergyPlus
v25.1.0. The verification block is green apart from failures that come
from the environment or from defects already ledgered (I1 to I3). Goldens
compare identical in all three modes, and the physics suites' metrics
are byte-identical to `5d66dafc`. M1 matches the standard the last review
used for CHANGES-REQUIRED: rules with no test that pins them. M2 is the
physics finding that matters most for the consumer. The defect existed
before this branch, but it is what drives OD-31's recorded golden delta.

## How this was checked

- Worktree `.workstrees/verify-or18c` (detached at `b68a8aca`, clean) for
  the block and pytest; target `target-verify-or18c`. Revert experiments
  ran on a `git archive` copy of `b68a8aca` in the scratchpad (target
  `target-verify-or18c-rev`), one hunk (or a set of independent hunks) at
  a time. Each was restored with Edit, and the copy was checked
  byte-identical to the archive afterwards (`diff -r` clean).
- Sweep, measurement and before/after physics ran on two more archive
  copies (`b68a8aca`, `5d66dafc`) with an identical scratch counter patch
  (a relaxed `AtomicU64` in `solve_ideal_capacity_for_target` and
  `ThermalSolver::integrate`, printed on drop). The patch is the same on
  both sides, so it does not bias the comparison.
- Private log directory: `scratchpad/or18c-verify/logs` (this session's
  scratchpad, under a name no other agent uses).
- Upstream fetched: OS-HPXML v1.12.0 `defaults.rb`, `geometry.rb`,
  `hpxml.rb`, `airflow.rb`, `weather.rb`, `EPvalidator.xml`, `LICENSE.md`,
  `data/zipcode_weather_stations.csv` (sha256 `d453ec42…cd4ecd`, matching
  the hash in `climate_zone.rs`); EnergyPlus v25.1.0
  `ZoneTempPredictorCorrector.cc`, `ZoneEquipmentManager.cc`.

## Verification block (b68a8aca)

| Line | Result |
|---|---|
| fmt; all nine clippy lines (incl. release, release -F dst, release -F observe) | pass (the old release/observe clippy failure, L5 of the last review, is gone) |
| nextest default / observe / dst / profiling | 7242 / 7300 / 7255 / 7248 passed, 0 failed, 0 skipped |
| nextest hares-core observe,profiling; release | 1381; 7199 passed |
| bench --no-run | pass |
| golden default, -F dst, -F profiling | all identical |
| ASHRAE reference check | `make check-ashrae-reference` fails in a fresh `.venv` (no `xmltodict`: `uv run` syncs without extras). The same generator run with `--all-extras --all-groups --no-sync`, diffed against the fixture, is identical. Environment (I6). |
| bestest, parity, conditioned/freefloat/structural/envelope oracle | 25 passed; output byte-identical to `5d66dafc` apart from timings |
| physics_validation_tests, solar_parity | pass |
| maturin develop --release; `uv pip install --no-deps -e vendors/OCHRE`; pytest -m "" | 1384 passed, 2 failed, 3 errors, 19 skipped (I1 to I3) |
| writing check (message and added lines, all four commits) | clean |

## Part A and Part B: fixed and pinned

| Finding | Fixed | Revert experiment (scratch copy) |
|---|---|---|
| Last review M1, water heater hierarchy order | yes | IECC 4-8 basement order swapped: `unlocated_water_heater_follows_the_hierarchy_order_between_basements` FAILS. No-IECC order swapped: same test FAILS. |
| Last review M2, zone-less HVAC capacitance | yes | `.or_else(conditioned zone)` removed: `hvac_without_a_zone_id_takes_the_conditioned_zone_capacitance` FAILS (the Room AC's bounds drop to 0; the boiler is zero on both sides, so the AC is what pins it). |
| Last review L1, dead defaults | yes (`DEFAULT_ZONE_ID`, `DEFAULT_EQUIPMENT_ID`, `ZoneState: Default`, the 20/24 °C setpoint generator removed) | Setpoint columns put back into `generate_default_schedule`: `hvac_without_a_schedule_file_keeps_the_hpxml_hourly_setpoints` and `generated_schedule_has_all_required_columns` FAIL. The 68/78 °F default is asserted by value in two tests. |
| Last review L2, IECC from the weather station | yes | `apply_climate_zone_default` call removed: `water_heater_default_follows_the_weather_station_climate_zone` FAILS (the Miami case; the Denver case cannot tell 5B from no zone, because base.xml has one basement kind). TMY3 station parse broken: no test fails (L2). |
| Last review L3, Python contract | yes (keyword-only `zone_id` on all five classes, stubs say so, `test_water_heater_without_zone_id_raises`) | Not re-built with the hunk reverted. Checked against the runtime: all five `__text_signature__` show `*, zone_id`, and the `5d66dafc` signature (`zone_id=None`) would let `heater_class("NoZone")` construct, so the test would fail there. |
| Last review L4 (ledger), L5 (release/observe clippy) | yes | n/a |
| Last review I1, measure the RH-versus-humidity-ratio blend | yes (test plus `docs/python.md`) | Recomputed by hand: W blend 0.00480 at the 20 °C floor gives about 32.6 % RH and a wet-bulb of about 11.3 °C, against about 15.1 °C at 60 % RH. The commit's numbers hold. (The last review's 12.2 °C was a mis-estimate.) |
| OD-31 environment 200 m³ fallback | yes | Restored: `zone_without_volume_or_derivation_errors` FAILS |
| OD-31 boundary_rc area × 2.5 m / 200 m³ | yes | Restored: `zone_capacitance_requires_a_volume` FAILS |
| OD-31 duct 400 m³ house | yes | Restored: `duct_dse_without_a_house_volume_errors` FAILS |
| OD-31 infiltration 400 m³ | yes | Restored: hares-core infiltration/volume tests all PASS (L1) |
| OD-31 spurious-zone rule | yes | The duct clause removed: `hpxml_parity::duct_parameters_extracted_for_hvac_equipment` FAILS. The floor-area and duct clauses removed together: 9 hares-io tests FAIL. `declared_spaces_no_surface_touches_are_no_zones` covers the surface clause. |
| OD-31 derivation rules | yes | crawlspace 3 ft→8 ft, attic `/3`→`/2`, roof term of the default ceiling zeroed, tallest wall `max`→`min`: all of hares-io and hares-core PASS (2689 tests) (M1) |

## Physics

- **Volume rules** (`zone_geometry.rs` against `geometry.rb:1315-1393`
  and `defaults.rb:899-924`): the slab area times the tallest foundation
  wall per location, 8/8/3 ft assumed heights, attic `max(fp·h/3, 0.01
  ft³)`, the square-hip roof height `0.5·sin(atan(p/12))·√fp` with 2 ft
  for a flat attic roof, and the default ceiling `((8+h/3)·f + 8(1-f))`
  rounded to 0.01 and the volume `(cfa·h + crawl).round`: all match line
  for line. Ruby's `Float#round` and Rust's `f64::round` both round half
  away from zero. The 0.01 ft³ floor is converted to m³ correctly.
  bldg0176227: 3310 ft² × 4 ft = 374.9 m³ and 864 ft² × 8 ft = 195.7 m³
  check out. For base.xml the conditioned zone (1350 ft² × 8 ft) plus
  the conditioned basement (1350 × 8) equals `ConditionedBuildingVolume`.
- **Spurious-zone rule** (`building.rs:884-894`): it keeps any attic,
  garage or foundation zone with a floor area, a duct, or any
  surface whose interior or exterior adjacency names it. OS-HPXML builds a
  space only for locations that surfaces name (`geometry.rb:1738`), and
  `EPvalidator.xml:2871-2927` rejects any location (a duct, a water
  heater, anything else) with no adjacent surface. OCHRE builds no zone
  for slab, ambient or above-apartment foundations, or for an attic
  without a roof (`hpxml.py:286, 577`). So the rule cannot drop a zone
  OS-HPXML would model. The sweep confirms it: every input that only lost
  zones, where its schedule did not also change, is bitwise unchanged
  (17 of 17 ResStock homes with a schedule CSV, and 41 of 42 sample
  files; the one that changed is bldg0527060, which has no schedule
  file, and its change is the setpoint fix, traced step by step: the
  heating setpoint at 02:30 is 49 °F in base and 55 °F at head, as its
  HPXML hourly profile says). Envelope node count is unchanged
  everywhere; only air nodes go.
- **IECC from the weather station** (`defaults.rb:974-983`, `5471-5500`):
  the 1004-station table equals the first-row-per-WMO reduction of the
  upstream CSV. Recomputed independently: same keys, zones and order.
  335 stations map to more than one zone across ZIP rows, and the Ruby
  lookup `break`s on the first match, so first-row is the right rule. EPW
  `LOCATION` field 5 is the WMO. An unknown station or a format without
  one (PSM3, ResStock CSV) is recorded as a warning and the zone is left
  unset, as OS-HPXML does. ResStock HPXML always carries the zone, so
  ResStock runs do not change.
- **68/78 °F default** (`defaults.rb:2825-2848`, `6746-6785`): OS-HPXML
  applies the manual-thermostat 68/78 °F only when the HVACControl has no
  constant setpoint, no weekday hourly setpoints and no schedule-file
  column. HARES applies it under the same condition, with a warning
  naming the unit. Schedule change: without a schedule file, OS-HPXML
  uses the HPXML's hourly weekday and weekend profiles. HARES used to
  flatten them to `weekday[0]` for the whole run (bldg0527060: 49 °F all
  day); it now keeps them, which is OS-HPXML's behaviour.
- **consumer_shape_900s delta** (annual outputs from both commits,
  monthly means): every month's crawlspace is colder except July (−0.4
  to −1.2 K October to May, +0.2 K July). bldg0176227's crawlspace
  ventilation is a constant 2 ACH × volume (M2), so the larger volume
  nearly doubles the outdoor air through it (400 → 750 m³/h). The bigger
  thermal mass alone would not shift a monthly mean. A crawlspace that
  holds more outdoor air is colder whenever outdoor air is below it.
  That holds in every month but June and July, and the ducts sit in the
  crawlspace. Result: gas heating 89.17 → 114.99 therms (+29 %), furnace
  fan 47.4 → 61.1 kWh, and cooling −28 kWh. Cooling rises in July
  (+10.0 kWh) and falls in the shoulder months, when a cooler crawlspace
  absorbs floor heat (May −0.6, August −5.7, September −14.9, October
  −16.3 kWh). Direction and magnitude are consistent; the floor area is
  307 m² and the supply and return ducts are in the crawlspace. The
  fleet's +0.21 kWh/week for the same home (January, hourly) has the same
  sign as the daily January increase in the sweep (+2.47 kWh thermal heat
  and +0.06 kWh electric per day; the fan electricity is the "kWh" the
  fleet records). EnergyPlus sizes the zone air capacitance by volume
  (`ZoneTempPredictorCorrector.cc:3080`); OS-HPXML's vented-crawlspace
  infiltration is an ACH applied to the space volume (`airflow.rb:1632-1641`).
  So a volume-proportional effect is right in kind. Its size here rests
  on HARES's 2 ACH constant rather than OS-HPXML's SLA rule (M2).

## Findings

### Medium

**M1. Four of OD-31's OS-HPXML volume rules are unpinned.**
`crates/hares-io/src/hpxml/zone_geometry.rs:17-23` (crawlspace 3 ft
assumed height), `:165` (tallest foundation wall per location), `:100-103`
(attic square-hip volume `fp·h/3`), `:125-133` (the roof-raised default
ceiling height). Repro: on a scratch copy, change each one (3→8 ft, `max`
→`min`, `/3`→`/2`, the roof term ×0), then `cargo nextest run -p hares-io
-p hares-core`: 2689 passed, 0 failed. The existing
`zone_volume_follows_the_os_hpxml_default` covers only walls of equal
height and a garage with no walls. `missing_conditioned_volume_takes_the_os_hpxml_default`
covers only the flat 8 ft case. Fix: one assertion per rule from a
vendored sample: `base-foundation-vented-crawlspace` with its walls
removed (3 ft), walls of mixed heights, an attic the OCHRE gable geometry
cannot size, `base-atticroof-cathedral.xml` with
`ConditionedBuildingVolume` removed (roof-raised height). Each should be
checked against OS-HPXML's `test_defaults.rb` values where they exist.

**M2. The vented-crawlspace SLA is ignored; ventilation silently falls to
2.0 ACH of the (now derived) volume, and OD-31's golden delta is that
fallback.** `crates/hares-core/src/dwelling/solver_builder.rs:1658-1693`.
The SLA branch needs the zone's floor area or the conditioned floor
area, and also `foundation_height_m(zone)` (`:1844`, volume / floor
area). A derived foundation zone has no floor area, so the height is
`None`. The function falls through to `ventilation_ach.unwrap_or(2.0)`
with no warning. Repro (scratch probe in `foundation_infiltration_method`,
2025.1 smoke): every vented crawlspace prints `sla=Some(0.006667)
floor_area=None height=None`. That covers four homes in the 2025.1
set, bldg0176227 among them, so each runs at 2 ACH. OS-HPXML computes the ACH from the SLA:
`get_infiltration_NL_from_SLA(sla, zone_height)·WSF`
(`airflow.rb:1632-1641, 2781-2797`); for this home that is about 5.0·WSF
ACH. Unvented crawlspaces and unconditioned basements get 0 ACH here,
against OS-HPXML's `UnventedSpaceACH = 0.1` (`airflow.rb:10, 1649-1655`).
These paths existed before the branch and OD-36 does not list them. The
derived slab area that OD-31 now computes is the floor area this branch
needs. Fix in this row (put the slab area on the zone, use OS-HPXML's
SLA→ACH rule, use 0.1 ACH for unvented spaces) or name it explicitly in
OD-36. Either way, record that OD-31's measured delta rides on the 2 ACH
constant.

**M3. OD-31's recorded delta leaves out the dominant fuel.** The commit
message and `00-schedule.md` row 21i report heating as "47.4 → 61.1 kWh".
That is furnace fan electricity. Gas heating moved 89.17 → 114.99 therms
(+29 %), and that change is the main consequence for a gas-heated home
and for the consumer's gas calibration. Repro: `frame_golden run
tests/fixtures/golden/consumer_shape_900s.toml --output-dir …` at both
commits, then sum `Gas Furnace Gas Power (therms/hour)·0.25`. Fix: add
the gas delta (and the monthly crawlspace and indoor shift above) to the
measurement log and the schedule row.

### Low

**L1. `Building.conditioned_volume_m3` is still `Option<f64>`.** Both
producers always set it: `building.rs:575-597` (HPXML, with the new
default) and `synthetic.rs:1528`. So the infiltration error
(`solver_builder.rs:1325-1332`), the duct `house_volume_m3` error
(`resolve_hvac.rs:248`) and the autosize path can only trigger on a
hand-built `Building`. Restoring the infiltration 400 m³ fallback fails no
test. Make the field `f64` (greenfield rule: no Option wrappers) and
delete the unreachable errors.

**L2. The TMY3 station WMO is unpinned.** `tmy3.rs:256-258`: inverting
the filter passes every `tmy3|station|climate|wmo|iecc` test. Add a TMY3
case to the station test, or drop TMY3 from the derivation (OS-HPXML
reads only EPW).

**L3. Two volume rules per zone type.** With a floor area, a foundation
takes the OCHRE rule: area × the first foundation wall's height
(`building.rs:784-786, 1119-1122`). Without one it takes the OS-HPXML
rule: the tallest wall (`zone_geometry.rs:165`). The attic's derived
volume is a square hip (`V = A·h/3`), but attic infiltration still
recovers its height as a gable, `2V/A` (`solver_builder.rs:1721, 1757`),
under `unwrap_or(100.0)` / `unwrap_or(1.5)` (`:1715, 1722, 1752, 1758`).
That leaves one zone geometry with two readings. Fix: one height per
location (the tallest wall), used by both, and carry the attic's derived
height rather than re-deriving it.

**L4. The new derivations make silent assumptions.** The 3/8 ft assumed
heights (`zone_geometry.rs:17-23`) and the attic and foundation volumes
emit no warning; the conditioned-volume default does. A roof over
conditioned space with no `Pitch` returns `None` (`:77`), and the default
ceiling height then drops silently to 8 ft (`:134`). OS-HPXML requires
`Pitch`, but HARES elsewhere accepts a missing pitch with a warning. Fix:
warn on each assumed value, and make a missing pitch in this path an
error or a warning.

**L5. Provenance and licence of the station table.** `climate_zone.rs:11-16`
records the source path, version, sha256 and the first-row rule. That is
good, but there is no regeneration script or check (the OCV tables have
`scripts/check_ocv.sh`), and the OS-HPXML BSD-style notice (LICENSE.md
clauses 1 and 2) does not accompany the redistributed, `include_str!`'d
table. Add a generator plus a check, and a notice next to
`crates/hares-io/data/wmo_iecc_zones.csv`.

**L6. HPXML setbacks are ignored.** `xml_helpers.rs:214-247` reads only the
hourly extension arrays or the constant `SetpointTemp*Season`.
`SetbackTemp*Season`, `TotalSetbackHoursperWeek*` and the setback start
hour (`defaults.rb:2854-2862`) are dropped, so
`base-hvac-setpoints-daily-setbacks.xml` runs flat. This was already
there, but the branch's new claim that HARES "keeps hourly setbacks"
holds only for the hourly-array form. Untracked: add it to OD-36 or a new
entry.

**L7. Public examples left behind by the keyword-only `zone_id`.**
`docs/superpowers/plans/2026-06-08-hpxml-equipment-swapping.md:1479,1496`
and `2026-06-08-python-equipment-ergonomics.md:1025,1057` construct water
heaters without `zone_id` and now raise `TypeError`. These are historical
plan documents in the public repo. Fix them, or remove them under the
"no AI slop public" rule. Tests and `python/` were all updated. Passing
`zone_id` positionally (7th argument) now fails as well, by design.

### Info

**I1. A HELICS worker crashed** (`test_single_dwelling_completion_signal_unblocks_aggregator`)
in the full run. Re-running `test_helics_integration.py` serially passed
2 of 3 times, and the third run hit the pytest timeout. Another agent
(`.workstrees/fix-testhyg`, `helics_loop.sh`) was running a HELICS loop
on the same host the whole time. That is environmental, and the
test-hygiene work owns it.

**I2. OD-33 (pandas 3):** `test_7day_benchmark` (1 failed) and
`test_thermal_trace` (3 setup errors), the same
`Cannot losslessly convert units` / `Invalid value '2262-04-11…'`
signature. Fixed at `38398bd4`, which this branch predates.

**I3. Skip census:** 19 skips against the head's empty census. They are
the pre-OD-13 set (6 `test_py_conversions` fixture skips, 5
`test_observer_diagnostics`, 8 `test_ochre_parity` column skips).
`d9133308` (OD-13) is not in the branch, and the diff touches none of
those tests.

**I4. `Dwelling::from_config` through `DwellingBlueprint::from_config`.**
The removed body was line for line the blueprint's body (parse, weather,
schedule, resample, then `from_preparsed`, which is `from_parts` plus
`build_from_blueprint`). The only differences are the two intended
changes: the IECC default and the setpoint-free generated schedule. The
TOML/synthetic path (`from_toml_config`) still goes through
`from_preparsed` and is unchanged. Every construction entry point
(Python `Dwelling`, `DwellingBlueprint`, fleet, engine, frame_golden)
reaches the single IECC path. The fleet's separate `parse_iecc_climate_zone`
reader is already in OD-36.

**I5. Public API growth** is justified by use: `hpxml::climate_zone`
(`apply_climate_zone_default` is called from hares-core),
`Building.climate_zone_iecc`, `WeatherMeta.station_wmo`, two `units`
converters, two `ZoneVolume` error variants, and the rename
`generate_schedule_from_hpxml` → `generate_default_schedule` (its
`Building` argument is gone). Only `iecc_zone_for_wmo` could be
`pub(crate)`.

**I6. ASHRAE reference check in a fresh venv:** the script's `uv run`
syncs without the `ochre` extra and fails on `xmltodict` (see the table).
That is not the branch's doing. It passes with the extras installed.

### Pre-existing defects found (not counted, untracked in the ledger)

- A "basement - conditioned" location becomes an unheated `Foundation`
  zone. base.xml's basement sits at 7.6 °C on 15 January in Denver (sweep),
  while OS-HPXML conditions it as part of the conditioned space. HARES
  inherits this from OCHRE's "Finished Basement" zone. It needs a ledger
  entry under the ASHRAE/E+-over-OCHRE rule.
- The STEP error `core_output contract violation for 'ASHP Cooler'/'MSHP
  Cooler' … thermal_output_w<0 … Standby` on the two
  `*-autosize-*-heating-only.xml` samples (and the 'ASHP/MSHP Heater'
  variants on 9 others). It happens at both commits with the same error
  text and only different numbers.

## Sweep: 425 OS-HPXML samples (Denver TMY3 EPW, generated schedules) and 31 ResStock homes (their own county weather and schedules), 96 × 900 s from 15 January

| | 5d66dafc | b68a8aca |
|---|---|---|
| build and step | 389 | 389 |
| errors | 67 | 67 (no input gains or loses an error; `base-misc-defaults.xml` moves from the missing `ConditionedBuildingVolume` error to its battery error; two step errors differ only in their numbers) |
| total thermal states over the 389 | 11878 | 11780 (−98) |
| inputs that lost air nodes | | 66 (34 lost one, 32 lost two) |
| ResStock homes, schedule CSV, numbers moved | | 13, all through a volume change (largest per day: bldg0442669 +2.69 kWh heat, bldg0000017 −3.11 kWh heat, bldg0176227 +2.47 kWh heat) |

## Measurement (release, same host, `frame_golden run --repeat 1 --discard-output`, base and head interleaved)

Host load averages during the timed runs (1/5/15 min): 15.1–16.5 /
21.8–22.2 / 27.8–28.1. Other agents were active, and nothing could be
done to quiet the host.

**Solver size** (zones / state-vector length / envelope nodes = state − zones):

| Input | 5d66dafc | b68a8aca |
|---|---|---|
| bldg0000002 (2025.1) | 2 / 28 / 26 | 1 / 27 / 26 |
| bldg0000004 (2025.1) | 2 / 22 / 20 | 1 / 21 / 20 |
| bldg0000007 (2025.1) | 3 / 22 / 19 | 1 / 20 / 19 |
| bldg0176227 (2025.1) | 3 / 78 / 75 | 3 / 78 / 75 (volumes 200,200 → 195.7, 374.9) |
| bldg0000002 / 0000004 / 0000007 (2024.2) | 3/46, 4/66, 3/35 | 2/45, 3/65, 2/34 |
| base-foundation-slab.xml | 3 / 22 / 19 | 2 / 21 / 19 |
| base-foundation-ambient.xml | 3 / 25 / 22 | 2 / 24 / 22 |
| base-bldgtype-multifamily.xml (above-apartment, below-apartment) | 3 / 15 / 12 | 1 / 13 / 12 |

**Run time, `simulate()`** (5 runs each; seconds):

| Fixture | 5d66dafc median (range) | b68a8aca median (range) |
|---|---|---|
| consumer_shape_900s (1 year, 900 s) | 1.382 (1.343–1.502) | 1.440 (1.367–1.981; one outlier) |
| fleet_mixed_offset | 0.115 (0.106–0.120) | 0.111 (0.106–0.120) |

**Profiling (-F profiling, consumer_shape_900s, one run each):**
step_total 1.138 s base and 1.200 s head. Phase shares are identical
(environment 12 %, control 8 %, ideal_capacity 5 %, equipment 15 %,
envelope 38 %, state_snapshot 18/19 %, invariants 2 %). Calls to
`solve_ideal_capacity_for_target`: 47008 → 46269 per year (1.342 → 1.320
per step, −1.6 %, fewer heating/cooling transitions after the volume
change). `integrate` runs 35040 times on both. Fleet: identical calls
(389/216/216 per home cumulative); state 28→27 and 22→21 for bldg0000002
and bldg0000004.

**Allocation:** `profiling_alloc` passes at head. Hot-path allocations
for consumer_shape_900s are identical before and after: 7,743,892 over
35,040 steps (every step allocates). That is the known telemetry-map
allocation `profiling_alloc.rs:98-99` documents, not zero, and unchanged
by this branch.

**Conclusion:** removing the spurious zones takes only air nodes away
(1 or 2 states, envelope nodes unchanged) from slab, ambient and
apartment homes. Neither golden fixture shows it in time: consumer_shape's
home keeps its 78 states, and the fleet's 1-state reductions sit inside
the run-to-run spread. The −1.6 % in ideal-capacity calls is real (the
counts are deterministic), but at 5 % of step time it is worth about
0.1 %. Every wall-time difference above is within noise on this host. The
change does not measurably reduce solver work on the goldens. It would
shave roughly 5–15 % of the state-update cost for a small apartment model
(13 vs 15 states), which no golden exercises.

## Consumer (HARES_CALIBRATION_SYSTEM, `i08-consumer`), reported, not run

- The consumer always passes a schedule CSV (`forward_model.py:281-291`
  requires one; `stock.py:316` and `engine.py:486` use `in.schedules.csv`).
  So the setpoint/schedule change cannot move B0.
- ResStock HPXML carries `ClimateZoneIECC`, so the IECC default cannot
  move B0.
- The consumer builds no Python water heater classes.
- The volume change will move B0 for every home with a vented or
  unvented crawlspace, an unconditioned basement or a garage that has no
  `FloorArea` (13 of the 30 ResStock fixtures with a schedule CSV moved
  in the sweep). The crawlspace part rides on M2's 2 ACH constant. Vented
  crawlspace homes also shift their gas use materially (+29 % for
  bldg0176227).
