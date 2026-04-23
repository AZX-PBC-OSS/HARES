# Missing OCHRE-equivalent Output Columns at Verbosity 7

**Severity**: Medium
**Priority**: P1
**Status**: Open
**Areas**: hares-io/output, hares-equipment/hvac, hares-types

## Problem

HARES matches OCHRE at output verbosity 4 but falls behind at v7. The following OCHRE-equivalent columns are missing from the output schema even though the data already exists in telemetry:

| OCHRE Column | HARES Telemetry Source | Status |
|---|---|---|
| `{name} SHR (-)` | `tk::SHR` | Missing from output schema |
| `{name} Speed (-)` | `tk::SPEED_INDEX` | Missing from output schema |
| `{name} Fan Power (kW)` | `tk::FAN_KW` | Missing from output schema |
| `{name} Main Power (kW)` | `tk::COMPRESSOR_KW` (cooling) / computed (heating) | Missing from output schema |
| `{name} Duct Losses (W)` | `gross_capacity_w * (1 - dse)` | Missing; requires new telemetry key or computation in port distribution |
| `{name} Runtime Fraction (-)` | `tk::RUNTIME_FRACTION` | Missing from output schema; furnace does not write this key (see DoD) |
| `{name} Capacity (W)` | — | Exists at v8 (`columns.rs:217-220`); **promote to v7** |
| `{name} COP (-)` | — | Exists at v8 (`columns.rs:222-226`); **promote to v7** |

Additionally, OCHRE outputs `{name} Latent Gains (W)` for cooling equipment at v7, which maps to `tk::LATENT_COOLING_W`.

## Current Behavior

### OCHRE `generate_results()` at `HVAC.py:568-604`

OCHRE outputs these columns at the indicated verbosity levels:

- **v4** (`HVAC.py:572-584`): `{name} Delivered (W)`, `{name} Setpoint (C)`, `{name} COP (-)` — all present in HARES
- **v5** (`HVAC.py:586-588`): `{name} Duct Losses (W)` — **missing** from HARES
- **v7** (`HVAC.py:590-599`): `{name} Main Power (kW)`, `{name} Fan Power (kW)`, `{name} Latent Gains (W)` (cooling only), `{name} SHR (-)` (cooling only), `{name} Speed (-)`, `{name} Capacity (W)`, `{name} Max Capacity (W)` — **SHR, Speed, Fan Power, Main Power missing** from HARES v7; `Capacity (W)` and `COP (-)` exist at v8 in HARES and must be promoted to v7

### HARES output schema at `columns.rs:198-212`

Verbosity 7 currently only adds `{name} Schedule (-)` and `Hot Water Mains Temperature (C)`. None of the HVAC performance columns from OCHRE v5/v7 are included.

### Telemetry availability

All required data is already computed and written to telemetry:

- `tk::SHR` — written at `air_conditioner.rs:848`
- `tk::SPEED_INDEX` — written at `air_conditioner.rs:852` and `furnace.rs:428`
- `tk::FAN_KW` — written at `air_conditioner.rs:865` and `furnace.rs:191,417`
- `tk::COMPRESSOR_KW` — written at `air_conditioner.rs:864`
- `tk::RUNTIME_FRACTION` — written at `air_conditioner.rs:863`
- `tk::LATENT_COOLING_W` — written at `air_conditioner.rs:847`

Only `DUCT_LOSS_W` is not currently tracked as a separate telemetry key — it is computed implicitly as `gross_capacity_w * (1 - duct_dse)` in the port distribution but not stored.

> **Important**: The telemetry values `sensible_cooling_w` and `latent_cooling_w` are POST-DSE
> (i.e., they have already been multiplied by DSE). They **cannot** be used to compute duct
> losses. The correct formula is `duct_loss_w = gross_capacity_w * (1.0 - dse)` where
> `gross_capacity_w` is the pre-DSE value. Either add a new gross-capacity telemetry key
> or compute duct losses in the port distribution code where gross values are available
> (in `duct_distribution.rs` where `zone_heat_fractions` are computed).

## Required Behavior

At output verbosity 7, HARES must produce all OCHRE-equivalent columns for HVAC equipment:

1. `{name} SHR (-)` — for cooling equipment only (0.0 for heating)
2. `{name} Speed (-)` — speed index (0-based, matches OCHRE `speed_idx`)
3. `{name} Fan Power (kW)` — fan electric power
4. `{name} Main Power (kW)` — total input power minus fan power, matching OCHRE `HVAC.py:575`: `main_power_kw = total_input_power_kw - fan_kw`. For gas equipment, `total_input_power_kw` includes both electric and gas (gas converted to kW via `gas_therms_per_hour / kwh_to_therms`). For ASHP heater specifically, OCHRE separates `Main Power` (compressor-only) from `ER Power` (`HVAC.py:1464-1467`).
5. `{name} Duct Losses (W)` — gross_capacity × (1 − dse), at verbosity 5+ (matching OCHRE placement). **Must use pre-DSE gross capacity, not post-DSE telemetry values.**
6. `{name} Runtime Fraction (-)` — PLR/PLF (the RTF value)

## Approach

### Step 1: Add and promote column definitions at verbosity 7 in `columns.rs`

In the `if verbosity >= 7` block at `columns.rs:198-212`, add per-equipment HVAC columns for equipment identified as HVAC by the existing `is_hvac_or_wh()` helper. Additionally, move `{name} Capacity (W)` and `{name} COP (-)` from the `if verbosity >= 8` block (`columns.rs:214-228`) into this block — these columns already exist at v8 and are promoted to v7 to match OCHRE verbosity-7 output:

```rust
if verbosity >= 7 {
    // Existing schedule and mains temp columns...
    for (name, _fuel) in &names {
        if is_hvac_or_wh(name) {
            fields.push(Field::new(format!("{name} SHR (-)"), DataType::Float64, true));
            fields.push(Field::new(format!("{name} Speed (-)"), DataType::Float64, true));
            fields.push(Field::new(format!("{name} Fan Power (kW)"), DataType::Float64, true));
            fields.push(Field::new(format!("{name} Main Power (kW)"), DataType::Float64, true));
            fields.push(Field::new(format!("{name} Runtime Fraction (-)"), DataType::Float64, true));
            // Promoted from v8: already exist, now also exposed at v7.
            fields.push(Field::new(format!("{name} Capacity (W)"), DataType::Float64, true));
            fields.push(Field::new(format!("{name} COP (-)"), DataType::Float64, true));
        }
    }
}
```

Remove the `{name} Capacity (W)` and `{name} COP (-)` entries from the `if verbosity >= 8` block since they are now covered at v7. The v8 block (`columns.rs:214-228`) must not duplicate them.

### Step 2: Add Duct Losses column at verbosity 5

In the `if verbosity >= 5` block at `columns.rs:150-170`, add:

```rust
fields.push(Field::new("HVAC Duct Losses (W)", DataType::Float64, true));
```

This ticket **owns** the `DUCT_LOSS_W` telemetry key and the v5 `"HVAC Duct Losses (W)"` column definition. Ticket 021 must not independently add this column; see "Coordination with 021" below.

### Step 3: Add `DUCT_LOSS_W` telemetry key

In `telemetry_keys.rs`, add:

```rust
pub const DUCT_LOSS_W: &str = "duct_loss_w";
```

### Step 4: Write `DUCT_LOSS_W` in equipment step methods

In each HVAC equipment `step()`, compute and store duct losses:

- `air_conditioner.rs` step: `duct_loss_w = gross_capacity_w * (1.0 - dse)` — **must use pre-DSE gross capacity**, NOT `sensible_cooling_w + latent_cooling_w` which are post-DSE
- `furnace.rs` step: `duct_loss_w = gross_capacity_w * (1.0 - dse)`
- `heater.rs` step (ASHP/MSHP heating): `duct_loss_w = gross_heating_capacity_w * (1.0 - dse)`

**Alternatively**, compute `DUCT_LOSS_W` in `duct_distribution.rs` where `zone_heat_fractions` are computed, since gross capacity values are already available in that context. This avoids the need for a new gross-capacity telemetry key.

Add to both `default_telemetry()` initializers and `telemetry_fields()` descriptors.

### Step 5: Route telemetry to output columns in `record_step`

In `dwelling/mod.rs:record_step()` (around line 2515), add column routing for the new fields. Map from existing telemetry keys to the new output column names:

- `tk::SHR` → `{name} SHR (-)`
- `tk::SPEED_INDEX` → `{name} Speed (-)`
- `tk::FAN_KW` → `{name} Fan Power (kW)`
- `tk::COMPRESSOR_KW` → `{name} Main Power (kW)` (for cooling); for furnaces/heaters, compute as `total_input_power_kw - fan_kw` per OCHRE definition. For gas equipment, total input includes gas (converted to kW). For ASHP, `Main Power` = compressor-only; ER power tracked separately as `ER Power` per `HVAC.py:1464-1467`.
- `tk::RUNTIME_FRACTION` → `{name} Runtime Fraction (-)`
- `tk::DUCT_LOSS_W` → `HVAC Duct Losses (W)` (aggregate at v5)

