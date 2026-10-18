# Review and red team: OD-30 at `528ebc6c` (branch `fix-od30`, from `38398bd4`)

Commits: `5543134a`, `6006c05d`, `1ec68ee2`, `a2502f68`, `4429f059`, `013e9dbd`,
`4b8abc8d`, `392f4b8f`, `b2cc1ff2`, `2d3a9076`, `528ebc6c`.
Reviewer did not write any of it. Worktree `.workstrees/verify-od30` (detached at
`528ebc6c`, `vendors/OCHRE` at its pin), `CARGO_TARGET_DIR=.workstrees/target-verify-od30`,
`-j 8`. Mutations ran in a `git archive` copy; nothing on the branch was modified.

## Verdict: CHANGES-REQUIRED

1 High, 2 Medium, 6 Low, plus Info notes. The core fix is correct, follows the
OpenStudio-HPXML v1.12.0 and EnergyPlus v24.2.0 sources, and its tests fail when
the fix is reverted. The High finding is a regression on valid input: overriding
a load's sensible fraction now drops that load from the dwelling or skews its split.

## Block results (at `528ebc6c`)

| Line | Result |
|---|---|
| `[fmt]` | pass |
| nine `[clippy*]` (all-features, default, dst, profiling, observe, observe+profiling, release, release+dst, release+observe) | all pass |
| `[test-debug]` | 7256/7256 |
| `[test-observe]` | 7314/7314 |
| `[test-dst]` | 7269/7269 |
| `[test-profiling]` | 7262/7262 |
| `[test-observe-profiling]` (`-p hares-core`) | 1385/1385. `profiling_alloc`: both tests pass. |
| `[test-release]` | 7213/7213 |
| `[bench]` (`--no-run`) | pass |
| `[golden]` release default / release `-F dst` / debug default / release `-F profiling` | all `compare-all` pass |
| `make check-ashrae-reference` | PASS. The first attempt failed because `xmltodict` was missing before the all-extras sync; it passed after `[python-build]`. |
| `bestest`, `parity`, `conditioned_oracle`, `freefloat_oracle`, `structural_envelope_oracle`, `envelope_oracle` | pass. BESTEST output is byte-identical to `38398bd4`. |
| `physics_validation_tests`, `solar_parity` | pass |
| `[python-build]` (no `PYTHONPATH`) | pass |
| `[pytest]` (`-m ""`) | 1382 passed, 19 skipped, 3 xfailed. Skips are listed below. |
| Python OCHRE parity / thermal trace (`-rxX`) | 3 passed, 8 skipped, 3 xfailed |
| `[writing]` (run per commit, all 11) | pass |
| ruff on changed `.py` | counts equal the parent's in every file (see L5) |
| `[stray]` | the git-tree part is clean. `TMPDIR` part fails (see M2). |

The disk filled during the first run, so `golden-debug`, ASHRAE and the physics
suites were re-run after space was freed. The results above come from those clean re-runs.

Skips in `[pytest]` (19). The census is empty on the engine head; this branch predates that.
- `test_observer_diagnostics.py` 89, 177, 221, 252, 297: "diagnostic visualization tool, not regression test" (5).
- `test_py_conversions.py` 164, 172, 180, 188, 196, 214: "ResStock test fixtures not available" (6).
- `test_ochre_parity.py:252` "HARES has no column matching ..." (8): HVAC Cooling and Heating Electric Power, Ventilation Fan, MELs, TV, Refrigerator, Indoor Lighting, Exterior Lighting. These suites used to be skipped as whole modules. `b2cc1ff2` made them run, and that exposed these skips (see L6).

## 1. Each fix shown to fail its tests when reverted

All mutations were made in an archive copy of `528ebc6c` and restored afterwards. Each restore was checked by diffing against the commit.

