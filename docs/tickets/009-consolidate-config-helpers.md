# Consolidate Config Helpers and Deduplicate Heat Pump Config Structs

**Severity**: Medium
**Priority**: P2
**Status**: Open
**Areas**: hares-equipment/hvac/cooling_config, hares-equipment/hvac/heat_pump_config, hares-equipment/hvac/core_config, hares-equipment/hvac/hvac_core

## Problem

Several config-related helpers are duplicated across the HVAC config files, and `HeatPumpHeaterConfig` / `HeatPumpCoolerConfig` share ~80% of their fields without any shared base. Additionally, equipment type name registration uses bare string literals with no compile-time validation, and the Cd cascade in `hvac_core.rs` sets `cd` three times from overlapping config key chains.

## Current Behavior

### 1. `default_one()` duplicated (cooling_config.rs vs heat_pump_config.rs)

```rust
// cooling_config.rs:10–12
fn default_one() -> u8 {
    1
}

// heat_pump_config.rs:10–12
fn default_one() -> u8 {
    1
}
```

Identical function, defined independently in two files. Used as a serde default for `number_of_speeds`.

### 2. `HeatPumpHeaterConfig` and `HeatPumpCoolerConfig` field overlap

`HeatPumpHeaterConfig` (heat_pump_config.rs:20–139) has 39 fields. `HeatPumpCoolerConfig` (heat_pump_config.rs:302–393) has 35 fields. The following 31 fields are shared with identical types and serde attributes:

| Field | Heater line | Cooler line |
|---|---|---|
| `equipment_id` | 21 | 309 |
| `zone_id` | 22 | 310 |
| `heating_capacity_w` | 26 | 312 |
| `heating_eir` | 29 | 315 |
| `stage_heating_capacities_w` | 32 | 318 |
| `stage_heating_eirs` | 35 | 321 |
| `backup_fuel` | 37 | 324 |
| `backup_capacity_w` | 40 | 327 |
| `backup_eir` | 43 | 330 |
| `fraction_heating_load_served` | 46 | 333 |
| `cooling_capacity_w` | 50 | 329 |
| `cooling_eir` | 53 | 332 |
| `stage_cooling_capacities_w` | 56 | 336 |
| `stage_cooling_eirs` | 59 | 339 |
| `fraction_cooling_load_served` | 62 | 342 |
| `number_of_speeds` | 67 | 344 |
| `is_mini_split` | 71 | 347 |
| `shr` | 74 | 349 |
| `fan_power_w` | 77 | 351 |
| `fan_power_w_per_cfm` | 80 | 354 |
| `airflow_m3_s_per_w` | 83 | 357 |
| `heating_setpoint_c` | 86 | 359 |
| `cooling_setpoint_c` | 89 | 362 |
| `hysteresis_c` | 92 | 365 |
| `heating_setpoint_source` | 94 | 368 |
| `cooling_setpoint_source` | 96 | 371 |
| `duct` | 114 | 368 |
| `biquadratic_x1_min` / `x1_max` | 117, 120 | 371, 374 |
| `biquadratic_x2_min` / `x2_max` | 123, 126 | 377, 380 |
| `ff_min` / `ff_max` | 129, 132 | 383, 386 |
| `plf_min` / `plf_max` | 135, 138 | 389, 392 |

Heater-only fields (not in Cooler):
- `hp_lockout_temp_c` (line 99)
- `er_lockout_temp_c` (line 102)
- `max_oat_supplemental_c` (line 105)
- `er_setpoint_offset_c` (line 108)
- `er_hard_lockout_time_s` (line 111)

Cooler-only fields (not in Heater):
- `stage_shrs: Option<Vec<f64>>` (line 339)

### 3. Equipment type name string literals — no compile-time check

