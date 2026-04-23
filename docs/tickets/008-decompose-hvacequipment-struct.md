# Decompose HvacEquipment Struct into Lifecycle-Grouped Sub-structs

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-equipment/hvac/hvac_core, hares-equipment/hvac/staging, hares-equipment/hvac/air_conditioner

## Problem

`HvacEquipment` is a god struct with 48 public fields mixing configuration, mutable runtime state, physics parameters, and control signal state. These have different lifecycles:

- **Config**: set once at init, immutable after — e.g., `biquadratic_coeffs`, `airflow_m3_s_per_w`, `shr`
- **Runtime state**: changes every step — e.g., `plf_state`, `startup`, `last_speed_index`
- **Control state**: changes on signal receipt — e.g., `max_capacity_fraction`, `disabled_speeds`

The lack of grouping makes invariants unverifiable at the type level. For example, the `mode` ↔ `mode_start_at` invariant (hvac_core.rs:138–143) is documented in a comment but not enforced by types — a caller can assign `self.mode` directly without updating `mode_start_at`, silently breaking minimum on/off time enforcement.

## Current Behavior

The full struct definition spans hvac_core.rs:122–249 (127 lines). The 48 public fields are (configuration: 28, runtime: 8, control: 3, thermostat: 9 — of which `min_on_time_s` and `min_off_time_s` are dual-listed under both configuration and thermostat because they are consumed by `ThermostatFsm` at runtime but set from config; total after ticket 006 extraction: 37 remaining fields in `HvacEquipment`). Field count verified by inspection of all `pub` field declarations at hvac_core.rs:122–249.

### Configuration fields (set once at init)
- `equipment_type: HvacEquipmentType` (line 123)
- `zone_id: ZoneId` (line 124)
- `shr: f64` (line 157)
- `fan_power_w_per_m3_s: f64` (line 156)
- `duct_dse: f64` (line 158)
- `duct_zone_id: Option<ZoneId>` (line 162)
- `basement_heat_frac: f64` (line 170)
- `basement_zone_id: Option<ZoneId>` (line 173)
- `airflow_m3_s_per_w: f64` (line 175)
- `biquadratic_coeffs: Vec<[f64; 6]>` (line 183)
- `biquadratic_x1_bounds: (f64, f64)` (line 184)
- `biquadratic_x2_bounds: (f64, f64)` (line 185)
- `speed_control_mode: SpeedControlMode` (line 186)
- `low_speed_capacity_fraction: f64` (line 187)
- `space_fraction: f64` (line 228)
- `cap_ff_coeffs: [f64; 3]` (line 236)
- `eir_ff_coeffs: [f64; 3]` (line 239)
- `ff_bounds: (f64, f64)` (line 244)
- `plf_min: f64` (line 248)
- `heating_capacities_w: Vec<f64>` (line 153)
- `cooling_capacities_w: Vec<f64>` (line 154)
- `eir_by_stage: Vec<f64>` (line 155)
- `eir_plr_coefficients: Option<Vec<[f64; 3]>>` (line 208)
- `zone_heat_fractions: Vec<(ZoneId, f64)>` (line 182)
- `min_time_per_speed_s: f64` (line 198)
- `min_on_time_s: f64` (line 148)
- `min_off_time_s: f64` (line 152)
- `supply_air_temp_c: f64` (line 174)

### Runtime state fields (change every step)
- `duty_cycle: f64` (line 127)
- `plf_cooling_degradation_coeff: f64` (line 188)
- `plf_state: f64` (line 189)
- `startup: StartupConfig` (line 190) — note: `c_d` is set from config, but `time_since_start_min` is runtime
- `last_speed_index: usize` (line 191)
- `last_speed_frac: f64` (line 192)
- `time_at_current_speed_s: f64` (line 196)
- `prev_zone_temp_c: Option<f64>` (line 224)

### Control state fields (change on signal receipt)
- `max_capacity_fraction: f64` (line 232)
- `disabled_speeds: Vec<bool>` (line 216)
- `max_enabled_speed: usize` (line 220)

