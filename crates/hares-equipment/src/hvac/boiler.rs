//! Electric and gas boiler models.

use std::borrow::Cow;
use std::time::Duration;

use chrono::{DateTime, FixedOffset};
use hares_types::{
    ControlCapabilities, ControlSignal, CoreCapabilities, CoreFlows, CoreOutput, CoreState,
    ElectricPower, EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId, ExecutionStage,
    FLUID, FluidDomainPayload, FluidType, FuelPower, FuelType, HaresError, LoopId, OperatingMode,
    PortContribution, PortDeclaration, PortSlots, Telemetry, TelemetryField, ThermalCategory,
    ZoneId,
};
use serde::{Deserialize, Serialize};

use hares_types::telemetry_keys as tk;

use crate::hvac::heating_config::{ElectricBoilerConfig, GasBoilerConfig};
use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_postcard, save_postcard};

use super::{
    HvacEquipment, HvacEquipmentType, RuntimeSetpointOverride, ThermostatMode,
    helpers::{
        apply_heating_control_unchecked, equipment_id_from_config, first_f64,
        loop_id_from_config, operating_mode_code, update_heating_control, zone_id_from_config,
    },
};

use hares_physics::constants::CP_LIQUID_WATER_J_KG_K;

/// Default condensing boiler outlet temperature [C] (150 F).
/// ASHRAE typical hydronic condensing temperature.
const DEFAULT_CONDENSING_OUTLET_TEMP_C: f64 = 65.56;

/// Default non-condensing boiler outlet temperature [C] (180 F).
const DEFAULT_NON_CONDENSING_OUTLET_TEMP_C: f64 = 82.22;

/// Default hydronic return water temperature [C] (104 F).
const DEFAULT_RETURN_TEMP_C: f64 = 40.0;

/// Default gas boiler AFUE. DOE 10 CFR Part 430, federal minimum.
const DEFAULT_GAS_BOILER_AFUE: f64 = 0.8;

const DEFAULT_CONDENSING_EIR_COEFFS: [f64; 6] = [
    1.058_343_061,
    -0.052_650_153,
    -0.008_727_2,
    -0.001_742_217,
    0.000_003_337_15,
    0.000_513_723,
];
const DEFAULT_NON_CONDENSING_EIR_COEFFS: [f64; 10] = [
    1.111_720_116,
    0.078_614_078,
    -0.400_425_756,
    0.0,
    -0.000_156_783,
    0.009_384_599,
    0.234_257_955,
    0.000_001_329_27,
    -0.004_446_701,
    -0.000_012_249_8,
];

pub struct ElectricBoiler {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,
    core_output: CoreOutput,
    hvac: HvacEquipment,
    rated_capacity_w: f64,
    efficiency: f64,
    loop_id: LoopId,
    fluid_type: FluidType,
    flow_rate_kg_s: f64,
    default_return_temp_c: f64,
    operating_mode: OperatingMode,
    run_time_s: f64,
}

pub struct GasBoiler {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,
    core_output: CoreOutput,
    hvac: HvacEquipment,
    rated_capacity_w: f64,
    loop_id: LoopId,
    fluid_type: FluidType,
    flow_rate_kg_s: f64,
    default_return_temp_c: f64,
    outlet_temp_c: f64,
    fuel_type: FuelType,
    eir_max: f64,
    pump_kw: f64,
    condensing: bool,
    condensing_eir_coeffs: [f64; 6],
    non_condensing_eir_coeffs: [f64; 10],
    operating_mode: OperatingMode,
    run_time_s: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct BoilerState {
    mode: ThermostatMode,
    duty_cycle: f64,
    last_mode_switch_at: Option<DateTime<FixedOffset>>,
    runtime_setpoints: Option<RuntimeSetpointOverride>,
    operating_mode: OperatingMode,
    run_time_s: f64,
    electric_kw: f64,
    fuel_input_w: f64,
    thermal_output_w: f64,
    eir: f64,
    supply_temp_c: f64,
    return_temp_c: f64,
}

impl ElectricBoiler {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let zone = zone_id_from_config(&config).unwrap_or(ZoneId(1));
        let loop_id =
            loop_id_from_config(&config, &["loop_id", "hydronic_loop_id"]).unwrap_or(LoopId(1));
        let descriptor = EquipmentDescriptor {
            id: EquipmentId(equipment_id_from_config(&config).unwrap_or(0)),
            name: config.name,
            end_use: EndUse::HVAC_HEATING,
            equipment_type: Cow::Borrowed("Electric Boiler"),
            zone: Some(zone),
            fuel: FuelType::Electric,
            stage: ExecutionStage::Thermal,
            control_capabilities: ControlCapabilities::THERMAL_SETPOINT
                | ControlCapabilities::THERMAL_SETPOINT_DELTA
                | ControlCapabilities::IDEAL_CAPACITY,
            core_capabilities: CoreCapabilities::ELECTRIC | CoreCapabilities::HAS_MODE,
            telemetry_fields: electric_boiler_telemetry_fields(),
        };

