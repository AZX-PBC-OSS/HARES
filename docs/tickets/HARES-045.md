---
id: HARES-045
title: "hares-core — Telemetry and Checkpoint"
kind: implement
depends_on: [HARES-043]
files_to_touch:
  - crates/hares-core/src/telemetry.rs
  - crates/hares-core/src/checkpoint.rs
  - crates/hares-core/src/lib.rs
references:
  - docs/architecture/08-operations.md
  - docs/architecture/03-control-interfaces.md
verification:
  - cargo check -p hares-core
  - cargo test -p hares-core
  - cargo clippy -p hares-core -- -D warnings
---

## Background/Context
RL agents need a low-latency, zero-copy view of the current dwelling state (telemetry), and long-running fleet simulations need the ability to suspend and resume without restarting (checkpointing). Telemetry fields must be contiguous `f64` values so they can be wrapped in a numpy array without copying. Checkpoint writes must be atomic to prevent corruption from interrupted saves.

## Work to Do
- [ ] Implement `telemetry.rs`: `DwellingTelemetry` struct
  - [ ] Fields: `timestep_index: u64`, `current_time: DateTime<Utc>`, zone temperatures per zone (`Vec<f64>`), equipment modes and states, SOC values for battery and EV, power draws per equipment and aggregate total, current setpoints — `timestep_index` and `current_time` are required so Python controllers can determine simulation time without maintaining external state
  - [ ] All numeric fields stored as contiguous `f64` to enable zero-copy numpy conversion
  - [ ] `to_observation_vec(fields: &[&str]) -> Vec<f64>` — selects named fields by key and returns them as a flat contiguous `f64` vector for RL observation space construction (required by HARES-052). Valid field keys: `zone_temp[{zone_name}]`, `outdoor_temp`, `outdoor_rh`, `equipment_soc[{equip_name}]`, `equipment_power[{equip_name}]`, `setpoint_heat[{zone_name}]`, `setpoint_cool[{zone_name}]`. This is the contract with HARES-052.
  - [ ] Implement `Dwelling::telemetry() -> DwellingTelemetry`
- [ ] Implement `checkpoint.rs`: `DwellingCheckpoint` struct
  - [ ] Fields: `format_version: u32`, `bldg_id: i64`, `timestep_index: u64`, `equipment_states: Vec<(EquipmentId, Vec<u8>)>`, `rng_state: [u8; 32]`, `envelope_state: Vec<f64>` (RC state vector x[k]), `humidity_state: f64` (zone moisture content), `fluid_states: Vec<f64>` (hydronic loop state) — `format_version` is required per arch doc `08-operations.md`; checkpoints are valid only within the same build and must be rejected when `format_version` does not match the current binary's expected version. Define `const CHECKPOINT_VERSION: u32 = 1` in hares-core; increment whenever `DwellingCheckpoint` schema or any save_state format changes.
  - [ ] `save(path: &Path) -> Result<()>`: atomic write — write to a temp file alongside `path`, then rename into place
  - [ ] `load(path: &Path) -> Result<DwellingCheckpoint>`
  - [ ] `Dwelling::save_checkpoint(&self) -> DwellingCheckpoint`: snapshot current state
  - [ ] `Dwelling::load_checkpoint(&mut self, cp: DwellingCheckpoint) -> Result<()>`: restore state and resume from saved timestep
- [ ] Re-export `DwellingTelemetry` and `DwellingCheckpoint` from `lib.rs`

## Files to Touch
- `crates/hares-core/src/telemetry.rs`: new file — `DwellingTelemetry` struct
- `crates/hares-core/src/checkpoint.rs`: new file — `DwellingCheckpoint`, `save`, `load`, and `Dwelling` integration methods
- `crates/hares-core/src/lib.rs`: declare and re-export new modules

## Measures of Success
- [ ] `telemetry()` returns values for all expected fields (zone temps, equipment states, SOC, power, setpoints, `timestep_index`, `current_time`)
- [ ] `to_observation_vec` returns the correct values in the declared field order; requesting an unknown field name returns `Err`
- [ ] Save checkpoint → modify dwelling state → load checkpoint → verify state matches the saved snapshot
- [ ] Loading a checkpoint whose `format_version` does not match the current binary returns a descriptive `Err` rather than silently loading corrupt state
- [ ] An interrupted (partial) checkpoint write does not corrupt the previously saved file
- [ ] The RNG state is fully restored after `load_checkpoint`, producing the same subsequent random draws
- [ ] After save→resume, zone temperature trajectory matches reference run at same timestep within 0.01°C
- [ ] Loading a checkpoint with version N+1 vs current version N returns Err

## Architecture Alignment Notes
- **`DwellingCheckpoint` struct**: Extends the architecture's struct definition with `format_version`, `rng_state`, `envelope_state`, `humidity_state`, and `fluid_states` fields. These are required for deterministic resume and were not enumerated in the original architecture spec. The architecture doc `08-operations.md` should be updated to include these fields.
- **Observation field naming convention**: The architecture uses flat field names (e.g. `"battery_soc"`), while this ticket uses bracket notation (e.g. `"equipment_soc[Battery]"`, `"zone_temp[{zone_name}]"`). The bracket notation is necessary because a dwelling can have multiple zones and multiple SOC-bearing equipment. The mapping convention is: `"{category}[{instance_name}]"` where category is one of `zone_temp`, `equipment_soc`, `equipment_power`, `setpoint_heat`, `setpoint_cool`, and instance_name is the zone or equipment name. Flat names from the architecture (e.g. `"battery_soc"`) should be supported as aliases that resolve to `"equipment_soc[Battery]"` for single-instance cases.

## Verification
- [ ] `cargo check -p hares-core` passes
- [ ] `cargo test -p hares-core` passes
- [ ] `cargo clippy -p hares-core -- -D warnings` passes
