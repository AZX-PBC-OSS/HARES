//! Shared helper utilities for HVAC equipment implementations.
//!
//! # Config access policy
//!
//! `get_f64`, `get_str`, `get_bool`, and `first_f64` must only be called from
//! custom equipment or the Python adapter layer. Built-in equipment must use
//! typed config structs (CFG-007 through CFG-015). CI enforces this via the
//! `check-no-magic-config` Makefile target once all built-in equipment has been
//! migrated.

use hares_types::normalize_ascii;
use hares_types::{
    ControlSignal, EnvironmentState, FuelType, HaresError, LoopId, OperatingMode, ZoneId,
};

use crate::{
    ConfigPayload, EquipmentConfig,
};

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
    let raw = config
        .get_f64("zone_id")
        .or_else(|| typed_f64(config, "zone_id"))?;
    if !validate_u16_id(raw) {
        return None;
    }
    Some(ZoneId(raw as u16))
}

/// Parse an optional `ZoneId` from a named config key.
pub fn parse_zone_id_key(config: &EquipmentConfig, key: &str) -> Option<ZoneId> {
    let raw = config.get_f64(key).or_else(|| typed_f64(config, key))?;
    if !validate_u16_id(raw) {
        return None;
    }
    Some(ZoneId(raw as u16))
}

pub fn loop_id_from_config(config: &EquipmentConfig, keys: &[&str]) -> Option<LoopId> {
    let raw = first_f64(config, keys).or_else(|| typed_first_f64(config, keys))?;
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
    let Some(raw) = config
        .get_f64("equipment_id")
        .or_else(|| typed_f64(config, "equipment_id"))
    else {
        return Ok(0);
    };
    if !raw.is_finite() || raw < 0.0 || raw.fract() != 0.0 || raw > u32::MAX as f64 {
        return Err(HaresError::Equipment(format!(
            "invalid equipment_id value {raw}"
        )));
    }
    Ok(raw as u32)
}

fn typed_f64(config: &EquipmentConfig, key: &str) -> Option<f64> {
    match &config.payload {
        ConfigPayload::Typed { data, .. } => data.get(key).and_then(|v| v.as_f64()),
        ConfigPayload::Raw { .. } => None,
    }
}

fn typed_first_f64(config: &EquipmentConfig, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| typed_f64(config, key))
}

