# External Tool Integration

## Design Philosophy

OCHRE's battery model uses hardcoded SAM 2020.2.29 Li-NMC parameters. Its PV model
depends on PySAM PVWatts. Rather than embedding specific tool versions, ochre_next uses
**Python adapter scripts** that generate lookup tables (LUTs) and parameter sets from
external tools. The Rust simulation core consumes these LUTs — it never calls SAM or
PyBaMM directly.

Adapters run **before simulation** (or once-and-cache). They are not called during the
timestep loop. The Rust core loads their output at initialization and interpolates
from tables at runtime.

This separation means:
- External tool versions can be upgraded without recompiling the Rust core
- LUTs are cached to disk — regenerated only when parameters change
- The Rust core stays fast (table interpolation, not Python library calls)
- Users can substitute their own parameter sources
- PySAM and PyBaMM remain optional Python dependencies, not build requirements

## SAM Integration (PV + Battery Parameters)

### PV: SAM PVWatts Adapter

OCHRE pre-computes PV generation by running a full SAM annual simulation at
initialization (`PV.initialize_schedule()`), then reads the result as a schedule each
timestep. ochre_next replaces this with a **cached, reusable LUT** that can be
regenerated independently of the simulation:

```python
# Python adapter (runs once before simulation, or cached)
from ochre_next.adapters import sam_pv

lut = sam_pv.generate_pv_lut(
    system_capacity_kw=5.0,
    tilt=30,
    azimuth=180,
    module_type=0,        # Standard
    array_type=0,         # Fixed roof mount
    weather_file="weather.epw",
)
lut.save("pv_south_lut.parquet")  # cached for reuse
```

The LUT maps `(month, hour, ghi, dni, dhi, temp_c)` → `ac_power_kw`. The Rust core
does multi-dimensional interpolation from this table each timestep.

**Alternative**: For simpler cases, use the PVWatts equations directly in Rust (capacity
× irradiance × temperature derating × inverter efficiency). This avoids the LUT entirely
and matches OCHRE's simplified PV path.

### Battery: SAM Cell Parameters

SAM provides current cell parameters for various chemistries. The adapter extracts
these into a parameter file:

```python
from ochre_next.adapters import sam_battery

params = sam_battery.extract_cell_params(
    chemistry="LFP",       # or "NMC", "NCA", "LTO"
    capacity_kwh=10,
    source="SAM",          # uses current SAM version
)
params.save("battery_lfp_params.toml")
```

Output TOML consumed by Rust:
```toml
[cell]
chemistry = "LFP"
v_nominal = 3.2
ah_rated = 50.0
r_internal_ohm = 0.003
n_series = 14
n_parallel = 4

[soc_ocv]  # OCV curve (SOC → voltage)
soc =  [0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0]
v_oc = [2.5, 3.0, 3.1, 3.15, 3.2, 3.22, 3.25, 3.28, 3.3, 3.35, 3.6]

[thermal]
r_thermal_k_per_w = 0.5
c_thermal_j_per_k = 90000

[losses]
standby_power_w = 5.0          # parasitic standby consumption
self_discharge_pct_per_day = 0.05
inverter_efficiency = 0.97

[degradation]
model = "rainflow_arrhenius"   # same as OCHRE
# ... degradation coefficients
```

## PyBaMM Integration (Advanced Battery Modeling)

PyBaMM (Python Battery Mathematical Modelling) provides physics-based electrochemical
models that can generate more accurate efficiency and degradation lookup tables than
SAM's simplified models.

### Use Case

For studies where battery behavior is critical (RL-optimized dispatch, degradation-aware
control, second-life battery analysis), PyBaMM can generate:

1. **Efficiency LUTs**: `(SOC, power_kw, temperature_c)` → `efficiency`
2. **Degradation rate tables**: `(DOD, temperature_c, C_rate)` → `capacity_fade_per_cycle`
3. **OCV curves**: Per-chemistry, per-age voltage profiles

```python
from ochre_next.adapters import pybamm_battery

# Generate efficiency LUT from PyBaMM SPM model
efficiency_lut = pybamm_battery.generate_efficiency_lut(
    chemistry="NMC",
    capacity_ah=50,
    n_series=14,
    n_parallel=4,
    temperature_range_c=(0, 45),
    soc_range=(0.1, 0.95),
    power_range_kw=(-5, 5),
    age_cycles=0,  # or specify degraded state
)
efficiency_lut.save("battery_efficiency_lut.parquet")

# Generate degradation coefficients
degradation = pybamm_battery.generate_degradation_params(
    chemistry="NMC",
    capacity_ah=50,
    temperature_c=25,
)
degradation.save("battery_degradation.toml")
```

### Fallback

PyBaMM is optional. If not installed, the battery model uses:
1. SAM-generated parameters (if available)
2. Built-in defaults matching OCHRE's current Li-NMC parameters

## Python Equipment Adapters

For custom equipment models that are too complex or specialized to implement in Rust,
users can write Python equipment that plugs into the simulation:

```python
from ochre_next import PythonEquipment, ControlSignal, PortContribution

class CustomHeatPump(PythonEquipment):
    """Custom heat pump model using manufacturer-specific curves."""

    def __init__(self, config):
        super().__init__(
            name="Custom HP",
            end_use="HVAC Heating",
            zone="indoor",
        )
        self.cop_table = load_manufacturer_data(config["data_file"])
        self.capacity_kw = config["rated_capacity_kw"]

    def step(self, env, dt_seconds):
        cop = self.cop_table.interpolate(
            env.outdoor_temp_c, env.zone("indoor").temperature_c
        )
        heat_output_w = self.capacity_kw * 1000
        electric_input_w = heat_output_w / cop

        return [
            PortContribution.thermal(
                zone="indoor",
                sensible_gain_w=heat_output_w,
            ),
            PortContribution.electrical(
                active_power_kw=electric_input_w / 1000,
            ),
        ]
```

Python equipment runs via PyO3 callback. Each step: Rust calls Python (acquires GIL),
Python returns port contributions, Rust continues. Overhead is ~1-5µs per step per
Python equipment — acceptable for single-building and small fleet studies.

**For fleet-scale performance**: Convert Python equipment to Rust. Python equipment is
a prototyping and customization path, not the fleet-scale execution path.

## Custom Rust Equipment

For performance-critical custom models, implement the `Equipment` trait directly:

```rust
use ochre_next::{Equipment, EquipmentConfig, EnvironmentState, ControlSignal, PortSlots};

pub struct GroundSourceHeatPump {
    config: GSHPConfig,
    mode: HVACMode,
    ground_loop_temp_c: f64,
    // ...
}

impl Equipment for GroundSourceHeatPump {
    fn step(&mut self, env: &EnvironmentState, dt: Duration, ports: &mut PortSlots) {
        // Physics implementation
        let cop = self.calculate_cop(env.outdoor_temp_c, self.ground_loop_temp_c);
        // ...
        ports.thermal[self.zone].sensible_gain_w += heat_output;
        ports.electrical.active_power_kw += electric_input;
    }
    // ... other trait methods
}
```

Register custom equipment with the engine:

```rust
registry.register("Ground Source Heat Pump", GroundSourceHeatPump::factory());
```

Or from Python via PyO3:

```python
from ochre_next import Dwelling
# Custom Rust equipment compiled as a separate crate, loaded as a Python extension
from my_custom_equipment import GroundSourceHeatPump

dwelling = Dwelling.from_hpxml("home.xml",
    equipment=[GroundSourceHeatPump(rated_capacity_kw=12, loop_length_m=150)],
)
```
