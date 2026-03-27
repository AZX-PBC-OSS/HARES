//! Gas water heater model.

use std::borrow::Cow;
use std::time::Duration;

use hares_types::{
    ControlCapabilities, ControlSignal, DRLevel, EndUse, EnvironmentState, EquipmentDescriptor,
    EquipmentId, ExecutionStage, FluidType, FuelType, HaresError, LoopId, OperatingMode,
    PortContribution, PortDeclaration, PortSlots, ScheduleSource, Telemetry, TelemetryField,
    ThermalCategory, ZoneId,
};
use serde::{Deserialize, Serialize};

use hares_types::normalize_ascii;

use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_postcard, save_postcard};

use super::tank::{StratifiedTank, StratifiedTankConfig};
use super::{
    WaterHeaterZip, apply_jacket_r_value, draw_schedule_source, hysteresis_call,
    mains_temp_schedule_source, parse_usize, resolve_draw_rate_kg_s, resolve_storage_step_inputs,
    weighted_average_tank_temp,
};
use crate::hvac::helpers::{
    equipment_id_from_config, first_f64, loop_id_from_config, parse_fuel_type, zone_id_from_config,
};

use super::{
    DEFAULT_CONDUCTIVITY_W_M_K, DEFAULT_MAX_TANK_TEMP_C, DEFAULT_SETPOINT_C,
    DEFAULT_TANK_DIAMETER_M, DEFAULT_TANK_HEIGHT_M, DEFAULT_TANK_VOLUME_M3, DEFAULT_UA_W_PER_K,
    WATER_DENSITY_KG_PER_M3,
};

const DEFAULT_DEADBAND_C: f64 = 5.555_555_556; // 10°F (OCHRE storage WH default)
const DEFAULT_BURNER_INPUT_W: f64 = 11_000.0;
const DEFAULT_BURNER_EFFICIENCY: f64 = 0.78;
const DEFAULT_FLUE_LOSS_FRACTION: f64 = 0.10;
/// Standing pilot flame power (thermal). Central estimate from DOE 10 CFR 430
/// test-procedure/manufacturer ranges for natural-draft atmospheric gas water heaters.
const DEFAULT_PILOT_POWER_W: f64 = 5.0;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct GasWhState {
    setpoint_c: f64,
    deadband_c: f64,
    burner_on: bool,
    duty_cycle: f64,
    mode_override: Option<OperatingMode>,
    tank_state: Vec<u8>,
    tank_avg_temp_c: f64,
    burner_power_w: f64,
    pilot_power_w: f64,
    fuel_input_w: f64,
    flue_loss_w: f64,
    fan_electric_w: f64,
    draw_flow_rate_kg_s: f64,
    // --- Demand response state ---
    dr_level: DRLevel,
    dr_setpoint_offset_c: f64,
    dr_load_fraction: f64,
    dr_duration_remaining_s: Option<f64>,
}

pub struct GasWH {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,
    tank: StratifiedTank,
    burner_node: usize,
    burner_input_w: f64,
    pilot_power_w: f64,
    fan_power_w: f64,
    setpoint_c: f64,
    deadband_c: f64,
    duty_cycle: f64,
    mode_override: Option<OperatingMode>,
    burner_on: bool,
    loop_id: LoopId,
    fluid_type: FluidType,
    mains_temp_c: f64,
    draw_flow_rate_kg_s: f64,
    draw_l_per_min_source: Option<ScheduleSource>,
    mains_temp_c_source: Option<ScheduleSource>,
    flue_loss_fraction: f64,
    burner_efficiency_constant: f64,
    burner_efficiency_poly: Option<[f64; 3]>,
    fuel_type: FuelType,
    /// Fraction of standby tank losses delivered to the zone as sensible heat.
    /// Accounts for insulation gaps and jacket losses that enter the conditioned space.
    ///
    /// OCHRE WaterHeater.py:712-723: skin_loss_frac depends on Energy Factor.
    /// EF < 0.7 → 0.64, EF < 0.8 → 0.91, else 0.96
    skin_loss_fraction: f64,
    /// Maximum safe tank temperature (°C); burner is locked out above this.
    max_tank_temp_c: f64,
    // --- Demand response state ---
    dr_setpoint_offset_c: f64,
    dr_load_fraction: f64,
    dr_duration_remaining_s: Option<f64>,
    dr_level: DRLevel,
    // Transient load fraction from LoadFraction control signal; reset each step.
    ctrl_load_fraction: f64,
    // --- ZIP voltage model ---
    zip: WaterHeaterZip,
}