        Self {
            descriptor,
            ports: vec![
                PortDeclaration::electrical(),
                PortDeclaration::fluid(loop_id, FluidType::Water),
            ],
            telemetry: electric_boiler_default_telemetry(),
            core_output: CoreOutput::default(),
            hvac: HvacEquipment::new(HvacEquipmentType::Other, zone),
            rated_capacity_w: 0.0,
            efficiency: 1.0,
            loop_id,
            fluid_type: FluidType::Water,
            flow_rate_kg_s: 0.0,
            default_return_temp_c: DEFAULT_RETURN_TEMP_C,
            operating_mode: OperatingMode::Off,
            run_time_s: 0.0,
        }
    }
}

impl Equipment for ElectricBoiler {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        self.hvac.init(config, env)?;
        if config.is_typed() {
            let typed = config.typed::<ElectricBoilerConfig>()?;
            self.rated_capacity_w = typed.capacity_w.max(0.0);
            self.efficiency = typed.eir;
            if let Some(lid) = typed.loop_id {
                self.loop_id = LoopId(lid);
            }
            self.fluid_type = typed.fluid_type;
            self.flow_rate_kg_s = typed.flow_rate_kg_s.max(0.0);
            self.default_return_temp_c = typed.return_temp_c;
        } else {
            self.rated_capacity_w = first_f64(
                config,
                &["capacity_w", "heating_capacity_w", "capacity", "HVAC Heating Capacity (W)"],
            )
                .unwrap_or(0.0)
                .max(0.0);
            self.efficiency = first_f64(config, &["eir", "efficiency"]).unwrap_or(1.0);
            if let Some(lid) = loop_id_from_config(config, &["loop_id", "hydronic_loop_id"]) {
                self.loop_id = lid;
            }
            self.flow_rate_kg_s = first_f64(config, &["flow_rate_kg_s"])
                .unwrap_or(0.5)
                .max(0.0);
            self.default_return_temp_c = first_f64(config, &["return_temp_c"]).unwrap_or(40.0);
        }
        if self.efficiency <= 0.0 || !self.efficiency.is_finite() {
            return Err(HaresError::Equipment(format!(
                "invalid Electric Boiler eir: {}",
                self.efficiency
            )));
        }
        self.hvac.heating_capacities_w = vec![self.rated_capacity_w];
        self.ports[1].loop_id = Some(self.loop_id);
        self.operating_mode = OperatingMode::Off;
        self.run_time_s = 0.0;
        self.telemetry = electric_boiler_default_telemetry();
        self.core_output = CoreOutput::default();
        Ok(())
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        self.operating_mode = update_heating_control(&mut self.hvac, env);
        self.operating_mode
    }

    fn step(
        &mut self,
        env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        let duty = self.hvac.duty_cycle.clamp(0.0, 1.0);
        let sf = self.hvac.space_fraction;
        let thermal_output_w = self.rated_capacity_w * duty;
        let electric_kw = (thermal_output_w / self.efficiency) / 1_000.0 * sf;

        let return_temp_c =
            loop_return_temp_c(env, self.loop_id).unwrap_or(self.default_return_temp_c);
        let supply_temp_c = if self.flow_rate_kg_s > 0.0 {
            return_temp_c + thermal_output_w / (self.flow_rate_kg_s * CP_LIQUID_WATER_J_KG_K)
        } else {
            return_temp_c
        };

        if electric_kw > 0.0 {
            ports.accumulate(&PortContribution::Electrical {
                active_power_kw: electric_kw,
                reactive_power_kvar: 0.0,
            })?;
        }

        if thermal_output_w > 0.0 {
            ports.accumulate(&PortContribution::Fluid {
                loop_id: self.loop_id,
                flow_rate_kg_s: self.flow_rate_kg_s,
                supply_temp_c,
                return_temp_c,
                fluid_type: self.fluid_type,
            })?;
            self.run_time_s += dt.as_secs_f64();
        }

        self.telemetry.set(tk::ELECTRIC_KW, electric_kw);
        self.telemetry.set(tk::THERMAL_OUTPUT_W, thermal_output_w);
        self.telemetry.set(tk::SUPPLY_TEMP_C, supply_temp_c);
        self.telemetry.set(tk::RETURN_TEMP_C, return_temp_c);
        self.telemetry
            .set(tk::OPERATING_MODE, operating_mode_code(self.operating_mode));
        self.core_output = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(electric_kw.max(0.0))),
                reactive_power_kvar: None,
                fuel_w: None,
            },
            state: CoreState {
                operating_mode: Some(self.operating_mode),
                soc: None,
            },
        };

        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn core_output(&self) -> &CoreOutput {
        &self.core_output
    }

    fn save_state(&self) -> Vec<u8> {
        save_postcard(&BoilerState {
            mode: self.hvac.mode,
            duty_cycle: self.hvac.duty_cycle,
            last_mode_switch_at: self.hvac.last_mode_switch_at,
            runtime_setpoints: self.hvac.runtime_setpoints,
            operating_mode: self.operating_mode,
            run_time_s: self.run_time_s,
            electric_kw: self.telemetry.get(tk::ELECTRIC_KW).unwrap_or(0.0),
            fuel_input_w: 0.0,
            thermal_output_w: self.telemetry.get(tk::THERMAL_OUTPUT_W).unwrap_or(0.0),
            eir: 1.0 / self.efficiency,
            supply_temp_c: self.telemetry.get(tk::SUPPLY_TEMP_C).unwrap_or(0.0),
            return_temp_c: self.telemetry.get(tk::RETURN_TEMP_C).unwrap_or(0.0),
        })
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let decoded: BoilerState = load_postcard(state)?;
        self.hvac.mode = decoded.mode;
        self.hvac.duty_cycle = decoded.duty_cycle;
        self.hvac.last_mode_switch_at = decoded.last_mode_switch_at;
        self.hvac.runtime_setpoints = decoded.runtime_setpoints;
        self.operating_mode = decoded.operating_mode;
        self.run_time_s = decoded.run_time_s;

        self.telemetry.insert(tk::ELECTRIC_KW, decoded.electric_kw);
        self.telemetry
            .insert(tk::THERMAL_OUTPUT_W, decoded.thermal_output_w);
        self.telemetry
            .insert(tk::SUPPLY_TEMP_C, decoded.supply_temp_c);
        self.telemetry
            .insert(tk::RETURN_TEMP_C, decoded.return_temp_c);
        self.telemetry.insert(
            tk::OPERATING_MODE,
            operating_mode_code(decoded.operating_mode),
        );
        self.core_output = CoreOutput::default();
        Ok(())
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        apply_heating_control_unchecked(&mut self.hvac, signal, "Electric Boiler")
    }
}

