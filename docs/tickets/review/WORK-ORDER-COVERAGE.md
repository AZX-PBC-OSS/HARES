# Work Order Coverage Gap Analysis

**Generated:** 2026-03-30
**Branch:** initial-implementation
**Work order:** `docs/tickets/WORK-ORDER.md`
**Ticket scope:** AR-001..010, TS-001..011, UC-001..010, CW-001..022, IC-001..007,
DT-001..012, WO-001..004, DL-001..004, FP-001..003, EG-001..007, DC-001..008,
TP-001..009, EA-001..007, PS-001..006, PA-001..008, SU-001..009, DV-001..005
(total 130 tickets, plus 14 supplementary = 144)

---

## Summary Counts

| Category | Count |
|---|---|
| Tickets explicitly referenced in work order | 58 |
| Tickets correctly excluded (no bugs / all findings acceptable) | 18 |
| Tickets with bugs NOT referenced in work order (MISSING) | 68 |

---

## Correctly Excluded Tickets (no actionable bugs found)

These tickets found only ACCEPTABLE DIFFERENCE, CORRECT, or INFO findings and require
no code changes. Their omission from the work order is appropriate.

| Ticket | Verdict summary |
|---|---|
| TS-002 | Burch-Christensen formula correct; only finding is LOW (leap-year edge case, EPW always 8760 h) |
| TS-003 | Kusuda-Achenbach formula correct; Finding 3 (dead code) is a pre-existing known gap with no physics impact today |
| TS-005 | All findings ACCEPTABLE DIFFERENCE — thermal mass architecture matches OCHRE exactly |
| TS-006 | All findings ACCEPTABLE DIFFERENCE — infiltration model is more correct than OCHRE |
| UC-004 | No findings section with material bugs; JacketRValue formula divergence is LOW |
| DC-002 | No bugs found — PLR/PLF/duty cycle pipeline is correct; OCHRE behavioral divergence documented |
| DC-008 | Only finding: misleading inline comment (`// OCHRE defaults: 120 s / 180 s` is wrong); LOW |
| DT-001 | No bug attributes found; recommendations are test-coverage additions only |
| DT-003 | Only finding is LATENT / LOW — `ColumnRef` path not time-aware for setpoints |
| DT-004 | Only finding is MEDIUM ACCEPTABLE DIFFERENCE — ideal-capacity FSM bypass |
| DT-006 | No findings — `Occupant` actor is not auto-attached; investigator confirmed no timezone bug in the actor |
| DT-012 | All findings CORRECT — tariff evaluator uses proper `chrono_tz`; DST tests pass |
| WO-001 | F1 (Medium) — 30-minute misalignment at whole-hour sim starts; documented alignment requirement |
| EG-007 | Only finding is LOW — `PartitionWallMass` validation gap, no parity impact under valid input |
| EA-003 | Only finding is Medium — `ansi_resnet_daily_hot_water_l` simplified function misleads callers; currently test-only usage |
| PA-002 | Battery/EV LUT write-only — usability gap, no physics incorrectness |
| PA-003 | `envelope_diagnostics()` feature-gated — usability gap, no physics incorrectness |
| PA-005 | F2 Medium — `output_to_parquet: bool` abstraction is fragile but functionally correct for existing variants |

---

## MISSING Tickets — Bugs Found, Not In Work Order

The following tickets contain confirmed bugs or high-severity gaps that are **not
referenced anywhere in the work order**. They are ordered by highest severity within
each series.

---

### AR Series

**AR-006** — Thermal energy accounting
- **MISSING** — Highest severity: **MEDIUM**
- F1 (Medium): HVAC fan heat missing from zone thermal port for furnace and ASHP heater
  (`furnace.rs:341-368`, `heater.rs:814-815`)
- F3 (Medium): Duct loss `(1−DSE)` discarded when `duct_zone_id == conditioned zone_id`
  (`duct_distribution.rs:35-41`) — ~10-20% of heating capacity silently dropped
- F2 (Low): `ThermalCategory::DuctLoss` defined but never emitted
- F4 (Low): Gas WH standby loss to zone uses nominal UA instead of post-step `skin_loss_w`
- Note: F3 is the same root cause as DL-002 (referenced in work order as P2-E) but
  AR-006 identifies the energy-loss consequence separately and is not cross-linked.

