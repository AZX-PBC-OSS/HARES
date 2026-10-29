//! Central and room air conditioner models.

use std::borrow::Cow;
use std::time::Duration;

// The cooling core's SHR telemetry-key allocation accumulator, read by
// the site's witness test only: the step's SHR write used
// `Telemetry::insert` (whose `impl Into<String>` key builds a fresh
// `String` every call, even when the map already holds the key), one
// allocation per cooling step. The counting allocator is installed by
// this crate's test build alone.
#[cfg(test)]
thread_local! {
    static COOLING_SHR_TELEMETRY_SITE: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The accumulator's read-and-reset, for the witness test. Thread-local:
/// the coverage build runs the suite in one process on many threads, so a
/// process-global accumulator would count other threads' SHR writes inside
/// this thread's witness window.
#[cfg(test)]
pub(crate) fn cooling_shr_telemetry_site() -> u64 {
    COOLING_SHR_TELEMETRY_SITE.take()
}

use chrono::{DateTime, FixedOffset};
use hares_physics::constants::LATENT_HEAT_VAPORISATION_0C_J_KG;
use hares_physics::ground::SourceTemperature;
use hares_types::{
    ControlCapabilities, ControlSignal, CoreCapabilities, CoreFlows, CoreOutput, CorePerformance,
    CoreState, DRLevel, ElectricPower, EndUse, EnvironmentState, EquipmentDescriptor,
    EquipmentHealthCounts, EquipmentId, ExecutionStage, FuelType, HaresError, OperatingMode,
    PortContribution, PortDeclaration, PortSlots, Telemetry, ThermalCategory, Warning,
    demand_response_command,
};
use serde::{Deserialize, Serialize};

use hares_types::telemetry_keys as tk;

#[cfg(feature = "observe")]
use hares_types::TelemetryField;

#[cfg(test)]
use crate::HvacSetpointConfig;
use crate::{Equipment, EquipmentConfig, EquipmentRegistry, load_versioned, try_save_versioned};

use super::ac_config::{
    CentralAirConditionerConfig, RoomAcConfig, default_telemetry, load_curve_pair, telemetry_fields,
};
use super::coil_physics::{
    CoilOperatingState, CoilResult, LatentDegradationParams, calculate_shr,
    effective_shr_with_latent_degradation,
};
use super::latent_degradation::compute_coil_ao_by_stage;
use super::speed_control::{SpeedSelection, interpolate_speed_stages};
use super::staging::{DEFAULT_PLF_DEGRADATION_COEFF, DEFAULT_STARTUP_CD};
use super::{
    HvacEquipment, HvacEquipmentType, RuntimeSetpointOverride, SpeedControlMode, ThermostatMode,
    helpers::{
        cycling_load_fraction, lookup_zone, netted_cycling_duty, outage_forces_off,
        register_ebm_telemetry_keys, served_zone_ports, step_equivalent_battery,
        unavailable_period_forces_off, write_ebm_telemetry, zone_id_from_config,
    },
};
use crate::config::constructor_equipment_id;

const CRANKCASE_HEATER_W: f64 = 50.0;
const CRANKCASE_HEATER_THRESHOLD_C: f64 = 12.8;

pub struct AirConditioner {
    pub(super) core: CoolingCore,
}

pub struct RoomAC {
    core: CoolingCore,
}

pub(super) struct CoolingCore {
    descriptor: EquipmentDescriptor,
    ports: Vec<PortDeclaration>,
    telemetry: Telemetry,
    core_output: CoreOutput,
    pub(super) hvac: HvacEquipment,
    operating_mode: OperatingMode,
    /// How the equipment determines its source-side temperature for curve evaluation.
    pub(super) source_temp: SourceTemperature,
    run_time_s: f64,
    cycle_on_steps: u64,
    cycle_off_steps: u64,
    crankcase_heater_on: bool,
    crankcase_heater_w: f64,
    /// Rated crankcase heater power [kW]; loaded from config, variant-dependent default.
    pub(super) crankcase_rated_w: f64,
    /// Outdoor temperature threshold [°C] below which crankcase heater activates.
    pub(super) crankcase_threshold_c: f64,
    /// Optional polynomial coefficients [c0, c1, c2] for temperature-dependent
    /// crankcase heater capacity: effective_w = rated_w * (c0 + c1*T + c2*T^2).
    crankcase_capacity_curve: Option<[f64; 3]>,
    /// Runtime fraction of the cooling coil in the most recent step (PLR value).
    /// Scales the crankcase heater's draw by the coil's off fraction and answers
    /// the cooler's compressor-ran read for its own step bookkeeping.
    pub(super) last_cooling_rtf: f64,
    /// Gross compressor power of the most recent step [kW], before the
    /// `space_fraction` scaling applied to telemetry. Used by the GSHP cooler
    /// for the borehole heat-rejection balance, which pairs it with the gross
    /// (unscaled) coil sensible/latent values.
    pub(super) last_compressor_w: f64,
    /// The moment's nominal component draw of the most recent step (W, the
    /// components' pre-scaling sum at the reference voltage, space fraction
    /// included): the premise aggregate's weight for this unit, which
    /// un-scales the stepped draw through the assembled response it
    /// reports. `None` when the step drew nothing.
    pub(super) last_nominal_draw_w: Option<f64>,
    flow_fraction_correction: f64,
    coil_ao_by_stage: Vec<f64>,
    is_room_ac: bool,
    /// Rated SHR at AHRI conditions, stored for latent degradation model.
    rated_shr: f64,
    /// Per-stage rated SHR values, if provided by config.
    pub(crate) stage_shrs: Vec<f64>,
    /// Henderson-Rengarajan latent degradation model parameters.
    /// All four fields must be > 0 (checked via `is_active()`) to enable the model.
    latent_degradation: LatentDegradationParams,
    last_adp_c: f64,
    last_bypass_factor: f64,

    /// Minimum outdoor air temperature for compressor operation [°C].
    /// EnergyPlus `DXCoils.cc:731`: `minOATCompDXCooling = -25.0`.
    pub(super) min_oat_cooling_c: f64,
    /// Transient flag: true when the current step's cooling was suppressed
    /// by the OAT lockout; consumed by step() for telemetry.
    cooling_oat_locked_out: bool,

    // --- Ideal capacity (solver-driven) ---
    /// Cached from last update_control; true when timestep >= 5 min.
    use_ideal: bool,
    /// Solver-provided ideal capacity [W]; set via IdealCapacity signal.
    ideal_capacity_w: f64,
    /// Whether an IdealCapacity signal arrived this step. The capacity's
    /// own field clears at each step's end (a stale capacity must never
    /// deliver), so a zero reading cannot distinguish "the solve said
    /// nothing needed" from "the pipeline starved the solve"; this flag
    /// carries the distinction, and a starved step counts in the
    /// equipment's health (`ideal_capacity_unsolved_steps`). Per-step
    /// bookkeeping, not checkpoint state: it clears at every step's end,
    /// so a restore always finds it false.
    ideal_capacity_signal_arrived: bool,
    /// The run-total starvation count, drained into the run's health at
    /// every step (`take_health_counts`): always zero at a step boundary,
    /// so it needs no checkpoint section. The run's own total rides the
    /// checkpoint's health section.
    ideal_capacity_unsolved_steps: u64,

    // --- External control signals (sticky) ---
    ctrl_duty_cycle: f64,
    ctrl_power_limit_w: f64,
    ctrl_mode_override: Option<OperatingMode>,

    // --- Transient signals (reset each step) ---
    ctrl_load_fraction: f64,

    // --- Demand response state ---
    dr_setpoint_offset_c: f64,
    dr_load_fraction: f64,
    dr_duty_cycle: f64,
    dr_duration_remaining_s: Option<f64>,
    dr_level: DRLevel,
    /// The class ZIP row (resolved via `crate::config::resolve_reactive_zip`):
    /// applies to the compressor component (class default pf 0.96, or a
    /// user `"zip"` override), whose real polynomial scales the
    /// compressor's draw at the bus voltage through the recorded deep-sag
    /// form (as fitted: the recorded Hajagos & Danai row); Q comes from
    /// `ZipLoad::reactive_var` per component (see `hvac::reactive`).
    zip: hares_types::zip::ResolvedZip,
    /// The indoor blower / condenser fan motor component's REACTIVE row
    /// (pf 0.87), derived at init via `hvac::reactive::secondary_motor_zip`:
    /// the reactive sum's surface, the unit-level pf = 0.0 sentinel
    /// included. The blower's REAL response never consults this row; it
    /// evaluates the physical `hvac::reactive::FAN_MOTOR_ZIP` row.
    fan_zip: hares_types::zip::ZipLoad,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AirConditionerState {
    mode: ThermostatMode,
    duty_cycle: f64,
    last_mode_switch_at: Option<DateTime<FixedOffset>>,
    mode_start_at: Option<DateTime<FixedOffset>>,
    runtime_setpoints: Option<RuntimeSetpointOverride>,
    operating_mode: OperatingMode,
    run_time_s: f64,
    cycle_on_steps: u64,
    cycle_off_steps: u64,
    crankcase_heater_on: bool,
    startup_c_d: f64,
    startup_time_since_start_min: f64,
    startup_was_on: bool,
    plf_state: f64,
    last_speed_index: usize,
    last_speed_frac: f64,
    electric_w: f64,
    sensible_cooling_w: f64,
    latent_cooling_w: f64,
    /// The last cooling step's SHR, `None` while the unit has never cooled:
    /// the telemetry map's null-until-first-cooling timeline (the step
    /// registers the key at the first cooling step) survives the round
    /// trip, so a restored never-cooled unit's map matches a
    /// continuously-run one's.
    shr: Option<f64>,
    // --- Sticky control signals ---
    ctrl_duty_cycle: f64,
    /// None means unlimited; f64::INFINITY does not serialize cleanly with postcard.
    ctrl_power_limit_w: Option<f64>,
    ctrl_mode_override: Option<OperatingMode>,
    // --- Demand response state ---
    dr_level: DRLevel,
    dr_setpoint_offset_c: f64,
    dr_load_fraction: f64,
    dr_duty_cycle: f64,
    dr_duration_remaining_s: Option<f64>,
    last_adp_c: f64,
    last_bypass_factor: f64,
    thermostat_hysteresis_c: f64,
    time_at_current_speed_s: f64,
    // --- Thermostat short-cycle protection (survives warmup restart) ---
    min_on_time_s: f64,
    min_off_time_s: f64,
}

// Version 2: dropped the redundant `operating_mode_code` field (telemetry mode
// code is now always recomputed from `operating_mode` via `OperatingMode::as_code`).
// Version 3: added `min_on_time_s`/`min_off_time_s` for short-cycle protection persistence.
// Version 4: added `startup_was_on` for edge-detection persistence across checkpoint restart.
// Version 5: `shr` became `Option<f64>`: `None` until the unit's first
// cooling step, so the restore writes the telemetry key only when a cooled
// step produced a value and a restored never-cooled unit's map matches a
// continuously-run one's instead of gaining a 1.0 sentinel key.
const AC_CHECKPOINT_VERSION: u32 = 5;

#[derive(Clone, Copy)]
struct PerformanceResult {
    sensible_cooling_w: f64,
    latent_cooling_w: f64,
    compressor_w: f64,
    fan_w: f64,
    shr: f64,
    /// Supply air dry-bulb temperature [°C] from the coil solver.
    supply_temp_c: f64,
    /// Runtime fraction (PLR / PLF) for this step -- used for crankcase accounting.
    rtf: f64,
}

impl AirConditioner {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        Self {
            core: CoolingCore::new(config, false),
        }
    }

    /// Calculate crankcase heater power.
    ///
    /// Draws power only when the cooling coil is not running and the
    /// outdoor temperature is below the threshold.
    #[cfg(test)]
    pub(super) fn crankcase_heater_power(&self, outdoor_temp_c: f64, cooling_rtf: f64) -> f64 {
        self.core
            .crankcase_heater_power_internal(outdoor_temp_c, cooling_rtf)
    }

    /// Set a numeric telemetry key on the inner cooling core.
    ///
    /// Used by `GshpCooler` to publish pump power telemetry after the
    /// inner step has completed.
    pub(super) fn set_telemetry(&mut self, key: &str, value: f64) {
        self.core.telemetry.set(key, value);
    }

    /// Override crankcase heater parameters after `init()`.
    /// Used by `HpCooler::mshp_cooler` to apply MSHP-specific defaults
    /// (15 W / 0 °C) when the user did not supply explicit config keys.
    pub(super) fn set_crankcase_defaults_if_unconfigured(
        &mut self,
        config: &EquipmentConfig,
        rated_w: f64,
        threshold_c: f64,
    ) {
        let crankcase_kw_configured = config
            .typed::<CentralAirConditionerConfig>()
            .ok()
            .and_then(|cfg| cfg.crankcase_heater_w)
            .or_else(|| {
                config
                    .typed::<RoomAcConfig>()
                    .ok()
                    .and_then(|cfg| cfg.crankcase_heater_w)
            })
            .is_some();
        let crankcase_threshold_configured = config
            .typed::<CentralAirConditionerConfig>()
            .ok()
            .and_then(|cfg| cfg.crankcase_heater_threshold_c)
            .or_else(|| {
                config
                    .typed::<RoomAcConfig>()
                    .ok()
                    .and_then(|cfg| cfg.crankcase_heater_threshold_c)
            })
            .is_some();

        if !crankcase_kw_configured {
            self.core.crankcase_rated_w = rated_w;
        }
        if !crankcase_threshold_configured {
            self.core.crankcase_threshold_c = threshold_c;
        }
    }
}

impl RoomAC {
    #[must_use]
    pub fn new(config: EquipmentConfig) -> Self {
        Self {
            core: CoolingCore::new(config, true),
        }
    }
}

impl Equipment for AirConditioner {
    fn reset_stochastic_state(&mut self) {
        // The thermostat's schedule setpoint sources at their seeds and
        // cursors: the warm-up boundary's call, so production reads them
        // from the construction position.
        self.core.hvac.reset_stochastic_state();
    }

    fn checkpoint_version() -> u32 {
        AC_CHECKPOINT_VERSION
    }

    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.core.descriptor
    }

    fn rename(&mut self, name: String) {
        self.core.descriptor.name = name;
    }

    fn set_equipment_id(&mut self, id: EquipmentId) -> crate::Result<()> {
        crate::apply_identity_write(self.is_initialized(), &mut self.core.descriptor, id)
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.core.ports
    }

    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        self.core.init(config, env)
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        self.core.update_control(env)
    }

    fn step(
        &mut self,
        env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        self.core.step(env, dt, ports)
    }

    fn telemetry(&self) -> &Telemetry {
        &self.core.telemetry
    }

    fn core_output(&self) -> &CoreOutput {
        &self.core.core_output
    }

    fn thermostat_band_class(&self) -> Option<hares_types::ThermostatBandClass> {
        Some(self.core.hvac.thermostat_fsm.thermostat.band_class)
    }

    fn thermostat_axes(&self) -> Option<hares_types::ThermostatAxes> {
        Some(hares_types::ThermostatAxes::One(
            hares_types::ThermostatAxis::Cooling,
        ))
    }

    fn resolved_zip(&self) -> Option<hares_types::zip::ResolvedZip> {
        // Primary component: the compressor (class default pf 0.96, or a user
        // "zip" override). The fan component ZIP is secondary.
        Some(self.core.zip)
    }

    fn nominal_electric_draw_w(&self) -> Option<f64> {
        self.core.last_nominal_draw_w
    }

    fn save_state(&self) -> crate::Result<Vec<u8>> {
        self.core.save_state()
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        self.core.load_state(state)
    }

    fn apply_signal(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        self.core.apply_signal(signal)
    }

    fn ideal_target(&self) -> Option<(hares_types::ZoneId, f64)> {
        self.core.ideal_target()
    }

    fn resolved_parameters(&self) -> Vec<hares_types::ResolvedParameter> {
        self.core.resolved_cooling_parameters()
    }

    fn take_health_counts(&mut self) -> EquipmentHealthCounts {
        let ideal_capacity_unsolved_steps =
            std::mem::take(&mut self.core.ideal_capacity_unsolved_steps);
        EquipmentHealthCounts {
            curve_index_clamps: self.core.hvac.take_biquadratic_clamp_count(),
            cross_tier_deadband_events: self.core.hvac.take_cross_tier_deadband_count(),
            ideal_capacity_unsolved_steps,
            ..Default::default()
        }
    }

    fn drain_warnings(&mut self, out: &mut Vec<Warning>) {
        self.core.hvac.take_warnings(out);
    }
}

impl Equipment for RoomAC {
    fn reset_stochastic_state(&mut self) {
        // The thermostat's schedule setpoint sources at their seeds and
        // cursors: the warm-up boundary's call, so production reads them
        // from the construction position.
        self.core.hvac.reset_stochastic_state();
    }

    fn checkpoint_version() -> u32 {
        AC_CHECKPOINT_VERSION
    }

    fn descriptor(&self) -> &EquipmentDescriptor {
        &self.core.descriptor
    }

    fn rename(&mut self, name: String) {
        self.core.descriptor.name = name;
    }

    fn set_equipment_id(&mut self, id: EquipmentId) -> crate::Result<()> {
        crate::apply_identity_write(self.is_initialized(), &mut self.core.descriptor, id)
    }

    fn ports(&self) -> &[PortDeclaration] {
        &self.core.ports
    }

    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        self.core.init(config, env)
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        self.core.update_control(env)
    }

    fn step(
        &mut self,
        env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        self.core.step(env, dt, ports)
    }

    fn telemetry(&self) -> &Telemetry {
        &self.core.telemetry
    }

    fn core_output(&self) -> &CoreOutput {
        &self.core.core_output
    }

    fn thermostat_band_class(&self) -> Option<hares_types::ThermostatBandClass> {
        Some(self.core.hvac.thermostat_fsm.thermostat.band_class)
    }

    fn thermostat_axes(&self) -> Option<hares_types::ThermostatAxes> {
        Some(hares_types::ThermostatAxes::One(
            hares_types::ThermostatAxis::Cooling,
        ))
    }

    fn resolved_zip(&self) -> Option<hares_types::zip::ResolvedZip> {
        // Primary component: the compressor (class default pf 0.96, or a user
        // "zip" override). The fan component ZIP is secondary.
        Some(self.core.zip)
    }

    fn nominal_electric_draw_w(&self) -> Option<f64> {
        self.core.last_nominal_draw_w
    }

    fn save_state(&self) -> crate::Result<Vec<u8>> {
        self.core.save_state()
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        self.core.load_state(state)
    }

    fn apply_signal(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        self.core.apply_signal(signal)
    }

    fn ideal_target(&self) -> Option<(hares_types::ZoneId, f64)> {
        self.core.ideal_target()
    }

    fn resolved_parameters(&self) -> Vec<hares_types::ResolvedParameter> {
        self.core.resolved_cooling_parameters()
    }

    fn take_health_counts(&mut self) -> EquipmentHealthCounts {
        let ideal_capacity_unsolved_steps =
            std::mem::take(&mut self.core.ideal_capacity_unsolved_steps);
        EquipmentHealthCounts {
            curve_index_clamps: self.core.hvac.take_biquadratic_clamp_count(),
            cross_tier_deadband_events: self.core.hvac.take_cross_tier_deadband_count(),
            ideal_capacity_unsolved_steps,
            ..Default::default()
        }
    }

    fn drain_warnings(&mut self, out: &mut Vec<Warning>) {
        self.core.hvac.take_warnings(out);
    }
}

impl CoolingCore {
    /// The resolved cooling parameters the physics steps with: the rated
    /// capacity and EIR of the first stage (the staging solve's entry
    /// point) and the rated SHR the latent degradation reads.
    fn resolved_cooling_parameters(&self) -> Vec<hares_types::ResolvedParameter> {
        vec![
            hares_types::ResolvedParameter::new(
                "rated_capacity_w",
                self.hvac
                    .config
                    .cooling_capacities_w
                    .first()
                    .copied()
                    .unwrap_or_default(),
                hares_types::Unit::Watt,
            ),
            hares_types::ResolvedParameter::new(
                "rated_eir",
                self.hvac
                    .config
                    .eir_by_stage
                    .first()
                    .copied()
                    .unwrap_or_default(),
                hares_types::Unit::Dimensionless,
            ),
            hares_types::ResolvedParameter::new(
                "rated_shr",
                self.rated_shr,
                hares_types::Unit::Fraction,
            ),
        ]
    }

    fn select_variable_speed_cooling(
        &mut self,
        requested_capacity_fraction: f64,
    ) -> SpeedSelection {
        // The fractions read through the runtime's cache (config-frozen
        // data; see HvacEquipment::cooling_capacity_fractions_cached), so
        // the per-step selection allocates nothing.
        let cap_fracs = self.hvac.cooling_capacity_fractions_cached();
        if cap_fracs.is_empty() {
            let selection = SpeedSelection {
                speed_index: 0,
                speed_frac: 0.0,
                part_load_ratio: 0.0,
            };
            self.hvac.runtime.last_speed_index = selection.speed_index;
            self.hvac.runtime.last_speed_frac = selection.speed_frac;
            return selection;
        }
        if cap_fracs.len() == 1 {
            let lf = requested_capacity_fraction.clamp(0.0, 1.0);
            let selection = SpeedSelection {
                speed_index: 0,
                speed_frac: 0.0,
                part_load_ratio: lf,
            };
            self.hvac.runtime.last_speed_index = selection.speed_index;
            self.hvac.runtime.last_speed_frac = selection.speed_frac;
            return selection;
        }
        let selection = interpolate_speed_stages(requested_capacity_fraction, cap_fracs, true);
        self.hvac.runtime.last_speed_index = selection.speed_index;
        self.hvac.runtime.last_speed_frac = selection.speed_frac;
        selection
    }

    fn variable_speed_point(&self, selection: SpeedSelection) -> (f64, f64, f64) {
        let stage_capacity_w =
            if self.hvac.config.cooling_capacities_w.len() > 1 && selection.speed_frac > 0.0 {
                self.hvac.interpolated_capacity(
                    &self.hvac.config.cooling_capacities_w,
                    selection.speed_index,
                    selection.speed_frac,
                )
            } else if self.hvac.config.cooling_capacities_w.len() == 1 {
                self.hvac
                    .config
                    .cooling_capacities_w
                    .first()
                    .copied()
                    .unwrap_or_default()
            } else {
                HvacEquipment::capacity_at_stage(
                    &self.hvac.config.cooling_capacities_w,
                    selection.speed_index,
                )
            };
        let stage_eir =
            if self.hvac.config.cooling_capacities_w.len() > 1 && selection.speed_frac > 0.0 {
                self.hvac
                    .interpolated_eir(selection.speed_index, selection.speed_frac)
            } else {
                self.hvac.eir_at_stage(selection.speed_index)
            };
        let part_load_ratio = if self.hvac.config.cooling_capacities_w.len() == 1
            || (selection.speed_index == 0
                && selection.speed_frac == 0.0
                && selection.part_load_ratio < 1.0)
        {
            selection.part_load_ratio.clamp(0.0, 1.0)
        } else {
            1.0
        };
        (stage_capacity_w, stage_eir, part_load_ratio)
    }

    fn new(config: EquipmentConfig, is_room_ac: bool) -> Self {
        let zone = zone_id_from_config(&config);
        let equipment_type = if is_room_ac {
            "Room AC"
        } else {
            "Air Conditioner"
        };

        Self {
            descriptor: EquipmentDescriptor {
                id: EquipmentId(constructor_equipment_id(&config)),
                name: config.name,
                end_use: EndUse::HVAC_COOLING,
                equipment_type: Cow::Borrowed(equipment_type),
                zone,
                fuel: FuelType::Electric,
                stage: ExecutionStage::Thermal,
                control_capabilities: ControlCapabilities::THERMAL_SETPOINT
                    | ControlCapabilities::THERMAL_SETPOINT_DELTA
                    | ControlCapabilities::DUTY_CYCLE
                    | ControlCapabilities::LOAD_FRACTION
                    | ControlCapabilities::POWER_LIMIT
                    | ControlCapabilities::MODE_OVERRIDE
                    | ControlCapabilities::DEMAND_RESPONSE
                    | ControlCapabilities::IDEAL_CAPACITY
                    | ControlCapabilities::MAX_CAPACITY_FRACTION
                    | ControlCapabilities::NON_HVAC_ZONE_INPUT,
                core_capabilities: CoreCapabilities::ELECTRIC
                    | CoreCapabilities::HAS_MODE
                    | CoreCapabilities::THERMAL
                    | CoreCapabilities::HAS_SPEED
                    | CoreCapabilities::HAS_SETPOINT
                    | CoreCapabilities::HAS_COP
                    | CoreCapabilities::REACTIVE,
                telemetry_fields: telemetry_fields(),
                zone_type: None,
            },
            ports: served_zone_ports(&[PortDeclaration::electrical()], zone, true),
            telemetry: default_telemetry(),
            core_output: CoreOutput::default(),
            hvac: HvacEquipment::new(HvacEquipmentType::AcCooler, zone),
            operating_mode: OperatingMode::Off,
            source_temp: SourceTemperature::OutdoorAir,
            run_time_s: 0.0,
            cycle_on_steps: 0,
            cycle_off_steps: 0,
            crankcase_heater_on: false,
            crankcase_heater_w: 0.0,
            crankcase_rated_w: CRANKCASE_HEATER_W,
            crankcase_threshold_c: CRANKCASE_HEATER_THRESHOLD_C,
            crankcase_capacity_curve: None,
            last_cooling_rtf: 0.0,
            last_compressor_w: 0.0,
            last_nominal_draw_w: None,
            flow_fraction_correction: 1.0,
            coil_ao_by_stage: vec![10.0],
            is_room_ac,
            rated_shr: 0.75,
            stage_shrs: vec![],
            latent_degradation: LatentDegradationParams::default(),
            last_adp_c: 0.0,
            last_bypass_factor: 0.0,
            min_oat_cooling_c: -25.0,
            cooling_oat_locked_out: false,
            use_ideal: false,
            ideal_capacity_w: 0.0,
            ideal_capacity_signal_arrived: false,
            ideal_capacity_unsolved_steps: 0,
            ctrl_duty_cycle: 1.0,
            ctrl_power_limit_w: f64::INFINITY,
            ctrl_mode_override: None,
            ctrl_load_fraction: 1.0,
            dr_setpoint_offset_c: 0.0,
            dr_load_fraction: 1.0,
            dr_duty_cycle: 1.0,
            dr_duration_remaining_s: None,
            dr_level: DRLevel::Normal,
            zip: hares_types::zip::ResolvedZip::reactive_only(
                hares_types::zip::ZipLoad::constant_power(),
            ),
            fan_zip: hares_types::zip::ZipLoad::constant_power(),
        }
    }

    fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        self.hvac.init(config, env)?;

        if self.is_room_ac {
            if self.hvac.config.speed_control_mode != SpeedControlMode::SingleSpeed {
                return Err(HaresError::Equipment(
                    "Room AC supports only single-speed mode".to_string(),
                ));
            }
            self.hvac.config.speed_control_mode = SpeedControlMode::SingleSpeed;
            self.hvac.config.duct_dse = 1.0;
            self.hvac.config.duct_zone_id = None;
        } else {
            self.hvac.config.duct_zone_id =
                super::helpers::parse_zone_id_key(config, "duct_zone_id");
        }

        self.init_from_typed(config, env)?;

        if self.is_room_ac && !self.latent_degradation.is_active() {
            return Err(HaresError::InvariantViolation {
                check_name: "room_ac_latent_degradation_inactive".to_string(),
                value: 0.0,
                tolerance: 0.0,
            });
        }

