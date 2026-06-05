//! Synthetic (BESTEST-style) dwelling construction from TOML config.

use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, Duration, FixedOffset};
use hares_io::{Building, ColumnAggregation, ScheduleTimeSeries, WeatherMeta, WeatherTimeSeries};
use hares_physics::solar::{EOT_C0, EOT_C1, EOT_C2, EOT_C3, EOT_C4};
use hares_types::HaresError;
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
    pub(crate) internal_gains_w: Option<f64>,
    #[serde(default)]
    pub(crate) internal_gains_constant: Option<bool>,
    #[serde(default)]
    pub(crate) internal_gains_sensible_fraction: Option<f64>,
    #[serde(default)]
    pub(crate) internal_gains_radiant_fraction: Option<f64>,
    /// Optional stochastic event-based load (CookingRange) for reproducibility
    /// testing. When enabled, a single CookingRange equipment is injected into
    /// the building. The event probability is controlled by
    /// `event_probability_constant` — set to a value in (0, 1) to trigger
    /// stochastic event starts via the dwelling's hierarchical RNG.
    ///
    /// When `event_window_schedule` is configured, the event window source
    /// switches from constant to a per-timestep schedule column, enabling
    /// time-of-day-dependent event windows without requiring a full HPXML
    /// fixture with CSV schedule columns.
    #[serde(default)]
    pub(crate) event_load: Option<SyntheticEventLoadConfig>,
}

/// Configuration for a stochastic event-based load (CookingRange) injected
/// into a synthetic dwelling for reproducibility and smoke testing.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SyntheticEventLoadConfig {
    /// Active power draw during an event [kW].
    #[serde(default = "default_event_active_power_kw")]
    pub(crate) active_power_kw: f64,
    /// Duration of each active event [s].
    #[serde(default = "default_event_active_duration_s")]
    pub(crate) active_duration_s: f64,
    /// Cooldown period between events [s].
    #[serde(default = "default_event_cooldown_duration_s")]
    pub(crate) cooldown_duration_s: f64,
    /// Probability of starting an event when the window is open, in (0, 1].
    /// Values in (0, 1) produce stochastic behaviour via the dwelling RNG;
    /// 1.0 always starts an event (deterministic but still consumes RNG).
    #[serde(default = "default_event_probability")]
    pub(crate) event_probability: f64,
    /// Sensible gain fraction (0–1).  Default matches the HPXML resolver's
    /// built-in value for an electric CookingRange.
    #[serde(default = "default_event_sensible_fraction")]
    pub(crate) sensible_gain_fraction: f64,
    /// Latent gain fraction (0–1).  Default matches the HPXML resolver's
    /// built-in value for an electric CookingRange.
    #[serde(default = "default_event_latent_fraction")]
    pub(crate) latent_gain_fraction: f64,
    /// Optional 24-element hourly schedule for event window openness [0–1].
    /// When present, the schedule is expanded to per-step values and the
    /// column index is emitted in the HPXML extension instead of
    /// `event_window_source = "constant"`. Each element is a binary window
    /// (0 = closed, 1 = open); fractional values interpolate.
    #[serde(default)]
    pub(crate) event_window_schedule: Option<Vec<f64>>,
    /// Optional 24-element hourly schedule for event start probability [0–1].
    /// When present alongside `event_window_schedule`, uses a separate column.
    /// When absent but `event_window_schedule` is present, defaults to the
    /// same column index as the window schedule.
    #[serde(default)]
    pub(crate) event_probability_schedule: Option<Vec<f64>>,
}

