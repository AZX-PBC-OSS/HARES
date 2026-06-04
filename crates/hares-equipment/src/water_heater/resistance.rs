//! Electric resistance water heater model.

use std::borrow::Cow;
use std::time::Duration;

use hares_physics::units::{power_kw_to_w, power_w_to_kw};
use hares_physics::water_density_kg_m3;
use hares_types::{
    ControlCapabilities, ControlSignal, CoreCapabilities, CoreFlows, CoreOutput, CorePerformance,
    CoreState, DRLevel, ElectricPower, EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId,
    ExecutionStage, FluidType, FuelType, HaresError, LoopId, OperatingMode, PortContribution,
    PortDeclaration, PortSlots, ScheduleSource, Telemetry, TelemetryField, ThermalCategory, ZoneId,
    telemetry_keys as tk,
};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_versioned, try_save_versioned};

use super::tank::{StratifiedTank, StratifiedTankConfig, TemperedDrawConfig};
use super::{
    WaterHeaterZip, hysteresis_call, parse_usize, resolve_storage_step_inputs,
    weighted_average_tank_temp,
};
use crate::hvac::helpers::{
    equipment_id_from_config, loop_id_from_config, zone_id_from_config_or_default,
};

/// Element priority control mode for dual-element electric resistance water heaters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ElementPriorityMode {
    /// Upper element (master) locks out lower element (slave) when firing.
    /// This is the standard wiring for most residential electric water heaters.
    #[default]
    MasterSlave,
    /// Both elements operate independently based on their own thermostat calls.
    Simultaneous,
}

use super::wh_config::ElectricResistanceWaterHeaterConfig;
use super::{
    DEFAULT_CONDUCTIVITY_W_M_K, DEFAULT_MAX_TANK_TEMP_C, DEFAULT_SETPOINT_C,
    DEFAULT_TANK_DIAMETER_M, DEFAULT_TANK_HEIGHT_M, DEFAULT_TANK_VOLUME_M3, DEFAULT_UA_W_PER_K,
};

const DEFAULT_DEADBAND_C: f64 = 5.555_555_556; // 10°F (OCHRE default)
const DEFAULT_ELEMENT_POWER_W: f64 = 4_500.0;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ResistanceWhState {
    setpoint_c: f64,
    target_setpoint_c: f64,
    deadband_c: f64,
    upper_element_on: bool,
    lower_element_on: bool,
    duty_cycle: f64,
    mode_override: Option<OperatingMode>,
    element_priority: ElementPriorityMode,
    tank_state: Vec<u8>,
    tank_avg_temp_c: f64,
    upper_element_power_w: f64,
    lower_element_power_w: f64,
    max_combined_power_w: Option<f64>,
    electric_kw: f64,
    draw_flow_rate_kg_s: f64,
    // --- Demand response state ---
    dr_level: DRLevel,
    dr_setpoint_offset_c: f64,
    dr_load_fraction: f64,
    dr_duration_remaining_s: Option<f64>,
}

pub struct ResistanceWH {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,
    core_output: CoreOutput,
    tank: StratifiedTank,
    upper_node: usize,
    lower_node: usize,
    upper_element_power_w: f64,
    lower_element_power_w: f64,
    /// Maximum total element power (W) before clamping in Simultaneous mode.
    max_combined_power_w: Option<f64>,
    setpoint_c: f64,
    deadband_c: f64,
    max_tank_temp_c: f64,
    duty_cycle: f64,
    mode_override: Option<OperatingMode>,
    element_priority: ElementPriorityMode,
    upper_element_on: bool,
    lower_element_on: bool,
    loop_id: LoopId,
    fluid_type: FluidType,
    mains_temp_c: f64,
    draw_flow_rate_kg_s: f64,
    draw_l_per_min_source: Option<ScheduleSource>,
    mains_temp_c_source: Option<ScheduleSource>,
    // --- ZIP voltage model ---
    zip: WaterHeaterZip,
    // --- Setpoint ramp rate ---
    target_setpoint_c: f64,
    setpoint_ramp_rate_c_per_s: Option<f64>,
    // --- TMV tempered draw ---
    /// Fixture delivery temperature for TMV blending (°C). Default 40.6°C.
    fixture_delivery_temp_c: f64,
    /// Hot-draw delivery temperature (°C). Defaults to setpoint.
    hot_draw_temp_c: Option<f64>,
    // --- Demand response state ---
    dr_setpoint_offset_c: f64,
    dr_load_fraction: f64,
    dr_duration_remaining_s: Option<f64>,
    dr_level: DRLevel,
    // Transient load fraction from LoadFraction control signal; reset each step.
    ctrl_load_fraction: f64,
    /// Whether zone_id was explicitly set in config or fell back to ZoneId(1).
    zone_id_explicit: bool,
}

impl ResistanceWH {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let (zone, zone_id_explicit) = zone_id_from_config_or_default(&config, &config.name);
        let loop_id = loop_id_from_config(&config, &["loop_id", "dhw_loop_id"]).unwrap_or_default();
        let n_nodes = parse_usize(config.get_f64("tank_nodes"))
            .unwrap_or(6)
            .clamp(1, 12);
        let upper_node = parse_usize(config.get_f64("upper_element_node"))
            .unwrap_or(0)
            .min(n_nodes - 1);
        let lower_node = parse_usize(config.get_f64("lower_element_node"))
            .unwrap_or(n_nodes - 1)
            .min(n_nodes - 1);

        let tank = StratifiedTank::new(StratifiedTankConfig {
            n_nodes,
            height_m: DEFAULT_TANK_HEIGHT_M,
            diameter_m: DEFAULT_TANK_DIAMETER_M,
            ua_w_per_k: DEFAULT_UA_W_PER_K,
            conductivity_w_m_k: DEFAULT_CONDUCTIVITY_W_M_K,
            initial_temp_c: DEFAULT_SETPOINT_C,
            element_nodes: [Some(upper_node), Some(lower_node)],
            node_volumes_m3: None,
            ua_end_cap_w_per_k: None,
        })
        .expect("default resistance water-heater tank config must be valid");

