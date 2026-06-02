//! Shared helper utilities for HVAC equipment implementations.
//!
//! # Config access policy
//!
//! `get_f64`, `get_str`, `get_bool`, and `first_f64` must not appear in built-in
//! equipment `init()` bodies. Built-in equipment uses typed config structs;
//! these accessors remain available only for compatibility helpers and
//! custom-equipment adapter paths outside init.

use hares_types::normalize_ascii;
use hares_types::{
    ControlSignal, EnvironmentState, FuelType, HaresError, LoopId, OperatingMode, ZoneId,
};

use crate::{ConfigPayload, EquipmentConfig};

use super::hvac_core::MAX_CONDITIONED_ZONE_TEMP_C;

// Re-exported from crate root; used by `apply_heating_control_unchecked`
// and `update_heating_control`.
use crate::HvacEquipment;

#[doc(hidden)]
/// Compatibility helper for legacy/raw config paths outside built-in `init()`
/// bodies. Do not use this in new built-in equipment initialization code.
pub fn first_f64(config: &EquipmentConfig, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| config.get_f64(key))
}

fn validate_u16_id(raw: f64) -> bool {
    raw.is_finite() && raw >= 0.0 && raw.fract() == 0.0 && raw <= u16::MAX as f64
}

pub(super) fn validate_zone_id(raw: f64) -> bool {
    raw.is_finite() && raw >= 1.0 && raw.fract() == 0.0 && raw <= u16::MAX as f64
}

pub fn zone_id_from_config(config: &EquipmentConfig) -> Option<ZoneId> {
    let raw = config
        .get_f64(crate::config::KEY_ZONE_ID)
        .or_else(|| typed_f64(config, crate::config::KEY_ZONE_ID))?;
    if raw == 0.0 {
        tracing::warn!(
            zone_id = raw,
            "zone_id=0 is not a valid thermal zone; zones are 1-indexed, rejecting"
        );
    }
    if !validate_zone_id(raw) {
        return None;
    }
    Some(ZoneId(raw as u16))
}

/// Resolve `ZoneId` from config, falling back to `ZoneId(1)` when absent.
///
/// Returns `(ZoneId, bool)` where the bool is `true` when `zone_id` was
/// explicitly present in the config and `false` when the fallback was used.
///
/// When `zone_id` is missing from the equipment's typed config, HPXML parsing
/// did not inject the conditioned-zone identifier. Logging the fallback makes
/// this gap visible during development and integration testing.
#[cfg(any(debug_assertions, feature = "check_invariants"))]
pub fn zone_id_from_config_or_default(
    config: &EquipmentConfig,
    equipment_name: &str,
) -> (ZoneId, bool) {
    match zone_id_from_config(config) {
        Some(id) => (id, true),
        None => {
            tracing::warn!(
                equipment = %equipment_name,
                key = crate::config::KEY_ZONE_ID,
                "zone_id not present in equipment config; falling back to ZoneId(1) — \
                 HPXML parsing may not have propagated conditioned_zone_id"
            );
            (ZoneId(1), false)
        }
    }
}

#[cfg(not(any(debug_assertions, feature = "check_invariants")))]
pub fn zone_id_from_config_or_default(
    config: &EquipmentConfig,
    _equipment_name: &str,
) -> (ZoneId, bool) {
    match zone_id_from_config(config) {
        Some(id) => (id, true),
        None => (ZoneId(1), false),
    }
}

/// Parse an optional `ZoneId` from a named config key.
pub fn parse_zone_id_key(config: &EquipmentConfig, key: &str) -> Option<ZoneId> {
    let raw = config.get_f64(key).or_else(|| typed_f64(config, key))?;
    if !validate_zone_id(raw) {
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
        "oil" | "fuel_oil" | "fuel oil" | "fuel oil 1" | "fuel oil 2" | "fuel oil 4"
        | "fuel oil 5/6" | "kerosene" | "diesel" => Some(FuelType::Oil),
        "wood" => Some(FuelType::Wood),
        "wood pellets" | "wood_pellets" => Some(FuelType::WoodPellet),
        "coal" | "anthracite coal" | "anthracite_coal" | "bituminous coal" | "bituminous_coal"
        | "coke" => Some(FuelType::Coal),
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
        OperatingMode::On => 12.0,
    }
}

