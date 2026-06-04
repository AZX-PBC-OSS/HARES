//! Synthetic (BESTEST-style) dwelling construction from TOML config.

use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, Duration, FixedOffset};
use hares_io::{Building, ColumnAggregation, ScheduleTimeSeries, WeatherMeta, WeatherTimeSeries};
use hares_physics::solar::{EOT_C0, EOT_C1, EOT_C2, EOT_C3, EOT_C4};
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
    #[serde(default)]
    pub(crate) internal_gains_radiant_fraction: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SyntheticSimulationConfig {
    pub(crate) start_time: DateTime<FixedOffset>,
    pub(crate) time_res_s: i64,
    pub(crate) duration_s: i64,
    #[serde(default)]
    pub(crate) initialization_duration_s: Option<u64>,
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
    /// Constant deep-ground temperature override [°C].
    ///
    /// When `Some(t)`, overrides the temporal-mean ground temperature
    /// approximation. When `None` (default), ground temperature is the
    /// temporal mean of the dry-bulb series — which, for a constant synthetic
    /// profile, equals `outdoor_temp_c`.
    ///
    /// This approximates an undisturbed deep-ground temperature with zero
    /// seasonal amplitude per the Kusuda-Achenbach (1965) model:
    /// T(z,t) → T̄_s as ΔT̄_s → 0.
    /// Cite: Kusuda, T. and Achenbach, P.R. (1965), ASHRAE Trans. 71(1):61-74.
    #[serde(default)]
    pub(crate) ground_temp_c: Option<f64>,
    /// Whether to compute clear-sky solar irradiance (default: true).
    /// When disabled, GHI/DNI/DHI are all zero (the pre-T-0188 behaviour).
    #[serde(default = "default_clear_sky_solar")]
    pub(crate) clear_sky_solar: bool,
    /// ASHRAE 2013 beam optical depth override. `None` uses the default
    /// mid-latitude summer value (τ_b = 0.556 per HoF 2013 Ch.33 Table 9.8).
    #[serde(default)]
    pub(crate) beam_optical_depth: Option<f64>,
    /// ASHRAE 2013 diffuse optical depth override. `None` uses the default
    /// mid-latitude summer value (τ_d = 2.0 per HoF 2013 Ch.33 Table 9.8).
    #[serde(default)]
    pub(crate) diffuse_optical_depth: Option<f64>,
    /// Diurnal temperature amplitude [°C].
    ///
    /// When zero (default), the dry-bulb temperature is constant across all
    /// hours — backward-compatible with pre-T-0189 BESTEST configurations.
    /// When > 0, a sinusoidal diurnal profile with peak in the afternoon is
    /// applied (see `build_synthetic_weather` for the model formula).
    ///
    /// Typical values: 5–10 °C for mid-latitude continental climates
    /// (ASHRAE HoF 2021 Ch.14 Fig.14.7).
    #[serde(default)]
    pub(crate) diurnal_amplitude_c: f64,
    /// Thermal lag from solar noon to peak air temperature [hours].
    ///
    /// Reflects the observed 2–3 hour lag between peak solar irradiance and
    /// peak air temperature due to surface-to-air convective heat transfer
    /// delay. Used as the phase offset in the diurnal sinusoid model.
    ///
    /// ASHRAE HoF 2021 Ch.14 §4 Table 14.6: typical range 2–3 hours for
    /// mid-latitude sites.
    #[serde(default = "default_thermal_lag_h")]
    pub(crate) thermal_lag_h: f64,
    /// Seasonal modulation factor for diurnal amplitude (dimensionless).
    ///
    /// Controls how much larger the diurnal amplitude is in summer versus
    /// winter. The effective amplitude for each day is:
    /// A_effective = A_base × [1 + B × cos(2π × (doy − 172) / 365)]
    /// where `B` is this value and day 172 is the summer solstice (June 21).
    /// First-order astronomical model: annual cosine envelope peaking at the
    /// summer solstice. ASHRAE HoF 2021 Ch.14 §4: design-day and seasonal
    /// temperature variation utilise sinusoidal models anchored to the solstices.
    ///
    /// Zero = no seasonal modulation (constant amplitude year-round).
    /// Typical: 0.3–0.5 for continental climates with strong seasonal variation.
    #[serde(default)]
    pub(crate) seasonal_modulation: f64,
}

fn default_clear_sky_solar() -> bool {
    true
}

fn default_thermal_lag_h() -> f64 {
    2.5
}

