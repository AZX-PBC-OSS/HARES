//! Gas and electric furnace models.

use std::borrow::Cow;
use std::time::Duration;

use chrono::{DateTime, Utc};
use hares_types::{
    ControlCapabilities, ControlSignal, EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId,
    ExecutionStage, FuelType, HaresError, OperatingMode, PortContribution, PortDeclaration,
    PortSlots, PortType, Telemetry, TelemetryField, ZoneId,
};
use serde::{Deserialize, Serialize};

use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_postcard, save_postcard};

use super::{
    common::{HvacEquipment, HvacEquipmentType, RuntimeSetpointOverride, ThermostatMode},
    helpers::{
        DUCT_DSE_KEYS, HEATING_CAPACITY_KEYS, apply_heating_control_unchecked,
        equipment_id_from_config, first_f64, operating_mode_code, parse_fuel_type,
        update_heating_control, zone_id_from_config,
    },
};
use hares_physics::constants::{CFM_TO_M3_S, W_PER_TON};

/// Default gas furnace AFUE. DOE 10 CFR Part 430, federal minimum.
const DEFAULT_GAS_AFUE: f64 = 0.8;
const FURNACE_FAN_CFM_PER_TON: f64 = 400.0;
const FURNACE_AIRFLOW_M3_S_PER_W_HEATING: f64 = FURNACE_FAN_CFM_PER_TON * CFM_TO_M3_S / W_PER_TON;

pub struct ElectricFurnace {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,
    hvac: HvacEquipment,
    rated_capacity_w: f64,
    eir: f64,
    operating_mode: OperatingMode,
    run_time_s: f64,
}

pub struct GasFurnace {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,
    hvac: HvacEquipment,
    rated_capacity_w: f64,
    fuel_efficiency: f64,
    fan_power_w: f64,
    fuel_type: FuelType,
    operating_mode: OperatingMode,
    run_time_s: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct FurnaceState {
    mode: ThermostatMode,
    duty_cycle: f64,
    last_mode_switch_at: Option<DateTime<Utc>>,
    runtime_setpoints: Option<RuntimeSetpointOverride>,
    operating_mode: OperatingMode,
    run_time_s: f64,
    electric_kw: f64,
    thermal_output_w: f64,
    fuel_input_w: f64,
}

impl ElectricFurnace {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let zone = zone_id_from_config(&config).unwrap_or(ZoneId(1));
        let descriptor = EquipmentDescriptor {
            id: EquipmentId(equipment_id_from_config(&config).unwrap_or(0)),
            name: config.name,
            end_use: EndUse::HvacHeating,
            equipment_type: Cow::Borrowed("Electric Furnace"),
            zone: Some(zone),
            fuel: FuelType::Electric,
            stage: ExecutionStage::Thermal,
            control_capabilities: ControlCapabilities::THERMAL_SETPOINT,
            telemetry_fields: electric_furnace_telemetry_fields(),
        };

        Self {
            descriptor,
            ports: vec![
                PortDeclaration {
                    port_type: PortType::Electrical,
                    zone: None,
                    loop_id: None,
                    domain_id: None,
                },
                PortDeclaration {
                    port_type: PortType::Thermal,
                    zone: Some(zone),
                    loop_id: None,
                    domain_id: None,
                },
            ],
            telemetry: electric_furnace_default_telemetry(),
            hvac: HvacEquipment::new(HvacEquipmentType::ElectricFurnace, zone),
            rated_capacity_w: 0.0,
            eir: 1.0,
            operating_mode: OperatingMode::Off,
            run_time_s: 0.0,
        }
    }
}

impl Equipment for ElectricFurnace {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        self.hvac.init(config, env)?;
        self.hvac.duct_dse = first_f64(config, DUCT_DSE_KEYS).unwrap_or(1.0);
        self.hvac.duct_zone_id = super::helpers::parse_zone_id_key(config, "duct_zone_id");
        self.hvac.update_zone_heat_fractions();

        self.rated_capacity_w = first_f64(config, HEATING_CAPACITY_KEYS)
            .unwrap_or(10_000.0)
            .max(0.0);
        self.eir = first_f64(config, &["eir", "heating_eir", "EIR"]).unwrap_or(1.0);

