# HARES

[![CI](https://github.com/NREL/HARES/actions/workflows/ci.yml/badge.svg)](https://github.com/NREL/HARES/actions/workflows/ci.yml)

HARES — High-performance Agent-based Residential Energy Simulation

A Rust workspace for whole-building energy simulation with Python bindings
via PyO3 / Maturin, published as the `ochre_next` Python package.

## Repository setup

```bash
git clone https://github.com/NREL/HARES.git
cd HARES
git submodule update --init --recursive
```

The `vendors/OCHRE` submodule contains reference data used by validation
tests. Tests that depend on it will fail if the submodule is not initialised.

## Prerequisites

| Tool   | Version | Install |
|--------|---------|---------|
| Rust   | 1.87+   | [rustup.rs](https://rustup.rs/) |
| uv     | 0.11+   | `brew install uv` or [docs.astral.sh/uv](https://docs.astral.sh/uv/) |
| Python | 3.13    | Managed by uv |

After installing Rust and uv:

```bash
uv venv --python 3.13
uv sync
```

## Building and checking

```bash
cargo build                        # debug build (all default members)
cargo build --release              # optimised build
cargo check                        # type-check without codegen (fastest feedback)
```

Debug builds enable runtime invariant checks (energy balance, temperature
bounds) automatically. Release builds compile these out for zero overhead
but can opt back in:

```bash
cargo build --release -F check_invariants   # release + conservation checks
cargo build --release -F observe            # release + step-level observer
```

**Use release builds for benchmarking and OCHRE performance comparisons.**
Debug builds include overhead not present in production.

See [docs/development.md](docs/development.md) for the full guide to
build profiles, feature flags, and when to use each.

## Linting

```bash
cargo clippy -- -D warnings        # lint — must be warning-free
cargo fmt --check                  # check formatting
cargo fmt                          # auto-format
```

## Testing

```bash
cargo test                         # run all Rust tests
cargo test -p hares-physics        # test a single crate
uv run pytest                      # Python tests
uv run pytest -m "not slow"        # skip slow Python tests
```

## Python bindings

See [docs/python.md](docs/python.md) for full details. Quick start:

```bash
uv run maturin develop             # build + install into venv (debug)
uv run maturin develop --release   # build + install into venv (release)
```

## ResStock and weather data

HARES fetches building models (HPXML + schedules) and weather files from
the NREL [OEDI data lake](https://data.openei.org/s3_viewer?bucket=oedi-data-lake&prefix=nrel-pds-building-stock%2Fend-use-load-profiles-for-us-building-stock%2F)
(public S3, no credentials or AWS CLI required).

### Supported ResStock versions

| Version | Dataset | Weather | S3 prefix |
|---------|---------|---------|-----------|
| `2024.2` | ResStock TMY3 Release 2 | TMY3 EPW (county FIPS) | `2024/resstock_tmy3_release_2/` |
| `2025.1` | ResStock AMY 2018 Release 1 | AMY 2018 CSV | `2025/resstock_amy2018_release_1/` |

- **2024.2** uses TMY3 EPW files from [BuildStock_TMY3_FIPS.zip](https://data.nrel.gov/submissions/156)
  (~760 MB, downloaded and extracted once).
- **2025.1** uses AMY 2018 weather CSVs fetched per-county from S3.

### Setup

```bash
uv pip install -e ".[resstock]"    # installs boto3 + httpx
```

### Fetching buildings

```python
from ochre_next.data import fetch_resstock_building

# Single building — cached to ~/.cache/ochre_next/resstock/
bldg = fetch_resstock_building(bldg_id=1, version="2024.2")
print(bldg.hpxml_path, bldg.weather_path)

# 2025.1 (AMY weather)
bldg = fetch_resstock_building(bldg_id=1, version="2025.1")
```

For fleet-scale fetches, `fetch_resstock_fleet` downloads buildings
concurrently via `httpx` when available. The fetcher tries
`httpx` -> `boto3` -> `urllib` in order; `httpx` is recommended for async
fleet downloads.

## Crate layout

```
crates/
├── hares-types       Shared types: control signals, environment, equipment descriptors, ports
├── hares-physics     Physical models: psychrometrics, solar, infiltration, ground temperature
├── hares-envelope    RC-network thermal envelope solver (state-space, electrical, fluid, humidity)
├── hares-control     Control dispatch, capability matching, OCHRE signal compatibility
├── hares-equipment   Equipment models: HVAC, battery, PV, EV, water heater, ventilation
├── hares-io          I/O: HPXML parsing, EPW/TMY3 weather, schedules, Arrow/Parquet output
├── hares-tariff      Electric and gas tariffs: URDB parsing, evaluation, billing
├── hares-core        Simulation engine: dwelling, clock, actor system, checkpoint, invariants
├── hares-fleet       Fleet-level parallel simulation with rayon and weighted aggregation
└── hares-python      PyO3 cdylib — exposes the engine as the ochre_next._hares Python module
```

Dependency flow (each crate depends on those above it):

```
types → physics → envelope
              ↘         ↘
         control → equipment → io ↘
                               tariff → core → fleet → python
```

`hares-python` is excluded from `default-members` so plain `cargo build` /
`cargo test` does not require a Python interpreter. Build it explicitly with
`cargo build -p hares-python` or via `maturin develop`.

## License

BSD-3-Clause
