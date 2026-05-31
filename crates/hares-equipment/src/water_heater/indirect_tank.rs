//! Indirect tank water heater model (combi boiler + storage tank).
//!
//! The indirect tank is a DHW storage tank heated by a boiler's hydronic loop
//! via an internal heat exchanger (HX). It has no internal heat source —
//! a separate boiler provides primary energy. The tank reads boiler loop
//! supply temperature from the FLUID domain, computes HX heat transfer, and
//! writes a cooler return temperature back to the boiler loop.
//!
//! HPXML `WaterHeatingType = "space-heating boiler with storage tank"`

use std::borrow::Cow;
use std::time::Duration;

use hares_physics::constants::cp_j_kg_k;
use hares_physics::water_density_kg_m3;
use hares_types::{
    ControlCapabilities, ControlSignal, CoreCapabilities, CoreFlows, CoreOutput, CorePerformance,
    CoreState, DRLevel, EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId, ExecutionStage,
    FluidType, FuelType, HaresError, LoopId, OperatingMode, PortContribution, PortDeclaration,
    PortSlots, ScheduleSource, Telemetry, TelemetryField, ThermalCategory, ZoneId,
    telemetry_keys as tk,
};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_versioned, save_versioned};

use super::tank::{StratifiedTank, StratifiedTankConfig, TemperedDrawConfig};
use super::{
    WaterHeaterZip, hysteresis_call, parse_usize, resolve_storage_step_inputs,
    weighted_average_tank_temp,
};
use crate::hvac::helpers::{equipment_id_from_config, zone_id_from_config_or_default};

use super::wh_config::IndirectTankConfig;
use super::{
    DEFAULT_CONDUCTIVITY_W_M_K, DEFAULT_MAX_TANK_TEMP_C, DEFAULT_SETPOINT_C,
    DEFAULT_TANK_DIAMETER_M, DEFAULT_TANK_HEIGHT_M, DEFAULT_TANK_VOLUME_M3, DEFAULT_UA_W_PER_K,
};

/// Default HX UA coefficient [W/K] for a copper-coil heat exchanger
/// in a 50-gal indirect tank.
const DEFAULT_HX_UA_W_PER_K: f64 = 150.0;
const DEFAULT_DEADBAND_C: f64 = 5.555_555_556; // 10°F
/// Default boiler loop assumed flow rate [kg/s].
/// Typical residential hydronic loop: Grundfos UPS15-58 on speed 1 ≈ 6 GPM (0.38 kg/s)
/// for a 10 ft head. We use 0.1 kg/s as a conservative low-flow default
/// that produces safe (large ΔT) return temperature estimates when actual
/// boiler flow is unknown.
const DEFAULT_BOILER_LOOP_FLOW_RATE_KG_S: f64 = 0.1;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct IndirectTankState {
    setpoint_c: f64,
    target_setpoint_c: f64,
    deadband_c: f64,
    boiler_loop_id: LoopId,
    hx_ua_w_per_k: f64,
    boiler_loop_flow_rate_kg_s: f64,
    heating_on: bool,
    duty_cycle: f64,
    mode_override: Option<OperatingMode>,
    tank_state: Vec<u8>,
    tank_avg_temp_c: f64,
    hx_power_w: f64,
    draw_flow_rate_kg_s: f64,
    dr_level: DRLevel,
    dr_setpoint_offset_c: f64,
    dr_load_fraction: f64,
    dr_duration_remaining_s: Option<f64>,
}

pub struct IndirectTank {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,
    core_output: CoreOutput,
    tank: StratifiedTank,
    boiler_loop_id: LoopId,
    hx_ua_w_per_k: f64,
    setpoint_c: f64,
    deadband_c: f64,
    max_tank_temp_c: f64,
    heating_on: bool,
    duty_cycle: f64,
    mode_override: Option<OperatingMode>,
    fluid_type: FluidType,
    mains_temp_c: f64,
    draw_flow_rate_kg_s: f64,
    draw_l_per_min_source: Option<ScheduleSource>,
    mains_temp_c_source: Option<ScheduleSource>,
    zip: WaterHeaterZip,
    target_setpoint_c: f64,
    fixture_delivery_temp_c: f64,
    hot_draw_temp_c: Option<f64>,
    dr_setpoint_offset_c: f64,
    dr_load_fraction: f64,
    dr_duration_remaining_s: Option<f64>,
    dr_level: DRLevel,
    boiler_loop_flow_rate_kg_s: f64,
    ctrl_load_fraction: f64,
    /// Whether zone_id was explicitly set in config or fell back to ZoneId(1).
    zone_id_explicit: bool,
}

