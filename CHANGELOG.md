# Changelog

User-visible changes to HARES, one section per release, newest first.
Each section itemizes what its release added to, changed in or removed
from the surfaces [VERSIONING.md](VERSIONING.md) names, with the
replacement named for everything taken away. The numeric log (what
moved, why, and the golden fixtures re-captured) stays in
[docs/python.md](docs/python.md) under "Numeric changes".

## Changes in this release

Every callable, class and keyword added to, changed in or removed from
`hares._hares` since `ebdb1a17`, each removed or renamed one with its
replacement.

- Changed: the `Battery`, `PV` and `EV` constructors take the SI names,
  keyword-only: `Battery(name, *, capacity_j, max_charge_w=None,
  max_discharge_w=None, ..., inverter_capacity_va=None, ...)`,
  `PV(name, *, capacity_w, tilt, azimuth, sam_lut_path=None,
  soiling=None)` and `EV(name, *, capacity_j=None,
  max_charging_power_w=None, ..., charger_capacity_va=None, ...)`; the
  `BmsMode.self_consumption` and `ChargingStrategy.solar_surplus` /
  `.v2g` payloads take `surplus_deadband_w`, `min_charge_rate_w` and
  `max_export_w`, keyword-only. A removed keyword names its replacement;
  the parent's positional forms raise `TypeError`.

- Changed: `batch_step` classes a step failure's fatality from the
  dwelling's own state (the flag crossing the batch boundary as data), not
  from the error message's text: a transient failure whose message merely
  carries the fatal sentinel's wording is no longer reported under
  `info["dwelling_fatal"]`, and a panic that poisons the dwelling's lock
  now is. The messages themselves are unchanged.

- Added: `defaults_dir()`: the packaged defaults directory, the
  `defaults` tree the installed package carries, resolved from the
  installed package's own location. `defaults=None` (the constructors'
  signature default) reads it: a dwelling built with no `defaults`
  keyword reads the packaged tree, an explicit `defaults` path
  overrides it, and nothing depends on the working directory. A missing
  or unreadable defaults tree at whatever path resolves is a
  construction error naming the file it missed. `ParsedDefaults`'s
  `base` is required, and the defaults loader's sub-tables (the HVAC
  multispeed CSV, the water-heating CSV, the envelope LUT's three CSVs)
  are required files: no warning degrades a store or a lookup any more.

- Added: `ReportSpec`, `Selection` (with `verbosity`, `named`, `pattern`,
  `diagnostics`), `Frequency` (with `timestep`), `Destination` (with
  `memory`, `parquet`, `csv`, `callback`), `RunResult`, `ReportResult`,
  `RunMetrics`, `RunHealth`, `WarmupOutcome`, `WarmupResiduals`, `Warning`,
  `Provenance`, `EquipmentProvenance`, `SiteLocationOverride`.
- Added: `HealthPolicy` (with `strict()`), `HaresHealthPolicyError`, and
  the `SimulationConfig.health_policy` keyword.
- Added: `WarmupConfig` (the EnergyPlus warm-up criteria over the Rust
  table's defaults) and the `SimulationConfig.warmup` keyword: an omitted
  `warmup` converges with the Rust defaults, `warmup=None` disables, a
  `WarmupConfig` sets the criteria; the removed `initialization_duration`
  keyword names `SimulationConfig.warmup`.
- Added: `Dwelling.finish()`, `Dwelling.health()`,
  `Dwelling.initialize()`, `Dwelling.attach_callback()`,
  `Dwelling.config()`, `Dwelling.dwelling_fatal`,
  `Dwelling.set_solar_override()` and the solar-override and LUT-injection
  methods, `DwellingBlueprint` and its constructors, `SimulationPlan`,
  `SimulationConfig.diagnostics`, `Fleet.from_resstock`,
  `SteppableFleet` and its methods.
- Added: `FleetReport` (a `Selection`, a `Frequency` and the fleet's
  `HealthPolicy`), `FleetResult` (with `aggregate`, `covered_weight`,
  `homes`, `coverage`, `n_succeeded`, `n_failed`,
  `peak_live_home_results`), `Coverage` (with `weight_total`,
  `weight_ok`, `weight_flagged`, `weight_failed`), and the
  `Frequency` constructors `minutes`, `hourly`, `daily`, `monthly`,
  `annual` and `run_period`.
- Added: `ReportResult.arrow()` and `FleetTable` (a fleet result's
  `aggregate`, `covered_weight` and `homes` are now `FleetTable`
  objects): each exports its table through the Arrow PyCapsule interface,
  `arrow()` returning the stream object every Arrow consumer (polars,
  pyarrow, pandas through pyarrow) imports zero-copy, the underlying
  buffers shared; `table()` reads it as the polars frame as before, and
  both take `rechunk: bool = True` (a report's batches concatenated once
  on the first rechunked export, then shared; a fleet table is one
  batch).
- Removed: `FleetResults` with `per_dwelling_metrics`,
  `aggregate_timeseries` and `failures`: read `FleetResult.homes` (a
  failed home is a `homes` row with status `failed` and its reason),
  `FleetResult.aggregate` and `FleetResult.n_failed`.
- Renamed: `AggregationResolution` (with the `resolution=` keyword) to
  the `FleetReport`'s `frequency` (`Frequency.minutes(15)` is the old
  `FifteenMin`, `Frequency.hourly` the old `Hourly`).
