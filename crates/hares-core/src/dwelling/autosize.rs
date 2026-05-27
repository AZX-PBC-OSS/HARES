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

/// Backup heating capacity factor: sized to 100% of design heating load
/// with no oversizing per ACCA Manual S-2017
/// (overridden by HPXML `<BackupHeatingAutosizingFactor>`).
const BACKUP_CAPACITY_FACTOR: f64 = 1.0;

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
/// For each equipment spec with `autosize_heating`, `autosize_cooling`, or
/// `autosize_backup` flags in its params:
///
/// 1. Determine design outdoor temperature from EPW "Extremes" header or
///    ASHRAE 152 climate station lookup (fallback).
/// 2. Call [`ThermalSolver::autosize_capacity`] to compute the required
///    HVAC capacity at design conditions (zero solar, zero internal gains).
/// 3. Apply oversizing factor from HPXML `<HeatingAutosizingFactor>` /
///    `<CoolingAutosizingFactor>` when present; fall back to Manual S
///    defaults (1.4x heating, 1.15x cooling) when absent.
///    Backup heating uses 100% of design load (no Manual S oversizing).
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
        // Cooler specs share the base params (cloned before heater/cooler split
        // in resolve_hvac.rs), so they also inherit autosize_backup when a backup
        // system is declared. Cooler configs do not consume backup_capacity_w —
        // computing backup capacity for them is wasted work and surprising.
        let needs_backup = spec
            .parameters
            .get("autosize_backup")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            && !spec.name.ends_with(" Cooler");

        if !needs_heating && !needs_cooling && !needs_backup {
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
            // Cooling autosizing: use peak solar design conditions per ACCA Manual J.
            // July 21 solar noon clear-sky irradiance is computed inside
            // autosize_capacity_cooling using the Perez (1990) model for each
            // envelope surface. This captures solar gain through windows, which
            // the heating path (zero-solar) does not.
            let raw_capacity = thermal
                .autosize_capacity_cooling(
                    indoor_zone_id,
                    cooling_setpoint_c,
                    cooling_design_c,
                    ctx.weather_lat,
                    ctx.weather_lon,
                )
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

        if needs_backup {
            // Backup heating autosizing: sized to 100% of design heating load
            // with no Manual S oversizing factor.
            // When heating capacity was already computed above, the same raw
            // capacity applies. Otherwise recompute it.
            let raw_capacity = thermal
                .autosize_capacity(indoor_zone_id, heating_setpoint_c, heating_design_c)
                .abs();

            // Backup factor: prefer HPXML <BackupHeatingAutosizingFactor>;
            // fall back to 100% of design load (BACKUP_CAPACITY_FACTOR = 1.0).
            let factor = spec
                .parameters
                .get("autosize_backup_factor")
                .and_then(|v| v.as_f64())
                .unwrap_or(BACKUP_CAPACITY_FACTOR);

            let backup_capacity = raw_capacity * factor;

            // Remove factor param consumed by autosizing.
            spec.parameters.remove("autosize_backup_factor");

            if backup_capacity > 0.0 {
                spec.parameters
                    .insert("backup_capacity_w".to_string(), json!(backup_capacity));
                spec.parameters.remove("autosize_backup");
                tracing::info!(
                    equipment = %spec.name,
                    backup_capacity_w = backup_capacity,
                    factor,
                    raw_capacity_w = raw_capacity,
                    design_outdoor_c = heating_design_c,
                    indoor_setpoint_c = heating_setpoint_c,
                    "autosized backup heating capacity"
                );
            } else {
                error!(
                    equipment = %spec.name,
                    raw_capacity_w = raw_capacity,
                    factor,
                    design_outdoor_c = heating_design_c,
                    "autosize backup capacity: solve returned zero — \
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
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
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
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
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
            name: "Gas Furnace".to_string(),
            instance_name: None,
            fuel_type: hares_types::FuelType::Gas,
            parameters: params,
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
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
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
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

    // ── Cooling autosizing with peak solar conditions (T-0119) ────────────

    /// Build a 1R1C solver with a south-facing window and a solar input
    /// column so that `autosize_capacity_cooling` can exercise the solar path.
    fn build_1r1c_solver_with_window(
        env: &EnvironmentState,
        indoor_temp_c: f64,
    ) -> (ThermalSolver, u32) {
        let ua = UA;
        let c = C;
        let a_c = DMatrix::from_row_slice(1, 1, &[-ua / c]);
        let b_c = DMatrix::from_row_slice(1, 3, &[ua / c, 1.0 / c, 1.0 / c]);
        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, DT_S, &mapping)
            .expect("1R1C state-space model must be stable");

        let window_surface_id: u32 = 42;
        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZONE, 0)]),
            zone_output_indices: HashMap::from([(ZONE, 0)]),
            zone_sensible_input_indices: HashMap::from([(ZONE, 1)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::from([(window_surface_id, 2)]),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: ZONE,
            window_zone_ids: HashMap::from([(window_surface_id, ZONE)]),
            window_properties: HashMap::from([(
                window_surface_id,
                hares_envelope::WindowSolarProperties {
                    shgc: 0.5,
                    winter_shgc: 0.5,
                    u_factor_w_m2_k: 2.0,
                    area_m2: 2.0,
                    transmittance: 0.4,
                    winter_transmittance: 0.4,
                    radiation_frac: 0.2,
                    glazing_curve: hares_physics::solar::GlazingCurve::from_u_shgc(2.0, 0.5),
                    tilt_deg: 90.0,
                    azimuth_deg: 180.0,
                },
            )]),
            ..ThermalSolverConfig::default()
        };
        let thermal = ThermalSolver::new(model, wiring, config, DT_S, env, indoor_temp_c)
            .expect("1R1C ThermalSolver with window must construct");
        (thermal, window_surface_id)
    }

    #[test]
    fn cooling_autosize_with_window_includes_solar_gain() {
        // Zone starts at 26 °C (above cooling setpoint 23.9 °C) so the
        // solver computes the COOLING capacity needed to reach the target.
        // With outdoor at 35 °C and a south-facing window adding solar gain,
        // more cooling capacity is required than the zero-solar case.
        let env = one_zone_env(26.0, 35.0);
        let (thermal, _win_id) = build_1r1c_solver_with_window(&env, 26.0);

        let zero_solar_capacity = thermal
            .autosize_capacity(ZONE, DEFAULT_COOLING_SETPOINT_C, 35.0)
            .abs();
        let solar_capacity = thermal
            .autosize_capacity_cooling(
                ZONE,
                DEFAULT_COOLING_SETPOINT_C,
                35.0,
                39.74, // Denver
                -104.87,
            )
            .abs();

        // Cooling with solar should be meaningfully larger than zero-solar.
        assert!(
            solar_capacity > zero_solar_capacity + 100.0,
            "solar cooling capacity {solar_capacity} W should exceed zero-solar \
             {zero_solar_capacity} W by >100 W for building with south-facing window"
        );

        // Both should be positive (cooling needed).
        assert!(
            zero_solar_capacity > 0.0,
            "zero-solar capacity {zero_solar_capacity} should be positive"
        );
        assert!(
            solar_capacity > 0.0,
            "solar capacity {solar_capacity} should be positive"
        );
    }

    #[test]
    fn heating_autosize_still_uses_zero_solar() {
        // Same solver with window, but heating mode with cold outdoor.
        let env = one_zone_env(18.0, -10.0);
        let (thermal, _win_id) = build_1r1c_solver_with_window(&env, 18.0);

        let heating_capacity = thermal
            .autosize_capacity(ZONE, DEFAULT_HEATING_SETPOINT_C, -10.0)
            .abs();
        let cooling_method_on_heating = thermal
            .autosize_capacity_cooling(ZONE, DEFAULT_HEATING_SETPOINT_C, -10.0, 39.74, -104.87)
            .abs();

        // Heating autosizing uses autosize_capacity (zero solar), not the
        // cooling method which would add July solar gain. The zero-solar
        // heating capacity should exceed the solar-included estimate
        // because July solar reduces the apparent heating load.
        assert!(
            heating_capacity > 400.0,
            "heating capacity {heating_capacity} W should be substantial at -10 °C"
        );
        assert!(
            heating_capacity > cooling_method_on_heating + 10.0,
            "zero-solar heating capacity {heating_capacity} W should exceed \
             solar-included heating capacity {cooling_method_on_heating} W \
             because July solar reduces the apparent heating load"
        );
    }

    // ── Backup heating autosizing (T-0120) ──────────────────────────

    #[test]
    fn autosize_backup_capacity_uses_design_heating_load() {
        let env = one_zone_env(20.0, 10.0);
        let thermal = build_1r1c_solver(&env, 20.0);

        let mut params = Map::new();
        params.insert("autosize_backup".to_string(), json!(true));
        // ASHP Heater — NOT ending in " Cooler" so needs_backup applies.
        let spec = EquipmentSpec {
            name: "ASHP Heater".to_string(),
            instance_name: None,
            fuel_type: hares_types::FuelType::Electric,
            parameters: params,
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
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
        let backup_w = result
            .parameters
            .get("backup_capacity_w")
            .and_then(|v| v.as_f64())
            .expect("backup_capacity_w must be set after backup autosizing");

        // Default factor = BACKUP_CAPACITY_FACTOR = 1.0 (no oversizing).
        let raw_capacity = thermal
            .autosize_capacity(ZONE, DEFAULT_HEATING_SETPOINT_C, -10.0)
            .abs();
        let expected = raw_capacity * BACKUP_CAPACITY_FACTOR;
        assert!(
            (backup_w - expected).abs() < 1e-6,
            "backup capacity {backup_w} should equal raw {raw_capacity} × factor 1.0 = {expected}"
        );
        // autosize_backup flag must be consumed.
        assert!(
            !result.parameters.contains_key("autosize_backup"),
            "autosize_backup flag must be removed after backup autosizing"
        );
    }

    #[test]
    fn autosize_backup_applies_factor_override() {
        let env = one_zone_env(20.0, 10.0);
        let thermal = build_1r1c_solver(&env, 20.0);

        let mut params = Map::new();
        params.insert("autosize_backup".to_string(), json!(true));
        params.insert("autosize_backup_factor".to_string(), json!(1.5));
        let spec = EquipmentSpec {
            name: "ASHP Heater".to_string(),
            instance_name: None,
            fuel_type: hares_types::FuelType::Electric,
            parameters: params,
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
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
        let backup_w = result
            .parameters
            .get("backup_capacity_w")
            .and_then(|v| v.as_f64())
            .expect("backup_capacity_w must be set after backup autosizing");

        let raw_capacity = thermal
            .autosize_capacity(ZONE, DEFAULT_HEATING_SETPOINT_C, -10.0)
            .abs();
        let with_override = raw_capacity * 1.5;
        let with_default = raw_capacity * BACKUP_CAPACITY_FACTOR;
        assert!(
            (backup_w - with_override).abs() < 1e-6,
            "with factor 1.5 override, backup {backup_w} should equal raw {raw_capacity} × 1.5 = {with_override}, not {with_default}"
        );
        assert!(
            (backup_w - with_default).abs() > 1.0,
            "with factor 1.5 override, backup {backup_w} must differ from default result {with_default}"
        );
        // autosize_backup flag must be consumed.
        assert!(
            !result.parameters.contains_key("autosize_backup"),
            "autosize_backup flag must be removed after backup autosizing"
        );
    }

    #[test]
    fn cooler_spec_excluded_from_backup_autosizing() {
        let env = one_zone_env(20.0, 10.0);
        let thermal = build_1r1c_solver(&env, 20.0);

        let mut params = Map::new();
        params.insert("autosize_backup".to_string(), json!(true));
        // Ends with " Cooler" — should be excluded from backup autosizing.
        let spec = EquipmentSpec {
            name: "ASHP Cooler".to_string(),
            instance_name: None,
            fuel_type: hares_types::FuelType::Electric,
            parameters: params,
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
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
        // Cooler specs should NOT receive backup_capacity_w.
        assert!(
            !result.parameters.contains_key("backup_capacity_w"),
            "cooler spec must not receive backup_capacity_w"
        );
        // autosize_backup flag should be left intact (not consumed).
        assert!(
            result.parameters.contains_key("autosize_backup"),
            "cooler spec should retain autosize_backup flag since it was excluded"
        );
    }

    #[test]
    fn cooling_autosize_without_windows_matches_zero_solar() {
        // No windows in config → autosize_capacity_cooling should return the
        // same result as autosize_capacity (pure conduction).
        let env = one_zone_env(26.0, 35.0);
        let thermal = build_1r1c_solver(&env, 26.0);

        let zero_solar = thermal
            .autosize_capacity(ZONE, DEFAULT_COOLING_SETPOINT_C, 35.0)
            .abs();
        let solar = thermal
            .autosize_capacity_cooling(ZONE, DEFAULT_COOLING_SETPOINT_C, 35.0, 0.0, 0.0)
            .abs();

        assert!(
            (solar - zero_solar).abs() < 1e-6,
            "without windows, solar and zero-solar should match: \
             solar={solar}, zero-solar={zero_solar}"
        );
    }

    // ── Multi-node steady-state capacity regression (T-0123) ────────────────

    /// Build a 2-node solver with a thermally massive wall node.
    ///
    /// Thermal network:
    ///   zone (C=C_zone) ── UA_zo W/K ── outdoor
    ///   zone (C=C_zone) ── UA_zw W/K ── wall (C=C_wall) ── UA_wo W/K ── outdoor
    ///
    /// This models a high-R envelope where the wall-mass node has much larger
    /// capacitance than the zone-air node. The cold-start back-solve bug (prior
    /// to T-0123) would dramatically inflate autosize capacity for this model
    /// because it had to heat the wall mass from a uniform initial temperature.
    fn build_2node_high_r_solver(
        env: &EnvironmentState,
        indoor_temp_c: f64,
    ) -> (ThermalSolver, f64) {
        const C_ZONE: f64 = 200_000.0;
        const C_WALL: f64 = 2_000_000.0;
        const UA_ZO: f64 = 20.0;
        const UA_ZW: f64 = 100.0;
        const UA_WO: f64 = 10.0;

        // Effective steady-state UA from zone to outdoor (wall acts as an
        // intermediate thermal path: zone → wall → outdoor).
        // UA_eff = UA_zo + 1 / (1/UA_zw + 1/UA_wo)
        //        = UA_zo + UA_zw × UA_wo / (UA_zw + UA_wo)
        let ua_eff = UA_ZO + UA_ZW * UA_WO / (UA_ZW + UA_WO);

        // A_c: 2×2, d/dt [T_zone; T_wall]
        let a_c = DMatrix::from_row_slice(
            2,
            2,
            &[
                -(UA_ZO + UA_ZW) / C_ZONE,
                UA_ZW / C_ZONE,
                UA_ZW / C_WALL,
                -(UA_ZW + UA_WO) / C_WALL,
            ],
        );

        // B_c: 2×2, columns = [outdoor temp, HVAC to zone]
        let b_c =
            DMatrix::from_row_slice(2, 2, &[UA_ZO / C_ZONE, 1.0 / C_ZONE, UA_WO / C_WALL, 0.0]);

        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };

        let model = StateSpaceModel::from_continuous(&a_c, &b_c, DT_S, &mapping)
            .expect("2-node model must be stable");

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

        let thermal = ThermalSolver::new(model, wiring, config, DT_S, env, indoor_temp_c)
            .expect("2-node ThermalSolver must construct");

        (thermal, ua_eff)
    }

    #[test]
    fn multi_node_autosize_matches_analytical_steady_state_load() {
        // Regression test for T-0123: a 2-node model with a thermally massive
        // wall node. The old code's cold-start one-step back-solve inflated
        // the apparent capacity by a factor proportional to C_wall/C_zone
        // (≈ 10× for this model). The fix computes the DC gain via two-call
        // perturbation, which produces the analytical steady-state capacity.
        let env = one_zone_env(20.0, -10.0);
        let (thermal, ua_eff) = build_2node_high_r_solver(&env, 20.0);

        let target_c = DEFAULT_HEATING_SETPOINT_C; // 21.1 °C
        let design_outdoor_c = -10.0;

        let capacity = thermal.autosize_capacity(ZONE, target_c, design_outdoor_c);

        // Analytical steady-state load: UA_eff × (T_target − T_outdoor).
        // For the parameters above: ~29.09 W/K × 31.1 K ≈ 904.7 W.
        let expected = ua_eff * (target_c - design_outdoor_c);
        let rel_error = (capacity - expected).abs() / expected.abs();

        assert!(
            rel_error < 0.02,
            "autosize capacity {capacity:.3} W deviates by {:.3}% from analytical \
             steady-state load {expected:.3} W (UA_eff={ua_eff:.3} W/K, \
             ΔT={:.1} K)",
            rel_error * 100.0,
            target_c - design_outdoor_c,
        );

        // Sanity: the old cold-start back-solve would return capacity inflated
        // by C_wall/C_zone + 1 ≈ 11×. The correct result should be well under 5×
        // the analytical load.
        assert!(
            capacity < 5.0 * expected,
            "capacity {capacity:.1} W is suspiciously inflated beyond 5× \
             steady-state load {expected:.1} W — check DC gain computation"
        );
    }

    #[test]
    fn multi_node_autosize_delta_t_linearity() {
        // The DC gain method is linear — doubling ΔT should double capacity
        // (within numerical precision). This guards against the old cold-start
        // behaviour where the transient energy to heat wall mass did NOT scale
        // linearly with ΔT.
        let env = one_zone_env(20.0, 0.0);
        let (thermal, ua_eff) = build_2node_high_r_solver(&env, 20.0);

        let target = 21.1;
        let d1 = thermal.autosize_capacity(ZONE, target, 0.0); // ΔT = 21.1
        let d2 = thermal.autosize_capacity(ZONE, target, -10.0); // ΔT = 31.1

        let ratio = d2 / d1;
        let expected_ratio = (target - (-10.0)) / (target - 0.0); // 31.1 / 21.1 ≈ 1.474
        let ratio_error = (ratio - expected_ratio).abs() / expected_ratio;

        assert!(
            ratio_error < 0.02,
            "capacity ratio ΔT=31.1 / ΔT=21.1 = {d2:.2} / {d1:.2} = {ratio:.4}, expected \
             {expected_ratio:.4} (±2%): ratio_error={:.3}%",
            ratio_error * 100.0,
        );

        // Both capacities must be positive (heating).
        assert!(d1 > 0.0, "ΔT=21.1 capacity must be positive, got {d1:.2}");
        assert!(d2 > 0.0, "ΔT=31.1 capacity must be positive, got {d2:.2}");

        // Both should be close to the analytical UA_eff × ΔT.
        let exp1 = ua_eff * (target - 0.0);
        let exp2 = ua_eff * (target - (-10.0));
        assert!(
            (d1 - exp1).abs() / exp1 < 0.02,
            "ΔT=21.1 deviates from analytical"
        );
        assert!(
            (d2 - exp2).abs() / exp2 < 0.02,
            "ΔT=31.1 deviates from analytical"
        );
    }

    // ── Full-pipeline integration tests (parse → resolve → autosize) ────

    use hares_io::defaults::DefaultsStore;
    use hares_io::hpxml::building::parse_building;
    use hares_io::hpxml::equipment::resolve_equipment;

    /// Build a minimal HPXML document with a gas furnace that omits
    /// `<HeatingCapacity>` but includes efficiency and building metadata.
    fn furnace_without_heating_capacity_hpxml() -> &'static str {
        r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site>
          <SiteType>suburban</SiteType>
          <Latitude>39.74</Latitude>
          <Longitude>-104.87</Longitude>
        </Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="ft2">2000</ConditionedFloorArea>
          <ConditionedBuildingVolume units="ft3">16000</ConditionedBuildingVolume>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure><Walls /></Enclosure>
      <Systems>
        <HVAC>
          <HeatingSystem>
            <SystemIdentifier id="fur1"/>
            <HeatingSystemFuel>natural gas</HeatingSystemFuel>
            <HeatingSystemType><Furnace/></HeatingSystemType>
            <AnnualHeatingEfficiency>
              <Units>AFUE</Units>
              <Value>0.92</Value>
            </AnnualHeatingEfficiency>
          </HeatingSystem>
        </HVAC>
      </Systems>
    </BuildingDetails>
  </Building>
</HPXML>"#
    }

    /// Build a minimal HPXML document with a gas furnace that includes an
    /// explicit `<HeatingCapacity>` — happy-path control for the autosizing
    /// pipeline test.
    fn furnace_with_heating_capacity_hpxml() -> &'static str {
        r#"<HPXML xmlns="http://hpxmlonline.com/2019/10" schemaVersion="4.0">
  <Building>
    <BuildingDetails>
      <BuildingSummary>
        <Site>
          <SiteType>suburban</SiteType>
          <Latitude>39.74</Latitude>
          <Longitude>-104.87</Longitude>
        </Site>
        <BuildingConstruction>
          <ConditionedFloorArea units="ft2">2000</ConditionedFloorArea>
          <ConditionedBuildingVolume units="ft3">16000</ConditionedBuildingVolume>
        </BuildingConstruction>
      </BuildingSummary>
      <Enclosure><Walls /></Enclosure>
      <Systems>
        <HVAC>
          <HeatingSystem>
            <SystemIdentifier id="fur1"/>
            <HeatingSystemFuel>natural gas</HeatingSystemFuel>
            <HeatingSystemType><Furnace/></HeatingSystemType>
            <AnnualHeatingEfficiency>
              <Units>AFUE</Units>
              <Value>0.92</Value>
            </AnnualHeatingEfficiency>
            <HeatingCapacity units="Btuh">60000</HeatingCapacity>
          </HeatingSystem>
        </HVAC>
      </Systems>
    </BuildingDetails>
  </Building>
</HPXML>"#
    }

    /// Resolve equipment from an HPXML string. Returns the gas furnace spec
    /// (the first HVAC spec in the resolved list).
    fn resolve_furnace_spec(xml: &str) -> EquipmentSpec {
        let building = parse_building(xml).expect("HPXML must parse");
        let specs = resolve_equipment(&building, &DefaultsStore::empty(), &json!({}))
            .expect("equipment must resolve");
        specs
            .into_iter()
            .find(|s| s.name == "Gas Furnace")
            .expect("Gas Furnace must be in resolved specs")
    }

    #[test]
    fn autosize_pipeline_parse_to_capacity_with_epw_conditions() {
        let xml = furnace_without_heating_capacity_hpxml();
        let spec = resolve_furnace_spec(xml);

        // After resolve: autosize flag must be set, no capacity, no typed config.
        assert!(
            spec.parameters
                .get("autosize_heating")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            "autosize_heating must be true when HeatingCapacity is omitted"
        );
        assert!(
            !spec.parameters.contains_key("heating_capacity_w"),
            "heating_capacity_w must be absent before autosizing"
        );
        assert!(
            spec.typed_config.is_none(),
            "typed_config must be None before autosizing"
        );

        // Build a 1R1C solver and run autosizing with EPW-derived design conditions.
        let env = one_zone_env(20.0, -5.0);
        let thermal = build_1r1c_solver(&env, 20.0);
        let building = minimal_building();
        let ctx = AutosizeContext {
            design_conditions: Some(DesignConditions {
                heating_design_db_c: -15.0,
                cooling_design_db_c: 38.0,
            }),
            weather_lat: 39.74,
            weather_lon: -104.87,
            duct_params: DuctDseParams::default(),
        };

        let mut specs = vec![spec];
        autosize_equipment_capacities(&mut specs, &thermal, &ctx, &building, ZONE);

        let result = &specs[0];
        let capacity_w = result
            .parameters
            .get("heating_capacity_w")
            .and_then(|v| v.as_f64())
            .expect("heating_capacity_w must be set after autosizing");

        // Autosize flag must be consumed.
        assert!(
            !result.parameters.contains_key("autosize_heating"),
            "autosize_heating must be removed after autosizing"
        );

        // Typed config must be rebuilt.
        assert!(
            result.typed_config.is_some(),
            "typed_config must be Some after autosizing"
        );

        // Capacity must be positive.
        assert!(
            capacity_w > 0.0,
            "autosized capacity {capacity_w} must be positive"
        );

        // Plausible range for this building: ΔT = 21.1 - (-15.0) = 36.1 K,
        // UA = 20 W/K, raw ≈ 722 W, after 1.4x ≈ 1010 W.
        let min_plausible = 100.0;
        let max_plausible = 50_000.0;
        assert!(
            capacity_w > min_plausible,
            "autosized capacity {capacity_w} W below plausible minimum {min_plausible} W"
        );
        assert!(
            capacity_w < max_plausible,
            "autosized capacity {capacity_w} W above plausible maximum {max_plausible} W"
        );

        // Capacity should scale with ΔT: computed value matches raw × Manual S factor.
        let raw_capacity = thermal
            .autosize_capacity(ZONE, DEFAULT_HEATING_SETPOINT_C, -15.0)
            .abs();
        let expected = raw_capacity * HEATING_OVERSIZE_FACTOR;
        assert!(
            (capacity_w - expected).abs() < 1e-6,
            "capacity {capacity_w} should equal raw {raw_capacity} × Manual S factor {HEATING_OVERSIZE_FACTOR} = {expected}"
        );
    }

    #[test]
    fn autosize_pipeline_parse_to_capacity_with_ashrae_152_fallback() {
        let xml = furnace_without_heating_capacity_hpxml();
        let spec = resolve_furnace_spec(xml);

        assert!(
            spec.parameters
                .get("autosize_heating")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            "autosize_heating must be true when HeatingCapacity is omitted"
        );

        // Build a 1R1C solver and run autosizing with design_conditions = None
        // to exercise the ASHRAE 152 climate station fallback.
        let env = one_zone_env(20.0, -5.0);
        let thermal = build_1r1c_solver(&env, 20.0);
        let building = minimal_building();
        let ctx = AutosizeContext {
            design_conditions: None,
            weather_lat: 39.74,
            weather_lon: -104.87,
            duct_params: DuctDseParams::default(),
        };

        let mut specs = vec![spec];
        autosize_equipment_capacities(&mut specs, &thermal, &ctx, &building, ZONE);

        let result = &specs[0];
        let capacity_w = result
            .parameters
            .get("heating_capacity_w")
            .and_then(|v| v.as_f64())
            .expect("heating_capacity_w must be set after autosizing via ASHRAE 152 fallback");

        assert!(
            !result.parameters.contains_key("autosize_heating"),
            "autosize_heating must be removed after autosizing"
        );
        assert!(
            result.typed_config.is_some(),
            "typed_config must be Some after autosizing via ASHRAE 152 fallback"
        );
        assert!(
            capacity_w > 0.0,
            "autosized capacity {capacity_w} must be positive with ASHRAE 152 fallback"
        );

        // Denver ASHRAE 152 heating design temp is 3 °F (≈ -16 °C).
        // ΔT = 21.1 − (−16.1) ≈ 37 K, UA = 20 W/K → raw ≈ 740 W, after 1.4× ≈ 1040 W.
        let min_plausible = 100.0;
        let max_plausible = 50_000.0;
        assert!(
            capacity_w > min_plausible,
            "ASHRAE 152 fallback capacity {capacity_w} W below plausible minimum {min_plausible} W"
        );
        assert!(
            capacity_w < max_plausible,
            "ASHRAE 152 fallback capacity {capacity_w} W above plausible maximum {max_plausible} W"
        );
    }

    #[test]
    fn autosize_pipeline_explicit_capacity_skips_autosizing() {
        let xml = furnace_with_heating_capacity_hpxml();
        let spec = resolve_furnace_spec(xml);

        // With explicit HeatingCapacity, autosize should NOT be flagged.
        assert!(
            !spec
                .parameters
                .get("autosize_heating")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            "autosize_heating must be false when HeatingCapacity is explicitly provided"
        );

        // The explicit capacity is stored in params by the resolver.
        let capacity_before = spec
            .parameters
            .get("heating_capacity_w")
            .and_then(|v| v.as_f64())
            .expect("heating_capacity_w must be present when capacity is explicit");

        // The typed config should already be populated.
        assert!(
            spec.typed_config.is_some(),
            "typed_config must be Some when capacity is explicitly provided"
        );

        // Running autosize should not change the spec (no autosize flags
        // means the spec is skipped entirely).
        let env = one_zone_env(20.0, -5.0);
        let thermal = build_1r1c_solver(&env, 20.0);
        let building = minimal_building();
        let ctx = AutosizeContext {
            design_conditions: Some(DesignConditions {
                heating_design_db_c: -15.0,
                cooling_design_db_c: 38.0,
            }),
            weather_lat: 39.74,
            weather_lon: -104.87,
            duct_params: DuctDseParams::default(),
        };

        let mut specs = vec![spec];
        autosize_equipment_capacities(&mut specs, &thermal, &ctx, &building, ZONE);

        let result = &specs[0];
        assert!(
            !result.parameters.contains_key("autosize_heating"),
            "autosize_heating was not set, must not appear after autosizing"
        );
        // Explicit capacity must still be present and unchanged.
        let capacity_after = result
            .parameters
            .get("heating_capacity_w")
            .and_then(|v| v.as_f64())
            .expect("heating_capacity_w must still be present after autosizing");
        assert!(
            (capacity_after - capacity_before).abs() < 1e-6,
            "explicit capacity must be unchanged by autosizing"
        );
    }
}