| Mutation | Failing tests |
|---|---|
| Event loads take their zone only from `zone_id` (pre-fix behaviour, `event_load.rs:590`, `:1153`) | `event_load_without_a_zone_errors_when_it_has_gains`, `event_load_takes_the_conditioned_zone_from_the_zone_map`, `hpxml_appliance_gains_reach_the_conditioned_zone`, `resstock_event_load_replay_delivers_its_gains` |
| A garage appliance put in conditioned space | `appliance_sites_follow_openstudio_hpxml_locations`, `appliances_give_their_heat_where_they_stand` |
| Freezer/extra-fridge default location dropped; non-electric range given the electric split; TV back to 0; unvented gas dryer accepted; gas dryer back to 0.89 | `appliances_give_their_heat_where_they_stand`, `non_electric_range_takes_the_fuel_split`, `television_and_ceiling_fan_heat_the_room`, `unvented_fuel_fired_dryer_is_rejected`, `vented_gas_dryer_gain_fractions` (one failure for each change) |
| Replay by a step counter (pre-fix) | `deterministic_replay_matches_schedule`, `event_replay_follows_the_calendar_date`, `event_replay_skips_february_29`, `event_replay_wraps_at_the_year_end` |
| Environment stops publishing `SCHEDULE_ROW_DOMAIN_ID` | the 3 replay tests plus both appliance-gain tests |
| `default_radiant_share`/`default_visible_share` return `None` | `radiant_part_follows_the_sensible_fraction`, `lighting_carries_a_visible_part`, `hpxml_lighting_splits_convective_radiant_and_visible`, both appliance-gain tests |
| `GainFractions::of` radiant forced to 0 | 2 `gain_fractions` tests, `event_load_splits_its_sensible_heat_radiant_and_convective`, 2 `scheduled_load` radiant tests, 3 `appliance_zone_gains` tests |
| `frame_golden` exclusion pathspec disabled | `golden_documents_do_not_dirty_the_capture_provenance` |
| Production `apply_port_shortwave_inputs` call removed (`thermal_solver/mod.rs:1265`) | **no hares-envelope/hares-core test fails** (only `[golden]`). See L2. |
| Rollback discarded-energy sum without shortwave (`dwelling/mod.rs:5401-5412`), and `internal_gain_w` without shortwave (`thermal_solver/mod.rs:1322`) | **no test fails** (the second is caught only by `[golden]`, through the output column). See L2. |

`equipment_zone_resolution::hvac_and_water_heaters_without_a_zone_fail_init` failed in the archive copy only. The cause is that `tests/fixtures/hpxml/ochre_samples/base.xml` is absent from `git archive`; this is not a mutation result.

## 2. Physics against the sources

Fetched: OpenStudio-HPXML v1.12.0 `HPXMLtoOpenStudio/resources/{hotwater_appliances,misc_loads,hvac,model,geometry,hpxml,defaults,lighting}.rb` and EnergyPlus v24.2.0 `InternalHeatGains.cc` and `HeatBalanceSurfaceManager.cc`.

- **Appliance splits** (`hotwater_appliances.rb`):
  - range: `frac_lost` 0.20, 0.90 sensible of the rest if electric, else 0.80 (577-584), giving 0.72/0.08 and 0.64/0.16;
  - dishwasher: 0.40, 0.50 (658-661), giving 0.30/0.30;
  - dryer: 0.85 when vented, 0.90 for every fuel (771-777), giving 0.135/0.015;
  - washer: 0.70, 0.90 (871-874), giving 0.27/0.03;
  - fridge and freezer: 1.0/0 (924-926).

  All match `resolve_loads.rs:1168-1199`. Defaults also match `defaults.rb`: TV 1.0/0 (4798-4802), "other" 0.855/0.045, fireplace 0.5/0.1 (4950-4954), grill and lighting fuel loads 0.
- **Radiant 0.6 × sensible:**
  - appliances: `hotwater_appliances.rb` 80, 121, 132, 172, 220, 268, 309, 320;
  - plug loads: `misc_loads.rb` 89. The `apply_ashrae140_assumptions` 0.7 branch is not reachable from HARES HPXML;
  - fuel loads: `misc_loads.rb` 186;
  - ceiling fan: `frac_radiant: 0.558` of all-sensible power (`hvac.rb` 1635).

  All correct (`resolve_loads.rs:1219-1229`).
