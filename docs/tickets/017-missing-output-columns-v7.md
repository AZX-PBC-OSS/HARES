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

---

## Verification Audit

**Auditor**: claude-cli (automated)
**Date**: 2026-05-20

### Code Confirmation

- [x] **Referenced line numbers still match** — with minor shifts noted below:
  - `columns.rs:198-212` (v7 block) and `columns.rs:214-228` (v8 block): **exact match**. v7 adds only `{name} Schedule (-)` and `Hot Water Mains Temperature (C)`. v8 adds `{name} Capacity (W)` and `{name} COP (-)`. None of the HVAC performance columns from OCHRE v5/v7 are present in either block.
  - `air_conditioner.rs:848` (`SHR`), `:852` (`SPEED_INDEX`), `:863` (`RUNTIME_FRACTION`), `:864` (`COMPRESSOR_KW`), `:865` (`FAN_KW`), `:847` (`LATENT_COOLING_W`): **all confirmed present at those exact lines**.
  - `furnace.rs:191` (`FAN_KW`), `:417` (`FAN_KW` again in gas branch), `:428` (`SPEED_INDEX`): **exact match**.
  - `columns.rs:217-220` (`Capacity (W)` at v8) and `:222-226` (`COP (-)` at v8): **confirmed at those exact lines**.

- [x] **Described logic matches current implementation**:
  - v7 block (`columns.rs:198-212`) adds `Schedule (-)` and `Hot Water Mains Temperature (C)` only — confirmed absent are SHR, Speed, Fan Power, Main Power, Runtime Fraction, Capacity, COP.
  - v8 block (`columns.rs:214-228`) adds `Capacity (W)` and `COP (-)` — confirmed. Promoting these to v7 and removing from v8 is the correct approach.
  - v5 block (`columns.rs:150-170`) adds reactive power and `Net Sensible Heat Gain - Indoor (W)` only — no duct-losses column.
  - `EquipmentColumns` struct (`dwelling/mod.rs:136-148`): does **not** contain fields for SHR, Speed, Fan Power, Main Power, or Runtime Fraction — confirming the routing gap too.
  - `DUCT_LOSS_W` telemetry key: **absent** from `telemetry_keys.rs` (165 lines, exhaustively read). The `DUCT_LOSS_W` constant does not exist anywhere under `crates/`.

- [x] **Bug is present and not already fixed**: All 8 regression tests added at `crates/hares-io/src/output/columns.rs` (in the `#[cfg(test)]` module) fail with `cargo test -p hares-io --lib -- ticket_017`, confirming the gap is live.

- [x] **OCHRE cross-check — matches**: The vendored OCHRE at `vendors/OCHRE/ochre/Equipment/HVAC.py` is an older version (line numbers ~568-604 vs upstream ~661-703 for the same method). The **logic is identical** between the vendored and upstream copies. Confirmed from both:
  - `vendors/OCHRE/ochre/Equipment/HVAC.py:568-604` (vendored): v4 adds Delivered/Setpoint/COP; v5 adds `Duct Losses (W)` as `abs(delivered_heat) * (1 - duct_dse)`; v7 adds Main Power, Fan Power, Latent Gains (cooling only), SHR (cooling only), Speed, Capacity, Max Capacity.
  - Upstream `NREL/OCHRE` (fetched via `raw.githubusercontent.com/NREL/OCHRE/main/ochre/Equipment/HVAC.py`, lines 661-703): identical logic, same column names.
  - ASHP heater override in vendored `HVAC.py:1460-1469` and upstream lines ~1362-1372: both replace `Main Power` with `tot_power - er_power` and add `ER Power (kW)` at v7. Ticket cites this correctly.

- [x] **EnergyPlus cross-check — N/A**: This ticket is about output column schema and verbosity-level gating, which is an OCHRE-specific feature. EnergyPlus uses a different output variable system (`Output:Variable`) and does not have a direct DSE-based duct loss reporting equivalent. The DSE formula itself (`duct_loss_w = gross_capacity_w * (1 - dse)`) originates from ASHRAE 152, referenced below. No EnergyPlus divergence to assess.

