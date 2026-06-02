//! Electric and gas boiler models.

use std::borrow::Cow;
use std::time::Duration;

use chrono::{DateTime, FixedOffset};
use hares_types::{
    ControlCapabilities, ControlSignal, CoreCapabilities, CoreFlows, CoreOutput, CorePerformance,
    CoreState, DRLevel, ElectricPower, EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId,
    ExecutionStage, FLUID, FluidDomainPayload, FluidType, FuelPower, FuelType, HaresError, LoopId,
    OperatingMode, PortContribution, PortDeclaration, PortSlots, Telemetry, TelemetryField,
    ThermalCategory,
};
use serde::{Deserialize, Serialize};

use hares_types::telemetry_keys as tk;

use crate::hvac::heating_config::{ElectricBoilerConfig, GasBoilerConfig};
use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_versioned, try_save_versioned};

use super::{
    HvacEquipment, HvacEquipmentType, RuntimeSetpointOverride, ThermostatMode,
    helpers::{
        apply_heating_control_unchecked, apply_simple_heating_ideal_capacity_control,
        apply_simple_mode_override_and_dr, apply_simple_mode_override_in_control,
        equipment_id_from_config, loop_id_from_config, operating_mode_code, update_heating_control,
        zone_id_from_config_or_default,
    },
};

use hares_physics::constants::cp_j_kg_k;
use hares_physics::units::{power_kw_to_w, power_w_to_kw};

/// Default condensing boiler outlet temperature [C] (150 F).
/// ASHRAE HVAC Systems and Equipment Ch.32 "Boilers": condensing boilers
/// are designed for ~65.6 °C (150 °F) outlet temperature to maximize
/// flue-gas condensation and latent-heat recovery.
const DEFAULT_CONDENSING_OUTLET_TEMP_C: f64 = 65.56;

/// Default non-condensing boiler outlet temperature [C] (180 F).
/// ASHRAE HVAC Systems and Equipment Ch.32 "Boilers": traditional
/// non-condensing cast-iron boilers typically operate at 82.2 °C (180 °F)
/// supply temperature to prevent flue-gas condensation and corrosion of
/// the heat exchanger.
const DEFAULT_NON_CONDENSING_OUTLET_TEMP_C: f64 = 82.22;

/// Default hydronic return water temperature [°C] (158 °F).
/// ASHRAE HVAC Systems and Equipment Ch.32 "Boilers": non-condensing boilers
/// must maintain return water temperature >= 70 °C to avoid sustained
/// flue-gas condensation and corrosion.
const DEFAULT_RETURN_TEMP_C: f64 = 70.0;

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
    eir: f64,
    loop_id: LoopId,
    fluid_type: FluidType,
    flow_rate_kg_s: f64,
    default_return_temp_c: f64,
    operating_mode: OperatingMode,
    run_time_s: f64,
    /// Cached from last update_control; true when timestep >= 5 min
    /// so ideal_target() can participate in the solver feedback loop.
    use_ideal: bool,
    /// Whether zone_id was explicitly set in config or fell back to ZoneId(1).
    zone_id_explicit: bool,
    /// External ModeOverride control (sticky).
    mode_override: Option<OperatingMode>,
    /// External DemandResponse level (sticky).
    dr_level: DRLevel,
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
    /// Cached from last update_control; true when timestep >= 5 min
    /// so ideal_target() can participate in the solver feedback loop.
    use_ideal: bool,
    /// Whether zone_id was explicitly set in config or fell back to ZoneId(1).
    zone_id_explicit: bool,
    /// External ModeOverride control (sticky).
    mode_override: Option<OperatingMode>,
    /// External DemandResponse level (sticky).
    dr_level: DRLevel,
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
    mode_override: Option<OperatingMode>,
    dr_level: DRLevel,
}

impl ElectricBoiler {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let (zone, zone_id_explicit) = zone_id_from_config_or_default(&config, &config.name);
        let loop_id =
            loop_id_from_config(&config, &["loop_id", "hydronic_loop_id"]).unwrap_or_default();
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
                | ControlCapabilities::IDEAL_CAPACITY
                | ControlCapabilities::MODE_OVERRIDE
                | ControlCapabilities::DEMAND_RESPONSE,
            core_capabilities: CoreCapabilities::ELECTRIC
                | CoreCapabilities::HAS_MODE
                | CoreCapabilities::THERMAL
                | CoreCapabilities::HAS_SETPOINT,
            telemetry_fields: electric_boiler_telemetry_fields(),
            zone_type: None,
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
            eir: 1.0,
            loop_id,
            fluid_type: FluidType::Water,
            flow_rate_kg_s: 0.0,
            default_return_temp_c: DEFAULT_RETURN_TEMP_C,
            operating_mode: OperatingMode::Off,
            run_time_s: 0.0,
            use_ideal: false,
            zone_id_explicit,
            mode_override: None,
            dr_level: DRLevel::Normal,
        }
    }
}

