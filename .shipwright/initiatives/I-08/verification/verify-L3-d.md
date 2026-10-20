# Verification L3-d: consumer (ledger/08-consumer.md) and numeric fixes (ledger/09-numeric-fixes.md)

Scope: `plan.md` §1-3, `ledger/08-consumer.md`, `ledger/09-numeric-fixes.md`, checked against
`decision-log.md` and `ledger/00-schedule.md`.

Code states checked:
- Engine `~/src/HARES` at `ebdb1a17` (the evidence commit), `872cf289`, `8b09ac4a`, and the live head `21d90588`.
- Consumer `~/src/HARES_CALIBRATION_SYSTEM` at `c71c542`, `aec2e3f` and `5c651bc` (the head of `i08-consumer`).
- Vendored EnergyPlus at `f7c103167a`.

Nothing in either repository was edited. Consumer mechanics were exercised only in isolated scratch copies:
- a copy of `pyproject.toml`, `uv.lock` and `.venv`, with `uv sync ... --dry-run`;
- a `git archive` of `5c651bc`, with `hares_contract_check.py`.

Totals: **4 blockers, 31 majors, 22 minors.**

## Blockers

### X-1 (B-1a) The PSM3 fixture cannot be parsed

**Where:** ledger 240-245, 269-272.

**Problem:** `psm3.rs` cannot read the fixture as the entry specifies it.
- It reads three header lines, not two (`psm3.rs:4-5, 57-107`).
- It accepts only full-year record counts (`validate_record_count`, `psm3.rs:445-462`). 96 records are rejected.
- It rejects Temperature outside [-60, 55] C (`:148-152`), so index values 56 to 95 fail.
- It requires Dew Point, Year, Month, Day, Hour and Minute, and requires dew point ≤ dry bulb (`:155, 379-392`).

**Correction:** the fixture becomes `tests/fixtures/weather/psm3_15min_2019.csv`:
- a full 2019 15-minute PSM3 file in `psm3.rs`'s three-line layout, with Latitude, Longitude, Time Zone -7 and Elevation;
- columns Year, Month, Day, Hour, Minute, Temperature, Dew Point, Relative Humidity, Pressure, GHI, DNI, DHI, Wind Speed, Wind Direction;
- 35040 records;
- on 2019-01-01 Temperature = -20.0 + 0.5·(k mod 96); every other field constant (Dew Point -40, irradiance 0, Pressure 1013, RH 50, wind 2 m/s at 180).

`psm3_record_covers_the_interval_it_starts` then expects `-20.0 + 0.5k` on step k. The alternative is for B-1a to relax the record-count rule explicitly.

### X-2 (B-8) The defect cannot be reached and the test cannot be built

**Where:** ledger 929-958.

**Problem:** the fallback the entry fixes never runs.
- `resolved_node_flows` is `Some` only inside `if let Some(pump) = pump_node` (`fluid_solver.rs:334-351`), and the pump's own flow is always inserted (`:469`).
- `pump_node_id` (`:746-767`) recomputes the same predicate on an unchanged topology.
- So `.unwrap_or_else(|| resolved.values().sum())` (`:785-787`) is dead.
- The proposed test topology (a Source node with an incoming edge) yields `None` and never enters the branch.
- The maps are built internally with `HashMap::new()` (`:315, 370`), so a test cannot choose `RandomState`.

**Correction:** recast B-8 as dead-code removal with zero delta.
- The resolution block returns `Option<(FluidNodeId, HashMap<..>)>`.
- `pump_node_id` at `:746-767` is deleted.
- `pump_total = resolved[&pump_id]` with no fallback.
- Acceptance: the existing `fluid_solver` tests pass, and `git grep 'resolved.values().sum()' crates` is empty.
- The decision item "deterministic fluid-solver summation" is then met by construction. Record that in the entry.

### X-3 (B-14) The fixture premise is false and the bitwise guard covers nothing

**Where:** ledger 1509-1525.

**Problem 1:** golden homes already carry PV.
- `tests/fixtures/resstock/2025.1/bldg0176227/home.xml` has a `PVSystem` with `MaxPowerOutput` 7000.
- That home is in `consumer_shape_900s` (whose golden has "PV Electric Power (kW)") and in `fleet_mixed_offset`.
- `fleet_same_offset_300s` (FL-1) also uses bldg0176227.
- `resstock_2025_fleet` (B-9a) includes bldg0352714, which has PV.

**Problem 2:** every existing golden home has an air conditioner or a heat pump:
- bldg0176227: central AC
- bldg0000002: room AC
- bldg0000004: heat pump
- cz4a: heat pump
- cz2a: central AC

So the clause "every other golden compares bitwise" covers no golden at all.

**Correction:**
- Write "no golden home carries a battery, an EV or a generator. `consumer_shape_900s`, `fleet_mixed_offset`, `fleet_same_offset_300s` and `resstock_2025_fleet` carry PV."
- Verification names each golden allowed to move, with its reason (PV in W with no /1000·×1000 round trip; crankcase heater).
- It records per golden whether the golden moved, and states which goldens must not move.

### X-4 (B-14) `equipment_config_fields_are_si` contradicts the entry's own exclusions

**Where:** ledger 1484-1486.

**Problem:** the census commands print lines that B-14 deliberately keeps:
- `BatterySpec` (`battery/catalog.rs:100-102`);
- `VehicleSpec` (`ev/catalog.rs:109-120`);
- the computed state of `EquivalentBatteryModel` (`hvac/equivalent_battery.rs:30-52`).

The test can never pass.

**Correction:** the census excludes those three files and states why. Preferably, the census is a test over the listed config types' `DescribeFields`.

## Majors: consumer

### X-5 (CN-13) The rename census and Change miss most `ochre_next` imports

**Where:** ledger 1337-1366.

