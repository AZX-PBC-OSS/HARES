//! Typed configuration structs for heating equipment.

use hares_types::FluidType;
use serde::{Deserialize, Serialize};

use crate::config::EquipmentTypedConfig;

/// Shared duct distribution parameters, flattened into heating config structs.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DuctConfig {
    /// Duct distribution system efficiency (DSE), dimensionless [0, 1].
    /// None means no duct losses (equipment in conditioned space or no ducts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dse_heat: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dse_cool: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GasFurnaceConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    /// Annual fuel utilization efficiency (AFUE), dimensionless [0, 1].
    pub afue: f64,
    /// Rated heating capacity in watts.
    pub heating_capacity_w: f64,
    /// Number of compressor/burner speeds (1 or 2).
    #[serde(default = "default_one")]
    pub number_of_speeds: u8,
    /// Fan power in watts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_power_w: Option<f64>,
    #[serde(flatten)]
    pub ducts: DuctConfig,
}

impl EquipmentTypedConfig for GasFurnaceConfig {
    fn equipment_type_name() -> &'static str {
        "Gas Furnace"
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElectricFurnaceConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    /// Coefficient of performance (COP), dimensionless (typically 1.0 for resistance).
    pub heating_efficiency: f64,
    pub heating_capacity_w: f64,
    #[serde(default = "default_one")]
    pub number_of_speeds: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_power_w: Option<f64>,
    #[serde(flatten)]
    pub ducts: DuctConfig,
}

impl EquipmentTypedConfig for ElectricFurnaceConfig {
    fn equipment_type_name() -> &'static str {
        "Electric Furnace"
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GasBoilerConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    pub loop_id: Option<u16>,
    pub afue: f64,
    pub heating_capacity_w: f64,
    #[serde(default = "default_one")]
    pub number_of_speeds: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_power_w: Option<f64>,
    /// Hydronic loop flow rate [kg/s]. Defaults to 0.5 kg/s.
    #[serde(default = "default_flow_rate_kg_s")]
    pub flow_rate_kg_s: f64,
    /// Hydronic loop return water temperature [C]. Defaults to 40 °C (104 °F).
    #[serde(default = "default_return_temp_c")]
    pub return_temp_c: f64,
    #[serde(default = "default_fluid_type")]
    pub fluid_type: FluidType,
}

impl EquipmentTypedConfig for GasBoilerConfig {
    fn equipment_type_name() -> &'static str {
        "Gas Boiler"
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElectricBoilerConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    pub loop_id: Option<u16>,
    pub heating_efficiency: f64,
    pub heating_capacity_w: f64,
    #[serde(default = "default_one")]
    pub number_of_speeds: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan_power_w: Option<f64>,
    /// Hydronic loop flow rate [kg/s]. Defaults to 0.5 kg/s.
    #[serde(default = "default_flow_rate_kg_s")]
    pub flow_rate_kg_s: f64,
    /// Hydronic loop return water temperature [C]. Defaults to 40 °C (104 °F).
    #[serde(default = "default_return_temp_c")]
    pub return_temp_c: f64,
    #[serde(default = "default_fluid_type")]
    pub fluid_type: FluidType,
}

impl EquipmentTypedConfig for ElectricBoilerConfig {
    fn equipment_type_name() -> &'static str {
        "Electric Boiler"
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElectricBaseboardConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    pub heating_capacity_w: f64,
}