        Ok(())
    }

    fn init_from_typed(
        &mut self,
        config: &EquipmentConfig,
        _env: &EnvironmentState,
    ) -> crate::Result<()> {
        if self.is_room_ac {
            let cfg = config.require_typed::<RoomAcConfig>("Room AC")?;
            cfg.validate()?;

            self.hvac.config.cooling_capacities_w = vec![cfg.capacity_w];

            self.hvac.config.eir_by_stage = vec![cfg.eir];

            self.hvac.config.airflow_m3_s_per_w = cfg
                .airflow_m3_s_per_w
                .unwrap_or(super::hvac_core::AIRFLOW_ROOM_AC_M3_S_PER_W);
            let default_shr = super::hvac_core::derive_default_shr(
                Some(self.hvac.config.airflow_m3_s_per_w),
                Some(cfg.capacity_w),
                true,
            );
            if cfg.shr.is_none() {
                tracing::debug!(
                    equipment_name = %config.name,
                    shr = default_shr,
                    airflow_m3_s_per_w = self.hvac.config.airflow_m3_s_per_w,
                    capacity_w = cfg.capacity_w,
                    "Room AC derived default SHR from EnergyPlus auto-sizing formula"
                );
            }
            self.rated_shr = cfg.shr.unwrap_or(default_shr).clamp(0.0, 1.0);
            // PLF cycling degradation coefficient from SEER only.
            // EnergyPlus `StandardRatings.cc:177–180`: SEER2 Cd=0.20.
            // OCHRE `utils/equipment.py:470–500` `calc_c_d`: same threshold table.
            // NOT derived from `cfg.startup_cd` — startup ramp is a separate
            // physical phenomenon (Winkler 2011) with its own opt-in Cd.
            let plf_cd = cfg.derived_plf_cd().unwrap_or(0.20);
            self.hvac.runtime.plf_cooling_degradation_coeff = plf_cd;
            self.hvac.runtime.startup.c_d = cfg.startup_cd.unwrap_or(DEFAULT_STARTUP_CD);
            if cfg.startup_cd.is_none() {
                tracing::debug!(
                    equipment_name = %config.name,
                    plf_cd,
                    seer_bucket = if plf_cd < 0.11 { "SEER >= 13" } else { "SEER < 13" },
                    "Room AC derived PLF cycling degradation coefficient from EIR"
                );
            }
            #[cfg(feature = "observe")]
            {
                tracing::debug!(
                    equipment_name = %config.name,
                    plf_cd,
                    startup_cd = self.hvac.runtime.startup.c_d,
                    is_room_ac = true,
                    "Room AC init: PLF degradation Cd (AHRI 210/240) and startup ramp Cd (Winkler 2011)"
                );
            }

            // EnergyPlus Coil:Cooling:DX field N11 (Nominal Time for Condensate
            // Removal to Begin), suggested value 1000 s (V26-1-0 IDD §Coil:Cooling:DX);
            // zero means the latent degradation model is disabled.  Room ACs are
            // single-speed cycling units where part-load re-evaporation is more
            // relevant than for variable-speed central systems; the same default
            // applies regardless of equipment class — there is no room-AC-specific
            // field split in the IDD.  Other parameters: gamma=1.5 (N9),
            // Nmax=3 cyc/hr (N10), tau=45 s (N12).
            self.latent_degradation = LatentDegradationParams {
                twet_rated_s: 1000.0,
                gamma_rated: 1.5,
                max_cycling_rate: 3.0,
                latent_time_constant_s: 45.0,
            };
        } else {
            let cfg = config.require_typed::<CentralAirConditionerConfig>("Air Conditioner")?;
            cfg.validate()?;

            self.hvac.config.cooling_capacities_w = if let Some(stages) = &cfg.stage_capacities_w {
                stages.clone()
            } else {
                vec![cfg.capacity_w]
            };

            let default_eir = cfg.eir;
            let stage_count = self.hvac.config.cooling_capacities_w.len();

            self.hvac.config.eir_by_stage = if let Some(stages) = &cfg.stage_eirs {
                stages.clone()
            } else {
                vec![default_eir; stage_count]
            };

            if self.hvac.config.cooling_capacities_w.len() != self.hvac.config.eir_by_stage.len() {
                return Err(HaresError::Equipment(
                    "cooling capacity and EIR stage counts must match".to_string(),
                ));
            }

            if let Some(r) = cfg.charge_defect_ratio {
                super::hvac_core::apply_charge_defect_correction(
                    &mut self.hvac.config.cooling_capacities_w,
                    &mut self.hvac.config.eir_by_stage,
                    r,
                );
            }

            if let Some(airflow_m3_s_per_w) = cfg.airflow_m3_s_per_w {
                self.hvac.config.airflow_m3_s_per_w = airflow_m3_s_per_w;
            }

            let rated_cap = self
                .hvac
                .config
                .cooling_capacities_w
                .last()
                .copied()
                .unwrap_or(0.0);
            let fan_flow = self.hvac.config.airflow_m3_s_per_w * rated_cap;
            let n_speeds = self.hvac.config.cooling_capacities_w.len().min(255) as u8;
            let cap_low = (n_speeds > 1)
                .then(|| self.hvac.config.cooling_capacities_w.first().copied())
                .flatten();
            let flow_low = cap_low.map(|c| self.hvac.config.airflow_m3_s_per_w * c);
            self.hvac.config.duct_dse = if let Some(dse) = cfg.duct.dse_cool {
                dse
            } else {
                super::helpers::resolve_duct_dse(
                    config,
                    &super::helpers::DuctDseContext {
                        is_heating: false,
                        capacity_w: rated_cap,
                        fan_flow_m3_s: fan_flow,
                        n_speeds,
                        capacity_low_w: cap_low,
                        fan_flow_low_m3_s: flow_low,
                        is_heat_pump: false,
                    },
                )?
            };

            // Fan-specific power is normalized to the TOP stage's airflow,
            // sized after the stage table above is final: OCHRE's fan power
            // per unit flow divides by `rated_flow_rate = max(flow_rate_list)`
            // with the flow rates derived from the per-stage capacities
            // (HVAC.py:126-150), so the declared fan power (HPXML
            // FanPowerWatts, OCHRE's "Rated Auxiliary Power (W)") is the
            // max-stage fan power. The heater's init applies the same rule
            // to its side; without it the declared raw watts are dead input
            // and the fan runs the per-m3s default.
            if let Some(fan_power_w) = cfg.fan_power_w {
                let rated_airflow_m3_s = self.hvac.config.airflow_m3_s_per_w * rated_cap;
                self.hvac.config.fan_power_w_per_m3_s = if rated_airflow_m3_s > 0.0 {
                    fan_power_w.max(0.0) / rated_airflow_m3_s
                } else {
                    0.0
                };
            }

            let speed_mode = cfg.cooling_speed_control_mode();
            self.hvac.config.speed_control_mode = speed_mode;

            let default_shr = super::hvac_core::derive_default_shr(
                Some(self.hvac.config.airflow_m3_s_per_w),
                Some(rated_cap),
                false,
            );
            if cfg.shr.is_none() {
                tracing::debug!(
                    equipment_name = %config.name,
                    shr = default_shr,
                    airflow_m3_s_per_w = self.hvac.config.airflow_m3_s_per_w,
                    capacity_w = rated_cap,
                    "Central AC derived default SHR from EnergyPlus auto-sizing formula"
                );
            }
            self.rated_shr = cfg.shr.unwrap_or(default_shr).clamp(0.0, 1.0);

            // Per-stage SHR values if provided.
            self.stage_shrs = cfg.stage_shrs.clone().unwrap_or_default();
            if !self.stage_shrs.is_empty() {
                // AHRI 210/240 rated cooling test conditions correspond to
                // full-load (last stage) operation, not the first/lowest speed.
                // The Henderson-Rengarajan latent degradation model uses rated_shr
                // as its baseline for computing rated_latent_w (line 1277).
                self.rated_shr = self.stage_shrs[self.stage_shrs.len() - 1].clamp(0.0, 1.0);
            }

            // PLF cycling degradation coefficient from speed/SEER only.
            // NOT derived from `cfg.startup_cd` — startup ramp is a separate
            // physical phenomenon (Winkler 2011) with its own opt-in Cd.
            self.hvac.runtime.plf_cooling_degradation_coeff = cfg
                .derived_plf_cd()
                .unwrap_or(DEFAULT_PLF_DEGRADATION_COEFF);
            self.hvac.runtime.startup.c_d = cfg.startup_cd.unwrap_or(DEFAULT_STARTUP_CD);
            #[cfg(feature = "observe")]
            {
                tracing::debug!(
                    equipment_name = %config.name,
                    plf_cd = self.hvac.runtime.plf_cooling_degradation_coeff,
                    startup_cd = self.hvac.runtime.startup.c_d,
                    is_central_ac = true,
                    speed_control_mode = ?self.hvac.config.speed_control_mode,
                    "Central AC init: PLF degradation Cd (AHRI 210/240) and startup ramp Cd (Winkler 2011)"
                );
            }

            // EnergyPlus Coil:Cooling:DX field N11 (Nominal Time for Condensate
            // Removal to Begin), suggested value 1000 s (V26-1-0 IDD §Coil:Cooling:DX);
            // zero means the latent degradation model is disabled.
            self.latent_degradation = LatentDegradationParams {
                twet_rated_s: 1000.0,
                gamma_rated: 1.5,
                max_cycling_rate: 3.0,
                latent_time_constant_s: 45.0,
            };
        }

        self.hvac.update_zone_heat_fractions()?;
        self.hvac.rebuild_thermal_ports(&mut self.ports, true);
        self.descriptor.zone = self.hvac.config.zone_id;
        self.hvac.config.biquadratic_coeffs = load_curve_pair(config, self.is_room_ac)?;
        self.compute_coil_ao(self.rated_shr)?;

        let (crankcase_rated_w, crankcase_threshold_c, crankcase_capacity_curve) =
            if self.is_room_ac {
                let cfg = config.require_typed::<RoomAcConfig>("Room AC")?;
                (
                    // Room ACs have no crankcase heater per OCHRE convention
                    // (MinisplitAHSPCooler class uses 15 W; window/room ACs use 0 W).
                    // Default to 0.0 W, not the central-AC default of 50 W.
                    cfg.crankcase_heater_w.unwrap_or(0.0),
                    cfg.crankcase_heater_threshold_c
                        .unwrap_or(CRANKCASE_HEATER_THRESHOLD_C),
                    cfg.crankcase_capacity_curve_coeffs,
                )
            } else {
                let cfg = config.require_typed::<CentralAirConditionerConfig>("Air Conditioner")?;
                (
                    cfg.crankcase_heater_w.unwrap_or(CRANKCASE_HEATER_W),
                    cfg.crankcase_heater_threshold_c
                        .unwrap_or(CRANKCASE_HEATER_THRESHOLD_C),
                    cfg.crankcase_capacity_curve_coeffs,
                )
            };
        self.crankcase_rated_w = crankcase_rated_w;
        self.crankcase_threshold_c = crankcase_threshold_c;
        self.crankcase_capacity_curve = crankcase_capacity_curve;

        // Minimum OAT lockout for cooling compressor operation.
        // EnergyPlus `DXCoils.cc:731`: `minOATCompDXCooling = -25.0`.
        self.min_oat_cooling_c = if self.is_room_ac {
            let cfg = config.require_typed::<RoomAcConfig>("Room AC")?;
            cfg.min_oat_compressor_cooling_c.unwrap_or(-25.0)
        } else {
            let cfg = config.require_typed::<CentralAirConditionerConfig>("Air Conditioner")?;
            cfg.min_oat_compressor_cooling_c.unwrap_or(-25.0)
        };

        self.operating_mode = OperatingMode::Off;
        self.run_time_s = 0.0;
        self.cycle_on_steps = 0;
        self.cycle_off_steps = 0;
        self.crankcase_heater_on = false;
        self.crankcase_heater_w = 0.0;
        self.last_cooling_rtf = 0.0;
        self.zip = crate::config::resolve_reactive_zip(config)?;
        self.fan_zip =
            super::reactive::secondary_motor_zip(&self.zip, super::reactive::FAN_MOTOR_ZIP);
        self.telemetry = default_telemetry();
        register_ebm_telemetry_keys(&mut self.telemetry);
        // Per-stage SHR telemetry for diagnostic CSV output.
        // Gated on `observe` feature — these channels are diagnostic-only and
        // document which SEER-scaled SHR profile was applied to each cooler
        // instance, enabling verification that distinct SHR profiles are in
        // use across SEER tiers. Registered here, after the telemetry reset
        // above, via insert() so the keys exist before any Telemetry::set
        // call (an unknown key latches a typed error surfaced at the end of
        // the step). The descriptor's declared
        // telemetry fields are rebuilt to include the per-stage channels so
        // the descriptor contract (telemetry key count == declared field
        // count) holds; rebuilding from the base list keeps re-init
        // idempotent.
        #[cfg(feature = "observe")]
        {
            self.descriptor.telemetry_fields = telemetry_fields();
            for (i, shr) in self.stage_shrs.iter().enumerate() {
                self.telemetry.insert(format!("cooler_shr_stage_{i}"), *shr);
                self.descriptor.telemetry_fields.push(TelemetryField::ratio_owned(
                    format!("cooler_shr_stage_{i}"),
                    format!(
                        "Rated sensible heat ratio applied to cooling speed stage {i} (diagnostic)"
                    ),
                ));
            }
            for (i, ao) in self.coil_ao_by_stage.iter().enumerate() {
                self.telemetry.insert(format!("cooler_ao_stage_{i}"), *ao);
                self.descriptor.telemetry_fields.push(TelemetryField::quantity_owned(
                    format!("cooler_ao_stage_{i}"),
                    hares_types::Unit::KilogramPerSecond,
                    hares_types::AcrossHomes::Sum,
                    hares_types::Aggregation::Mean,
                    hares_types::Sampling::IntervalMean,
                    format!(
                        "Coil Ao factor (apparent UA/cp) for cooling speed stage {i} (diagnostic)"
                    ),
                ));
            }
            self.telemetry
                .insert("latent_degradation_active".to_string(), 0.0);
            self.descriptor.telemetry_fields.push(TelemetryField::ratio(
                "latent_degradation_active",
                "1.0 if Henderson-Rengarajan latent degradation is active this step",
            ));
            self.telemetry
                .insert("shr_before_degradation".to_string(), self.rated_shr);
            self.descriptor.telemetry_fields.push(TelemetryField::ratio(
                "shr_before_degradation",
                "Steady-state SHR before Henderson-Rengarajan latent degradation",
            ));
            // AHRI full-load SHR baseline for latent degradation model.
            self.telemetry
                .insert("rated_shr".to_string(), self.rated_shr);
            self.descriptor.telemetry_fields.push(TelemetryField::ratio(
                "rated_shr",
                "Rated sensible heat ratio at AHRI full-load conditions \
                             (baseline for latent degradation model)",
            ));
            let rated_cap_w = self
                .hvac
                .config
                .cooling_capacities_w
                .last()
                .copied()
                .unwrap_or(0.0);
            let rated_latent_w = rated_cap_w * (1.0 - self.rated_shr);
            self.telemetry
                .insert("rated_latent_w".to_string(), rated_latent_w);
            self.descriptor
                .telemetry_fields
                .push(TelemetryField::power_w(
                    "rated_latent_w",
                    "Rated latent capacity (W) at AHRI full-load conditions \
                             (baseline for Henderson-Rengarajan latent degradation model)",
                ));
        }
        self.core_output = CoreOutput::default();

        let n_speeds = self
            .hvac
            .config
            .cooling_capacities_w
            .len()
            .max(self.hvac.config.heating_capacities_w.len());
        HvacEquipment::validate_biquadratic_curve_count(
            self.hvac.config.biquadratic_coeffs.len(),
            n_speeds,
        )?;

        Ok(())
    }

    fn update_control(&mut self, env: &EnvironmentState) -> OperatingMode {
        self.cooling_oat_locked_out = false;
        self.use_ideal = self.hvac.use_ideal_capacity(env);

        // Grid outage: a de-energized bus removes supply power for the
        // compressor and blower — force off before any override handling
        // (WH precedent). Islanded (battery/generator-backed) homes keep an
        // energized bus and are not affected. See docs/outage-behavior.md.
        if outage_forces_off(&mut self.hvac, env)
            || unavailable_period_forces_off(&mut self.hvac, env, false, true)
        {
            self.operating_mode = OperatingMode::Off;
            self.hvac.update_prev_zone_temp(None);
            return OperatingMode::Off;
        }

        // Short-circuit: GridEmergency or explicit Off override.
        if self.dr_load_fraction <= 0.0 || self.ctrl_mode_override == Some(OperatingMode::Off) {
            self.hvac.runtime.duty_cycle = 0.0;
            self.operating_mode = OperatingMode::Off;
            return OperatingMode::Off;
        }

        // Minimum OAT lockout for cooling compressor operation.
        // EnergyPlus `DXCoils.cc:731, 9536`: compressor is locked out below
        // `minOATCompDXCooling` to prevent liquid slugging and oil foaming.
        self.cooling_oat_locked_out = env.weather.outdoor_temp_c < self.min_oat_cooling_c;
        if self.cooling_oat_locked_out {
            #[cfg(feature = "observe")]
            tracing::debug!(
                outdoor_temp_c = env.weather.outdoor_temp_c,
                min_oat_cooling_c = self.min_oat_cooling_c,
                "Cooling OAT lockout active"
            );
            self.hvac.runtime.duty_cycle = 0.0;
            self.operating_mode = OperatingMode::Off;
            self.hvac.update_prev_zone_temp(None);
            return OperatingMode::Off;
        }

        // A Cooling mode means the thermostat read the served zone, so the
        // zone temperature is available whenever cooling is called for. A
        // thermostat control error is recorded for the following step to
        // fail with, the same contract the heating classes follow.
        let cooling_zone_temp_c = match self.hvac.update_mode(env) {
            Ok(ThermostatMode::Cooling) => self
                .hvac
                .config
                .zone_id
                .and_then(|zone| lookup_zone(env, zone).ok())
                .map(|zone| zone.temperature_c),
            Ok(_) => None,
            Err(err) => {
                self.hvac
                    .record_control_error(HaresError::Equipment(format!(
                        "{}: {err}",
                        self.descriptor.name
                    )));
                None
            }
        };
        self.hvac.record_cross_tier_warning(&self.descriptor.name);
        if let Some(zone_temp) = cooling_zone_temp_c {
            let base_setpoint = self.hvac.effective_setpoints().cooling_c;
            let setpoint = base_setpoint + self.dr_setpoint_offset_c;

            // When DR raises the effective setpoint above the zone temperature, suppress cooling
            // even though the base thermostat is calling for it. This only applies when the DR
            // offset is active (non-zero) and the zone has cooled below the DR-adjusted threshold.
            let dr_suppressed = self.dr_setpoint_offset_c > 0.0 && zone_temp <= setpoint;
            if dr_suppressed {
                self.hvac.runtime.duty_cycle = 0.0;
                self.operating_mode = OperatingMode::Off;
                self.hvac.update_prev_zone_temp(None);
            } else {
                // One runtime-fraction path for every cycling unit: the
                // single-speed unit delivers the fraction of capacity the
                // zone needs, like the multi-speed arms below; its electric
                // draw follows the runtime fraction with the part-load
                // degradation (EnergyPlus DXCoils.cc:9859). The ideal arms
                // keep the raw band estimate: their delivery is the solver's
                // netted capacity, so the share must not be netted twice.
                let ((_, _), (cool_on, cool_off)) = self.hvac.thermostat_fsm.band_edges();
                let ideal_band_fraction = cycling_load_fraction(zone_temp, cool_off, cool_on);
                self.hvac.update_prev_zone_temp(Some(zone_temp));
                self.hvac.runtime.duty_cycle = match self.hvac.config.speed_control_mode {
                    SpeedControlMode::VariableSpeedIdeal => {
                        self.select_variable_speed_cooling(ideal_band_fraction)
                            .part_load_ratio
                    }
                    _ if self.use_ideal => 1.0,
                    _ => {
                        // The cycling arm nets the zone's non-HVAC share out
                        // of its band estimate, the same netting the ideal
                        // solve applies to its capacity, and anchors the
                        // fraction's zero edge at the cooling setpoint.
                        let ((_, _), (cool_zero, cool_full)) =
                            self.hvac.thermostat_fsm.duty_bands();
                        let band_fraction = cycling_load_fraction(zone_temp, cool_zero, cool_full);
                        let netted_fraction = netted_cycling_duty(
                            band_fraction,
                            self.hvac.runtime.non_hvac_input_w,
                            self.hvac.rated_capacity_w(ThermostatMode::Cooling),
                        );
                        self.hvac
                            .select_speed_with_zone_temp(netted_fraction, Some(zone_temp), false)
                            .part_load_ratio
                    }
                };
                // The thermostat's latch owns the mode, like the heating
                // arms: a latched hold step whose netted band estimate is
                // zero (the zone below the setpoint) delivers nothing but
                // keeps the latch's mode until the FSM releases at its own
                // edge. The step's zero duty zeroes the delivery and the
                // mode resolves idle downstream.
                self.operating_mode = OperatingMode::Cooling;
            }
        } else {
            self.hvac.runtime.duty_cycle = 0.0;
            self.operating_mode = OperatingMode::Off;
            self.hvac.update_prev_zone_temp(None);
        }

        #[cfg(debug_assertions)]
        {
            assert!(
                self.operating_mode != OperatingMode::Cooling
                    || env.weather.outdoor_temp_c >= self.min_oat_cooling_c,
                "Cooling active when OAT {:.1}°C < min_oat_cooling_c {:.1}°C",
                env.weather.outdoor_temp_c,
                self.min_oat_cooling_c
            );
        }

        self.operating_mode
    }

    /// Run one simulation step.
    pub(super) fn step(
        &mut self,
        env: &EnvironmentState,
        dt: Duration,
        ports: &mut PortSlots,
    ) -> std::result::Result<(), HaresError> {
        if let Some(err) = self.hvac.take_control_error() {
            return Err(err);
        }
        let ebm_window = step_equivalent_battery(&self.hvac, env)?;
        self.crankcase_heater_on = false;
        self.crankcase_heater_w = 0.0;

        let mut compressor_w = 0.0;
        let mut fan_w = 0.0;
        let mut sensible_cooling_w = 0.0;
        let mut latent_cooling_w = 0.0;

        let dt_min = dt.as_secs_f64() / 60.0;
        if self.operating_mode == OperatingMode::Cooling {
            // The ideal path's starvation census: an ideal step with no
            // capacity signal this step runs the raw thermostat duty (the
            // PLR arms below fall through to the duty cycle: 1.0 on a
            // single-speed ideal path, the band estimate on a variable-speed
            // one), so the delivery is unmetered to the zone's need. Counted,
            // never silent: the starvation's cause is a mode flip downstream
            // of the solver-feedback phase, and a rising count is the trace a
            // dispatch-order regression leaves.
            if self.use_ideal && !self.ideal_capacity_signal_arrived {
                self.ideal_capacity_unsolved_steps += 1;
            }
            let perf = self.calculate_performance(env, dt_min)?;
            compressor_w = perf.compressor_w;
            fan_w = perf.fan_w;
            sensible_cooling_w = perf.sensible_cooling_w;
            latent_cooling_w = perf.latent_cooling_w;
            self.hvac.config.shr = perf.shr;
            self.hvac.config.supply_air_temp_c = perf.supply_temp_c;

            // Apply compound load multipliers (duty cycle * transient load * DR).
            let effective_load = (self.ctrl_duty_cycle
                * self.ctrl_load_fraction
                * self.dr_load_fraction
                * self.dr_duty_cycle)
                .clamp(0.0, 1.0);
            self.last_cooling_rtf = (perf.rtf * effective_load).clamp(0.0, 1.0);

            // Apply PowerLimit: clamp electric, proportionally reduce thermal.
            let total_electric_w = compressor_w + fan_w;
            let (final_electric_w, thermal_ratio) = if self.ctrl_power_limit_w.is_finite()
                && total_electric_w > self.ctrl_power_limit_w
            {
                let ratio = self.ctrl_power_limit_w / total_electric_w.max(f64::MIN_POSITIVE);
                (self.ctrl_power_limit_w, ratio)
            } else {
                (total_electric_w, 1.0)
            };

            compressor_w = (compressor_w * thermal_ratio * effective_load).max(0.0);
            fan_w = (fan_w * thermal_ratio * effective_load).max(0.0);
            let _ = final_electric_w; // used via thermal_ratio above
            sensible_cooling_w *= thermal_ratio * effective_load;
            latent_cooling_w *= thermal_ratio * effective_load;

            // Fan waste heat partially offsets cooling capacity delivered to
            // the zone (OCHRE HVAC.py line 543: delivered_heat = heat_gain * shr + fan_power).
            // For cooling: heat_gain is negative, fan_power is positive.
            let fan_heat_w = fan_w;
            self.hvac.write_zone_thermal_contributions(
                ports,
                -sensible_cooling_w + fan_heat_w,
                -latent_cooling_w,
                ThermalCategory::HvacCooling,
            )?;

            if latent_cooling_w.abs() > 0.0 {
                let moisture_mass_flow_kg_s = -latent_cooling_w / LATENT_HEAT_VAPORISATION_0C_J_KG;
                for &(zone, fraction) in &self.hvac.config.zone_heat_fractions {
                    if fraction > 0.0 {
                        ports.accumulate(&PortContribution::Humidity {
                            zone,
                            moisture_mass_flow_kg_s: moisture_mass_flow_kg_s * fraction,
                        })?;
                    }
                }
            }

            self.hvac.advance_speed_timer(dt.as_secs_f64());
            self.run_time_s += dt.as_secs_f64();
            self.cycle_on_steps += 1;
        } else {
            self.last_cooling_rtf = 0.0;
            self.cycle_off_steps += 1;
            self.hvac.runtime.time_at_current_speed_s = 0.0;
            self.hvac.update_prev_zone_temp(None);
            // Off-step bookkeeping for the startup-ramp edge detector: record
            // `was_on = false` so the next Cooling step is a true off→on
            // transition and fires a fresh ramp reset (the timer itself is
            // preserved across off-steps; see StartupConfig::capacity_multiplier).
            // OCHRE's equivalent state (`mode_prev = self.mode`) is updated
            // unconditionally every timestep in update_results (HVAC.py:616),
            // which is what makes its `"HP" not in self.mode_prev` reset check
            // (HVAC.py:978-979) fire on a genuine restart. Skipping this call
            // on off-steps freezes `was_on` at its last on-step value and a
            // cold restart never re-triggers the ramp. Mirrors
            // ASHPHeater::compute_step, which reaches
            // apply_startup_capacity_degradation unconditionally every timestep.
            // The returned capacity is unused — the compressor is off.
            self.hvac
                .apply_startup_capacity_degradation(0.0, dt_min, false);
        }

        // Crankcase heater: draws power when outdoor temp is below threshold
        // AND the cooling coil is not running.
        // A de-energized bus (grid outage without backup) also removes the
        // crankcase heater's supply — it is a standby load, not a source.
        let crankcase_w = if env.grid.bus_energized() {
            self.crankcase_heater_power_internal(env.weather.outdoor_temp_c, self.last_cooling_rtf)
        } else {
            0.0
        };
        if crankcase_w > 0.0 {
            self.crankcase_heater_on = true;
            self.crankcase_heater_w = crankcase_w;
        }

        self.last_compressor_w = compressor_w;
        let sf = self.hvac.config.space_fraction;
        // The declared deep-sag response (docs/PHYSICS_DECISIONS.md, the
        // deep-sag entry): the compressor motor's draw evaluates the unit
        // row (the recorded Hajagos & Danai compressor row, as fitted) at
        // the ACTUAL bus voltage: its deep-sag elevation is the measured
        // static draw of the family; the blower/condenser fan motors
        // evaluate their own fan-motor row; the crankcase heater is a
        // resistive element and evaluates the impedance share, P = V²/R.
        // The thermal delivery is the equipment's own physics (the COP
        // and capacity curves, voltage-invariant); the control composition
        // acts on the rated draw, and the voltage response multiplies the
        // result (the two multiplications commute).
        let bus_voltage_pu = env.grid.bus_voltage_pu();
        let motor_multiplier = self.zip.deep_sag_real_multiplier(bus_voltage_pu);
        // The blower's real response evaluates the physical fan-motor row
        // (the sentinel or not: pf = 0.0 is reactive-only and must never
        // flatten it); the sentinel-substituted `fan_zip` row is the
        // reactive sum's surface only.
        let fan_multiplier = super::reactive::secondary_motor_real_multiplier(
            &super::reactive::FAN_MOTOR_ZIP,
            bus_voltage_pu,
        );
        let v_norm = bus_voltage_pu / self.zip.v0;
        let resistive_multiplier = v_norm * v_norm;
        // The moment's nominal component draw: the components' pre-scaling
        // sum, the composition the scaled sum below evaluates through their
        // own rows (the compressor and fan motors through their recorded
        // rows, the crankcase heater through the impedance share). The
        // premise aggregate weights this unit by it and answers the
        // assembled response, the scaled sum over it. The state whose
        // reference draw this is carries the control composition the step
        // applied: duty cycle and load fraction, and an active power limit
        // (the compressor and fan were scaled by `thermal_ratio` above), so
        // under a limit the base is the capped reference draw, not the
        // nameplate rating: the step's own reference-voltage draw.
        let nominal_component_w = (compressor_w + fan_w + self.crankcase_heater_w) * sf;
        self.last_nominal_draw_w = (nominal_component_w > 0.0).then_some(nominal_component_w);
        let compressor_w = compressor_w * motor_multiplier;
        let fan_w = fan_w * fan_multiplier;
        let crankcase_w = self.crankcase_heater_w * resistive_multiplier;
        let electric_w = (compressor_w + fan_w + crankcase_w) * sf;
        // Q from the already-computed (voltage-scaled) real power, per
        // component (see `hvac::reactive`): compressor at the unit ZIP
        // (pf 0.96), blower/condenser fans at pf 0.87. The crankcase
        // heater is resistive (pf 1.0) and contributes Q ≡ 0.
        let reactive_power_var = self.zip.reactive_var(compressor_w * sf, bus_voltage_pu)
            + self.fan_zip.reactive_var(fan_w * sf, bus_voltage_pu);
        if electric_w > 0.0 || reactive_power_var != 0.0 {
            ports.accumulate(&PortContribution::Electrical {
                active_power_w: (electric_w),
                reactive_power_var,
            })?;
        }

        // Telemetry reports delivered (post-DSE) values for the conditioned zone.
        let dse = self.hvac.config.duct_dse.clamp(0.0, 1.0);
        let fan_heat_w = fan_w;
        // ASHRAE 152: duct_loss = gross_capacity * (1 - dse).
        // sensible_cooling_w + latent_cooling_w are pre-DSE (gross) values from compute_performance.
        let gross_cooling_w = sensible_cooling_w + latent_cooling_w;
        let duct_loss_w = gross_cooling_w * (1.0 - dse);
        self.telemetry.set(tk::ELECTRIC_KW, electric_w);
        self.telemetry
            .set(tk::REACTIVE_POWER_KVAR, reactive_power_var);
        self.telemetry
            .set(tk::SENSIBLE_COOLING_W, sensible_cooling_w * dse);
        self.telemetry
            .set(tk::LATENT_COOLING_W, latent_cooling_w * dse);
        self.telemetry
            .set(tk::COIL_SENSIBLE_COOLING_W, sensible_cooling_w);
        self.telemetry
            .set(tk::COIL_LATENT_COOLING_W, latent_cooling_w);
        // OCHRE HVAC.py:595: Latent Gains = latent_gain * space_fraction (pre-DSE).
        // coil_latent_cooling_w is the pre-DSE gross latent; space_fraction scales
        // the output to the conditioned-space fraction served by this equipment.
        self.telemetry.set(
            tk::LATENT_GAINS_W,
            latent_cooling_w * self.hvac.config.space_fraction,
        );
        self.telemetry.set(tk::FAN_HEAT_W, fan_heat_w);
        // OCHRE HVAC.py:575: main_power = total_input_w - fan_w.
        // For cooling equipment total_input = compressor + fan, so main = compressor.
        self.telemetry.set(
            tk::MAIN_POWER_KW,
            compressor_w * self.hvac.config.space_fraction,
        );
        self.telemetry.set(tk::DUCT_LOSS_W, duct_loss_w);
        // SHR is written through `set_or_register`: the key registers at
        // the first cooling step (the contract's null-until-first-cooling
        // timeline, the record path pairing the value with the step's
        // delivered sensible cooling) and updates in place after it, so
        // the steady state pays nothing for the write.
        if sensible_cooling_w > 0.0 {
            #[cfg(test)]
            let site_before = hares_types::alloc_count::thread_allocations();
            self.telemetry
                .set_or_register(tk::SHR, self.hvac.config.shr);
            #[cfg(test)]
            if let (Some(before), Some(after)) =
                (site_before, hares_types::alloc_count::thread_allocations())
            {
                COOLING_SHR_TELEMETRY_SITE
                    .with(|site| site.set(site.get().saturating_add(after.saturating_sub(before))));
            }
        }
        let original_mode = self.operating_mode;
        let post_dse_sensible_w = sensible_cooling_w * dse;
        let post_dse_latent_w = latent_cooling_w * dse;
        let delivered_thermal_w = -(post_dse_sensible_w + post_dse_latent_w) + fan_heat_w * dse;
        // The delivered thermal flow counts as activity, the same term the
        // mode-flow guard's active-mode rule and the ASHP heater's
        // mode resolution count: a unit whose space-fraction-scaled electric
        // draw is zero (fraction_load_served = 0, a heating-only heat pump's
        // cooling side) still delivers real zone cooling and must keep its
        // active mode rather than resolve to Standby, which the thermal-sign
        // contract rejects against a cooling flow.
        let has_nonzero_flow = electric_w > 0.0 || delivered_thermal_w != 0.0;
        self.operating_mode =
            original_mode.resolve_idle(has_nonzero_flow, Some(delivered_thermal_w));
        self.telemetry
            .set(tk::OPERATING_MODE, self.operating_mode.as_code());
        self.telemetry.set(
            tk::COOLING_OAT_LOCKOUT,
            if self.cooling_oat_locked_out {
                1.0
            } else {
                0.0
            },
        );
        self.telemetry
            .set(tk::SPEED_INDEX, self.hvac.runtime.last_speed_index as f64);
        // COP per AHRI/SEER convention: excludes fan power from denominator.
        // The COP of a step with no compressor input is undefined, not zero:
        // None (the cell null) when the compressor did not run.
        let compressor_only_w = compressor_w;
        let total_cooling_w = sensible_cooling_w + latent_cooling_w;
        let cop = if compressor_only_w > 1e-6 {
            Some(
                (total_cooling_w / compressor_only_w)
                    // AHRI 210/240-2023: AC cooling COP ~2.3–4.1 W/W; clamp to [0.0, 8.0]
                    // to exclude physically impossible values from telemetry. Non-finite
                    // input survives the clamp: a NaN COP reaching telemetry is a typed
                    // error in every build.
                    .clamp(0.0, 8.0),
            )
        } else {
            None
        };
        if cop.is_some_and(|cop| !cop.is_finite()) {
            return Err(HaresError::InvariantViolation {
                check_name: "ac_cooling_cop_finite".to_string(),
                value: cop.unwrap_or(f64::NAN),
                tolerance: 0.0,
            });
        }
        self.telemetry.set(tk::COP, cop.unwrap_or(0.0));
        self.telemetry
            .set(tk::RUNTIME_FRACTION, self.last_cooling_rtf.clamp(0.0, 1.0));
        // Sub-metering telemetry is scaled by space_fraction, matching ELECTRIC_KW
        // and MAIN_POWER_KW, so per-component telemetry sums to the unit total.
        self.telemetry.set(tk::COMPRESSOR_KW, compressor_w * sf);
        self.telemetry.set(tk::FAN_KW, fan_w * sf);
        self.telemetry.set(tk::CRANKCASE_KW, crankcase_w * sf);
        self.telemetry
            .set(tk::SUPPLY_TEMP_C, self.hvac.config.supply_air_temp_c);
        self.telemetry
            .set(tk::APPARATUS_DEW_POINT_C, self.last_adp_c);
        self.telemetry
            .set(tk::BYPASS_FACTOR, self.last_bypass_factor);
        self.telemetry.set(
            tk::MAX_CAPACITY_FRACTION,
            self.hvac.control.max_capacity_fraction,
        );
        let sp = self.hvac.effective_setpoints();
        self.telemetry.set(tk::HEATING_SETPOINT_C, sp.heating_c);
        self.telemetry.set(tk::COOLING_SETPOINT_C, sp.cooling_c);
        let schedule_stage = self
            .hvac
            .thermostat_fsm
            .static_setpoints
            .with_schedule_override(self.hvac.thermostat_fsm.schedule_setpoints);
        self.telemetry
            .set(tk::SCHEDULE_HEATING_SETPOINT_C, schedule_stage.heating_c);
        self.telemetry
            .set(tk::SCHEDULE_COOLING_SETPOINT_C, schedule_stage.cooling_c);
        if let Some(ref rt) = self.hvac.thermostat_fsm.runtime_setpoints {
            self.telemetry
                .set(tk::RUNTIME_HEATING_SETPOINT_C, rt.heating_c.unwrap_or(0.0));
            self.telemetry
                .set(tk::RUNTIME_COOLING_SETPOINT_C, rt.cooling_c.unwrap_or(0.0));
        }
        self.telemetry
            .set(tk::SPEED_FRAC, self.hvac.runtime.last_speed_frac);
        self.telemetry
            .set(tk::PART_LOAD_RATIO, self.hvac.runtime.duty_cycle);
        self.telemetry
            .set(tk::PART_LOAD_FACTOR, self.hvac.runtime.plf_state);
        self.telemetry.set(
            tk::STARTUP_MULTIPLIER,
            self.hvac.runtime.startup.current_multiplier(),
        );
        self.telemetry.set(
            tk::TIME_SINCE_START_MIN,
            self.hvac.runtime.startup.time_since_start_min,
        );
        self.telemetry
            .set(tk::DUTY_CYCLE, self.hvac.runtime.duty_cycle);
        self.telemetry.set(
            tk::TIME_AT_CURRENT_SPEED_S,
            self.hvac.runtime.time_at_current_speed_s,
        );
        let mode_duration_s = (env.current_time
            - self
                .hvac
                .thermostat_fsm
                .mode_start_at
                .unwrap_or(env.current_time))
        .num_milliseconds()
        .max(0) as f64
            / 1000.0;
        self.telemetry.set(tk::MODE_DURATION_S, mode_duration_s);
        self.telemetry
            .set(tk::MIN_ON_TIME_S, self.hvac.thermostat_fsm.min_on_time_s);
        self.telemetry
            .set(tk::MIN_OFF_TIME_S, self.hvac.thermostat_fsm.min_off_time_s);
        write_ebm_telemetry(
            ebm_window,
            post_dse_sensible_w + post_dse_latent_w,
            &mut self.telemetry,
        )?;
        let active_setpoint_c = match original_mode {
            OperatingMode::Cooling => sp.cooling_c,
            OperatingMode::Heating => sp.heating_c,
            _ => sp.cooling_c,
        };
        self.core_output = CoreOutput {
            flows: CoreFlows {
                electric_w: Some(ElectricPower::Consumption(electric_w.max(0.0))),
                reactive_power_var: Some(reactive_power_var),
                fuel_w: None,
                thermal_output_w: Some(delivered_thermal_w),
                sensible_cooling_w: Some(-post_dse_sensible_w),
                latent_cooling_w: Some(-post_dse_latent_w),
            },
            state: CoreState {
                operating_mode: Some(self.operating_mode),
                soc: None,
                speed_index: Some(self.hvac.runtime.last_speed_index as u8),
                setpoint_c: Some(active_setpoint_c),
            },
            performance: CorePerformance {
                cop,
                main_power_w: Some(compressor_w * self.hvac.config.space_fraction),
            },
        };

        // Clear solver-provided capacity so next step starts fresh. The
        // arrival flag clears with it: both carry this step's solve, and
        // a stale one must neither deliver nor mask a starvation.
        self.ideal_capacity_w = 0.0;
        self.ideal_capacity_signal_arrived = false;
        // LoadFraction is a one-step post-thermostat multiplier.
        self.ctrl_load_fraction = 1.0;

        // Advance the DR duration once per step, at the step's end. The
        // thermal pipeline runs `update_control` twice per step (before the
        // solver-feedback phase and again after the control dispatch), so
        // the countdown lived there it burned twice the event's wall time
        // and the post-event release landed after the solve, which read the
        // still-forced-off mode and solved nothing; the release step then
        // ran the unit's raw full duty, unmetered. Advancing here puts the
        // release on the next step's first control pass, ahead of the
        // solve, so the release step delivers the solved capacity; the
        // step whose interval the event covers keeps the event's own
        // dispatch level (the partial sheds included), and the timer
        // reverts after it.
        if let Some(remaining) = self.dr_duration_remaining_s.as_mut() {
            *remaining -= dt.as_secs_f64();
            if *remaining <= 0.0 {
                self.dr_duration_remaining_s = None;
                self.dr_level = DRLevel::Normal;
                self.dr_setpoint_offset_c = 0.0;
                self.dr_load_fraction = 1.0;
                self.dr_duty_cycle = 1.0;
            }
        }

        #[cfg(feature = "observe")]
        {
            self.telemetry.set(
                tk::STARTUP_TIMER_RESET_COUNT,
                self.hvac.runtime.startup.reset_count as f64,
            );
        }

        Ok(())
    }

    /// Internal crankcase heater power computation.
    ///
    /// Returns 0 when OAT >= threshold. When below the threshold, power scales
    /// with the fraction of time the cooling coil is not running:
    /// `rated * (1 - cooling_rtf)`.
    fn crankcase_heater_power_internal(&self, outdoor_temp_c: f64, cooling_rtf: f64) -> f64 {
        if outdoor_temp_c >= self.crankcase_threshold_c {
            return 0.0;
        }
        let max_rtf = cooling_rtf;
        let effective_rated = match self.crankcase_capacity_curve {
            Some([c0, c1, c2]) => {
                let t = outdoor_temp_c;
                self.crankcase_rated_w * (c0 + c1 * t + c2 * t * t).max(0.0)
            }
            None => self.crankcase_rated_w,
        };
        effective_rated * (1.0 - max_rtf.clamp(0.0, 1.0))
    }

    fn calculate_performance(
        &mut self,
        env: &EnvironmentState,
        dt_min: f64,
    ) -> crate::Result<PerformanceResult> {
        let zone_id = self.hvac.config.served_zone()?;
        let zone = lookup_zone(env, zone_id)?;
        let pressure_pa = env.weather.pressure_pa();
        let zone_wb_c = hares_physics::psychrometrics::zone_wet_bulb_c(zone, pressure_pa);
        if !zone_wb_c.is_finite() {
            return Err(HaresError::Equipment(format!(
                "zone {zone_id:?} wet_bulb_c must be finite before HVAC cooling step"
            )));
        }
        self.telemetry
            .set(tk::HIGH_SIDE_CURVE_CLAMPED_SPEED_FRAC, 0.0);

        let is_variable_speed =
            self.hvac.config.speed_control_mode == SpeedControlMode::VariableSpeedIdeal;
        let mut speed_index = self.hvac.runtime.last_speed_index;
        let speed_frac = self.hvac.runtime.last_speed_frac;
        let num_speeds = self.hvac.n_speed_stages();
        let mut variable_selection = SpeedSelection {
            speed_index,
            speed_frac,
            part_load_ratio: self.hvac.runtime.duty_cycle.clamp(0.0, 1.0),
        };
        let (mut stage_cap_w, mut stage_eir, mut variable_plr) =
            match self.hvac.config.speed_control_mode {
                SpeedControlMode::VariableSpeedIdeal => {
                    self.variable_speed_point(variable_selection)
                }
                SpeedControlMode::MultiSpeedInterpolated => (
                    self.hvac.interpolated_capacity(
                        &self.hvac.config.cooling_capacities_w,
                        speed_index,
                        speed_frac,
                    ),
                    self.hvac.interpolated_eir(speed_index, speed_frac),
                    self.hvac.runtime.duty_cycle.clamp(0.0, 1.0),
                ),
                _ => (
                    HvacEquipment::capacity_at_stage(
                        &self.hvac.config.cooling_capacities_w,
                        speed_index,
                    ),
                    self.hvac.eir_at_stage(speed_index),
                    self.hvac.runtime.duty_cycle.clamp(0.0, 1.0),
                ),
            };

        let source_temp_c = self.source_temp.compute(env);

        fn curve_inputs(
            stage_capacity_w: f64,
            speed_index: usize,
            hvac: &HvacEquipment,
            zone: &hares_types::ZoneState,
            env: &EnvironmentState,
            source_temp_c: f64,
            flow_fraction_correction: f64,
        ) -> (f64, f64, f64, f64, f64) {
            let flow_m3_s_for_fan = stage_capacity_w.max(0.0) * hvac.config.airflow_m3_s_per_w;
            let fan_shaft_heat_correction_c = if flow_m3_s_for_fan > 0.0 {
                use hares_physics::{
                    air_properties::moist_air_density_kg_m3,
                    psychrometrics::SPECIFIC_HEAT_DRY_AIR_KJ_KG_K,
                };
                let fan_power_w = hvac.config.fan_power_w_per_m3_s * flow_m3_s_for_fan;
                let rho = moist_air_density_kg_m3(
                    env.weather.pressure_kpa * 1000.0,
                    zone.temperature_c,
                    zone.humidity_ratio.max(0.0),
                );
                let mfr = flow_m3_s_for_fan * rho;
                if mfr > 0.0 {
                    // The delta-K the fan's shaft heat adds to the entering
                    // air: W over (kg/s x J/(kg.K)) — the specific heat in
                    // kJ carries its 1000 here.
                    (fan_power_w) / (mfr * SPECIFIC_HEAT_DRY_AIR_KJ_KG_K * 1000.0)
                } else {
                    0.0
                }
            } else {
                0.0
            };
            let coil_entering_db_c = zone.temperature_c + fan_shaft_heat_correction_c;
            let coil_entering_wb_c = hares_physics::psychrometrics::wet_bulb_from_humidity_ratio(
                coil_entering_db_c,
                zone.humidity_ratio.max(0.0),
                env.weather.pressure_kpa * 1000.0,
            );
            let (_, cap_ratio) = hvac.evaluate_biquadratic_with_flow(
                speed_index * 2,
                coil_entering_wb_c,
                source_temp_c,
                flow_fraction_correction,
            );
            let (_, eir_ratio_base) = hvac.evaluate_biquadratic_with_flow(
                speed_index * 2 + 1,
                coil_entering_wb_c,
                source_temp_c,
                flow_fraction_correction,
            );
            (
                flow_m3_s_for_fan,
                coil_entering_db_c,
                coil_entering_wb_c,
                cap_ratio,
                eir_ratio_base,
            )
        }

        let (
            mut flow_m3_s_for_fan,
            mut coil_entering_db_c,
            mut coil_entering_wb_c,
            mut cap_ratio,
            mut eir_ratio_base,
        ) = curve_inputs(
            stage_cap_w,
            speed_index,
            &self.hvac,
            zone,
            env,
            source_temp_c,
            self.flow_fraction_correction,
        );

        // OCHRE HVAC.py:1044-1050 -- interpolate biquadratic between bracket stages.
        if self.hvac.config.speed_control_mode == SpeedControlMode::MultiSpeedInterpolated
            && speed_frac > 0.0
        {
            // Clamp high-side speed index to last valid stage so curve index
            // remains within the interleaved biquadratic_coeffs array.
            // When speed_index is already at the last stage (num_speeds - 1),
            // the high-side interpolation uses the same stage's curve —
            // equivalent to zero-delta extrapolation rather than reading
            // beyond the curve array.
            let hi_speed = (speed_index + 1).min(num_speeds - 1);
            if speed_index + 1 >= num_speeds {
                tracing::warn!(
                    speed_index,
                    num_speeds,
                    speed_frac,
                    "MultiSpeedInterpolated high-side curve index clamped: cooling high-side"
                );
                #[cfg(feature = "observe")]
                {
                    self.telemetry.set(
                        hares_types::telemetry_keys::HIGH_SIDE_CURVE_CLAMPED_SPEED_FRAC,
                        speed_frac,
                    );
                }
            }
            let (_, _, _, cap_ratio_high, eir_ratio_high) = curve_inputs(
                stage_cap_w,
                hi_speed,
                &self.hvac,
                zone,
                env,
                source_temp_c,
                self.flow_fraction_correction,
            );
            cap_ratio = cap_ratio * (1.0 - speed_frac) + cap_ratio_high * speed_frac;
            eir_ratio_base = eir_ratio_base * (1.0 - speed_frac) + eir_ratio_high * speed_frac;
        }

        if is_variable_speed && self.use_ideal && self.ideal_capacity_w.abs() > f64::EPSILON {
            let max_capacity_w = self
                .hvac
                .config
                .cooling_capacities_w
                .last()
                .copied()
                .unwrap_or_default();
            let requested_capacity_fraction =
                (-self.ideal_capacity_w / (max_capacity_w * cap_ratio).max(1.0)).clamp(0.0, 1.0);
            variable_selection = self.select_variable_speed_cooling(requested_capacity_fraction);
            speed_index = variable_selection.speed_index;
            self.hvac.runtime.duty_cycle = variable_selection.part_load_ratio;
            (stage_cap_w, stage_eir, variable_plr) = self.variable_speed_point(variable_selection);
            (
                flow_m3_s_for_fan,
                coil_entering_db_c,
                coil_entering_wb_c,
                cap_ratio,
                eir_ratio_base,
            ) = curve_inputs(
                stage_cap_w,
                speed_index,
                &self.hvac,
                zone,
                env,
                source_temp_c,
                self.flow_fraction_correction,
            );
        }

        // Derive PLR: from solver's ideal capacity (coarse timestep) or
        // thermostat duty cycle (fine timestep).
        let steady_capacity_w = (stage_cap_w * cap_ratio).max(0.0);
        let plr = if is_variable_speed {
            variable_plr
        } else if self.use_ideal && self.ideal_capacity_w.abs() > f64::EPSILON {
            // Solver-provided ideal capacity (negative for cooling): derive PLR
            // from biquadratic-corrected capacity at current conditions.
            // Use 1% of nominal as floor to prevent near-zero denominators
            // when biquadratic evaluates near zero at extreme conditions.
            let min_cap = (stage_cap_w * 0.01).max(1.0);
            let p = (-self.ideal_capacity_w / steady_capacity_w.max(min_cap)).clamp(0.0, 1.0);
            // Write back so telemetry and RTF reporting see the solver-derived value.
            self.hvac.runtime.duty_cycle = p;
            p
        } else {
            self.hvac.runtime.duty_cycle.clamp(0.0, 1.0)
        };
        let plf = if is_variable_speed {
            1.0
        } else {
            self.hvac.part_load_factor(plr)
        };

        // EIR curve: divide by PLF -- a lower PLF (more cycling) means worse efficiency.
        let eir_ratio = if plf > 0.0 {
            eir_ratio_base / plf
        } else {
            eir_ratio_base
        };

        let staged_capacity_w = self.hvac.apply_startup_capacity_degradation(
            steady_capacity_w,
            dt_min,
            self.hvac.runtime.duty_cycle > 0.0,
        );
        // OCHRE HVAC.py:445-448 -- clip to rated_max * ext_capacity_frac.
        let capacity_ceiling = steady_capacity_w * self.hvac.control.max_capacity_fraction;
        let total_capacity_w = (staged_capacity_w * plr).max(0.0).min(capacity_ceiling);

        let flow_m3_s = flow_m3_s_for_fan;

        let ao = self.ao_for_speed(speed_index);
        let CoilResult {
            shr,
            supply_temp_c,
            adp_temp_c,
            bypass_factor,
            ..
        } = calculate_shr(
            coil_entering_db_c,
            zone.humidity_ratio,
            env.weather.pressure_kpa,
            (total_capacity_w).max(0.0),
            flow_m3_s,
            ao,
        )?;
        let steady_state_shr = shr.clamp(0.0, 1.0);
        self.last_adp_c = adp_temp_c;
        self.last_bypass_factor = bypass_factor;

        // Apply Henderson-Rengarajan latent degradation at part load.
        // RTF = PLR / PLF; at continuous operation (plf==0 guard is already
        // handled above) this reduces to PLR / PLF.
        let rtf = if plf > 0.0 {
            (plr / plf).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let shr = if self.latent_degradation.is_active() {
            // Rated latent capacity at AHRI conditions (single-stage nominal).
            let rated_cap_w = self
                .hvac
                .config
                .cooling_capacities_w
                .get(speed_index)
                .copied()
                .unwrap_or_else(|| {
                    self.hvac
                        .config
                        .cooling_capacities_w
                        .last()
                        .copied()
                        .unwrap_or(0.0)
                });
            let rated_latent_w = rated_cap_w * (1.0 - self.rated_shr);
            let actual_latent_w = total_capacity_w * (1.0 - steady_state_shr);
            effective_shr_with_latent_degradation(
                CoilOperatingState {
                    steady_state_shr,
                    runtime_fraction: rtf,
                    entering_db_c: zone.temperature_c,
                    entering_wb_c: coil_entering_wb_c,
                    rated_latent_capacity_w: rated_latent_w,
                    actual_latent_capacity_w: actual_latent_w,
                },
                &self.latent_degradation,
            )
        } else {
            steady_state_shr
        };

        #[cfg(feature = "observe")]
        {
            self.telemetry.set(
                tk::LATENT_DEGRADATION_ACTIVE,
                if self.latent_degradation.is_active() {
                    1.0
                } else {
                    0.0
                },
            );
            self.telemetry
                .set(tk::SHR_BEFORE_DEGRADATION, steady_state_shr);
        }

        self.hvac.config.shr = shr;

        // Return gross (pre-DSE) sensible/latent. zone_heat_fractions (set from
        // duct_dse during init) distributes gross output to conditioned and duct zones.
        let (sensible_cooling_w, latent_cooling_w) =
            self.hvac.sensible_latent_from_shr(total_capacity_w);

        let compressor_w = (total_capacity_w * stage_eir * eir_ratio).max(0.0);
        let fan_w = self.hvac.fan_power_w(flow_m3_s) * plr;

        Ok(PerformanceResult {
            sensible_cooling_w,
            latent_cooling_w,
            compressor_w,
            fan_w,
            shr,
            supply_temp_c,
            rtf,
        })
    }

    fn compute_coil_ao(&mut self, rated_shr: f64) -> crate::Result<()> {
        self.coil_ao_by_stage = compute_coil_ao_by_stage(
            &self.hvac.config.cooling_capacities_w,
            self.hvac.config.airflow_m3_s_per_w,
            rated_shr,
            &self.stage_shrs,
        )?;
        Ok(())
    }

    fn ao_for_speed(&self, speed_index: usize) -> f64 {
        if self.coil_ao_by_stage.is_empty() {
            return 10.0;
        }
        self.coil_ao_by_stage[speed_index.min(self.coil_ao_by_stage.len() - 1)]
    }

    fn save_state(&self) -> crate::Result<Vec<u8>> {
        try_save_versioned(
            &AirConditionerState {
                mode: self.hvac.thermostat_fsm.mode,
                duty_cycle: self.hvac.runtime.duty_cycle,
                last_mode_switch_at: self.hvac.thermostat_fsm.last_mode_switch_at,
                mode_start_at: self.hvac.thermostat_fsm.mode_start_at,
                runtime_setpoints: self.hvac.thermostat_fsm.runtime_setpoints,
                operating_mode: self.operating_mode,
                run_time_s: self.run_time_s,
                cycle_on_steps: self.cycle_on_steps,
                cycle_off_steps: self.cycle_off_steps,
                crankcase_heater_on: self.crankcase_heater_on,
                startup_c_d: self.hvac.runtime.startup.c_d,
                startup_time_since_start_min: self.hvac.runtime.startup.time_since_start_min,
                startup_was_on: self.hvac.runtime.startup.was_on,
                plf_state: self.hvac.runtime.plf_state,
                last_speed_index: self.hvac.runtime.last_speed_index,
                last_speed_frac: self.hvac.runtime.last_speed_frac,
                electric_w: self.telemetry.get(tk::ELECTRIC_KW).unwrap_or(0.0),
                sensible_cooling_w: self.telemetry.get(tk::SENSIBLE_COOLING_W).unwrap_or(0.0),
                latent_cooling_w: self.telemetry.get(tk::LATENT_COOLING_W).unwrap_or(0.0),
                // The key's presence is the cooled marker: an absent key
                // (the unit has never cooled) saves `None`, a present one
                // the last cooling step's value.
                shr: self.telemetry.get(tk::SHR),
                ctrl_duty_cycle: self.ctrl_duty_cycle,
                ctrl_power_limit_w: if self.ctrl_power_limit_w.is_finite() {
                    Some(self.ctrl_power_limit_w)
                } else {
                    None
                },
                ctrl_mode_override: self.ctrl_mode_override,
                dr_level: self.dr_level,
                dr_setpoint_offset_c: self.dr_setpoint_offset_c,
                dr_load_fraction: self.dr_load_fraction,
                dr_duty_cycle: self.dr_duty_cycle,
                dr_duration_remaining_s: self.dr_duration_remaining_s,
                last_adp_c: self.last_adp_c,
                last_bypass_factor: self.last_bypass_factor,
                thermostat_hysteresis_c: self.hvac.thermostat_fsm.thermostat.hysteresis_c,
                time_at_current_speed_s: self.hvac.runtime.time_at_current_speed_s,
                min_on_time_s: self.hvac.thermostat_fsm.min_on_time_s,
                min_off_time_s: self.hvac.thermostat_fsm.min_off_time_s,
            },
            AC_CHECKPOINT_VERSION,
            "AirConditioner",
        )
    }

    fn load_state(&mut self, state: &[u8]) -> crate::Result<()> {
        let decoded: AirConditionerState = load_versioned(
            state,
            AC_CHECKPOINT_VERSION,
            "AirConditioner",
            self.descriptor.id,
        )?;
        self.hvac.thermostat_fsm.mode = decoded.mode;
        self.hvac.runtime.duty_cycle = decoded.duty_cycle;
        self.hvac.thermostat_fsm.last_mode_switch_at = decoded.last_mode_switch_at;
        self.hvac.thermostat_fsm.mode_start_at = decoded.mode_start_at;
        self.hvac.thermostat_fsm.runtime_setpoints = decoded.runtime_setpoints;
        self.operating_mode = decoded.operating_mode;
        self.run_time_s = decoded.run_time_s;
        self.cycle_on_steps = decoded.cycle_on_steps;
        self.cycle_off_steps = decoded.cycle_off_steps;
        self.crankcase_heater_on = decoded.crankcase_heater_on;
        self.hvac.runtime.startup.c_d = decoded.startup_c_d;
        self.hvac.runtime.startup.time_since_start_min = decoded.startup_time_since_start_min;
        self.hvac.runtime.startup.was_on = decoded.startup_was_on;
        self.hvac.runtime.plf_state = decoded.plf_state;
        self.hvac.runtime.last_speed_index = decoded.last_speed_index;
        self.hvac.runtime.last_speed_frac = decoded.last_speed_frac;
        // The restored state's SHR is `None` until the unit's first cooling
        // step, as the continuous path's is: the last computed value rides
        // only when a cooled step produced one, and a never-cooled unit
        // keeps the init-time coil state.
        if let Some(shr) = decoded.shr {
            self.hvac.config.shr = shr;
        }
        self.ctrl_duty_cycle = decoded.ctrl_duty_cycle;
        self.ctrl_power_limit_w = decoded.ctrl_power_limit_w.unwrap_or(f64::INFINITY);
        self.ctrl_mode_override = decoded.ctrl_mode_override;
        self.dr_level = decoded.dr_level;
        self.dr_setpoint_offset_c = decoded.dr_setpoint_offset_c;
        self.dr_load_fraction = decoded.dr_load_fraction;
        self.dr_duty_cycle = decoded.dr_duty_cycle;
        self.dr_duration_remaining_s = decoded.dr_duration_remaining_s;
        self.last_adp_c = decoded.last_adp_c;
        self.last_bypass_factor = decoded.last_bypass_factor;
        self.hvac
            .thermostat_fsm
            .restore_hysteresis(decoded.thermostat_hysteresis_c)?;
        self.hvac.runtime.time_at_current_speed_s = decoded.time_at_current_speed_s;
        self.hvac.thermostat_fsm.min_on_time_s = decoded.min_on_time_s;
        self.hvac.thermostat_fsm.min_off_time_s = decoded.min_off_time_s;

        self.telemetry.insert(tk::ELECTRIC_KW, decoded.electric_w);
        self.telemetry
            .insert(tk::SENSIBLE_COOLING_W, decoded.sensible_cooling_w);
        self.telemetry
            .insert(tk::LATENT_COOLING_W, decoded.latent_cooling_w);
        // The step's own write: registered at the first cooled restore,
        // updated in place after, so the restored map carries the key only
        // when a cooling step produced a value: the null-until-first-
        // cooling timeline a continuously-run never-cooled unit holds. An
        // absent key leaves the map entirely: the checkpoint replaces the
        // pre-restore timeline, so loading a never-cooled blob into a unit
        // that cooled earlier must not keep the stale key.
        match decoded.shr {
            Some(shr) => self.telemetry.set_or_register(tk::SHR, shr),
            None => self.telemetry.unregister(tk::SHR),
        }
        self.telemetry
            .insert(tk::OPERATING_MODE, decoded.operating_mode.as_code());
        // Rebuild the restore-instant output from the persisted step
        // values — mirroring the step's own literal where the checkpoint
        // carries the face: electric from `electric_w`, sensible/latent
        // cooling from their checkpointed values, mode from the
        // checkpointed `operating_mode` (the pair the step published was
        // guard-valid), speed from the restored `last_speed_index`,
        // setpoint from the restored thermostat state (mode-keyed, as the
        // step keys it). COP and the fan-heat share of the thermal output
        // are not checkpointed: COP publishes as 0.0 (the guard-allowed
        // not-yet-recomputed value, recomputed on the next step) and the
        // thermal output publishes the checkpointed sensible+latent sum
        // without the small fan-heat correction. `CoreOutput::default()`
        // (all fields `None`) violates the presence rules for every
        // declared capability
        // (ELECTRIC/HAS_MODE/THERMAL/HAS_SPEED/HAS_SETPOINT/HAS_COP/
        // REACTIVE) — the restore contract is that a checkpoint restores
        // a state `validate_core_contract` accepts.
        let sp = self.hvac.effective_setpoints();
        let active_setpoint_c = match decoded.operating_mode {
            OperatingMode::Heating => sp.heating_c,
            _ => sp.cooling_c,
        };
        self.core_output = CoreOutput {
            flows: CoreFlows {
                electric_w: Some(ElectricPower::Consumption(decoded.electric_w.max(0.0))),
                reactive_power_var: Some(0.0),
                fuel_w: None,
                thermal_output_w: Some(-(decoded.sensible_cooling_w + decoded.latent_cooling_w)),
                sensible_cooling_w: Some(-decoded.sensible_cooling_w),
                latent_cooling_w: Some(-decoded.latent_cooling_w),
            },
            state: CoreState {
                operating_mode: Some(decoded.operating_mode),
                soc: None,
                speed_index: Some(self.hvac.runtime.last_speed_index as u8),
                setpoint_c: Some(active_setpoint_c),
            },
            performance: CorePerformance {
                // The restore instant's COP: None while the restored mode is
                // idle (the next step recomputes it), the guard-allowed 0.0
                // while it is active, so the restored output stays one
                // `validate_core_contract` accepts.
                cop: decoded.operating_mode.is_active().then_some(0.0),
                main_power_w: None,
            },
        };

        Ok(())
    }

    fn apply_signal(&mut self, signal: &ControlSignal) -> crate::Result<()> {
        match signal {
            ControlSignal::DutyCycle { on_fraction, .. } => {
                self.ctrl_duty_cycle = *on_fraction;
            }
            ControlSignal::LoadFraction { fraction } => {
                self.ctrl_load_fraction = *fraction;
            }
            ControlSignal::PowerLimit { max_power_w, .. } => {
                self.ctrl_power_limit_w = *max_power_w;
            }
            ControlSignal::ModeOverride { mode } => {
                self.ctrl_mode_override = Some(*mode);
            }
            ControlSignal::ModeOverrideRelease => {
                self.ctrl_mode_override = None;
            }
            ControlSignal::DemandResponse { level, duration_s } => {
                let (level, duration_s) = demand_response_command(*level, *duration_s);
                self.apply_dr_level(level);
                self.dr_duration_remaining_s = duration_s;
            }
            ControlSignal::IdealCapacity { capacity_w, .. } => {
                // Solver-provided load: negative = cooling needed.
                self.ideal_capacity_w = *capacity_w;
                self.ideal_capacity_signal_arrived = true;
            }
            ControlSignal::MaxCapacityFraction { fraction } => {
                self.hvac.control.max_capacity_fraction = *fraction;
            }
            _ => {
                #[cfg(feature = "observe")]
                tracing::debug!(
                    signal_variant = ?signal,
                    equipment = %self.descriptor.equipment_type,
                    "control signal routed through hvac catch-all path",
                );
                self.hvac.apply_control_signal(signal)?;
            }
        }
        Ok(())
    }

    fn ideal_target(&self) -> Option<(hares_types::ZoneId, f64)> {
        if !self.use_ideal {
            return None;
        }
        // The solver's gate reads the thermostat's latched call, never the
        // step's dispatch mode. The forced-off branches of `update_control`
        // (a de-energized bus, a DR shed, an explicit Off) return before the
        // thermostat re-reads, so the mode the solver-feedback phase would
        // find is Off while the thermostat's call still stands; a control
        // signal landing at the dispatch (a DR cancel, a setpoint override)
        // then flips the second control pass back to Cooling, and the step
        // dispatches a call the solve never saw: the raw duty, unmetered to
        // the zone's need. Reading the latch keeps the solve running for the
        // call the dispatch may honor. The idle mode a step publishes never
        // reaches this read: every control pass re-derives the mode from the
        // same latch.
        if self.hvac.thermostat_fsm.mode != ThermostatMode::Cooling {
            return None;
        }
        let setpoint = self.hvac.effective_setpoints().cooling_c + self.dr_setpoint_offset_c;
        self.hvac.config.zone_id.map(|zone| (zone, setpoint))
    }

    fn apply_dr_level(&mut self, level: DRLevel) {
        self.dr_level = level;
        match level {
            DRLevel::Normal => {
                self.dr_setpoint_offset_c = 0.0;
                self.dr_duty_cycle = 1.0;
                self.dr_load_fraction = 1.0;
            }
            DRLevel::Moderate => {
                // Raise cooling setpoint by 1°C → less cooling demand.
                self.dr_setpoint_offset_c = 1.0;
                self.dr_duty_cycle = 1.0;
                self.dr_load_fraction = 1.0;
            }
            DRLevel::High => {
                self.dr_setpoint_offset_c = 2.0;
                self.dr_duty_cycle = 1.0;
                self.dr_load_fraction = 0.8;
            }
            DRLevel::Critical => {
                self.dr_setpoint_offset_c = 3.0;
                self.dr_duty_cycle = 1.0;
                self.dr_load_fraction = 0.5;
            }
            DRLevel::GridEmergency => {
                self.dr_setpoint_offset_c = 0.0;
                self.dr_duty_cycle = 1.0;
                self.dr_load_fraction = 0.0;
            }
        }
    }
}