**Problem:** these sites import `ochre_next` (at `c71c542`) and are not named:
- `ami2/cvr_actuation.py:126`
- `ami2/dr_actuation.py:77,174`
- `ami2/tariff.py:71,82`
- `forward_model.py:260-262`, whose fallback `import HARES` is wrong
- `hares_der_remeasure.py:44`
- `hares_l1_archetype_sweep.py:42`
- `hares_ua_xval.py:50`
- `tests/suite_ami2.py:799,850,1696,1915`
- `tools/provision_data.py:65-66`

The text hits are not named either:
- `coupling.py:373`, `der_evidence.py:46`, `run_calibration.py:195`
- `STRUCTURE.md`, `CONTRIBUTING.md`
- at `5c651bc`, `suite_scale.py:335`

The gate fails on the PY-7 engine.

**Correction:** list every site. The Change says that every `import ochre_next` imports `hares` and that the `forward_model` fallback goes. The `CACHE` line is `paths.py:116-117`.

### X-6 (CN-1) `Runner(scratch, ...)` and `run_specs(scratch=...)` sites are missing from the evidence

**Where:** ledger 556-568.

**Problem:** these sites (at `c71c542`) break when `scratch` is dropped, and the census grep does not look for `scratch=`:
- `der_experiment.py:156,309`
- `flag.py:215`
- `forecast.py:214`
- `identify.py:108,110,158`
- `reactive.py:109,268,373`
- `residual.py:235`
- `parallel.py:257-266`
- callers that pass `scratch=` through: `der_experiment.py:129-167,318-332`, `extrapolate.py:112`, `feeder.py:96`, `forecast.py:282,301`, `frontier.py:212`, `premise.py:172`, `scale.py:569`, `ami2/run.py:207`

**Correction:** add them to the evidence and the Change. Either state that the fit-level `scratch` arguments (the injector slot directories) stay and only the engine-output `scratch` goes, or remove both explicitly.

### X-7 (CN-1) Selection presets are wrong for the AMI path

**Where:** ledger 629-631.

**Problem:** callers mutate the module global `E.OUTPUT_VERBOSITY` to 6 (`ADAPTER_VERBOSITY`, `ami2/adapter.py:72`) or to 0. Sites:
- `ami2/cvr_actuation.py:81-89`
- `ami2/run.py:192-225`
- `suite_ami2.py:54-63, 400-411, 882-888, 1182-1189, 1302-1306, 1730-1735, 1923-1937, 2021-2025, 2081-2090, 2146-2156`
- `engine.py:68,330,533`

**Correction:** add presets for verbosity 0, 2, 5 and 6. Every global-mutation site becomes an explicit selection argument and `E.OUTPUT_VERBOSITY` is deleted. List the sites.

### X-8 (CN-1) `calibration/schema.py` is not migrated, so step 4b breaks from PY-8 on

**Where:** ledger 575.

**Problem:**
- `Prober` (`schema.py:363-377`) sends `duration_s`, `time_res_s`, `defaults_path`, `output_verbosity` and `write_output`, and calls `from_hpxml` plus `initialize()` (`:392-395, 421-424`).
- `known_kwargs()` (`:249-251`) regex-scans `KNOWN_KWARGS` in `py_dwelling.rs`, which PY-8 deletes.
- After PY-8 every probe fails inside a broad `except`. Every class becomes NOT_WITNESSED, the kwarg surface is empty, and the `suite_coverage` cases fail at CN-1's gate.

**Correction:** CN-1 migrates `Prober` to `config=SimulationConfig(...)`, `defaults=` and a selection. It re-points `known_kwargs` to the published surface (the `from_hpxml` signature plus `REMOVED_KEYS` through `hares.input_schema()`, IN-3). Cite `schema.py:249-251, 363-424`.

### X-9 (CN-1) Only part of the kwarg classification is updated

**Where:** ledger 619-620, 705-709.

**Problem:**
- `suite_coverage.kwargs_and_name_lists_match_the_binding` (`:141-153`) also asserts `defaults_path` and `time_res_s`.
- `KWARG_RULES` at `coverage.py:116-117` (`duration_s`, `time_res_s`, `initialization_duration`), `:120` (`defaults_path`) and `:122-123` (the site keywords) name keys that leave the surface.
- The new `defaults` and `config` keywords are unclassified, so `every_accepted_field_is_classified` fails.

**Correction:** CN-1 reclassifies every keyword after PY-8 and updates `coverage.py:112-123` in full and the whole case at `suite_coverage.py:141-153`.

### X-10 (CN-1) Direct construction sites are not in the Change

**Problem:** these sites construct the engine directly:
- `scale.py:262-274` (`from_hpxml` with `duration_s`, `time_res_s`, `defaults_path`, `write_output`)
- `scale.py:334-338` (`DwellingConfig(..., defaults_path=)`)
- `scale.py:362` (`SteppableFleet.from_configs`)
- `inject.py:248-252` and `suite_der_inject.py:342-345` (`DwellingBlueprint.from_hpxml` with the old keywords)
- `forward_model.py:256-290`
- `run_calibration.py:209-211`
- `tools/bench_calibration.py:97-105`

**Correction:** list them and migrate them in CN-1.

### X-11 (CN-1) `_engine_metrics` is not migrated

**Problem:**
- `engine.py:411-472` reads `dw.metrics()`, which PY-9 deletes, through `getattr` with an `except` that returns `{}`. The metrics would silently become empty.
- It also reads `dw.dwelling_fatal` (`:471`).
- Other readers of the same keys: `suite_ami2.py:683-694, 743, 764, 772, 1208`.

**Correction:** CN-1 rebuilds `_engine_metrics` from `RunResult.metrics` (the `RunMetrics` fields), raises on a missing field, and names the new source of `dwelling_fatal`. Add the readers to the evidence.

### X-12 (CN-1) `finalize_billing` has no replacement

