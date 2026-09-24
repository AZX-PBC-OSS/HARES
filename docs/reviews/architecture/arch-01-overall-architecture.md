# Overall architecture assessment: crate dependency graph, data flow, coupling

**Review ID**: arch-01
**Category**: architecture
**Date**: 2026-05-26

## Files Reviewed

- `crates/hares-types/Cargo.toml`, `crates/hares-types/src/lib.rs`
- `crates/hares-physics/Cargo.toml`, `crates/hares-physics/src/lib.rs`
- `crates/hares-envelope/Cargo.toml`, `crates/hares-envelope/src/lib.rs`
- `crates/hares-control/Cargo.toml`, `crates/hares-control/src/lib.rs`
- `crates/hares-equipment/Cargo.toml`, `crates/hares-equipment/src/lib.rs`
- `crates/hares-io/Cargo.toml`, `crates/hares-io/src/lib.rs`
- `crates/hares-core/Cargo.toml`, `crates/hares-core/src/lib.rs`
- `crates/hares-fleet/Cargo.toml`, `crates/hares-fleet/src/lib.rs`
- `crates/hares-tariff/Cargo.toml`, `crates/hares-tariff/src/lib.rs`
- `crates/hares-python/Cargo.toml`, `crates/hares-python/src/lib.rs`
- `crates/hares-core/src/dwelling/mod.rs` (6041 lines)
- `crates/hares-core/src/environment.rs` (2800 lines)
- `crates/hares-types/src/environment.rs` (775 lines)

## Vendor/Reference Files Consulted

### OCHRE (Python) — `vendors/OCHRE/ochre/`
- `Simulator.py` (449 lines): Base class defining timing, I/O, sub-simulator composition. Inherited by `Dwelling` and `Equipment`.
- `Dwelling.py` (371 lines): Subclass of `Simulator`; owns an `Envelope` model and a collection of `Equipment` instances. Clear compositional relationship.
- `Equipment/Equipment.py` (316 lines): Base class for all equipment; subclass of `Simulator`. Each equipment type (`HVAC`, `WaterHeater`, `Battery`, `PV`, `EV`) lives in its own file.

### EnergyPlus (C++) — `vendors/EnergyPlus/src/EnergyPlus/`
- `Data/EnergyPlusData.hh` (617 lines): Central `EnergyPlusData` struct containing 190+ `std::unique_ptr<...>` members, each pointing to a `Data*` struct. This is the classic EnergyPlus monolith: every simulation subsystem's state lives in a single root struct.
- `DataGlobals.hh`, `DataHeatBalance.hh` (2059 lines), `DataSurfaces.hh` (1850 lines), `DataSizing.hh` (1437 lines): Large individual Data structs with deeply nested sub-structures.

---

## Findings

### Finding 1: Layering violation — `hares-io` depends on `hares-equipment` [Severity: high]

**Description**: The expected strict layering is `types < io < equipment < core < fleet/py`. In practice, `hares-io` declares `hares-equipment` as a dependency in its `Cargo.toml` (line 12: `hares-equipment = { workspace = true }`) and imports equipment types pervasively. This inverts the layer order, making `io` sit above `equipment` in the dependency graph.

The actual dependency graph is:
```
types
 ├─ physics
 ├─ control
 ├─ tariff
 ├────────> envelope  (depends on types, physics)
 │  └──────> equipment (depends on types, physics, control)
 │      └──> io        (depends on types, physics, envelope, control, equipment)  ← INVERSION
 │          └> core    (depends on types, physics, envelope, control, equipment, io, tariff)
 │              └> fleet (depends on core, io, types)
 │                  └> python (depends on all)
```