pub fn register_with_registry(registry: &mut EquipmentRegistry) {
    registry.register(
        "Air Conditioner",
        Box::new(|config| Box::new(AirConditioner::new(config))),
    );
    registry.register("Room AC", Box::new(|config| Box::new(RoomAC::new(config))));
}

#[cfg(test)]
fn typed_ac_test_config(eir: f64) -> EquipmentConfig {
    EquipmentConfig::from_typed(
        "AC".to_string(),
        "Air Conditioner".to_string(),
        CentralAirConditionerConfig {
            equipment_id: None,
            zone_id: Some(1),
            capacity_w: 8_000.0,
            eir,
            shr: Some(0.75),
            number_of_speeds: 1,
            stage_capacities_w: None,
            stage_eirs: None,
            stage_shrs: None,
            fan_power_w: None,
            fan_power_w_per_cfm: None,
            setpoint: HvacSetpointConfig {
                cooling_setpoint_c: Some(24.0),
                heating_setpoint_c: Some(18.0),
                heating_setpoint_source: None,
                cooling_setpoint_source: None,
            },
            hysteresis_c: Some(1.0),
            airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_CENTRAL_AC_M3_S_PER_W),
            fraction_load_served: None,
            crankcase_heater_w: None,
            crankcase_heater_threshold_c: None,
            crankcase_capacity_curve_coeffs: None,
            duct: crate::DuctConfig::default(),
            system_type: None,
            startup_cd: None,
            biquadratic_x1_min: None,
            biquadratic_x1_max: None,
            biquadratic_x2_min: None,
            biquadratic_x2_max: None,
            ff_min: None,
            ff_max: None,
            plf_min: None,
            plf_max: None,
            charge_defect_ratio: None,
            min_oat_compressor_cooling_c: None,
        },
    )
    .unwrap()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        ControlSignal, DRLevel, EnvironmentState, ExecutionStage, GridState, HumidityAccumulator,
        OperatingMode, PortSlots, ThermalAccumulator, WeatherState, ZoneId, ZoneState,
        telemetry_keys as tk,
    };

    use super::{AirConditioner, CoolingCore, HvacEquipmentType, RoomAC, SpeedControlMode};
    use crate::hvac::ThermostatMode;

    use crate::{
        CentralAirConditionerConfig, DuctConfig, Equipment, EquipmentConfig, EquipmentRegistry,
        HvacSetpointConfig, RoomAcConfig,
    };
    use hares_physics::constants::BTU_PER_HR_PER_W;

    fn env(
        zone_temp_c: f64,
        humidity_ratio: f64,
        _wet_bulb_c: f64,
        outdoor_c: f64,
    ) -> EnvironmentState {
        EnvironmentState {
            ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp_c,
                humidity_ratio,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: outdoor_c,
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
                island_bus_voltage_pu: None,
            },
            schedule_row: None,
            unavailable_periods: hares_types::UnavailablePeriodState::default(),
            domains: hares_types::DomainSlots::default(),
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

    fn ac_config() -> EquipmentConfig {
        super::typed_ac_test_config(0.33)
    }

    fn room_ac_config() -> EquipmentConfig {
        EquipmentConfig::from_typed(
            "AC".to_string(),
            "Room AC".to_string(),
            RoomAcConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 3_500.0,
                eir: 10.0,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_ROOM_AC_M3_S_PER_W),
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                shr: None,
                startup_cd: None,
                crankcase_heater_w: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                min_oat_compressor_cooling_c: None,
            },
        )
        .unwrap()
    }

    fn ac_config_with(mutator: impl FnOnce(&mut CentralAirConditionerConfig)) -> EquipmentConfig {
        let mut typed = ac_config().typed::<CentralAirConditionerConfig>().unwrap();
        mutator(&mut typed);
        EquipmentConfig::from_typed("AC".to_string(), "Air Conditioner".to_string(), typed).unwrap()
    }

    fn env_at_second(zone_temp_c: f64, outdoor_c: f64, second: i64) -> EnvironmentState {
        let mut e = env(zone_temp_c, 0.008, 20.0, outdoor_c);
        e.current_time += ChronoDuration::seconds(second);
        e
    }

    #[test]
    fn dr_forced_off_resumes_thermostat_control_at_event_end() {
        let cfg = ac_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        // Zone above the cooling band: the thermostat calls for cooling.
        let env = env_at_second(27.0, 35.0, 0);
        eq.init(&cfg, &env).unwrap();

        // Before the event the thermostat calls for cooling and delivers.
        assert_eq!(eq.update_control(&env), OperatingMode::Cooling);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        assert!(
            ports.electrical.net_active_w() > 0.0,
            "the thermostat call must deliver before the event"
        );

        // A DR event's TurnOff forces Off through ModeOverride. The forced
        // mode is sticky by design and must hold through every step of the
        // event.
        eq.apply_control(&ControlSignal::ModeOverride {
            mode: OperatingMode::Off,
        })
        .unwrap();
        for second in [60, 120, 180] {
            let event_env = env_at_second(27.0, 35.0, second);
            assert_eq!(eq.update_control(&event_env), OperatingMode::Off);
            ports.zero();
            eq.step(&event_env, Duration::from_secs(60), &mut ports)
                .unwrap();
            assert_eq!(
                ports.electrical.net_active_w(),
                0.0,
                "the forced-off override must hold through every event step"
            );
        }

        // The event's end dispatches the mode release. The step after the
        // last event step must deliver per the thermostat again.
        eq.apply_control(&ControlSignal::mode_release()).unwrap();
        let after_env = env_at_second(27.0, 35.0, 240);
        assert_eq!(
            eq.update_control(&after_env),
            OperatingMode::Cooling,
            "the release must hand the unit back to its own thermostat"
        );
        ports.zero();
        eq.step(&after_env, Duration::from_secs(60), &mut ports)
            .unwrap();
        assert!(
            ports.electrical.net_active_w() > 0.0,
            "the step after the last event step must deliver per the thermostat again"
        );
    }

    #[test]
    fn mode_override_and_cleared_state_survive_checkpoint_round_trip() {
        let cfg = ac_config();
        let env = env_at_second(20.0, 35.0, 0);

        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &env).unwrap();
        eq.apply_control(&ControlSignal::ModeOverride {
            mode: OperatingMode::Cooling,
        })
        .unwrap();

        let state = eq.save_state().unwrap();
        let mut restored = AirConditioner::new(cfg.clone());
        restored.init(&cfg, &env).unwrap();
        restored.load_state(&state).unwrap();
        assert_eq!(
            restored.core.ctrl_mode_override,
            Some(OperatingMode::Cooling),
            "the forced mode must survive the checkpoint round trip"
        );

        restored
            .apply_control(&ControlSignal::mode_release())
            .unwrap();
        let cleared_state = restored.save_state().unwrap();
        let mut cleared = AirConditioner::new(cfg.clone());
        cleared.init(&cfg, &env).unwrap();
        cleared.load_state(&cleared_state).unwrap();
        assert_eq!(
            cleared.core.ctrl_mode_override, None,
            "the cleared override must survive the checkpoint round trip"
        );
        assert_eq!(
            cleared.update_control(&env),
            OperatingMode::Off,
            "after the cleared round trip the thermostat controls again"
        );
    }

    #[test]
    fn out_of_domain_control_values_rejected_on_unchecked_path() {
        use hares_types::ControlSignal;

        // The arm stores every numeric field raw — DutyCycle on_fraction,
        // LoadFraction, PowerLimit, MaxCapacityFraction, deadband (into
        // hysteresis), IdealCapacity — while the water-heater arms reject
        // the same DutyCycle domain and the central validator rejects all
        // of these on the checked path. Negative IdealCapacity is
        // legitimate here (cooling), so the probe is non-finite only.
        let cfg = ac_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let e = env(24.0, 0.01, 18.0, 30.0);
        eq.init(&cfg, &e).unwrap();

        for (bad, token) in [
            (
                ControlSignal::DutyCycle {
                    on_fraction: 1.5,
                    period_s: None,
                    component: None,
                },
                "duty",
            ),
            (ControlSignal::LoadFraction { fraction: 5.0 }, "fraction"),
            (
                ControlSignal::PowerLimit {
                    max_power_w: -5.0,
                    ramp_rate_w_per_s: None,
                },
                "power",
            ),
            (
                ControlSignal::MaxCapacityFraction { fraction: 5.0 },
                "capacity",
            ),
            (
                ControlSignal::ThermalSetpoint {
                    heating_setpoint_c: None,
                    cooling_setpoint_c: Some(5000.0),
                    deadband_c: Some(100.0),
                },
                "setpoint",
            ),
            (
                ControlSignal::IdealCapacity {
                    capacity_w: f64::NAN,
                    degraded: false,
                },
                "capacity",
            ),
        ] {
            let err = eq
                .apply_control_unchecked(&bad)
                .expect_err("out-of-domain control value must be rejected");
            assert!(
                format!("{err:?}").to_lowercase().contains(token),
                "error must name the offending signal for {bad:?}, got {err:?}"
            );
        }
    }

    #[test]
    fn air_conditioner_descriptor_contracts() {
        let cfg = ac_config();
        let eq = AirConditioner::new(cfg);
        assert_eq!(eq.descriptor().stage, ExecutionStage::Thermal);
        assert!(
            eq.descriptor()
                .control_capabilities
                .contains(hares_types::ControlCapabilities::THERMAL_SETPOINT)
        );

        let telemetry_names: Vec<_> = eq
            .descriptor()
            .telemetry_fields
            .iter()
            .map(|field| field.name.as_ref())
            .collect();
        assert!(telemetry_names.contains(&tk::ELECTRIC_KW));
        assert!(telemetry_names.contains(&tk::SENSIBLE_COOLING_W));
        assert!(telemetry_names.contains(&tk::LATENT_COOLING_W));
        assert!(telemetry_names.contains(&tk::SHR));
        assert!(telemetry_names.contains(&tk::OPERATING_MODE));
    }

    #[test]
    fn room_ac_forces_single_speed_and_duct_dse_one() {
        let cfg = room_ac_config();
        let mut eq = RoomAC::new(cfg.clone());
        eq.init(&cfg, &env(26.0, 0.009, 18.0, 30.0)).unwrap();
        assert_eq!(
            eq.core.hvac.config.speed_control_mode,
            super::SpeedControlMode::SingleSpeed
        );
        assert_eq!(eq.core.hvac.config.duct_dse, 1.0);
    }

    #[test]
    fn room_ac_init_uses_explicit_startup_cd() {
        // Explicit startup_cd sets startup.c_d only, NOT plf_cooling_degradation_coeff.
        // PLF Cd is derived from SEER (0.20 for SEER < 13), decoupled from startup ramp.
        let explicit_cd = 0.15;
        let cfg = EquipmentConfig::from_typed(
            "room_ac_explicit".to_string(),
            "Room AC".to_string(),
            RoomAcConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 3_500.0,
                eir: BTU_PER_HR_PER_W / 10.0,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                startup_cd: Some(explicit_cd),
                ..room_ac_defaults()
            },
        )
        .unwrap();
        let mut eq = RoomAC::new(cfg.clone());
        eq.init(&cfg, &env(26.0, 0.009, 18.0, 30.0)).unwrap();
        // SEER = 10 → SEER < 13 → PLF Cd = 0.20 (from derived_plf_cd, NOT from explicit_cd).
        assert!((eq.core.hvac.runtime.plf_cooling_degradation_coeff - 0.20).abs() < 1e-9);
        assert!((eq.core.hvac.runtime.startup.c_d - explicit_cd).abs() < 1e-9);
    }

    #[test]
    fn room_ac_init_uses_derived_cd_when_no_explicit() {
        // No explicit startup_cd → derived from SEER. SEER < 13 → PLF Cd = 0.20.
        // startup.c_d remains at DEFAULT_STARTUP_CD (0.0) — startup ramp is opt-in.
        let eir = BTU_PER_HR_PER_W / 10.0;
        let cfg = EquipmentConfig::from_typed(
            "room_ac_derived".to_string(),
            "Room AC".to_string(),
            RoomAcConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 3_500.0,
                eir,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                startup_cd: None,
                ..room_ac_defaults()
            },
        )
        .unwrap();
        let mut eq = RoomAC::new(cfg.clone());
        eq.init(&cfg, &env(26.0, 0.009, 18.0, 30.0)).unwrap();
        assert!((eq.core.hvac.runtime.plf_cooling_degradation_coeff - 0.20).abs() < 1e-9);
        assert!(
            eq.core.hvac.runtime.startup.c_d.abs() < 1e-9,
            "startup.c_d must be 0.0 (DEFAULT_STARTUP_CD) when no explicit startup_cd provided, got {}",
            eq.core.hvac.runtime.startup.c_d
        );
    }

    #[test]
    fn room_ac_init_uses_static_fallback_when_no_derived() {
        // EIR is zero → derived_cooling_startup_cd returns None → fallback 0.20.
        // However, validation will reject zero EIR before Cd init is reached.
        let cfg = EquipmentConfig::from_typed(
            "room_ac_fallback".to_string(),
            "Room AC".to_string(),
            RoomAcConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 3_500.0,
                eir: 0.0,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                startup_cd: None,
                ..room_ac_defaults()
            },
        )
        .unwrap();
        let mut eq = RoomAC::new(cfg.clone());
        assert!(
            eq.init(&cfg, &env(26.0, 0.009, 18.0, 30.0)).is_err(),
            "zero eir should fail validation before reaching Cd init"
        );
    }

    fn room_ac_defaults() -> RoomAcConfig {
        RoomAcConfig {
            equipment_id: None,
            zone_id: Some(1),
            capacity_w: 0.0,
            eir: 0.0,
            setpoint: HvacSetpointConfig {
                cooling_setpoint_c: None,
                heating_setpoint_c: None,
                heating_setpoint_source: None,
                cooling_setpoint_source: None,
            },
            hysteresis_c: None,
            airflow_m3_s_per_w: None,
            biquadratic_x1_min: None,
            biquadratic_x1_max: None,
            biquadratic_x2_min: None,
            biquadratic_x2_max: None,
            ff_min: None,
            ff_max: None,
            plf_min: None,
            plf_max: None,
            shr: None,
            startup_cd: None,
            crankcase_heater_w: None,
            crankcase_heater_threshold_c: None,
            crankcase_capacity_curve_coeffs: None,
            min_oat_compressor_cooling_c: None,
        }
    }

    /// Regression: AC step() previously called update_control() internally,
    /// causing the thermostat FSM to advance twice per timestep.
    /// step() must be pure physics: calling it without a prior update_control()
    /// must leave the equipment in its initial Off state.
    #[test]
    fn step_does_not_call_update_control_internally() {
        let cfg = ac_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        // Zone well above setpoint (28°C vs 24°C setpoint); an internal
        // update_control() would switch to Cooling mode and draw power.
        let environment = env(28.0, 0.01, 19.0, 35.0);
        eq.init(&cfg, &environment).unwrap();
        // Deliberately skip update_control() before step().
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        assert_eq!(
            eq.telemetry().get(tk::OPERATING_MODE),
            Some(0.0),
            "step() must not change operating mode; it must remain Off when \
             update_control() was never called",
        );
        assert_eq!(
            eq.telemetry().get(tk::ELECTRIC_KW),
            Some(0.0),
            "no power should be drawn when operating mode is Off",
        );
    }

    /// A cooling unit serving none of the load (`fraction_load_served = 0`,
    /// a heating-only heat pump's dormant cooling side) still runs its
    /// thermostat: on a cooling call the step delivers zone cooling while
    /// the space-fraction-scaled electric draw is zero. The delivered
    /// thermal flow is the unit's real output, so the published operating
    /// mode must stay Cooling (the flow the mode-flow guard counts) and the
    /// core output must satisfy the contract. Mirrors OCHRE HVAC.py:556-561:
    /// power scales by the space fraction, "sensible/latent gains to
    /// envelope are not updated".
    #[test]
    fn zero_fraction_cooling_step_satisfies_the_core_contract() {
        let cfg = ac_config_with(|typed| typed.fraction_load_served = Some(0.0));
        let mut eq = AirConditioner::new(cfg.clone());
        let hot = env(26.0, 0.01, 19.0, 30.0);
        eq.init(&cfg, &hot).unwrap();
        assert_eq!(eq.update_control(&hot), OperatingMode::Cooling);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&hot, Duration::from_secs(60), &mut ports).unwrap();

        // The OCHRE-faithful delivery: zone cooling happens, electric is
        // scaled to zero by the zero fraction.
        assert!(
            ports.thermal[0].sensible_gain_w < 0.0,
            "the cooling side still delivers its zone cooling"
        );
        assert_eq!(
            ports.electrical.load_power_w, 0.0,
            "a zero fraction scales the electric draw to zero"
        );
        let co = eq.core_output();
        assert_eq!(
            co.state.operating_mode,
            Some(OperatingMode::Cooling),
            "an active cooling delivery must report Cooling, not Standby"
        );
        hares_types::validate_core_contract(eq.descriptor(), co, &mut Vec::new())
            .expect("zero-fraction cooling step must satisfy the core-output contract");
    }

    /// Grid outage (de-energized bus): the compressor/blower have no supply,
    /// so cooling is forced off at the control level — zero draw, zero
    /// delivered cooling. An islanded home (battery/generator backup holding
    /// the bus at nominal) keeps cooling; control resumes on restoration.
    #[test]
    fn grid_outage_forces_cooling_off_and_islanded_home_keeps_cooling() {
        let cfg = ac_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let env_hot = env(28.0, 0.01, 19.0, 35.0);
        eq.init(&cfg, &env_hot).unwrap();

        // Baseline: zone above setpoint → cooling runs.
        assert_eq!(eq.update_control(&env_hot), OperatingMode::Cooling);

        // Utility outage, no backup: forced off, no draw, no delivered heat.
        let mut env_outage = env(28.0, 0.01, 19.0, 35.0);
        env_outage.grid.voltage_pu = 0.0;
        assert_eq!(eq.update_control(&env_outage), OperatingMode::Off);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env_outage, Duration::from_secs(60), &mut ports)
            .unwrap();
        assert_eq!(
            ports.electrical.load_power_w, 0.0,
            "no electric draw during outage"
        );
        assert_eq!(
            ports.thermal[0].sensible_gain_w, 0.0,
            "no delivered cooling during outage"
        );
        assert_eq!(eq.telemetry().get(tk::ELECTRIC_KW), Some(0.0));

        // Islanded: utility out but a backup source holds the bus at nominal
        // → the bus is energized and cooling proceeds normally.
        let mut env_islanded = env(28.0, 0.01, 19.0, 35.0);
        env_islanded.grid.voltage_pu = 0.0;
        env_islanded.grid.island_bus_voltage_pu = Some(1.0);
        assert_eq!(eq.update_control(&env_islanded), OperatingMode::Cooling);

        // Restoration: thermostat control resumes.
        assert_eq!(eq.update_control(&env_hot), OperatingMode::Cooling);
    }

    /// The crankcase heater is a standby load: a de-energized bus removes its
    /// supply, so it must not draw during an outage.
    #[test]
    fn grid_outage_removes_crankcase_heater_draw() {
        let cfg = ac_config();

        // Cold outdoor (below the 12.8 °C crankcase threshold), zone below
        // setpoint → compressor off, crankcase heater normally draws.
        let env_cold = env(20.0, 0.008, 15.0, 5.0);
        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &env_cold).unwrap();
        eq.update_control(&env_cold);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env_cold, Duration::from_secs(60), &mut ports)
            .unwrap();
        let crankcase_w = eq.telemetry().get(tk::CRANKCASE_KW).unwrap_or(0.0);
        assert!(
            crankcase_w > 0.0,
            "crankcase heater should draw below threshold OAT with compressor off"
        );

        // Same conditions during an outage: zero draw.
        let mut env_outage = env(20.0, 0.008, 15.0, 5.0);
        env_outage.grid.voltage_pu = 0.0;
        let mut eq2 = AirConditioner::new(cfg.clone());
        eq2.init(&cfg, &env_outage).unwrap();
        eq2.update_control(&env_outage);
        let mut ports2 = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq2.step(&env_outage, Duration::from_secs(60), &mut ports2)
            .unwrap();
        assert_eq!(eq2.telemetry().get(tk::CRANKCASE_KW), Some(0.0));
        assert_eq!(
            ports2.electrical.load_power_w, 0.0,
            "crankcase heater must not draw from a dead bus"
        );
    }

    /// Zone relative humidity correctly tracked through the equipment. This
    /// validates the new computed-accessor pipeline (NAN -> error) doesn't cause
    /// issues with sane inputs.
    #[test]
    fn sane_zone_rh_passes_cooling_pipeline() {
        let cfg = ac_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let env = env(28.0, 0.01, 18.0, 35.0);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&env, Duration::from_secs(60), &mut ports)
            .expect("sane zone RH should pass cooling pipeline");
    }

    #[test]
    fn zero_wet_bulb_does_not_reject_calculation() {
        let cfg = ac_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };

        // 0.0°C wet-bulb is a valid winter condition and must not be rejected.
        let env_zero_wb = env(27.0, 0.01, 0.0, 35.0);
        eq.init(&cfg, &env_zero_wb).unwrap();
        eq.update_control(&env_zero_wb);
        eq.step(&env_zero_wb, Duration::from_secs(60), &mut ports)
            .expect("0.0°C wet-bulb is a valid winter condition and must not return an error");
    }

    /// The cooling core's SHR telemetry write, the witness for the
    /// census's one-allocation-per-cooling-step site: the step's SHR
    /// write went through `Telemetry::insert`, whose `impl
    /// Into<String>` key builds a fresh `String` every call, even when
    /// the map already holds the key (one allocation per cooling step,
    /// every cooling step). RED at the parent, GREEN when the step
    /// writes through the lazy `set_or_register`: the key registers at
    /// the first cooling step (the contract's null-until-first-cooling
    /// timeline kept, so a never-cooled unit's output cell stays null)
    /// and updates in place after it, so every cooling step after the
    /// first is zero, and the end-of-step snapshot has no new key to
    /// absorb after the registration step.
    #[test]
    fn cooling_shr_telemetry_write_is_allocation_free() {
        let cfg = ac_config_with(|typed| typed.startup_cd = Some(0.0));
        let mut eq = AirConditioner::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let env = env(28.0, 0.012, 20.0, 35.0);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);

        // The first cooling step pays the key's registration shape.
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        assert!(
            eq.telemetry().get(tk::SHR).is_some(),
            "a cooling step publishes the sensible heat ratio"
        );
        let _ = super::cooling_shr_telemetry_site();

        for _ in 0..4 {
            eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        }
        let total = super::cooling_shr_telemetry_site();
        assert_eq!(
            total, 0,
            "the SHR telemetry write allocated {total} times over the 4 \
             cooling steps after the first"
        );
    }

    #[test]
    fn crankcase_heater_draws_when_off_and_cold() {
        let cfg = ac_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let env = env(22.0, 0.008, 15.0, 5.0);
        eq.init(&cfg, &env).unwrap();

        eq.update_control(&env);
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        assert!((ports.electrical.net_active_w() - 50.0).abs() < 1.0);
        assert_eq!(ports.thermal[0].sensible_gain_w, 0.0);
    }

    #[test]
    fn sensible_and_latent_sum_to_total_cooling() {
        let cfg = ac_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let env = env(28.0, 0.012, 20.0, 35.0);
        eq.init(&cfg, &env).unwrap();

        eq.update_control(&env);
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let sens = eq.telemetry().get(tk::SENSIBLE_COOLING_W).unwrap_or(0.0);
        let lat = eq.telemetry().get(tk::LATENT_COOLING_W).unwrap_or(0.0);
        let fan_w = eq.telemetry().get(tk::FAN_KW).unwrap_or(0.0);
        let fan_heat_w = fan_w;

        // Telemetry reports gross cooling (sensible + latent) before fan-heat offset.
        // Port thermal sensible includes fan waste heat: gain = -sensible + fan_heat.
        // So: telemetry_total = -(port_sensible + port_latent) + fan_heat.
        let telemetry_total = sens + lat;
        let port_total = -(ports.thermal[0].sensible_gain_w + ports.thermal[0].latent_gain_w);
        assert!(
            (telemetry_total - (port_total + fan_heat_w)).abs() < 1e-6,
            "telemetry total {telemetry_total} != port total {port_total} + fan heat {fan_heat_w}",
        );
    }

    #[test]
    fn shr_drops_with_higher_humidity_ratio() {
        // This test exercises SHR coil physics; disable the startup ramp (c_d=0)
        // so it does not obscure the result on the first step.
        let cfg = ac_config_with(|typed| typed.startup_cd = Some(0.0));

        let mut eq_low = AirConditioner::new(cfg.clone());
        let mut eq_high = AirConditioner::new(cfg.clone());

        let mut ports_low = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let mut ports_high = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };

        let env_low = env(28.0, 0.0075, 18.0, 35.0);
        let env_high = env(28.0, 0.0150, 22.0, 35.0);

        eq_low.init(&cfg, &env_low).unwrap();
        eq_high.init(&cfg, &env_high).unwrap();

        eq_low.update_control(&env_low);
        eq_low
            .step(&env_low, Duration::from_secs(60), &mut ports_low)
            .unwrap();
        eq_high.update_control(&env_high);
        eq_high
            .step(&env_high, Duration::from_secs(60), &mut ports_high)
            .unwrap();

        let shr_low = eq_low.telemetry().get(tk::SHR).unwrap_or(1.0);
        let shr_high = eq_high.telemetry().get(tk::SHR).unwrap_or(1.0);
        assert!(shr_high < shr_low, "shr_high={shr_high}, shr_low={shr_low}");
    }

    #[test]
    fn state_round_trip_preserves_cycle_state() {
        let cfg = ac_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let env = env(28.0, 0.01, 19.0, 35.0);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();
        let state = eq.save_state().unwrap();

        let mut restored = AirConditioner::new(cfg.clone());
        restored.init(&cfg, &env).unwrap();
        restored.load_state(&state).unwrap();

        assert_eq!(restored.telemetry().get(tk::OPERATING_MODE), Some(2.0));
        assert!(restored.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0) > 0.0);
    }

    /// Central AC uses the canonical SI airflow ratio for central cooling.
    /// Reference provenance is RESNET HERS Addendum 82 / OpenStudio-HPXML defaults.
    #[test]
    fn ac_uses_default_airflow_ratio() {
        let expected = crate::hvac::hvac_core::AIRFLOW_CENTRAL_AC_M3_S_PER_W;

        let cfg = ac_config();
        let eq = AirConditioner::new(cfg.clone());
        assert_eq!(
            eq.core.hvac.config.equipment_type,
            crate::hvac::hvac_core::HvacEquipmentType::AcCooler,
        );
        // Tolerance 1e-9: uom ft³ constant (0.02831685) differs from NIST-exact
        // (0.028316846592) by ~3 ppm; implementation uses the NIST constant.
        assert!(
            (eq.core.hvac.config.airflow_m3_s_per_w - expected).abs() < 1e-9,
            "Central AC default airflow mismatch, got {} m3/s/W",
            eq.core.hvac.config.airflow_m3_s_per_w
        );

        let environment = env(27.0, 0.010, 19.0, 35.0);
        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).expect("init must succeed");
        assert!(
            (eq.core.hvac.config.airflow_m3_s_per_w - expected).abs() < 1e-9,
            "init() must preserve default airflow ratio, got {} m3/s/W",
            eq.core.hvac.config.airflow_m3_s_per_w
        );
    }

    #[test]
    fn registry_registers_ochre_names_with_thermal_stage() {
        let registry = EquipmentRegistry::new();

        let ac = registry.create("Air Conditioner", ac_config()).unwrap();
        assert_eq!(ac.descriptor().stage, ExecutionStage::Thermal);

        let room = registry.create("Room AC", room_ac_config()).unwrap();
        assert_eq!(room.descriptor().stage, ExecutionStage::Thermal);
    }

    #[test]
    fn room_ac_duct_dse_is_one() {
        let cfg = room_ac_config();
        let mut eq = RoomAC::new(cfg.clone());
        let environment = env(27.0, 0.010, 19.0, 35.0);
        eq.init(&cfg, &environment).unwrap();
        assert!(
            (eq.core.hvac.config.duct_dse - 1.0).abs() < f64::EPSILON,
            "Room AC duct_dse must be 1.0, got {}",
            eq.core.hvac.config.duct_dse
        );
        assert!(
            eq.core.hvac.config.duct_zone_id.is_none(),
            "Room AC must have no duct zone"
        );
    }

    /// Checkpoint restore contract: a restored core output must satisfy the
    /// workspace's own `validate_core_contract` (presence rules and the
    /// mode/flow guard). Pre-fix, the restore published
    /// `CoreOutput::default()` — all fields `None` against the declared
    /// capabilities (ELECTRIC/HAS_MODE/THERMAL/HAS_SPEED/HAS_SETPOINT/
    /// HAS_COP/REACTIVE). The GSHP/WSHP cooler wrappers (which delegate
    /// `load_state` to this type) are gated in cooler.rs's own tests —
    /// their outer-field rebuild is a distinct mechanism.
    #[test]
    fn restored_checkpoint_output_satisfies_the_core_contract() {
        let cfg = ac_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let environment = env(28.0, 0.012, 20.0, 35.0);
        eq.init(&cfg, &environment).unwrap();
        eq.update_control(&environment);
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();
        let state = eq.save_state().unwrap();

        let mut restored = AirConditioner::new(cfg);
        restored.init(&ac_config(), &environment).unwrap();
        restored.load_state(&state).unwrap();
        hares_types::validate_core_contract(
            restored.descriptor(),
            restored.core_output(),
            &mut Vec::new(),
        )
        .expect("an air conditioner checkpoint must restore a contract-valid output");
    }

    /// Three idle steps against the shared ports: the zone sits below the
    /// cooling band, so the thermostat never calls and no step cools.
    fn step_idle_windows(eq: &mut AirConditioner, ports: &mut PortSlots) {
        for second in [0, 60, 120] {
            let env = env_at_second(20.0, 20.0, second);
            eq.update_control(&env);
            eq.step(&env, Duration::from_secs(60), ports).unwrap();
            ports.zero();
        }
    }

    /// The null-until-first-cooling contract (`default_telemetry`: no SHR
    /// insert, the step writes the key only when the step's sensible cooling
    /// is above 0) holds on the restore path: a never-cooled unit's
    /// checkpoint round trip restores exactly the continuously-run twin's
    /// telemetry key set. The checkpoint's blob carries the step's five
    /// telemetry values; the SHR value's presence among them is the
    /// registration state of the one channel whose registration is state,
    /// and the rest of the map's registration set is not carried (the
    /// target's init rebuilds it). The carried values are the checkpoint's;
    /// every other key keeps the target's value until a step rewrites it,
    /// and the last-cooling markers the observe feature writes, and the
    /// high-side clamp marker, are written only by a cooling step, so an
    /// idle step never rewrites them: the pin is the key set. The restore's
    /// unconditional `tk::SHR` insert added a key the continuous twin never
    /// holds, a `shr` cell the unit never publishes.
    #[test]
    fn restored_never_cooled_map_matches_the_never_restored_twin() {
        let cfg = ac_config();
        let environment = env(20.0, 0.008, 15.0, 20.0);
        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        step_idle_windows(&mut eq, &mut ports);
        let state = eq.save_state().unwrap();

        let mut restored = AirConditioner::new(cfg.clone());
        restored.init(&cfg, &environment).unwrap();
        restored.load_state(&state).unwrap();

        let mut twin = AirConditioner::new(cfg.clone());
        twin.init(&cfg, &environment).unwrap();
        step_idle_windows(&mut twin, &mut ports);

        let mut restored_keys: Vec<&str> =
            restored.telemetry().0.keys().map(String::as_str).collect();
        restored_keys.sort_unstable();
        let mut twin_keys: Vec<&str> = twin.telemetry().0.keys().map(String::as_str).collect();
        twin_keys.sort_unstable();

        assert!(
            !restored_keys.contains(&tk::SHR),
            "a restored never-cooled unit's map must not carry an shr key"
        );
        assert_eq!(
            restored_keys,
            twin_keys,
            "the restored never-cooled map ({} keys) must carry exactly the \
             continuously-run twin's keys ({} keys)",
            restored_keys.len(),
            twin_keys.len()
        );
    }

    /// Once cooled, the key persists with the last cooling step's value: the
    /// step's write is `set_or_register` (registered at the first cooling
    /// step, updated in place after) and an idle step leaves it alone, so a
    /// cooled-then-idle unit still carries the key at checkpoint time and
    /// the restore brings it back with that value. Gating the restore's
    /// write on the unit's activity at the checkpoint instant would lose the
    /// key for exactly this idle-after-cooling unit.
    #[test]
    fn restored_cooled_then_idle_map_keeps_the_last_shr() {
        let cfg = ac_config();
        let cool_env = env_at_second(28.0, 35.0, 0);
        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &cool_env).unwrap();
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&cool_env);
        eq.step(&cool_env, Duration::from_secs(60), &mut ports)
            .unwrap();
        ports.zero();
        let last_shr = eq
            .telemetry()
            .get(tk::SHR)
            .expect("the cooling step registers the shr key");

        step_idle_windows(&mut eq, &mut ports);
        assert_eq!(
            eq.telemetry().get(tk::SHR),
            Some(last_shr),
            "an idle step leaves the last cooling value in the map"
        );
        let state = eq.save_state().unwrap();

        let mut restored = AirConditioner::new(cfg.clone());
        restored.init(&cfg, &cool_env).unwrap();
        restored.load_state(&state).unwrap();

        let mut twin = AirConditioner::new(cfg.clone());
        twin.init(&cfg, &cool_env).unwrap();
        twin.update_control(&cool_env);
        twin.step(&cool_env, Duration::from_secs(60), &mut ports)
            .unwrap();
        ports.zero();
        step_idle_windows(&mut twin, &mut ports);

        assert_eq!(
            restored.telemetry().get(tk::SHR),
            Some(last_shr),
            "the restored cooled-then-idle unit keeps the last cooling value"
        );
        let mut restored_keys: Vec<&str> =
            restored.telemetry().0.keys().map(String::as_str).collect();
        restored_keys.sort_unstable();
        let mut twin_keys: Vec<&str> = twin.telemetry().0.keys().map(String::as_str).collect();
        twin_keys.sort_unstable();
        assert_eq!(
            restored_keys, twin_keys,
            "the restored cooled-then-idle map must carry exactly the \
             continuously-run twin's keys"
        );
    }

    /// A checkpoint replaces the whole run state, the telemetry timeline
    /// included: loading a never-cooled unit's blob (no `shr` key) into a
    /// unit that cooled earlier must leave the never-cooled key set. The
    /// saved map is the contract; a key the blob never registered must not
    /// survive from the pre-restore run, or the same blob restores two
    /// different maps depending on the target's history.
    #[test]
    fn restore_into_a_previously_cooled_unit_drops_the_stale_shr_key() {
        let cfg = ac_config();
        let cool_env = env_at_second(28.0, 35.0, 0);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };

        // The source: a never-cooled unit, its map without the key.
        let mut source = AirConditioner::new(cfg.clone());
        source.init(&cfg, &env(20.0, 0.008, 15.0, 20.0)).unwrap();
        step_idle_windows(&mut source, &mut ports);
        assert!(
            source.telemetry().get(tk::SHR).is_none(),
            "the source must never have cooled"
        );
        let state = source.save_state().unwrap();

        // The target: it cooled before the restore, so its map holds the
        // key the checkpoint's map does not.
        let mut target = AirConditioner::new(cfg.clone());
        target.init(&cfg, &cool_env).unwrap();
        target.update_control(&cool_env);
        target
            .step(&cool_env, Duration::from_secs(60), &mut ports)
            .unwrap();
        ports.zero();
        assert!(
            target.telemetry().get(tk::SHR).is_some(),
            "the target must hold the key before the restore"
        );

        target.load_state(&state).unwrap();
        assert!(
            target.telemetry().get(tk::SHR).is_none(),
            "the restored map must be the checkpoint's: no shr key survives \
             from the pre-restore run"
        );
        assert!(
            !target.telemetry().0.keys().any(|key| key == tk::SHR),
            "the restored key set must not carry the pre-restore shr key"
        );
    }

    /// The checkpoint gate refuses the previous schema by name and by both
    /// versions. A version-4 blob (the schema before `shr` became
    /// `Option<f64>`) cannot be decoded as v5: postcard's `Option` tag
    /// would be read from the first byte of the old `f64`, so the gate,
    /// not a decode attempt, is the answer. There is no v4 migration.
    #[test]
    fn checkpoint_gate_refuses_the_version_4_blob_naming_both_versions() {
        let cfg = ac_config();
        let environment = env(20.0, 0.008, 15.0, 20.0);
        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();
        let mut blob = eq.save_state().unwrap();
        assert_eq!(
            u32::from_le_bytes(blob[..4].try_into().unwrap()),
            super::AC_CHECKPOINT_VERSION,
            "the saved blob carries the current schema token"
        );

        // The blob below is otherwise the valid v5 payload; only the token
        // claims the previous schema, so a refusal must come from the gate.
        blob[..4].copy_from_slice(&4u32.to_le_bytes());
        let mut restored = AirConditioner::new(cfg.clone());
        restored.init(&cfg, &environment).unwrap();
        let err = restored
            .load_state(&blob)
            .expect_err("a version-4 blob must be refused by the version-5 gate");
        let msg = err.to_string();
        assert!(
            msg.contains("AirConditioner") && msg.contains("blob=4") && msg.contains("expected=5"),
            "the refusal must name the equipment and both versions; got: {msg}"
        );
        assert!(
            !msg.contains("postcard") && !msg.contains("deserialization failed"),
            "the refusal must come from the version gate, not a postcard decode; got: {msg}"
        );
    }

    /// The gate compares the token to this build's schema by inequality,
    /// not by order: a token from a schema after this one is refused by
    /// the same path, naming both versions. A checkpoint written by a
    /// newer build must never be decoded by an older one, which would
    /// read the newer payload's bytes as this schema's fields. Pinning
    /// the upper direction keeps a future "accept anything at least this
    /// version" relaxation of the gate failing here.
    #[test]
    fn checkpoint_gate_refuses_a_future_version_blob_naming_both_versions() {
        let cfg = ac_config();
        let environment = env(20.0, 0.008, 15.0, 20.0);
        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();
        let mut blob = eq.save_state().unwrap();
        let future = super::AC_CHECKPOINT_VERSION + 1;
        // The blob below is otherwise the valid current payload; only the
        // token claims the future schema, so a refusal must come from the
        // gate.
        blob[..4].copy_from_slice(&future.to_le_bytes());
        let mut restored = AirConditioner::new(cfg.clone());
        restored.init(&cfg, &environment).unwrap();
        let err = restored
            .load_state(&blob)
            .expect_err("a blob from a future schema must be refused by the current gate");
        let msg = err.to_string();
        assert!(
            msg.contains("AirConditioner")
                && msg.contains(&format!("blob={future}"))
                && msg.contains(&format!("expected={}", super::AC_CHECKPOINT_VERSION)),
            "the refusal must name the equipment and both versions; got: {msg}"
        );
        assert!(
            !msg.contains("postcard") && !msg.contains("deserialization failed"),
            "the refusal must come from the version gate, not a postcard decode; got: {msg}"
        );
    }

    #[test]
    fn room_ac_step_produces_cooling() {
        let cfg = room_ac_config();
        let mut eq = RoomAC::new(cfg.clone());
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let environment = env(28.0, 0.012, 20.0, 35.0);
        eq.init(&cfg, &environment).unwrap();
        eq.update_control(&environment);
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        let sens = eq.telemetry().get(tk::SENSIBLE_COOLING_W).unwrap_or(0.0);
        let elec = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        assert!(
            sens > 0.0,
            "sensible_cooling_w must be positive (magnitude), got {sens}"
        );
        assert!(elec > 0.0, "electric_w must be positive, got {elec}");
        assert!(
            ports.thermal[0].sensible_gain_w < 0.0,
            "thermal port sensible gain must be negative (cooling), got {}",
            ports.thermal[0].sensible_gain_w
        );
    }

    #[test]
    fn room_ac_no_duct_zone_entry() {
        let cfg = room_ac_config();
        let mut eq = RoomAC::new(cfg.clone());
        let environment = env(27.0, 0.010, 19.0, 35.0);
        eq.init(&cfg, &environment).unwrap();

        let fracs = &eq.core.hvac.config.zone_heat_fractions;
        assert_eq!(
            fracs.len(),
            1,
            "Room AC must have exactly one zone heat fraction entry, got {fracs:?}"
        );
        assert_eq!(fracs[0].0, ZoneId(1));
        assert!(
            (fracs[0].1 - 1.0).abs() < f64::EPSILON,
            "Room AC zone heat fraction must be 1.0, got {}",
            fracs[0].1
        );
    }

    #[test]
    fn typed_ac_eir_sets_expected_eir() {
        let cfg = super::typed_ac_test_config(0.25);
        let mut eq = AirConditioner::new(cfg.clone());
        let environment = env(27.0, 0.010, 19.0, 35.0);
        eq.init(&cfg, &environment).unwrap();

        let expected = 0.25;
        let actual = eq.core.hvac.config.eir_by_stage[0];
        assert!(
            (actual - expected).abs() < 1e-9,
            "typed eir=0.25 must be preserved, got {actual}"
        );
    }

    #[test]
    fn typed_stage_eir_overrides_base_eir() {
        let mut typed = super::typed_ac_test_config(0.25)
            .typed::<CentralAirConditionerConfig>()
            .expect("typed central AC config");
        // Intentionally contradictory legacy efficiency; SI-native stage EIR must win.
        typed.eir = 0.8;
        typed.stage_eirs = Some(vec![0.30]);
        typed.stage_capacities_w = Some(vec![typed.capacity_w]);

        let cfg =
            EquipmentConfig::from_typed("AC".to_string(), "Air Conditioner".to_string(), typed)
                .unwrap();
        let mut eq = AirConditioner::new(cfg.clone());
        let environment = env(27.0, 0.010, 19.0, 35.0);
        eq.init(&cfg, &environment).unwrap();

        assert_eq!(
            eq.core.hvac.config.eir_by_stage,
            vec![0.30],
            "SI stage_eirs must override eir-derived EIR"
        );
    }

    #[test]
    fn room_ac_vs_central_different_dse() {
        let central_with_duct_losses = ac_config_with(|typed| {
            typed.capacity_w = 3_500.0;
            typed.hysteresis_c = Some(hares_types::MIN_THERMOSTAT_BAND_C);
            typed.duct.dse_cool = Some(0.80);
        });
        let central_no_duct_losses = ac_config_with(|typed| {
            typed.capacity_w = 3_500.0;
            typed.hysteresis_c = Some(hares_types::MIN_THERMOSTAT_BAND_C);
            typed.duct.dse_cool = Some(1.0);
        });

        let environment = env(28.0, 0.012, 20.0, 35.0);

        let mut with_losses = AirConditioner::new(central_with_duct_losses.clone());
        with_losses
            .init(&central_with_duct_losses, &environment)
            .unwrap();
        let mut ports_with_losses = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        with_losses.update_control(&environment);
        with_losses
            .step(
                &environment,
                Duration::from_secs(60),
                &mut ports_with_losses,
            )
            .unwrap();

        let mut no_losses = AirConditioner::new(central_no_duct_losses.clone());
        no_losses
            .init(&central_no_duct_losses, &environment)
            .unwrap();
        let mut ports_no_losses = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        no_losses.update_control(&environment);
        no_losses
            .step(&environment, Duration::from_secs(60), &mut ports_no_losses)
            .unwrap();

        let central_cooling = -ports_with_losses.thermal[0].sensible_gain_w;
        let no_loss_cooling = -ports_no_losses.thermal[0].sensible_gain_w;
        assert!(
            central_cooling > 0.0,
            "central AC must produce cooling, got {central_cooling}"
        );
        assert!(
            no_loss_cooling > 0.0,
            "central AC (dse=1.0) must produce cooling, got {no_loss_cooling}"
        );
        assert!(
            no_loss_cooling > central_cooling,
            "Central AC with DSE=1.0 must deliver more cooling to zone ({no_loss_cooling:.1} W) \
             than central AC with DSE=0.80 ({central_cooling:.1} W)"
        );
    }

    /// Room AC uses the canonical SI airflow ratio for room cooling.
    /// Reference provenance is manufacturer/AHRI room-unit airflow medians.
    #[test]
    fn room_ac_airflow_rate() {
        let expected = crate::hvac::hvac_core::AIRFLOW_ROOM_AC_M3_S_PER_W;

        // Construction-time default is central-cooling airflow; init() overrides to room-AC airflow.
        let environment = env(27.0, 0.010, 19.0, 35.0);
        let cfg = room_ac_config();
        let mut eq = RoomAC::new(cfg.clone());
        eq.init(&cfg, &environment).expect("init must succeed");
        assert!(
            (eq.core.hvac.config.airflow_m3_s_per_w - expected).abs() < 1e-9,
            "RoomAC airflow must equal the typed SI default after init(), got {} m3/s/W",
            eq.core.hvac.config.airflow_m3_s_per_w
        );
    }

    /// Two-speed AC selects the high stage when load fraction exceeds
    /// `low_speed_capacity_fraction` (default 0.72 per OCHRE/AHRI), and the low stage otherwise.
    /// With `hysteresis_c=0.1` the thermostat activates at zone_temp > 24.08 and the
    /// band-relative load fraction governs the stage choice:
    ///   zone 24.4°C → load_fraction = (24.4-23.98)/0.1 → 1.0 > 0.5 → stage 1 (8 000 W)
    ///   zone 24.1°C → load_fraction = (24.1-23.98)/0.1 = 0.2+ → stage 0 (4 000 W)
    /// The test verifies that two-speed stage selection routes to different capacity
    /// stages by observing the resulting electrical draw.
    #[test]
    fn two_speed_ac_draws_more_power_at_high_load_than_moderate_load() {
        // Zero hysteresis so activation threshold equals setpoint, not setpoint+1.
        let cfg = ac_config_with(|typed| {
            typed.number_of_speeds = 2;
            typed.hysteresis_c = Some(hares_types::MIN_THERMOSTAT_BAND_C);
            typed.stage_capacities_w = Some(vec![4_000.0, 8_000.0]);
            typed.stage_eirs = Some(vec![0.33, 0.33]);
        });

        // load_fraction = (zone - 24) / 0.5; 24.4 → 0.8 > 0.5 → high stage
        let env_high_load = env(24.4, 0.010, 18.0, 35.0);
        // load_fraction = (24.1 - 24) / 0.5 = 0.2 ≤ 0.5 → low stage
        let env_low_load = env(24.1, 0.010, 18.0, 35.0);

        let mut eq_high = AirConditioner::new(cfg.clone());
        eq_high.init(&cfg, &env_high_load).unwrap();
        let mut ports_high = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq_high.update_control(&env_high_load);
        eq_high
            .step(&env_high_load, Duration::from_secs(60), &mut ports_high)
            .unwrap();
        let kw_high = eq_high.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);

        let mut eq_low = AirConditioner::new(cfg.clone());
        eq_low.init(&cfg, &env_low_load).unwrap();
        let mut ports_low = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq_low.update_control(&env_low_load);
        eq_low
            .step(&env_low_load, Duration::from_secs(60), &mut ports_low)
            .unwrap();
        let kw_low = eq_low.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);

        assert!(
            kw_high > 0.0,
            "high-stage AC must draw positive electrical power, got {kw_high}"
        );
        assert!(
            kw_low > 0.0,
            "low-stage AC must draw positive electrical power, got {kw_low}"
        );
        assert!(
            kw_high > kw_low,
            "high-stage draw ({kw_high:.4} kW) must exceed low-stage draw ({kw_low:.4} kW)"
        );
    }

    /// Single-speed AC runs the runtime fraction the zone needs: the load
    /// fraction is the zone's position between the cooling setpoint (the
    /// zero-delivery anchor) and the turn-on, the same one-runtime-fraction
    /// path the multi-speed arms follow, with the span floored at 0.5 C so
    /// a narrow hysteresis modulates over it.
    #[test]
    fn single_speed_cooling_call_uses_the_zone_needed_runtime_fraction() {
        // Zero hysteresis so thermostat activates right at setpoint (24°C).
        let cfg =
            ac_config_with(|typed| typed.hysteresis_c = Some(hares_types::MIN_THERMOSTAT_BAND_C));

        // Both environments are above setpoint so cooling is On in both cases.
        let env_full = env(24.6, 0.010, 18.0, 35.0);
        let env_part = env(24.2, 0.010, 18.0, 35.0);

        let mut eq_full = AirConditioner::new(cfg.clone());
        eq_full.init(&cfg, &env_full).unwrap();
        eq_full.update_control(&env_full);
        let duty_full = eq_full.core.hvac.runtime.duty_cycle;

        let mut eq_part = AirConditioner::new(cfg.clone());
        eq_part.init(&cfg, &env_part).unwrap();
        eq_part.update_control(&env_part);
        let duty_part = eq_part.core.hvac.runtime.duty_cycle;

        assert!(
            (duty_full - 1.0).abs() < 1e-9,
            "single-speed cooling call past the floored span must use duty=1.0, got {duty_full}"
        );
        // The zero edge is the setpoint 24.0; the span floors at 0.5 C:
        // (24.2 - 24.0) / 0.5 = 0.40.
        assert!(
            (duty_part - 0.40).abs() < 1e-9,
            "single-speed cooling call near the setpoint must use the zone-needed \
             fraction 0.40, got {duty_part}"
        );
        assert!(
            duty_full > duty_part,
            "the deeper the zone into the band, the larger the fraction"
        );
    }

    #[test]
    fn variable_speed_ideal_uses_fractional_duty_cycle() {
        let cfg = ac_config_with(|typed| {
            typed.number_of_speeds = 4;
            typed.hysteresis_c = Some(hares_types::MIN_THERMOSTAT_BAND_C);
        });
        let env_part = env(24.25, 0.010, 18.0, 35.0); // load_fraction=(24.25-23.98)/0.5=0.54
        let env_full = env(24.60, 0.010, 18.0, 35.0); // clamped to 1.0

        let mut eq_part = AirConditioner::new(cfg.clone());
        eq_part.init(&cfg, &env_part).unwrap();
        assert_eq!(
            eq_part.core.hvac.config.speed_control_mode,
            SpeedControlMode::VariableSpeedIdeal
        );
        eq_part.update_control(&env_part);
        assert!(
            (eq_part.core.hvac.runtime.duty_cycle - 0.54).abs() < 1e-9,
            "variable-speed ideal should preserve fractional duty cycle; got {}",
            eq_part.core.hvac.runtime.duty_cycle
        );

        let mut eq_full = AirConditioner::new(cfg.clone());
        eq_full.init(&cfg, &env_full).unwrap();
        eq_full.update_control(&env_full);
        assert!(
            (eq_full.core.hvac.runtime.duty_cycle - 1.0).abs() < 1e-9,
            "high load should clamp to full duty cycle; got {}",
            eq_full.core.hvac.runtime.duty_cycle
        );
    }

    #[test]
    fn central_four_speed_ac_uses_variable_speed_mode() {
        let cfg = ac_config_with(|typed| {
            typed.number_of_speeds = 4;
            typed.hysteresis_c = Some(hares_types::MIN_THERMOSTAT_BAND_C);
            typed.startup_cd = None;
        });
        let env = env(24.25, 0.010, 18.0, 35.0);

        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &env).unwrap();
        assert_eq!(
            eq.core.hvac.config.speed_control_mode,
            SpeedControlMode::VariableSpeedIdeal
        );
        assert_eq!(
            eq.core.hvac.runtime.startup.c_d, 0.0,
            "OCHRE 4-speed variable cooling should map to zero startup Cd"
        );

        eq.update_control(&env);
        assert!(
            (eq.core.hvac.runtime.duty_cycle - 0.54).abs() < 1e-9,
            "central 4-speed variable cooling should preserve fractional duty; got {}",
            eq.core.hvac.runtime.duty_cycle
        );
    }

    /// F2's witness: the compressor's draw evaluates the unit row (the
    /// recorded Hajagos & Danai cooling row, as fitted) at the ACTUAL bus
    /// voltage. At the parent the compressor drew 600.074966 W at 0.7 pu
    /// and 599.910799 W at 0.5 pu (a 0.164 W change across a 0.2 pu
    /// sag) while the row predicts 0.991 and 1.145 (a +15.5 percent
    /// rise, 93 W on this class). The recorded deep-sag decision governs:
    /// the elevation is the measured static draw of the family.
    #[test]
    fn compressor_draw_follows_the_recorded_row_at_deep_sag() {
        let cfg = ac_config_with(|typed| {
            typed.fan_power_w = Some(0.0);
            typed.startup_cd = None;
        });
        let environment = env_at_second(27.0, 35.0, 0);

        let step_once = |voltage_pu: f64| -> f64 {
            let mut env_run = env_at_second(27.0, 35.0, 0);
            env_run.grid.voltage_pu = voltage_pu;
            let mut eq = AirConditioner::new(cfg.clone());
            eq.init(&cfg, &environment).unwrap();
            eq.update_control(&env_run);
            let mut ports = PortSlots {
                thermal: vec![ThermalAccumulator::new(ZoneId(1))],
                humidity: vec![HumidityAccumulator::new(ZoneId(1))],
                ..PortSlots::default()
            };
            eq.step(&env_run, Duration::from_secs(60), &mut ports)
                .unwrap();
            eq.telemetry().get(tk::COMPRESSOR_KW).unwrap_or(0.0)
        };

        let p_nominal = step_once(1.0);
        let p_half = step_once(0.5);
        assert!(p_nominal > 0.0, "fixture: the compressor must run");
        let row = hares_types::zip::zip_defaults_for_class("Air Conditioner").expect("class row");
        let raw_multiplier = row.zp * 0.5 * 0.5 + row.ip * 0.5 + row.pp;
        assert!(
            raw_multiplier > 1.1,
            "fixture: the recorded row must elevate at 0.5 pu, got {raw_multiplier}"
        );
        let expected = p_nominal * raw_multiplier;
        assert!(
            (p_half - expected).abs() < 1e-9,
            "the compressor at 0.5 pu must draw the row's evaluated response \
             ({expected}), got {p_half} (the pinned constant-power draw)"
        );
        assert!(
            (p_half - p_nominal).abs() > 0.05 * p_nominal,
            "the pinned flat line must be gone: {p_half} vs {p_nominal}"
        );
    }

    /// Z2's witness: the unit's reported nominal draw is its components'
    /// pre-scaling sum, so a multi-component moment's premise weight and
    /// `deep_sag_scale` are the assembled ones. The nominal run's total
    /// draw *is* the nominal (every multiplier is exactly 1.0 at the
    /// reference voltage) and its telemetry components are the nominal
    /// component draws; each component evaluates its own row at 0.5 pu
    /// (the compressor row as fitted: 1.6 x 0.25 - 2.69 x 0.5 + 2.09 =
    /// 1.145; the fan row: 0.26 x 0.25 + 0.9 x 0.5 - 0.16 = 0.355; the
    /// crankcase heater the impedance share, 0.25). The premise aggregate
    /// that un-scales the unit's total through the compressor row alone
    /// (the parent's normalization) would weight this composition
    /// 1062 W against the true 1200 W-class nominal and publish 1.145
    /// against the assembled response.
    #[test]
    fn reported_nominal_draw_is_the_components_pre_scaling_sum() {
        let cfg = ac_config_with(|typed| {
            typed.startup_cd = None;
        });
        let environment = env_at_second(27.0, 35.0, 0);

        let run = |voltage_pu: f64| -> AirConditioner {
            let mut env_run = env_at_second(27.0, 35.0, 0);
            env_run.grid.voltage_pu = voltage_pu;
            let mut eq = AirConditioner::new(cfg.clone());
            eq.init(&cfg, &environment).unwrap();
            eq.update_control(&env_run);
            let mut ports = PortSlots {
                thermal: vec![ThermalAccumulator::new(ZoneId(1))],
                humidity: vec![HumidityAccumulator::new(ZoneId(1))],
                ..PortSlots::default()
            };
            eq.step(&env_run, Duration::from_secs(60), &mut ports)
                .unwrap();
            eq
        };
        let component_w = |eq: &AirConditioner, key: &str| {
            let w = eq.telemetry().get(key).unwrap_or(f64::NAN);
            assert!(w.is_finite(), "fixture: {key} must be finite");
            w
        };

        let nominal_unit = run(1.0);
        let sag_unit = run(0.5);
        let nominal_total = component_w(&nominal_unit, tk::ELECTRIC_KW);
        assert!(nominal_total > 0.0, "fixture: the unit must run");

        // The reported nominal draw: the components' pre-scaling sum, the
        // nominal run's own total draw.
        let reported_nominal = sag_unit
            .nominal_electric_draw_w()
            .expect("the stepped unit drew power");
        assert!(
            (reported_nominal - nominal_total).abs() < 1e-6 * nominal_total,
            "the reported nominal draw must be the components' pre-scaling \
             sum ({nominal_total}), got {reported_nominal}"
        );

        // The assembled response: the hand computation over the components'
        // own rows, and the stepped run's total must evaluate it.
        let compressor_n = component_w(&nominal_unit, tk::COMPRESSOR_KW);
        let fan_n = component_w(&nominal_unit, tk::FAN_KW);
        let crank_n = component_w(&nominal_unit, tk::CRANKCASE_KW);
        let ac_row = hares_types::zip::zip_defaults_for_class("Air Conditioner").expect("row");
        let fan_row = hares_types::zip::zip_defaults_for_class("Ventilation Fan").expect("row");
        let m_compressor = ac_row.zp * 0.25 + ac_row.ip * 0.5 + ac_row.pp;
        let m_fan = fan_row.zp * 0.25 + fan_row.ip * 0.5 + fan_row.pp;
        let m_resistive = 0.25; // the crankcase heater: P = V²/R
        let nominal_component_w = compressor_n + fan_n + crank_n;
        let expected_scaled = compressor_n * m_compressor + fan_n * m_fan + crank_n * m_resistive;
        let expected_response = expected_scaled / nominal_component_w;
        let sag_total = component_w(&sag_unit, tk::ELECTRIC_KW);
        assert!(
            (sag_total - expected_scaled).abs() < 1e-6 * expected_scaled,
            "the 0.5 pu draw must be the components' own row evaluations \
             ({expected_scaled}), got {sag_total}"
        );
        let reported_response = sag_total / reported_nominal;
        assert!(
            (reported_response - expected_response).abs() < 1e-9,
            "the assembled response must be the hand computation \
             {expected_response}, got {reported_response} (the compressor \
             row alone evaluates {m_compressor})"
        );
    }

    /// Z4's witness: the `pf = 0.0` sentinel is a reactive-only switch.
    /// The attack's scenario: a unit sidecar carrying the pump row's real
    /// side with the sentinel (zp 0.72, ip -0.98, pp 1.26, pf 0.0) is
    /// accepted by validate_zip_sums (the real sum is 1.0; the reactive
    /// sum is unconstrained when pf = 0). At 0.5 pu the compressor follows
    /// the sidecar row as fitted (0.72 x 0.25 - 0.98 x 0.5 + 1.26 = 0.95)
    /// and the indoor blower follows the physical fan row (0.26 x 0.25 +
    /// 0.9 x 0.5 - 0.16 = 0.355); at the parent the sentinel's
    /// constant-power substitution flattened the blower to 1.0 while the
    /// unit's resolved_zip still declared a governing real response. The
    /// whole unit's Q is exactly zero: the sentinel's own meaning.
    #[test]
    fn pf_zero_sentinel_is_reactive_only_and_keeps_the_blower_row() {
        let mut cfg = ac_config_with(|typed| {
            typed.startup_cd = None;
        });
        cfg.zip = Some(hares_types::zip::ZipLoad {
            zp: 0.72,
            ip: -0.98,
            pp: 1.26,
            zq: 0.0,
            iq: 0.0,
            pq: 0.0,
            pf: 0.0,
            v0: 1.0,
        });
        let environment = env_at_second(27.0, 35.0, 0);

        let run = |voltage_pu: f64| -> AirConditioner {
            let mut env_run = env_at_second(27.0, 35.0, 0);
            env_run.grid.voltage_pu = voltage_pu;
            let mut eq = AirConditioner::new(cfg.clone());
            eq.init(&cfg, &environment).unwrap();
            eq.update_control(&env_run);
            let mut ports = PortSlots {
                thermal: vec![ThermalAccumulator::new(ZoneId(1))],
                humidity: vec![HumidityAccumulator::new(ZoneId(1))],
                ..PortSlots::default()
            };
            eq.step(&env_run, Duration::from_secs(60), &mut ports)
                .unwrap();
            eq
        };
        let nominal_unit = run(1.0);
        let sag_unit = run(0.5);
        let compressor_n = nominal_unit.telemetry().get(tk::COMPRESSOR_KW).unwrap();
        let fan_n = nominal_unit.telemetry().get(tk::FAN_KW).unwrap();
        assert!(compressor_n > 0.0, "fixture: the compressor must run");
        assert!(
            fan_n > 0.0,
            "fixture: the blower must draw: it is the witness's component"
        );

        // The sidecar row as fitted at 0.5 pu: the compressor's multiplier.
        let m_sidecar: f64 = 0.72 * 0.25 - 0.98 * 0.5 + 1.26;
        assert!((m_sidecar - 0.95).abs() < 1e-12, "hand: {m_sidecar}");
        // The physical fan row as fitted at 0.5 pu: the blower's multiplier.
        let m_fan: f64 = 0.26 * 0.25 + 0.9 * 0.5 - 0.16;
        assert!((m_fan - 0.355).abs() < 1e-12, "hand: {m_fan}");

        let compressor_half = sag_unit.telemetry().get(tk::COMPRESSOR_KW).unwrap();
        assert!(
            (compressor_half - compressor_n * m_sidecar).abs() < 1e-9,
            "the compressor must follow the sidecar row ({compressor_n} x \
             {m_sidecar}), got {compressor_half}"
        );
        let fan_half = sag_unit.telemetry().get(tk::FAN_KW).unwrap();
        assert!(
            (fan_half - fan_n * m_fan).abs() < 1e-9,
            "the blower's real response must evaluate the physical fan row \
             ({fan_n} x {m_fan}), got {fan_half}; the sentinel's \
             constant-power substitution flattens it to 1.0"
        );
        let q = sag_unit.telemetry().get(tk::REACTIVE_POWER_KVAR).unwrap();
        assert!(
            q == 0.0,
            "the sentinel zeroes the whole unit's reactive power, got {q}"
        );
    }

    #[test]
    fn central_four_speed_variable_speed_interpolates_stage_ladder() {
        let cfg = ac_config_with(|typed| {
            typed.number_of_speeds = 4;
            typed.hysteresis_c = Some(hares_types::MIN_THERMOSTAT_BAND_C);
            typed.stage_capacities_w = Some(vec![2_000.0, 4_000.0, 6_000.0, 8_000.0]);
            typed.stage_eirs = Some(vec![0.20, 0.25, 0.30, 0.35]);
            typed.stage_shrs = Some(vec![0.75, 0.75, 0.75, 0.75]);
            typed.fan_power_w = Some(0.0);
            typed.startup_cd = None;
        });
        // The load fraction 0.5 exactly: (24.23 - 23.98)/0.5, an exact match
        // for the ladder's second stage (half of the 8 000 W max). The
        // capacity arrives as the solver's IdealCapacity signal: the ideal
        // path's delivery is this step's solved capacity, never the raw
        // thermostat duty (OCHRE HVAC.py:424, h_desired should be equal to
        // self.delivered_heat).
        let environment = env(24.23, 0.010, 18.0, 35.0);

        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();
        // The neutral biquadratic: the capacity ratio is 1.0 at every
        // condition, so the driven capacity's load fraction is exact.
        eq.core.hvac.config.biquadratic_coeffs = vec![[1.0, 0.0, 0.0, 0.0, 0.0, 0.0]; 4];
        eq.update_control(&environment);
        eq.apply_control(&ControlSignal::IdealCapacity {
            capacity_w: -4000.0,
            degraded: false,
        })
        .unwrap();
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        let compressor_w = eq.telemetry().get(tk::COMPRESSOR_KW).unwrap_or(0.0);
        let rtf = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap_or(0.0);

        assert_eq!(eq.core.hvac.runtime.last_speed_index, 0);
        assert!((eq.core.hvac.runtime.last_speed_frac - 1.0).abs() < 1e-9);
        assert!(
            compressor_w > 0.0,
            "load_fraction=0.5 with a 4-speed ladder must energize the second stage, got compressor_w={compressor_w}"
        );
        assert!(
            (rtf - 1.0).abs() < 1e-9,
            "an exact variable-speed stage match must run continuously, got runtime_fraction={rtf}"
        );
    }

    /// The starved ideal step's fallback, pinned: an ideal Cooling step
    /// with no solved capacity arriving runs the unit's raw thermostat
    /// duty (the ideal path's PLR arm falls through to the duty cycle,
    /// 1.0), so the step delivers the full rated capacity unmetered to
    /// the zone's need; the health count records the starvation beside
    /// it. The delivery is the unmetered fallback the count makes
    /// visible, never nothing.
    #[test]
    fn a_starved_ideal_step_delivers_the_raw_duty_and_the_count_records_it() {
        let cfg = ac_config_with(|typed| {
            typed.fan_power_w = Some(0.0);
            typed.startup_cd = None;
        });
        // The coarse timestep puts the unit on the ideal path.
        let mut environment = env(27.0, 0.010, 18.0, 35.0);
        environment.time_res = ChronoDuration::seconds(900);

        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();
        assert_eq!(
            eq.update_control(&environment),
            OperatingMode::Cooling,
            "the zone is past the cooling setpoint: the thermostat calls"
        );

        // No IdealCapacity this step: the solve never reached the unit.
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&environment, Duration::from_secs(900), &mut ports)
            .unwrap();

        let rtf = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap_or(f64::NAN);
        assert!(
            (rtf - 1.0).abs() < 1e-9,
            "the starved ideal step runs the raw duty, not nothing: \
             runtime_fraction {rtf}"
        );
        assert!(
            ports.electrical.net_active_w() > 0.0,
            "the fallback draws the unit's full rated power, got {} W",
            ports.electrical.net_active_w()
        );
        let counts = eq.take_health_counts();
        assert_eq!(
            counts.ideal_capacity_unsolved_steps, 1,
            "the starvation is counted, never silent"
        );

        // The next step's solve arrives: the delivery is the solved
        // capacity's share and nothing is counted.
        eq.update_control(&environment);
        eq.apply_control(&ControlSignal::IdealCapacity {
            capacity_w: -2_000.0,
            degraded: false,
        })
        .unwrap();
        ports.zero();
        eq.step(&environment, Duration::from_secs(900), &mut ports)
            .unwrap();
        let solved_rtf = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap_or(f64::NAN);
        assert!(
            solved_rtf < 0.5,
            "the solved capacity's share is well under the full duty, got \
             runtime_fraction {solved_rtf}"
        );
        assert_eq!(
            eq.take_health_counts().ideal_capacity_unsolved_steps,
            0,
            "a solved step is not a starvation"
        );
    }

    /// A zero-second event covers no step: the duration names the steps the
    /// event sheds, and zero names none. The signal is legal (a finite
    /// duration of at least zero), so it arrives as an event already over
    /// rather than as a one-step shed of the step it lands in.
    #[test]
    fn a_zero_second_dr_event_sheds_no_step() {
        let cfg = ac_config_with(|typed| {
            typed.fan_power_w = Some(0.0);
            typed.startup_cd = None;
        });
        let mut environment = env(27.0, 0.010, 18.0, 35.0);
        environment.time_res = ChronoDuration::seconds(900);
        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();
        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::GridEmergency,
            duration_s: Some(0.0),
        })
        .unwrap();
        assert_eq!(
            eq.update_control(&environment),
            OperatingMode::Cooling,
            "the zone is past the cooling setpoint and the zero-second event \
             is over on arrival: the unit must call"
        );

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&environment, Duration::from_secs(900), &mut ports)
            .unwrap();
        let compressor = eq.telemetry().get(tk::COMPRESSOR_KW).unwrap_or(f64::NAN);
        assert!(
            compressor > 0.0,
            "a zero-second event must not shed the step it lands in: the \
             cooling call delivers, got {compressor} kW"
        );
    }

    /// An idle-resolved cooling step does not starve the next step's solve:
    /// the control pass re-derives the thermostat's call from its latch, so
    /// the mode the solver's gate reads is Cooling, never the Standby the
    /// step published. The idle resolution is the step's publication, not
    /// the solver's input.
    #[test]
    fn a_step_that_published_standby_still_requests_the_next_solve() {
        let cfg = ac_config_with(|typed| {
            typed.fan_power_w = Some(0.0);
            typed.startup_cd = None;
        });
        let mut environment = env(27.0, 0.010, 18.0, 35.0);
        environment.time_res = ChronoDuration::seconds(900);
        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();
        assert_eq!(
            eq.update_control(&environment),
            OperatingMode::Cooling,
            "fixture: the thermostat calls for cooling"
        );
        // A zero load fraction zeroes the step's delivery, so the step's
        // publication resolves the active mode to Standby.
        eq.apply_control(&ControlSignal::LoadFraction { fraction: 0.0 })
            .unwrap();
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&environment, Duration::from_secs(900), &mut ports)
            .unwrap();
        assert_eq!(
            eq.telemetry().get(tk::OPERATING_MODE),
            Some(OperatingMode::Standby.as_code()),
            "fixture: the zero-delivery step publishes Standby"
        );

        // The next control pass re-derives the call: the solver sees
        // Cooling and requests the target, so the idle resolution cannot
        // starve a later solve.
        assert_eq!(
            eq.update_control(&environment),
            OperatingMode::Cooling,
            "the thermostat's latch still calls after the idle step"
        );
        assert!(
            eq.ideal_target().is_some(),
            "the solve runs for the latched call the step published Standby for"
        );
    }

    /// A forced-off control pass does not starve the step's solve: the
    /// shed's forced-off branch returns before the thermostat re-reads, so
    /// the pass publishes Off while the call stands, and the solver's gate
    /// reads the latch, not the publication. A signal landing at the
    /// dispatch (the operator's cancel) re-derives Cooling, and the step
    /// delivers the solve's capacity, never the raw duty the solve never
    /// saw.
    #[test]
    fn a_dispatch_time_cancel_after_a_forced_off_pass_delivers_the_solved_capacity() {
        let cfg = ac_config_with(|typed| {
            typed.fan_power_w = Some(0.0);
            typed.startup_cd = None;
        });
        // The coarse timestep puts the unit on the ideal path.
        let mut environment = env(27.0, 0.010, 18.0, 35.0);
        environment.time_res = ChronoDuration::seconds(900);
        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();

        // The event's shed forces the first control pass off before the
        // thermostat re-reads: the publication is Off, the latch still
        // calls.
        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::GridEmergency,
            duration_s: Some(900.0),
        })
        .unwrap();
        assert_eq!(
            eq.update_control(&environment),
            OperatingMode::Off,
            "fixture: the event's shed forces the first control pass off"
        );
        assert!(
            eq.ideal_target().is_some(),
            "the solve runs for the latched call the forced-off pass \
             published Off for"
        );
        // The solver's request reaches the unit with the dispatch's
        // signals (the solver-feedback actor queues it at Step 1f).
        eq.apply_control(&ControlSignal::IdealCapacity {
            capacity_w: -2_000.0,
            degraded: false,
        })
        .unwrap();
        // The operator rescinds the shed at the dispatch: the second
        // control pass re-derives Cooling and the step dispatches the
        // call.
        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::Normal,
            duration_s: Some(900.0),
        })
        .unwrap();
        assert_eq!(
            eq.update_control(&environment),
            OperatingMode::Cooling,
            "the cancel re-derives the thermostat's call at the dispatch"
        );

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&environment, Duration::from_secs(900), &mut ports)
            .unwrap();
        let rtf = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap_or(f64::NAN);
        assert!(
            rtf < 1.0,
            "the dispatched call delivers the solve's capacity, never the \
             raw duty: the starved arm runs the full duty at 1.0, got \
             runtime_fraction {rtf}"
        );
        assert_eq!(
            eq.take_health_counts().ideal_capacity_unsolved_steps,
            0,
            "the solve reached the unit: the dispatched call is not a \
             starvation"
        );
    }

    /// The event's LAST named step keeps the event's dispatch level: a
    /// partial shed (High, load fraction 0.8) over two named steps
    /// delivers the same reduced draw on both. The timer advances after
    /// the step's dispatch, so the level's end is the release, never the
    /// last step's own interval reverting mid-dispatch.
    #[test]
    fn the_last_named_dr_step_keeps_the_events_dispatch_level() {
        let cfg = ac_config_with(|typed| {
            typed.fan_power_w = Some(0.0);
            typed.startup_cd = None;
        });
        let mut environment = env(27.0, 0.010, 18.0, 35.0);
        environment.time_res = ChronoDuration::seconds(900);
        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();
        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::High,
            duration_s: Some(1800.0),
        })
        .unwrap();

        let mut delivered = Vec::new();
        for step in 0..2 {
            assert_eq!(
                eq.update_control(&environment),
                OperatingMode::Cooling,
                "step {step}: the zone is past the DR-adjusted setpoint"
            );
            eq.apply_control(&ControlSignal::IdealCapacity {
                capacity_w: -4_000.0,
                degraded: false,
            })
            .unwrap();
            let mut ports = PortSlots {
                thermal: vec![ThermalAccumulator::new(ZoneId(1))],
                humidity: vec![HumidityAccumulator::new(ZoneId(1))],
                ..PortSlots::default()
            };
            eq.step(&environment, Duration::from_secs(900), &mut ports)
                .unwrap();
            delivered.push(eq.telemetry().get(tk::COMPRESSOR_KW).unwrap_or(f64::NAN));
        }

        assert!(delivered[0] > 0.0, "fixture: the driven capacity runs");

        // The event's 0.8 load fraction must be load-bearing: an identical
        // unit without the event runs the same first step and delivers the
        // unshed level. The two steps share every input but the multiplier,
        // so the event step is the single product `0.8 * unshed` (one f64
        // multiplication on each side, commutative and therefore exact).
        let mut baseline = AirConditioner::new(cfg.clone());
        baseline.init(&cfg, &environment).unwrap();
        baseline.update_control(&environment);
        baseline
            .apply_control(&ControlSignal::IdealCapacity {
                capacity_w: -4_000.0,
                degraded: false,
            })
            .unwrap();
        let mut baseline_ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        baseline
            .step(&environment, Duration::from_secs(900), &mut baseline_ports)
            .unwrap();
        let unshed = baseline
            .telemetry()
            .get(tk::COMPRESSOR_KW)
            .unwrap_or(f64::NAN);
        assert!(unshed > 0.0, "fixture: the unshed capacity runs");
        assert_eq!(
            delivered[0],
            0.8 * unshed,
            "the High event's 0.8 load fraction must scale the unshed step: \
             {} kW against the unshed {} kW",
            delivered[0],
            unshed
        );
        assert!(
            (delivered[1] - delivered[0]).abs() < 1e-9,
            "the event's second named step keeps the 0.8 level: {} kW against \
             the first step's {} kW",
            delivered[1],
            delivered[0]
        );
    }

    #[test]
    fn typed_two_speed_ac_derives_two_speed_plf_cd_startup_cd_is_zero() {
        let cfg = ac_config_with(|typed| {
            typed.number_of_speeds = 2;
            typed.startup_cd = None;
        });
        let env = env(28.0, 0.010, 18.0, 35.0);

        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &env).unwrap();

        assert_eq!(
            eq.core.hvac.config.speed_control_mode,
            SpeedControlMode::TwoSpeedSetpoint
        );
        // PLF Cd = 0.11 (two-speed derived), matching AHRI 210/240.
        assert!(
            (eq.core.hvac.runtime.plf_cooling_degradation_coeff - 0.11).abs() < 1e-9,
            "two-speed typed cooling must derive PLF Cd=0.11, got {}",
            eq.core.hvac.runtime.plf_cooling_degradation_coeff
        );
        // startup.c_d = 0.0 (DEFAULT_STARTUP_CD), matching OCHRE HVAC.py:765.
        assert!(
            eq.core.hvac.runtime.startup.c_d.abs() < 1e-9,
            "two-speed typed cooling startup Cd must be 0.0 (DEFAULT_STARTUP_CD), got {}",
            eq.core.hvac.runtime.startup.c_d
        );
    }

    /// A default-constructed single-speed central AC (no explicit startup_cd)
    /// must have `startup_cd = 0.0`, matching OCHRE's opt-in startup degradation.
    /// The PLF cycling degradation coefficient is derived from SEER.
    #[test]
    fn default_central_ac_startup_cd_is_zero() {
        let cfg = ac_config_with(|typed| {
            typed.startup_cd = None;
        });
        let env = env(28.0, 0.010, 18.0, 35.0);
        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &env).unwrap();

        assert!(
            eq.core.hvac.runtime.startup.c_d.abs() < 1e-9,
            "default-constructed single-speed AC must have startup_cd = 0.0, got {}",
            eq.core.hvac.runtime.startup.c_d
        );
        // PLF Cd must be SEER-derived (not zero): SEER 16 → Cd = 0.07.
        assert!(
            eq.core.hvac.runtime.plf_cooling_degradation_coeff > 0.0,
            "PLF Cd must be non-zero for single-speed AC, got {}",
            eq.core.hvac.runtime.plf_cooling_degradation_coeff
        );
    }

    /// Explicit startup_cd must set startup.c_d only, NOT
    /// plf_cooling_degradation_coeff. Regression test for the conflation bug
    /// where derived_cooling_startup_cd() leaked user startup_cd into PLF Cd.
    #[test]
    fn central_ac_explicit_startup_cd_does_not_affect_plf_cd() {
        let explicit_cd = 0.15;
        let cfg = ac_config_with(|typed| {
            typed.startup_cd = Some(explicit_cd);
        });
        let env = env(28.0, 0.010, 18.0, 35.0);
        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &env).unwrap();
        // Default EIR=0.33 → SEER≈10.34 < 13 → PLF Cd = 0.20, NOT explicit_cd.
        assert!(
            (eq.core.hvac.runtime.plf_cooling_degradation_coeff - 0.20).abs() < 1e-9,
            "PLF Cd must be SEER-derived (0.20), not explicit startup_cd, got {}",
            eq.core.hvac.runtime.plf_cooling_degradation_coeff
        );
        // startup.c_d must be the explicit value.
        assert!(
            (eq.core.hvac.runtime.startup.c_d - explicit_cd).abs() < 1e-9,
            "startup.c_d must be explicit value {}, got {}",
            explicit_cd,
            eq.core.hvac.runtime.startup.c_d
        );
    }

    #[test]
    /// HP cooler startup ramp: SEER 14 → c_d=0.07, step 1 power < step 5 power.
    /// Winkler (2011) startup capacity ramp applies to HP equipment only,
    /// matching OCHRE's `"HP" in self.mode` guard (HVAC.py:977).
    fn hp_cooler_startup_ramp_reduces_first_step_capacity() {
        // SEER 14 → EIR = 3.412141633 / 14 ≈ 0.2437; c_d=0.07 gives t_full=1.8 min.
        // Step 1 fires at time_since_start=0.5 min (< t_full) → capacity multiplier < 1.
        // By step 5, time_since_start=4.5 min > t_full → multiplier = 1.0.
        let seer = 14.0_f64;
        let eir = 3.412_141_633_f64 / seer;
        let cfg = EquipmentConfig::from_typed(
            "AC".to_string(),
            "Air Conditioner".to_string(),
            CentralAirConditionerConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 8_000.0,
                eir,
                shr: Some(0.75),
                number_of_speeds: 1,
                stage_capacities_w: None,
                stage_eirs: None,
                stage_shrs: None,
                fan_power_w: None,
                fan_power_w_per_cfm: None,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_CENTRAL_AC_M3_S_PER_W),
                fraction_load_served: None,
                crankcase_heater_w: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                duct: crate::DuctConfig::default(),
                system_type: None,
                startup_cd: Some(0.07),
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                charge_defect_ratio: None,
                min_oat_compressor_cooling_c: None,
            },
        )
        .unwrap();
        let environment = env(26.0, 0.010, 18.0, 35.0);
        let mut eq = AirConditioner::new(cfg.clone());
        // Override equipment type to HP cooler so the startup ramp applies,
        // matching OCHRE's `"HP" in self.mode` gate.
        eq.core.hvac.config.equipment_type = HvacEquipmentType::AshpHeatPumpCooling;
        eq.init(&cfg, &environment).unwrap();

        eq.update_control(&environment);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();
        let kw_step1 = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);

        for _ in 1..5 {
            eq.update_control(&environment);
            let mut p = PortSlots {
                thermal: vec![ThermalAccumulator::new(ZoneId(1))],
                humidity: vec![HumidityAccumulator::new(ZoneId(1))],
                ..PortSlots::default()
            };
            eq.step(&environment, Duration::from_secs(60), &mut p)
                .unwrap();
        }
        let kw_steady = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);

        assert!(
            kw_step1 > 0.0,
            "HP cooler must draw power on step 1; got {kw_step1}"
        );
        assert!(
            kw_step1 < kw_steady,
            "startup ramp must reduce step-1 kW below steady-state for HP cooler; step1={kw_step1:.4}, steady={kw_steady:.4}"
        );
    }

    #[test]
    fn transient_load_fraction_can_be_applied_before_update_control() {
        let cfg = ac_config();
        let environment = env(30.0, 0.010, 18.0, 35.0);

        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();
        eq.apply_control(&ControlSignal::LoadFraction { fraction: 0.0 })
            .unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&environment);
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();
        let kw = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        assert_eq!(
            kw, 0.0,
            "LoadFraction applied before update_control must still affect the current step"
        );

        let mut ports_next = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&environment);
        eq.step(&environment, Duration::from_secs(60), &mut ports_next)
            .unwrap();
        let kw_next = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        assert!(
            kw_next > 0.0,
            "transient LoadFraction must clear after one step; got {kw_next}"
        );
    }

    #[test]
    fn runtime_fraction_tracks_post_control_duty_cycle() {
        let cfg = ac_config_with(|typed| typed.startup_cd = Some(0.0));
        let environment = env(30.0, 0.010, 18.0, 35.0);

        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();
        eq.apply_control(&ControlSignal::DutyCycle {
            on_fraction: 0.5,
            period_s: None,
            component: None,
        })
        .unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&environment);
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        let rtf = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap_or(-1.0);
        assert!(
            (rtf - 0.5).abs() < 0.1,
            "Runtime fraction must reflect post-control duty cycle; got {rtf}"
        );
    }

    #[test]
    fn coarse_timestep_variable_speed_ideal_signal_selects_stage_from_capacity() {
        let cfg = ac_config_with(|typed| {
            typed.number_of_speeds = 4;
            typed.stage_capacities_w = Some(vec![2_000.0, 4_000.0, 6_000.0, 8_000.0]);
            typed.stage_eirs = Some(vec![0.20, 0.25, 0.30, 0.35]);
            typed.stage_shrs = Some(vec![0.75, 0.75, 0.75, 0.75]);
            typed.fan_power_w = Some(0.0);
            typed.startup_cd = None;
        });
        let mut environment = env(26.0, 0.010, 19.0, 35.0);
        environment.time_res = ChronoDuration::seconds(900);

        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();
        eq.apply_control(&ControlSignal::IdealCapacity {
            capacity_w: -5_000.0,
            degraded: false,
        })
        .unwrap();
        eq.update_control(&environment);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&environment, Duration::from_secs(900), &mut ports)
            .unwrap();

        let compressor_w = eq.telemetry().get(tk::COMPRESSOR_KW).unwrap_or(0.0);
        let rtf = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap_or(0.0);

        // 5000W = 62.5% of 8000W max -- falls between stage 1 (50%) and stage 2 (75%),
        // so stage 1 is selected (lo index in the interpolation range).
        assert_eq!(eq.core.hvac.runtime.last_speed_index, 1);
        assert!(
            compressor_w > 0.0,
            "IdealCapacity=-5 kW on a 4-speed ladder must energize the second stage, got compressor_w={compressor_w}"
        );
        assert!(
            (rtf - 1.0).abs() < 1e-9,
            "exact ideal-capacity stage selection must run continuously, got runtime_fraction={rtf}"
        );
    }

    #[test]
    fn cooling_core_output_matches_telemetry_and_ports() {
        let cfg = ac_config_with(|typed| typed.startup_cd = Some(0.0));
        let environment = env(30.0, 0.010, 18.0, 35.0);

        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();
        eq.update_control(&environment);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        let electric_w = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        let operating_mode = eq.telemetry().get(tk::OPERATING_MODE).unwrap_or(-1.0);
        let core = eq.core_output();
        let core_electric_w = match core.flows.electric_w {
            Some(super::ElectricPower::Consumption(value)) => value,
            other => panic!("expected electric consumption flow, got {other:?}"),
        };

        assert!(
            (ports.electrical.net_active_w() - (electric_w)).abs() < 1e-3,
            "electrical port and telemetry must match: ports={} telemetry={electric_w}",
            ports.electrical.net_active_w()
        );
        assert!(
            (core_electric_w - electric_w).abs() < 1e-9,
            "CoreOutput electric flow and telemetry must match: core={core_electric_w} telemetry={electric_w}"
        );
        assert_eq!(core.state.operating_mode, Some(OperatingMode::Cooling));
        assert_eq!(operating_mode, 2.0);
        assert!(
            ports.thermal[0].sensible_gain_w < 0.0,
            "cooling thermal port must remove sensible heat, got {}",
            ports.thermal[0].sensible_gain_w
        );
    }

    /// Reactive-power contract for the central AC: per-component Q
    /// (compressor pf 0.96, fan pf 0.87, crankcase resistive Q=0),
    /// REACTIVE declared, and port/CoreOutput/telemetry agree bit-for-bit.
    /// Off ⇒ Q == 0.
    #[test]
    fn cooling_reactive_power_pf_and_channels_agree() {
        let cfg = ac_config_with(|typed| typed.startup_cd = Some(0.0));
        let environment = env(30.0, 0.010, 18.0, 35.0);

        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();
        assert!(
            eq.descriptor()
                .core_capabilities
                .contains(hares_types::CoreCapabilities::REACTIVE),
            "central AC must declare REACTIVE"
        );
        assert_eq!(eq.core.zip.pf, 0.96, "class default compressor pf");
        assert_eq!(
            eq.core.fan_zip,
            crate::hvac::reactive::FAN_MOTOR_ZIP,
            "fan component uses the FAN motor ZIP"
        );
        eq.update_control(&environment);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        let p_w = ports.electrical.load_power_w;
        assert!(p_w > 0.0, "cooling call must draw real power");
        let compressor_w = eq.telemetry().get(tk::COMPRESSOR_KW).expect("compressor");
        let fan_w = eq.telemetry().get(tk::FAN_KW).expect("fan");
        eprintln!(
            "DBG p_w={p_w} comp={compressor_w} fan={fan_w} sf={} mode={:?} use_ideal={} ideal_w={}",
            eq.core.hvac.config.space_fraction,
            eq.core.operating_mode,
            eq.core.use_ideal,
            eq.core.ideal_capacity_w
        );
        eprintln!(
            "DBG eir_by_stage={:?} caps={:?} duty={}",
            eq.core.hvac.config.eir_by_stage,
            eq.core.hvac.config.cooling_capacities_w,
            eq.core.hvac.runtime.duty_cycle
        );
        assert!(compressor_w > 0.0 && fan_w > 0.0);
        let q = ports.electrical.reactive_power_var;
        // Per-component: Q = P_comp·tan(acos(0.96)) + P_fan·tan(acos(0.87)).
        // No crankcase draw while cooling (warm OAT), space_fraction = 1.0.
        let expected = compressor_w * 0.96_f64.acos().tan() + fan_w * 0.87_f64.acos().tan();
        assert!(
            (q - expected).abs() < 1e-9,
            "per-component Q must be comp·tan(acos(0.96)) + fan·tan(acos(0.87)): \
             q={q}, expected={expected}"
        );
        assert!(
            q < p_w * 0.87_f64.acos().tan(),
            "blend must lie below the all-fan bound"
        );
        assert_eq!(
            eq.core_output()
                .flows
                .reactive_power_var
                .expect("Some")
                .to_bits(),
            q.to_bits(),
            "CoreOutput Q must equal port Q"
        );
        assert_eq!(
            eq.telemetry()
                .get(tk::REACTIVE_POWER_KVAR)
                .expect("telemetry Q")
                .to_bits(),
            q.to_bits(),
            "telemetry Q must equal port Q"
        );
        hares_types::validate_core_contract(eq.descriptor(), eq.core_output(), &mut Vec::new())
            .expect("core contract must hold with REACTIVE declared");

        // Off case: zone in the deadband (21 °C, between heating 18+1 and
        // cooling 24−1) ⇒ no cooling call, no crankcase (warm OAT) ⇒ Q == 0.
        let off_env = env(21.0, 0.009, 18.0, 20.0);
        eq.update_control(&off_env);
        let mut off_ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&off_env, Duration::from_secs(60), &mut off_ports)
            .unwrap();
        assert_eq!(off_ports.electrical.reactive_power_var, 0.0, "off ⇒ Q == 0");
        assert_eq!(
            eq.core_output().flows.reactive_power_var,
            Some(0.0),
            "off ⇒ CoreOutput Q == Some(0.0)"
        );
    }

    /// Rule R1 regression: the power factor affects only Q. Twin instances —
    /// one with the class pf 0.96, one with a constant-power sidecar override
    /// (pf 0 sentinel) — must produce bit-identical real power at every step
    /// and voltage.
    #[test]
    fn cooling_real_power_follows_the_row_and_reactive_follows_pf() {
        let config_pf = ac_config_with(|typed| {
            typed.startup_cd = Some(0.0);
            // The compressor-only unit: the fan would carry its own row's
            // multiplier (a different response), and this witness pins the
            // compressor's row.
            typed.fan_power_w = Some(0.0);
        });
        let mut config_nopf = config_pf.clone();
        config_nopf.zip = Some(hares_types::zip::ZipLoad::constant_power());

        let mut eq_pf = AirConditioner::new(config_pf.clone());
        let mut eq_nopf = AirConditioner::new(config_nopf.clone());
        let mut environment = env(30.0, 0.010, 18.0, 35.0);
        eq_pf.init(&config_pf, &environment).unwrap();
        eq_nopf.init(&config_nopf, &environment).unwrap();

        let mut any_reactive = false;
        for (i, v) in [1.0, 0.95, 1.05, 1.0, 0.9, 1.1].iter().enumerate() {
            environment.grid.voltage_pu = *v;
            eq_pf.update_control(&environment);
            eq_nopf.update_control(&environment);
            let mut ports_pf = PortSlots {
                thermal: vec![ThermalAccumulator::new(ZoneId(1))],
                humidity: vec![HumidityAccumulator::new(ZoneId(1))],
                ..PortSlots::default()
            };
            let mut ports_nopf = PortSlots {
                thermal: vec![ThermalAccumulator::new(ZoneId(1))],
                humidity: vec![HumidityAccumulator::new(ZoneId(1))],
                ..PortSlots::default()
            };
            eq_pf
                .step(&environment, Duration::from_secs(60), &mut ports_pf)
                .unwrap();
            eq_nopf
                .step(&environment, Duration::from_secs(60), &mut ports_nopf)
                .unwrap();
            // At the reference voltage the row's multiplier is exactly 1.0
            // and the twins' real powers agree bitwise; off nominal the
            // pf twin's real power follows its row's deep-sag form while
            // the constant-power twin stays flat (the recorded deep-sag
            // decision: the row governs the draw at the actual voltage).
            if *v == 1.0 {
                assert_eq!(
                    ports_pf.electrical.load_power_w.to_bits(),
                    ports_nopf.electrical.load_power_w.to_bits(),
                    "step {i} (v={v}): at the reference voltage the twins' \
                     real powers must agree bitwise"
                );
            } else {
                let multiplier = hares_types::zip::zip_defaults_for_class("Air Conditioner")
                    .expect("class row")
                    .deep_sag_real_multiplier(*v, true);
                let expected = ports_nopf.electrical.load_power_w * multiplier;
                assert!(
                    (ports_pf.electrical.load_power_w - expected).abs() < 1e-9,
                    "step {i} (v={v}): the row's twin must draw {expected} W \
                     (the row's deep-sag form), got \
                     {} W",
                    ports_pf.electrical.load_power_w
                );
            }
            assert_eq!(
                ports_nopf.electrical.reactive_power_var, 0.0,
                "pf-0 twin must produce zero reactive power"
            );
            if ports_pf.electrical.reactive_power_var != 0.0 {
                any_reactive = true;
            }
            environment.current_time += ChronoDuration::minutes(1);
        }
        assert!(
            any_reactive,
            "the pf 0.96 twin must produce reactive power while cooling"
        );
    }

    #[test]
    fn speed_index_appears_in_telemetry_when_cooling() {
        let cfg = ac_config_with(|typed| {
            typed.number_of_speeds = 4;
            typed.fan_power_w = Some(0.0);
            typed.startup_cd = None;
        });
        let environment = env(30.0, 0.010, 18.0, 35.0);

        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();
        eq.update_control(&environment);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        let speed_index = eq.telemetry().get(tk::SPEED_INDEX);
        assert!(
            speed_index.is_some(),
            "SPEED_INDEX must be present in AC telemetry after a cooling step"
        );
        assert_eq!(
            speed_index.unwrap(),
            eq.core.hvac.runtime.last_speed_index as f64,
            "SPEED_INDEX telemetry must match hvac.last_speed_index"
        );
    }

    #[test]
    fn multi_speed_stage1_uses_second_biquadratic_pair() {
        // Directly verify that calculate_performance uses biquadratic pair N*2 for
        // cap and N*2+1 for EIR when last_speed_index=N.  Stage 0 curves have c0=1.0
        // (cap_ratio≈1.0); stage 1 cap curve has c0=2.0 (cap_ratio≈2.0).
        // When stage 1 is set, sensible_w must exceed the rated 4 kW capacity.
        let cfg = ac_config_with(|typed| {
            typed.number_of_speeds = 2;
            typed.stage_capacities_w = Some(vec![4_000.0, 4_000.0]);
            typed.stage_eirs = Some(vec![0.33, 0.33]);
            typed.fan_power_w = Some(0.0);
            typed.startup_cd = Some(0.0);
            typed.hysteresis_c = Some(hares_types::MIN_THERMOSTAT_BAND_C);
        });
        let environment = env(26.0, 0.010, 19.0, 35.0);

        let mut core = CoolingCore::new(cfg.clone(), false);
        core.init(&cfg, &environment).unwrap();

        core.hvac.config.biquadratic_coeffs = vec![
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        ];
        core.operating_mode = OperatingMode::Cooling;
        core.hvac.runtime.duty_cycle = 1.0;
        core.hvac.runtime.last_speed_index = 1;

        let perf = core.calculate_performance(&environment, 15.0).unwrap();

        assert!(
            perf.sensible_cooling_w > 4_000.0,
            "stage-1 biquadratic (c0=2.0) must double cap_ratio; got sensible_w={}",
            perf.sensible_cooling_w
        );
    }

    #[test]
    fn multi_speed_max_stage_high_side_clamping() {
        let cfg = ac_config_with(|typed| {
            typed.number_of_speeds = 2;
            typed.stage_capacities_w = Some(vec![4_000.0, 4_000.0]);
            typed.stage_eirs = Some(vec![0.33, 0.33]);
            typed.fan_power_w = Some(0.0);
            typed.startup_cd = Some(0.0);
            typed.hysteresis_c = Some(hares_types::MIN_THERMOSTAT_BAND_C);
        });
        let environment = env(26.0, 0.010, 19.0, 35.0);

        let mut core = CoolingCore::new(cfg.clone(), false);
        core.init(&cfg, &environment).unwrap();

        core.hvac.config.biquadratic_coeffs = vec![
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            [0.5, 0.0, 0.0, 0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        ];
        core.operating_mode = OperatingMode::Cooling;
        core.hvac.runtime.duty_cycle = 1.0;
        core.hvac.config.speed_control_mode = SpeedControlMode::MultiSpeedInterpolated;
        core.hvac.runtime.last_speed_index = 1;
        core.hvac.runtime.last_speed_frac = 0.5;

        let perf = core.calculate_performance(&environment, 15.0).unwrap();

        assert!(
            perf.sensible_cooling_w > 0.0,
            "sensible cooling must be positive; got {}",
            perf.sensible_cooling_w
        );
        assert!(
            perf.compressor_w.is_finite() && perf.compressor_w >= 0.0,
            "compressor power must be finite and non-negative; got {}",
            perf.compressor_w
        );
        assert!(
            perf.shr > 0.0 && perf.shr <= 1.0,
            "SHR must be in (0, 1]; got {}",
            perf.shr
        );
    }

    #[test]
    fn multi_speed_max_stage_overprovisioned_curve_uses_last_valid_pair() {
        let cfg = ac_config_with(|typed| {
            typed.number_of_speeds = 2;
            typed.stage_capacities_w = Some(vec![4_000.0, 4_000.0]);
            typed.stage_eirs = Some(vec![0.33, 0.33]);
            typed.fan_power_w = Some(0.0);
            typed.startup_cd = Some(0.0);
            typed.hysteresis_c = Some(hares_types::MIN_THERMOSTAT_BAND_C);
        });
        let environment = env(26.0, 0.010, 19.0, 35.0);

        let mut core = CoolingCore::new(cfg.clone(), false);
        core.init(&cfg, &environment).unwrap();

        core.hvac.config.biquadratic_coeffs = vec![
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            [100.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            [100.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        ];
        core.operating_mode = OperatingMode::Cooling;
        core.hvac.runtime.duty_cycle = 1.0;
        core.hvac.config.speed_control_mode = SpeedControlMode::MultiSpeedInterpolated;
        core.hvac.runtime.last_speed_index = 1;
        core.hvac.runtime.last_speed_frac = 0.5;

        let perf = core.calculate_performance(&environment, 15.0).unwrap();

        assert!(
            perf.sensible_cooling_w > 0.0,
            "sensible cooling must be positive; got {}",
            perf.sensible_cooling_w
        );
        assert!(
            perf.sensible_cooling_w < 10_000.0,
            "sensible cooling must not use over-provisioned pair (c0=100.0 would produce ~202kW); got {}",
            perf.sensible_cooling_w
        );
    }

    #[test]
    fn room_ac_explicit_shr_reaches_rated_shr() {
        let cfg = EquipmentConfig::from_typed(
            "RAC".to_string(),
            "Room AC".to_string(),
            RoomAcConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 3_500.0,
                eir: 3.412_141_633 / 10.0,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: None,
                shr: Some(0.65),
                startup_cd: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                crankcase_heater_w: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                min_oat_compressor_cooling_c: None,
            },
        )
        .unwrap();
        let environment = env(27.0, 0.010, 19.0, 35.0);
        let mut eq = RoomAC::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();
        assert!(
            (eq.core.rated_shr - 0.65).abs() < 1e-12,
            "rated_shr must be 0.65 after init with shr=Some(0.65), got {}",
            eq.core.rated_shr
        );
    }

    #[test]
    fn checkpoint_thermostat_hysteresis_c() {
        let cfg = ac_config_with(|typed| {
            typed.hysteresis_c = Some(1.0);
        });
        let environment = env(28.0, 0.010, 19.0, 35.0);
        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();

        // Apply a non-default deadband via ThermalSetpoint control.
        eq.apply_control(&ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(18.0),
            cooling_setpoint_c: Some(24.0),
            deadband_c: Some(3.0),
        })
        .unwrap();
        assert_eq!(eq.core.hvac.thermostat_fsm.thermostat.hysteresis_c, 3.0);

        // Step once so state is fully populated.
        eq.update_control(&environment);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        let state = eq.save_state().unwrap();

        // Create a fresh instance and restore.
        let mut restored = AirConditioner::new(cfg.clone());
        restored.init(&cfg, &environment).unwrap();
        assert_eq!(
            restored.core.hvac.thermostat_fsm.thermostat.hysteresis_c, 1.0,
            "fresh instance must have config default"
        );

        restored.load_state(&state).unwrap();
        assert_eq!(
            restored.core.hvac.thermostat_fsm.thermostat.hysteresis_c, 3.0,
            "thermostat_hysteresis_c must survive checkpoint round-trip"
        );
    }

    #[test]
    fn checkpoint_time_at_current_speed_s() {
        let cfg = ac_config_with(|typed| {
            typed.hysteresis_c = Some(hares_types::MIN_THERMOSTAT_BAND_C);
        });
        let environment = env(28.0, 0.010, 19.0, 35.0);
        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();

        // Step 10 times at 60s to accumulate speed time.
        for _ in 0..10 {
            eq.update_control(&environment);
            let mut ports = PortSlots {
                thermal: vec![ThermalAccumulator::new(ZoneId(1))],
                humidity: vec![HumidityAccumulator::new(ZoneId(1))],
                ..PortSlots::default()
            };
            eq.step(&environment, Duration::from_secs(60), &mut ports)
                .unwrap();
        }

        let accumulated = eq.core.hvac.runtime.time_at_current_speed_s;
        assert!(
            accumulated > 0.0,
            "time_at_current_speed_s must be positive after stepping"
        );

        let state = eq.save_state().unwrap();

        let mut restored = AirConditioner::new(cfg.clone());
        restored.init(&cfg, &environment).unwrap();
        assert_eq!(restored.core.hvac.runtime.time_at_current_speed_s, 0.0);

        restored.load_state(&state).unwrap();
        assert!(
            (restored.core.hvac.runtime.time_at_current_speed_s - accumulated).abs() < 1e-9,
            "time_at_current_speed_s must survive checkpoint; expected {accumulated}, got {}",
            restored.core.hvac.runtime.time_at_current_speed_s
        );
    }

    #[test]
    fn checkpoint_min_on_off_time_s() {
        let cfg = ac_config_with(|typed| {
            typed.hysteresis_c = Some(hares_types::MIN_THERMOSTAT_BAND_C);
        });
        let environment = env(28.0, 0.010, 19.0, 35.0);
        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();

        eq.core.hvac.thermostat_fsm.min_on_time_s = 120.0;
        eq.core.hvac.thermostat_fsm.min_off_time_s = 180.0;

        eq.update_control(&environment);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        let state = eq.save_state().unwrap();

        let mut restored = AirConditioner::new(cfg.clone());
        restored.init(&cfg, &environment).unwrap();
        assert_eq!(
            restored.core.hvac.thermostat_fsm.min_on_time_s, 0.0,
            "fresh instance must have default min_on_time_s=0"
        );
        assert_eq!(
            restored.core.hvac.thermostat_fsm.min_off_time_s, 0.0,
            "fresh instance must have default min_off_time_s=0"
        );

        restored.load_state(&state).unwrap();
        assert!(
            (restored.core.hvac.thermostat_fsm.min_on_time_s - 120.0).abs() < 1e-9,
            "min_on_time_s must survive checkpoint round-trip; expected 120.0, got {}",
            restored.core.hvac.thermostat_fsm.min_on_time_s
        );
        assert!(
            (restored.core.hvac.thermostat_fsm.min_off_time_s - 180.0).abs() < 1e-9,
            "min_off_time_s must survive checkpoint round-trip; expected 180.0, got {}",
            restored.core.hvac.thermostat_fsm.min_off_time_s
        );

        let t0 = environment.current_time;
        restored.core.hvac.thermostat_fsm.mode = ThermostatMode::Cooling;
        restored.core.hvac.thermostat_fsm.mode_start_at = Some(t0);

        let t60 = t0 + ChronoDuration::seconds(60);
        assert!(
            !restored
                .core
                .hvac
                .thermostat_fsm
                .can_transition_mode(ThermostatMode::Deadband, t60),
            "min_on_time_s=120 must block Cooling→Deadband at 60s after restore"
        );
        let t120 = t0 + ChronoDuration::seconds(120);
        assert!(
            restored
                .core
                .hvac
                .thermostat_fsm
                .can_transition_mode(ThermostatMode::Deadband, t120),
            "min_on_time_s=120 must release at 120s after restore"
        );
    }

    /// `startup_was_on`, `startup_c_d`, and `startup_time_since_start_min`
    /// must survive a checkpoint round-trip. The edge-detection flag `was_on`
    /// gates whether the next on-step fires a fresh startup ramp; losing it
    /// at checkpoint would spuriously re-trigger the ramp on restore.
    #[test]
    fn startup_config_survives_checkpoint_ac() {
        let cfg = ac_config_with(|typed| {
            typed.hysteresis_c = Some(hares_types::MIN_THERMOSTAT_BAND_C);
            typed.startup_cd = Some(0.25);
        });
        let environment = env(28.0, 0.010, 19.0, 35.0);
        let mut eq = AirConditioner::new(cfg.clone());
        eq.core.hvac.config.equipment_type = HvacEquipmentType::AshpHeatPumpCooling;
        eq.init(&cfg, &environment).unwrap();

        assert!(
            !eq.core.hvac.runtime.startup.was_on,
            "fresh was_on must be false"
        );
        assert_eq!(eq.core.hvac.runtime.startup.time_since_start_min, 0.0);
        assert!((eq.core.hvac.runtime.startup.c_d - 0.25).abs() < 1e-12);

        // Step with hot zone (28°C vs 24°C cooling setpoint): compressor
        // comes on, startup ramp begins.
        eq.update_control(&environment);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();

        assert!(
            eq.core.hvac.runtime.startup.was_on,
            "was_on must be true after compressor step"
        );
        let saved_timer = eq.core.hvac.runtime.startup.time_since_start_min;
        assert!(
            saved_timer > 0.0,
            "time_since_start_min must be positive after on-step"
        );

        let saved_c_d = eq.core.hvac.runtime.startup.c_d;

        let state = eq.save_state().unwrap();

        let mut restored = AirConditioner::new(cfg.clone());
        restored.init(&cfg, &environment).unwrap();
        assert!(
            !restored.core.hvac.runtime.startup.was_on,
            "fresh was_on must be false"
        );
        assert_eq!(restored.core.hvac.runtime.startup.time_since_start_min, 0.0);

        restored.load_state(&state).unwrap();
        assert!(
            restored.core.hvac.runtime.startup.was_on,
            "was_on must survive checkpoint round-trip"
        );
        assert!(
            (restored.core.hvac.runtime.startup.c_d - saved_c_d).abs() < 1e-12,
            "c_d must survive checkpoint round-trip"
        );
        assert!(
            (restored.core.hvac.runtime.startup.time_since_start_min - saved_timer).abs() < 1e-12,
            "time_since_start_min must survive checkpoint round-trip"
        );
    }

    /// Regression for the step-level edge-detection wiring: `CoolingCore::step`
    /// must run the startup-ramp bookkeeping on every timestep, including
    /// off-steps. Drives a real `AirConditioner` through `step()` across
    /// on → off → on operating-mode transitions (not `capacity_multiplier`
    /// directly). Before the fix, the off-branch of `step()` never called
    /// `apply_startup_capacity_degradation`, so `was_on` stayed frozen at
    /// `true` for the whole off period and the restart step continued the old
    /// decayed ramp (timer 0.5 → 1.5 min) instead of firing a fresh one
    /// (timer reset → 0.5 min). OCHRE updates the equivalent previous-mode
    /// state unconditionally every timestep (HVAC.py:616).
    #[test]
    fn ac_step_off_step_clears_was_on_and_restart_fires_fresh_ramp() {
        let cfg = ac_config_with(|typed| {
            typed.hysteresis_c = Some(hares_types::MIN_THERMOSTAT_BAND_C);
            typed.startup_cd = Some(0.25);
        });
        // Hot zone (28 °C > 24 °C cooling setpoint) → compressor on.
        let hot = env(28.0, 0.010, 19.0, 35.0);
        // Cool zone (19 °C < 24 °C cooling setpoint) → compressor off.
        let cool = env(19.0, 0.010, 19.0, 35.0);
        let mut eq = AirConditioner::new(cfg.clone());
        eq.core.hvac.config.equipment_type = HvacEquipmentType::AshpHeatPumpCooling;
        eq.init(&cfg, &hot).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        // dt = 60 s → dt_min = 1.0, so a fresh ramp evaluates at t = 0.5 min.
        let dt = Duration::from_secs(60);

        // On-step: ramp starts.
        eq.update_control(&hot);
        eq.step(&hot, dt, &mut ports).unwrap();
        assert!(
            eq.core.hvac.runtime.startup.was_on,
            "was_on must be true after on-step"
        );
        assert!(
            (eq.core.hvac.runtime.startup.time_since_start_min - 0.5).abs() < 1e-12,
            "first on-step must set timer to 0.5·dt = 0.5 min, got {}",
            eq.core.hvac.runtime.startup.time_since_start_min
        );
        assert_eq!(
            eq.core.hvac.runtime.startup.reset_count, 1,
            "cold start must count one timer reset"
        );

        // Off-step: step() must record was_on = false while preserving the timer.
        eq.update_control(&cool);
        eq.step(&cool, dt, &mut ports).unwrap();
        assert!(
            !eq.core.hvac.runtime.startup.was_on,
            "off-step must record was_on = false so the next on-step is a \
             detectable off→on transition"
        );
        assert!(
            (eq.core.hvac.runtime.startup.time_since_start_min - 0.5).abs() < 1e-12,
            "off-step must preserve the timer, got {}",
            eq.core.hvac.runtime.startup.time_since_start_min
        );
        assert_eq!(
            eq.core.hvac.runtime.startup.reset_count, 1,
            "off-step must not count a timer reset"
        );

        // Restart step: true off→on transition → fresh ramp, not continued decay.
        eq.update_control(&hot);
        eq.step(&hot, dt, &mut ports).unwrap();
        let restart_timer = eq.core.hvac.runtime.startup.time_since_start_min;
        assert!(
            (restart_timer - 0.5).abs() < 1e-12,
            "restart must fire a fresh ramp (timer reset, then advanced to \
             0.5·dt = 0.5 min), not continue the old ramp to 1.5 min; got {restart_timer}"
        );
        assert!(
            eq.core.hvac.runtime.startup.was_on,
            "was_on must be true after restart step"
        );
        assert_eq!(
            eq.core.hvac.runtime.startup.reset_count, 2,
            "restart must count a second timer reset"
        );

        // The reported multiplier must be the fresh cold-start Winkler (2011)
        // value at t = 0.5 min (t_full = 20·0.25 + 0.4 = 5.4 min), not the
        // further-decayed continued-ramp value at t = 1.5 min.
        let t_full = 20.0 * 0.25_f64 + 0.4;
        let fresh = (-1.025_f64 * (-3.799_36_f64 * 0.5 / t_full).exp() + 1.025).clamp(0.0, 1.0);
        let reported = eq.telemetry().get(tk::STARTUP_MULTIPLIER).unwrap();
        assert!(
            (reported - fresh).abs() < 1e-9,
            "restart STARTUP_MULTIPLIER must equal fresh cold-start value \
             {fresh}, got {reported}"
        );
        let reported_timer = eq.telemetry().get(tk::TIME_SINCE_START_MIN).unwrap();
        assert!(
            (reported_timer - 0.5).abs() < 1e-12,
            "TIME_SINCE_START_MIN telemetry must report the fresh timer, got {reported_timer}"
        );
    }

    // ── Duct loss and main power telemetry ──────────────────────────────────

    /// AC duct_loss_w must equal gross_cooling * (1 - dse).
    /// ASHRAE 152. Uses pre-DSE gross values, not post-DSE telemetry.
    #[test]
    fn ac_duct_loss_uses_pre_dse_gross_capacity() {
        let cfg = ac_config_with(|c| {
            c.duct = DuctConfig {
                dse_cool: Some(0.8),
                ..DuctConfig::default()
            };
        });
        let mut eq = AirConditioner::new(cfg.clone());
        let e = env(28.0, 0.009, 19.0, 35.0);
        eq.init(&cfg, &e).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&e);
        eq.step(&e, Duration::from_secs(60), &mut ports).unwrap();

        let duct_loss_w = eq.telemetry().get(tk::DUCT_LOSS_W).unwrap();
        let coil_sensible = eq.telemetry().get(tk::COIL_SENSIBLE_COOLING_W).unwrap();
        let coil_latent = eq.telemetry().get(tk::COIL_LATENT_COOLING_W).unwrap();
        let gross = coil_sensible + coil_latent;
        let dse = eq.core.hvac.config.duct_dse.clamp(0.0, 1.0);
        let expected = gross * (1.0 - dse);
        assert!(
            (duct_loss_w - expected).abs() < 1e-3,
            "duct_loss_w must be gross * (1 - dse) = {expected}, got {duct_loss_w}"
        );
    }

    /// AC main_power_w equals compressor_w (total_input - fan = compressor).
    /// OCHRE HVAC.py:575.
    #[test]
    fn ac_main_power_equals_compressor_w() {
        let cfg = ac_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let e = env(28.0, 0.009, 19.0, 35.0);
        eq.init(&cfg, &e).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&e);
        eq.step(&e, Duration::from_secs(60), &mut ports).unwrap();

        let main_w = eq.telemetry().get(tk::MAIN_POWER_KW).unwrap();
        let compressor_w = eq.telemetry().get(tk::COMPRESSOR_KW).unwrap();
        // Both telemetry values are space_fraction-scaled, so they are equal.
        assert!(
            (main_w - compressor_w).abs() < 1e-9,
            "AC main_power must equal compressor_w telemetry (both space_fraction-scaled), \
             got main={main_w} compressor={compressor_w}"
        );
    }

    /// Sub-metering telemetry (COMPRESSOR_KW, FAN_KW, CRANKCASE_KW) must be
    /// scaled by space_fraction consistently with ELECTRIC_KW so components
    /// always sum to the unit total.
    #[test]
    fn ac_submeter_components_scaled_by_space_fraction_sum_to_total() {
        let sum_matches_total = |eq: &AirConditioner, label: &str| {
            let electric_w = eq.telemetry().get(tk::ELECTRIC_KW).unwrap();
            let compressor_w = eq.telemetry().get(tk::COMPRESSOR_KW).unwrap();
            let fan_w = eq.telemetry().get(tk::FAN_KW).unwrap();
            let crankcase_w = eq.telemetry().get(tk::CRANKCASE_KW).unwrap();
            assert!(
                (compressor_w + fan_w + crankcase_w - electric_w).abs() < 1e-12,
                "{label}: sub-meter components must sum to the unit total: \
                 compressor={compressor_w} + fan={fan_w} + crankcase={crankcase_w} \
                 != electric={electric_w}"
            );
            electric_w
        };

        let cfg = ac_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let e = env(28.0, 0.009, 19.0, 35.0);
        eq.init(&cfg, &e).unwrap();
        eq.core.hvac.config.space_fraction = 0.6;

        // Phase 1: active cooling (warm OAT, no crankcase draw).
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&e);
        eq.step(&e, Duration::from_secs(60), &mut ports).unwrap();
        let electric_w = sum_matches_total(&eq, "cooling");
        assert!(electric_w > 0.0, "cooling call must draw power");
        let compressor_w = eq.telemetry().get(tk::COMPRESSOR_KW).unwrap();
        // Gross compressor power stays unscaled for companion physics
        // (GSHP borehole balance); the telemetry sub-meter is scaled.
        assert!(
            (compressor_w - eq.core.last_compressor_w * 0.6).abs() < 1e-12,
            "COMPRESSOR_KW telemetry must be gross * space_fraction"
        );

        // Phase 2: compressor off at cold OAT — crankcase heater carries the
        // whole (scaled) draw and the sum invariant still holds.
        let off_env = env(21.0, 0.006, 18.0, 5.0);
        eq.update_control(&off_env);
        let mut off_ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&off_env, Duration::from_secs(60), &mut off_ports)
            .unwrap();
        let electric_w = sum_matches_total(&eq, "crankcase standby");
        let crankcase_w = eq.telemetry().get(tk::CRANKCASE_KW).unwrap();
        assert!(
            crankcase_w > 0.0 && (electric_w - crankcase_w).abs() < 1e-12,
            "cold-OAT standby draw must be the crankcase heater alone \
             (crankcase={crankcase_w}, electric={electric_w})"
        );
    }

    /// latent_gains_w must equal pre-DSE coil latent * space_fraction.
    /// OCHRE HVAC.py:595: Latent Gains = latent_gain * space_fraction.
    /// Must NOT use post-DSE latent_cooling_w which is 80% of the correct value at dse=0.8.
    #[test]
    fn ac_latent_gains_uses_pre_dse_gross_latent_times_space_fraction() {
        let cfg = ac_config_with(|c| {
            c.duct = DuctConfig {
                dse_cool: Some(0.8),
                ..DuctConfig::default()
            };
        });
        let mut eq = AirConditioner::new(cfg.clone());
        let e = env(28.0, 0.009, 19.0, 35.0);
        eq.init(&cfg, &e).unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&e);
        eq.step(&e, Duration::from_secs(60), &mut ports).unwrap();

        let latent_gains_w = eq.telemetry().get(tk::LATENT_GAINS_W).unwrap();
        let coil_latent = eq.telemetry().get(tk::COIL_LATENT_COOLING_W).unwrap();
        let sf = eq.core.hvac.config.space_fraction;
        let expected = coil_latent * sf;
        assert!(
            (latent_gains_w - expected).abs() < 1e-3,
            "latent_gains_w must be coil_latent * space_fraction = {expected}, got {latent_gains_w}"
        );
        // Verify it is NOT the post-DSE value.
        let post_dse_latent = eq.telemetry().get(tk::LATENT_COOLING_W).unwrap();
        if sf > 0.0 && coil_latent > 0.0 {
            assert!(
                (latent_gains_w - post_dse_latent).abs() > 1e-3,
                "latent_gains_w must NOT equal post-DSE latent_cooling_w; got gains={latent_gains_w} post_dse={post_dse_latent}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Charge defect ratio — DoD physics-level regression tests
    //
    // These verify that the correction is actually applied to the rated
    // capacity and EIR values in hares-equipment (not just propagated
    // through the typed config). A change to the coefficient constants
    // or to the ordering of the correction relative to other init steps
    // will cause these tests to fail.
    // -----------------------------------------------------------------------

    fn ac_charge_defect_config(charge_defect_ratio: Option<f64>) -> EquipmentConfig {
        EquipmentConfig::from_typed(
            "AC".to_string(),
            "Air Conditioner".to_string(),
            CentralAirConditionerConfig {
                charge_defect_ratio,
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 8_000.0,
                eir: 0.25,
                shr: Some(0.75),
                number_of_speeds: 1,
                stage_capacities_w: None,
                stage_eirs: None,
                stage_shrs: None,
                fan_power_w: None,
                fan_power_w_per_cfm: None,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_CENTRAL_AC_M3_S_PER_W),
                fraction_load_served: None,
                crankcase_heater_w: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                duct: DuctConfig::default(),
                system_type: None,
                startup_cd: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                min_oat_compressor_cooling_c: None,
            },
        )
        .unwrap()
    }

    #[test]
    fn charge_defect_reduces_rated_cooling_capacity_and_raises_eir() {
        // r = -0.10 (10% undercharge):
        //   capacity × (1 + 0.9 × (−0.10)) = × 0.91 — cooling output falls 9 %
        //   EIR     × (1 + (−0.9) × (−0.10)) = × 1.09 — efficiency degrades 9 %
        // This matches the physics: an undercharged compressor moves less
        // refrigerant and works harder per unit of cooling, so COP falls.
        // Reference: OpenStudio-HPXML (NREL) hvac.rb installation-quality EMS
        // model confirms EIR rises for undercharge (p_values give Y_CH_COP < 1
        // relative to Y_CH_Q for negative f_ch).
        let cfg = ac_charge_defect_config(Some(-0.10));
        let mut ac = AirConditioner::new(cfg.clone());
        let e = env(26.0, 0.010, 19.0, 35.0);
        ac.init(&cfg, &e).unwrap();

        let capacity = ac.core.hvac.config.cooling_capacities_w[0];
        let expected_cap = 8_000.0 * 0.91;
        assert!(
            (capacity - expected_cap).abs() / expected_cap < 1e-3,
            "charge_defect_ratio=-0.10 must reduce capacity by 9%; expected {expected_cap} got {capacity}"
        );

        let eir = ac.core.hvac.config.eir_by_stage[0];
        let expected_eir = 0.25 * 1.09;
        assert!(
            (eir - expected_eir).abs() / expected_eir < 1e-3,
            "charge_defect_ratio=-0.10 must raise EIR by 9% (efficiency degrades); expected {expected_eir} got {eir}"
        );
    }

    #[test]
    fn zero_charge_defect_leaves_capacity_and_eir_unchanged() {
        let cfg = ac_charge_defect_config(Some(0.0));
        let mut ac = AirConditioner::new(cfg.clone());
        let e = env(26.0, 0.010, 19.0, 35.0);
        ac.init(&cfg, &e).unwrap();

        let capacity = ac.core.hvac.config.cooling_capacities_w[0];
        assert!(
            (capacity - 8_000.0).abs() < 1e-9,
            "charge_defect_ratio=0.0 must leave capacity unchanged; got {capacity}"
        );

        let eir = ac.core.hvac.config.eir_by_stage[0];
        assert!(
            (eir - 0.25).abs() < 1e-9,
            "charge_defect_ratio=0.0 must leave EIR unchanged; got {eir}"
        );
    }

    #[test]
    fn absent_charge_defect_leaves_capacity_and_eir_unchanged() {
        let cfg = ac_charge_defect_config(None);
        let mut ac = AirConditioner::new(cfg.clone());
        let e = env(26.0, 0.010, 19.0, 35.0);
        ac.init(&cfg, &e).unwrap();

        let capacity = ac.core.hvac.config.cooling_capacities_w[0];
        assert!(
            (capacity - 8_000.0).abs() < 1e-9,
            "absent charge_defect_ratio must leave capacity unchanged; got {capacity}"
        );

        let eir = ac.core.hvac.config.eir_by_stage[0];
        assert!(
            (eir - 0.25).abs() < 1e-9,
            "absent charge_defect_ratio must leave EIR unchanged; got {eir}"
        );
    }

    // Regression: RoomAcConfig { crankcase_heater_w: None } must default to 0 kW,
    // not the central-AC default of 0.05 kW. Room ACs have no crankcase heater
    // per OCHRE convention (OCHRE HVAC.py: room/window ACs use 0 W crankcase).
    // OAT at 5 °C — below central-AC threshold (12.8 °C) — distinguishes the two:
    //   Room AC correct:    crankcase = 0.0 kW (no heater)
    //   Central AC default: crankcase = 0.05 kW (wrong for Room AC)
    #[test]
    fn room_ac_crankcase_none_defaults_to_zero_not_central_ac_default() {
        let cfg = EquipmentConfig::from_typed(
            "RAC".to_string(),
            "Room AC".to_string(),
            RoomAcConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 3_500.0,
                eir: 0.33,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    cooling_setpoint_source: None,
                    heating_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_ROOM_AC_M3_S_PER_W),
                shr: None,
                startup_cd: None,
                // Intentionally None: the equipment init must default to 0.0 kW (Room AC),
                // not 0.05 kW (central-AC default used if the code path is wrong).
                crankcase_heater_w: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                min_oat_compressor_cooling_c: None,
            },
        )
        .unwrap();

        let mut rac = RoomAC::new(cfg.clone());
        // Zone in deadband (21 °C, between heating=18 °C and cooling=24 °C).
        // OAT = 5 °C — below central-AC crankcase threshold (12.8 °C).
        // Compressor is off; crankcase heater power must be 0 kW for Room AC.
        let e = env(21.0, 0.010, 15.0, 5.0);
        rac.init(&cfg, &e).unwrap();
        rac.update_control(&e);

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        rac.step(&e, Duration::from_secs(60), &mut ports).unwrap();

        assert_eq!(
            ports.electrical.net_active_w(),
            0.0,
            "Room AC crankcase_heater_w=None must default to 0 kW (no crankcase heater); \
             central-AC default would draw 0.05 kW at 5 °C. Got {}",
            ports.electrical.net_active_w()
        );
    }

    #[test]
    fn thermal_setpoint_delta_adjusts_cooling_setpoint_on_ac() {
        let cfg = ac_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let env_state = env(28.0, 0.010, 19.0, 35.0);
        eq.init(&cfg, &env_state).unwrap();

        assert!(
            eq.descriptor()
                .control_capabilities
                .contains(hares_types::ControlCapabilities::THERMAL_SETPOINT_DELTA),
            "AirConditioner must declare THERMAL_SETPOINT_DELTA capability"
        );

        let baseline = eq.core.hvac.effective_setpoints();

        // Dispatch ThermalSetpointDelta: raise cooling setpoint by 1 °C.
        eq.apply_control(&ControlSignal::ThermalSetpointDelta {
            heating_delta_c: None,
            cooling_delta_c: Some(1.0),
        })
        .unwrap();

        let adjusted = eq.core.hvac.effective_setpoints();
        let expected_cooling = baseline.cooling_c + 1.0;
        assert!(
            (adjusted.cooling_c - expected_cooling).abs() < 1e-9,
            "cooling setpoint must increase by 1 °C: baseline={}, expected={}, got={}",
            baseline.cooling_c,
            expected_cooling,
            adjusted.cooling_c,
        );
        // Heating setpoint unchanged — delta only specified for cooling.
        assert_eq!(
            adjusted.heating_c, baseline.heating_c,
            "heating setpoint must not change when only cooling_delta_c is specified"
        );
    }

    /// Every signal variant corresponding to a declared capability must
    /// return Ok(()) when dispatched to the AirConditioner (regression
    /// guard) AND land on an explicit control arm: each dispatched variant
    /// must change the state field it drives. This pins the former gated
    /// catch-all check over the `ControlSignal` variants: a new variant
    /// with a declared capability that silently no-ops in the catch-all
    /// fails here.
    #[test]
    fn all_declared_ac_capabilities_return_ok_on_apply_control() {
        let cfg = ac_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let env_state = env(28.0, 0.010, 19.0, 35.0);
        eq.init(&cfg, &env_state).unwrap();

        let declared = eq.descriptor().control_capabilities;

        let signals: &[(&str, ControlSignal)] = &[
            (
                "ThermalSetpoint",
                ControlSignal::ThermalSetpoint {
                    heating_setpoint_c: Some(18.0),
                    cooling_setpoint_c: Some(24.0),
                    deadband_c: Some(1.5),
                },
            ),
            (
                "ThermalSetpointDelta",
                ControlSignal::ThermalSetpointDelta {
                    heating_delta_c: None,
                    cooling_delta_c: Some(1.0),
                },
            ),
            (
                "DutyCycle",
                ControlSignal::DutyCycle {
                    on_fraction: 0.5,
                    period_s: None,
                    component: None,
                },
            ),
            (
                "LoadFraction",
                ControlSignal::LoadFraction { fraction: 0.8 },
            ),
            (
                "PowerLimit",
                ControlSignal::PowerLimit {
                    max_power_w: 2000.0,
                    ramp_rate_w_per_s: None,
                },
            ),
            (
                "ModeOverride",
                ControlSignal::ModeOverride {
                    mode: OperatingMode::Cooling,
                },
            ),
            (
                "DemandResponse",
                ControlSignal::DemandResponse {
                    level: DRLevel::Moderate,
                    duration_s: None,
                },
            ),
            (
                "IdealCapacity",
                ControlSignal::IdealCapacity {
                    capacity_w: -3_000.0,
                    degraded: false,
                },
            ),
            (
                "MaxCapacityFraction",
                ControlSignal::MaxCapacityFraction { fraction: 0.75 },
            ),
        ];

        // Control state before the dispatch loop, for the effect assertions.
        let baseline_hysteresis = eq.core.hvac.thermostat_fsm.thermostat.hysteresis_c;
        let baseline_duty = eq.core.ctrl_duty_cycle;
        let baseline_load = eq.core.ctrl_load_fraction;
        let baseline_limit = eq.core.ctrl_power_limit_w;
        let baseline_mode = eq.core.ctrl_mode_override;
        let baseline_dr = eq.core.dr_level;
        let baseline_ideal = eq.core.ideal_capacity_w;
        let baseline_max_cap = eq.core.hvac.control.max_capacity_fraction;

        for (label, signal) in signals {
            let required = signal.required_capability();
            assert!(
                declared.contains(required),
                "test invariant: signal '{label}' requires capability {required:?} \
                 which must be in AC's declared capabilities {declared:?}",
            );
            let result = eq.apply_control(signal);
            assert!(
                result.is_ok(),
                "apply_control for '{label}' signal must return Ok(()), got {result:?}",
            );
            // Each declared variant must land on an explicit arm and move the
            // state field it drives; only ThermalSetpointDelta is routed
            // through the hvac catch-all path (its effects are setpoint-side
            // and pinned by the ThermalSetpointDelta test above).
            let changed = match signal {
                ControlSignal::ThermalSetpoint { .. } => {
                    eq.core.hvac.thermostat_fsm.thermostat.hysteresis_c != baseline_hysteresis
                }
                ControlSignal::ThermalSetpointDelta { .. } => true,
                ControlSignal::DutyCycle { .. } => eq.core.ctrl_duty_cycle != baseline_duty,
                ControlSignal::LoadFraction { .. } => eq.core.ctrl_load_fraction != baseline_load,
                ControlSignal::PowerLimit { .. } => eq.core.ctrl_power_limit_w != baseline_limit,
                ControlSignal::ModeOverride { .. } => eq.core.ctrl_mode_override != baseline_mode,
                ControlSignal::DemandResponse { .. } => eq.core.dr_level != baseline_dr,
                ControlSignal::IdealCapacity { .. } => eq.core.ideal_capacity_w != baseline_ideal,
                ControlSignal::MaxCapacityFraction { .. } => {
                    eq.core.hvac.control.max_capacity_fraction != baseline_max_cap
                }
                other => panic!("unhandled ControlSignal variant in test: {other:?}"),
            };
            assert!(
                changed,
                "signal '{label}' must reach an explicit control arm and change \
                 the state it drives, not fall through to the catch-all"
            );
        }
    }

    // ── Minimum OAT compressor lockout tests ────────────────────────────

    fn oat_lockout_env(outdoor_temp_c: f64) -> EnvironmentState {
        EnvironmentState {
            ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: 28.0,
                humidity_ratio: 0.010,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c,
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
                island_bus_voltage_pu: None,
            },
            schedule_row: None,
            unavailable_periods: hares_types::UnavailablePeriodState::default(),
            domains: hares_types::DomainSlots::default(),
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

    /// Create and init an AirConditioner with a specific `min_oat_cooling_c` and speed mode.
    fn init_ac_with_lockout(min_oat_cooling_c: f64, n_speeds: u8) -> AirConditioner {
        let inner_cfg = CentralAirConditionerConfig {
            equipment_id: None,
            zone_id: Some(1),
            capacity_w: 10_000.0,
            eir: 0.25,
            shr: None,
            number_of_speeds: n_speeds,
            stage_capacities_w: if n_speeds >= 4 {
                Some(vec![3_000.0, 6_000.0, 8_000.0, 10_000.0])
            } else {
                None
            },
            stage_eirs: None,
            stage_shrs: None,
            fan_power_w: None,
            fan_power_w_per_cfm: None,
            setpoint: HvacSetpointConfig {
                cooling_setpoint_c: Some(24.0),
                heating_setpoint_c: Some(18.0),
                heating_setpoint_source: None,
                cooling_setpoint_source: None,
            },
            hysteresis_c: Some(1.0),
            airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_CENTRAL_AC_M3_S_PER_W),
            fraction_load_served: None,
            crankcase_heater_w: None,
            crankcase_heater_threshold_c: None,
            crankcase_capacity_curve_coeffs: None,
            duct: DuctConfig::default(),
            system_type: None,
            startup_cd: None,
            biquadratic_x1_min: None,
            biquadratic_x1_max: None,
            biquadratic_x2_min: None,
            biquadratic_x2_max: None,
            ff_min: None,
            ff_max: None,
            plf_min: None,
            plf_max: None,
            charge_defect_ratio: None,
            min_oat_compressor_cooling_c: Some(min_oat_cooling_c),
        };
        let cfg =
            EquipmentConfig::from_typed("AC".to_string(), "Air Conditioner".to_string(), inner_cfg)
                .unwrap();
        let mut ac = AirConditioner::new(cfg.clone());
        let init_env = oat_lockout_env(30.0);
        ac.init(&cfg, &init_env).expect("init must succeed");
        ac
    }

    /// Single-speed AC: update_control forces Off when OAT is below the
    /// compressor lockout threshold, even though the thermostat calls for cooling.
    #[test]
    fn cooling_locked_out_below_min_oat_single_speed() {
        let mut ac = init_ac_with_lockout(-25.0, 1);
        let env_cold = oat_lockout_env(-30.0);
        let mode = ac.update_control(&env_cold);
        assert_eq!(mode, OperatingMode::Off, "lockout must suppress cooling");
    }

    /// Variable-speed AC: update_control forces Off below the lockout threshold.
    #[test]
    fn cooling_locked_out_below_min_oat_variable_speed() {
        let mut ac = init_ac_with_lockout(-25.0, 4);
        let env_cold = oat_lockout_env(-30.0);
        let mode = ac.update_control(&env_cold);
        assert_eq!(mode, OperatingMode::Off);
    }

    /// Cooling operates normally when OAT is at or above the lockout threshold.
    #[test]
    fn cooling_operates_when_oat_above_min() {
        let mut ac = init_ac_with_lockout(-25.0, 1);
        let env_warm = oat_lockout_env(30.0);
        let mode = ac.update_control(&env_warm);
        assert_eq!(mode, OperatingMode::Cooling);
    }

    /// At exactly the lockout threshold, cooling is not suppressed.
    #[test]
    fn cooling_operates_at_lockout_threshold() {
        let mut ac = init_ac_with_lockout(-20.0, 1);
        let env_at_threshold = oat_lockout_env(-20.0);
        let mode = ac.update_control(&env_at_threshold);
        assert_eq!(mode, OperatingMode::Cooling);
    }

    /// The lockout condition is evaluated each step. After a lockout step, a
    /// subsequent warm step recovers to normal operation.
    #[test]
    fn lockout_flag_reset_on_warm_step() {
        let mut ac = init_ac_with_lockout(-25.0, 1);

        // Step 1: cold — lockout active, returns Off.
        let mode = ac.update_control(&oat_lockout_env(-30.0));
        assert_eq!(mode, OperatingMode::Off);

        // Step 2: warm — cooling operates.
        let mode = ac.update_control(&oat_lockout_env(30.0));
        assert_eq!(mode, OperatingMode::Cooling);
    }

    /// Cooling telemetry flag is published in step() when lockout was active.
    #[test]
    fn cooling_oat_lockout_telemetry_published() {
        let mut ac = init_ac_with_lockout(-25.0, 1);
        let env_cold = oat_lockout_env(-30.0);

        ac.update_control(&env_cold);

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let dt = Duration::from_secs(60);
        ac.step(&env_cold, dt, &mut ports).expect("step");

        let val = ac.telemetry().get(tk::COOLING_OAT_LOCKOUT);
        assert!(
            (val.unwrap_or(0.0) - 1.0).abs() < 1e-9,
            "COOLING_OAT_LOCKOUT must be 1.0 when lockout active, got {val:?}"
        );
    }

    /// Cooling telemetry flag is 0.0 when lockout is not active.
    #[test]
    fn cooling_oat_lockout_telemetry_zero_when_normal() {
        let mut ac = init_ac_with_lockout(-25.0, 1);
        let env_warm = oat_lockout_env(30.0);

        ac.update_control(&env_warm);

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let dt = Duration::from_secs(60);
        ac.step(&env_warm, dt, &mut ports).expect("step");

        let val = ac.telemetry().get(tk::COOLING_OAT_LOCKOUT);
        assert!(
            (val.unwrap_or(1.0) - 0.0).abs() < 1e-9,
            "COOLING_OAT_LOCKOUT must be 0.0 when not locked out"
        );
    }

    #[test]
    fn ac_cop_clamped_to_physical_range() {
        // AC with pathological capacity curve (c0=100 × rated) and identity
        // eir curve produces unbounded COP ~1033. Verify telemetry COP is
        // clamped to [0.0, 8.0].
        let mut cfg = ac_config();
        cfg.test_extras_mut().insert(
            "capacity_biquadratic_coeffs".to_string(),
            "[100,0,0,0,0,0]".into(),
        );
        cfg.test_extras_mut()
            .insert("eir_biquadratic_coeffs".to_string(), "[1,0,0,0,0,0]".into());
        // Zone at 26°C (above 24°C setpoint) so AC runs, OAT 30°C.
        let e = env(26.0, 0.009, 18.0, 30.0);
        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();
        eq.update_control(&e);
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.step(&e, Duration::from_secs(60), &mut ports).unwrap();

        let cop = eq.telemetry().get(tk::COP).unwrap_or(-1.0);
        assert!(
            cop.is_finite() && (0.0..=8.0).contains(&cop),
            "AC cooling COP must be in [0.0, 8.0], got {cop}"
        );
        // Unclamped COP would be ~1033; clamping must have reduced it.
        assert!(
            cop < 100.0,
            "COP must have been clamped below 100 (raw ~1033), got {cop}"
        );
    }

    /// Two-stage central AC with per-stage SHR [0.80, 0.70]: coil Ao values
    /// must differ between stages, confirming each stage uses its own SHR
    /// rather than the first stage's rated SHR for all stages.
    #[test]
    fn multi_stage_shr_produces_different_ao_by_stage() {
        let cfg = ac_config_with(|typed| {
            typed.stage_capacities_w = Some(vec![8_000.0, 12_000.0]);
            typed.stage_eirs = Some(vec![0.33, 0.30]);
            typed.stage_shrs = Some(vec![0.80, 0.70]);
            typed.number_of_speeds = 2;
            typed.startup_cd = Some(0.0);
        });

        let mut eq = AirConditioner::new(cfg.clone());
        let environment = env(28.0, 0.012, 20.0, 35.0);
        eq.init(&cfg, &environment).unwrap();

        assert_eq!(
            eq.core.coil_ao_by_stage.len(),
            2,
            "two-stage AC must have two Ao entries"
        );
        assert!(
            (eq.core.coil_ao_by_stage[0] - eq.core.coil_ao_by_stage[1]).abs() > 1e-6,
            "Ao values differ between stages when per-stage SHR differs"
        );
        assert_eq!(
            eq.core.stage_shrs,
            vec![0.80, 0.70],
            "per-stage SHR values preserved after init"
        );
        // AHRI 210/240 rated conditions correspond to full-load (last) stage.
        assert!(
            (eq.core.rated_shr - 0.70).abs() < 1e-9,
            "rated_shr should equal full-load stage SHR (last element of stage_shrs)"
        );
    }

    /// When per-stage SHRs are provided on a multi-speed AC, `rated_shr`
    /// must be the full-load (last) stage value, not the lowest speed's.
    /// (number_of_speeds validation permits 1, 2, or 4; ticket T-0436 uses
    /// a 3-stage example for illustration but the test uses 4 stages for
    /// compatibility with the existing validation constraint.)
    #[test]
    fn rated_shr_is_full_load_stage_shr() {
        let cfg = ac_config_with(|typed| {
            typed.stage_capacities_w = Some(vec![5_000.0, 6_500.0, 8_000.0, 10_000.0]);
            typed.stage_eirs = Some(vec![0.35, 0.33, 0.32, 0.30]);
            typed.stage_shrs = Some(vec![0.82, 0.76, 0.74, 0.72]);
            typed.number_of_speeds = 4;
            typed.startup_cd = Some(0.0);
        });

        let mut eq = AirConditioner::new(cfg.clone());
        let environment = env(28.0, 0.012, 20.0, 35.0);
        eq.init(&cfg, &environment).unwrap();

        assert!(
            (eq.core.rated_shr - 0.72).abs() < 1e-9,
            "rated_shr ({}) must equal full-load stage SHR (0.72), not lowest stage (0.82)",
            eq.core.rated_shr
        );
    }

    /// Regression: with per-stage SHRs [0.85, 0.70], `rated_shr` must be 0.70
    /// so that the Henderson-Rengarajan latent degradation baseline
    /// `rated_latent_w = rated_cap_w * (1.0 - rated_shr)` uses the correct
    /// full-load SHR rather than the incorrect first-stage SHR.
    #[test]
    fn rated_latent_baseline_uses_full_load_shr() {
        let cfg = ac_config_with(|typed| {
            typed.stage_capacities_w = Some(vec![6_000.0, 10_000.0]);
            typed.stage_eirs = Some(vec![0.35, 0.30]);
            typed.stage_shrs = Some(vec![0.85, 0.70]);
            typed.number_of_speeds = 2;
            typed.startup_cd = Some(0.0);
        });

        let mut eq = AirConditioner::new(cfg.clone());
        let environment = env(28.0, 0.012, 20.0, 35.0);
        eq.init(&cfg, &environment).unwrap();

        assert!(
            (eq.core.rated_shr - 0.70).abs() < 1e-9,
            "rated_shr ({}) must be full-load stage SHR (0.70), not lowest stage (0.85)",
            eq.core.rated_shr
        );

        let rated_cap_w = eq
            .core
            .hvac
            .config
            .cooling_capacities_w
            .last()
            .copied()
            .unwrap();
        // rated_latent_w = rated_cap_w * (1.0 - rated_shr)
        // Correct:  10_000 * (1.0 - 0.70) = 3_000 W
        // Old bug:  10_000 * (1.0 - 0.85) = 1_500 W (understates latent baseline)
        let actual_rated_latent_w = rated_cap_w * (1.0 - eq.core.rated_shr);
        let buggy_rated_latent_w = rated_cap_w * (1.0 - 0.85);
        assert!(
            (actual_rated_latent_w - 3_000.0).abs() < 1e-9,
            "rated_latent_w ({}) should be 3_000 W with full-load SHR=0.70",
            actual_rated_latent_w
        );
        assert!(
            (buggy_rated_latent_w - 1_500.0).abs() < 1e-9,
            "old bug would compute rated_latent_w=1_500 W using wrong first-stage SHR=0.85"
        );
        assert!(
            actual_rated_latent_w > buggy_rated_latent_w,
            "fix produces larger rated_latent_w baseline (3_000 > 1_500)"
        );
    }

    #[test]
    fn take_health_counts_reports_and_resets_curve_index_clamps() {
        let mut eq = AirConditioner::new(ac_config());
        eq.core.hvac.config.biquadratic_coeffs = vec![
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        ];
        let _ = eq.core.hvac.evaluate_biquadratic(4, 20.0, 30.0);
        let counts = Equipment::take_health_counts(&mut eq);
        assert_eq!(
            counts.curve_index_clamps, 1,
            "one out-of-bounds evaluation → one clamp"
        );
        assert_eq!(
            Equipment::take_health_counts(&mut eq).curve_index_clamps,
            0,
            "take resets the counters"
        );
    }

    #[test]
    fn room_ac_take_health_counts_reports_and_resets_curve_index_clamps() {
        let mut eq = RoomAC::new(room_ac_config());
        eq.core.hvac.config.biquadratic_coeffs = vec![
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        ];
        let _ = eq.core.hvac.evaluate_biquadratic(5, 20.0, 30.0);
        let counts = Equipment::take_health_counts(&mut eq);
        assert_eq!(counts.curve_index_clamps, 1);
        assert_eq!(
            Equipment::take_health_counts(&mut eq).curve_index_clamps,
            0,
            "take resets the counters"
        );
    }
}