        Self {
            descriptor: EquipmentDescriptor {
                id: EquipmentId(equipment_id_from_config(&config).unwrap_or(0)),
                name: config.name,
                end_use: EndUse::WATER_HEATING,
                equipment_type: Cow::Borrowed("Resistance Water Heater"),
                zone: Some(zone),
                fuel: FuelType::Electric,
                stage: ExecutionStage::Thermal,
                control_capabilities: ControlCapabilities::THERMAL_SETPOINT
                    | ControlCapabilities::DUTY_CYCLE
                    | ControlCapabilities::MODE_OVERRIDE
                    | ControlCapabilities::LOAD_FRACTION
                    | ControlCapabilities::POWER_LIMIT
                    | ControlCapabilities::DEMAND_RESPONSE,
                core_capabilities: CoreCapabilities::ELECTRIC | CoreCapabilities::HAS_MODE,
                telemetry_fields: telemetry_fields(n_nodes),
                zone_type: None,
            },
            ports: vec![
                PortDeclaration::electrical(),
                PortDeclaration::thermal(zone),
                PortDeclaration::fluid(loop_id, FluidType::Water),
                PortDeclaration::fluid(super::DHW_DEMAND_LOOP, FluidType::Water),
            ],
            telemetry: {
                let mut t = default_telemetry();
                tank.register_node_telemetry(&mut t);
                t
            },
            core_output: CoreOutput::default(),
            tank,
            upper_node,
            lower_node,
            upper_element_power_w: DEFAULT_ELEMENT_POWER_W,
            lower_element_power_w: DEFAULT_ELEMENT_POWER_W,
            max_combined_power_w: None,
            setpoint_c: DEFAULT_SETPOINT_C,
            deadband_c: DEFAULT_DEADBAND_C,
            max_tank_temp_c: DEFAULT_MAX_TANK_TEMP_C,
            duty_cycle: 1.0,
            mode_override: None,
            element_priority: ElementPriorityMode::default(),
            upper_element_on: false,
            lower_element_on: false,
            loop_id,
            fluid_type: FluidType::Water,
            mains_temp_c: 10.0,
            draw_flow_rate_kg_s: 0.0,
            draw_l_per_min_source: None,
            mains_temp_c_source: None,
            zip: WaterHeaterZip::default(),
            target_setpoint_c: DEFAULT_SETPOINT_C,
            setpoint_ramp_rate_c_per_s: None,
            fixture_delivery_temp_c: 40.6,
            hot_draw_temp_c: None,
            dr_setpoint_offset_c: 0.0,
            dr_load_fraction: 1.0,
            dr_duration_remaining_s: None,
            dr_level: DRLevel::Normal,
            ctrl_load_fraction: 1.0,
            zone_id_explicit,
        }
    }

    fn effective_setpoint_c(&self) -> f64 {
        self.setpoint_c + self.dr_setpoint_offset_c
    }

    fn thermostat_calls(&self) -> (bool, bool) {
        let upper_temp = self.tank.node_temps()[self.upper_node];
        // OCHRE averages the lower node with the node directly above it (when n_nodes >= 3)
        // to reduce sensitivity to single-node temperature spikes.
        let lower_temp = self.lower_sensor_temp();
        let effective_sp = self.effective_setpoint_c();
        let upper_call = hysteresis_call(
            upper_temp,
            effective_sp,
            self.deadband_c,
            self.upper_element_on,
        );
        let lower_call = hysteresis_call(
            lower_temp,
            effective_sp,
            self.deadband_c,
            self.lower_element_on,
        );
        (upper_call, lower_call)
    }

    /// Returns the temperature used by the lower thermostat.
    ///
    /// When the tank has at least 3 nodes, averages the lower node with the node
    /// directly above it (index `lower_node - 1`), matching OCHRE behavior.
    fn lower_sensor_temp(&self) -> f64 {
        let temps = self.tank.node_temps();
        let n = temps.len();
        if n >= 3 && self.lower_node > 0 {
            (temps[self.lower_node] + temps[self.lower_node - 1]) * 0.5
        } else {
            temps[self.lower_node]
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
}

impl ResistanceWH {
    fn init_typed(
        &mut self,
        config: &EquipmentConfig,
        env: &EnvironmentState,
    ) -> crate::Result<()> {
        let c = config.require_typed::<ElectricResistanceWaterHeaterConfig>(
            "Electric Resistance Water Heater",
        )?;
        c.validate()?;

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        if c.zone_id.is_none() && c.zone_type.is_some() {
            warn!(
                water_heater = %config.name,
                zone_type = ?c.zone_type,
                "zone_id not resolved from HPXML Location; falling back to ZoneId(1)"
            );
        }

        self.descriptor.id = EquipmentId(c.equipment_id.unwrap_or(self.descriptor.id.0));
        let zone = c.zone_id.map(ZoneId).or(self.descriptor.zone);
        self.descriptor.zone = zone;
        self.descriptor.zone_type = c.zone_type.clone();
        self.ports[1].zone = zone;
        self.loop_id = c.loop_id.map(LoopId).unwrap_or(self.loop_id);
        self.ports[2].loop_id = Some(self.loop_id);

        let tank_volume_m3 = c.tank_volume_m3.unwrap_or(DEFAULT_TANK_VOLUME_M3);
        let height_m = c.tank_height_m.unwrap_or(DEFAULT_TANK_HEIGHT_M).max(0.2);
        let diameter_m = (4.0 * tank_volume_m3 / (std::f64::consts::PI * height_m))
            .max(1e-6)
            .sqrt();
        let n_nodes = usize::from(c.tank_nodes.unwrap_or(6).max(1));
        self.upper_node = if n_nodes >= 12 { 2 } else { 0 };
        self.lower_node = if n_nodes >= 12 {
            9
        } else {
            n_nodes.saturating_sub(1)
        };

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
            element_nodes: [Some(self.upper_node), Some(self.lower_node)],
            node_volumes_m3: None,
            ua_end_cap_w_per_k: None,
        })?;

        let capacity_w = c
            .element_power_w
            .or(c.heating_capacity_w)
            .unwrap_or(DEFAULT_ELEMENT_POWER_W);
        self.upper_element_power_w = capacity_w.max(0.0);
        self.lower_element_power_w = capacity_w.max(0.0);

        self.setpoint_c = c.setpoint_c.unwrap_or(DEFAULT_SETPOINT_C);
        self.deadband_c = c.deadband_c.unwrap_or(DEFAULT_DEADBAND_C);
        self.max_tank_temp_c = c.max_tank_temp_c.unwrap_or(DEFAULT_MAX_TANK_TEMP_C);
        self.duty_cycle = 1.0;
        self.mode_override = None;
        self.element_priority = match c.element_priority_mode.as_deref() {
            Some("Simultaneous") | Some("simultaneous") => ElementPriorityMode::Simultaneous,
            _ => ElementPriorityMode::MasterSlave,
        };
        self.max_combined_power_w = c.max_combined_power_w;
        if self.element_priority == ElementPriorityMode::Simultaneous
            && self.max_combined_power_w.is_none()
            && self.upper_element_power_w >= DEFAULT_ELEMENT_POWER_W
            && self.lower_element_power_w >= DEFAULT_ELEMENT_POWER_W
        {
            // NEC Table 210.24(1): 30 A branch circuit at 240 V nominal → 7,200 W
            // continuous (80% derate for resistive loads per NEC 422.10(A)).
            // Two 4,500 W elements = 9,000 W draws 37.5 A → exceeds the 30 A rating.
            // In real installations, simultaneous-operation water heaters use
            // interlocked wiring, lower-rated elements, or dedicated dual circuits.
            warn!(
                equipment_id = self.descriptor.id.0,
                equipment_name = %self.descriptor.name,
                upper_element_w = self.upper_element_power_w,
                lower_element_w = self.lower_element_power_w,
                combined_w = self.upper_element_power_w + self.lower_element_power_w,
                branch_limit_w = 7_200.0,
                "Simultaneous operation with 2×4,500 W elements (9,000 W) \
                 exceeds typical 30 A / 240 V branch circuit rating (7,200 W). \
                 Set max_combined_power_w to enforce a power ceiling, or switch \
                 to MasterSlave priority mode (the default)."
            );
        }
        self.upper_element_on = false;
        self.lower_element_on = false;

        self.mains_temp_c = super::require_mains_temp_c(env, "Electric Resistance Water Heater")?;
        self.draw_flow_rate_kg_s = c.draw_flow_rate_kg_s.unwrap_or(0.0);
        self.draw_l_per_min_source = c.draw_flow_rate_source.clone().map(|s| s.into_runtime());
        self.mains_temp_c_source = c.mains_temp_c_source.clone().map(|s| s.into_runtime());
        self.zip = WaterHeaterZip::default();

        self.setpoint_ramp_rate_c_per_s =
            c.max_setpoint_ramp_rate_c_per_min.map(|rate| rate / 60.0);
        self.target_setpoint_c = self.setpoint_c;

        self.fixture_delivery_temp_c = c.fixture_delivery_temp_c.unwrap_or(40.6);
        self.hot_draw_temp_c = c.hot_draw_temp_c;

        self.dr_setpoint_offset_c = 0.0;
        self.dr_load_fraction = 1.0;
        self.dr_duration_remaining_s = None;
        self.dr_level = DRLevel::Normal;
        self.ctrl_load_fraction = 1.0;
        self.descriptor.telemetry_fields = telemetry_fields(n_nodes);
        self.telemetry = default_telemetry();
        self.tank.register_node_telemetry(&mut self.telemetry);
        self.core_output = CoreOutput::default();
        Ok(())
    }

    /// Override the runtime max_tank_temp_c safety limit.
    /// Intended for tests that need to trigger the safety cutout at a specific
    /// temperature below the configured setpoint.
    pub fn set_max_tank_temp_c(&mut self, temp_c: f64) {
        self.max_tank_temp_c = temp_c;
    }
}

impl Equipment for ResistanceWH {
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
        let dt_s = env.time_res.num_milliseconds().max(0) as f64 / 1000.0;

        // Ramp setpoint toward target.
        if let Some(rate) = self.setpoint_ramp_rate_c_per_s {
            self.setpoint_c =
                super::ramp_limited_setpoint(self.setpoint_c, self.target_setpoint_c, rate, dt_s);
        }

        // Advance DR duration; auto-revert to Normal when expired.
        if let Some(remaining) = self.dr_duration_remaining_s {
            let next = remaining - dt_s;
            if next <= 0.0 {
                self.dr_duration_remaining_s = None;
                self.apply_dr_level(DRLevel::Normal);
            } else {
                self.dr_duration_remaining_s = Some(next);
            }
        }

        // Safety cutout: force off if ANY node exceeds max tank temperature.
        // A high-limit aquastat/thermal fuse responds to the hottest point in the tank.
        let max_node_temp = self
            .tank
            .node_temps()
            .iter()
            .copied()
            .reduce(f64::max)
            .expect("node_temps is never empty");
        if !max_node_temp.is_finite() || max_node_temp > self.max_tank_temp_c {
            self.upper_element_on = false;
            self.lower_element_on = false;
            return OperatingMode::Off;
        }

        // DR GridEmergency full shed or explicit Off override.
        if self.dr_load_fraction <= 0.0
            || matches!(self.mode_override, Some(OperatingMode::Off))
            || self.duty_cycle <= 0.0
        {
            self.upper_element_on = false;
            self.lower_element_on = false;
            return OperatingMode::Off;
        }

        let (upper_call, lower_call) = self.thermostat_calls();
        match self.element_priority {
            ElementPriorityMode::MasterSlave => {
                self.upper_element_on = upper_call;
                // Slave lockout: lower cannot fire while upper is firing.
                self.lower_element_on = !self.upper_element_on && lower_call;
            }
            ElementPriorityMode::Simultaneous => {
                self.upper_element_on = upper_call;
                self.lower_element_on = lower_call;
            }
        }

