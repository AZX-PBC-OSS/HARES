# Fix Strategy — Prioritized Implementation Order

## Guiding Principles

1. **Fix wiring before physics** — no point tuning a formula if the input never reaches it
2. **Fix infrastructure before equipment** — typed configs eliminate entire bug classes
3. **Test bottom-up** — unit tests per equipment, then crate integration, then full dwelling
4. **No full-system parity tests until foundations are solid** — they'll just fail for 50 reasons at once
5. **Each fix must be independently verifiable** — don't batch unrelated changes

---

## Phase 0: Infrastructure (eliminates ~40 bugs at once)

### P0-A: Typed config structs per equipment (CC-001)
**Kills:** All CW-series key mismatches (~25 bugs), AR-002
**Implemented by:** CFG-007 through CFG-016
**Approach:**
1. Define per-equipment config structs with `#[serde(deny_unknown_fields)]`
2. Resolver writes the struct directly (no `Map<String, Value>`)
3. Equipment `init()` receives the typed struct
4. Compile errors immediately surface every mismatch
**Test:** Each struct has a round-trip test (serialize → deserialize → fields match)

### P0-B: Typed telemetry keys (CC-002)
**Kills:** AR-001 bugs ("mode" vs "operating_mode", generator power, dehumidifier power)
**Approach:**
1. Shared `pub const` module for all telemetry keys
2. Both writer and reader import the same constant
3. `Telemetry::set` hard-errors on undeclared keys in all builds
**Test:** Lifecycle test per equipment type asserting key count and names

### P0-C: Registration failure → hard error (CC-005)
**Kills:** AR-005 (EV, Gas Tankless WH, Generic Heater/Cooler silently dropped)
**Implemented by:** CFG-008
**Approach:**
1. `registry.create()` failure returns `Err`, not `continue`
2. Dwelling construction fails with a list of unresolvable equipment
3. User sees exactly what's missing
**Test:** Negative test: bad equipment name → construction error with descriptive message

### P0-D: Init-time config validation (CC-003)
**Kills:** Future regressions after P0-A
**Implemented by:** CFG-015
**Approach:**
1. Track which config keys are consumed during init
2. Warn on unconsumed keys (dead config)
3. Error on required-but-missing keys
**Test:** Integration test: inject config with extra key → warning emitted

---

## Phase 1: Envelope & Thermal Core (independent of equipment)

### P1-A: Foundation zone geometry
**Kills:** TS-004, EG-003, EG-005, EG-006
- Use actual foundation wall height (not conditioned ceiling height)
- Subtract foundation area from conditioned floor area
- Fix attic volume compound formula
**Test:** Unit tests with known geometries → expected volumes

### P1-B: Attic zone model
**Kills:** TS-011, EG-002, EG-006
- Fix radiant barrier surface (interior not exterior)
- Add interior LWR for attic zone
- Fix gable selection logic
- Remove silent 200 m³ fallback → error on missing data
**Test:** Synthetic box with attic → known temperature response

### P1-C: Thermal solver fixes
**Kills:** TS-007 (stale CN docstring, b_coeff derivation), TS-009 (LWR iteration count, radiant barrier face)
- Fix b_coeff to use continuous B_c, not discrete B_d
- Fix exterior LWR iteration count (off-by-one)
- Fix interior LWR: add iterative surface-temperature update
**Test:** Existing envelope oracle tests should tighten after fixes

### P1-D: Solar distribution energy conservation
**Kills:** TS-008 (energy loss when floor_area=0)
- Redistribute beam to walls when no floor surface
- Never silently drop energy
**Test:** Invariant test: sum of absorbed solar = transmitted solar within ε

### P1-E: Ventilation double-accounting (CC-009)
**Kills:** EA-001 F1 (CRITICAL), DC-004 F-2/F-4, CW-018 (key mismatches fixed by P0-A but thermal path must be resolved here)
- Remove `PortContribution::Thermal` from `Ventilation::step()` — equipment owns fan electrical and telemetry only
- Envelope solver (`apply_infiltration_and_ventilation`) is the sole owner of forced ventilation heat exchange
- Fix ventilation key mismatches so HPXML flow rate reaches the infiltration solver correctly (after P0-A, this is automatic via CFG-013)
- Add `HoursInOperation` parsing with warn-on-non-24h (EA-001 F5, DC-004 F-3)
- Fix ExhaustFan bypass telemetry false-positive (EA-001 F10, DC-004 F-6)
**Test:** Ventilation on/off → zone temp difference matches analytical expectation; energy balance closes within ε