impl Equipment for ElectricBoiler {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn zone_id_explicit(&self) -> bool {
        self.zone_id_explicit
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        self.hvac.init(config, env)?;
        let typed = config.require_typed::<ElectricBoilerConfig>("Electric Boiler")?;
        self.rated_capacity_w = typed.capacity_w.max(0.0);
        self.eir = typed.eir;
        if let Some(lid) = typed.loop_id {
            self.loop_id = LoopId(lid);
        }
        self.fluid_type = typed.fluid_type;
        self.flow_rate_kg_s = typed.flow_rate_kg_s.max(0.0);
        self.default_return_temp_c = typed.return_temp_c;
        if self.eir <= 0.0 || !self.eir.is_finite() {
            return Err(HaresError::Equipment(format!(
                "invalid Electric Boiler eir: {}",
                self.eir
            )));
        }
        self.hvac.config.heating_capacities_w = vec![self.rated_capacity_w];
        self.hvac.update_zone_heat_fractions();
        self.hvac.rebuild_thermal_ports(&mut self.ports, false);
        self.ports[1].loop_id = Some(self.loop_id);
        self.operating_mode = OperatingMode::Off;
        self.run_time_s = 0.0;
        self.telemetry = electric_boiler_default_telemetry();
        self.core_output = CoreOutput::default();
        Ok(())
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        self.use_ideal = self.hvac.use_ideal_capacity(env);
        if let Some(mode) = apply_simple_mode_override_in_control(
            &mut self.hvac,
            &mut self.mode_override,
            self.dr_level,
            "Electric Boiler",
        ) {
            self.operating_mode = mode;
            return mode;
        }
        self.operating_mode = update_heating_control(&mut self.hvac, env);
        self.operating_mode
    }

    fn step(
        &mut self,
        env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        let duty = self.hvac.runtime.duty_cycle.clamp(0.0, 1.0);
        let sf = self.hvac.config.space_fraction;
        let thermal_output_w = self.rated_capacity_w * duty * sf;
        let electric_kw = power_w_to_kw(thermal_output_w * self.eir);

        let return_temp_c =
            loop_return_temp_c(env, self.loop_id).unwrap_or(self.default_return_temp_c);
        let cp_used = cp_j_kg_k(self.fluid_type);
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            assert!(
                cp_used > 0.0 && cp_used.is_finite(),
                "ElectricBoiler fluid_type {:?} returned invalid cp {cp_used}",
                self.fluid_type
            );
        }
        let supply_temp_c = if self.flow_rate_kg_s > 0.0 {
            return_temp_c + thermal_output_w / (self.flow_rate_kg_s * cp_used)
        } else {
            return_temp_c
        };

        if electric_kw > 0.0 {
            ports.accumulate(&PortContribution::Electrical {
                active_power_w: power_kw_to_w(electric_kw),
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
                thermal_power_w: None,
            })?;
            self.hvac.write_zone_thermal_contributions(
                ports,
                thermal_output_w,
                0.0,
                ThermalCategory::HvacHeating,
            )?;
            self.run_time_s += dt.as_secs_f64();
        }

        self.telemetry.set(tk::ELECTRIC_KW, electric_kw);
        self.telemetry.set(tk::THERMAL_OUTPUT_W, thermal_output_w);
        self.telemetry.set(tk::BOILER_CP_USED_J_KG_K, cp_used);
        self.telemetry.set(tk::SUPPLY_TEMP_C, supply_temp_c);
        self.telemetry.set(tk::RETURN_TEMP_C, return_temp_c);
        self.telemetry
            .set(tk::OPERATING_MODE, operating_mode_code(self.operating_mode));
        let sp = self.hvac.effective_setpoints();
        // Heating-only equipment: setpoint_c is always the heating setpoint (per
        // validate_core_contract requirement that HAS_SETPOINT => setpoint_c is Some).
        let active_setpoint_c = sp.heating_c;
        self.core_output = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(electric_kw.max(0.0))),
                reactive_power_kvar: None,
                fuel_w: None,
                thermal_output_w: Some(thermal_output_w),
                sensible_cooling_w: None,
                latent_cooling_w: None,
            },
            state: CoreState {
                operating_mode: Some(self.operating_mode),
                soc: None,
                speed_index: None,
                setpoint_c: Some(active_setpoint_c),
            },
            performance: CorePerformance::default(),
        };
        self.telemetry
            .set(tk::HEATING_SETPOINT_C, active_setpoint_c);

        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn core_output(&self) -> &CoreOutput {
        &self.core_output
    }

    fn save_state(&self) -> crate::Result<Vec<u8>> {
        try_save_versioned(
            &BoilerState {
                mode: self.hvac.thermostat_fsm.mode,
                duty_cycle: self.hvac.runtime.duty_cycle,
                last_mode_switch_at: self.hvac.thermostat_fsm.last_mode_switch_at,
                runtime_setpoints: self.hvac.thermostat_fsm.runtime_setpoints,
                operating_mode: self.operating_mode,
                run_time_s: self.run_time_s,
                electric_kw: self.telemetry.get(tk::ELECTRIC_KW).unwrap_or(0.0),
                fuel_input_w: 0.0,
                thermal_output_w: self.telemetry.get(tk::THERMAL_OUTPUT_W).unwrap_or(0.0),
                eir: self.eir,
                supply_temp_c: self.telemetry.get(tk::SUPPLY_TEMP_C).unwrap_or(0.0),
                return_temp_c: self.telemetry.get(tk::RETURN_TEMP_C).unwrap_or(0.0),
                mode_override: self.mode_override,
                dr_level: self.dr_level,
            },
            Self::checkpoint_version(),
            "ElectricBoiler",
        )
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let decoded: BoilerState = load_versioned(
            state,
            Self::checkpoint_version(),
            "ElectricBoiler",
            self.descriptor().id,
        )?;
        self.hvac.thermostat_fsm.mode = decoded.mode;
        self.hvac.runtime.duty_cycle = decoded.duty_cycle;
        self.hvac.thermostat_fsm.last_mode_switch_at = decoded.last_mode_switch_at;
        self.hvac.thermostat_fsm.runtime_setpoints = decoded.runtime_setpoints;
        self.operating_mode = decoded.operating_mode;
        self.run_time_s = decoded.run_time_s;
        self.mode_override = decoded.mode_override;
        self.dr_level = decoded.dr_level;

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
        if apply_simple_mode_override_and_dr(
            &mut self.mode_override,
            &mut self.dr_level,
            signal,
            "Electric Boiler",
        )? {
            return Ok(());
        }
        apply_heating_control_unchecked(&mut self.hvac, signal, "Electric Boiler")?;
        apply_simple_heating_ideal_capacity_control(&mut self.hvac, signal, self.rated_capacity_w);
        Ok(())
    }

    fn ideal_target(&self) -> Option<(hares_types::ZoneId, f64)> {
        if !self.use_ideal {
            return None;
        }
        let setpoint = self.hvac.effective_setpoints().heating_c;
        Some((self.hvac.config.zone_id, setpoint))
    }
}

