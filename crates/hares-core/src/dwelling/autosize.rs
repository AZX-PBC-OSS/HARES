//! HVAC capacity autosizing from building envelope model.
//!
//! When HPXML equipment omits HeatingCapacity/CoolingCapacity, this module
//! computes the required capacity at design outdoor conditions using the
//! building's thermal model and ASHRAE 152 (or EPW) design temperatures.
//!
//! Oversizing factors per ACCA Manual S:
//! - Heating: 1.4x (overridden by HPXML `<HeatingAutosizingFactor>`)
//! - Cooling: 1.15x (overridden by HPXML `<CoolingAutosizingFactor>`)
//!
//! HPXML `<AutosizingLimits>` elements (Min/Max capacity bounds) are
//! applied as a clamp on the final sized capacity when present.

use hares_envelope::ThermalSolver;
use hares_io::{
    Building, DesignConditions, EquipmentSpec,
    hpxml::resolve_hvac::{DuctDseParams, rebuild_hvac_typed_config},
};
use hares_physics::ashrae152::design_temperatures_f;
use hares_physics::units::{temperature_c_to_f, temperature_f_to_c};
use hares_types::ZoneId;
use serde_json::json;
use tracing::{error, warn};

/// ASHRAE 90.1 default indoor design setpoints [°C].
/// ASHRAE 90.1-2019 §6.4.3.1.1.
const DEFAULT_HEATING_SETPOINT_C: f64 = 21.1; // 70 °F
const DEFAULT_COOLING_SETPOINT_C: f64 = 23.9; // 75 °F

/// Manual S oversizing factors.
/// ACCA Manual S-2017 §4: equipment sizing based on design loads.
const HEATING_OVERSIZE_FACTOR: f64 = 1.4;
const COOLING_OVERSIZE_FACTOR: f64 = 1.15;

/// Context bundle for autosizing: weather-derived design conditions and
/// duct parameters needed to rebuild typed equipment configs.
///
/// Groups together the EPW/ASHRAE 152 design temperature inputs and
/// ASHRAE 152 duct DSE parameters that `autosize_equipment_capacities`
/// requires, keeping the function signature under clippy's argument limit.
pub struct AutosizeContext {
    /// EPW design conditions from the weather file header, or `None`.
    pub design_conditions: Option<DesignConditions>,
    /// Weather file latitude [°N] for ASHRAE 152 fallback lookup.
    pub weather_lat: f64,
    /// Weather file longitude [°E] for ASHRAE 152 fallback lookup.
    pub weather_lon: f64,
    /// Duct DSE parameters for rebuilding typed equipment configs.
    pub duct_params: DuctDseParams,
}

