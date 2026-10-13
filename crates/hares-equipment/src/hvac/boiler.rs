//! Electric and gas boiler models.

use std::borrow::Cow;
use std::time::Duration;

use chrono::{DateTime, FixedOffset};
use hares_types::{
    ControlCapabilities, ControlSignal, CoreCapabilities, CoreFlows, CoreOutput, CorePerformance,
    CoreState, DRLevel, ElectricPower, EndUse, EnvironmentState, EquipmentDescriptor,
    EquipmentHealthCounts, EquipmentId, ExecutionStage, FluidDomainPayload, FluidNodeId, FluidType,
    FuelPower, FuelType, HaresError, HeatTransferDirection, LoopId, OperatingMode,
    PortContribution, PortDeclaration, PortSlots, Telemetry, TelemetryField, ThermalCategory,
    Warning,
};
use serde::{Deserialize, Serialize};

use hares_types::telemetry_keys as tk;

use crate::hvac::heating_config::{ElectricBoilerConfig, GasBoilerConfig};
use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_versioned, try_save_versioned};

use super::{
    HvacEquipment, HvacEquipmentType, RuntimeSetpointOverride, ThermostatMode,
    helpers::{
        apply_simple_heating_ideal_capacity_control, apply_simple_mode_override_and_dr,
        apply_simple_mode_override_in_control, lookup_zone, loop_id_from_config, outage_forces_off,
        register_ebm_telemetry_keys, require_loop_id, served_zone_ports, step_equivalent_battery,
        update_heating_control, write_ebm_telemetry, zone_id_from_config,
    },
};
use crate::config::constructor_equipment_id;

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
    loop_id: Option<LoopId>,
    fluid_type: FluidType,
    flow_rate_kg_s: f64,
    default_return_temp_c: f64,
    /// Hydronic circulation-pump electric draw [kW], from `fan_power_w`
    /// (same config surface as the gas boiler; 0.0 when unspecified).
    pump_kw: f64,
    operating_mode: OperatingMode,
    run_time_s: f64,
    /// Cached from last update_control; true when timestep >= 5 min
    /// so ideal_target() can participate in the solver feedback loop.
    use_ideal: bool,
    /// External ModeOverride control (sticky).
    mode_override: Option<OperatingMode>,
    /// External DemandResponse level (sticky).
    dr_level: DRLevel,
    /// Whether the fluid domain has published a payload this run. The
    /// absent-payload escape is bounded: silent before the first publish,
    /// a counted degradation after it.
    fluid_payload_seen: bool,
    /// Absent-payload steps since the last health drain (after the first
    /// publish; the count reaches the run's health through
    /// `take_health_counts`).
    fluid_payload_absences: u64,
    /// Whether this run already warned on a late absent payload: the
    /// first one carries the detail, the rest are counted silently in
    /// the run's health (the once-per-run warning pattern), so a payload
    /// starved for the rest of a year does not flood the run's warning
    /// log and tracing with a line per step.
    fluid_absence_warned: bool,
    /// Warnings raised since the last drain (the late absent-payload
    /// degradations; they reach the run's warning log through
    /// `drain_warnings`).
    warnings: Vec<Warning>,
    /// Rule R1 reactive-only ZIP for the primary component (resistive
    /// element, class default pf 1.0 → Q exactly zero); real power stays
    /// bit-identical.
    zip: hares_types::zip::ResolvedZip,
    /// Hydronic circulation-pump motor component ZIP (pf 0.84), derived at
    /// init via `hvac::reactive::secondary_motor_zip`.
    pump_zip: hares_types::zip::ZipLoad,
}

pub struct GasBoiler {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,
    core_output: CoreOutput,
    hvac: HvacEquipment,
    rated_capacity_w: f64,
    loop_id: Option<LoopId>,
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
    /// External ModeOverride control (sticky).
    mode_override: Option<OperatingMode>,
    /// External DemandResponse level (sticky).
    dr_level: DRLevel,
    /// Whether the fluid domain has published a payload this run: the
    /// absent-payload escape is silent only before the first publish.
    fluid_payload_seen: bool,
    /// Absent-payload steps since the last health drain (after the first
    /// publish).
    fluid_payload_absences: u64,
    /// Whether this run already warned on a late absent payload: the
    /// first one carries the detail, the rest are counted silently in
    /// the run's health (the once-per-run warning pattern).
    fluid_absence_warned: bool,
    /// Warnings raised since the last drain (the late absent-payload
    /// degradations).
    warnings: Vec<Warning>,
    /// Rule R1 reactive-only ZIP: circulation pump/aux pf 0.84;
    /// Q comes from ZipLoad::reactive_kvar.
    zip: hares_types::zip::ResolvedZip,
}

/// Version 3: the fluid domain's first-publish marker is persisted (the
/// absent-payload escape's bound survives a checkpoint resume).
const BOILER_CHECKPOINT_VERSION: u32 = 3;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct BoilerState {
    mode: ThermostatMode,
    duty_cycle: f64,
    last_mode_switch_at: Option<DateTime<FixedOffset>>,
    runtime_setpoints: Option<RuntimeSetpointOverride>,
    /// The hysteresis in force, which a named `ThermalSetpoint` may have set.
    thermostat_hysteresis_c: f64,
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
    /// The fluid domain has published a payload this run (bounds the
    /// absent-payload escape across a checkpoint resume).
    fluid_payload_seen: bool,
}

impl ElectricBoiler {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let zone = zone_id_from_config(&config);
        let loop_id = loop_id_from_config(&config, &["loop_id", "hydronic_loop_id"]);
        let descriptor = EquipmentDescriptor {
            id: EquipmentId(constructor_equipment_id(&config)),
            name: config.name,
            end_use: EndUse::HVAC_HEATING,
            equipment_type: Cow::Borrowed("Electric Boiler"),
            zone,
            fuel: FuelType::Electric,
            stage: ExecutionStage::Thermal,
            control_capabilities: ControlCapabilities::THERMAL_SETPOINT
                | ControlCapabilities::THERMAL_SETPOINT_DELTA
                | ControlCapabilities::IDEAL_CAPACITY
                | ControlCapabilities::MODE_OVERRIDE
                | ControlCapabilities::DEMAND_RESPONSE
                | ControlCapabilities::NON_HVAC_ZONE_INPUT,
            core_capabilities: CoreCapabilities::ELECTRIC
                | CoreCapabilities::REACTIVE
                | CoreCapabilities::HAS_MODE
                | CoreCapabilities::THERMAL
                | CoreCapabilities::HAS_SETPOINT,
            telemetry_fields: electric_boiler_telemetry_fields(),
            zone_type: None,
        };

        Self {
            descriptor,
            ports: vec![PortDeclaration::electrical(), declared_fluid_port(loop_id)],
            telemetry: electric_boiler_default_telemetry(),
            core_output: CoreOutput::default(),
            hvac: HvacEquipment::new(HvacEquipmentType::Other, zone),
            rated_capacity_w: 0.0,
            eir: 1.0,
            loop_id,
            fluid_type: FluidType::Water,
            flow_rate_kg_s: 0.0,
            default_return_temp_c: DEFAULT_RETURN_TEMP_C,
            pump_kw: 0.0,
            operating_mode: OperatingMode::Off,
            run_time_s: 0.0,
            use_ideal: false,
            mode_override: None,
            dr_level: DRLevel::Normal,
            fluid_payload_seen: false,
            fluid_payload_absences: 0,
            fluid_absence_warned: false,
            warnings: Vec::new(),
            zip: hares_types::zip::ResolvedZip::reactive_only(
                hares_types::zip::ZipLoad::constant_power(),
            ),
            pump_zip: hares_types::zip::ZipLoad::constant_power(),
        }
    }
}

impl Equipment for ElectricBoiler {
    fn checkpoint_version() -> u32 {
        BOILER_CHECKPOINT_VERSION
    }

    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn rename(&mut self, name: String) {
        self.descriptor.name = name;
    }

    fn set_equipment_id(&mut self, id: EquipmentId) -> crate::Result<()> {
        crate::apply_identity_write(self.is_initialized(), &mut self.descriptor, id)
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        let typed = config.require_typed::<ElectricBoilerConfig>("Electric Boiler")?;
        self.loop_id = Some(require_loop_id(
            typed.loop_id,
            "Electric Boiler",
            &config.name,
        )?);
        self.hvac.init(config, env)?;
        self.zip = crate::config::resolve_reactive_zip(config)?;
        // The circulation pump is a secondary motor component (pf 0.84); the
        // unit ZIP describes the primary component (resistive element, pf 1.0).
        self.pump_zip =
            super::reactive::secondary_motor_zip(&self.zip, super::reactive::LOOP_PUMP_ZIP);
        self.rated_capacity_w = typed.capacity_w.max(0.0);
        self.eir = typed.eir;
        self.pump_kw = power_w_to_kw(typed.fan_power_w.unwrap_or(0.0));
        self.fluid_type = typed.fluid_type;
        self.flow_rate_kg_s = typed.flow_rate_kg_s.max(0.0);
        self.default_return_temp_c = typed.return_temp_c;
        if self.eir <= 0.0 || !self.eir.is_finite() {
            return Err(HaresError::Equipment(format!(
                "invalid Electric Boiler eir: {}",
                self.eir
            )));
        }
        self.hvac.config.eir_by_stage = vec![self.eir];
        self.hvac.config.heating_capacities_w = vec![self.rated_capacity_w];
        self.hvac.update_zone_heat_fractions()?;
        self.hvac.rebuild_thermal_ports(&mut self.ports, false);
        self.descriptor.zone = self.hvac.config.zone_id;
        // The fluid port's declared loop id and fluid type follow the
        // configuration: rebuild_thermal_ports pushes the thermal ports to
        // the end of the vector, so the fluid port is located by type, not
        // by index. A glycol boiler declares a glycol loop, and a shared
        // loop with a water port is the conflicting-type construction error.
        for port in &mut self.ports {
            if port.port_type == hares_types::PortType::Fluid {
                port.loop_id = self.loop_id;
                port.fluid_type = Some(self.fluid_type);
            }
        }
        self.operating_mode = OperatingMode::Off;
        self.run_time_s = 0.0;
        self.fluid_payload_seen = false;
        self.fluid_payload_absences = 0;
        self.fluid_absence_warned = false;
        self.warnings.clear();
        self.telemetry = electric_boiler_default_telemetry();
        register_ebm_telemetry_keys(&mut self.telemetry);
        self.core_output = CoreOutput::default();
        Ok(())
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        self.use_ideal = self.hvac.use_ideal_capacity(env);
        // Grid outage: a de-energized bus removes supply power for the
        // elements/burner controls and the blower/circulator, so the unit
        // cannot run (and cannot deliver heat) regardless of thermostat
        // calls or overrides. Gated before override handling; islanded
        // (battery/generator-backed) homes keep an energized bus and are
        // not affected. See docs/outage-behavior.md.
        if outage_forces_off(&mut self.hvac, env) {
            self.operating_mode = OperatingMode::Off;
            return OperatingMode::Off;
        }
        if let Some(mode) = apply_simple_mode_override_in_control(
            &mut self.hvac,
            &mut self.mode_override,
            self.dr_level,
            "Electric Boiler",
        ) {
            self.operating_mode = mode;
            return mode;
        }
        self.operating_mode =
            match update_heating_control(&mut self.hvac, env, &self.descriptor.name) {
                Ok(mode) => mode,
                Err(err) => {
                    self.hvac.record_control_error(err);
                    OperatingMode::Off
                }
            };
        self.operating_mode
    }

