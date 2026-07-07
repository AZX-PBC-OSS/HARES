//! Gas and electric furnace models.

use std::borrow::Cow;
use std::time::Duration;

use chrono::{DateTime, FixedOffset};
use hares_types::{
    ControlCapabilities, ControlSignal, CoreCapabilities, CoreFlows, CoreOutput, CorePerformance,
    CoreState, DRLevel, ElectricPower, EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId,
    ExecutionStage, FuelPower, FuelType, HaresError, OperatingMode, PortContribution,
    PortDeclaration, PortSlots, Telemetry, TelemetryField, ThermalCategory, ZoneId,
};
use serde::{Deserialize, Serialize};

use hares_types::telemetry_keys as tk;

use hares_physics::units::{power_kw_to_w, power_w_to_kw};

use crate::hvac::heating_config::{ElectricFurnaceConfig, GasFurnaceConfig};
use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_versioned, try_save_versioned};

use super::{
    HvacEquipment, HvacEquipmentType, RuntimeSetpointOverride, ThermostatMode,
    helpers::{
        apply_heating_control_unchecked, apply_simple_heating_ideal_capacity_control,
        apply_simple_mode_override_and_dr, apply_simple_mode_override_in_control,
        equipment_id_from_config, update_heating_control, zone_id_from_config_or_default,
    },
};

/// Default gas furnace AFUE. DOE 10 CFR Part 430, federal minimum.
const DEFAULT_GAS_AFUE: f64 = 0.8;

pub struct ElectricFurnace {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,
    core_output: CoreOutput,
    hvac: HvacEquipment,
    rated_capacity_w: f64,
    eir: f64,
    fan_power_w: f64,
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
    /// Rule R1 reactive-only ZIP for the resistance element (class default
    /// pf 1.0 → Q exactly zero, or a user `"zip"` override). Real power
    /// stays bit-identical.
    zip: hares_types::zip::ZipLoad,
    /// Blower fan motor component ZIP (pf 0.87), derived at init via
    /// `hvac::reactive::secondary_motor_zip`.
    fan_zip: hares_types::zip::ZipLoad,
}

pub struct GasFurnace {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,
    core_output: CoreOutput,
    hvac: HvacEquipment,
    rated_capacity_w: f64,
    fuel_efficiency: f64,
    fan_power_w: f64,
    fuel_type: FuelType,
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
    /// Rule R1 reactive-only ZIP: blower fan motor pf 0.87;
    /// Q comes from ZipLoad::reactive_kvar.
    zip: hares_types::zip::ZipLoad,
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
    speed_index: f64,
    runtime_fraction: f64,
    main_power_kw: f64,
    duct_loss_w: f64,
    mode_override: Option<OperatingMode>,
    dr_level: DRLevel,
}

impl ElectricFurnace {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let (zone, zone_id_explicit) = zone_id_from_config_or_default(&config, &config.name);
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
                | ControlCapabilities::IDEAL_CAPACITY
                | ControlCapabilities::MODE_OVERRIDE
                | ControlCapabilities::DEMAND_RESPONSE,
            core_capabilities: CoreCapabilities::ELECTRIC
                | CoreCapabilities::REACTIVE
                | CoreCapabilities::HAS_MODE
                | CoreCapabilities::THERMAL
                | CoreCapabilities::HAS_SETPOINT,
            telemetry_fields: electric_furnace_telemetry_fields(),
            zone_type: None,
        };

        Self {
            descriptor,
            ports: vec![
                PortDeclaration::electrical(),
                PortDeclaration::thermal(zone),
            ],
            telemetry: electric_furnace_default_telemetry(),
            core_output: CoreOutput::default(),
            hvac: HvacEquipment::new(HvacEquipmentType::ElectricFurnace, zone),
            rated_capacity_w: 0.0,
            eir: 1.0,
            fan_power_w: 0.0,
            operating_mode: OperatingMode::Off,
            run_time_s: 0.0,
            use_ideal: false,
            zone_id_explicit,
            mode_override: None,
            dr_level: DRLevel::Normal,
            zip: hares_types::zip::ZipLoad::constant_power(),
            fan_zip: hares_types::zip::ZipLoad::constant_power(),
        }
    }
}

impl Equipment for ElectricFurnace {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn rename(&mut self, name: String) {
        self.descriptor.name = name;
    }