impl GasBoiler {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let zone = zone_id_from_config(&config).unwrap_or(ZoneId(1));
        let loop_id =
            loop_id_from_config(&config, &["loop_id", "hydronic_loop_id"]).unwrap_or(LoopId(1));
        let descriptor = EquipmentDescriptor {
            id: EquipmentId(equipment_id_from_config(&config).unwrap_or(0)),
            name: config.name,
            end_use: EndUse::HVAC_HEATING,
            equipment_type: Cow::Borrowed("Gas Boiler"),
            zone: Some(zone),
            fuel: FuelType::Gas,
            stage: ExecutionStage::Thermal,
            control_capabilities: ControlCapabilities::THERMAL_SETPOINT
                | ControlCapabilities::THERMAL_SETPOINT_DELTA
                | ControlCapabilities::IDEAL_CAPACITY,
            core_capabilities: CoreCapabilities::ELECTRIC
                | CoreCapabilities::FUEL
                | CoreCapabilities::HAS_MODE,
            telemetry_fields: gas_boiler_telemetry_fields(),
        };

        Self {
            descriptor,
            ports: vec![
                PortDeclaration::fuel(),
                PortDeclaration::electrical(),
                PortDeclaration::thermal(zone),
                PortDeclaration::fluid(loop_id, FluidType::Water),
            ],
            telemetry: gas_boiler_default_telemetry(),
            core_output: CoreOutput::default(),
            hvac: HvacEquipment::new(HvacEquipmentType::Other, zone),
            rated_capacity_w: 0.0,
            loop_id,
            fluid_type: FluidType::Water,
            flow_rate_kg_s: 0.0,
            default_return_temp_c: DEFAULT_RETURN_TEMP_C,
            outlet_temp_c: DEFAULT_CONDENSING_OUTLET_TEMP_C,
            fuel_type: FuelType::Gas,
            eir_max: 1.0 / DEFAULT_GAS_BOILER_AFUE,
            pump_kw: 0.0,
            condensing: false,
            condensing_eir_coeffs: DEFAULT_CONDENSING_EIR_COEFFS,
            non_condensing_eir_coeffs: DEFAULT_NON_CONDENSING_EIR_COEFFS,
            operating_mode: OperatingMode::Off,
            run_time_s: 0.0,
        }
    }

    fn current_eir(&self, plr: f64, t_in_c: f64) -> crate::Result<f64> {
        let curve = if self.condensing {
            let c = self.condensing_eir_coeffs;
            c[0] + c[1] * plr
                + c[2] * plr * plr
                + c[3] * t_in_c
                + c[4] * t_in_c * t_in_c
                + c[5] * plr * t_in_c
        } else {
            let c = self.non_condensing_eir_coeffs;
            let t_out = self.outlet_temp_c;
            c[0] + c[1] * plr
                + c[2] * plr * plr
                + c[3] * t_out
                + c[4] * t_out * t_out
                + c[5] * plr * t_out
                + c[6] * plr * plr * plr
                + c[7] * t_out * t_out * t_out
                + c[8] * plr * plr * t_out
                + c[9] * plr * t_out * t_out
        };
        if curve <= 0.0 || !curve.is_finite() {
            return Err(HaresError::Equipment(format!(
                "invalid Gas Boiler polynomial output: {curve}"
            )));
        }
        Ok(self.eir_max / curve)
    }
}

