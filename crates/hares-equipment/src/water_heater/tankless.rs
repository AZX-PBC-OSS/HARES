//! Tankless (on-demand) water heater model.

use chrono::Timelike;
use std::borrow::Cow;
use std::time::Duration;

use hares_types::{
    ControlCapabilities, ControlSignal, CoreCapabilities, CoreFlows, CoreOutput, CorePerformance,
    CoreState, DRLevel, ElectricPower, EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId,
    ExecutionStage, FluidType, FuelPower, FuelType, HaresError, OperatingMode, PortContribution,
    PortDeclaration, PortSlots, ScheduleSource, Telemetry, TelemetryField, ZoneId,
    telemetry_keys as tk,
};
use serde::{Deserialize, Serialize};
#[allow(unused_imports)] // warn! used only under debug_assertions or check_invariants
use tracing::warn;

use hares_physics::constants::{
    CP_LIQUID_WATER_J_KG_K, GALLONS_PER_MINUTE_TO_KG_PER_SECOND, UEF_TO_EF_GAS_INTERCEPT,
    UEF_TO_EF_GAS_SLOPE,
};
use hares_physics::units::{power_kw_to_w, power_w_to_kw};
use hares_physics::water_density_kg_m3;

use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_versioned, try_save_versioned};

use super::wh_config::TanklessWaterHeaterConfig;
use crate::hvac::helpers::{equipment_id_from_config, zone_id_from_config_or_default};

const DEFAULT_SETPOINT_C: f64 = 51.666_666_7;
const DEFAULT_EF: f64 = 0.9;
/// Default rated thermal capacity (W). OCHRE uses 20 kW for tankless.
const DEFAULT_MAX_THERMAL_POWER_W: f64 = 20_000.0;
const TANKLESS_WH_CHECKPOINT_VERSION: u32 = 2;
#[derive(Clone, Debug, Serialize, Deserialize)]
struct TanklessState {
    setpoint_c: f64,
    duty_cycle: f64,
    mode_override: Option<OperatingMode>,
    outlet_temp_c: f64,
    thermal_output_w: f64,
    fuel_input_w: f64,
    parasitic_electric_w: f64,
    reactive_power_kvar: f64,
    draw_flow_rate_kg_s: f64,
    // --- Demand response state ---
    dr_level: DRLevel,
    dr_setpoint_offset_c: f64,
    dr_load_fraction: f64,
    dr_duration_remaining_s: Option<f64>,
}

pub struct TanklessWH {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,
    core_output: CoreOutput,
    fuel_type: FuelType,
    setpoint_c: f64,
    efficiency_factor: f64,
    // si-guard-ignore: `GPM` in doc comment reflects HPXML user-facing config field unit; the
    // value is converted to kg/s (SI) at init and never used imperially in simulation.
    /// Minimum flow rate [kg/s] required for the burner to engage.
    /// Typical tankless flow sensors require ~0.03 kg/s (≈0.5 GPM).
    /// Defaults to 0.0 (no minimum) for backward compatibility.
    min_flow_kg_s: f64,
    /// Immutable rated maximum thermal output (W). Never mutated by control signals.
    /// Mirrors OCHRE's `capacity_rated`.
    rated_thermal_power_w: f64,
    /// Transient power limit from PowerLimit control signal (W thermal).
    /// Applied per-step as `min(rated, limit)`. Reset to `None` each step.
    power_limit_w: Option<f64>,
    /// Continuous electric standby draw for the gas electronic ignition controller (W).
    /// Only applies when `fuel_type` is Gas; always present regardless of burner state.
    parasitic_power_w: f64,
    duty_cycle: f64,
    mode_override: Option<OperatingMode>,
    inlet_temp_c: f64,
    draw_flow_rate_kg_s: f64,
    draw_flow_rate_kg_s_source: Option<ScheduleSource>,
    mains_temp_c_source: Option<ScheduleSource>,
    /// Rule R1 reactive-only ZIP (resolved via `crate::config::resolve_reactive_zip`):
    /// control electronics at unity power factor by class default (Q exactly
    /// zero, real power untouched).
    zip: hares_types::zip::ZipLoad,
    // --- Demand response state ---
    dr_setpoint_offset_c: f64,
    dr_load_fraction: f64,
    dr_duration_remaining_s: Option<f64>,
    dr_level: DRLevel,
    // Transient load fraction from LoadFraction control signal; reset each step.
    ctrl_load_fraction: f64,
    /// Whether zone_id was explicitly set in config or fell back to ZoneId(1).
    zone_id_explicit: bool,
    /// Tracks hourly/daily draw volumes for observer capture and invariant checks.
    draw_tracker: super::DrawVolumeTracker,
}

impl TanklessWH {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let (zone, zone_id_explicit) = zone_id_from_config_or_default(&config, &config.name);
        let typed = config
            .require_typed::<TanklessWaterHeaterConfig>("Tankless Water Heater")
            .expect("Tankless Water Heater requires typed FuelType");
        let fuel_type = typed.fuel_type;

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        if typed.zone_id.is_none() && typed.zone_type.is_some() {
            warn!(
                water_heater = %config.name,
                zone_type = ?typed.zone_type,
                "zone_id not resolved from HPXML Location; falling back to ZoneId(1)"
            );
        }

        let ports = build_ports(fuel_type);

        Self {
            descriptor: EquipmentDescriptor {
                id: EquipmentId(equipment_id_from_config(&config).unwrap_or(0)),
                name: config.name,
                end_use: EndUse::WATER_HEATING,
                equipment_type: Cow::Borrowed("Tankless Water Heater"),
                zone: Some(zone),
                fuel: fuel_type,
                stage: ExecutionStage::Thermal,
                control_capabilities: ControlCapabilities::THERMAL_SETPOINT
                    | ControlCapabilities::DUTY_CYCLE
                    | ControlCapabilities::MODE_OVERRIDE
                    | ControlCapabilities::LOAD_FRACTION
                    | ControlCapabilities::POWER_LIMIT
                    | ControlCapabilities::DEMAND_RESPONSE,
                core_capabilities: core_capabilities_for_fuel(fuel_type),
                telemetry_fields: telemetry_fields(),
                zone_type: typed.zone_type.clone(),
            },
            ports,
            telemetry: default_telemetry(),
            core_output: CoreOutput::default(),
            fuel_type,
            setpoint_c: DEFAULT_SETPOINT_C,
            efficiency_factor: DEFAULT_EF,
            min_flow_kg_s: 0.0,
            rated_thermal_power_w: DEFAULT_MAX_THERMAL_POWER_W,
            power_limit_w: None,
            parasitic_power_w: 7.38,
            duty_cycle: 1.0,
            mode_override: None,
            inlet_temp_c: 10.0,
            draw_flow_rate_kg_s: 0.0,
            draw_flow_rate_kg_s_source: None,
            mains_temp_c_source: None,
            zip: hares_types::zip::ZipLoad::constant_power(),
            dr_setpoint_offset_c: 0.0,
            dr_load_fraction: 1.0,
            dr_duration_remaining_s: None,
            dr_level: DRLevel::Normal,
            ctrl_load_fraction: 1.0,
            zone_id_explicit,
            draw_tracker: super::DrawVolumeTracker::new(),
        }
    }

    fn effective_setpoint_c(&self) -> f64 {
        self.setpoint_c + self.dr_setpoint_offset_c
    }

    fn is_enabled(&self) -> bool {
        !matches!(self.mode_override, Some(OperatingMode::Off))
            && self.duty_cycle > 0.0
            && self.dr_load_fraction > 0.0
    }
}

impl TanklessWH {
    fn apply_dr_level(&mut self, level: DRLevel) {
        self.dr_level = level;
        match level {
            DRLevel::Normal => {
                self.dr_setpoint_offset_c = 0.0;
                self.dr_load_fraction = 1.0;
            }
            DRLevel::Moderate => {
                self.dr_setpoint_offset_c = -3.0;
                self.dr_load_fraction = 1.0;
            }
            DRLevel::High => {
                self.dr_setpoint_offset_c = -6.0;
                self.dr_load_fraction = 0.8;
            }
            DRLevel::Critical => {
                self.dr_setpoint_offset_c = -10.0;
                self.dr_load_fraction = 0.5;
            }
            DRLevel::GridEmergency => {
                self.dr_setpoint_offset_c = 0.0;
                self.dr_load_fraction = 0.0;
            }
        }
    }
}