**Code Location**:
- `crates/hares-io/Cargo.toml:12` — declares `hares-equipment` dependency
- `crates/hares-io/src/hpxml/resolve_hvac.rs:8-20` — imports `EquipmentConfig`, `CentralAirConditionerConfig`, `RoomAcConfig`, `HeatPumpConfig`, `GasFurnaceConfig`, etc.
- `crates/hares-io/src/hpxml/resolve_der.rs:5` — imports `BatteryConfig`, `EvConfig`, `GeneratorConfig`, `PvConfig`
- `crates/hares-io/src/hpxml/resolve_water_heater.rs:5-9` — imports `ElectricResistanceWaterHeaterConfig`, `GasWaterHeaterConfig`, etc.
- `crates/hares-io/src/schedule_resolve.rs:10` — imports equipment config types for schedule injection

**Root Cause**: HPXML parsing/resolution logic in `hares-io` directly constructs equipment config structs (`*Config` types owned by `hares-equipment`). The parse-to-config workflow couples io to equipment-specific types. Equipment should not need to know about IO, but IO currently knows about equipment. The resolution logic (`resolve_hvac`, `resolve_der`, `resolve_water_heater`) crosses the intended boundary.

**Impact**: The configuration → equipment data flow has unclear boundaries. When equipment config types change, io (the parser) must be updated simultaneously, creating coupling risk. The intended "configuration enters at io, flows to core, then to equipment" pipeline is compromised because io directly instantiates equipment-layer types. This is not a circular dependency (no crate depends on itself), but it does violate the prescribed layering.

**Comparison to OCHRE**: OCHRE's HPXML parsing lives in `utils.py` (a separate utility layer), and equipment config is constructed independently — the parser returns dictionaries that the Dwelling feeds into Equipment constructors. There's a clean data-pass boundary: raw parsed data (plain dicts) → equipment config objects.

**Comparison to EnergyPlus**: EnergyPlus has no equivalent layer separation because *everything* is in the `EnergyPlusData` monolith — the parser modules write directly into the central state struct. HARES's architecture is structurally better than EnergyPlus's, but the io → equipment dependency bleed represents a regression toward that pattern.

---

### Finding 2: God object — `Dwelling` in `hares-core/src/dwelling/mod.rs` at 6041 lines [Severity: high]

**Description**: `crates/hares-core/src/dwelling/mod.rs` is 6041 lines (plus 4 submodules totaling 5640 lines, bringing the `dwelling/` directory to ~11,700 lines). The `Dwelling` struct orchestrates equipment lifecycle, thermal/humidity/electrical/fluid solvers, control dispatch, output column indexing, pricing/tariff evaluation, state checkpointing, invariant checking, and profiling. It imports from all sibling crates: `hares-types`, `hares-physics`, `hares-envelope`, `hares-control`, `hares-equipment`, `hares-io`, `hares-tariff`, and the `chrono`/`rand` family.

**Code Location**:
- `crates/hares-core/src/dwelling/mod.rs` — 6041 lines (entire file)
- Submodules: `solver_builder.rs` (2039), `conversions.rs` (1625), `synthetic.rs` (1227), `autosize.rs` (747)

**Root Cause**: The `Dwelling` struct directly implements too many concerns — solver construction, per-timestep orchestration, control signal dispatch, output serialization, tariff billing, and checkpointing — with no delegation to dedicated sub-components (besides the existing submodules). Control dispatch (`ControlDispatcher`, lines 339-500+), column-index building (lines 164-304, 248-304), and output recording are all mixed into the same struct.

**Impact**: Code navigation and testing are difficult when one module spans 6000+ lines. Changes to any concern risk affecting unrelated behavior. The dwelling is de facto the simulation monolith — the `core` crate is conceptually the integration layer, but the `dwelling` module itself concentrates too much logic.

**Comparison to OCHRE**: OCHRE's `Dwelling` class (371 lines) delegates to separate `Envelope` model, `Equipment` instances, and analysis modules. The `Simulator` base class handles timing. Each concern is a separate object. HARES's `Dwelling` has ~30x more code despite conceptually doing the same thing.