**AR-007** — EquipmentDescriptor and EndUse wiring
- **MISSING** — Highest severity: **HIGH**
- F1 (High): `IdealHvac` hardcodes `EndUse::HVAC_HEATING` despite being capable of cooling
  — cooling energy misattributed in output, `ByEndUse(HVAC_COOLING)` dispatch silently misses
  it (`ideal_hvac.rs:72`)
- F2 (High): `HpCooler` declares only 3 capabilities; DR/DutyCycle signals rejected for all
  ASHP/MSHP cooling (`cooler.rs:61-63`)
- F3 (Medium): `EventBasedLoad` and `WetAppliance` hardcode `EndUse::OTHER`; appliances
  invisible to end-use dispatch and output (`event_load.rs:225,657`)

**AR-009** — Ignored/failing tests
- **MISSING** — Highest severity: **BLOCKER**
- `tests/regression/mod.rs:16` references non-existent `aggregation_check.rs` module —
  compile error; `regression_full_suite` cannot build
- All 3 tracked BESTEST cases fail (600: zero annual loads; 600ff: peak temp ~55°C below
  band minimum; 640: invariant violation -52°C)
- `smoke_resstock_1h` uses HARES's own output as its own reference (tautological test)
- Phantom `#[ignore]` comments in `weather_parity.rs`, `hpxml_parity.rs`,
  `schedule_parity.rs`, `solar_parity.rs` — tests run unconditionally despite comments
  claiming they are skipped
- `freefloat_solar_override` tests panic (not skip) on missing fixtures

**AR-010** — Dispatch layer audit
- **MISSING** — Highest severity: **HIGH**
- F-1 (High): `conflicts_with` never detects cross-variant `ByName`/`ByEndUse` conflicts;
  `observe` feature's `overwrote_earlier` field is always wrong for cross-variant pairs
  (`dispatch.rs:52-58`)
- F-3 (Medium): No construction-time guard against multiple equipment sharing
  `HVAC_HEATING`, `HVAC_COOLING`, or `WATER_HEATING` — silent double-dispatch
- F-6 (Low): `Actor` trait has no `telemetry()` method; project policy requires actor
  telemetry (`feedback_actor_telemetry.md`)

---

### TS Series

**TS-001** — Thermal solver initialization
- **MISSING** — Highest severity: **MEDIUM**
- Finding 2 (Medium): Foundation zones initialized to `outdoor_temp_c` instead of
  `ground_temp_c` (`environment.rs:738-741`); error magnitude 15-25°C in cold climates in
  January

**TS-010** — Humidity solver
- **MISSING** — Highest severity: **MEDIUM**
- DEFECT-1 (Medium): Ventilation uses `LATENT_HEAT_VAPORISATION_J_KG = 2_450_000` while
  humidity solver uses `2_501_000` — end-to-end ventilation moisture flux ~2% lower
  than it should be (`ventilation.rs:338,344`)
- DEFECT-2 (Medium): HPXML resolver correctness for `latent_gain_fraction` (occupant,
  cooking, clothes washer) is unverified — if dropped, zone RH is systematically too low

---

### UC Series

**UC-001** — HVAC capacity BTU/hr→W
- **MISSING** — Highest severity: **MEDIUM**
- F-5 (Medium): `FanPowerWattsPerCFM` stored as rate in HARES vs absolute watts in OCHRE;
  central AC CFM/ton constant differs (HARES: 400, OCHRE: 312) — fan power diverges at
  all operating conditions for any building supplying this HPXML field
  (`resolve_hvac.rs:293-298`, `hvac_core.rs:338-339`)

**UC-006** — Infiltration ELA: in²→cm²
- **MISSING** — Highest severity: **MEDIUM**
- DEFECT-1 (Medium): `0.0524` constant in `solver_builder.rs:736` is unvalidated and
  ~38% lower than the physics-derived value from ASHRAE 136; existing test only checks
  broad plausible range

**UC-008** — Foundation wall/slab conversions
- **MISSING** — Highest severity: **LOW**
- F1 (Low): Foundation wall R-value format strings always round to integer; LUT miss for
  any non-integer R-value in foundation wall insulation keys (`building.rs:1120`)

---

### CW Series