**Problem:** `ami2/tariff.py:79-91` (set tariff, simulate, `finalize_billing`, `billing_summaries`) has no Change bullet, and PY-9 deletes `finalize_billing`.

**Correction:** cite `:79-91` and state how bills are read from the `RunResult`.

### X-13 (CN-1) The census grep cannot come back empty, and is too narrow

**Where:** ledger 703.

**Problem 1:** hits the Change does not touch:
- `ami2/adapter.py:23`
- `engine.py:11-12, 128, 151`
- `parallel.py:68`
- `ami2/tariff.py:79,90`
- `suite_engine.py:133`
- `suite_scale.py:197`, an unrelated fixture file named `run.parquet`

**Problem 2:** the pattern misses `duration_s=`, `time_res_s=`, `defaults_path`, `output_verbosity`, `OUTPUT_VERBOSITY`, `scratch=`, `\.metrics\(`, `KNOWN_KWARGS` and `DwellingConfig\(`.

**Correction:** list these as stale-text edits, rename the `suite_scale` fixture, and widen the pattern.

### X-14 (gate, CN-15) A contract check that cannot find the engine passes

**Where:** ledger 256-257, 1482.

**Problem:**
- `hares_contract_check.py` resolves `$HARES_REPO` or `DEFAULT_REPO="/home/claude/hares_src/..."`, not `paths.HARES_REPO`.
- When the source is absent it prints SKIP and returns 0 (`:27-58`, read at `5c651bc`).
- `suite_calibration.contract_matches_HARES_source_or_skips` (`:264-278`) skips only when `paths.have_repo()` is false.
- CN-15 runs with no `HARES_REPO`. The submodule satisfies `have_repo()`, the check SKIPs with rc 0, and that counts as a pass.
- "Exits 0" therefore cannot tell PASS from SKIP.

**Correction:**
- The gate requires `[hares-contract] PASS` in `contract.txt`.
- CN-15 sets `HARES_REPO=$PWD/HARES`, or the first consumer row that touches the check resolves the repository through `paths.HARES_REPO`.
- The gate case name is `hares_contract.contract_matches_HARES_source_or_skips` (preamble line 71 says `contract.`).

### X-15 (CN-3) Sites omitted when the DEFAULTS route moves to `overrides["envelope"]`

**Where:** ledger 813-848.

**Problem:** these sites are not named:
- `coverage.py:137, 281-296, 355-359`
- `registry.py:37, 66, 661-662, 692-693` (route constant, `defaults_key`, its validation and its description)
- `tools/bench_calibration.py:62-78`
- `build_bundle.py:46` (it registers `defaults.py`; deleting the module without this edit gives MISSING)
- `suite_coverage.every_registry_factor_targets_a_real_field` (`:111-125`), which resolves every OVERRIDE factor's equipment to a config struct. `envelope` resolves to none and the case fails.

**Correction:** list every site, and state how `envelope` is resolved: to its IN-3 schema class, or exempted by name.

### X-16 (CN-12) The acceptance command cannot see the `trial` group

**Where:** ledger 1311-1312.

**Problem:**
- Mechanism step 2 syncs without `--group trial`.
- `uv run --no-sync --group trial` does not install a group. Measured in the scratch venv: `uv run --no-sync --extra docs python -c 'import markdown'` raises `ModuleNotFoundError`.
- `egobox` is therefore absent.
- Step 4's "exactly one Would uninstall" line also no longer holds once the group is installed outside the step-2 sync.

**Correction:**
- CN-12's runs use `uv sync --locked --extra engine --group trial --no-install-package ochre-next` (named `hares-py` after CN-13) in step 2.
- Step 4's dry-run uses the same flags.
- The gate's `ruff check` is confirmed unchanged by the group.

### X-17 (schedule and CN-17) Rows ran past CN-17 before it landed

**Problem:**
- `00-schedule.md` puts CN-17 (row 13-1) before BR-11 (13a), and says a row starts only when the row above is committed.
- The engine has committed BR-11 (`8b09ac4a`), PR-1 (`d7f0e381`) and PR-2 (`21d90588`). The consumer head is still `5c651bc` (BR-6d); CN-17 has not landed.
- Rows 14 and 15 are labelled "B0 (CN-17)", a consumer state that does not exist yet. `02-measurement.md` records B0 only for BR-11 ("identical"), not for PR-1 or PR-2.
- CN-17's drift census "for rows 1 to 12" no longer matches the engine commit its gate will run on. CN-13 covers "BR-11 to row 26", so the assignment overlaps.

**Correction:**
- Record the executed order in the schedule and the decision log.
- Either land CN-17 now (gated on `21d90588`) with its census covering rows 1 to 15, CN-13 then covering rows 16 to 26; or re-label rows 14 to 15 as B0 (CN-16) and run their B0 checks.
- In both cases record the B0 checks of PR-1 and PR-2 in `02-measurement.md` before row 16.

## Majors: numeric fixes

### X-18 (B-1c) The Feb 29 fixture recipe yields an unparseable file

**Where:** ledger 430-433.

**Problem:** the Feb 28 records of the cz4a TMY3 file are year 1994, and `epw.rs:199-203` rejects 1994-02-29.

**Correction:** "copy each Feb 28 record, set year 1996, month 2, day 29, dry bulb 50.0".

### X-19 (B-1c) The restamped 2020 AMY recipe breaks interval-end stamps

**Where:** ledger 436-439.

**Problem:** `G0600770_2018.csv` runs from 2018-01-01 01:00 to 2019-01-01 00:00 (hour-ending). Setting only the year:
- puts the last record at 2020-01-01 00:00, before the first record;
- turns Feb 28's hour 24 (stamped 2018-03-01 00:00) into 2020-03-01 00:00, so the file is no longer contiguous.

