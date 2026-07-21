//! Canonical typed configuration structs for water heater equipment.

use serde::{Deserialize, Serialize};

use crate::config::EquipmentTypedConfig;
use hares_types::{FuelType, HaresError, ScheduleSourceConfig};

fn check_finite(name: &str, value: Option<f64>, min: f64, strict: bool) -> crate::Result<()> {
    if let Some(v) = value {
        let valid = v.is_finite() && if strict { v > min } else { v >= min };
        if !valid {
            return Err(HaresError::Equipment(format!(
                "{name} must be finite and {} {min}",
                if strict { ">" } else { ">=" }
            )));
        }
    }
    Ok(())
}

fn check_range(name: &str, value: Option<f64>, min: f64, max: f64) -> crate::Result<()> {
    if let Some(v) = value
        && (!v.is_finite() || !(min..=max).contains(&v))
    {
        return Err(HaresError::Equipment(format!(
            "{name} must be finite and within [{min}, {max}]"
        )));
    }
    Ok(())
}

fn check_setpoint_vs_max_temp(
    prefix: &str,
    setpoint_c: Option<f64>,
    max_tank_temp_c: Option<f64>,
    deadband_c: Option<f64>,
) -> crate::Result<()> {
    if let (Some(sp), Some(max_t)) = (setpoint_c, max_tank_temp_c) {
        let lower_bound = deadband_c.map_or(max_t, |db| max_t - db);
        if sp >= lower_bound {
            let msg = if let Some(db) = deadband_c {
                format!(
                    "{prefix}: setpoint_c ({sp}) must be strictly less than \
                     max_tank_temp_c ({max_t}) minus deadband_c ({db}); \
                     setpoint_c = {sp}, max_tank_temp_c - deadband_c = {lower_bound}"
                )
            } else {
                format!(
                    "{prefix}: setpoint_c ({sp}) must be strictly less than \
                     max_tank_temp_c ({max_t})"
                )
            };
            return Err(HaresError::Equipment(msg));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GasWaterHeaterConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    pub loop_id: Option<u16>,
    /// Required; resolver errors if absent.
    pub fuel_type: FuelType,
    pub tank_volume_m3: Option<f64>,
    pub tank_height_m: Option<f64>,
    pub energy_factor: Option<f64>,
    pub uniform_energy_factor: Option<f64>,
    pub heating_capacity_w: Option<f64>,
    pub ua_w_per_k: Option<f64>,
    pub setpoint_c: Option<f64>,
    pub deadband_c: Option<f64>,
    pub max_tank_temp_c: Option<f64>,
    pub initial_tank_temp_c: Option<f64>,
    pub tank_nodes: Option<u8>,
    pub avg_water_draw_l_per_day: Option<f64>,
    pub draw_flow_rate_kg_s: Option<f64>,
    pub draw_flow_rate_source: Option<ScheduleSourceConfig>,
    pub mains_temp_c_source: Option<ScheduleSourceConfig>,
    pub pilot_power_w: Option<f64>,
    pub flue_loss_fraction: Option<f64>,
    pub skin_loss_fraction: Option<f64>,
    pub ignition_type: Option<String>,
    pub performance_adjustment: Option<f64>,
    pub zone_type: Option<String>,
    pub first_hour_rating_m3: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jacket_r_value_m2_k_w: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversion_efficiency: Option<f64>,
    /// TMV fixture delivery temperature (°C). When set, `step_tempered` is used
    /// instead of `step`, mixing hot tank water with cold mains to deliver at
    /// this temperature. Default: 40.6°C (≈ 105°F) per OCHRE.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixture_delivery_temp_c: Option<f64>,
    /// Hot-draw delivery temperature (°C) for appliances like dishwashers.
    /// When `None`, defaults to the tank setpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hot_draw_temp_c: Option<f64>,
    /// Fraction of standing pilot thermal power routed to the tank water.
    /// The remainder is routed as ambient heat loss to the zone.
    /// Default: 0.80 — matches EnergyPlus's typical OffCycParaFracToTank
    /// (EnergyPlus WaterThermalTanks.cc:6213-6214).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pilot_fraction_to_tank: Option<f64>,
    /// Draft-inducer / power-vent blower electrical power (W), drawn while the
    /// burner fires. Default: `None` → 0 W, matching an atmospheric-vent unit
    /// with no blower (the OCHRE reference model carries no electric fan for
    /// gas storage water heaters — vendors/OCHRE/ochre/Equipment/WaterHeater.py).
    /// Typical power-vent draft-inducer blowers draw roughly 30–100 W while
    /// firing; OCHRE's gas-tankless reference hardcode uses 65 W on-cycle
    /// (vendors/OCHRE/ochre/Equipment/WaterHeater.py:800-804), and EnergyPlus
    /// models the same draw as WaterHeater:Mixed "On Cycle Parasitic Fuel
    /// Consumption Rate" with an electricity fuel type.
    /// This draw is a vented parasitic: it contributes real + reactive
    /// electrical power (fan-motor pf 0.87 class default) but no heat to the
    /// tank or zone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_power_w: Option<f64>,
}

impl EquipmentTypedConfig for GasWaterHeaterConfig {
    fn equipment_type_name() -> &'static str {
        "Gas Water Heater"
    }
}

impl GasWaterHeaterConfig {
    pub fn validate(&self) -> crate::Result<()> {
        check_finite("gas_wh: tank_volume_m3", self.tank_volume_m3, 0.0, true)?;
        check_finite("gas_wh: tank_height_m", self.tank_height_m, 0.0, true)?;
        check_range("gas_wh: energy_factor", self.energy_factor, 0.0, 1.5)?;
        check_range(
            "gas_wh: uniform_energy_factor",
            self.uniform_energy_factor,
            0.0,
            1.5,
        )?;
        check_finite(
            "gas_wh: heating_capacity_w",
            self.heating_capacity_w,
            0.0,
            false,
        )?;
        check_finite("gas_wh: ua_w_per_k", self.ua_w_per_k, 0.0, true)?;
        check_range("gas_wh: setpoint_c", self.setpoint_c, 40.0, 85.0)?;
        check_finite("gas_wh: deadband_c", self.deadband_c, 0.0, true)?;
        check_finite(
            "gas_wh: max_tank_temp_c",
            self.max_tank_temp_c,
            f64::NEG_INFINITY,
            false,
        )?;
        check_finite(
            "gas_wh: initial_tank_temp_c",
            self.initial_tank_temp_c,
            f64::NEG_INFINITY,
            false,
        )?;
        check_finite(
            "gas_wh: avg_water_draw_l_per_day",
            self.avg_water_draw_l_per_day,
            0.0,
            false,
        )?;
        check_finite(
            "gas_wh: draw_flow_rate_kg_s",
            self.draw_flow_rate_kg_s,
            0.0,
            false,
        )?;
        check_finite("gas_wh: pilot_power_w", self.pilot_power_w, 0.0, false)?;
        check_finite("gas_wh: fan_power_w", self.fan_power_w, 0.0, false)?;
        check_range(
            "gas_wh: flue_loss_fraction",
            self.flue_loss_fraction,
            0.0,
            1.0,
        )?;
        check_range(
            "gas_wh: skin_loss_fraction",
            self.skin_loss_fraction,
            0.0,
            1.0,
        )?;
        check_range(
            "gas_wh: performance_adjustment",
            self.performance_adjustment,
            0.0,
            1.0,
        )?;
        check_finite(
            "gas_wh: first_hour_rating_m3",
            self.first_hour_rating_m3,
            0.0,
            false,
        )?;
        check_finite(
            "gas_wh: jacket_r_value_m2_k_w",
            self.jacket_r_value_m2_k_w,
            0.0,
            false,
        )?;
        check_range(
            "gas_wh: conversion_efficiency",
            self.conversion_efficiency,
            0.01,
            1.0,
        )?;
        check_finite(
            "gas_wh: fixture_delivery_temp_c",
            self.fixture_delivery_temp_c,
            0.0,
            false,
        )?;
        check_finite("gas_wh: hot_draw_temp_c", self.hot_draw_temp_c, 0.0, false)?;
        check_range(
            "gas_wh: pilot_fraction_to_tank",
            self.pilot_fraction_to_tank,
            0.0,
            1.0,
        )?;
        check_setpoint_vs_max_temp(
            "gas_wh",
            self.setpoint_c,
            self.max_tank_temp_c,
            self.deadband_c,
        )?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElectricResistanceWaterHeaterConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    pub loop_id: Option<u16>,
    pub tank_volume_m3: Option<f64>,
    pub tank_height_m: Option<f64>,
    pub energy_factor: Option<f64>,
    pub uniform_energy_factor: Option<f64>,
    pub heating_capacity_w: Option<f64>,
    pub ua_w_per_k: Option<f64>,
    pub setpoint_c: Option<f64>,
    pub deadband_c: Option<f64>,
    pub max_tank_temp_c: Option<f64>,
    pub initial_tank_temp_c: Option<f64>,
    pub tank_nodes: Option<u8>,
    pub avg_water_draw_l_per_day: Option<f64>,
    pub draw_flow_rate_kg_s: Option<f64>,
    pub draw_flow_rate_source: Option<ScheduleSourceConfig>,
    pub mains_temp_c_source: Option<ScheduleSourceConfig>,
    pub performance_adjustment: Option<f64>,
    pub zone_type: Option<String>,
    pub first_hour_rating_m3: Option<f64>,
    pub element_power_w: Option<f64>,
    pub max_setpoint_ramp_rate_c_per_min: Option<f64>,
    pub element_priority_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jacket_r_value_m2_k_w: Option<f64>,
    /// Maximum combined power draw for both elements (W). When set and the
    /// priority mode is `Simultaneous`, the lower-element power is clamped so
    /// that `upper_power + lower_power ≤ max_combined_power_w`. Typical 30 A /
    /// 240 V residential branch circuits are limited to ~7,200 W.
    /// When `None`, no power ceiling is enforced; a `tracing::warn!` is emitted
    /// at init if both elements default to 4,500 W each in Simultaneous mode.
    /// Default residential branch-circuit limit: NEC Table 210.24(1), 30 A
    /// branch circuit at 240 V nominal = 7,200 W continuous (80% derate).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_combined_power_w: Option<f64>,
    /// TMV fixture delivery temperature (°C). Default: 40.6°C (≈ 105°F) per OCHRE.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixture_delivery_temp_c: Option<f64>,
    /// Hot-draw delivery temperature (°C). When `None`, defaults to tank setpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hot_draw_temp_c: Option<f64>,
}

impl EquipmentTypedConfig for ElectricResistanceWaterHeaterConfig {
    fn equipment_type_name() -> &'static str {
        "Electric Resistance Water Heater"
    }
}

impl ElectricResistanceWaterHeaterConfig {
    pub fn validate(&self) -> crate::Result<()> {
        check_finite(
            "resistance_wh: tank_volume_m3",
            self.tank_volume_m3,
            0.0,
            true,
        )?;
        check_finite(
            "resistance_wh: tank_height_m",
            self.tank_height_m,
            0.0,
            true,
        )?;
        check_range("resistance_wh: energy_factor", self.energy_factor, 0.0, 1.5)?;
        check_range(
            "resistance_wh: uniform_energy_factor",
            self.uniform_energy_factor,
            0.0,
            1.5,
        )?;
        check_finite(
            "resistance_wh: heating_capacity_w",
            self.heating_capacity_w,
            0.0,
            false,
        )?;
        check_finite("resistance_wh: ua_w_per_k", self.ua_w_per_k, 0.0, true)?;
        check_range("resistance_wh: setpoint_c", self.setpoint_c, 40.0, 85.0)?;
        check_finite("resistance_wh: deadband_c", self.deadband_c, 0.0, true)?;
        check_finite(
            "resistance_wh: max_tank_temp_c",
            self.max_tank_temp_c,
            f64::NEG_INFINITY,
            false,
        )?;
        check_finite(
            "resistance_wh: initial_tank_temp_c",
            self.initial_tank_temp_c,
            f64::NEG_INFINITY,
            false,
        )?;
        check_finite(
            "resistance_wh: avg_water_draw_l_per_day",
            self.avg_water_draw_l_per_day,
            0.0,
            false,
        )?;
        check_finite(
            "resistance_wh: draw_flow_rate_kg_s",
            self.draw_flow_rate_kg_s,
            0.0,
            false,
        )?;
        check_range(
            "resistance_wh: performance_adjustment",
            self.performance_adjustment,
            0.0,
            1.0,
        )?;
        check_finite(
            "resistance_wh: first_hour_rating_m3",
            self.first_hour_rating_m3,
            0.0,
            false,
        )?;
        check_finite(
            "resistance_wh: element_power_w",
            self.element_power_w,
            0.0,
            false,
        )?;
        check_finite(
            "resistance_wh: max_setpoint_ramp_rate_c_per_min",
            self.max_setpoint_ramp_rate_c_per_min,
            0.0,
            false,
        )?;
        check_finite(
            "resistance_wh: jacket_r_value_m2_k_w",
            self.jacket_r_value_m2_k_w,
            0.0,
            false,
        )?;
        check_finite(
            "resistance_wh: max_combined_power_w",
            self.max_combined_power_w,
            0.0,
            true,
        )?;
        check_finite(
            "resistance_wh: fixture_delivery_temp_c",
            self.fixture_delivery_temp_c,
            0.0,
            false,
        )?;
        check_finite(
            "resistance_wh: hot_draw_temp_c",
            self.hot_draw_temp_c,
            0.0,
            false,
        )?;
        check_setpoint_vs_max_temp(
            "resistance_wh",
            self.setpoint_c,
            self.max_tank_temp_c,
            self.deadband_c,
        )?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TanklessWaterHeaterConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    pub loop_id: Option<u16>,
    /// Required; resolver errors if absent.
    pub fuel_type: FuelType,
    pub energy_factor: Option<f64>,
    pub uniform_energy_factor: Option<f64>,
    pub heating_capacity_w: Option<f64>,
    pub setpoint_c: Option<f64>,
    pub parasitic_power_w: Option<f64>,
    pub performance_adjustment: Option<f64>,
    pub inlet_temp_c: Option<f64>,
    pub draw_flow_rate_kg_s: Option<f64>,
    pub draw_flow_rate_source: Option<ScheduleSourceConfig>,
    pub mains_temp_c_source: Option<ScheduleSourceConfig>,
    pub avg_water_draw_l_per_day: Option<f64>,
    pub zone_type: Option<String>,
    /// Minimum flow threshold [kg/s] below which the burner does not fire.
    /// Typical tankless flow sensors require ~0.5 GPM (≈ 0.03 kg/s).
    /// Defaults to 0.0 when `None` (preserves current behaviour).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_flow_kg_s: Option<f64>,
    /// Minimum flow threshold [US GPM] as a convenience input.
    /// Converted internally to kg/s via `GALLONS_PER_MINUTE_TO_KG_PER_SECOND`
    /// and used only when `min_flow_kg_s` is `None`.
    /// Defaults to `None` → 0.0 kg/s threshold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_flow_gpm: Option<f64>,
}

impl EquipmentTypedConfig for TanklessWaterHeaterConfig {
    fn equipment_type_name() -> &'static str {
        "Tankless Water Heater"
    }
}

impl TanklessWaterHeaterConfig {
    pub fn validate(&self) -> crate::Result<()> {
        check_range("tankless_wh: energy_factor", self.energy_factor, 0.0, 1.5)?;
        check_range(
            "tankless_wh: uniform_energy_factor",
            self.uniform_energy_factor,
            0.0,
            1.5,
        )?;
        check_finite(
            "tankless_wh: heating_capacity_w",
            self.heating_capacity_w,
            0.0,
            false,
        )?;
        check_finite(
            "tankless_wh: setpoint_c",
            self.setpoint_c,
            f64::NEG_INFINITY,
            false,
        )?;
        check_finite(
            "tankless_wh: parasitic_power_w",
            self.parasitic_power_w,
            0.0,
            false,
        )?;
        check_finite(
            "tankless_wh: inlet_temp_c",
            self.inlet_temp_c,
            f64::NEG_INFINITY,
            false,
        )?;
        check_finite(
            "tankless_wh: draw_flow_rate_kg_s",
            self.draw_flow_rate_kg_s,
            0.0,
            false,
        )?;
        check_range(
            "tankless_wh: performance_adjustment",
            self.performance_adjustment,
            0.0,
            1.0,
        )?;
        check_finite(
            "tankless_wh: avg_water_draw_l_per_day",
            self.avg_water_draw_l_per_day,
            0.0,
            false,
        )?;
        check_finite("tankless_wh: min_flow_kg_s", self.min_flow_kg_s, 0.0, false)?;
        check_finite("tankless_wh: min_flow_gpm", self.min_flow_gpm, 0.0, false)?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndirectTankConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    pub boiler_loop_id: Option<u16>,
    pub tank_volume_m3: Option<f64>,
    pub tank_height_m: Option<f64>,
    pub ua_w_per_k: Option<f64>,
    pub hx_ua_w_per_k: Option<f64>,
    pub setpoint_c: Option<f64>,
    pub deadband_c: Option<f64>,
    pub max_tank_temp_c: Option<f64>,
    pub initial_tank_temp_c: Option<f64>,
    pub tank_nodes: Option<u8>,
    pub draw_flow_rate_kg_s: Option<f64>,
    pub avg_water_draw_l_per_day: Option<f64>,
    pub draw_flow_rate_source: Option<ScheduleSourceConfig>,
    pub mains_temp_c_source: Option<ScheduleSourceConfig>,
    pub performance_adjustment: Option<f64>,
    pub zone_type: Option<String>,
    pub first_hour_rating_m3: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jacket_r_value_m2_k_w: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixture_delivery_temp_c: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hot_draw_temp_c: Option<f64>,
    /// Boiler loop assumed flow rate [kg/s] for computing return temperature.
    /// When `None`, defaults to 0.1 kg/s — a typical residential hydronic
    /// loop with a Grundfos UPS15-58 on speed 1 (~6 GPM for a 10 ft head).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boiler_loop_flow_rate_kg_s: Option<f64>,
}

impl EquipmentTypedConfig for IndirectTankConfig {
    fn equipment_type_name() -> &'static str {
        "Indirect Tank"
    }
}

impl IndirectTankConfig {
    pub fn validate(&self) -> crate::Result<()> {
        check_finite(
            "indirect_tank: tank_volume_m3",
            self.tank_volume_m3,
            0.0,
            true,
        )?;
        check_finite(
            "indirect_tank: tank_height_m",
            self.tank_height_m,
            0.0,
            true,
        )?;
        check_finite("indirect_tank: ua_w_per_k", self.ua_w_per_k, 0.0, true)?;
        check_finite(
            "indirect_tank: hx_ua_w_per_k",
            self.hx_ua_w_per_k,
            0.0,
            true,
        )?;
        check_range("indirect_tank: setpoint_c", self.setpoint_c, 40.0, 85.0)?;
        check_finite("indirect_tank: deadband_c", self.deadband_c, 0.0, true)?;
        check_finite(
            "indirect_tank: max_tank_temp_c",
            self.max_tank_temp_c,
            f64::NEG_INFINITY,
            false,
        )?;
        check_finite(
            "indirect_tank: initial_tank_temp_c",
            self.initial_tank_temp_c,
            f64::NEG_INFINITY,
            false,
        )?;
        check_finite(
            "indirect_tank: draw_flow_rate_kg_s",
            self.draw_flow_rate_kg_s,
            0.0,
            false,
        )?;
        check_finite(
            "indirect_tank: avg_water_draw_l_per_day",
            self.avg_water_draw_l_per_day,
            0.0,
            false,
        )?;
        check_range(
            "indirect_tank: performance_adjustment",
            self.performance_adjustment,
            0.0,
            1.0,
        )?;
        check_finite(
            "indirect_tank: first_hour_rating_m3",
            self.first_hour_rating_m3,
            0.0,
            false,
        )?;
        check_finite(
            "indirect_tank: jacket_r_value_m2_k_w",
            self.jacket_r_value_m2_k_w,
            0.0,
            false,
        )?;
        check_finite(
            "indirect_tank: fixture_delivery_temp_c",
            self.fixture_delivery_temp_c,
            0.0,
            false,
        )?;
        check_finite(
            "indirect_tank: hot_draw_temp_c",
            self.hot_draw_temp_c,
            0.0,
            false,
        )?;
        check_finite(
            "indirect_tank: boiler_loop_flow_rate_kg_s",
            self.boiler_loop_flow_rate_kg_s,
            0.0,
            true,
        )?;
        check_setpoint_vs_max_temp(
            "indirect_tank",
            self.setpoint_c,
            self.max_tank_temp_c,
            self.deadband_c,
        )?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeatPumpWaterHeaterConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    pub loop_id: Option<u16>,
    pub tank_volume_m3: Option<f64>,
    pub tank_height_m: Option<f64>,
    pub cop: Option<f64>,
    pub backup_element_power_w: Option<f64>,
    pub ua_w_per_k: Option<f64>,
    pub setpoint_c: Option<f64>,
    pub deadband_c: Option<f64>,
    pub max_tank_temp_c: Option<f64>,
    pub initial_tank_temp_c: Option<f64>,
    pub tank_nodes: Option<u8>,
    pub tempering_valve_setpoint_c: Option<f64>,
    pub avg_water_draw_l_per_day: Option<f64>,
    pub draw_flow_rate_kg_s: Option<f64>,
    pub compressor_power_w: Option<f64>,
    pub backup_enable_offset_c: Option<f64>,
    pub min_ambient_temp_c: Option<f64>,
    pub max_ambient_temp_c: Option<f64>,
    pub min_on_time_s: Option<f64>,
    pub min_off_time_s: Option<f64>,
    pub hp_only_mode: Option<bool>,
    pub element_hp_control_mode: Option<String>,
    pub fan_power_w: Option<f64>,
    pub parasitic_power_w: Option<f64>,
    pub backup_efficiency: Option<f64>,
    pub shr: Option<f64>,
    pub lost_heat_fraction: Option<f64>,
    pub wall_heat_fraction: Option<f64>,
    pub capacity_biquadratic_coeffs: Option<[f64; 6]>,
    pub cop_biquadratic_coeffs: Option<[f64; 6]>,
    pub performance_adjustment: Option<f64>,
    pub zone_type: Option<String>,
    pub first_hour_rating_m3: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jacket_r_value_m2_k_w: Option<f64>,
    /// TMV fixture delivery temperature (°C). Default: 40.6°C (≈ 105°F) per OCHRE.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixture_delivery_temp_c: Option<f64>,
    /// Set to true for low-power HPWH. Uses distinct COP/capacity biquadratic
    /// coefficients and widened ambient lockout bounds (2.778–62.778°C instead
    /// of 7.222–43.333°C). When `None`, HARES auto-detects from
    /// `uniform_energy_factor` if provided (UEF >= 4.8 triggers blending,
    /// UEF >= 4.9 identifies the unit as the low-power compressor class).
    ///
    /// **HARES divergence from OCHRE:** OCHRE's HPXML import layer
    /// (`ochre/utils/hpxml.py:1174-1181`) uses exact-equality `UEF == 4.9` as
    /// a sentinel for one specific 120V ResStock product and documents the flag
    /// as a "temporary flag." HARES generalizes this to a continuous
    /// UEF-based compressor-class transition with smooth blending over
    /// [4.8, 5.0] rather than a hard switch at a single sentinel value.
    /// The low-power coefficient set (COP and capacity curves) is verified
    /// against `WaterHeater.py:469-470` and is a physically-distinct compressor
    /// family independent of the triggering mechanism.
    /// Source for coefficients: vendors/OCHRE/ochre/Equipment/WaterHeater.py lines 444, 462-474, 611-616.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub low_power_hpwh: Option<bool>,
    /// Uniform Energy Factor (post-2015 test procedure). When >= 4.8 and
    /// `low_power_hpwh` is not explicitly set, HARES activates low-power
    /// curve blending with a linear cross-fade over [4.8, 5.0]. Units with
    /// UEF >= 4.9 are treated as the low-power compressor class with widened
    /// ambient lockout bounds.
    /// Source: HPXML 4.2 §3.8.2; ResStock waterheater.rb.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uniform_energy_factor: Option<f64>,
}

impl EquipmentTypedConfig for HeatPumpWaterHeaterConfig {
    fn equipment_type_name() -> &'static str {
        "Heat Pump Water Heater"
    }
}

impl HeatPumpWaterHeaterConfig {
    pub fn validate(&self) -> crate::Result<()> {
        check_finite("hpwh: tank_volume_m3", self.tank_volume_m3, 0.0, true)?;
        check_finite("hpwh: tank_height_m", self.tank_height_m, 0.0, true)?;
        check_finite("hpwh: cop", self.cop, 0.0, true)?;
        check_finite(
            "hpwh: backup_element_power_w",
            self.backup_element_power_w,
            0.0,
            false,
        )?;
        check_finite("hpwh: ua_w_per_k", self.ua_w_per_k, 0.0, true)?;
        check_range("hpwh: setpoint_c", self.setpoint_c, 40.0, 85.0)?;
        check_finite("hpwh: deadband_c", self.deadband_c, 0.0, true)?;
        check_finite(
            "hpwh: max_tank_temp_c",
            self.max_tank_temp_c,
            f64::NEG_INFINITY,
            false,
        )?;
        check_finite(
            "hpwh: initial_tank_temp_c",
            self.initial_tank_temp_c,
            f64::NEG_INFINITY,
            false,
        )?;
        check_finite(
            "hpwh: tempering_valve_setpoint_c",
            self.tempering_valve_setpoint_c,
            f64::NEG_INFINITY,
            false,
        )?;
        check_finite(
            "hpwh: avg_water_draw_l_per_day",
            self.avg_water_draw_l_per_day,
            0.0,
            false,
        )?;
        check_finite(
            "hpwh: draw_flow_rate_kg_s",
            self.draw_flow_rate_kg_s,
            0.0,
            false,
        )?;
        check_finite(
            "hpwh: compressor_power_w",
            self.compressor_power_w,
            0.0,
            false,
        )?;
        check_finite(
            "hpwh: backup_enable_offset_c",
            self.backup_enable_offset_c,
            0.0,
            false,
        )?;
        check_finite("hpwh: min_on_time_s", self.min_on_time_s, 0.0, false)?;
        check_finite("hpwh: min_off_time_s", self.min_off_time_s, 0.0, false)?;
        check_finite("hpwh: fan_power_w", self.fan_power_w, 0.0, false)?;
        check_finite(
            "hpwh: parasitic_power_w",
            self.parasitic_power_w,
            0.0,
            false,
        )?;
        check_range("hpwh: backup_efficiency", self.backup_efficiency, 0.0, 1.0)?;
        check_range("hpwh: shr", self.shr, 0.0, 1.0)?;
        check_range(
            "hpwh: lost_heat_fraction",
            self.lost_heat_fraction,
            0.0,
            1.0,
        )?;
        check_range(
            "hpwh: wall_heat_fraction",
            self.wall_heat_fraction,
            0.0,
            1.0,
        )?;
        check_range(
            "hpwh: performance_adjustment",
            self.performance_adjustment,
            0.0,
            1.0,
        )?;
        check_finite(
            "hpwh: first_hour_rating_m3",
            self.first_hour_rating_m3,
            0.0,
            false,
        )?;
        check_finite(
            "hpwh: jacket_r_value_m2_k_w",
            self.jacket_r_value_m2_k_w,
            0.0,
            false,
        )?;
        check_finite(
            "hpwh: fixture_delivery_temp_c",
            self.fixture_delivery_temp_c,
            0.0,
            false,
        )?;
        check_finite(
            "hpwh: uniform_energy_factor",
            self.uniform_energy_factor,
            0.0,
            true,
        )?;
        if let Some(min) = self.min_ambient_temp_c
            && let Some(max) = self.max_ambient_temp_c
            && min >= max
        {
            return Err(HaresError::Equipment(format!(
                "hpwh: min_ambient_temp_c ({min}) must be < max_ambient_temp_c ({max})"
            )));
        }
        check_setpoint_vs_max_temp(
            "hpwh",
            self.setpoint_c,
            self.max_tank_temp_c,
            self.deadband_c,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConfigPayload, EquipmentConfig};

    #[test]
    fn gas_wh_round_trips_and_rejects_unknown_fields() {
        let cfg = GasWaterHeaterConfig {
            fan_power_w: None,
            equipment_id: Some(7),
            zone_id: Some(2),
            loop_id: Some(3),
            fuel_type: FuelType::Propane,
            tank_volume_m3: Some(0.189),
            tank_height_m: Some(1.2),
            energy_factor: Some(0.78),
            uniform_energy_factor: Some(0.81),
            heating_capacity_w: Some(11_000.0),
            ua_w_per_k: Some(2.0),
            setpoint_c: Some(51.67),
            deadband_c: None,
            max_tank_temp_c: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            avg_water_draw_l_per_day: Some(227.0),
            draw_flow_rate_kg_s: None,
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            pilot_power_w: Some(5.0),
            flue_loss_fraction: Some(0.1),
            skin_loss_fraction: None,
            ignition_type: None,
            performance_adjustment: Some(0.92),
            zone_type: Some("conditioned".to_string()),
            first_hour_rating_m3: Some(0.2),
            jacket_r_value_m2_k_w: None,
            conversion_efficiency: None,
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
            pilot_fraction_to_tank: None,
        };
        let ec = EquipmentConfig::from_typed(
            "gas".to_string(),
            "Gas Water Heater".to_string(),
            cfg.clone(),
        )
        .unwrap();
        assert!(ec.is_typed());
        let recovered: GasWaterHeaterConfig = ec.typed().expect("typed decode");
        assert_eq!(recovered, cfg);
        assert!(cfg.validate().is_ok());

        let ec = EquipmentConfig::with_payload(
            "gas".to_string(),
            "Gas Water Heater".to_string(),
            ConfigPayload::Typed {
                type_name: "Gas Water Heater".to_string(),
                version: 1,
                data: serde_json::json!({"tank_volume_m3": 0.189, "unknown_field": 1}),
            },
        );
        let result: crate::Result<GasWaterHeaterConfig> = ec.typed();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown field"));
    }

    #[test]
    fn resistance_wh_round_trips_and_rejects_unknown_fields() {
        let cfg = ElectricResistanceWaterHeaterConfig {
            equipment_id: Some(7),
            zone_id: Some(2),
            loop_id: Some(3),
            tank_volume_m3: Some(0.189),
            tank_height_m: Some(1.2),
            energy_factor: Some(0.92),
            uniform_energy_factor: Some(0.95),
            heating_capacity_w: Some(4_500.0),
            ua_w_per_k: Some(2.0),
            setpoint_c: Some(51.67),
            deadband_c: None,
            max_tank_temp_c: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            avg_water_draw_l_per_day: Some(227.0),
            draw_flow_rate_kg_s: None,
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            performance_adjustment: Some(0.92),
            zone_type: Some("conditioned".to_string()),
            first_hour_rating_m3: Some(0.2),
            element_power_w: Some(4_500.0),
            max_setpoint_ramp_rate_c_per_min: Some(3.0),
            element_priority_mode: None,
            jacket_r_value_m2_k_w: None,
            max_combined_power_w: Some(7_200.0),
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
        };
        let ec = EquipmentConfig::from_typed(
            "resistance".to_string(),
            "Electric Resistance Water Heater".to_string(),
            cfg.clone(),
        )
        .unwrap();
        assert!(ec.is_typed());
        let recovered: ElectricResistanceWaterHeaterConfig = ec.typed().expect("typed decode");
        assert_eq!(recovered, cfg);
        assert!(cfg.validate().is_ok());

        let ec = EquipmentConfig::with_payload(
            "resistance".to_string(),
            "Electric Resistance Water Heater".to_string(),
            ConfigPayload::Typed {
                type_name: "Electric Resistance Water Heater".to_string(),
                version: 1,
                data: serde_json::json!({"element_power_w": 4500.0, "bogus": true}),
            },
        );
        let result: crate::Result<ElectricResistanceWaterHeaterConfig> = ec.typed();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown field"));
    }

    #[test]
    fn tankless_wh_round_trips_and_rejects_unknown_fields() {
        let cfg = TanklessWaterHeaterConfig {
            equipment_id: Some(7),
            zone_id: Some(2),
            loop_id: Some(3),
            fuel_type: FuelType::Gas,
            energy_factor: Some(0.87),
            uniform_energy_factor: Some(0.88),
            heating_capacity_w: Some(20_000.0),
            setpoint_c: Some(51.67),
            parasitic_power_w: Some(7.38),
            performance_adjustment: Some(0.92),
            inlet_temp_c: None,
            draw_flow_rate_kg_s: None,
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            avg_water_draw_l_per_day: Some(227.0),
            zone_type: None,
            min_flow_kg_s: None,
            min_flow_gpm: None,
        };
        let ec = EquipmentConfig::from_typed(
            "tankless".to_string(),
            "Tankless Water Heater".to_string(),
            cfg.clone(),
        )
        .unwrap();
        assert!(ec.is_typed());
        let recovered: TanklessWaterHeaterConfig = ec.typed().expect("typed decode");
        assert_eq!(recovered, cfg);
        assert!(cfg.validate().is_ok());

        let ec = EquipmentConfig::with_payload(
            "tankless".to_string(),
            "Tankless Water Heater".to_string(),
            ConfigPayload::Typed {
                type_name: "Tankless Water Heater".to_string(),
                version: 1,
                data: serde_json::json!({"fuel_type": "Gas", "mystery": 1}),
            },
        );
        let result: crate::Result<TanklessWaterHeaterConfig> = ec.typed();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown field"));
    }

    #[test]
    fn hpwh_round_trips_and_rejects_unknown_fields() {
        let cfg = HeatPumpWaterHeaterConfig {
            equipment_id: Some(7),
            zone_id: Some(2),
            loop_id: Some(3),
            tank_volume_m3: Some(0.189),
            tank_height_m: Some(1.2),
            cop: Some(3.5),
            backup_element_power_w: Some(4_500.0),
            ua_w_per_k: Some(2.0),
            setpoint_c: Some(51.67),
            deadband_c: None,
            max_tank_temp_c: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            tempering_valve_setpoint_c: Some(51.67),
            avg_water_draw_l_per_day: Some(227.0),
            draw_flow_rate_kg_s: None,
            compressor_power_w: None,
            backup_enable_offset_c: None,
            min_ambient_temp_c: None,
            max_ambient_temp_c: None,
            min_on_time_s: None,
            min_off_time_s: None,
            hp_only_mode: None,
            element_hp_control_mode: None,
            fan_power_w: None,
            parasitic_power_w: None,
            backup_efficiency: None,
            shr: None,
            lost_heat_fraction: None,
            wall_heat_fraction: None,
            capacity_biquadratic_coeffs: None,
            cop_biquadratic_coeffs: None,
            performance_adjustment: Some(0.92),
            zone_type: Some("conditioned".to_string()),
            first_hour_rating_m3: Some(0.2),
            jacket_r_value_m2_k_w: None,
            fixture_delivery_temp_c: None,
            low_power_hpwh: None,
            uniform_energy_factor: None,
        };
        let ec = EquipmentConfig::from_typed(
            "hpwh".to_string(),
            "Heat Pump Water Heater".to_string(),
            cfg.clone(),
        )
        .unwrap();
        assert!(ec.is_typed());
        let recovered: HeatPumpWaterHeaterConfig = ec.typed().expect("typed decode");
        assert_eq!(recovered, cfg);
        assert!(cfg.validate().is_ok());

        let ec = EquipmentConfig::with_payload(
            "hpwh".to_string(),
            "Heat Pump Water Heater".to_string(),
            ConfigPayload::Typed {
                type_name: "Heat Pump Water Heater".to_string(),
                version: 1,
                data: serde_json::json!({"cop": 3.5, "oops": 1}),
            },
        );
        let result: crate::Result<HeatPumpWaterHeaterConfig> = ec.typed();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown field"));
    }

    #[test]
    fn indirect_tank_round_trips_and_rejects_unknown_fields() {
        let cfg = IndirectTankConfig {
            equipment_id: Some(7),
            zone_id: Some(2),
            boiler_loop_id: Some(3),
            tank_volume_m3: Some(0.189),
            tank_height_m: Some(1.2),
            ua_w_per_k: Some(2.0),
            hx_ua_w_per_k: Some(150.0),
            setpoint_c: Some(51.67),
            deadband_c: None,
            max_tank_temp_c: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            draw_flow_rate_kg_s: None,
            avg_water_draw_l_per_day: Some(227.0),
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            performance_adjustment: Some(0.92),
            zone_type: Some("conditioned".to_string()),
            first_hour_rating_m3: Some(0.2),
            jacket_r_value_m2_k_w: None,
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
            boiler_loop_flow_rate_kg_s: Some(0.3),
        };
        let ec = EquipmentConfig::from_typed(
            "indirect".to_string(),
            "Indirect Tank".to_string(),
            cfg.clone(),
        )
        .unwrap();
        assert!(ec.is_typed());
        let recovered: IndirectTankConfig = ec.typed().expect("typed decode");
        assert_eq!(recovered, cfg);
        assert!(cfg.validate().is_ok());

        let ec = EquipmentConfig::with_payload(
            "indirect".to_string(),
            "Indirect Tank".to_string(),
            ConfigPayload::Typed {
                type_name: "Indirect Tank".to_string(),
                version: 1,
                data: serde_json::json!({"tank_volume_m3": 0.189, "unknown_field": 1}),
            },
        );
        let result: crate::Result<IndirectTankConfig> = ec.typed();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown field"));
    }

    // --- deadband validation tests ---

    fn gas_wh_with_deadband(deadband_c: Option<f64>) -> GasWaterHeaterConfig {
        GasWaterHeaterConfig {
            fan_power_w: None,
            fuel_type: FuelType::Gas,
            deadband_c,
            equipment_id: None,
            zone_id: None,
            loop_id: None,
            tank_volume_m3: None,
            tank_height_m: None,
            energy_factor: None,
            uniform_energy_factor: None,
            heating_capacity_w: None,
            ua_w_per_k: None,
            setpoint_c: None,
            max_tank_temp_c: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: None,
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            pilot_power_w: None,
            flue_loss_fraction: None,
            skin_loss_fraction: None,
            ignition_type: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            jacket_r_value_m2_k_w: None,
            conversion_efficiency: None,
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
            pilot_fraction_to_tank: None,
        }
    }

    fn resistance_wh_with_deadband(deadband_c: Option<f64>) -> ElectricResistanceWaterHeaterConfig {
        ElectricResistanceWaterHeaterConfig {
            deadband_c,
            equipment_id: None,
            zone_id: None,
            loop_id: None,
            tank_volume_m3: None,
            tank_height_m: None,
            energy_factor: None,
            uniform_energy_factor: None,
            heating_capacity_w: None,
            ua_w_per_k: None,
            setpoint_c: None,
            max_tank_temp_c: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: None,
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            element_power_w: None,
            max_setpoint_ramp_rate_c_per_min: None,
            element_priority_mode: None,
            jacket_r_value_m2_k_w: None,
            max_combined_power_w: None,
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
        }
    }

    fn hpwh_with_deadband(deadband_c: Option<f64>) -> HeatPumpWaterHeaterConfig {
        HeatPumpWaterHeaterConfig {
            deadband_c,
            equipment_id: None,
            zone_id: None,
            loop_id: None,
            tank_volume_m3: None,
            tank_height_m: None,
            cop: None,
            backup_element_power_w: None,
            ua_w_per_k: None,
            setpoint_c: None,
            max_tank_temp_c: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            tempering_valve_setpoint_c: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: None,
            compressor_power_w: None,
            backup_enable_offset_c: None,
            min_ambient_temp_c: None,
            max_ambient_temp_c: None,
            min_on_time_s: None,
            min_off_time_s: None,
            hp_only_mode: None,
            element_hp_control_mode: None,
            fan_power_w: None,
            parasitic_power_w: None,
            backup_efficiency: None,
            shr: None,
            lost_heat_fraction: None,
            wall_heat_fraction: None,
            capacity_biquadratic_coeffs: None,
            cop_biquadratic_coeffs: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            jacket_r_value_m2_k_w: None,
            fixture_delivery_temp_c: None,
            low_power_hpwh: None,
            uniform_energy_factor: None,
        }
    }

    fn indirect_tank_with_deadband(deadband_c: Option<f64>) -> IndirectTankConfig {
        IndirectTankConfig {
            deadband_c,
            equipment_id: None,
            zone_id: None,
            boiler_loop_id: None,
            tank_volume_m3: None,
            tank_height_m: None,
            ua_w_per_k: None,
            hx_ua_w_per_k: None,
            setpoint_c: None,
            max_tank_temp_c: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            draw_flow_rate_kg_s: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            jacket_r_value_m2_k_w: None,
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
            boiler_loop_flow_rate_kg_s: None,
        }
    }

    #[test]
    fn deadband_zero_rejected_for_storage_water_heaters() {
        assert!(gas_wh_with_deadband(Some(0.0)).validate().is_err());
        assert!(resistance_wh_with_deadband(Some(0.0)).validate().is_err());
        assert!(hpwh_with_deadband(Some(0.0)).validate().is_err());
        assert!(indirect_tank_with_deadband(Some(0.0)).validate().is_err());
    }

    #[test]
    fn deadband_negative_rejected_for_storage_water_heaters() {
        assert!(gas_wh_with_deadband(Some(-1.0)).validate().is_err());
        assert!(resistance_wh_with_deadband(Some(-1.0)).validate().is_err());
        assert!(hpwh_with_deadband(Some(-1.0)).validate().is_err());
        assert!(indirect_tank_with_deadband(Some(-1.0)).validate().is_err());
    }

    #[test]
    fn deadband_positive_accepted_for_storage_water_heaters() {
        // 5.56°C = 10°F, OCHRE's default deadband (WaterHeater.py:76).
        assert!(gas_wh_with_deadband(Some(5.56)).validate().is_ok());
        assert!(resistance_wh_with_deadband(Some(5.56)).validate().is_ok());
        assert!(hpwh_with_deadband(Some(5.56)).validate().is_ok());
        assert!(indirect_tank_with_deadband(Some(5.56)).validate().is_ok());
    }

    #[test]
    fn deadband_none_still_accepted() {
        // deadband_c is optional; None should still pass validation.
        assert!(gas_wh_with_deadband(None).validate().is_ok());
        assert!(resistance_wh_with_deadband(None).validate().is_ok());
        assert!(hpwh_with_deadband(None).validate().is_ok());
        assert!(indirect_tank_with_deadband(None).validate().is_ok());
    }

    // --- UA validation helpers ---

    fn gas_wh_with_ua(ua_w_per_k: Option<f64>) -> GasWaterHeaterConfig {
        GasWaterHeaterConfig {
            fan_power_w: None,
            fuel_type: FuelType::Gas,
            ua_w_per_k,
            equipment_id: None,
            zone_id: None,
            loop_id: None,
            tank_volume_m3: None,
            tank_height_m: None,
            energy_factor: None,
            uniform_energy_factor: None,
            heating_capacity_w: None,
            setpoint_c: None,
            deadband_c: None,
            max_tank_temp_c: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: None,
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            pilot_power_w: None,
            flue_loss_fraction: None,
            skin_loss_fraction: None,
            ignition_type: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            jacket_r_value_m2_k_w: None,
            conversion_efficiency: None,
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
            pilot_fraction_to_tank: None,
        }
    }

    fn resistance_wh_with_ua(ua_w_per_k: Option<f64>) -> ElectricResistanceWaterHeaterConfig {
        ElectricResistanceWaterHeaterConfig {
            ua_w_per_k,
            equipment_id: None,
            zone_id: None,
            loop_id: None,
            tank_volume_m3: None,
            tank_height_m: None,
            energy_factor: None,
            uniform_energy_factor: None,
            heating_capacity_w: None,
            setpoint_c: None,
            deadband_c: None,
            max_tank_temp_c: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: None,
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            element_power_w: None,
            max_setpoint_ramp_rate_c_per_min: None,
            element_priority_mode: None,
            jacket_r_value_m2_k_w: None,
            max_combined_power_w: None,
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
        }
    }

    fn hpwh_with_ua(ua_w_per_k: Option<f64>) -> HeatPumpWaterHeaterConfig {
        HeatPumpWaterHeaterConfig {
            ua_w_per_k,
            equipment_id: None,
            zone_id: None,
            loop_id: None,
            tank_volume_m3: None,
            tank_height_m: None,
            cop: None,
            backup_element_power_w: None,
            setpoint_c: None,
            deadband_c: None,
            max_tank_temp_c: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            tempering_valve_setpoint_c: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: None,
            compressor_power_w: None,
            backup_enable_offset_c: None,
            min_ambient_temp_c: None,
            max_ambient_temp_c: None,
            min_on_time_s: None,
            min_off_time_s: None,
            hp_only_mode: None,
            element_hp_control_mode: None,
            fan_power_w: None,
            parasitic_power_w: None,
            backup_efficiency: None,
            shr: None,
            lost_heat_fraction: None,
            wall_heat_fraction: None,
            capacity_biquadratic_coeffs: None,
            cop_biquadratic_coeffs: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            jacket_r_value_m2_k_w: None,
            fixture_delivery_temp_c: None,
            low_power_hpwh: None,
            uniform_energy_factor: None,
        }
    }

    fn indirect_tank_with_ua(ua_w_per_k: Option<f64>) -> IndirectTankConfig {
        IndirectTankConfig {
            ua_w_per_k,
            equipment_id: None,
            zone_id: None,
            boiler_loop_id: None,
            tank_volume_m3: None,
            tank_height_m: None,
            hx_ua_w_per_k: None,
            setpoint_c: None,
            deadband_c: None,
            max_tank_temp_c: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            draw_flow_rate_kg_s: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            jacket_r_value_m2_k_w: None,
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
            boiler_loop_flow_rate_kg_s: None,
        }
    }

    fn indirect_tank_with_hx_ua(hx_ua_w_per_k: Option<f64>) -> IndirectTankConfig {
        IndirectTankConfig {
            hx_ua_w_per_k,
            ua_w_per_k: Some(2.0),
            equipment_id: None,
            zone_id: None,
            boiler_loop_id: None,
            tank_volume_m3: None,
            tank_height_m: None,
            setpoint_c: None,
            deadband_c: None,
            max_tank_temp_c: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            draw_flow_rate_kg_s: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            jacket_r_value_m2_k_w: None,
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
            boiler_loop_flow_rate_kg_s: None,
        }
    }

    #[test]
    fn ua_zero_rejected_for_storage_water_heaters() {
        // Zero-conductance violates Second Law of Thermodynamics.
        // OCHRE hpxml.py:1141-1144 explicitly rejects UA <= 0.
        assert!(gas_wh_with_ua(Some(0.0)).validate().is_err());
        assert!(resistance_wh_with_ua(Some(0.0)).validate().is_err());
        assert!(hpwh_with_ua(Some(0.0)).validate().is_err());
        assert!(indirect_tank_with_ua(Some(0.0)).validate().is_err());
        assert!(indirect_tank_with_hx_ua(Some(0.0)).validate().is_err());
    }

    #[test]
    fn ua_negative_rejected_for_storage_water_heaters() {
        assert!(gas_wh_with_ua(Some(-1.0)).validate().is_err());
        assert!(resistance_wh_with_ua(Some(-1.0)).validate().is_err());
        assert!(hpwh_with_ua(Some(-1.0)).validate().is_err());
        assert!(indirect_tank_with_ua(Some(-1.0)).validate().is_err());
        assert!(indirect_tank_with_hx_ua(Some(-1.0)).validate().is_err());
    }

    #[test]
    fn ua_positive_accepted_for_storage_water_heaters() {
        // Positive UA should still pass (covered by round-trip tests as well).
        assert!(gas_wh_with_ua(Some(2.0)).validate().is_ok());
        assert!(resistance_wh_with_ua(Some(2.0)).validate().is_ok());
        assert!(hpwh_with_ua(Some(2.0)).validate().is_ok());
        assert!(indirect_tank_with_ua(Some(2.0)).validate().is_ok());
        assert!(indirect_tank_with_hx_ua(Some(2.0)).validate().is_ok());
    }

    // --- setpoint validation helpers ---

    fn gas_wh_with_setpoint(setpoint_c: Option<f64>) -> GasWaterHeaterConfig {
        GasWaterHeaterConfig {
            fan_power_w: None,
            fuel_type: FuelType::Gas,
            setpoint_c,
            equipment_id: None,
            zone_id: None,
            loop_id: None,
            tank_volume_m3: None,
            tank_height_m: None,
            energy_factor: None,
            uniform_energy_factor: None,
            heating_capacity_w: None,
            ua_w_per_k: None,
            deadband_c: None,
            max_tank_temp_c: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: None,
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            pilot_power_w: None,
            flue_loss_fraction: None,
            skin_loss_fraction: None,
            ignition_type: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            jacket_r_value_m2_k_w: None,
            conversion_efficiency: None,
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
            pilot_fraction_to_tank: None,
        }
    }

    fn resistance_wh_with_setpoint(setpoint_c: Option<f64>) -> ElectricResistanceWaterHeaterConfig {
        ElectricResistanceWaterHeaterConfig {
            setpoint_c,
            equipment_id: None,
            zone_id: None,
            loop_id: None,
            tank_volume_m3: None,
            tank_height_m: None,
            energy_factor: None,
            uniform_energy_factor: None,
            heating_capacity_w: None,
            ua_w_per_k: None,
            deadband_c: None,
            max_tank_temp_c: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: None,
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            element_power_w: None,
            max_setpoint_ramp_rate_c_per_min: None,
            element_priority_mode: None,
            jacket_r_value_m2_k_w: None,
            max_combined_power_w: None,
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
        }
    }

    fn hpwh_with_setpoint(setpoint_c: Option<f64>) -> HeatPumpWaterHeaterConfig {
        HeatPumpWaterHeaterConfig {
            setpoint_c,
            equipment_id: None,
            zone_id: None,
            loop_id: None,
            tank_volume_m3: None,
            tank_height_m: None,
            cop: None,
            backup_element_power_w: None,
            ua_w_per_k: None,
            deadband_c: None,
            max_tank_temp_c: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            tempering_valve_setpoint_c: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: None,
            compressor_power_w: None,
            backup_enable_offset_c: None,
            min_ambient_temp_c: None,
            max_ambient_temp_c: None,
            min_on_time_s: None,
            min_off_time_s: None,
            hp_only_mode: None,
            element_hp_control_mode: None,
            fan_power_w: None,
            parasitic_power_w: None,
            backup_efficiency: None,
            shr: None,
            lost_heat_fraction: None,
            wall_heat_fraction: None,
            capacity_biquadratic_coeffs: None,
            cop_biquadratic_coeffs: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            jacket_r_value_m2_k_w: None,
            fixture_delivery_temp_c: None,
            low_power_hpwh: None,
            uniform_energy_factor: None,
        }
    }

    fn indirect_tank_with_setpoint(setpoint_c: Option<f64>) -> IndirectTankConfig {
        IndirectTankConfig {
            setpoint_c,
            equipment_id: None,
            zone_id: None,
            boiler_loop_id: None,
            tank_volume_m3: None,
            tank_height_m: None,
            ua_w_per_k: None,
            hx_ua_w_per_k: None,
            deadband_c: None,
            max_tank_temp_c: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            draw_flow_rate_kg_s: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            jacket_r_value_m2_k_w: None,
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
            boiler_loop_flow_rate_kg_s: None,
        }
    }

    // --- setpoint validation tests ---

    #[test]
    fn setpoint_within_range_accepted() {
        // Residential safe zone: 49–60°C (120–140°F).
        // Extended range 40–85°C accommodates commercial/industrial WHs.
        for sp in [40.0, 51.67, 60.0, 85.0] {
            assert!(gas_wh_with_setpoint(Some(sp)).validate().is_ok());
            assert!(resistance_wh_with_setpoint(Some(sp)).validate().is_ok());
            assert!(hpwh_with_setpoint(Some(sp)).validate().is_ok());
            assert!(indirect_tank_with_setpoint(Some(sp)).validate().is_ok());
        }
    }

    #[test]
    fn setpoint_below_range_rejected() {
        for sp in [39.0, 0.0, -273.15] {
            assert!(gas_wh_with_setpoint(Some(sp)).validate().is_err());
            assert!(resistance_wh_with_setpoint(Some(sp)).validate().is_err());
            assert!(hpwh_with_setpoint(Some(sp)).validate().is_err());
            assert!(indirect_tank_with_setpoint(Some(sp)).validate().is_err());
        }
    }

    #[test]
    fn setpoint_above_range_rejected() {
        for sp in [86.0, 100.0] {
            assert!(gas_wh_with_setpoint(Some(sp)).validate().is_err());
            assert!(resistance_wh_with_setpoint(Some(sp)).validate().is_err());
            assert!(hpwh_with_setpoint(Some(sp)).validate().is_err());
            assert!(indirect_tank_with_setpoint(Some(sp)).validate().is_err());
        }
    }

    #[test]
    fn setpoint_non_finite_rejected() {
        for sp in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(gas_wh_with_setpoint(Some(sp)).validate().is_err());
            assert!(resistance_wh_with_setpoint(Some(sp)).validate().is_err());
            assert!(hpwh_with_setpoint(Some(sp)).validate().is_err());
            assert!(indirect_tank_with_setpoint(Some(sp)).validate().is_err());
        }
    }

    #[test]
    fn setpoint_none_accepted() {
        // setpoint_c is optional; None should pass validation.
        assert!(gas_wh_with_setpoint(None).validate().is_ok());
        assert!(resistance_wh_with_setpoint(None).validate().is_ok());
        assert!(hpwh_with_setpoint(None).validate().is_ok());
        assert!(indirect_tank_with_setpoint(None).validate().is_ok());
    }

    // --- setpoint vs max_tank_temp cross-field validation helpers ---

    fn gas_wh_with_sp_max_db(
        setpoint_c: Option<f64>,
        max_tank_temp_c: Option<f64>,
        deadband_c: Option<f64>,
    ) -> GasWaterHeaterConfig {
        GasWaterHeaterConfig {
            fan_power_w: None,
            fuel_type: FuelType::Gas,
            setpoint_c,
            deadband_c,
            max_tank_temp_c,
            equipment_id: None,
            zone_id: None,
            loop_id: None,
            tank_volume_m3: None,
            tank_height_m: None,
            energy_factor: None,
            uniform_energy_factor: None,
            heating_capacity_w: None,
            ua_w_per_k: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: None,
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            pilot_power_w: None,
            flue_loss_fraction: None,
            skin_loss_fraction: None,
            ignition_type: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            jacket_r_value_m2_k_w: None,
            conversion_efficiency: None,
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
            pilot_fraction_to_tank: None,
        }
    }

    fn resistance_wh_with_sp_max_db(
        setpoint_c: Option<f64>,
        max_tank_temp_c: Option<f64>,
        deadband_c: Option<f64>,
    ) -> ElectricResistanceWaterHeaterConfig {
        ElectricResistanceWaterHeaterConfig {
            setpoint_c,
            deadband_c,
            max_tank_temp_c,
            equipment_id: None,
            zone_id: None,
            loop_id: None,
            tank_volume_m3: None,
            tank_height_m: None,
            energy_factor: None,
            uniform_energy_factor: None,
            heating_capacity_w: None,
            ua_w_per_k: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: None,
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            element_power_w: None,
            max_setpoint_ramp_rate_c_per_min: None,
            element_priority_mode: None,
            jacket_r_value_m2_k_w: None,
            max_combined_power_w: None,
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
        }
    }

    fn hpwh_with_sp_max_db(
        setpoint_c: Option<f64>,
        max_tank_temp_c: Option<f64>,
        deadband_c: Option<f64>,
    ) -> HeatPumpWaterHeaterConfig {
        HeatPumpWaterHeaterConfig {
            setpoint_c,
            deadband_c,
            max_tank_temp_c,
            equipment_id: None,
            zone_id: None,
            loop_id: None,
            tank_volume_m3: None,
            tank_height_m: None,
            cop: None,
            backup_element_power_w: None,
            ua_w_per_k: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            tempering_valve_setpoint_c: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: None,
            compressor_power_w: None,
            backup_enable_offset_c: None,
            min_ambient_temp_c: None,
            max_ambient_temp_c: None,
            min_on_time_s: None,
            min_off_time_s: None,
            hp_only_mode: None,
            element_hp_control_mode: None,
            fan_power_w: None,
            parasitic_power_w: None,
            backup_efficiency: None,
            shr: None,
            lost_heat_fraction: None,
            wall_heat_fraction: None,
            capacity_biquadratic_coeffs: None,
            cop_biquadratic_coeffs: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            jacket_r_value_m2_k_w: None,
            fixture_delivery_temp_c: None,
            low_power_hpwh: None,
            uniform_energy_factor: None,
        }
    }

    fn indirect_tank_with_sp_max_db(
        setpoint_c: Option<f64>,
        max_tank_temp_c: Option<f64>,
        deadband_c: Option<f64>,
    ) -> IndirectTankConfig {
        IndirectTankConfig {
            setpoint_c,
            deadband_c,
            max_tank_temp_c,
            equipment_id: None,
            zone_id: None,
            boiler_loop_id: None,
            tank_volume_m3: None,
            tank_height_m: None,
            ua_w_per_k: None,
            hx_ua_w_per_k: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            draw_flow_rate_kg_s: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            jacket_r_value_m2_k_w: None,
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
            boiler_loop_flow_rate_kg_s: None,
        }
    }

    // --- setpoint vs max_tank_temp cross-field validation tests ---

    #[test]
    fn setpoint_above_max_tank_temp_rejected() {
        assert!(
            gas_wh_with_sp_max_db(Some(70.0), Some(60.0), None)
                .validate()
                .is_err()
        );
        assert!(
            resistance_wh_with_sp_max_db(Some(70.0), Some(60.0), None)
                .validate()
                .is_err()
        );
        assert!(
            hpwh_with_sp_max_db(Some(70.0), Some(60.0), None)
                .validate()
                .is_err()
        );
        assert!(
            indirect_tank_with_sp_max_db(Some(70.0), Some(60.0), None)
                .validate()
                .is_err()
        );
    }

    #[test]
    fn setpoint_equal_to_max_tank_temp_rejected() {
        assert!(
            gas_wh_with_sp_max_db(Some(60.0), Some(60.0), None)
                .validate()
                .is_err()
        );
        assert!(
            resistance_wh_with_sp_max_db(Some(60.0), Some(60.0), None)
                .validate()
                .is_err()
        );
        assert!(
            hpwh_with_sp_max_db(Some(60.0), Some(60.0), None)
                .validate()
                .is_err()
        );
        assert!(
            indirect_tank_with_sp_max_db(Some(60.0), Some(60.0), None)
                .validate()
                .is_err()
        );
    }

    #[test]
    fn setpoint_at_max_minus_deadband_rejected() {
        // sp=55.0, max=60.0, db=5.0 => sp == max-db => zero-width deadband => reject
        assert!(
            gas_wh_with_sp_max_db(Some(55.0), Some(60.0), Some(5.0))
                .validate()
                .is_err()
        );
        assert!(
            resistance_wh_with_sp_max_db(Some(55.0), Some(60.0), Some(5.0))
                .validate()
                .is_err()
        );
        assert!(
            hpwh_with_sp_max_db(Some(55.0), Some(60.0), Some(5.0))
                .validate()
                .is_err()
        );
        assert!(
            indirect_tank_with_sp_max_db(Some(55.0), Some(60.0), Some(5.0))
                .validate()
                .is_err()
        );
    }

    #[test]
    fn setpoint_below_max_minus_deadband_accepted() {
        // sp=54.9, max=60.0, db=5.0 => sp < max-db => adequate deadband => pass
        assert!(
            gas_wh_with_sp_max_db(Some(54.9), Some(60.0), Some(5.0))
                .validate()
                .is_ok()
        );
        assert!(
            resistance_wh_with_sp_max_db(Some(54.9), Some(60.0), Some(5.0))
                .validate()
                .is_ok()
        );
        assert!(
            hpwh_with_sp_max_db(Some(54.9), Some(60.0), Some(5.0))
                .validate()
                .is_ok()
        );
        assert!(
            indirect_tank_with_sp_max_db(Some(54.9), Some(60.0), Some(5.0))
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn setpoint_below_max_no_deadband_accepted() {
        assert!(
            gas_wh_with_sp_max_db(Some(55.0), Some(60.0), None)
                .validate()
                .is_ok()
        );
        assert!(
            resistance_wh_with_sp_max_db(Some(55.0), Some(60.0), None)
                .validate()
                .is_ok()
        );
        assert!(
            hpwh_with_sp_max_db(Some(55.0), Some(60.0), None)
                .validate()
                .is_ok()
        );
        assert!(
            indirect_tank_with_sp_max_db(Some(55.0), Some(60.0), None)
                .validate()
                .is_ok()
        );
    }
}