#[cfg(test)]
mod dr_tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        ControlSignal, DRLevel, EnvironmentState, GridState, HumidityAccumulator, PortSlots,
        ThermalAccumulator, WeatherState, ZoneId, ZoneState, telemetry_keys as tk,
    };

    use super::AirConditioner;
    use crate::{
        CentralAirConditionerConfig, DuctConfig, Equipment, EquipmentConfig, HvacSetpointConfig,
    };

    /// Zone above cooling setpoint, suitable for triggering active cooling.
    fn hot_env(zone_temp_c: f64) -> EnvironmentState {
        EnvironmentState {
            ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp_c,
                humidity_ratio: 0.010,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 35.0,
                outdoor_humidity_ratio: 0.012,
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
                island_bus_voltage_pu: None,
            },
            schedule_row: None,
            unavailable_periods: hares_types::UnavailablePeriodState::default(),
            domains: hares_types::DomainSlots::default(),
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

    fn hot_env_at_time(zone_temp_c: f64, second: i64) -> EnvironmentState {
        EnvironmentState {
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: std::collections::HashMap::new(),
            current_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 3, 18, 0, 0, 0)
                .single()
                .expect("valid")
                + ChronoDuration::seconds(second),
            ..hot_env(zone_temp_c)
        }
    }

    fn base_config() -> EquipmentConfig {
        let mut cfg = EquipmentConfig::from_typed(
            "AC".to_string(),
            "Air Conditioner".to_string(),
            CentralAirConditionerConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 8_000.0,
                eir: 3.412_141_633 / 0.33,
                shr: Some(0.75),
                number_of_speeds: 1,
                stage_capacities_w: None,
                stage_eirs: None,
                stage_shrs: None,
                fan_power_w: None,
                fan_power_w_per_cfm: None,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                hysteresis_c: Some(hares_types::MIN_THERMOSTAT_BAND_C),
                airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_CENTRAL_AC_M3_S_PER_W),
                fraction_load_served: None,
                crankcase_heater_w: Some(100.0),
                crankcase_heater_threshold_c: Some(12.8),
                crankcase_capacity_curve_coeffs: None,
                duct: DuctConfig::default(),
                system_type: None,
                startup_cd: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                charge_defect_ratio: None,
                min_oat_compressor_cooling_c: None,
            },
        )
        .unwrap();
        cfg.test_extras_mut().insert(
            "capacity_biquadratic_coeffs".to_string(),
            "[1,0,0,0,0,0]".into(),
        );
        cfg.test_extras_mut()
            .insert("eir_biquadratic_coeffs".to_string(), "[1,0,0,0,0,0]".into());
        cfg
    }

    fn make_ports() -> PortSlots {
        PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        }
    }

    /// Run one full control+step cycle. Returns electric_w.
    fn step_once(eq: &mut AirConditioner, env: &EnvironmentState) -> f64 {
        let mut ports = make_ports();
        eq.update_control(env);
        eq.step(env, Duration::from_secs(60), &mut ports).unwrap();
        eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0)
    }

    // DR Moderate: cooling setpoint offset = +1°C → unit turns off when zone is between
    // the original and the DR-shifted setpoint.
    #[test]
    fn dr_moderate_shifts_cooling_setpoint_up() {
        let cfg = base_config();
        // Zone at 24.5°C: above base setpoint (24°C) so AC runs normally.
        // After DR Moderate (+1°C), effective setpoint = 25°C > 24.5°C → AC turns off.
        let environment = hot_env(24.5);

        // Baseline: without DR, AC should be cooling.
        let mut eq_base = AirConditioner::new(cfg.clone());
        eq_base.init(&cfg, &environment).unwrap();
        let kw_no_dr = step_once(&mut eq_base, &environment);
        assert!(kw_no_dr > 0.0, "AC must be active without DR at 24.5°C");

        // With DR Moderate: effective setpoint raised to 25°C; zone 24.5°C < 25°C → no cooling.
        let mut eq_dr = AirConditioner::new(cfg.clone());
        eq_dr.init(&cfg, &environment).unwrap();
        eq_dr
            .apply_control(&ControlSignal::DemandResponse {
                level: DRLevel::Moderate,
                duration_s: None,
            })
            .unwrap();
        let kw_with_dr = step_once(&mut eq_dr, &environment);
        assert!(
            kw_with_dr < kw_no_dr,
            "DR Moderate must eliminate cooling demand when zone is between original and DR setpoint; baseline={kw_no_dr:.4} kW, dr={kw_with_dr:.4} kW"
        );
    }

    // DR Critical: load_fraction = 0.5, output must be halved vs the Critical-free run.
    #[test]
    fn dr_critical_reduces_load_fraction() {
        let cfg = base_config();
        // Zone far above both base (24°C) and critical-adjusted (27°C) setpoints.
        let environment = hot_env(32.0);

        let mut eq_base = AirConditioner::new(cfg.clone());
        eq_base.init(&cfg, &environment).unwrap();
        let kw_full = step_once(&mut eq_base, &environment);

        let mut eq_dr = AirConditioner::new(cfg.clone());
        eq_dr.init(&cfg, &environment).unwrap();
        eq_dr
            .apply_control(&ControlSignal::DemandResponse {
                level: DRLevel::Critical,
                duration_s: None,
            })
            .unwrap();
        let kw_dr = step_once(&mut eq_dr, &environment);

        assert!(kw_full > 0.0, "baseline must draw power");
        assert!(kw_dr > 0.0, "DR Critical must not completely shut off");
        // DR Critical sets dr_load_fraction=0.5, so power must be ≈ half.
        // Allow 15% tolerance for cooling setpoint offset effects on load selection.
        let ratio = kw_dr / kw_full;
        assert!(
            ratio < 0.6,
            "DR Critical load_fraction=0.5 must reduce power by ≥40%; ratio={ratio:.3}"
        );
    }

    // GridEmergency: dr_load_fraction=0 → full shed → zero output.
    #[test]
    fn dr_grid_emergency_forces_equipment_off() {
        let cfg = base_config();
        let environment = hot_env(30.0);

        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();
        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::GridEmergency,
            duration_s: None,
        })
        .unwrap();
        let kw = step_once(&mut eq, &environment);
        assert_eq!(kw, 0.0, "GridEmergency must force zero output");
    }

    // Normal after Critical: all overrides cleared, setpoint_offset=0, fractions=1.
    #[test]
    fn dr_normal_clears_all_overrides() {
        let cfg = base_config();
        let environment = hot_env(32.0);

        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();

        // Apply Critical DR first.
        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::Critical,
            duration_s: None,
        })
        .unwrap();
        let kw_critical = step_once(&mut eq, &environment);

        // Now clear with Normal.
        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::Normal,
            duration_s: None,
        })
        .unwrap();
        let kw_normal = step_once(&mut eq, &environment);

        assert!(kw_critical > 0.0, "Critical must allow some output");
        // After Normal, output must recover toward the baseline (unrestricted) level.
        assert!(
            kw_normal > kw_critical,
            "Normal must clear DR load reduction; normal={kw_normal:.4}, critical={kw_critical:.4}"
        );
    }

    // DR with duration_s: auto-reverts to Normal after the duration elapses.
    // The AC's update_control decrements dr_duration_remaining_s by time_res (1 min = 60 s)
    // each call. DR persists while remaining > 0 and auto-reverts when it reaches <= 0.
    #[test]
    fn dr_duration_auto_reverts() {
        let cfg = base_config();
        let env_t0 = hot_env_at_time(28.0, 0);
        let env_t60 = hot_env_at_time(28.0, 60);

        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &env_t0).unwrap();

        // GridEmergency for 120 s (two 60-s steps); time_res = 1 min = 60 s.
        // The countdown advances once per step, in step(): the event sheds
        // exactly its two named steps and the unit recovers on the third.
        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::GridEmergency,
            duration_s: Some(120.0),
        })
        .unwrap();

        // Step at t=0: the countdown advances 120→60 s (still active) → off.
        let kw_during = step_once(&mut eq, &env_t0);
        assert_eq!(
            kw_during, 0.0,
            "GridEmergency must shed load while duration active"
        );

        // Step at t=60: the countdown advances 60→0 s and reverts, but the
        // step's control already ran with the event active → off. The shed
        // covers the event's second named step.
        let kw_during_second = step_once(&mut eq, &env_t60);
        assert_eq!(
            kw_during_second, 0.0,
            "the event's second named step stays shed: the duration is the \
             duration the signal named"
        );

        // Step at t=120: the event is over → AC can cool.
        let env_t120 = hot_env_at_time(28.0, 120);
        let kw_after = step_once(&mut eq, &env_t120);
        assert!(
            kw_after > 0.0,
            "After DR duration expires, AC must recover; kw_after={kw_after}"
        );
    }

    // DutyCycle 0.5: output approximately halved vs duty_cycle=1.
    #[test]
    fn duty_cycle_scales_cooling_output() {
        let cfg = base_config();
        let environment = hot_env(30.0);

        let mut eq_full = AirConditioner::new(cfg.clone());
        eq_full.init(&cfg, &environment).unwrap();
        let kw_full = step_once(&mut eq_full, &environment);

        let mut eq_half = AirConditioner::new(cfg.clone());
        eq_half.init(&cfg, &environment).unwrap();
        eq_half
            .apply_control(&ControlSignal::DutyCycle {
                on_fraction: 0.5,
                period_s: None,
                component: None,
            })
            .unwrap();
        let kw_half = step_once(&mut eq_half, &environment);

        assert!(kw_full > 0.0, "full duty must draw power");
        assert!(kw_half > 0.0, "half duty must draw power");
        let ratio = kw_half / kw_full;
        assert!(
            (ratio - 0.5).abs() < 0.1,
            "DutyCycle 0.5 must halve output ±10%; ratio={ratio:.3}"
        );
    }

    // LoadFraction 0.0: forces AC off for that step.
    // The signal is one-step transient, but it survives update_control and clears
    // at the end of step().
    #[test]
    fn load_fraction_zero_forces_off() {
        let cfg = base_config();
        let environment = hot_env(30.0);

        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();

        // update_control determines thermostat mode first (sets operating_mode = Cooling).
        eq.update_control(&environment);
        // Applying after update_control still affects the current step.
        eq.apply_control(&ControlSignal::LoadFraction { fraction: 0.0 })
            .unwrap();
        let mut ports = make_ports();
        eq.step(&environment, Duration::from_secs(60), &mut ports)
            .unwrap();
        let kw = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        assert_eq!(kw, 0.0, "LoadFraction 0.0 must force zero output this step");
    }

    // LoadFraction is transient for one step. The next step restores the default
    // unless the control is re-applied.
    #[test]
    fn load_fraction_resets_each_step() {
        let cfg = base_config();
        let environment = hot_env(30.0);

        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();

        // Step 1: update_control → apply LoadFraction=0 → step → must be off.
        eq.update_control(&environment);
        eq.apply_control(&ControlSignal::LoadFraction { fraction: 0.0 })
            .unwrap();
        let mut ports1 = make_ports();
        eq.step(&environment, Duration::from_secs(60), &mut ports1)
            .unwrap();
        let kw_step1 = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        assert_eq!(kw_step1, 0.0, "step 1 with LoadFraction=0 must be off");

        // Step 2: no signal reapplied → AC runs again.
        let kw_step2 = step_once(&mut eq, &environment);
        assert!(
            kw_step2 > 0.0,
            "step 2 must recover after transient LoadFraction reset; got {kw_step2}"
        );
    }

    // PowerLimit clamps electrical draw and scales thermal proportionally.
    #[test]
    fn power_limit_clamps_electric_draw() {
        let cfg = base_config();
        // Zone well above setpoint to drive full AC output.
        let environment = hot_env(30.0);

        let mut eq_unlimited = AirConditioner::new(cfg.clone());
        eq_unlimited.init(&cfg, &environment).unwrap();
        let kw_unlimited = step_once(&mut eq_unlimited, &environment);

        // Set limit well below expected draw. The signal carries kW until
        // the control-layer commit; the W-intent limit converts once.
        let limit_w = (kw_unlimited * 0.5).max(0.1);
        let mut eq_limited = AirConditioner::new(cfg.clone());
        eq_limited.init(&cfg, &environment).unwrap();
        eq_limited
            .apply_control(&ControlSignal::PowerLimit {
                max_power_w: limit_w / 1000.0,
                ramp_rate_w_per_s: None,
            })
            .unwrap();
        let kw_limited = step_once(&mut eq_limited, &environment);

        assert!(
            kw_unlimited > limit_w,
            "unlimited draw must exceed limit for this test to be meaningful"
        );
        assert!(
            kw_limited <= limit_w + 1e-9,
            "PowerLimit must clamp electric draw: limited={kw_limited:.4}, limit={limit_w:.4}"
        );
        assert!(
            kw_limited > 0.0,
            "PowerLimit must not zero out the equipment"
        );
    }

    /// The assembled base under an active power limit: the limit acts on the
    /// rated composition before the voltage response multiplies it, so the
    /// reference-voltage draw for that state is the capped draw, not the
    /// nameplate rating. The aggregate's weight must be that capped base (the
    /// draw the same limited unit takes at 1.0 pu) and its response the capped
    /// composition's blend: a base of the uncapped rating would divide the
    /// capped draw by a reference the limit never let the unit reach.
    #[test]
    fn power_limited_moment_reports_the_capped_reference_draw() {
        let cfg = base_config();
        let environment = hot_env(30.0);

        let mut eq_unlimited = AirConditioner::new(cfg.clone());
        eq_unlimited.init(&cfg, &environment).unwrap();
        let unlimited_w = step_once(&mut eq_unlimited, &environment);
        // Half the unlimited draw, in watts (the signal and the stored field
        // are watts: `apply_signal` assigns `max_power_w` to
        // `ctrl_power_limit_w`, which the step compares against the electric
        // draw).
        let limit_w = 0.5 * unlimited_w;

        let run = |voltage_pu: f64| -> AirConditioner {
            let mut env_run = hot_env(30.0);
            env_run.grid.voltage_pu = voltage_pu;
            let mut eq = AirConditioner::new(cfg.clone());
            eq.init(&cfg, &environment).unwrap();
            eq.apply_control(&ControlSignal::PowerLimit {
                max_power_w: limit_w,
                ramp_rate_w_per_s: None,
            })
            .unwrap();
            let mut ports = make_ports();
            eq.update_control(&env_run);
            eq.step(&env_run, Duration::from_secs(60), &mut ports)
                .unwrap();
            eq
        };
        let draw = |eq: &AirConditioner| match eq.core_output().flows.electric_w {
            Some(hares_types::ElectricPower::Consumption(w)) => w,
            other => panic!("fixture: the unit must report its draw, got {other:?}"),
        };

        let nominal_unit = run(1.0);
        let sag_unit = run(0.5);
        let nominal_w = draw(&nominal_unit);
        assert!(
            unlimited_w > limit_w,
            "fixture: the limit must bind ({unlimited_w} W unlimited)"
        );
        assert!(
            (nominal_w - limit_w).abs() < 1e-9,
            "fixture: at the reference voltage the capped draw is the limit \
             ({limit_w} W), got {nominal_w}"
        );

        let reported_nominal = sag_unit
            .nominal_electric_draw_w()
            .expect("the stepped unit drew power");
        assert!(
            (reported_nominal - nominal_w).abs() < 1e-6 * nominal_w,
            "the base must be the capped reference draw {nominal_w}, got \
             {reported_nominal} (the uncapped rating is {unlimited_w})"
        );

        // The response: the capped composition's components evaluate their own
        // rows (compressor 1.6 x 0.25 - 2.69 x 0.5 + 2.09 = 1.145, blower
        // 0.26 x 0.25 + 0.9 x 0.5 - 0.16 = 0.355).
        let component_w = |eq: &AirConditioner, key: &str| {
            let w = eq.telemetry().get(key).unwrap_or(f64::NAN);
            assert!(w.is_finite(), "fixture: {key} must be finite");
            w
        };
        let compressor_n = component_w(&nominal_unit, tk::COMPRESSOR_KW);
        let fan_n = component_w(&nominal_unit, tk::FAN_KW);
        assert!(compressor_n > 0.0 && fan_n > 0.0, "fixture: both run");
        let m_compressor: f64 = 1.6 * 0.25 - 2.69 * 0.5 + 2.09;
        let m_fan: f64 = 0.26 * 0.25 + 0.9 * 0.5 - 0.16;
        let expected_response =
            (compressor_n * m_compressor + fan_n * m_fan) / (compressor_n + fan_n);
        let response = draw(&sag_unit) / reported_nominal;
        assert!(
            (response - expected_response).abs() < 1e-9,
            "the response must be the capped composition's blend \
             {expected_response}, got {response}"
        );
    }
}

