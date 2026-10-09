//! Output column definitions and verbosity-level schema construction.
//!
//! Column naming follows OCHRE conventions: `"{Name} {Metric} ({Unit})"`.
//! Multi-instance equipment includes an instance qualifier: `"Battery #1 SOC (-)"`.

use hares_types::{EndUse, FuelType, ZoneId};

/// Timestamp column included at every verbosity level.
const TIMESTAMP_COL: &str = "Time";

/// Verbosity 0: total power only.
const LEVEL_0_COLUMNS: &[&str] = &[
    "Total Electric Power (kW)",
    "Total Gas Power (therms/hour)",
    "Total Reactive Power (kVAR)",
];

/// Per-end-use column name suffixes for verbosity 1.
/// Each equipment contributes `"{name} Electric Power (kW)"` and/or
/// `"{name} Gas Power (therms/hour)"` depending on its fuel type.
pub const ELECTRIC_POWER_SUFFIX: &str = "Electric Power (kW)";
pub const GAS_POWER_SUFFIX: &str = "Gas Power (therms/hour)";

/// Whether equipment burning `fuel` reports a `"{name} Gas Power (therms/hour)"`
/// column (all combustion fuels; the OCHRE-style column name is historical).
///
/// Single source of truth for the fuel→column mapping: [`build_schema`] uses it
/// to emit the schema column and `hares-core`'s equipment column map uses it to
/// resolve (and populate) the same column. Keeping one predicate guarantees a
/// schema column is emitted if and only if it will be populated.
#[must_use]
pub fn fuel_reports_gas_power_column(fuel: FuelType) -> bool {
    match fuel {
        FuelType::Gas
        | FuelType::Propane
        | FuelType::Oil
        | FuelType::Wood
        | FuelType::Coal
        | FuelType::WoodPellet => true,
        FuelType::Electric | FuelType::None => false,
    }
}

/// Zone-level column templates for verbosity 2.
/// OCHRE convention: `"Temperature - {zone} (C)"` with zone name between dashes.
const ZONE_TEMP_PREFIX: &str = "Temperature -";
const ZONE_TEMP_UNIT: &str = "(C)";
const ZONE_UNMET_LOAD_COL: &str = "Unmet HVAC Load (C)";
const OUTDOOR_TEMP_COL: &str = "Outdoor Dry Bulb (C)";
const GROUND_TEMP_COL: &str = "Temperature - Ground (C)";
const NET_SENSIBLE_HEAT_GAIN_COL: &str = "Net Sensible Heat Gain - Indoor (W)";
const HOT_WATER_MAINS_TEMP_COL: &str = "Hot Water Mains Temperature (C)";

/// Equipment state column suffixes for verbosity 3.
pub const MODE_SUFFIX: &str = "Mode (-)";
pub const SETPOINT_SUFFIX: &str = "Setpoint (C)";
pub const SOC_SUFFIX: &str = "SOC (-)";

/// Energy column suffix for verbosity 4.
pub const ENERGY_SUFFIX: &str = "Energy (kWh)";

/// Reactive power column suffixes for verbosity 5.
pub const REACTIVE_POWER_SUFFIX: &str = "Reactive Power (kVAR)";
pub const POWER_FACTOR_SUFFIX: &str = "Power Factor (-)";

/// Defrost state column suffix for verbosity 7 (heat pump heaters only).
pub const DEFROST_STATE_SUFFIX: &str = "Defrost State (-)";

/// Duct losses column for verbosity 5.
pub const HVAC_DUCT_LOSSES_COL: &str = "HVAC Duct Losses (W)";

/// HVAC performance column suffixes for verbosity 7.
pub const CAPACITY_SUFFIX: &str = "Capacity (W)";
pub const COP_SUFFIX: &str = "COP (-)";
pub const SCHEDULE_SUFFIX: &str = "Schedule (-)";
pub const ER_POWER_SUFFIX: &str = "ER Power (kW)";
pub const SHR_SUFFIX: &str = "SHR (-)";
pub const SPEED_SUFFIX: &str = "Speed (-)";
pub const FAN_POWER_SUFFIX: &str = "Fan Power (kW)";
pub const MAIN_POWER_SUFFIX: &str = "Main Power (kW)";
pub const RUNTIME_FRACTION_SUFFIX: &str = "Runtime Fraction (-)";
pub const LATENT_GAINS_SUFFIX: &str = "Latent Gains (W)";

/// Setpoint chain dwelling-level columns for verbosity 7.
/// Emitted at dwelling level because setpoint schedules and runtime overrides
/// are zone-global state, not per-equipment.
pub const SCHEDULED_HEATING_SETPOINT_COL: &str = "Scheduled Heating Setpoint (C)";
pub const SCHEDULED_COOLING_SETPOINT_COL: &str = "Scheduled Cooling Setpoint (C)";
pub const RUNTIME_HEATING_SETPOINT_COL: &str = "Runtime Heating Setpoint (C)";
pub const RUNTIME_COOLING_SETPOINT_COL: &str = "Runtime Cooling Setpoint (C)";

// ── Verbosity 8: per-equipment telemetry diagnostic columns ─────────────────
// Each suffix is combined with the equipment instance-qualified name:
//   `"{equipment_name} {suffix}"`

/// Per-HVAC equipment temperature column suffixes for verbosity 8.
pub const SUPPLY_TEMP_SUFFIX: &str = "Supply Temperature (C)";
pub const SUPPLY_AIR_TEMP_SUFFIX: &str = "Supply Air Temperature (C)";
pub const RETURN_TEMP_SUFFIX: &str = "Return Temperature (C)";

/// Per-HVAC compressor power column suffixes for verbosity 8.
pub const COMPRESSOR_POWER_W_SUFFIX: &str = "Compressor Power (W)";
pub const COMPRESSOR_POWER_KW_SUFFIX: &str = "Compressor Power (kW)";
pub const FAN_ELECTRIC_POWER_SUFFIX: &str = "Fan Electric Power (W)";
pub const FAN_POWER_W_SUFFIX: &str = "Fan Power (W)";

/// Per-heat-pump detail column suffixes for verbosity 8.
pub const PAN_HEATER_POWER_SUFFIX: &str = "Pan Heater Power (kW)";
pub const HP_CAPACITY_SUFFIX: &str = "Heat Pump Capacity (W)";
pub const ER_CAPACITY_SUFFIX: &str = "ER Capacity (W)";

/// Per-PV diagnostic column suffixes for verbosity 8.
pub const PV_DC_POWER_SUFFIX: &str = "DC Power (kW)";
pub const PV_IRRADIANCE_SUFFIX: &str = "Irradiance (W/m2)";

/// Per-EV diagnostic column suffixes for verbosity 8.
pub const EV_CONNECTION_STATE_SUFFIX: &str = "Connection State (-)";
pub const EV_CHARGING_LEVEL_SUFFIX: &str = "Charging Level (-)";

/// Per-equipment thermostat short-cycle protection column suffixes for verbosity 8.
pub const MIN_ON_TIME_SUFFIX: &str = "Min On Time (s)";
pub const MIN_OFF_TIME_SUFFIX: &str = "Min Off Time (s)";

/// Maps an equipment spec name to its `EndUse` category based on the canonical
/// equipment naming conventions used by the HPXML resolver and registry.
///
/// The mapping mirrors the `end_use` assignments hardcoded in each equipment
/// constructor (furnace.rs, baseboard.rs, etc.) and the registry bindings in
/// `EquipmentRegistry::new()`. When an equipment name is not recognised, it
/// falls back to `EndUse::OTHER`.
pub fn equipment_name_to_end_use(name: &str) -> EndUse {
    match name {
        // HVAC Heating
        "Gas Furnace" | "Electric Furnace" | "Electric Baseboard" | "Gas Boiler"
        | "Electric Boiler" | "ASHP Heater" | "MSHP Heater" | "GSHP Heater" | "WSHP Heater"
        | "Heat Pump Heater" | "Ideal HVAC" => EndUse::HVAC_HEATING,

        // HVAC Cooling
        "Air Conditioner" | "Room AC" | "ASHP Cooler" | "MSHP Cooler" | "GSHP Cooler"
        | "WSHP Cooler" => EndUse::HVAC_COOLING,

        // Dehumidifier
        "Dehumidifier" => EndUse::DEHUMIDIFIER,

        // Water Heating
        "Gas Water Heater"
        | "Gas Tankless Water Heater"
        | "Electric Resistance Water Heater"
        | "Resistance Water Heater"
        | "Heat Pump Water Heater"
        | "Tankless Water Heater"
        | "Indirect Tank"
        | "Gas WH"
        | "Resistance WH"
        | "Heat Pump WH"
        | "Tankless WH"
        | "Indirect WH" => EndUse::WATER_HEATING,

        // Battery
        "Battery" => EndUse::BATTERY,

        // PV
        "PV" => EndUse::PV,

        // EV
        "EV" | "Electric Vehicle" | "Scheduled EV" => EndUse::EV,

        // Generator
        "Gas Generator" | "Gas Fuel Cell" => EndUse::GENERATOR,

        // Lighting: every LightingLoad name OCHRE's Equipment/__init__.py
        // registers under its "Lighting" end use, including "Exterior
        // Lighting", so the aggregate holds indoor plus exterior.
        "Lighting" | "Indoor Lighting" | "Outdoor Lighting" | "Garage Lighting"
        | "Exterior Lighting" => EndUse::LIGHTING,

        // Plug Loads
        "Plug Loads" | "MELs" => EndUse::PLUG_LOADS,

        // Refrigeration
        "Refrigerator" | "Freezer" => EndUse::REFRIGERATION,

        // Ventilation
        "Ventilation Fan" | "HRV" | "ERV" => EndUse::VENTILATION,

        // Ceiling Fan (separate from whole-house ventilation per HPXML 4.2)
        "Ceiling Fan" => EndUse::CEILING_FAN,

        // Cooking
        "Gas Grill" | "Cooking Range" => EndUse::COOKING,

        // Pool / Spa
        "Pool Pump" => EndUse::POOL_PUMP,
        "Pool Heater" => EndUse::POOL_HEATER,
        "Spa Pump" => EndUse::SPA_PUMP,
        "Spa Heater" => EndUse::SPA_HEATER,

        // Everything else (event loads, protocol bridge, etc.)
        _ => EndUse::OTHER,
    }
}

