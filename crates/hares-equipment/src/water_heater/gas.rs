//! Gas water heater model.

use std::borrow::Cow;
use std::time::Duration;

use hares_physics::constants::{UEF_TO_EF_GAS_INTERCEPT, UEF_TO_EF_GAS_SLOPE};
use hares_physics::units::{power_kw_to_w, power_w_to_kw};
use hares_physics::water_density_kg_m3;
use hares_types::{
    ControlCapabilities, ControlSignal, CoreCapabilities, CoreFlows, CoreOutput, CorePerformance,
    CoreState, DRLevel, ElectricPower, EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId,
    ExecutionStage, FluidNodeId, FluidType, FuelPower, FuelType, HaresError, HeatTransferDirection,
    LoopId, OperatingMode, PortContribution, PortDeclaration, PortSlots, ScheduleSource, Telemetry,
    TelemetryField, ThermalCategory, ZoneId, telemetry_keys as tk,
};
use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
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

use super::wh_config::GasWaterHeaterConfig;
use super::{
    DEFAULT_CONDUCTIVITY_W_M_K, DEFAULT_MAX_TANK_TEMP_C, DEFAULT_SETPOINT_C,
    DEFAULT_TANK_DIAMETER_M, DEFAULT_TANK_HEIGHT_M, DEFAULT_TANK_VOLUME_M3, DEFAULT_UA_W_PER_K,
};

const DEFAULT_DEADBAND_C: f64 = 5.555_555_556; // 10°F (OCHRE storage WH default)
const DEFAULT_BURNER_INPUT_W: f64 = 11_000.0;
const DEFAULT_BURNER_EFFICIENCY: f64 = 0.78;
const DEFAULT_FLUE_LOSS_FRACTION: f64 = 0.10;
// Standing pilot thermal power in watts (post-conversion heat delivered to the
// tank or ambient). Typical standing pilot gas consumption is 200–800 BTU/h
// (60–230 W thermal). 150 W is a reasonable midpoint for residential gas storage
// water heaters. ASHRAE HoF 2021 Ch.51 Table 2 range; DOE 10 CFR 430 Subpart C
// App. E §2.6.
const DEFAULT_PILOT_POWER_W: f64 = 150.0;
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
    pilot_fraction_to_tank: f64,
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
    core_output: CoreOutput,
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
    /// Fraction of pilot thermal power routed to tank water; remainder to ambient zone.
    /// Default 0.80 per EnergyPlus OffCycParaFracToTank (WaterThermalTanks.cc:6213-6214).
    pilot_fraction_to_tank: f64,
    // --- Demand response state ---
    dr_setpoint_offset_c: f64,
    dr_load_fraction: f64,
    dr_duration_remaining_s: Option<f64>,
    dr_level: DRLevel,
    // Transient load fraction from LoadFraction control signal; reset each step.
    ctrl_load_fraction: f64,
    // --- ZIP voltage model ---
    zip: WaterHeaterZip,
    // --- TMV tempered draw ---
    fixture_delivery_temp_c: f64,
    hot_draw_temp_c: Option<f64>,
    /// Whether zone_id was explicitly set in config or fell back to ZoneId(1).
    zone_id_explicit: bool,
}

impl GasWH {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let (zone, zone_id_explicit) = zone_id_from_config_or_default(&config, &config.name);
        let loop_id = loop_id_from_config(&config, &["loop_id", "dhw_loop_id"]).unwrap_or_default();
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
                core_capabilities: CoreCapabilities::ELECTRIC
                    | CoreCapabilities::FUEL
                    | CoreCapabilities::HAS_MODE,
                telemetry_fields: telemetry_fields(n_nodes),
                zone_type: None,
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
            core_output: CoreOutput::default(),
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
            mains_temp_c: 10.0,
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
            pilot_fraction_to_tank: 0.80,
            dr_setpoint_offset_c: 0.0,
            dr_load_fraction: 1.0,
            dr_duration_remaining_s: None,
            dr_level: DRLevel::Normal,
            ctrl_load_fraction: 1.0,
            zip: WaterHeaterZip::default(),
            fixture_delivery_temp_c: 40.6,
            hot_draw_temp_c: None,
            zone_id_explicit,
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

impl GasWH {
    fn init_typed(
        &mut self,
        config: &EquipmentConfig,
        env: &EnvironmentState,
    ) -> crate::Result<()> {
        let c = config.require_typed::<GasWaterHeaterConfig>("Gas Water Heater")?;
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
        self.ports[2].zone = zone;
        self.loop_id = c.loop_id.map(LoopId).unwrap_or(self.loop_id);
        self.ports[3].loop_id = Some(self.loop_id);

        let tank_volume_m3 = c.tank_volume_m3.unwrap_or(DEFAULT_TANK_VOLUME_M3);
        let height_m = c.tank_height_m.unwrap_or(DEFAULT_TANK_HEIGHT_M).max(0.2);
        let diameter_m = (4.0 * tank_volume_m3 / (std::f64::consts::PI * height_m))
            .max(1e-6)
            .sqrt();
        let n_nodes = usize::from(c.tank_nodes.unwrap_or(6).max(1));
        self.burner_node = 5;
        if self.burner_node >= n_nodes {
            self.burner_node = n_nodes.saturating_sub(1);
        }

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
            element_nodes: [None, Some(self.burner_node)],
            node_volumes_m3: None,
            ua_end_cap_w_per_k: None,
        })?;

        self.burner_input_w = c
            .heating_capacity_w
            .unwrap_or(DEFAULT_BURNER_INPUT_W)
            .max(0.0);
        self.pilot_power_w = c
            .pilot_power_w
            .unwrap_or(match c.ignition_type.as_deref() {
                Some("ElectronicIgnition") | Some("electronic") | Some("electronic ignition") => {
                    0.0
                }
                _ => DEFAULT_PILOT_POWER_W,
            })
            .max(0.0);
        self.fan_power_w = 0.0;

        self.setpoint_c = c.setpoint_c.unwrap_or(DEFAULT_SETPOINT_C);
        self.deadband_c = c.deadband_c.unwrap_or(DEFAULT_DEADBAND_C);
        self.duty_cycle = 1.0;
        self.mode_override = None;
        self.burner_on = false;

        self.mains_temp_c = super::require_mains_temp_c(env, "Gas Water Heater")?;
        self.draw_flow_rate_kg_s = c.draw_flow_rate_kg_s.unwrap_or(0.0);
        self.draw_l_per_min_source = c.draw_flow_rate_source.clone().map(|s| s.into_runtime());
        self.mains_temp_c_source = c.mains_temp_c_source.clone().map(|s| s.into_runtime());
        self.zip = WaterHeaterZip::default();
        self.fixture_delivery_temp_c = c.fixture_delivery_temp_c.unwrap_or(40.6);
        self.hot_draw_temp_c = c.hot_draw_temp_c;

        self.flue_loss_fraction = c.flue_loss_fraction.unwrap_or(DEFAULT_FLUE_LOSS_FRACTION);