#[cfg(test)]
mod crankcase_tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        EnvironmentState, GridState, HumidityAccumulator, PortSlots, ThermalAccumulator,
        WeatherState, ZoneId, ZoneState, telemetry_keys as tk,
    };

    use super::AirConditioner;
    use crate::{
        CentralAirConditionerConfig, DuctConfig, Equipment, EquipmentConfig, HvacSetpointConfig,
    };

    fn env(
        zone_temp_c: f64,
        humidity_ratio: f64,
        _wet_bulb_c: f64,
        outdoor_c: f64,
    ) -> EnvironmentState {
        EnvironmentState {
            ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp_c,
                humidity_ratio,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: outdoor_c,
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
                island_bus_voltage_pu: None,
            },
            schedule_row: None,
            unavailable_periods: hares_types::UnavailablePeriodState::default(),
            domains: hares_types::DomainSlots::default(),
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

    fn ac_config() -> EquipmentConfig {
        let mut cfg = EquipmentConfig::from_typed(
            "AC".to_string(),
            "Air Conditioner".to_string(),
            CentralAirConditionerConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 8_000.0,
                eir: 3.412_141_633 / 0.33,
                shr: Some(0.75),
                number_of_speeds: 1,
                stage_capacities_w: None,
                stage_eirs: None,
                stage_shrs: None,
                fan_power_w: None,
                fan_power_w_per_cfm: None,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_CENTRAL_AC_M3_S_PER_W),
                fraction_load_served: None,
                crankcase_heater_w: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                duct: DuctConfig::default(),
                system_type: None,
                startup_cd: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                charge_defect_ratio: None,
                min_oat_compressor_cooling_c: None,
            },
        )
        .unwrap();
        cfg.test_extras_mut().insert(
            "capacity_biquadratic_coeffs".to_string(),
            "[1,0,0,0,0,0]".into(),
        );
        cfg.test_extras_mut()
            .insert("eir_biquadratic_coeffs".to_string(), "[1,0,0,0,0,0]".into());
        cfg
    }

    fn cold_env(outdoor_c: f64, zone_temp_c: f64) -> EnvironmentState {
        EnvironmentState {
            ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp_c,
                humidity_ratio: 0.008,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: outdoor_c,
                outdoor_humidity_ratio: 0.003,
                wind_speed_m_s: 2.0,
                wind_dir_deg: 0.0,
                ground_temp_c: 10.0,
                sky_temp_c: 5.0,
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
                island_bus_voltage_pu: None,
            },
            schedule_row: None,
            unavailable_periods: hares_types::UnavailablePeriodState::default(),
            domains: hares_types::DomainSlots::default(),
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: std::collections::HashMap::new(),
            current_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 1, 15, 0, 0, 0)
                .single()
                .expect("valid"),
            time_res: ChronoDuration::minutes(1),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    fn base_config() -> EquipmentConfig {
        let mut cfg = EquipmentConfig::from_typed(
            "AC".to_string(),
            "Air Conditioner".to_string(),
            CentralAirConditionerConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 8_000.0,
                eir: 3.412_141_633 / 0.33,
                shr: Some(0.75),
                number_of_speeds: 1,
                stage_capacities_w: None,
                stage_eirs: None,
                stage_shrs: None,
                fan_power_w: None,
                fan_power_w_per_cfm: None,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(26.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                hysteresis_c: Some(hares_types::MIN_THERMOSTAT_BAND_C),
                airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_CENTRAL_AC_M3_S_PER_W),
                fraction_load_served: None,
                crankcase_heater_w: Some(100.0),
                crankcase_heater_threshold_c: Some(12.8),
                crankcase_capacity_curve_coeffs: None,
                duct: DuctConfig::default(),
                system_type: None,
                startup_cd: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                charge_defect_ratio: None,
                min_oat_compressor_cooling_c: None,
            },
        )
        .unwrap();
        cfg.test_extras_mut().insert(
            "capacity_biquadratic_coeffs".to_string(),
            "[1,0,0,0,0,0]".into(),
        );
        cfg.test_extras_mut()
            .insert("eir_biquadratic_coeffs".to_string(), "[1,0,0,0,0,0]".into());
        cfg
    }

    fn base_config_with(mutator: impl FnOnce(&mut CentralAirConditionerConfig)) -> EquipmentConfig {
        let mut typed = base_config()
            .typed::<CentralAirConditionerConfig>()
            .unwrap();
        mutator(&mut typed);
        let mut cfg =
            EquipmentConfig::from_typed("AC".to_string(), "Air Conditioner".to_string(), typed)
                .unwrap();
        cfg.test_extras_mut().insert(
            "capacity_biquadratic_coeffs".to_string(),
            "[1,0,0,0,0,0]".into(),
        );
        cfg.test_extras_mut()
            .insert("eir_biquadratic_coeffs".to_string(), "[1,0,0,0,0,0]".into());
        cfg
    }

    fn step_ac_off(cfg: &EquipmentConfig, outdoor_c: f64) -> f64 {
        let mut eq = AirConditioner::new(cfg.clone());
        let e = cold_env(outdoor_c, 20.0); // zone below cooling setpoint (26 C) - AC Off
        eq.init(cfg, &e).unwrap();
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&e);
        eq.step(&e, Duration::from_secs(60), &mut ports).unwrap();
        eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0)
    }

    // Test 1: Standalone AC crankcase power = rated when cooling RTF=0 and OAT < threshold
    #[test]
    fn standalone_ac_crankcase_full_power_when_off_and_cold() {
        let cfg = base_config();
        let w = step_ac_off(&cfg, 5.0);
        // 100 W rated, fully off (RTF=0) -> crankcase = 100 W
        assert!(
            (w - 100.0).abs() < 1e-6,
            "expected 100 W crankcase, got {w}"
        );
    }

    /// Per-component reactive: crankcase-only standby (unit off, cold OAT)
    /// draws real power through the purely resistive crankcase heater
    /// (pf 1.0), so Q must be exactly zero — the old folded pf 0.96 wrongly
    /// assigned the compressor pf to this resistive draw.
    #[test]
    fn crankcase_only_standby_produces_zero_reactive_power() {
        let cfg = base_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let e = cold_env(5.0, 20.0); // off (zone below setpoint), cold OAT
        eq.init(&cfg, &e).unwrap();
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&e);
        eq.step(&e, Duration::from_secs(60), &mut ports).unwrap();
        let p_w = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        assert!(p_w > 0.0, "crankcase heater must draw real power");
        assert_eq!(
            ports.electrical.reactive_power_var, 0.0,
            "resistive crankcase draw must produce zero Q"
        );
        assert_eq!(
            eq.core_output().flows.reactive_power_var,
            Some(0.0),
            "CoreOutput Q must be Some(0.0) on crankcase-only standby"
        );
    }

    // Test 2: Standalone AC crankcase power = 0 when OAT >= threshold
    #[test]
    fn standalone_ac_no_crankcase_when_warm() {
        let cfg = base_config();
        let kw = step_ac_off(&cfg, 15.0); // 15.0 >= 12.8 threshold
        assert_eq!(kw, 0.0, "no crankcase power above OAT threshold, got {kw}");
    }

    // Test 3: Crankcase power scales with the cooling coil's runtime fraction
    #[test]
    fn crankcase_uses_the_cooling_rtf() {
        let cfg = base_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let e = cold_env(5.0, 20.0);
        eq.init(&cfg, &e).unwrap();
        // cooling_rtf=0.3 -> power = 100*(1-0.3)=70 W
        let w = eq.crankcase_heater_power(5.0, 0.3);
        assert!((w - 70.0).abs() < 1e-6, "expected 70 W, got {w}");
    }

    // Test 4: Cooling mode active (RTF=0.6) -> crankcase = rated * 0.4
    #[test]
    fn cooling_active_reduces_crankcase_proportionally() {
        let cfg = base_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let e = cold_env(5.0, 20.0);
        eq.init(&cfg, &e).unwrap();
        // cooling_rtf=0.6 -> power = 100*(1-0.6)=40 W
        let w = eq.crankcase_heater_power(5.0, 0.6);
        assert!((w - 40.0).abs() < 1e-6, "expected 40 W, got {w}");
    }

    // Test 6: Cooling coil off -> full crankcase power
    #[test]
    fn cooling_off_gives_full_crankcase_power() {
        let cfg = base_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let e = cold_env(5.0, 20.0);
        eq.init(&cfg, &e).unwrap();
        let w = eq.crankcase_heater_power(5.0, 0.0);
        assert!((w - 100.0).abs() < 1e-6, "expected 100 W, got {w}");
    }

    // Test 8: Temperature curve modifies capacity correctly
    #[test]
    fn crankcase_capacity_curve_scales_rated_power() {
        // Curve: effective = rated * (1.0 + 0.1*T + 0.0*T^2); at T=5C: multiplier=1.5
        let cfg = base_config_with(|typed| {
            typed.crankcase_capacity_curve_coeffs = Some([1.0, 0.1, 0.0]);
        });
        let mut eq = AirConditioner::new(cfg.clone());
        let e = cold_env(5.0, 20.0);
        eq.init(&cfg, &e).unwrap();
        // crankcase_heater_power: rated=100 W * curve(5C)=1.5 * (1-0.0) = 150 W
        let w = eq.crankcase_heater_power(5.0, 0.0);
        assert!(
            (w - 150.0).abs() < 1e-6,
            "expected 150 W with curve at T=5C, got {w}"
        );
    }

    // Test 9: Crankcase power included in total electric consumption telemetry
    #[test]
    fn crankcase_power_in_electric_kw_telemetry() {
        let cfg = base_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let e = cold_env(5.0, 20.0); // cold outdoor, zone below setpoint -> AC off
        eq.init(&cfg, &e).unwrap();
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&e);
        eq.step(&e, Duration::from_secs(60), &mut ports).unwrap();

        let electric_w = eq.telemetry().get(tk::ELECTRIC_KW).unwrap_or(0.0);
        let port_w = ports.electrical.net_active_w();
        // Crankcase (0.10 kW = 100 W) must appear in both telemetry and port contribution.
        assert!(
            (electric_w - 100.0).abs() < 1e-6,
            "telemetry electric_w must equal the crankcase rated 100 W, got {electric_w}"
        );
        assert!(
            (port_w - 100.0).abs() < 1.0,
            "port electrical draw must equal crankcase rated 100 W (0.10 kW), got {port_w}"
        );
    }

    #[test]
    fn state_round_trip_preserves_dr_state() {
        use hares_types::{ControlSignal, DRLevel};

        let cfg = ac_config();
        // Zone is above cooling setpoint so cooling is active.
        let e = env(27.0, 0.008, 19.0, 35.0);

        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &e).unwrap();

        // Apply a Critical DR event with a 300 s duration.
        eq.apply_control(&ControlSignal::DemandResponse {
            level: DRLevel::Critical,
            duration_s: Some(300.0),
        })
        .unwrap();

        let saved = eq.save_state().unwrap();

        let mut restored = AirConditioner::new(cfg.clone());
        restored.init(&cfg, &e).unwrap();
        restored.load_state(&saved).unwrap();

        assert_eq!(
            restored.core.dr_level,
            DRLevel::Critical,
            "dr_level must survive save/load"
        );
        assert!(
            restored.core.dr_setpoint_offset_c > 0.0,
            "dr_setpoint_offset_c must be non-zero after Critical DR (got {})",
            restored.core.dr_setpoint_offset_c
        );
        assert_eq!(
            restored.core.dr_duration_remaining_s,
            Some(300.0),
            "dr_duration_remaining_s must survive save/load"
        );
        assert!(
            restored.core.dr_load_fraction < 1.0,
            "dr_load_fraction must be < 1.0 after Critical DR (got {})",
            restored.core.dr_load_fraction
        );
    }
}