**Comparison to EnergyPlus**: EnergyPlus's `EnergyPlusData` struct (617 lines) centralizes *state* but not *logic* — individual manager classes in separate files own the behavior (e.g., `HVACManager.hh`, `HeatBalanceManager.hh`). HARES's `dwelling/mod.rs` concentrates both state and behavior in one file, which is arguably worse than EnergyPlus's pattern of splitting logic across files even when state is centralized.

---

### Finding 3: Pervasive oversize modules (>500 lines) across all crates [Severity: medium]

**Description**: The 500-line threshold is exceeded by 65+ source files (excluding tests), some by an order of magnitude. The largest non-test modules are:

| File | Lines | Concern |
|------|-------|---------|
| `hares-equipment/src/hvac/heat_pump/heater.rs` | 6683 | Heat pump heating model |
| `hares-core/src/dwelling/mod.rs` | 6041 | Dwelling orchestrator |
| `hares-io/src/hpxml/resolve_hvac.rs` | 5818 | HPXML HVAC resolution |
| `hares-envelope/src/thermal_solver/mod.rs` | 5592 | Thermal solver |
| `hares-io/src/hpxml/building.rs` | 5542 | Building geometry parsing |
| `hares-equipment/src/hvac/air_conditioner.rs` | 4753 | Air conditioner model |
| `hares-equipment/src/battery/mod.rs` | 4064 | Battery model |
| `hares-equipment/src/hvac/hvac_core.rs` | 3354 | HVAC core state machine |
| `hares-envelope/src/boundary_rc.rs` | 3326 | Boundary RC model |
| `hares-equipment/src/water_heater/heat_pump_wh.rs` | 3306 | HPWH model |
| `hares-equipment/src/event_load.rs` | 3043 | Event-based load |
| `hares-core/src/environment.rs` | 2800 | Environment manager |
| `hares-equipment/src/hvac/ideal_hvac.rs` | 2780 | Ideal HVAC |
| `hares-equipment/src/generator.rs` | 2675 | Generator model |

**Code Location**: See table above for exact paths.

**Root Cause**: Individual equipment models and parsers have accumulated extensive logic — parameter initialization, curve lookups, state transitions, thermal calculations, and diagnostic output — in single monolithic files rather than extracting sub-problems into composable submodules.

**Impact**: Large files reduce code discoverability and make review harder. Refactoring risk increases because internal coupling within the file is invisible at the crate boundary. Inline test code (e.g., in `resolve_hvac.rs` with `#[cfg(test)]` blocks from line 3837 onward) further bloats source files.

**Comparison to OCHRE**: OCHRE's equipment files are 30-700 lines each; the largest (`HVAC.py` at ~600 lines) is still manageable. HARES's Rust implementations are 5-10x larger because they carry temperature-dependent curve evaluation, staging logic, and detailed physics inline.

---

### Finding 4: `EnvironmentState` as shared mutable bag — boundary erosion [Severity: medium]

**Description**: `EnvironmentState` (`crates/hares-types/src/environment.rs:312-343`) aggregates 13 fields including `zones`, `weather`, `grid`, `custom_domains`, `equipment_telemetry`, `equipment_core`, `current_time`, `time_res`, `price_signal`, and `electrical` summary. It is passed by `&EnvironmentState` to nearly every `Equipment::step()`, `Equipment::init()`, `Equipment::update_control()` call, and to solver interfaces. Individual equipment implementations must know which subset of these fields they depend on, but there is no static enforcement — a water heater that only needs `weather.outdoor_temp_c` receives the entire envelope state.

**Code Location**:
- `crates/hares-types/src/environment.rs:312-343` — struct definition
- `crates/hares-equipment/src/lib.rs:105-111` — `Equipment::step()` signature accepting full `&EnvironmentState`
- `crates/hares-core/src/dwelling/mod.rs:21-44` — imports all sub-layers to construct and pass `EnvironmentState`

**Root Cause**: The environment state struct grew organically as new features (price signals, electrical summaries, equipment telemetry) were added. Rather than defining a small `WeatherContext` for physics, a `ZoneContext` for equipment thermal decisions, and a `PricingContext` for tariff-aware actors, all of these landed in a single struct.