fn default_event_active_power_kw() -> f64 {
    1.5
}
fn default_event_active_duration_s() -> f64 {
    300.0
}
fn default_event_cooldown_duration_s() -> f64 {
    120.0
}
fn default_event_probability() -> f64 {
    0.5
}
fn default_event_sensible_fraction() -> f64 {
    0.72
}
fn default_event_latent_fraction() -> f64 {
    0.08
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
    #[allow(dead_code)]
    // Why: wall_area_m2 is parsed from 30+ existing TOML configs and test
    // fixtures for backward compatibility but is no longer read by
    // build_synthetic_building — T-0226 replaces the single-wall default
    // with geometry-derived wall areas from floor_area and zone_volume.
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
    #[serde(default)]
    pub(crate) retain_batches: bool,
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
            retain_batches: false,
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

pub(crate) fn build_synthetic_building(
    config: &SyntheticTomlConfig,
    event_window_schedule_col: Option<usize>,
    event_probability_schedule_col: Option<usize>,
) -> Result<Building> {
    use hares_io::hpxml::{Boundary, BoundaryType, MaterialLayer, Site, Window, Zone, ZoneType};
    use hares_physics::units as conv;

    // ── Range validation ────────────────────────────────────────────────
    // Validate material properties before constructing anything so users
    // get clear error messages referencing their TOML fields rather than
    // cryptic downstream errors (e.g. NonPositiveResistance in RC network).

    // wall_r_value_m2_k_w > 0.0 — reject 0, negative, NaN
    {
        let r_value = config.materials.wall_r_value_m2_k_w;
        if r_value.is_nan() || r_value <= 0.0 {
            #[cfg(feature = "observe")]
            tracing::debug!(
                synthetic.validation.wall_r_value_failure = true,
                value = r_value,
                "wall_r_value_m2_k_w validation failed"
            );
            return Err(HaresError::Dwelling(format!(
                "wall_r_value_m2_k_w must be positive, got {r_value}"
            )));
        }
    }

    // Window validation: u_factor > 0.0, 0.0 <= SHGC <= 1.0
    if let Some(windows) = &config.windows {
        for wc in windows {
            if wc.u_factor_w_m2_k.is_nan() || wc.u_factor_w_m2_k <= 0.0 {
                #[cfg(feature = "observe")]
                tracing::debug!(
                    synthetic.validation.u_factor_failure = true,
                    window_id = %wc.id,
                    value = wc.u_factor_w_m2_k,
                    "u_factor_w_m2_k validation failed"
                );
                return Err(HaresError::Dwelling(format!(
                    "u_factor_w_m2_k must be positive for window '{}', got {}",
                    wc.id, wc.u_factor_w_m2_k
                )));
            }
            if !(0.0 <= wc.shgc && wc.shgc <= 1.0) {
                #[cfg(feature = "observe")]
                tracing::debug!(
                    synthetic.validation.shgc_failure = true,
                    window_id = %wc.id,
                    value = wc.shgc,
                    "SHGC validation failed"
                );
                return Err(HaresError::Dwelling(format!(
                    "SHGC must be in [0.0, 1.0] for window '{}', got {}",
                    wc.id, wc.shgc
                )));
            }
        }
    }

    // Boundary material layer validation
    if let Some(boundary_configs) = &config.boundaries {
        for bc in boundary_configs {
            for ml in &bc.material_layers {
                // conductivity_w_m_k > 0.0 for layers with positive thickness
                if ml.thickness_m > 0.0
                    && (ml.conductivity_w_m_k.is_nan() || ml.conductivity_w_m_k <= 0.0)
                {
                    #[cfg(feature = "observe")]
                    tracing::debug!(
                        synthetic.validation.conductivity_failure = true,
                        boundary_id = %bc.id,
                        thickness_m = ml.thickness_m,
                        value = ml.conductivity_w_m_k,
                        "conductivity_w_m_k validation failed"
                    );
                    return Err(HaresError::Dwelling(format!(
                        "conductivity_w_m_k must be positive for boundary '{}' with thickness {:.6} m, got {}",
                        bc.id, ml.thickness_m, ml.conductivity_w_m_k
                    )));
                }
                // density_kg_m3 >= 0.0 — reject negative
                if ml.density_kg_m3.is_nan() || ml.density_kg_m3 < 0.0 {
                    #[cfg(feature = "observe")]
                    tracing::debug!(
                        synthetic.validation.density_failure = true,
                        boundary_id = %bc.id,
                        value = ml.density_kg_m3,
                        "density_kg_m3 validation failed"
                    );
                    return Err(HaresError::Dwelling(format!(
                        "density_kg_m3 must be non-negative for boundary '{}', got {}",
                        bc.id, ml.density_kg_m3
                    )));
                }
                // specific_heat_j_kg_k >= 0.0 — reject negative
                if ml.specific_heat_j_kg_k.is_nan() || ml.specific_heat_j_kg_k < 0.0 {
                    #[cfg(feature = "observe")]
                    tracing::debug!(
                        synthetic.validation.specific_heat_failure = true,
                        boundary_id = %bc.id,
                        value = ml.specific_heat_j_kg_k,
                        "specific_heat_j_kg_k validation failed"
                    );
                    return Err(HaresError::Dwelling(format!(
                        "specific_heat_j_kg_k must be non-negative for boundary '{}', got {}",
                        bc.id, ml.specific_heat_j_kg_k
                    )));
                }
                // Warn on zero density or specific_heat for layers with positive thickness
                if ml.thickness_m > 0.0 && ml.density_kg_m3 == 0.0 {
                    tracing::warn!(
                        boundary_id = %bc.id,
                        thickness_m = ml.thickness_m,
                        "density_kg_m3 is zero for layer with positive thickness — zero thermal capacitance"
                    );
                }
                if ml.thickness_m > 0.0 && ml.specific_heat_j_kg_k == 0.0 {
                    tracing::warn!(
                        boundary_id = %bc.id,
                        thickness_m = ml.thickness_m,
                        "specific_heat_j_kg_k is zero for layer with positive thickness — zero thermal capacitance"
                    );
                }
            }
        }
    }

    // ── End range validation ────────────────────────────────────────────

    // Heating capacity: when explicitly set via TOML, emit as explicit
    // HPXML <HeatingCapacity>; when absent (None), let the dwelling builder's
    // autosizing path compute capacity from building UA and design temperatures
    // per ACCA Manual S-2017 §4.
    let heating_capacity_btu_h = config
        .hvac
        .heating_capacity_kbtu_h
        .map(|kbtu| kbtu * 1000.0);
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
        ];
        // Only emit explicit HeatingCapacity when the user provided one.
        // When absent, the HPXML resolver sets autosize_heating = true,
        // triggering ACCA Manual S-based autosizing from building UA and
        // design temperatures (autosize.rs).
        if let Some(btu_h) = heating_capacity_btu_h {
            heating_children.push(hares_io::hpxml::building::XmlNode {
                name: "HeatingCapacity".to_string(),
                attrs: HashMap::new(),
                text: btu_h.to_string(),
                children: Vec::new(),
            });
        }

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
        let mut cooling_children = vec![
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
        ];
        // Only emit explicit CoolingCapacity when the user provided one.
        // When absent, the HPXML resolver sets autosize_cooling = true,
        // allowing cooling to be sized independently from heating.
        if let Some(btu_h) = heating_capacity_btu_h {
            cooling_children.push(hares_io::hpxml::building::XmlNode {
                name: "CoolingCapacity".to_string(),
                attrs: HashMap::new(),
                text: btu_h.to_string(),
                children: Vec::new(),
            });
        }
        hvac_children.push(hares_io::hpxml::building::XmlNode {
            name: "CoolingSystem".to_string(),
            attrs: HashMap::new(),
            text: String::new(),
            children: cooling_children,
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

    // Inject a stochastic CookingRange event-based load when configured.
    // The HPXML resolver processes the `<Appliances><CookingRange>` node and
    // creates an EventBasedLoad equipment with the parameters below.
    // `event_probability_constant` controls stochastic behaviour via the
    // dwelling's hierarchical RNG stream: values in (0, 1) produce
    // seed-dependent event starts for reproducibility testing.
    if let Some(el) = &config.event_load {
        let mut extension_children: Vec<hares_io::hpxml::building::XmlNode> = vec![
            hares_io::hpxml::building::XmlNode {
                name: "active_power_kw".to_string(),
                attrs: HashMap::new(),
                text: el.active_power_kw.to_string(),
                children: Vec::new(),
            },
            hares_io::hpxml::building::XmlNode {
                name: "active_duration_s".to_string(),
                attrs: HashMap::new(),
                text: el.active_duration_s.to_string(),
                children: Vec::new(),
            },
            hares_io::hpxml::building::XmlNode {
                name: "cooldown_duration_s".to_string(),
                attrs: HashMap::new(),
                text: el.cooldown_duration_s.to_string(),
                children: Vec::new(),
            },
        ];

        // Emit column-based schedule sources when schedule columns were
        // appended to the ScheduleTimeSeries by build_synthetic_schedule.
        // The column index must match the position in the schedule's columns
        // vector exactly — parse_event_schedule_sources reads it as a usize.
        if let Some(col_idx) = event_window_schedule_col {
            extension_children.push(hares_io::hpxml::building::XmlNode {
                name: "event_window_schedule_col".to_string(),
                attrs: HashMap::new(),
                text: col_idx.to_string(),
                children: Vec::new(),
            });
        } else {
            extension_children.push(hares_io::hpxml::building::XmlNode {
                name: "event_window_source".to_string(),
                attrs: HashMap::new(),
                text: "constant".to_string(),
                children: Vec::new(),
            });
        }

        if let Some(col_idx) = event_probability_schedule_col {
            extension_children.push(hares_io::hpxml::building::XmlNode {
                name: "event_probability_source".to_string(),
                attrs: HashMap::new(),
                text: "column".to_string(),
                children: Vec::new(),
            });
            extension_children.push(hares_io::hpxml::building::XmlNode {
                name: "event_probability_schedule_col".to_string(),
                attrs: HashMap::new(),
                text: col_idx.to_string(),
                children: Vec::new(),
            });
        } else {
            extension_children.push(hares_io::hpxml::building::XmlNode {
                name: "event_probability_source".to_string(),
                attrs: HashMap::new(),
                text: "constant".to_string(),
                children: Vec::new(),
            });
            extension_children.push(hares_io::hpxml::building::XmlNode {
                name: "event_probability_constant".to_string(),
                attrs: HashMap::new(),
                text: el.event_probability.to_string(),
                children: Vec::new(),
            });
        }

        extension_children.extend([
            hares_io::hpxml::building::XmlNode {
                name: "sensible_gain_fraction".to_string(),
                attrs: HashMap::new(),
                text: el.sensible_gain_fraction.to_string(),
                children: Vec::new(),
            },
            hares_io::hpxml::building::XmlNode {
                name: "latent_gain_fraction".to_string(),
                attrs: HashMap::new(),
                text: el.latent_gain_fraction.to_string(),
                children: Vec::new(),
            },
        ]);
        // Add default schedule fractions so the HPXML resolver doesn't
        // compute annual energy from the bedroom-count formula (which
        // would override the explicitly-set per-cycle power/duration).
        // A constant 24-element schedule fraction vector gives uniform
        // distribution across the day.
        let flat_24 = std::iter::repeat_n("0.04167", 24)
            .collect::<Vec<_>>()
            .join(", ");
        extension_children.push(hares_io::hpxml::building::XmlNode {
            name: "WeekdayScheduleFractions".to_string(),
            attrs: HashMap::new(),
            text: flat_24.clone(),
            children: Vec::new(),
        });
        extension_children.push(hares_io::hpxml::building::XmlNode {
            name: "WeekendScheduleFractions".to_string(),
            attrs: HashMap::new(),
            text: flat_24,
            children: Vec::new(),
        });

        details_children.push(hares_io::hpxml::building::XmlNode {
            name: "Appliances".to_string(),
            attrs: HashMap::new(),
            text: String::new(),
            children: vec![hares_io::hpxml::building::XmlNode {
                name: "CookingRange".to_string(),
                attrs: HashMap::new(),
                text: String::new(),
                children: vec![
                    hares_io::hpxml::building::XmlNode {
                        name: "FuelType".to_string(),
                        attrs: HashMap::new(),
                        text: "electricity".to_string(),
                        children: Vec::new(),
                    },
                    hares_io::hpxml::building::XmlNode {
                        name: "RatedAnnualkWh".to_string(),
                        attrs: HashMap::new(),
                        text: "0".to_string(),
                        children: Vec::new(),
                    },
                    hares_io::hpxml::building::XmlNode {
                        name: "extension".to_string(),
                        attrs: HashMap::new(),
                        text: String::new(),
                        children: extension_children,
                    },
                ],
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
        // Auto-generate a closed six-face thermal envelope from geometry
        // when no [[boundaries]] are specified.  Each face uses the
        // configured wall R-value and connects the conditioned zone to
        // outdoor (walls + roof) or ground (floor).
        //
        // Footprint is square (aspect ratio 1.0).  Ceiling height is
        // inferred from zone volume / floor area.  This replaces the
        // pre-T-0226 single-wall default which produced a non-physical
        // open envelope with one vertical surface.
        let ceiling_height_m = if config.geometry.floor_area_m2 > 0.0 {
            config.geometry.zone_volume_m3 / config.geometry.floor_area_m2
        } else {
            // Fallback to a reasonable ceiling height when floor area
            // is zero (should not occur in normal configs).
            2.5
        };
        let side_length_m = config.geometry.floor_area_m2.sqrt();
        let wall_area_m2 = side_length_m * ceiling_height_m;
        let r_value = config.materials.wall_r_value_m2_k_w;

        // Concrete-like default material layer for boundaries without
        // explicit [[boundaries.material_layers]] entries.  Provides
        // physically plausible thermal capacitance so the RC network has
        // higher-order dynamics rather than first-order RC decay.
        //
        // Use a fixed reference thickness d = 0.15 m and set effective
        // conductivity k = d / R so the single layer produces the target
        // R-value (d/k = R).  Density and specific_heat match concrete
        // (ASHRAE HoF 2021 Ch. 33, Table 1) giving thermal capacitance
        // C = ρ·c_p·d·A ≈ 317 kJ/(K·m²)·A.
        //
        // A fixed thickness avoids the diurnal-criterion sub-layer
        // explosion that d = R·k (with real concrete k = 1.4) would
        // produce: 2.8 m / 0.068 m ≈ 42 sub-layers per boundary for a
        // typical R = 2.0 wall.  The per-layer capacitance is identical
        // regardless of k because only d, ρ, and c_p control C.
        const DEFAULT_LAYER_THICKNESS_M: f64 = 0.15;
        let safe_r_value = r_value.max(1e-6);
        let effective_concrete_k = DEFAULT_LAYER_THICKNESS_M / safe_r_value;
        let make_concrete_layer = |area_m2| MaterialLayer {
            thickness_m: DEFAULT_LAYER_THICKNESS_M,
            conductivity_w_m_k: effective_concrete_k,
            density_kg_m3: hares_physics::constants::CONCRETE_DENSITY_KG_M3,
            specific_heat_j_kg_k: hares_physics::constants::CONCRETE_CP_J_KG_K,
            area_m2,
        };

        let make_wall = |id: &str, azimuth: f64| Boundary {
            id: id.to_string(),
            boundary_type: BoundaryType::Wall,
            area_m2: wall_area_m2,
            azimuth_deg: Some(azimuth),
            assembly_r_value_m2_k_w: Some(r_value),
            r_value_layers_m2_k_w: vec![r_value],
            interior_zone: Some(ZoneType::Conditioned),
            exterior_zone: Some(ZoneType::Outdoor),
            material_layers: vec![make_concrete_layer(wall_area_m2)],
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
        };

        let make_horizontal =
            |id: &str, btype: BoundaryType, ext_zone: ZoneType, tilt: f64| Boundary {
                id: id.to_string(),
                boundary_type: btype,
                area_m2: config.geometry.floor_area_m2,
                azimuth_deg: None,
                assembly_r_value_m2_k_w: Some(r_value),
                r_value_layers_m2_k_w: vec![r_value],
                interior_zone: Some(ZoneType::Conditioned),
                exterior_zone: Some(ext_zone),
                material_layers: vec![make_concrete_layer(config.geometry.floor_area_m2)],
                construction_type: None,
                finish_type: None,
                insulation_details: None,
                has_radiant_barrier: false,
                solar_absorptance: None,
                emittance: None,
                tilt_deg: Some(tilt),
                framing_factor: None,
                lut_boundary_name: None,
                floor_or_ceiling: None,
                perimeter_m: None,
                perimeter_insulation_r_m2_k_w: None,
                foundation_depth_m: None,
            };

        vec![
            make_wall("wall-north", 0.0),
            make_wall("wall-east", 90.0),
            make_wall("wall-south", 180.0),
            make_wall("wall-west", 270.0),
            make_horizontal("roof", BoundaryType::Roof, ZoneType::Outdoor, 0.0),
            make_horizontal("floor", BoundaryType::Floor, ZoneType::Ground, 180.0),
        ]
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

    // Deduct window areas from host wall areas so the wall's opaque
    // conduction path uses only the actual opaque area.  Without this,
    // the wall and window areas are double-counted in the envelope,
    // inflating overall UA and producing biased heating/cooling loads.
    let mut wall_original_area: std::collections::HashMap<String, f64> =
        std::collections::HashMap::new();
    for win in &windows {
        if let Some(ref wall_id) = win.attached_to_wall_id {
            if let Some(wall) = boundaries.iter_mut().find(|b| b.id == *wall_id) {
                wall_original_area
                    .entry(wall_id.clone())
                    .or_insert(wall.area_m2);
                wall.area_m2 -= win.area_m2;
                if wall.area_m2 < 0.0 {
                    tracing::warn!(
                        window_id = %win.id,
                        host_wall_id = %wall_id,
                        window_area_m2 = win.area_m2,
                        host_wall_original_area_m2 = *wall_original_area.get(wall_id).unwrap_or(&0.0),
                        "Window area exceeds host wall area; clamping wall opaque area to 0.0"
                    );
                    wall.area_m2 = 0.0;
                }
            }
        }
    }

    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    {
        for boundary in &boundaries {
            assert!(
                boundary.area_m2 >= 0.0,
                "boundary {} has negative area_m2 = {}",
                boundary.id,
                boundary.area_m2
            );
        }
        for boundary in &boundaries {
            if boundary.boundary_type == BoundaryType::Window {
                continue;
            }
            let attached_window_area: f64 = windows
                .iter()
                .filter(|w| w.attached_to_wall_id.as_deref() == Some(boundary.id.as_str()))
                .map(|w| w.area_m2)
                .sum();
            if attached_window_area > 0.0 {
                let reconstructed_original = boundary.area_m2 + attached_window_area;
                if let Some(&stored_original) = wall_original_area.get(&boundary.id) {
                    if boundary.area_m2 > 0.0 {
                        // Normal case: opaque area remaining, so reconstructed
                        // should equal stored original.
                        assert!(
                            (reconstructed_original - stored_original).abs() < 1e-9,
                            "boundary {}: reconstructed original {reconstructed_original} != stored original {stored_original}",
                            boundary.id
                        );
                    } else {
                        // Clamped case: reconstructed will be >= stored original
                        // because window area exceeded wall area.
                        assert!(
                            reconstructed_original >= stored_original - 1e-9,
                            "boundary {}: reconstructed original {reconstructed_original} < stored original {stored_original}",
                            boundary.id
                        );
                    }
                }
            }
        }
    }

    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    {
        // Verify the synthetic building has a closed thermal envelope.
        // A single-boundary envelope (the pre-T-0226 default) is an open,
        // non-physical geometry.  We require at minimum two of the three
        // structural face categories (Wall, Roof, Floor) when the zone
        // volume is non-zero.
        if config.geometry.zone_volume_m3 > 0.0 {
            let non_window: Vec<&Boundary> = boundaries
                .iter()
                .filter(|b| b.boundary_type != BoundaryType::Window)
                .collect();
            let has_wall = non_window
                .iter()
                .any(|b| b.boundary_type == BoundaryType::Wall);
            let has_roof = non_window
                .iter()
                .any(|b| b.boundary_type == BoundaryType::Roof);
            let has_floor = non_window
                .iter()
                .any(|b| b.boundary_type == BoundaryType::Floor);
            let face_categories = [has_wall, has_roof, has_floor]
                .iter()
                .filter(|&&x| x)
                .count();
            if face_categories < 2 {
                tracing::error!(
                    zone_volume_m3 = config.geometry.zone_volume_m3,
                    boundary_count = non_window.len(),
                    has_wall,
                    has_roof,
                    has_floor,
                    "Synthetic building envelope is incomplete: fewer than 2 of 3 required \
                     face categories (Wall, Roof, Floor) are present"
                );
            }
            if non_window.len() == 1 {
                tracing::error!(
                    zone_volume_m3 = config.geometry.zone_volume_m3,
                    single_boundary_id = %non_window[0].id,
                    single_boundary_type = ?non_window[0].boundary_type,
                    "Synthetic building has only one non-window boundary with non-zero \
                     zone volume; thermal envelope is open and non-physical"
                );
            }
        }
    }

    #[cfg(feature = "observe")]
    {
        for boundary in &boundaries {
            if boundary.boundary_type == BoundaryType::Window {
                continue;
            }
            let window_area: f64 = windows
                .iter()
                .filter(|w| w.attached_to_wall_id.as_deref() == Some(boundary.id.as_str()))
                .map(|w| w.area_m2)
                .sum();
            let original_area = boundary.area_m2 + window_area;
            if original_area > 0.0 {
                let wwr = window_area / original_area;
                tracing::debug!(
                    boundary_id = %boundary.id,
                    window_to_wall_ratio = wwr,
                    opaque_area_m2 = boundary.area_m2,
                    window_area_m2 = window_area,
                    "Per-boundary WWR diagnostic"
                );
            }
        }
    }

    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    {
        // Verify that explicit HVAC capacity is positive and finite relative
        // to floor area. The ratio is not clamped to a tight residential band
        // because synthetic test fixtures use deliberately extreme values
        // (e.g. 500 kBTU/h for stress testing). Autosized capacity
        // (hvac_capacity_w = None) is validated by the autosizer's internal
        // invariants instead.
        if let Some(capacity_w) = heating_capacity_btu_h.map(conv::power_btu_h_to_w)
            && capacity_w > 0.0
        {
            assert!(
                capacity_w.is_finite(),
                "synthetic building explicit hvac_capacity_w is NaN or infinite"
            );
            let w_per_m2 = capacity_w / floor_area;
            assert!(
                w_per_m2 > 0.0 && w_per_m2.is_finite(),
                "synthetic building with floor_area_m2 = {floor_area} has explicit \
                 hvac_capacity_w = {capacity_w:.0} W → {w_per_m2:.1} W/m², which is \
                 non-positive or non-finite"
            );
        }
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

    let building = Building {
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
        hvac_capacity_w: heating_capacity_btu_h.map(conv::power_btu_h_to_w),
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
    };

    // Post-construction debug assertions: verify ranges hold in the built object
    // to catch internal construction bugs.  These are redundant with the
    // pre-construction validation above but guard against regressions where
    // downstream code mangles the validated values.
    #[cfg(debug_assertions)]
    {
        for boundary in &building.boundaries {
            if boundary.boundary_type == BoundaryType::Window {
                continue;
            }
            if let Some(r_value) = boundary.assembly_r_value_m2_k_w {
                assert!(
                    r_value > 0.0,
                    "boundary {} assembly_r_value_m2_k_w must be positive after construction, got {r_value}",
                    boundary.id
                );
            }
            for layer in &boundary.material_layers {
                if layer.thickness_m > 0.0 {
                    assert!(
                        layer.conductivity_w_m_k > 0.0,
                        "boundary {} material layer conductivity_w_m_k must be positive, got {}",
                        boundary.id,
                        layer.conductivity_w_m_k
                    );
                }
                assert!(
                    layer.density_kg_m3 >= 0.0,
                    "boundary {} material layer density_kg_m3 must be non-negative, got {}",
                    boundary.id,
                    layer.density_kg_m3
                );
                assert!(
                    layer.specific_heat_j_kg_k >= 0.0,
                    "boundary {} material layer specific_heat_j_kg_k must be non-negative, got {}",
                    boundary.id,
                    layer.specific_heat_j_kg_k
                );
            }
        }
        for window in &building.windows {
            assert!(
                0.0 <= window.shgc.unwrap_or(0.0) && window.shgc.unwrap_or(0.0) <= 1.0,
                "window {} SHGC = {:?} out of [0, 1] range after construction",
                window.id,
                window.shgc
            );
            if let Some(u) = window.u_factor_w_m2_k {
                assert!(
                    u > 0.0,
                    "window {} U-factor must be positive after construction, got {u}",
                    window.id
                );
            }
        }
    }

    Ok(building)
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

/// Result of building a synthetic schedule, including optional column indices
/// for event-load schedule data that must be communicated to
/// `build_synthetic_building` so the HPXML extension XML emits matching
/// `ColumnRef` indices.
pub(crate) struct SyntheticScheduleResult {
    pub(crate) schedule: ScheduleTimeSeries,
    /// Column index for the event window schedule in the per-step columns, if
    /// `event_load.event_window_schedule` was provided.
    pub(crate) event_window_schedule_col: Option<usize>,
    /// Column index for the event probability schedule in the per-step columns,
    /// if `event_load.event_probability_schedule` was provided.
    pub(crate) event_probability_schedule_col: Option<usize>,
}

pub(crate) fn build_synthetic_schedule(
    config: &SyntheticTomlConfig,
) -> Result<SyntheticScheduleResult> {
    use chrono::TimeDelta;

    let step_secs = duration_to_u32_secs(Duration::seconds(config.simulation.time_res_s))?;
    let total_steps = (config.simulation.duration_s / config.simulation.time_res_s).max(1) as usize;
    let start = config.simulation.start_time;

    let mut timestamps = Vec::with_capacity(total_steps);
    for i in 0..total_steps {
        timestamps.push(start + TimeDelta::seconds((i as i64) * i64::from(step_secs)));
    }

    let mut column_names: Vec<String>;
    let mut columns: Vec<Vec<f64>>;
    let mut column_index: HashMap<String, usize>;
    let mut column_aggregations: Vec<ColumnAggregation>;

    if config.schedule.occupants_present {
        column_names = vec!["occupancy".to_string()];
        columns = vec![vec![config.schedule.occupancy; total_steps]];
        column_index = HashMap::from([("occupancy".to_string(), 0usize)]);
        column_aggregations = vec![ColumnAggregation::Mean];
    } else {
        column_names = vec![];
        columns = vec![];
        column_index = HashMap::new();
        column_aggregations = vec![];
    }

    let event_window_schedule_col: Option<usize>;
    let event_probability_schedule_col: Option<usize>;

    if let Some(el) = &config.event_load {
        // Expand the 24-hour event window schedule to per-step values when
        // provided.  Each step's hour-of-day index selects the corresponding
        // schedule fraction.
        if let Some(ref window_sched) = el.event_window_schedule {
            let expanded: Vec<f64> = (0..total_steps)
                .map(|i| {
                    let hour_of_day = ((i as u64 * u64::from(step_secs)) / 3600 % 24) as usize;
                    window_sched[hour_of_day]
                })
                .collect();
            let col_idx = columns.len();
            column_names.push("event_load_window".to_string());
            column_index.insert("event_load_window".to_string(), col_idx);
            columns.push(expanded);
            column_aggregations.push(ColumnAggregation::Mean);
            event_window_schedule_col = Some(col_idx);
        } else {
            event_window_schedule_col = None;
        }

        if let Some(ref prob_sched) = el.event_probability_schedule {
            let expanded: Vec<f64> = (0..total_steps)
                .map(|i| {
                    let hour_of_day = ((i as u64 * u64::from(step_secs)) / 3600 % 24) as usize;
                    prob_sched[hour_of_day]
                })
                .collect();
            let col_idx = columns.len();
            column_names.push("event_load_probability".to_string());
            column_index.insert("event_load_probability".to_string(), col_idx);
            columns.push(expanded);
            column_aggregations.push(ColumnAggregation::Mean);
            event_probability_schedule_col = Some(col_idx);
        } else {
            event_probability_schedule_col = None;
        }
    } else {
        event_window_schedule_col = None;
        event_probability_schedule_col = None;
    }

    Ok(SyntheticScheduleResult {
        schedule: ScheduleTimeSeries {
            timestamps,
            column_names,
            columns,
            column_index,
            source_step_secs: step_secs,
            column_aggregations,
        },
        event_window_schedule_col,
        event_probability_schedule_col,
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
    use hares_io::hpxml::building::XmlNode;
    use hares_io::hpxml::{BoundaryType, ZoneType};
    use std::collections::HashSet;

    fn find_xml_child<'a>(children: &'a [XmlNode], name: &str) -> Option<&'a XmlNode> {
        children.iter().find(|n| n.name == name)
    }

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

    // -------------------------------------------------------------------------
    // T-0225: Window area deducted from host wall in synthetic builder
    // -------------------------------------------------------------------------

    /// Window with `attached_to_wall_id` must have its area subtracted from the
    /// host wall's `area_m2` so the wall's opaque conduction path uses only the
    /// actual opaque area.
    #[test]
    fn window_area_deducted_from_host_wall() {
        let toml = r#"
building_id = 1
[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 3600
duration_s = 3600
[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0
[materials]
wall_r_value_m2_k_w = 2.0
[hvac]
equipment_name = "None"

[[boundaries]]
id = "wall-1"
boundary_type = "Wall"
area_m2 = 10.0
interior_zone = "Conditioned"
exterior_zone = "Outdoor"
[[boundaries.material_layers]]
thickness_m = 0.01
conductivity_w_m_k = 0.1
density_kg_m3 = 500.0
specific_heat_j_kg_k = 900.0

[[windows]]
id = "win-1"
area_m2 = 2.0
u_factor_w_m2_k = 3.0
shgc = 0.7
attached_to_wall_id = "wall-1"
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let building =
            build_synthetic_building(&config, None, None).expect("build synthetic building");

        let wall = building
            .boundaries
            .iter()
            .find(|b| b.id == "wall-1")
            .expect("host wall exists");
        assert!(
            (wall.area_m2 - 8.0).abs() < 1e-9,
            "Wall area after window deduction should be 8.0 m², got {}",
            wall.area_m2
        );

        // The window boundary should still exist with its own area.
        let win_boundary = building
            .boundaries
            .iter()
            .find(|b| b.id == "win-1")
            .expect("window boundary exists");
        assert!(
            (win_boundary.area_m2 - 2.0).abs() < 1e-9,
            "Window boundary area should be 2.0 m², got {}",
            win_boundary.area_m2
        );

        // Total boundary area: 8.0 (wall) + 2.0 (window) = 10.0.
        let total: f64 = building.boundaries.iter().map(|b| b.area_m2).sum();
        assert!(
            (total - 10.0).abs() < 1e-9,
            "Total area should be 10.0 m², got {}",
            total
        );
    }

    /// BESTEST 600 south wall (9.6 m²) has two 6.0 m² windows (12 m² total).
    /// The south wall opaque area must be clamped to 0.0 when window area
    /// exceeds wall area, rather than going negative.
    #[test]
    fn bestest600_south_wall_area_clamped_to_zero() {
        let toml = r#"
building_id = 600
[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 3600
duration_s = 3600
[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 129.6
[materials]
wall_r_value_m2_k_w = 2.0
[hvac]
equipment_name = "None"

[[boundaries]]
id = "south-wall"
boundary_type = "Wall"
area_m2 = 9.6
azimuth_deg = 180.0
tilt_deg = 90.0
interior_zone = "Conditioned"
exterior_zone = "Outdoor"
[[boundaries.material_layers]]
thickness_m = 0.01
conductivity_w_m_k = 0.1
density_kg_m3 = 500.0
specific_heat_j_kg_k = 900.0

[[boundaries]]
id = "north-wall"
boundary_type = "Wall"
area_m2 = 21.6
azimuth_deg = 0.0
tilt_deg = 90.0
interior_zone = "Conditioned"
exterior_zone = "Outdoor"
[[boundaries.material_layers]]
thickness_m = 0.01
conductivity_w_m_k = 0.1
density_kg_m3 = 500.0
specific_heat_j_kg_k = 900.0

[[boundaries]]
id = "east-wall"
boundary_type = "Wall"
area_m2 = 16.2
azimuth_deg = 90.0
tilt_deg = 90.0
interior_zone = "Conditioned"
exterior_zone = "Outdoor"
[[boundaries.material_layers]]
thickness_m = 0.01
conductivity_w_m_k = 0.1
density_kg_m3 = 500.0
specific_heat_j_kg_k = 900.0

[[boundaries]]
id = "west-wall"
boundary_type = "Wall"
area_m2 = 16.2
azimuth_deg = 270.0
tilt_deg = 90.0
interior_zone = "Conditioned"
exterior_zone = "Outdoor"
[[boundaries.material_layers]]
thickness_m = 0.01
conductivity_w_m_k = 0.1
density_kg_m3 = 500.0
specific_heat_j_kg_k = 900.0

[[boundaries]]
id = "roof"
boundary_type = "Roof"
area_m2 = 48.0
tilt_deg = 0.0
interior_zone = "Conditioned"
exterior_zone = "Outdoor"
[[boundaries.material_layers]]
thickness_m = 0.01
conductivity_w_m_k = 0.1
density_kg_m3 = 500.0
specific_heat_j_kg_k = 900.0

[[boundaries]]
id = "floor"
boundary_type = "Floor"
area_m2 = 48.0
tilt_deg = 180.0
interior_zone = "Conditioned"
exterior_zone = "Ground"
[[boundaries.material_layers]]
thickness_m = 0.01
conductivity_w_m_k = 0.1
density_kg_m3 = 500.0
specific_heat_j_kg_k = 900.0

[[windows]]
id = "south-window-1"
area_m2 = 6.0
azimuth_deg = 180.0
u_factor_w_m2_k = 3.0
shgc = 0.789
attached_to_wall_id = "south-wall"

[[windows]]
id = "south-window-2"
area_m2 = 6.0
azimuth_deg = 180.0
u_factor_w_m2_k = 3.0
shgc = 0.789
attached_to_wall_id = "south-wall"
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let building =
            build_synthetic_building(&config, None, None).expect("build synthetic building");

        let south_wall = building
            .boundaries
            .iter()
            .find(|b| b.id == "south-wall")
            .expect("south wall exists");
        assert!(
            south_wall.area_m2 == 0.0,
            "South wall area with two 6.0 m² windows should be clamped to 0.0, got {}",
            south_wall.area_m2
        );

        // Verify that total envelope area is no longer overestimated.
        // Walls/roof/floor after deduction:
        //   south: 9.6 - 12.0 = -2.4 → 0.0
        //   north: 21.6
        //   east:  16.2
        //   west:  16.2
        //   roof:  48.0
        //   floor: 48.0
        // Windows as boundaries:
        //   south-window-1: 6.0
        //   south-window-2: 6.0
        // Total = 0.0+21.6+16.2+16.2+48.0+48.0+6.0+6.0 = 162.0
        let total: f64 = building.boundaries.iter().map(|b| b.area_m2).sum();
        let expected_total = 162.0;
        assert!(
            (total - expected_total).abs() < 1e-9,
            "Total boundary area should be {expected_total} m² (no double-counting), got {total}"
        );
    }

    // -------------------------------------------------------------------------
    // T-0226: Auto-generated six-face envelope
    // -------------------------------------------------------------------------

    /// When no `[[boundaries]]` are configured, the builder auto-generates a
    /// closed six-face envelope: four vertical walls, a horizontal roof, and a
    /// horizontal floor.  Wall area is derived from floor area and ceiling
    /// height with a square footprint (aspect ratio 1.0).
    #[test]
    fn default_no_boundaries_auto_generates_six_face_envelope() {
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
wall_r_value_m2_k_w = 2.5
[hvac]
equipment_name = "None"
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let building =
            build_synthetic_building(&config, None, None).expect("build synthetic building");

        // Exclude windows from the face count (none configured here).
        let non_window: Vec<_> = building
            .boundaries
            .iter()
            .filter(|b| b.boundary_type != BoundaryType::Window)
            .collect();

        // Must have exactly 6 faces: 4 walls + roof + floor.
        assert_eq!(
            non_window.len(),
            6,
            "Expected 6 envelope faces, got {}: {:?}",
            non_window.len(),
            non_window.iter().map(|b| &b.id).collect::<Vec<_>>()
        );

        let walls: Vec<_> = non_window
            .iter()
            .filter(|b| b.boundary_type == BoundaryType::Wall)
            .collect();
        assert_eq!(walls.len(), 4, "Expected 4 walls, got {}", walls.len());

        let roof = non_window
            .iter()
            .find(|b| b.boundary_type == BoundaryType::Roof)
            .expect("roof must exist");
        let floor = non_window
            .iter()
            .find(|b| b.boundary_type == BoundaryType::Floor)
            .expect("floor must exist");

        // Ceiling height = 120 / 48 = 2.5 m.
        // Side length = sqrt(48) ≈ 6.9282 m.
        // Each wall area = 6.9282 * 2.5 ≈ 17.3205 m².
        let expected_wall_area = (48.0_f64).sqrt() * (120.0 / 48.0);
        for wall in &walls {
            assert!(
                (wall.area_m2 - expected_wall_area).abs() < 1e-9,
                "Wall {} area {:.6} != expected {:.6}",
                wall.id,
                wall.area_m2,
                expected_wall_area
            );
        }

        // Roof and floor area = floor_area_m2 = 48.0 m².
        assert!(
            (roof.area_m2 - 48.0).abs() < 1e-9,
            "Roof area {:.6} != 48.0",
            roof.area_m2
        );
        assert!(
            (floor.area_m2 - 48.0).abs() < 1e-9,
            "Floor area {:.6} != 48.0",
            floor.area_m2
        );

        // All faces share the configured R-value.
        for face in &non_window {
            assert_eq!(
                face.assembly_r_value_m2_k_w,
                Some(config.materials.wall_r_value_m2_k_w),
                "Face {} has wrong R-value",
                face.id
            );
        }

        // Wall azimuths must be 0°, 90°, 180°, 270°.
        let mut azimuths: Vec<f64> = walls.iter().map(|w| w.azimuth_deg.unwrap()).collect();
        azimuths.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(
            azimuths,
            vec![0.0, 90.0, 180.0, 270.0],
            "Wall azimuths should be N/E/S/W"
        );

        // All walls are vertical, roof is horizontal facing up, floor is
        // horizontal facing down.
        for wall in &walls {
            assert_eq!(
                wall.tilt_deg,
                Some(90.0),
                "Wall {} tilt should be 90°",
                wall.id
            );
        }
        assert_eq!(roof.tilt_deg, Some(0.0), "Roof tilt should be 0°");
        assert_eq!(floor.tilt_deg, Some(180.0), "Floor tilt should be 180°");

        // Wall exterior = Outdoor, floor exterior = Ground, roof exterior = Outdoor.
        assert_eq!(roof.exterior_zone, Some(ZoneType::Outdoor));
        assert_eq!(floor.exterior_zone, Some(ZoneType::Ground));
        for wall in &walls {
            assert_eq!(wall.exterior_zone, Some(ZoneType::Outdoor));
        }
    }

    /// When explicit `[[boundaries]]` are configured, the auto-generation
    /// path is skipped and the explicit boundaries are used unchanged.
    #[test]
    fn explicit_boundaries_not_affected_by_auto_generation() {
        let toml = r#"
building_id = 1
[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 3600
duration_s = 86400
[geometry]
floor_area_m2 = 100.0
zone_volume_m3 = 300.0
[materials]
wall_r_value_m2_k_w = 3.0
[hvac]
equipment_name = "None"

[[boundaries]]
id = "custom-wall"
boundary_type = "Wall"
area_m2 = 25.0
interior_zone = "Conditioned"
exterior_zone = "Outdoor"
[[boundaries.material_layers]]
thickness_m = 0.01
conductivity_w_m_k = 0.1
density_kg_m3 = 500.0
specific_heat_j_kg_k = 900.0

[[boundaries]]
id = "custom-roof"
boundary_type = "Roof"
area_m2 = 100.0
interior_zone = "Conditioned"
exterior_zone = "Outdoor"
[[boundaries.material_layers]]
thickness_m = 0.01
conductivity_w_m_k = 0.1
density_kg_m3 = 500.0
specific_heat_j_kg_k = 900.0
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let building =
            build_synthetic_building(&config, None, None).expect("build synthetic building");

        // The builder must use the explicit boundaries, not auto-generate.
        assert_eq!(
            building.boundaries.len(),
            2,
            "Should have exactly 2 explicit boundaries"
        );
        assert!(building.boundaries.iter().any(|b| b.id == "custom-wall"));
        assert!(building.boundaries.iter().any(|b| b.id == "custom-roof"));

        let wall = building
            .boundaries
            .iter()
            .find(|b| b.id == "custom-wall")
            .expect("custom-wall must exist");
        assert!(
            (wall.area_m2 - 25.0).abs() < 1e-9,
            "Custom wall area should be 25.0 m², got {}",
            wall.area_m2
        );
    }

    /// Benchmark-style TOML (matching `benches/common.rs::synthetic_toml_case`)
    /// produces a six-face envelope with total wall area consistent with
    /// floor area and volume-derived ceiling height.  The pre-T-0226 single-wall
    /// path produced a non-physical envelope; this test guards against
    /// regression of that behavior.
    #[test]
    fn benchmark_style_default_produces_physically_closed_envelope() {
        // Mirror the TOML from benches/common.rs::synthetic_toml_case exactly.
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 60
duration_s = 86400

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0
wall_area_m2 = 145.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "Furnace"
fuel = "electricity"
heating_capacity_kbtu_h = 25.0

[weather]
outdoor_temp_c = 8.0
dew_point_c = 4.0
rel_humidity_pct = 55.0
pressure_kpa = 101.325

[schedule]
occupancy = 1.0

[output]
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
master_seed = 0
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let building =
            build_synthetic_building(&config, None, None).expect("build synthetic building");

        let non_window: Vec<_> = building
            .boundaries
            .iter()
            .filter(|b| b.boundary_type != BoundaryType::Window)
            .collect();

        // Six-face envelope expected.
        assert_eq!(
            non_window.len(),
            6,
            "Benchmark-style default should produce 6 envelope faces"
        );

        // Total wall area should be 4 * sqrt(48) * (120/48) ≈ 69.28 m²,
        // not the 145 m² `wall_area_m2` from the old single-wall path.
        let ceiling_height_m = 120.0 / 48.0;
        let side_m = 48.0_f64.sqrt();
        let expected_wall_area_total = 4.0 * side_m * ceiling_height_m;
        let actual_wall_area_total: f64 = non_window
            .iter()
            .filter(|b| b.boundary_type == BoundaryType::Wall)
            .map(|b| b.area_m2)
            .sum();
        assert!(
            (actual_wall_area_total - expected_wall_area_total).abs() < 1e-9,
            "Total wall area {actual_wall_area_total} != expected {expected_wall_area_total}"
        );

        // The old single-wall area (145 m²) must NOT match — this proves
        // the envelope is no longer a single inflated wall.
        assert!(
            (actual_wall_area_total - 145.0).abs() > 1.0,
            "Total wall area {actual_wall_area_total} should not match the old 145 m² default"
        );

        // Roof + floor = 2 * floor_area = 96 m².
        let roof_area = non_window
            .iter()
            .find(|b| b.boundary_type == BoundaryType::Roof)
            .map(|b| b.area_m2)
            .unwrap();
        let floor_area_envelope = non_window
            .iter()
            .find(|b| b.boundary_type == BoundaryType::Floor)
            .map(|b| b.area_m2)
            .unwrap();
        assert!(
            (roof_area - 48.0).abs() < 1e-9,
            "Roof area {roof_area} != 48.0"
        );
        assert!(
            (floor_area_envelope - 48.0).abs() < 1e-9,
            "Floor area {floor_area_envelope} != 48.0"
        );
    }

    // ── Default boundary thermal mass invariants ───────────────────

    /// Default synthetic building (no `[[boundaries]]` in TOML) produces
    /// boundaries whose assembled RC network has capacitance-bearing nodes
    /// from the default concrete material layer.
    #[test]
    fn default_synthetic_boundaries_have_capacitance_nodes() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 60
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
        let building =
            build_synthetic_building(&config, None, None).expect("build synthetic building");

        // Verify each non-window boundary carries exactly one concrete
        // material layer with non-zero density and specific_heat.
        let non_window: Vec<_> = building
            .boundaries
            .iter()
            .filter(|b| b.boundary_type != BoundaryType::Window)
            .collect();

        assert!(
            !non_window.is_empty(),
            "Default synthetic building must have non-window boundaries"
        );

        for b in &non_window {
            assert_eq!(
                b.material_layers.len(),
                1,
                "boundary '{}' should have exactly one default material layer",
                b.id
            );
            let layer = &b.material_layers[0];
            assert!(
                layer.density_kg_m3 > 0.0,
                "boundary '{}' material layer has zero density",
                b.id
            );
            assert!(
                layer.specific_heat_j_kg_k > 0.0,
                "boundary '{}' material layer has zero specific_heat",
                b.id
            );
            assert!(
                layer.conductivity_w_m_k > 0.0,
                "boundary '{}' material layer has zero conductivity",
                b.id
            );
            assert!(
                layer.thickness_m > 0.0,
                "boundary '{}' material layer has zero thickness",
                b.id
            );
            // The layer area must be non-zero for capacitance contribution.
            assert!(
                layer.area_m2 > 0.0,
                "boundary '{}' material layer has zero area",
                b.id
            );
        }

        // Assemble the RC network and verify capacitance nodes exist
        // in the diagnostics.
        let store = hares_io::DefaultsStore::empty();
        let boundary_inputs = crate::dwelling::conversions::building_to_boundary_inputs(
            &building, 1, &store, 2.0, 10.0, 10.0,
        )
        .expect("building_to_boundary_inputs");

        let zone_inputs = vec![hares_envelope::ZoneInput {
            floor_area_m2: Some(48.0),
            volume_m3: None,
            mass_multiplier: hares_envelope::boundary_rc::INTERIOR_MASS_MULTIPLIER,
        }];
        let zone_caps = hares_envelope::derive_zone_capacitances(
            &zone_inputs,
            hares_physics::constants::SEA_LEVEL_PRESSURE_PA,
        )
        .expect("derive_zone_capacitances");

        let (_rc, diag) = hares_envelope::assemble_building_rc(
            &boundary_inputs,
            1,
            &zone_caps,
            hares_envelope::InteriorLwrMethod::StarMesh,
        )
        .expect("assemble_building_rc");

        // Every non-window boundary must have at least one RC node in
        // the assembled network. Windows are excluded because their
        // glass-layer path legitimately produces n_rc_nodes = 0; film
        // resistance alone does not create a dedicated capacitance node.
        let non_window_indices: HashSet<usize> = building
            .boundaries
            .iter()
            .enumerate()
            .filter(|(_, b)| b.boundary_type != BoundaryType::Window)
            .map(|(i, _)| i)
            .collect();

        for bd in &diag.boundaries {
            if !non_window_indices.contains(&bd.boundary_idx) {
                continue;
            }
            assert!(
                bd.n_rc_nodes > 0 || bd.capacitance_j_k > 0.0,
                "boundary idx {} has no capacitance-bearing RC nodes",
                bd.boundary_idx
            );
        }
    }

    /// Regression: the default building's state-space model has more states
    /// than the number of thermal zones, confirming higher-order thermal
    /// dynamics from the default concrete material layers.  A pure first-order
    /// RC system would have exactly `n_zones` states.
    #[test]
    fn default_synthetic_building_has_higher_order_eigenvalue_spectrum() {
        use nalgebra::DMatrix;

        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 60
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
        let building =
            build_synthetic_building(&config, None, None).expect("build synthetic building");

        let n_non_window = building
            .boundaries
            .iter()
            .filter(|b| b.boundary_type != BoundaryType::Window)
            .count();

        let store = hares_io::DefaultsStore::empty();
        let boundary_inputs = crate::dwelling::conversions::building_to_boundary_inputs(
            &building, 1, &store, 2.0, 10.0, 10.0,
        )
        .expect("building_to_boundary_inputs");

        let zone_inputs = vec![hares_envelope::ZoneInput {
            floor_area_m2: Some(48.0),
            volume_m3: None,
            mass_multiplier: hares_envelope::boundary_rc::INTERIOR_MASS_MULTIPLIER,
        }];
        let zone_caps = hares_envelope::derive_zone_capacitances(
            &zone_inputs,
            hares_physics::constants::SEA_LEVEL_PRESSURE_PA,
        )
        .expect("derive_zone_capacitances");

        let (rc, _diag) = hares_envelope::assemble_building_rc(
            &boundary_inputs,
            1,
            &zone_caps,
            hares_envelope::InteriorLwrMethod::StarMesh,
        )
        .expect("assemble_building_rc");

        let n_zones = 1;
        let n_states = rc.a_c.nrows();

        assert!(
            n_states > n_zones,
            "Default synthetic building with {} non-window boundaries has {} states \
             (expected > {} zone nodes).  A first-order RC system would have \
             exactly {} state(s); {} states confirm higher-order dynamics.",
            n_non_window,
            n_states,
            n_zones,
            n_zones,
            n_states
        );

        // Also check that the continuous system is stable (all eigenvalues
        // have negative real parts — energy must decay).
        use hares_envelope::state_space::discretize_zoh;
        let b_c = DMatrix::<f64>::zeros(n_states, rc.n_ext.max(1));
        let (a_d, _b_d) = discretize_zoh(&rc.a_c, &b_c, 60.0).expect("ZOH");
        let discrete_eigs = a_d.complex_eigenvalues();
        let stable = discrete_eigs.iter().all(|eig| eig.norm() < 1.0);
        assert!(
            stable,
            "Discrete eigenvalue magnitudes must all be < 1.0 (stable)."
        );
    }

    /// Regression: the benchmark-style synthetic building produces a
    /// numerically stable state-space model with all discrete eigenvalue
    /// magnitudes well within the unit circle.  Uses the actual eigenvalue
    /// decomposition rather than the conservative Gershgorin bound.
    #[test]
    fn benchmark_synthetic_building_is_numerically_stable() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 60
duration_s = 86400

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "Furnace"
fuel = "electricity"
heating_capacity_kbtu_h = 25.0

[weather]
outdoor_temp_c = 8.0
dew_point_c = 4.0
rel_humidity_pct = 55.0
pressure_kpa = 101.325

[schedule]
occupancy = 1.0

[output]
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
master_seed = 0
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let building =
            build_synthetic_building(&config, None, None).expect("build synthetic building");

        let store = hares_io::DefaultsStore::empty();
        let boundary_inputs = crate::dwelling::conversions::building_to_boundary_inputs(
            &building, 1, &store, 2.0, 10.0, 10.0,
        )
        .expect("building_to_boundary_inputs");

        let zone_inputs = vec![hares_envelope::ZoneInput {
            floor_area_m2: Some(48.0),
            volume_m3: None,
            mass_multiplier: hares_envelope::boundary_rc::INTERIOR_MASS_MULTIPLIER,
        }];
        let zone_caps = hares_envelope::derive_zone_capacitances(
            &zone_inputs,
            hares_physics::constants::SEA_LEVEL_PRESSURE_PA,
        )
        .expect("derive_zone_capacitances");

        let (rc, _diag) = hares_envelope::assemble_building_rc(
            &boundary_inputs,
            1,
            &zone_caps,
            hares_envelope::InteriorLwrMethod::StarMesh,
        )
        .expect("assemble_building_rc");

        let a_c = &rc.a_c;
        let n_states = a_c.nrows();
        // Empty B_c (no external driving inputs affect eigenvalue stability).
        let b_c = nalgebra::DMatrix::<f64>::zeros(n_states, rc.n_ext.max(1));

        // Discretize: A_d = expm(A_c · dt).
        use hares_envelope::state_space::discretize_zoh;
        let (a_d, _b_d) = discretize_zoh(a_c, &b_c, 60.0).expect("ZOH discretization");

        let discrete_eigs = a_d.complex_eigenvalues();
        let max_mag = discrete_eigs
            .iter()
            .fold(0.0f64, |m, eig| m.max(eig.norm()));

        assert!(
            max_mag < 1.0 - 1e-9,
            "Benchmark synthetic building has marginally unstable discrete eigenvalue: \
             max magnitude = {:.6} (expected < 1.0).",
            max_mag
        );
    }

    // ── T-0228: HVAC capacity autosizing for synthetic buildings ───────────

    /// When `heating_capacity_kbtu_h` is NOT set in the TOML config,
    /// `hvac_capacity_w` must be `None`, signalling to the dwelling builder
    /// that capacity should be autosized from building UA and design
    /// temperatures per ACCA Manual S-2017 §4.
    #[test]
    fn no_explicit_capacity_produces_none_hvac_capacity_w() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 60
duration_s = 86400

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "Furnace"
fuel = "electricity"

[setpoints]
cooling_c = 25.0

[weather]
outdoor_temp_c = 10.0
dew_point_c = 5.0

[schedule]
occupancy = 1.0

[output]
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
master_seed = 0
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let building =
            build_synthetic_building(&config, None, None).expect("build synthetic building");

        assert!(
            building.hvac_capacity_w.is_none(),
            "hvac_capacity_w must be None when heating_capacity_kbtu_h is not set, \
             got {:?}",
            building.hvac_capacity_w
        );
    }

    /// When `heating_capacity_kbtu_h` IS set in the TOML config,
    /// `hvac_capacity_w` must carry the explicit value in watts.
    #[test]
    fn explicit_capacity_preserves_hvac_capacity_w() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 60
duration_s = 86400

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "Furnace"
fuel = "electricity"
heating_capacity_kbtu_h = 25.0

[weather]
outdoor_temp_c = 10.0
dew_point_c = 5.0

[schedule]
occupancy = 1.0

[output]
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
master_seed = 0
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let building =
            build_synthetic_building(&config, None, None).expect("build synthetic building");

        let expected_w = hares_physics::units::power_btu_h_to_w(25.0 * 1000.0);
        let actual_w = building
            .hvac_capacity_w
            .expect("hvac_capacity_w must be Some when capacity is set");
        assert!(
            (actual_w - expected_w).abs() < 0.01,
            "hvac_capacity_w = {actual_w} W, expected ~{expected_w:.1} W for 25 kBTU/h"
        );
    }

    /// When `heating_capacity_kbtu_h` is not set, the HPXML
    /// `<HeatingCapacity>` element must be absent so the resolver flags
    /// `autosize_heating = true`.
    #[test]
    fn no_explicit_capacity_omits_heating_capacity_from_xml() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 60
duration_s = 86400

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "Furnace"
fuel = "electricity"

[setpoints]
cooling_c = 25.0

[weather]
outdoor_temp_c = 10.0
dew_point_c = 5.0

[schedule]
occupancy = 1.0

[output]
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
master_seed = 0
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let building =
            build_synthetic_building(&config, None, None).expect("build synthetic building");

        let hvac = find_xml_child(&building.details_xml.children, "Systems")
            .and_then(|s| find_xml_child(&s.children, "HVAC"))
            .expect("HVAC section must exist");
        let heating_system = find_xml_child(&hvac.children, "HeatingSystem")
            .expect("HeatingSystem must exist when equipment_name is 'Furnace'");
        let heating_capacity = find_xml_child(&heating_system.children, "HeatingCapacity");
        assert!(
            heating_capacity.is_none(),
            "HeatingCapacity must NOT be present when heating_capacity_kbtu_h is not set"
        );
    }

    /// When `heating_capacity_kbtu_h` IS set, the HPXML
    /// `<HeatingCapacity>` element must be present with the correct value.
    #[test]
    fn explicit_capacity_includes_heating_capacity_in_xml() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 60
duration_s = 86400

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "Furnace"
fuel = "electricity"
heating_capacity_kbtu_h = 25.0

[setpoints]
cooling_c = 25.0

[weather]
outdoor_temp_c = 10.0
dew_point_c = 5.0

[schedule]
occupancy = 1.0

[output]
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
master_seed = 0
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let building =
            build_synthetic_building(&config, None, None).expect("build synthetic building");

        let hvac = find_xml_child(&building.details_xml.children, "Systems")
            .and_then(|s| find_xml_child(&s.children, "HVAC"))
            .expect("HVAC section must exist");
        let heating_system = find_xml_child(&hvac.children, "HeatingSystem")
            .expect("HeatingSystem must exist when equipment_name is 'Furnace'");
        let heating_capacity = find_xml_child(&heating_system.children, "HeatingCapacity")
            .expect("HeatingCapacity must be present when heating_capacity_kbtu_h is set");
        let text: f64 = heating_capacity.text.parse().expect("must parse as f64");
        let expected_btu_h = 25_000.0;
        assert!(
            (text - expected_btu_h).abs() < 1.0,
            "HeatingCapacity = {text} BTU/h, expected {expected_btu_h}"
        );
    }

    /// Cooling capacity must be independently autosized, not set to the
    /// heating capacity value. When `heating_capacity_kbtu_h` is not set,
    /// `<CoolingCapacity>` must be absent so the resolver flags
    /// `autosize_cooling = true`.
    #[test]
    fn no_explicit_capacity_omits_cooling_capacity_from_xml() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 60
duration_s = 86400

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "Furnace"
fuel = "electricity"

[setpoints]
cooling_c = 25.0

[weather]
outdoor_temp_c = 10.0
dew_point_c = 5.0

[schedule]
occupancy = 1.0

[output]
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
master_seed = 0
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let building =
            build_synthetic_building(&config, None, None).expect("build synthetic building");

        let hvac = find_xml_child(&building.details_xml.children, "Systems")
            .and_then(|s| find_xml_child(&s.children, "HVAC"))
            .expect("HVAC section must exist");
        // CoolingSystem is present because setpoints are configured
        // (has_cooling = true), but CoolingCapacity must be absent.
        let cooling_system = find_xml_child(&hvac.children, "CoolingSystem");
        assert!(
            cooling_system.is_some(),
            "CoolingSystem must exist when setpoints are configured"
        );
        let cooling_capacity =
            cooling_system.and_then(|cs| find_xml_child(&cs.children, "CoolingCapacity"));
        assert!(
            cooling_capacity.is_none(),
            "CoolingCapacity must NOT be present when heating_capacity_kbtu_h is not set"
        );
    }

    // ── T-0229: Material property range validation ──────────────────────

    /// Synthetic builder with `wall_r_value_m2_k_w = 0.0` returns a clear error
    /// referencing the TOML field.
    #[test]
    fn wall_r_value_zero_returned_as_error() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 3600
duration_s = 3600

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 0.0

[hvac]
equipment_name = "None"
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let result = build_synthetic_building(&config, None, None);
        assert!(result.is_err(), "should fail with zero wall_r_value");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("wall_r_value_m2_k_w"),
            "error message must reference the TOML field, got: {err}"
        );
        assert!(
            err.contains("positive"),
            "error message must explain the constraint, got: {err}"
        );
    }

    /// Synthetic builder with `wall_r_value_m2_k_w = -1.0` (negative) is
    /// also rejected.
    #[test]
    fn wall_r_value_negative_returned_as_error() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 3600
duration_s = 3600

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = -1.0

[hvac]
equipment_name = "None"
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let result = build_synthetic_building(&config, None, None);
        assert!(result.is_err(), "should fail with negative wall_r_value");
    }

    /// Synthetic builder with `shgc = 1.5` returns an error.
    #[test]
    fn shgc_above_one_returned_as_error() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 3600
duration_s = 3600

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "None"

[[windows]]
id = "win-1"
area_m2 = 2.0
u_factor_w_m2_k = 3.0
shgc = 1.5
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let result = build_synthetic_building(&config, None, None);
        assert!(result.is_err(), "should fail with shgc = 1.5");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("SHGC"),
            "error message must reference SHGC, got: {err}"
        );
    }

    /// Synthetic builder with `shgc = 1.0` succeeds (boundary value).
    #[test]
    fn shgc_at_one_succeeds() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 3600
duration_s = 3600

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "None"

[[windows]]
id = "win-1"
area_m2 = 2.0
u_factor_w_m2_k = 3.0
shgc = 1.0
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let result = build_synthetic_building(&config, None, None);
        assert!(
            result.is_ok(),
            "shgc = 1.0 should be accepted as a boundary value"
        );
    }

    /// Synthetic builder with `shgc = 0.0` succeeds (boundary value).
    #[test]
    fn shgc_at_zero_succeeds() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 3600
duration_s = 3600

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "None"

[[windows]]
id = "win-1"
area_m2 = 2.0
u_factor_w_m2_k = 3.0
shgc = 0.0
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let result = build_synthetic_building(&config, None, None);
        assert!(
            result.is_ok(),
            "shgc = 0.0 should be accepted as a boundary value"
        );
    }

    /// Synthetic builder with `conductivity_w_m_k = 0.0` and
    /// `thickness_m = 0.1` returns an error.
    #[test]
    fn zero_conductivity_with_positive_thickness_returned_as_error() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 3600
duration_s = 3600

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "None"

[[boundaries]]
id = "wall-1"
boundary_type = "Wall"
area_m2 = 10.0
interior_zone = "Conditioned"
exterior_zone = "Outdoor"
[[boundaries.material_layers]]
thickness_m = 0.1
conductivity_w_m_k = 0.0
density_kg_m3 = 500.0
specific_heat_j_kg_k = 900.0
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let result = build_synthetic_building(&config, None, None);
        assert!(
            result.is_err(),
            "should fail with conductivity = 0.0 and thickness = 0.1 m"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("conductivity_w_m_k"),
            "error message must reference conductivity field, got: {err}"
        );
        assert!(
            err.contains("positive"),
            "error message must explain the constraint, got: {err}"
        );
    }

    /// Zero-density layer with positive thickness succeeds (zero-capacitance
    /// is a valid steady-state choice) — but emits a warning.
    #[test]
    fn zero_density_with_positive_thickness_succeeds() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 3600
