//! Ideal HVAC equipment with solver-provided capacity.

use std::borrow::Cow;
use std::time::Duration;

use chrono::{DateTime, FixedOffset};
use hares_types::{
    ControlCapabilities, ControlSignal, CoreCapabilities, CoreFlows, CoreOutput, CoreState,
    ElectricPower, EndUse, EnvironmentState, EquipmentDescriptor, EquipmentId, ExecutionStage,
    FuelType, HaresError, IdealCapacityMode, OperatingMode, PortContribution, PortDeclaration,
    PortSlots, ScheduleSource, Telemetry, TelemetryField, ThermalCategory, ZoneId,
};
use serde::{Deserialize, Serialize};

use hares_types::telemetry_keys as tk;

use crate::hvac::heating_config::IdealHvacConfig;
use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_postcard, save_postcard};

use super::hvac_core::IDEAL_CAPACITY_TIME_RES_THRESHOLD_S;
use super::thermostat::{
    ThermostatConfig, ThermostatMode, is_cycle_change_allowed, lookup_zone_temp,
};
use super::{
    RuntimeSetpointOverride, ScheduleSetpoints, ThermalSetpoints,
    core_config::{build_setpoint_source, extract_numeric, extract_text},
    helpers::{equipment_id_from_config, operating_mode_code, zone_id_from_config},
};

pub struct IdealHvac {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,
    core_output: CoreOutput,
    zone_id: ZoneId,
    thermostat: ThermostatConfig,
    static_setpoints: ThermalSetpoints,
    heating_setpoint_source: Option<ScheduleSource>,
    cooling_setpoint_source: Option<ScheduleSource>,
    schedule_setpoints: Option<ScheduleSetpoints>,
    runtime_setpoints: Option<RuntimeSetpointOverride>,
    ideal_capacity_w: f64,
    current_target_c: f64,
    mode: ThermostatMode,
    ideal_capacity_mode: IdealCapacityMode,
    rated_capacity_w: f64,
    cooling_capacity_w: f64,
    is_variable_speed: bool,
    last_mode_switch_at: Option<DateTime<FixedOffset>>,
    mode_start_at: Option<DateTime<FixedOffset>>,
    min_on_time_s: f64,
    min_off_time_s: f64,
    use_ideal_cached: bool,
    load_fraction: f64,
    last_sim_time: Option<DateTime<FixedOffset>>,
    /// Cooling sensible heat ratio (fraction of capacity that is sensible).
    /// Used to split ideal cooling capacity into sensible and latent components.
    /// Defaults to 1.0 (no latent); set from config "shr" key.
    shr: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct IdealHvacState {
    runtime_setpoints: Option<RuntimeSetpointOverride>,
    ideal_capacity_w: f64,
    current_target_c: f64,
    mode: ThermostatMode,
    last_mode_switch_at: Option<DateTime<FixedOffset>>,
    mode_start_at: Option<DateTime<FixedOffset>>,
    load_fraction: f64,
    last_sim_time: Option<DateTime<FixedOffset>>,
}

impl IdealHvac {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let zone = zone_id_from_config(&config).unwrap_or(ZoneId(1));
        let descriptor = EquipmentDescriptor {
            id: EquipmentId(equipment_id_from_config(&config).unwrap_or(0)),
            name: config.name,
            end_use: EndUse::HVAC_HEATING,
            equipment_type: Cow::Borrowed("Ideal HVAC"),
            zone: Some(zone),
            fuel: FuelType::Electric,
            stage: ExecutionStage::Thermal,
            control_capabilities: ControlCapabilities::IDEAL_CAPACITY
                | ControlCapabilities::IDEAL_CAPACITY_MODE_OVERRIDE
                | ControlCapabilities::THERMAL_SETPOINT
                | ControlCapabilities::THERMAL_SETPOINT_DELTA
                | ControlCapabilities::MODE_OVERRIDE,
            core_capabilities: CoreCapabilities::ELECTRIC | CoreCapabilities::HAS_MODE,
            telemetry_fields: ideal_hvac_telemetry_fields(),
        };

        Self {
            descriptor,
            ports: vec![PortDeclaration::thermal(zone)],
            telemetry: ideal_hvac_default_telemetry(),
            core_output: CoreOutput::default(),
            zone_id: zone,
            thermostat: ThermostatConfig::default(),
            static_setpoints: ThermalSetpoints {
                heating_c: 20.0,
                cooling_c: 24.0,
            },
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
            schedule_setpoints: None,
            runtime_setpoints: None,
            ideal_capacity_w: 0.0,
            current_target_c: 20.0,
            mode: ThermostatMode::Deadband,
            ideal_capacity_mode: IdealCapacityMode::default(),
            rated_capacity_w: 10_000.0,
            cooling_capacity_w: 10_000.0,
            is_variable_speed: false,
            last_mode_switch_at: None,
            mode_start_at: None,
            min_on_time_s: 0.0,
            min_off_time_s: 0.0,
            use_ideal_cached: true,
            load_fraction: 1.0,
            last_sim_time: None,
            shr: 1.0,
        }
    }

    pub fn with_setpoints(
        mut self,
        heating_weekday: [f64; 24],
        heating_weekend: [f64; 24],
        cooling_weekday: [f64; 24],
        cooling_weekend: [f64; 24],
    ) -> Self {
        self.heating_setpoint_source = Some(ScheduleSource::DailyProfile {
            weekday: heating_weekday,
            weekend: heating_weekend,
            month_multipliers: [1.0; 12],
            max_value: 1.0,
        });
        self.cooling_setpoint_source = Some(ScheduleSource::DailyProfile {
            weekday: cooling_weekday,
            weekend: cooling_weekend,
            month_multipliers: [1.0; 12],
            max_value: 1.0,
        });
        self
    }

