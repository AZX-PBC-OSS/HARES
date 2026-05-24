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
    Building, EquipmentSpec, WeatherTimeSeries,
    hpxml::resolve_hvac::{DuctDseParams, rebuild_hvac_typed_config},
};
use hares_physics::ashrae152::design_temperatures_f;
use hares_types::ZoneId;
use serde_json::{Map, Value, json};
use tracing::warn;

/// ASHRAE 90.1 default indoor design setpoints [°C].
const DEFAULT_HEATING_SETPOINT_C: f64 = 21.1; // 70 °F
const DEFAULT_COOLING_SETPOINT_C: f64 = 23.9; // 75 °F

/// Manual S oversizing factors.
const HEATING_OVERSIZE_FACTOR: f64 = 1.4;
const COOLING_OVERSIZE_FACTOR: f64 = 1.15;

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
/// * `weather` - The parsed weather time series (for lat/lon and EPW design temps).
/// * `building` - HPXML building data (for setpoint defaults).
/// * `duct_params` - Duct DSE parameters (needed to rebuild typed configs).
/// * `indoor_zone_id` - The primary conditioned zone for HVAC delivery.
pub fn autosize_equipment_capacities(
    specs: &mut [EquipmentSpec],
    thermal: &ThermalSolver,
    weather: &WeatherTimeSeries,
    building: &Building,
    duct_params: &DuctDseParams,
    indoor_zone_id: ZoneId,
) {
    // Resolve outdoor design temperatures.
    // Prefer EPW design conditions; fall back to ASHRAE 152 station lookup.
    let (heating_design_c, cooling_design_c) = resolve_design_temperatures(
        weather,
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
        spec.typed_config = rebuild_hvac_typed_config(&spec.name, &spec.parameters, duct_params);
    }
}

/// Resolve outdoor design dry-bulb temperatures [°C].
///
/// Prefers EPW design conditions (parsed from the "Extremes" section of the
/// EPW header). Falls back to the nearest ASHRAE 152 climate station when
/// EPW design conditions are unavailable (e.g., PSM3, TMY3, ResStock CSV).
fn resolve_design_temperatures(
    weather: &WeatherTimeSeries,
    site_lat: Option<f64>,
    site_lon: Option<f64>,
) -> (f64, f64) {
    // Try EPW design conditions first.
    if let Some(dc) = weather.design_conditions {
        if dc.heating_design_db_c.is_finite() && dc.cooling_design_db_c.is_finite() {
            return (dc.heating_design_db_c, dc.cooling_design_db_c);
        }
    }

    // Fall back to ASHRAE 152 climate station lookup.
    let lat = site_lat.unwrap_or(weather.meta.latitude);
    let lon = site_lon.unwrap_or(weather.meta.longitude);
    let (htg_f, clg_f) = design_temperatures_f(lat, lon).unwrap_or_else(|| {
        warn!(
            lat,
            lon,
            "no ASHRAE 152 station found — using conservative defaults: \
             heating -10 °C, cooling 35 °C"
        );
        (-10.0_f64.to_fahrenheit(), 35.0_f64.to_fahrenheit())
    });

    // ASHRAE 152 returns °F; convert to °C.
    let htg_c = (htg_f - 32.0) / 1.8;
    let clg_c = (clg_f - 32.0) / 1.8;
    (htg_c, clg_c)
}

/// Fahrenheit → Celsius conversion.
trait ToCelsius {
    fn to_fahrenheit(self) -> f64;
}

impl ToCelsius for f64 {
    fn to_fahrenheit(self) -> f64 {
        self * 1.8 + 32.0
    }
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
}