**Correction:** shift each record's interval start (stamp - 1 h) to the same month, day and hour of 2020, and restamp at interval end:
- Feb 28 hour 24 becomes 2020-02-29 00:00;
- Dec 31 hour 24 becomes 2021-01-01 00:00;
- 8760 records, none covering Feb 29.

### X-20 (B-1c) Partial-year typical files are unsupported, and the Change does not add support

**Where:** ledger 377-383, 467-469.

**Problem:**
- `epw.rs:318-325` accepts only 8760 or 8784 records, so `cz4a_january.epw` (744 records) fails at parse.
- DATA PERIODS dates are not kept.
- The mains-water and ground-temperature inputs (`environment.rs:889-932`) assume a full year.

**Correction:** the Change adds:
- the EPW reader accepts a record count equal to its DATA PERIODS span and keeps the period in `WeatherMeta` (any other count is an error naming both);
- for a file that is not a full year, mains and ground inputs come from the header, or construction fails naming the quantity;
- parsed weather keeps dated stamps for the `Actual` range checks.

### X-21 (B-1c) "Reads dry bulb 50.0 C on Feb 29" contradicts B-1a's interpolation

**Where:** ledger 457-459.

**Correction:** "steps ending 01:00 to 24:00 of Feb 29 read 50.0; the steps ending 00:15, 00:30 and 00:45 read `f28h24·(1-w) + 50·w` with w = 0.25, 0.5, 0.75".

### X-22 (B-1b) The first-hour rule also governs solar

**Where:** ledger 315-317.

**Problem:** `wvarsLastHr` is the whole weather struct (`WeatherManager.cc:3054-3055, 3113-3116`). On the run's first day, solar's previous hour for hour 1 is that day's hour 24, or hour 1 under `Hour1`, not the previous day's hour 24. B-1b states only the day-boundary rule. EnergyPlus also reuses the rule for every warm-up day (`Energy+.idd.in:1488`).

**Correction:** state the rule in B-1b and add the test `solar_first_hour_uses_hour24_of_the_first_day` (and its `Hour1` case).

### X-23 (B-2) `warmup_nan_residual_fails_the_run` never reaches the residual guard

**Where:** ledger 618-621.

**Problem:** OR-5 already fails a NaN zone temperature on every step, and day 1 has no residual.

**Correction:** a `#[cfg(test)]` hook overwrites the recorded day-1 daily maximum of the living zone with NaN while the zone state stays finite. The day-2 comparison fails with `NanDetected` naming that zone and day 2.

### X-24 (B-6) No producer emits `None`, so the catalogue rule alone yields no null

**Where:** ledger 832-845, 860-862.

**Problem:**
- COP is always `Some` (`heat_pump/heater.rs:1583, 2663`; `air_conditioner.rs:1409, 1943`).
- SHR is inserted as 1.0 at init (`ac_config.rs:32`) and set on every step (`air_conditioner.rs:1263`).
- The ASHP is two pieces of equipment, and only the cooler has an SHR column.

**Correction:**
- The Change makes producers write `None` for undefined quantities: COP when runtime is 0 or the mode is Off; SHR only when sensible cooling is above 0, with the init insert removed.
- The test reads: "the ASHP Cooler's SHR is null in every non-cooling step; the Heater's COP is null in every Off step". It asserts that at least one Off step and one heating step exist.

### X-25 (B-5) The PV #1 defect does not occur, and five classifiers are missed

**Where:** ledger 753-755, 765-797.

**Problem 1:** `assign_instance_names` (`hpxml/equipment.rs:425-446`) leaves `spec.name` as "PV" and writes "PV #1" only into `instance_name`. The cited sites read `spec.name`, so `instance_qualified_equipment_rolls_up_by_typed_end_use` probably passes at the parent commit.

**Problem 2:** the classifier list omits `has_soc`, `is_heat_pump_heater`, `is_cooling_equipment`, `is_pv` and `is_compressor_equipment` (`columns.rs:1105-1147`). Their call sites are `mod.rs:431-433, 533, 6644-6646, 6946` and `columns.rs:550, 712`.

**Problem 3:** the tests name no verbosity and no schedule. The name-gated columns exist only at verbosity 3, 7 or 8, so the tests can pass vacuously.

**Correction:**
- Run the PV test at the parent commit. If it passes, restate the PV bullet as a latent risk and the test as a regression guard.
- Extend the deleted list and the `no_name_classifier_remains` grep to those five classifiers.
- State the test inputs: verbosity 8 and a schedule the test writes.

### X-26 (B-4) The test's "February's first step" cannot be observed

**Where:** ledger 723-730.

**Problem:** `BillingPeriodSummary` (`hares-tariff/src/billing.rs:409-420`) records no per-step membership.

**Correction:**
- January `total_import_kwh` equals the step-order sum of net import (kW × 1 h) over the 168 steps from Jan 25 00:00 to Jan 31 23:00.
- February's equals the same sum over the 120 steps from Feb 1 00:00 to Feb 5 23:00.

### X-27 (B-10) A second relative-defaults fallback and partial load failures are not covered

**Problem 1:** `mod.rs:2104-2110@ebdb1a17` (`:2151` at the head) has `bp.defaults_path.clone().unwrap_or_else(|| PathBuf::from("defaults"))`, which feeds `inject_schedule_into_specs`.

**Problem 2:** `DefaultsStore::load` downgrades failures:
- an envelope LUT load failure is a warning, after which the lumped `fallback_r` path runs (`defaults.rs:492-498`);
- sub-tables are optional (`:507-513`);
- strictness depends on `HARES_STRICT_DEFAULTS` (`:462`).

**Correction:**
- Add the `mod.rs` site to the evidence; it takes `DefaultsSource.base`.
- "Any load failure, including the envelope LUT and every sub-table the loader reads, is a construction error naming the path; `HARES_STRICT_DEFAULTS` is removed." Alternatively, cite the entry that owns this.