/// Returns the display name for an `EndUse` category, suitable for column
/// headings (e.g. `"HVAC Heating"` for `EndUse::HVAC_HEATING`).
///
/// Custom end-uses return their `as_str()` value unmodified.
pub fn end_use_display_name(end_use: &EndUse) -> &str {
    if *end_use == EndUse::HVAC_HEATING {
        return "HVAC Heating";
    }
    if *end_use == EndUse::HVAC_COOLING {
        return "HVAC Cooling";
    }
    if *end_use == EndUse::WATER_HEATING {
        return "Water Heating";
    }
    if *end_use == EndUse::LIGHTING {
        return "Lighting";
    }
    if *end_use == EndUse::PLUG_LOADS {
        return "Plug Loads";
    }
    if *end_use == EndUse::REFRIGERATION {
        return "Refrigeration";
    }
    if *end_use == EndUse::VENTILATION {
        return "Ventilation";
    }
    if *end_use == EndUse::BATTERY {
        return "Battery";
    }
    if *end_use == EndUse::PV {
        return "PV";
    }
    if *end_use == EndUse::EV {
        return "EV";
    }
    if *end_use == EndUse::GENERATOR {
        return "Generator";
    }
    if *end_use == EndUse::COOKING {
        return "Cooking";
    }
    if *end_use == EndUse::LAUNDRY {
        return "Laundry";
    }
    if *end_use == EndUse::DISHWASHER {
        return "Dishwasher";
    }
    if *end_use == EndUse::POOL_PUMP {
        return "Pool Pump";
    }
    if *end_use == EndUse::POOL_HEATER {
        return "Pool Heater";
    }
    if *end_use == EndUse::SPA_PUMP {
        return "Spa Pump";
    }
    if *end_use == EndUse::SPA_HEATER {
        return "Spa Heater";
    }
    if *end_use == EndUse::CEILING_FAN {
        return "Ceiling Fan";
    }
    if *end_use == EndUse::DEHUMIDIFIER {
        return "Dehumidifier";
    }
    if *end_use == EndUse::OTHER {
        return "Other";
    }
    end_use.as_str()
}

/// Reverse mapping: given a display name string from a column heading, returns
/// the corresponding `EndUse` key string if the display name is a recognised
/// standard end-use.
pub fn display_name_to_end_use_key(display_name: &str) -> Option<String> {
    match display_name {
        "HVAC Heating" => Some("hvac_heating".to_string()),
        "HVAC Cooling" => Some("hvac_cooling".to_string()),
        "Water Heating" => Some("water_heating".to_string()),
        "Lighting" => Some("lighting".to_string()),
        "Plug Loads" => Some("plug_loads".to_string()),
        "Refrigeration" => Some("refrigeration".to_string()),
        "Ventilation" => Some("ventilation".to_string()),
        "Battery" => Some("battery".to_string()),
        "PV" => Some("pv".to_string()),
        "EV" => Some("ev".to_string()),
        "Generator" => Some("generator".to_string()),
        "Cooking" => Some("cooking".to_string()),
        "Laundry" => Some("laundry".to_string()),
        "Dishwasher" => Some("dishwasher".to_string()),
        "Pool Pump" => Some("pool_pump".to_string()),
        "Pool Heater" => Some("pool_heater".to_string()),
        "Spa Pump" => Some("spa_pump".to_string()),
        "Spa Heater" => Some("spa_heater".to_string()),
        "Ceiling Fan" => Some("ceiling_fan".to_string()),
        "Dehumidifier" => Some("dehumidifier".to_string()),
        "Other" => Some("other".to_string()),
        _ => None,
    }
}

/// Suffix for end-use aggregate electric power column names.
/// Aggregate columns use the form `"{DisplayName} End Use Electric Power (kW)"`
/// to avoid collisions with per-equipment columns.
const END_USE_AGGREGATE_SUFFIX: &str = " End Use Electric Power (kW)";

/// Returns the output column name for an end-use aggregate electric power column.
///
/// The return value is always distinct from any per-equipment electric-power
/// column, avoiding collision-based panics in Arrow IPC readers (including
/// Polars) that deduplicate schemas by field name.
///
/// # Examples
/// - `end_use_electric_power_column(&EndUse::HVAC_HEATING)` → `"HVAC Heating End Use Electric Power (kW)"`
/// - `end_use_electric_power_column(&EndUse::CEILING_FAN)` → `"Ceiling Fan End Use Electric Power (kW)"`
pub fn end_use_electric_power_column(end_use: &EndUse) -> String {
    format!(
        "{} End Use Electric Power (kW)",
        end_use_display_name(end_use)
    )
}

/// If `name` is an end-use aggregate electric power column, returns the
/// `EndUse` variant it represents. Returns `None` for per-equipment columns,
/// totals, and other column types.
///
/// Recognised format: `"{DisplayName} End Use Electric Power (kW)"`.
pub fn parse_end_use_electric_power_column(name: &str) -> Option<EndUse> {
    let key = parse_end_use_electric_power_column_key(name)?;
    end_use_from_key(&key)
}

/// Like [`parse_end_use_electric_power_column`] but returns the canonical
/// EndUse key string (e.g. `"hvac_heating"`) directly, avoiding a round-trip
/// through display name and EndUse variant.
pub fn parse_end_use_electric_power_column_key(name: &str) -> Option<String> {
    let display = name.strip_suffix(END_USE_AGGREGATE_SUFFIX)?;
    display_name_to_end_use_key(display)
}

/// Maps an EndUse key string (e.g. `"hvac_heating"`) back to its EndUse variant.
fn end_use_from_key(key: &str) -> Option<EndUse> {
    match key {
        "hvac_heating" => Some(EndUse::HVAC_HEATING),
        "hvac_cooling" => Some(EndUse::HVAC_COOLING),
        "water_heating" => Some(EndUse::WATER_HEATING),
        "lighting" => Some(EndUse::LIGHTING),
        "plug_loads" => Some(EndUse::PLUG_LOADS),
        "refrigeration" => Some(EndUse::REFRIGERATION),
        "ventilation" => Some(EndUse::VENTILATION),
        "battery" => Some(EndUse::BATTERY),
        "pv" => Some(EndUse::PV),
        "ev" => Some(EndUse::EV),
        "generator" => Some(EndUse::GENERATOR),
        "cooking" => Some(EndUse::COOKING),
        "laundry" => Some(EndUse::LAUNDRY),
        "dishwasher" => Some(EndUse::DISHWASHER),
        "pool_pump" => Some(EndUse::POOL_PUMP),
        "pool_heater" => Some(EndUse::POOL_HEATER),
        "spa_pump" => Some(EndUse::SPA_PUMP),
        "spa_heater" => Some(EndUse::SPA_HEATER),
        "ceiling_fan" => Some(EndUse::CEILING_FAN),
        "dehumidifier" => Some(EndUse::DEHUMIDIFIER),
        "other" => Some(EndUse::OTHER),
        _ => None,
    }
}

