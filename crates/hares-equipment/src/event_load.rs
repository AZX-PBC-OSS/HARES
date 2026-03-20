//! Event-driven (stochastic) load equipment model.

use std::borrow::Cow;
use std::time::Duration;

use hares_types::{
    BoundaryPolicy, ControlCapabilities, ControlSignal, EndUse, EnvironmentState,
    EquipmentDescriptor, EquipmentId, ExecutionStage, FluidType, FuelType, HaresError,
    OperatingMode, PortContribution, PortDeclaration, PortSlots, ScheduleSource,
    Telemetry, TelemetryField, ThermalCategory, ZoneId,
};
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::hvac::helpers::parse_fuel_type;
use crate::schedule_helpers::{
    ScheduleSourceState, capture_schedule_source_state, parse_u32, parse_usize, parse_zone_id,
    restore_schedule_source_state,
};
use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_postcard, save_postcard};

use crate::config::KEY_EQUIPMENT_ID;
const KEY_BUILDING_ID: &str = "building_id";
const KEY_MASTER_SEED: &str = "master_seed";
const KEY_N_UNITS: &str = "n_units";
const KEY_ACTIVE_POWER_KW: &str = "active_power_kw";
const KEY_ACTIVE_DURATION_S: &str = "active_duration_s";
const KEY_COOLDOWN_DURATION_S: &str = "cooldown_duration_s";
const KEY_SENSIBLE_GAIN_FRACTION: &str = "sensible_gain_fraction";
const KEY_LATENT_GAIN_FRACTION: &str = "latent_gain_fraction";
const KEY_PHASE_LEN: &str = "phase_len";
const KEY_EVENT_WINDOW_SOURCE: &str = "event_window_source";
const KEY_EVENT_WINDOW_SCHEDULE_COL: &str = "event_window_schedule_col";
const KEY_EVENT_PROBABILITY_SOURCE: &str = "event_probability_source";
const KEY_EVENT_PROBABILITY_SCHEDULE_COL: &str = "event_probability_schedule_col";
const KEY_EVENT_PROBABILITY_CONSTANT: &str = "event_probability_constant";

const KEY_HOT_WATER_DRAW_VOLUME_L: &str = "hot_water_draw_volume_l";

const PHASE_POWER_PREFIX_A: &str = "phase_";
const PHASE_POWER_SUFFIX_A: &str = "_power_kw";
const PHASE_DURATION_PREFIX_A: &str = "phase_";
const PHASE_DURATION_SUFFIX_A: &str = "_duration_s";