### X-28 (B-14) The census misses config fields

**Problem:** these fields are not in the table and not excluded:
- `GeneratorConfig.delta_kw_per_s` and `KEY_DELTA_KW_PER_S` (`generator.rs:79, 356`)
- `KEY_EVENT_POWER_KW_SERIES` (`event_load.rs:45`), written at `schedule_resolve.rs:1235, 1264`
- `power_profile_max_kw`, also written at `schedule_resolve.rs:1315`
- nested payload fields:
  - `BmsMode::SelfConsumption.surplus_deadband_kw` (`hares-types/src/equipment.rs:879`)
  - `ChargingStrategy::SolarSurplus.min_charge_rate_kw` (`:615`)
  - `ChargingStrategy::V2G.max_export_kw` (`:625`)
  - their Python surfaces (`py_enums.rs:1699, 1721, 2585`)
  - the consumer's `der_inject.py:157-180`
- `zone_capacitance_kwh_per_k` (`config.rs:287`, `hvac/hvac_core.rs:415`)
- EV keys `BatteryCapacity` (kWh), `MaxChargingPower` (kW) and `range_miles` (`ev/config.rs:10-16`)

**Correction:**
- Add rows: `delta_w_per_s`, `event_power_w_series`, `surplus_deadband_w`, `min_charge_rate_w`, `max_export_w`, and `zone_capacitance_j_per_k` (or exclude it as injected state, with that reason).
- Widen the census to `_(kw|kwh|kva)(_|:|")` over `crates/hares-equipment/src`, `crates/hares-types/src` and `crates/hares-core/src/dwelling/synthetic.rs`.

### X-29 (B-14) Actor configuration keys are neither in scope nor excluded

**Problem:** `actor_registry.rs` reads actor parameters in the same units under the same names as the renamed keys:
- `:383` `daily_drive_miles_mean`
- `:393` `fuel_economy_kwh_per_mi`
- `:394` `capacity_kwh`
- `:395` `average_speed_mph`
- `:396` `max_charge_kw`
- `:411` `range_anxiety_miles`
- `:413` `away_charge_power_kw`
- `:486` `surplus_deadband_kw`
- `:540-541` `max_charge_kw`, `max_discharge_kw`

**Correction:** bring them in, or add actor configuration to "Not in scope" with a reason.

### X-30 (B-14) "The only EV input before IN-9 is the HPXML form IN-9 deletes" is false

**Problem:**
- `resolve_loads.rs:867-918` builds `EvConfig` from a `PlugLoadType` "electric vehicle charging".
- The parity fixtures `cz6b_pv_battery_ev`, `cz2a_pv_ev`, `cz5a_ev_charging` and `cz5a_ev_only` use that path.
- Python `EV(...)` is another input path.

**Correction:** capture an EV golden from `cz6b_pv_battery_ev` (with a battery control override, per OD-8) in the first B-14 commit, or state why not, and delete the sentence.

### X-31 (B-14) Some input surfaces cannot report a removed key, and the lookup key format is unspecified

**Problem 1:** `SyntheticEventLoadConfig` (`synthetic.rs:66-70`) has no `deny_unknown_fields`, so an old `active_power_kw` is silently ignored.

**Problem 2:** `synthetic.rs:936-941` and `hpxml/xml_helpers.rs:357` carry the key as an HPXML extension element, a surface no row covers.

**Problem 3:** the raw override path accepts any key.

**Problem 4:** the merge cannot use the rows as written:
- the merge sees type names "PV" and "Battery", not "PvConfig";
- `capacity_kwh` and `crankcase_heater_kw` are shared by several configs;
- `phase_<n>_power_kw` is a pattern;
- `PvArray.capacity_kw` is nested under `arrays[]`;
- `removed_key(key)` is an exact match on a bare key.

**Correction:**
- Add `deny_unknown_fields` and a `REMOVED_KEYS` check to the synthetic event-load table.
- Rename the HPXML extension element and make the old name an error.
- Check every override key, typed and raw, against `REMOVED_KEYS` before deserialisation.
- Specify the qualified lookup (equipment type name to config type), pattern rows, and nested-array keys.

### X-32 (B-14) `uom` is missing from crates that convert

**Problem:** `uom` is a dependency of hares-equipment, hares-io and hares-physics only. The catalogue fills in hares-python (`PyBattery::from_spec` `py_equipment.rs:253-260`, `PyEv::from_spec` `:763-841`) and the conversions in hares-core have no `uom`.

**Correction:** put the conversions on `BatterySpec` and `VehicleSpec` in hares-equipment (`capacity_j()`, `max_charge_w()`, `fuel_economy_j_per_m()`) and use them from Rust and Python. Otherwise state the added dependency.

### X-33 (B-1a) The EnergyPlus albedo claim is wrong

**Where:** ledger 80, 199-200.

**Problem:** EnergyPlus does not carry the file's albedo into per-timestep weather. It is read only into `wvarsMissing` (`WeatherManager.cc:2936-2938, 3031`), and the interpolation loop (`:3093-3121`) has no albedo assignment.

**Correction:** "Snow indicator: the hour's record (`:3010, 3121`). Albedo: EnergyPlus drops it; holding the hour's record is a HARES choice." State the same in B-1a's field list.

### X-34 (B-1a, B-1b) Field coverage differs from EnergyPlus without saying so

**Problem:**
- EnergyPlus interpolates total sky cover (`:3101`); HARES has no such field (`weather.rs:451-467`).
- EnergyPlus stores no GHI (`WeatherManager.hh:713-732`), so applying `SolarInterpolation` to GHI is a HARES extension.

**Correction:** state both in the binding section and in `PHYSICS_DECISIONS.md`.

### X-35 (B-1a) The tests omit inputs the file's own rule requires

**Where:** ledger 28-31.