/// Whether `name` is a **reserved** output column — one the schema emits
/// independently of any equipment instance, whose name nevertheless sits
/// verbatim in the per-equipment `{name} {suffix}` namespace.
///
/// The reserved occupants of that namespace are the level-0 aggregate
/// totals (an equipment named `"Total"` would produce the per-equipment
/// column `"Total Electric Power (kW)"` — the aggregate's own name) and the
/// per-end-use aggregates (an equipment named `"HVAC Heating End Use"`
/// would produce `"HVAC Heating End Use Electric Power (kW)"`). Both sets
/// are recognized through their existing single homes — the `LEVEL_0_COLUMNS`
/// const and [`parse_end_use_electric_power_column`] — so this predicate
/// cannot drift from the emission sites.
///
/// The contract it enforces, shared by `build_schema` and the dwelling's
/// equipment column map: an equipment whose per-equipment column name is
/// reserved gets **no** per-equipment output columns — the schema does not
/// emit them (a duplicate field name would collapse in the name-keyed
/// column index) and the column map claims none (a claim would make
/// `record_step`'s later per-equipment write silently replace the
/// aggregate). Its values are missing, never misattributed; its energy
/// still reaches the totals and end-use aggregates through the port/core
/// outputs those columns are computed from.
#[must_use]
pub fn is_reserved_output_column_name(name: &str) -> bool {
    LEVEL_0_COLUMNS.contains(&name) || parse_end_use_electric_power_column(name).is_some()
}

/// Returns column names that should be present at a given verbosity level
/// for use in tests. These are the equipment-independent columns only;
/// zone-specific columns (temperature, infiltration, LWR) are conditional
/// on `zone_names` and are included when the corresponding zone is present.
/// Per-equipment columns (power, mode, SOC, energy, schedule, etc.) are
/// generated dynamically by `build_schema` based on the equipment list and
/// are not covered here.
pub fn expected_columns_at_verbosity(
    verbosity: u8,
    zone_names: &[(ZoneId, String)],
) -> Vec<String> {
    let mut cols: Vec<String> = vec![TIMESTAMP_COL.to_string()];
    for &col in LEVEL_0_COLUMNS {
        cols.push(col.to_string());
    }
    cols.push(OUTDOOR_TEMP_COL.to_string());
    cols.push("Temperature - Indoor (C)".to_string());

    if verbosity >= 2 {
        // Per-zone temperature columns for non-Indoor zones.
        for (_zone_id, zone_name) in zone_names {
            if zone_name != "Indoor" {
                cols.push(format!("{ZONE_TEMP_PREFIX} {zone_name} {ZONE_TEMP_UNIT}"));
            }
        }
        cols.push(ZONE_UNMET_LOAD_COL.to_string());
        cols.push(GROUND_TEMP_COL.to_string());
    }

    if verbosity >= 5 {
        cols.push(NET_SENSIBLE_HEAT_GAIN_COL.to_string());
        cols.push(HVAC_DUCT_LOSSES_COL.to_string());
    }

    if verbosity >= 7 {
        cols.push(HOT_WATER_MAINS_TEMP_COL.to_string());
        cols.push(SCHEDULED_HEATING_SETPOINT_COL.to_string());
        cols.push(SCHEDULED_COOLING_SETPOINT_COL.to_string());
        cols.push(RUNTIME_HEATING_SETPOINT_COL.to_string());
        cols.push(RUNTIME_COOLING_SETPOINT_COL.to_string());
    }

    cols
}

/// Every `OperatingMode` variant must have a corresponding entry in the
/// catalogue's mode labels so that Parquet output files remain
/// self-describing. The catalogue's schema metadata derives the ordinal map
/// from the labels' order; this test pins the order against the enum
/// itself.
#[cfg(test)]
mod mode_ordinals_tests {
    use hares_types::OperatingMode;
    use strum::IntoEnumIterator;

    #[test]
    fn mode_labels_cover_all_variants_in_ordinal_order() {
        let labels = hares_types::MODE_LABELS;
        assert_eq!(
            labels.len(),
            OperatingMode::iter().count(),
            "MODE_LABELS entry count must match OperatingMode variant count"
        );
        for mode in OperatingMode::iter() {
            let ordinal = mode as u8;
            let label = labels.get(ordinal as usize).unwrap_or_else(|| {
                panic!("MODE_LABELS is missing entry for OperatingMode variant {mode:?}")
            });
            assert_eq!(
                *label,
                format!("{mode:?}"),
                "MODE_LABELS[{ordinal}] must be the variant's name"
            );
        }
    }
}

pub fn is_hvac_or_wh(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("heater")
        || lower.contains("cooler")
        || lower.contains("furnace")
        || lower.contains("boiler")
        || lower.contains("baseboard")
        || lower.contains("heat pump")
        || lower.contains("air conditioner")
        || lower.contains("room ac")
        || lower.contains("ashp")
        || lower.contains("mshp")
        || lower.contains("water heat")
        || lower.contains("dehumidifier")
}

pub fn has_soc(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("battery")
        || lower.contains("electric vehicle")
        || lower == "ev"
        || lower.starts_with("ev ")
        || lower.starts_with("ev#")
}

pub fn is_heat_pump_heater(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("ashp heater")
        || lower.contains("mshp heater")
        || lower.contains("heat pump heater")
}

/// Returns true if the equipment name indicates cooling-only equipment
/// that should emit SHR and Latent Gains columns at v7.
/// Heat-pump heaters that also cool are excluded here — the cooler
/// companion emits those columns under its own name.
pub fn is_cooling_equipment(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("air conditioner")
        || lower.contains("room ac")
        || lower.contains("cooler")
        || lower.contains("dehumidifier")
}

/// Returns true if the equipment name indicates PV/solar equipment.
pub fn is_pv(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("pv")
}

/// Returns true if the equipment name indicates EV/vehicle equipment.
pub fn is_ev(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("ev") || lower.contains("electric vehicle")
}

/// Returns true if the equipment name indicates equipment with a compressor
/// (heat pumps, air conditioners) that should emit compressor power columns at v8.
pub fn is_compressor_equipment(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("heat pump")
        || lower.contains("ashp")
        || lower.contains("mshp")
        || lower.contains("air conditioner")
        || lower.contains("room ac")
        || lower.contains("cooler")
}

#[cfg(test)]
mod tests {
    use hares_types::FuelType;
    use hares_types::OperatingMode;
    use serde_json::Map;
    use strum::IntoEnumIterator;

    use super::*;
    use crate::hpxml::EquipmentSpec;
    use crate::output::catalogue::build_schema_for_specs;

    fn make_spec(name: &str, fuel: FuelType) -> EquipmentSpec {
        EquipmentSpec {
            instance_name: None,
            name: name.to_string(),
            fuel_type: fuel,
            parameters: Map::new(),
            zip_params: None,
            typed_overrides: serde_json::Map::new(),
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        }
    }

    #[test]
    fn verbosity_0_has_timestamp_total_power_and_context_columns() {
        let schema = build_schema_for_specs(&[], 0, &[]);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(
            names,
            vec![
                "Time",
                "Total Electric Power (kW)",
                "Total Gas Power (therms/hour)",
                "Total Reactive Power (kVAR)",
                "Outdoor Dry Bulb (C)",
                "Temperature - Indoor (C)",
            ]
        );
    }

    #[test]
    fn verbosity_0_column_names_match_ochre() {
        let expected = expected_columns_at_verbosity(0, &[]);
        assert!(expected.contains(&"Total Electric Power (kW)".to_string()));
        assert!(expected.contains(&"Total Gas Power (therms/hour)".to_string()));
    }

    #[test]
    fn verbosity_1_adds_per_equipment_power() {
        let specs = vec![
            make_spec("ASHP Heater", FuelType::Electric),
            make_spec("Gas Furnace", FuelType::Gas),
        ];
        let schema = build_schema_for_specs(&specs, 1, &[]);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(names.contains(&"ASHP Heater Electric Power (kW)"));
        assert!(names.contains(&"Gas Furnace Electric Power (kW)"));
        assert!(names.contains(&"Gas Furnace Gas Power (therms/hour)"));
    }

    #[test]
    fn verbosity_1_creates_gas_power_column_for_combustion_fuels() {
        // Wood, coal, and wood pellet boilers are mapped to "Gas Boiler" by
        // canonical_hvac_heating_name. The output schema must create the gas
        // power column for these fuels, not just natural gas/propane/oil.
        for fuel in [FuelType::Wood, FuelType::Coal, FuelType::WoodPellet] {
            let specs = vec![make_spec("Gas Boiler", fuel)];
            let schema = build_schema_for_specs(&specs, 1, &[]);
            let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
            assert!(
                names.contains(&"Gas Boiler Gas Power (therms/hour)"),
                "Gas Boiler with fuel {:?} must have a gas power column",
                fuel
            );
        }
    }

    #[test]
    fn multi_instance_gets_numbered_names() {
        let specs = vec![
            make_spec("Battery", FuelType::Electric),
            make_spec("Battery", FuelType::Electric),
        ];
        let schema = build_schema_for_specs(&specs, 1, &[]);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(names.contains(&"Battery #1 Electric Power (kW)"));
        assert!(names.contains(&"Battery #2 Electric Power (kW)"));
    }