**Impact**: Implicit coupling between equipment and unrelated environment fields. When the environment struct changes shape, all 10 equipment implementations recompile. Test setup requires constructing a full `EnvironmentState` (as seen in `hares-types/src/lib.rs:39-102` and `hares-equipment/src/lib.rs:404-451`), even for tests that only need one field.

**Comparison to EnergyPlus**: This is a much cleaner version of the EnergyPlus `EnergyPlusData` pattern. EnergyPlus's monolith carries 190+ data structs via raw pointers, and every function receives `EnergyPlusData &state`. HARES's `EnvironmentState` is ~775 lines vs EnergyPlus's 617 lines of pointer declarations (but thousands more if you count the actual Data struct sizes). HARES is on a better path, but the drift toward "pass everything" erodes the compositional boundary.

---

### Finding 5: No circular dependencies — acyclic crate graph confirmed [Severity: none (positive finding)]

**Description**: All dependencies are unidirectional. The dependency graph is a strict DAG:

```
types (leaf)
  ↑
physics, control, tariff
  ↑
envelope ← types+physics
equipment ← types+physics+control
  ↑
io ← types+physics+envelope+control+equipment
  ↑
core ← types+physics+envelope+control+equipment+io+tariff
  ↑
fleet ← core+io+types
  ↑
python ← core+fleet+control+equipment+io+physics+tariff+types
```

No crate transitively depends on itself. No `use hares_core` appears in `hares-io`, `hares-equipment`, or `hares-envelope`. No `use hares_fleet` appears in `hares-core` (only as a dev-dependency for benchmarks). The directed acyclic property is properly maintained.

**Code Location**: All 10 `Cargo.toml` files verified for `workspace = true` internal deps.

**Comparison to Both Vendors**: This is a structural advantage over EnergyPlus (which has no crate boundaries at all) and matches the spirit of OCHRE's module imports (which are also acyclic, thanks to Python's import system preventing import loops at runtime in most cases). HARES's DAG is well-ordered and clean.

---

### Finding 6: Telemetry key constants centralized in `hares-types` — appropriate ownership [Severity: none (positive finding)]

**Description**: The `hares-types/src/telemetry_keys.rs` file (231 lines) defines all shared telemetry key string constants in one place (`pub const ELECTRIC_KW: &str = "electric_kw"`, `pub const SOC: &str = "soc"`, etc.). This eliminates typo-class bugs where different modules use different string literals for the same telemetry field. Import conventions use `hares_types::telemetry_keys as tk` for concision.

**Code Location**: `crates/hares-types/src/telemetry_keys.rs:1-231`

**Impact**: String-key telemetry is inherently fragile, but centralizing keys in the types crate is the best available mitigation within the current architecture. Future work could replace string-keyed telemetry with a typed schema (enum-based or arrow-schema-based) to eliminate runtime key mismatch risk entirely.

---

### Finding 7: `hares-control` is well-isolated with minimal dependencies [Severity: none (positive finding)]

**Description**: The `hares-control` crate declares only `hares-types` as an internal dependency (plus external `bitflags`, `serde`, `thiserror`, `tracing`). It defines control signal types (`ControlSignal`), dispatch infrastructure (`DispatchRequest`, `DispatchTarget`, `PriorityTier`), capabilities bitmasking (`ControlCapabilities`), and OCHRE compatibility mapping (`compat::ochre_signal_to_control`). This clean isolation means control logic can evolve independently of physics, equipment, and I/O.

**Code Location**:
- `crates/hares-control/Cargo.toml:8-13` — deps: only `hares-types`
- `crates/hares-control/src/lib.rs:1-13` — module structure

**Comparison to OCHRE**: OCHRE does not have a separate control layer; control logic is mixed into `Equipment.update_external_control()` and `update_internal_control()` methods. HARES's extraction of control into its own crate is architecturally superior, enabling the actor-based dispatch system in `hares-core`.