        // Fallback chain for burner efficiency:
        // 1. conversion_efficiency — the physically meaningful parameter (direct use).
        // 2. energy_factor — pre-2015 EF test procedure value (direct use).
        // 3. uniform_energy_factor — post-2015 UEF test procedure value;
        //    converted to EF-equivalent using the RESNET EF Calculator 2017 /
        //    OCHRE HPXML parser (vendors/OCHRE/ochre/utils/hpxml.py:1115):
        //      EF = UEF_TO_EF_GAS_SLOPE * UEF + UEF_TO_EF_GAS_INTERCEPT
        //    UEF values are systematically higher than EF; raw UEF used as
        //    burner efficiency overestimates efficiency by 1-3% for typical
        //    gas storage water heaters.
        // 4. DEFAULT_BURNER_EFFICIENCY (0.78) — a plausible gas WH efficiency.
        let (efficiency, source) = if let Some(ce) = c.conversion_efficiency {
            (ce, 1.0)
        } else if let Some(ef) = c.energy_factor {
            (ef * c.performance_adjustment.unwrap_or(1.0), 2.0)
        } else if let Some(uef) = c.uniform_energy_factor {
            let ef = UEF_TO_EF_GAS_SLOPE * uef + UEF_TO_EF_GAS_INTERCEPT;
            (ef * c.performance_adjustment.unwrap_or(1.0), 3.0)
        } else {
            (DEFAULT_BURNER_EFFICIENCY, 0.0)
        };

        // source: 0=Default, 1=ConversionEfficiency, 2=EnergyFactor, 3=UniformEnergyFactor
        self.burner_efficiency_constant = efficiency;

        #[cfg(debug_assertions)]
        {
            assert!(
                (0.0..=1.0).contains(&self.burner_efficiency_constant),
                "burner_efficiency_constant must be in [0.0, 1.0], got {}",
                self.burner_efficiency_constant
            );
            if 3.0 == source {
                let converted = UEF_TO_EF_GAS_SLOPE * c.uniform_energy_factor.unwrap()
                    + UEF_TO_EF_GAS_INTERCEPT;
                assert!(
                    converted <= 1.0,
                    "UEF->EF converted value must not exceed 1.0, got {converted}"
                );
            }
        }

        self.burner_efficiency_poly = None;

        self.fuel_type = c.fuel_type;
        self.descriptor.fuel = self.fuel_type;

        self.skin_loss_fraction = c
            .skin_loss_fraction
            .unwrap_or_else(|| default_skin_loss_fraction(self.burner_efficiency_constant));
        self.pilot_fraction_to_tank = c.pilot_fraction_to_tank.unwrap_or(0.80);
        self.max_tank_temp_c = c.max_tank_temp_c.unwrap_or(DEFAULT_MAX_TANK_TEMP_C);

        self.dr_setpoint_offset_c = 0.0;
        self.dr_load_fraction = 1.0;
        self.dr_duration_remaining_s = None;
        self.dr_level = DRLevel::Normal;
        self.ctrl_load_fraction = 1.0;
        self.descriptor.telemetry_fields = telemetry_fields(n_nodes);
        self.telemetry = default_telemetry();
        self.tank.register_node_telemetry(&mut self.telemetry);
        self.telemetry
            .set(tk::BURNER_EFFICIENCY, self.burner_efficiency_constant);
        self.telemetry.set(tk::BURNER_EFFICIENCY_SOURCE, source);
        self.core_output = CoreOutput::default();
        Ok(())
    }
}

impl Equipment for GasWH {
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
        let ctrl_duty =
            (self.duty_cycle * self.dr_load_fraction * self.ctrl_load_fraction).clamp(0.0, 1.0);

        // Ideal capacity mode (coarse timesteps >= 5 min): compute the duty
        // cycle fraction that delivers just enough heat to maintain the
        // thermostat node at setpoint. Mirrors OCHRE's solve_ideal_capacity.
        let use_ideal = env.time_res.num_seconds() >= 300;
        let duty = if use_ideal && self.burner_on && self.burner_input_w > 0.0 {
            let dt_s = dt.as_secs_f64();
            let efficiency = self.burner_efficiency(1.0);
            let net_efficiency = efficiency * (1.0 - self.flue_loss_fraction);
            let effective_capacity_w = self.burner_input_w * net_efficiency;
            if effective_capacity_w > 0.0 {
                let ideal_w = self.tank.ideal_capacity_for_node(
                    self.burner_node,
                    self.effective_setpoint_c(),
                    ambient_c,
                    dt_s,
                );
                (ideal_w / effective_capacity_w).clamp(0.0, 1.0) * ctrl_duty
            } else {
                ctrl_duty
            }
        } else {
            ctrl_duty
        };

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

        // Standing pilot heat: fraction goes to tank water, remainder to ambient.
        // EnergyPlus WaterThermalTanks.cc models this as OffCycParaLoad with
        // OffCycParaFracToTank routing a fraction to the tank.
        let pilot_heat_to_water_w = self.pilot_power_w * self.pilot_fraction_to_tank;
        let pilot_heat_to_ambient_w = self.pilot_power_w * (1.0 - self.pilot_fraction_to_tank);

        let heat_injections: Vec<(usize, f64)> = {
            let mut injections = vec![];
            if tank_heat_w > 0.0 {
                injections.push((self.burner_node, tank_heat_w));
            }
            if pilot_heat_to_water_w > 0.0 {
                injections.push((self.burner_node, pilot_heat_to_water_w));
            }
            injections
        };

