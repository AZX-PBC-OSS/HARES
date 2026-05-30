//! Heat pump water heater model.

use std::borrow::Cow;
use std::time::Duration;

use hares_physics::biquadratic::BiquadraticCurve;
use hares_physics::water_density_kg_m3;
use hares_types::{
    ControlCapabilities, ControlSignal, CoreCapabilities, CoreFlows, CoreOutput, CorePerformance,
    CoreState, DRLevel, DutyCycleComponent, ElectricPower, EndUse, EnvironmentState,
    EquipmentDescriptor, EquipmentId, ExecutionStage, FluidType, FuelType, HaresError, LoopId,
    OperatingMode, PortContribution, PortDeclaration, PortSlots, Telemetry, TelemetryField,
    ThermalCategory, ZoneId, telemetry_keys as tk,
};
use serde::{Deserialize, Serialize};

use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_postcard, save_postcard};

pub use super::hpwh_compressor::ElementHpControlMode;
use super::hpwh_compressor::{
    self, DEFAULT_BACKUP_EFFICIENCY, DEFAULT_BACKUP_ELEMENT_POWER_W,
    DEFAULT_BACKUP_ENABLE_OFFSET_C, DEFAULT_CAPACITY_CURVE, DEFAULT_COMPRESSOR_POWER_W,
    DEFAULT_COP_CURVE, DEFAULT_DEADBAND_C, DEFAULT_FAN_POWER_W, DEFAULT_LOST_HEAT_FRACTION,
    DEFAULT_MAX_AMBIENT_TEMP_C, DEFAULT_MIN_AMBIENT_TEMP_C, DEFAULT_MIN_ON_TIME_S,
    DEFAULT_PARASITIC_POWER_W, DEFAULT_RATED_COP, DEFAULT_SHR, DEFAULT_TANK_TEMP_BOUNDS_C,
    DEFAULT_ZONE_TEMP_BOUNDS_C,
};
use super::tank::{StratifiedTank, StratifiedTankConfig, TemperedDrawConfig};
use super::wh_config::HeatPumpWaterHeaterConfig;
use super::{WaterHeaterZip, hysteresis_call, parse_usize, weighted_average_tank_temp};
use crate::hvac::helpers::{equipment_id_from_config, loop_id_from_config, zone_id_from_config};