impl TanklessWH {
    fn init_typed(
        &mut self,
        config: &EquipmentConfig,
        env: &EnvironmentState,
    ) -> crate::Result<()> {
        let c = config.require_typed::<TanklessWaterHeaterConfig>("Tankless Water Heater")?;
        c.validate()?;

        self.descriptor.id = EquipmentId(c.equipment_id.unwrap_or(self.descriptor.id.0));
        self.descriptor.zone = c.zone_id.map(ZoneId).or(self.descriptor.zone);
        self.fuel_type = c.fuel_type;
        self.descriptor.fuel = self.fuel_type;
        self.descriptor.core_capabilities = core_capabilities_for_fuel(self.fuel_type);
        self.ports = build_ports(self.fuel_type);

        self.setpoint_c = c.setpoint_c.unwrap_or(DEFAULT_SETPOINT_C);

        // Fallback chain for efficiency factor:
        // 1. energy_factor — pre-2015 EF test procedure value (direct use).
        // 2. uniform_energy_factor — post-2015 UEF test procedure value;
        //    converted to EF-equivalent using the RESNET EF Calculator 2017 /
        //    OCHRE HPXML parser (vendors/OCHRE/ochre/utils/hpxml.py:1115):
        //      EF = UEF_TO_EF_GAS_SLOPE * UEF + UEF_TO_EF_GAS_INTERCEPT
        // 3. DEFAULT_EF — 0.9, a plausible tankless WH efficiency.
        let raw_ef = if let Some(ef) = c.energy_factor {
            ef
        } else if let Some(uef) = c.uniform_energy_factor {
            UEF_TO_EF_GAS_SLOPE * uef + UEF_TO_EF_GAS_INTERCEPT
        } else {
            DEFAULT_EF
        };
        self.efficiency_factor = if let Some(perf_adj) = c.performance_adjustment {
            (raw_ef * perf_adj.clamp(0.0, 1.0)).max(1e-6)
        } else {
            raw_ef.max(1e-6)
        };

        /* si-guard-ignore: `gpm` in field name `min_flow_gpm` is an HPXML-convention
        config field; the value is immediately converted to kg/s (SI). */
        self.min_flow_kg_s = c
            .min_flow_kg_s
            .or_else(|| {
                c.min_flow_gpm
                    .map(|v| v * GALLONS_PER_MINUTE_TO_KG_PER_SECOND)
            }) // si-guard-ignore: `gpm` field name is HPXML convention; converted to kg/s
            .unwrap_or(0.0)
            .max(0.0);

        #[cfg(debug_assertions)]
        assert!(
            (0.0..=1.0).contains(&self.efficiency_factor),
            "efficiency_factor must be in [0.0, 1.0], got {}",
            self.efficiency_factor
        );
        self.rated_thermal_power_w = c
            .heating_capacity_w
            .unwrap_or(DEFAULT_MAX_THERMAL_POWER_W)
            .max(0.0);
        self.power_limit_w = None;
        self.parasitic_power_w = c.parasitic_power_w.unwrap_or(0.0).max(0.0);
        self.duty_cycle = 1.0;
        self.mode_override = None;
        self.inlet_temp_c = if let Some(inlet_temp_c) = c.inlet_temp_c {
            inlet_temp_c
        } else if let Some(source_cfg) = c.mains_temp_c_source.clone() {
            let mut source = source_cfg.into_runtime();
            source
                .value_at(env)
                .ok()
                .filter(|v| v.is_finite())
                .unwrap_or(super::require_mains_temp_c(env, "Tankless Water Heater")?)
        } else {
            super::require_mains_temp_c(env, "Tankless Water Heater")?
        };
        self.draw_flow_rate_kg_s = c.draw_flow_rate_kg_s.unwrap_or(0.0);
        self.draw_flow_rate_kg_s_source =
            c.draw_flow_rate_source.map(|source| source.into_runtime());
        self.mains_temp_c_source = c.mains_temp_c_source.map(|source| source.into_runtime());
        self.zip = crate::config::resolve_reactive_zip(config)?;

        self.dr_setpoint_offset_c = 0.0;
        self.dr_load_fraction = 1.0;
        self.dr_duration_remaining_s = None;
        self.dr_level = DRLevel::Normal;
        self.ctrl_load_fraction = 1.0;
        self.telemetry = default_telemetry();
        self.core_output = CoreOutput::default();
        self.draw_tracker = super::DrawVolumeTracker::new();
        Ok(())
    }
}

impl Equipment for TanklessWH {
    fn checkpoint_version() -> u32 {
        TANKLESS_WH_CHECKPOINT_VERSION
    }

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
        self.init_typed(config, env)
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        // Advance DR duration; auto-revert to Normal when expired.
        let dt_s = env.time_res.num_milliseconds().max(0) as f64 / 1000.0;
        if let Some(remaining) = self.dr_duration_remaining_s {
            let next = remaining - dt_s;
            if next <= 0.0 {
                self.dr_duration_remaining_s = None;
                self.apply_dr_level(DRLevel::Normal);
            } else {
                self.dr_duration_remaining_s = Some(next);
            }
        }

        // Grid outage (de-energized bus): an electric tankless heater has no supply
        // power and cannot fire — cold water passes through unheated. Gating
        // here (the root of dispatch) keeps the energy balance consistent:
        // previously only the *reported* electric draw was zeroed while the
        // water was still heated to setpoint. Gas-fired units keep firing
        // (fuel-side heat is unaffected); their electric ignition-controller
        // parasitic is zeroed separately in `step`.
        if self.fuel_type == FuelType::Electric && !env.grid.bus_energized() {
            return OperatingMode::Off;
        }

