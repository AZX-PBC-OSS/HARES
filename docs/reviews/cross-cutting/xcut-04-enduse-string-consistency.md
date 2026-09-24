# EndUse string consistency: audit for typos, inconsistent naming, missing end-uses, HPXML alignment
**Review ID**: xcut-04
**Category**: cross-cutting
**Date**: 2026-05-26

## Files Reviewed
- `crates/hares-types/src/equipment.rs` — EndUse type definition (lines 39–119)
- `crates/hares-types/src/ports.rs` — PortContribution / PortSlots (lines 57–91, 428+)
- `crates/hares-io/src/output/columns.rs` — Column schema construction (full file)
- `crates/hares-io/src/output/metrics.rs` — Metrics calculator & end-use aggregation (full file)
- `crates/hares-equipment/src/scheduled_load.rs` — Scheduled load templates (lines 688–814)
- All equipment `step()` / constructor files searched for `EndUse::` usage (see table below)

## Vendor/Reference Files Consulted
None

## Findings

### Finding 1: [Severity: high] Metrics end-use aggregation keys on equipment name, not EndUse enum value
**Description**: The `discover_end_use_columns` function (`crates/hares-io/src/output/metrics.rs:777–800`) discovers "end-use" columns by stripping `" Electric Power (kW)"` from column names. However, output columns are named after equipment instances (e.g., `"ASHP Heater Electric Power (kW)"`, `"Gas Furnace Electric Power (kW)"`, `"Baseboard Electric Power (kW)"` -- see `columns.rs:92`), **not** after `EndUse` string constants. This means `energy_by_end_use` and `peak_by_end_use` in metrics use **equipment instance names** as keys rather than end-use category names.

**Code Location**:
- Column naming: `crates/hares-io/src/output/columns.rs:92` — `format!("{name} {ELECTRIC_POWER_SUFFIX}")` uses `EquipmentSpec.name`
- End-use discovery: `crates/hares-io/src/output/metrics.rs:777–800`
- Aggregation: `crates/hares-io/src/output/metrics.rs:405–412` — keys initialized from discovered column names
- Accumulation: `crates/hares-io/src/output/metrics.rs:548–558` — `self.energy_by_end_use.get_mut(name)` where `name` is the column name prefix

**Root Cause**: The output schema in `columns.rs` builds per-equipment columns from `EquipmentSpec.name` (a descriptive human label like `"ASHP Heater"`, `"Battery"`). There are **no per-EndUse aggregate columns** generated. The metrics `discover_end_use_columns` function simply finds all columns suffixed with `" Electric Power (kW)"` and groups them by the column name prefix — which is the equipment name, not the end-use category.

**Impact**: Three HVAC_HEATING equipment (e.g., ASHP Heater, Gas Furnace, Baseboard) produce three separate energy totals in `per_end_use` even though they all serve `EndUse::HVAC_HEATING`. Reports disaggregate the same end-use into multiple buckets, making it impossible to answer "how much heating energy was used?" without post-hoc arithmetic. The standalone `hvac_heating_kw_idx` and `hvac_cooling_kw_idx` (lines 397–401) look for columns literally named `"HVAC Heating Electric Power (kW)"` and `"HVAC Cooling Electric Power (kW)"` -- but these columns **never exist** in the schema produced by `build_schema()`, so those indices are always `None` and `hvac_heating_electric_wh` is never populated.

### Finding 2: [Severity: high] Missing end-use categories relative to HPXML
**Description**: HPXML defines a comprehensive set of end-use categories for residential energy analysis (heating, cooling, water heating, lighting, appliances, electronics, cooking, laundry, dishwasher, pool pump, hot tub/pool heater, EV charging, etc.). Several HPXML end-uses have **no dedicated `EndUse` constant** in HARES and are collapsed into `OTHER`, `PLUG_LOADS`, or `VENTILATION`.