---

## Summary

- **Total findings**: 7 (4 actionable, 3 positive)
- **Critical**: 0
- **High**: 2 (layering violation io→equipment, God object dwelling/mod.rs at 6041 lines)
- **Medium**: 2 (65+ modules >500 lines, EnvironmentState boundary erosion)
- **Low**: 0
- **Positive/Negative**: 3 (acyclic DAG, centralized telemetry keys, well-isolated control crate)

## Recommendations

1. **Resolve the io → equipment dependency inversion (Finding 1)**: Introduce an intermediate `hares-config` (or `hares-schema`) crate that defines only *data-transfer types* (plain structs with `Serialize`/`Deserialize` but no behavioral methods). Both `hares-io` (to populate them from HPXML) and `hares-equipment` (to construct Equipment from them) would depend on it, but neither depends on the other. This restores the intended layering: types < config < io, and types < config < equipment, with no cross-edge.

2. **Decompose `dwelling/mod.rs` (Finding 2)**: Extract the following concerns into dedicated crates or submodules:
   - `ControlDispatcher` → `hares-control` or a new `hares-core/src/control/dispatcher.rs`
   - Output/column indexing (`EquipmentColumns`, `ZoneColumnCaches`, `build_*` fns) → `hares-io/src/output/` or `hares-core/src/dwelling/output.rs`
   - Tariff billing/pricing → already in `hares-tariff`, but dwelling-side integration logic could move to a `BillingManager` wrapper in `hares-core`
   - Profiling instrumentation → `hares-core/src/dwelling/profiling.rs` (feature-gated)

3. **Split oversize equipment modules (Finding 3)**: Prioritize the largest offenders (>2000 lines):
   - `hvac/heat_pump/heater.rs` (6683 lines) — extract defrost logic, compressor map evaluation, and diagnostic output into separate submodules
   - `battery/mod.rs` (4064 lines) — extract degradation model, OCV table, and thermal model
   - `water_heater/heat_pump_wh.rs` (3306 lines) — extract compressor model, tank thermal stratification

4. **Introduce narrower context types (Finding 4)**: Define `WeatherContext`, `ZoneContext`, and `PricingContext` as immutable views derived from `EnvironmentState`. Equipment and solvers should accept only the context they need (e.g., `fn step(&mut self, zones: &ZoneContext, weather: &WeatherContext, dt: Duration)`) rather than the full environment. This narrows the recompile boundary and makes dependency intent explicit in function signatures.

5. **Adopt a module-level size lint**: Configure `rustc` or `clippy` to warn when individual `.rs` files exceed 500 lines (using a script in CI), with a whitelist for files currently being refactored. This prevents future growth of oversize modules.

## References / Citations

- **OCHRE Architecture**: `vendors/OCHRE/ochre/Simulator.py:22-80` — `Simulator` base class defining timing + sub-simulator tree; `Dwelling.py:28-80` — `Dwelling(Simulator)` with `Envelope` and `Equipment` composition; `Equipment/Equipment.py:10-60` — `Equipment(Simulator)` base with mode/zone/power interface.
- **EnergyPlus Monolith**: `vendors/EnergyPlus/src/EnergyPlus/Data/EnergyPlusData.hh:315-617` — `EnergyPlusData` struct with 190+ `unique_ptr<Data*>` members; `DataHeatBalance.hh:1-2059` — largest individual Data struct.
- **HARES Dependency Graph**: `Cargo.toml:1-44` (workspace members + workspace deps); all `crates/*/Cargo.toml` files verified.
- **HARES Equipment Trait**: `crates/hares-equipment/src/lib.rs:101-235` — `Equipment` trait definition (17 methods, including control, LUT, actor seed).
- **HARES EnvironmentState**: `crates/hares-types/src/environment.rs:312-343` — full struct with 13 fields.
- **HARES Telemetry Keys**: `crates/hares-types/src/telemetry_keys.rs:1-231` — centralized string constants.
