# L3 resolution, pass d: consumer and numeric fixes

Files revised in place: `ledger/08-consumer.md`, `ledger/09-numeric-fixes.md`.
Findings resolved: every finding in `verify-L3-d.md` (X-1 to X-57), and
every finding in `verify-L3-e.md` that concerns these two files (8, 12, 14,
16, 17). Findings or halves of findings that belong to other files are
handed on under "Schedule changes" and "Items for other owners".

Checked against: HARES `feat/output-results` at `21d90588` (and `ebdb1a17`,
`872cf289`, `8b09ac4a` where a citation names them), read with `git show`;
the consumer `i08-consumer` at `c71c542` and `5c651bc`, read with
`git show`; the vendored EnergyPlus source, IDD and documentation at
`f7c103167a`; the ResStock metadata parquet in the cache (sha256 taken);
`decision-log.md` in full; `ledger/00-schedule.md` and
`ledger/02-measurement.md` (read only). One mechanic was re-measured in a
isolated scratch copy of the consumer's `pyproject.toml`, `uv.lock` and `.venv`:
`uv run --no-sync --extra docs python -c
'import markdown'` raises `ModuleNotFoundError` (uv 0.12.8). Neither code
repository was edited.

Counts:
- `verify-L3-d.md`, 57 findings (4 blockers, 31 majors, 22 minors): 57
  fixed in these files. Three take one of the options the finding offered
  and state why: X-20 (a partial-year file is an error, not supported),
  X-29 (actor configuration is out of B-14's scope, with its reason),
  X-46 (recorded in CN-4's attribution note, no warm-up twin). X-17 is
  fixed in `08` and needs schedule and log edits by their owners (below).
  X-52's last bullet is a new defect for `11-other-defects.md` (below).
  0 rejected.
- `verify-L3-e.md`, 5 findings concern these files: 16 and 17 fixed; 8
  fixed for B-11 (its FL-5 half is `04`'s); 14 fixed for `08` (its
  schedule half is the integrator's); 12 rejected as a B-14 scope addition,
  with its reason, and listed as an operator decision.
- 5 schedule changes, 9 items for other owners, 3 operator questions.

## Blockers

**X-1 (B-1a PSM3 fixture).** Fixed. The fixture is now a full year the
parser accepts as it stands: "`tests/fixtures/weather/psm3_15min_2019.csv`,
a full-year 15-minute PSM3 file that `psm3.rs` parses as it stands", with
the three header lines spelled out (Latitude, Longitude, Time Zone -7,
Elevation; every required column), "35040 records stamped 2019-01-01 00:00
to 2019-12-31 23:45", and "record `k` (0-based) has `Temperature` `-20.0 +
0.5·(k mod 96)` C, ... inside the parser's [-60, 55] C bound (`:148-152`);
`Dew Point` -40.0 C, never above the dry bulb (`:155`)". The test reads
"step `k` (0 to 95) reads outdoor dry bulb `-20.0 + 0.5·k`". Checked
against `psm3.rs` at `21d90588`: three-line header (`:57-107`), required
columns (`build_column_map`), record counts (`:445-462`), bounds
(`:148-200`).

**X-2 (B-8).** Fixed, recast. Kind: "dead-code removal with zero delta, in
the numeric block because it closes the `numeric_fixes` item
'deterministic fluid-solver summation'". Evidence re-read at `21d90588`
(`:334-351`, `:469`, `:746-767`, `:785-787`, `HashMap::new()` at `:315,
370`). Change: "The flow-resolution block returns `Option<(FluidNodeId,
f64, HashMap<FluidNodeId, f64>)>`", "`pump_node_id` (`:746-767`) is
deleted", "`pump_total` is the pump's resolved flow carried out of the
resolution, with no lookup and no fallback". Acceptance: existing tests
plus "`git grep -n -F 'resolved.values().sum()' crates` prints nothing".
The decision item is "met by construction; the commit body says so". New
commit subject (schedule change 3).

**X-3 (B-14 goldens).** Fixed. "no golden home carries a battery, an EV or
a generator (`consumer_shape_900s`, `fleet_mixed_offset`,
`fleet_same_offset_300s` and `resstock_2025_fleet` carry PV)". Verification
now lists every golden with the renamed fields its homes reach (checked:
`bldg0176227` PV 7000 W and central AC, `bldg0352714` PV 11000 W,
`bldg0000002` room AC, `bldg0000004` heat pump, cz2a central AC, cz4a heat
pump). The guard is meaningful because it is per value, not per golden:
`resolved_models_are_unit_invariant` "serialises to the JSON captured for
that home in this entry's first commit ... A missed or doubled conversion
(a factor of 1000 or 3.6e6) fails it", and "A golden may then move only
where a value that entered in kW or kWh now passes through a conversion
pair that is not exact in f64 ... Expected: every golden compares bitwise.
The entry records, per golden, `identical` or the moved columns with the
value and conversion pair that moved them; a golden that moves without
such a value ... stops the entry."

**X-4 (B-14 census test).** Fixed. `equipment_config_fields_are_si` is "a
`crates/hares-io` test over the config types themselves, so it can pass:
it reads no source text and does not see `BatterySpec`, `VehicleSpec` or
the `EquivalentBatteryModel` state, which keep their units by the scope
above"; it serialises a fully populated instance of every config type
(each `bms_mode` and `charging_strategy` variant once) and walks every key
path, and also checks every class of `input_schema().overrides`. The raw
keys have their own grep (`raw_equipment_keys_are_si`) over the constants
that remain after the deletions.

## Majors: consumer

**X-5 (CN-13 census).** Fixed. Evidence lists every import found by a full
`git grep -n -i -E 'ochre[_-]next'` at `5c651bc` (`paths.py`, `stock.py`,
`ami2/cvr_actuation.py:126-128`, `ami2/dr_actuation.py:77, 174`,
`ami2/tariff.py:71, 82`, `forward_model.py:260-262`, the three `hares_*`
tools, `suite_ami2.py:799, 850, 1737, 1956` (`:1696, 1915` at `c71c542`),
`tools/provision_data.py:65-66`) and every text hit (`coupling.py:373`,
`der_evidence.py:46`, `run_calibration.py:195`, `suite_scale.py:335`,
`AGENTS.md`, `STRUCTURE.md`, `CONTRIBUTING.md`, and the current-tense
`docs/GridLABD_Install.md:189`, `Code_and_Verification.md:600, 652`). Change:
"Every `import ochre_next` imports `hares` ... the `forward_model` fallback
to `import HARES` goes. `CACHE` (`paths.py:116-117`)". Dated records are
named and left as records. The census covers the current-tense docs and
adds `git grep -n -E 'import HARES\b'`.

**X-6 (CN-1 scratch).** Fixed. Two lists: engine-output scratch (adds
`truth.py:155,228`, which the finding did not name, and every site it did)
and fit-level scratch. Change: engine-output `scratch` goes from `Runner`,
`worker_runner`, `_Job`, `run_specs`, `parallel.scaling` and
`reactive._run`; "The fit-level argument stays, because it names a real
disk need ... and is renamed `slot_dir` ... so that after this commit
`scratch` names nothing and the census below can come back empty."

**X-7 (CN-1 presets).** Fixed. "The global `E.OUTPUT_VERBOSITY` is
deleted. `engine.py` gains the named presets the callers use, each a
report selection: `VERBOSITY_0`, `VERBOSITY_2`, `VERBOSITY_5` and
`VERBOSITY_6` ... Every site that set and restored the global (listed in
the evidence) passes its preset as the explicit selection argument". The
evidence lists all 14 sites; test `presets_are_the_engine_verbosity_selections`.

**X-8 (CN-1 schema.py).** Fixed. "`Prober` builds its probe dwellings
through `config=SimulationConfig(...)`, `defaults=paths.DEFAULTS` and the
`VERBOSITY_0` selection ... its accepted-field list per class is read from
`hares.input_schema()["overrides"]` ... `known_kwargs()` (`:249-251`)
returns the published surface". Test `prober_reads_the_published_field_lists`.

**X-9 (CN-1 KWARG_RULES).** Fixed. "`KWARG_RULES` (`coverage.py:111-127`)
is rewritten over that surface, every keyword classified once", with the
classification of `config`, `defaults`, `reports`, `metrics`,
`health_policy`, `duration`, `time_res` and the removed keys
(`REMOVED`); the whole of `suite_coverage.py:141-153` and `:156-170` is
migrated.

**X-10 (CN-1 direct construction).** Fixed. Every site the finding names
is in the evidence, plus the AMI and test sites a grep found, and the
Change: "Every direct construction site in the evidence is migrated to the
construction surface `Runner.build_kwargs` uses".

**X-11 (CN-1 `_engine_metrics`).** Fixed. "rebuilt from
`RunResult.metrics` ... a missing field raises naming it (no `getattr`
default, no `except`)"; "`dwelling_fatal` ... is read from
`Dwelling.dwelling_fatal` (the binding's poisoned flag,
`crates/hares-python/src/py_dwelling.rs:996-999` at `21d90588`)". Readers
added (including `ami2/services.py:40-49`), and `_equipment_metadata`'s
swallow (`:296`) goes too. Test `engine_metrics_raise_on_a_missing_field`.

**X-12 (CN-1 billing).** Fixed. "`bill()` drops `finalize_billing()`.
`simulate()` returns after `finish()`, which closes the final billing
period ... `RunResult` carries no billing, so the bill is read from
`Dwelling.billing_summaries()` after `simulate()`". Test
`bill_reads_every_period_without_finalize_billing`.

**X-13 (CN-1 census).** Fixed. Each untouched hit is now a stale-text edit
(`adapter.py:23`, `engine.py:11-12, 128, 151`, `parallel.py:68`,
`tariff.py:79-80`, `suite_engine.py:133`), the `suite_scale.py:197` fixture
"is renamed `stale_output.bin`", and the pattern gains `defaults_path`,
`output_verbosity`, `OUTPUT_VERBOSITY`, `ADAPTER_VERBOSITY`, `\bscratch=`,
`KNOWN_KWARGS`, `\.metrics\(\)` and `RESAMPLE_OVERRIDES`. `duration_s=`
and `time_res_s=` get their own grep excluding four files that use those
names for the consumer's own `Grid` and AMI series types (checked: those
are the only other hits at `c71c542`). `DwellingConfig\(` is not a pattern
because legitimate constructions remain; their old keywords are all in the
patterns.

**X-14 (gate, CN-15).** Fixed. Gate: "`contract.txt` holds the line
`[hares-contract] PASS` ... A `SKIP` or `FAIL` line fails the gate";
every "exits 0" verification now reads the PASS line. CN-17: "`_repo()`
returns `paths.HARES_REPO` when `paths.have_repo()` holds ...
`DEFAULT_REPO` is deleted". CN-15: "`export HARES_REPO="$PWD/HARES"`" and
`contract.txt` captured. Case name: "`hares_contract.contract_matches_HARES_source_or_skips`".

**X-15 (CN-3 sites).** Fixed. Evidence and Change name `coverage.py:137,
281-296, 355-359`, `registry.py:37, 66, 661-662, 692-693`,
`tools/bench_calibration.py:62-78`, `build_bundle.py:46` ("`calibration/defaults.py`
itself with its `build_bundle.py:46` registration") and
`suite_coverage.py:111-125`, where "the reserved `envelope` key resolves to
its schema class, whose fields are `opaque_r_scale` and
`thermal_mass_scale`".

**X-16 (CN-12 trial group).** Fixed. "step 2 reads `uv sync --locked
--extra engine --group trial --no-install-package ochre-next` ... step 4's
dry run uses the same flags ... `uv run --no-sync ruff --version` is
recorded before and after the group is installed and is the same". The
`--no-sync` mechanic was re-measured (header). The package keeps the name
`ochre-next` in these flags until CN-15, as the mechanism states.

**X-17 (CN-17 placement).** Fixed in `08`; schedule and log edits handed on.
Order step 1: "CN-17 ... did not land there: the engine rows BR-11
(`8b09ac4a`), PR-1 (`d7f0e381`) and PR-2 (`21d90588`) were committed while
the consumer stayed at `5c651bc`." Drift assignment: "CN-17 for every
engine row committed when CN-17's gate runs: at its required position,
rows 1 to 13a, 14 and 15 ... CN-13 for every engine row committed after
CN-17's gated engine and up to row 26 (rows 13c to 13g included)". CN-17
Dependencies: "Required position: directly after ... PR-2 (`21d90588`,
row 15), before row 16 (PR-3) is committed, gated on `21d90588`. If row 16
or a later row is committed before this entry lands, it lands directly
after that row, gated on it, and its census range extends to it." CN-17's
Change requires the PR-1 and PR-2 B0 checks to be recorded before its
gate.

## Majors: numeric fixes

**X-18 (B-1c Feb 29 fixture).** Fixed. "each a copy of the Feb 28 record of
the same hour with year 1996, month 2, day 29 and dry bulb 50.0 C (the cz4a
Feb 28 records are year 1994, and `epw.rs:199-203` rejects 1994-02-29)".

**X-19 (B-1c restamp).** Fixed. "the record's interval start (its stamp
less 1 h) is moved to the same month, day and hour of 2020, and the record
is stamped at that start plus 1 h. Feb 28 hour 24 ... becomes 2020-02-29
00:00 and Dec 31 hour 24 becomes 2021-01-01 00:00; the file keeps 8760
records". Checked against the file: 2018-01-01 01:00 to 2019-01-01 00:00.

**X-20 (B-1c partial year).** Fixed by the error option. "A file whose data
period is not a complete year is a construction error naming the file's
data period and the inputs that need a complete year". Reason recorded in
the entry: EnergyPlus's weather-file water-mains correlation reads twelve
complete months and otherwise falls back to a fixed 10.0 C
(`WeatherManager.cc:8742-8800`, now in the binding section), HARES always
derives mains from the weather file (`environment.rs:889-932`, at
environment construction, for every dwelling; every parity and
`ochre_samples/base*.xml` building also has a water heater), and a substituted value is
forbidden (`dl-i08-criteria`). The EPW reader keeps DATA PERIODS and names
it; "Parsed weather keeps each record's dated stamp" for `Actual`. Test
`partial_year_file_is_an_error`.

**X-21 (B-1c Feb 29 values).** Fixed. "the steps ending Feb 29 01:00 to
Mar 1 00:00 ... read 50.0 C; the steps ending Feb 29 00:15, 00:30 and 00:45
read `f28h24 * (1 - w) + 50.0 * w` with `w` 0.25, 0.5 and 0.75". Day types
stay on the calendar (`leap_day_day_types`).

**X-22 (B-1b first hour).** Fixed. B-1b: "before the half hour of hour 1,
the previous-hour value is the previous day's hour 24, except in the run's
first hour and in the first hour of every warm-up day, where it is the
record `first_hour_interpolation` names". Test
`solar_first_hour_uses_hour24_of_the_first_day` with its `Hour1` case; B-2
gains `warmup_days_repeat_the_first_hour_rule`.

**X-23 (B-2 NaN test).** Fixed. "a `#[cfg(test)]` hook overwrites the
recorded day-1 daily maximum temperature of the living zone with NaN while
the zone's thermal state stays finite ... the day-2 comparison fails the
run with `NanDetected` naming that zone and day 2".

**X-24 (B-6 producers).** Fixed. "the producers write `None` for a
quantity that is undefined", with COP (`heater.rs:1583, 2663`;
`air_conditioner.rs:1409, 1943`) and SHR (init insert at
`hvac/ac_config.rs:32` removed; written "only when the step's sensible
cooling is above 0"). Test: "the `ASHP Cooler`'s sensible heat ratio is
null in every step in which it is not cooling ... and the `ASHP Heater`'s
COP is null in every Off step", after asserting an Off step and a heating
step exist.

**X-25 (B-5).** Fixed. The PV bullet is "a latent defect, not an observed
one" with `assign_instance_names` cited; the PV test "is written first and
run at the parent commit ... if it passes there (expected, see Problem), it
is a regression guard". The five classifiers are in Problem, Evidence (with
call sites at `21d90588`), Change and the `no_name_classifier_remains`
grep. Inputs: "the verbosity-8 selection ... and a schedule the test
writes".

**X-26 (B-4).** Fixed. "`BillingPeriodSummary` records no per-step
membership ... The January summary's `total_import_kwh` equals, on bits,
the step-order sum ... over the 168 steps labelled Jan 25 00:00 to Jan 31
23:00, and the February summary's equals the same sum over the 120 steps
labelled Feb 1 00:00 to Feb 5 23:00."

**X-27 (B-10).** Fixed. Evidence: "A second relative fallback:
`crates/hares-core/src/dwelling/mod.rs:2104-2110` at `ebdb1a17`
(`:2244-2249` at `21d90588`)" (the finding's `:2151` is the wrong line at
the head; re-located) and the three downgrades in `DefaultsStore::load`.
Change: "Any load failure is a construction error naming the path: ... an
envelope LUT that fails to load (the `fallback_r` downgrade goes), and
every sub-table the loader reads ... `HARES_STRICT_DEFAULTS` and
`load_with_strict_defaults` are removed". Two tests added.

**X-28 (B-14 census).** Fixed. Table rows added for `delta_kw_per_s`,
`event_power_kw_series` (with its writers `schedule_resolve.rs:1235,
1264`), `power_profile_max_kw`'s writer `:1315`, `surplus_deadband_kw`,
`min_charge_rate_kw`, `max_export_kw` (with `py_enums.rs` and the
consumer's `der_inject.py:157-180`), and `zone_capacitance_kwh_per_k`
renamed `zone_capacitance_j_per_k` ("no `REMOVED_KEYS` row: no input surface
sets it"). The EV HPXML keys are deleted with the dead raw path (X-52).
The census is the serialisation walk of X-4 rather than a widened grep,
because the widened grep matches telemetry keys, control signals and
locals that are out of scope (it prints over 1,500 lines at `21d90588`).

**X-29 (actor keys).** Fixed by the out-of-scope option: "Actor
configuration parameters (`crates/hares-core/src/actor_registry.rs`: ...)
the decision names equipment config fields, and these configure the
control layer, whose quantities ... the same decision leaves in kW;
migrating the parameters alone would split the control layer between
units. Where an actor parameter fills an equipment-config payload (`:486`
builds `BmsMode::SelfConsumption`), the fill converts with `uom`". Listed as
an operator question.

**X-30 (EV golden).** Fixed. The sentence is gone; the first commit
captures `der_pv_battery_ev_900s` from `tests/fixtures/parity/cz6b_pv_battery_ev/`
("whose EV is a `PlugLoad` of type 'electric vehicle charging' built into
`EvConfig` by `resolve_loads.rs:867-918`"), with the battery's declared
control.

**X-31 (removed-key surfaces).** Fixed. "`SyntheticEventLoadConfig` gains
`#[serde(deny_unknown_fields)]` and its keys are checked against
`REMOVED_KEYS` first"; the HPXML extension element "is renamed
`active_power_w`; an HPXML document carrying the old `active_power_kw`
extension element fails parsing"; the merge walks "every key of the
override payload, the `"all"`/`"*"` payload included, ... for the typed
and the raw path alike". Key format: "`<ConfigType>` is the config type,
not the class name an override is keyed by ... maps it to its config type
through the registry entry that builds the class"; arrays as `[]`;
JSON-string payloads parsed and pathed under the variant; "`removed_key`
matches `<n>` against a run of ASCII digits". Three tests cover them.

**X-32 (`uom`).** Fixed. "`BatterySpec::capacity_j()`, `max_charge_w()` and
`max_discharge_w()`, and `VehicleSpec::capacity_j()` and
`fuel_economy_j_per_m()` ... `PyBattery::from_spec` and `PyEv::from_spec`
call them"; "`hares-core` ... and `hares-python` ... gain `uom = {
workspace = true }`, added with `cargo add uom -p hares-core -p
hares-python`", each with the boundary that needs it (the BMS actor's
fill; the EV builder defaults at `py_dwelling.rs:539-542`).

**X-33 (albedo).** Fixed. Binding section: "Albedo: EnergyPlus does not
carry the file's albedo into its per-timestep weather; the value is read
only into `wvarsMissing` (`:2936-2938, 3031`) and the loop assigns no
albedo, so holding the hour's record is a HARES choice." B-1a's field
list says the same. Checked: albedo is never assigned into the per-step
arrays in `WeatherManager.cc`.

**X-34 (field coverage).** Fixed. Binding section: HARES has no total sky
cover field; "EnergyPlus keeps no global horizontal irradiance in its
per-timestep weather (`WeatherVars`, `WeatherManager.hh:713-732`); HARES
applies the same weights to GHI, which is a HARES extension". B-1a's
`docs/PHYSICS_DECISIONS.md` bullet and B-1b state both.

**X-35 (B-1a test inputs).** Fixed. The acceptance preamble states the
UTC-7 cz4a offset and the observable ("each fixture test compares the
run's outdoor dry-bulb column with B-1a's interpolation of the named
records"); each test now names its start with offset, duration and step,
including "from 2020-02-27T00:00:00-08:00 for 3 d at 3600 s".

## Minors

**X-36.** Fixed: `tex:721` for the first-hour quote with the code at
`:3050-3056`; `Energy+.idd.in:1480-1484`; `:3074-3091`; `:3061-3065`;
`:941-991`; `:904-926` with the fatal at `:990`; `:872` and the rollover at
`:1896-1900`.

**X-37.** Fixed: ZTAV (`:3237-3241`), `airSysHeatRate`/`airSysCoolRate`
(`:3243-3247`), "`ShowSevereError` per unconverged zone (`:3387-3424`)",
"with a warning (`:686-697`)", and "The IDD gives this field `\units W`
(`Energy+.idd.in:542`), but the code uses it as a fraction".

**X-38.** Fixed: "Step `ts` of N is before the half hour when `2·ts < N`,
at it when `2·ts == N` ... and after it when `2·ts > N`", in the binding
section and B-1b.

**X-39.** Fixed: ground temperature "held at the value of the hour its step
end lies in"; "an hourly PSM3 record stamped `t` is the record of the hour
ending `t + 1 h`"; "Hourly files at a step longer than one hour keep
today's interval mean".

**X-40.** Fixed: `environment.rs:532-536`; `weather.rs:568-713` with
precipitation at `:708` and albedo at `:709-713`; "at 900 s the first 30
minutes read a PCHIP blend of the Dec 31 records".

**X-41.** Fixed: the adjustments at `:448-450` and `:869` and the tests at
`:1975-2040, 2090-2130` are cited and replaced; `weather.rs:294` /
`epw.rs:167`; the non-leap Feb 29 skip (`:2817`) with test
`non_leap_run_skips_the_files_feb29`; "a `DwellingBlueprint::validate()`
error".

**X-42.** Fixed: `py_dwelling.rs:2474-2480` at `872cf289`; B-2 evidence "at
`ebdb1a17`" with `:2725`, `:4992-5073`, `:5039-5043`, `:5066-5071`,
`:2780-2787` (each re-read).

**X-43.** Fixed: "the key is per home today (`[[home]]
initialization_duration_s`, `tools/frame_golden/src/manifest.rs:56`) ...
A value of `0` becomes `[simulation] warmup = "disabled"`; any positive
value ... becomes the default", covering every manifest, the BR-14 and
FL-1 ones included; the synthetic key is named as `[simulation]` until
PY-8 moves it.

**X-44.** Fixed: "its weather is the same Denver TMY3 file as cz4a's, so D
adds a heating system type, not a colder climate".

**X-45.** Fixed: "the manifests run through the Rust adapter, which B-3
does not change, so no golden changes".

**X-46.** Fixed in CN-4: "`consumer_shape_900s` declares `warmup =
"disabled"` ... from `09-numeric-fixes.md` B-3 on the consumer ... warms up
by default; so the deltas B-4 to B-14 record on that golden are measured on
a cold-start run the consumer no longer makes, and B1 is the only
measurement of the consumer's own shape". No twin golden: it would add a
fixture whose only reader is this note.

**X-47.** Fixed: "the same cz4a inputs with `initialization_duration=0`
passed to `Dwelling.from_hpxml` and, separately, to `DwellingConfig` each
raise the removed-key error naming `SimulationConfig.warmup`".

**X-48.** Fixed: `columns.rs:1089-1103`; "`extract_unit_from_name` ... is
already removed by `03-output-reporting.md` OR-1"; "total gas" dropped
from the summed cells; B-11 "the mode column's unit is `-`, so it takes
`Mean` through the `"-"` entry".

**X-49.** Fixed: `fleet.rs:193`, `:836-849` at `ebdb1a17`; the parquet's
cache path, size and full sha256 (`83a288b3...e06`, computed here) and
"the three rows in `bldg_id` order"; `parse_weather_station_fips` "returns
`Result`" with test `resstock_unreadable_weather_station_is_an_error`;
B-9's "file's own year is the calendar year whose hours its records cover";
"the one error names every unresolved building with its county".

**X-50.** Fixed: `fleet.rs:1174, 1191` moved to tests with the three
missing test callers; each test names its inputs; "unreadable" uses "a
regular file ... which no user, root included, can list as a directory",
so no root skip is needed; PY-7 added to Dependencies.

**X-51.** Fixed: the film-sum test rounds the IP value down and checks its
SI conversion; the material-layer half is "a unit test over the boundary
conversion with a boundary the test builds"; `conversions.rs:240-291`,
clamp `:149`, and the R >= 17.6 m2K/W `Minimal` row (`envelope_lut.rs:571-576`,
checked at `8b09ac4a`).

**X-52.** Fixed: `conversions.rs:773-841`; "`base-pv.xml`'s two PV systems
(`MaxPowerOutput` 4000 W and 1500 W)"; both DER goldens use a
self-consumption `bms_mode`; the EV measurement test is named with
"2018-01-01T00:00:00-10:00 for 45 d" (checked in the test source);
battery and generator `#[cfg(test)]` constants and the EV raw path are
deleted; the RT-6 mapping is "PV `nameplate_w` from `PvConfig.capacity_w`,
battery `energy_capacity_j` from `BatteryConfig.capacity_j` and
`power_capacity_w` from `BatteryConfig.max_charge_w`, EV ..."; the "37
lines" count is replaced by the named Python files, with the PySAM
adapters and control-signal keys out of scope with reasons; out-of-scope
reasons for `heating_capacity_kbtu_h`, the therms-per-hour gas keys and
the PV-sizing API. `register_pv_roof_shading`: handed to `11`'s owner.

**X-53.** Fixed, every row: CN-0 `:467-501`, `:573-583`, `county` added;
CN-11 `:209-225`; CN-7 `:272-298`, `:406-407` with the helper at
`:411-472`; CN-1 `frame_hash` `:249-264`; CN-6, CN-10, CN-11 `fit.py:215-222`;
CN-10 `inject.py:198`; CN-1 `suite_der_inject.py:178-198` and `:166-175`;
CN-3 `suite_inject.py:353-398`; CN-16 `:1441-1442` and `:1481`; CN-1
`adapter.py:342-353`; CN-1 `registry.py:221-224`; CN-14 `suite_network.py:3006, 3022`.
CN-10's `_get_pool` citation was also corrected to `parallel.py:37-47`.

**X-54.** Fixed: "four `suite_coverage` cases read
`results/engine_schema.json` ... and all eight cases of the suite skip ...
through the suite-level `_needs_schema` guard
(`calibration/tests/suite_coverage.py:23-31`)".

**X-55.** Fixed: "`compare` has no `--declared` option today
(`tools/capture_de_trace.py:224-229` at `5c651bc`); this entry adds it".

**X-56.** Fixed by reusing one pool: "`Fitter` computes its pool size once,
`wave_balanced_workers(P, self.workers)` ... and passes it to every
`run_specs` call of the fit", with test `a_fit_uses_one_pool_size`.

**X-57.** Fixed: `registry.py:40, 70, 305-307`, `inject.py:156-158`, the
import at `engine.py:32` and `grid.py:168-173`.

## `verify-L3-e.md` findings on these files

**8 (B-11 half).** Fixed: B-11's verification states that no golden
changes ("the fleet goldens ... report the verbosity-2 selection ... which
holds no `Enum` variable (the mode columns start at verbosity 3)") and
"The unit tests above carry the delta"; Measurement records the unchanged
goldens. FL-5's test exclusion is `04`'s.

**12 (non-SI serialised types).** Rejected for B-14, with its reason, and
put to the operator. `dl-i08-architecture.si_units` scopes the migration
to equipment config fields; `BillingSnapshot`, `MetricsAccumulators` and
`StepResult.net_electric_power_kw` are tariff, metric and per-step
reporting quantities. B-14's "Not in scope" now names them: "Whether they
are boundary units or running state that must be SI before
`03-output-reporting.md` OR-10 freezes them into checkpoints is an
operator decision this entry does not take." `plan.md` §1 ("running state
and every type this plan defines are SI") conflicts with the kWh fields
`plan.md` defines; that conflict is the plan owner's (below).

**14 (`08` half).** Fixed: order step 2 now reads "The consumer at CN-16
(`aec2e3f`) ran engine rows 1 to 12; the consumer at BR-6d (`5c651bc` ...)
runs the engine rows committed before CN-17 lands ...; the consumer at
CN-17 runs every later engine row before `05-python-surface.md` PY-7". The
labels are the integrator's (schedule change 1).

**16 (CN-6).** Fixed: "reports a difference of exactly `s` per fit (2 when
both standalone probes were submitted)".

**17 (CN-3).** Fixed: CN-3's Dependencies add "`04-fleet.md` FL-5
(preamble order step 5)" and IN-3.

## Schedule changes (for the integrator; `00-schedule.md` and `decision-log.md` are not edited here)

1. Executed order. Committed: BR-11 (row 13a, `8b09ac4a`), M-3 (13b,
   measurement), PR-1 (14, `d7f0e381`), PR-2 (15, `21d90588`). Not
   committed: CN-17 (13-1) and rows 13c to 13g (BR-10, BR-12 to BR-15),
   although row 14 and 15 are. Record this in the schedule and in
   `dl-i08-execution.progress`.
2. CN-17 moves to row 15-1: directly after PR-2, before row 16 (PR-3,
   in progress in the working tree), gated on `21d90588`. If row 16 or
   later commits first, CN-17 sits directly after the last committed row
   and its drift range extends to it. Its row title and commit subject:
   `fix: the contract check reads the engine tree every gate resolves, and a capture compare ignores the provenance header`.
3. Consumer-check labels: rows 13a, 14 and 15 ran on the consumer at
   BR-6d (`5c651bc`); label them "B0 (BR-6d)" (or extend the legend so
   "B0 (CN-16)" covers the BR-6d consumer), and record the B0 checks of
   PR-1 and PR-2 in `02-measurement.md` before row 16. Rows 13c to 13g,
   when they run after CN-17, and rows 16 to 25 are "B0 (CN-17)". The
   legend's parenthetical about rows 13a to 13g running "on the consumer
   after CN-17" is false as executed and goes.
4. Row 90 (B-8): title "remove the unreachable hash-order fluid-flow
   fallback"; commit `refactor: carry the pump's resolved flow out of the fluid-loop flow resolution`;
   consumer check "golden" (zero delta).
5. Row 94c (B-14): the first commit is
   `test: capture golden PV, battery and EV dwellings and the resolved models of every golden`,
   adding `der_pv_battery_ev_900s` and `tests/fixtures/golden/resolved/`.

## Items for other owners

1. `02-measurement.md` (owner of the log): record the B0 checks of PR-1
   (`d7f0e381`) and PR-2 (`21d90588`) on the consumer at `5c651bc`; in the
   "Changed by later entries" list, B-14 adds `der_pv_battery_ev_900s` and
   the resolved-model JSON files, and B-14's `der_pv_battery_900s` override
   is self-consumption, not `Manual`.
2. `decision-log.md`: record the executed order (schedule change 1) in
   `dl-i08-execution.progress`; the `next:` line names CN-17 first.
3. `plan.md` §2.7 `REMOVED_KEYS`: the `EquipmentConfig` scope's key format
   B-14 defines (`<ConfigType>.<key path>`, `[]` for array elements, the
   variant path inside `bms_mode` and `charging_strategy`, `<n>` matching a
   digit run in `removed_key`).
4. `plan.md` §1 Units and §2.x: the statement that every type the plan
   defines is SI conflicts with `BillingSnapshot`, `MetricsAccumulators`
   and `StepResult.net_electric_power_kw` in kW and kWh; resolve after the
   operator answers question 2 below.
5. `11-other-defects.md`: a new found-defect entry for
   `register_pv_roof_shading` (`crates/hares-core/src/dwelling/mod.rs:1420-1435`
   at `8b09ac4a`), which never fires for HPXML PV (`verify-L3-d.md` X-52).
6. `05-python-surface.md` PY-9: state that `Dwelling.billing_summaries()`
   and the `Dwelling.dwelling_fatal` property stay (CN-1 reads both after
   PY-9 deletes `finalize_billing()` and `metrics()`).
7. `04-fleet.md` FL-5: its `fleet_cells_follow_declared_kinds` test
   excludes `Enum` variables (`verify-L3-e.md` 8, the FL-5 half).
8. `12-inputs.md` IN-3: B-14 renames the `active_power_kw` HPXML extension
   element in `HPXML_ELEMENTS` in its own commit; IN-4's per-step weather
   function is the one B-2's `warmup_days_repeat_the_first_hour_rule`
   calls.
9. `06-runtime-memory.md` RT-6: the read-back names' SI sources are fixed
   by B-14 (`power_capacity_w` from `BatteryConfig.max_charge_w`); RT-6's
   own conversion text should name the same fields.

## Operator questions

1. Control-layer units: actor configuration parameters (`actor_registry.rs`,
   kW, kWh, miles, mph), `ControlSignal` power setpoints and actor
   telemetry stay in kW under `dl-i08-architecture.si_units` as written.
   Migrate them to SI as their own entry, or record that they are boundary
   units?
2. Billing, metrics and per-step result units (`verify-L3-e.md` 12):
   boundary units in kW and kWh, or SI running state before OR-10 (row 48)
   freezes them into checkpoints?
3. The scheduled load's gas rates in therms per hour are outside
   `si_units` as worded (kW, kWh, kVA). Same question as 1.