        if self.upper_element_on || self.lower_element_on {
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
        let ctrl_duty =
            (self.duty_cycle * self.dr_load_fraction * self.ctrl_load_fraction).clamp(0.0, 1.0);

        // Ideal capacity mode (coarse timesteps >= 5 min): compute the fraction
        // of rated power needed to maintain tank temperature at setpoint, matching
        // OCHRE's WaterHeater.solve_ideal_capacity(). This produces time-averaged
        // power instead of full on/off cycling spikes.
        let use_ideal = env.time_res.num_seconds() >= 300;
        let (upper_power_w, mut lower_power_w) = if use_ideal && mode == OperatingMode::Heating {
            let dt_s = dt.as_secs_f64();
            let ambient_c = self.ambient_temp_c(env);
            let up = if self.upper_element_on && self.upper_element_power_w > 0.0 {
                let ideal_w = self.tank.ideal_capacity_for_node(
                    self.upper_node,
                    self.setpoint_c,
                    ambient_c,
                    dt_s,
                );
                self.upper_element_power_w
                    * (ideal_w / self.upper_element_power_w).clamp(0.0, 1.0)
                    * ctrl_duty
            } else {
                0.0
            };
            let lo = if self.lower_element_on && self.lower_element_power_w > 0.0 {
                let ideal_w = self.tank.ideal_capacity_for_node(
                    self.lower_node,
                    self.setpoint_c,
                    ambient_c,
                    dt_s,
                );
                self.lower_element_power_w
                    * (ideal_w / self.lower_element_power_w).clamp(0.0, 1.0)
                    * ctrl_duty
            } else {
                0.0
            };
            (up, lo)
        } else {
            let up = if self.upper_element_on {
                self.upper_element_power_w * ctrl_duty
            } else {
                0.0
            };
            let lo = if self.lower_element_on {
                self.lower_element_power_w * ctrl_duty
            } else {
                0.0
            };
            (up, lo)
        };

        // Clamp combined power in Simultaneous mode when max_combined_power_w is set.
        // Upper element has priority; lower element duty is limited by remaining
        // capacity. This matches OCHRE's duty-clamping approach: upper element gets
        // its full required power, and the lower element is bounded by whatever
        // capacity remains under the ceiling. NEC Table 210.24(1): 30 A / 240 V
        // branch circuit → 7,200 W continuous with 80% derate.
        if self.element_priority == ElementPriorityMode::Simultaneous {
            if let Some(max_w) = self.max_combined_power_w {
                let combined = upper_power_w + lower_power_w;
                if combined > max_w {
                    lower_power_w = (max_w - upper_power_w).max(0.0);
                }
            }
        }

        let mut heat_buf = [(0usize, 0.0f64); 2];
        let mut n = 0;
        if upper_power_w > 0.0 {
            heat_buf[n] = (self.upper_node, upper_power_w);
            n += 1;
        }
        if lower_power_w > 0.0 {
            heat_buf[n] = (self.lower_node, lower_power_w);
            n += 1;
        }
        let heat_injections = &heat_buf[..n];

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
        // Use step_tempered: TMV mixes hot tank water with cold mains to deliver
        // at fixture_delivery_temp_c, reducing the actual hot-water withdrawal.
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

        let rated_electric_power_w = upper_power_w + lower_power_w;
        let (electric_power_w, reactive_power_kvar) =
            self.zip.apply(rated_electric_power_w, env.grid.voltage_pu);
        if electric_power_w > 0.0 {
            ports.accumulate(&PortContribution::Electrical {
                active_power_w: electric_power_w,
                reactive_power_kvar,
            })?;
        }

        if total_draw_kg_s > 0.0 {
            ports.accumulate(&PortContribution::Fluid {
                loop_id: self.loop_id,
                flow_rate_kg_s: total_draw_kg_s,
                supply_temp_c: draw.outlet_temp_c,
                return_temp_c: mains_temp_c,
                fluid_type: self.fluid_type,
                thermal_power_w: None,
            })?;
        }

        // Jacket loss: tank skin heat flows into the conditioned zone.
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
        self.telemetry.set(tk::UPPER_ELEMENT_POWER_W, upper_power_w);
        self.telemetry.set(tk::LOWER_ELEMENT_POWER_W, lower_power_w);
        self.telemetry
            .set(tk::ELECTRIC_KW, power_w_to_kw(electric_power_w));
        self.telemetry
            .set(tk::ELEMENT_KW, power_w_to_kw(electric_power_w));
        self.telemetry.set(tk::ELECTRIC_POWER_W, electric_power_w);
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
                electric_kw: Some(ElectricPower::Consumption(
                    power_w_to_kw(electric_power_w).max(0.0),
                )),
                reactive_power_kvar: None,
                fuel_w: None,
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

        // Reset transient ctrl_load_fraction after this step so it does not
        // carry over to the next step unless reapplied by the controller.
        self.ctrl_load_fraction = 1.0;
        self.core_output = core_output;

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
            &ResistanceWhState {
                setpoint_c: self.setpoint_c,
                target_setpoint_c: self.target_setpoint_c,
                deadband_c: self.deadband_c,
                upper_element_on: self.upper_element_on,
                lower_element_on: self.lower_element_on,
                duty_cycle: self.duty_cycle,
                mode_override: self.mode_override,
                element_priority: self.element_priority,
                tank_state: self.tank.save_state()?,
                tank_avg_temp_c: self.telemetry.get(tk::TANK_AVG_TEMP_C).unwrap_or(0.0),
                upper_element_power_w: self.telemetry.get(tk::UPPER_ELEMENT_POWER_W).unwrap_or(0.0),
                lower_element_power_w: self.telemetry.get(tk::LOWER_ELEMENT_POWER_W).unwrap_or(0.0),
                max_combined_power_w: self.max_combined_power_w,
                electric_kw: self.telemetry.get(tk::ELECTRIC_KW).unwrap_or(0.0),
                draw_flow_rate_kg_s: self.telemetry.get(tk::DRAW_FLOW_RATE_KG_S).unwrap_or(0.0),
                dr_level: self.dr_level,
                dr_setpoint_offset_c: self.dr_setpoint_offset_c,
                dr_load_fraction: self.dr_load_fraction,
                dr_duration_remaining_s: self.dr_duration_remaining_s,
            },
            Self::checkpoint_version(),
            "ResistanceWH",
        )
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let decoded: ResistanceWhState = load_versioned(
            state,
            Self::checkpoint_version(),
            "ResistanceWH",
            self.descriptor().id,
        )?;
        self.setpoint_c = decoded.setpoint_c;
        self.target_setpoint_c = decoded.target_setpoint_c;
        self.deadband_c = decoded.deadband_c;
        self.upper_element_on = decoded.upper_element_on;
        self.lower_element_on = decoded.lower_element_on;
        self.duty_cycle = decoded.duty_cycle;
        self.mode_override = decoded.mode_override;
        self.element_priority = decoded.element_priority;
        self.max_combined_power_w = decoded.max_combined_power_w;
        self.dr_level = decoded.dr_level;
        self.dr_setpoint_offset_c = decoded.dr_setpoint_offset_c;
        self.dr_load_fraction = decoded.dr_load_fraction;
        self.dr_duration_remaining_s = decoded.dr_duration_remaining_s;
        self.tank.load_state(&decoded.tank_state)?;