**CW-001** — Gas furnace config wiring
- **MISSING** — Highest severity: **HIGH**
- Finding 1 (High): `insert_startup_degradation` called unconditionally for gas furnaces;
  OCHRE never sets startup CD for pure heaters; gas furnaces get `startup_cd=0.11`
  (`resolve_hvac.rs:309`)
- Finding 2 (Medium): `insert_startup_degradation` efficiency lookup does not find AFUE
  for gas furnaces

**CW-002** — Electric furnace config wiring
- **MISSING** — Highest severity: **HIGH (CRITICAL label)**
- DEFECT-1 (High): `ElectricFurnace` fan power never applied to electrical output;
  `ElectricAuxiliaryEnergy` from HPXML silently dropped; blower motor power always zero
  (`furnace.rs:154-156`)

**CW-003** — Baseboard config wiring
- **MISSING** — Highest severity: **MEDIUM**
- Defect 1 (Medium): Basement heat fraction not suppressed for baseboard; baseboard should
  deliver 100% to its own zone
- Defect 2 (Low): Startup degradation injected for baseboard (resistive element has none)

**CW-004** — Boiler config wiring
- **MISSING** — Highest severity: **CRITICAL**
- Defect 1 (Critical): AFUE from HPXML never reaches `GasBoiler` — efficiency key mismatch
  (`resolve_hvac.rs` emits `"heating_efficiency"`, `GasBoiler` reads `"fuel_efficiency"`
  / `"afue"` / `"efficiency"`)
- Defect 2 (Critical): `ElectricBoiler` efficiency from HPXML never applied — key mismatch

**CW-006** — Room AC config wiring
- **MISSING** — Highest severity: **HIGH**
- F1 (High): EER-specified Room AC silently uses fallback efficiency
- F2 (High): Room AC `Cd` set to 0.07 (central AC default) instead of 0.22 via HPXML path

**CW-007** — ASHP heater config wiring
- **MISSING** — Highest severity: **MEDIUM (GAP)**
- F-1 (Gap): Single-speed HP heater PLF `[0.89, 0.11, 0]` not injected when HSPF ≥ 7
  (OCHRE `HeatPumpHeater.__init__:1124-1126` injects it; HARES early-returns for
  `n_speeds <= 1`)

**CW-011** — Electric resistance WH config wiring
- **MISSING** — Highest severity: **MEDIUM**
- Finding 1 (Medium): Upper element node for ≥12-node tanks should be index 2 (OCHRE
  parity) but defaults to 0

**CW-013** — Heat pump WH config wiring
- **MISSING** — Highest severity: **CRITICAL**
- DEFECT-3 (Critical): Low-power HPWH (UEF == 4.9) overrides not wired — COP computed as
  `1.17 × 4.9 = 5.75` instead of OCHRE's hardcoded 4.2; capacity defaults to 1200 W
  instead of 1499 W; `hp_only_mode` not set
- DEFECT-5 (High): `heating_capacity_w` emitted by resolver is silently ignored by HPWH
  init
- DEFECT-6 (High): `lost_heat_fraction` and `wall_heat_fraction` default to 0 instead of
  OCHRE defaults (0.75 and 0.5 respectively) — HPWH zone interaction not modelled
- DEFECT-2 (Medium): EF-only HPWH gets no COP derivation
- DEFECT-7 (Medium): Jacket R-value formula diverges from OCHRE (series vs parallel model)
- DEFECT-1 (Medium): Tempering valve setpoint hardcoded to 51.67°C, ignores HPXML setpoint

**CW-014** — Tankless WH config wiring
- **MISSING** — Highest severity: **HIGH (CRITICAL label)**
- F-1 (High): FuelType key name mismatch — `"fuel_type"` vs OCHRE-style fuel string
- F-2 (High): Efficiency key mismatch — resolver inserts `"energy_factor"`, tankless init
  never finds it
- F-4 (High): Capacity key mismatch — `"heating_capacity_w"` never read by tankless init
- F-5 (High): Gas parasitic power not computed or forwarded
- F-6 (Critical): `"Gas Tankless Water Heater"` registry name has no registered handler
  (pre-existing, partially addressed in B0-5 but CW-014 documents additional key mismatches)