pub fn parse_fuel_type(raw: Option<&str>) -> Option<FuelType> {
    match normalize_ascii(raw?).as_str() {
        "electric" | "electricity" | "elec" => Some(FuelType::Electric),
        "gas" | "natural_gas" | "natural gas" => Some(FuelType::Gas),
        "propane" => Some(FuelType::Propane),
        "oil" | "fuel_oil" | "fuel oil" => Some(FuelType::Oil),
        "none" | "no_fuel" | "no fuel" => Some(FuelType::None),
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

/// Resolve duct DSE from equipment config.
///
/// Checks for a direct `duct_dse` override first, then builds ASHRAE 152
/// inputs from raw duct parameters and computes DSE dynamically.
///
/// `capacity_low_w` and `fan_flow_low_m3_s` must be `Some` for multi-speed
/// systems so the ASHRAE 152 low-speed branch uses actual low-speed values.
/// Passing `None` for a multi-speed system causes the low-speed branch to fall
/// back to high-speed values, which underestimates duct losses.
pub fn resolve_duct_dse(
    config: &EquipmentConfig,
    is_heating: bool,
    capacity_w: f64,
    fan_flow_m3_s: f64,
    n_speeds: u8,
    capacity_low_w: Option<f64>,
    fan_flow_low_m3_s: Option<f64>,
    is_heat_pump: bool,
) -> f64 {
    // Direct override takes priority.
    if let Some(dse) = first_f64(config, DUCT_DSE_KEYS) {
        return dse.clamp(0.0, 1.0);
    }

    // Check for raw duct params from ASHRAE 152 passthrough.
    let Some(zone_type_str) = config.get_str("duct_zone_type") else {
        return 1.0;
    };
    let Some(zone_type) = parse_ashrae152_zone_type(zone_type_str) else {
        return 1.0;
    };

    let lat = config.get_f64("duct_latitude_deg").unwrap_or(40.0);
    let lon = config.get_f64("duct_longitude_deg").unwrap_or(-100.0);
    let house_vol = config.get_f64("duct_house_volume_m3").unwrap_or(400.0);
    let supply_leak = config
        .get_f64("duct_supply_leakage_frac")
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    let supply_area = config.get_f64("duct_supply_area_m2").unwrap_or(0.0);
    let supply_r = config.get_f64("duct_supply_r_m2_k_w").unwrap_or(0.0);
    let return_leak = config
        .get_f64("duct_return_leakage_frac")
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    let return_area = config.get_f64("duct_return_area_m2").unwrap_or(0.0);
    let return_r = config.get_f64("duct_return_r_m2_k_w").unwrap_or(0.0);

    // Need positive capacity and fan flow for meaningful DSE calculation.
    if capacity_w <= 0.0 || fan_flow_m3_s <= 0.0 {
        return 1.0;
    }

    let input = hares_physics::ashrae152::DuctDseInput {
        zone_type,
        latitude_deg: lat,
        longitude_deg: lon,
        house_volume_m3: house_vol,
        supply_leakage_frac: supply_leak,
        supply_area_m2: supply_area,
        supply_r_nominal_m2_k_w: supply_r,
        return_leakage_frac: return_leak,
        return_area_m2: return_area,
        return_r_nominal_m2_k_w: return_r,
        is_heating,
        capacity_w,
        fan_flow_m3_s,
        n_speeds,
        capacity_low_w,
        fan_flow_low_m3_s,
        is_heat_pump,
    };

    hares_physics::ashrae152::calculate_dse(&input)
}

fn parse_ashrae152_zone_type(s: &str) -> Option<hares_physics::ashrae152::Ashrae152ZoneType> {
    use hares_physics::ashrae152::Ashrae152ZoneType;
    match s {
        "attic_vented" => Some(Ashrae152ZoneType::AtticVented),
        "attic_vented_radiant_barrier" => Some(Ashrae152ZoneType::AtticVentedRadiantBarrier),
        "attic_unvented" => Some(Ashrae152ZoneType::AtticUnvented),
        "attic_unvented_radiant_barrier" => Some(Ashrae152ZoneType::AtticUnventedRadiantBarrier),
        "garage" => Some(Ashrae152ZoneType::Garage),
        "unvent_unins_crawlspace" => Some(Ashrae152ZoneType::UnventUninsulatedCrawlspace),
        "unvent_crawlspace_ins_floor_wall" => Some(Ashrae152ZoneType::UnventCrawlspaceInsFloorWall),
        "unvent_crawlspace_ins_floor" => Some(Ashrae152ZoneType::UnventCrawlspaceInsFloor),
        "vent_unins_crawlspace" => Some(Ashrae152ZoneType::VentUninsulatedCrawlspace),
        "vent_crawlspace_ins_floor_wall" => Some(Ashrae152ZoneType::VentCrawlspaceInsFloorWall),
        "vent_crawlspace_ins_floor" => Some(Ashrae152ZoneType::VentCrawlspaceInsFloor),
        "unins_basement" => Some(Ashrae152ZoneType::UninsulatedBasement),
        "basement_ins_walls" => Some(Ashrae152ZoneType::BasementInsWalls),
        "basement_ins_ceiling" => Some(Ashrae152ZoneType::BasementInsCeiling),
        "under_slab" => Some(Ashrae152ZoneType::UnderSlab),
        "ext_walls" => Some(Ashrae152ZoneType::ExteriorWalls),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use hares_types::{FuelType, OperatingMode};

    use crate::{
        EquipmentConfig,
        hvac::heating_config::{ElectricBoilerConfig, GasFurnaceConfig},
    };

    use super::{
        equipment_id_from_config, loop_id_from_config, operating_mode_code, parse_fuel_type,
        zone_id_from_config,
    };

    #[test]
    fn parse_fuel_type_covers_all_variants() {
        let cases: &[(&str, FuelType)] = &[
            ("electric", FuelType::Electric),
            ("electricity", FuelType::Electric),
            ("Electric", FuelType::Electric),
            ("ELECTRICITY", FuelType::Electric),
            ("gas", FuelType::Gas),
            ("natural_gas", FuelType::Gas),
            ("natural gas", FuelType::Gas),
            ("Gas", FuelType::Gas),
            ("propane", FuelType::Propane),
            ("Propane", FuelType::Propane),
            ("oil", FuelType::Oil),
            ("fuel_oil", FuelType::Oil),
            ("fuel oil", FuelType::Oil),
            ("none", FuelType::None),
            ("no_fuel", FuelType::None),
            ("no fuel", FuelType::None),
        ];

        for &(input, expected) in cases {
            let got = parse_fuel_type(Some(input));
            assert_eq!(
                got,
                Some(expected),
                "parse_fuel_type({input:?}) should be {expected:?}, got {got:?}"
            );
        }

        assert_eq!(parse_fuel_type(None), None, "None input should return None");
        assert_eq!(
            parse_fuel_type(Some("unknown_fuel")),
            None,
            "unknown fuel should return None"
        );
    }

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

    #[test]
    fn typed_configs_preserve_identity_fields_for_helper_accessors() {
        let gas_furnace = EquipmentConfig::from_typed(
            "GF".to_string(),
            "Gas Furnace".to_string(),
            GasFurnaceConfig {
                equipment_id: Some(42),
                zone_id: Some(7),
                capacity_w: 10_000.0,
                afue: 0.96,
                ..GasFurnaceConfig::default()
            },
        );
        assert_eq!(equipment_id_from_config(&gas_furnace).unwrap(), 42);
        assert_eq!(zone_id_from_config(&gas_furnace), Some(hares_types::ZoneId(7)));

        let boiler = EquipmentConfig::from_typed(
            "EB".to_string(),
            "Electric Boiler".to_string(),
            ElectricBoilerConfig {
                equipment_id: Some(11),
                zone_id: Some(3),
                loop_id: Some(9),
                capacity_w: 8_000.0,
                eir: 1.0,
                ..ElectricBoilerConfig::default()
            },
        );
        assert_eq!(
            loop_id_from_config(&boiler, &["loop_id", "hydronic_loop_id"]),
            Some(hares_types::LoopId(9))
        );
    }
}
