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
from hares import Dwelling, ControlSignal, SimulationConfig

dw = Dwelling.from_hpxml(
    hpxml="path/to/building.xml",
    schedule="path/to/schedules.csv",
    weather="path/to/weather.epw",
    config=SimulationConfig(),
    defaults_path="path/to/HARES/defaults",
)
dw.initialize()

# Run a full-year simulation
results = dw.simulate()
print(results.head())

# Or step through manually with control
for ts in dw.timesteps():
    signal = ControlSignal.thermal_setpoint(heat_c=20.0, cool_c=24.0)
    dw.apply_control("HVAC Heating", signal)
    obs = dw.step()
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

from hares import Dwelling, SimulationConfig

dw = Dwelling.from_hpxml(
    hpxml="tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh/building.xml",
    schedule="tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh/schedule.csv",
    weather="tests/fixtures/parity/cz2a_gas_furnace_ac_res_wh/weather.epw",
    config=SimulationConfig(
        start_time="2023-01-01T00:00:00-07:00",
        duration=6 * 3600,
        time_res=3600,
    ),
    defaults_path="defaults",
)
dw.initialize()

frame = dw.simulate()
first = frame["Time"][0]  # a naive datetime: the local wall clock
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
other run-total counters.

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
