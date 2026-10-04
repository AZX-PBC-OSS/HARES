//! Resolve index-based schedule CSV columns into per-equipment kW schedules.
//!
//! Mirrors OCHRE's `SCHEDULE_NAMES` mapping and `convert_power_column` logic:
//! normalized schedule fractions are scaled to kW using
//! `max_kw = annual_kwh / 8760 / mean(fraction)`.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use chrono::{Datelike, Timelike};
use hares_equipment::{
    ConfigPayload, ElectricResistanceWaterHeaterConfig, GasWaterHeaterConfig,
    HeatPumpWaterHeaterConfig, TanklessWaterHeaterConfig,
};
use hares_physics::constants::HOURS_PER_YEAR;
use hares_types::{
    BoundaryPolicy, FuelType, HaresError, ScheduleSourceConfig, Warning, normalize_ascii,
    parse_trimmed_f64,
};
use serde_json::{Map, Value};
use tracing::warn;

use crate::EquipmentSpec;
use crate::defaults::DefaultsStore;
use crate::draw_profile::normalize_draw_profile;
use crate::hpxml::{MICROWAVE_DEFAULT_ANNUAL_KWH, build_spec};
use crate::schedule::{ColumnAggregation, ScheduleTimeSeries, resolve_occupancy_column};

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
    /// Read by `inject_water_heater_schedule_columns` (the DHW draw and the
    /// mains temperature), never through `mapping_by_equipment`.
    WaterHeater,
    /// A column no code reads; the reason states why the engine does not
    /// consume it, so a column the mapping table should know about is never
    /// silently ignored.
    NotUsed {
        reason: &'static str,
    },
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
    ColumnMapping {
        csv_column: "microwave",
        equipment_name: "Microwave",
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
        category: ScheduleCategory::NotUsed {
            reason: "HARES models one refrigerator, the `refrigerator` column; \
                     a second unit's load is not modelled separately",
        },
    },
    ColumnMapping {
        csv_column: "clothes_dryer_exhaust",
        equipment_name: "",
        category: ScheduleCategory::NotUsed {
            reason: "the clothes dryer's exhaust air heat is not modelled",
        },
    },
    ColumnMapping {
        csv_column: "lighting_exterior_holiday",
        equipment_name: "",
        category: ScheduleCategory::NotUsed {
            reason: "holiday lighting is not modelled",
        },
    },
    ColumnMapping {
        csv_column: "plug_loads_vehicle",
        equipment_name: "",
        category: ScheduleCategory::NotUsed {
            reason: "vehicle charging is modelled by the EV equipment, whose \
                     schedule comes from the ev_driver actor's trip model",
        },
    },
    ColumnMapping {
        csv_column: "battery",
        equipment_name: "",
        category: ScheduleCategory::NotUsed {
            reason: "the battery follows its own control strategy, not a schedule",
        },
    },
    ColumnMapping {
        csv_column: "vacancy",
        equipment_name: "",
        category: ScheduleCategory::NotUsed {
            reason: "unavailable periods are not modelled",
        },
    },
    ColumnMapping {
        csv_column: "water_heater_operating_mode",
        equipment_name: "",
        category: ScheduleCategory::NotUsed {
            reason: "the water heater's operating mode follows its own controller",
        },
    },
    ColumnMapping {
        csv_column: "power_outage",
        equipment_name: "",
        category: ScheduleCategory::NotUsed {
            reason: "unavailable periods are not modelled",
        },
    },
    ColumnMapping {
        csv_column: "no_space_heating",
        equipment_name: "",
        category: ScheduleCategory::NotUsed {
            reason: "unavailable periods are not modelled",
        },
    },
    ColumnMapping {
        csv_column: "no_space_cooling",
        equipment_name: "",
        category: ScheduleCategory::NotUsed {
            reason: "unavailable periods are not modelled",
        },
    },
    // Water heater: read by inject_water_heater_schedule_columns, which no
    // table entry declared before.
    ColumnMapping {
        csv_column: "hot_water_fixtures",
        equipment_name: "Water Heating",
        category: ScheduleCategory::WaterHeater,
    },
    ColumnMapping {
        csv_column: "hot_water_mains_temperature",
        equipment_name: "Water Heating",
        category: ScheduleCategory::WaterHeater,
    },
    // Wet appliances draw their own hot water on the DHW demand loop during
    // the water-draw phases of the cycles the dishwasher and clothes_washer
    // event-window columns schedule; consuming the hot-water column as well
    // would count the draw twice.
    ColumnMapping {
        csv_column: "hot_water_dishwasher",
        equipment_name: "",
        category: ScheduleCategory::NotUsed {
            reason: "the dishwasher draws its own hot water on the DHW demand \
                     loop during the draw phases of the `dishwasher` column's \
                     cycles; consuming this column as well would count the \
                     draw twice",
        },
    },
    ColumnMapping {
        csv_column: "hot_water_clothes_washer",
        equipment_name: "",
        category: ScheduleCategory::NotUsed {
            reason: "the clothes washer draws its own hot water on the DHW \
                     demand loop during the draw phases of the \
                     `clothes_washer` column's cycles; consuming this column \
                     as well would count the draw twice",
        },
    },
    // Charging and driving times come from the ev_driver actor's trip model,
    // not an OpenStudio-HPXML schedule.
    ColumnMapping {
        csv_column: "electric_vehicle_charging",
        equipment_name: "",
        category: ScheduleCategory::NotUsed {
            reason: "EV charging times come from the ev_driver actor's trip \
                     model, not an OpenStudio-HPXML schedule",
        },
    },
    ColumnMapping {
        csv_column: "electric_vehicle_discharging",
        equipment_name: "",
        category: ScheduleCategory::NotUsed {
            reason: "EV discharging (driving) times come from the ev_driver \
                     actor's trip model, not an OpenStudio-HPXML schedule",
        },
    },
];

// ---------------------------------------------------------------------------
// Default schedule profiles (weekday/weekend fractions + monthly multipliers)
// ---------------------------------------------------------------------------

/// The file inside the defaults directory that holds the default schedule
/// profiles.
pub const DEFAULT_SCHEDULES_CSV: &str = "Default Schedule Parameters.csv";

#[derive(Debug, Clone)]
pub struct DefaultScheduleProfile {
    /// The 24 hourly fractions for weekdays.
    pub weekday_fractions: [f64; 24],
    /// The 24 hourly fractions for weekends.
    pub weekend_fractions: [f64; 24],
    /// The 12 monthly multipliers applied on top of the hourly fractions.
    pub month_multipliers: [f64; 12],
}

/// The default schedule profiles parsed from `Default Schedule Parameters.csv`
/// in the configured defaults directory. The `Default` value is the empty
/// set a config with no defaults directory resolves against: it has no
/// file, and every lookup names the `defaults_path` setting.
#[derive(Debug, Clone, Default)]
pub struct DefaultProfiles {
    profiles: HashMap<String, DefaultScheduleProfile>,
    csv_path: Option<PathBuf>,
}

impl DefaultProfiles {
    /// The profile named `name`, or an error naming the missing profile and
    /// the file it was expected in.
    pub fn get(&self, name: &str) -> Result<&DefaultScheduleProfile, HaresError> {
        self.profiles
            .get(name)
            .ok_or_else(|| HaresError::Io(self.missing_profile(name)))
    }

    /// The profile named `name`, if present: the probe for fallback chains
    /// that have another source to try.
    pub(crate) fn find(&self, name: &str) -> Option<&DefaultScheduleProfile> {
        self.profiles.get(name)
    }

    /// Why no profile named `name` is available, for error messages: the
    /// file it is missing from, or the unset defaults directory.
    pub(crate) fn missing_profile(&self, name: &str) -> String {
        match &self.csv_path {
            Some(path) => format!("no '{name}' profile in '{}'", path.display()),
            None => format!(
                "no '{name}' profile, because no defaults directory is configured to \
                 load '{DEFAULT_SCHEDULES_CSV}' from; set defaults_path"
            ),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }
}

/// One parsed row of the default schedule profiles file, for the error a
/// second row of the same profile and element raises.
struct ProfileRow<'a> {
    csv_path: &'a Path,
    profile: &'a str,
    element: &'a str,
    line_no: usize,
}

impl ProfileRow<'_> {
    fn insert_into<const N: usize>(
        &self,
        map: &mut HashMap<String, (usize, [f64; N])>,
        values: [f64; N],
    ) -> Result<(), HaresError> {
        match map.entry(self.profile.to_string()) {
            std::collections::hash_map::Entry::Occupied(first) => Err(HaresError::Io(format!(
                "'{}' line {}: profile '{}' element '{}' repeats the row at line {}",
                self.csv_path.display(),
                self.line_no,
                self.profile,
                self.element,
                first.get().0,
            ))),
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert((self.line_no, values));
                Ok(())
            }
        }
    }
}

