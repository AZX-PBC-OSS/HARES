//! Resolve index-based schedule CSV columns into per-equipment kW schedules.
//!
//! Mirrors OCHRE's `SCHEDULE_NAMES` mapping and `convert_power_column` logic:
//! normalized schedule fractions are scaled to kW using
//! `max_kw = annual_kwh / 8760 / mean(fraction)`.

use std::collections::HashMap;
use std::path::Path;

use hares_equipment::{
    ConfigPayload, ElectricResistanceWaterHeaterConfig, GasWaterHeaterConfig,
    HeatPumpWaterHeaterConfig, TanklessWaterHeaterConfig,
};
use hares_physics::constants::HOURS_PER_YEAR;
use hares_types::{BoundaryPolicy, ScheduleSourceConfig, normalize_ascii, parse_trimmed_f64};
use serde_json::{Map, Value};
use tracing::warn;

use crate::EquipmentSpec;
use crate::draw_profile::normalize_draw_profile;
use crate::schedule::{ColumnAggregation, ScheduleTimeSeries};

// HERS Reference Home default thermostat setpoints (ASHRAE 90.2).
pub(super) const HERS_HEATING_SETPOINT_C: f64 = 20.0;
pub(super) const HERS_COOLING_SETPOINT_C: f64 = 24.0;

/// Maps HPXML/ResStock schedule CSV column names (lowercase, normalized) to
/// OCHRE equipment names.  The category determines how to convert:
/// - `Power`:      fraction → kW via annual electric energy
/// - `EventWindow`: fraction → event window (for wet appliances)
/// - `Setpoint`:   raw value (already in schedule units, typically °F → °C)
/// - `Ignore`:     skip
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScheduleCategory {
    Power,
    EventWindow,
    Setpoint,
    Occupancy,
    Ignore,
}

struct ColumnMapping {
    csv_column: &'static str,
    equipment_name: &'static str,
    category: ScheduleCategory,
}

const COLUMN_MAPPINGS: &[ColumnMapping] = &[
    // Occupancy
    ColumnMapping {
        csv_column: "occupants",
        equipment_name: "Occupancy",
        category: ScheduleCategory::Occupancy,
    },
    // Event-based (wet appliances)
    ColumnMapping {
        csv_column: "clothes_washer",
        equipment_name: "Clothes Washer",
        category: ScheduleCategory::EventWindow,
    },
    ColumnMapping {
        csv_column: "clothes_dryer",
        equipment_name: "Clothes Dryer",
        category: ScheduleCategory::EventWindow,
    },
    ColumnMapping {
        csv_column: "dishwasher",
        equipment_name: "Dishwasher",
        category: ScheduleCategory::EventWindow,
    },
    // Power (appliances)
    ColumnMapping {
        csv_column: "refrigerator",
        equipment_name: "Refrigerator",
        category: ScheduleCategory::Power,
    },
    ColumnMapping {
        csv_column: "freezer",
        equipment_name: "Freezer",
        category: ScheduleCategory::Power,
    },
    ColumnMapping {
        csv_column: "cooking_range",
        equipment_name: "Cooking Range",
        category: ScheduleCategory::EventWindow,
    },
    // Power (lighting)
    ColumnMapping {
        csv_column: "lighting_interior",
        equipment_name: "Indoor Lighting",
        category: ScheduleCategory::Power,
    },
    ColumnMapping {
        csv_column: "lighting_exterior",
        equipment_name: "Exterior Lighting",
        category: ScheduleCategory::Power,
    },
    ColumnMapping {
        csv_column: "lighting_basement",
        equipment_name: "Basement Lighting",
        category: ScheduleCategory::Power,
    },
    ColumnMapping {
        csv_column: "lighting_garage",
        equipment_name: "Garage Lighting",
        category: ScheduleCategory::Power,
    },
    // Power (plug loads)
    ColumnMapping {
        csv_column: "plug_loads_other",
        equipment_name: "MELs",
        category: ScheduleCategory::Power,
    },
    ColumnMapping {
        csv_column: "plug_loads_tv",
        equipment_name: "TV",
        category: ScheduleCategory::Power,
    },
    ColumnMapping {
        csv_column: "plug_loads_well_pump",
        equipment_name: "Well Pump",
        category: ScheduleCategory::Power,
    },
    // Power (misc)
    ColumnMapping {
        csv_column: "ceiling_fan",
        equipment_name: "Ceiling Fan",
        category: ScheduleCategory::Power,
    },
    ColumnMapping {
        csv_column: "pool_pump",
        equipment_name: "Pool Pump",
        category: ScheduleCategory::Power,
    },
    ColumnMapping {
        csv_column: "pool_heater",
        equipment_name: "Pool Heater",
        category: ScheduleCategory::Power,
    },
    ColumnMapping {
        csv_column: "permanent_spa_pump",
        equipment_name: "Spa Pump",
        category: ScheduleCategory::Power,
    },
    ColumnMapping {
        csv_column: "permanent_spa_heater",
        equipment_name: "Spa Heater",
        category: ScheduleCategory::Power,
    },
    ColumnMapping {
        csv_column: "fuel_loads_grill",
        equipment_name: "Gas Grill",
        category: ScheduleCategory::Power,
    },
    ColumnMapping {
        csv_column: "fuel_loads_fireplace",
        equipment_name: "Gas Fireplace",
        category: ScheduleCategory::Power,
    },
    ColumnMapping {
        csv_column: "fuel_loads_lighting",
        equipment_name: "Gas Lighting",
        category: ScheduleCategory::Power,
    },
    // Setpoints
    ColumnMapping {
        csv_column: "heating_setpoint",
        equipment_name: "HVAC Heating",
        category: ScheduleCategory::Setpoint,
    },
    ColumnMapping {
        csv_column: "cooling_setpoint",
        equipment_name: "HVAC Cooling",
        category: ScheduleCategory::Setpoint,
    },
    ColumnMapping {
        csv_column: "water_heater_setpoint",
        equipment_name: "Water Heating",
        category: ScheduleCategory::Setpoint,
    },
    // Ignored
    ColumnMapping {
        csv_column: "extra_refrigerator",
        equipment_name: "",
        category: ScheduleCategory::Ignore,
    },
    ColumnMapping {
        csv_column: "clothes_dryer_exhaust",
        equipment_name: "",
        category: ScheduleCategory::Ignore,
    },
    ColumnMapping {
        csv_column: "lighting_exterior_holiday",
        equipment_name: "",
        category: ScheduleCategory::Ignore,
    },
    ColumnMapping {
        csv_column: "plug_loads_vehicle",
        equipment_name: "",
        category: ScheduleCategory::Ignore,
    },
    ColumnMapping {
        csv_column: "battery",
        equipment_name: "",
        category: ScheduleCategory::Ignore,
    },
    ColumnMapping {
        csv_column: "vacancy",
        equipment_name: "",
        category: ScheduleCategory::Ignore,
    },
    ColumnMapping {
        csv_column: "water_heater_operating_mode",
        equipment_name: "",
        category: ScheduleCategory::Ignore,
    },
    ColumnMapping {
        csv_column: "power_outage",
        equipment_name: "",
        category: ScheduleCategory::Ignore,
    },
    ColumnMapping {
        csv_column: "no_space_heating",
        equipment_name: "",
        category: ScheduleCategory::Ignore,
    },
    ColumnMapping {
        csv_column: "no_space_cooling",
        equipment_name: "",
        category: ScheduleCategory::Ignore,
    },
];

// ---------------------------------------------------------------------------
// Default schedule profiles (weekday/weekend fractions + monthly multipliers)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct DefaultScheduleProfile {
    pub(crate) weekday_fractions: [f64; 24],
    pub(crate) weekend_fractions: [f64; 24],
    pub(crate) month_multipliers: [f64; 12],
}

/// Load default schedule profiles from `Default Schedule Parameters.csv`.
/// Returns a map keyed by "OCHRE Name" (e.g. "Indoor Lighting", "MELs").
pub(crate) fn load_default_profiles(
    defaults_dir: &Path,
) -> HashMap<String, DefaultScheduleProfile> {
    let csv_path = defaults_dir.join("Default Schedule Parameters.csv");
    let content = match std::fs::read_to_string(&csv_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                path = %csv_path.display(),
                error = %e,
                "cannot read defaults CSV; default schedule profiles will be empty"
            );
            return HashMap::new();
        }
    };

    // Intermediate: collect raw vectors per (ochre_name, element_kind)
    let mut weekday_map: HashMap<String, [f64; 24]> = HashMap::new();
    let mut weekend_map: HashMap<String, [f64; 24]> = HashMap::new();
    let mut month_map: HashMap<String, [f64; 12]> = HashMap::new();

    for line in content.lines().skip(1) {
        // Parse CSV line handling quoted "Values" field
        let fields = parse_csv_line(line);
        if fields.len() < 5 {
            continue;
        }

        let ochre_name = fields[2].trim();
        let ochre_element = fields[3].trim();
        let values_str = fields[4].trim();

        if ochre_name.is_empty() || ochre_name == "N/A" {
            continue;
        }

        let values: Vec<f64> = values_str
            .split(',')
            .filter_map(parse_trimmed_f64)
            .collect();

        match ochre_element {
            "weekday_fractions" if values.len() == 24 => {
                let mut arr = [0.0; 24];
                arr.copy_from_slice(&values);
                weekday_map.insert(ochre_name.to_string(), arr);
            }
            "weekend_fractions" if values.len() == 24 => {
                let mut arr = [0.0; 24];
                arr.copy_from_slice(&values);
                weekend_map.insert(ochre_name.to_string(), arr);
            }
            "month_multipliers" if values.len() == 12 => {
                let mut arr = [0.0; 12];
                arr.copy_from_slice(&values);
                month_map.insert(ochre_name.to_string(), arr);
            }
            _ => {}
        }
    }

    // Assemble profiles for each equipment name that has at least weekday fractions
    let mut profiles = HashMap::new();
    for (name, weekday) in &weekday_map {
        let weekend = weekend_map.get(name).copied().unwrap_or(*weekday);
        let months = month_map.get(name).copied().unwrap_or([1.0; 12]);
        profiles.insert(
            name.clone(),
            DefaultScheduleProfile {
                weekday_fractions: *weekday,
                weekend_fractions: weekend,
                month_multipliers: months,
            },
        );
    }

    // Observer capture: log weekday and weekend fractions for each schedule type
    // at simulation startup so users can visually verify distinct patterns.
    #[cfg(feature = "observe")]
    for (name, profile) in &profiles {
        tracing::info!(
            schedule_type = %name,
            weekday_fractions = ?profile.weekday_fractions,
            weekend_fractions = ?profile.weekend_fractions,
            weekday_eq_weekend = profile.weekday_fractions == profile.weekend_fractions,
            "default schedule profile loaded",
        );
    }

    // Invariant: at least the 'Occupancy' schedule must have non-identical
    // weekday and weekend fraction arrays. Identical arrays mean the data
    // was imported verbatim from the ANSI 301 source without weekend derivation.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    {
        if let Some(occ) = profiles.get("Occupancy") {
            if occ.weekday_fractions == occ.weekend_fractions {
                tracing::error!(
                    "Occupancy weekday and weekend schedule fractions are identical; \
                     the default CSV has not been updated with distinct weekend patterns. \
                     ASHRAE 90.2/HERS Reference Home requires distinct weekday/weekend occupancy."
                );
            }
        }
    }

    profiles
}

