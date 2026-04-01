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
        check_finite("gas_wh: ua_w_per_k", self.ua_w_per_k, 0.0, false)?;
        check_finite(
            "gas_wh: setpoint_c",
            self.setpoint_c,
            f64::NEG_INFINITY,
            false,
        )?;
        check_finite("gas_wh: deadband_c", self.deadband_c, 0.0, false)?;
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
        check_finite("resistance_wh: ua_w_per_k", self.ua_w_per_k, 0.0, false)?;
        check_finite(
            "resistance_wh: setpoint_c",
            self.setpoint_c,
            f64::NEG_INFINITY,
            false,
        )?;
        check_finite("resistance_wh: deadband_c", self.deadband_c, 0.0, false)?;
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
    /// Number of bedrooms (ANSI/RESNET 301 occupancy proxy).
    /// When provided, gas parasitic power is computed as `5 + 60 * on_time_frac`
    /// where `on_time_frac = [0.0269, 0.0333, 0.0397, 0.0462, 0.0529][bedrooms - 1]` (clamped 1–5).
    /// Ignored for electric tankless (no ignition controller).
    /// Overridden by `parasitic_power_w` if both are set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number_of_bedrooms: Option<f64>,
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
        check_range(
            "tankless_wh: number_of_bedrooms",
            self.number_of_bedrooms,
            0.5,
            10.0,
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
        check_finite("hpwh: ua_w_per_k", self.ua_w_per_k, 0.0, false)?;
        check_finite(
            "hpwh: setpoint_c",
            self.setpoint_c,
            f64::NEG_INFINITY,
            false,
        )?;
        check_finite("hpwh: deadband_c", self.deadband_c, 0.0, false)?;
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
        };
        let ec = EquipmentConfig::from_typed(
            "gas".to_string(),
            "Gas Water Heater".to_string(),
            cfg.clone(),
        );
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
        };
        let ec = EquipmentConfig::from_typed(
            "resistance".to_string(),
            "Electric Resistance Water Heater".to_string(),
            cfg.clone(),
        );
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
            number_of_bedrooms: None,
            performance_adjustment: Some(0.92),
            inlet_temp_c: None,
            draw_flow_rate_kg_s: None,
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            avg_water_draw_l_per_day: Some(227.0),
        };
        let ec = EquipmentConfig::from_typed(
            "tankless".to_string(),
            "Tankless Water Heater".to_string(),
            cfg.clone(),
        );
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
        };
        let ec = EquipmentConfig::from_typed(
            "hpwh".to_string(),
            "Heat Pump Water Heater".to_string(),
            cfg.clone(),
        );
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
}