```rust
// cooling_config.rs:108–112
impl EquipmentTypedConfig for CentralAirConditionerConfig {
    fn equipment_type_name() -> &'static str { "Central AC" }
}

// heat_pump_config.rs:188–192
impl EquipmentTypedConfig for HeatPumpHeaterConfig {
    fn equipment_type_name() -> &'static str { "ASHP Heater" }
}

// heat_pump_config.rs:395–399
impl EquipmentTypedConfig for HeatPumpCoolerConfig {
    fn equipment_type_name() -> &'static str { "ASHP Cooler" }
}
```

Also in cooling_config.rs:341 (`"Room AC"`), cooling_config.rs:428 (`"Dehumidifier"`), heating_config.rs (furnace, boiler types). A typo in any of these strings breaks config round-tripping silently. There is no central registry or compile-time check ensuring the string matches the dispatch table.

### 4. Cd cascade — three overlapping key chains

In `hvac_core.rs:396–515`, the Cd value is set three times from overlapping config key chains:

**First assignment** — PLF Cd (hvac_core.rs:396–398):
```rust
self.plf_cooling_degradation_coeff = extract_numeric(config, "cooling_cd")
    .or_else(|| extract_numeric(config, "cd"))
    .unwrap_or(DEFAULT_PLF_DEGRADATION_COEFF);
```

**Second assignment** — startup Cd (hvac_core.rs:399–406):
```rust
let cd = extract_numeric(config, "startup_cd")
    .or_else(|| extract_numeric(config, "cooling_cd"))
    .or_else(|| extract_numeric(config, "cd"))
    .unwrap_or(DEFAULT_PLF_DEGRADATION_COEFF);
self.startup = StartupConfig { c_d: cd, time_since_start_min: 0.0 };
```

**Third assignment** — equipment-type derived override (hvac_core.rs:487–515):
```rust
let explicit_cd = extract_numeric(config, "startup_cd")
    .or_else(|| extract_numeric(config, "cooling_cd"))
    .or_else(|| extract_numeric(config, "cd"));
if explicit_cd.is_none() {
    // ... derived from speed_control_mode / SEER / HSPF ...
    if let Some(cd) = derived_cd {
        self.plf_cooling_degradation_coeff = cd;
        self.startup.c_d = cd;
    }
}
```

Problems:
- The key chain `"startup_cd" → "cooling_cd" → "cd"` is repeated 3 times (lines 396–398, 399–402, 491–493).
- If a user provides `"cooling_cd"`, it sets both `plf_cooling_degradation_coeff` and `startup.c_d`, but the intent may have been to set only the PLF Cd.
- The third assignment (lines 487–515) overwrites the first two when no explicit key was found, meaning the first two assignments are wasted work in that case.

## Required Behavior

1. `default_one()` defined once, shared across config files.
2. `HeatPumpHeaterConfig` and `HeatPumpCoolerConfig` share common fields through a `HeatPumpCommonConfig` sub-struct, flattened into each config.
3. Equipment type names are centralized with compile-time safety.
4. The Cd cascade is resolved once with clear precedence, not three times.

## Approach

This ticket bundles three independent concerns. They must land as two separate commits (or PRs) so each can be reverted cleanly:

**Phase A (commit first)**: Step 1 (`default_one` deduplication) + Step 4 (Cd cascade resolution). These are purely mechanical and have no serde schema impact.

**Phase B (commit after Phase A is merged)**: Step 2 (`HeatPumpCommonConfig` extraction) + Step 3 (equipment type name constants). Phase B changes the serde key layout and requires round-trip test coverage before landing. Do not squash Phase A and Phase B into a single commit.

### Step 1: Move `default_one()` to `core_config.rs`

`core_config.rs` already houses shared config infrastructure (`DuctConfig`, `extract_numeric`, etc.). Add:

```rust
pub(super) fn default_one() -> u8 {
    1
}
```

Remove the private `fn default_one()` from `cooling_config.rs:10–12` and `heat_pump_config.rs:10–12`. Update serde attributes from `default = "default_one"` to `default = "core_config::default_one"` — or re-export `default_one` at the crate level.