/// Parse a single CSV line, respecting double-quoted fields.
fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for ch in line.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                fields.push(current.clone());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    fields.push(current);
    fields
}

/// Compute the annual mean of a schedule profile analytically.
///
/// This must be used instead of averaging the (potentially short) simulation
/// window, because `max_kw = annual_mean_kw / annual_mean_fraction` requires
/// the full-year mean regardless of simulation duration.
///
/// Monthly multipliers are weighted by days-per-month (non-leap year) to
/// correctly account for months of different length.
fn annual_mean_fraction(profile: &DefaultScheduleProfile) -> f64 {
    const DAYS_PER_MONTH: [f64; 12] = [
        31.0, 28.0, 31.0, 30.0, 31.0, 30.0, 31.0, 31.0, 30.0, 31.0, 30.0, 31.0,
    ];
    let weekday_mean: f64 = profile.weekday_fractions.iter().sum::<f64>() / 24.0;
    let weekend_mean: f64 = profile.weekend_fractions.iter().sum::<f64>() / 24.0;
    let hourly_mean = (5.0 * weekday_mean + 2.0 * weekend_mean) / 7.0;
    let total_days: f64 = DAYS_PER_MONTH.iter().sum();
    let month_mean: f64 = profile
        .month_multipliers
        .iter()
        .zip(DAYS_PER_MONTH.iter())
        .map(|(m, d)| m * d)
        .sum::<f64>()
        / total_days;
    hourly_mean * month_mean
}

/// Inject resolved power/event schedule metadata into equipment specs.
pub fn inject_schedule_into_specs(
    specs: &mut [EquipmentSpec],
    schedule: &mut ScheduleTimeSeries,
    defaults_path: Option<&Path>,
) {
    let csv_col_map: HashMap<String, usize> = schedule
        .column_names
        .iter()
        .enumerate()
        .map(|(i, name)| (name.clone(), i))
        .collect();

    let profiles = defaults_path.map(load_default_profiles).unwrap_or_default();

    let mapping_by_equipment: HashMap<&str, &ColumnMapping> = COLUMN_MAPPINGS
        .iter()
        .filter(|m| {
            matches!(
                m.category,
                ScheduleCategory::Power | ScheduleCategory::EventWindow
            )
        })
        .map(|m| (m.equipment_name, m))
        .collect();

    // Invariant: if HPXML-derived pool/spa specs have annual energy but the
    // schedule CSV also has pool/spa columns, the CSV fractions will override
    // the HPXML extension fractions (while HPXML annual energy is used for
    // max_kW scaling). This is an intentional precedence, but the combination
    // may produce unexpected results. Warn when both sources are present.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    {
        let pool_source = ["Pool Pump", "Pool Heater", "Spa Pump", "Spa Heater"];
        let csv_to_eq: [(&str, &str); 4] = [
            ("pool_pump", "Pool Pump"),
            ("pool_heater", "Pool Heater"),
            ("permanent_spa_pump", "Spa Pump"),
            ("permanent_spa_heater", "Spa Heater"),
        ];
        for spec in specs.iter() {
            if !pool_source.contains(&spec.name.as_str()) {
                continue;
            }
            for (csv_col, eq_name) in &csv_to_eq {
                if eq_name != &spec.name.as_str() {
                    continue;
                }
                let csv_key = normalize_schedule_col_name(csv_col);
                if csv_col_map.contains_key(&csv_key) {
                    tracing::warn!(
                        equipment = %spec.name,
                        csv_column = csv_col,
                        "HPXML provides '{eq_name}' equipment AND schedule CSV defines \
                         column '{csv_col}': CSV schedule fractions will override \
                         HPXML <extension> fractions while HPXML annual energy drives \
                         max kW. Verify this combination is intentional to avoid \
                         unexpected load profiles."
                    );
                }
            }
        }
    }

    for spec in specs.iter_mut() {
        // Ventilation Fan: constant power from equipment properties, not schedule CSV.
        // Mirrors OCHRE: schedule["Ventilation Fan (kW)"] = equipment["Power (W)"] / 1000
        if spec.name == "Ventilation Fan" {
            inject_constant_power_schedule(spec, schedule.len());
            continue;
        }

        let Some(mapping) = mapping_by_equipment.get(spec.name.as_str()) else {
            continue;
        };

        match mapping.category {
            ScheduleCategory::Power => {
                inject_power_schedule(spec, mapping, &csv_col_map, schedule, &profiles);
            }
            ScheduleCategory::EventWindow => {
                inject_event_schedule(spec, mapping, &csv_col_map, schedule);
            }
            _ => {}
        }
    }

    // Setpoint columns: inject per-timestep arrays from schedule CSV into
    // any heating/cooling HVAC equipment. The CSV column `heating_setpoint`
    // maps to all heating equipment, `cooling_setpoint` to all cooling.
    // When neither a schedule CSV column nor HPXML-derived setpoints are
    // present, falls back to the HERS reference-home default profiles
    // loaded from Default Schedule Parameters.csv.
    inject_setpoint_schedules(specs, &csv_col_map, schedule, &profiles);
}

/// HVAC equipment names that consume heating setpoints.
pub(crate) const HEATING_EQUIPMENT: &[&str] = &[
    "ASHP Heater",
    "MSHP Heater",
    "Gas Furnace",
    "Electric Furnace",
    "Oil Furnace",
    "Electric Baseboard",
    "Gas Boiler",
    "Electric Boiler",
    "Oil Boiler",
];

/// HVAC equipment names that consume cooling setpoints.
pub(crate) const COOLING_EQUIPMENT: &[&str] =
    &["ASHP Cooler", "MSHP Cooler", "Air Conditioner", "Room AC"];

fn spec_has_setpoint_source(spec: &EquipmentSpec, prefix: &str) -> bool {
    let key = format!("{prefix}_setpoint_source");
    spec.typed_config
        .as_ref()
        .and_then(|tc| match &tc.payload {
            ConfigPayload::Typed { data, .. } => Some(data),
            _ => None,
        })
        .and_then(|data| find_setpoint_source(data, &key))
        .is_some()
}

/// Walk a JSON object tree looking for a setpoint key at any nesting level.
fn find_setpoint_source<'a>(data: &'a Value, key: &str) -> Option<&'a Value> {
    let obj = data.as_object()?;
    if let Some(sp) = obj.get("setpoint") {
        if let Some(v) = sp.get(key) {
            return Some(v);
        }
    }
    None
}

fn inject_default_setpoint_profile(
    spec: &mut EquipmentSpec,
    ochre_name: &str,
    prefix: &str,
    profiles: &HashMap<String, DefaultScheduleProfile>,
) {
    let Some(profile) = profiles.get(ochre_name) else {
        return;
    };
    let temp = match prefix {
        "heating" => HERS_HEATING_SETPOINT_C,
        "cooling" => HERS_COOLING_SETPOINT_C,
        _ => return,
    };
    let source = ScheduleSourceConfig::DailyProfile {
        weekday: profile.weekday_fractions.map(|f| f * temp),
        weekend: profile.weekend_fractions.map(|f| f * temp),
        month_multipliers: profile.month_multipliers,
        max_value: 1.0,
    };
    let Some(typed) = spec.typed_config.as_mut() else {
        return;
    };
    let ConfigPayload::Typed { data, .. } = &mut typed.payload else {
        return;
    };
    let Some(obj) = data.as_object_mut() else {
        return;
    };
    if let Ok(json) = serde_json::to_value(source) {
        insert_setpoint_into_obj(obj, &format!("{prefix}_setpoint_source"), json);
    }
}

fn insert_setpoint_into_obj(obj: &mut Map<String, Value>, key: &str, value: Value) {
    if let Some(sp) = obj.get_mut("setpoint") {
        if let Some(sp_obj) = sp.as_object_mut() {
            sp_obj.insert(key.to_string(), value);
        }
    }
}

fn inject_setpoint_schedules(
    specs: &mut [EquipmentSpec],
    csv_col_map: &HashMap<String, usize>,
    schedule: &mut ScheduleTimeSeries,
    profiles: &HashMap<String, DefaultScheduleProfile>,
) {
    // Store only the column index -- the equipment resolves the value each
    // timestep from the environment's schedule domain payload. No materialization.
    let heating_col = csv_col_map.get("heating_setpoint").copied();
    let cooling_col = csv_col_map.get("cooling_setpoint").copied();

    inject_water_heater_schedule_columns(specs, csv_col_map, schedule);

    // Skip the entire loop only when there is no work to do at all.
    if heating_col.is_none() && cooling_col.is_none() && profiles.is_empty() {
        return;
    }

    for spec in specs.iter_mut() {
        if HEATING_EQUIPMENT.contains(&spec.name.as_str()) {
            if let Some(col_idx) = heating_col {
                // Schedule CSV provides a per-timestep heating column.
                set_typed_setpoint_source(spec, "heating", col_idx);
            } else if !spec_has_setpoint_source(spec, "heating") {
                // No HPXML-derived setpoint schedule and no CSV column:
                // fall back to the HERS reference-home default profile.
                inject_default_setpoint_profile(spec, "HVAC Heating", "heating", profiles);
            }
        }
        if COOLING_EQUIPMENT.contains(&spec.name.as_str()) {
            if let Some(col_idx) = cooling_col {
                set_typed_setpoint_source(spec, "cooling", col_idx);
            } else if !spec_has_setpoint_source(spec, "cooling") {
                inject_default_setpoint_profile(spec, "HVAC Cooling", "cooling", profiles);
            }
        }
    }
}

fn set_typed_setpoint_source(spec: &mut EquipmentSpec, prefix: &str, col_idx: usize) {
    let Some(typed) = spec.typed_config.as_mut() else {
        return;
    };
    let ConfigPayload::Typed { data, .. } = &mut typed.payload else {
        return;
    };
    let Some(obj) = data.as_object_mut() else {
        return;
    };

    let source = ScheduleSourceConfig::ColumnRef {
        col_idx,
        boundary: BoundaryPolicy::Clamp,
    };
    if let Ok(json) = serde_json::to_value(source) {
        insert_setpoint_into_obj(obj, &format!("{prefix}_setpoint_source"), json);
    }
}

fn set_typed_schedule_source(spec: &mut EquipmentSpec, field: &str, col_idx: usize) {
    let Some(typed) = spec.typed_config.as_mut() else {
        return;
    };
    let ConfigPayload::Typed { data, .. } = &mut typed.payload else {
        return;
    };
    let Some(obj) = data.as_object_mut() else {
        return;
    };

    let source = ScheduleSourceConfig::ColumnRef {
        col_idx,
        boundary: BoundaryPolicy::Clamp,
    };
    if let Ok(json) = serde_json::to_value(source) {
        obj.insert(field.to_string(), json);
    }
}