**Code Location**:
- `crates/hares-types/src/equipment.rs:44–68` — All 13 standard EndUse constants
- `crates/hares-equipment/src/scheduled_load.rs:688–814` — Scheduled load templates assignment:
  - Line 733: Pool Pump → `EndUse::OTHER`
  - Line 737: Pool Heater → `EndUse::OTHER`
  - Line 741: Spa Pump → `EndUse::OTHER`
  - Line 745: Spa Heater → `EndUse::OTHER`
  - Line 751: Gas Grill → `EndUse::OTHER` (cooking appliance)
  - Line 755: Gas Fireplace → `EndUse::OTHER`
  - Line 769: Ceiling Fan → `EndUse::VENTILATION`

**Root Cause**: The `EndUse` type has no constants for HPXML categories: `cooking`, `laundry` (or `clothes_washer`/`clothes_dryer`), `dishwasher`, `pool_pump`, `spa_pump`, `pool_heater`, `spa_heater`, `ceiling_fan`. Equipment that serves these end-uses must either use `EndUse::OTHER` (losing all specificity) or use `EndUse::custom()` (which bypasses `is_standard()` and any standard reporting aggregation).

**Impact**: When HPXML data containing cooking, laundry, dishwasher, pool pumps, or hot tub equipment is simulated, consumption is labeled `OTHER` and cannot be disaggregated by HPXML-compliant tools. The `EndUse::custom()` escape hatch permits custom naming but offers no guarantee of HPXML alignment, no validation, and no central registry of known custom end-uses in use.

### Finding 3: [Severity: medium] End-use naming convention uses snake_case, not HPXML convention
**Description**: All HARES `EndUse` string values use `snake_case` (`"hvac_heating"`, `"water_heating"`, `"plug_loads"`). HPXML and Building America use human-readable names (`"Heating"`, `"Water Heating"`, `"Plug Loads"`). OCHRE uses PascalCase / Title Case for end-use columns (`"HVAC Heating Electric Power (kW)"`).

**Code Location**: `crates/hares-types/src/equipment.rs:44–68`

**Root Cause**: The `EndUse` string values were chosen as internal identifiers (`snake_case`, single word or underscore_separated) rather than HPXML-standard display names. The serialized form is these raw strings, which would appear in JSON reports and Python API outputs as `"hvac_heating"` rather than `"Heating"` or `"HVAC Heating"`.

**Impact**: Interoperability with HPXML-based tools (BEopt, OpenStudio, URBANopt) requires a translation layer that maps `"hvac_heating"` → `"Heating"`. This mapping does not exist in the codebase. The `EndUse` type is serialized directly (via `Serialize/Deserialize`) with no display-name transformation.

### Finding 4: [Severity: medium] PortContribution has no EndUse field
**Description**: The `PortContribution` enum (`crates/hares-types/src/ports.rs:57–91`) has no field to carry end-use metadata. An individual electrical, thermal, or fuel contribution is impossible to trace back to the originating end-use category from the port layer alone.

**Code Location**: `crates/hares-types/src/ports.rs:57–91` — `PortContribution` definition. Six variants (`Thermal`, `Electrical`, `Fuel`, `Fluid`, `Custom`, `Humidity`) — none contain an `EndUse` or `end_use` field.

**Root Cause**: End-use classification is kept exclusively in `EquipmentDescriptor.end_use` and never flows into the port contribution pipeline. The `ThermalCategory` enum on `PortContribution::Thermal` provides a different axis (physical origin classification: HvacHeating, HvacCooling, InternalGain, JacketLoss, DuctLoss, HvacDehumidification) but cannot distinguish, e.g., `HVAC_HEATING` thermal from `WATER_HEATING` jacket losses.

**Impact**: Any code operating on `PortSlots` after accumulation (e.g., the electrical solver, thermal solver) has zero information about which end-use generated each contribution. This prevents per-end-use electrical balance checking, per-end-use thermal validation, or per-end-use cost allocation at the solver layer.