        self.hvac.heating_capacities_w = vec![self.rated_capacity_w];
        self.hvac.eir_by_stage = vec![self.eir];
        self.operating_mode = OperatingMode::Off;
        self.run_time_s = 0.0;
        self.telemetry = electric_furnace_default_telemetry();
        Ok(())
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        self.operating_mode = update_heating_control(&mut self.hvac, env);
        self.operating_mode
    }

    fn step(
        &mut self,
        _env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        let duty = self.hvac.duty_cycle.clamp(0.0, 1.0);
        let gross_capacity_w = self.rated_capacity_w * duty;
        let electric_kw = (self.rated_capacity_w * self.eir * duty) / 1_000.0;

        if electric_kw > 0.0 {
            ports.accumulate(&PortContribution::Electrical {
                active_power_kw: electric_kw,
                reactive_power_kvar: 0.0,
            })?;
        }

        if gross_capacity_w > 0.0 {
            self.hvac
                .write_zone_thermal_contributions(ports, gross_capacity_w, 0.0)?;
        }

        if self.operating_mode == OperatingMode::Heating {
            self.run_time_s += dt.as_secs_f64();
        }

        // Telemetry reports delivered capacity (post-DSE) for the conditioned zone,
        // consistent with OCHRE's "thermal_output_w" reporting.
        let thermal_output_w = gross_capacity_w * self.hvac.duct_dse.clamp(0.0, 1.0);
        self.telemetry.set("electric_kw", electric_kw);
        self.telemetry.set("thermal_output_w", thermal_output_w);
        self.telemetry
            .set("operating_mode", operating_mode_code(self.operating_mode));
        self.telemetry
            .set("supply_air_temp_c", self.hvac.supply_air_temp_c);

        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn save_state(&self) -> Vec<u8> {
        save_postcard(&FurnaceState {
            mode: self.hvac.mode,
            duty_cycle: self.hvac.duty_cycle,
            last_mode_switch_at: self.hvac.last_mode_switch_at,
            runtime_setpoints: self.hvac.runtime_setpoints,
            operating_mode: self.operating_mode,
            run_time_s: self.run_time_s,
            electric_kw: self.telemetry.get("electric_kw").unwrap_or(0.0),
            thermal_output_w: self.telemetry.get("thermal_output_w").unwrap_or(0.0),
            fuel_input_w: 0.0,
        })
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let decoded: FurnaceState = load_postcard(state)?;
        self.hvac.mode = decoded.mode;
        self.hvac.duty_cycle = decoded.duty_cycle;
        self.hvac.last_mode_switch_at = decoded.last_mode_switch_at;
        self.hvac.runtime_setpoints = decoded.runtime_setpoints;
        self.operating_mode = decoded.operating_mode;
        self.run_time_s = decoded.run_time_s;
        self.telemetry.insert("electric_kw", decoded.electric_kw);
        self.telemetry
            .insert("thermal_output_w", decoded.thermal_output_w);
        self.telemetry.insert(
            "operating_mode",
            operating_mode_code(decoded.operating_mode),
        );
        self.telemetry
            .insert("supply_air_temp_c", self.hvac.supply_air_temp_c);
        Ok(())
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        apply_heating_control_unchecked(&mut self.hvac, signal, "Electric Furnace")
    }
}

impl GasFurnace {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let zone = zone_id_from_config(&config).unwrap_or(ZoneId(1));
        let descriptor = EquipmentDescriptor {
            id: EquipmentId(equipment_id_from_config(&config).unwrap_or(0)),
            name: config.name,
            end_use: EndUse::HvacHeating,
            equipment_type: Cow::Borrowed("Gas Furnace"),
            zone: Some(zone),
            fuel: FuelType::Gas,
            stage: ExecutionStage::Thermal,
            control_capabilities: ControlCapabilities::THERMAL_SETPOINT,
            telemetry_fields: gas_furnace_telemetry_fields(),
        };

        Self {
            descriptor,
            ports: vec![
                PortDeclaration {
                    port_type: PortType::Fuel,
                    zone: None,
                    loop_id: None,
                    domain_id: None,
                },
                PortDeclaration {
                    port_type: PortType::Electrical,
                    zone: None,
                    loop_id: None,
                    domain_id: None,
                },
                PortDeclaration {
                    port_type: PortType::Thermal,
                    zone: Some(zone),
                    loop_id: None,
                    domain_id: None,
                },
            ],
            telemetry: gas_furnace_default_telemetry(),
            hvac: HvacEquipment::new(HvacEquipmentType::GasFurnace, zone),
            rated_capacity_w: 0.0,
            fuel_efficiency: DEFAULT_GAS_AFUE,
            fan_power_w: 0.0,
            fuel_type: FuelType::Gas,
            operating_mode: OperatingMode::Off,
            run_time_s: 0.0,
        }
    }
}