### Step 6: Add "Main Power" telemetry key

Add `MAIN_POWER_KW` to `telemetry_keys.rs`. This follows OCHRE's definition at `HVAC.py:575`:
`main_power_kw = total_input_power_kw - fan_kw`.

- **Cooling equipment (AC/ASHP cooling)**: `main_power_kw = compressor_kw` (already tracked; fan is separate)
- **Gas furnace**: `main_power_kw = gas_input_kw` (gas converted to kW via `gas_therms_per_hour / kwh_to_therms`) — fan is excluded
- **Electric furnace**: `main_power_kw = electric_kw - fan_kw` (resistive element power)
- **ASHP heating**: `main_power_kw = compressor_kw` (compressor-only per OCHRE `HVAC.py:1464-1467`); ER power is tracked separately if present

### Step 7: Update `equipment_column_map` pre-computation

Extend the `EquipmentColumns` struct and the column-resolution logic in `conversions.rs:build_output_column_index()` to include the new column indices.

## Definition of Done

- [ ] `{name} SHR (-)` column present at verbosity 7 for cooling equipment
- [ ] `{name} Speed (-)` column present at verbosity 7 for all HVAC equipment
- [ ] `{name} Fan Power (kW)` column present at verbosity 7 for all HVAC equipment
- [ ] `{name} Main Power (kW)` column present at verbosity 7 for all HVAC equipment
- [ ] `HVAC Duct Losses (W)` column present at verbosity 5 (this ticket owns the column and `DUCT_LOSS_W` key)
- [ ] `{name} Runtime Fraction (-)` column present at verbosity 7 for all HVAC equipment including furnace
- [ ] `RUNTIME_FRACTION` telemetry key written in `furnace.rs` `step()` method (currently absent; `air_conditioner.rs:863` already writes it; furnace must match)
- [ ] `{name} Capacity (W)` and `{name} COP (-)` promoted from v8 to v7; no longer duplicated at v8
- [ ] All columns populated with correct values from telemetry
- [ ] Non-HVAC equipment gets 0.0 for HVAC-specific columns (no missing values)
- [ ] Existing tests pass; new schema tests added for v5 and v7 columns

## Verification

1. Run a simulation at verbosity 7 with a central AC and a gas furnace. Verify all new columns appear with non-zero values during operation.
2. Compare output column values against OCHRE output at the same verbosity for the same building configuration.
3. Verify `DUCT_LOSS_W` = `gross × (1 - dse)` matches OCHRE's `{name} Duct Losses (W)`.
4. Verify duct losses are computed for both cooling and heating modes (not just cooling/furnace). ASHP and MSHP heating must also include duct losses when DSE < 1.0.
5. Run `build_schema` unit tests with verbosity 5 and 7 to confirm new columns appear.

## References

- OCHRE `HVAC.py:568-604`: `generate_results()` — verbosity-tiered output columns
- OCHRE `HVAC.py:1460-1469`: ASHP heater overrides `Main Power` and adds `ER Power`
- `columns.rs:150-212`: HARES verbosity-tiered schema construction
- `air_conditioner.rs:843-877`: CoolingCore telemetry writes in `step()`
- `furnace.rs:191-199,417-428`: Furnace telemetry writes in `step()`
- `telemetry_keys.rs:1-164`: Existing telemetry key constants

## Coordination with 021

This ticket owns the `DUCT_LOSS_W` telemetry key and the `"HVAC Duct Losses (W)"` column at verbosity 5. Ticket 021 must treat this ticket as a prerequisite and consume the existing column rather than re-adding it. 021 only adds per-zone attribution columns (`{name} Conditioned Zone Fraction (-)` etc.) and must not introduce a second duct-losses column.

## Ordering

**018 must ship before this ticket.** 018 builds the `CoreOutput` surface from which thermal/COP/setpoint values are routed. If this ticket lands first it wires directly from telemetry and must be rewired after 018 ships. Ship 018 first to avoid the double-wiring cost.

**019 must ship before this ticket's RTF column work.** The PLR/PLF telemetry keys that back the `{name} Runtime Fraction (-)` column are introduced in 019.

## Related Tickets

- #018 — Promote thermal/COP/setpoint to CoreOutput (must ship first; see Ordering)
- #019 — Expose PLR, PLF, speed_frac as telemetry keys (must ship first; see Ordering)
- #021 — Per-zone thermal attribution (consumes `DUCT_LOSS_W` from this ticket; see Coordination)