impl GasBoiler {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let (zone, zone_id_explicit) = zone_id_from_config_or_default(&config, &config.name);
        let loop_id =
            loop_id_from_config(&config, &["loop_id", "hydronic_loop_id"]).unwrap_or_default();
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
                | ControlCapabilities::IDEAL_CAPACITY
                | ControlCapabilities::MODE_OVERRIDE
                | ControlCapabilities::DEMAND_RESPONSE,
            core_capabilities: CoreCapabilities::ELECTRIC
                | CoreCapabilities::FUEL
                | CoreCapabilities::HAS_MODE
                | CoreCapabilities::THERMAL
                | CoreCapabilities::HAS_SETPOINT,
            telemetry_fields: gas_boiler_telemetry_fields(),
            zone_type: None,
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
            use_ideal: false,
            zone_id_explicit,
            mode_override: None,
            dr_level: DRLevel::Normal,
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

    fn zone_id_explicit(&self) -> bool {
        self.zone_id_explicit
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        self.hvac.init(config, env)?;
        let typed = config.require_typed::<GasBoilerConfig>("Gas Boiler")?;
        typed.validate()?;
        self.rated_capacity_w = typed.capacity_w.max(0.0);
        if let Some(lid) = typed.loop_id {
            self.loop_id = LoopId(lid);
        }
        self.fluid_type = typed.fluid_type;
        self.flow_rate_kg_s = typed.flow_rate_kg_s.max(0.0);
        self.default_return_temp_c = typed.return_temp_c;
        self.pump_kw = power_w_to_kw(typed.fan_power_w.unwrap_or(0.0));
        let fuel_efficiency = typed.afue;
        // Condensing mode inferred from AFUE > 0.90 (OCHRE convention).
        // The typed config also carries `condensing: bool` (set by resolver) for
        // explicit override and round-trip fidelity; the runtime computation from
        // AFUE guarantees correctness regardless of config source.
        self.condensing = typed.condensing || fuel_efficiency > 0.9;
        self.outlet_temp_c = if self.condensing {
            DEFAULT_CONDENSING_OUTLET_TEMP_C
        } else {
            DEFAULT_NON_CONDENSING_OUTLET_TEMP_C
        };
        self.eir_max = 1.0 / fuel_efficiency;
        self.condensing_eir_coeffs = DEFAULT_CONDENSING_EIR_COEFFS;
        self.non_condensing_eir_coeffs = DEFAULT_NON_CONDENSING_EIR_COEFFS;
        self.hvac.config.heating_capacities_w = vec![self.rated_capacity_w];
        self.hvac.update_zone_heat_fractions();
        self.hvac.rebuild_thermal_ports(&mut self.ports, false);
        self.ports[3].loop_id = Some(self.loop_id);
        self.operating_mode = OperatingMode::Off;
        self.run_time_s = 0.0;
        self.telemetry = gas_boiler_default_telemetry();
        self.core_output = CoreOutput::default();
        Ok(())
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        self.use_ideal = self.hvac.use_ideal_capacity(env);
        if let Some(mode) = apply_simple_mode_override_in_control(
            &mut self.hvac,
            &mut self.mode_override,
            self.dr_level,
            "Gas Boiler",
        ) {
            self.operating_mode = mode;
            return mode;
        }
        self.operating_mode = update_heating_control(&mut self.hvac, env);
        self.operating_mode
    }

