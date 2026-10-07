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
    ControlSignal, EnvironmentState, FuelType, HaresError, LoopId, OperatingMode, PortDeclaration,
    Telemetry, ZoneId,
};

use crate::{ConfigPayload, EquipmentConfig};

use super::equivalent_battery::EquivalentBatteryWindow;
use super::hvac_core::MAX_CONDITIONED_ZONE_TEMP_C;
use super::thermostat::lookup_zone_temp;

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

/// The zone a unit that conditions the dwelling's own air serves: the
/// config's zone_id, else the zone the unit already carries, else the
/// dwelling zone map's indoor (conditioned) zone. A unit with none of these
/// has no zone to condition: a typed error naming it.
pub(crate) fn resolve_served_zone(
    config: &EquipmentConfig,
    current: Option<ZoneId>,
) -> crate::Result<ZoneId> {
    zone_id_from_config(config)
        .or(current)
        .or_else(|| {
            config
                .zone_map
                .as_ref()
                .and_then(|map| map.get(hares_types::ZoneRole::Indoor))
        })
        .ok_or_else(|| {
            HaresError::Equipment(format!(
                "{}: no zone_id and no conditioned zone in the dwelling zone map; \
                 the unit has no zone to serve",
                config.name
            ))
        })
}

/// `leading` followed by the served zone's thermal (and, for units that
/// move moisture, humidity) port; no zone port while the zone is unresolved.
pub(crate) fn served_zone_ports(
    leading: &[PortDeclaration],
    zone: Option<ZoneId>,
    humidity: bool,
) -> Vec<PortDeclaration> {
    let zone_ports = zone.into_iter().flat_map(|zone| {
        std::iter::once(PortDeclaration::thermal(zone))
            .chain(humidity.then(|| PortDeclaration::humidity(zone)))
    });
    leading.iter().copied().chain(zone_ports).collect()
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
        "wood pellets" | "wood_pellets" | "wood pellet" | "woodpellet" | "wood_pellet" => {
            Some(FuelType::WoodPellet)
        }
        "coal" | "anthracite coal" | "anthracite_coal" | "bituminous coal" | "bituminous_coal"
        | "coke" => Some(FuelType::Coal),
        "none" | "no_fuel" | "no fuel" | "nofuel" => Some(FuelType::None),
        _ => None,
    }
}

/// Grid-outage gate shared by all HVAC `update_control` implementations
/// (water-heater precedent: force off at the **root of dispatch**, never by
/// zeroing only the reported draw).
///
/// Returns `true` and zeroes the duty cycle when the home bus is
/// de-energized ([`hares_types::GridState::bus_energized`] is `false`):
/// no supply power exists, so the equipment cannot run regardless of
/// thermostat calls, `ModeOverride`, or DR state — callers must place this
/// check *before* mode-override handling and return `OperatingMode::Off`.
/// Battery-/generator-backed (islanded) homes keep an energized bus, so
/// this gate stays open for them by construction.
///
/// Timers and thermostat hysteresis resume normally once power returns;
/// callers with compressor off-timers must advance them as for any forced
/// off (mirroring the heat-pump water heater's outage behaviour).
pub fn outage_forces_off(hvac: &mut HvacEquipment, env: &EnvironmentState) -> bool {
    if env.grid.bus_energized() {
        return false;
    }
    hvac.runtime.duty_cycle = 0.0;
    true
}

/// Floor for the band span a cycling unit's runtime fraction divides by, so
/// a narrow hysteresis does not saturate the fraction a hair's breadth past
/// the release edge.
pub const MIN_LOAD_FRACTION_DEADBAND_C: f64 = 0.5;