### Thermostat fields (will be extracted by ticket 006)
- `thermostat: ThermostatConfig` (line 125) → `ThermostatFsm`
- `mode: ThermostatMode` (line 126) → `ThermostatFsm`
- `static_setpoints: ThermalSetpoints` (line 128) → `ThermostatFsm`
- `heating_setpoint_source: Option<ScheduleSource>` (line 130) → `ThermostatFsm`
- `cooling_setpoint_source: Option<ScheduleSource>` (line 132) → `ThermostatFsm`
- `schedule_setpoints: Option<ScheduleSetpoints>` (line 133) → `ThermostatFsm`
- `runtime_setpoints: Option<RuntimeSetpointOverride>` (line 134) → `ThermostatFsm`
- `last_mode_switch_at: Option<DateTime<FixedOffset>>` (line 135) → `ThermostatFsm`
- `mode_start_at: Option<DateTime<FixedOffset>>` (line 144) → `ThermostatFsm`

## Required Behavior

Group remaining fields (after ticket 006 extracts thermostat fields) into sub-structs with clear lifecycle boundaries. This makes invariants enforceable at the type level and makes the data flow visible in the struct layout. Pure refactor — no behavioral change.

## Approach

**Prerequisite**: Complete ticket 006 (ThermostatFsm extraction) first. This removes 11 fields and simplifies the decomposition.

### Step 1: Define `HvacConfig` — immutable after init

```rust
/// Equipment configuration set once at initialization and never modified at runtime.
#[derive(Clone, Debug)]
pub struct HvacConfig {
    pub equipment_type: HvacEquipmentType,
    pub zone_id: ZoneId,
    pub shr: f64,
    pub fan_power_w_per_m3_s: f64,
    pub duct_dse: f64,
    pub duct_zone_id: Option<ZoneId>,
    pub basement_heat_frac: f64,
    pub basement_zone_id: Option<ZoneId>,
    pub airflow_m3_s_per_w: f64,
    pub biquadratic_coeffs: Vec<[f64; 6]>,
    pub biquadratic_x1_bounds: (f64, f64),
    pub biquadratic_x2_bounds: (f64, f64),
    pub speed_control_mode: SpeedControlMode,
    pub low_speed_capacity_fraction: f64,
    pub space_fraction: f64,
    pub cap_ff_coeffs: [f64; 3],
    pub eir_ff_coeffs: [f64; 3],
    pub ff_bounds: (f64, f64),
    pub plf_min: f64,
    pub heating_capacities_w: Vec<f64>,
    pub cooling_capacities_w: Vec<f64>,
    pub eir_by_stage: Vec<f64>,
    pub eir_plr_coefficients: Option<Vec<[f64; 3]>>,
    pub zone_heat_fractions: Vec<(ZoneId, f64)>,
    pub min_time_per_speed_s: f64,
    pub min_on_time_s: f64,
    pub min_off_time_s: f64,
    pub supply_air_temp_c: f64,
}
```

### Step 2: Define `HvacRuntimeState` — changes every step

```rust
/// Runtime state updated every simulation timestep.
#[derive(Clone, Debug)]
pub struct HvacRuntimeState {
    pub duty_cycle: f64,
    pub plf_cooling_degradation_coeff: f64,
    pub plf_state: f64,
    pub startup: StartupConfig,
    pub last_speed_index: usize,
    pub last_speed_frac: f64,
    pub time_at_current_speed_s: f64,
    pub prev_zone_temp_c: Option<f64>,
}
```

**Design note**: `plf_cooling_degradation_coeff` is set from config at init (hvac_core.rs:396–398) but may also be overridden at runtime by equipment-type Cd defaults (hvac_core.rs:511–513) and by `CoolingCore` (air_conditioner.rs:330). It's borderline config/runtime — grouping it with runtime is safer since it can change. If future work makes it truly immutable, it can move to `HvacConfig`.

Similarly, `startup.c_d` is set from config (hvac_core.rs:399–406) but shares the same override path (hvac_core.rs:513). `startup.time_since_start_min` is definitively runtime. Keep `startup` in `HvacRuntimeState`.

### Step 3: Define `HvacControlState` — changes on signal receipt

**REQUIRED as part of this decomposition (not a follow-up)**: replace `disabled_speeds: Vec<bool>` with a fixed-size array. `set_disabled_speeds` in `staging.rs:42–56` calls `Vec::resize` on every invocation (`hvac_core.rs:563` equivalent in staging), which allocates in the hot control-signal path. The project's hot-loop policy prohibits per-step heap allocation. `MAX_SPEEDS` is confirmed absent from the codebase (searched crates/); define it as `const MAX_SPEEDS: usize = 8` (the maximum stage count used across all default performance curves in the HVAC subsystem). Replace the Vec with `[bool; MAX_SPEEDS]` and add a `speed_count: u8` field so iteration is bounded to the actual stage count without reading a length from a Vec header.