/// Shared `update_control` logic for simple heating equipment.
///
/// Runs the thermostat FSM and returns the resulting `OperatingMode`.
/// On a `Heating` call the duty cycle is set to 1.0 for cycling (on/off) mode,
/// or preserved from an ideal-capacity solver for coarse timesteps (>=300 s) or
/// when `use_ideal_capacity` is configured. All other modes set duty to 0.0 and
/// return `Off`.
pub fn update_heating_control(hvac: &mut HvacEquipment, env: &EnvironmentState) -> OperatingMode {
    // Safety cutoff: prevent simulation runaway where zone temperatures
    // reach physically impossible levels (e.g. 49.5 °C indoors in January).
    // Only the served (conditioned) zone is checked; duct zones in attics
    // or garages are not subject to this limit because they can legitimately
    // reach high temperatures without heating equipment running.
    if let Ok(zone) = lookup_zone(env, hvac.config.zone_id) {
        if zone.temperature_c > MAX_CONDITIONED_ZONE_TEMP_C {
            tracing::warn!(
                zone_temp_c = zone.temperature_c,
                equipment_type = ?hvac.config.equipment_type,
                zone_id = hvac.config.zone_id.0,
                max_safe_temp = MAX_CONDITIONED_ZONE_TEMP_C,
                "Safety cutoff: conditioned zone temperature exceeds max safe limit; forcing heating equipment Off"
            );
            hvac.runtime.duty_cycle = 0.0;
            return OperatingMode::Off;
        }
    }

    // Apply DR setpoint offset (set by apply_simple_mode_override_in_control)
    // as a temporary override so the thermostat FSM sees the lowered setpoint.
    // runtime_setpoints is saved and restored because it is also used by
    // ThermalSetpoint/ThermalSetpointDelta signals; this avoids cross-talk
    // where a DR Normal would otherwise clear an active ThermalSetpoint.
    let saved_runtime_setpoints = hvac.thermostat_fsm.runtime_setpoints;
    if hvac.runtime.dr_setpoint_offset_c != 0.0 {
        let base = hvac
            .thermostat_fsm
            .static_setpoints
            .with_schedule_override(hvac.thermostat_fsm.schedule_setpoints);
        hvac.thermostat_fsm.runtime_setpoints = Some(super::RuntimeSetpointOverride {
            heating_c: Some(base.heating_c + hvac.runtime.dr_setpoint_offset_c),
            cooling_c: None,
        });
    }

    let result = match hvac.update_mode(env) {
        Ok(super::thermostat::ThermostatMode::Heating) => {
            if !hvac.use_ideal_capacity(env) {
                hvac.runtime.duty_cycle = 1.0;
            } else {
                hvac.runtime.duty_cycle = hvac.runtime.duty_cycle.clamp(0.0, 1.0);
            }
            OperatingMode::Heating
        }
        _ => {
            if !hvac.use_ideal_capacity(env) {
                hvac.runtime.duty_cycle = 0.0;
            }
            OperatingMode::Off
        }
    };

    hvac.thermostat_fsm.runtime_setpoints = saved_runtime_setpoints;
    result
}

/// Shared `apply_control_unchecked` logic for heating equipment that uses
pub fn apply_heating_control_unchecked(
    hvac: &mut HvacEquipment,
    signal: &ControlSignal,
    equipment_name: &str,
) -> crate::Result<()> {
    hvac.apply_control_signal(signal)?;
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
        hvac.thermostat_fsm.thermostat.hysteresis_c = *deadband_c;
    }
    Ok(())
}

/// Apply solver-driven ideal heating capacity for simple heating equipment.
///
/// `capacity_w` is interpreted as delivered heating capacity [W]. The control is
/// converted to a duty-cycle fraction against the rated thermal capacity.
pub fn apply_simple_heating_ideal_capacity_control(
    hvac: &mut HvacEquipment,
    signal: &ControlSignal,
    rated_capacity_w: f64,
) {
    if let ControlSignal::IdealCapacity { capacity_w } = signal {
        let duty = if rated_capacity_w > 0.0 {
            capacity_w.max(0.0) / rated_capacity_w
        } else {
            0.0
        };
        hvac.thermostat_fsm.thermostat.use_ideal_capacity = true;
        hvac.runtime.duty_cycle = duty.clamp(0.0, 1.0);
    }
}