impl Default for SyntheticWeatherConfig {
    fn default() -> Self {
        Self {
            outdoor_temp_c: default_outdoor_temp_c(),
            dew_point_c: default_dew_point_c(),
            rel_humidity_pct: default_rel_humidity_pct(),
            pressure_kpa: default_pressure_kpa(),
            epw_path: None,
            ground_temp_c: None,
            clear_sky_solar: true,
            beam_optical_depth: None,
            diffuse_optical_depth: None,
            diurnal_amplitude_c: 0.0,
            thermal_lag_h: default_thermal_lag_h(),
            seasonal_modulation: 0.0,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SyntheticScheduleConfig {
    #[serde(default = "default_schedule_value")]
    pub(crate) occupancy: f64,
    /// Set to `false` when the dwelling legitimately has no occupants (e.g. BESTEST
    /// base cases). When `false` the schedule is built without an occupancy column,
    /// and `occupancy_column_idx` is `None`, causing `apply_occupancy_gains` to
    /// return early with no side effects and no diagnostic.
    #[serde(default = "default_occupants_present")]
    pub(crate) occupants_present: bool,
}

fn default_occupants_present() -> bool {
    true
}

impl Default for SyntheticScheduleConfig {
    fn default() -> Self {
        Self {
            occupancy: default_schedule_value(),
            occupants_present: true,
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
    /// Material density [kg/m³].
    ///
    /// Defaults to `0.0` when omitted — a zero-density layer has **zero
    /// thermal capacitance** and behaves as a pure resistance. This is a
    /// valid steady-state modelling choice, but transient thermal storage
    /// (the "C" nodes in a 3R2C wall model) will be absent. Prefer
    /// setting this explicitly (e.g. 2240 kg/m³ for concrete, 800 kg/m³
    /// for wood, per EnergyPlus `Material` defaults) unless you
    /// intentionally want a massless layer.
    #[serde(default)]
    pub(crate) density_kg_m3: f64,
    /// Material specific heat [J/(kg·K)].
    ///
    /// Defaults to `0.0` when omitted. Together with [`density_kg_m3`]
    /// this controls the layer's volumetric heat capacity
    /// `ρ·c_p·thickness`.  Zero specific heat eliminates transient
    /// thermal storage in the wall assembly.  Typical values:
    /// 900 J/(kg·K) for concrete/masonry, 840 for glass, 1_200 for wood.
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
        let mut heating_children = vec![
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
        ];

        // Combustion-fuel furnaces/boilers need AFUE for the HPXML resolver.
        // 0.80 is the ANSI/RESNET 301 floor for existing equipment.
        let fuel_norm = fuel.to_ascii_lowercase();
        let is_combustion = matches!(
            fuel_norm.as_str(),
            "natural gas" | "propane" | "fuel oil" | "wood" | "wood pellets" | "coal"
        );
        if is_combustion {
            heating_children.push(hares_io::hpxml::building::XmlNode {
                name: "AnnualHeatingEfficiency".to_string(),
                attrs: HashMap::new(),
                text: String::new(),
                children: vec![
                    hares_io::hpxml::building::XmlNode {
                        name: "Units".to_string(),
                        attrs: HashMap::new(),
                        text: "AFUE".to_string(),
                        children: Vec::new(),
                    },
                    hares_io::hpxml::building::XmlNode {
                        name: "Value".to_string(),
                        attrs: HashMap::new(),
                        text: "0.80".to_string(),
                        children: Vec::new(),
                    },
                ],
            });
        } else {
            // Electric resistance heating is by definition 100% efficient
            // at the appliance per ASHRAE HVAC Systems & Equipment 2020 Ch. 33.
            // The HPXML resolver now requires explicit efficiency — a silent
            // default is no longer permitted.
            heating_children.push(hares_io::hpxml::building::XmlNode {
                name: "AnnualHeatingEfficiency".to_string(),
                attrs: HashMap::new(),
                text: String::new(),
                children: vec![
                    hares_io::hpxml::building::XmlNode {
                        name: "Units".to_string(),
                        attrs: HashMap::new(),
                        text: "Percent".to_string(),
                        children: Vec::new(),
                    },
                    hares_io::hpxml::building::XmlNode {
                        name: "Value".to_string(),
                        attrs: HashMap::new(),
                        text: "1.0".to_string(),
                        children: Vec::new(),
                    },
                ],
            });
        }

        hvac_children.push(hares_io::hpxml::building::XmlNode {
            name: "HeatingSystem".to_string(),
            attrs: HashMap::new(),
            text: String::new(),
            children: heating_children,
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
                let flat_24 = std::iter::repeat_n("0.04167", 24)
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
                    text: std::iter::repeat_n("1.0", 12)
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
            if let Some(rf) = config.internal_gains_radiant_fraction {
                ext_children.push(hares_io::hpxml::building::XmlNode {
                    name: "FracRadiant".to_string(),
                    attrs: HashMap::new(),
                    text: rf.to_string(),
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
                    perimeter_m: None,
                    perimeter_insulation_r_m2_k_w: None,
                    foundation_depth_m: None,
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
            perimeter_m: None,
            perimeter_insulation_r_m2_k_w: None,
            foundation_depth_m: None,
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
            perimeter_m: None,
            perimeter_insulation_r_m2_k_w: None,
            foundation_depth_m: None,
        });
    }

    // Build 24-hour setpoint vectors when setpoints are configured.
    let (heating_weekday, cooling_weekday) = if let Some(sp) = &config.setpoints {
        let heating = if let Some(ref schedule) = sp.heating_schedule_c {
            assert_eq!(
                schedule.len(),
                24,
                "heating_schedule_c must have exactly 24 elements"
            );
            Some(schedule.clone())
        } else {
            sp.heating_c.map(|t| vec![t; 24])
        };
        (heating, Some(vec![sp.cooling_c; 24]))
    } else {
        (None, None)
    };

    // BESTEST/ASHRAE 140 specifies constant ACH -- no weather-dependent model.
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
            utc_offset_h: None,
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
        infiltration_ach_natural: None,
        infiltration_cfm_natural: None,
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

/// Spencer (1971) solar altitude for hourly synthetic weather.
///
/// Computes solar altitude [degrees] from latitude, longitude, and the
/// hour-of-year index (0..8759). Uses the same Spencer Fourier-series
/// declination/EOT model as `hares_physics::solar::solar_position`, but
/// operates on hour index directly so no DateTime construction is needed.
///
/// # References
/// - Spencer, J.W. (1971), Search 2(5):172.
/// - ASHRAE HoF 2021 Ch.14 Eq.6: sin(α) = sin(φ)·sin(δ) + cos(φ)·cos(δ)·cos(ω).
fn solar_altitude_spencer(latitude_deg: f64, longitude_deg: f64, hour: u32) -> f64 {
    use std::f64::consts::PI;

    const MINUTES_PER_HOUR_F64: f64 = 60.0;
    const MINUTES_PER_DAY_F64: f64 = 1440.0;
    const SOLAR_NOON_MINUTES_F64: f64 = 720.0;
    const EOT_SCALE: f64 = 229.18;
    const DEGREES_HALF_CIRCLE: f64 = 180.0;
    const DEGREES_FULL_CIRCLE: f64 = 360.0;
    const MINUTES_PER_DEGREE_LONGITUDE: f64 = 4.0;
    const DEGREES_PER_MINUTE_SOLAR: f64 = 0.25;
    const DAYS_PER_YEAR: f64 = 365.0;

    // Spencer (1971) EOT constants from hares_physics::solar.
    // Corrected from misprint noted in pvlib-python (0.000075 → 0.0000075).

    let day = f64::from(hour / 24) + 1.0;
    let minutes_utc = f64::from(hour % 24) * MINUTES_PER_HOUR_F64;

    let gamma = 2.0 * PI / DAYS_PER_YEAR
        * (day - 1.0 + (minutes_utc - SOLAR_NOON_MINUTES_F64) / MINUTES_PER_DAY_F64);

    // Spencer (1971) Fourier series for declination.
    let decl_rad = 0.006_918 - 0.399_912 * gamma.cos() + 0.070_257 * gamma.sin()
        - 0.006_758 * (2.0 * gamma).cos()
        + 0.000_907 * (2.0 * gamma).sin()
        - 0.002_697 * (3.0 * gamma).cos()
        + 0.001_48 * (3.0 * gamma).sin();

    // Spencer (1971) equation of time [minutes].
    let eq_time_min = EOT_SCALE
        * (EOT_C0
            + EOT_C1 * gamma.cos()
            + EOT_C2 * gamma.sin()
            + EOT_C3 * (2.0 * gamma).cos()
            + EOT_C4 * (2.0 * gamma).sin());

    let true_solar_time_min =
        minutes_utc + eq_time_min + MINUTES_PER_DEGREE_LONGITUDE * longitude_deg;
    let mut hour_angle_deg = true_solar_time_min * DEGREES_PER_MINUTE_SOLAR - DEGREES_HALF_CIRCLE;
    if hour_angle_deg < -DEGREES_HALF_CIRCLE {
        hour_angle_deg += DEGREES_FULL_CIRCLE;
    } else if hour_angle_deg > DEGREES_HALF_CIRCLE {
        hour_angle_deg -= DEGREES_FULL_CIRCLE;
    }

    let lat_rad = latitude_deg.to_radians();
    let hour_angle_rad = hour_angle_deg.to_radians();

    let cos_zenith = (lat_rad.sin() * decl_rad.sin()
        + lat_rad.cos() * decl_rad.cos() * hour_angle_rad.cos())
    .clamp(-1.0, 1.0);
    let zenith_rad = cos_zenith.acos();
    90.0 - zenith_rad.to_degrees()
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
        wf_allows_leap_years: true,
        source_step_secs: 3600,
        midpoint_offset_secs: 0,
        has_embedded_location: true,
    };

    let outdoor_temp_c = config.weather.outdoor_temp_c;
    let dew_point_c = config.weather.dew_point_c;

    // Compute clear-sky horizontal IR from Berdahl-Martin emissivity.
    //
    // ε_clear = 0.758 + 0.521·(T_dp/100) + 0.625·(T_dp/100)²
    // IR_horizontal = ε_clear · σ · T_air_K⁴
    //
    // With sky_cover = 0 (clear sky) this is the correct downwelling LWR.
    // The previous value of 300 W/m² was inconsistent with clear sky at
    // typical BESTEST winter conditions (Denver, ~0 °C) where IR ≈ 180–200 W/m².
    //
    // Quadratic functional form: Martin & Berdahl (1984), Solar Energy
    // 33(3/4):321-336. Coefficient values 0.758 / 0.521 / 0.625 are the
    // recalibrated set from Li, Jiang & Coimbra (2017), Solar Energy 144:40-48,
    // as used by EnergyPlus (Engineering Reference, Sky Radiation Modeling).
    let eps_clear = hares_io::berdahl_martin_sky_emissivity(dew_point_c);
    let t_air_k = outdoor_temp_c + hares_io::KELVIN_OFFSET_C;
    let horizontal_ir_w_m2 = eps_clear * hares_io::STEFAN_BOLTZMANN * t_air_k.powi(4);

    // Compute sky temperature from the IR we just computed, using the same
    // Stefan-Boltzmann inversion as the EPW/TMY3 pipeline.
    //
    // T_sky = (IR / σ)^0.25 − 273.15
    //
    // This ensures sky_temp_c and horizontal_infrared_w_m2 are physically
    // consistent with each other and with opaque_sky_cover = 0.0.
    // The previous code set sky_temp_c = outdoor_temp_c, which eliminated ALL
    // longwave radiative cooling to the sky — the clear sky is typically
    // 10–30 °C colder than ambient air.
    //
    // Cite: Clark, G. and Allen, C. (1978), "The Estimation of Atmospheric
    // Radiation for Clear and Cloudy Skies", Proc. 2nd National Passive Solar
    // Conference, pp. 675-678.
    // Cite: EnergyPlus WeatherManager.cc:3113 (sky temp recomputation).
    let sky_temp_c = hares_io::compute_sky_temp_c(
        horizontal_ir_w_m2,
        outdoor_temp_c,
        dew_point_c,
        50.0, // rel_humidity_pct — not used (IR path always active for synthetic)
        0.0,  // opaque_sky_cover = 0 (clear sky)
        hares_io::SkyTempModel::default(),
    );

    // Clear-sky solar irradiance via the ASHRAE 2013 model.
    //
    // When clear_sky_solar is enabled (default) and no EPW path is provided,
    // compute GHI, DNI, and DHI for all 8760 hours using the Spencer (1971)
    // solar geometry and the ASHRAE HoF 2013 clear-sky model.
    // Cite: Spencer, J.W. (1971), Search 2(5):172.
    // Cite: ASHRAE HoF 2013 Ch.33 Table 9.8.
    let (ghi_w_m2, dni_w_m2, dhi_w_m2) = if config.weather.clear_sky_solar {
        let latitude_deg = meta.latitude;
        let longitude_deg = meta.longitude;
        let beam_tau = config.weather.beam_optical_depth.unwrap_or(0.556); // ASHRAE HoF 2013 Ch.33 Table 9.8
        let diffuse_tau = config.weather.diffuse_optical_depth.unwrap_or(2.0); // ASHRAE HoF 2013 Ch.33 Table 9.8

        let mut ghi = Vec::with_capacity(n);
        let mut dni = Vec::with_capacity(n);
        let mut dhi = Vec::with_capacity(n);

        for hour in 0..n {
            let day_of_year = (hour / 24) as u32 + 1;
            let altitude_deg = solar_altitude_spencer(latitude_deg, longitude_deg, hour as u32);
            let (dni_val, dhi_val, ghi_val) = hares_physics::solar::clear_sky_irradiance_params(
                day_of_year,
                altitude_deg,
                beam_tau,
                diffuse_tau,
            );
            ghi.push(ghi_val);
            dni.push(dni_val);
            dhi.push(dhi_val);
        }

        (ghi, dni, dhi)
    } else {
        (vec![0.0; n], vec![0.0; n], vec![0.0; n])
    };

    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    {
        // Invariant: if GHI[t] > 0, then DNI[t] ≥ 0, DHI[t] ≥ 0, and
        // GHI ≈ DNI × cos(zenith) + DHI within tolerance.
        for hour in 0..n {
            if ghi_w_m2[hour] > 0.0 {
                let altitude_deg =
                    solar_altitude_spencer(meta.latitude, meta.longitude, hour as u32);
                let zenith_deg = 90.0 - altitude_deg;
                let ghi_check = dni_w_m2[hour] * zenith_deg.to_radians().cos() + dhi_w_m2[hour];
                assert!(
                    dni_w_m2[hour] >= 0.0,
                    "step {hour}: DNI ({}) is negative while GHI > 0",
                    dni_w_m2[hour]
                );
                assert!(
                    dhi_w_m2[hour] >= 0.0,
                    "step {hour}: DHI ({}) is negative while GHI > 0",
                    dhi_w_m2[hour]
                );
                assert!(
                    (ghi_w_m2[hour] - ghi_check).abs() < 1e-6,
                    "step {hour}: GHI ({}) ≠ DNI·cos(zenith) + DHI ({})",
                    ghi_w_m2[hour],
                    ghi_check,
                );
            }
        }
        // Warn if all 8760 GHI values are zero: the clear-sky model was not integrated.
        let all_zero = ghi_w_m2.iter().all(|&v| v < f64::EPSILON);
        assert!(
            !all_zero || !config.weather.clear_sky_solar,
            "all 8760 GHI values are zero — clear-sky solar model not integrated"
        );
    }

    #[cfg(feature = "observe")]
    {
        let mut daily_ghi_sum = 0.0_f64;
        let mut daily_count = 0_u32;
        let mut peak_dni = 0.0_f64;
        let mut ghi_daily_means: Vec<f64> = Vec::with_capacity(365);
        for hour in 0..n {
            daily_ghi_sum += ghi_w_m2[hour];
            daily_count += 1;
            peak_dni = peak_dni.max(dni_w_m2[hour]);
            if daily_count == 24 {
                ghi_daily_means.push(daily_ghi_sum / 24.0);
                daily_ghi_sum = 0.0;
                daily_count = 0;
            }
        }
        let ghi_mean_daily = ghi_daily_means.iter().sum::<f64>() / ghi_daily_means.len() as f64;
        tracing::info!(
            weather.solar.ghi_mean_daily = ghi_mean_daily,
            weather.solar.peak_dni = peak_dni,
            n_hours = n,
            "synthetic clear-sky solar telemetry"
        );
    }

    // Diurnal temperature model.
    //
    // T(hour) = T_mean + A_effective × sin(2π × (hour_of_day − 6 − t_lag) / 24)
    //
    // The sin wave peaks at hour_of_day = 12 + t_lag (solar noon + thermal lag),
    // which for the default t_lag = 2.5 h gives a peak at 14:30 local time.
    // This matches the observed 2–3 h lag between peak solar irradiance and
    // peak air temperature (ASHRAE HoF 2021 Ch.14 §4 Table 14.6).
    //
    // Seasonal amplitude envelope:
    //   A_effective = A_base × [1 + B × cos(2π × (doy − 172) / 365)]
    // where doy 172 = June 21 (summer solstice) and B controls seasonal modulation.
    // With B > 0, summer diurnal amplitude exceeds winter amplitude.
    // The cos peaks at the solstice (1 + B) and troughs at the winter
    // solstice (1 − B). ASHRAE HoF 2021 Ch.14 §4.
    //
    // Ticket T-0189 specified sin(2π × (hour − t_lag − 12) / 24) which would
    // place the peak at hour 18 + t_lag (~20:30), inconsistent with the
    // invariant that the peak should fall in hours 13–17. The constant 12 was
    // corrected to 6 so the peak occurs at solar noon + lag.
    let diurnal_amp = config.weather.diurnal_amplitude_c;
    let thermal_lag_h = config.weather.thermal_lag_h;
    let seasonal_mod = config.weather.seasonal_modulation;
    let dry_bulb_c: Vec<f64> = if diurnal_amp > 0.0 {
        (0..n)
            .map(|hour| {
                let day_of_year = (hour / 24) as u32 + 1; // 1-based day-of-year
                let hour_of_day = (hour % 24) as f64;
                // Seasonal amplitude envelope: peak at summer solstice (doy 172).
                // cos(TAU * (doy − 172) / 365) = 1 at doy 172, −1 at doy 355.
                let season_factor = 1.0
                    + seasonal_mod
                        * (std::f64::consts::TAU * (day_of_year as f64 - 172.0) / 365.0).cos();
                let a_effective = diurnal_amp * season_factor;
                outdoor_temp_c
                    + a_effective
                        * (std::f64::consts::TAU * (hour_of_day - 6.0 - thermal_lag_h) / 24.0).sin()
            })
            .collect()
    } else {
        vec![outdoor_temp_c; n]
    };

    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    {
        // Invariant: if diurnal amplitude > 0, the output must have non-zero
        // variance (the diurnal model produced actual variation).
        if diurnal_amp > 0.0 {
            let min = dry_bulb_c.iter().copied().fold(f64::INFINITY, f64::min);
            let max = dry_bulb_c.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let range = max - min;
            assert!(
                range > 0.01,
                "diurnal_amplitude_c ({diurnal_amp}) > 0 but dry_bulb_c range ({range:.6}) ≈ 0 — \
                 diurnal model produced degenerate flat output"
            );
            // Invariant: the daily peak temperature should occur during
            // afternoon hours (13–17 local time at timezone offset 0), not
            // at midnight or dawn.
            //
            // Check day 180 (June 29): summer, peak should be well into
            // afternoon. Day index 179 (0-based, hour 4296..4319).
            let day_start = 179 * 24;
            let day_end = day_start + 24;
            if day_end <= n {
                let mut peak_hour = 0;
                let mut peak_val = f64::NEG_INFINITY;
                for (h, &val) in dry_bulb_c.iter().enumerate().take(day_end).skip(day_start) {
                    if val > peak_val {
                        peak_val = val;
                        peak_hour = h % 24;
                    }
                }
                assert!(
                    (13..=17).contains(&peak_hour),
                    "day 180 (June 29) peak dry-bulb hour ({peak_hour}) should be in 13–17 \
                     (afternoon local time); diurnal_amplitude_c = {diurnal_amp}, \
                     thermal_lag_h = {thermal_lag_h}",
                );
            }
        }
    }

    #[cfg(feature = "observe")]
    {
        let mut daily_ranges: Vec<f64> = Vec::with_capacity(365);
        let mut daily_peak_hours: Vec<u32> = Vec::with_capacity(365);
        for day in 0..365 {
            let day_start = day * 24;
            let day_end = (day_start + 24).min(n);
            if day_end <= day_start {
                continue;
            }
            let day_slice = &dry_bulb_c[day_start..day_end];
            let min = day_slice.iter().copied().fold(f64::INFINITY, f64::min);
            let max = day_slice.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            daily_ranges.push(max - min);
            let peak_idx = day_slice
                .iter()
                .enumerate()
                .fold(0, |idx, (i, &v)| if v > day_slice[idx] { i } else { idx });
            daily_peak_hours.push(peak_idx as u32);
        }
        let mean_daily_range = daily_ranges.iter().sum::<f64>() / daily_ranges.len() as f64;
        let mode_peak_hour = {
            let mut counts: [u32; 24] = [0; 24];
            for &h in &daily_peak_hours {
                if (h as usize) < 24 {
                    counts[h as usize] += 1;
                }
            }
            counts
                .iter()
                .enumerate()
                .fold(0, |best, (i, &c)| if c > counts[best] { i } else { best })
        };
        tracing::info!(
            weather.dry_bulb.daily_range = mean_daily_range,
            weather.dry_bulb.hour_of_peak = mode_peak_hour,
            diurnal_amplitude_c = diurnal_amp,
            thermal_lag_h = thermal_lag_h,
            seasonal_modulation = seasonal_mod,
            "synthetic dry-bulb diurnal telemetry"
        );
    }

    // Ground temperature: temporal mean of the dry-bulb series.
    //
    // Recomputes ground_temp_c from the actual time-varying dry_bulb series
    // so the deep-ground approximation tracks the true annual mean when a
    // diurnal profile is active. The Kusuda-Achenbach zero-amplitude limit
    // T(z,t) → T̄_s applies to the actual series, not the config constant.
    // Cite: Kusuda, T. and Achenbach, P.R. (1965), ASHRAE Trans. 71(1):61-74.
    let temporal_mean_c = dry_bulb_c.iter().sum::<f64>() / dry_bulb_c.len() as f64;
    let ground_temp_c = config.weather.ground_temp_c.unwrap_or(temporal_mean_c);

    Ok(WeatherTimeSeries {
        meta,
        design_conditions: None,
        dry_bulb_c,
        dew_point_c: vec![dew_point_c; n],
        rel_humidity_pct: vec![config.weather.rel_humidity_pct; n],
        pressure_kpa: vec![config.weather.pressure_kpa; n],
        ghi_w_m2,
        dni_w_m2,
        dhi_w_m2,
        wind_speed_m_s: vec![0.0; n],
        wind_dir_deg: vec![0.0; n],
        opaque_sky_cover: vec![0.0; n],
        horizontal_infrared_w_m2: vec![horizontal_ir_w_m2; n],
        sky_temp_c: vec![sky_temp_c; n],
        ground_temp_c: vec![ground_temp_c; n],
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

    let (column_names, columns, column_index, column_aggregations) =
        if config.schedule.occupants_present {
            (
                vec!["occupancy".to_string()],
                vec![vec![config.schedule.occupancy; total_steps]],
                HashMap::from([("occupancy".to_string(), 0usize)]),
                vec![ColumnAggregation::Mean],
            )
        } else {
            (vec![], vec![], HashMap::new(), vec![])
        };
    Ok(ScheduleTimeSeries {
        timestamps,
        column_names,
        columns,
        column_index,
        source_step_secs: step_secs,
        column_aggregations,
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

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // T-0188: Clear-sky solar irradiance in synthetic weather
    // -------------------------------------------------------------------------

    /// Verify that build_synthetic_weather with default config produces GHI > 0
    /// during daylight hours at a mid-latitude location (Denver 39.76° N) in July.
    #[test]
    fn synthetic_weather_produces_non_zero_ghi() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 3600
duration_s = 86400

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "None"
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let weather = build_synthetic_weather(&config, Path::new(".")).expect("weather");
        // July 21 is day_of_year 202 (31+28+31+30+31+30+21).
        // Hours (202-1)*24 = 4824 through 4824+23 represent July 21.
        // Denver at -104.86° longitude: solar noon UTC ≈ hour 19 of each day,
        // so peak DNI should be around hour 4824 + 19 = 4843.
        let july_21_start = (202 - 1) * 24;
        let mut peak_ghi = 0.0_f64;
        let mut peak_hour = 0;
        for hour in july_21_start..july_21_start + 24 {
            if weather.ghi_w_m2[hour] > peak_ghi {
                peak_ghi = weather.ghi_w_m2[hour];
                peak_hour = hour;
            }
        }
        let hour_of_day = peak_hour % 24;
        assert!(
            peak_ghi > 500.0,
            "peak GHI {peak_ghi} at hour_of_day {hour_of_day} (Denver lat 39.76° N on Jul 21) should exceed 500 W/m²"
        );
        // Solar noon UTC at Denver longitude (-104.86°) should be near hour 19
        assert!(
            (17..=21).contains(&hour_of_day),
            "peak hour_of_day {hour_of_day} should be near 19 UTC for Denver longitude"
        );
    }

    /// Verify that GHI values from build_synthetic_weather match calling
    /// clear_sky_irradiance directly with the same inputs.
    #[test]
    fn synthetic_clear_sky_ghi_matches_solar_model() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 3600
duration_s = 86400

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "None"
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let weather = build_synthetic_weather(&config, Path::new(".")).expect("weather");
        let latitude = weather.meta.latitude;
        let longitude = weather.meta.longitude;

        // Check four hours spread across the year: noon of day 1, day 2, day 180,
        // and day 364 (all within the 8760-hour series).
        for hour in [12, 24 + 12, 179 * 24 + 12, 363 * 24 + 12] {
            let day_of_year = (hour / 24) as u32 + 1;
            let altitude_deg = solar_altitude_spencer(latitude, longitude, hour as u32);
            let (dni_expected, dhi_expected, ghi_expected) =
                hares_physics::solar::clear_sky_irradiance(day_of_year, altitude_deg);
            let ghi_actual = weather.ghi_w_m2[hour];
            let dni_actual = weather.dni_w_m2[hour];
            let dhi_actual = weather.dhi_w_m2[hour];
            assert!(
                (ghi_actual - ghi_expected).abs() < 1e-9,
                "hour {hour} (doy {day_of_year}, alt {altitude_deg:.2}°): \
                 ghi={ghi_actual} expected {ghi_expected}"
            );
            assert!(
                (dni_actual - dni_expected).abs() < 1e-9,
                "hour {hour}: dni={dni_actual} expected {dni_expected}"
            );
            assert!(
                (dhi_actual - dhi_expected).abs() < 1e-9,
                "hour {hour}: dhi={dhi_actual} expected {dhi_expected}"
            );
        }
    }

    /// Verify GHI = 0 during nighttime hours (hours where solar altitude ≤ 0).
    #[test]
    fn synthetic_solar_is_zero_at_night() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 3600
duration_s = 86400

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "None"
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let weather = build_synthetic_weather(&config, Path::new(".")).expect("weather");
        let latitude = weather.meta.latitude;
        let longitude = weather.meta.longitude;

        for hour in 0..8760 {
            let altitude_deg = solar_altitude_spencer(latitude, longitude, hour as u32);
            if altitude_deg <= 0.0 {
                assert!(
                    weather.ghi_w_m2[hour] < 0.01,
                    "hour {hour}: GHI={} should be zero when solar altitude={altitude_deg:.2}° ≤ 0",
                    weather.ghi_w_m2[hour]
                );
                assert!(
                    weather.dni_w_m2[hour] < 0.01,
                    "hour {hour}: DNI={} should be zero at night",
                    weather.dni_w_m2[hour]
                );
                assert!(
                    weather.dhi_w_m2[hour] < 0.01,
                    "hour {hour}: DHI={} should be zero at night",
                    weather.dhi_w_m2[hour]
                );
            }
        }
    }

    /// Verify morning/afternoon symmetry of clear-sky irradiance about solar noon.
    #[test]
    fn synthetic_solar_symmetric_about_solar_noon() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-06-21T00:00:00Z"
time_res_s = 3600
duration_s = 86400

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "None"
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let weather = build_synthetic_weather(&config, Path::new(".")).expect("weather");
        let latitude = weather.meta.latitude;
        let longitude = weather.meta.longitude;

        // Find the hour nearest to solar noon (when solar altitude peaks).
        let mut max_alt = -1.0_f64;
        let mut max_alt_hour = 0;
        for hour in 0..24 {
            let alt = solar_altitude_spencer(latitude, longitude, hour as u32);
            if alt > max_alt {
                max_alt = alt;
                max_alt_hour = hour as i32;
            }
        }
        // Check symmetry around solar noon for as many offset pairs as
        // fall within the 24-hour window. The EOT varies over the day,
        // so solar altitude is not perfectly symmetric about the UTC hour
        // with peak altitude — a 15 W/m² tolerance on GHI accommodates this.
        for offset in 1..=6 {
            let before = max_alt_hour - offset;
            let after = max_alt_hour + offset;
            if before < 0 || after > 23 {
                continue;
            }
            let before_idx = before as usize;
            let after_idx = after as usize;
            let ghi_diff = (weather.ghi_w_m2[before_idx] - weather.ghi_w_m2[after_idx]).abs();
            assert!(
                ghi_diff < 15.0,
                "asymmetry at offset {offset} (before hour {before_idx}, after hour {after_idx}): \
                 GHI difference {ghi_diff:.2} W/m² exceeds 15.0 W/m² tolerance",
            );
        }
    }

    /// Verify that disabling clear_sky_solar produces all-zero irradiance
    /// (backward-compatibility with pre-T-0188 behaviour).
    #[test]
    fn synthetic_clear_sky_solar_disabled_produces_zero_irradiance() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-07-21T00:00:00Z"
time_res_s = 3600
duration_s = 86400

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "None"

[weather]
clear_sky_solar = false
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let weather = build_synthetic_weather(&config, Path::new(".")).expect("weather");
        for hour in 0..8760 {
            assert!(
                weather.ghi_w_m2[hour] < 0.01,
                "hour {hour}: GHI={} should be zero when clear_sky_solar disabled",
                weather.ghi_w_m2[hour]
            );
            assert!(
                weather.dni_w_m2[hour] < 0.01,
                "hour {hour}: DNI={} should be zero when clear_sky_solar disabled",
                weather.dni_w_m2[hour]
            );
            assert!(
                weather.dhi_w_m2[hour] < 0.01,
                "hour {hour}: DHI={} should be zero when clear_sky_solar disabled",
                weather.dhi_w_m2[hour]
            );
        }
    }

    /// Integration: BESTEST 600-like config without EPW path — verify thermal
    /// balance includes non-zero solar gains through surfaces and windows.
    #[test]
    fn bestest600_no_epw_produces_solar_gains() {
        use crate::dwelling::Dwelling;
        use std::fs;

        let toml_path = {
            let mut path = std::env::temp_dir();
            path.push(format!(
                "hares-t0188-solar-{}.toml",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            path
        };
        // Minimum BESTEST-600-like config with no EPW path (synthetic weather),
        // start time at solar noon UTC (~19:00) for Denver longitude (-104.86°).
        let content = r#"building_id = 600
[simulation]
start_time = "2024-07-21T19:00:00Z"
time_res_s = 3600
duration_s = 3600
[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 129.6
mass_multiplier = 1.0
[materials]
wall_r_value_m2_k_w = 2.0
[hvac]
equipment_name = "None"
[setpoints]
heating_c = 20.0
cooling_c = 27.0
[schedule]
occupancy = 0.0
occupants_present = false
[output]
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
write_output = false
master_seed = 0
"#;

        fs::write(&toml_path, content).expect("write temp TOML");

        let mut dwelling = Dwelling::from_toml_config_with_write_output(&toml_path, Some(false))
            .expect("build dwelling from synthetic BESTEST-600 config");

        dwelling
            .run_timestep(false)
            .expect("run timestep at solar noon");
        let env = dwelling.latest_env();

        let ghi = env.weather.ghi_w_m2;
        let dni = env.weather.dni_w_m2;
        let dhi = env.weather.dhi_w_m2;

        let _ = fs::remove_file(&toml_path);

        // At Denver lat 39.76° N on July 21, 19:00 UTC ≈ solar noon → GHI >> 0.
        assert!(
            ghi > 500.0,
            "GHI {ghi:.1} W/m² at Denver solar noon on Jul 21 should exceed 500 W/m²"
        );
        assert!(
            dni > 100.0,
            "DNI {dni:.1} W/m² at Denver solar noon should be substantial"
        );
        assert!(
            dhi > 50.0,
            "DHI {dhi:.1} W/m² should be non-zero for clear-sky mid-latitude summer"
        );

        // Solar altitude should be positive (sun is above horizon).
        assert!(
            env.weather.solar_altitude_deg > 0.0,
            "solar altitude {}° should be above horizon at noon",
            env.weather.solar_altitude_deg
        );

        // Surface irradiance should be allocated to opaque and glazed surfaces.
        let total_irradiance: f64 = env
            .weather
            .solar_irradiance
            .iter()
            .map(|s| s.direct_w_m2 + s.diffuse_w_m2 + s.reflected_w_m2)
            .sum();
        assert!(
            total_irradiance > 0.0,
            "total surface irradiance should be non-zero with non-zero GHI"
        );
    }

    #[test]
    fn initialization_duration_s_parsed_from_toml() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 60
duration_s = 3600
initialization_duration_s = 86400

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "None"
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        assert_eq!(config.simulation.initialization_duration_s, Some(86400));
    }

    #[test]
    fn initialization_duration_s_defaults_to_none() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 60
duration_s = 3600

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "None"
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        assert_eq!(config.simulation.initialization_duration_s, None);
    }

    #[test]
    fn initialization_duration_s_negative_rejected() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 60
duration_s = 3600
initialization_duration_s = -1

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "None"
"#;
        let result = toml::from_str::<SyntheticTomlConfig>(toml);
        assert!(
            result.is_err(),
            "negative initialization_duration_s must be rejected at parse time"
        );
    }

    /// B3 fix: synthetic weather sky_temp_c must be computed from physics,
    /// NOT set equal to outdoor_temp_c (which eliminates all LWR cooling).
    #[test]
    fn synthetic_sky_temp_is_below_outdoor_temp() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 60
duration_s = 3600

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "None"

[weather]
outdoor_temp_c = 10.0
dew_point_c = 5.0
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let weather = build_synthetic_weather(&config, Path::new(".")).expect("weather");
        // Clear sky at 10 °C / 5 °C dew point should produce sky_temp well
        // below outdoor_temp. The clear sky is typically 10–30 °C colder.
        assert!(
            weather.sky_temp_c[0] < config.weather.outdoor_temp_c - 5.0,
            "sky_temp_c ({}) must be at least 5 °C below outdoor_temp_c ({}) for clear sky",
            weather.sky_temp_c[0],
            config.weather.outdoor_temp_c,
        );
    }

    /// B3 fix: horizontal_infrared_w_m2 must be physically consistent with
    /// clear sky (sky_cover = 0), NOT hardcoded to 300 W/m².
    #[test]
    fn synthetic_horizontal_ir_matches_clear_sky_emissivity() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 60
duration_s = 3600

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "None"

[weather]
outdoor_temp_c = 0.0
dew_point_c = -5.0
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let weather = build_synthetic_weather(&config, Path::new(".")).expect("weather");
        let ir = weather.horizontal_infrared_w_m2[0];
        // At 0 °C / -5 °C dew point (cold clear sky, Denver winter):
        // Berdahl-Martin ε ≈ 0.732, IR ≈ 0.732 × σ × 273.15⁴ ≈ 183 W/m².
        // Must be well below the old hardcoded 300 W/m².
        assert!(
            (150.0..=250.0).contains(&ir),
            "horizontal IR {ir} out of expected range [150, 250] W/m² for clear sky at 0 °C",
        );
        assert_ne!(
            ir, 300.0,
            "horizontal IR must not equal the old hardcoded 300 W/m²"
        );
    }

    /// B3 fix: sky_temp_c and horizontal_infrared_w_m2 must be consistent
    /// with each other via the Stefan-Boltzmann relation.
    #[test]
    fn synthetic_sky_temp_and_ir_are_physically_consistent() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 60
duration_s = 3600

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "None"

[weather]
outdoor_temp_c = 10.0
dew_point_c = 5.0
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let weather = build_synthetic_weather(&config, Path::new(".")).expect("weather");
        let ir = weather.horizontal_infrared_w_m2[0];
        let sky_temp_c = weather.sky_temp_c[0];
        // Stefan-Boltzmann inversion: T_sky_K = (IR / σ)^0.25
        let sky_temp_k = (ir / hares_io::STEFAN_BOLTZMANN).powf(0.25);
        let sky_temp_c_check = sky_temp_k - hares_io::KELVIN_OFFSET_C;
        assert!(
            (sky_temp_c - sky_temp_c_check).abs() < 0.01,
            "sky_temp_c ({sky_temp_c}) inconsistent with IR ({ir}): \
             expected {sky_temp_c_check} from Stefan-Boltzmann inversion",
        );
    }

    // -------------------------------------------------------------------------
    // Ground temperature in synthetic weather
    // -------------------------------------------------------------------------

    /// For a constant synthetic dry-bulb series the temporal mean equals the
    /// per-record value, so ground_temp_c should equal outdoor_temp_c. This
    /// confirms no regression for existing callers that rely on the default
    /// temporal-mean approximation.
    #[test]
    fn synthetic_ground_temp_equals_outdoor_temp_for_constant_profile() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 3600
duration_s = 86400

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "None"

[weather]
outdoor_temp_c = -20.0
dew_point_c = -25.0
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let weather = build_synthetic_weather(&config, Path::new(".")).expect("weather");
        // For a constant dry-bulb series, temporal mean == outdoor_temp_c.
        // ground_temp_c must equal that mean (= -20.0), not diverge from it.
        for (i, &gt) in weather.ground_temp_c.iter().enumerate() {
            assert!(
                (gt - (-20.0_f64)).abs() < 0.01,
                "step {i}: ground_temp_c ({gt}) should equal temporal mean (-20.0) \
                 for a constant-temperature profile",
            );
        }
    }

    /// When `weather.ground_temp_c = Some(8.0)` is set in config alongside
    /// `outdoor_temp_c = -20.0`, the explicit override takes precedence:
    /// deserialization yields `Some(8.0)`, and every weather record carries
    /// 8.0 regardless of the outdoor temperature.
    #[test]
    fn synthetic_ground_temp_override_takes_precedence_over_outdoor_temp() {
        let toml = r#"
building_id = 1
[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 3600
duration_s = 86400
[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0
[materials]
wall_r_value_m2_k_w = 2.0
[hvac]
equipment_name = "None"
[weather]
outdoor_temp_c = -20.0
dew_point_c = -25.0
ground_temp_c = 8.0
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        assert_eq!(config.weather.ground_temp_c, Some(8.0));
        let weather = build_synthetic_weather(&config, Path::new(".")).expect("weather");
        for (i, &gt) in weather.ground_temp_c.iter().enumerate() {
            assert!(
                (gt - 8.0_f64).abs() < 0.01,
                "step {i}: ground_temp_c ({gt}) should be 8.0"
            );
        }
    }

    // -------------------------------------------------------------------------
    // T-0189: Diurnal temperature profile in synthetic weather
    // -------------------------------------------------------------------------

    /// With `diurnal_amplitude_c = 10.0`, the dry-bulb series must not be
    /// constant: daily temperature should swing between a minimum and maximum.
    #[test]
    fn synthetic_diurnal_produces_variation() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 3600
duration_s = 86400

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "None"

[weather]
outdoor_temp_c = 10.0
dew_point_c = 5.0
diurnal_amplitude_c = 10.0
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let weather = build_synthetic_weather(&config, Path::new(".")).expect("weather");
        let min = weather
            .dry_bulb_c
            .iter()
            .copied()
            .fold(f64::INFINITY, f64::min);
        let max = weather
            .dry_bulb_c
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        let range = max - min;
        assert!(
            range > 5.0,
            "dry_bulb_c range ({range:.2}) °C with amplitude 10 °C should exceed 5 °C"
        );
        assert_ne!(
            min, max,
            "dry_bulb_c min ({min}) must differ from max ({max})"
        );
    }

    /// The daily peak dry-bulb temperature must occur during afternoon hours
    /// (13–17 local time), not at midnight or dawn.
    #[test]
    fn synthetic_diurnal_peak_in_afternoon() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 3600
duration_s = 86400

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "None"

[weather]
outdoor_temp_c = 10.0
dew_point_c = 5.0
diurnal_amplitude_c = 10.0
thermal_lag_h = 2.5
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let weather = build_synthetic_weather(&config, Path::new(".")).expect("weather");
        // Check first full day (hours 0..23).
        let mut peak_hour = 0;
        let mut peak_val = f64::NEG_INFINITY;
        for h in 0..24 {
            if weather.dry_bulb_c[h] > peak_val {
                peak_val = weather.dry_bulb_c[h];
                peak_hour = h;
            }
        }
        assert!(
            peak_hour > 12,
            "peak hour ({peak_hour}) must be after solar noon (12)"
        );
        assert!(
            peak_hour < 18,
            "peak hour ({peak_hour}) must be before sunset (~18)"
        );
        // Also verify the minimum occurs at dawn (~hour 2–5 local time).
        let mut trough_hour = 0;
        let mut trough_val = f64::INFINITY;
        for h in 0..24 {
            if weather.dry_bulb_c[h] < trough_val {
                trough_val = weather.dry_bulb_c[h];
                trough_hour = h;
            }
        }
        assert!(
            trough_hour < 12,
            "trough hour ({trough_hour}) should be before noon"
        );
    }

    /// With `diurnal_amplitude_c = 0.0`, the output must be constant
    /// (backward-compatible with pre-T-0189 behavior).
    #[test]
    fn synthetic_diurnal_zero_amplitude_is_constant() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 3600
duration_s = 86400

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "None"

[weather]
outdoor_temp_c = 10.0
dew_point_c = 5.0
diurnal_amplitude_c = 0.0
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let weather = build_synthetic_weather(&config, Path::new(".")).expect("weather");
        let first = weather.dry_bulb_c[0];
        for (i, &t) in weather.dry_bulb_c.iter().enumerate() {
            assert!(
                (t - first).abs() < 1e-9,
                "step {i}: dry_bulb_c ({t}) differs from first ({first}) with zero amplitude"
            );
        }
    }

    /// When seasonal modulation is enabled (`seasonal_modulation = 0.4`), the
    /// diurnal temperature range in summer (July) must exceed the range in
    /// winter (January).
    #[test]
    fn synthetic_diurnal_seasonal_envelope() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 3600
duration_s = 86400

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "None"

[weather]
outdoor_temp_c = 10.0
dew_point_c = 5.0
diurnal_amplitude_c = 10.0
thermal_lag_h = 2.5
seasonal_modulation = 0.4
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let weather = build_synthetic_weather(&config, Path::new(".")).expect("weather");

        fn daily_range_at_doy(weather: &WeatherTimeSeries, doy: u32) -> f64 {
            let start = (doy as usize - 1) * 24;
            let end = start + 24;
            let max = weather.dry_bulb_c[start..end]
                .iter()
                .copied()
                .fold(f64::NEG_INFINITY, f64::max);
            let min = weather.dry_bulb_c[start..end]
                .iter()
                .copied()
                .fold(f64::INFINITY, f64::min);
            max - min
        }

        let solstice_range = daily_range_at_doy(&weather, 172); // June 21
        let winter_range = daily_range_at_doy(&weather, 355); // Dec 21
        assert!(
            solstice_range > winter_range,
            "Summer solstice range ({solstice_range:.2}) must exceed winter solstice range ({winter_range:.2}) \
             with seasonal_modulation = 0.4"
        );

        // The seasonal envelope cos peaks at solstice: A_eff = A_base × (1 + B) = 14.0 °C,
        // so the full diurnal swing = 2 × 14.0 = 28.0 °C. Allow ±0.5 °C for floating-point
        // with the diurnal phase offset.
        assert!(
            (solstice_range - 28.0).abs() < 0.5,
            "Summer solstice range ({solstice_range:.2}) should be ~28.0 °C"
        );
        assert!(
            (winter_range - 12.0).abs() < 0.5,
            "Winter solstice range ({winter_range:.2}) should be ~12.0 °C (A_eff = 6.0 °C)"
        );

        // Cross-check: July (doy 202) still exceeds January (doy 15).
        let july_range = daily_range_at_doy(&weather, 202);
        let jan_range = daily_range_at_doy(&weather, 15);
        assert!(
            july_range > jan_range,
            "July daily range ({july_range:.2}) must exceed January daily range ({jan_range:.2}) \
             with seasonal_modulation = 0.4"
        );
    }
}
