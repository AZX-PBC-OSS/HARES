//! Gas and electric furnace models.

use std::borrow::Cow;
use std::time::Duration;

use chrono::{DateTime, FixedOffset};
use hares_types::{
    ControlCapabilities, ControlSignal, CoreCapabilities, EndUse, EnvironmentState,
    EquipmentDescriptor, EquipmentId, ExecutionStage, FuelType, HaresError, OperatingMode,
    PortContribution, PortDeclaration, PortSlots, Telemetry, TelemetryField, ThermalCategory,
    ZoneId,
};
use serde::{Deserialize, Serialize};

use hares_types::telemetry_keys as tk;

use crate::hvac::heating_config::{ElectricFurnaceConfig, GasFurnaceConfig};
use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_postcard, save_postcard};

use super::{
    HvacEquipment, HvacEquipmentType, RuntimeSetpointOverride, ThermostatMode,
    helpers::{
        HEATING_CAPACITY_KEYS, apply_heating_control_unchecked, equipment_id_from_config,
        first_f64, operating_mode_code, parse_fuel_type, update_heating_control,
        zone_id_from_config,
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
    fan_power_w: f64,
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
    last_mode_switch_at: Option<DateTime<FixedOffset>>,
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
            end_use: EndUse::HVAC_HEATING,
            equipment_type: Cow::Borrowed("Electric Furnace"),
            zone: Some(zone),
            fuel: FuelType::Electric,
            stage: ExecutionStage::Thermal,
            control_capabilities: ControlCapabilities::THERMAL_SETPOINT
                | ControlCapabilities::THERMAL_SETPOINT_DELTA
                | ControlCapabilities::IDEAL_CAPACITY,
            core_capabilities: CoreCapabilities::empty(),
            telemetry_fields: electric_furnace_telemetry_fields(),
        };

        Self {
            descriptor,
            ports: vec![
                PortDeclaration::electrical(),
                PortDeclaration::thermal(zone),
            ],
            telemetry: electric_furnace_default_telemetry(),
            hvac: HvacEquipment::new(HvacEquipmentType::ElectricFurnace, zone),
            rated_capacity_w: 0.0,
            eir: 1.0,
            fan_power_w: 0.0,
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
        if config.is_typed() {
            let typed = config.typed::<ElectricFurnaceConfig>()?;
            self.hvac.init(config, env)?;
            self.rated_capacity_w = typed.heating_capacity_w.max(0.0);
            self.eir = typed.heating_efficiency;
            if self.eir <= 0.0 || !self.eir.is_finite() {
                return Err(HaresError::Equipment(format!(
                    "invalid Electric Furnace heating efficiency: {}",
                    self.eir
                )));
            }
            let airflow_m3_s = self.hvac.airflow_m3_s_per_w * self.rated_capacity_w;
            self.fan_power_w = typed
                .fan_power_w
                .unwrap_or_else(|| self.hvac.fan_power_w(airflow_m3_s));
            self.hvac.duct_dse = typed.ducts.dse_heat.unwrap_or(1.0).clamp(0.0, 1.0);
            self.hvac.duct_zone_id = None;
            self.hvac.update_zone_heat_fractions();
            self.hvac.heating_capacities_w = vec![self.rated_capacity_w];
            self.hvac.eir_by_stage = vec![self.eir];
            self.operating_mode = OperatingMode::Off;
            self.run_time_s = 0.0;
            self.telemetry = electric_furnace_default_telemetry();
            return Ok(());
        }

        self.hvac.init(config, env)?;

        self.rated_capacity_w = first_f64(config, HEATING_CAPACITY_KEYS)
            .unwrap_or(10_000.0)
            .max(0.0);
        self.eir = first_f64(config, &["eir", "heating_eir", "EIR"]).unwrap_or(1.0);
        let airflow_m3_s = self.hvac.airflow_m3_s_per_w * self.rated_capacity_w;
        self.fan_power_w = first_f64(
            config,
            &["fan_power_w", "fan_only_power_w", "auxiliary_power_w"],
        )
        .unwrap_or_else(|| self.hvac.fan_power_w(airflow_m3_s));

        let fan_flow = airflow_m3_s;
        self.hvac.duct_dse = super::helpers::resolve_duct_dse(
            config,
            true,
            self.rated_capacity_w,
            fan_flow,
            1,
            false,
        );
        self.hvac.duct_zone_id = super::helpers::parse_zone_id_key(config, "duct_zone_id");
        self.hvac.update_zone_heat_fractions();

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
        let sf = self.hvac.space_fraction;
        let gross_capacity_w = self.rated_capacity_w * duty;
        let fan_kw = (self.fan_power_w * duty) / 1_000.0 * sf;
        // Heating element power + fan power
        let electric_kw = (self.rated_capacity_w * self.eir * duty) / 1_000.0 * sf + fan_kw;

        if electric_kw > 0.0 {
            ports.accumulate(&PortContribution::Electrical {
                active_power_kw: electric_kw,
                reactive_power_kvar: 0.0,
            })?;
        }

        // Fan waste heat contributes to zone sensible gain (OCHRE HVAC.py line 543).
        let fan_heat_w = fan_kw * 1000.0;
        let total_sensible_w = gross_capacity_w + fan_heat_w;

        if total_sensible_w > 0.0 {
            self.hvac.write_zone_thermal_contributions(
                ports,
                total_sensible_w,
                0.0,
                ThermalCategory::HvacHeating,
            )?;
        }

        if self.operating_mode == OperatingMode::Heating {
            self.run_time_s += dt.as_secs_f64();
        }

        let thermal_output_w = total_sensible_w * self.hvac.duct_dse.clamp(0.0, 1.0);
        let sp = self.hvac.effective_setpoints();
        self.telemetry.set(tk::FAN_KW, fan_kw);
        self.telemetry.set(tk::ELECTRIC_KW, electric_kw);
        self.telemetry.set(tk::THERMAL_OUTPUT_W, thermal_output_w);
        self.telemetry
            .set(tk::OPERATING_MODE, operating_mode_code(self.operating_mode));
        self.telemetry
            .set(tk::SUPPLY_AIR_TEMP_C, self.hvac.supply_air_temp_c);
        self.telemetry.set(tk::HEATING_SETPOINT_C, sp.heating_c);
        self.telemetry.set(tk::COOLING_SETPOINT_C, sp.cooling_c);

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
            electric_kw: self.telemetry.get(tk::ELECTRIC_KW).unwrap_or(0.0),
            thermal_output_w: self.telemetry.get(tk::THERMAL_OUTPUT_W).unwrap_or(0.0),
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
        self.telemetry.insert(tk::ELECTRIC_KW, decoded.electric_kw);
        self.telemetry
            .insert(tk::THERMAL_OUTPUT_W, decoded.thermal_output_w);
        self.telemetry.insert(
            tk::OPERATING_MODE,
            operating_mode_code(decoded.operating_mode),
        );
        self.telemetry
            .insert(tk::SUPPLY_AIR_TEMP_C, self.hvac.supply_air_temp_c);
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
            end_use: EndUse::HVAC_HEATING,
            equipment_type: Cow::Borrowed("Gas Furnace"),
            zone: Some(zone),
            fuel: FuelType::Gas,
            stage: ExecutionStage::Thermal,
            control_capabilities: ControlCapabilities::THERMAL_SETPOINT
                | ControlCapabilities::THERMAL_SETPOINT_DELTA
                | ControlCapabilities::IDEAL_CAPACITY,
            core_capabilities: CoreCapabilities::empty(),
            telemetry_fields: gas_furnace_telemetry_fields(),
        };

        Self {
            descriptor,
            ports: vec![
                PortDeclaration::fuel(),
                PortDeclaration::electrical(),
                PortDeclaration::thermal(zone),
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
        if config.is_typed() {
            let typed = config.typed::<GasFurnaceConfig>()?;
            self.hvac.init(config, env)?;
            self.rated_capacity_w = typed.heating_capacity_w.max(0.0);
            self.fuel_efficiency = typed.afue;
            if self.fuel_efficiency <= 0.0 || !self.fuel_efficiency.is_finite() {
                return Err(HaresError::Equipment(format!(
                    "invalid Gas Furnace fuel efficiency: {}",
                    self.fuel_efficiency
                )));
            }
            let airflow_m3_s = self.rated_capacity_w * FURNACE_AIRFLOW_M3_S_PER_W_HEATING;
            self.fan_power_w = typed
                .fan_power_w
                .unwrap_or_else(|| self.hvac.fan_power_w(airflow_m3_s));
            self.hvac.duct_dse = typed.ducts.dse_heat.unwrap_or(1.0).clamp(0.0, 1.0);
            self.hvac.duct_zone_id = None;
            self.hvac.update_zone_heat_fractions();
            self.hvac.heating_capacities_w = vec![self.rated_capacity_w];
            self.hvac.eir_by_stage = vec![1.0 / self.fuel_efficiency];
            self.operating_mode = OperatingMode::Off;
            self.run_time_s = 0.0;
            self.telemetry = gas_furnace_default_telemetry();
            return Ok(());
        }

        self.hvac.init(config, env)?;

        self.rated_capacity_w = first_f64(config, HEATING_CAPACITY_KEYS)
            .unwrap_or(10_000.0)
            .max(0.0);

        let fan_flow = self.hvac.airflow_m3_s_per_w * self.rated_capacity_w;
        self.hvac.duct_dse = super::helpers::resolve_duct_dse(
            config,
            true,
            self.rated_capacity_w,
            fan_flow,
            1,
            false,
        );
        self.hvac.duct_zone_id = super::helpers::parse_zone_id_key(config, "duct_zone_id");
        self.hvac.update_zone_heat_fractions();
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
        let sf = self.hvac.space_fraction;
        // Fuel is computed from gross capacity: the furnace burns fuel regardless
        // of duct losses. zone_heat_fractions distributes gross output by DSE.
        let gross_capacity_w = self.rated_capacity_w * duty;
        let fan_kw = (self.fan_power_w * duty) / 1_000.0 * sf;
        let fuel_input_w = if gross_capacity_w > 0.0 {
            gross_capacity_w / self.fuel_efficiency * sf
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

        // Fan waste heat contributes to zone sensible gain (OCHRE HVAC.py line 543).
        let fan_heat_w = fan_kw * 1000.0;
        let total_sensible_w = gross_capacity_w + fan_heat_w;

        if total_sensible_w > 0.0 {
            self.hvac.write_zone_thermal_contributions(
                ports,
                total_sensible_w,
                0.0,
                ThermalCategory::HvacHeating,
            )?;
        }

        if self.operating_mode == OperatingMode::Heating {
            self.run_time_s += dt.as_secs_f64();
        }

        // Telemetry reports delivered capacity (post-DSE) for the conditioned zone.
        let thermal_output_w = total_sensible_w * self.hvac.duct_dse.clamp(0.0, 1.0);
        let sp = self.hvac.effective_setpoints();
        self.telemetry.set(tk::FAN_KW, fan_kw);
        self.telemetry.set(tk::ELECTRIC_KW, fan_kw);
        self.telemetry.set(tk::FUEL_INPUT_W, fuel_input_w);
        self.telemetry.set(tk::THERMAL_OUTPUT_W, thermal_output_w);
        self.telemetry
            .set(tk::OPERATING_MODE, operating_mode_code(self.operating_mode));
        self.telemetry
            .set(tk::SUPPLY_AIR_TEMP_C, self.hvac.supply_air_temp_c);
        self.telemetry.set(tk::HEATING_SETPOINT_C, sp.heating_c);
        self.telemetry.set(tk::COOLING_SETPOINT_C, sp.cooling_c);

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
            electric_kw: self.telemetry.get(tk::ELECTRIC_KW).unwrap_or(0.0),
            thermal_output_w: self.telemetry.get(tk::THERMAL_OUTPUT_W).unwrap_or(0.0),
            fuel_input_w: self.telemetry.get(tk::FUEL_INPUT_W).unwrap_or(0.0),
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

        self.telemetry.insert(tk::FAN_KW, decoded.electric_kw);
        self.telemetry.insert(tk::ELECTRIC_KW, decoded.electric_kw);
        self.telemetry
            .insert(tk::FUEL_INPUT_W, decoded.fuel_input_w);
        self.telemetry
            .insert(tk::THERMAL_OUTPUT_W, decoded.thermal_output_w);
        self.telemetry.insert(
            tk::OPERATING_MODE,
            operating_mode_code(decoded.operating_mode),
        );
        self.telemetry
            .insert(tk::SUPPLY_AIR_TEMP_C, self.hvac.supply_air_temp_c);
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
    let mut telemetry = Telemetry::with_capacity(7);
    telemetry.insert(tk::FAN_KW, 0.0);
    telemetry.insert(tk::ELECTRIC_KW, 0.0);
    telemetry.insert(tk::THERMAL_OUTPUT_W, 0.0);
    telemetry.insert(tk::OPERATING_MODE, 0.0);
    telemetry.insert(tk::SUPPLY_AIR_TEMP_C, 0.0);
    telemetry.insert(tk::HEATING_SETPOINT_C, 0.0);
    telemetry.insert(tk::COOLING_SETPOINT_C, 0.0);
    telemetry
}

fn gas_furnace_default_telemetry() -> Telemetry {
    let mut telemetry = Telemetry::with_capacity(8);
    telemetry.insert(tk::FAN_KW, 0.0);
    telemetry.insert(tk::ELECTRIC_KW, 0.0);
    telemetry.insert(tk::FUEL_INPUT_W, 0.0);
    telemetry.insert(tk::THERMAL_OUTPUT_W, 0.0);
    telemetry.insert(tk::OPERATING_MODE, 0.0);
    telemetry.insert(tk::SUPPLY_AIR_TEMP_C, 0.0);
    telemetry.insert(tk::HEATING_SETPOINT_C, 0.0);
    telemetry.insert(tk::COOLING_SETPOINT_C, 0.0);
    telemetry
}

fn setpoint_telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField {
            name: tk::HEATING_SETPOINT_C.to_string(),
            unit: "C".to_string(),
            description: "Active heating setpoint from thermostat schedule".to_string(),
        },
        TelemetryField {
            name: tk::COOLING_SETPOINT_C.to_string(),
            unit: "C".to_string(),
            description: "Active cooling setpoint from thermostat schedule".to_string(),
        },
    ]
}

fn electric_furnace_telemetry_fields() -> Vec<TelemetryField> {
    let mut fields = vec![
        TelemetryField {
            name: tk::ELECTRIC_KW.to_string(),
            unit: "kW".to_string(),
            description: "Electric furnace active power draw".to_string(),
        },
        TelemetryField {
            name: tk::THERMAL_OUTPUT_W.to_string(),
            unit: "W".to_string(),
            description: "Delivered sensible heat to conditioned zone after duct DSE".to_string(),
        },
        TelemetryField {
            name: tk::OPERATING_MODE.to_string(),
            unit: "enum".to_string(),
            description: "Operating mode code: 0=Off, 1=Heating".to_string(),
        },
        TelemetryField {
            name: tk::SUPPLY_AIR_TEMP_C.to_string(),
            unit: "C".to_string(),
            description: "Configured furnace supply-air temperature".to_string(),
        },
    ];
    fields.extend(setpoint_telemetry_fields());
    fields
}

fn gas_furnace_telemetry_fields() -> Vec<TelemetryField> {
    let mut fields = vec![
        TelemetryField {
            name: tk::FAN_KW.to_string(),
            unit: "kW".to_string(),
            description: "Gas furnace fan-only electric power".to_string(),
        },
        TelemetryField {
            name: tk::ELECTRIC_KW.to_string(),
            unit: "kW".to_string(),
            description: "Total electric power draw (fan-only for gas furnace)".to_string(),
        },
        TelemetryField {
            name: tk::FUEL_INPUT_W.to_string(),
            unit: "W".to_string(),
            description: "Fuel input power derived from delivered capacity and fuel efficiency"
                .to_string(),
        },
        TelemetryField {
            name: tk::THERMAL_OUTPUT_W.to_string(),
            unit: "W".to_string(),
            description: "Delivered sensible heat to conditioned zone after duct DSE".to_string(),
        },
        TelemetryField {
            name: tk::OPERATING_MODE.to_string(),
            unit: "enum".to_string(),
            description: "Operating mode code: 0=Off, 1=Heating".to_string(),
        },
        TelemetryField {
            name: tk::SUPPLY_AIR_TEMP_C.to_string(),
            unit: "C".to_string(),
            description: "Configured furnace supply-air temperature".to_string(),
        },
    ];
    fields.extend(setpoint_telemetry_fields());
    fields
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, time::Duration};

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_physics::constants::{CFM_TO_M3_S, W_PER_TON};
    use hares_types::{
        EnvironmentState, ExecutionStage, GridState, PortSlots, ThermalAccumulator, WeatherState,
        ZoneId, ZoneState, telemetry_keys as tk,
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
            equipment_telemetry: std::collections::HashMap::new(),
            current_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 3, 18, 0, 0, 0)
                .single()
                .expect("valid"),
            time_res: ChronoDuration::minutes(1),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    fn config(name: &str, class: &str) -> EquipmentConfig {
        EquipmentConfig {
            name: name.to_string(),
            ochre_class: class.to_string(),
            payload: crate::config::ConfigPayload::Raw {
                data: HashMap::new(),
            },
        }
    }

    #[test]
    fn electric_furnace_power_matches_capacity_times_eir() {
        let mut cfg = config("EF", "Electric Furnace");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".to_string(), 1.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("capacity_w".to_string(), 8_000.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("eir".to_string(), 0.5.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("fan_power_w".to_string(), 0.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("heating_setpoint_c".to_string(), 21.0.into());
        cfg.raw_config_mut()
            .unwrap()
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
        const RATED_CAPACITY_W: f64 = 10_000.0;
        const FUEL_EFFICIENCY: f64 = 0.8;
        const FAN_POWER_W: f64 = 400.0;
        const EXPECTED_FUEL_INPUT_W: f64 = RATED_CAPACITY_W / FUEL_EFFICIENCY;
        const EXPECTED_FAN_KW: f64 = FAN_POWER_W / 1_000.0;
        const EXPECTED_SENSIBLE_GAIN_W: f64 = RATED_CAPACITY_W + FAN_POWER_W;

        let mut cfg = config("GF", "Gas Furnace");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".to_string(), 1.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("capacity_w".to_string(), RATED_CAPACITY_W.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("fuel_efficiency".to_string(), FUEL_EFFICIENCY.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("fan_power_w".to_string(), FAN_POWER_W.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("heating_setpoint_c".to_string(), 21.0.into());
        cfg.raw_config_mut()
            .unwrap()
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

        assert!((ports.fuel.get(hares_types::FuelType::Gas) - EXPECTED_FUEL_INPUT_W).abs() < 1e-6);
        assert!((ports.electrical.net_active_kw() - EXPECTED_FAN_KW).abs() < 1e-9);
        assert!((ports.thermal[0].sensible_gain_w - EXPECTED_SENSIBLE_GAIN_W).abs() < 1e-6);
    }

    #[test]
    fn gas_furnace_init_derives_fan_power_from_w_per_cfm() {
        let mut cfg = config("GF", "Gas Furnace");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".to_string(), 1.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("capacity_w".to_string(), (3.0 * W_PER_TON).into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("fan_power_w_per_cfm".to_string(), 0.58.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("heating_setpoint_c".to_string(), 21.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("cooling_setpoint_c".to_string(), 27.0.into());

        let mut eq = GasFurnace::new(cfg.clone());
        eq.init(&cfg, &env(18.0)).unwrap();

        assert!((eq.fan_power_w - 696.0).abs() < 1e-9);
    }

    #[test]
    fn gas_furnace_init_explicit_fan_power_takes_precedence() {
        let mut cfg = config("GF", "Gas Furnace");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".to_string(), 1.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("capacity_w".to_string(), (3.0 * W_PER_TON).into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("fan_power_w_per_cfm".to_string(), 0.58.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("fan_power_w".to_string(), 500.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("heating_setpoint_c".to_string(), 21.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("cooling_setpoint_c".to_string(), 27.0.into());

        let mut eq = GasFurnace::new(cfg.clone());
        eq.init(&cfg, &env(18.0)).unwrap();

        assert!((eq.fan_power_w - 500.0).abs() < 1e-9);
    }

    #[test]
    fn gas_furnace_init_without_fan_power_uses_default_w_per_cfm() {
        let mut cfg = config("GF", "Gas Furnace");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".to_string(), 1.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("capacity_w".to_string(), (3.0 * W_PER_TON).into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("heating_setpoint_c".to_string(), 21.0.into());
        cfg.raw_config_mut()
            .unwrap()
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
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".to_string(), 1.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("capacity_w".to_string(), 10_000.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("fuel_efficiency".to_string(), 0.8.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("fan_power_w".to_string(), 0.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("duct_dse".to_string(), 0.8.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("heating_setpoint_c".to_string(), 21.0.into());
        cfg.raw_config_mut()
            .unwrap()
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
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".to_string(), 1.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("heating_setpoint_c".to_string(), 21.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("cooling_setpoint_c".to_string(), 27.0.into());

        let mut eq = ElectricFurnace::new(cfg.clone());
        eq.init(&cfg, &env(20.0)).unwrap();
        assert!((eq.hvac.supply_air_temp_c - 48.9).abs() < 1e-9);

        cfg.raw_config_mut()
            .unwrap()
            .insert("supply_air_temp_c".to_string(), 51.0.into());
        let mut eq_override = ElectricFurnace::new(cfg.clone());
        eq_override.init(&cfg, &env(20.0)).unwrap();
        assert!((eq_override.hvac.supply_air_temp_c - 51.0).abs() < 1e-9);
    }

    #[test]
    fn furnace_state_round_trip_preserves_mode_and_outputs() {
        let mut cfg = config("EF", "Electric Furnace");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".to_string(), 1.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("capacity_w".to_string(), 8_000.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("eir".to_string(), 0.5.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("heating_setpoint_c".to_string(), 21.0.into());
        cfg.raw_config_mut()
            .unwrap()
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
            restored.telemetry().get(tk::ELECTRIC_KW),
            eq.telemetry().get(tk::ELECTRIC_KW)
        );
    }

    #[test]
    fn ashrae_152_dse_identity() {
        // ASHRAE Standard 152-2004: DSE=1.0 means no duct losses.
        // capacity_w * DSE = delivered capacity.
        // At DSE=0.8, 10000W -> 8000W delivered.
        let mut cfg = config("GF", "Gas Furnace");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".to_string(), 1.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("capacity_w".to_string(), 10_000.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("fuel_efficiency".to_string(), 0.8.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("fan_power_w".to_string(), 0.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("duct_dse".to_string(), 0.8.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("heating_setpoint_c".to_string(), 21.0.into());
        cfg.raw_config_mut()
            .unwrap()
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
        cfg.raw_config_mut()
            .unwrap()
            .insert("duct_dse".to_string(), 1.0.into());
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

    /// Regression test for bug: per-equipment gas output column read the telemetry key
    /// "gas_consumption_w" but furnace writes "fuel_input_w", so the breakdown was always
    /// 0.0.  This test verifies the correct key is written and the old key is absent.
    #[test]
    fn gas_furnace_writes_fuel_input_w_telemetry_key() {
        let mut cfg = config("GF", "Gas Furnace");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".to_string(), 1.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("capacity_w".to_string(), 10_000.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("fuel_efficiency".to_string(), 0.9.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("fan_power_w".to_string(), 0.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("heating_setpoint_c".to_string(), 21.0.into());
        cfg.raw_config_mut()
            .unwrap()
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

        let fuel_input_w = eq
            .telemetry()
            .get(tk::FUEL_INPUT_W)
            .expect("furnace must write fuel_input_w telemetry key");
        assert!(
            fuel_input_w > 0.0,
            "fuel_input_w must be positive during heating; got {fuel_input_w}"
        );
        // The old (wrong) key must not exist in telemetry.
        assert!(
            eq.telemetry().get("gas_consumption_w").is_none(),
            "gas_consumption_w must not be a telemetry key on furnace"
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
