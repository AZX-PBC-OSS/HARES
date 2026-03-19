# Data Ingestion & Fleet Execution

## Single-Building Input (OCHRE-Compatible)

The primary input model matches OCHRE: **HPXML 4.0 + schedule CSV + EPW weather file**.
These are the three files in a ResStock building bundle.

### Property Loading Chain

Matches OCHRE's loading sequence:

1. **HPXML parse** — Parse HPXML 4.0 XML into typed building description (envelope
   geometry, equipment specs, zones, boundaries)
2. **Schedule load** — Parse schedule CSV (occupancy, appliance events, setpoints) with
   zero-order hold resampling to simulation timestep
3. **Weather load** — Parse EPW file (TMY3 or AMY), extract temperature, humidity, wind,
   solar irradiance; compute per-surface solar irradiance
4. **Equipment instantiation** — Resolve equipment types from HPXML fuel/category,
   split heat pumps into heater + cooler instances (matching OCHRE's
   `update_equipment_properties()`)
5. **Parameter overrides** — Apply user kwargs (Equipment, Envelope, Occupancy dicts)
   via recursive deep merge (matching OCHRE's `nested_update()`)
6. **ZIP parameters** — Apply voltage-dependency parameters from defaults library

The Rust HPXML parser must produce identical equipment sets from the same HPXML input.
This is the primary validation criterion for OCHRE compatibility.

### OCHRE Equipment Name Registry

These names are part of the public API — OCHRE users reference them in control signals
and `Equipment={...}` kwargs:

| OCHRE Name | Category | Notes |
|------------|----------|-------|
| `Electric Furnace`, `Gas Furnace` | HVAC Heating | |
| `Electric Baseboard`, `Electric Boiler`, `Gas Boiler` | HVAC Heating | |
| `Heat Pump Heater`, `ASHP Heater`, `MSHP Heater` | HVAC Heating | |
| `Air Conditioner`, `Room AC` | HVAC Cooling | |
| `ASHP Cooler`, `MSHP Cooler` | HVAC Cooling | |
| `Electric Resistance Water Heater` | Water Heating | |
| `Heat Pump Water Heater`, `Gas Water Heater` | Water Heating | |
| `Tankless Water Heater`, `Gas Tankless Water Heater` | Water Heating | |
| `PV` | Solar | ochre_next: supports multiple instances |
| `Battery` | Storage | ochre_next: supports multiple instances |
| `EV`, `Electric Vehicle`, `Scheduled EV` | EV | ochre_next: supports multiple instances |
| `Gas Generator`, `Gas Fuel Cell` | Generator | |
| Lighting, appliance, MEL names | Loads | ~15 scheduled/event-based types |

**Heat pump splitting**: OCHRE splits a single HPXML "Air Source Heat Pump" into
`ASHP Heater` + `ASHP Cooler`. The Rust parser replicates this.

### Python API

```python
# OCHRE compat
from ochre_next.compat import Dwelling
dwelling = Dwelling(
    hpxml_file="home.xml",
    hpxml_schedule_file="schedules.csv",
    weather_file="weather.epw",
    start_time=datetime(2019, 5, 5, 12),
    time_res=timedelta(minutes=1),
    duration=timedelta(days=30),
    Equipment={"Battery": {"capacity_kwh": 10}},
)

# Native API
from ochre_next import Dwelling
dwelling = Dwelling.from_hpxml(
    "home.xml",
    schedule="schedules.csv",
    weather="weather.epw",
    equipment=[
        Battery(name="Main", capacity_kwh=10),
        PV(name="South", capacity_kw=5, azimuth=180),
    ],
)
```

## ResStock Dataset Support

### ResStock 2024/2025 Structure

```
resstock_tmy3_release_2/
  building_energy_models/
    <state>/
      up00-baseline/
        bldg<id>.zip          # HPXML + schedule CSV + EPW
  metadata/
    national/
      baseline/
        results_up00.parquet  # One row per sampled building
```

### Metadata Parquet

Each row = one sampled dwelling:
- `bldg_id: i64` — unique building identifier
- `upgrade: i64` — 0 = baseline, 1..N = upgrade scenario
- `sample_weight: f64` — number of real US dwelling units represented
- `in.*` columns — ~260 categorical building characteristics (strings)
- `out.*` columns — simulation output metrics

### Versioned Column Mapping

ResStock schema evolves across releases. Per-version `ColumnMapper` adapters handle
column name and semantics differences:

```rust
pub enum ResStockVersion { V2024_1, V2024_2, V2025_1 }

pub trait ColumnMapper: Send + Sync {
    fn bldg_id_col(&self) -> &str;
    fn sample_weight_col(&self) -> &str;
    fn map_characteristics(&self, row: &ArrowRow) -> Result<BuildingCharacteristics>;
}
```

ResStock version is **user-configured** — parquet files have no embedded version tag.
Range strings in `in.*` columns (`"1500-1999"`) are parsed to midpoints at ingestion.

### Per-Building Bundle

Each ZIP contains: `in.xml` (HPXML 4.0), `schedules.csv`, `<station>.epw`

### Weather File Support

The Rust EPW parser accepts any standard EPW file — the format is the same for both
TMY3 (Typical Meteorological Year) and AMY (Actual Meteorological Year) data. The
difference is the source dataset, not the file format:

| Type | Use Case | Record Count |
|------|----------|-------------|
| **TMY3** | Long-term average behavior, code compliance, fleet studies | 8760 |
| **AMY** | Historical event analysis, DR replay, specific-year RL training | 8760 or 8784 (leap) |

AMY files are important for studies that need real weather events (heat waves, polar
vortices) rather than smoothed typical years. The engine treats both identically —
the simulation period is bounded by the weather file's temporal coverage.

### ResStock Convenience Loaders (Python)

Python-side convenience functions fetch ResStock building bundles from the NREL OEDI
data lake on AWS S3, then hand the local files to the Rust engine. These loaders are
part of the `ochre_next` Python package, not the Rust core:

```python
from ochre_next.data import fetch_resstock_building, fetch_resstock_fleet

# Single building (downloads HPXML + schedule + EPW, returns local paths)
building = fetch_resstock_building(
    bldg_id=12345,
    upgrade_id=0,                    # 0 = baseline
    year="2024",
    release="resstock_tmy3_release_2",
    local_dir="./data/resstock",     # cache directory
)
# building.hpxml_path, building.schedule_path, building.weather_path

# Fleet (downloads metadata parquet + all building bundles for a filter)
fleet_data = fetch_resstock_fleet(
    metadata_path="results_up00.parquet",
    hpxml_dir="./data/resstock/building_energy_models",
    weather_dir="./data/weather/tmy3",
    filter={"in.state": "CO", "in.hvac_heating_type": "Heat Pump"},
    n_workers=8,                     # parallel downloads
)
```

These are pure Python (boto3/httpx for S3 access) and produce the same local file
layout that `Dwelling.from_hpxml()` and `Fleet.from_resstock()` expect. The Rust
engine never touches the network — it receives local file paths.

**AMY weather for ResStock buildings**: ResStock bundles include TMY3 weather by
default. For AMY studies, users provide an AMY EPW file via the `weather_file`
override parameter — the building geometry and schedules from ResStock are still
valid, only the weather changes.

### Ingestion

```rust
pub struct ResStockBuilding {
    pub bldg_id: i64,
    pub sample_weight: f64,
    pub hpxml_path: PathBuf,
    pub schedule_path: PathBuf,
    pub weather_path: PathBuf,
}
```

Parse the metadata parquet to get building IDs and weights, then load each building's
HPXML bundle. No clustering, no archetype tables, no SoA vectorization in v1. Each
building is an independent `Dwelling` instance.

### HPXML Version

Requires HPXML 4.0 (ResStock 2024+). HPXML 3.x zone adjacency semantics differ
materially — not worth supporting.

## Fleet Execution

### v1: Simple Parallel

Fleet mode = run N independent dwellings in parallel via rayon:

```rust
let results: Vec<DwellingResult> = buildings
    .par_iter()
    .map(|building| {
        let dwelling = Dwelling::from_resstock(building)?;
        dwelling.simulate()
    })
    .collect();
```

Each dwelling is fully independent — owns its state, schedule, weather data. No shared
mutable state. rayon work-stealing handles load balancing automatically.

Each dwelling is fully self-contained. Memory per dwelling depends on schedule size,
equipment count, and output verbosity — profiling will establish the actual footprint.
For fleets that exceed single-machine memory, chunk into batches or use distributed
execution (see below).

### Weight-Aware Aggregation

`sample_weight` propagates through the entire pipeline. All fleet aggregations multiply
by weight. No unweighted statistics.

```python
fleet = Fleet.from_resstock(
    metadata="results_up00.parquet",
    hpxml_dir="/data/resstock/building_energy_models",
    weather_dir="/data/weather/tmy3",
)
results = fleet.simulate(n_threads=8)  # rayon with thread cap

# Weighted aggregate
total_load = sum(r.timeseries["total_electric_kw"] * r.sample_weight for r in results)
```

### Fleet Output

```python
fleet_results = fleet.simulate()
# Per-dwelling: summary metrics row
# Per-dwelling: periodic timeseries (15-min or hourly, configurable)
# Fleet aggregate: weighted sum timeseries
```

### Distributed Fleet Execution (Dask)

For full ResStock national samples (550k+ buildings) or HPC workloads, fleet execution
is distributed via **Python-side sharding** — not by making ochre_next itself distributed.
ochre_next processes single-node batches; Dask (or similar) handles partitioning, scheduling,
and result aggregation:

```python
import dask.bag as db
from ochre_next import Dwelling

def simulate_building(building_record):
    dwelling = Dwelling.from_resstock(building_record)
    return dwelling.simulate()

# Shard buildings across Dask workers (local, SLURM, or cloud)
buildings = load_resstock_metadata("results_up00.parquet")
bag = db.from_sequence(buildings, npartitions=num_workers)
results = bag.map(simulate_building).compute()
```

Each Dask worker runs ochre_next with rayon parallelism on its local cores. This
separates concerns: ochre_next handles single-machine performance; Dask handles
distribution, fault tolerance, and scheduling.

For HPC (NREL Kestrel, AWS Batch): partition buildings into N independent job array
entries. Each job handles `total/N` buildings. Embarrassingly parallel — no inter-job
communication needed.

### Future Fleet Optimizations (Not v1)

The architecture doesn't block these, but they're not needed for v1:
- **SoA batching**: Group dwellings by topology key (RC state count, zone mask) and
  equipment signature for vectorized kernels. Buildings within a batch share array
  dimensions; only scalar parameters (R-values, COP curves, schedules) vary. ResStock
  produces 10–20 distinct topology classes nationally.
- **Archetype clustering**: Reduce simulation count for aggregate studies via k-prototype
  clustering (mixed categorical/numeric features). Destroys stochastic diversity needed
  for DR/RL — use knowingly as a compute optimization, not a fidelity choice.
- **Shared weather**: Deduplicate EPW data for buildings sharing a weather station
- **Streaming schedules/weather**: Load schedule and weather data via streaming iterators
  instead of fully materializing — avoids loading an entire year of 1-minute data per
  dwelling into memory simultaneously