- Removed: `Dwelling.results()`: read the run's reports off the
  `RunResult` (`simulate()`'s or `finish()`'s return,
  `result.reports[i].table()`).
- Removed: `Dwelling.metrics()`: read `result.metrics` off the
  `RunResult`.
- Removed: `Dwelling.finalize_billing()`: `finish()` closes the final
  billing period; `billing_summaries()` reads the summaries after it.
- Removed: `Dwelling.set_tariff()`: use `set_electric_tariff()` (and
  `set_gas_tariff()`).
- Renamed: `SimulationMetrics` to `RunMetrics` (the same getters, read
  off the `RunResult`).
- Removed keywords: `write_output`, `output_format`, `output_path`,
  `output_verbosity`, `retain_batches`, `rotation` (pass
  `reports=[ReportSpec(...)]`); `latitude`, `longitude`, `elevation_m`,
  `utc_offset_h` (pass `site_location=SiteLocationOverride(...)`); the
  `time_res_s`/`duration_s` forms became `time_res`/`duration`.
- Moved to the shared `_hares_types` stub module (still exported, typed
  there once for both modules): `StepResult`, `FleetStepEntry`,
  `SteppableBuildError`, `SteppableStepResult`.
- Added module functions: `batch_step`, `field_bounds`,
  `field_bounds_table`, `clip_action` (the RL fast paths), and the
  weather parsers `parse_weather`, `parse_epw`, `parse_psm3`,
  `parse_tmy3`, `parse_resstock_csv`.