const PHASE_POWER_PREFIX_B: &str = "cycle_phase_";
const PHASE_POWER_SUFFIX_B: &str = "_power_kw";
const PHASE_DURATION_PREFIX_B: &str = "cycle_phase_";
const PHASE_DURATION_SUFFIX_B: &str = "_duration_s";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum ForcedMode {
    Idle,
    Active,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum EventPhase {
    Idle,
    Active,
    Cooldown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct EventBasedLoadState {
    phase: EventPhase,
    remaining_phase_s: f64,
    load_fraction: f64,
    forced_mode: Option<ForcedMode>,
    rng_seed: [u8; 32],
    rng_draws: u64,
    event_window_source_state: ScheduleSourceState,
    event_probability_source_state: ScheduleSourceState,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct WetApplianceState {
    active: bool,
    phase_index: usize,
    elapsed_in_phase_s: f64,
    load_fraction: f64,
    forced_mode: Option<ForcedMode>,
    hot_water_draw_rate_kg_s: f64,
    rng_seed: [u8; 32],
    rng_draws: u64,
    event_window_source_state: ScheduleSourceState,
    event_probability_source_state: ScheduleSourceState,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CyclePhase {
    power_kw: f64,
    duration_s: f64,
}

/// Event-based stochastic load with an Idle -> Active -> Cooldown cycle.
pub struct EventBasedLoad {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,
    fuel_type: FuelType,

    event_window_source: ScheduleSource,
    event_probability_source: ScheduleSource,

    active_power_kw: f64,
    active_duration_s: f64,
    cooldown_duration_s: f64,
    sensible_gain_fraction: f64,
    latent_gain_fraction: f64,

    phase: EventPhase,
    remaining_phase_s: f64,
    load_fraction: f64,
    forced_mode: Option<ForcedMode>,
    /// Per-step absolute power override set by `PowerSetpoint`. Cleared after each step.
    power_setpoint_override: Option<f64>,

    rng_seed: [u8; 32],
    rng_draws: u64,
    rng: ChaCha8Rng,
}

/// Multi-phase wet appliance cycle with stochastic starts.
pub struct WetAppliance {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,
    fuel_type: FuelType,

    event_window_source: ScheduleSource,
    event_probability_source: ScheduleSource,

    phases: Vec<CyclePhase>,
    n_units: f64,
    sensible_gain_fraction: f64,
    latent_gain_fraction: f64,

    active: bool,
    phase_index: usize,
    elapsed_in_phase_s: f64,
    load_fraction: f64,
    forced_mode: Option<ForcedMode>,

    hot_water_draw_rate_kg_s: f64,

    rng_seed: [u8; 32],
    rng_draws: u64,
    rng: ChaCha8Rng,
}

impl EventBasedLoad {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        let equipment_name = config.name.clone();
        let descriptor = EquipmentDescriptor {
            id: EquipmentId(parse_u32(&config.raw_config, KEY_EQUIPMENT_ID).unwrap_or_default()),
            name: equipment_name,
            end_use: EndUse::Other,
            equipment_type: Cow::Borrowed("EventBasedLoad"),
            zone: parse_zone_id(&config.raw_config),
            fuel: FuelType::Electric,
            stage: ExecutionStage::Independent,
            control_capabilities: ControlCapabilities::LOAD_FRACTION
                | ControlCapabilities::MODE_OVERRIDE
                | ControlCapabilities::POWER_SETPOINT,
            telemetry_fields: event_load_telemetry_fields(),
        };
        let ports = ports_for_zone(descriptor.zone);
        let rng_seed = derive_rng_seed(&config);
        Self {
            descriptor,
            ports,
            telemetry: default_event_load_telemetry(),
            fuel_type: FuelType::Electric,
            event_window_source: ScheduleSource::Constant(1.0),
            event_probability_source: ScheduleSource::Constant(1.0),
            active_power_kw: 0.0,
            active_duration_s: 0.0,
            cooldown_duration_s: 0.0,
            sensible_gain_fraction: 0.0,
            latent_gain_fraction: 0.0,
            phase: EventPhase::Idle,
            remaining_phase_s: 0.0,
            load_fraction: 1.0,
            forced_mode: None,
            power_setpoint_override: None,
            rng_seed,
            rng_draws: 0,
            rng: ChaCha8Rng::from_seed(rng_seed),
        }
    }

    fn maybe_start_event(&mut self, window_open: bool, probability: f64) {
        if self.phase != EventPhase::Idle {
            return;
        }
        let should_start = match self.forced_mode {
            Some(ForcedMode::Idle) => false,
            Some(ForcedMode::Active) => true,
            None => {
                if !window_open {
                    false
                } else {
                    let p = probability.clamp(0.0, 1.0);
                    if p <= 0.0 {
                        false
                    } else if p >= 1.0 {
                        true
                    } else {
                        self.rng_draws = self.rng_draws.saturating_add(1);
                        self.rng.random::<f64>() < p
                    }
                }
            }
        };

        if should_start {
            self.phase = EventPhase::Active;
            self.remaining_phase_s = self.active_duration_s;
        }
    }

    fn apply_overrides(&mut self) {
        match self.forced_mode {
            Some(ForcedMode::Idle) => {
                self.phase = EventPhase::Idle;
                self.remaining_phase_s = 0.0;
            }
            Some(ForcedMode::Active) => {
                self.phase = EventPhase::Active;
                if self.remaining_phase_s <= 0.0 {
                    self.remaining_phase_s = self.active_duration_s;
                }
            }
            None => {}
        }
    }

    fn advance_phase_timer(&mut self, dt_s: f64) {
        if dt_s <= 0.0 || matches!(self.forced_mode, Some(ForcedMode::Active)) {
            return;
        }

        let mut remaining_dt = dt_s;
        while remaining_dt > 0.0 {
            match self.phase {
                EventPhase::Idle => break,
                EventPhase::Active => {
                    if self.remaining_phase_s > remaining_dt {
                        self.remaining_phase_s -= remaining_dt;
                        break;
                    }
                    remaining_dt -= self.remaining_phase_s.max(0.0);
                    if self.cooldown_duration_s > 0.0 {
                        self.phase = EventPhase::Cooldown;
                        self.remaining_phase_s = self.cooldown_duration_s;
                    } else {
                        self.phase = EventPhase::Idle;
                        self.remaining_phase_s = 0.0;
                    }
                }
                EventPhase::Cooldown => {
                    if self.remaining_phase_s > remaining_dt {
                        self.remaining_phase_s -= remaining_dt;
                        break;
                    }
                    remaining_dt -= self.remaining_phase_s.max(0.0);
                    self.phase = EventPhase::Idle;
                    self.remaining_phase_s = 0.0;
                }
            }
        }
    }

    fn update_outputs(&mut self, ports: &mut PortSlots) -> std::result::Result<(), HaresError> {
        let active_now = self.phase == EventPhase::Active;
        // PowerSetpoint overrides the configured active_power_kw for this step.
        // The override is unconditional: it replaces the phase-based power regardless
        // of whether the equipment is currently in the Active phase.
        let active_power_kw = if let Some(override_kw) = self.power_setpoint_override.take() {
            override_kw
        } else if active_now {
            self.active_power_kw * self.load_fraction.max(0.0)
        } else {
            0.0
        };

        let is_fuel = self.fuel_type != FuelType::Electric;
        let fuel_consumption_w = if is_fuel { active_power_kw * 1_000.0 } else { 0.0 };
        let electric_power_kw = if is_fuel { 0.0 } else { active_power_kw };

        // Thermal gains come from all input energy regardless of fuel type.
        let gain_source_w = active_power_kw * 1_000.0;
        let sensible_gain_w = gain_source_w * self.sensible_gain_fraction;
        let latent_gain_w = gain_source_w * self.latent_gain_fraction;

        if electric_power_kw != 0.0 {
            ports.accumulate(&PortContribution::Electrical {
                active_power_kw: electric_power_kw,
                reactive_power_kvar: 0.0,
            })?;
        }

        if fuel_consumption_w > 0.0 {
            ports.accumulate(&PortContribution::Fuel {
                fuel_type: self.fuel_type,
                consumption_w: fuel_consumption_w,
            })?;
        }

        if let Some(zone) = self.descriptor.zone
            && (sensible_gain_w != 0.0 || latent_gain_w != 0.0)
        {
            ports.accumulate(&PortContribution::Thermal {
                zone,
                sensible_gain_w,
                latent_gain_w,
                category: ThermalCategory::InternalGain,
            })?;
        }

        self.telemetry.set("active_power_kw", electric_power_kw);
        self.telemetry.set("sensible_gain_w", sensible_gain_w);
        self.telemetry.set("latent_gain_w", latent_gain_w);
        self.telemetry.set("gas_consumption_w", fuel_consumption_w);
        self.telemetry.set("state", phase_ordinal(self.phase));
        Ok(())
    }
}

impl Equipment for EventBasedLoad {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, _env: &EnvironmentState) -> crate::Result<()> {
        let (window_source, probability_source) = parse_event_schedule_sources(config)?;
        self.event_window_source = window_source;
        self.event_probability_source = probability_source;

        self.active_power_kw = parse_non_negative(config, KEY_ACTIVE_POWER_KW)?.unwrap_or(1.0);
        self.active_duration_s = parse_positive(config, KEY_ACTIVE_DURATION_S)?.unwrap_or(900.0);
        self.cooldown_duration_s =
            parse_non_negative(config, KEY_COOLDOWN_DURATION_S)?.unwrap_or(0.0);
        // "frac_sensible" / "frac_latent" are HPXML-parsed aliases.
        self.sensible_gain_fraction = config
            .get_f64(KEY_SENSIBLE_GAIN_FRACTION)
            .or_else(|| config.get_f64("frac_sensible"))
            .unwrap_or(0.0);
        self.latent_gain_fraction = config
            .get_f64(KEY_LATENT_GAIN_FRACTION)
            .or_else(|| config.get_f64("frac_latent"))
            .unwrap_or(0.0);

        self.fuel_type = match config.get_str("fuel_type") {
            None => FuelType::Electric,
            Some(raw) => parse_fuel_type(Some(raw)).ok_or_else(|| {
                HaresError::Equipment(format!("unrecognised fuel_type: {raw}"))
            })?,
        };
        self.descriptor.fuel = self.fuel_type;

        self.phase = EventPhase::Idle;
        self.remaining_phase_s = 0.0;
        self.load_fraction = 1.0;
        self.forced_mode = None;
        self.power_setpoint_override = None;
        self.telemetry = default_event_load_telemetry();

        self.ports = ports_for_zone(self.descriptor.zone);
        if self.fuel_type != FuelType::Electric {
            self.ports.push(PortDeclaration::fuel());
        }

        self.rng_seed = derive_rng_seed(config);
        self.rng_draws = 0;
        self.rng = ChaCha8Rng::from_seed(self.rng_seed);
        Ok(())
    }

    fn update_control(&mut self, _env: &EnvironmentState) -> OperatingMode {
        match self.phase {
            EventPhase::Idle => OperatingMode::Off,
            EventPhase::Active => OperatingMode::Standby,
            EventPhase::Cooldown => OperatingMode::Off,
        }
    }

    fn step(
        &mut self,
        env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        self.apply_overrides();
        let window_open = self.event_window_source.value_at(env)? > 0.0;
        let probability = self.event_probability_source.value_at(env)?;
        self.maybe_start_event(window_open, probability);
        self.update_outputs(ports)?;
        self.advance_phase_timer(dt.as_secs_f64());
        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn save_state(&self) -> Vec<u8> {
        save_postcard(&EventBasedLoadState {
            phase: self.phase,
            remaining_phase_s: self.remaining_phase_s,
            load_fraction: self.load_fraction,
            forced_mode: self.forced_mode,
            rng_seed: self.rng_seed,
            rng_draws: self.rng_draws,
            event_window_source_state: capture_schedule_source_state(&self.event_window_source),
            event_probability_source_state: capture_schedule_source_state(
                &self.event_probability_source,
            ),
        })
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let decoded: EventBasedLoadState = load_postcard(state)?;
        self.phase = decoded.phase;
        self.remaining_phase_s = decoded.remaining_phase_s;
        self.load_fraction = decoded.load_fraction;
        self.forced_mode = decoded.forced_mode;
        self.rng_seed = decoded.rng_seed;
        self.rng_draws = decoded.rng_draws;

        self.rng = ChaCha8Rng::from_seed(self.rng_seed);
        self.rng.set_word_pos((self.rng_draws as u128) * 2);
        restore_schedule_source_state(
            &mut self.event_window_source,
            &decoded.event_window_source_state,
        )?;
        restore_schedule_source_state(
            &mut self.event_probability_source,
            &decoded.event_probability_source_state,
        )?;

        // Rebuild ports from config-derived state (fuel_type set during init).
        self.ports = ports_for_zone(self.descriptor.zone);
        if self.fuel_type != FuelType::Electric {
            self.ports.push(PortDeclaration::fuel());
        }

        let active_power_kw = if self.phase == EventPhase::Active {
            self.active_power_kw * self.load_fraction.max(0.0)
        } else {
            0.0
        };
        let is_fuel = self.fuel_type != FuelType::Electric;
        let fuel_consumption_w = if is_fuel { active_power_kw * 1_000.0 } else { 0.0 };
        let electric_power_kw = if is_fuel { 0.0 } else { active_power_kw };
        let gain_source_w = active_power_kw * 1_000.0;
        self.telemetry.set("active_power_kw", electric_power_kw);
        self.telemetry.set(
            "sensible_gain_w",
            gain_source_w * self.sensible_gain_fraction,
        );
        self.telemetry
            .set("latent_gain_w", gain_source_w * self.latent_gain_fraction);
        self.telemetry.set("gas_consumption_w", fuel_consumption_w);
        self.telemetry.set("state", phase_ordinal(self.phase));
        Ok(())
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        match signal {
            ControlSignal::LoadFraction { fraction } => {
                if !fraction.is_finite() {
                    return Err(HaresError::Control(
                        "EventBasedLoad LoadFraction must be finite".to_string(),
                    ));
                }
                self.load_fraction = fraction.max(0.0);
            }
            ControlSignal::ModeOverride { mode } => {
                self.forced_mode = Some(mode_to_forced(*mode));
            }
            ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } => {
                if !active_power_kw.is_finite() {
                    return Err(HaresError::Control(
                        "EventBasedLoad PowerSetpoint active_power_kw must be finite".to_string(),
                    ));
                }
                self.power_setpoint_override = Some(active_power_kw.max(0.0));
            }
            _ => {
                return Err(HaresError::Control(format!(
                    "EventBasedLoad does not handle control signal: {signal:?}"
                )));
            }
        }
        Ok(())
    }
}

impl WetAppliance {
    #[must_use]
    pub fn new(config: EquipmentConfig, name: &'static str) -> Self {
        let equipment_name = config.name.clone();
        let descriptor = EquipmentDescriptor {
            id: EquipmentId(parse_u32(&config.raw_config, KEY_EQUIPMENT_ID).unwrap_or_default()),
            name: equipment_name,
            end_use: EndUse::Other,
            equipment_type: Cow::Borrowed(name),
            zone: parse_zone_id(&config.raw_config),
            fuel: FuelType::Electric,
            stage: ExecutionStage::Independent,
            control_capabilities: ControlCapabilities::LOAD_FRACTION
                | ControlCapabilities::MODE_OVERRIDE,
            telemetry_fields: wet_appliance_telemetry_fields(),
        };
        let ports = ports_for_zone(descriptor.zone);
        let rng_seed = derive_rng_seed(&config);
        Self {
            descriptor,
            ports,
            telemetry: default_wet_appliance_telemetry(),
            fuel_type: FuelType::Electric,
            event_window_source: ScheduleSource::Constant(1.0),
            event_probability_source: ScheduleSource::Constant(1.0),
            phases: vec![CyclePhase {
                power_kw: 1.0,
                duration_s: 900.0,
            }],
            n_units: 1.0,
            sensible_gain_fraction: 0.0,
            latent_gain_fraction: 0.0,
            active: false,
            phase_index: 0,
            elapsed_in_phase_s: 0.0,
            load_fraction: 1.0,
            forced_mode: None,
            hot_water_draw_rate_kg_s: 0.0,
            rng_seed,
            rng_draws: 0,
            rng: ChaCha8Rng::from_seed(rng_seed),
        }
    }

    fn maybe_start_cycle(&mut self, window_open: bool, probability: f64) {
        if self.active {
            return;
        }
        let should_start = match self.forced_mode {
            Some(ForcedMode::Idle) => false,
            Some(ForcedMode::Active) => true,
            None => {
                if !window_open {
                    false
                } else {
                    let p = probability.clamp(0.0, 1.0);
                    if p <= 0.0 {
                        false
                    } else if p >= 1.0 {
                        true
                    } else {
                        self.rng_draws = self.rng_draws.saturating_add(1);
                        self.rng.random::<f64>() < p
                    }
                }
            }
        };

        if should_start {
            self.active = true;
            self.phase_index = 0;
            self.elapsed_in_phase_s = 0.0;
        }
    }

    fn apply_overrides(&mut self) {
        match self.forced_mode {
            Some(ForcedMode::Idle) => {
                self.active = false;
                self.phase_index = 0;
                self.elapsed_in_phase_s = 0.0;
            }
            Some(ForcedMode::Active) => {
                if !self.active {
                    self.active = true;
                    self.phase_index = 0;
                    self.elapsed_in_phase_s = 0.0;
                }
            }
            None => {}
        }
    }

    fn advance_cycle(&mut self, dt_s: f64) {
        if !self.active || dt_s <= 0.0 || matches!(self.forced_mode, Some(ForcedMode::Active)) {
            return;
        }

        let mut remaining_dt = dt_s;
        while remaining_dt > 0.0 && self.active {
            let phase = self.phases[self.phase_index];
            let remaining_phase = (phase.duration_s - self.elapsed_in_phase_s).max(0.0);
            if remaining_phase > remaining_dt {
                self.elapsed_in_phase_s += remaining_dt;
                break;
            }

            remaining_dt -= remaining_phase;
            self.phase_index += 1;
            self.elapsed_in_phase_s = 0.0;

            if self.phase_index >= self.phases.len() {
                self.active = false;
                self.phase_index = 0;
            }
        }
    }

    fn update_outputs(&mut self, ports: &mut PortSlots) -> std::result::Result<(), HaresError> {
        let active_power_kw = if self.active {
            self.phases[self.phase_index].power_kw * self.n_units * self.load_fraction.max(0.0)
        } else {
            0.0
        };

        let is_fuel = self.fuel_type != FuelType::Electric;
        let fuel_consumption_w = if is_fuel { active_power_kw * 1_000.0 } else { 0.0 };
        let electric_power_kw = if is_fuel { 0.0 } else { active_power_kw };

        // Thermal gains come from all input energy regardless of fuel type.
        let gain_source_w = active_power_kw * 1_000.0;
        let sensible_gain_w = gain_source_w * self.sensible_gain_fraction;
        let latent_gain_w = gain_source_w * self.latent_gain_fraction;

        if electric_power_kw != 0.0 {
            ports.accumulate(&PortContribution::Electrical {
                active_power_kw: electric_power_kw,
                reactive_power_kvar: 0.0,
            })?;
        }

        if fuel_consumption_w > 0.0 {
            ports.accumulate(&PortContribution::Fuel {
                fuel_type: self.fuel_type,
                consumption_w: fuel_consumption_w,
            })?;
        }

        if let Some(zone) = self.descriptor.zone
            && (sensible_gain_w != 0.0 || latent_gain_w != 0.0)
        {
            ports.accumulate(&PortContribution::Thermal {
                zone,
                sensible_gain_w,
                latent_gain_w,
                category: ThermalCategory::InternalGain,
            })?;
        }

        if self.active && self.hot_water_draw_rate_kg_s > 0.0 {
            ports.accumulate(&PortContribution::Fluid {
                loop_id: crate::water_heater::DHW_DEMAND_LOOP,
                flow_rate_kg_s: self.hot_water_draw_rate_kg_s * self.load_fraction.max(0.0),
                supply_temp_c: 0.0,
                return_temp_c: 0.0,
                fluid_type: FluidType::Water,
            })?;
        }

        self.telemetry.set("active_power_kw", electric_power_kw);
        self.telemetry.set("sensible_gain_w", sensible_gain_w);
        self.telemetry.set("latent_gain_w", latent_gain_w);
        self.telemetry.set("gas_consumption_w", fuel_consumption_w);
        self.telemetry.set(
            "cycle_phase",
            cycle_phase_ordinal(self.active, self.phase_index),
        );
        Ok(())
    }
}

impl Equipment for WetAppliance {
    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.descriptor
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.ports
    }

    fn init(&mut self, config: &EquipmentConfig, _env: &EnvironmentState) -> crate::Result<()> {
        let (window_source, probability_source) = parse_event_schedule_sources(config)?;
        self.event_window_source = window_source;
        self.event_probability_source = probability_source;
        self.phases = parse_cycle_phases(config)?;
        self.n_units = parse_non_negative(config, KEY_N_UNITS)?.unwrap_or(1.0);
        // "frac_sensible" / "frac_latent" are HPXML-parsed aliases.
        self.sensible_gain_fraction = config
            .get_f64(KEY_SENSIBLE_GAIN_FRACTION)
            .or_else(|| config.get_f64("frac_sensible"))
            .unwrap_or(0.0);
        self.latent_gain_fraction = config
            .get_f64(KEY_LATENT_GAIN_FRACTION)
            .or_else(|| config.get_f64("frac_latent"))
            .unwrap_or(0.0);

        self.fuel_type = match config.get_str("fuel_type") {
            None => FuelType::Electric,
            Some(raw) => parse_fuel_type(Some(raw)).ok_or_else(|| {
                HaresError::Equipment(format!("unrecognised fuel_type: {raw}"))
            })?,
        };
        self.descriptor.fuel = self.fuel_type;

        let total_cycle_duration_s: f64 = self.phases.iter().map(|p| p.duration_s).sum();
        self.hot_water_draw_rate_kg_s = if total_cycle_duration_s > 0.0 {
            config
                .get_f64(KEY_HOT_WATER_DRAW_VOLUME_L)
                .unwrap_or(0.0)
                .max(0.0)
                / total_cycle_duration_s
        } else {
            0.0
        };

        self.ports = ports_for_zone(self.descriptor.zone);
        if self.hot_water_draw_rate_kg_s > 0.0 {
            self.ports.push(PortDeclaration::fluid(crate::water_heater::DHW_DEMAND_LOOP, FluidType::Water));
        }
        if self.fuel_type != FuelType::Electric {
            self.ports.push(PortDeclaration::fuel());
        }

        self.active = false;
        self.phase_index = 0;
        self.elapsed_in_phase_s = 0.0;
        self.load_fraction = 1.0;
        self.forced_mode = None;
        self.telemetry = default_wet_appliance_telemetry();

        self.rng_seed = derive_rng_seed(config);
        self.rng_draws = 0;
        self.rng = ChaCha8Rng::from_seed(self.rng_seed);
        Ok(())
    }

    fn update_control(&mut self, _env: &EnvironmentState) -> OperatingMode {
        if self.active {
            OperatingMode::Standby
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
        self.apply_overrides();
        let window_open = self.event_window_source.value_at(env)? > 0.0;
        let probability = self.event_probability_source.value_at(env)?;
        self.maybe_start_cycle(window_open, probability);
        self.update_outputs(ports)?;
        self.advance_cycle(dt.as_secs_f64());
        Ok(())
    }

    fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn save_state(&self) -> Vec<u8> {
        save_postcard(&WetApplianceState {
            active: self.active,
            phase_index: self.phase_index,
            elapsed_in_phase_s: self.elapsed_in_phase_s,
            load_fraction: self.load_fraction,
            forced_mode: self.forced_mode,
            hot_water_draw_rate_kg_s: self.hot_water_draw_rate_kg_s,
            rng_seed: self.rng_seed,
            rng_draws: self.rng_draws,
            event_window_source_state: capture_schedule_source_state(&self.event_window_source),
            event_probability_source_state: capture_schedule_source_state(
                &self.event_probability_source,
            ),
        })
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let decoded: WetApplianceState = load_postcard(state)?;
        self.active = decoded.active;
        if decoded.phase_index >= self.phases.len() {
            return Err(HaresError::Equipment(format!(
                "checkpoint phase_index {} exceeds configured phase count {}",
                decoded.phase_index,
                self.phases.len()
            )));
        }
        self.phase_index = decoded.phase_index;
        self.elapsed_in_phase_s = decoded.elapsed_in_phase_s;
        self.load_fraction = decoded.load_fraction;
        self.forced_mode = decoded.forced_mode;
        self.hot_water_draw_rate_kg_s = decoded.hot_water_draw_rate_kg_s;

        // Regenerate ports to reflect restored DHW demand and fuel state.
        self.ports = ports_for_zone(self.descriptor.zone);
        if self.hot_water_draw_rate_kg_s > 0.0 {
            self.ports.push(PortDeclaration::fluid(crate::water_heater::DHW_DEMAND_LOOP, FluidType::Water));
        }
        if self.fuel_type != FuelType::Electric {
            self.ports.push(PortDeclaration::fuel());
        }

        self.rng_seed = decoded.rng_seed;
        self.rng_draws = decoded.rng_draws;

        self.rng = ChaCha8Rng::from_seed(self.rng_seed);
        self.rng.set_word_pos((self.rng_draws as u128) * 2);
        restore_schedule_source_state(
            &mut self.event_window_source,
            &decoded.event_window_source_state,
        )?;
        restore_schedule_source_state(
            &mut self.event_probability_source,
            &decoded.event_probability_source_state,
        )?;

        let active_power_kw = if self.active {
            self.phases[self.phase_index].power_kw * self.n_units * self.load_fraction.max(0.0)
        } else {
            0.0
        };
        let is_fuel = self.fuel_type != FuelType::Electric;
        let fuel_consumption_w = if is_fuel { active_power_kw * 1_000.0 } else { 0.0 };
        let electric_power_kw = if is_fuel { 0.0 } else { active_power_kw };
        let gain_source_w = active_power_kw * 1_000.0;
        self.telemetry.set("active_power_kw", electric_power_kw);
        self.telemetry.set(
            "sensible_gain_w",
            gain_source_w * self.sensible_gain_fraction,
        );
        self.telemetry
            .set("latent_gain_w", gain_source_w * self.latent_gain_fraction);
        self.telemetry.set("gas_consumption_w", fuel_consumption_w);
        self.telemetry.set(
            "cycle_phase",
            cycle_phase_ordinal(self.active, self.phase_index),
        );
        Ok(())
    }

    fn apply_control_unchecked(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        match signal {
            ControlSignal::LoadFraction { fraction } => {
                if !fraction.is_finite() {
                    return Err(HaresError::Control(
                        "WetAppliance LoadFraction must be finite".to_string(),
                    ));
                }
                self.load_fraction = fraction.max(0.0);
            }
            ControlSignal::ModeOverride { mode } => {
                self.forced_mode = Some(mode_to_forced(*mode));
            }
            _ => {
                return Err(HaresError::Control(format!(
                    "WetAppliance does not handle control signal: {signal:?}"
                )));
            }
        }
        Ok(())
    }
}

pub fn register_with_registry(registry: &mut EquipmentRegistry) {
    registry.register(
        "EventBasedLoad",
        Box::new(|config| Box::new(EventBasedLoad::new(config))),
    );
    registry.register(
        "Clothes Washer",
        Box::new(|config| Box::new(WetAppliance::new(config, "Clothes Washer"))),
    );
    registry.register(
        "Dishwasher",
        Box::new(|config| Box::new(WetAppliance::new(config, "Dishwasher"))),
    );
    registry.register(
        "Clothes Dryer",
        Box::new(|config| Box::new(WetAppliance::new(config, "Clothes Dryer"))),
    );
    registry.register(
        "Cooking Range",
        Box::new(|config| Box::new(EventBasedLoad::new(config))),
    );
}

/// Convert a 1440-minute OCHRE-style daily start-probability vector (`pdf_*.csv`)
/// into a timestep-aligned event-window/probability schedule.
///
/// Mapping contract:
/// - `minute_pdf[i]` is interpreted as the probability mass for minute `i`.
/// - For each simulation step, probabilities in that step window are summed and
///   clamped to `[0, 1]`.
/// - `event_window[k]` is `1.0` when the step probability is non-zero, else `0.0`.
/// - `event_probability[k]` is the summed/clamped probability for that step.
///
/// This function is intended for ingestion layers (e.g. HARES-036) that flatten
/// OCHRE schedule inputs into `EquipmentConfig` keys.
pub fn map_ochre_pdf_to_cycle_schedule(
    minute_pdf: &[f64],
    timestep_minutes: usize,
) -> Result<(Vec<f64>, Vec<f64>), HaresError> {
    if minute_pdf.len() != 1_440 {
        return Err(HaresError::Equipment(format!(
            "OCHRE daily PDF must contain 1440 entries, got {}",
            minute_pdf.len()
        )));
    }
    if timestep_minutes == 0 {
        return Err(HaresError::Equipment(
            "timestep_minutes must be > 0".to_string(),
        ));
    }

    let mut window = Vec::new();
    let mut probability = Vec::new();

    let mut start = 0;
    while start < 1_440 {
        let end = (start + timestep_minutes).min(1_440);
        let p_sum = minute_pdf[start..end]
            .iter()
            .copied()
            .sum::<f64>()
            .clamp(0.0, 1.0);
        probability.push(p_sum);
        window.push(if p_sum > 0.0 { 1.0 } else { 0.0 });
        start = end;
    }

    Ok((window, probability))
}

fn parse_event_schedule_sources(
    config: &EquipmentConfig,
) -> crate::Result<(ScheduleSource, ScheduleSource)> {
    let window_source = if let Some(col_idx) =
        parse_usize(&config.raw_config, KEY_EVENT_WINDOW_SCHEDULE_COL)?
    {
        ScheduleSource::ColumnRef {
            col_idx,
            boundary: BoundaryPolicy::Wrap,
        }
    } else {
        let source = config.get_str(KEY_EVENT_WINDOW_SOURCE).ok_or_else(|| {
            HaresError::Equipment(format!(
                "missing required key `{KEY_EVENT_WINDOW_SCHEDULE_COL}` or `{KEY_EVENT_WINDOW_SOURCE}`"
            ))
        })?;
        match source.to_ascii_lowercase().as_str() {
            "constant" => ScheduleSource::Constant(1.0),
            other => {
                return Err(HaresError::Equipment(format!(
                    "unsupported {KEY_EVENT_WINDOW_SOURCE} value `{other}` (expected `constant`)"
                )));
            }
        }
    };

    let probability_source = parse_event_probability_source(config)?.unwrap_or_else(|| {
        // Default: use the same CSV column as event window if present; otherwise
        // flat schedules are deterministic starts while window is open.
        match &window_source {
            ScheduleSource::ColumnRef { col_idx, .. } => ScheduleSource::ColumnRef {
                col_idx: *col_idx,
                boundary: BoundaryPolicy::Wrap,
            },
            _ => ScheduleSource::Constant(1.0),
        }
    });

    Ok((window_source, probability_source))
}

fn parse_event_probability_source(
    config: &EquipmentConfig,
) -> crate::Result<Option<ScheduleSource>> {
    if let Some(col_idx) = parse_usize(&config.raw_config, KEY_EVENT_PROBABILITY_SCHEDULE_COL)? {
        return Ok(Some(ScheduleSource::ColumnRef {
            col_idx,
            boundary: BoundaryPolicy::Wrap,
        }));
    }

    let Some(source) = config.get_str(KEY_EVENT_PROBABILITY_SOURCE) else {
        return Ok(None);
    };
    let source = source.to_ascii_lowercase();
    match source.as_str() {
        "constant" => Ok(Some(ScheduleSource::Constant(
            config
                .get_f64(KEY_EVENT_PROBABILITY_CONSTANT)
                .unwrap_or(1.0),
        ))),
        "column" => {
            let Some(col_idx) =
                parse_usize(&config.raw_config, KEY_EVENT_PROBABILITY_SCHEDULE_COL)?
            else {
                return Err(HaresError::Equipment(format!(
                    "missing required key `{KEY_EVENT_PROBABILITY_SCHEDULE_COL}` for column event probability schedule"
                )));
            };
            Ok(Some(ScheduleSource::ColumnRef {
                col_idx,
                boundary: BoundaryPolicy::Wrap,
            }))
        }
        _ => Err(HaresError::Equipment(format!(
            "unsupported {KEY_EVENT_PROBABILITY_SOURCE} value `{source}` (expected `column` or `constant`)"
        ))),
    }
}

fn parse_cycle_phases(config: &EquipmentConfig) -> crate::Result<Vec<CyclePhase>> {
    let count = parse_usize(&config.raw_config, KEY_PHASE_LEN)?.unwrap_or(0);
    if count == 0 {
        return Ok(vec![CyclePhase {
            power_kw: parse_non_negative(config, KEY_ACTIVE_POWER_KW)?.unwrap_or(1.0),
            duration_s: parse_positive(config, KEY_ACTIVE_DURATION_S)?.unwrap_or(900.0),
        }]);
    }

    let mut phases = Vec::with_capacity(count);
    for idx in 0..count {
        let p_a = format!("{PHASE_POWER_PREFIX_A}{idx}{PHASE_POWER_SUFFIX_A}");
        let d_a = format!("{PHASE_DURATION_PREFIX_A}{idx}{PHASE_DURATION_SUFFIX_A}");
        let p_b = format!("{PHASE_POWER_PREFIX_B}{idx}{PHASE_POWER_SUFFIX_B}");
        let d_b = format!("{PHASE_DURATION_PREFIX_B}{idx}{PHASE_DURATION_SUFFIX_B}");

        let power_kw = config
            .get_f64(&p_a)
            .or_else(|| config.get_f64(&p_b))
            .ok_or_else(|| {
                HaresError::Equipment(format!(
                    "missing wet appliance phase power at index {idx} (keys '{p_a}' or '{p_b}')"
                ))
            })?;
        let duration_s = config
            .get_f64(&d_a)
            .or_else(|| config.get_f64(&d_b))
            .ok_or_else(|| {
                HaresError::Equipment(format!(
                    "missing wet appliance phase duration at index {idx} (keys '{d_a}' or '{d_b}')"
                ))
            })?;

        if !power_kw.is_finite() || power_kw < 0.0 {
            return Err(HaresError::Equipment(format!(
                "wet appliance phase power at index {idx} must be finite and >= 0"
            )));
        }
        if !duration_s.is_finite() || duration_s <= 0.0 {
            return Err(HaresError::Equipment(format!(
                "wet appliance phase duration at index {idx} must be finite and > 0"
            )));
        }

        phases.push(CyclePhase {
            power_kw,
            duration_s,
        });
    }

    Ok(phases)
}

fn parse_non_negative(config: &EquipmentConfig, key: &str) -> crate::Result<Option<f64>> {
    let Some(value) = config.get_f64(key) else {
        return Ok(None);
    };
    if !value.is_finite() || value < 0.0 {
        return Err(HaresError::Equipment(format!(
            "{key} must be finite and >= 0, got {value}"
        )));
    }
    Ok(Some(value))
}

fn parse_positive(config: &EquipmentConfig, key: &str) -> crate::Result<Option<f64>> {
    let Some(value) = config.get_f64(key) else {
        return Ok(None);
    };
    if !value.is_finite() || value <= 0.0 {
        return Err(HaresError::Equipment(format!(
            "{key} must be finite and > 0, got {value}"
        )));
    }
    Ok(Some(value))
}

fn derive_rng_seed(config: &EquipmentConfig) -> [u8; 32] {
    // TODO(HARES-041): replace with hierarchical seeding (derive_dwelling_rng)
    // once core RNG management lands. For now use stable master_seed + building_id + equipment name.
    let master_seed = config.get_f64(KEY_MASTER_SEED).unwrap_or_default() as u64;
    let building_id = config.get_f64(KEY_BUILDING_ID).unwrap_or_default() as i64;

    // Hash the equipment name to discriminate seeds for different equipment
    // within the same building (e.g. two EventBasedLoads or a washer + dryer).
    let name_hash = {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325; // FNV-1a offset basis
        for byte in config.name.as_bytes() {
            h ^= *byte as u64;
            h = h.wrapping_mul(0x0100_0000_01b3); // FNV-1a prime
        }
        h
    };

    let mut seed = [0_u8; 32];
    seed[0..8].copy_from_slice(&master_seed.to_le_bytes());
    seed[8..16].copy_from_slice(&building_id.to_le_bytes());
    seed[16..24].copy_from_slice(&name_hash.to_le_bytes());
    seed
}

fn mode_to_forced(mode: OperatingMode) -> ForcedMode {
    match mode {
        OperatingMode::Off => ForcedMode::Idle,
        _ => ForcedMode::Active,
    }
}

fn phase_ordinal(phase: EventPhase) -> f64 {
    match phase {
        EventPhase::Idle => 0.0,
        EventPhase::Active => 1.0,
        EventPhase::Cooldown => 2.0,
    }
}

fn cycle_phase_ordinal(active: bool, phase_index: usize) -> f64 {
    if active {
        (phase_index + 1) as f64
    } else {
        0.0
    }
}

fn ports_for_zone(zone: Option<ZoneId>) -> Vec<PortDeclaration> {
    let mut ports = vec![PortDeclaration::electrical()];
    if let Some(zone) = zone {
        ports.push(PortDeclaration::thermal(zone));
    }
    ports
}

fn default_event_load_telemetry() -> Telemetry {
    let mut telemetry = Telemetry::with_capacity(5);
    telemetry.insert("active_power_kw", 0.0);
    telemetry.insert("sensible_gain_w", 0.0);
    telemetry.insert("latent_gain_w", 0.0);
    telemetry.insert("gas_consumption_w", 0.0);
    telemetry.insert("state", 0.0);
    telemetry
}

fn default_wet_appliance_telemetry() -> Telemetry {
    let mut telemetry = Telemetry::with_capacity(5);
    telemetry.insert("active_power_kw", 0.0);
    telemetry.insert("sensible_gain_w", 0.0);
    telemetry.insert("latent_gain_w", 0.0);
    telemetry.insert("gas_consumption_w", 0.0);
    telemetry.insert("cycle_phase", 0.0);
    telemetry
}

fn event_load_telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField {
            name: "active_power_kw".to_string(),
            unit: "kW".to_string(),
            description: "Active electrical power draw".to_string(),
        },
        TelemetryField {
            name: "sensible_gain_w".to_string(),
            unit: "W".to_string(),
            description: "Sensible thermal gain to assigned zone".to_string(),
        },
        TelemetryField {
            name: "latent_gain_w".to_string(),
            unit: "W".to_string(),
            description: "Latent thermal gain to assigned zone".to_string(),
        },
        TelemetryField {
            name: "gas_consumption_w".to_string(),
            unit: "W".to_string(),
            description: "Gas fuel consumption rate converted to watts".to_string(),
        },
        TelemetryField {
            name: "state".to_string(),
            unit: "ordinal".to_string(),
            description: "State machine phase (0=Idle,1=Active,2=Cooldown)".to_string(),
        },
    ]
}