/// Storage water heater equipment names that consume draw and mains schedule sources.
const STORAGE_WATER_HEATER_EQUIPMENT: &[&str] = &[
    "Electric Resistance Water Heater",
    "Gas Water Heater",
    "Heat Pump Water Heater",
    "Tankless Water Heater",
    "Gas Tankless Water Heater",
];

fn inject_water_heater_schedule_columns(
    specs: &mut [EquipmentSpec],
    csv_col_map: &HashMap<String, usize>,
    schedule: &mut ScheduleTimeSeries,
) {
    let draw_col = first_present_column(csv_col_map, &["hot_water_fixtures"]);
    let mains_col = first_present_column(csv_col_map, &["hot_water_mains_temperature"]);

    if draw_col.is_none() && mains_col.is_none() {
        return;
    }

    for (i, spec) in specs.iter_mut().enumerate() {
        if !STORAGE_WATER_HEATER_EQUIPMENT.contains(&spec.name.as_str()) {
            continue;
        }
        let has_typed_sources = !matches!(spec.name.as_str(), "Heat Pump Water Heater");
        if let Some(col_idx) = draw_col {
            let raw_values = schedule.columns[col_idx].clone();
            let avg_daily_l = water_heater_avg_daily_draw_l(spec);
            let kg_s_series: Vec<f64> = normalize_draw_profile(&raw_values, avg_daily_l);

            let col_name = format!(
                "hot_water_draw_kg_s_{}_{}",
                normalize_schedule_col_name(&spec.name),
                i
            );
            match schedule.append_derived_column(&col_name, kg_s_series, ColumnAggregation::Mean) {
                Ok(derived_col_idx) => {
                    if has_typed_sources {
                        set_typed_schedule_source(spec, "draw_flow_rate_source", derived_col_idx);
                    }
                }
                Err(err) => {
                    panic!(
                        "failed to append normalized draw column for {}: {err}",
                        spec.name
                    );
                }
            }
        }
        if let Some(col_idx) = mains_col {
            if has_typed_sources {
                set_typed_schedule_source(spec, "mains_temp_c_source", col_idx);
            }
        }
    }
}

fn tankless_avg_daily_draw_l(spec: &EquipmentSpec) -> f64 {
    let typed = spec.typed_config.as_ref().unwrap_or_else(|| {
        panic!(
            "tankless water heater '{}' requires typed config",
            spec.name
        )
    });
    let tankless = typed
        .typed::<TanklessWaterHeaterConfig>()
        .unwrap_or_else(|err| {
            panic!(
                "tankless water heater '{}' typed config failed to decode: {err}",
                spec.name
            )
        });
    tankless.avg_water_draw_l_per_day.unwrap_or_else(|| {
        panic!(
            "tankless water heater '{}' requires typed avg_water_draw_l_per_day to normalize draw schedule fractions",
            spec.name
        )
    })
}

fn water_heater_avg_daily_draw_l(spec: &EquipmentSpec) -> f64 {
    match spec.name.as_str() {
        "Tankless Water Heater" | "Gas Tankless Water Heater" => tankless_avg_daily_draw_l(spec),
        "Electric Resistance Water Heater" => {
            let typed = spec
                .typed_config
                .as_ref()
                .unwrap_or_else(|| panic!("water heater '{}' requires typed config", spec.name));
            let cfg = typed
                .typed::<ElectricResistanceWaterHeaterConfig>()
                .unwrap_or_else(|err| {
                    panic!(
                        "electric resistance water heater '{}' typed config failed to decode: {err}",
                        spec.name
                    )
                });
            cfg.avg_water_draw_l_per_day.unwrap_or_else(|| {
                panic!(
                    "electric resistance water heater '{}' requires typed avg_water_draw_l_per_day",
                    spec.name
                )
            })
        }
        "Gas Water Heater" => {
            let typed = spec
                .typed_config
                .as_ref()
                .unwrap_or_else(|| panic!("water heater '{}' requires typed config", spec.name));
            let cfg = typed.typed::<GasWaterHeaterConfig>().unwrap_or_else(|err| {
                panic!(
                    "gas water heater '{}' typed config failed to decode: {err}",
                    spec.name
                )
            });
            cfg.avg_water_draw_l_per_day.unwrap_or_else(|| {
                panic!(
                    "gas water heater '{}' requires typed avg_water_draw_l_per_day",
                    spec.name
                )
            })
        }
        "Heat Pump Water Heater" => {
            let typed = spec
                .typed_config
                .as_ref()
                .unwrap_or_else(|| panic!("water heater '{}' requires typed config", spec.name));
            let cfg = typed
                .typed::<HeatPumpWaterHeaterConfig>()
                .unwrap_or_else(|err| {
                    panic!(
                        "heat pump water heater '{}' typed config failed to decode: {err}",
                        spec.name
                    )
                });
            cfg.avg_water_draw_l_per_day.unwrap_or_else(|| {
                panic!(
                    "heat pump water heater '{}' requires typed avg_water_draw_l_per_day",
                    spec.name
                )
            })
        }
        other => panic!("unsupported water heater type for draw normalization: {other}"),
    }
}

fn first_present_column(csv_col_map: &HashMap<String, usize>, names: &[&str]) -> Option<usize> {
    names
        .iter()
        .find_map(|name| csv_col_map.get(*name).copied())
}

fn inject_power_schedule(
    spec: &mut EquipmentSpec,
    mapping: &ColumnMapping,
    csv_col_map: &HashMap<String, usize>,
    schedule: &mut ScheduleTimeSeries,
    profiles: &HashMap<String, DefaultScheduleProfile>,
) {
    // Skip if equipment already has power schedule source keys.
    if spec.parameters.keys().any(|k| {
        k.starts_with("power_schedule_")
            || k.starts_with("power_profile_")
            || k == "power_constant_kw"
    }) {
        return;
    }

    let col_name = normalize_schedule_col_name(mapping.csv_column);
    let schedule_len = schedule.len();

    if let Some(&col_idx) = csv_col_map.get(col_name.as_str()) {
        // CSV column exists -- use it directly and add a derived kW column.
        let fraction_series = schedule.columns[col_idx].clone();
        if fraction_series.is_empty() {
            return;
        }

        let mean_fraction: f64 =
            fraction_series.iter().copied().sum::<f64>() / fraction_series.len() as f64;

        let Some(max_kw) = determine_max_kw(spec, mean_fraction) else {
            inject_compact_constant_power(spec, 0.0);
            return;
        };

        let kw_series: Vec<f64> = fraction_series.iter().map(|f| f * max_kw).collect();
        if let Ok(derived_col_idx) = schedule.append_derived_column(
            &format!(
                "power_schedule_kw_{}",
                normalize_schedule_col_name(spec.name.as_str())
            ),
            kw_series,
            ColumnAggregation::Mean,
        ) {
            inject_compact_column_power(spec, derived_col_idx);
        } else {
            tracing::warn!(
                equipment = %spec.name,
                "failed to append derived kW column; falling back to constant 0.0 kW"
            );
            inject_compact_constant_power(spec, 0.0);
        }
    } else if schedule_len > 0 {
        // No CSV column -- prefer building-specific HPXML profile, then generic defaults.
        if let Some(profile) = resolve_hpxml_profile(spec) {
            warn!(
                "schedule_resolve: no CSV column '{}' for '{}'; using HPXML profile fractions",
                col_name, mapping.equipment_name
            );
            let max_kw = determine_max_kw(spec, annual_mean_fraction(&profile)).unwrap_or(0.0);
            inject_compact_profile_power(spec, &profile, max_kw);
        } else if let Some(profile) = profiles.get(mapping.equipment_name) {
            warn!(
                "schedule_resolve: no CSV column '{}' for '{}'; using default profile",
                col_name, mapping.equipment_name
            );
            let max_kw = determine_max_kw(spec, annual_mean_fraction(profile)).unwrap_or(0.0);
            inject_compact_profile_power(spec, profile, max_kw);
        } else {
            let constant_kw = determine_constant_kw(spec).unwrap_or(0.0);
            warn!(
                "schedule_resolve: no CSV column '{}' and no default profile for '{}'; falling back to constant power {:.3} kW -- THIS MAY BE INCORRECT",
                col_name, mapping.equipment_name, constant_kw
            );
            inject_compact_constant_power(spec, constant_kw);
        }
    }
}

/// Build a `DefaultScheduleProfile` from HPXML-parsed schedule fractions.
///
/// Returns `None` if no HPXML fractions are present in the equipment parameters.
fn resolve_hpxml_profile(spec: &EquipmentSpec) -> Option<DefaultScheduleProfile> {
    let hpxml_weekday = spec
        .parameters
        .get("weekday_schedule_fractions")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_f64()).collect::<Vec<_>>());

    if let Some(ref wd) = hpxml_weekday {
        if !wd.is_empty() {
            let we = spec
                .parameters
                .get("weekend_schedule_fractions")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_f64()).collect::<Vec<_>>());

            let month = spec
                .parameters
                .get("month_multipliers")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_f64()).collect::<Vec<_>>());

            let mut profile = DefaultScheduleProfile {
                weekday_fractions: [0.0; 24],
                weekend_fractions: [0.0; 24],
                month_multipliers: [1.0; 12],
            };

            let n = wd.len().min(24);
            profile.weekday_fractions[..n].copy_from_slice(&wd[..n]);

            if let Some(ref we_vals) = we {
                let n = we_vals.len().min(24);
                profile.weekend_fractions[..n].copy_from_slice(&we_vals[..n]);
            } else {
                profile.weekend_fractions = profile.weekday_fractions;
            }

            if let Some(ref m_vals) = month {
                let n = m_vals.len().min(12);
                profile.month_multipliers[..n].copy_from_slice(&m_vals[..n]);
            }

            return Some(profile);
        }
    }

    None
}

/// Derive peak power from annual energy. For gas appliances, the total includes
/// both electric parasitic and gas combustion energy (EA-004 F1 fix).
fn determine_max_kw(spec: &EquipmentSpec, mean_fraction: f64) -> Option<f64> {
    if let Some(max_w) = spec
        .parameters
        .get("max_electric_power_w")
        .and_then(|v| v.as_f64())
    {
        return Some(max_w / 1000.0);
    }

    let annual_electric_kwh = spec
        .parameters
        .get("annual_electric_kwh")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);

    // Gas appliances (dryers, cooking ranges) have combustion energy in addition
    // to electric parasitic.
    let annual_gas_kwh = hares_physics::units::energy_therms_to_kwh(
        spec.parameters
            .get("annual_gas_therms")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0),
    );

    let annual_kwh = annual_electric_kwh + annual_gas_kwh;
    if annual_kwh <= 0.0 || mean_fraction <= 0.0 {
        return None;
    }

    Some((annual_kwh / HOURS_PER_YEAR) / mean_fraction)
}

