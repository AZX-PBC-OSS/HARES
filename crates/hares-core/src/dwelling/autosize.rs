//! HVAC capacity autosizing from building envelope model.
//!
//! When HPXML equipment omits HeatingCapacity/CoolingCapacity, this module
//! computes the required capacity at design outdoor conditions using the
//! building's thermal model and ASHRAE 152 (or EPW) design temperatures.
//!
//! Oversizing factors per ACCA Manual S:
//! - Heating: 1.4x
//! - Cooling: 1.15x

use hares_envelope::ThermalSolver;
use hares_io::{
    Building, DesignConditions, EquipmentSpec,
    hpxml::resolve_hvac::{DuctDseParams, rebuild_hvac_typed_config},
};
use hares_physics::ashrae152::design_temperatures_f;
use hares_types::ZoneId;
use serde_json::json;
use tracing::warn;

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
/// 3. Apply Manual S oversizing factors (1.4x heating, 1.15x cooling).
/// 4. Update the equipment spec's parameters and rebuild its typed config.
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
            let sized_capacity = raw_capacity * HEATING_OVERSIZE_FACTOR;

            if sized_capacity > 0.0 {
                spec.parameters
                    .insert("heating_capacity_w".to_string(), json!(sized_capacity));
                spec.parameters.remove("autosize_heating");
                tracing::info!(
                    equipment = %spec.name,
                    raw_capacity_w = raw_capacity,
                    sized_capacity_w = sized_capacity,
                    design_outdoor_c = heating_design_c,
                    indoor_setpoint_c = heating_setpoint_c,
                    "autosized heating capacity"
                );
            } else {
                warn!(
                    equipment = %spec.name,
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
            let sized_capacity = raw_capacity * COOLING_OVERSIZE_FACTOR;

            if sized_capacity > 0.0 {
                spec.parameters
                    .insert("cooling_capacity_w".to_string(), json!(sized_capacity));
                spec.parameters.remove("autosize_cooling");
                tracing::info!(
                    equipment = %spec.name,
                    raw_capacity_w = raw_capacity,
                    sized_capacity_w = sized_capacity,
                    design_outdoor_c = cooling_design_c,
                    indoor_setpoint_c = cooling_setpoint_c,
                    "autosized cooling capacity"
                );
            } else {
                warn!(
                    equipment = %spec.name,
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
        (celsius_to_fahrenheit(-10.0), celsius_to_fahrenheit(35.0))
    });

    // ASHRAE 152 returns °F; convert to °C.
    let htg_c = (htg_f - 32.0) / 1.8;
    let clg_c = (clg_f - 32.0) / 1.8;
    (htg_c, clg_c)
}

/// Celsius → Fahrenheit conversion.
fn celsius_to_fahrenheit(c: f64) -> f64 {
    c * 1.8 + 32.0
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
    fn celsius_to_fahrenheit_conversion() {
        assert!((celsius_to_fahrenheit(0.0) - 32.0).abs() < f64::EPSILON);
        assert!((celsius_to_fahrenheit(100.0) - 212.0).abs() < f64::EPSILON);
        assert!((celsius_to_fahrenheit(-10.0) - 14.0).abs() < f64::EPSILON);
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
        // Denver coordinates (39.74, -104.87). ASHRAE 152 station should be nearby
        // and return plausible design temperatures in °F.
        let (htg, clg) =
            resolve_design_temperatures(None, 39.74, -104.87, Some(39.74), Some(-104.87));
        // Heating design temp for Denver should be well below freezing in °C.
        assert!(
            htg < -5.0,
            "Denver heating design {htg}°C should be below -5°C"
        );
        // Cooling design temp should be ≥ 30°C.
        assert!(
            clg > 30.0,
            "Denver cooling design {clg}°C should be above 30°C"
        );
    }
}
