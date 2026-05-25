//! Output column definitions and verbosity-level schema construction.
//!
//! Column naming follows OCHRE conventions: `"{Name} {Metric} ({Unit})"`.
//! Multi-instance equipment includes an instance qualifier: `"Battery #1 SOC (-)"`.

use arrow::datatypes::{DataType, Field, Schema};

use crate::hpxml::EquipmentSpec;
use hares_types::{FuelType, OperatingMode, ZoneId};

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
const ELECTRIC_POWER_SUFFIX: &str = "Electric Power (kW)";
const GAS_POWER_SUFFIX: &str = "Gas Power (therms/hour)";

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
const MODE_SUFFIX: &str = "Mode (-)";
const SETPOINT_SUFFIX: &str = "Setpoint (C)";
const SOC_SUFFIX: &str = "SOC (-)";

/// Energy column suffix for verbosity 4.
const ENERGY_SUFFIX: &str = "Energy (kWh)";

/// Reactive power column suffixes for verbosity 5.
const REACTIVE_POWER_SUFFIX: &str = "Reactive Power (kVAR)";
const POWER_FACTOR_SUFFIX: &str = "Power Factor (-)";

/// Defrost state column suffix for verbosity 7 (heat pump heaters only).
const DEFROST_STATE_SUFFIX: &str = "Defrost State (-)";

/// Duct losses column for verbosity 5.
const HVAC_DUCT_LOSSES_COL: &str = "HVAC Duct Losses (W)";

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
    }

    if verbosity >= 2 {
        fields.push(Field::new(
            format!("{ZONE_TEMP_PREFIX} Attic {ZONE_TEMP_UNIT}"),
            DataType::Float64,
            true,
        ));
        fields.push(Field::new(ZONE_UNMET_LOAD_COL, DataType::Float64, true));
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
        // Envelope component load columns for Indoor zone.
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
            // Attic zone envelope breakdown (multi-zone buildings).
            "Infiltration Heat Gain - Attic (W)",
            "Interior LWR Exchange - Attic (W)",
        ] {
            fields.push(Field::new(*label, DataType::Float64, true));
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
                format!("{name} Schedule (-)"),
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
                    format!("{name} ER Power (kW)"),
                    DataType::Float64,
                    true,
                ));
            }
            // OCHRE HVAC.py:590-599: per-equipment HVAC performance columns at v7.
            if is_hvac_or_wh(name) {
                // SHR and Latent Gains are cooling-only (OCHRE HVAC.py:595-596).
                if is_cooling_equipment(name) {
                    fields.push(Field::new(
                        format!("{name} SHR (-)"),
                        DataType::Float64,
                        true,
                    ));
                    fields.push(Field::new(
                        format!("{name} Latent Gains (W)"),
                        DataType::Float64,
                        true,
                    ));
                }
                fields.push(Field::new(
                    format!("{name} Speed (-)"),
                    DataType::Float64,
                    true,
                ));
                // OCHRE HVAC.py:575: main_power = total_input - fan.
                fields.push(Field::new(
                    format!("{name} Main Power (kW)"),
                    DataType::Float64,
                    true,
                ));
                fields.push(Field::new(
                    format!("{name} Fan Power (kW)"),
                    DataType::Float64,
                    true,
                ));
                fields.push(Field::new(
                    format!("{name} Runtime Fraction (-)"),
                    DataType::Float64,
                    true,
                ));
                // Promoted from v8: OCHRE emits Capacity and COP at v7 (HVAC.py:584,598).
                fields.push(Field::new(
                    format!("{name} Capacity (W)"),
                    DataType::Float64,
                    true,
                ));
                fields.push(Field::new(
                    format!("{name} COP (-)"),
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
    }

    if verbosity >= 8 {
        // Level 8: all individual equipment state variables.
        // (Capacity and COP were promoted to v7 to match OCHRE HVAC.py:584,598.)
    }

    let mut metadata = std::collections::HashMap::new();
    metadata.insert("hares_verbosity".to_string(), verbosity.to_string());
    metadata.insert("hares_mode_map".to_string(), mode_ordinal_json());

    Schema::new_with_metadata(fields, metadata)
}

/// Returns column names that should be present at a given verbosity level
/// for use in tests. These are the static (equipment-independent) columns only;
/// per-equipment columns (power, mode, SOC, energy, schedule, etc.) are
/// generated dynamically by `build_schema` based on the equipment list and
/// are not covered here.
pub fn expected_columns_at_verbosity(verbosity: u8) -> Vec<&'static str> {
    let mut cols = vec![TIMESTAMP_COL];
    cols.extend_from_slice(LEVEL_0_COLUMNS);
    cols.push(OUTDOOR_TEMP_COL);
    cols.push("Temperature - Indoor (C)");

    if verbosity >= 2 {
        cols.push("Temperature - Attic (C)");
        cols.push(ZONE_UNMET_LOAD_COL);
        cols.push(GROUND_TEMP_COL);
    }

    if verbosity >= 5 {
        cols.push(NET_SENSIBLE_HEAT_GAIN_COL);
        cols.push(HVAC_DUCT_LOSSES_COL);
    }

    if verbosity >= 7 {
        cols.push(HOT_WATER_MAINS_TEMP_COL);
    }

    cols
}