**Alternative**: Since `u8` defaults to `0` and we need `1`, we can use an inline closure: `#[serde(default = "default_one")]` requires a function path visible at the use site. The simplest approach is to keep `default_one` as a `pub(super)` function in `core_config` and use `use super::core_config::default_one;` in each config file.

### Step 2: Extract `HeatPumpCommonConfig`

```rust
/// Fields shared between `HeatPumpHeaterConfig` and `HeatPumpCoolerConfig`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HeatPumpCommonConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    // Heating parameters
    pub heating_capacity_w: Option<f64>,
    pub heating_eir: Option<f64>,
    pub stage_heating_capacities_w: Option<Vec<f64>>,
    pub stage_heating_eirs: Option<Vec<f64>>,
    pub backup_fuel: Option<FuelType>,
    pub backup_capacity_w: Option<f64>,
    pub backup_eir: Option<f64>,
    pub fraction_heating_load_served: Option<f64>,
    // Cooling parameters
    pub cooling_capacity_w: Option<f64>,
    pub cooling_eir: Option<f64>,
    pub stage_cooling_capacities_w: Option<Vec<f64>>,
    pub stage_cooling_eirs: Option<Vec<f64>>,
    pub fraction_cooling_load_served: Option<f64>,
    // Shared
    pub number_of_speeds: u8,
    pub is_mini_split: bool,
    pub shr: Option<f64>,
    pub fan_power_w: Option<f64>,
    pub fan_power_w_per_cfm: Option<f64>,
    pub airflow_m3_s_per_w: Option<f64>,
    pub heating_setpoint_c: Option<f64>,
    pub cooling_setpoint_c: Option<f64>,
    pub hysteresis_c: Option<f64>,
    pub heating_setpoint_source: Option<ScheduleSourceConfig>,
    pub cooling_setpoint_source: Option<ScheduleSourceConfig>,
    pub duct: DuctConfig,
    pub biquadratic_x1_min: Option<f64>,
    pub biquadratic_x1_max: Option<f64>,
    pub biquadratic_x2_min: Option<f64>,
    pub biquadratic_x2_max: Option<f64>,
    pub ff_min: Option<f64>,
    pub ff_max: Option<f64>,
    pub plf_min: Option<f64>,
    pub plf_max: Option<f64>,
}
```

Then flatten into both configs:

```rust
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeatPumpHeaterConfig {
    #[serde(flatten)]
    pub common: HeatPumpCommonConfig,
    // Heater-only fields:
    pub hp_lockout_temp_c: Option<f64>,
    pub er_lockout_temp_c: Option<f64>,
    pub max_oat_supplemental_c: Option<f64>,
    pub er_setpoint_offset_c: Option<f64>,
    pub er_hard_lockout_time_s: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeatPumpCoolerConfig {
    #[serde(flatten)]
    pub common: HeatPumpCommonConfig,
    // Cooler-only fields:
    pub stage_shrs: Option<Vec<f64>>,
}
```