    fn step(
        &mut self,
        env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        if let Some(err) = self.hvac.take_control_error() {
            return Err(err);
        }
        let duty = self.hvac.runtime.duty_cycle.clamp(0.0, 1.0);
        let sf = self.hvac.config.space_fraction;
        let thermal_output_w = self.rated_capacity_w * duty * sf;
        let element_kw = power_w_to_kw(thermal_output_w * self.eir);
        // Circulation pump runs whenever the boiler delivers heat (same
        // modeling as the gas boiler: contactor-switched, rated draw, not
        // modulated by PLR).
        let pump_kw = if thermal_output_w > 0.0 {
            self.pump_kw * sf
        } else {
            0.0
        };
        let electric_kw = element_kw + pump_kw;

        let loop_id = bound_loop_id(self.loop_id, "Electric Boiler", &self.descriptor.name)?;
        let payload_was_seen = self.fluid_payload_seen;
        let return_temp_read = loop_return_temp_c(
            env,
            loop_id,
            "Electric Boiler",
            &self.descriptor.name,
            &mut self.fluid_payload_seen,
        )?;
        let return_temp_c = match return_temp_read {
            Some(temp) => temp,
            None => {
                // The fluid domain published no payload: the configured
                // return temperature covers the step. Before the first
                // publish that is the escape's whole contract (the first
                // step runs before the envelope's first resolve); after
                // it, the absence is a degradation: counted in the run's
                // health, and warned once per run with the detail (the
                // once-per-run pattern keeps a payload starved for the
                // rest of a run from flooding the warning log and the
                // tracing with a line per step).
                if payload_was_seen {
                    self.fluid_payload_absences += 1;
                    if !self.fluid_absence_warned {
                        self.fluid_absence_warned = true;
                        let message = format!(
                            "Electric Boiler '{}': the fluid domain published no payload \
                             this step; the return temperature of loop {loop_id:?} reads \
                             from the configured return temperature until the fluid \
                             domain publishes again; further absent-payload steps are \
                             counted silently in the run health's fluid_payload_absences",
                            self.descriptor.name
                        );
                        tracing::warn!(
                            equipment = %self.descriptor.name,
                            loop_id = ?loop_id,
                            "{message}"
                        );
                        self.warnings
                            .push(Warning::new(self.descriptor.name.clone(), message));
                    }
                }
                self.default_return_temp_c
            }
        };
        let cp_used = cp_j_kg_k(self.fluid_type);
        let supply_temp_c = if self.flow_rate_kg_s > 0.0 {
            return_temp_c + thermal_output_w / (self.flow_rate_kg_s * cp_used)
        } else {
            return_temp_c
        };

        // Per-component reactive (see `hvac::reactive`): the resistance
        // element at the unit ZIP (pf 1.0 → Q ≡ 0), the circulation pump
        // motor at pf 0.84.
        let reactive_power_kvar = self
            .zip
            .reactive_kvar(element_kw, env.grid.bus_voltage_pu())
            + self
                .pump_zip
                .reactive_kvar(pump_kw, env.grid.bus_voltage_pu());
        let ebm_window = step_equivalent_battery(&self.hvac, env)?;
        if electric_kw > 0.0 {
            ports.accumulate(&PortContribution::Electrical {
                active_power_w: power_kw_to_w(electric_kw),
                reactive_power_kvar,
            })?;
        }

        if thermal_output_w > 0.0 {
            ports.accumulate(&PortContribution::Fluid {
                loop_id,
                flow_rate_kg_s: self.flow_rate_kg_s,
                supply_temp_c,
                return_temp_c,
                fluid_type: self.fluid_type,
                thermal_power_w: None,
                node_id: FluidNodeId(0),
                direction: HeatTransferDirection::Source,
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
        self.telemetry.set(tk::PUMP_POWER_KW, pump_kw);
        self.telemetry
            .set(tk::REACTIVE_POWER_KVAR, reactive_power_kvar);
        self.telemetry.set(tk::THERMAL_OUTPUT_W, thermal_output_w);
        // The runtime fraction and the part-load ratio coincide for a
        // resistance element (EnergyPlus HeatingCoils.cc:1873: `ElecUseLoad
        // *= PartLoadRatio`).
        self.telemetry.set(tk::RUNTIME_FRACTION, duty);
        self.telemetry.set(tk::PART_LOAD_RATIO, duty);
        self.telemetry.set(tk::BOILER_CP_USED_J_KG_K, cp_used);
        self.telemetry.set(tk::SUPPLY_TEMP_C, supply_temp_c);
        self.telemetry.set(tk::RETURN_TEMP_C, return_temp_c);
        let has_nonzero_flow = electric_kw > 0.0 || thermal_output_w != 0.0;
        self.operating_mode = self
            .operating_mode
            .resolve_idle(has_nonzero_flow, Some(thermal_output_w));
        self.telemetry
            .set(tk::OPERATING_MODE, self.operating_mode.as_code());
        let sp = self.hvac.effective_setpoints();
        // Heating-only equipment: setpoint_c is always the heating setpoint (per
        // validate_core_contract requirement that HAS_SETPOINT => setpoint_c is Some).
        let active_setpoint_c = sp.heating_c;
        write_ebm_telemetry(ebm_window, thermal_output_w, &mut self.telemetry)?;
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

    fn thermostat_band_class(&self) -> Option<hares_types::ThermostatBandClass> {
        Some(self.hvac.thermostat_fsm.thermostat.band_class)
    }

    fn thermostat_axes(&self) -> Option<hares_types::ThermostatAxes> {
        Some(hares_types::ThermostatAxes::One(
            hares_types::ThermostatAxis::Heating,
        ))
    }

    fn resolved_zip(&self) -> Option<hares_types::zip::ResolvedZip> {
        Some(self.zip)
    }

    fn take_health_counts(&mut self) -> EquipmentHealthCounts {
        EquipmentHealthCounts {
            curve_index_clamps: 0,
            fluid_payload_absences: std::mem::take(&mut self.fluid_payload_absences),
        }
    }

    fn drain_warnings(&mut self, out: &mut Vec<Warning>) {
        out.append(&mut self.warnings);
    }

    fn save_state(&self) -> crate::Result<Vec<u8>> {
        try_save_versioned(
            &BoilerState {
                mode: self.hvac.thermostat_fsm.mode,
                duty_cycle: self.hvac.runtime.duty_cycle,
                last_mode_switch_at: self.hvac.thermostat_fsm.last_mode_switch_at,
                runtime_setpoints: self.hvac.thermostat_fsm.runtime_setpoints,
                thermostat_hysteresis_c: self.hvac.thermostat_fsm.thermostat.hysteresis_c,
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
                fluid_payload_seen: self.fluid_payload_seen,
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
        self.hvac
            .thermostat_fsm
            .restore_hysteresis(decoded.thermostat_hysteresis_c)?;
        self.operating_mode = decoded.operating_mode;
        self.run_time_s = decoded.run_time_s;
        self.mode_override = decoded.mode_override;
        self.dr_level = decoded.dr_level;
        self.fluid_payload_seen = decoded.fluid_payload_seen;

        self.telemetry.insert(tk::ELECTRIC_KW, decoded.electric_kw);
        self.telemetry
            .insert(tk::THERMAL_OUTPUT_W, decoded.thermal_output_w);
        self.telemetry
            .insert(tk::SUPPLY_TEMP_C, decoded.supply_temp_c);
        self.telemetry
            .insert(tk::RETURN_TEMP_C, decoded.return_temp_c);
        self.telemetry
            .insert(tk::OPERATING_MODE, decoded.operating_mode.as_code());
        // Rebuild the restore-instant output from the persisted step
        // values — mirroring the step's own literal: the flows from the
        // checkpointed `electric_kw`/`thermal_output_w`, the mode from the
        // checkpointed `operating_mode` (the pair the step published was
        // guard-valid, and the checkpoint carries both faces), the
        // setpoint from the restored thermostat state (the same
        // `effective_setpoints` the step reads, now reflecting the
        // restored `runtime_setpoints`). The reactive ZIP power is not
        // checkpointed and publishes as 0. `CoreOutput::default()` (all
        // fields `None`) violates the presence rules for every declared
        // capability (ELECTRIC/REACTIVE/HAS_MODE/THERMAL/HAS_SETPOINT) —
        // the restore contract is that a checkpoint restores a state
        // `validate_core_contract` accepts.
        let active_setpoint_c = self.hvac.effective_setpoints().heating_c;
        self.core_output = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(decoded.electric_kw.max(0.0))),
                reactive_power_kvar: Some(0.0),
                fuel_w: None,
                thermal_output_w: Some(decoded.thermal_output_w),
                sensible_cooling_w: None,
                latent_cooling_w: None,
            },
            state: CoreState {
                operating_mode: Some(decoded.operating_mode),
                soc: None,
                speed_index: None,
                setpoint_c: Some(active_setpoint_c),
            },
            performance: CorePerformance::default(),
        };
        Ok(())
    }

    fn apply_signal(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        if apply_simple_mode_override_and_dr(
            &mut self.mode_override,
            &mut self.dr_level,
            signal,
            "Electric Boiler",
        )? {
            return Ok(());
        }
        self.hvac.apply_control_signal(signal)?;
        apply_simple_heating_ideal_capacity_control(&mut self.hvac, signal, self.rated_capacity_w);
        Ok(())
    }

    fn ideal_target(&self) -> Option<(hares_types::ZoneId, f64)> {
        if !self.use_ideal {
            return None;
        }
        let setpoint = self.hvac.effective_setpoints().heating_c;
        self.hvac.config.zone_id.map(|zone| (zone, setpoint))
    }

    fn resolved_parameters(&self) -> Vec<hares_types::ResolvedParameter> {
        vec![
            hares_types::ResolvedParameter::new(
                "rated_capacity_w",
                self.rated_capacity_w,
                hares_types::Unit::Watt,
            ),
            hares_types::ResolvedParameter::new(
                "efficiency",
                1.0 / self.eir,
                hares_types::Unit::Fraction,
            ),
            hares_types::ResolvedParameter::new(
                "rated_eir",
                self.eir,
                hares_types::Unit::Dimensionless,
            ),
        ]
    }
}

impl GasBoiler {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let zone = zone_id_from_config(&config);
        let loop_id = loop_id_from_config(&config, &["loop_id", "hydronic_loop_id"]);
        let descriptor = EquipmentDescriptor {
            id: EquipmentId(constructor_equipment_id(&config)),
            name: config.name,
            end_use: EndUse::HVAC_HEATING,
            equipment_type: Cow::Borrowed("Gas Boiler"),
            zone,
            fuel: FuelType::Gas,
            stage: ExecutionStage::Thermal,
            control_capabilities: ControlCapabilities::THERMAL_SETPOINT
                | ControlCapabilities::THERMAL_SETPOINT_DELTA
                | ControlCapabilities::IDEAL_CAPACITY
                | ControlCapabilities::MODE_OVERRIDE
                | ControlCapabilities::DEMAND_RESPONSE
                | ControlCapabilities::NON_HVAC_ZONE_INPUT,
            core_capabilities: CoreCapabilities::ELECTRIC
                | CoreCapabilities::FUEL
                | CoreCapabilities::REACTIVE
                | CoreCapabilities::HAS_MODE
                | CoreCapabilities::THERMAL
                | CoreCapabilities::HAS_SETPOINT,
            telemetry_fields: gas_boiler_telemetry_fields(),
            zone_type: None,
        };

        Self {
            descriptor,
            ports: served_zone_ports(
                &[PortDeclaration::fuel(), PortDeclaration::electrical()],
                zone,
                false,
            )
            .into_iter()
            .chain([declared_fluid_port(loop_id)])
            .collect(),
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
            mode_override: None,
            dr_level: DRLevel::Normal,
            fluid_payload_seen: false,
            fluid_payload_absences: 0,
            fluid_absence_warned: false,
            warnings: Vec::new(),
            zip: hares_types::zip::ResolvedZip::reactive_only(
                hares_types::zip::ZipLoad::constant_power(),
            ),
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
    fn checkpoint_version() -> u32 {
        BOILER_CHECKPOINT_VERSION
    }

    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn rename(&mut self, name: String) {
        self.descriptor.name = name;
    }

    fn set_equipment_id(&mut self, id: EquipmentId) -> crate::Result<()> {
        crate::apply_identity_write(self.is_initialized(), &mut self.descriptor, id)
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        let typed = config.require_typed::<GasBoilerConfig>("Gas Boiler")?;
        self.loop_id = Some(require_loop_id(typed.loop_id, "Gas Boiler", &config.name)?);
        self.hvac.init(config, env)?;
        self.zip = crate::config::resolve_reactive_zip(config)?;
        typed.validate()?;
        self.rated_capacity_w = typed.capacity_w.max(0.0);
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
        self.hvac.config.eir_by_stage = vec![self.eir_max];
        self.hvac.config.heating_capacities_w = vec![self.rated_capacity_w];
        self.hvac.update_zone_heat_fractions()?;
        self.hvac.rebuild_thermal_ports(&mut self.ports, false);
        self.descriptor.zone = self.hvac.config.zone_id;
        // The fluid port's declared loop id and fluid type follow the
        // configuration: rebuild_thermal_ports pushes the thermal ports to
        // the end of the vector, so the fluid port is located by type, not
        // by index. A glycol boiler declares a glycol loop, and a shared
        // loop with a water port is the conflicting-type construction error.
        for port in &mut self.ports {
            if port.port_type == hares_types::PortType::Fluid {
                port.loop_id = self.loop_id;
                port.fluid_type = Some(self.fluid_type);
            }
        }
        self.operating_mode = OperatingMode::Off;
        self.run_time_s = 0.0;
        self.fluid_payload_seen = false;
        self.fluid_payload_absences = 0;
        self.fluid_absence_warned = false;
        self.warnings.clear();
        self.telemetry = gas_boiler_default_telemetry();
        register_ebm_telemetry_keys(&mut self.telemetry);
        self.core_output = CoreOutput::default();
        Ok(())
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        self.use_ideal = self.hvac.use_ideal_capacity(env);
        // Grid outage: a de-energized bus removes supply power for the
        // elements/burner controls and the blower/circulator, so the unit
        // cannot run (and cannot deliver heat) regardless of thermostat
        // calls or overrides. Gated before override handling; islanded
        // (battery/generator-backed) homes keep an energized bus and are
        // not affected. See docs/outage-behavior.md.
        if outage_forces_off(&mut self.hvac, env) {
            self.operating_mode = OperatingMode::Off;
            return OperatingMode::Off;
        }
        if let Some(mode) = apply_simple_mode_override_in_control(
            &mut self.hvac,
            &mut self.mode_override,
            self.dr_level,
            "Gas Boiler",
        ) {
            self.operating_mode = mode;
            return mode;
        }
        self.operating_mode =
            match update_heating_control(&mut self.hvac, env, &self.descriptor.name) {
                Ok(mode) => mode,
                Err(err) => {
                    self.hvac.record_control_error(err);
                    OperatingMode::Off
                }
            };
        self.operating_mode
    }

    fn step(
        &mut self,
        env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        if let Some(err) = self.hvac.take_control_error() {
            return Err(err);
        }
        let plr = self.hvac.runtime.duty_cycle.clamp(0.0, 1.0);
        let sf = self.hvac.config.space_fraction;
        let thermal_output_w = self.rated_capacity_w * plr * sf;
        let loop_id = bound_loop_id(self.loop_id, "Gas Boiler", &self.descriptor.name)?;
        let payload_was_seen = self.fluid_payload_seen;
        let return_temp_read = loop_return_temp_c(
            env,
            loop_id,
            "Gas Boiler",
            &self.descriptor.name,
            &mut self.fluid_payload_seen,
        )?;
        let return_temp_c = match return_temp_read {
            Some(temp) => temp,
            None => {
                // The fluid domain published no payload: the configured
                // return temperature covers the step. Before the first
                // publish that is the escape's whole contract (the first
                // step runs before the envelope's first resolve); after
                // it, the absence is a degradation: counted in the run's
                // health, and warned once per run with the detail (the
                // once-per-run pattern keeps a payload starved for the
                // rest of a run from flooding the warning log and the
                // tracing with a line per step).
                if payload_was_seen {
                    self.fluid_payload_absences += 1;
                    if !self.fluid_absence_warned {
                        self.fluid_absence_warned = true;
                        let message = format!(
                            "Gas Boiler '{}': the fluid domain published no payload \
                             this step; the return temperature of loop {loop_id:?} reads \
                             from the configured return temperature until the fluid \
                             domain publishes again; further absent-payload steps are \
                             counted silently in the run health's fluid_payload_absences",
                            self.descriptor.name
                        );
                        tracing::warn!(
                            equipment = %self.descriptor.name,
                            loop_id = ?loop_id,
                            "{message}"
                        );
                        self.warnings
                            .push(Warning::new(self.descriptor.name.clone(), message));
                    }
                }
                self.default_return_temp_c
            }
        };
        // Condensing EIR polynomial uses zone air temperature (OCHRE HVAC.py:651:
        // t_in = self.zone.temperature), not return water temperature.
        let zone_temp_c = lookup_zone(env, self.hvac.config.served_zone()?)?.temperature_c;
        let eir = if thermal_output_w > 0.0 {
            self.current_eir(plr, zone_temp_c)?
        } else {
            self.eir_max
        };
        let fuel_input_w = thermal_output_w * eir;

        let cp_used = cp_j_kg_k(self.fluid_type);
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
        let ebm_window = step_equivalent_battery(&self.hvac, env)?;

        if fuel_input_w > 0.0 {
            ports.accumulate(&PortContribution::Fuel {
                fuel_type: self.fuel_type,
                consumption_w: fuel_input_w,
            })?;
        }
        let reactive_power_kvar = self
            .zip
            .reactive_kvar(electric_kw, env.grid.bus_voltage_pu());
        if electric_kw > 0.0 {
            ports.accumulate(&PortContribution::Electrical {
                active_power_w: power_kw_to_w(electric_kw),
                reactive_power_kvar,
            })?;
        }
        if thermal_output_w > 0.0 {
            ports.accumulate(&PortContribution::Fluid {
                loop_id,
                flow_rate_kg_s: self.flow_rate_kg_s,
                supply_temp_c,
                return_temp_c,
                fluid_type: self.fluid_type,
                thermal_power_w: None,
                node_id: FluidNodeId(0),
                direction: HeatTransferDirection::Source,
            })?;
            self.hvac.write_zone_thermal_contributions(
                ports,
                thermal_output_w,
                0.0,
                ThermalCategory::HvacHeating,
            )?;
        }

        // Jacket loss: fuel energy minus useful thermal output, plus pump electrical.
        // For condensing boilers (EIR < 1), jacket loss is zero -- the extra output
        // comes from latent heat recovery, not from the room.
        let jacket_loss_w = (fuel_input_w + electric_kw * 1e3 - thermal_output_w).max(0.0);
        if let Some(zone) = self.descriptor.zone
            && jacket_loss_w > 0.0
        {
            ports.accumulate(&PortContribution::Thermal {
                zone,
                sensible_gain_w: jacket_loss_w,
                radiant_gain_w: 0.0,
                latent_gain_w: 0.0,
                category: ThermalCategory::JacketLoss,
            })?;
        }
        if thermal_output_w > 0.0 {
            self.run_time_s += dt.as_secs_f64();
        }

        self.telemetry.set(tk::ELECTRIC_KW, electric_kw);
        self.telemetry
            .set(tk::REACTIVE_POWER_KVAR, reactive_power_kvar);
        self.telemetry.set(tk::FUEL_INPUT_W, fuel_input_w);
        self.telemetry.set(tk::THERMAL_OUTPUT_W, thermal_output_w);
        self.telemetry.set(tk::JACKET_LOSS_W, jacket_loss_w);
        self.telemetry.set(tk::EIR, eir);
        // The runtime fraction and the part-load ratio coincide for a fuel
        // boiler: it delivers `plr` of rated for the whole step and burns for
        // that fraction (EnergyPlus HeatingCoils.cc:1873).
        self.telemetry.set(tk::RUNTIME_FRACTION, plr);
        self.telemetry.set(tk::PART_LOAD_RATIO, plr);
        self.telemetry.set(tk::BOILER_CP_USED_J_KG_K, cp_used);
        self.telemetry.set(tk::SUPPLY_TEMP_C, supply_temp_c);
        self.telemetry.set(tk::RETURN_TEMP_C, return_temp_c);
        let has_nonzero_flow = electric_kw > 0.0 || fuel_input_w > 0.0 || thermal_output_w != 0.0;
        self.operating_mode = self
            .operating_mode
            .resolve_idle(has_nonzero_flow, Some(thermal_output_w));
        self.telemetry
            .set(tk::OPERATING_MODE, self.operating_mode.as_code());
        let sp = self.hvac.effective_setpoints();
        // Heating-only equipment: setpoint_c is always the heating setpoint (per
        // validate_core_contract requirement that HAS_SETPOINT => setpoint_c is Some).
        let active_setpoint_c = sp.heating_c;
        write_ebm_telemetry(ebm_window, thermal_output_w, &mut self.telemetry)?;
        self.core_output = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(electric_kw.max(0.0))),
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

    fn thermostat_band_class(&self) -> Option<hares_types::ThermostatBandClass> {
        Some(self.hvac.thermostat_fsm.thermostat.band_class)
    }

    fn thermostat_axes(&self) -> Option<hares_types::ThermostatAxes> {
        Some(hares_types::ThermostatAxes::One(
            hares_types::ThermostatAxis::Heating,
        ))
    }

    fn resolved_zip(&self) -> Option<hares_types::zip::ResolvedZip> {
        Some(self.zip)
    }

    fn take_health_counts(&mut self) -> EquipmentHealthCounts {
        EquipmentHealthCounts {
            curve_index_clamps: 0,
            fluid_payload_absences: std::mem::take(&mut self.fluid_payload_absences),
        }
    }

    fn drain_warnings(&mut self, out: &mut Vec<Warning>) {
        out.append(&mut self.warnings);
    }

    fn save_state(&self) -> crate::Result<Vec<u8>> {
        try_save_versioned(
            &BoilerState {
                mode: self.hvac.thermostat_fsm.mode,
                duty_cycle: self.hvac.runtime.duty_cycle,
                last_mode_switch_at: self.hvac.thermostat_fsm.last_mode_switch_at,
                runtime_setpoints: self.hvac.thermostat_fsm.runtime_setpoints,
                thermostat_hysteresis_c: self.hvac.thermostat_fsm.thermostat.hysteresis_c,
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
                fluid_payload_seen: self.fluid_payload_seen,
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
        self.hvac
            .thermostat_fsm
            .restore_hysteresis(decoded.thermostat_hysteresis_c)?;
        self.operating_mode = decoded.operating_mode;
        self.run_time_s = decoded.run_time_s;
        self.mode_override = decoded.mode_override;
        self.dr_level = decoded.dr_level;
        self.fluid_payload_seen = decoded.fluid_payload_seen;

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
        self.telemetry
            .insert(tk::OPERATING_MODE, decoded.operating_mode.as_code());
        // Rebuild the restore-instant output from the persisted step
        // values — mirroring the gas step's own literal (fuel arm
        // included): the flows from the checkpointed
        // `electric_kw`/`thermal_output_w`/`fuel_input_w`, the mode from
        // the checkpointed `operating_mode`, the setpoint from the
        // restored thermostat state. The reactive ZIP power is not
        // checkpointed and publishes as 0. Same restore contract as the
        // electric boiler — `CoreOutput::default()` violates the
        // presence rules for every declared capability
        // (ELECTRIC/FUEL/REACTIVE/HAS_MODE/THERMAL/HAS_SETPOINT).
        let active_setpoint_c = self.hvac.effective_setpoints().heating_c;
        self.core_output = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(decoded.electric_kw.max(0.0))),
                reactive_power_kvar: Some(0.0),
                fuel_w: Some(FuelPower {
                    fuel_type: self.fuel_type,
                    consumption_w: decoded.fuel_input_w.max(0.0),
                }),
                thermal_output_w: Some(decoded.thermal_output_w),
                sensible_cooling_w: None,
                latent_cooling_w: None,
            },
            state: CoreState {
                operating_mode: Some(decoded.operating_mode),
                soc: None,
                speed_index: None,
                setpoint_c: Some(active_setpoint_c),
            },
            performance: CorePerformance::default(),
        };
        Ok(())
    }

    fn apply_signal(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        if apply_simple_mode_override_and_dr(
            &mut self.mode_override,
            &mut self.dr_level,
            signal,
            "Gas Boiler",
        )? {
            return Ok(());
        }
        self.hvac.apply_control_signal(signal)?;
        apply_simple_heating_ideal_capacity_control(&mut self.hvac, signal, self.rated_capacity_w);
        Ok(())
    }

    fn ideal_target(&self) -> Option<(hares_types::ZoneId, f64)> {
        if !self.use_ideal {
            return None;
        }
        let setpoint = self.hvac.effective_setpoints().heating_c;
        self.hvac.config.zone_id.map(|zone| (zone, setpoint))
    }

    fn resolved_parameters(&self) -> Vec<hares_types::ResolvedParameter> {
        vec![
            hares_types::ResolvedParameter::new(
                "rated_capacity_w",
                self.rated_capacity_w,
                hares_types::Unit::Watt,
            ),
            // The stored form is the EIR (1/AFUE); both faces of the same
            // resolved parameter, reported so a consumer reads either.
            hares_types::ResolvedParameter::new(
                "afue",
                1.0 / self.eir_max,
                hares_types::Unit::Fraction,
            ),
            hares_types::ResolvedParameter::new(
                "rated_eir",
                self.eir_max,
                hares_types::Unit::Dimensionless,
            ),
        ]
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

/// A boiler's water-side port as `new` declares it: bound to the config's
/// loop id when the config carries one, and unbound otherwise until `init`
/// binds it or fails.
fn declared_fluid_port(loop_id: Option<LoopId>) -> PortDeclaration {
    PortDeclaration {
        port_type: hares_types::PortType::Fluid,
        zone: None,
        loop_id,
        domain_id: None,
        fluid_type: Some(FluidType::Water),
        fluid_node_id: None,
    }
}

/// The loop `init` bound; stepping a boiler whose init did not bind one is
/// an error rather than a step on a stand-in loop.
fn bound_loop_id(loop_id: Option<LoopId>, kind: &str, name: &str) -> crate::Result<LoopId> {
    loop_id.ok_or_else(|| {
        HaresError::Equipment(format!(
            "{kind} '{name}' stepped with no fluid loop bound: init did not run or failed"
        ))
    })
}

/// The bound loop's mean return temperature from the fluid domain's payload.
///
/// `payload_seen` tracks whether the fluid domain has published a payload
/// this run and is set to true the moment a payload is present. A payload
/// that does not decode, or a decoded payload that does not carry the bound
/// loop, is an error naming the boiler and the loop: corrupt fluid state is
/// not the absence of fluid state. The absence itself is the caller's
/// `None`: silent before the first publish (the configured return
/// temperature covers the first step, before the envelope's first resolve),
/// a logged and counted degradation after it.
fn loop_return_temp_c(
    env: &EnvironmentState,
    loop_id: LoopId,
    kind: &str,
    name: &str,
    payload_seen: &mut bool,
) -> crate::Result<Option<f64>> {
    let Some(payload) = env
        .domains
        .fluid
        .get()
        .and_then(|d| d.custom_payload.as_ref())
    else {
        return Ok(None);
    };
    *payload_seen = true;
    let states = FluidDomainPayload::decode(payload).map_err(|err| {
        HaresError::Equipment(format!(
            "{kind} '{name}': the fluid domain's payload does not decode, so the \
             return temperature of loop {loop_id:?} cannot be read: {err}; corrupt \
             fluid state is not the absence of fluid state"
        ))
    })?;
    states
        .into_iter()
        .find(|state| state.loop_id == loop_id)
        .map(|state| Some(state.mean_return_temp_c))
        .ok_or_else(|| {
            HaresError::Equipment(format!(
                "{kind} '{name}': the decoded fluid state carries no loop \
                 {loop_id:?}; the fluid domain must carry a state for every \
                 declared loop"
            ))
        })
}

fn electric_boiler_default_telemetry() -> Telemetry {
    let mut telemetry = Telemetry::with_capacity(10);
    telemetry.insert(tk::ELECTRIC_KW, 0.0);
    telemetry.insert(tk::REACTIVE_POWER_KVAR, 0.0);
    telemetry.insert(tk::PUMP_POWER_KW, 0.0);
    telemetry.insert(tk::THERMAL_OUTPUT_W, 0.0);
    telemetry.insert(tk::RUNTIME_FRACTION, 0.0);
    telemetry.insert(tk::PART_LOAD_RATIO, 0.0);
    telemetry.insert(tk::BOILER_CP_USED_J_KG_K, cp_j_kg_k(FluidType::Water));
    telemetry.insert(tk::SUPPLY_TEMP_C, 0.0);
    telemetry.insert(tk::RETURN_TEMP_C, 0.0);
    telemetry.insert(tk::OPERATING_MODE, 0.0);
    telemetry.insert(tk::HEATING_SETPOINT_C, 0.0);
    telemetry.insert(tk::COOLING_SETPOINT_C, 0.0);
    telemetry
}

fn gas_boiler_default_telemetry() -> Telemetry {
    let mut telemetry = Telemetry::with_capacity(12);
    telemetry.insert(tk::ELECTRIC_KW, 0.0);
    telemetry.insert(tk::REACTIVE_POWER_KVAR, 0.0);
    telemetry.insert(tk::FUEL_INPUT_W, 0.0);
    telemetry.insert(tk::THERMAL_OUTPUT_W, 0.0);
    telemetry.insert(tk::RUNTIME_FRACTION, 0.0);
    telemetry.insert(tk::PART_LOAD_RATIO, 0.0);
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
        TelemetryField::temp_c(
            tk::HEATING_SETPOINT_C,
            "Active heating setpoint from thermostat schedule",
        ),
        TelemetryField::temp_c(
            tk::COOLING_SETPOINT_C,
            "Active cooling setpoint from thermostat schedule",
        ),
    ]
}

fn electric_boiler_telemetry_fields() -> Vec<TelemetryField> {
    let mut fields = vec![
        TelemetryField::power_kw(
            tk::ELECTRIC_KW,
            "Electric boiler active power draw (element + circulation pump)",
        ),
        TelemetryField::power_kvar(
            tk::REACTIVE_POWER_KVAR,
            "Reactive power (positive = inductive/lagging)",
        ),
        TelemetryField::power_kw(tk::PUMP_POWER_KW, "Hydronic circulation pump electric draw"),
        TelemetryField::ratio(
            tk::RUNTIME_FRACTION,
            "Fraction of the step the element runs (equals the part-load ratio)",
        ),
        TelemetryField::ratio(
            tk::PART_LOAD_RATIO,
            "Delivered capacity as a fraction of rated",
        ),
        TelemetryField::power_w(
            tk::THERMAL_OUTPUT_W,
            "Thermal output transferred to hydronic loop",
        ),
        TelemetryField::flow_mean(
            tk::BOILER_CP_USED_J_KG_K,
            hares_types::Unit::JoulePerKilogramKelvin,
            "Effective specific heat used in supply temperature calculation",
        ),
        TelemetryField::temp_c(tk::SUPPLY_TEMP_C, "Fluid supply temperature"),
        TelemetryField::temp_c(tk::RETURN_TEMP_C, "Fluid return temperature"),
        TelemetryField::code_code(
            tk::OPERATING_MODE,
            hares_types::MODE_LABELS,
            "Operating mode code: 0=Off, 1=Heating",
        ),
        TelemetryField::ratio(tk::EBM_EFFICIENCY, "EBM efficiency (COP = 1/EIR)"),
        TelemetryField::power_kw(
            tk::EBM_BASELINE_POWER_KW,
            "EBM baseline power to hold setpoint",
        ),
        TelemetryField::stock_kwh(tk::EBM_ENERGY_KWH, "EBM current energy state"),
        TelemetryField::stock_kwh(
            tk::EBM_MIN_ENERGY_KWH,
            "EBM minimum energy at turn-on threshold",
        ),
        TelemetryField::stock_kwh(
            tk::EBM_MAX_ENERGY_KWH,
            "EBM maximum energy at turn-off threshold",
        ),
        TelemetryField::power_kw(tk::EBM_MAX_POWER_KW, "EBM maximum electrical power"),
    ];
    fields.extend(setpoint_telemetry_fields());
    fields
}

fn gas_boiler_telemetry_fields() -> Vec<TelemetryField> {
    let mut fields = vec![
        TelemetryField::power_kw(
            tk::ELECTRIC_KW,
            "Gas boiler auxiliary electrical draw (pump/fan)",
        ),
        TelemetryField::power_kvar(
            tk::REACTIVE_POWER_KVAR,
            "Reactive power (positive = inductive/lagging)",
        ),
        TelemetryField::power_w(tk::FUEL_INPUT_W, "Gas boiler fuel input power"),
        TelemetryField::ratio(
            tk::RUNTIME_FRACTION,
            "Fraction of the step the burner runs (equals the part-load ratio)",
        ),
        TelemetryField::ratio(
            tk::PART_LOAD_RATIO,
            "Delivered capacity as a fraction of rated",
        ),
        TelemetryField::power_w(
            tk::THERMAL_OUTPUT_W,
            "Thermal output transferred to hydronic loop",
        ),
        TelemetryField::power_w(
            tk::JACKET_LOSS_W,
            "Jacket heat loss to zone (fuel_input - thermal_output)",
        ),
        TelemetryField::ratio(
            tk::EIR,
            "Instantaneous energy-input ratio after polynomial adjustment",
        ),
        TelemetryField::flow_mean(
            tk::BOILER_CP_USED_J_KG_K,
            hares_types::Unit::JoulePerKilogramKelvin,
            "Effective specific heat used in supply temperature calculation",
        ),
        TelemetryField::temp_c(tk::SUPPLY_TEMP_C, "Fluid supply temperature"),
        TelemetryField::temp_c(tk::RETURN_TEMP_C, "Fluid return temperature"),
        TelemetryField::code_code(
            tk::OPERATING_MODE,
            hares_types::MODE_LABELS,
            "Operating mode code: 0=Off, 1=Heating",
        ),
        TelemetryField::ratio(tk::EBM_EFFICIENCY, "EBM efficiency (COP = 1/EIR)"),
        TelemetryField::power_kw(
            tk::EBM_BASELINE_POWER_KW,
            "EBM baseline power to hold setpoint",
        ),
        TelemetryField::stock_kwh(tk::EBM_ENERGY_KWH, "EBM current energy state"),
        TelemetryField::stock_kwh(
            tk::EBM_MIN_ENERGY_KWH,
            "EBM minimum energy at turn-on threshold",
        ),
        TelemetryField::stock_kwh(
            tk::EBM_MAX_ENERGY_KWH,
            "EBM maximum energy at turn-off threshold",
        ),
        TelemetryField::power_kw(tk::EBM_MAX_POWER_KW, "EBM maximum electrical power"),
    ];
    fields.extend(setpoint_telemetry_fields());
    fields
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_physics::constants::cp_j_kg_k;
    use hares_types::{
        ControlSignal, CoreCapabilities, DRLevel, DomainUpdate, EnvironmentState, ExecutionStage,
        FLUID, FluidDomainPayload, FluidLoopState, FluidType, GridState, LoopId, OperatingMode,
        PortSlots, ThermalAccumulator, WeatherState, ZoneId, ZoneState, telemetry_keys as tk,
    };

    use super::{
        DEFAULT_CONDENSING_EIR_COEFFS, DEFAULT_NON_CONDENSING_EIR_COEFFS, DEFAULT_RETURN_TEMP_C,
        ElectricBoiler, GasBoiler,
    };

    /// The per-fluid cp table must be positive and finite for every fluid
    /// type (exhaustive match: a fluid type without a cp cannot compile).
    /// Replaced the gated per-step asserts in both boiler step paths.
    #[test]
    fn cp_table_positive_and_finite_for_every_fluid_type() {
        for fluid_type in [FluidType::Water, FluidType::Glycol, FluidType::Refrigerant] {
            let cp = cp_j_kg_k(fluid_type);
            assert!(
                cp > 0.0 && cp.is_finite(),
                "{fluid_type:?}: cp must be positive and finite, got {cp}"
            );
        }
    }
    use crate::hvac::heating_config::{ElectricBoilerConfig, GasBoilerConfig};
    use crate::{Equipment, EquipmentConfig, EquipmentRegistry};

    fn env(zone_temp_c: f64) -> EnvironmentState {
        EnvironmentState {
            ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
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
                island_bus_voltage_pu: None,
            },
            schedule_row: None,
            domains: hares_types::DomainSlots::default(),
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
        .unwrap()
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
        .unwrap()
    }

    /// A boiler built with no loop id is a construction error naming the
    /// boiler: every resolved boiler carries the loop id its wiring or loop
    /// allocation assigned, so a `None` reached init only if that channel
    /// dropped it.
    #[test]
    fn boiler_without_loop_id_is_a_construction_error() {
        let env = env(18.0);

        let electric_cfg = EquipmentConfig::from_typed(
            "Electric Boiler".to_string(),
            "Electric Boiler".to_string(),
            ElectricBoilerConfig {
                zone_id: Some(1),
                loop_id: None,
                eir: 1.0,
                capacity_w: 5_000.0,
                ..ElectricBoilerConfig::default()
            },
        )
        .expect("the config builds");
        let mut electric = ElectricBoiler::new(electric_cfg.clone());
        let electric_err = electric
            .init(&electric_cfg, &env)
            .expect_err("a boiler with no loop id fails init");
        assert!(
            electric_err.to_string().contains("Electric Boiler"),
            "the error names the boiler: {electric_err}"
        );

        let gas_cfg = EquipmentConfig::from_typed(
            "Gas Boiler".to_string(),
            "Gas Boiler".to_string(),
            GasBoilerConfig {
                zone_id: Some(1),
                loop_id: None,
                afue: 0.85,
                capacity_w: 10_000.0,
                ..GasBoilerConfig::default()
            },
        )
        .expect("the config builds");
        let mut gas = GasBoiler::new(gas_cfg.clone());
        let gas_err = gas
            .init(&gas_cfg, &env)
            .expect_err("a boiler with no loop id fails init");
        assert!(
            gas_err.to_string().contains("Gas Boiler"),
            "the error names the boiler: {gas_err}"
        );
    }

    /// Before init, a boiler whose config carries no loop id declares its
    /// fluid port with no loop: no placeholder loop stands in for the
    /// missing id, so nothing can plan or step against a loop the input
    /// never named.
    #[test]
    fn boiler_without_loop_id_declares_no_placeholder_loop() {
        let electric = ElectricBoiler::new(
            EquipmentConfig::from_typed(
                "Electric Boiler".to_string(),
                "Electric Boiler".to_string(),
                ElectricBoilerConfig {
                    zone_id: Some(1),
                    loop_id: None,
                    ..ElectricBoilerConfig::default()
                },
            )
            .expect("the config builds"),
        );
        let gas = GasBoiler::new(
            EquipmentConfig::from_typed(
                "Gas Boiler".to_string(),
                "Gas Boiler".to_string(),
                GasBoilerConfig {
                    zone_id: Some(1),
                    loop_id: None,
                    ..GasBoilerConfig::default()
                },
            )
            .expect("the config builds"),
        );
        for (name, ports) in [("electric", electric.ports()), ("gas", gas.ports())] {
            let fluid: Vec<_> = ports
                .iter()
                .filter(|port| port.port_type == hares_types::PortType::Fluid)
                .collect();
            assert_eq!(fluid.len(), 1, "the {name} boiler declares one fluid port");
            assert_eq!(
                fluid[0].loop_id, None,
                "the {name} boiler's fluid port has no loop before init binds one"
            );
        }
    }

    /// A boiler stepped with no loop bound is an error naming it, never a
    /// step on a stand-in loop.
    #[test]
    fn boiler_stepped_with_no_loop_bound_is_an_error() {
        let env = env(18.0);
        let mut electric = ElectricBoiler::new(
            EquipmentConfig::from_typed(
                "EB".to_string(),
                "Electric Boiler".to_string(),
                ElectricBoilerConfig {
                    zone_id: Some(1),
                    loop_id: None,
                    ..ElectricBoilerConfig::default()
                },
            )
            .expect("the config builds"),
        );
        let mut gas = GasBoiler::new(
            EquipmentConfig::from_typed(
                "GB".to_string(),
                "Gas Boiler".to_string(),
                GasBoilerConfig {
                    zone_id: Some(1),
                    loop_id: None,
                    ..GasBoilerConfig::default()
                },
            )
            .expect("the config builds"),
        );
        let errors = [
            electric
                .step(&env, Duration::from_secs(60), &mut PortSlots::default())
                .expect_err("an electric boiler with no loop cannot step"),
            gas.step(&env, Duration::from_secs(60), &mut PortSlots::default())
                .expect_err("a gas boiler with no loop cannot step"),
        ];
        for (err, name) in errors.iter().zip(["EB", "GB"]) {
            let msg = err.to_string();
            assert!(
                msg.contains(name) && msg.contains("no fluid loop"),
                "the error names the boiler and the missing loop, got: {msg}"
            );
        }
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

    /// Grid outage (de-energized bus): boilers cannot run — elements/burner
    /// controls and the circulation pump are electric, so no heat enters the
    /// hydronic loop and (for gas) no fuel is burned. Islanded homes keep an
    /// energized bus and keep heating; control resumes on restoration.
    #[test]
    fn grid_outage_forces_boilers_off_and_islanded_home_keeps_heating() {
        // Electric boiler.
        let cfg = eb_config(8_000.0, 1.0);
        let mut eb = ElectricBoiler::new(cfg.clone());
        let env_nominal = env(18.0);
        eb.init(&cfg, &env_nominal).unwrap();

        let mut env_outage = env(18.0);
        env_outage.grid.voltage_pu = 0.0;
        assert_eq!(eb.update_control(&env_outage), OperatingMode::Off);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..PortSlots::default()
        };
        eb.step(&env_outage, Duration::from_secs(60), &mut ports)
            .unwrap();
        assert_eq!(ports.electrical.net_active_w(), 0.0);
        assert_eq!(ports.fluid[0].total_flow_kg_s, 0.0);

        // Islanded: bus energized by a backup source → heating proceeds.
        let mut env_islanded = env(18.0);
        env_islanded.grid.voltage_pu = 0.0;
        env_islanded.grid.island_bus_voltage_pu = Some(1.0);
        env_islanded.current_time += ChronoDuration::minutes(1);
        assert_eq!(eb.update_control(&env_islanded), OperatingMode::Heating);
        ports.zero();
        eb.step(&env_islanded, Duration::from_secs(60), &mut ports)
            .unwrap();
        assert!(ports.electrical.net_active_w() > 0.0);
        assert!(ports.fluid[0].total_flow_kg_s > 0.0);

        // Gas boiler: burner controls + circulator are electric → also off,
        // no fuel burned.
        let gcfg = gb_config(10_000.0, 0.8);
        let mut gb = GasBoiler::new(gcfg.clone());
        gb.init(&gcfg, &env_nominal).unwrap();
        assert_eq!(gb.update_control(&env_outage), OperatingMode::Off);
        ports.zero();
        gb.step(&env_outage, Duration::from_secs(60), &mut ports)
            .unwrap();
        assert_eq!(ports.fuel.get(hares_types::FuelType::Gas), 0.0);
        assert_eq!(ports.electrical.net_active_w(), 0.0);
        assert_eq!(ports.fluid[0].total_flow_kg_s, 0.0);

        // Restoration: gas boiler resumes.
        let mut env_restored = env(18.0);
        env_restored.current_time += ChronoDuration::minutes(2);
        assert_eq!(gb.update_control(&env_restored), OperatingMode::Heating);
        ports.zero();
        gb.step(&env_restored, Duration::from_secs(60), &mut ports)
            .unwrap();
        assert!(ports.fuel.get(hares_types::FuelType::Gas) > 0.0);
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

        env.domains.fluid.set_from(&DomainUpdate {
            domain_id: FLUID,
            zone_temperatures_c: vec![],
            custom_payload: FluidDomainPayload::encode(&[FluidLoopState {
                loop_id: LoopId(1),
                fluid_type: FluidType::Water,
                heating_power_w: 0.0,
                cooling_power_w: 0.0,
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

    /// A fluid payload that does not decode fails the step naming the boiler
    /// and the loop: corrupt fluid state is not the absence of fluid state
    /// (whose first step runs on the configured return temperature).
    #[test]
    fn a_fluid_payload_that_does_not_decode_fails_the_step() {
        let cfg = gb_config(10_000.0, 0.95);
        let mut eq = GasBoiler::new(cfg.clone());
        let mut env = env(18.0);
        eq.init(&cfg, &env).unwrap();
        eq.hvac.runtime.duty_cycle = 0.5;
        eq.hvac.thermostat_fsm.mode = super::ThermostatMode::Heating;

        env.domains.fluid.set_from(&DomainUpdate {
            domain_id: FLUID,
            zone_temperatures_c: vec![],
            // A length that is not a multiple of the 7-wide state row: the
            // decode cannot read it.
            custom_payload: Some(vec![1.0, 0.0, 2.0]),
        });

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..PortSlots::default()
        };
        let err = eq
            .step(&env, Duration::from_secs(60), &mut ports)
            .expect_err("a corrupt fluid payload must fail the step");
        let shown = err.to_string();
        assert!(
            shown.contains("Gas Boiler") && shown.contains("does not decode"),
            "the error names the boiler and the unreadable payload: {shown}"
        );

        // A decoded payload that does not carry the bound loop fails the
        // same way: a bound loop the fluid domain does not carry is a wiring
        // defect, not a step on the configured return temperature.
        env.domains.fluid.set_from(&DomainUpdate {
            domain_id: FLUID,
            zone_temperatures_c: vec![],
            custom_payload: FluidDomainPayload::encode(&[FluidLoopState {
                loop_id: LoopId(2),
                fluid_type: FluidType::Water,
                heating_power_w: 0.0,
                cooling_power_w: 0.0,
                net_power_w: 0.0,
                mean_supply_temp_c: 45.0,
                mean_return_temp_c: 40.0,
            }]),
        });
        let err = eq
            .step(&env, Duration::from_secs(60), &mut ports)
            .expect_err("a missing loop state must fail the step");
        let shown = err.to_string();
        assert!(
            shown.contains("LoopId(1)") && shown.contains("no loop"),
            "the error names the missing loop: {shown}"
        );
    }

    /// The first step's absent payload stays silent: before the fluid
    /// domain's first publish the boiler's configured return temperature
    /// covers the step, with no warning and no health event.
    #[test]
    fn the_first_step_absent_payload_runs_on_the_configured_return_temperature() {
        let cfg = gb_config(10_000.0, 0.95);
        let mut eq = GasBoiler::new(cfg.clone());
        let env = env(18.0);
        eq.init(&cfg, &env).unwrap();
        eq.hvac.runtime.duty_cycle = 0.5;
        eq.hvac.thermostat_fsm.mode = super::ThermostatMode::Heating;

        // No fluid payload at all: the slot was never written.
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports)
            .expect("the first step's absent payload runs on the configured return temp");
        let configured = eq.telemetry().get(tk::RETURN_TEMP_C).unwrap();
        assert_eq!(
            configured, DEFAULT_RETURN_TEMP_C,
            "the absent payload's return temperature is the configured one"
        );

        let mut warnings = Vec::new();
        eq.drain_warnings(&mut warnings);
        assert!(
            warnings.is_empty(),
            "the first step's absence stays silent: {warnings:?}"
        );
        let counts = Equipment::take_health_counts(&mut eq);
        assert_eq!(
            counts.fluid_payload_absences, 0,
            "the first step's absence is not a health event"
        );
    }

    /// A fluid payload that goes absent after the fluid domain has
    /// published once is the established degradation: the step still runs
    /// on the configured return temperature, and the absence is a logged
    /// warning counted in the run's health. An unbounded silent escape
    /// would substitute the configured temperature for any step's missing
    /// payload without a trace.
    #[test]
    fn an_absent_payload_after_the_first_publish_warns_and_counts() {
        let cfg = gb_config(10_000.0, 0.95);
        let mut eq = GasBoiler::new(cfg.clone());
        let mut env = env(18.0);
        eq.init(&cfg, &env).unwrap();
        eq.hvac.runtime.duty_cycle = 0.5;
        eq.hvac.thermostat_fsm.mode = super::ThermostatMode::Heating;

        let payload = DomainUpdate {
            domain_id: FLUID,
            zone_temperatures_c: vec![],
            custom_payload: FluidDomainPayload::encode(&[FluidLoopState {
                loop_id: LoopId(1),
                fluid_type: FluidType::Water,
                heating_power_w: 0.0,
                cooling_power_w: 0.0,
                net_power_w: 0.0,
                mean_supply_temp_c: 45.0,
                mean_return_temp_c: 40.0,
            }]),
        };

        // Step 1: the fluid domain publishes; the return temperature is the
        // payload's.
        env.domains.fluid.set_from(&payload);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports)
            .expect("the published payload decodes");
        assert_eq!(eq.telemetry().get(tk::RETURN_TEMP_C).unwrap(), 40.0);

        // Step 2: the payload goes absent after the publish. The step still
        // runs on the configured return temperature, and the absence is
        // counted.
        env.domains.fluid.clear();
        eq.step(&env, Duration::from_secs(60), &mut ports)
            .expect("the absent payload degrades to the configured return temp");
        assert_eq!(
            eq.telemetry().get(tk::RETURN_TEMP_C).unwrap(),
            DEFAULT_RETURN_TEMP_C,
            "the configured return temperature covers the absent payload"
        );

        let mut warnings = Vec::new();
        eq.drain_warnings(&mut warnings);
        assert_eq!(warnings.len(), 1, "the late absence warns: {warnings:?}");
        assert!(
            warnings[0].message.contains("Gas Boiler 'GB'")
                && warnings[0].message.contains("LoopId(1)"),
            "the warning names the boiler and the loop: {:?}",
            warnings[0].message
        );
        let counts = Equipment::take_health_counts(&mut eq);
        assert_eq!(
            counts.fluid_payload_absences, 1,
            "the late absence is counted in the run's health"
        );

        // The count is take-semantics: a clean step reports none.
        let counts = Equipment::take_health_counts(&mut eq);
        assert_eq!(counts.fluid_payload_absences, 0);
    }

    /// A run whose payload stays absent warns once and counts every
    /// absent step: the first absence carries the detail, the rest are
    /// counted silently in the run's health, so a payload starved for a
    /// year of steps cannot flood the run's warning log with a line per
    /// step.
    #[test]
    fn repeated_absent_payloads_warn_once_and_count_every_step() {
        let cfg = gb_config(10_000.0, 0.95);
        let mut eq = GasBoiler::new(cfg.clone());
        let mut env = env(18.0);
        eq.init(&cfg, &env).unwrap();
        eq.hvac.runtime.duty_cycle = 0.5;
        eq.hvac.thermostat_fsm.mode = super::ThermostatMode::Heating;

        let payload = DomainUpdate {
            domain_id: FLUID,
            zone_temperatures_c: vec![],
            custom_payload: FluidDomainPayload::encode(&[FluidLoopState {
                loop_id: LoopId(1),
                fluid_type: FluidType::Water,
                heating_power_w: 0.0,
                cooling_power_w: 0.0,
                net_power_w: 0.0,
                mean_supply_temp_c: 45.0,
                mean_return_temp_c: 40.0,
            }]),
        };

        // The fluid domain publishes once; the marker flips true.
        env.domains.fluid.set_from(&payload);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports)
            .expect("the published payload decodes");

        // Ten absent-payload steps: every one counted, exactly one warned.
        env.domains.fluid.clear();
        for _ in 0..10 {
            eq.step(&env, Duration::from_secs(60), &mut ports)
                .expect("the absent payload degrades to the configured return temp");
        }
        let counts = Equipment::take_health_counts(&mut eq);
        assert_eq!(
            counts.fluid_payload_absences, 10,
            "every absent-payload step is counted in the run's health"
        );
        let mut warnings = Vec::new();
        eq.drain_warnings(&mut warnings);
        assert_eq!(
            warnings.len(),
            1,
            "the absent payload warns once per run, not once per step: {warnings:?}"
        );
        assert!(
            warnings[0]
                .message
                .contains("counted silently in the run health's fluid_payload_absences"),
            "the warning names where the silent counts surface: {:?}",
            warnings[0].message
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

    /// Checkpoint restore contract (both boiler variants): a restored core
    /// output must satisfy the workspace's own `validate_core_contract`
    /// (presence rules and the mode/flow guard). Pre-fix, the restore
    /// published `CoreOutput::default()` — all fields `None` against the
    /// declared capabilities (electric: ELECTRIC/REACTIVE/HAS_MODE/THERMAL/
    /// HAS_SETPOINT; gas: plus FUEL).
    #[test]
    fn restored_checkpoint_output_satisfies_the_core_contract() {
        // Electric boiler face.
        {
            let cfg = eb_config(10_000.0, 0.80);
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
            eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
            let state = eq.save_state().unwrap();

            let mut restored = ElectricBoiler::new(cfg);
            restored.init(&eb_config(10_000.0, 0.80), &env).unwrap();
            restored.load_state(&state).unwrap();
            hares_types::validate_core_contract(
                restored.descriptor(),
                restored.core_output(),
                &mut Vec::new(),
            )
            .expect("an electric boiler checkpoint must restore a contract-valid output");
        }
        // Gas boiler face (the fuel arm).
        {
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

            let mut restored = GasBoiler::new(cfg);
            restored.init(&gb_config(10_000.0, 0.80), &env).unwrap();
            restored.load_state(&state).unwrap();
            hares_types::validate_core_contract(
                restored.descriptor(),
                restored.core_output(),
                &mut Vec::new(),
            )
            .expect("a gas boiler checkpoint must restore a contract-valid output");
        }
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
            degraded: false,
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
        )
        .unwrap();
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
        )
        .unwrap();

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
        )
        .unwrap();
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
        )
        .unwrap();
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
        )
        .unwrap();
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

    #[test]
    fn electric_boiler_reactive_power_is_some_zero_at_unity_pf() {
        let cfg = eb_config(8_000.0, 1.05);
        let mut eq = ElectricBoiler::new(cfg.clone());
        let env = env(18.0);
        eq.init(&cfg, &env).unwrap();
        assert!(
            eq.descriptor()
                .core_capabilities
                .contains(CoreCapabilities::REACTIVE)
        );
        assert_eq!(eq.zip.pf, 1.0);

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

        assert!(ports.electrical.load_power_w > 0.0);
        assert_eq!(ports.electrical.reactive_power_kvar, 0.0);
        assert_eq!(eq.core_output().flows.reactive_power_kvar, Some(0.0));
        assert_eq!(eq.telemetry().get(tk::REACTIVE_POWER_KVAR), Some(0.0));
        hares_types::validate_core_contract(eq.descriptor(), eq.core_output(), &mut Vec::new())
            .expect("validate_core_contract");
    }

    #[test]
    fn gas_boiler_reactive_power_blended_pf_and_channels_agree() {
        let config = gb_config(20_000.0, 0.8);
        let mut eq = GasBoiler::new(config.clone());
        let mut env = env(18.0);
        eq.init(&config, &env).unwrap();
        eq.pump_kw = 0.1;
        eq.hvac.runtime.duty_cycle = 0.5;
        eq.hvac.thermostat_fsm.mode = super::ThermostatMode::Heating;
        env.domains.fluid.set_from(&DomainUpdate {
            domain_id: FLUID,
            zone_temperatures_c: vec![],
            custom_payload: FluidDomainPayload::encode(&[FluidLoopState {
                loop_id: LoopId(1),
                fluid_type: FluidType::Water,
                heating_power_w: 0.0,
                cooling_power_w: 0.0,
                net_power_w: 0.0,
                mean_supply_temp_c: 45.0,
                mean_return_temp_c: 40.0,
            }]),
        });

        assert!(
            eq.descriptor()
                .core_capabilities
                .contains(CoreCapabilities::REACTIVE)
        );
        let pf = 0.84_f64;
        assert_eq!(eq.zip.pf, pf);

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
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
        hares_types::validate_core_contract(eq.descriptor(), eq.core_output(), &mut Vec::new())
            .expect("validate_core_contract");
    }

    /// Electric boiler circulation-pump draw: `fan_power_w` (same config
    /// surface as the gas boiler) adds a pump draw whenever the boiler
    /// delivers heat, sub-metered via PUMP_POWER_KW, with per-component
    /// reactive power (element pf 1.0 → Q ≡ 0; pump pf 0.84).
    #[test]
    fn electric_boiler_pump_draw_real_and_reactive() {
        const CAPACITY_W: f64 = 8_000.0;
        const EIR: f64 = 1.0;
        const PUMP_W: f64 = 87.0;

        let cfg = EquipmentConfig::from_typed(
            "EB".to_string(),
            "Electric Boiler".to_string(),
            ElectricBoilerConfig {
                zone_id: Some(1),
                loop_id: Some(1),
                eir: EIR,
                capacity_w: CAPACITY_W,
                fan_power_w: Some(PUMP_W),
                ..ElectricBoilerConfig::default()
            },
        )
        .unwrap();
        let cold_env = env(18.0);
        let mut eq = ElectricBoiler::new(cfg.clone());
        eq.init(&cfg, &cold_env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..PortSlots::default()
        };
        eq.update_control(&cold_env);
        eq.step(&cold_env, Duration::from_secs(60), &mut ports)
            .unwrap();

        // Real power: element + pump.
        let expected_w = CAPACITY_W * EIR + PUMP_W;
        let p_w = ports.electrical.net_active_w();
        assert!(
            (p_w - expected_w).abs() < 1e-6,
            "electric boiler draw must include the pump: {p_w} W vs expected {expected_w} W"
        );
        let pump_kw = eq.telemetry().get(tk::PUMP_POWER_KW).unwrap();
        assert!(
            (pump_kw - PUMP_W / 1000.0).abs() < 1e-12,
            "PUMP_POWER_KW sub-meter must report the pump draw; got {pump_kw}"
        );

        // Reactive: element Q ≡ 0 (pf 1.0), pump at the loop-pump component
        // (pf 0.84, reactive polynomial sums to 1.0 at nominal voltage).
        let q = ports.electrical.reactive_power_kvar;
        let expected_q = pump_kw * 0.84_f64.acos().tan();
        assert!(
            (q - expected_q).abs() < 1e-9,
            "Q must come from the pump component alone: {q} vs {expected_q}"
        );
        assert_eq!(
            eq.core_output().flows.reactive_power_kvar.map(f64::to_bits),
            Some(q.to_bits()),
            "CoreOutput Q must match port Q"
        );
        assert_eq!(
            eq.telemetry()
                .get(tk::REACTIVE_POWER_KVAR)
                .map(f64::to_bits),
            Some(q.to_bits()),
            "telemetry Q must match port Q"
        );

        // Off: no thermal output → no pump draw, Q back to zero.
        let warm_env = env(25.0);
        eq.update_control(&warm_env);
        let mut off_ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            fluid: vec![hares_types::FluidAccumulator::new(
                LoopId(1),
                FluidType::Water,
            )],
            ..PortSlots::default()
        };
        eq.step(&warm_env, Duration::from_secs(60), &mut off_ports)
            .unwrap();
        assert_eq!(off_ports.electrical.load_power_w, 0.0);
        assert_eq!(eq.telemetry().get(tk::PUMP_POWER_KW), Some(0.0));
        assert_eq!(off_ports.electrical.reactive_power_kvar, 0.0);
    }

    #[test]
    fn gas_boiler_reactive_power_zero_when_off() {
        let config = gb_config(20_000.0, 0.8);
        let mut eq = GasBoiler::new(config.clone());
        let mut env = env(25.0);
        eq.init(&config, &env).unwrap();
        env.domains.fluid.set_from(&DomainUpdate {
            domain_id: FLUID,
            zone_temperatures_c: vec![],
            custom_payload: FluidDomainPayload::encode(&[FluidLoopState {
                loop_id: LoopId(1),
                fluid_type: FluidType::Water,
                heating_power_w: 0.0,
                cooling_power_w: 0.0,
                net_power_w: 0.0,
                mean_supply_temp_c: 45.0,
                mean_return_temp_c: 40.0,
            }]),
        });

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

        assert_eq!(ports.electrical.load_power_w, 0.0);
        assert_eq!(ports.electrical.reactive_power_kvar, 0.0);
        assert_eq!(
            eq.core_output().flows.reactive_power_kvar,
            Some(0.0),
            "off equipment must report Some(0.0)"
        );
    }

    #[test]
    fn gas_boiler_ebm_efficiency_matches_inverse_eir_max_after_init() {
        // AFUE=0.80 → eir_max = 1/0.80 = 1.25 → EBM COP = 1/1.25 = 0.80.
        // Verifies that GasBoiler::init() populates eir_by_stage with eir_max
        // so the EBM reads the correct EIR, not the empty-vec fallback of 1.0.
        let cfg = gb_config(10_000.0, 0.80);
        let mut eq = GasBoiler::new(cfg.clone());
        let env = env(18.0);
        eq.init(&cfg, &env).unwrap();
        eq.hvac.config.zone_capacitance_kwh_per_k = 2.0;

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

        let expected_eir_max = 1.0 / 0.80;
        let expected_efficiency = 1.0 / expected_eir_max;
        let actual = eq.telemetry().get(tk::EBM_EFFICIENCY).unwrap();
        assert!(
            (actual - expected_efficiency).abs() < 1e-9,
            "GasBoiler EBM efficiency should be 1/eir_max = 1/{expected_eir_max} = {expected_efficiency}, got {actual}"
        );
    }

    #[test]
    fn electric_boiler_ebm_efficiency_matches_inverse_eir_after_init() {
        // EIR=1.05 → EBM COP = 1/1.05.
        // Verifies that ElectricBoiler::init() populates eir_by_stage with
        // the configured EIR so the EBM uses the correct value, not the
        // empty-vec fallback of 1.0.
        let cfg = eb_config(8_000.0, 1.05);
        let mut eq = ElectricBoiler::new(cfg.clone());
        let env = env(18.0);
        eq.init(&cfg, &env).unwrap();
        eq.hvac.config.zone_capacitance_kwh_per_k = 2.0;

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

        let expected_efficiency = 1.0 / 1.05;
        let actual = eq.telemetry().get(tk::EBM_EFFICIENCY).unwrap();
        assert!(
            (actual - expected_efficiency).abs() < 1e-9,
            "ElectricBoiler EBM efficiency should be 1/1.05 = {expected_efficiency}, got {actual}"
        );
    }

    /// Port declarations are truthful: `init` sets the fluid port's declared
    /// fluid type from the configured `fluid_type`, where it already sets the
    /// port's loop id, so a glycol boiler declares a glycol loop and the
    /// default configuration declares water.
    #[test]
    fn boiler_port_declares_configured_fluid_type() {
        let env = env(18.0);

        // Electric boiler configured Glycol.
        let cfg = EquipmentConfig::from_typed(
            "EB".to_string(),
            "Electric Boiler".to_string(),
            ElectricBoilerConfig {
                zone_id: Some(1),
                loop_id: Some(1),
                fluid_type: FluidType::Glycol,
                ..ElectricBoilerConfig::default()
            },
        )
        .unwrap();
        let mut eq = ElectricBoiler::new(cfg.clone());
        eq.init(&cfg, &env).unwrap();
        let fluid_ports: Vec<_> = eq
            .ports()
            .iter()
            .filter(|p| p.port_type == hares_types::PortType::Fluid)
            .collect();
        assert_eq!(fluid_ports.len(), 1);
        assert_eq!(
            fluid_ports[0].fluid_type,
            Some(FluidType::Glycol),
            "an electric boiler configured Glycol must declare a Glycol fluid port after init"
        );

        // Electric boiler, default configuration: Water.
        let cfg = eb_config(8_000.0, 1.05);
        let mut eq = ElectricBoiler::new(cfg.clone());
        eq.init(&cfg, &env).unwrap();
        let fluid_ports: Vec<_> = eq
            .ports()
            .iter()
            .filter(|p| p.port_type == hares_types::PortType::Fluid)
            .collect();
        assert_eq!(fluid_ports[0].fluid_type, Some(FluidType::Water));

        // Gas boiler configured Glycol.
        let cfg = EquipmentConfig::from_typed(
            "GB".to_string(),
            "Gas Boiler".to_string(),
            GasBoilerConfig {
                zone_id: Some(1),
                loop_id: Some(1),
                fluid_type: FluidType::Glycol,
                ..GasBoilerConfig::default()
            },
        )
        .unwrap();
        let mut eq = GasBoiler::new(cfg.clone());
        eq.init(&cfg, &env).unwrap();
        let fluid_ports: Vec<_> = eq
            .ports()
            .iter()
            .filter(|p| p.port_type == hares_types::PortType::Fluid)
            .collect();
        assert_eq!(fluid_ports.len(), 1);
        assert_eq!(
            fluid_ports[0].fluid_type,
            Some(FluidType::Glycol),
            "a gas boiler configured Glycol must declare a Glycol fluid port after init"
        );

        // Gas boiler, default configuration: Water.
        let cfg = gb_config(8_000.0, 0.95);
        let mut eq = GasBoiler::new(cfg.clone());
        eq.init(&cfg, &env).unwrap();
        let fluid_ports: Vec<_> = eq
            .ports()
            .iter()
            .filter(|p| p.port_type == hares_types::PortType::Fluid)
            .collect();
        assert_eq!(fluid_ports[0].fluid_type, Some(FluidType::Water));
    }
}