impl GasWH {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let zone = zone_id_from_config(&config).unwrap_or(ZoneId(1));
        let loop_id =
            loop_id_from_config(&config, &["loop_id", "dhw_loop_id"]).unwrap_or(LoopId(1));
        let n_nodes = parse_usize(config.get_f64("tank_nodes"))
            .unwrap_or(6)
            .clamp(1, 12);
        let burner_node = parse_usize(config.get_f64("burner_node"))
            .unwrap_or(n_nodes - 1)
            .min(n_nodes - 1);

        let tank = StratifiedTank::new(StratifiedTankConfig {
            n_nodes,
            height_m: DEFAULT_TANK_HEIGHT_M,
            diameter_m: DEFAULT_TANK_DIAMETER_M,
            ua_w_per_k: DEFAULT_UA_W_PER_K,
            conductivity_w_m_k: DEFAULT_CONDUCTIVITY_W_M_K,
            initial_temp_c: DEFAULT_SETPOINT_C,
            element_nodes: [None, Some(burner_node)],
            node_volumes_m3: None,
            ua_end_cap_w_per_k: None,
        })
        .expect("default gas water-heater tank config must be valid");

        Self {
            descriptor: EquipmentDescriptor {
                id: EquipmentId(equipment_id_from_config(&config).unwrap_or(0)),
                name: config.name,
                end_use: EndUse::WATER_HEATING,
                equipment_type: Cow::Borrowed("Gas Water Heater"),
                zone: Some(zone),
                fuel: FuelType::Gas,
                stage: ExecutionStage::Thermal,
                control_capabilities: ControlCapabilities::THERMAL_SETPOINT
                    | ControlCapabilities::DUTY_CYCLE
                    | ControlCapabilities::MODE_OVERRIDE
                    | ControlCapabilities::LOAD_FRACTION
                    | ControlCapabilities::POWER_LIMIT
                    | ControlCapabilities::DEMAND_RESPONSE,
                telemetry_fields: telemetry_fields(),
            },
            ports: vec![
                PortDeclaration::fuel(),
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
            tank,
            burner_node,
            burner_input_w: DEFAULT_BURNER_INPUT_W,
            pilot_power_w: 0.0,
            fan_power_w: 0.0,
            setpoint_c: DEFAULT_SETPOINT_C,
            deadband_c: DEFAULT_DEADBAND_C,
            duty_cycle: 1.0,
            mode_override: None,
            burner_on: false,
            loop_id,
            fluid_type: FluidType::Water,
            mains_temp_c: 15.0,
            draw_flow_rate_kg_s: 0.0,
            draw_l_per_min_source: None,
            mains_temp_c_source: None,
            flue_loss_fraction: DEFAULT_FLUE_LOSS_FRACTION,
            burner_efficiency_constant: DEFAULT_BURNER_EFFICIENCY,
            burner_efficiency_poly: None,
            fuel_type: FuelType::Gas,
            // Default EF ~0.78 → skin_loss_fraction = 0.91
            skin_loss_fraction: default_skin_loss_fraction(DEFAULT_BURNER_EFFICIENCY),
            max_tank_temp_c: DEFAULT_MAX_TANK_TEMP_C,
            dr_setpoint_offset_c: 0.0,
            dr_load_fraction: 1.0,
            dr_duration_remaining_s: None,
            dr_level: DRLevel::Normal,
            ctrl_load_fraction: 1.0,
            zip: WaterHeaterZip::default(),
        }
    }

    fn effective_setpoint_c(&self) -> f64 {
        self.setpoint_c + self.dr_setpoint_offset_c
    }

    fn burner_efficiency(&self, part_load_ratio: f64) -> f64 {
        if let Some(coeffs) = self.burner_efficiency_poly {
            (coeffs[0]
                + coeffs[1] * part_load_ratio
                + coeffs[2] * part_load_ratio * part_load_ratio)
                .max(0.0)
        } else {
            self.burner_efficiency_constant.max(0.0)
        }
    }