    #[test]
    fn verbosity_2_adds_attic_temp_when_attic_zone_present() {
        let zone_names = vec![
            (ZoneId(1), "Indoor".to_string()),
            (ZoneId(2), "Attic".to_string()),
        ];
        let schema = build_schema_for_specs(&[], 2, &zone_names);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(names.contains(&"Temperature - Attic (C)"));
        assert!(names.contains(&"Unmet HVAC Load (C)"));
        assert!(names.contains(&"Temperature - Ground (C)"));
    }

    #[test]
    fn verbosity_2_skips_attic_temp_when_no_attic_zone() {
        let schema = build_schema_for_specs(&[], 2, &[]);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(!names.contains(&"Temperature - Attic (C)"));
        assert!(names.contains(&"Unmet HVAC Load (C)"));
        assert!(names.contains(&"Temperature - Ground (C)"));
    }

    #[test]
    fn verbosity_5_adds_net_sensible_heat_gain_column() {
        let schema = build_schema_for_specs(&[], 5, &[]);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(names.contains(&"Net Sensible Heat Gain - Indoor (W)"));
    }

    #[test]
    fn verbosity_7_adds_hot_water_mains_temperature_column() {
        let schema = build_schema_for_specs(&[], 7, &[]);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(names.contains(&"Hot Water Mains Temperature (C)"));
        // Setpoint chain columns are dwelling-level, independent of equipment list.
        assert!(names.contains(&"Scheduled Heating Setpoint (C)"));
        assert!(names.contains(&"Scheduled Cooling Setpoint (C)"));
        assert!(names.contains(&"Runtime Heating Setpoint (C)"));
        assert!(names.contains(&"Runtime Cooling Setpoint (C)"));
    }

    #[test]
    fn all_verbosity_levels_include_outdoor_temp_and_indoor_temp() {
        for v in 0..=8u8 {
            let schema = build_schema_for_specs(&[], v, &[]);
            let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
            assert!(
                names.contains(&"Outdoor Dry Bulb (C)"),
                "verbosity {v} must include 'Outdoor Dry Bulb (C)'"
            );
            assert!(
                names.contains(&"Temperature - Indoor (C)"),
                "verbosity {v} must include 'Temperature - Indoor (C)'"
            );
        }
    }

    #[test]
    fn verbosity_3_adds_mode_and_soc() {
        let specs = vec![
            make_spec("ASHP Heater", FuelType::Electric),
            make_spec("Battery", FuelType::Electric),
        ];
        let schema = build_schema_for_specs(&specs, 3, &[]);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(names.contains(&"ASHP Heater Mode (-)"));
        assert!(names.contains(&"ASHP Heater Setpoint (C)"));
        assert!(names.contains(&"Battery Mode (-)"));
        assert!(names.contains(&"Battery SOC (-)"));
    }