/// Load the default schedule profiles from
/// `Default Schedule Parameters.csv` in `defaults_dir`, keyed by "OCHRE Name"
/// (e.g. "Indoor Lighting", "MELs").
///
/// Strict: an unreadable file, a row with fewer than five fields, a value
/// that does not parse, a value count other than 24 (fractions) or 12
/// (monthly multipliers), a second row for the same profile and element,
/// and a profile missing any of its
/// `weekday_fractions` / `weekend_fractions` / `month_multipliers` rows are
/// all errors naming the file, the line, the profile and the element.
pub fn load_default_profiles(defaults_dir: &Path) -> Result<DefaultProfiles, HaresError> {
    let csv_path = defaults_dir.join(DEFAULT_SCHEDULES_CSV);
    let content = std::fs::read_to_string(&csv_path).map_err(|e| {
        HaresError::Io(format!(
            "default schedule profiles file '{}' could not be read: {e}",
            csv_path.display()
        ))
    })?;

    // Intermediate: the values and source line per (ochre_name, element_kind)
    let mut weekday_map: HashMap<String, (usize, [f64; 24])> = HashMap::new();
    let mut weekend_map: HashMap<String, (usize, [f64; 24])> = HashMap::new();
    let mut month_map: HashMap<String, (usize, [f64; 12])> = HashMap::new();

    for (idx, line) in content.lines().enumerate().skip(1) {
        let line_no = idx + 1;

        // Parse CSV line handling quoted "Values" field
        let fields = parse_csv_line(line);
        if fields.len() < 5 {
            return Err(HaresError::Io(format!(
                "'{}' line {line_no}: row has {} fields, expected at least 5 \
                 (Schedule Name, Element, OCHRE Name, OCHRE Element, Values, Data Source): '{line}'",
                csv_path.display(),
                fields.len(),
            )));
        }

        let profile_name = fields[2].trim();
        if profile_name.is_empty() || profile_name == "N/A" {
            continue;
        }
        let element = fields[3].trim();
        let values_str = fields[4].trim();

        let mut values: Vec<f64> = Vec::new();
        for token in values_str.split(',') {
            let Some(value) = parse_trimmed_f64(token) else {
                return Err(HaresError::Io(format!(
                    "'{}' line {line_no}: profile '{profile_name}' element '{element}': \
                     value '{token}' does not parse as a finite number",
                    csv_path.display(),
                )));
            };
            values.push(value);
        }

        let value_count = values.len();
        let wrong_count = |expected: usize| {
            HaresError::Io(format!(
                "'{}' line {line_no}: profile '{profile_name}' element '{element}': \
                 {value_count} values, expected {expected}",
                csv_path.display(),
            ))
        };
        let row = ProfileRow {
            csv_path: &csv_path,
            profile: profile_name,
            element,
            line_no,
        };
        match element {
            "weekday_fractions" => {
                let arr: [f64; 24] = values.try_into().map_err(|_| wrong_count(24))?;
                row.insert_into(&mut weekday_map, arr)?;
            }
            "weekend_fractions" => {
                let arr: [f64; 24] = values.try_into().map_err(|_| wrong_count(24))?;
                row.insert_into(&mut weekend_map, arr)?;
            }
            "month_multipliers" => {
                let arr: [f64; 12] = values.try_into().map_err(|_| wrong_count(12))?;
                row.insert_into(&mut month_map, arr)?;
            }
            other => {
                return Err(HaresError::Io(format!(
                    "'{}' line {line_no}: profile '{profile_name}' has unknown \
                     element '{other}': expected weekday_fractions, weekend_fractions \
                     or month_multipliers",
                    csv_path.display(),
                )));
            }
        }
    }

    // Assemble profiles for every name that has any of the three rows; every
    // profile needs all three, and a missing one is an error naming the
    // profile. Sorted for a deterministic error when several are incomplete.
    let mut names: BTreeSet<String> = BTreeSet::new();
    names.extend(weekday_map.keys().cloned());
    names.extend(weekend_map.keys().cloned());
    names.extend(month_map.keys().cloned());
    let mut profiles = HashMap::new();
    for name in &names {
        let missing = |element: &str| {
            HaresError::Io(format!(
                "'{}': profile '{name}' has no {element} row",
                csv_path.display(),
            ))
        };
        let (_, weekday) = weekday_map
            .get(name)
            .ok_or_else(|| missing("weekday_fractions"))?;
        let (_, weekend) = weekend_map
            .get(name)
            .ok_or_else(|| missing("weekend_fractions"))?;
        let (_, months) = month_map
            .get(name)
            .ok_or_else(|| missing("month_multipliers"))?;
        profiles.insert(
            name.clone(),
            DefaultScheduleProfile {
                weekday_fractions: *weekday,
                weekend_fractions: *weekend,
                month_multipliers: *months,
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
    // was imported verbatim from the ANSI 301 source without weekend
    // derivation; the shipped default schedule CSV is pinned by the unit
    // test `shipped_default_occupancy_has_distinct_weekend`.
    Ok(DefaultProfiles {
        profiles,
        csv_path: Some(csv_path),
    })
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
    specs: &mut Vec<EquipmentSpec>,
    schedule: &mut ScheduleTimeSeries,
    defaults_path: Option<&Path>,
    defaults: &DefaultsStore,
    foundation_name: Option<&str>,
    garage_modeled: bool,
    warnings: &mut Vec<Warning>,
) -> Result<(), HaresError> {
    let mut csv_col_map: HashMap<String, usize> = schedule
        .column_names
        .iter()
        .enumerate()
        .map(|(i, name)| (name.clone(), i))
        .collect();

    // OCHRE schedule.py:390-391: copy zone-specific schedules when the
    // schedule file lacks them — Basement Lighting follows the interior
    // lighting column when `lighting_basement` is absent but
    // `lighting_interior` is present. A Basement Lighting spec can only
    // exist for a Finished Basement foundation (gated in resolve_loads and
    // ensure_specs_for_csv_columns), so no extra foundation check is needed.
    if specs.iter().any(|s| s.name == "Basement Lighting")
        && !csv_col_map.contains_key("lighting_basement")
        && let Some(&interior_idx) = csv_col_map.get("lighting_interior")
    {
        let values = schedule.columns[interior_idx].clone();
        let aggregation = schedule
            .column_aggregations
            .get(interior_idx)
            .copied()
            .unwrap_or(ColumnAggregation::Mean);
        match schedule.add_column("lighting_basement", values, aggregation) {
            Ok(()) => {
                if let Some(&idx) = schedule.column_index.get("lighting_basement") {
                    csv_col_map.insert("lighting_basement".to_string(), idx);
                }
            }
            Err(error) => {
                warn!(
                    %error,
                    "failed to copy lighting_interior column to lighting_basement; \
                         Basement Lighting will fall back to other schedule sources"
                );
            }
        }
    }

    // Invariant: check for unknown CSV columns before any processing. A
    // column neither the mapping table nor the environment reads is reported
    // once per column as a `Warning`, so a column the mapping table should
    // know about is never silently ignored.
    {
        let unknown = find_unknown_schedule_columns(&csv_col_map);
        for col_name in &unknown {
            warnings.push(Warning::new(
                "schedule",
                format!(
                    "schedule CSV column '{col_name}' has no entry in COLUMN_MAPPINGS \
                     and the environment does not read it; HARES does not read it"
                ),
            ));
        }
    }

    // Ensure specs exist for CSV columns that have column mappings but
    // no corresponding spec from HPXML parsing (e.g. microwave).
    ensure_specs_for_csv_columns(
        specs,
        &csv_col_map,
        &schedule.columns,
        defaults,
        foundation_name,
        garage_modeled,
        warnings,
    );

    let profiles = match defaults_path {
        Some(dir) => load_default_profiles(dir)?,
        None => DefaultProfiles::default(),
    };

    let mapping_by_equipment: HashMap<&str, &ColumnMapping> = COLUMN_MAPPINGS
        .iter()
        .filter(|m| {
            matches!(
                m.category,
                ScheduleCategory::Power
                    | ScheduleCategory::EventWindow
                    | ScheduleCategory::Occupancy
            )
        })
        .map(|m| (m.equipment_name, m))
        .collect();

    // Invariant: if HPXML-derived pool/spa specs have annual energy but the
    // schedule CSV also has pool/spa columns, the CSV fractions will override
    // the HPXML extension fractions (while HPXML annual energy is used for
    // max_kW scaling). This is the documented precedence; no warning.

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
                inject_power_schedule(spec, mapping, &csv_col_map, schedule, &profiles)?;
            }
            ScheduleCategory::EventWindow => {
                inject_event_schedule(spec, mapping, &csv_col_map, schedule);
            }
            ScheduleCategory::Occupancy => {
                inject_occupancy_schedule(spec, schedule, &profiles);
            }
            ScheduleCategory::Setpoint | ScheduleCategory::WaterHeater => {}
            ScheduleCategory::NotUsed { .. } => {}
        }
    }

    // Setpoint columns: inject per-timestep arrays from schedule CSV into
    // any heating/cooling HVAC equipment. The CSV column `heating_setpoint`
    // maps to all heating equipment, `cooling_setpoint` to all cooling.
    // When neither a schedule CSV column nor HPXML-derived setpoints are
    // present, falls back to the HERS reference-home default profiles
    // loaded from Default Schedule Parameters.csv.
    inject_setpoint_schedules(specs, &csv_col_map, schedule, &profiles)?;
    Ok(())
}

/// HVAC equipment names that consume heating setpoints.
pub(crate) const HEATING_EQUIPMENT: &[&str] = &[
    "ASHP Heater",
    "MSHP Heater",
    "GSHP Heater",
    "WSHP Heater",
    "Gas Furnace",
    "Electric Furnace",
    "Oil Furnace",
    "Electric Baseboard",
    "Gas Boiler",
    "Electric Boiler",
    "Oil Boiler",
];

/// HVAC equipment names that consume cooling setpoints.
pub(crate) const COOLING_EQUIPMENT: &[&str] = &[
    "ASHP Cooler",
    "MSHP Cooler",
    "GSHP Cooler",
    "WSHP Cooler",
    "Air Conditioner",
    "Room AC",
];

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
    if let Some(sp) = obj.get("setpoint")
        && let Some(v) = sp.get(key)
    {
        return Some(v);
    }
    None
}