**Problem:** these tests lack required inputs:
- no offset: `hourly_step...` and `first_hour...`
- no duration: `leap_year_start_after_feb29...` and `start_on_skipped_feb29...`
- no duration or step: `actual_year_run_outside_the_file...`
- no offset, duration or step: `actual_year_leap_run_without_feb29...`

"Reads the Mar 1 records" also has no observable.

**Correction:**
- State the missing values: -07:00 for the cz4a starts; 2020-02-27T00:00:00-08:00, 3 d at 3600 s for the actual-leap test.
- The observable is the outdoor dry-bulb column against B-1a's interpolation of the named records.

## Minors

### X-36 (B-1a, B-1b) Citation corrections

- The "only affects the first hour" quote is at IO Reference `group-location-climate-weather-file-access.tex:721`; the code is `WeatherManager.cc:3054-3055`.
- The "Treat Weather as Actual" default No is at `Energy+.idd.in:1480-1484`, not doc 709-716.
- Half-hour logic: `:3074-3091`, not 3066-3082. `NextHr`: `:3061-3065`.
- Partial-period check: `:941-991`. Actual-year range: `:904-926` with the fatal at `:990`.
- `CurrentYearIsLeapYear` is forced false at `:1896-1900` only on the multi-year rollover; cite `:872` too.

### X-37 (B-2) EnergyPlus detail to add

- The minimum-above-maximum reset is `HeatBalanceManager.cc:686-697` (a warning).
- Non-convergence is `ShowSevereError` per zone (`:3387-3424`).
- Temperatures are ZTAV.
- Loads are `airSysHeatRate` and `airSysCoolRate` (`:3237-3247`).
- The IDD gives Loads Convergence Tolerance `\units W` (`:542`), but the code uses it as a fraction.

### X-38 (B-1b) Odd N needs a precise rule

**Correction:** define before, at and after the half hour as `2·ts < N`, `2·ts == N` and `2·ts > N`.

### X-39 (B-1a) Rules to tighten

- Ground temperature is held at the hour its step end lies in.
- An hourly IntervalStart record stamped t covers the hour ending t + 1 h.
- The "> 1 h step keeps the interval mean" rule is limited to hourly files.

### X-40 (B-1a) The modulo citation is off by a few lines

**Correction:** the modulo is at `environment.rs:532-536`. The method table spans `weather.rs:568-713` (precipitation at `:708`, albedo at `:709-713`). At 900 s the first 30 minutes are a PCHIP blend.

### X-41 (B-1c) Leap-adjustment sites and tests are under-cited

**Correction:** the leap adjustment also sits at `environment.rs:448-450, 869`. The tests that pin it (`:1975-2040, 2090-2130`) are replaced. `wf_allows_leap_years` is `weather.rs:294` / `epw.rs:167`, not `psm3.rs:307`. Add "in a non-leap run year a typical file's Feb 29 is skipped" (`WeatherManager.cc:2817`). The start-on-Feb-29 check belongs to `DwellingBlueprint::validate()`, not `SimulationConfig::validate()`.

### X-42 (B-2) Wrong lines and commit for the `py_dwelling.rs` citation

**Correction:** `py_dwelling.rs:2474-2480` at `872cf289`, not 2473-2479. Lines at `ebdb1a17`: 2725, 4992-5073, 5039-5043, 5066-5071, 2780-2787.

### X-43 (B-2) The manifest key location and value mapping are misstated

**Correction:** the key is per `[[home]]` in `frame_golden` (`manifest.rs:56`), not in `[simulation]`. The synthetic key is `[simulation] initialization_duration_s`. Mapping: 0 becomes `warmup = "disabled"`; any positive value takes the default. This covers the BR-14 and FL-1 manifests too.

### X-44 (B-2) Candidate D is not cold-climate

**Correction:** cz6b uses the same Denver TMY3 weather.

### X-45 (B-3) B-3's verification contradicts B-2

**Problem:** B-2 omits `warmup` in the 604800 manifests; B-3 says every manifest declares it.

**Correction:** "manifests run through the Rust adapter, which B-3 does not change".

### X-46 (B-3) `consumer_shape_900s` stops mirroring the consumer

**Problem:** after B-3 the consumer, which passes no keyword, warms up, while `consumer_shape_900s` stays `warmup = "disabled"`. The per-entry deltas B-4 to B-14 that feed B1's attribution (CN-4) are then measured on a shape the consumer no longer runs.

**Correction:** record this in CN-4's attribution note, or add a warm-up twin.

### X-47 (B-3) A test names no inputs

**Correction:** `initialization_duration_kwarg_names_warmup` states its inputs: the cz4a inputs with `initialization_duration=0` on `from_hpxml` and `DwellingConfig`.

### X-48 (B-5, B-6, B-11) Citation and evidence corrections

- B-5: `is_hvac_or_wh` is `columns.rs:1089-1103`. `extract_unit_from_name` is already removed by OR-1; say so.
- B-6: "total gas" as a summed cell has no evidence line; cite one or drop it.
- B-11: the `Mode (-)` column unit is "-", not "enum". The fix still holds.

### X-49 (B-9a, B-9) Fixture and naming details

- The empty path is `fleet.rs:193`. `resolve_weather_path_for_building` is at `:836-849@ebdb1a17`.
- The source parquet is not in the repository: `~/.cache/ochre_next/resstock_meta/upgrade0_2025_1.parquet`, sha256 `83a288b3...`. The recorded command names the file and its hash and writes rows in `bldg_id` order.
- `parse_weather_station_fips` returns `Option` silently; make it `Result`.
- B-9: "the weather file's own year" is defined for IntervalEnd files as the year whose hours the records cover.
- B-9: the unresolved-weather error names every unresolved building.

### X-50 (B-10) Callers, inputs and package path