### Web-Verified Citations

**Citation 1**: OCHRE `HVAC.py:568-604` — `generate_results()` verbosity-tiered output columns
- **Source found**: `https://raw.githubusercontent.com/NREL/OCHRE/main/ochre/Equipment/HVAC.py` (upstream NREL/OCHRE repository, main branch, fetched 2026-05-20)
- **Quoted passage** (upstream lines 661-703, equivalent to vendored 568-604):
  ```python
  if self.verbosity >= 4:
      results[f'{self.end_use} Delivered (W)'] = abs(self.delivered_heat) * self.duct_dse
      results[f'{self.end_use} Setpoint (C)'] = self.temp_setpoint
      results[f'{self.end_use} COP (-)'] = cop
  if self.verbosity >= 5:
      results[f'{self.end_use} Duct Losses (W)'] = abs(self.delivered_heat) * (1 - self.duct_dse)
  if self.verbosity >= 7:
      results[f'{self.end_use} Main Power (kW)'] = main_power
      results[f'{self.end_use} Fan Power (kW)'] = self.fan_power / 1000
      # cooling only:
      results[f'{self.end_use} Latent Gains (W)'] = self.latent_gain * self.space_fraction
      results[f'{self.end_use} SHR (-)'] = self.shr if on or self.show_eir_shr else 0
      results[f'{self.end_use} Speed (-)'] = self.speed_idx
      results[f'{self.end_use} Capacity (W)'] = self.capacity
      results[f'{self.end_use} Max Capacity (W)'] = self.capacity_max
  ```
  where `main_power = self.electric_kw + self.gas_therms_per_hour / kwh_to_therms - self.fan_power / 1000`
- **Verdict**: **Confirmed**. Column names, verbosity levels, and the `main_power` formula all match the ticket's description.

**Citation 2**: OCHRE `HVAC.py:1460-1469` — ASHP heater override of `Main Power` and `ER Power`
- **Source found**: Same upstream file, lines ~1362-1372 (upstream) / 1460-1469 (vendored)
- **Quoted passage**:
  ```python
  def generate_results(self):
      results = super().generate_results()
      if self.verbosity >= 7:
          tot_power = self.capacity * self.eir * self.space_fraction / 1000
          er_power = self.er_capacity * self.er_eir_rated * self.space_fraction / 1000
          results[f'{self.end_use} Main Power (kW)'] = tot_power - er_power
          results[f'{self.end_use} ER Power (kW)'] = er_power
      return results
  ```
- **Verdict**: **Confirmed**. The ticket's statement that ASHP separates compressor-only power from ER power is accurate.

**Citation 3**: OCHRE duct DSE formula — `abs(delivered_heat) * (1 - duct_dse)` as duct losses
- **Source found**: Same upstream HVAC.py; also `vendors/OCHRE/ochre/Equipment/HVAC.py:165-197` (DSE zone-fraction setup, vendored)
- **Quoted passage** (vendored HVAC.py:189-192):
  ```python
  self.zone_fractions = {self.zone: self.duct_dse * (1 - self.basement_heat_frac)}
  if self.duct_dse < 1 and self.duct_zone is not None:
      self.zone_fractions[self.duct_zone] = 1 - self.duct_dse
  ```
  This confirms that `1 - duct_dse` is the fraction of gross capacity lost to ducts. At output time (HVAC.py:588): `results[f'{self.end_use} Duct Losses (W)'] = abs(self.delivered_heat) * (1 - self.duct_dse)` where `delivered_heat` is the **gross** (pre-DSE) capacity. The ticket correctly warns that HARES post-DSE telemetry values cannot be used for this calculation.
- **Verdict**: **Confirmed**. The duct loss formula and the pre-DSE/post-DSE distinction are accurately described.