---

## Phase 2: Equipment Physics (after wiring is trustworthy)

### P2-A: Fan heat injection (all HVAC types) + IdealHvac completeness (CC-008, CC-012)
**Kills:** FP-001, FP-002, FP-003, AR-006, CW-002, IC-001, IC-002, IC-003
- Add `fan_power_w` to `ElectricFurnace` (CRITICAL — FP-001 F1): inject fan heat into zone thermal port and fan draw into electrical port
- Fix `GasFurnace` fan fallback: use 350 CFM/ton path from `hvac.airflow_m3_s_per_w` (FP-001 F2)
- Add fan heat to zone thermal port for `HeatPumpHeater` and `AirConditioner` (FP-003 F1/F2)
- Add fan power to `IdealHvac`: `rated_fan_power_w`, `fan_power_ratio`, emit to both ports (FP-002)
- Clip `IdealHvac` ideal capacity at `rated_capacity_w` / `cooling_capacity_w` (IC-003 F2, HIGH)
- Add `HVAC Heating/Cooling Capacity (W)` telemetry to `IdealHvac` for unmet-load detection (IC-003 F6, HIGH)
- Fix `IdealHvac` latent cooling: emit `latent_gain_w = capacity * (1-shr)` in cooling mode (IC-002 D1, MEDIUM)
- Fix `IdealHvac` EndUse: emit `HVAC_COOLING` when in cooling mode (IC-002 D4, LOW)
- Fix `HvacEquipment` auto-select: check `n_speeds >= 4 || time_res >= 300s` consistently (IC-001)
**Test:** Per-equipment energy balance: electrical_in = thermal_out + losses within ε; IdealHvac cooling delivers correct latent fraction

### P2-B: Battery physics (CC-011)
**Kills:** TP-004 (CRITICAL), EA-006 F1 (CRITICAL), EA-006 F3 (HIGH), EA-006 F6 (MEDIUM)
- Remove double ohmic loss subtraction in SOC update on discharge (EA-006 F1, CRITICAL)
- Apply `soh = 1 - fade_pct` to `capacity_kwh_nominal` at daily boundary (TP-004, CRITICAL)
- Fix mechanism-3 (BOL transient) dead code: implement sign-correct `q3` update or document removal (EA-006 F3, HIGH)
- Fix SOC-target controller: convert DC power to AC setpoint using efficiency model (EA-006 F6, MEDIUM)
- Add `capacity_fade_pct` unit alignment: fraction [0,1] vs percent annotation (TP-004 HIGH)
- Add `nominal_capacity_kwh`, `actual_capacity_kwh`, `energy_to_discharge_kwh` telemetry (TP-004 MEDIUM)
**Test:** 10-cycle charge/discharge → cumulative SOC drift ≤ 0.1%; 30-day BOL run → `q_li3` non-zero and decreasing

### P2-C: Defrost physics (CC-017)
**Kills:** EA-007 F1 (CRITICAL), EA-007 F2 (HIGH)
- Compute `extra_power_w` from post-defrost capacity (EA-007 F1): `post_cap = current_cap * cap_mult - q_defrost`
- Change `DEFAULT_DEFROST_CAPACITY_REDUCTION_FACTOR` from 0.75 to 1.0 (EA-007 F2, HIGH)
- In ideal-capacity mode, clamp `hp_capacity_w` to defrost-reduced rated maximum (EA-007 F3, MEDIUM)
- Add defrost telemetry fields: `defrost_extra_power_w`, `defrost_q_w`, `defrost_capacity_multiplier` (EA-007 F5, LOW)
**Test:** At OAT=0°C and time_frac=0.3, HARES defrost extra_power_w matches OCHRE reference within 1%