impl IndirectTank {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let (zone, zone_id_explicit) = zone_id_from_config_or_default(&config, &config.name);
        let boiler_loop_id =
            crate::hvac::helpers::loop_id_from_config(&config, &["boiler_loop_id", "loop_id"])
                .unwrap_or_default();
        let n_nodes = parse_usize(config.get_f64("tank_nodes"))
            .unwrap_or(6)
            .clamp(1, 12);

        let tank = StratifiedTank::new(StratifiedTankConfig {
            n_nodes,
            height_m: DEFAULT_TANK_HEIGHT_M,
            diameter_m: DEFAULT_TANK_DIAMETER_M,
            ua_w_per_k: DEFAULT_UA_W_PER_K,
            conductivity_w_m_k: DEFAULT_CONDUCTIVITY_W_M_K,
            initial_temp_c: DEFAULT_SETPOINT_C,
            element_nodes: [None, None],
            node_volumes_m3: None,
            ua_end_cap_w_per_k: None,
        })
        .expect("default indirect-tank config must be valid");

        let fields = telemetry_fields(n_nodes);
        let mut telemetry = default_telemetry();
        tank.register_node_telemetry(&mut telemetry);

        Self {
            descriptor: EquipmentDescriptor {
                id: EquipmentId(equipment_id_from_config(&config).unwrap_or(0)),
                name: config.name,
                end_use: EndUse::WATER_HEATING,
                equipment_type: Cow::Borrowed("Indirect Tank"),
                zone: Some(zone),
                fuel: FuelType::Electric,
                stage: ExecutionStage::Thermal,
                control_capabilities: ControlCapabilities::THERMAL_SETPOINT
                    | ControlCapabilities::DUTY_CYCLE
                    | ControlCapabilities::MODE_OVERRIDE
                    | ControlCapabilities::LOAD_FRACTION
                    | ControlCapabilities::DEMAND_RESPONSE,
                core_capabilities: CoreCapabilities::HAS_MODE | CoreCapabilities::THERMAL,
                telemetry_fields: fields,
                zone_type: None,
            },
            ports: vec![
                PortDeclaration::fluid(boiler_loop_id, FluidType::Water),
                PortDeclaration::fluid(super::DHW_DEMAND_LOOP, FluidType::Water),
                PortDeclaration::thermal(zone),
            ],
            telemetry,
            core_output: CoreOutput::default(),
            tank,
            boiler_loop_id,
            hx_ua_w_per_k: DEFAULT_HX_UA_W_PER_K,
            setpoint_c: DEFAULT_SETPOINT_C,
            deadband_c: DEFAULT_DEADBAND_C,
            max_tank_temp_c: DEFAULT_MAX_TANK_TEMP_C,
            heating_on: false,
            duty_cycle: 1.0,
            mode_override: None,
            fluid_type: FluidType::Water,
            mains_temp_c: 10.0,
            draw_flow_rate_kg_s: 0.0,
            draw_l_per_min_source: None,
            mains_temp_c_source: None,
            zip: WaterHeaterZip::default(),
            target_setpoint_c: DEFAULT_SETPOINT_C,
            fixture_delivery_temp_c: 40.6,
            hot_draw_temp_c: None,
            dr_setpoint_offset_c: 0.0,
            dr_load_fraction: 1.0,
            dr_duration_remaining_s: None,
            dr_level: DRLevel::Normal,
            boiler_loop_flow_rate_kg_s: DEFAULT_BOILER_LOOP_FLOW_RATE_KG_S,
            ctrl_load_fraction: 1.0,
            zone_id_explicit,
        }
    }

    fn effective_setpoint_c(&self) -> f64 {
        self.setpoint_c + self.dr_setpoint_offset_c
    }

    fn bottom_temp_c(&self) -> f64 {
        let temps = self.tank.node_temps();
        let n = temps.len();
        if n >= 3 {
            (temps[n - 1] + temps[n - 2]) * 0.5
        } else {
            temps[0]
        }
    }

    fn ambient_temp_c(&self, env: &EnvironmentState) -> f64 {
        self.descriptor
            .zone
            .and_then(|zone| {
                env.zones
                    .iter()
                    .find(|z| z.id == zone)
                    .map(|z| z.temperature_c)
            })
            .unwrap_or(env.weather.outdoor_temp_c)
    }

    fn read_boiler_supply_temp_c(&self, ports: &PortSlots) -> f64 {
        ports
            .fluid
            .iter()
            .find(|acc| acc.loop_id == self.boiler_loop_id)
            .map(|acc| acc.mean_supply_temp_c)
            .unwrap_or(0.0)
    }
}