fn inject_default_setpoint_profile(
    spec: &mut EquipmentSpec,
    ochre_name: &str,
    prefix: &str,
    profiles: &DefaultProfiles,
) {
    let Some(profile) = profiles.find(ochre_name) else {
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
    if let Some(sp) = obj.get_mut("setpoint")
        && let Some(sp_obj) = sp.as_object_mut()
    {
        sp_obj.insert(key.to_string(), value);
    }
}

fn inject_setpoint_schedules(
    specs: &mut [EquipmentSpec],
    csv_col_map: &HashMap<String, usize>,
    schedule: &mut ScheduleTimeSeries,
    profiles: &DefaultProfiles,
) -> Result<(), HaresError> {
    // Store only the column index -- the equipment resolves the value each
    // timestep from the environment's schedule domain payload. No materialization.
    let heating_col = csv_col_map.get("heating_setpoint").copied();
    let cooling_col = csv_col_map.get("cooling_setpoint").copied();

    inject_water_heater_schedule_columns(specs, csv_col_map, schedule)?;

    // Skip the entire loop only when there is no work to do at all.
    if heating_col.is_none() && cooling_col.is_none() && profiles.is_empty() {
        return Ok(());
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
    Ok(())
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
) -> Result<(), HaresError> {
    let draw_col = first_present_column(csv_col_map, &["hot_water_fixtures"]);
    let mains_col = first_present_column(csv_col_map, &["hot_water_mains_temperature"]);

    if draw_col.is_none() && mains_col.is_none() {
        return Ok(());
    }

    for (i, spec) in specs.iter_mut().enumerate() {
        if !STORAGE_WATER_HEATER_EQUIPMENT.contains(&spec.name.as_str()) {
            continue;
        }
        let has_typed_sources = !matches!(spec.name.as_str(), "Heat Pump Water Heater");
        if let Some(col_idx) = draw_col {
            let raw_values = schedule.columns[col_idx].clone();
            let avg_daily_l = water_heater_avg_daily_draw_l(spec)?;
            let kg_s_series: Vec<f64> = normalize_draw_profile(&raw_values, avg_daily_l);

            let col_name = format!(
                "hot_water_draw_kg_s_{}_{}",
                normalize_schedule_col_name(&spec.name),
                i
            );
            let derived_col_idx = schedule
                .append_derived_column(&col_name, kg_s_series, ColumnAggregation::Mean)
                .map_err(|err| {
                    let msg = format!(
                        "failed to append normalized draw column for {}: {err}",
                        spec.name
                    );
                    tracing::error!(equipment = %spec.name, error = %err, "{msg}");
                    HaresError::Io(msg)
                })?;
            if has_typed_sources {
                set_typed_schedule_source(spec, "draw_flow_rate_source", derived_col_idx);
            }
        }
        if let Some(col_idx) = mains_col
            && has_typed_sources
        {
            set_typed_schedule_source(spec, "mains_temp_c_source", col_idx);
        }
    }
    Ok(())
}

fn tankless_avg_daily_draw_l(spec: &EquipmentSpec) -> Result<f64, HaresError> {
    let typed = spec.typed_config.as_ref().ok_or_else(|| {
        let msg = format!(
            "tankless water heater '{}' requires typed config",
            spec.name
        );
        tracing::error!(equipment = %spec.name, "{msg}");
        HaresError::Equipment(msg)
    })?;
    let tankless = typed.typed::<TanklessWaterHeaterConfig>().map_err(|err| {
        let msg = format!(
            "tankless water heater '{}' typed config failed to decode: {err}",
            spec.name
        );
        tracing::error!(equipment = %spec.name, error = %err, "{msg}");
        HaresError::Equipment(msg)
    })?;
    tankless.avg_water_draw_l_per_day.ok_or_else(|| {
        let msg = format!(
            "tankless water heater '{}' requires typed avg_water_draw_l_per_day to normalize draw schedule fractions",
            spec.name
        );
        tracing::error!(equipment = %spec.name, "{msg}");
        HaresError::Equipment(msg)
    })
}

fn water_heater_avg_daily_draw_l(spec: &EquipmentSpec) -> Result<f64, HaresError> {
    match spec.name.as_str() {
        "Tankless Water Heater" | "Gas Tankless Water Heater" => tankless_avg_daily_draw_l(spec),
        "Electric Resistance Water Heater" => {
            let typed = spec.typed_config.as_ref().ok_or_else(|| {
                let msg = format!(
                    "electric resistance water heater '{}' requires typed config",
                    spec.name
                );
                tracing::error!(equipment = %spec.name, "{msg}");
                HaresError::Equipment(msg)
            })?;
            let cfg = typed
                .typed::<ElectricResistanceWaterHeaterConfig>()
                .map_err(|err| {
                    let msg = format!(
                        "electric resistance water heater '{}' typed config failed to decode: {err}",
                        spec.name
                    );
                    tracing::error!(equipment = %spec.name, error = %err, "{msg}");
                    HaresError::Equipment(msg)
                })?;
            cfg.avg_water_draw_l_per_day.ok_or_else(|| {
                let msg = format!(
                    "electric resistance water heater '{}' requires typed avg_water_draw_l_per_day",
                    spec.name
                );
                tracing::error!(equipment = %spec.name, "{msg}");
                HaresError::Equipment(msg)
            })
        }
        "Gas Water Heater" => {
            let typed = spec.typed_config.as_ref().ok_or_else(|| {
                let msg = format!("gas water heater '{}' requires typed config", spec.name);
                tracing::error!(equipment = %spec.name, "{msg}");
                HaresError::Equipment(msg)
            })?;
            let cfg = typed.typed::<GasWaterHeaterConfig>().map_err(|err| {
                let msg = format!(
                    "gas water heater '{}' typed config failed to decode: {err}",
                    spec.name
                );
                tracing::error!(equipment = %spec.name, error = %err, "{msg}");
                HaresError::Equipment(msg)
            })?;
            cfg.avg_water_draw_l_per_day.ok_or_else(|| {
                let msg = format!(
                    "gas water heater '{}' requires typed avg_water_draw_l_per_day",
                    spec.name
                );
                tracing::error!(equipment = %spec.name, "{msg}");
                HaresError::Equipment(msg)
            })
        }
        "Heat Pump Water Heater" => {
            let typed = spec.typed_config.as_ref().ok_or_else(|| {
                let msg = format!(
                    "heat pump water heater '{}' requires typed config",
                    spec.name
                );
                tracing::error!(equipment = %spec.name, "{msg}");
                HaresError::Equipment(msg)
            })?;
            let cfg = typed.typed::<HeatPumpWaterHeaterConfig>().map_err(|err| {
                let msg = format!(
                    "heat pump water heater '{}' typed config failed to decode: {err}",
                    spec.name
                );
                tracing::error!(equipment = %spec.name, error = %err, "{msg}");
                HaresError::Equipment(msg)
            })?;
            cfg.avg_water_draw_l_per_day.ok_or_else(|| {
                let msg = format!(
                    "heat pump water heater '{}' requires typed avg_water_draw_l_per_day",
                    spec.name
                );
                tracing::error!(equipment = %spec.name, "{msg}");
                HaresError::Equipment(msg)
            })
        }
        other => Err({
            let msg = format!("unsupported water heater type for draw normalization: {other}");
            tracing::error!(equipment = %other, "{msg}");
            HaresError::Equipment(msg)
        }),
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
    profiles: &DefaultProfiles,
) -> Result<(), HaresError> {
    // Skip if equipment already has power schedule source keys.
    if spec.parameters.keys().any(|k| {
        k.starts_with("power_schedule_")
            || k.starts_with("power_profile_")
            || k == "power_constant_kw"
    }) {
        return Ok(());
    }

    let col_name = normalize_schedule_col_name(mapping.csv_column);
    let schedule_len = schedule.len();

    if let Some(&col_idx) = csv_col_map.get(col_name.as_str()) {
        // CSV column exists -- use it directly and add a derived kW column.
        let fraction_series = schedule.columns[col_idx].clone();
        if fraction_series.is_empty() {
            return Ok(());
        }

        let mean_fraction: f64 =
            fraction_series.iter().copied().sum::<f64>() / fraction_series.len() as f64;

        let max_kw = resolve_max_kw(
            spec,
            mean_fraction,
            &format!("the schedule column '{col_name}'"),
        )?;

        let kw_series: Vec<f64> = fraction_series.iter().map(|f| f * max_kw).collect();
        let derived_col_idx = schedule
            .append_derived_column(
                &format!(
                    "power_schedule_kw_{}",
                    normalize_schedule_col_name(spec.name.as_str())
                ),
                kw_series,
                ColumnAggregation::Mean,
            )
            .map_err(|err| {
                let msg = format!(
                    "failed to append derived kW column for {}: {err}",
                    spec.name
                );
                tracing::error!(equipment = %spec.name, error = %err, "{msg}");
                HaresError::Io(msg)
            })?;
        inject_compact_column_power(spec, derived_col_idx);
    } else if schedule_len > 0 {
        // No CSV column -- prefer building-specific HPXML profile, then generic defaults.
        if let Some(profile) = resolve_hpxml_profile(spec) {
            warn!(
                "schedule_resolve: no CSV column '{}' for '{}'; using HPXML profile fractions",
                col_name, mapping.equipment_name
            );
            let max_kw = resolve_max_kw(
                spec,
                annual_mean_fraction(&profile),
                "the HPXML schedule fractions on the spec",
            )?;
            inject_compact_profile_power(spec, &profile, max_kw);
        } else if let Some(profile) = profiles.find(mapping.equipment_name) {
            warn!(
                "schedule_resolve: no CSV column '{}' for '{}'; using default profile",
                col_name, mapping.equipment_name
            );
            let max_kw = resolve_max_kw(
                spec,
                annual_mean_fraction(profile),
                &format!(
                    "the default schedule profile for '{}'",
                    mapping.equipment_name
                ),
            )?;
            inject_compact_profile_power(spec, profile, max_kw);
        } else {
            return Err(HaresError::Io(format!(
                "equipment '{}' has no schedule source: no '{}' column in the \
                 schedule file, no HPXML schedule fractions on the spec, and {}",
                spec.name,
                col_name,
                profiles.missing_profile(mapping.equipment_name),
            )));
        }
    }
    Ok(())
}

/// Inject occupancy schedule data into the schedule timeseries using a three-tier
/// fallback chain that mirrors OCHRE's `import_occupancy_schedule` in
/// `vendors/OCHRE/ochre/utils/schedule.py:428-436`:
///
/// 1. CSV "occupants" column already present in schedule — nothing to generate.
/// 2. HPXML extension-derived `weekday_schedule_fractions` / `weekend_schedule_fractions` /
///    `month_multipliers` on the Occupancy spec — generate 8760-hour timeseries.
/// 3. Default "Occupancy" schedule profile from `Default Schedule Parameters.csv` —
///    generate 8760-hour timeseries.
///
/// The generated column uses the same "occupants" name so the dwelling picks it up
/// via `occupancy_column_idx()` without any special-casing.
fn inject_occupancy_schedule(
    spec: &mut EquipmentSpec,
    schedule: &mut ScheduleTimeSeries,
    profiles: &DefaultProfiles,
) {
    // If an "occupants" column already exists in the schedule — from any source
    // (CSV, previously generated by another spec, or HPXML schedule generation) —
    // the dwelling will read it. Nothing more to do beyond recording the source.
    if schedule.column_index.contains_key("occupants") {
        spec.parameters.insert(
            "occupancy_schedule_source".to_string(),
            Value::from("csv_column"),
        );
        #[cfg(feature = "observe")]
        {
            let col_idx = schedule.column_index["occupants"];
            tracing::info!(
                equipment = %spec.name,
                col_idx,
                "occupancy schedule resolved from existing column 'occupants'"
            );
        }
        return;
    }

    // Tier 2: HPXML extension fractions (weekday/weekend/month) from Occupancy spec.
    if let Some(profile) = resolve_hpxml_profile(spec) {
        let values = generate_occupancy_timeseries(schedule, &profile);
        match schedule.add_column(
            "occupants",
            values,
            crate::schedule::ColumnAggregation::Mean,
        ) {
            Ok(_) => {
                spec.parameters.insert(
                    "occupancy_schedule_source".to_string(),
                    Value::from("hpxml_extension"),
                );
                #[cfg(feature = "observe")]
                tracing::info!(
                    equipment = %spec.name,
                    "occupancy schedule generated from HPXML extension fractions"
                );
            }
            Err(e) => {
                warn!(
                    equipment = %spec.name,
                    error = %e,
                    "failed to add HPXML-generated occupancy column to schedule"
                );
            }
        }
        return;
    }

    // Tier 3: Default "Occupancy" schedule profile.
    if let Some(profile) = profiles.find("Occupancy") {
        let values = generate_occupancy_timeseries(schedule, profile);
        match schedule.add_column(
            "occupants",
            values,
            crate::schedule::ColumnAggregation::Mean,
        ) {
            Ok(_) => {
                spec.parameters.insert(
                    "occupancy_schedule_source".to_string(),
                    Value::from("default_profile"),
                );
                #[cfg(feature = "observe")]
                tracing::info!(
                    equipment = %spec.name,
                    "occupancy schedule generated from default 'Occupancy' profile"
                );
            }
            Err(e) => {
                warn!(
                    equipment = %spec.name,
                    error = %e,
                    "failed to add default-profile occupancy column to schedule"
                );
            }
        }
        return;
    }

    // No source available — the dwelling constructor's invariant check will
    // hard-error because occupancy_column_idx() returns None.
    warn!(
        equipment = %spec.name,
        "no occupancy schedule source available; dwelling construction will error"
    );
}

/// Generate an occupancy fraction timeseries from a schedule profile.
///
/// For each timestep in the schedule, computes:
///   value = hourly_fraction(weekday or weekend) × month_multiplier
///
/// This mirrors OCHRE's `create_simple_schedule` which produces
/// `w_fracs × m_fracs` indexed by (month, hour, weekday) in
/// `vendors/OCHRE/ochre/utils/schedule.py:283-305`.
fn generate_occupancy_timeseries(
    schedule: &ScheduleTimeSeries,
    profile: &DefaultScheduleProfile,
) -> Vec<f64> {
    let n_steps = schedule.len();
    let mut values = Vec::with_capacity(n_steps);
    for ts in &schedule.timestamps {
        let hour = ts.hour() as usize;
        let month = ts.month0() as usize;
        let is_weekend = ts.weekday().num_days_from_monday() >= 5;
        let frac = if is_weekend {
            profile.weekend_fractions[hour]
        } else {
            profile.weekday_fractions[hour]
        };
        values.push(frac * profile.month_multipliers[month]);
    }
    values
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

    if let Some(ref wd) = hpxml_weekday
        && !wd.is_empty()
    {
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

/// Determine an equipment's peak power from its schedule source, or fail
/// when the power is undeterminable.
///
/// Two zeros are determined, not undeterminable: an all-zero fraction
/// series (the schedule never runs the equipment, so the spec's annual
/// energy is never drawn), and a spec that declares its annual energy as
/// zero (there is no energy to scale). An equipment whose schedule does
/// run it and which declares neither a rated power (`max_electric_power_w`)
/// nor annual energy (`annual_electric_kwh`, `annual_gas_therms`) has no
/// determinable power: the returned error names the equipment and the
/// missing field.
fn resolve_max_kw(
    spec: &EquipmentSpec,
    mean_fraction: f64,
    source: &str,
) -> Result<f64, HaresError> {
    match determine_max_kw(spec, mean_fraction) {
        Some(max_kw) => Ok(max_kw),
        None if mean_fraction <= 0.0 || spec_declares_a_power_field(spec) => Ok(0.0),
        None => Err(undeterminable_schedule_power_error(spec, source)),
    }
}

/// Whether the spec declares any power field at all (even at zero): a
/// declared zero is a determined power, an absent one is not.
fn spec_declares_a_power_field(spec: &EquipmentSpec) -> bool {
    spec.parameters.contains_key("max_electric_power_w")
        || spec.parameters.contains_key("annual_electric_kwh")
        || spec.parameters.contains_key("annual_gas_therms")
}

/// Build the resolve error for an equipment whose schedule power cannot be
/// determined: the spec carries neither a rated power (`max_electric_power_w`)
/// nor annual energy (`annual_electric_kwh`, `annual_gas_therms`). Names the
/// equipment and the missing field.
fn undeterminable_schedule_power_error(spec: &EquipmentSpec, source: &str) -> HaresError {
    let msg = format!(
        "equipment '{}' has a schedule source ({source}) but no \
         determinable power: the spec carries no 'max_electric_power_w', \
         no 'annual_electric_kwh' and no 'annual_gas_therms'",
        spec.name
    );
    tracing::error!(equipment = %spec.name, "{msg}");
    HaresError::Equipment(msg)
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

/// Auto-create EquipmentSpecs for CSV columns that have COLUMN_MAPPINGS entries
/// but no corresponding spec from HPXML parsing (e.g. microwave, which is a
/// separate schedule CSV column not produced by the HPXML appliance parser).
///
/// Basement Lighting is gated on `foundation_name == "Finished Basement"` to
/// match the HPXML-resolution gate in `resolve_loads.rs` and OCHRE's behaviour
/// (hpxml.py:1695-1698). Without this check, a schedule CSV with a
/// `lighting_basement` column would silently create basement lighting equipment
/// for unconditioned foundations, reintroducing the exact regression class the
/// `check_basement_lighting_foundation` invariant was written to catch.
///
/// Specs are constructed through `build_spec` — the same path every HPXML-derived
/// spec takes — so default gain fractions, fuel-type labels, and ZIP parameters
/// are injected consistently.  This prevents the class of bug where an
/// auto-created spec is missing a required parameter that `build_spec` would
/// have supplied.
fn ensure_specs_for_csv_columns(
    specs: &mut Vec<EquipmentSpec>,
    csv_col_map: &HashMap<String, usize>,
    columns: &[Vec<f64>],
    defaults: &DefaultsStore,
    foundation_name: Option<&str>,
    garage_modeled: bool,
    warnings: &mut Vec<Warning>,
) {
    for mapping in COLUMN_MAPPINGS {
        if matches!(
            mapping.category,
            ScheduleCategory::NotUsed { .. }
                | ScheduleCategory::Occupancy
                | ScheduleCategory::Setpoint
                | ScheduleCategory::WaterHeater
        ) {
            continue;
        }
        let col_name = normalize_schedule_col_name(mapping.csv_column);
        let Some(&col_idx) = csv_col_map.get(&col_name) else {
            continue;
        };
        // The zone gates follow the OCHRE rules the HPXML lighting resolver
        // applies (hpxml.py:1695-1709, resolve_loads.rs): basement lighting
        // exists only for a finished basement and garage lighting only when
        // a garage is modeled. A spec here would give zone heat to a zone
        // that does not exist, so the column is not read.
        //
        // An unread `lighting_garage` is not a warning, as in OS-HPXML
        // v1.12.0: its simulation reads the column only when garage lighting
        // exists (HPXMLtoOpenStudio/resources/lighting.rb:113-122) and warns
        // only for column names it does not know (schedules.rb:1098-1101),
        // and the ResStock 2024 schedule files carry the column for every
        // home, garage or not.
        //
        // `lighting_basement` is OCHRE's column, which no schedule generator
        // writes for a home without a finished basement: a non-zero one
        // describes a load for a space the dwelling lacks, which the input
        // should correct. An all-zero one carries no load.
        match mapping.equipment_name {
            "Garage Lighting" if !garage_modeled => continue,
            "Basement Lighting" if foundation_name != Some("Finished Basement") => {
                if columns[col_idx].iter().any(|&value| value != 0.0) {
                    warnings.push(Warning::new(
                        "schedule",
                        format!(
                            "schedule CSV column '{col_name}' carries a basement lighting \
                             load, but the foundation is not a Finished Basement, so no \
                             Basement Lighting is created and the column is not read"
                        ),
                    ));
                }
                continue;
            }
            _ => {}
        }
        if specs.iter().any(|s| s.name == mapping.equipment_name) {
            continue;
        }
        // An auto-created spec with no annual-energy default could never
        // resolve a determinable power (that resolve is an error): the home
        // declares no such equipment, so no spec is created and the column
        // stays a known, unused column, matching OCHRE, which creates no
        // equipment from a schedule column alone.
        let Some(annual_kwh) = default_annual_kwh_for_equipment(mapping.equipment_name) else {
            warn!(
                equipment = %mapping.equipment_name,
                "schedule column '{col_name}' names an equipment the HPXML \
                 does not declare and no default annual energy exists; no \
                 spec is auto-created",
            );
            continue;
        };
        let mut params = Map::new();
        params.insert("annual_electric_kwh".to_string(), Value::from(annual_kwh));
        specs.push(build_spec(
            mapping.equipment_name.to_string(),
            FuelType::Electric,
            params,
            defaults,
        ));
    }
}

fn default_annual_kwh_for_equipment(equipment_name: &str) -> Option<f64> {
    match equipment_name {
        "Microwave" => {
            // ANSI/RESNET 301-2014 §4.2.2.5.2: microwave oven default.
            Some(MICROWAVE_DEFAULT_ANNUAL_KWH)
        }
        _ => None,
    }
}

/// Return the set of CSV column names that are neither in `COLUMN_MAPPINGS`
/// (any category, so `NotUsed` entries keep their columns known) nor the
/// occupancy column the environment resolves.
///
/// The caller reports one `Warning` per returned column.
pub(crate) fn find_unknown_schedule_columns(csv_col_map: &HashMap<String, usize>) -> Vec<String> {
    let occupancy_column = resolve_occupancy_column(csv_col_map).map(|(name, _)| name);
    let mut unknown: Vec<String> = csv_col_map
        .keys()
        .filter(|col_name| {
            let normalized = normalize_schedule_col_name(col_name);
            let is_mapped = COLUMN_MAPPINGS
                .iter()
                .any(|m| normalize_schedule_col_name(m.csv_column) == normalized);
            !is_mapped && occupancy_column.as_deref() != Some(col_name.as_str())
        })
        .cloned()
        .collect();
    unknown.sort();
    unknown
}

/// Gated invariant: every HVAC equipment spec must have a setpoint source.
///
/// If heating/cooling equipment is present and no setpoint schedule is
/// configured (schedule CSV column, HPXML-derived, or default profile),
/// the diagnostic names the missing schedule and the affected equipment.
/// Runs in every build profile as a warning diagnostic.
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
    use crate::defaults::DefaultsStore;
    use crate::{EquipmentSpec, ScheduleTimeSeries};
    use chrono::{DateTime, Duration};
    use hares_equipment::{
        ConfigPayload, ElectricResistanceWaterHeaterConfig, EquipmentConfig, GasWaterHeaterConfig,
        HeatPumpWaterHeaterConfig, HvacSetpointConfig, TanklessWaterHeaterConfig,
    };
    use hares_types::{BoundaryPolicy, FuelType, ScheduleSourceConfig, Warning};
    use serde_json::{Map, Value};
    use tempfile::tempdir;

    use super::find_unknown_schedule_columns;

    fn find_setpoint_in_json<'a>(
        data: &'a serde_json::Map<String, Value>,
        key: &str,
    ) -> Option<&'a Value> {
        if let Some(common) = data.get("common")
            && let Some(sp) = common.get("setpoint")
            && let Some(v) = sp.get(key)
        {
            return Some(v);
        }
        if let Some(sp) = data.get("setpoint")
            && let Some(v) = sp.get(key)
        {
            return Some(v);
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

        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            Some(dir.path()),
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        )
        .expect("inject_schedule_into_specs should succeed with valid config");

        assert_eq!(
            specs[0]
                .parameters
                .get("power_schedule_source")
                .and_then(Value::as_str),
            Some("daily_profile")
        );
    }

    #[test]
    fn missing_csv_and_missing_default_profile_is_an_error() {
        // A defaults CSV that loads (Occupancy only) but has no
        // Indoor Lighting profile.
        let dir = tempdir().expect("create temp dir");
        let mut csv = String::from("Category,Name,OCHRE Name,OCHRE Element,Values\n");
        csv.push_str(
            "Schedules,Occupants,Occupancy,weekday_fractions,\"0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1\"\n",
        );
        csv.push_str(
            "Schedules,Occupants,Occupancy,weekend_fractions,\"0.2, 0.2, 0.2, 0.2, 0.2, 0.2, 0.2, 0.2, 0.2, 0.2, 0.2, 0.2, 0.2, 0.2, 0.2, 0.2, 0.2, 0.2, 0.2, 0.2, 0.2, 0.2, 0.2, 0.2\"\n",
        );
        csv.push_str(
            "Schedules,Occupants,Occupancy,month_multipliers,\"1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0\"\n",
        );
        std::fs::write(dir.path().join("Default Schedule Parameters.csv"), csv)
            .expect("write the occupancy-only defaults CSV");

        let mut schedule = make_schedule(24);
        let mut specs = vec![make_spec("Indoor Lighting", 876.0)];
        let err = inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            Some(dir.path()),
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        )
        .expect_err("an equipment with no schedule source must fail the injection");

        let message = err.to_string();
        assert!(
            message.contains("Indoor Lighting"),
            "the error must name the equipment, got: {message}"
        );
        assert!(
            message.contains("lighting_interior"),
            "the error must name the schedule column looked for, got: {message}"
        );
        assert!(
            message.contains("Default Schedule Parameters.csv"),
            "the error must name the defaults file the profile was looked for \
             in, got: {message}"
        );
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
        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            Some(dir.path()),
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        )
        .expect("inject_schedule_into_specs should succeed with valid config");

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
        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            None,
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        )
        .expect("inject_schedule_into_specs should succeed with valid config");

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
        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            None,
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        )
        .expect("inject_schedule_into_specs should succeed with valid config");

        let kw = extract_compact_column_schedule(&specs[0], &schedule);
        let peak = kw.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        assert!((peak - 0.5).abs() < 1e-9, "max power should win precedence");
    }

    #[test]
    fn annual_kwh_only_behavior_unchanged_csv_branch() {
        let mut schedule = make_schedule_with_lighting_column(&[0.2, 1.0, 0.4]);
        let mut specs = vec![make_spec_with_power("Indoor Lighting", Some(1200.0), None)];
        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            None,
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        )
        .expect("inject_schedule_into_specs should succeed with valid config");
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
        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            Some(dir.path()),
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        )
        .expect("inject_schedule_into_specs should succeed with valid config");

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
    fn schedule_resolve_equipment_with_no_determinable_power_is_an_error() {
        // A spec with no rated power and no annual energy routed through
        // each of the three schedule-source branches must fail the resolve
        // naming the equipment and the missing field, never silently run at
        // 0 kW.

        // Branch 1: the schedule CSV carries the equipment's fraction
        // column, so the column is the source and the max kW determination
        // fails.
        let mut schedule = make_schedule_with_lighting_column(&[0.2, 1.0, 0.4]);
        let mut specs = vec![make_spec_with_power("Indoor Lighting", None, None)];
        let err = inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            None,
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        )
        .expect_err("a CSV-column source with no determinable power must fail");
        let message = err.to_string();
        assert!(
            message.contains("Indoor Lighting"),
            "the error must name the equipment, got: {message}"
        );
        assert!(
            message.contains("max_electric_power_w") && message.contains("annual_electric_kwh"),
            "the error must name the missing fields, got: {message}"
        );
        assert!(
            message.contains("lighting_interior"),
            "the error must name the schedule source, got: {message}"
        );

        // Branch 2: no CSV column, but the spec carries HPXML schedule
        // fractions, so the fractions are the source.
        let mut hpxml_spec = make_spec_with_power("Indoor Lighting", None, None);
        hpxml_spec.parameters.insert(
            "weekday_schedule_fractions".to_string(),
            Value::Array(
                (0..24)
                    .map(|h| Value::from(0.5 + f64::from(h % 12) * 0.01))
                    .collect(),
            ),
        );
        hpxml_spec.parameters.insert(
            "weekend_schedule_fractions".to_string(),
            Value::Array((0..24).map(|_| Value::from(0.4)).collect()),
        );
        let mut schedule = make_schedule(24);
        let mut specs = vec![hpxml_spec];
        let err = inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            None,
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        )
        .expect_err("an HPXML-fraction source with no determinable power must fail");
        let message = err.to_string();
        assert!(
            message.contains("Indoor Lighting"),
            "the error must name the equipment, got: {message}"
        );
        assert!(
            message.contains("max_electric_power_w") && message.contains("annual_electric_kwh"),
            "the error must name the missing fields, got: {message}"
        );
        assert!(
            message.contains("HPXML schedule fractions"),
            "the error must name the schedule source, got: {message}"
        );

        // Branch 3: no CSV column and no HPXML fractions, but a default
        // profile exists for the equipment.
        let dir = tempdir().expect("create temp dir");
        write_default_profile_csv(dir.path());
        let mut schedule = make_schedule(24);
        let mut specs = vec![make_spec_with_power("Indoor Lighting", None, None)];
        let err = inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            Some(dir.path()),
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        )
        .expect_err("a default-profile source with no determinable power must fail");
        let message = err.to_string();
        assert!(
            message.contains("Indoor Lighting"),
            "the error must name the equipment, got: {message}"
        );
        assert!(
            message.contains("max_electric_power_w") && message.contains("annual_electric_kwh"),
            "the error must name the missing fields, got: {message}"
        );
        assert!(
            message.contains("default schedule profile"),
            "the error must name the schedule source, got: {message}"
        );
    }

    #[test]
    fn all_zero_schedule_fractions_are_a_determined_zero_not_an_error() {
        // A spec WITH annual energy whose schedule fractions are all zero
        // runs never: its determined power is exactly 0 kW, the schedule's
        // own claim, not the silent zero-fill of an undeterminable one.
        let mut schedule = make_schedule_with_lighting_column(&[0.0, 0.0, 0.0]);
        let mut specs = vec![make_spec("Indoor Lighting", 876.0)];
        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            None,
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        )
        .expect("an all-zero fraction column resolves to a determined 0 kW");

        let derived_idx = specs[0]
            .parameters
            .get("power_schedule_col")
            .and_then(Value::as_u64)
            .expect("power_schedule_col must be present") as usize;
        assert!(
            schedule.columns[derived_idx].iter().all(|kw| *kw == 0.0),
            "the derived kW column must be all zero"
        );

        // Same through the HPXML-fraction branch: no CSV column, zero
        // fractions on the spec, annual energy present.
        let mut spec = make_spec_with_power("Indoor Lighting", Some(876.0), None);
        spec.parameters.insert(
            "weekday_schedule_fractions".to_string(),
            Value::Array((0..24).map(|_| Value::from(0.0)).collect()),
        );
        let mut schedule = make_schedule(24);
        let mut specs = vec![spec];
        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            None,
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        )
        .expect("an all-zero HPXML profile resolves to a determined 0 kW");
        let max_kw = specs[0]
            .parameters
            .get("power_profile_max_kw")
            .and_then(Value::as_f64)
            .expect("power_profile_max_kw must be present");
        assert_eq!(max_kw, 0.0);
    }

    #[test]
    fn declared_zero_annual_energy_is_a_determined_zero_not_an_error() {
        // A spec whose annual energy is DECLARED as zero has a determined
        // power: exactly 0 kW. Only a spec that declares no power field at
        // all is undeterminable.
        let mut schedule = make_schedule_with_lighting_column(&[0.2, 1.0, 0.4]);
        let mut spec = make_spec_with_power("Indoor Lighting", None, None);
        spec.parameters
            .insert("annual_electric_kwh".to_string(), Value::from(0.0));
        let mut specs = vec![spec];
        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            None,
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        )
        .expect("a declared zero annual energy resolves to a determined 0 kW");

        let derived_idx = specs[0]
            .parameters
            .get("power_schedule_col")
            .and_then(Value::as_u64)
            .expect("power_schedule_col must be present") as usize;
        assert!(
            schedule.columns[derived_idx].iter().all(|kw| *kw == 0.0),
            "the derived kW column must be all zero"
        );
    }

    #[test]
    fn event_csv_injects_schedule_column_reference() {
        let mut schedule = make_schedule_with_event_column(&[0.0, 1.0, 0.0]);
        let mut specs = vec![make_spec("Dishwasher", 0.0)];

        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            None,
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        )
        .expect("inject_schedule_into_specs should succeed with valid config");

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

        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            None,
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        )
        .expect("inject_schedule_into_specs should succeed with valid config");

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
            typed_config: Some(
                hares_equipment::EquipmentConfig::from_typed(
                    name.to_string(),
                    ochre_class.to_string(),
                    config,
                )
                .unwrap(),
            ),
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
                    reject_unknown_keys: hares_equipment::RejectUnknownKeys,
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
                    min_oat_cooling_c: 10.0,
                },
            ),
        ];

        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            None,
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        )
        .expect("inject_schedule_into_specs should succeed with valid config");

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
                    max_combined_power_w: None,
                    jacket_r_value_m2_k_w: None,
                    fixture_delivery_temp_c: None,
                    hot_draw_temp_c: None,
                },
            ),
            "Gas Water Heater" => EquipmentConfig::from_typed(
                "gas".to_string(),
                "Gas Water Heater".to_string(),
                GasWaterHeaterConfig {
                    fan_power_w: None,
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
                    pilot_fraction_to_tank: None,
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
                    cop_curve_is_normalized: None,
                    performance_adjustment: Some(0.92),
                    zone_type: Some("conditioned".to_string()),
                    first_hour_rating_m3: Some(0.20),
                    jacket_r_value_m2_k_w: None,
                    fixture_delivery_temp_c: None,
                    low_power_hpwh: None,
                    uniform_energy_factor: None,
                },
            ),
            other => panic!("unsupported water heater type for schedule test: {other}"),
        }
        .unwrap();
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
                zone_type: None,
                min_flow_kg_s: None,
                min_flow_gpm: None,
            },
        )
        .unwrap();
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

        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            None,
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        )
        .expect("inject_schedule_into_specs should succeed with valid config");

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

        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            None,
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        )
        .expect("inject_schedule_into_specs should succeed with valid config");

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
    fn tankless_without_avg_water_draw_returns_err() {
        let mut schedule = make_schedule_with_water_heater_columns(
            "hot_water_fixtures",
            &[0.2, 0.0],
            &[11.0, 12.0],
        );
        let mut specs = vec![make_typed_tankless_spec(None)];

        let result = inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            None,
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        );
        assert!(result.is_err(), "expected Err, got Ok");
        let err = result.unwrap_err();
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("requires typed avg_water_draw_l_per_day"),
            "expected error message about missing avg_water_draw_l_per_day, got: {err_msg}"
        );
    }

    // =======================================================================
    // Water heater draw normalization error propagation
    // =======================================================================

    #[test]
    fn storage_water_heater_without_draw_field_returns_err() {
        // Electric resistance water heater without avg_water_draw_l_per_day.
        let config = ElectricResistanceWaterHeaterConfig {
            equipment_id: None,
            zone_id: None,
            loop_id: None,
            tank_volume_m3: Some(0.19),
            tank_height_m: Some(1.4),
            energy_factor: None,
            uniform_energy_factor: None,
            heating_capacity_w: Some(4_500.0),
            ua_w_per_k: None,
            setpoint_c: Some(51.67),
            deadband_c: None,
            max_tank_temp_c: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: None,
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            element_power_w: None,
            max_setpoint_ramp_rate_c_per_min: None,
            element_priority_mode: None,
            jacket_r_value_m2_k_w: None,
            max_combined_power_w: None,
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
        };
        let typed_config = EquipmentConfig::from_typed(
            "electric".to_string(),
            "Electric Resistance Water Heater".to_string(),
            config,
        )
        .unwrap();
        let spec = EquipmentSpec {
            instance_name: None,
            name: "Electric Resistance Water Heater".to_string(),
            fuel_type: FuelType::Electric,
            parameters: Map::new(),
            zip_params: None,
            typed_config: Some(typed_config),
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        };

        let mut schedule = make_schedule_with_water_heater_columns(
            "hot_water_fixtures",
            &[0.2, 0.0],
            &[11.0, 12.0],
        );
        let mut specs = vec![spec];

        let result = inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            None,
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        );
        assert!(result.is_err(), "expected Err, got Ok");
        let err = result.unwrap_err();
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("requires typed avg_water_draw_l_per_day"),
            "expected error message about missing avg_water_draw_l_per_day, got: {err_msg}"
        );
    }

    #[test]
    fn unsupported_water_heater_type_returns_err() {
        // An equipment spec whose name is in STORAGE_WATER_HEATER_EQUIPMENT but is
        // not handled by water_heater_avg_daily_draw_l triggers the 'other' arm.
        // Since all current entries are handled, test the defensive arm directly.
        let spec = EquipmentSpec {
            instance_name: None,
            name: "Fusion Water Heater".to_string(),
            fuel_type: FuelType::Electric,
            parameters: Map::new(),
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        };

        let result = super::water_heater_avg_daily_draw_l(&spec);
        assert!(result.is_err(), "expected Err for unsupported type, got Ok");
        let err = result.unwrap_err();
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("unsupported water heater type"),
            "expected error message about unsupported water heater type, got: {err_msg}"
        );
    }

    #[test]
    fn valid_water_heater_with_draw_produces_correct_schedule_columns() {
        let mut schedule = make_schedule_with_water_heater_columns(
            "hot_water_fixtures",
            &[0.2, 0.0],
            &[11.0, 12.0],
        );
        let mut specs = vec![make_water_heater_spec(
            "Electric Resistance Water Heater",
            200.0,
        )];

        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            None,
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        )
        .expect("inject_schedule_into_specs should succeed with valid config");

        let typed = specs[0]
            .typed_config
            .as_ref()
            .expect("spec should retain typed config");
        let cfg = typed
            .typed::<ElectricResistanceWaterHeaterConfig>()
            .expect("typed config should decode");
        assert!(
            matches!(
                cfg.draw_flow_rate_source,
                Some(ScheduleSourceConfig::ColumnRef { .. })
            ),
            "valid water heater should receive draw_flow_rate_source ColumnRef"
        );
        assert!(
            matches!(
                cfg.mains_temp_c_source,
                Some(ScheduleSourceConfig::ColumnRef { .. })
            ),
            "valid water heater should receive mains_temp_c_source ColumnRef"
        );
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

    /// A second row for the same profile and element is an error naming
    /// both lines, never a silent override by the later row.
    #[test]
    fn load_default_profiles_rejects_a_duplicate_profile_row() {
        let dir = tempdir().expect("create temp dir");
        write_defaults_csv_with_setpoints(dir.path());
        let path = dir.path().join("Default Schedule Parameters.csv");
        let mut csv = std::fs::read_to_string(&path).expect("read the written csv");
        csv.push_str(
            "Schedules,Lighting,Indoor Lighting,month_multipliers,\"1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0\"\n",
        );
        std::fs::write(&path, csv).expect("rewrite the csv");

        let err = super::load_default_profiles(dir.path())
            .expect_err("a duplicate profile row must not override the first");
        let msg = err.to_string();
        assert!(
            msg.contains("Indoor Lighting")
                && msg.contains("month_multipliers")
                && msg.contains("line 4")
                && msg.contains("line 11"),
            "the error names the profile, the element and both lines, got: {msg}"
        );
    }

    // ── Setpoint default profile tests ──

    #[test]
    fn load_default_profiles_parses_hvac_heating_and_cooling_from_csv() {
        let dir = tempdir().expect("create temp dir");
        write_defaults_csv_with_setpoints(dir.path());

        let profiles =
            super::load_default_profiles(dir.path()).expect("the setpoints defaults CSV must load");

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
                    reject_unknown_keys: hares_equipment::RejectUnknownKeys,
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
                    min_oat_cooling_c: 10.0,
                },
            ),
        ];

        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            Some(dir.path()),
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        )
        .expect("inject_schedule_into_specs should succeed with valid config");

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
                    reject_unknown_keys: hares_equipment::RejectUnknownKeys,
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
                    min_oat_cooling_c: 10.0,
                },
            ),
        ];

        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            Some(dir.path()),
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        )
        .expect("inject_schedule_into_specs should succeed with valid config");

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
        let profiles = super::load_default_profiles(&defaults_dir)
            .expect("the shipped defaults CSV must load");

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
        let profiles = super::load_default_profiles(&defaults_dir)
            .expect("the shipped defaults CSV must load");

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
                .get(name)
                .unwrap_or_else(|err| panic!("schedule '{name}' must exist in defaults: {err}"));
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
        let profiles = super::load_default_profiles(&defaults_dir)
            .expect("the shipped defaults CSV must load");

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
            if let Some(profile) = profiles.find(name) {
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
        let profiles = super::load_default_profiles(&defaults_dir)
            .expect("the shipped defaults CSV must load");

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

    // ── Occupancy schedule injection tests ───────────────────────────────────

    fn make_occupancy_spec(
        weekday_fractions: &[f64],
        weekend_fractions: &[f64],
        month_multipliers: &[f64],
        n_occupants: f64,
    ) -> EquipmentSpec {
        let mut parameters = Map::new();
        parameters.insert(
            "weekday_schedule_fractions".to_string(),
            Value::Array(weekday_fractions.iter().map(|&v| Value::from(v)).collect()),
        );
        parameters.insert(
            "weekend_schedule_fractions".to_string(),
            Value::Array(weekend_fractions.iter().map(|&v| Value::from(v)).collect()),
        );
        parameters.insert(
            "month_multipliers".to_string(),
            Value::Array(month_multipliers.iter().map(|&v| Value::from(v)).collect()),
        );
        parameters.insert("number_of_occupants".to_string(), Value::from(n_occupants));
        EquipmentSpec {
            instance_name: None,
            name: "Occupancy".to_string(),
            fuel_type: FuelType::Electric,
            parameters,
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        }
    }

    fn write_occupancy_default_csv(path: &std::path::Path) {
        let mut csv =
            String::from("Schedule Name,Element,OCHRE Name,OCHRE Element,Values,Data Source\n");
        csv.push_str(
            "occupants,WeekdayScheduleFractions,Occupancy,weekday_fractions,\"0.1,0.1,0.1,0.1,0.1,0.2,0.3,0.5,0.8,0.9,0.9,0.8,0.7,0.6,0.5,0.6,0.7,0.8,0.9,0.9,0.8,0.5,0.3,0.1\"\n",
        );
        csv.push_str(
            "occupants,WeekendScheduleFractions,Occupancy,weekend_fractions,\"0.05,0.05,0.05,0.05,0.05,0.1,0.2,0.4,0.7,0.9,0.9,0.8,0.7,0.6,0.5,0.6,0.7,0.8,0.9,0.9,0.8,0.5,0.3,0.1\"\n",
        );
        csv.push_str(
            "occupants,MonthlyScheduleMultipliers,Occupancy,month_multipliers,\"1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0,1.0\"\n",
        );
        std::fs::write(path.join("Default Schedule Parameters.csv"), csv)
            .expect("write occupancy default csv");
    }

    #[test]
    fn inject_occupancy_schedule_uses_hpxml_fractions_when_no_csv_column() {
        let dir = tempdir().expect("create temp dir");
        write_occupancy_default_csv(dir.path());

        let mut schedule = make_schedule(48); // 2 days of hourly data
        // Jan 1 (Wed) and Jan 2 (Thu) — both weekdays
        let mut specs = vec![make_occupancy_spec(
            &[0.5; 24], // weekday: 0.5 every hour
            &[0.2; 24], // weekend: 0.2 every hour
            &[1.0; 12], // no monthly variation
            3.0,
        )];

        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            Some(dir.path()),
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        )
        .expect("inject_schedule_into_specs should succeed with valid config");

        // Verify the occupancy column was added
        let col_idx = schedule
            .column_index
            .get("occupants")
            .expect("schedule must have occupants column after injection");
        let col = &schedule.columns[*col_idx];

        assert_eq!(col.len(), 48);
        // All timesteps are weekdays (Wed & Thu), so fraction = weekday_fraction * month_multiplier = 0.5 * 1.0
        for (i, val) in col.iter().enumerate() {
            assert!(
                (*val - 0.5).abs() < 1e-10,
                "hour {i}: expected 0.5, got {val}"
            );
        }

        // Verify source is recorded
        assert_eq!(
            specs[0]
                .parameters
                .get("occupancy_schedule_source")
                .and_then(Value::as_str),
            Some("hpxml_extension")
        );
    }

    #[test]
    fn inject_occupancy_schedule_applies_weekend_fractions_correctly() {
        let dir = tempdir().expect("create temp dir");
        write_occupancy_default_csv(dir.path());

        // Use 7 days starting Jan 1 2025 (Wednesday) — includes Sat & Sun
        let start =
            DateTime::parse_from_rfc3339("2025-01-01T00:00:00+00:00").expect("valid datetime");
        let timestamps: Vec<_> = (0..168)
            .map(|i| start + Duration::hours(i as i64))
            .collect();
        let mut schedule = ScheduleTimeSeries {
            timestamps,
            column_names: Vec::new(),
            columns: Vec::new(),
            column_index: HashMap::new(),
            source_step_secs: 3600,
            column_aggregations: Vec::new(),
        };

        let mut specs = vec![make_occupancy_spec(
            &[0.8; 24], // weekday: 0.8
            &[0.3; 24], // weekend: 0.3
            &[1.0; 12], // no monthly variation
            2.0,
        )];

        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            Some(dir.path()),
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        )
        .expect("inject_schedule_into_specs should succeed with valid config");

        let col_idx = schedule.column_index["occupants"];
        let col = &schedule.columns[col_idx];

        assert_eq!(col.len(), 168);
        // Day 1 (Wed): weekday → 0.8
        for (h, &v) in col.iter().enumerate().take(24) {
            assert!((v - 0.8).abs() < 1e-10, "Wed hour {h}: expected 0.8");
        }
        // Day 5 (Sun): weekend → 0.3
        for (h, &v) in col.iter().enumerate().skip(96).take(24) {
            assert!((v - 0.3).abs() < 1e-10, "Sun hour {}: expected 0.3", h - 96);
        }
        // Day 6 (Mon): weekday → 0.8
        for (h, &v) in col.iter().enumerate().skip(144).take(24) {
            assert!(
                (v - 0.8).abs() < 1e-10,
                "Mon hour {}: expected 0.8",
                h - 144
            );
        }
    }

    #[test]
    fn inject_occupancy_schedule_applies_month_multipliers() {
        let dir = tempdir().expect("create temp dir");
        write_occupancy_default_csv(dir.path());

        // June 1 2025 is a Sunday, June 2 is Monday
        let start =
            DateTime::parse_from_rfc3339("2025-06-01T00:00:00+00:00").expect("valid datetime");
        let timestamps: Vec<_> = (0..48).map(|i| start + Duration::hours(i as i64)).collect();
        let mut schedule = ScheduleTimeSeries {
            timestamps,
            column_names: Vec::new(),
            columns: Vec::new(),
            column_index: HashMap::new(),
            source_step_secs: 3600,
            column_aggregations: Vec::new(),
        };

        let mut month_mult = [1.0; 12];
        month_mult[5] = 2.0; // June (month index 5) has multiplier 2.0

        let mut specs = vec![make_occupancy_spec(
            &[0.5; 24],
            &[0.5; 24],
            &month_mult,
            1.0,
        )];

        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            Some(dir.path()),
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        )
        .expect("inject_schedule_into_specs should succeed with valid config");

        let col_idx = schedule.column_index["occupants"];
        let col = &schedule.columns[col_idx];

        assert_eq!(col.len(), 48);
        // Every hour in June: 0.5 * 2.0 = 1.0
        for (i, val) in col.iter().enumerate() {
            assert!(
                (*val - 1.0).abs() < 1e-10,
                "June hour {i}: expected 1.0, got {val}"
            );
        }
    }

    #[test]
    fn inject_occupancy_schedule_uses_default_profile_when_no_hpxml_fractions() {
        let dir = tempdir().expect("create temp dir");
        write_occupancy_default_csv(dir.path());

        let mut schedule = make_schedule(24);
        // Occupancy spec with number_of_occupants but NO schedule fractions
        let mut parameters = Map::new();
        parameters.insert("number_of_occupants".to_string(), Value::from(2.0));
        let spec = EquipmentSpec {
            instance_name: None,
            name: "Occupancy".to_string(),
            fuel_type: FuelType::Electric,
            parameters,
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        };
        let mut specs = vec![spec];

        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            Some(dir.path()),
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        )
        .expect("inject_schedule_into_specs should succeed with valid config");

        // Verify the occupancy column was generated from default profile
        assert!(
            schedule.column_index.contains_key("occupants"),
            "schedule must have occupants column from default profile"
        );
        assert_eq!(
            specs[0]
                .parameters
                .get("occupancy_schedule_source")
                .and_then(Value::as_str),
            Some("default_profile")
        );
    }

    #[test]
    fn inject_occupancy_schedule_preserves_existing_csv_column() {
        let dir = tempdir().expect("create temp dir");
        write_occupancy_default_csv(dir.path());

        // Create a schedule that already has an "occupants" column from CSV
        let existing_values: Vec<f64> = (0..24).map(|i| i as f64 / 24.0).collect();
        let mut schedule = make_schedule_with_occupancy_column(&existing_values);

        let mut specs = vec![make_occupancy_spec(&[0.5; 24], &[0.2; 24], &[1.0; 12], 3.0)];

        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            Some(dir.path()),
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        )
        .expect("inject_schedule_into_specs should succeed with valid config");

        // Verify existing column is preserved (not overwritten)
        let col_idx = schedule.column_index["occupants"];
        let col = &schedule.columns[col_idx];
        for (i, val) in col.iter().enumerate() {
            assert!(
                (*val - existing_values[i]).abs() < 1e-10,
                "hour {i}: expected existing value {}, got {val}",
                existing_values[i]
            );
        }

        // Source should indicate CSV column
        assert_eq!(
            specs[0]
                .parameters
                .get("occupancy_schedule_source")
                .and_then(Value::as_str),
            Some("csv_column")
        );
    }

    #[test]
    fn inject_occupancy_schedule_no_number_of_occupants_still_generates_column() {
        // Verify that absent NumberofResidents (number_of_occupants key) still
        // produces a valid occupancy schedule column. The dwelling constructor
        // may separately error on missing number_of_occupants — that interaction
        // is tracked in T-0191.
        let dir = tempdir().expect("create temp dir");
        write_occupancy_default_csv(dir.path());

        let mut schedule = make_schedule(24);
        let mut parameters = Map::new();
        parameters.insert(
            "weekday_schedule_fractions".to_string(),
            Value::Array((0..24).map(|_| Value::from(0.5)).collect()),
        );
        // No number_of_occupants key

        let spec = EquipmentSpec {
            instance_name: None,
            name: "Occupancy".to_string(),
            fuel_type: FuelType::Electric,
            parameters,
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        };
        let mut specs = vec![spec];

        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            Some(dir.path()),
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        )
        .expect("inject_schedule_into_specs should succeed with valid config");

        assert!(
            schedule.column_index.contains_key("occupants"),
            "schedule must have occupants column even when number_of_occupants is absent"
        );
        assert_eq!(
            specs[0]
                .parameters
                .get("occupancy_schedule_source")
                .and_then(Value::as_str),
            Some("hpxml_extension")
        );
    }

    fn make_schedule_with_occupancy_column(values: &[f64]) -> ScheduleTimeSeries {
        let start =
            DateTime::parse_from_rfc3339("2025-01-01T00:00:00+00:00").expect("valid datetime");
        let timestamps = (0..values.len())
            .map(|i| start + Duration::hours(i as i64))
            .collect::<Vec<_>>();

        let mut column_index = HashMap::new();
        column_index.insert("occupants".to_string(), 0);
        ScheduleTimeSeries {
            timestamps,
            column_names: vec!["occupants".to_string()],
            columns: vec![values.to_vec()],
            column_index,
            source_step_secs: 3600,
            column_aggregations: vec![crate::schedule::ColumnAggregation::Mean],
        }
    }

    // ── Microwave column mapping tests ──

    fn make_schedule_with_microwave_column(values: &[f64]) -> ScheduleTimeSeries {
        let start =
            DateTime::parse_from_rfc3339("2025-01-01T00:00:00+00:00").expect("valid datetime");
        let timestamps = (0..values.len())
            .map(|i| start + Duration::hours(i as i64))
            .collect::<Vec<_>>();

        let mut column_index = HashMap::new();
        column_index.insert("microwave".to_string(), 0);
        ScheduleTimeSeries {
            timestamps,
            column_names: vec!["microwave".to_string()],
            columns: vec![values.to_vec()],
            column_index,
            source_step_secs: 3600,
            column_aggregations: vec![crate::ColumnAggregation::Mean],
        }
    }

    #[test]
    fn microwave_csv_column_creates_spec_with_event_schedule_and_nonzero_energy() {
        let mut schedule = make_schedule_with_microwave_column(&[0.0, 1.0, 0.5, 0.0]);
        let mut specs: Vec<EquipmentSpec> = Vec::new();

        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            None,
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        )
        .expect("inject_schedule_into_specs should succeed with valid config");

        assert_eq!(
            specs.len(),
            1,
            "auto-creation should produce one Microwave spec"
        );
        assert_eq!(specs[0].name, "Microwave");
        assert_eq!(specs[0].fuel_type, FuelType::Electric);

        let annual_kwh = specs[0]
            .parameters
            .get("annual_electric_kwh")
            .and_then(Value::as_f64)
            .expect("annual_electric_kwh must be set");
        assert!(
            (annual_kwh - 100.0).abs() < 1e-12,
            "default annual kWh should be 100"
        );

        let schedule_col = specs[0]
            .parameters
            .get("event_window_schedule_col")
            .and_then(Value::as_u64)
            .expect("event_window_schedule_col must be set");
        assert_eq!(schedule_col, 0);

        assert!(
            specs[0].parameters.contains_key("event_power_kw_series"),
            "non-zero annual energy should produce event_power_kw_series"
        );
        let kw_series = specs[0]
            .parameters
            .get("event_power_kw_series")
            .and_then(Value::as_array)
            .expect("event_power_kw_series must be an array");
        assert_eq!(kw_series.len(), 4);
        let peak_kw: f64 = kw_series
            .iter()
            .filter_map(Value::as_f64)
            .fold(0.0_f64, f64::max);
        assert!(
            peak_kw > 0.0,
            "microwave schedule should have non-zero power; annual kWh=100"
        );

        let sensible = specs[0]
            .parameters
            .get("sensible_gain_fraction")
            .and_then(Value::as_f64)
            .expect("sensible_gain_fraction must be injected by build_spec");
        assert!(
            (sensible - 0.72).abs() < 1e-12,
            "microwave sensible gain fraction should match default_gain_fractions"
        );

        let latent = specs[0]
            .parameters
            .get("latent_gain_fraction")
            .and_then(Value::as_f64)
            .expect("latent_gain_fraction must be injected by build_spec");
        assert!(
            (latent - 0.08).abs() < 1e-12,
            "microwave latent gain fraction should match default_gain_fractions"
        );
    }

    /// Regression: an auto-created Microwave spec (from a schedule CSV column
    /// with no HPXML `<Microwave>` element) must survive the full equipment
    /// construction path — `build_spec` → `EquipmentConfig` →
    /// `EventBasedLoad::init()`.  Before the fix, `ensure_specs_for_csv_columns`
    /// hand-rolled the `EquipmentSpec` without gain fractions, so `init()`
    /// returned `Err("sensible_gain_fraction missing for 'Microwave'")`.
    #[test]
    fn auto_created_microwave_spec_survives_event_load_init() {
        use chrono::TimeZone;
        use hares_equipment::Equipment;
        use hares_equipment::config::ConfigValue;
        use hares_equipment::event_load::EventBasedLoad;
        use hares_types::{
            DomainUpdate, GridState, SCHEDULE_DOMAIN_ID, WeatherState, ZoneId, ZoneState,
        };

        let mut schedule = make_schedule_with_microwave_column(&[0.0, 1.0, 0.5, 0.0]);
        let mut specs: Vec<EquipmentSpec> = Vec::new();

        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            None,
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        )
        .expect("inject should succeed");

        let spec = &specs[0];
        assert_eq!(spec.name, "Microwave");

        let raw_config: HashMap<String, ConfigValue> = spec
            .parameters
            .iter()
            .filter_map(|(k, v)| {
                let cv = match v {
                    Value::Number(n) => n.as_f64().map(ConfigValue::Float),
                    Value::String(s) => Some(ConfigValue::Text(s.clone())),
                    Value::Bool(b) => Some(ConfigValue::Bool(*b)),
                    Value::Array(arr) => {
                        let floats: Vec<f64> = arr.iter().filter_map(Value::as_f64).collect();
                        if floats.len() == arr.len() {
                            Some(ConfigValue::FloatArray(floats))
                        } else {
                            None
                        }
                    }
                    _ => None,
                };
                cv.map(|c| (k.clone(), c))
            })
            .collect();

        let stream = hares_types::rng::RngStream::event_load(
            hares_types::rng::dwelling_seed(0, 0),
            &spec.name,
        );
        let mut cfg = EquipmentConfig::raw(spec.name.clone(), spec.name.clone(), raw_config)
            .with_rng_stream(stream);
        let mut zone_map = hares_types::ZoneMap::new();
        zone_map.insert(hares_types::ZoneRole::Indoor, ZoneId(1));
        cfg.zone_map = Some(zone_map);

        let env = hares_types::EnvironmentState {
            ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: 21.0,
                humidity_ratio: 0.008,
                volume_m3: 200.0,
            }],
            weather: WeatherState::default(),
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
                island_bus_voltage_pu: None,
            },
            custom_domains: vec![DomainUpdate {
                domain_id: SCHEDULE_DOMAIN_ID,
                zone_temperatures_c: Vec::new(),
                custom_payload: Some(vec![0.0, 0.0]),
            }],
            equipment_telemetry: HashMap::new(),
            equipment_core: HashMap::new(),
            current_time: chrono::FixedOffset::east_opt(0)
                .expect("UTC offset")
                .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                .single()
                .expect("valid timestamp"),
            time_res: chrono::Duration::minutes(1),
            price_signal: Default::default(),
            electrical: Default::default(),
        };

        let mut eq = EventBasedLoad::new(cfg.clone());
        eq.init(&cfg, &env).expect(
            "auto-created Microwave spec must survive EventBasedLoad::init — \
                     before the fix this failed with 'sensible_gain_fraction missing'",
        );
    }

    #[test]
    fn find_unmapped_csv_columns_detects_unknown_column() {
        let mut column_index = HashMap::new();
        column_index.insert("unknown_column".to_string(), 0);
        column_index.insert("microwave".to_string(), 1);

        let unmapped = super::find_unknown_schedule_columns(&column_index);
        assert!(
            unmapped.contains(&"unknown_column".to_string()),
            "unknown_column should be flagged as unmapped; got {unmapped:?}"
        );
        assert!(
            !unmapped.contains(&"microwave".to_string()),
            "microwave should NOT be in unmapped list; it has a mapping"
        );
    }

    #[test]
    fn find_unmapped_csv_columns_returns_empty_when_all_mapped() {
        let mut column_index = HashMap::new();
        column_index.insert("cooking_range".to_string(), 0);
        column_index.insert("refrigerator".to_string(), 1);

        let unmapped = super::find_unknown_schedule_columns(&column_index);
        assert!(unmapped.is_empty(), "all columns mapped; got {unmapped:?}");
    }

    #[test]
    fn microwave_and_cooking_range_are_distinct_independent_schedules() {
        // Regression: microwave addition must not affect Cooking Range.
        use crate::schedule::ColumnAggregation;

        let start =
            DateTime::parse_from_rfc3339("2025-01-01T00:00:00+00:00").expect("valid datetime");
        let timestamps = (0..4)
            .map(|i| start + Duration::hours(i as i64))
            .collect::<Vec<_>>();

        let mut column_index = HashMap::new();
        column_index.insert("cooking_range".to_string(), 0);
        column_index.insert("microwave".to_string(), 1);

        let mut schedule = ScheduleTimeSeries {
            timestamps,
            column_names: vec!["cooking_range".to_string(), "microwave".to_string()],
            columns: vec![vec![0.1, 0.5, 0.3, 0.1], vec![0.0, 1.0, 0.5, 0.0]],
            column_index,
            source_step_secs: 3600,
            column_aggregations: vec![ColumnAggregation::Mean, ColumnAggregation::Mean],
        };

        let mut params = Map::new();
        params.insert("annual_electric_kwh".to_string(), Value::from(600.0));
        let mut specs: Vec<EquipmentSpec> = vec![EquipmentSpec {
            instance_name: None,
            name: "Cooking Range".to_string(),
            fuel_type: FuelType::Electric,
            parameters: params,
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        }];

        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            None,
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        )
        .expect("inject_schedule_into_specs should succeed");

        // Cooking Range must have its own schedule column
        let cooking = specs
            .iter()
            .find(|s| s.name == "Cooking Range")
            .expect("Cooking Range spec must exist");
        assert!(
            cooking.parameters.contains_key("event_window_schedule_col"),
            "Cooking Range must have event_window_schedule_col"
        );

        // Microwave auto-created spec must also be independent
        let microwave = specs
            .iter()
            .find(|s| s.name == "Microwave")
            .expect("Microwave spec must be auto-created");
        assert!(
            microwave
                .parameters
                .contains_key("event_window_schedule_col"),
            "Microwave must have event_window_schedule_col"
        );

        // Columns must be distinct
        let cooking_col = cooking
            .parameters
            .get("event_window_schedule_col")
            .and_then(Value::as_u64);
        let microwave_col = microwave
            .parameters
            .get("event_window_schedule_col")
            .and_then(Value::as_u64);
        assert!(
            cooking_col != microwave_col,
            "Cooking Range and Microwave must have different schedule column indices"
        );
    }

    fn make_schedule_with_basement_lighting_column(values: &[f64]) -> ScheduleTimeSeries {
        make_schedule_with_column("lighting_basement", values)
    }

    fn make_schedule_with_column(name: &str, values: &[f64]) -> ScheduleTimeSeries {
        let start =
            DateTime::parse_from_rfc3339("2025-01-01T00:00:00+00:00").expect("valid datetime");
        let timestamps = (0..values.len())
            .map(|i| start + Duration::hours(i as i64))
            .collect::<Vec<_>>();

        let mut column_index = HashMap::new();
        column_index.insert(name.to_string(), 0);
        ScheduleTimeSeries {
            timestamps,
            column_names: vec![name.to_string()],
            columns: vec![values.to_vec()],
            column_index,
            source_step_secs: 3600,
            column_aggregations: vec![crate::ColumnAggregation::Mean],
        }
    }

    #[test]
    fn basement_lighting_not_auto_created_from_csv_for_unfinished_foundation() {
        let mut schedule = make_schedule_with_basement_lighting_column(&[0.02, 0.01, 0.005]);
        let mut specs: Vec<EquipmentSpec> = Vec::new();

        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            None,
            &DefaultsStore::empty(),
            None,
            false,
            &mut Vec::new(),
        )
        .expect("inject_schedule_into_specs should succeed");

        assert!(
            !specs.iter().any(|s| s.name == "Basement Lighting"),
            "Basement Lighting must not be auto-created from CSV column when foundation is not Finished Basement"
        );
    }

    #[test]
    fn basement_lighting_not_auto_created_from_csv_for_unfinished_basement() {
        let mut schedule = make_schedule_with_basement_lighting_column(&[0.02, 0.01, 0.005]);
        let mut specs: Vec<EquipmentSpec> = Vec::new();

        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            None,
            &DefaultsStore::empty(),
            Some("Unfinished Basement"),
            false,
            &mut Vec::new(),
        )
        .expect("inject_schedule_into_specs should succeed");

        assert!(
            !specs.iter().any(|s| s.name == "Basement Lighting"),
            "Basement Lighting must not be auto-created from CSV column when foundation is Unfinished Basement"
        );
    }

    #[test]
    fn basement_lighting_not_auto_created_from_csv_even_for_finished_basement() {
        let mut schedule = make_schedule_with_basement_lighting_column(&[0.02, 0.01, 0.005]);
        let mut specs: Vec<EquipmentSpec> = Vec::new();

        // A CSV column alone does not create equipment the home does not
        // declare: an auto-created spec with no annual-energy default could
        // never resolve a determinable power, so no spec is created and the
        // resolve succeeds without one.
        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            None,
            &DefaultsStore::empty(),
            Some("Finished Basement"),
            false,
            &mut Vec::new(),
        )
        .expect("a declared-in-CSV-only column is not an error");

        assert!(
            !specs.iter().any(|s| s.name == "Basement Lighting"),
            "no Basement Lighting spec is auto-created without an \
             annual-energy default"
        );
    }

    #[test]
    fn basement_lighting_not_auto_created_from_csv_for_crawlspace() {
        let mut schedule = make_schedule_with_basement_lighting_column(&[0.02, 0.01, 0.005]);
        let mut specs: Vec<EquipmentSpec> = Vec::new();

        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            None,
            &DefaultsStore::empty(),
            Some("Crawlspace"),
            false,
            &mut Vec::new(),
        )
        .expect("inject_schedule_into_specs should succeed");

        assert!(
            !specs.iter().any(|s| s.name == "Basement Lighting"),
            "Basement Lighting must not be auto-created from CSV column when foundation is Crawlspace"
        );
    }

    fn unread_column_warnings<'a>(warnings: &'a [Warning], column: &str) -> Vec<&'a Warning> {
        warnings
            .iter()
            .filter(|w| w.message.contains(&format!("'{column}'")))
            .collect()
    }

    /// Injects a one-column schedule into a dwelling with no specs, a
    /// crawlspace foundation and no garage, returning the specs created and
    /// the warnings raised.
    fn inject_into_crawlspace_home(
        column: &str,
        values: &[f64],
    ) -> (Vec<EquipmentSpec>, Vec<Warning>) {
        let mut schedule = make_schedule_with_column(column, values);
        let mut specs: Vec<EquipmentSpec> = Vec::new();
        let mut warnings = Vec::new();
        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            None,
            &DefaultsStore::empty(),
            Some("Crawlspace"),
            false,
            &mut warnings,
        )
        .expect("inject_schedule_into_specs should succeed");
        (specs, warnings)
    }

    #[test]
    fn basement_lighting_load_without_finished_basement_is_a_warning() {
        let (specs, warnings) =
            inject_into_crawlspace_home("lighting_basement", &[0.0, 0.01, 0.005]);

        assert!(
            !specs.iter().any(|s| s.name == "Basement Lighting"),
            "Basement Lighting must not be auto-created without a finished basement"
        );
        let matching = unread_column_warnings(&warnings, "lighting_basement");
        assert_eq!(
            matching.len(),
            1,
            "the unread lighting_basement load must be reported once, got {warnings:?}"
        );
        assert!(
            matching[0].message.contains("Finished Basement"),
            "the warning must name the foundation rule, got: {}",
            matching[0].message
        );
    }

    #[test]
    fn all_zero_basement_lighting_column_without_finished_basement_is_not_a_warning() {
        let (specs, warnings) = inject_into_crawlspace_home("lighting_basement", &[0.0; 3]);

        assert!(
            !specs.iter().any(|s| s.name == "Basement Lighting"),
            "Basement Lighting must not be auto-created without a finished basement"
        );
        assert!(
            unread_column_warnings(&warnings, "lighting_basement").is_empty(),
            "a column with no load needs no action, got {warnings:?}"
        );
    }

    /// OS-HPXML reads `lighting_garage` only when garage lighting exists and
    /// does not warn otherwise; the ResStock 2024 schedules carry a non-zero
    /// column for every home.
    #[test]
    fn garage_lighting_column_without_a_garage_is_not_a_warning() {
        let (specs, warnings) =
            inject_into_crawlspace_home("lighting_garage", &[0.02, 0.01, 0.005]);

        assert!(
            !specs.iter().any(|s| s.name == "Garage Lighting"),
            "Garage Lighting must not be auto-created for a dwelling with no garage"
        );
        assert!(
            unread_column_warnings(&warnings, "lighting_garage").is_empty(),
            "an unread lighting_garage column is not a warning, got {warnings:?}"
        );
    }

    /// OCHRE schedule.py:390-391: when Basement Lighting equipment exists and
    /// the schedule file has `lighting_interior` but not `lighting_basement`,
    /// the interior column is copied so basement lighting follows the
    /// interior profile.
    #[test]
    fn basement_lighting_uses_interior_csv_column_when_basement_column_absent() {
        let mut schedule = make_schedule_with_lighting_column(&[0.02, 0.01, 0.005]);
        // Powered specs: the subject here is the column copy, not the
        // power determination, and an auto-created Indoor Lighting spec
        // would carry no annual energy and fail the resolve.
        let mut specs = vec![
            make_spec("Indoor Lighting", 100.0),
            make_spec("Basement Lighting", 100.0),
        ];

        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            None,
            &DefaultsStore::empty(),
            Some("Finished Basement"),
            false,
            &mut Vec::new(),
        )
        .expect("inject_schedule_into_specs should succeed");

        let basement_idx = schedule
            .column_index
            .get("lighting_basement")
            .expect("lighting_basement column must be copied from lighting_interior");
        let interior_idx = schedule.column_index["lighting_interior"];
        assert_eq!(
            schedule.columns[*basement_idx], schedule.columns[interior_idx],
            "copied lighting_basement column must match lighting_interior values"
        );
        let basement = specs
            .iter()
            .find(|s| s.name == "Basement Lighting")
            .expect("Basement Lighting spec");
        assert_eq!(
            basement
                .parameters
                .get("power_schedule_source")
                .and_then(Value::as_str),
            Some("column"),
            "Basement Lighting must resolve its power schedule from the copied column"
        );
    }

    /// The copy is scoped to Basement Lighting equipment: no basement spec,
    /// no `lighting_basement` column.
    #[test]
    fn interior_lighting_column_not_copied_without_basement_spec() {
        let mut schedule = make_schedule_with_lighting_column(&[0.02, 0.01, 0.005]);
        let mut specs = vec![make_spec("Indoor Lighting", 100.0)];

        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            None,
            &DefaultsStore::empty(),
            Some("Finished Basement"),
            false,
            &mut Vec::new(),
        )
        .expect("inject_schedule_into_specs should succeed");

        assert!(
            !schedule.column_index.contains_key("lighting_basement"),
            "lighting_interior must not be copied without a Basement Lighting spec"
        );
    }

    /// An explicit `lighting_basement` column is never overwritten by the
    /// interior column.
    #[test]
    fn basement_lighting_column_not_overwritten_by_interior_copy() {
        let start =
            DateTime::parse_from_rfc3339("2025-01-01T00:00:00+00:00").expect("valid datetime");
        let timestamps = (0..3)
            .map(|i| start + Duration::hours(i as i64))
            .collect::<Vec<_>>();
        let mut column_index = HashMap::new();
        column_index.insert("lighting_interior".to_string(), 0);
        column_index.insert("lighting_basement".to_string(), 1);
        let mut schedule = ScheduleTimeSeries {
            timestamps,
            column_names: vec![
                "lighting_interior".to_string(),
                "lighting_basement".to_string(),
            ],
            columns: vec![vec![0.02, 0.01, 0.005], vec![0.9, 0.8, 0.7]],
            column_index,
            source_step_secs: 3600,
            column_aggregations: vec![
                crate::ColumnAggregation::Mean,
                crate::ColumnAggregation::Mean,
            ],
        };
        let basement_original = schedule.columns[1].clone();
        // Powered specs: the subject here is column preservation, not the
        // power determination.
        let mut specs = vec![
            make_spec("Indoor Lighting", 100.0),
            make_spec("Basement Lighting", 100.0),
        ];

        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            None,
            &DefaultsStore::empty(),
            Some("Finished Basement"),
            false,
            &mut Vec::new(),
        )
        .expect("inject_schedule_into_specs should succeed");

        assert_eq!(
            schedule.columns[1], basement_original,
            "explicit lighting_basement column must be preserved"
        );
        assert_eq!(
            schedule
                .column_names
                .iter()
                .filter(|name| name.as_str() == "lighting_basement")
                .count(),
            1,
            "lighting_basement must not be duplicated by the interior copy"
        );
    }

    /// Builds the CSV column map from a schedule file's header line.
    fn header_map(path: &std::path::Path) -> HashMap<String, usize> {
        let header = std::fs::read_to_string(path)
            .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
        let line = header
            .lines()
            .next()
            .unwrap_or_else(|| panic!("{} has a header line", path.display()));
        line.split(',')
            .enumerate()
            .map(|(i, name)| (name.trim().trim_matches('"').to_string(), i))
            .collect()
    }

    /// Every schedule column in the fixture tree (ResStock, parity and the
    /// BEopt example) is read by the engine or stated as not used: the
    /// unknown-column check returns nothing for all of them.
    #[test]
    fn fixture_schedule_columns_are_all_known() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");

        let mut schedule_files: Vec<std::path::PathBuf> = Vec::new();
        let resstock = root.join("tests/fixtures/resstock");
        if resstock.is_dir() {
            for year in std::fs::read_dir(&resstock).expect("resstock years") {
                let year = year.expect("year entry").path();
                if !year.is_dir() {
                    continue;
                }
                for bldg in std::fs::read_dir(&year).expect("resstock buildings") {
                    let bldg = bldg.expect("bldg entry").path();
                    let schedules = bldg.join("in.schedules.csv");
                    if schedules.is_file() {
                        schedule_files.push(schedules);
                    }
                }
            }
        }
        let parity = root.join("tests/fixtures/parity");
        if parity.is_dir() {
            for case in std::fs::read_dir(&parity).expect("parity cases") {
                let case = case.expect("case entry").path();
                let schedule = case.join("schedule.csv");
                if schedule.is_file() {
                    schedule_files.push(schedule);
                }
            }
        }
        let beopt = root.join("data/examples/BEopt_example_schedule.csv");
        if beopt.is_file() {
            schedule_files.push(beopt);
        }
        assert!(
            schedule_files.len() >= 40,
            "the fixture census must find the schedule files, found {}",
            schedule_files.len()
        );

        for file in &schedule_files {
            let map = header_map(file);
            let unknown = find_unknown_schedule_columns(&map);
            assert!(
                unknown.is_empty(),
                "{} carries unknown schedule columns {unknown:?}",
                file.display()
            );
        }
    }

    /// An unknown column warns through the collector, naming the column, and
    /// does not fail the injection.
    #[test]
    fn unknown_schedule_column_warns_naming_it() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let fixture = root.join("tests/fixtures/parity/cz4a_ashp_hpwh/schedule.csv");
        let csv = std::fs::read_to_string(&fixture).expect("read the parity schedule");
        let rows: Vec<String> = csv
            .lines()
            .enumerate()
            .map(|(i, line)| {
                if i == 0 {
                    format!("{line},lighting_interor")
                } else {
                    format!("{line},0.0")
                }
            })
            .collect();
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("schedule.csv");
        std::fs::write(&path, rows.join("\n")).expect("write modified schedule");

        let mut schedule = crate::parse_schedule_csv(&path, &[], None, None)
            .expect("the modified parity schedule parses");
        let mut specs: Vec<EquipmentSpec> = Vec::new();
        let mut warnings: Vec<Warning> = Vec::new();
        inject_schedule_into_specs(
            &mut specs,
            &mut schedule,
            None,
            &DefaultsStore::empty(),
            None,
            false,
            &mut warnings,
        )
        .expect("an unknown column is a warning, not an error");

        let schedule_warnings: Vec<&Warning> = warnings
            .iter()
            .filter(|w| w.source.as_ref() == "schedule")
            .collect();
        assert_eq!(
            schedule_warnings.len(),
            1,
            "one schedule warning for the one unknown column, got {warnings:?}"
        );
        assert!(
            schedule_warnings[0].message.contains("lighting_interor"),
            "the warning names the column: {}",
            schedule_warnings[0].message
        );
    }

    /// Census over every checked-in HPXML: the equipment specs whose power
    /// schedule has no source (no schedule CSV column, no HPXML fractions, no
    /// default profile) are exactly the pinned list. Each entry ran at a
    /// constant power under a "THIS MAY BE INCORRECT" log line before that
    /// fallback became the three-source error; the list is the record of
    /// which specs the error can surface when the run's schedule file lacks
    /// their column. Files that do not parse as HPXML have no specs and are
    /// counted, not silently skipped.
    #[test]
    fn census_specs_without_any_schedule_source_across_checked_in_hpxmls() {
        use std::path::{Path, PathBuf};

        fn collect_xmls(dir: &Path, out: &mut Vec<PathBuf>) {
            let mut entries: Vec<_> = std::fs::read_dir(dir)
                .expect("read fixture directory")
                .map(|entry| entry.expect("fixture directory entry").path())
                .collect();
            entries.sort();
            for entry in entries {
                if entry.is_dir() {
                    collect_xmls(&entry, out);
                } else if entry.extension().is_some_and(|ext| ext == "xml") {
                    out.push(entry);
                }
            }
        }

        let fixtures_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
        let mut xml_paths = Vec::new();
        collect_xmls(&fixtures_root, &mut xml_paths);
        assert!(
            xml_paths.len() > 40,
            "the census must walk the checked-in HPXML corpus, found {}",
            xml_paths.len()
        );

        let defaults_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../defaults");
        let profiles = super::load_default_profiles(&defaults_dir)
            .expect("the shipped defaults CSV must load");
        let defaults = DefaultsStore::load(&defaults_dir).expect("the shipped defaults load");
        let overrides = Value::Object(Map::new());

        let mut unreachable: Vec<String> = Vec::new();
        let mut unparsed: Vec<String> = Vec::new();
        let mut unresolved: Vec<String> = Vec::new();
        let mut power_specs_considered = 0usize;
        for path in &xml_paths {
            let Ok(building) = crate::parse_hpxml(path) else {
                unparsed.push(
                    path.strip_prefix(&fixtures_root)
                        .expect("the walked paths live under the fixtures root")
                        .display()
                        .to_string(),
                );
                continue;
            };
            let mut warnings: Vec<Warning> = Vec::new();
            // A fixture whose equipment cannot resolve standalone (it needs
            // the weather site or patches its own test supplies) has no
            // census here: it is counted in `unresolved`, pinned below, so a
            // resolve failure is never silently eaten.
            let specs = match crate::resolve_equipment(
                &building,
                &defaults,
                &overrides,
                None,
                &mut warnings,
            ) {
                Ok(specs) => specs,
                Err(_) => {
                    unresolved.push(
                        path.strip_prefix(&fixtures_root)
                            .expect("the walked paths live under the fixtures root")
                            .display()
                            .to_string(),
                    );
                    continue;
                }
            };
            for spec in &specs {
                let Some(mapping) = super::COLUMN_MAPPINGS.iter().find(|m| {
                    matches!(m.category, super::ScheduleCategory::Power)
                        && m.equipment_name == spec.name
                }) else {
                    continue;
                };
                power_specs_considered += 1;
                if super::resolve_hpxml_profile(spec).is_none()
                    && profiles.find(mapping.equipment_name).is_none()
                {
                    let relative = path
                        .strip_prefix(&fixtures_root)
                        .expect("the walked paths live under the fixtures root")
                        .display()
                        .to_string();
                    unreachable.push(format!("{relative}: {}", spec.name));
                }
            }
        }
        unreachable.sort();
        unreachable.dedup();
        assert!(
            power_specs_considered > 100,
            "the census must examine the corpus's power specs, saw {power_specs_considered}"
        );
        println!(
            "census: {power_specs_considered} power specs across {} checked-in \
             HPXMLs, {} with no schedule source of any kind",
            xml_paths.len() - unparsed.len() - unresolved.len(),
            unreachable.len()
        );

        let pinned: Vec<String> = Vec::new();
        assert_eq!(
            unreachable, pinned,
            "the no-source census moved: every new entry is a spec the \
             three-source error can now fail construction on"
        );
        // The full dwelling entrance (schema and domain validation) rejects
        // this fixture; it is exercised only through the raw string parser
        // in the defaults regressions, so it has no census here.
        assert_eq!(
            unparsed,
            vec!["hpxml/ochre_samples/base-enclosure-windows-physical-properties.xml".to_string()],
            "the set of fixture XMLs the dwelling entrance cannot parse moved"
        );
        // These ResStock homes resolve only with the data patches their
        // runs supply (site location and equipment metadata), never
        // standalone; their census is their own run's business.
        assert_eq!(
            unresolved,
            [
                "resstock/2024.2/bldg0000002/home.xml",
                "resstock/2024.2/bldg0000004/home.xml",
                "resstock/2024.2/bldg0000005/home.xml",
                "resstock/2024.2/bldg0000006/home.xml",
                "resstock/2024.2/bldg0000007/home.xml",
                "resstock/2024.2/bldg0000008/home.xml",
                "resstock/2024.2/bldg0000010/home.xml",
                "resstock/2024.2/bldg0174177/home.xml",
                "resstock/2024.2/bldg0449186/home.xml",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<String>>(),
            "the set of fixture HPXMLs that cannot resolve without their \
             run's data patches moved"
        );
    }
}
