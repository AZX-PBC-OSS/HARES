# L2 resolution: plan.md §2, ledger/03, ledger/05, ledger/12

Scope: every finding of `verify-L2-b.md`, and every finding of
`verify-L2-e.md` that concerns `plan.md` §2, `ledger/03-output-reporting.md`,
`ledger/05-python-surface.md` or `ledger/12-inputs.md`. Code claims were
re-read at HEAD `872cf289` (branch `feat/output-results`, which carries
OR-5 as `dc033cee` and `469ea955`). Binding: `decision-log.md`, including
`dl-i08-execution.*` and its `findings_for_later_entries`.

Counts: verify-L2-b 24 findings, 24 fixed, 0 rejected. verify-L2-e 23
findings in scope, 23 fixed (9 of them in these files only, with the
rest passed to the owning file below), 0 rejected. Further defects found while fixing:
4, all fixed in these files. One operator question.

## verify-L2-b.md

| # | Resolution |
|---|---|
| 1 | fixed: `plan.md` §2.1 and §2.7. §2.1 now carries the manifest graph as a table ("`hares-equipment` \| `hares-types`, `hares-physics`, `hares-control`"; "`hares-io` \| `hares-types`, `hares-physics`, `hares-envelope`, `hares-control`, `hares-equipment`") and "a type an `Equipment` method returns or an equipment config implements lives in `hares-types`". §2.7: "`// crate: hares-types (crates/hares-types/src/resolved.rs, new)` ... `pub struct ResolvedParameter`". |
| 2 | fixed: `plan.md` §2.7, 12 IN-3. "`// crate: hares-types (crates/hares-types/src/schema.rs, new)`" for `FieldSchema`, `ValueType`, `DescribeFields`; `InputSchema` and `OverrideClassSchema` stay in `hares-io`. IN-3: "`FieldSchema`, `ValueType` and `DescribeFields` in `hares-types` (`plan.md` §2.7: the equipment configs live in `hares-equipment`, which cannot see `hares-io`)". |
| 3 | fixed: `plan.md` §2.2 "Catalogue digest" ("`sha2`, which `hares-io` gains in `ledger/03-output-reporting.md` OR-7, the entry that first writes it"), `Catalogue::digest()` in the §2.2 block, §2.1 `Provenance` comment "`Catalogue::digest()` (§2.2), defined in 03 OR-7"; 03 OR-7 "This entry adds `Catalogue::digest()` ... and the `sha2` dependency of `hares-io`" with `catalogue_digest_is_canonical`; 05 PY-6 "copied from `Catalogue::digest()`"; 12 IN-1 "`sha2`, which `hares-io` has had since OR-7". |
| 4 | fixed (option a): 12 IN-6 "This entry creates `hares_io::config::REMOVED_KEYS` ... with its first row, `resample_overrides` ... and the one lookup `removed_key(key)`"; the TOML surface is named ("a synthetic TOML file whose dwelling table holds `resample_overrides`"). 05 PY-8 "exist from `12-inputs.md` IN-6"; 03 OR-3 "`REMOVED_KEYS` (created by `12-inputs.md` IN-6)"; `plan.md` §2.7 table gains a "Row added by" column. |
| 5 | fixed (EPW layout kept): 12 IN-6 "returns EPW content at `time_res_s` ... header lines 1 to 7 ... are copied verbatim; line 8 (`DATA PERIODS`) states `3600 / time_res_s` records per hour"; the EPW reader accepts N records per hour with "`midpoint_offset_secs = 1800 / N` (the same interval-end label as hourly EPW)"; test `subhourly_epw_header_fields_equal_hourly`; "**Stop condition:** ... If any parity case, oracle or `test_ochre_parity.py` metric leaves its existing tolerance, the entry stops before committing". |
| 6 | fixed: 12 IN-5 "The rule applies to a schedule read from a file ... each such column records `SetpointSource::GeneratedConstant { value_c }` ..., raises no warning, is never read as Fahrenheit, and so never marks a fleet home `Degraded`"; `plan.md` §2.7 `GeneratedConstant { value_c: f64 }, // ... 11 OD-9 deletes it`; test `generated_schedule_setpoint_columns_raise_no_warning_and_are_celsius`. |
| 7 | fixed: IN-5 is structural ("Column values are read as today (Celsius) in this entry"); new 12 IN-8 "**Kind:** fix, numeric ... its own commit and its own row in the numeric block, delta recorded" with `setpoint_column_is_read_in_fahrenheit` and the delta run. `water_heater_setpoint`: IN-5 "A schedule file with a `water_heater_setpoint` column becomes a construction error naming the column" (the code maps the column but never reads it, `schedule_resolve.rs:186-189`, `:743-790`). |
| 8 | fixed: 03 OR-3 "Python can request output from this commit on: the frozen pyclasses `ReportSpec(...)`, `Selection.verbosity(n)/named([...])/pattern(s)`, `Frequency.timestep()` and `Destination.memory()/parquet(...)/csv(...)`", interim `simulate()` frame and test migration; OR-12 adds `HealthPolicy`, `HealthPolicy.strict()` and `HaresHealthPolicyError`; PY-9 moves to directly after OR-6 (05 intro "PY-9 runs directly after OR-6"); `plan.md` §2.2 "Each variant arrives with the entry that implements it ... The Python constructor of each variant lands in the same commit". |
| 9 | fixed: `plan.md` §2.1 "the workspace `serde` dependency gains the `rc` feature in ... OR-1's commit ... The hand-written `ser_arc_str`/`de_arc_str` helpers ... are deleted"; 03 OR-1 item 2 states it. |
| 10 | fixed: `plan.md` §2.7 "`Warmup` has a hand-written `Serialize` and `Deserialize` ... a string, which must be `"disabled"` ... or a table, which deserialises as `WarmupConfig`"; `#[serde(default, deny_unknown_fields)]` on `WarmupConfig` and `HealthPolicy`, `#[serde(default)] overwrite`, `rename_all = "snake_case"` on `Selection`, `Frequency`, `Destination`, `FirstHour`, `WeatherYear`; "Every config enum is externally tagged with `rename_all = "snake_case"`". |
| 11 | fixed: `plan.md` §2.7 keyword table ("The Python keyword of every `SimulationConfig` field is the Rust field name"; `site_location` is "`SiteLocationOverride(...)`, a frozen pyclass over the Rust type") and removed-key rows `defaults_path` -> `defaults`, `duration_s`/`time_res_s`, the four site keywords, `output_to_parquet`; 05 PY-8 keyword list and `python_keyword_names_equal_rust_field_names`. |
| 12 | fixed: 03 OR-5 "the two `observe`-gated `BIQUADRATIC_INDEX_CLAMPED` publications inside `step()` ..., which would drain it before the dwelling reads it, are deleted" (as landed in `dc033cee`: no `BIQUADRATIC_INDEX_CLAMPED` remains at HEAD); `curve_index_clamp_counted` "also runs under the `-F observe` block line". |
| 13 | fixed: `plan.md` §2.3 "`step_index: Option<u64>, // None for an event during warm-up or at its end`"; 03 OR-12 `strict_policy_escalates_warmup_nonconvergence` expects "`step_index: None`". |
| 14 | fixed (to the landed behaviour): 03 OR-5 "Each of those sites already pushed one warning per occurrence and keeps doing so (one warning per rejection, not one per equipment), so the gym's per-step `take_warnings` stream is unchanged"; ambiguous targets counted too; `plan.md` §2.3 Warning paragraph rewritten the same way. HEAD confirms: `rejected_signal_counted_per_run` asserts "every rejection stays a warning" (`mod.rs:14643-14700`). |
| 15 | fixed: 05 PY-11 "`labels` is a list of strings, required exactly when `kind == "enum"`"; tests `python_actor_enum_output_decodes_labels`, `python_actor_enum_without_labels_is_an_error`. |
| 16 | fixed: `plan.md` §2.2 and 03 OR-1 "each equipment's `descriptor().telemetry_fields`"; OR-1 lists `TestActor` (`actor.rs:477`), `NoStateActor` (`ev_driver/mod.rs:6303`), the five test files and the Python wrapper (`py_actor.rs:99`). |
| 17 | fixed: 12 IN-3 grep extended to "`\.path\(&\["\|\b(child_text\|child_f64\|child_energy_kwh\|descendants_named)\([^,]*, *"`" ("it prints 440 lines at HEAD `872cf289`"); `validation.rs` added to the evidence; the count "58 of them are `.path(&["..."])` calls" is from HEAD. |
| 18 | fixed: 03 OR-8 "three runs with `metrics = true` whose selection union is the `Verbosity(0)` preset". |
| 19 | fixed: 03 OR-15 "consecutive `Time` values differ by exactly `time_res` and there is no `Step Count` column; otherwise `MetricsError` names the mismatch"; test `metrics_from_table_rejects_non_timestep_table`. |
| 20 | fixed: 03 OR-10 "the restored run's rows whose period starts at or after the checkpoint's period start (`2023-01-28T16:00`) equal the uninterrupted run's rows". |
| 21 | fixed: 03 OR-16 owns "`RunResult.export_replay(dir)` and `hares.replay(dir, defaults=None)`" and `replay_round_trip_in_python`; it writes "`bldg_id`, `patches` ... `initialization_duration`" in `replay.toml` and "`overrides.json` (the `overrides` value as JSON, because it is a `serde_json::Value` that may hold `null`, which TOML cannot encode)"; PY-9 no longer lists replay. |
| 22 | fixed: 05 PY-1 "returns pyo3-arrow's `PyTable` class (a `#[pyclass(module = "arro3.core._core", name = "Table")]` ...; there is no `pyo3_arrow` Python module)", stub "`ArrowStreamExportable`, a `typing.Protocol`"; PY-10 `PyRecordBatch` annotated "`ArrowArrayExportable`". |
| 23 | fixed: PY-5 cites `py_gym.rs:494`; PY-6 "`build.rs` resolves each from `CARGO_MANIFEST_DIR` to the workspace root (`../../.git/HEAD`, ...) and emits only paths that exist"; `plan.md` §2.7 "The handle types, `ForcingKey`, `ForcingTable`, `DwellingBlueprintBuilder` and `ObservationPlan` (§2.9) are opaque"; §2.1 defines `TariffSnapshot`, `BillingSnapshot`, `LocalInstant`, `DemandWindowSnapshot`, `MetricsSnapshot`, `MetricsAccumulators`, `ReducerSnapshot` with fields; 12 IN-4 "`site` is the `site` dict of `DwellingBlueprint.validate()`" and IN-2 states that dict's keys. |
| 24 | fixed: `verification/L1-resolution-b.md` row 2 now reads "for SOC bounds the existing `ev_soc_check_fails_loudly_when_core_output_soc_is_absent` (`check_soc` is deleted as vacuous ...)". |

## verify-L2-e.md (findings that concern these files)

| # | Resolution |
|---|---|
| 1 | fixed: `plan.md` §2.7 removed-key rows ("`latitude`, `longitude`, `elevation_m`, `utc_offset_h` (Python keywords) \| `SimulationConfig.site_location.latitude_deg`, ..."; "`defaults_path` ... \| `defaults`"); 05 PY-2 "Disposition of every `KNOWN_KWARGS` entry at HEAD `872cf289` (`py_dwelling.rs:2183-2206`, 22 entries)" and `removed_keyword_names_replacement` "parametrized over every removed or renamed keyword ... each on `Dwelling.from_hpxml`, `DwellingBlueprint.from_hpxml` and `DwellingConfig`". A removed key is checked before the current-field rule, so `latitude=` never names a field that does not exist. |
| 4 | fixed (design frame; RT-4 and RT-2 text passed on below): `plan.md` §2.7 "`DwellingBlueprintBuilder::new(inputs: ParsedInputs, settings: DwellingSettings) -> Result<Self>; // no parse: shares the handles`", `ParsedInputs`, `DwellingSettings`, "two blueprints built over one `Arc<ParsedWeather>` on one time grid and site share one table", and the Python classes "`hares.ParsedHpxml`, `hares.ParsedWeather`, `hares.ParsedSchedule` ... `hares.ParsedDefaults(base, files=None)`" accepted by `from_hpxml`. |
| 6 | fixed for IN-5 (B-4, B-9 and OD-2 site 6 passed on): the Fahrenheit reading is 12 IN-8, a numeric-block entry with "Delta: a 48 h run of the IN-5 inputs with a 70.0 heating column, before (read as 70 C) and after", and a `docs/PHYSICS_DECISIONS.md` section. |
| 7 | fixed: as verify-L2-b 21. |
| 8 | fixed: as verify-L2-b 11; the §2.7 table states the accepted Python type per field ("`duration`: `int` seconds or `timedelta`", "`site_location`: `SiteLocationOverride(...)`", "`reports`: `list[ReportSpec]`", "`warmup`: `WarmupConfig` or `None`"); `duration_s` and `time_res_s` are `REMOVED_KEYS` rows. |
| 9 | fixed for 03 and 05 (B-* passed on): OR-5 "Every dwelling below is the BESTEST case 600 (`tests/fixtures/bestest/600.toml` ...)" with each test's inputs as landed; OR-6, OR-8, OR-15 use "the `leap_year_february_900s` inputs"; OR-10 "the HPXML, schedule and weather of `tests/fixtures/parity/cz4a_ashp_hpwh/`, start `2023-01-25T00:00:00-07:00`, 12 days ... `tests/fixtures/urdb/pge_e_tou_c.json`"; OR-13 the enum pattern "code 2 for the first 48 steps and code 1 for the next 48 on day 1"; OR-14, PY-3, PY-4, PY-10, PY-11 name the cz4a inputs; PY-5 "the `tests/python/conftest.py` inputs ... seeds 0 and 1"; IN-2 "`tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh/`"; IN-4, IN-5 dates and steps. |
| 10 | fixed: new 12 IN-9 "HPXML vehicles and their chargers are simulated" (numeric, own row), reading `Vehicles/Vehicle/VehicleType/BatteryElectricVehicle`, `Battery/UsableCapacity`, `ConnectedCharger idref` to `ElectricVehicleCharger/ChargingPower` in W "converted to the EV config's kW field with `uom` at the parse boundary"; tests `resstock_vehicle_ev_is_simulated` (`bldg0176775`, `G3400270_2018.csv`) and `charger_power_read_in_watts`; "A home that declares both a `Vehicles` EV and an `electric vehicle charging` plug load ... is a construction error". Storing EV power in W is the operator question below. |
| 12 | fixed: 03 OR-7 "`docs/python.md` gains a "Time" section stating that `Time` is naive local standard time labelled at the start of its interval ... `hares:utc_offset_s` ... `Step Count` ... how a schedule row maps to a step (... `environment.rs:418-445`)" with `test_docs_time_example`; `plan.md` §2.5 points to it. |
| 13 | fixed: 05 PY-9 "Changes in this release" section, the committed `tests/python/fixtures/_hares_ebdb1a17.pyi` baseline and `stub_changes_are_listed`; "Every later entry that changes the stub updates this section in its own commit". |
| 19 | fixed: 05 PY-1 "built ... once per `ReportResult` and kept in a `OnceLock<RecordBatch>` on it"; test `second_export_does_not_recopy`. |
| 20 | fixed: `plan.md` §2.2 "each with up to three catalogue names nearest by edit distance"; 03 OR-2 test expects `"Total Electric Power (kW)"` first among the suggestions. |
| 22 | fixed: 12 IN-8 "converted to Celsius at parse with `uom` ... equals, on bits, the `uom` conversion of 70 F to Celsius computed in the test". |
| 23 | fixed for OR-11 (RT-3 passed on): 03 OR-11 "The census below decides every site: ... 504 lines in 117 files", tables per crate with a disposition for every row, the found defects, the hot-path rows and a stop condition. |
| 24 | fixed for OR-7 (B-2 passed on): OR-7 keeps `frame_starts_at_start_time_with_exact_row_count`. |
| 27 | fixed: `plan.md` §2.10's closing filler is deleted; the pointer to `08-consumer.md` stays. |
| 29 | fixed for OR-6 (RT-4 passed on): OR-6 "the three memory-growth readings of `02-measurement.md` M-3 item 5 ... re-taken after this commit ... (requirements R18, RL-1, RL-2 and X-4)". |
| 32 | fixed for §2.10 and docs (review.md passed on): "`SteppableFleet` gains no save or restore method now, and has none to call ... `docs/python.md` states that fleet stepping does not rewind (`ledger/05-python-surface.md` PY-9)". |
| 33 | fixed: `plan.md` §2.2 "The hard link plus removal is the no-clobber form of the rename the destinations decision requires". |
| 34 | fixed: `plan.md` §2.1 `LocalInstant { local: NaiveDateTime, utc_offset_s: i32 } // a local wall-clock time, never a UTC instant`; 03 OR-10 "each `DateTime<Tz>` bound stored as a `LocalInstant`". |
| 35 | fixed: `plan.md` §2.7 `pub base: PathBuf` and "RT-4 introduces the type with `base: Option<PathBuf>` ... and `ledger/09-numeric-fixes.md` B-10 makes it required". |
| 38 | fixed for OR-3 (RT-5, FL-1 passed on): OR-3 "The bench group `output_destination` (new, in `benches/single_building.rs`) holds ... `no_report`, `memory`, `csv` and `parquet`". |
| 39 | fixed: `plan.md` §2.10 entries for FL-11 and RS-9 with their basis and seam; R24 built as 05 PY-12; PY-14 built in PY-9 and OR-7 docs. |
| 43 | fixed for §2.4 (FL-1 passed on): "The bound is the chunk size times one home's live result ... at `Timestep` frequency it is that home's full selected-column frame". |

## Further defects found and fixed

- Checkpoint floats are not round-trip exact: every checkpoint is JSON
  (`checkpoint.rs:114,159-167`) and the workspace `serde_json` has no
  `float_roundtrip` feature, so OR-10's bitwise acceptance could not hold.
  Fixed: `plan.md` §2.1 and 03 OR-10 add the feature, with
  `restore_round_trips_every_float_bitwise`.
- The diagnostics writer would lose its gate and path at OR-3 (it reads
  `output_verbosity` and `output_path`, `mod.rs:2819-2830`; the decision
  log's `findings_for_later_entries` gives the writer to OR-3). Fixed: 03
  OR-3 states its gate and path until OR-4.
- ResStock sample weights: a metadata file without a weight column gives
  every fleet home weight 1.0 silently, and a missing column is reported
  as "building not found" (`resstock.py:747-805,1055,1095,1166-1179`).
  Fixed: new 05 PY-12.
- A Python keyword present at HEAD but not a Rust field
  (`output_to_parquet`) would have become a nameless `TypeError` at PY-8.
  Fixed: a `REMOVED_KEYS` row from PY-8, rewritten by OR-3.

The gym dict-path note of `findings_for_later_entries` is covered: 05
PY-8 builds the gym's config "passing every field the caller set" (OD-7
has removed the mapping path).

## Schedule changes (`ledger/00-schedule.md`, for its owner)

- PY-9 moves from row 54 to a new row 43a, directly after OR-6 and
  before PR-4. Its dependencies are now OR-6 and OR-12 (OR-15 dropped:
  `hares.metrics_from_table` lands with OR-15). Rows 46 (OR-8), 49 (OR-15)
  and 69 (OR-16) now depend on it and stay where they are.
- New row 30a after PY-5: PY-12, file 05,
  `fix: ResStock sample-weight lookups name the missing column or building`,
  consumer check B0 (CN-13).
- New rows 94b and 94c after OD-9: IN-8, file 12,
  `fix: read schedule-file thermostat setpoint columns in degrees Fahrenheit`,
  and IN-9, file 12, `fix: simulate HPXML vehicles and their chargers`;
  consumer check "delta" for both.
- The preamble rule "Each numeric fix (`09-numeric-fixes.md`, and
  `11-other-defects.md` OD-9)" gains `12-inputs.md` IN-8 and IN-9.
