# Python Bindings

HARES exposes its Rust simulation engine to Python through [PyO3](https://pyo3.rs/)
and [Maturin](https://www.maturin.rs/). The resulting package is called `ochre_next`.

## Prerequisites

| Tool  | Minimum version | Install |
|-------|----------------|---------|
| Rust  | 1.87+          | [rustup.rs](https://rustup.rs/) |
| uv    | 0.11+          | `brew install uv` or [docs.astral.sh/uv](https://docs.astral.sh/uv/) |
| Python| 3.13           | Managed by uv (see below) |

## Quick start

```bash
# Clone and initialise submodules
git clone https://github.com/NREL/HARES.git
cd HARES
git submodule update --init --recursive

# Create a virtual environment with Python 3.13 and install dev dependencies
uv venv --python 3.13
uv sync

# Build the Rust extension in-place (debug mode, fast iteration)
uv run maturin develop

# Verify the install
uv run python -c "from ochre_next import Dwelling; print('ok')"
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
│   └── src/lib.rs       #[pymodule] entry point → ochre_next._hares
└── python/ochre_next/   Pure-Python package that re-exports Rust types
    ├── __init__.py      Dwelling, Fleet, ControlSignal
    ├── _hares.pyi       Type stubs for IDE/mypy support
    ├── adapters/        PyBaMM, PySAM adapters
    ├── data/            ResStock + weather fetching
    ├── rl/              Gymnasium environment wrappers
    └── helics/          HELICS co-simulation
```

Maturin reads `[tool.maturin]` in `pyproject.toml` to find the Rust manifest
(`crates/hares-python/Cargo.toml`) and the Python source root (`python/`).
It compiles the cdylib, names it `ochre_next._hares`, and places the `.so`
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
from ochre_next import Dwelling, ControlSignal

dw = Dwelling.from_hpxml(
    hpxml="path/to/building.xml",
    schedule="path/to/schedules.csv",
    weather="path/to/weather.epw",
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

## Troubleshooting

**`maturin develop` fails with linker errors**
Make sure Rust is installed via rustup and your toolchain is up to date:
`rustup update stable`

**`ImportError: cannot import name '_hares'`**
The extension hasn't been built yet. Run `uv run maturin develop`.

**Wrong Python version**
Maturin builds against the Python in the active virtualenv. Verify with
`uv run python --version` — it should be 3.13.x.