duration_s = 3600

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "None"

[[boundaries]]
id = "wall-1"
boundary_type = "Wall"
area_m2 = 10.0
interior_zone = "Conditioned"
exterior_zone = "Outdoor"
[[boundaries.material_layers]]
thickness_m = 0.1
conductivity_w_m_k = 1.0
density_kg_m3 = 0.0
specific_heat_j_kg_k = 900.0
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let result = build_synthetic_building(&config, None, None);
        assert!(
            result.is_ok(),
            "zero density with positive thickness should succeed (warning emitted)"
        );
    }

    /// Zero-specific-heat layer with positive thickness succeeds.
    #[test]
    fn zero_specific_heat_with_positive_thickness_succeeds() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 3600
duration_s = 3600

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "None"

[[boundaries]]
id = "wall-1"
boundary_type = "Wall"
area_m2 = 10.0
interior_zone = "Conditioned"
exterior_zone = "Outdoor"
[[boundaries.material_layers]]
thickness_m = 0.1
conductivity_w_m_k = 1.0
density_kg_m3 = 500.0
specific_heat_j_kg_k = 0.0
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let result = build_synthetic_building(&config, None, None);
        assert!(
            result.is_ok(),
            "zero specific_heat with positive thickness should succeed (warning emitted)"
        );
    }

    /// Negative density is rejected outright (physically impossible).
    #[test]
    fn negative_density_returned_as_error() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 3600
duration_s = 3600

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "None"

[[boundaries]]
id = "wall-1"
boundary_type = "Wall"
area_m2 = 10.0
interior_zone = "Conditioned"
exterior_zone = "Outdoor"
[[boundaries.material_layers]]
thickness_m = 0.1
conductivity_w_m_k = 1.0
density_kg_m3 = -500.0
specific_heat_j_kg_k = 900.0
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let result = build_synthetic_building(&config, None, None);
        assert!(result.is_err(), "negative density should be rejected");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("density_kg_m3"),
            "error message must reference density field, got: {err}"
        );
    }

    /// Negative U-factor is rejected.
    #[test]
    fn negative_u_factor_returned_as_error() {
        let toml = r#"
building_id = 1

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 3600
duration_s = 3600

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.0

[hvac]
equipment_name = "None"

[[windows]]
id = "win-1"
area_m2 = 2.0
u_factor_w_m2_k = -0.5
shgc = 0.7
"#;
        let config: SyntheticTomlConfig = toml::from_str(toml).expect("parse");
        let result = build_synthetic_building(&config, None, None);
        assert!(result.is_err(), "negative u_factor should be rejected");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("u_factor_w_m2_k"),
            "error message must reference u_factor field, got: {err}"
        );
    }
}
