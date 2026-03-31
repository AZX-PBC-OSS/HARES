# Review Ticket Index — Systematic OCHRE Parity Investigation

**Total: 130 investigation tickets across 16 series**

See [WORKFLOW.md](WORKFLOW.md) for agent instructions, priority order, and mandatory
test coverage requirements.

---

## AR — Architecture Review (10 tickets) — **PRIORITY: FIRST**

Front-loaded architectural assessment of type safety, magic strings, premature
aggregation, and wiring correctness. These findings inform whether structural fixes
are needed before the per-equipment investigations proceed.

| ID | Title |
|----|-------|
| [AR-001](AR-001.md) | Telemetry key system: magic strings → typed constants or enum |
| [AR-002](AR-002.md) | Config key system: magic strings in EquipmentSpec.parameters |
| [AR-003](AR-003.md) | Port contribution wiring: zone/category correctness |
| [AR-004](AR-004.md) | Unit handling: raw f64s for values that have units |
| [AR-005](AR-005.md) | Equipment registration and wiring validation |
| [AR-006](AR-006.md) | Thermal energy accounting: double-counting and lost energy |
| [AR-007](AR-007.md) | EquipmentDescriptor and EndUse wiring consistency |
| [AR-008](AR-008.md) | Output column naming and energy disaggregation debugging |
| [AR-009](AR-009.md) | **Audit all ignored/failing tests — unignore and fix every one** |
| [AR-010](AR-010.md) | hares-control dispatch layer: routing, priority, broadcasting |

## TS — Thermal Solver Init & Physics (11 tickets) — **PRIORITY: SECOND**

Thermal solver initialization, derived temperatures (mains water, ground), foundation
coupling, thermal mass behavior, infiltration model, and numerical stability.

| ID | Title |
|----|-------|
| [TS-001](TS-001.md) | Thermal solver initialization and steady-state initial conditions |
| [TS-002](TS-002.md) | Mains water temperature model (Burch-Christensen) |
| [TS-003](TS-003.md) | Ground temperature model (Kusuda-Achenbach) |
| [TS-004](TS-004.md) | Foundation zone thermal model: ground coupling, mass, air exchange |
| [TS-005](TS-005.md) | Zone air vs thermal mass: furniture/wall mass behavior |
| [TS-006](TS-006.md) | Infiltration model and ventilation interaction |
| [TS-007](TS-007.md) | Crank-Nicolson stepping and numerical stability |
| [TS-008](TS-008.md) | Solar distribution model audit |
| [TS-009](TS-009.md) | Interior/exterior longwave radiation models |
| [TS-010](TS-010.md) | Humidity solver: moisture balance, buffering, latent heat |
| [TS-011](TS-011.md) | Attic zone thermal model |

## PS — Python Safety (6 tickets)

Replace brittle dict/kwargs patterns with strong typing. Validate inputs at boundaries.
Ensure Python users get clear errors, IDE autocomplete, and type checking.

| ID | Title |
|----|-------|
| [PS-001](PS-001.md) | DwellingConfig: replace **kwargs with typed fields |
| [PS-002](PS-002.md) | ControlSignal: dict-based construction safety |
| [PS-003](PS-003.md) | Actor.decide() env dict → typed EnvironmentView |
| [PS-004](PS-004.md) | Type stubs (_hares.pyi) completeness and accuracy |
| [PS-005](PS-005.md) | Equipment construction parameter validation |
| [PS-006](PS-006.md) | Dwelling.step()/simulate() error handling and type safety |

---

## DT — Datetime Consistency (12 tickets)

Traces local-time handling per equipment and actor. Wrong timezone shifts schedules,
setpoints, and event timing.

| ID | Title | Files |
|----|-------|-------|
| [DT-001](DT-001.md) | ScheduledLoad time-of-day evaluation | scheduled_load.rs, schedule.rs |
| [DT-002](DT-002.md) | EventBasedLoad event timing | event_load.rs |
| [DT-003](DT-003.md) | HVAC thermostat schedule evaluation | thermostat.rs, hvac_core.rs |
| [DT-004](DT-004.md) | IdealThermostat actor setpoint switching | ideal_thermostat.rs |
| [DT-005](DT-005.md) | EvDriverActor departure/arrival hour | ev_driver/ |
| [DT-006](DT-006.md) | Occupant actor presence schedule | occupant.rs |
| [DT-007](DT-007.md) | BatteryManagementActor TOU windows | bms.rs |
| [DT-008](DT-008.md) | DrCompliance actor DR event timing | dr_compliance.rs |
| [DT-009](DT-009.md) | SimulationConfig.start_time Python→Rust propagation | py_config.rs, utils.rs |
| [DT-010](DT-010.md) | EnvironmentManager schedule domain indexing | environment.rs |
| [DT-011](DT-011.md) | DST spring-forward/fall-back schedule impact |
| [DT-012](DT-012.md) | Tariff evaluator TOU local-time indexing |