**CW-015** — PV config wiring
- **MISSING** — Highest severity: **HIGH**
- F-1 (High): `ModuleType` case-sensitive matching silently falls back to `Standard` for
  `"thin film"` (space variant) — wrong temperature coefficient, ~3.5% power error per 10°C
  (`pv/mod.rs`)
- F-2 (High): `ArrayTilt` key is dead code in `from_single_config`; tilt extracted but
  dropped for some HPXML paths

**CW-016** — Battery config wiring
- **MISSING** — Highest severity: **CRITICAL**
- F-1 (Critical): Double `sqrt` on round-trip efficiency from HPXML — `rte` applied as
  `sqrt(rte)` twice, producing wrong charging/discharging efficiency
- F-2 (Medium): Default `cell_ua_w_per_k` does not match OCHRE thermal resistance default
- F-3 (Medium): Pack topology (`n_series`) derived from incompatible pack voltage target
  (also tracked as DV-004 but CW-016 documents the resolver-side path independently)

**CW-018** — Ventilation config wiring
- **MISSING** — Highest severity: **CRITICAL**
- F-1 (Critical): Flow rate never converted CFM→m³/s; equipment silently uses default
- F-2 (Critical): Sensible/latent effectiveness keys never consumed; equipment uses
  hardcoded defaults
- F-3 (Critical): Ventilation type never wired; HRV/ERV/exhaust distinction always falls
  back to HRV
- F-4 (Medium): Default fan power when `FanPower` absent not implemented
- F-5 (Medium): `HoursInOperation` not read; schedule always constant 1.0

**CW-021** — EV config wiring
- **MISSING** — Highest severity: **HIGH (CRITICAL label for F-002)**
- F-001 (High): Dead key `"battery_capacity_kwh"` emitted by PlugLoad path, never read by
  `Ev` init
- F-002 (Critical): `"Electric Vehicle"` spec name not registered in equipment registry
  (partially addressed in B0-5 but CW-021 documents the PlugLoad key mismatch independently)
- F-003 (Medium): OCHRE derives EV type from annual kWh; HARES PlugLoad path uses fixed
  threshold

**CW-022** — Generator config wiring
- **MISSING** — Highest severity: **HIGH**
- F-001 (High): Fuel type parsed but silently dropped for non-gas generators
- F-002 (High): CHP thermal output has no HPXML extraction path

---

### IC Series

**IC-002** — HVAC ideal capacity formula
- **MISSING** — Highest severity: **MEDIUM**
- D1 (Medium): `latent_gain_w` always `0.0` in `IdealHvac::step()` — humidity solver
  receives no latent cooling from ideal HVAC (`ideal_hvac.rs:428`)
- D2 (Low): `IdealHvac` emits no electrical port contribution — electrical consumption
  silently zero in ideal HVAC mode

**IC-003** — HVAC ideal capacity clipping
- **MISSING** — Highest severity: **HIGH**
- Finding 2 (High): No upper-bound clip on ideal capacity — a 10 kW unit can silently
  deliver 50 kW, corrupting energy balance
- Finding 6 (High): Missing capacity column for unmet-load metric — `unmet_load_hours`
  always zero for ideal-HVAC dwellings
- Finding 4 (Medium): `capacity_min` missing — affects modulation-limited equipment at
  coarse timesteps

**IC-005** — HPWH ideal capacity
- **MISSING** — Highest severity: **CRITICAL**
- F1 (Critical): ER backup trigger is a single configurable offset, not OCHRE's two-node
  temperature thresholds — backup element activation timing is wrong for all HPWH
  configurations

**IC-006** — Gas WH ideal capacity
- **MISSING** — Highest severity: **HIGH**
- F1 (High): Gas WH has binary on/off only; no proportional (ideal) capacity mode
- F2 (Medium): `capacity_rated` semantic mismatch between OCHRE and HARES
- F3 (Medium): Thermostat sensor node vs OCHRE node selection

**IC-007** — Tankless WH ideal mode
- **MISSING** — Highest severity: **HIGH**
- F3 (High): `PowerLimit` signal direction inverted for gas tankless units
- F4 (Medium): Parasitic power telemetry emitted unconditionally for electric units

---

### DT Series

**DT-002** — EventBasedLoad event timing
- **MISSING** — Highest severity: **HIGH**
- F1 (High): UTC/local mismatch — `start_time` offset unvalidated against EPW timezone
- F2 (Medium): `current_step` not cross-checked against `env.current_time`