impl Equipment for GasBoiler {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        self.hvac.init(config, env)?;
        let fuel_efficiency = if config.is_typed() {
            let typed = config.typed::<GasBoilerConfig>()?;
            self.rated_capacity_w = typed.capacity_w.max(0.0);
            if let Some(lid) = typed.loop_id {
                self.loop_id = LoopId(lid);
            }
            self.fluid_type = typed.fluid_type;
            self.flow_rate_kg_s = typed.flow_rate_kg_s.max(0.0);
            self.default_return_temp_c = typed.return_temp_c;
            self.pump_kw = typed.fan_power_w.unwrap_or(0.0) / 1_000.0;
            typed.afue
        } else {
            self.rated_capacity_w = first_f64(
                config,
                &["capacity_w", "heating_capacity_w", "capacity", "HVAC Heating Capacity (W)"],
            )
            .unwrap_or(0.0)
            .max(0.0);
            if let Some(lid) = loop_id_from_config(config, &["loop_id", "hydronic_loop_id"]) {
                self.loop_id = lid;
            }
            self.flow_rate_kg_s = first_f64(config, &["flow_rate_kg_s"])
                .unwrap_or(0.5)
                .max(0.0);
            self.default_return_temp_c = first_f64(config, &["return_temp_c"]).unwrap_or(40.0);
            self.pump_kw = first_f64(config, &["fan_power_w"]).unwrap_or(0.0) / 1_000.0;
            first_f64(config, &["afue", "fuel_efficiency", "efficiency"]).unwrap_or(0.8)
        };
        if fuel_efficiency <= 0.0 || !fuel_efficiency.is_finite() {
            return Err(HaresError::Equipment(format!(
                "invalid Gas Boiler efficiency: {fuel_efficiency}"
            )));
        }
        self.condensing = fuel_efficiency > 0.9;
        self.outlet_temp_c = if self.condensing {
            DEFAULT_CONDENSING_OUTLET_TEMP_C
        } else {
            DEFAULT_NON_CONDENSING_OUTLET_TEMP_C
        };
        self.eir_max = 1.0 / fuel_efficiency;
        self.condensing_eir_coeffs = DEFAULT_CONDENSING_EIR_COEFFS;
        self.non_condensing_eir_coeffs = DEFAULT_NON_CONDENSING_EIR_COEFFS;
        self.hvac.heating_capacities_w = vec![self.rated_capacity_w];
        self.ports[3].loop_id = Some(self.loop_id);
        self.operating_mode = OperatingMode::Off;
        self.run_time_s = 0.0;
        self.telemetry = gas_boiler_default_telemetry();
        self.core_output = CoreOutput::default();
        Ok(())
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        self.operating_mode = update_heating_control(&mut self.hvac, env);
        self.operating_mode
    }

    fn step(
        &mut self,
        env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        let plr = self.hvac.duty_cycle.clamp(0.0, 1.0);
        let thermal_output_w = self.rated_capacity_w * plr;
        let return_temp_c =
            loop_return_temp_c(env, self.loop_id).unwrap_or(self.default_return_temp_c);
        // Condensing EIR polynomial uses zone air temperature (OCHRE HVAC.py:651:
        // t_in = self.zone.temperature), not return water temperature.
        let zone_temp_c = env
            .zones
            .iter()
            .find(|z| z.id == self.hvac.zone_id)
            .map(|z| z.temperature_c)
            .unwrap_or(20.0);
        let eir = if thermal_output_w > 0.0 {
            self.current_eir(plr, zone_temp_c)?
        } else {
            self.eir_max
        };
        let sf = self.hvac.space_fraction;
        let fuel_input_w = thermal_output_w * eir * sf;

        let supply_temp_c = if self.flow_rate_kg_s > 0.0 {
            return_temp_c + thermal_output_w / (self.flow_rate_kg_s * CP_LIQUID_WATER_J_KG_K)
        } else {
            return_temp_c
        };

        let electric_kw = if thermal_output_w > 0.0 {
            self.pump_kw * sf
        } else {
            0.0
        };

        if fuel_input_w > 0.0 {
            ports.accumulate(&PortContribution::Fuel {
                fuel_type: self.fuel_type,
                consumption_w: fuel_input_w,
            })?;
        }
        if electric_kw > 0.0 {
            ports.accumulate(&PortContribution::Electrical {
                active_power_kw: electric_kw,
                reactive_power_kvar: 0.0,
            })?;
        }
        if thermal_output_w > 0.0 {
            ports.accumulate(&PortContribution::Fluid {
                loop_id: self.loop_id,
                flow_rate_kg_s: self.flow_rate_kg_s,
                supply_temp_c,
                return_temp_c,
                fluid_type: self.fluid_type,
            })?;
            self.run_time_s += dt.as_secs_f64();
        }

        // Jacket loss: fuel energy minus useful thermal output, plus pump electrical.
        // For condensing boilers (EIR < 1), jacket loss is zero — the extra output
        // comes from latent heat recovery, not from the room.
        // space_fraction scales the fuel input; thermal_output_w is the fraction this
        // boiler serves, so both sides must be in the same frame.
        let thermal_output_sf_w = thermal_output_w * sf;
        let jacket_loss_w = (fuel_input_w + electric_kw * 1e3 - thermal_output_sf_w).max(0.0);
        if let Some(zone) = self.descriptor.zone {
            if jacket_loss_w > 0.0 {
                ports.accumulate(&PortContribution::Thermal {
                    zone,
                    sensible_gain_w: jacket_loss_w,
                    latent_gain_w: 0.0,
                    category: ThermalCategory::JacketLoss,
                })?;
            }
        }

        self.telemetry.set(tk::ELECTRIC_KW, electric_kw);
        self.telemetry.set(tk::FUEL_INPUT_W, fuel_input_w);
        self.telemetry.set(tk::THERMAL_OUTPUT_W, thermal_output_w);
        self.telemetry.set(tk::JACKET_LOSS_W, jacket_loss_w);
        self.telemetry.set(tk::EIR, eir);
        self.telemetry.set(tk::SUPPLY_TEMP_C, supply_temp_c);
        self.telemetry.set(tk::RETURN_TEMP_C, return_temp_c);
        self.telemetry
            .set(tk::OPERATING_MODE, operating_mode_code(self.operating_mode));
        self.core_output = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(electric_kw.max(0.0))),
                reactive_power_kvar: None,
                fuel_w: Some(FuelPower {
                    fuel_type: self.fuel_type,
                    consumption_w: fuel_input_w.max(0.0),
                }),
            },
            state: CoreState {
                operating_mode: Some(self.operating_mode),
                soc: None,
            },
        };

        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn core_output(&self) -> &CoreOutput {
        &self.core_output
    }

    fn save_state(&self) -> Vec<u8> {
        save_postcard(&BoilerState {
            mode: self.hvac.mode,
            duty_cycle: self.hvac.duty_cycle,
            last_mode_switch_at: self.hvac.last_mode_switch_at,
            runtime_setpoints: self.hvac.runtime_setpoints,
            operating_mode: self.operating_mode,
            run_time_s: self.run_time_s,
            electric_kw: self.telemetry.get(tk::ELECTRIC_KW).unwrap_or(0.0),
            fuel_input_w: self.telemetry.get(tk::FUEL_INPUT_W).unwrap_or(0.0),
            thermal_output_w: self.telemetry.get(tk::THERMAL_OUTPUT_W).unwrap_or(0.0),
            eir: self.telemetry.get(tk::EIR).unwrap_or(self.eir_max),
            supply_temp_c: self.telemetry.get(tk::SUPPLY_TEMP_C).unwrap_or(0.0),
            return_temp_c: self.telemetry.get(tk::RETURN_TEMP_C).unwrap_or(0.0),
        })
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let decoded: BoilerState = load_postcard(state)?;
        self.hvac.mode = decoded.mode;
        self.hvac.duty_cycle = decoded.duty_cycle;
        self.hvac.last_mode_switch_at = decoded.last_mode_switch_at;
        self.hvac.runtime_setpoints = decoded.runtime_setpoints;
        self.operating_mode = decoded.operating_mode;
        self.run_time_s = decoded.run_time_s;

        self.telemetry.insert(tk::ELECTRIC_KW, decoded.electric_kw);
        self.telemetry
            .insert(tk::FUEL_INPUT_W, decoded.fuel_input_w);
        self.telemetry
            .insert(tk::THERMAL_OUTPUT_W, decoded.thermal_output_w);
        self.telemetry.insert(tk::EIR, decoded.eir);
        self.telemetry
            .insert(tk::SUPPLY_TEMP_C, decoded.supply_temp_c);
        self.telemetry
            .insert(tk::RETURN_TEMP_C, decoded.return_temp_c);
        self.telemetry.insert(
            tk::OPERATING_MODE,
            operating_mode_code(decoded.operating_mode),
        );
        self.core_output = CoreOutput::default();
        Ok(())
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        apply_heating_control_unchecked(&mut self.hvac, signal, "Gas Boiler")
    }
}