        self.telemetry
            .insert(tk::TANK_AVG_TEMP_C, decoded.tank_avg_temp_c);
        self.telemetry
            .insert(tk::UPPER_ELEMENT_POWER_W, decoded.upper_element_power_w);
        self.telemetry
            .insert(tk::LOWER_ELEMENT_POWER_W, decoded.lower_element_power_w);
        self.telemetry.insert(tk::ELECTRIC_KW, decoded.electric_kw);
        self.telemetry.insert(tk::ELEMENT_KW, decoded.electric_kw);
        self.telemetry
            .insert(tk::ELECTRIC_POWER_W, power_kw_to_w(decoded.electric_kw));
        self.telemetry
            .insert(tk::DRAW_FLOW_RATE_KG_S, decoded.draw_flow_rate_kg_s);
        self.telemetry.insert(
            tk::OPERATING_MODE,
            if decoded.upper_element_on || decoded.lower_element_on {
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
                deadband_c,
            } => {
                if let Some(sp) = heating_setpoint_c.or(*cooling_setpoint_c) {
                    if !sp.is_finite() {
                        return Err(HaresError::Control(format!(
                            "invalid water-heater setpoint: {sp}"
                        )));
                    }
                    self.target_setpoint_c = sp;
                    if self.setpoint_ramp_rate_c_per_s.is_none() {
                        self.setpoint_c = sp;
                    }
                }
                if let Some(db) = deadband_c {
                    if !db.is_finite() || *db < 0.0 {
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
                        "invalid duty cycle for ResistanceWH: {on_fraction}"
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
                // Clamp element power to respect the limit by reducing ctrl_load_fraction.
                let total_w = self.upper_element_power_w + self.lower_element_power_w;
                if total_w > 0.0 {
                    let max_fraction = (power_kw_to_w(*max_power_kw) / total_w).clamp(0.0, 1.0);
                    self.ctrl_load_fraction = self.ctrl_load_fraction.min(max_fraction);
                }
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

impl ResistanceWH {
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

pub fn register_with_registry(registry: &mut EquipmentRegistry) {
    registry.register(
        "Resistance Water Heater",
        Box::new(|config| Box::new(ResistanceWH::new(config))),
    );
    registry.register(
        "Electric Resistance Water Heater",
        Box::new(|config| Box::new(ResistanceWH::new(config))),
    );
}

fn default_telemetry() -> Telemetry {
    let mut telemetry = Telemetry::with_capacity(10);
    telemetry.insert(tk::TANK_AVG_TEMP_C, 0.0);
    telemetry.insert(tk::UPPER_ELEMENT_POWER_W, 0.0);
    telemetry.insert(tk::LOWER_ELEMENT_POWER_W, 0.0);
    telemetry.insert(tk::ELECTRIC_KW, 0.0);
    telemetry.insert(tk::ELEMENT_KW, 0.0);
    telemetry.insert(tk::ELECTRIC_POWER_W, 0.0);
    telemetry.insert(tk::DRAW_FLOW_RATE_KG_S, 0.0);
    telemetry.insert(tk::UNMET_LOAD_W, 0.0);
    telemetry.insert(tk::OPERATING_MODE, 0.0);
    telemetry.insert(tk::OUTLET_TEMP_C, 0.0);
    telemetry
}

fn telemetry_fields(n_nodes: usize) -> Vec<TelemetryField> {
    let mut fields = vec![
        TelemetryField {
            name: tk::TANK_AVG_TEMP_C.to_string(),
            unit: "C".to_string(),
            description: "Volume-weighted average tank temperature".to_string(),
        },
        TelemetryField {
            name: tk::UPPER_ELEMENT_POWER_W.to_string(),
            unit: "W".to_string(),
            description: "Upper element electric power".to_string(),
        },
        TelemetryField {
            name: tk::LOWER_ELEMENT_POWER_W.to_string(),
            unit: "W".to_string(),
            description: "Lower element electric power".to_string(),
        },
        TelemetryField {
            name: tk::ELECTRIC_KW.to_string(),
            unit: "kW".to_string(),
            description: "Total electric draw".to_string(),
        },
        TelemetryField {
            name: tk::ELEMENT_KW.to_string(),
            unit: "kW".to_string(),
            description: "Heating element electric power".to_string(),
        },
        TelemetryField {
            name: tk::ELECTRIC_POWER_W.to_string(),
            unit: "W".to_string(),
            description: "Total electric draw".to_string(),
        },
        TelemetryField {
            name: tk::DRAW_FLOW_RATE_KG_S.to_string(),
            unit: "kg/s".to_string(),
            description: "Domestic hot water draw flow rate".to_string(),
        },
        TelemetryField {
            name: tk::UNMET_LOAD_W.to_string(),
            unit: "W".to_string(),
            description: "Unmet fixture load when tank below delivery temp".to_string(),
        },
        TelemetryField {
            name: tk::OPERATING_MODE.to_string(),
            unit: "enum".to_string(),
            description: "0=Off, 1=Heating".to_string(),
        },
        TelemetryField {
            name: tk::OUTLET_TEMP_C.to_string(),
            unit: "C".to_string(),
            description: "Hot water outlet temperature delivered to fixture".to_string(),
        },
    ];
    for i in 0..n_nodes {
        fields.push(TelemetryField {
            name: tk::tank_node_key(i),
            unit: "C".to_string(),
            description: format!("Tank node {i} temperature"),
        });
    }
    fields.push(TelemetryField {
        name: tk::SKIN_LOSS_W.to_string(),
        unit: "W".to_string(),
        description: "Tank jacket (skin) heat loss to zone".to_string(),
    });
    fields
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        ControlSignal, EnvironmentState, GridState, PortDeclaration, PortSlots, ThermalAccumulator,
        WeatherState, ZoneId, ZoneState, telemetry_keys as tk,
    };

    use super::ResistanceWH;
    use crate::water_heater::DHW_DEMAND_LOOP;
    use crate::{ElectricResistanceWaterHeaterConfig, Equipment, EquipmentConfig};

    fn env(zone_temp_c: f64) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp_c,
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
                .expect("UTC offset")
                .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                .single()
                .expect("valid"),
            time_res: ChronoDuration::seconds(60),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    fn config_from_typed(cfg: ElectricResistanceWaterHeaterConfig) -> EquipmentConfig {
        EquipmentConfig::from_typed("WH".to_string(), "Resistance Water Heater".to_string(), cfg)
    }

    fn typed_config() -> ElectricResistanceWaterHeaterConfig {
        ElectricResistanceWaterHeaterConfig {
            equipment_id: None,
            zone_id: None,
            loop_id: Some(1),
            tank_volume_m3: None,
            tank_height_m: None,
            energy_factor: None,
            uniform_energy_factor: None,
            heating_capacity_w: None,
            ua_w_per_k: None,
            setpoint_c: Some(52.0),
            deadband_c: Some(2.0),
            max_tank_temp_c: Some(300.0),
            initial_tank_temp_c: Some(40.0),
            tank_nodes: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: Some(0.0),
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            element_power_w: None,
            max_setpoint_ramp_rate_c_per_min: None,
            element_priority_mode: None,
            jacket_r_value_m2_k_w: None,
            max_combined_power_w: None,
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
        }
    }

    fn config() -> EquipmentConfig {
        config_from_typed(typed_config())
    }

    #[test]
    fn ports_and_telemetry_contract_match_water_heater_schema() {
        let cfg = config();
        let mut wh = ResistanceWH::new(cfg.clone());
        wh.init(&cfg, &env(21.0)).unwrap();

        assert_eq!(wh.ports().len(), 4);
        assert!(wh.ports().contains(&PortDeclaration::electrical()));
        assert!(wh.ports().contains(&PortDeclaration::thermal(ZoneId(1))));
        assert!(wh.ports().contains(&PortDeclaration::fluid(
            hares_types::LoopId(1),
            hares_types::FluidType::Water,
        )));
        assert!(wh.ports().contains(&PortDeclaration::fluid(
            DHW_DEMAND_LOOP,
            hares_types::FluidType::Water
        )));

        let mut p = ports();
        wh.step(&env(21.0), Duration::from_secs(60), &mut p)
            .unwrap();
        assert_eq!(
            wh.telemetry().len(),
            wh.descriptor().telemetry_fields.len(),
            "resistance telemetry map must match declared telemetry schema"
        );
    }

    fn ports() -> PortSlots {
        PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            electrical: Default::default(),
            fuel: Default::default(),
            fluid: vec![hares_types::FluidAccumulator::new(
                hares_types::LoopId(1),
                hares_types::FluidType::Water,
            )],
            custom: vec![],
            humidity: vec![],
            ..Default::default()
        }
    }

    #[test]
    fn upper_element_has_priority_when_tank_is_cold() {
        let mut eq = ResistanceWH::new(config());
        eq.init(&config(), &env(21.0)).unwrap();

        let mut p = ports();
        eq.step(&env(21.0), Duration::from_secs(60), &mut p)
            .unwrap();

        assert!(eq.telemetry().get(tk::UPPER_ELEMENT_POWER_W).unwrap_or(0.0) > 0.0);
        assert_eq!(
            eq.telemetry()
                .get(tk::LOWER_ELEMENT_POWER_W)
                .unwrap_or(-1.0),
            0.0
        );
    }

    #[test]
    fn lower_element_runs_after_upper_is_satisfied() {
        let mut eq = ResistanceWH::new(config());
        let cfg = config();
        eq.init(&cfg, &env(21.0)).unwrap();

        eq.tank
            .heat_node(eq.upper_node, 200_000.0, Duration::from_secs(120))
            .unwrap();

        let mode = eq.update_control(&env(21.0));
        assert_eq!(mode, hares_types::OperatingMode::Heating);
        assert!(!eq.upper_element_on);
        assert!(eq.lower_element_on);
    }

    #[test]
    fn state_round_trip_restores_tank_and_control_state() {
        let mut eq = ResistanceWH::new(config());
        eq.init(&config(), &env(21.0)).unwrap();
        eq.apply_control(&ControlSignal::DutyCycle {
            on_fraction: 0.4,
            period_s: None,
            component: None,
        })
        .unwrap();

        let mut p = ports();
        eq.step(&env(21.0), Duration::from_secs(60), &mut p)
            .unwrap();
        let state = eq.save_state().unwrap();

        let mut restored = ResistanceWH::new(config());
        restored.init(&config(), &env(21.0)).unwrap();
        restored.load_state(&state).unwrap();

        assert_eq!(restored.duty_cycle, eq.duty_cycle);
        assert_eq!(restored.upper_element_on, eq.upper_element_on);
        assert_eq!(restored.lower_element_on, eq.lower_element_on);
        assert_eq!(restored.tank.node_temps(), eq.tank.node_temps());
    }

    #[test]
    fn default_deadband_matches_ochre_when_not_configured() {
        let mut typed = typed_config();
        typed.deadband_c = None;
        let cfg = config_from_typed(typed);

        let mut eq = ResistanceWH::new(cfg.clone());
        eq.init(&cfg, &env(21.0)).unwrap();

        assert!(
            (eq.deadband_c - 5.555_555_556).abs() < 1e-9,
            "expected default deadband to be 5.555555556°C, got {}",
            eq.deadband_c
        );
    }

    #[test]
    fn explicit_deadband_override_is_applied() {
        let mut typed = typed_config();
        typed.deadband_c = Some(3.0);
        let cfg = config_from_typed(typed);

        let mut eq = ResistanceWH::new(cfg.clone());
        eq.init(&cfg, &env(21.0)).unwrap();

        assert_eq!(eq.deadband_c, 3.0);
    }

    // --- Regression tests for bug fixes ---

    /// For a 12-node tank with n_nodes >= 3, the lower thermostat must read the
    /// average of lower_node and lower_node-1, not just lower_node alone.
    #[test]
    fn lower_thermostat_averages_two_bottom_nodes_for_large_tanks() {
        let mut typed = typed_config();
        typed.initial_tank_temp_c = Some(40.0);
        typed.tank_nodes = Some(12);
        let cfg = config_from_typed(typed);

        let mut eq = ResistanceWH::new(cfg.clone());
        eq.init(&cfg, &env(21.0)).unwrap();

        let n = eq.tank.n_nodes();
        assert!(n >= 3, "Tank must have at least 3 nodes for this test");

        // Force the bottom node to be cold (below deadband) but the node above it hot.
        // The average should be above setpoint - deadband, so lower element stays off.
        let lower = eq.lower_node;
        let above = lower - 1;
        // Heat the node directly above lower_node well above setpoint.
        eq.tank
            .heat_node(above, 500_000.0, Duration::from_secs(120))
            .unwrap();
        // Now lower_node is still at ~40°C (below setpoint-deadband=50), but above is hot.
        // Sensor average = (40 + hot) / 2. If hot is >60, average > 50, lower element stays off.

        let lower_sensor = eq.lower_sensor_temp();
        let node_temps = eq.tank.node_temps();
        let expected_avg = (node_temps[lower] + node_temps[above]) * 0.5;
        assert!(
            (lower_sensor - expected_avg).abs() < 1e-9,
            "lower_sensor_temp() should average lower_node ({:.2}) and above ({:.2}), \
             got {lower_sensor:.2}, expected {expected_avg:.2}",
            node_temps[lower],
            node_temps[above]
        );

        // For a 2-node or 1-node tank, it should use only the lower_node directly.
        let mut typed_small = typed_config();
        typed_small.initial_tank_temp_c = Some(40.0);
        typed_small.tank_nodes = Some(2);
        let cfg2 = config_from_typed(typed_small);
        let mut eq2 = ResistanceWH::new(cfg2.clone());
        eq2.init(&cfg2, &env(21.0)).unwrap();
        let single_sensor = eq2.lower_sensor_temp();
        assert_eq!(
            single_sensor,
            eq2.tank.node_temps()[eq2.lower_node],
            "2-node tank should use lower_node directly without averaging"
        );
    }

    // --- DR and control signal tests ---
    //
    // DR Moderate: setpoint offset = -3°C. Tank at 49°C with setpoint 52°C -- normally
    // calling for heat (49 < 52 - 2 = 50). After Moderate, effective setpoint = 49°C; tank at
    // 49°C is not below 49 - 2 = 47, so no call from Off state.
    #[test]
    fn wh_dr_moderate_reduces_setpoint() {
        let mut typed = typed_config();
        typed.initial_tank_temp_c = Some(49.0);
        let cfg = config_from_typed(typed);
        let e = env(21.0);

        // Baseline: tank at 49°C < setpoint - deadband = 50°C → heating from Off.
        let mut eq_base = ResistanceWH::new(cfg.clone());
        eq_base.init(&cfg, &e).unwrap();
        let mode_base = eq_base.update_control(&e);
        assert!(
            matches!(mode_base, hares_types::OperatingMode::Heating),
            "baseline must be heating; got {mode_base:?}"
        );

        // DR Moderate: effective setpoint = 52 + (-3) = 49°C.
        // From Off: fires if tank < 49 - 2 = 47. Tank at 49°C → no call.
        let mut eq_dr = ResistanceWH::new(cfg.clone());
        eq_dr.init(&cfg, &e).unwrap();
        eq_dr
            .apply_control(&ControlSignal::DemandResponse {
                level: hares_types::DRLevel::Moderate,
                duration_s: None,
            })
            .unwrap();
        let mode_dr = eq_dr.update_control(&e);
        assert_eq!(
            mode_dr,
            hares_types::OperatingMode::Off,
            "DR Moderate must suppress heating by reducing effective setpoint; got {mode_dr:?}"
        );
    }

    // DR Critical: setpoint offset = -10°C, dr_load_fraction = 0.5.
    // Start with tank well below setpoint so heating is still active, but power is halved.
    #[test]
    fn wh_dr_critical_reduces_setpoint_and_load() {
        // Tank at 40°C, setpoint=52, deadband=2; Critical offset=-10 → effective_sp=42.
        // 40 < 42-2=40 is the hysteresis boundary -- tank at 40°C is at the deadband edge.
        // Use 38°C so it is clearly below 42-2=40 to ensure heating still fires.
        let mut typed = typed_config();
        typed.initial_tank_temp_c = Some(38.0);
        let cfg = config_from_typed(typed);

        let e = env(21.0);

        // Baseline: full element power without DR.
        let mut eq_base = ResistanceWH::new(cfg.clone());
        eq_base.init(&cfg, &e).unwrap();
        let mut p_base = ports();
        eq_base
            .step(&e, Duration::from_secs(60), &mut p_base)
            .unwrap();
        let w_base = eq_base.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);

        // DR Critical: dr_load_fraction=0.5 → power halved.
        let mut eq_dr = ResistanceWH::new(cfg.clone());
        eq_dr.init(&cfg, &e).unwrap();
        eq_dr
            .apply_control(&ControlSignal::DemandResponse {
                level: hares_types::DRLevel::Critical,
                duration_s: None,
            })
            .unwrap();
        let mut p_dr = ports();
        eq_dr.step(&e, Duration::from_secs(60), &mut p_dr).unwrap();
        let w_dr = eq_dr.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);

        assert!(w_base > 0.0, "baseline must draw power");
        assert!(
            w_dr > 0.0,
            "DR Critical must not completely shed (tank is heating)"
        );
        let ratio = w_dr / w_base;
        assert!(
            (ratio - 0.5).abs() < 0.05,
            "DR Critical load_fraction=0.5 must halve element power; ratio={ratio:.3}"
        );
    }

    /// When the upper node temperature exceeds max_tank_temp_c, both elements
    /// must be forced off regardless of thermostat call.
    #[test]
    fn max_tank_temp_safety_forces_off_for_resistance_wh() {
        let mut typed = typed_config();
        typed.max_tank_temp_c = Some(60.0);
        let cfg = config_from_typed(typed);

        let mut eq = ResistanceWH::new(cfg.clone());
        eq.init(&cfg, &env(21.0)).unwrap();

        // Manually set the tank temperature above the max to trigger the safety cutout.
        eq.tank.node_temps_mut().fill(65.0);

        let mode = eq.update_control(&env(21.0));
        assert_eq!(
            mode,
            hares_types::OperatingMode::Off,
            "Expected Off when tank exceeds max_tank_temp_c"
        );
        assert!(!eq.upper_element_on);
        assert!(!eq.lower_element_on);
    }

    /// Stratified tank: lower node exceeds max_tank_temp_c but upper node is cool.
    /// Safety cutout must still trigger (checks max of all nodes).
    #[test]
    fn max_tank_temp_safety_triggers_on_stratified_hot_bottom() {
        let mut typed = typed_config();
        typed.initial_tank_temp_c = Some(30.0);
        typed.max_tank_temp_c = Some(55.0);
        let cfg = config_from_typed(typed);

        let mut eq = ResistanceWH::new(cfg.clone());
        eq.init(&cfg, &env(21.0)).unwrap();

        let n = eq.tank.node_temps().len();
        eq.tank.node_temps_mut()[0] = 30.0;
        eq.tank.node_temps_mut()[n - 1] = 60.0;

        let mode = eq.update_control(&env(21.0));
        assert_eq!(
            mode,
            hares_types::OperatingMode::Off,
            "Safety cutout must trigger when ANY node exceeds max_tank_temp_c"
        );
        assert!(!eq.upper_element_on);
        assert!(!eq.lower_element_on);
    }

    /// Stratified tank where all nodes are below max_tank_temp_c.
    /// Safety cutout must NOT fire; element must be allowed to operate.
    #[test]
    fn safety_cutout_does_not_fire_when_all_nodes_below_limit() {
        let mut typed = typed_config();
        typed.initial_tank_temp_c = Some(30.0);
        typed.max_tank_temp_c = Some(55.0);
        let cfg = config_from_typed(typed);

        let mut eq = ResistanceWH::new(cfg.clone());
        eq.init(&cfg, &env(21.0)).unwrap();

        // Hottest node below limit; upper node cold so thermostat calls for heat.
        let n = eq.tank.node_temps().len();
        eq.tank.node_temps_mut()[0] = 30.0;
        eq.tank.node_temps_mut()[n - 1] = 54.0;

        let mode = eq.update_control(&env(21.0));
        assert_ne!(
            mode,
            hares_types::OperatingMode::Off,
            "Elements must be allowed to fire when all nodes are below max_tank_temp_c"
        );
    }

    /// Tank at 50°C in a 20°C zone with explicit UA=5 W/K should produce a
    /// positive jacket loss in the thermal port after one step.
    ///
    /// Total UA including end-caps = 5.0 + 2×(5.0×0.1) = 6.0 W/K.
    /// Expected loss ≈ 6.0 × (50 - 20) = 180 W (pre-step estimate; exact value
    /// depends on the Euler step cooling the tank slightly).
    #[test]
    fn jacket_loss_appears_in_thermal_port_when_tank_above_zone_temp() {
        use hares_types::ThermalCategory;

        let mut typed = typed_config();
        typed.setpoint_c = Some(55.0);
        // Start the tank at 50°C -- below setpoint so elements don't interfere with the loss signal.
        typed.initial_tank_temp_c = Some(50.0);
        typed.ua_w_per_k = Some(5.0);
        typed.draw_flow_rate_kg_s = Some(0.0);
        let cfg = config_from_typed(typed);

        let zone_temp_c = 20.0;
        let e = env(zone_temp_c);
        let mut eq = ResistanceWH::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();

        let mut p = ports();
        eq.step(&e, Duration::from_secs(60), &mut p).unwrap();

        // Thermal port must carry a positive JacketLoss sensible gain.
        let jacket = p.thermal[0].sensible_for_category(ThermalCategory::JacketLoss);
        assert!(
            jacket > 0.0,
            "expected positive jacket loss in thermal port, got {jacket}"
        );

        // Telemetry must agree with the port.
        let telem = eq.telemetry().get(tk::SKIN_LOSS_W).unwrap_or(0.0);
        assert!(
            (jacket - telem).abs() < 1e-9,
            "thermal port jacket ({jacket:.3} W) must match telemetry skin_loss_w ({telem:.3} W)"
        );

        // Rough sanity: loss must be in the range [100, 300] W for UA=5 W/K, ΔT=30 K.
        // Total UA with end-caps ≈ 6 W/K → ~180 W.  Allow ±50% for step dynamics.
        assert!(
            jacket > 100.0 && jacket < 300.0,
            "jacket loss {jacket:.1} W is outside plausible range [100, 300] W \
             (UA≈6 W/K × ΔT=30 K ≈ 180 W)"
        );
    }

    /// M4: LoadFraction must not corrupt dr_load_fraction.
    ///
    /// After applying DR Critical (dr_load_fraction = 0.5), sending a
    /// LoadFraction(0.5) must leave dr_load_fraction at 0.5 (not 0.25).
    /// The effective load reduction is expressed through ctrl_load_fraction,
    /// which is separate from the persistent DR state.
    #[test]
    fn load_fraction_does_not_corrupt_dr_state() {
        use hares_types::{ControlSignal, DRLevel};

        let mut typed = typed_config();
        typed.initial_tank_temp_c = Some(38.0);
        let cfg = config_from_typed(typed);
        let e = env(21.0);

        let mut eq = ResistanceWH::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();

        // Step 1: apply DR Critical → dr_load_fraction = 0.5.
        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::Critical,
            duration_s: None,
        })
        .unwrap();
        assert_eq!(
            eq.dr_load_fraction, 0.5,
            "DR Critical must set dr_load_fraction to 0.5"
        );

        // Step 2: apply LoadFraction(0.5) → must set ctrl_load_fraction, NOT touch dr_load_fraction.
        eq.apply_control(&ControlSignal::LoadFraction { fraction: 0.5 })
            .unwrap();
        assert_eq!(
            eq.dr_load_fraction, 0.5,
            "LoadFraction must not corrupt dr_load_fraction (was 0.5, must remain 0.5, not 0.25)"
        );
        assert!(
            (eq.ctrl_load_fraction - 0.5).abs() < 1e-9,
            "ctrl_load_fraction must be 0.5 after LoadFraction(0.5); got {}",
            eq.ctrl_load_fraction
        );

        // Step 3: step -- effective load = duty_cycle(1.0) * dr_load_fraction(0.5) * ctrl_load_fraction(0.5) = 0.25.
        let mut p = ports();
        eq.step(&e, Duration::from_secs(60), &mut p).unwrap();

        // Baseline: full power.
        let mut eq_base = ResistanceWH::new(cfg.clone());
        eq_base.init(&cfg, &e).unwrap();
        let mut p_base = ports();
        eq_base
            .step(&e, Duration::from_secs(60), &mut p_base)
            .unwrap();

        let w_compound = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        let w_base = eq_base.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        assert!(w_base > 0.0, "baseline must draw power");
        let ratio = w_compound / w_base;
        assert!(
            (ratio - 0.25).abs() < 0.05,
            "effective load with dr=0.5 and ctrl=0.5 must be 25% of baseline; ratio={ratio:.3}"
        );
    }