**DT-005** — EV driver actor departure/arrival
- **MISSING** — Highest severity: **HIGH**
- DEFECT-1 (High): `FixedOffset` does not shift for DST — departure/arrival hour wrong
  during DST transitions
- DEFECT-2 (Medium): `resolve_departure` applies today's `DayFilter` to midnight-wrap
  departure

**DT-007** — BMS TOU windows
- **MISSING** — Highest severity: **MEDIUM**
- Finding 1 (Medium): `ordinal0()` used as day-of-year price-array index — misaligns price
  buckets for up to `abs(utc_offset_hours)` hours (`bms.rs:215,408,443`)

**DT-008** — DR compliance actor timing
- **MISSING** — Highest severity: **MEDIUM**
- Finding (Medium): No auto-revert in `DrCompliance` actor for duration-bounded events

**DT-009** — SimulationConfig start_time propagation
- **MISSING** — Highest severity: **MEDIUM**
- D1 (Medium): Naive `start_time` input silently assumed UTC (`utils.rs:38-51`)
- D2 (Medium): EPW rebase silently discards the supplied offset (`dwelling/mod.rs:761-767`)

**DT-010** — EnvironmentManager schedule domain indexing
- **MISSING** — Highest severity: **HIGH**
- Finding 1 (High): `solar_override` ignores `weather_start_offset` — solar override
  schedule misaligned with weather series
- Finding 2 (Medium): Leap-year `year_secs` inflates annual offset for EPW data

---

### WO Series

**WO-001** — EPW 30-minute midpoint offset
- **MISSING** — Highest severity: **MEDIUM**
- F1 (Medium): 30-minute misalignment when sim starts at a whole-hour boundary (e.g.,
  `13:00:00`) — step-0 reads EPW row for 12:30 midpoint instead of 13:00
  (`environment.rs:603-625`)

**WO-004** — Weather resampling methods per field
- **MISSING** — Highest severity: **MEDIUM**
- Defect 1 (Medium): `rel_humidity_pct` and `opaque_sky_cover` ignore `ResampleOverrides`
  — `ochre_compat()` override is silently discarded for these two fields (`weather.rs:428,432`)

---

### DL Series

**DL-001** — ASHRAE 152 DSE calculation
- **MISSING** — Highest severity: **CRITICAL**
- F1 (Critical): `dTe_low` air density constant differs between OCHRE (`0.0775`) and HARES
  (`0.075`) on multi-speed low-speed path (`ashrae152.rs`) — DSE result differs for all
  multi-speed heating configurations
- F2 (Critical): `resolve_duct_dse` always passes `capacity_low_w: None` and
  `fan_flow_low_m3_s: None` for multi-speed systems — low-speed DSE branch always uses
  high-speed values

**DL-003** — ASHRAE 152 zone temperature lookup
- **MISSING** — Highest severity: **MEDIUM**
- Finding 1 (Medium): `capacity_low_w` / `fan_flow_low_m3_s` always `None` for multi-speed
  equipment — understates duct losses for variable-speed equipment (same root cause as DL-001
  F2; DL-003 documents the zone-temperature impact path independently)

---

### FP Series

**FP-002** — Ideal HVAC fan power
- **MISSING** — Highest severity: **HIGH**
- `IdealHvac` emits zero fan power in all circumstances — both electrical consumption and
  zone sensible heat from fan motor are absent (`ideal_hvac.rs`)

**FP-003** — Fan power in electrical + thermal ports
- **MISSING** — Highest severity: **HIGH**
- Finding 1 (High): `AirConditioner` omits fan heat from zone thermal port
- Finding 2 (High): `HeatPumpHeater` omits fan heat from zone thermal port
- Note: Same root cause as AR-006 F1 and P2-A in the work order. However P2-A is
  framed around fan heat injection generally; FP-003 documents it as a separate
  confirmed ticket.

---

### EG Series

**EG-001** — Ceiling height derivation
- **MISSING** — Highest severity: **MEDIUM**
- DEF-1 (Medium): Silent fallback to 2.5 m constant when `ConditionedBuildingVolume` or
  `ConditionedFloorArea` is absent — incorrect zone volumes, infiltration ACH, and natural
  ventilation with no warning
