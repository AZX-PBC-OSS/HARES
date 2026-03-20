//! Electric baseboard heater model.

use std::borrow::Cow;
use std::time::Duration;

use chrono::{DateTime, Utc};
use hares_types::{
    ControlCapabilities, ControlSignal, EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId,
    ExecutionStage, FuelType, HaresError, OperatingMode, PortContribution, PortDeclaration,
    PortSlots, Telemetry, TelemetryField, ThermalCategory, ZoneId,
};
use serde::{Deserialize, Serialize};

use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_postcard, save_postcard};

use super::{
    HvacEquipment, HvacEquipmentType, RuntimeSetpointOverride, ThermostatMode,
    helpers::{
        HEATING_CAPACITY_KEYS, apply_heating_control_unchecked, equipment_id_from_config,
        first_f64, operating_mode_code, update_heating_control, zone_id_from_config,
    },
};

pub struct ElectricBaseboard {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,
    hvac: HvacEquipment,
    rated_capacity_w: f64,
    operating_mode: OperatingMode,
    run_time_s: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct BaseboardState {
    mode: ThermostatMode,
    duty_cycle: f64,
    last_mode_switch_at: Option<DateTime<Utc>>,
    runtime_setpoints: Option<RuntimeSetpointOverride>,
    operating_mode: OperatingMode,
    run_time_s: f64,
    electric_kw: f64,
    thermal_output_w: f64,
}

impl ElectricBaseboard {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let zone = zone_id_from_config(&config).unwrap_or(ZoneId(1));
        let descriptor = EquipmentDescriptor {
            id: EquipmentId(equipment_id_from_config(&config).unwrap_or(0)),
            name: config.name,
            end_use: EndUse::HvacHeating,
            equipment_type: Cow::Borrowed("Electric Baseboard"),
            zone: Some(zone),
            fuel: FuelType::Electric,
            stage: ExecutionStage::Thermal,
            control_capabilities: ControlCapabilities::THERMAL_SETPOINT,
            telemetry_fields: telemetry_fields(),
        };

        Self {
            descriptor,
            ports: vec![
                PortDeclaration::electrical(),
                PortDeclaration::thermal(zone),
            ],
            telemetry: default_telemetry(),
            hvac: HvacEquipment::new(HvacEquipmentType::Baseboard, zone),
            rated_capacity_w: 0.0,
            operating_mode: OperatingMode::Off,
            run_time_s: 0.0,
        }
    }
}

impl Equipment for ElectricBaseboard {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        self.hvac.init(config, env)?;
        self.hvac.duct_dse = 1.0;
        self.hvac.duct_zone_id = None;
        self.hvac.update_zone_heat_fractions();
        self.rated_capacity_w = first_f64(config, HEATING_CAPACITY_KEYS)
            .unwrap_or(6_000.0)
            .max(0.0);
        self.hvac.heating_capacities_w = vec![self.rated_capacity_w];
        self.operating_mode = OperatingMode::Off;
        self.run_time_s = 0.0;
        self.telemetry = default_telemetry();
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
        let thermal_output_w = self.rated_capacity_w * duty;
        let electric_kw = thermal_output_w / 1_000.0 * self.hvac.space_fraction;

        if electric_kw > 0.0 {
            ports.accumulate(&PortContribution::Electrical {
                active_power_kw: electric_kw,
                reactive_power_kvar: 0.0,
            })?;
            self.hvac
                .write_zone_thermal_contributions(ports, thermal_output_w, 0.0, ThermalCategory::HvacHeating)?;
            self.run_time_s += dt.as_secs_f64();
        }

        self.telemetry.set("electric_kw", electric_kw);
        self.telemetry.set("thermal_output_w", thermal_output_w);
        self.telemetry
            .set("operating_mode", operating_mode_code(self.operating_mode));

        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn save_state(&self) -> Vec<u8> {
        save_postcard(&BaseboardState {
            mode: self.hvac.mode,
            duty_cycle: self.hvac.duty_cycle,
            last_mode_switch_at: self.hvac.last_mode_switch_at,
            runtime_setpoints: self.hvac.runtime_setpoints,
            operating_mode: self.operating_mode,
            run_time_s: self.run_time_s,
            electric_kw: self.telemetry.get("electric_kw").unwrap_or(0.0),
            thermal_output_w: self.telemetry.get("thermal_output_w").unwrap_or(0.0),
        })
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let decoded: BaseboardState = load_postcard(state)?;
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
        Ok(())
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        apply_heating_control_unchecked(&mut self.hvac, signal, "Electric Baseboard")
    }
}