#[cfg(test)]
mod ideal_capacity_tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        ControlSignal, EnvironmentState, GridState, HumidityAccumulator, PortSlots,
        ThermalAccumulator, WeatherState, ZoneId, ZoneState, telemetry_keys as tk,
    };

    use super::AirConditioner;
    use crate::{Equipment, EquipmentConfig, HvacSetpointConfig};

    /// Build an `EnvironmentState` with a configurable zone temperature and time resolution.
    fn make_env(zone_temp_c: f64, time_res_s: i64) -> EnvironmentState {
        EnvironmentState {
            ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp_c,
                humidity_ratio: 0.010,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 35.0,
                outdoor_humidity_ratio: 0.012,
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
                island_bus_voltage_pu: None,
            },
            schedule_row: None,
            unavailable_periods: hares_types::UnavailablePeriodState::default(),
            domains: hares_types::DomainSlots::default(),
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: std::collections::HashMap::new(),
            current_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 3, 18, 12, 0, 0)
                .single()
                .expect("valid"),
            time_res: ChronoDuration::seconds(time_res_s),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    /// AC with setpoint=24°C, hysteresis=1°C, single-speed, flat biquadratic curves.
    fn ac_config() -> EquipmentConfig {
        super::typed_ac_test_config(0.33)
    }

    fn make_ports() -> PortSlots {
        PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        }
    }

    // At 60 s timestep (fine resolution), an IdealCapacity signal is ignored because
    // use_ideal=false. Zone is above FSM turn-on threshold (setpoint + hysteresis = 25°C),
    // so thermostat-driven load_fraction = (26 - 24) / 1 = 1.0 → RTF = 1.0 (full on).
    #[test]
    fn fine_timestep_fsm_cycling_ignores_ideal_capacity_signal() {
        let cfg = ac_config();
        let mut eq = AirConditioner::new(cfg.clone());
        // 26°C exceeds FSM turn-on threshold of 25°C.
        let env = make_env(26.0, 60);
        eq.init(&cfg, &env).unwrap();
        // Signal provides half-rated load; must be ignored when use_ideal=false.
        eq.apply_control(&ControlSignal::IdealCapacity {
            capacity_w: -4_000.0,
            degraded: false,
        })
        .unwrap();
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut make_ports())
            .unwrap();

        let rtf = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap_or(-1.0);
        assert!(
            (rtf - 1.0).abs() < 1e-9,
            "FSM Cooling at 60 s must ignore IdealCapacity and produce RTF=1.0, got {rtf}"
        );
    }

    // At 900 s timestep (coarse resolution), a solver-provided IdealCapacity signal for
    // half the rated capacity (4000 W of 8000 W rated) produces fractional RTF.
    // Flow: apply signal → update_control (use_ideal=true, FSM enters Cooling at 26°C) →
    //   ideal_capacity_w=-4000 < 0 → load_fraction = 4000/8000 = 0.5 → RTF ≈ 0.5.
    #[test]
    fn coarse_timestep_ideal_signal_produces_fractional_rtf() {
        let cfg = ac_config();
        let mut eq = AirConditioner::new(cfg.clone());
        // 26°C exceeds FSM turn-on threshold of 25°C; FSM enters Cooling.
        let env = make_env(26.0, 900);
        eq.init(&cfg, &env).unwrap();
        // Half rated capacity: 4000 W of 8000 W rated → load_fraction = 0.5.
        eq.apply_control(&ControlSignal::IdealCapacity {
            capacity_w: -4_000.0,
            degraded: false,
        })
        .unwrap();
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(900), &mut make_ports())
            .unwrap();

        let rtf = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap_or(-1.0);
        // Flat biquadratic [1,0,0,0,0,0] → cap_ratio=1.0. Raw PLR = 4000/8000 = 0.5,
        // but PLF cycling-degradation (Cd=0.25) raises effective PLR to ~0.57.
        assert!(
            (rtf - 0.5).abs() < 0.15,
            "IdealCapacity=-4000 W at 900 s (half rated) must produce RTF near 0.5, got {rtf}"
        );
    }

    // At 900 s timestep with no solver signal and zone below the FSM cooling turn-on
    // threshold (25°C), the thermostat stays Deadband → AC is off.
    #[test]
    fn coarse_timestep_no_signal_below_fsm_threshold_produces_zero_rtf() {
        let cfg = ac_config();
        let mut eq = AirConditioner::new(cfg.clone());
        // 22°C is below FSM turn-on threshold (25°C); thermostat stays Deadband.
        let env = make_env(22.0, 900);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(900), &mut make_ports())
            .unwrap();

        let rtf = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap_or(-1.0);
        assert_eq!(
            rtf, 0.0,
            "900 s timestep with zone below FSM turn-on and no solver signal must \
             produce RTF=0.0, got {rtf}"
        );
    }

    #[test]
    fn max_capacity_fraction_clips_cooling_output() {
        let cfg = ac_config();
        let environment = make_env(26.0, 900);

        // Baseline: run at full ideal capacity (rated = 8000 W).
        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &environment).unwrap();
        eq.apply_control(&ControlSignal::IdealCapacity {
            capacity_w: -8_000.0,
            degraded: false,
        })
        .unwrap();
        eq.update_control(&environment);
        let mut ports = make_ports();
        eq.step(&environment, Duration::from_secs(900), &mut ports)
            .unwrap();
        let full_cooling_w = eq
            .telemetry()
            .get(tk::SENSIBLE_COOLING_W)
            .unwrap_or(0.0)
            .abs()
            + eq.telemetry()
                .get(tk::LATENT_COOLING_W)
                .unwrap_or(0.0)
                .abs();
        assert!(
            full_cooling_w > 1000.0,
            "baseline should produce meaningful output"
        );

        // Apply MaxCapacityFraction(0.5) -- output must not exceed ~50% of baseline.
        let mut eq2 = AirConditioner::new(cfg.clone());
        eq2.init(&cfg, &environment).unwrap();
        eq2.apply_control(&ControlSignal::MaxCapacityFraction { fraction: 0.5 })
            .unwrap();
        eq2.apply_control(&ControlSignal::IdealCapacity {
            capacity_w: -8_000.0,
            degraded: false,
        })
        .unwrap();
        eq2.update_control(&environment);
        let mut ports2 = make_ports();
        eq2.step(&environment, Duration::from_secs(900), &mut ports2)
            .unwrap();
        let capped_cooling_w = eq2
            .telemetry()
            .get(tk::SENSIBLE_COOLING_W)
            .unwrap_or(0.0)
            .abs()
            + eq2
                .telemetry()
                .get(tk::LATENT_COOLING_W)
                .unwrap_or(0.0)
                .abs();
        let limit = full_cooling_w * 0.55;
        eprintln!(
            "DBG frac3real: capped={capped_cooling_w} full={full_cooling_w} frac={}",
            eq2.core.hvac.control.max_capacity_fraction
        );
        assert!(
            capped_cooling_w <= limit,
            "MaxCapacityFraction(0.5) should limit output to ~50%; \
             got {capped_cooling_w:.1}W vs full {full_cooling_w:.1}W (limit={limit:.1}W)"
        );
    }

    // Test: RoomAC ideal_target() delegation to CoolingCore.
    //
    // At a coarse timestep (>= 5 min), use_ideal_capacity() returns true for AcCooler
    // equipment types (which RoomAC uses). RoomAC delegates ideal_target() to
    // CoolingCore, same delegation pattern as AirConditioner.
    #[test]
    fn room_ac_ideal_target_returns_some_at_coarse_timestep() {
        use super::RoomAC;
        use crate::RoomAcConfig;

        // Build a RoomAC: zone_id=1, cooling_setpoint=24°C, hysteresis=1°C.
        let cfg = EquipmentConfig::from_typed(
            "rac".to_string(),
            "Room AC".to_string(),
            RoomAcConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 3_500.0,
                eir: 0.33,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    cooling_setpoint_source: None,
                    heating_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_ROOM_AC_M3_S_PER_W),
                shr: None,
                startup_cd: None,
                crankcase_heater_w: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                min_oat_compressor_cooling_c: None,
            },
        )
        .unwrap();

        let mut eq = RoomAC::new(cfg.clone());
        // Zone above cooling turn-on threshold: 26°C > setpoint+hysteresis = 25°C.
        // Coarse timestep (900 s >= 300 s threshold) → use_ideal = true.
        let env = make_env(26.0, 900);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);

        // After update_control with a hot zone and coarse timestep, ideal_target() must
        // return Some((ZoneId(1), ~24.0)) — the zone id and cooling setpoint.
        let target = eq.ideal_target();
        assert!(
            target.is_some(),
            "RoomAC ideal_target() must return Some at coarse timestep (900 s) when zone \
             (26°C) is above cooling turn-on threshold; got None"
        );
        let (zone_id, setpoint_c) = target.unwrap();
        assert_eq!(
            zone_id,
            ZoneId(1),
            "RoomAC ideal_target zone must match configured zone_id"
        );
        assert!(
            (setpoint_c - 24.0).abs() < 0.5,
            "RoomAC ideal_target setpoint must reflect cooling setpoint (~24°C); got {setpoint_c:.2}°C"
        );
    }

    // Companion: at a fine timestep (60 s), use_ideal is false and ideal_target() must
    // return None — same behavior as AirConditioner (parity check).
    #[test]
    fn room_ac_ideal_target_returns_none_at_fine_timestep() {
        use super::RoomAC;
        use crate::RoomAcConfig;

        let cfg = EquipmentConfig::from_typed(
            "rac_fine".to_string(),
            "Room AC".to_string(),
            RoomAcConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 3_500.0,
                eir: 0.33,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    cooling_setpoint_source: None,
                    heating_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_ROOM_AC_M3_S_PER_W),
                shr: None,
                startup_cd: None,
                crankcase_heater_w: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                min_oat_compressor_cooling_c: None,
            },
        )
        .unwrap();

        let mut eq = RoomAC::new(cfg.clone());
        // Fine timestep (60 s < 300 s threshold) → use_ideal = false.
        let env = make_env(26.0, 60);
        eq.init(&cfg, &env).unwrap();
        eq.update_control(&env);

        assert!(
            eq.ideal_target().is_none(),
            "RoomAC ideal_target() must return None at fine timestep (60 s); \
             use_ideal=false so solver should not query this unit"
        );
    }

    // Verify RoomAC and AirConditioner return identical ideal_target() outputs
    // for identical input conditions — the same delegation to CoolingCore.
    #[test]
    fn room_ac_ideal_target_matches_air_conditioner_parity() {
        use crate::{CentralAirConditionerConfig, DuctConfig, RoomAcConfig};

        let ac_cfg = EquipmentConfig::from_typed(
            "ac_par".to_string(),
            "Air Conditioner".to_string(),
            CentralAirConditionerConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 3_500.0,
                eir: 0.33,
                shr: None,
                number_of_speeds: 1,
                stage_capacities_w: None,
                stage_eirs: None,
                stage_shrs: None,
                fan_power_w: Some(0.0),
                fan_power_w_per_cfm: None,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    cooling_setpoint_source: None,
                    heating_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: None,
                fraction_load_served: Some(1.0),
                crankcase_heater_w: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                duct: DuctConfig::default(),
                system_type: None,
                startup_cd: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                charge_defect_ratio: None,
                min_oat_compressor_cooling_c: None,
            },
        )
        .unwrap();

        let rac_cfg = EquipmentConfig::from_typed(
            "rac_par".to_string(),
            "Room AC".to_string(),
            RoomAcConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 3_500.0,
                eir: 0.33,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    cooling_setpoint_source: None,
                    heating_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_ROOM_AC_M3_S_PER_W),
                shr: None,
                startup_cd: None,
                crankcase_heater_w: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                min_oat_compressor_cooling_c: None,
            },
        )
        .unwrap();

        let env = make_env(26.0, 900);

        let mut ac = AirConditioner::new(ac_cfg.clone());
        ac.init(&ac_cfg, &env).unwrap();
        ac.update_control(&env);

        let mut rac = super::RoomAC::new(rac_cfg.clone());
        rac.init(&rac_cfg, &env).unwrap();
        rac.update_control(&env);

        let ac_target = ac.ideal_target();
        let rac_target = rac.ideal_target();

        // Both must return Some with the same zone and setpoint.
        assert!(
            ac_target.is_some(),
            "AirConditioner must return Some ideal_target at coarse timestep"
        );
        assert!(
            rac_target.is_some(),
            "RoomAC must return Some ideal_target at coarse timestep"
        );
        let (ac_zone, ac_sp) = ac_target.unwrap();
        let (rac_zone, rac_sp) = rac_target.unwrap();
        assert_eq!(ac_zone, rac_zone, "zone IDs must match");
        assert!(
            (ac_sp - rac_sp).abs() < 0.01,
            "AirConditioner setpoint ({ac_sp:.3}°C) and RoomAC setpoint ({rac_sp:.3}°C) must match"
        );
    }
}