## WO — Weather/Schedule Offsets (4 tickets)

Verifies EPW midpoint shift, schedule unit conversion, capacity fraction wiring, and
resampling methods.

| ID | Title | Files |
|----|-------|-------|
| [WO-001](WO-001.md) | EPW 30-minute midpoint offset | epw.rs, environment.rs |
| [WO-002](WO-002.md) | Schedule fraction→physical unit conversion | schedule.rs, resolve_loads.rs |
| [WO-003](WO-003.md) | HVAC ext_capacity_frac wiring | hvac_core.rs, ideal_hvac.rs |
| [WO-004](WO-004.md) | Weather resampling methods per field | weather.rs |

## UC — HPXML Unit Conversion Audit (10 tickets)

Audits imperial→SI conversion for every numeric HPXML field category.

| ID | Title | Files |
|----|-------|-------|
| [UC-001](UC-001.md) | HVAC capacity: BTU/hr→W | resolve_hvac.rs |
| [UC-002](UC-002.md) | HVAC efficiency: SEER/EER/HSPF/AFUE→EIR | resolve_hvac.rs |
| [UC-003](UC-003.md) | Duct fields: leakage %, R-value, area | resolve_hvac.rs, ashrae152.rs |
| [UC-004](UC-004.md) | Water heater: volume, capacity, EF/UEF→UA | resolve_water_heater.rs |
| [UC-005](UC-005.md) | Envelope: area, U-factor, R-value | building.rs |
| [UC-006](UC-006.md) | Infiltration ELA: in²→cm² | building.rs, infiltration.rs |
| [UC-007](UC-007.md) | Window: U-factor, SHGC, shading | building.rs |
| [UC-008](UC-008.md) | Foundation wall/slab: height, R-value, area | building.rs |
| [UC-009](UC-009.md) | PV: capacity, tilt (pitch→degrees), losses | resolve_der.rs |
| [UC-010](UC-010.md) | Temperature fields: all °F→°C | resolve_hvac.rs, resolve_water_heater.rs |

## CW — Config Wiring Per Equipment (22 tickets)

Traces every config field OCHRE extracts from HPXML for each equipment type and verifies
HARES extracts and wires the same fields.

| ID | Title | OCHRE Class |
|----|-------|-------------|
| [CW-001](CW-001.md) | Gas furnace | GasFurnace |
| [CW-002](CW-002.md) | Electric furnace | ElectricFurnace |
| [CW-003](CW-003.md) | Baseboard | ElectricBaseboard |
| [CW-004](CW-004.md) | Boiler | GasBoiler/ElectricBoiler |
| [CW-005](CW-005.md) | Central AC | AirConditioner |
| [CW-006](CW-006.md) | Room AC | RoomAC |
| [CW-007](CW-007.md) | ASHP heater | ASHPHeater |
| [CW-008](CW-008.md) | ASHP cooler | ASHPCooler |
| [CW-009](CW-009.md) | Mini-split heater | MinisplitASHPHeater |
| [CW-010](CW-010.md) | Mini-split cooler | MinisplitASHPCooler |
| [CW-011](CW-011.md) | Electric resistance WH | ElectricResistanceWaterHeater |
| [CW-012](CW-012.md) | Gas WH | GasWaterHeater |
| [CW-013](CW-013.md) | Heat pump WH | HeatPumpWaterHeater |
| [CW-014](CW-014.md) | Tankless WH | TanklessWaterHeater |
| [CW-015](CW-015.md) | PV | PV |
| [CW-016](CW-016.md) | Battery | Battery |
| [CW-017](CW-017.md) | Lighting/MELs/plug loads | ScheduledLoad |
| [CW-018](CW-018.md) | Ventilation (HRV/ERV/exhaust) | ventilation fan |
| [CW-019](CW-019.md) | Dehumidifier | dehumidifier |
| [CW-020](CW-020.md) | Wet appliances (washer/dryer/dishwasher) | EventBasedLoad |
| [CW-021](CW-021.md) | EV config wiring from HPXML | EV |
| [CW-022](CW-022.md) | Generator config wiring from HPXML | Generator |

## IC — Ideal Capacity (7 tickets)

Verifies ideal heating/cooling mode switching, formulas, and clipping for HVAC and
water heaters.