impl Equipment for GasFurnace {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        self.hvac.init(config, env)?;
        self.hvac.duct_dse = first_f64(config, DUCT_DSE_KEYS).unwrap_or(1.0);
        self.hvac.duct_zone_id = super::helpers::parse_zone_id_key(config, "duct_zone_id");
        self.hvac.update_zone_heat_fractions();
        self.rated_capacity_w = first_f64(config, HEATING_CAPACITY_KEYS)
            .unwrap_or(10_000.0)
            .max(0.0);
        self.fuel_efficiency = first_f64(config, &["fuel_efficiency", "afue", "efficiency"])
            .unwrap_or(DEFAULT_GAS_AFUE);
        if self.fuel_efficiency <= 0.0 || !self.fuel_efficiency.is_finite() {
            return Err(HaresError::Equipment(format!(
                "invalid Gas Furnace fuel efficiency: {}",
                self.fuel_efficiency
            )));
        }
        self.fan_power_w = first_f64(
            config,
            &["fan_power_w", "fan_only_power_w", "auxiliary_power_w"],
        )
        .unwrap_or_else(|| {
            let airflow_m3_s = self.rated_capacity_w * FURNACE_AIRFLOW_M3_S_PER_W_HEATING;
            self.hvac.fan_power_w(airflow_m3_s)
        });
        self.fuel_type = parse_fuel_type(config.get_str("fuel_type")).unwrap_or(FuelType::Gas);
        self.descriptor.fuel = self.fuel_type;

        self.hvac.heating_capacities_w = vec![self.rated_capacity_w];
        self.operating_mode = OperatingMode::Off;
        self.run_time_s = 0.0;
        self.telemetry = gas_furnace_default_telemetry();

        Ok(())
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        self.operating_mode = update_heating_control(&mut self.hvac, env);
        self.operating_mode
    }

    fn step(
        &mut self,
        _env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        let duty = self.hvac.duty_cycle.clamp(0.0, 1.0);
        // Fuel is computed from gross capacity: the furnace burns fuel regardless
        // of duct losses. zone_heat_fractions distributes gross output by DSE.
        let gross_capacity_w = self.rated_capacity_w * duty;
        let fan_kw = (self.fan_power_w * duty) / 1_000.0;
        let fuel_input_w = if gross_capacity_w > 0.0 {
            gross_capacity_w / self.fuel_efficiency
        } else {
            0.0
        };

        if fuel_input_w > 0.0 {
            ports.accumulate(&PortContribution::Fuel {
                fuel_type: self.fuel_type,
                consumption_w: fuel_input_w,
            })?;
        }

        if fan_kw > 0.0 {
            ports.accumulate(&PortContribution::Electrical {
                active_power_kw: fan_kw,
                reactive_power_kvar: 0.0,
            })?;
        }

        if gross_capacity_w > 0.0 {
            self.hvac
                .write_zone_thermal_contributions(ports, gross_capacity_w, 0.0)?;
        }

        if self.operating_mode == OperatingMode::Heating {
            self.run_time_s += dt.as_secs_f64();
        }

        // Telemetry reports delivered capacity (post-DSE) for the conditioned zone.
        let thermal_output_w = gross_capacity_w * self.hvac.duct_dse.clamp(0.0, 1.0);
        self.telemetry.set("fan_kw", fan_kw);
        self.telemetry.set("electric_kw", fan_kw);
        self.telemetry.set("fuel_input_w", fuel_input_w);
        self.telemetry.set("thermal_output_w", thermal_output_w);
        self.telemetry
            .set("operating_mode", operating_mode_code(self.operating_mode));
        self.telemetry
            .set("supply_air_temp_c", self.hvac.supply_air_temp_c);

        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn save_state(&self) -> Vec<u8> {
        save_postcard(&FurnaceState {
            mode: self.hvac.mode,
            duty_cycle: self.hvac.duty_cycle,
            last_mode_switch_at: self.hvac.last_mode_switch_at,
            runtime_setpoints: self.hvac.runtime_setpoints,
            operating_mode: self.operating_mode,
            run_time_s: self.run_time_s,
            electric_kw: self.telemetry.get("electric_kw").unwrap_or(0.0),
            thermal_output_w: self.telemetry.get("thermal_output_w").unwrap_or(0.0),
            fuel_input_w: self.telemetry.get("fuel_input_w").unwrap_or(0.0),
        })
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let decoded: FurnaceState = load_postcard(state)?;
        self.hvac.mode = decoded.mode;
        self.hvac.duty_cycle = decoded.duty_cycle;
        self.hvac.last_mode_switch_at = decoded.last_mode_switch_at;
        self.hvac.runtime_setpoints = decoded.runtime_setpoints;
        self.operating_mode = decoded.operating_mode;
        self.run_time_s = decoded.run_time_s;

        self.telemetry.insert("fan_kw", decoded.electric_kw);
        self.telemetry.insert("electric_kw", decoded.electric_kw);
        self.telemetry.insert("fuel_input_w", decoded.fuel_input_w);
        self.telemetry
            .insert("thermal_output_w", decoded.thermal_output_w);
        self.telemetry.insert(
            "operating_mode",
            operating_mode_code(decoded.operating_mode),
        );
        self.telemetry
            .insert("supply_air_temp_c", self.hvac.supply_air_temp_c);
        Ok(())
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        apply_heating_control_unchecked(&mut self.hvac, signal, "Gas Furnace")
    }
}