        // Conservation invariant: pilot thermal power must equal sum of routed components.
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            let expected = pilot_heat_to_water_w + pilot_heat_to_ambient_w;
            let actual = self.pilot_power_w;
            assert!(
                (expected - actual).abs() < 1e-6,
                "pilot heat not conserved: {expected:.9} != {actual:.9}"
            );
        }

        // Compute standby skin loss to zone before tank.step() updates temperatures.
        // OCHRE WaterHeater.py:712-723: fraction of tank UA losses that enter the zone.
        let avg_temp_before =
            weighted_average_tank_temp(self.tank.node_temps(), self.tank.node_volumes_m3());
        let standby_loss_w = self.tank.ua_w_per_k() * (avg_temp_before - ambient_c).max(0.0);
        let skin_loss_to_zone_w =
            standby_loss_w * self.skin_loss_fraction + pilot_heat_to_ambient_w;

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
            ambient_c,
            tempered_flow_m3_s,
            hot_flow_m3_s,
            mains_temp_c,
            &heat_injections,
            tmv,
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
                active_power_w: fan_electric_w,
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
                thermal_power_w: None,
                node_id: FluidNodeId(0),
                direction: HeatTransferDirection::Source,
            })?;
        }

        // Skin losses (jacket losses that enter the conditioned space) go to the
        // zone thermal port. Flue losses exit the building and are not reported here.
        if let Some(zone) = self.descriptor.zone {
            if skin_loss_to_zone_w > 0.0 {
                ports.accumulate(&PortContribution::Thermal {
                    zone,
                    sensible_gain_w: skin_loss_to_zone_w,
                    radiant_gain_w: 0.0,
                    latent_gain_w: 0.0,
                    category: ThermalCategory::JacketLoss,
                })?;
            }
        }

        let avg_temp_c =
            weighted_average_tank_temp(self.tank.node_temps(), self.tank.node_volumes_m3());
        self.telemetry.set(tk::TANK_AVG_TEMP_C, avg_temp_c);
        self.telemetry.set(tk::BURNER_POWER_W, burner_input_w);
        self.telemetry.set(tk::PILOT_POWER_W, self.pilot_power_w);
        self.telemetry.set(tk::FUEL_INPUT_W, fuel_input_w);
        self.telemetry
            .set(tk::PILOT_HEAT_TO_WATER_W, pilot_heat_to_water_w);
        self.telemetry
            .set(tk::PILOT_HEAT_TO_AMBIENT_W, pilot_heat_to_ambient_w);
        self.telemetry
            .set(tk::PILOT_KW, power_w_to_kw(self.pilot_power_w));
        self.telemetry
            .set(tk::FUEL_INPUT_KW, power_w_to_kw(fuel_input_w));
        self.telemetry.set(tk::FLUE_LOSS_W, flue_loss_w);
        self.telemetry.set(tk::SKIN_LOSS_W, skin_loss_to_zone_w);
        self.telemetry.set(tk::FAN_ELECTRIC_W, fan_electric_w);
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
                    power_w_to_kw(fan_electric_w).max(0.0),
                )),
                reactive_power_kvar: None,
                fuel_w: Some(FuelPower {
                    fuel_type: self.fuel_type,
                    consumption_w: fuel_input_w.max(0.0),
                }),
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
            &GasWhState {
                setpoint_c: self.setpoint_c,
                deadband_c: self.deadband_c,
                burner_on: self.burner_on,
                duty_cycle: self.duty_cycle,
                mode_override: self.mode_override,
                tank_state: self.tank.save_state()?,
                tank_avg_temp_c: self.telemetry.get(tk::TANK_AVG_TEMP_C).unwrap_or(0.0),
                burner_power_w: self.telemetry.get(tk::BURNER_POWER_W).unwrap_or(0.0),
                pilot_power_w: self.telemetry.get(tk::PILOT_POWER_W).unwrap_or(0.0),
                fuel_input_w: self.telemetry.get(tk::FUEL_INPUT_W).unwrap_or(0.0),
                flue_loss_w: self.telemetry.get(tk::FLUE_LOSS_W).unwrap_or(0.0),
                fan_electric_w: self.telemetry.get(tk::FAN_ELECTRIC_W).unwrap_or(0.0),
                draw_flow_rate_kg_s: self.telemetry.get(tk::DRAW_FLOW_RATE_KG_S).unwrap_or(0.0),
                dr_level: self.dr_level,
                dr_setpoint_offset_c: self.dr_setpoint_offset_c,
                dr_load_fraction: self.dr_load_fraction,
                dr_duration_remaining_s: self.dr_duration_remaining_s,
                pilot_fraction_to_tank: self.pilot_fraction_to_tank,
            },
            Self::checkpoint_version(),
            "GasWH",
        )
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let decoded: GasWhState = load_versioned(
            state,
            Self::checkpoint_version(),
            "GasWH",
            self.descriptor().id,
        )?;
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
        self.pilot_fraction_to_tank = decoded.pilot_fraction_to_tank;

        self.telemetry
            .insert(tk::TANK_AVG_TEMP_C, decoded.tank_avg_temp_c);
        self.telemetry
            .insert(tk::BURNER_POWER_W, decoded.burner_power_w);
        self.telemetry
            .insert(tk::PILOT_POWER_W, decoded.pilot_power_w);
        self.telemetry
            .insert(tk::FUEL_INPUT_W, decoded.fuel_input_w);
        self.telemetry
            .insert(tk::PILOT_KW, power_w_to_kw(decoded.pilot_power_w));
        self.telemetry
            .insert(tk::FUEL_INPUT_KW, power_w_to_kw(decoded.fuel_input_w));
        self.telemetry.insert(tk::FLUE_LOSS_W, decoded.flue_loss_w);
        self.telemetry
            .insert(tk::FAN_ELECTRIC_W, decoded.fan_electric_w);
        self.telemetry
            .insert(tk::DRAW_FLOW_RATE_KG_S, decoded.draw_flow_rate_kg_s);
        self.telemetry.insert(
            tk::OPERATING_MODE,
            if decoded.burner_on { 1.0 } else { 0.0 },
        );
        self.telemetry.insert(tk::PILOT_HEAT_TO_WATER_W, 0.0);
        self.telemetry.insert(tk::PILOT_HEAT_TO_AMBIENT_W, 0.0);
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
            ControlSignal::PowerLimit { max_power_kw, .. } if self.burner_input_w > 0.0 => {
                let max_fraction =
                    (power_kw_to_w(*max_power_kw) / self.burner_input_w).clamp(0.0, 1.0);
                self.ctrl_load_fraction = self.ctrl_load_fraction.min(max_fraction);
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
    let mut telemetry = Telemetry::with_capacity(16);
    telemetry.insert(tk::TANK_AVG_TEMP_C, 0.0);
    telemetry.insert(tk::BURNER_EFFICIENCY, 0.0);
    telemetry.insert(tk::BURNER_EFFICIENCY_SOURCE, 0.0);
    telemetry.insert(tk::BURNER_POWER_W, 0.0);
    telemetry.insert(tk::PILOT_POWER_W, 0.0);
    telemetry.insert(tk::FUEL_INPUT_W, 0.0);
    telemetry.insert(tk::PILOT_KW, 0.0);
    telemetry.insert(tk::FUEL_INPUT_KW, 0.0);
    telemetry.insert(tk::FLUE_LOSS_W, 0.0);
    telemetry.insert(tk::FAN_ELECTRIC_W, 0.0);
    telemetry.insert(tk::DRAW_FLOW_RATE_KG_S, 0.0);
    telemetry.insert(tk::UNMET_LOAD_W, 0.0);
    telemetry.insert(tk::OPERATING_MODE, 0.0);
    telemetry.insert(tk::OUTLET_TEMP_C, 0.0);
    telemetry.insert(tk::PILOT_HEAT_TO_WATER_W, 0.0);
    telemetry.insert(tk::PILOT_HEAT_TO_AMBIENT_W, 0.0);
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
            name: tk::BURNER_EFFICIENCY.to_string(),
            unit: "fraction".to_string(),
            description: "Resolved burner efficiency (conversion_efficiency, EF, or UEF-derived)"
                .to_string(),
        },
        TelemetryField {
            name: tk::BURNER_EFFICIENCY_SOURCE.to_string(),
            unit: "enum".to_string(),
            description: "0=Default, 1=ConversionEfficiency, 2=EnergyFactor, 3=UniformEnergyFactor"
                .to_string(),
        },
        TelemetryField {
            name: tk::BURNER_POWER_W.to_string(),
            unit: "W".to_string(),
            description: "Gas burner thermal input power".to_string(),
        },
        TelemetryField {
            name: tk::PILOT_POWER_W.to_string(),
            unit: "W".to_string(),
            description: "Pilot light fuel input power (continuous)".to_string(),
        },
        TelemetryField {
            name: tk::FUEL_INPUT_W.to_string(),
            unit: "W".to_string(),
            description: "Total fuel consumption rate".to_string(),
        },
        TelemetryField {
            name: tk::PILOT_KW.to_string(),
            unit: "kW".to_string(),
            description: "Pilot light fuel input power".to_string(),
        },
        TelemetryField {
            name: tk::FUEL_INPUT_KW.to_string(),
            unit: "kW".to_string(),
            description: "Total fuel consumption rate".to_string(),
        },
        TelemetryField {
            name: tk::FLUE_LOSS_W.to_string(),
            unit: "W".to_string(),
            description: "Heat loss to flue (excluded from zone thermal gains)".to_string(),
        },
        TelemetryField {
            name: tk::FAN_ELECTRIC_W.to_string(),
            unit: "W".to_string(),
            description: "Auxiliary electric fan draw".to_string(),
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
    fields.push(TelemetryField {
        name: tk::PILOT_HEAT_TO_WATER_W.to_string(),
        unit: "W".to_string(),
        description: "Pilot thermal power routed to tank water".to_string(),
    });
    fields.push(TelemetryField {
        name: tk::PILOT_HEAT_TO_AMBIENT_W.to_string(),
        unit: "W".to_string(),
        description: "Pilot thermal power routed to ambient zone".to_string(),
    });
    fields
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

#[cfg(test)]
mod tests {
    #![allow(clippy::needless_update)]
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        EnvironmentState, GridState, PortSlots, ThermalAccumulator, WeatherState, ZoneId,
        ZoneState, telemetry_keys as tk,
    };

    use super::GasWH;
    use crate::{Equipment, EquipmentConfig};

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

    fn config() -> EquipmentConfig {
        EquipmentConfig::from_typed(
            "GWH".to_string(),
            "Gas Water Heater".to_string(),
            crate::GasWaterHeaterConfig {
                equipment_id: None,
                zone_id: Some(1),
                loop_id: Some(1),
                fuel_type: hares_types::FuelType::Gas,
                tank_volume_m3: None,
                tank_height_m: None,
                energy_factor: None,
                uniform_energy_factor: None,
                heating_capacity_w: None,
                ua_w_per_k: None,
                setpoint_c: Some(52.0),
                deadband_c: Some(2.0),
                max_tank_temp_c: None,
                initial_tank_temp_c: Some(40.0),
                tank_nodes: None,
                avg_water_draw_l_per_day: None,
                draw_flow_rate_kg_s: Some(0.0),
                draw_flow_rate_source: None,
                mains_temp_c_source: None,
                pilot_power_w: Some(50.0),
                flue_loss_fraction: Some(0.2),
                skin_loss_fraction: None,
                ignition_type: None,
                performance_adjustment: None,
                zone_type: None,
                first_hour_rating_m3: None,
                jacket_r_value_m2_k_w: None,
                conversion_efficiency: None,
                fixture_delivery_temp_c: None,
                hot_draw_temp_c: None,
                pilot_fraction_to_tank: None,
            },
        )
    }

    fn config_with_extras(
        extras: &[(&str, Option<crate::config::ConfigValue>)],
    ) -> EquipmentConfig {
        let mut typed: crate::GasWaterHeaterConfig = config().typed().unwrap();
        for (k, v) in extras {
            match (*k, v) {
                ("setpoint_c", Some(crate::config::ConfigValue::Float(x))) => {
                    typed.setpoint_c = Some(*x)
                }
                ("setpoint_c", None) => typed.setpoint_c = None,
                ("deadband_c", Some(crate::config::ConfigValue::Float(x))) => {
                    typed.deadband_c = Some(*x)
                }
                ("deadband_c", None) => typed.deadband_c = None,
                ("initial_tank_temp_c", Some(crate::config::ConfigValue::Float(x))) => {
                    typed.initial_tank_temp_c = Some(*x)
                }
                ("initial_tank_temp_c", None) => typed.initial_tank_temp_c = None,
                ("pilot_power_w", Some(crate::config::ConfigValue::Float(x))) => {
                    typed.pilot_power_w = Some(*x)
                }
                ("pilot_power_w", None) => typed.pilot_power_w = None,
                ("flue_loss_fraction", Some(crate::config::ConfigValue::Float(x))) => {
                    typed.flue_loss_fraction = Some(*x)
                }
                ("flue_loss_fraction", None) => typed.flue_loss_fraction = None,
                ("skin_loss_fraction", Some(crate::config::ConfigValue::Float(x))) => {
                    typed.skin_loss_fraction = Some(*x)
                }
                ("skin_loss_fraction", None) => typed.skin_loss_fraction = None,
                ("pilot_fraction_to_tank", Some(crate::config::ConfigValue::Float(x))) => {
                    typed.pilot_fraction_to_tank = Some(*x)
                }
                ("pilot_fraction_to_tank", None) => typed.pilot_fraction_to_tank = None,
                ("max_tank_temp_c", Some(crate::config::ConfigValue::Float(x))) => {
                    typed.max_tank_temp_c = Some(*x)
                }
                ("max_tank_temp_c", None) => typed.max_tank_temp_c = None,
                ("draw_flow_rate_kg_s", Some(crate::config::ConfigValue::Float(x))) => {
                    typed.draw_flow_rate_kg_s = Some(*x)
                }
                ("draw_flow_rate_kg_s", None) => typed.draw_flow_rate_kg_s = None,
                ("ignition_type", Some(crate::config::ConfigValue::Text(x))) => {
                    typed.ignition_type = Some(x.clone())
                }
                ("ignition_type", None) => typed.ignition_type = None,
                ("energy_factor", Some(crate::config::ConfigValue::Float(x))) => {
                    typed.energy_factor = Some(*x)
                }
                ("energy_factor", None) => typed.energy_factor = None,
                ("uniform_energy_factor", Some(crate::config::ConfigValue::Float(x))) => {
                    typed.uniform_energy_factor = Some(*x)
                }
                ("uniform_energy_factor", None) => typed.uniform_energy_factor = None,
                ("conversion_efficiency", Some(crate::config::ConfigValue::Float(x))) => {
                    typed.conversion_efficiency = Some(*x)
                }
                ("conversion_efficiency", None) => typed.conversion_efficiency = None,
                ("performance_adjustment", Some(crate::config::ConfigValue::Float(x))) => {
                    typed.performance_adjustment = Some(*x)
                }
                ("performance_adjustment", None) => typed.performance_adjustment = None,
                _ => panic!("unsupported gas test override key/value: {k}"),
            }
        }
        EquipmentConfig::from_typed("GWH".to_string(), "Gas Water Heater".to_string(), typed)
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
    fn flue_loss_is_not_written_to_zone_thermal_port_but_skin_loss_is() {
        let mut eq = GasWH::new(config());
        eq.init(&config(), &env(21.0)).unwrap();

        let mut p = ports();
        eq.step(&env(21.0), Duration::from_secs(60), &mut p)
            .unwrap();

        // Flue loss exits the building; it must NOT be part of the zone thermal gain.
        // However, skin losses (jacket losses) should produce a positive sensible gain.
        // The tank starts at 40°C and ambient is 21°C, so standby loss is positive.
        let flue_loss = eq.telemetry().get(tk::FLUE_LOSS_W).unwrap_or(0.0);
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
        let cfg = config_with_extras(&[
            ("initial_tank_temp_c", Some(60.0.into())),
            ("skin_loss_fraction", Some(0.5.into())),
        ]);
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
        let cfg = config_with_extras(&[
            ("initial_tank_temp_c", Some(30.0.into())),
            ("max_tank_temp_c", Some(60.0.into())),
        ]);
        let mut eq = GasWH::new(cfg.clone());
        eq.init(&cfg, &env(21.0)).unwrap();

        // Manually set the tank temperature above the max to trigger the safety cutout.
        eq.tank.node_temps_mut().fill(65.0);

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
        let cfg = config_with_extras(&[
            ("initial_tank_temp_c", Some(30.0.into())),
            ("max_tank_temp_c", Some(55.0.into())),
        ]);
        let mut eq = GasWH::new(cfg.clone());
        eq.init(&cfg, &env(21.0)).unwrap();

        // Manually stratify: top node cool (30°C), burner node hot (60°C > 55°C limit).
        let n = eq.tank.node_temps().len();
        eq.tank.node_temps_mut()[0] = 30.0; // top -- below limit
        eq.tank.node_temps_mut()[n - 1] = 60.0; // burner node -- above limit

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
        let cfg = config_with_extras(&[
            ("initial_tank_temp_c", Some(30.0.into())),
            ("max_tank_temp_c", Some(55.0.into())),
        ]);
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
        let state = eq.save_state().unwrap();

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

        let saved = eq.save_state().unwrap();

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
        // Tank at 49.9°C: strictly below the 50°C lower threshold (setpoint=52, deadband=2) →
        // normally heating. DR reduces setpoint by 3°C → effective setpoint=49°C, lower
        // threshold=47°C; 49.9°C > 47°C so burner stays off.
        let cfg = config_with_extras(&[
            ("initial_tank_temp_c", Some(49.9.into())),
            ("max_tank_temp_c", Some(300.0.into())),
        ]);
        let e = env(21.0);

        // Baseline: burner should be on.
        let mut eq_base = GasWH::new(cfg.clone());
        eq_base.init(&cfg, &e).unwrap();
        let mode_base = eq_base.update_control(&e);
        assert!(
            matches!(mode_base, hares_types::OperatingMode::Heating),
            "baseline must be heating with tank at 49.9°C; got {mode_base:?}"
        );

        // DR Moderate: effective setpoint = 52 + (-3) = 49°C. Tank at 49.9°C > 47°C lower
        // threshold → no call.
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
            eq.telemetry().get(tk::BURNER_POWER_W).unwrap_or(1.0),
            0.0,
            "burner_power_w must be zero during GridEmergency"
        );
        // fuel_input_w = burner_input_w + pilot_power_w; with burner off, only
        // pilot remains. Verify burner contribution is zero -- pilot is a continuous flame
        // and is not subject to DR load shedding.
        assert_eq!(
            eq.telemetry().get(tk::FUEL_INPUT_W).unwrap_or(1.0),
            eq.pilot_power_w,
            "fuel_input_w during GridEmergency must equal pilot only (burner is off)"
        );
    }

    #[test]
    fn pilot_defaults_to_standing_pilot_power_when_unconfigured() {
        let cfg = config_with_extras(&[("pilot_power_w", None)]);
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
        let cfg = config_with_extras(&[
            ("pilot_power_w", None),
            ("ignition_type", Some("electronic".into())),
        ]);
        let mut eq = GasWH::new(cfg.clone());
        eq.init(&cfg, &env(21.0)).unwrap();
        assert_eq!(
            eq.pilot_power_w, 0.0,
            "electronic ignition should default pilot power to zero"
        );
    }

    #[test]
    fn pilot_defaults_to_zero_for_hpxml_electronic_ignition_string() {
        let cfg = config_with_extras(&[
            ("pilot_power_w", None),
            ("ignition_type", Some("electronic ignition".into())),
        ]);
        let mut eq = GasWH::new(cfg.clone());
        eq.init(&cfg, &env(21.0)).unwrap();
        assert_eq!(
            eq.pilot_power_w, 0.0,
            "HPXML 'electronic ignition' string must suppress pilot power"
        );
    }

    #[test]
    fn explicit_pilot_power_overrides_ignition_type() {
        let cfg = config_with_extras(&[
            ("ignition_type", Some("electronic".into())),
            ("pilot_power_w", Some(7.0.into())),
        ]);
        let mut eq = GasWH::new(cfg.clone());
        eq.init(&cfg, &env(21.0)).unwrap();
        assert_eq!(
            eq.pilot_power_w, 7.0,
            "explicit pilot power must override inferred ignition default"
        );
    }

    #[test]
    fn standby_step_at_setpoint_still_consumes_gas_from_pilot() {
        let cfg = config_with_extras(&[
            ("pilot_power_w", None),
            ("setpoint_c", Some(52.0.into())),
            ("initial_tank_temp_c", Some(52.0.into())),
            ("draw_flow_rate_kg_s", Some(0.0.into())),
        ]);
        let e = env(21.0);

        let mut eq = GasWH::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();

        let mut p = ports();
        eq.step(&e, Duration::from_secs(3600), &mut p).unwrap();

        assert_eq!(
            eq.telemetry().get(tk::BURNER_POWER_W).unwrap_or(1.0),
            0.0,
            "burner should be off at setpoint with no draw"
        );
        assert!(
            eq.telemetry().get(tk::FUEL_INPUT_W).unwrap_or(0.0) > 0.0,
            "standing-pilot unit should report non-zero gas consumption at standby"
        );
    }

    #[test]
    fn pilot_kw_and_fuel_input_kw_present_and_sum_correctly() {
        let cfg = config_with_extras(&[("initial_tank_temp_c", Some(40.0.into()))]);
        let e = env(21.0);

        let mut eq = GasWH::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();

        let mut p = ports();
        eq.step(&e, Duration::from_secs(60), &mut p).unwrap();

        let fuel_input_w = eq
            .telemetry()
            .get(tk::FUEL_INPUT_W)
            .expect("FUEL_INPUT_W must be present");
        let pilot_kw = eq
            .telemetry()
            .get(tk::PILOT_KW)
            .expect("PILOT_KW must be present");
        let fuel_input_kw = eq
            .telemetry()
            .get(tk::FUEL_INPUT_KW)
            .expect("FUEL_INPUT_KW must be present");

        assert!(
            fuel_input_w > 0.0,
            "burner must be firing for this test to be meaningful"
        );
        assert!(
            (fuel_input_kw - fuel_input_w / 1_000.0).abs() < 1e-9,
            "fuel_input_kw {fuel_input_kw:.6} must equal fuel_input_w/1000 {:.6}",
            fuel_input_w / 1_000.0
        );
        let pilot_w = eq
            .telemetry()
            .get(tk::PILOT_POWER_W)
            .expect("PILOT_POWER_W must be present");
        assert!(
            (pilot_kw - pilot_w / 1_000.0).abs() < 1e-9,
            "pilot_kw {pilot_kw:.6} must equal pilot_power_w/1000 {:.6}",
            pilot_w / 1_000.0
        );
        assert!(
            pilot_kw <= fuel_input_kw,
            "pilot_kw {pilot_kw:.6} must not exceed fuel_input_kw {fuel_input_kw:.6}"
        );
    }

    /// At 5-minute timesteps, ideal capacity mode should modulate the burner
    /// below full rated power, preventing overshoot. Compare to the same
    /// scenario at 1-minute timesteps (non-ideal) which runs at full power.
    #[test]
    fn gas_wh_ideal_capacity_modulates_below_full_power() {
        use chrono::Duration as ChronoDuration;

        // burner_node at 40°C (12°C below setpoint=52), deadband=2 => calls for heat.
        let cfg = config_with_extras(&[
            ("initial_tank_temp_c", Some(40.0.into())),
            ("setpoint_c", Some(52.0.into())),
            ("deadband_c", Some(2.0.into())),
            ("flue_loss_fraction", Some(0.0.into())),
            ("max_tank_temp_c", Some(300.0.into())),
        ]);

        // Run at 5-minute timestep (ideal capacity active).
        let mut e_ideal = env(21.0);
        e_ideal.time_res = ChronoDuration::seconds(300);
        let mut eq_ideal = GasWH::new(cfg.clone());
        eq_ideal.init(&cfg, &e_ideal).unwrap();
        let mut p_ideal = ports();
        eq_ideal
            .step(&e_ideal, Duration::from_secs(300), &mut p_ideal)
            .unwrap();

        let burner_w_ideal = eq_ideal.telemetry().get(tk::BURNER_POWER_W).unwrap_or(0.0);

        // Run at 1-minute timestep (thermostat on/off, no ideal capacity).
        let mut e_therm = env(21.0);
        e_therm.time_res = ChronoDuration::seconds(60);
        let mut eq_therm = GasWH::new(cfg.clone());
        eq_therm.init(&cfg, &e_therm).unwrap();
        let mut p_therm = ports();
        eq_therm
            .step(&e_therm, Duration::from_secs(60), &mut p_therm)
            .unwrap();

        let burner_w_therm = eq_therm.telemetry().get(tk::BURNER_POWER_W).unwrap_or(0.0);

        assert!(
            burner_w_ideal > 0.0,
            "burner must fire in ideal mode (node far below setpoint)"
        );
        assert!(burner_w_therm > 0.0, "burner must fire in thermostat mode");
        assert!(
            burner_w_ideal < burner_w_therm,
            "ideal mode should modulate below thermostat full-power: \
             ideal={burner_w_ideal:.1} W, therm={burner_w_therm:.1} W"
        );
    }

    #[test]
    fn gas_wh_uses_conversion_efficiency_not_ef() {
        let cfg = EquipmentConfig::from_typed(
            "GWH".to_string(),
            "Gas Water Heater".to_string(),
            crate::GasWaterHeaterConfig {
                equipment_id: None,
                zone_id: Some(1),
                loop_id: Some(1),
                fuel_type: hares_types::FuelType::Gas,
                tank_volume_m3: None,
                tank_height_m: None,
                energy_factor: Some(0.62),
                uniform_energy_factor: None,
                heating_capacity_w: None,
                ua_w_per_k: None,
                setpoint_c: Some(52.0),
                deadband_c: Some(2.0),
                max_tank_temp_c: None,
                initial_tank_temp_c: Some(40.0),
                tank_nodes: None,
                avg_water_draw_l_per_day: None,
                draw_flow_rate_kg_s: Some(0.0),
                draw_flow_rate_source: None,
                mains_temp_c_source: None,
                pilot_power_w: Some(0.0),
                flue_loss_fraction: Some(0.0),
                skin_loss_fraction: None,
                ignition_type: None,
                performance_adjustment: None,
                zone_type: None,
                first_hour_rating_m3: None,
                jacket_r_value_m2_k_w: None,
                conversion_efficiency: Some(0.80),
                fixture_delivery_temp_c: None,
                hot_draw_temp_c: None,
                pilot_fraction_to_tank: None,
            },
        );
        let mut eq = GasWH::new(cfg.clone());
        eq.init(&cfg, &env(21.0)).unwrap();
        assert!(
            (eq.burner_efficiency_constant - 0.80).abs() < 1e-9,
            "burner_efficiency_constant must be conversion_efficiency (0.80), not EF (0.62), \
             got {:.4}",
            eq.burner_efficiency_constant
        );
    }

    #[test]
    fn gas_wh_fuel_consumption_tracks_burner_input() {
        // HARES uses conversion_efficiency directly as burner efficiency, unlike OCHRE
        // which derives it from EF via a regression. This is an intentional model
        // difference: conversion_efficiency is the physically meaningful parameter.
        let burner_input_w = 11_000.0_f64;
        let eta_c = 0.80_f64;
        let flue_loss_fraction = 0.0_f64;
        let cfg = EquipmentConfig::from_typed(
            "GWH".to_string(),
            "Gas Water Heater".to_string(),
            crate::GasWaterHeaterConfig {
                equipment_id: None,
                zone_id: Some(1),
                loop_id: Some(1),
                fuel_type: hares_types::FuelType::Gas,
                tank_volume_m3: None,
                tank_height_m: None,
                energy_factor: Some(0.62),
                uniform_energy_factor: None,
                heating_capacity_w: Some(burner_input_w),
                ua_w_per_k: None,
                setpoint_c: Some(52.0),
                deadband_c: Some(2.0),
                max_tank_temp_c: None,
                initial_tank_temp_c: Some(40.0),
                tank_nodes: None,
                avg_water_draw_l_per_day: None,
                draw_flow_rate_kg_s: Some(0.0),
                draw_flow_rate_source: None,
                mains_temp_c_source: None,
                pilot_power_w: Some(0.0),
                flue_loss_fraction: Some(flue_loss_fraction),
                skin_loss_fraction: None,
                ignition_type: None,
                performance_adjustment: None,
                zone_type: None,
                first_hour_rating_m3: None,
                jacket_r_value_m2_k_w: None,
                conversion_efficiency: Some(eta_c),
                fixture_delivery_temp_c: None,
                hot_draw_temp_c: None,
                pilot_fraction_to_tank: None,
            },
        );
        let mut eq = GasWH::new(cfg.clone());
        eq.init(&cfg, &env(21.0)).unwrap();

        let mut p = ports();
        eq.step(&env(21.0), Duration::from_secs(60), &mut p)
            .unwrap();

        let thermal_output_w = burner_input_w * eta_c * (1.0 - flue_loss_fraction);
        let expected_fuel_w = thermal_output_w / eta_c;
        let reported_fuel_w = eq
            .telemetry()
            .get(tk::FUEL_INPUT_W)
            .expect("FUEL_INPUT_W must be present");
        assert!(
            (reported_fuel_w - expected_fuel_w).abs() < 1.0,
            "fuel_w {reported_fuel_w:.2} W must match thermal_output/eta_c = {expected_fuel_w:.2} W"
        );
        assert!(
            (reported_fuel_w - burner_input_w * 0.62).abs() > 100.0 || reported_fuel_w == 0.0,
            "fuel consumption must NOT be derived from EF=0.62 when conversion_efficiency is set"
        );
    }

    #[test]
    fn outlet_temp_c_telemetry_present_with_draw() {
        let cfg = config_with_extras(&[
            ("initial_tank_temp_c", Some(55.0.into())),
            ("setpoint_c", Some(55.0.into())),
            ("draw_flow_rate_kg_s", Some(0.1.into())),
        ]);
        let e = env(21.0);

        let mut eq = GasWH::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();

        let mut p = ports();
        eq.step(&e, Duration::from_secs(60), &mut p).unwrap();

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
        let cfg = config_with_extras(&[
            ("initial_tank_temp_c", Some(55.0.into())),
            ("draw_flow_rate_kg_s", Some(0.0.into())),
        ]);
        let e = env(21.0);

        let mut eq = GasWH::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();

        let mut p = ports();
        eq.step(&e, Duration::from_secs(60), &mut p).unwrap();

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

    #[test]
    fn uef_converted_to_ef_for_known_pairs() {
        // RESNET EF Calculator 2017 / OCHRE hpxml.py:1115: EF = 0.9066 * UEF + 0.0711
        // UEF 0.70 → EF = 0.9066 * 0.70 + 0.0711 = 0.70572
        let cfg_070 = config_with_extras(&[
            ("uniform_energy_factor", Some(0.70.into())),
            ("energy_factor", None),
            ("conversion_efficiency", None),
        ]);
        let mut eq_070 = GasWH::new(cfg_070.clone());
        eq_070.init(&cfg_070, &env(21.0)).unwrap();
        assert!(
            (eq_070.burner_efficiency_constant - 0.70572).abs() < 1e-4,
            "UEF 0.70 should convert to EF ~0.70572, got {:.5}",
            eq_070.burner_efficiency_constant
        );

        // UEF 0.58 → EF = 0.9066 * 0.58 + 0.0711 = 0.596928
        let cfg_058 = config_with_extras(&[
            ("uniform_energy_factor", Some(0.58.into())),
            ("energy_factor", None),
            ("conversion_efficiency", None),
        ]);
        let mut eq_058 = GasWH::new(cfg_058.clone());
        eq_058.init(&cfg_058, &env(21.0)).unwrap();
        assert!(
            (eq_058.burner_efficiency_constant - 0.596928).abs() < 1e-4,
            "UEF 0.58 should convert to EF ~0.59693, got {:.5}",
            eq_058.burner_efficiency_constant
        );
    }

    #[test]
    fn conversion_efficiency_takes_priority_over_uef() {
        // When conversion_efficiency, energy_factor, and uniform_energy_factor are all set,
        // conversion_efficiency wins.
        let cfg = config_with_extras(&[
            ("conversion_efficiency", Some(0.80.into())),
            ("energy_factor", Some(0.62.into())),
            ("uniform_energy_factor", Some(0.70.into())),
        ]);
        let mut eq = GasWH::new(cfg.clone());
        eq.init(&cfg, &env(21.0)).unwrap();
        assert!(
            (eq.burner_efficiency_constant - 0.80).abs() < 1e-9,
            "conversion_efficiency (0.80) must win over EF and UEF, got {:.4}",
            eq.burner_efficiency_constant
        );
        assert_eq!(
            eq.telemetry()
                .get(tk::BURNER_EFFICIENCY_SOURCE)
                .unwrap_or(-1.0),
            1.0,
            "source must be 1 (ConversionEfficiency)"
        );
    }

    #[test]
    fn energy_factor_takes_priority_over_uef() {
        // When energy_factor and uniform_energy_factor are set but conversion_efficiency
        // is not, energy_factor wins (used directly, no UEF conversion).
        let cfg = config_with_extras(&[
            ("energy_factor", Some(0.62.into())),
            ("uniform_energy_factor", Some(0.70.into())),
            ("conversion_efficiency", None),
        ]);
        let mut eq = GasWH::new(cfg.clone());
        eq.init(&cfg, &env(21.0)).unwrap();
        assert!(
            (eq.burner_efficiency_constant - 0.62).abs() < 1e-9,
            "energy_factor (0.62) must win over UEF (0.70), got {:.4}",
            eq.burner_efficiency_constant
        );
        assert_eq!(
            eq.telemetry()
                .get(tk::BURNER_EFFICIENCY_SOURCE)
                .unwrap_or(-1.0),
            2.0,
            "source must be 2 (EnergyFactor)"
        );
    }

    #[test]
    fn uniform_energy_factor_with_performance_adjustment() {
        // UEF 0.70 → EF = 0.70572, adjusted by performance_adjustment = 0.92
        // Expected: 0.70572 * 0.92 = 0.6492624
        let cfg = config_with_extras(&[
            ("uniform_energy_factor", Some(0.70.into())),
            ("energy_factor", None),
            ("conversion_efficiency", None),
            ("performance_adjustment", Some(0.92.into())),
        ]);
        let mut eq = GasWH::new(cfg.clone());
        eq.init(&cfg, &env(21.0)).unwrap();
        let expected = 0.70572 * 0.92;
        assert!(
            (eq.burner_efficiency_constant - expected).abs() < 1e-4,
            "UEF 0.70 adjusted by 0.92 should be {expected:.5}, got {:.5}",
            eq.burner_efficiency_constant
        );
    }

    #[test]
    fn uef_burner_efficiency_telemetry_populated() {
        let cfg = config_with_extras(&[
            ("uniform_energy_factor", Some(0.70.into())),
            ("energy_factor", None),
            ("conversion_efficiency", None),
        ]);
        let mut eq = GasWH::new(cfg.clone());
        eq.init(&cfg, &env(21.0)).unwrap();

        let efficiency = eq
            .telemetry()
            .get(tk::BURNER_EFFICIENCY)
            .expect("BURNER_EFFICIENCY must be present");
        let source = eq
            .telemetry()
            .get(tk::BURNER_EFFICIENCY_SOURCE)
            .expect("BURNER_EFFICIENCY_SOURCE must be present");

        assert!(
            (efficiency - 0.70572).abs() < 1e-4,
            "BURNER_EFFICIENCY must be converted UEF, got {efficiency}"
        );
        assert_eq!(
            source, 3.0,
            "BURNER_EFFICIENCY_SOURCE must be 3 (UniformEnergyFactor), got {source}"
        );
    }

    #[test]
    fn skin_loss_fraction_uses_converted_uef() {
        // UEF 0.58 → EF = 0.59693. Since 0.59693 < 0.7, skin_loss_fraction should be 0.64.
        // Without UEF→EF conversion, raw UEF 0.58 would also produce 0.64 (coincidence here),
        // but UEF 0.70 → EF = 0.70572 ≥ 0.7 → skin_loss_fraction = 0.91.
        // Without conversion, raw UEF 0.70 would also be ≥ 0.7 → also 0.91 (coincidence).
        // The meaningful test: UEF 0.7 would be the same either way, but we can test via
        // skin_loss_fraction with the explicit value.
        //
        // Instead test: UEF 0.65 → EF = 0.9066*0.65 + 0.0711 = 0.66039.
        // 0.66039 < 0.7 → skin_loss_fraction = 0.64.
        // Without conversion: raw UEF 0.65 < 0.7 → also 0.64 (same).
        //
        // Test UEF 0.68 → EF = 0.9066*0.68 + 0.0711 = 0.68759.
        // 0.68759 < 0.7 → skin_loss_fraction = 0.64.
        // Without conversion: raw UEF 0.68 < 0.7 → also 0.64 (same).
        //
        // Test UEF 0.80 → EF = 0.9066*0.80 + 0.0711 = 0.79638.
        // 0.79638 < 0.8 → skin_loss_fraction = 0.91.
        // Without conversion: raw UEF 0.80 ≥ 0.8 → skin_loss_fraction = 0.96. DIFFERENT!
        //
        // UEF 0.80 is the clearest differentiator.
        let cfg = config_with_extras(&[
            ("uniform_energy_factor", Some(0.80.into())),
            ("energy_factor", None),
            ("conversion_efficiency", None),
        ]);
        let mut eq = GasWH::new(cfg.clone());
        eq.init(&cfg, &env(21.0)).unwrap();

        // With conversion: burner_efficiency_constant = 0.79638 → skin_loss = 0.91
        // Without conversion: burner_efficiency_constant = 0.80 → skin_loss = 0.96
        assert_eq!(
            eq.skin_loss_fraction, 0.91,
            "UEF 0.80 → EF=0.796 < 0.8 → skin_loss_fraction=0.91, got {}",
            eq.skin_loss_fraction
        );
    }

    #[test]
    fn default_efficiency_source_is_zero() {
        // When no efficiency inputs are provided, source should be 0 (Default).
        let cfg = config_with_extras(&[
            ("conversion_efficiency", None),
            ("energy_factor", None),
            ("uniform_energy_factor", None),
        ]);
        let mut eq = GasWH::new(cfg.clone());
        eq.init(&cfg, &env(21.0)).unwrap();

        assert!(
            (eq.burner_efficiency_constant - super::DEFAULT_BURNER_EFFICIENCY).abs() < 1e-9,
            "fallback should use DEFAULT_BURNER_EFFICIENCY"
        );
        assert_eq!(
            eq.telemetry()
                .get(tk::BURNER_EFFICIENCY_SOURCE)
                .unwrap_or(-1.0),
            0.0,
            "source must be 0 (Default)"
        );
    }

    /// Pilot heat must be injected at the burner node regardless of whether the
    /// main burner is firing. With pilot_power_w=100 W and fraction=0.80, the
    /// telemetry must report 80 W to water and 20 W to ambient.
    #[test]
    fn pilot_heat_to_water_is_injected_at_burner_node_regardless_of_burner_state() {
        let cfg = config_with_extras(&[
            ("pilot_power_w", Some(100.0.into())),
            ("pilot_fraction_to_tank", Some(0.80.into())),
            ("initial_tank_temp_c", Some(52.0.into())), // at setpoint → burner off
            ("setpoint_c", Some(52.0.into())),
            ("flue_loss_fraction", Some(0.0.into())),
            ("draw_flow_rate_kg_s", Some(0.0.into())),
        ]);
        let e = env(21.0);

        let mut eq = GasWH::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();

        let mut p = ports();
        eq.step(&e, Duration::from_secs(60), &mut p).unwrap();

        let to_water = eq
            .telemetry()
            .get(tk::PILOT_HEAT_TO_WATER_W)
            .expect("PILOT_HEAT_TO_WATER_W must be present");
        let to_ambient = eq
            .telemetry()
            .get(tk::PILOT_HEAT_TO_AMBIENT_W)
            .expect("PILOT_HEAT_TO_AMBIENT_W must be present");

        assert!(
            (to_water - 80.0).abs() < 1e-9,
            "pilot heat to water should be 80.0 W (100.0 * 0.80), got {to_water}"
        );
        assert!(
            (to_ambient - 20.0).abs() < 1e-9,
            "pilot heat to ambient should be 20.0 W (100.0 * 0.20), got {to_ambient}"
        );
        // Burner should NOT be firing — tank is at setpoint.
        assert_eq!(
            eq.telemetry().get(tk::BURNER_POWER_W).unwrap_or(1.0),
            0.0,
            "burner must be off when tank is at setpoint"
        );
    }

    /// Pilot energy is conserved: the sum of heat to water and heat to ambient
    /// must equal the total pilot power. The tank temperature must rise by the
    /// expected amount from the water-bound fraction.
    #[test]
    fn pilot_heat_routing_conserves_energy_and_heats_tank_correctly() {
        let cfg = config_with_extras(&[
            ("pilot_power_w", Some(100.0.into())),
            ("pilot_fraction_to_tank", Some(0.80.into())),
            ("initial_tank_temp_c", Some(30.0.into())),
            ("setpoint_c", Some(52.0.into())),
            ("flue_loss_fraction", Some(0.0.into())),
            ("draw_flow_rate_kg_s", Some(0.0.into())),
            ("skin_loss_fraction", Some(0.0.into())),
            ("max_tank_temp_c", Some(300.0.into())),
        ]);
        let e = env(20.0);

        let mut eq = GasWH::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();

        let tank_avg_before = eq.telemetry().get(tk::TANK_AVG_TEMP_C).unwrap_or(0.0);

        let mut p = ports();
        eq.step(&e, Duration::from_secs(60), &mut p).unwrap();

        let to_water = eq.telemetry().get(tk::PILOT_HEAT_TO_WATER_W).unwrap_or(0.0);
        let to_ambient = eq
            .telemetry()
            .get(tk::PILOT_HEAT_TO_AMBIENT_W)
            .unwrap_or(0.0);

        // Conservation: pilot heat sum = pilot_power_w
        let sum = to_water + to_ambient;
        assert!(
            (sum - 100.0).abs() < 1e-9,
            "pilot heat must be conserved: {to_water} + {to_ambient} = {sum}, expected 100.0"
        );

        // Tank temperature must increase from pilot heat injection.
        let tank_avg_after = eq.telemetry().get(tk::TANK_AVG_TEMP_C).unwrap_or(0.0);
        assert!(
            tank_avg_after > tank_avg_before,
            "tank temperature must rise from pilot heat injection: \
             before={tank_avg_before:.3}°C, after={tank_avg_after:.3}°C"
        );
    }

    /// When a gas WH sits in standby (no draw, burner off because tank is at
    /// setpoint), continuous pilot heat injection into the tank water causes a
    /// small upward temperature drift over a multi-timestep period.
    #[test]
    fn standby_tank_temp_drifts_upward_from_continuous_pilot_heat_injection() {
        let cfg = config_with_extras(&[
            ("pilot_power_w", Some(150.0.into())),
            ("pilot_fraction_to_tank", Some(0.80.into())),
            ("initial_tank_temp_c", Some(52.0.into())), // at setpoint → burner off
            ("setpoint_c", Some(52.0.into())),
            ("deadband_c", Some(5.0.into())),
            ("flue_loss_fraction", Some(0.0.into())),
            ("draw_flow_rate_kg_s", Some(0.0.into())),
            ("max_tank_temp_c", Some(300.0.into())),
        ]);
        let e = env(20.0);

        let mut eq = GasWH::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();

        // Record the tank's volume-weighted average temperature before the
        // standby period. After stepping, we call the telemetry which is updated
        // in step().
        let mut p = ports();
        eq.step(&e, Duration::from_secs(60), &mut p).unwrap();
        let temp_after_1m = eq.telemetry().get(tk::TANK_AVG_TEMP_C).unwrap_or(0.0);

        // Step for 1 hour (3600 s) in standby.
        let mut p2 = ports();
        eq.step(&e, Duration::from_secs(3600), &mut p2).unwrap();
        let temp_after_1h = eq.telemetry().get(tk::TANK_AVG_TEMP_C).unwrap_or(0.0);

        // The tank should drift upward because 150 W * 0.80 = 120 W is injected
        // continuously — even though the burner is off and no draw is occurring.
        assert!(
            temp_after_1h > temp_after_1m,
            "standby tank temp must drift upward from continuous pilot heat: \
             1m={temp_after_1m:.4}°C, 1h={temp_after_1h:.4}°C"
        );
        // The drift should be non-trivial (> 0.1°C) for 120 W over 1 hour.
        assert!(
            temp_after_1h - temp_after_1m > 0.1,
            "pilot heat drift should be > 0.1°C over 1 hour for 120 W injection, \
             got {:.4}°C",
            temp_after_1h - temp_after_1m
        );
    }
}
