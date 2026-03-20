//! Tankless (on-demand) water heater model.

use std::borrow::Cow;
use std::time::Duration;

use hares_types::{
    ControlCapabilities, ControlSignal, DRLevel, EndUse, EnvironmentState, EquipmentDescriptor,
    EquipmentId, ExecutionStage, FluidType, FuelType, HaresError, OperatingMode, PortContribution,
    PortDeclaration, PortSlots, PortType, Telemetry, TelemetryField, ZoneId,
};
use serde::{Deserialize, Serialize};

use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_postcard, save_postcard};

use super::{WaterHeaterZip, resolve_draw_rate_kg_s, resolve_mains_temp_c};
use crate::hvac::helpers::{
    equipment_id_from_config, first_f64, parse_fuel_type, zone_id_from_config,
};

const WATER_SPECIFIC_HEAT_J_PER_KG_K: f64 = 4183.0;
const DEFAULT_SETPOINT_C: f64 = 51.666_666_7;
const DEFAULT_EF: f64 = 0.9;
/// Default rated thermal capacity (W). OCHRE uses 20 kW for tankless.
const DEFAULT_MAX_THERMAL_POWER_W: f64 = 20_000.0;
/// OCHRE/ANSI RESNET 301 parasitic electric draw for gas tankless controller (W).
/// Formula: 5.0 + 60.0 * on_time_frac; default 7.38 W = 3 bedrooms at typical usage.
const DEFAULT_GAS_PARASITIC_POWER_W: f64 = 7.38;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct TanklessState {
    setpoint_c: f64,
    duty_cycle: f64,
    mode_override: Option<OperatingMode>,
    outlet_temp_c: f64,
    thermal_output_w: f64,
    fuel_input_w: f64,
    parasitic_electric_w: f64,
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
    fuel_type: FuelType,
    setpoint_c: f64,
    efficiency_factor: f64,
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
    zip: WaterHeaterZip,
    // --- Demand response state ---
    dr_setpoint_offset_c: f64,
    dr_load_fraction: f64,
    dr_duration_remaining_s: Option<f64>,
    dr_level: DRLevel,
    // Transient load fraction from LoadFraction control signal; reset each step.
    ctrl_load_fraction: f64,
}

impl TanklessWH {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let zone = zone_id_from_config(&config).unwrap_or(ZoneId(1));
        let fuel_type = parse_tankless_fuel_type(config.get_str("FuelType"));

        let ports = build_ports(fuel_type);