    pub fn with_heating_setpoint_source(mut self, source: ScheduleSource) -> Self {
        self.heating_setpoint_source = Some(source);
        self
    }

    pub fn with_cooling_setpoint_source(mut self, source: ScheduleSource) -> Self {
        self.cooling_setpoint_source = Some(source);
        self
    }

    pub fn with_ideal_capacity_mode(mut self, mode: IdealCapacityMode) -> Self {
        self.ideal_capacity_mode = mode;
        self
    }

    fn effective_setpoints(&self) -> ThermalSetpoints {
        self.static_setpoints
            .with_schedule_override(self.schedule_setpoints)
            .with_control_override(self.runtime_setpoints)
    }

    fn resolve_schedule_setpoints(&mut self, env: &EnvironmentState) {
        if self.heating_setpoint_source.is_none() && self.cooling_setpoint_source.is_none() {
            return;
        }

        let heating_c = self
            .heating_setpoint_source
            .as_mut()
            .and_then(|source| source.value_at(env).ok());
        let cooling_c = self
            .cooling_setpoint_source
            .as_mut()
            .and_then(|source| source.value_at(env).ok());

        if heating_c.is_some() || cooling_c.is_some() {
            self.schedule_setpoints = Some(ScheduleSetpoints {
                heating_c,
                cooling_c,
                ..ScheduleSetpoints::default()
            });
        }
    }

    fn use_ideal_capacity(&self, env: &EnvironmentState) -> bool {
        match self.ideal_capacity_mode {
            IdealCapacityMode::On => true,
            IdealCapacityMode::Off => false,
            IdealCapacityMode::Auto => {
                let time_res_s = env.time_res.num_seconds();
                time_res_s >= IDEAL_CAPACITY_TIME_RES_THRESHOLD_S || self.is_variable_speed
            }
        }
    }

    fn set_mode(&mut self, mode: ThermostatMode, when: DateTime<FixedOffset>) {
        if self.mode != mode {
            self.mode = mode;
            self.last_mode_switch_at = Some(when);
            self.mode_start_at = Some(when);
            if mode == ThermostatMode::Deadband {
                self.ideal_capacity_w = 0.0;
            }
        }
    }

    fn can_transition_mode(&self, proposed: ThermostatMode, now: DateTime<FixedOffset>) -> bool {
        if self.mode == proposed {
            return true;
        }
        let Some(start) = self.mode_start_at else {
            return true;
        };
        let elapsed_s = (now - start).num_milliseconds().max(0) as f64 / 1000.0;
        let current_is_on = self.mode != ThermostatMode::Deadband;
        let min_s = if current_is_on {
            self.min_on_time_s
        } else {
            self.min_off_time_s
        };
        elapsed_s >= min_s
    }

    fn update_mode(&mut self, env: &EnvironmentState) -> crate::Result<ThermostatMode> {
        self.resolve_schedule_setpoints(env);
        let zone_temp = lookup_zone_temp(env, self.zone_id)?;
        let setpoints = self.effective_setpoints();

        // Always track schedule setpoint changes even during min-cycle lockout.
        // The solver needs current_target_c to reflect the live setpoint.
        match self.mode {
            ThermostatMode::Heating => self.current_target_c = setpoints.heating_c,
            ThermostatMode::Cooling => self.current_target_c = setpoints.cooling_c,
            ThermostatMode::Deadband => {}
        }

        if !is_cycle_change_allowed(&self.thermostat, self.last_mode_switch_at, env.current_time) {
            return Ok(self.mode);
        }

        let hysteresis = self.thermostat.hysteresis_c;
        let offset = self.thermostat.deadband_offset.clamp(0.0, 1.0);
        let cutout = self.thermostat.cutout_ratio;

        let next_mode = if offset > 0.0 {
            match self.mode {
                ThermostatMode::Heating => {
                    let turn_off = setpoints.heating_c + hysteresis * offset;
                    if zone_temp > turn_off {
                        ThermostatMode::Deadband
                    } else {
                        ThermostatMode::Heating
                    }
                }
                ThermostatMode::Cooling => {
                    let turn_off = setpoints.cooling_c - hysteresis * offset;
                    if zone_temp < turn_off {
                        ThermostatMode::Deadband
                    } else {
                        ThermostatMode::Cooling
                    }
                }
                ThermostatMode::Deadband => {
                    let heat_turn_on = setpoints.heating_c - hysteresis * (1.0 - offset);
                    let cool_turn_on = setpoints.cooling_c + hysteresis * (1.0 - offset);
                    if zone_temp < heat_turn_on {
                        ThermostatMode::Heating
                    } else if zone_temp > cool_turn_on {
                        ThermostatMode::Cooling
                    } else {
                        ThermostatMode::Deadband
                    }
                }
            }
        } else {
            match self.mode {
                ThermostatMode::Heating => {
                    if zone_temp > setpoints.heating_c + hysteresis * cutout {
                        ThermostatMode::Deadband
                    } else {
                        ThermostatMode::Heating
                    }
                }
                ThermostatMode::Cooling => {
                    if zone_temp < setpoints.cooling_c - hysteresis * cutout {
                        ThermostatMode::Deadband
                    } else {
                        ThermostatMode::Cooling
                    }
                }
                ThermostatMode::Deadband => {
                    if zone_temp < setpoints.heating_c - hysteresis {
                        ThermostatMode::Heating
                    } else if zone_temp > setpoints.cooling_c + hysteresis {
                        ThermostatMode::Cooling
                    } else {
                        ThermostatMode::Deadband
                    }
                }
            }
        };

        if self.can_transition_mode(next_mode, env.current_time) {
            self.set_mode(next_mode, env.current_time);
        }

        // Always update target from current setpoints — even when mode doesn't
        // transition, the schedule setpoint may have changed (day↔night shift).
        match self.mode {
            ThermostatMode::Heating => self.current_target_c = setpoints.heating_c,
            ThermostatMode::Cooling => self.current_target_c = setpoints.cooling_c,
            ThermostatMode::Deadband => {}
        }

        Ok(self.mode)
    }
}