impl EquipmentTypedConfig for ElectricBaseboardConfig {
    fn equipment_type_name() -> &'static str {
        "Electric Baseboard"
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdealHvacConfig {
    pub equipment_id: Option<u32>,
    pub zone_id: Option<u16>,
    pub heating_capacity_w: f64,
    pub cooling_capacity_w: f64,
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

    fn typed_config<T: EquipmentTypedConfig>(config: T) -> EquipmentConfig {
        EquipmentConfig::from_typed(
            "test".to_string(),
            T::equipment_type_name().to_string(),
            config,
        )
        .unwrap()
    }

    #[test]
    fn gas_furnace_config_round_trips() {
        let cfg = GasFurnaceConfig {
            equipment_id: Some(1),
            zone_id: Some(1),
            afue: 0.96,
            heating_capacity_w: 12_000.0,
            number_of_speeds: 1,
            fan_power_w: Some(300.0),
            ducts: DuctConfig {
                dse_heat: Some(0.8),
                dse_cool: None,
            },
        };
        let ec = typed_config(cfg.clone());
        assert!(ec.is_typed());
        let recovered: GasFurnaceConfig = ec.typed().unwrap();
        assert!((recovered.afue - cfg.afue).abs() < 1e-12);
        assert!((recovered.heating_capacity_w - cfg.heating_capacity_w).abs() < 1e-12);
        assert_eq!(recovered.ducts.dse_heat, cfg.ducts.dse_heat);
    }

    #[test]
    fn electric_furnace_config_round_trips() {
        let cfg = ElectricFurnaceConfig {
            equipment_id: None,
            zone_id: Some(2),
            heating_efficiency: 1.0,
            heating_capacity_w: 8_000.0,
            number_of_speeds: 1,
            fan_power_w: None,
            ducts: DuctConfig::default(),
        };
        let ec = typed_config(cfg.clone());
        let recovered: ElectricFurnaceConfig = ec.typed().unwrap();
        assert!((recovered.heating_capacity_w - cfg.heating_capacity_w).abs() < 1e-12);
    }

    #[test]
    fn gas_boiler_config_round_trips() {
        let cfg = GasBoilerConfig {
            equipment_id: None,
            zone_id: Some(1),
            loop_id: Some(1),
            afue: 0.85,
            heating_capacity_w: 15_000.0,
            number_of_speeds: 1,
            fan_power_w: None,
            flow_rate_kg_s: 0.5,
            return_temp_c: 40.0,
            fluid_type: FluidType::Water,
        };
        let ec = typed_config(cfg.clone());
        let recovered: GasBoilerConfig = ec.typed().unwrap();
        assert!((recovered.afue - cfg.afue).abs() < 1e-12);
        assert!((recovered.flow_rate_kg_s - cfg.flow_rate_kg_s).abs() < 1e-12);
    }

    #[test]
    fn electric_boiler_config_round_trips() {
        let cfg = ElectricBoilerConfig {
            equipment_id: None,
            zone_id: Some(1),
            loop_id: Some(1),
            heating_efficiency: 1.0,
            heating_capacity_w: 10_000.0,
            number_of_speeds: 1,
            fan_power_w: None,
            flow_rate_kg_s: 0.5,
            return_temp_c: 40.0,
            fluid_type: FluidType::Water,
        };
        let ec = typed_config(cfg.clone());
        let recovered: ElectricBoilerConfig = ec.typed().unwrap();
        assert!((recovered.heating_capacity_w - cfg.heating_capacity_w).abs() < 1e-12);
        assert!((recovered.flow_rate_kg_s - cfg.flow_rate_kg_s).abs() < 1e-12);
    }

    #[test]
    fn electric_baseboard_config_round_trips() {
        let cfg = ElectricBaseboardConfig {
            equipment_id: None,
            zone_id: Some(1),
            heating_capacity_w: 5_000.0,
        };
        let ec = typed_config(cfg.clone());
        let recovered: ElectricBaseboardConfig = ec.typed().unwrap();
        assert!((recovered.heating_capacity_w - cfg.heating_capacity_w).abs() < 1e-12);
    }

    #[test]
    fn ideal_hvac_config_round_trips() {
        let cfg = IdealHvacConfig {
            equipment_id: Some(5),
            zone_id: Some(1),
            heating_capacity_w: 10_000.0,
            cooling_capacity_w: 8_000.0,
        };
        let ec = typed_config(cfg.clone());
        let recovered: IdealHvacConfig = ec.typed().unwrap();
        assert!((recovered.cooling_capacity_w - cfg.cooling_capacity_w).abs() < 1e-12);
    }

    #[test]
    fn gas_furnace_rejects_unknown_fields() {
        let data = serde_json::json!({
            "afue": 0.96,
            "heating_capacity_w": 12000.0,
            "unknown_key": true,
        });
        let ec = EquipmentConfig {
            name: "test".to_string(),
            ochre_class: "Gas Furnace".to_string(),
            payload: ConfigPayload::Typed {
                type_name: "Gas Furnace".to_string(),
                version: 1,
                data,
            },
        };
        let result: crate::Result<GasFurnaceConfig> = ec.typed();
        assert!(result.is_err());
    }

    #[test]
    fn number_of_speeds_defaults_to_one() {
        let json = serde_json::json!({
            "afue": 0.96,
            "heating_capacity_w": 12000.0,
        });
        let cfg: GasFurnaceConfig = serde_json::from_value(json).unwrap();
        assert_eq!(cfg.number_of_speeds, 1);
    }

    fn minimal_env() -> hares_types::EnvironmentState {
        use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
        use hares_types::{GridState, WeatherState, ZoneId, ZoneState};
        hares_types::EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: 18.0,
                humidity_ratio: 0.008,
                relative_humidity: 0.45,
                wet_bulb_c: 14.0,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 5.0,
                outdoor_humidity_ratio: 0.003,
                pressure_kpa: 101.325,
                ..WeatherState::default()
            },
            grid: GridState { voltage_pu: 1.0, frequency_hz: 60.0 },
            custom_domains: vec![],
            equipment_telemetry: std::collections::HashMap::new(),
            current_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                .single()
                .expect("valid"),
            time_res: ChronoDuration::minutes(1),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    fn heating_ports() -> hares_types::PortSlots {
        use hares_types::{ZoneId, PortSlots, ThermalAccumulator};
        PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        }
    }