/// JSON encoding of the `OperatingMode` → ordinal mapping, embedded as
/// Parquet custom metadata so output files are self-describing.
///
/// Ordinal values:
///   Off=0, Heating=1, Cooling=2, Defrost=3, Standby=4, Charging=5,
///   Discharging=6, HeatingHP=7, HeatingER=8, HeatingHPAndER=9,
///   HeatPumpWH=10, BackupElement=11
fn mode_ordinal_json() -> String {
    serde_json::to_string(&MODE_ORDINALS).expect("static map serialises")
}

/// Returns the integer ordinal for a given `OperatingMode`.
pub fn mode_to_ordinal(mode: OperatingMode) -> f64 {
    match mode {
        OperatingMode::Off => 0.0,
        OperatingMode::Heating => 1.0,
        OperatingMode::Cooling => 2.0,
        OperatingMode::Defrost => 3.0,
        OperatingMode::Standby => 4.0,
        OperatingMode::Charging => 5.0,
        OperatingMode::Discharging => 6.0,
        OperatingMode::HeatingHP => 7.0,
        OperatingMode::HeatingER => 8.0,
        OperatingMode::HeatingHPAndER => 9.0,
        OperatingMode::HeatPumpWH => 10.0,
        OperatingMode::BackupElement => 11.0,
        OperatingMode::On => 12.0,
    }
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
];

/// Generates instance-qualified names for multi-instance equipment.
///
/// If an equipment name appears more than once, instances are numbered:
/// `"Battery #1"`, `"Battery #2"`. Single instances keep the bare name.
fn instance_qualified_names(specs: &[EquipmentSpec]) -> Vec<(String, FuelType)> {
    use std::collections::HashMap;

    // Count occurrences of each name.
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for spec in specs {
        *counts.entry(&spec.name).or_insert(0) += 1;
    }

    // Assign instance numbers.
    let mut indices: HashMap<&str, usize> = HashMap::new();
    let mut result = Vec::with_capacity(specs.len());
    for spec in specs {
        let total = counts[spec.name.as_str()];
        if total > 1 {
            let idx = indices.entry(&spec.name).or_insert(0);
            *idx += 1;
            result.push((format!("{} #{}", spec.name, *idx), spec.fuel_type));
        } else {
            result.push((spec.name.clone(), spec.fuel_type));
        }
    }
    result
}

fn is_hvac_or_wh(name: &str) -> bool {
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

fn has_soc(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("battery")
        || lower.contains("electric vehicle")
        || lower == "ev"
        || lower.starts_with("ev ")
        || lower.starts_with("ev#")
}

fn is_heat_pump_heater(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("ashp heater")
        || lower.contains("mshp heater")
        || lower.contains("heat pump heater")
}

/// Returns true if the equipment name indicates cooling-only equipment
/// that should emit SHR and Latent Gains columns at v7.
/// Heat-pump heaters that also cool are excluded here — the cooler
/// companion emits those columns under its own name.
fn is_cooling_equipment(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("air conditioner")
        || lower.contains("room ac")
        || lower.contains("cooler")
        || lower.contains("dehumidifier")
}

#[cfg(test)]
mod tests {
    use hares_types::FuelType;
    use serde_json::Map;

    use super::*;
    use crate::hpxml::EquipmentSpec;

    fn make_spec(name: &str, fuel: FuelType) -> EquipmentSpec {
        EquipmentSpec {
            name: name.to_string(),
            fuel_type: fuel,
            parameters: Map::new(),
            zip_params: None,
            typed_config: None,
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
        let expected = expected_columns_at_verbosity(0);
        assert!(expected.contains(&"Total Electric Power (kW)"));
        assert!(expected.contains(&"Total Gas Power (therms/hour)"));
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
    fn verbosity_2_adds_attic_and_ground_temp_columns() {
        let schema = build_schema(&[], 2, &[]);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(names.contains(&"Temperature - Attic (C)"));
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
        // Should contain all 12 mode entries
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 12);
    }

    #[test]
    fn mode_ordinal_round_trips() {
        assert_eq!(mode_to_ordinal(OperatingMode::Off), 0.0);
        assert_eq!(mode_to_ordinal(OperatingMode::Heating), 1.0);
        assert_eq!(mode_to_ordinal(OperatingMode::BackupElement), 11.0);
    }

    #[test]
    fn expected_columns_at_verbosity_0_is_subset_of_schema() {
        let schema = build_schema(&[], 0, &[]);
        let schema_names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        for col in expected_columns_at_verbosity(0) {
            assert!(
                schema_names.contains(&col),
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
    #[test]
    fn capacity_and_cop_promoted_to_v7_not_duplicated_at_v8() {
        let specs = vec![make_spec("Air Conditioner", FuelType::Electric)];
        let s7 = build_schema(&specs, 7, &[]);
        let s8 = build_schema(&specs, 8, &[]);
        let n7: Vec<&str> = s7.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(n7.contains(&"Air Conditioner Capacity (W)"));
        assert!(n7.contains(&"Air Conditioner COP (-)"));
        // v8 must be identical in length to v7 (Capacity and COP are not re-added at v8).
        assert_eq!(s8.fields().len(), s7.fields().len());
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
}