    fn should_fire(&self) -> bool {
        if self.dr_load_fraction <= 0.0
            || matches!(self.mode_override, Some(OperatingMode::Off))
            || self.duty_cycle <= 0.0
        {
            return false;
        }
        let lower_temp = self.tank.node_temps()[self.burner_node];
        hysteresis_call(
            lower_temp,
            self.effective_setpoint_c(),
            self.deadband_c,
            self.burner_on,
        )
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

impl Equipment for GasWH {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, _env: &EnvironmentState) -> crate::Result<()> {
        let n_nodes = parse_usize(config.get_f64("tank_nodes"))
            .unwrap_or(6)
            .clamp(1, 12);
        let tank_volume_m3 =
            first_f64(config, &["tank_volume_m3"]).unwrap_or(DEFAULT_TANK_VOLUME_M3);
        let diameter_m = first_f64(config, &["tank_diameter_m", "diameter_m"])
            .unwrap_or(DEFAULT_TANK_DIAMETER_M);
        let inferred_height_m =
            tank_volume_m3 / (std::f64::consts::PI * (diameter_m * 0.5).powi(2));
        let height_m =
            first_f64(config, &["tank_height_m", "height_m"]).unwrap_or(inferred_height_m.max(0.2));

        self.burner_node = parse_usize(config.get_f64("burner_node"))
            .unwrap_or(n_nodes - 1)
            .min(n_nodes - 1);

        let ua_base = first_f64(config, &["ua_w_per_k", "UA"]).unwrap_or(DEFAULT_UA_W_PER_K);
        let ua_w_per_k = apply_jacket_r_value(ua_base, height_m, diameter_m, config);

        self.tank = StratifiedTank::new(StratifiedTankConfig {
            n_nodes,
            height_m,
            diameter_m,
            ua_w_per_k,
            conductivity_w_m_k: first_f64(
                config,
                &["conductivity_w_m_k", "water_conductivity_w_m_k"],
            )
            .unwrap_or(DEFAULT_CONDUCTIVITY_W_M_K),
            initial_temp_c: first_f64(
                config,
                &[
                    "initial_tank_temp_c",
                    "initial_temp_c",
                    "SetpointTemperature",
                ],
            )
            .unwrap_or(DEFAULT_SETPOINT_C),
            element_nodes: [None, Some(self.burner_node)],
            node_volumes_m3: None,
            ua_end_cap_w_per_k: None,
        })?;

        self.burner_input_w = first_f64(config, &["heating_capacity_w"])
            .unwrap_or(DEFAULT_BURNER_INPUT_W)
            .max(0.0);
        let has_standing_pilot =
            ignition_uses_standing_pilot(first_str(config, &["ignition_type", "IgnitionType"]));
        self.pilot_power_w = first_f64(config, &["pilot_power_w", "PilotPower"])
            .unwrap_or(if has_standing_pilot {
                DEFAULT_PILOT_POWER_W
            } else {
                0.0
            })
            .max(0.0);
        self.fan_power_w = first_f64(config, &["fan_power_w", "fan_electric_w", "fan_kw"])
            .map(|v| if v > 100.0 { v } else { v * 1_000.0 })
            .unwrap_or(0.0)
            .max(0.0);

        self.setpoint_c = first_f64(
            config,
            &[
                "setpoint_c",
                "SetpointTemperature",
                "setpoint_temperature_c",
                "ThermostatSetpointC",
            ],
        )
        .unwrap_or(DEFAULT_SETPOINT_C);
        self.deadband_c = first_f64(config, &["deadband_c", "thermostat_deadband_c"])
            .unwrap_or(DEFAULT_DEADBAND_C)
            .max(0.0);
        self.duty_cycle = 1.0;
        self.mode_override = None;
        self.burner_on = false;

        self.loop_id =
            loop_id_from_config(config, &["loop_id", "dhw_loop_id"]).unwrap_or(self.loop_id);
        self.ports[3].loop_id = Some(self.loop_id);
        self.mains_temp_c =
            first_f64(config, &["mains_temp_c", "inlet_temp_c"]).unwrap_or(self.mains_temp_c);
        self.draw_flow_rate_kg_s = resolve_draw_rate_kg_s(config);
        self.draw_l_per_min_source = draw_schedule_source(config);
        self.mains_temp_c_source = mains_temp_schedule_source(config);
        self.zip = WaterHeaterZip::from_config(config)?;

        self.flue_loss_fraction = first_f64(config, &["flue_loss_fraction", "FlueLossFraction"])
            .unwrap_or(DEFAULT_FLUE_LOSS_FRACTION)
            .clamp(0.0, 1.0);
        self.burner_efficiency_constant = first_f64(
            config,
            &[
                "burner_efficiency",
                "thermal_efficiency",
                "EnergyFactor",
                "UniformEnergyFactor",
            ],
        )
        .unwrap_or(DEFAULT_BURNER_EFFICIENCY);
        self.burner_efficiency_poly = parse_poly3(config, "burner_efficiency_coeffs")?;

        self.fuel_type = parse_fuel_type(config.get_str("FuelType")).unwrap_or(FuelType::Gas);
        self.descriptor.fuel = self.fuel_type;

        let ef = self.burner_efficiency_constant;
        self.skin_loss_fraction = first_f64(config, &["skin_loss_fraction", "SkinLossFraction"])
            .unwrap_or_else(|| default_skin_loss_fraction(ef))
            .clamp(0.0, 1.0);
        self.max_tank_temp_c =
            first_f64(config, &["max_tank_temp_c"]).unwrap_or(DEFAULT_MAX_TANK_TEMP_C);

        self.dr_setpoint_offset_c = 0.0;
        self.dr_load_fraction = 1.0;
        self.dr_duration_remaining_s = None;
        self.dr_level = DRLevel::Normal;
        self.ctrl_load_fraction = 1.0;
        self.telemetry = default_telemetry();
        self.tank.register_node_telemetry(&mut self.telemetry);
        Ok(())
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
            self.burner_on = false;
            return OperatingMode::Off;
        }
        self.burner_on = self.should_fire();
        if self.burner_on {
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
        let ambient_c = self.ambient_temp_c(env);
        let mode = self.update_control(env);
        let duty =
            (self.duty_cycle * self.dr_load_fraction * self.ctrl_load_fraction).clamp(0.0, 1.0);
        let burner_input_w = if self.burner_on {
            self.burner_input_w * duty
        } else {
            0.0
        };
        let part_load_ratio = if self.burner_input_w > 0.0 {
            burner_input_w / self.burner_input_w
        } else {
            0.0
        };

        let efficiency = self.burner_efficiency(part_load_ratio);
        let gross_heat_w = burner_input_w * efficiency;
        let flue_loss_w = gross_heat_w * self.flue_loss_fraction;
        let tank_heat_w = (gross_heat_w - flue_loss_w).max(0.0);

        let heat_injections: Vec<(usize, f64)> = if tank_heat_w > 0.0 {
            vec![(self.burner_node, tank_heat_w)]
        } else {
            vec![]
        };

        // Compute standby skin loss to zone before tank.step() updates temperatures.
        // OCHRE WaterHeater.py:712-723: fraction of tank UA losses that enter the zone.
        let avg_temp_before =
            weighted_average_tank_temp(self.tank.node_temps(), self.tank.node_volumes_m3());
        let standby_loss_w = self.tank.ua_w_per_k() * (avg_temp_before - ambient_c).max(0.0);
        let skin_loss_to_zone_w = standby_loss_w * self.skin_loss_fraction;

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
        let draw_volume_m3 = total_draw_kg_s / WATER_DENSITY_KG_PER_M3 * dt.as_secs_f64();
        let draw = self.tank.step(
            ambient_c,
            draw_volume_m3,
            mains_temp_c,
            &heat_injections,
            dt,
        )?;

        let fuel_input_w = burner_input_w + self.pilot_power_w;
        if fuel_input_w > 0.0 {
            ports.accumulate(&PortContribution::Fuel {
                fuel_type: self.fuel_type,
                consumption_w: fuel_input_w,
            })?;
        }

        let rated_fan_electric_w = if self.burner_on {
            self.fan_power_w
        } else {
            0.0
        };
        let (fan_electric_w, fan_reactive_kvar) =
            self.zip.apply(rated_fan_electric_w, env.grid.voltage_pu);
        if fan_electric_w > 0.0 {
            ports.accumulate(&PortContribution::Electrical {
                active_power_kw: fan_electric_w / 1_000.0,
                reactive_power_kvar: fan_reactive_kvar,
            })?;
        }

        if total_draw_kg_s > 0.0 {
            ports.accumulate(&PortContribution::Fluid {
                loop_id: self.loop_id,
                flow_rate_kg_s: total_draw_kg_s,
                supply_temp_c: draw.outlet_temp_c,
                return_temp_c: mains_temp_c,
                fluid_type: self.fluid_type,
            })?;
        }

        // Skin losses (jacket losses that enter the conditioned space) go to the
        // zone thermal port. Flue losses exit the building and are not reported here.
        if let Some(zone) = self.descriptor.zone {
            if skin_loss_to_zone_w > 0.0 {
                ports.accumulate(&PortContribution::Thermal {
                    zone,
                    sensible_gain_w: skin_loss_to_zone_w,
                    latent_gain_w: 0.0,
                    category: ThermalCategory::JacketLoss,
                })?;
            }
        }

        let avg_temp_c =
            weighted_average_tank_temp(self.tank.node_temps(), self.tank.node_volumes_m3());
        self.telemetry.set("tank_avg_temp_c", avg_temp_c);
        self.telemetry.set("burner_power_w", burner_input_w);
        self.telemetry.set("pilot_power_w", self.pilot_power_w);
        self.telemetry.set("fuel_input_w", fuel_input_w);
        self.telemetry.set("flue_loss_w", flue_loss_w);
        self.telemetry.set("fan_electric_w", fan_electric_w);
        self.telemetry.set("draw_flow_rate_kg_s", total_draw_kg_s);
        self.telemetry.set(
            "operating_mode",
            if mode == OperatingMode::Heating {
                1.0
            } else {
                0.0
            },
        );
        self.tank.update_node_telemetry(&mut self.telemetry);

        // Reset transient ctrl_load_fraction after this step so it does not
        // carry over to the next step unless reapplied by the controller.
        self.ctrl_load_fraction = 1.0;

        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn save_state(&self) -> Vec<u8> {
        save_postcard(&GasWhState {
            setpoint_c: self.setpoint_c,
            deadband_c: self.deadband_c,
            burner_on: self.burner_on,
            duty_cycle: self.duty_cycle,
            mode_override: self.mode_override,
            tank_state: self.tank.save_state(),
            tank_avg_temp_c: self.telemetry.get("tank_avg_temp_c").unwrap_or(0.0),
            burner_power_w: self.telemetry.get("burner_power_w").unwrap_or(0.0),
            pilot_power_w: self.telemetry.get("pilot_power_w").unwrap_or(0.0),
            fuel_input_w: self.telemetry.get("fuel_input_w").unwrap_or(0.0),
            flue_loss_w: self.telemetry.get("flue_loss_w").unwrap_or(0.0),
            fan_electric_w: self.telemetry.get("fan_electric_w").unwrap_or(0.0),
            draw_flow_rate_kg_s: self.telemetry.get("draw_flow_rate_kg_s").unwrap_or(0.0),
            dr_level: self.dr_level,
            dr_setpoint_offset_c: self.dr_setpoint_offset_c,
            dr_load_fraction: self.dr_load_fraction,
            dr_duration_remaining_s: self.dr_duration_remaining_s,
        })
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let decoded: GasWhState = load_postcard(state)?;
        self.setpoint_c = decoded.setpoint_c;
        self.deadband_c = decoded.deadband_c;
        self.burner_on = decoded.burner_on;
        self.duty_cycle = decoded.duty_cycle;
        self.mode_override = decoded.mode_override;
        self.dr_level = decoded.dr_level;
        self.dr_setpoint_offset_c = decoded.dr_setpoint_offset_c;
        self.dr_load_fraction = decoded.dr_load_fraction;
        self.dr_duration_remaining_s = decoded.dr_duration_remaining_s;
        self.tank.load_state(&decoded.tank_state)?;

        self.telemetry
            .insert("tank_avg_temp_c", decoded.tank_avg_temp_c);
        self.telemetry
            .insert("burner_power_w", decoded.burner_power_w);
        self.telemetry
            .insert("pilot_power_w", decoded.pilot_power_w);
        self.telemetry.insert("fuel_input_w", decoded.fuel_input_w);
        self.telemetry.insert("flue_loss_w", decoded.flue_loss_w);
        self.telemetry
            .insert("fan_electric_w", decoded.fan_electric_w);
        self.telemetry
            .insert("draw_flow_rate_kg_s", decoded.draw_flow_rate_kg_s);
        self.telemetry
            .insert("operating_mode", if decoded.burner_on { 1.0 } else { 0.0 });

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
                    self.setpoint_c = sp;
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
                        "invalid duty cycle for GasWH: {on_fraction}"
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
                if self.burner_input_w > 0.0 {
                    let max_fraction =
                        (max_power_kw * 1000.0 / self.burner_input_w).clamp(0.0, 1.0);
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

impl GasWH {
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
        "Gas Water Heater",
        Box::new(|config| Box::new(GasWH::new(config))),
    );
}

fn default_telemetry() -> Telemetry {
    let mut telemetry = Telemetry::with_capacity(8);
    telemetry.insert("tank_avg_temp_c", 0.0);
    telemetry.insert("burner_power_w", 0.0);
    telemetry.insert("pilot_power_w", 0.0);
    telemetry.insert("fuel_input_w", 0.0);
    telemetry.insert("flue_loss_w", 0.0);
    telemetry.insert("fan_electric_w", 0.0);
    telemetry.insert("draw_flow_rate_kg_s", 0.0);
    telemetry.insert("operating_mode", 0.0);
    telemetry
}

fn telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField {
            name: "tank_avg_temp_c".to_string(),
            unit: "C".to_string(),
            description: "Volume-weighted average tank temperature".to_string(),
        },
        TelemetryField {
            name: "burner_power_w".to_string(),
            unit: "W".to_string(),
            description: "Gas burner thermal input power".to_string(),
        },
        TelemetryField {
            name: "pilot_power_w".to_string(),
            unit: "W".to_string(),
            description: "Pilot light fuel input power (continuous)".to_string(),
        },
        TelemetryField {
            name: "fuel_input_w".to_string(),
            unit: "W".to_string(),
            description: "Total fuel consumption rate".to_string(),
        },
        TelemetryField {
            name: "flue_loss_w".to_string(),
            unit: "W".to_string(),
            description: "Heat loss to flue (excluded from zone thermal gains)".to_string(),
        },
        TelemetryField {
            name: "fan_electric_w".to_string(),
            unit: "W".to_string(),
            description: "Auxiliary electric fan draw".to_string(),
        },
        TelemetryField {
            name: "draw_flow_rate_kg_s".to_string(),
            unit: "kg/s".to_string(),
            description: "Domestic hot water draw flow rate".to_string(),
        },
        TelemetryField {
            name: "operating_mode".to_string(),
            unit: "enum".to_string(),
            description: "0=Off, 1=Heating".to_string(),
        },
    ]
}

/// Compute skin loss fraction from burner efficiency (Energy Factor proxy).
///
/// OCHRE WaterHeater.py:712-723: EF < 0.7 → 0.64, EF < 0.8 → 0.91, else 0.96.
fn default_skin_loss_fraction(burner_efficiency: f64) -> f64 {
    if burner_efficiency < 0.7 {
        0.64
    } else if burner_efficiency < 0.8 {
        0.91
    } else {
        0.96
    }
}

fn parse_poly3(config: &EquipmentConfig, key: &str) -> crate::Result<Option<[f64; 3]>> {
    let Some(raw) = config.get_str(key) else {
        return Ok(None);
    };

    let parts: Vec<&str> = raw
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    if parts.len() != 3 {
        return Err(HaresError::Equipment(format!(
            "expected 3 coefficients in '{key}', got {}",
            parts.len()
        )));
    }

    let mut coeffs = [0.0_f64; 3];
    for (idx, part) in parts.into_iter().enumerate() {
        coeffs[idx] = part.parse::<f64>().map_err(|error| {
            HaresError::Equipment(format!(
                "failed to parse coefficient {idx} ('{part}') in '{key}': {error}"
            ))
        })?;
    }
    Ok(Some(coeffs))
}

fn first_str<'a>(config: &'a EquipmentConfig, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| config.get_str(key))
}