**Citation 4**: `main_power_kw = total_input_power_kw - fan_kw` (ticket Step 6 citing `HVAC.py:575`)
- **Source found**: Same upstream HVAC.py (line 575 in vendored version corresponds to the `main_power` calculation in `generate_results`)
- **Quoted passage**: `main_power = self.electric_kw + self.gas_therms_per_hour / kwh_to_therms - self.fan_power / 1000`
- **Verdict**: **Confirmed**. The formula exactly matches the ticket's description. `kwh_to_therms ≈ 0.0341 therms/kWh` (standard conversion: 1 therm = 29.307 kWh), imported from `ochre.utils.units`.

**Citation 5**: `columns.rs:198-212` (v7 block) and `columns.rs:217-220`, `columns.rs:222-226` (v8 Capacity/COP)
- **Source found**: `/Users/rich/source/HARES/crates/hares-io/src/output/columns.rs`, read directly
- **Quoted passage** (lines 198-228):
  ```rust
  if verbosity >= 7 {
      // Level 7: schedule inputs and detailed equipment modes.
      for (name, _fuel) in &names {
          fields.push(Field::new(format!("{name} Schedule (-)"), ...));
      }
      fields.push(Field::new(HOT_WATER_MAINS_TEMP_COL, ...));
  }
  if verbosity >= 8 {
      // Level 8: all individual equipment state variables.
      for (name, _fuel) in &names {
          fields.push(Field::new(format!("{name} Capacity (W)"), ...));  // line 217-220
          fields.push(Field::new(format!("{name} COP (-)"), ...));        // line 222-226
      }
  }
  ```
- **Verdict**: **Confirmed**. Line numbers match exactly.

**Citation 6**: `air_conditioner.rs:843-877` — telemetry writes in `step()`
- **Source found**: `/Users/rich/source/HARES/crates/hares-equipment/src/hvac/air_conditioner.rs`, read directly
- **Quoted passage** (lines 843-865): `tk::ELECTRIC_KW` (843), `tk::SENSIBLE_COOLING_W` (845), `tk::LATENT_COOLING_W` (847), `tk::SHR` (848), `tk::OPERATING_MODE` (850), `tk::SPEED_INDEX` (852), `tk::COP` (861), `tk::RUNTIME_FRACTION` (863), `tk::COMPRESSOR_KW` (864), `tk::FAN_KW` (865)
- **Verdict**: **Confirmed**. The ticket's line-number citations for `air_conditioner.rs` are accurate.

**Citation 7**: `furnace.rs:191-199` and `furnace.rs:417-428` — furnace telemetry
- **Source found**: `/Users/rich/source/HARES/crates/hares-equipment/src/hvac/furnace.rs`, read directly
- **Quoted passage** (lines 191-199): writes `FAN_KW`, `ELECTRIC_KW`, `THERMAL_OUTPUT_W`, `OPERATING_MODE`, `SUPPLY_AIR_TEMP_C`, setpoints; (lines 417-428): writes `FAN_KW`, `ELECTRIC_KW` (set to `fan_kw` — see note below), `FUEL_INPUT_W`, `THERMAL_OUTPUT_W`, `OPERATING_MODE`, `SUPPLY_AIR_TEMP_C`, setpoints, `SPEED_INDEX`; **`RUNTIME_FRACTION` is absent from both blocks**.
- **Verdict**: **Confirmed** that furnace does not write `RUNTIME_FRACTION`. Additionally, at `furnace.rs:418`, the gas furnace sets `tk::ELECTRIC_KW = fan_kw` rather than total electric consumption — a separate bug but not this ticket's concern.

### Additional Finding Not Noted in Ticket

**Latent Gains column name discrepancy**: OCHRE emits `{name} Latent Gains (W)` (not `Latent Cooling (W)`) for cooling equipment at v7 (HVAC.py:595: `results[f'{self.end_use} Latent Gains (W)']`). The ticket's problem table lists this under "Status: Missing" and cites `tk::LATENT_COOLING_W` as the source. The ticket's approach section does not define a column definition for `{name} Latent Gains (W)`, only covering SHR, Speed, Fan Power, Main Power, RTF, Capacity, and COP. This column is **also missing** from the approach steps — the ticket's DoD and Step 1 pseudocode omit it. This is an incompleteness in the ticket, not an error in the core claim.