impl IndirectTank {
    fn init_typed(
        &mut self,
        config: &EquipmentConfig,
        env: &EnvironmentState,
    ) -> crate::Result<()> {
        let c = config.require_typed::<IndirectTankConfig>("Indirect Tank")?;
        c.validate()?;

        self.descriptor.id = EquipmentId(c.equipment_id.unwrap_or(self.descriptor.id.0));

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        if c.zone_id.is_none() && c.zone_type.is_some() {
            warn!(
                water_heater = %config.name,
                zone_type = ?c.zone_type,
                "zone_id not resolved from HPXML Location; falling back to ZoneId(1)"
            );
        }

        let zone = c.zone_id.map(ZoneId).or(self.descriptor.zone);
        self.descriptor.zone = zone;
        self.descriptor.zone_type = c.zone_type.clone();
        self.ports[2].zone = zone;

        let boiler_loop_id = c.boiler_loop_id.map(LoopId).unwrap_or(self.boiler_loop_id);
        self.boiler_loop_id = boiler_loop_id;
        self.ports[0].loop_id = Some(boiler_loop_id);

        let tank_volume_m3 = c.tank_volume_m3.unwrap_or(DEFAULT_TANK_VOLUME_M3);
        let height_m = c.tank_height_m.unwrap_or(DEFAULT_TANK_HEIGHT_M).max(0.2);
        let diameter_m = (4.0 * tank_volume_m3 / (std::f64::consts::PI * height_m))
            .max(1e-6)
            .sqrt();
        let n_nodes = usize::from(c.tank_nodes.unwrap_or(6).max(1));

        self.tank = StratifiedTank::new(StratifiedTankConfig {
            n_nodes,
            height_m,
            diameter_m,
            ua_w_per_k: super::apply_jacket_r_value(
                c.ua_w_per_k.unwrap_or(DEFAULT_UA_W_PER_K),
                height_m,
                diameter_m,
                c.jacket_r_value_m2_k_w,
            ),
            conductivity_w_m_k: DEFAULT_CONDUCTIVITY_W_M_K,
            initial_temp_c: c
                .initial_tank_temp_c
                .unwrap_or(c.setpoint_c.unwrap_or(DEFAULT_SETPOINT_C)),
            element_nodes: [None, None],
            node_volumes_m3: None,
            ua_end_cap_w_per_k: None,
        })?;

        self.hx_ua_w_per_k = c.hx_ua_w_per_k.unwrap_or(DEFAULT_HX_UA_W_PER_K).max(0.0);
        self.setpoint_c = c.setpoint_c.unwrap_or(DEFAULT_SETPOINT_C);
        self.deadband_c = c.deadband_c.unwrap_or(DEFAULT_DEADBAND_C);
        self.max_tank_temp_c = c.max_tank_temp_c.unwrap_or(DEFAULT_MAX_TANK_TEMP_C);
        self.duty_cycle = 1.0;
        self.mode_override = None;
        self.heating_on = false;

        self.mains_temp_c = super::require_mains_temp_c(env, "Indirect Tank")?;
        self.draw_flow_rate_kg_s = c.draw_flow_rate_kg_s.unwrap_or(0.0);
        self.draw_l_per_min_source = c.draw_flow_rate_source.clone().map(|s| s.into_runtime());
        self.mains_temp_c_source = c.mains_temp_c_source.clone().map(|s| s.into_runtime());
        self.zip = WaterHeaterZip::default();

        self.target_setpoint_c = self.setpoint_c;
        self.fixture_delivery_temp_c = c.fixture_delivery_temp_c.unwrap_or(40.6);
        self.hot_draw_temp_c = c.hot_draw_temp_c;
        self.boiler_loop_flow_rate_kg_s = c
            .boiler_loop_flow_rate_kg_s
            .unwrap_or(DEFAULT_BOILER_LOOP_FLOW_RATE_KG_S);

        self.dr_setpoint_offset_c = 0.0;
        self.dr_load_fraction = 1.0;
        self.dr_duration_remaining_s = None;
        self.dr_level = DRLevel::Normal;
        self.ctrl_load_fraction = 1.0;

        let fields = telemetry_fields(n_nodes);
        self.descriptor.telemetry_fields = fields;
        self.telemetry = {
            let mut t = default_telemetry();
            self.tank.register_node_telemetry(&mut t);
            t
        };
        self.core_output = CoreOutput::default();
        Ok(())
    }

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

impl Equipment for IndirectTank {
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
        self.init_typed(config, env)
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
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

