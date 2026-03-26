//! Electric and gas boiler models.

use std::borrow::Cow;
use std::time::Duration;

use chrono::{DateTime, FixedOffset};
use hares_types::{
    ControlCapabilities, ControlSignal, EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId,
    ExecutionStage, FLUID, FluidDomainPayload, FluidType, FuelType, HaresError, LoopId,
    OperatingMode, PortContribution, PortDeclaration, PortSlots, Telemetry, TelemetryField,
    ThermalCategory, ZoneId,
};
use serde::{Deserialize, Serialize};

use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_postcard, save_postcard};

use super::{
    HvacEquipment, HvacEquipmentType, RuntimeSetpointOverride, ThermostatMode,
    helpers::{
        HEATING_CAPACITY_KEYS, apply_heating_control_unchecked, equipment_id_from_config,
        first_f64, loop_id_from_config, operating_mode_code, parse_fluid_type, parse_fuel_type,
        update_heating_control, zone_id_from_config,
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
                | ControlCapabilities::THERMAL_SETPOINT_DELTA,
            telemetry_fields: electric_boiler_telemetry_fields(),
        };

        Self {
            descriptor,
            ports: vec![
                PortDeclaration::electrical(),
                PortDeclaration::fluid(loop_id, FluidType::Water),
            ],
            telemetry: electric_boiler_default_telemetry(),
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
        self.rated_capacity_w = first_f64(config, HEATING_CAPACITY_KEYS)
            .unwrap_or(12_000.0)
            .max(0.0);
        self.efficiency = first_f64(config, &["efficiency", "boiler_efficiency"]).unwrap_or(1.0);
        if self.efficiency <= 0.0 || !self.efficiency.is_finite() {
            return Err(HaresError::Equipment(format!(
                "invalid Electric Boiler efficiency: {}",
                self.efficiency
            )));
        }

        self.loop_id =
            loop_id_from_config(config, &["loop_id", "hydronic_loop_id"]).unwrap_or(self.loop_id);
        self.fluid_type =
            parse_fluid_type(config.get_str("fluid_type")).unwrap_or(FluidType::Water);
        self.flow_rate_kg_s = first_f64(config, &["flow_rate_kg_s", "boiler_flow_rate_kg_s"])
            .unwrap_or(0.5)
            .max(0.0);
        self.default_return_temp_c =
            first_f64(config, &["return_temp_c", "inlet_temp_c"]).unwrap_or(DEFAULT_RETURN_TEMP_C);

        self.hvac.heating_capacities_w = vec![self.rated_capacity_w];
        self.ports[1].loop_id = Some(self.loop_id);
        self.operating_mode = OperatingMode::Off;
        self.run_time_s = 0.0;
        self.telemetry = electric_boiler_default_telemetry();

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

        self.telemetry.set("electric_kw", electric_kw);
        self.telemetry.set("thermal_output_w", thermal_output_w);
        self.telemetry.set("supply_temp_c", supply_temp_c);
        self.telemetry.set("return_temp_c", return_temp_c);
        self.telemetry
            .set("operating_mode", operating_mode_code(self.operating_mode));

        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn save_state(&self) -> Vec<u8> {
        save_postcard(&BoilerState {
            mode: self.hvac.mode,
            duty_cycle: self.hvac.duty_cycle,
            last_mode_switch_at: self.hvac.last_mode_switch_at,
            runtime_setpoints: self.hvac.runtime_setpoints,
            operating_mode: self.operating_mode,
            run_time_s: self.run_time_s,
            electric_kw: self.telemetry.get("electric_kw").unwrap_or(0.0),
            fuel_input_w: 0.0,
            thermal_output_w: self.telemetry.get("thermal_output_w").unwrap_or(0.0),
            eir: 1.0 / self.efficiency,
            supply_temp_c: self.telemetry.get("supply_temp_c").unwrap_or(0.0),
            return_temp_c: self.telemetry.get("return_temp_c").unwrap_or(0.0),
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

        self.telemetry.insert("electric_kw", decoded.electric_kw);
        self.telemetry
            .insert("thermal_output_w", decoded.thermal_output_w);
        self.telemetry
            .insert("supply_temp_c", decoded.supply_temp_c);
        self.telemetry
            .insert("return_temp_c", decoded.return_temp_c);
        self.telemetry.insert(
            "operating_mode",
            operating_mode_code(decoded.operating_mode),
        );
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
                | ControlCapabilities::THERMAL_SETPOINT_DELTA,
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

        self.rated_capacity_w = first_f64(config, HEATING_CAPACITY_KEYS)
            .unwrap_or(12_000.0)
            .max(0.0);
        let fuel_efficiency = first_f64(config, &["fuel_efficiency", "afue", "efficiency"])
            .unwrap_or(DEFAULT_GAS_BOILER_AFUE);
        if fuel_efficiency <= 0.0 || !fuel_efficiency.is_finite() {
            return Err(HaresError::Equipment(format!(
                "invalid Gas Boiler efficiency: {fuel_efficiency}"
            )));
        }

        self.loop_id =
            loop_id_from_config(config, &["loop_id", "hydronic_loop_id"]).unwrap_or(self.loop_id);
        self.fluid_type =
            parse_fluid_type(config.get_str("fluid_type")).unwrap_or(FluidType::Water);
        self.flow_rate_kg_s = first_f64(config, &["flow_rate_kg_s", "boiler_flow_rate_kg_s"])
            .unwrap_or(0.5)
            .max(0.0);
        self.default_return_temp_c =
            first_f64(config, &["return_temp_c", "inlet_temp_c"]).unwrap_or(DEFAULT_RETURN_TEMP_C);
        self.fuel_type = parse_fuel_type(config.get_str("fuel_type")).unwrap_or(FuelType::Gas);
        self.descriptor.fuel = self.fuel_type;

        self.condensing = config
            .get_bool("condensing")
            .or_else(|| config.get_f64("condensing").map(|v| v > 0.0))
            .unwrap_or(fuel_efficiency > 0.9);
        self.outlet_temp_c = first_f64(config, &["outlet_temp_c", "outlet_water_temp_c"])
            .unwrap_or(if self.condensing {
                DEFAULT_CONDENSING_OUTLET_TEMP_C
            } else {
                DEFAULT_NON_CONDENSING_OUTLET_TEMP_C
            });

        self.condensing_eir_coeffs = parse_coeff_array(
            config,
            "condensing_eir_coeffs",
            DEFAULT_CONDENSING_EIR_COEFFS,
        )?;
        self.non_condensing_eir_coeffs = parse_coeff_array(
            config,
            "non_condensing_eir_coeffs",
            DEFAULT_NON_CONDENSING_EIR_COEFFS,
        )?;
        self.eir_max = first_f64(config, &["eir_max"]).unwrap_or(1.0 / fuel_efficiency);
        self.pump_kw = first_f64(config, &["pump_kw", "fan_kw", "parasitic_kw"])
            .or_else(|| first_f64(config, &["auxiliary_power_w"]).map(|w| w / 1_000.0))
            .unwrap_or(0.0);

        self.hvac.heating_capacities_w = vec![self.rated_capacity_w];
        self.ports[3].loop_id = Some(self.loop_id);
        self.operating_mode = OperatingMode::Off;
        self.run_time_s = 0.0;
        self.telemetry = gas_boiler_default_telemetry();

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

        self.telemetry.set("electric_kw", electric_kw);
        self.telemetry.set("fuel_input_w", fuel_input_w);
        self.telemetry.set("thermal_output_w", thermal_output_w);
        self.telemetry.set("jacket_loss_w", jacket_loss_w);
        self.telemetry.set("eir", eir);
        self.telemetry.set("supply_temp_c", supply_temp_c);
        self.telemetry.set("return_temp_c", return_temp_c);
        self.telemetry
            .set("operating_mode", operating_mode_code(self.operating_mode));

        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn save_state(&self) -> Vec<u8> {
        save_postcard(&BoilerState {
            mode: self.hvac.mode,
            duty_cycle: self.hvac.duty_cycle,
            last_mode_switch_at: self.hvac.last_mode_switch_at,
            runtime_setpoints: self.hvac.runtime_setpoints,
            operating_mode: self.operating_mode,
            run_time_s: self.run_time_s,
            electric_kw: self.telemetry.get("electric_kw").unwrap_or(0.0),
            fuel_input_w: self.telemetry.get("fuel_input_w").unwrap_or(0.0),
            thermal_output_w: self.telemetry.get("thermal_output_w").unwrap_or(0.0),
            eir: self.telemetry.get("eir").unwrap_or(self.eir_max),
            supply_temp_c: self.telemetry.get("supply_temp_c").unwrap_or(0.0),
            return_temp_c: self.telemetry.get("return_temp_c").unwrap_or(0.0),
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

        self.telemetry.insert("electric_kw", decoded.electric_kw);
        self.telemetry.insert("fuel_input_w", decoded.fuel_input_w);
        self.telemetry
            .insert("thermal_output_w", decoded.thermal_output_w);
        self.telemetry.insert("eir", decoded.eir);
        self.telemetry
            .insert("supply_temp_c", decoded.supply_temp_c);
        self.telemetry
            .insert("return_temp_c", decoded.return_temp_c);
        self.telemetry.insert(
            "operating_mode",
            operating_mode_code(decoded.operating_mode),
        );
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
    telemetry.insert("electric_kw", 0.0);
    telemetry.insert("thermal_output_w", 0.0);
    telemetry.insert("supply_temp_c", 0.0);
    telemetry.insert("return_temp_c", 0.0);
    telemetry.insert("operating_mode", 0.0);
    telemetry
}

fn gas_boiler_default_telemetry() -> Telemetry {
    let mut telemetry = Telemetry::with_capacity(8);
    telemetry.insert("electric_kw", 0.0);
    telemetry.insert("fuel_input_w", 0.0);
    telemetry.insert("thermal_output_w", 0.0);
    telemetry.insert("jacket_loss_w", 0.0);
    telemetry.insert("eir", 0.0);
    telemetry.insert("supply_temp_c", 0.0);
    telemetry.insert("return_temp_c", 0.0);
    telemetry.insert("operating_mode", 0.0);
    telemetry
}

fn electric_boiler_telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField {
            name: "electric_kw".to_string(),
            unit: "kW".to_string(),
            description: "Electric boiler active power draw".to_string(),
        },
        TelemetryField {
            name: "thermal_output_w".to_string(),
            unit: "W".to_string(),
            description: "Thermal output transferred to hydronic loop".to_string(),
        },
        TelemetryField {
            name: "supply_temp_c".to_string(),
            unit: "C".to_string(),
            description: "Fluid supply temperature".to_string(),
        },
        TelemetryField {
            name: "return_temp_c".to_string(),
            unit: "C".to_string(),
            description: "Fluid return temperature".to_string(),
        },
        TelemetryField {
            name: "operating_mode".to_string(),
            unit: "enum".to_string(),
            description: "Operating mode code: 0=Off, 1=Heating".to_string(),
        },
    ]
}

fn gas_boiler_telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField {
            name: "electric_kw".to_string(),
            unit: "kW".to_string(),
            description: "Gas boiler auxiliary electrical draw (pump/fan)".to_string(),
        },
        TelemetryField {
            name: "fuel_input_w".to_string(),
            unit: "W".to_string(),
            description: "Gas boiler fuel input power".to_string(),
        },
        TelemetryField {
            name: "thermal_output_w".to_string(),
            unit: "W".to_string(),
            description: "Thermal output transferred to hydronic loop".to_string(),
        },
        TelemetryField {
            name: "jacket_loss_w".to_string(),
            unit: "W".to_string(),
            description: "Jacket heat loss to zone (fuel_input - thermal_output)".to_string(),
        },
        TelemetryField {
            name: "eir".to_string(),
            unit: "-".to_string(),
            description: "Instantaneous energy-input ratio after polynomial adjustment".to_string(),
        },
        TelemetryField {
            name: "supply_temp_c".to_string(),
            unit: "C".to_string(),
            description: "Fluid supply temperature".to_string(),
        },
        TelemetryField {
            name: "return_temp_c".to_string(),
            unit: "C".to_string(),
            description: "Fluid return temperature".to_string(),
        },
        TelemetryField {
            name: "operating_mode".to_string(),
            unit: "enum".to_string(),
            description: "Operating mode code: 0=Off, 1=Heating".to_string(),
        },
    ]
}

fn parse_coeff_array<const N: usize>(
    config: &EquipmentConfig,
    key: &str,
    default: [f64; N],
) -> crate::Result<[f64; N]> {
    if let Some(raw) = config.get_str(key) {
        return parse_coeff_string(raw, key);
    }

    let mut values = [0.0; N];
    let mut any_found = false;
    for (i, slot) in values.iter_mut().enumerate() {
        let indexed_key = format!("{key}_{i}");
        if let Some(value) = config.get_f64(&indexed_key) {
            any_found = true;
            *slot = value;
        } else if any_found {
            return Err(HaresError::Equipment(format!(
                "missing coefficient {indexed_key} while parsing {key}"
            )));
        } else {
            return Ok(default);
        }
    }
    Ok(values)
}

fn parse_coeff_string<const N: usize>(value: &str, key: &str) -> crate::Result<[f64; N]> {
    let trimmed = value.trim().trim_start_matches('[').trim_end_matches(']');
    let parts: Vec<&str> = trimmed
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .collect();
    if parts.len() != N {
        return Err(HaresError::Equipment(format!(
            "invalid coefficient count for {key}: expected {N}, got {}",
            parts.len()
        )));
    }

    let mut out = [0.0; N];
    for (slot, token) in out.iter_mut().zip(parts) {
        *slot = token.parse::<f64>().map_err(|err| {
            HaresError::Equipment(format!("invalid coefficient in {key}: {token} ({err})"))
        })?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, time::Duration};

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        DomainUpdate, EnvironmentState, ExecutionStage, FLUID, FluidDomainPayload, FluidLoopState,
        FluidType, GridState, LoopId, PortSlots, ThermalAccumulator, WeatherState, ZoneId,
        ZoneState,
    };

    use super::{
        DEFAULT_CONDENSING_EIR_COEFFS, DEFAULT_NON_CONDENSING_EIR_COEFFS, ElectricBoiler,
        GasBoiler, register_with_registry,
    };
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
            current_time: FixedOffset::east_opt(0)
                .unwrap()
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
    fn electric_boiler_writes_electrical_and_fluid_ports() {
        let mut cfg = config("EB", "Electric Boiler");
        cfg.raw_config.insert("zone_id".to_string(), 1.0.into());
        cfg.raw_config.insert("loop_id".to_string(), 1.0.into());
        cfg.raw_config
            .insert("capacity_w".to_string(), 8_000.0.into());
        cfg.raw_config.insert("efficiency".to_string(), 0.95.into());
        cfg.raw_config
            .insert("flow_rate_kg_s".to_string(), 0.5.into());
        cfg.raw_config
            .insert("heating_setpoint_c".to_string(), 21.0.into());
        cfg.raw_config
            .insert("cooling_setpoint_c".to_string(), 27.0.into());

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
    fn gas_boiler_condensing_curve_matches_expected_at_plr_half() {
        let mut cfg = config("GB", "Gas Boiler");
        cfg.raw_config.insert("zone_id".to_string(), 1.0.into());
        cfg.raw_config.insert("loop_id".to_string(), 1.0.into());
        cfg.raw_config
            .insert("capacity_w".to_string(), 10_000.0.into());
        cfg.raw_config.insert("eir_max".to_string(), 1.2.into());
        cfg.raw_config.insert("condensing".to_string(), 1.0.into());
        cfg.raw_config
            .insert("flow_rate_kg_s".to_string(), 0.5.into());
        cfg.raw_config.insert(
            "condensing_eir_coeffs".to_string(),
            format!(
                "[{},{},{},{},{},{}]",
                DEFAULT_CONDENSING_EIR_COEFFS[0],
                DEFAULT_CONDENSING_EIR_COEFFS[1],
                DEFAULT_CONDENSING_EIR_COEFFS[2],
                DEFAULT_CONDENSING_EIR_COEFFS[3],
                DEFAULT_CONDENSING_EIR_COEFFS[4],
                DEFAULT_CONDENSING_EIR_COEFFS[5]
            )
            .into(),
        );
        cfg.raw_config
            .insert("heating_setpoint_c".to_string(), 21.0.into());
        cfg.raw_config
            .insert("cooling_setpoint_c".to_string(), 27.0.into());

        let mut eq = GasBoiler::new(cfg.clone());
        let mut env = env(18.0);
        eq.init(&cfg, &env).unwrap();

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
        let observed_eir = eq.telemetry().get("eir").unwrap();
        assert!(
            (observed_eir - expected_eir).abs() < 1e-4,
            "condensing EIR at PLR=0.5, t_zone=18°C: observed={observed_eir}, expected={expected_eir}"
        );
    }

    #[test]
    fn condensing_is_more_efficient_than_non_condensing_at_partial_load() {
        let mut cond = config("GB Cond", "Gas Boiler");
        cond.raw_config.insert("zone_id".to_string(), 1.0.into());
        cond.raw_config.insert("loop_id".to_string(), 1.0.into());
        cond.raw_config
            .insert("capacity_w".to_string(), 10_000.0.into());
        cond.raw_config
            .insert("fuel_efficiency".to_string(), 0.95.into());
        cond.raw_config.insert("condensing".to_string(), 1.0.into());
        cond.raw_config
            .insert("flow_rate_kg_s".to_string(), 0.5.into());
        cond.raw_config
            .insert("heating_setpoint_c".to_string(), 21.0.into());
        cond.raw_config
            .insert("cooling_setpoint_c".to_string(), 27.0.into());

        let mut non = cond.clone();
        non.raw_config.insert("condensing".to_string(), 0.0.into());
        non.raw_config
            .insert("fuel_efficiency".to_string(), 0.8.into());
        non.raw_config.insert(
            "non_condensing_eir_coeffs".to_string(),
            format!(
                "[{},{},{},{},{},{},{},{},{},{}]",
                DEFAULT_NON_CONDENSING_EIR_COEFFS[0],
                DEFAULT_NON_CONDENSING_EIR_COEFFS[1],
                DEFAULT_NON_CONDENSING_EIR_COEFFS[2],
                DEFAULT_NON_CONDENSING_EIR_COEFFS[3],
                DEFAULT_NON_CONDENSING_EIR_COEFFS[4],
                DEFAULT_NON_CONDENSING_EIR_COEFFS[5],
                DEFAULT_NON_CONDENSING_EIR_COEFFS[6],
                DEFAULT_NON_CONDENSING_EIR_COEFFS[7],
                DEFAULT_NON_CONDENSING_EIR_COEFFS[8],
                DEFAULT_NON_CONDENSING_EIR_COEFFS[9]
            )
            .into(),
        );

        let mut cond_boiler = GasBoiler::new(cond.clone());
        let mut non_boiler = GasBoiler::new(non.clone());
        let env = env(18.0);
        cond_boiler.init(&cond, &env).unwrap();
        non_boiler.init(&non, &env).unwrap();

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

        let cond_eff = 1.0 / cond_boiler.telemetry().get("eir").unwrap();
        let non_eff = 1.0 / non_boiler.telemetry().get("eir").unwrap();
        assert!(cond_eff > non_eff);
    }

    #[test]
    fn gas_boiler_state_round_trip_preserves_outputs() {
        let mut cfg = config("GB", "Gas Boiler");
        cfg.raw_config.insert("zone_id".to_string(), 1.0.into());
        cfg.raw_config.insert("loop_id".to_string(), 1.0.into());
        cfg.raw_config
            .insert("capacity_w".to_string(), 10_000.0.into());
        cfg.raw_config
            .insert("flow_rate_kg_s".to_string(), 0.5.into());
        cfg.raw_config
            .insert("heating_setpoint_c".to_string(), 21.0.into());
        cfg.raw_config
            .insert("cooling_setpoint_c".to_string(), 27.0.into());

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
            restored.telemetry().get("fuel_input_w"),
            eq.telemetry().get("fuel_input_w")
        );
    }

    /// Non-condensing EIR at PLR=0.5, outlet_temp=82.22°C against OCHRE-derived
    /// reference. The 10-coefficient polynomial uses outlet water temperature.
    #[test]
    fn non_condensing_eir_at_plr_half_matches_ochre_reference() {
        let mut cfg = config("GB", "Gas Boiler");
        cfg.raw_config.insert("zone_id".to_string(), 1.0.into());
        cfg.raw_config.insert("loop_id".to_string(), 1.0.into());
        cfg.raw_config
            .insert("capacity_w".to_string(), 10_000.0.into());
        cfg.raw_config
            .insert("fuel_efficiency".to_string(), 0.8.into());
        cfg.raw_config.insert("condensing".to_string(), 0.0.into());
        cfg.raw_config
            .insert("flow_rate_kg_s".to_string(), 0.5.into());
        cfg.raw_config
            .insert("heating_setpoint_c".to_string(), 21.0.into());
        cfg.raw_config
            .insert("cooling_setpoint_c".to_string(), 27.0.into());

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
        let observed_eir = eq.telemetry().get("eir").unwrap();
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
        let mut cfg = config("GB", "Gas Boiler");
        cfg.raw_config.insert("zone_id".to_string(), 1.0.into());
        cfg.raw_config.insert("loop_id".to_string(), 1.0.into());
        cfg.raw_config
            .insert("capacity_w".to_string(), 10_000.0.into());
        cfg.raw_config
            .insert("fuel_efficiency".to_string(), 0.8.into());
        cfg.raw_config.insert("condensing".to_string(), 0.0.into());
        cfg.raw_config
            .insert("flow_rate_kg_s".to_string(), 0.5.into());
        cfg.raw_config
            .insert("heating_setpoint_c".to_string(), 21.0.into());
        cfg.raw_config
            .insert("cooling_setpoint_c".to_string(), 27.0.into());

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

        let fuel_input_w = eq.telemetry().get("fuel_input_w").unwrap();
        let thermal_output_w = eq.telemetry().get("thermal_output_w").unwrap();
        let jacket_loss_w = eq.telemetry().get("jacket_loss_w").unwrap();

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
        let mut cfg = config("GB off", "Gas Boiler");
        cfg.raw_config.insert("zone_id".to_string(), 1.0.into());
        cfg.raw_config.insert("loop_id".to_string(), 1.0.into());
        cfg.raw_config
            .insert("capacity_w".to_string(), 10_000.0.into());
        cfg.raw_config
            .insert("fuel_efficiency".to_string(), 0.8.into());
        cfg.raw_config
            .insert("flow_rate_kg_s".to_string(), 0.5.into());
        cfg.raw_config
            .insert("heating_setpoint_c".to_string(), 21.0.into());
        cfg.raw_config
            .insert("cooling_setpoint_c".to_string(), 27.0.into());

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
            eq.telemetry().get("jacket_loss_w").unwrap(),
            0.0,
            "telemetry jacket_loss_w must be zero when boiler is off"
        );
    }

    #[test]
    fn registry_includes_boiler_aliases_and_thermal_stage() {
        let mut registry = EquipmentRegistry::new();
        register_with_registry(&mut registry);
        assert!(registry.get("Electric Boiler").is_some());
        assert!(registry.get("Gas Boiler").is_some());

        let eq = registry
            .create("Gas Boiler", config("GB", "Gas Boiler"))
            .unwrap();
        assert_eq!(eq.descriptor().stage, ExecutionStage::Thermal);
    }
}