- Row 33 (IN-6) now precedes OR-3 as a dependency (REMOVED_KEYS); row 45
  (OR-7) precedes PY-6 (row 58), which now depends on it. Both orders
  already hold.

## Items for other owners

- `ledger/06-runtime-memory.md` RT-4: build `ParsedInputs`,
  `DwellingSettings` (`DwellingConfig` gains `settings`),
  `DwellingBlueprintBuilder::new(inputs, settings)` and the Python handle
  classes of `plan.md` §2.7; add
  `blueprints_from_different_hpxml_share_weather` (two HPXMLs over one
  `ParsedWeather`: `Arc::ptr_eq` on the weather handle, frames bitwise
  equal to separate parses) and its Python counterpart; `DefaultsSource.base`
  is `Option<PathBuf>` until B-10; drop "`context.md`: 45 to 48 ms per
  parse" (verify-L2-e 40) and add the M-3 item 5 after-measurements
  (verify-L2-e 29).
- 06 RT-2: add `blueprints_from_different_hpxml_share_forcing`
  (`Arc::ptr_eq` on the `ForcingTable`).
- 06 RT-5: the census row for `REJECTED_SIGNAL_COUNT` says "first per
  equipment a `Warning`"; OR-5 landed one warning per rejection (`plan.md`
  §2.3).