    /// Force duty cycle to 1.0 by inserting a heating control signal so the
    /// equipment is fully on during step. Equipment must have been init'd and
    /// its thermostat configured to call for heat (zone < heating setpoint).
    fn run_step_at_full_duty(
        eq: &mut dyn crate::Equipment,
        env: &hares_types::EnvironmentState,
        ports: &mut hares_types::PortSlots,
    ) {
        use hares_types::ControlSignal;
        let _ = eq.apply_control_unchecked(&ControlSignal::IdealCapacity { capacity_w: f64::MAX });
        eq.update_control(env);
        eq.step(env, std::time::Duration::from_secs(60), ports).unwrap();
    }

    #[test]
    fn gas_furnace_typed_init_produces_thermal_output() {
        use crate::Equipment;
        use crate::hvac::furnace::GasFurnace;
        use hares_types::telemetry_keys as tk;
        let cfg = GasFurnaceConfig {
            equipment_id: Some(1),
            zone_id: Some(1),
            afue: 0.96,
            heating_capacity_w: 12_000.0,
            number_of_speeds: 1,
            fan_power_w: Some(0.0),
            ducts: DuctConfig::default(),
        };
        let ec = typed_config(cfg);
        let mut eq = GasFurnace::new(ec.clone());
        let env = minimal_env();
        eq.init(&ec, &env).unwrap();
        let mut ports = heating_ports();
        run_step_at_full_duty(&mut eq, &env, &mut ports);
        let thermal_w = eq.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap_or(0.0);
        let fuel_w = eq.telemetry().get(tk::FUEL_INPUT_W).unwrap_or(0.0);
        assert!(thermal_w > 0.0, "thermal output must be positive after typed init");
        assert!(fuel_w > 0.0, "fuel input must be positive after typed init");
        assert!((fuel_w / thermal_w - 1.0 / 0.96).abs() < 0.01, "fuel/thermal ratio must match 1/AFUE");
    }

    #[test]
    fn electric_furnace_typed_init_produces_power() {
        use crate::Equipment;
        use crate::hvac::furnace::ElectricFurnace;
        use hares_types::telemetry_keys as tk;
        let cfg = ElectricFurnaceConfig {
            equipment_id: None,
            zone_id: Some(1),
            heating_efficiency: 1.0,
            heating_capacity_w: 8_000.0,
            number_of_speeds: 1,
            fan_power_w: Some(0.0),
            ducts: DuctConfig::default(),
        };
        let ec = typed_config(cfg);
        let mut eq = ElectricFurnace::new(ec.clone());
        let env = minimal_env();
        eq.init(&ec, &env).unwrap();
        let mut ports = heating_ports();
        run_step_at_full_duty(&mut eq, &env, &mut ports);
        let electric_kw = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        assert!(electric_kw > 0.0, "electric power must be positive after typed init");
        assert!((electric_kw - 8.0).abs() < 0.5, "electric kw must match capacity / 1000 for eir=1");
    }