fn determine_constant_kw(spec: &EquipmentSpec) -> Option<f64> {
    spec.parameters
        .get("max_electric_power_w")
        .and_then(|v| v.as_f64())
        .map(|w| w / 1000.0)
        .or_else(|| {
            spec.parameters
                .get("annual_electric_kwh")
                .and_then(|v| v.as_f64())
                .filter(|kwh| *kwh > 0.0)
                .map(|kwh| kwh / HOURS_PER_YEAR)
        })
}

fn inject_event_schedule(
    spec: &mut EquipmentSpec,
    mapping: &ColumnMapping,
    csv_col_map: &HashMap<String, usize>,
    schedule: &ScheduleTimeSeries,
) {
    // Skip if equipment already has event source keys.
    if spec.parameters.keys().any(|k| {
        k == "event_window_schedule_col"
            || k == "event_window_source"
            || k == "event_power_kw_series"
    }) {
        return;
    }

    let col_name = normalize_schedule_col_name(mapping.csv_column);
    let schedule_len = schedule.len();

    if let Some(&col_idx) = csv_col_map.get(col_name.as_str()) {
        let fraction_series = &schedule.columns[col_idx];
        if fraction_series.is_empty() {
            return;
        }

        // Always inject the column index for the stochastic path.
        spec.parameters.insert(
            "event_window_schedule_col".to_string(),
            Value::from(col_idx as u64),
        );

        // Compute kW time series from fractions × max_kw for deterministic event extraction.
        let mean_fraction: f64 =
            fraction_series.iter().copied().sum::<f64>() / fraction_series.len() as f64;
        if let Some(max_kw) = determine_max_kw(spec, mean_fraction) {
            let kw_series: Vec<Value> = fraction_series
                .iter()
                .map(|f| Value::from(f * max_kw))
                .collect();
            spec.parameters
                .insert("event_power_kw_series".to_string(), Value::Array(kw_series));
        }
    } else if schedule_len > 0 {
        spec.parameters
            .insert("event_window_source".to_string(), Value::from("constant"));
    }
}

/// Inject a constant power schedule for equipment that runs at rated power
/// (e.g., Ventilation Fan).  Reads `power_w` from the spec parameters.
fn inject_constant_power_schedule(spec: &mut EquipmentSpec, schedule_len: usize) {
    if schedule_len == 0 {
        return;
    }
    if spec.parameters.keys().any(|k| {
        k.starts_with("power_schedule_")
            || k.starts_with("power_profile_")
            || k == "power_constant_kw"
    }) {
        return;
    }

    let power_kw = spec
        .parameters
        .get("power_w")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0)
        / 1000.0;

    inject_compact_constant_power(spec, power_kw);
}

fn inject_compact_column_power(spec: &mut EquipmentSpec, col_idx: usize) {
    spec.parameters
        .insert("power_schedule_source".to_string(), Value::from("column"));
    spec.parameters.insert(
        "power_schedule_col".to_string(),
        Value::from(col_idx as u64),
    );
}

fn inject_compact_profile_power(
    spec: &mut EquipmentSpec,
    profile: &DefaultScheduleProfile,
    max_kw: f64,
) {
    spec.parameters.insert(
        "power_schedule_source".to_string(),
        Value::from("daily_profile"),
    );
    spec.parameters
        .insert("power_profile_max_kw".to_string(), Value::from(max_kw));
    spec.parameters.insert(
        "power_profile_weekday".to_string(),
        Value::Array(
            profile
                .weekday_fractions
                .iter()
                .map(|v| Value::from(*v))
                .collect(),
        ),
    );
    spec.parameters.insert(
        "power_profile_weekend".to_string(),
        Value::Array(
            profile
                .weekend_fractions
                .iter()
                .map(|v| Value::from(*v))
                .collect(),
        ),
    );
    spec.parameters.insert(
        "power_profile_month".to_string(),
        Value::Array(
            profile
                .month_multipliers
                .iter()
                .map(|v| Value::from(*v))
                .collect(),
        ),
    );
}

fn inject_compact_constant_power(spec: &mut EquipmentSpec, constant_kw: f64) {
    spec.parameters
        .insert("power_schedule_source".to_string(), Value::from("constant"));
    spec.parameters
        .insert("power_constant_kw".to_string(), Value::from(constant_kw));
}

fn normalize_schedule_col_name(name: &str) -> String {
    normalize_ascii(name).replace([' ', '-'], "_")
}