pub fn register_with_registry(registry: &mut EquipmentRegistry) {
    registry.register(
        "Electric Furnace",
        Box::new(|config| Box::new(ElectricFurnace::new(config))),
    );
    registry.register(
        "Gas Furnace",
        Box::new(|config| Box::new(GasFurnace::new(config))),
    );
}

fn electric_furnace_default_telemetry() -> Telemetry {
    let mut telemetry = Telemetry::with_capacity(4);
    telemetry.insert("electric_kw", 0.0);
    telemetry.insert("thermal_output_w", 0.0);
    telemetry.insert("operating_mode", 0.0);
    telemetry.insert("supply_air_temp_c", 0.0);
    telemetry
}

fn gas_furnace_default_telemetry() -> Telemetry {
    let mut telemetry = Telemetry::with_capacity(6);
    telemetry.insert("fan_kw", 0.0);
    telemetry.insert("electric_kw", 0.0);
    telemetry.insert("fuel_input_w", 0.0);
    telemetry.insert("thermal_output_w", 0.0);
    telemetry.insert("operating_mode", 0.0);
    telemetry.insert("supply_air_temp_c", 0.0);
    telemetry
}

fn electric_furnace_telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField {
            name: "electric_kw".to_string(),
            unit: "kW".to_string(),
            description: "Electric furnace active power draw".to_string(),
        },
        TelemetryField {
            name: "thermal_output_w".to_string(),
            unit: "W".to_string(),
            description: "Delivered sensible heat to conditioned zone after duct DSE".to_string(),
        },
        TelemetryField {
            name: "operating_mode".to_string(),
            unit: "enum".to_string(),
            description: "Operating mode code: 0=Off, 1=Heating".to_string(),
        },
        TelemetryField {
            name: "supply_air_temp_c".to_string(),
            unit: "C".to_string(),
            description: "Configured furnace supply-air temperature".to_string(),
        },
    ]
}