| ID | Title |
|----|-------|
| [IC-001](IC-001.md) | HVAC ideal capacity auto-selection logic |
| [IC-002](IC-002.md) | HVAC ideal capacity formula (fan power, SHR) |
| [IC-003](IC-003.md) | HVAC ideal capacity clipping (min/max bounds) |
| [IC-004](IC-004.md) | Resistance WH ideal capacity (upper/lower split) |
| [IC-005](IC-005.md) | HPWH ideal capacity (HP vs ER split) |
| [IC-006](IC-006.md) | Gas WH ideal capacity |
| [IC-007](IC-007.md) | Tankless WH ideal mode behavior |

## TP — Telemetry/Port Output (9 tickets)

Audits output field names and thermal category tagging per equipment type.

| ID | Title |
|----|-------|
| [TP-001](TP-001.md) | HVAC heating telemetry fields |
| [TP-002](TP-002.md) | HVAC cooling telemetry fields |
| [TP-003](TP-003.md) | Water heater telemetry fields |
| [TP-004](TP-004.md) | Battery telemetry fields |
| [TP-005](TP-005.md) | PV telemetry fields |
| [TP-006](TP-006.md) | EV telemetry fields |
| [TP-007](TP-007.md) | Scheduled/event load telemetry fields |
| [TP-008](TP-008.md) | Envelope/dwelling-level output columns |
| [TP-009](TP-009.md) | ThermalCategory correctness per equipment |

## DL — Duct Losses (4 tickets)

| ID | Title |
|----|-------|
| [DL-001](DL-001.md) | ASHRAE 152 DSE calculation vs OCHRE |
| [DL-002](DL-002.md) | Zone fraction distribution (conditioned/duct/basement) |
| [DL-003](DL-003.md) | ASHRAE 152 zone temperature lookup |
| [DL-004](DL-004.md) | Duct loss reporting formula |

## FP — Fan Power (3 tickets)

| ID | Title |
|----|-------|
| [FP-001](FP-001.md) | HVAC fan power model |
| [FP-002](FP-002.md) | Ideal HVAC fan power scaling |
| [FP-003](FP-003.md) | Fan power in electrical + thermal ports |

## EG — Envelope Geometry (7 tickets)

| ID | Title |
|----|-------|
| [EG-001](EG-001.md) | Ceiling height derivation |
| [EG-002](EG-002.md) | Attic height from gable area + roof tilt |
| [EG-003](EG-003.md) | Foundation height from HPXML |
| [EG-004](EG-004.md) | Garage geometry back-calculation |
| [EG-005](EG-005.md) | Floor area derivation |
| [EG-006](EG-006.md) | Attic volume formula |
| [EG-007](EG-007.md) | Interior wall + furniture area fractions |

## DC — Duty Cycles / Fixed Schedules (8 tickets)

| ID | Title |
|----|-------|
| [DC-001](DC-001.md) | OCHRE mode priority / duty cycle counters |
| [DC-002](DC-002.md) | HVAC duty cycle (off/on/partial) |
| [DC-003](DC-003.md) | WH duty cycle (upper/lower ER split) |
| [DC-004](DC-004.md) | Ventilation schedule independence |
| [DC-005](DC-005.md) | Dehumidifier humidity-based activation |
| [DC-006](DC-006.md) | Refrigerator/freezer fixed duty cycle |
| [DC-007](DC-007.md) | Wet appliance event schedule |
| [DC-008](DC-008.md) | Compressor lockout timer defaults mismatch |

## PA — Python API Gaps (8 tickets)

| ID | Title |
|----|-------|
| [PA-001](PA-001.md) | Telemetry missing timestep_index/current_time |
| [PA-002](PA-002.md) | Battery/EV LUT write-only (no getters) |
| [PA-003](PA-003.md) | Dwelling.envelope_diagnostics() not exposed |
| [PA-004](PA-004.md) | Dwelling.profiling_summary() not exposed |
| [PA-005](PA-005.md) | SimulationConfig.output_format Arrow not selectable |
| [PA-006](PA-006.md) | from_hpxml() kwarg → equipment config audit |
| [PA-007](PA-007.md) | step() return dict keys vs OCHRE |
| [PA-008](PA-008.md) | DispatchRequest missing EV signal constructors |

## EA — Equipment Deep Audits (7 tickets)

| ID | Title |
|----|-------|
| [EA-001](EA-001.md) | Ventilation (HRV/ERV/exhaust) full behavior |
| [EA-002](EA-002.md) | Generator behavior |
| [EA-003](EA-003.md) | Hot water drain/standpipe/recirculation losses |
| [EA-004](EA-004.md) | Event-based load full behavior |
| [EA-005](EA-005.md) | EV charging, SOC model, V2G/V2H discharge |
| [EA-006](EA-006.md) | Battery dispatch, OCV model, degradation |
| [EA-007](EA-007.md) | Heat pump defrost cycle physics |