/// The runtime fraction of rated capacity a cycling unit delivers within the
/// step: the zone's position between the thermostat's release edge (fraction
/// 0) and its full-capacity edge (fraction 1), clamped to [0, 1].
///
/// A cycling unit no longer runs whole steps at full capacity or off: it
/// delivers within the step the fraction of capacity the zone needs to reach
/// and hold the setpoint band, and its electric or fuel input follows the
/// runtime fraction with the part-load degradation applied where the
/// reference applies one (EnergyPlus `DXCoils.cc:9859`:
/// `CoolingCoilRuntimeFraction = PartLoadRatio / PLF`, the PLF curve clamped
/// to [0.7, 1]; fuel and resistance coils scale their delivered load and
/// energy by the part-load ratio per `HeatingCoils.cc:1873-1874`). OCHRE
/// runs the same part-load delivery through duty-cycle control at sub-hourly
/// resolution (HVAC.py:315-317, 328-333).
///
/// `released_c` is the threshold the unit's mode releases at (the heating
/// turn-off, the cooling turn-off) and `full_c` the one it turns on at (the
/// heating turn-on, the cooling turn-on): one formula for both axes, the
/// band signed so the fraction runs 0 at the release edge to 1 at the call
/// edge. The span is held to at least [`MIN_LOAD_FRACTION_DEADBAND_C`] so a
/// narrow hysteresis modulates over the same 0.5 C the wider bands do.
#[must_use]
pub fn cycling_load_fraction(zone_temp_c: f64, released_c: f64, full_c: f64) -> f64 {
    let band = full_c - released_c;
    if band.abs() <= f64::EPSILON {
        // A degenerate band resolves no fraction; the thresholds are
        // validated to at least MIN_THERMOSTAT_BAND_C apart at construction.
        return 0.0;
    }
    let span = band.signum() * band.abs().max(MIN_LOAD_FRACTION_DEADBAND_C);
    ((zone_temp_c - released_c) / span).clamp(0.0, 1.0)
}

