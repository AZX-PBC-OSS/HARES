# Port name and declaration consistency across equipment, solver, and actors
**Review ID**: wiring-01
**Category**: wiring
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-types/src/ports.rs` — Core `PortDeclaration`, `PortContribution`, `PortSlots`, accumulators
- `crates/hares-equipment/src/ports.rs` — Empty stub (1-line comment only)
- `crates/hares-envelope/src/thermal_solver/ports.rs` — Thermal solver port consumption

## Vendor/Reference Files Consulted
- `vendors/OCHRE/ochre/Equipment/Equipment.py` — OCHRE base equipment port model (zone_name, sensible/latent gains)
- `vendors/OCHRE/ochre/Equipment/HVAC.py` — OCHRE HVAC port abstraction

## Architecture Summary

HARES does NOT use string-based port names. Ports are identified by the `PortDeclaration` struct (`hare-types/src/ports.rs:105-112`) containing typed fields: `port_type: PortType`, `zone: Option<ZoneId>`, `loop_id: Option<LoopId>`, `domain_id: Option<DomainId>`, `fluid_type: Option<FluidType>`. Equipment declares ports via `fn ports(&self) -> &[PortDeclaration]` (`Equipment` trait, `lib.rs:103`). The dwelling aggregates all declarations and builds a shared `PortSlots` (`from_declarations`, `ports.rs:441-498`). Equipment writes contributions via `ports.accumulate()`. Solvers read from the shared `PortSlots`.

This is a structurally sound design that prevents the class of string-typo bugs common in dynamically-typed simulation frameworks. However, several wiring issues exist at the declaration-vs-usage level.

---

## Findings

### Finding 1: `rebuild_thermal_ports` unconditionally adds humidity ports to non-dehumidifying HVAC equipment
**Severity**: High

**Description**: The `rebuild_thermal_ports` function (`duct_distribution.rs:141-148`) calls `PortDeclaration::humidity(zone)` for every zone in `zone_heat_fractions`, regardless of whether the equipment writes humidity contributions. This function is called by six HVAC equipment types during `init()`, but only two of them (AirConditioner and HP Coolers) actually write humidity via `PortContribution::Humidity`.

**Code Location**:
- **Declaration site**: `crates/hares-equipment/src/hvac/duct_distribution.rs:141-148`
  ```rust
  pub fn rebuild_thermal_ports(&self, ports: &mut Vec<PortDeclaration>) {
      use hares_types::PortType;
      ports.retain(|p| p.port_type != PortType::Thermal && p.port_type != PortType::Humidity);
      for &(zone, _) in &self.config.zone_heat_fractions {
          ports.push(PortDeclaration::thermal(zone));
          ports.push(PortDeclaration::humidity(zone)); // ← unconditionally added
      }
  }
  ```
- **Callers that write humidity** (correct):
  - `AirConditioner` / `RoomAC` init: `air_conditioner.rs:621` — writes humidity at `air_conditioner.rs:814`
- **Callers that NEVER write humidity** (orphaned):
  - `ElectricBoiler` init: `boiler.rs:200` — only writes Electrical, Fluid, Thermal (via `write_zone_thermal_contributions`)
  - `GasBoiler` init: `boiler.rs:480` — only writes Fuel, Electrical, Fluid, Thermal
  - `ElectricFurnace` init: `furnace.rs:149` — only writes Electrical, Thermal
  - `GasFurnace` init: `furnace.rs:437` — only writes Fuel, Electrical, Thermal
  - `HeatPumpHeaterCore` (ASHP/MSHP/GSHP Heater) init: `heater.rs:730` — only writes Electrical, Fuel(backup), Thermal

**Root Cause**: `rebuild_thermal_ports` was designed to cover the common case of ducted A/C equipment (which produces both thermal and humidity outputs) but was naively reused by heating-only equipment that also has duct zones. The function has no parameter to control whether humidity ports should be created.

**Impact**: Extra `HumidityAccumulator` entries are allocated in `PortSlots` for zones that never receive humidity contributions. The accumulators remain zero throughout simulation. While the humidity solver (`humidity_solver.rs:160-165`) sums from all humidity accumulators filtered by zone, a zero accumulator is harmless. However:
- Memory is wasted (one `HumidityAccumulator` per unused zone per equipment)
- The wiring contract between declarations and actual writes is violated silently
- Future maintainers may assume these ports are used and depend on their existence
- In a configuration with only a furnace (no A/C), the humidity solver could receive no humidity contributions even though accumulators exist — a misleading state

### Finding 2: `PortType::Custom` / `PortContribution::Custom` has zero production usage
**Severity**: Medium

**Description**: The `Custom` port type (defined at `ports.rs:87-91`, factory at `ports.rs:155-163`, accumulator at `ports.rs:372-395`) has no equipment declaring it and no built-in solver consuming it. The only usages are in unit tests (`ports.rs:985, 1049-1051`, `port_accumulation_tests.rs:333`).

**Code Location**:
- Type definition: `crates/hares-types/src/ports.rs:87-91` (`PortContribution::Custom`), `ports.rs:99-101` (`PortType::Custom`)
- Factory: `ports.rs:155-163` (`PortDeclaration::custom()`)
- Accumulator: `ports.rs:372-395` (`CustomAccumulator`)
- `PortSlots::from_declarations` handler: `ports.rs:465-474`
- `PortSlots::accumulate` handler: `ports.rs:570-581`

**Root Cause**: The `Custom` port type was designed for extensibility (user-registered `DomainSolver` implementations) but no built-in equipment or solver uses it. The feature exists as forward-looking infrastructure without any consumer.

**Impact**: Dead code in the port accumulation hot path. The `from_declarations` and `accumulate` methods handle `PortType::Custom` / `PortContribution::Custom` despite never being exercised in production. This adds branches that are always cold and increases code comprehension burden. If a user-provided `DomainSolver` expects a custom port to exist but no equipment declares it, the solver silently receives zero contributions (no error, no warning).

**Comparison to OCHRE**: OCHRE (Python) has no equivalent "custom" port abstraction — all equipment outputs are fixed (electric_kw, reactive_kvar, gas_therms_per_hour, sensible_gain, latent_gain). HARES's `Custom` port is a forward-looking extension beyond the OCHRE reference.

### Finding 3: Hardcoded ZoneId values in ScheduledLoad zone auto-routing assume fixed zone numbering
**Severity**: Medium

**Description**: The `ScheduledLoad::update_ports()` function (`scheduled_load.rs`) assigns zones based on hardcoded `ZoneId` values: `ZoneId(1)` for indoor, `ZoneId(2)` for garage, `ZoneId(3)` for basement. This mirrors OCHRE's zone name conventions (`Equipment.py:43-50`) but couples the equipment model to a specific zone numbering that may not hold in all dwelling configurations.

**Code Location**: `crates/hares-equipment/src/scheduled_load.rs` — `update_ports()` function (search for "Garage", "Basement", "ZoneId(2)", "ZoneId(3)")

**Root Cause**: The zone assignment logic was directly translated from OCHRE's string-based zone name matching (OCHRE `Equipment.py:43-50`: `"Garage" → "Garage"`, `"Basement" → "Foundation"`) into HARES's integer-based `ZoneId` system. But unlike OCHRE where zone names are looked up from the envelope model, HARES hardcodes the numeric IDs.

**Impact**: If a dwelling configuration uses non-standard zone numbering (e.g., ZoneId(5) for garage), scheduled loads would write thermal contributions to the wrong zone or not write them at all (if zone is `None`). The thermal solver would receive gains in the wrong zone's accumulator. This is a silent data-routing bug with no runtime error.

### Finding 4: No compile-time or init-time validation of "declared but unwritten" ports
**Severity**: Low

**Description**: The `PortSlots::accumulate()` method (`ports.rs:517-597`) returns `Err` when equipment writes to an undeclared zone/loop/domain. This is a runtime check that catches the "undeclared write" direction. However, there is no corresponding check for the reverse: equipment that declares a port but never writes to it. Such orphaned declarations are silently accepted.

**Code Location**: `crates/hares-types/src/ports.rs:441-498` (`from_declarations`) — creates accumulators; `ports.rs:517-597` (`accumulate`) — only validates the write side

**Root Cause**: The port wiring system was designed as a one-way validation (guarding against missing accumulators at write time) rather than a bidirectional contract check. This is a pragmatic choice (checking unwritten declarations would require post-simulation analysis or instrumentation), but it leaves the door open for the kind of orphaned declarations seen in Finding 1.

**Impact**: Low immediate impact, since unused accumulators are zero and do not corrupt solver results. However, they violate the principle of least surprise and may mask genuine configuration errors where an equipment intentionally omits a port write.

### Finding 5: DHW_DEMAND_LOOP uses a well-known sentinel value without namespace isolation
**Severity**: Low

**Description**: `DHW_DEMAND_LOOP` is defined as `LoopId(u16::MAX - 1)` (`water_heater/mod.rs:32`). This value is exposed as a public constant in `hares_equipment::DHW_DEMAND_LOOP` (`lib.rs:73`) and used across multiple equipment types (water heaters, wet appliances) for inter-equipment DHW demand communication. However, this sentinel value occupies the `u16` namespace without any guard against collisions with user-configured loop IDs.

**Code Location**:
- Definition: `crates/hares-equipment/src/water_heater/mod.rs:32`
- Re-export: `crates/hares-equipment/src/lib.rs:73`
- Consumers (declarations): `resistance.rs:162`, `heat_pump_wh.rs:214`, `gas.rs:156`, `tankless.rs:544`, `event_load.rs:1054-1057, 1204-1207`
- Reader: `water_heater/mod.rs:35-41` (`read_dhw_demand_kg_s`)

**Root Cause**: The sentinel value was chosen as a simple well-known constant rather than using a dedicated enum variant or a reserved range with validation.

**Impact**: If a user-provided configuration assigns a fluid loop with ID `u16::MAX - 1` (65534) to a hydronic or CHP loop, the `PortSlots::accumulate` would route the hydronic/CHP contributions into the DHW demand accumulator, and the DHW demand reader would incorrectly interpret them as water draw. The `PortSlots::from_declarations` deduplicates by `(loop_id, fluid_type)`, so both a user loop and `DHW_DEMAND_LOOP` using the same ID and `FluidType::Water` would share a single accumulator — a silent merge bug.

### Finding 6: OCHRE's string-based zone names vs HARES's integer-based ZoneId — no translation layer
**Severity**: Low

**Description**: OCHRE uses string-based zone names (`"Indoor"`, `"Garage"`, `"Foundation"`, `"Outdoor"` — `Equipment.py:43-50`) with lookup from the envelope model (`equipment.py:53-54`). HARES uses integer `ZoneId` values assigned at HPXML parse time. There is no central mapping between these two systems. Equipment that auto-routes to zones (ScheduledLoad, see Finding 3) uses hardcoded integer IDs based on the OCHRE naming convention, but this mapping is not enforced or validated against the actual zone configuration.

**Code Location**: `crates/hares-equipment/src/scheduled_load.rs` — zone auto-routing logic

**Root Cause**: The OCHRE-to-HARES translation performed by the HPXML importer (`hares-io` crate) assigns `ZoneId` values implicitly. Equipment models that replicate OCHRE's zone-routing logic re-derive the mapping independently, creating a potential for drift.

**Impact**: Same as Finding 3. The two independent zone-mapping implementations can diverge, causing misrouted thermal gains.

---

## Port Classification Summary

| Port Type | Declared By | Consumed By | Classification |
|-----------|------------|-------------|----------------|
| `Thermal` | All zone-capable equipment + all env zones at dwelling init | ThermalSolver, HumiditySolver, Dwelling output, Diagnostics, Observer | **Connected** |
| `Electrical` | Every equipment model | ElectricalSolver, Dwelling summary, Diagnostics, Observer | **Connected** |
| `Fuel` | Gas equipment only | Dwelling output, Observer, Python bindings | **Connected** |
| `Fluid` | Water heaters, boilers, generator(CHP), wet appliances | FluidSolver, Water heater demand readers, Observer | **Connected** |
| `Humidity` | A/C, HP Coolers, Dehumidifier, IdealHvac + spurious declarations (Finding 1) | HumiditySolver | **Partially orphaned** (Finding 1) |
| `Custom` | *(none — test only)* | *(none — user-registered DomainSolver only)* | **Orphaned** (Finding 2) |

### Per-Equipment Declaration Table

| Equipment | electrical | thermal | fuel | fluid | humidity | Status |
|-----------|:----------:|:-------:|:----:|:-----:|:--------:|--------|
| EventBasedLoad | ✅ | zone? | fuel? | - | - | Connected |
| WetAppliance | ✅ | zone? | fuel? | DHW? | - | Connected |
| ASHP/Minisp/GSHP Heater | ✅ | zone(s) | - | - | ✱zone(s)✱ | **Humidity orphaned** |
| HP Cooler (ASHP/MSHP/GSHP) | ✅ | zone(s) | - | - | ✅ | Connected |
| AirConditioner / RoomAC | ✅ | zone(s) | - | - | ✅ | Connected |
| ElectricBaseboard | ✅ | zone | - | - | - | Connected |
| ElectricBoiler | ✅ | zone(s) | - | loop:Water | ✱zone(s)✱ | **Humidity orphaned** |
| GasBoiler | ✅ | zone(s) | ✅ | loop:Water | ✱zone(s)✱ | **Humidity orphaned** |
| ElectricFurnace | ✅ | zone(s) | - | - | ✱zone(s)✱ | **Humidity orphaned** |
| GasFurnace | ✅ | zone(s) | ✅ | - | ✱zone(s)✱ | **Humidity orphaned** |
| Dehumidifier | ✅ | zone | - | - | ✅ | Connected |
| ResistanceWH | ✅ | zone | - | loop+DHW:Water | - | Connected |
| HeatPumpWH | ✅ | zone | - | loop+DHW:Water | - | Connected |
| GasWH | ✅ | zone | ✅ | loop+DHW:Water | - | Connected |
| TanklessWH (Elec) | ✅ | - | - | DHW:Water | - | Connected |
| TanklessWH (Gas) | ✅ | - | ✅ | DHW:Water | - | Connected |
| Battery | ✅ | zone? | - | - | - | Connected |
| EV | ✅ | - | - | - | - | Connected |
| PV | ✅ | - | - | - | - | Connected |
| Generator | ✅ | zone? | ✅ | loop:Water? | - | Connected |
| ScheduledLoad | ✅ | zone? | fuel? | - | - | Connected |
| Ventilation | ✅ | zone | - | - | - | Connected |
| IdealHvac | ✅ | zone | fuel? | - | ✅ | Connected |

**Key**: ✅ = declared and written; ✱...✱ = declared (via `rebuild_thermal_ports`) but never written; ? = conditional

---

## Summary

- **Total findings**: 6
- **High**: 1 (Finding 1 — spurious humidity declarations)
- **Medium**: 2 (Finding 2 — orphaned Custom port type; Finding 3 — hardcoded ZoneId assumptions)
- **Low**: 3 (Finding 4 — no bidirectional validation; Finding 5 — DHW_DEMAND_LOOP sentinel collision risk; Finding 6 — OCHRE zone name mismatch)

**No "missing" ports found**: Every port type consumed by a built-in solver has at least one equipment that declares and writes to it. No built-in solver expects a port that no equipment produces. Runtime errors from undeclared writes (`PortSlots::accumulate`, `ports.rs:529, 565, 578, 590`) protect against the "undeclared write" direction.

**No string-naming mismatches**: The port system uses typed structs rather than string identifiers, which eliminates the entire class of string-typo bugs. The non-string nature of the port system is itself a strength compared to frameworks that use text-based port names.

## Recommendations

1. **Add a `needs_humidity: bool` parameter to `rebuild_thermal_ports`** or split it into two functions: `rebuild_thermal_ports` (thermal only) and `rebuild_thermal_and_humidity_ports` (thermal + humidity). Furnaces, boilers, and HP heaters should call the thermal-only variant. This eliminates Finding 1.

2. **Add a `PortDeclaration::custom()` usage or remove the `Custom` port type** during the next refactoring cycle. If the extensibility path is intentional, add documentation and a compile-time feature flag. If not, dead-code elimination reduces maintenance burden. This addresses Finding 2.

3. **Add a `ZoneMap` abstraction** that translates OCHRE-style zone names (`"Indoor"`, `"Garage"`, `"Basement"`) into `ZoneId` at dwelling construction time, and have all equipment query this map rather than hardcoding integer IDs. This addresses Findings 3 and 6.

4. **Add an optional debug assertion or logger warning** for declared-but-never-written ports. This could be gated behind a `debug_assertions` or `cfg(feature = "observe")` flag. This addresses Finding 4.

5. **Reserve a dedicated `LoopId` range for well-known loops** (e.g., `0xFE00-0xFEFF` for framework internal use) and validate at configuration load that user-defined loop IDs do not collide. Add a `LoopId::is_reserved()` method. This addresses Finding 5.

## References / Citations

- OCHRE Equipment base class: `vendors/OCHRE/ochre/Equipment/Equipment.py:10-57` — zone_name conventions, is_electric/is_gas flags, sensible/latent gain fractions
- OCHRE HVAC base class: `vendors/OCHRE/ochre/Equipment/HVAC.py:63-80` — end_use-based heater/cooler dispatch
- HARES port types: `crates/hares-types/src/ports.rs:57-91` — `PortContribution` enum
- HARES port declarations: `crates/hares-types/src/ports.rs:105-174` — `PortDeclaration` struct and factories
- HARES port accumulation: `crates/hares-types/src/ports.rs:441-597` — `from_declarations` and `accumulate`
- HARES duct distribution: `crates/hares-equipment/src/hvac/duct_distribution.rs:90-148` — `write_zone_thermal_contributions` and `rebuild_thermal_ports`
- HARES thermal solver consumption: `crates/hares-envelope/src/thermal_solver/ports.rs:14-109` — `apply_port_convective_inputs` and `apply_port_radiant_inputs`
- HARES humidity solver consumption: `crates/hares-envelope/src/humidity_solver.rs:160-172` — moisture mass flow and latent gain reading
- HARES fluid solver consumption: `crates/hares-envelope/src/fluid_solver.rs:111-159` — loop grouping and net power computation
- HARES electrical solver consumption: `crates/hares-envelope/src/electrical_solver.rs:113-123` — load, generation, reactive power
- DHW demand loop: `crates/hares-equipment/src/water_heater/mod.rs:29-42` — constant definition and reader
