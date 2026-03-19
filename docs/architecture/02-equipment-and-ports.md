# Equipment & Port Model

## Equipment Trait

Every equipment model — built-in Rust or Python adapter — implements this trait:

```rust
pub trait Equipment: Send + Sync {
    /// Metadata: name, end-use category, zone, fuel type
    fn descriptor(&self) -> &EquipmentDescriptor;

    /// What ports this equipment writes to
    fn ports(&self) -> &[PortDeclaration];

    /// Initialize from config + initial environment
    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> Result<()>;

    /// Apply external control signal (reject invalid signals with Err)
    fn apply_control(&mut self, signal: &ControlSignal) -> Result<()>;

    /// Run internal control logic (thermostat, SOC tracking, etc.)
    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode;

    /// Run physics. Write results to preallocated port slots.
    fn step(&mut self, env: &EnvironmentState, dt: Duration, ports: &mut PortSlots);

    /// Current telemetry for output/observation
    fn telemetry(&self) -> Telemetry;

    /// State serialization for RL checkpointing
    fn save_state(&self) -> Vec<u8>;
    fn load_state(&mut self, state: &[u8]) -> Result<()>;
}
```

This is intentionally simpler than the previous architecture's trait. No coupling
requirement declarations, no multi-rate hooks, no preferred step rates. Those can be
added later if needed — but v1 runs everything sequentially at one timestep.

## Equipment Descriptor

```rust
pub struct EquipmentDescriptor {
    pub id: EquipmentId,          // unique instance ID (not just type name)
    pub name: String,             // user-facing name ("Battery #1", "PV South")
    pub end_use: EndUse,          // category for control routing
    pub equipment_type: &'static str,  // OCHRE name for compat ("Battery", "PV")
    pub zone: Option<ZoneId>,     // thermal zone (None for non-zonal equipment)
    pub fuel: Fuel,               // Electric, Gas, None
    pub stage: ExecutionStage,    // determines update order within timestep
    pub control_capabilities: ControlCapabilities,  // signals this equipment accepts
    pub telemetry_fields: Vec<TelemetryField>,       // fields this equipment reports
}

/// Fixed execution stages matching OCHRE's proven causality ordering.
/// v1 uses this explicit enum; a full dependency graph is a post-v1 generalization.
pub enum ExecutionStage {
    /// Scheduled loads, PV, event-based loads (no thermal feedback needed)
    Independent = 1,
    /// Battery, EV, Generator (electrical only, may need PV output)
    Electrical = 2,
    /// HVAC, Water Heater (thermal coupling — need accumulated loads)
    Thermal = 3,
    // Stage 4 (envelope solver + humidity) is not equipment — runs after all equipment
}
```

`control_capabilities` enables runtime validation of control signals before dispatch —
a `SOCTarget` sent to an HVAC unit is rejected with a clear error rather than silently
ignored. `telemetry_fields` makes equipment self-describing for observation space
construction (RL Gym) and automatic output column generation.

```rust
// Example: Battery accepts power setpoints, SOC targets, grid connect
pub fn battery_capabilities() -> ControlCapabilities {
    ControlCapabilities::POWER_SETPOINT | ControlCapabilities::SOC_TARGET
        | ControlCapabilities::GRID_CONNECT | ControlCapabilities::SELF_CONSUMPTION
}
```

### Multi-Instance Equipment

Unlike OCHRE, a dwelling can have multiple instances of the same equipment type:

```python
# OCHRE: one battery, one PV, period.
Equipment={"Battery": {"capacity_kwh": 10}, "PV": {"capacity": 5}}

# ochre_next native API: multiple instances
dwelling = Dwelling.from_hpxml("home.xml",
    equipment=[
        Battery(name="Garage Battery", capacity_kwh=10, chemistry="LFP"),
        Battery(name="Wall Battery", capacity_kwh=5, chemistry="NMC"),
        PV(name="South Roof", capacity_kw=5, tilt=30, azimuth=180),
        PV(name="West Roof", capacity_kw=3, tilt=25, azimuth=270),
        EV(name="Truck", capacity_kwh=100, max_charge_kw=19.2),
        EV(name="Sedan", capacity_kwh=60, max_charge_kw=7.7),
    ]
)

# Control signals target by instance name
dwelling.apply_control("Garage Battery", ControlSignal.power_setpoint(kw=3.0))
dwelling.apply_control("Wall Battery", ControlSignal.power_setpoint(kw=-2.0))
```