- DEF-3 (Low): `building_height_m` uses conditioned zone count instead of floor count
  — half the correct height for a single-zone two-storey dwelling

**EG-004** — Garage geometry
- **MISSING** — Highest severity: **HIGH**
- Finding 1 (High): `garage_protruded_area` never computed
- Finding 2 (High): Attic volume formula drops the garage correction for 3-gable homes
- Finding 3 (Medium): Ceiling height fallback of 2.5 m diverges from OCHRE
- Finding 4 (Medium): `garage_wall_height` back-calculation absent

**EG-005** — Floor area derivation
- **MISSING** — Highest severity: **CRITICAL**
- Finding 1 (Critical): Conditioned zone floor area includes underground foundation area
  — inflates floor area for all buildings with basement or crawlspace
- Finding 2 (Medium): Conditioned zone volume uses inflated area (consequence of Finding 1)

**EG-006** — Attic volume formula
- **MISSING** — Highest severity: **CRITICAL**
- F1 (Critical): 3-gable compound volume formula entirely absent
- F2 (Critical): `attic_floor_area` excludes garage ceiling area
- F3 (High): `garage_protruded_area` never computed
- F4 (High): Silent 200 m³ fallback swallows parse failures
- F5 (Medium): Gable selection index mismatch for Path B

---

### DC Series

**DC-001** — OCHRE mode priority / duty cycle counters
- **MISSING** — Highest severity: **HIGH**
- F-1 (High): No `ext_mode_counters` equivalent; `period_s` is a dead field
- F-2 (High): Speed timer never resets on Off-cycle
- F-3 (Medium): No `ext_ignore_thermostat` equivalent

**DC-003** — WH duty cycle (upper/lower ER split)
- **MISSING** — Highest severity: **CRITICAL**
- F1 (Critical): ERWH ideal-capacity path ignores lower element when upper is active
  (incorrect power suppression)
- F2 (High): `ideal_capacity_for_node` computes single-node deficit, not zone deficit
- F3/F4 (High): Upper thermostat priority logic diverges from OCHRE

**DC-005** — Dehumidifier humidity-based activation
- **MISSING** — Highest severity: **MEDIUM**
- Bug: `Some(OperatingMode::Standby)` in forced-on arm incorrectly suppresses activation
- Bug: `update_control` called twice per step
- Gap: No HPXML parsing for Dehumidifier — all HPXML-specified dehumidifiers silently
  ignored

**DC-007** — Wet appliance event schedule
- **MISSING** — Highest severity: **HIGH**
- Two DEFECT HIGH findings and three additional MEDIUM/GAP findings in event schedule
  handling

---

### TP Series

**TP-001** — HVAC heating telemetry
- **MISSING** — Highest severity: **HIGH**
- F-1 (High): `operating_mode` encoded as float enum; OCHRE emits human-readable string
- F-5 (High): `HVAC Heating Capacity (W)`, `Max Capacity (W)`, `Main Power (kW)` entirely
  absent
- F-6 (High): `HVAC Heating ER Power (kW)` absent for ASHPHeater
- Note: partly covered by P5-D but the heating-specific columns are not explicitly called out

**TP-002** — HVAC cooling telemetry
- **MISSING** — Highest severity: **MEDIUM**
- F1 (Medium): COP definition diverges from OCHRE
- F2 (Medium): `HVAC Cooling Setpoint (C)`, `Capacity (W)`, `Max Capacity (W)` absent

**TP-003** — Water heater telemetry
- **MISSING** — Highest severity: **HIGH**
- HIGH-1 (High): HPWH `ELECTRIC_KW` not saved/restored across checkpoint round-trips
- HIGH-2 (High): `ResistanceWhState.electric_power_w` field name implies W but holds kW
- MEDIUM-1 (Medium): HPWH `telemetry_fields` omits `ELECTRIC_KW` from declared schema
- MEDIUM-2 (Medium): HPWH `load_state` does not restore `WALL_SENSIBLE_GAIN_W` or
  `UNMET_LOAD_W`
- MEDIUM-3 (Medium): `OUTLET_TEMP_C` and `UNMET_LOAD_W` absent from ResistanceWH and
  GasWH telemetry
