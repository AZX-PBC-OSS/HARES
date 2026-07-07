//! Electric baseboard heater model.

use std::borrow::Cow;
use std::time::Duration;

use chrono::{DateTime, FixedOffset};
use hares_types::{
    ControlCapabilities, ControlSignal, CoreCapabilities, CoreFlows, CoreOutput, CorePerformance,
    CoreState, DRLevel, ElectricPower, EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId,
    ExecutionStage, FuelType, HaresError, OperatingMode, PortContribution, PortDeclaration,
    PortSlots, Telemetry, TelemetryField, ThermalCategory,
};
use serde::{Deserialize, Serialize};

use hares_types::telemetry_keys as tk;

use hares_physics::units::{power_kw_to_w, power_w_to_kw};

use crate::hvac::heating_config::ElectricBaseboardConfig;
use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_versioned, try_save_versioned};

use super::{
    HvacEquipment, HvacEquipmentType, RuntimeSetpointOverride, ThermostatMode,
    helpers::{
        apply_heating_control_unchecked, apply_simple_heating_ideal_capacity_control,
        apply_simple_mode_override_and_dr, apply_simple_mode_override_in_control,
        equipment_id_from_config, outage_forces_off, update_heating_control,
        zone_id_from_config_or_default,
    },
};

pub struct ElectricBaseboard {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,
    core_output: CoreOutput,
    hvac: HvacEquipment,
    rated_capacity_w: f64,
    eir: f64,
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
    /// Rule R1 reactive-only ZIP: resistive element pf 1.0 →
    /// Q exactly zero, real power stays bit-identical.
    zip: hares_types::zip::ZipLoad,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct BaseboardState {
    mode: ThermostatMode,
    duty_cycle: f64,
    last_mode_switch_at: Option<DateTime<FixedOffset>>,
    runtime_setpoints: Option<RuntimeSetpointOverride>,
    operating_mode: OperatingMode,
    run_time_s: f64,
    electric_kw: f64,
    thermal_output_w: f64,
    mode_override: Option<OperatingMode>,
    dr_level: DRLevel,
}

impl ElectricBaseboard {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let (zone, zone_id_explicit) = zone_id_from_config_or_default(&config, &config.name);
        let descriptor = EquipmentDescriptor {
            id: EquipmentId(equipment_id_from_config(&config).unwrap_or(0)),
            name: config.name,
            end_use: EndUse::HVAC_HEATING,
            equipment_type: Cow::Borrowed("Electric Baseboard"),
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
            telemetry_fields: telemetry_fields(),
            zone_type: None,
        };