**OCHRE compat layer**: The compat API maps OCHRE's single-instance equipment dict to
the multi-instance model internally. `Equipment={"Battery": {...}}` creates one instance
named "Battery". Existing OCHRE scripts work unchanged.

## Equipment Owns Its State

Each instance owns all internal state. No shared mutable buffers. This enables safe
parallel fleet execution and trivial state serialization.

```rust
pub struct HeatPump {
    config: HeatPumpConfig,       // rated capacity, COP curves, speed config
    mode: HVACMode,               // Off, Heating, Cooling, Defrost
    speed_index: u8,              // current speed (multi-speed units)
    time_in_mode: Duration,       // for minimum on/off time enforcement
    defrost_accumulator: f64,     // defrost cycle tracking
}

pub struct BatteryStorage {
    config: BatteryConfig,
    soc: f64,                     // state of charge (0-1)
    capacity_kwh: f64,            // current usable capacity (degradation-adjusted)
    temperature_c: f64,           // cell temperature (if thermal model active)
    degradation: DegradationState,
    standby_power_w: f64,         // parasitic standby consumption
}

pub struct PVSystem {
    config: PVConfig,             // capacity, tilt, azimuth, efficiency curves
    // stateless — output depends only on weather + config
}
```

## Equipment Type Catalog

Matches OCHRE's equipment set plus fixes for known OCHRE bugs. Each type is a Rust
struct implementing `Equipment`.

| Category | Types | Key Physics |
|----------|-------|-------------|
| **HVAC Heating** | Gas/Electric Furnace, Boiler, Baseboard, ASHP, Mini-Split | Biquadratic EIR/capacity, SHR, defrost, duct losses, multi-speed |
| **HVAC Cooling** | AC, Room AC, ASHP Cooler, Mini-Split | Biquadratic, SHR via psychrometrics, crankcase heater |
| **Water Heating** | Resistance, Gas, HPWH, Tankless | Stratified tank (1-12 nodes), draw mixing, COP curves |
| **PV** | Fixed-tilt (multiple arrays) | PVWatts model, temperature derating, inverter limits |
| **Battery** | Li-ion (multiple instances) | SOC, internal resistance, OCV curves, self-discharge, standby power, degradation, thermal model |
| **EV** | PHEV, BEV (multiple instances) | Stochastic arrival/departure, SOC, charging curves. V2G is a **new capability** (OCHRE prohibits EV discharge) — scope to Phase 3+ |
| **Generator** | Gas generator, fuel cell | Efficiency curves, ramp limits |
| **Loads** | Scheduled, Event-based | Pre-computed profiles, stochastic events |

### Battery Improvements Over OCHRE

| Feature | OCHRE | ochre_next |
|---------|-------|-----------|
| Standby power | Not modeled (0W) | Configurable, default from chemistry LUT |
| Self-discharge | Implemented with 0.05%/day default (SAM Li-NMC) | Chemistry-aware defaults from PyBaMM LUTs |
| Cell parameters | SAM 2020.2.29 Li-NMC only | Multi-chemistry via SAM/PyBaMM adapters |
| Thermal coupling | Optional, not fed back to zone | Battery heat gain flows to zone via thermal port |
| Multiple instances | One battery per dwelling | Arbitrary number, independent control |
| Degradation | Rainflow + Arrhenius (daily) | Same algorithm, cleaner implementation |

### PV Improvements Over OCHRE

| Feature | OCHRE | ochre_next |
|---------|-------|-----------|
| Array count | Single array | Multiple arrays, independent orientation |
| SAM version | PySAM PVWatts v8 | SAM adapter, version-independent |

### HVAC Improvements Over OCHRE

See [Appendix](appendix-physics-improvements.md) for verified equations and coefficients.

| Feature | OCHRE | ochre_next |
|---------|-------|-----------|
| Thermostat deadband | Broken — inconsistent data sources, asymmetric hysteresis | Clean FSM with explicit priority stack (HPXML < schedule < control signal) |
| Supply air temperature | Hardcoded (105°F heating, 54-58°F cooling for all types) | Per-equipment-type defaults (130°F furnace, 90°F ASHP HP-only); ASHP varies with OAT |
| Airflow rate | Hardcoded 312 CFM/ton (below ACCA minimum 350) | 375 CFM/ton default, scales with HPXML `AirflowDefectRatio` |
| Multiple HVAC per zone | One heater + one cooler per zone | Multiple instances via port model |
| Capacity calculation | Computed twice per step (performance bug) | Single computation, cached |
| Staged backup heating | Not implemented | Planned for v1 |