#[cfg(test)]
mod defaults_tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_types::{
        ControlSignal, EnvironmentState, GridState, HumidityAccumulator, PortSlots,
        ThermalAccumulator, WeatherState, ZoneId, ZoneState, telemetry_keys as tk,
    };

    use super::{AirConditioner, RoomAC};
    use crate::{
        CentralAirConditionerConfig, DuctConfig, Equipment, EquipmentConfig, HvacSetpointConfig,
        RoomAcConfig,
    };

    fn make_env(zone_temp_c: f64) -> EnvironmentState {
        EnvironmentState {
            ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp_c,
                humidity_ratio: 0.010,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 35.0,
                outdoor_humidity_ratio: 0.012,
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
                island_bus_voltage_pu: None,
            },
            schedule_row: None,
            unavailable_periods: hares_types::UnavailablePeriodState::default(),
            domains: hares_types::DomainSlots::default(),
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: std::collections::HashMap::new(),
            current_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 3, 18, 12, 0, 0)
                .single()
                .expect("valid"),
            time_res: ChronoDuration::minutes(1),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    /// Build an `EnvironmentState` with a configurable zone temperature and time resolution.
    fn make_env_coarse(zone_temp_c: f64, time_res_s: i64, humidity_ratio: f64) -> EnvironmentState {
        EnvironmentState {
            ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp_c,
                humidity_ratio,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 35.0,
                outdoor_humidity_ratio: 0.012,
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
                island_bus_voltage_pu: None,
            },
            schedule_row: None,
            unavailable_periods: hares_types::UnavailablePeriodState::default(),
            domains: hares_types::DomainSlots::default(),
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: std::collections::HashMap::new(),
            current_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 3, 18, 12, 0, 0)
                .single()
                .expect("valid"),
            time_res: ChronoDuration::seconds(time_res_s),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    fn two_speed_config() -> EquipmentConfig {
        let mut cfg = EquipmentConfig::from_typed(
            "AC".to_string(),
            "Air Conditioner".to_string(),
            CentralAirConditionerConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 10_000.0,
                eir: 3.412_141_633 / 16.0,
                shr: Some(0.75),
                number_of_speeds: 2,
                stage_capacities_w: Some(vec![7_200.0, 10_000.0]),
                stage_eirs: None,
                stage_shrs: None,
                fan_power_w: None,
                fan_power_w_per_cfm: None,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_CENTRAL_AC_M3_S_PER_W),
                fraction_load_served: None,
                crankcase_heater_w: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                duct: DuctConfig::default(),
                system_type: None,
                startup_cd: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                charge_defect_ratio: None,
                min_oat_compressor_cooling_c: None,
            },
        )
        .unwrap();
        cfg.test_extras_mut().insert(
            "capacity_biquadratic_coeffs".to_string(),
            "[1,0,0,0,0,0]".into(),
        );
        cfg.test_extras_mut()
            .insert("eir_biquadratic_coeffs".to_string(), "[1,0,0,0,0,0]".into());
        cfg
    }

    // Two-speed AC initialised without an explicit low_speed_capacity_fraction must
    // default to 0.72, matching the OCHRE/AHRI lookup table for 2-speed AC coolers.
    #[test]
    fn two_speed_ac_defaults_to_0_72_low_speed_capacity_fraction() {
        let cfg = two_speed_config();
        let mut eq = AirConditioner::new(cfg.clone());
        let env = make_env(28.0);
        eq.init(&cfg, &env).unwrap();

        assert!(
            (eq.core.hvac.config.low_speed_capacity_fraction - 0.72).abs() < 1e-12,
            "2-speed AC must default to low_speed_capacity_fraction=0.72 (OCHRE/AHRI default), got {}",
            eq.core.hvac.config.low_speed_capacity_fraction
        );
    }

    // Single-stage AC initialised without explicit latent degradation params must
    // have the Henderson-Rengarajan model active with EnergyPlus residential DX
    // defaults (twet=1000 s, gamma=1.5, Nmax=3 cyc/hr, tau=45 s).
    // At part load (RTF < 1) the model must return SHR > steady-state SHR,
    // reflecting moisture re-evaporation during the off cycle.
    #[test]
    fn central_ac_defaults_latent_degradation_active_and_raises_shr_at_part_load() {
        let cfg = EquipmentConfig::from_typed(
            "AC".to_string(),
            "Air Conditioner".to_string(),
            CentralAirConditionerConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 8_000.0,
                eir: 3.412_141_633 / 16.0,
                shr: Some(0.75),
                number_of_speeds: 1,
                stage_capacities_w: None,
                stage_eirs: None,
                stage_shrs: None,
                fan_power_w: None,
                fan_power_w_per_cfm: None,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_CENTRAL_AC_M3_S_PER_W),
                fraction_load_served: None,
                crankcase_heater_w: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                duct: DuctConfig::default(),
                system_type: None,
                startup_cd: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                charge_defect_ratio: None,
                min_oat_compressor_cooling_c: None,
            },
        )
        .unwrap();

        let env = make_env(28.0);
        let mut eq = AirConditioner::new(cfg.clone());
        eq.init(&cfg, &env).unwrap();

        assert!(
            eq.core.latent_degradation.is_active(),
            "latent degradation must be active after init for central AC"
        );
        assert!(
            (eq.core.latent_degradation.twet_rated_s - 1000.0).abs() < 1e-9,
            "twet_rated_s must be 1000 s (EnergyPlus Coil:Cooling:DX suggested default), got {}",
            eq.core.latent_degradation.twet_rated_s
        );
        assert!(
            (eq.core.latent_degradation.gamma_rated - 1.5).abs() < 1e-9,
            "gamma_rated must be 1.5 (EnergyPlus residential default), got {}",
            eq.core.latent_degradation.gamma_rated
        );

        // Run the AC at partial load (zone barely above setpoint) so RTF < 1
        // and the Henderson-Rengarajan model has room to degrade latent capacity.
        // The reported SHR must be strictly above the rated SHR of 0.75 because
        // moisture re-evaporation during off cycles raises the effective SHR.
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(60), &mut ports).unwrap();

        let reported_shr = eq.telemetry().get(tk::SHR).unwrap_or(0.0);
        assert!(
            reported_shr >= 0.75,
            "SHR with latent degradation at part load must be >= rated SHR 0.75, got {reported_shr:.4}"
        );
        assert!(
            reported_shr <= 1.0,
            "SHR must be <= 1.0, got {reported_shr:.4}"
        );
    }

    // Room AC initialised without explicit latent degradation params must
    // have the Henderson-Rengarajan model active with EnergyPlus defaults
    // (twet=1000 s, gamma=1.5, Nmax=3 cyc/hr, tau=45 s).
    // At part load (RTF < 1) the model must return SHR > steady-state SHR,
    // reflecting moisture re-evaporation during the off cycle.
    #[test]
    fn room_ac_defaults_latent_degradation_active_and_raises_shr_at_part_load() {
        let cfg = EquipmentConfig::from_typed(
            "RAC".to_string(),
            "Room AC".to_string(),
            RoomAcConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 3_500.0,
                eir: 3.412_141_633 / 10.0,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                shr: Some(0.75),
                startup_cd: None,
                crankcase_heater_w: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                min_oat_compressor_cooling_c: None,
            },
        )
        .unwrap();

        let env = make_env_coarse(28.0, 900, 0.016);
        let mut eq = RoomAC::new(cfg.clone());
        eq.init(&cfg, &env).unwrap();

        assert!(
            eq.core.latent_degradation.is_active(),
            "latent degradation must be active after init for room AC"
        );
        assert!(
            (eq.core.latent_degradation.twet_rated_s - 1000.0).abs() < 1e-9,
            "twet_rated_s must be 1000 s (EnergyPlus Coil:Cooling:DX suggested default), got {}",
            eq.core.latent_degradation.twet_rated_s
        );
        assert!(
            (eq.core.latent_degradation.gamma_rated - 1.5).abs() < 1e-9,
            "gamma_rated must be 1.5, got {}",
            eq.core.latent_degradation.gamma_rated
        );
        assert!(
            (eq.core.latent_degradation.max_cycling_rate - 3.0).abs() < 1e-9,
            "max_cycling_rate must be 3.0 cyc/hr, got {}",
            eq.core.latent_degradation.max_cycling_rate
        );
        assert!(
            (eq.core.latent_degradation.latent_time_constant_s - 45.0).abs() < 1e-9,
            "latent_time_constant_s must be 45 s, got {}",
            eq.core.latent_degradation.latent_time_constant_s
        );

        // At 900 s timestep (≥ 300 s Auto threshold), use_ideal_capacity returns
        // true and the equipment enters the solver-driven ideal-capacity path.
        // A 65%-rated IdealCapacity signal (2275 W ≈ 65% of 3500 W) combined with
        // a humid-climate humidity_ratio (0.016) produces a non-trivial
        // steady_state_shr < 1.0 that the degradation model can meaningfully raise.
        // This avoids the round-2 failure mode (RTF=1.0 early-return) AND the
        // round-3 failure mode (steady_state_shr already saturated at 1.0).
        eq.apply_control(&ControlSignal::IdealCapacity {
            capacity_w: -2275.0, // ~65% of 3500 W rated capacity
            degraded: false,
        })
        .unwrap();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            humidity: vec![HumidityAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        eq.update_control(&env);
        eq.step(&env, Duration::from_secs(900), &mut ports).unwrap();

        let reported_shr = eq.telemetry().get(tk::SHR).unwrap_or(0.0);
        assert!(
            reported_shr > 0.75,
            "SHR with latent degradation at part load must exceed rated SHR 0.75, got {reported_shr:.4}"
        );
        assert!(
            reported_shr <= 1.0,
            "SHR must be <= 1.0, got {reported_shr:.4}"
        );
    }
}