- **Lighting** (`model.rb` 249-250, `add_lights`): FractionRadiant 0.6, FractionVisible 0.2, the remaining 0.2 convective, in the space the lights stand in (garage lights in the garage, `lighting.rb`). Exterior lights are ElectricEquipment with `frac_lost` 1. All correct.
- **Location rule:**
  - `geometry.rb` 1704-1716: outside, other heated space, other housing unit, other MF buffer and other non-freezing have no space, so zero gain. Conditioned locations map to conditioned space.
  - `hpxml.rb` 12311-12316 lists conditioned space, basement-conditioned, crawlspace-conditioned and other housing unit. The early return in `get_space_from_location` wins for "other housing unit".
  - `appliance_site` (`resolve_loads.rs:1114-1152`) matches this.
  - The freezer/extra-fridge default hierarchy (`defaults.rb` 5683-5697) matches `default_appliance_location`.
- **Visible lighting in EnergyPlus:**
  - `InternalHeatGains.cc` 7640/7649: `VisGainRate = Q × FractionShortWave` is added to `QLTSW`.
  - `HeatBalanceSurfaceManager.cc` 3687-3693: `EnclSolQSWRad = EnclSolQD + ΣQLTSW`, then opaque absorption at `AbsIntSurf` (3773) with `solVMULT = 1/SUM1` (4314-4315).
  - HARES reuses its transmitted-diffuse path (area × inside absorptance, normalised), so the energy lands on the same surfaces. One difference: EnergyPlus's `SUM1` includes windows' `TransDiff + AbsDiff`, so part of the visible light leaves through the glazing. HARES absorbs all of it. This is queued as OD-44 and is not a new finding.
- **Replay by calendar row.**
  - `compute_schedule_idx` (`environment.rs:435-478`) indexes by calendar date and time: leap-day skip (doy > 59 in leap years), modulo year wrap, civil time under `dst`.
  - Tests pin the date, Feb 29 and wrap cases.
  - DST: under `-F dst` the spring-forward row is skipped and the fall-back row repeats. Replayed events therefore lose or double that hour, the same as scheduled loads. This is consistent and follows the local-time rule.
  - Warm-up follows the calendar.
- **Unvented fuel-fired dryer.** The rejection matches the IFGC 614.1 requirement that clothes dryer exhaust be conveyed outdoors. Rejecting the input instead of relabelling it is the right call (but see L4).

## 3. Golden deltas reproduced (`frame_golden run --output-dir` at `38398bd4`, `5543134a`, `b2cc1ff2`, `528ebc6c`)

**`leap_year_february_900s`** (cz4a ASHP, 27 Feb to 3 Mar 2024), `38398bd4` to `5543134a`:
- Appliance input energy is unchanged (replay still counts from row 0).
- Added sensible gain = range 7.760 × 0.72 + dishwasher 1.634 × 0.30 + TV 9.789 × 1.0 (TV went from 0 to 1) = **15.87 kWh**, plus 1.11 kWh latent.
- ASHP heating electric fell 149.944 to 147.102 (−2.84 kWh; about 7 to 8.5 kWh of heat at COP 2.5 to 3).
- Mean indoor temperature rose 21.035 to 21.096 °C.

**`month_boundary_billing_3600s`** (cz2a gas furnace, 12 days from 25 Jan), same step:
- Added sensible gain = range 16.150 × 0.72 + washer 0.934 × 0.27 + dryer 15.421 × 0.135 + dishwasher 1.984 × 0.30 + TV 25.541 = **40.10 kWh**.
- Furnace gas fell 28.872 to 28.434 therm (−12.8 kWh fuel, about −10 kWh heat) and furnace fan electric fell 0.43 kWh.
- Indoor temperature rose 0.09 °C.