        if self.is_enabled() {
            OperatingMode::Heating
        } else {
            OperatingMode::Off
        }
    }

    fn step(
        &mut self,
        env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        let mode = self.update_control(env);

        let duty =
            (self.duty_cycle * self.dr_load_fraction * self.ctrl_load_fraction).clamp(0.0, 1.0);
        // DR may offset setpoint; use effective setpoint for thermal calculation.
        let setpoint_c = self.effective_setpoint_c();
        let draw_flow_rate_kg_s_source = self.draw_flow_rate_kg_s_source.as_mut();
        let mains_temp_c_source = self.mains_temp_c_source.as_mut();
        let (inlet_temp_c, schedule_draw_kg_s) = super::resolve_storage_step_inputs(
            env,
            self.inlet_temp_c,
            self.draw_flow_rate_kg_s,
            draw_flow_rate_kg_s_source,
            mains_temp_c_source,
        );
        let delta_t_c = (setpoint_c - inlet_temp_c).max(0.0);
        let _ = dt; // dt not used for tankless (on-demand model)

        // Effective capacity: rated power, optionally clamped by PowerLimit signal.
        let effective_max_w = match self.power_limit_w {
            Some(limit) => self.rated_thermal_power_w.min(limit),
            None => self.rated_thermal_power_w,
        };

        let appliance_demand_kg_s = super::read_dhw_demand_kg_s(ports);
        let total_draw_kg_s = schedule_draw_kg_s + appliance_demand_kg_s;

        let draw_volume_l =
            total_draw_kg_s * dt.as_secs_f64() / water_density_kg_m3(inlet_temp_c) * 1000.0;
        self.draw_tracker
            .accumulate(draw_volume_l, env.current_time.hour());

        let (thermal_output_w, outlet_temp_c) =
            if mode == OperatingMode::Heating && total_draw_kg_s > self.min_flow_kg_s {
                // Unclamped thermal demand to reach setpoint.
                let demand_w = total_draw_kg_s * CP_LIQUID_WATER_J_KG_K * delta_t_c * duty;

                let capacity_w = effective_max_w * duty;

                if demand_w <= capacity_w {
                    // Within capacity: deliver setpoint temperature.
                    (demand_w, setpoint_c)
                } else {
                    // Over-capacity: clamp time-averaged output. Outlet temperature
                    // uses effective_max_w (rated power, optionally capped by PowerLimit).
                    // The heater fires at effective_max_w during its on-fraction:
                    // duty-cycle-only → effective_max_w == rated (100% fire during on-phase);
                    // PowerLimit → effective_max_w < rated (firing rate is actively limited,
                    // so the burner never reaches full nameplate rating).
                    let outlet_c =
                        inlet_temp_c + effective_max_w / (total_draw_kg_s * CP_LIQUID_WATER_J_KG_K);
                    (capacity_w, outlet_c)
                }
            } else if mode == OperatingMode::Heating {
                // Heating mode but flow at or below minimum threshold: no burner output.
                (0.0, setpoint_c)
            } else {
                // Off: outlet equals inlet.
                (0.0, inlet_temp_c)
            };

        let fuel_input_w = thermal_output_w / self.efficiency_factor;
        let mut fuel_w_for_core = None;
        let mut parasitic_electric_w_reported = 0.0_f64;
        let (electric_kw_for_core, reactive_kvar_for_core) = if self.fuel_type == FuelType::Electric
        {
            // Grid outage is gated at the root in `update_control` (electric
            // units are forced Off on a de-energized bus, making fuel_input_w zero),
            // so delivered heat and metered draw are always consistent here.
            // Rule R1: Q from the already-computed real power (never through
            // the real-power ZIP polynomial).
            let electric_w = fuel_input_w;
            let reactive_kvar = self
                .zip
                .reactive_kvar(power_w_to_kw(electric_w), env.grid.bus_voltage_pu());
            if electric_w > 0.0 || reactive_kvar != 0.0 {
                ports.accumulate(&PortContribution::Electrical {
                    active_power_w: electric_w,
                    reactive_power_kvar: reactive_kvar,
                })?;
            }
            (power_w_to_kw(electric_w).max(0.0), reactive_kvar)
        } else {
            if fuel_input_w > 0.0 {
                ports.accumulate(&PortContribution::Fuel {
                    fuel_type: self.fuel_type,
                    consumption_w: fuel_input_w,
                })?;
            }
            fuel_w_for_core = Some(FuelPower {
                fuel_type: self.fuel_type,
                consumption_w: fuel_input_w.max(0.0),
            });
            // Gas ignition controller draws electricity continuously regardless of
            // burner state (OCHRE/ANSI RESNET 301 standby parasitic).
            // Grid outage guard and Rule R1 Q as above.
            let parasitic_w = if !env.grid.bus_energized() {
                0.0
            } else {
                self.parasitic_power_w
            };
            let parasitic_kvar = self
                .zip
                .reactive_kvar(power_w_to_kw(parasitic_w), env.grid.bus_voltage_pu());
            ports.accumulate(&PortContribution::Electrical {
                active_power_w: parasitic_w,
                reactive_power_kvar: parasitic_kvar,
            })?;
            parasitic_electric_w_reported = parasitic_w.max(0.0);
            (power_w_to_kw(parasitic_w).max(0.0), parasitic_kvar)
        };

        self.telemetry.set(tk::OUTLET_TEMP_C, outlet_temp_c);
        self.telemetry.set(tk::THERMAL_OUTPUT_W, thermal_output_w);
        self.telemetry.set(tk::FUEL_INPUT_W, fuel_input_w);
        self.telemetry
            .set(tk::PARASITIC_ELECTRIC_W, parasitic_electric_w_reported);
        self.telemetry
            .set(tk::REACTIVE_POWER_KVAR, reactive_kvar_for_core);
        self.telemetry.set(tk::DRAW_FLOW_RATE_KG_S, total_draw_kg_s);
        let has_nonzero_flow = electric_kw_for_core > 0.0 || fuel_input_w > 0.0;
        let mode = mode.resolve_idle(has_nonzero_flow, None);
        self.telemetry.set(
            tk::OPERATING_MODE,
            if mode == OperatingMode::Heating {
                1.0
            } else {
                0.0
            },
        );
        let core_output = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(electric_kw_for_core)),
                reactive_power_kvar: Some(reactive_kvar_for_core),
                fuel_w: fuel_w_for_core,
                thermal_output_w: None,
                sensible_cooling_w: None,
                latent_cooling_w: None,
            },
            state: CoreState {
                operating_mode: Some(mode),
                soc: None,
                speed_index: None,
                setpoint_c: None,
            },
            performance: CorePerformance::default(),
        };

        // Reset transient signals after this step so they do not carry over
        // unless reapplied by the controller.
        self.ctrl_load_fraction = 1.0;
        self.power_limit_w = None;
        self.core_output = core_output;

        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn core_output(&self) -> &CoreOutput {
        &self.core_output
    }

    fn resolved_zip(&self) -> Option<hares_types::zip::ZipLoad> {
        Some(self.zip)
    }

    fn save_state(&self) -> crate::Result<Vec<u8>> {
        try_save_versioned(
            &TanklessState {
                setpoint_c: self.setpoint_c,
                duty_cycle: self.duty_cycle,
                mode_override: self.mode_override,
                outlet_temp_c: self.telemetry.get(tk::OUTLET_TEMP_C).unwrap_or(0.0),
                thermal_output_w: self.telemetry.get(tk::THERMAL_OUTPUT_W).unwrap_or(0.0),
                fuel_input_w: self.telemetry.get(tk::FUEL_INPUT_W).unwrap_or(0.0),
                parasitic_electric_w: self.telemetry.get(tk::PARASITIC_ELECTRIC_W).unwrap_or(0.0),
                reactive_power_kvar: self.telemetry.get(tk::REACTIVE_POWER_KVAR).unwrap_or(0.0),
                draw_flow_rate_kg_s: self.telemetry.get(tk::DRAW_FLOW_RATE_KG_S).unwrap_or(0.0),
                dr_level: self.dr_level,
                dr_setpoint_offset_c: self.dr_setpoint_offset_c,
                dr_load_fraction: self.dr_load_fraction,
                dr_duration_remaining_s: self.dr_duration_remaining_s,
            },
            Self::checkpoint_version(),
            "TanklessWH",
        )
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let decoded: TanklessState = load_versioned(
            state,
            Self::checkpoint_version(),
            "TanklessWH",
            self.descriptor().id,
        )?;
        self.setpoint_c = decoded.setpoint_c;
        self.duty_cycle = decoded.duty_cycle;
        self.mode_override = decoded.mode_override;
        self.dr_level = decoded.dr_level;
        self.dr_setpoint_offset_c = decoded.dr_setpoint_offset_c;
        self.dr_load_fraction = decoded.dr_load_fraction;
        self.dr_duration_remaining_s = decoded.dr_duration_remaining_s;

        self.telemetry
            .insert(tk::OUTLET_TEMP_C, decoded.outlet_temp_c);
        self.telemetry
            .insert(tk::THERMAL_OUTPUT_W, decoded.thermal_output_w);
        self.telemetry
            .insert(tk::FUEL_INPUT_W, decoded.fuel_input_w);
        self.telemetry
            .insert(tk::PARASITIC_ELECTRIC_W, decoded.parasitic_electric_w);
        self.telemetry
            .insert(tk::REACTIVE_POWER_KVAR, decoded.reactive_power_kvar);
        self.telemetry
            .insert(tk::DRAW_FLOW_RATE_KG_S, decoded.draw_flow_rate_kg_s);
        self.telemetry.insert(
            tk::OPERATING_MODE,
            if decoded.thermal_output_w > 0.0 {
                1.0
            } else {
                0.0
            },
        );
        self.core_output = CoreOutput::default();

        Ok(())
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        match signal {
            ControlSignal::ThermalSetpoint {
                heating_setpoint_c,
                cooling_setpoint_c,
                ..
            } => {
                if let Some(sp) = heating_setpoint_c.or(*cooling_setpoint_c) {
                    if !sp.is_finite() {
                        return Err(HaresError::Control(format!(
                            "invalid water-heater setpoint: {sp}"
                        )));
                    }
                    self.setpoint_c = sp;
                }
            }
            ControlSignal::DutyCycle { on_fraction, .. } => {
                if !on_fraction.is_finite() || !(0.0..=1.0).contains(on_fraction) {
                    return Err(HaresError::Control(format!(
                        "invalid duty cycle for TanklessWH: {on_fraction}"
                    )));
                }
                self.duty_cycle = *on_fraction;
            }
            ControlSignal::ModeOverride { mode } => {
                self.mode_override = Some(*mode);
            }
            ControlSignal::LoadFraction { fraction } => {
                self.ctrl_load_fraction = fraction.clamp(0.0, 1.0);
            }
            ControlSignal::PowerLimit { max_power_kw, .. } => {
                // For gas units, max_power_kw caps fuel consumption; thermal limit
                // = fuel_limit × efficiency. For electric units, it directly caps
                // electrical input which equals thermal output / efficiency.
                let limit_w = (power_kw_to_w(*max_power_kw) * self.efficiency_factor).max(0.0);
                self.power_limit_w = Some(limit_w);
            }
            ControlSignal::DemandResponse { level, duration_s } => {
                self.apply_dr_level(*level);
                self.dr_duration_remaining_s = *duration_s;
            }
            _ => {}
        }
        Ok(())
    }
}

pub fn register_with_registry(registry: &mut EquipmentRegistry) {
    registry.register(
        "Tankless Water Heater",
        Box::new(|config| Box::new(TanklessWH::new(config))),
    );
    registry.register(
        "Gas Tankless Water Heater",
        Box::new(|config| Box::new(TanklessWH::new(config))),
    );
}

/// Build port declarations for a tankless water heater.
///
/// Electric: one Electrical port only.
/// Gas: one Fuel port + one Electrical port (for the ignition controller parasitic).
fn build_ports(fuel_type: FuelType) -> Vec<PortDeclaration> {
    let dhw_port = PortDeclaration::fluid(super::DHW_DEMAND_LOOP, FluidType::Water);
    if fuel_type == FuelType::Electric {
        vec![PortDeclaration::electrical(), dhw_port]
    } else {
        vec![
            PortDeclaration::fuel(),
            PortDeclaration::electrical(),
            dhw_port,
        ]
    }
}

fn core_capabilities_for_fuel(fuel_type: FuelType) -> CoreCapabilities {
    let mut capabilities =
        CoreCapabilities::ELECTRIC | CoreCapabilities::REACTIVE | CoreCapabilities::HAS_MODE;
    if fuel_type != FuelType::Electric {
        capabilities |= CoreCapabilities::FUEL;
    }
    capabilities
}

fn default_telemetry() -> Telemetry {
    let mut telemetry = Telemetry::with_capacity(6);
    telemetry.insert(tk::OUTLET_TEMP_C, 0.0);
    telemetry.insert(tk::THERMAL_OUTPUT_W, 0.0);
    telemetry.insert(tk::FUEL_INPUT_W, 0.0);
    telemetry.insert(tk::PARASITIC_ELECTRIC_W, 0.0);
    telemetry.insert(tk::REACTIVE_POWER_KVAR, 0.0);
    telemetry.insert(tk::DRAW_FLOW_RATE_KG_S, 0.0);
    telemetry.insert(tk::OPERATING_MODE, 0.0);
    telemetry
}