use super::{
    DEFAULT_CONDUCTIVITY_W_M_K, DEFAULT_MAX_TANK_TEMP_C, DEFAULT_SETPOINT_C,
    DEFAULT_TANK_DIAMETER_M, DEFAULT_TANK_HEIGHT_M, DEFAULT_TANK_VOLUME_M3, DEFAULT_UA_W_PER_K,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct HpwhState {
    setpoint_c: f64,
    target_setpoint_c: f64,
    deadband_c: f64,
    compressor_on: bool,
    backup_on: bool,
    duty_cycle: f64,
    hp_duty_cycle: f64,
    er_duty_cycle: f64,
    mode_override: Option<OperatingMode>,
    element_hp_control: ElementHpControlMode,
    tank_state: Vec<u8>,
    tank_avg_temp_c: f64,
    cop: f64,
    cap_mult: f64,
    electric_kw: f64,
    compressor_power_w: f64,
    backup_element_power_w: f64,
    zone_heat_extraction_w: f64,
    wall_sensible_gain_w: f64,
    unmet_load_w: f64,
    draw_flow_rate_kg_s: f64,
    /// Elapsed time (s) since compressor last turned on; `None` if currently off.
    compressor_on_since_s: Option<f64>,
    /// Elapsed time (s) since compressor last turned off; `None` if currently on.
    compressor_off_since_s: Option<f64>,
    // --- Minimum cycle time thresholds ---
    min_on_time_s: f64,
    min_off_time_s: f64,
    // --- Demand response state ---
    dr_level: DRLevel,
    dr_setpoint_offset_c: f64,
    dr_load_fraction: f64,
    dr_duration_remaining_s: Option<f64>,
}

pub struct HeatPumpWH {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,
    core_output: CoreOutput,
    tank: StratifiedTank,
    thermostat_node: usize,
    /// Upper thermostat node used for the 3/4 weight in composite control temperature.
    ///
    /// OCHRE WaterHeater.py:592-593: HPWH control temperature is a weighted average
    /// of upper (3/4) and lower (1/4) nodes for better representation of usable energy.
    thermostat_upper_node: usize,
    condenser_node: usize,
    /// Fractional weights for distributing condenser heat across tank nodes.
    /// Must sum to a positive value; weights are normalized before use.
    condenser_node_weights: Vec<f64>,
    setpoint_c: f64,
    target_setpoint_c: f64,
    setpoint_ramp_rate_c_per_s: Option<f64>,
    deadband_c: f64,
    duty_cycle: f64,
    /// Per-component duty cycle override for compressor (1.0 = full, 0.0 = off).
    hp_duty_cycle: f64,
    /// Per-component duty cycle override for backup element (1.0 = full, 0.0 = off).
    er_duty_cycle: f64,
    mode_override: Option<OperatingMode>,
    compressor_on: bool,
    backup_on: bool,
    compressor_power_w: f64,
    backup_element_power_w: f64,
    backup_enable_offset_c: f64,
    /// Resistance backup element efficiency (fraction).
    backup_efficiency: f64,
    cop_curve: BiquadraticCurve,
    /// Scaling factor applied to the COP biquadratic output.
    /// When HPXML provides a UEF-derived COP, this anchors the curve to that rated value
    /// by computing scale = cop_rated / cop_curve(rated_conditions).
    /// Defaults to 1.0 (no scaling; pure curve output).
    cop_scale: f64,
    /// Tempering valve delivery temperature (°C) for hot draws (e.g. dishwasher).
    /// Maps to `hot_draw_temp_c` in the TMV logic. Typically 51.67°C (125°F);
    /// `None` means no valve present (defaults to tank setpoint).
    tempering_valve_setpoint_c: Option<f64>,
    /// Fixture delivery temperature for TMV blending (°C). Default 40.6°C.
    fixture_delivery_temp_c: f64,
    capacity_curve: BiquadraticCurve,
    min_ambient_temp_c: f64,
    max_ambient_temp_c: f64,
    max_tank_temp_c: f64,
    /// Sensible heat ratio of zone-air cooling from evaporator.
    shr: f64,
    /// Fraction of waste heat that exits the building rather than entering the zone.
    lost_heat_fraction: f64,
    /// Evaporator fan power (W); runs whenever compressor is on.
    fan_power_w: f64,
    /// Standby parasitic power (W); drawn when compressor is off.
    parasitic_power_w: f64,
    /// Elapsed time since compressor last turned on (s); `None` when compressor is off.
    compressor_on_since_s: Option<f64>,
    /// Elapsed time since compressor last turned off (s); `None` when compressor is on.
    compressor_off_since_s: Option<f64>,
    /// Minimum compressor on-time (s) before an Off transition is allowed.
    min_on_time_s: f64,
    /// Minimum compressor off-time (s) before a restart is allowed.
    /// OCHRE does not implement this; added for realistic compressor cycling.
    min_off_time_s: f64,
    /// When true, the backup resistance element is permanently disabled.
    /// OCHRE WaterHeater.py:521: `if not self.hp_only_mode`.
    hp_only_mode: bool,
    /// Fraction of sensible zone heat gain that goes to interior wall surfaces
    /// rather than the zone air.  0.0 = all to zone air.
    /// OCHRE WaterHeater.py:483: `HPWH Wall Interaction Factor (-)`, default 0.5.
    wall_heat_fraction: f64,
    element_hp_control: ElementHpControlMode,
    loop_id: LoopId,
    fluid_type: FluidType,
    mains_temp_c: f64,
    draw_flow_rate_kg_s: f64,
    zip: WaterHeaterZip,
    // --- Demand response state ---
    dr_setpoint_offset_c: f64,
    dr_load_fraction: f64,
    dr_duration_remaining_s: Option<f64>,
    dr_level: DRLevel,
    // Transient load fraction from LoadFraction control signal; reset each step.
    ctrl_load_fraction: f64,
}

impl HeatPumpWH {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let zone = zone_id_from_config(&config).unwrap_or(ZoneId(1));
        let loop_id = loop_id_from_config(&config, &["loop_id", "dhw_loop_id"]).unwrap_or_default();
        let n_nodes = parse_usize(config.get_f64("tank_nodes"))
            .unwrap_or(6)
            .clamp(1, 12);
        let thermostat_node = parse_usize(config.get_f64("thermostat_node"))
            .unwrap_or(n_nodes - 1)
            .min(n_nodes - 1);
        let condenser_node = parse_usize(config.get_f64("condenser_node"))
            .unwrap_or(n_nodes / 2)
            .min(n_nodes - 1);

        let tank = StratifiedTank::new(StratifiedTankConfig {
            n_nodes,
            height_m: DEFAULT_TANK_HEIGHT_M,
            diameter_m: DEFAULT_TANK_DIAMETER_M,
            ua_w_per_k: DEFAULT_UA_W_PER_K,
            conductivity_w_m_k: DEFAULT_CONDUCTIVITY_W_M_K,
            initial_temp_c: DEFAULT_SETPOINT_C,
            element_nodes: [Some(condenser_node), Some(thermostat_node)],
            node_volumes_m3: None,
            ua_end_cap_w_per_k: None,
        })
        .expect("default HPWH tank config must be valid");

        Self {
            descriptor: EquipmentDescriptor {
                id: EquipmentId(equipment_id_from_config(&config).unwrap_or(0)),
                name: config.name,
                end_use: EndUse::WATER_HEATING,
                equipment_type: Cow::Borrowed("Heat Pump Water Heater"),
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
            thermostat_node,
            thermostat_upper_node: 0,
            condenser_node,
            condenser_node_weights: default_condenser_weights(n_nodes),
            setpoint_c: DEFAULT_SETPOINT_C,
            target_setpoint_c: DEFAULT_SETPOINT_C,
            setpoint_ramp_rate_c_per_s: None,
            deadband_c: DEFAULT_DEADBAND_C,
            duty_cycle: 1.0,
            hp_duty_cycle: 1.0,
            er_duty_cycle: 1.0,
            mode_override: None,
            compressor_on: false,
            backup_on: false,
            compressor_power_w: DEFAULT_COMPRESSOR_POWER_W,
            backup_element_power_w: DEFAULT_BACKUP_ELEMENT_POWER_W,
            backup_enable_offset_c: DEFAULT_BACKUP_ENABLE_OFFSET_C,
            backup_efficiency: DEFAULT_BACKUP_EFFICIENCY,
            cop_curve: BiquadraticCurve {
                coeffs: DEFAULT_COP_CURVE,
                x1_bounds: DEFAULT_ZONE_TEMP_BOUNDS_C,
                x2_bounds: DEFAULT_TANK_TEMP_BOUNDS_C,
                warn_on_clamp: false,
                output_min: None,
                output_max: None,
            },
            cop_scale: 1.0,
            tempering_valve_setpoint_c: None,
            fixture_delivery_temp_c: 40.6,
            capacity_curve: BiquadraticCurve {
                coeffs: DEFAULT_CAPACITY_CURVE,
                x1_bounds: DEFAULT_ZONE_TEMP_BOUNDS_C,
                x2_bounds: DEFAULT_TANK_TEMP_BOUNDS_C,
                warn_on_clamp: false,
                output_min: Some(0.0),
                output_max: None,
            },
            min_ambient_temp_c: DEFAULT_MIN_AMBIENT_TEMP_C,
            max_ambient_temp_c: DEFAULT_MAX_AMBIENT_TEMP_C,
            max_tank_temp_c: DEFAULT_MAX_TANK_TEMP_C,
            shr: DEFAULT_SHR,
            lost_heat_fraction: DEFAULT_LOST_HEAT_FRACTION,
            fan_power_w: DEFAULT_FAN_POWER_W,
            parasitic_power_w: DEFAULT_PARASITIC_POWER_W,
            compressor_on_since_s: None,
            compressor_off_since_s: None,
            min_on_time_s: DEFAULT_MIN_ON_TIME_S,
            min_off_time_s: 0.0,
            hp_only_mode: false,
            wall_heat_fraction: 0.0,
            element_hp_control: ElementHpControlMode::default(),
            loop_id,
            fluid_type: FluidType::Water,
            mains_temp_c: 10.0,
            draw_flow_rate_kg_s: 0.0,
            zip: WaterHeaterZip::default(),
            dr_setpoint_offset_c: 0.0,
            dr_load_fraction: 1.0,
            dr_duration_remaining_s: None,
            dr_level: DRLevel::Normal,
            ctrl_load_fraction: 1.0,
        }
    }

    fn zone_temp_c(&self, env: &EnvironmentState) -> f64 {
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

    /// Wet-bulb temperature of the zone (used for COP/capacity curve input).
    /// Falls back to dry-bulb when the zone is not found.
    fn zone_wet_bulb_c(&self, env: &EnvironmentState) -> f64 {
        self.descriptor
            .zone
            .and_then(|zone| {
                env.zones
                    .iter()
                    .find(|z| z.id == zone)
                    .map(|z| z.wet_bulb_c)
            })
            .unwrap_or_else(|| self.zone_temp_c(env))
    }

    fn ambient_temp_c(&self, env: &EnvironmentState) -> f64 {
        self.zone_temp_c(env)
    }

    /// Composite thermostat temperature: 3/4 upper node + 1/4 lower node.
    ///
    /// OCHRE WaterHeater.py:592-593: HPWH control temperature is a weighted average
    /// of upper (3/4) and lower (1/4) nodes, representing usable energy better than
    /// a single-node sensor.
    fn control_temp_c(&self) -> f64 {
        let temps = self.tank.node_temps();
        let t_upper = temps[self.thermostat_upper_node];
        let t_lower = temps[self.thermostat_node];
        0.75 * t_upper + 0.25 * t_lower
    }

    fn effective_setpoint_c(&self) -> f64 {
        self.setpoint_c + self.dr_setpoint_offset_c
    }

    fn call_for_heat(&self) -> bool {
        hysteresis_call(
            self.control_temp_c(),
            self.effective_setpoint_c(),
            self.deadband_c,
            self.compressor_on || self.backup_on,
        )
    }
}

impl HeatPumpWH {
    fn init_typed(
        &mut self,
        config: &EquipmentConfig,
        env: &EnvironmentState,
    ) -> crate::Result<()> {
        let c = config.require_typed::<HeatPumpWaterHeaterConfig>("Heat Pump Water Heater")?;
        c.validate()?;

        self.descriptor.id = EquipmentId(c.equipment_id.unwrap_or(self.descriptor.id.0));
        let zone = c.zone_id.map(ZoneId).or(self.descriptor.zone);
        self.descriptor.zone = zone;
        self.ports[1].zone = zone;
        self.loop_id = c.loop_id.map(LoopId).unwrap_or(self.loop_id);
        self.ports[2].loop_id = Some(self.loop_id);

        let tank_volume_m3 = c.tank_volume_m3.unwrap_or(DEFAULT_TANK_VOLUME_M3);
        let height_m = c.tank_height_m.unwrap_or(DEFAULT_TANK_HEIGHT_M).max(0.2);
        let diameter_m = (4.0 * tank_volume_m3 / (std::f64::consts::PI * height_m))
            .max(1e-6)
            .sqrt();
        let n_nodes = usize::from(c.tank_nodes.unwrap_or(6).max(1));
        self.thermostat_node = n_nodes.saturating_sub(1);
        self.thermostat_upper_node = 0;
        self.condenser_node = if n_nodes > 1 { n_nodes / 2 } else { 0 };
        self.condenser_node_weights = default_condenser_weights(n_nodes);

        self.setpoint_c = c.setpoint_c.unwrap_or(DEFAULT_SETPOINT_C);
        self.deadband_c = c.deadband_c.unwrap_or(DEFAULT_DEADBAND_C);
        self.setpoint_ramp_rate_c_per_s = None;
        self.target_setpoint_c = self.setpoint_c;

        let initial_tank_temp_c = c
            .initial_tank_temp_c
            .unwrap_or(self.setpoint_c - self.deadband_c / 10.0);

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
            initial_temp_c: initial_tank_temp_c,
            element_nodes: [Some(self.condenser_node), Some(self.thermostat_node)],
            node_volumes_m3: None,
            ua_end_cap_w_per_k: None,
        })?;

        self.duty_cycle = 1.0;
        self.hp_duty_cycle = 1.0;
        self.er_duty_cycle = 1.0;
        self.mode_override = None;
        self.compressor_on = false;
        self.backup_on = false;

        self.compressor_power_w = c.compressor_power_w.unwrap_or(DEFAULT_COMPRESSOR_POWER_W);
        self.backup_element_power_w = c
            .backup_element_power_w
            .unwrap_or(DEFAULT_BACKUP_ELEMENT_POWER_W)
            .max(0.0);
        self.backup_enable_offset_c = c
            .backup_enable_offset_c
            .unwrap_or(DEFAULT_BACKUP_ENABLE_OFFSET_C);

        let cop_rated =
            c.cop.unwrap_or(DEFAULT_RATED_COP) * c.performance_adjustment.unwrap_or(1.0);

        self.cop_curve = BiquadraticCurve {
            coeffs: c.cop_biquadratic_coeffs.unwrap_or(DEFAULT_COP_CURVE),
            x1_bounds: (DEFAULT_ZONE_TEMP_BOUNDS_C.0, DEFAULT_ZONE_TEMP_BOUNDS_C.1),
            x2_bounds: (DEFAULT_TANK_TEMP_BOUNDS_C.0, DEFAULT_TANK_TEMP_BOUNDS_C.1),
            warn_on_clamp: false,
            output_min: None,
            output_max: None,
        };
        let ref_zone_temp = (self.cop_curve.x1_bounds.0 + self.cop_curve.x1_bounds.1) * 0.5;
        let ref_tank_temp = (self.cop_curve.x2_bounds.0 + self.cop_curve.x2_bounds.1) * 0.5;
        let cop_at_ref = self
            .cop_curve
            .evaluate(ref_zone_temp, ref_tank_temp)
            .max(1e-6);
        self.cop_scale = (cop_rated / cop_at_ref).max(0.1);

        self.tempering_valve_setpoint_c = c.tempering_valve_setpoint_c.filter(|&t| t > 0.0);
        self.fixture_delivery_temp_c = c.fixture_delivery_temp_c.unwrap_or(40.6);
        self.capacity_curve = BiquadraticCurve {
            coeffs: c
                .capacity_biquadratic_coeffs
                .unwrap_or(DEFAULT_CAPACITY_CURVE),
            x1_bounds: self.cop_curve.x1_bounds,
            x2_bounds: self.cop_curve.x2_bounds,
            warn_on_clamp: false,
            output_min: Some(0.0),
            output_max: None,
        };

        self.min_ambient_temp_c = c.min_ambient_temp_c.unwrap_or(DEFAULT_MIN_AMBIENT_TEMP_C);
        self.max_ambient_temp_c = c.max_ambient_temp_c.unwrap_or(DEFAULT_MAX_AMBIENT_TEMP_C);
        self.max_tank_temp_c = c.max_tank_temp_c.unwrap_or(DEFAULT_MAX_TANK_TEMP_C);

        self.shr = c.shr.unwrap_or(DEFAULT_SHR);
        self.lost_heat_fraction = c.lost_heat_fraction.unwrap_or(DEFAULT_LOST_HEAT_FRACTION);
        self.wall_heat_fraction = c
            .wall_heat_fraction
            .unwrap_or(match c.zone_type.as_deref() {
                Some("conditioned") => 0.5,
                _ => 0.0,
            });
        self.fan_power_w = c.fan_power_w.unwrap_or(DEFAULT_FAN_POWER_W);
        self.parasitic_power_w = c.parasitic_power_w.unwrap_or(DEFAULT_PARASITIC_POWER_W);
        self.backup_efficiency = c.backup_efficiency.unwrap_or(DEFAULT_BACKUP_EFFICIENCY);
        self.min_on_time_s = c.min_on_time_s.unwrap_or(DEFAULT_MIN_ON_TIME_S);
        self.min_off_time_s = c.min_off_time_s.unwrap_or(0.0);
        self.hp_only_mode = c.hp_only_mode.unwrap_or(false);

        self.element_hp_control = match c.element_hp_control_mode.as_deref() {
            Some("Simultaneous") | Some("simultaneous") => ElementHpControlMode::Simultaneous,
            _ => ElementHpControlMode::MutuallyExclusive,
        };
        self.mains_temp_c = super::require_mains_temp_c(env, "Heat Pump Water Heater")?;
        self.draw_flow_rate_kg_s = c.draw_flow_rate_kg_s.unwrap_or(0.0);
        self.zip = WaterHeaterZip::default();

        self.compressor_on_since_s = None;
        self.compressor_off_since_s = None;
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
}

impl Equipment for HeatPumpWH {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
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
            self.compressor_on = false;
            self.backup_on = false;
            return OperatingMode::Off;
        }

        // DR GridEmergency: full load shed.
        if self.dr_load_fraction <= 0.0 {
            self.compressor_on = false;
            self.backup_on = false;
            return OperatingMode::Off;
        }

        let call_for_heat = self.call_for_heat() && self.duty_cycle > 0.0;
        let ambient_c = self.ambient_temp_c(env);
        let ambient_in_range =
            ambient_c >= self.min_ambient_temp_c && ambient_c <= self.max_ambient_temp_c;

        // Minimum off-time guard: prevent compressor restart until min_off_time_s has elapsed.
        let min_off_elapsed = self
            .compressor_off_since_s
            .is_none_or(|t| t >= self.min_off_time_s);

        // hp_only_mode: backup element is permanently disabled.
        // OCHRE WaterHeater.py:521: `if not self.hp_only_mode`.
        let backup_allowed = !self.hp_only_mode;

        match self.mode_override {
            Some(OperatingMode::Off) => {
                self.compressor_on = false;
                self.backup_on = false;
            }
            Some(OperatingMode::HeatPumpWH) | Some(OperatingMode::HeatingHP) => {
                // Respect mode override but still apply ambient lockout and min-off-time.
                self.compressor_on = call_for_heat && ambient_in_range && min_off_elapsed;
                self.backup_on = false;
            }
            Some(OperatingMode::BackupElement) | Some(OperatingMode::HeatingER) => {
                self.compressor_on = false;
                self.backup_on = call_for_heat && backup_allowed;
            }
            Some(OperatingMode::HeatingHPAndER) => {
                self.compressor_on = call_for_heat && ambient_in_range && min_off_elapsed;
                self.backup_on = call_for_heat && backup_allowed;
            }
            _ => {
                if !ambient_in_range {
                    // Outside operating envelope: compressor locked out, backup only.
                    // OCHRE WaterHeater.py:617-620: er_only_mode when outside bounds.
                    self.compressor_on = false;
                    self.backup_on = call_for_heat && backup_allowed;
                } else {
                    let below_backup_threshold = self.control_temp_c()
                        <= self.effective_setpoint_c() - self.backup_enable_offset_c;
                    match self.element_hp_control {
                        ElementHpControlMode::MutuallyExclusive => {
                            if self.compressor_on {
                                // Compressor already running: keep it, lock out backup.
                                // Enforce min-on-time: only allow turning off when
                                // call_for_heat is false and the timer has expired.
                                let min_on_elapsed = self
                                    .compressor_on_since_s
                                    .is_none_or(|t| t >= self.min_on_time_s);
                                self.compressor_on = call_for_heat || !min_on_elapsed;
                                self.backup_on = false;
                            } else if self.backup_on {
                                // Backup already running: keep it, lock out compressor.
                                self.compressor_on = false;
                                self.backup_on =
                                    call_for_heat && below_backup_threshold && backup_allowed;
                            } else {
                                // Neither running: compressor gets priority (unless min-off active).
                                self.compressor_on = call_for_heat && min_off_elapsed;
                                self.backup_on = false;
                            }
                        }
                        ElementHpControlMode::Simultaneous => {
                            // Enforce min-on-time when compressor is running.
                            let min_on_elapsed = self
                                .compressor_on_since_s
                                .is_none_or(|t| t >= self.min_on_time_s);
                            self.compressor_on = (call_for_heat && min_off_elapsed)
                                || (self.compressor_on && !min_on_elapsed);
                            self.backup_on =
                                call_for_heat && below_backup_threshold && backup_allowed;
                        }
                    }
                }
            }
        }

        match (self.compressor_on, self.backup_on) {
            (true, true) => OperatingMode::HeatingHPAndER,
            (true, false) => OperatingMode::HeatPumpWH,
            (false, true) => OperatingMode::BackupElement,
            (false, false) => OperatingMode::Off,
        }
    }

    fn step(
        &mut self,
        env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        let mode = self.update_control(env);
        let base_fraction =
            (self.duty_cycle * self.dr_load_fraction * self.ctrl_load_fraction).clamp(0.0, 1.0);
        let hp_duty = (base_fraction * self.hp_duty_cycle).clamp(0.0, 1.0);
        let er_duty = (base_fraction * self.er_duty_cycle).clamp(0.0, 1.0);

        // Update compressor on/off timers for min-on-time and min-off-time enforcement.
        if self.compressor_on {
            let elapsed = self.compressor_on_since_s.get_or_insert(0.0);
            *elapsed += dt.as_secs_f64();
            self.compressor_off_since_s = None;
        } else {
            self.compressor_on_since_s = None;
            let off_elapsed = self.compressor_off_since_s.get_or_insert(0.0);
            *off_elapsed += dt.as_secs_f64();
        }

        let tank_avg_temp_c =
            weighted_average_tank_temp(self.tank.node_temps(), self.tank.node_volumes_m3());
        // Use wet-bulb temperature for COP/capacity curves: HPWH performance depends
        // on available enthalpy in the ambient air, not dry-bulb temperature alone.
        let wet_bulb_c = self.zone_wet_bulb_c(env);
        let cop_raw = self.cop_curve.evaluate(wet_bulb_c, tank_avg_temp_c) * self.cop_scale;
        // Divisor floor: when the biquadratic COP curve evaluates to ≤ 0
        // within valid input bounds, the raw COP would be zero or negative,
        // producing divide-by-zero (Inf) in the compressor power calculation
        // and downstream NaN that corrupts tank thermal state. Use a small
        // non-zero floor for the physics computation to keep arithmetic safe.
        let cop_for_physics = cop_raw.max(1e-6);
        // AHRI 210/240-2023: HPWH COP clamp to [0.0, 8.0] to exclude
        // physically impossible values from telemetry reporting.
        let cop = cop_raw.clamp(0.0, 8.0);
        // Capacity multiplier modulates the rated delivered heat based on ambient
        // wet-bulb and tank temperature, matching EnergyPlus/OCHRE HPWH model.
        let cap_mult = self
            .capacity_curve
            .evaluate(wet_bulb_c, tank_avg_temp_c)
            .max(0.0);
        // capacity_actual_w = rated compressor input * cap_mult (rated capacity delivered).
        // power_input_w = capacity_actual_w / cop_actual (electrical input required).
        let capacity_actual_w = self.compressor_power_w * cap_mult;
        let compressor_power_w = if self.compressor_on {
            (capacity_actual_w / cop_for_physics) * hp_duty
        } else {
            0.0
        };
        let backup_power_w = if self.backup_on {
            self.backup_element_power_w * er_duty
        } else {
            0.0
        };

        // OCHRE WaterHeater.py:660-664: delivered heat from HP and ER.
        // HP delivers capacity_actual_w * duty to tank; electrical draw is compressor_power_w.
        let delivered_hp_w = compressor_power_w * cop_for_physics;
        let delivered_er_w = backup_power_w * self.backup_efficiency;
        let q_tank_delivered_w = delivered_hp_w + delivered_er_w;

        let heat_injections = build_heat_injections(
            &self.condenser_node_weights,
            q_tank_delivered_w,
            self.tank.n_nodes(),
        );

        let appliance_demand_kg_s = super::read_dhw_demand_kg_s(ports);
        let total_draw_kg_s = self.draw_flow_rate_kg_s + appliance_demand_kg_s;
        let hot_draw_temp_c = self.tempering_valve_setpoint_c.unwrap_or(self.setpoint_c);
        let tmv = TemperedDrawConfig {
            tempered_draw_temp_c: self.fixture_delivery_temp_c,
            hot_draw_temp_c,
            setpoint_temp_c: self.setpoint_c,
        };
        let tempered_flow_m3_s =
            self.draw_flow_rate_kg_s / water_density_kg_m3(self.tank.node_temps()[0]);
        let hot_flow_m3_s = appliance_demand_kg_s / water_density_kg_m3(self.tank.node_temps()[0]);
        let draw = self.tank.step_tempered(
            self.ambient_temp_c(env),
            tempered_flow_m3_s,
            hot_flow_m3_s,
            self.mains_temp_c,
            &heat_injections,
            tmv,
            dt,
        )?;

        // OCHRE WaterHeater.py:662: fan runs when compressor is on; parasitic when off.
        let fan_parasitic_w = if self.compressor_on {
            self.fan_power_w
        } else {
            self.parasitic_power_w
        };

        // OCHRE WaterHeater.py:671: total electric = compressor + ER + fan/parasitic.
        let rated_electric_power_w = compressor_power_w + backup_power_w + fan_parasitic_w;
        let (electric_power_w, reactive_power_kvar) =
            self.zip.apply(rated_electric_power_w, env.grid.voltage_pu);

        if electric_power_w > 0.0 || reactive_power_kvar != 0.0 {
            ports.accumulate(&PortContribution::Electrical {
                active_power_kw: electric_power_w / 1_000.0,
                reactive_power_kvar,
            })?;
        }

        // OCHRE WaterHeater.py:666-674: zone heat gains decomposed by SHR and lost_heat_fraction.
        // hp_waste = power_hp - delivered_hp (negative: HP extracts heat from zone)
        // er_waste = power_er - delivered_er (zero for 100% efficient electric)
        let dry_bulb_c = self.zone_temp_c(env);
        let shr = if (dry_bulb_c - wet_bulb_c) > 0.1 {
            self.shr
        } else {
            1.0
        };
        let hp_waste_w = compressor_power_w - delivered_hp_w; // negative when COP > 1
        let er_waste_w = backup_power_w - delivered_er_w; // zero for ideal ER
        let keep_fraction = 1.0 - self.lost_heat_fraction;
        let sensible_gain_w = (hp_waste_w * shr + fan_parasitic_w + er_waste_w) * keep_fraction;
        let latent_gain_w = hp_waste_w * (1.0 - shr) * keep_fraction;

        // zone_heat_extraction_w records the total heat moved from zone to tank.
        let zone_heat_extraction_w = delivered_hp_w - compressor_power_w;

        // OCHRE WaterHeater.py:678-685: split sensible gain between zone air and interior wall.
        // The wall share is tracked separately so downstream envelope solvers can
        // preserve the physical distinction between zone air and jacket coupling.
        let sensible_to_zone_w = sensible_gain_w * (1.0 - self.wall_heat_fraction);
        let sensible_to_wall_w = sensible_gain_w * self.wall_heat_fraction;

        if let Some(zone) = self.descriptor.zone {
            if sensible_to_zone_w != 0.0 || latent_gain_w != 0.0 {
                ports.accumulate(&PortContribution::Thermal {
                    zone,
                    sensible_gain_w: sensible_to_zone_w,
                    radiant_gain_w: 0.0,
                    latent_gain_w,
                    // HPWH compressor waste heat and evaporator moisture removal
                    // are mechanical equipment effects, not passive internal gains.
                    // EnergyPlus Engineering Reference: the HP evaporator extracts
                    // sensible + latent heat from zone air — the same physics as a
                    // standalone dehumidifier. OCHRE WaterHeater.py folds this into
                    // internal_sens_gain but HARES separates it for per-category
                    // diagnostics (mechanical conditioning vs occupant/appliance heat).
                    category: ThermalCategory::HvacDehumidification,
                })?;
            }
            if sensible_to_wall_w != 0.0 {
                ports.accumulate(&PortContribution::Thermal {
                    zone,
                    sensible_gain_w: sensible_to_wall_w,
                    radiant_gain_w: 0.0,
                    latent_gain_w: 0.0,
                    // Compressor waste heat routed to interior wall face.
                    // Shares JacketLoss with tank skin conduction (below);
                    // the two are distinguishable in per-category diagnostics
                    // only by telemetry (skin_loss_w, wall_sensible_gain_w).
                    // A dedicated HvacWasteHeat variant would allow full
                    // separation of HP-cycle losses from tank losses.
                    category: ThermalCategory::JacketLoss,
                })?;
            }
        }

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

        if total_draw_kg_s > 0.0 {
            // TMV blending is handled inside step_tempered; outlet_temp_c already
            // reflects the delivered temperature after mixing-valve adjustment.
            ports.accumulate(&PortContribution::Fluid {
                loop_id: self.loop_id,
                flow_rate_kg_s: total_draw_kg_s,
                supply_temp_c: draw.outlet_temp_c,
                return_temp_c: self.mains_temp_c,
                fluid_type: self.fluid_type,
                thermal_power_w: None,
            })?;
        }

        self.telemetry.set(
            tk::TANK_AVG_TEMP_C,
            weighted_average_tank_temp(self.tank.node_temps(), self.tank.node_volumes_m3()),
        );
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        debug_assert!(
            cop.is_finite() && (0.0..=8.0).contains(&cop),
            "HPWH COP {cop} not in [0.0, 8.0]"
        );
        self.telemetry.set(tk::COP, cop);
        self.telemetry.set(tk::CAP_MULT, cap_mult);
        self.telemetry
            .set(tk::ELECTRIC_KW, electric_power_w / 1_000.0);
        self.telemetry
            .set(tk::COMPRESSOR_KW, compressor_power_w / 1_000.0);
        self.telemetry.set(tk::ELEMENT_KW, backup_power_w / 1_000.0);
        self.telemetry
            .set(tk::COMPRESSOR_POWER_W, compressor_power_w);
        self.telemetry
            .set(tk::BACKUP_ELEMENT_POWER_W, backup_power_w);
        self.telemetry
            .set(tk::ZONE_HEAT_EXTRACTION_W, zone_heat_extraction_w);
        self.telemetry.set(tk::DRAW_FLOW_RATE_KG_S, total_draw_kg_s);
        self.telemetry
            .set(tk::WALL_SENSIBLE_GAIN_W, sensible_to_wall_w);
        self.telemetry.set(tk::UNMET_LOAD_W, draw.unmet_load_w);
        self.telemetry.set(
            tk::OPERATING_MODE,
            match mode {
                OperatingMode::HeatPumpWH => 1.0,
                OperatingMode::BackupElement => 2.0,
                OperatingMode::HeatingHPAndER => 3.0,
                _ => 0.0,
            },
        );
        self.tank.update_node_telemetry(&mut self.telemetry);
        let core_output = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(
                    (electric_power_w / 1_000.0).max(0.0),
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

    fn save_state(&self) -> Vec<u8> {
        save_postcard(&HpwhState {
            setpoint_c: self.setpoint_c,
            target_setpoint_c: self.target_setpoint_c,
            deadband_c: self.deadband_c,
            compressor_on: self.compressor_on,
            backup_on: self.backup_on,
            duty_cycle: self.duty_cycle,
            hp_duty_cycle: self.hp_duty_cycle,
            er_duty_cycle: self.er_duty_cycle,
            mode_override: self.mode_override,
            element_hp_control: self.element_hp_control,
            tank_state: self.tank.save_state(),
            tank_avg_temp_c: self.telemetry.get(tk::TANK_AVG_TEMP_C).unwrap_or(0.0),
            cop: self.telemetry.get(tk::COP).unwrap_or(0.0),
            cap_mult: self.telemetry.get(tk::CAP_MULT).unwrap_or(1.0),
            electric_kw: self.telemetry.get(tk::ELECTRIC_KW).unwrap_or(0.0),
            compressor_power_w: self.telemetry.get(tk::COMPRESSOR_POWER_W).unwrap_or(0.0),
            backup_element_power_w: self
                .telemetry
                .get(tk::BACKUP_ELEMENT_POWER_W)
                .unwrap_or(0.0),
            zone_heat_extraction_w: self
                .telemetry
                .get(tk::ZONE_HEAT_EXTRACTION_W)
                .unwrap_or(0.0),
            wall_sensible_gain_w: self.telemetry.get(tk::WALL_SENSIBLE_GAIN_W).unwrap_or(0.0),
            unmet_load_w: self.telemetry.get(tk::UNMET_LOAD_W).unwrap_or(0.0),
            draw_flow_rate_kg_s: self.telemetry.get(tk::DRAW_FLOW_RATE_KG_S).unwrap_or(0.0),
            compressor_on_since_s: self.compressor_on_since_s,
            compressor_off_since_s: self.compressor_off_since_s,
            min_on_time_s: self.min_on_time_s,
            min_off_time_s: self.min_off_time_s,
            dr_level: self.dr_level,
            dr_setpoint_offset_c: self.dr_setpoint_offset_c,
            dr_load_fraction: self.dr_load_fraction,
            dr_duration_remaining_s: self.dr_duration_remaining_s,
        })
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let decoded: HpwhState = load_postcard(state)?;
        self.setpoint_c = decoded.setpoint_c;
        self.target_setpoint_c = decoded.target_setpoint_c;
        self.deadband_c = decoded.deadband_c;
        self.compressor_on = decoded.compressor_on;
        self.backup_on = decoded.backup_on;
        self.duty_cycle = decoded.duty_cycle;
        self.hp_duty_cycle = decoded.hp_duty_cycle;
        self.er_duty_cycle = decoded.er_duty_cycle;
        self.mode_override = decoded.mode_override;
        self.element_hp_control = decoded.element_hp_control;
        self.compressor_on_since_s = decoded.compressor_on_since_s;
        self.compressor_off_since_s = decoded.compressor_off_since_s;
        self.min_on_time_s = decoded.min_on_time_s;
        self.min_off_time_s = decoded.min_off_time_s;
        self.dr_level = decoded.dr_level;
        self.dr_setpoint_offset_c = decoded.dr_setpoint_offset_c;
        self.dr_load_fraction = decoded.dr_load_fraction;
        self.dr_duration_remaining_s = decoded.dr_duration_remaining_s;
        self.tank.load_state(&decoded.tank_state)?;

        self.telemetry
            .insert(tk::TANK_AVG_TEMP_C, decoded.tank_avg_temp_c);
        self.telemetry.insert(tk::COP, decoded.cop);
        self.telemetry.insert(tk::CAP_MULT, decoded.cap_mult);
        self.telemetry.insert(tk::ELECTRIC_KW, decoded.electric_kw);
        self.telemetry
            .insert(tk::COMPRESSOR_KW, decoded.compressor_power_w / 1_000.0);
        self.telemetry
            .insert(tk::ELEMENT_KW, decoded.backup_element_power_w / 1_000.0);
        self.telemetry
            .insert(tk::COMPRESSOR_POWER_W, decoded.compressor_power_w);
        self.telemetry
            .insert(tk::BACKUP_ELEMENT_POWER_W, decoded.backup_element_power_w);
        self.telemetry
            .insert(tk::ZONE_HEAT_EXTRACTION_W, decoded.zone_heat_extraction_w);
        self.telemetry
            .insert(tk::WALL_SENSIBLE_GAIN_W, decoded.wall_sensible_gain_w);
        self.telemetry
            .insert(tk::UNMET_LOAD_W, decoded.unmet_load_w);
        self.telemetry
            .insert(tk::DRAW_FLOW_RATE_KG_S, decoded.draw_flow_rate_kg_s);
        self.telemetry.insert(
            tk::OPERATING_MODE,
            match (decoded.compressor_on, decoded.backup_on) {
                (true, false) => 1.0,
                (false, true) => 2.0,
                (true, true) => 3.0,
                (false, false) => 0.0,
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
            ControlSignal::DutyCycle {
                on_fraction,
                component,
                ..
            } => {
                if !on_fraction.is_finite() || !(0.0..=1.0).contains(on_fraction) {
                    return Err(HaresError::Control(format!(
                        "invalid duty cycle for HeatPumpWH: {on_fraction}"
                    )));
                }
                match component {
                    Some(DutyCycleComponent::Compressor) => {
                        self.hp_duty_cycle = *on_fraction;
                    }
                    Some(DutyCycleComponent::BackupElement) => {
                        self.er_duty_cycle = *on_fraction;
                    }
                    None => {
                        self.duty_cycle = *on_fraction;
                    }
                }
            }
            ControlSignal::ModeOverride { mode } => {
                self.mode_override = Some(*mode);
            }
            ControlSignal::LoadFraction { fraction } => {
                self.ctrl_load_fraction = fraction.clamp(0.0, 1.0);
            }
            ControlSignal::PowerLimit { max_power_kw, .. } => {
                let rated_w = self.compressor_power_w + self.backup_element_power_w;
                if rated_w > 0.0 {
                    let max_fraction = (max_power_kw * 1000.0 / rated_w).clamp(0.0, 1.0);
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

impl HeatPumpWH {
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
        "Heat Pump Water Heater",
        Box::new(|config| Box::new(HeatPumpWH::new(config))),
    );
    registry.register("HPWH", Box::new(|config| Box::new(HeatPumpWH::new(config))));
}

fn default_telemetry() -> Telemetry {
    let mut telemetry = Telemetry::with_capacity(12);
    telemetry.insert(tk::TANK_AVG_TEMP_C, 0.0);
    telemetry.insert(tk::COP, 0.0);
    telemetry.insert(tk::CAP_MULT, 1.0);
    telemetry.insert(tk::ELECTRIC_KW, 0.0);
    telemetry.insert(tk::COMPRESSOR_KW, 0.0);
    telemetry.insert(tk::ELEMENT_KW, 0.0);
    telemetry.insert(tk::COMPRESSOR_POWER_W, 0.0);
    telemetry.insert(tk::BACKUP_ELEMENT_POWER_W, 0.0);
    telemetry.insert(tk::ZONE_HEAT_EXTRACTION_W, 0.0);
    telemetry.insert(tk::DRAW_FLOW_RATE_KG_S, 0.0);
    telemetry.insert(tk::OPERATING_MODE, 0.0);
    telemetry.insert(tk::WALL_SENSIBLE_GAIN_W, 0.0);
    telemetry.insert(tk::UNMET_LOAD_W, 0.0);
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
            name: tk::COP.to_string(),
            unit: "-".to_string(),
            description: "Instantaneous heat-pump COP".to_string(),
        },
        TelemetryField {
            name: tk::CAP_MULT.to_string(),
            unit: "-".to_string(),
            description: "Capacity curve multiplier (function of wet-bulb and tank temperature)"
                .to_string(),
        },
        TelemetryField {
            name: tk::ELECTRIC_KW.to_string(),
            unit: "kW".to_string(),
            description: "Total electrical power draw (compressor + backup + fan + parasitic)"
                .to_string(),
        },
        TelemetryField {
            name: tk::COMPRESSOR_KW.to_string(),
            unit: "kW".to_string(),
            description: "Heat pump compressor electric power".to_string(),
        },
        TelemetryField {
            name: tk::ELEMENT_KW.to_string(),
            unit: "kW".to_string(),
            description: "Backup resistance element electric power".to_string(),
        },
        TelemetryField {
            name: tk::COMPRESSOR_POWER_W.to_string(),
            unit: "W".to_string(),
            description: "Compressor electric power".to_string(),
        },
        TelemetryField {
            name: tk::BACKUP_ELEMENT_POWER_W.to_string(),
            unit: "W".to_string(),
            description: "Backup resistance element power".to_string(),
        },
        TelemetryField {
            name: tk::ZONE_HEAT_EXTRACTION_W.to_string(),
            unit: "W".to_string(),
            description: "Heat extracted from surrounding zone air".to_string(),
        },
        TelemetryField {
            name: tk::DRAW_FLOW_RATE_KG_S.to_string(),
            unit: "kg/s".to_string(),
            description: "Domestic hot water draw flow rate".to_string(),
        },
        TelemetryField {
            name: tk::OPERATING_MODE.to_string(),
            unit: "enum".to_string(),
            description: "0=Off, 1=HP, 2=Backup, 3=HP+Backup".to_string(),
        },
        TelemetryField {
            name: tk::WALL_SENSIBLE_GAIN_W.to_string(),
            unit: "W".to_string(),
            description:
                "Sensible heat directed to interior wall surfaces (wall_heat_fraction share)"
                    .to_string(),
        },
        TelemetryField {
            name: tk::UNMET_LOAD_W.to_string(),
            unit: "W".to_string(),
            description:
                "Unmet fixture load: heat not delivered because outlet temp < fixture setpoint"
                    .to_string(),
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

fn build_heat_injections(weights: &[f64], q_w: f64, n_nodes: usize) -> Vec<(usize, f64)> {
    hpwh_compressor::build_heat_injections(weights, q_w, n_nodes)
}

fn default_condenser_weights(n_nodes: usize) -> Vec<f64> {
    hpwh_compressor::default_condenser_weights(n_nodes)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        EnvironmentState, GridState, PortSlots, ThermalAccumulator, WeatherState, ZoneId,
        ZoneState, telemetry_keys as tk,
    };

    use super::{HeatPumpWH, weighted_average_tank_temp};
    use crate::{Equipment, EquipmentConfig, EquipmentTypedConfig, HeatPumpWaterHeaterConfig};

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

    pub(super) fn base_typed_config() -> HeatPumpWaterHeaterConfig {
        HeatPumpWaterHeaterConfig {
            equipment_id: None,
            zone_id: Some(1),
            loop_id: Some(1),
            tank_volume_m3: None,
            tank_height_m: None,
            cop: Some(2.5),
            backup_element_power_w: Some(4500.0),
            ua_w_per_k: None,
            setpoint_c: Some(52.0),
            deadband_c: Some(2.0),
            max_tank_temp_c: Some(300.0),
            initial_tank_temp_c: Some(40.0),
            tank_nodes: None,
            tempering_valve_setpoint_c: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: Some(0.0),
            compressor_power_w: Some(1200.0),
            backup_enable_offset_c: Some(3.0),
            min_ambient_temp_c: None,
            max_ambient_temp_c: None,
            min_on_time_s: Some(0.0),
            min_off_time_s: Some(0.0),
            hp_only_mode: Some(false),
            element_hp_control_mode: None,
            fan_power_w: Some(35.0),
            parasitic_power_w: Some(1.0),
            backup_efficiency: Some(1.0),
            shr: Some(0.88),
            lost_heat_fraction: Some(0.0),
            wall_heat_fraction: Some(0.0),
            capacity_biquadratic_coeffs: None,
            cop_biquadratic_coeffs: None,
            performance_adjustment: Some(1.0),
            zone_type: Some("conditioned".to_string()),
            first_hour_rating_m3: None,
            jacket_r_value_m2_k_w: None,
            fixture_delivery_temp_c: None,
        }
    }

    pub(super) fn equipment_config(typed: HeatPumpWaterHeaterConfig) -> EquipmentConfig {
        EquipmentConfig::from_typed(
            "HPWH".to_string(),
            HeatPumpWaterHeaterConfig::equipment_type_name().to_string(),
            typed,
        )
    }

    pub(super) fn config() -> EquipmentConfig {
        equipment_config(base_typed_config())
    }

    #[test]
    fn typed_init_uses_hpxml_heating_capacity_for_backup_element_power() {
        let typed = HeatPumpWaterHeaterConfig {
            equipment_id: None,
            zone_id: Some(1),
            loop_id: None,
            tank_volume_m3: None,
            tank_height_m: None,
            cop: Some(2.5),
            backup_element_power_w: Some(4500.0),
            ua_w_per_k: None,
            setpoint_c: Some(52.0),
            deadband_c: None,
            max_tank_temp_c: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            tempering_valve_setpoint_c: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: None,
            compressor_power_w: None,
            backup_enable_offset_c: None,
            min_ambient_temp_c: None,
            max_ambient_temp_c: None,
            min_on_time_s: None,
            min_off_time_s: None,
            hp_only_mode: None,
            element_hp_control_mode: None,
            fan_power_w: None,
            parasitic_power_w: None,
            backup_efficiency: None,
            shr: None,
            lost_heat_fraction: None,
            wall_heat_fraction: None,
            capacity_biquadratic_coeffs: None,
            cop_biquadratic_coeffs: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            jacket_r_value_m2_k_w: None,
            fixture_delivery_temp_c: None,
        };
        let config = equipment_config(typed);
        let env = env(20.0);
        let mut eq = HeatPumpWH::new(config.clone());
        eq.init(&config, &env).expect("typed init should succeed");

        assert_eq!(eq.backup_element_power_w, 4500.0);
    }

    #[test]
    fn typed_init_defaults_initial_tank_temp_close_to_top_of_deadband() {
        let mut typed = base_typed_config();
        typed.initial_tank_temp_c = None;
        let config = equipment_config(typed.clone());
        let env = env(20.0);

        let mut eq = HeatPumpWH::new(config.clone());
        eq.init(&config, &env).expect("typed init should succeed");

        let expected = typed.setpoint_c.expect("base typed config has setpoint")
            - typed.deadband_c.expect("base typed config has deadband") / 10.0;
        let actual = weighted_average_tank_temp(eq.tank.node_temps(), eq.tank.node_volumes_m3());

        assert!(
            (actual - expected).abs() < 1e-9,
            "unspecified initial_tank_temp_c must default near the top of the deadband: expected {expected}, got {actual}"
        );
    }

    #[test]
    fn typed_init_honors_explicit_initial_tank_temp_override() {
        let mut typed = base_typed_config();
        typed.initial_tank_temp_c = Some(41.25);
        let config = equipment_config(typed.clone());
        let env = env(20.0);

        let mut eq = HeatPumpWH::new(config.clone());
        eq.init(&config, &env).expect("typed init should succeed");

        let actual = weighted_average_tank_temp(eq.tank.node_temps(), eq.tank.node_volumes_m3());
        assert!(
            (actual - 41.25).abs() < 1e-9,
            "explicit initial_tank_temp_c must be preserved"
        );
    }

    #[test]
    fn hpwh_default_thermal_capacity_matches_ochre() {
        let typed = HeatPumpWaterHeaterConfig {
            equipment_id: None,
            zone_id: Some(1),
            loop_id: None,
            tank_volume_m3: None,
            tank_height_m: None,
            cop: None,
            backup_element_power_w: None,
            ua_w_per_k: None,
            setpoint_c: None,
            deadband_c: None,
            max_tank_temp_c: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            tempering_valve_setpoint_c: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: None,
            compressor_power_w: None,
            backup_enable_offset_c: None,
            min_ambient_temp_c: None,
            max_ambient_temp_c: None,
            min_on_time_s: None,
            min_off_time_s: None,
            hp_only_mode: None,
            element_hp_control_mode: None,
            fan_power_w: None,
            parasitic_power_w: None,
            backup_efficiency: None,
            shr: None,
            lost_heat_fraction: None,
            wall_heat_fraction: None,
            capacity_biquadratic_coeffs: None,
            cop_biquadratic_coeffs: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            jacket_r_value_m2_k_w: None,
            fixture_delivery_temp_c: None,
        };
        let config = equipment_config(typed);
        let e = env(20.0);
        let mut eq = HeatPumpWH::new(config.clone());
        eq.init(&config, &e).expect("init should succeed");
        assert!(
            (eq.compressor_power_w - 1_725.0).abs() < 1e-9,
            "default thermal capacity must be 500 W × 3.45 COP = 1725 W; got {}",
            eq.compressor_power_w
        );
    }

    #[test]
    fn low_power_hpwh_thermal_capacity_is_1499_4() {
        let typed = HeatPumpWaterHeaterConfig {
            equipment_id: None,
            zone_id: Some(1),
            loop_id: None,
            tank_volume_m3: None,
            tank_height_m: None,
            cop: None,
            backup_element_power_w: None,
            ua_w_per_k: None,
            setpoint_c: None,
            deadband_c: None,
            max_tank_temp_c: None,
            initial_tank_temp_c: None,
            tank_nodes: None,
            tempering_valve_setpoint_c: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: None,
            compressor_power_w: Some(1_499.4),
            backup_enable_offset_c: None,
            min_ambient_temp_c: None,
            max_ambient_temp_c: None,
            min_on_time_s: None,
            min_off_time_s: None,
            hp_only_mode: Some(true),
            element_hp_control_mode: None,
            fan_power_w: None,
            parasitic_power_w: None,
            backup_efficiency: None,
            shr: None,
            lost_heat_fraction: None,
            wall_heat_fraction: None,
            capacity_biquadratic_coeffs: None,
            cop_biquadratic_coeffs: None,
            performance_adjustment: None,
            zone_type: None,
            first_hour_rating_m3: None,
            jacket_r_value_m2_k_w: None,
            fixture_delivery_temp_c: None,
        };
        let config = equipment_config(typed);
        let e = env(20.0);
        let mut eq = HeatPumpWH::new(config.clone());
        eq.init(&config, &e).expect("init should succeed");
        assert!(
            (eq.compressor_power_w - 1_499.4).abs() < 1e-9,
            "low-power HPWH thermal capacity must be 1499.4 W; got {}",
            eq.compressor_power_w
        );
    }

    #[test]
    fn hpwh_skin_loss_telemetry_populated_after_step() {
        let mut typed = base_typed_config();
        typed.initial_tank_temp_c = Some(55.0);
        typed.draw_flow_rate_kg_s = Some(0.0);
        let config = equipment_config(typed);
        let e = env(15.0);
        let mut eq = HeatPumpWH::new(config.clone());
        eq.init(&config, &e).expect("init should succeed");
        let mut p = ports();
        eq.step(&e, Duration::from_secs(60), &mut p)
            .expect("step should succeed");
        let skin_loss = eq.telemetry().get(tk::SKIN_LOSS_W).unwrap_or(0.0);
        assert!(
            skin_loss > 0.0,
            "SKIN_LOSS_W must be positive when tank ({} °C) is hotter than ambient (15 °C); got {skin_loss}",
            55.0
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

    fn env_with_wet_bulb(zone_temp_c: f64, wet_bulb_c: f64) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp_c,
                humidity_ratio: 0.008,
                relative_humidity: 0.45,
                wet_bulb_c,
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

    #[test]
    fn zone_thermal_port_is_negative_when_compressor_runs() {
        let mut eq = HeatPumpWH::new(config());
        eq.init(&config(), &env(24.0)).unwrap();

        let mut p = ports();
        eq.step(&env(24.0), Duration::from_secs(60), &mut p)
            .unwrap();

        assert!(eq.telemetry().get(tk::COMPRESSOR_POWER_W).unwrap_or(0.0) > 0.0);
        assert!(p.thermal[0].sensible_gain_w < 0.0);
    }

    /// zone_heat_extraction_w = comp*COP - comp (heat moved from zone to tank by HP).
    /// This is separate from fan/parasitic and ER contributions.
    #[test]
    fn hpwh_energy_balance_holds_each_step() {
        let mut eq = HeatPumpWH::new(config());
        eq.init(&config(), &env(24.0)).unwrap();

        let mut p = ports();
        eq.step(&env(24.0), Duration::from_secs(60), &mut p)
            .unwrap();

        let cop = eq.telemetry().get(tk::COP).unwrap();
        let comp = eq.telemetry().get(tk::COMPRESSOR_POWER_W).unwrap();
        let extracted = eq.telemetry().get(tk::ZONE_HEAT_EXTRACTION_W).unwrap();

        // extracted = delivered_hp - comp = comp * cop - comp = comp * (cop - 1)
        let expected_extraction = comp * (cop - 1.0);
        assert!(
            (extracted - expected_extraction).abs() < 1e-6,
            "zone extraction should be comp*(COP-1): {extracted} vs {expected_extraction}"
        );
    }

    // --- Regression tests for bug fixes ---

    /// Same dry-bulb temperature but different wet-bulb temperatures must produce
    /// different COP values, confirming the curve uses wet-bulb not dry-bulb.
    #[test]
    fn wet_bulb_cop_differs_with_same_dry_bulb_different_humidity() {
        let dry_bulb_c = 24.0;
        // Low humidity → low wet-bulb (~14°C at 24°C DB)
        let env_low_wb = env_with_wet_bulb(dry_bulb_c, 14.0);
        // High humidity → high wet-bulb (~20°C at 24°C DB, ~80% RH)
        let env_high_wb = env_with_wet_bulb(dry_bulb_c, 20.0);

        let mut eq_low = HeatPumpWH::new(config());
        eq_low.init(&config(), &env_low_wb).unwrap();
        let mut p_low = ports();
        eq_low
            .step(&env_low_wb, Duration::from_secs(60), &mut p_low)
            .unwrap();
        let cop_low_wb = eq_low.telemetry().get(tk::COP).unwrap();

        let mut eq_high = HeatPumpWH::new(config());
        eq_high.init(&config(), &env_high_wb).unwrap();
        let mut p_high = ports();
        eq_high
            .step(&env_high_wb, Duration::from_secs(60), &mut p_high)
            .unwrap();
        let cop_high_wb = eq_high.telemetry().get(tk::COP).unwrap();

        // Higher wet-bulb → more available enthalpy → higher COP.
        // The default COP curve has a positive first-order wet-bulb coefficient.
        assert!(
            cop_high_wb > cop_low_wb,
            "COP at WB=20°C ({cop_high_wb:.4}) should exceed COP at WB=14°C ({cop_low_wb:.4})"
        );
    }

    #[test]
    fn hpwh_cop_clamped_to_physical_range() {
        // HPWH with pathological COP curve (c0=100) and rated COP=100
        // produces unbounded per-step COP ~100. Verify telemetry COP is
        // clamped to [0.0, 8.0].
        let typed = HeatPumpWaterHeaterConfig {
            cop: Some(100.0),
            cop_biquadratic_coeffs: Some([100.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
            ..base_typed_config()
        };
        let cfg = equipment_config(typed);
        let mut eq = HeatPumpWH::new(cfg.clone());
        // Zone at 24°C (well above default COP curve ref), tank at 40°C.
        eq.init(&cfg, &env(24.0)).unwrap();
        let mut ports = ports();
        eq.step(&env(24.0), Duration::from_secs(60), &mut ports)
            .unwrap();

        let cop = eq.telemetry().get(tk::COP).unwrap_or(-1.0);
        assert!(
            cop.is_finite() && (0.0..=8.0).contains(&cop),
            "HPWH COP must be in [0.0, 8.0], got {cop}"
        );
        // Unclamped COP would be ~100; clamping must have reduced it.
        assert!(
            cop < 50.0,
            "COP must have been clamped below 50 (raw ~100), got {cop}"
        );
    }

    /// HPWH with a zero-output biquadratic COP curve produces a near-zero
    /// raw COP at every evaluation point. The divisor floor (1e-6) must prevent
    /// divide-by-zero / NaN in the per-step energy balance and delivered heat.
    /// Regression for the lower-bound path — without the fix, `cop == 0.0` is
    /// used as a divisor, producing Inf → NaN that corrupts tank thermal state.
    #[test]
    fn hpwh_cop_zero_curve_prevents_nan_in_energy_balance() {
        // All-zero biquadratic: evaluate() returns 0 for every input pair
        // within bounds. cop_ratio becomes (rated_cop / max(0, 1e-6)) ≈ huge,
        // but the product 0.0 * huge = 0.0, giving a raw COP of 0.0.
        // The divisor floor in cop_for_physics must keep arithmetic safe.
        let typed = HeatPumpWaterHeaterConfig {
            cop_biquadratic_coeffs: Some([0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
            cop: Some(3.45),
            ..base_typed_config()
        };
        let cfg = equipment_config(typed);
        let mut eq = HeatPumpWH::new(cfg.clone());
        // Tank starts at 40°C (from base_typed_config) with setpoint 52°C:
        // call-for-heat is active and the compressor turns on during step().
        eq.init(&cfg, &env(24.0)).unwrap();
        let mut ports = ports();
        let result = eq.step(&env(24.0), Duration::from_secs(60), &mut ports);
        assert!(result.is_ok(), "step must succeed without NaN panic");

        let cop = eq.telemetry().get(tk::COP).unwrap_or(f64::NAN);
        assert!(
            cop.is_finite() && (0.0..=8.0).contains(&cop),
            "HPWH COP must be in [0.0, 8.0], got {cop}"
        );
        // With an all-zero biquadratic, the raw COP is 0.0; telemetry must
        // report 0.0 (clamped to the physical range) rather than NaN.
        assert!(
            cop < 0.5,
            "COP must be near zero for all-zero curve, got {cop}"
        );

        let electric_kw = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(f64::NAN);
        assert!(
            electric_kw.is_finite(),
            "ELECTRIC_KW must be finite (not NaN/Inf), got {electric_kw}"
        );
        let compressor_kw = eq.telemetry().get(tk::COMPRESSOR_KW).unwrap_or(f64::NAN);
        assert!(
            compressor_kw.is_finite(),
            "COMPRESSOR_KW must be finite (not NaN/Inf), got {compressor_kw}"
        );
    }

    /// For a 12-node tank, condenser heat must be spread across multiple nodes
    /// rather than concentrated at a single node.
    #[test]
    fn condenser_heat_distributed_across_multiple_nodes_for_12_node_tank() {
        let mut typed = base_typed_config();
        typed.initial_tank_temp_c = Some(40.0);
        typed.tank_nodes = Some(12);
        let cfg = equipment_config(typed);

        let mut eq = HeatPumpWH::new(cfg.clone());
        eq.init(&cfg, &env(24.0)).unwrap();

        // Snapshot temperatures before step.
        let temps_before: Vec<f64> = eq.tank.node_temps().to_vec();

        let mut p = ports();
        eq.step(&env(24.0), Duration::from_secs(60), &mut p)
            .unwrap();

        let temps_after = eq.tank.node_temps();
        // Count how many nodes increased in temperature due to condenser heat.
        let nodes_heated = temps_before
            .iter()
            .zip(temps_after.iter())
            .filter(|(before, after)| *after > *before)
            .count();

        // The OCHRE 12-node distribution has non-zero weights for 7 nodes (indices 5–11).
        // After inversion mixing, at least 2 nodes must have been directly heated.
        assert!(
            nodes_heated >= 2,
            "Expected condenser heat distributed to multiple nodes, \
             but only {nodes_heated} nodes increased"
        );
    }

    /// When ambient temperature is below the lockout threshold (7.2°C), the
    /// compressor must be inhibited and mode must be BackupElement (if call for heat).
    #[test]
    fn ambient_lockout_below_minimum_forces_backup_element_mode() {
        let cold_env = env_with_wet_bulb(5.0, 4.0); // 5°C DB, well below 7.2°C lockout

        let mut eq = HeatPumpWH::new(config());
        eq.init(&config(), &cold_env).unwrap();
        // Tank starts at 40°C with setpoint 52°C → call for heat is active.

        let mode = eq.update_control(&cold_env);

        assert!(
            !eq.compressor_on,
            "Compressor must be off when ambient is below lockout"
        );
        // Backup should be on because there is a call for heat and ambient lockout forces ER.
        assert_eq!(
            mode,
            hares_types::OperatingMode::BackupElement,
            "Expected BackupElement mode during ambient lockout, got {mode:?}"
        );
    }

    /// When the tank's upper node exceeds max_tank_temp_c, the equipment must
    /// force Off regardless of call for heat or mode override.
    #[test]
    fn max_tank_temp_safety_forces_off_when_exceeded() {
        let mut typed = base_typed_config();
        typed.max_tank_temp_c = Some(60.0);
        let cfg = equipment_config(typed);

        let mut eq = HeatPumpWH::new(cfg.clone());
        eq.init(&cfg, &env(24.0)).unwrap();

        // Manually set the tank temperature above the max to trigger the safety cutout.
        eq.tank.node_temps_mut().fill(65.0);

        let mode = eq.update_control(&env(24.0));
        assert_eq!(
            mode,
            hares_types::OperatingMode::Off,
            "Expected Off when tank exceeds max_tank_temp_c"
        );
        assert!(!eq.compressor_on);
        assert!(!eq.backup_on);
    }

    /// Composite thermostat (3/4 upper + 1/4 lower) shuts compressor off earlier
    /// than a single lower-node sensor when the upper portion of the tank is hot
    /// but the lower portion is still cold.
    #[test]
    fn composite_thermostat_shuts_off_sooner_than_single_node() {
        use std::time::Duration;

        // Build two HPWHs: one with composite (default), one where we force single-node
        // by making thermostat_upper_node == thermostat_node.
        let mut cfg_composite = base_typed_config();
        cfg_composite.min_on_time_s = Some(0.0);
        let cfg_composite = equipment_config(cfg_composite);

        let mut cfg_single = base_typed_config();
        cfg_single.min_on_time_s = Some(0.0);
        let cfg_single = equipment_config(cfg_single);

        let e = env(24.0);

        let mut eq_composite = HeatPumpWH::new(cfg_composite.clone());
        eq_composite.init(&cfg_composite, &e).unwrap();

        let mut eq_single = HeatPumpWH::new(cfg_single.clone());
        eq_single.init(&cfg_single, &e).unwrap();
        eq_single.thermostat_upper_node = eq_single.thermostat_node;

        // Set tank profile: upper nodes hot (above setpoint), lower nodes cold.
        // With composite: control_temp = 0.75 * hot_upper + 0.25 * cold_lower (> setpoint → off).
        // With single (lower only): reads cold_lower → still calling for heat.
        let setpoint = 52.0_f64;
        let hot = setpoint + 5.0; // 57°C – well above setpoint
        let cold = setpoint - 10.0; // 42°C – well below setpoint - deadband

        for tank in [&mut eq_composite.tank, &mut eq_single.tank] {
            let n = tank.n_nodes();
            // Heat the top half hot, leave bottom half cold.
            let half = n / 2;
            for node in 0..half {
                let delta = hot - tank.node_temps()[node];
                let mcp = 1000.0 * tank.node_volumes_m3()[node] * 4183.0;
                let energy = delta * mcp;
                tank.heat_node(node, energy, Duration::from_secs(1))
                    .unwrap();
            }
            // Force bottom nodes cold by directly setting temperatures.
            for node in half..n {
                let delta = cold - tank.node_temps()[node];
                let mcp = 1000.0 * tank.node_volumes_m3()[node] * 4183.0;
                let energy = delta * mcp;
                if energy.abs() > 0.0 {
                    tank.heat_node(node, energy, Duration::from_secs(1))
                        .unwrap();
                }
            }
        }

        let mode_composite = eq_composite.update_control(&e);
        let mode_single = eq_single.update_control(&e);

        // Composite reads mostly the hot upper node → should be Off (satisfied).
        // Single reads only the cold lower node → should still call for heat.
        assert_eq!(
            mode_composite,
            hares_types::OperatingMode::Off,
            "Composite thermostat should be Off with hot upper, cold lower: got {mode_composite:?}"
        );
        assert_ne!(
            mode_single,
            hares_types::OperatingMode::Off,
            "Single lower-node thermostat should still call for heat when lower node is cold: got {mode_single:?}"
        );
    }

    /// Fan power must appear in total electrical consumption when compressor runs,
    /// and parasitic power when compressor is off.
    /// OCHRE WaterHeater.py:662,671.
    #[test]
    fn fan_and_parasitic_appear_in_electrical() {
        let mut typed = base_typed_config();
        typed.fan_power_w = Some(35.0);
        typed.parasitic_power_w = Some(2.0);
        let cfg = equipment_config(typed);
        let mut eq = HeatPumpWH::new(cfg.clone());
        eq.init(&cfg, &env(24.0)).unwrap();

        // Step with compressor running (tank at 40C, setpoint 52C).
        let mut p = ports();
        eq.step(&env(24.0), Duration::from_secs(60), &mut p)
            .unwrap();

        let comp = eq.telemetry().get(tk::COMPRESSOR_POWER_W).unwrap();
        let backup = eq.telemetry().get(tk::BACKUP_ELEMENT_POWER_W).unwrap();
        assert!(comp > 0.0, "compressor must be running for this test");
        let total_electric_w = p.electrical.net_active_kw() * 1000.0;
        let expected_with_fan = comp + backup + 35.0;
        assert!(
            (total_electric_w - expected_with_fan).abs() < 1e-6,
            "total electric ({total_electric_w} W) must include fan power ({expected_with_fan} W)"
        );
    }

    /// SHR decomposes zone heat extraction into sensible + latent components.
    /// OCHRE WaterHeater.py:672-674.
    #[test]
    fn shr_produces_latent_gain_when_humidity_gap_exists() {
        let mut typed = base_typed_config();
        typed.shr = Some(0.88);
        typed.lost_heat_fraction = Some(0.0);
        let cfg = equipment_config(typed);
        let mut eq = HeatPumpWH::new(cfg.clone());
        eq.init(&cfg, &env(24.0)).unwrap();

        // DB=24, WB=14 => gap=10 > 0.1, so SHR < 1 applies.
        let e = env_with_wet_bulb(24.0, 14.0);
        let mut p = ports();
        eq.step(&e, Duration::from_secs(60), &mut p).unwrap();

        let comp = eq.telemetry().get(tk::COMPRESSOR_POWER_W).unwrap();
        assert!(comp > 0.0, "compressor must be running for SHR test");
        // HP extracts heat from zone (COP > 1), so hp_waste < 0.
        // Latent = hp_waste * (1-SHR) * keep_frac < 0 (dehumidification).
        assert!(
            p.thermal[0].latent_gain_w < 0.0,
            "latent gain should be negative (dehumidification) when COP>1, got {}",
            p.thermal[0].latent_gain_w
        );
    }

    /// Min-on-time prevents compressor from shutting off before the timer expires,
    /// even when the tank has risen above setpoint.
    #[test]
    fn hpwh_min_on_time_prevents_early_shutdown() {
        let mut typed = base_typed_config();
        typed.min_on_time_s = Some(120.0);
        let cfg = equipment_config(typed);

        let e = env(24.0);
        let mut eq = HeatPumpWH::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();

        // Tank starts at 40°C, setpoint 52°C → compressor turns on after first step.
        let mut p = ports();
        eq.step(&e, Duration::from_secs(60), &mut p).unwrap();
        assert!(
            eq.compressor_on,
            "compressor must be on after first step with cold tank"
        );
        // Timer is now Some(60.0) -- 60s elapsed, less than 120s min_on_time.

        // Force the tank above setpoint+deadband by raising the setpoint to a very low value,
        // simulating conditions where call_for_heat would return false.
        // Direct approach: lower the setpoint below the current tank temperature.
        let tank_avg = eq
            .telemetry()
            .get(tk::TANK_AVG_TEMP_C)
            .expect("tank_avg_temp_c telemetry present");
        eq.setpoint_c = tank_avg - 10.0; // setpoint now well below tank → no call for heat

        // update_control with no call_for_heat but timer < min_on_time → compressor stays on.
        let mode = eq.update_control(&e);
        assert!(
            eq.compressor_on,
            "compressor must stay on: min-on-time not yet elapsed (60s < 120s), mode={mode:?}"
        );

        // Advance timer past 120s: one more step adds another 60s → total ≥ 120s elapsed.
        let mut p2 = ports();
        eq.step(&e, Duration::from_secs(60), &mut p2).unwrap();
        // compressor_on_since_s is now Some(120.0); min_on_time is met.
        // update_control should now allow the compressor to shut off.
        let mode_after = eq.update_control(&e);
        assert!(
            !eq.compressor_on,
            "compressor must turn off after min-on-time (120s) has elapsed, mode={mode_after:?}"
        );
    }

    /// compressor_on_since_s increments each step while running, then resets to None when off.
    #[test]
    fn hpwh_min_on_time_timer_lifecycle() {
        let mut typed = base_typed_config();
        // Set min_on_time_s well above test duration so the compressor never self-stops.
        typed.min_on_time_s = Some(9999.0);
        let cfg = equipment_config(typed);

        let e = env(24.0);
        let mut eq = HeatPumpWH::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();

        // Three 60-second steps with cold tank → compressor runs all three.
        for _ in 0..3 {
            let mut p = ports();
            eq.step(&e, Duration::from_secs(60), &mut p).unwrap();
            assert!(
                eq.compressor_on,
                "compressor must remain on during each step"
            );
        }

        let elapsed = eq
            .compressor_on_since_s
            .expect("compressor_on_since_s must be Some while running");
        assert!(
            (elapsed - 180.0).abs() < 1.0,
            "elapsed time should be ~180s after 3×60s steps, got {elapsed}"
        );

        // Turn off the compressor by dropping min_on_time_s to 0 and setting setpoint
        // well below the current tank temp so there is no call for heat.
        eq.min_on_time_s = 0.0;
        eq.setpoint_c = 0.0;
        // Call step: update_control (inside step) sets compressor_on=false, then the
        // timer management block in step clears compressor_on_since_s to None.
        let mut p_off = ports();
        eq.step(&e, Duration::from_secs(60), &mut p_off).unwrap();

        assert!(
            !eq.compressor_on,
            "compressor should be off after setpoint dropped below tank temp"
        );
        assert!(
            eq.compressor_on_since_s.is_none(),
            "compressor_on_since_s must reset to None when compressor turns off"
        );
    }

    /// Safety cutout (max_tank_temp_c exceeded) must override min-on-time and force
    /// the compressor off immediately regardless of how recently it started.
    #[test]
    fn hpwh_safety_override_bypasses_min_on_time() {
        let mut typed = base_typed_config();
        // Long min-on-time so normal logic would keep the compressor running.
        typed.min_on_time_s = Some(600.0);
        let cfg = equipment_config(typed);

        let e = env(24.0);
        let mut eq = HeatPumpWH::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();

        // One step to start the compressor.
        let mut p = ports();
        eq.step(&e, Duration::from_secs(60), &mut p).unwrap();
        assert!(
            eq.compressor_on,
            "compressor must be running before safety test"
        );
        // Timer is Some(60.0) -- well below 600s.
        assert!(
            eq.compressor_on_since_s.is_some_and(|t| t < 600.0),
            "timer must be below min_on_time_s"
        );

        // Trigger the safety cutout: set max_tank_temp below the current upper-node temp.
        // The upper-node defaults to index 0 after init (n_nodes=6, upper=0).
        let upper_temp = eq.tank.node_temps()[eq.thermostat_upper_node];
        eq.max_tank_temp_c = upper_temp - 1.0;

        // update_control must force Off immediately, ignoring min-on-time.
        let mode = eq.update_control(&e);
        assert_eq!(
            mode,
            hares_types::OperatingMode::Off,
            "safety cutout must produce Off mode, got {mode:?}"
        );
        assert!(
            !eq.compressor_on,
            "compressor must be off after safety cutout despite min-on-time not elapsed"
        );
        assert!(!eq.backup_on, "backup must also be off after safety cutout");
    }

    /// Heat delivered to the tank is proportional to cap_mult from the capacity curve.
    /// A unit-constant capacity curve (cap_mult = 1.0 everywhere) must deliver more
    /// heat than a curve that returns cap_mult < 1.0 at the same conditions.
    #[test]
    fn hpwh_capacity_curve_modulates_heat_delivery() {
        let mut low_typed = base_typed_config();
        low_typed.capacity_biquadratic_coeffs = Some([0.8, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let cfg_low = equipment_config(low_typed);

        let mut unit_typed = base_typed_config();
        unit_typed.capacity_biquadratic_coeffs = Some([1.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let cfg_unit = equipment_config(unit_typed);

        let e = env(24.0);

        let mut eq_low = HeatPumpWH::new(cfg_low.clone());
        eq_low.init(&cfg_low, &e).unwrap();
        let mut p_low = ports();
        eq_low
            .step(&e, Duration::from_secs(60), &mut p_low)
            .unwrap();

        let mut eq_unit = HeatPumpWH::new(cfg_unit.clone());
        eq_unit.init(&cfg_unit, &e).unwrap();
        let mut p_unit = ports();
        eq_unit
            .step(&e, Duration::from_secs(60), &mut p_unit)
            .unwrap();

        let cap_low = eq_low
            .telemetry()
            .get(tk::CAP_MULT)
            .expect("cap_mult must be in telemetry");
        let cap_unit = eq_unit
            .telemetry()
            .get(tk::CAP_MULT)
            .expect("cap_mult must be in telemetry");

        assert!(
            (cap_low - 0.8).abs() < 1e-6,
            "low curve must produce cap_mult = 0.8, got {cap_low}"
        );
        assert!(
            (cap_unit - 1.0).abs() < 1e-6,
            "unit curve must produce cap_mult = 1.0, got {cap_unit}"
        );

        let comp_low = eq_low
            .telemetry()
            .get(tk::COMPRESSOR_POWER_W)
            .expect("compressor_power_w in telemetry");
        let comp_unit = eq_unit
            .telemetry()
            .get(tk::COMPRESSOR_POWER_W)
            .expect("compressor_power_w in telemetry");

        assert!(
            comp_low > 0.0 && comp_unit > 0.0,
            "both compressors must be running for this comparison"
        );
        // Electrical draw = (compressor_power_w * cap_mult / cop) * duty.
        // Since both share the same COP curve and conditions, comp_unit / comp_low ≈ 1.0 / 0.8.
        let ratio = comp_unit / comp_low;
        assert!(
            (ratio - 1.0 / 0.8).abs() < 0.05,
            "unit-curve draw should be ~1/0.8 × low-curve draw, ratio={ratio:.4}"
        );
    }

    /// lost_heat_fraction reduces zone gains proportionally.
    /// OCHRE WaterHeater.py:673: sensible *= (1 - lost_heat_fraction).
    #[test]
    fn lost_heat_fraction_reduces_zone_gains() {
        let e = env_with_wet_bulb(24.0, 14.0);

        let mut typed0 = base_typed_config();
        typed0.lost_heat_fraction = Some(0.0);
        let cfg0 = equipment_config(typed0);
        let mut eq0 = HeatPumpWH::new(cfg0.clone());
        eq0.init(&cfg0, &e).unwrap();
        let mut p0 = ports();
        eq0.step(&e, Duration::from_secs(60), &mut p0).unwrap();

        let mut typed50 = base_typed_config();
        typed50.lost_heat_fraction = Some(0.5);
        let cfg50 = equipment_config(typed50);
        let mut eq50 = HeatPumpWH::new(cfg50.clone());
        eq50.init(&cfg50, &e).unwrap();
        let mut p50 = ports();
        eq50.step(&e, Duration::from_secs(60), &mut p50).unwrap();

        let sens0 = p0.thermal[0].sensible_gain_w;
        let sens50 = p50.thermal[0].sensible_gain_w;
        assert!(
            sens0.abs() > 1e-6,
            "baseline sensible gain must be non-zero for this test"
        );
        let ratio = sens50 / sens0;
        assert!(
            (ratio - 0.5).abs() < 0.05,
            "lost_heat_fraction=0.5 should halve sensible gain: ratio={ratio:.3}"
        );
    }

    #[test]
    fn lost_heat_fraction_0_75_reduces_zone_gain_to_25_percent() {
        let e = env_with_wet_bulb(24.0, 14.0);

        let mut typed0 = base_typed_config();
        typed0.lost_heat_fraction = Some(0.0);
        let cfg0 = equipment_config(typed0);
        let mut eq0 = HeatPumpWH::new(cfg0.clone());
        eq0.init(&cfg0, &e).unwrap();
        let mut p0 = ports();
        eq0.step(&e, Duration::from_secs(60), &mut p0).unwrap();

        let mut typed75 = base_typed_config();
        typed75.lost_heat_fraction = Some(0.75);
        let cfg75 = equipment_config(typed75);
        let mut eq75 = HeatPumpWH::new(cfg75.clone());
        eq75.init(&cfg75, &e).unwrap();
        let mut p75 = ports();
        eq75.step(&e, Duration::from_secs(60), &mut p75).unwrap();

        let sens0 = p0.thermal[0].sensible_gain_w;
        let sens75 = p75.thermal[0].sensible_gain_w;
        assert!(
            sens0.abs() > 1e-6,
            "baseline sensible gain must be non-zero for this test"
        );
        let skin_loss_w = eq0.telemetry().get(tk::SKIN_LOSS_W).unwrap_or(0.0);
        let hp_waste0 = sens0 - skin_loss_w;
        let hp_waste75 = sens75 - skin_loss_w;
        assert!(
            hp_waste0.abs() > 1e-6,
            "HP waste component must be non-zero for this test"
        );
        let ratio = hp_waste75 / hp_waste0;
        assert!(
            (ratio - 0.25).abs() < 0.05,
            "lost_heat_fraction=0.75 should leave 25% of HP waste heat in zone: ratio={ratio:.3}"
        );
    }

    #[test]
    fn lost_heat_fraction_default_none_equals_zero() {
        let e = env_with_wet_bulb(24.0, 14.0);

        let mut typed_explicit = base_typed_config();
        typed_explicit.lost_heat_fraction = Some(0.0);
        let cfg_explicit = equipment_config(typed_explicit);
        let mut eq_explicit = HeatPumpWH::new(cfg_explicit.clone());
        eq_explicit.init(&cfg_explicit, &e).unwrap();
        let mut p_explicit = ports();
        eq_explicit
            .step(&e, Duration::from_secs(60), &mut p_explicit)
            .unwrap();

        let mut typed_default = base_typed_config();
        typed_default.lost_heat_fraction = None;
        let cfg_default = equipment_config(typed_default);
        let mut eq_default = HeatPumpWH::new(cfg_default.clone());
        eq_default.init(&cfg_default, &e).unwrap();
        let mut p_default = ports();
        eq_default
            .step(&e, Duration::from_secs(60), &mut p_default)
            .unwrap();

        let sens_explicit = p_explicit.thermal[0].sensible_gain_w;
        let sens_default = p_default.thermal[0].sensible_gain_w;
        assert!(
            (sens_explicit - sens_default).abs() < 1e-6,
            "omitting lost_heat_fraction must produce same result as lost_heat_fraction=0.0: \
             explicit={sens_explicit:.6}, default={sens_default:.6}"
        );
    }

    #[test]
    fn backup_efficiency_scales_backup_power_delivery() {
        // Force backup-only mode so the backup element fires on the first step.
        // With backup_efficiency=0.8, delivered heat = 0.8 * electrical input,
        // so the tank heats less per step than at efficiency=1.0.
        let make_cfg = |eff: f64| {
            let mut typed = base_typed_config();
            typed.initial_tank_temp_c = Some(20.0);
            typed.backup_enable_offset_c = Some(8.0);
            typed.backup_efficiency = Some(eff);
            // Use Simultaneous mode so backup fires alongside compressor
            // when control temp is below backup threshold.
            typed.element_hp_control_mode = Some("Simultaneous".into());
            equipment_config(typed)
        };

        let e = env(24.0);

        let cfg_100 = make_cfg(1.0);
        let mut eq_100 = HeatPumpWH::new(cfg_100.clone());
        eq_100.init(&cfg_100, &e).unwrap();
        let mut p_100 = ports();
        eq_100
            .step(&e, Duration::from_secs(60), &mut p_100)
            .unwrap();
        let backup_100 = eq_100.telemetry().get(tk::BACKUP_ELEMENT_POWER_W).unwrap();
        assert!(
            backup_100 > 0.0,
            "backup must fire for this test: got {backup_100}"
        );
        let tank_100 = eq_100.telemetry().get(tk::TANK_AVG_TEMP_C).unwrap();

        let cfg_80 = make_cfg(0.8);
        let mut eq_80 = HeatPumpWH::new(cfg_80.clone());
        eq_80.init(&cfg_80, &e).unwrap();
        let mut p_80 = ports();
        eq_80.step(&e, Duration::from_secs(60), &mut p_80).unwrap();
        let backup_80 = eq_80.telemetry().get(tk::BACKUP_ELEMENT_POWER_W).unwrap();
        assert!(
            backup_80 > 0.0,
            "backup must fire for this test: got {backup_80}"
        );
        let tank_80 = eq_80.telemetry().get(tk::TANK_AVG_TEMP_C).unwrap();

        // Both draw the same backup electrical power, but the 80% efficient
        // unit delivers less heat to the tank.
        assert!(
            tank_80 < tank_100,
            "backup_efficiency=0.8 ({tank_80:.4} C) must deliver less heat than 1.0 ({tank_100:.4} C)"
        );

        // Electrical consumption should be the same (same backup element draw).
        let elec_100 = p_100.electrical.load_power_kw;
        let elec_80 = p_80.electrical.load_power_kw;
        assert!(
            (elec_100 - elec_80).abs() < 0.01,
            "electrical consumption should be the same: {elec_100:.4} vs {elec_80:.4}"
        );
    }
}

#[cfg(test)]
mod mutual_exclusion_tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        ControlSignal, EnvironmentState, GridState, OperatingMode, PortSlots, ThermalAccumulator,
        WeatherState, ZoneId, ZoneState,
    };

    use super::{ElementHpControlMode, HeatPumpWH};
    use crate::water_heater::heat_pump_wh::tests::{base_typed_config, equipment_config};
    use crate::{Equipment, EquipmentConfig};

    fn env_state() -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: 24.0,
                humidity_ratio: 0.008,
                relative_humidity: 0.45,
                wet_bulb_c: 14.0,
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

    /// Config: tank at 40°C (below setpoint 52°C, so call for heat active),
    /// backup_enable_offset is large so backup only triggers well below setpoint.
    fn base_config(mode: &str) -> EquipmentConfig {
        let mut typed = base_typed_config();
        typed.initial_tank_temp_c = Some(40.0);
        typed.backup_enable_offset_c = Some(20.0);
        typed.element_hp_control_mode = (!mode.is_empty()).then(|| mode.to_string());
        equipment_config(typed)
    }

    /// Config where tank is very cold so backup threshold is also triggered.
    fn very_cold_config(mode: &str) -> EquipmentConfig {
        let mut typed = base_typed_config();
        // Tank starts so cold that control_temp <= setpoint - backup_enable_offset.
        typed.initial_tank_temp_c = Some(20.0);
        typed.backup_enable_offset_c = Some(8.0);
        typed.element_hp_control_mode = (!mode.is_empty()).then(|| mode.to_string());
        equipment_config(typed)
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

    /// MutuallyExclusive: once compressor is running, backup element cannot start.
    #[test]
    fn mutually_exclusive_compressor_running_prevents_backup() {
        let cfg = very_cold_config("MutuallyExclusive");
        let mut wh = HeatPumpWH::new(cfg.clone());
        wh.init(&cfg, &env_state()).unwrap();

        // Force compressor_on = true before calling update_control.
        wh.compressor_on = true;
        wh.backup_on = false;

        let mode = wh.update_control(&env_state());
        assert!(wh.compressor_on, "compressor must stay on");
        assert!(
            !wh.backup_on,
            "backup must be locked out while compressor is running"
        );
        assert_eq!(mode, OperatingMode::HeatPumpWH);
    }

    /// MutuallyExclusive: once backup element is running, compressor cannot start.
    #[test]
    fn mutually_exclusive_backup_running_prevents_compressor() {
        let cfg = very_cold_config("MutuallyExclusive");
        let mut wh = HeatPumpWH::new(cfg.clone());
        wh.init(&cfg, &env_state()).unwrap();

        // Force backup_on = true, compressor_on = false before calling update_control.
        wh.compressor_on = false;
        wh.backup_on = true;

        let mode = wh.update_control(&env_state());
        assert!(
            !wh.compressor_on,
            "compressor must be locked out while backup is running"
        );
        assert!(wh.backup_on, "backup element must continue running");
        assert_eq!(mode, OperatingMode::BackupElement);
    }

    /// MutuallyExclusive: when neither is running, compressor gets priority over backup.
    #[test]
    fn mutually_exclusive_neither_running_compressor_gets_priority() {
        let cfg = very_cold_config("MutuallyExclusive");
        let mut wh = HeatPumpWH::new(cfg.clone());
        wh.init(&cfg, &env_state()).unwrap();

        wh.compressor_on = false;
        wh.backup_on = false;

        let mode = wh.update_control(&env_state());
        // Compressor should start; backup stays off even though tank is very cold.
        assert!(
            wh.compressor_on,
            "compressor should start when neither is running"
        );
        assert!(
            !wh.backup_on,
            "backup must not start (compressor has priority)"
        );
        assert_eq!(mode, OperatingMode::HeatPumpWH);
    }

    /// Simultaneous: both compressor and backup can run when conditions are met.
    #[test]
    fn simultaneous_both_can_run_when_conditions_met() {
        let cfg = very_cold_config("Simultaneous");
        let mut wh = HeatPumpWH::new(cfg.clone());
        wh.init(&cfg, &env_state()).unwrap();

        wh.compressor_on = false;
        wh.backup_on = false;

        let mode = wh.update_control(&env_state());
        assert!(wh.compressor_on, "compressor must run in Simultaneous mode");
        assert!(
            wh.backup_on,
            "backup must also run when temp is below backup threshold in Simultaneous mode"
        );
        assert_eq!(mode, OperatingMode::HeatingHPAndER);
    }

    /// ModeOverride::HeatingHPAndER forces both on, overriding mutual exclusion.
    #[test]
    fn mode_override_heating_hp_and_er_overrides_mutual_exclusion() {
        let cfg = base_config("MutuallyExclusive");
        let mut wh = HeatPumpWH::new(cfg.clone());
        wh.init(&cfg, &env_state()).unwrap();
        wh.apply_control(&ControlSignal::ModeOverride {
            mode: OperatingMode::HeatingHPAndER,
        })
        .unwrap();

        let mode = wh.update_control(&env_state());
        assert!(
            wh.compressor_on,
            "compressor must be on with HeatingHPAndER override"
        );
        assert!(
            wh.backup_on,
            "backup must be on with HeatingHPAndER override"
        );
        assert_eq!(mode, OperatingMode::HeatingHPAndER);
    }

    /// ModeOverride::HeatingHP forces compressor only, backup stays off.
    #[test]
    fn mode_override_heating_hp_respects_hp_only_command() {
        let cfg = very_cold_config("MutuallyExclusive");
        let mut wh = HeatPumpWH::new(cfg.clone());
        wh.init(&cfg, &env_state()).unwrap();
        wh.apply_control(&ControlSignal::ModeOverride {
            mode: OperatingMode::HeatingHP,
        })
        .unwrap();

        let mode = wh.update_control(&env_state());
        assert!(
            wh.compressor_on,
            "compressor must be on with HeatingHP override"
        );
        assert!(!wh.backup_on, "backup must be off with HeatingHP override");
        assert_eq!(mode, OperatingMode::HeatPumpWH);
    }

    /// Verify state round-trip preserves the element_hp_control field.
    #[test]
    fn state_round_trip_preserves_element_hp_control() {
        for (mode_str, expected_mode) in [
            ("MutuallyExclusive", ElementHpControlMode::MutuallyExclusive),
            ("Simultaneous", ElementHpControlMode::Simultaneous),
        ] {
            let cfg = base_config(mode_str);
            let mut wh = HeatPumpWH::new(cfg.clone());
            wh.init(&cfg, &env_state()).unwrap();

            let mut p = ports();
            wh.step(&env_state(), Duration::from_secs(60), &mut p)
                .unwrap();
            let saved = wh.save_state();

            let mut restored = HeatPumpWH::new(cfg.clone());
            restored.init(&cfg, &env_state()).unwrap();
            restored.load_state(&saved).unwrap();

            assert_eq!(
                restored.element_hp_control, expected_mode,
                "element_hp_control must survive save/load round-trip ({mode_str})"
            );
        }
    }

    #[test]
    fn state_round_trip_preserves_dr_state() {
        use hares_types::{ControlSignal, DRLevel};

        let cfg = base_config("");
        let mut wh = HeatPumpWH::new(cfg.clone());
        wh.init(&cfg, &env_state()).unwrap();

        wh.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::Critical,
            duration_s: Some(300.0),
        })
        .unwrap();

        let saved = wh.save_state();

        let mut restored = HeatPumpWH::new(cfg.clone());
        restored.init(&cfg, &env_state()).unwrap();
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
}

#[cfg(test)]
mod dr_tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        ControlSignal, DRLevel, EnvironmentState, GridState, OperatingMode, PortSlots,
        ThermalAccumulator, WeatherState, ZoneId, ZoneState, telemetry_keys as tk,
    };

    use super::HeatPumpWH;
    use crate::water_heater::heat_pump_wh::tests::{base_typed_config, equipment_config};
    use crate::{Equipment, EquipmentConfig};

    fn env_state() -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: 24.0,
                humidity_ratio: 0.008,
                relative_humidity: 0.45,
                wet_bulb_c: 14.0,
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

    /// Config: tank at 50°C (just below setpoint 52°C, within deadband → calling for heat).
    /// max_tank_temp_c is high so safety never fires.
    fn config_near_setpoint() -> EquipmentConfig {
        let mut typed = base_typed_config();
        typed.initial_tank_temp_c = Some(49.0); // 49 < 52 - 2 = 50 → calls for heat from Off
        typed.backup_enable_offset_c = Some(20.0);
        typed.min_on_time_s = Some(0.0);
        equipment_config(typed)
    }

    /// Config: tank at 40°C (well below setpoint 52°C → calling for heat even with DR offsets).
    fn config_cold_tank() -> EquipmentConfig {
        let mut typed = base_typed_config();
        typed.initial_tank_temp_c = Some(40.0);
        typed.backup_enable_offset_c = Some(20.0);
        typed.min_on_time_s = Some(0.0);
        equipment_config(typed)
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

    /// DR Moderate applies a -3°C setpoint offset. A tank at 49°C that would normally
    /// call for heat (setpoint=52°C, 49 < 50 deadband floor) must stop calling after the
    /// offset (effective setpoint=49°C; 49 is not below 49 - 2 = 47).
    #[test]
    fn hpwh_dr_moderate_reduces_setpoint() {
        let cfg = config_near_setpoint();
        let e = env_state();

        // Baseline: tank at 49°C < setpoint - deadband = 50°C → heating from Off.
        let mut eq_base = HeatPumpWH::new(cfg.clone());
        eq_base.init(&cfg, &e).unwrap();
        let mode_base = eq_base.update_control(&e);
        assert!(
            mode_base != OperatingMode::Off,
            "baseline must be heating with tank at 49°C; got {mode_base:?}"
        );

        // DR Moderate: effective setpoint = 49°C, deadband floor = 47°C → no call.
        let mut eq_dr = HeatPumpWH::new(cfg.clone());
        eq_dr.init(&cfg, &e).unwrap();
        eq_dr
            .apply_control(&ControlSignal::DemandResponse {
                level: DRLevel::Moderate,
                duration_s: None,
            })
            .unwrap();
        assert_eq!(
            eq_dr.dr_setpoint_offset_c, -3.0,
            "Moderate DR must set offset to -3°C"
        );
        let mode_dr = eq_dr.update_control(&e);
        assert_eq!(
            mode_dr,
            OperatingMode::Off,
            "DR Moderate must suppress heating by reducing effective setpoint; got {mode_dr:?}"
        );
    }

    /// DR GridEmergency sets dr_load_fraction=0.0, which forces the HPWH fully off.
    #[test]
    fn hpwh_dr_grid_emergency_forces_off() {
        let cfg = config_cold_tank();
        let e = env_state();

        let mut eq = HeatPumpWH::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();

        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::GridEmergency,
            duration_s: None,
        })
        .unwrap();
        assert_eq!(
            eq.dr_load_fraction, 0.0,
            "GridEmergency must set dr_load_fraction to 0"
        );

        let mut p = ports();
        eq.step(&e, Duration::from_secs(60), &mut p).unwrap();

        assert_eq!(
            eq.telemetry().get(tk::COMPRESSOR_POWER_W).unwrap_or(1.0),
            0.0,
            "compressor must be off during GridEmergency"
        );
        assert_eq!(
            eq.telemetry()
                .get(tk::BACKUP_ELEMENT_POWER_W)
                .unwrap_or(1.0),
            0.0,
            "backup element must be off during GridEmergency"
        );
        let total_kw = p.electrical.net_active_kw();
        assert!(
            !eq.compressor_on,
            "compressor_on must be false during GridEmergency"
        );
        assert!(
            !eq.backup_on,
            "backup_on must be false during GridEmergency"
        );
        // Compressor and backup element must contribute zero power.
        let comp_kw = eq.telemetry().get(tk::COMPRESSOR_POWER_W).unwrap_or(1.0) / 1_000.0;
        let backup_kw = eq
            .telemetry()
            .get(tk::BACKUP_ELEMENT_POWER_W)
            .unwrap_or(1.0)
            / 1_000.0;
        assert!(
            comp_kw < 1e-9,
            "compressor power must be zero during GridEmergency, got {comp_kw} kW"
        );
        assert!(
            backup_kw < 1e-9,
            "backup element power must be zero during GridEmergency, got {backup_kw} kW"
        );
        // When compressor is off, parasitic standby power (DEFAULT_PARASITIC_POWER_W = 1 W)
        // still draws. Total should be at most that parasitic amount.
        assert!(
            total_kw <= 0.002,
            "total electrical draw must be at most parasitic standby during GridEmergency, got {total_kw} kW"
        );
    }

    /// DR Critical applies load_fraction=0.5. When the HPWH is in MutuallyExclusive mode
    /// and the compressor is running, backup cannot co-fire (mutual exclusion),
    /// and compressor power is scaled by the load fraction.
    #[test]
    fn hpwh_dr_critical_with_mutual_exclusion_interaction() {
        let mut typed = base_typed_config();
        // Tank at 30°C: calls for heat under both normal and DR Critical (effective sp=42, floor=40).
        typed.initial_tank_temp_c = Some(30.0);
        // Large offset: backup fires only when control_temp <= 22°C; tank at 30°C → compressor only.
        typed.backup_enable_offset_c = Some(30.0);
        typed.min_on_time_s = Some(0.0);
        // Explicit MutuallyExclusive (default, but stated for clarity).
        let cfg = equipment_config(typed);

        let e = env_state();

        // Baseline: full compressor power, no DR.
        let mut eq_base = HeatPumpWH::new(cfg.clone());
        eq_base.init(&cfg, &e).unwrap();
        let mut p_base = ports();
        eq_base
            .step(&e, Duration::from_secs(60), &mut p_base)
            .unwrap();
        let comp_w_base = eq_base
            .telemetry()
            .get(tk::COMPRESSOR_POWER_W)
            .unwrap_or(0.0);
        assert!(comp_w_base > 0.0, "baseline compressor must be running");

        // DR Critical: load_fraction=0.5 → compressor power halved.
        let mut eq_dr = HeatPumpWH::new(cfg.clone());
        eq_dr.init(&cfg, &e).unwrap();
        eq_dr
            .apply_control(&ControlSignal::DemandResponse {
                level: DRLevel::Critical,
                duration_s: None,
            })
            .unwrap();
        assert_eq!(
            eq_dr.dr_load_fraction, 0.5,
            "Critical DR must set load fraction to 0.5"
        );

        let mut p_dr = ports();
        eq_dr.step(&e, Duration::from_secs(60), &mut p_dr).unwrap();

        let comp_w_dr = eq_dr.telemetry().get(tk::COMPRESSOR_POWER_W).unwrap_or(0.0);
        let backup_w_dr = eq_dr
            .telemetry()
            .get(tk::BACKUP_ELEMENT_POWER_W)
            .unwrap_or(0.0);

        // Compressor power should be ~50% of baseline.
        let ratio = comp_w_dr / comp_w_base;
        assert!(
            (ratio - 0.5).abs() < 0.05,
            "DR Critical must halve compressor power; ratio={ratio:.3}"
        );

        // Mutual exclusion must still hold: backup cannot fire while compressor runs.
        assert_eq!(
            backup_w_dr, 0.0,
            "backup element must stay off (MutuallyExclusive) even under DR Critical"
        );
        assert!(
            !eq_dr.backup_on,
            "backup_on must be false (MutuallyExclusive + compressor running)"
        );
    }

    /// Safety cutout checks max of ALL nodes, not just a single thermostat node.
    /// Any node exceeding max_tank_temp_c must trigger the cutout regardless of
    /// thermostat_upper_node configuration.
    #[test]
    fn safety_cutout_fires_when_any_node_exceeds_limit() {
        let mut typed = base_typed_config();
        typed.initial_tank_temp_c = Some(20.0);
        typed.max_tank_temp_c = Some(60.0);
        typed.min_on_time_s = Some(0.0);
        let cfg = equipment_config(typed);

        let e = env_state();

        let mut eq = HeatPumpWH::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();
        // Set node 1 above max_tank_temp_c (60°C); node 0 (thermostat) stays cold.
        eq.tank.node_temps_mut()[1] = 70.0;
        let mode = eq.update_control(&e);
        assert_eq!(
            mode,
            OperatingMode::Off,
            "Safety must fire when ANY node exceeds max_tank_temp_c, even if thermostat node is cool"
        );
    }

    /// All nodes below max_tank_temp_c -- safety cutout must NOT fire.
    #[test]
    fn safety_cutout_does_not_fire_when_all_nodes_below_limit() {
        let mut typed = base_typed_config();
        typed.initial_tank_temp_c = Some(30.0);
        typed.max_tank_temp_c = Some(55.0);
        typed.min_on_time_s = Some(0.0);
        typed.backup_enable_offset_c = Some(30.0);
        let cfg = equipment_config(typed);

        let e = env_state();

        let mut eq = HeatPumpWH::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();
        // Hottest node below limit; tank cold so compressor should run.
        eq.tank.node_temps_mut()[0] = 54.0;

        let mode = eq.update_control(&e);
        assert_ne!(
            mode,
            OperatingMode::Off,
            "Compressor must be allowed to run when all nodes are below max_tank_temp_c"
        );
    }
}

#[cfg(test)]
mod new_feature_tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        ControlSignal, DutyCycleComponent, EnvironmentState, GridState, OperatingMode, PortSlots,
        ThermalAccumulator, ThermalCategory, WeatherState, ZoneId, ZoneState, telemetry_keys as tk,
    };

    use super::HeatPumpWH;
    use crate::water_heater::heat_pump_wh::tests::{base_typed_config, equipment_config};
    use crate::{Equipment, EquipmentConfig, HeatPumpWaterHeaterConfig};

    fn env_at(zone_temp_c: f64) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp_c,
                humidity_ratio: 0.008,
                relative_humidity: 0.45,
                wet_bulb_c: zone_temp_c - 5.0,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: zone_temp_c,
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

    fn base_config() -> EquipmentConfig {
        let mut typed = base_typed_config();
        typed.backup_enable_offset_c = Some(8.0);
        typed.min_on_time_s = Some(0.0);
        equipment_config(typed)
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

    // --- Change 2: explicit lockout bounds ---

    #[test]
    fn standard_hpwh_lockout_bounds_are_ochre_values() {
        let cfg = base_config();
        let mut eq = HeatPumpWH::new(cfg.clone());
        eq.init(&cfg, &env_at(24.0)).unwrap();
        assert!(
            (eq.min_ambient_temp_c - 7.222).abs() < 1e-3,
            "standard min lockout should be 7.222, got {}",
            eq.min_ambient_temp_c
        );
        assert!(
            (eq.max_ambient_temp_c - 43.333).abs() < 1e-3,
            "standard max lockout should be 43.333, got {}",
            eq.max_ambient_temp_c
        );
    }

    #[test]
    fn explicit_lockout_bounds_are_respected() {
        let mut typed = base_typed_config();
        typed.min_ambient_temp_c = Some(2.778);
        typed.max_ambient_temp_c = Some(62.778);
        let cfg = equipment_config(typed);
        let mut eq = HeatPumpWH::new(cfg.clone());
        eq.init(&cfg, &env_at(24.0)).unwrap();
        assert!(
            (eq.min_ambient_temp_c - 2.778).abs() < 1e-3,
            "configured min lockout should be 2.778, got {}",
            eq.min_ambient_temp_c
        );
        assert!(
            (eq.max_ambient_temp_c - 62.778).abs() < 1e-3,
            "configured max lockout should be 62.778, got {}",
            eq.max_ambient_temp_c
        );
    }

    #[test]
    fn explicit_lockout_config_overrides_default_bounds() {
        let mut typed = base_typed_config();
        typed.min_ambient_temp_c = Some(1.0);
        let cfg = equipment_config(typed);
        let mut eq = HeatPumpWH::new(cfg.clone());
        eq.init(&cfg, &env_at(24.0)).unwrap();
        assert!(
            (eq.min_ambient_temp_c - 1.0).abs() < 1e-9,
            "explicit min_ambient_temp_c=1.0 must override default"
        );
    }

    #[test]
    fn unset_lost_heat_fraction_defaults_to_zero_regardless_of_zone_type() {
        let typed = HeatPumpWaterHeaterConfig {
            equipment_id: None,
            zone_id: Some(1),
            loop_id: Some(1),
            tank_volume_m3: None,
            tank_height_m: None,
            cop: Some(2.5),
            backup_element_power_w: Some(4500.0),
            ua_w_per_k: None,
            setpoint_c: Some(52.0),
            deadband_c: Some(2.0),
            max_tank_temp_c: Some(300.0),
            initial_tank_temp_c: Some(40.0),
            tank_nodes: None,
            tempering_valve_setpoint_c: None,
            avg_water_draw_l_per_day: None,
            draw_flow_rate_kg_s: Some(0.0),
            compressor_power_w: Some(1200.0),
            backup_enable_offset_c: Some(3.0),
            min_ambient_temp_c: None,
            max_ambient_temp_c: None,
            min_on_time_s: Some(0.0),
            min_off_time_s: Some(0.0),
            hp_only_mode: Some(false),
            element_hp_control_mode: None,
            fan_power_w: Some(35.0),
            parasitic_power_w: Some(1.0),
            backup_efficiency: Some(1.0),
            shr: Some(0.88),
            lost_heat_fraction: None,
            wall_heat_fraction: None,
            capacity_biquadratic_coeffs: None,
            cop_biquadratic_coeffs: None,
            performance_adjustment: Some(1.0),
            zone_type: Some("conditioned".to_string()),
            first_hour_rating_m3: None,
            jacket_r_value_m2_k_w: None,
            fixture_delivery_temp_c: None,
        };
        let cfg = equipment_config(typed);
        let mut eq = HeatPumpWH::new(cfg.clone());
        eq.init(&cfg, &env_at(24.0)).unwrap();
        assert!(
            eq.lost_heat_fraction.abs() < 1e-12,
            "unset lost_heat_fraction must default to 0.0, got {}",
            eq.lost_heat_fraction
        );
        assert!(
            (eq.wall_heat_fraction - 0.5).abs() < 1e-12,
            "conditioned-zone default wall_heat_fraction should be 0.5, got {}",
            eq.wall_heat_fraction
        );
    }

    // --- Change 3: wall_heat_fraction ---

    #[test]
    fn wall_heat_fraction_preserves_energy_balance_and_tracks_split() {
        let e = env_at(24.0);

        let cfg0 = base_config();
        let mut eq0 = HeatPumpWH::new(cfg0.clone());
        eq0.init(&cfg0, &e).unwrap();
        let mut p0 = ports();
        eq0.step(&e, Duration::from_secs(60), &mut p0).unwrap();
        let sens0 = p0.thermal[0].sensible_gain_w;

        let mut typed50 = base_typed_config();
        typed50.wall_heat_fraction = Some(0.5);
        let cfg50 = equipment_config(typed50);
        let mut eq50 = HeatPumpWH::new(cfg50.clone());
        eq50.init(&cfg50, &e).unwrap();
        let mut p50 = ports();
        eq50.step(&e, Duration::from_secs(60), &mut p50).unwrap();
        let sens50 = p50.thermal[0].sensible_gain_w;
        let internal50 = p50.thermal[0].sensible_for_category(ThermalCategory::InternalGain);
        let dehumid50 = p50.thermal[0].sensible_for_category(ThermalCategory::HvacDehumidification);
        let jacket50 = p50.thermal[0].sensible_for_category(ThermalCategory::JacketLoss);

        assert!(
            sens0.abs() > 1e-6,
            "baseline sensible gain must be non-zero"
        );
        let ratio = sens50 / sens0;
        assert!(
            (ratio - 1.0).abs() < 0.01,
            "total sensible gain must be preserved across the wall split: ratio={ratio:.4}"
        );
        // InternalGain must be zero: HPWH compressor zone heat is
        // HvacDehumidification, not a passive internal gain.
        assert!(
            internal50.abs() < 1e-6,
            "InternalGain must be zero when HP is running (zone heat is HvacDehumidification): {internal50:.2} W"
        );
        let skin_loss_w = eq50.telemetry().get(tk::SKIN_LOSS_W).unwrap_or(0.0);
        let wall_w = eq50
            .telemetry()
            .get(tk::WALL_SENSIBLE_GAIN_W)
            .unwrap_or(0.0);
        assert!(
            (dehumid50 - wall_w).abs() < 1.0,
            "HvacDehumidification must equal wall share of HP waste heat (zone-half at wf=0.5): {dehumid50:.2} vs {wall_w:.2}"
        );
        assert!(
            (jacket50 - wall_w - skin_loss_w).abs() < 1.0,
            "jacket loss must equal wall share plus skin loss: {jacket50:.2} vs wall={wall_w:.2} + skin={skin_loss_w:.2}"
        );
        assert!(
            (wall_w.abs() - (sens0 - skin_loss_w).abs() * 0.5).abs() < 1.0,
            "wall_sensible_gain_w telemetry must be half of HP waste heat: {wall_w:.2} vs {:.2}",
            (sens0 - skin_loss_w).abs() * 0.5
        );
    }

    // --- Change 4: hp_only_mode ---

    #[test]
    fn hp_only_mode_disables_backup_element() {
        let mut typed = base_typed_config();
        typed.hp_only_mode = Some(true);
        typed.initial_tank_temp_c = Some(20.0);
        typed.backup_enable_offset_c = Some(5.0);
        typed.element_hp_control_mode = Some("Simultaneous".into());
        let cfg = equipment_config(typed);
        let mut eq = HeatPumpWH::new(cfg.clone());
        eq.init(&cfg, &env_at(24.0)).unwrap();
        for _ in 0..5 {
            let mut p = ports();
            eq.step(&env_at(24.0), Duration::from_secs(60), &mut p)
                .unwrap();
            assert!(
                !eq.backup_on,
                "backup element must remain off in hp_only_mode"
            );
        }
    }

    #[test]
    fn without_hp_only_mode_backup_fires_when_tank_cold() {
        let mut typed = base_typed_config();
        typed.initial_tank_temp_c = Some(20.0);
        typed.backup_enable_offset_c = Some(5.0);
        typed.element_hp_control_mode = Some("Simultaneous".into());
        let cfg = equipment_config(typed);
        let mut eq = HeatPumpWH::new(cfg.clone());
        eq.init(&cfg, &env_at(24.0)).unwrap();
        let mode = eq.update_control(&env_at(24.0));
        assert_eq!(
            mode,
            OperatingMode::HeatingHPAndER,
            "Simultaneous + cold tank + no hp_only_mode must produce HeatingHPAndER"
        );
    }

    // --- Change 5: min_off_time_s ---

    #[test]
    fn min_off_time_prevents_early_compressor_restart() {
        let mut typed = base_typed_config();
        typed.min_off_time_s = Some(120.0);
        let cfg = equipment_config(typed);
        let e = env_at(24.0);
        let mut eq = HeatPumpWH::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();

        let mut p = ports();
        eq.step(&e, Duration::from_secs(60), &mut p).unwrap();
        assert!(eq.compressor_on, "compressor should be running initially");

        eq.setpoint_c = eq.telemetry().get(tk::TANK_AVG_TEMP_C).unwrap_or(40.0) - 20.0;
        let mut p2 = ports();
        eq.step(&e, Duration::from_secs(60), &mut p2).unwrap();
        assert!(!eq.compressor_on, "compressor should now be off");
        assert!(
            eq.compressor_off_since_s.is_some(),
            "compressor_off_since_s must start counting after shutdown"
        );

        eq.setpoint_c = 80.0;
        let mode = eq.update_control(&e);
        assert!(
            !eq.compressor_on,
            "compressor must not restart before min_off_time_s (off_since<120s), mode={mode:?}"
        );

        if let Some(t) = eq.compressor_off_since_s.as_mut() {
            *t = 121.0;
        }
        let mode_after = eq.update_control(&e);
        assert!(
            eq.compressor_on,
            "compressor must restart once min_off_time_s elapsed, mode={mode_after:?}"
        );
    }

    #[test]
    fn compressor_off_timer_resets_when_compressor_starts() {
        let cfg = base_config();
        let e = env_at(24.0);
        let mut eq = HeatPumpWH::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();

        eq.compressor_off_since_s = Some(200.0);
        eq.compressor_on = false;

        let mut p = ports();
        eq.step(&e, Duration::from_secs(60), &mut p).unwrap();

        assert!(
            eq.compressor_on,
            "compressor must be on after step with call for heat"
        );
        assert!(
            eq.compressor_off_since_s.is_none(),
            "compressor_off_since_s must be None while compressor is running"
        );
    }

    // ── Split duty cycle tests ─────────────────────────────────────

    #[test]
    fn split_duty_cycle_curtails_compressor_independently() {
        let mut typed = base_typed_config();
        typed.setpoint_c = Some(50.0);
        let cfg = equipment_config(typed);
        let mut eq = HeatPumpWH::new(cfg.clone());
        eq.init(&cfg, &env_at(20.0)).expect("init");

        eq.apply_control(&ControlSignal::DutyCycle {
            on_fraction: 0.5,
            period_s: None,
            component: Some(DutyCycleComponent::Compressor),
        })
        .expect("compressor duty cycle accepted");

        assert!(
            (eq.hp_duty_cycle - 0.5).abs() < 1e-10,
            "hp_duty_cycle should be 0.5, got {}",
            eq.hp_duty_cycle
        );
        assert!(
            (eq.er_duty_cycle - 1.0).abs() < 1e-10,
            "er_duty_cycle should remain 1.0, got {}",
            eq.er_duty_cycle
        );
    }

    #[test]
    fn split_duty_cycle_curtails_backup_independently() {
        let mut typed = base_typed_config();
        typed.setpoint_c = Some(50.0);
        let cfg = equipment_config(typed);
        let mut eq = HeatPumpWH::new(cfg.clone());
        eq.init(&cfg, &env_at(20.0)).expect("init");

        eq.apply_control(&ControlSignal::DutyCycle {
            on_fraction: 0.0,
            period_s: None,
            component: Some(DutyCycleComponent::BackupElement),
        })
        .expect("backup duty cycle accepted");

        assert!(
            (eq.er_duty_cycle - 0.0).abs() < 1e-10,
            "er_duty_cycle should be 0.0, got {}",
            eq.er_duty_cycle
        );
        assert!(
            (eq.hp_duty_cycle - 1.0).abs() < 1e-10,
            "hp_duty_cycle should remain 1.0, got {}",
            eq.hp_duty_cycle
        );
    }

    #[test]
    fn whole_equipment_duty_cycle_applies_to_both_components() {
        let mut typed = base_typed_config();
        typed.setpoint_c = Some(50.0);
        let cfg = equipment_config(typed);
        let mut eq = HeatPumpWH::new(cfg.clone());
        eq.init(&cfg, &env_at(20.0)).expect("init");

        eq.apply_control(&ControlSignal::DutyCycle {
            on_fraction: 0.3,
            period_s: None,
            component: None,
        })
        .expect("whole-equipment duty cycle accepted");

        assert!(
            (eq.duty_cycle - 0.3).abs() < 1e-10,
            "duty_cycle should be 0.3, got {}",
            eq.duty_cycle
        );
        assert!(
            (eq.hp_duty_cycle - 1.0).abs() < 1e-10,
            "hp_duty_cycle should remain 1.0"
        );
        assert!(
            (eq.er_duty_cycle - 1.0).abs() < 1e-10,
            "er_duty_cycle should remain 1.0"
        );
    }

    #[test]
    fn compressor_kw_and_element_kw_present_and_sum_to_electric_kw_without_fan() {
        let mut typed = base_typed_config();
        typed.initial_tank_temp_c = Some(40.0);
        typed.fan_power_w = Some(0.0);
        typed.parasitic_power_w = Some(0.0);
        let cfg = equipment_config(typed);

        let mut eq = HeatPumpWH::new(cfg.clone());
        eq.init(&cfg, &env_at(20.0)).unwrap();
        eq.apply_control(&ControlSignal::ModeOverride {
            mode: OperatingMode::HeatPumpWH,
        })
        .unwrap();

        let mut p = ports();
        eq.step(&env_at(20.0), Duration::from_secs(60), &mut p)
            .unwrap();

        let electric_kw = eq
            .telemetry()
            .get(tk::ELECTRIC_KW)
            .expect("ELECTRIC_KW must be present");
        let compressor_kw = eq
            .telemetry()
            .get(tk::COMPRESSOR_KW)
            .expect("COMPRESSOR_KW must be present");
        let element_kw = eq
            .telemetry()
            .get(tk::ELEMENT_KW)
            .expect("ELEMENT_KW must be present");

        assert!(
            electric_kw > 0.0,
            "HPWH must be drawing power for this test to be meaningful"
        );
        let sub_sum = compressor_kw + element_kw;
        let rel_err = (sub_sum - electric_kw).abs() / electric_kw.max(f64::MIN_POSITIVE);
        assert!(
            rel_err < 1e-9,
            "compressor_kw {compressor_kw:.6} + element_kw {element_kw:.6} = {sub_sum:.6} \
             must equal electric_kw {electric_kw:.6} (rel_err={rel_err:.2e})"
        );
    }
}
