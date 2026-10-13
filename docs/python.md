# Python Bindings

HARES exposes its Rust simulation engine to Python through [PyO3](https://pyo3.rs/)
and [Maturin](https://www.maturin.rs/). The resulting package is called `hares`.

## Prerequisites

| Tool  | Minimum version | Install |
|-------|----------------|---------|
| Rust  | 1.89+          | [rustup.rs](https://rustup.rs/) |
| uv    | 0.11+          | `brew install uv` or [docs.astral.sh/uv](https://docs.astral.sh/uv/) |
| Python| 3.12–3.13     | Managed by uv (see below) |

## Quick start

```bash
# Clone and initialise submodules
git clone https://github.com/AZX-PBC-OSS/HARES.git
cd HARES
git submodule update --init --recursive

# Create a virtual environment with Python 3.12 and install dev dependencies
uv venv --python 3.12
uv sync

# Build the Rust extension in-place (debug mode, fast iteration)
uv run maturin develop

# Verify the install
uv run python -c "from hares import Dwelling; print('ok')"
```

## Building

### Development (debug, editable)

```bash
uv run maturin develop
```

This compiles the `hares-python` crate as a shared library and installs it
into the active virtualenv as an editable package. Subsequent calls only
recompile changed Rust code.

### Development (release, optimised)

```bash
uv run maturin develop --release
```

Use this when you need representative runtime performance (benchmarks,
profiling, parity tests against OCHRE).

### Wheel (distributable)

```bash
uv run maturin build --release
```

The wheel lands in `target/wheels/` and can be installed with `uv pip install`.

## How it works

```
pyproject.toml          Maturin build config + Python project metadata
├── crates/hares-python  PyO3 cdylib crate (Cargo.toml lib type = ["cdylib"])
│   └── src/lib.rs       #[pymodule] entry point → hares._hares
└── python/hares/   Pure-Python package that re-exports Rust types
    ├── __init__.py      Dwelling, Fleet, ControlSignal
    ├── _hares.pyi       Type stubs for IDE/mypy support
    ├── adapters/        PyBaMM, PySAM adapters
    ├── data/            ResStock + weather fetching
    ├── rl/              Gymnasium environment wrappers
    └── helics/          HELICS co-simulation
```

Maturin reads `[tool.maturin]` in `pyproject.toml` to find the Rust manifest
(`crates/hares-python/Cargo.toml`) and the Python source root (`python/`).
It compiles the cdylib, names it `hares._hares`, and places the `.so`
alongside the Python source.

## Running tests

```bash
# Rust tests
cargo test

# Python tests (fast, parallel — ~5s)
uv run pytest

# Include slow tests (ResStock fetches, PyBaMM model gen)
uv run pytest -m ""

# Slow tests only
uv run pytest -m slow

# Sequential with output (debugging)
uv run pytest -n0 -s
```

## Optional dependencies

Install extras for specific integrations:

```bash
uv pip install -e ".[sam]"       # NREL PySAM (PV + battery parameter extraction)
uv pip install -e ".[pybamm]"   # PyBaMM electrochemical battery model
uv pip install -e ".[resstock]" # ResStock dataset fetching (boto3, httpx)
uv pip install -e ".[helics]"   # HELICS co-simulation
uv pip install -e ".[rl]"       # Gymnasium RL environments
```

## Usage example

```python
from hares import Destination, Dwelling, ReportSpec, Selection, SimulationConfig, ControlSignal

dw = Dwelling.from_hpxml(
    hpxml="path/to/building.xml",
    schedule="path/to/schedules.csv",
    weather="path/to/weather.epw",
    config=SimulationConfig(
        reports=[ReportSpec("frame", Selection.verbosity(2), Destination.memory())],
    ),
    defaults_path="path/to/HARES/defaults",
)
dw.initialize()

# Run a full-year simulation: the return value is the run's RunResult
result = dw.simulate()
print(result.reports[0].table().head())

# Or step through manually with control, then finalise the run
for ts in dw.timesteps():
    signal = ControlSignal.thermal_setpoint(heat_c=20.0, cool_c=24.0)
    dw.apply_control("HVAC Heating", signal)
    obs = dw.step()
result = dw.finish()
```

`defaults_path` names the directory holding the default schedule profiles
(`Default Schedule Parameters.csv`), normally the repository's `defaults/`.
An equipment whose load has no column in the schedule file and no HPXML
schedule fractions takes its default profile from there. Without
`defaults_path` there are no default profiles, so such an equipment fails
construction, naming itself and `defaults_path`; a schedule file that
names every column the building's equipment needs builds without one. A
schedule generated from the HPXML (`schedule=None`) always needs it.

## Time

Every output row's `Time` is an Arrow `Timestamp(Millisecond, None)`:
the **naive local standard time** at the interval's start, a step's
start for a timestep report or a period's nominal start for a
reporting-period frequency, and nothing else. `Time` is one convention
across the whole surface: a frame's column, a step's `timestamp` and the
environment's `current_time` all carry the same local wall clock, as
naive values with no time zone attached.

To recover the UTC instant, apply the site's standard-time UTC offset,
carried in the output schema's metadata as `hares:utc_offset_s` (decimal
seconds, negative west of Greenwich): `UTC = Time - hares:utc_offset_s`.
For a site at UTC-7 the metadata is `-25200`, so local midnight is
`07:00Z`. A non-`Timestep` report additionally carries a `Step Count`
column and, for each nullable variable, a `<name> [valid steps]` column
counting the steps that contributed a value.

`step()`'s dict carries `"timestamp"` as a naive `datetime` equal to the
row's `Time`. A schedule row maps to a step by the step's local time of
year divided by the step length, over a 365-day year: dates after
February 28 in a leap year take their non-leap ordinal (February 29
shares March 1's row). With `civil_timezone` set, the local time is the
step's civil time in that zone, so the schedule follows the wall clock
through daylight saving.

```python
from datetime import datetime

from hares import Destination, Dwelling, ReportSpec, Selection, SimulationConfig

dw = Dwelling.from_hpxml(
    hpxml="tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh/building.xml",
    schedule="tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh/schedule.csv",
    weather="tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh/weather.epw",
    config=SimulationConfig(
        start_time="2023-01-01T00:00:00-07:00",
        duration=6 * 3600,
        time_res=3600,
        reports=[ReportSpec("frame", Selection.verbosity(1), Destination.memory())],
    ),
    defaults_path="defaults",
)
dw.initialize()

result = dw.simulate()
first = result.reports[0].table()["Time"][0]  # a naive datetime: the local wall clock
assert first == datetime(2023, 1, 1, 0, 0), "the first Time is start_time's wall clock"
```

## Equipment zones

Every HVAC unit, water heater and ventilation unit runs in a zone it can
name; a unit that names none fails when the dwelling is built.

- HVAC equipment (furnaces, boilers, baseboards, air conditioners, heat
  pumps, ideal HVAC), dehumidifiers and ventilation serve the zone given by
  `zone_id`, or the dwelling's conditioned zone when `zone_id` is omitted.
- Water heaters need a `zone_id` (or, from HPXML, a `Location`). The
  Python water heater classes take it as a required keyword argument, so a
  heater built without one raises `TypeError` at construction. HPXML
  locations with no modeled zone ("other heated space", "other
  multifamily buffer space", "other non-freezing space", "other housing
  unit", "other exterior", "outside") run against that location's ambient
  air: the OS-HPXML indoor/outdoor blend with its temperature floor, and a
  wet-bulb from the blended humidity ratio for heat pump water heaters.
  Their standing losses leave the model there instead of heating a zone.
  Blending the humidity ratio conserves water vapour; OS-HPXML averages
  the two relative humidities instead. On a winter hour (22 °C / 40 % RH
  indoors, 0 °C / 80 % RH outdoors, "other heated space" at its 20 °C
  floor) HARES's evaporator inlet is 3.8 K lower in wet-bulb (11.3 °C
  against 15.1 °C) and the heat pump water heater's COP is 9.3 % lower
  than under OS-HPXML's rule (the
  `hpwh_winter_cop_under_the_vapour_conserving_blend_against_the_rh_average`
  test in `hares-equipment`).

## Reinforcement learning

The `hares.rl` package wraps the engine in Gymnasium environments
(`DwellingGymEnv` for one dwelling, `VecDwellingGymEnv` for a fleet stepped
through the extension's `batch_step`). Both paths clamp every action value
to the declared bounds of its column before the value reaches the dwelling,
and the clamped values are counted on the acting dwelling's run health in
every build; the count is exposed with the run's `RunResult`, alongside the
other run-total counters. An episode's truncating step finalises the run
through `finish()`; the finished `RunResult` is the wrapper's `run_result`
(one dwelling) or `run_results` (the vector), and the underlying dwellings
are reachable (`env.dwelling`, `envs`), so `health()` reads at any step.

The reset contract: `reset(seed=...)` restores the warmed-up state and
reseeds the random streams, without rebuilding. The warmed-up state is the
one the environment's construction built (the converged initial condition
its construction seed produced, snapshotted at construction); `load_state`
restores it and `Dwelling.reseed(seed)` re-derives every random stream the
dwelling holds for the seed given, with each stream at its start, so
`reset(seed=...)` fixes the random streams of the episode that follows. No
dwelling is re-constructed and no warm-up runs again; the explicit full
rebuild remains `Dwelling.reset_with_seed(seed)`. The observations are read
through a frozen observation plan (`Dwelling.observation_plan(fields)`,
built once per episode at reset): every step fills the environment's own
buffer in place through its buffer protocol, and `step()` returns a copy of
that buffer by default; a `VecDwellingGymEnv` constructed with
`copy=False` returns the buffer itself, overwritten by the next `step()` or
`reset()`, for callers that consume each observation before stepping again.

Two contracts the batched step inherits from the engine. A step whose
observation read fails ends that environment with its error:
`terminated=True`, the error named in `info["error_msg"]`, and its row of
the buffer set to NaN. And a Python actor's publication made outside
`decide()` lands at the next drain (the next `decide()`'s return); one
made after the last drain is dropped.

## Results

One run carries one result: `Dwelling.simulate()` and
`Dwelling.finish()` return the run's `RunResult`: the reports' delivered
outputs, the metrics, the health totals, the warning record and the
provenance.

```python
from hares import Destination, Dwelling, ReportSpec, Selection, SimulationConfig

dw = Dwelling.from_hpxml(
    hpxml="path/to/building.xml",
    schedule="path/to/schedules.csv",
    weather="path/to/weather.epw",
    config=SimulationConfig(
        start_time="2019-01-01T00:00:00",
        duration=86400,
        time_res=900,
        metrics=True,
        reports=[ReportSpec("frame", Selection.verbosity(2), Destination.memory())],
    ),
    defaults_path="path/to/HARES/defaults",
)
dw.initialize()

# Run the whole horizon: the return value is the run's RunResult.
result = dw.simulate()

frame = result.reports[0].table()   # the memory report's polars DataFrame
print(result.metrics.total_energy_kwh.net_energy_kwh)
print(result.health.clamped_actions)
for warning in result.warnings:
    print(warning.source, warning.message)
print(result.provenance.bldg_id, result.provenance.site_utc_offset_s)
```

The lifecycle: `initialize()` is required once before the first `step()`
or `simulate()`, both of which raise `RuntimeError` without it, and
`reset_with_seed` clears it (a fresh `initialize()` re-snapshots). A run
steps to its end either through `simulate()` or one `step()` at a time,
then `finish()` finalises it: the billing close, the recorders' flush and
publish (the final partial batch included), the metrics, the health, the
warning record and the provenance, in one path. `finish()` is idempotent
(every call after the first returns the first call's result) and a
`step()` past the finished run is a `RuntimeError`. A run that ended at an
error stays ended: a failed batch callback (at a step's flush or at the
close) or an equipment past its failure budget latches the run, and the
latched run's `finish()` raises the latched error, carrying the original
cause, instead of returning a result.

```python
from hares import Destination, Dwelling, ReportSpec, Selection, SimulationConfig

dw = Dwelling.from_hpxml(
    hpxml="path/to/building.xml",
    schedule="path/to/schedules.csv",
    weather="path/to/weather.epw",
    config=SimulationConfig(
        start_time="2019-01-01T00:00:00",
        duration=86400,
        time_res=900,
        reports=[ReportSpec("frame", Selection.verbosity(2), Destination.memory())],
    ),
    defaults_path="path/to/HARES/defaults",
)
dw.initialize()
for _ in dw.timesteps():
    dw.step()
result = dw.finish()          # the same finalisation simulate() runs
print(result.reports[0].rows)
```

`health()` is live: readable at any step, needing no report.

```python
from hares import Dwelling, SimulationConfig

dw = Dwelling.from_hpxml(
    hpxml="path/to/building.xml",
    schedule="path/to/schedules.csv",
    weather="path/to/weather.epw",
    config=SimulationConfig(
        start_time="2019-01-01T00:00:00",
        duration=86400,
        time_res=900,
    ),
    defaults_path="path/to/HARES/defaults",
)
dw.initialize()
dw.step()
print(dw.health().rejected_control_signals)
print(dw.health().warmup.outcome)   # "disabled" or "ran"
```

Requesting reports: a run records nothing no report asks for. A report is
a name, a selection (`Selection.verbosity(n)`, `.named([...])`,
`.pattern(glob)`, `.diagnostics()`), a frequency (`Frequency.timestep()`)
and a destination (`Destination.memory()`, `.parquet(path, overwrite=False)`,
`.csv(path, overwrite=False)`). A `Memory` destination retains the run's
batches and its `ReportResult.table()` returns the frame; `arrow()`
exposes the same batches through the Arrow PyCapsule interface, the
buffers shared, so any Arrow consumer reads them zero-copy; a file
destination's result is the published path (its `table()` and `arrow()`
raise, naming the report and its destination); a run with no report
carries no metrics.

Each column's Arrow type is the report's schema's type for that column,
with units and enum labels read from the catalogue (polars drops Arrow
field metadata, so the schema, `ReportResult`'s and the catalogue's, is
where a column's unit and labels live).

`SteppableFleet` stepping does not rewind: a stepped fleet's dwellings
advance one step per `step()` call across the whole fleet.

## Changes in this release

Every callable, class and keyword added to, changed in or removed from
`hares._hares` since `ebdb1a17`, each removed or renamed one with its
replacement.

- Added: `ReportSpec`, `Selection` (with `verbosity`, `named`, `pattern`,
  `diagnostics`), `Frequency` (with `timestep`), `Destination` (with
  `memory`, `parquet`, `csv`, `callback`), `RunResult`, `ReportResult`,
  `RunMetrics`, `RunHealth`, `WarmupOutcome`, `WarmupResiduals`, `Warning`,
  `Provenance`, `EquipmentProvenance`, `SiteLocationOverride`.
- Added: `HealthPolicy` (with `strict()`), `HaresHealthPolicyError`, and
  the `SimulationConfig.health_policy` keyword.
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
  bitwise the original's (the `defaults` directory is required until the
  packaged defaults land). A run whose dwelling was changed in a way a
  replay cannot rebuild (an edited blueprint, equipment or LUT changes,
  a caller-registered actor, caller-supplied control inputs, a restored
  state) fails the export with `HaresConfigError` naming every such
  category, nothing written. `Provenance.tariff` reads the attached
  tariff as a dict holding `tariff` (the tariff's own dict) and `zone`
  (the IANA name), or `None`.

## Troubleshooting

**`maturin develop` fails with linker errors**
Make sure Rust is installed via rustup and your toolchain is up to date:
`rustup update stable`

**`ImportError: cannot import name '_hares'`**
The extension hasn't been built yet. Run `uv run maturin develop`.

**Wrong Python version**
Maturin builds against the Python in the active virtualenv. Verify with
`uv run python --version` — it should be 3.12.x or 3.13.x.

## Numeric changes

Every number a commit moves is recorded here: what moved, why, and the
golden fixtures re-captured.

- 2026-10-08: the wet appliances' daily hot-water volume and annual energy
  follow the OpenStudio-HPXML v1.12.0
  `calc_clothes_washer_energy_gpd` / `calc_dishwasher_energy_gpd` cycles
  rule (operational in residents, Addendum 81 asset otherwise) instead of a
  fixed 15 L and 6 L per cycle replayed on the step; the volume is drawn
  through the `hot_water_clothes_washer` and `hot_water_dishwasher` schedule
  columns at the column's peak flow and is step-independent. Every golden
  re-captured: `full_year_hourly`, `full_year_900s_dst`, `leap_year_february_900s`,
  `weather_wrap_900s`, `month_boundary_billing_3600s`, `consumer_shape_900s`,
  `fleet_mixed_offset`, `resstock_2025_fleet` (each home has a washer or
  dishwasher and a water heater).
