# Grid Outage and Islanding Behaviour

This document defines HARES's outage/islanding semantics and the physically
correct behaviour of every electrical equipment family during a utility
outage. It generalizes the water-heater precedent
([water-heater.md — Grid Outage Behaviour](equipment/water-heater.md#grid-outage-behaviour))
to all equipment.

## Semantics

Two distinct concepts, both carried in `GridState` (`hares-types`):

| Concept | Field / method | Meaning |
|---------|----------------|---------|
| Utility outage | `voltage_pu == 0.0` (`GridState::grid_outage()`) | The distribution feeder is dead. Set externally: scenario config, `Dwelling::set_grid_voltage(0.0)`, `Fleet::set_grid_voltage*`. (HELICS voltage subscriptions reject values outside [0.5, 1.5] pu and hold the last valid value, so co-simulation outages are signalled through the explicit setter, not through the voltage topic.) |
| Bus energization | `GridState::bus_energized()` / `GridState::bus_voltage_pu()` | Whether the **home's bus** carries voltage. During an outage, an island-capable source holds the bus at nominal (`island_bus_voltage_pu == Some(1.0)`); otherwise the bus is dead. |

The dwelling resolves bus energization at the start of every timestep: if the
utility voltage is `0.0` and any equipment reports
`Equipment::island_source_available()`, the bus is islanded at 1.0 pu.
Availability is evaluated from end-of-previous-step equipment state, so
island formation/collapse takes effect with a one-step lag.

Island sources:

- **Battery** — grid-connected and dischargeable: SOC above its effective
  floor (`min_soc`, narrowed by any SOC-target window), hardware discharge
  capability, cell temperature above the discharge cutoff, DR not commanding
  a full shed.
- **Generator** — enabled (self-consumption control active, or an explicit
  positive setpoint). Fuel is modeled as unlimited (no on-site tank model).
- **EV** — plugged in at home **and actively discharging** (V2L/V2G). A
  merely plugged-in EV is not counted: most EVSEs cannot island a home, and
  HARES only dispatches EV discharge on explicit negative setpoints.
- **PV is never an island source** — it is a grid-following inverter
  (IEEE 1547) and cannot form a bus on its own.

Grid-forming capability (transfer switch / grid-forming inverter) is assumed
present whenever a source can deliver power; there is no separate opt-in
config flag yet. This is the minimal model that keeps battery-backed homes
from dropping their loads during outages (the VPP/resilience use case);
a per-equipment `backup_capable` flag is the natural extension point.

## The gating rule

**Loads gate on `bus_energized()`, never on raw `voltage_pu == 0.0`.**
Gating happens at the **control level** (the root of dispatch — the
water-heater precedent), so delivered energy ≡ metered energy by
construction: a gated element/compressor/fan produces no heat *and* no
draw, never "zero reported power with full delivered heat."

**Sources do not gate on the bus** — they are what keeps it energized.

Voltage-dependent physics (ZIP real-power scaling, reactive power) evaluates
at `bus_voltage_pu()`: islanded equipment sees nominal voltage; a dead bus
produces no power and no vars. The electrical solver
(`ElectricalSolver::effective_load_scale`) holds the ZIP scale at 1.0 on a
dead bus so any un-gated (leaked) load stays fully visible at the meter
instead of being ZIP-attenuated — the electrical-balance invariant and the
no-outage regression gate then catch it.

## Per-family behaviour

During a **dead-bus outage** (utility out, no island source). During
**islanded operation** every family below behaves exactly as in normal
operation (the bus is at nominal voltage).

| Family | During dead-bus outage | Notes |
|--------|------------------------|-------|
| Electric resistance WH | Elements off: no tank heat, 0 W | Tank evolves as if forced Off; hysteresis resumes on restoration |
| Heat pump WH | Compressor, backup element, standby parasitic off | Off-timer keeps advancing; min-off honoured at restoration |
| Electric tankless WH | Cannot fire: outlet = inlet, 0 W | Resumes on restoration |
| Gas / gas tankless WH | Burner + pilot keep firing; electric parasitics (draft fan, ignition controller) 0 W | Modelling simplification following OCHRE: gas water heating stays available |
| Indirect tank | Unaffected directly | No electrical port — but its boiler force-offs (below), so loop heat stops |
| Central AC / Room AC | Compressor + blower off; **crankcase heater 0 W** | Forced off before ModeOverride/DR handling; thermostat FSM resumes on restoration |
| ASHP / MSHP / GSHP / WSHP heater | Compressor, ER backup, blower, defrost, pan heater off | ER off-timer bookkeeping advances as for any forced off |
| ASHP / MSHP / GSHP cooler | As Central AC (shared cooling core); ground-loop pump off | Pump follows the compressor |
| Electric furnace / baseboard / boiler | Elements + blower/circulator off | |
| Gas furnace | **Off — no fuel burned** | The blower and burner controls are electric; unlike a gas WH, a forced-air furnace cannot deliver heat without power |
| Gas boiler | **Off — no fuel burned** | Burner controls and circulation pump are electric |
| Ideal HVAC | Off; solver ideal target cleared | |
| Dehumidifier | Off | Humidistat hysteresis resumes on restoration |
| Ventilation (HRV/ERV/exhaust) | Fans off: no airflow, no recovery, 0 W | |
| Scheduled loads (lighting, MELs, …) | 0 W, no gains | Gas scheduled loads are also zeroed — modern gas appliances need electricity (electronic ignition, controls) |
| Event loads / wet appliances | 0 W, no gains; event timers freeze | The interrupted cycle resumes when power returns; cycles are not re-scheduled |
| EV (charging) | Home charging + battery preconditioning stop; SOC holds; no vars (even on a commanded q-setpoint) | *Away* charging is off-site and unaffected by the home's outage |
| EV (V2L/V2G discharge) | **Not gated** — the EV is a source; while discharging it islands the home (one-step lag) | |
| PV | **Inverter trips** (IEEE 1547 anti-islanding): no AC, no DC extraction, no vars | Keeps producing when the home is islanded by a grid-forming source; panel thermal/soiling states evolve either way |
| Battery (charge) | Blocked — nothing on a dead bus to charge from | Standby electronics and cell heater also 0 W (they are AC loads) |
| Battery (discharge) | Never blocked by the bus — discharge capability *is* what energizes it | A dead bus with a battery present implies the battery is empty/cold/disconnected |
| Generator | **Runs** — an outage is precisely when it runs (self-consumption picks up the house load) | Never gated |

## Meter behaviour

- **Dead bus**: every load is force-gated, PV is tripped, battery/EV draw
  nothing → net grid power is exactly 0 kW. Any non-zero residual indicates
  an un-gated load (kept visible by design — see `effective_load_scale`).
- **Islanded**: loads run and sources serve them; the "grid" channel of the
  electrical solver reports the island's internal balance. HARES does **not**
  yet enforce zero flow at the service entrance during islanded operation:
  if commanded dispatch exceeds on-site source capability (e.g. an explicit
  battery charge setpoint beyond PV surplus, or load beyond
  `max_discharge_kw`), the residual appears as phantom grid import/export.
  Enforcing island power balance (unserved-energy accounting, source
  saturation) is intentionally out of scope of the outage gate and is the
  next extension point.

## Observability

- Rust: `EnvironmentState::grid` — `voltage_pu` (utility), `bus_voltage_pu()`,
  `islanded()`.
- Python (`Dwelling.get_env_dict()`): `grid_voltage_pu`, `grid_bus_voltage_pu`,
  `grid_islanded`.
- Python actors (`env["grid"]`): `voltage_pu`, `bus_voltage_pu`, `islanded`.

## Testing

Every family has a unit test following the water-heater pattern (off during a
dead-bus outage, unaffected when islanded, correct recovery after
restoration); source-type equipment have source-behaviour tests
(`island_source_available`, un-gated discharge/generation). Dwelling-level
integration tests live in `crates/hares-core/tests/grid_outage_islanding.rs`:
a battery-backed home keeps its loads through an outage, an unbacked (or
depleted-battery) home drops them and meters exactly 0 kW.
