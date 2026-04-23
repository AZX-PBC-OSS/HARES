# CoreOutput HVAC Field Promotion

**Severity**: Medium
**Priority**: P1
**Status**: Open
**Areas**: hares-types, hares-core, hares-equipment/hvac

## Problem

`CoreOutput` carries `operating_mode` and `electric_kw` but no thermal output, no COP, no capacity, no speed, no setpoint. The `record_step` method accesses telemetry directly with comments like `"allowed: setpoint remains telemetry-only until CoreOutput gains setpoint fields"`. This creates an ad-hoc dependency on telemetry internals for core output recording — 3 distinct logical telemetry escapes exist in `record_step()` (setpoint at lines 2532–2535, capacity at lines 2544–2548, COP at lines 2553–2554), spanning 9 raw `// allowed:` comment lines; 2 additional logical escapes exist in the `DwellingTelemetry` construction block (heating setpoint at line 1721, cooling setpoint at line 1725), spanning 2 raw `// allowed:` comment lines; for 5 logical escapes and 11 raw `// allowed:` comment lines total across both locations. (Verified by grepping `crates/` for `allowed.*telemetry`.)

## Current Behavior

### `CoreOutput` / `CoreFlows` / `CoreState` at `equipment.rs:882-905`

```rust
pub struct CoreFlows {
    pub electric_kw: Option<ElectricPower>,
    pub reactive_power_kvar: Option<f64>,
    pub fuel_w: Option<FuelPower>,
}

pub struct CoreState {
    pub operating_mode: Option<OperatingMode>,
    pub soc: Option<Soc>,
}

pub struct CoreOutput {
    pub flows: CoreFlows,
    pub state: CoreState,
}
```

No thermal, no COP, no capacity, no speed, no setpoint fields. `CoreCapabilities` at line 1005-1015 only covers `ELECTRIC`, `REACTIVE`, `FUEL`, `HAS_SOC`, `HAS_MODE` — no thermal or performance capability bits.

### Telemetry escapes in `record_step()` at `dwelling/mod.rs:2496-2560`

Six "allowed" escape hatches where `record_step` reads telemetry directly because `CoreOutput` lacks the field:

1. **Line 2529-2532**: Setpoint — reads `tk::HEATING_SETPOINT_C` / `tk::COOLING_SETPOINT_C` from telemetry (3x `"allowed: setpoint remains telemetry-only until CoreOutput gains setpoint fields"`)
2. **Line 2541-2545**: Capacity — reads `tk::THERMAL_OUTPUT_W` / `tk::IDEAL_CAPACITY_W` / `tk::SENSIBLE_COOLING_W` from telemetry (4x `"allowed: per-equipment capacity output is telemetry-only until CoreOutput adds capacity fields"`)
3. **Line 2550-2551**: COP — reads `tk::COP` from telemetry (2x `"allowed: COP remains telemetry-only until CoreOutput gains a COP field"`)

### Telemetry escapes in environment gathering at `dwelling/mod.rs:1720-1726`

4. **Line 1720-1726**: Setpoints for `DwellingTelemetry` — reads `tk::HEATING_SETPOINT_C` / `tk::COOLING_SETPOINT_C` from telemetry (2x `"allowed: thermal setpoints are telemetry-only until CoreOutput gains setpoint fields"`)

### HVAC equipment constructs minimal `CoreOutput`

- `air_conditioner.rs:877-888`: Only populates `electric_kw` and `operating_mode` — thermal output, COP, speed, setpoint all go to telemetry only.
- `furnace.rs:200-210,429-442`: Same — only `electric_kw`, `fuel_w`, and `operating_mode`.

## Required Behavior

`CoreOutput` must carry enough information for `record_step()` to fill all output columns without reading telemetry. Specifically:

1. **`CoreState`** gains: `speed_index: Option<u8>`, `setpoint_c: Option<f64>`
2. **`CoreFlows`** gains: `thermal_output_w: Option<f64>`, `sensible_cooling_w: Option<f64>`, `latent_cooling_w: Option<f64>`
3. **New `CorePerformance`** struct gains: `cop: Option<f64>`, `main_power_kw: Option<f64>`
4. **`CoreCapabilities`** gains: `THERMAL`, `HAS_SPEED`, `HAS_SETPOINT`, `HAS_COP` bits
5. All HVAC equipment populates these new fields in their `core_output()` / `step()` impls
6. All 6 "allowed: telemetry-only" hacks in `record_step()` are replaced with `CoreOutput` field reads
7. Non-HVAC equipment defaults all new fields to `None` (backward compatible — `Option<T>` makes this safe)