/// Autosize HVAC equipment capacities for all specs that are missing
/// explicit capacity values from HPXML.
///
/// Runs after the thermal solver has been built, using the building's RC
/// model to compute the heating and cooling loads at ASHRAE design conditions.
///
/// For each equipment spec with `autosize_heating` or `autosize_cooling`
/// flags in its params:
///
/// 1. Determine design outdoor temperature from EPW "Extremes" header or
///    ASHRAE 152 climate station lookup (fallback).
/// 2. Call [`ThermalSolver::autosize_capacity`] to compute the required
///    HVAC capacity at design conditions (zero solar, zero internal gains).
/// 3. Apply oversizing factor from HPXML `<HeatingAutosizingFactor>` /
///    `<CoolingAutosizingFactor>` when present; fall back to Manual S
///    defaults (1.4x heating, 1.15x cooling) when absent.
/// 4. Apply capacity limits from HPXML `<AutosizingLimits>` when present
///    (clamp to Min/Max after oversizing).
/// 5. Update the equipment spec's parameters and rebuild its typed config.
///
/// # Arguments
///
/// * `specs` - Equipment specs from HPXML resolution (mutated in place).
/// * `thermal` - The assembled building thermal solver.
/// * `ctx` - Autosizing context (design conditions, weather location, duct params).
/// * `building` - HPXML building data (for setpoint defaults and site lat/lon).
/// * `indoor_zone_id` - The primary conditioned zone for HVAC delivery.
pub fn autosize_equipment_capacities(
    specs: &mut [EquipmentSpec],
    thermal: &ThermalSolver,
    ctx: &AutosizeContext,
    building: &Building,
    indoor_zone_id: ZoneId,
) {
    // Resolve outdoor design temperatures.
    // Prefer EPW design conditions; fall back to ASHRAE 152 station lookup.
    let (heating_design_c, cooling_design_c) = resolve_design_temperatures(
        ctx.design_conditions,
        ctx.weather_lat,
        ctx.weather_lon,
        building.site.latitude_deg,
        building.site.longitude_deg,
    );

    for spec in specs.iter_mut() {
        let needs_heating = spec
            .parameters
            .get("autosize_heating")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let needs_cooling = spec
            .parameters
            .get("autosize_cooling")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if !needs_heating && !needs_cooling {
            continue;
        }

        // Determine indoor design setpoints.
        // Prefer the equipment's own setpoint; fall back to building setpoints;
        // fall back to ASHRAE 90.1 defaults.
        let heating_setpoint_c = resolve_heating_setpoint_c(spec, building);
        let cooling_setpoint_c = resolve_cooling_setpoint_c(spec, building);

        if needs_heating {
            let raw_capacity = thermal
                .autosize_capacity(indoor_zone_id, heating_setpoint_c, heating_design_c)
                .abs();

            // Oversizing factor: prefer HPXML <HeatingAutosizingFactor>;
            // fall back to ACCA Manual S default 1.4x.
            let factor = spec
                .parameters
                .get("autosize_heating_factor")
                .and_then(|v| v.as_f64())
                .unwrap_or(HEATING_OVERSIZE_FACTOR);

            let mut sized_capacity = raw_capacity * factor;

            // Apply capacity limits from HPXML <AutosizingLimits> if present.
            if let Some(min_w) = spec
                .parameters
                .get("autosize_heating_min_w")
                .and_then(|v| v.as_f64())
            {
                sized_capacity = sized_capacity.max(min_w);
            }
            if let Some(max_w) = spec
                .parameters
                .get("autosize_heating_max_w")
                .and_then(|v| v.as_f64())
            {
                let unclamped = sized_capacity;
                sized_capacity = sized_capacity.min(max_w);
                if unclamped > max_w + f64::EPSILON {
                    tracing::debug!(
                        equipment = %spec.name,
                        max_w,
                        clamped_capacity = sized_capacity,
                        "heating capacity clamped to <AutosizingLimits> maximum"
                    );
                }
            }

            // Remove factor and limit params consumed by autosizing.
            spec.parameters.remove("autosize_heating_factor");
            spec.parameters.remove("autosize_heating_min_w");
            spec.parameters.remove("autosize_heating_max_w");

            if sized_capacity > 0.0 {
                spec.parameters
                    .insert("heating_capacity_w".to_string(), json!(sized_capacity));
                spec.parameters.remove("autosize_heating");
                tracing::info!(
                    equipment = %spec.name,
                    raw_capacity_w = raw_capacity,
                    sized_capacity_w = sized_capacity,
                    factor,
                    design_outdoor_c = heating_design_c,
                    indoor_setpoint_c = heating_setpoint_c,
                    "autosized heating capacity"
                );
            } else {
                error!(
                    equipment = %spec.name,
                    raw_capacity_w = raw_capacity,
                    factor,
                    design_outdoor_c = heating_design_c,
                    "autosize heating capacity: solve returned zero — \
                     check model configuration"
                );
            }
        }

        if needs_cooling {
            let raw_capacity = thermal
                .autosize_capacity(indoor_zone_id, cooling_setpoint_c, cooling_design_c)
                .abs();

            // Oversizing factor: prefer HPXML <CoolingAutosizingFactor>;
            // fall back to ACCA Manual S default 1.15x.
            let factor = spec
                .parameters
                .get("autosize_cooling_factor")
                .and_then(|v| v.as_f64())
                .unwrap_or(COOLING_OVERSIZE_FACTOR);

            let mut sized_capacity = raw_capacity * factor;

            // Apply capacity limits from HPXML <AutosizingLimits> if present.
            if let Some(min_w) = spec
                .parameters
                .get("autosize_cooling_min_w")
                .and_then(|v| v.as_f64())
            {
                sized_capacity = sized_capacity.max(min_w);
            }
            if let Some(max_w) = spec
                .parameters
                .get("autosize_cooling_max_w")
                .and_then(|v| v.as_f64())
            {
                let unclamped = sized_capacity;
                sized_capacity = sized_capacity.min(max_w);
                if unclamped > max_w + f64::EPSILON {
                    tracing::debug!(
                        equipment = %spec.name,
                        max_w,
                        clamped_capacity = sized_capacity,
                        "cooling capacity clamped to <AutosizingLimits> maximum"
                    );
                }
            }

            // Remove factor and limit params consumed by autosizing.
            spec.parameters.remove("autosize_cooling_factor");
            spec.parameters.remove("autosize_cooling_min_w");
            spec.parameters.remove("autosize_cooling_max_w");

            if sized_capacity > 0.0 {
                spec.parameters
                    .insert("cooling_capacity_w".to_string(), json!(sized_capacity));
                spec.parameters.remove("autosize_cooling");
                tracing::info!(
                    equipment = %spec.name,
                    raw_capacity_w = raw_capacity,
                    sized_capacity_w = sized_capacity,
                    factor,
                    design_outdoor_c = cooling_design_c,
                    indoor_setpoint_c = cooling_setpoint_c,
                    "autosized cooling capacity"
                );
            } else {
                error!(
                    equipment = %spec.name,
                    raw_capacity_w = raw_capacity,
                    factor,
                    design_outdoor_c = cooling_design_c,
                    "autosize cooling capacity: solve returned zero — \
                     check model configuration"
                );
            }
        }

        // Rebuild typed config with updated capacities.
        spec.typed_config =
            rebuild_hvac_typed_config(&spec.name, &spec.parameters, &ctx.duct_params);
    }
}