fn telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField {
            name: tk::OUTLET_TEMP_C.to_string(),
            unit: "C".to_string(),
            description: "Delivered outlet water temperature".to_string(),
        },
        TelemetryField {
            name: tk::THERMAL_OUTPUT_W.to_string(),
            unit: "W".to_string(),
            description: "Instantaneous thermal output".to_string(),
        },
        TelemetryField {
            name: tk::FUEL_INPUT_W.to_string(),
            unit: "W".to_string(),
            description: "Input fuel/electric power".to_string(),
        },
        TelemetryField {
            name: tk::PARASITIC_ELECTRIC_W.to_string(),
            unit: "W".to_string(),
            description: "Gas ignition controller standby electric draw (gas units only)"
                .to_string(),
        },
        TelemetryField {
            name: tk::REACTIVE_POWER_KVAR.to_string(),
            unit: "kVAR".to_string(),
            description: "Reactive power (positive = inductive/lagging)".to_string(),
        },
        TelemetryField {
            name: tk::DRAW_FLOW_RATE_KG_S.to_string(),
            unit: "kg/s".to_string(),
            description: "Domestic hot water draw flow rate".to_string(),
        },
        TelemetryField {
            name: tk::OPERATING_MODE.to_string(),
            unit: "enum".to_string(),
            description: "0=Off, 1=Heating".to_string(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        EnvironmentState, FuelType, GridState, PortDeclaration, PortSlots, WeatherState, ZoneId,
        ZoneState, telemetry_keys as tk,
    };

    use hares_physics::constants::CP_LIQUID_WATER_J_KG_K;

    use super::TanklessWH;
    use crate::water_heater::DHW_DEMAND_LOOP;
    use crate::{Equipment, EquipmentConfig, TanklessWaterHeaterConfig};

    fn env() -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: 21.0,
                humidity_ratio: 0.008,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 10.0,
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
                mains_temp_c: 20.0,
                ..Default::default()
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
                island_bus_voltage_pu: None,
            },
            custom_domains: vec![],
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: std::collections::HashMap::new(),
            current_time: FixedOffset::east_opt(0)
                .expect("UTC offset")
                .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                .single()
                .expect("valid"),
            time_res: ChronoDuration::seconds(60),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    /// Reactive-power contract for tankless water heaters: control
    /// electronics at unity power factor (class default) → Q is exactly
    /// Some(0.0) while drawing power, REACTIVE declared, channels agree.
    #[test]
    fn reactive_power_is_some_zero_at_unity_pf() {
        for fuel in [FuelType::Gas, FuelType::Electric] {
            let mut typed = typed_config();
            typed.fuel_type = fuel;
            if fuel == FuelType::Electric {
                typed.energy_factor = Some(0.95);
            }
            let ochre_class = match fuel {
                FuelType::Electric => "Tankless Water Heater",
                _ => "Gas Tankless Water Heater",
            };
            let config =
                EquipmentConfig::from_typed("Tankless".to_string(), ochre_class.to_string(), typed)
                    .unwrap();
            let mut eq = TanklessWH::new(config.clone());
            let env = env();
            eq.init(&config, &env).unwrap();
            assert!(
                eq.descriptor()
                    .core_capabilities
                    .contains(hares_types::CoreCapabilities::REACTIVE),
                "tankless WH must declare REACTIVE"
            );
            assert_eq!(
                eq.zip.pf, 1.0,
                "class default pf must be unity ({ochre_class})"
            );

            let mut ports = PortSlots::from_declarations(eq.ports());
            eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
            assert!(
                ports.electrical.load_power_w > 0.0,
                "{ochre_class}: burner draw or parasitic controller must draw power"
            );
            assert_eq!(ports.electrical.reactive_power_kvar, 0.0);
            assert_eq!(eq.core_output().flows.reactive_power_kvar, Some(0.0));
            assert_eq!(eq.telemetry().get(tk::REACTIVE_POWER_KVAR), Some(0.0));
            hares_types::validate_core_contract(eq.descriptor(), eq.core_output())
                .expect("core contract must hold with REACTIVE declared");
        }
    }

    fn config_from_typed(cfg: TanklessWaterHeaterConfig) -> EquipmentConfig {
        EquipmentConfig::from_typed(
            "Tankless".to_string(),
            "Tankless Water Heater".to_string(),
            cfg,
        )
        .unwrap()
    }

    fn typed_config() -> TanklessWaterHeaterConfig {
        TanklessWaterHeaterConfig {
            equipment_id: None,
            zone_id: None,
            loop_id: None,
            fuel_type: FuelType::Gas,
            energy_factor: Some(0.8),
            uniform_energy_factor: None,
            heating_capacity_w: Some(30_000.0),
            setpoint_c: Some(50.0),
            parasitic_power_w: Some(7.38),
            performance_adjustment: None,
            inlet_temp_c: Some(20.0),
            draw_flow_rate_kg_s: Some(0.2),
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            avg_water_draw_l_per_day: None,
            zone_type: None,
            min_flow_kg_s: None,
            min_flow_gpm: None,
        }
    }

    fn config() -> EquipmentConfig {
        config_from_typed(typed_config())
    }

    /// Grid outage: an electric tankless heater cannot fire — cold water
    /// passes through unheated and the meter reads zero (energy-balance
    /// consistency). Heating resumes once the grid is restored.
    #[test]
    fn grid_outage_stops_electric_tankless_heating_until_restoration() {
        let mut typed = typed_config();
        typed.fuel_type = FuelType::Electric;
        typed.energy_factor = Some(0.95);
        let config = config_from_typed(typed);
        let mut eq = TanklessWH::new(config.clone());
        let mut env = env();
        eq.init(&config, &env).unwrap();

        env.grid.voltage_pu = 0.0;
        let mut ports = PortSlots::from_declarations(eq.ports());
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        assert_eq!(
            ports.electrical.load_power_w, 0.0,
            "no electric draw during outage"
        );
        assert_eq!(eq.telemetry().get(tk::THERMAL_OUTPUT_W), Some(0.0));
        assert_eq!(eq.telemetry().get(tk::FUEL_INPUT_W), Some(0.0));
        assert_eq!(eq.telemetry().get(tk::OPERATING_MODE), Some(0.0));
        // Cold water passes through: outlet equals inlet.
        assert_eq!(eq.telemetry().get(tk::OUTLET_TEMP_C), Some(20.0));

        env.grid.voltage_pu = 1.0;
        let mut ports = PortSlots::from_declarations(eq.ports());
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        assert!(
            ports.electrical.load_power_w > 0.0,
            "heating must resume after grid restoration"
        );
        assert_eq!(
            eq.telemetry().get(tk::OUTLET_TEMP_C),
            Some(50.0),
            "within capacity, restored unit delivers setpoint temperature"
        );
    }

    /// Grid outage: a gas tankless keeps firing (fuel-side heat is
    /// unaffected) but its ignition-controller electric parasitic is lost.
    #[test]
    fn grid_outage_gas_tankless_keeps_firing_without_parasitic() {
        let config = config();
        let mut eq = TanklessWH::new(config.clone());
        let mut env = env();
        eq.init(&config, &env).unwrap();

        env.grid.voltage_pu = 0.0;
        let mut ports = PortSlots::from_declarations(eq.ports());
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        let thermal_w = eq.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap();
        assert!(
            thermal_w > 0.0,
            "gas burner must keep firing during an outage"
        );
        assert!(eq.telemetry().get(tk::FUEL_INPUT_W).unwrap() > thermal_w);
        assert_eq!(
            ports.electrical.load_power_w, 0.0,
            "ignition-controller parasitic must be lost during the outage"
        );
        assert_eq!(eq.telemetry().get(tk::PARASITIC_ELECTRIC_W), Some(0.0));
    }

    /// Build a config that also sets max_thermal_power_w explicitly.
    fn config_with_capacity(max_thermal_power_w: f64) -> EquipmentConfig {
        let mut cfg = typed_config();
        cfg.heating_capacity_w = Some(max_thermal_power_w);
        config_from_typed(cfg)
    }

    #[test]
    fn port_declarations_and_telemetry_contract_match_fuel_type() {
        let gas = TanklessWH::new(config());
        assert_eq!(gas.ports().len(), 3);
        assert!(gas.ports().contains(&PortDeclaration::fluid(
            DHW_DEMAND_LOOP,
            hares_types::FluidType::Water
        )));
        assert!(
            gas.ports().contains(&PortDeclaration::fuel())
                || gas.ports().contains(&PortDeclaration::electrical()),
            "tankless gas config must declare at least one energy port"
        );

        let mut electric_cfg = typed_config();
        electric_cfg.fuel_type = FuelType::Electric;
        electric_cfg.energy_factor = Some(0.95);
        let electric_cfg = config_from_typed(electric_cfg);
        let electric = TanklessWH::new(electric_cfg.clone());
        assert_eq!(electric.ports().len(), 2);
        assert!(electric.ports().contains(&PortDeclaration::electrical()));
        assert!(electric.ports().contains(&PortDeclaration::fluid(
            DHW_DEMAND_LOOP,
            hares_types::FluidType::Water
        )));

        let mut eq = TanklessWH::new(electric_cfg.clone());
        eq.init(&electric_cfg, &env()).unwrap();
        step_once(&mut eq);
        assert_eq!(
            eq.telemetry().len(),
            eq.descriptor().telemetry_fields.len(),
            "tankless telemetry map must match declared telemetry schema"
        );
    }

    fn step_once(eq: &mut TanklessWH) -> PortSlots {
        let mut ports = PortSlots::default();
        eq.step(&env(), Duration::from_secs(60), &mut ports)
            .unwrap();
        ports
    }

    // --- Existing regression tests (must continue passing) ---

    #[test]
    fn thermal_output_and_fuel_input_follow_exact_formula() {
        let mut eq = TanklessWH::new(config());
        eq.init(&config(), &env()).unwrap();

        let mut ports = PortSlots::default();
        eq.step(&env(), Duration::from_secs(60), &mut ports)
            .unwrap();

        let expected_thermal = 0.2 * CP_LIQUID_WATER_J_KG_K * (50.0 - 20.0);
        let expected_input = expected_thermal / 0.8;

        assert!(
            (eq.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap() - expected_thermal).abs() < 1e-6
        );
        assert!((eq.telemetry().get(tk::FUEL_INPUT_W).unwrap() - expected_input).abs() < 1e-6);
        assert!((ports.fuel.get(hares_types::FuelType::Gas) - expected_input).abs() < 1e-6);
    }

    /// When the tankless WH is off (mode override), outlet_temp_c must equal
    /// inlet_temp_c rather than the setpoint temperature.
    #[test]
    fn outlet_temp_equals_inlet_when_off() {
        use hares_types::{ControlSignal, OperatingMode};

        let mut eq = TanklessWH::new(config());
        eq.init(&config(), &env()).unwrap();

        eq.apply_control(&ControlSignal::ModeOverride {
            mode: OperatingMode::Off,
        })
        .unwrap();

        let mut ports = PortSlots::default();
        eq.step(&env(), Duration::from_secs(60), &mut ports)
            .unwrap();

        let outlet = eq.telemetry().get(tk::OUTLET_TEMP_C).unwrap();
        assert_eq!(
            outlet, 20.0,
            "outlet_temp_c must equal inlet_temp_c (20.0) when off, got {outlet}"
        );
        assert_eq!(
            eq.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap(),
            0.0,
            "thermal_output_w must be zero when off"
        );
    }

    #[test]
    fn state_round_trip_preserves_dr_state() {
        use hares_types::{ControlSignal, DRLevel};

        let mut eq = TanklessWH::new(config());
        eq.init(&config(), &env()).unwrap();

        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::Critical,
            duration_s: Some(300.0),
        })
        .unwrap();

        let saved = eq.save_state().unwrap();

        let mut restored = TanklessWH::new(config());
        restored.init(&config(), &env()).unwrap();
        restored.load_state(&saved).unwrap();

        assert_eq!(
            restored.dr_level,
            DRLevel::Critical,
            "dr_level must survive save/load"
        );
        assert!(
            restored.dr_setpoint_offset_c < 0.0,
            "dr_setpoint_offset_c must be negative after Critical DR (got {})",
            restored.dr_setpoint_offset_c
        );
        assert_eq!(
            restored.dr_duration_remaining_s,
            Some(300.0),
            "dr_duration_remaining_s must survive save/load"
        );
        assert!(
            restored.dr_load_fraction < 1.0,
            "dr_load_fraction must be < 1.0 after Critical DR (got {})",
            restored.dr_load_fraction
        );
    }

    /// Checkpoint round-trip: REACTIVE_POWER_KVAR telemetry must survive
    /// save_state/load_state exactly (bit-for-bit), mirroring the persistence
    /// of PARASITIC_ELECTRIC_W and other per-step telemetry.
    #[test]
    fn state_round_trip_preserves_reactive_power_kvar() {
        let mut eq = TanklessWH::new(config());
        eq.init(&config(), &env()).unwrap();

        let mut p = PortSlots::default();
        eq.step(&env(), Duration::from_secs(60), &mut p).unwrap();
        let pre_save = eq
            .telemetry()
            .get(tk::REACTIVE_POWER_KVAR)
            .expect("REACTIVE_POWER_KVAR telemetry present");

        let state = eq.save_state().unwrap();

        let mut restored = TanklessWH::new(config());
        restored.init(&config(), &env()).unwrap();
        restored.load_state(&state).unwrap();

        let post_load = restored
            .telemetry()
            .get(tk::REACTIVE_POWER_KVAR)
            .expect("REACTIVE_POWER_KVAR telemetry restored");
        assert_eq!(
            pre_save.to_bits(),
            post_load.to_bits(),
            "REACTIVE_POWER_KVAR must be bit-identical after save/load round-trip"
        );
    }

    /// DR GridEmergency sets dr_load_fraction=0.0, which disables the tankless WH.
    /// Even with active draw flow, thermal output must be zero.
    #[test]
    fn tankless_dr_grid_emergency_forces_off() {
        let mut eq = TanklessWH::new(config());
        eq.init(&config(), &env()).unwrap();

        let mut p_before = PortSlots::default();
        eq.step(&env(), Duration::from_secs(60), &mut p_before)
            .unwrap();
        assert!(
            eq.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap_or(0.0) > 0.0,
            "must be producing heat before DR is applied"
        );

        let mut eq2 = TanklessWH::new(config());
        eq2.init(&config(), &env()).unwrap();
        eq2.apply_control(&hares_types::ControlSignal::DemandResponse {
            level: hares_types::DRLevel::GridEmergency,
            duration_s: None,
        })
        .unwrap();
        assert_eq!(
            eq2.dr_load_fraction, 0.0,
            "GridEmergency must set dr_load_fraction to 0"
        );

        let mut p = PortSlots::default();
        eq2.step(&env(), Duration::from_secs(60), &mut p).unwrap();

        assert_eq!(
            eq2.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap_or(1.0),
            0.0,
            "thermal output must be zero during GridEmergency"
        );
        assert_eq!(
            eq2.telemetry().get(tk::FUEL_INPUT_W).unwrap_or(1.0),
            0.0,
            "fuel input must be zero during GridEmergency"
        );
        assert_eq!(
            p.fuel.get(hares_types::FuelType::Gas),
            0.0,
            "fuel port must report zero consumption during GridEmergency"
        );
    }

    /// DR Critical sets dr_load_fraction=0.5. An external LoadFraction(0.5) sets
    /// ctrl_load_fraction=0.5. These two fields compound multiplicatively in the duty
    /// calculation: duty = duty_cycle * dr_load_fraction * ctrl_load_fraction.
    #[test]
    fn dr_and_external_load_fraction_compound_correctly() {
        let mut eq_dr = TanklessWH::new(config());
        eq_dr.init(&config(), &env()).unwrap();
        eq_dr
            .apply_control(&hares_types::ControlSignal::DemandResponse {
                level: hares_types::DRLevel::Critical,
                duration_s: None,
            })
            .unwrap();
        assert_eq!(
            eq_dr.dr_load_fraction, 0.5,
            "Critical must set dr_load_fraction to 0.5"
        );
        assert!(
            (eq_dr.ctrl_load_fraction - 1.0).abs() < 1e-9,
            "ctrl_load_fraction must be unchanged (1.0) after DemandResponse; got {}",
            eq_dr.ctrl_load_fraction
        );

        let mut eq_compound = TanklessWH::new(config());
        eq_compound.init(&config(), &env()).unwrap();
        eq_compound
            .apply_control(&hares_types::ControlSignal::DemandResponse {
                level: hares_types::DRLevel::Critical,
                duration_s: None,
            })
            .unwrap();
        eq_compound
            .apply_control(&hares_types::ControlSignal::LoadFraction { fraction: 0.5 })
            .unwrap();

        assert_eq!(
            eq_compound.dr_load_fraction, 0.5,
            "dr_load_fraction must be 0.5 after Critical DR"
        );
        assert!(
            (eq_compound.ctrl_load_fraction - 0.5).abs() < 1e-9,
            "ctrl_load_fraction must be 0.5 after LoadFraction(0.5); got {}",
            eq_compound.ctrl_load_fraction
        );

        let effective_duty =
            eq_compound.duty_cycle * eq_compound.dr_load_fraction * eq_compound.ctrl_load_fraction;
        assert!(
            (effective_duty - 0.25).abs() < 1e-9,
            "compound duty before step must be 0.25 (dr=0.5 * ctrl=0.5); got {effective_duty:.4}"
        );
    }

    // --- New capacity-limiting tests ---

    /// Normal operation: demand is within capacity, so setpoint is delivered.
    /// Config: flow=0.2 kg/s, delta_T=30 K → demand = 0.2*CP_LIQUID_WATER*30 ≈ 25,098 W.
    /// With max_thermal_power_w=30,000 W this is within capacity.
    #[test]
    fn normal_operation_within_capacity_delivers_setpoint() {
        // demand_w = 0.2 * CP_LIQUID_WATER_J_KG_K * 30 ≈ 25,098 W < 30,000 W capacity
        let cap = config_with_capacity(30_000.0);
        let mut eq = TanklessWH::new(cap.clone());
        eq.init(&cap, &env()).unwrap();

        step_once(&mut eq);

        let outlet = eq.telemetry().get(tk::OUTLET_TEMP_C).unwrap();
        let thermal = eq.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap();
        let fuel = eq.telemetry().get(tk::FUEL_INPUT_W).unwrap();
        let expected_thermal = 0.2 * CP_LIQUID_WATER_J_KG_K * 30.0;

        assert!(
            (outlet - 50.0).abs() < 1e-6,
            "outlet must equal setpoint (50°C) when within capacity, got {outlet}"
        );
        assert!(
            (thermal - expected_thermal).abs() < 1e-6,
            "thermal output must match demand {expected_thermal:.1} W, got {thermal:.1}"
        );
        assert!(
            (fuel - expected_thermal / 0.8).abs() < 1e-6,
            "fuel input must be thermal/efficiency"
        );
    }

    /// Over-capacity: flow rate is high enough that demand exceeds rated capacity.
    /// Config: flow=1.0 kg/s, delta_T=30 K → demand = 1.0*CP_LIQUID_WATER*30 ≈ 125,280 W.
    /// With max_thermal_power_w=20,000 W, output clamps at 20,000 W.
    #[test]
    fn over_capacity_clamps_output_and_reduces_outlet_temp() {
        // 1.0 kg/s → demand = 1.0 * CP_LIQUID_WATER_J_KG_K * 30 >> 20,000 W capacity
        let mut typed = typed_config();
        typed.draw_flow_rate_kg_s = Some(1.0);
        typed.heating_capacity_w = Some(20_000.0);
        let cap = config_from_typed(typed);

        let mut eq = TanklessWH::new(cap.clone());
        eq.init(&cap, &env()).unwrap();

        step_once(&mut eq);

        let thermal = eq.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap();
        let fuel = eq.telemetry().get(tk::FUEL_INPUT_W).unwrap();
        let outlet = eq.telemetry().get(tk::OUTLET_TEMP_C).unwrap();

        // Thermal output must be clamped at capacity.
        assert!(
            (thermal - 20_000.0).abs() < 1e-6,
            "thermal must be clamped at 20,000 W, got {thermal:.1}"
        );
        // Fuel input = capacity / efficiency.
        assert!(
            (fuel - 20_000.0 / 0.8).abs() < 1e-6,
            "fuel input must equal capacity/efficiency = {:.1} W, got {fuel:.1}",
            20_000.0_f64 / 0.8
        );
        // Outlet temperature must be below setpoint.
        assert!(
            outlet < 50.0,
            "outlet temp must be below setpoint (50°C) when over-capacity, got {outlet}"
        );
        // Outlet temperature formula: T_out = T_in + Q_max / (m_dot * c_p)
        let expected_outlet = 20.0 + 20_000.0 / (1.0 * CP_LIQUID_WATER_J_KG_K);
        assert!(
            (outlet - expected_outlet).abs() < 1e-6,
            "outlet temp must be {expected_outlet:.4}°C, got {outlet:.4}"
        );
        // Outlet must be above inlet (heater is still adding heat).
        assert!(
            outlet > 20.0,
            "outlet must be above inlet (20°C), got {outlet}"
        );
    }

    /// Zero flow: no thermal output and no fuel consumption in any mode.
    #[test]
    fn zero_flow_produces_no_output_and_no_fuel() {
        let mut typed = typed_config();
        typed.fuel_type = FuelType::Electric;
        typed.energy_factor = Some(0.95);
        typed.draw_flow_rate_kg_s = Some(0.0);
        let cfg = config_from_typed(typed);

        let mut eq = TanklessWH::new(cfg.clone());
        eq.init(&cfg, &env()).unwrap();

        let ports = step_once(&mut eq);

        assert_eq!(
            eq.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap(),
            0.0,
            "zero flow must produce zero thermal output"
        );
        assert_eq!(
            eq.telemetry().get(tk::FUEL_INPUT_W).unwrap(),
            0.0,
            "zero flow must produce zero fuel input"
        );
        assert_eq!(
            ports.electrical.load_power_w, 0.0,
            "zero flow must produce zero electrical draw"
        );
    }

    /// Fuel consumption (input power) must never exceed max_input_power_w = capacity / efficiency.
    #[test]
    fn fuel_input_never_exceeds_max_input_power() {
        // Very high flow to force over-capacity.
        let mut typed = typed_config();
        typed.setpoint_c = Some(60.0);
        typed.inlet_temp_c = Some(5.0);
        typed.draw_flow_rate_kg_s = Some(5.0);
        typed.energy_factor = Some(0.85);
        typed.heating_capacity_w = Some(25_000.0);
        let cfg = config_from_typed(typed);

        let mut eq = TanklessWH::new(cfg.clone());
        eq.init(&cfg, &env()).unwrap();

        step_once(&mut eq);

        let fuel = eq.telemetry().get(tk::FUEL_INPUT_W).unwrap();
        let max_input_w = 25_000.0 / 0.85;

        assert!(
            fuel <= max_input_w + 1e-6,
            "fuel_input_w ({fuel:.2} W) must not exceed max_input_power_w ({max_input_w:.2} W)"
        );
    }

    /// Very low flow rate: demand is well within capacity, setpoint delivered.
    #[test]
    fn very_low_flow_rate_within_capacity() {
        // 0.001 kg/s → demand = 0.001 * CP_LIQUID_WATER_J_KG_K * 30 ≈ 125.4 W, well below 20 kW capacity
        let mut typed = typed_config();
        typed.draw_flow_rate_kg_s = Some(0.001_f64);
        typed.energy_factor = Some(0.9);
        typed.heating_capacity_w = Some(20_000.0);
        let cfg = config_from_typed(typed);

        let mut eq = TanklessWH::new(cfg.clone());
        eq.init(&cfg, &env()).unwrap();

        step_once(&mut eq);

        let outlet = eq.telemetry().get(tk::OUTLET_TEMP_C).unwrap();
        assert!(
            (outlet - 50.0).abs() < 1e-6,
            "very low flow must still deliver setpoint (50°C), got {outlet}"
        );
    }

    /// Exactly at capacity boundary: demand == capacity → outlet == setpoint.
    #[test]
    fn at_capacity_boundary_delivers_setpoint() {
        // demand_w = m_dot * cp * delta_T
        // capacity = demand exactly when m_dot = capacity / (cp * delta_T)
        let delta_t = 30.0_f64;
        let capacity_w = 20_000.0_f64;
        let m_dot = capacity_w / (CP_LIQUID_WATER_J_KG_K * delta_t);

        let mut typed = typed_config();
        typed.draw_flow_rate_kg_s = Some(m_dot);
        typed.energy_factor = Some(0.9);
        typed.heating_capacity_w = Some(capacity_w);
        let cfg = config_from_typed(typed);

        let mut eq = TanklessWH::new(cfg.clone());
        eq.init(&cfg, &env()).unwrap();

        step_once(&mut eq);

        let outlet = eq.telemetry().get(tk::OUTLET_TEMP_C).unwrap();
        let thermal = eq.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap();

        assert!(
            (outlet - 50.0).abs() < 1e-6,
            "at-boundary demand must deliver setpoint (50°C), got {outlet}"
        );
        assert!(
            (thermal - capacity_w).abs() < 1e-6,
            "thermal output must equal capacity ({capacity_w} W), got {thermal}"
        );
    }

    /// Over-capacity with duty cycle < 1: time-averaged output = max_power * duty.
    /// But outlet temperature uses full rated power (heater fires at 100% during on-phase).
    #[test]
    fn over_capacity_with_reduced_duty_cycle() {
        use hares_types::ControlSignal;

        // flow=1.0 kg/s, delta_T=30 K → unclamped demand = 125,490 W
        // max capacity = 20,000 W, duty=0.5 → time-averaged output = 10,000 W
        let mut typed = typed_config();
        typed.draw_flow_rate_kg_s = Some(1.0);
        typed.energy_factor = Some(0.8);
        typed.heating_capacity_w = Some(20_000.0);
        let cfg = config_from_typed(typed);

        let mut eq = TanklessWH::new(cfg.clone());
        eq.init(&cfg, &env()).unwrap();
        eq.apply_control(&ControlSignal::DutyCycle {
            on_fraction: 0.5,
            period_s: None,
            component: None,
        })
        .unwrap();

        step_once(&mut eq);

        let thermal = eq.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap();
        let outlet = eq.telemetry().get(tk::OUTLET_TEMP_C).unwrap();
        let effective_capacity = 20_000.0 * 0.5;

        assert!(
            (thermal - effective_capacity).abs() < 1e-6,
            "thermal must clamp at effective capacity {effective_capacity} W (duty=0.5), got {thermal}"
        );
        assert!(
            outlet < 50.0,
            "outlet must be below setpoint when over effective capacity, got {outlet}"
        );
        // Outlet uses FULL rated power (20 kW), not duty-scaled -- heater fires at 100% during on-phase
        let expected_outlet = 20.0 + 20_000.0 / (1.0 * CP_LIQUID_WATER_J_KG_K);
        assert!(
            (outlet - expected_outlet).abs() < 1e-6,
            "outlet temp must use full rated power: {expected_outlet:.4}°C, got {outlet:.4}"
        );
    }

    /// Over-capacity outlet temperature uses instantaneous rated power, not duty-scaled power.
    /// OCHRE reference: outlet formula uses `capacity_rated`, not `capacity_rated * duty`.
    /// Setup: rated=20 kW, duty=0.5, flow=0.5 kg/s, inlet=20°C.
    /// Expected outlet = 20 + 20000 / (0.5 * CP_LIQUID_WATER_J_KG_K) ≈ 29.56°C (not 24.78°C from 10 kW).
    #[test]
    fn over_capacity_outlet_temp_uses_instantaneous_power() {
        use hares_types::ControlSignal;

        let mut typed = typed_config();
        typed.draw_flow_rate_kg_s = Some(0.5);
        typed.heating_capacity_w = Some(20_000.0);
        typed.energy_factor = Some(0.8);
        typed.inlet_temp_c = Some(20.0);
        typed.setpoint_c = Some(51.666_666_7);
        let cfg = config_from_typed(typed);

        let mut eq = TanklessWH::new(cfg.clone());
        eq.init(&cfg, &env()).unwrap();
        eq.apply_control(&ControlSignal::DutyCycle {
            on_fraction: 0.5,
            period_s: None,
            component: None,
        })
        .unwrap();

        step_once(&mut eq);

        let outlet = eq.telemetry().get(tk::OUTLET_TEMP_C).unwrap();
        let thermal = eq.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap();

        // Time-averaged output = rated * duty = 10,000 W
        assert!(
            (thermal - 10_000.0).abs() < 1e-6,
            "time-averaged thermal must be 10,000 W (rated * duty), got {thermal}"
        );
        // Outlet uses full 20 kW instantaneous, not duty-scaled 10 kW
        let expected_outlet = 20.0 + 20_000.0 / (0.5 * CP_LIQUID_WATER_J_KG_K);
        let wrong_outlet = 20.0 + 10_000.0 / (0.5 * CP_LIQUID_WATER_J_KG_K);
        assert!(
            (outlet - expected_outlet).abs() < 1e-4,
            "outlet must use instantaneous rated power: {expected_outlet:.4}°C, got {outlet:.4}°C (wrong duty-scaled would be {wrong_outlet:.4}°C)"
        );
    }

    /// PowerLimit signal reduces max_thermal_power_w and caps fuel consumption accordingly.
    #[test]
    fn power_limit_signal_caps_fuel_input() {
        use hares_types::ControlSignal;

        // High-flow config to ensure we are in the over-capacity regime.
        let mut typed = typed_config();
        typed.draw_flow_rate_kg_s = Some(1.0);
        typed.energy_factor = Some(0.8);
        typed.heating_capacity_w = Some(20_000.0);
        let cfg = config_from_typed(typed);

        let mut eq = TanklessWH::new(cfg.clone());
        eq.init(&cfg, &env()).unwrap();

        // Limit input power to 10 kW → thermal limit = 10,000 * 0.8 = 8,000 W
        eq.apply_control(&ControlSignal::PowerLimit {
            max_power_kw: 10.0,
            ramp_rate_kw_per_s: None,
        })
        .unwrap();

        step_once(&mut eq);

        let fuel = eq.telemetry().get(tk::FUEL_INPUT_W).unwrap();
        assert!(
            fuel <= 10_000.0 + 1e-6,
            "fuel input ({fuel:.1} W) must not exceed 10,000 W limit"
        );
    }

    /// PowerLimit + over-capacity: outlet temperature uses effective_max_w,
    /// not the full rated_thermal_power_w. A PowerLimit signal reduces the
    /// burner's firing rate, so the instantaneous outlet temperature during
    /// the on-phase must reflect the limited thermal power, not nameplate rating.
    #[test]
    fn power_limit_over_capacity_outlet_temp_uses_effective_max_w() {
        use hares_types::ControlSignal;

        let mut typed = typed_config();
        typed.draw_flow_rate_kg_s = Some(1.0);
        typed.energy_factor = Some(0.8);
        typed.heating_capacity_w = Some(20_000.0);
        let cfg = config_from_typed(typed);

        let mut eq = TanklessWH::new(cfg.clone());
        eq.init(&cfg, &env()).unwrap();

        // PowerLimit { max_power_kw: 5.0 } → thermal limit = 5,000 * 0.8 = 4,000 W.
        // With flow=1.0 kg/s, delta_T demands exceed 4,000 W → over-capacity.
        eq.apply_control(&ControlSignal::PowerLimit {
            max_power_kw: 5.0,
            ramp_rate_kw_per_s: None,
        })
        .unwrap();

        step_once(&mut eq);

        let outlet = eq.telemetry().get(tk::OUTLET_TEMP_C).unwrap();
        let thermal = eq.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap();
        let fuel = eq.telemetry().get(tk::FUEL_INPUT_W).unwrap();

        // effective_max_w = min(20_000, 4_000) = 4_000 W
        let effective_max = 4_000.0;
        // Time-averaged thermal output = effective_max * duty (duty=1.0) = 4,000 W
        assert!(
            (thermal - effective_max).abs() < 1e-6,
            "thermal output must equal effective_max_w {effective_max} W, got {thermal}"
        );
        // Fuel input = thermal / efficiency = 4,000 / 0.8 = 5,000 W
        assert!(
            fuel <= 5_000.0 + 1e-6,
            "fuel input ({fuel:.1} W) must not exceed 5,000 W (5 kW limit / 0.8 eff)"
        );
        // Outlet temp uses effective_max_w (4,000 W), not rated 20,000 W:
        // correct = 20.0 + 4000.0 / (1.0 * CP) ≈ 20.96 °C
        // wrong   = 20.0 + 20000.0 / (1.0 * CP) ≈ 24.78 °C
        let expected_outlet = 20.0 + effective_max / (1.0 * CP_LIQUID_WATER_J_KG_K);
        assert!(
            (outlet - expected_outlet).abs() < 1e-6,
            "outlet temp must use effective_max_w ({effective_max} W): expected {expected_outlet:.4}°C, got {outlet:.4}°C"
        );
    }

    /// Gas tankless always draws parasitic_power_w electrically, even when burner is off.
    #[test]
    fn gas_parasitic_electric_draw_present_when_burner_off() {
        use hares_types::{ControlSignal, OperatingMode};

        let mut eq = TanklessWH::new(config());
        eq.init(&config(), &env()).unwrap();

        // Force the burner off; parasitic standby draw should still appear.
        eq.apply_control(&ControlSignal::ModeOverride {
            mode: OperatingMode::Off,
        })
        .unwrap();

        let ports = step_once(&mut eq);

        assert_eq!(
            eq.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap(),
            0.0,
            "thermal output must be zero when off"
        );
        assert_eq!(
            eq.telemetry().get(tk::FUEL_INPUT_W).unwrap(),
            0.0,
            "fuel input must be zero when burner is off"
        );
        assert_eq!(
            ports.fuel.get(hares_types::FuelType::Gas),
            0.0,
            "gas fuel port must be zero when burner is off"
        );
        // Parasitic electric draw must be non-zero even when off.
        let parasitic_w = 7.38;
        assert!(
            (ports.electrical.load_power_w - parasitic_w).abs() < 1.0,
            "gas standby parasitic electric must be {parasitic_w:.3} W, got {}",
            ports.electrical.load_power_w
        );
    }

    /// Gas tankless draws parasitic_power_w electrically even while burner is firing.
    #[test]
    fn gas_parasitic_electric_draw_present_when_burner_firing() {
        let mut eq = TanklessWH::new(config());
        eq.init(&config(), &env()).unwrap();

        let ports = step_once(&mut eq);

        // Burner should be on (flow > 0, enabled).
        assert!(
            eq.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap() > 0.0,
            "burner must be producing heat in this test"
        );

        // Electrical port must include at least the parasitic draw.
        let parasitic_kw = 7.38 / 1_000.0;
        assert!(
            ports.electrical.load_power_w >= parasitic_kw - 1e-9,
            "gas electrical draw must include at least parasitic_power_w ({parasitic_kw:.6} kW), got {}",
            ports.electrical.load_power_w
        );
        // Fuel port must still have the burner consumption.
        assert!(
            ports.fuel.get(hares_types::FuelType::Gas) > 0.0,
            "gas fuel consumption must be positive when burner fires"
        );
    }

    /// Gas parasitic_power_w is configurable from config.
    #[test]
    fn gas_parasitic_power_configurable() {
        use hares_types::{ControlSignal, OperatingMode};

        let mut typed = typed_config();
        typed.draw_flow_rate_kg_s = Some(0.2);
        typed.energy_factor = Some(0.8);
        typed.parasitic_power_w = Some(15.0);
        let cfg = config_from_typed(typed);

        let mut eq = TanklessWH::new(cfg.clone());
        eq.init(&cfg, &env()).unwrap();

        // Force off to isolate just the parasitic draw.
        eq.apply_control(&ControlSignal::ModeOverride {
            mode: OperatingMode::Off,
        })
        .unwrap();

        let ports = step_once(&mut eq);

        assert!(
            (ports.electrical.load_power_w - 15.0).abs() < 1.0,
            "configured parasitic 15 W must appear in electrical port, got {}",
            ports.electrical.load_power_w
        );
    }

    /// PowerLimit is transient: it resets after each step. After a step without
    /// reapplying PowerLimit, the unit should deliver full rated capacity.
    #[test]
    fn power_limit_resets_after_step() {
        use hares_types::ControlSignal;

        let mut typed = typed_config();
        typed.draw_flow_rate_kg_s = Some(1.0);
        typed.energy_factor = Some(0.8);
        typed.heating_capacity_w = Some(20_000.0);
        let cfg = config_from_typed(typed);

        let mut eq = TanklessWH::new(cfg.clone());
        eq.init(&cfg, &env()).unwrap();

        // Apply PowerLimit → step → limit active
        eq.apply_control(&ControlSignal::PowerLimit {
            max_power_kw: 5.0,
            ramp_rate_kw_per_s: None,
        })
        .unwrap();
        step_once(&mut eq);
        let thermal_limited = eq.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap();

        // Step again WITHOUT reapplying PowerLimit → full capacity restored
        step_once(&mut eq);
        let thermal_full = eq.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap();

        assert!(
            thermal_full > thermal_limited,
            "after PowerLimit reset, thermal should increase: limited={thermal_limited}, full={thermal_full}"
        );
        assert!(
            (thermal_full - 20_000.0).abs() < 1e-6,
            "full capacity should be rated 20 kW, got {thermal_full}"
        );
    }

    /// PowerLimit survives checkpoint (as transient = None after restore, which is correct:
    /// power limits are per-step signals that must be reapplied each step).
    #[test]
    fn power_limit_is_transient_across_checkpoint() {
        use hares_types::ControlSignal;

        let mut typed = typed_config();
        typed.draw_flow_rate_kg_s = Some(1.0);
        typed.energy_factor = Some(0.8);
        typed.heating_capacity_w = Some(20_000.0);
        let cfg = config_from_typed(typed);

        let mut eq = TanklessWH::new(cfg.clone());
        eq.init(&cfg, &env()).unwrap();

        // Apply power limit, save, restore
        eq.apply_control(&ControlSignal::PowerLimit {
            max_power_kw: 5.0,
            ramp_rate_kw_per_s: None,
        })
        .unwrap();

        let state = eq.save_state().unwrap();
        let mut restored = TanklessWH::new(cfg.clone());
        restored.init(&cfg, &env()).unwrap();
        restored.load_state(&state).unwrap();

        // Restored unit should use full rated capacity (power limit is transient)
        step_once(&mut restored);
        let thermal = restored.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap();
        assert!(
            (thermal - 20_000.0).abs() < 1e-6,
            "after restore, full rated capacity should apply: got {thermal}"
        );
    }

    /// Electric tankless must not have any parasitic draw (gas-only feature).
    #[test]
    fn electric_tankless_has_no_parasitic_draw_when_off() {
        use hares_types::{ControlSignal, OperatingMode};

        let mut typed = typed_config();
        typed.fuel_type = FuelType::Electric;
        typed.energy_factor = Some(0.95);
        typed.draw_flow_rate_kg_s = Some(0.2);
        let cfg = config_from_typed(typed);

        let mut eq = TanklessWH::new(cfg.clone());
        eq.init(&cfg, &env()).unwrap();

        eq.apply_control(&ControlSignal::ModeOverride {
            mode: OperatingMode::Off,
        })
        .unwrap();

        let ports = step_once(&mut eq);

        assert_eq!(
            ports.electrical.load_power_w, 0.0,
            "electric tankless must draw zero when off (no parasitic)"
        );
    }

    /// Verify the tankless WH reads mains temp from canonical weather
    /// rather than the configured inlet temperature.
    ///
    /// Two environments with different mains temps (5°C vs 25°C) injected via
    /// `weather.mains_temp_c` must produce different thermal outputs: a colder
    /// inlet requires more energy to reach setpoint.
    #[test]
    fn tankless_wh_uses_dynamic_mains_temp_from_environment() {
        fn env_with_mains(mains_c: f64) -> EnvironmentState {
            EnvironmentState {
                zones: vec![ZoneState {
                    id: ZoneId(1),
                    temperature_c: 21.0,
                    humidity_ratio: 0.008,
                    volume_m3: 200.0,
                }],
                weather: WeatherState {
                    outdoor_temp_c: 10.0,
                    mains_temp_c: mains_c,
                    ..Default::default()
                },
                grid: hares_types::GridState {
                    voltage_pu: 1.0,
                    frequency_hz: 60.0,
                    island_bus_voltage_pu: None,
                },
                custom_domains: vec![],
                equipment_telemetry: std::collections::HashMap::new(),
                equipment_core: std::collections::HashMap::new(),
                current_time: FixedOffset::east_opt(0)
                    .expect("UTC offset")
                    .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                    .single()
                    .expect("valid"),
                time_res: ChronoDuration::seconds(60),
                price_signal: Default::default(),
                electrical: Default::default(),
            }
        }

        let cfg = {
            let mut typed = typed_config();
            typed.heating_capacity_w = Some(60_000.0);
            typed.energy_factor = Some(0.9);
            typed.draw_flow_rate_kg_s = Some(0.2);
            // inlet_temp_c is the configured default; the dynamic value should override it.
            config_from_typed(typed)
        };

        let cold_env = env_with_mains(5.0);
        let warm_env = env_with_mains(25.0);

        let mut eq_cold = TanklessWH::new(cfg.clone());
        eq_cold.init(&cfg, &cold_env).unwrap();
        let mut ports_cold = PortSlots::default();
        eq_cold
            .step(&cold_env, Duration::from_secs(60), &mut ports_cold)
            .unwrap();
        let thermal_cold = eq_cold.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap_or(0.0);

        let mut eq_warm = TanklessWH::new(cfg.clone());
        eq_warm.init(&cfg, &warm_env).unwrap();
        let mut ports_warm = PortSlots::default();
        eq_warm
            .step(&warm_env, Duration::from_secs(60), &mut ports_warm)
            .unwrap();
        let thermal_warm = eq_warm.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap_or(0.0);

        assert!(
            thermal_cold > thermal_warm,
            "cold inlet (5°C) must require more thermal energy than warm inlet (25°C); \
             cold={thermal_cold:.1}W warm={thermal_warm:.1}W"
        );

        // Verify the values align with exact physics: Q = m_dot * Cp * (Tset - Tinlet)
        let cp = CP_LIQUID_WATER_J_KG_K;
        let m_dot = 0.2_f64;
        let tset = 50.0_f64;
        let expected_cold = m_dot * cp * (tset - 5.0);
        let expected_warm = m_dot * cp * (tset - 25.0);
        assert!(
            (thermal_cold - expected_cold).abs() < 1.0,
            "thermal output with 5°C inlet must be ≈{expected_cold:.1}W, got {thermal_cold:.1}W"
        );
        assert!(
            (thermal_warm - expected_warm).abs() < 1.0,
            "thermal output with 25°C inlet must be ≈{expected_warm:.1}W, got {thermal_warm:.1}W"
        );
    }

    /// The serde default for `WeatherState.mains_temp_c` must be 10°C (ASHRAE/EnergyPlus US annual
    /// average), not 15°C.  When deserializing a JSON object that omits `mains_temp_c`, the field
    /// must default to 10.0.
    #[test]
    fn default_mains_temp_c_is_10() {
        let json = serde_json::json!({
            "outdoor_temp_c": 5.0,
            "outdoor_humidity_ratio": 0.005,
            "outdoor_wet_bulb_c": 3.0,
            "outdoor_enthalpy_j_kg": 15000.0,
            "wind_speed_m_s": 2.0,
            "wind_dir_deg": 180.0,
            "ground_temp_c": 8.0,
            "sky_temp_c": 2.0,
            "pressure_kpa": 101.3,
            "solar_irradiance": [],
            "ghi_w_m2": 0.0,
            "dni_w_m2": 0.0,
            "dhi_w_m2": 0.0
        });
        let w: hares_types::WeatherState =
            serde_json::from_value(json).expect("deserialize WeatherState without mains_temp_c");
        assert!(
            (w.mains_temp_c - 10.0).abs() < f64::EPSILON,
            "WeatherState serde default mains_temp_c must be 10.0, got {}",
            w.mains_temp_c
        );
    }

    /// Parasitic power is passed through directly from config.
    #[test]
    fn gas_parasitic_power_passed_through() {
        let mut typed = typed_config();
        typed.fuel_type = FuelType::Gas;
        typed.parasitic_power_w = Some(10.0);
        typed.draw_flow_rate_kg_s = Some(0.0);
        let cfg = config_from_typed(typed);

        let mut eq = TanklessWH::new(cfg.clone());
        eq.init(&cfg, &env()).unwrap();

        let ports = step_once(&mut eq);
        assert!(
            (ports.electrical.load_power_w - 10.0).abs() < 1.0,
            "parasitic power must be 10 W, got {} W",
            ports.electrical.load_power_w
        );
    }

    // --- Minimum flow threshold tests ---

    /// With min_flow_kg_s = 0.03, flow of 0.02 kg/s is below the threshold
    /// and must produce zero thermal output.
    #[test]
    fn flow_below_min_threshold_produces_zero_thermal_output() {
        let mut typed = typed_config();
        typed.draw_flow_rate_kg_s = Some(0.02);
        typed.min_flow_kg_s = Some(0.03);
        let cfg = config_from_typed(typed);

        let mut eq = TanklessWH::new(cfg.clone());
        eq.init(&cfg, &env()).unwrap();

        step_once(&mut eq);

        assert_eq!(
            eq.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap(),
            0.0,
            "flow 0.02 kg/s below min_flow_kg_s=0.03 must produce zero thermal output"
        );
        assert_eq!(
            eq.telemetry().get(tk::FUEL_INPUT_W).unwrap(),
            0.0,
            "flow 0.02 kg/s below min_flow_kg_s=0.03 must produce zero fuel input"
        );
    }

    /// With min_flow_kg_s = 0.03, flow of 0.04 kg/s exceeds the threshold
    /// and must produce non-zero thermal output.
    #[test]
    fn flow_above_min_threshold_produces_thermal_output() {
        let mut typed = typed_config();
        typed.draw_flow_rate_kg_s = Some(0.04);
        typed.min_flow_kg_s = Some(0.03);
        let cfg = config_from_typed(typed);

        let mut eq = TanklessWH::new(cfg.clone());
        eq.init(&cfg, &env()).unwrap();

        step_once(&mut eq);

        assert!(
            eq.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap() > 0.0,
            "flow 0.04 kg/s above min_flow_kg_s=0.03 must produce non-zero thermal output"
        );
        assert!(
            eq.telemetry().get(tk::FUEL_INPUT_W).unwrap() > 0.0,
            "flow 0.04 kg/s above min_flow_kg_s=0.03 must produce non-zero fuel input"
        );
    }

    /// When min_flow_kg_s is not specified (default 0.0), a very small
    /// positive flow must still produce thermal output (backward compatibility).
    #[test]
    fn default_min_flow_threshold_allows_any_positive_flow() {
        // 0.001 kg/s is a tiny flow — with default threshold 0.0 it must still fire.
        let mut typed = typed_config();
        typed.draw_flow_rate_kg_s = Some(0.001);
        let cfg = config_from_typed(typed);

        let mut eq = TanklessWH::new(cfg.clone());
        eq.init(&cfg, &env()).unwrap();

        step_once(&mut eq);

        assert!(
            eq.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap() > 0.0,
            "default min_flow_kg_s=0.0 must allow tiny positive flows to fire"
        );
    }

    /// min_flow_gpm convenience input converts to kg/s and gates the burner
    /// the same as min_flow_kg_s. 0.5 GPM ≈ 0.0315 kg/s; flow 0.02 kg/s
    /// is below this threshold and must produce zero output.
    #[test]
    fn min_flow_gpm_convenience_input_gates_burner() {
        use hares_physics::constants::GALLONS_PER_MINUTE_TO_KG_PER_SECOND;

        let mut typed = typed_config();
        typed.draw_flow_rate_kg_s = Some(0.02);
        typed.min_flow_gpm = Some(0.5);
        let cfg = config_from_typed(typed);

        let mut eq = TanklessWH::new(cfg.clone());
        eq.init(&cfg, &env()).unwrap();

        let threshold = 0.5 * GALLONS_PER_MINUTE_TO_KG_PER_SECOND;
        assert!(
            threshold > 0.02,
            "0.5 GPM ({threshold:.6} kg/s) must be above test flow 0.02 kg/s"
        );

        step_once(&mut eq);

        assert_eq!(
            eq.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap(),
            0.0,
            "flow 0.02 kg/s below min_flow_gpm=0.5 ({threshold:.6} kg/s) must produce zero output"
        );
    }

    /// When both min_flow_kg_s and min_flow_gpm are set, min_flow_kg_s takes
    /// precedence (it is the direct internal unit).
    #[test]
    fn min_flow_kg_s_takes_precedence_over_min_flow_gpm() {
        let mut typed = typed_config();
        typed.draw_flow_rate_kg_s = Some(0.02);
        typed.min_flow_kg_s = Some(0.01);
        typed.min_flow_gpm = Some(1.0);
        let cfg = config_from_typed(typed);

        let mut eq = TanklessWH::new(cfg.clone());
        eq.init(&cfg, &env()).unwrap();

        step_once(&mut eq);

        assert!(
            eq.telemetry().get(tk::THERMAL_OUTPUT_W).unwrap() > 0.0,
            "flow 0.02 kg/s must fire when min_flow_kg_s=0.01 takes precedence"
        );
    }
}