pub fn register_with_registry(registry: &mut EquipmentRegistry) {
    registry.register(
        "Electric Boiler",
        Box::new(|config| Box::new(ElectricBoiler::new(config))),
    );
    registry.register(
        "Gas Boiler",
        Box::new(|config| Box::new(GasBoiler::new(config))),
    );
}

fn loop_return_temp_c(env: &EnvironmentState, loop_id: LoopId) -> Option<f64> {
    let payload = env
        .custom_domains
        .iter()
        .find(|update| update.domain_id == FLUID)?
        .custom_payload
        .as_ref()?;
    let states = FluidDomainPayload::decode(payload).ok()?;
    states
        .into_iter()
        .find(|state| state.loop_id == loop_id)
        .map(|state| state.mean_return_temp_c)
}

fn electric_boiler_default_telemetry() -> Telemetry {
    let mut telemetry = Telemetry::with_capacity(5);
    telemetry.insert(tk::ELECTRIC_KW, 0.0);
    telemetry.insert(tk::THERMAL_OUTPUT_W, 0.0);
    telemetry.insert(tk::SUPPLY_TEMP_C, 0.0);
    telemetry.insert(tk::RETURN_TEMP_C, 0.0);
    telemetry.insert(tk::OPERATING_MODE, 0.0);
    telemetry
}