fn gas_furnace_telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField {
            name: "fan_kw".to_string(),
            unit: "kW".to_string(),
            description: "Gas furnace fan-only electric power".to_string(),
        },
        TelemetryField {
            name: "electric_kw".to_string(),
            unit: "kW".to_string(),
            description: "Total electric power draw (fan-only for gas furnace)".to_string(),
        },
        TelemetryField {
            name: "fuel_input_w".to_string(),
            unit: "W".to_string(),
            description: "Fuel input power derived from delivered capacity and fuel efficiency"
                .to_string(),
        },
        TelemetryField {
            name: "thermal_output_w".to_string(),
            unit: "W".to_string(),
            description: "Delivered sensible heat to conditioned zone after duct DSE".to_string(),
        },
        TelemetryField {
            name: "operating_mode".to_string(),
            unit: "enum".to_string(),
            description: "Operating mode code: 0=Off, 1=Heating".to_string(),
        },
        TelemetryField {
            name: "supply_air_temp_c".to_string(),
            unit: "C".to_string(),
            description: "Configured furnace supply-air temperature".to_string(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, time::Duration};

    use chrono::{Duration as ChronoDuration, TimeZone, Utc};
    use hares_physics::constants::{CFM_TO_M3_S, W_PER_TON};
    use hares_types::{
        EnvironmentState, ExecutionStage, GridState, PortSlots, ThermalAccumulator, WeatherState,
        ZoneId, ZoneState,
    };

    use super::{ElectricFurnace, FURNACE_FAN_CFM_PER_TON, GasFurnace, register_with_registry};
    use crate::{Equipment, EquipmentConfig, EquipmentRegistry};

    fn env(zone_temp_c: f64) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp_c,
                humidity_ratio: 0.008,
                relative_humidity: 0.45,
                wet_bulb_c: 14.0,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 8.3,
                outdoor_humidity_ratio: 0.005,
                wind_speed_m_s: 2.0,
                wind_dir_deg: 0.0,
                ground_temp_c: 12.0,
                sky_temp_c: 8.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![],
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
                solar_altitude_deg: 0.0,
                ..Default::default()
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
            },
            custom_domains: vec![],
            current_time: Utc
                .with_ymd_and_hms(2026, 3, 18, 0, 0, 0)
                .single()
                .expect("valid"),
            time_res: ChronoDuration::minutes(1),
        }
    }

    fn config(name: &str, class: &str) -> EquipmentConfig {
        EquipmentConfig {
            name: name.to_string(),
            ochre_class: class.to_string(),
            raw_config: HashMap::new(),
        }
    }

    #[test]
    fn electric_furnace_power_matches_capacity_times_eir() {
        let mut cfg = config("EF", "Electric Furnace");
        cfg.raw_config.insert("zone_id".to_string(), 1.0.into());
        cfg.raw_config
            .insert("capacity_w".to_string(), 8_000.0.into());
        cfg.raw_config.insert("eir".to_string(), 0.5.into());
        cfg.raw_config
            .insert("heating_setpoint_c".to_string(), 21.0.into());
        cfg.raw_config
            .insert("cooling_setpoint_c".to_string(), 27.0.into());

        let mut eq = ElectricFurnace::new(cfg.clone());
        let mut state = env(18.0);
        eq.init(&cfg, &state).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&state);
        eq.step(&state, Duration::from_secs(60), &mut ports)
            .unwrap();
        assert!((ports.electrical.net_active_kw() - 4.0).abs() < 1e-9);

        state.current_time += ChronoDuration::minutes(1);
        ports.zero();
        eq.update_control(&state);
        eq.step(&state, Duration::from_secs(60), &mut ports)
            .unwrap();
        assert!(ports.thermal[0].sensible_gain_w > 0.0);
    }

    #[test]
    fn gas_furnace_energy_balance_tracks_fuel_plus_fan() {
        let mut cfg = config("GF", "Gas Furnace");
        cfg.raw_config.insert("zone_id".to_string(), 1.0.into());
        cfg.raw_config
            .insert("capacity_w".to_string(), 10_000.0.into());
        cfg.raw_config
            .insert("fuel_efficiency".to_string(), 0.8.into());
        cfg.raw_config
            .insert("fan_power_w".to_string(), 400.0.into());
        cfg.raw_config
            .insert("heating_setpoint_c".to_string(), 21.0.into());
        cfg.raw_config
            .insert("cooling_setpoint_c".to_string(), 27.0.into());

        let mut eq = GasFurnace::new(cfg.clone());
        let env = env(18.0);
        eq.init(&cfg, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert!((ports.fuel.get(hares_types::FuelType::Gas) - 12_500.0).abs() < 1e-6);
        assert!((ports.electrical.net_active_kw() - 0.4).abs() < 1e-9);
    }

    #[test]
    fn gas_furnace_init_derives_fan_power_from_w_per_cfm() {
        let mut cfg = config("GF", "Gas Furnace");
        cfg.raw_config.insert("zone_id".to_string(), 1.0.into());
        cfg.raw_config
            .insert("capacity_w".to_string(), (3.0 * W_PER_TON).into());
        cfg.raw_config
            .insert("fan_power_w_per_cfm".to_string(), 0.58.into());
        cfg.raw_config
            .insert("heating_setpoint_c".to_string(), 21.0.into());
        cfg.raw_config
            .insert("cooling_setpoint_c".to_string(), 27.0.into());

        let mut eq = GasFurnace::new(cfg.clone());
        eq.init(&cfg, &env(18.0)).unwrap();

        assert!((eq.fan_power_w - 696.0).abs() < 1e-9);
    }

    #[test]
    fn gas_furnace_init_explicit_fan_power_takes_precedence() {
        let mut cfg = config("GF", "Gas Furnace");
        cfg.raw_config.insert("zone_id".to_string(), 1.0.into());
        cfg.raw_config
            .insert("capacity_w".to_string(), (3.0 * W_PER_TON).into());
        cfg.raw_config
            .insert("fan_power_w_per_cfm".to_string(), 0.58.into());
        cfg.raw_config
            .insert("fan_power_w".to_string(), 500.0.into());
        cfg.raw_config
            .insert("heating_setpoint_c".to_string(), 21.0.into());
        cfg.raw_config
            .insert("cooling_setpoint_c".to_string(), 27.0.into());

        let mut eq = GasFurnace::new(cfg.clone());
        eq.init(&cfg, &env(18.0)).unwrap();

        assert!((eq.fan_power_w - 500.0).abs() < 1e-9);
    }

    #[test]
    fn gas_furnace_init_without_fan_power_uses_default_w_per_cfm() {
        let mut cfg = config("GF", "Gas Furnace");
        cfg.raw_config.insert("zone_id".to_string(), 1.0.into());
        cfg.raw_config
            .insert("capacity_w".to_string(), (3.0 * W_PER_TON).into());
        cfg.raw_config
            .insert("heating_setpoint_c".to_string(), 21.0.into());
        cfg.raw_config
            .insert("cooling_setpoint_c".to_string(), 27.0.into());

        let mut eq = GasFurnace::new(cfg.clone());
        eq.init(&cfg, &env(18.0)).unwrap();

        let expected_airflow_m3_s = FURNACE_FAN_CFM_PER_TON * CFM_TO_M3_S * 3.0;
        let expected = eq.hvac.fan_power_w(expected_airflow_m3_s);
        assert!((eq.fan_power_w - expected).abs() < 1e-9);
    }

    /// Fuel consumption is independent of duct DSE — the furnace burns the same
    /// gas regardless of duct losses. Only the zone thermal delivery is reduced.
    #[test]
    fn gas_furnace_fuel_is_independent_of_duct_dse() {
        let mut cfg = config("GF", "Gas Furnace");
        cfg.raw_config.insert("zone_id".to_string(), 1.0.into());
        cfg.raw_config
            .insert("capacity_w".to_string(), 10_000.0.into());
        cfg.raw_config
            .insert("fuel_efficiency".to_string(), 0.8.into());
        cfg.raw_config.insert("fan_power_w".to_string(), 0.0.into());
        cfg.raw_config.insert("duct_dse".to_string(), 0.8.into());
        cfg.raw_config
            .insert("heating_setpoint_c".to_string(), 21.0.into());
        cfg.raw_config
            .insert("cooling_setpoint_c".to_string(), 27.0.into());

        let mut eq = GasFurnace::new(cfg.clone());
        let env = env(18.0);
        eq.init(&cfg, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        // Fuel = capacity / efficiency = 10000 / 0.8 = 12500 W (not affected by DSE)
        assert!(
            (ports.fuel.get(hares_types::FuelType::Gas) - 12_500.0).abs() < 1e-6,
            "fuel should be capacity/efficiency, not capacity*dse/efficiency"
        );
        // Thermal delivery = capacity * DSE = 10000 * 0.8 = 8000 W
        assert!(
            (ports.thermal[0].sensible_gain_w - 8_000.0).abs() < 1e-6,
            "zone thermal = capacity * DSE"
        );
    }

    #[test]
    fn supply_air_temp_defaults_and_overrides_are_applied() {
        let mut cfg = config("EF", "Electric Furnace");
        cfg.raw_config.insert("zone_id".to_string(), 1.0.into());
        cfg.raw_config
            .insert("heating_setpoint_c".to_string(), 21.0.into());
        cfg.raw_config
            .insert("cooling_setpoint_c".to_string(), 27.0.into());

        let mut eq = ElectricFurnace::new(cfg.clone());
        eq.init(&cfg, &env(20.0)).unwrap();
        assert!((eq.hvac.supply_air_temp_c - 48.9).abs() < 1e-9);

        cfg.raw_config
            .insert("supply_air_temp_c".to_string(), 51.0.into());
        let mut eq_override = ElectricFurnace::new(cfg.clone());
        eq_override.init(&cfg, &env(20.0)).unwrap();
        assert!((eq_override.hvac.supply_air_temp_c - 51.0).abs() < 1e-9);
    }

    #[test]
    fn furnace_state_round_trip_preserves_mode_and_outputs() {
        let mut cfg = config("EF", "Electric Furnace");
        cfg.raw_config.insert("zone_id".to_string(), 1.0.into());
        cfg.raw_config
            .insert("capacity_w".to_string(), 8_000.0.into());
        cfg.raw_config.insert("eir".to_string(), 0.5.into());
        cfg.raw_config
            .insert("heating_setpoint_c".to_string(), 21.0.into());
        cfg.raw_config
            .insert("cooling_setpoint_c".to_string(), 27.0.into());

        let mut eq = ElectricFurnace::new(cfg.clone());
        let env = env(18.0);
        eq.init(&cfg, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        let state = eq.save_state();

        let mut restored = ElectricFurnace::new(cfg.clone());
        restored.init(&cfg, &env).unwrap();
        restored.load_state(&state).unwrap();
        assert_eq!(
            restored.telemetry().get("electric_kw"),
            eq.telemetry().get("electric_kw")
        );
    }

    #[test]
    fn ashrae_152_dse_identity() {
        // ASHRAE Standard 152-2004: DSE=1.0 means no duct losses.
        // capacity_w * DSE = delivered capacity.
        // At DSE=0.8, 10000W -> 8000W delivered.
        let mut cfg = config("GF", "Gas Furnace");
        cfg.raw_config.insert("zone_id".to_string(), 1.0.into());
        cfg.raw_config
            .insert("capacity_w".to_string(), 10_000.0.into());
        cfg.raw_config
            .insert("fuel_efficiency".to_string(), 0.8.into());
        cfg.raw_config.insert("fan_power_w".to_string(), 0.0.into());
        cfg.raw_config.insert("duct_dse".to_string(), 0.8.into());
        cfg.raw_config
            .insert("heating_setpoint_c".to_string(), 21.0.into());
        cfg.raw_config
            .insert("cooling_setpoint_c".to_string(), 27.0.into());

        let mut eq = GasFurnace::new(cfg.clone());
        let env = env(18.0);
        eq.init(&cfg, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        // DSE=0.8 => 10000W * 0.8 = 8000W delivered
        assert!(
            (ports.thermal[0].sensible_gain_w - 8_000.0).abs() < 1e-6,
            "ASHRAE 152: 10kW * DSE=0.8 should deliver 8kW, got {}",
            ports.thermal[0].sensible_gain_w,
        );

        // DSE=1.0 => no losses
        cfg.raw_config.insert("duct_dse".to_string(), 1.0.into());
        let mut eq_perfect = GasFurnace::new(cfg.clone());
        eq_perfect.init(&cfg, &env).unwrap();

        let mut ports_perfect = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq_perfect.update_control(&env);
        eq_perfect
            .step(&env, Duration::from_secs(60), &mut ports_perfect)
            .unwrap();
        assert!(
            (ports_perfect.thermal[0].sensible_gain_w - 10_000.0).abs() < 1e-6,
            "ASHRAE 152: DSE=1.0 should deliver full capacity",
        );
    }

    #[test]
    fn registry_includes_furnace_aliases_and_thermal_stage() {
        let mut registry = EquipmentRegistry::new();
        register_with_registry(&mut registry);
        assert!(registry.get("Electric Furnace").is_some());
        assert!(registry.get("Gas Furnace").is_some());

        let cfg = config("EF", "Electric Furnace");
        let eq = registry.create("Electric Furnace", cfg).unwrap();
        assert_eq!(eq.descriptor().stage, ExecutionStage::Thermal);
    }
}