- Note: MEDIUM-3 is partially covered by P5-D, but HIGH-1 and HIGH-2 are not addressed

**TP-005** — PV telemetry
- **MISSING** — Highest severity: **CRITICAL**
- F1 (Critical): `dc_power_kw` is wrong in the LUT path
- F2 (Medium): `update_control` reads stale `last_ac_power_kw` — mode lags one step

**TP-006** — EV telemetry
- **MISSING** — Highest severity: **MEDIUM**
- F1 (Medium): V2G discharge has no dedicated telemetry field
- F2 (Medium): `ACTIVE_POWER_KW` and `ELECTRIC_KW` are always identical — one is redundant

**TP-007** — Scheduled/event load telemetry
- **MISSING** — Highest severity: **HIGH**
- F1 (High): `REACTIVE_POWER_KVAR` not restored in `ScheduledLoad::load_state`
- F2 (Medium): `ACTIVE_POWER_KW` always `0.0` for non-electric `EventBasedLoad`/`WetAppliance`

**TP-008** — Envelope/dwelling-level output columns
- **MISSING** — Highest severity: **CRITICAL**
- Finding 1 (Critical): `Unmet HVAC Load (C)` declared in schema, never written
- Finding 2 (High): `EquipmentColumns` missing setpoint, soc, energy, capacity, cop indices
- Finding 3 (High): `MetricsCalculator` looks up `HVAC Heating/Cooling Setpoint (C)` but
  these columns are never in the schema
- Finding 4 (Medium): Five boundary-component heat gain columns declared, not written
- Note: AR-008 is referenced in the work order (P5-A) but TP-008 documents specific
  missing columns not enumerated there

**TP-009** — ThermalCategory correctness
- **MISSING** — Highest severity: **CRITICAL**
- Finding 1 (Bug): Dehumidifier uses `InternalGain` despite removing latent energy
- Finding 2 (Bug): HPWH uses `InternalGain` for a signed zone-heat interaction
- Finding 4 (Bug): Ventilation uses `InternalGain`, masking a signed contribution
- Finding 5 (Critical): `DuctLoss` category never emitted — confirmed (also in AR-006 F2
  and DL-004 but TP-009 documents the ThermalCategory impact)

---

### EA Series

**EA-002** — Generator behavior
- **MISSING** — Highest severity: **CRITICAL**
- F-001 (Critical): Fuel type hardcoded to Gas; non-gas generators silently misbehave

**EA-005** — EV charging, SOC model, V2G/V2H discharge
- **MISSING** — Highest severity: **HIGH (revised from CRITICAL)**
- F-001 (High): V2G/V2L discharge uses `charging_efficiency` inverted — wrong sign for
  round-trip loss
- F-002 (Medium): V2G priority over V2L is undocumented and may be incorrect
- F-003 (Medium): V2G discharge does not apply efficiency in `apply_soc_and_thermal`
- Note: EA-005 is structurally distinct from EA-006 (Battery); EV-specific discharge bugs
  are not covered by P2-B in the work order

---

### PS Series

**PS-002** — ControlSignal dict construction safety
- **MISSING** — Highest severity: **MEDIUM**
- F1 (Medium): `EventDelay` has no static constructor on `PyControlSignal`
- F2 (Medium): No numeric range validation on control signal parameters

**PS-003** — Actor.decide() env dict → typed EnvironmentView
- **MISSING** — Highest severity: **CRITICAL**
- Finding 1 (Critical): `EnvironmentState` fields silently dropped in
  `environment_to_py_dict` — Python actors receive incomplete environment
- Finding 2 (High): `current_time` passed as RFC3339 string, not `datetime`
- Finding 3 (High): Stub annotation `decide(self, env: dict[str, Any])` provides zero
  type safety
- Finding 4 (Medium): `DispatchRequest` return path has a silent error swallow

**PS-004** — Type stubs completeness
- **MISSING** — Highest severity: **CRITICAL**
- F1 (Critical): `StormWatchTrigger` stub is structurally wrong
- F2 (Critical): `Signal` class missing three EV control factory methods
- F3 (High): `SimulationConfig` stub missing setters for eight properties
- F5 (High): `DRLevel.from_str` stub method does not exist at runtime