### P2-D: HVAC efficiency wiring
**Kills:** UC-002, CW-005, CW-006, CW-008 (SEER key mismatch)
- This is fixed by P0-A (typed config), but verify the values flow correctly end-to-end
**Test:** Per-equipment: rated SEER in → correct EIR used in step

### P2-E: Duct loss accounting
**Kills:** DL-002, DL-004, AR-003, AR-006
- DSE=1 when ducts in conditioned zone (don't discard energy)
- Tag duct zone contributions with ThermalCategory::DuctLoss
- Read duct_loss_w from correct accumulator
- Basement airflow ratio: heating only, not cooling
**Test:** HVAC with ducts: sum(zone gains) = gross capacity within ε

### P2-F: Water heater fixes (CC-015)
**Kills:** CW-012, CW-013, CW-014, IC-004, DC-003, WO-002
- Fix hot water draw fraction → L/min conversion (WO-002 CRITICAL): wire `normalize_draw_profile()` into `inject_water_heater_schedule_columns`; scale by `(avg_l_per_day / 1440) / mean_fraction`
- Fix gas WH conversion_efficiency forwarding
- Fix HPWH backup element capacity key, ER threshold, COP curve input temp (CW-013; fixed by P0-A)
- Fix tankless WH key casing (CW-014; fixed by P0-A)
- Fix simultaneous element mode (DC-003)
**Test:** Per-WH-type: draw profile → energy consumption within 5% of analytical; tank depletes under sustained draw at correct rate

### P2-G: Gas appliance fuel consumption (CC-016)
**Kills:** EA-004 F1 (CRITICAL)
- Read `annual_gas_therms` in `determine_max_kw()` and add combustion watts to event power series for gas dryers and cooking ranges
- Fix `PowerSetpoint` override firing unconditionally regardless of phase (EA-004 F2, HIGH)
- Fix `EventBasedLoad` config field mutation in deterministic mode (EA-004 F3, HIGH)
- Fix `OperatingMode::Standby` returned for active phase (EA-004 F8, LOW)
**Test:** Gas dryer: step through one event, assert fuel port output ≈ total annual_gas_therms energy rate; electric parasitic also correct

### P2-H: Capacity curves and corrections
**Kills:** WO-003, CW-008, CW-010
- Wire flow-fraction quadratic curves (cap_ff, eir_ff)
- Propagate biquadratic temperature bounds
- Use per-stage curves for multi-speed (not just last stage)
- Add MaxCapacityFraction control signal
**Test:** Biquadratic evaluation at AHRI rated conditions = 1.0 within ε

### P2-I: Gain fraction defaults
**Kills:** CW-017, CW-020, DC-006
- Centralize per-load-type (sensible, latent) fraction table
- Fix lighting=1.0, MELs=(0.855, 0.045), TV/ceiling fan/freezer=0.0
- Add bedroom-based defaults for refrigerator/freezer kWh
**Test:** Load type → fraction lookup matches ASHRAE table values

### P2-J: Mini-split speed inference
**Kills:** CW-009, CW-010
- Force 4 speeds for all mini-split heat pumps regardless of CompressorType
- Propagate per-stage SHR
**Test:** Mini-split from HPXML → 4 stages in equipment state

---

## Phase 3: HPXML Parsing Fixes (after equipment models are correct)

### P3-A: XML element path fixes (CC-014)
**Kills:** UC-003 F1/F2 (CRITICAL), UC-005 F-001 (HIGH)
- Duct leakage: navigate to sibling `<DuctLeakageMeasurement>` under `<AirDistribution>`, read `<Value>` child, check `<Units>` for Percent vs CFM25
- `AssemblyEffectiveRValue`: replace `node.child(...)` with `node.first_descendant(...)` for all wall/roof/floor surfaces
**Test:** Parse parity-corpus HPXML → assert non-zero leakage fractions; assert `assembly_r_value_m2_k_w` present for all surfaces

### P3-B: Missing HPXML fields
**Kills:** UC-007 (ExteriorShading), CW-019 (Dehumidifier), UC-009 (PV ModuleType case)
- Add exterior shading parsing
- Add dehumidifier to resolve allowlist
- Fix ModuleType case-insensitive matching
**Test:** Parse HPXML with each field → equipment receives correct value

### P3-C: Unit conversion hardening
**Kills:** AR-004 (conductivity passthrough), UC-010 (temperature passthrough)
- Error on unrecognized unit strings (not silent passthrough)
- Use uom at all conversion boundaries
**Test:** Invalid unit string → error, not silent default

---

## Phase 4: Integration & Parity Testing

### P4-A: Per-equipment oracle tests
- Each equipment type run standalone for 24h with known inputs
- Compare against analytical or OCHRE reference output
- These are the FIRST integration tests worth running

### P4-B: Per-fixture parity tests
- Run each parity corpus fixture
- Compare against OCHRE reference parquet output
- Tighten tolerances incrementally

### P4-C: BESTEST cases
- Run 600, 640, 900, 600FF, 900FF
- Must fall within ASHRAE 140 acceptance bands
- These validate the full stack end-to-end

---

## Phase 5: Output & Python API (last priority)

### P5-A: Output column population (AR-008)

### P5-B: Python safety boundary (PS-001 through PS-006) — CC-013
Priority order within P5-B:
1. Add `catch_unwind` in `step_core`, `simulate` closure, and each Rayon closure in `batch_step_py` (PS-006 CRITICAL — Rust panics abort the Python process)
2. Wire `overrides` and `resample_overrides` through `from_hpxml` → `build_config` (PS-001 F1 CRITICAL)
3. Add unknown-key detection in `build_config` (PS-001 F2, HIGH)
4. Unify two parallel construction paths: `from_hpxml` delegates to `DwellingConfig` (PS-001 F3, HIGH)
5. Define typed Python exceptions via `pyo3::create_exception!` (PS-006 HIGH)
6. Augment step/simulate error messages with `bldg_id` and `current_step` (PS-006 HIGH)
7. `ControlSignal.event_delay()` static constructor (PS-002 F1, MEDIUM)
8. `Actor.decide()` env dict → typed `EnvironmentView` (PS-003)
9. Complete `_hares.pyi` type stubs (PS-004)
10. Equipment construction parameter validation (PS-005)

### P5-C: Python API gaps (PA-001 through PA-008)
- `PA-001`: telemetry missing `timestep_index`/`current_time`
- `PA-002`: Battery/EV LUT getters
- `PA-003`: `Dwelling.envelope_diagnostics()` exposure
- `PA-004`: `Dwelling.profiling_summary()` exposure
- `PA-005`: `SimulationConfig.output_format` Arrow selection
- `PA-006`: `from_hpxml()` kwarg → equipment config audit
- `PA-007`: `step()` return dict keys vs OCHRE
- `PA-008`: `DispatchRequest` missing EV signal constructors

### P5-D: Checkpoint telemetry completeness (CC-010 / TP-003, TP-007)
- HPWH: add `ELECTRIC_KW`, `WALL_SENSIBLE_GAIN_W`, `UNMET_LOAD_W` to `HpwhState`
- `ResistanceWH`, `GasWH`: add `OUTLET_TEMP_C`, `UNMET_LOAD_W` to states
- `ScheduledLoad`: add `REACTIVE_POWER_KVAR` to `ScheduledLoadState`
- All tank-backed WH: checkpoint `SKIN_LOSS_W`
- Battery: use start-of-step SOC/OCV in degradation accumulate (EA-006 F4)

### P5-E: Remaining telemetry field coverage (TP remaining gaps)

---

## Anti-patterns to avoid

- **Don't run BESTEST before Phase 2 is complete** — it will fail for 20 reasons
- **Don't fix individual key mismatches** — P0-A (CFG-007 through CFG-016) eliminates them all at once
- **Don't add integration tests for broken equipment** — fix the equipment first
- **Don't chase parity percentages** — fix the physics, parity follows
- **Don't fix output formatting before the values are correct**
- **Don't implement P5-B `catch_unwind` last** — it belongs at the start of P5-B to prevent process aborts during all subsequent Python testing
