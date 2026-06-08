//! Output column definitions and verbosity-level schema construction.
//!
//! Column naming follows OCHRE conventions: `"{Name} {Metric} ({Unit})"`.
//! Multi-instance equipment includes an instance qualifier: `"Battery #1 SOC (-)"`.

use arrow::datatypes::{DataType, Field, Schema};

use crate::hpxml::EquipmentSpec;
use crate::hpxml::equipment::canonical_instance_namer;
#[cfg(any(debug_assertions, feature = "check_invariants"))]
use hares_types::OperatingMode;
use hares_types::{EndUse, FuelType, ZoneId};
#[cfg(any(debug_assertions, feature = "check_invariants"))]
use strum::IntoEnumIterator;

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

        // Lighting
        "Lighting" | "Indoor Lighting" | "Outdoor Lighting" | "Garage Lighting" => EndUse::LIGHTING,

        // Plug Loads
        "Plug Loads" | "MELs" => EndUse::PLUG_LOADS,

        // Refrigeration
        "Refrigerator" | "Freezer" => EndUse::REFRIGERATION,

        // Ventilation
        "Ventilation Fan" | "HRV" | "ERV" => EndUse::VENTILATION,

        // Ceiling Fan (separate from whole-house ventilation per HPXML 4.2)
        "Ceiling Fan" => EndUse::CEILING_FAN,

        // Cooking
        "Gas Grill" => EndUse::COOKING,

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

