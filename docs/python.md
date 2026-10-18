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
    # No `defaults` keyword: the packaged defaults tree the installed
    # package carries (hares.defaults_dir()). An explicit path overrides it.
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

`defaults` names the directory holding the default schedule profiles
(`Default Schedule Parameters.csv`), normally the packaged defaults tree
the installed package itself carries. Without it, `defaults=None` (the
default), the tree is resolved from the installed package's own location:
`hares.defaults_dir()` returns that path, and a dwelling built with no
`defaults` keyword reads it. An explicit path overrides it, and a
missing, unreadable or incomplete tree at the path it names (the
packaged one included) is a construction error naming the file it
missed: nothing depends on the working directory the process was started
from. An equipment whose load has no column in the schedule file and no
HPXML schedule fractions takes its default profile from the tree. A
schedule generated from the HPXML (`schedule=None`) always reads the
tree's profiles.

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
February 28 in a leap year take their non-leap ordinal. With
`civil_timezone` set, the local time is the step's civil time in that
zone, so the schedule follows the wall clock through daylight saving.

The weather is read by calendar date under the EnergyPlus typical and
actual year rules (the config's `weather_year`, `typical` by default):
a typical file is addressed by month, day and hour whatever its years,
a run past Dec 31 wraps to Jan 1 of the file, and a Feb 29 the file has
no records for is **not simulated** (the typical rule over a leap-year
run): no row is labelled Feb 29, the clock steps from the last step of
Feb 28 to Mar 1 00:00, the day types of the later dates stay the
calendar's own weekdays, and a run's step count is the calendar's less
the skipped day's. An `actual` year run must lie inside the file's own
dated records. (The schedule's February 29 note above now applies only
to a typical file that has Feb 29 records, which a leap-year run reads;
the day is otherwise not simulated.)

```python
from datetime import datetime

import hares
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
    defaults=hares.defaults_dir(),
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

## Warm-up

Every dwelling warms up by default: the warm-up repeats the run's
first day until the EnergyPlus warm-up convergence criteria are met
(at most 25 days), so the recorded run starts from a periodic initial
state, and warm-up steps are never reported (the first report row is
`start_time`). The criteria are the EnergyPlus defaults: the daily
maximum and minimum zone air temperature against
`temperature_tolerance_c` (0.4 delta C) and the daily peak HVAC
heating and cooling delivered against `loads_tolerance` (0.04, with
the 100 W floor), for every zone, with at least `min_days` (1) and at
most `max_days` (25) warm-up days.

The `warmup` keyword on `SimulationConfig` has three cases: an omitted
keyword converges with the Rust defaults; an explicit `warmup=None`
disables the warm-up (the run starts from the construction's initial
state, the outcome reads `disabled`); a `WarmupConfig(...)` sets the
criteria. The outcome is read from `RunResult.health.warmup` (or
`dwelling.health().warmup` before the run): `days_run`, `converged`
and the last day's residuals.

```python
from hares import Dwelling, SimulationConfig, WarmupConfig

dw = Dwelling.from_hpxml(
    hpxml="path/to/building.xml",
    schedule="path/to/schedules.csv",
    weather="path/to/weather.epw",
    defaults="path/to/HARES/defaults",
    config=SimulationConfig(
        start_time="2023-01-01T00:00:00-07:00",
        duration=2 * 86400,
        time_res=3600,
        warmup=WarmupConfig(max_days=2),
    ),
)
outcome = dw.health().warmup
print(outcome.outcome, outcome.days_run, outcome.converged)
```

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
    defaults="path/to/HARES/defaults",
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
    defaults="path/to/HARES/defaults",
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
    defaults="path/to/HARES/defaults",
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

- 2026-10-17: the heat pump's defrost penalty is applied by one model
  selected by the defrost config: with the discrete frost cycle (the
  default) `evaluate_defrost` feeds only the frost accumulator and the
  cycle's phases own the per-step multipliers (the Accumulating phase the
  bare frost derate without the duty factor, the Defrosting events the
  continuous average concentrated into the event window), and the cycle
  trigger honors its documented duty (`cycle_duration_s * (1 - tf)`
  frost-seconds, the EWMA persisting across events). The resistive
  defrost element's destination is one shared split (the configured zone
  fraction, default 0, the remainder frost-melt loss) across both paths.
  Re-captured: `fleet_mixed_offset`, `leap_year_february_900s`,
  `weather_wrap_900s`, `full_year_900s_dst`.

- 2026-10-17: a gas boiler's combustion residual (fuel input minus
  useful output) splits by the shared `combustion_loss_split` between
  the flue (outdoors, `flue_loss_fraction` default 0.9) and the jacket
  (the zone share); the residual was routed 100 percent to the zone as
  jacket loss. Re-captured: `fleet_mixed_offset`.

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

- 2026-10-14: the weather point values (dry bulb, dew point, relative
  humidity, pressure, wind speed and direction, sky cover, infrared,
  precipitation) follow EnergyPlus's Last Hour Interpolation at the
  step's end, replacing the PCHIP resampling and the half-hour record
  shift; the sky temperature is recomputed from the interpolated inputs,
  the ground temperature and surface albedo hold the hour's record, a
  sub-hourly file's records are read directly at its interval and an
  interval mismatch is a construction error; a step shorter than an hour
  must divide the hour. The new `first_hour_interpolation` config field
  (default `hour24`) picks the run's first hour's starting record;
  `ParsedWeather.record_label` replaces `midpoint_offset_secs`. Every
  golden re-captured (each fixture's weather reads move under the new
  rules); the per-golden deltas are in the measurement log.

- 2026-10-14: the solar irradiance values (GHI, DNI, DHI) follow
  EnergyPlus's midpoint interpolation for an hourly file at a step
  shorter than an hour: the hourly average is the value at the hour's
  midpoint, and the step's value is the linear interpolation between
  hour midpoints evaluated at the step's end, replacing the per-hour
  hold. GHI takes the same weights as beam and diffuse (EnergyPlus keeps
  no GHI; a recorded extension). Every golden re-captured again (the
  solar reads move); the per-golden deltas are in the measurement log,
  recorded separately from the point-value rule's.

- 2026-10-15: the warm-up converges by the EnergyPlus temperature and
  load criteria: after each repeated first day, every zone's daily
  maximum and minimum temperature and daily peak HVAC heating and
  cooling delivery are compared day over day against the tolerances
  (0.4 delta C and 0.04, EnergyPlus's defaults), with the minimum and
  maximum day counts (1 and 25). The old rule compared only the last
  step's conditioned-zone temperatures at 0.5 delta C and ignored
  loads, daily extremes, unconditioned zones and the day-count minimum.
  The `initialization_duration` settings are removed everywhere: the
  warm-up lives in `SimulationConfig.warmup` (the TOML `warmup =
  "disabled"` or a `[warmup]` table of the four criteria). The warmed
  fixtures' warm-ups run longer under the real criteria, so their
  initial conditions move and the goldens re-captured are
  `full_year_hourly`, `full_year_900s_dst`, `leap_year_february_900s`,
  `weather_wrap_900s`, `month_boundary_billing_3600s`,
  `fleet_mixed_offset` and `resstock_2025_fleet` (each warmed home's
  warm-up days changed); the warm-up-disabled fixtures
  (`consumer_shape_900s`, `fleet_same_offset_300s`) are unchanged. The
  days-run table and the first-day energy deltas are in the measurement
  log.

- 2026-10-15: Python dwellings warm up by default, as Rust ones do: the
  Python `SimulationConfig` gains the `warmup` keyword (an omitted
  keyword is the Rust default, `warmup=None` disables, a `WarmupConfig`
  sets the criteria) and the removed `initialization_duration` keyword
  is an error naming `SimulationConfig.warmup`. The calibration consumer
  never passed the keyword, so its evaluations warm up from this change;
  the consumer's fit absorbs it. No golden was re-captured: the golden
  manifests run through the Rust adapter, which this change does not
  touch.

- 2026-10-15: the billing month boundary: a step's energy bills to the
  billing period whose span holds it (the dwelling passes the tariff
  evaluator the step's end time, so the period closes on the fold of the
  step its boundary ends), and the monthly cycle's period ends keep the
  first period's start day of month as their anchor, clamped to each
  month's length, so a clamped boundary (a February end among them) no
  longer drifts the chain. The boundary step's energy, cost and peak
  move from the closing period's summary to the opening one's in every
  tariffed run that spans a cycle boundary; the totals across the
  periods are unchanged, every kWh billed exactly once. A tariff
  attached exactly at a boundary and a from-start run now book the
  boundary step to the same period. The Python billing summaries move
  the same way. No golden was re-captured: the one tariffed golden
  closes no billing boundary inside its run, and all nine are bitwise
  (compare-all 8 + dst 1).

- 2026-10-16: the end-use rollup classifies equipment by its typed
  end use (the descriptor's declared end use through the catalogue),
  never by its name string; the name classifiers
  (`equipment_name_to_end_use`, `is_hvac_or_wh`, `is_ev`, `has_soc`,
  `is_heat_pump_heater`, `is_cooling_equipment`, `is_pv`,
  `is_compressor_equipment`, `display_name_to_end_use_key`) are
  deleted. Every golden re-captured: in the resstock homes the
  `TV` plug load's power moves from the `other` end use to
  `plug_loads` (the class declares `plug_loads`; the name string fell
  to `other`), and in every fixture the `Cooking Range`'s power moves
  from `cooking` to `other` (the range is an event-based load whose
  descriptor declares `other`; the name string claimed `cooking`), so
  the `cooking` end-use aggregate column disappears from every golden
  schema and the per-end-use peaks and energies re-bucket. The parity
  homes' per-equipment columns are otherwise unchanged (their
  classes' declared end uses agree with the old name mapping), and the
  per-end-use totals across the aggregates are unchanged: the rollup
  re-buckets, it does not move energy between equipment. The all-classes
  catalogue pins (`tests/fixtures/schema/`) re-captured with the same
  cause (the phantom EV columns of the event-based load and the
  television pin are gone; the pool and spa heaters leave the HVAC
  family; the ideal load, the `HPWH` alias and the indirect tank gain
  the HVAC family their declarations carry; the ground-source and
  water-loop heat pump heaters gain the defrost and ER columns their
  pan heater declaration carries and the old name matcher missed).
- 2026-10-16: a not-applicable output value is null, never 0.0: every
  output column's cell where the underlying quantity does not apply to
  the current state reads null (the Arrow null, Python `None` in the
  polars frames), so means and sums are not poisoned by zeros-as-data.
  The catalogue declares every variable's nullability; the not-nullable
  quantities (the electric power, gas power, energy, capacity, reactive
  power, ER power, fan power, main power, runtime fraction, latent
  gains, schedule state and end-use aggregates: physically zero when
  inactive) keep their zeros. The heat pump's and air conditioner's COP
  is null when the step's compressor did not run, the sensible heat
  ratio null when the step delivered no sensible cooling, an actor
  telemetry key the actor did not emit is null, the setpoint columns
  are null before the first reconcile, and a column no producer claims
  (the unmet-load column) is null. Every golden re-captured (compare-all
  8 + dst 1, twice byte-identical): the unmet column's cells 0.0 to
  null and the schema's nullable flags; no metric, health or billing
  figure moved.

- 2026-10-16: the fleet's `Enum` columns (the per-equipment operating
  modes and compressor speed, the EV connection state and charging
  level, an actor's declared enum channel) fold by the weighted Mode in
  both fleet stages, replacing the mean of mode codes the unit-keyed
  rules took: each home's period reduction takes the most frequent code
  (ties to the lowest code, the reporting reducer's own Mode rule, fed
  the enum's label count from the schema field's new `enum_labels`
  metadata) and the across-homes fold takes the code with the greatest
  summed home weight. A mean over mode codes is no code and is never
  taken; binary state columns keep the weighted mean (the duty
  fraction). No golden re-captured: the fleet goldens' verbosity-2
  selection holds no `Enum` column, and every golden compares bitwise
  (compare-all 8 identical, the dst set identical).

- 2026-10-16: the run metrics are independent of the reports' verbosity
  and selection: when a run requests metrics, the metrics' inputs
  resolve from the producer surface (the row plan's full declared
  surface, every catalogue variable the dwelling's producers can
  produce) instead of the union of the reports' selected columns, so
  `RunResult.metrics` is identical at every verbosity and with no
  report at all. Before, the envelope-component metrics computed only
  where the selection carried the verbosity-4 delivered columns and the
  verbosity-6 gain columns (`envelope_loads_kwh` None and the HVAC COPs
  None below verbosity 4, the per-end-use maps empty at verbosity 0);
  after, the same components compute at every verbosity, and the
  verbosity-8 values are bitwise unchanged. Every golden re-captured:
  in each the `envelope_loads_kwh` and the two HVAC COP fields move
  from null to their verbosity-8 values and no frame product, schema or
  other metrics field moves; the per-golden values are in the
  measurement log. A run that does not request metrics is unchanged
  (`None`, nothing computed).

- 2026-10-16: the ResStock fleet's start is declared, not hardcoded: every
  home starts Jan 1 00:00 local standard time at its own site's standard
  offset (the site-location resolution the built dwelling runs) in the
  weather file's own year for an actual-year release (2025.1, AMY: the
  calendar year whose hours the file's records cover, read from the
  file's dated stamps, so an AMY 2018 file runs 2018, not the last
  stamp's 2019), and the caller's `typical_year` argument for a
  typical-year release (2024.x, TMY3: no built-in year). The release
  declares the weather-year rule (`weather_year` = `actual` for 2025.1,
  `typical` for 2024.x), and a building whose weather file cannot be
  resolved or read is a construction error naming every failing building
  with its county (the warned empty weather path is gone). `Fleet.from_resstock`
  gains the `typical_year=None` keyword. `resstock_2025_fleet`
  re-captured: the movement is the `Time` labels' year (2019 to 2018)
  in the `aggregate` and `covered_weight` products only - every energy,
  power and weather column is bitwise unchanged (the homes read the
  same records of the file: the same month, day and hour), and the
  `homes` and `fleet_report` products are bitwise unchanged.

- 2026-10-16: a missing defaults directory is an error and Python uses the
  packaged defaults. The defaults source's base directory is required
  everywhere (no caller falls back to a working-directory `defaults`, so
  the numbers a run produces no longer depend on where it was started),
  and every sub-table the loader reads is a required file: the HVAC
  multispeed CSV, the water-heating CSV, the EV vehicle mapping, the
  generator efficiency curve and the envelope LUT's three CSVs - a
  missing, unreadable or malformed one fails the load naming the file,
  the envelope LUT's failure no longer warns into the lumped fallback-R
  path, and the `HARES_STRICT_DEFAULTS` environment variable is gone
  (the loader has one behaviour, the strict one). In Python,
  `defaults=None` (the default) reads the packaged defaults tree the
  installed package carries, resolved from the package's own location
  (`hares.defaults_dir()`); an explicit path overrides it. The wheel
  carries the tree at `hares/defaults/` with no top-level `defaults/`
  entry (it moved from the repository root into `python/hares/defaults/`,
  so `maturin develop` resolves the same packaged path). Every golden
  re-captured: the products are bitwise identical, the recorded
  movement is the manifests' `defaults_dir` provenance string
  (`defaults` to `python/hares/defaults`) and the capture head; the
  defaults digest is unchanged (the digest runs over tree-relative
  paths and bytes).

- 2026-10-16: the fleet's `Time` labels are the local site time: the
  fleet's `aggregate` and `covered_weight` products carry a naive
  `Timestamp(Millisecond, None)` at each period's start, the convention
  every per-dwelling output follows, replacing the Utf8 RFC 3339
  strings (an undeclared fleet and `Instant(+00:00)` rendered each
  row's start instant in UTC with a `Z` suffix, `Instant(offset)` at
  the declared offset, `LocalClock` as bare text). A single-offset
  fleet's labels are the homes' own local wall clock; a multi-offset
  fleet declares its alignment and `LocalClock` labels by the local
  wall clock each folded home's step starts at, `Instant(offset)` by
  the declared reference zone's. The schema metadata records the offset
  or the reference zone as `hares:utc_offset_s` (under `LocalClock` no
  single offset applies and none is recorded). The three fleet goldens
  re-captured (`fleet_mixed_offset`, `fleet_same_offset_300s`,
  `resstock_2025_fleet`): the movement is the `Time` labels only in the
  `aggregate` and `covered_weight` products; every energy, power and
  weather column and the `homes` and `fleet_report` products are
  bitwise unchanged. The other goldens are bitwise identical
  (compare-all 8 + dst 1).

- 2026-10-17: the fleet's variables fold by their declared kind, not by
  unit string: each home's period reduction takes the variable's
  declared over-time rule (a quantity declared `Sum` sums its steps
  instead of averaging them, a running total takes its value at the
  period's last valid step) and the across-homes fold takes the kind's
  rule (energy and power the weighted sum, temperature, ratio and state
  the weighted mean, a quantity its declared across-homes rule, an enum
  the weighted Mode), replacing the unit-keyed tables and their
  fallbacks. A fleet's ratio, binary state, mode code and
  intensive-quantity cells are no longer weighted sums over homes, and
  a per-step count or volume is no longer averaged over the period. The
  declared rules reach the fold as schema field metadata: the schema
  builders write every variable's `kind`, over-time `aggregation` and,
  for a quantity, `across_homes` tag beside the unit. The `enum` unit
  symbol merges into `code` (no catalogue variable declared it, so no
  column's unit metadata changes). No golden re-captured: the fleet
  goldens' verbosity-2 selection holds only energy, power and
  temperature columns, whose unit-keyed rules already matched the
  declared ones, and every golden compares bitwise (compare-all 8
  identical, the dst set identical). A fleet run at a selection holding
  the columns the fallback got wrong moves those cells: the acceptance
  run's delta (three weather channels, from the weighted sum to the
  weighted mean) is in the measurement log.
- 2026-10-17: a dwelling with no schedule file runs the HPXML thermostat's
  own hourly weekday and weekend profiles, or the manual-thermostat default
  profile when the HPXML declares no thermostat, instead of the generated
  schedule's constant setpoint columns (the profile's first weekday hour,
  or 20 C heating and 24 C cooling when the HPXML has none): the generated
  schedule carries no setpoint column, so each side resolves by the
  setpoint-source rule and the resolution's active source is recorded and
  logged at debug level. No golden fixture moves: no golden manifest runs a
  generated schedule, compare-all 8 and the dst set's 1 are bitwise
  identical compared twice on the final state, and the acceptance run's
  every variable is bitwise identical before and after on this line (the
  constant columns' removal landed with the phase-1 fold, before any row's
  capture; the acceptance tests fail against the reverted generator, which
  still wrote the constants). No golden re-captured.

- 2026-10-17: a schedule file's `heating_setpoint` and `cooling_setpoint`
  columns are read in degrees Fahrenheit, OpenStudio-HPXML's
  detailed-schedule convention, and converted to Celsius at the parse with
  the uom conversion, once per value (they were read as Celsius, so a
  schedule written for the HPXML ecosystem drove the thermostat at, for
  example, 70 C). The input schema's column table states the unit as
  Fahrenheit. No golden moves: no golden manifest's schedule carries a
  setpoint column (the census), and compare-all 8 and the dst set's 1 are
  bitwise identical compared twice on the final state. No golden
  re-captured.

- 2026-10-17: the Fahrenheit-to-Celsius conversion
  (`hares_physics::units::temperature_f_to_c`) follows the faithful
  formula `(t - 32.0) * 5.0 / 9.0` instead of the `uom` conversion: uom
  reaches Celsius through the kelvin base with rounded offset constants
  (`(t + 459.67) * 5/9 - 273.15`), which landed the result ulps off the
  formula's exact value (70 F converted to 21.1111111111112 instead of
  21.111111111111114, 25 ulps high; found by the Fahrenheit setpoint
  test's independent bit pin, which the schedule-setpoint row's review
  required). The water mains temperature moves at the same scale in
  every run, so the run numbers shift in their last few digits: all
  nine goldens re-captured (`consumer_shape_900s`, `fleet_mixed_offset`,
  `fleet_same_offset_300s`, `full_year_hourly`,
  `leap_year_february_900s`, `month_boundary_billing_3600s`,
  `resstock_2025_fleet`, `weather_wrap_900s`, `full_year_900s_dst`;
  compare-all 8 and the dst set's 1 bitwise identical after; the
  movement is the metrics' last few digits, for example the consumer
  shape's heating COP 39.192071398252686 to 39.192071398252665 and its
  cooling COP 2.8742769469758733 to 2.874276946975894). The
  water-heater autosizing's mains bound moves from the exclusive
  (0, 50) C to [0, 50) C with it: the Burch-Christensen clamp floor
  (32 F) converts to exactly 0 C under the faithful formula, while the
  previous conversion's rounding sat an epsilon above the floor and
  never met the exclusive bound; 0 C is the weather's own value, not a
  stand-in, and negatives stay rejected.

- 2026-10-17: a constant setpoint override (`setpoint.heating_setpoint_c`
  or `setpoint.cooling_setpoint_c` in an equipment's override) replaces
  the schedule column, the HPXML setpoints and the default profile the
  resolution stamped for that side, instead of being accepted and never
  applied: the merge removes the matching `*_setpoint_source` when the
  override sets the constant, and the resolved read-back records the
  constant as the side's active source. The deadband reconciliation at
  equipment init never moves a side the config names a constant for. No
  golden re-captured: no golden sets a setpoint override and every golden
  compares bitwise (compare-all 8 identical, the dst set identical); the
  override's own delta is in the measurement log.
