//! Shared helper utilities for HVAC equipment implementations.

use hares_types::{
    ControlSignal, EnvironmentState, FluidType, FuelType, HaresError, LoopId, OperatingMode, ZoneId,
};

use crate::EquipmentConfig;

// Re-exported from crate root; used by `apply_heating_control_unchecked`
// and `update_heating_control`.
use crate::HvacEquipment;

/// Standard config keys for heating capacity with OCHRE aliases.
#[allow(dead_code)]
pub const HEATING_CAPACITY_KEYS: &[&str] = &[
    "capacity_w",
    "heating_capacity_w",
    "capacity",
    "HVAC Heating Capacity (W)",
];

/// Standard config keys for duct distribution system efficiency.
#[allow(dead_code)]
pub const DUCT_DSE_KEYS: &[&str] = &["duct_dse", "duct_distribution_efficiency"];

pub fn first_f64(config: &EquipmentConfig, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| config.get_f64(key))
}

fn validate_u16_id(raw: f64) -> bool {
    raw.is_finite() && raw >= 0.0 && raw.fract() == 0.0 && raw <= u16::MAX as f64
}

pub fn zone_id_from_config(config: &EquipmentConfig) -> Option<ZoneId> {
    let raw = config.get_f64("zone_id")?;
    if !validate_u16_id(raw) {
        return None;
    }
    Some(ZoneId(raw as u16))
}

/// Parse an optional `ZoneId` from a named config key.
pub fn parse_zone_id_key(config: &EquipmentConfig, key: &str) -> Option<ZoneId> {
    let raw = config.get_f64(key)?;
    if !validate_u16_id(raw) {
        return None;
    }
    Some(ZoneId(raw as u16))
}

pub fn loop_id_from_config(config: &EquipmentConfig, keys: &[&str]) -> Option<LoopId> {
    let raw = first_f64(config, keys)?;
    if !validate_u16_id(raw) {
        return None;
    }
    Some(LoopId(raw as u16))
}

pub fn load_stage_values(
    config: &EquipmentConfig,
    scalar_keys: &[&str],
    stage_prefix: &str,
    default_value: f64,
) -> Vec<f64> {
    let mut stages = Vec::new();
    let mut idx = 0usize;
    loop {
        let key = format!("{stage_prefix}_{idx}");
        let Some(value) = config.get_f64(&key) else {
            break;
        };
        stages.push(value.max(0.0));
        idx += 1;
    }

    if stages.is_empty() {
        let scalar = first_f64(config, scalar_keys).unwrap_or(default_value);
        vec![scalar.max(0.0)]
    } else {
        stages
    }
}

pub fn lookup_zone(
    env: &EnvironmentState,
    zone_id: ZoneId,
) -> crate::Result<&hares_types::ZoneState> {
    env.zones
        .iter()
        .find(|zone| zone.id == zone_id)
        .ok_or_else(|| HaresError::Equipment(format!("zone {zone_id:?} not found")))
}

pub fn equipment_id_from_config(config: &EquipmentConfig) -> crate::Result<u32> {
    let Some(raw) = config.get_f64("equipment_id") else {
        return Ok(0);
    };
    if !raw.is_finite() || raw < 0.0 || raw.fract() != 0.0 || raw > u32::MAX as f64 {
        return Err(HaresError::Equipment(format!(
            "invalid equipment_id value {raw}"
        )));
    }
    Ok(raw as u32)
}

pub fn parse_fuel_type(raw: Option<&str>) -> Option<FuelType> {
    match raw?.trim().to_ascii_lowercase().as_str() {
        "gas" | "natural_gas" | "natural gas" => Some(FuelType::Gas),
        "propane" => Some(FuelType::Propane),
        "oil" | "fuel_oil" | "fuel oil" => Some(FuelType::Oil),
        _ => None,
    }
}

pub fn parse_fluid_type(raw: Option<&str>) -> Option<FluidType> {
    match raw?.trim().to_ascii_lowercase().as_str() {
        "water" => Some(FluidType::Water),
        "glycol" => Some(FluidType::Glycol),
        "refrigerant" => Some(FluidType::Refrigerant),
        _ => None,
    }
}