        Self {
            descriptor: EquipmentDescriptor {
                id: EquipmentId(equipment_id_from_config(&config).unwrap_or(0)),
                name: config.name,
                end_use: EndUse::WaterHeating,
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
                telemetry_fields: telemetry_fields(),
            },
            ports,
            telemetry: default_telemetry(),
            fuel_type,
            setpoint_c: DEFAULT_SETPOINT_C,
            efficiency_factor: DEFAULT_EF,
            rated_thermal_power_w: DEFAULT_MAX_THERMAL_POWER_W,
            power_limit_w: None,
            parasitic_power_w: DEFAULT_GAS_PARASITIC_POWER_W,
            duty_cycle: 1.0,
            mode_override: None,
            inlet_temp_c: 15.0,
            draw_flow_rate_kg_s: 0.0,
            zip: WaterHeaterZip::default(),
            dr_setpoint_offset_c: 0.0,
            dr_load_fraction: 1.0,
            dr_duration_remaining_s: None,
            dr_level: DRLevel::Normal,
            ctrl_load_fraction: 1.0,
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

impl Equipment for TanklessWH {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, _env: &EnvironmentState) -> crate::Result<()> {
        self.fuel_type = parse_tankless_fuel_type(config.get_str("FuelType"));
        self.descriptor.fuel = self.fuel_type;
        self.ports = build_ports(self.fuel_type);

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
        self.efficiency_factor = first_f64(
            config,
            &["EnergyFactor", "UniformEnergyFactor", "efficiency_factor"],
        )
        .unwrap_or(DEFAULT_EF)
        .max(1e-6);
        // ANSI/RESNET 301 performance adjustment for tankless units (typically 0.92).
        // Applied as a derate on top of the base efficiency factor.
        if let Some(perf_adj) = first_f64(config, &["performance_adjustment"]) {
            self.efficiency_factor = (self.efficiency_factor * perf_adj.clamp(0.0, 1.0)).max(1e-6);
        }
        self.rated_thermal_power_w = first_f64(
            config,
            &["max_thermal_power_w", "RatedCapacityW", "capacity_w"],
        )
        .unwrap_or(DEFAULT_MAX_THERMAL_POWER_W)
        .max(0.0);
        self.power_limit_w = None;
        self.parasitic_power_w = first_f64(config, &["parasitic_power_w"])
            .unwrap_or(DEFAULT_GAS_PARASITIC_POWER_W)
            .max(0.0);
        self.duty_cycle = 1.0;
        self.mode_override = None;
        self.inlet_temp_c =
            first_f64(config, &["inlet_temp_c", "mains_temp_c"]).unwrap_or(self.inlet_temp_c);
        self.draw_flow_rate_kg_s = resolve_draw_rate_kg_s(config);
        self.zip = WaterHeaterZip::from_config(config);
        self.dr_setpoint_offset_c = 0.0;
        self.dr_load_fraction = 1.0;
        self.dr_duration_remaining_s = None;
        self.dr_level = DRLevel::Normal;
        self.ctrl_load_fraction = 1.0;
        self.telemetry = default_telemetry();
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
        let inlet_temp_c = resolve_mains_temp_c(env, self.inlet_temp_c);
        let delta_t_c = (setpoint_c - inlet_temp_c).max(0.0);
        let _ = dt; // dt not used for tankless (on-demand model)

        // Effective capacity: rated power, optionally clamped by PowerLimit signal.
        let effective_max_w = match self.power_limit_w {
            Some(limit) => self.rated_thermal_power_w.min(limit),
            None => self.rated_thermal_power_w,
        };

        let appliance_demand_kg_s = super::read_dhw_demand_kg_s(ports);
        let total_draw_kg_s = self.draw_flow_rate_kg_s + appliance_demand_kg_s;

        let (thermal_output_w, outlet_temp_c) = if mode == OperatingMode::Heating
            && total_draw_kg_s > 0.0
        {
            // Unclamped thermal demand to reach setpoint.
            let demand_w =
                total_draw_kg_s * WATER_SPECIFIC_HEAT_J_PER_KG_K * delta_t_c * duty;

            let capacity_w = effective_max_w * duty;

            if demand_w <= capacity_w {
                // Within capacity: deliver setpoint temperature.
                (demand_w, setpoint_c)
            } else {
                // Over-capacity: clamp time-averaged output; compute on-phase outlet temp
                // using full (non-duty-scaled) power — the heater fires at rated power
                // during its on-fraction.
                let outlet_c = inlet_temp_c
                    + effective_max_w / (total_draw_kg_s * WATER_SPECIFIC_HEAT_J_PER_KG_K);
                (capacity_w, outlet_c)
            }
        } else if mode == OperatingMode::Heating {
            // Heating mode but zero flow: no output needed.
            (0.0, setpoint_c)
        } else {
            // Off: outlet equals inlet.
            (0.0, inlet_temp_c)
        };

        let fuel_input_w = thermal_output_w / self.efficiency_factor;

        if self.fuel_type == FuelType::Electric {
            let (electric_w, reactive_kvar) = self.zip.apply(fuel_input_w, env.grid.voltage_pu);
            if electric_w > 0.0 || reactive_kvar != 0.0 {
                ports.accumulate(&PortContribution::Electrical {
                    active_power_kw: electric_w / 1_000.0,
                    reactive_power_kvar: reactive_kvar,
                })?;
            }
        } else {
            if fuel_input_w > 0.0 {
                ports.accumulate(&PortContribution::Fuel {
                    fuel_type: self.fuel_type,
                    consumption_w: fuel_input_w,
                })?;
            }
            // Gas ignition controller draws electricity continuously regardless of
            // burner state (OCHRE/ANSI RESNET 301 standby parasitic).
            let (parasitic_w, parasitic_kvar) =
                self.zip.apply(self.parasitic_power_w, env.grid.voltage_pu);
            ports.accumulate(&PortContribution::Electrical {
                active_power_kw: parasitic_w / 1_000.0,
                reactive_power_kvar: parasitic_kvar,
            })?;
        }

        self.telemetry.set("outlet_temp_c", outlet_temp_c);
        self.telemetry.set("thermal_output_w", thermal_output_w);
        self.telemetry.set("fuel_input_w", fuel_input_w);
        self.telemetry
            .set("parasitic_electric_w", self.parasitic_power_w);
        self.telemetry
            .set("draw_flow_rate_kg_s", total_draw_kg_s);
        self.telemetry.set(
            "operating_mode",
            if mode == OperatingMode::Heating {
                1.0
            } else {
                0.0
            },
        );

        // Reset transient signals after this step so they do not carry over
        // unless reapplied by the controller.
        self.ctrl_load_fraction = 1.0;
        self.power_limit_w = None;

        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn save_state(&self) -> Vec<u8> {
        save_postcard(&TanklessState {
            setpoint_c: self.setpoint_c,
            duty_cycle: self.duty_cycle,
            mode_override: self.mode_override,
            outlet_temp_c: self.telemetry.get("outlet_temp_c").unwrap_or(0.0),
            thermal_output_w: self.telemetry.get("thermal_output_w").unwrap_or(0.0),
            fuel_input_w: self.telemetry.get("fuel_input_w").unwrap_or(0.0),
            parasitic_electric_w: self.telemetry.get("parasitic_electric_w").unwrap_or(0.0),
            draw_flow_rate_kg_s: self.telemetry.get("draw_flow_rate_kg_s").unwrap_or(0.0),
            dr_level: self.dr_level,
            dr_setpoint_offset_c: self.dr_setpoint_offset_c,
            dr_load_fraction: self.dr_load_fraction,
            dr_duration_remaining_s: self.dr_duration_remaining_s,
        })
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let decoded: TanklessState = load_postcard(state)?;
        self.setpoint_c = decoded.setpoint_c;
        self.duty_cycle = decoded.duty_cycle;
        self.mode_override = decoded.mode_override;
        self.dr_level = decoded.dr_level;
        self.dr_setpoint_offset_c = decoded.dr_setpoint_offset_c;
        self.dr_load_fraction = decoded.dr_load_fraction;
        self.dr_duration_remaining_s = decoded.dr_duration_remaining_s;

        self.telemetry
            .insert("outlet_temp_c", decoded.outlet_temp_c);
        self.telemetry
            .insert("thermal_output_w", decoded.thermal_output_w);
        self.telemetry.insert("fuel_input_w", decoded.fuel_input_w);
        self.telemetry
            .insert("parasitic_electric_w", decoded.parasitic_electric_w);
        self.telemetry
            .insert("draw_flow_rate_kg_s", decoded.draw_flow_rate_kg_s);
        self.telemetry.insert(
            "operating_mode",
            if decoded.thermal_output_w > 0.0 {
                1.0
            } else {
                0.0
            },
        );

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
                let limit_w = (max_power_kw * 1_000.0 * self.efficiency_factor).max(0.0);
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
}

fn parse_tankless_fuel_type(raw: Option<&str>) -> FuelType {
    match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("electric") => FuelType::Electric,
        Some("none") => FuelType::Electric,
        _ => parse_fuel_type(raw).unwrap_or(FuelType::Gas),
    }
}

/// Build port declarations for a tankless water heater.
///
/// Electric: one Electrical port only.
/// Gas: one Fuel port + one Electrical port (for the ignition controller parasitic).
fn build_ports(fuel_type: FuelType) -> Vec<PortDeclaration> {
    let dhw_port = PortDeclaration {
        port_type: PortType::Fluid,
        zone: None,
        loop_id: Some(super::DHW_DEMAND_LOOP),
        domain_id: None,
        fluid_type: Some(FluidType::Water),
    };
    if fuel_type == FuelType::Electric {
        vec![
            PortDeclaration {
                port_type: PortType::Electrical,
                zone: None,
                loop_id: None,
                domain_id: None,
                fluid_type: None,
            },
            dhw_port,
        ]
    } else {
        vec![
            PortDeclaration {
                port_type: PortType::Fuel,
                zone: None,
                loop_id: None,
                domain_id: None,
                fluid_type: None,
            },
            PortDeclaration {
                port_type: PortType::Electrical,
                zone: None,
                loop_id: None,
                domain_id: None,
                fluid_type: None,
            },
            dhw_port,
        ]
    }
}

fn default_telemetry() -> Telemetry {
    let mut telemetry = Telemetry::with_capacity(6);
    telemetry.insert("outlet_temp_c", 0.0);
    telemetry.insert("thermal_output_w", 0.0);
    telemetry.insert("fuel_input_w", 0.0);
    telemetry.insert("parasitic_electric_w", 0.0);
    telemetry.insert("draw_flow_rate_kg_s", 0.0);
    telemetry.insert("operating_mode", 0.0);
    telemetry
}

fn telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField {
            name: "outlet_temp_c".to_string(),
            unit: "C".to_string(),
            description: "Delivered outlet water temperature".to_string(),
        },
        TelemetryField {
            name: "thermal_output_w".to_string(),
            unit: "W".to_string(),
            description: "Instantaneous thermal output".to_string(),
        },
        TelemetryField {
            name: "fuel_input_w".to_string(),
            unit: "W".to_string(),
            description: "Input fuel/electric power".to_string(),
        },
        TelemetryField {
            name: "parasitic_electric_w".to_string(),
            unit: "W".to_string(),
            description: "Gas ignition controller standby electric draw (gas units only)"
                .to_string(),
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

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, time::Duration};

    use chrono::{Duration as ChronoDuration, TimeZone, Utc};
    use hares_types::{EnvironmentState, GridState, PortSlots, WeatherState, ZoneId, ZoneState};

    use super::{DEFAULT_GAS_PARASITIC_POWER_W, TanklessWH, WATER_SPECIFIC_HEAT_J_PER_KG_K};
    use crate::{Equipment, EquipmentConfig};

    fn env() -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: 21.0,
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
            current_time: Utc
                .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                .single()
                .expect("valid"),
            time_res: ChronoDuration::seconds(60),
        }
    }

    fn config() -> EquipmentConfig {
        let mut raw = HashMap::new();
        raw.insert("FuelType".to_string(), "gas".into());
        raw.insert("setpoint_c".to_string(), 50.0.into());
        raw.insert("inlet_temp_c".to_string(), 20.0.into());
        raw.insert("draw_flow_rate_kg_s".to_string(), 0.2.into());
        raw.insert("EnergyFactor".to_string(), 0.8.into());
        // 0.2 kg/s × 4183 J/(kg·K) × 30 K ≈ 25,098 W; set capacity above demand.
        raw.insert("max_thermal_power_w".to_string(), 30_000.0_f64.into());
        EquipmentConfig {
            name: "Tankless".to_string(),
            ochre_class: "Tankless Water Heater".to_string(),
            raw_config: raw,
        }
    }

    /// Build a config that also sets max_thermal_power_w explicitly.
    fn config_with_capacity(max_thermal_power_w: f64) -> EquipmentConfig {
        let mut raw = HashMap::new();
        raw.insert("FuelType".to_string(), "gas".into());
        raw.insert("setpoint_c".to_string(), 50.0.into());
        raw.insert("inlet_temp_c".to_string(), 20.0.into());
        raw.insert("draw_flow_rate_kg_s".to_string(), 0.2.into());
        raw.insert("EnergyFactor".to_string(), 0.8.into());
        raw.insert(
            "max_thermal_power_w".to_string(),
            max_thermal_power_w.into(),
        );
        EquipmentConfig {
            name: "Tankless".to_string(),
            ochre_class: "Tankless Water Heater".to_string(),
            raw_config: raw,
        }
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

        let expected_thermal = 0.2 * 4183.0 * (50.0 - 20.0);
        let expected_input = expected_thermal / 0.8;

        assert!((eq.telemetry().get("thermal_output_w").unwrap() - expected_thermal).abs() < 1e-6);
        assert!((eq.telemetry().get("fuel_input_w").unwrap() - expected_input).abs() < 1e-6);
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

        let outlet = eq.telemetry().get("outlet_temp_c").unwrap();
        assert_eq!(
            outlet, 20.0,
            "outlet_temp_c must equal inlet_temp_c (20.0) when off, got {outlet}"
        );
        assert_eq!(
            eq.telemetry().get("thermal_output_w").unwrap(),
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

        let saved = eq.save_state();

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
            eq.telemetry().get("thermal_output_w").unwrap_or(0.0) > 0.0,
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
            eq2.telemetry().get("thermal_output_w").unwrap_or(1.0),
            0.0,
            "thermal output must be zero during GridEmergency"
        );
        assert_eq!(
            eq2.telemetry().get("fuel_input_w").unwrap_or(1.0),
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
    /// Config: flow=0.2 kg/s, delta_T=30 K → demand = 0.2*4183*30 ≈ 25,098 W.
    /// With max_thermal_power_w=30,000 W this is within capacity.
    #[test]
    fn normal_operation_within_capacity_delivers_setpoint() {
        // demand_w = 0.2 * 4183 * 30 = 25,098 W < 30,000 W capacity
        let cap = config_with_capacity(30_000.0);
        let mut eq = TanklessWH::new(cap.clone());
        eq.init(&cap, &env()).unwrap();

        step_once(&mut eq);

        let outlet = eq.telemetry().get("outlet_temp_c").unwrap();
        let thermal = eq.telemetry().get("thermal_output_w").unwrap();
        let fuel = eq.telemetry().get("fuel_input_w").unwrap();
        let expected_thermal = 0.2 * WATER_SPECIFIC_HEAT_J_PER_KG_K * 30.0;

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
    /// Config: flow=1.0 kg/s, delta_T=30 K → demand = 1.0*4183*30 = 125,490 W.
    /// With max_thermal_power_w=20,000 W, output clamps at 20,000 W.
    #[test]
    fn over_capacity_clamps_output_and_reduces_outlet_temp() {
        let mut raw = HashMap::new();
        raw.insert("FuelType".to_string(), "gas".into());
        raw.insert("setpoint_c".to_string(), 50.0.into());
        raw.insert("inlet_temp_c".to_string(), 20.0.into());
        // 1.0 kg/s → demand = 1.0 * 4183 * 30 = 125,490 W >> 20,000 W capacity
        raw.insert("draw_flow_rate_kg_s".to_string(), 1.0.into());
        raw.insert("EnergyFactor".to_string(), 0.8.into());
        raw.insert("max_thermal_power_w".to_string(), 20_000.0_f64.into());
        let cap = EquipmentConfig {
            name: "Tankless".to_string(),
            ochre_class: "Tankless Water Heater".to_string(),
            raw_config: raw,
        };

        let mut eq = TanklessWH::new(cap.clone());
        eq.init(&cap, &env()).unwrap();

        step_once(&mut eq);

        let thermal = eq.telemetry().get("thermal_output_w").unwrap();
        let fuel = eq.telemetry().get("fuel_input_w").unwrap();
        let outlet = eq.telemetry().get("outlet_temp_c").unwrap();

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
        let expected_outlet = 20.0 + 20_000.0 / (1.0 * WATER_SPECIFIC_HEAT_J_PER_KG_K);
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
        let mut raw = HashMap::new();
        raw.insert("FuelType".to_string(), "electric".into());
        raw.insert("setpoint_c".to_string(), 50.0.into());
        raw.insert("inlet_temp_c".to_string(), 20.0.into());
        raw.insert("draw_flow_rate_kg_s".to_string(), 0.0_f64.into());
        raw.insert("EnergyFactor".to_string(), 0.95.into());
        let cfg = EquipmentConfig {
            name: "Tankless".to_string(),
            ochre_class: "Tankless Water Heater".to_string(),
            raw_config: raw,
        };

        let mut eq = TanklessWH::new(cfg.clone());
        eq.init(&cfg, &env()).unwrap();

        let ports = step_once(&mut eq);

        assert_eq!(
            eq.telemetry().get("thermal_output_w").unwrap(),
            0.0,
            "zero flow must produce zero thermal output"
        );
        assert_eq!(
            eq.telemetry().get("fuel_input_w").unwrap(),
            0.0,
            "zero flow must produce zero fuel input"
        );
        assert_eq!(
            ports.electrical.load_power_kw, 0.0,
            "zero flow must produce zero electrical draw"
        );
    }

    /// Fuel consumption (input power) must never exceed max_input_power_w = capacity / efficiency.
    #[test]
    fn fuel_input_never_exceeds_max_input_power() {
        // Very high flow to force over-capacity.
        let mut raw = HashMap::new();
        raw.insert("FuelType".to_string(), "gas".into());
        raw.insert("setpoint_c".to_string(), 60.0.into());
        raw.insert("inlet_temp_c".to_string(), 5.0.into());
        raw.insert("draw_flow_rate_kg_s".to_string(), 5.0.into());
        raw.insert("EnergyFactor".to_string(), 0.85.into());
        raw.insert("max_thermal_power_w".to_string(), 25_000.0_f64.into());
        let cfg = EquipmentConfig {
            name: "Tankless".to_string(),
            ochre_class: "Tankless Water Heater".to_string(),
            raw_config: raw,
        };

        let mut eq = TanklessWH::new(cfg.clone());
        eq.init(&cfg, &env()).unwrap();

        step_once(&mut eq);

        let fuel = eq.telemetry().get("fuel_input_w").unwrap();
        let max_input_w = 25_000.0 / 0.85;

        assert!(
            fuel <= max_input_w + 1e-6,
            "fuel_input_w ({fuel:.2} W) must not exceed max_input_power_w ({max_input_w:.2} W)"
        );
    }

    /// Very low flow rate: demand is well within capacity, setpoint delivered.
    #[test]
    fn very_low_flow_rate_within_capacity() {
        let mut raw = HashMap::new();
        raw.insert("FuelType".to_string(), "gas".into());
        raw.insert("setpoint_c".to_string(), 50.0.into());
        raw.insert("inlet_temp_c".to_string(), 20.0.into());
        // 0.001 kg/s → demand = 0.001 * 4183 * 30 ≈ 125.5 W, well below 20 kW capacity
        raw.insert("draw_flow_rate_kg_s".to_string(), 0.001_f64.into());
        raw.insert("EnergyFactor".to_string(), 0.9.into());
        raw.insert("max_thermal_power_w".to_string(), 20_000.0_f64.into());
        let cfg = EquipmentConfig {
            name: "Tankless".to_string(),
            ochre_class: "Tankless Water Heater".to_string(),
            raw_config: raw,
        };

        let mut eq = TanklessWH::new(cfg.clone());
        eq.init(&cfg, &env()).unwrap();

        step_once(&mut eq);

        let outlet = eq.telemetry().get("outlet_temp_c").unwrap();
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
        let m_dot = capacity_w / (WATER_SPECIFIC_HEAT_J_PER_KG_K * delta_t);

        let mut raw = HashMap::new();
        raw.insert("FuelType".to_string(), "gas".into());
        raw.insert("setpoint_c".to_string(), 50.0.into());
        raw.insert("inlet_temp_c".to_string(), 20.0.into());
        raw.insert("draw_flow_rate_kg_s".to_string(), m_dot.into());
        raw.insert("EnergyFactor".to_string(), 0.9.into());
        raw.insert("max_thermal_power_w".to_string(), capacity_w.into());
        let cfg = EquipmentConfig {
            name: "Tankless".to_string(),
            ochre_class: "Tankless Water Heater".to_string(),
            raw_config: raw,
        };

        let mut eq = TanklessWH::new(cfg.clone());
        eq.init(&cfg, &env()).unwrap();

        step_once(&mut eq);

        let outlet = eq.telemetry().get("outlet_temp_c").unwrap();
        let thermal = eq.telemetry().get("thermal_output_w").unwrap();

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
        let mut raw = HashMap::new();
        raw.insert("FuelType".to_string(), "gas".into());
        raw.insert("setpoint_c".to_string(), 50.0.into());
        raw.insert("inlet_temp_c".to_string(), 20.0.into());
        raw.insert("draw_flow_rate_kg_s".to_string(), 1.0.into());
        raw.insert("EnergyFactor".to_string(), 0.8.into());
        raw.insert("max_thermal_power_w".to_string(), 20_000.0_f64.into());
        let cfg = EquipmentConfig {
            name: "Tankless".to_string(),
            ochre_class: "Tankless Water Heater".to_string(),
            raw_config: raw,
        };

        let mut eq = TanklessWH::new(cfg.clone());
        eq.init(&cfg, &env()).unwrap();
        eq.apply_control(&ControlSignal::DutyCycle {
            on_fraction: 0.5,
            period_s: None,
        })
        .unwrap();

        step_once(&mut eq);

        let thermal = eq.telemetry().get("thermal_output_w").unwrap();
        let outlet = eq.telemetry().get("outlet_temp_c").unwrap();
        let effective_capacity = 20_000.0 * 0.5;

        assert!(
            (thermal - effective_capacity).abs() < 1e-6,
            "thermal must clamp at effective capacity {effective_capacity} W (duty=0.5), got {thermal}"
        );
        assert!(
            outlet < 50.0,
            "outlet must be below setpoint when over effective capacity, got {outlet}"
        );
        // Outlet uses FULL rated power (20 kW), not duty-scaled — heater fires at 100% during on-phase
        let expected_outlet = 20.0 + 20_000.0 / (1.0 * WATER_SPECIFIC_HEAT_J_PER_KG_K);
        assert!(
            (outlet - expected_outlet).abs() < 1e-6,
            "outlet temp must use full rated power: {expected_outlet:.4}°C, got {outlet:.4}"
        );
    }

    /// PowerLimit signal reduces max_thermal_power_w and caps fuel consumption accordingly.
    #[test]
    fn power_limit_signal_caps_fuel_input() {
        use hares_types::ControlSignal;

        // High-flow config to ensure we are in the over-capacity regime.
        let mut raw = HashMap::new();
        raw.insert("FuelType".to_string(), "gas".into());
        raw.insert("setpoint_c".to_string(), 50.0.into());
        raw.insert("inlet_temp_c".to_string(), 20.0.into());
        raw.insert("draw_flow_rate_kg_s".to_string(), 1.0.into());
        raw.insert("EnergyFactor".to_string(), 0.8.into());
        raw.insert("max_thermal_power_w".to_string(), 20_000.0_f64.into());
        let cfg = EquipmentConfig {
            name: "Tankless".to_string(),
            ochre_class: "Tankless Water Heater".to_string(),
            raw_config: raw,
        };

        let mut eq = TanklessWH::new(cfg.clone());
        eq.init(&cfg, &env()).unwrap();

        // Limit input power to 10 kW → thermal limit = 10,000 * 0.8 = 8,000 W
        eq.apply_control(&ControlSignal::PowerLimit {
            max_power_kw: 10.0,
            ramp_rate_kw_per_s: None,
        })
        .unwrap();

        step_once(&mut eq);

        let fuel = eq.telemetry().get("fuel_input_w").unwrap();
        assert!(
            fuel <= 10_000.0 + 1e-6,
            "fuel input ({fuel:.1} W) must not exceed 10,000 W limit"
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
            eq.telemetry().get("thermal_output_w").unwrap(),
            0.0,
            "thermal output must be zero when off"
        );
        assert_eq!(
            eq.telemetry().get("fuel_input_w").unwrap(),
            0.0,
            "fuel input must be zero when burner is off"
        );
        assert_eq!(
            ports.fuel.get(hares_types::FuelType::Gas),
            0.0,
            "gas fuel port must be zero when burner is off"
        );
        // Parasitic electric draw must be non-zero even when off.
        let parasitic_kw = DEFAULT_GAS_PARASITIC_POWER_W / 1_000.0;
        assert!(
            (ports.electrical.load_power_kw - parasitic_kw).abs() < 1e-9,
            "gas standby parasitic electric must be {parasitic_kw:.6} kW, got {}",
            ports.electrical.load_power_kw
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
            eq.telemetry().get("thermal_output_w").unwrap() > 0.0,
            "burner must be producing heat in this test"
        );

        // Electrical port must include at least the parasitic draw.
        let parasitic_kw = DEFAULT_GAS_PARASITIC_POWER_W / 1_000.0;
        assert!(
            ports.electrical.load_power_kw >= parasitic_kw - 1e-9,
            "gas electrical draw must include at least parasitic_power_w ({parasitic_kw:.6} kW), got {}",
            ports.electrical.load_power_kw
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

        let mut raw = HashMap::new();
        raw.insert("FuelType".to_string(), "gas".into());
        raw.insert("setpoint_c".to_string(), 50.0.into());
        raw.insert("inlet_temp_c".to_string(), 20.0.into());
        raw.insert("draw_flow_rate_kg_s".to_string(), 0.2.into());
        raw.insert("EnergyFactor".to_string(), 0.8.into());
        raw.insert("parasitic_power_w".to_string(), 15.0_f64.into());
        let cfg = EquipmentConfig {
            name: "Tankless".to_string(),
            ochre_class: "Tankless Water Heater".to_string(),
            raw_config: raw,
        };

        let mut eq = TanklessWH::new(cfg.clone());
        eq.init(&cfg, &env()).unwrap();

        // Force off to isolate just the parasitic draw.
        eq.apply_control(&ControlSignal::ModeOverride {
            mode: OperatingMode::Off,
        })
        .unwrap();

        let ports = step_once(&mut eq);

        assert!(
            (ports.electrical.load_power_kw - 15.0 / 1_000.0).abs() < 1e-9,
            "configured parasitic 15 W must appear in electrical port, got {} kW",
            ports.electrical.load_power_kw
        );
    }

    /// PowerLimit is transient: it resets after each step. After a step without
    /// reapplying PowerLimit, the unit should deliver full rated capacity.
    #[test]
    fn power_limit_resets_after_step() {
        use hares_types::ControlSignal;

        let mut raw = HashMap::new();
        raw.insert("FuelType".to_string(), "gas".into());
        raw.insert("setpoint_c".to_string(), 50.0.into());
        raw.insert("inlet_temp_c".to_string(), 20.0.into());
        raw.insert("draw_flow_rate_kg_s".to_string(), 1.0.into());
        raw.insert("EnergyFactor".to_string(), 0.8.into());
        raw.insert("max_thermal_power_w".to_string(), 20_000.0_f64.into());
        let cfg = EquipmentConfig {
            name: "Tankless".to_string(),
            ochre_class: "Tankless Water Heater".to_string(),
            raw_config: raw,
        };

        let mut eq = TanklessWH::new(cfg.clone());
        eq.init(&cfg, &env()).unwrap();

        // Apply PowerLimit → step → limit active
        eq.apply_control(&ControlSignal::PowerLimit {
            max_power_kw: 5.0,
            ramp_rate_kw_per_s: None,
        })
        .unwrap();
        step_once(&mut eq);
        let thermal_limited = eq.telemetry().get("thermal_output_w").unwrap();

        // Step again WITHOUT reapplying PowerLimit → full capacity restored
        step_once(&mut eq);
        let thermal_full = eq.telemetry().get("thermal_output_w").unwrap();

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

        let mut raw = HashMap::new();
        raw.insert("FuelType".to_string(), "gas".into());
        raw.insert("setpoint_c".to_string(), 50.0.into());
        raw.insert("inlet_temp_c".to_string(), 20.0.into());
        raw.insert("draw_flow_rate_kg_s".to_string(), 1.0.into());
        raw.insert("EnergyFactor".to_string(), 0.8.into());
        raw.insert("max_thermal_power_w".to_string(), 20_000.0_f64.into());
        let cfg = EquipmentConfig {
            name: "Tankless".to_string(),
            ochre_class: "Tankless Water Heater".to_string(),
            raw_config: raw,
        };

        let mut eq = TanklessWH::new(cfg.clone());
        eq.init(&cfg, &env()).unwrap();

        // Apply power limit, save, restore
        eq.apply_control(&ControlSignal::PowerLimit {
            max_power_kw: 5.0,
            ramp_rate_kw_per_s: None,
        })
        .unwrap();

        let state = eq.save_state();
        let mut restored = TanklessWH::new(cfg.clone());
        restored.init(&cfg, &env()).unwrap();
        restored.load_state(&state).unwrap();

        // Restored unit should use full rated capacity (power limit is transient)
        step_once(&mut restored);
        let thermal = restored.telemetry().get("thermal_output_w").unwrap();
        assert!(
            (thermal - 20_000.0).abs() < 1e-6,
            "after restore, full rated capacity should apply: got {thermal}"
        );
    }

    /// Electric tankless must not have any parasitic draw (gas-only feature).
    #[test]
    fn electric_tankless_has_no_parasitic_draw_when_off() {
        use hares_types::{ControlSignal, OperatingMode};

        let mut raw = HashMap::new();
        raw.insert("FuelType".to_string(), "electric".into());
        raw.insert("setpoint_c".to_string(), 50.0.into());
        raw.insert("inlet_temp_c".to_string(), 20.0.into());
        raw.insert("draw_flow_rate_kg_s".to_string(), 0.2.into());
        raw.insert("EnergyFactor".to_string(), 0.95.into());
        let cfg = EquipmentConfig {
            name: "Tankless".to_string(),
            ochre_class: "Tankless Water Heater".to_string(),
            raw_config: raw,
        };

        let mut eq = TanklessWH::new(cfg.clone());
        eq.init(&cfg, &env()).unwrap();

        eq.apply_control(&ControlSignal::ModeOverride {
            mode: OperatingMode::Off,
        })
        .unwrap();

        let ports = step_once(&mut eq);

        assert_eq!(
            ports.electrical.load_power_kw, 0.0,
            "electric tankless must draw zero when off (no parasitic)"
        );
    }

    /// Verify the tankless WH reads mains temp from the environment's custom domain
    /// rather than the static fallback.
    ///
    /// Two environments with different mains temps (5°C vs 25°C) injected via
    /// MAINS_WATER_DOMAIN_ID must produce different thermal outputs: a colder
    /// inlet requires more energy to reach setpoint.
    #[test]
    fn tankless_wh_uses_dynamic_mains_temp_from_environment() {
        use hares_types::{DomainId, DomainUpdate};

        fn env_with_mains(mains_c: f64) -> EnvironmentState {
            EnvironmentState {
                zones: vec![ZoneState {
                    id: ZoneId(1),
                    temperature_c: 21.0,
                    humidity_ratio: 0.008,
                    relative_humidity: 0.45,
                    wet_bulb_c: 14.0,
                    volume_m3: 200.0,
                }],
                weather: WeatherState {
                    outdoor_temp_c: 10.0,
                    ..Default::default()
                },
                grid: hares_types::GridState {
                    voltage_pu: 1.0,
                    frequency_hz: 60.0,
                },
                custom_domains: vec![DomainUpdate {
                    domain_id: DomainId(u16::MAX - 1),
                    zone_temperatures_c: vec![],
                    custom_payload: Some(vec![mains_c]),
                }],
                current_time: Utc
                    .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                    .single()
                    .expect("valid"),
                time_res: ChronoDuration::seconds(60),
            }
        }

        let cfg = {
            let mut raw = HashMap::new();
            raw.insert("FuelType".to_string(), "gas".into());
            raw.insert("setpoint_c".to_string(), 50.0.into());
            // inlet_temp_c is the static fallback; the dynamic value should override it.
            raw.insert("inlet_temp_c".to_string(), 20.0.into());
            raw.insert("draw_flow_rate_kg_s".to_string(), 0.2.into());
            raw.insert("EnergyFactor".to_string(), 0.9.into());
            raw.insert("max_thermal_power_w".to_string(), 60_000.0_f64.into());
            EquipmentConfig {
                name: "TanklessTest".to_string(),
                ochre_class: "Tankless Water Heater".to_string(),
                raw_config: raw,
            }
        };

        let cold_env = env_with_mains(5.0);
        let warm_env = env_with_mains(25.0);

        let mut eq_cold = TanklessWH::new(cfg.clone());
        eq_cold.init(&cfg, &cold_env).unwrap();
        let mut ports_cold = PortSlots::default();
        eq_cold
            .step(&cold_env, Duration::from_secs(60), &mut ports_cold)
            .unwrap();
        let thermal_cold = eq_cold.telemetry().get("thermal_output_w").unwrap_or(0.0);

        let mut eq_warm = TanklessWH::new(cfg.clone());
        eq_warm.init(&cfg, &warm_env).unwrap();
        let mut ports_warm = PortSlots::default();
        eq_warm
            .step(&warm_env, Duration::from_secs(60), &mut ports_warm)
            .unwrap();
        let thermal_warm = eq_warm.telemetry().get("thermal_output_w").unwrap_or(0.0);

        assert!(
            thermal_cold > thermal_warm,
            "cold inlet (5°C) must require more thermal energy than warm inlet (25°C); \
             cold={thermal_cold:.1}W warm={thermal_warm:.1}W"
        );

        // Verify the values align with exact physics: Q = m_dot * Cp * (Tset - Tinlet)
        let cp = WATER_SPECIFIC_HEAT_J_PER_KG_K;
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
}
