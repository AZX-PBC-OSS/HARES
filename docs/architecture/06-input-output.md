# Input Validation & Output

## Input Validation

All validation runs at startup, before the first timestep.

### HPXML Validation

1. XSD schema validation against HPXML 4.0
2. Domain-specific range checks:

| Field | Valid Range | Action |
|-------|------------|--------|
| Conditioned floor area | 20–1000 m² | Error if out of range |
| Infiltration (ACH50) | 0.5–30 | Warn if extreme |
| HVAC capacity | 1–200 kBtu/h | Error if out of range |
| SEER2 / HSPF2 | 10–40 / 6–15 | Error if out of range |
| Water heater setpoint | 40–70°C | Warn if <49°C (Legionella) |
| Battery round-trip eff. | 0.70–0.99 | Error if <0.70 |
| PV array tilt | 0–90° | Warn if >60° |
| Window-to-wall ratio | 0.02–0.40 | Warn if >0.30 |

### EPW Validation

| Check | Criterion |
|-------|----------|
| Record count | 8760 or 8784 |
| Dry-bulb temperature | -60°C to 55°C |
| Dew-point ≤ Dry-bulb | Physical constraint |
| GHI | 0–1500 W/m² |
| Wind speed | 0–60 m/s |
| Pressure | 60–110 kPa | Altitude range for US sites |
| No gaps > 1 hour | Detect missing records |
| Location match | EPW lat/lon within 200 km of HPXML site | Catches common user error |

### Schedule CSV Validation

- Column names match expected OCHRE format: `{equipment} ({unit})`
- Temporal coverage matches simulation period
- No NaN values in required columns

### Compile-Time Units

The `uom` crate eliminates unit conversion bugs at zero runtime cost:

```rust
use uom::si::f64::*;
use uom::si::power::watt;
use uom::si::thermodynamic_temperature::degree_celsius;

fn hvac_electric_draw(heat_output: Power, cop: f64) -> Power {
    heat_output / cop  // compiler verifies dimensional correctness
}
```

Use `uom` for all physics-domain quantities at API boundaries and in equipment
implementations. Plain `f64` is acceptable in performance-critical inner loops
where units are documented in the enclosing function signature.

## Output Architecture

### OCHRE-Compatible Output

The primary output format matches OCHRE's existing output: a timeseries DataFrame
(via polars) with columns for each metric, exportable to CSV or Parquet.

```python
# Same column names as OCHRE
results = dwelling.simulate()
# results columns include:
#   "Total Electric Power (kW)"
#   "HVAC Heating Electric Power (kW)"
#   "Battery SOC (-)"
#   "Indoor Temperature (C)"
#   ... etc, depending on verbosity
```

### Verbosity Levels

Matches OCHRE's verbosity system:

| Level | Content | Typical Use |
|-------|---------|-------------|
| 0 | Total electric/gas power only | Fleet screening |
| 1 | Power by end-use | Load profiling |
| 2 | + Zone temperatures, unmet loads | Comfort analysis |
| 3 | + Equipment modes, setpoints, SOC | Control analysis |
| 4 | + Energy (kWh) per end-use | Billing/metering |
| 5 | + Reactive power, power factor | Grid studies |
| 6 | + Component loads per boundary | Envelope diagnostics |
| 7 | + Schedule inputs | Debugging |
| 8 | + Individual equipment details | Deep diagnostics |

### Output Formats

```python
# CSV (OCHRE default)
dwelling = Dwelling(..., output_to_parquet=False)

# Parquet (recommended for large datasets)
dwelling = Dwelling(..., output_to_parquet=True)
```

Both formats use the same column naming convention as OCHRE for compatibility with
existing analysis scripts.

### Streaming Output

A 1-year simulation at 1-min resolution produces 525,600 rows — materializing this
in memory before writing is wasteful. Results are written incrementally:

- Output accumulates in a bounded Arrow RecordBatch buffer (configurable chunk size,
  default 10,000 rows ≈ 1 week at 1-min resolution)
- When the buffer fills, it is flushed to disk (Parquet row group or CSV append)
- Buffer memory is reclaimed after each flush
- At finalization, any remaining rows are flushed and Parquet metadata is written

This means peak output memory is proportional to the chunk size, not the simulation
duration. A 1-year sim uses the same output memory as a 1-week sim.

```python
# Configuration
dwelling = Dwelling(...,
    output_to_parquet=True,      # or False for CSV append
    output_chunk_size=10000,     # rows per flush (default)
)
```

### Metrics Output

Summary metrics (annual energy, peak power, comfort hours) are computed in a
post-processing pass and written to a separate metrics CSV, matching OCHRE's
`Analysis.calculate_metrics()` output.

### Multi-Instance Output Columns

For multi-instance equipment, output columns include the instance name:

```
Battery #1 SOC (-)
Battery #1 Electric Power (kW)
Battery #2 SOC (-)
Battery #2 Electric Power (kW)
PV South Electric Power (kW)
PV West Electric Power (kW)
```

The OCHRE compat layer (single-instance) uses OCHRE's original column names without
instance qualifiers.

## Fleet Output

For fleet runs:

```python
fleet_results = fleet.simulate()
# Per-dwelling: one metrics row (always)
# Per-dwelling: periodic timeseries (configurable interval: 15-min, hourly)
# Fleet aggregate: weighted sum timeseries
```

Fleet timeseries use `sample_weight` for all aggregations. No unweighted statistics.