/// Gated invariant: every HVAC equipment spec must have a setpoint source.
///
/// If heating/cooling equipment is present and no setpoint schedule is
/// configured (schedule CSV column, HPXML-derived, or default profile),
/// the diagnostic names the missing schedule and the affected equipment.
/// This guard only runs in debug or when `feature = "check_invariants"`.
#[cfg(any(debug_assertions, feature = "check_invariants"))]
pub fn check_hvac_setpoint_invariants(specs: &[EquipmentSpec]) {
    for spec in specs {
        if HEATING_EQUIPMENT.contains(&spec.name.as_str()) {
            let has = spec_has_setpoint_source(spec, "heating");
            if !has {
                tracing::warn!(
                    equipment = %spec.name,
                    "HVAC heating equipment has no 'heating_setpoint_source': \
                     schedule CSV has no 'heating_setpoint' column, \
                     HPXML provides no heating setpoints, and \
                     defaults file has no profile for 'HVAC Heating'. \
                     The thermostat will have no heating control."
                );
            }
        }
        if COOLING_EQUIPMENT.contains(&spec.name.as_str()) {
            let has = spec_has_setpoint_source(spec, "cooling");
            if !has {
                tracing::warn!(
                    equipment = %spec.name,
                    "HVAC cooling equipment has no 'cooling_setpoint_source': \
                     schedule CSV has no 'cooling_setpoint' column, \
                     HPXML provides no cooling setpoints, and \
                     defaults file has no profile for 'HVAC Cooling'. \
                     The thermostat will have no cooling control."
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{determine_max_kw, inject_schedule_into_specs};
    use crate::{EquipmentSpec, ScheduleTimeSeries};
    use chrono::{DateTime, Duration};
    use hares_equipment::{
        ConfigPayload, ElectricResistanceWaterHeaterConfig, EquipmentConfig, GasWaterHeaterConfig,
        HeatPumpWaterHeaterConfig, HvacSetpointConfig, TanklessWaterHeaterConfig,
    };
    use hares_types::{BoundaryPolicy, FuelType, ScheduleSourceConfig};
    use serde_json::{Map, Value};
    use tempfile::tempdir;

    fn find_setpoint_in_json<'a>(
        data: &'a serde_json::Map<String, Value>,
        key: &str,
    ) -> Option<&'a Value> {
        if let Some(common) = data.get("common") {
            if let Some(sp) = common.get("setpoint") {
                if let Some(v) = sp.get(key) {
                    return Some(v);
                }
            }
        }
        if let Some(sp) = data.get("setpoint") {
            if let Some(v) = sp.get(key) {
                return Some(v);
            }
        }
        data.get(key)
    }

    fn make_schedule(hours: usize) -> ScheduleTimeSeries {
        let start =
            DateTime::parse_from_rfc3339("2025-01-01T00:00:00+00:00").expect("valid datetime");
        let timestamps = (0..hours)
            .map(|i| start + Duration::hours(i as i64))
            .collect::<Vec<_>>();

        ScheduleTimeSeries {
            timestamps,
            column_names: Vec::new(),
            columns: Vec::new(),
            column_index: HashMap::new(),
            source_step_secs: 3600,
            column_aggregations: Vec::new(),
        }
    }

    fn make_schedule_with_lighting_column(values: &[f64]) -> ScheduleTimeSeries {
        let start =
            DateTime::parse_from_rfc3339("2025-01-01T00:00:00+00:00").expect("valid datetime");
        let timestamps = (0..values.len())
            .map(|i| start + Duration::hours(i as i64))
            .collect::<Vec<_>>();

        let mut column_index = HashMap::new();
        column_index.insert("lighting_interior".to_string(), 0);
        ScheduleTimeSeries {
            timestamps,
            column_names: vec!["lighting_interior".to_string()],
            columns: vec![values.to_vec()],
            column_index,
            source_step_secs: 3600,
            column_aggregations: vec![crate::ColumnAggregation::Mean],
        }
    }

    fn make_schedule_with_water_heater_columns(
        draw_col_name: &str,
        draw_values: &[f64],
        mains_values: &[f64],
    ) -> ScheduleTimeSeries {
        assert_eq!(
            draw_values.len(),
            mains_values.len(),
            "draw and mains test vectors must have equal lengths"
        );

        let start =
            DateTime::parse_from_rfc3339("2025-01-01T00:00:00+00:00").expect("valid datetime");
        let timestamps = (0..draw_values.len())
            .map(|i| start + Duration::hours(i as i64))
            .collect::<Vec<_>>();

        let mut column_index = HashMap::new();
        column_index.insert(draw_col_name.to_string(), 0);
        column_index.insert("hot_water_mains_temperature".to_string(), 1);
        ScheduleTimeSeries {
            timestamps,
            column_names: vec![
                draw_col_name.to_string(),
                "hot_water_mains_temperature".to_string(),
            ],
            columns: vec![draw_values.to_vec(), mains_values.to_vec()],
            column_index,
            source_step_secs: 3600,
            column_aggregations: vec![
                crate::ColumnAggregation::Mean,
                crate::ColumnAggregation::Mean,
            ],
        }
    }

    fn make_schedule_with_event_column(values: &[f64]) -> ScheduleTimeSeries {
        let start =
            DateTime::parse_from_rfc3339("2025-01-01T00:00:00+00:00").expect("valid datetime");
        let timestamps = (0..values.len())
            .map(|i| start + Duration::hours(i as i64))
            .collect::<Vec<_>>();

        let mut column_index = HashMap::new();
        column_index.insert("dishwasher".to_string(), 0);
        ScheduleTimeSeries {
            timestamps,
            column_names: vec!["dishwasher".to_string()],
            columns: vec![values.to_vec()],
            column_index,
            source_step_secs: 3600,
            column_aggregations: vec![crate::ColumnAggregation::Mean],
        }
    }

    fn make_schedule_with_setpoint_columns(
        heating_values: &[f64],
        cooling_values: &[f64],
    ) -> ScheduleTimeSeries {
        assert_eq!(
            heating_values.len(),
            cooling_values.len(),
            "heating and cooling setpoint vectors must match"
        );

        let start =
            DateTime::parse_from_rfc3339("2025-01-01T00:00:00+00:00").expect("valid datetime");
        let timestamps = (0..heating_values.len())
            .map(|i| start + Duration::hours(i as i64))
            .collect::<Vec<_>>();

        let mut column_index = HashMap::new();
        column_index.insert("heating_setpoint".to_string(), 0);
        column_index.insert("cooling_setpoint".to_string(), 1);
        ScheduleTimeSeries {
            timestamps,
            column_names: vec![
                "heating_setpoint".to_string(),
                "cooling_setpoint".to_string(),
            ],
            columns: vec![heating_values.to_vec(), cooling_values.to_vec()],
            column_index,
            source_step_secs: 3600,
            column_aggregations: vec![
                crate::ColumnAggregation::Mean,
                crate::ColumnAggregation::Mean,
            ],
        }
    }

    fn make_spec(name: &str, annual_kwh: f64) -> EquipmentSpec {
        let mut parameters = Map::new();
        parameters.insert("annual_electric_kwh".to_string(), Value::from(annual_kwh));
        EquipmentSpec {
            instance_name: None,
            name: name.to_string(),
            fuel_type: FuelType::Electric,
            parameters,
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        }
    }

    fn make_spec_with_power(
        name: &str,
        annual_kwh: Option<f64>,
        max_power_w: Option<f64>,
    ) -> EquipmentSpec {
        let mut parameters = Map::new();
        if let Some(kwh) = annual_kwh {
            parameters.insert("annual_electric_kwh".to_string(), Value::from(kwh));
        }
        if let Some(max_w) = max_power_w {
            parameters.insert("max_electric_power_w".to_string(), Value::from(max_w));
        }
        EquipmentSpec {
            instance_name: None,
            name: name.to_string(),
            fuel_type: FuelType::Electric,
            parameters,
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        }
    }

    fn extract_compact_column_schedule(
        spec: &EquipmentSpec,
        schedule: &ScheduleTimeSeries,
    ) -> Vec<f64> {
        let col_idx = spec
            .parameters
            .get("power_schedule_col")
            .and_then(Value::as_u64)
            .expect("power_schedule_col must be present") as usize;
        schedule.columns[col_idx].clone()
    }

    fn write_default_profile_csv(path: &std::path::Path) {
        let mut csv = String::from("Category,Name,OCHRE Name,OCHRE Element,Values\n");
        csv.push_str(
            "Schedules,Lighting,Indoor Lighting,weekday_fractions,\"0.2,0.2,0.2,0.2,0.2,0.2,0.3,0.5,0.8,1.0,1.0,0.9,0.8,0.7,0.7,0.8,0.9,1.0,0.9,0.8,0.7,0.5,0.3,0.2\"\n",
        );
        csv.push_str(
            "Schedules,Lighting,Indoor Lighting,weekend_fractions,\"0.3,0.3,0.3,0.3,0.3,0.3,0.4,0.6,0.9,1.1,1.1,1.0,0.9,0.8,0.8,0.9,1.0,1.1,1.0,0.9,0.8,0.6,0.4,0.3\"\n",
        );
        csv.push_str(
            "Schedules,Lighting,Indoor Lighting,month_multipliers,\"0.8,0.8,0.9,1.0,1.0,1.0,1.1,1.1,1.0,0.9,0.8,0.8\"\n",
        );
        std::fs::write(path.join("Default Schedule Parameters.csv"), csv)
            .expect("write default profile csv");
    }

    #[test]
    fn missing_csv_column_uses_default_profile() {
        let dir = tempdir().expect("create temp dir");
        write_default_profile_csv(dir.path());

        let mut schedule = make_schedule(24);
        let mut specs = vec![make_spec("Indoor Lighting", 876.0)];

        inject_schedule_into_specs(&mut specs, &mut schedule, Some(dir.path()));

        assert_eq!(
            specs[0]
                .parameters
                .get("power_schedule_source")
                .and_then(Value::as_str),
            Some("daily_profile")
        );
    }

    #[test]
    fn missing_csv_and_missing_default_profile_falls_back_to_constant() {
        let dir = tempdir().expect("create temp dir");

        let mut schedule = make_schedule(24);
        let mut specs = vec![make_spec("Indoor Lighting", 876.0)];
        inject_schedule_into_specs(&mut specs, &mut schedule, Some(dir.path()));

        let expected = 876.0 / 8760.0;
        let constant_kw = specs[0]
            .parameters
            .get("power_constant_kw")
            .and_then(Value::as_f64)
            .expect("power_constant_kw must be set");
        assert!((constant_kw - expected).abs() < 1e-12);
    }

    #[test]
    fn default_profile_scaling_varies_and_scales_with_annual_kwh() {
        let dir = tempdir().expect("create temp dir");
        write_default_profile_csv(dir.path());

        let mut schedule = make_schedule(24);
        let mut specs = vec![
            make_spec("Indoor Lighting", 876.0),
            make_spec("Indoor Lighting", 1752.0),
        ];
        inject_schedule_into_specs(&mut specs, &mut schedule, Some(dir.path()));

        let max_a = specs[0]
            .parameters
            .get("power_profile_max_kw")
            .and_then(Value::as_f64)
            .expect("power_profile_max_kw should be set");
        let max_b = specs[1]
            .parameters
            .get("power_profile_max_kw")
            .and_then(Value::as_f64)
            .expect("power_profile_max_kw should be set");
        assert!((max_b - 2.0 * max_a).abs() < 1e-12);
    }

    #[test]
    fn max_electric_power_w_without_annual_kwh_sets_csv_peak() {
        let mut schedule = make_schedule_with_lighting_column(&[0.2, 1.0, 0.4]);
        let mut specs = vec![make_spec_with_power("Indoor Lighting", None, Some(500.0))];
        inject_schedule_into_specs(&mut specs, &mut schedule, None);

        let kw = extract_compact_column_schedule(&specs[0], &schedule);
        let peak = kw.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        assert!(
            (peak - 0.5).abs() < 1e-9,
            "expected peak 0.5 kW, got {peak}"
        );

        assert_eq!(
            specs[0]
                .parameters
                .get("power_schedule_source")
                .and_then(Value::as_str),
            Some("column")
        );
        let derived_col = specs[0]
            .parameters
            .get("power_schedule_col")
            .and_then(Value::as_u64)
            .expect("power_schedule_col should be present") as usize;
        assert!(
            derived_col < schedule.columns.len(),
            "derived column index should be in-range"
        );
    }

    #[test]
    fn max_electric_power_w_takes_precedence_over_annual_kwh_csv_branch() {
        let mut schedule = make_schedule_with_lighting_column(&[0.2, 1.0, 0.4]);
        let mut specs = vec![make_spec_with_power(
            "Indoor Lighting",
            Some(1200.0),
            Some(500.0),
        )];
        inject_schedule_into_specs(&mut specs, &mut schedule, None);

        let kw = extract_compact_column_schedule(&specs[0], &schedule);
        let peak = kw.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        assert!((peak - 0.5).abs() < 1e-9, "max power should win precedence");
    }

    #[test]
    fn annual_kwh_only_behavior_unchanged_csv_branch() {
        let mut schedule = make_schedule_with_lighting_column(&[0.2, 1.0, 0.4]);
        let mut specs = vec![make_spec_with_power("Indoor Lighting", Some(1200.0), None)];
        inject_schedule_into_specs(&mut specs, &mut schedule, None);
        let kw = extract_compact_column_schedule(&specs[0], &schedule);

        let mean_fraction = (0.2 + 1.0 + 0.4) / 3.0;
        let expected_peak = (1200.0 / 8760.0) / mean_fraction;
        let peak = kw.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        assert!(
            (peak - expected_peak).abs() < 1e-9,
            "expected peak from annual_kwh scaling; expected {expected_peak}, got {peak}"
        );
    }

    #[test]
    fn max_electric_power_w_without_annual_kwh_default_profile_branch_is_non_zero() {
        let dir = tempdir().expect("create temp dir");
        write_default_profile_csv(dir.path());

        let mut schedule = make_schedule(24);
        let mut specs = vec![make_spec_with_power("Indoor Lighting", None, Some(500.0))];
        inject_schedule_into_specs(&mut specs, &mut schedule, Some(dir.path()));

        let peak = specs[0]
            .parameters
            .get("power_profile_max_kw")
            .and_then(Value::as_f64)
            .expect("power_profile_max_kw should be present");
        assert!(
            peak > 0.0,
            "default-profile branch should not produce dead schedule"
        );
        assert_eq!(
            specs[0]
                .parameters
                .get("power_schedule_source")
                .and_then(Value::as_str),
            Some("daily_profile")
        );
        assert!(
            specs[0].parameters.contains_key("power_profile_max_kw"),
            "profile max should be injected"
        );
        assert_eq!(
            specs[0]
                .parameters
                .get("power_profile_weekday")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(24)
        );
    }

    #[test]
    fn constant_injects_compact_constant_keys() {
        let dir = tempdir().expect("create temp dir");
        let mut schedule = make_schedule(24);
        let mut specs = vec![make_spec("Indoor Lighting", 876.0)];

        inject_schedule_into_specs(&mut specs, &mut schedule, Some(dir.path()));

        assert_eq!(
            specs[0]
                .parameters
                .get("power_schedule_source")
                .and_then(Value::as_str),
            Some("constant")
        );
        let constant_kw = specs[0]
            .parameters
            .get("power_constant_kw")
            .and_then(Value::as_f64)
            .expect("power_constant_kw should be present");
        assert!((constant_kw - (876.0 / 8760.0)).abs() < 1e-12);
    }

    #[test]
    fn event_csv_injects_schedule_column_reference() {
        let mut schedule = make_schedule_with_event_column(&[0.0, 1.0, 0.0]);
        let mut specs = vec![make_spec("Dishwasher", 0.0)];

        inject_schedule_into_specs(&mut specs, &mut schedule, None);

        assert_eq!(
            specs[0]
                .parameters
                .get("event_window_schedule_col")
                .and_then(Value::as_u64),
            Some(0)
        );
        assert!(!specs[0].parameters.contains_key("event_window_0"));
    }

    #[test]
    fn event_missing_csv_injects_constant_source() {
        let mut schedule = make_schedule(4);
        let mut specs = vec![make_spec("Dishwasher", 0.0)];

        inject_schedule_into_specs(&mut specs, &mut schedule, None);

        assert_eq!(
            specs[0]
                .parameters
                .get("event_window_source")
                .and_then(Value::as_str),
            Some("constant")
        );
        assert!(!specs[0].parameters.contains_key("event_window_len"));
    }

    fn make_typed_spec<T: hares_equipment::EquipmentTypedConfig>(
        name: &str,
        ochre_class: &str,
        config: T,
    ) -> EquipmentSpec {
        EquipmentSpec {
            instance_name: None,
            name: name.to_string(),
            fuel_type: FuelType::Electric,
            parameters: Map::new(),
            zip_params: None,
            typed_config: Some(hares_equipment::EquipmentConfig::from_typed(
                name.to_string(),
                ochre_class.to_string(),
                config,
            )),
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        }
    }

    #[test]
    fn typed_hvac_specs_receive_column_ref_setpoint_sources() {
        use hares_equipment::hvac::heat_pump_config::{
            HeatPumpCommonConfig, HeatPumpCoolerConfig, HeatPumpHeaterConfig,
        };

        let mut schedule = make_schedule_with_setpoint_columns(&[20.0, 19.5], &[26.0, 25.5]);
        let mut specs = vec![
            make_typed_spec(
                "ASHP Heater",
                "ASHP Heater",
                HeatPumpHeaterConfig {
                    common: HeatPumpCommonConfig {
                        zone_id: Some(1),
                        ..HeatPumpCommonConfig::default()
                    },
                    ..HeatPumpHeaterConfig::default()
                },
            ),
            make_typed_spec(
                "ASHP Cooler",
                "ASHP Cooler",
                HeatPumpCoolerConfig {
                    common: HeatPumpCommonConfig {
                        equipment_id: None,
                        zone_id: Some(1),
                        heating_capacity_w: None,
                        heating_eir: None,
                        stage_heating_capacities_w: None,
                        stage_heating_eirs: None,
                        backup_fuel: None,
                        backup_capacity_w: None,
                        backup_eir: None,
                        fraction_heating_load_served: None,
                        cooling_capacity_w: None,
                        cooling_eir: None,
                        stage_cooling_capacities_w: None,
                        stage_cooling_eirs: None,
                        fraction_cooling_load_served: None,
                        number_of_speeds: 1,
                        is_mini_split: false,
                        shr: None,
                        fan_power_w: None,
                        fan_power_w_per_cfm: None,
                        airflow_m3_s_per_w: None,
                        setpoint: HvacSetpointConfig::default(),
                        hysteresis_c: None,
                        duct: hares_equipment::DuctConfig::default(),
                        biquadratic_x1_min: None,
                        biquadratic_x1_max: None,
                        biquadratic_x2_min: None,
                        biquadratic_x2_max: None,
                        ff_min: None,
                        ff_max: None,
                        plf_min: None,
                        plf_max: None,
                        min_compressor_fraction: 0.25,
                        eir_part_load_benefit: None,
                        er_stages: 1,
                        charge_defect_ratio: None,
                        ..Default::default()
                    },
                    stage_shrs: None,
                    crankcase_heater_kw: None,
                    crankcase_heater_threshold_c: None,
                },
            ),
        ];

        inject_schedule_into_specs(&mut specs, &mut schedule, None);

        let heater_typed = specs[0]
            .typed_config
            .as_ref()
            .expect("heater typed config must remain present");
        let heater_data = match &heater_typed.payload {
            ConfigPayload::Typed { data, .. } => data.as_object().expect("data must be an object"),
            other => panic!("expected typed payload, got {other:?}"),
        };
        let heater_source: hares_types::ScheduleSourceConfig = serde_json::from_value(
            find_setpoint_in_json(heater_data, "heating_setpoint_source")
                .cloned()
                .expect("heater heating_setpoint_source must be injected"),
        )
        .expect("heater source must deserialize");
        assert_eq!(
            heater_source,
            hares_types::ScheduleSourceConfig::ColumnRef {
                col_idx: 0,
                boundary: BoundaryPolicy::Clamp
            }
        );

        let cooler_typed = specs[1]
            .typed_config
            .as_ref()
            .expect("cooler typed config must remain present");
        let cooler_data = match &cooler_typed.payload {
            ConfigPayload::Typed { data, .. } => data.as_object().expect("data must be an object"),
            other => panic!("expected typed payload, got {other:?}"),
        };
        let cooler_source: hares_types::ScheduleSourceConfig = serde_json::from_value(
            find_setpoint_in_json(cooler_data, "cooling_setpoint_source")
                .cloned()
                .expect("cooler cooling_setpoint_source must be injected"),
        )
        .expect("cooler source must deserialize");
        assert_eq!(
            cooler_source,
            hares_types::ScheduleSourceConfig::ColumnRef {
                col_idx: 1,
                boundary: BoundaryPolicy::Clamp
            }
        );
    }

    fn make_water_heater_spec(name: &str, avg_water_draw_l_per_day: f64) -> EquipmentSpec {
        let typed_config = match name {
            "Electric Resistance Water Heater" => EquipmentConfig::from_typed(
                "electric".to_string(),
                "Electric Resistance Water Heater".to_string(),
                ElectricResistanceWaterHeaterConfig {
                    equipment_id: None,
                    zone_id: None,
                    loop_id: None,
                    tank_volume_m3: Some(0.19),
                    tank_height_m: Some(1.4),
                    energy_factor: Some(0.92),
                    uniform_energy_factor: Some(0.94),
                    heating_capacity_w: Some(4_500.0),
                    ua_w_per_k: Some(3.0),
                    setpoint_c: Some(51.67),
                    deadband_c: None,
                    max_tank_temp_c: None,
                    initial_tank_temp_c: None,
                    tank_nodes: None,
                    avg_water_draw_l_per_day: Some(avg_water_draw_l_per_day),
                    draw_flow_rate_kg_s: None,
                    draw_flow_rate_source: None,
                    mains_temp_c_source: None,
                    performance_adjustment: Some(0.95),
                    zone_type: Some("conditioned".to_string()),
                    first_hour_rating_m3: Some(0.20),
                    element_power_w: Some(4_500.0),
                    element_priority_mode: None,
                    max_setpoint_ramp_rate_c_per_min: None,
                    jacket_r_value_m2_k_w: None,
                    fixture_delivery_temp_c: None,
                    hot_draw_temp_c: None,
                },
            ),
            "Gas Water Heater" => EquipmentConfig::from_typed(
                "gas".to_string(),
                "Gas Water Heater".to_string(),
                GasWaterHeaterConfig {
                    equipment_id: None,
                    zone_id: None,
                    loop_id: None,
                    fuel_type: FuelType::Gas,
                    tank_volume_m3: Some(0.19),
                    tank_height_m: Some(1.4),
                    energy_factor: Some(0.82),
                    uniform_energy_factor: Some(0.84),
                    heating_capacity_w: Some(11_000.0),
                    ua_w_per_k: Some(3.0),
                    setpoint_c: Some(51.67),
                    deadband_c: None,
                    max_tank_temp_c: None,
                    initial_tank_temp_c: None,
                    tank_nodes: None,
                    avg_water_draw_l_per_day: Some(avg_water_draw_l_per_day),
                    draw_flow_rate_kg_s: None,
                    draw_flow_rate_source: None,
                    mains_temp_c_source: None,
                    pilot_power_w: Some(5.0),
                    flue_loss_fraction: Some(0.12),
                    skin_loss_fraction: None,
                    ignition_type: None,
                    performance_adjustment: Some(0.92),
                    zone_type: Some("conditioned".to_string()),
                    first_hour_rating_m3: Some(0.20),
                    jacket_r_value_m2_k_w: None,
                    conversion_efficiency: None,
                    fixture_delivery_temp_c: None,
                    hot_draw_temp_c: None,
                },
            ),
            "Heat Pump Water Heater" => EquipmentConfig::from_typed(
                "hpwh".to_string(),
                "Heat Pump Water Heater".to_string(),
                HeatPumpWaterHeaterConfig {
                    equipment_id: None,
                    zone_id: None,
                    loop_id: None,
                    tank_volume_m3: Some(0.24),
                    tank_height_m: Some(1.5),
                    cop: Some(3.5),
                    backup_element_power_w: Some(4_500.0),
                    ua_w_per_k: Some(2.5),
                    setpoint_c: Some(51.67),
                    deadband_c: None,
                    max_tank_temp_c: None,
                    initial_tank_temp_c: None,
                    tank_nodes: None,
                    tempering_valve_setpoint_c: Some(51.67),
                    avg_water_draw_l_per_day: Some(avg_water_draw_l_per_day),
                    draw_flow_rate_kg_s: None,
                    compressor_power_w: None,
                    backup_enable_offset_c: None,
                    min_ambient_temp_c: None,
                    max_ambient_temp_c: None,
                    min_on_time_s: None,
                    min_off_time_s: None,
                    hp_only_mode: None,
                    element_hp_control_mode: None,
                    fan_power_w: None,
                    parasitic_power_w: None,
                    backup_efficiency: None,
                    shr: None,
                    lost_heat_fraction: None,
                    wall_heat_fraction: None,
                    capacity_biquadratic_coeffs: None,
                    cop_biquadratic_coeffs: None,
                    performance_adjustment: Some(0.92),
                    zone_type: Some("conditioned".to_string()),
                    first_hour_rating_m3: Some(0.20),
                    jacket_r_value_m2_k_w: None,
                    fixture_delivery_temp_c: None,
                },
            ),
            other => panic!("unsupported water heater type for schedule test: {other}"),
        };
        EquipmentSpec {
            instance_name: None,
            name: name.to_string(),
            fuel_type: if name == "Gas Water Heater" {
                FuelType::Gas
            } else {
                FuelType::Electric
            },
            parameters: Map::new(),
            zip_params: None,
            typed_config: Some(typed_config),
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        }
    }

    fn make_typed_tankless_spec(avg_water_draw_l_per_day: Option<f64>) -> EquipmentSpec {
        let typed_config = EquipmentConfig::from_typed(
            "tankless".to_string(),
            "Tankless Water Heater".to_string(),
            TanklessWaterHeaterConfig {
                equipment_id: None,
                zone_id: None,
                loop_id: None,
                fuel_type: FuelType::Electric,
                energy_factor: Some(1.0),
                uniform_energy_factor: None,
                heating_capacity_w: Some(20_000.0),
                setpoint_c: Some(60.0),
                parasitic_power_w: Some(0.0),
                performance_adjustment: Some(1.0),
                inlet_temp_c: Some(25.0),
                draw_flow_rate_kg_s: Some(0.10),
                draw_flow_rate_source: None,
                mains_temp_c_source: None,
                avg_water_draw_l_per_day,
            },
        );
        EquipmentSpec {
            instance_name: None,
            name: "Tankless Water Heater".to_string(),
            fuel_type: FuelType::Electric,
            parameters: Map::new(),
            zip_params: None,
            typed_config: Some(typed_config),
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        }
    }

    #[test]
    fn water_heaters_receive_draw_and_mains_schedule_columns() {
        let mut schedule = make_schedule_with_water_heater_columns(
            "hot_water_fixtures",
            &[0.2, 0.0],
            &[11.0, 12.0],
        );
        let mut specs = vec![
            make_water_heater_spec("Electric Resistance Water Heater", 200.0),
            make_water_heater_spec("Gas Water Heater", 200.0),
            make_water_heater_spec("Heat Pump Water Heater", 200.0),
            make_typed_tankless_spec(Some(200.0)),
        ];

        inject_schedule_into_specs(&mut specs, &mut schedule, None);

        for spec in specs.iter().take(3) {
            let typed = spec
                .typed_config
                .as_ref()
                .expect("storage water heater should retain typed config");
            if spec.name == "Heat Pump Water Heater" {
                let cfg = typed
                    .typed::<HeatPumpWaterHeaterConfig>()
                    .expect("typed HPWH config should decode");
                assert_eq!(cfg.draw_flow_rate_kg_s, None);
                assert_eq!(cfg.avg_water_draw_l_per_day, Some(200.0));
            } else if spec.name == "Electric Resistance Water Heater" {
                let cfg = typed
                    .typed::<ElectricResistanceWaterHeaterConfig>()
                    .expect("typed resistance config should decode");
                assert!(matches!(
                    cfg.draw_flow_rate_source,
                    Some(ScheduleSourceConfig::ColumnRef { col_idx, .. }) if col_idx >= 2
                ));
                assert!(matches!(
                    cfg.mains_temp_c_source,
                    Some(ScheduleSourceConfig::ColumnRef { col_idx: 1, .. })
                ));
            } else {
                let cfg = typed
                    .typed::<GasWaterHeaterConfig>()
                    .expect("typed gas water heater config should decode");
                assert!(matches!(
                    cfg.draw_flow_rate_source,
                    Some(ScheduleSourceConfig::ColumnRef { col_idx, .. }) if col_idx >= 2
                ));
                assert!(matches!(
                    cfg.mains_temp_c_source,
                    Some(ScheduleSourceConfig::ColumnRef { col_idx: 1, .. })
                ));
            }
        }

        let tankless = specs[3]
            .typed_config
            .as_ref()
            .expect("tankless should retain typed config");
        let tankless = tankless
            .typed::<TanklessWaterHeaterConfig>()
            .expect("typed tankless config should decode");
        assert!(
            matches!(
                tankless.draw_flow_rate_source,
                Some(ScheduleSourceConfig::ColumnRef { col_idx, .. }) if col_idx >= 2
            ),
            "tankless draw_flow_rate_source should point at derived schedule column"
        );
        assert!(
            matches!(
                tankless.mains_temp_c_source,
                Some(ScheduleSourceConfig::ColumnRef { col_idx: 1, .. })
            ),
            "tankless mains_temp_c_source should point at the mains schedule column"
        );
    }

    #[test]
    fn water_heater_draw_fractions_normalized_to_kg_s() {
        // Known fractions and avg_water_draw_l_per_day = 200.
        // normalize_draw_profile formula returns kg/s:
        //   mean_fraction = mean(fractions)
        //   scale = (avg_daily_l / 1440) / mean_fraction
        //   result_kg_s = fraction * scale / 60
        let fractions = vec![0.04, 0.08, 0.02, 0.06];
        let avg_daily_l = 200.0;
        let mean_frac: f64 = fractions.iter().sum::<f64>() / fractions.len() as f64; // 0.05
        let scale = (avg_daily_l / 1440.0) / mean_frac;

        let mains = vec![12.0; 4];
        let mut schedule =
            make_schedule_with_water_heater_columns("hot_water_fixtures", &fractions, &mains);
        let mut specs = vec![make_water_heater_spec(
            "Electric Resistance Water Heater",
            avg_daily_l,
        )];

        inject_schedule_into_specs(&mut specs, &mut schedule, None);

        let draw_col_idx = specs[0]
            .typed_config
            .as_ref()
            .and_then(|typed| typed.typed::<ElectricResistanceWaterHeaterConfig>().ok())
            .and_then(|cfg| cfg.draw_flow_rate_source)
            .and_then(|source| match source {
                ScheduleSourceConfig::ColumnRef { col_idx, .. } => Some(col_idx),
                _ => None,
            })
            .expect("typed draw_flow_rate_source must be present");

        let resolved = &schedule.columns[draw_col_idx];
        assert_eq!(resolved.len(), fractions.len());

        for (i, &frac) in fractions.iter().enumerate() {
            let expected_kg_s = frac * scale / 60.0;
            assert!(
                (resolved[i] - expected_kg_s).abs() < 1e-10,
                "timestep {i}: expected {expected_kg_s:.8e} kg/s, got {:.8e}",
                resolved[i]
            );
        }

        // Sanity: the 0.04 fraction should produce a small but non-zero SI mass flow.
        let expected_for_004 = 0.04 * scale / 60.0;
        assert!(
            expected_for_004 > 0.0,
            "expected meaningful SI mass flow, got {expected_for_004:.6e} kg/s"
        );
    }

    #[test]
    #[should_panic(expected = "requires typed avg_water_draw_l_per_day")]
    fn tankless_draw_fractions_require_typed_avg_water_draw() {
        let mut schedule = make_schedule_with_water_heater_columns(
            "hot_water_fixtures",
            &[0.2, 0.0],
            &[11.0, 12.0],
        );
        let mut specs = vec![make_typed_tankless_spec(None)];

        inject_schedule_into_specs(&mut specs, &mut schedule, None);
    }

    // =======================================================================
    // Quantitative: determine_max_kw with both electric and gas annual energy
    // =======================================================================

    #[test]
    fn determine_max_kw_electric_plus_gas_therms() {
        let annual_electric_kwh = 44.0;
        let annual_gas_therms = 153.0;
        let fractions = [0.2, 1.0, 0.4];
        let mean_fraction: f64 = fractions.iter().sum::<f64>() / fractions.len() as f64;

        let mut parameters = Map::new();
        parameters.insert(
            "annual_electric_kwh".to_string(),
            Value::from(annual_electric_kwh),
        );
        parameters.insert(
            "annual_gas_therms".to_string(),
            Value::from(annual_gas_therms),
        );
        let spec = EquipmentSpec {
            instance_name: None,
            name: "Gas Dryer".to_string(),
            fuel_type: FuelType::Gas,
            parameters,
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        };

        let result = determine_max_kw(&spec, mean_fraction);
        assert!(result.is_some(), "determine_max_kw must return Some");

        let annual_gas_kwh = hares_physics::units::energy_therms_to_kwh(annual_gas_therms);
        let expected = (annual_electric_kwh + annual_gas_kwh) / 8760.0 / mean_fraction;
        let actual = result.unwrap();

        assert!(
            (actual - expected).abs() < 1e-9,
            "expected {expected:.6}, got {actual:.6}"
        );
    }

    // ── Helper: produce a defaults CSV that includes setpoint profiles ──

    fn write_defaults_csv_with_setpoints(path: &std::path::Path) {
        let mut csv = String::from("Category,Name,OCHRE Name,OCHRE Element,Values\n");
        // Power profile (so existing tests can also use this)
        csv.push_str(
            "Schedules,Lighting,Indoor Lighting,weekday_fractions,\"0.2,0.2,0.2,0.2,0.2,0.2,0.3,0.5,0.8,1.0,1.0,0.9,0.8,0.7,0.7,0.8,0.9,1.0,0.9,0.8,0.7,0.5,0.3,0.2\"\n",
        );
        csv.push_str(
            "Schedules,Lighting,Indoor Lighting,weekend_fractions,\"0.3,0.3,0.3,0.3,0.3,0.3,0.4,0.6,0.9,1.1,1.1,1.0,0.9,0.8,0.8,0.9,1.0,1.1,1.0,0.9,0.8,0.6,0.4,0.3\"\n",
        );
        csv.push_str(
            "Schedules,Lighting,Indoor Lighting,month_multipliers,\"0.8,0.8,0.9,1.0,1.0,1.0,1.1,1.1,1.0,0.9,0.8,0.8\"\n",
        );
        // Setpoint profiles: 1.0 fractions × month multipliers (the
        // temperature value is carried as max_value in the Rust code).
        for (col0, ochre_name) in [
            ("heating_setpoint", "HVAC Heating"),
            ("cooling_setpoint", "HVAC Cooling"),
        ] {
            csv.push_str(&format!(
                "{col0},WeekdayScheduleFractions,{ochre_name},weekday_fractions,\"1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0\"\n",
            ));
            csv.push_str(&format!(
                "{col0},WeekendScheduleFractions,{ochre_name},weekend_fractions,\"1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0\"\n",
            ));
            csv.push_str(&format!(
                "{col0},MonthlyScheduleMultipliers,{ochre_name},month_multipliers,\"1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0\"\n",
            ));
        }
        std::fs::write(path.join("Default Schedule Parameters.csv"), csv)
            .expect("write defaults csv with setpoints");
    }

    // ── Setpoint default profile tests ──

    #[test]
    fn load_default_profiles_parses_hvac_heating_and_cooling_from_csv() {
        let dir = tempdir().expect("create temp dir");
        write_defaults_csv_with_setpoints(dir.path());

        let profiles = super::load_default_profiles(dir.path());

        let heat = profiles
            .get("HVAC Heating")
            .expect("HVAC Heating profile must be loaded");
        assert_eq!(heat.weekday_fractions, [1.0; 24]);
        assert_eq!(heat.weekend_fractions, [1.0; 24]);
        assert_eq!(heat.month_multipliers, [1.0; 12]);

        let cool = profiles
            .get("HVAC Cooling")
            .expect("HVAC Cooling profile must be loaded");
        assert_eq!(cool.weekday_fractions, [1.0; 24]);
        assert_eq!(cool.weekend_fractions, [1.0; 24]);
        assert_eq!(cool.month_multipliers, [1.0; 12]);
    }

    #[test]
    fn hvac_spec_without_hpxml_setpoints_gets_default_daily_profile() {
        use hares_equipment::hvac::heat_pump_config::{
            HeatPumpCommonConfig, HeatPumpCoolerConfig, HeatPumpHeaterConfig,
        };

        let dir = tempdir().expect("create temp dir");
        write_defaults_csv_with_setpoints(dir.path());

        let mut schedule = make_schedule(24);
        let mut specs = vec![
            make_typed_spec(
                "ASHP Heater",
                "ASHP Heater",
                HeatPumpHeaterConfig {
                    common: HeatPumpCommonConfig {
                        zone_id: Some(1),
                        setpoint: HvacSetpointConfig::default(),
                        ..HeatPumpCommonConfig::default()
                    },
                    ..HeatPumpHeaterConfig::default()
                },
            ),
            make_typed_spec(
                "ASHP Cooler",
                "ASHP Cooler",
                HeatPumpCoolerConfig {
                    common: HeatPumpCommonConfig {
                        equipment_id: None,
                        zone_id: Some(1),
                        heating_capacity_w: None,
                        heating_eir: None,
                        stage_heating_capacities_w: None,
                        stage_heating_eirs: None,
                        backup_fuel: None,
                        backup_capacity_w: None,
                        backup_eir: None,
                        fraction_heating_load_served: None,
                        cooling_capacity_w: None,
                        cooling_eir: None,
                        stage_cooling_capacities_w: None,
                        stage_cooling_eirs: None,
                        fraction_cooling_load_served: None,
                        number_of_speeds: 1,
                        is_mini_split: false,
                        shr: None,
                        fan_power_w: None,
                        fan_power_w_per_cfm: None,
                        airflow_m3_s_per_w: None,
                        setpoint: HvacSetpointConfig::default(),
                        hysteresis_c: None,
                        duct: hares_equipment::DuctConfig::default(),
                        biquadratic_x1_min: None,
                        biquadratic_x1_max: None,
                        biquadratic_x2_min: None,
                        biquadratic_x2_max: None,
                        ff_min: None,
                        ff_max: None,
                        plf_min: None,
                        plf_max: None,
                        min_compressor_fraction: 0.25,
                        eir_part_load_benefit: None,
                        er_stages: 1,
                        charge_defect_ratio: None,
                        ..Default::default()
                    },
                    stage_shrs: None,
                    crankcase_heater_kw: None,
                    crankcase_heater_threshold_c: None,
                },
            ),
        ];

        inject_schedule_into_specs(&mut specs, &mut schedule, Some(dir.path()));

        // Heater: should receive a heating DailyProfile with max_value = 20 °C.
        let heater_data = typed_data_of_spec(&specs[0]);
        let heater_source: hares_types::ScheduleSourceConfig = serde_json::from_value(
            find_setpoint_in_json(heater_data, "heating_setpoint_source")
                .cloned()
                .expect("heater heating_setpoint_source must be injected"),
        )
        .expect("heater source must deserialize");
        assert!(
            matches!(&heater_source,
                hares_types::ScheduleSourceConfig::DailyProfile { weekday, max_value, .. }
                if (weekday[0] - super::HERS_HEATING_SETPOINT_C).abs() < 1e-12
                && (max_value - 1.0).abs() < 1e-12),
            "expected DailyProfile with weekday[0]={}°C and max_value=1.0, got {heater_source:?}",
            super::HERS_HEATING_SETPOINT_C,
        );

        // Cooler: should receive a cooling DailyProfile with max_value = 24 °C.
        let cooler_data = typed_data_of_spec(&specs[1]);
        let cooler_source: hares_types::ScheduleSourceConfig = serde_json::from_value(
            find_setpoint_in_json(cooler_data, "cooling_setpoint_source")
                .cloned()
                .expect("cooler cooling_setpoint_source must be injected"),
        )
        .expect("cooler source must deserialize");
        assert!(
            matches!(&cooler_source,
                hares_types::ScheduleSourceConfig::DailyProfile { weekday, max_value, .. }
                if (weekday[0] - super::HERS_COOLING_SETPOINT_C).abs() < 1e-12
                && (max_value - 1.0).abs() < 1e-12),
            "expected DailyProfile with weekday[0]={}°C and max_value=1.0, got {cooler_source:?}",
            super::HERS_COOLING_SETPOINT_C,
        );

        // Heater should NOT get a cooling source, and cooler should NOT get a
        // heating source.
        assert!(
            find_setpoint_in_json(heater_data, "cooling_setpoint_source").is_none(),
            "heater should not have a cooling setpoint source"
        );
        assert!(
            find_setpoint_in_json(cooler_data, "heating_setpoint_source").is_none(),
            "cooler should not have a heating setpoint source"
        );
    }

    /// Extract the typed data JSON object from a spec.
    fn typed_data_of_spec(spec: &EquipmentSpec) -> &serde_json::Map<String, Value> {
        let typed = spec
            .typed_config
            .as_ref()
            .expect("typed config must be present");
        match &typed.payload {
            ConfigPayload::Typed { data, .. } => data.as_object().expect("data must be an object"),
            other => panic!("expected Typed payload, got {other:?}"),
        }
    }

    #[test]
    fn setpoint_defaults_are_not_injected_when_hpxml_source_already_present() {
        use hares_equipment::hvac::heat_pump_config::{
            HeatPumpCommonConfig, HeatPumpCoolerConfig, HeatPumpHeaterConfig,
        };

        let dir = tempdir().expect("create temp dir");
        write_defaults_csv_with_setpoints(dir.path());

        let mut schedule = make_schedule(24);

        let hpxml_heating_source = hares_types::ScheduleSourceConfig::DailyProfile {
            weekday: [1.0; 24],
            weekend: [1.0; 24],
            month_multipliers: [1.0; 12],
            max_value: 22.0,
        };
        let hpxml_cooling_source = hares_types::ScheduleSourceConfig::DailyProfile {
            weekday: [1.0; 24],
            weekend: [1.0; 24],
            month_multipliers: [1.0; 12],
            max_value: 26.0,
        };

        let mut specs = vec![
            make_typed_spec(
                "ASHP Heater",
                "ASHP Heater",
                HeatPumpHeaterConfig {
                    common: HeatPumpCommonConfig {
                        zone_id: Some(1),
                        setpoint: HvacSetpointConfig {
                            heating_setpoint_c: Some(22.0),
                            heating_setpoint_source: Some(hpxml_heating_source.clone()),
                            ..Default::default()
                        },
                        ..HeatPumpCommonConfig::default()
                    },
                    ..HeatPumpHeaterConfig::default()
                },
            ),
            make_typed_spec(
                "ASHP Cooler",
                "ASHP Cooler",
                HeatPumpCoolerConfig {
                    common: HeatPumpCommonConfig {
                        equipment_id: None,
                        zone_id: Some(1),
                        heating_capacity_w: None,
                        heating_eir: None,
                        stage_heating_capacities_w: None,
                        stage_heating_eirs: None,
                        backup_fuel: None,
                        backup_capacity_w: None,
                        backup_eir: None,
                        fraction_heating_load_served: None,
                        cooling_capacity_w: None,
                        cooling_eir: None,
                        stage_cooling_capacities_w: None,
                        stage_cooling_eirs: None,
                        fraction_cooling_load_served: None,
                        number_of_speeds: 1,
                        is_mini_split: false,
                        shr: None,
                        fan_power_w: None,
                        fan_power_w_per_cfm: None,
                        airflow_m3_s_per_w: None,
                        setpoint: HvacSetpointConfig {
                            cooling_setpoint_c: Some(26.0),
                            cooling_setpoint_source: Some(hpxml_cooling_source.clone()),
                            ..Default::default()
                        },
                        hysteresis_c: None,
                        duct: hares_equipment::DuctConfig::default(),
                        biquadratic_x1_min: None,
                        biquadratic_x1_max: None,
                        biquadratic_x2_min: None,
                        biquadratic_x2_max: None,
                        ff_min: None,
                        ff_max: None,
                        plf_min: None,
                        plf_max: None,
                        min_compressor_fraction: 0.25,
                        eir_part_load_benefit: None,
                        er_stages: 1,
                        charge_defect_ratio: None,
                        ..Default::default()
                    },
                    stage_shrs: None,
                    crankcase_heater_kw: None,
                    crankcase_heater_threshold_c: None,
                },
            ),
        ];

        inject_schedule_into_specs(&mut specs, &mut schedule, Some(dir.path()));

        let heater_data = typed_data_of_spec(&specs[0]);
        let heater_source: hares_types::ScheduleSourceConfig = serde_json::from_value(
            find_setpoint_in_json(heater_data, "heating_setpoint_source")
                .cloned()
                .expect("heating_setpoint_source must remain"),
        )
        .expect("source must deserialize");
        assert_eq!(
            heater_source, hpxml_heating_source,
            "HPXML heating setpoint source must NOT be overridden by defaults"
        );

        let cooler_data = typed_data_of_spec(&specs[1]);
        let cooler_source: hares_types::ScheduleSourceConfig = serde_json::from_value(
            find_setpoint_in_json(cooler_data, "cooling_setpoint_source")
                .cloned()
                .expect("cooling_setpoint_source must remain"),
        )
        .expect("source must deserialize");
        assert_eq!(
            cooler_source, hpxml_cooling_source,
            "HPXML cooling setpoint source must NOT be overridden by defaults"
        );
    }

    #[test]
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    fn invariant_detects_missing_heating_setpoint_source() {
        use hares_equipment::hvac::heat_pump_config::{HeatPumpCommonConfig, HeatPumpHeaterConfig};

        let specs = vec![make_typed_spec(
            "ASHP Heater",
            "ASHP Heater",
            HeatPumpHeaterConfig {
                common: HeatPumpCommonConfig {
                    zone_id: Some(1),
                    setpoint: HvacSetpointConfig::default(),
                    ..HeatPumpCommonConfig::default()
                },
                ..HeatPumpHeaterConfig::default()
            },
        )];

        let heater_data = typed_data_of_spec(&specs[0]);
        assert!(
            !heater_data.contains_key("heating_setpoint_source"),
            "spec must not have heating_setpoint_source — precondition for invariant check"
        );

        super::check_hvac_setpoint_invariants(&specs);
    }

    #[test]
    fn load_default_profiles_occupants_has_distinct_weekend_fractions() {
        use std::path::Path;
        let defaults_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../defaults");
        let profiles = super::load_default_profiles(&defaults_dir);

        let occ = profiles
            .get("Occupancy")
            .expect("Occupancy profile must exist in defaults");
        assert!(
            occ.weekday_fractions != occ.weekend_fractions,
            "Occupancy weekday and weekend fractions must differ; \
             ASHRAE 90.2/HERS Reference Home requires distinct weekend occupancy patterns"
        );
    }

    #[test]
    fn load_default_profiles_all_priority_schedules_have_distinct_weekend_fractions() {
        use std::path::Path;
        let defaults_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../defaults");
        let profiles = super::load_default_profiles(&defaults_dir);

        let priority_schedules = [
            "Occupancy",
            "Indoor Lighting",
            "Exterior Lighting",
            "Garage Lighting",
            "Water Heating",
            "Cooking Range",
            "Dishwasher",
            "Clothes Washer",
            "Clothes Dryer",
            "MELs",
            "TV",
            "Ceiling Fan",
        ];

        for name in &priority_schedules {
            let profile = profiles
                .get(*name)
                .unwrap_or_else(|| panic!("schedule '{name}' must exist in defaults"));
            assert!(
                profile.weekday_fractions != profile.weekend_fractions,
                "schedule '{name}' must have distinct weekday and weekend fraction arrays"
            );
        }
    }

    #[test]
    fn fixed_operation_schedules_keep_identical_fractions() {
        use std::path::Path;
        let defaults_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../defaults");
        let profiles = super::load_default_profiles(&defaults_dir);

        let fixed_schedules = [
            "Refrigerator",
            "Pool Pump",
            "Spa Pump",
            "Spa Heater",
            "Pool Heater",
            "Well Pump",
            "Gas Fireplace",
            "Gas Lighting",
            "HVAC Heating",
            "HVAC Cooling",
        ];

        for name in &fixed_schedules {
            if let Some(profile) = profiles.get(*name) {
                assert_eq!(
                    profile.weekday_fractions, profile.weekend_fractions,
                    "fixed-operation schedule '{name}' should keep identical fractions"
                );
            }
        }
    }

    #[test]
    fn spa_heater_month_multipliers_match_spa_pump() {
        use std::path::Path;
        let defaults_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../defaults");
        let profiles = super::load_default_profiles(&defaults_dir);

        let spa_pump = profiles
            .get("Spa Pump")
            .expect("Spa Pump profile must exist in defaults");
        let spa_heater = profiles
            .get("Spa Heater")
            .expect("Spa Heater profile must exist in defaults");

        assert_eq!(
            spa_pump.month_multipliers, spa_heater.month_multipliers,
            "spa heater month multipliers must match spa pump — both equipment share the \
             same seasonal usage pattern"
        );
    }
}