/// Shared `update_control` logic for simple heating equipment.
///
/// Runs the thermostat FSM and returns the resulting `OperatingMode`.
/// On a `Heating` call a cycling unit's duty cycle is the runtime fraction
/// the zone needs ([`cycling_load_fraction`]); an ideal-capacity unit's duty
/// is preserved for the solver's dispatch to set. All other modes set duty
/// to 0.0 and return `Off`.
pub fn update_heating_control(hvac: &mut HvacEquipment, env: &EnvironmentState) -> OperatingMode {
    // Safety cutoff: prevent simulation runaway where zone temperatures
    // reach physically impossible levels (e.g. 49.5 °C indoors in January).
    // Only the served (conditioned) zone is checked; duct zones in attics
    // or garages are not subject to this limit because they can legitimately
    // reach high temperatures without heating equipment running.
    if let Some(zone_id) = hvac.config.zone_id
        && let Ok(zone) = lookup_zone(env, zone_id)
        && zone.temperature_c > MAX_CONDITIONED_ZONE_TEMP_C
    {
        tracing::warn!(
            zone_temp_c = zone.temperature_c,
            equipment_type = ?hvac.config.equipment_type,
            zone_id = zone_id.0,
            max_safe_temp = MAX_CONDITIONED_ZONE_TEMP_C,
            "Safety cutoff: conditioned zone temperature exceeds max safe limit; forcing heating equipment Off"
        );
        hvac.runtime.duty_cycle = 0.0;
        return OperatingMode::Off;
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
                // Cycling (on/off) mode: deliver the runtime fraction the
                // zone needs to reach and hold the setpoint band.
                let zone_temp_c = hvac
                    .config
                    .zone_id
                    .and_then(|zone| lookup_zone(env, zone).ok())
                    .map(|zone| zone.temperature_c)
                    .unwrap_or(f64::NAN);
                let ((heat_on, heat_off), _) = hvac.thermostat_fsm.band_edges();
                hvac.runtime.duty_cycle = cycling_load_fraction(zone_temp_c, heat_off, heat_on);
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

/// Apply solver-driven ideal heating capacity for simple heating equipment.
///
/// `capacity_w` is interpreted as delivered heating capacity [W]. The control is
/// converted to a duty-cycle fraction against the rated thermal capacity.
pub fn apply_simple_heating_ideal_capacity_control(
    hvac: &mut HvacEquipment,
    signal: &ControlSignal,
    rated_capacity_w: f64,
) {
    if let ControlSignal::IdealCapacity { capacity_w, .. } = signal {
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
/// not forward it to `HvacEquipment::apply_control_signal`.
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

pub fn resolve_duct_dse(config: &EquipmentConfig, ctx: &DuctDseContext) -> crate::Result<f64> {
    // Direct override takes priority.
    if let Some(dse) = first_f64(config, &["duct_dse", "duct_distribution_efficiency"]) {
        return Ok(dse.clamp(0.0, 1.0));
    }

    // Check for raw duct params from ASHRAE 152 passthrough.
    let Some(zone_type_str) = config.get_str("duct_zone_type") else {
        return Ok(1.0);
    };
    let Some(zone_type) = parse_ashrae152_zone_type(zone_type_str) else {
        return Ok(1.0);
    };

    let lat = config.get_f64("duct_latitude_deg").unwrap_or(40.0);
    let lon = config.get_f64("duct_longitude_deg").unwrap_or(-100.0);
    let house_vol = config.get_f64("duct_house_volume_m3").ok_or_else(|| {
        HaresError::Equipment(format!(
            "{}: duct_zone_type is set but duct_house_volume_m3 is not; the duct \
             distribution efficiency needs the conditioned volume",
            config.name
        ))
    })?;
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
        return Ok(1.0);
    }

    let (supply_class, return_class) = zone_type.default_leakage_class(ctx.is_heating);
    let supply_class = if supply_leak == 0.0 {
        Some(supply_class)
    } else {
        None
    };
    let return_class = if return_leak == 0.0 {
        Some(return_class)
    } else {
        None
    };

    let input = hares_physics::ashrae152::DuctDseInput {
        zone_type,
        latitude_deg: lat,
        longitude_deg: lon,
        house_volume_m3: house_vol,
        supply_leakage_frac: supply_leak,
        supply_leakage_class: supply_class,
        supply_area_m2: supply_area,
        supply_r_nominal_m2_k_w: supply_r,
        return_leakage_frac: return_leak,
        return_leakage_class: return_class,
        return_area_m2: return_area,
        return_r_nominal_m2_k_w: return_r,
        is_heating: ctx.is_heating,
        capacity_w: ctx.capacity_w,
        fan_flow_m3_s: ctx.fan_flow_m3_s,
        n_speeds: ctx.n_speeds,
        capacity_low_w: ctx.capacity_low_w,
        fan_flow_low_m3_s: ctx.fan_flow_low_m3_s,
        is_heat_pump: ctx.is_heat_pump,
        burial_depth_m: None,
        soil_conductivity_w_m_k: None,
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

// ── Equivalent Battery Model telemetry helpers ───────────────────────────

/// Register EBM telemetry keys on an equipment's telemetry map.
///
/// Call once during equipment init. Pre-registers all six EBM keys with 0.0
/// so the telemetry output schema is stable regardless of whether the EBM
/// gate (`zone_capacitance_kwh_per_k > 0`) is open at runtime.
pub fn register_ebm_telemetry_keys(telemetry: &mut Telemetry) {
    telemetry.insert(hares_types::telemetry_keys::EBM_EFFICIENCY, 0.0);
    telemetry.insert(hares_types::telemetry_keys::EBM_BASELINE_POWER_KW, 0.0);
    telemetry.insert(hares_types::telemetry_keys::EBM_ENERGY_KWH, 0.0);
    telemetry.insert(hares_types::telemetry_keys::EBM_MIN_ENERGY_KWH, 0.0);
    telemetry.insert(hares_types::telemetry_keys::EBM_MAX_ENERGY_KWH, 0.0);
    telemetry.insert(hares_types::telemetry_keys::EBM_MAX_POWER_KW, 0.0);
}

/// The step's equivalent battery window, or `None` when the zone capacitance
/// (`hvac.config.zone_capacitance_kwh_per_k`, set by the dwelling from the
/// envelope solver) is zero and the model is disabled.
///
/// A step computes it first, before it commits any state, so a failure here
/// leaves the equipment exactly as it was; [`write_ebm_telemetry`] completes
/// it with the step's load once the step has run.
///
/// # Errors
///
/// The window's own errors, and an error when the equipment's zone is not
/// in `env`.
pub(crate) fn step_equivalent_battery(
    hvac: &HvacEquipment,
    env: &EnvironmentState,
) -> crate::Result<Option<EquivalentBatteryWindow>> {
    let cap_kwh = hvac.config.zone_capacitance_kwh_per_k;
    if cap_kwh <= 0.0 {
        return Ok(None);
    }
    let zone_temp_c = lookup_zone_temp(env, hvac.config.served_zone()?)?;
    hvac.equivalent_battery_window(zone_temp_c, cap_kwh, hvac.eir_at_stage(0))
        .map(Some)
}

/// Completes a step's equivalent battery window with the step's load and
/// publishes it; a disabled model leaves the pre-registered 0.0
/// placeholders.
///
/// `capacity_ideal_w` is the thermal output the step delivers [W] (sensible
/// plus latent cooling for an AC, heating for a heater), the EBM's
/// `capacity_ideal` input (OCHRE HVAC.py:640), giving
/// `baseline_power_kw = capacity_ideal_w * rated_eir / 1000`. OCHRE solves
/// the steady-state hold load every step (`solve_ideal_capacity()`,
/// HVAC.py:434-435); the delivered output equals it while the thermostat
/// holds the setpoint and is zero while the unit is off.
///
/// # Errors
///
/// [`EquivalentBatteryWindow::with_load`]'s error for a negative load.
pub(crate) fn write_ebm_telemetry(
    window: Option<EquivalentBatteryWindow>,
    capacity_ideal_w: f64,
    telemetry: &mut Telemetry,
) -> crate::Result<()> {
    use hares_types::telemetry_keys as tk;

    let Some(window) = window else {
        return Ok(());
    };
    let ebm = window.with_load(capacity_ideal_w)?;
    telemetry.set(tk::EBM_EFFICIENCY, ebm.efficiency);
    telemetry.set(tk::EBM_BASELINE_POWER_KW, ebm.baseline_power_kw);
    telemetry.set(tk::EBM_ENERGY_KWH, ebm.energy_kwh.unwrap_or(0.0));
    telemetry.set(tk::EBM_MIN_ENERGY_KWH, ebm.min_energy_kwh);
    telemetry.set(tk::EBM_MAX_ENERGY_KWH, ebm.max_energy_kwh.unwrap_or(0.0));
    telemetry.set(tk::EBM_MAX_POWER_KW, ebm.max_power_kw.unwrap_or(0.0));
    Ok(())
}

#[cfg(test)]
mod tests {
    use hares_types::FuelType;

    use crate::{
        EquipmentConfig,
        hvac::heating_config::{ElectricBoilerConfig, GasFurnaceConfig},
    };

    use crate::config::{ConfigPayload, ConfigValue};

    use super::{
        DuctDseContext, cycling_load_fraction, loop_id_from_config, parse_fuel_type,
        parse_zone_id_key, register_ebm_telemetry_keys, resolve_duct_dse, step_equivalent_battery,
        write_ebm_telemetry, zone_id_from_config,
    };

    /// The fraction runs 0 at the release edge, 1 at the call edge, and
    /// clamps outside; one formula covers both axes (the heating band's
    /// edges arrive descending, the cooling's ascending).
    #[test]
    fn cycling_load_fraction_runs_zero_at_the_release_edge_to_one_at_the_call_edge() {
        // Cooling: release 23.8, full call 24.8.
        assert!((cycling_load_fraction(23.8, 23.8, 24.8)).abs() < 1e-12);
        assert!((cycling_load_fraction(24.3, 23.8, 24.8) - 0.5).abs() < 1e-12);
        assert!((cycling_load_fraction(24.8, 23.8, 24.8) - 1.0).abs() < 1e-12);
        assert!((cycling_load_fraction(26.0, 23.8, 24.8) - 1.0).abs() < 1e-12);
        assert!((cycling_load_fraction(23.0, 23.8, 24.8)).abs() < 1e-12);
        // Heating: release 20.2 (descending to the call edge 19.2).
        assert!((cycling_load_fraction(20.2, 20.2, 19.2)).abs() < 1e-12);
        assert!((cycling_load_fraction(19.7, 20.2, 19.2) - 0.5).abs() < 1e-12);
        assert!((cycling_load_fraction(19.0, 20.2, 19.2) - 1.0).abs() < 1e-12);
        // A narrow hysteresis modulates over the floored 0.5 C span.
        assert!((cycling_load_fraction(24.2, 23.98, 24.08) - 0.44).abs() < 1e-12);
    }

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
            ("wood pellet", FuelType::WoodPellet),
            ("woodpellet", FuelType::WoodPellet),
            ("wood_pellet", FuelType::WoodPellet),
            ("none", FuelType::None),
            ("no_fuel", FuelType::None),
            ("no fuel", FuelType::None),
            ("nofuel", FuelType::None),
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
        )
        .unwrap();
        assert_eq!(
            crate::config::equipment_id_from_config(&gas_furnace).unwrap(),
            Some(42)
        );
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
        )
        .unwrap();
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
        )
        .unwrap();
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
        )
        .unwrap();
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
        )
        .unwrap();
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
        )
        .unwrap();
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
        )
        .unwrap();
        assert_eq!(
            zone_id_from_config(&config),
            None,
            "zone_id_from_config must return None when zone_id key is absent from the typed config"
        );
    }

    #[test]
    fn zero_explicit_leakage_defaults_to_zone_type_class() {
        use std::collections::HashMap;

        let ctx = DuctDseContext {
            is_heating: true,
            capacity_w: 12_000.0,
            fan_flow_m3_s: 0.5,
            n_speeds: 1,
            capacity_low_w: None,
            fan_flow_low_m3_s: None,
            is_heat_pump: false,
        };

        let make_config = |zone_type: &str| -> EquipmentConfig {
            EquipmentConfig::with_payload(
                "test".to_string(),
                "Gas Furnace".to_string(),
                ConfigPayload::Raw {
                    data: HashMap::from([
                        (
                            "duct_zone_type".to_string(),
                            ConfigValue::Text(zone_type.to_string()),
                        ),
                        ("duct_latitude_deg".to_string(), ConfigValue::Float(40.0)),
                        ("duct_longitude_deg".to_string(), ConfigValue::Float(-105.0)),
                        (
                            "duct_house_volume_m3".to_string(),
                            ConfigValue::Float(400.0),
                        ),
                        (
                            "duct_supply_leakage_frac".to_string(),
                            ConfigValue::Float(0.0),
                        ),
                        (
                            "duct_return_leakage_frac".to_string(),
                            ConfigValue::Float(0.0),
                        ),
                        ("duct_supply_area_m2".to_string(), ConfigValue::Float(20.0)),
                        ("duct_supply_r_m2_k_w".to_string(), ConfigValue::Float(0.5)),
                        ("duct_return_area_m2".to_string(), ConfigValue::Float(12.0)),
                        ("duct_return_r_m2_k_w".to_string(), ConfigValue::Float(0.5)),
                    ]),
                },
            )
        };

        let attic_dse = resolve_duct_dse(&make_config("attic_unvented"), &ctx).unwrap();
        let basement_dse = resolve_duct_dse(&make_config("unins_basement"), &ctx).unwrap();

        assert!(
            attic_dse < basement_dse,
            "attic DSE ({attic_dse}) must be < basement DSE ({basement_dse}) — \
             attic defaults to Unsealed (more leakage), basement to WellSealed (less leakage)"
        );
        assert!(
            (0.0..1.0).contains(&attic_dse),
            "attic DSE {attic_dse} must be in (0,1) — class-default leakage must not be zero"
        );
        assert!(
            (0.0..1.0).contains(&basement_dse),
            "basement DSE {basement_dse} must be in (0,1) — class-default leakage must not be zero"
        );
    }

    /// Duct parameters with no conditioned volume are an error naming the
    /// unit, not a 400 m3 house.
    #[test]
    fn duct_dse_without_a_house_volume_errors() {
        use std::collections::HashMap;

        let ctx = DuctDseContext {
            is_heating: true,
            capacity_w: 12_000.0,
            fan_flow_m3_s: 0.5,
            n_speeds: 1,
            capacity_low_w: None,
            fan_flow_low_m3_s: None,
            is_heat_pump: false,
        };
        let config = EquipmentConfig::with_payload(
            "Unhoused".to_string(),
            "Gas Furnace".to_string(),
            ConfigPayload::Raw {
                data: HashMap::from([
                    (
                        "duct_zone_type".to_string(),
                        ConfigValue::Text("attic_unvented".to_string()),
                    ),
                    ("duct_latitude_deg".to_string(), ConfigValue::Float(40.0)),
                    ("duct_longitude_deg".to_string(), ConfigValue::Float(-105.0)),
                ]),
            },
        );
        let err = resolve_duct_dse(&config, &ctx).expect_err("no house volume must fail");
        assert!(
            err.to_string().contains("Unhoused")
                && err.to_string().contains("duct_house_volume_m3"),
            "got: {err}"
        );
    }

    #[test]
    fn ebm_telemetry_writes_nonzero_baseline_when_capacitance_and_load_are_nonzero() {
        use crate::hvac::ThermalSetpoints;
        use crate::hvac::hvac_core::{HvacEquipment, HvacEquipmentType};
        use crate::hvac::thermostat::{ThermostatConfig, ThermostatFsm, ThermostatMode};
        use hares_types::Telemetry;
        use hares_types::telemetry_keys as tk;

        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, hares_types::ZoneId(1));
        hvac.thermostat_fsm = ThermostatFsm::new(ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 25.0,
        });
        hvac.thermostat_fsm.thermostat = ThermostatConfig {
            hysteresis_c: 1.0,
            deadband_offset: 0.2,
            ..ThermostatConfig::default()
        };
        hvac.thermostat_fsm.mode = ThermostatMode::Heating;
        hvac.config.heating_capacities_w = vec![10_000.0];
        hvac.config.eir_by_stage = vec![0.25];
        hvac.config.zone_capacitance_kwh_per_k = 2.0;

        let mut telemetry = Telemetry::new();
        register_ebm_telemetry_keys(&mut telemetry);

        let env = crate::hvac::thermostat::tests::env_with_zone_temp(18.0);
        let window = step_equivalent_battery(&hvac, &env).unwrap();
        write_ebm_telemetry(window, 3000.0, &mut telemetry).unwrap();

        let efficiency = telemetry.get(tk::EBM_EFFICIENCY).unwrap();
        let baseline = telemetry.get(tk::EBM_BASELINE_POWER_KW).unwrap();
        assert!(
            efficiency > 0.0,
            "EBM efficiency should be COP = 1/EIR = 1/0.25 = 4.0, got {efficiency}"
        );
        assert!(
            baseline > 0.0,
            "EBM baseline_power_kw should be > 0 when capacity_ideal_w > 0, got {baseline}"
        );
        let expected_baseline = 3000.0 * 0.25 / 1000.0;
        assert!(
            (baseline - expected_baseline).abs() < 1e-9,
            "EBM baseline_power_kw should be capacity_ideal_w * eir / 1000 = {expected_baseline}, got {baseline}"
        );
    }

    #[test]
    fn ebm_telemetry_noop_when_capacitance_is_zero() {
        use crate::hvac::hvac_core::{HvacEquipment, HvacEquipmentType};
        use hares_types::Telemetry;
        use hares_types::telemetry_keys as tk;

        let hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, hares_types::ZoneId(1));
        let mut telemetry = Telemetry::new();
        register_ebm_telemetry_keys(&mut telemetry);

        let env = crate::hvac::thermostat::tests::env_with_zone_temp(20.0);
        let window = step_equivalent_battery(&hvac, &env).unwrap();
        assert!(window.is_none());
        write_ebm_telemetry(window, 5000.0, &mut telemetry).unwrap();

        let baseline = telemetry.get(tk::EBM_BASELINE_POWER_KW).unwrap();
        assert_eq!(
            baseline, 0.0,
            "EBM baseline should stay at 0.0 when zone_capacitance_kwh_per_k is 0 (disabled)"
        );
    }
}