fn gas_boiler_default_telemetry() -> Telemetry {
    let mut telemetry = Telemetry::with_capacity(8);
    telemetry.insert(tk::ELECTRIC_KW, 0.0);
    telemetry.insert(tk::FUEL_INPUT_W, 0.0);
    telemetry.insert(tk::THERMAL_OUTPUT_W, 0.0);
    telemetry.insert(tk::JACKET_LOSS_W, 0.0);
    telemetry.insert(tk::EIR, 0.0);
    telemetry.insert(tk::SUPPLY_TEMP_C, 0.0);
    telemetry.insert(tk::RETURN_TEMP_C, 0.0);
    telemetry.insert(tk::OPERATING_MODE, 0.0);
    telemetry
}

fn electric_boiler_telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField {
            name: tk::ELECTRIC_KW.to_string(),
            unit: "kW".to_string(),
            description: "Electric boiler active power draw".to_string(),
        },
        TelemetryField {
            name: tk::THERMAL_OUTPUT_W.to_string(),
            unit: "W".to_string(),
            description: "Thermal output transferred to hydronic loop".to_string(),
        },
        TelemetryField {
            name: tk::SUPPLY_TEMP_C.to_string(),
            unit: "C".to_string(),
            description: "Fluid supply temperature".to_string(),
        },
        TelemetryField {
            name: tk::RETURN_TEMP_C.to_string(),
            unit: "C".to_string(),
            description: "Fluid return temperature".to_string(),
        },
        TelemetryField {
            name: tk::OPERATING_MODE.to_string(),
            unit: "enum".to_string(),
            description: "Operating mode code: 0=Off, 1=Heating".to_string(),
        },
    ]
}