pub fn register_with_registry(registry: &mut EquipmentRegistry) {
    registry.register(
        "Electric Baseboard",
        Box::new(|config| Box::new(ElectricBaseboard::new(config))),
    );
}

fn default_telemetry() -> Telemetry {
    let mut telemetry = Telemetry::with_capacity(3);
    telemetry.insert("electric_kw", 0.0);
    telemetry.insert("thermal_output_w", 0.0);
    telemetry.insert("operating_mode", 0.0);
    telemetry
}

fn telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField {
            name: "electric_kw".to_string(),
            unit: "kW".to_string(),
            description: "Electric baseboard active power draw".to_string(),
        },
        TelemetryField {
            name: "thermal_output_w".to_string(),
            unit: "W".to_string(),
            description: "Delivered sensible zone heat (baseboard bypasses ducts)".to_string(),
        },
        TelemetryField {
            name: "operating_mode".to_string(),
            unit: "enum".to_string(),
            description: "Operating mode code: 0=Off, 1=Heating".to_string(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, time::Duration};

    use chrono::{Duration as ChronoDuration, TimeZone, Utc};
    use hares_types::{
        EnvironmentState, ExecutionStage, GridState, PortSlots, ThermalAccumulator, WeatherState,
        ZoneId, ZoneState,
    };

    use super::{ElectricBaseboard, register_with_registry};
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

    fn config() -> EquipmentConfig {
        EquipmentConfig {
            name: "Baseboard".to_string(),
            ochre_class: "Electric Baseboard".to_string(),
            raw_config: HashMap::new(),
        }
    }

    #[test]
    fn baseboard_forces_dse_to_one_and_writes_direct_zone_heat() {
        let mut cfg = config();
        cfg.raw_config.insert("zone_id".to_string(), 1.0.into());
        cfg.raw_config
            .insert("capacity_w".to_string(), 3_000.0.into());
        cfg.raw_config.insert("duct_dse".to_string(), 0.2.into());
        cfg.raw_config
            .insert("heating_setpoint_c".to_string(), 21.0.into());
        cfg.raw_config
            .insert("cooling_setpoint_c".to_string(), 27.0.into());

        let mut eq = ElectricBaseboard::new(cfg.clone());
        let env = env(18.0);
        eq.init(&cfg, &env).unwrap();
        assert!((eq.hvac.duct_dse - 1.0).abs() < 1e-12);

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        assert!((ports.electrical.net_active_kw() - 3.0).abs() < 1e-9);
        assert!((ports.thermal[0].sensible_gain_w - 3_000.0).abs() < 1e-9);
    }

    #[test]
    fn state_round_trip_preserves_mode() {
        let mut cfg = config();
        cfg.raw_config.insert("zone_id".to_string(), 1.0.into());
        cfg.raw_config
            .insert("capacity_w".to_string(), 3_000.0.into());
        cfg.raw_config
            .insert("heating_setpoint_c".to_string(), 21.0.into());
        cfg.raw_config
            .insert("cooling_setpoint_c".to_string(), 27.0.into());

        let mut eq = ElectricBaseboard::new(cfg.clone());
        let env = env(18.0);
        eq.init(&cfg, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        let state = eq.save_state();

        let mut restored = ElectricBaseboard::new(cfg.clone());
        restored.init(&cfg, &env).unwrap();
        restored.load_state(&state).unwrap();
        assert_eq!(restored.telemetry().get("electric_kw"), Some(3.0));
    }

    #[test]
    fn registry_includes_baseboard_alias_and_thermal_stage() {
        let mut registry = EquipmentRegistry::new();
        register_with_registry(&mut registry);
        assert!(registry.get("Electric Baseboard").is_some());

        let eq = registry.create("Electric Baseboard", config()).unwrap();
        assert_eq!(eq.descriptor().stage, ExecutionStage::Thermal);
    }
}