/// Apply `ModeOverride` and `DemandResponse` signals for simple heating-only
/// equipment. Returns `true` when the signal was consumed; the caller should
/// not forward it to `apply_heating_control_unchecked`.
pub fn apply_simple_mode_override_and_dr(
    mode_override: &mut Option<OperatingMode>,
    dr_level: &mut hares_types::DRLevel,
    signal: &ControlSignal,
    equipment_name: &str,
) -> crate::Result<bool> {
    match signal {
        ControlSignal::ModeOverride { mode } => {
            *mode_override = Some(*mode);
            tracing::debug!(
                equipment = equipment_name,
                mode = ?mode,
                "ModeOverride applied"
            );
            Ok(true)
        }
        ControlSignal::DemandResponse {
            level,
            duration_s: _, // Discarded: simple heating equipment does not maintain a
                           // step clock; callers must send an explicit Normal to cancel.
        } => {
            *dr_level = *level;
            tracing::debug!(
                equipment = equipment_name,
                level = ?level,
                "DemandResponse applied"
            );
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Apply ModeOverride and DemandResponse effects in `update_control` for simple
/// heating-only equipment.
///
/// Returns `Some(mode)` when a control override took effect (the caller should
/// return that mode immediately). Returns `None` if normal thermostat control
/// should proceed. When `Some(Off)` is returned, the caller should also zero
/// the duty cycle.
pub fn apply_simple_mode_override_in_control(
    hvac: &mut HvacEquipment,
    mode_override: &mut Option<OperatingMode>,
    dr_level: hares_types::DRLevel,
    equipment_name: &str,
) -> Option<OperatingMode> {
    // ModeOverride takes priority over all thermostat and DR control.
    if let Some(mode) = *mode_override {
        match mode {
            OperatingMode::Off => {
                hvac.runtime.duty_cycle = 0.0;
                tracing::debug!(
                    equipment = equipment_name,
                    "ModeOverride Off: forcing equipment off"
                );
                return Some(OperatingMode::Off);
            }
            OperatingMode::Heating | OperatingMode::On => {
                hvac.runtime.duty_cycle = 1.0;
                hvac.thermostat_fsm.mode = super::ThermostatMode::Heating;
                tracing::debug!(
                    equipment = equipment_name,
                    mode = ?mode,
                    "ModeOverride: forcing equipment to Heating at full duty"
                );
                return Some(OperatingMode::Heating);
            }
            _ => { /* unrecognised modes fall through to normal control */ }
        }
    }

    // DemandResponse curtailing: GridEmergency forces equipment off.
    if dr_level == hares_types::DRLevel::GridEmergency {
        hvac.runtime.duty_cycle = 0.0;
        tracing::debug!(
            equipment = equipment_name,
            "DemandResponse GridEmergency: forcing equipment off"
        );
        return Some(OperatingMode::Off);
    }

    // DR setpoint offset: lower the effective heating setpoint to curtail.
    let dr_offset_c = match dr_level {
        hares_types::DRLevel::Normal => 0.0,
        hares_types::DRLevel::Moderate => -1.0,
        hares_types::DRLevel::High => -2.0,
        hares_types::DRLevel::Critical => -3.0,
        hares_types::DRLevel::GridEmergency => 0.0, // handled above
    };

    hvac.runtime.dr_setpoint_offset_c = dr_offset_c;

    None
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
pub struct DuctDseContext {
    pub is_heating: bool,
    pub capacity_w: f64,
    pub fan_flow_m3_s: f64,
    pub n_speeds: u8,
    pub capacity_low_w: Option<f64>,
    pub fan_flow_low_m3_s: Option<f64>,
    pub is_heat_pump: bool,
}

pub fn resolve_duct_dse(config: &EquipmentConfig, ctx: &DuctDseContext) -> f64 {
    // Direct override takes priority.
    if let Some(dse) = first_f64(config, &["duct_dse", "duct_distribution_efficiency"]) {
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
    if ctx.capacity_w <= 0.0 || ctx.fan_flow_m3_s <= 0.0 {
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
        is_heating: ctx.is_heating,
        capacity_w: ctx.capacity_w,
        fan_flow_m3_s: ctx.fan_flow_m3_s,
        n_speeds: ctx.n_speeds,
        capacity_low_w: ctx.capacity_low_w,
        fan_flow_low_m3_s: ctx.fan_flow_low_m3_s,
        is_heat_pump: ctx.is_heat_pump,
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
        parse_zone_id_key, zone_id_from_config, zone_id_from_config_or_default,
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
            ("wood", FuelType::Wood),
            ("Wood", FuelType::Wood),
            ("WOOD", FuelType::Wood),
            ("coal", FuelType::Coal),
            ("Coal", FuelType::Coal),
            ("anthracite coal", FuelType::Coal),
            ("anthracite_coal", FuelType::Coal),
            ("bituminous coal", FuelType::Coal),
            ("bituminous_coal", FuelType::Coal),
            ("coke", FuelType::Coal),
            ("wood pellets", FuelType::WoodPellet),
            ("wood_pellets", FuelType::WoodPellet),
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
            (OperatingMode::On, 12.0),
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
        assert_eq!(
            zone_id_from_config(&gas_furnace),
            Some(hares_types::ZoneId(7))
        );

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

    #[test]
    fn zone_id_0_rejected_by_zone_id_from_config() {
        let config = EquipmentConfig::from_typed(
            "Z0".to_string(),
            "Gas Furnace".to_string(),
            GasFurnaceConfig {
                zone_id: Some(0),
                capacity_w: 10_000.0,
                afue: 0.96,
                ..GasFurnaceConfig::default()
            },
        );
        assert_eq!(
            zone_id_from_config(&config),
            None,
            "zone_id=0 must be rejected (zones are 1-indexed)"
        );
    }

    #[test]
    fn zone_id_1_accepted_by_zone_id_from_config() {
        let config = EquipmentConfig::from_typed(
            "Z1".to_string(),
            "Gas Furnace".to_string(),
            GasFurnaceConfig {
                zone_id: Some(1),
                capacity_w: 10_000.0,
                afue: 0.96,
                ..GasFurnaceConfig::default()
            },
        );
        assert_eq!(
            zone_id_from_config(&config),
            Some(hares_types::ZoneId(1)),
            "zone_id=1 must be accepted"
        );
    }

    #[test]
    fn zone_id_0_rejected_by_parse_zone_id_key() {
        let config = EquipmentConfig::from_typed(
            "Z0".to_string(),
            "Gas Furnace".to_string(),
            GasFurnaceConfig {
                zone_id: Some(0),
                capacity_w: 10_000.0,
                afue: 0.96,
                ..GasFurnaceConfig::default()
            },
        );
        assert_eq!(
            parse_zone_id_key(&config, "zone_id"),
            None,
            "parse_zone_id_key must reject zone_id=0"
        );
    }

    #[test]
    fn loop_id_0_still_accepted_by_loop_id_from_config() {
        let config = EquipmentConfig::from_typed(
            "L0".to_string(),
            "Electric Boiler".to_string(),
            ElectricBoilerConfig {
                loop_id: Some(0),
                zone_id: Some(1),
                capacity_w: 8_000.0,
                eir: 1.0,
                ..ElectricBoilerConfig::default()
            },
        );
        assert_eq!(
            loop_id_from_config(&config, &["loop_id", "hydronic_loop_id"]),
            Some(hares_types::LoopId(0)),
            "loop_id=0 must still be accepted (validate_u16_id permits 0)"
        );
    }

    #[test]
    fn zone_id_none_when_zone_id_key_absent_from_config() {
        let config = EquipmentConfig::from_typed(
            "ZN".to_string(),
            "Gas Furnace".to_string(),
            GasFurnaceConfig {
                zone_id: None,
                capacity_w: 10_000.0,
                afue: 0.96,
                ..GasFurnaceConfig::default()
            },
        );
        assert_eq!(
            zone_id_from_config(&config),
            None,
            "zone_id_from_config must return None when zone_id key is absent from the typed config"
        );
    }

    #[test]
    fn zone_id_from_config_or_default_falls_back_to_zone_1_with_explicit_false() {
        let config = EquipmentConfig::from_typed(
            "ZN".to_string(),
            "Gas Furnace".to_string(),
            GasFurnaceConfig {
                zone_id: None,
                capacity_w: 10_000.0,
                afue: 0.96,
                ..GasFurnaceConfig::default()
            },
        );
        let (zone_id, explicit) = zone_id_from_config_or_default(&config, &config.name);
        assert_eq!(
            zone_id,
            hares_types::ZoneId(1),
            "fallback must be ZoneId(1) when zone_id key is absent"
        );
        assert!(
            !explicit,
            "zone_id_explicit must be false when fallback is used"
        );
    }

    #[test]
    fn zone_id_from_config_or_default_returns_explicit_true_when_present() {
        let config = EquipmentConfig::from_typed(
            "Z5".to_string(),
            "Gas Furnace".to_string(),
            GasFurnaceConfig {
                zone_id: Some(5),
                capacity_w: 10_000.0,
                afue: 0.96,
                ..GasFurnaceConfig::default()
            },
        );
        let (zone_id, explicit) = zone_id_from_config_or_default(&config, &config.name);
        assert_eq!(
            zone_id,
            hares_types::ZoneId(5),
            "zone_id must be ZoneId(5) when present in config"
        );
        assert!(
            explicit,
            "zone_id_explicit must be true when zone_id is present in config"
        );
    }
}