## Approach

### Step 1: Extend `CoreState` in `equipment.rs`

Add three new `Option` fields:

```rust
pub struct CoreState {
    pub operating_mode: Option<OperatingMode>,
    pub soc: Option<Soc>,
    pub speed_index: Option<u8>,
    pub setpoint_c: Option<f64>,
}
```

`setpoint_c` stores the active setpoint: the heating setpoint when in heating mode, the cooling setpoint when in cooling mode, and `None` when in Deadband or off. Sentinel values such as 0.0 are forbidden — `None` is the only correct encoding for an absent setpoint. This matches how the output column works: one setpoint value per equipment, absent when the equipment is not actively conditioning.

### Step 2: Extend `CoreFlows` in `equipment.rs`

Add thermal flow fields:

```rust
pub struct CoreFlows {
    pub electric_kw: Option<ElectricPower>,
    pub reactive_power_kvar: Option<f64>,
    pub fuel_w: Option<FuelPower>,
    pub thermal_output_w: Option<f64>,
    pub sensible_cooling_w: Option<f64>,
    pub latent_cooling_w: Option<f64>,
}
```

`thermal_output_w` is the net delivered thermal (positive for heating, negative for cooling), post-DSE. `sensible_cooling_w` and `latent_cooling_w` are always negative (or zero for non-cooling equipment).

### Step 3: Add `CorePerformance` struct in `equipment.rs`

```rust
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CorePerformance {
    pub cop: Option<f64>,
    pub main_power_kw: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CoreOutput {
    pub flows: CoreFlows,
    pub state: CoreState,
    pub performance: CorePerformance,
}
```

### Step 4: Extend `CoreCapabilities` in `equipment.rs`

Add bits:

```rust
bitflags! {
    pub struct CoreCapabilities: u16 {
        const ELECTRIC   = 0b0000_0001;
        const REACTIVE   = 0b0000_0010;
        const FUEL       = 0b0000_0100;
        const HAS_SOC    = 0b0000_1000;
        const HAS_MODE   = 0b0001_0000;
        const THERMAL    = 0b0010_0000;
        const HAS_SPEED  = 0b0100_0000;
        const HAS_SETPOINT = 0b1000_0000;
        const HAS_COP    = 0b0001_0000_0000;
    }
}
```

Note: `u8` → `u16` to accommodate new bits.

**Blocker — serialization break investigation (must complete before coding Step 4)**:
`CoreCapabilities` is `#[derive(Serialize, Deserialize)]` and is embedded in `EquipmentDescriptor`, which has a confirmed JSON round-trip test (`equipment.rs:1078`). Before widening from `u8` to `u16`, search `/home/rich/src/HARES/crates/` for any persisted or checkpointed usage of `CoreCapabilities` (e.g., saved `EquipmentDescriptor` JSON blobs in test fixtures, output files, or snapshot files). If any such artifact exists with a `u8`-encoded value, widening to `u16` changes the serde wire format (bitflags serializes as an integer) and requires either: (a) a migration of existing test fixture JSON, or (b) keeping `u8` and finding the new bits in a separate `ExtendedCapabilities: u8` flag. If no persisted artifacts exist outside of in-process test round-trips, the `u8` → `u16` change is safe. Document the finding before proceeding.

**Preliminary investigation result**: A search of the codebase finds `CoreCapabilities` used in `equipment.rs` (round-trip tests at lines 2169–2172, 2396), `dwelling/mod.rs` (in-process `EquipmentDescriptor` construction at lines 3472, 3572, 3680, 4303), and test fixtures in `dispatch_ordering_regressions.rs` (line 130). No external snapshot files or persisted JSON blobs containing `CoreCapabilities` as a serialized integer were found. All serialization is in-process round-trips. The `u8` → `u16` widening is therefore likely safe, but the implementor must grep for any fixture files (e.g., `*.json` in `tests/` or `fixtures/`) before proceeding, as the automated search above covered only Rust source files.

### Step 5: Update HVAC equipment `CoreOutput` construction

For each HVAC equipment, populate the new fields:

- **CoolingCore** (`air_conditioner.rs:877-888`): Set `thermal_output_w`, `sensible_cooling_w`, `latent_cooling_w`, `speed_index`, `setpoint_c`, `cop`, `main_power_kw`
- **ElectricFurnace** (`furnace.rs:200-210`): Set `thermal_output_w`, `setpoint_c`
- **GasFurnace** (`furnace.rs:429-442`): Set `thermal_output_w`, `speed_index`, `setpoint_c`, `main_power_kw`
- **IdealHvac** (`ideal_hvac.rs`): Set `thermal_output_w`, `setpoint_c`, `cop`
- **Heat pump equipment**: Set all relevant fields

### Step 6: Remove telemetry escapes in `record_step()`

Replace the 6 "allowed" telemetry reads in `dwelling/mod.rs:2496-2560` with `CoreOutput` field access:

- Line 2529-2533: `co.state.setpoint_c` instead of `telemetry.get(tk::HEATING_SETPOINT_C)`
- Line 2541-2546: `co.flows.thermal_output_w` instead of `telemetry.get(tk::THERMAL_OUTPUT_W)`
- Line 2550-2551: `co.performance.cop` instead of `eq.telemetry().get(tk::COP)`

Similarly replace lines 1720-1726 in `DwellingTelemetry` construction.

### Step 7: Update `validate_core_contract()` in `equipment.rs:907-1003`

Add validation rules for the new capability bits and fields. Non-HVAC equipment that doesn't declare `THERMAL` must not populate `thermal_output_w`, etc.

## Definition of Done

- [ ] `CoreState` has `speed_index` and `setpoint_c` fields
- [ ] `CoreFlows` has `thermal_output_w`, `sensible_cooling_w`, `latent_cooling_w` fields
- [ ] `CoreOutput` has `performance: CorePerformance` with `cop` and `main_power_kw`
- [ ] `CoreCapabilities` has `THERMAL`, `HAS_SPEED`, `HAS_SETPOINT`, `HAS_COP` bits
- [ ] All HVAC equipment populates the new `CoreOutput` fields
- [ ] All 9 "allowed: telemetry-only" comment lines in `record_step()` (3 logical escapes: setpoint, capacity, COP) are removed; replaced with `CoreOutput` field reads
- [ ] All 2 "allowed: telemetry-only" comment lines in `DwellingTelemetry` construction (dwelling/mod.rs:1721, 1725) are removed
- [ ] `validate_core_contract()` checks the new capability/field pairs
- [ ] Non-HVAC equipment (battery, PV, EV, water heater, dehumidifier) unchanged — new fields default to `None`
- [ ] Existing tests pass; serialization round-trip tests added for extended `CoreOutput`

## Verification

1. Run full test suite — no regressions.
2. Search codebase for `"allowed.*telemetry-only"` — must return zero hits (across all 11 former comment lines) after the change.
3. Verify `CoreOutput` serialization/deserialization round-trips with the new fields.
4. Run a simulation at verbosity 8 and confirm output values are identical to before (values now come from `CoreOutput` instead of telemetry, but must be the same numbers).

## References

- `equipment.rs:882-905`: Current `CoreFlows`, `CoreState`, `CoreOutput` definitions
- `equipment.rs:1005-1015`: `CoreCapabilities` bitflags
- `equipment.rs:907-1003`: `validate_core_contract()` validation logic
- `dwelling/mod.rs:2496-2560`: `record_step()` with 6 telemetry escape hatches
- `dwelling/mod.rs:1720-1726`: `DwellingTelemetry` construction with 2 telemetry escape hatches
- `air_conditioner.rs:877-888`: CoolingCore `CoreOutput` construction
- `furnace.rs:200-210,429-442`: Furnace `CoreOutput` construction

## Related Tickets

- #017 — Missing output columns v7: **018 must be completed and merged before 017 begins**. Ticket 017's output columns route from `CoreOutput` fields; if 017 is implemented before 018, its column wiring targets telemetry and then must be re-wired when 018 lands. Implementing in order (018 → 017) avoids double-wiring.
- #019 — Telemetry key gaps (some keys like COP already exist; this ticket makes them redundant for output)
- #020 — Setpoint chain visibility (setpoint_c in CoreOutput becomes the effective value; schedule/runtime setpoints stay in telemetry)