/// Resolve outdoor design dry-bulb temperatures [°C].
///
/// Prefers EPW design conditions (parsed from the "Extremes" section of the
/// EPW header). Falls back to the nearest ASHRAE 152 climate station when
/// EPW design conditions are unavailable (e.g., PSM3, TMY3, ResStock CSV).
fn resolve_design_temperatures(
    design_conditions: Option<DesignConditions>,
    weather_lat: f64,
    weather_lon: f64,
    site_lat: Option<f64>,
    site_lon: Option<f64>,
) -> (f64, f64) {
    // Try EPW design conditions first.
    if let Some(dc) = design_conditions {
        if dc.heating_design_db_c.is_finite() && dc.cooling_design_db_c.is_finite() {
            return (dc.heating_design_db_c, dc.cooling_design_db_c);
        }
    }

    // Fall back to ASHRAE 152 climate station lookup.
    let lat = site_lat.unwrap_or(weather_lat);
    let lon = site_lon.unwrap_or(weather_lon);
    let (htg_f, clg_f) = design_temperatures_f(lat, lon).unwrap_or_else(|| {
        warn!(
            lat,
            lon,
            "no ASHRAE 152 station found — using conservative defaults: \
             heating -10 °C, cooling 35 °C"
        );
        (temperature_c_to_f(-10.0), temperature_c_to_f(35.0))
    });

    // ASHRAE 152 returns °F; convert to °C.
    let htg_c = temperature_f_to_c(htg_f);
    let clg_c = temperature_f_to_c(clg_f);
    (htg_c, clg_c)
}

/// Extract the heating setpoint from an equipment spec, falling back to
/// building setpoints and ASHRAE 90.1 defaults.
fn resolve_heating_setpoint_c(spec: &EquipmentSpec, building: &Building) -> f64 {
    // Try the equipment's explicit heating setpoint.
    if let Some(sp) = spec
        .parameters
        .get("heating_setpoint_c")
        .and_then(|v| v.as_f64())
    {
        return sp;
    }

    // Try the building's heating setpoint profile (use midnight value).
    if let Some(ref profile) = building.heating_weekday_setpoints_c {
        if !profile.is_empty() {
            return profile[0];
        }
    }

    // ASHRAE 90.1 default.
    DEFAULT_HEATING_SETPOINT_C
}