        Self {
            descriptor,
            ports: vec![
                PortDeclaration::electrical(),
                PortDeclaration::thermal(zone),
            ],
            telemetry: default_telemetry(),
            core_output: CoreOutput::default(),
            hvac: HvacEquipment::new(HvacEquipmentType::Baseboard, zone),
            rated_capacity_w: 0.0,
            eir: 1.0,
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

impl Equipment for ElectricBaseboard {
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
        self.hvac.config.duct_dse = 1.0;
        self.hvac.config.duct_zone_id = None;
        self.hvac.config.basement_heat_frac = 0.0;
        self.hvac.config.basement_zone_id = None;
        self.hvac.update_zone_heat_fractions();
        let typed = config.require_typed::<ElectricBaseboardConfig>("Electric Baseboard")?;
        self.rated_capacity_w = typed.capacity_w.max(0.0);
        self.eir = typed.eir;
        if self.eir <= 0.0 || !self.eir.is_finite() {
            return Err(HaresError::Equipment(format!(
                "invalid Electric Baseboard eir: {}",
                self.eir
            )));
        }
        self.hvac.config.heating_capacities_w = vec![self.rated_capacity_w];
        self.operating_mode = OperatingMode::Off;
        self.run_time_s = 0.0;
        self.telemetry = default_telemetry();
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
            "Electric Baseboard",
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
        let reactive_power_kvar = self
            .zip
            .reactive_kvar(electric_kw, env.grid.bus_voltage_pu());

        if thermal_output_w > 0.0 {
            ports.accumulate(&PortContribution::Electrical {
                active_power_w: power_kw_to_w(electric_kw),
                reactive_power_kvar,
            })?;
            self.hvac.write_zone_thermal_contributions(
                ports,
                thermal_output_w,
                0.0,
                ThermalCategory::HvacHeating,
            )?;
            self.run_time_s += dt.as_secs_f64();
        }

        let has_nonzero_flow = electric_kw > 0.0 || thermal_output_w != 0.0;
        self.operating_mode = self
            .operating_mode
            .resolve_idle(has_nonzero_flow, Some(thermal_output_w));

        self.telemetry.set(tk::ELECTRIC_KW, electric_kw);
        self.telemetry
            .set(tk::REACTIVE_POWER_KVAR, reactive_power_kvar);
        self.telemetry.set(tk::THERMAL_OUTPUT_W, thermal_output_w);
        self.telemetry
            .set(tk::OPERATING_MODE, self.operating_mode.as_code());
        let sp = self.hvac.effective_setpoints();
        self.telemetry.set(tk::HEATING_SETPOINT_C, sp.heating_c);
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
                setpoint_c: Some(sp.heating_c),
            },
            performance: CorePerformance::default(),
        };

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
            &BaseboardState {
                mode: self.hvac.thermostat_fsm.mode,
                duty_cycle: self.hvac.runtime.duty_cycle,
                last_mode_switch_at: self.hvac.thermostat_fsm.last_mode_switch_at,
                runtime_setpoints: self.hvac.thermostat_fsm.runtime_setpoints,
                operating_mode: self.operating_mode,
                run_time_s: self.run_time_s,
                electric_kw: self.telemetry.get(tk::ELECTRIC_KW).unwrap_or(0.0),
                thermal_output_w: self.telemetry.get(tk::THERMAL_OUTPUT_W).unwrap_or(0.0),
                mode_override: self.mode_override,
                dr_level: self.dr_level,
            },
            Self::checkpoint_version(),
            "Baseboard",
        )
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let decoded: BaseboardState = load_versioned(
            state,
            Self::checkpoint_version(),
            "Baseboard",
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
        self.core_output = CoreOutput::default();
        Ok(())
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        if apply_simple_mode_override_and_dr(
            &mut self.mode_override,
            &mut self.dr_level,
            signal,
            "Electric Baseboard",
        )? {
            return Ok(());
        }
        apply_heating_control_unchecked(&mut self.hvac, signal, "Electric Baseboard")?;
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
        "Electric Baseboard",
        Box::new(|config| Box::new(ElectricBaseboard::new(config))),
    );
}

fn default_telemetry() -> Telemetry {
    let mut telemetry = Telemetry::with_capacity(6);
    telemetry.insert(tk::ELECTRIC_KW, 0.0);
    telemetry.insert(tk::REACTIVE_POWER_KVAR, 0.0);
    telemetry.insert(tk::THERMAL_OUTPUT_W, 0.0);
    telemetry.insert(tk::OPERATING_MODE, 0.0);
    telemetry.insert(tk::HEATING_SETPOINT_C, 0.0);
    telemetry.insert(tk::COOLING_SETPOINT_C, 0.0);
    telemetry
}

