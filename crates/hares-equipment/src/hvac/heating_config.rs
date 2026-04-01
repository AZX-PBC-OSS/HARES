//! Typed configuration structs for heating equipment.

use hares_types::{FluidType, ScheduleSourceConfig};
use serde::{Deserialize, Serialize};

use crate::config::EquipmentTypedConfig;

pub use super::core_config::DuctConfig;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GasFurnaceConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    /// Required; resolver errors if absent.
    pub capacity_w: f64,
    /// Required; resolver errors if absent.
    pub afue: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_power_w: Option<f64>,
    #[serde(default = "default_one")]
    pub number_of_speeds: u8,
    #[serde(flatten)]
    pub ducts: DuctConfig,
}

impl EquipmentTypedConfig for GasFurnaceConfig {
    fn equipment_type_name() -> &'static str {
        "Gas Furnace"
    }
}

impl Default for GasFurnaceConfig {
    fn default() -> Self {
        Self {
            equipment_id: None,
            zone_id: None,
            capacity_w: 0.0,
            afue: 0.80,
            fan_power_w: None,
            number_of_speeds: 1,
            ducts: DuctConfig::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElectricFurnaceConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    /// Required; resolver errors if absent.
    pub capacity_w: f64,
    /// Electric input ratio [W/W] = electric input power divided by delivered
    /// thermal output. Unity is ideal resistive conversion.
    pub eir: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_power_w: Option<f64>,
    #[serde(default = "default_one")]
    pub number_of_speeds: u8,
    #[serde(flatten)]
    pub ducts: DuctConfig,
}

impl EquipmentTypedConfig for ElectricFurnaceConfig {
    fn equipment_type_name() -> &'static str {
        "Electric Furnace"
    }
}

impl Default for ElectricFurnaceConfig {
    fn default() -> Self {
        Self {
            equipment_id: None,
            zone_id: None,
            capacity_w: 0.0,
            eir: 1.0,
            fan_power_w: None,
            number_of_speeds: 1,
            ducts: DuctConfig::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GasBoilerConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    pub loop_id: Option<u16>,
    /// Required; resolver errors if absent.
    pub capacity_w: f64,
    /// Required; resolver errors if absent.
    pub afue: f64,
    #[serde(default = "default_flow_rate_kg_s")]
    pub flow_rate_kg_s: f64,
    #[serde(default = "default_return_temp_c")]
    pub return_temp_c: f64,
    #[serde(default = "default_fluid_type")]
    pub fluid_type: FluidType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_power_w: Option<f64>,
    #[serde(default = "default_one")]
    pub number_of_speeds: u8,
}

impl EquipmentTypedConfig for GasBoilerConfig {
    fn equipment_type_name() -> &'static str {
        "Gas Boiler"
    }
}

impl Default for GasBoilerConfig {
    fn default() -> Self {
        Self {
            equipment_id: None,
            zone_id: None,
            loop_id: None,
            capacity_w: 0.0,
            afue: 0.80,
            flow_rate_kg_s: default_flow_rate_kg_s(),
            return_temp_c: default_return_temp_c(),
            fluid_type: default_fluid_type(),
            fan_power_w: None,
            number_of_speeds: 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElectricBoilerConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    pub loop_id: Option<u16>,
    /// Required; resolver errors if absent.
    pub capacity_w: f64,
    /// Electric input ratio [W/W] = electric input power divided by delivered
    /// thermal output. Unity is ideal resistive conversion.
    pub eir: f64,
    #[serde(default = "default_flow_rate_kg_s")]
    pub flow_rate_kg_s: f64,
    #[serde(default = "default_return_temp_c")]
    pub return_temp_c: f64,
    #[serde(default = "default_fluid_type")]
    pub fluid_type: FluidType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_power_w: Option<f64>,
    #[serde(default = "default_one")]
    pub number_of_speeds: u8,
}

impl EquipmentTypedConfig for ElectricBoilerConfig {
    fn equipment_type_name() -> &'static str {
        "Electric Boiler"
    }
}

impl Default for ElectricBoilerConfig {
    fn default() -> Self {
        Self {
            equipment_id: None,
            zone_id: None,
            loop_id: None,
            capacity_w: 0.0,
            eir: 1.0,
            flow_rate_kg_s: default_flow_rate_kg_s(),
            return_temp_c: default_return_temp_c(),
            fluid_type: default_fluid_type(),
            fan_power_w: None,
            number_of_speeds: 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElectricBaseboardConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    /// Required; resolver errors if absent.
    pub capacity_w: f64,
    /// Electric input ratio [W/W] = electric input power divided by delivered
    /// thermal output. Unity is ideal resistive conversion.
    pub eir: f64,
}

impl EquipmentTypedConfig for ElectricBaseboardConfig {
    fn equipment_type_name() -> &'static str {
        "Electric Baseboard"
    }
}

impl Default for ElectricBaseboardConfig {
    fn default() -> Self {
        Self {
            equipment_id: None,
            zone_id: None,
            capacity_w: 0.0,
            eir: 1.0,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdealHvacConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heating_setpoint_c: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooling_setpoint_c: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadband_c: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n_speeds: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ideal_capacity_mode: Option<IdealCapacityModeConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heating_setpoint_source: Option<ScheduleSourceConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooling_setpoint_source: Option<ScheduleSourceConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heating_capacity_w: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooling_capacity_w: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heating_eir: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooling_eir: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shr: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction_heating_load_served: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction_cooling_load_served: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IdealCapacityModeConfig {
    Auto,
    On,
    Off,
}

impl EquipmentTypedConfig for IdealHvacConfig {
    fn equipment_type_name() -> &'static str {
        "Ideal HVAC"
    }
}

fn default_one() -> u8 {
    1
}

fn default_flow_rate_kg_s() -> f64 {
    0.5
}

fn default_return_temp_c() -> f64 {
    40.0
}

fn default_fluid_type() -> FluidType {
    FluidType::Water
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConfigPayload, EquipmentConfig};

    #[test]
    fn all_configs_round_trip_via_serde_json() {
        let gf = GasFurnaceConfig {
            capacity_w: 12_000.0,
            afue: 0.95,
            ducts: DuctConfig {
                dse_heat: Some(0.85),
                ..DuctConfig::default()
            },
            ..GasFurnaceConfig::default()
        };
        let ef = ElectricFurnaceConfig {
            capacity_w: 8_000.0,
            eir: 1.0,
            ..ElectricFurnaceConfig::default()
        };
        let gb = GasBoilerConfig {
            capacity_w: 15_000.0,
            afue: 0.88,
            ..GasBoilerConfig::default()
        };
        let eb = ElectricBoilerConfig {
            capacity_w: 10_000.0,
            eir: 0.98,
            ..ElectricBoilerConfig::default()
        };
        let bb = ElectricBaseboardConfig {
            capacity_w: 5_000.0,
            eir: 1.0,
            ..ElectricBaseboardConfig::default()
        };
        let ih = IdealHvacConfig {
            heating_capacity_w: Some(10_000.0),
            cooling_capacity_w: Some(8_000.0),
            heating_eir: Some(1.0),
            cooling_eir: Some(1.0),
            shr: Some(0.75),
            ..IdealHvacConfig::default()
        };

        fn assert_round_trip<T>(config: T)
        where
            T: EquipmentTypedConfig,
        {
            let value = serde_json::to_value(&config).unwrap();
            let recovered: T = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(recovered).unwrap(), value);
        }

        assert_round_trip(gf);
        assert_round_trip(ef);
        assert_round_trip(gb);
        assert_round_trip(eb);
        assert_round_trip(bb);
        assert_round_trip(ih);
    }

    #[test]
    fn gas_furnace_rejects_unknown_fields_with_key_name() {
        let data = serde_json::json!({
            "capacity_w": 12000.0,
            "afue": 0.96,
            "unknown_key": true,
        });
        let ec = EquipmentConfig::with_payload(
            "test".to_string(),
            "Gas Furnace".to_string(),
            ConfigPayload::Typed {
                type_name: "Gas Furnace".to_string(),
                version: 1,
                data,
            },
        );
        let result: crate::Result<GasFurnaceConfig> = ec.typed();
        let err = result.expect_err("unknown fields must be rejected");
        assert!(
            err.to_string().contains("unknown_key"),
            "error should name unknown key; got {err}"
        );
    }
}