```rust
pub const MAX_SPEEDS: usize = 8;

/// Control-signal state modified by external control signals.
#[derive(Clone, Debug)]
pub struct HvacControlState {
    pub max_capacity_fraction: f64,
    /// Per-speed disable flags. Only indices `0..speed_count` are meaningful.
    pub disabled_speeds: [bool; MAX_SPEEDS],
    /// Number of active speed stages (mirrors the length of `heating_capacities_w`).
    pub speed_count: u8,
    pub max_enabled_speed: usize,
}
```

Update `set_disabled_speeds` in `staging.rs` to write into the fixed array and iterate only up to `speed_count`, eliminating all `Vec::resize` / `Vec::push` calls.

### Step 4: Restructure `HvacEquipment`

```rust
pub struct HvacEquipment {
    pub config: HvacConfig,
    pub thermostat_fsm: ThermostatFsm,  // from ticket 006
    pub runtime: HvacRuntimeState,
    pub control: HvacControlState,
}
```

### Step 5: Update all access sites

The largest mechanical effort. Every `self.<field>` becomes `self.config.<field>`, `self.runtime.<field>`, or `self.control.<field>`. Key files:

- `hvac_core.rs` — struct definition, `init_from_config`, all methods
- `staging.rs` — speed selection, duty cycle, PLF
- `air_conditioner.rs` — `CoolingCore` accesses `self.hvac.*`
- `heat_pump.rs` — `HeatPumpCore` accesses `self.hvac.*`
- `furnace.rs` — `FurnaceCore` accesses `self.hvac.*`
- `baseboard.rs` — if applicable
- `boiler.rs` — if applicable

Use `cargo clippy` to catch any missed field accesses.

### Step 6: Run full test suite

```
cargo test -p hares-equipment
```

## Definition of Done

- [ ] `HvacConfig` struct defined in `hvac_core.rs` with all configuration-only fields
- [ ] `HvacRuntimeState` struct defined with per-step mutable fields
- [ ] `HvacControlState` struct defined with signal-response fields; `disabled_speeds` is `[bool; MAX_SPEEDS]` with a `speed_count: u8` field — no `Vec<bool>`
- [ ] `MAX_SPEEDS: usize = 8` constant defined and used by `HvacControlState`
- [ ] `set_disabled_speeds` in `staging.rs` writes into the fixed array; no `Vec::resize` or heap allocation
- [ ] `HvacEquipment` struct composed of `config`, `thermostat_fsm`, `runtime`, `control` sub-structs
- [ ] No field that is semantically config is in `runtime` or `control` (except `plf_cooling_degradation_coeff` and `startup.c_d` which are documented as borderline)
- [ ] All access sites updated: `self.<field>` → `self.config.<field>`, `self.runtime.<field>`, or `self.control.<field>`
- [ ] `cargo test -p hares-equipment` passes with no behavioral changes
- [ ] `cargo clippy -p hares-equipment` produces no new warnings

## Verification

1. `cargo test -p hares-equipment` — all existing tests pass unchanged
2. `cargo clippy -p hares-equipment` — no new warnings (confirms no stale field accesses)
3. Grep for `self\.mode\b` and `self\.mode_start_at\b` — should not exist; these should be `self.thermostat_fsm.mode` etc.
4. Grep for `self\.max_capacity_fraction` — should be `self.control.max_capacity_fraction`
5. Grep for `self\.biquadratic_coeffs` — should be `self.config.biquadratic_coeffs`
6. Grep for `self\.duty_cycle` — should be `self.runtime.duty_cycle`

## References

- `hvac_core.rs:122–249` — full `HvacEquipment` struct definition with all 48 public fields (verified)
- `hvac_core.rs:138–143` — invariant comment for `mode` ↔ `mode_start_at`
- `hvac_core.rs:396–515` — Cd cascade that demonstrates the config/runtime blur for `plf_cooling_degradation_coeff`
- `air_conditioner.rs:330–332` — runtime Cd override from `CoolingCore`
- `staging.rs` — heavy user of `HvacEquipment` fields
- `thermostat.rs:1–184` — `ThermostatConfig` and related types (pre-ticket-006)

## Related Tickets

- 006-extract-thermostat-fsm — **must be completed first**; removes 11 thermostat fields from `HvacEquipment`
- 007-unify-variable-speed-selection — reduces method count on `HvacEquipment`, simplifying this decomposition
- 009-consolidate-config-helpers — overlaps with `HvacConfig` definition; coordinate on field types