    fn step(
        &mut self,
        env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        let plr = self.hvac.runtime.duty_cycle.clamp(0.0, 1.0);
        let sf = self.hvac.config.space_fraction;
        let thermal_output_w = self.rated_capacity_w * plr * sf;
        let return_temp_c =
            loop_return_temp_c(env, self.loop_id).unwrap_or(self.default_return_temp_c);
        // Condensing EIR polynomial uses zone air temperature (OCHRE HVAC.py:651:
        // t_in = self.zone.temperature), not return water temperature.
        let zone_temp_c = env
            .zones
            .iter()
            .find(|z| z.id == self.hvac.config.zone_id)
            .map(|z| z.temperature_c)
            .unwrap_or(20.0);
        let eir = if thermal_output_w > 0.0 {
            self.current_eir(plr, zone_temp_c)?
        } else {
            self.eir_max
        };
        let fuel_input_w = thermal_output_w * eir;

        let cp_used = cp_j_kg_k(self.fluid_type);
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            assert!(
                cp_used > 0.0 && cp_used.is_finite(),
                "GasBoiler fluid_type {:?} returned invalid cp {cp_used}",
                self.fluid_type
            );
        }
        let supply_temp_c = if self.flow_rate_kg_s > 0.0 {
            return_temp_c + thermal_output_w / (self.flow_rate_kg_s * cp_used)
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
                active_power_w: power_kw_to_w(electric_kw),
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
                thermal_power_w: None,
            })?;
            self.hvac.write_zone_thermal_contributions(
                ports,
                thermal_output_w,
                0.0,
                ThermalCategory::HvacHeating,
            )?;
            self.run_time_s += dt.as_secs_f64();
        }

        // Jacket loss: fuel energy minus useful thermal output, plus pump electrical.
        // For condensing boilers (EIR < 1), jacket loss is zero -- the extra output
        // comes from latent heat recovery, not from the room.
        let jacket_loss_w = (fuel_input_w + electric_kw * 1e3 - thermal_output_w).max(0.0);
        if let Some(zone) = self.descriptor.zone {
            if jacket_loss_w > 0.0 {
                ports.accumulate(&PortContribution::Thermal {
                    zone,
                    sensible_gain_w: jacket_loss_w,
                    radiant_gain_w: 0.0,
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
        self.telemetry.set(tk::BOILER_CP_USED_J_KG_K, cp_used);
        self.telemetry.set(tk::SUPPLY_TEMP_C, supply_temp_c);
        self.telemetry.set(tk::RETURN_TEMP_C, return_temp_c);
        self.telemetry
            .set(tk::OPERATING_MODE, operating_mode_code(self.operating_mode));
        let sp = self.hvac.effective_setpoints();
        // Heating-only equipment: setpoint_c is always the heating setpoint (per
        // validate_core_contract requirement that HAS_SETPOINT => setpoint_c is Some).
        let active_setpoint_c = sp.heating_c;
        self.core_output = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(electric_kw.max(0.0))),
                reactive_power_kvar: None,
                fuel_w: Some(FuelPower {
                    fuel_type: self.fuel_type,
                    consumption_w: fuel_input_w.max(0.0),
                }),
                thermal_output_w: Some(thermal_output_w),
                sensible_cooling_w: None,
                latent_cooling_w: None,
            },
            state: CoreState {
                operating_mode: Some(self.operating_mode),
                soc: None,
                speed_index: None,
                setpoint_c: Some(active_setpoint_c),
            },
            performance: CorePerformance::default(),
        };
        self.telemetry
            .set(tk::HEATING_SETPOINT_C, active_setpoint_c);

        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn core_output(&self) -> &CoreOutput {
        &self.core_output
    }

    fn save_state(&self) -> crate::Result<Vec<u8>> {
        try_save_versioned(
            &BoilerState {
                mode: self.hvac.thermostat_fsm.mode,
                duty_cycle: self.hvac.runtime.duty_cycle,
                last_mode_switch_at: self.hvac.thermostat_fsm.last_mode_switch_at,
                runtime_setpoints: self.hvac.thermostat_fsm.runtime_setpoints,
                operating_mode: self.operating_mode,
                run_time_s: self.run_time_s,
                electric_kw: self.telemetry.get(tk::ELECTRIC_KW).unwrap_or(0.0),
                fuel_input_w: self.telemetry.get(tk::FUEL_INPUT_W).unwrap_or(0.0),
                thermal_output_w: self.telemetry.get(tk::THERMAL_OUTPUT_W).unwrap_or(0.0),
                eir: self.telemetry.get(tk::EIR).unwrap_or(self.eir_max),
                supply_temp_c: self.telemetry.get(tk::SUPPLY_TEMP_C).unwrap_or(0.0),
                return_temp_c: self.telemetry.get(tk::RETURN_TEMP_C).unwrap_or(0.0),
                mode_override: self.mode_override,
                dr_level: self.dr_level,
            },
            Self::checkpoint_version(),
            "GasBoiler",
        )
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let decoded: BoilerState = load_versioned(
            state,
            Self::checkpoint_version(),
            "GasBoiler",
            self.descriptor().id,
        )?;
        self.hvac.thermostat_fsm.mode = decoded.mode;
        self.hvac.runtime.duty_cycle = decoded.duty_cycle;
        self.hvac.thermostat_fsm.last_mode_switch_at = decoded.last_mode_switch_at;
        self.hvac.thermostat_fsm.runtime_setpoints = decoded.runtime_setpoints;
        self.operating_mode = decoded.operating_mode;
        self.run_time_s = decoded.run_time_s;
        self.mode_override = decoded.mode_override;
        self.dr_level = decoded.dr_level;

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
        if apply_simple_mode_override_and_dr(
            &mut self.mode_override,
            &mut self.dr_level,
            signal,
            "Gas Boiler",
        )? {
            return Ok(());
        }
        apply_heating_control_unchecked(&mut self.hvac, signal, "Gas Boiler")?;
        apply_simple_heating_ideal_capacity_control(&mut self.hvac, signal, self.rated_capacity_w);
        Ok(())
    }

    fn ideal_target(&self) -> Option<(hares_types::ZoneId, f64)> {
        if !self.use_ideal {
            return None;
        }
        let setpoint = self.hvac.effective_setpoints().heating_c;
        Some((self.hvac.config.zone_id, setpoint))
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
    let mut telemetry = Telemetry::with_capacity(8);
    telemetry.insert(tk::ELECTRIC_KW, 0.0);
    telemetry.insert(tk::THERMAL_OUTPUT_W, 0.0);
    telemetry.insert(tk::BOILER_CP_USED_J_KG_K, cp_j_kg_k(FluidType::Water));
    telemetry.insert(tk::SUPPLY_TEMP_C, 0.0);
    telemetry.insert(tk::RETURN_TEMP_C, 0.0);
    telemetry.insert(tk::OPERATING_MODE, 0.0);
    telemetry.insert(tk::HEATING_SETPOINT_C, 0.0);
    telemetry.insert(tk::COOLING_SETPOINT_C, 0.0);
    telemetry
}

fn gas_boiler_default_telemetry() -> Telemetry {
    let mut telemetry = Telemetry::with_capacity(11);
    telemetry.insert(tk::ELECTRIC_KW, 0.0);
    telemetry.insert(tk::FUEL_INPUT_W, 0.0);
    telemetry.insert(tk::THERMAL_OUTPUT_W, 0.0);
    telemetry.insert(tk::JACKET_LOSS_W, 0.0);
    telemetry.insert(tk::EIR, 0.0);
    telemetry.insert(tk::BOILER_CP_USED_J_KG_K, cp_j_kg_k(FluidType::Water));
    telemetry.insert(tk::SUPPLY_TEMP_C, 0.0);
    telemetry.insert(tk::RETURN_TEMP_C, 0.0);
    telemetry.insert(tk::OPERATING_MODE, 0.0);
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

fn electric_boiler_telemetry_fields() -> Vec<TelemetryField> {
    let mut fields = vec![
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
            name: tk::BOILER_CP_USED_J_KG_K.to_string(),
            unit: "J/(kg*K)".to_string(),
            description: "Effective specific heat used in supply temperature calculation"
                .to_string(),
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
    ];
    fields.extend(setpoint_telemetry_fields());
    fields
}

fn gas_boiler_telemetry_fields() -> Vec<TelemetryField> {
    let mut fields = vec![
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
            name: tk::BOILER_CP_USED_J_KG_K.to_string(),
            unit: "J/(kg*K)".to_string(),
            description: "Effective specific heat used in supply temperature calculation"
                .to_string(),
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
    ];
    fields.extend(setpoint_telemetry_fields());
    fields
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        ControlSignal, DRLevel, DomainUpdate, EnvironmentState, ExecutionStage, FLUID,
        FluidDomainPayload, FluidLoopState, FluidType, GridState, LoopId, OperatingMode, PortSlots,
        ThermalAccumulator, WeatherState, ZoneId, ZoneState, telemetry_keys as tk,
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

    fn eb_config(capacity_w: f64, eir: f64) -> EquipmentConfig {
        EquipmentConfig::from_typed(
            "EB".to_string(),
            "Electric Boiler".to_string(),
            ElectricBoilerConfig {
                zone_id: Some(1),
                loop_id: Some(1),
                eir,
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
        const THERMAL_OUTPUT_W: f64 = 8_000.0;
        const EIR: f64 = 1.05;

        let cfg = eb_config(THERMAL_OUTPUT_W, EIR);
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

        let expected_electric_w = THERMAL_OUTPUT_W * EIR;
        assert!(
            (ports.electrical.net_active_w() - expected_electric_w).abs() < 1.0,
            "electric boiler electric input must equal thermal_output * EIR"
        );
        assert!(ports.fluid[0].total_flow_kg_s > 0.0);
        assert!(
            (eq.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap() - THERMAL_OUTPUT_W).abs() < 1e-9
        );
    }

    #[test]
    fn raw_config_rejected_for_electric_boiler_with_typed_diagnostic() {
        let cfg = EquipmentConfig::raw(
            "EB".to_string(),
            "Electric Boiler".to_string(),
            std::collections::HashMap::new(),
        );
        let mut eq = ElectricBoiler::new(cfg.clone());
        let result = eq.init(&cfg, &env(18.0));
        let err = result.expect_err("raw electric boiler config must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("Electric Boiler requires typed config"));
        assert!(msg.contains("from_typed"));
    }

    #[test]
    fn raw_config_rejected_for_gas_boiler_with_typed_diagnostic() {
        let cfg = EquipmentConfig::raw(
            "GB".to_string(),
            "Gas Boiler".to_string(),
            std::collections::HashMap::new(),
        );
        let mut eq = GasBoiler::new(cfg.clone());
        let result = eq.init(&cfg, &env(18.0));
        let err = result.expect_err("raw gas boiler config must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("Gas Boiler requires typed config"));
        assert!(msg.contains("from_typed"));
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

        eq.hvac.runtime.duty_cycle = 0.5;
        eq.hvac.thermostat_fsm.mode = super::ThermostatMode::Heating;

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
            (observed_eir - expected_eir).abs() < 1e-3,
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

        cond_boiler.hvac.runtime.duty_cycle = 0.5;
        cond_boiler.hvac.thermostat_fsm.mode = super::ThermostatMode::Heating;
        non_boiler.hvac.runtime.duty_cycle = 0.5;
        non_boiler.hvac.thermostat_fsm.mode = super::ThermostatMode::Heating;

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
        let state = eq.save_state().unwrap();

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

        eq.hvac.runtime.duty_cycle = 0.5;
        eq.hvac.thermostat_fsm.mode = super::ThermostatMode::Heating;

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
            (observed_eir - expected_eir).abs() < 1e-3,
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
        eq.hvac.runtime.duty_cycle = 1.0;
        eq.hvac.thermostat_fsm.mode = super::ThermostatMode::Heating;

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

        // Total sensible gain equals thermal output + jacket loss:
        // both the useful heat and the waste heat end up in the zone.
        let total_sensible = ports.thermal[0].sensible_gain_w;
        let expected_total = thermal_output_w + jacket_loss_w;
        assert!(
            (total_sensible - expected_total).abs() < 1e-6,
            "total sensible={:.4} != thermal_output + jacket_loss = {:.4} + {:.4} = {:.4}",
            total_sensible,
            thermal_output_w,
            jacket_loss_w,
            expected_total
        );
    }

    #[test]
    fn gas_boiler_no_jacket_loss_when_off() {
        let cfg = gb_config(10_000.0, 0.80);
        let mut eq = GasBoiler::new(cfg.clone());
        let env = env(22.0); // above setpoint -- boiler should be off
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

    #[test]
    fn electric_boiler_space_fraction_halves_thermal_and_electrical_output() {
        const CAPACITY_W: f64 = 8_000.0;
        const EIR: f64 = 1.0;

        let cfg = eb_config(CAPACITY_W, EIR);
        let env = env(18.0);

        let mut eq_full = ElectricBoiler::new(cfg.clone());
        eq_full.init(&cfg, &env).unwrap();

        let mut eq_half = ElectricBoiler::new(cfg.clone());
        eq_half.init(&cfg, &env).unwrap();
        eq_half.hvac.config.space_fraction = 0.5;

        let mut ports_full = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..PortSlots::default()
        };
        let mut ports_half = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..PortSlots::default()
        };

        eq_full.update_control(&env);
        eq_full
            .step(&env, Duration::from_secs(60), &mut ports_full)
            .unwrap();
        eq_half.update_control(&env);
        eq_half
            .step(&env, Duration::from_secs(60), &mut ports_half)
            .unwrap();

        let thermal_full = eq_full.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap();
        let thermal_half = eq_half.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap();
        assert!(
            (thermal_half - thermal_full * 0.5).abs() < 1e-6,
            "space_fraction=0.5 must halve thermal output: full={thermal_full}, half={thermal_half}"
        );

        let elec_full = ports_full.electrical.net_active_w();
        let elec_half = ports_half.electrical.net_active_w();
        assert!(
            (elec_half - elec_full * 0.5).abs() < 1e-6,
            "space_fraction=0.5 must halve electrical consumption: full={elec_full}, half={elec_half}"
        );
    }

    #[test]
    fn gas_boiler_space_fraction_halves_thermal_and_fuel_output() {
        const CAPACITY_W: f64 = 10_000.0;
        const AFUE: f64 = 0.80;

        let cfg = gb_config(CAPACITY_W, AFUE);
        let env = env(18.0);

        let mut eq_full = GasBoiler::new(cfg.clone());
        eq_full.init(&cfg, &env).unwrap();

        let mut eq_half = GasBoiler::new(cfg.clone());
        eq_half.init(&cfg, &env).unwrap();
        eq_half.hvac.config.space_fraction = 0.5;

        let mut ports_full = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..PortSlots::default()
        };
        let mut ports_half = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..PortSlots::default()
        };

        eq_full.update_control(&env);
        eq_full
            .step(&env, Duration::from_secs(60), &mut ports_full)
            .unwrap();
        eq_half.update_control(&env);
        eq_half
            .step(&env, Duration::from_secs(60), &mut ports_half)
            .unwrap();

        let thermal_full = eq_full.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap();
        let thermal_half = eq_half.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap();
        assert!(
            (thermal_half - thermal_full * 0.5).abs() < 1e-6,
            "space_fraction=0.5 must halve thermal output: full={thermal_full}, half={thermal_half}"
        );

        let fuel_full = eq_full.telemetry().get(tk::FUEL_INPUT_W).unwrap();
        let fuel_half = eq_half.telemetry().get(tk::FUEL_INPUT_W).unwrap();
        assert!(
            (fuel_half - fuel_full * 0.5).abs() < 1e-6,
            "space_fraction=0.5 must halve fuel consumption: full={fuel_full}, half={fuel_half}"
        );
    }

    #[test]
    fn electric_boiler_ideal_capacity_control_scales_output() {
        let cfg = eb_config(8_000.0, 1.0);
        let mut eq = ElectricBoiler::new(cfg.clone());
        let env = env(18.0);
        eq.init(&cfg, &env).unwrap();
        eq.apply_control(&ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(21.0),
            cooling_setpoint_c: None,
            deadband_c: None,
        })
        .unwrap();
        eq.apply_control(&ControlSignal::IdealCapacity {
            capacity_w: 4_000.0,
        })
        .unwrap();

        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert!(
            (eq.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap_or(0.0) - 4_000.0).abs() < 1e-6,
            "IdealCapacity must scale electric-boiler thermal output to commanded value"
        );
    }

    #[test]
    fn test_supply_temp_glycol_vs_water() {
        const CAPACITY_W: f64 = 8_000.0;
        const FLOW_KG_S: f64 = 0.5;
        const WATER_CP: f64 = 4_180.0;
        const GLYCOL_CP: f64 = 3_800.0;

        let water_cfg = EquipmentConfig::from_typed(
            "EB-water".to_string(),
            "Electric Boiler".to_string(),
            ElectricBoilerConfig {
                zone_id: Some(1),
                loop_id: Some(1),
                eir: 1.0,
                capacity_w: CAPACITY_W,
                flow_rate_kg_s: FLOW_KG_S,
                fluid_type: FluidType::Water,
                ..ElectricBoilerConfig::default()
            },
        );
        let glycol_cfg = EquipmentConfig::from_typed(
            "EB-glycol".to_string(),
            "Electric Boiler".to_string(),
            ElectricBoilerConfig {
                zone_id: Some(1),
                loop_id: Some(1),
                eir: 1.0,
                capacity_w: CAPACITY_W,
                flow_rate_kg_s: FLOW_KG_S,
                fluid_type: FluidType::Glycol,
                ..ElectricBoilerConfig::default()
            },
        );

        let env = env(18.0);
        let mut water_eq = ElectricBoiler::new(water_cfg.clone());
        let mut glycol_eq = ElectricBoiler::new(glycol_cfg.clone());
        water_eq.init(&water_cfg, &env).unwrap();
        glycol_eq.init(&glycol_cfg, &env).unwrap();

        let mut water_ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..PortSlots::default()
        };
        let mut glycol_ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Glycol,
            )],
            ..PortSlots::default()
        };

        water_eq.update_control(&env);
        water_eq
            .step(&env, Duration::from_secs(60), &mut water_ports)
            .unwrap();
        glycol_eq.update_control(&env);
        glycol_eq
            .step(&env, Duration::from_secs(60), &mut glycol_ports)
            .unwrap();

        let water_supply = water_eq.telemetry().get(tk::SUPPLY_TEMP_C).unwrap();
        let glycol_supply = glycol_eq.telemetry().get(tk::SUPPLY_TEMP_C).unwrap();
        let return_temp = water_eq.telemetry().get(tk::RETURN_TEMP_C).unwrap();

        let dt_water = water_supply - return_temp;
        let dt_glycol = glycol_supply - return_temp;

        // ΔT_glycol / ΔT_water = cp_water / cp_glycol
        let expected_ratio = WATER_CP / GLYCOL_CP;
        let actual_ratio = dt_glycol / dt_water;
        assert!(
            (actual_ratio - expected_ratio).abs() < 1e-6,
            "glycol ΔT ratio {actual_ratio} != expected {expected_ratio}: water_dt={dt_water}, glycol_dt={dt_glycol}"
        );

        // Glycol ΔT must be strictly larger (lower cp → larger temp rise)
        assert!(
            dt_glycol > dt_water,
            "glycol ΔT ({dt_glycol}) must exceed water ΔT ({dt_water})"
        );
    }

    #[test]
    fn test_fluid_type_plumbed_to_cp() {
        const CAPACITY_W: f64 = 5_000.0;
        const EIR: f64 = 1.0;
        const WATER_CP: f64 = 4_180.0;
        const GLYCOL_CP: f64 = 3_800.0;

        // Step 1: configure with Water, verify cp_used is water cp
        let water_cfg = EquipmentConfig::from_typed(
            "EB-cp-test".to_string(),
            "Electric Boiler".to_string(),
            ElectricBoilerConfig {
                zone_id: Some(1),
                loop_id: Some(1),
                eir: EIR,
                capacity_w: CAPACITY_W,
                fluid_type: FluidType::Water,
                ..ElectricBoilerConfig::default()
            },
        );
        let env = env(18.0);
        let mut eq = ElectricBoiler::new(water_cfg.clone());
        eq.init(&water_cfg, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let cp_used = eq.telemetry().get(tk::BOILER_CP_USED_J_KG_K).unwrap();
        assert!(
            (cp_used - WATER_CP).abs() < 1e-6,
            "Water config: cp_used={cp_used}, expected water cp={WATER_CP}"
        );

        // Step 2: re-init with Glycol, verify cp_used changes
        let glycol_cfg = EquipmentConfig::from_typed(
            "EB-cp-test".to_string(),
            "Electric Boiler".to_string(),
            ElectricBoilerConfig {
                zone_id: Some(1),
                loop_id: Some(1),
                eir: EIR,
                capacity_w: CAPACITY_W,
                fluid_type: FluidType::Glycol,
                ..ElectricBoilerConfig::default()
            },
        );
        eq.init(&glycol_cfg, &env).unwrap();
        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Glycol,
            )],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let cp_used = eq.telemetry().get(tk::BOILER_CP_USED_J_KG_K).unwrap();
        assert!(
            (cp_used - GLYCOL_CP).abs() < 1e-6,
            "Glycol config: cp_used={cp_used}, expected glycol cp={GLYCOL_CP}"
        );

        // Step 3: re-init with Refrigerant, verify cp_used changes
        let refrig_cfg = EquipmentConfig::from_typed(
            "EB-cp-test".to_string(),
            "Electric Boiler".to_string(),
            ElectricBoilerConfig {
                zone_id: Some(1),
                loop_id: Some(1),
                eir: EIR,
                capacity_w: CAPACITY_W,
                fluid_type: FluidType::Refrigerant,
                ..ElectricBoilerConfig::default()
            },
        );
        eq.init(&refrig_cfg, &env).unwrap();
        let mut ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Refrigerant,
            )],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let cp_used = eq.telemetry().get(tk::BOILER_CP_USED_J_KG_K).unwrap();
        assert!(
            (cp_used - 1_450.0).abs() < 1e-6,
            "Refrigerant config: cp_used={cp_used}, expected refrigerant cp=1450"
        );
    }

    #[test]
    fn electric_boiler_can_accept_mode_override_and_demand_response() {
        let cfg = eb_config(8_000.0, 1.05);
        let eq = ElectricBoiler::new(cfg);
        use hares_control::capabilities::can_accept;
        assert!(can_accept(
            eq.descriptor().control_capabilities,
            &ControlSignal::ModeOverride {
                mode: OperatingMode::Off,
            }
        ));
        assert!(can_accept(
            eq.descriptor().control_capabilities,
            &ControlSignal::DemandResponse {
                level: DRLevel::High,
                duration_s: Some(3600.0),
            }
        ));
    }

    #[test]
    fn gas_boiler_can_accept_mode_override_and_demand_response() {
        let cfg = gb_config(10_000.0, 0.80);
        let eq = GasBoiler::new(cfg);
        use hares_control::capabilities::can_accept;
        assert!(can_accept(
            eq.descriptor().control_capabilities,
            &ControlSignal::ModeOverride {
                mode: OperatingMode::Off,
            }
        ));
        assert!(can_accept(
            eq.descriptor().control_capabilities,
            &ControlSignal::DemandResponse {
                level: DRLevel::Critical,
                duration_s: Some(1800.0),
            }
        ));
    }

    #[test]
    fn electric_boiler_mode_override_off_forces_off() {
        let cfg = eb_config(8_000.0, 1.05);
        let mut eq = ElectricBoiler::new(cfg.clone());
        let env = env(18.0);
        eq.init(&cfg, &env).unwrap();

        eq.apply_control_unchecked(&ControlSignal::ModeOverride {
            mode: OperatingMode::Off,
        })
        .unwrap();
        let mode = eq.update_control(&env);
        assert_eq!(mode, OperatingMode::Off);

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        assert!(
            ports.thermal[0].sensible_gain_w < 1e-9,
            "ModeOverride Off must prevent boiler heating"
        );
    }

    #[test]
    fn gas_boiler_mode_override_off_forces_off() {
        let cfg = gb_config(10_000.0, 0.80);
        let mut eq = GasBoiler::new(cfg.clone());
        let env = env(18.0);
        eq.init(&cfg, &env).unwrap();

        eq.apply_control_unchecked(&ControlSignal::ModeOverride {
            mode: OperatingMode::Off,
        })
        .unwrap();
        let mode = eq.update_control(&env);
        assert_eq!(mode, OperatingMode::Off);

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        assert!(
            ports.thermal[0].sensible_gain_w < 1e-9,
            "ModeOverride Off must prevent gas boiler heating"
        );
    }

    #[test]
    fn electric_boiler_demand_response_grid_emergency_forces_off() {
        let cfg = eb_config(8_000.0, 1.05);
        let mut eq = ElectricBoiler::new(cfg.clone());
        let env = env(18.0);
        eq.init(&cfg, &env).unwrap();

        eq.apply_control_unchecked(&ControlSignal::DemandResponse {
            level: DRLevel::GridEmergency,
            duration_s: Some(3600.0),
        })
        .unwrap();
        let mode = eq.update_control(&env);
        assert_eq!(mode, OperatingMode::Off);

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        assert!(
            ports.thermal[0].sensible_gain_w < 1e-9,
            "GridEmergency must prevent boiler heating"
        );
    }
}
