//! Synthetic (BESTEST-style) dwelling construction from TOML config.

use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, Duration, FixedOffset};
use hares_io::{Building, ColumnAggregation, ScheduleTimeSeries, WeatherMeta, WeatherTimeSeries};
use serde::Deserialize;
use serde_json::Value;

use super::Result;
use super::conversions::duration_to_u32_secs;

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
    #[serde(default)]
    pub(crate) boundaries: Option<Vec<SyntheticBoundaryConfig>>,
    #[serde(default)]
    pub(crate) windows: Option<Vec<SyntheticWindowConfig>>,
    #[serde(default)]
    pub(crate) setpoints: Option<SyntheticSetpointConfig>,
    #[serde(default)]
    pub(crate) infiltration: Option<SyntheticInfiltrationConfig>,
    #[serde(default)]
    #[allow(dead_code)] // parsed from TOML, wired in future ticket
    pub(crate) internal_gains_w: Option<f64>,
    #[serde(default)]
    pub(crate) internal_gains_constant: Option<bool>,
    #[serde(default)]
    pub(crate) internal_gains_sensible_fraction: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SyntheticSimulationConfig {
    pub(crate) start_time: DateTime<FixedOffset>,
    pub(crate) time_res_s: i64,
    pub(crate) duration_s: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SyntheticGeometryConfig {
    pub(crate) floor_area_m2: f64,
    pub(crate) zone_volume_m3: f64,
    #[serde(default = "default_wall_area_m2")]
    pub(crate) wall_area_m2: f64,
    #[serde(default)]
    pub(crate) mass_multiplier: Option<f64>,
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
    #[serde(default)]
    pub(crate) deadband_c: Option<f64>,
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
    #[serde(default)]
    pub(crate) epw_path: Option<String>,
}

impl Default for SyntheticWeatherConfig {
    fn default() -> Self {
        Self {
            outdoor_temp_c: default_outdoor_temp_c(),
            dew_point_c: default_dew_point_c(),
            rel_humidity_pct: default_rel_humidity_pct(),
            pressure_kpa: default_pressure_kpa(),
            epw_path: None,
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
    #[serde(default = "default_write_output")]
    pub(crate) write_output: bool,
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
            write_output: default_write_output(),
            output_format: hares_io::OutputFormat::Csv,
            output_chunk_size: default_output_chunk_size(),
            master_seed: 0,
        }
    }
}

fn default_write_output() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SyntheticBoundaryConfig {
    pub(crate) id: String,
    pub(crate) boundary_type: String,
    pub(crate) area_m2: f64,
    #[serde(default)]
    pub(crate) azimuth_deg: Option<f64>,
    #[serde(default)]
    pub(crate) tilt_deg: Option<f64>,
    #[serde(default)]
    pub(crate) solar_absorptance: Option<f64>,
    #[serde(default)]
    pub(crate) emittance: Option<f64>,
    #[serde(default = "default_interior_zone")]
    pub(crate) interior_zone: String,
    #[serde(default = "default_exterior_zone")]
    pub(crate) exterior_zone: String,
    #[serde(default)]
    pub(crate) material_layers: Vec<SyntheticMaterialLayer>,
    #[serde(default)]
    pub(crate) r_value_m2_k_w: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SyntheticMaterialLayer {
    pub(crate) thickness_m: f64,
    pub(crate) conductivity_w_m_k: f64,
    #[serde(default)]
    pub(crate) density_kg_m3: f64,
    #[serde(default)]
    pub(crate) specific_heat_j_kg_k: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SyntheticWindowConfig {
    pub(crate) id: String,
    pub(crate) area_m2: f64,
    #[serde(default)]
    pub(crate) azimuth_deg: Option<f64>,
    pub(crate) u_factor_w_m2_k: f64,
    pub(crate) shgc: f64,
    #[serde(default)]
    pub(crate) attached_to_wall_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SyntheticSetpointConfig {
    #[serde(default)]
    pub(crate) heating_c: Option<f64>,
    pub(crate) cooling_c: f64,
    /// Optional 24-element hourly heating setpoint profile (°C).
    /// When present, overrides `heating_c` with an hourly schedule.
    #[serde(default)]
    pub(crate) heating_schedule_c: Option<Vec<f64>>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SyntheticInfiltrationConfig {
    /// Continuous ACH (not ACH50).
    pub(crate) ach: f64,
    /// Optional constant internal gains when provided under `[infiltration]`
    /// in synthetic BESTEST-style fixtures.
    #[serde(default)]
    pub(crate) internal_gains_w: Option<f64>,
}

fn default_interior_zone() -> String {
    "Conditioned".to_string()
}

fn default_exterior_zone() -> String {
    "Outdoor".to_string()
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

fn parse_zone_type(s: &str) -> hares_io::hpxml::ZoneType {
    use hares_io::hpxml::ZoneType;
    match s {
        "Conditioned" => ZoneType::Conditioned,
        "Outdoor" => ZoneType::Outdoor,
        "Ground" => ZoneType::Ground,
        "Attic" => ZoneType::Attic,
        "Garage" => ZoneType::Garage,
        "Foundation" => ZoneType::Foundation,
        other => ZoneType::Other(other.to_string()),
    }
}

fn parse_boundary_type(s: &str) -> hares_io::hpxml::BoundaryType {
    use hares_io::hpxml::BoundaryType;
    match s {
        "Wall" => BoundaryType::Wall,
        "Roof" => BoundaryType::Roof,
        "Floor" => BoundaryType::Floor,
        "Door" => BoundaryType::Door,
        "FoundationWall" => BoundaryType::FoundationWall,
        "RimJoist" => BoundaryType::RimJoist,
        "Slab" => BoundaryType::Slab,
        other => BoundaryType::Other(other.to_string()),
    }
}

pub(crate) fn build_synthetic_building(config: &SyntheticTomlConfig) -> Building {
    use hares_io::hpxml::{Boundary, BoundaryType, MaterialLayer, Site, Window, Zone, ZoneType};
    use hares_physics::units as conv;

    let heating_capacity_kbtu_h = config.hvac.heating_capacity_kbtu_h.unwrap_or(30.0);
    let heating_capacity_btu_h = heating_capacity_kbtu_h * 1000.0;
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
    let equipment_name_norm = config.hvac.equipment_name.to_ascii_lowercase();
    let is_ideal_hvac = matches!(equipment_name_norm.as_str(), "idealhvac" | "ideal hvac");
    let has_heating = !config.hvac.equipment_name.eq_ignore_ascii_case("none");
    let has_cooling = has_heating && config.setpoints.is_some() && !is_ideal_hvac;
    let internal_gains_w = config.internal_gains_w.or_else(|| {
        config
            .infiltration
            .as_ref()
            .and_then(|i| i.internal_gains_w)
    });

    let mut hvac_children = Vec::new();
    if has_heating {
        hvac_children.push(hares_io::hpxml::building::XmlNode {
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
                    text: fuel.clone(),
                    children: Vec::new(),
                },
                hares_io::hpxml::building::XmlNode {
                    name: "HeatingCapacity".to_string(),
                    attrs: HashMap::new(),
                    text: heating_capacity_btu_h.to_string(),
                    children: Vec::new(),
                },
            ],
        });
    }
    if has_cooling {
        hvac_children.push(hares_io::hpxml::building::XmlNode {
            name: "CoolingSystem".to_string(),
            attrs: HashMap::new(),
            text: String::new(),
            children: vec![
                hares_io::hpxml::building::XmlNode {
                    name: "CoolingSystemType".to_string(),
                    attrs: HashMap::new(),
                    text: "central air conditioner".to_string(),
                    children: Vec::new(),
                },
                hares_io::hpxml::building::XmlNode {
                    name: "CoolingSystemFuel".to_string(),
                    attrs: HashMap::new(),
                    text: "electricity".to_string(),
                    children: Vec::new(),
                },
                hares_io::hpxml::building::XmlNode {
                    name: "CoolingCapacity".to_string(),
                    attrs: HashMap::new(),
                    text: heating_capacity_btu_h.to_string(),
                    children: Vec::new(),
                },
            ],
        });
    }

    let mut details_children = vec![hares_io::hpxml::building::XmlNode {
        name: "Systems".to_string(),
        attrs: HashMap::new(),
        text: String::new(),
        children: vec![hares_io::hpxml::building::XmlNode {
            name: "HVAC".to_string(),
            attrs: HashMap::new(),
            text: String::new(),
            children: hvac_children,
        }],
    }];

    if let Some(internal_gains_w) = internal_gains_w
        && internal_gains_w.is_finite()
        && internal_gains_w > 0.0
    {
        let annual_kwh = internal_gains_w * 8760.0 / 1000.0;
        let mut plug_children = vec![
            hares_io::hpxml::building::XmlNode {
                name: "PlugLoadType".to_string(),
                attrs: HashMap::new(),
                text: "other".to_string(),
                children: Vec::new(),
            },
            hares_io::hpxml::building::XmlNode {
                name: "Load".to_string(),
                attrs: HashMap::new(),
                text: String::new(),
                children: vec![
                    hares_io::hpxml::building::XmlNode {
                        name: "Units".to_string(),
                        attrs: HashMap::new(),
                        text: "kWh/year".to_string(),
                        children: Vec::new(),
                    },
                    hares_io::hpxml::building::XmlNode {
                        name: "Value".to_string(),
                        attrs: HashMap::new(),
                        text: annual_kwh.to_string(),
                        children: Vec::new(),
                    },
                ],
            },
        ];

        let is_constant = config.internal_gains_constant.unwrap_or(false);
        let sensible_frac = config.internal_gains_sensible_fraction;
        if is_constant || sensible_frac.is_some() {
            let mut ext_children = Vec::new();
            if is_constant {
                let flat_24 = std::iter::repeat("0.04167")
                    .take(24)
                    .collect::<Vec<_>>()
                    .join(", ");
                ext_children.push(hares_io::hpxml::building::XmlNode {
                    name: "WeekdayScheduleFractions".to_string(),
                    attrs: HashMap::new(),
                    text: flat_24.clone(),
                    children: Vec::new(),
                });
                ext_children.push(hares_io::hpxml::building::XmlNode {
                    name: "WeekendScheduleFractions".to_string(),
                    attrs: HashMap::new(),
                    text: flat_24,
                    children: Vec::new(),
                });
                ext_children.push(hares_io::hpxml::building::XmlNode {
                    name: "MonthlyScheduleMultipliers".to_string(),
                    attrs: HashMap::new(),
                    text: std::iter::repeat("1.0")
                        .take(12)
                        .collect::<Vec<_>>()
                        .join(", "),
                    children: Vec::new(),
                });
            }
            if let Some(sf) = sensible_frac {
                ext_children.push(hares_io::hpxml::building::XmlNode {
                    name: "FracSensible".to_string(),
                    attrs: HashMap::new(),
                    text: sf.to_string(),
                    children: Vec::new(),
                });
                ext_children.push(hares_io::hpxml::building::XmlNode {
                    name: "FracLatent".to_string(),
                    attrs: HashMap::new(),
                    text: (1.0 - sf).max(0.0).to_string(),
                    children: Vec::new(),
                });
            }
            plug_children.push(hares_io::hpxml::building::XmlNode {
                name: "extension".to_string(),
                attrs: HashMap::new(),
                text: String::new(),
                children: ext_children,
            });
        }

        details_children.push(hares_io::hpxml::building::XmlNode {
            name: "MiscLoads".to_string(),
            attrs: HashMap::new(),
            text: String::new(),
            children: vec![hares_io::hpxml::building::XmlNode {
                name: "PlugLoad".to_string(),
                attrs: HashMap::new(),
                text: String::new(),
                children: plug_children,
            }],
        });
    }

    let details_xml = hares_io::hpxml::building::XmlNode {
        name: "BuildingDetails".to_string(),
        attrs: HashMap::new(),
        text: String::new(),
        children: details_children,
    };

    let mut boundaries: Vec<Boundary> = if let Some(boundary_configs) = &config.boundaries {
        boundary_configs
            .iter()
            .map(|bc| {
                let layers: Vec<MaterialLayer> = bc
                    .material_layers
                    .iter()
                    .map(|ml| MaterialLayer {
                        thickness_m: ml.thickness_m,
                        conductivity_w_m_k: ml.conductivity_w_m_k,
                        density_kg_m3: ml.density_kg_m3,
                        specific_heat_j_kg_k: ml.specific_heat_j_kg_k,
                        area_m2: bc.area_m2,
                    })
                    .collect();

                let r_value = bc.r_value_m2_k_w.or_else(|| {
                    if layers.is_empty() {
                        None
                    } else {
                        let total_r: f64 = layers
                            .iter()
                            .map(|l| l.thickness_m / l.conductivity_w_m_k)
                            .sum();
                        Some(total_r)
                    }
                });

                Boundary {
                    id: bc.id.clone(),
                    boundary_type: parse_boundary_type(&bc.boundary_type),
                    area_m2: bc.area_m2,
                    azimuth_deg: bc.azimuth_deg,
                    assembly_r_value_m2_k_w: r_value,
                    r_value_layers_m2_k_w: r_value.map(|r| vec![r]).unwrap_or_default(),
                    interior_zone: Some(parse_zone_type(&bc.interior_zone)),
                    exterior_zone: Some(parse_zone_type(&bc.exterior_zone)),
                    material_layers: layers,
                    construction_type: None,
                    finish_type: None,
                    insulation_details: None,
                    has_radiant_barrier: false,
                    solar_absorptance: bc.solar_absorptance,
                    emittance: bc.emittance,
                    tilt_deg: bc.tilt_deg,
                    framing_factor: None,
                    lut_boundary_name: None,
                    floor_or_ceiling: None,
                }
            })
            .collect()
    } else {
        vec![Boundary {
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
            framing_factor: None,
            lut_boundary_name: None,
            floor_or_ceiling: None,
        }]
    };

    let windows: Vec<Window> = config
        .windows
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|wc| Window {
            id: wc.id.clone(),
            area_m2: wc.area_m2,
            azimuth_deg: wc.azimuth_deg,
            u_factor_w_m2_k: Some(wc.u_factor_w_m2_k),
            shgc: Some(wc.shgc),
            interior_shading_fraction: 1.0,
            winter_shading_fraction: 1.0,
            fraction_operable: 0.0,
            exterior_shading_summer: 1.0,
            exterior_shading_winter: 1.0,
            attached_to_wall_id: wc.attached_to_wall_id.clone(),
        })
        .collect();

    // Mirror parsed-HPXML behavior by materializing each window as a boundary.
    // Without this, synthetic cases carry window metadata but never wire window
    // solar gains into the thermal solver.
    for win in &windows {
        boundaries.push(Boundary {
            id: win.id.clone(),
            boundary_type: BoundaryType::Window,
            area_m2: win.area_m2,
            azimuth_deg: win.azimuth_deg,
            assembly_r_value_m2_k_w: None,
            r_value_layers_m2_k_w: Vec::new(),
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
            framing_factor: None,
            lut_boundary_name: None,
            floor_or_ceiling: None,
        });
    }

    // Build 24-hour setpoint vectors when setpoints are configured.
    let (heating_weekday, cooling_weekday) = if let Some(sp) = &config.setpoints {
        let heating = if let Some(ref schedule) = sp.heating_schedule_c {
            assert_eq!(schedule.len(), 24, "heating_schedule_c must have exactly 24 elements");
            Some(schedule.clone())
        } else {
            sp.heating_c.map(|t| vec![t; 24])
        };
        (heating, Some(vec![sp.cooling_c; 24]))
    } else {
        (None, None)
    };

    // BESTEST/ASHRAE 140 specifies constant ACH — no weather-dependent model.
    let infiltration_constant_ach = config.infiltration.as_ref().map(|inf| inf.ach);

    let wall_ids: Vec<String> = boundaries
        .iter()
        .filter(|b| b.boundary_type != BoundaryType::Window)
        .map(|b| b.id.clone())
        .collect();

    Building {
        site: Site {
            elevation_m: Some(1609.0),
            site_type: None,
            shielding_of_home: None,
            latitude_deg: Some(39.76),
            longitude_deg: Some(-104.86),
        },
        zones: vec![Zone {
            zone_type: ZoneType::Conditioned,
            floor_area_m2: Some(floor_area),
            volume_m3: Some(config.geometry.zone_volume_m3),
            attached_wall_ids: wall_ids,
            duct_systems: Vec::new(),
            vented: false,
            ventilation_ach: None,
            ventilation_sla: None,
        }],
        boundaries,
        windows,
        infiltration_ach50: None,
        infiltration_cfm50: None,
        infiltration_ela_cm2: None,
        infiltration_constant_ach,
        hvac_capacity_w: Some(conv::power_btu_h_to_w(heating_capacity_btu_h)),
        seer2: None,
        hspf2: None,
        water_heater_setpoint_c: None,
        heating_weekday_setpoints_c: heating_weekday.clone(),
        heating_weekend_setpoints_c: heating_weekday,
        cooling_weekday_setpoints_c: cooling_weekday.clone(),
        cooling_weekend_setpoints_c: cooling_weekday,
        battery_round_trip_efficiency: None,
        pv_tilt_deg: None,
        conditioned_volume_m3: Some(config.geometry.zone_volume_m3),
        ceiling_height_m: None,
        infiltration_height_m: None,
        floors_above_grade: None,
        has_flue_or_chimney: None,
        foundation_name: None,
        residential_facility_type: None,
        mass_multiplier_override: config.geometry.mass_multiplier,
        hvac_deadband_c: config.hvac.deadband_c,
        details_xml,
    }
}

pub(crate) fn build_synthetic_weather(
    config: &SyntheticTomlConfig,
    toml_path: &Path,
) -> super::Result<WeatherTimeSeries> {
    if let Some(epw_rel) = &config.weather.epw_path {
        let base_dir = toml_path.parent().unwrap_or(Path::new("."));
        let epw_path = base_dir.join(epw_rel);
        return hares_io::parse_epw(&epw_path)
            .map_err(|err| hares_types::HaresError::Io(format!("EPW load failed: {err}")));
    }

    let n = 8760usize;
    let meta = WeatherMeta {
        location: "Synthetic".to_string(),
        latitude: 39.76,
        longitude: -104.86,
        timezone_offset_h: 0.0,
        elevation_m: 0.0,
        source_step_secs: 3600,
        midpoint_offset_secs: 0,
    };
    Ok(WeatherTimeSeries {
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
        liquid_precip_m: vec![0.0; n],
        surface_albedo: None,
    })
}

pub(crate) fn build_synthetic_schedule(config: &SyntheticTomlConfig) -> Result<ScheduleTimeSeries> {
    use chrono::TimeDelta;

    let step_secs = duration_to_u32_secs(Duration::seconds(config.simulation.time_res_s))?;
    let total_steps = (config.simulation.duration_s / config.simulation.time_res_s).max(1) as usize;
    let start = config.simulation.start_time;

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