/// Builds an Arrow schema for the output based on equipment list, zone names,
/// and verbosity.
///
/// The schema always includes a timestamp column, followed by columns
/// appropriate for the requested verbosity level.
pub fn build_schema(
    equipment_list: &[EquipmentSpec],
    verbosity: u8,
    zone_names: &[(ZoneId, String)],
) -> Schema {
    let mut fields = vec![Field::new(TIMESTAMP_COL, DataType::Utf8, false)];

    // Level 0: total power
    for &col in LEVEL_0_COLUMNS {
        fields.push(Field::new(col, DataType::Float64, true));
    }

    // Always-present context columns: outdoor temp and primary indoor zone temp.
    // These appear at every verbosity level so any output file is self-contained
    // enough to correlate equipment behavior with conditions.
    fields.push(Field::new(OUTDOOR_TEMP_COL, DataType::Float64, true));
    fields.push(Field::new(
        format!("{ZONE_TEMP_PREFIX} Indoor {ZONE_TEMP_UNIT}"),
        DataType::Float64,
        true,
    ));

    // Compute instance-qualified names once for all verbosity levels that need them.
    let names = if verbosity >= 1 && !equipment_list.is_empty() {
        instance_qualified_names(equipment_list)
    } else {
        Vec::new()
    };

    if verbosity >= 1 {
        for (name, fuel) in &names {
            fields.push(Field::new(
                format!("{name} {ELECTRIC_POWER_SUFFIX}"),
                DataType::Float64,
                true,
            ));
            if matches!(fuel, FuelType::Gas | FuelType::Propane | FuelType::Oil) {
                fields.push(Field::new(
                    format!("{name} {GAS_POWER_SUFFIX}"),
                    DataType::Float64,
                    true,
                ));
            }
        }

        // Per-EndUse aggregate electric power columns.
        // One column per EndUse category that has at least one equipment,
        // summing all equipment contributions so metrics aggregation keys
        // on EndUse category rather than equipment instance name.
        //
        // Aggregate columns use end_use_electric_power_column() for a
        // stable, guaranteed-unique namespace (e.g. "HVAC Heating End Use
        // Electric Power (kW)").  This avoids collisions with per-equipment
        // columns whose equipment name may match the end-use display name.
        let mut seen_end_uses: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        for spec in equipment_list {
            let end_use = equipment_name_to_end_use(&spec.name);
            if seen_end_uses.insert(end_use_display_name(&end_use).to_string()) {
                fields.push(Field::new(
                    end_use_electric_power_column(&end_use),
                    DataType::Float64,
                    true,
                ));
            }
        }
    }

    if verbosity >= 2 {
        // Per-zone temperature columns for non-Indoor zones.
        // Indoor temperature is always present as a context column at every
        // verbosity level (added above). Attic and other structural zones
        // are emitted only when present in the zone_names list, eliminating
        // all-null columns for simulations without those zones.
        for (_zone_id, zone_name) in zone_names {
            if zone_name != "Indoor" {
                fields.push(Field::new(
                    format!("{ZONE_TEMP_PREFIX} {zone_name} {ZONE_TEMP_UNIT}"),
                    DataType::Float64,
                    true,
                ));
            }
        }
        fields.push(Field::new(ZONE_UNMET_LOAD_COL, DataType::Float64, true));
        // Temperature - Ground (C) is always emitted (not zone-conditional):
        // it is populated from weather.ground_temp_c which is valid for
        // every simulation, regardless of zone configuration. OCHRE convention
        // includes it unconditionally alongside Outdoor Dry Bulb.
        fields.push(Field::new(GROUND_TEMP_COL, DataType::Float64, true));
    }

    if verbosity >= 3 {
        for (name, _fuel) in &names {
            fields.push(Field::new(
                format!("{name} {MODE_SUFFIX}"),
                DataType::Float64,
                true,
            ));
            if is_hvac_or_wh(name) {
                fields.push(Field::new(
                    format!("{name} {SETPOINT_SUFFIX}"),
                    DataType::Float64,
                    true,
                ));
            }
            if has_soc(name) {
                fields.push(Field::new(
                    format!("{name} {SOC_SUFFIX}"),
                    DataType::Float64,
                    true,
                ));
            }
        }
    }

    if verbosity >= 4 {
        for (name, _fuel) in &names {
            fields.push(Field::new(
                format!("{name} {ENERGY_SUFFIX}"),
                DataType::Float64,
                true,
            ));
        }
        // HVAC thermal delivery columns (OCHRE convention, verbosity 4).
        fields.push(Field::new(
            "HVAC Heating Delivered (W)",
            DataType::Float64,
            true,
        ));
        fields.push(Field::new(
            "HVAC Cooling Delivered (W)",
            DataType::Float64,
            true,
        ));
    }

    if verbosity >= 5 {
        for (name, fuel) in &names {
            if matches!(fuel, FuelType::Electric) {
                fields.push(Field::new(
                    format!("{name} {REACTIVE_POWER_SUFFIX}"),
                    DataType::Float64,
                    true,
                ));
                fields.push(Field::new(
                    format!("{name} {POWER_FACTOR_SUFFIX}"),
                    DataType::Float64,
                    true,
                ));
            }
        }
        fields.push(Field::new(
            NET_SENSIBLE_HEAT_GAIN_COL,
            DataType::Float64,
            true,
        ));
        // OCHRE HVAC.py:588: duct losses at verbosity 5.
        // Computed as gross_capacity_w * (1 - dse) per ASHRAE 152.
        fields.push(Field::new(HVAC_DUCT_LOSSES_COL, DataType::Float64, true));
    }

    if verbosity >= 6 {
        // Level 6: component loads per boundary.
        // Note: Energy (kWh) columns are already added at verbosity 4.
        // Indoor zone envelope breakdown — always present.
        for label in &[
            "Window Transmitted Solar Gain (W)",
            "Infiltration Heat Gain - Indoor (W)",
            "Forced Ventilation Heat Gain - Indoor (W)",
            "Natural Ventilation Heat Gain - Indoor (W)",
            "Internal Heat Gain - Indoor (W)",
            "Interior LWR Exchange - Indoor (W)",
            "Opaque Surface Heat Gain - Indoor (W)",
            "Duct Loss Heat Gain - Indoor (W)",
            "Roof Heat Gain - Indoor (W)",
            "Floor Heat Gain - Indoor (W)",
            "Wall Heat Gain - Indoor (W)",
            "Window Heat Gain - Indoor (W)",
            "Internal Mass Heat Gain - Indoor (W)",
        ] {
            fields.push(Field::new(*label, DataType::Float64, true));
        }
        // Non-Indoor zone envelope breakdown — conditional on zone presence.
        // Eliminates all-null columns (see review Finding 3) for simulations
        // without attic or other structural zones.
        for (_zone_id, zone_name) in zone_names {
            if zone_name != "Indoor" {
                fields.push(Field::new(
                    format!("Infiltration Heat Gain - {zone_name} (W)"),
                    DataType::Float64,
                    true,
                ));
                fields.push(Field::new(
                    format!("Interior LWR Exchange - {zone_name} (W)"),
                    DataType::Float64,
                    true,
                ));
            }
        }
        // Per-zone HVAC thermal attribution columns.
        // Enables diagnosing how much heating/cooling went to each zone
        // (conditioned, basement, duct) in multi-zone buildings.
        for (_zone_id, zone_name) in zone_names {
            fields.push(Field::new(
                format!("HVAC Heating Delivered - {zone_name} (W)"),
                DataType::Float64,
                true,
            ));
            fields.push(Field::new(
                format!("HVAC Cooling Delivered - {zone_name} (W)"),
                DataType::Float64,
                true,
            ));
        }
    }

    if verbosity >= 7 {
        // Level 7: schedule inputs and detailed equipment modes.
        for (name, _fuel) in &names {
            fields.push(Field::new(
                format!("{name} {SCHEDULE_SUFFIX}"),
                DataType::Float64,
                true,
            ));
            if is_heat_pump_heater(name) {
                fields.push(Field::new(
                    format!("{name} {DEFROST_STATE_SUFFIX}"),
                    DataType::Float64,
                    true,
                ));
                // OCHRE HVAC.py:1464-1467: ER Power column for ASHP heaters at v7.
                // Reports backup electric resistance power during ER-only or HP+ER modes.
                fields.push(Field::new(
                    format!("{name} {ER_POWER_SUFFIX}"),
                    DataType::Float64,
                    true,
                ));
            }
            // OCHRE HVAC.py:590-599: per-equipment HVAC performance columns at v7.
            if is_hvac_or_wh(name) {
                // SHR and Latent Gains are cooling-only (OCHRE HVAC.py:595-596).
                if is_cooling_equipment(name) {
                    fields.push(Field::new(
                        format!("{name} {SHR_SUFFIX}"),
                        DataType::Float64,
                        true,
                    ));
                    fields.push(Field::new(
                        format!("{name} {LATENT_GAINS_SUFFIX}"),
                        DataType::Float64,
                        true,
                    ));
                }
                fields.push(Field::new(
                    format!("{name} {SPEED_SUFFIX}"),
                    DataType::Float64,
                    true,
                ));
                // OCHRE HVAC.py:575: main_power = total_input - fan.
                fields.push(Field::new(
                    format!("{name} {MAIN_POWER_SUFFIX}"),
                    DataType::Float64,
                    true,
                ));
                fields.push(Field::new(
                    format!("{name} {FAN_POWER_SUFFIX}"),
                    DataType::Float64,
                    true,
                ));
                fields.push(Field::new(
                    format!("{name} {RUNTIME_FRACTION_SUFFIX}"),
                    DataType::Float64,
                    true,
                ));
                // Promoted from v8: OCHRE emits Capacity and COP at v7 (HVAC.py:584,598).
                fields.push(Field::new(
                    format!("{name} {CAPACITY_SUFFIX}"),
                    DataType::Float64,
                    true,
                ));
                fields.push(Field::new(
                    format!("{name} {COP_SUFFIX}"),
                    DataType::Float64,
                    true,
                ));
            }
        }
        fields.push(Field::new(
            HOT_WATER_MAINS_TEMP_COL,
            DataType::Float64,
            true,
        ));
        // Setpoint chain: dwelling-level context columns for control auditing.
        // Each column reflects a stage in the setpoint resolution pipeline
        // (schedule → runtime override), enabling diagnosis of DR/override
        // behaviour without tracing per-equipment telemetry.
        fields.push(Field::new(
            SCHEDULED_HEATING_SETPOINT_COL,
            DataType::Float64,
            true,
        ));
        fields.push(Field::new(
            SCHEDULED_COOLING_SETPOINT_COL,
            DataType::Float64,
            true,
        ));
        fields.push(Field::new(
            RUNTIME_HEATING_SETPOINT_COL,
            DataType::Float64,
            true,
        ));
        fields.push(Field::new(
            RUNTIME_COOLING_SETPOINT_COL,
            DataType::Float64,
            true,
        ));
    }

    if verbosity >= 8 {
        // Level 8: per-equipment telemetry diagnostic columns.
        // Equipment temperatures, compressor power, PV/EV diagnostics — these
        // keys are not in CoreOutput and remain telemetry-only below v8.
        for (name, _fuel) in &names {
            if is_hvac_or_wh(name) {
                fields.push(Field::new(
                    format!("{name} {SUPPLY_TEMP_SUFFIX}"),
                    DataType::Float64,
                    true,
                ));
                fields.push(Field::new(
                    format!("{name} {RETURN_TEMP_SUFFIX}"),
                    DataType::Float64,
                    true,
                ));
                fields.push(Field::new(
                    format!("{name} {COMPRESSOR_POWER_W_SUFFIX}"),
                    DataType::Float64,
                    true,
                ));
                fields.push(Field::new(
                    format!("{name} {COMPRESSOR_POWER_KW_SUFFIX}"),
                    DataType::Float64,
                    true,
                ));
                fields.push(Field::new(
                    format!("{name} {FAN_ELECTRIC_POWER_SUFFIX}"),
                    DataType::Float64,
                    true,
                ));
                fields.push(Field::new(
                    format!("{name} {FAN_POWER_W_SUFFIX}"),
                    DataType::Float64,
                    true,
                ));
            }
            if is_heat_pump_heater(name) {
                fields.push(Field::new(
                    format!("{name} {SUPPLY_AIR_TEMP_SUFFIX}"),
                    DataType::Float64,
                    true,
                ));
                fields.push(Field::new(
                    format!("{name} {PAN_HEATER_POWER_SUFFIX}"),
                    DataType::Float64,
                    true,
                ));
                fields.push(Field::new(
                    format!("{name} {HP_CAPACITY_SUFFIX}"),
                    DataType::Float64,
                    true,
                ));
                fields.push(Field::new(
                    format!("{name} {ER_CAPACITY_SUFFIX}"),
                    DataType::Float64,
                    true,
                ));
            }
            if is_pv(name) {
                fields.push(Field::new(
                    format!("{name} {PV_DC_POWER_SUFFIX}"),
                    DataType::Float64,
                    true,
                ));
                fields.push(Field::new(
                    format!("{name} {PV_IRRADIANCE_SUFFIX}"),
                    DataType::Float64,
                    true,
                ));
            }
            if is_ev(name) {
                fields.push(Field::new(
                    format!("{name} {EV_CONNECTION_STATE_SUFFIX}"),
                    DataType::Float64,
                    true,
                ));
                fields.push(Field::new(
                    format!("{name} {EV_CHARGING_LEVEL_SUFFIX}"),
                    DataType::Float64,
                    true,
                ));
            }
        }
    }

    #[cfg(feature = "observe")]
    {
        let zone_temp_count = fields
            .iter()
            .filter(|f| {
                f.name().starts_with("Temperature - ")
                    && f.name().ends_with("(C)")
                    && !f.name().starts_with("Temperature - Indoor")
                    && f.name() != GROUND_TEMP_COL
            })
            .count();
        let zone_inf_lwr_count = fields
            .iter()
            .filter(|f| {
                f.name().starts_with("Infiltration Heat Gain - ")
                    && !f.name().starts_with("Infiltration Heat Gain - Indoor")
            })
            .count();
        let any_attic = zone_names.iter().any(|(_, n)| n == "Attic");
        tracing::info!(
            verbosity,
            zone_count = zone_names.len(),
            zone_temp_columns = zone_temp_count,
            zone_infiltration_lwr_columns = zone_inf_lwr_count,
            attic_present = any_attic,
            "output schema constructed"
        );
    }

    let mut metadata = std::collections::HashMap::new();
    metadata.insert("hares_verbosity".to_string(), verbosity.to_string());
    metadata.insert("hares_mode_map".to_string(), mode_ordinal_json());

    Schema::new_with_metadata(fields, metadata)
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

/// JSON encoding of the `OperatingMode` → ordinal mapping, embedded as
/// Parquet custom metadata so output files are self-describing.
///
/// Ordinal values:
///   Off=0, Heating=1, Cooling=2, Defrost=3, Standby=4, Charging=5,
///   Discharging=6, HeatingHP=7, HeatingER=8, HeatingHPAndER=9,
///   HeatPumpWH=10, BackupElement=11, On=12
fn mode_ordinal_json() -> String {
    serde_json::to_string(&MODE_ORDINALS).expect("static map serialises")
}

const MODE_ORDINALS: &[(&str, u8)] = &[
    ("Off", 0),
    ("Heating", 1),
    ("Cooling", 2),
    ("Defrost", 3),
    ("Standby", 4),
    ("Charging", 5),
    ("Discharging", 6),
    ("HeatingHP", 7),
    ("HeatingER", 8),
    ("HeatingHPAndER", 9),
    ("HeatPumpWH", 10),
    ("BackupElement", 11),
    ("On", 12),
];

/// Gated invariant: every `OperatingMode` variant must have a corresponding
/// entry in `MODE_ORDINALS` so that Parquet output files remain self-describing.
///
/// Uses `strum::EnumIter` to enumerate variants directly from the enum
/// definition, eliminating the manually-maintained parallel list that was the
/// root cause of the original drift.
///
/// This guard runs at startup in debug builds or when
/// `feature = "check_invariants"` is enabled. It is not part of the hot path.
#[cfg(any(debug_assertions, feature = "check_invariants"))]
pub fn check_mode_ordinals_invariant() {
    for mode in OperatingMode::iter() {
        let ordinal = mode as u8;
        let found = MODE_ORDINALS.iter().any(|(_, o)| *o == ordinal);
        if !found {
            tracing::error!(
                ordinal,
                variant = ?mode,
                "MODE_ORDINALS is missing entry for OperatingMode variant; \
                 Parquet metadata will not be self-describing for files \
                 containing this mode"
            );
        }
    }
}

/// Generates instance-qualified names for multi-instance equipment.
///
/// If an equipment name appears more than once, instances are numbered:
/// `"Battery #1"`, `"Battery #2"`. Single instances keep the bare name.
fn instance_qualified_names(specs: &[EquipmentSpec]) -> Vec<(String, FuelType)> {
    use std::collections::HashMap;

    let mut counts: HashMap<&str, usize> = HashMap::new();
    for spec in specs {
        *counts.entry(&spec.name).or_insert(0) += 1;
    }

    let mut indices: HashMap<&str, usize> = HashMap::new();
    let mut result = Vec::with_capacity(specs.len());
    for spec in specs {
        if let Some(ref instance_name) = spec.instance_name {
            result.push((instance_name.clone(), spec.fuel_type));
        } else {
            let total = counts.get(spec.name.as_str()).copied().unwrap_or(1);
            if total > 1 {
                let idx = indices.entry(&spec.name).or_insert(0);
                *idx += 1;
                result.push((canonical_instance_namer(&spec.name, *idx), spec.fuel_type));
            } else {
                result.push((spec.name.clone(), spec.fuel_type));
            }
        }
    }
    result
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

    fn make_spec(name: &str, fuel: FuelType) -> EquipmentSpec {
        EquipmentSpec {
            instance_name: None,
            name: name.to_string(),
            fuel_type: fuel,
            parameters: Map::new(),
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        }
    }

    #[test]
    fn verbosity_0_has_timestamp_total_power_and_context_columns() {
        let schema = build_schema(&[], 0, &[]);
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
        let schema = build_schema(&specs, 1, &[]);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(names.contains(&"ASHP Heater Electric Power (kW)"));
        assert!(names.contains(&"Gas Furnace Electric Power (kW)"));
        assert!(names.contains(&"Gas Furnace Gas Power (therms/hour)"));
    }

    #[test]
    fn multi_instance_gets_numbered_names() {
        let specs = vec![
            make_spec("Battery", FuelType::Electric),
            make_spec("Battery", FuelType::Electric),
        ];
        let schema = build_schema(&specs, 1, &[]);
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
        let schema = build_schema(&[], 2, &zone_names);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(names.contains(&"Temperature - Attic (C)"));
        assert!(names.contains(&"Unmet HVAC Load (C)"));
        assert!(names.contains(&"Temperature - Ground (C)"));
    }

    #[test]
    fn verbosity_2_skips_attic_temp_when_no_attic_zone() {
        let schema = build_schema(&[], 2, &[]);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(!names.contains(&"Temperature - Attic (C)"));
        assert!(names.contains(&"Unmet HVAC Load (C)"));
        assert!(names.contains(&"Temperature - Ground (C)"));
    }

    #[test]
    fn verbosity_5_adds_net_sensible_heat_gain_column() {
        let schema = build_schema(&[], 5, &[]);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(names.contains(&"Net Sensible Heat Gain - Indoor (W)"));
    }

    #[test]
    fn verbosity_7_adds_hot_water_mains_temperature_column() {
        let schema = build_schema(&[], 7, &[]);
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
            let schema = build_schema(&[], v, &[]);
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
        let schema = build_schema(&specs, 3, &[]);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(names.contains(&"ASHP Heater Mode (-)"));
        assert!(names.contains(&"ASHP Heater Setpoint (C)"));
        assert!(names.contains(&"Battery Mode (-)"));
        assert!(names.contains(&"Battery SOC (-)"));
    }

    #[test]
    fn schema_metadata_includes_mode_map() {
        let schema = build_schema(&[], 0, &[]);
        let meta = schema.metadata();
        assert!(meta.contains_key("hares_mode_map"));
        let json: serde_json::Value =
            serde_json::from_str(meta.get("hares_mode_map").unwrap()).unwrap();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), OperatingMode::iter().count());
    }

    #[test]
    fn mode_ordinal_round_trips() {
        assert_eq!(OperatingMode::Off.as_code(), 0.0);
        assert_eq!(OperatingMode::Heating.as_code(), 1.0);
        assert_eq!(OperatingMode::BackupElement.as_code(), 11.0);
        assert_eq!(OperatingMode::On.as_code(), 12.0);
    }

    /// Asserts that `MODE_ORDINALS` has one entry for every `OperatingMode`
    /// variant. Adding a new variant without updating `MODE_ORDINALS` causes
    /// this test to fail, guarding against future drift.
    ///
    /// Uses `strum::EnumIter` to enumerate variants directly from the enum
    /// definition. There is no manually-maintained parallel list that could
    /// drift out of sync.
    #[test]
    fn test_mode_ordinals_covers_all_variants() {
        assert_eq!(
            MODE_ORDINALS.len(),
            OperatingMode::iter().count(),
            "MODE_ORDINALS entry count must match OperatingMode variant count"
        );
        for mode in OperatingMode::iter() {
            let ordinal = mode as u8;
            let name = format!("{mode:?}");
            let found = MODE_ORDINALS
                .iter()
                .any(|(n, o)| *o == ordinal && *n == name);
            assert!(
                found,
                "MODE_ORDINALS missing entry for {name} (ordinal {ordinal})"
            );
        }
    }

    /// `OperatingMode::On` round-trips through `as_code()` and the metadata
    /// entry `("On", 12)` exists in `MODE_ORDINALS`.
    #[test]
    fn test_on_mode_roundtrips() {
        assert_eq!(OperatingMode::On.as_code(), 12.0);
        let entry = MODE_ORDINALS
            .iter()
            .find(|(name, _)| *name == "On")
            .expect("On entry must exist in MODE_ORDINALS");
        assert_eq!(entry, &("On", 12));
    }

    #[test]
    fn expected_columns_at_verbosity_0_is_subset_of_schema() {
        let schema = build_schema(&[], 0, &[]);
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
        let cool = build_schema(&[make_spec("Air Conditioner", FuelType::Electric)], 7, &[]);
        let heat = build_schema(&[make_spec("Gas Furnace", FuelType::Gas)], 7, &[]);
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
        let hvac = build_schema(&[make_spec("Gas Furnace", FuelType::Gas)], 7, &[]);
        let non = build_schema(&[make_spec("Battery", FuelType::Electric)], 7, &[]);
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
        let s7 = build_schema(&specs, 7, &[]);
        let s8 = build_schema(&specs, 8, &[]);
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
        let schema = build_schema(&[], 5, &[]);
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
        let s2 = build_schema(&[], 2, &no_attic);
        let s6 = build_schema(&[], 6, &no_attic);
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
        let s2 = build_schema(&[], 2, &with_attic);
        let s6 = build_schema(&[], 6, &with_attic);
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
        let s2 = build_schema(&[], 2, &no_basement);
        let s6 = build_schema(&[], 6, &no_basement);
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
        let s2 = build_schema(&[], 2, &with_basement);
        let s6 = build_schema(&[], 6, &with_basement);
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
            let schema = build_schema(&[], v, &zone_names);
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
        let schema = build_schema(&[], 6, &zone_names);
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
        let schema = build_schema(&[], 6, &zone_names);
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
            let schema = build_schema(&[], v, &zone_names);
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
        let schema = build_schema(&[], 6, &zone_names);
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
        let schema = build_schema(&specs, 1, &[]);
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
        let s0 = build_schema(&specs, 0, &[]);
        let s1 = build_schema(&specs, 1, &[]);
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
        let schema = build_schema(&specs, 1, &[]);
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
        let schema = build_schema(&specs, 1, &[]);
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
        let schema = build_schema(&specs, 8, &[]);
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
        let schema = build_schema(&specs, 8, &[]);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(names.contains(&"PV DC Power (kW)"));
        assert!(names.contains(&"PV Irradiance (W/m2)"));
    }

    /// EV equipment gets diagnostic columns at v8.
    #[test]
    fn ev_equipment_gets_v8_telemetry_columns() {
        let specs = vec![make_spec("EV", FuelType::Electric)];
        let schema = build_schema(&specs, 8, &[]);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(names.contains(&"EV Connection State (-)"));
        assert!(names.contains(&"EV Charging Level (-)"));
    }

    /// Non-HVAC/PV/EV equipment does NOT get v8 telemetry columns.
    #[test]
    fn non_diagnostic_equipment_has_no_v8_telemetry_columns() {
        let specs = vec![make_spec("Battery", FuelType::Electric)];
        let s7 = build_schema(&specs, 7, &[]);
        let s8 = build_schema(&specs, 8, &[]);
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
}