pub fn operating_mode_code(mode: OperatingMode) -> f64 {
    match mode {
        OperatingMode::Off => 0.0,
        OperatingMode::Heating => 1.0,
        OperatingMode::Cooling => 2.0,
        OperatingMode::HeatingHP => 3.0,
        OperatingMode::HeatingHPAndER => 4.0,
        OperatingMode::HeatingER => 5.0,
        OperatingMode::HeatPumpWH => 6.0,
        OperatingMode::BackupElement => 7.0,
        OperatingMode::Defrost => 8.0,
        OperatingMode::Standby => 9.0,
        OperatingMode::Charging => 10.0,
        OperatingMode::Discharging => 11.0,
    }
}

/// Shared `update_control` logic for simple heating equipment.
///
/// Runs the thermostat FSM and returns the resulting `OperatingMode`.
/// On a `Heating` call the duty cycle is set to 1.0 for cycling (on/off) mode,
/// or preserved from an ideal-capacity solver for coarse timesteps (>=300 s) or
/// when `use_ideal_capacity` is configured. All other modes set duty to 0.0 and
/// return `Off`.
#[allow(dead_code)]
pub fn update_heating_control(hvac: &mut HvacEquipment, env: &EnvironmentState) -> OperatingMode {
    match hvac.update_mode(env) {
        Ok(super::thermostat::ThermostatMode::Heating) => {
            if !hvac.use_ideal_capacity(env) {
                hvac.duty_cycle = 1.0;
            } else {
                hvac.duty_cycle = hvac.duty_cycle.clamp(0.0, 1.0);
            }
            OperatingMode::Heating
        }
        _ => {
            hvac.duty_cycle = 0.0;
            OperatingMode::Off
        }
    }
}

/// Shared `apply_control_unchecked` logic for heating equipment that uses
/// `ThermalSetpoint` signals and delegates into `HvacEquipment`.
pub fn apply_heating_control_unchecked(
    hvac: &mut HvacEquipment,
    signal: &ControlSignal,
    equipment_name: &str,
) -> crate::Result<()> {
    hvac.apply_control_signal(signal);
    if let ControlSignal::ThermalSetpoint {
        deadband_c: Some(deadband_c),
        ..
    } = signal
    {
        if !deadband_c.is_finite() || *deadband_c < 0.0 {
            return Err(HaresError::Control(format!(
                "invalid deadband_c for {equipment_name}: {deadband_c}"
            )));
        }
        hvac.thermostat.hysteresis_c = *deadband_c;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use hares_types::OperatingMode;

    use super::operating_mode_code;

    #[test]
    fn operating_mode_code_covers_all_variants() {
        // Each variant must map to a unique, non-negative code.
        // Codes must match the values used in heater.rs constants and OCHRE telemetry.
        let cases: &[(OperatingMode, f64)] = &[
            (OperatingMode::Off, 0.0),
            (OperatingMode::Heating, 1.0),
            (OperatingMode::Cooling, 2.0),
            (OperatingMode::HeatingHP, 3.0),
            (OperatingMode::HeatingHPAndER, 4.0),
            (OperatingMode::HeatingER, 5.0),
            (OperatingMode::HeatPumpWH, 6.0),
            (OperatingMode::BackupElement, 7.0),
            (OperatingMode::Defrost, 8.0),
            (OperatingMode::Standby, 9.0),
            (OperatingMode::Charging, 10.0),
            (OperatingMode::Discharging, 11.0),
        ];

        for &(mode, expected) in cases {
            let got = operating_mode_code(mode);
            assert_eq!(
                got, expected,
                "{mode:?} must map to code {expected}, got {got}"
            );
            assert!(got >= 0.0, "{mode:?} must not return a negative code");
        }

        // All codes must be distinct (no two modes share a telemetry code).
        let codes: Vec<f64> = cases.iter().map(|&(m, _)| operating_mode_code(m)).collect();
        for i in 0..codes.len() {
            for j in (i + 1)..codes.len() {
                assert_ne!(
                    codes[i], codes[j],
                    "modes {:?} and {:?} must have distinct codes",
                    cases[i].0, cases[j].0
                );
            }
        }
    }
}