    #[test]
    fn electric_furnace_typed_init_rejects_zero_efficiency() {
        use crate::Equipment;
        use crate::hvac::furnace::ElectricFurnace;
        let cfg = ElectricFurnaceConfig {
            equipment_id: None,
            zone_id: Some(1),
            heating_efficiency: 0.0,
            heating_capacity_w: 8_000.0,
            number_of_speeds: 1,
            fan_power_w: None,
            ducts: DuctConfig::default(),
        };
        let ec = typed_config(cfg);
        let mut eq = ElectricFurnace::new(ec.clone());
        let env = minimal_env();
        assert!(eq.init(&ec, &env).is_err());
    }

    #[test]
    fn electric_boiler_typed_init_writes_fluid_port() {
        use crate::Equipment;
        use crate::hvac::boiler::ElectricBoiler;
        use hares_types::{FluidAccumulator, LoopId, PortSlots, ThermalAccumulator, ZoneId};
        let cfg = ElectricBoilerConfig {
            equipment_id: None,
            zone_id: Some(1),
            loop_id: Some(1),
            heating_efficiency: 0.98,
            heating_capacity_w: 10_000.0,
            number_of_speeds: 1,
            fan_power_w: None,
            flow_rate_kg_s: 0.6,
            return_temp_c: 40.0,
            fluid_type: FluidType::Water,
        };
        let ec = typed_config(cfg);
        let mut eq = ElectricBoiler::new(ec.clone());
        let env = minimal_env();
        eq.init(&ec, &env).unwrap();
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            fluid: vec![FluidAccumulator::new(LoopId(1), FluidType::Water)],
            ..PortSlots::default()
        };
        run_step_at_full_duty(&mut eq, &env, &mut ports);
        assert!(ports.fluid[0].total_flow_kg_s > 0.0, "fluid flow must be positive after typed init");
    }

    #[test]
    fn gas_boiler_typed_init_writes_fluid_port() {
        use crate::Equipment;
        use crate::hvac::boiler::GasBoiler;
        use hares_types::{FluidAccumulator, LoopId, PortSlots, ThermalAccumulator, ZoneId};
        let cfg = GasBoilerConfig {
            equipment_id: None,
            zone_id: Some(1),
            loop_id: Some(1),
            afue: 0.85,
            heating_capacity_w: 15_000.0,
            number_of_speeds: 1,
            fan_power_w: None,
            flow_rate_kg_s: 0.5,
            return_temp_c: 40.0,
            fluid_type: FluidType::Water,
        };
        let ec = typed_config(cfg);
        let mut eq = GasBoiler::new(ec.clone());
        let env = minimal_env();
        eq.init(&ec, &env).unwrap();
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            fluid: vec![FluidAccumulator::new(LoopId(1), FluidType::Water)],
            ..PortSlots::default()
        };
        run_step_at_full_duty(&mut eq, &env, &mut ports);
        assert!(ports.fluid[0].total_flow_kg_s > 0.0, "fluid flow must be positive after typed init");
    }

    #[test]
    fn electric_baseboard_typed_init_produces_power() {
        use crate::Equipment;
        use crate::hvac::baseboard::ElectricBaseboard;
        use hares_types::telemetry_keys as tk;
        let cfg = ElectricBaseboardConfig {
            equipment_id: None,
            zone_id: Some(1),
            heating_capacity_w: 5_000.0,
        };
        let ec = typed_config(cfg);
        let mut eq = ElectricBaseboard::new(ec.clone());
        let env = minimal_env();
        eq.init(&ec, &env).unwrap();
        let mut ports = heating_ports();
        run_step_at_full_duty(&mut eq, &env, &mut ports);
        let electric_kw = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        assert!(electric_kw > 0.0, "electric power must be positive after typed init");
        assert!((electric_kw - 5.0).abs() < 0.5, "electric kw must match 5 kW capacity");
    }

    #[test]
    fn ideal_hvac_typed_init_succeeds() {
        use crate::Equipment;
        use crate::hvac::ideal_hvac::IdealHvac;
        let cfg = IdealHvacConfig {
            equipment_id: None,
            zone_id: Some(1),
            heating_capacity_w: 10_000.0,
            cooling_capacity_w: 8_000.0,
        };
        let ec = typed_config(cfg);
        let mut eq = IdealHvac::new(ec.clone());
        let env = minimal_env();
        eq.init(&ec, &env).unwrap();
        // Verify descriptor reflects the zone from the typed config.
        assert_eq!(eq.descriptor().zone, Some(hares_types::ZoneId(1)));
    }
}
