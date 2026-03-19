---
id: HARES-001
title: "hares-types — Core IDs, Enums, and Error Types"
kind: implement
depends_on: []
files_to_touch:
  - crates/hares-types/src/equipment.rs
  - crates/hares-types/src/error.rs
  - crates/hares-types/src/lib.rs
references:
  - docs/architecture/02-equipment-and-ports.md
verification:
  - cargo check -p hares-types
  - cargo test -p hares-types
  - cargo clippy -p hares-types -- -D warnings
---

## Background/Context
All other crates in HARES depend on a shared vocabulary of IDs, enums, and error types. Establishing these in `hares-types` first avoids circular dependencies and gives every crate a stable, serde-compatible foundation.

## Work to Do
- [ ] Define `EquipmentId(u32)` newtype deriving `Hash, Eq, Copy, Clone, Debug`
- [ ] Define `EquipmentDescriptor` struct with fields: `id`, `name`, `end_use`, `equipment_type: String`, `zone: Option<ZoneId>`, `fuel`, `stage`, `control_capabilities: ControlCapabilities`, `telemetry_fields: Vec<TelemetryField>`
- [ ] Define `TelemetryField` struct with fields: `name: String`, `unit: String`, `description: String`
- [ ] Define `EndUse` enum with variants: `HvacHeating`, `HvacCooling`, `WaterHeating`, `Lighting`, `PlugLoads`, `Refrigeration`, `Ventilation`, `Battery`, `PV`, `EV`, `Generator`, `Dehumidifier`, `Other`
- [ ] Define `FuelType` enum with variants: `Electric`, `Gas`, `Propane`, `Oil`, `None`
- [ ] Define `ExecutionStage` enum with four variants: `Independent`, `Electrical`, `Thermal`, `EnvelopeResolution` — the fourth variant is used by domain solvers (thermal, humidity, electrical, fluid) that run after all equipment
- [ ] Define `OperatingMode` enum with variants: `Off`, `Heating`, `Cooling`, `Defrost`, `Standby`, `Charging`, `Discharging`, `HeatingHP`, `HeatingER`, `HeatingHPAndER`, `HeatPumpWH`, `BackupElement`
- [ ] Define `DomainId(u16)` newtype — this is the canonical definition; HARES-014 must import it, not redefine it
- [ ] Define `LoopId(u16)` newtype
- [ ] Define `ProtocolId(u16)` newtype
- [ ] Define `FluidType` enum with variants: `Water`, `Glycol`, `Refrigerant`
- [ ] Define `HaresError` enum with domain variants: `Physics`, `Envelope`, `Equipment`, `Io`, `Control` — use `#[derive(thiserror::Error)]`
- [ ] Define `SimError` enum with variants: `Physics(HaresError)`, `Panic(String)`, `Timeout` — used by fleet execution for per-dwelling error isolation
- [ ] Derive `serde::Serialize` and `serde::Deserialize` on all public types
- [ ] Re-export all types from `lib.rs`

## Files to Touch
- `crates/hares-types/src/equipment.rs`: new file — `EquipmentId`, `EquipmentDescriptor`, `TelemetryField`, `EndUse`, `FuelType`, `ExecutionStage`, `OperatingMode`
- `crates/hares-types/src/error.rs`: new file — `HaresError` enum
- `crates/hares-types/src/lib.rs`: re-export all modules

## Measures of Success
- [ ] All types compile with `cargo check -p hares-types`
- [ ] `EquipmentId` derives `Hash, Eq, Copy, Clone, Debug`
- [ ] `equipment_type` field is `String` (avoids `Box::leak` memory leak required by `&'static str` deserialization)
- [ ] `TelemetryField` struct has exactly three fields: `name`, `unit`, `description` (all `String`)
- [ ] `ExecutionStage` has exactly four variants: `Independent`, `Electrical`, `Thermal`, `EnvelopeResolution`
- [ ] `OperatingMode` has exactly twelve variants as specified
- [ ] `EndUse` has exactly thirteen variants as specified
- [ ] `DomainId`, `LoopId`, `ProtocolId` are `u16` newtypes (not raw `u16` aliases)
- [ ] `DomainId` is a newtype struct, not a type alias
- [ ] `FluidType` has exactly three variants: `Water`, `Glycol`, `Refrigerant`
- [ ] `FuelType::None` variant exists and is distinct from Rust's `Option::None`
- [ ] All types round-trip through serde JSON without loss
- [ ] `HaresError` covers all five domain variants and uses `#[derive(thiserror::Error)]`
- [ ] `EquipmentDescriptor::zone` is `Option<ZoneId>`, not `Option<String>`
- [ ] `control_capabilities` field is `ControlCapabilities` (from HARES-004), not raw `u32`
- [ ] `SimError` has three variants and derives `Debug`, `Display` via `thiserror`

## Verification
- [ ] `cargo check -p hares-types` passes
- [ ] `cargo test -p hares-types` passes
- [ ] `cargo clippy -p hares-types -- -D warnings` passes