fn gas_boiler_telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField {
            name: tk::ELECTRIC_KW.to_string(),
            unit: "kW".to_string(),
            description: "Gas boiler auxiliary electrical draw (pump/fan)".to_string(),
        },
        TelemetryField {
            name: tk::FUEL_INPUT_W.to_string(),
            unit: "W".to_string(),
            description: "Gas boiler fuel input power".to_string(),
        },
        TelemetryField {
            name: tk::THERMAL_OUTPUT_W.to_string(),
            unit: "W".to_string(),
            description: "Thermal output transferred to hydronic loop".to_string(),
        },
        TelemetryField {
            name: tk::JACKET_LOSS_W.to_string(),
            unit: "W".to_string(),
            description: "Jacket heat loss to zone (fuel_input - thermal_output)".to_string(),
        },
        TelemetryField {
            name: tk::EIR.to_string(),
            unit: "-".to_string(),
            description: "Instantaneous energy-input ratio after polynomial adjustment".to_string(),
        },
        TelemetryField {
            name: tk::SUPPLY_TEMP_C.to_string(),
            unit: "C".to_string(),
            description: "Fluid supply temperature".to_string(),
        },
        TelemetryField {
            name: tk::RETURN_TEMP_C.to_string(),
            unit: "C".to_string(),
            description: "Fluid return temperature".to_string(),
        },
        TelemetryField {
            name: tk::OPERATING_MODE.to_string(),
            unit: "enum".to_string(),
            description: "Operating mode code: 0=Off, 1=Heating".to_string(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        DomainUpdate, EnvironmentState, ExecutionStage, FLUID, FluidDomainPayload, FluidLoopState,
        FluidType, GridState, LoopId, PortSlots, ThermalAccumulator, WeatherState, ZoneId,
        ZoneState, telemetry_keys as tk,
    };

    use super::{
        DEFAULT_CONDENSING_EIR_COEFFS, DEFAULT_NON_CONDENSING_EIR_COEFFS, ElectricBoiler, GasBoiler,
    };

    use crate::hvac::heating_config::{ElectricBoilerConfig, GasBoilerConfig};
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
            equipment_core: std::collections::HashMap::new(),
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

    fn eb_config(capacity_w: f64, efficiency: f64) -> EquipmentConfig {
        EquipmentConfig::from_typed(
            "EB".to_string(),
            "Electric Boiler".to_string(),
            ElectricBoilerConfig {
                zone_id: Some(1),
                loop_id: Some(1),
                eir: efficiency,
                capacity_w,
                ..ElectricBoilerConfig::default()
            },
        )
    }

    fn gb_config(capacity_w: f64, afue: f64) -> EquipmentConfig {
        EquipmentConfig::from_typed(
            "GB".to_string(),
            "Gas Boiler".to_string(),
            GasBoilerConfig {
                zone_id: Some(1),
                loop_id: Some(1),
                afue,
                capacity_w,
                ..GasBoilerConfig::default()
            },
        )
    }

    #[test]
    fn electric_boiler_writes_electrical_and_fluid_ports() {
        let cfg = eb_config(8_000.0, 0.95);
        let mut eq = ElectricBoiler::new(cfg.clone());
        let env = env(18.0);
        eq.init(&cfg, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        assert!(ports.electrical.net_active_kw() > 0.0);
        assert!(ports.fluid[0].total_flow_kg_s > 0.0);
    }

    #[test]
    fn raw_config_accepted_for_electric_boiler_with_defaults() {
        use crate::config::ConfigPayload;
        let cfg = EquipmentConfig {
            name: "EB".to_string(),
            ochre_class: "Electric Boiler".to_string(),
            payload: ConfigPayload::Raw {
                data: std::collections::HashMap::new(),
            },
        };
        let mut eq = ElectricBoiler::new(cfg.clone());
        let result = eq.init(&cfg, &env(18.0));
        assert!(
            result.is_ok(),
            "Electric Boiler should accept raw config with defaults"
        );
    }

    #[test]
    fn raw_config_accepted_for_gas_boiler_with_defaults() {
        use crate::config::ConfigPayload;
        let cfg = EquipmentConfig {
            name: "GB".to_string(),
            ochre_class: "Gas Boiler".to_string(),
            payload: ConfigPayload::Raw {
                data: std::collections::HashMap::new(),
            },
        };
        let mut eq = GasBoiler::new(cfg.clone());
        let result = eq.init(&cfg, &env(18.0));
        assert!(
            result.is_ok(),
            "Gas Boiler should accept raw config with defaults"
        );
    }

    #[test]
    fn gas_boiler_condensing_curve_matches_expected_at_plr_half() {
        // AFUE > 0.9 → condensing path is selected by init().
        let cfg = gb_config(10_000.0, 0.95);
        let mut eq = GasBoiler::new(cfg.clone());
        let mut env = env(18.0);
        eq.init(&cfg, &env).unwrap();
        // Override eir_max so the expected formula uses the same value.
        eq.eir_max = 1.2;
        eq.condensing_eir_coeffs = DEFAULT_CONDENSING_EIR_COEFFS;

        eq.hvac.duty_cycle = 0.5;
        eq.hvac.mode = super::ThermostatMode::Heating;

        env.custom_domains.push(DomainUpdate {
            domain_id: FLUID,
            zone_temperatures_c: vec![],
            custom_payload: FluidDomainPayload::encode(&[FluidLoopState {
                loop_id: LoopId(1),
                fluid_type: FluidType::Water,
                net_power_w: 0.0,
                mean_supply_temp_c: 45.0,
                mean_return_temp_c: 40.0,
            }]),
        });

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        // Condensing EIR uses zone air temperature (18°C from env), not return
        // water temperature. Reference: OCHRE HVAC.py:651 t_in = self.zone.temperature.
        let plr = 0.5;
        let t_in = 18.0; // zone air temp from env()
        let c = DEFAULT_CONDENSING_EIR_COEFFS;
        let eff_curve = c[0]
            + c[1] * plr
            + c[2] * plr * plr
            + c[3] * t_in
            + c[4] * t_in * t_in
            + c[5] * plr * t_in;
        let expected_eir = 1.2 / eff_curve;
        let observed_eir = eq.telemetry().get(tk::EIR).unwrap();
        assert!(
            (observed_eir - expected_eir).abs() < 1e-4,
            "condensing EIR at PLR=0.5, t_zone=18°C: observed={observed_eir}, expected={expected_eir}"
        );
    }

    #[test]
    fn condensing_is_more_efficient_than_non_condensing_at_partial_load() {
        // AFUE 0.95 → condensing; AFUE 0.8 → non-condensing.
        let cond_cfg = gb_config(10_000.0, 0.95);
        let non_cfg = gb_config(10_000.0, 0.80);

        let mut cond_boiler = GasBoiler::new(cond_cfg.clone());
        let mut non_boiler = GasBoiler::new(non_cfg.clone());
        let env = env(18.0);
        cond_boiler.init(&cond_cfg, &env).unwrap();
        non_boiler.init(&non_cfg, &env).unwrap();
        // Force non-condensing path explicitly so the test intent is clear.
        non_boiler.condensing = false;
        non_boiler.non_condensing_eir_coeffs = DEFAULT_NON_CONDENSING_EIR_COEFFS;

        cond_boiler.hvac.duty_cycle = 0.5;
        cond_boiler.hvac.mode = super::ThermostatMode::Heating;
        non_boiler.hvac.duty_cycle = 0.5;
        non_boiler.hvac.mode = super::ThermostatMode::Heating;

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..PortSlots::default()
        };

        cond_boiler
            .step(&env, Duration::from_secs(60), &mut ports)
            .unwrap();
        ports.zero();
        non_boiler
            .step(&env, Duration::from_secs(60), &mut ports)
            .unwrap();

        let cond_eff = 1.0 / cond_boiler.telemetry().get(tk::EIR).unwrap();
        let non_eff = 1.0 / non_boiler.telemetry().get(tk::EIR).unwrap();
        assert!(cond_eff > non_eff);
    }

    #[test]
    fn gas_boiler_state_round_trip_preserves_outputs() {
        let cfg = gb_config(10_000.0, 0.80);
        let mut eq = GasBoiler::new(cfg.clone());
        let env = env(18.0);
        eq.init(&cfg, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        let state = eq.save_state();

        let mut restored = GasBoiler::new(cfg.clone());
        restored.init(&cfg, &env).unwrap();
        restored.load_state(&state).unwrap();
        assert_eq!(
            restored.telemetry().get(tk::FUEL_INPUT_W),
            eq.telemetry().get(tk::FUEL_INPUT_W)
        );
    }

    /// Non-condensing EIR at PLR=0.5, outlet_temp=82.22°C against OCHRE-derived
    /// reference. The 10-coefficient polynomial uses outlet water temperature.
    #[test]
    fn non_condensing_eir_at_plr_half_matches_ochre_reference() {
        // AFUE 0.8 → non-condensing path.
        let cfg = gb_config(10_000.0, 0.80);
        let mut eq = GasBoiler::new(cfg.clone());
        let env = env(18.0);
        eq.init(&cfg, &env).unwrap();

        eq.hvac.duty_cycle = 0.5;
        eq.hvac.mode = super::ThermostatMode::Heating;

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        // Compute expected EIR from the 10-coeff polynomial at PLR=0.5, t_out=82.22°C
        let plr = 0.5;
        let t_out = 82.22; // default non-condensing outlet temp
        let c = DEFAULT_NON_CONDENSING_EIR_COEFFS;
        let curve = c[0]
            + c[1] * plr
            + c[2] * plr * plr
            + c[3] * t_out
            + c[4] * t_out * t_out
            + c[5] * plr * t_out
            + c[6] * plr * plr * plr
            + c[7] * t_out * t_out * t_out
            + c[8] * plr * plr * t_out
            + c[9] * plr * t_out * t_out;
        let eir_max = 1.0 / 0.8;
        let expected_eir = eir_max / curve;
        let observed_eir = eq.telemetry().get(tk::EIR).unwrap();
        assert!(
            (observed_eir - expected_eir).abs() < 1e-4,
            "non-condensing EIR at PLR=0.5, t_out=82.22°C: observed={observed_eir}, expected={expected_eir}"
        );

        // Verify EIR varies from eir_max (not constant efficiency)
        assert!(
            (observed_eir - eir_max).abs() > 1e-6,
            "non-condensing EIR should vary with PLR, not be constant"
        );
    }

    #[test]
    fn gas_boiler_jacket_loss_writes_to_thermal_port_and_conserves_energy() {
        // Non-condensing boiler at 80% AFUE: jacket loss = fuel_input - thermal_output.
        let cfg = gb_config(10_000.0, 0.80);
        let mut eq = GasBoiler::new(cfg.clone());
        let env = env(18.0);
        eq.init(&cfg, &env).unwrap();

        // Force full-load operation.
        eq.hvac.duty_cycle = 1.0;
        eq.hvac.mode = super::ThermostatMode::Heating;

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let fuel_input_w = eq.telemetry().get(tk::FUEL_INPUT_W).unwrap();
        let thermal_output_w = eq.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap();
        let jacket_loss_w = eq.telemetry().get(tk::JACKET_LOSS_W).unwrap();

        // Fuel > thermal output for a sub-unity efficiency boiler.
        assert!(
            fuel_input_w > thermal_output_w,
            "fuel_input_w={fuel_input_w} should exceed thermal_output_w={thermal_output_w}"
        );

        // Energy conservation: jacket_loss = fuel_input - thermal_output.
        let expected_jacket = fuel_input_w - thermal_output_w;
        assert!(
            (jacket_loss_w - expected_jacket).abs() < 1e-6,
            "jacket_loss_w={jacket_loss_w} != fuel_input_w - thermal_output_w={expected_jacket}"
        );

        // Jacket loss appears in the zone thermal port under JacketLoss category.
        use hares_types::ThermalCategory;
        let zone_jacket = ports.thermal[0].sensible_for_category(ThermalCategory::JacketLoss);
        assert!(
            (zone_jacket - jacket_loss_w).abs() < 1e-6,
            "thermal port jacket loss={zone_jacket} != telemetry jacket_loss_w={jacket_loss_w}"
        );

        // Total sensible gain on the zone equals the jacket loss (only contribution).
        assert!(
            (ports.thermal[0].sensible_gain_w - jacket_loss_w).abs() < 1e-6,
            "total sensible gain={} should equal jacket_loss_w={}",
            ports.thermal[0].sensible_gain_w,
            jacket_loss_w
        );
    }

    #[test]
    fn gas_boiler_no_jacket_loss_when_off() {
        let cfg = gb_config(10_000.0, 0.80);
        let mut eq = GasBoiler::new(cfg.clone());
        let env = env(22.0); // above setpoint — boiler should be off
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        use hares_types::ThermalCategory;
        assert_eq!(
            ports.thermal[0].sensible_for_category(ThermalCategory::JacketLoss),
            0.0,
            "jacket loss must be zero when boiler is off"
        );
        assert_eq!(
            eq.telemetry().get(tk::JACKET_LOSS_W).unwrap(),
            0.0,
            "telemetry jacket_loss_w must be zero when boiler is off"
        );
    }

    #[test]
    fn registry_includes_boiler_aliases_and_thermal_stage() {
        let registry = EquipmentRegistry::new();
        assert!(registry.get("Electric Boiler").is_some());
        assert!(registry.get("Gas Boiler").is_some());

        let eq = registry
            .create("Gas Boiler", gb_config(10_000.0, 0.80))
            .unwrap();
        assert_eq!(eq.descriptor().stage, ExecutionStage::Thermal);
    }
}