- `fleet.rs:1174, 1191` are tests.
- Missing test callers: `crates/hares-core/tests/engine.rs:158-464`, `wiring_properties.rs:146-464`, `autosize_from_config.rs:181`.
- The missing, unreadable and empty tests name inputs, and "unreadable" asserts a skip when run as root.
- The package is `ochre_next` until PY-7; add PY-7 to Dependencies.

### X-51 (B-13) Test inputs

- "Declared R set to its film sum" can land a few ULP above the film sum in SI. Use an IP value whose SI conversion is ≤ the film sum.
- `base.xml` has no material-layer boundary (`hpxml/building.rs:2239-2244` skips NominalRValue-only layers), so that half of `slab_and_material_layer_boundaries_are_unchanged` is vacuous. Use a test-built boundary.
- The LUT branch is `conversions.rs:240-291` and the clamp is `:149`. Mention the R ≥ 17.6 clamp to the Minimal row (`envelope_lut.rs:571-576`).

### X-52 (B-14) Details

- `merged_equipment_config` spans `conversions.rs:773-841`.
- The `hpxml_pv_capacity_enters_in_watts` fixture has two systems (4000 and 1500 W).
- The battery under `bms_mode "Manual"` never cycles; use a cycling mode.
- Name the EV measurement test: `nightly_strategy_keeps_ev_charged_through_a_sustained_cold_snap`, 2018-01-01T00:00:00-10:00, 45 d.
- The battery and generator raw-key consts are `#[cfg(test)]` and the EV raw path is rejected at init. Delete them rather than add rows.
- The RT-6 names differ: `nameplate_w` vs `capacity_w`, and `energy_capacity_j` / `power_capacity_w` vs `capacity_j` / `max_charge_w`. Define the mapping.
- "37 lines in 5 files" reproduces, but only 28 are equipment-config surface.
- Out-of-scope reasons for `heating_capacity_kbtu_h` (`synthetic.rs:159`), scheduled-load therms/h, and the PV-sizing kW API that feeds `capacity_w`.
- `register_pv_roof_shading` (`mod.rs:1420-1435`) never fires for HPXML PV; record it in `10-slop-cleanup.md` or `11-other-defects.md`.

### X-53 (CN) Line corrections

| Entry | Ledger cites | Correct |
|---|---|---|
| CN-0 | `scale.py:466-500` | `:467-501` |
| CN-0 | `scale.py:571-580` | `:573-583` |
| CN-0 | payload fields | add `county` |
| CN-11 | `engine.py:208-225` | `:209-225` |
| CN-7 | `engine.py:271-298` | `:272-298` |
| CN-7 | `engine.py:406-413` | `:406-407`, helper `:411-472` |
| CN-1 | `frame_hash` `engine.py:252-268` | `:249-264` |
| CN-6, CN-10, CN-11 | `fit.py:214-221` | `:215-222` |
| CN-10 | `inject.py:197` | `:198` |
| CN-1 | `suite_der_inject.py:179-200` | `:178-198` |
| CN-1 | `suite_der_inject.py:170-176` | `:166-175` |
| CN-3 | `suite_inject.py:372-392` | `:353-398` |
| CN-16 | `suite_ami2.py:1446` | `:1441-1442` |
| CN-16 | `suite_ami2.py:1473` | `:1481` at `c71c542` |
| CN-1 | `adapter.py:350-362` docstring | `:342-353` |
| CN-1 | `registry.py:218-222` | `:221-224` |
| CN-14 | `suite_network.py:3006` | also `:3022` |

### X-54 (step 4b) The skip mechanism is misdescribed

**Correction:** only 4 of the 8 `suite_coverage` cases read `engine_schema.json`; all 8 skip through the suite-level `_needs_schema` guard (`suite_coverage.py:23-31`).

### X-55 (CN-6) `compare --declared` does not exist yet

**Correction:** CN-6 adds the `--declared` option to `capture_de_trace.py` (absent at `5c651bc`, `:224-229`).

### X-56 (CN-5) Several pools at once

**Problem:** `wave_balanced_workers` gives a different pool size per batch, and `_get_pool` (`parallel.py:40-47`) keeps one persistent pool per size, so until CN-10 several spawned pools coexist.

**Correction:** state this, or reuse one pool.

### X-57 (CN-1) Dead plumbing to remove

**Correction:** the dead `rotation` plumbing also sits at `registry.py:40, 70, 305-307` and `inject.py:156-158`. The `RESAMPLE_OVERRIDES` import is at `engine.py:32`, with a comment at `grid.py:168-173`.

## Verified correct

### Engine-under-test mechanism

- **Step 2:** in a scratch copy of the consumer's `pyproject.toml`, `uv.lock` and `.venv` (uv 0.12.8), `uv sync --locked --extra engine --no-install-package ochre-next --dry-run` prints exactly "Would uninstall 1 package: ochre-next (from file:///home/rich/src/HARES)". The lock lists pyarrow only as an engine dependency (`uv.lock:181-188`).
- **Step 3:**
  - `uv pip install` has `--build-constraints`, `--reinstall-package` and `--editable`.
  - The `sed` extraction yields `maturin==1.15.0` at `872cf289`, `75a182a0` and `8b09ac4a`.
  - The installed editable dist-info's `WHEEL` reads `Generator: maturin (1.15.0)`, and the grep path matches.
  - The project name `ochre_next` normalises to `ochre-next`.
  - No engine entry adds a Python runtime dependency, so `--no-deps` is sound.
- **Step 4b:** `python -m calibration.schema --emit --all-cached` exists and writes only `results/engine_schema.json`. The current file is dated 2026-10-02 06:01 with `engine_sha256` `0e673f6cd097168b`, as CN-17 states.
- **Step 5 and `HARES_REPO`:** resolution is as `paths.py:28-35` states, and `summary()` has the seven fields.

### Gate