### Finding 5: [Severity: medium] IdealHvac dynamically swaps EndUse at runtime
**Description**: `IdealHvac` assigns `EndUse::HVAC_HEATING` at construction (`crates/hares-equipment/src/hvac/ideal_hvac.rs:143`) but mutates `self.descriptor.end_use` at runtime based on capacity sign (`ideal_hvac.rs:535–539`):
```rust
self.descriptor.end_use = if capacity_w >= 0.0 {
    EndUse::HVAC_HEATING
} else {
    EndUse::HVAC_COOLING
};
```

**Code Location**: `crates/hares-equipment/src/hvac/ideal_hvac.rs:535–539`

**Root Cause**: IdealHvac is a combined heating/cooling model that serves both end-uses within a single equipment instance. The runtime mutation is necessary for correct dispatch routing by `ByEndUse` target, but it creates a race condition for any concurrent reader of `descriptor.end_use` during the timestep.

**Impact**: If a control actor or observer reads `eq.descriptor().end_use` while IdealHvac is mid-update (before line 539 executes), the end-use may report `HVAC_HEATING` even though the step will produce cooling. `DispatchTarget::ByEndUse` routing in the same timestep would be wrong. The `is_critical` check in `dwelling/mod.rs:1109–1114` reads `descriptor.end_use` and could misclassify IdealHvac as critical (heating) when it is actually cooling.

### Finding 6: [Severity: low] No centralized registry of all valid EndUse values
**Description**: While the `EndUse` type provides `is_standard()` (line 101–117) to distinguish predefined end-uses from custom ones, there is no iteration method, no `all_standard()` function, no `from_str` / `TryFrom` integration, and no canonical documentation of the full list within the `EndUse` impl block. A developer adding a new equipment type must search the codebase to discover the available constants.

**Code Location**: `crates/hares-types/src/equipment.rs:39–119`

**Root Cause**: The `EndUse` type is a newtype over `Cow<'static, str>` with associated constants. Unlike a Rust `enum`, there is no exhaustive compile-time checking. The 13 standard constants are defined at lines 44–68 and repeated as string literals in `is_standard()` at lines 104–116, creating a DRY violation where the same strings appear in two locations that must be kept in sync.

**Impact**: Adding a new standard end-use (e.g., for cooking, laundry, pool pump) requires updates in three places: the constant definition (line xx), the `is_standard()` match (line 1xx), and any documentation. Forgetting any of these produces a silently incorrect `is_standard()` result. A developer unfamiliar with the pattern may add only the constant and introduce a bug where `is_standard()` returns `false` for that new constant.

### Finding 7: [Severity: low] No typo protection — any string is a valid EndUse
**Description**: `EndUse::custom("any_string")` and `"hvac_heating".into()` are always valid. If a developer types `EndUse::HVAC_HEATING` but introduces a typo in a custom string (e.g., `EndUse::custom("heeting")`), there is no compile-time or runtime validation. Dispatch routing and metrics aggregation silently create a new category.

**Code Location**: `crates/hares-types/src/equipment.rs:90–92` — `EndUse::custom()`
`crates/hares-types/src/equipment.rs:121–125` — `From<&'static str>` impl

**Root Cause**: The `EndUse` type is intentionally extensible, which is a valid design choice. However, there is no lint, test, or validation that could catch a near-miss typo of a standard value (e.g., `"hvac-heating"` instead of `"hvac_heating"`). The `is_standard()` check only detects standard constants, not "almost-standard" custom strings.

**Impact**: A typo like `"hvac-heating"` or `"hv ac_heating"` creates a distinct end-use category in metrics reports. With case-sensitive string matching in `BTreeMap`, this category appears as a separate line item, silently fragmenting energy totals.

### Finding 8: [Severity: low] Case-sensitive BTreeMap aggregation in metrics
**Description**: `energy_by_end_use` and `peak_by_end_use` are `BTreeMap<String, f64>` (line 312–313). Lookups are case-sensitive. If a column name `"ashp heater Electric Power (kW)"` differs in case from another column `"ASHP Heater Electric Power (kW)"`, they would produce separate aggregation keys.

**Code Location**: `crates/hares-io/src/output/metrics.rs:312–313`, `405–412`, `548–558`

