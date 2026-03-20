//! Synthetic (BESTEST-style) dwelling construction from TOML config.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use hares_io::{
    Building, ColumnAggregation, ScheduleTimeSeries, WeatherMeta, WeatherTimeSeries,
};
use hares_types::HaresError;
use serde::Deserialize;
use serde_json::Value;

use super::conversions::duration_to_u32_secs;
use super::Result;

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SyntheticTomlConfig {
    #[serde(default)]
    pub(crate) building_id: Option<i64>,
    pub(crate) simulation: SyntheticSimulationConfig,
    pub(crate) geometry: SyntheticGeometryConfig,
    pub(crate) materials: SyntheticMaterialsConfig,
    pub(crate) hvac: SyntheticHvacConfig,
    #[serde(default)]
    pub(crate) weather: SyntheticWeatherConfig,
    #[serde(default)]
    pub(crate) schedule: SyntheticScheduleConfig,
    #[serde(default)]
    pub(crate) overrides: Option<Value>,
    #[serde(default)]
    pub(crate) output: SyntheticOutputConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SyntheticSimulationConfig {
    pub(crate) start_time: DateTime<Utc>,
    pub(crate) time_res_s: i64,
    pub(crate) duration_s: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SyntheticGeometryConfig {
    pub(crate) floor_area_m2: f64,
    pub(crate) zone_volume_m3: f64,
    #[serde(default = "default_wall_area_m2")]
    pub(crate) wall_area_m2: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SyntheticMaterialsConfig {
    pub(crate) wall_r_value_m2_k_w: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SyntheticHvacConfig {
    pub(crate) equipment_name: String,
    #[serde(default)]
    pub(crate) fuel: Option<String>,
    #[serde(default)]
    pub(crate) heating_capacity_kbtu_h: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SyntheticWeatherConfig {
    #[serde(default = "default_outdoor_temp_c")]
    pub(crate) outdoor_temp_c: f64,
    #[serde(default = "default_dew_point_c")]
    pub(crate) dew_point_c: f64,
    #[serde(default = "default_rel_humidity_pct")]
    pub(crate) rel_humidity_pct: f64,
    #[serde(default = "default_pressure_kpa")]
    pub(crate) pressure_kpa: f64,
}

impl Default for SyntheticWeatherConfig {
    fn default() -> Self {
        Self {
            outdoor_temp_c: default_outdoor_temp_c(),
            dew_point_c: default_dew_point_c(),
            rel_humidity_pct: default_rel_humidity_pct(),
            pressure_kpa: default_pressure_kpa(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SyntheticScheduleConfig {
    #[serde(default = "default_schedule_value")]
    pub(crate) occupancy: f64,
}

impl Default for SyntheticScheduleConfig {
    fn default() -> Self {
        Self {
            occupancy: default_schedule_value(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SyntheticOutputConfig {
    #[serde(default)]
    pub(crate) output_verbosity: u8,
    #[serde(default)]
    pub(crate) output_path: Option<String>,
    #[serde(default)]
    pub(crate) output_format: hares_io::OutputFormat,
    #[serde(default = "default_output_chunk_size")]
    pub(crate) output_chunk_size: usize,
    #[serde(default)]
    pub(crate) master_seed: u64,
}

impl Default for SyntheticOutputConfig {
    fn default() -> Self {
        Self {
            output_verbosity: 0,
            output_path: None,
            output_format: hares_io::OutputFormat::Csv,
            output_chunk_size: default_output_chunk_size(),
            master_seed: 0,
        }
    }
}

fn default_wall_area_m2() -> f64 {
    120.0
}

fn default_outdoor_temp_c() -> f64 {
    10.0
}

fn default_dew_point_c() -> f64 {
    5.0
}

fn default_rel_humidity_pct() -> f64 {
    50.0
}

fn default_pressure_kpa() -> f64 {
    101.325
}

fn default_schedule_value() -> f64 {
    1.0
}

fn default_output_chunk_size() -> usize {
    10_000
}

pub(crate) fn build_synthetic_building(config: &SyntheticTomlConfig) -> Building {
    use hares_io::hpxml::{Boundary, BoundaryType, Site, Zone, ZoneType};

    let heating_capacity = config.hvac.heating_capacity_kbtu_h.unwrap_or(30.0);
    let floor_area = if config.geometry.floor_area_m2 > 0.0 {
        config.geometry.floor_area_m2
    } else {
        config.geometry.zone_volume_m3 / 2.5
    };
    let fuel = config
        .hvac
        .fuel
        .clone()
        .unwrap_or_else(|| "natural gas".to_string());

    let details_xml = hares_io::hpxml::building::XmlNode {
        name: "BuildingDetails".to_string(),
        attrs: HashMap::new(),
        text: String::new(),
        children: vec![hares_io::hpxml::building::XmlNode {
            name: "Systems".to_string(),
            attrs: HashMap::new(),
            text: String::new(),
            children: vec![hares_io::hpxml::building::XmlNode {
                name: "HVAC".to_string(),
                attrs: HashMap::new(),
                text: String::new(),
                children: vec![hares_io::hpxml::building::XmlNode {
                    name: "HeatingSystem".to_string(),
                    attrs: HashMap::new(),
                    text: String::new(),
                    children: vec![
                        hares_io::hpxml::building::XmlNode {
                            name: "HeatingSystemType".to_string(),
                            attrs: HashMap::new(),
                            text: config.hvac.equipment_name.clone(),
                            children: Vec::new(),
                        },
                        hares_io::hpxml::building::XmlNode {
                            name: "HeatingSystemFuel".to_string(),
                            attrs: HashMap::new(),
                            text: fuel,
                            children: Vec::new(),
                        },
                        hares_io::hpxml::building::XmlNode {
                            name: "HeatingCapacity".to_string(),
                            attrs: HashMap::new(),
                            text: (heating_capacity * 1000.0).to_string(),
                            children: Vec::new(),
                        },
                    ],
                }],
            }],
        }],
    };

    Building {
        site: Site {
            elevation_m: Some(0.0),
            site_type: None,
            shielding_of_home: None,
            latitude_deg: Some(39.0),
            longitude_deg: Some(-105.0),
        },
        zones: vec![Zone {
            zone_type: ZoneType::Conditioned,
            floor_area_m2: Some(floor_area),
            volume_m3: Some(config.geometry.zone_volume_m3),
            attached_wall_ids: vec!["wall-1".to_string()],
            duct_systems: Vec::new(),
            vented: false,
            ventilation_ach: None,
            ventilation_sla: None,
        }],
        boundaries: vec![Boundary {
            id: "wall-1".to_string(),
            boundary_type: BoundaryType::Wall,
            area_m2: config.geometry.wall_area_m2,
            azimuth_deg: Some(180.0),
            assembly_r_value_m2_k_w: Some(config.materials.wall_r_value_m2_k_w),
            r_value_layers_m2_k_w: vec![config.materials.wall_r_value_m2_k_w],
            interior_zone: Some(ZoneType::Conditioned),
            exterior_zone: Some(ZoneType::Outdoor),
            material_layers: Vec::new(),
            construction_type: None,
            finish_type: None,
            insulation_details: None,
            has_radiant_barrier: false,
            solar_absorptance: None,
            emittance: None,
            tilt_deg: Some(90.0),
        }],
        windows: Vec::new(),
        infiltration_ach50: None,
        hvac_capacity_w: Some(heating_capacity),
        seer2: None,
        hspf2: None,
        water_heater_setpoint_c: None,
        heating_weekday_setpoints_c: None,
        heating_weekend_setpoints_c: None,
        cooling_weekday_setpoints_c: None,
        cooling_weekend_setpoints_c: None,
        battery_round_trip_efficiency: None,
        pv_tilt_deg: None,
        conditioned_volume_m3: Some(config.geometry.zone_volume_m3),
        ceiling_height_m: None,
        infiltration_height_m: None,
        floors_above_grade: None,
        has_flue_or_chimney: None,
        details_xml,
    }
}

pub(crate) fn build_synthetic_weather(config: &SyntheticTomlConfig) -> WeatherTimeSeries {
    let n = 8760usize;
    let meta = WeatherMeta {
        location: "Synthetic".to_string(),
        latitude: 39.0,
        longitude: -105.0,
        timezone_offset_h: 0.0,
        elevation_m: 0.0,
    };
    WeatherTimeSeries {
        meta,
        dry_bulb_c: vec![config.weather.outdoor_temp_c; n],
        dew_point_c: vec![config.weather.dew_point_c; n],
        rel_humidity_pct: vec![config.weather.rel_humidity_pct; n],
        pressure_kpa: vec![config.weather.pressure_kpa; n],
        ghi_w_m2: vec![0.0; n],
        dni_w_m2: vec![0.0; n],
        dhi_w_m2: vec![0.0; n],
        wind_speed_m_s: vec![0.0; n],
        wind_dir_deg: vec![0.0; n],
        opaque_sky_cover: vec![0.0; n],
        horizontal_infrared_w_m2: vec![300.0; n],
        sky_temp_c: vec![config.weather.outdoor_temp_c; n],
        ground_temp_c: vec![config.weather.outdoor_temp_c; n],
    }
}

pub(crate) fn build_synthetic_schedule(config: &SyntheticTomlConfig) -> Result<ScheduleTimeSeries> {
    use chrono::{FixedOffset, TimeDelta};

    let step_secs = duration_to_u32_secs(Duration::seconds(config.simulation.time_res_s))?;
    let total_steps = (config.simulation.duration_s / config.simulation.time_res_s).max(1) as usize;
    let offset = FixedOffset::east_opt(0)
        .ok_or_else(|| HaresError::Io("failed to build UTC offset".to_string()))?;
    let start = config.simulation.start_time.with_timezone(&offset);

    let mut timestamps = Vec::with_capacity(total_steps);
    for i in 0..total_steps {
        timestamps.push(start + TimeDelta::seconds((i as i64) * i64::from(step_secs)));
    }

    let column_names = vec!["occupancy".to_string()];
    let columns = vec![vec![config.schedule.occupancy; total_steps]];
    let column_index = HashMap::from([("occupancy".to_string(), 0usize)]);
    Ok(ScheduleTimeSeries {
        timestamps,
        column_names,
        columns,
        column_index,
        source_step_secs: step_secs,
        column_aggregations: vec![ColumnAggregation::Mean],
    })
}

#[cfg(feature = "profiling")]
pub(crate) fn current_process_hwm_kb() -> u64 {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return 0;
    };

    status
        .lines()
        .find_map(|line| {
            if !line.starts_with("VmHWM:") {
                return None;
            }
            line.split_whitespace().nth(1)?.parse::<u64>().ok()
        })
        .unwrap_or(0)
}

#[cfg(feature = "profiling")]
pub(crate) fn hot_path_alloc_counter() -> u64 {
    0
}