- 06 RT-6: `ResolvedParameter` lives in `hares-types`
  (`crates/hares-types/src/resolved.rs`); expose
  `SetpointSource::GeneratedConstant` until OD-9.
- 06 RT-3 (verify-L2-e 17): the factorization-count test is RT-3's; §2.9
  is unchanged.
- verify-L2-e 11 (gym reset discards the restored state, requirement
  RL-3): the reset path is `reset_with_seed`, which RT-4 owns; the entry
  belongs in 06 after RT-4 and OR-10.
- `ledger/09-numeric-fixes.md` B-2: add `warmup_toml_forms_round_trip`
  for the hand-written `Warmup` serde (`plan.md` §2.7); B-2 also moves
  `initialization_duration` out of OR-16's `replay.toml` into
  `SimulationConfig.warmup`; extend PY-2's `removed_keyword_names_replacement`
  with its row. B-2's copy of `frame_starts_at_start_time_with_exact_row_count`
  is a re-run of OR-7's (verify-L2-e 24).
- 09 B-1a: IN-6 adds sub-hourly EPW reading; B-1a's sub-hourly rule (an
  interval different from the step is an error; `RecordLabel::IntervalEnd`
  replaces `midpoint_offset_secs = 1800 / N`) applies to EPW files too.
- 09 B-1c: if it changes how a leap-year date maps to a schedule row,
  update OR-7's "Time" section of `docs/python.md` in its commit.