/// Extract the cooling setpoint from an equipment spec, falling back to
/// building setpoints and ASHRAE 90.1 defaults.
fn resolve_cooling_setpoint_c(spec: &EquipmentSpec, building: &Building) -> f64 {
    // Try the equipment's explicit cooling setpoint.
    if let Some(sp) = spec
        .parameters
        .get("cooling_setpoint_c")
        .and_then(|v| v.as_f64())
    {
        return sp;
    }

    // Try the building's cooling setpoint profile (use midnight value).
    if let Some(ref profile) = building.cooling_weekday_setpoints_c {
        if !profile.is_empty() {
            return profile[0];
        }
    }

    // ASHRAE 90.1 default.
    DEFAULT_COOLING_SETPOINT_C
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use hares_envelope::{OutputMapping, StateSpaceModel, StateSpaceWiring, ThermalSolverConfig};
    use hares_types::{EnvironmentState, GridState, WeatherState, ZoneId, ZoneState};
    use nalgebra::DMatrix;
    use serde_json::Map;
    use std::collections::HashMap;

    const ZONE: ZoneId = ZoneId(1);
    const UA: f64 = 20.0; // W/K
    const C: f64 = 200_000.0; // J/K
    const DT_S: f64 = 60.0; // s

    fn one_zone_env(zone_temp_c: f64, outdoor_temp_c: f64) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZONE,
                temperature_c: zone_temp_c,
                humidity_ratio: 0.008,
                relative_humidity: 0.45,
                wet_bulb_c: zone_temp_c - 5.0,
                volume_m3: 250.0,
            }],
            weather: WeatherState {
                outdoor_temp_c,
                outdoor_humidity_ratio: 0.004,
                outdoor_wet_bulb_c: outdoor_temp_c - 5.0,
                outdoor_enthalpy_j_kg: 22_800.0,
                wind_speed_m_s: 0.0,
                wind_dir_deg: 0.0,
                ground_temp_c: outdoor_temp_c,
                sky_temp_c: outdoor_temp_c - 5.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![],
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
                solar_altitude_deg: 0.0,
                solar_azimuth_deg: 180.0,
                mains_temp_c: 15.0,
                rainfall_m: 0.0,
                ground_albedo: 0.2,
                ground_t_mean_c: 10.0,
                ground_t_amplitude_c: 0.0,
                ground_phase_day: 35.0,
                day_of_year: 1.0,
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
            },
            custom_domains: vec![],
            equipment_telemetry: HashMap::new(),
            current_time: chrono::FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 3, 20, 12, 0, 0)
                .single()
                .expect("valid timestamp"),
            time_res: chrono::Duration::seconds(DT_S as i64),
            price_signal: Default::default(),
            electrical: Default::default(),
            equipment_core: Default::default(),
        }
    }

    fn build_1r1c_solver(env: &EnvironmentState, indoor_temp_c: f64) -> ThermalSolver {
        let a_c = DMatrix::from_row_slice(1, 1, &[-UA / C]);
        let b_c = DMatrix::from_row_slice(1, 2, &[UA / C, 1.0 / C]);
        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, DT_S, &mapping)
            .expect("1R1C state-space model must be stable");
        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZONE, 0)]),
            zone_output_indices: HashMap::from([(ZONE, 0)]),
            zone_sensible_input_indices: HashMap::from([(ZONE, 1)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: ZONE,
            ..ThermalSolverConfig::default()
        };
        ThermalSolver::new(model, wiring, config, DT_S, env, indoor_temp_c)
            .expect("1R1C ThermalSolver construction must succeed")
    }

    fn minimal_building() -> Building {
        Building {
            site: hares_io::hpxml::Site {
                elevation_m: None,
                site_type: None,
                shielding_of_home: None,
                latitude_deg: Some(39.74),
                longitude_deg: Some(-104.87),
            },
            zones: vec![],
            boundaries: vec![],
            windows: vec![],
            infiltration_ach50: None,
            infiltration_cfm50: None,
            infiltration_ach_natural: None,
            infiltration_cfm_natural: None,
            infiltration_ela_cm2: None,
            infiltration_constant_ach: None,
            hvac_capacity_w: None,
            seer2: None,
            hspf2: None,
            water_heater_setpoint_c: None,
            heating_weekday_setpoints_c: None,
            heating_weekend_setpoints_c: None,
            cooling_weekday_setpoints_c: None,
            cooling_weekend_setpoints_c: None,
            battery_round_trip_efficiency: None,
            pv_tilt_deg: None,
            conditioned_volume_m3: None,
            ceiling_height_m: None,
            infiltration_height_m: None,
            floors_above_grade: None,
            has_flue_or_chimney: None,
            foundation_name: None,
            residential_facility_type: None,
            mass_multiplier_override: None,
            hvac_deadband_c: None,
            details_xml: hares_io::hpxml::building::XmlNode {
                name: "root".into(),
                attrs: Default::default(),
                text: String::new(),
                children: vec![],
            },
        }
    }

    #[test]
    fn default_setpoints_match_ashrae_90_1() {
        assert!((DEFAULT_HEATING_SETPOINT_C - 21.1).abs() < f64::EPSILON);
        assert!((DEFAULT_COOLING_SETPOINT_C - 23.9).abs() < f64::EPSILON);
    }

    #[test]
    fn oversize_factors_match_manual_s() {
        assert!((HEATING_OVERSIZE_FACTOR - 1.4).abs() < f64::EPSILON);
        assert!((COOLING_OVERSIZE_FACTOR - 1.15).abs() < f64::EPSILON);
    }

    #[test]
    fn resolve_design_temps_uses_epw_when_available() {
        let dc = DesignConditions {
            heating_design_db_c: -15.0,
            cooling_design_db_c: 38.0,
        };
        let (htg, clg) = resolve_design_temperatures(Some(dc), 0.0, 0.0, None, None);
        assert!((htg - (-15.0)).abs() < f64::EPSILON, "got {htg}");
        assert!((clg - 38.0).abs() < f64::EPSILON, "got {clg}");
    }

    #[test]
    fn resolve_design_temps_falls_back_to_ashrae_152() {
        let (htg, clg) =
            resolve_design_temperatures(None, 39.74, -104.87, Some(39.74), Some(-104.87));
        assert!(
            htg < -5.0,
            "Denver heating design {htg}°C should be below -5°C"
        );
        assert!(
            clg > 30.0,
            "Denver cooling design {clg}°C should be above 30°C"
        );
    }

    #[test]
    fn autosize_uses_manual_s_default_when_factor_absent() {
        let env = one_zone_env(20.0, 10.0);
        let thermal = build_1r1c_solver(&env, 20.0);

        let mut params = Map::new();
        params.insert("autosize_heating".to_string(), json!(true));
        let spec = EquipmentSpec {
            name: "Gas Furnace".to_string(),
            instance_name: None,
            fuel_type: hares_types::FuelType::Gas,
            parameters: params,
            zip_params: None,
            typed_config: None,
        };
        let mut specs = vec![spec];

        let ctx = AutosizeContext {
            design_conditions: Some(DesignConditions {
                heating_design_db_c: -10.0,
                cooling_design_db_c: 35.0,
            }),
            weather_lat: 0.0,
            weather_lon: 0.0,
            duct_params: DuctDseParams::default(),
        };
        let building = minimal_building();

        autosize_equipment_capacities(&mut specs, &thermal, &ctx, &building, ZONE);

        let result = &specs[0];
        let capacity_w = result
            .parameters
            .get("heating_capacity_w")
            .and_then(|v| v.as_f64())
            .expect("heating_capacity_w must be set after autosizing");

        let raw_capacity = thermal
            .autosize_capacity(ZONE, DEFAULT_HEATING_SETPOINT_C, -10.0)
            .abs();
        let expected = raw_capacity * HEATING_OVERSIZE_FACTOR;
        assert!(
            (capacity_w - expected).abs() < 1e-6,
            "with no factor override, capacity {capacity_w} should equal raw {raw_capacity} × Manual S factor {HEATING_OVERSIZE_FACTOR} = {expected}"
        );
    }

    #[test]
    fn autosize_applies_hpxml_heating_factor_override() {
        let env = one_zone_env(20.0, 10.0);
        let thermal = build_1r1c_solver(&env, 20.0);

        let mut params = Map::new();
        params.insert("autosize_heating".to_string(), json!(true));
        params.insert("autosize_heating_factor".to_string(), json!(1.2));
        let spec = EquipmentSpec {
            name: "Gas Furnace".to_string(),
            instance_name: None,
            fuel_type: hares_types::FuelType::Gas,
            parameters: params,
            zip_params: None,
            typed_config: None,
        };
        let mut specs = vec![spec];

        let ctx = AutosizeContext {
            design_conditions: Some(DesignConditions {
                heating_design_db_c: -10.0,
                cooling_design_db_c: 35.0,
            }),
            weather_lat: 0.0,
            weather_lon: 0.0,
            duct_params: DuctDseParams::default(),
        };
        let building = minimal_building();

        autosize_equipment_capacities(&mut specs, &thermal, &ctx, &building, ZONE);

        let result = &specs[0];
        let capacity_w = result
            .parameters
            .get("heating_capacity_w")
            .and_then(|v| v.as_f64())
            .expect("heating_capacity_w must be set after autosizing");

        let raw_capacity = thermal
            .autosize_capacity(ZONE, DEFAULT_HEATING_SETPOINT_C, -10.0)
            .abs();
        let with_override = raw_capacity * 1.2;
        let with_default = raw_capacity * HEATING_OVERSIZE_FACTOR;
        assert!(
            (capacity_w - with_override).abs() < 1e-6,
            "with factor 1.2 override, capacity {capacity_w} should equal raw {raw_capacity} × 1.2 = {with_override}, not {with_default}"
        );
        assert!(
            (capacity_w - with_default).abs() > 1.0,
            "with factor 1.2 override, capacity {capacity_w} must differ from Manual S default result {with_default}"
        );
    }

    #[test]
    fn autosize_applies_hpxml_cooling_factor_override() {
        let env = one_zone_env(20.0, 10.0);
        let thermal = build_1r1c_solver(&env, 20.0);

        let mut params = Map::new();
        params.insert("autosize_cooling".to_string(), json!(true));
        params.insert("autosize_cooling_factor".to_string(), json!(1.0));
        let spec = EquipmentSpec {
            name: "Air Conditioner".to_string(),
            instance_name: None,
            fuel_type: hares_types::FuelType::Electric,
            parameters: params,
            zip_params: None,
            typed_config: None,
        };
        let mut specs = vec![spec];

        let ctx = AutosizeContext {
            design_conditions: Some(DesignConditions {
                heating_design_db_c: -10.0,
                cooling_design_db_c: 35.0,
            }),
            weather_lat: 0.0,
            weather_lon: 0.0,
            duct_params: DuctDseParams::default(),
        };
        let building = minimal_building();

        autosize_equipment_capacities(&mut specs, &thermal, &ctx, &building, ZONE);

        let result = &specs[0];
        let capacity_w = result
            .parameters
            .get("cooling_capacity_w")
            .and_then(|v| v.as_f64())
            .expect("cooling_capacity_w must be set after autosizing");

        let raw_capacity = thermal
            .autosize_capacity(ZONE, DEFAULT_COOLING_SETPOINT_C, 35.0)
            .abs();
        let with_override = raw_capacity * 1.0;
        let with_default = raw_capacity * COOLING_OVERSIZE_FACTOR;
        assert!(
            (capacity_w - with_override).abs() < 1e-6,
            "with factor 1.0 override, capacity {capacity_w} should equal raw {raw_capacity} × 1.0 = {with_override}, not {with_default}"
        );
        assert!(
            (capacity_w - with_default).abs() > 1.0,
            "with factor 1.0 override, capacity {capacity_w} must differ from Manual S default result {with_default}"
        );
    }

    #[test]
    fn autosize_applies_limits_clamp() {
        let env = one_zone_env(20.0, 10.0);
        let thermal = build_1r1c_solver(&env, 20.0);

        let mut params = Map::new();
        params.insert("autosize_heating".to_string(), json!(true));
        params.insert("autosize_heating_min_w".to_string(), json!(5000.0));
        params.insert("autosize_heating_max_w".to_string(), json!(600.0));
        let spec = EquipmentSpec {
            name: "Gas Furnace".to_string(),
            instance_name: None,
            fuel_type: hares_types::FuelType::Gas,
            parameters: params,
            zip_params: None,
            typed_config: None,
        };
        let mut specs = vec![spec];

        let ctx = AutosizeContext {
            design_conditions: Some(DesignConditions {
                heating_design_db_c: -10.0,
                cooling_design_db_c: 35.0,
            }),
            weather_lat: 0.0,
            weather_lon: 0.0,
            duct_params: DuctDseParams::default(),
        };
        let building = minimal_building();

        autosize_equipment_capacities(&mut specs, &thermal, &ctx, &building, ZONE);

        let result = &specs[0];
        let capacity_w = result
            .parameters
            .get("heating_capacity_w")
            .and_then(|v| v.as_f64())
            .expect("heating_capacity_w must be set after autosizing");

        let raw_capacity = thermal
            .autosize_capacity(ZONE, DEFAULT_HEATING_SETPOINT_C, -10.0)
            .abs();
        let sized = raw_capacity * HEATING_OVERSIZE_FACTOR;
        assert!(
            sized > 600.0,
            "raw capacity {sized} must exceed max limit 600 W for this test to be meaningful"
        );
        assert!(
            (capacity_w - 600.0).abs() < 1e-6,
            "capacity must be clamped to max limit 600 W, got {capacity_w}"
        );
    }
}