**PS-005** — Equipment construction parameter validation
- **MISSING** — Highest severity: **MEDIUM**
- F1 (Medium): `PyBattery.__new__` accepts all invalid physical values
- F2 (Medium): `PyEv.__new__` performs no validation on `initial_soc` or `capacity_kwh`

---

### PA Series

**PA-004** — `profiling_summary()` not exposed
- **MISSING** — Highest severity: **MEDIUM**
- Finding 1 (Medium): `profiling_summary()` entirely absent from Python bindings
- Finding 2 (Medium): `profiling` and `actor_profiling` features not forwarded in
  `hares-python/Cargo.toml`
- Finding 3 (Medium): `actor_timing()` also absent from Python bindings

**PA-006** — `from_hpxml()` kwarg → equipment config audit
- **MISSING** — Highest severity: **CRITICAL**
- F1 (Critical): `overrides` and `resample_overrides` are hardcoded `None` in `build_config`
  — all user-supplied overrides via `from_hpxml()` are silently ignored
- F2 (High): `build_config` does not reject unknown kwargs
- F3 (High): Parallel implementations of the same logic will diverge
- Note: PS-001 F1 in the work order (P5-B) covers `overrides` wiring but PA-006 F1
  documents the same bug at the `from_hpxml` path specifically, which is not referenced

**PA-007** — `step()` return dict keys vs OCHRE
- **MISSING** — Highest severity: **HIGH**
- SEV-1 (High): `step()` emits no gas power key — gas-heated dwelling callers get nothing
  for gas consumption
- SEV-2 (High): Zone temperature keys absent from TypedDict stub — static type checkers
  report errors for any caller accessing zone temperatures
- SEV-2b (High): `reactive_power_kvar` typed as required in `SteppableStepResult` but
  conditionally absent at runtime

---

### DV Series

**DV-002** — ASHRAE 152 multi-speed DSE
- **MISSING** — Highest severity: **MEDIUM**
- F1 (Medium): `0.0775` vs `0.075` air density constant on multi-speed low-speed path —
  OCHRE is likely a bug; HARES is physically correct (0.075) but this divergence must be
  explicitly documented and a numeric regression test added
- F2 (Medium): No test pins a numeric DSE value; all tests are bounds-only — silent formula
  regression is undetectable
- F3 (Medium): Multi-speed heat-pump heating path has no test at all

---

## Cross-Cutting Observations

1. **The work order's Batch 5 (P5-D) partially addresses TP-003** (HPWH missing telemetry
   fields, ResistanceWH/GasWH missing `OUTLET_TEMP_C`/`UNMET_LOAD_W`) but does not address
   TP-003 HIGH-1 (checkpoint non-restoration of `ELECTRIC_KW`) or HIGH-2 (unit semantic
   mismatch in `electric_power_w` field name).

2. **B0-5 (registry name fixes)** partially addresses CW-021 F-002 and CW-014 F-6 but
   does not address the key mismatches documented in CW-014 F-1 through F-5, nor the dead
   key in CW-021 F-001.

3. **P2-A** references `FP-001..003` and `IC-001..003` but the work order item focuses
   on fan heat injection and `IdealHvac` capacity clipping. FP-002, FP-003, IC-002, and
   IC-003 each document additional bugs not enumerated in P2-A.

4. **AR-009 BLOCKER** (`aggregation_check.rs` missing) is not addressed anywhere in the
   work order. The `regression_full_suite` test cannot compile until this is resolved.

5. **EG-004, EG-005, EG-006** (garage geometry, floor area, attic volume) contain CRITICAL
   and HIGH bugs that affect every building with a garage or basement. EG-002 and EG-003
   are referenced in Batch 3 P1-A; EG-004 through EG-006 are not.

6. **TP-009** (`ThermalCategory` correctness) and **AR-006** F2 both independently confirm
   that `DuctLoss` category is never emitted, yet neither is directly referenced — only
   the related but different DL-004 is referenced in the work order.

7. **CW-018** (Ventilation config wiring — 3 CRITICAL findings) is one of the highest-
   impact omissions in the work order. Ventilation type, flow rate, and HRV/ERV
   effectiveness are all silently wrong for all HPXML-parsed ventilation configurations.

8. **PA-006 F1** and **PS-001 F1** describe the same `overrides` wiring bug from different
   entry points. PS-001 is referenced in P5-B; PA-006 is not.