fn telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField {
            name: tk::ELECTRIC_KW.to_string(),
            unit: "kW".to_string(),
            description: "Electric baseboard active power draw".to_string(),
        },
        TelemetryField {
            name: tk::REACTIVE_POWER_KVAR.to_string(),
            unit: "kVAR".to_string(),
            description: "Reactive power (positive = inductive/lagging)".to_string(),
        },
        TelemetryField {
            name: tk::THERMAL_OUTPUT_W.to_string(),
            unit: "W".to_string(),
            description: "Delivered sensible zone heat (baseboard bypasses ducts)".to_string(),
        },
        TelemetryField {
            name: tk::OPERATING_MODE.to_string(),
            unit: "enum".to_string(),
            description: "Operating mode code: 0=Off, 1=Heating".to_string(),
        },
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        ControlSignal, CoreCapabilities, DRLevel, EnvironmentState, ExecutionStage, GridState,
        OperatingMode, PortSlots, ThermalAccumulator, WeatherState, ZoneId, ZoneState,
        telemetry_keys as tk,
    };

    use super::ElectricBaseboard;

    use crate::hvac::heating_config::ElectricBaseboardConfig;
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
                island_bus_voltage_pu: None,
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

    fn config(capacity_w: f64) -> EquipmentConfig {
        EquipmentConfig::from_typed(
            "Baseboard".to_string(),
            "Electric Baseboard".to_string(),
            ElectricBaseboardConfig {
                zone_id: Some(1),
                capacity_w,
                ..ElectricBaseboardConfig::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn baseboard_forces_dse_to_one_and_writes_direct_zone_heat() {
        let cfg = config(3_000.0);
        let mut eq = ElectricBaseboard::new(cfg.clone());
        let env = env(18.0);
        eq.init(&cfg, &env).unwrap();
        assert!((eq.hvac.config.duct_dse - 1.0).abs() < 1e-12);

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        assert!((ports.electrical.net_active_w() - 3000.0).abs() < 1.0);
        assert!((ports.thermal[0].sensible_gain_w - 3_000.0).abs() < 1e-9);
    }

    /// Grid outage (de-energized bus): the resistance elements have no
    /// supply — forced off at the control level (zero draw, zero delivered
    /// heat). Islanded homes keep heating; control resumes on restoration.
    #[test]
    fn grid_outage_forces_baseboard_off_and_islanded_home_keeps_heating() {
        let cfg = config(3_000.0);
        let mut eq = ElectricBaseboard::new(cfg.clone());
        let env_nominal = env(18.0);
        eq.init(&cfg, &env_nominal).unwrap();

        let mut env_outage = env(18.0);
        env_outage.grid.voltage_pu = 0.0;
        assert_eq!(eq.update_control(&env_outage), OperatingMode::Off);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env_outage, Duration::from_secs(60), &mut ports)
            .unwrap();
        assert_eq!(ports.electrical.net_active_w(), 0.0);
        assert_eq!(ports.thermal[0].sensible_gain_w, 0.0);

        // Islanded: bus energized by a backup source → heating proceeds.
        let mut env_islanded = env(18.0);
        env_islanded.grid.voltage_pu = 0.0;
        env_islanded.grid.island_bus_voltage_pu = Some(1.0);
        env_islanded.current_time += ChronoDuration::minutes(1);
        assert_eq!(eq.update_control(&env_islanded), OperatingMode::Heating);
        ports.zero();
        eq.step(&env_islanded, Duration::from_secs(60), &mut ports)
            .unwrap();
        assert!((ports.electrical.net_active_w() - 3_000.0).abs() < 1.0);
        assert!((ports.thermal[0].sensible_gain_w - 3_000.0).abs() < 1e-9);

        // Restoration: heating resumes.
        let mut env_restored = env(18.0);
        env_restored.current_time += ChronoDuration::minutes(2);
        assert_eq!(eq.update_control(&env_restored), OperatingMode::Heating);
    }

    #[test]
    fn state_round_trip_preserves_mode() {
        let cfg = config(3_000.0);
        let mut eq = ElectricBaseboard::new(cfg.clone());
        let env = env(18.0);
        eq.init(&cfg, &env).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        let state = eq.save_state().unwrap();

        let mut restored = ElectricBaseboard::new(cfg.clone());
        restored.init(&cfg, &env).unwrap();
        restored.load_state(&state).unwrap();
        assert_eq!(
            restored
                .telemetry()
                .get(hares_types::telemetry_keys::ELECTRIC_KW),
            Some(3.0)
        );
    }

    #[test]
    fn raw_config_rejected_with_typed_diagnostic() {
        let cfg = EquipmentConfig::raw(
            "BB".to_string(),
            "Electric Baseboard".to_string(),
            std::collections::HashMap::new(),
        );
        let mut eq = ElectricBaseboard::new(cfg.clone());
        let result = eq.init(&cfg, &env(18.0));
        let err = result.expect_err("raw electric baseboard config must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("Electric Baseboard requires typed config"));
        assert!(msg.contains("from_typed"));
    }

    #[test]
    fn registry_includes_baseboard_alias_and_thermal_stage() {
        let registry = EquipmentRegistry::new();
        assert!(registry.get("Electric Baseboard").is_some());

        let eq = registry
            .create("Electric Baseboard", config(5_000.0))
            .unwrap();
        assert_eq!(eq.descriptor().stage, ExecutionStage::Thermal);
    }

    #[test]
    fn ideal_capacity_control_scales_baseboard_output() {
        let cfg = config(3_000.0);
        let mut eq = ElectricBaseboard::new(cfg.clone());
        let env = env(18.0);
        eq.init(&cfg, &env).unwrap();
        eq.apply_control(&ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(21.0),
            cooling_setpoint_c: None,
            deadband_c: None,
        })
        .unwrap();
        eq.apply_control(&ControlSignal::IdealCapacity {
            capacity_w: 1_500.0,
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
            (ports.thermal[0].sensible_gain_w - 1_500.0).abs() < 1e-6,
            "IdealCapacity must scale delivered heat to 50% of rated output"
        );
    }

    #[test]
    fn dr_normal_clears_setpoint_curtailment() {
        let cfg = config(3_000.0);
        let mut eq = ElectricBaseboard::new(cfg.clone());
        let env = env(18.0);
        eq.init(&cfg, &env).unwrap();

        // Set a heating setpoint so the base is known.
        eq.apply_control_unchecked(&ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(21.0),
            cooling_setpoint_c: None,
            deadband_c: None,
        })
        .unwrap();

        // Apply DR High: -2°C setpoint offset.
        eq.apply_control_unchecked(&ControlSignal::DemandResponse {
            level: DRLevel::High,
            duration_s: Some(3600.0),
        })
        .unwrap();

        // DR offset must be applied in runtime state.
        eq.update_control(&env);
        assert!(
            (eq.hvac.runtime.dr_setpoint_offset_c + 2.0).abs() < 1e-9,
            "DR High must set dr_setpoint_offset_c to -2.0, got {}",
            eq.hvac.runtime.dr_setpoint_offset_c
        );

        // Revert to DR Normal: offset must be cleared.
        eq.apply_control_unchecked(&ControlSignal::DemandResponse {
            level: DRLevel::Normal,
            duration_s: None,
        })
        .unwrap();
        eq.update_control(&env);
        assert!(
            eq.hvac.runtime.dr_setpoint_offset_c.abs() < 1e-9,
            "DR Normal must clear dr_setpoint_offset_c, got {}",
            eq.hvac.runtime.dr_setpoint_offset_c
        );

        // Verify the effective setpoint is unaffected (no runtime_setpoints pollution).
        let sp = eq.hvac.effective_setpoints();
        assert!(
            (sp.heating_c - 21.0).abs() < 1e-9,
            "heating setpoint must be 21.0 after DR Normal, got {:.3}",
            sp.heating_c
        );
    }

    #[test]
    fn can_accept_mode_override_and_demand_response() {
        let cfg = config(3_000.0);
        let eq = ElectricBaseboard::new(cfg);
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
    fn mode_override_off_forces_equipment_off() {
        let cfg = config(3_000.0);
        let mut eq = ElectricBaseboard::new(cfg.clone());
        let env = env(18.0); // below setpoint → thermostat would heat
        eq.init(&cfg, &env).unwrap();

        // Force off via ModeOverride
        eq.apply_control_unchecked(&ControlSignal::ModeOverride {
            mode: OperatingMode::Off,
        })
        .unwrap();

        let mode = eq.update_control(&env);
        assert_eq!(mode, OperatingMode::Off);

        // Verify no heating output
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        assert!(
            ports.thermal[0].sensible_gain_w < 1e-9,
            "ModeOverride Off must prevent heating"
        );
    }

    #[test]
    fn demand_response_curtails_heating() {
        let cfg = config(3_000.0);
        let mut eq = ElectricBaseboard::new(cfg.clone());
        let env = env(18.0); // below setpoint → thermostat would normally heat
        eq.init(&cfg, &env).unwrap();

        // Apply DemandResponse High: -2°C setpoint offset
        eq.apply_control_unchecked(&ControlSignal::DemandResponse {
            level: DRLevel::High,
            duration_s: Some(3600.0),
        })
        .unwrap();

        // GridEmergency fully prevents heating
        let mut eq_emerg = ElectricBaseboard::new(cfg.clone());
        eq_emerg.init(&cfg, &env).unwrap();
        eq_emerg
            .apply_control_unchecked(&ControlSignal::DemandResponse {
                level: DRLevel::GridEmergency,
                duration_s: Some(3600.0),
            })
            .unwrap();

        let mode_emerg = eq_emerg.update_control(&env);
        assert_eq!(
            mode_emerg,
            OperatingMode::Off,
            "GridEmergency must force equipment off"
        );

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq_emerg
            .step(&env, Duration::from_secs(60), &mut ports)
            .unwrap();
        assert!(
            ports.thermal[0].sensible_gain_w < 1e-9,
            "GridEmergency must prevent heating"
        );
    }

    #[test]
    fn electric_baseboard_reactive_power_is_some_zero_at_unity_pf() {
        let cfg = config(3_000.0);
        let mut eq = ElectricBaseboard::new(cfg.clone());
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
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert!(ports.electrical.load_power_w > 0.0);
        assert_eq!(ports.electrical.reactive_power_kvar, 0.0);
        assert_eq!(eq.core_output().flows.reactive_power_kvar, Some(0.0));
        assert_eq!(eq.telemetry().get(tk::REACTIVE_POWER_KVAR), Some(0.0));
        hares_types::validate_core_contract(eq.descriptor(), eq.core_output())
            .expect("validate_core_contract");
    }

    #[test]
    fn zero_capacity_dispatch_reconciles_heating_to_standby() {
        let cfg = config(3_000.0);
        let mut eq = ElectricBaseboard::new(cfg.clone());
        let env = env(18.0);
        eq.init(&cfg, &env).unwrap();
        eq.apply_control(&ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(21.0),
            cooling_setpoint_c: None,
            deadband_c: None,
        })
        .unwrap();
        eq.apply_control(&ControlSignal::IdealCapacity {
            capacity_w: 0.0,
            degraded: false,
        })
        .unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let co = eq.core_output();
        assert_eq!(
            co.state.operating_mode,
            Some(OperatingMode::Standby),
            "zero-capacity ideal dispatch must reconcile Heating→Standby"
        );
        hares_types::validate_core_contract(eq.descriptor(), eq.core_output())
            .expect("validate_core_contract");
    }

    #[test]
    fn electric_baseboard_real_power_bit_identical_with_and_without_reactive_zip() {
        let cfg_pf = config(3_000.0);
        let mut cfg_nopf = config(3_000.0);
        cfg_nopf.zip = Some(hares_types::zip::ZipLoad::constant_power());

        let env_base = env(18.0);
        let mut eq_pf = ElectricBaseboard::new(cfg_pf.clone());
        let mut eq_nopf = ElectricBaseboard::new(cfg_nopf.clone());
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
}