    /// A jacket R-value applied at init must reduce the effective tank UA so
    /// that the standby skin loss after one step is strictly lower than without
    /// the jacket.
    ///
    /// No draw is configured and the initial tank temp is above setpoint so
    /// elements never fire, isolating the standby-loss signal.
    #[test]
    fn jacket_r_value_reduces_standby_skin_loss() {
        let make_cfg = |jacket: Option<f64>| {
            config_from_typed(ElectricResistanceWaterHeaterConfig {
                equipment_id: None,
                zone_id: None,
                loop_id: None,
                tank_volume_m3: None,
                tank_height_m: None,
                energy_factor: None,
                uniform_energy_factor: None,
                heating_capacity_w: None,
                ua_w_per_k: Some(3.0),
                setpoint_c: Some(52.0),
                deadband_c: Some(2.0),
                max_tank_temp_c: Some(300.0),
                initial_tank_temp_c: Some(55.0),
                tank_nodes: None,
                avg_water_draw_l_per_day: None,
                draw_flow_rate_kg_s: Some(0.0),
                draw_flow_rate_source: None,
                mains_temp_c_source: None,
                performance_adjustment: None,
                zone_type: None,
                first_hour_rating_m3: None,
                element_power_w: None,
                max_setpoint_ramp_rate_c_per_min: None,
                element_priority_mode: None,
                jacket_r_value_m2_k_w: jacket,
                max_combined_power_w: None,
                fixture_delivery_temp_c: None,
                hot_draw_temp_c: None,
            })
        };

        let e = env(20.0);

        let cfg_no_jacket = make_cfg(None);
        let mut wh_no_jacket = ResistanceWH::new(cfg_no_jacket.clone());
        wh_no_jacket.init(&cfg_no_jacket, &e).unwrap();
        let mut p1 = ports();
        wh_no_jacket
            .step(&e, Duration::from_secs(60), &mut p1)
            .unwrap();
        let loss_no_jacket = wh_no_jacket.telemetry().get(tk::SKIN_LOSS_W).unwrap_or(0.0);

        // jacket_r_value_m2_k_w = 1.761 m²·K/W ≈ 10 hr·ft²·°F/BTU -- a
        // substantial insulation blanket that should cut losses measurably.
        let cfg_with_jacket = make_cfg(Some(1.761_101_84));
        let mut wh_with_jacket = ResistanceWH::new(cfg_with_jacket.clone());
        wh_with_jacket.init(&cfg_with_jacket, &e).unwrap();
        let mut p2 = ports();
        wh_with_jacket
            .step(&e, Duration::from_secs(60), &mut p2)
            .unwrap();
        let loss_with_jacket = wh_with_jacket
            .telemetry()
            .get(tk::SKIN_LOSS_W)
            .unwrap_or(0.0);

        assert!(
            loss_no_jacket > 0.0,
            "expected positive skin loss without jacket, got {loss_no_jacket}"
        );
        assert!(
            loss_with_jacket < loss_no_jacket,
            "jacket insulation must reduce standby skin loss: \
             with_jacket={loss_with_jacket:.4} W must be < no_jacket={loss_no_jacket:.4} W"
        );
    }