    #[test]
    fn schema_metadata_includes_mode_map() {
        let schema = build_schema_for_specs(&[], 0, &[]);
        let meta = schema.metadata();
        assert!(meta.contains_key("hares_mode_map"));
        let json: serde_json::Value =
            serde_json::from_str(meta.get("hares_mode_map").unwrap()).unwrap();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), OperatingMode::iter().count());
    }

    #[test]
    fn schema_metadata_includes_basement_lighting_created() {
        let specs = vec![
            make_spec("Indoor Lighting", FuelType::Electric),
            make_spec("Basement Lighting", FuelType::Electric),
        ];
        let schema = build_schema_for_specs(&specs, 1, &[]);
        let meta = schema.metadata();
        assert_eq!(
            meta.get("basement_lighting_created").map(|s| s.as_str()),
            Some("true"),
        );

        let schema_no_basement = build_schema_for_specs(&[], 0, &[]);
        let meta_no = schema_no_basement.metadata();
        assert_eq!(
            meta_no.get("basement_lighting_created").map(|s| s.as_str()),
            Some("false"),
        );
    }

    #[test]
    fn schema_metadata_includes_garage_lighting_created() {
        let specs = vec![
            make_spec("Indoor Lighting", FuelType::Electric),
            make_spec("Garage Lighting", FuelType::Electric),
        ];
        let schema = build_schema_for_specs(&specs, 1, &[]);
        let meta = schema.metadata();
        assert_eq!(
            meta.get("garage_lighting_created").map(|s| s.as_str()),
            Some("true"),
        );

        let schema_no_garage = build_schema_for_specs(&[], 0, &[]);
        let meta_no = schema_no_garage.metadata();
        assert_eq!(
            meta_no.get("garage_lighting_created").map(|s| s.as_str()),
            Some("false"),
        );
    }

    #[test]
    fn mode_ordinal_round_trips() {
        assert_eq!(OperatingMode::Off.as_code(), 0.0);
        assert_eq!(OperatingMode::Heating.as_code(), 1.0);
        assert_eq!(OperatingMode::BackupElement.as_code(), 11.0);
        assert_eq!(OperatingMode::On.as_code(), 12.0);
    }

    /// The catalogue's mode labels carry the ordinal map: one label per
    /// `OperatingMode` variant, in ordinal order, and `OperatingMode::On`
    /// round-trips through `as_code()` at the last ordinal.
    #[test]
    fn test_mode_ordinals_covers_all_variants() {
        let mode_labels = hares_types::MODE_LABELS;
        assert_eq!(
            mode_labels.len(),
            OperatingMode::iter().count(),
            "MODE_LABELS entry count must match OperatingMode variant count"
        );
        for mode in OperatingMode::iter() {
            let ordinal = mode as u8;
            let name = format!("{mode:?}");
            assert_eq!(
                mode_labels.get(ordinal as usize).copied(),
                Some(name.as_str()),
                "MODE_LABELS missing or wrong at {name} (ordinal {ordinal})"
            );
        }
    }

    /// `OperatingMode::On` round-trips through `as_code()` and the mode
    /// labels' last entry is `"On"`.
    #[test]
    fn test_on_mode_roundtrips() {
        assert_eq!(OperatingMode::On.as_code(), 12.0);
        let mode_labels = hares_types::MODE_LABELS;
        assert_eq!(
            mode_labels.last(),
            Some(&"On"),
            "the mode labels' last entry must be 'On' at ordinal 12"
        );
    }

    #[test]
    fn expected_columns_at_verbosity_0_is_subset_of_schema() {
        let schema = build_schema_for_specs(&[], 0, &[]);
        let schema_names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        for col in expected_columns_at_verbosity(0, &[]) {
            assert!(
                schema_names.contains(&col.as_str()),
                "expected column '{col}' not in schema"
            );
        }
    }

    // ── Verbosity 7 HVAC output columns ────────────────────────────────────

    /// Cooling equipment gets SHR and Latent Gains at v7; heating-only does not.
    /// OCHRE HVAC.py:595-596.
    #[test]
    fn cooling_equipment_gets_shr_and_latent_gains_at_v7_heating_does_not() {
        let cool =
            build_schema_for_specs(&[make_spec("Air Conditioner", FuelType::Electric)], 7, &[]);
        let heat = build_schema_for_specs(&[make_spec("Gas Furnace", FuelType::Gas)], 7, &[]);
        let cool_names: Vec<&str> = cool.fields().iter().map(|f| f.name().as_str()).collect();
        let heat_names: Vec<&str> = heat.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(cool_names.contains(&"Air Conditioner SHR (-)"));
        assert!(cool_names.contains(&"Air Conditioner Latent Gains (W)"));
        assert!(!heat_names.contains(&"Gas Furnace SHR (-)"));
        assert!(!heat_names.contains(&"Gas Furnace Latent Gains (W)"));
    }

    /// HVAC equipment gets Speed, Fan Power, Main Power, Runtime Fraction,
    /// Capacity, and COP at v7. Non-HVAC equipment gets none of these.
    /// OCHRE HVAC.py:592-599.
    #[test]
    fn hvac_equipment_gets_performance_columns_non_hvac_does_not() {
        let hvac = build_schema_for_specs(&[make_spec("Gas Furnace", FuelType::Gas)], 7, &[]);
        let non = build_schema_for_specs(&[make_spec("Battery", FuelType::Electric)], 7, &[]);
        let hvac_names: Vec<&str> = hvac.fields().iter().map(|f| f.name().as_str()).collect();
        let non_names: Vec<&str> = non.fields().iter().map(|f| f.name().as_str()).collect();
        let hvac_only = [
            "Gas Furnace Speed (-)",
            "Gas Furnace Fan Power (kW)",
            "Gas Furnace Main Power (kW)",
            "Gas Furnace Runtime Fraction (-)",
            "Gas Furnace Capacity (W)",
            "Gas Furnace COP (-)",
        ];
        for col in &hvac_only {
            assert!(hvac_names.contains(col), "HVAC schema missing '{col}'");
            assert!(
                !non_names.contains(col),
                "non-HVAC schema must not include '{col}'"
            );
        }
    }

    /// Capacity and COP promoted from v8 to v7; not duplicated at v8.
    /// v8 adds additional per-equipment telemetry columns (equipment temps,
    /// compressor power, etc.) so the schema length increases beyond v7.
    #[test]
    fn capacity_and_cop_promoted_to_v7_not_duplicated_at_v8() {
        let specs = vec![make_spec("Air Conditioner", FuelType::Electric)];
        let s7 = build_schema_for_specs(&specs, 7, &[]);
        let s8 = build_schema_for_specs(&specs, 8, &[]);
        let n7: Vec<&str> = s7.fields().iter().map(|f| f.name().as_str()).collect();
        let n8: Vec<&str> = s8.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(n7.contains(&"Air Conditioner Capacity (W)"));
        assert!(n7.contains(&"Air Conditioner COP (-)"));
        // Capacity and COP are at v7 and must also be at v8 (not duplicated, but still present).
        assert!(n8.contains(&"Air Conditioner Capacity (W)"));
        assert!(n8.contains(&"Air Conditioner COP (-)"));
        // v8 adds telemetry diagnostic columns beyond v7.
        assert!(s8.fields().len() > s7.fields().len());
    }

    /// HVAC Duct Losses column present at v5 (OCHRE HVAC.py:588).
    #[test]
    fn hvac_duct_losses_column_present_at_verbosity_5() {
        let schema = build_schema_for_specs(&[], 5, &[]);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(
            names.contains(&"HVAC Duct Losses (W)"),
            "verbosity 5 must include 'HVAC Duct Losses (W)' (OCHRE HVAC.py:588); got: {names:?}"
        );
    }

    // ── Conditional zone column tests ────────────────────────────────────

    /// Attic-specific columns are present only when an "Attic" zone exists.
    /// At v2: `Temperature - Attic (C)`. At v6: infiltration, LWR, HVAC attribution.
    #[test]
    fn test_attic_columns_conditional() {
        // Without attic zone: no attic-specific columns.
        let no_attic = vec![(ZoneId(1), "Indoor".to_string())];
        let s2 = build_schema_for_specs(&[], 2, &no_attic);
        let s6 = build_schema_for_specs(&[], 6, &no_attic);
        let n2: Vec<&str> = s2.fields().iter().map(|f| f.name().as_str()).collect();
        let n6: Vec<&str> = s6.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(
            !n2.contains(&"Temperature - Attic (C)"),
            "without attic zone, 'Temperature - Attic (C)' must be absent"
        );
        assert!(
            !n6.contains(&"Infiltration Heat Gain - Attic (W)"),
            "without attic zone, 'Infiltration Heat Gain - Attic (W)' must be absent"
        );
        assert!(
            !n6.contains(&"Interior LWR Exchange - Attic (W)"),
            "without attic zone, 'Interior LWR Exchange - Attic (W)' must be absent"
        );

        // With attic zone: attic-specific columns present.
        let with_attic = vec![
            (ZoneId(1), "Indoor".to_string()),
            (ZoneId(2), "Attic".to_string()),
        ];
        let s2 = build_schema_for_specs(&[], 2, &with_attic);
        let s6 = build_schema_for_specs(&[], 6, &with_attic);
        let n2: Vec<&str> = s2.fields().iter().map(|f| f.name().as_str()).collect();
        let n6: Vec<&str> = s6.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(
            n2.contains(&"Temperature - Attic (C)"),
            "with attic zone, 'Temperature - Attic (C)' must be present"
        );
        assert!(
            n6.contains(&"Infiltration Heat Gain - Attic (W)"),
            "with attic zone, 'Infiltration Heat Gain - Attic (W)' must be present"
        );
        assert!(
            n6.contains(&"Interior LWR Exchange - Attic (W)"),
            "with attic zone, 'Interior LWR Exchange - Attic (W)' must be present"
        );
    }

    /// Non-Indoor zone columns are conditional on zone presence, covering
    /// ground/foundation/basement zones whose display name is not "Attic".
    #[test]
    fn test_ground_columns_conditional() {
        // Without ground/basement zone: no per-zone columns for that zone.
        let no_basement = vec![(ZoneId(1), "Indoor".to_string())];
        let s2 = build_schema_for_specs(&[], 2, &no_basement);
        let s6 = build_schema_for_specs(&[], 6, &no_basement);
        let n2: Vec<&str> = s2.fields().iter().map(|f| f.name().as_str()).collect();
        let n6: Vec<&str> = s6.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(
            !n2.contains(&"Temperature - Zone_2 (C)"),
            "without basement zone, 'Temperature - Zone_2 (C)' must be absent; got: {n2:?}"
        );
        assert!(
            !n6.contains(&"Infiltration Heat Gain - Zone_2 (W)"),
            "without basement zone, 'Infiltration Heat Gain - Zone_2 (W)' must be absent"
        );
        // Temperature - Ground (C) is always present (weather data, not zone-conditional).
        assert!(
            n2.contains(&"Temperature - Ground (C)"),
            "'Temperature - Ground (C)' must always be present (weather data)"
        );

        // With basement zone: per-zone columns for that zone are present.
        let with_basement = vec![
            (ZoneId(1), "Indoor".to_string()),
            (ZoneId(2), "Zone_2".to_string()),
        ];
        let s2 = build_schema_for_specs(&[], 2, &with_basement);
        let s6 = build_schema_for_specs(&[], 6, &with_basement);
        let n2: Vec<&str> = s2.fields().iter().map(|f| f.name().as_str()).collect();
        let n6: Vec<&str> = s6.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(
            n2.contains(&"Temperature - Zone_2 (C)"),
            "with basement zone, 'Temperature - Zone_2 (C)' must be present; got: {n2:?}"
        );
        assert!(
            n6.contains(&"Infiltration Heat Gain - Zone_2 (W)"),
            "with basement zone, 'Infiltration Heat Gain - Zone_2 (W)' must be present; got: {n6:?}"
        );
        assert!(
            n6.contains(&"Interior LWR Exchange - Zone_2 (W)"),
            "with basement zone, 'Interior LWR Exchange - Zone_2 (W)' must be present; got: {n6:?}"
        );
    }

    /// Regression: single-zone building (indoor-only) produces no non-Indoor
    /// zone columns at any verbosity level.
    #[test]
    fn indoor_only_building_produces_no_non_indoor_zone_columns() {
        let zone_names = vec![(ZoneId(1), "Indoor".to_string())];
        for v in 2..=8u8 {
            let schema = build_schema_for_specs(&[], v, &zone_names);
            let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
            assert!(
                !names.contains(&"Temperature - Attic (C)"),
                "verbosity {v}: 'Temperature - Attic (C)' must be absent for indoor-only building"
            );
            assert!(
                !names.iter().any(|n| n.starts_with("Temperature - Zone_")),
                "verbosity {v}: no 'Temperature - Zone_N' must exist for indoor-only; got: {names:?}"
            );
            if v >= 6 {
                assert!(
                    !names.contains(&"Infiltration Heat Gain - Attic (W)"),
                    "verbosity {v}: attic infiltration must be absent for indoor-only"
                );
                assert!(
                    !names.contains(&"Interior LWR Exchange - Attic (W)"),
                    "verbosity {v}: attic LWR must be absent for indoor-only"
                );
                assert!(
                    !names
                        .iter()
                        .any(|n| n.starts_with("Infiltration Heat Gain - Zone_")),
                    "verbosity {v}: no 'Infiltration Heat Gain - Zone_N' for indoor-only; got: {names:?}"
                );
                assert!(
                    !names
                        .iter()
                        .any(|n| n.starts_with("Interior LWR Exchange - Zone_")),
                    "verbosity {v}: no 'Interior LWR Exchange - Zone_N' for indoor-only; got: {names:?}"
                );
            }
        }
    }

    // ── Per-zone HVAC attribution column tests ────────────────────────────

    /// At verbosity 6, per-zone HVAC heating columns exist for each zone name supplied.
    #[test]
    fn verbosity_6_has_per_zone_hvac_heating_columns() {
        let zone_names = vec![
            (ZoneId(1), "Indoor".to_string()),
            (ZoneId(2), "Basement".to_string()),
            (ZoneId(3), "Attic".to_string()),
        ];
        let schema = build_schema_for_specs(&[], 6, &zone_names);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(
            names.contains(&"HVAC Heating Delivered - Indoor (W)"),
            "verbosity 6 must include per-zone 'HVAC Heating Delivered - Indoor (W)'; got: {names:?}"
        );
        assert!(
            names.contains(&"HVAC Heating Delivered - Basement (W)"),
            "verbosity 6 must include per-zone 'HVAC Heating Delivered - Basement (W)'; got: {names:?}"
        );
        assert!(
            names.contains(&"HVAC Heating Delivered - Attic (W)"),
            "verbosity 6 must include per-zone 'HVAC Heating Delivered - Attic (W)'; got: {names:?}"
        );
    }

    /// At verbosity 6, per-zone HVAC cooling columns exist for each zone.
    #[test]
    fn verbosity_6_has_per_zone_hvac_cooling_columns() {
        let zone_names = vec![
            (ZoneId(1), "Indoor".to_string()),
            (ZoneId(2), "Basement".to_string()),
        ];
        let schema = build_schema_for_specs(&[], 6, &zone_names);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(
            names.contains(&"HVAC Cooling Delivered - Indoor (W)"),
            "verbosity 6 must include per-zone 'HVAC Cooling Delivered - Indoor (W)'; got: {names:?}"
        );
        assert!(
            names.contains(&"HVAC Cooling Delivered - Basement (W)"),
            "verbosity 6 must include per-zone 'HVAC Cooling Delivered - Basement (W)'; got: {names:?}"
        );
    }

    /// Verbosity 5 must NOT include per-zone attribution columns (those live at v6).
    #[test]
    fn per_zone_hvac_columns_not_present_below_verbosity_6() {
        let zone_names = vec![(ZoneId(1), "Indoor".to_string())];
        for v in 0..=5u8 {
            let schema = build_schema_for_specs(&[], v, &zone_names);
            let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
            assert!(
                !names
                    .iter()
                    .any(|n| n.starts_with("HVAC Heating Delivered - ")),
                "verbosity {v} must NOT include per-zone HVAC heating columns; got: {names:?}"
            );
            assert!(
                !names
                    .iter()
                    .any(|n| n.starts_with("HVAC Cooling Delivered - ")),
                "verbosity {v} must NOT include per-zone HVAC cooling columns; got: {names:?}"
            );
        }
    }

    /// Single-zone buildings produce exactly 2 per-zone columns (heating + cooling).
    #[test]
    fn single_zone_produces_one_pair_of_per_zone_hvac_columns() {
        let zone_names = vec![(ZoneId(1), "Indoor".to_string())];
        let schema = build_schema_for_specs(&[], 6, &zone_names);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(names.contains(&"HVAC Heating Delivered - Indoor (W)"));
        assert!(names.contains(&"HVAC Cooling Delivered - Indoor (W)"));
        // No stray zone columns for zones that don't exist.
        assert!(!names.contains(&"HVAC Heating Delivered - Attic (W)"));
    }

    // ── End‑use aggregate column tests ──────────────────────────────────

    /// Multiple HVAC_HEATING equipment (ASHP Heater + Gas Furnace) produce a
    /// single `"HVAC Heating End Use Electric Power (kW)"` aggregate column.
    #[test]
    fn multiple_hvac_heating_equipment_produce_single_aggregate_column() {
        let specs = vec![
            make_spec("ASHP Heater", FuelType::Electric),
            make_spec("Gas Furnace", FuelType::Gas),
            make_spec("Electric Baseboard", FuelType::Electric),
        ];
        let schema = build_schema_for_specs(&specs, 1, &[]);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        // Per-equipment columns still present.
        assert!(names.contains(&"ASHP Heater Electric Power (kW)"));
        assert!(names.contains(&"Gas Furnace Electric Power (kW)"));
        assert!(names.contains(&"Electric Baseboard Electric Power (kW)"));
        // Single aggregate column for all HVAC_HEATING equipment.
        assert!(
            names.contains(&"HVAC Heating End Use Electric Power (kW)"),
            "schema must contain 'HVAC Heating End Use Electric Power (kW)' aggregate column"
        );
        // No separate aggregate columns for individual equipment names.
        assert!(!names.contains(&"ASHP Heater HVAC Heating Electric Power (kW)"));
        assert!(!names.contains(&"Gas Furnace HVAC Heating Electric Power (kW)"));
    }

    /// Aggregate columns are only added at verbosity >= 1.
    #[test]
    fn aggregate_columns_only_at_verbosity_1_and_above() {
        let specs = vec![make_spec("ASHP Heater", FuelType::Electric)];
        let s0 = build_schema_for_specs(&specs, 0, &[]);
        let s1 = build_schema_for_specs(&specs, 1, &[]);
        let n0: Vec<&str> = s0.fields().iter().map(|f| f.name().as_str()).collect();
        let n1: Vec<&str> = s1.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(!n0.contains(&"HVAC Heating End Use Electric Power (kW)"));
        assert!(n1.contains(&"HVAC Heating End Use Electric Power (kW)"));
    }

    /// Mixed end-use equipment produce one aggregate column per EndUse category.
    #[test]
    fn mixed_end_use_equipment_produce_one_aggregate_per_category() {
        let specs = vec![
            make_spec("ASHP Heater", FuelType::Electric),
            make_spec("Gas Furnace", FuelType::Gas),
            make_spec("Air Conditioner", FuelType::Electric),
            make_spec("Battery", FuelType::Electric),
            make_spec("PV", FuelType::Electric),
        ];
        let schema = build_schema_for_specs(&specs, 1, &[]);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(names.contains(&"HVAC Heating End Use Electric Power (kW)"));
        assert!(names.contains(&"HVAC Cooling End Use Electric Power (kW)"));
        assert!(names.contains(&"Battery End Use Electric Power (kW)"));
        assert!(names.contains(&"PV End Use Electric Power (kW)"));
    }

    /// `equipment_name_to_end_use` maps canonical equipment names to the
    /// correct EndUse category.
    #[test]
    fn equipment_name_to_end_use_maps_correctly() {
        use hares_types::EndUse;
        assert_eq!(
            equipment_name_to_end_use("ASHP Heater"),
            EndUse::HVAC_HEATING
        );
        assert_eq!(
            equipment_name_to_end_use("Gas Furnace"),
            EndUse::HVAC_HEATING
        );
        assert_eq!(
            equipment_name_to_end_use("Air Conditioner"),
            EndUse::HVAC_COOLING
        );
        // Every LightingLoad name OCHRE's Equipment/__init__.py registers maps
        // to LIGHTING, so the aggregate holds indoor plus exterior.
        assert_eq!(equipment_name_to_end_use("Lighting"), EndUse::LIGHTING);
        assert_eq!(
            equipment_name_to_end_use("Indoor Lighting"),
            EndUse::LIGHTING
        );
        assert_eq!(
            equipment_name_to_end_use("Exterior Lighting"),
            EndUse::LIGHTING
        );
        assert_eq!(
            equipment_name_to_end_use("Outdoor Lighting"),
            EndUse::LIGHTING
        );
        assert_eq!(
            equipment_name_to_end_use("Garage Lighting"),
            EndUse::LIGHTING
        );
        assert_eq!(
            equipment_name_to_end_use("Dehumidifier"),
            EndUse::DEHUMIDIFIER
        );
        assert_eq!(equipment_name_to_end_use("Battery"), EndUse::BATTERY);
        assert_eq!(equipment_name_to_end_use("PV"), EndUse::PV);
        assert_eq!(
            equipment_name_to_end_use("Gas Water Heater"),
            EndUse::WATER_HEATING
        );
        assert_eq!(
            equipment_name_to_end_use("Ceiling Fan"),
            EndUse::CEILING_FAN
        );
        assert_eq!(equipment_name_to_end_use("Gas Grill"), EndUse::COOKING);
        assert_eq!(equipment_name_to_end_use("Pool Pump"), EndUse::POOL_PUMP);
        assert_eq!(
            equipment_name_to_end_use("Pool Heater"),
            EndUse::POOL_HEATER
        );
        assert_eq!(equipment_name_to_end_use("Clothes Washer"), EndUse::OTHER);
    }

    /// `display_name_to_end_use_key` maps every standard EndUse display name
    /// to its canonical key string, and returns `None` for unknown names.
    #[test]
    fn display_name_to_end_use_key_maps_all_standard_end_uses() {
        assert_eq!(
            display_name_to_end_use_key("HVAC Heating"),
            Some("hvac_heating".to_string())
        );
        assert_eq!(
            display_name_to_end_use_key("HVAC Cooling"),
            Some("hvac_cooling".to_string())
        );
        assert_eq!(
            display_name_to_end_use_key("Water Heating"),
            Some("water_heating".to_string())
        );
        assert_eq!(
            display_name_to_end_use_key("Lighting"),
            Some("lighting".to_string())
        );
        assert_eq!(
            display_name_to_end_use_key("Plug Loads"),
            Some("plug_loads".to_string())
        );
        assert_eq!(
            display_name_to_end_use_key("Refrigeration"),
            Some("refrigeration".to_string())
        );
        assert_eq!(
            display_name_to_end_use_key("Ventilation"),
            Some("ventilation".to_string())
        );
        assert_eq!(
            display_name_to_end_use_key("Battery"),
            Some("battery".to_string())
        );
        assert_eq!(display_name_to_end_use_key("PV"), Some("pv".to_string()));
        assert_eq!(display_name_to_end_use_key("EV"), Some("ev".to_string()));
        assert_eq!(
            display_name_to_end_use_key("Generator"),
            Some("generator".to_string())
        );
        assert_eq!(
            display_name_to_end_use_key("Cooking"),
            Some("cooking".to_string())
        );
        assert_eq!(
            display_name_to_end_use_key("Laundry"),
            Some("laundry".to_string())
        );
        assert_eq!(
            display_name_to_end_use_key("Dishwasher"),
            Some("dishwasher".to_string())
        );
        assert_eq!(
            display_name_to_end_use_key("Pool Pump"),
            Some("pool_pump".to_string())
        );
        assert_eq!(
            display_name_to_end_use_key("Pool Heater"),
            Some("pool_heater".to_string())
        );
        assert_eq!(
            display_name_to_end_use_key("Spa Pump"),
            Some("spa_pump".to_string())
        );
        assert_eq!(
            display_name_to_end_use_key("Spa Heater"),
            Some("spa_heater".to_string())
        );
        assert_eq!(
            display_name_to_end_use_key("Ceiling Fan"),
            Some("ceiling_fan".to_string())
        );
        assert_eq!(
            display_name_to_end_use_key("Dehumidifier"),
            Some("dehumidifier".to_string())
        );
        assert_eq!(
            display_name_to_end_use_key("Other"),
            Some("other".to_string())
        );
        // Unknown display names return None.
        assert_eq!(display_name_to_end_use_key("ASHP Heater"), None);
        assert_eq!(display_name_to_end_use_key("Total"), None);
    }

    /// `end_use_display_name` maps every standard EndUse to a display
    /// name that round-trips through `display_name_to_end_use_key` and
    /// matches `EndUse::as_str()` converted to key form.
    #[test]
    fn end_use_display_name_round_trips_all_standard_variants() {
        let standard = &[
            (
                &hares_types::EndUse::HVAC_HEATING,
                "HVAC Heating",
                "hvac_heating",
            ),
            (
                &hares_types::EndUse::HVAC_COOLING,
                "HVAC Cooling",
                "hvac_cooling",
            ),
            (
                &hares_types::EndUse::WATER_HEATING,
                "Water Heating",
                "water_heating",
            ),
            (&hares_types::EndUse::LIGHTING, "Lighting", "lighting"),
            (&hares_types::EndUse::PLUG_LOADS, "Plug Loads", "plug_loads"),
            (
                &hares_types::EndUse::REFRIGERATION,
                "Refrigeration",
                "refrigeration",
            ),
            (
                &hares_types::EndUse::VENTILATION,
                "Ventilation",
                "ventilation",
            ),
            (&hares_types::EndUse::BATTERY, "Battery", "battery"),
            (&hares_types::EndUse::PV, "PV", "pv"),
            (&hares_types::EndUse::EV, "EV", "ev"),
            (&hares_types::EndUse::GENERATOR, "Generator", "generator"),
            (&hares_types::EndUse::COOKING, "Cooking", "cooking"),
            (&hares_types::EndUse::LAUNDRY, "Laundry", "laundry"),
            (&hares_types::EndUse::DISHWASHER, "Dishwasher", "dishwasher"),
            (&hares_types::EndUse::POOL_PUMP, "Pool Pump", "pool_pump"),
            (
                &hares_types::EndUse::POOL_HEATER,
                "Pool Heater",
                "pool_heater",
            ),
            (&hares_types::EndUse::SPA_PUMP, "Spa Pump", "spa_pump"),
            (&hares_types::EndUse::SPA_HEATER, "Spa Heater", "spa_heater"),
            (
                &hares_types::EndUse::CEILING_FAN,
                "Ceiling Fan",
                "ceiling_fan",
            ),
            (
                &hares_types::EndUse::DEHUMIDIFIER,
                "Dehumidifier",
                "dehumidifier",
            ),
            (&hares_types::EndUse::OTHER, "Other", "other"),
        ];
        for (end_use, expected_display, expected_key) in standard {
            let display = end_use_display_name(end_use);
            assert_eq!(
                display, *expected_display,
                "end_use_display_name({expected_key}) = '{display}', expected '{expected_display}'"
            );
            assert_eq!(
                display_name_to_end_use_key(display),
                Some(expected_key.to_string()),
                "display_name_to_end_use_key('{display}') round-trip failed for {expected_key}"
            );
        }
    }

    /// `is_reserved_output_column_name` flags exactly the schema's
    /// non-per-equipment columns that occupy the per-equipment
    /// `{name} {suffix}` namespace: the level-0 aggregate totals and the
    /// per-end-use aggregates. Per-equipment columns and fixed columns
    /// outside that namespace (zone/setpoint/context columns) are not
    /// reserved — the membership test in the dwelling's column map must
    /// keep resolving them.
    #[test]
    fn is_reserved_output_column_name_flags_the_aggregate_namespace() {
        for reserved in [
            "Total Electric Power (kW)",
            "Total Gas Power (therms/hour)",
            "Total Reactive Power (kVAR)",
            "HVAC Heating End Use Electric Power (kW)",
            "Other End Use Electric Power (kW)",
        ] {
            assert!(
                is_reserved_output_column_name(reserved),
                "'{reserved}' is a reserved aggregate column occupying the \
                 per-equipment namespace"
            );
        }
        for not_reserved in [
            "Clothes Washer Electric Power (kW)",
            "EV Mode (-)",
            "Total Mode (-)",
            "Total Energy (kWh)",
            "Outdoor Dry Bulb (C)",
            "HVAC Duct Losses (W)",
            "Scheduled Heating Setpoint (C)",
        ] {
            assert!(
                !is_reserved_output_column_name(not_reserved),
                "'{not_reserved}' is not a reserved column — the per-equipment \
                 membership test must not exclude it"
            );
        }
    }

    /// An equipment named "Total" shadows the level-0 aggregate column
    /// name: the schema must emit that name exactly once (the aggregate) and
    /// no other per-equipment columns for that name at any verbosity — a
    /// duplicate field name collapses in the name-keyed column index, and
    /// the advertised aggregate column becomes the one never written. The
    /// equipment's end-use aggregate is still emitted: its energy keeps
    /// counting toward the aggregates computed from port/core outputs.
    #[test]
    fn build_schema_emits_no_per_equipment_columns_for_a_name_shadowing_an_aggregate() {
        let specs = vec![make_spec("Total", FuelType::Electric)];
        let schema = build_schema_for_specs(&specs, 8, &[]);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(
            names
                .iter()
                .filter(|&&n| n == "Total Electric Power (kW)")
                .count(),
            1,
            "the aggregate column name must appear exactly once (the \
             level-0 aggregate), got: {names:?}"
        );
        assert_eq!(
            names
                .iter()
                .filter(|&&n| n == "Total Reactive Power (kVAR)")
                .count(),
            1,
            "the reactive aggregate must appear exactly once — the \
             per-equipment reactive column for the shadowing name would be a \
             duplicate, got: {names:?}"
        );
        for shadowed in [
            "Total Mode (-)",
            "Total Energy (kWh)",
            "Total Power Factor (-)",
        ] {
            assert!(
                !names.contains(&shadowed),
                "no per-equipment column may be emitted for the shadowing \
                 name, got: {names:?}"
            );
        }
        assert!(
            names.contains(&"Other End Use Electric Power (kW)"),
            "the shadowing equipment's end-use aggregate must still be \
             emitted, got: {names:?}"
        );
    }

    /// `end_use_electric_power_column` returns column names that are distinct
    /// from per-equipment column names, even when the equipment name matches an
    /// EndUse display name (e.g. "Battery", "PV", "Ceiling Fan").  This
    /// prevents schema collisions in Arrow IPC readers.
    #[test]
    fn end_use_electric_power_column_has_unique_names() {
        use hares_types::EndUse;
        // Equipment names that collide with their EndUse display name.
        let collision_prone: &[(&str, EndUse)] = &[
            ("Battery", EndUse::BATTERY),
            ("PV", EndUse::PV),
            ("Ceiling Fan", EndUse::CEILING_FAN),
            ("Lighting", EndUse::LIGHTING),
            ("Plug Loads", EndUse::PLUG_LOADS),
            ("Refrigeration", EndUse::REFRIGERATION),
            ("Ventilation", EndUse::VENTILATION),
            ("Cooking", EndUse::COOKING),
            ("Generator", EndUse::GENERATOR),
            ("Dehumidifier", EndUse::DEHUMIDIFIER),
        ];
        for (eq_name, end_use) in collision_prone {
            let per_eq = format!("{eq_name} {ELECTRIC_POWER_SUFFIX}");
            let aggregate = end_use_electric_power_column(end_use);
            assert_ne!(
                per_eq, aggregate,
                "per-equipment column '{per_eq}' must differ from aggregate column '{aggregate}'"
            );
        }
    }

    /// Using collision-prone equipment (Ceiling Fan, Battery, PV) whose names
    /// match EndUse display names, the schema must contain both per-equipment
    /// and aggregate columns without any duplicate field names.
    #[test]
    fn no_duplicate_column_names_in_schema() {
        let specs = vec![
            make_spec("Ceiling Fan", FuelType::Electric),
            make_spec("Battery", FuelType::Electric),
            make_spec("PV", FuelType::Electric),
        ];
        let schema = build_schema_for_specs(&specs, 1, &[]);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        // Aggregate columns use the "End Use" suffix.
        assert!(
            names.contains(&"Ceiling Fan End Use Electric Power (kW)"),
            "missing aggregate: 'Ceiling Fan End Use Electric Power (kW)'; got: {names:?}"
        );
        assert!(
            names.contains(&"Battery End Use Electric Power (kW)"),
            "missing aggregate: 'Battery End Use Electric Power (kW)'; got: {names:?}"
        );
        assert!(
            names.contains(&"PV End Use Electric Power (kW)"),
            "missing aggregate: 'PV End Use Electric Power (kW)'; got: {names:?}"
        );
        // Per-equipment columns are still present.
        assert!(
            names.contains(&"Ceiling Fan Electric Power (kW)"),
            "missing per-equipment: 'Ceiling Fan Electric Power (kW)'; got: {names:?}"
        );
        assert!(
            names.contains(&"Battery Electric Power (kW)"),
            "missing per-equipment: 'Battery Electric Power (kW)'; got: {names:?}"
        );
        assert!(
            names.contains(&"PV Electric Power (kW)"),
            "missing per-equipment: 'PV Electric Power (kW)'; got: {names:?}"
        );
        // No duplicate field names anywhere in the schema.
        let mut seen = std::collections::HashSet::new();
        for name in &names {
            assert!(
                seen.insert(*name),
                "duplicate column name '{name}' in schema"
            );
        }
    }

    /// `parse_end_use_electric_power_column` round-trips correctly for all
    /// standard end-uses and rejects per-equipment and total columns.
    #[test]
    fn parse_end_use_electric_power_column_round_trip() {
        use hares_types::EndUse;
        let end_uses: &[EndUse] = &[
            EndUse::HVAC_HEATING,
            EndUse::HVAC_COOLING,
            EndUse::BATTERY,
            EndUse::PV,
            EndUse::CEILING_FAN,
            EndUse::WATER_HEATING,
            EndUse::LIGHTING,
            EndUse::DEHUMIDIFIER,
            EndUse::OTHER,
        ];
        for end_use in end_uses {
            let col = end_use_electric_power_column(end_use);
            let parsed = parse_end_use_electric_power_column(&col);
            assert_eq!(
                parsed,
                Some(end_use.clone()),
                "round-trip failed for {end_use:?}: column='{col}', parsed={parsed:?}"
            );
        }
        // Per-equipment columns should not parse as end-use aggregate.
        assert_eq!(
            parse_end_use_electric_power_column("Battery Electric Power (kW)"),
            None
        );
        assert_eq!(
            parse_end_use_electric_power_column("ASHP Heater Electric Power (kW)"),
            None
        );
        assert_eq!(
            parse_end_use_electric_power_column("Total Electric Power (kW)"),
            None
        );
    }

    // ── Verbosity 8 per-equipment telemetry column tests ─────────────────

    /// HVAC/WH equipment gets temperature and compressor power columns at v8.
    #[test]
    fn hvac_equipment_gets_v8_telemetry_columns() {
        let specs = vec![make_spec("ASHP Heater", FuelType::Electric)];
        let schema = build_schema_for_specs(&specs, 8, &[]);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(names.contains(&"ASHP Heater Supply Temperature (C)"));
        assert!(names.contains(&"ASHP Heater Return Temperature (C)"));
        assert!(names.contains(&"ASHP Heater Compressor Power (W)"));
        assert!(names.contains(&"ASHP Heater Compressor Power (kW)"));
        assert!(names.contains(&"ASHP Heater Fan Electric Power (W)"));
        assert!(names.contains(&"ASHP Heater Fan Power (W)"));
        // Heat-pump-specific columns.
        assert!(names.contains(&"ASHP Heater Supply Air Temperature (C)"));
        assert!(names.contains(&"ASHP Heater Pan Heater Power (kW)"));
        assert!(names.contains(&"ASHP Heater Heat Pump Capacity (W)"));
        assert!(names.contains(&"ASHP Heater ER Capacity (W)"));
    }

    /// PV equipment gets diagnostic columns at v8.
    #[test]
    fn pv_equipment_gets_v8_telemetry_columns() {
        let specs = vec![make_spec("PV", FuelType::Electric)];
        let schema = build_schema_for_specs(&specs, 8, &[]);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(names.contains(&"PV DC Power (kW)"));
        assert!(names.contains(&"PV Irradiance (W/m2)"));
    }

    /// EV equipment gets diagnostic columns at v8.
    #[test]
    fn ev_equipment_gets_v8_telemetry_columns() {
        let specs = vec![make_spec("EV", FuelType::Electric)];
        let schema = build_schema_for_specs(&specs, 8, &[]);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(names.contains(&"EV Connection State (-)"));
        assert!(names.contains(&"EV Charging Level (-)"));
    }

    /// Non-HVAC/PV/EV equipment does NOT get v8 telemetry columns.
    #[test]
    fn non_diagnostic_equipment_has_no_v8_telemetry_columns() {
        let specs = vec![make_spec("Battery", FuelType::Electric)];
        let s7 = build_schema_for_specs(&specs, 7, &[]);
        let s8 = build_schema_for_specs(&specs, 8, &[]);
        let n7: Vec<&str> = s7.fields().iter().map(|f| f.name().as_str()).collect();
        let n8: Vec<&str> = s8.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(!n7.contains(&"Battery Supply Temperature (C)"));
        assert!(!n8.contains(&"Battery Supply Temperature (C)"));
        assert!(!n8.contains(&"Battery DC Power (kW)"));
        assert!(!n8.contains(&"Battery Connection State (-)"));
        // v8 length equals v7 length for non-diagnostic equipment.
        assert_eq!(s8.fields().len(), s7.fields().len());
    }

    /// is_pv / is_ev / is_compressor_equipment correctly classify equipment names.
    #[test]
    fn equipment_classification_functions() {
        assert!(is_pv("PV"));
        assert!(!is_pv("Battery"));
        assert!(is_ev("EV"));
        assert!(is_ev("Electric Vehicle"));
        assert!(!is_ev("PV"));
        assert!(is_compressor_equipment("ASHP Heater"));
        assert!(is_compressor_equipment("Air Conditioner"));
        assert!(is_compressor_equipment("MSHP Cooler"));
        assert!(!is_compressor_equipment("Gas Furnace"));
        assert!(!is_compressor_equipment("Battery"));
    }

    /// Output schema columns carry unit metadata on their Arrow fields, verified
    /// against independent hand-written expected units rather than recomputing
    /// the expected value from the same extraction function. The metadata
    /// comes from the catalogue's `Unit::symbol()` (the pre-catalogue schema
    /// parsed it back out of the column name).
    #[test]
    fn output_schema_columns_carry_unit_metadata() {
        let specs = vec![make_spec("ASHP Heater", FuelType::Electric)];
        let schema = build_schema_for_specs(&specs, 3, &[]);
        let expected_units: &[(&str, &str)] = &[
            ("Total Electric Power (kW)", "kW"),
            ("Total Gas Power (therms/hour)", "therms/hour"),
            ("Total Reactive Power (kVAR)", "kVAR"),
            ("Outdoor Dry Bulb (C)", "C"),
            ("Temperature - Indoor (C)", "C"),
            ("ASHP Heater Electric Power (kW)", "kW"),
            ("ASHP Heater Mode (-)", "-"),
            ("ASHP Heater Setpoint (C)", "C"),
        ];
        for (col_name, expected_unit) in expected_units {
            let field = schema
                .fields()
                .iter()
                .find(|f| f.name() == *col_name)
                .unwrap_or_else(|| panic!("column '{col_name}' not found in schema"));
            assert_eq!(
                field.metadata().get("unit").map(|s| s.as_str()),
                Some(*expected_unit),
                "column '{col_name}' should have unit='{expected_unit}' in metadata"
            );
        }
        let time_field = schema.fields().iter().find(|f| f.name() == "Time").unwrap();
        assert!(
            time_field.metadata().get("unit").is_none(),
            "Time column should not have unit metadata"
        );
    }
}