**Root Cause**: The column name discovery path (`discover_end_use_columns`) stores names verbatim without normalization. Since column names are always constructed consistently by `columns.rs`, this is unlikely to trigger in practice, but represents a latent fragility.

**Impact**: Latent — cannot be triggered in current code because `build_schema` / `instance_qualified_names` produce consistently-cased names. However, if end-use names were ever case-mismatched (e.g., through a Python adapter), aggregation would silently split.

## EndUse Value Enumeration

### Standard EndUse Constants (defined in `crates/hares-types/src/equipment.rs:44–68`)

| Constant | String Value | HPXML Equivalent |
|---|---|---|
| `EndUse::HVAC_HEATING` | `"hvac_heating"` | Heating |
| `EndUse::HVAC_COOLING` | `"hvac_cooling"` | Cooling |
| `EndUse::WATER_HEATING` | `"water_heating"` | Water Heating |
| `EndUse::LIGHTING` | `"lighting"` | Lighting |
| `EndUse::PLUG_LOADS` | `"plug_loads"` | Appliances / Electronics (partial) |
| `EndUse::REFRIGERATION` | `"refrigeration"` | Appliances (subset) |
| `EndUse::VENTILATION` | `"ventilation"` | Ventilation |
| `EndUse::BATTERY` | `"battery"` | Battery Storage |
| `EndUse::PV` | `"pv"` | PV Generation |
| `EndUse::EV` | `"ev"` | EV Charging |
| `EndUse::GENERATOR` | `"generator"` | Generator |
| `EndUse::DEHUMIDIFIER` | `"dehumidifier"` | Dehumidifier |
| `EndUse::OTHER` | `"other"` | Other / Unknown |

### HPXML End-Uses Not Represented
- **Cooking** — Equipment like `Gas Grill` uses `OTHER` (`scheduled_load.rs:751`)
- **Laundry / Clothes Washer / Clothes Dryer** — No dedicated constant; CSV column names exist at `schedule_resolve.rs:53–58` but no `EndUse` constant
- **Dishwasher** — CSV column name exists (`schedule_resolve.rs:63`) but no `EndUse` constant
- **Pool Pump / Pool Heater / Spa Pump / Spa Heater** — All use `EndUse::OTHER` (`scheduled_load.rs:733–745`)
- **Ceiling Fan** — Uses `EndUse::VENTILATION` (`scheduled_load.rs:769`), which conflates ventilation and fan loads

### Custom EndUse Values in Use (Production Code)
No custom end-use values are used in production equipment constructors. Custom values appear only in test code:
- `"heat_pump_water_heater"` — test only
- `"ice_storage"` — test only
- `"vehicle_to_grid_charger"` — test only
- `"novel_equipment_type"` — test only
- `"my_custom_category"` — test only
- `"vehicle_to_grid"` — test only

### Typo Audit
No typos found in EndUse string literals across the codebase. All 13 standard string values (`"hvac_heating"`, `"hvac_cooling"`, `"water_heating"`, `"lighting"`, `"plug_loads"`, `"refrigeration"`, `"ventilation"`, `"battery"`, `"pv"`, `"ev"`, `"generator"`, `"dehumidifier"`, `"other"`) are consistently used.

### Equipment EndUse Assignment