    #[test]
    fn element_kw_present_and_equals_electric_kw_when_heating() {
        let mut eq = ResistanceWH::new(config());
        eq.init(&config(), &env(21.0)).unwrap();

        let mut p = ports();
        eq.step(&env(21.0), Duration::from_secs(60), &mut p)
            .unwrap();

        let electric_kw = eq
            .telemetry()
            .get(tk::ELECTRIC_KW)
            .expect("ELECTRIC_KW must be present");
        let element_kw = eq
            .telemetry()
            .get(tk::ELEMENT_KW)
            .expect("ELEMENT_KW must be present");

        assert!(
            electric_kw > 0.0,
            "WH must be heating for this test to be meaningful"
        );
        assert!(
            (element_kw - electric_kw).abs() < 1e-9,
            "element_kw {element_kw:.6} must equal electric_kw {electric_kw:.6}"
        );
    }

    #[test]
    fn twelve_node_tank_element_positions_match_ochre() {
        let mut typed = typed_config();
        typed.tank_nodes = Some(12);
        let cfg = config_from_typed(typed);
        let mut eq = ResistanceWH::new(cfg.clone());
        eq.init(&cfg, &env(21.0)).unwrap();
        assert_eq!(
            eq.upper_node, 2,
            "12-node tank: upper element must be at node 2"
        );
        assert_eq!(
            eq.lower_node, 9,
            "12-node tank: lower element must be at node 9"
        );
    }

    #[test]
    fn default_node_tank_element_positions() {
        let cfg = config_from_typed(typed_config());
        let mut eq = ResistanceWH::new(cfg.clone());
        eq.init(&cfg, &env(21.0)).unwrap();
        assert_eq!(
            eq.upper_node, 0,
            "default 6-node tank: upper element must be at node 0"
        );
        assert_eq!(
            eq.lower_node,
            eq.tank.node_temps().len() - 1,
            "default 6-node tank: lower element must be at last node"
        );
    }

