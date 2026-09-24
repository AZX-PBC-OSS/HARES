# Output column definitions: completeness, naming conventions, unit labeling
**Review ID**: output-02
**Category**: output
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-io/src/output/columns.rs`
- `crates/hares-types/src/telemetry.rs`
- `crates/hares-types/src/telemetry_keys.rs`
- `crates/hares-core/src/dwelling/mod.rs` (record_step, build_equipment_column_map, extend_schema_with_actor_columns)

## Vendor/Reference Files Consulted
- `vendors/EnergyPlus/src/EnergyPlus/OutputProcessor.cc`

## Findings

### Finding 1: [Severity: critical] No compile-time or runtime validation of telemetry key to output column mapping

**Description**: The linkage between internal telemetry keys (e.g., `tk::FAN_KW` = `"fan_kw"`) and OCHRE-style output column names (e.g., `"ASHP Heater Fan Power (kW)"`) is entirely based on loose string-formatting at runtime, with no compile-time verification. If a column name string in `columns.rs` drifts from the corresponding lookup in `build_equipment_column_map()` or `record_step()`, data is silently dropped into a null column.

**Code Location**:
- `crates/hares-core/src/dwelling/mod.rs:167-227` — `build_equipment_column_map()` resolves column indices from OCHRE-style column name strings
- `crates/hares-core/src/dwelling/mod.rs:2910-3019` — `record_step()` reads specific telemetry keys and writes to pre-resolved column indices
- `crates/hares-core/src/dwelling/mod.rs:2984` — `eq.telemetry().get(tk::DEFROST_CYCLE_STATE).unwrap_or(0.0)` — silent zero-fill if key missing
- `crates/hares-core/src/dwelling/mod.rs:3001` — `eq.telemetry().get(tk::FAN_KW).unwrap_or(0.0)` — silent zero-fill if key missing

**Root Cause**: The architecture uses two independent naming conventions — snake_case telemetry key constants in `telemetry_keys.rs` and OCHRE HumanReadable column names in `columns.rs` — with a loosely-coupled `HashMap<String, usize>` for index resolution. There is no type-level mapping between a telemetry key and its output column. Compare to EnergyPlus's `SetupOutputVariable()` (`OutputProcessor.cc:1433-1517`) which explicitly registers output variables with typed units, meter attachments, and duplicate-detection at setup time, producing a compile-time-visible registration.

**Impact**: 
- A typo in a column name format string (e.g., `"Mode (-)"` changed to `"Modes (-)"` in only one of the two files) silently stops data emission for that column
- A telemetry key renamed in `telemetry_keys.rs` but not updated in `record_step()` silently zero-fills the column
- The `Option<usize>` pattern throughout `EquipmentColumns` means every column is nullable by construction — there is no `debug_assert!` or `tracing::warn!` when a column index resolves to `None`
- No single source of truth exists for "which telemetry keys feed which output columns" — the mapping is spread across three separate code regions

---

### Finding 2: [Severity: high] Telemetry keys without output column coverage — opaque internal/external distinction

**Description**: Of the 125+ telemetry key constants defined in `telemetry_keys.rs`, fewer than 20 are consumed by the output system. The remainder are used only for internal equipment control logic and inter-equipment communication, but there is no documentation, naming convention, or compile-time marker that distinguishes "output-bound" keys from "internal-only" keys. This makes it impossible to audit completeness without exhaustive manual tracing.

**Code Location**: `crates/hares-types/src/telemetry_keys.rs` (all 231 lines)

**Root Cause**: All telemetry keys share a single flat namespace (`HashMap<String, f64>`) with no metadata about their intended visibility scope. EnergyPlus addresses this through `SetupOutputVariable()` which requires an explicit registration step — a variable is internal until explicitly registered for output.

**Impact**:
- Consumers cannot determine if a missing signal (e.g., compressor power, battery temperature) is absent because the equipment doesn't compute it or because HARES lacks an output column for it
- The following telemetry categories have no direct output columns:
  - **Equipment temperatures**: `CELL_TEMP_C`, `BATTERY_TEMP_C`, `SUPPLY_TEMP_C`, `SUPPLY_AIR_TEMP_C`, `RETURN_TEMP_C`, `TANK_AVG_TEMP_C`, `OUTLET_TEMP_C` — only available via actor telemetry pass-through
  - **Detailed HVAC power breakdown**: `COMPRESSOR_KW`, `COMPRESSOR_POWER_W`, `FAN_ELECTRIC_W`, `FAN_POWER_W`, `PAN_HEATER_KW`, `HP_CAPACITY_W`, `ER_CAPACITY_W` — only `FAN_KW` and `BACKUP_ER_KW` have columns
  - **EV-specific**: `CONNECTION_STATE`, `CHARGING_LEVEL`, `V2L_POWER_KW`, `AWAY_CHARGE_POWER_KW`, `CAPACITY_KWH`, `FUEL_ECONOMY_KWH_PER_MI`
  - **PV/solar**: `DC_POWER_KW`, `IRRADIANCE_W_M2`, `CURTAILMENT_KW`, `SOILING_RATIO`, `SHADING_FACTOR`
  - **Setpoint chain**: `SCHEDULE_HEATING_SETPOINT_C`, `SCHEDULE_COOLING_SETPOINT_C`, `RUNTIME_HEATING_SETPOINT_C`, `RUNTIME_COOLING_SETPOINT_C`
  - **Electrical detail**: `TERMINAL_VOLTAGE_V`, `CURRENT_A`, `OHMIC_LOSS_W`, `STANDBY_POWER_W`, `HEATER_POWER_W`
  - **Water heater**: `ELEMENT_KW`, `PILOT_KW`, `FUEL_INPUT_KW`, `UPPER_ELEMENT_POWER_W`, `LOWER_ELEMENT_POWER_W`, `BURNER_POWER_W`, `PILOT_POWER_W`, `BACKUP_ELEMENT_POWER_W`
  - **Defrost detail**: `DEFROST_TIME_FRACTION`, `DEFROST_EXTRA_POWER_W`, `DEFROST_Q_W`, `DEFROST_CAPACITY_MULTIPLIER`, `DEFROST_ACCUMULATED_FROST_S`, `DEFROST_ELAPSED_S` — only `DEFROST_CYCLE_STATE` has a column
  - **Efficiency**: `EIR`, `CAP_MULT`, `ETA_ELECTRIC`, `INVERTER_EFFICIENCY`, `CAP_RATIO`, `EIR_RATIO` — only `COP` and `SHR` have columns
  - **Cycle/phase**: `CYCLE_PHASE`, `TIME_AT_CURRENT_SPEED_S`, `MODE_DURATION_S`, `RAMP_LIMITED`

---

### Finding 3: [Severity: high] Unconditionally added attic columns produce all-null output for non-attic buildings

**Description**: At verbosity 6, the static columns `"Infiltration Heat Gain - Attic (W)"` and `"Interior LWR Exchange - Attic (W)"` are always added to the schema, but their population code path (`zone_infiltration_columns` and `zone_lwr_columns` pre-resolved caches) only produces values if an attic zone is present in the thermal model. In single-zone or basement-only buildings, these columns are present in every row but always null.

**Code Location**:
- `crates/hares-io/src/output/columns.rs:205-206` — hardwarecoded attic columns at v6
- `crates/hares-core/src/dwelling/mod.rs:3111-3121` — infiltration/LWR by-zone population only covers zones that exist in the thermal solver

**Root Cause**: The attic envelope columns are added unconditionally (statically) at verbosity 6 rather than conditionally based on the zone_names list, unlike the per-zone HVAC attribution columns (columns.rs:213-224) which are dynamically generated from the `zone_names` parameter.

**Impact**: Wasted storage for all-null columns in every row for simulations without attic zones. Downstream consumers see a column that exists but has no data, requiring defensive null-handling.

---

### Finding 4: [Severity: medium] Column naming convention is inconsistent across OCHRE patterns

**Description**: The column naming code follows OCHRE's `"{Name} {Metric} ({Unit})"` convention (stated in columns.rs:3) but contains several deviations:

1. **"Outdoor Dry Bulb (C)"** (line 32) uses `"{Metric} ({Unit})"` format without a zone name prefix, inconsistently with `"Temperature - Indoor (C)"`, `"Temperature - Attic (C)"`, `"Temperature - Ground (C)"` which use `"Temperature - {Zone} (C)"`
2. **"Net Sensible Heat Gain - Indoor (W)"** (line 34) appends zone as a suffix (`"{Metric} - {Zone} ({Unit})"`), while envelope component columns use `"{Metric} - {Zone} ({Unit})"` consistently — this is a consistent sub-pattern, but inconsistent with zone temperatures where the zone name appears between dashes after the metric
3. **"HVAC Heating Delivered (W)"** (line 152) lacks a zone suffix (uses the dwelling-aggregate form), while per-zone variants at v6 use `"HVAC Heating Delivered - {Zone} (W)"`
4. **"Indoor Temperature (C)"** appears as a runtime fallback (mod.rs:3040) suggesting a recent rename from this old form to `"Temperature - Indoor (C)"`

**Code Location**:
- `crates/hares-io/src/output/columns.rs:32-34`
- `crates/hares-io/src/output/columns.rs:151-157`
- `crates/hares-core/src/dwelling/mod.rs:3040` — old-style fallback

**Root Cause**: The OCHRE naming convention was adopted from the vendor reference but adapted piecemeal as new columns were added. EnergyPlus avoids this by using a keyed-variable model (`{Key}:{Variable} [{Unit}]`) that places metadata in delimited fields rather than free-form text.

**Impact**: Downstream consumers parsing column names for automated unit extraction or aggregation must handle multiple formats. The `"Outdoor Dry Bulb (C)"` / `"Temperature - Indoor (C)"` inconsistency means zone-name extraction logic needs special cases.

---

### Finding 5: [Severity: medium] Column ordering is deterministic but lacks stability guarantee

**Description**: The `build_schema()` function produces columns in a deterministic order (timestamp → level-0 totals → always-present context → per-equipment power → zone data → ...), but no specification or test asserts that this ordering must remain stable across schema versions. If a refactor reorders the verbosity blocks or changes the iteration order over equipment/zone names, downstream consumers relying on positional column access (common in Parquet/Arrow pipelines) would silently receive wrong data.

**Code Location**: `crates/hares-io/src/output/columns.rs:60-315` — `build_schema()` constructs columns in fixed order

**Root Cause**: The column order is an emergent property of the `build_schema()` control flow rather than an explicitly defined ordered list. The `element_columns` test helper (`expected_columns_at_verbosity` at line 322) only covers static columns, not the dynamically-generated per-equipment and per-zone columns that constitute the majority of the schema.

**Impact**: No regression test can detect an accidental reordering. Compare to EnergyPlus's `SetupOutputVariable()` which assigns unique report numbers sequentially — the ordering is implicitly the registration order, but the `.rdd` file explicitly lists all variables by name, allowing consumers to resolve by name rather than position.

---

### Finding 6: [Severity: medium] MODE_ORDINALS metadata omits OperatingMode::On (ordinal 12)

**Description**: The `MODE_ORDINALS` array embedded as Parquet metadata under the `hares_mode_map` key has 12 entries (ordinals 0–11), but `mode_to_ordinal()` maps `OperatingMode::On` to ordinal 12.0. If any equipment emits mode `On`, the data contains an ordinal value not documented in the file's metadata, making the output file non-self-describing.

**Code Location**:
- `crates/hares-io/src/output/columns.rs:376-389` — `MODE_ORDINALS` array (12 entries, 0–11)
- `crates/hares-io/src/output/columns.rs:358-373` — `mode_to_ordinal()` maps `On` to `12.0`

**Root Cause**: `OperatingMode::On` was added to the enum after `MODE_ORDINALS` was defined, and the two were not kept in sync. The test at line 594 (`schema_metadata_includes_mode_map`) asserts the array length is 12 but never validates it covers all enum variants.

**Impact**: Consumers reading `hares_mode_map` from Parquet metadata will not know what ordinal 12 represents, breaking round-trip interpretability.

---

### Finding 7: [Severity: low] HVAC Duct Losses column uses duplicate-index-per-equipment pattern

**Description**: The `"HVAC Duct Losses (W)"` column is a dwelling-level aggregate column, but its index is resolved per-equipment in `EquipmentColumns`. Every equipment's `cols.duct_losses` resolves to the same column index, and the population code at `mod.rs:3017` uses `+=` to accumulate contributions. This works but creates a latent bug surface: if the column-name lookup logic diverges between equipment instances (e.g., different schema construction), different equipment would write to different columns.

**Code Location**:
- `crates/hares-core/src/dwelling/mod.rs:223` — all equipment resolve to same `"HVAC Duct Losses (W)"` index
- `crates/hares-core/src/dwelling/mod.rs:3013-3018` — accumulated via `+=`

**Root Cause**: The duct-loss aggregate column was retrofitted into the per-equipment column resolution pattern. It should be a dwelling-level column in the same style as `"Total Electric Power (kW)"` (lines 2914-2915).

**Impact**: Low in practice because the column name is a constant string rather than per-equipment, but the pattern is fragile and violates the principle that `EquipmentColumns` should only contain per-equipment columns.

---

### Finding 8: [Severity: low] "Temperature - Attic (C)" always-null in non-attic buildings

**Description**: Similar to Finding 3 but for zone temperature columns: `"Temperature - Attic (C)"` is always added at verbosity 2+, and `"Temperature - Ground (C)"` at verbosity 2+. These are populated via `zone_temp_scratch` which only contains zones present in the thermal model. For buildings without explicit attic or ground zones, these columns are always null.

**Code Location**:
- `crates/hares-io/src/output/columns.rs:108-113` — unconditional attic and ground temperature columns at v2+
- `crates/hares-core/src/dwelling/mod.rs:3034-3038` — population via `zone_temp_scratch`

**Root Cause**: These columns are added unconditionally (static) rather than being gated on zone_names presence, unlike the per-zone HVAC attribution columns at v6 which are dynamically generated from `zone_names`.

**Impact**: Wasted storage for null columns in single-zone simulations; unlike Finding 3, the temperature columns are present from v2 onward, affecting a broader range of output.

## Summary
- Total findings: 8
- Critical: 1 (no compile-time or runtime validation of key→column mapping)
- High: 2 (telemetry keys without output coverage, unconditional attic columns always-null)
- Medium: 3 (naming convention inconsistency, undocumented column ordering, MODE_ORDINALS omission)
- Low: 2 (duct-losses pattern, attic/ground temps always-null)

## Recommendations

1. **Create a type-level telemetry-to-column mapping registry** with compile-time verification. Define a struct or const array that pairs each telemetry key constant with its OCHRE-style output column name template and verbosity level. Generate both the `EquipmentColumns` lookup and the column-population code from this single source of truth. This would make a typo a compile-time error rather than a silent null column.

2. **Add debug_assert or tracing for unresolved column indices**. At minimum, in `record_step()` or `build_equipment_column_map()`, emit a `tracing::warn!` when a column index resolves to `None`, so dropped data is visible during development and integration testing.

3. **Gate static columns on zone_names presence**. Move `"Temperature - Attic (C)"`, `"Temperature - Ground (C)"`, `"Infiltration Heat Gain - Attic (W)"`, and `"Interior LWR Exchange - Attic (W)"` into the dynamic section that iterates zone_names (like per-zone HVAC attribution), so they are only present when the zones exist in the model.

4. **Normalize column naming to a single convention**. Choose one of:
   - OCHRE-style: `"{Metric} - {Zone} ({Unit})"` — "Temperature - Indoor (C)", "Net Sensible Heat Gain - Indoor (W)"
   - EnergyPlus-style: `"{Zone}:{Metric} [{Unit}]"` — "Indoor:Temperature [C]"
   - Rename `"Outdoor Dry Bulb (C)"` to `"Temperature - Outdoor (C)"` for consistency

5. **Fix MODE_ORDINALS to include OperatingMode::On**. Add `("On", 12)` to the `MODE_ORDINALS` array and update the test to assert the correct count (13). Consider using a proc macro or build script that derives `MODE_ORDINALS` from the `OperatingMode` enum definition to prevent future drift.

6. **Document column ordering contract**. Add a comment above `build_schema()` explicitly stating the ordering guarantee (time column first, then sorted by verbosity level block, equipment in iteration order, zones in alphabetical order). Add a regression test that asserts the full column order for a known equipment list and verbosity.

7. **Add a "column population audit" test**. An integration test that runs a minimal simulation with known equipment, captures the output schema and data, and asserts that every non-nullable column has at least one non-null value and that every column with all-null values has a documented reason why.

8. **Consider adopting EnergyPlus-style explicit variable registration** as a long-term architectural improvement. Each output variable would be registered with its name, units, data type, and pointer to the source data, enabling automatic schema construction, unit consistency checking, and meter linkage — similar to `SetupOutputVariable()` in `OutputProcessor.cc`.

## References / Citations
- EnergyPlus `SetupOutputVariable()`: `vendors/EnergyPlus/src/EnergyPlus/OutputProcessor.cc:1433-1517` — explicit variable registration with meter attachment and frequency handling
- EnergyPlus `CheckReportVariable()`: `vendors/EnergyPlus/src/EnergyPlus/OutputProcessor.cc:284-345` — variable name/key matching with optional regex support
- EnergyPlus `AttachMeters()`: `vendors/EnergyPlus/src/EnergyPlus/OutputProcessor.cc:1433-1518` — automatic meter hierarchy construction (Facility → Zone → EndUse) per registered variable
- OCHRE column naming convention: `crates/hares-io/src/output/columns.rs:3-4`
- Telemetry key registry: `crates/hares-types/src/telemetry_keys.rs:1-231` (all 231 lines)
- Column schema builder: `crates/hares-io/src/output/columns.rs:60-315`
- Equipment-to-column mapping: `crates/hares-core/src/dwelling/mod.rs:167-227`
- Column population logic: `crates/hares-core/src/dwelling/mod.rs:2910-3149`