impl Equipment for IdealHvac {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        #[cfg(test)]
        let prefer_raw = !config.is_typed() || config.raw_data().is_some();
        #[cfg(not(test))]
        let prefer_raw = !config.is_typed();
        if !prefer_raw {
            let typed = config.require_typed::<IdealHvacConfig>("Ideal HVAC")?;
            if let Some(v) = typed.heating_capacity_w {
                self.rated_capacity_w = v.max(0.0);
            }
            if let Some(v) = typed.cooling_capacity_w {
                self.cooling_capacity_w = v.max(0.0);
            }
            if let Some(shr) = typed.shr {
                self.shr = shr.clamp(0.0, 1.0);
            }
            if let Some(fraction) = typed
                .fraction_heating_load_served
                .or(typed.fraction_cooling_load_served)
            {
                self.load_fraction = fraction.clamp(0.0, 1.0);
            }
        }

        if let Some(zone) = extract_numeric(config, "zone_id") {
            let zone = ZoneId(zone as u16);
            self.zone_id = zone;
            self.descriptor.zone = Some(zone);
            if let Some(thermal_port) = self.ports.first_mut() {
                thermal_port.zone = Some(zone);
            }
        }
        if let Some(heating_sp) = extract_numeric(config, "heating_setpoint_c") {
            self.static_setpoints.heating_c = heating_sp;
        }
        if let Some(cooling_sp) = extract_numeric(config, "cooling_setpoint_c") {
            self.static_setpoints.cooling_c = cooling_sp;
        }
        if let Some(cap) = extract_numeric(config, "capacity_w") {
            self.rated_capacity_w = cap.max(0.0);
            self.cooling_capacity_w = cap.max(0.0);
        }
        if let Some(cool_cap) = extract_numeric(config, "cooling_capacity_w") {
            self.cooling_capacity_w = cool_cap.max(0.0);
        }
        if let Some(deadband) = extract_numeric(config, "deadband_c") {
            self.thermostat.hysteresis_c = deadband.max(0.0);
        }
        if let Some(n_speeds) = extract_numeric(config, "n_speeds") {
            self.is_variable_speed = n_speeds >= 2.0;
        }
        if let Some(mode) = extract_text(config, "ideal_capacity_mode") {
            self.ideal_capacity_mode = match mode.trim().to_ascii_lowercase().as_str() {
                "on" => IdealCapacityMode::On,
                "off" => IdealCapacityMode::Off,
                _ => IdealCapacityMode::Auto,
            };
        }
        if let Some(source) = build_setpoint_source(config, "heating") {
            self.heating_setpoint_source = Some(source);
        }
        if let Some(source) = build_setpoint_source(config, "cooling") {
            self.cooling_setpoint_source = Some(source);
        }

        self.effective_setpoints()
            .validate_for_deadband(self.thermostat.hysteresis_c)?;
        self.thermostat.validate(env)?;
        self.telemetry = ideal_hvac_default_telemetry();
        self.core_output = CoreOutput::default();
        Ok(())
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        self.use_ideal_cached = self.use_ideal_capacity(env);
        self.last_sim_time = Some(env.current_time);