    #[test]
    fn outlet_temp_c_telemetry_present_with_draw() {
        let mut typed = typed_config();
        typed.initial_tank_temp_c = Some(55.0);
        typed.setpoint_c = Some(55.0);
        typed.draw_flow_rate_kg_s = Some(0.1);
        let cfg = config_from_typed(typed);

        let mut eq = ResistanceWH::new(cfg.clone());
        eq.init(&cfg, &env(21.0)).unwrap();

        let mut p = ports();
        eq.step(&env(21.0), Duration::from_secs(60), &mut p)
            .unwrap();

        let outlet = eq
            .telemetry()
            .get(tk::OUTLET_TEMP_C)
            .expect("OUTLET_TEMP_C must be present");
        assert!(
            outlet > 0.0,
            "OUTLET_TEMP_C should be > 0 when there is a draw, got {outlet}"
        );
    }

    #[test]
    fn outlet_temp_c_telemetry_present_without_draw() {
        let mut typed = typed_config();
        typed.initial_tank_temp_c = Some(55.0);
        typed.draw_flow_rate_kg_s = Some(0.0);
        let cfg = config_from_typed(typed);

        let mut eq = ResistanceWH::new(cfg.clone());
        eq.init(&cfg, &env(21.0)).unwrap();

        let mut p = ports();
        eq.step(&env(21.0), Duration::from_secs(60), &mut p)
            .unwrap();

        let outlet = eq
            .telemetry()
            .get(tk::OUTLET_TEMP_C)
            .expect("OUTLET_TEMP_C must be present");
        let tank_avg = eq
            .telemetry()
            .get(tk::TANK_AVG_TEMP_C)
            .expect("TANK_AVG_TEMP_C must be present");
        assert!(
            (outlet - tank_avg).abs() < 1.0,
            "OUTLET_TEMP_C should equal tank avg temp with no draw: outlet={outlet}, tank_avg={tank_avg}"
        );
    }
}

