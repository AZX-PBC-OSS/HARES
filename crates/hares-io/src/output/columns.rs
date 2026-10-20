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
    format!("{} End Use Electric Power (kW)", end_use.display_name())
}

/// If `name` is an end-use aggregate electric power column, returns the
/// canonical EndUse key string (e.g. `"hvac_heating"`) it represents.
/// Returns `None` for per-equipment columns, totals, and other column
/// types.
///
/// Recognised format: `"{DisplayName} End Use Electric Power (kW)"`, where
/// the display name is the typed end use's own
/// [`EndUse::display_name`]; the naming scheme is the aggregate's, not a
/// classifier.
pub fn parse_end_use_electric_power_column_key(name: &str) -> Option<String> {
    let display = name.strip_suffix(END_USE_AGGREGATE_SUFFIX)?;
    EndUse::from_display(display).map(|end_use| end_use.as_str().to_string())
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
    LEVEL_0_COLUMNS.contains(&name) || parse_end_use_electric_power_column_key(name).is_some()
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
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
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

    /// `parse_end_use_electric_power_column_key` round-trips every
    /// standard end use's aggregate column and rejects per-equipment and
    /// total columns.
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
            let parsed = parse_end_use_electric_power_column_key(&col);
            assert_eq!(
                parsed.as_deref(),
                Some(end_use.as_str()),
                "round-trip failed for {end_use:?}: column='{col}', parsed={parsed:?}"
            );
        }
        // Per-equipment columns should not parse as end-use aggregate.
        assert_eq!(
            parse_end_use_electric_power_column_key("Battery Electric Power (kW)"),
            None
        );
        assert_eq!(
            parse_end_use_electric_power_column_key("ASHP Heater Electric Power (kW)"),
            None
        );
        assert_eq!(
            parse_end_use_electric_power_column_key("Total Electric Power (kW)"),
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

    /// EV equipment gets diagnostic columns at v8.
    #[test]
    fn ev_equipment_gets_v8_telemetry_columns() {
        let specs = vec![make_spec("EV", FuelType::Electric)];
        let schema = build_schema_for_specs(&specs, 8, &[]);
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(names.contains(&"EV Connection State (-)"));
        assert!(names.contains(&"EV Charging Level (-)"));
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
