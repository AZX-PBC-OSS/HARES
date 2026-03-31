//! Output column definitions and verbosity-level schema construction.
//!
//! Column naming follows OCHRE conventions: `"{Name} {Metric} ({Unit})"`.
//! Multi-instance equipment includes an instance qualifier: `"Battery #1 SOC (-)"`.

use std::sync::Arc;

use arrow::datatypes::{DataType, Field, Schema};

use crate::hpxml::EquipmentSpec;
use hares_types::{FuelType, OperatingMode};

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

/// Equipment state column suffixes for verbosity 3.
const MODE_SUFFIX: &str = "Mode (-)";
const SETPOINT_SUFFIX: &str = "Setpoint (C)";
const SOC_SUFFIX: &str = "SOC (-)";

/// Energy column suffix for verbosity 4.
const ENERGY_SUFFIX: &str = "Energy (kWh)";

/// Reactive power column suffixes for verbosity 5.
const REACTIVE_POWER_SUFFIX: &str = "Reactive Power (kVAR)";
const POWER_FACTOR_SUFFIX: &str = "Power Factor (-)";

/// Builds an Arrow schema for the output based on equipment list and verbosity.
///
/// The schema always includes a timestamp column, followed by columns
/// appropriate for the requested verbosity level.
pub fn build_schema(equipment_list: &[EquipmentSpec], verbosity: u8) -> Schema {
    let mut fields = vec![Field::new(TIMESTAMP_COL, DataType::Utf8, false)];

    // Level 0: total power
    for &col in LEVEL_0_COLUMNS {
        fields.push(Field::new(col, DataType::Float64, true));
    }

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
        // Zone columns — we always include at least one conditioned zone.
        // At schema-build time we don't know exact zone count, so emit the
        // common indoor + attic temperature channels used by parity/oracle
        // paths. Dwelling record logic writes whichever zones exist.
        fields.push(Field::new(
            format!("{ZONE_TEMP_PREFIX} Indoor {ZONE_TEMP_UNIT}"),
            DataType::Float64,
            true,
        ));
        fields.push(Field::new(
            format!("{ZONE_TEMP_PREFIX} Attic {ZONE_TEMP_UNIT}"),
            DataType::Float64,
            true,
        ));
        fields.push(Field::new(ZONE_UNMET_LOAD_COL, DataType::Float64, true));
        fields.push(Field::new(OUTDOOR_TEMP_COL, DataType::Float64, true));
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
            "Radiation Heat Gain - Indoor (W)",
            "Opaque Surface Heat Gain - Indoor (W)",
            "Duct Loss Heat Gain - Indoor (W)",
            "Roof Heat Gain - Indoor (W)",
            "Floor Heat Gain - Indoor (W)",
            "Wall Heat Gain - Indoor (W)",
            "Window Heat Gain - Indoor (W)",
            "Internal Mass Heat Gain - Indoor (W)",
            // Attic zone envelope breakdown (multi-zone buildings).
            "Infiltration Heat Gain - Attic (W)",
            "Radiation Heat Gain - Attic (W)",
        ] {
            fields.push(Field::new(*label, DataType::Float64, true));
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
        }
    }

    if verbosity >= 8 {
        // Level 8: all individual equipment state variables.
        for (name, _fuel) in &names {
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

    let mut metadata = std::collections::HashMap::new();
    metadata.insert("hares_verbosity".to_string(), verbosity.to_string());
    metadata.insert("hares_mode_map".to_string(), mode_ordinal_json());

    Schema::new_with_metadata(fields, metadata)
}

/// Returns column names that should be present at a given verbosity level
/// for use in tests. These are the static (equipment-independent) columns.
pub fn expected_columns_at_verbosity(verbosity: u8) -> Vec<&'static str> {
    let mut cols = vec![TIMESTAMP_COL];
    cols.extend_from_slice(LEVEL_0_COLUMNS);

    if verbosity >= 2 {
        cols.push("Temperature - Indoor (C)");
        cols.push("Temperature - Attic (C)");
        cols.push(ZONE_UNMET_LOAD_COL);
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

/// Returns a new schema containing only fields present at the given
/// verbosity level, by filtering `full_schema` against the field list
/// that `build_schema` would produce.
#[allow(dead_code)] // Used when filtering schemas by verbosity level
pub(crate) fn filter_schema_to_verbosity(full_schema: &Schema, verbosity: u8) -> Schema {
    let reference = build_schema(&[], verbosity);
    let ref_names: std::collections::HashSet<&str> = reference
        .fields()
        .iter()
        .map(|f| f.name().as_str())
        .collect();

    let fields: Vec<Arc<Field>> = full_schema
        .fields()
        .iter()
        .filter(|f| ref_names.contains(f.name().as_str()))
        .cloned()
        .collect();

    Schema::new_with_metadata(fields, full_schema.metadata().clone())
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
    fn verbosity_0_has_timestamp_and_total_power() {
        let schema = build_schema(&[], 0);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(
            names,
            vec![
                "Time",
                "Total Electric Power (kW)",
                "Total Gas Power (therms/hour)",
                "Total Reactive Power (kVAR)",
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
        let schema = build_schema(&specs, 1);
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
        let schema = build_schema(&specs, 1);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(names.contains(&"Battery #1 Electric Power (kW)"));
        assert!(names.contains(&"Battery #2 Electric Power (kW)"));
    }

    #[test]
    fn verbosity_2_adds_zone_and_outdoor_temp_columns() {
        let schema = build_schema(&[], 2);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(names.contains(&"Temperature - Indoor (C)"));
        assert!(names.contains(&"Unmet HVAC Load (C)"));
        assert!(names.contains(&"Outdoor Dry Bulb (C)"));
    }

    #[test]
    fn verbosity_0_and_1_exclude_outdoor_temp() {
        for v in [0, 1] {
            let schema = build_schema(&[], v);
            let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
            assert!(
                !names.contains(&"Outdoor Dry Bulb (C)"),
                "verbosity {v} should not include outdoor temp"
            );
        }
    }

    #[test]
    fn verbosity_3_adds_mode_and_soc() {
        let specs = vec![
            make_spec("ASHP Heater", FuelType::Electric),
            make_spec("Battery", FuelType::Electric),
        ];
        let schema = build_schema(&specs, 3);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(names.contains(&"ASHP Heater Mode (-)"));
        assert!(names.contains(&"ASHP Heater Setpoint (C)"));
        assert!(names.contains(&"Battery Mode (-)"));
        assert!(names.contains(&"Battery SOC (-)"));
    }

    #[test]
    fn schema_metadata_includes_mode_map() {
        let schema = build_schema(&[], 0);
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
        let schema = build_schema(&[], 0);
        let schema_names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        for col in expected_columns_at_verbosity(0) {
            assert!(
                schema_names.contains(&col),
                "expected column '{col}' not in schema"
            );
        }
    }
}