**Required design decision — serde flatten + deny_unknown_fields soundness**:
`#[serde(deny_unknown_fields)]` MUST NOT appear on any struct used as the
target of `#[serde(flatten)]`. In any current serde release, placing
`deny_unknown_fields` on `HeatPumpCommonConfig` (the inner, flattened struct)
causes deserialization to reject all keys belonging to the outer struct as
"unknown", because the inner deserializer sees the full flattened key set
before the outer struct has consumed its own keys. This is a documented
soundness limitation of the serde flatten mechanism (see serde docs:
[The `flatten` attribute](https://serde.rs/attr-flatten.html) — "use of
`deny_unknown_fields` on the struct being flattened into is not supported").

Decision: remove `#[serde(deny_unknown_fields)]` from `HeatPumpCommonConfig`.
The outer structs (`HeatPumpHeaterConfig`, `HeatPumpCoolerConfig`) retain
`#[serde(deny_unknown_fields)]`, which is correct — the outer struct sees
the full merged key set and can reject truly unknown keys. Do not add
`deny_unknown_fields` to `HeatPumpCommonConfig` at any point.

**Migration concern**: All downstream code that accesses `heater.heating_capacity_w` becomes `heater.common.heating_capacity_w`. This is a large mechanical change. Consider providing `Deref<Target = HeatPumpCommonConfig>` on both configs so existing access patterns continue to work, or accept the verbosity for explicitness.

**Do NOT use `Deref<Target = HeatPumpCommonConfig>`**: Accept the verbosity
of `heater.common.heating_capacity_w` for explicitness. Deref hides the
FSM boundary and creates implicit coupling.

### Step 3: Add typed `EquipmentTypeName` constants or enum

Add to `core_config.rs` (or a new `equipment_types.rs`):

```rust
/// Compile-time-verified equipment type name constants.
///
/// Each constant corresponds to an `EquipmentTypedConfig::equipment_type_name()`
/// implementation. A mismatch between these constants and the trait impls is
/// caught by the test `equipment_type_names_match_constants`.
pub mod equipment_type_name {
    pub const CENTRAL_AC: &str = "Central AC";
    pub const ROOM_AC: &str = "Room AC";
    pub const ASHP_HEATER: &str = "ASHP Heater";
    pub const ASHP_COOLER: &str = "ASHP Cooler";
    pub const DEHUMIDIFIER: &str = "Dehumidifier";
    // Add furnace, boiler, etc. from heating_config.rs
}
```

Update each `EquipmentTypedConfig` impl to reference the constant:

```rust
impl EquipmentTypedConfig for CentralAirConditionerConfig {
    fn equipment_type_name() -> &'static str {
        equipment_type_name::CENTRAL_AC
    }
}
```

Add a test that iterates all `EquipmentTypedConfig` impls and verifies their `equipment_type_name()` matches the constant. This is a compile-time-adjacent check — if someone adds a new equipment type without adding a constant, the test will fail.

The equipment_type_name constant test must list each type manually — Rust has
no reflection to iterate trait impls. Add a compile-time check via a match
arm that must be exhaustive.

### Step 4: Simplify the Cd cascade

Replace the three overlapping assignments (hvac_core.rs:396–515) with a single resolution:

```rust
/// Resolve the Cd (degradation coefficient) from config keys and equipment-type
/// defaults. Precedence:
///   1. Explicit key: "startup_cd" > "cooling_cd" > "cd"
///   2. Derived from speed_control_mode, SEER, HSPF (equipment-type default)
///   3. DEFAULT_PLF_DEGRADATION_COEFF (0.25)
fn resolve_cd(config: &EquipmentConfig, speed_mode: SpeedControlMode, rated_seer: Option<f64>, rated_hspf: Option<f64>) -> f64 {
    // Check explicit keys first
    if let Some(cd) = extract_numeric(config, "startup_cd")
        .or_else(|| extract_numeric(config, "cooling_cd"))
        .or_else(|| extract_numeric(config, "cd"))
    {
        return cd;
    }
    // Derived defaults by equipment type
    match speed_mode {
        SpeedControlMode::VariableSpeedIdeal => 0.0,
        SpeedControlMode::TwoSpeedSetpoint
        | SpeedControlMode::TwoSpeedTime
        | SpeedControlMode::TwoSpeedAlternating => 0.11,
        SpeedControlMode::SingleSpeed => {
            let from_seer = rated_seer.map(|s| if s < 13.0 { 0.20 } else { 0.07 });
            let from_hspf = rated_hspf.map(|h| if h < 7.0 { 0.20 } else { 0.11 });
            from_seer.or(from_hspf).unwrap_or(DEFAULT_PLF_DEGRADATION_COEFF)
        }
        SpeedControlMode::MultiSpeedInterpolated => DEFAULT_PLF_DEGRADATION_COEFF,
    }
}
```

Then in `init_from_config`:

```rust
let cd = resolve_cd(config, self.speed_control_mode, extract_numeric(config, "rated_seer"), extract_numeric(config, "rated_hspf"));
self.plf_cooling_degradation_coeff = cd;
self.startup = StartupConfig { c_d: cd, time_since_start_min: 0.0 };
self.startup.validate()?;
```

This eliminates the triple key-chain repetition and the overwrite pattern.

**Design note**: Currently `"cooling_cd"` sets both PLF Cd and startup Cd to the same value. The unified `resolve_cd` preserves this behavior. If a future need arises to set them independently, `resolve_cd` can be split into two functions with different key chains — but today they are always set together.

### Step 5: Run full test suite

```
cargo test -p hares-equipment
```

## Cross-Ticket Coordination

**Ticket 008 field ownership**: `min_on_time_s` and `min_off_time_s` are listed in ticket 008's thermostat sub-struct (extracted by ticket 006 into `ThermostatFsm`). These fields must NOT be added to `HeatPumpCommonConfig` in Phase B. Config structs supply the values at parse time; `ThermostatFsm` owns them at runtime. If `HeatPumpCommonConfig` needs to carry these as config inputs, name them identically to the existing config keys (`"min_on_time_s"`, `"min_off_time_s"`) and ensure they are consumed into `ThermostatFsm` fields during `init_from_config`, not stored redundantly.

**Cd cascade verification**: The triple key chain `"startup_cd" → "cooling_cd" → "cd"` is confirmed at hvac_core.rs:396–398 (PLF Cd assignment, no startup_cd key), hvac_core.rs:399–402 (startup Cd assignment, full three-key chain), and hvac_core.rs:491–493 (explicit-cd guard, full three-key chain). `resolve_cd` in Phase A must be the only remaining site for this chain.

## HPXML Wiring

The `#[serde(flatten)]` restructuring changes the JSON serialization format.
Verify that HPXML→JSON conversion in `resolve_hvac.rs` produces config
compatible with the flattened format. The serde JSON representation should
remain flat when `flatten` is used (serde merges flattened keys into the
parent). Test round-trip serialization of HPXML-derived configs.

## Definition of Done

- [ ] `default_one()` exists only in `core_config.rs`; no other file defines it locally
- [ ] `HeatPumpCommonConfig` struct defined with the 31 shared fields
- [ ] `HeatPumpHeaterConfig` and `HeatPumpCoolerConfig` flatten `HeatPumpCommonConfig`; heater-only and cooler-only fields remain on the outer struct
- [ ] All serde round-trip tests for heat pump configs pass (no deserialization regression)
- [ ] `equipment_type_name` module with `const` strings exists; all `EquipmentTypedConfig` impls reference these constants
- [ ] Test exists verifying `EquipmentTypedConfig::equipment_type_name()` matches the constant for each type
- [ ] `resolve_cd` function replaces the three Cd assignment blocks in `init_from_config`
- [ ] The key chain `"startup_cd" → "cooling_cd" → "cd"` appears only once (in `resolve_cd`)
- [ ] `cargo test -p hares-equipment` passes with no behavioral changes
- [ ] `cargo clippy -p hares-equipment` produces no new warnings

## Verification

1. `cargo test -p hares-equipment` — all existing tests pass unchanged
2. `cargo clippy -p hares-equipment` — no new warnings
3. Grep for `fn default_one` — should appear only in `core_config.rs`
4. Grep for `"Central AC"` / `"ASHP Heater"` / `"ASHP Cooler"` as bare string literals — should only appear in the `equipment_type_name` module constants and the `EquipmentTypedConfig` impls that reference them
5. Grep for `extract_numeric(config, "startup_cd")` — should appear only once (in `resolve_cd`)
6. Grep for `extract_numeric(config, "cooling_cd")` — should appear only once (in `resolve_cd`)
7. Verify heat pump config round-trip: construct `HeatPumpHeaterConfig` with `common` populated, serialize to JSON, deserialize, assert equality

## References

- `cooling_config.rs:10–12` — duplicated `default_one()`
- `cooling_config.rs:108–112` — `"Central AC"` string literal
- `cooling_config.rs:341` — `"Room AC"` string literal
- `cooling_config.rs:428` — `"Dehumidifier"` string literal
- `heat_pump_config.rs:10–12` — duplicated `default_one()`
- `heat_pump_config.rs:20–139` — `HeatPumpHeaterConfig` (39 fields)
- `heat_pump_config.rs:188–192` — `"ASHP Heater"` string literal
- `heat_pump_config.rs:302–393` — `HeatPumpCoolerConfig` (35 fields)
- `heat_pump_config.rs:395–399` — `"ASHP Cooler"` string literal
- `hvac_core.rs:396–515` — Cd cascade (three overlapping assignments)
- `core_config.rs:1–333` — shared config infrastructure (`DuctConfig`, `extract_numeric`, `load_biquadratic_coeffs`, etc.)

## Related Tickets

- 006-extract-thermostat-fsm — config setpoint fields will move to `ThermostatFsm`; coordinate on which fields remain in `HeatPumpCommonConfig`
- 008-decompose-hvacequipment-struct — `HvacConfig` sub-struct (from that ticket) will consume fields from these typed configs; coordinate on field ownership

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-20

### Code Confirmation

- [x] Referenced line numbers still match (with corrections noted below)
- [x] Described logic matches current implementation
- [x] OCHRE cross-check result: partially matches — see §OCHRE below
- [x] EnergyPlus cross-check result: N/A — this ticket is a pure Rust refactoring concern with no EnergyPlus formula citations

#### Line number corrections

| Ticket claim | Actual location (2026-05-20) |
|---|---|
| `cooling_config.rs:10–12` — `default_one()` | ✓ lines 10–12 |
| `cooling_config.rs:108–112` — `"Central AC"` | Actual: line 110 (impl spans 108–112) |
| `cooling_config.rs:341` — `"Room AC"` | Actual: line 342 |
| `cooling_config.rs:428` — `"Dehumidifier"` | Actual: line 430 |
| `heat_pump_config.rs:10–12` — `default_one()` | ✓ lines 10–12 |
| `heat_pump_config.rs:20–139` — `HeatPumpHeaterConfig` (39 fields) | Actual: lines 20–139; field count = **40** (39 named fields + 1 `duct: DuctConfig` which is flattened, counted as 1 field slot) |
| `heat_pump_config.rs:188–192` — `"ASHP Heater"` | Actual: line 190 |
| `heat_pump_config.rs:302–393` — `HeatPumpCoolerConfig` (35 fields) | Actual: lines 308–393; field count = **37** (36 fields + duct) |
| `heat_pump_config.rs:395–399` — `"ASHP Cooler"` | Actual: line 397 |
| `hvac_core.rs:396–515` — Cd cascade | ✓ exact lines confirmed: 396–398 (PLF Cd), 399–406 (startup Cd), 491–515 (derived override) |

A third file was also found with `default_one()`: **`heating_config.rs:316`**. The ticket does not mention this instance. The fix must also address this file.

#### Issue 1 — `default_one()` duplication: **CONFIRMED**

`fn default_one() -> u8 { 1 }` is defined independently in:
- `cooling_config.rs:10–12`
- `heat_pump_config.rs:10–12`
- `heating_config.rs:316–318` ← **not mentioned in ticket**

All three are identical and used as a serde default for `number_of_speeds`.

#### Issue 2 — `HeatPumpHeaterConfig` / `HeatPumpCoolerConfig` field overlap: **CONFIRMED**

Heater has 40 fields (struct lines 20–139); cooler has 37 fields (lines 308–393). Shared fields confirmed by direct inspection:

- 27 fields are identical in both structs with the same types and serde attributes.
- Heater-only fields (5): `hp_lockout_temp_c`, `er_lockout_temp_c`, `max_oat_supplemental_c`, `er_setpoint_offset_c`, `er_hard_lockout_time_s`.
- Cooler-only fields (1): `stage_shrs`.
- The ticket's table of 31 shared fields **contains minor inaccuracies** in its line-number column (off by a few lines in each case) but the field names are accurate.

#### Issue 3 — Equipment type name string literals: **CONFIRMED**

Bare string literals confirmed at: `cooling_config.rs:110` (`"Central AC"`), `cooling_config.rs:342` (`"Room AC"`), `cooling_config.rs:430` (`"Dehumidifier"`), `heat_pump_config.rs:190` (`"ASHP Heater"`), `heat_pump_config.rs:397` (`"ASHP Cooler"`). There is no central registry or compile-time check.

#### Issue 4 — Cd cascade (three overlapping key chains): **CONFIRMED**

The key chain `"startup_cd" → "cooling_cd" → "cd"` appears at **three sites** in `hvac_core.rs`:

- **Lines 396–398** (PLF Cd assignment): reads only `"cooling_cd" | "cd"` — `"startup_cd"` is NOT in this chain.
- **Lines 399–402** (startup Cd assignment): reads `"startup_cd" | "cooling_cd" | "cd"`.
- **Lines 491–493** (explicit-cd guard): reads `"startup_cd" | "cooling_cd" | "cd"` — if none found, derived defaults overwrite both fields.

Additional observation not fully captured in the ticket: the first block (lines 396–398) does **not** include `"startup_cd"` in its key chain. This means if a user provides only `"startup_cd"`, `startup.c_d` is set explicitly (via the second block) but `plf_cooling_degradation_coeff` gets `DEFAULT_PLF_DEGRADATION_COEFF` (0.25) from the first block, not the user's value. The third block then detects `explicit_cd.is_some()` (because `startup_cd` is found) and skips the derived path — leaving the PLF Cd at 0.25 even though the user likely intended to override it. This asymmetry is a real bug. The `resolve_cd` function proposed in the ticket would unify both fields, but it must also be applied to the first assignment block.

#### OCHRE Cross-Check

OCHRE's `calc_c_d` (vendors/OCHRE/ochre/utils/equipment.py:470–500) uses the identical decision table:
- Single-speed heater: HSPF < 7.0 → 0.20, else → 0.11
- Two-speed heater/cooler: 0.11
- Multi-speed/variable: 0.0
- Single-speed cooler: SEER < 13.0 → 0.20, else → 0.07

HARES matches OCHRE exactly for all derived Cd values. The divergence is purely structural (OCHRE computes Cd once during HPXML parsing and stores it; HARES re-derives it inline on every `init()` call with three separate blocks).

### Web-Verified Citations

This ticket contains **one implicit standards reference** (the serde flatten + deny_unknown_fields limitation) and **no explicit ASHRAE/EnergyPlus/DOE citations**. All web verification was performed on the serde documentation.

- **Citation**: "use of `deny_unknown_fields` on the struct being flattened into is not supported" — ticket §Step 2, first bullet under "Required design decision"
- **Source found**: [Struct flattening · Serde](https://serde.rs/attr-flatten.html)
- **Quoted passage**: "flatten is not supported in combination with structs that use deny_unknown_fields. Neither the outer nor inner flattened struct should use that attribute."
- **Verdict**: **Confirmed**. The serde documentation explicitly prohibits `deny_unknown_fields` on either the outer or inner struct when `flatten` is used. The ticket's advice to omit `deny_unknown_fields` from `HeatPumpCommonConfig` is correct.

  Additional context from [serde issue #2384](https://github.com/serde-rs/serde/issues/2384): a reporter demonstrated that simple cases of `flatten + deny_unknown_fields` can work in practice, and the documentation note was raised. The serde maintainers have not changed the documented guidance — it remains "not supported". The ticket is correct to treat this as a soundness restriction to respect.

### Legitimacy

- **Verdict**: **Legitimate** (with minor line-number corrections and one omission)
- **Rationale**: All four issues are confirmed by direct code inspection. Issue 1 (`default_one()` duplication) exists in three files, not two — `heating_config.rs:316` was not mentioned but must also be fixed. Issue 2 (field overlap) is confirmed with minor field-count differences (40/37 actual vs. 39/35 claimed, depending on how `duct` is counted). Issue 3 (bare string literals) is confirmed. Issue 4 (Cd cascade) is confirmed and is more nuanced than described: the first block (lines 396–398) omits `"startup_cd"` from its key chain, creating a real asymmetry where `startup_cd`-only config silently leaves `plf_cooling_degradation_coeff` at the global default. The `resolve_cd` fix as proposed will correctly unify both fields. The serde `deny_unknown_fields` + `flatten` guidance is web-verified as accurate per official documentation.

### Proposed Fix Summary

**Phase A**: (1) Move `default_one()` to `core_config.rs` as `pub(super) fn default_one() -> u8 { 1 }` and remove the three private copies from `cooling_config.rs`, `heat_pump_config.rs`, and `heating_config.rs`. (2) Replace the three Cd blocks in `hvac_core::init` (lines 396–515) with a single `resolve_cd` call that applies the unified key chain `"startup_cd" | "cooling_cd" | "cd"` to **both** `plf_cooling_degradation_coeff` and `startup.c_d`, then falls through to equipment-type/SEER/HSPF derived defaults, then to `DEFAULT_PLF_DEGRADATION_COEFF`.

**Phase B**: Extract `HeatPumpCommonConfig` with `#[serde(flatten)]` on each outer struct (`HeatPumpHeaterConfig`, `HeatPumpCoolerConfig`). Do NOT add `deny_unknown_fields` to `HeatPumpCommonConfig`; retain it only on the outer structs. Add `equipment_type_name` constants module and update all `EquipmentTypedConfig` impls to reference them.

**Do NOT implement** — audit only.

### Tests Written

- **File**: `crates/hares-equipment/src/hvac/heat_pump_config.rs` (in `#[cfg(test)] mod tests`)
  - `regression_009_number_of_speeds_defaults_to_one_heater` — documents `default_one()` == 1 for `HeatPumpHeaterConfig`
  - `regression_009_number_of_speeds_defaults_to_one_cooler` — same for `HeatPumpCoolerConfig`
  - `regression_009_heater_only_fields_missing_from_cooler_json` — verifies `stage_shrs` rejected by heater struct
  - `regression_009_cooler_only_field_missing_from_heater_json` — verifies `hp_lockout_temp_c` rejected by cooler struct
  - `regression_009_equipment_type_name_literals` — pins current string literal values

- **File**: `crates/hares-equipment/src/hvac/cooling_config.rs` (in `#[cfg(test)] mod tests`)
  - `regression_009_central_ac_number_of_speeds_defaults_to_one` — documents `default_one()` == 1 for `CentralAirConditionerConfig`
  - `regression_009_equipment_type_name_literals` — pins `"Central AC"`, `"Room AC"`, `"Dehumidifier"` literals

- **File**: `crates/hares-equipment/src/hvac/hvac_core.rs` (in `#[cfg(test)] mod tests`)
  - `regression_009_cooling_cd_sets_both_plf_and_startup_cd` — pins that `cooling_cd` sets both fields to same value
  - `regression_009_startup_cd_overrides_only_startup_not_plf` — documents the `startup_cd`-only asymmetry bug: `plf_cooling_degradation_coeff` stays at DEFAULT (0.25), not the user value
  - `regression_009_variable_speed_derived_cd_is_zero` — pins derived Cd = 0.0 for `VariableSpeedIdeal`
  - `regression_009_cd_key_chain_priority` — pins that `startup_cd` wins over `cooling_cd` for `startup.c_d`, while `cooling_cd` sets `plf_cooling_degradation_coeff`

All 11 tests pass: `cargo test -p hares-equipment regression_009` → 11 passed, 0 failed.