        let max_node_temp = self
            .tank
            .node_temps()
            .iter()
            .copied()
            .reduce(f64::max)
            .expect("node_temps is never empty");
        if !max_node_temp.is_finite() || max_node_temp > self.max_tank_temp_c {
            self.heating_on = false;
            return OperatingMode::Off;
        }

        if self.dr_load_fraction <= 0.0
            || matches!(self.mode_override, Some(OperatingMode::Off))
            || self.duty_cycle <= 0.0
        {
            self.heating_on = false;
            return OperatingMode::Off;
        }

        let bottom_temp = self.bottom_temp_c();
        let effective_sp = self.effective_setpoint_c();
        self.heating_on =
            hysteresis_call(bottom_temp, effective_sp, self.deadband_c, self.heating_on);

        if self.heating_on {
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
        let dt_s = dt.as_secs_f64();

        let boiler_supply_c = self.read_boiler_supply_temp_c(ports);
        let bottom_tank_temp = self.bottom_temp_c();
        let delta_t_hx = (boiler_supply_c - bottom_tank_temp).max(0.0);

        let hx_power_w = if self.heating_on && delta_t_hx > 0.5 {
            let max_hx_power = self.hx_ua_w_per_k * delta_t_hx;
            max_hx_power * self.duty_cycle * self.dr_load_fraction * self.ctrl_load_fraction
        } else {
            0.0
        };

        let hx_energy_j = hx_power_w * dt_s;
        let mut heat_buf = [(0usize, 0.0f64); 1];
        let mut n_heat = 0;
        if hx_power_w > 0.0 {
            let bottom_node = self.tank.node_temps().len().saturating_sub(1);
            heat_buf[0] = (bottom_node, hx_power_w);
            n_heat = 1;
        }
        let heat_injections = &heat_buf[..n_heat];

        let draw_l_per_min_source = self.draw_l_per_min_source.as_mut();
        let mains_temp_c_source = self.mains_temp_c_source.as_mut();
        let (mains_temp_c, draw_flow_rate_kg_s) = resolve_storage_step_inputs(
            env,
            self.mains_temp_c,
            self.draw_flow_rate_kg_s,
            draw_l_per_min_source,
            mains_temp_c_source,
        );
        let appliance_demand_kg_s = super::read_dhw_demand_kg_s(ports);
        let total_draw_kg_s = draw_flow_rate_kg_s + appliance_demand_kg_s;

        let hot_draw_temp_c = self.hot_draw_temp_c.unwrap_or(self.setpoint_c);
        let tmv = TemperedDrawConfig {
            tempered_draw_temp_c: self.fixture_delivery_temp_c,
            hot_draw_temp_c,
            setpoint_temp_c: self.setpoint_c,
        };
        let tempered_flow_m3_s =
            draw_flow_rate_kg_s / water_density_kg_m3(self.tank.node_temps()[0]);
        let hot_flow_m3_s = appliance_demand_kg_s / water_density_kg_m3(self.tank.node_temps()[0]);
        let draw = self.tank.step_tempered(
            self.ambient_temp_c(env),
            tempered_flow_m3_s,
            hot_flow_m3_s,
            mains_temp_c,
            heat_injections,
            tmv,
            dt,
        )?;

        // Write boiler loop return: mass-conserving energy balance.
        let boiler_flow_kg_s = self.boiler_loop_flow_rate_kg_s;
        if hx_power_w > 0.0 {
            let return_temp_c = boiler_supply_c
                - hx_energy_j / (boiler_flow_kg_s * cp_j_kg_k(self.fluid_type) * dt_s.max(1e-6));
            let return_temp_finite = return_temp_c.max(0.0).min(boiler_supply_c);
            ports.accumulate(&PortContribution::Fluid {
                loop_id: self.boiler_loop_id,
                flow_rate_kg_s: boiler_flow_kg_s,
                supply_temp_c: boiler_supply_c,
                return_temp_c: return_temp_finite,
                fluid_type: self.fluid_type,
                thermal_power_w: None,
            })?;
        }

        // DHW draw to distribution loop
        if total_draw_kg_s > 0.0 {
            ports.accumulate(&PortContribution::Fluid {
                loop_id: super::DHW_DEMAND_LOOP,
                flow_rate_kg_s: total_draw_kg_s,
                supply_temp_c: draw.outlet_temp_c,
                return_temp_c: mains_temp_c,
                fluid_type: self.fluid_type,
                thermal_power_w: None,
            })?;
        }

        // Jacket loss to zone
        let skin_loss_w = self.tank.skin_loss_w();
        if let Some(zone) = self.descriptor.zone {
            if skin_loss_w.abs() > 1e-3 {
                ports.accumulate(&PortContribution::Thermal {
                    zone,
                    sensible_gain_w: skin_loss_w,
                    radiant_gain_w: 0.0,
                    latent_gain_w: 0.0,
                    category: ThermalCategory::JacketLoss,
                })?;
            }
        }

        let avg_temp_c =
            weighted_average_tank_temp(self.tank.node_temps(), self.tank.node_volumes_m3());
        self.telemetry.set(tk::TANK_AVG_TEMP_C, avg_temp_c);
        self.telemetry.set(tk::ELECTRIC_KW, 0.0);
        self.telemetry.set(tk::ELECTRIC_POWER_W, hx_power_w);
        self.telemetry.set(tk::DRAW_FLOW_RATE_KG_S, total_draw_kg_s);
        self.telemetry.set(tk::UNMET_LOAD_W, draw.unmet_load_w);
        self.telemetry.set(tk::OUTLET_TEMP_C, draw.outlet_temp_c);
        self.telemetry.set(
            tk::OPERATING_MODE,
            if mode == OperatingMode::Heating {
                1.0
            } else {
                0.0
            },
        );
        self.tank.update_node_telemetry(&mut self.telemetry);

        let core_output = CoreOutput {
            flows: CoreFlows {
                electric_kw: None,
                reactive_power_kvar: None,
                fuel_w: None,
                thermal_output_w: Some(hx_power_w.max(0.0)),
                sensible_cooling_w: None,
                latent_cooling_w: None,
            },
            state: CoreState {
                operating_mode: Some(mode),
                soc: None,
                speed_index: None,
                setpoint_c: None,
            },
            performance: CorePerformance {
                cop: None,
                main_power_kw: None,
            },
        };
        self.core_output = core_output;
        self.ctrl_load_fraction = 1.0;
        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn core_output(&self) -> &CoreOutput {
        &self.core_output
    }

    fn save_state(&self) -> Vec<u8> {
        save_versioned(
            &IndirectTankState {
                setpoint_c: self.setpoint_c,
                target_setpoint_c: self.target_setpoint_c,
                deadband_c: self.deadband_c,
                boiler_loop_id: self.boiler_loop_id,
                hx_ua_w_per_k: self.hx_ua_w_per_k,
                boiler_loop_flow_rate_kg_s: self.boiler_loop_flow_rate_kg_s,
                heating_on: self.heating_on,
                duty_cycle: self.duty_cycle,
                mode_override: self.mode_override,
                tank_state: self.tank.save_state(),
                tank_avg_temp_c: self
                    .telemetry
                    .get(tk::TANK_AVG_TEMP_C)
                    .unwrap_or(DEFAULT_SETPOINT_C),
                hx_power_w: self.telemetry.get(tk::ELECTRIC_POWER_W).unwrap_or(0.0),
                draw_flow_rate_kg_s: self.draw_flow_rate_kg_s,
                dr_level: self.dr_level,
                dr_setpoint_offset_c: self.dr_setpoint_offset_c,
                dr_load_fraction: self.dr_load_fraction,
                dr_duration_remaining_s: self.dr_duration_remaining_s,
            },
            Self::checkpoint_version(),
            "IndirectTank",
        )
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let decoded: IndirectTankState = load_versioned(
            state,
            Self::checkpoint_version(),
            "IndirectTank",
            self.descriptor().id,
        )?;
        self.setpoint_c = decoded.setpoint_c;
        self.target_setpoint_c = decoded.target_setpoint_c;
        self.deadband_c = decoded.deadband_c;
        self.boiler_loop_id = decoded.boiler_loop_id;
        self.hx_ua_w_per_k = decoded.hx_ua_w_per_k;
        self.boiler_loop_flow_rate_kg_s = decoded.boiler_loop_flow_rate_kg_s;
        self.heating_on = decoded.heating_on;
        self.duty_cycle = decoded.duty_cycle;
        self.mode_override = decoded.mode_override;
        self.tank.load_state(&decoded.tank_state)?;
        self.telemetry
            .set(tk::TANK_AVG_TEMP_C, decoded.tank_avg_temp_c);
        self.telemetry.set(tk::ELECTRIC_POWER_W, decoded.hx_power_w);
        self.draw_flow_rate_kg_s = decoded.draw_flow_rate_kg_s;
        self.dr_level = decoded.dr_level;
        self.dr_setpoint_offset_c = decoded.dr_setpoint_offset_c;
        self.dr_load_fraction = decoded.dr_load_fraction;
        self.dr_duration_remaining_s = decoded.dr_duration_remaining_s;
        self.core_output = CoreOutput::default();
        Ok(())
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        match signal {
            ControlSignal::ThermalSetpoint {
                heating_setpoint_c,
                deadband_c,
                ..
            } => {
                if let Some(sp) = heating_setpoint_c {
                    self.target_setpoint_c = *sp;
                    self.setpoint_c = *sp;
                }
                if let Some(db) = deadband_c {
                    if !db.is_finite() || *db <= 0.0 {
                        return Err(HaresError::Control(format!(
                            "invalid water-heater deadband: {db}"
                        )));
                    }
                    self.deadband_c = *db;
                }
            }
            ControlSignal::DutyCycle { on_fraction, .. } => {
                if !on_fraction.is_finite() || !(0.0..=1.0).contains(on_fraction) {
                    return Err(HaresError::Control(format!(
                        "invalid duty cycle for IndirectTank: {on_fraction}"
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
        "Indirect Tank",
        Box::new(|config| Box::new(IndirectTank::new(config))),
    );
}

fn telemetry_fields(n_nodes: usize) -> Vec<TelemetryField> {
    let mut fields = vec![
        TelemetryField {
            name: tk::TANK_AVG_TEMP_C.to_string(),
            unit: "C".to_string(),
            description: "Volume-weighted average tank temperature".to_string(),
        },
        TelemetryField {
            name: tk::ELECTRIC_KW.to_string(),
            unit: "kW".to_string(),
            description: "Electric power (always 0 for indirect tank)".to_string(),
        },
        TelemetryField {
            name: tk::ELECTRIC_POWER_W.to_string(),
            unit: "W".to_string(),
            description: "HX heat transfer power".to_string(),
        },
        TelemetryField {
            name: tk::DRAW_FLOW_RATE_KG_S.to_string(),
            unit: "kg/s".to_string(),
            description: "Total DHW draw flow rate".to_string(),
        },
        TelemetryField {
            name: tk::UNMET_LOAD_W.to_string(),
            unit: "W".to_string(),
            description: "Unmet DHW load".to_string(),
        },
        TelemetryField {
            name: tk::OUTLET_TEMP_C.to_string(),
            unit: "C".to_string(),
            description: "DHW outlet temperature".to_string(),
        },
        TelemetryField {
            name: tk::SKIN_LOSS_W.to_string(),
            unit: "W".to_string(),
            description: "Tank jacket thermal loss to zone".to_string(),
        },
        TelemetryField {
            name: tk::OPERATING_MODE.to_string(),
            unit: "".to_string(),
            description: "Operating mode (1=heating, 0=off)".to_string(),
        },
    ];
    for i in 0..n_nodes {
        fields.push(TelemetryField {
            name: format!("tank_node_{i}_temp_c"),
            unit: "C".to_string(),
            description: format!("Tank node {i} temperature"),
        });
    }
    fields
}

fn default_telemetry() -> Telemetry {
    let mut telemetry = Telemetry::with_capacity(10);
    telemetry.insert(tk::TANK_AVG_TEMP_C, 0.0);
    telemetry.insert(tk::ELECTRIC_KW, 0.0);
    telemetry.insert(tk::ELECTRIC_POWER_W, 0.0);
    telemetry.insert(tk::DRAW_FLOW_RATE_KG_S, 0.0);
    telemetry.insert(tk::UNMET_LOAD_W, 0.0);
    telemetry.insert(tk::OUTLET_TEMP_C, 0.0);
    telemetry.insert(tk::OPERATING_MODE, 0.0);
    telemetry
}
