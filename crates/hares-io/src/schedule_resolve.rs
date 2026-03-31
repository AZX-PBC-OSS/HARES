//! Resolve index-based schedule CSV columns into per-equipment kW schedules.
//!
//! Mirrors OCHRE's `SCHEDULE_NAMES` mapping and `convert_power_column` logic:
//! normalized schedule fractions are scaled to kW using
//! `max_kw = annual_kwh / 8760 / mean(fraction)`.

use std::collections::HashMap;
use std::path::Path;

use hares_equipment::ConfigPayload;
use hares_types::{BoundaryPolicy, ScheduleSourceConfig, normalize_ascii, parse_trimmed_f64};
use serde_json::Value;
use tracing::warn;

use crate::EquipmentSpec;
use crate::draw_profile::normalize_draw_profile;
use crate::schedule::{ColumnAggregation, ScheduleTimeSeries};

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
struct DefaultScheduleProfile {
    weekday_fractions: [f64; 24],
    weekend_fractions: [f64; 24],
    month_multipliers: [f64; 12],
}

/// Load default schedule profiles from `Default Schedule Parameters.csv`.
/// Returns a map keyed by "OCHRE Name" (e.g. "Indoor Lighting", "MELs").
fn load_default_profiles(defaults_dir: &Path) -> HashMap<String, DefaultScheduleProfile> {
    let csv_path = defaults_dir.join("Default Schedule Parameters.csv");
    let content = match std::fs::read_to_string(&csv_path) {
        Ok(c) => c,
        Err(_) => return HashMap::new(),
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
    inject_setpoint_schedules(specs, &csv_col_map, schedule);
}

/// HVAC equipment names that consume heating setpoints.
const HEATING_EQUIPMENT: &[&str] = &[
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
const COOLING_EQUIPMENT: &[&str] = &["ASHP Cooler", "MSHP Cooler", "Air Conditioner", "Room AC"];

fn inject_setpoint_schedules(
    specs: &mut [EquipmentSpec],
    csv_col_map: &HashMap<String, usize>,
    schedule: &mut ScheduleTimeSeries,
) {
    // Store only the column index — the equipment resolves the value each
    // timestep from the environment's schedule domain payload. No materialization.
    let heating_col = csv_col_map.get("heating_setpoint").copied();
    let cooling_col = csv_col_map.get("cooling_setpoint").copied();

    inject_water_heater_schedule_columns(specs, csv_col_map, schedule);

    if heating_col.is_none() && cooling_col.is_none() {
        return;
    }

    for spec in specs.iter_mut() {
        if HEATING_EQUIPMENT.contains(&spec.name.as_str())
            && let Some(col_idx) = heating_col
        {
            spec.parameters.insert(
                "heating_setpoint_schedule_col".to_string(),
                Value::from(col_idx as u64),
            );
            set_typed_setpoint_source(spec, "heating", col_idx);
        }
        if COOLING_EQUIPMENT.contains(&spec.name.as_str())
            && let Some(col_idx) = cooling_col
        {
            spec.parameters.insert(
                "cooling_setpoint_schedule_col".to_string(),
                Value::from(col_idx as u64),
            );
            set_typed_setpoint_source(spec, "cooling", col_idx);
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
        obj.insert(format!("{prefix}_setpoint_source"), json);
    }
}

/// Storage water heater equipment names that consume runtime draw/mains schedule columns.
const STORAGE_WATER_HEATER_EQUIPMENT: &[&str] = &[
    "Electric Resistance Water Heater",
    "Gas Water Heater",
    "Heat Pump Water Heater",
];

fn inject_water_heater_schedule_columns(
    specs: &mut [EquipmentSpec],
    csv_col_map: &HashMap<String, usize>,
    schedule: &mut ScheduleTimeSeries,
) {
    // ResStock/OCHRE commonly provide fixture draw fractions in `hot_water_fixtures`.
    // When the input is already pre-scaled in L/min, prefer the explicit SI-to-be-converted
    // alias `hot_water_delivered_l_min` and convert once to kg/s here.
    let draw_source = first_present_column(csv_col_map, &["hot_water_delivered_l_min"])
        .map(|col_idx| (col_idx, WaterHeaterDrawSource::PreScaledLMin))
        .or_else(|| {
            first_present_column(
                csv_col_map,
                &[
                    "hot_water_fixtures",
                    "hot_water_draw",
                    "hot_water_delivered",
                ],
            )
            .map(|col_idx| (col_idx, WaterHeaterDrawSource::Fraction))
        });
    let mains_col = first_present_column(
        csv_col_map,
        &[
            "hot_water_mains_temperature",
            "mains_temperature",
            "mains_temp",
            "water_mains_temp",
        ],
    );

    if draw_source.is_none() && mains_col.is_none() {
        return;
    }

    for (i, spec) in specs.iter_mut().enumerate() {
        if !STORAGE_WATER_HEATER_EQUIPMENT.contains(&spec.name.as_str()) {
            continue;
        }
        if let Some((col_idx, draw_source)) = draw_source {
            let raw_values = schedule.columns[col_idx].clone();
            let kg_s_series: Vec<f64> = match draw_source {
                WaterHeaterDrawSource::Fraction => {
                    // Raw fractions must be normalized to an SI mass-flow series.
                    let avg_daily_l = spec
                        .parameters
                        .get("avg_water_draw_l_per_day")
                        .and_then(|v| v.as_f64())
                        .unwrap_or_else(|| {
                            panic!(
                                "water heater spec '{}' missing or invalid avg_water_draw_l_per_day; \
                                 cannot normalize draw schedule fractions",
                                spec.name
                            )
                        });
                    normalize_draw_profile(&raw_values, avg_daily_l)
                }
                WaterHeaterDrawSource::PreScaledLMin => {
                    // The alias is already in L/min; convert once to kg/s.
                    raw_values.iter().map(|&v| (v.max(0.0)) / 60.0).collect()
                }
            };

            let col_name = format!(
                "hot_water_draw_kg_s_{}_{}",
                normalize_schedule_col_name(&spec.name),
                i
            );
            match schedule.append_derived_column(&col_name, kg_s_series, ColumnAggregation::Mean) {
                Ok(derived_col_idx) => {
                    spec.parameters.insert(
                        "draw_rate_schedule_col".to_string(),
                        Value::from(derived_col_idx as u64),
                    );
                    spec.parameters.insert(
                        "draw_flow_rate_schedule_col".to_string(),
                        Value::from(derived_col_idx as u64),
                    );
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
            spec.parameters.insert(
                "mains_temp_schedule_col".to_string(),
                Value::from(col_idx as u64),
            );
        }
    }
}

fn first_present_column(csv_col_map: &HashMap<String, usize>, names: &[&str]) -> Option<usize> {
    names
        .iter()
        .find_map(|name| csv_col_map.get(*name).copied())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaterHeaterDrawSource {
    Fraction,
    PreScaledLMin,
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
        // CSV column exists — use it directly and add a derived kW column.
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
        // No CSV column — prefer building-specific HPXML profile, then generic defaults.
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

    Some((annual_kwh / 8760.0) / mean_fraction)
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
                .map(|kwh| kwh / 8760.0)
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

        // Always inject the column index for the stochastic fallback path.
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    use chrono::{DateTime, Duration};
    use hares_equipment::ConfigPayload;
    use hares_types::BoundaryPolicy;
    use hares_types::FuelType;
    use serde_json::{Map, Value};
    use tempfile::tempdir;
    use tracing_subscriber::fmt::MakeWriter;

    use super::inject_schedule_into_specs;
    use crate::{EquipmentSpec, ScheduleTimeSeries};

    #[derive(Clone, Default)]
    struct SharedWriter(Arc<Mutex<Vec<u8>>>);

    impl<'a> MakeWriter<'a> for SharedWriter {
        type Writer = SharedWriterGuard;

        fn make_writer(&'a self) -> Self::Writer {
            SharedWriterGuard(self.0.clone())
        }
    }

    struct SharedWriterGuard(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedWriterGuard {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("writer lock poisoned")
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn capture_warnings<F>(f: F) -> String
    where
        F: FnOnce(),
    {
        let buffer = Arc::new(Mutex::new(Vec::<u8>::new()));
        let writer = SharedWriter(buffer.clone());
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(writer)
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        String::from_utf8(buffer.lock().expect("log lock poisoned").clone())
            .expect("logs must be valid utf8")
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
            name: name.to_string(),
            fuel_type: FuelType::Electric,
            parameters,
            zip_params: None,
            typed_config: None,
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
            name: name.to_string(),
            fuel_type: FuelType::Electric,
            parameters,
            zip_params: None,
            typed_config: None,
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
    fn missing_csv_column_uses_default_profile_and_logs_warning() {
        let dir = tempdir().expect("create temp dir");
        write_default_profile_csv(dir.path());

        let mut schedule = make_schedule(24);
        let mut specs = vec![make_spec("Indoor Lighting", 876.0)];

        let logs = capture_warnings(|| {
            inject_schedule_into_specs(&mut specs, &mut schedule, Some(dir.path()));
        });

        assert_eq!(
            specs[0]
                .parameters
                .get("power_schedule_source")
                .and_then(Value::as_str),
            Some("daily_profile")
        );
        assert!(logs.contains("using default profile"));
    }

    #[test]
    fn missing_csv_and_missing_default_profile_falls_back_to_constant_with_loud_warning() {
        let dir = tempdir().expect("create temp dir");

        let mut schedule = make_schedule(24);
        let mut specs = vec![make_spec("Indoor Lighting", 876.0)];
        let logs = capture_warnings(|| {
            inject_schedule_into_specs(&mut specs, &mut schedule, Some(dir.path()));
        });

        let expected = 876.0 / 8760.0;
        let constant_kw = specs[0]
            .parameters
            .get("power_constant_kw")
            .and_then(Value::as_f64)
            .expect("power_constant_kw must be set");
        assert!((constant_kw - expected).abs() < 1e-12);
        assert!(logs.contains("THIS MAY BE INCORRECT"));
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
    fn constant_fallback_injects_compact_constant_keys() {
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
            name: name.to_string(),
            fuel_type: FuelType::Electric,
            parameters: Map::new(),
            zip_params: None,
            typed_config: Some(hares_equipment::EquipmentConfig::from_typed(
                name.to_string(),
                ochre_class.to_string(),
                config,
            )),
        }
    }

    #[test]
    fn typed_hvac_specs_receive_column_ref_setpoint_sources() {
        use hares_equipment::hvac::heat_pump_config::{HeatPumpCoolerConfig, HeatPumpHeaterConfig};

        let mut schedule = make_schedule_with_setpoint_columns(&[20.0, 19.5], &[26.0, 25.5]);
        let mut specs = vec![
            make_typed_spec(
                "ASHP Heater",
                "ASHP Heater",
                HeatPumpHeaterConfig {
                    zone_id: Some(1),
                    ..Default::default()
                },
            ),
            make_typed_spec(
                "ASHP Cooler",
                "ASHP Cooler",
                HeatPumpCoolerConfig {
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
                    stage_shrs: None,
                    fraction_cooling_load_served: None,
                    number_of_speeds: 1,
                    is_mini_split: false,
                    shr: None,
                    fan_power_w: None,
                    fan_power_w_per_cfm: None,
                    airflow_m3_s_per_w: None,
                    heating_setpoint_c: None,
                    cooling_setpoint_c: None,
                    hysteresis_c: None,
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                    duct: hares_equipment::DuctConfig::default(),
                    biquadratic_x1_min: None,
                    biquadratic_x1_max: None,
                    biquadratic_x2_min: None,
                    biquadratic_x2_max: None,
                    ff_min: None,
                    ff_max: None,
                    plf_min: None,
                    plf_max: None,
                },
            ),
        ];

        inject_schedule_into_specs(&mut specs, &mut schedule, None);

        let heater_typed = specs[0]
            .typed_config
            .as_ref()
            .expect("heater typed config must remain present");
        let heater_data = match &heater_typed.payload {
            ConfigPayload::Typed { data, .. } => data,
            other => panic!("expected typed payload, got {other:?}"),
        };
        let heater_source: hares_types::ScheduleSourceConfig = serde_json::from_value(
            heater_data
                .get("heating_setpoint_source")
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
            ConfigPayload::Typed { data, .. } => data,
            other => panic!("expected typed payload, got {other:?}"),
        };
        let cooler_source: hares_types::ScheduleSourceConfig = serde_json::from_value(
            cooler_data
                .get("cooling_setpoint_source")
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
        let mut parameters = Map::new();
        parameters.insert("annual_electric_kwh".to_string(), Value::from(0.0));
        parameters.insert(
            "avg_water_draw_l_per_day".to_string(),
            Value::from(avg_water_draw_l_per_day),
        );
        EquipmentSpec {
            name: name.to_string(),
            fuel_type: FuelType::Electric,
            parameters,
            zip_params: None,
            typed_config: None,
        }
    }

    #[test]
    fn storage_water_heaters_receive_draw_and_mains_schedule_columns() {
        let mut schedule = make_schedule_with_water_heater_columns(
            "hot_water_fixtures",
            &[0.2, 0.0],
            &[11.0, 12.0],
        );
        let mut specs = vec![
            make_water_heater_spec("Electric Resistance Water Heater", 200.0),
            make_water_heater_spec("Gas Water Heater", 200.0),
            make_water_heater_spec("Heat Pump Water Heater", 200.0),
            make_spec("Tankless Water Heater", 0.0),
        ];

        inject_schedule_into_specs(&mut specs, &mut schedule, None);

        for spec in specs.iter().take(3) {
            let draw_col = spec
                .parameters
                .get("draw_flow_rate_schedule_col")
                .and_then(Value::as_u64);
            let draw_rate_col = spec
                .parameters
                .get("draw_rate_schedule_col")
                .and_then(Value::as_u64);
            let mains_col = spec
                .parameters
                .get("mains_temp_schedule_col")
                .and_then(Value::as_u64);
            assert!(
                draw_col.is_some(),
                "{} should receive draw_flow_rate_schedule_col",
                spec.name
            );
            // The draw column should point to a derived column, not the raw fraction column (0).
            assert!(
                draw_col.unwrap() >= 2,
                "{} draw_flow_rate_schedule_col should point to a derived column, got {}",
                spec.name,
                draw_col.unwrap()
            );
            assert_eq!(
                draw_rate_col, draw_col,
                "{} should mirror draw_rate_schedule_col and draw_flow_rate_schedule_col",
                spec.name
            );
            assert_eq!(
                mains_col,
                Some(1),
                "{} should receive mains_temp_schedule_col=1",
                spec.name
            );
        }

        assert!(
            !specs[3]
                .parameters
                .contains_key("draw_flow_rate_schedule_col"),
            "tankless should not receive storage draw schedule columns"
        );
        assert!(
            !specs[3].parameters.contains_key("mains_temp_schedule_col"),
            "tankless should not receive storage mains schedule columns"
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
            .parameters
            .get("draw_flow_rate_schedule_col")
            .and_then(Value::as_u64)
            .expect("draw_flow_rate_schedule_col must be present")
            as usize;

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
    fn water_heater_delivered_l_min_converted_once_to_kg_s() {
        let delivered_l_min = vec![6.0, 12.0, 0.0, 3.0];
        let mains = vec![12.0; delivered_l_min.len()];
        let mut schedule = make_schedule_with_water_heater_columns(
            "hot_water_delivered_l_min",
            &delivered_l_min,
            &mains,
        );
        let mut specs = vec![make_water_heater_spec(
            "Electric Resistance Water Heater",
            200.0,
        )];

        inject_schedule_into_specs(&mut specs, &mut schedule, None);

        let draw_col_idx = specs[0]
            .parameters
            .get("draw_flow_rate_schedule_col")
            .and_then(Value::as_u64)
            .expect("draw_flow_rate_schedule_col must be present")
            as usize;

        let resolved = &schedule.columns[draw_col_idx];
        assert_eq!(resolved.len(), delivered_l_min.len());

        for (i, &l_min) in delivered_l_min.iter().enumerate() {
            let expected_kg_s = (l_min.max(0.0)) / 60.0;
            assert!(
                (resolved[i] - expected_kg_s).abs() < 1e-12,
                "timestep {i}: expected {expected_kg_s:.8e} kg/s, got {:.8e}",
                resolved[i]
            );
        }

        assert_eq!(
            specs[0]
                .parameters
                .get("draw_rate_schedule_col")
                .and_then(Value::as_u64),
            Some(draw_col_idx as u64),
            "SI draw column index must be mirrored to draw_rate_schedule_col"
        );
    }
}