- 09 B-4, B-9 (verify-L2-e 6) and the warm-up and fixture naming of B-2,
  B-4 to B-7, B-9, B-12 (verify-L2-e 9).
- `ledger/11-other-defects.md` OD-9: its change also deletes
  `SetpointSource::GeneratedConstant` and the recording IN-5 adds for
  generated columns, so `generated_schedule_keeps_the_hpxml_setpoint_profile`
  sees `Hpxml`; it must land before IN-8 (row 94b), which no longer
  converts anything a generated schedule writes. OD-2 site 6 (verify-L2-e
  6). OD-1: its pickle test gains `HaresHealthPolicyError` from OR-12 and
  `HaresCallbackError` from PY-10 (not PY-9, as its line 41 says). OD-2
  site 7's 19.2 kW check applies to IN-9's converted value unchanged.
- `ledger/04-fleet.md` FL-1: record bytes per live home result
  (verify-L2-e 43) and name the bench target (38); FL-3 the FL-9
  sentence (36).
- `ledger/08-consumer.md` CN-1, CN-10: the Python keywords are
  `duration`, `time_res`, `site_location=SiteLocationOverride(...)` and
  `defaults`; CN-10 uses `hares.ParsedWeather`; the consumer's ResStock
  metadata must carry a weight column once PY-12 lands.