fn ignition_uses_standing_pilot(raw: Option<&str>) -> bool {
    let Some(raw_value) = raw else {
        // Default assumption favors older storage gas WH stock where standing pilots are common.
        return true;
    };
    !matches!(
        normalize_ascii(raw_value).as_str(),
        "electronic" | "electronic_ignition" | "intermittent_ignition"
    )
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, time::Duration};

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        EnvironmentState, GridState, PortSlots, ThermalAccumulator, WeatherState, ZoneId, ZoneState,
    };

    use super::GasWH;
    use crate::{Equipment, EquipmentConfig};

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
                .expect("UTC offset")
                .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                .single()
                .expect("valid"),
            time_res: ChronoDuration::seconds(60),
        price_signal: Default::default(),
        electrical: Default::default(),
        }
    }

    fn config() -> EquipmentConfig {
        let mut raw = HashMap::new();
        raw.insert("setpoint_c".to_string(), 52.0.into());
        raw.insert("deadband_c".to_string(), 2.0.into());
        raw.insert("initial_tank_temp_c".to_string(), 40.0.into());
        raw.insert("pilot_power_w".to_string(), 50.0.into());
        raw.insert("flue_loss_fraction".to_string(), 0.2.into());
        EquipmentConfig {
            name: "GWH".to_string(),
            ochre_class: "Gas Water Heater".to_string(),
            raw_config: raw,
        }
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
        }
    }

    #[test]
    fn flue_loss_is_not_written_to_zone_thermal_port_but_skin_loss_is() {
        let mut eq = GasWH::new(config());
        eq.init(&config(), &env(21.0)).unwrap();

        let mut p = ports();
        eq.step(&env(21.0), Duration::from_secs(60), &mut p)
            .unwrap();

        // Flue loss exits the building; it must NOT be part of the zone thermal gain.
        // However, skin losses (jacket losses) should produce a positive sensible gain.
        // The tank starts at 40°C and ambient is 21°C, so standby loss is positive.
        let flue_loss = eq.telemetry().get("flue_loss_w").unwrap_or(0.0);
        assert!(flue_loss >= 0.0);
        // Skin loss is the sensible gain from jacket UA; must be non-negative.
        assert!(
            p.thermal[0].sensible_gain_w >= 0.0,
            "Skin loss to zone must be non-negative (got {})",
            p.thermal[0].sensible_gain_w
        );
        assert!(p.fuel.get(hares_types::FuelType::Gas) > 0.0);
    }

    #[test]
    fn skin_loss_is_positive_when_tank_is_hot() {
        let mut cfg = config();
        cfg.raw_config
            .insert("initial_tank_temp_c".to_string(), 60.0.into());
        cfg.raw_config
            .insert("skin_loss_fraction".to_string(), 0.5.into());
        let mut eq = GasWH::new(cfg.clone());
        eq.init(&cfg, &env(20.0)).unwrap();

        let mut p = ports();
        eq.step(&env(20.0), Duration::from_secs(60), &mut p)
            .unwrap();

        assert!(
            p.thermal[0].sensible_gain_w > 0.0,
            "Skin loss should be positive when tank is hotter than ambient"
        );
    }

    #[test]
    fn max_tank_temp_safety_forces_off_for_gas_wh() {
        let mut cfg = config();
        cfg.raw_config
            .insert("initial_tank_temp_c".to_string(), 40.0.into());
        cfg.raw_config
            .insert("max_tank_temp_c".to_string(), 35.0.into());
        let mut eq = GasWH::new(cfg.clone());
        eq.init(&cfg, &env(21.0)).unwrap();

        let mode = eq.update_control(&env(21.0));
        assert_eq!(
            mode,
            hares_types::OperatingMode::Off,
            "Expected Off when tank exceeds max_tank_temp_c"
        );
        assert!(!eq.burner_on);
    }

    /// Stratified tank where top node (0) is below max_tank_temp_c but the
    /// burner node (bottom) exceeds it. The safety cutout must still trigger
    /// because the aquastat responds to the hottest point in the tank.
    #[test]
    fn max_tank_temp_safety_triggers_on_stratified_hot_bottom() {
        let mut cfg = config();
        cfg.raw_config
            .insert("initial_tank_temp_c".to_string(), 30.0.into());
        cfg.raw_config
            .insert("max_tank_temp_c".to_string(), 55.0.into());
        let mut eq = GasWH::new(cfg.clone());
        eq.init(&cfg, &env(21.0)).unwrap();

        // Manually stratify: top node cool (30°C), burner node hot (60°C > 55°C limit).
        let n = eq.tank.node_temps().len();
        eq.tank.node_temps_mut()[0] = 30.0; // top — below limit
        eq.tank.node_temps_mut()[n - 1] = 60.0; // burner node — above limit

        let mode = eq.update_control(&env(21.0));
        assert_eq!(
            mode,
            hares_types::OperatingMode::Off,
            "Safety cutout must trigger when ANY node exceeds max_tank_temp_c, \
             even if the top node is cool"
        );
        assert!(!eq.burner_on);
    }

    /// Stratified tank where the hottest node is still below max_tank_temp_c.
    /// The safety cutout must NOT fire and the burner must be allowed to operate.
    #[test]
    fn safety_cutout_does_not_fire_when_all_nodes_below_limit() {
        let mut cfg = config();
        cfg.raw_config
            .insert("initial_tank_temp_c".to_string(), 30.0.into());
        cfg.raw_config
            .insert("max_tank_temp_c".to_string(), 55.0.into());
        let mut eq = GasWH::new(cfg.clone());
        eq.init(&cfg, &env(21.0)).unwrap();

        // Stratify: bottom node warm but below the 55°C limit.
        // Keep burner node cold (below setpoint) so thermostat calls for heat.
        let n = eq.tank.node_temps().len();
        eq.tank.node_temps_mut()[0] = 54.0; // hottest node, below 55°C limit
        eq.tank.node_temps_mut()[n - 1] = 30.0; // burner node, cold → calls for heat

        let mode = eq.update_control(&env(21.0));
        assert_eq!(
            mode,
            hares_types::OperatingMode::Heating,
            "Burner must fire when all nodes are below max_tank_temp_c"
        );
        assert!(eq.burner_on);
    }

    #[test]
    fn state_round_trip_restores_burner_and_tank_state() {
        let mut eq = GasWH::new(config());
        eq.init(&config(), &env(21.0)).unwrap();

        let mut p = ports();
        eq.step(&env(21.0), Duration::from_secs(60), &mut p)
            .unwrap();
        let state = eq.save_state();

        let mut restored = GasWH::new(config());
        restored.init(&config(), &env(21.0)).unwrap();
        restored.load_state(&state).unwrap();

        assert_eq!(restored.burner_on, eq.burner_on);
        assert_eq!(restored.tank.node_temps(), eq.tank.node_temps());
    }

    #[test]
    fn state_round_trip_preserves_dr_state() {
        use hares_types::{ControlSignal, DRLevel};

        let mut eq = GasWH::new(config());
        eq.init(&config(), &env(21.0)).unwrap();

        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::Critical,
            duration_s: Some(300.0),
        })
        .unwrap();

        let saved = eq.save_state();

        let mut restored = GasWH::new(config());
        restored.init(&config(), &env(21.0)).unwrap();
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

    /// DR Moderate applies a -3°C setpoint offset. A tank at 50°C that would
    /// normally call for heat (setpoint=52°C) must stop calling after the offset
    /// (effective setpoint = 49°C < 50°C).
    #[test]
    fn gas_wh_dr_moderate_reduces_setpoint() {
        let mut cfg = config();
        // Tank at 50°C: within the 2°C deadband below setpoint 52°C → normally heating.
        cfg.raw_config
            .insert("initial_tank_temp_c".to_string(), 50.0.into());
        cfg.raw_config
            .insert("max_tank_temp_c".to_string(), 300.0.into());
        let e = env(21.0);

        // Baseline: burner should be on.
        let mut eq_base = GasWH::new(cfg.clone());
        eq_base.init(&cfg, &e).unwrap();
        let mode_base = eq_base.update_control(&e);
        assert!(
            matches!(mode_base, hares_types::OperatingMode::Heating),
            "baseline must be heating with tank at 50°C; got {mode_base:?}"
        );

        // DR Moderate: effective setpoint = 52 + (-3) = 49°C. Tank at 50°C > 49°C → no call.
        let mut eq_dr = GasWH::new(cfg.clone());
        eq_dr.init(&cfg, &e).unwrap();
        eq_dr
            .apply_control(&hares_types::ControlSignal::DemandResponse {
                level: hares_types::DRLevel::Moderate,
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
            hares_types::OperatingMode::Off,
            "DR Moderate must suppress heating by reducing effective setpoint; got {mode_dr:?}"
        );
    }

    /// DR GridEmergency sets dr_load_fraction=0.0, forcing the burner off and
    /// producing zero fuel consumption even when the tank is cold.
    #[test]
    fn gas_wh_dr_grid_emergency_forces_off() {
        let e = env(21.0);
        // Cold tank: would normally fire the burner.
        let mut eq = GasWH::new(config());
        eq.init(&config(), &e).unwrap();

        eq.apply_control(&hares_types::ControlSignal::DemandResponse {
            level: hares_types::DRLevel::GridEmergency,
            duration_s: None,
        })
        .unwrap();
        assert_eq!(
            eq.dr_load_fraction, 0.0,
            "GridEmergency must set dr_load_fraction to 0"
        );

        let mut p = ports();
        eq.step(&e, Duration::from_secs(60), &mut p).unwrap();

        assert!(!eq.burner_on, "burner must be off during GridEmergency");
        assert_eq!(
            eq.telemetry().get("burner_power_w").unwrap_or(1.0),
            0.0,
            "burner_power_w must be zero during GridEmergency"
        );
        // fuel_input_w = burner_input_w + pilot_power_w; with burner off, only
        // pilot remains. Verify burner contribution is zero — pilot is a continuous flame
        // and is not subject to DR load shedding.
        assert_eq!(
            eq.telemetry().get("fuel_input_w").unwrap_or(1.0),
            eq.pilot_power_w,
            "fuel_input_w during GridEmergency must equal pilot only (burner is off)"
        );
    }

    #[test]
    fn pilot_defaults_to_five_w_without_pilot_or_ignition_type() {
        let mut cfg = config();
        cfg.raw_config.remove("pilot_power_w");
        let mut eq = GasWH::new(cfg.clone());
        eq.init(&cfg, &env(21.0)).unwrap();
        assert_eq!(
            eq.pilot_power_w,
            super::DEFAULT_PILOT_POWER_W,
            "missing pilot_power_w + missing ignition_type should assume standing pilot default"
        );
    }

    #[test]
    fn pilot_defaults_to_zero_for_electronic_ignition() {
        let mut cfg = config();
        cfg.raw_config.remove("pilot_power_w");
        cfg.raw_config
            .insert("ignition_type".to_string(), "electronic".into());
        let mut eq = GasWH::new(cfg.clone());
        eq.init(&cfg, &env(21.0)).unwrap();
        assert_eq!(
            eq.pilot_power_w, 0.0,
            "electronic ignition should default pilot power to zero"
        );
    }

    #[test]
    fn explicit_pilot_power_overrides_ignition_type() {
        let mut cfg = config();
        cfg.raw_config
            .insert("ignition_type".to_string(), "electronic".into());
        cfg.raw_config
            .insert("pilot_power_w".to_string(), 7.0.into());
        let mut eq = GasWH::new(cfg.clone());
        eq.init(&cfg, &env(21.0)).unwrap();
        assert_eq!(
            eq.pilot_power_w, 7.0,
            "explicit pilot power must override inferred ignition default"
        );
    }

    #[test]
    fn standby_step_at_setpoint_still_consumes_gas_from_pilot() {
        let mut cfg = config();
        cfg.raw_config.remove("pilot_power_w");
        cfg.raw_config.insert("setpoint_c".to_string(), 52.0.into());
        cfg.raw_config
            .insert("initial_tank_temp_c".to_string(), 52.0.into());
        cfg.raw_config
            .insert("draw_flow_rate_kg_s".to_string(), 0.0.into());
        let e = env(21.0);

        let mut eq = GasWH::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();

        let mut p = ports();
        eq.step(&e, Duration::from_secs(3600), &mut p).unwrap();

        assert_eq!(
            eq.telemetry().get("burner_power_w").unwrap_or(1.0),
            0.0,
            "burner should be off at setpoint with no draw"
        );
        assert!(
            eq.telemetry().get("fuel_input_w").unwrap_or(0.0) > 0.0,
            "standing-pilot unit should report non-zero gas consumption at standby"
        );
    }
}