| Equipment Type | EndUse | Source File:Line |
|---|---|---|
| ASHP Heater | `HVAC_HEATING` | `hvac/heat_pump/heater.rs:423` |
| ASHP Cooler | `HVAC_COOLING` | `hvac/heat_pump/cooler.rs:56,301` |
| Air Conditioner | `HVAC_COOLING` | `hvac/air_conditioner.rs:428` |
| Baseboard | `HVAC_HEATING` | `hvac/baseboard.rs:62` |
| Boiler | `HVAC_HEATING` | `hvac/boiler.rs:135,364` |
| Furnace | `HVAC_HEATING` | `hvac/furnace.rs:88,372` |
| Ideal HVAC | `HVAC_HEATING` → `HVAC_COOLING` (mutable) | `hvac/ideal_hvac.rs:143,535–539` |
| Dehumidifier | `DEHUMIDIFIER` | `hvac/dehumidifier.rs:100` |
| Resistance WH | `WATER_HEATING` | `water_heater/resistance.rs:144` |
| Heat Pump WH | `WATER_HEATING` | `water_heater/heat_pump_wh.rs:196` |
| Gas WH | `WATER_HEATING` | `water_heater/gas.rs:135` |
| Tankless WH | `WATER_HEATING` | `water_heater/tankless.rs:108` |
| Battery | `BATTERY` | `battery/mod.rs:383` |
| PV | `PV` | `pv/mod.rs:143` |
| EV | `EV` | `ev/mod.rs:129` |
| Generator | `GENERATOR` | `generator.rs:514` |
| Ventilation | `VENTILATION` | `ventilation.rs:203` |
| ScheduledLoad | varies by subtype | `scheduled_load.rs:688–814` |
| EventLoad | `OTHER` | `event_load.rs:228` |

## Summary
- Total findings: **8**
- Critical: 0 | High: **2** | Medium: **3** | Low: **3**

## Recommendations

1. **Add per-EndUse aggregate output columns** in `columns.rs` that sum all equipment contributions for each `EndUse` category (e.g., `"HVAC Heating Electric Power (kW)"` should aggregate all `HVAC_HEATING` equipment). This fixes Finding 1 and enables the `hvac_heating_kw_idx`/`hvac_cooling_kw_idx` lookups in metrics to work correctly.

2. **Add HPXML-aligned EndUse constants** for the missing categories: `COOKING`, `LAUNDRY` (or separate `CLOTHES_WASHER`/`CLOTHES_DRYER`), `DISHWASHER`, `POOL_PUMP`, `SPA_PUMP`, `POOL_HEATER`, `SPA_HEATER`, `CEILING_FAN`. Update `scheduled_load.rs` templates to use these new constants instead of `OTHER`/`VENTILATION`. This fixes Finding 2.

3. **Add HPXML display-name mapping** to `EndUse` (e.g., `fn hpxml_name(&self) -> &str`). Use the HPXML standard names rather than `snake_case` internal identifiers. This fixes Finding 3.

4. **Consider adding `end_use: EndUse` to `PortContribution::Electrical` and `PortContribution::Fuel`** for per-contribution end-use traceability. Alternatively, add an `end_use` field to `PortSlots` that records which end-use last contributed. This fixes Finding 4.

5. **Make IdealHvac end-use mutation observable-safe**: either use a `Cell<EndUse>` with atomic semantics, or split IdealHvac into separate heating and cooling equipment instances (matching the OCHRE model). This fixes Finding 5.

6. **Centralize EndUse validation**: add `EndUse::all_standard()` returning a static slice, `impl FromStr for EndUse`, and a compile-time check that `is_standard()` matches the constant list via a `const` assertion or `lazy_static` hash set. This fixes Findings 6 and 7.

7. **Normalize end-use keys** in `discover_end_use_columns` (e.g., `to_lowercase()`) to prevent case-sensitivity fragmentation. This fixes Finding 8.

## References / Citations
- HPXML v4.0 Data Dictionary: EndUse categories for residential buildings
- OCHRE (NREL): End-use naming conventions (`"{End Use} Electric Power (kW)"`)
- Building America: End-use category taxonomy
- `crates/hares-types/src/equipment.rs:39–119` — `EndUse` type definition
- `crates/hares-types/src/ports.rs:57–91` — `PortContribution` definition
- `crates/hares-io/src/output/metrics.rs:777–800` — `discover_end_use_columns`
- `crates/hares-io/src/output/columns.rs:60–315` — `build_schema` with per-equipment column naming
- `crates/hares-equipment/src/scheduled_load.rs:688–814` — Scheduled load template end-use assignments
- `crates/hares-equipment/src/hvac/ideal_hvac.rs:535–539` — Runtime EndUse mutation