- Added: the equipment classes `ElectricBoiler`, `ElectricFurnace`,
  `GasBoiler`, `IndirectTank`, `TanklessWaterHeater`; the exceptions
  `HaresConfigError`, `HaresEquipmentError`, `HaresSimulationError`;
  the callback info class `BatchInfo` and the callback error class
  `HaresCallbackError` (a `HaresSimulationError` subclass; an exception
  from an attached callable is chained as its `__cause__`). The
  `Dwelling.attach_callback` callable receives each batch as the
  zero-copy Arrow batch (the Arrow PyCapsule array interface over the
  batch's own buffers), which every Arrow consumer imports directly.
- Added: `Alignment` (with `instant`, `local_clock`) and the
  `alignment=` keyword on `Fleet.from_buildings` and
  `Fleet.from_resstock`: the fleet's declared time alignment.
- Added: the `master_seed=None` keyword on `Fleet.from_resstock`: the
  fleet's master random seed, each home's event-load streams deriving
  from it and the home's own building id; `None` keeps the documented
  default seed of 0, and the build report names the seed the fleet runs
  under. The seed does not reach the DR compliance driver's draw, the
  EV driver's key material or a noisy TimeWindows source's keystream
  key: each keys on its own configured seed or identity, and the fleet
  path constructs none of them today. If the fleet path gains actor or
  noisy-source construction, the master seed must thread into their
  unspecified-seed derivations the same way.
- Added: `ObservationPlan`, `Dwelling.observation_plan(fields)` and
  `Dwelling.observe_into(plan, out)`: the frozen per-step observation
  layout the RL environments read through. The plan is built once per
  episode against a built dwelling (an unknown field fails there, naming
  it); every step fills a caller-owned buffer in place through its
  buffer protocol, allocation-free, the values and NaN handling
  `to_observation_vec` reports. `batch_step` takes the plans and the
  batch's own `(n_envs, n_fields)` buffer instead of the field list, and
  its rows no longer carry `obs`: the buffer is the observation.
- Added: `DwellingBlueprint.validate()` and
  `DwellingBlueprint.catalogue()`: the blueprint's assembly alone, with
  no warm-up and no step. `validate()` reads the resolved model back as
  a dict: `site` (each resolved field's value beside its `*_source`
  label), `clock` (`start` as the first step's local standard time,
  `utc_offset_s`, `time_res_s` and the simulated step count `steps`) and
  `boundaries` (each boundary's `id`, `kind`, `area_m2`,
  `r_effective_m2k_w`, `films_m2k_w`, `c_j_k` and its construction
  `layers`, outside to inside, each layer's `r_m2k_w` and `c_j_m2k`).
  An infeasible input raises here with the same message `build()` fails
  with, and a fraction of its cost: the build's warm-up never runs.
  `catalogue()` returns the `Catalogue` the built dwelling's
  `catalogue()` returns, without building it. The blueprint is not
  consumed: `build()` still works after either call.
- Added: `Catalogue` and `Variable` (every variable's `name`, `unit`,
  `kind`, `aggregation`, `sampling`, `nullable`, `source`, `labels`,
  `across_homes` and `description` readable, the `digest()`, the
  iteration, `to_json()`/`from_json()`), `Dwelling.catalogue()` and
  `RunResult.catalogue`, and `metrics_from_table(table, catalogue,
  time_res, setpoint_deadband_c=None)`: any `__arrow_c_stream__`
  results table scored through the catalogue with the run's own
  accumulator (`time_res` in whole seconds; a reduced or non-Timestep
  table raises, naming the mismatch).
- Added: `BuildInfo` (the build's compile-time identity: `version`,
  `commit`, `dirty`, `profile`, `features`, `rustc`), `build_info()`
  (the extension build's own record), `hares.__version__` (the build
  identity's version, the pyproject version), and `Provenance.build` and
  `Provenance.catalogue_digest` (the catalogue digest as a 64 char hex
  string): every run's provenance now records what built it and the
  catalogue the run's reports are typed by.
- Added: `input_schema()`: the machine-readable schema of every input the
  engine reads, as a dict of its sections (`config_fields`,
  `dwelling_fields`, `settings_fields`, `override_classes`,
  `removed_keys`, `schedule_columns`, `unused_schedule_columns`,
  `defaults_files`, `hpxml_elements`, `rejected_hpxml_elements`), each
  field with its type, unit, default, bounds and description; every
  section derives from the engine's own typed definitions, so a consumer
  that classifies its fields reads the schema instead of HARES source.
- Added: `ParsedDefaults.read_files()`: the defaults files the load read,
  in load order (the read list the schema's `defaults_files` publishes);
  and the `ParsedWeather` header getters `latitude`, `longitude`,
  `elevation_m` and `timezone_offset_h` (the header's own values, the
  site resolution's fallbacks).
- Added: `resample_weather(weather, site, config)`: the engine's own
  per-step weather channels as a polars frame, one row per step of the
  configured horizon — the `Time` column and `Outdoor Dry Bulb (C)`,
  `Outdoor Humidity Ratio (kg/kg)`, `Outdoor Pressure (kPa)`,
  `Wind Speed (m/s)`, `Sky Temperature (C)`, `GHI (W/m2)`, `DNI (W/m2)`,
  `DHI (W/m2)`, `Temperature - Ground (C)` and `Hot Water Mains
  Temperature (C)`. The evaluation is the environment update's own, so
  the channels match the run's internal series bitwise. `weather` is a
  path, a `(text, format)` pair or a `ParsedWeather`; `site` is the
  resolved site dict (a missing coordinate a `KeyError` naming it);
  `config` the run's `SimulationConfig`. The wind direction is not one
  of the channels: a direction has no mean over a period, so it is not a
  catalogue variable.
- Added: `Provenance.inputs`: what the run read, recorded when each input
  was parsed. `None` for a synthetic TOML dwelling (that path reads the
  TOML, recorded nowhere in the records, and the absence discloses that);
  otherwise a dict with `hpxml`, `schedule` (`None` when the schedule is
  generated from the HPXML: its inputs are the config and the profiles
  CSV, both recorded) and `weather`, each a record of
  the file's `path` (or `kind` `"text"`/`"bytes"` for content) and its
  `sha256` as a 64 char hex digest over the bytes read; plus
  `weather_format` (the format the run parsed the weather as:
  `"epw"`, `"psm3"`, `"res_stock_csv"` or `"tmy3"`),
  `defaults_files_sha256` (the digest over every defaults file the load
  read, in load order, the schedule profiles file composed last) and
  `defaults_replacements`.
- Added: `OutputVariable` and the `Actor` methods `output_variables()`
  and `set_output()`: a Python actor declares its output variables (name,
  unit, kind, aggregation, across homes, sampling, labels, description)
  and publishes their per-step values during `decide()`; the declared
  variables enter the output catalogue as `actor:<name>:<var>` columns.
  A publication made outside `decide()` lands at the next drain (the
  next `decide()`'s return); one made after the last drain is dropped.
- Added: the parsed-input handles `ParsedHpxml`, `ParsedWeather` (with
  `from_path(path)` and `from_text(text)`; `ParsedWeather` also taking
  `format=` for content, the format a path detects), `ParsedSchedule`
  (with `from_path(path)` and `from_text(text)`, each taking
  `weather_meta=`: the parsed weather whose meta a schedule's
  offset-less timestamps resolve against, the same fallback the config
  path passes) and `ParsedDefaults`
  (`ParsedDefaults(base, files=None)`, the per-file replacements a
  relative-path to content dict, and `with_file(relative_path,
  content)`): parse once, build many dwellings. `Dwelling.from_hpxml`,
  `DwellingBlueprint.from_hpxml` and the `DwellingConfig` constructor
  accept a handle anywhere they accept a path, and share it
  (construction from a handle reads no file); construction still
  releases the GIL. `Dwelling.blueprint()` reads the frozen blueprint
  the dwelling was built from (`DwellingBlueprintRef`, whose
  `parsed_inputs()` is the `ParsedInputs` record holding the handles'
  shared `Arc`s).
- Added: `Dwelling.resolved_model()`: the resolved-parameter read-back,
  the parameters the physics actually uses, computed at assembly after
  defaults, autosizing and overrides and before warm-up and never
  rebuilt afterwards. A dict of SI floats and strings: `equipment`
  (each entry's `name`, `id`, `end_use`, `parameters` as
  `name`/`value`/`unit` dicts and the active `heating_setpoint` and
  `cooling_setpoint` provenance labels), `boundaries` (the RC network's
  `r_effective_m2k_w`, `films_m2k_w`, `c_j_k` and the layers'
  `r_m2k_w`/`c_j_m2k`), `windows` (the resolved effective `u_w_m2k` and
  `shgc`), `infiltration` (`ach50`, `ela_m2`, `constant_ach`) and
  `site`.
- Added: `RunResult.export_replay(dir)`, `replay(dir, defaults=None)`
  and `Provenance.tariff`: the replay export. `export_replay` writes the
  run's resolved configuration, inputs, seeds and build identity into
  `dir` (`replay.toml`, `overrides.json` when the run carried overrides,
  `tariff.json` when it attached a tariff, and the inputs' content);
  `hares.replay(dir, defaults=...)` loads it, builds the dwelling and
  simulates, returning the replayed run's `RunResult`, its frames
  bitwise the original's (`defaults=None` reads the packaged defaults). A
  run whose dwelling was changed in a way a
  replay cannot rebuild (an edited blueprint, equipment or LUT changes,
  a caller-registered actor, caller-supplied control inputs, a restored
  state) fails the export with `HaresConfigError` naming every such
  category, nothing written. `Provenance.tariff` reads the attached
  tariff as a dict holding `tariff` (the tariff's own dict) and `zone`
  (the IANA name), or `None`.