- `ledger/02-measurement.md` M-3 item 5 commands use `duration_s` and
  `write_output`; OR-6's re-measurement uses `duration` and `reports=[]`.
- `plan.md` §3: the `[test-release-invariants]` removal is OR-11's
  (unchanged); the public issue reference in OR-3's commit message is
  verify-L2-e 31, a §3 Writing rule.
- `review.md`: the `SteppableFleet` sentence (verify-L2-e 32) should match
  `plan.md` §2.10.

## Operator question

Equipment configs store energy and power in kWh, kW and kVA: about 30
fields across `crates/hares-equipment/src`
(`grep -rhoE "pub [a-z_]+_(kw|kwh|kva|kwh_per_mi)\b: " crates/hares-equipment/src`),
including the EV's `capacity_kwh` and `max_charging_power_kw` that
verify-L2-e 10 names. The SI rule says SI internally; the decision log's
purpose (runtime, output and performance) does not include a units
migration, and no decision covers one. IN-9 therefore converts the HPXML
watts to the existing kW field with `uom` at the parse boundary, and
`ResolvedModel` reports SI (`max_charging_power_w`, `battery_capacity_j`).
Should I-08 add an entry that moves every equipment config to SI (a
public override-key rename, bitwise-neutral only up to rounding, so a
numeric-block entry), or does that wait for a later initiative?