        let mode = self.update_mode(env).unwrap_or(ThermostatMode::Deadband);
        match mode {
            ThermostatMode::Heating => OperatingMode::Heating,
            ThermostatMode::Cooling => OperatingMode::Cooling,
            ThermostatMode::Deadband => OperatingMode::Off,
        }
    }

    fn step(
        &mut self,
        _env: &EnvironmentState,
        _dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        let capacity_w = match self.mode {
            ThermostatMode::Deadband => 0.0,
            ThermostatMode::Heating if self.use_ideal_cached => {
                // Clamp: non-negative (one-step-stale estimate may go negative) and
                // no more than rated capacity (solver has no upper bound).
                (self.ideal_capacity_w * self.load_fraction)
                    .max(0.0)
                    .min(self.rated_capacity_w)
            }
            ThermostatMode::Cooling if self.use_ideal_cached => {
                // Clamp: non-positive and no more than rated cooling capacity in magnitude.
                (self.ideal_capacity_w * self.load_fraction)
                    .min(0.0)
                    .max(-self.cooling_capacity_w)
            }
            ThermostatMode::Heating => self.rated_capacity_w * self.load_fraction,
            ThermostatMode::Cooling => -self.cooling_capacity_w * self.load_fraction,
        };

        // Update end_use to reflect actual operating mode (D4: was hardcoded to HVAC_HEATING).
        self.descriptor.end_use = if capacity_w >= 0.0 {
            EndUse::HVAC_HEATING
        } else {
            EndUse::HVAC_COOLING
        };

        if capacity_w.abs() > 0.0 {
            let (sensible_w, latent_w, category) = if capacity_w > 0.0 {
                // Heating: all sensible, no latent.
                (capacity_w, 0.0, ThermalCategory::HvacHeating)
            } else {
                // Cooling: split by SHR. Latent removes moisture (negative = cooling).
                let sensible = capacity_w * self.shr;
                let latent = capacity_w * (1.0 - self.shr);
                (sensible, latent, ThermalCategory::HvacCooling)
            };
            ports.accumulate(&PortContribution::Thermal {
                zone: self.zone_id,
                sensible_gain_w: sensible_w,
                latent_gain_w: latent_w,
                category,
            })?;
        }

        self.telemetry.set(tk::THERMAL_OUTPUT_W, capacity_w);
        self.telemetry.set(
            tk::OPERATING_MODE,
            operating_mode_code(match self.mode {
                ThermostatMode::Heating => OperatingMode::Heating,
                ThermostatMode::Cooling => OperatingMode::Cooling,
                ThermostatMode::Deadband => OperatingMode::Off,
            }),
        );
        self.telemetry
            .set(tk::IDEAL_CAPACITY_W, self.ideal_capacity_w);
        self.telemetry
            .set(tk::CURRENT_TARGET_C, self.current_target_c);
        let operating_mode = match self.mode {
            ThermostatMode::Heating => OperatingMode::Heating,
            ThermostatMode::Cooling => OperatingMode::Cooling,
            ThermostatMode::Deadband => OperatingMode::Off,
        };
        self.core_output = CoreOutput {
            flows: CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(0.0)),
                reactive_power_kvar: None,
                fuel_w: None,
            },
            state: CoreState {
                operating_mode: Some(operating_mode),
                soc: None,
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

    fn save_state(&self) -> Vec<u8> {
        save_postcard(&IdealHvacState {
            runtime_setpoints: self.runtime_setpoints,
            ideal_capacity_w: self.ideal_capacity_w,
            current_target_c: self.current_target_c,
            mode: self.mode,
            last_mode_switch_at: self.last_mode_switch_at,
            mode_start_at: self.mode_start_at,
            load_fraction: self.load_fraction,
            last_sim_time: self.last_sim_time,
        })
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let decoded: IdealHvacState = load_postcard(state)?;
        self.runtime_setpoints = decoded.runtime_setpoints;
        self.ideal_capacity_w = decoded.ideal_capacity_w;
        self.current_target_c = decoded.current_target_c;
        self.mode = decoded.mode;
        self.last_mode_switch_at = decoded.last_mode_switch_at;
        self.mode_start_at = decoded.mode_start_at;
        self.load_fraction = decoded.load_fraction;
        self.last_sim_time = decoded.last_sim_time;
        self.core_output = CoreOutput::default();
        Ok(())
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        match signal {
            ControlSignal::IdealCapacity { capacity_w } => {
                self.ideal_capacity_w = *capacity_w;
            }
            ControlSignal::ThermalSetpoint {
                heating_setpoint_c,
                cooling_setpoint_c,
                deadband_c,
            } => {
                self.runtime_setpoints = Some(RuntimeSetpointOverride {
                    heating_c: *heating_setpoint_c,
                    cooling_c: *cooling_setpoint_c,
                });
                if let Some(db) = deadband_c {
                    if db.is_finite() && *db >= 0.0 {
                        self.thermostat.hysteresis_c = *db;
                    }
                }
            }
            ControlSignal::ModeOverride { mode } => {
                if *mode == OperatingMode::Off {
                    if let Some(sim_time) = self.last_sim_time {
                        self.set_mode(ThermostatMode::Deadband, sim_time);
                    } else {
                        self.mode = ThermostatMode::Deadband;
                        self.ideal_capacity_w = 0.0;
                    }
                }
            }
            ControlSignal::ThermalSetpointDelta {
                heating_delta_c,
                cooling_delta_c,
            } => {
                let base = self
                    .static_setpoints
                    .with_schedule_override(self.schedule_setpoints);
                let prior = self.runtime_setpoints.unwrap_or_default();
                self.runtime_setpoints = Some(RuntimeSetpointOverride {
                    heating_c: heating_delta_c
                        .map(|d| base.heating_c + d)
                        .or(prior.heating_c),
                    cooling_c: cooling_delta_c
                        .map(|d| base.cooling_c + d)
                        .or(prior.cooling_c),
                });
            }
            ControlSignal::IdealCapacityModeOverride { mode } => {
                self.ideal_capacity_mode = *mode;
            }
            ControlSignal::LoadFraction { fraction } => {
                self.load_fraction = fraction.clamp(0.0, 1.0);
                if *fraction <= 0.0 {
                    if let Some(sim_time) = self.last_sim_time {
                        self.set_mode(ThermostatMode::Deadband, sim_time);
                    } else {
                        self.mode = ThermostatMode::Deadband;
                    }
                    self.ideal_capacity_w = 0.0;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn ideal_target(&self) -> Option<(ZoneId, f64)> {
        if self.mode == ThermostatMode::Deadband || !self.use_ideal_cached {
            return None;
        }
        Some((self.zone_id, self.current_target_c))
    }
}

pub fn register_with_registry(registry: &mut EquipmentRegistry) {
    registry.register(
        "Ideal HVAC",
        Box::new(|config| Box::new(IdealHvac::new(config))),
    );
}

fn ideal_hvac_default_telemetry() -> Telemetry {
    let mut telemetry = Telemetry::with_capacity(4);
    telemetry.insert(tk::THERMAL_OUTPUT_W, 0.0);
    telemetry.insert(tk::OPERATING_MODE, 0.0);
    telemetry.insert(tk::IDEAL_CAPACITY_W, 0.0);
    telemetry.insert(tk::CURRENT_TARGET_C, 0.0);
    telemetry
}

fn ideal_hvac_telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField {
            name: tk::THERMAL_OUTPUT_W.to_string(),
            unit: "W".to_string(),
            description: "Thermal power delivered to zone (positive=heating, negative=cooling)"
                .to_string(),
        },
        TelemetryField {
            name: tk::OPERATING_MODE.to_string(),
            unit: "enum".to_string(),
            description: "Operating mode: 0=Off, 1=Heating, 2=Cooling".to_string(),
        },
        TelemetryField {
            name: tk::IDEAL_CAPACITY_W.to_string(),
            unit: "W".to_string(),
            description: "Ideal capacity from solver (positive=heating, negative=cooling)"
                .to_string(),
        },
        TelemetryField {
            name: tk::CURRENT_TARGET_C.to_string(),
            unit: "C".to_string(),
            description: "Current setpoint target temperature".to_string(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        ControlCapabilities, DomainUpdate, EnvironmentState, GridState, PortSlots,
        SCHEDULE_DOMAIN_ID, ScheduleSource, SurfaceIrradiance, ThermalAccumulator, WeatherState,
        ZoneId, ZoneState,
    };

    use super::{IdealHvac, register_with_registry};
    use crate::config::ConfigPayload;
    use crate::{Equipment, EquipmentConfig, EquipmentRegistry};

    fn env(zone_temp_c: f64, time_res_s: i64, second: i64) -> EnvironmentState {
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
                outdoor_temp_c: 8.3,
                outdoor_humidity_ratio: 0.005,
                wind_speed_m_s: 2.0,
                wind_dir_deg: 0.0,
                ground_temp_c: 12.0,
                sky_temp_c: 8.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![SurfaceIrradiance {
                    surface_id: 1,
                    direct_w_m2: 0.0,
                    diffuse_w_m2: 0.0,
                    reflected_w_m2: 0.0,
                    angle_of_incidence_rad: 0.0,
                }],
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
                .expect("valid")
                + ChronoDuration::seconds(second),
            time_res: ChronoDuration::seconds(time_res_s),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    fn config(name: &str) -> EquipmentConfig {
        EquipmentConfig::from_typed(
            name.to_string(),
            "Ideal HVAC".to_string(),
            crate::IdealHvacConfig::default(),
        )
    }

    #[test]
    fn ideal_hvac_heating_turns_on_and_writes_capacity() {
        let mut cfg = config("IH");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".into(), 1.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("cooling_setpoint_c".into(), 26.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("capacity_w".into(), 10_000.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("ideal_capacity_mode".into(), "on".into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(18.0, 60, 0);
        eq.init(&cfg, &env).unwrap();

        let mode = eq.update_control(&env);
        assert_eq!(mode, hares_types::OperatingMode::Heating);

        let target = eq.ideal_target();
        assert!(target.is_some());
        let (zone, temp) = target.unwrap();
        assert_eq!(zone, ZoneId(1));
        assert!((temp - 20.0).abs() < 1e-9);
    }

    #[test]
    fn ideal_hvac_cooling_turns_on_when_zone_hot() {
        let mut cfg = config("IH");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".into(), 1.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("cooling_setpoint_c".into(), 24.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("ideal_capacity_mode".into(), "on".into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(28.0, 60, 0);
        eq.init(&cfg, &env).unwrap();

        let mode = eq.update_control(&env);
        assert_eq!(mode, hares_types::OperatingMode::Cooling);

        let target = eq.ideal_target();
        assert!(target.is_some());
    }

    #[test]
    fn ideal_hvac_deadband_returns_none_target() {
        let mut cfg = config("IH");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".into(), 1.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("cooling_setpoint_c".into(), 26.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(22.0, 60, 0);
        eq.init(&cfg, &env).unwrap();

        let mode = eq.update_control(&env);
        assert_eq!(mode, hares_types::OperatingMode::Off);

        let target = eq.ideal_target();
        assert!(target.is_none());
    }

    #[test]
    fn ideal_hvac_accepts_ideal_capacity_signal() {
        let mut cfg = config("IH");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".into(), 1.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(18.0, 300, 0);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);

        let signal = hares_types::ControlSignal::IdealCapacity { capacity_w: 5000.0 };
        eq.apply_control(&signal).unwrap();
        assert!((eq.ideal_capacity_w - 5000.0).abs() < 1e-9);
    }

    #[test]
    fn ideal_hvac_accepts_thermal_setpoint_signal() {
        let mut cfg = config("IH");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".into(), 1.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(18.0, 60, 0);
        eq.init(&cfg, &env).unwrap();

        let signal = hares_types::ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(19.0),
            cooling_setpoint_c: Some(25.0),
            deadband_c: None,
        };
        eq.apply_control(&signal).unwrap();

        assert!(eq.runtime_setpoints.is_some());
        let sp = eq.runtime_setpoints.unwrap();
        assert_eq!(sp.heating_c, Some(19.0));
        assert_eq!(sp.cooling_c, Some(25.0));
    }

    #[test]
    fn ideal_hvac_mode_override_forces_off() {
        let mut cfg = config("IH");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".into(), 1.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(18.0, 60, 0);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);

        let signal = hares_types::ControlSignal::ModeOverride {
            mode: hares_types::OperatingMode::Off,
        };
        eq.apply_control_unchecked(&signal).unwrap();
    }

    #[test]
    fn ideal_hvac_step_writes_ideal_capacity_to_thermal_port() {
        let mut cfg = config("IH");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".into(), 1.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("capacity_w".into(), 10_000.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(18.0, 300, 0);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);

        eq.ideal_capacity_w = 3500.0;

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert!((ports.thermal[0].sensible_gain_w - 3500.0).abs() < 1e-9);
    }

    #[test]
    fn ideal_hvac_cooling_step_writes_negative_to_thermal_port() {
        let mut cfg = config("IH");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".into(), 1.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("cooling_setpoint_c".into(), 24.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("cooling_capacity_w".into(), 8_000.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(28.0, 300, 0);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);

        eq.ideal_capacity_w = -5000.0;

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert!(
            ports.thermal[0].sensible_gain_w < 0.0,
            "cooling should write negative value, got {}",
            ports.thermal[0].sensible_gain_w
        );
        assert!((ports.thermal[0].sensible_gain_w - (-5000.0)).abs() < 1e-9);
    }

    #[test]
    fn ideal_hvac_state_round_trips() {
        let mut cfg = config("IH");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".into(), 1.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("capacity_w".into(), 10_000.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(18.0, 60, 0);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);
        eq.ideal_capacity_w = 4200.0;

        let state = eq.save_state();
        let mut restored = IdealHvac::new(cfg.clone());
        restored.init(&cfg, &env).unwrap();
        restored.load_state(&state).unwrap();

        assert!((restored.ideal_capacity_w - 4200.0).abs() < 1e-9);
    }

    #[test]
    fn ideal_hvac_descriptor_has_correct_capabilities() {
        let cfg = config("IH");
        let eq = IdealHvac::new(cfg);
        let caps = eq.descriptor().control_capabilities;

        assert!(caps.contains(ControlCapabilities::IDEAL_CAPACITY));
        assert!(caps.contains(ControlCapabilities::THERMAL_SETPOINT));
        assert!(caps.contains(ControlCapabilities::MODE_OVERRIDE));
        assert!(!caps.contains(ControlCapabilities::POWER_SETPOINT));
    }

    #[test]
    fn registry_includes_ideal_hvac() {
        let registry = EquipmentRegistry::new();
        assert!(registry.get("Ideal HVAC").is_some());
    }

    #[test]
    fn ideal_capacity_mode_auto_engages_at_coarse_timestep() {
        let mut cfg = config("IH");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".into(), 1.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("ideal_capacity_mode".into(), "auto".into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env_coarse = env(18.0, 300, 0);
        eq.init(&cfg, &env_coarse).unwrap();

        assert!(eq.use_ideal_capacity(&env_coarse));

        let env_fine = env(18.0, 60, 0);
        assert!(!eq.use_ideal_capacity(&env_fine));
    }

    #[test]
    fn ideal_capacity_mode_on_always_uses_ideal() {
        let mut cfg = config("IH");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".into(), 1.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("ideal_capacity_mode".into(), "on".into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env_fine = env(18.0, 60, 0);
        eq.init(&cfg, &env_fine).unwrap();

        assert!(eq.use_ideal_capacity(&env_fine));
    }

    #[test]
    fn ideal_capacity_mode_off_never_uses_ideal() {
        let mut cfg = config("IH");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".into(), 1.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("ideal_capacity_mode".into(), "off".into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env_coarse = env(18.0, 300, 0);
        eq.init(&cfg, &env_coarse).unwrap();

        assert!(!eq.use_ideal_capacity(&env_coarse));
    }

    #[test]
    fn ideal_capacity_mode_off_returns_none_from_ideal_target() {
        let mut cfg = config("IH");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".into(), 1.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("ideal_capacity_mode".into(), "off".into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("cooling_setpoint_c".into(), 26.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(18.0, 300, 0);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);

        assert!(eq.mode != super::ThermostatMode::Deadband);
        assert!(
            eq.ideal_target().is_none(),
            "ideal_target should return None when mode is Off"
        );
    }

    #[test]
    fn ideal_capacity_mode_off_step_uses_rated_capacity() {
        let mut cfg = config("IH");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".into(), 1.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("ideal_capacity_mode".into(), "off".into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("cooling_setpoint_c".into(), 26.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("capacity_w".into(), 10_000.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(18.0, 300, 0);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);

        eq.ideal_capacity_w = 5000.0;

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert!(
            (ports.thermal[0].sensible_gain_w - 10_000.0).abs() < 1e-9,
            "should use rated capacity, not ideal capacity"
        );
    }

    #[test]
    fn variable_speed_forces_ideal_in_auto_mode() {
        let mut cfg = config("IH");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".into(), 1.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("n_speeds".into(), 4.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env_fine = env(18.0, 60, 0);
        eq.init(&cfg, &env_fine).unwrap();

        assert!(eq.is_variable_speed);
        assert!(eq.use_ideal_capacity(&env_fine));
    }

    #[test]
    fn load_fraction_zero_forces_off() {
        let mut cfg = config("IH");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".into(), 1.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(18.0, 60, 0);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);
        eq.ideal_capacity_w = 5000.0;

        let signal = hares_types::ControlSignal::LoadFraction { fraction: 0.0 };
        eq.apply_control_unchecked(&signal).unwrap();

        assert!((eq.ideal_capacity_w - 0.0).abs() < 1e-9);
    }

    #[test]
    fn load_fraction_partial_scales_capacity() {
        let mut cfg = config("IH");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".into(), 1.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("cooling_setpoint_c".into(), 24.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("heating_setpoint_c".into(), 20.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(28.0, 300, 0);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);

        eq.ideal_capacity_w = -5000.0;

        let signal = hares_types::ControlSignal::LoadFraction { fraction: 0.5 };
        eq.apply_control_unchecked(&signal).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert!(
            (ports.thermal[0].sensible_gain_w - (-2500.0)).abs() < 1e-9,
            "capacity should be scaled by load fraction 0.5"
        );
    }

    #[test]
    fn column_ref_setpoint_source_reads_schedule_domain() {
        let mut cfg = config("IH");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".into(), 1.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("heating_setpoint_schedule_col".into(), 0.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("cooling_setpoint_schedule_col".into(), 1.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let mut env = env(18.0, 60, 0);
        env.custom_domains = vec![DomainUpdate {
            domain_id: SCHEDULE_DOMAIN_ID,
            zone_temperatures_c: vec![],
            custom_payload: Some(vec![19.5, 26.0]),
        }];

        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);

        let setpoints = eq.effective_setpoints();
        assert!((setpoints.heating_c - 19.5).abs() < 1e-9);
        assert!((setpoints.cooling_c - 26.0).abs() < 1e-9);
    }

    #[test]
    fn with_setpoint_source_builder_method() {
        let cfg = config("IH");
        let eq = IdealHvac::new(cfg)
            .with_heating_setpoint_source(ScheduleSource::Constant(21.0))
            .with_cooling_setpoint_source(ScheduleSource::Constant(25.0));

        assert!(eq.heating_setpoint_source.is_some());
        assert!(eq.cooling_setpoint_source.is_some());
    }

    #[test]
    fn daily_profile_setpoints_vary_by_hour() {
        let mut heating_weekday = [20.0; 24];
        heating_weekday[8] = 18.0;
        heating_weekday[22] = 19.0;
        let mut cooling_weekday = [26.0; 24];
        cooling_weekday[14] = 24.0;

        let cfg = config("IH");
        let eq = IdealHvac::new(cfg).with_setpoints(
            heating_weekday,
            heating_weekday,
            cooling_weekday,
            cooling_weekday,
        );

        assert!(eq.heating_setpoint_source.is_some());
        assert!(eq.cooling_setpoint_source.is_some());
    }

    #[test]
    fn mode_override_off_is_recoverable() {
        let mut cfg = config("IH");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".into(), 1.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("cooling_setpoint_c".into(), 26.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(18.0, 60, 0);
        eq.init(&cfg, &env).unwrap();

        let mode = eq.update_control(&env);
        assert_eq!(mode, hares_types::OperatingMode::Heating);

        eq.apply_control_unchecked(&hares_types::ControlSignal::ModeOverride {
            mode: hares_types::OperatingMode::Off,
        })
        .unwrap();
        assert!(eq.ideal_target().is_none());

        let mode = eq.update_control(&env);
        assert_eq!(
            mode,
            hares_types::OperatingMode::Heating,
            "should recover to heating after mode override clears"
        );
    }

    #[test]
    fn mode_override_off_before_first_step_clears_capacity() {
        let cfg = config("IH");
        let mut eq = IdealHvac::new(cfg);

        // Before init/step, last_sim_time is None. Set a stale capacity.
        eq.ideal_capacity_w = 5000.0;

        eq.apply_control_unchecked(&hares_types::ControlSignal::ModeOverride {
            mode: hares_types::OperatingMode::Off,
        })
        .unwrap();

        assert_eq!(
            eq.ideal_capacity_w, 0.0,
            "ModeOverride(Off) before first step must clear ideal_capacity_w"
        );
    }

    #[test]
    fn ideal_capacity_mode_override_switches_at_runtime() {
        let mut cfg = config("IH");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".into(), 1.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("ideal_capacity_mode".into(), "off".into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("cooling_setpoint_c".into(), 26.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let environment = env(18.0, 60, 0);
        eq.init(&cfg, &environment).unwrap();

        // Initially off — should NOT use ideal capacity
        assert!(!eq.use_ideal_capacity(&environment));

        // Override to On at runtime
        eq.apply_control_unchecked(&hares_types::ControlSignal::IdealCapacityModeOverride {
            mode: hares_types::IdealCapacityMode::On,
        })
        .unwrap();
        assert!(eq.use_ideal_capacity(&environment));

        // Override back to Off
        eq.apply_control_unchecked(&hares_types::ControlSignal::IdealCapacityModeOverride {
            mode: hares_types::IdealCapacityMode::Off,
        })
        .unwrap();
        assert!(!eq.use_ideal_capacity(&environment));
    }

    #[test]
    fn dynamic_cooling_writes_negative_rated_capacity() {
        let mut cfg = config("IH");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".into(), 1.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("ideal_capacity_mode".into(), "off".into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("cooling_setpoint_c".into(), 24.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("cooling_capacity_w".into(), 8_000.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(28.0, 60, 0);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        assert!(
            (ports.thermal[0].sensible_gain_w - (-8_000.0)).abs() < 1e-9,
            "dynamic cooling should use -cooling_capacity_w, got {}",
            ports.thermal[0].sensible_gain_w
        );
    }

    #[test]
    fn overlapping_setpoints_rejected() {
        let mut cfg = config("IH");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".into(), 1.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("heating_setpoint_c".into(), 22.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("cooling_setpoint_c".into(), 21.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("deadband_c".into(), 1.0.into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env = env(20.0, 60, 0);
        let result = eq.init(&cfg, &env);

        assert!(result.is_err(), "overlapping setpoints should be rejected");
    }

    // Bug 1: current_target_c must update when setpoint changes mid-mode.
    // DailyProfile: hour 0 = 21.67°C, hour 8 = 18.33°C. Zone is below heat
    // turn-on at both hours so no mode transition occurs. The target must still
    // follow the schedule shift.
    #[test]
    fn current_target_c_tracks_schedule_shift_without_mode_change() {
        let mut heating_weekday = [21.67f64; 24];
        heating_weekday[8] = 18.33;
        let cooling_weekday = [26.0f64; 24];

        let mut cfg = config("IH");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".into(), 1.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("ideal_capacity_mode".into(), "on".into());
        cfg.raw_config_mut().unwrap().insert(
            "heating_weekday_setpoints_c".into(),
            heating_weekday.to_vec().into(),
        );
        cfg.raw_config_mut().unwrap().insert(
            "heating_weekend_setpoints_c".into(),
            heating_weekday.to_vec().into(),
        );
        cfg.raw_config_mut().unwrap().insert(
            "cooling_weekday_setpoints_c".into(),
            cooling_weekday.to_vec().into(),
        );
        cfg.raw_config_mut().unwrap().insert(
            "cooling_weekend_setpoints_c".into(),
            cooling_weekday.to_vec().into(),
        );

        // Zone at 15°C — well below both setpoints, always triggers Heating.
        // hysteresis=1.0, deadband_offset=0.2 → heat turn-on at setpoint-0.8
        // 15 < 21.67-0.8=20.87 and 15 < 18.33-0.8=17.53
        let env_h0 = env(15.0, 60, 0);
        let mut eq = IdealHvac::new(cfg.clone());
        eq.init(&cfg, &env_h0).unwrap();
        eq.update_control(&env_h0);

        assert_eq!(eq.mode, super::ThermostatMode::Heating);
        assert!(
            (eq.current_target_c - 21.67).abs() < 1e-6,
            "hour 0 target should be 21.67, got {}",
            eq.current_target_c
        );

        // Advance to hour 8 — same zone temp, same mode, but setpoint drops.
        let env_h8 = env(15.0, 60, 8 * 3600);
        eq.update_control(&env_h8);

        assert_eq!(
            eq.mode,
            super::ThermostatMode::Heating,
            "mode must not change"
        );
        assert!(
            (eq.current_target_c - 18.33).abs() < 1e-6,
            "current_target_c should update to new schedule setpoint, got {}",
            eq.current_target_c
        );
    }

    // Bug 2: Deadband must produce 0W even when ideal_capacity_w was set while
    // heating and the transition to Deadband did not clear it explicitly via the
    // ControlSignal path.
    #[test]
    fn deadband_outputs_zero_despite_stale_ideal_capacity() {
        let mut cfg = config("IH");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".into(), 1.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("cooling_setpoint_c".into(), 26.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("ideal_capacity_mode".into(), "on".into());

        let mut eq = IdealHvac::new(cfg.clone());
        // Zone cold → enters Heating.
        let env_cold = env(18.0, 60, 0);
        eq.init(&cfg, &env_cold).unwrap();
        eq.update_control(&env_cold);
        assert_eq!(eq.mode, super::ThermostatMode::Heating);

        // Solver dispatches capacity while in Heating.
        let signal = hares_types::ControlSignal::IdealCapacity { capacity_w: 5000.0 };
        eq.apply_control_unchecked(&signal).unwrap();
        assert!((eq.ideal_capacity_w - 5000.0).abs() < 1e-9);

        // Zone warms past turn-off threshold (20.0 + 1.0*0.2 = 20.2) → Deadband.
        let env_warm = env(21.0, 60, 0);
        eq.update_control(&env_warm);
        assert_eq!(
            eq.mode,
            super::ThermostatMode::Deadband,
            "zone at 21°C should be in Deadband"
        );

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env_warm, std::time::Duration::from_secs(60), &mut ports)
            .unwrap();

        assert_eq!(
            ports.thermal[0].sensible_gain_w, 0.0,
            "Deadband must output 0W, got {}",
            ports.thermal[0].sensible_gain_w
        );
    }

    // Bug 3a: Cooling mode must clamp positive ideal_capacity_w to 0W.
    #[test]
    fn cooling_mode_clamps_positive_ideal_capacity_to_zero() {
        let mut cfg = config("IH");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".into(), 1.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("cooling_setpoint_c".into(), 24.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("ideal_capacity_mode".into(), "on".into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env_hot = env(28.0, 60, 0);
        eq.init(&cfg, &env_hot).unwrap();
        eq.update_control(&env_hot);
        assert_eq!(eq.mode, super::ThermostatMode::Cooling);

        // Solver mistakenly returns positive capacity (e.g. outdoor dropped).
        let signal = hares_types::ControlSignal::IdealCapacity { capacity_w: 3000.0 };
        eq.apply_control_unchecked(&signal).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env_hot, std::time::Duration::from_secs(60), &mut ports)
            .unwrap();

        assert_eq!(
            ports.thermal[0].sensible_gain_w, 0.0,
            "Cooling mode must not output positive capacity, got {}",
            ports.thermal[0].sensible_gain_w
        );
    }

    // Bug 3b: Heating mode must clamp negative ideal_capacity_w to 0W.
    #[test]
    fn heating_mode_clamps_negative_ideal_capacity_to_zero() {
        let mut cfg = config("IH");
        cfg.raw_config_mut()
            .unwrap()
            .insert("zone_id".into(), 1.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("heating_setpoint_c".into(), 20.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("cooling_setpoint_c".into(), 26.0.into());
        cfg.raw_config_mut()
            .unwrap()
            .insert("ideal_capacity_mode".into(), "on".into());

        let mut eq = IdealHvac::new(cfg.clone());
        let env_cold = env(18.0, 60, 0);
        eq.init(&cfg, &env_cold).unwrap();
        eq.update_control(&env_cold);
        assert_eq!(eq.mode, super::ThermostatMode::Heating);

        // Solver returns negative capacity (zone overshot, cooling needed).
        let signal = hares_types::ControlSignal::IdealCapacity {
            capacity_w: -2000.0,
        };
        eq.apply_control_unchecked(&signal).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env_cold, std::time::Duration::from_secs(60), &mut ports)
            .unwrap();

        assert_eq!(
            ports.thermal[0].sensible_gain_w, 0.0,
            "Heating mode must not output negative capacity, got {}",
            ports.thermal[0].sensible_gain_w
        );
    }
}