### Legitimacy

- **Verdict**: **Legitimate**

- **Rationale**: Every specific claim in the ticket is independently verifiable and correct. The OCHRE HVAC.py source (both vendored and upstream NREL GitHub) confirms the exact verbosity-level column assignments. The HARES code at `columns.rs:198-228` confirms that none of the v7 OCHRE columns (SHR, Speed, Fan Power, Main Power, Capacity, COP) appear at v7, that COP and Capacity are erroneously v8-only, and that the v5 duct-losses column is absent. The `telemetry_keys.rs` file confirms `DUCT_LOSS_W` does not exist. The `furnace.rs` code confirms `RUNTIME_FRACTION` is not written by the furnace. The `EquipmentColumns` struct confirms the routing infrastructure does not yet have slots for the new fields. The duct-loss formula `gross_capacity_w * (1 - dse)` and the pre-DSE/post-DSE distinction are correctly described and match OCHRE's implementation. The one incompleteness is that `{name} Latent Gains (W)` (OCHRE HVAC.py:595) is identified in the problem table but omitted from the Approach Steps and DoD — implementors should add it.

### Proposed Fix Summary

1. In `columns.rs` v7 block (`if verbosity >= 7`, currently line 198), for each equipment name matching `is_hvac_or_wh()`, add fields: `{name} SHR (-)`, `{name} Speed (-)`, `{name} Fan Power (kW)`, `{name} Main Power (kW)`, `{name} Runtime Fraction (-)`, `{name} Latent Gains (W)` (cooling equipment only — needs a cooling-equipment predicate), `{name} Capacity (W)`, `{name} COP (-)`.
2. Remove `{name} Capacity (W)` and `{name} COP (-)` from the v8 block to avoid duplication.
3. In `columns.rs` v5 block (`if verbosity >= 5`, line 150), add a static `"HVAC Duct Losses (W)"` field.
4. In `telemetry_keys.rs`, add `pub const DUCT_LOSS_W: &str = "duct_loss_w";` and `pub const MAIN_POWER_KW: &str = "main_power_kw";`.
5. In `air_conditioner.rs`, `furnace.rs`, and `heater.rs` `step()` methods, compute and write `DUCT_LOSS_W = gross_capacity_w * (1.0 - dse)` (pre-DSE gross, not post-DSE telemetry). Write `MAIN_POWER_KW` per the OCHRE formula.
6. In `furnace.rs` `step()`, add `self.telemetry.set(tk::RUNTIME_FRACTION, ...)` matching the air-conditioner pattern.
7. Extend `EquipmentColumns` struct (`dwelling/mod.rs:136`) with new fields (`shr`, `speed`, `fan_kw`, `main_power_kw`, `runtime_fraction`, `latent_gains`) and populate them in `build_equipment_column_map()` and `record_step()`.
8. Do **not** use post-DSE `sensible_cooling_w` or `latent_cooling_w` for duct loss calculation.

### Test Written

- **File**: `crates/hares-io/src/output/columns.rs` (in the existing `#[cfg(test)] mod tests` block at the bottom of the file)
- **Tests added** (all currently failing, confirming the bug):
  - `ticket_017_verbosity_7_includes_shr_column_for_cooling_equipment`
  - `ticket_017_verbosity_7_includes_speed_column_for_hvac_equipment`
  - `ticket_017_verbosity_7_includes_fan_power_column`
  - `ticket_017_verbosity_7_includes_main_power_column`
  - `ticket_017_verbosity_7_includes_runtime_fraction_column`
  - `ticket_017_capacity_column_promoted_from_v8_to_v7`
  - `ticket_017_cop_column_promoted_from_v8_to_v7`
  - `ticket_017_verbosity_5_includes_duct_losses_column`
- **Verified failing**: `cargo test -p hares-io --lib -- ticket_017` → 8 failed, 0 passed. Pre-existing 484 tests unaffected.