#[cfg(test)]
mod element_priority_tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        ControlSignal, EnvironmentState, GridState, OperatingMode, PortSlots, ThermalAccumulator,
        WeatherState, ZoneId, ZoneState,
    };

    use super::{ElementPriorityMode, ResistanceWH};
    use crate::{Equipment, EquipmentConfig};

    fn env_state() -> EnvironmentState {
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
                .expect("UTC offset")
                .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                .single()
                .expect("valid"),
            time_res: ChronoDuration::seconds(60),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    /// Build a config with both elements cold (initial_tank_temp_c well below setpoint).
    /// max_tank_temp_c is set high so the safety clamp never interferes.
    fn cold_config(mode: &str) -> EquipmentConfig {
        EquipmentConfig::from_typed(
            "WH".to_string(),
            "Resistance Water Heater".to_string(),
            crate::ElectricResistanceWaterHeaterConfig {
                equipment_id: None,
                zone_id: None,
                loop_id: None,
                tank_volume_m3: None,
                tank_height_m: None,
                energy_factor: None,
                uniform_energy_factor: None,
                heating_capacity_w: None,
                ua_w_per_k: None,
                setpoint_c: Some(52.0),
                deadband_c: Some(2.0),
                max_tank_temp_c: Some(300.0),
                initial_tank_temp_c: Some(40.0),
                tank_nodes: None,
                avg_water_draw_l_per_day: None,
                draw_flow_rate_kg_s: Some(0.0),
                draw_flow_rate_source: None,
                mains_temp_c_source: None,
                performance_adjustment: None,
                zone_type: None,
                first_hour_rating_m3: None,
                element_power_w: None,
                max_setpoint_ramp_rate_c_per_min: None,
                element_priority_mode: Some(mode.to_string()),
                jacket_r_value_m2_k_w: None,
                max_combined_power_w: None,
                fixture_delivery_temp_c: None,
                hot_draw_temp_c: None,
            },
        )
    }

    fn ports() -> PortSlots {
        PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            electrical: Default::default(),
            fuel: Default::default(),
            fluid: vec![hares_types::FluidAccumulator::new(
                hares_types::LoopId(1),
                hares_types::FluidType::Water,
            )],
            custom: vec![],
            humidity: vec![],
            ..Default::default()
        }
    }

    /// MasterSlave: when the upper element has a call for heat, the lower element
    /// must be locked out even if its own thermostat also calls for heat.
    #[test]
    fn master_slave_upper_firing_locks_out_lower() {
        let cfg = cold_config("MasterSlave");
        let mut wh = ResistanceWH::new(cfg.clone());
        wh.init(&cfg, &env_state()).unwrap();
        // Tank is at 40°C; setpoint=52, deadband=2. Both upper and lower thermostats call.
        // Upper element has priority → lower must be locked out.
        let mode = wh.update_control(&env_state());
        assert!(
            wh.upper_element_on,
            "upper element must fire in MasterSlave mode"
        );
        assert!(
            !wh.lower_element_on,
            "lower element must be locked out when upper is firing"
        );
        assert_eq!(mode, OperatingMode::Heating);
    }

    /// MasterSlave: once the upper node is heated above setpoint, the upper element
    /// turns off and the lower element is free to respond to its own call for heat.
    #[test]
    fn master_slave_upper_satisfied_allows_lower() {
        let cfg = cold_config("MasterSlave");
        let mut wh = ResistanceWH::new(cfg.clone());
        wh.init(&cfg, &env_state()).unwrap();
        // Heat the upper node above setpoint using the same energy as the existing
        // passing test `lower_element_runs_after_upper_is_satisfied`, which concentrates
        // heat enough to satisfy the upper thermostat without heating the whole tank.
        wh.tank
            .heat_node(wh.upper_node, 200_000.0, Duration::from_secs(120))
            .unwrap();
        // Lower node stays cold (40°C), so lower thermostat still calls.
        let mode = wh.update_control(&env_state());
        assert!(
            !wh.upper_element_on,
            "upper element must be off when upper node is above setpoint"
        );
        assert!(
            wh.lower_element_on,
            "lower element must fire once upper is satisfied"
        );
        assert_eq!(mode, OperatingMode::Heating);
    }

    /// Simultaneous: both elements fire independently when both thermostats call.
    #[test]
    fn simultaneous_both_elements_can_fire_together() {
        let cfg = cold_config("Simultaneous");
        let mut wh = ResistanceWH::new(cfg.clone());
        wh.init(&cfg, &env_state()).unwrap();
        // Tank is uniformly cold at 40°C; both thermostats call for heat.
        let mode = wh.update_control(&env_state());
        assert!(
            wh.upper_element_on,
            "upper element must fire in Simultaneous mode"
        );
        assert!(
            wh.lower_element_on,
            "lower element must also fire in Simultaneous mode"
        );
        assert_eq!(mode, OperatingMode::Heating);
    }

    /// MasterSlave recovery: upper fires first until satisfied, then lower takes over.
    /// Verify the transition is clean with no overlap.
    #[test]
    fn master_slave_recovery_sequence_no_overlap() {
        let cfg = cold_config("MasterSlave");
        let mut wh = ResistanceWH::new(cfg.clone());
        wh.init(&cfg, &env_state()).unwrap();

        // Initially both nodes cold -- upper fires, lower locked out.
        wh.update_control(&env_state());
        assert!(wh.upper_element_on);
        assert!(!wh.lower_element_on);

        // Heat upper node above setpoint (same energy as the passing baseline test).
        wh.tank
            .heat_node(wh.upper_node, 200_000.0, Duration::from_secs(120))
            .unwrap();

        // Upper satisfied; lower takes over.
        wh.update_control(&env_state());
        assert!(!wh.upper_element_on, "upper must turn off when satisfied");
        assert!(
            wh.lower_element_on,
            "lower must take over after upper is satisfied"
        );

        // Verify they never both fire at the same time in MasterSlave.
        assert!(
            !(wh.upper_element_on && wh.lower_element_on),
            "MasterSlave must never fire both elements simultaneously"
        );
    }

    /// ModeOverride::Off disables both elements regardless of priority mode.
    #[test]
    fn mode_override_off_disables_both_in_either_mode() {
        for mode_str in ["MasterSlave", "Simultaneous"] {
            let cfg = cold_config(mode_str);
            let mut wh = ResistanceWH::new(cfg.clone());
            wh.init(&cfg, &env_state()).unwrap();
            wh.apply_control(&ControlSignal::ModeOverride {
                mode: OperatingMode::Off,
            })
            .unwrap();
            let mode = wh.update_control(&env_state());
            assert!(
                !wh.upper_element_on,
                "upper must be off with ModeOverride::Off ({mode_str})"
            );
            assert!(
                !wh.lower_element_on,
                "lower must be off with ModeOverride::Off ({mode_str})"
            );
            assert_eq!(mode, OperatingMode::Off, "mode must be Off ({mode_str})");
        }
    }

    /// Verify state round-trip preserves the element_priority field.
    #[test]
    fn state_round_trip_preserves_element_priority() {
        for (mode_str, expected_mode) in [
            ("MasterSlave", ElementPriorityMode::MasterSlave),
            ("Simultaneous", ElementPriorityMode::Simultaneous),
        ] {
            let cfg = cold_config(mode_str);
            let mut wh = ResistanceWH::new(cfg.clone());
            wh.init(&cfg, &env_state()).unwrap();

            let mut p = ports();
            wh.step(&env_state(), Duration::from_secs(60), &mut p)
                .unwrap();
            let saved = wh.save_state().unwrap();

            let mut restored = ResistanceWH::new(cfg.clone());
            restored.init(&cfg, &env_state()).unwrap();
            restored.load_state(&saved).unwrap();

            assert_eq!(
                restored.element_priority, expected_mode,
                "element_priority must survive save/load round-trip ({mode_str})"
            );
        }
    }

    #[test]
    fn state_round_trip_preserves_dr_state() {
        use hares_types::{ControlSignal, DRLevel};

        let cfg = cold_config("MasterSlave");
        let environment = env_state();

        let mut eq = ResistanceWH::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();

        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::Critical,
            duration_s: Some(300.0),
        })
        .unwrap();

        let saved = eq.save_state().unwrap();

        let mut restored = ResistanceWH::new(cfg.clone());
        restored.init(&cfg, &environment).unwrap();
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

    /// Simultaneous mode with ideal capacity: upper node slightly below deadband
    /// floor (low duty) and lower node far below (high duty) must produce
    /// independent duties for each element.
    #[test]
    fn simultaneous_ideal_capacity_independent_duties() {
        let mut env = env_state();
        // 5-minute timestep to activate ideal capacity mode.
        env.time_res = ChronoDuration::seconds(300);

        let cfg = cold_config("Simultaneous");
        let mut wh = ResistanceWH::new(cfg.clone());
        wh.init(&cfg, &env).unwrap();

        // setpoint=52, deadband=2 => deadband floor = 50°C.
        // Upper node at 49.5°C: just below the floor, so upper element fires (small deficit).
        // Lower node at 30°C: far below floor (large deficit).
        // Upper node at 49.5°C < (52 - 2) = 50°C floor, so thermostat calls for heat on first step.
        wh.tank.node_temps_mut()[wh.upper_node] = 49.5;
        wh.tank.node_temps_mut()[wh.lower_node] = 30.0;

        let mut p = ports();
        wh.step(&env, Duration::from_secs(300), &mut p).unwrap();

        let upper_w = wh
            .telemetry()
            .get(hares_types::telemetry_keys::UPPER_ELEMENT_POWER_W)
            .unwrap_or(0.0);
        let lower_w = wh
            .telemetry()
            .get(hares_types::telemetry_keys::LOWER_ELEMENT_POWER_W)
            .unwrap_or(0.0);

        assert!(
            upper_w > 0.0,
            "upper element should deliver some power (node is 2.5K below setpoint)"
        );
        assert!(
            lower_w > 0.0,
            "lower element should deliver power (node is 22K below setpoint)"
        );
        assert!(
            lower_w > upper_w,
            "lower element (cold node) must have higher duty than upper (near setpoint): \
             upper={upper_w:.1} W, lower={lower_w:.1} W"
        );
    }

    /// Simultaneous mode with `max_combined_power_w` set: when both elements fire,
    /// the upper element gets its full power (priority) and the lower element is
    /// clamped to keep total power ≤ max_combined_power_w.
    /// Default elements: 4,500 W each → 9,000 W combined.
    /// max_combined_power_w = 7,200 W → lower limited to 7,200 - 4,500 = 2,700 W.
    #[test]
    fn simultaneous_max_combined_power_clamps_lower_element() {
        let cfg = crate::ElectricResistanceWaterHeaterConfig {
            equipment_id: None,
            zone_id: None,
            loop_id: None,
            tank_volume_m3: None,
            tank_height_m: None,
            energy_factor: None,
            uniform_energy_factor: None,
            heating_capacity_w: None,
            ua_w_per_k: None,
            setpoint_c: Some(52.0),
            deadband_c: Some(2.0),
            max_tank_temp_c: Some(300.0),
            initial_tank_temp_c: Some(40.0),
            tank_nodes: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: Some(0.0),
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            element_power_w: None,
            max_setpoint_ramp_rate_c_per_min: None,
            element_priority_mode: Some("Simultaneous".to_string()),
            jacket_r_value_m2_k_w: None,
            max_combined_power_w: Some(7_200.0),
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
        };
        let config = EquipmentConfig::from_typed(
            "WH".to_string(),
            "Resistance Water Heater".to_string(),
            cfg,
        );

        let mut wh = ResistanceWH::new(config.clone());
        wh.init(&config, &env_state()).unwrap();

        let mut p = ports();
        wh.step(&env_state(), Duration::from_secs(60), &mut p)
            .unwrap();

        let upper_w = wh
            .telemetry()
            .get(hares_types::telemetry_keys::UPPER_ELEMENT_POWER_W)
            .unwrap_or(0.0);
        let lower_w = wh
            .telemetry()
            .get(hares_types::telemetry_keys::LOWER_ELEMENT_POWER_W)
            .unwrap_or(0.0);
        let total_w = upper_w + lower_w;

        assert!(upper_w > 0.0, "upper element must fire");
        assert!(
            lower_w > 0.0,
            "lower element fires but must be clamped, not zero"
        );
        assert!(
            total_w <= 7_200.0 + 1.0,
            "total power {total_w:.1} W must not exceed max_combined_power_w=7200 W"
        );
        assert!(
            lower_w < 4_500.0,
            "lower element power {lower_w:.1} W must be below rated 4500 W, \
             clamped to make room for upper element"
        );
    }

    /// Regression: MasterSlave mode is unaffected by max_combined_power_w.
    /// The clamping only engages for Simultaneous mode. MasterSlave already
    /// enforces single-element operation by hardware interlock.
    #[test]
    fn master_slave_unaffected_by_max_combined_power() {
        for max_w in [None, Some(7_200.0)] {
            let cfg = crate::ElectricResistanceWaterHeaterConfig {
                equipment_id: None,
                zone_id: None,
                loop_id: None,
                tank_volume_m3: None,
                tank_height_m: None,
                energy_factor: None,
                uniform_energy_factor: None,
                heating_capacity_w: None,
                ua_w_per_k: None,
                setpoint_c: Some(52.0),
                deadband_c: Some(2.0),
                max_tank_temp_c: Some(300.0),
                initial_tank_temp_c: Some(40.0),
                tank_nodes: None,
                avg_water_draw_l_per_day: None,
                draw_flow_rate_kg_s: Some(0.0),
                draw_flow_rate_source: None,
                mains_temp_c_source: None,
                performance_adjustment: None,
                zone_type: None,
                first_hour_rating_m3: None,
                element_power_w: None,
                max_setpoint_ramp_rate_c_per_min: None,
                element_priority_mode: Some("MasterSlave".to_string()),
                jacket_r_value_m2_k_w: None,
                max_combined_power_w: max_w,
                fixture_delivery_temp_c: None,
                hot_draw_temp_c: None,
            };
            let config = EquipmentConfig::from_typed(
                "WH".to_string(),
                "Resistance Water Heater".to_string(),
                cfg,
            );

            let mut wh = ResistanceWH::new(config.clone());
            wh.init(&config, &env_state()).unwrap();

            let mut p = ports();
            wh.step(&env_state(), Duration::from_secs(60), &mut p)
                .unwrap();

            let upper_w = wh
                .telemetry()
                .get(hares_types::telemetry_keys::UPPER_ELEMENT_POWER_W)
                .unwrap_or(0.0);
            let lower_w = wh
                .telemetry()
                .get(hares_types::telemetry_keys::LOWER_ELEMENT_POWER_W)
                .unwrap_or(0.0);

            assert!(
                upper_w > 0.0,
                "MasterSlave upper element must fire (max_w={max_w:?})"
            );
            assert_eq!(
                lower_w, 0.0,
                "MasterSlave lower element must be locked out (max_w={max_w:?})"
            );
        }
    }

    /// Save/load round-trip preserves `max_combined_power_w`.
    #[test]
    fn state_round_trip_preserves_max_combined_power_w() {
        let cfg = crate::ElectricResistanceWaterHeaterConfig {
            equipment_id: None,
            zone_id: None,
            loop_id: None,
            tank_volume_m3: None,
            tank_height_m: None,
            energy_factor: None,
            uniform_energy_factor: None,
            heating_capacity_w: None,
            ua_w_per_k: None,
            setpoint_c: Some(52.0),
            deadband_c: Some(2.0),
            max_tank_temp_c: Some(300.0),
            initial_tank_temp_c: Some(40.0),
            tank_nodes: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: Some(0.0),
            draw_flow_rate_source: None,
            mains_temp_c_source: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            element_power_w: None,
            max_setpoint_ramp_rate_c_per_min: None,
            element_priority_mode: Some("Simultaneous".to_string()),
            jacket_r_value_m2_k_w: None,
            max_combined_power_w: Some(6_500.0),
            fixture_delivery_temp_c: None,
            hot_draw_temp_c: None,
        };
        let config = EquipmentConfig::from_typed(
            "WH".to_string(),
            "Resistance Water Heater".to_string(),
            cfg,
        );

        let mut wh = ResistanceWH::new(config.clone());
        wh.init(&config, &env_state()).unwrap();
        assert_eq!(wh.max_combined_power_w, Some(6_500.0));

        let saved = wh.save_state().unwrap();

        let mut restored = ResistanceWH::new(config.clone());
        restored.init(&config, &env_state()).unwrap();
        restored.load_state(&saved).unwrap();

        assert_eq!(
            restored.max_combined_power_w,
            Some(6_500.0),
            "max_combined_power_w must survive save/load round-trip"
        );
    }
}