In both fixtures the direction is right: more internal gain gives less heating and a warmer zone. The heating reduction is a fraction of the added gain, which is plausible because much of the appliance heat arrives in daytime or float hours.

`a2502f68` (replay on its own dates) then changes the appliance energies themselves:
- cz4a: washer 0 to 0.575, dryer 0 to 4.532, range 7.76 to 10.32 kWh;
- cz2a: range 16.15 to 11.80, dryer 15.42 to 25.72 kWh;
- hot-water energy moves with the wet-appliance draws.

This is expected.

Lighting (`b2cc1ff2` to `528ebc6c`, total lighting heat unchanged, 0.8 moved off the air node):
- mean indoor air falls 0.07 °C (cz4a) and 0.08 °C (cz2a);
- heating falls slightly: ASHP −0.31 kWh, gas −0.02 therm.

That is the expected radiant/mass behaviour: lower daytime air peaks, and stored heat released at night.

**Parity** (`parity` MAE °C):

| Fixture | `38398bd4` | after appliance fixes (`b2cc1ff2`) | `528ebc6c` | allowed |
|---|---|---|---|---|
| cz5a_minisplit_gas_wh | 1.7109 | 1.6223 | 1.7433 | 1.75 |
| cz6b_resistance_res_wh | 0.7459 | 0.6963 | 0.7084 | 0.75 |
| resstock_bldg0112631_24h | 0.4543 | 0.4212 | 0.4407 | 0.60 |
| the other 9 fixtures | | | improve slightly at each step | |

Assessment:
- The lighting step moves these three away from OCHRE. Their direction is explained: OCHRE gives lighting a convective fraction of 1 (`vendors/OCHRE/ochre/utils/hpxml.py` 1274ff). OS-HPXML and EnergyPlus give 0.2 convective, 0.6 radiant and 0.2 visible. The change is right under the "EnergyPlus-grade, not OCHRE's simplification" rule.
- Against the parent, every fixture is still at or better than before except minisplit, which is 0.032 worse. It sits 0.0067 inside its tolerance. The ratchet still bites: any further regression trips it. It now has almost no headroom, so the next numeric entry on that fixture must re-justify the number.
- The ratchets that improved (for example cz2a 0.676 against 0.70, cz6b 0.708 against 0.75) were not tightened. Tightening them is the ledger's call. The before and after values above should go into `02-measurement.md`.
- BESTEST is unchanged.

## 4. Design

- **Shared load-zone resolution.** `load_zone.rs` (`resolve_load_zone`) is the one path for scheduled and event loads, and `gain_fractions.rs` (`GainFractions`, `accumulate_zone_gain`) is the one parser and accumulator. There are no remaining duplicates (`scheduled_load.rs` and `event_load.rs` both call them). Good.
- **`PortContribution::ShortWave` / `shortwave_gain_w`.** These are included in:
  - `ThermalAccumulator::total_gain_w` (rollback);
  - `internal_gain_w` (`thermal_solver/mod.rs:1322`), so the "Internal Heat Gain" and "Net Sensible" columns;
  - the zone air balance residual (via `u`);
  - `zero()`.

  They are not included in observer contributions, step diagnostics, or the port convective/radiant diagnostics (M1), or in the debug breakdown (L1). They are not per-category; the variant is documented as always an internal gain, which is acceptable.
- **`SCHEDULE_ROW_DOMAIN_ID`** (`hares-types/src/schedule.rs:162`, `environment.rs:846-854`). It is allocation-free after the first step: the payload `Vec` is swapped back through the `custom_domains` drain (`environment.rs:818-821`). `schedule_row` does a linear scan of a few domains, and replay uses `partition_point` (O(log n)). There is no hot-path cost. The abstraction is weak, though (L3).
- **`frame_golden` dirty flag** (`tools/frame_golden/src/golden.rs:316-321`). It excludes only `tests/fixtures/golden/*.golden.json`. Every engine input is still counted: the manifests, the CSV/JSON fixtures beside them (`flat_tariff.json`, `consumer_shape_envelope_materials.csv`), `defaults/`, crates and submodules. A `.golden.json` is never an engine input. `root` is the repository root (`manifest.rs:332-345`), so `-- .` does not narrow the check to a subdirectory. No real input change escapes.