fn wet_appliance_telemetry_fields() -> Vec<TelemetryField> {
    vec![
        TelemetryField {
            name: "active_power_kw".to_string(),
            unit: "kW".to_string(),
            description: "Active electrical power draw".to_string(),
        },
        TelemetryField {
            name: "sensible_gain_w".to_string(),
            unit: "W".to_string(),
            description: "Sensible thermal gain to assigned zone".to_string(),
        },
        TelemetryField {
            name: "latent_gain_w".to_string(),
            unit: "W".to_string(),
            description: "Latent thermal gain to assigned zone".to_string(),
        },
        TelemetryField {
            name: "gas_consumption_w".to_string(),
            unit: "W".to_string(),
            description: "Gas fuel consumption rate converted to watts".to_string(),
        },
        TelemetryField {
            name: "cycle_phase".to_string(),
            unit: "ordinal".to_string(),
            description: "Cycle phase (0=Idle,1..N=phase index + 1)".to_string(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, TimeZone, Utc};
    use hares_types::{
        ControlSignal, DomainUpdate, EnvironmentState, ExecutionStage, FuelType, GridState,
        PortSlots, PortType, WeatherState, ZoneId, ZoneState, schedule_domain_id,
    };

    use super::{
        EventBasedLoad, WetAppliance, map_ochre_pdf_to_cycle_schedule, register_with_registry,
    };
    use crate::{Equipment, EquipmentConfig, EquipmentRegistry};

    fn base_env() -> EnvironmentState {
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
            custom_domains: vec![DomainUpdate {
                domain_id: schedule_domain_id(),
                zone_temperatures_c: Vec::new(),
                custom_payload: Some(vec![0.0, 0.0]),
            }],
            current_time: Utc
                .with_ymd_and_hms(2026, 3, 18, 0, 0, 0)
                .single()
                .expect("valid UTC timestamp"),
            time_res: ChronoDuration::minutes(1),
        }
    }

    fn event_config(name: &str, class_name: &str) -> EquipmentConfig {
        let mut raw: HashMap<String, crate::config::ConfigValue> = HashMap::new();
        raw.insert("zone_id".to_string(), 1.0.into());
        raw.insert("event_window_schedule_col".to_string(), 0.0.into());
        raw.insert("event_probability_schedule_col".to_string(), 1.0.into());
        raw.insert("active_power_kw".to_string(), 1.5.into());
        raw.insert("active_duration_s".to_string(), 60.0.into());
        raw.insert("cooldown_duration_s".to_string(), 60.0.into());
        raw.insert("sensible_gain_fraction".to_string(), 0.4.into());
        raw.insert("latent_gain_fraction".to_string(), 0.1.into());
        raw.insert("building_id".to_string(), 7.0.into());
        raw.insert("master_seed".to_string(), 1234.0.into());
        EquipmentConfig {
            name: name.to_string(),
            ochre_class: class_name.to_string(),
            raw_config: raw,
        }
    }

    fn wet_config(name: &str, class_name: &str, n_units: f64) -> EquipmentConfig {
        let mut raw: HashMap<String, crate::config::ConfigValue> = HashMap::new();
        raw.insert("zone_id".to_string(), 1.0.into());
        raw.insert("event_window_schedule_col".to_string(), 0.0.into());
        raw.insert("event_probability_schedule_col".to_string(), 1.0.into());
        raw.insert("phase_len".to_string(), 2.0.into());
        raw.insert("phase_0_power_kw".to_string(), 0.5.into());
        raw.insert("phase_0_duration_s".to_string(), 60.0.into());
        raw.insert("phase_1_power_kw".to_string(), 1.2.into());
        raw.insert("phase_1_duration_s".to_string(), 120.0.into());
        raw.insert("n_units".to_string(), n_units.into());
        raw.insert("sensible_gain_fraction".to_string(), 0.2.into());
        raw.insert("latent_gain_fraction".to_string(), 0.05.into());
        raw.insert("building_id".to_string(), 11.0.into());
        raw.insert("master_seed".to_string(), 987.0.into());
        EquipmentConfig {
            name: name.to_string(),
            ochre_class: class_name.to_string(),
            raw_config: raw,
        }
    }

    fn set_schedule_payload(env: &mut EnvironmentState, values: Vec<f64>) {
        let slot = env
            .custom_domains
            .iter_mut()
            .find(|d| d.domain_id == schedule_domain_id())
            .expect("schedule domain must exist");
        slot.custom_payload = Some(values);
    }

    #[test]
    fn descriptors_are_independent_stage() {
        let e = EventBasedLoad::new(event_config("event", "EventBasedLoad"));
        assert_eq!(e.descriptor().stage, ExecutionStage::Independent);

        let w = WetAppliance::new(
            wet_config("washer", "Clothes Washer", 1.0),
            "Clothes Washer",
        );
        assert_eq!(w.descriptor().stage, ExecutionStage::Independent);
    }

    #[test]
    fn event_triggers_within_one_timestep_window() {
        let mut env = base_env();
        let config = event_config("event", "EventBasedLoad");
        let mut eq = EventBasedLoad::new(config.clone());
        eq.init(&config, &env).unwrap();

        let mut slots = PortSlots::from_declarations(eq.ports());
        set_schedule_payload(&mut env, vec![0.0, 0.0]);
        eq.step(&env, Duration::from_secs(60), &mut slots).unwrap();
        assert_eq!(slots.electrical.load_power_kw, 0.0);

        env.current_time += ChronoDuration::minutes(1);
        set_schedule_payload(&mut env, vec![1.0, 1.0]);
        slots.zero();
        eq.step(&env, Duration::from_secs(60), &mut slots).unwrap();
        assert!((slots.electrical.load_power_kw - 1.5).abs() < 1e-9);
    }

    #[test]
    fn wet_appliance_multi_phase_profile_sequence() {
        let mut env = base_env();
        let config = wet_config("washer", "Clothes Washer", 1.0);
        let mut eq = WetAppliance::new(config.clone(), "Clothes Washer");
        eq.init(&config, &env).unwrap();

        let mut slots = PortSlots::from_declarations(eq.ports());

        set_schedule_payload(&mut env, vec![1.0, 1.0]);
        eq.step(&env, Duration::from_secs(60), &mut slots).unwrap();
        assert!((slots.electrical.load_power_kw - 0.5).abs() < 1e-9);

        env.current_time += ChronoDuration::minutes(1);
        set_schedule_payload(&mut env, vec![0.0, 0.0]);
        slots.zero();
        eq.step(&env, Duration::from_secs(60), &mut slots).unwrap();
        assert!((slots.electrical.load_power_kw - 1.2).abs() < 1e-9);

        env.current_time += ChronoDuration::minutes(1);
        set_schedule_payload(&mut env, vec![0.0, 0.0]);
        slots.zero();
        eq.step(&env, Duration::from_secs(60), &mut slots).unwrap();
        assert!((slots.electrical.load_power_kw - 1.2).abs() < 1e-9);

        env.current_time += ChronoDuration::minutes(1);
        set_schedule_payload(&mut env, vec![0.0, 0.0]);
        slots.zero();
        eq.step(&env, Duration::from_secs(60), &mut slots).unwrap();
        assert_eq!(slots.electrical.load_power_kw, 0.0);
    }

    #[test]
    fn thermal_gains_are_zero_when_idle() {
        let mut env = base_env();
        let config = event_config("event", "EventBasedLoad");

        let mut eq = EventBasedLoad::new(config.clone());
        eq.init(&config, &env).unwrap();
        let mut slots = PortSlots::from_declarations(eq.ports());
        set_schedule_payload(&mut env, vec![0.0, 0.0]);
        eq.step(&env, Duration::from_secs(60), &mut slots).unwrap();

        let t = slots.thermal.iter().find(|t| t.zone == ZoneId(1)).unwrap();
        assert_eq!(t.sensible_gain_w, 0.0);
        assert_eq!(t.latent_gain_w, 0.0);
    }

    #[test]
    fn wet_appliance_n_units_scales_power_linearly() {
        let mut env = base_env();

        let cfg1 = wet_config("washer1", "Clothes Washer", 1.0);
        let mut eq1 = WetAppliance::new(cfg1.clone(), "Clothes Washer");
        eq1.init(&cfg1, &env).unwrap();
        let mut p1 = PortSlots::from_declarations(eq1.ports());
        set_schedule_payload(&mut env, vec![1.0, 1.0]);
        eq1.step(&env, Duration::from_secs(60), &mut p1).unwrap();

        let cfg2 = wet_config("washer2", "Clothes Washer", 2.0);
        let mut eq2 = WetAppliance::new(cfg2.clone(), "Clothes Washer");
        eq2.init(&cfg2, &env).unwrap();
        let mut p2 = PortSlots::from_declarations(eq2.ports());
        set_schedule_payload(&mut env, vec![1.0, 1.0]);
        eq2.step(&env, Duration::from_secs(60), &mut p2).unwrap();

        assert!((p2.electrical.load_power_kw - 2.0 * p1.electrical.load_power_kw).abs() < 1e-9);
    }

    #[test]
    fn save_then_load_restores_identical_followup_behavior() {
        let mut env_a = base_env();
        let config = wet_config("dryer", "Clothes Dryer", 1.0);

        let mut eq_a = WetAppliance::new(config.clone(), "Clothes Dryer");
        eq_a.init(&config, &env_a).unwrap();
        let mut ports_a = PortSlots::from_declarations(eq_a.ports());

        set_schedule_payload(&mut env_a, vec![1.0, 1.0]);
        eq_a.step(&env_a, Duration::from_secs(60), &mut ports_a)
            .unwrap();
        env_a.current_time += ChronoDuration::minutes(1);
        set_schedule_payload(&mut env_a, vec![0.0, 0.0]);
        ports_a.zero();
        eq_a.step(&env_a, Duration::from_secs(60), &mut ports_a)
            .unwrap();

        let checkpoint = eq_a.save_state();

        let mut env_b = env_a.clone();
        let mut eq_b = WetAppliance::new(config.clone(), "Clothes Dryer");
        eq_b.init(&config, &env_b).unwrap();
        eq_b.load_state(&checkpoint).unwrap();
        let mut ports_b = PortSlots::from_declarations(eq_b.ports());

        env_a.current_time += ChronoDuration::minutes(1);
        env_b.current_time += ChronoDuration::minutes(1);
        set_schedule_payload(&mut env_a, vec![0.0, 0.0]);
        set_schedule_payload(&mut env_b, vec![0.0, 0.0]);
        ports_a.zero();
        ports_b.zero();
        eq_a.step(&env_a, Duration::from_secs(60), &mut ports_a)
            .unwrap();
        eq_b.step(&env_b, Duration::from_secs(60), &mut ports_b)
            .unwrap();

        assert!((ports_a.electrical.load_power_kw - ports_b.electrical.load_power_kw).abs() < 1e-9);
        assert_eq!(
            eq_a.telemetry().get("cycle_phase"),
            eq_b.telemetry().get("cycle_phase")
        );
    }

    #[test]
    fn registry_contains_wet_appliance_ochre_names() {
        let mut registry = EquipmentRegistry::default();
        register_with_registry(&mut registry);
        assert!(registry.get("Clothes Washer").is_some());
        assert!(registry.get("Dishwasher").is_some());
        assert!(registry.get("Clothes Dryer").is_some());
    }

    #[test]
    fn mode_override_and_load_fraction_controls_work() {
        let mut env = base_env();
        let config = event_config("event", "EventBasedLoad");
        let mut eq = EventBasedLoad::new(config.clone());
        eq.init(&config, &env).unwrap();

        eq.apply_control(&ControlSignal::LoadFraction { fraction: 0.5 })
            .unwrap();
        eq.apply_control(&ControlSignal::ModeOverride {
            mode: hares_types::OperatingMode::Standby,
        })
        .unwrap();

        let mut ports = PortSlots::from_declarations(eq.ports());
        set_schedule_payload(&mut env, vec![0.0, 0.0]);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        assert!((ports.electrical.load_power_kw - 0.75).abs() < 1e-9);
    }

    #[test]
    fn map_ochre_pdf_to_cycle_schedule_aggregates_probabilities() {
        let mut pdf = vec![0.0; 1_440];
        pdf[0] = 0.2;
        pdf[1] = 0.3;
        pdf[10] = 0.8;
        let (window, probability) = map_ochre_pdf_to_cycle_schedule(&pdf, 5).unwrap();
        assert_eq!(window[0], 1.0);
        assert!((probability[0] - 0.5).abs() < 1e-9);
        assert_eq!(window[2], 1.0);
        assert!((probability[2] - 0.8).abs() < 1e-9);
    }

    #[test]
    fn registry_new_wires_event_loads_module() {
        let registry = EquipmentRegistry::new();
        assert!(registry.get("EventBasedLoad").is_some());
    }

    // =======================================================================
    // Cooldown phase returns OperatingMode::Off, not Standby
    // =======================================================================

    #[test]
    fn cooldown_phase_returns_off_not_standby() {
        let mut env = base_env();
        let config = event_config("event", "EventBasedLoad");
        let mut eq = EventBasedLoad::new(config.clone());
        eq.init(&config, &env).unwrap();

        // Step at index 1 (probability 1.0) to trigger Active phase
        env.current_time += ChronoDuration::minutes(1);
        let mut slots = PortSlots::from_declarations(eq.ports());
        set_schedule_payload(&mut env, vec![1.0, 1.0]);
        eq.step(&env, Duration::from_secs(60), &mut slots).unwrap();
        // Active phase: update_control should return Standby
        // (internally the step already set phase = Active)
        // After step, we advanced the timer by 60s, which equals active_duration_s,
        // so phase transitions to Cooldown. Let's verify directly with update_control.

        // To get Active mode, step without advancing the timer first
        let mut eq2 = EventBasedLoad::new(config.clone());
        eq2.init(&config, &env).unwrap();

        // Force Active via mode override to ensure we're in Active phase
        eq2.apply_control(&ControlSignal::ModeOverride {
            mode: hares_types::OperatingMode::Standby,
        })
        .unwrap();
        let mut slots2 = PortSlots::from_declarations(eq2.ports());
        set_schedule_payload(&mut env, vec![1.0, 1.0]);
        eq2.step(&env, Duration::from_secs(1), &mut slots2).unwrap();

        // Now remove the override and check phase
        // The eq should be in Active phase
        let mode_active = eq2.update_control(&env);
        assert_eq!(
            mode_active,
            hares_types::OperatingMode::Standby,
            "Active phase should return Standby"
        );

        // Clear override, advance past active_duration to enter Cooldown
        eq2.apply_control(&ControlSignal::ModeOverride {
            mode: hares_types::OperatingMode::Off,
        })
        .unwrap();
        eq2.apply_control(&ControlSignal::ModeOverride {
            mode: hares_types::OperatingMode::Standby,
        })
        .unwrap();
        // Actually, let's just directly manipulate a fresh instance more cleanly.
        let mut eq3 = EventBasedLoad::new(config.clone());
        eq3.init(&config, &env).unwrap();
        // Force Active
        eq3.apply_control(&ControlSignal::ModeOverride {
            mode: hares_types::OperatingMode::Standby,
        })
        .unwrap();
        let mut slots3 = PortSlots::from_declarations(eq3.ports());
        set_schedule_payload(&mut env, vec![1.0, 1.0]);
        eq3.step(&env, Duration::from_secs(1), &mut slots3).unwrap();
        // Clear override so natural phase transitions happen
        eq3.forced_mode = None;
        // Advance past active_duration_s (60s) to enter Cooldown
        env.current_time += ChronoDuration::minutes(1);
        set_schedule_payload(&mut env, vec![0.0, 0.0]);
        slots3.zero();
        eq3.step(&env, Duration::from_secs(60), &mut slots3)
            .unwrap();

        // After advancing 60s from Active (which had remaining_phase_s=60-1=59),
        // we should be in Cooldown now
        let mode_cooldown = eq3.update_control(&env);
        assert_eq!(
            mode_cooldown,
            hares_types::OperatingMode::Off,
            "Cooldown phase must return OperatingMode::Off, not Standby"
        );
    }

    // =======================================================================
    // Schedule wrap-around for multi-day simulations
    // =======================================================================

    #[test]
    fn column_ref_wrap_boundary_is_used_for_event_schedules() {
        // When col_idx is out of range for current schedule payload, Wrap maps it modulo payload len.
        let mut raw: HashMap<String, crate::config::ConfigValue> = HashMap::new();
        raw.insert("zone_id".to_string(), 1.0.into());
        raw.insert("event_window_schedule_col".to_string(), 2.0.into()); // wraps to 0 for payload len=2
        raw.insert("event_probability_schedule_col".to_string(), 3.0.into()); // wraps to 1 for payload len=2
        raw.insert("active_power_kw".to_string(), 2.0.into());
        raw.insert("active_duration_s".to_string(), 30.0.into());
        raw.insert("cooldown_duration_s".to_string(), 0.0.into());
        raw.insert("building_id".to_string(), 1.0.into());
        raw.insert("master_seed".to_string(), 42.0.into());
        let config = EquipmentConfig {
            name: "wrap_test".to_string(),
            ochre_class: "EventBasedLoad".to_string(),
            raw_config: raw,
        };

        let mut env = base_env();
        set_schedule_payload(&mut env, vec![1.0, 1.0]);
        let mut eq = EventBasedLoad::new(config.clone());
        eq.init(&config, &env).unwrap();

        let mut slots = PortSlots::from_declarations(eq.ports());
        eq.step(&env, Duration::from_secs(60), &mut slots).unwrap();
        assert!(slots.electrical.load_power_kw > 0.0);
    }

    #[test]
    fn schedule_len_zero_returns_error() {
        let env = base_env();
        // Create a config with no schedule entries at all
        let mut raw: HashMap<String, crate::config::ConfigValue> = HashMap::new();
        raw.insert("zone_id".to_string(), 1.0.into());
        raw.insert("active_power_kw".to_string(), 1.0.into());
        raw.insert("active_duration_s".to_string(), 60.0.into());
        // No compact event schedule keys
        let config = EquipmentConfig {
            name: "empty".to_string(),
            ochre_class: "EventBasedLoad".to_string(),
            raw_config: raw,
        };
        let mut eq = EventBasedLoad::new(config.clone());
        let err = eq.init(&config, &env);
        assert!(err.is_err(), "init with no schedule should fail");
    }

    // =======================================================================
    // ChaCha8 set_word_pos regression test
    // =======================================================================

    #[test]
    fn chacha8_set_word_pos_matches_sequential_draws() {
        use rand::RngExt;
        use rand::SeedableRng;
        use rand_chacha::ChaCha8Rng;

        let seed = [42_u8; 32];
        let n_draws = 10_u64;

        // Method 1: sequential draws
        let mut rng_sequential = ChaCha8Rng::from_seed(seed);
        let mut sequential_values = Vec::new();
        for _ in 0..n_draws {
            sequential_values.push(rng_sequential.random::<f64>());
        }
        // Continue drawing after N draws
        let next_sequential = rng_sequential.random::<f64>();

        // Method 2: set_word_pos to skip N draws, then draw
        let mut rng_skip = ChaCha8Rng::from_seed(seed);
        rng_skip.set_word_pos((n_draws as u128) * 2);
        let next_skip = rng_skip.random::<f64>();

        assert_eq!(
            next_sequential,
            next_skip,
            "set_word_pos({}) must produce identical subsequent draws to N sequential draws",
            n_draws * 2
        );

        // Also verify that N+1, N+2 draws match
        let next_sequential_2 = rng_sequential.random::<f64>();
        let next_skip_2 = rng_skip.random::<f64>();
        assert_eq!(next_sequential_2, next_skip_2);
    }

    #[test]
    fn load_state_after_n_draws_matches_sequential_rng() {
        // Integration test: save state after several steps, load into new instance,
        // verify subsequent RNG-dependent behavior is identical.
        let env = base_env();
        let config = event_config("rng_test", "EventBasedLoad");
        let mut eq_a = EventBasedLoad::new(config.clone());
        eq_a.init(&config, &env).unwrap();

        // Run several steps to accumulate rng_draws
        let mut env_step = env.clone();
        for _ in 0..5 {
            let mut slots = PortSlots::from_declarations(eq_a.ports());
            set_schedule_payload(&mut env_step, vec![1.0, 0.5]);
            eq_a.step(&env_step, Duration::from_secs(60), &mut slots)
                .unwrap();
            env_step.current_time += ChronoDuration::minutes(1);
        }

        let checkpoint = eq_a.save_state();

        // Create new instance and load state
        let mut eq_b = EventBasedLoad::new(config.clone());
        eq_b.init(&config, &env).unwrap();
        eq_b.load_state(&checkpoint).unwrap();

        // Both should produce identical behavior going forward
        for _ in 0..5 {
            let mut slots_a = PortSlots::from_declarations(eq_a.ports());
            let mut slots_b = PortSlots::from_declarations(eq_b.ports());
            set_schedule_payload(&mut env_step, vec![1.0, 0.5]);
            eq_a.step(&env_step, Duration::from_secs(60), &mut slots_a)
                .unwrap();
            eq_b.step(&env_step, Duration::from_secs(60), &mut slots_b)
                .unwrap();
            assert_eq!(
                slots_a.electrical.load_power_kw, slots_b.electrical.load_power_kw,
                "RNG divergence after load_state"
            );
            env_step.current_time += ChronoDuration::minutes(1);
        }
    }

    // =======================================================================
    // equipment_id in RNG seed: different names produce different seeds
    // =======================================================================

    #[test]
    fn different_equipment_names_produce_different_rng_sequences() {
        let env = base_env();

        // Two configs with same master_seed and building_id but different names
        let config_a = event_config("Dishwasher", "EventBasedLoad");
        let config_b = event_config("Clothes Washer", "EventBasedLoad");

        let mut eq_a = EventBasedLoad::new(config_a.clone());
        eq_a.init(&config_a, &env).unwrap();
        let mut eq_b = EventBasedLoad::new(config_b.clone());
        eq_b.init(&config_b, &env).unwrap();

        // The seeds should differ because equipment names differ
        assert_ne!(
            eq_a.rng_seed, eq_b.rng_seed,
            "Different equipment names must produce different RNG seeds"
        );
    }

    #[test]
    fn same_name_same_seed_produces_identical_rng() {
        let env = base_env();
        let config_a = event_config("Dishwasher", "EventBasedLoad");
        let config_b = event_config("Dishwasher", "EventBasedLoad");

        let mut eq_a = EventBasedLoad::new(config_a.clone());
        eq_a.init(&config_a, &env).unwrap();
        let mut eq_b = EventBasedLoad::new(config_b.clone());
        eq_b.init(&config_b, &env).unwrap();

        assert_eq!(
            eq_a.rng_seed, eq_b.rng_seed,
            "Same name + same config should produce identical seeds"
        );
    }

    // =======================================================================
    // WetAppliance load_state with invalid phase_index returns error
    // =======================================================================

    #[test]
    fn wet_appliance_load_state_invalid_phase_index_returns_error() {
        let env = base_env();
        let config = wet_config("washer", "Clothes Washer", 1.0);
        let mut eq = WetAppliance::new(config.clone(), "Clothes Washer");
        eq.init(&config, &env).unwrap();

        // eq has 2 phases (phase_len=2). Craft a checkpoint with phase_index=5.
        let bad_state = super::WetApplianceState {
            active: true,
            phase_index: 5,
            elapsed_in_phase_s: 0.0,
            load_fraction: 1.0,
            forced_mode: None,
            hot_water_draw_rate_kg_s: 0.0,
            rng_seed: eq.rng_seed,
            rng_draws: 0,
            event_window_source_state: super::ScheduleSourceState::Stateless,
            event_probability_source_state: super::ScheduleSourceState::Stateless,
        };
        let bytes = crate::save_postcard(&bad_state);

        let err = eq.load_state(&bytes);
        assert!(
            err.is_err(),
            "load_state with phase_index=5 and 2 phases must return an error"
        );
        let err_msg = err.unwrap_err().to_string();
        assert!(
            err_msg.contains("phase_index") && err_msg.contains("5"),
            "error message should mention phase_index and the value 5, got: {err_msg}"
        );
    }

    /// PowerSetpoint overrides active power for the current step only; the next
    /// step without a new signal reverts to the configured active_power_kw.
    #[test]
    fn power_setpoint_overrides_active_power_for_one_step() {
        // Use a schedule where index 0 is open with probability 1.0 so the
        // equipment enters the Active phase immediately on the first step.
        let mut raw: HashMap<String, crate::config::ConfigValue> = HashMap::new();
        raw.insert("zone_id".to_string(), 1.0.into());
        raw.insert("event_window_schedule_col".to_string(), 0.0.into());
        raw.insert("event_probability_schedule_col".to_string(), 1.0.into());
        // Long duration so the Active phase persists across multiple steps.
        raw.insert("active_power_kw".to_string(), 3.0.into());
        raw.insert("active_duration_s".to_string(), 3600.0.into());
        raw.insert("cooldown_duration_s".to_string(), 0.0.into());
        raw.insert("building_id".to_string(), 1.0.into());
        raw.insert("master_seed".to_string(), 1.0.into());
        let config = EquipmentConfig {
            name: "setpoint_test".to_string(),
            ochre_class: "EventBasedLoad".to_string(),
            raw_config: raw,
        };

        let mut env = base_env();
        let mut eq = EventBasedLoad::new(config.clone());
        eq.init(&config, &env).unwrap();

        // Step once to enter Active phase (window open, probability 1.0).
        set_schedule_payload(&mut env, vec![1.0, 1.0]);
        let mut ports = PortSlots::from_declarations(eq.ports());
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        // Verify the equipment is now drawing at the configured power.
        assert!(
            (ports.electrical.load_power_kw - 3.0).abs() < 1e-9,
            "expected 3.0 kW before setpoint, got {}",
            ports.electrical.load_power_kw
        );

        // Apply a PowerSetpoint override for the next step only.
        eq.apply_control(&ControlSignal::PowerSetpoint {
            active_power_kw: 0.5,
            reactive_power_kvar: None,
        })
        .unwrap();

        // Step with the setpoint active — output must be the override value.
        let mut env2 = env.clone();
        env2.current_time += chrono::Duration::minutes(1);
        set_schedule_payload(&mut env2, vec![1.0, 1.0]);
        ports.zero();
        eq.step(&env2, Duration::from_secs(60), &mut ports).unwrap();
        assert!(
            (ports.electrical.load_power_kw - 0.5).abs() < 1e-9,
            "expected 0.5 kW with PowerSetpoint override, got {}",
            ports.electrical.load_power_kw
        );

        // Next step without any new signal: override is consumed, output reverts.
        let mut env3 = env2.clone();
        env3.current_time += chrono::Duration::minutes(1);
        set_schedule_payload(&mut env3, vec![1.0, 1.0]);
        ports.zero();
        eq.step(&env3, Duration::from_secs(60), &mut ports).unwrap();
        assert!(
            (ports.electrical.load_power_kw - 3.0).abs() < 1e-9,
            "expected 3.0 kW after setpoint consumed, got {}",
            ports.electrical.load_power_kw
        );
    }

    #[test]
    fn wet_appliance_load_state_valid_phase_index_succeeds() {
        let env = base_env();
        let config = wet_config("washer", "Clothes Washer", 1.0);
        let mut eq = WetAppliance::new(config.clone(), "Clothes Washer");
        eq.init(&config, &env).unwrap();

        // phase_index=1 is valid for a 2-phase appliance
        let good_state = super::WetApplianceState {
            active: true,
            phase_index: 1,
            elapsed_in_phase_s: 10.0,
            load_fraction: 1.0,
            forced_mode: None,
            hot_water_draw_rate_kg_s: 0.0,
            rng_seed: eq.rng_seed,
            rng_draws: 0,
            event_window_source_state: super::ScheduleSourceState::Stateless,
            event_probability_source_state: super::ScheduleSourceState::Stateless,
        };
        let bytes = crate::save_postcard(&good_state);

        let result = eq.load_state(&bytes);
        assert!(
            result.is_ok(),
            "phase_index=1 should be valid for 2-phase appliance"
        );
        assert_eq!(eq.phase_index, 1);
    }

    // =======================================================================
    // FX-025: Gas fuel type handling
    // =======================================================================

    fn gas_event_config(name: &str) -> EquipmentConfig {
        let mut cfg = event_config(name, "Cooking Range");
        cfg.raw_config
            .insert("fuel_type".to_string(), "Gas".into());
        cfg
    }

    fn gas_wet_config(name: &str) -> EquipmentConfig {
        let mut cfg = wet_config(name, "Clothes Dryer", 1.0);
        cfg.raw_config
            .insert("fuel_type".to_string(), "Gas".into());
        cfg
    }

    #[test]
    fn gas_cooking_range_reports_gas_not_electric() {
        let mut env = base_env();
        let config = gas_event_config("Gas Range");
        let mut eq = EventBasedLoad::new(config.clone());
        eq.init(&config, &env).unwrap();

        assert_eq!(eq.descriptor().fuel, FuelType::Gas);
        assert!(eq.ports().iter().any(|p| p.port_type == PortType::Fuel));

        let mut slots = PortSlots::from_declarations(eq.ports());
        set_schedule_payload(&mut env, vec![1.0, 1.0]);
        eq.step(&env, Duration::from_secs(60), &mut slots).unwrap();

        // Gas power should be non-zero, electric should be zero.
        assert!(
            slots.fuel.get(FuelType::Gas) > 0.0,
            "gas cooking range must report gas consumption"
        );
        assert_eq!(
            slots.electrical.load_power_kw, 0.0,
            "gas cooking range must not report electrical consumption"
        );
        // Thermal gains must still be emitted (gas energy heats the zone).
        let t = slots.thermal.iter().find(|t| t.zone == ZoneId(1)).unwrap();
        assert!(t.sensible_gain_w > 0.0);
    }

    #[test]
    fn gas_clothes_dryer_reports_gas() {
        let mut env = base_env();
        let config = gas_wet_config("Gas Dryer");
        let mut eq = WetAppliance::new(config.clone(), "Clothes Dryer");
        eq.init(&config, &env).unwrap();

        assert_eq!(eq.descriptor().fuel, FuelType::Gas);
        assert!(eq.ports().iter().any(|p| p.port_type == PortType::Fuel));

        let mut slots = PortSlots::from_declarations(eq.ports());
        set_schedule_payload(&mut env, vec![1.0, 1.0]);
        eq.step(&env, Duration::from_secs(60), &mut slots).unwrap();

        assert!(
            slots.fuel.get(FuelType::Gas) > 0.0,
            "gas clothes dryer must report gas consumption"
        );
        assert_eq!(
            slots.electrical.load_power_kw, 0.0,
            "gas clothes dryer must not report electrical consumption"
        );
        let t = slots.thermal.iter().find(|t| t.zone == ZoneId(1)).unwrap();
        assert!(t.sensible_gain_w > 0.0);
    }

    #[test]
    fn electric_cooking_range_still_reports_electric() {
        let mut env = base_env();
        let config = event_config("Electric Range", "Cooking Range");
        let mut eq = EventBasedLoad::new(config.clone());
        eq.init(&config, &env).unwrap();

        assert_eq!(eq.descriptor().fuel, FuelType::Electric);
        assert!(!eq.ports().iter().any(|p| p.port_type == PortType::Fuel));

        let mut slots = PortSlots::from_declarations(eq.ports());
        set_schedule_payload(&mut env, vec![1.0, 1.0]);
        eq.step(&env, Duration::from_secs(60), &mut slots).unwrap();

        assert!(
            slots.electrical.load_power_kw > 0.0,
            "electric cooking range must report electrical consumption"
        );
        assert_eq!(
            slots.fuel.get(FuelType::Gas),
            0.0,
            "electric cooking range must not report gas consumption"
        );
    }

    #[test]
    fn gas_telemetry_reports_gas_consumption_w() {
        let mut env = base_env();
        let config = gas_event_config("Gas Range Telem");
        let mut eq = EventBasedLoad::new(config.clone());
        eq.init(&config, &env).unwrap();

        let mut slots = PortSlots::from_declarations(eq.ports());
        set_schedule_payload(&mut env, vec![1.0, 1.0]);
        eq.step(&env, Duration::from_secs(60), &mut slots).unwrap();

        let gas_w = eq.telemetry().get("gas_consumption_w").unwrap();
        let elec_kw = eq.telemetry().get("active_power_kw").unwrap();
        assert!(gas_w > 0.0, "gas_consumption_w must be positive for gas equipment");
        assert_eq!(elec_kw, 0.0, "active_power_kw must be zero for gas equipment");
    }
}