    fn zone_id_explicit(&self) -> bool {
        self.zone_id_explicit
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        self.hvac.init(config, env)?;
        self.zip = crate::config::resolve_reactive_zip(config)?;
        self.fan_zip =
            super::reactive::secondary_motor_zip(&self.zip, super::reactive::FAN_MOTOR_ZIP);
        let typed = config.require_typed::<ElectricFurnaceConfig>("Electric Furnace")?;
        self.rated_capacity_w = typed.capacity_w.max(0.0);
        self.eir = typed.eir;
        let airflow_m3_s = self.hvac.config.airflow_m3_s_per_w * self.rated_capacity_w;
        self.fan_power_w = typed
            .fan_power_w
            .unwrap_or_else(|| self.hvac.fan_power_w(airflow_m3_s));
        self.hvac.config.duct_dse = typed.ducts.dse_heat.unwrap_or(1.0).clamp(0.0, 1.0);
        self.hvac.config.duct_zone_id = typed.ducts.duct_zone_id.map(ZoneId);
        if self.eir <= 0.0 || !self.eir.is_finite() {
            return Err(HaresError::Equipment(format!(
                "invalid Electric Furnace eir: {}",
                self.eir
            )));
        }
        self.hvac.update_zone_heat_fractions();
        self.hvac.rebuild_thermal_ports(&mut self.ports, false);
        self.hvac.config.heating_capacities_w = vec![self.rated_capacity_w];
        self.hvac.config.eir_by_stage = vec![self.eir];
        self.hvac.runtime.startup.c_d = 0.0;
        self.operating_mode = OperatingMode::Off;
        self.run_time_s = 0.0;
        self.telemetry = electric_furnace_default_telemetry();
        self.core_output = CoreOutput::default();
        Ok(())
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        self.use_ideal = self.hvac.use_ideal_capacity(env);
        if let Some(mode) = apply_simple_mode_override_in_control(
            &mut self.hvac,
            &mut self.mode_override,
            self.dr_level,
            "Electric Furnace",
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
        let gross_capacity_w = self.rated_capacity_w * duty * sf;
        let fan_kw = power_w_to_kw(self.fan_power_w * duty) * sf;
        // Heating element power + fan power
        let element_kw = power_w_to_kw(gross_capacity_w * self.eir);
        let electric_kw = element_kw + fan_kw;

        // Per-component reactive (see `hvac::reactive`): the resistance
        // element at the unit ZIP (pf 1.0 → Q ≡ 0), the blower fan motor at
        // pf 0.87.
        let reactive_power_kvar = self.zip.reactive_kvar(element_kw, env.grid.voltage_pu)
            + self.fan_zip.reactive_kvar(fan_kw, env.grid.voltage_pu);
        if electric_kw > 0.0 {
            ports.accumulate(&PortContribution::Electrical {
                active_power_w: power_kw_to_w(electric_kw),
                reactive_power_kvar,
            })?;
        }

        // Fan waste heat contributes to zone sensible gain (OCHRE HVAC.py line 543).
        let fan_heat_w = power_kw_to_w(fan_kw);
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

        let thermal_output_w = total_sensible_w * self.hvac.config.duct_dse.clamp(0.0, 1.0);
        // ASHRAE 152: duct_loss = gross_capacity * (1 - dse).
        let duct_loss_w = gross_capacity_w * (1.0 - self.hvac.config.duct_dse.clamp(0.0, 1.0));
        // OCHRE HVAC.py:575: main_power = total_input - fan.
        // Electric furnace: main = heating elements = electric_kw - fan_kw.
        let main_power_kw = (electric_kw - fan_kw).max(0.0);
        let rtf = if self.operating_mode != OperatingMode::Off {
            self.hvac.runtime.duty_cycle.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let sp = self.hvac.effective_setpoints();
        self.telemetry.set(tk::FAN_KW, fan_kw);
        self.telemetry.set(tk::ELECTRIC_KW, electric_kw);
        self.telemetry
            .set(tk::REACTIVE_POWER_KVAR, reactive_power_kvar);
        self.telemetry.set(tk::THERMAL_OUTPUT_W, thermal_output_w);
        self.telemetry
            .set(tk::OPERATING_MODE, self.operating_mode.as_code());
        self.telemetry
            .set(tk::SUPPLY_AIR_TEMP_C, self.hvac.config.supply_air_temp_c);
        self.telemetry.set(tk::HEATING_SETPOINT_C, sp.heating_c);
        self.telemetry.set(tk::COOLING_SETPOINT_C, sp.cooling_c);
        let schedule_stage = self
            .hvac
            .thermostat_fsm
            .static_setpoints
            .with_schedule_override(self.hvac.thermostat_fsm.schedule_setpoints);
        self.telemetry
            .set(tk::SCHEDULE_HEATING_SETPOINT_C, schedule_stage.heating_c);
        self.telemetry
            .set(tk::SCHEDULE_COOLING_SETPOINT_C, schedule_stage.cooling_c);
        if let Some(ref rt) = self.hvac.thermostat_fsm.runtime_setpoints {
            self.telemetry
                .set(tk::RUNTIME_HEATING_SETPOINT_C, rt.heating_c.unwrap_or(0.0));
            self.telemetry
                .set(tk::RUNTIME_COOLING_SETPOINT_C, rt.cooling_c.unwrap_or(0.0));
        }
        self.telemetry.set(tk::RUNTIME_FRACTION, rtf);
        self.telemetry.set(tk::MAIN_POWER_KW, main_power_kw);
        self.telemetry.set(tk::DUCT_LOSS_W, duct_loss_w);
        self.telemetry
            .set(tk::SPEED_INDEX, self.hvac.runtime.last_speed_index as f64);
        self.telemetry
            .set(tk::SPEED_FRAC, self.hvac.runtime.last_speed_frac);
        self.telemetry
            .set(tk::PART_LOAD_RATIO, self.hvac.runtime.duty_cycle);
        self.telemetry
            .set(tk::PART_LOAD_FACTOR, self.hvac.runtime.plf_state);
        self.telemetry.set(
            tk::STARTUP_MULTIPLIER,
            self.hvac.runtime.startup.current_multiplier(),
        );
        self.telemetry
            .set(tk::DUTY_CYCLE, self.hvac.runtime.duty_cycle);
        self.telemetry.set(
            tk::TIME_AT_CURRENT_SPEED_S,
            self.hvac.runtime.time_at_current_speed_s,
        );
        let mode_duration_s = (env.current_time
            - self
                .hvac
                .thermostat_fsm
                .mode_start_at
                .unwrap_or(env.current_time))
        .num_milliseconds()
        .max(0) as f64
            / 1000.0;
        self.telemetry.set(tk::MODE_DURATION_S, mode_duration_s);
        // Heating-only equipment: setpoint_c is always the heating setpoint (per
        // validate_core_contract requirement that HAS_SETPOINT => setpoint_c is Some).
        let active_setpoint_c = sp.heating_c;
        self.core_output = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(electric_kw.max(0.0))),
                reactive_power_kvar: Some(reactive_power_kvar),
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
            performance: CorePerformance {
                cop: None,
                main_power_kw: Some(main_power_kw),
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

    fn save_state(&self) -> crate::Result<Vec<u8>> {
        try_save_versioned(
            &FurnaceState {
                mode: self.hvac.thermostat_fsm.mode,
                duty_cycle: self.hvac.runtime.duty_cycle,
                last_mode_switch_at: self.hvac.thermostat_fsm.last_mode_switch_at,
                runtime_setpoints: self.hvac.thermostat_fsm.runtime_setpoints,
                operating_mode: self.operating_mode,
                run_time_s: self.run_time_s,
                electric_kw: self.telemetry.get(tk::ELECTRIC_KW).unwrap_or(0.0),
                thermal_output_w: self.telemetry.get(tk::THERMAL_OUTPUT_W).unwrap_or(0.0),
                fuel_input_w: 0.0,
                speed_index: self.telemetry.get(tk::SPEED_INDEX).unwrap_or(0.0),
                runtime_fraction: self.telemetry.get(tk::RUNTIME_FRACTION).unwrap_or(0.0),
                main_power_kw: self.telemetry.get(tk::MAIN_POWER_KW).unwrap_or(0.0),
                duct_loss_w: self.telemetry.get(tk::DUCT_LOSS_W).unwrap_or(0.0),
                mode_override: self.mode_override,
                dr_level: self.dr_level,
            },
            Self::checkpoint_version(),
            "ElectricFurnace",
        )
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let decoded: FurnaceState = load_versioned(
            state,
            Self::checkpoint_version(),
            "ElectricFurnace",
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
            .insert(tk::OPERATING_MODE, decoded.operating_mode.as_code());
        self.telemetry
            .insert(tk::SUPPLY_AIR_TEMP_C, self.hvac.config.supply_air_temp_c);
        self.telemetry.insert(tk::SPEED_INDEX, decoded.speed_index);
        self.telemetry
            .insert(tk::RUNTIME_FRACTION, decoded.runtime_fraction);
        self.telemetry
            .insert(tk::MAIN_POWER_KW, decoded.main_power_kw);
        self.telemetry.insert(tk::DUCT_LOSS_W, decoded.duct_loss_w);
        self.core_output = CoreOutput::default();
        Ok(())
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        if apply_simple_mode_override_and_dr(
            &mut self.mode_override,
            &mut self.dr_level,
            signal,
            "Electric Furnace",
        )? {
            return Ok(());
        }
        apply_heating_control_unchecked(&mut self.hvac, signal, "Electric Furnace")?;
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

impl GasFurnace {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let (zone, zone_id_explicit) = zone_id_from_config_or_default(&config, &config.name);
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
                | ControlCapabilities::IDEAL_CAPACITY
                | ControlCapabilities::MODE_OVERRIDE
                | ControlCapabilities::DEMAND_RESPONSE,
            core_capabilities: CoreCapabilities::ELECTRIC
                | CoreCapabilities::FUEL
                | CoreCapabilities::REACTIVE
                | CoreCapabilities::HAS_MODE
                | CoreCapabilities::THERMAL
                | CoreCapabilities::HAS_SPEED
                | CoreCapabilities::HAS_SETPOINT,
            telemetry_fields: gas_furnace_telemetry_fields(),
            zone_type: None,
        };

        Self {
            descriptor,
            ports: vec![
                PortDeclaration::fuel(),
                PortDeclaration::electrical(),
                PortDeclaration::thermal(zone),
            ],
            telemetry: gas_furnace_default_telemetry(),
            core_output: CoreOutput::default(),
            hvac: HvacEquipment::new(HvacEquipmentType::GasFurnace, zone),
            rated_capacity_w: 0.0,
            fuel_efficiency: DEFAULT_GAS_AFUE,
            fan_power_w: 0.0,
            fuel_type: FuelType::Gas,
            operating_mode: OperatingMode::Off,
            run_time_s: 0.0,
            use_ideal: false,
            zone_id_explicit,
            mode_override: None,
            dr_level: DRLevel::Normal,
            zip: hares_types::zip::ZipLoad::constant_power(),
        }
    }
}

impl Equipment for GasFurnace {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn rename(&mut self, name: String) {
        self.descriptor.name = name;
    }

    fn zone_id_explicit(&self) -> bool {
        self.zone_id_explicit
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        self.hvac.init(config, env)?;
        self.zip = crate::config::resolve_reactive_zip(config)?;
        let typed = config.require_typed::<GasFurnaceConfig>("Gas Furnace")?;
        typed.validate()?;
        self.rated_capacity_w = typed.capacity_w.max(0.0);
        self.fuel_efficiency = typed.afue;
        let airflow_m3_s = self.rated_capacity_w * self.hvac.config.airflow_m3_s_per_w;
        self.fan_power_w = typed
            .fan_power_w
            .unwrap_or_else(|| self.hvac.fan_power_w(airflow_m3_s));
        self.hvac.config.duct_dse = typed.ducts.dse_heat.unwrap_or(1.0).clamp(0.0, 1.0);
        self.hvac.config.duct_zone_id = typed.ducts.duct_zone_id.map(ZoneId);
        self.hvac.update_zone_heat_fractions();
        self.hvac.rebuild_thermal_ports(&mut self.ports, false);
        self.hvac.config.heating_capacities_w =
            if let Some(stages) = &typed.stage_heating_capacities_w {
                stages.clone()
            } else {
                vec![self.rated_capacity_w]
            };
        let default_eir = 1.0 / self.fuel_efficiency;
        let stage_count = self.hvac.config.heating_capacities_w.len();
        self.hvac.config.eir_by_stage = if let Some(stages) = &typed.stage_heating_eirs {
            stages.clone()
        } else {
            vec![default_eir; stage_count]
        };
        if self.hvac.config.heating_capacities_w.len() != self.hvac.config.eir_by_stage.len() {
            return Err(HaresError::Equipment(
                "heating capacity and EIR stage counts must match".to_string(),
            ));
        }
        self.hvac.runtime.startup.c_d = 0.0;
        self.operating_mode = OperatingMode::Off;
        self.run_time_s = 0.0;
        self.telemetry = gas_furnace_default_telemetry();
        self.core_output = CoreOutput::default();
        Ok(())
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        self.use_ideal = self.hvac.use_ideal_capacity(env);
        if let Some(mode) = apply_simple_mode_override_in_control(
            &mut self.hvac,
            &mut self.mode_override,
            self.dr_level,
            "Gas Furnace",
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
        // Fuel is computed from gross capacity: the furnace burns fuel regardless
        // of duct losses. zone_heat_fractions distributes gross output by DSE.
        let gross_capacity_w = self.rated_capacity_w * duty * sf;
        let fan_kw = power_w_to_kw(self.fan_power_w * duty) * sf;
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

        let reactive_power_kvar = self.zip.reactive_kvar(fan_kw, env.grid.voltage_pu);
        if fan_kw > 0.0 {
            ports.accumulate(&PortContribution::Electrical {
                active_power_w: power_kw_to_w(fan_kw),
                reactive_power_kvar,
            })?;
        }

        // Fan waste heat contributes to zone sensible gain (OCHRE HVAC.py line 543).
        let fan_heat_w = power_kw_to_w(fan_kw);
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
        let thermal_output_w = total_sensible_w * self.hvac.config.duct_dse.clamp(0.0, 1.0);
        // ASHRAE 152: duct_loss = gross_capacity * (1 - dse).
        let duct_loss_w = gross_capacity_w * (1.0 - self.hvac.config.duct_dse.clamp(0.0, 1.0));
        // OCHRE HVAC.py:575: main_power = total_input_kw - fan_kw.
        // Gas furnace: main = gas input converted to kW. Fan is separate (electric).
        // OCHRE uses gas_therms_per_hour / kwh_to_therms; HARES uses fuel_input_w / 1000.
        let main_power_kw = power_w_to_kw(fuel_input_w);
        let rtf = if self.operating_mode != OperatingMode::Off {
            self.hvac.runtime.duty_cycle.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let sp = self.hvac.effective_setpoints();
        self.telemetry.set(tk::FAN_KW, fan_kw);
        self.telemetry.set(tk::ELECTRIC_KW, fan_kw);
        self.telemetry
            .set(tk::REACTIVE_POWER_KVAR, reactive_power_kvar);
        self.telemetry.set(tk::FUEL_INPUT_W, fuel_input_w);
        self.telemetry.set(tk::THERMAL_OUTPUT_W, thermal_output_w);
        self.telemetry
            .set(tk::OPERATING_MODE, self.operating_mode.as_code());
        self.telemetry
            .set(tk::SUPPLY_AIR_TEMP_C, self.hvac.config.supply_air_temp_c);
        self.telemetry.set(tk::HEATING_SETPOINT_C, sp.heating_c);
        self.telemetry.set(tk::COOLING_SETPOINT_C, sp.cooling_c);
        let schedule_stage = self
            .hvac
            .thermostat_fsm
            .static_setpoints
            .with_schedule_override(self.hvac.thermostat_fsm.schedule_setpoints);
        self.telemetry
            .set(tk::SCHEDULE_HEATING_SETPOINT_C, schedule_stage.heating_c);
        self.telemetry
            .set(tk::SCHEDULE_COOLING_SETPOINT_C, schedule_stage.cooling_c);
        if let Some(ref rt) = self.hvac.thermostat_fsm.runtime_setpoints {
            self.telemetry
                .set(tk::RUNTIME_HEATING_SETPOINT_C, rt.heating_c.unwrap_or(0.0));
            self.telemetry
                .set(tk::RUNTIME_COOLING_SETPOINT_C, rt.cooling_c.unwrap_or(0.0));
        }
        self.telemetry
            .set(tk::SPEED_INDEX, self.hvac.runtime.last_speed_index as f64);
        self.telemetry.set(tk::RUNTIME_FRACTION, rtf);
        self.telemetry.set(tk::MAIN_POWER_KW, main_power_kw);
        self.telemetry.set(tk::DUCT_LOSS_W, duct_loss_w);
        self.telemetry
            .set(tk::SPEED_FRAC, self.hvac.runtime.last_speed_frac);
        self.telemetry
            .set(tk::PART_LOAD_RATIO, self.hvac.runtime.duty_cycle);
        self.telemetry
            .set(tk::PART_LOAD_FACTOR, self.hvac.runtime.plf_state);
        self.telemetry.set(
            tk::STARTUP_MULTIPLIER,
            self.hvac.runtime.startup.current_multiplier(),
        );
        self.telemetry
            .set(tk::DUTY_CYCLE, self.hvac.runtime.duty_cycle);
        self.telemetry.set(
            tk::TIME_AT_CURRENT_SPEED_S,
            self.hvac.runtime.time_at_current_speed_s,
        );
        let mode_duration_s = (env.current_time
            - self
                .hvac
                .thermostat_fsm
                .mode_start_at
                .unwrap_or(env.current_time))
        .num_milliseconds()
        .max(0) as f64
            / 1000.0;
        self.telemetry.set(tk::MODE_DURATION_S, mode_duration_s);
        // Heating-only equipment: setpoint_c is always the heating setpoint (per
        // validate_core_contract requirement that HAS_SETPOINT => setpoint_c is Some).
        let active_setpoint_c = sp.heating_c;
        self.core_output = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(fan_kw.max(0.0))),
                reactive_power_kvar: Some(reactive_power_kvar),
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
                speed_index: Some(self.hvac.runtime.last_speed_index as u8),
                setpoint_c: Some(active_setpoint_c),
            },
            performance: CorePerformance {
                cop: None,
                main_power_kw: Some(main_power_kw),
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

    fn save_state(&self) -> crate::Result<Vec<u8>> {
        try_save_versioned(
            &FurnaceState {
                mode: self.hvac.thermostat_fsm.mode,
                duty_cycle: self.hvac.runtime.duty_cycle,
                last_mode_switch_at: self.hvac.thermostat_fsm.last_mode_switch_at,
                runtime_setpoints: self.hvac.thermostat_fsm.runtime_setpoints,
                operating_mode: self.operating_mode,
                run_time_s: self.run_time_s,
                electric_kw: self.telemetry.get(tk::ELECTRIC_KW).unwrap_or(0.0),
                thermal_output_w: self.telemetry.get(tk::THERMAL_OUTPUT_W).unwrap_or(0.0),
                fuel_input_w: self.telemetry.get(tk::FUEL_INPUT_W).unwrap_or(0.0),
                speed_index: self.telemetry.get(tk::SPEED_INDEX).unwrap_or(0.0),
                runtime_fraction: self.telemetry.get(tk::RUNTIME_FRACTION).unwrap_or(0.0),
                main_power_kw: self.telemetry.get(tk::MAIN_POWER_KW).unwrap_or(0.0),
                duct_loss_w: self.telemetry.get(tk::DUCT_LOSS_W).unwrap_or(0.0),
                mode_override: self.mode_override,
                dr_level: self.dr_level,
            },
            Self::checkpoint_version(),
            "GasFurnace",
        )
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let decoded: FurnaceState = load_versioned(
            state,
            Self::checkpoint_version(),
            "GasFurnace",
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
        self.hvac.runtime.last_speed_index = decoded.speed_index as usize;

        self.telemetry.insert(tk::FAN_KW, decoded.electric_kw);
        self.telemetry.insert(tk::ELECTRIC_KW, decoded.electric_kw);
        self.telemetry
            .insert(tk::FUEL_INPUT_W, decoded.fuel_input_w);
        self.telemetry
            .insert(tk::THERMAL_OUTPUT_W, decoded.thermal_output_w);
        self.telemetry
            .insert(tk::OPERATING_MODE, decoded.operating_mode.as_code());
        self.telemetry
            .insert(tk::SUPPLY_AIR_TEMP_C, self.hvac.config.supply_air_temp_c);
        self.telemetry.insert(tk::SPEED_INDEX, decoded.speed_index);
        self.telemetry
            .insert(tk::RUNTIME_FRACTION, decoded.runtime_fraction);
        self.telemetry
            .insert(tk::MAIN_POWER_KW, decoded.main_power_kw);
        self.telemetry.insert(tk::DUCT_LOSS_W, decoded.duct_loss_w);
        self.core_output = CoreOutput::default();
        Ok(())
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        if apply_simple_mode_override_and_dr(
            &mut self.mode_override,
            &mut self.dr_level,
            signal,
            "Gas Furnace",
        )? {
            return Ok(());
        }
        apply_heating_control_unchecked(&mut self.hvac, signal, "Gas Furnace")?;
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
        "Electric Furnace",
        Box::new(|config| Box::new(ElectricFurnace::new(config))),
    );
    registry.register(
        "Gas Furnace",
        Box::new(|config| Box::new(GasFurnace::new(config))),
    );
}

fn electric_furnace_default_telemetry() -> Telemetry {
    let mut telemetry = Telemetry::with_capacity(23);
    telemetry.insert(tk::FAN_KW, 0.0);
    telemetry.insert(tk::ELECTRIC_KW, 0.0);
    telemetry.insert(tk::REACTIVE_POWER_KVAR, 0.0);
    telemetry.insert(tk::THERMAL_OUTPUT_W, 0.0);
    telemetry.insert(tk::OPERATING_MODE, 0.0);
    telemetry.insert(tk::SUPPLY_AIR_TEMP_C, 0.0);
    telemetry.insert(tk::HEATING_SETPOINT_C, 0.0);
    telemetry.insert(tk::COOLING_SETPOINT_C, 0.0);
    telemetry.insert(tk::SCHEDULE_HEATING_SETPOINT_C, 0.0);
    telemetry.insert(tk::SCHEDULE_COOLING_SETPOINT_C, 0.0);
    telemetry.insert(tk::RUNTIME_HEATING_SETPOINT_C, 0.0);
    telemetry.insert(tk::RUNTIME_COOLING_SETPOINT_C, 0.0);
    telemetry.insert(tk::RUNTIME_FRACTION, 0.0);
    telemetry.insert(tk::MAIN_POWER_KW, 0.0);
    telemetry.insert(tk::DUCT_LOSS_W, 0.0);
    telemetry.insert(tk::SPEED_INDEX, 0.0);
    telemetry.insert(tk::SPEED_FRAC, 0.0);
    telemetry.insert(tk::PART_LOAD_RATIO, 0.0);
    telemetry.insert(tk::PART_LOAD_FACTOR, 0.0);
    telemetry.insert(tk::STARTUP_MULTIPLIER, 0.0);
    telemetry.insert(tk::DUTY_CYCLE, 0.0);
    telemetry.insert(tk::TIME_AT_CURRENT_SPEED_S, 0.0);
    telemetry.insert(tk::MODE_DURATION_S, 0.0);
    telemetry
}

fn gas_furnace_default_telemetry() -> Telemetry {
    let mut telemetry = Telemetry::with_capacity(25);
    telemetry.insert(tk::FAN_KW, 0.0);
    telemetry.insert(tk::ELECTRIC_KW, 0.0);
    telemetry.insert(tk::REACTIVE_POWER_KVAR, 0.0);
    telemetry.insert(tk::FUEL_INPUT_W, 0.0);
    telemetry.insert(tk::THERMAL_OUTPUT_W, 0.0);
    telemetry.insert(tk::OPERATING_MODE, 0.0);
    telemetry.insert(tk::SUPPLY_AIR_TEMP_C, 0.0);
    telemetry.insert(tk::HEATING_SETPOINT_C, 0.0);
    telemetry.insert(tk::COOLING_SETPOINT_C, 0.0);
    telemetry.insert(tk::SCHEDULE_HEATING_SETPOINT_C, 0.0);
    telemetry.insert(tk::SCHEDULE_COOLING_SETPOINT_C, 0.0);
    telemetry.insert(tk::RUNTIME_HEATING_SETPOINT_C, 0.0);
    telemetry.insert(tk::RUNTIME_COOLING_SETPOINT_C, 0.0);
    telemetry.insert(tk::SPEED_INDEX, 0.0);
    telemetry.insert(tk::RUNTIME_FRACTION, 0.0);
    telemetry.insert(tk::MAIN_POWER_KW, 0.0);
    telemetry.insert(tk::DUCT_LOSS_W, 0.0);
    telemetry.insert(tk::SPEED_FRAC, 0.0);
    telemetry.insert(tk::PART_LOAD_RATIO, 0.0);
    telemetry.insert(tk::PART_LOAD_FACTOR, 0.0);
    telemetry.insert(tk::STARTUP_MULTIPLIER, 0.0);
    telemetry.insert(tk::DUTY_CYCLE, 0.0);
    telemetry.insert(tk::TIME_AT_CURRENT_SPEED_S, 0.0);
    telemetry.insert(tk::MODE_DURATION_S, 0.0);
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
        TelemetryField {
            name: tk::SCHEDULE_HEATING_SETPOINT_C.to_string(),
            unit: "C".to_string(),
            description: "Schedule-stage heating setpoint (before runtime override)".to_string(),
        },
        TelemetryField {
            name: tk::SCHEDULE_COOLING_SETPOINT_C.to_string(),
            unit: "C".to_string(),
            description: "Schedule-stage cooling setpoint (before runtime override)".to_string(),
        },
        TelemetryField {
            name: tk::RUNTIME_HEATING_SETPOINT_C.to_string(),
            unit: "C".to_string(),
            description: "Runtime override heating setpoint (0.0 when no override active)"
                .to_string(),
        },
        TelemetryField {
            name: tk::RUNTIME_COOLING_SETPOINT_C.to_string(),
            unit: "C".to_string(),
            description: "Runtime override cooling setpoint (0.0 when no override active)"
                .to_string(),
        },
    ]
}

fn electric_furnace_telemetry_fields() -> Vec<TelemetryField> {
    let mut fields = vec![
        TelemetryField {
            name: tk::FAN_KW.to_string(),
            unit: "kW".to_string(),
            description: "Electric furnace fan electric power".to_string(),
        },
        TelemetryField {
            name: tk::ELECTRIC_KW.to_string(),
            unit: "kW".to_string(),
            description: "Electric furnace active power draw".to_string(),
        },
        TelemetryField {
            name: tk::REACTIVE_POWER_KVAR.to_string(),
            unit: "kVAR".to_string(),
            description: "Reactive power (positive = inductive/lagging)".to_string(),
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
        TelemetryField {
            name: tk::RUNTIME_FRACTION.to_string(),
            unit: "-".to_string(),
            description: "Runtime fraction (duty cycle / PLR) [0..1]".to_string(),
        },
        TelemetryField {
            name: tk::MAIN_POWER_KW.to_string(),
            unit: "kW".to_string(),
            description: "Main power (total_input - fan) per OCHRE HVAC.py:575".to_string(),
        },
        TelemetryField {
            name: tk::DUCT_LOSS_W.to_string(),
            unit: "W".to_string(),
            description: "Duct distribution losses per ASHRAE 152: gross_capacity * (1 - dse)"
                .to_string(),
        },
        TelemetryField {
            name: tk::SPEED_INDEX.to_string(),
            unit: "-".to_string(),
            description: "Active heating speed stage index (0-based)".to_string(),
        },
        TelemetryField {
            name: tk::SPEED_FRAC.to_string(),
            unit: "-".to_string(),
            description: "Interpolation weight between speed stages [0..1]".to_string(),
        },
        TelemetryField {
            name: tk::PART_LOAD_RATIO.to_string(),
            unit: "-".to_string(),
            description: "Heating load fraction this timestep [0..1]".to_string(),
        },
        TelemetryField {
            name: tk::PART_LOAD_FACTOR.to_string(),
            unit: "-".to_string(),
            description: "Part-load factor for cycling degradation (PLF)".to_string(),
        },
        TelemetryField {
            name: tk::STARTUP_MULTIPLIER.to_string(),
            unit: "-".to_string(),
            description: "Capacity ramp multiplier on startup (1.0 for furnace)".to_string(),
        },
        TelemetryField {
            name: tk::DUTY_CYCLE.to_string(),
            unit: "-".to_string(),
            description: "Thermostat on/off fraction this timestep [0..1]".to_string(),
        },
        TelemetryField {
            name: tk::TIME_AT_CURRENT_SPEED_S.to_string(),
            unit: "s".to_string(),
            description: "Seconds since the last speed-stage change".to_string(),
        },
        TelemetryField {
            name: tk::MODE_DURATION_S.to_string(),
            unit: "s".to_string(),
            description: "Seconds since the last thermostat mode change".to_string(),
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
            name: tk::REACTIVE_POWER_KVAR.to_string(),
            unit: "kVAR".to_string(),
            description: "Reactive power (positive = inductive/lagging)".to_string(),
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
        TelemetryField {
            name: tk::SPEED_INDEX.to_string(),
            unit: "-".to_string(),
            description: "Active heating speed stage index (0-based)".to_string(),
        },
        TelemetryField {
            name: tk::RUNTIME_FRACTION.to_string(),
            unit: "-".to_string(),
            description: "Runtime fraction (duty cycle / PLR) [0..1]".to_string(),
        },
        TelemetryField {
            name: tk::MAIN_POWER_KW.to_string(),
            unit: "kW".to_string(),
            description: "Main power (gas input in kW) per OCHRE HVAC.py:575".to_string(),
        },
        TelemetryField {
            name: tk::DUCT_LOSS_W.to_string(),
            unit: "W".to_string(),
            description: "Duct distribution losses per ASHRAE 152: gross_capacity * (1 - dse)"
                .to_string(),
        },
        TelemetryField {
            name: tk::SPEED_FRAC.to_string(),
            unit: "-".to_string(),
            description: "Interpolation weight between speed stages [0..1]".to_string(),
        },
        TelemetryField {
            name: tk::PART_LOAD_RATIO.to_string(),
            unit: "-".to_string(),
            description: "Heating load fraction this timestep [0..1]".to_string(),
        },
        TelemetryField {
            name: tk::PART_LOAD_FACTOR.to_string(),
            unit: "-".to_string(),
            description: "Part-load factor for cycling degradation (PLF)".to_string(),
        },
        TelemetryField {
            name: tk::STARTUP_MULTIPLIER.to_string(),
            unit: "-".to_string(),
            description: "Capacity ramp multiplier on startup (1.0 for furnace)".to_string(),
        },
        TelemetryField {
            name: tk::DUTY_CYCLE.to_string(),
            unit: "-".to_string(),
            description: "Thermostat on/off fraction this timestep [0..1]".to_string(),
        },
        TelemetryField {
            name: tk::TIME_AT_CURRENT_SPEED_S.to_string(),
            unit: "s".to_string(),
            description: "Seconds since the last speed-stage change".to_string(),
        },
        TelemetryField {
            name: tk::MODE_DURATION_S.to_string(),
            unit: "s".to_string(),
            description: "Seconds since the last thermostat mode change".to_string(),
        },
    ];
    fields.extend(setpoint_telemetry_fields());
    fields
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_physics::constants::W_PER_TON;
    use hares_types::{
        ControlSignal, CoreCapabilities, DRLevel, EnvironmentState, ExecutionStage, GridState,
        OperatingMode, PortSlots, ThermalAccumulator, WeatherState, ZoneId, ZoneState,
        telemetry_keys as tk,
    };

    use super::{ElectricFurnace, GasFurnace};

    use crate::hvac::heating_config::{DuctConfig, ElectricFurnaceConfig, GasFurnaceConfig};
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

    fn ef_config(capacity_w: f64, eir: f64) -> EquipmentConfig {
        EquipmentConfig::from_typed(
            "EF".to_string(),
            "Electric Furnace".to_string(),
            ElectricFurnaceConfig {
                eir,
                capacity_w,
                fan_power_w: Some(0.0),
                zone_id: Some(1),
                ..ElectricFurnaceConfig::default()
            },
        )
        .unwrap()
    }

    fn gf_config(capacity_w: f64, afue: f64) -> EquipmentConfig {
        EquipmentConfig::from_typed(
            "GF".to_string(),
            "Gas Furnace".to_string(),
            GasFurnaceConfig {
                afue,
                capacity_w,
                fan_power_w: Some(0.0),
                zone_id: Some(1),
                ..GasFurnaceConfig::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn electric_furnace_power_matches_capacity_times_eir() {
        let cfg = ef_config(8_000.0, 1.05);
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
        assert!((ports.electrical.net_active_w() - 8400.0).abs() < 1.0);
        assert!((ports.thermal[0].sensible_gain_w - 8_000.0).abs() < 1e-9);

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
        const AFUE: f64 = 0.8;
        const FAN_POWER_W: f64 = 400.0;
        const EXPECTED_FUEL_INPUT_W: f64 = RATED_CAPACITY_W / AFUE;
        const EXPECTED_FAN_W: f64 = FAN_POWER_W;
        const EXPECTED_SENSIBLE_GAIN_W: f64 = RATED_CAPACITY_W + FAN_POWER_W;

        let cfg = EquipmentConfig::from_typed(
            "GF".to_string(),
            "Gas Furnace".to_string(),
            GasFurnaceConfig {
                afue: AFUE,
                capacity_w: RATED_CAPACITY_W,
                fan_power_w: Some(FAN_POWER_W),
                zone_id: Some(1),
                ..GasFurnaceConfig::default()
            },
        )
        .unwrap();

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
        assert!((ports.electrical.net_active_w() - EXPECTED_FAN_W).abs() < 1.0);
        assert!((ports.thermal[0].sensible_gain_w - EXPECTED_SENSIBLE_GAIN_W).abs() < 1e-6);
    }

    #[test]
    fn gas_furnace_init_without_explicit_fan_power_computes_default() {
        let capacity_w = 3.0 * W_PER_TON;
        let cfg = EquipmentConfig::from_typed(
            "GF".to_string(),
            "Gas Furnace".to_string(),
            GasFurnaceConfig {
                afue: 0.8,
                capacity_w,
                zone_id: Some(1),
                ..GasFurnaceConfig::default()
            },
        )
        .unwrap();

        let mut eq = GasFurnace::new(cfg.clone());
        eq.init(&cfg, &env(18.0)).expect("gas furnace init");

        let expected_airflow_m3_s = eq.hvac.config.airflow_m3_s_per_w * capacity_w;
        let expected = eq.hvac.fan_power_w(expected_airflow_m3_s);
        assert!((eq.fan_power_w - expected).abs() < 1e-9);
        assert!(eq.fan_power_w > 0.0);
    }

    #[test]
    fn gas_furnace_init_explicit_fan_power_takes_precedence() {
        let cfg = EquipmentConfig::from_typed(
            "GF".to_string(),
            "Gas Furnace".to_string(),
            GasFurnaceConfig {
                afue: 0.8,
                capacity_w: 3.0 * W_PER_TON,
                fan_power_w: Some(500.0),
                zone_id: Some(1),
                ..GasFurnaceConfig::default()
            },
        )
        .unwrap();

        let mut eq = GasFurnace::new(cfg.clone());
        eq.init(&cfg, &env(18.0)).expect("gas furnace init");

        assert!((eq.fan_power_w - 500.0).abs() < 1e-9);
    }

    /// Fuel consumption is independent of duct DSE -- the furnace burns the same
    /// gas regardless of duct losses. Only the zone thermal delivery is reduced.
    #[test]
    fn gas_furnace_fuel_is_independent_of_duct_dse() {
        let cfg = EquipmentConfig::from_typed(
            "GF".to_string(),
            "Gas Furnace".to_string(),
            GasFurnaceConfig {
                afue: 0.8,
                capacity_w: 10_000.0,
                fan_power_w: Some(0.0),
                zone_id: Some(1),
                ducts: DuctConfig {
                    dse_heat: Some(0.8),
                    ..DuctConfig::default()
                },
                ..GasFurnaceConfig::default()
            },
        )
        .unwrap();

        let mut eq = GasFurnace::new(cfg.clone());
        let env = env(18.0);
        eq.init(&cfg, &env).expect("gas furnace init");

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert!(
            (ports.fuel.get(hares_types::FuelType::Gas) - 12_500.0).abs() < 1e-6,
            "fuel should be capacity/efficiency, not capacity*dse/efficiency"
        );
        assert!(
            (ports.thermal[0].sensible_gain_w - 8_000.0).abs() < 1e-6,
            "zone thermal = capacity * DSE"
        );
    }

    #[test]
    fn electric_furnace_supply_air_temp_is_set_after_init() {
        let cfg = ef_config(8_000.0, 1.0);
        let mut eq = ElectricFurnace::new(cfg.clone());
        eq.init(&cfg, &env(20.0)).unwrap();
        assert!(
            eq.hvac.config.supply_air_temp_c > 0.0,
            "supply_air_temp_c must be set after init"
        );
    }

    #[test]
    fn furnace_state_round_trip_preserves_mode_and_outputs() {
        let cfg = ef_config(8_000.0, 1.05);
        let mut eq = ElectricFurnace::new(cfg.clone());
        let env = env(18.0);
        eq.init(&cfg, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        let state = eq.save_state().unwrap();

        let mut restored = ElectricFurnace::new(cfg.clone());
        restored.init(&cfg, &env).unwrap();
        restored.load_state(&state).unwrap();
        assert_eq!(
            restored.telemetry().get(tk::ELECTRIC_KW),
            eq.telemetry().get(tk::ELECTRIC_KW)
        );
    }

    #[test]
    fn gas_furnace_state_round_trip_preserves_mode_and_outputs() {
        let cfg = EquipmentConfig::from_typed(
            "GF".to_string(),
            "Gas Furnace".to_string(),
            GasFurnaceConfig {
                afue: 0.8,
                capacity_w: 10_000.0,
                fan_power_w: Some(0.0),
                zone_id: Some(1),
                ..GasFurnaceConfig::default()
            },
        )
        .unwrap();
        let mut eq = GasFurnace::new(cfg.clone());
        let env = env(18.0);
        eq.init(&cfg, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        let state = eq.save_state().unwrap();

        let mut restored = GasFurnace::new(cfg.clone());
        restored.init(&cfg, &env).unwrap();
        restored.load_state(&state).unwrap();
        assert_eq!(
            restored.telemetry().get(tk::ELECTRIC_KW),
            eq.telemetry().get(tk::ELECTRIC_KW)
        );
    }

    #[test]
    fn ashrae_152_dse_identity() {
        let cfg_dse = EquipmentConfig::from_typed(
            "GF".to_string(),
            "Gas Furnace".to_string(),
            GasFurnaceConfig {
                afue: 0.8,
                capacity_w: 10_000.0,
                fan_power_w: Some(0.0),
                zone_id: Some(1),
                ducts: DuctConfig {
                    dse_heat: Some(0.8),
                    ..DuctConfig::default()
                },
                ..GasFurnaceConfig::default()
            },
        )
        .unwrap();

        let mut eq = GasFurnace::new(cfg_dse.clone());
        let env = env(18.0);
        eq.init(&cfg_dse, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert!(
            (ports.thermal[0].sensible_gain_w - 8_000.0).abs() < 1e-6,
            "ASHRAE 152: 10kW * DSE=0.8 should deliver 8kW, got {}",
            ports.thermal[0].sensible_gain_w,
        );

        let cfg_perfect = EquipmentConfig::from_typed(
            "GF".to_string(),
            "Gas Furnace".to_string(),
            GasFurnaceConfig {
                afue: 0.8,
                capacity_w: 10_000.0,
                fan_power_w: Some(0.0),
                zone_id: Some(1),
                ducts: DuctConfig {
                    dse_heat: Some(1.0),
                    ..DuctConfig::default()
                },
                ..GasFurnaceConfig::default()
            },
        )
        .unwrap();

        let mut eq_perfect = GasFurnace::new(cfg_perfect.clone());
        eq_perfect.init(&cfg_perfect, &env).unwrap();

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
    fn gas_furnace_writes_fuel_input_w_telemetry_key() {
        let cfg = gf_config(10_000.0, 0.9);
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
        assert!(
            eq.telemetry().get("gas_consumption_w").is_none(),
            "gas_consumption_w must not be a telemetry key on furnace"
        );
    }

    #[test]
    fn raw_config_rejected_for_gas_furnace_with_typed_diagnostic() {
        let cfg = EquipmentConfig::raw(
            "GF".to_string(),
            "Gas Furnace".to_string(),
            std::collections::HashMap::new(),
        );
        let mut eq = GasFurnace::new(cfg.clone());
        let result = eq.init(&cfg, &env(18.0));
        let err = result.expect_err("raw gas furnace config must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("Gas Furnace requires typed config"));
        assert!(msg.contains("from_typed"));
    }

    #[test]
    fn raw_config_rejected_for_electric_furnace_with_typed_diagnostic() {
        let cfg = EquipmentConfig::raw(
            "EF".to_string(),
            "Electric Furnace".to_string(),
            std::collections::HashMap::new(),
        );
        let mut eq = ElectricFurnace::new(cfg.clone());
        let result = eq.init(&cfg, &env(18.0));
        let err = result.expect_err("raw electric furnace config must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("Electric Furnace requires typed config"));
        assert!(msg.contains("from_typed"));
    }

    #[test]
    fn registry_includes_furnace_aliases_and_thermal_stage() {
        let registry = EquipmentRegistry::new();
        assert!(registry.get("Electric Furnace").is_some());
        assert!(registry.get("Gas Furnace").is_some());

        let cfg = EquipmentConfig::from_typed(
            "EF".to_string(),
            "Electric Furnace".to_string(),
            ElectricFurnaceConfig {
                eir: 1.0,
                capacity_w: 5_000.0,
                fan_power_w: Some(0.0),
                zone_id: Some(1),
                ..ElectricFurnaceConfig::default()
            },
        )
        .unwrap();
        let eq = registry.create("Electric Furnace", cfg).unwrap();
        assert_eq!(eq.descriptor().stage, ExecutionStage::Thermal);
    }

    #[test]
    fn electric_furnace_ideal_capacity_control_scales_output() {
        let cfg = ef_config(8_000.0, 1.0);
        let mut eq = ElectricFurnace::new(cfg.clone());
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
            degraded: false,
        })
        .unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert!(
            (ports.thermal[0].sensible_gain_w - 4_000.0).abs() < 1e-6,
            "IdealCapacity must scale electric-furnace thermal output to commanded value"
        );
    }

    #[test]
    fn electric_furnace_telemetry_fields_declares_fan_kw() {
        use super::electric_furnace_telemetry_fields;
        let fields = electric_furnace_telemetry_fields();
        assert!(
            fields.iter().any(|f| f.name == tk::FAN_KW),
            "electric furnace telemetry_fields must declare FAN_KW"
        );
    }

    /// Gas furnaces and electric furnaces have no compressor startup transient.
    /// OCHRE only applies startup capacity degradation (c_d) to heat pumps and
    /// cooling equipment; furnaces must have c_d == 0.0 after init.
    #[test]
    fn furnaces_have_no_startup_capacity_degradation() {
        let gas_cfg = EquipmentConfig::from_typed(
            "GF".to_string(),
            "Gas Furnace".to_string(),
            GasFurnaceConfig {
                afue: 0.8,
                capacity_w: 10_000.0,
                fan_power_w: Some(0.0),
                zone_id: Some(1),
                ..GasFurnaceConfig::default()
            },
        )
        .unwrap();
        let mut gas_eq = GasFurnace::new(gas_cfg.clone());
        gas_eq.init(&gas_cfg, &env(18.0)).unwrap();
        assert_eq!(
            gas_eq.hvac.runtime.startup.c_d, 0.0,
            "gas furnace must have startup c_d == 0.0 after init"
        );

        let elec_cfg = EquipmentConfig::from_typed(
            "EF".to_string(),
            "Electric Furnace".to_string(),
            ElectricFurnaceConfig {
                eir: 1.0,
                capacity_w: 8_000.0,
                fan_power_w: Some(0.0),
                zone_id: Some(1),
                ..ElectricFurnaceConfig::default()
            },
        )
        .unwrap();
        let mut elec_eq = ElectricFurnace::new(elec_cfg.clone());
        elec_eq.init(&elec_cfg, &env(18.0)).unwrap();
        assert_eq!(
            elec_eq.hvac.runtime.startup.c_d, 0.0,
            "electric furnace must have startup c_d == 0.0 after init"
        );
    }

    #[test]
    fn gas_furnace_two_speed_stage_capacities_and_eirs_are_wired() {
        let low_cap_w = 6_500.0;
        let high_cap_w = 10_000.0;
        let low_eir = 1.0 / 0.78;
        let high_eir = 1.0 / 0.80;
        let cfg = EquipmentConfig::from_typed(
            "GF".to_string(),
            "Gas Furnace".to_string(),
            GasFurnaceConfig {
                afue: 0.80,
                capacity_w: high_cap_w,
                fan_power_w: Some(0.0),
                zone_id: Some(1),
                number_of_speeds: 2,
                stage_heating_capacities_w: Some(vec![low_cap_w, high_cap_w]),
                stage_heating_eirs: Some(vec![low_eir, high_eir]),
                ..GasFurnaceConfig::default()
            },
        )
        .unwrap();
        let mut eq = GasFurnace::new(cfg.clone());
        eq.init(&cfg, &env(18.0)).unwrap();

        assert_eq!(
            eq.hvac.config.heating_capacities_w.len(),
            2,
            "two-speed furnace must have two capacity stages"
        );
        assert!(
            (eq.hvac.config.heating_capacities_w[0] - low_cap_w).abs() < 1e-9,
            "low stage capacity must match stage_heating_capacities_w[0]"
        );
        assert!(
            (eq.hvac.config.heating_capacities_w[1] - high_cap_w).abs() < 1e-9,
            "high stage capacity must match stage_heating_capacities_w[1]"
        );
        assert_eq!(
            eq.hvac.config.eir_by_stage.len(),
            2,
            "two-speed furnace must have two EIR stages"
        );
        assert!(
            (eq.hvac.config.eir_by_stage[0] - low_eir).abs() < 1e-9,
            "low stage EIR must match stage_heating_eirs[0]"
        );
        assert!(
            (eq.hvac.config.eir_by_stage[1] - high_eir).abs() < 1e-9,
            "high stage EIR must match stage_heating_eirs[1]"
        );
    }

    #[test]
    fn gas_furnace_two_speed_mismatched_stage_counts_are_rejected() {
        let cfg = EquipmentConfig::from_typed(
            "GF".to_string(),
            "Gas Furnace".to_string(),
            GasFurnaceConfig {
                afue: 0.80,
                capacity_w: 10_000.0,
                fan_power_w: Some(0.0),
                zone_id: Some(1),
                number_of_speeds: 2,
                stage_heating_capacities_w: Some(vec![6_500.0, 10_000.0]),
                stage_heating_eirs: Some(vec![1.25]),
                ..GasFurnaceConfig::default()
            },
        )
        .unwrap();
        let mut eq = GasFurnace::new(cfg.clone());
        let err = eq
            .init(&cfg, &env(18.0))
            .expect_err("mismatched stage counts must be rejected");
        assert!(
            err.to_string().contains("stage counts must match"),
            "error must describe the mismatch; got: {err}"
        );
    }

    #[test]
    fn gas_furnace_speed_index_telemetry_present_after_step() {
        let cfg = gf_config(10_000.0, 0.80);
        let mut eq = GasFurnace::new(cfg.clone());
        let env = env(18.0);
        eq.init(&cfg, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert!(
            eq.telemetry().get(tk::SPEED_INDEX).is_some(),
            "SPEED_INDEX must be present in gas furnace telemetry after a heating step"
        );
    }

    #[test]
    fn gas_furnace_speed_index_in_telemetry_fields() {
        use super::gas_furnace_telemetry_fields;
        let fields = gas_furnace_telemetry_fields();
        assert!(
            fields.iter().any(|f| f.name == tk::SPEED_INDEX),
            "gas_furnace_telemetry_fields must declare SPEED_INDEX"
        );
    }

    // ── Duct loss and main power telemetry ──────────────────────────────────

    /// ASHRAE 152: duct_loss_w = gross_capacity_w * (1 - dse).
    /// Gas furnace with DSE=0.8 and 10 kW capacity must report 2000 W duct loss.
    #[test]
    fn gas_furnace_duct_loss_uses_pre_dse_gross_capacity() {
        const CAP_W: f64 = 10_000.0;
        const DSE: f64 = 0.8;
        let cfg = EquipmentConfig::from_typed(
            "GF".to_string(),
            "Gas Furnace".to_string(),
            GasFurnaceConfig {
                afue: 0.80,
                capacity_w: CAP_W,
                fan_power_w: Some(0.0),
                zone_id: Some(1),
                ducts: DuctConfig {
                    dse_heat: Some(DSE),
                    ..DuctConfig::default()
                },
                ..GasFurnaceConfig::default()
            },
        )
        .unwrap();
        let mut eq = GasFurnace::new(cfg.clone());
        let env = env(18.0);
        eq.init(&cfg, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let duct_loss_w = eq.telemetry().get(tk::DUCT_LOSS_W).unwrap();
        // gross_capacity_w = CAP_W * duty=1.0, duct_loss = CAP_W * (1 - 0.8) = 2000
        assert!(
            (duct_loss_w - CAP_W * (1.0 - DSE)).abs() < 1e-6,
            "duct_loss_w must be gross_capacity * (1 - dse) = {}, got {duct_loss_w}",
            CAP_W * (1.0 - DSE)
        );
    }

    /// Gas furnace main_power = gas fuel input in kW.
    /// OCHRE HVAC.py:575: main_power = total_input - fan.
    #[test]
    fn gas_furnace_main_power_equals_fuel_input_in_kw() {
        const CAP_W: f64 = 10_000.0;
        const AFUE: f64 = 0.80;
        let cfg = EquipmentConfig::from_typed(
            "GF".to_string(),
            "Gas Furnace".to_string(),
            GasFurnaceConfig {
                afue: AFUE,
                capacity_w: CAP_W,
                fan_power_w: Some(0.0),
                zone_id: Some(1),
                ..GasFurnaceConfig::default()
            },
        )
        .unwrap();
        let mut eq = GasFurnace::new(cfg.clone());
        let env = env(18.0);
        eq.init(&cfg, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let main_kw = eq.telemetry().get(tk::MAIN_POWER_KW).unwrap();
        let expected_fuel_kw = (CAP_W / AFUE) / 1000.0;
        assert!(
            (main_kw - expected_fuel_kw).abs() < 1e-6,
            "gas furnace main_power_kw must equal fuel_input in kW ({expected_fuel_kw}), got {main_kw}"
        );
    }

    /// Furnace writes RUNTIME_FRACTION = duty_cycle when heating, 0 when off.
    #[test]
    fn gas_furnace_runtime_fraction_is_duty_cycle_when_heating() {
        let cfg = gf_config(10_000.0, 0.80);
        let mut eq = GasFurnace::new(cfg.clone());
        let env = env(18.0);
        eq.init(&cfg, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let rtf = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap();
        let duty = eq.hvac.runtime.duty_cycle.clamp(0.0, 1.0);
        assert!(
            (rtf - duty).abs() < 1e-9,
            "runtime_fraction must match duty cycle ({duty}), got {rtf}"
        );
    }

    /// Electric furnace main_power = total_input - fan (heating element only).
    /// OCHRE HVAC.py:575.
    #[test]
    fn electric_furnace_main_power_excludes_fan() {
        const CAP_W: f64 = 8_000.0;
        const EIR: f64 = 1.0;
        const FAN_W: f64 = 400.0;
        let cfg = EquipmentConfig::from_typed(
            "EF".to_string(),
            "Electric Furnace".to_string(),
            ElectricFurnaceConfig {
                eir: EIR,
                capacity_w: CAP_W,
                fan_power_w: Some(FAN_W),
                zone_id: Some(1),
                ..ElectricFurnaceConfig::default()
            },
        )
        .unwrap();
        let mut eq = ElectricFurnace::new(cfg.clone());
        let env = env(18.0);
        eq.init(&cfg, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let main_kw = eq.telemetry().get(tk::MAIN_POWER_KW).unwrap();
        let electric_kw = eq.telemetry().get(tk::ELECTRIC_KW).unwrap();
        let fan_kw = eq.telemetry().get(tk::FAN_KW).unwrap();
        // main_power = electric_kw - fan_kw = (elements + fan) - fan = elements only
        let expected = electric_kw - fan_kw;
        assert!(
            (main_kw - expected).abs() < 1e-9,
            "electric furnace main_power must equal electric_kw - fan_kw ({expected}), got {main_kw}"
        );
        assert!(main_kw > 0.0, "main_power must be positive during heating");
    }

    #[test]
    fn electric_furnace_can_accept_mode_override_and_demand_response() {
        let cfg = ef_config(8_000.0, 1.05);
        let eq = ElectricFurnace::new(cfg);
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
    fn gas_furnace_can_accept_mode_override_and_demand_response() {
        let cfg = gf_config(10_000.0, 0.80);
        let eq = GasFurnace::new(cfg);
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
    fn electric_furnace_mode_override_off_forces_off() {
        let cfg = ef_config(8_000.0, 1.05);
        let mut eq = ElectricFurnace::new(cfg.clone());
        let state = env(18.0);
        eq.init(&cfg, &state).unwrap();

        eq.apply_control_unchecked(&ControlSignal::ModeOverride {
            mode: OperatingMode::Off,
        })
        .unwrap();
        let mode = eq.update_control(&state);
        assert_eq!(mode, OperatingMode::Off);

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&state, Duration::from_secs(60), &mut ports)
            .unwrap();
        assert!(
            ports.thermal[0].sensible_gain_w < 1e-9,
            "ModeOverride Off must prevent furnace heating"
        );
    }

    #[test]
    fn gas_furnace_mode_override_off_forces_off() {
        let cfg = gf_config(10_000.0, 0.80);
        let mut eq = GasFurnace::new(cfg.clone());
        let state = env(18.0);
        eq.init(&cfg, &state).unwrap();

        eq.apply_control_unchecked(&ControlSignal::ModeOverride {
            mode: OperatingMode::Off,
        })
        .unwrap();
        let mode = eq.update_control(&state);
        assert_eq!(mode, OperatingMode::Off);

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&state, Duration::from_secs(60), &mut ports)
            .unwrap();
        assert!(
            ports.thermal[0].sensible_gain_w < 1e-9,
            "ModeOverride Off must prevent gas furnace heating"
        );
    }

    #[test]
    fn electric_furnace_demand_response_grid_emergency_forces_off() {
        let cfg = ef_config(8_000.0, 1.05);
        let mut eq = ElectricFurnace::new(cfg.clone());
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
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        assert!(
            ports.thermal[0].sensible_gain_w < 1e-9,
            "GridEmergency must prevent furnace heating"
        );
    }

    /// Per-component reactive for the electric furnace: the resistance
    /// element is unity pf (Q ≡ 0), but the blower fan motor carries the
    /// FAN component (pf 0.87) — Q must equal fan·tan(acos(0.87)) alone.
    #[test]
    fn electric_furnace_reactive_power_is_fan_component_only() {
        let cfg = EquipmentConfig::from_typed(
            "EF".to_string(),
            "Electric Furnace".to_string(),
            ElectricFurnaceConfig {
                eir: 1.05,
                capacity_w: 8_000.0,
                fan_power_w: Some(300.0),
                zone_id: Some(1),
                ..ElectricFurnaceConfig::default()
            },
        )
        .unwrap();
        let mut eq = ElectricFurnace::new(cfg.clone());
        let env = env(18.0);
        eq.init(&cfg, &env).unwrap();
        assert!(
            eq.descriptor()
                .core_capabilities
                .contains(CoreCapabilities::REACTIVE)
        );
        assert_eq!(eq.zip.pf, 1.0, "element class default pf (RESISTANCE)");
        assert_eq!(
            eq.fan_zip,
            crate::hvac::reactive::FAN_MOTOR_ZIP,
            "blower component uses the FAN motor ZIP"
        );

        eq.update_control(&env);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert!(ports.electrical.load_power_w > 0.0);
        let fan_kw = eq.telemetry().get(tk::FAN_KW).expect("fan");
        assert!(fan_kw > 0.0, "blower must run while heating");
        let q = ports.electrical.reactive_power_kvar;
        let expected = fan_kw * 0.87_f64.acos().tan();
        assert!(
            (q - expected).abs() < 1e-12,
            "Q must be the blower component only (element resistive): \
             q={q}, expected={expected}"
        );
        assert_eq!(eq.core_output().flows.reactive_power_kvar, Some(q));
        assert_eq!(eq.telemetry().get(tk::REACTIVE_POWER_KVAR), Some(q));
        hares_types::validate_core_contract(eq.descriptor(), eq.core_output())
            .expect("validate_core_contract");
    }

    #[test]
    fn electric_furnace_real_power_bit_identical_with_and_without_reactive_zip() {
        let cfg_pf = ef_config(8_000.0, 1.05);
        let mut cfg_nopf = ef_config(8_000.0, 1.05);
        cfg_nopf.zip = Some(hares_types::zip::ZipLoad::constant_power());

        let env_base = env(18.0);
        let mut eq_pf = ElectricFurnace::new(cfg_pf.clone());
        let mut eq_nopf = ElectricFurnace::new(cfg_nopf.clone());
        eq_pf.init(&cfg_pf, &env_base).unwrap();
        eq_nopf.init(&cfg_nopf, &env_base).unwrap();

        assert!(
            eq_pf
                .descriptor()
                .core_capabilities
                .contains(CoreCapabilities::REACTIVE)
        );

        for (i, v) in [1.0, 0.95, 1.05, 1.0, 0.9, 1.1].iter().enumerate() {
            let mut env_v = env(18.0);
            env_v.grid.voltage_pu = *v;
            let mut ports_pf = PortSlots {
                thermal: vec![ThermalAccumulator::new(ZoneId(1))],
                ..PortSlots::default()
            };
            let mut ports_nopf = PortSlots {
                thermal: vec![ThermalAccumulator::new(ZoneId(1))],
                ..PortSlots::default()
            };
            eq_pf.update_control(&env_v);
            eq_nopf.update_control(&env_v);
            eq_pf
                .step(&env_v, Duration::from_secs(60), &mut ports_pf)
                .unwrap();
            eq_nopf
                .step(&env_v, Duration::from_secs(60), &mut ports_nopf)
                .unwrap();
            assert_eq!(
                ports_pf.electrical.load_power_w.to_bits(),
                ports_nopf.electrical.load_power_w.to_bits(),
                "step {i} (v={v}): real power diverged"
            );
            assert_eq!(ports_nopf.electrical.reactive_power_kvar, 0.0);
        }
    }

    #[test]
    fn gas_furnace_reactive_power_blended_pf_and_channels_agree() {
        let config = EquipmentConfig::from_typed(
            "GF".to_string(),
            "Gas Furnace".to_string(),
            GasFurnaceConfig {
                afue: 0.8,
                capacity_w: 10_000.0,
                fan_power_w: Some(100.0),
                zone_id: Some(1),
                ..GasFurnaceConfig::default()
            },
        )
        .unwrap();
        let mut eq = GasFurnace::new(config.clone());
        let env = env(18.0);
        eq.init(&config, &env).unwrap();
        assert!(
            eq.descriptor()
                .core_capabilities
                .contains(CoreCapabilities::REACTIVE)
        );
        let pf = 0.87_f64;
        assert_eq!(eq.zip.pf, pf);

        eq.update_control(&env);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let p_kw = ports.electrical.net_active_w() / 1000.0;
        let q = ports.electrical.reactive_power_kvar;
        let expected = p_kw * pf.acos().tan();
        assert!(
            (q - expected).abs() < 1e-9,
            "Q/P must equal tan(acos({pf})): {q} vs {expected}"
        );

        let co_q = eq.core_output().flows.reactive_power_kvar.expect("Some");
        assert_eq!(
            co_q.to_bits(),
            q.to_bits(),
            "CoreOutput Q must match port Q"
        );
        assert_eq!(
            eq.telemetry()
                .get(tk::REACTIVE_POWER_KVAR)
                .unwrap()
                .to_bits(),
            q.to_bits(),
            "telemetry Q must match port Q"
        );
        hares_types::validate_core_contract(eq.descriptor(), eq.core_output())
            .expect("validate_core_contract");
    }

    #[test]
    fn gas_furnace_reactive_power_zero_when_off() {
        let config = EquipmentConfig::from_typed(
            "GF-off".to_string(),
            "Gas Furnace".to_string(),
            GasFurnaceConfig {
                afue: 0.8,
                capacity_w: 10_000.0,
                fan_power_w: Some(100.0),
                zone_id: Some(1),
                ..GasFurnaceConfig::default()
            },
        )
        .unwrap();
        let mut eq = GasFurnace::new(config.clone());
        let env = env(25.0);
        eq.init(&config, &env).unwrap();

        eq.update_control(&env);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert_eq!(ports.electrical.load_power_w, 0.0);
        assert_eq!(ports.electrical.reactive_power_kvar, 0.0);
        assert_eq!(
            eq.core_output().flows.reactive_power_kvar,
            Some(0.0),
            "off equipment must report Some(0.0)"
        );
    }

    #[test]
    fn gas_furnace_real_power_bit_identical_with_and_without_reactive_zip() {
        let base_cfg = GasFurnaceConfig {
            afue: 0.8,
            capacity_w: 10_000.0,
            fan_power_w: Some(100.0),
            zone_id: Some(1),
            ..GasFurnaceConfig::default()
        };
        let cfg_pf = EquipmentConfig::from_typed(
            "GF-PF".to_string(),
            "Gas Furnace".to_string(),
            base_cfg.clone(),
        )
        .unwrap();
        let mut cfg_nopf =
            EquipmentConfig::from_typed("GF-NOPF".to_string(), "Gas Furnace".to_string(), base_cfg)
                .unwrap();
        cfg_nopf.zip = Some(hares_types::zip::ZipLoad::constant_power());

        let env_base = env(18.0);
        let mut eq_pf = GasFurnace::new(cfg_pf.clone());
        let mut eq_nopf = GasFurnace::new(cfg_nopf.clone());
        eq_pf.init(&cfg_pf, &env_base).unwrap();
        eq_nopf.init(&cfg_nopf, &env_base).unwrap();

        assert!(
            eq_pf
                .descriptor()
                .core_capabilities
                .contains(CoreCapabilities::REACTIVE)
        );

        let mut any_reactive = false;
        for (i, v) in [1.0, 0.95, 1.05, 1.0, 0.9, 1.1].iter().enumerate() {
            let mut env_v = env(18.0);
            env_v.grid.voltage_pu = *v;
            let mut ports_pf = PortSlots {
                thermal: vec![ThermalAccumulator::new(ZoneId(1))],
                ..PortSlots::default()
            };
            let mut ports_nopf = PortSlots {
                thermal: vec![ThermalAccumulator::new(ZoneId(1))],
                ..PortSlots::default()
            };
            eq_pf.update_control(&env_v);
            eq_nopf.update_control(&env_v);
            eq_pf
                .step(&env_v, Duration::from_secs(60), &mut ports_pf)
                .unwrap();
            eq_nopf
                .step(&env_v, Duration::from_secs(60), &mut ports_nopf)
                .unwrap();
            assert_eq!(
                ports_pf.electrical.load_power_w.to_bits(),
                ports_nopf.electrical.load_power_w.to_bits(),
                "step {i} (v={v}): real power diverged"
            );
            if ports_pf.electrical.reactive_power_kvar != 0.0 {
                any_reactive = true;
            }
            assert_eq!(ports_nopf.electrical.reactive_power_kvar, 0.0);
        }
        assert!(any_reactive, "the pf 0.87 twin must produce reactive power");
    }
}