## Findings

### High

**H1. Overriding a load's sensible fraction drops the load, or skews its split.**
`build_spec` (`crates/hares-io/src/hpxml/equipment.rs:132-146`) writes `radiative_gain_fraction` and `visible_gain_fraction` as absolute fractions (share × the default sensible fraction). Two override passes then run after it and can change the sensible fraction without changing these:
- HPXML `apply_overrides` (`equipment.rs:69`);
- the dwelling's `apply_equipment_overrides` (`crates/hares-core/src/dwelling/conversions.rs:911`).

If the override is below radiant + visible, `GainFractions::from_config` (`crates/hares-equipment/src/gain_fractions.rs:91-96`) rejects it. The non-critical init failure path (`dwelling/mod.rs:2631-2652`) then logs "init failed, skipping" and the appliance leaves the dwelling, taking its electricity and heat with it. If the override is above radiant + visible, it is accepted with the stale radiant part: a range at 0.9 gives radiant 0.432, a 48 % share instead of 60 %.

Repro: a scratch test on `tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh` with 1-day `DwellingConfig`.
- `overrides: None`: range conv/rad = 0.288/0.432, lights 0.2/0.6/0.2.
- `{"Cooking Range": {"sensible_gain_fraction": 0.3}}` builds, but `remove_equipment("Cooking Range")` fails with "equipment 'Cooking Range' not found".
- `{"Indoor Lighting": {"sensible_gain_fraction": 0.5}}`: the same, with "Indoor Lighting" not found.

At `38398bd4` neither key existed (`git show 38398bd4:crates/hares-io/src/hpxml/equipment.rs | grep -c radiative` gives 0), so these overrides worked.

Fix: carry the radiant and visible *shares* of sensible (or resolve them after overrides, at init), and pin with a test that overrides a sensible fraction both below and above the default.

### Medium

**M1. Observer and step-diagnostic consumers of zone gains omit the radiant and short-wave parts.**
- `observer_capture.rs:88-100` (`diff_ports`) and `:157-163` (`capture_ports`) record only `sensible_gain_w` (the convective part) and `latent_gain_w`. The non-zero filter means a contribution that is only radiant or only short-wave does not appear at all.
- `diagnostics.rs:330-334` (`thermal_gains_w`) records convective only.
- `EnvelopeComponentGains`/`EnvelopeDiag`/`py_dwelling.rs:2749-2750` expose `port_convective_w` and `port_radiant_w`, with no short-wave term.

After this branch, 60 % of every appliance's and plug load's sensible heat and 80 % of lighting no longer appears in the per-equipment contribution. Radiant was already omitted before this branch, but these commits made the omission material; for short-wave the omission is new. Add the radiant and short-wave parts to the observer tuple and the diagnostics, add a `port_shortwave_w` beside `port_radiant_w`, and pin each.

**M2. `[stray]`: the OCHRE suites that now run leave temp output behind.**
`b2cc1ff2` makes `test_ochre_parity.py` and `test_thermal_trace.py` run. They write into `tempfile.mkdtemp()` directories they never remove (`test_ochre_parity.py:99`, `:139`, `:323`; `test_thermal_trace.py:211`). After `[pytest]`, `target/block-tmp` holds `tmp*/hares_parity_diagnostics.csv` and `tmp*/hares_thermal_trace*.csv`.

Other leftovers in the same run predate this branch and should be ledgered with it:
- `dwelling_0.csv`/`dwelling_42.csv`/`fleet_*.csv` mkdtemp dirs from other modules;
- `pytest-of-$USER/pytest-0/popen-gw*` children, which the `[stray]` rule also fails.