- `run_all` exits 1 only on failure (`run_all.py:127-141`).
- The line prefixes are as cited (`testkit.py:190-213`).
- All seven base-skip names exist (`suite_fit.py:333`; `suite_ami2.py:2370-2685`).
- The rule "skips equal the base set exactly, failures empty", with any `suite_coverage` skip a failure, cannot pass on skips or new failures.
- `build_bundle.py` behaviour is as cited at `aec2e3f`.
- The drift check passes against `21d90588` and `1d60f478`, run from a scratch archive.

### DE-trace capture and comparison

- `capture_de_trace.py` at `5c651bc`: `compare` diffs only `premises`, with exit codes 0, 1 and 2.
- `RULES` is empty.
- `capture` refuses an engine from outside `paths.HARES_REPO`.
- The B0 files hold a header plus a payload of premises.

### Contract-drift assignment and its six categories

The categories are as cited at `hares_contract_check.py:65-126`. Engine rows are assigned to consumer entries as follows:

| Engine rows | Consumer entry |
|---|---|
| 1-12 | CN-17 |
| BR-11 to row 26 | CN-13 |
| 28-71a | CN-1 |
| 79-81 | CN-14 |
| 83-94d | CN-3 |

Each range is the first consumer gate after those rows. The rows themselves are subject to X-17.

### Bitwise neutrality, re-baseline and pin

- **Neutrality:** every engine row before PY-8 carries a B0 check, every row from PY-8 to 78 carries a `consumer_shape_900s` golden check, and the consumer block is gated against B0 (CN-6 with its declared rule).
- **Re-baseline:** B1 happens once, at CN-4, after every numeric row, FL-5, OD-9, IN-8, IN-9, CN-14, CN-3 and CN-2. CN-2, CN-3 and CN-14 are absorbed by B1.
- **Pin:**
  - The pin advances once, at CN-15, after the operator push.
  - Intermediate commits keep `0ec757cb` and carry `Engine:` trailers.
  - The CN-15 lock rename sequence is consistent with CN-13 keeping the `ochre-next` source until then.

### CN citations

Every CN citation not listed in X-53 is correct. This includes CN-5's sizing defect, CN-6's figures (512, 206 and 494 become 510, 204 and 492), CN-9's `check_losses.py` lines, and CN-14's offsets.

### EnergyPlus

- **IO Reference interpolation:** formula, weights, hour-1 definition and the day-boundary quote (`weather-data-hourly-interpolation.tex:7-75`).
- **Engineering Reference** midpoint quote (`:304-306`).
- **Interpolation guard and fields:** `:3043`; linear fields `:3095-3102`; IR and `calcSky`; wind direction; precipitation `:3118-3119`.
- **SolarInterpolation** weights for N = 2 to 12, reproduced in scratch.
- **Test values:**
  - B-1b, four steps: 700, 800, 600, 400.
  - B-1b, six steps: 666.67, 733.33, 800, 666.67, 533.33, 400.
  - Hour 24: 450, 500, 400, 300.
  - Wind direction: 355, 0, 5, 10.
- **IDD and file handling:**
  - Timesteps per hour (`Energy+.idd.in:925-936`).
  - Sub-hourly mismatch text (`:715-722`).
  - Standard-time quote (`:830`) and leap-day quote (`:597-601`).
- **Leap years and day of week:** `WFAllowsLeapYears` (`:7896`), `IsLeapYear` (`:8456`), `LeapYearAdd`, `SkipThisDay` and its warning, the missing-Feb-29 fatal only for actual weather, and `CurDayOfWeek` advancing per day read.
- **Calendar departure:** the day-type departure is exactly the leap-day decision (`leap_day_day_types`).
- **Warm-up (B-2):** defaults 0.4, 0.04, 1 and 25; range (0, 0.5]; the "should NOT be set less than 25" text; per-zone checks; the 100 W floor, the 1e-4 W threshold and the `/today` formula; the first day compared with a zeroed day (`:2888-2891`); `DataHeatBalance.hh:96-97`.

### Numeric-fix code and fixtures

- **Defects real at the cited code:** B-1a's offset and shift (`environment.rs:823-847`) and PCHIP/ZOH defaults; B-2's NaN fold, ignored duration and literal 25, and the TOML/Python zero split (`mod.rs:1935-1937@872cf289`); B-3; B-4; B-6; B-7; B-9a; B-9; B-11; B-12; B-13 (`conversions.rs:147-166`).
- **Fixtures:**
  - The parity fixtures exist (cz4a TMY3, 8760 records, leap flag No).
  - `base-pv.xml` has two PV systems; `base-misc-loads-large-uncommon.xml` has `Pool1Heater` gas and `HotTub1Heater`; `base.xml` Wall1 is R-23 with no layers.
  - The ResStock buildings and weather stations exist, as do the golden manifests with the stated settings.
  - `flat_tariff.json` comes from BR-14 before B-4.
  - The B-9a metadata rows are WA, G5300330, weight 253.9036...
- **Arithmetic:** 384 rows; Wed, Fri, Sat; 168 steps; the B-12 Instant first row at 02:00.
- **B-14 conversions:** `uom` 0.38 provides kilowatt_hour, kilowatt, mile and newton, and Energy/Length = Force. Checked in scratch: 0.3 kWh/mi = 671.08 J/m, and 19.2 kW = 19200 W exactly, which matches OD-2 site 7's 19 200 W.
- **B-14 citations:** every row of the table is correct at `8b09ac4a`.

### Order

- The weather triple B-1a/B-1b/B-1c lands after IN-6, PY-8, OR-7, RT-2 and IN-2.
- B-2 follows B-1c; B-6, FL-5 and B-11 are adjacent; B-9a precedes B-9.
- IN-7 (row 66a) precedes B-13 and composes as stated.
- B-14 lands between IN-8 and IN-9.
- CN-14 follows the weather entries; CN-3, CN-2 and CN-4 come at the end; CN-15 comes after M-4b and the push.
