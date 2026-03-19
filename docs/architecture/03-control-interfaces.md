# Control & External Interfaces

## Control Signal Design

All external control — Python controllers, RL agents, HELICS peers, future protocol
adapters — maps to typed `ControlSignal` values delivered to equipment by instance ID.

```rust
pub enum ControlSignal {
    /// Temperature setpoint (HVAC, Water Heater)
    ThermalSetpoint {
        heating_setpoint_c: Option<f64>,
        cooling_setpoint_c: Option<f64>,
        deadband_c: Option<f64>,
    },
    /// Direct power setpoint (Battery, EV, Generator)
    PowerSetpoint {
        active_power_kw: f64,
        reactive_power_kvar: Option<f64>,
    },
    /// Power limit / curtailment (PV, Battery, EV)
    PowerLimit {
        max_power_kw: f64,
        ramp_rate_kw_per_s: Option<f64>,  // gradual curtailment (IEEE 2030.5, SunSpec)
    },
    /// SOC target with bounds (Battery, EV)
    SOCTarget {
        target_soc: f64,
        min_soc: Option<f64>,
        max_soc: Option<f64>,
    },
    /// Operating mode override (HVAC, all)
    ModeOverride {
        mode: OperatingMode,
    },
    /// Duty cycle control (Water Heater, HVAC)
    DutyCycle {
        on_fraction: f64,
        period_s: Option<f64>,            // cycle period (default: timestep)
    },
    /// Load scaling (Scheduled loads)
    LoadFraction {
        fraction: f64,
    },
    /// Connect/disconnect from grid
    GridConnect {
        connected: bool,
    },
    /// Self-consumption mode (Battery)
    SelfConsumption {
        enabled: bool,
        solar_only_charging: bool,
    },
    /// Demand response event (OpenADR, CTA-2045 commodity events)
    DemandResponse {
        level: DRLevel,               // Normal, Moderate, High, Critical, GridEmergency
        duration_s: Option<f64>,
    },
    /// Protocol-native envelope for features that don't map to core primitives.
    /// Heap-allocates — acceptable because these are infrequent (DR events, curve
    /// updates), not per-timestep.
    ProtocolNative {
        protocol: ProtocolId,
        payload: Vec<u8>,
    },
}
```

These core primitives plus `DemandResponse` and `ProtocolNative` cover OCHRE's existing
control vocabulary, common DR/grid-interactive patterns, and an escape hatch for
protocol-specific semantics (OCPP charging profile stacks, IEEE 2030.5 volt-var curves,
OpenADR price signals). The enum is designed to extend cleanly when protocol adapters
are added in later phases.

### Mapping to Future Protocol Adapters

The control signal taxonomy is designed so future protocol adapters can map protocol
commands to `ControlSignal` values without changing the core:

| Protocol | Maps To | Notes |
|----------|---------|-------|
| **OpenADR 3.0** | `SIMPLE` → DemandResponse, `LOAD_DISPATCH` → PowerSetpoint/PowerLimit, `CHARGE_STATE_SETPOINT` → SOCTarget | Price signals (`ELECTRICITY_PRICE`, `GHG`) feed controller layer, not ControlSignal directly |
| **CTA-2045** | ModeOverride, DutyCycle, GridConnect | Commodity events map to mode overrides |
| **IEEE 2030.5** | PowerSetpoint, PowerLimit, SOCTarget | DER function sets map directly |
| **OCPP 2.0.1** | PowerSetpoint, SOCTarget, PowerLimit | Charging profiles → power setpoints |

Protocol adapters are **not in v1 scope**. But the control signal design doesn't need
rewriting when they arrive — each adapter is a translation layer from protocol messages
to `ControlSignal` values. See [Appendix](appendix-physics-improvements.md) §6 for
detailed OpenADR 3.0 signal mapping and the `openleadr-rs` Rust crate.

**Note**: Price/tariff signals (OpenADR `ELECTRICITY_PRICE`, `EXPORT_PRICE`, `GHG`)
are not `ControlSignal` variants — they're inputs to a controller that *produces*
`ControlSignal` values. The Dwelling API should expose a price signal channel separate
from the control signal dispatch path.

## Control Signal Routing

Control signals target equipment by **instance name** (multi-instance) or **end-use
category** (OCHRE compat):

```python
# Native API: target by instance name
dwelling.apply_control("Garage Battery", ControlSignal.power_setpoint(kw=3.0))
dwelling.apply_control("South Roof PV", ControlSignal.power_limit(max_kw=2.0))

# OCHRE compat: target by end-use (routes to all instances of that type)
control_signal = {
    "HVAC Heating": {"Setpoint Temperature (C)": 21.0},
    "Battery": {"P Setpoint": 2.0},
}
dwelling.update_model(control_signal)
```

The compat layer maps OCHRE's string-keyed dict signals to typed `ControlSignal` values:

| OCHRE Key | ControlSignal Variant |
|-----------|----------------------|
| `Setpoint Temperature (C)` | `ThermalSetpoint { heating_setpoint_c }` |
| `P Setpoint` (kW) | `PowerSetpoint { active_power_kw }` |
| `Duty Cycle` | `DutyCycle { on_fraction }` |
| `Load Fraction` | `LoadFraction { fraction }` |
| `SOC` | `SOCTarget { target_soc }` |
| `Min SOC`, `Max SOC` | `SOCTarget { min_soc, max_soc }` |
| `Self Consumption Mode` | `SelfConsumption { enabled }` |

## Custom Python Controllers

The most common extension point. Write controller logic in Python; simulation runs in
Rust:

```python
from ochre_next import Dwelling, ControlSignal

dwelling = Dwelling.from_hpxml("home.xml", schedule="schedules.csv", weather="weather.epw")
dwelling.initialize()

for t in dwelling.timesteps():
    # Read state
    telemetry = dwelling.telemetry()
    indoor_temp = telemetry.zone("indoor").temperature_c
    battery_soc = telemetry.equipment("Garage Battery").soc
    pv_gen = telemetry.equipment("South Roof PV").generation_kw

    # Custom control logic
    if pv_gen > 3.0 and battery_soc < 0.9:
        dwelling.apply_control("Garage Battery",
            ControlSignal.power_setpoint(kw=min(pv_gen - 1.0, 5.0)))

    dwelling.step()

results = dwelling.results()  # → polars DataFrame
```

Each `dwelling.step()` releases the GIL, runs the full Rust timestep, and returns.
Python overhead per step is minimal (PyO3 call + control signal marshaling).

## RL Gymnasium Interface

```python
from ochre_next import Dwelling, DwellingGymEnv

dwelling = Dwelling.from_hpxml("home.xml", schedule="schedules.csv", weather="weather.epw")

env = DwellingGymEnv(
    dwelling=dwelling,
    observation_fields=[
        "indoor_temp_c", "outdoor_temp_c", "battery_soc",
        "pv_generation_kw", "total_electric_kw",
    ],
    action_space_config={
        "HVAC": ["setpoint_c"],          # continuous: [18.0, 26.0]
        "Battery": ["p_setpoint_kw"],     # continuous: [-5.0, 5.0]
    },
    reward_fn=my_reward_function,
    episode_length=timedelta(days=1),
)

obs, info = env.reset(seed=42)
for _ in range(1440):  # 1-day episode at 1-min resolution
    action = agent.predict(obs)
    obs, reward, terminated, truncated, info = env.step(action)
```

### Requirements

- **Deterministic reset**: `env.reset(seed=N)` produces identical initial state
- **Fast save/load**: Equipment `save_state()`/`load_state()` for RL exploration
- **Vectorized environments**: `VecDwellingGymEnv` runs N environments in parallel
  using rayon (one dwelling per thread, GIL released). Compatible with SB3 `DummyVecEnv`
- **Observations as numpy arrays**: Contiguous f64, zero-copy where possible

### Vectorized Environment

```python
from ochre_next import VecDwellingGymEnv

# N independent environments, parallelized inside Rust
env = VecDwellingGymEnv(
    dwelling_configs=[config1, config2, ...],  # N configs
    observation_fields=[...],
    action_space_config={...},
    reward_fn=reward,
)
# Single step() call processes all N environments (GIL released)
obs, rewards, dones, truncs, infos = env.step(actions)  # actions: (N, action_dim)
```

**Not compatible with SB3 `SubprocVecEnv`** — forking with an active rayon pool causes
deadlocks. Use `DummyVecEnv` wrapper or the native `VecDwellingGymEnv`.

## HELICS Co-Simulation

### Architecture

Python-orchestrated HELICS, matching the proven NREL pattern from OCHRE's existing
`run_cosimulation.py`. The Rust engine handles physics; Python handles HELICS federation:

```
┌──────────────┐    ┌──────────────┐    ┌──────────────┐
│  Grid Model  │    │  ochre_next  │    │  Aggregator  │
│  (OpenDSS)   │◄──►│  (Rust+Py)   │◄──►│  (Python)    │
│              │    │              │    │              │
│  pub: V      │    │  pub: P, Q   │    │  pub: ctrl   │
│  sub: P, Q   │    │  sub: V, ctrl│    │  sub: P, Q   │
└──────────────┘    └──────────────┘    └──────────────┘
        └───────────────┴────────────────┘
                    HELICS Broker
```

### Integration Pattern

Same as OCHRE's existing co-sim, with typed control signals:

```python
import helics as h
from ochre_next import Dwelling

# Setup HELICS federate
fedinfo = h.helicsCreateFederateInfo()
h.helicsFederateInfoSetCoreTypeFromString(fedinfo, "zmq")
fed = h.helicsCreateValueFederate("dwelling_1", fedinfo)

# Register pubs/subs
pub_power = h.helicsFederateRegisterGlobalPublication(fed, "house1/power", h.HELICS_DATA_TYPE_DOUBLE)
sub_voltage = h.helicsFederateRegisterSubscription(fed, "grid/voltage_pu", "")
sub_control = h.helicsFederateRegisterSubscription(fed, "aggregator/control", "")

# Dwelling
dwelling = Dwelling.from_hpxml("home.xml", schedule="schedules.csv", weather="weather.epw")
dwelling.initialize()

h.helicsFederateEnterExecutingMode(fed)

for t in dwelling.timesteps():
    h.helicsFederateRequestTime(fed, t.timestamp())

    # Receive voltage from grid model → populates GridState.voltage_pu
    if h.helicsInputIsUpdated(sub_voltage):
        dwelling.set_grid_voltage(h.helicsInputGetDouble(sub_voltage))

    # Receive control from aggregator
    if h.helicsInputIsUpdated(sub_control):
        ctrl = json.loads(h.helicsInputGetString(sub_control))
        for equip_name, signal in ctrl.items():
            dwelling.apply_control(equip_name, ControlSignal.from_dict(signal))

    dwelling.step()

    # Publish state
    h.helicsPublicationPublishDouble(pub_power, dwelling.telemetry().total_electric_kw)

h.helicsFederateFinalize(fed)
```

### Fleet HELICS

For large fleet co-sim (100+ dwellings), run all dwellings in a single Python process
using rayon-parallel `Fleet.step()`. The fleet is a single HELICS federate publishing
aggregate power:

```python
fleet = Fleet.from_resstock(metadata="results_up00.parquet", ...)
# One federate, aggregate pub/sub
# Internal: rayon par_iter over dwellings per step
```

This avoids HELICS broker congestion from one-federate-per-dwelling at scale.