Use `tmp_path` (or a `TemporaryDirectory` context) in the two modules.

### Low

**L1. The debug breakdown and doc comments are stale for short-wave.**
- `zone_sensible_breakdown_debug` (`crates/hares-envelope/src/thermal_solver/mod.rs:634`) applies short-wave to a `u` it then discards. The call is dead.
- `ZoneSensibleBreakdown` has no short-wave term, so its documented closure (`mod.rs:338-342`, "after_int_lwr + convective_direct + radiant_to_air_residual equals the total zone air input") fails when lights are on.
- `EnvelopeComponentGains::internal_gain_w`'s doc (`thermal_solver/config.rs:921-924`) still says "convective and radiant".

**L2. Behaviour changes that no test pins** (mutation results in §1). Each belongs in the named test file:
- The production short-wave injection (`thermal_solver/mod.rs:1265`) is caught only by `[golden]`. Add a `build_input_vector` conservation test with a short-wave gain (`crates/hares-envelope/tests/thermal_pathway_physics.rs`).
- Rollback's discarded thermal power includes short-wave (`dwelling/mod.rs:5401-5412`), and nothing pins it. Add a rollback test with a lighting-like load (`dwelling/mod.rs` tests).
- `internal_gain_w` includes short-wave (`mod.rs:1322`), and only `[golden]` catches it. Add an `EnvelopeComponentGains` test.

**L3. `SCHEDULE_ROW_DOMAIN_ID` encodes an integer as an `f64` in a custom domain** (`hares-types/src/schedule.rs:159-190`).
- It needs a fractional/negative check on read.
- It claims `DomainId(u16::MAX - 2)` with no reservation guard. A user custom domain solver with that id would overwrite the row: either replay errors or replays the wrong date.
- A typed `EnvironmentState` field (`schedule_row: Option<u32>`) would be type-safe, just as allocation-free, and needs no reserved id.

The existing schedule and mains domains share the reservation gap.

**L4. `ClothesDryer/Vented` is parsed with `eq_ignore_ascii_case("true")`** (`resolve_loads.rs:318-320`). `xs:boolean` also allows `1`, so a gas dryer with `<Vented>1</Vented>` is now rejected as an unvented fuel-fired dryer (it was relabelled before). Parse `true`/`1` and `false`/`0`, and reject anything else.

**L5. ruff I001 on the touched import block of `tests/python/conftest.py:3`** (the block now includes the added `from types import ModuleType` / `import pytest`). The finding count equals the parent's, but the finding sits on a changed block. There is also a leftover run of three blank lines where `importorskip` was removed (`tests/python/test_ochre_parity.py`, about line 186).

**L6. The OCHRE comparison now runs, but compares no lighting.** `test_ochre_parity.py:252` skips 8 per-end-use columns (Indoor/Exterior Lighting, MELs, TV, Refrigerator, Ventilation Fan, HVAC heating/cooling electric) because HARES has no matching name. The lighting change therefore has no Python-level OCHRE check. Map the names, or remove the comparisons that cannot run.

### Info

- The acceptance test `hpxml_appliance_gains_reach_the_conditioned_zone` checks each appliance's own port output on the dwelling's environment. It does not check the dwelling's zone accumulation of it. Zone accumulation is generic and the goldens move as expected, so this is adequate, but it is narrower than the ledger text ("the zone's internal gain over a day").
- Diffuse short-wave leaving through windows is queued as OD-44. Microwave double counting is queued as OD-43, and microwave has no radiant share (not an OS-HPXML category).
- `1ec68ee2` restores the strict 28 °C bound. `TestHvacActiveCooling` passes at `528ebc6c`.
- Goldens at `528ebc6c` record `git_head` `2d3a9076`, `git_dirty: false`, and pass `compare-all` in all four modes.
- Checkpoint format version 3 drops the replay cursor and step counter. Whether `load_versioned` refuses a version-2 checkpoint was not checked by a test (greenfield rule).