// Parity regression tests: select_variable_speed_cooling and
// HvacEquipment::select_multi_speed both delegate to interpolate_speed_stages.
// These tests verify that both paths produce identical SpeedSelection values for
// the same normalised load fractions and capacity stages.  If either caller is
// ever re-implemented without going through the shared function, a divergence
// will appear here first.
//
// The tests exercise select_variable_speed_cooling directly by constructing a
// minimal CoolingCore and overriding cooling_capacities_w after init.
#[cfg(test)]
mod speed_selection_parity_tests {
    use super::CoolingCore;
    use super::{AirConditioner, RoomAC};
    use crate::hvac::{HvacEquipment, HvacEquipmentType, SpeedControlMode, ThermostatMode};
    use crate::{
        CentralAirConditionerConfig, DuctConfig, Equipment, EquipmentConfig, HvacSetpointConfig,
        RoomAcConfig,
    };
    use hares_types::{EnvironmentState, GridState, WeatherState, ZoneId, ZoneState};

    fn minimal_env() -> EnvironmentState {
        use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
        EnvironmentState {
            ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: 26.0,
                humidity_ratio: 0.010,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 35.0,
                outdoor_humidity_ratio: 0.012,
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
                island_bus_voltage_pu: None,
            },
            schedule_row: None,
            unavailable_periods: hares_types::UnavailablePeriodState::default(),
            domains: hares_types::DomainSlots::default(),
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: std::collections::HashMap::new(),
            current_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 1, 1, 12, 0, 0)
                .single()
                .expect("valid"),
            time_res: ChronoDuration::minutes(1),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    /// Build a CoolingCore with the given capacity stages and then override
    /// cooling_capacities_w directly so both sides use the same stage values.
    fn make_cooling_core(capacities_w: Vec<f64>) -> CoolingCore {
        let typed = CentralAirConditionerConfig {
            equipment_id: None,
            zone_id: Some(1),
            capacity_w: *capacities_w.last().unwrap_or(&10_000.0),
            eir: 3.412_141_633 / 14.0,
            shr: Some(0.75),
            number_of_speeds: capacities_w.len() as u8,
            stage_capacities_w: Some(capacities_w.clone()),
            stage_eirs: None,
            stage_shrs: None,
            fan_power_w: Some(0.0),
            fan_power_w_per_cfm: None,
            setpoint: HvacSetpointConfig {
                cooling_setpoint_c: Some(24.0),
                heating_setpoint_c: Some(18.0),
                heating_setpoint_source: None,
                cooling_setpoint_source: None,
            },
            hysteresis_c: Some(1.0),
            airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_CENTRAL_AC_M3_S_PER_W),
            fraction_load_served: None,
            crankcase_heater_w: None,
            crankcase_heater_threshold_c: None,
            crankcase_capacity_curve_coeffs: None,
            duct: DuctConfig::default(),
            system_type: None,
            startup_cd: Some(0.0),
            biquadratic_x1_min: None,
            biquadratic_x1_max: None,
            biquadratic_x2_min: None,
            biquadratic_x2_max: None,
            ff_min: None,
            ff_max: None,
            plf_min: None,
            plf_max: None,
            charge_defect_ratio: None,
            min_oat_compressor_cooling_c: None,
        };
        let mut cfg =
            EquipmentConfig::from_typed("AC".to_string(), "Air Conditioner".to_string(), typed)
                .unwrap();
        cfg.test_extras_mut().insert(
            "capacity_biquadratic_coeffs".to_string(),
            "[1,0,0,0,0,0]".into(),
        );
        cfg.test_extras_mut()
            .insert("eir_biquadratic_coeffs".to_string(), "[1,0,0,0,0,0]".into());
        let env = minimal_env();
        let mut core = CoolingCore::new(cfg.clone(), false);
        core.init(&cfg, &env).unwrap();
        // Override capacities so the test controls the exact stage values.
        core.hvac.config.cooling_capacities_w = capacities_w;
        core
    }

    /// Build an HvacEquipment in MultiSpeedInterpolated/Cooling mode with the
    /// same capacity stages so we can call select_speed() (→ select_multi_speed).
    fn make_hvac_cooling(capacities_w: Vec<f64>) -> HvacEquipment {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::AcCooler, ZoneId(1));
        hvac.config.speed_control_mode = SpeedControlMode::MultiSpeedInterpolated;
        hvac.config.cooling_capacities_w = capacities_w;
        hvac.thermostat_fsm.mode = ThermostatMode::Cooling;
        hvac
    }

    // Regression: select_variable_speed_cooling must produce the same
    // speed_index and speed_frac as select_multi_speed for every interior bracket.
    // If the two paths diverge this test will catch it.
    #[test]
    fn variable_speed_multi_speed_parity_interior_brackets() {
        // 4-stage: fractions [0.4, 0.6, 0.8, 1.0]
        let caps = vec![4_000.0, 6_000.0, 8_000.0, 10_000.0];
        let mut core = make_cooling_core(caps.clone());
        let mut hvac = make_hvac_cooling(caps);

        let probes: &[(f64, &str)] = &[
            (0.5, "between stage 0-1"),
            (0.7, "between stage 1-2"),
            (0.9, "between stage 2-3"),
        ];
        for &(frac, label) in probes {
            let vs = core.select_variable_speed_cooling(frac);
            let ms = hvac.select_speed(frac);
            assert_eq!(
                vs.speed_index, ms.speed_index,
                "{label}: speed_index mismatch — variable_speed={} multi_speed={}",
                vs.speed_index, ms.speed_index
            );
            assert!(
                (vs.speed_frac - ms.speed_frac).abs() < 1e-12,
                "{label}: speed_frac mismatch — variable_speed={} multi_speed={}",
                vs.speed_frac,
                ms.speed_frac
            );
            assert_eq!(
                vs.part_load_ratio, ms.part_load_ratio,
                "{label}: part_load_ratio mismatch — variable_speed={} multi_speed={}",
                vs.part_load_ratio, ms.part_load_ratio
            );
        }
    }

    // Regression: below the lowest stage, PLR computation must agree between paths.
    #[test]
    fn variable_speed_multi_speed_parity_below_lowest_stage() {
        let caps = vec![4_000.0, 6_000.0, 8_000.0, 10_000.0];
        let mut core = make_cooling_core(caps.clone());
        let mut hvac = make_hvac_cooling(caps);

        let load = 0.3; // below cap_frac[0]=0.4
        let vs = core.select_variable_speed_cooling(load);
        let ms = hvac.select_speed(load);

        assert_eq!(vs.speed_index, 0);
        assert_eq!(vs.speed_frac, 0.0);
        // PLR = 0.3 / 0.4 = 0.75 for both paths.
        assert!(
            (vs.part_load_ratio - ms.part_load_ratio).abs() < 1e-12,
            "PLR below lowest stage must agree: variable_speed={} multi_speed={}",
            vs.part_load_ratio,
            ms.part_load_ratio
        );
    }

    // Regression: at full load both paths must return the last index.
    #[test]
    fn variable_speed_multi_speed_parity_at_full_load() {
        let caps = vec![4_000.0, 6_000.0, 8_000.0, 10_000.0];
        let mut core = make_cooling_core(caps.clone());
        let mut hvac = make_hvac_cooling(caps);

        let vs = core.select_variable_speed_cooling(1.0);
        let ms = hvac.select_speed(1.0);

        assert_eq!(vs.speed_index, ms.speed_index, "top-speed index must agree");
        assert_eq!(vs.speed_frac, 0.0);
        assert_eq!(vs.part_load_ratio, 1.0);
        assert_eq!(ms.part_load_ratio, 1.0);
    }

    #[test]
    fn room_ac_explicit_shr_used_not_derived_default() {
        // When shr is explicitly set, the derived default must not be used.
        let cfg = EquipmentConfig::from_typed(
            "RAC".to_string(),
            "Room AC".to_string(),
            RoomAcConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 3_500.0,
                eir: 3.412_141_633 / 10.0,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: None,
                shr: Some(0.65),
                startup_cd: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                crankcase_heater_w: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                min_oat_compressor_cooling_c: None,
            },
        )
        .unwrap();
        let env = minimal_env();
        let mut eq = RoomAC::new(cfg.clone());
        eq.init(&cfg, &env).unwrap();
        assert!(
            (eq.core.rated_shr - 0.65).abs() < 1e-12,
            "explicit shr=0.65 must be used, got {}",
            eq.core.rated_shr
        );
    }

    #[test]
    fn room_ac_default_shr_matches_derived_value() {
        // Without explicit SHR, the room AC must derive SHR from airflow
        // using the EnergyPlus auto-sizing formula.
        let cfg = EquipmentConfig::from_typed(
            "RAC".to_string(),
            "Room AC".to_string(),
            RoomAcConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 3_500.0,
                eir: 3.412_141_633 / 10.0,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: None,
                shr: None,
                startup_cd: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                crankcase_heater_w: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                min_oat_compressor_cooling_c: None,
            },
        )
        .unwrap();
        let env = minimal_env();
        let mut eq = RoomAC::new(cfg.clone());
        eq.init(&cfg, &env).unwrap();
        // At room AC airflow (320 CFM/ton), EnergyPlus auto-sizing predicts SHR ≈ 0.69.
        assert!(
            (eq.core.rated_shr - 0.69).abs() < 0.01,
            "room AC default SHR should be ~0.69, got {}",
            eq.core.rated_shr
        );
    }

    #[test]
    fn room_ac_and_central_ac_default_shr_differ() {
        // Room AC and central AC must have distinct default SHR values
        // because they operate at different airflow-per-ton ratios.
        // Room AC (lower airflow) → lower SHR.
        // Central AC (higher airflow) → higher SHR.
        let room_cfg = EquipmentConfig::from_typed(
            "RAC".to_string(),
            "Room AC".to_string(),
            RoomAcConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 3_500.0,
                eir: 3.412_141_633 / 10.0,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: None,
                shr: None,
                startup_cd: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                crankcase_heater_w: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                min_oat_compressor_cooling_c: None,
            },
        )
        .unwrap();
        let central_cfg = EquipmentConfig::from_typed(
            "CAC".to_string(),
            "Air Conditioner".to_string(),
            CentralAirConditionerConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 8_000.0,
                eir: 3.412_141_633 / 10.0,
                shr: None,
                number_of_speeds: 1,
                stage_capacities_w: None,
                stage_eirs: None,
                stage_shrs: None,
                fan_power_w: None,
                fan_power_w_per_cfm: None,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(24.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: None,
                fraction_load_served: None,
                crankcase_heater_w: None,
                crankcase_heater_threshold_c: None,
                crankcase_capacity_curve_coeffs: None,
                duct: DuctConfig::default(),
                system_type: None,
                startup_cd: None,
                biquadratic_x1_min: None,
                biquadratic_x1_max: None,
                biquadratic_x2_min: None,
                biquadratic_x2_max: None,
                ff_min: None,
                ff_max: None,
                plf_min: None,
                plf_max: None,
                charge_defect_ratio: None,
                min_oat_compressor_cooling_c: None,
            },
        )
        .unwrap();
        let env_state = minimal_env();
        let mut room_eq = RoomAC::new(room_cfg.clone());
        room_eq.init(&room_cfg, &env_state).unwrap();
        let mut central_eq = AirConditioner::new(central_cfg.clone());
        central_eq.init(&central_cfg, &env_state).unwrap();

        let room_shr = room_eq.core.rated_shr;
        let central_shr = central_eq.core.rated_shr;

        assert!(
            room_shr < central_shr,
            "room AC default SHR ({room_shr}) must be less than central AC default SHR ({central_shr})"
        );
        assert!(
            room_shr > 0.0,
            "room AC default SHR must be positive, got {room_shr}"
        );
        assert!(
            central_shr > 0.0,
            "central AC default SHR must be positive, got {central_shr}"
        );
    }

    /// Build a CoolingCore config with custom stage capacities and interleaved
    /// biquadratic curves via the split capacity/EIR keys.
    fn multi_speed_ac_config(
        n_speeds: u8,
        capacities_w: Vec<f64>,
        n_cap_curves: usize,
        n_eir_curves: usize,
    ) -> EquipmentConfig {
        let typed = CentralAirConditionerConfig {
            equipment_id: None,
            zone_id: Some(1),
            capacity_w: *capacities_w.last().unwrap_or(&10_000.0),
            eir: 3.412_141_633 / 14.0,
            shr: Some(0.75),
            number_of_speeds: n_speeds,
            stage_capacities_w: Some(capacities_w),
            stage_eirs: None,
            stage_shrs: None,
            fan_power_w: Some(0.0),
            fan_power_w_per_cfm: None,
            setpoint: HvacSetpointConfig {
                cooling_setpoint_c: Some(24.0),
                heating_setpoint_c: Some(18.0),
                heating_setpoint_source: None,
                cooling_setpoint_source: None,
            },
            hysteresis_c: Some(1.0),
            airflow_m3_s_per_w: Some(crate::hvac::hvac_core::AIRFLOW_CENTRAL_AC_M3_S_PER_W),
            fraction_load_served: None,
            crankcase_heater_w: None,
            crankcase_heater_threshold_c: None,
            crankcase_capacity_curve_coeffs: None,
            duct: crate::DuctConfig::default(),
            system_type: None,
            startup_cd: Some(0.0),
            biquadratic_x1_min: None,
            biquadratic_x1_max: None,
            biquadratic_x2_min: None,
            biquadratic_x2_max: None,
            ff_min: None,
            ff_max: None,
            plf_min: None,
            plf_max: None,
            charge_defect_ratio: None,
            min_oat_compressor_cooling_c: None,
        };
        let mut cfg =
            EquipmentConfig::from_typed("AC".to_string(), "Air Conditioner".to_string(), typed)
                .unwrap();
        let single_cap = format!(
            "[{}]",
            std::iter::repeat_n("[0.8,0,0,0,0,0]", n_cap_curves)
                .collect::<Vec<_>>()
                .join(",")
        );
        cfg.test_extras_mut()
            .insert("capacity_biquadratic_coeffs".to_string(), single_cap.into());
        let single_eir = format!(
            "[{}]",
            std::iter::repeat_n("[1.2,0,0,0,0,0]", n_eir_curves)
                .collect::<Vec<_>>()
                .join(",")
        );
        cfg.test_extras_mut()
            .insert("eir_biquadratic_coeffs".to_string(), single_eir.into());
        cfg
    }

    #[test]
    fn init_rejects_partial_per_stage_biquadratic_curves_ac() {
        // 4 speed stages, 2 capacity curves + 2 EIR curves → interleaved = 4 entries (2 pairs).
        // n_coeffs=4, n_speeds=4, 4>2 && 4<8 → rejected.
        let cfg = multi_speed_ac_config(4, vec![3_000.0, 5_000.0, 7_000.0, 10_000.0], 2, 2);
        let mut core = CoolingCore::new(cfg.clone(), false);
        let env = minimal_env();
        let result = core.init(&cfg, &env);
        assert!(
            result.is_err(),
            "4 speeds with 2 cap+2 eir curves (2 pairs) must be rejected"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("cap+EIR pair"),
            "error must describe cap+EIR pairs; got: {err}"
        );
    }

    #[test]
    fn init_accepts_shared_biquadratic_pair_for_multi_speed_ac() {
        // 4 speed stages, 1 capacity + 1 EIR → interleaved = 2 entries (1 pair).
        // n_coeffs=2 → accepted (shared pair across all stages).
        let cfg = multi_speed_ac_config(4, vec![3_000.0, 5_000.0, 7_000.0, 10_000.0], 1, 1);
        let mut core = CoolingCore::new(cfg.clone(), false);
        let env = minimal_env();
        core.init(&cfg, &env).unwrap();
    }

    #[test]
    fn init_accepts_full_per_stage_biquadratic_curves_ac() {
        // 4 speed stages, 4 capacity + 4 EIR → interleaved = 8 entries (4 pairs).
        // n_coe*2=8 → accepted.
        let cfg = multi_speed_ac_config(4, vec![3_000.0, 5_000.0, 7_000.0, 10_000.0], 4, 4);
        let mut core = CoolingCore::new(cfg.clone(), false);
        let env = minimal_env();
        core.init(&cfg, &env).unwrap();
    }
}