### Other Equipment Fixes

| Feature | OCHRE | ochre_next |
|---------|-------|-----------|
| HPWH circuit parameters | Hardcoded, awaiting HPXML updates | Configurable via equipment params |
| Dehumidifier | Acknowledged gap, not implemented | DX dehumidifier with biquadratic curves (same kernel as HVAC), HPXML `Dehumidifier` element |
| Event-based load generation | `NotImplementedError` stubs | Stochastic event model (v1.1) |
| Generator CHP | Computes `power_chp` but never uses it (dead code) | Thermal port for waste heat, configurable `efficiency_thermal` |

## Port Types

Equipment declares typed ports. The engine accumulates contributions per domain.

```rust
pub enum PortContribution {
    Thermal {
        zone: ZoneId,
        sensible_gain_w: f64,
        latent_gain_w: f64,
    },
    Electrical {
        active_power_kw: f64,       // +consume, -generate
        reactive_power_kvar: f64,
    },
    Fuel {
        fuel_type: FuelType,        // NaturalGas, Propane
        consumption_w: f64,         // rate in watts (thermal)
    },
    Fluid {
        loop_id: LoopId,
        flow_rate_kg_s: f64,
        supply_temp_c: f64,
        return_temp_c: f64,
        fluid_type: FluidType,     // Water, Glycol, Refrigerant
    },
    Custom {
        domain_id: DomainId,       // u16 index, not String
        payload: [f64; 16],        // fixed-size, no heap allocation
    },
}
```

Ports are simple enum variants, not a complex type hierarchy. `Fluid` supports
ground-source heat pumps, hydronic heating, and solar thermal — equipment that
exchanges heat via a working fluid loop. `Custom` is an escape hatch for future
physics domains (CO2 concentration, wall vapor diffusion, multi-zone airflow)
without requiring enum changes. The 16-element `f64` payload is sized for
multi-zone airflow and ground loop parameters; domains needing more fields use
multiple `Custom` contributions with the same `domain_id`.

Adding a new built-in port type means adding a variant to this enum and a handler
in the resolution step — a one-file change, not an architectural overhaul.

### Port Accumulation

Each timestep:
1. Zero all accumulation buffers (one `memset` per zone/bus)
2. Equipment writes `PortContribution` values via `PortSlots`
3. After all equipment: sum thermal gains per zone, sum electrical per bus
4. Feed accumulated gains into envelope solver

```rust
pub struct PortSlots {
    pub thermal: Vec<ThermalAccumulator>,   // one per zone
    pub electrical: ElectricalAccumulator,  // single bus (v1)
    pub fuel: FuelAccumulator,
    pub fluid: Vec<FluidAccumulator>,      // one per loop (sized if hydronic present)
    pub custom: Vec<CustomAccumulator>,    // one per registered custom domain
}
```

No per-step heap allocation. Slots are preallocated at init and reused.

## Environment State

What equipment reads each step:

```rust
pub struct EnvironmentState {
    pub zones: Vec<ZoneState>,
    pub weather: WeatherState,
    pub grid: GridState,
    pub custom_domains: Vec<DomainUpdate>,  // indexed by DomainId (u16)
    pub current_time: DateTime<Utc>,
    pub time_res: Duration,
}

pub struct ZoneState {
    pub id: ZoneId,
    pub temperature_c: f64,
    pub humidity_ratio: f64,
    pub relative_humidity: f64,    // derived from humidity_ratio + temp
    pub wet_bulb_c: f64,           // derived via psychrometric functions
    pub volume_m3: f64,            // needed for moisture mass balance
}

pub struct WeatherState {
    pub outdoor_temp_c: f64,
    pub outdoor_humidity_ratio: f64,
    pub wind_speed_m_s: f64,
    pub ground_temp_c: f64,
    pub sky_temp_c: f64,           // needed for radiant exchange / night sky cooling
    pub pressure_kpa: f64,
    pub solar_irradiance: Vec<SurfaceIrradiance>,  // per surface orientation
}

pub struct GridState {
    pub voltage_pu: f64,           // from HELICS grid co-sim peer (1.0 in standalone)
    pub frequency_hz: f64,         // inert for v1; available for frequency-responsive loads
}
```

In sequential mode, equipment reads previous-step zone temps and current-step weather.
This matches OCHRE exactly. `GridState` receives `voltage_pu` from the HELICS grid
co-sim peer (or defaults to 1.0 in standalone mode) for ZIP voltage-dependent load
models. `frequency_hz` is available for future frequency-responsive equipment.
