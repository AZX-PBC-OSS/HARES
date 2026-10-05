//! Dwelling orchestrator: integrates environment, equipment, solvers, and output.

mod autosize;
pub mod blueprint;
mod checkpointing;
mod conversions;
mod loop_allocator;
mod premise_zip;
mod roster;
mod solver_builder;
mod synthetic;
mod warnings;

pub use warnings::WarningLog;

pub use premise_zip::PremiseZip;

pub use blueprint::DwellingBlueprint;
pub use conversions::{
    building_to_boundary_inputs, building_to_zone_inputs, mass_multiplier_for_zone, stage_rank,
};

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration as StdDuration;
#[cfg(feature = "profiling")]
use std::time::Instant;

use arrow::datatypes::{Field, Schema};
use chrono::{DateTime, Duration, FixedOffset, Timelike};
use hares_control::{
    DispatchRequest, DispatchTarget, PRIORITY_TIER_COUNT, PriceSignal, PriorityTier,
};
#[cfg(any(debug_assertions, feature = "observe_detailed"))]
use hares_envelope::EnvelopeDiagnostics;
use hares_envelope::{
    ElectricalSolver, FluidSolver, FluidSolverConfig, HumiditySolver, ThermalSolver,
};
use hares_equipment::{
    ActorSeed, BatteryLutType, Equipment, EquipmentConfig, EquipmentRegistry, OcvTable,
    RegularGridInterpolator, SetpointReconciliation, UNegTable,
};
use hares_io::{
    Building, CAPACITY_SUFFIX, COMPRESSOR_POWER_KW_SUFFIX, COMPRESSOR_POWER_W_SUFFIX, COP_SUFFIX,
    DEFROST_STATE_SUFFIX, ELECTRIC_POWER_SUFFIX, ENERGY_SUFFIX, ER_CAPACITY_SUFFIX,
    ER_POWER_SUFFIX, EV_CHARGING_LEVEL_SUFFIX, EV_CONNECTION_STATE_SUFFIX,
    FAN_ELECTRIC_POWER_SUFFIX, FAN_POWER_SUFFIX, FAN_POWER_W_SUFFIX, FullSimulationMetrics,
    GAS_POWER_SUFFIX, HP_CAPACITY_SUFFIX, HVAC_DUCT_LOSSES_COL, LATENT_GAINS_SUFFIX,
    MAIN_POWER_SUFFIX, MIN_OFF_TIME_SUFFIX, MIN_ON_TIME_SUFFIX, MODE_SUFFIX,
    PAN_HEATER_POWER_SUFFIX, POWER_FACTOR_SUFFIX, PV_DC_POWER_SUFFIX, PV_IRRADIANCE_SUFFIX,
    PvPanelDefaults, REACTIVE_POWER_SUFFIX, RETURN_TEMP_SUFFIX, RUNTIME_COOLING_SETPOINT_COL,
    RUNTIME_FRACTION_SUFFIX, RUNTIME_HEATING_SETPOINT_COL, SCHEDULE_SUFFIX,
    SCHEDULED_COOLING_SETPOINT_COL, SCHEDULED_HEATING_SETPOINT_COL, SETPOINT_SUFFIX, SHR_SUFFIX,
    SOC_SUFFIX, SPEED_SUFFIX, SUPPLY_AIR_TEMP_SUFFIX, SUPPLY_TEMP_SUFFIX, ScheduleTimeSeries,
    SimulationConfig, StreamingRecorder, WeatherTimeSeries, end_use_electric_power_column,
    equipment_name_to_end_use, extract_unit_from_name, has_soc, is_cooling_equipment, is_ev,
    is_heat_pump_heater, is_hvac_or_wh, is_pv, is_reserved_output_column_name, parse_hpxml,
    parse_schedule_csv, parse_weather,
};

use hares_physics::constants::{
    GAS_THERMS_PER_HOUR_TO_W, J_PER_KWH, OCCUPANT_CONVECTIVE_FRACTION, OCCUPANT_LATENT_GAIN_W,
    OCCUPANT_RADIATIVE_FRACTION, OCCUPANT_SENSIBLE_GAIN_W, SECONDS_PER_HOUR,
};
use hares_physics::pv_sizing::RoofInfo;
use hares_physics::units::power_w_to_kw;
use hares_tariff::{BillingPeriodSummary, TariffEvaluator};
#[cfg(test)]
use hares_types::ElectricPower;
use hares_types::rng::RngStream;
use hares_types::{
    ALL_FUEL_TYPES, BmsMode, ChargingStrategy, ControlSignal, DomainSolver, ElectricalSummary,
    EndUse, EnvironmentState, EquipmentId, ExecutionStage, GridState, HaresError, OperatingMode,
    PortContribution, PortDeclaration, PortSlots, SCHEDULE_DOMAIN_ID, ScheduleSource,
    ThermalCategory, Warning, ZoneId, ZoneMap, ZoneRole, telemetry_keys as tk,
    validate_core_contract,
};
use hares_types::{ControlCapabilities, validate_port_core_electrical_consistency};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use serde_json::{Map, Value};

use crate::actors::{
    BatteryManagementActor, BmsParams, EvDriverActor, EvDriverParams, SolverFeedbackActor,
};
use crate::diagnostics::{self, EnvelopeDiag};
use crate::environment::EnvironmentInitOptions;
#[cfg(feature = "profiling")]
use crate::health::{ActorTiming, ActorTimings};
use crate::health::{RunHealth, WarmupOutcome, WarmupResiduals};
use crate::invariants::InvariantChecker;
use crate::invariants::check_basement_lighting_foundation;
use crate::rng::{advance_dwelling_rng, derive_sub_rng, ev_driver_stream};
use crate::scheduler::{ExecutionPhase, StepScheduler};
use crate::telemetry::DwellingTelemetry;
use crate::{Actor, ActorInterest, EnvironmentManager, SimClock};

#[cfg(feature = "observe")]
use crate::diagnostics::DiagnosticAccumulator;
#[cfg(all(feature = "observe", debug_assertions))]
use crate::observer::MoistureZoneInvariant;
#[cfg(feature = "observe")]
use crate::observer::{
    DispatchCapture, DispatchedSignal, EquipmentObservation, MoistureInvariantCapture,
    ObserverBuffer, PhaseSnapshots, SameTierConflict, StepSnapshot,
};
#[cfg(feature = "observe")]
use crate::observer_capture;
use roster::RosterCaches;

use conversions::{
    apply_humidity_update_to_zones, apply_thermal_update_to_zones, chrono_to_std_duration,
    default_output_path, duration_to_u32_secs, merged_equipment_config, required_datetime,
    required_duration, required_path, validate_equipment_override_keys, validate_sim_config,
};
#[cfg(feature = "profiling")]
use hares_types::alloc_count::thread_allocations;
use solver_builder::build_default_solvers;
#[cfg(feature = "profiling")]
use synthetic::current_process_hwm_kb;
use synthetic::{
    SyntheticTomlConfig, build_synthetic_building, build_synthetic_schedule,
    build_synthetic_weather,
};

const DEFAULT_GRID_FREQUENCY_HZ: f64 = 60.0;

/// Bus voltage [pu] formed by an island-capable source during a utility
/// outage. The minimal islanding model assumes the grid-forming source
/// regulates the bus at nominal; no island power-flow or droop physics is
/// modeled (see `GridState::island_bus_voltage_pu`).
const ISLAND_BUS_NOMINAL_VOLTAGE_PU: f64 = 1.0;

/// Unserved-load threshold [kW] above which islanded operation emits a
/// scenario-signal warning. 50 W sits above numerical residue and standby
/// noise but below any real appliance draw.
const ISLAND_UNSERVED_WARN_THRESHOLD_KW: f64 = 0.05;

/// Rate-limit gate for the islanded unserved-load warning: `tracing::warn!`
/// once per process, `tracing::debug!` for every subsequent occurrence
/// (unserved load recurs on every islanded step, so unthrottled warns would
/// flood the log).
static ISLAND_UNSERVED_WARNED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Minimum timestep resolution at which PV re-evaluation and BMS staleness
/// diagnostics have meaningful impact.  5 min matches the EnergyPlus minimum
/// `TimeStep` for sub-hourly simulation.
const PV_RE_EVAL_MIN_STEP_SECS: f64 = 300.0;

/// The identity a spec's random stream is keyed by: its HPXML
/// SystemIdentifier id, else the instance name its caller gave it, else its
/// class name. Two event loads with the same identity are rejected at
/// assembly rather than told apart by their order.
fn rng_identity(spec: &hares_io::EquipmentSpec) -> String {
    spec.system_id
        .as_ref()
        .or(spec.instance_name.as_ref())
        .unwrap_or(&spec.name)
        .clone()
}

fn create_equipment_from_config(
    registry: &EquipmentRegistry,
    config: EquipmentConfig,
) -> Result<Box<dyn Equipment>> {
    let name = config.name.clone();
    let ochre_class = config.ochre_class.clone();
    registry.create(&ochre_class, config).map_err(|err| {
        HaresError::Equipment(format!(
            "equipment '{}' (class '{}') create failed: {err}",
            name, ochre_class,
        ))
    })
}

/// Entrance-1 unassigned-id rejection, diagnosing *why* the sentinel
/// `EquipmentId(0)` reached the door: the constitution's loud-errors rule
/// requires the error to name the field, the value received, and the
/// expected form — "its config channel did not deliver one" is true only
/// for the absent case, and misdirects a user who wrote a malformed value
/// (or an explicit 0) to delete the field instead of correcting it.
/// Re-uses the assignment pass's classifier
/// (`hares_io::hpxml::equipment::explicit_equipment_id`) so the diagnosis
/// and the pass's leave-untouched decision can never disagree.
fn unassigned_equipment_id_rejection(spec: &hares_io::EquipmentSpec, name: &str) -> HaresError {
    use hares_io::hpxml::equipment::{ExplicitEquipmentId, explicit_equipment_id};
    let prefix = format!(
        "equipment '{name}' has an unassigned equipment id (0): ids are \
         assigned by the dwelling assembly and are not configurable; "
    );
    match explicit_equipment_id(spec) {
        ExplicitEquipmentId::Malformed(value) => HaresError::Equipment(format!(
            "{prefix}its config channel delivered a malformed equipment_id \
             ({value}) — equipment_id must be a non-negative integer within \
             u32 range; correct or remove the field"
        )),
        ExplicitEquipmentId::Valid(0) => HaresError::Equipment(format!(
            "{prefix}its config channel explicitly delivered the unassigned \
             sentinel equipment_id 0; remove the field"
        )),
        ExplicitEquipmentId::Valid(delivered) => HaresError::Equipment(format!(
            "{prefix}its config channel delivered equipment_id {delivered} but \
             its constructor did not apply it — the equipment's `Equipment` \
             implementation ignores the identity config channel"
        )),
        ExplicitEquipmentId::Absent => {
            HaresError::Equipment(format!("{prefix}its config channel did not deliver one"))
        }
    }
}

/// Core result type for dwelling operations.
pub type Result<T> = std::result::Result<T, HaresError>;

/// Stable config contract between fleet runners and a single dwelling orchestrator.
#[derive(Debug, Clone)]
pub struct DwellingConfig {
    pub hpxml_path: PathBuf,
    /// The dwelling's schedule source. `None` requests a schedule generated
    /// from the HPXML; `Some(path)` requests the CSV at `path`. A set path
    /// that cannot be read is a construction error naming it: a mistyped
    /// path must never silently run on a generated schedule's different
    /// occupancy and loads.
    pub schedule_path: Option<PathBuf>,
    pub weather_path: PathBuf,
    /// The directory the default schedule profiles load from. `None` means
    /// no default profiles: an equipment with no schedule column and no
    /// HPXML schedule fractions is a construction error naming this field,
    /// and a generated schedule cannot be built. The profiles never come
    /// from a working-directory guess.
    pub defaults_path: Option<PathBuf>,
    pub sim_config: SimulationConfig,
    pub overrides: Option<serde_json::Value>,
    pub bldg_id: i64,
    pub initialization_duration: Option<StdDuration>,
    /// Per-column weather resampling overrides. `None` uses defaults
    /// (PCHIP for continuous fields, ZOH for energy/wind).
    /// Use `Some(ResampleOverrides::ochre_compat())` for OCHRE parity testing.
    pub resample_overrides: Option<hares_io::ResampleOverrides>,
    /// HPXML data-quality patches from external metadata (e.g. ResStock).
    /// Fields supplement or correct HPXML-parsed data when the source
    /// document contains missing or invalid values.
    /// `None` for direct HPXML use where no external metadata is available.
    pub patches: Option<hares_io::HpxmlDataPatches>,
}

impl DwellingConfig {
    /// The raw schedule this config requests, before resampling to the
    /// simulation's step: the schedule file when `schedule_path` is set,
    /// otherwise a schedule generated from the parsed HPXML. A set path
    /// that cannot be read is an error naming it. A generated schedule
    /// needs the default schedule profiles from the defaults directory:
    /// no directory, an unreadable profile file, or a missing profile is
    /// a construction error naming it.
    pub(crate) fn load_schedule(
        &self,
        building: &Building,
        weather_meta: &hares_io::WeatherMeta,
    ) -> Result<ScheduleTimeSeries> {
        match self.schedule_path.as_deref() {
            Some(path) => parse_schedule_csv(path, &[], Some(weather_meta), None).map_err(|err| {
                HaresError::Io(format!(
                    "schedule file '{}' could not be read: {err}",
                    path.display()
                ))
            }),
            None => {
                let defaults_dir = self.defaults_path.as_deref().ok_or_else(|| {
                    HaresError::Io(format!(
                        "no defaults directory is configured: a generated schedule \
                         needs the default schedule profiles file '{}' from the \
                         defaults directory; set defaults_path",
                        hares_io::schedule_resolve::DEFAULT_SCHEDULES_CSV,
                    ))
                })?;
                let profiles = hares_io::load_default_profiles(defaults_dir)?;
                Ok(hares_io::hpxml_schedule::generate_schedule_from_hpxml(
                    building,
                    self.sim_config.start_time,
                    self.sim_config.duration,
                    self.sim_config.time_res,
                    &profiles,
                )?)
            }
        }
    }
}

/// Wall-clock breakdown of one dwelling's `run_timestep` work, behind the
/// `profiling` feature. The eleven phase fields partition every completed
/// step exactly: each phase is the sum of spans closed by `Instant::now()`
/// readings taken at the step's phase boundaries, so their sum equals
/// `step_total` (no residual). Like every field, they accumulate over the
/// run's steps. `memory_high_water_kb` is the one exception: it is read
/// from the process once, when `profiling_summary()` is called, so the
/// per-step work pays for no `/proc` reads. The hot-path allocation fields
/// are `None` unless the binary installed the workspace's counting
/// allocator. `per_actor` is not a twelfth phase: it is the run-total
/// breakdown *within* the `actors` phase, one entry per registered actor
/// in registration order, whose totals sum to that phase exactly because
/// each entry's span is the same phase-clock difference the phase gets.
#[cfg(feature = "profiling")]
#[derive(Debug, Clone, Default)]
pub struct DwellingProfilingSummary {
    pub environment: StdDuration,
    pub control: StdDuration,
    pub ideal_capacity: StdDuration,
    pub actors: StdDuration,
    pub dispatch: StdDuration,
    pub equipment: StdDuration,
    pub envelope: StdDuration,
    pub invariants: StdDuration,
    pub state_snapshot: StdDuration,
    pub output: StdDuration,
    pub accounting: StdDuration,
    pub step_total: StdDuration,
    /// The process's peak resident set size in KiB, read once when
    /// `profiling_summary()` is called. `None` off Linux, or when the read
    /// or parse fails there: never a zero that would read as a measurement.
    pub memory_high_water_kb: Option<u64>,
    /// Sum over the run's steps of the hot path's per-step allocation
    /// deltas, counted on the stepping thread. `None` when the binary did
    /// not install `hares_types::alloc_count::CountingAllocator`.
    pub hot_path_allocations: Option<u64>,
    /// Steps whose hot path allocated at least once. `None` when the
    /// binary did not install `hares_types::alloc_count::CountingAllocator`.
    pub hot_path_alloc_violations: Option<u64>,
    /// Per-actor totals over the run (`health::ActorTiming`): one entry per
    /// registered actor in registration order, filled from the dwelling's
    /// `ActorTimings` store when `profiling_summary()` is called.
    pub per_actor: Vec<ActorTiming>,
}

/// The phases of `run_timestep`, named only by the step's phase clock.
/// Declaration order is the spans' code order in `run_timestep`.
#[cfg(feature = "profiling")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProfilePhase {
    Environment,
    Control,
    IdealCapacity,
    Actors,
    Dispatch,
    Equipment,
    Envelope,
    Invariants,
    StateSnapshot,
    Output,
    Accounting,
}

#[cfg(feature = "profiling")]
impl DwellingProfilingSummary {
    /// Adds one closed span to its phase. A phase may be closed more than
    /// once per step; its spans are summed.
    fn add_span(&mut self, phase: ProfilePhase, span: StdDuration) {
        let field = match phase {
            ProfilePhase::Environment => &mut self.environment,
            ProfilePhase::Control => &mut self.control,
            ProfilePhase::IdealCapacity => &mut self.ideal_capacity,
            ProfilePhase::Actors => &mut self.actors,
            ProfilePhase::Dispatch => &mut self.dispatch,
            ProfilePhase::Equipment => &mut self.equipment,
            ProfilePhase::Envelope => &mut self.envelope,
            ProfilePhase::Invariants => &mut self.invariants,
            ProfilePhase::StateSnapshot => &mut self.state_snapshot,
            ProfilePhase::Output => &mut self.output,
            ProfilePhase::Accounting => &mut self.accounting,
        };
        *field += span;
    }
}

/// One phase clock per step: `Instant::now()` is read once at entry and once
/// at each phase boundary, and each reading closes the open span into its
/// phase and opens the next, so the phases partition the step exactly with
/// no residual. A step that returns early (a failed invariant) records
/// nothing for that step; an unhealthy actor is logged and the step
/// continues.
#[cfg(feature = "profiling")]
struct PhaseClock {
    started: Instant,
    open: Instant,
    phase: ProfilePhase,
    /// The actor slot whose `Actors` span is currently open, with whether
    /// the interest filter called its `decide()` so far. `None` when the
    /// open span is not an `Actors` span. The flag is what keeps the
    /// per-actor totals summing to the `actors` phase exactly: the closed
    /// span is one `Instant` difference routed to both the phase and the
    /// slot.
    open_actor: Option<(usize, bool)>,
}

#[cfg(feature = "profiling")]
impl PhaseClock {
    fn start() -> Self {
        let now = Instant::now();
        Self {
            started: now,
            open: now,
            phase: ProfilePhase::Environment,
            open_actor: None,
        }
    }

    /// Closes the open span into its phase and opens `next`.
    fn enter(&mut self, profiling: &mut DwellingProfilingSummary, next: ProfilePhase) {
        let now = Instant::now();
        profiling.add_span(self.phase, now - self.open);
        self.open = now;
        self.phase = next;
    }

    /// Scheduler-loop variant of [`Self::enter`]: closes the open span into
    /// its phase, routing an `Actors` span to its actor's slot in
    /// `timings`, then opens `next`. `actor_slot` is `Some` only for an
    /// `ActorDecide` entry, whose span is the actor's scheduler-entry time
    /// (interest filter, `decide()` and health check included: the whole
    /// span the phase clock attributes to `actors`).
    fn enter_plan_entry(
        &mut self,
        profiling: &mut DwellingProfilingSummary,
        timings: &mut ActorTimings,
        next: ProfilePhase,
        actor_slot: Option<usize>,
    ) {
        let now = Instant::now();
        let span = now - self.open;
        profiling.add_span(self.phase, span);
        self.route_closed_actor_span(timings, span);
        self.open = now;
        self.phase = next;
        self.open_actor = actor_slot.map(|slot| (slot, false));
    }

    /// Records that the open `Actors` span's filter called `decide()`. Must
    /// run inside the plan entry whose span is open.
    fn mark_open_actor_called(&mut self) {
        if let Some((_, called)) = &mut self.open_actor {
            *called = true;
        }
    }

    /// Closes the final span at return and accumulates `step_total` (entry
    /// to return) over the run. The open phase here is always `accounting`
    /// (the last `enter` before return opens it and nothing intervenes), so
    /// no actor span is routed.
    fn finish(self, profiling: &mut DwellingProfilingSummary) {
        let now = Instant::now();
        profiling.add_span(self.phase, now - self.open);
        profiling.step_total += now - self.started;
    }

    /// Routes a just-closed span to its actor's slot when it is an `Actors`
    /// span. The same `Instant` difference already went to the phase, so
    /// the per-actor totals sum to the `actors` phase exactly.
    fn route_closed_actor_span(&self, timings: &mut ActorTimings, span: StdDuration) {
        if self.phase == ProfilePhase::Actors
            && let Some((slot, called)) = self.open_actor
        {
            timings.record(slot, span, called);
        }
    }
}

/// Pre-resolved output column indices for one equipment piece.
#[derive(Debug, Clone, Default)]
struct EquipmentColumns {
    electric_power: Option<usize>,
    gas_power: Option<usize>,
    mode: Option<usize>,
    setpoint: Option<usize>,
    soc: Option<usize>,
    capacity: Option<usize>,
    cop: Option<usize>,
    reactive_power: Option<usize>,
    power_factor: Option<usize>,
    energy_kwh: Option<usize>,
    schedule: Option<usize>,
    defrost_state: Option<usize>,
    er_power: Option<usize>,
    shr: Option<usize>,
    speed: Option<usize>,
    fan_power: Option<usize>,
    main_power: Option<usize>,
    runtime_fraction: Option<usize>,
    latent_gains: Option<usize>,
    duct_losses: Option<usize>,
    /// V8 per-equipment telemetry diagnostic columns. Each entry is
    /// (telemetry_key, column_index). Populated from telemetry in record_step.
    v8_columns: Vec<(&'static str, usize)>,
}

/// Enriches the output schema's Arrow field metadata with unit declarations
/// from equipment `TelemetryField` descriptors, closing the round-trip gap
/// between equipment-declared units and the column-suffix-derived units used
/// by the aggregation pipeline.
///
/// For every schema column whose name starts with an equipment instance name,
/// the column's unit (extracted from the name suffix) is verified against the
/// corresponding equipment's `telemetry_fields`. When the unit is found in the
/// equipment's declared fields, the field's metadata is enriched with a
/// `"unit_source"` key set to `"telemetry_field"` to indicate the unit has been
/// validated against the authoritative equipment telemetry declaration.
///
/// Columns whose unit is not recognized by the owning equipment's telemetry
/// fields keep their suffix-derived metadata and emit a `tracing::warn!`.
fn enrich_schema_with_telemetry_units(schema: Schema, equipment: &[&dyn Equipment]) -> Schema {
    // Compute instance-qualified names matching build_schema's convention.
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for eq in equipment {
        *counts.entry(eq.descriptor().name.as_str()).or_default() += 1;
    }
    let mut indices: HashMap<&str, usize> = HashMap::new();
    let instance_units: Vec<(String, HashSet<String>)> = equipment
        .iter()
        .map(|eq| {
            let desc = eq.descriptor();
            let base = desc.name.as_str();
            let name = if counts[base] > 1 {
                let idx = indices.entry(base).or_insert(0);
                *idx += 1;
                format!("{base} #{idx}")
            } else {
                base.to_string()
            };
            let units: HashSet<String> = desc
                .telemetry_fields
                .iter()
                .map(|tf| tf.unit.clone())
                .collect();
            (name, units)
        })
        .collect();

    // For each schema field that belongs to a known equipment instance,
    // verify the column's unit against the equipment's declared telemetry
    // field units and enrich metadata.
    let fields: Vec<std::sync::Arc<Field>> = schema
        .fields()
        .iter()
        .map(|field| {
            let col_name = field.name();
            let mut metadata = field.metadata().clone();

            // Check if this column name starts with a known instance name
            // followed by a space (to avoid false prefix matches).
            if let Some((_name, units)) = instance_units.iter().find(|(name, _)| {
                col_name.as_bytes().starts_with(name.as_bytes())
                    && (col_name.len() == name.len() || col_name.as_bytes()[name.len()] == b' ')
            }) && let Some(col_unit) = extract_unit_from_name(col_name)
            {
                if units.contains(col_unit) {
                    metadata.insert("unit_source".to_string(), "telemetry_field".to_string());
                } else {
                    tracing::warn!(
                        column = col_name,
                        unit = col_unit,
                        "column unit not found in equipment telemetry fields"
                    );
                }
            }

            std::sync::Arc::new(field.as_ref().clone().with_metadata(metadata))
        })
        .collect();

    Schema::new_with_metadata(fields, schema.metadata().clone())
}

/// Build column index maps for each equipment piece using instance-qualified
/// names (matching `hares_io::output::columns::instance_qualified_names`).
/// Schema membership rule, single home for the column-map builder and
/// record_step's column invariants: an equipment is schema-known iff the
/// schema emitted its unconditional per-equipment column under its own
/// name (verbosity ≥ 1 always emits `{name} Electric Power (kW)`), and
/// that column is not one of the reserved aggregate names — an equipment
/// named "Total" must not read the aggregate column as evidence of its
/// own membership.
fn is_schema_known_equipment(column_index: &HashMap<String, usize>, name: &str) -> bool {
    let membership_column = format!("{name} {ELECTRIC_POWER_SUFFIX}");
    column_index.contains_key(&membership_column)
        && !is_reserved_output_column_name(&membership_column)
}

fn build_equipment_column_map(
    equipment: &[&dyn Equipment],
    column_index: &HashMap<String, usize>,
    verbosity: u8,
) -> Result<Vec<EquipmentColumns>> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for eq in equipment {
        *counts.entry(&eq.descriptor().name).or_default() += 1;
    }
    let mut indices: HashMap<&str, usize> = HashMap::new();
    equipment
        .iter()
        .map(|eq| {
            let desc = eq.descriptor();
            let base = &desc.name;
            let name = if counts[base.as_str()] > 1 {
                let idx = indices.entry(base).or_insert(0);
                *idx += 1;
                format!("{base} #{idx}")
            } else {
                base.clone()
            };
            // `expected` holds only for equipment the schema was built from.
            // The column map is re-derived against a FROZEN column index
            // whenever rows have been recorded (mid-run add/remove/replace),
            // and equipment that joined after the schema was frozen has no
            // columns in it — by design ("missing values, never misattributed
            // ones"). Demanding its columns would fail here; instead it gets
            // an empty map. Equipment the schema DOES know must resolve every
            // applicable column — a miss there is a real schema/map drift.
            // Membership is EXACT — the schema's unconditional
            // `{name} Electric Power (kW)` column at verbosity ≥ 1 — not a
            // name-prefix match: a mid-run equipment whose name is a
            // word-boundary prefix of another equipment's name ("Electric"
            // vs "Electric Resistance Water Heater") must not inherit the
            // other instance's columns as evidence of its own. It also
            // excludes the RESERVED aggregate namespace via the shared
            // membership rule (`is_schema_known_equipment`).
            let in_schema = is_schema_known_equipment(column_index, &name);
            // A schema-known equipment resolves — and demands — every
            // applicable column; a schema-unknown one claims NOTHING, not
            // even a column that happens to exist under its name (the
            // reserved aggregates are exactly that case: present in the
            // index, never this equipment's own).
            let resolve = |col_name: &str, min_verbosity: u8| -> Result<Option<usize>> {
                if !in_schema {
                    return Ok(None);
                }
                resolve_col(col_name, column_index, &name, verbosity >= min_verbosity)
            };
            // Shared predicate with build_schema: the schema emits a
            // "{name} Gas Power (therms/hour)" column iff this returns true,
            // so resolving with the same predicate guarantees every emitted
            // column is populated.
            let has_gas = hares_io::fuel_reports_gas_power_column(desc.fuel);
            let is_hvac = is_hvac_or_wh(&desc.name);
            let is_cooling = is_cooling_equipment(&desc.name);
            let is_hp_heater = is_heat_pump_heater(&desc.name);
            let has_soc = has_soc(&desc.name);

            let electric_power = resolve(&format!("{name} {ELECTRIC_POWER_SUFFIX}"), 1)?;
            let gas_power = if has_gas {
                resolve(&format!("{name} {GAS_POWER_SUFFIX}"), 1)?
            } else {
                None
            };
            let mode = resolve(&format!("{name} {MODE_SUFFIX}"), 3)?;
            let setpoint = if is_hvac {
                resolve(&format!("{name} {SETPOINT_SUFFIX}"), 3)?
            } else {
                None
            };
            let soc = if has_soc {
                resolve(&format!("{name} {SOC_SUFFIX}"), 3)?
            } else {
                None
            };
            let capacity = if is_hvac {
                resolve(&format!("{name} {CAPACITY_SUFFIX}"), 7)?
            } else {
                None
            };
            let cop = if is_hvac {
                resolve(&format!("{name} {COP_SUFFIX}"), 7)?
            } else {
                None
            };
            // Reactive/PF columns exist for every equipment at verbosity ≥ 5
            // (mirroring the unconditional Electric Power column): gas
            // equipment with electric parasitics (blower fans, circulation
            // pumps, draft inducers) emits reactive power too.
            let reactive = resolve(&format!("{name} {REACTIVE_POWER_SUFFIX}"), 5)?;
            let pf = resolve(&format!("{name} {POWER_FACTOR_SUFFIX}"), 5)?;
            let energy_kwh = resolve(&format!("{name} {ENERGY_SUFFIX}"), 4)?;
            let schedule = resolve(&format!("{name} {SCHEDULE_SUFFIX}"), 7)?;
            let defrost_state = if is_hp_heater {
                resolve(&format!("{name} {DEFROST_STATE_SUFFIX}"), 7)?
            } else {
                None
            };
            let er_power = if is_hp_heater {
                resolve(&format!("{name} {ER_POWER_SUFFIX}"), 7)?
            } else {
                None
            };
            let shr = if is_cooling {
                resolve(&format!("{name} {SHR_SUFFIX}"), 7)?
            } else {
                None
            };
            let speed = if is_hvac {
                resolve(&format!("{name} {SPEED_SUFFIX}"), 7)?
            } else {
                None
            };
            let fan_power = if is_hvac {
                resolve(&format!("{name} {FAN_POWER_SUFFIX}"), 7)?
            } else {
                None
            };
            let main_power = if is_hvac {
                resolve(&format!("{name} {MAIN_POWER_SUFFIX}"), 7)?
            } else {
                None
            };
            let runtime_fraction = if is_hvac {
                resolve(&format!("{name} {RUNTIME_FRACTION_SUFFIX}"), 7)?
            } else {
                None
            };
            let latent_gains = if is_cooling {
                resolve(&format!("{name} {LATENT_GAINS_SUFFIX}"), 7)?
            } else {
                None
            };
            // `HVAC Duct Losses (W)` is a GLOBAL aggregate column, not a
            // per-equipment one: record_step accumulates every equipment's
            // duct-loss telemetry into the same row slot (`row[idx] +=`),
            // and its invariant demands the column for every
            // equipment at verbosity ≥ 5. It therefore resolves for ANY
            // equipment when the schema carries it — including equipment
            // added after the schema froze, whose per-equipment columns are
            // missing by design but whose duct contribution must still
            // count toward the aggregate (gating it on `in_schema` would
            // fail the invariant and silently undercount the
            // aggregate in release builds).
            let duct_losses =
                resolve_col(HVAC_DUCT_LOSSES_COL, column_index, &name, verbosity >= 5)?;

            // V8 per-equipment telemetry diagnostic columns.
            let v8_columns = if verbosity >= 8 {
                resolve_v8_columns(
                    &name,
                    column_index,
                    is_hvac,
                    is_hp_heater,
                    &desc.name,
                    is_pv(&desc.name),
                    is_ev(&desc.name),
                )
            } else {
                Vec::new()
            };

            Ok(EquipmentColumns {
                electric_power,
                gas_power,
                mode,
                setpoint,
                soc,
                capacity,
                cop,
                reactive_power: reactive,
                power_factor: pf,
                energy_kwh,
                schedule,
                defrost_state,
                er_power,
                shr,
                speed,
                fan_power,
                main_power,
                runtime_fraction,
                latent_gains,
                duct_losses,
                v8_columns,
            })
        })
        .collect()
}

/// Derive output `EquipmentSpec`s from live equipment descriptors — the
/// spec-shaped view the schema-facing column helpers consume. Positional:
/// one spec per equipment, in vector order.
fn equipment_descriptor_specs(equipment: &[&dyn Equipment]) -> Vec<hares_io::EquipmentSpec> {
    equipment
        .iter()
        .map(|eq| {
            let d = eq.descriptor();
            hares_io::EquipmentSpec {
                instance_name: None,
                name: d.name.clone(),
                fuel_type: d.fuel,
                parameters: Map::new(),
                zip_params: None,
                typed_config: None,
                system_id: None,
                related_hvac_idref: None,
                primary_role: None,
            }
        })
        .collect()
}

/// Resolve each spec's EndUse aggregate electric-power column index —
/// positional, one entry per spec in vector order (`record_step` zips it
/// against the equipment vector, accumulating every member's power into
/// its end-use's single column). The positional contract is why the map
/// must be re-derived whenever the equipment vector changes mid-run: a
/// remove that shifts the vector would otherwise leave every survivor
/// writing into its departed neighbour's end-use aggregate. Shared by
/// assembly, the fresh-schema rebuild, and the frozen-schema
/// re-derivation so the three cannot drift.
fn build_end_use_aggregate_indices(
    specs: &[hares_io::EquipmentSpec],
    column_index: &HashMap<String, usize>,
) -> Vec<Option<usize>> {
    specs
        .iter()
        .map(|spec| {
            let end_use = equipment_name_to_end_use(&spec.name);
            let col_name = end_use_electric_power_column(&end_use);
            column_index.get(&col_name).copied()
        })
        .collect()
}

/// Resolve each actor's telemetry columns against the output column
/// index: `(key, index)` entry lists, one per actor, in actor order.
/// Actors without telemetry get an empty entry list — the positional
/// `zip` against the actor vector in `record_step` requires a per-actor
/// entry regardless. Shared by the fresh-schema and frozen-schema roster
/// plans so the two derivations cannot drift; pre-resolving also keeps the
/// per-step `format!("actor:{name}:{key}")` out of the hot path.
fn build_actor_column_map(
    actors: &[&dyn Actor],
    column_index: &HashMap<String, usize>,
) -> Vec<Vec<(String, usize)>> {
    actors
        .iter()
        .map(|actor| {
            let Some(tel) = actor.telemetry() else {
                return Vec::new();
            };
            let actor_name = actor.name();
            let mut entries = Vec::with_capacity(tel.0.len());
            for key in tel.0.keys() {
                let col_name = format!("actor:{actor_name}:{key}");
                if let Some(&idx) = column_index.get(&col_name) {
                    entries.push((key.clone(), idx));
                }
            }
            entries
        })
        .collect()
}

/// Resolves a column name in the output column index map.
///
/// Schema drift for a column the schema owes the equipment (`expected`) is
/// a typed error in every build profile; a missing column that was never
/// demanded resolves to `None` by design ("missing values, never
/// misattributed ones").
fn resolve_col(
    col_name: &str,
    column_index: &HashMap<String, usize>,
    equipment_name: &str,
    expected: bool,
) -> Result<Option<usize>> {
    match column_index.get(col_name).copied() {
        Some(idx) => Ok(Some(idx)),
        None if expected => {
            tracing::warn!(
                column_name = %col_name,
                equipment_name = equipment_name,
                "output column index not resolved; data will not be emitted for this column"
            );
            Err(HaresError::InvariantViolation {
                check_name: format!(
                    "output_schema_drift: expected output column '{col_name}' for equipment \
                     '{equipment_name}' not found in schema"
                ),
                value: 0.0,
                tolerance: 0.0,
            })
        }
        None => Ok(None),
    }
}

/// Resolves v8 per-equipment telemetry diagnostic columns for the given
/// equipment instance. Each entry is a (telemetry_key, column_index) pair
/// populated directly from equipment telemetry in record_step.
fn resolve_v8_columns(
    instance_name: &str,
    column_index: &HashMap<String, usize>,
    is_hvac: bool,
    is_hp_heater: bool,
    _equipment_name: &str,
    is_pv: bool,
    is_ev: bool,
) -> Vec<(&'static str, usize)> {
    let mut columns = Vec::new();

    if is_hvac {
        let hvac_defs: &[(&str, &str)] = &[
            (tk::SUPPLY_TEMP_C, SUPPLY_TEMP_SUFFIX),
            (tk::RETURN_TEMP_C, RETURN_TEMP_SUFFIX),
            (tk::COMPRESSOR_POWER_W, COMPRESSOR_POWER_W_SUFFIX),
            (tk::COMPRESSOR_KW, COMPRESSOR_POWER_KW_SUFFIX),
            (tk::FAN_ELECTRIC_W, FAN_ELECTRIC_POWER_SUFFIX),
            (tk::FAN_POWER_W, FAN_POWER_W_SUFFIX),
            (tk::MIN_ON_TIME_S, MIN_ON_TIME_SUFFIX),
            (tk::MIN_OFF_TIME_S, MIN_OFF_TIME_SUFFIX),
        ];
        for &(key, suffix) in hvac_defs {
            let col_name = format!("{instance_name} {suffix}");
            if let Some(&idx) = column_index.get(&col_name) {
                columns.push((key, idx));
            }
        }
    }
    if is_hp_heater {
        let hp_defs: &[(&str, &str)] = &[
            (tk::SUPPLY_AIR_TEMP_C, SUPPLY_AIR_TEMP_SUFFIX),
            (tk::PAN_HEATER_KW, PAN_HEATER_POWER_SUFFIX),
            (tk::HP_CAPACITY_W, HP_CAPACITY_SUFFIX),
            (tk::ER_CAPACITY_W, ER_CAPACITY_SUFFIX),
        ];
        for &(key, suffix) in hp_defs {
            let col_name = format!("{instance_name} {suffix}");
            if let Some(&idx) = column_index.get(&col_name) {
                columns.push((key, idx));
            }
        }
    }
    if is_pv {
        let pv_defs: &[(&str, &str)] = &[
            (tk::DC_POWER_KW, PV_DC_POWER_SUFFIX),
            (tk::IRRADIANCE_W_M2, PV_IRRADIANCE_SUFFIX),
        ];
        for &(key, suffix) in pv_defs {
            let col_name = format!("{instance_name} {suffix}");
            if let Some(&idx) = column_index.get(&col_name) {
                columns.push((key, idx));
            }
        }
    }
    if is_ev {
        let ev_defs: &[(&str, &str)] = &[
            (tk::CONNECTION_STATE, EV_CONNECTION_STATE_SUFFIX),
            (tk::CHARGING_LEVEL, EV_CHARGING_LEVEL_SUFFIX),
        ];
        for &(key, suffix) in ev_defs {
            let col_name = format!("{instance_name} {suffix}");
            if let Some(&idx) = column_index.get(&col_name) {
                columns.push((key, idx));
            }
        }
    }

    columns
}

fn extend_schema_with_actor_columns(
    schema: &arrow::datatypes::Schema,
    actors: &[&dyn Actor],
) -> arrow::datatypes::Schema {
    use arrow::datatypes::{DataType, Field};
    let mut fields: Vec<Field> = schema.fields().iter().map(|f| f.as_ref().clone()).collect();
    for actor in actors {
        if let Some(tel) = actor.telemetry() {
            // Telemetry is a HashMap whose iteration order varies per
            // process (RandomState). Sort the keys so the output schema —
            // and therefore DataFrame column order — is deterministic
            // across runs (fixed-seed reproducibility requirement).
            let mut keys: Vec<&String> = tel.0.keys().collect();
            keys.sort();
            for key in keys {
                let col_name = format!("actor:{}:{}", actor.name(), key);
                if !fields.iter().any(|f| f.name() == &col_name) {
                    fields.push(Field::new(&col_name, DataType::Float64, true));
                }
            }
        }
    }
    arrow::datatypes::Schema::new_with_metadata(fields, schema.metadata().clone())
}

/// Pre-resolved zone column indices for record_step, avoiding per-step format!() allocations.
#[derive(Default)]
struct ZoneColumnCaches {
    /// ZoneId → column_index for per-zone infiltration columns (non-indoor only).
    infiltration_columns: HashMap<ZoneId, usize>,
    /// ZoneId → column_index for per-zone interior LWR columns.
    lwr_columns: HashMap<ZoneId, usize>,
    /// ZoneId → (heating_col, cooling_col) for per-zone HVAC thermal attribution columns.
    hvac_columns: HashMap<ZoneId, (usize, usize)>,
    /// Zone IDs in sorted order for StepResult construction.
    sorted_zone_ids: Vec<ZoneId>,
    /// For each entry in sorted_zone_ids, the index into EnvironmentState::zones where
    /// that zone lives. Enables O(1) temperature lookup in the hot loop.
    zone_env_indices: Vec<Option<usize>>,
    /// For each entry in sorted_zone_ids, the output column index for zone temperature,
    /// or None if no temperature column exists for that zone.
    zone_temp_col_indices: Vec<Option<usize>>,
}

fn build_zone_column_caches(
    zones: &[hares_types::ZoneState],
    zone_types: &[hares_io::hpxml::ZoneType],
    indoor_zone: ZoneId,
    column_index: &HashMap<String, usize>,
) -> ZoneColumnCaches {
    let mut sorted_zone_ids: Vec<ZoneId> = zones.iter().map(|z| z.id).collect();
    sorted_zone_ids.sort();

    let mut infiltration_columns = HashMap::new();
    let mut lwr_columns = HashMap::new();
    let mut hvac_columns = HashMap::new();
    let mut zone_env_indices = Vec::with_capacity(sorted_zone_ids.len());
    let mut zone_temp_col_indices = Vec::with_capacity(sorted_zone_ids.len());

    for &zone_id in &sorted_zone_ids {
        // Pre-compute the index of this zone in the zones slice for O(1) hot-loop access.
        let env_idx = zones.iter().position(|z| z.id == zone_id);
        let zone_type = env_idx.and_then(|idx| zone_types.get(idx));
        let label = zone_display_name(zone_id, indoor_zone, zone_type);
        let temp_key = format!("Temperature - {label} (C)");
        let temp_col = column_index.get(&temp_key).copied();
        if zone_id != indoor_zone {
            let inf_key = format!("Infiltration Heat Gain - {label} (W)");
            if let Some(&idx) = column_index.get(&inf_key) {
                infiltration_columns.insert(zone_id, idx);
            }
        }
        let lwr_key = format!("Interior LWR Exchange - {label} (W)");
        if let Some(&idx) = column_index.get(&lwr_key) {
            lwr_columns.insert(zone_id, idx);
        }
        let heat_key = format!("HVAC Heating Delivered - {label} (W)");
        let cool_key = format!("HVAC Cooling Delivered - {label} (W)");
        if let (Some(&heat_idx), Some(&cool_idx)) =
            (column_index.get(&heat_key), column_index.get(&cool_key))
        {
            hvac_columns.insert(zone_id, (heat_idx, cool_idx));
        }
        zone_env_indices.push(env_idx);
        zone_temp_col_indices.push(temp_col);
    }

    ZoneColumnCaches {
        infiltration_columns,
        lwr_columns,
        hvac_columns,
        sorted_zone_ids,
        zone_env_indices,
        zone_temp_col_indices,
    }
}

/// Single-step observable output.
#[derive(Debug, Clone, PartialEq)]
pub struct StepResult {
    pub timestamp: DateTime<FixedOffset>,
    pub net_electric_power_kw: f64,
    pub zone_temperatures_c: Vec<(ZoneId, f64)>,
    /// Thermal energy delivered to the zone by HVAC heating equipment (W, positive).
    pub hvac_heating_w: f64,
    /// Thermal energy removed from the zone by HVAC cooling equipment (W, positive = heat removed).
    pub hvac_cooling_w: f64,
    /// Total gas fuel consumption across all equipment (W).
    pub gas_power_w: f64,
}

/// Accumulated simulation outputs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SimulationResults {
    pub steps: Vec<StepResult>,
}

/// Internal control queue and routing logic.
///
/// # Tier ordering (across priority levels)
///
/// Signals are bucketed into tier queues on `queue()` and drained low→high in
/// `dispatch_into()`. Because every signal fires (no deduplication), the
/// highest-priority tier writes last and wins. Equipment `apply_control` must
/// be overwrite-safe (idempotent set, not accumulate).
///
/// # Same-tier conflict resolution
///
/// Within a single priority tier, signals are dispatched in FIFO order (the
/// order they were queued). When two or more signals at the same tier target
/// the same equipment, the last signal queued within that tier wins because
/// `apply_control` is overwrite-safe — each subsequent `apply_control` call
/// overwrites the state written by the prior call for the same target. There
/// is no deduplication, merging, or composition within a tier.
///
/// **Strategy:** last-write-wins (FIFO queuing order within the tier).
///
/// **Rationale:** Simple, deterministic for a given actor registration order,
/// and requires no per-signal-type composition rules. Composition strategies
/// — most-restrictive (min consumption for curtailment, widest deadband for
/// thermostats), least-restrictive, weighted average — would require
/// signal-type-specific merge logic that must be designed and maintained for
/// every new signal variant. Last-write-wins avoids this complexity while
/// remaining predictable: the outcome is fully determined by the order in
/// which actors emit signals into the same tier.
///
/// **Implications:**
/// - Simulation results depend on actor registration order. Reordering actor
///   registration — e.g. swapping the order in which two DR programs are
///   added — changes which signal wins when both target the same equipment
///   at the same tier.
/// - Reproducibility requires recording actor registration order alongside
///   simulation outputs. The debug/invariant-check log emitted at dwelling
///   initialization captures this order explicitly.
/// - Upstream callers that programmatically register actors must be aware
///   that the last-added actor at a given tier dominates for shared targets.
///
/// # Cross-dispatch ledger
///
/// A single timestep may dispatch multiple times (e.g. once pre-thermal-FSM to
/// flush externally queued setpoints, once post-actor-decide to apply actor
/// signals). `begin_step()` resets the cross-dispatch conflict ledger so that
/// priority ordering holds across passes: a lower-priority signal arriving
/// in a later pass is SKIPPED when a higher-priority signal has already been
/// applied to the same target in an earlier pass.
struct ControlDispatcher {
    by_tier: [VecDeque<DispatchRequest>; PRIORITY_TIER_COUNT],
    /// Scratch buffer for conflict detection -- tracks (target, tier_index).
    /// Pre-allocated, cleared at the start of each step via `begin_step()`.
    /// Preserved across multiple dispatch passes within a single step so that
    /// priority inversion cannot occur: once a tier has been seen for a target,
    /// strictly lower tiers are rejected even if they are queued later.
    seen_targets: Vec<(DispatchTarget, usize)>,
}

/// Returns true if two dispatch targets route to the same physical equipment,
/// accounting for `ByEndUse` expansion against the equipment list.
///
/// Unlike `DispatchTarget::conflicts_with()`, this resolves `ByEndUse` targets
/// to the equipment names they match, so cross-variant pairs (e.g.
/// `ByEndUse(BATTERY)` vs `ByName("Battery #1")`) are correctly detected as
/// conflicting when they target the same physical equipment.
///
/// Used by the same-tier conflict pre-scans in `dispatch_into_observed`,
/// which operate on raw
/// pre-expansion queue entries where cross-variant calls are expected.
#[cfg(feature = "observe")]
fn targets_conflict(
    a: &DispatchTarget,
    b: &DispatchTarget,
    equipment: &[Box<dyn Equipment>],
) -> bool {
    match (a, b) {
        (DispatchTarget::ByName(an), DispatchTarget::ByName(bn)) => an == bn,
        (DispatchTarget::ByEndUse(ae), DispatchTarget::ByEndUse(be)) => ae == be,
        (DispatchTarget::ByName(name), DispatchTarget::ByEndUse(end_use))
        | (DispatchTarget::ByEndUse(end_use), DispatchTarget::ByName(name)) => {
            equipment.iter().any(|eq| {
                eq.descriptor().name.as_str() == &**name && eq.descriptor().end_use == *end_use
            })
        }
    }
}

impl Default for ControlDispatcher {
    fn default() -> Self {
        Self {
            by_tier: std::array::from_fn(|_| VecDeque::new()),
            seen_targets: Vec::with_capacity(16),
        }
    }
}

impl ControlDispatcher {
    fn queue(&mut self, request: DispatchRequest) {
        self.by_tier[request.priority.index()].push_back(request);
    }

    /// Resets the per-step priority ledger. Must be called once per timestep
    /// before the first `dispatch_into` call; subsequent dispatches within the
    /// step inherit the ledger so priority ordering holds across passes.
    fn begin_step(&mut self) {
        self.seen_targets.clear();
    }

    fn dispatch_into(
        &mut self,
        equipment: &mut [Box<dyn Equipment>],
        warnings: &mut WarningLog,
        rejected_control_signals: &mut u64,
    ) {
        self.drain_tiers(
            equipment,
            warnings,
            rejected_control_signals,
            |_, _, _, _| {},
        );
    }

    #[cfg(feature = "observe")]
    fn dispatch_into_observed(
        &mut self,
        equipment: &mut [Box<dyn Equipment>],
        warnings: &mut WarningLog,
        rejected_control_signals: &mut u64,
    ) -> DispatchCapture {
        let mut same_tier_conflicts = Vec::new();
        for (tier_idx, tier_que) in self.by_tier.iter().enumerate() {
            let (head, tail) = tier_que.as_slices();
            let requests: Vec<&DispatchRequest> = head.iter().chain(tail.iter()).collect();
            for i in 0..requests.len() {
                for j in (i + 1)..requests.len() {
                    if targets_conflict(&requests[i].target, &requests[j].target, equipment) {
                        let tier = PriorityTier::from_index(tier_idx);
                        let already_recorded =
                            same_tier_conflicts.iter().any(|c: &SameTierConflict| {
                                c.tier == tier
                                    && targets_conflict(&c.target, &requests[i].target, equipment)
                            });
                        if !already_recorded {
                            let signals: Vec<_> = requests
                                .iter()
                                .filter(|r| {
                                    r.priority == tier
                                        && targets_conflict(
                                            &r.target,
                                            &requests[i].target,
                                            equipment,
                                        )
                                })
                                .map(|r| r.signal.clone())
                                .collect();
                            same_tier_conflicts.push(SameTierConflict {
                                tier,
                                target: requests[i].target.clone(),
                                signals,
                            });
                        }
                    }
                }
            }
        }

        let mut signals = Vec::new();
        self.drain_tiers(
            equipment,
            warnings,
            rejected_control_signals,
            |request, delivered, overwrote, skipped| {
                signals.push(DispatchedSignal {
                    target: request.target.clone(),
                    signal: request.signal.clone(),
                    priority: request.priority,
                    overwrote_earlier: overwrote,
                    delivered: delivered && !skipped,
                });
            },
        );
        DispatchCapture {
            signals,
            same_tier_conflicts,
        }
    }

    fn drain_tiers(
        &mut self,
        equipment: &mut [Box<dyn Equipment>],
        warnings: &mut WarningLog,
        rejected_control_signals: &mut u64,
        mut on_signal: impl FnMut(&DispatchRequest, bool, bool, bool),
    ) {
        // NOTE: `seen_targets` is deliberately NOT cleared here. The per-step
        // ledger is reset by `begin_step()` exactly once per timestep so that
        // a lower-priority signal queued after a higher-priority one in an
        // earlier pass does not silently overwrite it.

        for (tier_idx, tier_que) in self.by_tier.iter_mut().enumerate() {
            for request in tier_que.drain(..) {
                match &request.target {
                    DispatchTarget::ByName(_) => {
                        let prior_higher = self.seen_targets.iter().any(|&(ref t, prev_tier)| {
                            t.conflicts_with(&request.target) && tier_idx < prev_tier
                        });
                        if prior_higher {
                            tracing::debug!(
                                target_equipment = ?request.target,
                                priority = ?request.priority,
                                "lower priority signal rejected: a higher priority signal already applied to this target"
                            );
                            on_signal(&request, false, false, true);
                            continue;
                        }

                        let overwrote = self.seen_targets.iter().any(|&(ref t, prev_tier)| {
                            t.conflicts_with(&request.target) && tier_idx > prev_tier
                        });
                        if overwrote {
                            tracing::debug!(
                                target_equipment = ?request.target,
                                priority = ?request.priority,
                                "higher priority signal overwriting earlier signal for same equipment"
                            );
                        }
                        self.seen_targets.push((request.target.clone(), tier_idx));

                        let delivered =
                            route_request(&request, equipment, warnings, rejected_control_signals);
                        on_signal(&request, delivered, overwrote, false);
                    }
                    DispatchTarget::ByEndUse(end_use) => {
                        // Expand ByEndUse to individual ByName targets per
                        // matching equipment. This ensures seen_targets contains
                        // only homogeneous ByName entries, so conflicts_with
                        // comparisons are reliable across dispatch passes.
                        let mut any_matched = false;
                        let mut any_delivered = false;
                        let mut any_overwrote = false;
                        let mut any_skipped = false;

                        for eq in equipment.iter_mut() {
                            if eq.descriptor().end_use != *end_use {
                                continue;
                            }
                            // Matched: the signal was routed at a real target.
                            // An apply failure below is a per-equipment
                            // rejection, NOT a "target not found": the
                            // not-found increment must not fire for it
                            // (mirrors `apply_to_matching`, which marks
                            // delivered on match before applying).
                            any_matched = true;
                            let eq_name = eq.descriptor().name.clone();
                            let by_name = DispatchTarget::ByName(Arc::from(eq_name.as_str()));

                            let prior_higher =
                                self.seen_targets.iter().any(|&(ref t, prev_tier)| {
                                    t.conflicts_with(&by_name) && tier_idx < prev_tier
                                });
                            if prior_higher {
                                tracing::debug!(
                                    target_equipment = eq_name,
                                    priority = ?request.priority,
                                    "lower priority ByEndUse signal rejected: a higher priority signal already applied to this target"
                                );
                                any_skipped = true;
                                continue;
                            }

                            let overwrote = self.seen_targets.iter().any(|&(ref t, prev_tier)| {
                                t.conflicts_with(&by_name) && tier_idx > prev_tier
                            });
                            if overwrote {
                                any_overwrote = true;
                                tracing::debug!(
                                    target_equipment = eq_name,
                                    priority = ?request.priority,
                                    "higher priority signal overwriting earlier signal for same equipment via ByEndUse expansion"
                                );
                            }

                            self.seen_targets.push((by_name, tier_idx));

                            if let Err(err) = eq.apply_control(&request.signal) {
                                *rejected_control_signals += 1;
                                warnings.push(format!(
                                    "control apply failed for '{}' : {err}",
                                    eq_name
                                ));
                            } else {
                                any_delivered = true;
                            }
                        }

                        if !any_matched && !any_skipped {
                            *rejected_control_signals += 1;
                            warnings.push(format!(
                                "control target not found by end-use: {:?}",
                                end_use
                            ));
                        }

                        on_signal(&request, any_delivered, any_overwrote, any_skipped);
                    }
                }
            }
        }

        // Code-logic check no simulation input can trigger: ByEndUse targets
        // are expanded to ByName unconditionally above, so a non-ByName entry
        // in the ledger can only be a dispatcher bug: the class of check
        // that stays a debug_assert (input-triggerable invariant enforcement
        // belongs to the always-on checks instead).
        #[cfg(debug_assertions)]
        debug_assert!(
            self.seen_targets
                .iter()
                .all(|(t, _)| matches!(t, DispatchTarget::ByName(_))),
            "seen_targets must contain only ByName entries — ByEndUse targets must be expanded before recording"
        );
    }
}

fn route_request(
    request: &DispatchRequest,
    equipment: &mut [Box<dyn Equipment>],
    warnings: &mut WarningLog,
    rejected_control_signals: &mut u64,
) -> bool {
    match &request.target {
        DispatchTarget::ByName(name) => {
            let delivered = apply_to_matching(
                equipment,
                &request.signal,
                warnings,
                rejected_control_signals,
                |eq| eq.descriptor().name.as_str() == &**name,
            );
            if !delivered {
                *rejected_control_signals += 1;
                let instance_count = equipment
                    .iter()
                    .filter(|eq| {
                        let n = eq.descriptor().name.as_str();
                        n == &**name || n.starts_with(&format!("{name} #"))
                    })
                    .count();
                if instance_count >= 2 {
                    warnings.push(format!(
                        "control target '{name}' is ambiguous — {instance_count} instances exist. Use ByEndUse or a qualified name like '{name} #1'"
                    ));
                } else {
                    warnings.push(format!("control target not found by name: {name}"));
                }
            }
            delivered
        }
        DispatchTarget::ByEndUse(end_use) => {
            let delivered = apply_to_matching(
                equipment,
                &request.signal,
                warnings,
                rejected_control_signals,
                |eq| eq.descriptor().end_use == *end_use,
            );
            if !delivered {
                *rejected_control_signals += 1;
                warnings.push(format!(
                    "control target not found by end-use: {:?}",
                    end_use
                ));
            }
            delivered
        }
    }
}

fn apply_to_matching(
    equipment: &mut [Box<dyn Equipment>],
    signal: &hares_types::ControlSignal,
    warnings: &mut WarningLog,
    rejected_control_signals: &mut u64,
    matches: impl Fn(&dyn Equipment) -> bool,
) -> bool {
    let mut delivered = false;
    for eq in equipment.iter_mut() {
        if matches(&**eq) {
            delivered = true;
            if let Err(err) = eq.apply_control(signal) {
                *rejected_control_signals += 1;
                warnings.push(format!(
                    "control apply failed for '{}' : {err}",
                    eq.descriptor().name
                ));
            }
        }
    }
    delivered
}

fn compute_equipment_execution_order(equipment: &[&dyn Equipment]) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..equipment.len()).collect();
    indices.sort_by_key(|&idx| stage_rank(equipment[idx].descriptor().stage));
    indices
}

fn compute_equipment_dispatch_targets(equipment: &[&dyn Equipment]) -> Vec<DispatchTarget> {
    equipment
        .iter()
        .map(|eq| DispatchTarget::ByName(Arc::from(eq.descriptor().name.as_str())))
        .collect()
}

/// Pre-resolve the per-zone thermal consistency wiring: for each thermal
/// port accumulator (in `ports.thermal` order) collect the equipment indices
/// whose descriptor assigns that zone and whose end use is HVAC heating or
/// cooling. Only `HvacHeating` and `HvacCooling` categories are compared by
/// the check; non-HVAC equipment is excluded even when it reports a zone.
/// Called at assembly and wherever the equipment list changes; the per-step
/// check is a lookup plus a sum over these lists.
fn build_hvac_thermal_consistency(
    ports: &PortSlots,
    equipment: &[&dyn Equipment],
) -> Vec<Vec<usize>> {
    ports
        .thermal
        .iter()
        .map(|acc| {
            equipment
                .iter()
                .enumerate()
                .filter(|(_, eq)| {
                    let desc = eq.descriptor();
                    desc.zone == Some(acc.zone)
                        && (desc.end_use == EndUse::HVAC_HEATING
                            || desc.end_use == EndUse::HVAC_COOLING)
                })
                .map(|(idx, _)| idx)
                .collect()
        })
        .collect()
}

/// Register PV array orientations as environment surfaces so Perez irradiance
/// is computed for them. PV orientations use quantised surface IDs that differ
/// from the sequential envelope boundary IDs.
fn register_pv_surfaces(specs: &[hares_io::EquipmentSpec], env: &mut EnvironmentManager) {
    use hares_equipment::pv::surface_id_for_orientation;
    for spec in specs.iter().filter(|s| s.name == "PV") {
        let tilt = spec
            .parameters
            .get("tilt_deg")
            .and_then(|v| v.as_f64())
            .unwrap_or(20.0);
        let az = spec
            .parameters
            .get("azimuth_deg")
            .and_then(|v| v.as_f64())
            .unwrap_or(180.0);
        if let Ok(sid) = surface_id_for_orientation(tilt, az, 5.0) {
            env.register_surface(crate::environment::SurfaceGeometry {
                surface_id: sid,
                azimuth_deg: az,
                tilt_deg: tilt,
                area_m2: 1.0, // area irrelevant for Perez -- only orientation matters
                omni_directional: false,
            });
        }
    }
}

/// Auto-attach PV arrays to the closest matching roof boundary by orientation.
/// Sets `attached_boundary_id` on matching specs so the thermal model can
/// account for PV shading.
fn attach_pv_to_roofs(specs: &mut [hares_io::EquipmentSpec], building: &Building) {
    use hares_io::hpxml::building::BoundaryType;

    let roofs: Vec<(u32, f64, f64, f64)> = building
        .boundaries
        .iter()
        .enumerate()
        .filter(|(_, b)| b.boundary_type == BoundaryType::Roof)
        .map(|(idx, b)| {
            (
                idx as u32,
                b.azimuth_deg.unwrap_or(180.0),
                b.tilt_deg.unwrap_or(0.0),
                b.area_m2,
            )
        })
        .collect();

    for spec in specs.iter_mut().filter(|s| s.name == "PV") {
        if spec.parameters.contains_key("attached_boundary_id") {
            continue;
        }
        let pv_az = spec
            .parameters
            .get("azimuth_deg")
            .and_then(|v| v.as_f64())
            .unwrap_or(180.0);
        let pv_tilt = spec
            .parameters
            .get("tilt_deg")
            .and_then(|v| v.as_f64())
            .unwrap_or(20.0);

        // Find the closest roof within tolerance (15° azimuth, 10° tilt).
        let best = roofs
            .iter()
            .filter(|(_, az, tilt, _)| {
                let az_diff = (*az - pv_az).abs().min(360.0 - (*az - pv_az).abs());
                az_diff <= 15.0 && (*tilt - pv_tilt).abs() <= 10.0
            })
            .min_by(|(_, az_a, tilt_a, _), (_, az_b, tilt_b, _)| {
                let da = (*az_a - pv_az).abs().min(360.0 - (*az_a - pv_az).abs())
                    + (*tilt_a - pv_tilt).abs();
                let db = (*az_b - pv_az).abs().min(360.0 - (*az_b - pv_az).abs())
                    + (*tilt_b - pv_tilt).abs();
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            });

        if let Some(&(roof_id, _, _, _)) = best {
            spec.parameters
                .insert("attached_boundary_id".into(), serde_json::json!(roof_id));
        }
    }
}

/// Compute PV panel coverage on attached roofs and register shading with the
/// environment. Reduces incident solar on envelope surfaces proportionally.
fn register_pv_roof_shading(
    specs: &[hares_io::EquipmentSpec],
    building: &Building,
    env: &mut EnvironmentManager,
) {
    let mut coverage_by_roof: std::collections::HashMap<u32, f64> =
        std::collections::HashMap::new();

    for spec in specs.iter().filter(|s| s.name == "PV") {
        let boundary_id = spec
            .parameters
            .get("attached_boundary_id")
            .and_then(|v| v.as_u64());
        let capacity_kw = spec.parameters.get("capacity_kw").and_then(|v| v.as_f64());

        if let (Some(bid), Some(cap)) = (boundary_id, capacity_kw) {
            let roof_area = building
                .boundaries
                .get(bid as usize)
                .map(|b| b.area_m2)
                .unwrap_or(1.0);
            // ~2 m² per 420 W panel
            let collector_area = cap * 1000.0 / 420.0 * 2.0;
            *coverage_by_roof.entry(bid as u32).or_default() += collector_area / roof_area;
        }
    }

    for (surface_id, coverage) in coverage_by_roof {
        env.set_pv_roof_coverage(surface_id, coverage);
    }
}

/// Verify that every zone referenced by a port declaration exists in the
/// environment model. Equipment targeting a nonexistent zone would silently
/// orphan its contributions — reject it at construction time instead.
fn validate_equipment_zones(
    decls: &[PortDeclaration],
    env_zone_ids: &HashSet<ZoneId>,
) -> Result<()> {
    for decl in decls {
        if let Some(zone) = decl.zone
            && !env_zone_ids.contains(&zone)
        {
            return Err(HaresError::Dwelling(format!(
                "equipment declares port for zone {zone:?} \
                     which does not exist in the environment model"
            )));
        }
    }
    Ok(())
}

/// Verify that every fluid-loop port declaration references a loop ID that was
/// allocated to equipment via [`loop_allocator::allocate_loop_ids`].
///
/// A fluid port with an unallocated loop ID indicates an equipment constructor
/// that hardcoded a loop ID instead of using its typed config — the
/// contribution would be orphaned with no corresponding accumulator in the
/// fluid solver.
fn validate_equipment_loops(
    decls: &[PortDeclaration],
    allocated_loop_ids: &HashSet<u16>,
) -> Result<()> {
    for decl in decls {
        if let Some(loop_id) = decl.loop_id
            && !allocated_loop_ids.contains(&loop_id.0)
        {
            return Err(HaresError::Dwelling(format!(
                "equipment declares fluid port for loop {loop_id:?} \
                     which has not been allocated to any equipment spec"
            )));
        }
    }
    Ok(())
}

/// Top-level single-dwelling simulation orchestrator.
pub struct Dwelling {
    pub bldg_id: i64,
    /// Set when the dwelling panicked during [`step()`](Self::step) and must
    /// never be stepped again.  A `failed` dwelling is permanently excluded
    /// from fleet simulation; its state may be inconsistent.
    pub failed: bool,
    equipment: Vec<Box<dyn Equipment>>,
    equipment_id_by_name: HashMap<String, EquipmentId>,
    /// Next never-reused equipment id for `add_equipment` /
    /// `replace_equipment` auto-assignment. Monotonic: advanced past every
    /// id that enters the equipment vector (assembly-assigned or
    /// caller-explicit), never decremented — `remove_equipment` frees no
    /// ids. Guarantees auto-assignment can never collide with an id already
    /// in the vector.
    next_equipment_id: u32,
    /// Private: the dwelling's port, zone and equipment caches are built
    /// against this solver's geometry, so it is read through
    /// [`Self::thermal_solver`] and changed only by the dwelling.
    thermal_solver: ThermalSolver,
    pub humidity_solver: HumiditySolver,
    pub electrical_solver: ElectricalSolver,
    pub fluid_solver: FluidSolver,
    pub clock: SimClock,
    pub environment: EnvironmentManager,
    pub ports: PortSlots,
    /// Pre-allocated snapshot buffer for equipment port rollback.
    /// On each equipment step the current `ports` state is copied into this
    /// buffer (reusing existing Vec capacities) via [`PortSlots::copy_into`].
    /// If the step fails, `ports` and `rollback_ports` are swapped (O(1),
    /// no allocation) to restore the pre-step state.  Initialised from the
    /// same port declarations as `ports` so capacities always match.
    rollback_ports: PortSlots,
    pub recorder: Option<StreamingRecorder>,
    pub rng: ChaCha8Rng,
    /// Bounded warning buffer (first [`WarningLog::CAPACITY`] messages kept,
    /// overflow counted) — see [`Dwelling::take_warnings`].
    pub warnings: WarningLog,
    /// Reused scratch for the per-step warning drain: a step in which no
    /// equipment warns allocates nothing.
    warning_scratch: Vec<Warning>,
    /// Per-equipment HPXML setpoint reconciliation records, keyed by instance name.
    /// Populated during `from_config()` / `from_preparsed()` from the typed configs
    /// attached to each `EquipmentSpec`.
    pub setpoints_reconciled_by_equipment: HashMap<String, Option<Vec<SetpointReconciliation>>>,

    /// Roof geometry extracted from HPXML at construction time.
    pub roof_info: RoofInfo,
    /// Wall azimuths from HPXML, for PV sizing fallback orientation.
    pub wall_azimuths: Vec<f64>,
    /// Site latitude from HPXML, for PV sizing.
    pub latitude_deg: Option<f64>,
    /// Facility type from HPXML, for roof shape inference.
    pub facility_type: Option<String>,
    /// PV panel defaults loaded from `defaults/pv/*.toml`, keyed by file stem.
    /// Populated during `from_preparsed` so PV sizing call sites can resolve
    /// panel specifications when the caller provides no explicit overrides.
    pub pv_panel_defaults: HashMap<String, PvPanelDefaults>,

    control_dispatcher: ControlDispatcher,
    price_signal: PriceSignal,
    tariff_evaluator: Option<TariffEvaluator>,
    billing_summaries: Vec<BillingPeriodSummary>,
    prior_electrical_summary: ElectricalSummary,
    latest_env: EnvironmentState,
    simulation_results: SimulationResults,
    custom_domain_solvers: Vec<Box<dyn DomainSolver>>,
    /// Pre-allocated DomainUpdate buffers for each built-in solver, reused per step.
    thermal_update_buf: hares_types::DomainUpdate,
    humidity_update_buf: hares_types::DomainUpdate,
    electrical_update_buf: hares_types::DomainUpdate,
    fluid_update_buf: hares_types::DomainUpdate,
    /// Pre-allocated DomainUpdate buffers for custom domain solvers, one per solver.
    custom_update_bufs: Vec<hares_types::DomainUpdate>,
    #[cfg(debug_assertions)]
    test_panic_on_step: bool,
    #[cfg(debug_assertions)]
    test_assert_panic_on_step: bool,
    /// Test-only: causes the next thermal invariant check in
    /// [`check_step_invariants`](Self::check_step_invariants) to receive deliberately
    /// broken balance terms, forcing `InvariantViolation { check_name: "thermal_balance" }`.
    #[cfg(any(test, debug_assertions))]
    test_thermal_invariant_failure: bool,
    /// Test-only: causes the HVAC delivered-energy non-negativity invariant
    /// checks in [`check_step_invariants`](Self::check_step_invariants) to receive a
    /// deliberately negative `hvac_heating_w` and `hvac_cooling_w`, forcing
    /// `NegativeDeliveredEnergy`.
    #[cfg(any(test, debug_assertions))]
    test_hvac_negative_energy_failure: bool,
    /// Everything the step path reads that derives from the equipment and
    /// actor rosters, installed as a whole by each roster change.
    roster: RosterCaches,
    /// Pre-allocated buffer for RFC 3339 timestamp formatting, reused each step.
    timestamp_buf: String,
    /// Schedule column index for the occupancy time series, or `None` if the
    /// schedule does not include an occupancy column.
    occupancy_column_idx: Option<usize>,
    /// Scale factor applied to the raw occupancy schedule fraction (0–1) to
    /// convert it to a person count.  Equals `number_of_occupants` from the
    /// Occupancy equipment spec (defaults to 1.0 when unspecified).
    occupancy_scale: f64,
    prev_humidity_ratios: Vec<(ZoneId, f64)>,
    /// Run-total health counters (port rollbacks, rejected control signals,
    /// clamped actions, curve-index clamps, warm-up outcome). Every event is
    /// recorded unconditionally in every build profile; the totals are never
    /// reset per step.
    health: RunHealth,
    /// Actor decision-makers that emit control signals each timestep.
    /// Actors execute in the order determined by the scheduler plan
    /// (phase ordinal, then within-phase priority, then registration order).
    /// Signals are dispatched by PriorityTier.
    actors: Vec<Box<dyn Actor>>,
    /// Per-timestep actor execution plan. Actors register for phases and
    /// the scheduler builds a deterministic execution order. The plan is
    /// rebuilt when actors are added or removed.
    scheduler: StepScheduler,
    /// Names of actors that were auto-registered from equipment seeds.
    /// Used to evict stale built-in actors when set_tariff() triggers rebuild.
    auto_registered_actor_names: HashSet<String>,
    /// Offset (from `RNG_STREAM_EV_DRIVER_BASE`) of the RNG stream the next
    /// built-in EV driver receives. It never moves back: each driver built
    /// takes the next stream (a driver rebuilt with new prices keeps its
    /// own), and the checkpoint carries it, so no two live built-in drivers
    /// share one.
    next_ev_driver_stream: u64,
    /// Pre-allocated buffer for actor dispatch requests, reused each step.
    actor_dispatch_buf: Vec<DispatchRequest>,
    /// Solver feedback actor: bridges thermal solver to IdealHvac equipment.
    /// Stored separately (not in actors Vec) so dwelling can call collect_and_solve().
    solver_feedback_actor: SolverFeedbackActor,
    /// Previous-step zone temperatures for ActorInterest::ZoneTemperatureDelta.
    /// Empty on the very first step (all interests trigger).
    prev_zone_temps: HashMap<ZoneId, f64>,
    /// Zone temperatures from the step before the previous step (step N-2).
    /// Used by interest_triggered as a one-step-lagged comparison target so the
    /// delta reflects an actual per-step change rather than comparing
    /// latest_env (not yet updated by the Step 4 envelope solve when
    /// ActorDecide runs at Step 1e-1f) against prev_zone_temps (same source).
    prior_zone_temps: HashMap<ZoneId, f64>,
    /// Previous-step price signal for ActorInterest::PriceSignalChange.
    prev_price_signal: PriceSignal,
    /// Previous-step equipment operating modes for ActorInterest::EquipmentModeChange.
    prev_equipment_modes: HashMap<EquipmentId, Option<OperatingMode>>,
    /// Numerical invariant checker, allocated once and reused each step.
    invariant_checker: InvariantChecker,
    /// Pre-allocated scratch buffer for conditioned zone temps in check_step_invariants.
    invariant_conditioned_temps: Vec<f64>,
    /// Pre-allocated scratch buffer for unconditioned zone temps in check_step_invariants.
    invariant_unconditioned_temps: Vec<f64>,
    /// Pre-allocated scratch buffer for tank node temps in check_step_invariants.
    invariant_tank_temps: Vec<f64>,
    /// Pre-computed tank node telemetry keys, avoiding format!() per step.
    tank_node_keys: Vec<String>,
    /// Pre-allocated scratch buffer for infiltration latent by zone in check_step_invariants.
    invariant_infiltration_latent: Vec<(ZoneId, f64)>,
    /// Pre-allocated scratch maps for semi-implicit infiltration coupling data.
    invariant_infiltration_m_dot: HashMap<ZoneId, f64>,
    invariant_infiltration_w_outdoor: HashMap<ZoneId, f64>,
    /// Whether the dwelling is running a warm-up convergence loop.
    /// Drives warm-up-specific accounting (e.g. daily peak accumulation for
    /// warm-up residuals); enforcement is NOT relaxed during warm-up.
    is_warming_up: bool,
    /// HVAC delivery of the step just simulated, in delivered-energy
    /// magnitudes (heating ≥ 0 W into zones, cooling ≥ 0 W removed). Updated
    /// every step by `run_timestep`; the warm-up loop accumulates these into
    /// daily peaks for `WarmupResiduals`.
    step_hvac_heating_w: f64,
    step_hvac_cooling_w: f64,
    /// Per-zone moisture invariant capture data from `check_moisture`.
    /// Populated by check_step_invariants; consumed by the observer push
    /// when both debug_assertions and `observe` are active.
    #[cfg(all(feature = "observe", debug_assertions))]
    invariant_moisture_capture: Vec<MoistureZoneInvariant>,
    /// Set to `true` when `load_checkpoint` restores state. The first
    /// post-restore `run_timestep` checks that `equipment_core` is populated
    /// for every equipment instance, then resets this flag to `false`.
    restored_from_checkpoint: bool,
    /// Per-zone conditioning status, aligned with `latest_env.zones` order.
    /// `true` = conditioned (HVAC-served), `false` = unconditioned (attic, garage, etc.).
    zone_is_conditioned: Vec<bool>,
    /// Islanded-operation imbalance accounting, recomputed every step after
    /// the electrical solver resolves net grid power. While islanded there is
    /// no service connection, so any residual net flow at the "meter" is a
    /// power-balance violation the island sources could not resolve:
    /// `island_unserved_kw` is the load the island sources failed to cover
    /// (would-be phantom import, `net_active_kw().max(0.0)`); 0.0 when not
    /// islanded. See docs/outage-behavior.md.
    island_unserved_kw: f64,
    /// Surplus generation the island could not absorb (would-be phantom
    /// export, `(-net_active_kw()).max(0.0)`); 0.0 when not islanded.
    island_excess_kw: f64,
    /// Output config retained for schema rebuilds when equipment changes.
    output_verbosity: u8,
    output_chunk_size: usize,
    output_format: hares_io::OutputFormat,
    output_path: PathBuf,
    write_output: bool,
    retain_batches: bool,
    output_rotation: hares_io::RotationPolicy,
    /// Simulation config retained for output-schema rebuilds when equipment
    /// changes: the incremental metrics calculator re-initializes against
    /// the rebuilt schema.
    sim_config: SimulationConfig,
    /// Diagnostic CSV writer, opened when `output_verbosity >= 4`.
    diagnostic_writer: Option<std::io::BufWriter<std::fs::File>>,
    #[cfg(feature = "profiling")]
    profiling: DwellingProfilingSummary,
    /// Run-total per-actor scheduler-entry timings (one slot per registered
    /// actor, rebuilt by every roster change with the actor list);
    /// read once by `profiling_summary()`, never cleared per step.
    #[cfg(feature = "profiling")]
    actor_timings: ActorTimings,
    #[cfg(feature = "observe")]
    observer_buf: Option<ObserverBuffer>,
    /// Accumulates per-step data for post-hoc diagnostic checks (unmet hours,
    /// short-cycling, freezing excursions, simultaneous heating/cooling).
    /// Evaluated at end-of-run by `run_post_hoc_checks()`.
    #[cfg(feature = "observe")]
    diagnostic_accum: Option<DiagnosticAccumulator>,
    #[cfg(any(debug_assertions, feature = "observe_detailed"))]
    envelope_diagnostics: EnvelopeDiagnostics,
}

/// Borrowed view of per-step actor-interest filter state passed to
/// `Dwelling::interest_triggered`. Groups the shift-register zone-temp maps,
/// the previous price signal, equipment mode snapshot, and the equipment
/// metadata needed to resolve `DispatchTarget` references into a single
/// argument so the hot-loop call site stays readable.
struct InterestFilterState<'a> {
    /// Step N-1 zone temperatures (used as first-step sentinel for PriceSignalChange).
    prev_zone_temps: &'a HashMap<ZoneId, f64>,
    /// Step N-2 zone temperatures (used for the actual ZoneTemperatureDelta comparison).
    prior_zone_temps: &'a HashMap<ZoneId, f64>,
    prev_price_signal: &'a PriceSignal,
    prev_equipment_modes: &'a HashMap<EquipmentId, Option<OperatingMode>>,
    equipment_id_by_name: &'a HashMap<String, EquipmentId>,
    equipment: &'a [Box<dyn Equipment>],
}

/// Borrowed paths of the ResStock-style input files for the
/// [`Dwelling::from_hpxml`] constructor family, grouped so the constructors
/// take one coherent source bundle.
pub struct HpxmlInputs<'a> {
    pub hpxml_path: &'a Path,
    pub schedule_path: &'a Path,
    pub weather_path: &'a Path,
}

impl Dwelling {
    /// Builds a dwelling from HPXML + schedule/weather paths and simulation config.
    pub fn from_config(config: DwellingConfig) -> Result<Self> {
        let building = parse_hpxml(&config.hpxml_path)
            .map_err(|err| HaresError::Io(format!("HPXML parse failed: {err}")))?;

        let weather = parse_weather(&config.weather_path)
            .map_err(|err| HaresError::Io(format!("weather parse failed: {err}")))?;

        let schedule_raw = config.load_schedule(&building, &weather.meta)?;

        let target_step_secs = duration_to_u32_secs(config.sim_config.time_res)?;
        let schedule = schedule_raw
            .resample(target_step_secs)
            .map_err(|err| HaresError::Io(format!("schedule resample failed: {err}")))?;

        Self::from_preparsed(config, building, weather, schedule)
    }

    /// Builds a dwelling directly from ResStock-style input files.
    ///
    /// No defaults directory is configured, so the schedule file must name
    /// every mapped column the building's equipment needs: an equipment with
    /// no schedule column, no HPXML fractions and no default profile is a
    /// construction error. Pass a defaults directory via
    /// [`Self::from_hpxml_with_write_output`] or [`Self::from_config`].
    pub fn from_hpxml(
        hpxml_path: &Path,
        schedule_path: &Path,
        weather_path: &Path,
        start_time: DateTime<FixedOffset>,
        time_res: Duration,
        duration: Duration,
        overrides: Option<Value>,
    ) -> Result<Self> {
        Self::from_hpxml_with_write_output(
            HpxmlInputs {
                hpxml_path,
                schedule_path,
                weather_path,
            },
            start_time,
            time_res,
            duration,
            overrides,
            None,
            None,
        )
    }

    /// [`Self::from_hpxml`] with an optional output-write override, for tests
    /// that exercise this constructor's defaults without paying for a file.
    ///
    /// `defaults_path` feeds `DwellingConfig.defaults_path`: the default
    /// schedule profiles load from there, and equipment whose schedule source
    /// is missing otherwise (no schedule column, no HPXML fractions) errors.
    /// The BEopt example schedule this constructor family is tested with does
    /// not name every mapped column, so the warmup regression passes the
    /// repo's `defaults/` directory.
    pub fn from_hpxml_with_write_output(
        inputs: HpxmlInputs<'_>,
        start_time: DateTime<FixedOffset>,
        time_res: Duration,
        duration: Duration,
        overrides: Option<Value>,
        write_output_override: Option<bool>,
        defaults_path: Option<PathBuf>,
    ) -> Result<Self> {
        let HpxmlInputs {
            hpxml_path,
            schedule_path,
            weather_path,
        } = inputs;
        let sim_config = SimulationConfig {
            start_time,
            duration,
            time_res,
            output_verbosity: 0,
            output_path: None,
            write_output: write_output_override.unwrap_or(true),
            output_format: hares_io::OutputFormat::Csv,
            output_chunk_size: 10_000,
            setpoint_deadband_c: None,
            master_seed: 0,
            civil_timezone: None,
            site_location: hares_io::SiteLocationOverride::default(),
            retain_batches: false,
            rotation: hares_io::RotationPolicy::None,
        };

        let config = DwellingConfig {
            hpxml_path: hpxml_path.to_path_buf(),
            schedule_path: Some(schedule_path.to_path_buf()),
            weather_path: weather_path.to_path_buf(),
            defaults_path,
            sim_config,
            overrides,
            bldg_id: 0,
            initialization_duration: Some(StdDuration::from_secs(7 * 24 * 3600)),
            resample_overrides: None,
            patches: None,
        };
        Self::from_config(config)
    }

    /// OCHRE-compatible constructor from kwargs.
    pub fn from_ochre_kwargs(kwargs: HashMap<String, Value>) -> Result<Self> {
        let hpxml_path = required_path(&kwargs, "hpxml_file")?;
        let schedule_path = required_path(&kwargs, "hpxml_schedule_file")?;
        let weather_path = required_path(&kwargs, "weather_file")?;
        let start_time = required_datetime(&kwargs, "start_time")?;
        let time_res = required_duration(&kwargs, "time_res")?;
        let duration = required_duration(&kwargs, "duration")?;
        let overrides = kwargs.get("Equipment").cloned();

        Self::from_hpxml(
            &hpxml_path,
            &schedule_path,
            &weather_path,
            start_time,
            time_res,
            duration,
            overrides,
        )
    }

    /// Constructor for synthetic TOML dwelling definitions (BESTEST-style inputs).
    pub fn from_toml_config(path: &Path) -> Result<Self> {
        Self::from_toml_config_with_write_output(path, None)
    }

    /// Constructor for synthetic TOML dwelling definitions with optional
    /// output-write override.
    pub fn from_toml_config_with_write_output(
        path: &Path,
        write_output_override: Option<bool>,
    ) -> Result<Self> {
        let toml_str = std::fs::read_to_string(path)
            .map_err(|err| HaresError::Io(format!("failed to read TOML config: {err}")))?;
        let mut config: SyntheticTomlConfig = toml::from_str(&toml_str)
            .map_err(|err| HaresError::Io(format!("failed to parse TOML config: {err}")))?;
        if let Some(write_output) = write_output_override {
            config.output.write_output = write_output;
        }

        let sim_config = SimulationConfig {
            start_time: config.simulation.start_time,
            duration: Duration::seconds(config.simulation.duration_s),
            time_res: Duration::seconds(config.simulation.time_res_s),
            output_verbosity: config.output.output_verbosity,
            output_path: config.output.output_path.as_ref().map(PathBuf::from),
            write_output: config.output.write_output,
            output_format: config.output.output_format,
            output_chunk_size: config.output.output_chunk_size,
            setpoint_deadband_c: None,
            master_seed: config.output.master_seed,
            civil_timezone: None,
            site_location: hares_io::SiteLocationOverride::default(),
            retain_batches: config.output.retain_batches,
            rotation: config.output.rotation,
        };
        validate_sim_config(&sim_config)?;

        let schedule_result = build_synthetic_schedule(&config)?;
        let hpxml_building = build_synthetic_building(
            &config,
            schedule_result.event_window_schedule_col,
            schedule_result.event_probability_schedule_col,
        )?;
        let weather = build_synthetic_weather(&config, path)?;
        let dwelling_config = DwellingConfig {
            hpxml_path: path.to_path_buf(),
            // The synthetic schedule is built below from the TOML definition
            // and handed to `from_preparsed` directly; the config's schedule
            // source is never read on this path.
            schedule_path: None,
            weather_path: path.to_path_buf(),
            defaults_path: None,
            sim_config,
            overrides: config.overrides.clone(),
            bldg_id: config.building_id.unwrap_or(0),
            initialization_duration: config
                .simulation
                .initialization_duration_s
                .map(StdDuration::from_secs),
            resample_overrides: None,
            patches: None,
        };

        Self::from_preparsed(
            dwelling_config,
            hpxml_building,
            weather,
            schedule_result.schedule,
        )
    }

    /// PV panel defaults loaded from `defaults/pv/*.toml`, keyed by file stem.
    /// Returns an empty map when no panel specs were loaded.
    pub fn pv_panel_defaults(&self) -> &HashMap<String, PvPanelDefaults> {
        &self.pv_panel_defaults
    }

    fn from_preparsed(
        config: DwellingConfig,
        building: Building,
        weather: WeatherTimeSeries,
        schedule: ScheduleTimeSeries,
    ) -> Result<Self> {
        let blueprint = DwellingBlueprint::from_parts(config, building, weather, schedule)?;
        build_from_blueprint(blueprint)
    }
}

pub(crate) fn build_from_blueprint(mut bp: DwellingBlueprint) -> Result<Dwelling> {
    let mut warnings = WarningLog::new();
    // Construction-time warnings first, in report order: the blueprint's
    // HPXML parse warnings, then its equipment-resolution warnings. The
    // schedule-injection warnings and each equipment's `init` warnings join
    // them in the body below, in that order. The log is filled before the
    // body runs so every failure path below can emit it.
    for warning in bp.equipment_warnings.drain(..) {
        warnings.push_warning(warning);
    }
    let result = build_from_blueprint_inner(bp, &mut warnings);
    if let Err(err) = &result {
        // A construction failure names what broke; the accumulated
        // warnings are the context that led there. Emit each stored entry
        // (the WarningLog's own formatting) so the error never arrives
        // without its history; the success path keeps the log in the
        // dwelling instead.
        for entry in warnings.iter() {
            tracing::warn!("{entry}");
        }
        if warnings.dropped() > 0 {
            tracing::warn!(
                "warning log capacity ({}) reached; {} additional warning(s) were dropped",
                WarningLog::CAPACITY,
                warnings.dropped()
            );
        }
        tracing::warn!("dwelling construction failed: {err}");
    }
    result
}

fn build_from_blueprint_inner(
    bp: DwellingBlueprint,
    warnings: &mut WarningLog,
) -> Result<Dwelling> {
    let site_location = bp.site_location;
    let local_start = bp.local_start;
    let mut defaults = bp.defaults;
    let config = bp.config;
    let rng = bp.rng;

    let mut equipment_specs = bp.equipment_specs;

    let mut clock = SimClock::new(
        local_start,
        config.sim_config.time_res,
        config.sim_config.duration + bp.init_chrono,
    );
    #[cfg(feature = "dst")]
    {
        clock.civil_tz = bp.parsed_civil_tz;
    }

    let mut environment = EnvironmentManager::new_with_resample(
        bp.weather,
        bp.schedule,
        &bp.building,
        bp.time_res,
        local_start,
        EnvironmentInitOptions {
            civil_timezone: config.sim_config.civil_timezone.as_deref(),
            resample_overrides: config.resample_overrides.as_ref(),
            initial_rng: Some(rng.clone()),
            setpoint_deadband_c: config.sim_config.setpoint_deadband_c,
        },
    )
    .map_err(|err| HaresError::Io(format!("environment initialization failed: {err}")))?;

    // Reused scratch for the per-equipment drains below and in the step
    // loop: a step in which no equipment warns allocates nothing.
    let mut init_warning_scratch: Vec<Warning> = Vec::new();

    // ── WH typed_config debug assertion ─────────────────────────────────
    // WH specs with autosize_water_heater = true will have typed_config = None
    // here because autosizing hasn't run yet; it runs later and rebuilds the
    // typed_config before schedule injection consumes it.
    #[cfg(debug_assertions)]
    for spec in &equipment_specs {
        if matches!(
            spec.name.as_str(),
            "Electric Resistance Water Heater"
                | "Gas Water Heater"
                | "Heat Pump Water Heater"
                | "Tankless Water Heater"
                | "Gas Tankless Water Heater"
        ) {
            let will_be_autosized = spec
                .parameters
                .get("autosize_water_heater")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if !will_be_autosized {
                assert!(
                    spec.typed_config.is_some(),
                    "WH spec '{}' has no typed_config — schedule injection will panic",
                    spec.name
                );
            }
        }
    }

    // Centralized fluid loop ID allocation — must run after wiring
    // (resolve_loop_wiring, inside resolve_equipment) and before
    // equipment construction so every instance receives a unique
    // loop ID above the wired range.
    loop_allocator::allocate_loop_ids(&mut equipment_specs)?;

    // Register PV surfaces with the environment so Perez irradiance is
    // computed for PV orientations (which may not match any envelope surface).
    register_pv_surfaces(&equipment_specs, &mut environment);

    // Auto-attach PV arrays to the closest matching roof surface and
    // register shading coverage on attached roofs.
    attach_pv_to_roofs(&mut equipment_specs, &bp.building);
    register_pv_roof_shading(&equipment_specs, &bp.building, &mut environment);

    let initial_env = environment.update(&clock, &[])?;

    let solvers = build_default_solvers(
        &initial_env,
        &config.sim_config,
        &bp.building,
        &defaults,
        &bp.weather_avgs,
        &equipment_specs,
    )?;

    // ── Zone thermal capacitances for EBM ─────────────────────────────────
    //
    // Convert envelope solver zone capacitances [J/K] to [kWh/K] and store
    // in a lookup map. Injected into each HVAC equipment's config before
    // init() so the equivalent battery model can compute energy state and
    // baseline power from the building's actual thermal mass.
    let zone_cap_kwh_per_k: HashMap<ZoneId, f64> = solvers
        .zone_capacitances_j_k
        .iter()
        .map(|(zone_id, cap_j_k)| (*zone_id, *cap_j_k / J_PER_KWH))
        .collect();

    // ── Autosize HVAC capacities at design conditions ──────────────────────
    //
    // When HPXML omits HeatingCapacity/CoolingCapacity, compute the required
    // capacity from the building's thermal envelope model at ASHRAE design
    // outdoor conditions. Uses EPW "Extremes" header (preferred) or ASHRAE 152
    // climate station lookup (fallback).
    //
    // Must run after the thermal solver is built (it needs the RC model) and
    // before equipment creation (it sets capacities on equipment specs).
    // ACCA Manual S-2017 oversizing factors applied: 1.4x heating, 1.15x cooling.
    {
        let duct_params = match hares_io::hpxml::resolve_hvac::compute_duct_dse_params(&bp.building)
        {
            Ok(dp) => dp,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "autosizing: duct DSE params unavailable; using defaults"
                );
                hares_io::hpxml::resolve_hvac::DuctDseParams::default()
            }
        };

        let indoor_zone = solvers.thermal.config().indoor_zone_id;

        let ctx = crate::dwelling::autosize::AutosizeContext {
            design_conditions: bp.design_conditions,
            weather_lat: site_location.latitude_deg,
            weather_lon: site_location.longitude_deg,
            weather_elevation_m: site_location.elevation_m,
            duct_params,
            internal_gains_w: 0.0,
            internal_gains_latent_w: 0.0,
        };

        crate::dwelling::autosize::autosize_equipment_capacities(
            &mut equipment_specs,
            &solvers.thermal,
            &ctx,
            &bp.building,
            indoor_zone,
        )?;
    }

    // ── Autosize water heater capacities ────────────────────────────────
    //
    // When HPXML water heaters omit HeatingCapacity or TankVolume,
    // compute the required capacity and storage volume using a First-Hour
    // Rating methodology (DOE 10 CFR Part 430 Subpart B Appendix E).
    // Tank volume is sized by bedroom count; heating capacity is computed
    // from the required FHR, usable tank volume, and design temperature
    // rise (setpoint − mains temperature).
    //
    // Runs after HVAC autosizing (no dependency between them) and before
    // equipment creation.
    {
        // FHR sizing by bedroom count is a structural property (number of
        // bedrooms), not an occupancy proxy. The occupancy-adjusted bedroom
        // count used by parse_avg_water_draw_and_bedrooms for draw estimation
        // is intentionally NOT used here so the structural count drives the
        // per-bedroom FHR values (DOE 10 CFR Part 430 App E test procedure
        // bases sizing on the dwelling, not on occupancy).
        let n_bedrooms =
            hares_io::hpxml::extract_bedroom_count(&bp.building, config.patches.as_ref());
        crate::dwelling::autosize::autosize_water_heater_capacities(
            &mut equipment_specs,
            Some(n_bedrooms),
            initial_env.weather.mains_temp_c,
        )?;
    }

    // Enable ideal HVAC on the indoor zone when both heating AND cooling
    // setpoints are configured -- the thermal solver back-calculates the exact
    // load needed to maintain the setpoint at each timestep.
    let mut schedule_warnings: Vec<hares_types::Warning> = Vec::new();
    hares_io::inject_schedule_into_specs(
        &mut equipment_specs,
        environment.schedule_mut(),
        bp.defaults_path.as_deref(),
        &defaults,
        bp.building.foundation_name.as_deref(),
        bp.building.models_garage(),
        &mut schedule_warnings,
    )?;
    for warning in schedule_warnings.drain(..) {
        warnings.push_warning(warning);
    }

    // occupancy_column_idx must be resolved AFTER inject_schedule_into_specs,
    // which may generate an occupancy column from HPXML extension fractions or
    // the default schedule profile when the schedule CSV lacks one.
    let occupancy_column_idx = environment.occupancy_column_idx();

    // Construction-time invariants: run in every build profile.
    {
        hares_io::check_hvac_setpoint_invariants(&equipment_specs);
        hares_io::check_foundation_zone_invariant(&bp.building);
        let equipment_names: Vec<String> = equipment_specs.iter().map(|s| s.name.clone()).collect();
        check_basement_lighting_foundation(
            &equipment_names,
            bp.building.foundation_name.as_deref(),
        )?;
    }
    let override_root = config
        .overrides
        .clone()
        .unwrap_or_else(|| Value::Object(Map::new()));
    // Loud override validation at the assembly boundary where the
    // population is known: an overrides key that matches no equipment name
    // (e.g. an HPXML SystemIdentifier) was previously a silent no-op. Runs
    // before any equipment is constructed so the build fails before
    // side effects, not partway through the population.
    {
        let overridable: Vec<&str> = equipment_specs
            .iter()
            .map(|s| s.name.as_str())
            .filter(|n| !HANDLED_OUTSIDE_REGISTRY.contains(n))
            .collect();
        validate_equipment_override_keys(&override_root, &overridable, HANDLED_OUTSIDE_REGISTRY)?;
    }

    // Read number_of_occupants from the Occupancy spec to scale the raw
    // schedule fraction (0–1) into a person count for internal heat gains.
    #[cfg(any(test, debug_assertions))]
    let has_occupancy_spec;
    let occupancy_scale = match equipment_specs.iter().find(|s| s.name == "Occupancy") {
        Some(spec) => {
            #[cfg(any(test, debug_assertions))]
            {
                has_occupancy_spec = true;
            }
            let val = spec.parameters.get("number_of_occupants").ok_or_else(|| {
                HaresError::Equipment(
                    "Occupancy spec is missing required key 'number_of_occupants'".into(),
                )
            })?;
            val.as_f64().ok_or_else(|| {
                HaresError::Equipment(format!(
                    "Occupancy 'number_of_occupants' must be a valid number, got: {val}"
                ))
            })?
        }
        None => {
            #[cfg(any(test, debug_assertions))]
            {
                has_occupancy_spec = false;
            }
            1.0
        }
    };

    // Invariant: number_of_occupants must be non-negative regardless of
    // derivation path (HPXML NumberofResidents, derived from bedrooms, or default).
    // A negative value indicates a data error in the parser or input.
    #[cfg(any(test, debug_assertions))]
    if has_occupancy_spec && occupancy_scale < 0.0 {
        return Err(HaresError::Dwelling(format!(
            "Occupancy 'number_of_occupants' must be non-negative; got {}; \
             this indicates a data problem in the HPXML parser's occupant-count \
             derivation",
            occupancy_scale
        )));
    }

    // Invariant: if an Occupancy spec was configured the schedule MUST have an
    // occupancy column. `inject_schedule_into_specs` is responsible for generating
    // the column from HPXML extension fractions or the default profile when the
    // schedule CSV lacks one. Absence of both a column AND schedule fractions on the
    // spec is a data integrity error — either the defaults CSV is missing or the
    // Occupancy spec was created without any schedule data source.
    //
    // An absent Occupancy spec is valid (e.g. BESTEST unconditioned structures).
    // ASHRAE HoF 2021 Ch.18 §18.4 — occupant heat gain is a primary driver of
    // cooling load; silently zeroing it produces a systematic underestimate.
    #[cfg(any(test, debug_assertions))]
    if has_occupancy_spec && occupancy_column_idx.is_none() {
        let has_hpxml_fractions = equipment_specs
            .iter()
            .find(|s| s.name == "Occupancy")
            .map(|s| {
                s.parameters.contains_key("weekday_schedule_fractions")
                    || s.parameters.contains_key("weekend_schedule_fractions")
            })
            .unwrap_or(false);
        let detail = if has_hpxml_fractions {
            "Occupancy spec has HPXML extension schedule fractions but the \
             occupancy column was not generated in the schedule timeseries. \
             This is a bug in schedule_resolve::inject_occupancy_schedule — \
             the HPXML profile should have been converted to a schedule column."
        } else {
            "Occupancy spec configured but no occupancy column found in schedule \
             AND no HPXML schedule fractions available on the spec. The default \
             Occupancy profile from Default Schedule Parameters.csv may be missing \
             or unreadable."
        };
        return Err(HaresError::Dwelling(detail.into()));
    }

    // Build zone-to-role map for equipment auto-routing.
    // Maps semantic zone roles (Indoor, Garage, Basement, etc.) to concrete
    // ZoneId values derived from the sorted building zone list.
    let zone_map = {
        let mut map = ZoneMap::new();
        for (idx, zone) in bp.building.zones.iter().enumerate() {
            let id = ZoneId(u16::try_from(idx + 1).unwrap_or(u16::MAX));
            match &zone.zone_type {
                hares_io::hpxml::ZoneType::Conditioned => {
                    map.insert(ZoneRole::Indoor, id);
                }
                hares_io::hpxml::ZoneType::Garage => {
                    map.insert(ZoneRole::Garage, id);
                }
                hares_io::hpxml::ZoneType::Foundation => {
                    map.insert(ZoneRole::Basement, id);
                    map.insert(ZoneRole::Crawlspace, id);
                }
                hares_io::hpxml::ZoneType::Attic => {
                    map.insert(ZoneRole::Attic, id);
                }
                // Not modelled thermal zones — intentionally excluded from ZoneMap.
                hares_io::hpxml::ZoneType::Outdoor
                | hares_io::hpxml::ZoneType::Ground
                | hares_io::hpxml::ZoneType::Adjacent => {}
                hares_io::hpxml::ZoneType::Other(_) => {}
            }
        }
        map
    };

    // Invariant: after zone map construction, verify that every Attic zone's
    // `vented` flag is consistent. By default (no `<Attics>` group in HPXML),
    // attics are vented (ASHRAE 152-2004 default; OCHRE hpxml.py:635 Vented=True).
    // An unvented attic must come from an explicit `<AtticType><Attic><Vented>false`
    // declaration.
    //
    // Known limitation: this block emits observability logs but does not assert.
    // The fix at crates/hares-io/src/hpxml/building.rs (T-0206) makes the
    // `ensure_referenced_zones_exist` / `build_zone_map` divergence structurally
    // impossible — both paths now set `vented: true` for attics — so a runtime
    // assertion would never fire in practice. The `tracing::debug!` log remains as
    // a low-cost diagnostic to confirm attic zone vented status during development.
    #[cfg(debug_assertions)]
    {
        for zone in &bp.building.zones {
            if zone.zone_type == hares_io::hpxml::ZoneType::Attic {
                tracing::debug!(
                    vented = zone.vented,
                    floor_area_m2 = zone.floor_area_m2,
                    "dwelling assembly: Attic zone vented status"
                );
            }
        }
    }

    // Captured before instance naming, which numbers same-class specs by
    // their order and so is not a stable identity for a random stream.
    let rng_identities: Vec<String> = equipment_specs.iter().map(rng_identity).collect();

    // Deduplicate equipment instance names before the uniqueness check.
    // Blueprint callers may add specs with the same name (e.g. two "PV"
    // arrays) via add_equipment_spec(); assign canonical "Name #N" instance
    // names so the duplicate-detection pass below sees distinct names.
    hares_io::hpxml::equipment::assign_instance_names(&mut equipment_specs);

    // Dwelling-assigned equipment identity: inject a unique id into each
    // spec's config channel (raw parameters / typed payload) before
    // construction, so every `EquipmentId`-keyed structure in the dwelling
    // — `equipment_core`, `equipment_id_by_name`, `prev_equipment_modes` —
    // addresses equipment individually instead of collapsing onto a single
    // shared id. Assembly-time only: blueprint callers can add specs after
    // parse, so the population is not final until here.
    hares_io::hpxml::equipment::assign_equipment_ids(&mut equipment_specs);

    // Equipment names whose loads are handled outside the registry (e.g. directly in the
    // simulation loop) -- silently skip them rather than emitting a warning.
    const HANDLED_OUTSIDE_REGISTRY: &[&str] = &["Occupancy"];

    let mut setpoints_reconciled_by_equipment: HashMap<
        String,
        Option<Vec<SetpointReconciliation>>,
    > = HashMap::new();

    let registry = EquipmentRegistry::new();
    let mut equipment: Vec<Box<dyn Equipment>> = Vec::new();
    let mut equipment_by_rng_stream: HashMap<u64, (String, &str)> = HashMap::new();
    for (spec, rng_identity) in equipment_specs.iter().zip(&rng_identities) {
        if HANDLED_OUTSIDE_REGISTRY.contains(&spec.name.as_str()) {
            continue;
        }
        // One config generation per equipment: the merged config (the
        // spec's typed payload with the dwelling-level overrides applied
        // and validated against the typed struct; a blueprint-added spec's
        // own parameters landed in its payload at add time) is what the
        // constructor and `init` both read, so every constructor-time
        // decision sees the same config the step's flows follow.
        let merged_cfg = merged_equipment_config(spec, &override_root)?;
        let mut eq = create_equipment_from_config(&registry, merged_cfg)?;

        // Entrance-1 identity validation runs before `init` so an unassigned
        // id (a malformed explicit `equipment_id` the constructor stamped as
        // the sentinel, an explicit 0, or an injection write that did not
        // land) fails the build as its own error naming the defect on every
        // config channel, not as an init error attributed to whatever the
        // equipment's config otherwise rejected.
        if eq.descriptor().id.0 == 0 {
            return Err(unassigned_equipment_id_rejection(
                spec,
                &eq.descriptor().name,
            ));
        }

        let mut merged_cfg = merged_equipment_config(spec, &override_root)?;
        setpoints_reconciled_by_equipment.insert(
            merged_cfg.name.clone(),
            merged_cfg.setpoints_reconciled.clone(),
        );
        merged_cfg.zone_map = Some(zone_map.clone());
        if eq.uses_rng_stream() {
            let rng_stream = RngStream::event_load(rng.get_seed(), rng_identity);
            if let Some((other, other_identity)) = equipment_by_rng_stream
                .insert(rng_stream.stream, (merged_cfg.name.clone(), rng_identity))
            {
                return Err(HaresError::Dwelling(format!(
                    "event loads '{other}' (identity '{other_identity}') and '{}' \
                     (identity '{rng_identity}') map to the same random stream; give \
                     each a unique HPXML SystemIdentifier id or instance name",
                    merged_cfg.name
                )));
            }
            merged_cfg.rng_stream = Some(rng_stream);
        }
        // The served zone as HVAC resolves it: the config's zone_id, else
        // the conditioned zone. Only HVAC's equivalent battery model reads
        // the capacitance.
        if let Some(zone_id) = merged_cfg
            .zone_id()
            .or_else(|| zone_map.get(ZoneRole::Indoor))
            && let Some(&cap) = zone_cap_kwh_per_k.get(&zone_id)
        {
            merged_cfg.zone_capacitance_kwh_per_k = cap;
        }
        let init_result = eq.init(&merged_cfg, &initial_env);
        // Drain the equipment's init warnings whether init succeeded or not:
        // the warnings were raised before the failure and belong in the log.
        eq.drain_warnings(&mut init_warning_scratch);
        for warning in init_warning_scratch.drain(..) {
            warnings.push_warning(warning);
        }
        // An init failure stops construction for every equipment, HVAC or
        // not: a load the input declares must join the model or the build
        // must fail naming the equipment and the cause. Skipping a
        // non-critical failure would run the dwelling short a declared
        // load while reporting success.
        if let Err(err) = init_result {
            return Err(HaresError::Equipment(format!(
                "equipment '{}' init failed: {err}",
                merged_cfg.name
            )));
        }
        equipment.push(eq);
    }
    let mut equipment_id_by_name = HashMap::with_capacity(equipment.len());
    // Entrance-1 identity validation, beside the name check it mirrors: no
    // two equipment share an id, and the unassigned sentinel 0 never
    // survives into the vector. The constructor's stamp is already checked
    // before `init` in the loop above; this check guards the init
    // re-assignment sites, which take their id from the same config channel
    // and must never write the sentinel back — checked at the door, not
    // assumed, because the config channel is written by a pre-pass whose
    // landing is one config write away from being wrong.
    let mut id_first_owner: HashMap<EquipmentId, String> = HashMap::with_capacity(equipment.len());
    for eq in &equipment {
        let desc = eq.descriptor();
        if equipment_id_by_name
            .insert(desc.name.clone(), desc.id)
            .is_some()
        {
            return Err(HaresError::Equipment(format!(
                "duplicate equipment name '{}' is not allowed",
                desc.name
            )));
        }
        if desc.id.0 == 0 {
            // The pre-init check above already rejected every id-0 entrance
            // cause it can diagnose from the spec, so reaching this net means
            // an init re-assignment site wrote the sentinel despite the
            // channel delivering a valid id — an internal invariant break,
            // reported as such rather than as a config problem.
            return Err(HaresError::Equipment(format!(
                "equipment '{}' has an unassigned equipment id (0) after init: \
                 an init re-assignment site wrote the sentinel — equipment ids \
                 are assigned by the dwelling assembly and are not configurable",
                desc.name
            )));
        }
        if let Some(first_owner) = id_first_owner.insert(desc.id, desc.name.clone()) {
            return Err(HaresError::Equipment(format!(
                "duplicate equipment id {:?}: equipment '{}' and '{}' share it \
                 — every equipment in a dwelling must be individually addressable",
                desc.id, first_owner, desc.name
            )));
        }
    }
    // The never-reused id counter starts past every id that entered through
    // assembly, so post-construction `add_equipment` auto-assignment can
    // never hand out an id already in the vector.
    let next_equipment_id = equipment
        .iter()
        .map(|eq| eq.descriptor().id.0)
        .max()
        .map_or(1, |max| max.saturating_add(1));

    let mut declarations: Vec<PortDeclaration> = Vec::new();
    for eq in &equipment {
        declarations.extend_from_slice(eq.ports());
    }

    let env_zone_ids: HashSet<ZoneId> = initial_env.zones.iter().map(|z| z.id).collect();
    validate_equipment_zones(&declarations, &env_zone_ids)?;
    environment.set_ambient_locations(equipment.iter().filter_map(|eq| eq.ambient_location()))?;

    let mut allocated_loop_ids = loop_allocator::collect_allocated_loop_ids(&equipment_specs)?;
    // Sentinel loop IDs always valid — well-known addresses used by
    // equipment as sentinels when no typed config is available (0) or
    // for the shared DHW demand loop (u16::MAX - 1).
    allocated_loop_ids.insert(0); // LoopId::default() — fallback for raw-config equipment
    allocated_loop_ids.insert(hares_equipment::DHW_DEMAND_LOOP.0);
    validate_equipment_loops(&declarations, &allocated_loop_ids)?;

    let output_path = config
        .sim_config
        .output_path
        .clone()
        .unwrap_or_else(|| default_output_path(&config));

    let (roof_info, wall_azimuths) = hares_io::pv_sizing::extract_roof_info(&bp.building);
    let latitude_deg = bp.building.site.latitude_deg;
    let facility_type = bp.building.residential_facility_type.clone();
    let solver_feedback_actor = SolverFeedbackActor::new();

    let init_humidity_ratios: Vec<(ZoneId, f64)> = solvers
        .humidity
        .humidity_ratios
        .iter()
        .map(|(&z, &w)| (z, w))
        .collect();

    let mut dwelling = Dwelling {
        bldg_id: config.bldg_id,
        failed: false,
        restored_from_checkpoint: false,
        #[cfg(debug_assertions)]
        test_panic_on_step: false,
        #[cfg(debug_assertions)]
        test_assert_panic_on_step: false,
        #[cfg(any(test, debug_assertions))]
        test_thermal_invariant_failure: false,
        #[cfg(any(test, debug_assertions))]
        test_hvac_negative_energy_failure: false,
        equipment,
        equipment_id_by_name,
        next_equipment_id,
        thermal_solver: solvers.thermal,
        humidity_solver: solvers.humidity,
        electrical_solver: solvers.electrical,
        // The port tables, the fluid loop-type map, the recorder and the
        // roster caches are derived by the roster plan installed right
        // after construction.
        fluid_solver: FluidSolver::new(FluidSolverConfig::default(), &[])?,
        clock: clock.clone(),
        environment,
        ports: PortSlots::default(),
        rollback_ports: PortSlots::default(),
        recorder: None,
        roster: RosterCaches::default(),
        rng,
        warnings: std::mem::take(warnings),
        warning_scratch: Vec::new(),
        setpoints_reconciled_by_equipment,
        roof_info,
        wall_azimuths,
        latitude_deg,
        facility_type,
        pv_panel_defaults: defaults.take_pv_panel_map(),
        control_dispatcher: ControlDispatcher::default(),
        price_signal: PriceSignal::default(),
        tariff_evaluator: None,
        billing_summaries: Vec::new(),
        prior_electrical_summary: ElectricalSummary::default(),
        latest_env: initial_env,
        simulation_results: SimulationResults::default(),
        custom_domain_solvers: Vec::new(),
        thermal_update_buf: hares_types::DomainUpdate::empty(hares_types::THERMAL),
        humidity_update_buf: hares_types::DomainUpdate::empty(hares_types::HUMIDITY),
        electrical_update_buf: hares_types::DomainUpdate::empty(hares_types::ELECTRICAL),
        fluid_update_buf: hares_types::DomainUpdate::empty(hares_types::FLUID),
        custom_update_bufs: Vec::new(),
        timestamp_buf: String::with_capacity(32),
        occupancy_column_idx,
        occupancy_scale,
        prev_humidity_ratios: init_humidity_ratios,
        health: RunHealth::default(),
        actors: Vec::new(),
        scheduler: StepScheduler::default(),
        auto_registered_actor_names: HashSet::new(),
        next_ev_driver_stream: 0,
        actor_dispatch_buf: Vec::with_capacity(16),
        solver_feedback_actor,
        prev_zone_temps: HashMap::new(),
        prior_zone_temps: HashMap::new(),
        prev_price_signal: PriceSignal::default(),
        prev_equipment_modes: HashMap::new(),
        invariant_checker: InvariantChecker::new(),
        invariant_conditioned_temps: Vec::with_capacity(bp.building.zones.len()),
        invariant_unconditioned_temps: Vec::with_capacity(bp.building.zones.len()),
        invariant_tank_temps: Vec::new(),
        tank_node_keys: (0..24).map(tk::tank_node_key).collect(),
        invariant_infiltration_latent: Vec::new(),
        invariant_infiltration_m_dot: HashMap::new(),
        invariant_infiltration_w_outdoor: HashMap::new(),
        is_warming_up: false,
        step_hvac_heating_w: 0.0,
        step_hvac_cooling_w: 0.0,
        #[cfg(all(feature = "observe", debug_assertions))]
        invariant_moisture_capture: Vec::new(),
        zone_is_conditioned: if bp.building.zones.is_empty() {
            vec![true]
        } else {
            bp.building
                .zones
                .iter()
                .map(|z| z.zone_type == hares_io::hpxml::ZoneType::Conditioned)
                .collect()
        },
        island_unserved_kw: 0.0,
        island_excess_kw: 0.0,
        output_verbosity: config.sim_config.output_verbosity,
        output_chunk_size: config.sim_config.output_chunk_size,
        output_format: config.sim_config.output_format,
        output_path: output_path.clone(),
        write_output: config.sim_config.write_output,
        retain_batches: config.sim_config.retain_batches,
        output_rotation: config.sim_config.rotation,
        sim_config: config.sim_config.clone(),
        diagnostic_writer: None,
        #[cfg(feature = "profiling")]
        profiling: DwellingProfilingSummary::default(),
        #[cfg(feature = "profiling")]
        actor_timings: ActorTimings::default(),
        #[cfg(feature = "observe")]
        observer_buf: None,
        #[cfg(feature = "observe")]
        diagnostic_accum: None,
        #[cfg(any(debug_assertions, feature = "observe_detailed"))]
        envelope_diagnostics: solvers.envelope_diagnostics,
    };

    // Registering the built-in actors is the one roster plan of assembly:
    // it derives the port tables, the fluid loop-type map, the output
    // schema with its recorder and every roster cache, the same way every
    // later roster change does.
    if let Err(err) = dwelling.auto_register_actors() {
        // The dwelling is about to be dropped carrying the accumulated
        // warning log; emit it so this failure carries the context that
        // led here.
        for entry in dwelling.take_warnings() {
            tracing::warn!("{entry}");
        }
        return Err(err);
    }

    if let Some(_init_dur) = config.initialization_duration {
        // Save RNG state before warmup. The clock is reset after warmup for
        // weather replay; restoring the RNG ensures the production phase
        // starts from the same RNG position regardless of how many warmup
        // iterations were needed. Two runs with the same initial seed
        // produce identical stochastic output even when warmup converges
        // in a different number of iterations.
        let rng_seed_before = dwelling.rng.get_seed();
        let rng_stream_before = dwelling.rng.get_stream();
        let rng_word_pos_before = dwelling.rng.get_word_pos();

        match dwelling.run_warmup_converged(0.5, 25) {
            Err(err) => {
                // The dwelling is about to be dropped carrying the
                // accumulated warning log; emit it so this failure carries
                // the context that led here.
                for entry in dwelling.take_warnings() {
                    tracing::warn!("{entry}");
                }
                return Err(err);
            }
            #[cfg(feature = "observe")]
            Ok(iterations) => tracing::debug!(
                warmup_iterations = iterations,
                rng_word_pos_delta =
                    (dwelling.rng.get_word_pos() as i128) - (rng_word_pos_before as i128),
                "warmup complete; restoring the RNG for production-phase reproducibility"
            ),
            #[cfg(not(feature = "observe"))]
            Ok(_) => {}
        }

        // Restore RNG state to pre-warmup position.
        let mut restored_rng = ChaCha8Rng::from_seed(rng_seed_before);
        restored_rng.set_stream(rng_stream_before);
        restored_rng.set_word_pos(rng_word_pos_before);
        dwelling.rng = restored_rng;

        clock = SimClock::new(
            local_start,
            config.sim_config.time_res,
            config.sim_config.duration,
        );
        #[cfg(feature = "dst")]
        {
            clock.civil_tz = bp.parsed_civil_tz;
        }
        dwelling.clock = clock;
    } else {
        tracing::warn!(
            bldg_id = config.bldg_id,
            "initialization_duration is None — no warm-up period will run; \
             this produces biased heat-transfer predictions for heavyweight \
             construction. Set initialization_duration to at least 7 days \
             (604800 s) for concrete/masonry buildings."
        );
    }

    // Initialise diagnostic CSV when output_verbosity >= 4.
    dwelling.diagnostic_writer = if dwelling.output_verbosity >= 4 {
        let stem = output_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("dwelling_{}", dwelling.bldg_id));
        let diag_path = output_path.with_file_name(format!("{stem}_diagnostics.csv"));
        tracing::info!(
            path = %diag_path.display(),
            "diagnostic CSV output enabled (output_verbosity >= 4)"
        );
        let file = match std::fs::File::create(&diag_path) {
            Ok(file) => file,
            Err(err) => {
                // The dwelling is about to be dropped carrying the
                // accumulated warning log; emit it so this failure carries
                // the context that led here.
                for entry in dwelling.take_warnings() {
                    tracing::warn!("{entry}");
                }
                return Err(HaresError::Io(format!(
                    "diagnostic file create failed: {err}"
                )));
            }
        };
        let mut writer = std::io::BufWriter::new(file);
        let n_zones = dwelling.latest_env.zones.len();
        diagnostics::write_header(&mut writer, n_zones);
        // Emit equipment zone-id mapping so zone routing is traceable.
        let equipment_zones: Vec<(String, u16, Option<String>)> = dwelling
            .equipment
            .iter()
            .filter_map(|eq| {
                eq.descriptor().zone.map(|z| {
                    (
                        eq.descriptor().name.clone(),
                        z.0,
                        eq.descriptor().zone_type.clone(),
                    )
                })
            })
            .collect();
        let ocv_sources: Vec<(String, String)> = dwelling
            .equipment
            .iter()
            .filter_map(|eq| {
                eq.ocv_source()
                    .map(|s| (eq.descriptor().name.clone(), s.to_string()))
            })
            .collect();
        let provenance_lines: Vec<String> = dwelling
            .equipment
            .iter()
            .flat_map(|eq| eq.provenance_lines())
            .collect();
        diagnostics::write_equipment_init(
            &mut writer,
            &equipment_zones,
            &ocv_sources,
            &provenance_lines,
        );
        Some(writer)
    } else {
        None
    };

    Ok(dwelling)
}

impl Dwelling {
    /// Runs the full configured horizon and returns accumulated results.
    pub fn simulate(&mut self) -> Result<SimulationResults> {
        while self.clock.current_step() < self.clock.total_steps() {
            self.run_timestep(self.write_output)?;
        }
        self.finalize_billing();

        // --- Post-hoc diagnostic checks on accumulated observer data ---
        #[cfg(feature = "observe")]
        {
            let violations = self.run_diagnostic_checks();
            if violations > 0 {
                tracing::warn!(
                    violation_count = violations,
                    "post-hoc diagnostic checks found {violations} violation(s)"
                );
            }
        }

        if let Some(recorder) = self.recorder.as_mut() {
            recorder
                .flush_and_close()
                .map_err(|err| HaresError::Io(format!("output close failed: {err}")))?;
        }
        if let Some(ref mut writer) = self.diagnostic_writer {
            std::io::Write::flush(writer)
                .map_err(|err| HaresError::Io(format!("diagnostic close failed: {err}")))?;
        }
        Ok(self.simulation_results.clone())
    }

    /// Runs post-hoc diagnostic checks on accumulated observer data and returns
    /// the number of violations found.
    #[cfg(feature = "observe")]
    fn run_diagnostic_checks(&mut self) -> usize {
        let Some(accum) = &self.diagnostic_accum else {
            return 0;
        };
        let zone_names: Vec<String> = self
            .latest_env
            .zones
            .iter()
            .map(|z| {
                if z.id == ZoneId(1) {
                    "Indoor".to_string()
                } else {
                    format!("Zone{}", z.id.0)
                }
            })
            .collect();
        let time_res_s = self.latest_env.time_step_secs();
        accum.run_post_hoc_checks(&mut self.diagnostic_writer, &zone_names, time_res_s)
    }

    /// Emit the final partial billing period (if any). Call after the last
    /// `step()` when driving the simulation step-by-step. Idempotent --
    /// a second call has no effect.
    pub fn finalize_billing(&mut self) {
        if let Some(ref mut evaluator) = self.tariff_evaluator {
            let tz = evaluator.simulation_start().timezone();
            let sim_end = self.clock.current_time().with_timezone(&tz);
            if let Some(summary) = evaluator.finalize(sim_end) {
                self.billing_summaries.push(summary);
            }
        }
    }

    /// Returns accumulated results gathered so far.
    #[must_use]
    pub fn results(&self) -> SimulationResults {
        self.simulation_results.clone()
    }

    /// Test-only hook: causes the next [`step()`](Self::step) call to panic.
    /// Only available in debug_assertions builds (tests). Has no effect in release.
    #[cfg(debug_assertions)]
    pub fn set_test_panic(&mut self) {
        self.test_panic_on_step = true;
    }

    /// Test-only hook: causes the next [`step()`](Self::step) call to panic
    /// via `assert!` failure for integration testing of assert-based panics.
    #[cfg(debug_assertions)]
    pub fn set_test_assert_panic(&mut self) {
        self.test_assert_panic_on_step = true;
    }

    /// Test-only hook: causes the thermal invariant check in the next
    /// [`check_step_invariants`](Self::check_step_invariants) call to receive deliberately
    /// broken balance terms, forcing `InvariantViolation { check_name: "thermal_balance" }`.
    ///
    /// Available in test builds (including plain-release test runs). Has no
    /// effect on the production simulation: the real balance terms computed
    /// by the solver are preserved.
    #[cfg(any(test, debug_assertions))]
    pub fn set_thermal_invariant_failure_for_test(&mut self) {
        self.test_thermal_invariant_failure = true;
    }

    /// Test-only: causes the next HVAC delivered-energy invariant checks to
    /// receive deliberately negative values, forcing `NegativeDeliveredEnergy`.
    /// The flag is reset to `false` after one check so the effect is scoped.
    #[cfg(any(test, debug_assertions))]
    pub fn set_hvac_negative_energy_failure_for_test(&mut self) {
        self.test_hvac_negative_energy_failure = true;
    }

    /// Executes exactly one simulation timestep.
    pub fn step(&mut self) -> Result<StepResult> {
        #[cfg(debug_assertions)]
        if self.test_panic_on_step {
            panic!("test-induced panic in Dwelling::step()");
        }
        #[cfg(debug_assertions)]
        assert!(
            !self.test_assert_panic_on_step,
            "test-induced assert failure in Dwelling::step()"
        );
        self.run_timestep(self.write_output)?;
        Ok(self.simulation_results.steps.last().unwrap().clone())
    }

    /// Queues a control signal for one equipment instance by name.
    ///
    /// Priority is derived from the signal type via the central
    /// `From<&ControlSignal> for PriorityTier` mapping.
    pub fn apply_control(&mut self, name: &str, signal: ControlSignal) {
        let priority = PriorityTier::from(&signal);
        self.control_dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from(name)),
            signal,
            priority,
        });
    }

    /// Validates a control signal against the named equipment's current state
    /// and queues it on success.
    ///
    /// Signals that only update connection/mode state (e.g. `EvPlugIn`) are
    /// applied eagerly so that subsequent validated signals in the same
    /// timestep see the updated state. They remain queued so that the
    /// dispatcher's telemetry capture still fires at step time; re-applying an
    /// idempotent state-assignment is safe.
    ///
    /// When `priority` is `None`, the tier is derived from the signal type via
    /// the centralised `From<&ControlSignal> for PriorityTier` mapping.
    /// When `Some(tier)`, the explicit tier is used instead — this allows
    /// external callers (e.g. Python bridges) to elevate a signal's priority
    /// (e.g. a freeze-protection `ThermalSetpoint` at `Safety`).
    ///
    /// Returns `Err` if the equipment is not found or the signal is rejected
    /// by the equipment's current state (e.g. EvDrive while plugged in).
    pub fn apply_control_validated(
        &mut self,
        name: &str,
        signal: ControlSignal,
        priority: Option<PriorityTier>,
    ) -> Result<()> {
        let is_immediate = signal.is_immediate_state_update();
        if is_immediate {
            let eq = self
                .equipment
                .iter_mut()
                .find(|e| e.descriptor().name == name)
                .ok_or_else(|| HaresError::Equipment(format!("equipment '{name}' not found")))?;
            eq.apply_control(&signal)?;
        } else {
            let eq = self
                .equipment
                .iter()
                .find(|e| e.descriptor().name == name)
                .ok_or_else(|| HaresError::Equipment(format!("equipment '{name}' not found")))?;
            eq.validate_signal(&signal)?;
        }
        let priority = priority.unwrap_or_else(|| PriorityTier::from(&signal));
        if matches!(priority, PriorityTier::Safety | PriorityTier::Grid) {
            tracing::warn!(
                equipment = name,
                priority_tier = ?priority,
                signal = ?signal,
                "Control signal with elevated priority received via non-actor path; \
                 this may indicate a manual override that should be reviewed",
            );
        }
        self.control_dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from(name)),
            signal,
            priority,
        });
        Ok(())
    }

    /// Queues a control signal by end-use category.
    ///
    /// Priority is derived from the signal type via the central
    /// `From<&ControlSignal> for PriorityTier` mapping.
    pub fn queue_end_use_control(&mut self, end_use: EndUse, signal: ControlSignal) {
        let priority = PriorityTier::from(&signal);
        self.control_dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByEndUse(end_use),
            signal,
            priority,
        });
    }

    /// Queues a typed dispatch request.
    pub fn queue_dispatch(&mut self, request: DispatchRequest) {
        self.control_dispatcher.queue(request);
    }

    /// Returns the number of registered actors.
    #[must_use]
    pub fn actor_count(&self) -> usize {
        self.actors.len()
    }
}

/// Typed payload for `set_battery_lut` -- ensures the data matches the LUT type at compile time.
pub enum BatteryLutData {
    ChargingCurve(Box<RegularGridInterpolator>),
    Ocv(OcvTable),
    UNeg(UNegTable),
}

impl Dwelling {
    /// Returns a slice of equipment for read-only access.
    #[must_use]
    pub fn equipment(&self) -> &[Box<dyn Equipment>] {
        &self.equipment
    }

    /// The dwelling's aggregate real-power voltage response, without
    /// simulation. See [`PremiseZip`] for the full contract.
    ///
    /// The aggregate covers the equipment whose real-power ZIP genuinely
    /// governs their real power (scheduled and event loads), weighted by
    /// each one's expected mean draw over the loaded schedule data.
    /// Rule-R1 physics equipment and DER are listed in
    /// [`PremiseZip::constant_power`] instead: their real power is
    /// voltage-invariant by construction, but their *share* of total draw
    /// is knowable only by simulation, so a whole-premise %P/%V combines
    /// this aggregate with those shares.
    ///
    /// `None` when no ZIP-governed equipment has a computable positive
    /// expected draw — never a silent constant-power answer.
    #[must_use]
    pub fn premise_zip(&self) -> Option<PremiseZip> {
        premise_zip::aggregate_premise_zip(self.equipment(), self.environment.schedule())
    }

    /// Returns per-equipment HPXML setpoint reconciliation records, keyed by
    /// instance name.  `None` entries mean the equipment had no setpoint
    /// reconciliation (no hours were widened).
    #[must_use]
    pub fn setpoints_reconciled(&self) -> &HashMap<String, Option<Vec<SetpointReconciliation>>> {
        &self.setpoints_reconciled_by_equipment
    }

    /// Set a LUT on a battery equipment by name.
    ///
    /// For `BatteryLutType::ChargingCurve`, pass a pre-built `RegularGridInterpolator`.
    /// For `Ocv` / `UNeg`, pass validated tables.
    /// Returns `Err` if the equipment doesn't exist or doesn't support the LUT type.
    pub fn set_battery_lut(
        &mut self,
        name: &str,
        lut_type: BatteryLutType,
        lut: BatteryLutData,
    ) -> Result<()> {
        let eq = self
            .equipment
            .iter_mut()
            .find(|e| e.descriptor().name == name)
            .ok_or_else(|| HaresError::Equipment(format!("equipment '{}' not found", name)))?;

        match (lut_type, lut) {
            (BatteryLutType::ChargingCurve, BatteryLutData::ChargingCurve(interp)) => {
                eq.unmark_initialized();
                let result = eq.set_charging_curve_lut(Some(*interp));
                eq.mark_initialized();
                result
            }
            (BatteryLutType::Ocv, BatteryLutData::Ocv(table)) => {
                eq.unmark_initialized();
                let result = eq.set_ocv_table(table);
                eq.mark_initialized();
                result
            }
            (BatteryLutType::UNeg, BatteryLutData::UNeg(table)) => {
                eq.unmark_initialized();
                let result = eq.set_u_neg_table(table);
                eq.mark_initialized();
                result
            }
            _ => Err(HaresError::Equipment(format!(
                "mismatched lut_type and data for equipment '{}'",
                name
            ))),
        }
    }

    /// Clear / reset a LUT on a battery equipment by name.
    pub fn clear_battery_lut(&mut self, name: &str, lut_type: BatteryLutType) -> Result<()> {
        let eq = self
            .equipment
            .iter_mut()
            .find(|e| e.descriptor().name == name)
            .ok_or_else(|| HaresError::Equipment(format!("equipment '{}' not found", name)))?;

        match lut_type {
            BatteryLutType::ChargingCurve => {
                eq.unmark_initialized();
                let result = eq.set_charging_curve_lut(None);
                eq.mark_initialized();
                result
            }
            BatteryLutType::Ocv => eq.reset_ocv_table(),
            BatteryLutType::UNeg => eq.reset_u_neg_table(),
        }
    }

    /// Set a 4D charging curve LUT on an EV equipment by name.
    pub fn set_ev_charging_curve_lut(
        &mut self,
        name: &str,
        lut: RegularGridInterpolator,
    ) -> Result<()> {
        let eq = self
            .equipment
            .iter_mut()
            .find(|e| e.descriptor().name == name)
            .ok_or_else(|| HaresError::Equipment(format!("equipment '{}' not found", name)))?;

        eq.unmark_initialized();
        let result = eq.set_charging_curve_lut(Some(lut));
        eq.mark_initialized();
        result
    }

    /// Clear the charging curve LUT on an EV equipment by name.
    pub fn clear_ev_charging_curve_lut(&mut self, name: &str) -> Result<()> {
        let eq = self
            .equipment
            .iter_mut()
            .find(|e| e.descriptor().name == name)
            .ok_or_else(|| HaresError::Equipment(format!("equipment '{}' not found", name)))?;

        eq.unmark_initialized();
        let result = eq.set_charging_curve_lut(None);
        eq.mark_initialized();
        result
    }

    /// Returns the current environment state (zone temps, weather, grid, time).
    ///
    /// Useful for initializing equipment with realistic state before the first step.
    #[must_use]
    pub fn latest_env(&self) -> &EnvironmentState {
        &self.latest_env
    }

    /// The dwelling's thermal (envelope) solver.
    #[must_use]
    pub fn thermal_solver(&self) -> &ThermalSolver {
        &self.thermal_solver
    }

    /// Load [kW] the island sources failed to cover during the last islanded
    /// step (would-be phantom grid import). 0.0 when not islanded.
    #[must_use]
    pub fn island_unserved_kw(&self) -> f64 {
        self.island_unserved_kw
    }

    /// Surplus on-site generation [kW] the island could not absorb during the
    /// last islanded step (would-be phantom grid export). 0.0 when not
    /// islanded.
    #[must_use]
    pub fn island_excess_kw(&self) -> f64 {
        self.island_excess_kw
    }

    #[cfg(any(debug_assertions, feature = "observe_detailed"))]
    pub fn envelope_diagnostics(&self) -> &EnvelopeDiagnostics {
        &self.envelope_diagnostics
    }

    #[cfg(any(debug_assertions, feature = "observe_detailed"))]
    pub fn envelope_diagnostics_json(&self) -> Result<String> {
        serde_json::to_string_pretty(&self.envelope_diagnostics)
            .map_err(|e| HaresError::Io(format!("EnvelopeDiagnostics serialization: {e}")))
    }

    /// Stores the active price signal for equipment controllers.
    pub fn set_price_signal(&mut self, signal: PriceSignal) {
        self.price_signal = signal;
    }

    /// Returns the current price signal.
    #[must_use]
    pub fn price_signal(&self) -> &PriceSignal {
        &self.price_signal
    }

    /// Returns accumulated billing period summaries.
    #[must_use]
    pub fn billing_summaries(&self) -> &[BillingPeriodSummary] {
        &self.billing_summaries
    }

    /// Returns a reference to the tariff evaluator, if one is set.
    #[must_use]
    pub fn tariff_evaluator(&self) -> Option<&TariffEvaluator> {
        self.tariff_evaluator.as_ref()
    }

    /// Applies a utility grid voltage override through the environment
    /// manager. `voltage_pu == 0.0` signals a utility outage; whether the
    /// home bus stays energized is resolved per step from island-capable
    /// sources (see `GridState::bus_energized`).
    pub fn set_grid_voltage(&mut self, voltage_pu: f64) {
        self.environment.set_grid_override(GridState {
            voltage_pu,
            frequency_hz: DEFAULT_GRID_FREQUENCY_HZ,
            island_bus_voltage_pu: None,
        });
    }

    /// Returns current observable state.
    ///
    /// # Errors
    ///
    /// `HaresError::InvariantViolation` when the outdoor humidity ratio is
    /// negative, `HaresError::NanDetected` when any value feeding the
    /// snapshot (outdoor temperature, zone temperatures, equipment power,
    /// energy-balance residuals) is non-finite in every build profile.
    pub fn telemetry(&self) -> Result<DwellingTelemetry> {
        let mut zone_ids: Vec<ZoneId> = self.latest_env.zones.iter().map(|z| z.id).collect();
        zone_ids.sort_unstable();
        let zone_names: Vec<String> = zone_ids
            .iter()
            .enumerate()
            .map(|(idx, zone)| {
                if idx == 0 {
                    "Indoor".to_string()
                } else {
                    format!("Zone{}", zone.0)
                }
            })
            .collect();
        let zone_temperatures_c: Vec<f64> = zone_ids
            .iter()
            .filter_map(|zone| {
                self.latest_env
                    .zones
                    .iter()
                    .find(|z| z.id == *zone)
                    .map(|z| z.temperature_c)
            })
            .collect();

        // Per-zone energy balance residual from the thermal solver's
        // custom_payload.  The payload carries 5 floats per zone:
        // [zone_id, q_latent_w, m_dot_inf_kg_s, w_outdoor, residual_w].
        let mut energy_balance_residuals = vec![0.0; zone_ids.len()];
        if let Some(thermal_update) = self
            .latest_env
            .custom_domains
            .iter()
            .find(|u| u.domain_id == hares_types::THERMAL)
            && let Some(payload) = &thermal_update.custom_payload
        {
            let residual_map: HashMap<ZoneId, f64> = payload
                .chunks_exact(5)
                .filter_map(|quint| {
                    let zone_raw = quint[0];
                    if zone_raw.is_finite() && zone_raw >= 0.0 {
                        Some((ZoneId(zone_raw as u16), quint[4]))
                    } else {
                        None
                    }
                })
                .collect();
            for (i, zone) in zone_ids.iter().enumerate() {
                energy_balance_residuals[i] = residual_map.get(zone).copied().unwrap_or(0.0);
            }
        }

        let mut setpoint_heat_c = zone_temperatures_c.clone();
        let mut setpoint_cool_c = zone_temperatures_c.clone();
        let mut equipment_names = Vec::with_capacity(self.equipment.len());
        let mut equipment_modes = Vec::with_capacity(self.equipment.len());
        let mut equipment_soc = Vec::with_capacity(self.equipment.len());
        let mut equipment_power_kw = Vec::with_capacity(self.equipment.len());

        for eq in &self.equipment {
            let co = eq.core_output();
            equipment_names.push(eq.descriptor().name.clone());
            equipment_modes.push(co.state.operating_mode.map_or(0.0, |m| m.as_code()));
            equipment_soc.push(co.state.soc.map_or(0.0, |s| s.get()));
            equipment_power_kw.push(co.flows.electric_kw.map_or(0.0, |e| e.net_consumption_kw()));

            if let Some(zone_id) = eq.descriptor().zone
                && let Some(zone_idx) = zone_ids.iter().position(|z| *z == zone_id)
                && let Some(sp) = co.state.setpoint_c
            {
                match co.state.operating_mode {
                    // Standard heating mode and all heat-pump-specific heating modes.
                    Some(
                        hares_types::OperatingMode::Heating
                        | hares_types::OperatingMode::HeatingHP
                        | hares_types::OperatingMode::HeatingER
                        | hares_types::OperatingMode::HeatingHPAndER,
                    ) => setpoint_heat_c[zone_idx] = sp,
                    Some(hares_types::OperatingMode::Cooling) => setpoint_cool_c[zone_idx] = sp,
                    _ => {}
                }
            }
        }

        // BTreeMaps keep actor and channel iteration deterministic; equipment
        // `Telemetry` maps are HashMaps, so channels are re-collected rather
        // than cloned.
        let mut actor_telemetry: std::collections::BTreeMap<
            String,
            std::collections::BTreeMap<String, f64>,
        > = std::collections::BTreeMap::new();
        let collect_channels = |tel: &hares_types::Telemetry| {
            tel.0
                .iter()
                .map(|(k, &v)| (k.clone(), v))
                .collect::<std::collections::BTreeMap<String, f64>>()
        };

        // Collect solver feedback actor telemetry (always None, but included for completeness).
        if let Some(tel) = self.solver_feedback_actor.telemetry() {
            actor_telemetry.insert(
                self.solver_feedback_actor.name().to_string(),
                collect_channels(tel),
            );
        }

        for actor in &self.actors {
            if let Some(tel) = actor.telemetry() {
                actor_telemetry.insert(actor.name().to_string(), collect_channels(tel));
            }
        }

        #[cfg(feature = "observe")]
        tracing::debug!(
            step = self.clock.current_step(),
            telemetry_keys = ?actor_telemetry
                .iter()
                .map(|(name, channels)| {
                    (name.clone(), channels.keys().cloned().collect::<Vec<_>>())
                })
                .collect::<Vec<_>>(),
            "actor_telemetry keys at timestep"
        );

        // Snapshot-construction checks: typed errors in every build profile.
        // Non-finite values reach downstream consumers (RL agents,
        // monitor scripts, fleet controller) and cause silent misbehaviour
        // that is far more expensive to diagnose than the check that catches
        // it here.
        let step = self.clock.current_step();
        {
            let w = self.latest_env.weather.outdoor_humidity_ratio;
            if !w.is_finite() || w < 0.0 {
                return Err(HaresError::InvariantViolation {
                    check_name: "outdoor_humidity_ratio_non_negative".to_string(),
                    value: w,
                    tolerance: 0.0,
                });
            }
            // NaN screening on every float field before the telemetry snapshot
            // is emitted.
            self.invariant_checker.check_nan_screen(
                step,
                &[(
                    "outdoor_temp_c",
                    None,
                    self.latest_env.weather.outdoor_temp_c,
                )],
            )?;
            for (i, &t) in zone_temperatures_c.iter().enumerate() {
                if !t.is_finite() {
                    let zid = zone_ids.get(i).copied();
                    return Err(HaresError::NanDetected {
                        step_index: step,
                        zone_id: zid,
                        value_name: "zone_temperature_c".to_string(),
                    });
                }
            }
            for (i, &p) in equipment_power_kw.iter().enumerate() {
                if !p.is_finite() {
                    let name = equipment_names.get(i).map(|s| s.as_str()).unwrap_or("?");
                    return Err(HaresError::NanDetected {
                        step_index: step,
                        zone_id: None,
                        value_name: format!("equipment_power_kw for '{name}'"),
                    });
                }
            }
            for (i, &p) in energy_balance_residuals.iter().enumerate() {
                if !p.is_finite() {
                    let zid = zone_ids.get(i).copied();
                    return Err(HaresError::NanDetected {
                        step_index: step,
                        zone_id: zid,
                        value_name: "energy_balance_residual".to_string(),
                    });
                }
            }
            if !self.electrical_solver.net_active_kw().is_finite() {
                return Err(HaresError::InvariantViolation {
                    check_name: "telemetry_total_power_kw_finite".to_string(),
                    value: self.electrical_solver.net_active_kw(),
                    tolerance: 0.0,
                });
            }
            if !self.electrical_solver.net_reactive_kvar().is_finite() {
                return Err(HaresError::InvariantViolation {
                    check_name: "telemetry_reactive_power_kvar_finite".to_string(),
                    value: self.electrical_solver.net_reactive_kvar(),
                    tolerance: 0.0,
                });
            }
        }

        let telem = DwellingTelemetry {
            timestep_index: self.clock.current_step(),
            current_time: self.latest_env.current_time,
            zone_names,
            zone_temperatures_c,
            equipment_names,
            equipment_modes,
            equipment_soc,
            equipment_power_kw,
            setpoint_heat_c,
            setpoint_cool_c,
            energy_balance_residuals,
            total_power_kw: self.electrical_solver.net_active_kw(),
            reactive_power_kvar: self.electrical_solver.net_reactive_kvar(),
            island_unserved_kw: self.island_unserved_kw,
            island_excess_kw: self.island_excess_kw,
            outdoor_temp_c: self.latest_env.weather.outdoor_temp_c,
            outdoor_humidity_ratio: self.latest_env.weather.outdoor_humidity_ratio,
            actor_telemetry,
            dwelling_failed: self.failed,
            initialized: self.clock.current_step() > 0,
        };

        Ok(telem)
    }

    /// Per-zone thermal consistency check: compares HVAC heating and cooling
    /// port-accumulated totals (sensible + radiant + latent) against the sum
    /// of equipment `thermal_output_w` for HVAC equipment assigned to each zone.
    ///
    /// Only `HvacHeating` and `HvacCooling` categories are compared. All
    /// other thermal categories (`InternalGain`, `JacketLoss`, `DuctLoss`,
    /// `HvacDehumidification`) are intentionally excluded: occupant gains,
    /// appliance waste heat, and non-HVAC equipment deposits do not
    /// correspond to any equipment's `thermal_output_w` and would produce
    /// false-positive mismatches on every timestep in realistic residential
    /// simulations.
    ///
    /// Only equipment with `end_use == HVAC_HEATING` or `end_use ==
    /// HVAC_COOLING` is included in the equipment-side sum.  Water heaters,
    /// scheduled loads, EV chargers, batteries, PV, and other non-HVAC
    /// equipment are excluded even when they have a zone assigned and set
    /// `thermal_output_w`, because they do not deposit via `HvacHeating` or
    /// `HvacCooling` ports.
    ///
    /// Runs in every build profile, over the zone and HVAC-equipment index
    /// lists resolved at assembly ([`Self::hvac_thermal_consistency`]): a
    /// mismatch is a physics violation (a typed error), not a flag.
    fn verify_per_zone_thermal_consistency(
        &self,
        step: u64,
        step_succeeded: &[bool],
    ) -> Result<()> {
        for (i, acc) in self.ports.thermal.iter().enumerate() {
            let port_total: f64 = [ThermalCategory::HvacHeating, ThermalCategory::HvacCooling]
                .into_iter()
                .map(|cat| {
                    acc.sensible_for_category(cat)
                        + acc.radiant_for_category(cat)
                        + acc.latent_for_category(cat)
                })
                .sum();
            let equip_total: f64 = self
                .roster
                .hvac_thermal_consistency
                .get(i)
                .map(|indices| {
                    indices
                        .iter()
                        .filter_map(|&ei| {
                            // A failed-and-rolled-back equipment booked no
                            // thermal power this step (its port
                            // contributions were removed); its core_output
                            // retains the last committed value, which must
                            // not be summed against the rolled-back ports.
                            if !step_succeeded.get(ei).copied().unwrap_or(false) {
                                return None;
                            }
                            self.equipment.get(ei)
                        })
                        .map(|eq| eq.core_output().flows.thermal_output_w.unwrap_or(0.0))
                        .sum()
                })
                .unwrap_or(0.0);
            let tolerance = 1.0_f64.max(1e-6 * port_total.abs());
            let diff = (port_total - equip_total).abs();
            if diff > tolerance {
                return Err(HaresError::InvalidState(format!(
                    "per-zone thermal consistency at step {step}: zone {:?} HVAC \
                     port total {port_total:.6} W does not match the equipment \
                     thermal_output_w sum {equip_total:.6} W (diff {diff:.6} W)",
                    acc.zone
                )));
            }
        }
        Ok(())
    }

    /// Drains and returns warning messages accumulated since the previous
    /// call. The buffer is bounded ([`WarningLog::CAPACITY`]); if messages
    /// were dropped since the last drain, the final entry reports how many.
    pub fn take_warnings(&mut self) -> Vec<String> {
        self.warnings.take()
    }

    /// Run-total health counters for this dwelling (port rollbacks, rejected
    /// control signals, clamped actions, curve-index clamps, warm-up
    /// outcome). Recorded unconditionally in every build profile; never
    /// reset per step.
    #[must_use]
    pub fn health(&self) -> &RunHealth {
        &self.health
    }

    /// Adds to the run-total clamped-actions counter.
    ///
    /// # Errors
    ///
    /// Always returns `Ok(())` today: the `Result` is the contract a future
    /// policy escalation will fill (a clamp threshold that escalates to a
    /// run failure).
    pub fn record_clamped_actions(&mut self, count: u64) -> Result<()> {
        self.health.clamped_actions += count;
        Ok(())
    }

    #[cfg(feature = "profiling")]
    #[must_use]
    pub fn profiling_summary(&self) -> DwellingProfilingSummary {
        let mut summary = self.profiling.clone();
        // The one field not accumulated per step: the peak resident size is
        // monotone, so a single read at summary time adds nothing to per-step
        // reads and the per-step work pays for no /proc parsing.
        summary.memory_high_water_kb = current_process_hwm_kb();
        // The run's per-actor totals, read from the run's store at summary
        // time like the high-water mark: the hot path never clones a name.
        summary.per_actor = self.actor_timings.timings().to_vec();
        summary
    }

    /// Enables the step observer with a ring buffer of the given capacity.
    ///
    /// Calling this again replaces any existing buffer and discards buffered snapshots.
    /// Also initialises the post-hoc diagnostic accumulator when called for the
    /// first time or re-created.
    #[cfg(feature = "observe")]
    pub fn enable_observer(&mut self, capacity: usize) {
        self.observer_buf = Some(ObserverBuffer::new(capacity));
        self.initialize_diagnostic_accum();
    }

    /// Initialises (or re-initialises) the post-hoc diagnostic accumulator from
    /// current zone and equipment state.  Call after `enable_observer()` and
    /// after any equipment add/remove that changes the equipment list.
    #[cfg(feature = "observe")]
    fn initialize_diagnostic_accum(&mut self) {
        let zone_ids: Vec<ZoneId> = self.latest_env.zones.iter().map(|z| z.id).collect();
        let equipment_names: Vec<String> = self
            .equipment
            .iter()
            .map(|eq| eq.descriptor().name.clone())
            .collect();
        let equipment_zones: Vec<Option<ZoneId>> = self
            .equipment
            .iter()
            .map(|eq| eq.descriptor().zone)
            .collect();
        self.diagnostic_accum = Some(DiagnosticAccumulator::new(
            &zone_ids,
            &self.zone_is_conditioned,
            &equipment_names,
            &equipment_zones,
        ));
    }

    /// Drains all buffered step snapshots.
    #[cfg(feature = "observe")]
    pub fn drain_observations(&mut self) -> Vec<StepSnapshot> {
        self.observer_buf
            .as_mut()
            .map(|buf| buf.drain())
            .unwrap_or_default()
    }

    /// Returns a reference to the observer buffer, if enabled.
    #[cfg(feature = "observe")]
    #[must_use]
    pub fn observer_buffer(&self) -> Option<&ObserverBuffer> {
        self.observer_buf.as_ref()
    }

    /// Pushes a warning string into the internal warning queue.
    pub fn push_warning(&mut self, msg: String) {
        self.warnings.push(msg);
    }

    /// Returns all record batches flushed by the streaming recorder so far.
    #[must_use]
    pub fn flushed_batches(&self) -> &[arrow::record_batch::RecordBatch] {
        self.recorder
            .as_ref()
            .map_or(&[], StreamingRecorder::flushed_batches)
    }

    /// Takes run metrics collected incrementally by the streaming recorder.
    ///
    /// `None` when the recorder was never constructed, when no metrics
    /// calculator was attached (retained-batch runs compute metrics post-hoc
    /// from [`Self::flushed_batches`] instead), or when an attached
    /// calculator failed to initialize — [`Self::recorded_rows`] and the
    /// dwelling warnings distinguish the cases. The inner `Result` carries
    /// the calculator's typed error (a non-finite accumulator) when the
    /// metrics could not be finalized.
    pub fn take_streamed_metrics(
        &mut self,
    ) -> Option<std::result::Result<FullSimulationMetrics, HaresError>> {
        self.recorder
            .as_mut()?
            .take_metrics()
            .map(|res| res.map_err(|e| HaresError::InvalidState(e.to_string())))
    }

    /// Rows written to the output recorder over the run; `None` when no
    /// recorder was constructed (`write_output` disabled).
    #[must_use]
    pub fn recorded_rows(&self) -> Option<usize> {
        self.recorder.as_ref().map(StreamingRecorder::total_rows)
    }

    /// Populates `latest_env.equipment_core` and `latest_env.equipment_telemetry`
    /// by snapshotting the current `core_output()` and `telemetry()` from every
    /// equipment instance.  Called after `load_checkpoint` so that the first
    /// post-restore step's actors see committed equipment state rather than an
    /// empty map (which would force fallback paths in SOC-dependent actors like
    /// `EvDriverActor` and `BatteryManagementActor`).
    fn snapshot_equipment_state(&mut self) {
        self.latest_env.equipment_core.clear();
        self.latest_env
            .equipment_telemetry
            .retain(|name, _| name == hares_types::telemetry_keys::HUMIDITY_SOLVER_TELEMETRY_KEY);

        for eq in &self.equipment {
            let desc = eq.descriptor();
            let id = self
                .equipment_id_by_name
                .get(&desc.name)
                .copied()
                .expect("invariant: equipment_id_by_name is built from this equipment set");
            // Identity desync invariant: the name→id map is rebuilt only by
            // a roster change, while equipment-side consumers read
            // `descriptor().id` live. A divergence here means some code path
            // mutated an equipment's id after registration, so every
            // id-keyed lookup would address the wrong equipment. No public
            // API can reach it; this assert catches any future internal
            // mutator.
            debug_assert_eq!(
                id, desc.id,
                "equipment '{}' id drifted from its registered identity",
                desc.name
            );
            self.latest_env
                .equipment_core
                .insert(id, eq.core_output().clone());

            let telemetry = eq.telemetry();
            match self.latest_env.equipment_telemetry.get_mut(&desc.name) {
                Some(existing) => existing.clone_from(telemetry),
                None => {
                    self.latest_env
                        .equipment_telemetry
                        .insert(desc.name.clone(), telemetry.clone());
                }
            }
        }
    }

    /// Accumulates occupancy-driven internal heat gains into zone thermal ports.
    ///
    /// Reads the current occupancy count from the schedule payload carried in
    /// `latest_env.custom_domains`, then deposits gains into the **conditioned
    /// (indoor) zone only** — matching OCHRE behaviour where occupant heat is
    /// applied solely to the `indoor_zone`:
    ///   - sensible convective: `n_occupants × OCCUPANT_SENSIBLE_GAIN_W × OCCUPANT_CONVECTIVE_FRACTION`
    ///   - sensible radiative:  `n_occupants × OCCUPANT_SENSIBLE_GAIN_W × OCCUPANT_RADIATIVE_FRACTION`
    ///   - latent:              `n_occupants × OCCUPANT_LATENT_GAIN_W`
    ///
    /// If no occupancy column is present in the schedule the method returns without
    /// side-effects.  Construction-time validation in `from_preparsed` guarantees that
    /// when an Occupancy spec exists the schedule always includes an occupancy column,
    /// so a `None` column index here indicates an intentionally unoccupied dwelling
    /// (e.g. BESTEST base cases with `occupants_present: false`).
    fn apply_occupancy_gains(&mut self) -> Result<()> {
        let Some(col_idx) = self.occupancy_column_idx else {
            return Ok(());
        };

        // Construction-time validation guarantees the schedule domain exists
        // and carries a payload when occupancy_column_idx is Some.  Absence at
        // step time is a programming error (schedule update not pushed).
        let n_occupants = self
            .latest_env
            .custom_domains
            .iter()
            .find(|u| u.domain_id == SCHEDULE_DOMAIN_ID)
            .and_then(|u| u.custom_payload.as_ref())
            .and_then(|p| p.get(col_idx))
            .copied()
            .expect(
                "SCHEDULE_DOMAIN_ID payload absent at step time: \
                 construction validated occupancy column exists in schedule; \
                 environment update must always push the schedule domain payload.",
            )
            * self.occupancy_scale;

        if n_occupants <= 0.0 {
            return Ok(());
        }

        let sensible_w = n_occupants * OCCUPANT_SENSIBLE_GAIN_W * OCCUPANT_CONVECTIVE_FRACTION;
        let radiant_w = n_occupants * OCCUPANT_SENSIBLE_GAIN_W * OCCUPANT_RADIATIVE_FRACTION;
        let latent_w = n_occupants * OCCUPANT_LATENT_GAIN_W;

        let indoor_zone = self.thermal_solver.config().indoor_zone_id;
        self.ports.accumulate(&PortContribution::Thermal {
            zone: indoor_zone,
            sensible_gain_w: sensible_w,
            radiant_gain_w: radiant_w,
            latent_gain_w: latent_w,
            category: ThermalCategory::InternalGain,
        })?;

        Ok(())
    }

    /// Iterative warm-up convergence per EnergyPlus ERM 26.1 — Warmup Convergence.
    ///
    /// Repeatedly simulates the first 24-hour weather day until the maximum zone
    /// temperature change across all conditioned zones between consecutive iterations
    /// falls below `threshold_c` °C, up to `max_iter` iterations.
    ///
    /// EnergyPlus defaults: threshold = 0.5 °C, max_iter = 25 iterations.
    /// EnergyPlus I/O Reference 26.1 — Simulation Parameters: Building: "This value represents the number at
    /// which the zone temperatures must agree ... before 'convergence' is reached."
    /// Typical convergence: 1-2 iterations for lightweight construction, 4-7 for
    /// heavyweight (concrete slab, masonry).
    ///
    /// The outcome (days run, convergence, day-over-day residuals) is recorded
    /// on [`Self::health`] at every return path once warm-up runs.
    pub fn run_warmup_converged(&mut self, threshold_c: f64, max_iter: u32) -> Result<u32> {
        let time_res_s = u64::try_from(self.clock.time_res.num_seconds())
            .map_err(|_| HaresError::Io("invalid time resolution".to_string()))?;
        let steps_per_day = (24u64 * 3600).checked_div(time_res_s).unwrap_or(0);

        if steps_per_day == 0 {
            return Ok(1);
        }

        self.is_warming_up = true;

        let mut prev_zone_temps: Vec<f64> = Vec::new();
        // Previous day's per-zone daily max/min temperatures and aggregate
        // daily peak HVAC delivery: the residual baseline.
        let mut prev_day_max_t: Option<Vec<f64>> = None;
        let mut prev_day_min_t: Vec<f64> = Vec::new();
        let mut prev_peak_heating_w: f64 = 0.0;
        let mut prev_peak_cooling_w: f64 = 0.0;
        // Residuals of the final day (recorded on health at the return paths)
        // and the zones whose final-day temperature residual exceeded the
        // convergence threshold (named in the non-convergence warning).
        let mut final_residuals: Option<WarmupResiduals> = None;
        let mut threshold_exceeding_zones: Vec<ZoneId> = Vec::new();

        for iteration in 1..=max_iter {
            // Per-zone daily extremes and aggregate daily delivery peaks for
            // this warm-up day, accumulated per step below.
            let mut day_max_t: Vec<f64> = vec![f64::NEG_INFINITY; self.latest_env.zones.len()];
            let mut day_min_t: Vec<f64> = vec![f64::INFINITY; self.latest_env.zones.len()];
            let mut day_peak_heating_w: f64 = 0.0;
            let mut day_peak_cooling_w: f64 = 0.0;

            // Reset clock to start of first day for weather replay.
            // Thermal state carries forward from previous iteration:
            // EnergyPlus ERM 26.1 — Warmup Convergence — initial conditions for each
            // warmup day are the final conditions from the previous warmup day.
            self.clock.current_step = 0;

            for _ in 0..steps_per_day {
                if let Err(err) = self.run_timestep(false) {
                    // The run failed mid-warm-up: exit warm-up mode and
                    // record the outcome honestly (stopped during
                    // `iteration`, not converged) before propagating.
                    self.is_warming_up = false;
                    self.health.warmup = WarmupOutcome::Ran {
                        days_run: iteration,
                        converged: false,
                        residuals: final_residuals.clone(),
                    };
                    return Err(err);
                }
                for (i, zone) in self.latest_env.zones.iter().enumerate() {
                    day_max_t[i] = day_max_t[i].max(zone.temperature_c);
                    day_min_t[i] = day_min_t[i].min(zone.temperature_c);
                }
                day_peak_heating_w = day_peak_heating_w.max(self.step_hvac_heating_w);
                day_peak_cooling_w = day_peak_cooling_w.max(self.step_hvac_cooling_w);
            }

            // Collect conditioned zone temperatures from the environment state.
            // Order is stable: both `latest_env.zones` and `zone_is_conditioned`
            // are aligned by construction.
            let zone_temps: Vec<f64> = (0..self.latest_env.zones.len())
                .filter(|&i| self.zone_is_conditioned[i])
                .map(|i| self.latest_env.zones[i].temperature_c)
                .collect();

            if let Some(prev_max_t) = &prev_day_max_t {
                // Day-over-day residuals per zone: |Δ daily max| and
                // |Δ daily min|, worst zone = largest max of the two. The
                // aggregate daily-peak heating/cooling relative changes are
                // attributed to that zone's row (delivery is measured in
                // aggregate only).
                let mut worst_idx = 0usize;
                let mut worst_residual = f64::MIN;
                let mut worst_dmax = 0.0f64;
                let mut worst_dmin = 0.0f64;
                threshold_exceeding_zones.clear();
                for (i, zone) in self.latest_env.zones.iter().enumerate() {
                    let d_max = (day_max_t[i] - prev_max_t[i]).abs();
                    let d_min = (day_min_t[i] - prev_day_min_t[i]).abs();
                    let residual = d_max.max(d_min);
                    if residual > worst_residual {
                        worst_residual = residual;
                        worst_idx = i;
                        worst_dmax = d_max;
                        worst_dmin = d_min;
                    }
                    if residual >= threshold_c {
                        threshold_exceeding_zones.push(zone.id);
                    }
                }
                let heating_load = if prev_peak_heating_w > 0.0 {
                    (day_peak_heating_w - prev_peak_heating_w).abs() / prev_peak_heating_w
                } else {
                    0.0
                };
                let cooling_load = if prev_peak_cooling_w > 0.0 {
                    (day_peak_cooling_w - prev_peak_cooling_w).abs() / prev_peak_cooling_w
                } else {
                    0.0
                };
                final_residuals = Some(WarmupResiduals {
                    worst_zone: self.latest_env.zones[worst_idx].id,
                    max_temperature_c: worst_dmax,
                    min_temperature_c: worst_dmin,
                    heating_load,
                    cooling_load,
                });
            }

            if !prev_zone_temps.is_empty() {
                // EnergyPlus ERM 26.1 — Warmup Convergence:
                // max |ΔT_zone| across all conditioned zones. Convergence
                // criterion: 0.5 °C (EnergyPlus I/O Reference default).
                let max_delta = prev_zone_temps
                    .iter()
                    .zip(zone_temps.iter())
                    .map(|(prev, curr)| (prev - curr).abs())
                    .fold(0.0_f64, f64::max);

                self.simulation_results.steps.clear();

                if max_delta < threshold_c {
                    tracing::info!(
                        iterations = iteration,
                        max_delta_c = max_delta,
                        threshold_c,
                        "warm-up converged"
                    );
                    self.is_warming_up = false;
                    self.health.warmup = WarmupOutcome::Ran {
                        days_run: iteration,
                        converged: true,
                        residuals: final_residuals.clone(),
                    };
                    return Ok(iteration);
                }
            }

            prev_zone_temps = zone_temps;
            prev_day_max_t = Some(day_max_t);
            prev_day_min_t = day_min_t;
            prev_peak_heating_w = day_peak_heating_w;
            prev_peak_cooling_w = day_peak_cooling_w;
        }

        self.is_warming_up = false;

        self.simulation_results.steps.clear();

        // Honest warning surface: the run proceeded without a converged
        // initial state, so the non-convergence is counted in health AND
        // pushed as a run warning naming the zones whose day-over-day
        // temperature residual exceeded the convergence threshold (the
        // worst-residual zone when none individually exceeds it).
        self.health.warmup = WarmupOutcome::Ran {
            days_run: max_iter,
            converged: false,
            residuals: final_residuals.clone(),
        };
        let warned_zones = if threshold_exceeding_zones.is_empty() {
            final_residuals
                .as_ref()
                .map(|r| vec![r.worst_zone])
                .unwrap_or_default()
        } else {
            threshold_exceeding_zones
        };
        self.warnings.push(format!(
            "warm-up failed to converge within {max_iter} iterations; proceeding with \
             current thermal state; zones exceeding the {threshold_c} °C daily \
             residual: {warned_zones:?}"
        ));
        tracing::warn!(
            iterations = max_iter,
            "warm-up failed to converge within {} iterations; \
             proceeding with current thermal state",
            max_iter
        );
        Ok(max_iter)
    }

    /// Returns true if an actor interest was triggered by state changes
    /// between the previous step and the current step.
    fn interest_triggered(
        interest: &ActorInterest,
        env: &EnvironmentState,
        filter: &InterestFilterState<'_>,
    ) -> bool {
        let InterestFilterState {
            prev_zone_temps,
            prior_zone_temps,
            prev_price_signal,
            prev_equipment_modes,
            equipment_id_by_name,
            equipment,
        } = filter;
        match interest {
            ActorInterest::EveryStep => true,
            ActorInterest::ZoneTemperatureDelta { zone, threshold_c } => {
                let current = env
                    .zones
                    .iter()
                    .find(|z| z.id == *zone)
                    .map(|z| z.temperature_c);
                match (current, prior_zone_temps.get(zone)) {
                    (Some(current), Some(prev)) => (current - prev).abs() >= *threshold_c,
                    (Some(_), None) => true,
                    _ => false,
                }
            }
            ActorInterest::TimeOfDay { hour } => env.current_time.hour() as u8 == *hour,
            ActorInterest::EquipmentModeChange { target } => Self::equipment_mode_changed(
                target,
                env,
                prev_equipment_modes,
                equipment_id_by_name,
                equipment,
            ),
            ActorInterest::PriceSignalChange => {
                prev_zone_temps.is_empty() || env.price_signal != **prev_price_signal
            }
        }
    }

    /// Checks whether the operating mode of the equipment identified by `target`
    /// has changed since the previous step.
    fn equipment_mode_changed(
        target: &DispatchTarget,
        env: &EnvironmentState,
        prev_equipment_modes: &HashMap<EquipmentId, Option<OperatingMode>>,
        equipment_id_by_name: &HashMap<String, EquipmentId>,
        equipment: &[Box<dyn Equipment>],
    ) -> bool {
        match target {
            DispatchTarget::ByName(name) => {
                let Some(&id) = equipment_id_by_name.get(name.as_ref()) else {
                    return false;
                };
                let current = env
                    .equipment_core
                    .get(&id)
                    .and_then(|co| co.state.operating_mode);
                let prev = prev_equipment_modes.get(&id).copied().flatten();
                current != prev
            }
            DispatchTarget::ByEndUse(end_use) => {
                for eq in equipment {
                    if eq.descriptor().end_use == *end_use {
                        let desc = eq.descriptor();
                        let Some(&id) = equipment_id_by_name.get(&desc.name) else {
                            continue;
                        };
                        let current = env
                            .equipment_core
                            .get(&id)
                            .and_then(|co| co.state.operating_mode);
                        let prev = prev_equipment_modes.get(&id).copied().flatten();
                        if current != prev {
                            return true;
                        }
                    }
                }
                false
            }
        }
    }

    /// Roll back port contributions from a failed equipment step.
    ///
    /// Called after `self.rollback_ports.copy_into(&self.ports)` captured
    /// the pre-step snapshot and the equipment's `step()` returned `Err`.
    /// Swaps `self.ports` ↔ `self.rollback_ports` (restoring pre-step state),
    /// computes discarded contribution totals, pushes a warning, increments
    /// the run-total health counter, and emits a `tracing::warn!`.
    fn rollback_failed_equipment_ports(&mut self, idx: usize, err: &HaresError) {
        self.warnings.push(format!(
            "equipment step failed for '{}' : {err}",
            self.equipment[idx].descriptor().name
        ));
        // self.rollback_ports holds the pre-step snapshot (from copy_into);
        // self.ports holds the polluted post-step state.
        std::mem::swap(&mut self.ports, &mut self.rollback_ports);
        // Now self.ports is restored, self.rollback_ports is polluted.
        let name = &self.equipment[idx].descriptor().name;
        let id = self.equipment[idx].descriptor().id;
        let discarded_thermal_w: f64 = self
            .rollback_ports
            .thermal
            .iter()
            .map(|t| t.sensible_gain_w + t.radiant_gain_w + t.latent_gain_w)
            .sum::<f64>()
            - self
                .ports
                .thermal
                .iter()
                .map(|t| t.sensible_gain_w + t.radiant_gain_w + t.latent_gain_w)
                .sum::<f64>();
        let discarded_electrical_w: f64 =
            self.rollback_ports.electrical.net_active_w() - self.ports.electrical.net_active_w();
        let discarded_electrical_kvar: f64 = self.rollback_ports.electrical.reactive_power_kvar
            - self.ports.electrical.reactive_power_kvar;
        let discarded_fuel_w: f64 = ALL_FUEL_TYPES
            .iter()
            .map(|&ft| self.rollback_ports.fuel.get(ft))
            .sum::<f64>()
            - ALL_FUEL_TYPES
                .iter()
                .map(|&ft| self.ports.fuel.get(ft))
                .sum::<f64>();
        self.health.port_rollbacks += 1;
        tracing::warn!(
            equipment = %name,
            equipment_id = %id,
            discarded_thermal_w,
            discarded_electrical_w,
            discarded_electrical_kvar,
            discarded_fuel_w,
            "rolled back port contributions from failed equipment step"
        );
    }

    fn run_timestep(&mut self, record_output: bool) -> Result<()> {
        if self.failed {
            return Err(HaresError::Simulation(
                "dwelling permanently failed after prior panic, cannot step".to_string(),
            ));
        }
        if self.clock.current_step() >= self.clock.total_steps() {
            return Err(HaresError::Simulation(
                "simulation already reached configured end".to_string(),
            ));
        }

        // One phase clock per step: read at entry, closed at each phase
        // boundary below, finished at return. The phases partition the step
        // exactly.
        #[cfg(feature = "profiling")]
        let mut phase_clock = PhaseClock::start();

        // Advance the dwelling RNG on every timestep so checkpoint captures
        // reflect simulation progress.  The value is intentionally discarded;
        // stochastic components use independent sub-RNGs derived from the
        // dwelling RNG's seed via stream partitioning.
        #[cfg(debug_assertions)]
        let rng_word_pos_before = self.rng.get_word_pos();
        let _ = advance_dwelling_rng(&mut self.rng);

        #[cfg(debug_assertions)]
        let rng_consumed_this_step = self.rng.get_word_pos() > rng_word_pos_before;

        #[cfg(debug_assertions)]
        if !rng_consumed_this_step {
            debug_assert!(false, "dwelling RNG consumed no bits this step");
        }

        // After the first step, every equipment instance must have a core
        // output entry in the environment: actors observe equipment through
        // these id-keyed entries, and a missing entry means the equipment
        // is unobservable (the no-observation sentinel) for the rest of
        // the run. Known causes this check guards against: an identity
        // refresh that failed to seed a newly entered equipment's entry,
        // and a checkpoint restore that skipped the post-restore
        // snapshot. A tolerated equipment-step failure is NOT a cause:
        // the last committed entry is retained, not dropped.
        // Unconditional in every build profile, over the ids pre-resolved
        // at refresh (no per-equipment hash lookups on the step path).
        if self.clock.current_step() > 0 {
            for (idx, &id) in self.roster.equipment_ids.iter().enumerate() {
                if !self.latest_env.equipment_core.contains_key(&id) {
                    let name = self
                        .equipment
                        .get(idx)
                        .map(|eq| eq.descriptor().name.clone())
                        .unwrap_or_default();
                    return Err(HaresError::InvalidState(format!(
                        "equipment_core missing entry for equipment '{name}' (id={id:?}) \
                         at start of step {}: every active equipment must be \
                         observable; check the identity-refresh seeding and \
                         the checkpoint-restore snapshot (tolerated step \
                         failures retain the last committed entry)",
                        self.clock.current_step(),
                    )));
                }
            }
        }

        // First step after checkpoint restore: double-check that
        // equipment_core is not accidentally empty when we expect
        // restored state. The latch reset runs in every build profile.
        if self.restored_from_checkpoint && !self.equipment.is_empty() {
            if self.latest_env.equipment_core.is_empty() {
                return Err(HaresError::InvalidState(format!(
                    "equipment_core is empty on first post-restore step {}; \
                     load_checkpoint must call snapshot_equipment_state",
                    self.clock.current_step(),
                )));
            }
            self.restored_from_checkpoint = false;
        }

        #[cfg(feature = "profiling")]
        let alloc_before = thread_allocations();

        #[cfg(feature = "observe")]
        let mut obs_phases = PhaseSnapshots::default();

        // Step 1: update environment at current clock state.
        // Feed zone temperatures back first so the borrow on self.latest_env.zones
        // is released before we mutably borrow self.latest_env for update_in_place.
        self.environment.feed_zones(&self.latest_env.zones);
        self.environment
            .update_in_place(&mut self.latest_env, &self.clock)?;

        // Populate price signal from tariff evaluator (deterministic function of clock).
        if let Some(ref evaluator) = self.tariff_evaluator {
            self.latest_env.price_signal = PriceSignal {
                electricity_price: Some(evaluator.current_price()),
                export_price: Some(evaluator.current_export_price()),
                ghg_intensity: self.latest_env.price_signal.ghg_intensity,
            };
        } else {
            self.latest_env.price_signal = self.price_signal.clone();
        }

        // Populate electrical summary from prior step's solver results.
        self.latest_env.electrical = self.prior_electrical_summary.clone();

        // Resolve bus energization for this step. During a utility outage
        // (voltage_pu == 0.0), an island-capable source (battery with usable
        // charge, generator, discharging V2G EV) holds the home bus at
        // nominal voltage so loads keep running; otherwise the bus is dead
        // and every load force-offs at the control level (see
        // `GridState::bus_energized`). Source availability is evaluated from
        // end-of-previous-step equipment state, so island formation/collapse
        // takes effect with a one-step lag.
        self.latest_env.grid.island_bus_voltage_pu = if self.latest_env.grid.voltage_pu == 0.0
            && self.equipment.iter().any(|eq| eq.island_source_available())
        {
            Some(ISLAND_BUS_NOMINAL_VOLTAGE_PU)
        } else {
            None
        };

        // Step-start humidity invariant: confirm that the humidity ratio in
        // `latest_env.zones` matches the humidity solver's committed state.
        // Both are updated together at the end of each timestep by
        // `apply_humidity_update_to_zones` → `humidity_solver.resolve`, and
        // Step 1's environment update preserves zone state byte-for-byte via
        // `extend_from_slice`. A mismatch here indicates a zone was added
        // after solver construction without seeding its initial humidity ratio.
        #[cfg(debug_assertions)]
        for zone in &self.latest_env.zones {
            if self.is_warming_up {
                continue;
            }
            let solver_hr = self.humidity_solver.humidity_ratio(zone.id);
            if zone.humidity_ratio.is_nan() && solver_hr.is_nan() {
                continue;
            }
            debug_assert!(
                (zone.humidity_ratio - solver_hr).abs() < f64::EPSILON,
                "zone {} humidity ratio {:.6e} diverged from solver committed {:.6e} at start of step",
                zone.id.0,
                zone.humidity_ratio,
                solver_hr,
            );
        }

        #[cfg(feature = "observe")]
        if self.observer_buf.is_some() {
            obs_phases.post_environment = Some(observer_capture::capture_environment(
                &self.latest_env,
                self.environment.weather_meta.wf_allows_leap_years,
                self.environment.raw_mains_temp_f,
                self.environment.obs_raw_day_of_year,
                self.environment.obs_civil_day_of_year,
                self.environment.obs_utc_day_of_year,
            ));
        }

        #[cfg(feature = "observe")]
        let observing = self.observer_buf.is_some();

        let dt = chrono_to_std_duration(self.clock.time_res)?;

        // Begin a new dispatch window: clear the cross-pass priority ledger so
        // that lower-priority signals queued late in the step cannot overwrite
        // higher-priority signals applied in an earlier pass.
        self.control_dispatcher.begin_step();

        // Step 1a': dispatch any externally queued control signals (e.g. from
        // `apply_control_validated`) before thermal equipment `update_control`
        // runs. Without this, setpoint/mode overrides queued for the current
        // step would be serviced at Step 2 (after the thermostat FSM has
        // already advanced in Step 1c), and short-cycle protection in
        // `is_cycle_change_allowed` would block the re-evaluation at Step 2a
        // until the next step. Dispatching first lets setpoint and mode
        // overrides take effect on the same step.
        #[cfg(feature = "observe")]
        let pre_dispatch_capture = if self.observer_buf.is_some() {
            Some(self.control_dispatcher.dispatch_into_observed(
                &mut self.equipment,
                &mut self.warnings,
                &mut self.health.rejected_control_signals,
            ))
        } else {
            self.control_dispatcher.dispatch_into(
                &mut self.equipment,
                &mut self.warnings,
                &mut self.health.rejected_control_signals,
            );
            None
        };
        #[cfg(not(feature = "observe"))]
        self.control_dispatcher.dispatch_into(
            &mut self.equipment,
            &mut self.warnings,
            &mut self.health.rejected_control_signals,
        );
        // End of the `environment` phase: entry through Step 1a' (environment
        // update, queued control dispatch).
        #[cfg(feature = "profiling")]
        phase_clock.enter(&mut self.profiling, ProfilePhase::Control);

        // Step 1b: deposit deterministic internal gains (occupancy, plug loads)
        // BEFORE prepare_inputs so the ideal solver sees them when computing
        // required HVAC capacity.
        self.apply_occupancy_gains()?;

        let mut step_succeeded = vec![false; self.equipment.len()];

        // Step 1c: thermal equipment update_control() to determine mode and ideal targets.
        // Must run BEFORE solver feedback actor collects targets.
        for &idx in &self.roster.equipment_execution_order {
            if self.equipment[idx].descriptor().stage == ExecutionStage::Thermal {
                let _ = self.equipment[idx].update_control(&self.latest_env);
            }
        }

        // Step 1d: build current-step inputs with all non-HVAC gains already on ports.
        self.thermal_solver
            .prepare_inputs(&self.ports, &self.latest_env)?;

        // Steps 1e–1f: phase-ordered actor execution driven by the scheduler's
        // plan (explicit phase registration, within-phase priority ordering),
        // rebuilt below only when a registration changed.
        #[cfg(feature = "observe")]
        let mut scheduled_phases: Vec<String> = Vec::new();
        #[cfg(feature = "observe")]
        let mut actor_skips: usize = 0;
        #[cfg(feature = "observe")]
        let mut actor_calls: usize = 0;
        #[cfg(feature = "observe")]
        let mut actor_error_count: usize = 0;
        #[cfg(debug_assertions)]
        let mut executed_actors: std::collections::HashSet<usize> =
            std::collections::HashSet::new();

        self.actor_dispatch_buf.clear();
        for entry in self.scheduler.build() {
            // The scheduler loop interleaves `ideal_capacity` and `actors`
            // spans in plan order: each entry closes the previous span and
            // opens its own phase. An `ActorDecide` entry's span (the
            // interest filter, `decide()` and the health check) is routed
            // to that actor's slot in `actor_timings` when it closes, so
            // the per-actor totals sum to the `actors` phase exactly.
            #[cfg(feature = "profiling")]
            match entry.phase {
                ExecutionPhase::SolverFeedback => phase_clock.enter_plan_entry(
                    &mut self.profiling,
                    &mut self.actor_timings,
                    ProfilePhase::IdealCapacity,
                    None,
                ),
                ExecutionPhase::ActorDecide => phase_clock.enter_plan_entry(
                    &mut self.profiling,
                    &mut self.actor_timings,
                    ProfilePhase::Actors,
                    Some(
                        entry
                            .slot
                            .expect("ActorDecide plan entries must carry a slot")
                            .0,
                    ),
                ),
            }
            match entry.phase {
                ExecutionPhase::SolverFeedback => {
                    #[cfg(feature = "observe")]
                    scheduled_phases.push("SolverFeedback".to_string());

                    // Step 1e: collect ideal targets and solve capacities.
                    self.solver_feedback_actor
                        .collect_and_solve(&self.equipment, &mut self.thermal_solver);
                    // Step 1f: decide and queue solver feedback signals.
                    self.solver_feedback_actor
                        .decide(&self.latest_env, &mut self.actor_dispatch_buf);
                    for req in self.actor_dispatch_buf.drain(..) {
                        self.control_dispatcher.queue(req);
                    }
                }
                ExecutionPhase::ActorDecide => {
                    let idx = entry
                        .slot
                        .expect("ActorDecide plan entries must carry a slot")
                        .0;

                    let interests = self.actors[idx].interests();
                    let filter = InterestFilterState {
                        prev_zone_temps: &self.prev_zone_temps,
                        prior_zone_temps: &self.prior_zone_temps,
                        prev_price_signal: &self.prev_price_signal,
                        prev_equipment_modes: &self.prev_equipment_modes,
                        equipment_id_by_name: &self.equipment_id_by_name,
                        equipment: &self.equipment,
                    };
                    let should_call = interests.is_empty()
                        || interests.iter().any(|interest| {
                            Self::interest_triggered(interest, &self.latest_env, &filter)
                        });

                    #[cfg(feature = "observe")]
                    if should_call {
                        actor_calls += 1;
                    } else {
                        actor_skips += 1;
                    }

                    if should_call {
                        #[cfg(feature = "observe")]
                        scheduled_phases.push(format!("ActorDecide({})", entry.name));
                        #[cfg(debug_assertions)]
                        {
                            executed_actors.insert(idx);
                        }

                        // The open `Actors` span (this entry's whole
                        // scheduler time, closed at the next phase
                        // boundary) counts a `decide()` call. No extra
                        // `Instant` read: the span's difference is the
                        // phase clock's own.
                        #[cfg(feature = "profiling")]
                        phase_clock.mark_open_actor_called();

                        let emitted_from = self.actor_dispatch_buf.len();
                        self.actors[idx].decide(&self.latest_env, &mut self.actor_dispatch_buf);
                        crate::actor::check_declared_signals(
                            self.actors[idx].as_ref(),
                            &self.actor_dispatch_buf[emitted_from..],
                        )?;

                        if !self.actors[idx].healthy() {
                            #[cfg(feature = "observe")]
                            {
                                actor_error_count += 1;
                            }
                            tracing::error!(
                                actor = %self.actors[idx].name(),
                                "actor is unhealthy after decide() — dispatch output may be compromised"
                            );
                        }
                    }
                }
            }
        }
        // The `dispatch` phase opens here: the ActorDecide buffer drain, the
        // post-loop actor health checks and Steps 2 and 2b all queue or apply
        // control signals. The plan variant routes the final plan entry's
        // span (an `Actors` span when the plan ends with one) to its slot.
        #[cfg(feature = "profiling")]
        phase_clock.enter_plan_entry(
            &mut self.profiling,
            &mut self.actor_timings,
            ProfilePhase::Dispatch,
            None,
        );

        // Drain dispatch from all ActorDecide entries.
        for req in self.actor_dispatch_buf.drain(..) {
            self.control_dispatcher.queue(req);
        }

        #[cfg(debug_assertions)]
        {
            for (i, actor) in self.actors.iter().enumerate() {
                let interests = actor.interests();
                let is_every_step = interests.is_empty()
                    || interests
                        .iter()
                        .any(|i| matches!(i, ActorInterest::EveryStep));
                let registered = self
                    .scheduler
                    .plan()
                    .iter()
                    .any(|e| e.slot == Some(crate::scheduler::ActorSlot(i)));
                if registered && is_every_step {
                    debug_assert!(
                        executed_actors.contains(&i),
                        "EveryStep actor '{}' (index {}) was not called during ActorDecide phase",
                        actor.name(),
                        i,
                    );
                }
            }
        }

        // Actor health after the decide phase: an unhealthy actor indicates a
        // swallowed error (Python exception or invalid return type) at the
        // decide() call site: input-triggerable enforcement, so it fails the
        // step in every build profile (the decide site's unconditional
        // tracing::error! already logged it).
        for (i, actor) in self.actors.iter().enumerate() {
            if !actor.healthy() {
                return Err(HaresError::Dwelling(format!(
                    "actor '{}' (index {}) is unhealthy after the ActorDecide phase; \
                     its decide() swallowed an error; dispatch output may be compromised",
                    actor.name(),
                    i
                )));
            }
        }

        #[cfg(feature = "observe")]
        if self.observer_buf.is_some() {
            tracing::debug!(
                phase_count = scheduled_phases.len(),
                phases = ?scheduled_phases,
                "scheduler phases executed"
            );
        }

        // Step 2: dispatch all actor-generated control signals. The priority
        // ledger from the pre-thermal-FSM pass is preserved so that a
        // lower-priority actor signal cannot overwrite a higher-priority
        // external signal already applied.
        #[cfg(feature = "observe")]
        if self.observer_buf.is_some() {
            let capture = self.control_dispatcher.dispatch_into_observed(
                &mut self.equipment,
                &mut self.warnings,
                &mut self.health.rejected_control_signals,
            );
            let merged = match pre_dispatch_capture {
                Some(mut pre) => {
                    pre.signals.extend(capture.signals);
                    pre
                }
                None => capture,
            };
            obs_phases.post_dispatch = Some(merged);
        } else {
            self.control_dispatcher.dispatch_into(
                &mut self.equipment,
                &mut self.warnings,
                &mut self.health.rejected_control_signals,
            );
        }
        #[cfg(not(feature = "observe"))]
        self.control_dispatcher.dispatch_into(
            &mut self.equipment,
            &mut self.warnings,
            &mut self.health.rejected_control_signals,
        );

        // Step 2b: collect and dispatch derived control signals from equipment
        // that translate one signal into others (e.g. ProtocolBridge converting
        // a ProtocolNative payload into standard ControlSignal variants).
        //
        // This runs AFTER the actor dispatch pass (Step 2) and BEFORE thermal
        // re-update (Step 2a) so that translated setpoints, mode overrides, and
        // power targets take effect on the CURRENT timestep.
        //
        // A bounded loop (max 3 iterations) handles cascades: a derived signal
        // dispatched to a second ProtocolBridge could produce further derived
        // signals. In practice one iteration suffices because bridges emit
        // standard signals, not ProtocolNative.
        const MAX_DERIVED_ITERATIONS: u32 = 3;
        let mut protocol_native_checked = false;
        for _iteration in 0..MAX_DERIVED_ITERATIONS {
            let mut derived: Vec<DispatchRequest> = Vec::new();
            for eq in self.equipment.iter_mut() {
                for (target_name, signal) in eq.drain_command_signals() {
                    derived.push(DispatchRequest {
                        target: DispatchTarget::ByName(target_name.into()),
                        signal,
                        // Protocol-bridge-translated signals are equipment-internal
                        // commands that should take precedence over schedule-tier
                        // operations but not override grid-level DR signals.
                        priority: PriorityTier::UserOverride,
                    });
                }
            }
            if derived.is_empty() {
                break;
            }
            if !protocol_native_checked {
                protocol_native_checked = true;
                let capabilities: Vec<ControlCapabilities> = self
                    .equipment
                    .iter()
                    .map(|e| e.descriptor().control_capabilities)
                    .collect();
                self.invariant_checker
                    .check_protocol_native_registration(&capabilities)?;
            }
            for req in derived {
                self.control_dispatcher.queue(req);
            }
            #[cfg(feature = "observe")]
            if self.observer_buf.is_some() {
                let capture = self.control_dispatcher.dispatch_into_observed(
                    &mut self.equipment,
                    &mut self.warnings,
                    &mut self.health.rejected_control_signals,
                );
                if let Some(ref mut merged) = obs_phases.post_dispatch {
                    merged.signals.extend(capture.signals);
                }
            } else {
                self.control_dispatcher.dispatch_into(
                    &mut self.equipment,
                    &mut self.warnings,
                    &mut self.health.rejected_control_signals,
                );
            }
            #[cfg(not(feature = "observe"))]
            self.control_dispatcher.dispatch_into(
                &mut self.equipment,
                &mut self.warnings,
                &mut self.health.rejected_control_signals,
            );
        }

        // End of the `dispatch` phase (Steps 2 and 2b); Step 2a re-runs
        // `update_control`, the second span of the `control` phase.
        #[cfg(feature = "profiling")]
        phase_clock.enter(&mut self.profiling, ProfilePhase::Control);

        // Step 2a: re-run update_control for thermal equipment after control
        // dispatch so ThermalSetpoint / ModeOverride / DR signals take effect
        // on the current timestep, not one step later.
        for &idx in &self.roster.equipment_execution_order {
            if self.equipment[idx].descriptor().stage == ExecutionStage::Thermal {
                let _ = self.equipment[idx].update_control(&self.latest_env);
            }
        }

        #[cfg(feature = "profiling")]
        phase_clock.enter(&mut self.profiling, ProfilePhase::Equipment);

        // Step 3: unified equipment step in stage_rank order
        // (Independent → Electrical → Thermal).
        //
        // Independent equipment (PV, generators) steps first, depositing
        // generation data to ports. BMS actors re-evaluate based on actual
        // PV generation. Electrical equipment (battery, EV) steps next
        // with the revised signals. Thermal equipment (HVAC, water heaters,
        // ventilation) steps last, committing heating/cooling power with
        // full visibility of same-step PV generation and storage dispatch.
        //
        // This replaces the prior two-pass structure (Step 3a thermal-first,
        // Step 3b non-thermal-second) that reversed the stage_rank dependency
        // chain and caused a one-step lag for PV-informed HVAC dispatch.
        //
        // The PV-BMS re-evaluation pass is gated behind a timestep threshold
        // (default: ≥ 5 minutes) because 1-minute timesteps have <1%/s PV
        // ramp rates where the one-step PV lag has negligible impact.
        #[cfg(feature = "observe")]
        let mut thermal_obs: Vec<EquipmentObservation> = Vec::new();
        #[cfg(feature = "observe")]
        let mut nonthermal_obs: Vec<EquipmentObservation> = Vec::new();
        #[cfg(feature = "observe")]
        let mut pre_snapshot = if observing {
            Some(self.ports.clone())
        } else {
            None
        };

        // Runtime stage-rank tracking: records the actual step execution
        // sequence so the invariant checker validates real loop ordering,
        // not just the static sort of equipment_execution_order.
        let mut stepped_stage_ranks: Vec<u8> =
            Vec::with_capacity(self.roster.equipment_execution_order.len());

        // Phase 1: Independent-stage equipment (PV, generators).
        for _oi in 0..self.roster.equipment_execution_order.len() {
            let idx = self.roster.equipment_execution_order[_oi];
            let stage = self.equipment[idx].descriptor().stage;
            if stage != ExecutionStage::Independent {
                continue;
            }
            #[cfg(feature = "observe")]
            let pre_ports = pre_snapshot.as_ref().map(observer_capture::capture_ports);

            let _ = self.equipment[idx].update_control(&self.latest_env);
            self.rollback_ports.copy_into(&self.ports);
            // Snapshot the shared electrical bus so the post-step delta is
            // exactly this equipment's contribution (ElectricalAccumulator
            // is Copy).
            let pre_electrical = self.ports.electrical;
            if let Err(err) = self.equipment[idx].step(&self.latest_env, dt, &mut self.ports) {
                self.rollback_failed_equipment_ports(idx, &err);
            } else {
                validate_core_contract(
                    self.equipment[idx].descriptor(),
                    self.equipment[idx].core_output(),
                )?;
                validate_port_core_electrical_consistency(
                    self.equipment[idx].descriptor(),
                    self.equipment[idx].core_output(),
                    pre_electrical,
                    &self.ports.electrical,
                )?;
                step_succeeded[idx] = true;
                stepped_stage_ranks.push(stage_rank(stage));
            }

            #[cfg(feature = "observe")]
            {
                tracing::debug!(
                    stage = ?stage,
                    equipment = %self.equipment[idx].descriptor().name,
                    "equipment step executed"
                );
                if let Some(ref mut snapshot) = pre_snapshot {
                    let contribution = observer_capture::diff_ports(snapshot, &self.ports);
                    nonthermal_obs.push(observer_capture::capture_single_equipment(
                        self.equipment[idx].as_ref(),
                        contribution,
                        pre_ports.expect("pre_ports is Some when pre_snapshot is Some"),
                    ));
                    *snapshot = self.ports.clone();
                }
            }
        }

        // PV-BMS re-evaluation: after Independent-stage equipment has
        // deposited generation data to ports, re-evaluate BMS actors
        // with actual (not prior-step) PV generation so that battery
        // charge power setpoints reflect same-step PV availability.
        {
            let time_step_secs = self.latest_env.time_step_secs();
            if time_step_secs >= PV_RE_EVAL_MIN_STEP_SECS {
                let pv_kw = -power_w_to_kw(self.ports.electrical.generation_power_w);
                self.actor_dispatch_buf.clear();
                for actor in &mut self.actors {
                    let emitted_from = self.actor_dispatch_buf.len();
                    actor.adjust_for_pv(pv_kw, &self.latest_env, &mut self.actor_dispatch_buf);
                    crate::actor::check_declared_signals(
                        actor.as_ref(),
                        &self.actor_dispatch_buf[emitted_from..],
                    )?;
                }
                if !self.actor_dispatch_buf.is_empty() {
                    for req in self.actor_dispatch_buf.drain(..) {
                        self.control_dispatcher.queue(req);
                    }
                    self.control_dispatcher.dispatch_into(
                        &mut self.equipment,
                        &mut self.warnings,
                        &mut self.health.rejected_control_signals,
                    );
                }
            }
        }

        // Phase 2: Electrical-stage equipment (Battery, EV).
        for _oi in 0..self.roster.equipment_execution_order.len() {
            let idx = self.roster.equipment_execution_order[_oi];
            let stage = self.equipment[idx].descriptor().stage;
            if stage != ExecutionStage::Electrical {
                continue;
            }
            #[cfg(feature = "observe")]
            let pre_ports = pre_snapshot.as_ref().map(observer_capture::capture_ports);

            let _ = self.equipment[idx].update_control(&self.latest_env);
            self.rollback_ports.copy_into(&self.ports);
            // Snapshot the shared electrical bus so the post-step delta is
            // exactly this equipment's contribution.
            let pre_electrical = self.ports.electrical;
            if let Err(err) = self.equipment[idx].step(&self.latest_env, dt, &mut self.ports) {
                self.rollback_failed_equipment_ports(idx, &err);
            } else {
                validate_core_contract(
                    self.equipment[idx].descriptor(),
                    self.equipment[idx].core_output(),
                )?;
                validate_port_core_electrical_consistency(
                    self.equipment[idx].descriptor(),
                    self.equipment[idx].core_output(),
                    pre_electrical,
                    &self.ports.electrical,
                )?;
                step_succeeded[idx] = true;
                stepped_stage_ranks.push(stage_rank(stage));
            }

            #[cfg(feature = "observe")]
            {
                tracing::debug!(
                    stage = ?stage,
                    equipment = %self.equipment[idx].descriptor().name,
                    "equipment step executed"
                );
                if let Some(ref mut snapshot) = pre_snapshot {
                    let contribution = observer_capture::diff_ports(snapshot, &self.ports);
                    nonthermal_obs.push(observer_capture::capture_single_equipment(
                        self.equipment[idx].as_ref(),
                        contribution,
                        pre_ports.expect("pre_ports is Some when pre_snapshot is Some"),
                    ));
                    *snapshot = self.ports.clone();
                }
            }
        }

        // Phase 3: Thermal-stage equipment (HVAC, water heaters, ventilation).
        // update_control() already ran in Step 2a after control dispatch, so
        // only .step() is called here. Thermal equipment commits heating/cooling
        // power to ports with full visibility of same-step PV generation and
        // storage dispatch from Phases 1–2.
        for _oi in 0..self.roster.equipment_execution_order.len() {
            let idx = self.roster.equipment_execution_order[_oi];
            if self.equipment[idx].descriptor().stage != ExecutionStage::Thermal {
                continue;
            }
            #[cfg(feature = "observe")]
            let pre_ports = pre_snapshot.as_ref().map(observer_capture::capture_ports);

            self.rollback_ports.copy_into(&self.ports);
            // Snapshot the shared electrical bus so the post-step delta is
            // exactly this equipment's contribution.
            let pre_electrical = self.ports.electrical;
            if let Err(err) = self.equipment[idx].step(&self.latest_env, dt, &mut self.ports) {
                self.rollback_failed_equipment_ports(idx, &err);
            } else {
                validate_core_contract(
                    self.equipment[idx].descriptor(),
                    self.equipment[idx].core_output(),
                )?;
                validate_port_core_electrical_consistency(
                    self.equipment[idx].descriptor(),
                    self.equipment[idx].core_output(),
                    pre_electrical,
                    &self.ports.electrical,
                )?;
                step_succeeded[idx] = true;
                stepped_stage_ranks.push(stage_rank(ExecutionStage::Thermal));
            }

            #[cfg(feature = "observe")]
            {
                tracing::debug!(
                    stage = ?ExecutionStage::Thermal,
                    equipment = %self.equipment[idx].descriptor().name,
                    "equipment step executed"
                );
                if let Some(ref mut snapshot) = pre_snapshot {
                    let contribution = observer_capture::diff_ports(snapshot, &self.ports);
                    thermal_obs.push(observer_capture::capture_single_equipment(
                        self.equipment[idx].as_ref(),
                        contribution,
                        pre_ports.expect("pre_ports is Some when pre_snapshot is Some"),
                    ));
                    *snapshot = self.ports.clone();
                }
            }
        }

        // Observer captures: nonthermal first (Independent + Electrical ran
        // first), then thermal (Thermal ran last). This reverses the prior
        // capture order to match the new execution order.
        #[cfg(feature = "observe")]
        if observing {
            obs_phases.post_nonthermal_equipment = Some(observer_capture::capture_equipment_phase(
                nonthermal_obs,
                &self.ports,
            ));
            obs_phases.post_thermal_equipment = Some(observer_capture::capture_equipment_phase(
                thermal_obs,
                &self.ports,
            ));
        }

        // Stage-ordering invariant: verify that equipment step execution order
        // respects stage_rank ordering (non-decreasing ranks).
        self.invariant_checker
            .check_equipment_step_order(&stepped_stage_ranks)?;

        // Non-finite telemetry enforcement (always on): equipment
        // writes are rejected at the Telemetry boundary in every build, and
        // the first rejection is latched; ANY latched map (failed-and-
        // rolled-back equipment included: the port rollback does not undo
        // the telemetry map, and the corruption that failed the step is
        // exactly what produced the write) turns the step into a run
        // failure naming the equipment and the key.
        for eq in &self.equipment {
            if let Some((key, value)) = eq.telemetry().non_finite_latch() {
                return Err(HaresError::NanDetected {
                    step_index: self.clock.current_step(),
                    zone_id: None,
                    value_name: format!(
                        "telemetry key '{key}' on equipment '{}' (rejected value {value})",
                        eq.descriptor().name
                    ),
                });
            }
        }

        // Unknown telemetry keys (always on): a `set` on a key the
        // equipment never registered is a wiring fault. The first one is
        // latched by the telemetry map, and ANY latch (failed-and-rolled-
        // back equipment included, mirroring the non-finite check above)
        // turns the step into a run failure naming the equipment and the
        // key.
        for eq in &self.equipment {
            if let Some(key) = eq.telemetry().unknown_key_latch() {
                return Err(HaresError::Equipment(format!(
                    "telemetry key '{key}' was never registered on equipment \
                     '{}' (register telemetry keys via insert() at init)",
                    eq.descriptor().name
                )));
            }
        }

        // Curve-index clamps: once per timestep, after the equipment step
        // loops complete, drain each equipment's health counters into the
        // run total. Failed-and-rolled-back equipment are skipped for the
        // counters: take-and-discard would lose nothing (their counts
        // surface on the equipment's next successful step, take semantics
        // reset per take), and their step already failed the port contract.
        let mut step_curve_index_clamps: u64 = 0;
        for (idx, eq) in self.equipment.iter_mut().enumerate() {
            if step_succeeded[idx] {
                step_curve_index_clamps += eq.take_health_counts().curve_index_clamps;
            }
            // Warnings raised this step drain beside the health counts,
            // from failed-and-rolled-back equipment too (warnings only, no
            // health counts): their warnings would otherwise sit undrained
            // until the equipment's next successful step, stamped with the
            // later step, or be lost entirely if the equipment never
            // succeeds. The drain happens in the step the warnings fired
            // in, so the stamp the dwelling adds is the step they belong
            // to.
            eq.drain_warnings(&mut self.warning_scratch);
            for mut warning in self.warning_scratch.drain(..) {
                warning.step_index = Some(self.clock.current_step);
                self.warnings.push_warning(warning);
            }
        }
        self.health.curve_index_clamps += step_curve_index_clamps;

        // Step 3c: propagate per-timestep ventilation recovery effectiveness
        // from equipment to the thermal solver config.  The Ventilation equipment
        // computes effective sensible/latent effectiveness accounting for bypass
        // and defrost derating; the thermal solver's infiltration module must see
        // these dynamic values (not the static rated ones) to correctly compute
        // the ventilation sensible/latent load.
        //
        // EnergyPlus Engineering Reference, "Heat Exchangers" chapter:
        // per-timestep effectiveness application with bypass suspending heat
        // transfer (effectiveness = 0).  Using stale rated values during bypass
        // underestimates the ventilation load by a factor equal to
        // 1 / (1 - rated_eff), e.g. 4× for a 75% effective HRV.
        for eq in &self.equipment {
            if let Some((eff_s, eff_l)) = eq.effective_ventilation_effectiveness() {
                self.thermal_solver
                    .set_ventilation_recovery(eff_s, eff_l)
                    .map_err(|err| {
                        HaresError::Envelope(format!(
                            "ventilation recovery from '{}' rejected: {err}",
                            eq.descriptor().name
                        ))
                    })?;
                #[cfg(feature = "observe")]
                tracing::debug!(
                    equipment = %eq.descriptor().name,
                    mode = ?eq.core_output().state.operating_mode,
                    eff_s,
                    eff_l,
                    "ventilation effectiveness propagated to thermal solver config"
                );
            }
        }

        // End of the `equipment` phase (Steps 3 and 3c).
        #[cfg(feature = "profiling")]
        phase_clock.enter(&mut self.profiling, ProfilePhase::Envelope);

        // Step 4: envelope/domain resolution.
        self.thermal_solver.integrate(
            &self.ports,
            &self.latest_env,
            &mut self.thermal_update_buf,
        )?;

        // Apply zone temps and capture observer data while we still have the borrow.
        apply_thermal_update_to_zones(&mut self.latest_env, &self.thermal_update_buf);

        // Upsert thermal domain so humidity/electrical solvers see updated state.
        // Zone temperatures are already applied above; the upsert stores the full
        // DomainUpdate in custom_domains for solvers that read it (humidity).
        #[cfg(feature = "observe")]
        let thermal_for_observer = self.thermal_update_buf.clone();
        self.latest_env.upsert_domain_ref(&self.thermal_update_buf);

        // Capture the pre-resolve (committed) humidity ratios so the
        // always-on moisture balance check can diff this step's solver
        // output against them (recorded in every build profile).
        self.prev_humidity_ratios.clear();
        self.prev_humidity_ratios.extend(
            self.humidity_solver
                .humidity_ratios
                .iter()
                .map(|(&z, &w)| (z, w)),
        );

        self.humidity_solver.resolve(
            &self.ports,
            &self.latest_env,
            dt,
            &mut self.humidity_update_buf,
        )?;
        self.electrical_solver.resolve(
            &self.ports,
            &self.latest_env,
            dt,
            &mut self.electrical_update_buf,
        )?;
        self.fluid_solver.resolve(
            &self.ports,
            &self.latest_env,
            dt,
            &mut self.fluid_update_buf,
        )?;

        apply_humidity_update_to_zones(&mut self.latest_env, &self.humidity_update_buf);

        // BMS PV staleness diagnostic: after the electrical solver runs at
        // Step 4 and the timestep is ≥ 5 min, compare the PV generation used
        // in the BMS decision (bms_pv_stale_kw) against the actual PV
        // generation from the current step (bms_pv_actual_kw). If the
        // re-evaluation pass ran, both telemetry fields are populated and
        // the difference is the stale-data error from the one-step ordering.
        {
            if self.latest_env.time_step_secs() >= PV_RE_EVAL_MIN_STEP_SECS {
                for actor in &self.actors {
                    let Some(tel) = actor.telemetry() else {
                        continue;
                    };
                    let stale = tel.get("bms_pv_stale_kw").unwrap_or(0.0);
                    let actual = tel.get("bms_pv_actual_kw").unwrap_or(0.0);
                    let diff = (actual - stale).abs();
                    // Emit diagnostic when the stale-data error exceeds a
                    // configurable fraction of the larger PV value (default 5%).
                    let max_pv = stale.max(actual).max(0.01); // avoid div-by-zero
                    if diff > 0.05 * max_pv {
                        tracing::debug!(
                            actor = actor.name(),
                            bms_pv_stale_kw = stale,
                            bms_pv_actual_kw = actual,
                            diff_kw = diff,
                            pct = diff / max_pv * 100.0,
                            "BMS PV staleness: step-ordering data gap exceeds 5% of PV",
                        );
                    }
                }
            }
        }

        #[cfg(feature = "observe")]
        if self.observer_buf.is_some() {
            let zip_scale = self
                .electrical_solver
                .effective_load_scale(&self.latest_env.grid);
            let port_load_raw_w = self.ports.electrical.load_power_w;
            let port_load_adj_w = port_load_raw_w * zip_scale;
            let port_net_w = port_load_adj_w + self.ports.electrical.generation_power_w;
            let port_net_kw = power_w_to_kw(port_net_w);
            let port_load_raw_kw = power_w_to_kw(port_load_raw_w);
            let port_load_adj_kw = power_w_to_kw(port_load_adj_w);
            let residual = (self.electrical_solver.net_active_kw() - port_net_kw).abs();
            obs_phases.post_solvers = Some(observer_capture::capture_solvers(
                &thermal_for_observer,
                &self.humidity_update_buf,
                &self.electrical_update_buf,
                &self.fluid_update_buf,
                &self.thermal_solver,
                observer_capture::PortBalance {
                    zip_load_scale: zip_scale,
                    port_load_raw_kw,
                    port_load_adjusted_kw: port_load_adj_kw,
                    residual_kw: residual,
                },
            ));
        }

        // Step 4 diagnostic: capture per-timestep diagnostics for CSV output.
        if let Some(ref mut writer) = self.diagnostic_writer {
            let gains = self.thermal_solver.component_gains();
            let envelope = EnvelopeDiag {
                window_solar_w: gains.window_solar_w,
                opaque_solar_lwr_w: gains.opaque_solar_lwr_w,
                interior_lwr_w: gains.interior_lwr_w,
                internal_gain_w: gains.internal_gain_w,
                port_convective_w: gains.port_convective_w,
                port_radiant_w: gains.port_radiant_w,
                air_density_kg_m3: gains.air_density_kg_m3,
                ghi_w_m2: self.latest_env.weather.ghi_w_m2,
            };
            let step = self.clock.current_step();
            let diag = diagnostics::capture(step, &self.latest_env, &self.ports, Some(envelope));
            let n_zones = self.latest_env.zones.len();
            diagnostics::write_row(writer, &diag, n_zones);
        }

        self.latest_env.upsert_domain_ref(&self.humidity_update_buf);
        self.latest_env
            .upsert_domain_ref(&self.electrical_update_buf);
        self.latest_env.upsert_domain_ref(&self.fluid_update_buf);

        // Ensure custom update buffers match the number of custom solvers.
        while self.custom_update_bufs.len() < self.custom_domain_solvers.len() {
            self.custom_update_bufs
                .push(hares_types::DomainUpdate::empty(hares_types::DomainId(0)));
        }
        for (i, solver) in self.custom_domain_solvers.iter_mut().enumerate() {
            solver.resolve(
                &self.ports,
                &self.latest_env,
                dt,
                &mut self.custom_update_bufs[i],
            )?;
            self.latest_env
                .upsert_domain_ref(&self.custom_update_bufs[i]);
        }

        #[cfg(feature = "observe")]
        if self.observer_buf.is_some() {
            let capture = observer_capture::capture_custom_solvers(&self.custom_domain_solvers);
            debug_assert_eq!(
                capture.solvers.len(),
                self.custom_domain_solvers.len(),
                "custom solver capture count mismatch"
            );
            obs_phases.post_custom_solvers = Some(capture);
        }

        // End of the `envelope` phase (Step 4 and its diagnostic capture);
        // the always-on invariant checks and check_step_invariants follow.
        #[cfg(feature = "profiling")]
        phase_clock.enter(&mut self.profiling, ProfilePhase::Invariants);

        // Always-on invariant checks — unconditional in all build configurations.
        // These catch unrecoverable data corruption (NaN/Inf) in the two domains
        // where silent propagation would corrupt output records and downstream
        // metrics. O(n_zones + O(1)) per step — cheap enough to run every timestep.

        // Electrical finiteness: NaN or Inf in the electrical solver output is
        // unrecoverable data corruption. The check mirrors the one inside the
        // the always-on check_step_invariants() so that plain release builds
        // still catch it.
        {
            let net_kw = self.electrical_solver.net_active_kw();
            if !net_kw.is_finite() {
                tracing::error!(
                    electrical_net_kw = net_kw,
                    "electrical solver output is non-finite — quarantining dwelling"
                );
                return Err(HaresError::InvariantViolation {
                    check_name: "electrical_net_finite".to_string(),
                    value: net_kw,
                    tolerance: 0.0,
                });
            }
            let net_kvar = self.electrical_solver.net_reactive_kvar();
            if !net_kvar.is_finite() {
                tracing::error!(
                    electrical_net_kvar = net_kvar,
                    "electrical solver reactive output is non-finite — quarantining dwelling"
                );
                return Err(HaresError::InvariantViolation {
                    check_name: "electrical_net_finite".to_string(),
                    value: net_kvar,
                    tolerance: 0.0,
                });
            }

            // Islanded-imbalance accounting (honest accounting, not fake
            // physics): while islanded there is no service connection, so any
            // residual net flow the electrical solver reports at the "grid"
            // channel is a power-balance violation inside the island —
            // positive = load the island sources failed to cover (would-be
            // phantom import), negative = surplus generation the island could
            // not absorb (would-be phantom export). Both are 0.0 during
            // normal (grid-connected) operation. See docs/outage-behavior.md.
            if self.latest_env.grid.islanded() {
                self.island_unserved_kw = net_kw.max(0.0);
                self.island_excess_kw = (-net_kw).max(0.0);
                if self.island_unserved_kw > ISLAND_UNSERVED_WARN_THRESHOLD_KW {
                    // Rate-limited: warn! once per process, debug! thereafter
                    // — unserved load recurs every islanded step, and one
                    // warning is enough to flag the scenario.
                    if ISLAND_UNSERVED_WARNED
                        .compare_exchange(
                            false,
                            true,
                            std::sync::atomic::Ordering::Relaxed,
                            std::sync::atomic::Ordering::Relaxed,
                        )
                        .is_ok()
                    {
                        tracing::warn!(
                            step = self.clock.current_step(),
                            island_unserved_kw = self.island_unserved_kw,
                            "islanded operation: on-site sources are not covering the load \
                             (unserved load; subsequent occurrences logged at debug level)"
                        );
                    } else {
                        tracing::debug!(
                            step = self.clock.current_step(),
                            island_unserved_kw = self.island_unserved_kw,
                            "islanded operation: unserved load"
                        );
                    }
                }
            } else {
                self.island_unserved_kw = 0.0;
                self.island_excess_kw = 0.0;
            }
        }

        // Zone temperature NaN: silently propagating a NaN zone temperature
        // corrupts output recording, equipment control, and downstream metrics
        // for the remainder of the simulation. EVERY zone is checked,
        // warm-up included: there is no warm-up tolerance, because the run
        // must fail loudly instead of propagating a corrupted initial state;
        // and every non-finite zone is logged before quarantining.
        {
            let mut any_nan = false;
            for zone in &self.latest_env.zones {
                if !zone.temperature_c.is_finite() {
                    any_nan = true;
                    tracing::error!(
                        zone_id = %zone.id,
                        temperature_c = zone.temperature_c,
                        "zone temperature is NaN; quarantining dwelling"
                    );
                }
            }
            if any_nan {
                return Err(HaresError::InvariantViolation {
                    check_name: "zone_temperature_nan".to_string(),
                    value: f64::NAN,
                    tolerance: 0.0,
                });
            }
        }

        self.check_step_invariants(dt)?;

        // End of the `invariants` phase; the per-step telemetry snapshot
        // (`state_snapshot` phase) follows.
        #[cfg(feature = "profiling")]
        phase_clock.enter(&mut self.profiling, ProfilePhase::StateSnapshot);

        // Push observer step snapshot after invariant checks so moisture invariant
        // capture data (populated in check_step_invariants) is available for the snapshot.
        #[cfg(feature = "observe")]
        if let Some(buf) = &mut self.observer_buf {
            obs_phases.post_zone_update =
                Some(observer_capture::capture_zone_update(&self.latest_env));
            #[cfg(all(feature = "observe", debug_assertions))]
            let moisture_invariant = if self.invariant_moisture_capture.is_empty() {
                None
            } else {
                Some(MoistureInvariantCapture {
                    zones: std::mem::take(&mut self.invariant_moisture_capture),
                })
            };
            #[cfg(not(all(feature = "observe", debug_assertions)))]
            let moisture_invariant: Option<MoistureInvariantCapture> = None;
            buf.push(StepSnapshot {
                step_index: self.clock.current_step(),
                timestamp: self.latest_env.current_time,
                phases: obs_phases,
                actor_skips,
                actor_calls,
                actor_error_count,
                moisture_invariant,
                time_sin: {
                    let fhour = self.latest_env.current_time.hour() as f64
                        + self.latest_env.current_time.minute() as f64 / 60.0
                        + self.latest_env.current_time.second() as f64 / 3600.0;
                    (2.0 * std::f64::consts::PI * fhour / 24.0).sin()
                },
                time_cos: {
                    let fhour = self.latest_env.current_time.hour() as f64
                        + self.latest_env.current_time.minute() as f64 / 60.0
                        + self.latest_env.current_time.second() as f64 / 3600.0;
                    (2.0 * std::f64::consts::PI * fhour / 24.0).cos()
                },
                actor_telemetry_keys: self
                    .actors
                    .iter()
                    .filter_map(|actor| {
                        actor.telemetry().map(|tel| {
                            let mut keys: Vec<String> = tel.0.keys().cloned().collect();
                            keys.sort();
                            (actor.name().to_string(), keys)
                        })
                    })
                    .collect(),
            });
        }

        // Record per-step diagnostic data for post-hoc checks (unmet hours,
        // short-cycling, freezing, simultaneous heating/cooling).  Runs when
        // the observer is active so check data is available for end-of-run
        // analysis.
        #[cfg(feature = "observe")]
        if let Some(accum) = &mut self.diagnostic_accum {
            accum.record_from_state(&self.latest_env.zones, &self.equipment);
        }

        // Equipment-mode interest baseline: capture the snapshot generation
        // the actors just observed — the map as it stands BEFORE this step's
        // end-of-step population overwrites it — so the next step's
        // `EquipmentModeChange` comparison sees two distinct generations
        // (current = this step's snapshot, prev = the one before). The
        // previous shape captured AFTER the population, storing the new
        // snapshot itself: the comparison then read the map against a copy
        // of itself and the interest never fired on any real transition —
        // a registered interest whose trigger never fires is the "silence
        // is not a decision" failure in its purest form. The zone-temperature
        // interests keep the same one-generation-behind contract via the
        // `prior_zone_temps`/`prev_zone_temps` shift register below;
        // equipment modes need only the single previous generation, so a
        // pre-population capture is the whole mechanism. Mid-run adds still
        // fire on first observation: the identity refresh seeds the joining
        // equipment's `equipment_core` entry between steps, after this
        // capture, so the joining step's comparison reads `Some(mode)`
        // against a `prev` that lacks the id.
        self.prev_equipment_modes.clear();
        self.prev_equipment_modes.extend(
            self.latest_env
                .equipment_core
                .iter()
                .map(|(&id, co)| (id, co.state.operating_mode)),
        );

        // Snapshot end-of-timestep equipment state into latest_env so that the
        // NEXT step's actors see the freshest committed state for every
        // equipment that stepped this timestep. Snapshot runs AFTER all
        // equipment phases (Independent + Electrical + Thermal in Step 3) so
        // nothing is one step stale.
        let active_equipment_ids: HashSet<EquipmentId> = self
            .equipment
            .iter()
            .map(|eq| {
                let desc = eq.descriptor();
                self.equipment_id_by_name
                    .get(&desc.name)
                    .copied()
                    .expect("invariant: equipment_id_by_name is built from this equipment set")
            })
            .collect();
        self.latest_env
            .equipment_core
            .retain(|id, _| active_equipment_ids.contains(id));
        self.latest_env.equipment_core.reserve(self.equipment.len());
        self.latest_env.equipment_telemetry.retain(|name, _| {
            name == hares_types::telemetry_keys::HUMIDITY_SOLVER_TELEMETRY_KEY
                || self.equipment_id_by_name.contains_key(name)
        });
        self.latest_env
            .equipment_telemetry
            .reserve(self.equipment.len());
        for (idx, eq) in self.equipment.iter().enumerate() {
            if !step_succeeded[idx] {
                continue;
            }
            let desc = eq.descriptor();
            let id = self
                .equipment_id_by_name
                .get(&desc.name)
                .copied()
                .expect("invariant: equipment_id_by_name is built from this equipment set");
            // Identity desync invariant — see `snapshot_equipment_state`.
            debug_assert_eq!(
                id, desc.id,
                "equipment '{}' id drifted from its registered identity",
                desc.name
            );
            self.latest_env
                .equipment_core
                .insert(id, eq.core_output().clone());

            // Per-equipment telemetry snapshot for next-step actor reads.
            // Use `clone_from` on the existing entry so steady-state telemetry
            // keys reuse their f64 slots without reallocating the inner map.
            let telemetry = eq.telemetry();
            match self.latest_env.equipment_telemetry.get_mut(&desc.name) {
                Some(existing) => existing.clone_from(telemetry),
                None => {
                    self.latest_env
                        .equipment_telemetry
                        .insert(desc.name.clone(), telemetry.clone());
                }
            }
        }

        for (i, entry) in self.roster.zone_temp_scratch.iter_mut().enumerate() {
            entry.1 = if let Some(env_idx) = self.roster.zones.zone_env_indices[i] {
                self.latest_env.zones[env_idx].temperature_c
            } else {
                // Zone not found in environment -- should not happen in a correctly
                // built dwelling. Use previous value (initialized to 0.0 at
                // construction, updated each step when the zone is present).
                entry.1
            };
        }

        // HVAC thermal delivery: use component_gains which includes both equipment
        // port contributions and ideal HVAC loads from the thermal solver.
        // Sign convention (EnvelopeComponentGains): positive = heat into zone.
        // cooling_w is negative when cooling (heat removed); .abs() converts to
        // delivered-energy magnitude for the StepResult/public API.
        let gains = self.thermal_solver.component_gains();
        let hvac_heating_w = gains.hvac_heating_w;
        let hvac_cooling_w = gains.hvac_cooling_w.abs();
        // Stash the step's HVAC delivery (delivered-energy magnitudes) for
        // the warm-up loop's daily peaks; see `step_hvac_heating_w`.
        self.step_hvac_heating_w = hvac_heating_w;
        self.step_hvac_cooling_w = hvac_cooling_w;
        if hvac_heating_w < 0.0 {
            tracing::warn!(
                hvac_heating_w = hvac_heating_w,
                "negative HVAC heating delivered — sign error in equipment \
                 port contributions or thermal solver gain inversion"
            );
        }

        let gas_power_w = self.ports.fuel.get(hares_types::FuelType::Gas);

        let step_result = StepResult {
            timestamp: self.latest_env.current_time,
            net_electric_power_kw: self.electrical_solver.net_active_kw(),
            zone_temperatures_c: self.roster.zone_temp_scratch.clone(),
            hvac_heating_w,
            hvac_cooling_w,
            gas_power_w,
        };

        // End of the `state_snapshot` phase (observer push, diagnostic
        // accumulation, end-of-step equipment snapshot, step result).
        #[cfg(feature = "profiling")]
        phase_clock.enter(&mut self.profiling, ProfilePhase::Output);

        // Step 5: record outputs to disk (when enabled) and accumulate step results.
        if record_output {
            self.record_step(&step_result)?;
        }
        // End of the `output` phase (Step 5).
        #[cfg(feature = "profiling")]
        phase_clock.enter(&mut self.profiling, ProfilePhase::Accounting);
        #[cfg(feature = "profiling")]
        {
            let alloc_after = thread_allocations();
            if let (Some(before), Some(after)) = (alloc_before, alloc_after) {
                // The counter only ever grows, so the delta is the step's
                // allocation count. Without the counting allocator both
                // reads are `None` and no field is touched. A violating
                // step is counted and reported, never asserted: steps
                // allocate today.
                let delta = after.saturating_sub(before);
                self.profiling.hot_path_allocations =
                    Some(self.profiling.hot_path_allocations.unwrap_or(0) + delta);
                self.profiling.hot_path_alloc_violations = Some(
                    self.profiling.hot_path_alloc_violations.unwrap_or(0) + u64::from(delta > 0),
                );
            }
        }

        // Post-solver: accumulate billing state from metered power.
        if let Some(ref mut evaluator) = self.tariff_evaluator {
            let net_kw = self.electrical_solver.net_active_kw();
            let dt_secs = self.latest_env.time_step_secs();
            let tz = evaluator.simulation_start().timezone();
            let current_tz = self.latest_env.current_time.with_timezone(&tz);
            if let Some(summary) = evaluator.step(net_kw, 0.0, dt_secs, current_tz) {
                self.billing_summaries.push(summary);
            }
        }

        // Capture electrical summary for next step's EnvironmentState.
        let mut battery_kw = 0.0;
        let mut ev_kw = 0.0;
        let mut pv_kw = 0.0;
        for eq in &self.equipment {
            let end_use = &eq.descriptor().end_use;
            let co = eq.core_output();
            if *end_use == EndUse::BATTERY {
                battery_kw += co.flows.electric_kw.map_or(0.0, |e| e.signed_kw());
            } else if *end_use == EndUse::EV {
                ev_kw += co.flows.electric_kw.map_or(0.0, |e| e.signed_kw());
            } else if *end_use == EndUse::PV {
                pv_kw += co.flows.electric_kw.map_or(0.0, |e| e.signed_kw());
            }
        }
        let net_grid = self.electrical_solver.net_active_kw();
        // The electrical summary observation is checked once here, where it
        // is built (the consumers' per-decide PV re-checks were deleted):
        // non-finite is covered by the solver's unconditional screens above,
        // and a negative PV observation (PV equipment drawing power) is an
        // invariant violation.
        if !(-pv_kw).is_finite() || (-pv_kw) < 0.0 {
            return Err(HaresError::InvariantViolation {
                check_name: "electrical_summary_pv_observation".to_string(),
                value: -pv_kw,
                tolerance: 0.0,
            });
        }
        self.prior_electrical_summary = ElectricalSummary {
            pv_generation_kw: -pv_kw,
            actual_pv_kw: -pv_kw,
            // Charge-only subtraction for battery and EV alike: discharge
            // power lands in generation_power_w, never in load_power_w, so
            // subtracting a negative (discharging) value here would inflate
            // the non-dispatchable base load.
            base_load_kw: power_w_to_kw(self.ports.electrical.load_power_w)
                - battery_kw.max(0.0)
                - ev_kw.max(0.0),
            net_grid_kw: net_grid,
            battery_power_kw: battery_kw,
            ev_power_kw: ev_kw,
        };

        // Update previous-step state for interest filtering on the next step.
        // Shift-register: prior ← prev (swap, zero alloc), then prev ← current
        // step's zones (clear + re-insert, zero alloc — reuses existing capacity).
        std::mem::swap(&mut self.prior_zone_temps, &mut self.prev_zone_temps);
        self.prev_zone_temps.clear();
        for zone in &self.latest_env.zones {
            self.prev_zone_temps.insert(zone.id, zone.temperature_c);
        }
        self.prev_price_signal = self.latest_env.price_signal.clone();

        // ORDERING: ports.zero() must come AFTER check_step_invariants() (called above)
        // because the electrical balance check reads self.ports.electrical.load_power_w
        // and generation_power_w to compute the ZIP-adjusted port net.
        // Physics checks raised from run_timestep in every build profile: the
        // per-equipment electric sum against the solver total, and the
        // per-zone HVAC port totals against the equipment thermal_output_w
        // sums (over the wiring pre-resolved at assembly). Both sums read
        // core_output, which a failed-and-rolled-back equipment retains from
        // its last successful step ("tolerated step failures retain the last
        // committed entry"): the rollback removed its port contributions, so
        // the sums must skip it too. Its stale power must not be counted
        // against the solver total, which no longer carries it.
        {
            let total = self.electrical_solver.net_active_kw();
            let sum_equip: f64 = self
                .equipment
                .iter()
                .enumerate()
                .filter(|(idx, _)| step_succeeded[*idx])
                .map(|(_, eq)| {
                    eq.core_output()
                        .flows
                        .electric_kw
                        .map_or(0.0, |e| e.net_consumption_kw())
                })
                .sum();
            DwellingTelemetry::verify_consistency(self.clock.current_step(), sum_equip, total)?;
            self.verify_per_zone_thermal_consistency(self.clock.current_step(), &step_succeeded)?;
        }
        self.ports.zero();
        let _ = self.clock.next();

        self.simulation_results.steps.push(step_result);

        // Invariant: the dwelling RNG must have been consumed during this
        // timestep.  A zero-delta means run_timestep never called
        // advance_dwelling_rng, which would make checkpoint captures
        // reflect a dead RNG state. Debug-build logic check (the advance
        // call above is unconditional and cannot skip).
        #[cfg(debug_assertions)]
        debug_assert!(
            rng_consumed_this_step,
            "dwelling RNG was not consumed during timestep"
        );

        // Observe the dwelling RNG state for per-step auditability.
        #[cfg(feature = "observe")]
        tracing::debug!(
            rng_stream = self.rng.get_stream(),
            rng_word_pos = self.rng.get_word_pos(),
            "dwelling RNG state at end of timestep"
        );

        // Closes the `accounting` phase (billing, electrical summary, clock
        // advance) and records `step_total` from entry to return.
        #[cfg(feature = "profiling")]
        phase_clock.finish(&mut self.profiling);

        Ok(())
    }

    fn record_step(&mut self, step: &StepResult) -> Result<()> {
        self.roster.record_scratch.fill(0.0);
        let row = &mut self.roster.record_scratch;

        if let Some(&idx) = self
            .roster
            .output_column_index
            .get("Total Electric Power (kW)")
        {
            row[idx] = step.net_electric_power_kw;
        }
        if let Some(&idx) = self
            .roster
            .output_column_index
            .get("Total Gas Power (therms/hour)")
        {
            let gas_w = self.ports.fuel.get(hares_types::FuelType::Gas);
            row[idx] = gas_w / GAS_THERMS_PER_HOUR_TO_W;
        }
        if let Some(&idx) = self
            .roster
            .output_column_index
            .get("Total Reactive Power (kVAR)")
        {
            row[idx] = self.electrical_solver.net_reactive_kvar();
        }

        // Per-equipment columns via pre-resolved index map.
        for (eq, cols) in self.equipment.iter().zip(&self.roster.equipment_column_map) {
            let co = eq.core_output();
            #[cfg(debug_assertions)]
            {
                let desc = eq.descriptor();
                let name = &desc.name;
                let v = self.output_verbosity;
                let is_hvac = is_hvac_or_wh(name);
                let is_cooling = is_cooling_equipment(name);
                let is_hp_heater = is_heat_pump_heater(name);
                // The type-gated verbosity-7 invariants demand PER-EQUIPMENT
                // telemetry columns, which exist only for equipment the
                // schema was built from. A mid-run add after the schema froze
                // is schema-unknown (empty column map by design — missing
                // values, never misattributed ones), so demanding its
                // per-equipment columns here would panic the debug build for a
                // documented contract, not a drift. The one exception is the
                // global `HVAC Duct Losses (W)` aggregate below: it resolves
                // for ANY equipment (every equipment's duct telemetry
                // accumulates into one slot), so its demand stays ungated.
                let schema_known =
                    is_schema_known_equipment(&self.roster.output_column_index, name);
                // Telemetry-based columns: assert index is Some when expected.
                if schema_known && is_hp_heater && v >= 7 {
                    debug_assert!(cols.defrost_state.is_some());
                    debug_assert!(cols.er_power.is_some());
                }
                if schema_known && is_cooling && v >= 7 {
                    debug_assert!(cols.shr.is_some());
                    debug_assert!(cols.latent_gains.is_some());
                }
                if schema_known && is_hvac && v >= 7 {
                    debug_assert!(cols.fan_power.is_some());
                    debug_assert!(cols.runtime_fraction.is_some());
                }
                if v >= 5 {
                    debug_assert!(cols.duct_losses.is_some());
                }
            }
            if let Some(idx) = cols.electric_power {
                row[idx] = co.flows.electric_kw.map_or(0.0, |e| e.net_consumption_kw());
            }
            if let Some(idx) = cols.gas_power {
                row[idx] =
                    co.flows.fuel_w.map_or(0.0, |f| f.consumption_w) / GAS_THERMS_PER_HOUR_TO_W;
            }
            if let Some(idx) = cols.mode {
                row[idx] = co.state.operating_mode.map_or(0.0, |m| m.as_code());
            }
            if let Some(idx) = cols.setpoint {
                row[idx] = co.state.setpoint_c.unwrap_or(0.0);
            }
            if let Some(idx) = cols.soc {
                row[idx] = co.state.soc.map_or(0.0, |soc| soc.get());
            }
            if let Some(idx) = cols.capacity {
                // Capacity column is always a positive magnitude (OCHRE HVAC.py:598:
                // `capacity_list` is asserted strictly positive). thermal_output_w uses
                // the signed convention (negative for cooling), so take abs().
                let capacity_w = co
                    .flows
                    .thermal_output_w
                    .map(f64::abs)
                    .or(co.flows.sensible_cooling_w.map(f64::abs))
                    .unwrap_or(0.0);
                row[idx] = capacity_w;
            }
            if let Some(idx) = cols.cop {
                row[idx] = co.performance.cop.unwrap_or(0.0);
            }
            if let Some(idx) = cols.reactive_power {
                let q = co.flows.reactive_power_kvar.unwrap_or(0.0);
                row[idx] = q;
                if let Some(pf_idx) = cols.power_factor {
                    let p = co.flows.electric_kw.map_or(0.0, |e| e.signed_kw());
                    let s = (p * p + q * q).sqrt();
                    row[pf_idx] = if s > 1e-9 { (p / s).abs() } else { 1.0 };
                }
            }
            if let Some(idx) = cols.energy_kwh {
                let dt_hours = self.latest_env.time_step_secs() / SECONDS_PER_HOUR;
                let electric_kw = co.flows.electric_kw.map_or(0.0, |e| e.net_consumption_kw());
                row[idx] = electric_kw * dt_hours;
            }
            if let Some(idx) = cols.schedule {
                let is_active = co
                    .state
                    .operating_mode
                    .is_some_and(|m| m != hares_types::OperatingMode::Off);
                row[idx] = if is_active { 1.0 } else { 0.0 };
            }
            if let Some(idx) = cols.defrost_state {
                row[idx] = eq.telemetry().get(tk::DEFROST_CYCLE_STATE).unwrap_or(0.0); // allowed: defrost cycle state remains telemetry-only until CoreOutput gains a defrost_state field.
            }
            if let Some(idx) = cols.er_power {
                // OCHRE HVAC.py:1464-1467: ER Power = er_capacity * er_eir_rated * space_fraction / 1000.
                // Maps to BACKUP_ER_KW (already space-fraction-adjusted in heater step).
                row[idx] = eq.telemetry().get(tk::BACKUP_ER_KW).unwrap_or(0.0); // allowed: ER power remains telemetry-only until CoreOutput gains an er_power field.
            }
            // Per-equipment HVAC performance columns at v7.
            // Some read from CoreOutput (setpoint, capacity, COP, speed, main_power),
            // the rest remain telemetry-only until their fields are promoted.
            if let Some(idx) = cols.shr {
                row[idx] = eq.telemetry().get(tk::SHR).unwrap_or(0.0); // allowed: SHR remains telemetry-only until CoreOutput gains an SHR field.
            }
            if let Some(idx) = cols.speed {
                row[idx] = co.state.speed_index.map_or(0.0, |s| s as f64);
            }
            if let Some(idx) = cols.fan_power {
                row[idx] = eq.telemetry().get(tk::FAN_KW).unwrap_or(0.0); // allowed: fan power remains telemetry-only until CoreOutput gains a fan power field.
            }
            if let Some(idx) = cols.main_power {
                row[idx] = co.performance.main_power_kw.unwrap_or(0.0);
            }
            if let Some(idx) = cols.runtime_fraction {
                row[idx] = eq.telemetry().get(tk::RUNTIME_FRACTION).unwrap_or(0.0); // allowed: runtime fraction remains telemetry-only until CoreOutput gains an RTF field.
            }
            if let Some(idx) = cols.latent_gains {
                // OCHRE HVAC.py:595: Latent Gains = latent_gain * space_fraction (pre-DSE).
                row[idx] = eq.telemetry().get(tk::LATENT_GAINS_W).unwrap_or(0.0); // allowed: latent gains remains telemetry-only until CoreOutput gains a latent gains field.
            }
            if let Some(idx) = cols.duct_losses {
                // Aggregate column: sum duct_loss_w across all HVAC equipment.
                // Only the first equipment's index is used; all HVAC contributions
                // are accumulated into the same row slot.
                row[idx] += eq.telemetry().get(tk::DUCT_LOSS_W).unwrap_or(0.0); // allowed: duct loss remains telemetry-only until CoreOutput gains a duct loss field.
            }
            // V8 per-equipment telemetry diagnostic columns: each (key, idx) pair
            // reads directly from equipment telemetry.
            for &(key, idx) in &cols.v8_columns {
                row[idx] = eq.telemetry().get(key).unwrap_or(0.0); // allowed: v8 telemetry pass-through uses pre-resolved tk:: constants bound to `key`.
            }
        }

        // Per-EndUse aggregate electric power columns.
        // Each equipment's electric power is accumulated into the aggregate column
        // for its EndUse category (e.g. all HVAC_HEATING equipment contribute to
        // "HVAC Heating End Use Electric Power (kW)").
        for (eq, &agg_idx_opt) in self
            .equipment
            .iter()
            .zip(&self.roster.end_use_aggregate_indices)
        {
            if let Some(idx) = agg_idx_opt {
                let co = eq.core_output();
                row[idx] += co.flows.electric_kw.map_or(0.0, |e| e.net_consumption_kw());
            }
        }

        // Actor telemetry columns: pre-resolved column indices avoid
        // per-timestep format!() allocation (parallel to equipment_column_map).
        for (actor, pre_resolved) in self.actors.iter().zip(&self.roster.actor_column_map) {
            if let Some(tel) = actor.telemetry() {
                for (key, column_idx) in pre_resolved {
                    if let Some(value) = tel.get(key) {
                        row[*column_idx] = value;
                    }
                }
            }
        }

        // Zone temperature columns -- direct index lookup, no allocations.
        for (i, &(_, temp_c)) in self.roster.zone_temp_scratch.iter().enumerate() {
            if let Some(idx) = self.roster.zones.zone_temp_col_indices[i] {
                row[idx] = temp_c;
            }
        }
        // Fallback for old-style column name
        if let Some(&idx) = self
            .roster
            .output_column_index
            .get("Indoor Temperature (C)")
            && let Some(&(_, temp_c)) = self.roster.zone_temp_scratch.first()
        {
            row[idx] = temp_c;
        }

        if let Some(&idx) = self.roster.output_column_index.get("Outdoor Dry Bulb (C)") {
            row[idx] = self.latest_env.weather.outdoor_temp_c;
        }

        // Envelope, boundary, and HVAC thermal gains from the thermal solver.
        let gains = self.thermal_solver.component_gains();
        // Net sensible = direct heat injections on zone air, matching OCHRE's
        // "Net Sensible Heat Gain - {zone} (W)" (zone heat input: occupancy,
        // HVAC, equipment, infiltration, ventilation, window solar). Opaque
        // envelope conduction reaches the zone through the A-matrix surface
        // coupling rather than a direct injection, so it is reported per
        // boundary below instead of here. The gross exterior solar + LWR
        // absorption (`opaque_solar_lwr_w`) is a boundary condition on the
        // exterior RC nodes, not a zone input, and is excluded entirely.
        let net_sensible_indoor_w = gains.window_solar_w
            + gains.infiltration_w
            + gains.ventilation_w
            + gains.natural_ventilation_w
            + gains.internal_gain_w
            + gains.jacket_loss_w
            + gains.duct_loss_w
            // interior_lwr_w is excluded: it reports Σ|q_i|/2 (gross exchange
            // activity), not a net gain. By conservation, Σ q_i ≈ 0, so the
            // net contribution was always ~0 anyway.
            + gains.hvac_heating_w.max(0.0)
            + gains.hvac_cooling_w;
        let envelope_cols: &[(&str, f64)] = &[
            ("Window Transmitted Solar Gain (W)", gains.window_solar_w),
            ("Infiltration Heat Gain - Indoor (W)", gains.infiltration_w),
            (
                "Forced Ventilation Heat Gain - Indoor (W)",
                gains.ventilation_w,
            ),
            (
                "Natural Ventilation Heat Gain - Indoor (W)",
                gains.natural_ventilation_w,
            ),
            #[cfg(feature = "observe")]
            ("Natural Ventilation Cw (-)", gains.natural_ventilation_cw),
            #[cfg(feature = "observe")]
            (
                "Natural Ventilation Wind Angle (deg)",
                gains.natural_ventilation_wind_angle_deg,
            ),
            #[cfg(feature = "observe")]
            (
                "Natural Ventilation Q_Stack (m\u{b3}/s)",
                gains.natural_ventilation_q_stack_m3_s,
            ),
            #[cfg(feature = "observe")]
            (
                "Natural Ventilation Q_Wind (m\u{b3}/s)",
                gains.natural_ventilation_q_wind_m3_s,
            ),
            #[cfg(feature = "observe")]
            ("Natural Ventilation Cd", gains.natural_ventilation_cd_used),
            ("Net Sensible Heat Gain - Indoor (W)", net_sensible_indoor_w),
            ("Internal Heat Gain - Indoor (W)", gains.internal_gain_w),
            ("Interior LWR Exchange - Indoor (W)", gains.interior_lwr_w),
            // Net conduction from the opaque envelope into the zone (wall +
            // floor + roof interior surfaces) — the inside-face convection
            // term of the zone air heat balance (EnergyPlus ERM 26.1 —
            // "Inside Heat Balance": Interior Convection), aggregated from
            // the three per-boundary columns below. The gross exterior
            // solar + LWR absorption is an outside-face balance driver
            // (EnergyPlus ERM 26.1 — "Outside Surface Heat Balance"): most
            // of it re-leaves via exterior convection and sky exchange, so
            // it is not a load on the conditioned zone.
            (
                "Opaque Surface Heat Gain - Indoor (W)",
                gains.wall_heat_gain_w + gains.floor_heat_gain_w + gains.roof_heat_gain_w,
            ),
            ("Duct Loss Heat Gain - Indoor (W)", gains.duct_loss_w),
            ("Roof Heat Gain - Indoor (W)", gains.roof_heat_gain_w),
            ("Floor Heat Gain - Indoor (W)", gains.floor_heat_gain_w),
            ("Wall Heat Gain - Indoor (W)", gains.wall_heat_gain_w),
            ("Window Heat Gain - Indoor (W)", gains.window_heat_gain_w),
            (
                "Internal Mass Heat Gain - Indoor (W)",
                gains.internal_mass_heat_gain_w,
            ),
            ("HVAC Heating Delivered (W)", gains.hvac_heating_w),
            ("HVAC Cooling Delivered (W)", gains.hvac_cooling_w.abs()),
            // Complete zone air heat-balance residual (E+ Output:Diagnostics
            // analogue): C_zone·ΔT/dt − Σ(all gain terms). Small in a correct
            // model; O(kW) values mean a mis-wired or mislabeled gain — the
            // I-02 defect class made visible in every output file.
            (
                "Zone Air Heat Balance Residual (W)",
                gains.zone_air_balance_residual_w,
            ),
        ];
        for &(col_name, value) in envelope_cols {
            if let Some(&idx) = self.roster.output_column_index.get(col_name) {
                row[idx] = value;
            }
        }

        if let Some(&idx) = self
            .roster
            .output_column_index
            .get("Temperature - Ground (C)")
        {
            row[idx] = self.latest_env.weather.ground_temp_c;
        }
        if let Some(&idx) = self
            .roster
            .output_column_index
            .get("Hot Water Mains Temperature (C)")
        {
            row[idx] = self.latest_env.weather.mains_temp_c;
        }

        // Setpoint chain: dwelling-level context columns for control auditing.
        // Read from the first HVAC equipment that publishes these keys; setpoint
        // reconciliation writes them to all HVAC equipment telemetry, so any
        // one carries the correct value.
        if let Some(first_hvac) = self
            .equipment
            .iter()
            .find(|eq| is_hvac_or_wh(&eq.descriptor().name))
        {
            if let Some(&idx) = self
                .roster
                .output_column_index
                .get(SCHEDULED_HEATING_SETPOINT_COL)
            {
                // allowed: setpoint telemetry keys are populated by HVAC equipment
                // reconcile logic after the first timestep. Before that, get() returns
                // None — 0.0 is the safe pre-init sentinel for output-column display.
                row[idx] = first_hvac
                    .telemetry()
                    .get(tk::SCHEDULE_HEATING_SETPOINT_C)
                    .unwrap_or(0.0);
            }
            if let Some(&idx) = self
                .roster
                .output_column_index
                .get(SCHEDULED_COOLING_SETPOINT_COL)
            {
                // allowed: see SCHEDULED_HEATING_SETPOINT_COL comment above.
                row[idx] = first_hvac
                    .telemetry()
                    .get(tk::SCHEDULE_COOLING_SETPOINT_C)
                    .unwrap_or(0.0);
            }
            if let Some(&idx) = self
                .roster
                .output_column_index
                .get(RUNTIME_HEATING_SETPOINT_COL)
            {
                // allowed: see SCHEDULED_HEATING_SETPOINT_COL comment above.
                row[idx] = first_hvac
                    .telemetry()
                    .get(tk::RUNTIME_HEATING_SETPOINT_C)
                    .unwrap_or(0.0);
            }
            if let Some(&idx) = self
                .roster
                .output_column_index
                .get(RUNTIME_COOLING_SETPOINT_COL)
            {
                // allowed: see SCHEDULED_HEATING_SETPOINT_COL comment above.
                row[idx] = first_hvac
                    .telemetry()
                    .get(tk::RUNTIME_COOLING_SETPOINT_C)
                    .unwrap_or(0.0);
            }
        }

        for &(zone, value) in &gains.infiltration_by_zone {
            if let Some(&idx) = self.roster.zones.infiltration_columns.get(&zone) {
                row[idx] = value;
            }
        }

        for &(zone, value) in &gains.interior_lwr_by_zone {
            if let Some(&idx) = self.roster.zones.lwr_columns.get(&zone) {
                row[idx] = value;
            }
        }

        // Per-zone HVAC thermal attribution: read sensible gains by category
        // from the thermal port accumulators. Ports still hold equipment-step
        // values because ports.zero() runs AFTER record_step.
        // The basement zone receives HvacHeating/HvacCooling directly
        // (not merged into conditioned); duct zones are tagged DuctLoss which
        // is reported in the separate "HVAC Duct Losses (W)" column at
        // verbosity 5, not here.
        for thermal in &self.ports.thermal {
            if let Some(&(heat_idx, cool_idx)) = self.roster.zones.hvac_columns.get(&thermal.zone) {
                row[heat_idx] = thermal.sensible_for_category(ThermalCategory::HvacHeating);
                row[cool_idx] = thermal
                    .sensible_for_category(ThermalCategory::HvacCooling)
                    .abs();
            }
        }

        self.timestamp_buf.clear();
        use std::fmt::Write;
        write!(&mut self.timestamp_buf, "{}", step.timestamp.format("%+"))
            .expect("write to String is infallible");
        if let Some(recorder) = self.recorder.as_mut() {
            recorder
                .push_row(&self.timestamp_buf, row)
                .map_err(|err| HaresError::Io(format!("record push failed: {err}")))?;
        }
        Ok(())
    }

    /// Runs per-timestep invariant checks.
    ///
    /// Compiled and executed in every build profile, warm-up included.
    /// Returns `Err(HaresError::InvariantViolation { .. })` on the first violation;
    /// the engine then quarantines this dwelling rather than propagating a panic.
    fn check_step_invariants(&mut self, dt: StdDuration) -> Result<()> {
        let checker = &mut self.invariant_checker;

        let dt_s = dt.as_secs_f64();
        if !dt_s.is_finite() || dt_s <= 0.0 {
            return Err(HaresError::InvariantViolation {
                check_name: "timestep_dt".to_string(),
                value: dt_s,
                tolerance: 0.0,
            });
        }

        // Zone temperature bounds (read from already-updated zones).
        // Split by conditioning status: conditioned zones get tighter bounds
        // (80 °C) while unconditioned zones (attics under solar load) get
        // wider bounds (120 °C).
        {
            debug_assert_eq!(
                self.latest_env.zones.len(),
                self.zone_is_conditioned.len(),
                "zone_is_conditioned length must match latest_env.zones length"
            );
            self.invariant_conditioned_temps.clear();
            self.invariant_unconditioned_temps.clear();
            for (zone, &is_cond) in self
                .latest_env
                .zones
                .iter()
                .zip(self.zone_is_conditioned.iter())
            {
                if is_cond {
                    self.invariant_conditioned_temps.push(zone.temperature_c);
                } else {
                    self.invariant_unconditioned_temps.push(zone.temperature_c);
                }
            }

            // Tank node temperatures from all water heater equipment.
            self.invariant_tank_temps.clear();
            for eq in &self.equipment {
                if eq.descriptor().end_use != EndUse::WATER_HEATING {
                    continue;
                }
                let telem = eq.telemetry();
                for key in &self.tank_node_keys {
                    match telem.get(key) {
                        // allowed: tank node keys are pre-computed at init.
                        Some(t) => self.invariant_tank_temps.push(t),
                        None => break,
                    }
                }
            }
            checker.check_temperatures(
                &self.invariant_conditioned_temps,
                &self.invariant_unconditioned_temps,
                &self.invariant_tank_temps,
            )?;
        }

        // SOC bounds for storage equipment.
        for eq in &self.equipment {
            let end_use = &eq.descriptor().end_use;
            if *end_use != EndUse::BATTERY && *end_use != EndUse::EV {
                continue;
            }
            // Loud on absence: every real Battery and EV publishes SOC in
            // its core output at every step (the publish sites null the
            // channel only when the internal SOC is non-finite or outside
            // [0, 1] — itself a defect), so an absent SOC is a wiring or
            // state defect, not a value condition — a silent skip would
            // blind the bounds check to exactly the observation failure
            // it exists to catch (the same monitor rule the
            // ev_capacity_degraded gate below enforces for its keys).
            // (`check_soc` itself was deleted: `Soc::try_from` already
            // bounds the value the channel publishes.)
            if eq.core_output().state.soc.is_none() {
                return Err(HaresError::Dwelling(format!(
                    "soc_bounds: equipment '{}' published no core-output SOC \
                     (every real Battery and EV publishes SOC at every step; \
                     absence is a wiring or state defect)",
                    eq.descriptor().name
                )));
            }
            // EV usable capacity must track its degraded SOH: the runtime SOC
            // divisor (`capacity_kwh`) must equal rated · (1 − capacity_fade).
            // These are static EV telemetry keys populated at init and every
            // step; reading them here keeps the check DAG-clean (hares-core
            // never reaches into hares-equipment internals). `capacity_fade_pct`
            // is a percentage.
            if *end_use == EndUse::EV {
                let telem = eq.telemetry();
                // allowed: capacity_kwh is a static EV telemetry key set at init.
                let current = telem.get(tk::CAPACITY_KWH);
                // allowed: capacity_kwh_rated is a static EV telemetry key set at init.
                let rated = telem.get(tk::CAPACITY_KWH_RATED);
                // allowed: capacity_fade_pct is a static EV telemetry key set at init.
                let fade_pct = telem.get(tk::CAPACITY_FADE_PCT);
                // allowed: battery_temp_c is a static EV telemetry key set at init.
                let pack_temp_c = telem.get(tk::BATTERY_TEMP_C);
                let (current, rated, fade_pct, temp_c) =
                    match (current, rated, fade_pct, pack_temp_c) {
                        (Some(c), Some(r), Some(f), Some(t)) => (c, r, f, t),
                        _ => {
                            // Loud on absence: these are static keys published at
                            // init and every step, so a missing key is a wiring
                            // defect, not a value condition — a silent skip would
                            // blind this check to exactly the misbinding it
                            // exists to catch (a monitor must detect the absence
                            // of expected input, not merely anomalous values).
                            let missing = [
                                (tk::CAPACITY_KWH, current),
                                (tk::CAPACITY_KWH_RATED, rated),
                                (tk::CAPACITY_FADE_PCT, fade_pct),
                                (tk::BATTERY_TEMP_C, pack_temp_c),
                            ]
                            .into_iter()
                            .find(|(_, v)| v.is_none());
                            return Err(HaresError::Dwelling(format!(
                                "ev_capacity_degraded: EV equipment '{}' is missing \
                                 static telemetry key '{}' (published at init and \
                                 every step; absence is a wiring defect)",
                                eq.descriptor().name,
                                missing.map(|(key, _)| key).unwrap_or("<unknown>")
                            )));
                        }
                    };
                checker.check_ev_capacity_degraded(current, rated, fade_pct / 100.0, temp_c)?;
            }
        }

        // Electrical balance: solver net must match ZIP-adjusted port accumulation.
        // The solver applies ZIP load scaling (`net_active_kw() = P_load·scale + P_gen`).
        // Adjust the port-side load accumulation by the same scale factor for a
        // like-for-like comparison; without this, a non-default ZIP model at non-nominal
        // voltage produces a false-positive residual of P_load·(scale − 1).
        // (Non-finiteness of net_kw/net_kvar is already enforced unconditionally
        // by the electrical_net_finite block earlier in run_timestep, so no
        // redundant NaN screen here.)
        let net_kw = self.electrical_solver.net_active_kw();
        let scale = self
            .electrical_solver
            .effective_load_scale(&self.latest_env.grid);
        let port_net = power_w_to_kw(self.ports.electrical.load_power_w) * scale
            + power_w_to_kw(self.ports.electrical.generation_power_w);
        checker.check_electrical(net_kw, &[-port_net])?;

        // Reactive balance: solver net reactive must match port accumulation.
        // The solver passes slot Q through directly (no voltage scaling), so
        // the comparison is a simple difference against the port-side signed
        // reactive sum.
        let net_kvar = self.electrical_solver.net_reactive_kvar();
        checker.check_reactive(net_kvar, self.ports.electrical.reactive_power_kvar)?;

        // Fuel accumulator must not contain electric contributions: electric
        // power routes through ElectricalAccumulator, never through the fuel
        // accumulator.
        checker.check_fuel_electric_absent(self.ports.fuel.get(hares_types::FuelType::Electric))?;

        // Verify that every non-zero fuel accumulator slot has observer coverage
        // — catches desynchronisation between ALL_FUEL_TYPES and fuel_index that
        // would silently drop fuel contributions from diagnostic output.
        checker.check_fuel_coverage(&self.ports.fuel)?;

        // Thermal balance: full-system energy conservation check.
        //
        // The thermal solver computes three independently-verified energy
        // balance terms during integrate_inner using the same affine step
        // operator as the production path:
        //   q_gains = [Σ C_i·(G·u)[i]/dt, Σ C_i·h_i/dt] — external + coupling [W]
        //   delta_E = Σ C_i·(T_next_i − T_prev_i)/dt — stored rate [W]
        //   q_loss  = −Σ C_i·((F−I)·x_prev)[i]/dt — envelope conduction [W]
        //
        // By construction these satisfy Σ q_gains − delta_E − q_loss ≡ 0
        // (to machine precision) for any correctly-functioning solver, with
        // or without interior convection coupling.
        //
        // Note: when node_capacitances is empty (e.g., simple test fixtures
        // that use R-value-only walls without discrete RC layers), q_gains will
        // also be empty and the thermal check is silently skipped. An RC model
        // with no registered capacitances cannot be energy-balanced — this
        // guard prevents a false positive on an empty system.
        let (q_gains, delta_e_storage, q_loss) = self.thermal_solver.thermal_balance_terms();
        if !q_gains.is_empty() {
            #[cfg(any(test, debug_assertions))]
            {
                if self.test_thermal_invariant_failure {
                    // Test seam: verify thermal invariant wiring by injecting
                    // deliberately broken terms so that the check always fails.
                    // A -1e9 W gain with zero storage and zero loss ensures the
                    // residual exceeds check_thermal's tolerance and produces
                    // InvariantViolation { check_name: "thermal_balance" }.
                    checker.check_thermal(&[-1e9], 0.0, 0.0)?;
                } else {
                    checker.check_thermal(q_gains, delta_e_storage, q_loss)?;
                }
            }
            #[cfg(not(any(test, debug_assertions)))]
            {
                checker.check_thermal(q_gains, delta_e_storage, q_loss)?;
            }
        }

        // HVAC delivered-energy non-negativity: per-step and cumulative checks.
        // These fire before the historical clamping `max(0.0)` / `abs()` masked
        // sign errors in equipment port contributions or thermal solver gains.
        // Warm-up included: no warm-up tolerance is granted.
        {
            let indoor_zone = self.thermal_solver.config().indoor_zone_id;
            let gains = self.thermal_solver.component_gains();
            let step = self.clock.current_step();
            let dt_h = dt_s / SECONDS_PER_HOUR;
            let (heating_w, cooling_w) = {
                #[cfg(any(test, debug_assertions))]
                {
                    if self.test_hvac_negative_energy_failure {
                        // Test seam: inject deliberately negative delivered-energy
                        // values so the invariant checks always produce
                        // NegativeDeliveredEnergy.
                        self.test_hvac_negative_energy_failure = false;
                        (-1000.0, -500.0)
                    } else {
                        (gains.hvac_heating_w, gains.hvac_cooling_w)
                    }
                }
                #[cfg(not(any(test, debug_assertions)))]
                {
                    (gains.hvac_heating_w, gains.hvac_cooling_w)
                }
            };
            checker.check_heating_accumulator(step, indoor_zone, heating_w, cooling_w, dt_h)?;
            checker.check_hvac_power_non_negative(step, indoor_zone, heating_w, cooling_w)?;
        }

        // Moisture balance: mass conservation across the humidity solver.
        // Warm-up included: no warm-up tolerance is granted.
        if let Some(update) = self
            .latest_env
            .custom_domains
            .iter()
            .find(|u| u.domain_id == hares_types::HUMIDITY)
            && let Some(payload) = &update.custom_payload
        {
            for &value in payload {
                if !value.is_finite() {
                    return Err(HaresError::InvariantViolation {
                        check_name: "humidity_payload_finite".to_string(),
                        value,
                        tolerance: 0.0,
                    });
                }
            }
        }
        let p_pa = self.latest_env.weather.pressure_pa();
        let moisture_mult = self.humidity_solver.config.moisture_buffering_multiplier;
        self.invariant_infiltration_latent.clear();
        self.invariant_infiltration_m_dot.clear();
        self.invariant_infiltration_w_outdoor.clear();
        #[cfg(all(feature = "observe", debug_assertions))]
        self.invariant_moisture_capture.clear();
        if let Some(thermal_update) = self
            .latest_env
            .custom_domains
            .iter()
            .find(|u| u.domain_id == hares_types::THERMAL)
            && let Some(payload) = &thermal_update.custom_payload
        {
            // Thermal custom_payload format: [zone_id, q_latent_w, m_dot_inf_kg_s, w_outdoor,
            // energy_balance_residual_w] per zone. The 5-float format carries moisture
            // coupling data and the per-step energy balance residual for observability.
            for quint in payload.chunks_exact(5) {
                let zone_raw = quint[0];
                let latent = quint[1];
                let m_dot_inf = quint[2];
                let w_outdoor = quint[3];
                // quint[4] (energy_balance_residual_w) is available from the
                // solver's custom_payload for observability; the thermal balance
                // invariant check above uses thermal_balance_terms() instead.
                let _ = quint[4];
                if zone_raw.is_finite() && zone_raw >= 0.0 {
                    let zone_id = ZoneId(zone_raw as u16);
                    self.invariant_infiltration_latent.push((zone_id, latent));
                    if m_dot_inf > 0.0 {
                        self.invariant_infiltration_m_dot.insert(zone_id, m_dot_inf);
                        self.invariant_infiltration_w_outdoor
                            .insert(zone_id, w_outdoor);
                    }
                }
            }
        }
        for zone in &self.latest_env.zones {
            let w_new = self.humidity_solver.humidity_ratio(zone.id);
            let w_old = self
                .prev_humidity_ratios
                .iter()
                .find(|(z, _)| *z == zone.id)
                .map(|(_, w)| *w)
                .unwrap_or(w_new);
            let d_w = w_new - w_old;
            if d_w.abs() < f64::EPSILON {
                continue;
            }
            let rho_air = hares_physics::air_properties::moist_air_density_kg_m3(
                p_pa,
                zone.temperature_c,
                w_old,
            );
            // Actual moisture mass change in the zone air (no extra M multiplication).
            // The solver's dW already incorporates the buffering multiplier, so we
            // do NOT multiply by it again — the old code's delta_m = dW * rho * V * M
            // was the algebraic twin of Q_latent * dt / h_fg and made the check self-referential.
            let actual_delta_kg = d_w * rho_air * zone.volume_m3;
            // Condensation mass: moisture removed from (or added to) the zone air by
            // humidity-ratio clamp enforcement. This mass was silently discarded before
            // T-0180; now it is tracked and included in the moisture inventory so the
            // invariant check accounts for it as a sink rather than skipping clamped zones.
            let condensation_kg = self.humidity_solver.condensation_mass_kg(zone.id);
            let solver_total_kg = actual_delta_kg + condensation_kg;
            // Approximate zone air moisture mass for tolerance scaling.
            let gross_moisture_kg = w_new * rho_air * zone.volume_m3;

            // Independently tracked moisture sources and sinks from equipment ports.
            // The thermal port's latent_gain_w is the primary moisture accounting
            // channel: all equipment that moves moisture (dehumidifiers, ACs with
            // latent cooling, ideal HVAC) writes its moisture effect as a signed
            // latent gain in the thermal port. The humidity port is a supplementary
            // channel used by the solver to bypass the h_fg round-trip, not an
            // additional independent source/sink. Subtracting both would cancel
            // equipment contributions from the independent physical mass.
            let independent_latent_w: f64 = self
                .ports
                .thermal
                .iter()
                .filter(|e| e.zone == zone.id)
                .map(|e| e.latent_gain_w)
                .sum();
            let h_fg = hares_physics::constants::LATENT_HEAT_VAPORISATION_0C_J_KG;

            let latent_from_infiltration: f64 = self
                .invariant_infiltration_latent
                .iter()
                .filter(|(z, _)| *z == zone.id)
                .map(|(_, l)| *l)
                .sum();
            let m_dot_inf = self
                .invariant_infiltration_m_dot
                .get(&zone.id)
                .copied()
                .unwrap_or(0.0);

            // Compute independent physical net moisture mass (without buffering multiplier).
            // All equipment moisture effects are tracked through the thermal port's
            // latent_gain_w, converted to mass via h_fg.
            //
            // For semi-implicit infiltration, the solver's unclamped humidity change is:
            //   w_raw - w_old = dt * L / (h_fg * rho * V * M * (1 + alpha))
            // where L = total latent power and alpha = m_dot * dt / (rho * V * M).
            //
            // The independent physical mass (air + materials, no buffering) is:
            //   independent_physical_kg = (w_raw - w_old) * rho * V * M = dt * L / (h_fg * (1 + alpha))
            //
            // The expected air moisture change (after buffering) is:
            //   expected_balance_kg = dt * L / (h_fg * M * (1 + alpha))
            //
            // For the explicit case (alpha = 0), this simplifies to:
            //   independent_physical_kg = dt * L / h_fg
            //   expected_balance_kg = dt * L / (h_fg * M)
            let total_latent = independent_latent_w + latent_from_infiltration;
            let independent_physical_kg = if m_dot_inf > 0.0 {
                // Semi-implicit infiltration: the (1+alpha) denominator from the solver's
                // implicit coupling. Without it, the explicit approximation diverges from
                // the solver output — especially when condensation reduces w_new below w_sat
                // and the explicit m_dot*(w_outdoor - w_new)*dt expression differs from
                // the solver's implicit w_{n+1} usage.
                let alpha = m_dot_inf * dt_s / (rho_air * zone.volume_m3 * moisture_mult);
                total_latent * dt_s / (h_fg * (1.0 + alpha))
            } else {
                // Explicit (no significant infiltration coupling).
                total_latent * dt_s / h_fg
            };

            // Expected balanced moisture mass: the independent physical mass with
            // the expected buffering multiplier applied. This matches the solver's
            // formula: actual_dW = total_moisture_effect / (rho * V * M).
            // For the semi-implicit case the relationship still holds because all
            // moisture effect terms go through the same effective mass denominator.
            let expected_balance_kg = independent_physical_kg / moisture_mult;

            checker.check_moisture(
                independent_physical_kg,
                expected_balance_kg,
                solver_total_kg,
                gross_moisture_kg,
                dt_s,
            )?;

            #[cfg(all(feature = "observe", debug_assertions))]
            {
                let w_outdoor = self
                    .invariant_infiltration_w_outdoor
                    .get(&zone.id)
                    .copied()
                    .unwrap_or(0.0);
                let (sources_kg, sinks_kg) = if m_dot_inf > 0.0 {
                    // Semi-implicit infiltration: sources include outdoor moisture
                    // brought in by infiltration; sinks remove indoor moisture carried
                    // out by exfiltration plus condensation mass.
                    let src = independent_physical_kg + m_dot_inf * w_outdoor * dt_s;
                    let snk = m_dot_inf * w_new * dt_s + condensation_kg;
                    (src, snk)
                } else {
                    // Explicit path: sources are the raw mass from equipment and
                    // thermal-domain latent; sinks are condensation only.
                    let src = independent_physical_kg;
                    let snk = condensation_kg;
                    (src, snk)
                };
                let sorption_kg = sources_kg - sinks_kg - solver_total_kg;
                self.invariant_moisture_capture.push(MoistureZoneInvariant {
                    zone_id: zone.id,
                    expected_sources_kg: sources_kg,
                    expected_sinks_kg: sinks_kg,
                    solver_delta_kg: actual_delta_kg,
                    sorption_residual_kg: sorption_kg,
                    condensation_kg,
                });
            }
        }
        Ok(())
    }
}

/// Maps a ZoneId to its display name for output column labels.
/// The configured indoor zone = "Indoor", attic zones = "Attic", others = "Zone_{id}".
fn zone_display_name(
    zone: ZoneId,
    indoor_zone: ZoneId,
    zone_type: Option<&hares_io::hpxml::ZoneType>,
) -> String {
    if zone == indoor_zone {
        "Indoor".to_string()
    } else if matches!(zone_type, Some(hares_io::hpxml::ZoneType::Attic)) {
        "Attic".to_string()
    } else {
        format!("Zone_{}", zone.0)
    }
}

/// Guards against EV driver RNG stream collisions during actor construction.
///
/// Records each `(seed, stream)` pair used for an EV driver actor and panics
/// if the same pair has already been assigned to another actor.  This catches
/// programming errors that would cause two EV drivers to share identical
/// stochastic behaviour — silently correlated streams are a latent data-quality
/// hazard that is nearly impossible to detect from simulation output alone.
#[cfg(any(debug_assertions, test))]
fn check_ev_rng_stream_no_collision(
    seen: &mut HashMap<([u8; 32], u64), String>,
    seed: [u8; 32],
    stream: u64,
    name: &str,
) {
    let key = (seed, stream);
    if let Some(existing) = seen.get(&key) {
        let seed_hex: String = key
            .0
            .iter()
            .take(8)
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join("");
        panic!(
            "EV driver RNG stream collision: actor '{name}' shares \
             seed-stream pair (seed=0x{seed_hex}..., stream={stream}) \
             with actor '{existing}'"
        );
    }
    seen.insert(key, name.to_string());
}

/// The tariff inputs of built-in actor construction.
pub(crate) struct ActorPricing {
    /// Whether the dwelling has a tariff; tariff-dependent strategies fall
    /// back to tariff-free ones without one.
    pub(crate) has_tariff: bool,
    /// Price per step over the horizon, when the tariff provides one.
    pub(crate) price_schedule: Option<Arc<[f64]>>,
    pub(crate) steps_per_day: usize,
}

/// Where built-in actors get their starting state.
pub(crate) struct ActorSeedState<'a> {
    /// The dwelling RNG every EV driver stream derives from.
    pub(crate) rng: &'a ChaCha8Rng,
    /// Offset of the next unused stream in the range reserved for built-in
    /// EV drivers (see [`crate::rng::ev_driver_stream`]).
    pub(crate) next_stream: u64,
    /// Live built-in actors being rebuilt with new prices. An actor built
    /// under one of their names takes over that actor's decision state
    /// and telemetry (an EV driver its RNG stream and position too, using
    /// no new stream), so a rebuild never replays draws or restarts a
    /// timer; see the actors' `take_over`.
    pub(crate) rebuilt: &'a [&'a dyn Actor],
}

/// Build actor instances from equipment seeds.
///
/// Pure function for testability -- takes equipment, existing actors and
/// the tariff inputs, and returns new built-in actors. A seed whose actor
/// name an existing actor holds builds nothing. An actor rebuilt under the
/// name of one of `seed_state.rebuilt` takes over its state, except a
/// battery management actor's price-derived cache, which it recomputes
/// from its own prices. Each EV driver that takes over nothing gets the
/// next unused driver stream.
/// `pub(crate)` so integration tests outside this module can exercise the
/// real seed → actor construction path (e.g. the EV driver's actor/equipment
/// contract tests) rather than re-deriving the arm's wiring by hand.
///
/// # Errors
///
/// The driver stream range is used up, or a rebuilt driver's state cannot
/// be carried over.
pub(crate) fn build_actors_from_seeds(
    equipment: &[Box<dyn Equipment>],
    existing_actors: &[&dyn Actor],
    pricing: ActorPricing,
    equipment_id_by_name: &HashMap<String, EquipmentId>,
    seed_state: &mut ActorSeedState<'_>,
) -> Result<Vec<Box<dyn Actor>>> {
    let ActorPricing {
        has_tariff,
        price_schedule,
        steps_per_day,
    } = pricing;
    let seeds: Vec<(String, ActorSeed)> = equipment
        .iter()
        .filter_map(|eq| {
            eq.actor_seed()
                .map(|seed| (eq.descriptor().name.clone(), seed))
        })
        .collect();

    let existing_names: HashSet<&str> = existing_actors.iter().map(|a| a.name()).collect();

    let mut built_in_actors: Vec<Box<dyn Actor>> = Vec::new();

    #[cfg(any(debug_assertions, test))]
    let mut seen_rng_pairs: HashMap<([u8; 32], u64), String> = HashMap::new();

    for (name, seed) in seeds {
        match seed {
            ActorSeed::Battery {
                mut bms_mode,
                grid_export_rule,
                max_charge_kw,
                max_discharge_kw,
                min_dwell_steps,
            } => {
                let actor_name = format!("BatteryManagementActor:{name}");
                if existing_names.contains(actor_name.as_str()) {
                    continue;
                }

                if !has_tariff && matches!(bms_mode, BmsMode::TimeOfUseOptimization { .. }) {
                    tracing::warn!(
                        equipment = %name,
                        "TimeOfUseOptimization requires tariff; falling back to SelfConsumption"
                    );
                    if let BmsMode::TimeOfUseOptimization {
                        reserve_soc,
                        solar_only_charging,
                        ..
                    } = &bms_mode
                    {
                        bms_mode = BmsMode::SelfConsumption {
                            min_soc: *reserve_soc,
                            max_soc: 1.0,
                            solar_only_charging: *solar_only_charging,
                            surplus_deadband_kw: 0.0,
                        };
                    }
                }

                let actor = BatteryManagementActor::new(
                    &name,
                    BmsParams {
                        bms_mode,
                        grid_export_rule,
                        max_charge_kw,
                        max_discharge_kw,
                        price_schedule: price_schedule.clone(),
                        steps_per_day,
                        min_dwell_steps,
                    },
                );
                let mut actor = actor;
                if let Some(predecessor) =
                    seed_state.rebuilt.iter().find(|a| a.name() == actor_name)
                {
                    actor.take_over(*predecessor)?;
                }
                actor.resolve_equipment_id(equipment_id_by_name);
                built_in_actors.push(Box::new(actor));
            }
            ActorSeed::Ev {
                mut strategy,
                plug_in_policy,
                capacity_kwh,
                max_charge_kw,
                fuel_economy_kwh_per_mi,
            } => {
                let actor_name = format!("EvDriver:{name}");
                if existing_names.contains(actor_name.as_str()) {
                    continue;
                }

                if !has_tariff
                    && matches!(
                        strategy,
                        ChargingStrategy::TouAware { .. } | ChargingStrategy::V2G { .. }
                    )
                {
                    tracing::warn!(
                        equipment = %name,
                        strategy = ?strategy,
                        "tariff-dependent ChargingStrategy requires tariff; falling back to Immediate"
                    );
                    if let ChargingStrategy::TouAware { target_soc, .. } = &strategy {
                        strategy = ChargingStrategy::Immediate {
                            target_soc: *target_soc,
                        };
                    } else {
                        // V2G without tariff: charge to full is the safe default.
                        // V2G's min_soc is a discharge floor, not a charge target;
                        // without price signals the actor cannot decide when to
                        // export, so charging to 100% avoids stranding the driver.
                        strategy = ChargingStrategy::Immediate { target_soc: 1.0 };
                    }
                }

                let predecessor = seed_state.rebuilt.iter().find(|a| a.name() == actor_name);
                let ev_seed = match predecessor {
                    // Replaced below by the predecessor's own RNG state.
                    Some(_) => seed_state.rng.clone(),
                    None => {
                        let stream = ev_driver_stream(seed_state.next_stream)?;
                        let sub = derive_sub_rng(seed_state.rng, stream);
                        #[cfg(any(debug_assertions, test))]
                        check_ev_rng_stream_no_collision(
                            &mut seen_rng_pairs,
                            sub.get_seed(),
                            sub.get_stream(),
                            &actor_name,
                        );
                        seed_state.next_stream += 1;
                        sub
                    }
                };
                let mut actor = EvDriverActor::new(
                    &format!("EvDriver:{name}"),
                    &name,
                    EvDriverParams {
                        strategy,
                        plug_in_policy,
                        daily_drive_miles: ScheduleSource::Constant(30.0),
                        departure_time: ScheduleSource::Constant(480.0),
                        trip_duration: ScheduleSource::Constant(600.0),
                        arrival_time: None,
                        event_day_ratio: 0.8,
                        fuel_economy_kwh_per_mi,
                        capacity_kwh,
                        max_charge_kw,
                        average_speed_mph: 30.0,
                        range_anxiety_miles: 20.0,
                        away_charge_fraction: 0.0,
                        away_charge_power_kw: 6.6,
                    },
                    ev_seed,
                );

                if let Some(ref prices) = price_schedule {
                    actor = actor.with_price_schedule(Arc::clone(prices), steps_per_day);
                }
                if let Some(predecessor) = predecessor {
                    actor.take_over(*predecessor)?;
                }
                actor.resolve_equipment_id(equipment_id_by_name);

                built_in_actors.push(Box::new(actor));
            }
        }
    }

    Ok(built_in_actors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkpoint::DwellingCheckpoint;
    use crate::derive_dwelling_rng;
    use crate::rng::RNG_STREAM_EV_DRIVER_BASE;
    use conversions::{
        build_output_column_index, equipment_config_from_spec, json_value_to_config_value,
    };
    use hares_control::PriorityTier;
    use hares_equipment::config::ConfigValue;
    use hares_equipment::{Equipment, EquipmentConfig};
    use hares_tariff::ElectricTariff;
    use hares_types::ports::{PortContribution, PortSlots};
    use hares_types::{
        ControlCapabilities, ControlSignal, CoreCapabilities, CoreOutput, DRLevel, EndUse,
        EquipmentDescriptor, EquipmentId, ExecutionStage, FluidType, FuelType, LoopId,
        OperatingMode, PortDeclaration, Telemetry, TelemetryField, ZoneId, ZoneState,
    };
    use std::borrow::Cow;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
    use std::time::Duration;
    use syn::spanned::Spanned;
    use syn::visit::Visit;

    fn collect_rs_files(root: &PathBuf, out: &mut Vec<PathBuf>) {
        let read_dir = fs::read_dir(root)
            .unwrap_or_else(|e| panic!("failed to read directory {}: {e}", root.display()));
        for entry in read_dir {
            let entry = entry.unwrap_or_else(|e| {
                panic!(
                    "failed to read directory entry under {}: {e}",
                    root.display()
                )
            });
            let path = entry.path();
            if path.is_dir() {
                collect_rs_files(&path, out);
                continue;
            }
            if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }

    /// Guards the no-string-telemetry-reads contract for `hares-core`'s
    /// production code: telemetry maps are string-keyed, so a production
    /// `.get` on a telemetry channel must carry an explicit `// allowed:`
    /// comment naming why the value is not yet a typed `CoreOutput` field
    /// (the established reason shape: "remains telemetry-only until
    /// CoreOutput gains a <field> field"). A raw read has no static
    /// guarantee the key exists, and every value that crosses the boundary
    /// untyped stays untyped; the per-site comment is the contract that
    /// keeps the escape hatch deliberate and reviewable.
    ///
    /// Detection is AST-based (`syn`), matching the sibling guard tests in
    /// `hares-equipment/tests/*_guard.rs`. This guard previously sanitized
    /// source text and brace-balanced `mod tests` spans by hand, which broke
    /// on any construct the byte scanner did not model: an apostrophe in a
    /// lifetime (`&'static str`) was treated as a char-literal opener and
    /// swallowed the text up to the next apostrophe, unbalancing the file's
    /// braces so the test-module span never resolved — the guard then
    /// reported test helpers as production violations, and the same
    /// swallowing silently hid real production reads. Parsing the file
    /// makes both exclusion and detection structural: lifetimes, char
    /// literals, and formatting cannot perturb them.
    ///
    /// Reach, stated honestly: `syn` sees no tokens inside macro bodies
    /// (`macro_rules!` expansions, `quote!` output), so a banned read that
    /// exists only in macro-generated code is invisible here — the same
    /// limitation the sibling guards carry.
    struct TelemetryReadGuard<'src> {
        /// Path label (relative to the crate root) for violation messages.
        rel: String,
        /// Raw source split into lines for allowlist lookups
        /// (`lines[n - 1]` is line `n`, 1-based like span locations).
        lines: Vec<&'src str>,
        violations: Vec<String>,
    }

    impl<'src> TelemetryReadGuard<'src> {
        fn new(rel: String, raw: &'src str) -> Self {
            Self {
                rel,
                lines: raw.lines().collect(),
                violations: Vec::new(),
            }
        }

        fn line(&self, one_based: usize) -> Option<&'src str> {
            one_based
                .checked_sub(1)
                .and_then(|idx| self.lines.get(idx).copied())
        }

        /// Whether the read starting on `line` carries an explicit
        /// contract: a `// allowed:` comment on the read's own line, on the
        /// line after it (comments inside a `match`/`if` block the read
        /// heads), or in the contiguous `//` lines directly above it
        /// (leading comments on the statement) — the positions the
        /// established allowlist comments occupy.
        fn is_allowlisted(&self, line: usize) -> bool {
            if self
                .line(line)
                .is_some_and(|text| text.contains("// allowed:"))
                || self
                    .line(line + 1)
                    .is_some_and(|text| text.contains("// allowed:"))
            {
                return true;
            }
            let mut above = line.saturating_sub(1);
            while above >= 1 {
                match self.line(above) {
                    Some(text) if text.trim_start().starts_with("//") => {
                        if text.contains("// allowed:") {
                            return true;
                        }
                        above -= 1;
                    }
                    // A non-comment line ends the statement's leading
                    // comments.
                    _ => break,
                }
            }
            false
        }
    }

    /// A telemetry-channel identifier: `telemetry`/`telem`, or a channel
    /// field ending in `_telemetry`/`_telem` (`equipment_telemetry`,
    /// `actor_telemetry`).
    fn is_telemetry_ident(ident: &syn::Ident) -> bool {
        let name = ident.to_string();
        name == "telemetry"
            || name == "telem"
            || name.ends_with("_telemetry")
            || name.ends_with("_telem")
    }

    /// Whether a `.get(..)` receiver is a telemetry channel: a terminal
    /// telemetry identifier (`actor.telemetry`, `env.equipment_telemetry`,
    /// a local `telemetry` binding, a `telemetry()` call) or a tuple-field
    /// `.0` channel. Any other receiver shape is by definition not a
    /// telemetry channel — the negative answer, not a swallowed variant.
    fn receiver_is_telemetry_channel(receiver: &syn::Expr) -> bool {
        match receiver {
            syn::Expr::Field(field) => match &field.member {
                syn::Member::Named(ident) => is_telemetry_ident(ident),
                syn::Member::Unnamed(index) => index.index == 0,
            },
            syn::Expr::MethodCall(call) => is_telemetry_ident(&call.method),
            syn::Expr::Path(path) => path
                .path
                .segments
                .last()
                .is_some_and(|segment| is_telemetry_ident(&segment.ident)),
            syn::Expr::Paren(paren) => receiver_is_telemetry_channel(&paren.expr),
            _ => false,
        }
    }

    /// A `#[cfg(test)]`-family attribute: `cfg(test)`, `cfg(any(test, ..))`,
    /// `cfg(all(test, ..))`. The token-text substring match is the same
    /// heuristic the sibling guards use; it can only err toward exemption
    /// (a configuration literally containing the substring "test" is
    /// misread as test gating), never toward a false violation.
    fn is_cfg_test_attr(attr: &syn::Attribute) -> bool {
        attr.path().is_ident("cfg")
            && matches!(&attr.meta, syn::Meta::List(list) if list.tokens.to_string().contains("test"))
    }

    /// Whether an item is test code: a module named `tests` (the
    /// convention this crate follows even where the attribute is absent),
    /// or any item gated by a `#[cfg(test)]`-family attribute.
    fn item_is_test_code(node: &syn::Item) -> bool {
        let (attrs, is_tests_module) = match node {
            syn::Item::Const(item) => (&item.attrs, false),
            syn::Item::Enum(item) => (&item.attrs, false),
            syn::Item::ExternCrate(item) => (&item.attrs, false),
            syn::Item::Fn(item) => (&item.attrs, false),
            syn::Item::ForeignMod(item) => (&item.attrs, false),
            syn::Item::Impl(item) => (&item.attrs, false),
            syn::Item::Macro(item) => (&item.attrs, false),
            syn::Item::Mod(item) => (&item.attrs, item.ident == "tests"),
            syn::Item::Static(item) => (&item.attrs, false),
            syn::Item::Struct(item) => (&item.attrs, false),
            syn::Item::Trait(item) => (&item.attrs, false),
            syn::Item::TraitAlias(item) => (&item.attrs, false),
            syn::Item::Type(item) => (&item.attrs, false),
            syn::Item::Union(item) => (&item.attrs, false),
            syn::Item::Use(item) => (&item.attrs, false),
            // `syn::Item` is #[non_exhaustive]: unknown or future variants
            // carry no attributes this guard can read, so they can only be
            // treated as non-test-gated — the visitor still descends into
            // them, which errs toward scanning, never toward silently
            // skipping code.
            _ => return false,
        };
        is_tests_module || attrs.iter().any(is_cfg_test_attr)
    }

    /// The same `#[cfg(test)]` gating for items inside `impl` blocks
    /// (e.g. test-helper methods on production types).
    fn impl_item_is_test_code(node: &syn::ImplItem) -> bool {
        let attrs = match node {
            syn::ImplItem::Const(item) => &item.attrs,
            syn::ImplItem::Fn(item) => &item.attrs,
            syn::ImplItem::Macro(item) => &item.attrs,
            syn::ImplItem::Type(item) => &item.attrs,
            // `syn::ImplItem` is #[non_exhaustive]: same rule as
            // `item_is_test_code` — unknown variants are descended into,
            // never silently skipped.
            _ => return false,
        };
        attrs.iter().any(is_cfg_test_attr)
    }

    /// The same `#[cfg(test)]` gating for items inside `trait` definitions.
    fn trait_item_is_test_code(node: &syn::TraitItem) -> bool {
        let attrs = match node {
            syn::TraitItem::Const(item) => &item.attrs,
            syn::TraitItem::Fn(item) => &item.attrs,
            syn::TraitItem::Macro(item) => &item.attrs,
            // `syn::TraitItem` is #[non_exhaustive]: same rule as
            // `item_is_test_code` — unknown variants are descended into,
            // never silently skipped.
            _ => return false,
        };
        attrs.iter().any(is_cfg_test_attr)
    }

    impl<'ast> Visit<'ast> for TelemetryReadGuard<'_> {
        /// Test code is exempt — structurally: modules named `tests`,
        /// `#[cfg(test)]`-gated items, and the same gating on impl/trait
        /// items. The exemption this visitor implements is the mechanism
        /// that previously broke (hand-balanced spans), so it is pinned by
        /// `telemetry_read_guard_flags_and_excludes_by_syntax` below.
        fn visit_item(&mut self, node: &'ast syn::Item) {
            if !item_is_test_code(node) {
                syn::visit::visit_item(self, node);
            }
        }

        fn visit_impl_item(&mut self, node: &'ast syn::ImplItem) {
            if !impl_item_is_test_code(node) {
                syn::visit::visit_impl_item(self, node);
            }
        }

        fn visit_trait_item(&mut self, node: &'ast syn::TraitItem) {
            if !trait_item_is_test_code(node) {
                syn::visit::visit_trait_item(self, node);
            }
        }

        fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
            if node.method == "get" && receiver_is_telemetry_channel(&node.receiver) {
                let start = node.span().start();
                if !self.is_allowlisted(start.line) {
                    let snippet = self.line(start.line).unwrap_or_default().trim();
                    self.violations.push(format!(
                        "{}:{}:{}: telemetry string read in `{}`",
                        self.rel,
                        start.line,
                        start.column + 1,
                        snippet
                    ));
                }
            }
            syn::visit::visit_expr_method_call(self, node);
        }
    }

    #[test]
    fn no_string_telemetry_reads_in_core() {
        let src_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        collect_rs_files(&src_root, &mut files);
        files.sort();
        assert!(
            !files.is_empty(),
            "guard found no source files under {src_root:?} — is it scanning the right tree?"
        );

        let mut hits = Vec::new();
        for path in &files {
            let raw = fs::read_to_string(path)
                .unwrap_or_else(|e| panic!("failed reading {}: {e}", path.display()));
            let parsed = syn::parse_file(&raw)
                .unwrap_or_else(|e| panic!("failed to parse {}: {e}", path.display()));
            let rel = path
                .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                .unwrap_or(path.as_path())
                .display()
                .to_string();
            let mut guard = TelemetryReadGuard::new(rel, &raw);
            guard.visit_file(&parsed);
            hits.extend(guard.violations);
        }

        assert!(
            hits.is_empty(),
            "found disallowed telemetry string reads in core:\n{}",
            hits.join("\n")
        );
    }

    /// The guard must not be vacuous and its test-code exemption must not
    /// be blind. Synthetic source pins every arm: production reads on all
    /// four receiver shapes flag; `other.get`, `telemetry.insert`, and
    /// `telemetry[0].get` do not; `// allowed:` contracts hold on the
    /// read's own line, the next line, and as leading comments; `mod
    /// tests` and `#[cfg(test)]` functions are exempt. The `&'static str`
    /// inside the test module is load-bearing: the hand-rolled scanner
    /// this guard replaced false-positived exactly there (its apostrophe
    /// arm swallowed the text to the next `'`, unbalancing the brace
    /// match, so the test-module span never resolved); parsing makes the
    /// exemption immune to lifetimes and char literals.
    #[test]
    fn telemetry_read_guard_flags_and_excludes_by_syntax() {
        let src = r#"
fn production_reads() {
    let a = telemetry.get("charge_kw");
    let b = eq.telemetry().get(tk::SOC);
    let c = env.equipment_telemetry.get(name);
    let d = channel.0.get("k");
}
fn permitted_shapes() {
    let ok1 = other.get("k");
    telemetry.insert("k", 1.0);
    let ok2 = telemetry[0].get("k");
}
fn allowlisted_same_line() {
    let a = telemetry.get("k"); // allowed: same-line contract.
}
fn allowlisted_next_line() {
    let b = telemetry.get("k");
    // allowed: next-line contract.
}
fn allowlisted_leading_comment() {
    // allowed: leading-comment contract.
    let c = eq
        .telemetry()
        .get("k");
}
mod tests {
    const LABEL: &'static str = "charge_kw";
    fn helper() {
        let t = telemetry.get(LABEL);
    }
}
#[cfg(test)]
fn cfg_gated_helper() {
    let t = telemetry.get("charge_kw");
}
"#;
        let parsed = syn::parse_file(src).expect("synthetic guard source parses");
        let mut guard = TelemetryReadGuard::new("synthetic.rs".to_string(), src);
        guard.visit_file(&parsed);

        assert_eq!(
            guard.violations.len(),
            4,
            "exactly the four production telemetry reads must flag (telemetry.get, \
             telemetry().get, equipment_telemetry.get, .0.get); permitted receiver \
             shapes, allowlisted reads, and test code must not: {:?}",
            guard.violations
        );
    }

    fn equipment_refs(equipment: &[Box<dyn Equipment>]) -> Vec<&dyn Equipment> {
        equipment.iter().map(AsRef::as_ref).collect()
    }

    fn replace_equipment_for_test(dwelling: &mut Dwelling, equipment: Vec<Box<dyn Equipment>>) {
        // Keep the never-reused id counter monotonic across the swap: advance
        // past every entering id (never reuse — same contract as
        // `add_equipment`), but never lower it, so ids issued before the
        // swap cannot be reissued even if the new vector's max id is lower.
        let entering_max = equipment
            .iter()
            .map(|eq| eq.descriptor().id.0)
            .max()
            .unwrap_or(0);
        if let Some(next) = entering_max.checked_add(1) {
            dwelling.next_equipment_id = dwelling.next_equipment_id.max(next);
        }
        dwelling.equipment = equipment;
        dwelling.equipment_id_by_name = dwelling
            .equipment
            .iter()
            .map(|eq| (eq.descriptor().name.clone(), eq.descriptor().id))
            .collect();
        let declarations: Vec<PortDeclaration> = dwelling
            .equipment
            .iter()
            .flat_map(|eq| eq.ports())
            .copied()
            .collect();
        dwelling.ports = PortSlots::from_declarations(&declarations);
        dwelling.rollback_ports = PortSlots::from_declarations(&declarations);
        let equipment = equipment_refs(&dwelling.equipment);
        dwelling.roster.equipment_execution_order = compute_equipment_execution_order(&equipment);
        dwelling.roster.equipment_ids = equipment.iter().map(|eq| eq.descriptor().id).collect();
        dwelling.roster.hvac_thermal_consistency =
            build_hvac_thermal_consistency(&dwelling.ports, &equipment);
        dwelling.roster.equipment_column_map = build_equipment_column_map(
            &equipment,
            &dwelling.roster.output_column_index,
            dwelling.output_verbosity,
        )
        .expect("test helper: test-equipment column map must resolve");
        dwelling
            .solver_feedback_actor
            .set_dispatch_targets(compute_equipment_dispatch_targets(&equipment));
        dwelling.latest_env.equipment_telemetry.retain(|name, _| {
            name == hares_types::telemetry_keys::HUMIDITY_SOLVER_TELEMETRY_KEY
                || dwelling
                    .equipment
                    .iter()
                    .any(|eq| eq.descriptor().name == *name)
        });
        dwelling.latest_env.equipment_core.retain(|id, _| {
            dwelling
                .equipment
                .iter()
                .any(|eq| eq.descriptor().id == *id)
        });
    }

    struct TestEquipment {
        descriptor: EquipmentDescriptor,
        telemetry: Telemetry,
        last_power_kw: f64,
        last_soc_target: f64,
        last_dr_level: Option<DRLevel>,
        core_output: CoreOutput,
        initialized: bool,
    }

    impl TestEquipment {
        fn new(name: &str, capabilities: ControlCapabilities) -> Self {
            Self {
                descriptor: EquipmentDescriptor {
                    // Unassigned sentinel: when this stub joins a dwelling
                    // through `add_equipment`, the dwelling auto-assigns its
                    // never-reused id — the same path every Python-side
                    // `add_*` equipment takes. A hard-coded explicit id
                    // would collide with the dwelling's assembly-assigned
                    // ids (they start at 1).
                    id: EquipmentId(0),
                    name: name.to_string(),
                    end_use: EndUse::OTHER,
                    equipment_type: Cow::Borrowed("TestEquipment"),
                    zone: Some(ZoneId(1)),
                    fuel: FuelType::Electric,
                    stage: ExecutionStage::Independent,
                    control_capabilities: capabilities,
                    core_capabilities: CoreCapabilities::empty(),
                    telemetry_fields: vec![
                        TelemetryField {
                            name: tk::LAST_POWER_KW.to_string(),
                            unit: "kW".to_string(),
                            description: "last applied power".to_string(),
                        },
                        TelemetryField {
                            name: tk::LAST_SOC_TARGET.to_string(),
                            unit: "fraction".to_string(),
                            description: "last applied SOC target".to_string(),
                        },
                        TelemetryField {
                            name: "last_dr_level".to_string(),
                            unit: "enum".to_string(),
                            description: "last applied DR level".to_string(),
                        },
                    ],
                    zone_type: None,
                },
                telemetry: Telemetry::with_capacity(3),
                last_power_kw: 0.0,
                last_soc_target: 0.0,
                last_dr_level: None,
                core_output: CoreOutput::default(),
                initialized: false,
            }
        }
    }

    impl Equipment for TestEquipment {
        fn descriptor(&self) -> &EquipmentDescriptor {
            &self.descriptor
        }

        fn rename(&mut self, name: String) {
            self.descriptor.name = name;
        }

        fn set_equipment_id(
            &mut self,
            id: EquipmentId,
        ) -> std::result::Result<(), hares_types::HaresError> {
            hares_equipment::apply_identity_write(self.is_initialized(), &mut self.descriptor, id)
        }

        fn ports(&self) -> &[PortDeclaration] {
            &[]
        }

        fn init(
            &mut self,
            _config: &EquipmentConfig,
            _env: &hares_types::EnvironmentState,
        ) -> std::result::Result<(), hares_types::HaresError> {
            self.telemetry.insert(tk::LAST_POWER_KW, self.last_power_kw);
            self.telemetry
                .insert(tk::LAST_SOC_TARGET, self.last_soc_target);
            Ok(())
        }

        fn update_control(&mut self, _env: &hares_types::EnvironmentState) -> OperatingMode {
            OperatingMode::Off
        }

        fn step(
            &mut self,
            _env: &hares_types::EnvironmentState,
            _dt: Duration,
            _ports: &mut PortSlots,
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }

        fn telemetry(&self) -> &Telemetry {
            &self.telemetry
        }

        fn core_output(&self) -> &CoreOutput {
            &self.core_output
        }

        fn save_state(&self) -> std::result::Result<Vec<u8>, hares_types::HaresError> {
            Ok(vec![])
        }

        fn load_state(
            &mut self,
            _state: &[u8],
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }

        fn apply_signal(
            &mut self,
            signal: &ControlSignal,
        ) -> std::result::Result<(), hares_types::HaresError> {
            match signal {
                ControlSignal::PowerSetpoint {
                    active_power_kw, ..
                } => {
                    self.last_power_kw = *active_power_kw;
                    self.telemetry.insert(tk::LAST_POWER_KW, self.last_power_kw);
                }
                ControlSignal::SOCTarget { target_soc, .. } => {
                    self.last_soc_target = *target_soc;
                    self.telemetry
                        .insert(tk::LAST_SOC_TARGET, self.last_soc_target);
                }
                ControlSignal::DemandResponse { level, .. } => {
                    self.last_dr_level = Some(*level);
                    self.telemetry.insert("last_dr_level", *level as u8 as f64);
                }
                _ => {}
            }
            Ok(())
        }

        fn is_initialized(&self) -> bool {
            self.initialized
        }

        fn mark_initialized(&mut self) {
            self.initialized = true;
        }

        fn unmark_initialized(&mut self) {
            self.initialized = false;
        }
    }

    struct TestReactiveEquipment {
        descriptor: EquipmentDescriptor,
        telemetry: Telemetry,
        active_power_kw: f64,
        reactive_power_kvar: f64,
        ports: Vec<PortDeclaration>,
        core_output: CoreOutput,
    }

    impl TestReactiveEquipment {
        fn new(name: &str, active_power_kw: f64, reactive_power_kvar: f64) -> Self {
            Self {
                descriptor: EquipmentDescriptor {
                    id: EquipmentId(2),
                    name: name.to_string(),
                    end_use: EndUse::OTHER,
                    equipment_type: Cow::Borrowed("TestReactiveEquipment"),
                    zone: Some(ZoneId(1)),
                    fuel: FuelType::Electric,
                    stage: ExecutionStage::Independent,
                    control_capabilities: ControlCapabilities::empty(),
                    core_capabilities: CoreCapabilities::ELECTRIC | CoreCapabilities::REACTIVE,
                    telemetry_fields: vec![
                        TelemetryField {
                            name: "active_power_kw".to_string(),
                            unit: "kW".to_string(),
                            description: "active electrical power".to_string(),
                        },
                        TelemetryField {
                            name: "reactive_power_kvar".to_string(),
                            unit: "kVAR".to_string(),
                            description: "reactive electrical power".to_string(),
                        },
                    ],
                    zone_type: None,
                },
                telemetry: Telemetry::with_capacity(2),
                active_power_kw,
                reactive_power_kvar,
                ports: vec![PortDeclaration::electrical()],
                core_output: CoreOutput::default(),
            }
        }
    }

    impl Equipment for TestReactiveEquipment {
        fn descriptor(&self) -> &EquipmentDescriptor {
            &self.descriptor
        }

        fn rename(&mut self, name: String) {
            self.descriptor.name = name;
        }

        fn set_equipment_id(
            &mut self,
            id: EquipmentId,
        ) -> std::result::Result<(), hares_types::HaresError> {
            hares_equipment::apply_identity_write(self.is_initialized(), &mut self.descriptor, id)
        }

        fn ports(&self) -> &[PortDeclaration] {
            &self.ports
        }

        fn init(
            &mut self,
            _config: &EquipmentConfig,
            _env: &hares_types::EnvironmentState,
        ) -> std::result::Result<(), hares_types::HaresError> {
            self.telemetry
                .insert("active_power_kw", self.active_power_kw);
            self.telemetry
                .insert("reactive_power_kvar", self.reactive_power_kvar);
            Ok(())
        }

        fn update_control(&mut self, _env: &hares_types::EnvironmentState) -> OperatingMode {
            OperatingMode::Off
        }

        fn step(
            &mut self,
            _env: &hares_types::EnvironmentState,
            _dt: Duration,
            ports: &mut PortSlots,
        ) -> std::result::Result<(), hares_types::HaresError> {
            ports.accumulate(&PortContribution::Electrical {
                active_power_w: self.active_power_kw * 1000.0,
                reactive_power_kvar: self.reactive_power_kvar,
            })?;
            // Keep CoreOutput consistent with the port contribution above
            // (validate_port_core_electrical_consistency is unconditional).
            self.core_output.flows.electric_kw = Some(hares_types::ElectricPower::consumption(
                self.active_power_kw,
            )?);
            self.core_output.flows.reactive_power_kvar = Some(self.reactive_power_kvar);
            Ok(())
        }

        fn telemetry(&self) -> &Telemetry {
            &self.telemetry
        }

        fn core_output(&self) -> &CoreOutput {
            &self.core_output
        }

        fn save_state(&self) -> std::result::Result<Vec<u8>, hares_types::HaresError> {
            Ok(vec![])
        }

        fn load_state(
            &mut self,
            _state: &[u8],
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }

        fn apply_signal(
            &mut self,
            _signal: &ControlSignal,
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }
    }

    /// Test equipment that raises one warning in `init` and one in its third
    /// step, through the `drain_warnings` channel.
    #[cfg(test)]
    struct TestWarningEquipment {
        descriptor: EquipmentDescriptor,
        telemetry: Telemetry,
        core_output: CoreOutput,
        ports: Vec<PortDeclaration>,
        warnings: Vec<Warning>,
        steps_done: u32,
    }

    #[cfg(test)]
    impl TestWarningEquipment {
        fn new(name: &str) -> Self {
            Self {
                descriptor: EquipmentDescriptor {
                    id: EquipmentId(0),
                    name: name.to_string(),
                    end_use: EndUse::OTHER,
                    equipment_type: Cow::Borrowed("TestWarningEquipment"),
                    zone: Some(ZoneId(1)),
                    fuel: FuelType::Electric,
                    stage: ExecutionStage::Independent,
                    control_capabilities: ControlCapabilities::empty(),
                    core_capabilities: CoreCapabilities::ELECTRIC,
                    telemetry_fields: vec![],
                    zone_type: None,
                },
                telemetry: Telemetry::with_capacity(1),
                core_output: CoreOutput::default(),
                ports: vec![PortDeclaration::electrical()],
                warnings: Vec::new(),
                steps_done: 0,
            }
        }
    }

    #[cfg(test)]
    impl Equipment for TestWarningEquipment {
        fn descriptor(&self) -> &EquipmentDescriptor {
            &self.descriptor
        }

        fn rename(&mut self, name: String) {
            self.descriptor.name = name;
        }

        fn set_equipment_id(
            &mut self,
            id: EquipmentId,
        ) -> std::result::Result<(), hares_types::HaresError> {
            hares_equipment::apply_identity_write(self.is_initialized(), &mut self.descriptor, id)
        }

        fn ports(&self) -> &[PortDeclaration] {
            &self.ports
        }

        fn init(
            &mut self,
            _config: &EquipmentConfig,
            _env: &hares_types::EnvironmentState,
        ) -> std::result::Result<(), hares_types::HaresError> {
            self.warnings.clear();
            self.steps_done = 0;
            self.warnings.push(Warning::new(
                self.descriptor.name.as_str(),
                "test equipment init warning",
            ));
            Ok(())
        }

        fn update_control(&mut self, _env: &hares_types::EnvironmentState) -> OperatingMode {
            OperatingMode::Off
        }

        fn step(
            &mut self,
            _env: &hares_types::EnvironmentState,
            _dt: Duration,
            ports: &mut PortSlots,
        ) -> std::result::Result<(), hares_types::HaresError> {
            self.steps_done += 1;
            if self.steps_done == 3 {
                self.warnings.push(Warning::new(
                    self.descriptor.name.as_str(),
                    "test equipment step warning",
                ));
            }
            ports.accumulate(&PortContribution::Electrical {
                active_power_w: 0.0,
                reactive_power_kvar: 0.0,
            })?;
            self.core_output.flows.electric_kw =
                Some(hares_types::ElectricPower::consumption(0.0)?);
            Ok(())
        }

        fn telemetry(&self) -> &Telemetry {
            &self.telemetry
        }

        fn core_output(&self) -> &CoreOutput {
            &self.core_output
        }

        fn save_state(&self) -> std::result::Result<Vec<u8>, hares_types::HaresError> {
            Ok(vec![])
        }

        fn load_state(
            &mut self,
            _state: &[u8],
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }

        fn apply_signal(
            &mut self,
            _signal: &ControlSignal,
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }

        fn drain_warnings(&mut self, out: &mut Vec<Warning>) {
            out.append(&mut self.warnings);
        }
    }

    /// The warning channel carries an equipment's init and step warnings into
    /// the dwelling's log: the init warning drains at registration, the step
    /// warning drains beside the health counts carrying the step index.
    #[test]
    fn equipment_warnings_reach_the_dwelling_log() {
        let base_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/bestest/600.toml");
        let mut dwelling = Dwelling::from_toml_config_with_write_output(&base_path, Some(false))
            .expect("build dwelling");

        let mut warner = TestWarningEquipment::new("Warner");
        warner
            .init(&EquipmentConfig::default(), &dwelling.latest_env)
            .expect("init warning test equipment");
        dwelling
            .add_equipment(Box::new(warner))
            .expect("add warning test equipment");

        let warnings = dwelling.take_warnings();
        assert_eq!(
            warnings.len(),
            1,
            "the init warning drains at registration, got {warnings:?}"
        );
        assert!(
            warnings[0].contains("Warner") && warnings[0].contains("init warning"),
            "the init warning names the equipment: {}",
            warnings[0]
        );

        for _ in 0..3 {
            dwelling.run_timestep(false).expect("dwelling step");
        }
        let warnings = dwelling.take_warnings();
        assert_eq!(
            warnings.len(),
            1,
            "one step warning from the third step, got {warnings:?}"
        );
        assert!(
            warnings[0].starts_with("step 2: "),
            "the step warning carries the step index: {}",
            warnings[0]
        );
        assert!(
            warnings[0].contains("Warner") && warnings[0].contains("step warning"),
            "the step warning names the equipment: {}",
            warnings[0]
        );
    }

    /// A tariff's parse warnings report through `set_tariff` into the run's
    /// warning log.
    #[test]
    fn set_tariff_reports_tariff_parse_warnings() {
        let base_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/bestest/600.toml");
        let mut dwelling = Dwelling::from_toml_config_with_write_output(&base_path, Some(false))
            .expect("build dwelling");

        let tariff = ElectricTariff {
            parse_warnings: vec!["test tariff parse warning".to_string()],
            ..ElectricTariff::default()
        };
        dwelling
            .set_tariff(tariff, chrono_tz::Tz::UTC)
            .expect("set_tariff with a parse warning succeeds");

        let warnings = dwelling.take_warnings();
        assert_eq!(
            warnings.len(),
            1,
            "the tariff parse warning reaches the log, got {warnings:?}"
        );
        assert!(
            warnings[0].starts_with("tariff: test tariff parse warning"),
            "the warning is prefixed with the tariff source: {}",
            warnings[0]
        );
    }

    /// Test equipment that correctly deposits power into ports but
    /// under-reports electric power in `core_output()`, used to verify
    /// that `validate_port_core_electrical_consistency` fails the step on
    /// the discrepancy.
    #[cfg(test)]
    struct UnderReportingEquipment {
        descriptor: EquipmentDescriptor,
        telemetry: Telemetry,
        core_output: CoreOutput,
        true_power_kw: f64,
        reported_power_kw: f64,
        ports: Vec<PortDeclaration>,
    }

    #[cfg(test)]
    impl UnderReportingEquipment {
        fn new(name: &str, true_power_kw: f64, reported_power_kw: f64) -> Self {
            let mut co = CoreOutput::default();
            co.flows.electric_kw =
                Some(hares_types::ElectricPower::consumption(reported_power_kw).unwrap());
            Self {
                descriptor: EquipmentDescriptor {
                    id: EquipmentId(99),
                    name: name.to_string(),
                    end_use: EndUse::OTHER,
                    equipment_type: Cow::Borrowed("UnderReportingEquipment"),
                    zone: Some(ZoneId(1)),
                    fuel: FuelType::Electric,
                    stage: ExecutionStage::Independent,
                    control_capabilities: ControlCapabilities::empty(),
                    core_capabilities: CoreCapabilities::ELECTRIC,
                    telemetry_fields: vec![TelemetryField {
                        name: "power_kw".to_string(),
                        unit: "kW".to_string(),
                        description: "electrical power".to_string(),
                    }],
                    zone_type: None,
                },
                telemetry: Telemetry::with_capacity(1),
                core_output: co,
                true_power_kw,
                reported_power_kw,
                ports: vec![PortDeclaration::electrical()],
            }
        }
    }

    #[cfg(test)]
    impl Equipment for UnderReportingEquipment {
        fn descriptor(&self) -> &EquipmentDescriptor {
            &self.descriptor
        }

        fn rename(&mut self, name: String) {
            self.descriptor.name = name;
        }

        fn set_equipment_id(
            &mut self,
            id: EquipmentId,
        ) -> std::result::Result<(), hares_types::HaresError> {
            hares_equipment::apply_identity_write(self.is_initialized(), &mut self.descriptor, id)
        }

        fn ports(&self) -> &[PortDeclaration] {
            &self.ports
        }

        fn init(
            &mut self,
            _config: &EquipmentConfig,
            _env: &hares_types::EnvironmentState,
        ) -> std::result::Result<(), hares_types::HaresError> {
            self.telemetry.insert("power_kw", self.reported_power_kw);
            Ok(())
        }

        fn update_control(&mut self, _env: &hares_types::EnvironmentState) -> OperatingMode {
            OperatingMode::Off
        }

        fn step(
            &mut self,
            _env: &hares_types::EnvironmentState,
            _dt: Duration,
            ports: &mut PortSlots,
        ) -> std::result::Result<(), hares_types::HaresError> {
            ports.accumulate(&PortContribution::Electrical {
                active_power_w: self.true_power_kw * 1000.0,
                reactive_power_kvar: 0.0,
            })?;
            Ok(())
        }

        fn telemetry(&self) -> &Telemetry {
            &self.telemetry
        }

        fn core_output(&self) -> &CoreOutput {
            &self.core_output
        }

        fn save_state(&self) -> std::result::Result<Vec<u8>, hares_types::HaresError> {
            Ok(vec![])
        }

        fn load_state(
            &mut self,
            _state: &[u8],
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }

        fn apply_signal(
            &mut self,
            _signal: &ControlSignal,
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }
    }

    /// Test equipment reproducing the water-heater bug class: the port
    /// receives a nonzero reactive contribution while `CoreOutput` reports
    /// `flows.reactive_power_kvar = None`. Active power is consistent, so
    /// only the reactive check of
    /// `validate_port_core_electrical_consistency` fires.
    #[cfg(test)]
    struct ReactiveDivergentEquipment {
        descriptor: EquipmentDescriptor,
        telemetry: Telemetry,
        core_output: CoreOutput,
        active_power_kw: f64,
        silent_reactive_kvar: f64,
        ports: Vec<PortDeclaration>,
    }

    #[cfg(test)]
    impl ReactiveDivergentEquipment {
        fn new(name: &str, active_power_kw: f64, silent_reactive_kvar: f64) -> Self {
            let mut co = CoreOutput::default();
            co.flows.electric_kw =
                Some(hares_types::ElectricPower::consumption(active_power_kw).unwrap());
            Self {
                descriptor: EquipmentDescriptor {
                    id: EquipmentId(98),
                    name: name.to_string(),
                    end_use: EndUse::OTHER,
                    equipment_type: Cow::Borrowed("ReactiveDivergentEquipment"),
                    zone: Some(ZoneId(1)),
                    fuel: FuelType::Electric,
                    stage: ExecutionStage::Independent,
                    control_capabilities: ControlCapabilities::empty(),
                    core_capabilities: CoreCapabilities::ELECTRIC,
                    telemetry_fields: vec![],
                    zone_type: None,
                },
                telemetry: Telemetry::default(),
                core_output: co,
                active_power_kw,
                silent_reactive_kvar,
                ports: vec![PortDeclaration::electrical()],
            }
        }
    }

    #[cfg(test)]
    impl Equipment for ReactiveDivergentEquipment {
        fn descriptor(&self) -> &EquipmentDescriptor {
            &self.descriptor
        }

        fn rename(&mut self, name: String) {
            self.descriptor.name = name;
        }

        fn set_equipment_id(
            &mut self,
            id: EquipmentId,
        ) -> std::result::Result<(), hares_types::HaresError> {
            hares_equipment::apply_identity_write(self.is_initialized(), &mut self.descriptor, id)
        }

        fn ports(&self) -> &[PortDeclaration] {
            &self.ports
        }

        fn init(
            &mut self,
            _config: &EquipmentConfig,
            _env: &hares_types::EnvironmentState,
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }

        fn update_control(&mut self, _env: &hares_types::EnvironmentState) -> OperatingMode {
            OperatingMode::Off
        }

        fn step(
            &mut self,
            _env: &hares_types::EnvironmentState,
            _dt: Duration,
            ports: &mut PortSlots,
        ) -> std::result::Result<(), hares_types::HaresError> {
            ports.accumulate(&PortContribution::Electrical {
                active_power_w: self.active_power_kw * 1000.0,
                reactive_power_kvar: self.silent_reactive_kvar,
            })?;
            Ok(())
        }

        fn telemetry(&self) -> &Telemetry {
            &self.telemetry
        }

        fn core_output(&self) -> &CoreOutput {
            &self.core_output
        }

        fn save_state(&self) -> std::result::Result<Vec<u8>, hares_types::HaresError> {
            Ok(vec![])
        }

        fn load_state(
            &mut self,
            _state: &[u8],
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }

        fn apply_signal(
            &mut self,
            _signal: &ControlSignal,
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }
    }

    struct DispatchAwareThermalEquipment {
        descriptor: EquipmentDescriptor,
        mode_override: Option<OperatingMode>,
        update_calls: Arc<AtomicUsize>,
        last_mode_code: Arc<AtomicU8>,
        core_output: CoreOutput,
    }

    impl DispatchAwareThermalEquipment {
        fn new(name: &str, update_calls: Arc<AtomicUsize>, last_mode_code: Arc<AtomicU8>) -> Self {
            Self {
                descriptor: EquipmentDescriptor {
                    id: EquipmentId(777),
                    name: name.to_string(),
                    end_use: EndUse::HVAC_HEATING,
                    equipment_type: Cow::Borrowed("DispatchAwareThermalEquipment"),
                    zone: Some(ZoneId(1)),
                    fuel: FuelType::Electric,
                    stage: ExecutionStage::Thermal,
                    control_capabilities: ControlCapabilities::MODE_OVERRIDE,
                    core_capabilities: CoreCapabilities::HAS_MODE | CoreCapabilities::ELECTRIC,
                    telemetry_fields: vec![],
                    zone_type: None,
                },
                mode_override: None,
                update_calls,
                last_mode_code,
                core_output: CoreOutput::default(),
            }
        }
    }

    impl Equipment for DispatchAwareThermalEquipment {
        fn descriptor(&self) -> &EquipmentDescriptor {
            &self.descriptor
        }

        fn rename(&mut self, name: String) {
            self.descriptor.name = name;
        }

        fn set_equipment_id(
            &mut self,
            id: EquipmentId,
        ) -> std::result::Result<(), hares_types::HaresError> {
            hares_equipment::apply_identity_write(self.is_initialized(), &mut self.descriptor, id)
        }

        fn ports(&self) -> &[PortDeclaration] {
            &[]
        }

        fn init(
            &mut self,
            _config: &EquipmentConfig,
            _env: &hares_types::EnvironmentState,
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }

        fn update_control(&mut self, _env: &hares_types::EnvironmentState) -> OperatingMode {
            self.update_calls.fetch_add(1, Ordering::Relaxed);
            let mode = self.mode_override.unwrap_or(OperatingMode::Off);
            let code = if mode == OperatingMode::Heating { 1 } else { 0 };
            self.last_mode_code.store(code, Ordering::Relaxed);
            self.core_output.state.operating_mode = Some(mode);
            mode
        }

        fn step(
            &mut self,
            _env: &hares_types::EnvironmentState,
            _dt: Duration,
            ports: &mut PortSlots,
        ) -> std::result::Result<(), hares_types::HaresError> {
            if self
                .core_output
                .state
                .operating_mode
                .is_some_and(|m| m.is_active())
            {
                self.core_output.flows.electric_kw = Some(ElectricPower::Consumption(0.001));
                ports.accumulate(&PortContribution::Electrical {
                    active_power_w: 1.0,
                    reactive_power_kvar: 0.0,
                })?;
            }
            Ok(())
        }

        fn telemetry(&self) -> &Telemetry {
            static EMPTY_TELEMETRY: std::sync::OnceLock<Telemetry> = std::sync::OnceLock::new();
            EMPTY_TELEMETRY.get_or_init(Telemetry::default)
        }

        fn core_output(&self) -> &CoreOutput {
            &self.core_output
        }

        fn save_state(&self) -> std::result::Result<Vec<u8>, hares_types::HaresError> {
            Ok(Vec::new())
        }

        fn load_state(
            &mut self,
            _state: &[u8],
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }

        fn apply_signal(
            &mut self,
            signal: &ControlSignal,
        ) -> std::result::Result<(), hares_types::HaresError> {
            if let ControlSignal::ModeOverride { mode } = signal {
                self.mode_override = Some(*mode);
            }
            Ok(())
        }
    }

    #[cfg(test)]
    struct ThermalUnderReportingEquipment {
        descriptor: EquipmentDescriptor,
        telemetry: Telemetry,
        core_output: CoreOutput,
        deposited_thermal_w: f64,
        ports: Vec<PortDeclaration>,
    }

    #[cfg(test)]
    impl ThermalUnderReportingEquipment {
        fn new(name: &str, deposited_thermal_w: f64, reported_thermal_w: f64) -> Self {
            let mut co = CoreOutput::default();
            co.flows.thermal_output_w = Some(reported_thermal_w);
            co.flows.electric_kw = Some(hares_types::ElectricPower::consumption(0.0).unwrap());
            Self {
                descriptor: EquipmentDescriptor {
                    id: EquipmentId(100),
                    name: name.to_string(),
                    end_use: EndUse::HVAC_HEATING,
                    equipment_type: Cow::Borrowed("ThermalUnderReportingEquipment"),
                    zone: Some(ZoneId(1)),
                    fuel: FuelType::Electric,
                    stage: ExecutionStage::Thermal,
                    control_capabilities: ControlCapabilities::empty(),
                    core_capabilities: CoreCapabilities::ELECTRIC | CoreCapabilities::THERMAL,
                    telemetry_fields: vec![TelemetryField {
                        name: "thermal_output_w".to_string(),
                        unit: "W".to_string(),
                        description: "thermal output".to_string(),
                    }],
                    zone_type: None,
                },
                telemetry: Telemetry::with_capacity(1),
                core_output: co,
                deposited_thermal_w,
                ports: vec![
                    PortDeclaration::electrical(),
                    PortDeclaration::thermal(ZoneId(1)),
                ],
            }
        }
    }

    #[cfg(test)]
    impl Equipment for ThermalUnderReportingEquipment {
        fn descriptor(&self) -> &EquipmentDescriptor {
            &self.descriptor
        }

        fn rename(&mut self, name: String) {
            self.descriptor.name = name;
        }

        fn set_equipment_id(
            &mut self,
            id: EquipmentId,
        ) -> std::result::Result<(), hares_types::HaresError> {
            hares_equipment::apply_identity_write(self.is_initialized(), &mut self.descriptor, id)
        }

        fn ports(&self) -> &[PortDeclaration] {
            &self.ports
        }

        fn init(
            &mut self,
            _config: &EquipmentConfig,
            _env: &hares_types::EnvironmentState,
        ) -> std::result::Result<(), hares_types::HaresError> {
            self.telemetry.insert(
                "thermal_output_w",
                self.core_output.flows.thermal_output_w.unwrap_or(0.0),
            );
            Ok(())
        }

        fn update_control(&mut self, _env: &hares_types::EnvironmentState) -> OperatingMode {
            OperatingMode::Heating
        }

        fn step(
            &mut self,
            _env: &hares_types::EnvironmentState,
            _dt: Duration,
            ports: &mut PortSlots,
        ) -> std::result::Result<(), hares_types::HaresError> {
            ports.accumulate(&PortContribution::Thermal {
                zone: ZoneId(1),
                sensible_gain_w: self.deposited_thermal_w,
                radiant_gain_w: 0.0,
                latent_gain_w: 0.0,
                category: ThermalCategory::HvacHeating,
            })?;
            Ok(())
        }

        fn telemetry(&self) -> &Telemetry {
            &self.telemetry
        }

        fn core_output(&self) -> &CoreOutput {
            &self.core_output
        }

        fn save_state(&self) -> std::result::Result<Vec<u8>, hares_types::HaresError> {
            Ok(vec![])
        }

        fn load_state(
            &mut self,
            _state: &[u8],
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }

        fn apply_signal(
            &mut self,
            _signal: &ControlSignal,
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }
    }

    #[cfg(test)]
    struct WaterHeatingEquipment {
        descriptor: EquipmentDescriptor,
        telemetry: Telemetry,
        core_output: CoreOutput,
        thermal_w: f64,
        ports: Vec<PortDeclaration>,
    }

    #[cfg(test)]
    impl WaterHeatingEquipment {
        fn new(name: &str, thermal_w: f64) -> Self {
            let mut co = CoreOutput::default();
            co.flows.thermal_output_w = Some(thermal_w);
            co.flows.electric_kw = Some(hares_types::ElectricPower::consumption(0.0).unwrap());
            Self {
                descriptor: EquipmentDescriptor {
                    id: EquipmentId(200),
                    name: name.to_string(),
                    end_use: EndUse::WATER_HEATING,
                    equipment_type: Cow::Borrowed("WaterHeatingEquipment"),
                    zone: Some(ZoneId(1)),
                    fuel: FuelType::Electric,
                    stage: ExecutionStage::Thermal,
                    control_capabilities: ControlCapabilities::empty(),
                    core_capabilities: CoreCapabilities::ELECTRIC | CoreCapabilities::THERMAL,
                    telemetry_fields: vec![TelemetryField {
                        name: "thermal_output_w".to_string(),
                        unit: "W".to_string(),
                        description: "thermal output".to_string(),
                    }],
                    zone_type: None,
                },
                telemetry: Telemetry::with_capacity(1),
                core_output: co,
                thermal_w,
                ports: vec![
                    PortDeclaration::electrical(),
                    PortDeclaration::thermal(ZoneId(1)),
                ],
            }
        }
    }

    #[cfg(test)]
    impl Equipment for WaterHeatingEquipment {
        fn descriptor(&self) -> &EquipmentDescriptor {
            &self.descriptor
        }

        fn rename(&mut self, name: String) {
            self.descriptor.name = name;
        }

        fn set_equipment_id(
            &mut self,
            id: EquipmentId,
        ) -> std::result::Result<(), hares_types::HaresError> {
            hares_equipment::apply_identity_write(self.is_initialized(), &mut self.descriptor, id)
        }

        fn ports(&self) -> &[PortDeclaration] {
            &self.ports
        }

        fn init(
            &mut self,
            _config: &EquipmentConfig,
            _env: &hares_types::EnvironmentState,
        ) -> std::result::Result<(), HaresError> {
            self.telemetry.insert(
                "thermal_output_w",
                self.core_output.flows.thermal_output_w.unwrap_or(0.0),
            );
            Ok(())
        }

        fn update_control(&mut self, _env: &hares_types::EnvironmentState) -> OperatingMode {
            OperatingMode::Off
        }

        fn step(
            &mut self,
            _env: &hares_types::EnvironmentState,
            _dt: Duration,
            ports: &mut PortSlots,
        ) -> std::result::Result<(), HaresError> {
            ports.accumulate(&PortContribution::Thermal {
                zone: ZoneId(1),
                sensible_gain_w: self.thermal_w,
                radiant_gain_w: 0.0,
                latent_gain_w: 0.0,
                category: ThermalCategory::JacketLoss,
            })?;
            Ok(())
        }

        fn telemetry(&self) -> &Telemetry {
            &self.telemetry
        }

        fn core_output(&self) -> &CoreOutput {
            &self.core_output
        }

        fn save_state(&self) -> std::result::Result<Vec<u8>, HaresError> {
            Ok(vec![])
        }

        fn load_state(&mut self, _state: &[u8]) -> std::result::Result<(), HaresError> {
            Ok(())
        }

        fn apply_signal(&mut self, _signal: &ControlSignal) -> std::result::Result<(), HaresError> {
            Ok(())
        }
    }

    #[cfg(test)]
    struct CoolingWithLatentEquipment {
        descriptor: EquipmentDescriptor,
        telemetry: Telemetry,
        core_output: CoreOutput,
        sensible_w: f64,
        latent_w: f64,
        ports: Vec<PortDeclaration>,
    }

    #[cfg(test)]
    impl CoolingWithLatentEquipment {
        fn new(name: &str, sensible_w: f64, latent_w: f64, reported_thermal_w: f64) -> Self {
            let mut co = CoreOutput::default();
            co.flows.thermal_output_w = Some(reported_thermal_w);
            co.flows.electric_kw = Some(hares_types::ElectricPower::consumption(0.0).unwrap());
            Self {
                descriptor: EquipmentDescriptor {
                    id: EquipmentId(201),
                    name: name.to_string(),
                    end_use: EndUse::HVAC_COOLING,
                    equipment_type: Cow::Borrowed("CoolingWithLatentEquipment"),
                    zone: Some(ZoneId(1)),
                    fuel: FuelType::Electric,
                    stage: ExecutionStage::Thermal,
                    control_capabilities: ControlCapabilities::empty(),
                    core_capabilities: CoreCapabilities::ELECTRIC | CoreCapabilities::THERMAL,
                    telemetry_fields: vec![TelemetryField {
                        name: "thermal_output_w".to_string(),
                        unit: "W".to_string(),
                        description: "thermal output".to_string(),
                    }],
                    zone_type: None,
                },
                telemetry: Telemetry::with_capacity(1),
                core_output: co,
                sensible_w,
                latent_w,
                ports: vec![
                    PortDeclaration::electrical(),
                    PortDeclaration::thermal(ZoneId(1)),
                ],
            }
        }
    }

    #[cfg(test)]
    impl Equipment for CoolingWithLatentEquipment {
        fn descriptor(&self) -> &EquipmentDescriptor {
            &self.descriptor
        }

        fn rename(&mut self, name: String) {
            self.descriptor.name = name;
        }

        fn set_equipment_id(
            &mut self,
            id: EquipmentId,
        ) -> std::result::Result<(), hares_types::HaresError> {
            hares_equipment::apply_identity_write(self.is_initialized(), &mut self.descriptor, id)
        }

        fn ports(&self) -> &[PortDeclaration] {
            &self.ports
        }

        fn init(
            &mut self,
            _config: &EquipmentConfig,
            _env: &hares_types::EnvironmentState,
        ) -> std::result::Result<(), HaresError> {
            self.telemetry.insert(
                "thermal_output_w",
                self.core_output.flows.thermal_output_w.unwrap_or(0.0),
            );
            Ok(())
        }

        fn update_control(&mut self, _env: &hares_types::EnvironmentState) -> OperatingMode {
            OperatingMode::Off
        }

        fn step(
            &mut self,
            _env: &hares_types::EnvironmentState,
            _dt: Duration,
            ports: &mut PortSlots,
        ) -> std::result::Result<(), HaresError> {
            ports.accumulate(&PortContribution::Thermal {
                zone: ZoneId(1),
                sensible_gain_w: self.sensible_w,
                radiant_gain_w: 0.0,
                latent_gain_w: self.latent_w,
                category: ThermalCategory::HvacCooling,
            })?;
            Ok(())
        }

        fn telemetry(&self) -> &Telemetry {
            &self.telemetry
        }

        fn core_output(&self) -> &CoreOutput {
            &self.core_output
        }

        fn save_state(&self) -> std::result::Result<Vec<u8>, HaresError> {
            Ok(vec![])
        }

        fn load_state(&mut self, _state: &[u8]) -> std::result::Result<(), HaresError> {
            Ok(())
        }

        fn apply_signal(&mut self, _signal: &ControlSignal) -> std::result::Result<(), HaresError> {
            Ok(())
        }
    }

    #[test]
    fn stage_rank_orders_execution_stages() {
        assert!(stage_rank(ExecutionStage::Independent) < stage_rank(ExecutionStage::Electrical));
        assert!(stage_rank(ExecutionStage::Electrical) < stage_rank(ExecutionStage::Thermal));
    }

    #[test]
    fn telemetry_reports_reactive_power_after_dwelling_step() {
        let base_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/bestest/600.toml");
        let mut dwelling = Dwelling::from_toml_config_with_write_output(&base_path, Some(false))
            .expect("build dwelling");

        let mut reactive_eq = TestReactiveEquipment::new("ReactiveLoad", 2.0, 0.75);
        reactive_eq
            .init(&EquipmentConfig::default(), &dwelling.latest_env)
            .expect("init reactive test equipment");

        replace_equipment_for_test(&mut dwelling, vec![Box::new(reactive_eq)]);

        dwelling.run_timestep(false).expect("dwelling step");
        let telemetry = dwelling.telemetry().unwrap();

        assert!((telemetry.reactive_power_kvar - 0.75).abs() < 1e-9);
    }

    /// Regression: an EV actively discharging (V2L, negative `ev_power_kw`)
    /// must not raise `base_load_kw`. The electrical port already routes the
    /// discharge into `generation_power_w`, so subtracting the *signed* EV
    /// power from `load_power_w` added the discharge magnitude to the
    /// non-dispatchable load — and the BMS caps its discharge at
    /// `base_load_kw`, so the error fed a control decision.
    #[test]
    fn base_load_kw_excludes_ev_discharge() {
        use hares_equipment::EvConfig;
        use hares_equipment::ev::Ev;
        use hares_equipment::scheduled_load::ScheduledLoad;
        use std::collections::HashMap;

        let (_dir, toml_path) = temp_toml("base_load_ev_discharge");
        write_minimal_toml(&toml_path);

        let mut dwelling = Dwelling::from_toml_config(&toml_path).expect("build dwelling");
        let env = dwelling.latest_env().clone();

        // 1.5 kW constant non-dispatchable load.
        let mut raw: HashMap<String, ConfigValue> = HashMap::new();
        raw.insert("power_schedule_source".to_string(), "constant".into());
        raw.insert("power_constant_kw".to_string(), 1.5.into());
        raw.insert("sensible_gain_fraction".to_string(), 0.5.into());
        raw.insert("zone_id".to_string(), 1.0.into());
        let config = EquipmentConfig::raw("BaseLoad".to_string(), "ScheduledLoad".to_string(), raw);
        let mut load = ScheduledLoad::new(config.clone(), EndUse::LIGHTING, "Lighting");
        load.init(&config, &env).expect("init ScheduledLoad");
        dwelling
            .add_equipment(Box::new(load))
            .expect("add_equipment must succeed");

        // V2L-capable EV, plugged in at home by default.
        let config = EquipmentConfig::from_typed(
            "EV1".to_string(),
            "EV".to_string(),
            EvConfig {
                equipment_id: None,
                capacity_kwh: 60.0,
                charging_level: Some("L2".to_string()),
                max_charging_power_kw: 7.2,
                charging_efficiency: None,
                l1_current_a: None,
                l1_voltage_v: None,
                soc_max: None,
                initial_soc: Some(0.8),
                battery_temp_c: None,
                min_charge_temp_c: None,
                full_power_temp_c: None,
                heater_power_w: None,
                heater_threshold_c: None,
                thermal_mass_j_per_k: None,
                ua_w_per_k: None,
                n_series: None,
                n_parallel: None,
                cell_resistance_ohm: None,
                v2l_enabled: Some(true),
                v2l_soc_reserve: Some(0.2),
                v2l_max_discharge_kw: Some(3.0),
                v2g_enabled: None,
                v2g_soc_reserve: None,
                v2g_max_discharge_kw: None,
                chemistry: None,
                fuel_economy_kwh_per_mi: None,
                ready_soc: None,
                charging_strategy: None,
                plug_in_policy: None,
                power_limit_kw: None,
                initial_connection_state: None,
                power_factor: None,
                charger_capacity_kva: None,
                cc_cv_transition_soc: None,
                charging_priority: None,
                discharge_respects_deadline: true,
            },
        )
        .expect("typed EV config");
        let mut ev = Ev::new(config.clone());
        ev.init(&config, &env).expect("init EV");
        // An idle actor holds the driver's name, so the EV runs on the
        // control command below alone.
        dwelling
            .add_equipment_with_actors(
                Box::new(ev),
                vec![Box::new(StubActor {
                    name: "EvDriver:EV1".to_string(),
                })],
            )
            .expect("add_equipment must succeed");

        // Command a 2 kW discharge and step once.
        dwelling.apply_control(
            "EV1",
            ControlSignal::PowerSetpoint {
                active_power_kw: -2.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
        );
        dwelling.step().expect("step with discharging EV");

        let summary = &dwelling.prior_electrical_summary;
        assert!(
            summary.ev_power_kw < 0.0,
            "EV must be actively discharging, got ev_power_kw = {}",
            summary.ev_power_kw
        );
        assert!(
            (summary.base_load_kw - 1.5).abs() < 1e-9,
            "base_load_kw must equal the 1.5 kW non-dispatchable load; signed \
             EV subtraction would have added the discharge (ev_power_kw = {}), \
             got {}",
            summary.ev_power_kw,
            summary.base_load_kw
        );
    }

    /// Equipment that reports less electric power in `CoreOutput` than it
    /// deposits at the electrical port previously only tripped the soft
    /// `telemetry_consistency_flag`; the divergence now fails the step hard
    /// via `validate_port_core_electrical_consistency` before telemetry is
    /// ever assembled.
    #[test]
    #[cfg(test)]
    fn dwelling_step_errors_when_equipment_under_reports_power() {
        let base_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/bestest/600.toml");
        let mut dwelling = Dwelling::from_toml_config_with_write_output(&base_path, Some(false))
            .expect("build dwelling");

        let mut eq = UnderReportingEquipment::new("UnderReporter", 3.0, 1.0);
        eq.init(&EquipmentConfig::default(), &dwelling.latest_env)
            .expect("init under-reporting equipment");

        replace_equipment_for_test(&mut dwelling, vec![Box::new(eq)]);

        let err = dwelling
            .run_timestep(false)
            .expect_err("under-reporting equipment must fail the step in debug builds");
        let msg = err.to_string();
        assert!(
            msg.contains("port/core electrical consistency violation")
                && msg.contains("UnderReporter"),
            "step error must identify the port/core divergence, got: {msg}"
        );
    }

    /// The water-heater bug class: equipment deposits reactive power at the
    /// electrical port while reporting `flows.reactive_power_kvar = None` in
    /// `CoreOutput`. The port/core consistency validator must fail the step.
    #[test]
    #[cfg(test)]
    fn dwelling_step_errors_when_port_reactive_diverges_from_core_output() {
        let base_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/bestest/600.toml");
        let mut dwelling = Dwelling::from_toml_config_with_write_output(&base_path, Some(false))
            .expect("build dwelling");

        let mut eq = ReactiveDivergentEquipment::new("SilentReactive", 2.0, 0.6);
        eq.init(&EquipmentConfig::default(), &dwelling.latest_env)
            .expect("init reactive-divergent equipment");

        replace_equipment_for_test(&mut dwelling, vec![Box::new(eq)]);

        let err = dwelling
            .run_timestep(false)
            .expect_err("silent port reactive contribution must fail the step in debug builds");
        let msg = err.to_string();
        assert!(
            msg.contains("port/core electrical consistency violation")
                && msg.contains("flows.reactive_power_kvar is None")
                && msg.contains("SilentReactive"),
            "step error must identify the silent reactive contribution, got: {msg}"
        );
    }

    #[test]
    fn run_timestep_fails_when_thermal_equipment_under_reports() {
        let base_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/bestest/600.toml");
        let mut dwelling = Dwelling::from_toml_config_with_write_output(&base_path, Some(false))
            .expect("build dwelling");

        let mut eq = ThermalUnderReportingEquipment::new("ThermalUnderReporter", 1000.0, 50.0);
        eq.init(&EquipmentConfig::default(), &dwelling.latest_env)
            .expect("init thermal under-reporting equipment");

        replace_equipment_for_test(&mut dwelling, vec![Box::new(eq)]);

        let err = dwelling
            .run_timestep(false)
            .expect_err("thermal consistency must fail the step when equipment reports 50 W while depositing 1000 W into the HvacHeating port");
        assert!(
            err.to_string().contains("per-zone thermal consistency"),
            "got: {err}"
        );
    }

    #[test]
    fn thermal_consistency_excludes_water_heating_by_end_use() {
        let base_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/bestest/600.toml");
        let mut dwelling = Dwelling::from_toml_config_with_write_output(&base_path, Some(false))
            .expect("build dwelling");

        let mut eq = WaterHeatingEquipment::new("IndirectTank", 1000.0);
        eq.init(&EquipmentConfig::default(), &dwelling.latest_env)
            .expect("init water heating equipment");

        replace_equipment_for_test(&mut dwelling, vec![Box::new(eq)]);

        dwelling
            .run_timestep(false)
            .expect("water heating equipment depositing 1000 W via JacketLoss with thermal_output_w=1000 W must be excluded from the thermal consistency check (end_use != HVAC_HEATING/COOLING)");
    }

    #[test]
    fn thermal_consistency_accounts_for_latent_cooling() {
        let base_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/bestest/600.toml");
        let mut dwelling = Dwelling::from_toml_config_with_write_output(&base_path, Some(false))
            .expect("build dwelling");

        // Deposit -1000 W sensible + -200 W latent = -1200 W total cooling.
        // Reported thermal_output_w = -1200 W matches total port deposit.
        let mut eq = CoolingWithLatentEquipment::new("CoolingEq", -1000.0, -200.0, -1200.0);
        eq.init(&EquipmentConfig::default(), &dwelling.latest_env)
            .expect("init cooling equipment");

        replace_equipment_for_test(&mut dwelling, vec![Box::new(eq)]);

        dwelling
            .run_timestep(false)
            .expect("cooling equipment depositing -1000 W sensible + -200 W latent with thermal_output_w=-1200 W must match when latent is included in the port comparison");
    }

    #[test]
    fn thermal_consistency_detects_cooling_latent_mismatch() {
        let base_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/bestest/600.toml");
        let mut dwelling = Dwelling::from_toml_config_with_write_output(&base_path, Some(false))
            .expect("build dwelling");

        // Deposit -1000 W sensible + -200 W latent = -1200 W total cooling.
        // Reported thermal_output_w = -1000 W omits latent and under-reports.
        let mut eq = CoolingWithLatentEquipment::new("CoolingEq", -1000.0, -200.0, -1000.0);
        eq.init(&EquipmentConfig::default(), &dwelling.latest_env)
            .expect("init cooling equipment");

        replace_equipment_for_test(&mut dwelling, vec![Box::new(eq)]);

        let err = dwelling
            .run_timestep(false)
            .expect_err("thermal consistency must detect the -200 W latent mismatch when the equipment reports thermal_output_w=-1000 W but the ports receive -1200 W total");
        assert!(
            err.to_string().contains("per-zone thermal consistency"),
            "got: {err}"
        );
    }

    #[test]
    fn thermal_mode_override_dispatch_is_applied_same_timestep() {
        let base_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/bestest/600.toml");
        let mut dwelling = Dwelling::from_toml_config_with_write_output(&base_path, Some(false))
            .expect("build dwelling");

        let update_calls = Arc::new(AtomicUsize::new(0));
        let last_mode_code = Arc::new(AtomicU8::new(0));
        let thermal_eq = DispatchAwareThermalEquipment::new(
            "ThermalDispatchEq",
            Arc::clone(&update_calls),
            Arc::clone(&last_mode_code),
        );

        replace_equipment_for_test(&mut dwelling, vec![Box::new(thermal_eq)]);
        dwelling.apply_control(
            "ThermalDispatchEq",
            ControlSignal::ModeOverride {
                mode: OperatingMode::Heating,
            },
        );

        dwelling.run_timestep(false).expect("dwelling step");

        assert_eq!(
            update_calls.load(Ordering::Relaxed),
            2,
            "thermal equipment update_control must run before and after dispatch in the same step"
        );
        assert_eq!(
            last_mode_code.load(Ordering::Relaxed),
            1,
            "post-dispatch update_control must observe ModeOverride=Heating in the same step"
        );
    }

    #[test]
    fn invalid_signal_rejected_and_simulation_continues() {
        let base_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/bestest/600.toml");
        let mut dwelling = Dwelling::from_toml_config_with_write_output(&base_path, Some(false))
            .expect("build dwelling");

        dwelling.apply_control(
            "Ideal HVAC",
            ControlSignal::IdealCapacity {
                capacity_w: f64::NAN,
                degraded: false,
            },
        );

        let result = dwelling.run_timestep(false);
        assert!(
            result.is_ok(),
            "timestep should not panic on rejected signal"
        );

        let warnings = dwelling.take_warnings();
        let rejection = warnings
            .iter()
            .find(|w| w.contains("control apply failed for ") && w.contains("IdealCapacity"));
        assert!(
            rejection.is_some(),
            "expected a control-apply-failed warning, got: {warnings:?}"
        );
    }

    #[test]
    fn simulate_accumulates_steps_when_write_output_disabled() {
        let dir = tempfile::tempdir().expect("temp dir");
        let toml_path = dir.path().join("write-output-off.toml");

        fs::write(
            &toml_path,
            r#"building_id = 424242

[simulation]
start_time = "2024-01-15T00:00:00Z"
time_res_s = 60
duration_s = 600

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.8

[hvac]
equipment_name = "Furnace"
fuel = "electricity"
heating_capacity_kbtu_h = 30.0

[weather]
outdoor_temp_c = -10.0
dew_point_c = -5.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325

[schedule]
occupancy = 1.0

[output]
write_output = false
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
master_seed = 0
"#,
        )
        .expect("write synthetic TOML");

        let mut dwelling = Dwelling::from_toml_config(&toml_path).expect("build dwelling");
        let results = dwelling.simulate().expect("simulate");

        assert_eq!(
            results.steps.len(),
            10,
            "simulate() must still accumulate in-memory step results when write_output=false"
        );
        assert!(
            dwelling.flushed_batches().is_empty(),
            "write_output=false must not produce recorder batches"
        );
    }

    #[test]
    fn unregistered_critical_equipment_returns_err_with_equipment_name() {
        let dir = tempfile::tempdir().expect("temp dir");
        let toml_path = dir.path().join("unregistered-critical.toml");

        fs::write(
            &toml_path,
            r#"building_id = 424243

[simulation]
start_time = "2024-01-15T00:00:00Z"
time_res_s = 60
duration_s = 600

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.8

[hvac]
equipment_name = "MysteryBoiler9000"
fuel = "electricity"
heating_capacity_kbtu_h = 30.0

[weather]
outdoor_temp_c = -10.0
dew_point_c = -5.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325

[schedule]
occupancy = 1.0
"#,
        )
        .expect("write synthetic TOML");

        let result = Dwelling::from_toml_config(&toml_path);

        let err = match result {
            Ok(_) => panic!("unknown HVAC class must fail dwelling construction"),
            Err(err) => err,
        };
        let msg = err.to_string();
        assert!(
            msg.contains("MysteryBoiler9000"),
            "error must name missing equipment, got: {msg}"
        );
    }

    #[test]
    fn config_value_conversion_handles_scalars() {
        assert_eq!(
            json_value_to_config_value(&serde_json::json!(2.0)),
            Some(ConfigValue::Float(2.0))
        );
        assert_eq!(
            json_value_to_config_value(&serde_json::json!("x")),
            Some(ConfigValue::Text("x".to_string()))
        );
        assert_eq!(
            json_value_to_config_value(&serde_json::json!(true)),
            Some(ConfigValue::Bool(true))
        );
    }

    #[test]
    fn control_dispatcher_routes_by_tier_schedule_applied_first() {
        let mut dispatcher = ControlDispatcher::default();
        let signal_schedule = ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(20.0),
            cooling_setpoint_c: None,
            deadband_c: None,
        };
        let signal_grid = ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(18.0),
            cooling_setpoint_c: None,
            deadband_c: None,
        };

        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Test")),
            signal: signal_grid,
            priority: PriorityTier::Grid,
        });
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Test")),
            signal: signal_schedule,
            priority: PriorityTier::Schedule,
        });

        assert_eq!(dispatcher.by_tier[0].len(), 1);
        assert_eq!(dispatcher.by_tier[2].len(), 1);
        assert!(dispatcher.by_tier[1].is_empty());
        assert!(dispatcher.by_tier[3].is_empty());
    }

    #[test]
    fn control_dispatcher_drains_all_tiers_in_order() {
        let mut dispatcher = ControlDispatcher::default();
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("A")),
            signal: ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(20.0),
                cooling_setpoint_c: None,
                deadband_c: None,
            },
            priority: PriorityTier::Schedule,
        });
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("B")),
            signal: ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(21.0),
                cooling_setpoint_c: None,
                deadband_c: None,
            },
            priority: PriorityTier::Grid,
        });

        let mut warnings = WarningLog::new();
        let equipment: &mut [Box<dyn Equipment>] = &mut [];
        dispatcher.dispatch_into(equipment, &mut warnings, &mut 0);

        assert!(dispatcher.by_tier.iter().all(|q| q.is_empty()));
    }

    #[test]
    fn control_dispatcher_higher_priority_wins_over_lower() {
        let eq = TestEquipment::new("Heater", ControlCapabilities::POWER_SETPOINT);

        let mut dispatcher = ControlDispatcher::default();

        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Heater")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 1.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Heater")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 5.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Grid,
        });

        let mut warnings = WarningLog::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings, &mut 0);

        assert!(warnings.is_empty());
        assert_eq!(equipment[0].telemetry().get(tk::LAST_POWER_KW), Some(5.0));
    }

    #[test]
    fn control_dispatcher_safety_priority_wins_over_all() {
        let eq = TestEquipment::new("Heater", ControlCapabilities::POWER_SETPOINT);

        let mut dispatcher = ControlDispatcher::default();

        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Heater")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 1.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Heater")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 2.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::UserOverride,
        });
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Heater")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 3.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Grid,
        });
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Heater")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 0.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Safety,
        });

        let mut warnings = WarningLog::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings, &mut 0);

        assert!(warnings.is_empty());
        assert_eq!(equipment[0].telemetry().get(tk::LAST_POWER_KW), Some(0.0));
    }

    #[test]
    fn control_dispatcher_warns_on_missing_target_by_name() {
        let eq = TestEquipment::new("Heater", ControlCapabilities::POWER_SETPOINT);

        let mut dispatcher = ControlDispatcher::default();
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("NonExistent")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 1.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });

        let mut warnings = WarningLog::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings, &mut 0);

        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("control target not found by name"));
        assert!(warnings[0].contains("NonExistent"));
    }

    #[test]
    fn control_dispatcher_warns_on_missing_target_by_end_use() {
        let eq = TestEquipment::new("Heater", ControlCapabilities::POWER_SETPOINT);

        let mut dispatcher = ControlDispatcher::default();
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByEndUse(EndUse::BATTERY),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 1.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });

        let mut warnings = WarningLog::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings, &mut 0);

        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("control target not found by end-use"));
        assert!(warnings[0].contains("battery"));
    }

    #[test]
    fn control_dispatcher_warns_on_unsupported_signal() {
        let eq = TestEquipment::new("Heater", ControlCapabilities::empty());

        let mut dispatcher = ControlDispatcher::default();
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Heater")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 1.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });

        let mut warnings = WarningLog::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings, &mut 0);

        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("control apply failed"));
    }

    #[test]
    fn control_dispatcher_routes_to_custom_end_use() {
        // Create equipment with a custom end use
        let mut eq = TestEquipment::new("HPWH", ControlCapabilities::POWER_SETPOINT);
        eq.descriptor.end_use = EndUse::custom("heat_pump_water_heater");

        let mut dispatcher = ControlDispatcher::default();
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByEndUse(EndUse::custom("heat_pump_water_heater")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 2.5,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });

        let mut warnings = WarningLog::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings, &mut 0);

        // Should find the equipment and apply the signal
        assert!(warnings.is_empty(), "unexpected warnings: {:?}", warnings);
        assert_eq!(equipment[0].telemetry().get(tk::LAST_POWER_KW), Some(2.5));
    }

    #[test]
    fn control_dispatcher_custom_end_use_misses_different_custom() {
        // Equipment with one custom end use, dispatch to a different custom end use
        let mut eq = TestEquipment::new("HPWH", ControlCapabilities::POWER_SETPOINT);
        eq.descriptor.end_use = EndUse::custom("heat_pump_water_heater");

        let mut dispatcher = ControlDispatcher::default();
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByEndUse(EndUse::custom("ice_storage")), // Different custom type
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 1.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });

        let mut warnings = WarningLog::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings, &mut 0);

        // Should not find the equipment (different custom end use)
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("control target not found by end-use"));
        assert!(warnings[0].contains("ice_storage"));
    }

    #[test]
    fn control_dispatcher_warns_on_missing_custom_end_use() {
        let eq = TestEquipment::new("Heater", ControlCapabilities::POWER_SETPOINT);
        // Equipment has standard end use, dispatch targets custom end use

        let mut dispatcher = ControlDispatcher::default();
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByEndUse(EndUse::custom("novel_equipment_type")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 1.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });

        let mut warnings = WarningLog::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings, &mut 0);

        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("control target not found by end-use"));
        assert!(warnings[0].contains("novel_equipment_type"));
    }

    #[test]
    fn solver_feedback_collect_and_decide_dispatches_ideal_capacity() {
        use crate::Actor;
        use crate::actors::SolverFeedbackActor;

        let eq = TestIdealEquipment::new("HVAC", ZoneId(1), 20.0);
        let equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];

        let mut actor = SolverFeedbackActor::new();
        actor.set_dispatch_targets(compute_equipment_dispatch_targets(&equipment_refs(
            &equipment,
        )));

        // Simulate what the dwelling does: collect → decide
        // We can't call collect_and_solve without a real ThermalSolver,
        // but we can verify the full decide path by using the internal test helper.
        // The solver_feedback.rs unit tests cover collect_and_solve separately.
        actor.collect_and_solve_test(&equipment, |_zone, _target| 5000.0);

        let env = crate::actor::testing::test_env().build();
        let mut requests = Vec::new();
        actor.decide(&env, &mut requests);

        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].target,
            DispatchTarget::ByName(Arc::from("HVAC"))
        );
        assert_eq!(requests[0].priority, PriorityTier::Schedule);
        match &requests[0].signal {
            ControlSignal::IdealCapacity { capacity_w, .. } => {
                assert!((*capacity_w - 5000.0).abs() < 1e-9);
            }
            _ => panic!("expected IdealCapacity signal"),
        }
    }

    /// Test equipment that reports ideal_target() for solver feedback testing.
    struct TestIdealEquipment {
        descriptor: EquipmentDescriptor,
        telemetry: Telemetry,
        ideal_capacity_w: f64,
        ideal_zone: ZoneId,
        ideal_target_c: f64,
        core_output: CoreOutput,
    }

    impl TestIdealEquipment {
        fn new(name: &str, zone: ZoneId, target_c: f64) -> Self {
            Self {
                descriptor: EquipmentDescriptor {
                    id: EquipmentId(1),
                    name: name.to_string(),
                    end_use: EndUse::HVAC_HEATING,
                    equipment_type: Cow::Borrowed("TestIdealEquipment"),
                    zone: Some(zone),
                    fuel: FuelType::Electric,
                    stage: ExecutionStage::Thermal,
                    control_capabilities: ControlCapabilities::IDEAL_CAPACITY
                        | ControlCapabilities::THERMAL_SETPOINT,
                    core_capabilities: CoreCapabilities::empty(),
                    telemetry_fields: vec![TelemetryField {
                        name: tk::IDEAL_CAPACITY_W.to_string(),
                        unit: "W".to_string(),
                        description: "ideal capacity from solver".to_string(),
                    }],
                    zone_type: None,
                },
                telemetry: Telemetry::with_capacity(1),
                ideal_capacity_w: 0.0,
                ideal_zone: zone,
                ideal_target_c: target_c,
                core_output: CoreOutput::default(),
            }
        }
    }

    impl Equipment for TestIdealEquipment {
        fn descriptor(&self) -> &EquipmentDescriptor {
            &self.descriptor
        }

        fn rename(&mut self, name: String) {
            self.descriptor.name = name;
        }

        fn set_equipment_id(
            &mut self,
            id: EquipmentId,
        ) -> std::result::Result<(), hares_types::HaresError> {
            hares_equipment::apply_identity_write(self.is_initialized(), &mut self.descriptor, id)
        }

        fn ports(&self) -> &[PortDeclaration] {
            &[]
        }

        fn init(
            &mut self,
            _config: &EquipmentConfig,
            _env: &hares_types::EnvironmentState,
        ) -> std::result::Result<(), hares_types::HaresError> {
            self.telemetry
                .insert(tk::IDEAL_CAPACITY_W, self.ideal_capacity_w);
            Ok(())
        }

        fn update_control(&mut self, _env: &hares_types::EnvironmentState) -> OperatingMode {
            OperatingMode::Heating
        }

        fn step(
            &mut self,
            _env: &hares_types::EnvironmentState,
            _dt: Duration,
            _ports: &mut PortSlots,
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }

        fn telemetry(&self) -> &Telemetry {
            &self.telemetry
        }

        fn core_output(&self) -> &CoreOutput {
            &self.core_output
        }

        fn save_state(&self) -> std::result::Result<Vec<u8>, hares_types::HaresError> {
            Ok(vec![])
        }

        fn load_state(
            &mut self,
            _state: &[u8],
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }

        fn apply_signal(
            &mut self,
            signal: &ControlSignal,
        ) -> std::result::Result<(), hares_types::HaresError> {
            if let ControlSignal::IdealCapacity { capacity_w, .. } = signal {
                self.ideal_capacity_w = *capacity_w;
                self.telemetry.insert(tk::IDEAL_CAPACITY_W, *capacity_w);
            }
            Ok(())
        }

        fn ideal_target(&self) -> Option<(ZoneId, f64)> {
            Some((self.ideal_zone, self.ideal_target_c))
        }
    }

    #[test]
    fn solver_feedback_collect_decide_dispatch_full_pipeline() {
        use crate::Actor;
        use crate::actors::SolverFeedbackActor;

        // Set up equipment that returns ideal_target
        let mut eq = TestIdealEquipment::new("IdealHVAC", ZoneId(1), 20.0);
        let env = crate::actor::testing::test_env().build();
        eq.init(&EquipmentConfig::default(), &env).ok();

        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];

        // Set up actor with cached targets (same as dwelling does)
        let mut actor = SolverFeedbackActor::new();
        actor.set_dispatch_targets(compute_equipment_dispatch_targets(&equipment_refs(
            &equipment,
        )));

        // Step 1: collect_and_solve (mock solver returns 5000W)
        actor.collect_and_solve_test(&equipment, |_zone, _target| 5000.0);

        // Step 2: decide through Actor interface
        let mut requests = Vec::new();
        actor.decide(&env, &mut requests);

        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].target,
            DispatchTarget::ByName(Arc::from("IdealHVAC"))
        );
        assert_eq!(requests[0].priority, PriorityTier::Schedule);

        // Verify the IdealCapacity signal is NOT degraded (normal operation).
        match &requests[0].signal {
            ControlSignal::IdealCapacity {
                capacity_w,
                degraded,
            } => {
                assert!((*capacity_w - 5000.0).abs() < 1e-9);
                assert!(
                    !degraded,
                    "normal solver output must not be flagged as degraded"
                );
            }
            _ => panic!("expected IdealCapacity signal"),
        }

        // Step 3: dispatch to equipment
        let mut dispatcher = ControlDispatcher::default();
        for req in requests {
            dispatcher.queue(req);
        }
        let mut warnings = WarningLog::new();
        dispatcher.dispatch_into(&mut equipment, &mut warnings, &mut 0);

        assert!(warnings.is_empty(), "unexpected warnings: {:?}", warnings);
        assert!(
            (equipment[0]
                .telemetry()
                .get(tk::IDEAL_CAPACITY_W)
                .unwrap_or(0.0)
                - 5000.0)
                .abs()
                < 1e-9,
            "equipment should have received 5000W ideal capacity"
        );
    }

    /// When the solver feedback actor dispatches a degraded ideal capacity,
    /// the `degraded` flag must propagate through the signal to equipment.
    #[test]
    fn solver_feedback_degraded_capacity_propagates_to_signal() {
        use crate::Actor;
        use crate::actors::SolverFeedbackActor;

        let eq = TestIdealEquipment::new("IdealHVAC", ZoneId(1), 20.0);
        let equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];

        let mut actor = SolverFeedbackActor::new();
        actor.set_dispatch_targets(compute_equipment_dispatch_targets(&equipment_refs(
            &equipment,
        )));

        // Simulate solver returning a degraded (last-good) capacity.
        actor.push_pending_test(0, 4500.0, true);

        let env = crate::actor::testing::test_env().build();
        let mut requests = Vec::new();
        actor.decide(&env, &mut requests);

        assert_eq!(requests.len(), 1);
        match &requests[0].signal {
            ControlSignal::IdealCapacity {
                capacity_w,
                degraded,
            } => {
                assert!((*capacity_w - 4500.0).abs() < 1e-9);
                assert!(
                    *degraded,
                    "degraded flag must be true when capacity is a fallback"
                );
            }
            _ => panic!("expected IdealCapacity signal"),
        }
    }

    #[test]
    fn solver_feedback_multiple_equipment_dispatches_correctly() {
        use crate::Actor;
        use crate::actors::SolverFeedbackActor;

        let eq1 = TestIdealEquipment::new("HVAC_Zone1", ZoneId(1), 20.0);
        let eq2 = TestIdealEquipment::new("HVAC_Zone2", ZoneId(2), 22.0);
        let equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq1), Box::new(eq2)];

        let mut actor = SolverFeedbackActor::new();
        actor.set_dispatch_targets(compute_equipment_dispatch_targets(&equipment_refs(
            &equipment,
        )));
        actor.collect_and_solve_test(&equipment, |_zone, _target| 3000.0);

        let env = crate::actor::testing::test_env().build();
        let mut requests = Vec::new();
        actor.decide(&env, &mut requests);

        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[0].target,
            DispatchTarget::ByName(Arc::from("HVAC_Zone1"))
        );
        assert_eq!(
            requests[1].target,
            DispatchTarget::ByName(Arc::from("HVAC_Zone2"))
        );
    }

    #[test]
    fn solver_feedback_skips_equipment_without_ideal_target() {
        use crate::Actor;
        use crate::actors::SolverFeedbackActor;

        let non_ideal = TestEquipment::new("Battery", ControlCapabilities::POWER_SETPOINT);
        let ideal = TestIdealEquipment::new("HVAC", ZoneId(1), 20.0);
        let equipment: Vec<Box<dyn Equipment>> = vec![
            Box::new(non_ideal) as Box<dyn Equipment>,
            Box::new(ideal) as Box<dyn Equipment>,
        ];

        let mut actor = SolverFeedbackActor::new();
        actor.set_dispatch_targets(compute_equipment_dispatch_targets(&equipment_refs(
            &equipment,
        )));
        actor.collect_and_solve_test(&equipment, |_zone, _target| 7000.0);

        let env = crate::actor::testing::test_env().build();
        let mut requests = Vec::new();
        actor.decide(&env, &mut requests);

        assert_eq!(
            requests.len(),
            1,
            "only ideal equipment should produce a signal"
        );
        assert_eq!(
            requests[0].target,
            DispatchTarget::ByName(Arc::from("HVAC"))
        );
    }

    #[test]
    fn equipment_execution_order_precomputed_at_init() {
        // Verify that equipment_execution_order is computed once and stored
        let eq1 = TestEquipment::new("NonThermal1", ControlCapabilities::POWER_SETPOINT);
        let eq2 = TestEquipment::new("NonThermal2", ControlCapabilities::POWER_SETPOINT);

        let equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq1), Box::new(eq2)];
        let order = compute_equipment_execution_order(&equipment_refs(&equipment));

        // Order should match equipment length
        assert_eq!(order.len(), 2);
        // All indices should be present
        assert!(order.contains(&0));
        assert!(order.contains(&1));
    }

    #[test]
    fn equipment_dispatch_targets_precomputed_at_init() {
        let eq1 = TestEquipment::new("Equipment1", ControlCapabilities::POWER_SETPOINT);
        let eq2 = TestEquipment::new("Equipment2", ControlCapabilities::POWER_SETPOINT);

        let equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq1), Box::new(eq2)];
        let targets = compute_equipment_dispatch_targets(&equipment_refs(&equipment));

        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0], DispatchTarget::ByName(Arc::from("Equipment1")));
        assert_eq!(targets[1], DispatchTarget::ByName(Arc::from("Equipment2")));
    }

    #[test]
    fn equipment_caches_sorted_by_stage_rank() {
        // Thermal equipment should come after non-thermal
        let thermal_eq = TestIdealEquipment::new("ThermalHVAC", ZoneId(1), 20.0);
        let nonthermal_eq = TestEquipment::new("NonThermal", ControlCapabilities::POWER_SETPOINT);

        // Add in reverse order (thermal first)
        let equipment: Vec<Box<dyn Equipment>> = vec![
            Box::new(thermal_eq) as Box<dyn Equipment>,
            Box::new(nonthermal_eq) as Box<dyn Equipment>,
        ];
        let order = compute_equipment_execution_order(&equipment_refs(&equipment));

        // Non-thermal should come first (lower stage rank)
        let nonthermal_idx = equipment
            .iter()
            .position(|e| e.descriptor().stage == ExecutionStage::Independent)
            .expect("non-thermal equipment exists");
        let thermal_idx = equipment
            .iter()
            .position(|e| e.descriptor().stage == ExecutionStage::Thermal)
            .expect("thermal equipment exists");

        // Order should have non-thermal before thermal
        let pos_nonthermal = order.iter().position(|&i| i == nonthermal_idx).unwrap();
        let pos_thermal = order.iter().position(|&i| i == thermal_idx).unwrap();
        assert!(
            pos_nonthermal < pos_thermal,
            "non-thermal should execute before thermal"
        );
    }

    // -----------------------------------------------------------------------
    // Dispatch wiring: signal count preservation (no signal loss)
    // -----------------------------------------------------------------------

    #[test]
    fn dispatch_delivers_all_signals_no_loss() {
        let eq1 = TestEquipment::new("Eq1", ControlCapabilities::POWER_SETPOINT);
        let eq2 = TestEquipment::new("Eq2", ControlCapabilities::POWER_SETPOINT);

        let mut dispatcher = ControlDispatcher::default();
        // Queue 3 signals to 2 different equipment
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Eq1")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 1.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Eq2")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 2.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Eq1")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 3.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Grid,
        });

        let mut warnings = WarningLog::new();
        let mut delivered_count = 0u32;
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq1), Box::new(eq2)];
        dispatcher.begin_step();
        dispatcher.drain_tiers(
            &mut equipment,
            &mut warnings,
            &mut 0,
            |_, delivered, _, _| {
                if delivered {
                    delivered_count += 1;
                }
            },
        );

        assert!(warnings.is_empty(), "unexpected warnings: {:?}", warnings);
        assert_eq!(delivered_count, 3, "all 3 signals must be delivered");
        // Eq1 gets Grid (3.0) as last write, Eq2 gets Schedule (2.0)
        assert_eq!(equipment[0].telemetry().get(tk::LAST_POWER_KW), Some(3.0));
        assert_eq!(equipment[1].telemetry().get(tk::LAST_POWER_KW), Some(2.0));
    }

    // -----------------------------------------------------------------------
    // Dispatch wiring: same-tier same-target last-write-wins
    // -----------------------------------------------------------------------

    #[test]
    fn dispatch_same_tier_same_target_last_write_wins() {
        let eq = TestEquipment::new("Heater", ControlCapabilities::POWER_SETPOINT);

        let mut dispatcher = ControlDispatcher::default();
        // Two Schedule-tier signals to same equipment
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Heater")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 1.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Heater")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 9.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });

        let mut warnings = WarningLog::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings, &mut 0);

        assert!(warnings.is_empty());
        // Last queued signal in the same tier wins (FIFO within tier, last write wins)
        assert_eq!(equipment[0].telemetry().get(tk::LAST_POWER_KW), Some(9.0));
    }

    #[test]
    fn dispatch_same_tier_same_target_reversed_order_changes_winner() {
        let eq = TestEquipment::new("Heater", ControlCapabilities::POWER_SETPOINT);

        let mut dispatcher = ControlDispatcher::default();
        // Same two signals as the last_write_wins test, but queued in reverse order.
        // This demonstrates that the outcome depends on FIFO order (actor registration
        // order), not on signal magnitude or any composition strategy.
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Heater")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 9.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Heater")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 1.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });

        let mut warnings = WarningLog::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings, &mut 0);

        assert!(warnings.is_empty());
        // Now the 1.0 kW signal wins — it was queued last.
        assert_eq!(equipment[0].telemetry().get(tk::LAST_POWER_KW), Some(1.0));
    }

    #[test]
    fn dispatch_same_tier_same_target_three_signals_last_wins() {
        let eq = TestEquipment::new("Heater", ControlCapabilities::POWER_SETPOINT);

        let mut dispatcher = ControlDispatcher::default();
        for kw in [2.0, 7.0, 4.0] {
            dispatcher.queue(DispatchRequest {
                target: DispatchTarget::ByName(Arc::from("Heater")),
                signal: ControlSignal::PowerSetpoint {
                    active_power_kw: kw,
                    reactive_power_kvar: None,
                    min_soc: None,
                    max_soc: None,
                },
                priority: PriorityTier::Schedule,
            });
        }

        let mut warnings = WarningLog::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings, &mut 0);

        assert!(warnings.is_empty());
        // Last queued (4.0 kW) wins.
        assert_eq!(equipment[0].telemetry().get(tk::LAST_POWER_KW), Some(4.0));
    }

    #[cfg(feature = "observe")]
    #[test]
    fn dispatch_same_tier_same_target_observer_captures_conflict() {
        let eq = TestEquipment::new("Heater", ControlCapabilities::POWER_SETPOINT);

        let mut dispatcher = ControlDispatcher::default();
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Heater")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 1.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Heater")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 5.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });

        let mut warnings = WarningLog::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        let capture = dispatcher.dispatch_into_observed(&mut equipment, &mut warnings, &mut 0);

        assert!(warnings.is_empty());
        assert_eq!(capture.same_tier_conflicts.len(), 1);
        let conflict = &capture.same_tier_conflicts[0];
        assert_eq!(conflict.tier, PriorityTier::Schedule);
        assert!(matches!(&conflict.target, DispatchTarget::ByName(n) if n.as_ref() == "Heater"));
        assert_eq!(conflict.signals.len(), 2);
        // Last signal wins (overwrite-safe apply_control).
        assert_eq!(equipment[0].telemetry().get(tk::LAST_POWER_KW), Some(5.0));
        // Signals are recorded in FIFO order; the observer capture preserves both.
        assert_eq!(capture.signals.len(), 2);
    }

    #[cfg(feature = "observe")]
    #[test]
    fn dispatch_no_same_tier_conflict_observer_empty() {
        let eq1 = TestEquipment::new("Heater", ControlCapabilities::POWER_SETPOINT);
        let eq2 = TestEquipment::new("Battery", ControlCapabilities::POWER_SETPOINT);

        let mut dispatcher = ControlDispatcher::default();
        // Different targets — no conflict.
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Heater")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 1.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Battery")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 2.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });

        let mut warnings = WarningLog::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq1), Box::new(eq2)];
        let capture = dispatcher.dispatch_into_observed(&mut equipment, &mut warnings, &mut 0);

        assert!(warnings.is_empty());
        assert!(
            capture.same_tier_conflicts.is_empty(),
            "no conflict expected for different targets"
        );
    }

    #[cfg(feature = "observe")]
    #[test]
    fn dispatch_different_tier_same_target_no_same_tier_conflict() {
        let eq = TestEquipment::new(
            "Heater",
            ControlCapabilities::POWER_SETPOINT | ControlCapabilities::THERMAL_SETPOINT,
        );

        let mut dispatcher = ControlDispatcher::default();
        // Different tiers don't count as same-tier conflicts.
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Heater")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 1.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Heater")),
            signal: ControlSignal::ThermalSetpoint {
                heating_setpoint_c: Some(22.0),
                cooling_setpoint_c: None,
                deadband_c: None,
            },
            priority: PriorityTier::UserOverride,
        });

        let mut warnings = WarningLog::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        let capture = dispatcher.dispatch_into_observed(&mut equipment, &mut warnings, &mut 0);

        assert!(warnings.is_empty());
        // Cross-tier overwrite is normal priority-based dispatch, not a same-tier conflict.
        assert!(capture.same_tier_conflicts.is_empty());
        assert_eq!(capture.signals.len(), 2);
    }

    // -----------------------------------------------------------------------
    // Integration: two DR programs targeting the same battery
    // -----------------------------------------------------------------------

    #[test]
    fn two_dr_programs_targeting_same_battery_last_write_wins() {
        // Two DR programs at Grid tier emit different SOC setpoints for the
        // same battery. Last-write-wins determines the effective setpoint.
        let battery = TestEquipment::new(
            "Battery1",
            ControlCapabilities::POWER_SETPOINT | ControlCapabilities::SOC_TARGET,
        );

        let mut dispatcher = ControlDispatcher::default();
        // DR program A: request SOC target of 0.80
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Battery1")),
            signal: ControlSignal::SOCTarget {
                target_soc: 0.80,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Grid,
        });
        // DR program B: request SOC target of 0.30
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Battery1")),
            signal: ControlSignal::SOCTarget {
                target_soc: 0.30,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Grid,
        });

        let mut warnings = WarningLog::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(battery)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings, &mut 0);

        assert!(warnings.is_empty());
        // Last-queued signal (0.30) wins.
        assert_eq!(
            equipment[0].telemetry().get(tk::LAST_SOC_TARGET),
            Some(0.30)
        );
    }

    #[test]
    fn two_dr_programs_same_battery_reversed_order_changes_winner() {
        let battery = TestEquipment::new(
            "Battery1",
            ControlCapabilities::POWER_SETPOINT | ControlCapabilities::SOC_TARGET,
        );

        let mut dispatcher = ControlDispatcher::default();
        // Same signals, reversed: 0.30 first, 0.80 last → 0.80 wins.
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Battery1")),
            signal: ControlSignal::SOCTarget {
                target_soc: 0.30,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Grid,
        });
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Battery1")),
            signal: ControlSignal::SOCTarget {
                target_soc: 0.80,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Grid,
        });

        let mut warnings = WarningLog::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(battery)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings, &mut 0);

        assert!(warnings.is_empty());
        assert_eq!(
            equipment[0].telemetry().get(tk::LAST_SOC_TARGET),
            Some(0.80)
        );
    }

    #[cfg(feature = "observe")]
    #[test]
    fn two_dr_programs_same_battery_observer_captures_conflict() {
        let battery = TestEquipment::new(
            "Battery1",
            ControlCapabilities::POWER_SETPOINT | ControlCapabilities::SOC_TARGET,
        );

        let mut dispatcher = ControlDispatcher::default();
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Battery1")),
            signal: ControlSignal::SOCTarget {
                target_soc: 0.80,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Grid,
        });
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Battery1")),
            signal: ControlSignal::SOCTarget {
                target_soc: 0.30,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Grid,
        });

        let mut warnings = WarningLog::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(battery)];
        let capture = dispatcher.dispatch_into_observed(&mut equipment, &mut warnings, &mut 0);

        assert!(warnings.is_empty());
        assert_eq!(capture.same_tier_conflicts.len(), 1);
        let conflict = &capture.same_tier_conflicts[0];
        assert_eq!(conflict.tier, PriorityTier::Grid);
        assert!(matches!(&conflict.target, DispatchTarget::ByName(n) if n.as_ref() == "Battery1"));
        assert_eq!(conflict.signals.len(), 2);
        // Last signal (0.30) wins.
        assert_eq!(
            equipment[0].telemetry().get(tk::LAST_SOC_TARGET),
            Some(0.30)
        );
    }

    #[cfg(feature = "observe")]
    #[test]
    fn by_end_use_and_by_name_same_equipment_same_tier_observer_captures_conflict() {
        let mut eq = TestEquipment::new("Battery #1", ControlCapabilities::POWER_SETPOINT);
        eq.descriptor.end_use = EndUse::BATTERY;

        let mut dispatcher = ControlDispatcher::default();
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByEndUse(EndUse::BATTERY),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 5.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Battery #1")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 8.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });

        let mut warnings = WarningLog::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        let capture = dispatcher.dispatch_into_observed(&mut equipment, &mut warnings, &mut 0);

        assert!(warnings.is_empty());
        assert_eq!(
            capture.same_tier_conflicts.len(),
            1,
            "observer must detect mixed-variant (ByEndUse vs ByName) same-tier conflict"
        );
        let conflict = &capture.same_tier_conflicts[0];
        assert_eq!(conflict.tier, PriorityTier::Schedule);
        assert_eq!(conflict.signals.len(), 2);
        // Last-write-wins: the ByName signal (8.0) overwrites the ByEndUse signal (5.0).
        assert_eq!(equipment[0].telemetry().get(tk::LAST_POWER_KW), Some(8.0));
    }

    #[cfg(feature = "observe")]
    #[test]
    fn by_end_use_and_by_name_different_equipment_no_false_observer_conflict() {
        let mut battery = TestEquipment::new("Battery #1", ControlCapabilities::POWER_SETPOINT);
        battery.descriptor.end_use = EndUse::BATTERY;
        let mut heater = TestEquipment::new("Heater", ControlCapabilities::POWER_SETPOINT);
        heater.descriptor.end_use = EndUse::HVAC_HEATING;

        let mut dispatcher = ControlDispatcher::default();
        // ByEndUse for BATTERY targets Battery #1, ByName("Heater") targets Heater.
        // These are different equipment — no same-tier conflict.
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByEndUse(EndUse::BATTERY),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 5.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Heater")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 3.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });

        let mut warnings = WarningLog::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(battery), Box::new(heater)];
        let capture = dispatcher.dispatch_into_observed(&mut equipment, &mut warnings, &mut 0);

        assert!(warnings.is_empty());
        assert!(
            capture.same_tier_conflicts.is_empty(),
            "no conflict expected for ByEndUse vs ByName targeting different equipment"
        );
    }

    // -----------------------------------------------------------------------
    // Dispatch wiring: ByEndUse targets all matching equipment
    // -----------------------------------------------------------------------

    #[test]
    fn dispatch_by_end_use_targets_all_matching_equipment() {
        let mut eq1 = TestEquipment::new("Heater1", ControlCapabilities::POWER_SETPOINT);
        eq1.descriptor.end_use = EndUse::HVAC_HEATING;
        let mut eq2 = TestEquipment::new("Heater2", ControlCapabilities::POWER_SETPOINT);
        eq2.descriptor.end_use = EndUse::HVAC_HEATING;
        let mut eq3 = TestEquipment::new("Battery", ControlCapabilities::POWER_SETPOINT);
        eq3.descriptor.end_use = EndUse::BATTERY;

        let mut dispatcher = ControlDispatcher::default();
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByEndUse(EndUse::HVAC_HEATING),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 5.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });

        let mut warnings = WarningLog::new();
        let mut equipment: Vec<Box<dyn Equipment>> =
            vec![Box::new(eq1), Box::new(eq2), Box::new(eq3)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings, &mut 0);

        assert!(warnings.is_empty());
        // Both HVAC_HEATING equipment should receive the signal
        assert_eq!(equipment[0].telemetry().get(tk::LAST_POWER_KW), Some(5.0));
        assert_eq!(equipment[1].telemetry().get(tk::LAST_POWER_KW), Some(5.0));
        // Battery should NOT receive it
        assert_eq!(equipment[2].telemetry().get(tk::LAST_POWER_KW), None);
    }

    #[test]
    fn dispatch_demand_response_by_end_use_ev_routes_to_ev_equipment() {
        let mut ev = TestEquipment::new(
            "Home EV",
            ControlCapabilities::POWER_SETPOINT | ControlCapabilities::DEMAND_RESPONSE,
        );
        ev.descriptor.end_use = EndUse::EV;
        let mut battery = TestEquipment::new(
            "Home Battery",
            ControlCapabilities::POWER_SETPOINT | ControlCapabilities::DEMAND_RESPONSE,
        );
        battery.descriptor.end_use = EndUse::BATTERY;

        let mut dispatcher = ControlDispatcher::default();
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByEndUse(EndUse::EV),
            signal: ControlSignal::DemandResponse {
                level: DRLevel::Critical,
                duration_s: Some(3600.0),
            },
            priority: PriorityTier::Grid,
        });

        let mut warnings = WarningLog::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(ev), Box::new(battery)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings, &mut 0);

        assert!(warnings.is_empty());
        // EV should have received the DR signal (Critical = 3)
        assert_eq!(
            equipment[0].telemetry().get("last_dr_level"),
            Some(DRLevel::Critical as u8 as f64)
        );
        // Battery should NOT have received it
        assert_eq!(equipment[1].telemetry().get("last_dr_level"), None);
    }

    // -----------------------------------------------------------------------
    // Dispatch: cross-variant priority inversion prevention
    // -----------------------------------------------------------------------

    #[test]
    fn by_end_use_expanded_to_by_name_detects_cross_pass_priority_inversion() {
        // Regression: T-0181 — ByEndUse targets must expand to ByName so that
        // seen_targets entries are homogeneous and a lower-priority ByName
        // signal in a later pass is correctly rejected when a higher-priority
        // ByEndUse signal was applied to the same equipment in an earlier pass.
        let mut eq = TestEquipment::new("Battery #1", ControlCapabilities::POWER_SETPOINT);
        eq.descriptor.end_use = EndUse::BATTERY;

        let mut dispatcher = ControlDispatcher::default();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        let mut warnings = WarningLog::new();

        // Begin the step (reset cross-pass ledger).
        dispatcher.begin_step();

        // Pass 1: Safety-tier signal targeting ByEndUse(BATTERY)
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByEndUse(EndUse::BATTERY),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 10.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Safety,
        });
        dispatcher.drain_tiers(&mut equipment, &mut warnings, &mut 0, |_, _, _, _| {});
        // Pass 1: Safety setpoint applied.
        assert_eq!(equipment[0].telemetry().get(tk::LAST_POWER_KW), Some(10.0));
        assert!(warnings.is_empty());

        // Pass 2: Schedule-tier signal targeting the same battery by name.
        // This MUST be rejected because Safety > Schedule.
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Battery #1")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 1.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });
        let mut skipped = false;
        dispatcher.drain_tiers(&mut equipment, &mut warnings, &mut 0, |_, _, _, s| {
            if s {
                skipped = true;
            }
        });
        assert!(
            skipped,
            "Schedule signal should be skipped — higher-priority Safety signal already applied"
        );
        // The Safety setpoint must still be in effect.
        assert_eq!(
            equipment[0].telemetry().get(tk::LAST_POWER_KW),
            Some(10.0),
            "Safety setpoint must survive lower-priority overwrite"
        );
    }

    #[test]
    fn by_end_use_and_by_name_same_equipment_conflict_in_single_pass() {
        // Two Schedule-tier signals for the same equipment — one by end-use,
        // one by name — are correctly detected as conflicting through ByEndUse
        // expansion, and last-queued wins.
        let mut eq = TestEquipment::new("Battery #1", ControlCapabilities::POWER_SETPOINT);
        eq.descriptor.end_use = EndUse::BATTERY;

        let mut dispatcher = ControlDispatcher::default();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        let mut warnings = WarningLog::new();

        // Queue by end-use first, then by name. Both Schedule-tier.
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByEndUse(EndUse::BATTERY),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 5.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Battery #1")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 8.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });

        let mut delivered_count = 0;
        dispatcher.drain_tiers(&mut equipment, &mut warnings, &mut 0, |_, d, _, _| {
            if d {
                delivered_count += 1;
            }
        });
        assert!(warnings.is_empty());
        // Both signals should be "delivered" (the ByEndUse one applies, then
        // the ByName one overwrites). Last-write-wins: 8.0 is the final value.
        assert_eq!(delivered_count, 2);
        assert_eq!(
            equipment[0].telemetry().get(tk::LAST_POWER_KW),
            Some(8.0),
            "last-queued ByName signal must overwrite ByEndUse signal at same tier"
        );
    }

    #[test]
    fn by_end_use_expansion_only_affects_matching_equipment() {
        // Two batteries, one targeted by end-use. Only the matching one gets
        // the signal. The other is untouched and does not appear in seen_targets.
        let mut battery1 = TestEquipment::new("Battery #1", ControlCapabilities::POWER_SETPOINT);
        battery1.descriptor.end_use = EndUse::BATTERY;
        let mut battery2 = TestEquipment::new("Battery #2", ControlCapabilities::POWER_SETPOINT);
        battery2.descriptor.end_use = EndUse::BATTERY;
        let mut ev = TestEquipment::new("Home EV", ControlCapabilities::POWER_SETPOINT);
        ev.descriptor.end_use = EndUse::EV;

        let mut dispatcher = ControlDispatcher::default();
        let mut equipment: Vec<Box<dyn Equipment>> =
            vec![Box::new(battery1), Box::new(battery2), Box::new(ev)];
        let mut warnings = WarningLog::new();

        dispatcher.begin_step();

        // Pass 1: Safety signal to all batteries via ByEndUse.
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByEndUse(EndUse::BATTERY),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 10.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Safety,
        });
        dispatcher.drain_tiers(&mut equipment, &mut warnings, &mut 0, |_, _, _, _| {});

        // Both batteries got the signal.
        assert_eq!(equipment[0].telemetry().get(tk::LAST_POWER_KW), Some(10.0));
        assert_eq!(equipment[1].telemetry().get(tk::LAST_POWER_KW), Some(10.0));
        // EV was not affected.
        assert_eq!(equipment[2].telemetry().get(tk::LAST_POWER_KW), None);

        // Pass 2: Schedule signal to Battery #1 by name. Should be blocked
        // because Safety already targeted it via ByEndUse expansion.
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Battery #1")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 1.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });
        let mut skipped = false;
        dispatcher.drain_tiers(&mut equipment, &mut warnings, &mut 0, |_, _, _, s| {
            if s {
                skipped = true;
            }
        });
        assert!(
            skipped,
            "Schedule signal for Battery #1 must be skipped — Safety already applied"
        );

        // Battery #1 still has Safety value.
        assert_eq!(equipment[0].telemetry().get(tk::LAST_POWER_KW), Some(10.0));
        // Battery #2 still has Safety value.
        assert_eq!(equipment[1].telemetry().get(tk::LAST_POWER_KW), Some(10.0));

        // Pass 3: Schedule signal to Battery #2 by name. Also blocked.
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Battery #2")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 2.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });
        let mut skipped = false;
        dispatcher.drain_tiers(&mut equipment, &mut warnings, &mut 0, |_, _, _, s| {
            if s {
                skipped = true;
            }
        });
        assert!(
            skipped,
            "Schedule signal for Battery #2 must also be skipped"
        );

        // Both batteries still at Safety value.
        assert_eq!(equipment[0].telemetry().get(tk::LAST_POWER_KW), Some(10.0));
        assert_eq!(equipment[1].telemetry().get(tk::LAST_POWER_KW), Some(10.0));
    }

    // -----------------------------------------------------------------------
    // Dispatch wiring: queues are drained each dispatch (no carryover)
    // -----------------------------------------------------------------------

    #[test]
    fn dispatch_queues_drained_no_carryover_between_steps() {
        let eq = TestEquipment::new("Heater", ControlCapabilities::POWER_SETPOINT);

        let mut dispatcher = ControlDispatcher::default();
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Heater")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 5.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });

        let mut warnings = WarningLog::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];

        // First dispatch
        dispatcher.dispatch_into(&mut equipment, &mut warnings, &mut 0);
        assert_eq!(equipment[0].telemetry().get(tk::LAST_POWER_KW), Some(5.0));

        // Second dispatch with nothing queued -- queues should be empty
        let mut delivered_count = 0u32;
        dispatcher.begin_step();
        dispatcher.drain_tiers(
            &mut equipment,
            &mut warnings,
            &mut 0,
            |_, delivered, _, _| {
                if delivered {
                    delivered_count += 1;
                }
            },
        );
        assert_eq!(
            delivered_count, 0,
            "no signals should be delivered on second dispatch"
        );
    }

    // -----------------------------------------------------------------------
    // Solver feedback: collect→decide→dispatch→equipment full round-trip
    // with signal count verification
    // -----------------------------------------------------------------------

    #[test]
    fn solver_feedback_signal_count_matches_ideal_equipment_count() {
        use crate::Actor;
        use crate::actors::SolverFeedbackActor;

        // 2 ideal + 1 non-ideal = expect exactly 2 signals
        let ideal1 = TestIdealEquipment::new("HVAC_1", ZoneId(1), 20.0);
        let ideal2 = TestIdealEquipment::new("HVAC_2", ZoneId(2), 22.0);
        let battery = TestEquipment::new("Battery", ControlCapabilities::POWER_SETPOINT);

        let equipment: Vec<Box<dyn Equipment>> =
            vec![Box::new(ideal1), Box::new(ideal2), Box::new(battery)];

        let mut actor = SolverFeedbackActor::new();
        actor.set_dispatch_targets(compute_equipment_dispatch_targets(&equipment_refs(
            &equipment,
        )));
        actor.collect_and_solve_test(
            &equipment,
            |zone, _target_c| {
                if zone == ZoneId(1) { 3000.0 } else { -2000.0 }
            },
        );

        let env = crate::actor::testing::test_env().build();
        let mut requests = Vec::new();
        actor.decide(&env, &mut requests);

        assert_eq!(requests.len(), 2, "exactly 2 ideal equipment → 2 signals");
        // Verify correct zone→capacity mapping
        match &requests[0].signal {
            ControlSignal::IdealCapacity { capacity_w, .. } => {
                assert!((capacity_w - 3000.0).abs() < 1e-9, "zone 1 → 3000W");
            }
            _ => panic!("expected IdealCapacity"),
        }
        match &requests[1].signal {
            ControlSignal::IdealCapacity { capacity_w, .. } => {
                assert!((capacity_w - (-2000.0)).abs() < 1e-9, "zone 2 → -2000W");
            }
            _ => panic!("expected IdealCapacity"),
        }
    }

    // -----------------------------------------------------------------------
    // Full pipeline: actor→dispatch→equipment with IdealCapacity + override
    // -----------------------------------------------------------------------

    #[test]
    fn solver_feedback_signal_overridden_by_higher_priority_actor() {
        use crate::Actor;
        use crate::actors::SolverFeedbackActor;

        let mut eq = TestIdealEquipment::new("HVAC", ZoneId(1), 20.0);
        let env = crate::actor::testing::test_env().build();
        eq.init(&EquipmentConfig::default(), &env).ok();

        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];

        // Step 1: SolverFeedback emits IdealCapacity at Schedule priority
        let mut actor = SolverFeedbackActor::new();
        actor.set_dispatch_targets(compute_equipment_dispatch_targets(&equipment_refs(
            &equipment,
        )));
        actor.collect_and_solve_test(&equipment, |_, _| 5000.0);

        let mut requests = Vec::new();
        actor.decide(&env, &mut requests);
        assert_eq!(requests.len(), 1);

        // Step 2: User actor emits override at Grid priority (e.g., DR curtailment)
        requests.push(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("HVAC")),
            signal: ControlSignal::IdealCapacity {
                capacity_w: 0.0,
                degraded: false,
            },
            priority: PriorityTier::Grid,
        });

        // Step 3: Queue both and dispatch
        let mut dispatcher = ControlDispatcher::default();
        for req in requests {
            dispatcher.queue(req);
        }
        let mut warnings = WarningLog::new();
        dispatcher.dispatch_into(&mut equipment, &mut warnings, &mut 0);

        // Grid priority (0W) should overwrite Schedule priority (5000W)
        assert!(warnings.is_empty());
        assert!(
            (equipment[0]
                .telemetry()
                .get(tk::IDEAL_CAPACITY_W)
                .unwrap_or(999.0)
                - 0.0)
                .abs()
                < 1e-9,
            "Grid priority override should zero out the ideal capacity"
        );
    }

    // -----------------------------------------------------------------------
    // Dispatcher correctly handles equipment apply_control errors
    // -----------------------------------------------------------------------

    #[test]
    fn dispatch_continues_after_one_equipment_rejects_signal() {
        // eq1 has no capabilities (rejects all), eq2 accepts PowerSetpoint
        let eq1 = TestEquipment::new("NoCapEq", ControlCapabilities::empty());
        let mut eq2 = TestEquipment::new("CapEq", ControlCapabilities::POWER_SETPOINT);
        eq2.descriptor.end_use = EndUse::OTHER;
        // Both have same end_use (OTHER) from TestEquipment default

        let mut dispatcher = ControlDispatcher::default();
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByEndUse(EndUse::OTHER),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 7.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            },
            priority: PriorityTier::Schedule,
        });

        let mut warnings = WarningLog::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq1), Box::new(eq2)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings, &mut 0);

        // eq1 should generate a warning but eq2 should still receive the signal
        assert_eq!(warnings.len(), 1, "one warning for rejected signal");
        assert!(warnings[0].contains("control apply failed"));
        assert_eq!(
            equipment[1].telemetry().get(tk::LAST_POWER_KW),
            Some(7.0),
            "second equipment should still receive signal despite first rejecting"
        );
    }

    // ---------------------------------------------------------------
    // SeedableTestEquipment -- TestEquipment + optional ActorSeed
    // ---------------------------------------------------------------

    struct SeedableTestEquipment {
        inner: TestEquipment,
        seed: Option<ActorSeed>,
    }

    impl SeedableTestEquipment {
        fn new(name: &str, seed: Option<ActorSeed>) -> Self {
            Self {
                inner: TestEquipment::new(name, ControlCapabilities::POWER_SETPOINT),
                seed,
            }
        }
    }

    impl Equipment for SeedableTestEquipment {
        fn descriptor(&self) -> &EquipmentDescriptor {
            self.inner.descriptor()
        }
        fn rename(&mut self, name: String) {
            self.inner.rename(name);
        }
        fn set_equipment_id(
            &mut self,
            id: EquipmentId,
        ) -> std::result::Result<(), hares_types::HaresError> {
            self.inner.set_equipment_id(id)
        }
        fn ports(&self) -> &[PortDeclaration] {
            self.inner.ports()
        }
        fn init(
            &mut self,
            config: &EquipmentConfig,
            env: &hares_types::EnvironmentState,
        ) -> std::result::Result<(), hares_types::HaresError> {
            self.inner.init(config, env)
        }
        fn update_control(&mut self, env: &hares_types::EnvironmentState) -> OperatingMode {
            self.inner.update_control(env)
        }
        fn step(
            &mut self,
            env: &hares_types::EnvironmentState,
            dt: Duration,
            ports: &mut PortSlots,
        ) -> std::result::Result<(), hares_types::HaresError> {
            self.inner.step(env, dt, ports)
        }
        fn telemetry(&self) -> &Telemetry {
            self.inner.telemetry()
        }
        fn core_output(&self) -> &CoreOutput {
            self.inner.core_output()
        }
        fn save_state(&self) -> std::result::Result<Vec<u8>, hares_types::HaresError> {
            self.inner.save_state().map_err(|e| {
                hares_types::HaresError::Equipment(format!("delegate save_state: {e}"))
            })
        }
        fn load_state(&mut self, state: &[u8]) -> std::result::Result<(), hares_types::HaresError> {
            self.inner.load_state(state)
        }
        fn apply_signal(
            &mut self,
            signal: &ControlSignal,
        ) -> std::result::Result<(), hares_types::HaresError> {
            self.inner.apply_signal(signal)
        }
        fn actor_seed(&self) -> Option<ActorSeed> {
            self.seed.clone()
        }
    }

    // Minimal actor for testing ordering and idempotency.
    struct StubActor {
        name: String,
    }
    impl crate::Actor for StubActor {
        fn name(&self) -> &str {
            &self.name
        }
        fn decide(
            &mut self,
            _env: &hares_types::EnvironmentState,
            _out: &mut Vec<hares_control::DispatchRequest>,
        ) {
        }
    }

    // Actor with checkpointable mutable state, published as telemetry so a
    // checkpoint round-trip can be asserted through the dwelling's public
    // `telemetry()` surface rather than by reaching into the actor.
    struct StatefulStubActor {
        name: String,
        counter: u32,
        telemetry: Telemetry,
    }
    impl StatefulStubActor {
        fn new(name: &str, counter: u32) -> Self {
            let mut telemetry = Telemetry::default();
            telemetry.insert("counter", counter as f64);
            Self {
                name: name.to_string(),
                counter,
                telemetry,
            }
        }
    }
    impl crate::Actor for StatefulStubActor {
        fn name(&self) -> &str {
            &self.name
        }
        fn telemetry(&self) -> Option<&Telemetry> {
            Some(&self.telemetry)
        }
        fn decide(
            &mut self,
            _env: &hares_types::EnvironmentState,
            _out: &mut Vec<hares_control::DispatchRequest>,
        ) {
        }
        fn save_state(&self) -> std::result::Result<Vec<u8>, HaresError> {
            postcard::to_allocvec(&self.counter)
                .map_err(|e| HaresError::Io(format!("StatefulStubActor save_state: {e}")))
        }
        fn load_state(&mut self, data: &[u8]) -> std::result::Result<(), HaresError> {
            if data.is_empty() {
                return Ok(());
            }
            self.counter = postcard::from_bytes(data)
                .map_err(|e| HaresError::Io(format!("StatefulStubActor load_state: {e}")))?;
            self.telemetry.insert("counter", self.counter as f64);
            Ok(())
        }
    }

    // Actor that always reports unhealthy, used to verify that the dwelling
    // health check catches actor errors after decide().
    struct UnhealthyStubActor {
        name: String,
    }
    impl crate::Actor for UnhealthyStubActor {
        fn name(&self) -> &str {
            &self.name
        }
        fn healthy(&self) -> bool {
            false
        }
        fn decide(
            &mut self,
            _env: &hares_types::EnvironmentState,
            _out: &mut Vec<hares_control::DispatchRequest>,
        ) {
        }
    }

    // The panic relies on the actor-health invariant, which is gated the
    // same way as the check itself in run_timestep.
    #[cfg(test)]
    #[test]
    fn run_timestep_panics_on_unhealthy_actor() {
        let (_dir, toml_path) = temp_toml("unhealthy_actor");
        write_minimal_toml(&toml_path);

        let mut dwelling = Dwelling::from_toml_config(&toml_path).expect("build dwelling");
        let unhealthy = UnhealthyStubActor {
            name: "UnhealthyActor".to_string(),
        };
        dwelling.add_actor(Box::new(unhealthy)).unwrap();

        // The dwelling debug_assert should fire because the actor reports
        // unhealthy after decide(). In debug builds (tests), this panics.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            dwelling.run_timestep(false).unwrap();
        }));
        assert!(
            result.is_err(),
            "run_timestep should panic when an actor is unhealthy"
        );
    }

    // ---------------------------------------------------------------
    // auto_register tests
    // ---------------------------------------------------------------

    /// `build_actors_from_seeds` with no equipment ids, 24 steps per day,
    /// a tariff exactly when a price schedule is given, no rebuilt drivers,
    /// and EV driver streams starting at the first.
    fn built_in_actors(
        equipment: &[Box<dyn Equipment>],
        existing: &[Box<dyn Actor>],
        price_schedule: Option<Arc<[f64]>>,
        rng: &ChaCha8Rng,
    ) -> Vec<Box<dyn Actor>> {
        let existing: Vec<&dyn Actor> = existing.iter().map(AsRef::as_ref).collect();
        build_actors_from_seeds(
            equipment,
            &existing,
            ActorPricing {
                has_tariff: price_schedule.is_some(),
                price_schedule,
                steps_per_day: 24,
            },
            &HashMap::new(),
            &mut ActorSeedState {
                rng,
                next_stream: 0,
                rebuilt: &[],
            },
        )
        .expect("build the built-in actors")
    }

    #[test]
    fn auto_register_bms_actor() {
        let eq: Box<dyn Equipment> = Box::new(SeedableTestEquipment::new(
            "Battery1",
            Some(ActorSeed::Battery {
                bms_mode: BmsMode::SelfConsumption {
                    min_soc: 0.15,
                    max_soc: 0.95,
                    solar_only_charging: false,
                    surplus_deadband_kw: 0.0,
                },
                grid_export_rule: hares_types::GridExportRule::Unrestricted,
                max_charge_kw: 5.0,
                max_discharge_kw: 5.0,
                min_dwell_steps: 0,
            }),
        ));

        let actors = built_in_actors(
            &[eq],
            &[],
            Some(Arc::from(vec![0.10; 24])),
            &derive_dwelling_rng(0, 0),
        );
        assert_eq!(actors.len(), 1);
        assert_eq!(actors[0].name(), "BatteryManagementActor:Battery1");
    }

    #[test]
    fn auto_register_ev_actor() {
        let eq: Box<dyn Equipment> = Box::new(SeedableTestEquipment::new(
            "EV1",
            Some(ActorSeed::Ev {
                strategy: ChargingStrategy::Nightly {
                    off_peak_start_hour: 23.0,
                    off_peak_end_hour: 6.0,
                    target_soc: 0.9,
                },
                plug_in_policy: hares_types::PlugInPolicy::Always,
                capacity_kwh: 60.0,
                max_charge_kw: 7.6,
                fuel_economy_kwh_per_mi: 0.3,
            }),
        ));

        let actors = built_in_actors(
            &[eq],
            &[],
            Some(Arc::from(vec![0.10; 24])),
            &derive_dwelling_rng(0, 0),
        );
        assert_eq!(actors.len(), 1);
        assert_eq!(actors[0].name(), "EvDriver:EV1");
    }

    #[test]
    fn manual_mode_no_bms_actor() {
        // Equipment with no ActorSeed (simulates BmsMode::Manual)
        let eq: Box<dyn Equipment> = Box::new(SeedableTestEquipment::new("Battery1", None));

        let actors = built_in_actors(&[eq], &[], None, &derive_dwelling_rng(0, 0));
        assert!(actors.is_empty());
    }

    #[test]
    fn add_actor_rejects_duplicate_names() {
        // Two same-named actors would both dispatch every step — mirroring
        // add_equipment's duplicate guard, the second registration is a loud
        // error, not a silent double-driver.
        let (_dir, toml_path) = temp_toml("dup_actor");
        write_minimal_toml(&toml_path);
        let mut dwelling = Dwelling::from_toml_config(&toml_path).expect("build dwelling");

        dwelling
            .add_actor(Box::new(StubActor {
                name: "MyActor".to_string(),
            }))
            .unwrap();
        let err = dwelling
            .add_actor(Box::new(StubActor {
                name: "MyActor".to_string(),
            }))
            .expect_err("duplicate actor name must be rejected");
        assert!(
            format!("{err:?}").contains("MyActor"),
            "error must name the duplicate, got {err:?}"
        );
    }

    #[test]
    fn auto_register_ev_idempotent() {
        // An actor already named EvDriver:<equipment> (e.g. one attached
        // manually via add_ev_with_driver) must suppress the built-in, so a
        // later auto_register_actors re-run (set_tariff) cannot attach a
        // second, competing driver to the same EV.
        let eq: Box<dyn Equipment> = Box::new(SeedableTestEquipment::new(
            "EV1",
            Some(ActorSeed::Ev {
                strategy: ChargingStrategy::Immediate { target_soc: 1.0 },
                plug_in_policy: hares_types::PlugInPolicy::Always,
                capacity_kwh: 60.0,
                max_charge_kw: 7.6,
                fuel_economy_kwh_per_mi: 0.3,
            }),
        ));

        let existing_actor: Box<dyn crate::Actor> = Box::new(StubActor {
            name: "EvDriver:EV1".to_string(),
        });
        let existing: Vec<Box<dyn crate::Actor>> = vec![existing_actor];

        let actors = built_in_actors(&[eq], &existing, None, &derive_dwelling_rng(0, 0));
        assert!(
            actors.is_empty(),
            "duplicate EvDriver should not be registered"
        );
    }

    #[test]
    fn auto_register_ev_dedup_is_per_equipment() {
        // EV1 already has a driver attached under the canonical name; EV2
        // does not. A re-registration must build only EV2's driver: dedup
        // is keyed on the equipment name, so one EV's existing driver must
        // not suppress another EV's built-in one.
        let seed = || {
            Some(ActorSeed::Ev {
                strategy: ChargingStrategy::Immediate { target_soc: 1.0 },
                plug_in_policy: hares_types::PlugInPolicy::Always,
                capacity_kwh: 60.0,
                max_charge_kw: 7.6,
                fuel_economy_kwh_per_mi: 0.3,
            })
        };
        let eq1: Box<dyn Equipment> = Box::new(SeedableTestEquipment::new("EV1", seed()));
        let eq2: Box<dyn Equipment> = Box::new(SeedableTestEquipment::new("EV2", seed()));
        let existing: Vec<Box<dyn crate::Actor>> = vec![Box::new(StubActor {
            name: "EvDriver:EV1".to_string(),
        })];

        let actors = built_in_actors(&[eq1, eq2], &existing, None, &derive_dwelling_rng(0, 0));
        let names: Vec<&str> = actors.iter().map(|a| a.name()).collect();
        assert_eq!(
            names,
            vec!["EvDriver:EV2"],
            "only the EV without an existing driver should get one"
        );
    }

    #[test]
    fn tou_without_tariff_warns_and_falls_back() {
        let eq: Box<dyn Equipment> = Box::new(SeedableTestEquipment::new(
            "Battery1",
            Some(ActorSeed::Battery {
                bms_mode: BmsMode::TimeOfUseOptimization {
                    reserve_soc: 0.2,
                    charge_threshold_percentile: 0.25,
                    discharge_threshold_percentile: 0.75,
                    solar_only_charging: false,
                    price_deadband: 0.0,
                    min_duration_steps: None,
                },
                grid_export_rule: hares_types::GridExportRule::Unrestricted,
                max_charge_kw: 5.0,
                max_discharge_kw: 5.0,
                min_dwell_steps: 0,
            }),
        ));

        // No tariff: has_tariff=false, price_schedule=None
        let actors = built_in_actors(&[eq], &[], None, &derive_dwelling_rng(0, 0));
        // Should still register an actor (with fallback to SelfConsumption)
        assert_eq!(actors.len(), 1);
        assert_eq!(actors[0].name(), "BatteryManagementActor:Battery1");
    }

    #[test]
    fn tou_aware_ev_without_tariff_warns() {
        let eq: Box<dyn Equipment> = Box::new(SeedableTestEquipment::new(
            "EV1",
            Some(ActorSeed::Ev {
                strategy: ChargingStrategy::TouAware {
                    target_soc: 0.9,
                    departure_schedule: vec![],
                    charge_buffer_hours: 2.0,
                },
                plug_in_policy: hares_types::PlugInPolicy::Always,
                capacity_kwh: 60.0,
                max_charge_kw: 7.6,
                fuel_economy_kwh_per_mi: 0.3,
            }),
        ));

        // No tariff: falls back to Immediate
        let actors = built_in_actors(&[eq], &[], None, &derive_dwelling_rng(0, 0));
        assert_eq!(actors.len(), 1);
        assert_eq!(actors[0].name(), "EvDriver:EV1");
    }

    #[test]
    fn actor_order_before_user_actors() {
        let eq: Box<dyn Equipment> = Box::new(SeedableTestEquipment::new(
            "Battery1",
            Some(ActorSeed::Battery {
                bms_mode: BmsMode::SelfConsumption {
                    min_soc: 0.15,
                    max_soc: 0.95,
                    solar_only_charging: false,
                    surplus_deadband_kw: 0.0,
                },
                grid_export_rule: hares_types::GridExportRule::Unrestricted,
                max_charge_kw: 5.0,
                max_discharge_kw: 5.0,
                min_dwell_steps: 0,
            }),
        ));

        let user_actor: Box<dyn crate::Actor> = Box::new(StubActor {
            name: "UserActor".to_string(),
        });
        let existing: Vec<Box<dyn crate::Actor>> = vec![user_actor];

        let built_in = built_in_actors(&[eq], &existing, None, &derive_dwelling_rng(0, 0));
        assert_eq!(built_in.len(), 1);
        assert_eq!(built_in[0].name(), "BatteryManagementActor:Battery1");
        // Caller (auto_register_actors) prepends built_in before existing.
        // Verify existing user actor is not among built_in.
        assert!(built_in.iter().all(|a| a.name() != "UserActor"));
    }

    #[test]
    fn auto_register_idempotent() {
        let eq: Box<dyn Equipment> = Box::new(SeedableTestEquipment::new(
            "Battery1",
            Some(ActorSeed::Battery {
                bms_mode: BmsMode::SelfConsumption {
                    min_soc: 0.15,
                    max_soc: 0.95,
                    solar_only_charging: false,
                    surplus_deadband_kw: 0.0,
                },
                grid_export_rule: hares_types::GridExportRule::Unrestricted,
                max_charge_kw: 5.0,
                max_discharge_kw: 5.0,
                min_dwell_steps: 0,
            }),
        ));

        // Simulate existing actor with same name
        let existing_actor: Box<dyn crate::Actor> = Box::new(StubActor {
            name: "BatteryManagementActor:Battery1".to_string(),
        });
        let existing: Vec<Box<dyn crate::Actor>> = vec![existing_actor];

        let built_in = built_in_actors(&[eq], &existing, None, &derive_dwelling_rng(0, 0));
        assert!(
            built_in.is_empty(),
            "duplicate actor should not be registered"
        );
    }

    #[test]
    fn auto_register_multiple_batteries() {
        let eq1: Box<dyn Equipment> = Box::new(SeedableTestEquipment::new(
            "Battery1",
            Some(ActorSeed::Battery {
                bms_mode: BmsMode::SelfConsumption {
                    min_soc: 0.15,
                    max_soc: 0.95,
                    solar_only_charging: false,
                    surplus_deadband_kw: 0.0,
                },
                grid_export_rule: hares_types::GridExportRule::Unrestricted,
                max_charge_kw: 5.0,
                max_discharge_kw: 5.0,
                min_dwell_steps: 0,
            }),
        ));
        let eq2: Box<dyn Equipment> = Box::new(SeedableTestEquipment::new(
            "Battery2",
            Some(ActorSeed::Battery {
                bms_mode: BmsMode::BackupReserve {
                    target_soc: 1.0,
                    charge_from_grid: true,
                    charge_rate_fraction: 0.5,
                    soc_deadband: 0.0,
                },
                grid_export_rule: hares_types::GridExportRule::Disabled,
                max_charge_kw: 3.0,
                max_discharge_kw: 3.0,
                min_dwell_steps: 0,
            }),
        ));

        let actors = built_in_actors(&[eq1, eq2], &[], None, &derive_dwelling_rng(0, 0));
        assert_eq!(actors.len(), 2);
        assert_eq!(actors[0].name(), "BatteryManagementActor:Battery1");
        assert_eq!(actors[1].name(), "BatteryManagementActor:Battery2");
    }

    #[test]
    fn auto_register_multiple_evs() {
        let eq1: Box<dyn Equipment> = Box::new(SeedableTestEquipment::new(
            "EV1",
            Some(ActorSeed::Ev {
                strategy: ChargingStrategy::Nightly {
                    off_peak_start_hour: 23.0,
                    off_peak_end_hour: 6.0,
                    target_soc: 0.9,
                },
                plug_in_policy: hares_types::PlugInPolicy::Always,
                capacity_kwh: 60.0,
                max_charge_kw: 7.6,
                fuel_economy_kwh_per_mi: 0.3,
            }),
        ));
        let eq2: Box<dyn Equipment> = Box::new(SeedableTestEquipment::new(
            "EV2",
            Some(ActorSeed::Ev {
                strategy: ChargingStrategy::LowSoc {
                    threshold: 0.3,
                    target_soc: 0.8,
                },
                plug_in_policy: hares_types::PlugInPolicy::Always,
                capacity_kwh: 75.0,
                max_charge_kw: 11.5,
                fuel_economy_kwh_per_mi: 0.28,
            }),
        ));

        let actors = built_in_actors(&[eq1, eq2], &[], None, &derive_dwelling_rng(0, 0));
        assert_eq!(actors.len(), 2);
        assert_eq!(actors[0].name(), "EvDriver:EV1");
        assert_eq!(actors[1].name(), "EvDriver:EV2");
    }

    #[test]
    fn v2g_without_tariff_falls_back_to_immediate() {
        let eq: Box<dyn Equipment> = Box::new(SeedableTestEquipment::new(
            "EV1",
            Some(ActorSeed::Ev {
                strategy: ChargingStrategy::V2G {
                    min_soc: 0.3,
                    max_export_kw: 5.0,
                    price_threshold: 0.15,
                },
                plug_in_policy: hares_types::PlugInPolicy::Always,
                capacity_kwh: 60.0,
                max_charge_kw: 7.6,
                fuel_economy_kwh_per_mi: 0.3,
            }),
        ));

        let actors = built_in_actors(&[eq], &[], None, &derive_dwelling_rng(0, 0));
        assert_eq!(actors.len(), 1);
        assert_eq!(actors[0].name(), "EvDriver:EV1");
    }

    #[test]
    fn ev_driver_similar_names_get_unique_rng_streams() {
        let eq1: Box<dyn Equipment> = Box::new(SeedableTestEquipment::new(
            "EV_001",
            Some(ActorSeed::Ev {
                strategy: ChargingStrategy::Immediate { target_soc: 0.9 },
                plug_in_policy: hares_types::PlugInPolicy::Always,
                capacity_kwh: 60.0,
                max_charge_kw: 7.6,
                fuel_economy_kwh_per_mi: 0.3,
            }),
        ));
        let eq2: Box<dyn Equipment> = Box::new(SeedableTestEquipment::new(
            "EV_002",
            Some(ActorSeed::Ev {
                strategy: ChargingStrategy::Immediate { target_soc: 0.9 },
                plug_in_policy: hares_types::PlugInPolicy::Always,
                capacity_kwh: 60.0,
                max_charge_kw: 7.6,
                fuel_economy_kwh_per_mi: 0.3,
            }),
        ));

        let actors = built_in_actors(&[eq1, eq2], &[], None, &derive_dwelling_rng(42, 1));
        assert_eq!(actors.len(), 2);
        assert_eq!(actors[0].name(), "EvDriver:EV_001");
        assert_eq!(actors[1].name(), "EvDriver:EV_002");

        let pair0 = actors[0]
            .rng_pair()
            .expect("EvDriverActor must report its RNG pair");
        let pair1 = actors[1]
            .rng_pair()
            .expect("EvDriverActor must report its RNG pair");
        assert_ne!(
            pair0, pair1,
            "EV_001 and EV_002 must receive distinct (seed, stream) pairs, \
             but both actors share the same RNG state"
        );
    }

    #[test]
    #[should_panic(expected = "EV driver RNG stream collision")]
    fn ev_driver_seed_stream_collision_is_detected() {
        let rng = derive_dwelling_rng(42, 1);
        let sub = derive_sub_rng(&rng, RNG_STREAM_EV_DRIVER_BASE);

        let seed = sub.get_seed();
        let stream = sub.get_stream();

        let mut seen: HashMap<([u8; 32], u64), String> = HashMap::new();

        // First insertion — should succeed.
        check_ev_rng_stream_no_collision(&mut seen, seed, stream, "EvDriver:EV_001");

        // Second insertion with identical (seed, stream) — must panic.
        check_ev_rng_stream_no_collision(&mut seen, seed, stream, "EvDriver:EV_002");
    }

    #[test]
    fn ev_driver_seed_stream_collision_check_passes_for_unique_pairs() {
        let rng = derive_dwelling_rng(42, 1);
        let sub_a = derive_sub_rng(&rng, RNG_STREAM_EV_DRIVER_BASE);
        let sub_b = derive_sub_rng(&rng, RNG_STREAM_EV_DRIVER_BASE + 1);

        let mut seen: HashMap<([u8; 32], u64), String> = HashMap::new();

        // Different stream indices — must not panic.
        check_ev_rng_stream_no_collision(
            &mut seen,
            sub_a.get_seed(),
            sub_a.get_stream(),
            "EvDriver:EV_001",
        );
        check_ev_rng_stream_no_collision(
            &mut seen,
            sub_b.get_seed(),
            sub_b.get_stream(),
            "EvDriver:EV_002",
        );
    }

    #[test]
    fn occupancy_gains_scaled_by_number_of_occupants() {
        use hares_types::DomainUpdate;

        let base_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/bestest/600.toml");
        let mut dwelling = Dwelling::from_toml_config_with_write_output(&base_path, Some(false))
            .expect("build dwelling");

        // Configure: occupancy column at index 0, scale = 4 occupants.
        dwelling.occupancy_column_idx = Some(0);
        dwelling.occupancy_scale = 4.0;

        // Inject a schedule domain update with occupancy fraction = 0.5.
        dwelling.latest_env.upsert_domain(DomainUpdate {
            domain_id: SCHEDULE_DOMAIN_ID,
            zone_temperatures_c: vec![],
            custom_payload: Some(vec![0.5]),
        });

        // Zero thermal ports so we can measure the contribution.
        for thermal in &mut dwelling.ports.thermal {
            thermal.zero();
        }

        dwelling
            .apply_occupancy_gains()
            .expect("apply_occupancy_gains");

        // Expected n_occupants = 0.5 * 4.0 = 2.0
        // Convective sensible = 2.0 × 75.0 × 0.70 = 105.0 W
        // Radiative sensible  = 2.0 × 75.0 × 0.30 = 45.0 W
        // Latent              = 2.0 × 55.0 = 110.0 W
        let expected_sensible = 2.0 * OCCUPANT_SENSIBLE_GAIN_W * OCCUPANT_CONVECTIVE_FRACTION;
        let expected_radiant = 2.0 * OCCUPANT_SENSIBLE_GAIN_W * OCCUPANT_RADIATIVE_FRACTION;
        let expected_latent = 2.0 * OCCUPANT_LATENT_GAIN_W;

        // Occupancy gains must be deposited into the indoor (conditioned)
        // zone ONLY — not broadcast to every zone.
        let indoor_zone = dwelling.thermal_solver.config().indoor_zone_id;

        let indoor_port = dwelling
            .ports
            .thermal
            .iter()
            .find(|t| t.zone == indoor_zone)
            .expect("indoor zone must have a thermal port");

        assert!(
            (indoor_port.sensible_gain_w - expected_sensible).abs() < 1e-9,
            "indoor zone convective sensible gain: expected {expected_sensible}, got {}",
            indoor_port.sensible_gain_w
        );
        assert!(
            (indoor_port.radiant_gain_w - expected_radiant).abs() < 1e-9,
            "indoor zone radiant sensible gain: expected {expected_radiant}, got {}",
            indoor_port.radiant_gain_w
        );
        assert!(
            (indoor_port.latent_gain_w - expected_latent).abs() < 1e-9,
            "indoor zone latent gain: expected {expected_latent}, got {}",
            indoor_port.latent_gain_w
        );

        // Occupancy gains are bucketed under InternalGain category.
        // If the category were changed (e.g. to HvacHeating), the
        // aggregate totals would be correct but the per-category
        // breakdown in the thermal output CSV would be silently wrong.
        assert!(
            (indoor_port.sensible_for_category(ThermalCategory::InternalGain) - expected_sensible)
                .abs()
                < 1e-9,
            "occupancy gain must be categorized as InternalGain convective sensible"
        );
        assert!(
            (indoor_port.radiant_for_category(ThermalCategory::InternalGain) - expected_radiant)
                .abs()
                < 1e-9,
            "occupancy gain must be categorized as InternalGain radiant sensible"
        );
        assert!(
            (indoor_port.latent_for_category(ThermalCategory::InternalGain) - expected_latent)
                .abs()
                < 1e-9,
            "occupancy gain must be categorized as InternalGain latent"
        );

        // Non-indoor zones must receive ZERO occupancy gains.
        for thermal in &dwelling.ports.thermal {
            if thermal.zone == indoor_zone {
                continue;
            }
            assert_eq!(
                thermal.sensible_gain_w, 0.0,
                "non-indoor zone {:?} must not receive convective sensible gain",
                thermal.zone
            );
            assert_eq!(
                thermal.radiant_gain_w, 0.0,
                "non-indoor zone {:?} must not receive radiant gain",
                thermal.zone
            );
            assert_eq!(
                thermal.latent_gain_w, 0.0,
                "non-indoor zone {:?} must not receive latent gain",
                thermal.zone
            );
        }
    }

    /// Verify that calling `apply_occupancy_gains` without a valid schedule domain
    /// payload in `latest_env` panics — the `.expect()` replaces the old silent
    /// `.unwrap_or(0.0)` because construction-time validation guarantees the
    /// schedule domain is always present when `occupancy_column_idx` is `Some`.
    ///
    /// With `occupancy_column_idx = Some(0)` and `occupancy_scale = 3.0`, but
    /// NO `SCHEDULE_DOMAIN_ID` update pushed into `latest_env`, the dwelling
    /// must panic rather than silently computing zero occupants.
    #[test]
    #[should_panic(
        expected = "SCHEDULE_DOMAIN_ID payload absent at step time: construction validated occupancy column exists in schedule"
    )]
    fn absent_schedule_domain_panics_instead_of_silent_zero() {
        let base_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/bestest/600.toml");
        let mut dwelling = Dwelling::from_toml_config_with_write_output(&base_path, Some(false))
            .expect("build dwelling");

        // Manually override to simulate an Occupancy spec with 3 occupants
        // and a valid schedule column index, but with NO schedule domain
        // update pushed into latest_env.  The `.expect()` in the hot path
        // must fire because construction validation guarantees this scenario
        // should never occur.
        dwelling.occupancy_column_idx = Some(0);
        dwelling.occupancy_scale = 3.0;

        for thermal in &mut dwelling.ports.thermal {
            thermal.zero();
        }

        // This must panic — the schedule domain payload is absent and the
        // `.expect()` replaces the old silent `.unwrap_or(0.0)`.
        let _ = dwelling.apply_occupancy_gains();
    }

    /// Verify that `apply_occupancy_gains` returns `Err` when the indoor
    /// zone accumulator is missing from `ports.thermal` — exercising the
    /// error path through `PortSlots::accumulate()`.
    #[test]
    fn occupancy_gains_errors_on_missing_accumulator() {
        use hares_types::DomainUpdate;

        let base_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/bestest/600.toml");
        let mut dwelling = Dwelling::from_toml_config_with_write_output(&base_path, Some(false))
            .expect("build dwelling");

        dwelling.occupancy_column_idx = Some(0);
        dwelling.occupancy_scale = 2.0;

        dwelling.latest_env.upsert_domain(DomainUpdate {
            domain_id: SCHEDULE_DOMAIN_ID,
            zone_temperatures_c: vec![],
            custom_payload: Some(vec![0.5]),
        });

        let indoor_zone = dwelling.thermal_solver.config().indoor_zone_id;

        // Remove the indoor zone accumulator from ports to force the error path.
        dwelling.ports.thermal.retain(|t| t.zone != indoor_zone);
        assert!(
            !dwelling.ports.thermal.iter().any(|t| t.zone == indoor_zone),
            "indoor zone accumulator must be removed to trigger the error"
        );

        let result = dwelling.apply_occupancy_gains();
        assert!(
            result.is_err(),
            "apply_occupancy_gains must return Err when indoor zone accumulator is missing"
        );
        let err = result.unwrap_err();
        assert!(
            matches!(err, HaresError::Equipment(_)),
            "error must be HaresError::Equipment, got: {err}"
        );
    }

    /// Verify that construct-time validation REJECTS a dwelling where the
    /// Occupancy spec is present but NO occupancy schedule data source exists
    /// (no CSV column, no HPXML extension fractions, and no default profile).
    ///
    /// This guards against a data integrity error: `inject_schedule_into_specs`
    /// should always generate the occupancy column when any data source is
    /// available, so this error path only triggers when all sources are missing.
    #[test]
    fn occupied_dwelling_no_occupancy_data_source_errors_at_construction() {
        use hares_io::hpxml::building::XmlNode;
        use std::collections::HashMap;

        // Load a fixture with occupants_present = false so the schedule
        // has no occupancy column.
        let base_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/bestest/600.toml");
        let toml_str = fs::read_to_string(&base_path).expect("read 600.toml");
        let config: SyntheticTomlConfig = toml::from_str(&toml_str).expect("parse 600.toml");
        assert!(
            !config.schedule.occupants_present,
            "600.toml must have occupants_present = false"
        );

        let mut building =
            build_synthetic_building(&config, None, None).expect("build synthetic building");
        let schedule_result = build_synthetic_schedule(&config).expect("build schedule");
        let weather = build_synthetic_weather(&config, &base_path).expect("build weather");

        // Inject BuildingOccupancy / NumberofResidents into the HPXML details
        // so resolve_equipment creates an Occupancy spec.  Without this node
        // the BESTEST 600 building has no Occupancy equipment at all, and the
        // construction path would succeed — exactly the happy path that the
        // other tests already cover.
        {
            let n_residents = XmlNode {
                name: "NumberofResidents".to_string(),
                attrs: HashMap::new(),
                text: "3".to_string(),
                children: Vec::new(),
            };
            let building_occupancy = XmlNode {
                name: "BuildingOccupancy".to_string(),
                attrs: HashMap::new(),
                text: String::new(),
                children: vec![n_residents],
            };
            let building_summary = XmlNode {
                name: "BuildingSummary".to_string(),
                attrs: HashMap::new(),
                text: String::new(),
                children: vec![building_occupancy],
            };
            building.details_xml.children.push(building_summary);
        }

        let sim_config = SimulationConfig {
            start_time: config.simulation.start_time,
            duration: chrono::Duration::seconds(config.simulation.duration_s),
            time_res: chrono::Duration::seconds(config.simulation.time_res_s),
            output_verbosity: config.output.output_verbosity,
            output_path: config.output.output_path.as_ref().map(PathBuf::from),
            write_output: config.output.write_output,
            output_format: config.output.output_format,
            output_chunk_size: config.output.output_chunk_size,
            setpoint_deadband_c: None,
            master_seed: config.output.master_seed,
            civil_timezone: None,
            site_location: hares_io::SiteLocationOverride::default(),
            retain_batches: config.output.retain_batches,
            rotation: config.output.rotation,
        };
        validate_sim_config(&sim_config).expect("valid sim config");

        // Point defaults_path at a defaults directory whose CSV loads but
        // carries no 'Occupancy' profile, and the Occupancy spec has no
        // HPXML extension fractions (only NumberofResidents): no occupancy
        // source of any kind is available.
        let no_occupancy_defaults = tempfile::tempdir().expect("create temp dir");
        std::fs::write(
            no_occupancy_defaults
                .path()
                .join("Default Schedule Parameters.csv"),
            // A valid but Occupancy-free profile file: the strict loader
            // requires every row present, so one complete dummy profile.
            concat!(
                "Category,Name,OCHRE Name,OCHRE Element,Values\n",
                "Schedules,Occupants,Dummy,weekday_fractions,\"",
                "0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, ",
                "0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1\"\n",
                "Schedules,Occupants,Dummy,weekend_fractions,\"",
                "0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, ",
                "0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1\"\n",
                "Schedules,Occupants,Dummy,month_multipliers,\"",
                "1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0\"\n",
            ),
        )
        .expect("write the Occupancy-free defaults CSV");
        let dwelling_config = DwellingConfig {
            hpxml_path: base_path.clone(),
            // `from_preparsed` receives the schedule directly and never
            // reads the config's schedule source.
            schedule_path: None,
            weather_path: base_path.clone(),
            defaults_path: Some(no_occupancy_defaults.path().to_path_buf()),
            sim_config,
            overrides: config.overrides.clone(),
            bldg_id: config.building_id.unwrap_or(0),
            initialization_duration: config
                .simulation
                .initialization_duration_s
                .map(StdDuration::from_secs),
            resample_overrides: None,
            patches: None,
        };

        match Dwelling::from_preparsed(dwelling_config, building, weather, schedule_result.schedule)
        {
            Err(HaresError::Dwelling(msg)) => {
                assert!(
                    msg.contains("no occupancy column found in schedule"),
                    "error must identify the missing occupancy schedule column; got: {msg}"
                );
            }
            Ok(_) => panic!("expected Err(HaresError::Dwelling(...)) but construction succeeded"),
            Err(e) => {
                panic!("expected Err(HaresError::Dwelling(...)) but got different error: {e}")
            }
        }
    }

    /// Parse 600.toml fixture and verify the internal gains radiative fraction
    /// matches the EnergyPlus BESTEST authoritative value.
    ///
    /// EnergyPlus BESTEST IDF OtherEquipment Fraction Radiant = 0.3 (30% radiant,
    /// 70% convective). Source: E+ I/O Reference §Group-InternalGains,
    /// OtherEquipment object; E+ IDD V9-6-0-Energy+.idd N5 field default=0 but
    /// BESTEST IDF overrides to 0.3. This matches E+ v9.6 BESTEST test reference
    /// value. The prior claim of 60% in audit documents was a misattribution
    /// (the 60% figure refers to occupant sensible gain conventions, not
    /// OtherEquipment); documented in docs/findings/consolidated.md §1.
    #[test]
    fn bestest_600_internal_gains_radiant_fraction_matches_energyplus() {
        let base_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/bestest/600.toml");
        let toml_str = fs::read_to_string(&base_path).expect("read 600.toml");
        let config: SyntheticTomlConfig = toml::from_str(&toml_str).expect("parse 600.toml");

        assert!(
            config.internal_gains_w.is_some(),
            "600.toml must have internal_gains_w"
        );
        assert!(
            config.internal_gains_sensible_fraction.is_some(),
            "600.toml must have internal_gains_sensible_fraction"
        );
        assert!(
            config.internal_gains_radiant_fraction.is_some(),
            "600.toml must have internal_gains_radiant_fraction"
        );

        let sensible_frac = config.internal_gains_sensible_fraction.unwrap();
        let radiant_frac = config.internal_gains_radiant_fraction.unwrap();

        assert!(
            (sensible_frac - 1.0).abs() < 1e-9,
            "600.toml sensible_fraction = {sensible_frac}, expected 1.0 (all-sensible BESTEST gains)"
        );
        assert!(
            (radiant_frac - 0.3).abs() < 1e-9,
            "600.toml radiant_fraction = {radiant_frac}, expected 0.3 (E+ BESTEST IDF FractionRadiant)"
        );
    }

    #[test]
    fn dwelling_equipment_creation_errors_on_unknown_class() {
        let registry = EquipmentRegistry::new();
        let spec = hares_io::EquipmentSpec {
            instance_name: None,
            name: "Imaginary Widget".to_string(),
            fuel_type: hares_types::FuelType::Electric,
            parameters: Map::new(),
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        };

        let base_cfg = equipment_config_from_spec(&spec).expect("raw spec conversion must succeed");
        let err = match create_equipment_from_config(&registry, base_cfg) {
            Err(err) => err,
            Ok(_) => panic!("unknown equipment class must return Err"),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("Imaginary Widget"),
            "error should preserve equipment name, got: {msg}"
        );
        assert!(
            msg.contains("unknown equipment class"),
            "error should preserve registry failure context, got: {msg}"
        );
    }

    #[test]
    fn attic_temperature_column_uses_zone_type_not_zone_id() {
        use hares_io::hpxml::ZoneType;

        let zones = vec![
            ZoneState {
                id: ZoneId(10),
                temperature_c: 21.0,
                humidity_ratio: 0.008,
                volume_m3: 200.0,
            },
            ZoneState {
                id: ZoneId(3),
                temperature_c: 16.0,
                humidity_ratio: 0.008,
                volume_m3: 120.0,
            },
        ];
        let zone_types = vec![ZoneType::Conditioned, ZoneType::Attic];
        let schema = hares_io::build_schema(
            &[],
            2,
            &[
                (ZoneId(10), "Indoor".to_string()),
                (ZoneId(3), "Attic".to_string()),
            ],
        );
        let column_index = build_output_column_index(&schema);
        let caches = build_zone_column_caches(&zones, &zone_types, ZoneId(10), &column_index);

        let attic_idx = column_index
            .get("Temperature - Attic (C)")
            .copied()
            .expect("schema must include attic temperature column");
        assert_eq!(
            caches.zone_temp_col_indices,
            vec![
                Some(attic_idx),
                Some(column_index["Temperature - Indoor (C)"])
            ],
            "zone temperatures should be mapped by zone type, not zone number"
        );
    }

    #[test]
    fn per_zone_hvac_columns_written_from_port_accumulators() {
        use hares_io::hpxml::ZoneType;
        use hares_types::ports::PortContribution;

        let zones = vec![ZoneState {
            id: ZoneId(1),
            temperature_c: 21.0,
            humidity_ratio: 0.008,
            volume_m3: 200.0,
        }];
        let zone_types = vec![ZoneType::Conditioned];
        let schema = hares_io::build_schema(&[], 6, &[(ZoneId(1), "Indoor".to_string())]);
        let column_index = build_output_column_index(&schema);
        let caches = build_zone_column_caches(&zones, &zone_types, ZoneId(1), &column_index);

        let (heat_idx, cool_idx) = caches
            .hvac_columns
            .get(&ZoneId(1))
            .copied()
            .expect("Indoor zone must have HVAC column indices at verbosity 6");
        assert_ne!(
            heat_idx, cool_idx,
            "heating and cooling column indices must differ"
        );

        // Simulate equipment writing HvacHeating and HvacCooling to ports.
        let mut ports = PortSlots::from_declarations(&[PortDeclaration::thermal(ZoneId(1))]);
        ports
            .accumulate(&PortContribution::Thermal {
                zone: ZoneId(1),
                sensible_gain_w: 500.0,
                radiant_gain_w: 0.0,
                latent_gain_w: 0.0,
                category: ThermalCategory::HvacHeating,
            })
            .expect("accumulate must succeed");
        ports
            .accumulate(&PortContribution::Thermal {
                zone: ZoneId(1),
                sensible_gain_w: -200.0,
                radiant_gain_w: 0.0,
                latent_gain_w: 0.0,
                category: ThermalCategory::HvacCooling,
            })
            .expect("accumulate must succeed");

        let indoor_heating = ports.thermal[0].sensible_for_category(ThermalCategory::HvacHeating);
        let indoor_cooling = ports.thermal[0].sensible_for_category(ThermalCategory::HvacCooling);
        assert!(
            (indoor_heating - 500.0).abs() < 1e-9,
            "expected 500.0 W heating, got {indoor_heating}"
        );
        assert!(
            (indoor_cooling - (-200.0)).abs() < 1e-9,
            "expected -200.0 W cooling, got {indoor_cooling}"
        );

        // Build a minimal record context and write to scratch.
        let mut record_scratch = vec![0.0; column_index.len()];
        record_scratch[heat_idx] = indoor_heating;
        record_scratch[cool_idx] = indoor_cooling.abs();
        assert!((record_scratch[heat_idx] - 500.0).abs() < 1e-9);
        assert!((record_scratch[cool_idx] - 200.0).abs() < 1e-9);
    }

    #[test]
    fn actor_telemetry_columns_appended_to_schema_and_populated_in_output() {
        use crate::actors::Occupant;
        use crate::actors::Presence;

        // Build a schema with 2 zones at verbosity 0 (minimum columns).
        let schema = hares_io::build_schema(&[], 0, &[]);
        let base_fields = schema.fields().len();

        // Create an actor with telemetry keys.
        let schedule = vec![Presence::Home, Presence::Home];
        let mut actor = Occupant::new("Occupant").with_presence_schedule(schedule);

        // decide() populates telemetry values.
        let env = crate::actor::testing::test_env().build();
        let mut out = Vec::new();
        actor.decide(&env, &mut out);

        let actors: Vec<Box<dyn Actor>> = vec![Box::new(actor)];
        let extended = extend_schema_with_actor_columns(
            &schema,
            &actors.iter().map(AsRef::as_ref).collect::<Vec<_>>(),
        );
        let extended_fields = extended.fields().len();

        // Schema must gain columns (one per actor telemetry key).
        let n_actor_keys = 5; // away, transition, signals_count, presence_changes, current_step
        assert_eq!(
            extended_fields,
            base_fields + n_actor_keys,
            "schema must include actor telemetry columns"
        );

        let field_names: Vec<&str> = extended
            .fields()
            .iter()
            .map(|f| f.name().as_str())
            .collect();

        for key in &[
            "away",
            "transition",
            "signals_count",
            "presence_changes",
            "current_step",
        ] {
            let col = format!("actor:Occupant:{key}");
            assert!(
                field_names.contains(&col.as_str()),
                "schema must contain '{col}'"
            );
        }

        // Build column index and exercise record_step pattern.
        let column_index = build_output_column_index(&extended);
        let mut scratch = vec![0.0; column_index.len()];

        for actor in &actors {
            if let Some(tel) = actor.telemetry() {
                for (key, &value) in &tel.0 {
                    let col_name = format!("actor:{}:{}", actor.name(), key);
                    if let Some(&idx) = column_index.get(&col_name) {
                        scratch[idx] = value;
                    }
                }
            }
        }

        // Verify values populated in scratch at correct indices.
        for key in &["away", "transition", "signals_count", "presence_changes"] {
            let col = format!("actor:Occupant:{key}");
            let idx = column_index[&col];
            assert!(scratch[idx] >= 0.0,);
        }
    }

    #[test]
    fn zone_map_from_multi_zone_building_routes_equipment_correctly() {
        use chrono::TimeZone;
        use hares_equipment::scheduled_load::ScheduledLoad;
        use hares_io::hpxml::building::XmlNode;
        use hares_io::hpxml::{Site, Zone, ZoneType};
        use hares_types::{EndUse, ZoneMap, ZoneRole};
        use std::collections::HashMap;

        // Construct a multi-zone building: Conditioned, Garage, Foundation, Attic.
        // ZoneId = idx + 1 (matching initial_zones() / zone_sort_key convention).
        let building = Building {
            site: Site {
                elevation_m: None,
                site_type: None,
                shielding_of_home: None,
                latitude_deg: None,
                longitude_deg: None,
                utc_offset_h: None,
            },
            zones: vec![
                Zone {
                    zone_type: ZoneType::Conditioned,
                    floor_area_m2: Some(100.0),
                    volume_m3: Some(250.0),
                    attached_wall_ids: vec![],
                    duct_systems: vec![],
                    vented: false,
                    ventilation_ach: None,
                    ventilation_sla: None,
                },
                Zone {
                    zone_type: ZoneType::Garage,
                    floor_area_m2: Some(40.0),
                    volume_m3: Some(90.0),
                    attached_wall_ids: vec![],
                    duct_systems: vec![],
                    vented: false,
                    ventilation_ach: None,
                    ventilation_sla: None,
                },
                Zone {
                    zone_type: ZoneType::Foundation,
                    floor_area_m2: Some(50.0),
                    volume_m3: Some(120.0),
                    attached_wall_ids: vec![],
                    duct_systems: vec![],
                    vented: false,
                    ventilation_ach: None,
                    ventilation_sla: None,
                },
                Zone {
                    zone_type: ZoneType::Attic,
                    floor_area_m2: Some(60.0),
                    volume_m3: Some(150.0),
                    attached_wall_ids: vec![],
                    duct_systems: vec![],
                    vented: false,
                    ventilation_ach: None,
                    ventilation_sla: None,
                },
            ],
            boundaries: vec![],
            windows: vec![],
            skylights: vec![],
            infiltration_ach50: None,
            infiltration_cfm50: None,
            infiltration_ach_natural: None,
            infiltration_cfm_natural: None,
            infiltration_ela_cm2: None,
            infiltration_constant_ach: None,
            hvac_capacity_w: None,
            seer2: None,
            hspf2: None,
            water_heater_setpoint_c: None,
            heating_weekday_setpoints_c: None,
            heating_weekend_setpoints_c: None,
            cooling_weekday_setpoints_c: None,
            cooling_weekend_setpoints_c: None,
            battery_round_trip_efficiency: None,
            pv_tilt_deg: None,
            conditioned_volume_m3: None,
            ceiling_height_m: 2.5,
            infiltration_height_m: None,
            floors_above_grade: 1.0,
            has_flue_or_chimney: None,
            foundation_name: None,
            residential_facility_type: None,
            mass_multiplier_override: None,
            hvac_deadband_c: None,
            details_xml: XmlNode {
                name: String::new(),
                attrs: HashMap::new(),
                text: String::new(),
                children: vec![],
            },
            parse_warnings: Vec::new(),
        };

        // Build ZoneMap from building zones (same logic as from_preparsed()).
        let mut zone_map = ZoneMap::new();
        for (idx, zone) in building.zones.iter().enumerate() {
            let id = ZoneId(u16::try_from(idx + 1).unwrap_or(u16::MAX));
            match &zone.zone_type {
                ZoneType::Conditioned => {
                    zone_map.insert(ZoneRole::Indoor, id);
                }
                ZoneType::Garage => {
                    zone_map.insert(ZoneRole::Garage, id);
                }
                ZoneType::Foundation => {
                    zone_map.insert(ZoneRole::Basement, id);
                    zone_map.insert(ZoneRole::Crawlspace, id);
                }
                ZoneType::Attic => {
                    zone_map.insert(ZoneRole::Attic, id);
                }
                ZoneType::Outdoor | ZoneType::Ground | ZoneType::Adjacent => {}
                ZoneType::Other(_) => {}
            }
        }

        // Verify ZoneMap has correct role-to-ZoneId mappings.
        assert_eq!(
            zone_map.get(ZoneRole::Indoor),
            Some(ZoneId(1)),
            "first zone (Conditioned) -> Indoor -> ZoneId(1)"
        );
        assert_eq!(
            zone_map.get(ZoneRole::Garage),
            Some(ZoneId(2)),
            "second zone (Garage) -> ZoneId(2)"
        );
        assert_eq!(
            zone_map.get(ZoneRole::Basement),
            Some(ZoneId(3)),
            "third zone (Foundation) -> Basement -> ZoneId(3)"
        );
        assert_eq!(
            zone_map.get(ZoneRole::Crawlspace),
            Some(ZoneId(3)),
            "third zone (Foundation) -> Crawlspace -> ZoneId(3)"
        );
        assert_eq!(
            zone_map.get(ZoneRole::Attic),
            Some(ZoneId(4)),
            "fourth zone (Attic) -> ZoneId(4)"
        );

        // Build a raw EquipmentConfig for a garage lighting load.
        let mut raw: HashMap<String, ConfigValue> = HashMap::new();
        raw.insert("power_schedule_source".to_string(), "constant".into());
        raw.insert("power_constant_kw".to_string(), 1.0.into());
        // sensible_gain_fraction is required by init(); set to zero since this
        // test only verifies zone routing, not thermal output.
        raw.insert("sensible_gain_fraction".to_string(), 0.0.into());
        let mut config = EquipmentConfig::raw(
            "Garage Lighting".to_string(),
            "Garage Lighting".to_string(),
            raw,
        );
        config.zone_map = Some(zone_map);

        let env = EnvironmentState {
            ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: 21.0,
                humidity_ratio: 0.008,
                volume_m3: 200.0,
            }],
            weather: hares_types::WeatherState::default(),
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
                island_bus_voltage_pu: None,
            },
            custom_domains: vec![],
            equipment_telemetry: HashMap::new(),
            equipment_core: HashMap::new(),
            current_time: chrono::FixedOffset::east_opt(0)
                .expect("UTC offset")
                .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                .single()
                .expect("valid UTC timestamp"),
            time_res: chrono::Duration::seconds(60),
            price_signal: Default::default(),
            electrical: Default::default(),
        };
        let mut eq = ScheduledLoad::new(config.clone(), EndUse::LIGHTING, "Garage Lighting");
        eq.init(&config, &env).unwrap();

        assert_eq!(
            eq.descriptor().zone,
            Some(ZoneId(2)),
            "Garage Lighting should auto-route via ZoneMap to garage zone (ZoneId(2))"
        );
    }

    /// Verify that constructing a dwelling with a window U-factor >= ~10
    /// (which produces negative r_glass in `window_u_factor_decomposition`)
    /// returns `HaresError::Physics` rather than silently producing garbage
    /// or panicking.
    ///
    /// Integration test: T-0145 — regression protection for physics error
    /// propagation from solar model through dwelling construction.
    #[test]
    fn dwelling_construction_with_corrupted_window_u_returns_physics_error() {
        let dir = tempfile::tempdir().expect("temp dir");
        let toml_path = dir.path().join("corrupt-window.toml");

        fs::write(
            &toml_path,
            r#"building_id = 999
[simulation]
start_time = "2024-01-15T00:00:00Z"
time_res_s = 60
duration_s = 120

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.8

[hvac]
equipment_name = "Furnace"
fuel = "electricity"
heating_capacity_kbtu_h = 30.0

[[windows]]
id = "Win1"
area_m2 = 4.0
azimuth_deg = 180
u_factor_w_m2_k = 10.0
shgc = 0.6

[weather]
outdoor_temp_c = -10.0
dew_point_c = -5.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325

[schedule]
occupancy = 1.0

[output]
write_output = false
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
master_seed = 0
"#,
        )
        .expect("write synthetic TOML");

        let result = Dwelling::from_toml_config(&toml_path);

        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("dwelling with U=10 window must return error"),
        };
        assert!(
            matches!(err, HaresError::Physics(_)),
            "error must be HaresError::Physics, got {:?}",
            err
        );
        let msg = err.to_string();
        assert!(
            msg.contains("r_glass"),
            "error message must mention r_glass, got: {msg}"
        );
    }

    /// Verify that calling `run_timestep` after the simulation has exhausted
    /// all steps returns `HaresError::Simulation`, not a spurious physics
    /// error. Regression: T-0144 reclassified the step-overflow guard from
    /// `HaresError::Physics` to `HaresError::Dwelling`; it was later moved to
    /// `HaresError::Simulation` so the Python boundary raises
    /// `HaresSimulationError` (a `RuntimeError`) for this runtime-state case
    /// instead of `HaresConfigError` (a `ValueError`).
    #[test]
    fn run_timestep_past_end_returns_simulation_error() {
        let dir = tempfile::tempdir().expect("temp dir");
        let toml_path = dir.path().join("step-overflow.toml");

        fs::write(
            &toml_path,
            r#"building_id = 999
[simulation]
start_time = "2024-01-15T00:00:00Z"
time_res_s = 60
duration_s = 120

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.8

[hvac]
equipment_name = "Furnace"
fuel = "electricity"
heating_capacity_kbtu_h = 30.0

[weather]
outdoor_temp_c = -10.0
dew_point_c = -5.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325

[schedule]
occupancy = 1.0

[output]
write_output = false
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
master_seed = 0
"#,
        )
        .expect("write synthetic TOML");

        let mut dwelling = Dwelling::from_toml_config(&toml_path).expect("build dwelling");

        let total = dwelling.clock.total_steps();
        for _ in 0..total {
            dwelling.run_timestep(false).expect("step within bounds");
        }

        let err = dwelling
            .run_timestep(false)
            .expect_err("step past end must error");
        assert!(
            matches!(err, HaresError::Simulation(_)),
            "step-past-end error must be HaresError::Simulation, got {:?}",
            err
        );
    }

    #[test]
    fn zone_temperature_delta_above_threshold_triggers_interest() {
        use chrono::{Duration, FixedOffset, TimeZone};
        use hares_types::{ElectricalSummary, GridState, WeatherState};
        use std::collections::HashMap;

        let zone = ZoneId(1);
        let threshold_c = 2.0;
        let interest = ActorInterest::ZoneTemperatureDelta { zone, threshold_c };
        let prev_price = PriceSignal::default();
        let prev_modes: HashMap<EquipmentId, Option<OperatingMode>> = HashMap::new();
        let id_by_name: HashMap<String, EquipmentId> = HashMap::new();
        let equipment: Vec<Box<dyn Equipment>> = vec![];

        fn env_with_zone(zone_id: ZoneId, temperature_c: f64) -> EnvironmentState {
            EnvironmentState {
                ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
                zones: vec![ZoneState {
                    id: zone_id,
                    temperature_c,
                    humidity_ratio: 0.008,
                    volume_m3: 120.0,
                }],
                weather: WeatherState::default(),
                grid: GridState {
                    voltage_pu: 1.0,
                    frequency_hz: 60.0,
                    island_bus_voltage_pu: None,
                },
                custom_domains: vec![],
                equipment_telemetry: Default::default(),
                equipment_core: Default::default(),
                current_time: FixedOffset::east_opt(0)
                    .expect("UTC")
                    .with_ymd_and_hms(2024, 6, 15, 12, 0, 0)
                    .single()
                    .expect("valid timestamp"),
                time_res: Duration::minutes(1),
                price_signal: PriceSignal::default(),
                electrical: ElectricalSummary::default(),
            }
        }

        // Case 1: prior_zone_temps empty → first comparison, always triggers.
        {
            let env = env_with_zone(zone, 21.0);
            let prior_empty: HashMap<ZoneId, f64> = HashMap::new();
            let prev_empty: HashMap<ZoneId, f64> = HashMap::new();
            assert!(
                Dwelling::interest_triggered(
                    &interest,
                    &env,
                    &InterestFilterState {
                        prev_zone_temps: &prev_empty,
                        prior_zone_temps: &prior_empty,
                        prev_price_signal: &prev_price,
                        prev_equipment_modes: &prev_modes,
                        equipment_id_by_name: &id_by_name,
                        equipment: &equipment,
                    },
                ),
                "first step with empty prior_zone_temps must trigger"
            );
        }

        // Case 2: delta below threshold → should NOT trigger.
        {
            let env = env_with_zone(zone, 20.5);
            let mut prior = HashMap::new();
            prior.insert(zone, 20.0);
            let prev_dummy: HashMap<ZoneId, f64> = HashMap::new();
            assert!(
                !Dwelling::interest_triggered(
                    &interest,
                    &env,
                    &InterestFilterState {
                        prev_zone_temps: &prev_dummy,
                        prior_zone_temps: &prior,
                        prev_price_signal: &prev_price,
                        prev_equipment_modes: &prev_modes,
                        equipment_id_by_name: &id_by_name,
                        equipment: &equipment,
                    },
                ),
                "0.5°C delta below 2.0°C threshold must not trigger"
            );
        }

        // Case 3: delta above threshold (heating) → SHOULD trigger.
        {
            let env = env_with_zone(zone, 23.0);
            let mut prior = HashMap::new();
            prior.insert(zone, 20.0);
            let prev_dummy: HashMap<ZoneId, f64> = HashMap::new();
            assert!(
                Dwelling::interest_triggered(
                    &interest,
                    &env,
                    &InterestFilterState {
                        prev_zone_temps: &prev_dummy,
                        prior_zone_temps: &prior,
                        prev_price_signal: &prev_price,
                        prev_equipment_modes: &prev_modes,
                        equipment_id_by_name: &id_by_name,
                        equipment: &equipment,
                    },
                ),
                "3.0°C heating delta must trigger when >= 2.0°C threshold"
            );
        }

        // Case 4: negative delta above threshold (cooling) → SHOULD trigger via abs().
        {
            let env = env_with_zone(zone, 17.0);
            let mut prior = HashMap::new();
            prior.insert(zone, 20.0);
            let prev_dummy: HashMap<ZoneId, f64> = HashMap::new();
            assert!(
                Dwelling::interest_triggered(
                    &interest,
                    &env,
                    &InterestFilterState {
                        prev_zone_temps: &prev_dummy,
                        prior_zone_temps: &prior,
                        prev_price_signal: &prev_price,
                        prev_equipment_modes: &prev_modes,
                        equipment_id_by_name: &id_by_name,
                        equipment: &equipment,
                    },
                ),
                "3.0°C cooling delta must trigger (abs) when >= 2.0°C threshold"
            );
        }

        // Case 5: zone present in env but absent from prior → triggers (first comparison).
        {
            let env = env_with_zone(ZoneId(2), 30.0);
            let mut prior = HashMap::new();
            prior.insert(ZoneId(1), 20.0);
            let prev_dummy: HashMap<ZoneId, f64> = HashMap::new();
            let interest_z2 = ActorInterest::ZoneTemperatureDelta {
                zone: ZoneId(2),
                threshold_c: 2.0,
            };
            assert!(
                Dwelling::interest_triggered(
                    &interest_z2,
                    &env,
                    &InterestFilterState {
                        prev_zone_temps: &prev_dummy,
                        prior_zone_temps: &prior,
                        prev_price_signal: &prev_price,
                        prev_equipment_modes: &prev_modes,
                        equipment_id_by_name: &id_by_name,
                        equipment: &equipment,
                    },
                ),
                "zone in env but not in prior_zone_temps must trigger"
            );
        }

        // Case 6: zone absent from env → never triggers.
        {
            let env = env_with_zone(ZoneId(1), 20.0);
            let prior: HashMap<ZoneId, f64> = HashMap::new();
            let prev_dummy: HashMap<ZoneId, f64> = HashMap::new();
            let interest_z99 = ActorInterest::ZoneTemperatureDelta {
                zone: ZoneId(99),
                threshold_c: 2.0,
            };
            assert!(
                !Dwelling::interest_triggered(
                    &interest_z99,
                    &env,
                    &InterestFilterState {
                        prev_zone_temps: &prev_dummy,
                        prior_zone_temps: &prior,
                        prev_price_signal: &prev_price,
                        prev_equipment_modes: &prev_modes,
                        equipment_id_by_name: &id_by_name,
                        equipment: &equipment,
                    },
                ),
                "zone not present in env must never trigger"
            );
        }
    }

    /// Verify that `validate_equipment_zones` rejects a declaration targeting a zone
    /// absent from the environment model.  Deleting the validation block from
    /// `from_preparsed` would silently remove this protection; this test would fail.
    #[test]
    fn equipment_declaring_nonexistent_zone_is_rejected_at_construction() {
        let env_zones: HashSet<ZoneId> = [ZoneId(1), ZoneId(2)].into_iter().collect();

        // ZoneId(1) is valid.
        let valid = vec![PortDeclaration::thermal(ZoneId(1))];
        assert!(
            validate_equipment_zones(&valid, &env_zones).is_ok(),
            "declaration for an env zone must succeed"
        );

        // ZoneId(999) does not exist in the env.
        let bad = vec![
            PortDeclaration::thermal(ZoneId(1)),
            PortDeclaration::thermal(ZoneId(999)),
        ];
        let err = validate_equipment_zones(&bad, &env_zones)
            .expect_err("declaration for a nonexistent zone must return Err");
        match err {
            HaresError::Dwelling(msg) => {
                assert!(
                    msg.contains("ZoneId(999)"),
                    "error must name the offending zone; got: {msg}"
                );
                assert!(
                    msg.contains("does not exist in the environment model"),
                    "error must explain why; got: {msg}"
                );
            }
            other => panic!("expected HaresError::Dwelling, got {other:?}"),
        }

        // Declarations without a zone (Electrical, Fuel) are not zone-checked.
        let no_zone = vec![PortDeclaration::electrical(), PortDeclaration::fuel()];
        assert!(
            validate_equipment_zones(&no_zone, &env_zones).is_ok(),
            "non-zoned declarations must always pass zone validation"
        );
    }

    /// Verify that `validate_equipment_loops` rejects a fluid port declaration
    /// referencing a loop ID that was never allocated to any equipment spec.
    /// A fluid port with an unallocated loop ID indicates an equipment
    /// constructor that hardcoded a loop ID instead of using its typed config.
    #[test]
    fn equipment_declaring_unallocated_loop_is_rejected_at_construction() {
        // LoopId(1) and LoopId(2) were allocated.
        let allocated: HashSet<u16> = [1, 2].into_iter().collect();

        // LoopId(1) is valid.
        let valid = vec![PortDeclaration::fluid(LoopId(1), FluidType::Water)];
        assert!(
            validate_equipment_loops(&valid, &allocated).is_ok(),
            "declaration for an allocated loop must succeed"
        );

        // LoopId(999) was never allocated.
        let bad = vec![
            PortDeclaration::fluid(LoopId(1), FluidType::Water),
            PortDeclaration::fluid(LoopId(999), FluidType::Water),
        ];
        let err = validate_equipment_loops(&bad, &allocated)
            .expect_err("declaration for an unallocated loop must return Err");
        match err {
            HaresError::Dwelling(msg) => {
                assert!(
                    msg.contains("LoopId(999)"),
                    "error must name the offending loop; got: {msg}"
                );
                assert!(
                    msg.contains("has not been allocated"),
                    "error must explain why; got: {msg}"
                );
            }
            other => panic!("expected HaresError::Dwelling, got {other:?}"),
        }

        // Declarations without a loop_id (Electrical, Fuel, Thermal) are not loop-checked.
        let no_loop = vec![
            PortDeclaration::electrical(),
            PortDeclaration::fuel(),
            PortDeclaration::thermal(ZoneId(1)),
        ];
        assert!(
            validate_equipment_loops(&no_loop, &allocated).is_ok(),
            "non-fluid declarations must always pass loop validation"
        );
    }

    #[test]
    fn save_checkpoint_does_not_panic_on_minimal_dwelling() {
        // Build a minimal dwelling via TOML, step once, and verify
        // save_checkpoint() returns a Result (not a panic).
        let (_dir, toml_path) = temp_toml("save_checkpoint_no_panic");
        write_minimal_toml(&toml_path);

        let mut dwelling = Dwelling::from_toml_config(&toml_path).expect("build dwelling");
        dwelling.step().expect("step succeeds");
        let result = dwelling.save_checkpoint();
        assert!(
            result.is_ok(),
            "save_checkpoint should succeed on minimal dwelling"
        );
    }

    #[test]
    fn checkpoint_actor_state_round_trips_with_schema_version() {
        // An actor's mutable decision-state must survive a dwelling
        // checkpoint round-trip, and the checkpoint must record the actor's
        // snapshot schema version beside the blob so restore can gate on it.
        let (_dir, toml_path) = temp_toml("actor_state_round_trip");
        write_minimal_toml(&toml_path);

        let mut dwelling_a = Dwelling::from_toml_config(&toml_path).expect("build dwelling A");
        dwelling_a
            .add_actor(Box::new(StatefulStubActor::new("StatefulActor", 7)))
            .unwrap();
        dwelling_a.step().expect("step succeeds");

        let checkpoint = dwelling_a.save_checkpoint().expect("save checkpoint");
        let actor_state = checkpoint
            .actor_states
            .iter()
            .find(|s| s.name == "StatefulActor")
            .expect("checkpoint must carry the actor's state");
        assert_eq!(
            actor_state.schema_version, 1,
            "the actor's checkpoint_version() must be recorded beside its blob"
        );

        // Restore into a second dwelling whose same-named actor holds
        // different state — the saved blob must win.
        let mut dwelling_b = Dwelling::from_toml_config(&toml_path).expect("build dwelling B");
        dwelling_b
            .add_actor(Box::new(StatefulStubActor::new("StatefulActor", 0)))
            .unwrap();
        dwelling_b
            .load_checkpoint(checkpoint)
            .expect("load checkpoint");

        let counter = dwelling_b
            .telemetry()
            .unwrap()
            .actor_telemetry
            .get("StatefulActor")
            .and_then(|channels| channels.get("counter"))
            .copied();
        assert_eq!(
            counter,
            Some(7.0),
            "actor state must be restored from the checkpoint blob (visible via actor telemetry)"
        );
    }

    #[test]
    fn checkpoint_actor_schema_version_mismatch_rejected_with_named_error() {
        // A blob written against a different actor snapshot schema must be
        // rejected at the checkpoint boundary with a version-mismatch error
        // naming the actor and both versions — not surface as a postcard
        // decode failure from inside the actor's load_state.
        let (_dir, toml_path) = temp_toml("actor_version_mismatch");
        write_minimal_toml(&toml_path);

        let mut dwelling_a = Dwelling::from_toml_config(&toml_path).expect("build dwelling A");
        dwelling_a
            .add_actor(Box::new(StatefulStubActor::new("StatefulActor", 7)))
            .unwrap();
        dwelling_a.step().expect("step succeeds");

        let mut checkpoint = dwelling_a.save_checkpoint().expect("save checkpoint");
        // Tamper: pretend the blob was written by a schema generation the
        // live actor no longer understands.
        checkpoint
            .actor_states
            .iter_mut()
            .find(|s| s.name == "StatefulActor")
            .expect("checkpoint must carry the actor's state")
            .schema_version = 2;

        let mut dwelling_b = Dwelling::from_toml_config(&toml_path).expect("build dwelling B");
        dwelling_b
            .add_actor(Box::new(StatefulStubActor::new("StatefulActor", 0)))
            .unwrap();

        let err = dwelling_b
            .load_checkpoint(checkpoint)
            .expect_err("version mismatch must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("actor schema version mismatch")
                && msg.contains("StatefulActor")
                && msg.contains("blob=2")
                && msg.contains("expected=1"),
            "error must name the actor and both versions; got: {msg}"
        );
        assert!(
            !msg.contains("postcard") && !msg.contains("deserialize"),
            "the rejection must come from the version gate, not a postcard decode failure; got: {msg}"
        );
    }

    #[test]
    fn checkpoint_truncated_stateful_blob_must_not_restore_as_silent_success() {
        // A stateful actor's empty blob passes the schema-version gate (the
        // recorded version matches — truncation predates the stamp), so the
        // only thing between a truncated/corrupt checkpoint and a silent
        // partial restore is `load_state`'s empty-blob early return. That
        // guard exists for the "no mutable state" convention, but a stateful
        // actor handed an empty blob keeps its freshly-constructed decision
        // state while the restore reports success — the checkpoint's state
        // is silently discarded and the resumed run diverges from the
        // original with no error naming the loss.
        let (_dir, toml_path) = temp_toml("actor_blob_truncated");
        write_minimal_toml(&toml_path);

        let mut dwelling_a = Dwelling::from_toml_config(&toml_path).expect("build dwelling A");
        dwelling_a
            .add_actor(Box::new(StatefulStubActor::new("StatefulActor", 7)))
            .unwrap();
        dwelling_a.step().expect("step succeeds");

        let mut checkpoint = dwelling_a.save_checkpoint().expect("save checkpoint");
        let state = checkpoint
            .actor_states
            .iter_mut()
            .find(|s| s.name == "StatefulActor")
            .expect("checkpoint must carry the actor's state");
        assert!(
            !state.blob.is_empty(),
            "precondition: a stateful actor saves a non-empty blob"
        );
        state.blob = Vec::new();

        let mut dwelling_b = Dwelling::from_toml_config(&toml_path).expect("build dwelling B");
        dwelling_b
            .add_actor(Box::new(StatefulStubActor::new("StatefulActor", 0)))
            .unwrap();

        let err = dwelling_b.load_checkpoint(checkpoint).expect_err(
            "a truncated stateful blob must not restore as silent success — the \
                 empty-blob guard turns it into a no-op and the dwelling continues on \
                 freshly-constructed decision-state with the restore reporting Ok",
        );
        assert!(
            err.to_string().contains("StatefulActor"),
            "the rejection must name the actor whose blob was lost; got: {err}"
        );
    }

    #[test]
    fn checkpoint_stateless_actor_empty_blob_restores_cleanly() {
        // The other side of the empty-blob rule: "actors with no mutable
        // state contribute an empty blob" is the `Actor::save_state`
        // convention, and the truncation gate must not over-reject it. A
        // stateless actor (trait-default `save_state` → empty blob, default
        // `load_state` → no-op) must round-trip through the dwelling
        // checkpoint unchanged.
        let (_dir, toml_path) = temp_toml("actor_stateless_round_trip");
        write_minimal_toml(&toml_path);

        let mut dwelling_a = Dwelling::from_toml_config(&toml_path).expect("build dwelling A");
        dwelling_a
            .add_actor(Box::new(StubActor {
                name: "StatelessActor".to_string(),
            }))
            .unwrap();
        dwelling_a.step().expect("step succeeds");

        let checkpoint = dwelling_a.save_checkpoint().expect("save checkpoint");
        let state = checkpoint
            .actor_states
            .iter()
            .find(|s| s.name == "StatelessActor")
            .expect("checkpoint must carry the stateless actor's entry");
        assert!(
            state.blob.is_empty(),
            "precondition: a stateless actor saves an empty blob"
        );

        let mut dwelling_b = Dwelling::from_toml_config(&toml_path).expect("build dwelling B");
        dwelling_b
            .add_actor(Box::new(StubActor {
                name: "StatelessActor".to_string(),
            }))
            .unwrap();

        dwelling_b
            .load_checkpoint(checkpoint)
            .expect("a stateless actor's empty blob is the no-state convention, not truncation");
    }

    // Actor whose `save_state` fails, to drive the empty-blob probe's error
    // arm in `load_checkpoint`.
    struct FailingProbeActor {
        name: String,
    }
    impl crate::Actor for FailingProbeActor {
        fn name(&self) -> &str {
            &self.name
        }
        fn decide(
            &mut self,
            _env: &hares_types::EnvironmentState,
            _out: &mut Vec<hares_control::DispatchRequest>,
        ) {
        }
        fn save_state(&self) -> std::result::Result<Vec<u8>, HaresError> {
            Err(HaresError::Control(
                "simulated serialization failure".to_string(),
            ))
        }
    }

    #[test]
    fn checkpoint_empty_blob_probe_failure_rejected_with_named_error() {
        // The empty-blob rule's third arm: when the checkpoint entry's blob
        // is empty, the restore probes the live actor's `save_state` to
        // tell "stateless convention" from "truncated stateful blob" — and
        // a probe that itself fails must be a named rejection, not an
        // unwrap/panic or a silent pass. (Saved with a stateless stub of
        // the same name so the checkpoint legitimately carries an empty
        // blob; the load side substitutes the failing-probe actor.)
        let (_dir, toml_path) = temp_toml("actor_blob_probe_failure");
        write_minimal_toml(&toml_path);

        let mut dwelling_a = Dwelling::from_toml_config(&toml_path).expect("build dwelling A");
        dwelling_a
            .add_actor(Box::new(StubActor {
                name: "ProbeActor".to_string(),
            }))
            .unwrap();
        dwelling_a.step().expect("step succeeds");
        let checkpoint = dwelling_a.save_checkpoint().expect("save checkpoint");
        assert!(
            checkpoint
                .actor_states
                .iter()
                .find(|s| s.name == "ProbeActor")
                .expect("checkpoint must carry the actor's entry")
                .blob
                .is_empty(),
            "precondition: the saved blob is empty"
        );

        let mut dwelling_b = Dwelling::from_toml_config(&toml_path).expect("build dwelling B");
        dwelling_b
            .add_actor(Box::new(FailingProbeActor {
                name: "ProbeActor".to_string(),
            }))
            .unwrap();

        let err = dwelling_b.load_checkpoint(checkpoint).expect_err(
            "a failing empty-blob probe must reject the restore, not panic or silently pass",
        );
        let msg = err.to_string();
        assert!(
            msg.contains("cannot verify"),
            "the rejection must name the probe failure; got: {err}"
        );
        assert!(
            msg.contains("ProbeActor"),
            "the rejection must name the actor whose blob could not be verified; got: {err}"
        );
    }

    // An actor whose statefulness is time-varying: constructed with a
    // non-empty state that legitimately drains to empty on the first
    // decide. Its checkpoint blob is empty because the state is *currently*
    // empty — the "no mutable state" convention's letter is satisfied even
    // though its spirit (a stateless actor) is not.
    struct DrainingStateActor {
        name: String,
        counter: u32,
        telemetry: Telemetry,
    }
    impl DrainingStateActor {
        fn new(name: &str, counter: u32) -> Self {
            let mut telemetry = Telemetry::default();
            telemetry.insert("counter", counter as f64);
            Self {
                name: name.to_string(),
                counter,
                telemetry,
            }
        }
    }
    impl crate::Actor for DrainingStateActor {
        fn name(&self) -> &str {
            &self.name
        }
        fn telemetry(&self) -> Option<&Telemetry> {
            Some(&self.telemetry)
        }
        fn decide(
            &mut self,
            _env: &hares_types::EnvironmentState,
            _out: &mut Vec<hares_control::DispatchRequest>,
        ) {
            self.counter = 0;
            self.telemetry.insert("counter", 0.0);
        }
        fn save_state(&self) -> std::result::Result<Vec<u8>, HaresError> {
            if self.counter == 0 {
                return Ok(Vec::new());
            }
            postcard::to_allocvec(&self.counter)
                .map_err(|e| HaresError::Io(format!("DrainingStateActor save_state: {e}")))
        }
        fn load_state(&mut self, data: &[u8]) -> std::result::Result<(), HaresError> {
            if data.is_empty() {
                return Ok(());
            }
            self.counter = postcard::from_bytes(data)
                .map_err(|e| HaresError::Io(format!("DrainingStateActor load_state: {e}")))?;
            self.telemetry.insert("counter", self.counter as f64);
            Ok(())
        }
    }

    #[test]
    fn checkpoint_drained_state_actor_must_not_resume_on_fresh_state() {
        // The empty-blob rule's fourth arm — the misuse shape the probe's
        // ownership rule does not model: an actor whose `save_state` is
        // empty *at save time* (the state legitimately drained) but
        // non-empty on the freshly-constructed restore-time actor. The
        // probe cannot distinguish this from truncation, and the
        // empty-blob `load_state` no-op would silently keep the fresh
        // state — contradicting the checkpoint, which says the state is
        // drained. Whatever the resolution (reject, or restore the drained
        // state), the restore must not report success while the actor
        // carries state the checkpoint says it does not have.
        let (_dir, toml_path) = temp_toml("actor_drained_state");
        write_minimal_toml(&toml_path);

        let mut dwelling_a = Dwelling::from_toml_config(&toml_path).expect("build dwelling A");
        dwelling_a
            .add_actor(Box::new(DrainingStateActor::new("DrainActor", 5)))
            .unwrap();
        dwelling_a.step().expect("step succeeds");

        let mut checkpoint = dwelling_a.save_checkpoint().expect("save checkpoint");
        let state = checkpoint
            .actor_states
            .iter_mut()
            .find(|s| s.name == "DrainActor")
            .expect("checkpoint must carry the actor's entry");
        assert!(
            state.blob.is_empty(),
            "precondition: the drained actor legitimately saves an empty blob"
        );

        let mut dwelling_b = Dwelling::from_toml_config(&toml_path).expect("build dwelling B");
        dwelling_b
            .add_actor(Box::new(DrainingStateActor::new("DrainActor", 5)))
            .unwrap();

        match dwelling_b.load_checkpoint(checkpoint) {
            Ok(()) => {
                let counter = dwelling_b
                    .telemetry()
                    .unwrap()
                    .actor_telemetry
                    .get("DrainActor")
                    .and_then(|channels| channels.get("counter"))
                    .copied();
                assert_eq!(
                    counter,
                    Some(0.0),
                    "a successful restore must leave the actor in the checkpointed \
                     (drained) state, not the freshly-constructed state"
                );
            }
            Err(err) => {
                let msg = err.to_string();
                assert!(
                    msg.contains("DrainActor"),
                    "if the restore rejects the empty blob, the rejection must name \
                     the actor; got: {err}"
                );
            }
        }
    }

    #[test]
    fn an_ev_driver_blob_from_an_earlier_schema_is_rejected_by_name() {
        // A driver blob stamped with an earlier snapshot schema version, as
        // an earlier build writes it, must be rejected at the checkpoint
        // boundary with the named mismatch error, not accepted into a
        // postcard decode that fails (or silently misdecodes) inside
        // `load_state`. The stub-actor gate tests above exercise the gate
        // mechanism but would still pass if this actor's
        // `checkpoint_version()` override were dropped back to the trait
        // default of 1; this test pins the real actor's participation.
        let (_dir, toml_path) = temp_toml("ev_driver_earlier_schema_blob");
        write_minimal_toml(&toml_path);

        let make_driver = || {
            Box::new(EvDriverActor::new(
                "EvDriver:EV1",
                "EV1",
                EvDriverParams {
                    strategy: hares_types::equipment::ChargingStrategy::Immediate {
                        target_soc: 0.9,
                    },
                    plug_in_policy: hares_types::PlugInPolicy::Always,
                    daily_drive_miles: ScheduleSource::Constant(30.0),
                    departure_time: ScheduleSource::Constant(480.0),
                    trip_duration: ScheduleSource::Constant(600.0),
                    arrival_time: None,
                    event_day_ratio: 0.8,
                    fuel_economy_kwh_per_mi: 0.3,
                    capacity_kwh: 60.0,
                    max_charge_kw: 7.2,
                    average_speed_mph: 30.0,
                    range_anxiety_miles: 20.0,
                    away_charge_fraction: 0.0,
                    away_charge_power_kw: 6.6,
                },
                ChaCha8Rng::seed_from_u64(42),
            )) as Box<dyn crate::Actor>
        };

        let mut dwelling_a = Dwelling::from_toml_config(&toml_path).expect("build dwelling A");
        dwelling_a.add_actor(make_driver()).unwrap();
        dwelling_a.step().expect("step succeeds");

        let mut checkpoint = dwelling_a.save_checkpoint().expect("save checkpoint");
        let entry = checkpoint
            .actor_states
            .iter_mut()
            .find(|s| s.name == "EvDriver:EV1")
            .expect("checkpoint must carry the EV driver's state");
        assert_eq!(
            entry.schema_version, 3,
            "the EV driver snapshot schema is version 3 (gate latches and estimate failure)"
        );
        // Tamper: pretend the blob was written by an earlier build.
        entry.schema_version = 1;

        let mut dwelling_b = Dwelling::from_toml_config(&toml_path).expect("build dwelling B");
        dwelling_b.add_actor(make_driver()).unwrap();

        let err = dwelling_b
            .load_checkpoint(checkpoint)
            .expect_err("a v1 EV driver blob must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("actor schema version mismatch")
                && msg.contains("EvDriver:EV1")
                && msg.contains("blob=1")
                && msg.contains("expected=3"),
            "error must name the actor and both versions; got: {msg}"
        );
        assert!(
            !msg.contains("postcard") && !msg.contains("deserialize"),
            "the rejection must come from the version gate, not a postcard decode failure; got: {msg}"
        );
    }

    #[test]
    fn save_checkpoint_propagates_equipment_error() {
        // Build a minimal dwelling, inject equipment whose save_state() always
        // fails, and verify save_checkpoint() returns Err rather than panicking.
        let (_dir, toml_path) = temp_toml("save_checkpoint_failure");
        write_minimal_toml(&toml_path);

        let mut dwelling = Dwelling::from_toml_config(&toml_path).expect("build dwelling");
        dwelling.step().expect("step succeeds");

        #[derive(Clone)]
        struct BadSer {
            descriptor: EquipmentDescriptor,
            ports: Vec<PortDeclaration>,
            telemetry: Telemetry,
            core_output: CoreOutput,
        }

        impl Equipment for BadSer {
            fn descriptor(&self) -> &EquipmentDescriptor {
                &self.descriptor
            }
            fn rename(&mut self, name: String) {
                self.descriptor.name = name;
            }
            fn set_equipment_id(
                &mut self,
                id: EquipmentId,
            ) -> std::result::Result<(), hares_types::HaresError> {
                hares_equipment::apply_identity_write(
                    self.is_initialized(),
                    &mut self.descriptor,
                    id,
                )
            }
            fn ports(&self) -> &[PortDeclaration] {
                &self.ports
            }
            fn init(
                &mut self,
                _: &EquipmentConfig,
                _: &EnvironmentState,
            ) -> std::result::Result<(), HaresError> {
                Ok(())
            }
            fn update_control(&mut self, _: &EnvironmentState) -> OperatingMode {
                OperatingMode::Off
            }
            fn step(
                &mut self,
                _: &EnvironmentState,
                _: Duration,
                _: &mut PortSlots,
            ) -> std::result::Result<(), HaresError> {
                Ok(())
            }
            fn telemetry(&self) -> &Telemetry {
                &self.telemetry
            }
            fn core_output(&self) -> &CoreOutput {
                &self.core_output
            }
            fn apply_signal(&mut self, _: &ControlSignal) -> std::result::Result<(), HaresError> {
                Ok(())
            }
            fn save_state(&self) -> std::result::Result<Vec<u8>, HaresError> {
                Err(HaresError::Equipment("serialization failed".to_string()))
            }
            fn load_state(&mut self, _: &[u8]) -> std::result::Result<(), HaresError> {
                Ok(())
            }
        }

        let bad = BadSer {
            descriptor: EquipmentDescriptor {
                id: EquipmentId(9999),
                name: "BadSer".to_string(),
                end_use: EndUse::OTHER,
                equipment_type: Cow::Borrowed("BadSer"),
                zone: Some(ZoneId(1)),
                fuel: FuelType::Electric,
                stage: ExecutionStage::Independent,
                control_capabilities: ControlCapabilities::empty(),
                core_capabilities: CoreCapabilities::empty(),
                telemetry_fields: vec![],
                zone_type: None,
            },
            ports: vec![],
            telemetry: Telemetry::with_capacity(0),
            core_output: CoreOutput::default(),
        };

        replace_equipment_for_test(&mut dwelling, vec![Box::new(bad)]);
        let result = dwelling.save_checkpoint();
        assert!(
            result.is_err(),
            "save_checkpoint should return Err when equipment save_state fails"
        );
    }

    fn temp_toml(tag: &str) -> (tempfile::TempDir, PathBuf) {
        crate::temp_file::temp_file(&format!("{tag}.toml"))
    }

    fn write_minimal_toml(path: &Path) {
        let content = r#"building_id = 9001

[simulation]
start_time = "2024-06-15T12:00:00Z"
time_res_s = 60
duration_s = 600

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.8

[hvac]
equipment_name = "none"

[weather]
outdoor_temp_c = 20.0
dew_point_c = 10.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325

[schedule]
occupancy = 0.0

[output]
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
write_output = false
master_seed = 0
"#;
        fs::write(path, content).expect("failed to write synthetic TOML");
    }

    // --- Port rollback test equipment ---

    struct FailingPortEquipment {
        descriptor: EquipmentDescriptor,
        telemetry: Telemetry,
        core_output: CoreOutput,
        ports: Vec<PortDeclaration>,
    }

    impl FailingPortEquipment {
        fn new(name: &str, stage: ExecutionStage) -> Self {
            Self {
                descriptor: EquipmentDescriptor {
                    id: EquipmentId(999),
                    name: name.to_string(),
                    end_use: EndUse::OTHER,
                    equipment_type: Cow::Borrowed("FailingPortEquipment"),
                    zone: Some(ZoneId(1)),
                    fuel: FuelType::Electric,
                    stage,
                    control_capabilities: ControlCapabilities::empty(),
                    core_capabilities: CoreCapabilities::empty(),
                    telemetry_fields: vec![],
                    zone_type: None,
                },
                telemetry: Telemetry::default(),
                core_output: CoreOutput::default(),
                ports: vec![
                    PortDeclaration::electrical(),
                    PortDeclaration::thermal(ZoneId(1)),
                ],
            }
        }
    }

    impl Equipment for FailingPortEquipment {
        fn descriptor(&self) -> &EquipmentDescriptor {
            &self.descriptor
        }

        fn rename(&mut self, name: String) {
            self.descriptor.name = name;
        }

        fn set_equipment_id(
            &mut self,
            id: EquipmentId,
        ) -> std::result::Result<(), hares_types::HaresError> {
            hares_equipment::apply_identity_write(self.is_initialized(), &mut self.descriptor, id)
        }

        fn ports(&self) -> &[PortDeclaration] {
            &self.ports
        }

        fn init(
            &mut self,
            _config: &EquipmentConfig,
            _env: &hares_types::EnvironmentState,
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }

        fn update_control(&mut self, _env: &hares_types::EnvironmentState) -> OperatingMode {
            OperatingMode::Off
        }

        fn step(
            &mut self,
            _env: &hares_types::EnvironmentState,
            _dt: Duration,
            ports: &mut PortSlots,
        ) -> std::result::Result<(), hares_types::HaresError> {
            // Write electrical contribution then fail — this exercises the
            // rollback path.
            ports.accumulate(&PortContribution::Electrical {
                active_power_w: 500.0,
                reactive_power_kvar: 0.1,
            })?;
            ports.accumulate(&PortContribution::Thermal {
                zone: ZoneId(1),
                sensible_gain_w: 1000.0,
                radiant_gain_w: 200.0,
                latent_gain_w: 50.0,
                category: ThermalCategory::InternalGain,
            })?;
            Err(HaresError::Equipment("simulated step failure".to_string()))
        }

        fn telemetry(&self) -> &Telemetry {
            &self.telemetry
        }

        fn core_output(&self) -> &CoreOutput {
            &self.core_output
        }

        fn save_state(&self) -> std::result::Result<Vec<u8>, hares_types::HaresError> {
            Ok(vec![])
        }

        fn load_state(
            &mut self,
            _state: &[u8],
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }

        fn apply_signal(
            &mut self,
            _signal: &ControlSignal,
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }
    }

    struct FunctionalPortEquipment {
        descriptor: EquipmentDescriptor,
        telemetry: Telemetry,
        core_output: CoreOutput,
        power_w: f64,
        ports: Vec<PortDeclaration>,
    }

    impl FunctionalPortEquipment {
        fn new(name: &str, power_w: f64, stage: ExecutionStage) -> Self {
            Self {
                descriptor: EquipmentDescriptor {
                    id: EquipmentId(1000),
                    name: name.to_string(),
                    end_use: EndUse::OTHER,
                    equipment_type: Cow::Borrowed("FunctionalPortEquipment"),
                    zone: Some(ZoneId(1)),
                    fuel: FuelType::Electric,
                    stage,
                    control_capabilities: ControlCapabilities::empty(),
                    core_capabilities: CoreCapabilities::ELECTRIC,
                    telemetry_fields: vec![],
                    zone_type: None,
                },
                telemetry: Telemetry::default(),
                core_output: CoreOutput::default(),
                power_w,
                ports: vec![PortDeclaration::electrical()],
            }
        }
    }

    impl Equipment for FunctionalPortEquipment {
        fn descriptor(&self) -> &EquipmentDescriptor {
            &self.descriptor
        }

        fn rename(&mut self, name: String) {
            self.descriptor.name = name;
        }

        fn set_equipment_id(
            &mut self,
            id: EquipmentId,
        ) -> std::result::Result<(), hares_types::HaresError> {
            hares_equipment::apply_identity_write(self.is_initialized(), &mut self.descriptor, id)
        }

        fn ports(&self) -> &[PortDeclaration] {
            &self.ports
        }

        fn init(
            &mut self,
            _config: &EquipmentConfig,
            _env: &hares_types::EnvironmentState,
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }

        fn update_control(&mut self, _env: &hares_types::EnvironmentState) -> OperatingMode {
            OperatingMode::Off
        }

        fn step(
            &mut self,
            _env: &hares_types::EnvironmentState,
            _dt: Duration,
            ports: &mut PortSlots,
        ) -> std::result::Result<(), hares_types::HaresError> {
            ports.accumulate(&PortContribution::Electrical {
                active_power_w: self.power_w,
                reactive_power_kvar: 0.0,
            })?;
            // Keep CoreOutput consistent with the port contribution above
            // (validate_port_core_electrical_consistency is unconditional).
            self.core_output.flows.electric_kw = Some(hares_types::ElectricPower::consumption(
                self.power_w / 1000.0,
            )?);
            Ok(())
        }

        fn telemetry(&self) -> &Telemetry {
            &self.telemetry
        }

        fn core_output(&self) -> &CoreOutput {
            &self.core_output
        }

        fn save_state(&self) -> std::result::Result<Vec<u8>, hares_types::HaresError> {
            Ok(vec![])
        }

        fn load_state(
            &mut self,
            _state: &[u8],
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }

        fn apply_signal(
            &mut self,
            _signal: &ControlSignal,
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }
    }

    struct SpyPortEquipment {
        descriptor: EquipmentDescriptor,
        telemetry: Telemetry,
        core_output: CoreOutput,
        ports: Vec<PortDeclaration>,
    }

    impl SpyPortEquipment {
        fn new(name: &str, stage: ExecutionStage) -> Self {
            Self {
                descriptor: EquipmentDescriptor {
                    id: EquipmentId(1001),
                    name: name.to_string(),
                    end_use: EndUse::OTHER,
                    equipment_type: Cow::Borrowed("SpyPortEquipment"),
                    zone: Some(ZoneId(1)),
                    fuel: FuelType::Electric,
                    stage,
                    control_capabilities: ControlCapabilities::empty(),
                    core_capabilities: CoreCapabilities::empty(),
                    telemetry_fields: vec![],
                    zone_type: None,
                },
                telemetry: Telemetry::default(),
                core_output: CoreOutput::default(),
                ports: vec![PortDeclaration::electrical()],
            }
        }
    }

    impl Equipment for SpyPortEquipment {
        fn descriptor(&self) -> &EquipmentDescriptor {
            &self.descriptor
        }

        fn rename(&mut self, name: String) {
            self.descriptor.name = name;
        }

        fn set_equipment_id(
            &mut self,
            id: EquipmentId,
        ) -> std::result::Result<(), hares_types::HaresError> {
            hares_equipment::apply_identity_write(self.is_initialized(), &mut self.descriptor, id)
        }

        fn ports(&self) -> &[PortDeclaration] {
            &self.ports
        }

        fn init(
            &mut self,
            _config: &EquipmentConfig,
            _env: &hares_types::EnvironmentState,
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }

        fn update_control(&mut self, _env: &hares_types::EnvironmentState) -> OperatingMode {
            OperatingMode::Off
        }

        fn step(
            &mut self,
            _env: &hares_types::EnvironmentState,
            _dt: Duration,
            ports: &mut PortSlots,
        ) -> std::result::Result<(), hares_types::HaresError> {
            // Record the electrical load state this equipment observes.
            self.telemetry
                .insert("observed_load_power_w", ports.electrical.load_power_w);
            Ok(())
        }

        fn telemetry(&self) -> &Telemetry {
            &self.telemetry
        }

        fn core_output(&self) -> &CoreOutput {
            &self.core_output
        }

        fn save_state(&self) -> std::result::Result<Vec<u8>, hares_types::HaresError> {
            Ok(vec![])
        }

        fn load_state(
            &mut self,
            _state: &[u8],
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }

        fn apply_signal(
            &mut self,
            _signal: &ControlSignal,
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }
    }

    #[test]
    fn failed_equipment_step_prevents_snapshot_and_surfaces_warning() {
        let base_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/bestest/600.toml");
        let mut dwelling = Dwelling::from_toml_config_with_write_output(&base_path, Some(false))
            .expect("build dwelling");

        let failing = FailingPortEquipment::new("FailingEq", ExecutionStage::Independent);
        replace_equipment_for_test(&mut dwelling, vec![Box::new(failing)]);

        dwelling.run_timestep(false).expect("dwelling step");

        // The failing equipment's step() returns Err, so its core output must
        // not be snapshotted into equipment_core.
        let failing_id = EquipmentId(999);
        assert!(
            !dwelling.latest_env.equipment_core.contains_key(&failing_id),
            "failed equipment core output must not be in equipment_core; id={:?}",
            failing_id
        );

        // Warning must be pushed.
        assert!(
            dwelling.warnings.iter().any(|w| w.contains("FailingEq")),
            "warning must contain failing equipment name; warnings: {:?}",
            dwelling.warnings
        );
    }

    #[test]
    fn failing_equipment_does_not_contaminate_downstream_equipment_ports() {
        let base_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/bestest/600.toml");
        let mut dwelling = Dwelling::from_toml_config_with_write_output(&base_path, Some(false))
            .expect("build dwelling");

        // Failing equipment writes 500 W to electrical port, then Err.
        let failing = FailingPortEquipment::new("FailingEq", ExecutionStage::Independent);
        // Spy observes the electrical port state when its step() is called.
        let spy = SpyPortEquipment::new("SpyEq", ExecutionStage::Electrical);

        replace_equipment_for_test(&mut dwelling, vec![Box::new(failing), Box::new(spy)]);

        dwelling.run_timestep(false).expect("dwelling step");

        // After rollback, the spy should see 0 W from the failing equipment.
        // If rollback didn't happen, spy would see 500 W.
        assert_eq!(dwelling.equipment[1].descriptor().name, "SpyEq",);
        // The spy records observed_load_power_w in telemetry during step().
        let spy_observed = dwelling.equipment[1]
            .telemetry()
            .get("observed_load_power_w")
            .unwrap_or(-1.0);
        assert_eq!(
            spy_observed, 0.0,
            "spy should observe 0 W load power after rollback; got {}",
            spy_observed
        );
        assert!(
            dwelling.warnings.iter().any(|w| w.contains("FailingEq")),
            "warning must contain failing equipment name"
        );
        // The failing equipment's core output must not be snapshotted.
        let failing_id = EquipmentId(999);
        let spy_id = EquipmentId(1001);
        assert!(
            !dwelling.latest_env.equipment_core.contains_key(&failing_id),
            "failed equipment core output must not be in equipment_core; id={:?}",
            failing_id
        );
        assert!(
            dwelling.latest_env.equipment_core.contains_key(&spy_id),
            "spy equipment core output must be snapshotted; id={:?}",
            spy_id
        );
    }

    #[test]
    fn failed_equipment_core_output_not_snapshotted_to_environment() {
        let base_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/bestest/600.toml");
        let mut dwelling = Dwelling::from_toml_config_with_write_output(&base_path, Some(false))
            .expect("build dwelling");

        let failing = FailingPortEquipment::new("FailingEq", ExecutionStage::Independent);
        let functional =
            FunctionalPortEquipment::new("FunctionalEq", 100.0, ExecutionStage::Independent);

        replace_equipment_for_test(&mut dwelling, vec![Box::new(failing), Box::new(functional)]);

        dwelling.run_timestep(false).expect("dwelling step");

        // Failing equipment's core output must NOT be in equipment_core.
        let failing_id = EquipmentId(999);
        let functional_id = EquipmentId(1000);

        assert!(
            !dwelling.latest_env.equipment_core.contains_key(&failing_id),
            "failed equipment's core output must not be snapshotted; id={:?}",
            failing_id
        );
        assert!(
            dwelling
                .latest_env
                .equipment_core
                .contains_key(&functional_id),
            "functional equipment's core output must be snapshotted; id={:?}",
            functional_id
        );
    }

    /// Equipment that books its electrical power and core_output on every
    /// successful step, then fails on the step after `successful_steps`
    /// successes (ports written first, then an error, so the rollback path
    /// runs with a committed core_output from the previous step).
    struct FlakyPortEquipment {
        descriptor: EquipmentDescriptor,
        telemetry: Telemetry,
        core_output: CoreOutput,
        power_w: f64,
        remaining_successes: u32,
        ports: Vec<PortDeclaration>,
    }

    impl FlakyPortEquipment {
        fn new(name: &str, power_w: f64, successful_steps: u32) -> Self {
            Self {
                descriptor: EquipmentDescriptor {
                    id: EquipmentId(1002),
                    name: name.to_string(),
                    end_use: EndUse::OTHER,
                    equipment_type: Cow::Borrowed("FlakyPortEquipment"),
                    zone: Some(ZoneId(1)),
                    fuel: FuelType::Electric,
                    stage: ExecutionStage::Independent,
                    control_capabilities: ControlCapabilities::empty(),
                    core_capabilities: CoreCapabilities::ELECTRIC,
                    telemetry_fields: vec![],
                    zone_type: None,
                },
                telemetry: Telemetry::default(),
                core_output: CoreOutput::default(),
                power_w,
                remaining_successes: successful_steps,
                ports: vec![PortDeclaration::electrical()],
            }
        }
    }

    impl Equipment for FlakyPortEquipment {
        fn descriptor(&self) -> &EquipmentDescriptor {
            &self.descriptor
        }

        fn rename(&mut self, name: String) {
            self.descriptor.name = name;
        }

        fn set_equipment_id(
            &mut self,
            id: EquipmentId,
        ) -> std::result::Result<(), hares_types::HaresError> {
            hares_equipment::apply_identity_write(self.is_initialized(), &mut self.descriptor, id)
        }

        fn ports(&self) -> &[PortDeclaration] {
            &self.ports
        }

        fn init(
            &mut self,
            _config: &EquipmentConfig,
            _env: &hares_types::EnvironmentState,
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }

        fn update_control(&mut self, _env: &hares_types::EnvironmentState) -> OperatingMode {
            OperatingMode::Off
        }

        fn step(
            &mut self,
            _env: &hares_types::EnvironmentState,
            _dt: Duration,
            ports: &mut PortSlots,
        ) -> std::result::Result<(), hares_types::HaresError> {
            if self.remaining_successes == 0 {
                // Book the port contribution, then fail: the rollback removes
                // the contribution while core_output keeps the last
                // committed power.
                ports.accumulate(&PortContribution::Electrical {
                    active_power_w: self.power_w,
                    reactive_power_kvar: 0.0,
                })?;
                return Err(HaresError::Equipment("simulated step failure".to_string()));
            }
            self.remaining_successes -= 1;
            ports.accumulate(&PortContribution::Electrical {
                active_power_w: self.power_w,
                reactive_power_kvar: 0.0,
            })?;
            self.core_output.flows.electric_kw = Some(hares_types::ElectricPower::consumption(
                self.power_w / 1000.0,
            )?);
            Ok(())
        }

        fn telemetry(&self) -> &Telemetry {
            &self.telemetry
        }

        fn core_output(&self) -> &CoreOutput {
            &self.core_output
        }

        fn save_state(&self) -> std::result::Result<Vec<u8>, hares_types::HaresError> {
            Ok(vec![])
        }

        fn load_state(
            &mut self,
            _state: &[u8],
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }

        fn apply_signal(
            &mut self,
            _signal: &ControlSignal,
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }
    }

    /// A tolerated equipment step failure must reconcile the electrical
    /// accounting: the rollback removes the failed equipment's port
    /// contributions, so its stale core_output power must not be summed
    /// against the solver total. Before the reconciliation the consistency
    /// check failed the step right after the tolerated one, contradicting
    /// the tolerate-and-rollback design.
    #[test]
    fn tolerated_step_failure_reconciles_the_electrical_sum() {
        let base_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/bestest/600.toml");
        let mut dwelling = Dwelling::from_toml_config_with_write_output(&base_path, Some(false))
            .expect("build dwelling");

        let flaky = FlakyPortEquipment::new("FlakyEq", 500.0, 1);
        replace_equipment_for_test(&mut dwelling, vec![Box::new(flaky)]);

        // Step 1: the equipment succeeds and books 500 W.
        dwelling.run_timestep(false).expect("first dwelling step");

        // Step 2: the equipment writes its port contribution, then fails. The
        // step is tolerated (rollback + warning); the electrical consistency
        // check must see the same equipment set on both sides: the sum skips
        // the failed equipment exactly as the rolled-back ports do.
        dwelling
            .run_timestep(false)
            .expect("a tolerated equipment failure must not desync the electrical sum");

        assert_eq!(dwelling.health.port_rollbacks, 1, "the failure is counted");
        assert!(
            dwelling.warnings.iter().any(|w| w.contains("FlakyEq")),
            "the failure stays loud as a warning; warnings: {:?}",
            dwelling.warnings.iter().collect::<Vec<_>>()
        );
    }

    // ── Run-health counters and always-on invariant enforcement ──

    /// Equipment that writes a non-finite value to a telemetry key during
    /// step; drives the Telemetry non-finite latch through the dwelling's
    /// enforcement.
    struct NonFiniteTelemetryEquipment {
        descriptor: EquipmentDescriptor,
        telemetry: Telemetry,
        core_output: CoreOutput,
    }

    impl NonFiniteTelemetryEquipment {
        fn new(name: &str) -> Self {
            Self {
                descriptor: EquipmentDescriptor {
                    id: EquipmentId(0),
                    name: name.to_string(),
                    end_use: EndUse::OTHER,
                    equipment_type: Cow::Borrowed("NonFiniteTelemetryEquipment"),
                    zone: Some(ZoneId(1)),
                    fuel: FuelType::Electric,
                    stage: ExecutionStage::Independent,
                    control_capabilities: ControlCapabilities::empty(),
                    core_capabilities: CoreCapabilities::empty(),
                    telemetry_fields: vec![],
                    zone_type: None,
                },
                telemetry: Telemetry::default(),
                core_output: CoreOutput::default(),
            }
        }
    }

    impl Equipment for NonFiniteTelemetryEquipment {
        fn descriptor(&self) -> &EquipmentDescriptor {
            &self.descriptor
        }

        fn rename(&mut self, name: String) {
            self.descriptor.name = name;
        }

        fn set_equipment_id(&mut self, id: EquipmentId) -> std::result::Result<(), HaresError> {
            hares_equipment::apply_identity_write(self.is_initialized(), &mut self.descriptor, id)
        }

        fn ports(&self) -> &[PortDeclaration] {
            &[]
        }

        fn init(
            &mut self,
            _config: &EquipmentConfig,
            _env: &hares_types::EnvironmentState,
        ) -> std::result::Result<(), HaresError> {
            Ok(())
        }

        fn update_control(&mut self, _env: &hares_types::EnvironmentState) -> OperatingMode {
            OperatingMode::Off
        }

        fn step(
            &mut self,
            _env: &hares_types::EnvironmentState,
            _dt: Duration,
            _ports: &mut PortSlots,
        ) -> std::result::Result<(), HaresError> {
            // The non-finite write is rejected and latched by Telemetry in
            // every build profile; the dwelling turns the latch into a run
            // failure.
            self.telemetry.insert("corrupted_w", f64::NAN);
            Ok(())
        }

        fn telemetry(&self) -> &Telemetry {
            &self.telemetry
        }

        fn core_output(&self) -> &CoreOutput {
            &self.core_output
        }

        fn save_state(&self) -> std::result::Result<Vec<u8>, HaresError> {
            Ok(vec![])
        }

        fn load_state(&mut self, _state: &[u8]) -> std::result::Result<(), HaresError> {
            Ok(())
        }

        fn apply_signal(&mut self, _signal: &ControlSignal) -> std::result::Result<(), HaresError> {
            Ok(())
        }
    }

    /// Equipment that registers one telemetry key at init and then `set`s
    /// an unregistered key during step; drives the dwelling's unknown-key
    /// step-end check the same way [`NonFiniteTelemetryEquipment`] drives
    /// the non-finite one.
    struct UnknownKeyTelemetryEquipment {
        descriptor: EquipmentDescriptor,
        telemetry: Telemetry,
        core_output: CoreOutput,
    }

    impl UnknownKeyTelemetryEquipment {
        fn new(name: &str) -> Self {
            Self {
                descriptor: EquipmentDescriptor {
                    id: EquipmentId(0),
                    name: name.to_string(),
                    end_use: EndUse::OTHER,
                    equipment_type: Cow::Borrowed("UnknownKeyTelemetryEquipment"),
                    zone: Some(ZoneId(1)),
                    fuel: FuelType::Electric,
                    stage: ExecutionStage::Independent,
                    control_capabilities: ControlCapabilities::empty(),
                    core_capabilities: CoreCapabilities::empty(),
                    telemetry_fields: vec![],
                    zone_type: None,
                },
                telemetry: Telemetry::default(),
                core_output: CoreOutput::default(),
            }
        }
    }

    impl Equipment for UnknownKeyTelemetryEquipment {
        fn descriptor(&self) -> &EquipmentDescriptor {
            &self.descriptor
        }

        fn rename(&mut self, name: String) {
            self.descriptor.name = name;
        }

        fn set_equipment_id(&mut self, id: EquipmentId) -> std::result::Result<(), HaresError> {
            hares_equipment::apply_identity_write(self.is_initialized(), &mut self.descriptor, id)
        }

        fn ports(&self) -> &[PortDeclaration] {
            &[]
        }

        fn init(
            &mut self,
            _config: &EquipmentConfig,
            _env: &hares_types::EnvironmentState,
        ) -> std::result::Result<(), HaresError> {
            // The one registered key: `set` may only target keys
            // pre-populated here.
            self.telemetry.insert("registered_w", 0.0);
            Ok(())
        }

        fn update_control(&mut self, _env: &hares_types::EnvironmentState) -> OperatingMode {
            OperatingMode::Off
        }

        fn step(
            &mut self,
            _env: &hares_types::EnvironmentState,
            _dt: Duration,
            _ports: &mut PortSlots,
        ) -> std::result::Result<(), HaresError> {
            // The unknown-key write is dropped and latched by Telemetry in
            // every build profile; the dwelling turns the latch into a run
            // failure.
            self.telemetry.set("unregistered_w", 1.0);
            Ok(())
        }

        fn telemetry(&self) -> &Telemetry {
            &self.telemetry
        }

        fn core_output(&self) -> &CoreOutput {
            &self.core_output
        }

        fn save_state(&self) -> std::result::Result<Vec<u8>, HaresError> {
            Ok(vec![])
        }

        fn load_state(&mut self, _state: &[u8]) -> std::result::Result<(), HaresError> {
            Ok(())
        }

        fn apply_signal(&mut self, _signal: &ControlSignal) -> std::result::Result<(), HaresError> {
            Ok(())
        }
    }

    /// Equipment that deposits a NaN sensible gain into a zone's thermal
    /// port during step; drives the zone-temperature NaN check.
    struct NanThermalGainEquipment {
        descriptor: EquipmentDescriptor,
        telemetry: Telemetry,
        core_output: CoreOutput,
        zone: ZoneId,
        ports: [PortDeclaration; 1],
    }

    impl NanThermalGainEquipment {
        fn new(name: &str, zone: ZoneId) -> Self {
            Self {
                descriptor: EquipmentDescriptor {
                    id: EquipmentId(0),
                    name: name.to_string(),
                    end_use: EndUse::OTHER,
                    equipment_type: Cow::Borrowed("NanThermalGainEquipment"),
                    zone: Some(zone),
                    fuel: FuelType::Electric,
                    stage: ExecutionStage::Independent,
                    control_capabilities: ControlCapabilities::empty(),
                    core_capabilities: CoreCapabilities::empty(),
                    telemetry_fields: vec![],
                    zone_type: None,
                },
                telemetry: Telemetry::default(),
                core_output: CoreOutput::default(),
                zone,
                ports: [PortDeclaration::thermal(zone)],
            }
        }
    }

    impl Equipment for NanThermalGainEquipment {
        fn descriptor(&self) -> &EquipmentDescriptor {
            &self.descriptor
        }

        fn rename(&mut self, name: String) {
            self.descriptor.name = name;
        }

        fn set_equipment_id(&mut self, id: EquipmentId) -> std::result::Result<(), HaresError> {
            hares_equipment::apply_identity_write(self.is_initialized(), &mut self.descriptor, id)
        }

        fn ports(&self) -> &[PortDeclaration] {
            &self.ports
        }

        fn init(
            &mut self,
            _config: &EquipmentConfig,
            _env: &hares_types::EnvironmentState,
        ) -> std::result::Result<(), HaresError> {
            Ok(())
        }

        fn update_control(&mut self, _env: &hares_types::EnvironmentState) -> OperatingMode {
            OperatingMode::Off
        }

        fn step(
            &mut self,
            _env: &hares_types::EnvironmentState,
            _dt: Duration,
            ports: &mut PortSlots,
        ) -> std::result::Result<(), HaresError> {
            ports.accumulate(&PortContribution::Thermal {
                zone: self.zone,
                sensible_gain_w: f64::NAN,
                radiant_gain_w: 0.0,
                latent_gain_w: 0.0,
                category: ThermalCategory::InternalGain,
            })?;
            Ok(())
        }

        fn telemetry(&self) -> &Telemetry {
            &self.telemetry
        }

        fn core_output(&self) -> &CoreOutput {
            &self.core_output
        }

        fn save_state(&self) -> std::result::Result<Vec<u8>, HaresError> {
            Ok(vec![])
        }

        fn load_state(&mut self, _state: &[u8]) -> std::result::Result<(), HaresError> {
            Ok(())
        }

        fn apply_signal(&mut self, _signal: &ControlSignal) -> std::result::Result<(), HaresError> {
            Ok(())
        }
    }

    fn bestest_dwelling() -> Dwelling {
        let base_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/bestest/600.toml");
        Dwelling::from_toml_config_with_write_output(&base_path, Some(false))
            .expect("build bestest dwelling")
    }

    /// Builds an initialized Electric Boiler declaring `loop_id` with
    /// `fluid_type`: a real, fully-functional fluid-port equipment, not a
    /// stub, so a dwelling carrying one can step for real.
    fn boiler_on_loop(
        name: &str,
        loop_id: u16,
        fluid_type: FluidType,
        env: &EnvironmentState,
    ) -> Box<dyn Equipment> {
        let config = EquipmentConfig::from_typed(
            name.to_string(),
            "Electric Boiler".to_string(),
            hares_equipment::ElectricBoilerConfig {
                zone_id: Some(1),
                loop_id: Some(loop_id),
                capacity_w: 0.0,
                fluid_type,
                ..hares_equipment::ElectricBoilerConfig::default()
            },
        )
        .expect("boiler config");
        let registry = EquipmentRegistry::new();
        let mut eq = registry
            .create("Electric Boiler", config.clone())
            .expect("boiler equipment");
        eq.init(&config, env).expect("init boiler");
        eq
    }

    /// The reason inside the `RejectedEquipment` every rejected add or
    /// replace returns.
    fn rejection_reason(err: &HaresError) -> &HaresError {
        match err {
            HaresError::RejectedEquipment { reason, .. } => reason,
            other => panic!("a rejected add or replace returns RejectedEquipment, got {other:?}"),
        }
    }

    /// `add_equipment` rejecting a fluid-type-conflicting candidate must
    /// leave the dwelling exactly as it was: it must not push the candidate
    /// and then fail.
    #[test]
    fn rejected_add_leaves_roster_unchanged() {
        let mut dwelling = bestest_dwelling();
        let water = boiler_on_loop("Boiler-Water", 7, FluidType::Water, &dwelling.latest_env);
        dwelling
            .add_equipment(water)
            .expect("the first declarer of loop 7 is accepted");

        let names_before: Vec<String> = dwelling
            .equipment()
            .iter()
            .map(|e| e.descriptor().name.clone())
            .collect();
        let loop_type_before = dwelling.fluid_solver.loop_fluid_type(LoopId(7));

        let glycol = boiler_on_loop("Boiler-Glycol", 7, FluidType::Glycol, &dwelling.latest_env);
        let err = dwelling
            .add_equipment(glycol)
            .expect_err("a second declarer of loop 7 with a conflicting fluid type is rejected");
        assert!(
            matches!(rejection_reason(&err), HaresError::Envelope(_)),
            "got: {err:?}"
        );

        let names_after: Vec<String> = dwelling
            .equipment()
            .iter()
            .map(|e| e.descriptor().name.clone())
            .collect();
        assert_eq!(
            names_before, names_after,
            "a rejected add must not leave the candidate in the vector"
        );
        assert_eq!(
            dwelling.fluid_solver.loop_fluid_type(LoopId(7)),
            loop_type_before,
            "a rejected add must not perturb the fluid-loop map"
        );
    }

    /// A fully rejected `add_equipment` must not advance the never-reused
    /// equipment-id counter: the candidate's identity is only assigned in
    /// the plan, and only written to `self.next_equipment_id` once the
    /// plan is accepted and installed.
    #[test]
    fn rejected_add_does_not_advance_the_equipment_id_counter() {
        let mut dwelling = bestest_dwelling();
        let water = boiler_on_loop("Boiler-Water", 7, FluidType::Water, &dwelling.latest_env);
        dwelling
            .add_equipment(water)
            .expect("the first declarer of loop 7 is accepted");

        let next_id_before = dwelling.next_equipment_id;

        let glycol = boiler_on_loop("Boiler-Glycol", 7, FluidType::Glycol, &dwelling.latest_env);
        dwelling
            .add_equipment(glycol)
            .expect_err("a second declarer of loop 7 with a conflicting fluid type is rejected");

        assert_eq!(
            dwelling.next_equipment_id, next_id_before,
            "a fully rejected add must not advance the never-reused equipment id counter"
        );
    }

    /// A fully rejected `replace_equipment` must not advance the
    /// never-reused equipment-id counter either: the same identity
    /// assignment, planned but not written until the replacement is
    /// actually installed.
    #[test]
    fn rejected_replace_does_not_advance_the_equipment_id_counter() {
        let mut dwelling = bestest_dwelling();
        let water = boiler_on_loop("Boiler-Water", 7, FluidType::Water, &dwelling.latest_env);
        dwelling.add_equipment(water).expect("accepted");
        let furnace = FunctionalPortEquipment::new("Furnace", 0.0, ExecutionStage::Independent);
        dwelling.add_equipment(Box::new(furnace)).expect("accepted");

        let next_id_before = dwelling.next_equipment_id;

        let glycol_replacement =
            boiler_on_loop("Furnace", 7, FluidType::Glycol, &dwelling.latest_env);
        match dwelling.replace_equipment("Furnace", glycol_replacement) {
            Err(_) => {}
            Ok(_) => panic!("a replacement conflicting with loop 7's fluid type must be rejected"),
        }

        assert_eq!(
            dwelling.next_equipment_id, next_id_before,
            "a fully rejected replace must not advance the never-reused equipment id counter"
        );
    }

    /// `replace_equipment` rejecting a fluid-type-conflicting candidate
    /// must keep the original equipment installed, not destroy it and
    /// install the rejected replacement.
    #[test]
    fn rejected_replace_keeps_the_original() {
        let mut dwelling = bestest_dwelling();
        let water = boiler_on_loop("Boiler-Water", 7, FluidType::Water, &dwelling.latest_env);
        dwelling.add_equipment(water).expect("accepted");
        let furnace = FunctionalPortEquipment::new("Furnace", 0.0, ExecutionStage::Independent);
        dwelling.add_equipment(Box::new(furnace)).expect("accepted");
        let count_before = dwelling.equipment().len();
        let furnace_id = dwelling
            .equipment()
            .iter()
            .find(|e| e.descriptor().name == "Furnace")
            .expect("Furnace present")
            .descriptor()
            .id;

        let glycol_replacement =
            boiler_on_loop("Furnace", 7, FluidType::Glycol, &dwelling.latest_env);
        let err = match dwelling.replace_equipment("Furnace", glycol_replacement) {
            Err(err) => err,
            Ok(_) => panic!("a replacement conflicting with loop 7's fluid type must be rejected"),
        };
        assert!(
            matches!(rejection_reason(&err), HaresError::Envelope(_)),
            "got: {err:?}"
        );

        let furnace = dwelling
            .equipment()
            .iter()
            .find(|e| e.descriptor().name == "Furnace")
            .expect("the original Furnace must still be installed");
        assert_eq!(
            furnace.descriptor().id,
            furnace_id,
            "the surviving equipment must be the original instance, not the rejected replacement"
        );
        assert_eq!(
            dwelling.equipment().len(),
            count_before,
            "no equipment must be lost or gained"
        );
    }

    /// A dwelling whose every roster plan fails: one row is already
    /// recorded, so the schema is frozen, and the frozen column index
    /// carries "Battery"'s Electric Power and Mode columns but not the SOC
    /// column its end-use class demands at verbosity 3 (the drift
    /// `build_equipment_column_map_errors_on_schema_drift_for_known_equipment`
    /// pins). The returned directory owns the recorder's file.
    fn dwelling_whose_roster_plans_drift() -> (Dwelling, tempfile::TempDir) {
        let mut dwelling = bestest_dwelling();
        let mut battery = TestEquipment::new("Battery", ControlCapabilities::empty());
        battery.descriptor.end_use = EndUse::BATTERY;
        replace_equipment_for_test(&mut dwelling, vec![Box::new(battery)]);

        let tmp = tempfile::tempdir().expect("tempdir");
        let schema = hares_io::build_schema(&[], 0, &[]);
        let value_count = schema.fields().len() - 1;
        let mut recorder = StreamingRecorder::new(
            schema,
            10,
            hares_io::OutputFormat::Csv,
            &tmp.path().join("drift.csv"),
            false,
            hares_io::RotationPolicy::None,
        )
        .expect("recorder");
        recorder
            .push_row("2023-01-01T00:00:00", &vec![0.0; value_count])
            .expect("push row");
        dwelling.recorder = Some(recorder);
        dwelling.write_output = true;
        dwelling.output_verbosity = 3;
        let column_index = &mut dwelling.roster.output_column_index;
        column_index.clear();
        column_index.insert("Battery Electric Power (kW)".to_string(), 0);
        column_index.insert("Battery Mode (-)".to_string(), 1);
        (dwelling, tmp)
    }

    fn actor_names(dwelling: &Dwelling) -> Vec<String> {
        dwelling
            .actors
            .iter()
            .map(|a| a.name().to_string())
            .collect()
    }

    /// Any entrance whose plan fails, not only a fluid-loop conflict,
    /// must leave the actor roster and schedule untouched.
    #[test]
    fn rejected_actor_add_leaves_roster_unchanged() {
        let (mut dwelling, _output_dir) = dwelling_whose_roster_plans_drift();

        let actors_before = dwelling.actor_count();
        let err = dwelling
            .add_actor(Box::new(StubActor {
                name: "Observer".to_string(),
            }))
            .expect_err("a plan drifting on pre-existing equipment must reject the actor add");
        assert!(
            matches!(&err, HaresError::InvariantViolation { check_name, .. }
                if check_name.contains("Battery")),
            "got: {err:?}"
        );
        assert_eq!(
            dwelling.actor_count(),
            actors_before,
            "a rejected actor add must not leave the actor registered"
        );
    }

    /// Before the first step, removing a loop's only declarer frees its
    /// fluid type: the loop-type map must come from the live equipment's
    /// declarations, not from a retained port-slot accumulator.
    #[test]
    fn removed_loop_declarer_frees_its_fluid_type_before_first_step() {
        let mut dwelling = bestest_dwelling();
        let glycol = boiler_on_loop("Boiler-Glycol", 7, FluidType::Glycol, &dwelling.latest_env);
        dwelling.add_equipment(glycol).expect("accepted");
        assert_eq!(
            dwelling.fluid_solver.loop_fluid_type(LoopId(7)),
            Some(FluidType::Glycol)
        );

        dwelling
            .remove_equipment("Boiler-Glycol")
            .expect("remove the only declarer of loop 7");

        let water = boiler_on_loop("Boiler-Water", 7, FluidType::Water, &dwelling.latest_env);
        dwelling
            .add_equipment(water)
            .expect("loop 7 is free once its only declarer left");
        assert_eq!(
            dwelling.fluid_solver.loop_fluid_type(LoopId(7)),
            Some(FluidType::Water),
            "the loop-type map must resolve from the live declaration alone"
        );
    }

    /// After the first step, a candidate declaring a running loop with a
    /// conflicting fluid type is rejected by the frozen-table guard
    /// (`ensure_ports_satisfied`), and the rejection leaves the roster,
    /// the fluid-loop map and the output schema exactly as they were.
    #[test]
    fn post_step_conflicting_fluid_declaration_is_rejected() {
        let mut dwelling = bestest_dwelling();
        let water = boiler_on_loop("Boiler-Water", 7, FluidType::Water, &dwelling.latest_env);
        dwelling.add_equipment(water).expect("accepted pre-step");

        dwelling
            .run_timestep(false)
            .expect("the dwelling steps once, freezing the port-slot table");

        let names_before: Vec<String> = dwelling
            .equipment()
            .iter()
            .map(|e| e.descriptor().name.clone())
            .collect();
        let loop_type_before = dwelling.fluid_solver.loop_fluid_type(LoopId(7));
        let column_index_before = dwelling.roster.output_column_index.clone();

        let glycol = boiler_on_loop("Boiler-Glycol", 7, FluidType::Glycol, dwelling.latest_env());
        let err = dwelling
            .add_equipment(glycol)
            .expect_err("a post-step conflicting fluid declaration is rejected");
        assert!(
            matches!(rejection_reason(&err), HaresError::Equipment(_)),
            "got: {err:?}"
        );

        let names_after: Vec<String> = dwelling
            .equipment()
            .iter()
            .map(|e| e.descriptor().name.clone())
            .collect();
        assert_eq!(
            names_before, names_after,
            "a rejected post-step add must not leave the candidate in the vector"
        );
        assert_eq!(
            dwelling.fluid_solver.loop_fluid_type(LoopId(7)),
            loop_type_before,
            "a rejected post-step add must not perturb the frozen fluid-loop map"
        );
        assert_eq!(
            dwelling.roster.output_column_index, column_index_before,
            "a rejected post-step add must not perturb the frozen output schema"
        );
    }

    /// `set_tariff` is all-or-nothing: when the roster plan for the
    /// tariff's built-in actors is rejected, the dwelling keeps no tariff
    /// and the tariff's parse warnings stay out of the run's log.
    #[test]
    fn rejected_set_tariff_leaves_no_tariff_and_no_parse_warnings() {
        let (mut dwelling, _output_dir) = dwelling_whose_roster_plans_drift();
        let warnings_before = dwelling.warnings.to_vec();
        let actors_before = actor_names(&dwelling);

        let tariff = ElectricTariff {
            parse_warnings: vec!["test tariff parse warning".to_string()],
            ..ElectricTariff::default()
        };
        dwelling
            .set_tariff(tariff, chrono_tz::Tz::UTC)
            .expect_err("a drifting roster plan rejects the tariff");

        assert!(
            dwelling.tariff_evaluator().is_none(),
            "a rejected set_tariff must not leave the tariff attached"
        );
        assert_eq!(dwelling.warnings.to_vec(), warnings_before);
        assert_eq!(actor_names(&dwelling), actors_before);
    }

    fn warning_candidate(name: &str, env: &EnvironmentState) -> Box<dyn Equipment> {
        let mut candidate = TestWarningEquipment::new(name);
        candidate
            .init(&EquipmentConfig::default(), env)
            .expect("init warning test equipment");
        Box::new(candidate)
    }

    /// A rejected candidate never joined the dwelling: the warnings its
    /// own `init` raised come back in the error, and the run's warning log
    /// is unchanged.
    #[test]
    fn rejected_add_returns_the_candidate_warnings_and_leaves_the_log_unchanged() {
        let mut dwelling = bestest_dwelling();
        let existing = dwelling
            .equipment()
            .first()
            .expect("bestest assembles equipment")
            .descriptor()
            .name
            .clone();
        let warnings_before = dwelling.warnings.to_vec();

        let err = dwelling
            .add_equipment(warning_candidate(&existing, &dwelling.latest_env))
            .expect_err("a duplicate name is rejected");

        assert_eq!(dwelling.warnings.to_vec(), warnings_before);
        assert!(
            err.to_string().contains("test equipment init warning"),
            "the rejection carries the candidate's init warning: {err}"
        );
    }

    #[test]
    fn rejected_replace_returns_the_candidate_warnings_and_leaves_the_log_unchanged() {
        let mut dwelling = bestest_dwelling();
        let warnings_before = dwelling.warnings.to_vec();

        let err = match dwelling.replace_equipment(
            "No Such Equipment",
            warning_candidate("Replacement", &dwelling.latest_env),
        ) {
            Err(err) => err,
            Ok(_) => panic!("replacing a missing equipment is rejected"),
        };

        assert_eq!(dwelling.warnings.to_vec(), warnings_before);
        assert!(
            err.to_string().contains("test equipment init warning"),
            "the rejection carries the candidate's init warning: {err}"
        );
    }

    fn battery_seed() -> ActorSeed {
        ActorSeed::Battery {
            bms_mode: BmsMode::SelfConsumption {
                min_soc: 0.15,
                max_soc: 0.95,
                solar_only_charging: false,
                surplus_deadband_kw: 0.0,
            },
            grid_export_rule: hares_types::GridExportRule::Unrestricted,
            max_charge_kw: 5.0,
            max_discharge_kw: 5.0,
            min_dwell_steps: 0,
        }
    }

    fn ev_seed() -> ActorSeed {
        ActorSeed::Ev {
            strategy: ChargingStrategy::Immediate { target_soc: 1.0 },
            plug_in_policy: hares_types::PlugInPolicy::Always,
            capacity_kwh: 60.0,
            max_charge_kw: 7.6,
            fuel_economy_kwh_per_mi: 0.3,
        }
    }

    struct TargetingStubActor {
        name: String,
        target: String,
    }
    impl crate::Actor for TargetingStubActor {
        fn name(&self) -> &str {
            &self.name
        }
        fn dispatch_target_name(&self) -> Option<&str> {
            Some(&self.target)
        }
        fn decide(
            &mut self,
            _env: &hares_types::EnvironmentState,
            _out: &mut Vec<hares_control::DispatchRequest>,
        ) {
        }
    }

    /// Equipment whose actor seed builds a built-in actor gets that actor
    /// whenever it joins the dwelling, so removing it and adding it back
    /// restores the actor the removal evicted.
    #[test]
    fn re_added_equipment_gets_its_built_in_actor_back() {
        let mut dwelling = bestest_dwelling();
        dwelling
            .add_equipment(Box::new(SeedableTestEquipment::new("EV1", Some(ev_seed()))))
            .expect("add the EV");
        assert!(actor_names(&dwelling).contains(&"EvDriver:EV1".to_string()));

        let ev = dwelling.remove_equipment("EV1").expect("remove the EV");
        assert!(!actor_names(&dwelling).contains(&"EvDriver:EV1".to_string()));

        dwelling.add_equipment(ev).expect("re-add the EV");
        assert_eq!(
            actor_names(&dwelling)
                .iter()
                .filter(|n| n.as_str() == "EvDriver:EV1")
                .count(),
            1,
            "the re-added EV is driven by exactly one driver: {:?}",
            actor_names(&dwelling)
        );
    }

    /// Replacing equipment under a different name leaves no actor bound to
    /// the old name: its built-in actor and the user actors targeting it
    /// are evicted, and the replacement gets its own built-in actor.
    #[test]
    fn replace_under_a_new_name_drops_actors_bound_to_the_old_name() {
        let mut dwelling = bestest_dwelling();
        dwelling
            .add_equipment(Box::new(SeedableTestEquipment::new(
                "Battery1",
                Some(battery_seed()),
            )))
            .expect("add the battery");
        dwelling
            .auto_register_actors()
            .expect("register the battery's built-in actor");
        dwelling
            .add_actor(Box::new(TargetingStubActor {
                name: "Scheduler".to_string(),
                target: "Battery1".to_string(),
            }))
            .expect("add a user actor targeting the battery");

        if let Err(err) = dwelling.replace_equipment(
            "Battery1",
            Box::new(SeedableTestEquipment::new("Battery2", Some(battery_seed()))),
        ) {
            panic!("replace under a new name: {err}");
        }

        let names = actor_names(&dwelling);
        assert!(
            !names.contains(&"BatteryManagementActor:Battery1".to_string())
                && !names.contains(&"Scheduler".to_string()),
            "no actor stays bound to the replaced name: {names:?}"
        );
        assert!(
            names.contains(&"BatteryManagementActor:Battery2".to_string()),
            "the replacement gets its own built-in actor: {names:?}"
        );
    }

    /// Removing equipment drops its entry from the environment snapshot at
    /// once, so nothing reads the removed equipment's output afterwards.
    #[test]
    fn removed_equipment_leaves_the_environment_snapshot_at_removal() {
        let mut dwelling = bestest_dwelling();
        dwelling
            .add_equipment(Box::new(TestEquipment::new(
                "Removed",
                ControlCapabilities::empty(),
            )))
            .expect("add");
        let id = dwelling.equipment_id_by_name["Removed"];
        assert!(dwelling.latest_env().equipment_core.contains_key(&id));

        dwelling.remove_equipment("Removed").expect("remove");

        assert!(
            !dwelling.latest_env().equipment_core.contains_key(&id),
            "the removed equipment's core output must leave the snapshot at removal"
        );
    }

    /// A real EV with the default charging strategy, initialised against
    /// `env`: its actor seed builds an `EvDriver:<name>` that rolls driving
    /// days from its RNG stream.
    fn driven_ev(name: &str, env: &EnvironmentState) -> Box<dyn Equipment> {
        let config: hares_equipment::EvConfig = serde_json::from_value(serde_json::json!({
            "capacity_kwh": 60.0,
            "charging_level": "L2",
            "max_charging_power_kw": 7.2,
        }))
        .expect("minimal EvConfig");
        let config = EquipmentConfig::from_typed(name.to_string(), "EV".to_string(), config)
            .expect("typed EV config");
        let mut ev = EquipmentRegistry::new()
            .create("EV", config.clone())
            .expect("create EV");
        ev.init(&config, env).expect("init EV");
        ev
    }

    fn add_driven_ev(dwelling: &mut Dwelling, name: &str) {
        let ev = driven_ev(name, &dwelling.latest_env);
        dwelling
            .add_equipment(ev)
            .unwrap_or_else(|err| panic!("add {name}: {err}"));
    }

    fn driver_stream(dwelling: &Dwelling, ev: &str) -> u64 {
        let name = format!("EvDriver:{ev}");
        dwelling
            .actors
            .iter()
            .find(|a| a.name() == name)
            .and_then(|a| a.rng_pair())
            .unwrap_or_else(|| panic!("{name} is registered and owns an RNG stream"))
            .1
    }

    fn net_power_kw(dwelling: &mut Dwelling, steps: usize) -> Vec<f64> {
        (0..steps)
            .map(|_| dwelling.step().expect("step").net_electric_power_kw)
            .collect()
    }

    /// The billing summaries as a bit-level digest: a rejected set_tariff
    /// must leave them, the outgoing period's close included, bitwise
    /// unchanged.
    fn billing_summary_digest(dwelling: &Dwelling) -> String {
        use std::fmt::Write as _;
        let mut digest = String::new();
        for summary in dwelling.billing_summaries() {
            let _ = write!(
                digest,
                "{}..{}|{:x}|{:x}|{:x}|{:x}|{:x}|{:x}|{:x}|{:x};",
                summary.period_start,
                summary.period_end,
                summary.energy_charge_usd.to_bits(),
                summary.demand_charge_usd.to_bits(),
                summary.fixed_charge_usd.to_bits(),
                summary.export_credit_usd.to_bits(),
                summary.net_bill_usd.to_bits(),
                summary.peak_demand_kw.to_bits(),
                summary.total_import_kwh.to_bits(),
                summary.total_export_kwh.to_bits(),
            );
        }
        digest
    }

    /// Everything a rejected roster change must leave as it was.
    fn roster_fingerprint(dwelling: &Dwelling) -> String {
        let equipment: Vec<(String, EquipmentId)> = dwelling
            .equipment()
            .iter()
            .map(|e| (e.descriptor().name.clone(), e.descriptor().id))
            .collect();
        let mut core_ids: Vec<EquipmentId> =
            dwelling.latest_env.equipment_core.keys().copied().collect();
        core_ids.sort();
        let mut telemetry_names: Vec<&String> =
            dwelling.latest_env.equipment_telemetry.keys().collect();
        telemetry_names.sort();
        let mut auto_names: Vec<&String> = dwelling.auto_registered_actor_names.iter().collect();
        auto_names.sort();
        let mut column_index: Vec<(&String, &usize)> =
            dwelling.roster.output_column_index.iter().collect();
        column_index.sort();
        format!(
            "{equipment:?}|{:?}|{auto_names:?}|{:?}|{}|{}|{core_ids:?}|{telemetry_names:?}|\
             {:?}|{column_index:?}|{}|{}|{:?}|{:?}|{:?}|{:?}|{:?}|{}|{}",
            actor_names(dwelling),
            dwelling.scheduler.plan(),
            dwelling.next_equipment_id,
            dwelling.next_ev_driver_stream,
            dwelling.warnings.to_vec(),
            dwelling.roster.output_value_count,
            dwelling.roster.record_scratch.len(),
            dwelling.roster.equipment_execution_order,
            dwelling.roster.equipment_ids,
            dwelling.ports,
            dwelling.rollback_ports,
            dwelling
                .recorder
                .as_ref()
                .map(|r| (r.total_rows(), r.schema().fields().len())),
            dwelling.tariff_evaluator.is_some(),
            billing_summary_digest(dwelling),
        )
    }

    /// A pre-step dwelling with output enabled whose output file cannot be
    /// created, so every roster plan fails at its last step.
    fn dwelling_whose_recorder_cannot_be_created() -> (Dwelling, tempfile::TempDir) {
        let mut dwelling = bestest_dwelling();
        let tmp = tempfile::tempdir().expect("tempdir");
        dwelling.write_output = true;
        dwelling.output_path = tmp.path().join("missing").join("out.csv");
        (dwelling, tmp)
    }

    /// Every roster entrance rejects a change whose plan fails, and leaves
    /// the dwelling as it was: rosters, ids, schedule, caches, environment
    /// snapshot, ports, recorder, tariff and warning log.
    #[test]
    fn every_roster_entrance_leaves_the_dwelling_unchanged_when_its_plan_fails() {
        let (mut dwelling, _tmp) = dwelling_whose_recorder_cannot_be_created();
        let existing = dwelling
            .equipment()
            .first()
            .expect("bestest assembles equipment")
            .descriptor()
            .clone();
        let before = roster_fingerprint(&dwelling);
        let mut attempts: Vec<(&str, Result<()>)> = vec![
            (
                "remove_equipment",
                dwelling.remove_equipment(&existing.name).map(drop),
            ),
            (
                "remove_equipment_by_end_use",
                dwelling
                    .remove_equipment_by_end_use(std::slice::from_ref(&existing.end_use))
                    .map(drop),
            ),
            ("clear_equipment", dwelling.clear_equipment()),
            ("auto_register_actors", dwelling.auto_register_actors()),
            (
                "add_actor",
                dwelling.add_actor(Box::new(StubActor {
                    name: "Observer".to_string(),
                })),
            ),
            (
                "set_tariff",
                dwelling.set_tariff(ElectricTariff::default(), chrono_tz::Tz::UTC),
            ),
        ];
        let added = TestEquipment::new("Added", ControlCapabilities::empty());
        attempts.push(("add_equipment", dwelling.add_equipment(Box::new(added))));
        let replacement = TestEquipment::new("Replacement", ControlCapabilities::empty());
        attempts.push((
            "replace_equipment",
            dwelling
                .replace_equipment(&existing.name, Box::new(replacement))
                .map(drop),
        ));
        for (entrance, result) in attempts {
            assert!(result.is_err(), "{entrance} must be rejected");
        }
        assert_eq!(roster_fingerprint(&dwelling), before);
    }

    /// A set_tariff rejected at the roster plan leaves the billing
    /// summaries bitwise unchanged: no outgoing close, no accrued-charge
    /// loss, nothing appended.
    #[test]
    fn a_rejected_set_tariff_leaves_the_billing_summaries_unchanged() {
        let tz = chrono_tz::America::Denver;
        let mut dwelling = bestest_dwelling();
        dwelling
            .set_tariff(flat_tariff(), tz)
            .expect("attach the tariff");
        // Past the January close (step 744): one summary exists, and the
        // open February period holds accruals.
        net_power_kw(&mut dwelling, 800);
        assert!(
            !dwelling.billing_summaries().is_empty(),
            "the January close produced a summary"
        );

        // The next roster plan fails at the recorder: the summary digest
        // before and after the rejected set_tariff must match on bits.
        let tmp = tempfile::tempdir().expect("tempdir");
        dwelling.write_output = true;
        dwelling.output_path = tmp.path().join("missing").join("out.csv");
        let fingerprint_before = roster_fingerprint(&dwelling);
        let digest_before = billing_summary_digest(&dwelling);

        let err = dwelling
            .set_tariff(flat_tariff(), tz)
            .expect_err("the recorder plan fails");

        assert!(err.to_string().contains("recorder"), "got: {err}");
        assert_eq!(roster_fingerprint(&dwelling), fingerprint_before);
        assert_eq!(billing_summary_digest(&dwelling), digest_before);
    }

    /// Two EVs added after assembly drive on distinct RNG streams: the
    /// stream cursor advances with each driver built.
    #[test]
    fn ev_drivers_added_after_assembly_take_distinct_streams() {
        let mut dwelling = bestest_dwelling();
        add_driven_ev(&mut dwelling, "EV1");
        add_driven_ev(&mut dwelling, "EV2");
        assert_ne!(
            driver_stream(&dwelling, "EV1"),
            driver_stream(&dwelling, "EV2")
        );
    }

    /// A dwelling resumed from a checkpoint continues the stream cursor of
    /// the run that wrote it: after EV1 is removed and added back (its
    /// driver taking a second stream), an EV added after the resume gets
    /// the stream and the trajectory of the continuous run, never the
    /// restored EV1 driver's stream.
    #[test]
    fn a_resumed_dwelling_gives_a_new_ev_driver_the_stream_of_the_continuous_run() {
        let mut continuous = bestest_dwelling();
        add_driven_ev(&mut continuous, "EV1");
        let ev1 = continuous.remove_equipment("EV1").expect("remove EV1");
        continuous.add_equipment(ev1).expect("add EV1 back");
        net_power_kw(&mut continuous, 4);
        let checkpoint = continuous.save_checkpoint().expect("checkpoint");

        let mut resumed = bestest_dwelling();
        add_driven_ev(&mut resumed, "EV1");
        resumed.load_checkpoint(checkpoint).expect("restore");

        add_driven_ev(&mut continuous, "EV2");
        add_driven_ev(&mut resumed, "EV2");
        assert_eq!(
            driver_stream(&resumed, "EV2"),
            driver_stream(&continuous, "EV2")
        );
        assert_ne!(
            driver_stream(&resumed, "EV2"),
            driver_stream(&resumed, "EV1")
        );
        let resumed_power = net_power_kw(&mut resumed, 96);
        let continuous_power = net_power_kw(&mut continuous, 96);
        let first_difference = resumed_power
            .iter()
            .zip(&continuous_power)
            .position(|(r, c)| r != c);
        assert_eq!(
            first_difference, None,
            "resumed {resumed_power:?}\ncontinuous {continuous_power:?}"
        );
    }

    /// Between steps a restored dwelling reports the time and weather of
    /// its last completed step, as the run that wrote the checkpoint does,
    /// so equipment initialised against it after the resume starts alike.
    #[test]
    fn a_restored_dwelling_reports_the_environment_of_its_last_step() {
        let mut continuous = bestest_dwelling();
        net_power_kw(&mut continuous, 4);
        let checkpoint = continuous.save_checkpoint().expect("checkpoint");

        let mut resumed = bestest_dwelling();
        resumed.load_checkpoint(checkpoint).expect("restore");

        assert_eq!(
            resumed.latest_env.current_time,
            continuous.latest_env.current_time
        );
        assert_eq!(
            format!("{:?}", resumed.latest_env.weather),
            format!("{:?}", continuous.latest_env.weather)
        );
    }

    /// Built-in EV drivers draw from a bounded range of RNG streams below
    /// the event loads' streams; a dwelling that has used them all rejects
    /// the next EV instead of aliasing another stream.
    #[test]
    fn exhausted_ev_driver_streams_reject_the_next_ev() {
        let mut dwelling = bestest_dwelling();
        dwelling.next_ev_driver_stream = crate::rng::EV_DRIVER_STREAM_COUNT;
        let before = roster_fingerprint(&dwelling);

        let ev = driven_ev("EV1", &dwelling.latest_env);
        let err = dwelling
            .add_equipment(ev)
            .expect_err("no EV driver stream is left");

        assert!(err.to_string().contains("RNG stream"), "got: {err}");
        assert_eq!(roster_fingerprint(&dwelling), before);
    }

    /// Attaching a tariff mid-run rebuilds the built-in actors with its
    /// prices but keeps each existing EV driver's state, RNG position
    /// included: a flat tariff leaves the default-strategy EV's trajectory
    /// exactly as it is without one.
    #[test]
    fn a_mid_run_tariff_keeps_existing_ev_driver_state() {
        let mut with_tariff = bestest_dwelling();
        let mut without = bestest_dwelling();
        for dwelling in [&mut with_tariff, &mut without] {
            add_driven_ev(dwelling, "EV1");
        }
        // Step until the driver is charging, so its telemetry differs from
        // a freshly built driver's.
        let charging = |dwelling: &Dwelling| {
            actor_telemetry(dwelling, "EvDriver:EV1")
                .iter()
                .any(|(key, bits)| key == "charge_kw" && f64::from_bits(*bits) > 0.0)
        };
        for _ in 0..200 {
            if charging(&with_tariff) {
                break;
            }
            net_power_kw(&mut with_tariff, 1);
            net_power_kw(&mut without, 1);
        }
        assert!(
            charging(&with_tariff),
            "the driver charges within 200 steps"
        );
        let stream = driver_stream(&with_tariff, "EV1");
        let telemetry = actor_telemetry(&with_tariff, "EvDriver:EV1");
        with_tariff
            .set_tariff(ElectricTariff::default(), chrono_tz::Tz::UTC)
            .expect("attach a tariff");

        assert_eq!(driver_stream(&with_tariff, "EV1"), stream);
        assert_eq!(
            actor_telemetry(&with_tariff, "EvDriver:EV1"),
            telemetry,
            "between the rebuild and the next step the driver reports its carried state"
        );
        assert_eq!(
            net_power_kw(&mut with_tariff, 96),
            net_power_kw(&mut without, 96)
        );
    }

    /// An actor's telemetry as sorted `(key, bits)` pairs, for bitwise
    /// comparison.
    fn actor_telemetry(dwelling: &Dwelling, actor: &str) -> Vec<(String, u64)> {
        let telemetry = dwelling
            .actors
            .iter()
            .find(|a| a.name() == actor)
            .and_then(|a| a.telemetry())
            .unwrap_or_else(|| panic!("{actor} is registered and publishes telemetry"));
        let mut entries: Vec<(String, u64)> = telemetry
            .0
            .iter()
            .map(|(key, value)| (key.clone(), value.to_bits()))
            .collect();
        entries.sort();
        entries
    }

    const PGE_E_TOU_C: &str = include_str!("../../../../tests/fixtures/urdb/pge_e_tou_c.json");

    /// A battery whose management actor optimises against time-of-use
    /// prices, with a three-step minimum dwell.
    fn tou_battery(env: &EnvironmentState) -> Box<dyn Equipment> {
        let bms_mode = serde_json::json!({ "TimeOfUseOptimization": {
            "reserve_soc": 0.2,
            "charge_threshold_percentile": 0.3,
            "discharge_threshold_percentile": 0.7,
            "solar_only_charging": false,
            "min_duration_steps": 3,
        }});
        let config: hares_equipment::BatteryConfig = serde_json::from_value(serde_json::json!({
            "capacity_kwh": 13.5,
            "max_charge_kw": 5.0,
            "max_discharge_kw": 5.0,
            "bms_mode": bms_mode.to_string(),
            "min_dwell_steps": 3,
        }))
        .expect("BatteryConfig");
        let config = EquipmentConfig::from_typed("Bat".to_string(), "Battery".to_string(), config)
            .expect("typed battery config");
        let mut battery = EquipmentRegistry::new()
            .create("Battery", config.clone())
            .expect("create battery");
        battery.init(&config, env).expect("init battery");
        battery
    }

    /// Re-attaching the tariff a dwelling already has rebuilds its battery
    /// management actor from the same prices with its decision state
    /// carried over, and the new tariff prices from the current step, so
    /// nothing changes: the actor's telemetry right after the rebuild and
    /// every later step are bitwise those of the dwelling that kept its
    /// actor and tariff.
    #[test]
    fn reattaching_the_same_tariff_mid_run_changes_nothing() {
        let tariff = || hares_tariff::parse_urdb(PGE_E_TOU_C).expect("parse the URDB tariff");
        let tz = chrono_tz::America::Los_Angeles;
        let mut continuous = bestest_dwelling();
        let mut reattached = bestest_dwelling();
        for dwelling in [&mut continuous, &mut reattached] {
            dwelling
                .set_tariff(tariff(), tz)
                .expect("attach the tariff");
            let battery = tou_battery(&dwelling.latest_env);
            dwelling.add_equipment(battery).expect("add the battery");
            net_power_kw(dwelling, 40);
        }

        reattached
            .set_tariff(tariff(), tz)
            .expect("attach the same tariff again");

        let bms = "BatteryManagementActor:Bat";
        assert_eq!(
            actor_telemetry(&reattached, bms),
            actor_telemetry(&continuous, bms)
        );
        assert_eq!(
            net_power_kw(&mut reattached, 96),
            net_power_kw(&mut continuous, 96)
        );
        assert_eq!(
            actor_telemetry(&reattached, bms),
            actor_telemetry(&continuous, bms)
        );
    }

    const FLAT_TARIFF_JSON: &str =
        include_str!("../../../../tests/fixtures/golden/flat_tariff.json");

    /// The golden fixture's flat tariff: one all-hours period at 0.15
    /// USD/kWh, a 10.00 USD monthly fixed charge, monthly billing.
    fn flat_tariff() -> ElectricTariff {
        serde_json::from_str(FLAT_TARIFF_JSON).expect("parse the flat tariff fixture")
    }

    /// A TOU tariff whose demand charges make the demand-window and
    /// ratchet state load-bearing: a coincident rate and a peak-period
    /// rate, both with a two-month ratchet, over a 30 minute demand
    /// window.
    fn tou_demand_tariff() -> ElectricTariff {
        use hares_tariff::types::{DemandRate, EnergyRate, FixedCharges, RatchetConfig};
        use hares_types::{DayFilter, SeasonFilter, TimeWindow, TouPeriod};

        let ratchet = RatchetConfig {
            lookback_months: 2,
            minimum_fraction: 0.6,
        };
        ElectricTariff {
            name: Some("tou-demand-ratchet".into()),
            tou_schedule: vec![
                TouPeriod {
                    name: "peak".into(),
                    schedule: vec![TimeWindow::new(DayFilter::Any, 960, 1260, 0.0)],
                    season: SeasonFilter::All,
                },
                TouPeriod {
                    name: "off-peak".into(),
                    schedule: vec![TimeWindow::new(DayFilter::Any, 0, 1440, 0.0)],
                    season: SeasonFilter::All,
                },
            ],
            energy_rates: vec![
                EnergyRate {
                    period_name: "peak".into(),
                    season: SeasonFilter::All,
                    rate_per_kwh: 0.25,
                },
                EnergyRate {
                    period_name: "off-peak".into(),
                    season: SeasonFilter::All,
                    rate_per_kwh: 0.08,
                },
            ],
            demand_rates: vec![
                DemandRate {
                    period_name: None,
                    season: SeasonFilter::All,
                    rate_per_kw: 5.0,
                    ratchet: Some(ratchet.clone()),
                },
                DemandRate {
                    period_name: Some("peak".into()),
                    season: SeasonFilter::All,
                    rate_per_kw: 12.0,
                    ratchet: Some(ratchet),
                },
            ],
            fixed_charges: FixedCharges {
                monthly_usd: 10.0,
                daily_usd: 0.0,
            },
            demand_window_minutes: 30,
            ..Default::default()
        }
    }

    /// The synthetic dwelling whose TOML the 900 s pins step: 5800 steps
    /// at 900 s cover January and February 2024, so the January close and
    /// the February close both fall inside the run. A stochastic event
    /// load (3 kW spikes at a quarter of steps) makes the demand window's
    /// averages non-stationary, so the window state is load-bearing.
    fn dwelling_at_900s() -> Dwelling {
        const STEPS: usize = 5800;
        let toml = format!(
            r#"building_id = 2005

[simulation]
start_time = "2024-01-01T00:00:00Z"
time_res_s = 900
duration_s = {duration}

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.8

[hvac]
equipment_name = "Furnace"
fuel = "electricity"
heating_capacity_kbtu_h = 30.0

[weather]
outdoor_temp_c = 10.0
dew_point_c = 5.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325

[schedule]
occupancy = 1.0

[event_load]
active_power_kw = 3.0
active_duration_s = 900.0
cooldown_duration_s = 900.0
event_probability = 0.25
sensible_gain_fraction = 0.72
latent_gain_fraction = 0.08

[output]
write_output = false
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
master_seed = 42
"#,
            duration = STEPS * 900,
        );
        let tmp = tempfile::tempdir().expect("tempdir");
        let toml_path = tmp.path().join("dwelling_900s.toml");
        fs::write(&toml_path, toml).expect("write the 900 s TOML");
        let dwelling = Dwelling::from_toml_config_with_write_output(&toml_path, Some(false))
            .expect("build the 900 s dwelling");
        drop(tmp);
        dwelling
    }

    /// Every field of two billing summaries compared on bits.
    fn assert_billing_bitwise_equal(a: &BillingPeriodSummary, b: &BillingPeriodSummary) {
        assert_eq!(a.period_start, b.period_start, "period_start");
        assert_eq!(a.period_end, b.period_end, "period_end");
        assert_eq!(
            a.energy_charge_usd.to_bits(),
            b.energy_charge_usd.to_bits(),
            "energy_charge_usd"
        );
        assert_eq!(
            a.demand_charge_usd.to_bits(),
            b.demand_charge_usd.to_bits(),
            "demand_charge_usd"
        );
        assert_eq!(
            a.fixed_charge_usd.to_bits(),
            b.fixed_charge_usd.to_bits(),
            "fixed_charge_usd"
        );
        assert_eq!(
            a.export_credit_usd.to_bits(),
            b.export_credit_usd.to_bits(),
            "export_credit_usd"
        );
        assert_eq!(
            a.net_bill_usd.to_bits(),
            b.net_bill_usd.to_bits(),
            "net_bill_usd"
        );
        assert_eq!(
            a.peak_demand_kw.to_bits(),
            b.peak_demand_kw.to_bits(),
            "peak_demand_kw"
        );
        assert_eq!(
            a.total_import_kwh.to_bits(),
            b.total_import_kwh.to_bits(),
            "total_import_kwh"
        );
        assert_eq!(
            a.total_export_kwh.to_bits(),
            b.total_export_kwh.to_bits(),
            "total_export_kwh"
        );
    }

    /// The summary whose period starts at `start`, or `None`.
    fn summary_starting_at(
        summaries: &[BillingPeriodSummary],
        start: chrono::DateTime<chrono_tz::Tz>,
    ) -> Option<&BillingPeriodSummary> {
        summaries.iter().find(|s| s.period_start == start)
    }

    /// A tariff attached after step 0 bills from the period containing the
    /// attach step: no bill exists before the attach, the containing
    /// period's fixed charge is prorated to the span from the attach step
    /// to the period end, and every later full period is bitwise the
    /// from-start run's.
    #[test]
    fn a_tariff_attached_mid_run_bills_from_the_attach_period() {
        let tz = chrono_tz::America::Denver;
        let mut from_start = bestest_dwelling();
        from_start
            .set_tariff(flat_tariff(), tz)
            .expect("attach the tariff from the start");
        let mut attached_mid_run = bestest_dwelling();

        // Step 300 is Jan 13 12:00, inside the January period.
        const ATTACH_STEP: usize = 300;
        net_power_kw(&mut attached_mid_run, ATTACH_STEP);
        assert!(
            attached_mid_run.billing_summaries().is_empty(),
            "no bill exists before the attach"
        );

        attached_mid_run
            .set_tariff(flat_tariff(), tz)
            .expect("attach the tariff mid-run");

        // Run both dwellings past the February and March closes (the first
        // step whose start reaches a period end closes it).
        const TOTAL_STEPS: usize = 2300;
        net_power_kw(&mut attached_mid_run, TOTAL_STEPS - ATTACH_STEP);
        net_power_kw(&mut from_start, TOTAL_STEPS);

        let from_start_bills = from_start.billing_summaries();
        let mid_run_bills = attached_mid_run.billing_summaries();
        assert_eq!(
            mid_run_bills.len(),
            from_start_bills.len(),
            "both runs close the same periods"
        );

        let january_start = attached_mid_run.clock.start_time.with_timezone(&tz);
        let containing = summary_starting_at(mid_run_bills, january_start)
            .expect("the first bill is the period containing the attach step");
        let from_start_january = summary_starting_at(from_start_bills, january_start)
            .expect("the from-start run closed January");
        assert_eq!(containing.period_start, from_start_january.period_start);
        assert_eq!(containing.period_end, from_start_january.period_end);
        assert!(
            containing.fixed_charge_usd < from_start_january.fixed_charge_usd,
            "the first period's fixed charge is prorated: {} < {}",
            containing.fixed_charge_usd,
            from_start_january.fixed_charge_usd
        );
        // The proration: 18.5 of January's 31 days active.
        let attach_time = january_start + chrono::Duration::hours(ATTACH_STEP as i64);
        let fraction = (containing.period_end - attach_time).num_seconds() as f64
            / (containing.period_end - containing.period_start).num_seconds() as f64;
        assert_eq!(
            containing.fixed_charge_usd.to_bits(),
            (10.0_f64 * fraction).to_bits(),
            "the fixed charge is the full charge by the active fraction"
        );
        assert!(
            containing.total_import_kwh < from_start_january.total_import_kwh,
            "only the steps from the attach on accrue to the containing period"
        );

        // Every later period is bitwise the from-start run's.
        for bill in &mid_run_bills[1..] {
            let matching = summary_starting_at(from_start_bills, bill.period_start)
                .expect("each later period closed in the from-start run too");
            assert_billing_bitwise_equal(bill, matching);
        }
    }

    /// A tariff attached exactly at a period boundary opens the new
    /// period, and the boundary step's energy books to it; a from-start
    /// run folds that step into the old period (its close fires on the
    /// fold whose time reaches the boundary, after the fold books). One
    /// step's attribution differs by design; every kWh is billed exactly
    /// once.
    #[test]
    fn a_tariff_attached_at_a_period_boundary_books_the_boundary_step_to_the_new_period() {
        let tz = chrono_tz::America::Denver;
        let mut from_start = bestest_dwelling();
        from_start
            .set_tariff(flat_tariff(), tz)
            .expect("attach the tariff from the start");

        // Hourly steps: the boundary is Feb 1 00:00, the start of step
        // 744, so the 745th fold is the boundary step.
        const BOUNDARY_STEP: usize = 744;
        let mut powers = net_power_kw(&mut from_start, BOUNDARY_STEP + 1);
        let boundary_step_kwh = powers.pop().expect("the boundary step's power").max(0.0);

        let mut at_boundary = bestest_dwelling();
        net_power_kw(&mut at_boundary, BOUNDARY_STEP);
        at_boundary
            .set_tariff(flat_tariff(), tz)
            .expect("attach exactly at the boundary");

        // Past the February close (step 1416) on both runs.
        net_power_kw(&mut from_start, 1500 - BOUNDARY_STEP - 1);
        net_power_kw(&mut at_boundary, 1500 - BOUNDARY_STEP);

        let from_start_bills = from_start.billing_summaries();
        let at_boundary_bills = at_boundary.billing_summaries();

        // The attached run has no January bill: the attach opened
        // February.
        let january_start = at_boundary.clock.start_time.with_timezone(&tz);
        assert!(
            summary_starting_at(at_boundary_bills, january_start).is_none(),
            "an attach at the boundary opens the new period: no January bill exists"
        );
        let february_start = outgoing_end(january_start);
        let attached_february = summary_starting_at(at_boundary_bills, february_start)
            .expect("the attached run closes February");
        let from_start_february = summary_starting_at(from_start_bills, february_start)
            .expect("the from-start run closes February");

        // The boundary step's energy books to the attached run's February,
        // not to the from-start run's.
        assert!(
            (attached_february.total_import_kwh
                - from_start_february.total_import_kwh
                - boundary_step_kwh)
                .abs()
                < 1e-9,
            "the boundary step's energy books to the new period: {} - {} must be {}",
            attached_february.total_import_kwh,
            from_start_february.total_import_kwh,
            boundary_step_kwh
        );

        // February is a period opened at its own boundary on both runs: no
        // proration at a boundary attach, the fixed charges bitwise equal.
        assert_eq!(
            attached_february.fixed_charge_usd.to_bits(),
            from_start_february.fixed_charge_usd.to_bits(),
            "a boundary attach bills the new period in full"
        );
    }

    /// Replacing a tariff closes the outgoing open period with its
    /// accruals: the bill exists, covering the span the outgoing tariff
    /// served, and the replacement opens the period containing the switch
    /// step with its charges prorated from it.
    #[test]
    fn replacing_a_tariff_closes_the_outgoing_period() {
        let tz = chrono_tz::America::Denver;
        let mut replaced = bestest_dwelling();
        replaced
            .set_tariff(flat_tariff(), tz)
            .expect("attach the outgoing tariff");

        // The accruals the outgoing period holds at the switch, read from
        // a twin that is stepped the same and never replaced.
        let mut twin = bestest_dwelling();
        twin.set_tariff(flat_tariff(), tz)
            .expect("attach the outgoing tariff");
        const SWITCH_STEP: usize = 300;
        net_power_kw(&mut replaced, SWITCH_STEP);
        net_power_kw(&mut twin, SWITCH_STEP);
        let accruals = twin
            .tariff_evaluator()
            .expect("the twin carries the tariff")
            .current_metrics();

        let mut expensive = flat_tariff();
        expensive.energy_rates[0].rate_per_kwh = 0.20;
        expensive.fixed_charges.monthly_usd = 20.0;
        replaced
            .set_tariff(expensive, tz)
            .expect("replace the tariff");

        let bills = replaced.billing_summaries();
        assert_eq!(bills.len(), 1, "exactly the outgoing period's bill exists");
        let outgoing = &bills[0];
        let january_start = replaced.clock.start_time.with_timezone(&tz);
        let switch_time = january_start + chrono::Duration::hours(SWITCH_STEP as i64);
        assert_eq!(outgoing.period_start, accruals.period_start);
        assert_eq!(outgoing.period_end, switch_time);
        assert_eq!(
            outgoing.total_import_kwh.to_bits(),
            accruals.total_import_kwh.to_bits(),
            "the outgoing bill carries the accruals"
        );
        // The served span: 300 of January's 744 hours.
        let full_month_seconds = (outgoing_end(january_start) - january_start).num_seconds() as f64;
        let fraction = (switch_time - january_start).num_seconds() as f64 / full_month_seconds;
        assert_eq!(
            outgoing.fixed_charge_usd.to_bits(),
            (10.0_f64 * fraction).to_bits(),
            "the outgoing fixed charge is prorated to the served span"
        );

        // The replacement opens the period containing the switch step: its
        // first bill, at the February boundary, is prorated from the
        // switch, and it carries the replacement's accruals.
        net_power_kw(&mut replaced, 500);
        let bills = replaced.billing_summaries();
        assert_eq!(bills.len(), 2, "the replacement closed the February period");
        let replacement_bill = &bills[1];
        let switch_to_boundary =
            (replacement_bill.period_end - switch_time).num_seconds() as f64 / full_month_seconds;
        assert_eq!(
            replacement_bill.fixed_charge_usd.to_bits(),
            (20.0_f64 * switch_to_boundary).to_bits(),
            "the replacement's fixed charge is prorated from the switch step"
        );
        assert!(
            replacement_bill.total_import_kwh > 0.0,
            "the replacement's accruals carried into its first bill"
        );
    }

    /// The end of the billing period that starts at `start` under the
    /// fixture's monthly cycle: the anniversary day of the next month.
    fn outgoing_end(start: chrono::DateTime<chrono_tz::Tz>) -> chrono::DateTime<chrono_tz::Tz> {
        use chrono::{Datelike, TimeZone};
        let (year, month) = if start.month() == 12 {
            (start.year() + 1, 1)
        } else {
            (start.year(), start.month() + 1)
        };
        start
            .timezone()
            .with_ymd_and_hms(year, month, start.day(), 0, 0, 0)
            .earliest()
            .expect("the next anniversary is a valid instant")
    }

    /// A dwelling checkpointed mid-run with a tariff attached resumes so
    /// that every later step result and every later billing summary is
    /// bitwise the continuous run's.
    #[test]
    fn a_resumed_dwelling_with_a_tariff_equals_the_continuous_run() {
        let tz = chrono_tz::America::Denver;
        let mut continuous = bestest_dwelling();
        continuous
            .set_tariff(flat_tariff(), tz)
            .expect("attach the tariff");

        // Past the January close (the first step whose start reaches
        // February 1), in the middle of the February period.
        const CHECKPOINT_AT: usize = 800;
        net_power_kw(&mut continuous, CHECKPOINT_AT);
        let checkpoint = continuous.save_checkpoint().expect("checkpoint");
        assert!(
            checkpoint.tariff_state.is_some(),
            "the checkpoint carries the tariff evaluator's state"
        );

        let mut resumed = bestest_dwelling();
        resumed.load_checkpoint(checkpoint).expect("restore");
        assert!(
            resumed.tariff_evaluator().is_some(),
            "the restored dwelling carries the tariff"
        );

        // Continue both runs past the February close (step 1416).
        const TAIL: usize = 700;
        let continuous_tail: Vec<StepResult> = (0..TAIL)
            .map(|_| continuous.step().expect("step"))
            .collect();
        let resumed_tail: Vec<StepResult> =
            (0..TAIL).map(|_| resumed.step().expect("step")).collect();

        for (i, (a, b)) in continuous_tail.iter().zip(resumed_tail.iter()).enumerate() {
            assert_eq!(a.timestamp, b.timestamp, "step {i} timestamp");
            assert_eq!(
                a.net_electric_power_kw.to_bits(),
                b.net_electric_power_kw.to_bits(),
                "step {i} net power"
            );
            assert_eq!(
                a.hvac_heating_w.to_bits(),
                b.hvac_heating_w.to_bits(),
                "step {i} hvac heating"
            );
            assert_eq!(
                a.hvac_cooling_w.to_bits(),
                b.hvac_cooling_w.to_bits(),
                "step {i} hvac cooling"
            );
            assert_eq!(
                a.gas_power_w.to_bits(),
                b.gas_power_w.to_bits(),
                "step {i} gas"
            );
        }

        // The billing summaries from the checkpoint on are bitwise the
        // continuous run's; the resumed run lacks only the January bill the
        // continuous run closed before the checkpoint.
        let closed_before_checkpoint = 1;
        let continuous_bills = continuous.billing_summaries();
        let resumed_bills = resumed.billing_summaries();
        assert_eq!(
            resumed_bills.len(),
            continuous_bills.len() - closed_before_checkpoint,
            "the resumed run closes the same periods from the checkpoint on"
        );
        for (i, bill) in resumed_bills.iter().enumerate() {
            assert_billing_bitwise_equal(bill, &continuous_bills[closed_before_checkpoint + i]);
        }
    }

    /// A dwelling checkpointed mid-run over a demand/ratchet tariff
    /// resumes so its post-resume billing summaries are bitwise the
    /// continuous run's: a 30 minute demand window at 900 s steps (two
    /// samples, the ring head rotating), a TOU tariff with a coincident
    /// and a peak-period demand rate, a two-month ratchet whose prior
    /// peaks the January close populated, and the window's running sum,
    /// count and push cadence (a re-sum falls inside the resumed tail)
    /// all restore exactly. The checkpoint sits so the February period's
    /// unique window-average maximum falls inside the resumed span: a
    /// dropped running sum or a lost ratchet history moves the February
    /// summary off the continuous run's bits.
    #[test]
    fn a_resumed_dwelling_restores_the_demand_window_ratchet_and_tou_peaks() {
        let tz = chrono_tz::UTC;
        let mut continuous = dwelling_at_900s();
        continuous
            .set_tariff(tou_demand_tariff(), tz)
            .expect("attach the tariff");

        // The checkpoint sits after the January close (the step whose
        // start is Feb 1 00:00 UTC), with the open February period holding
        // accruals, the window's push count off a re-sum boundary, and the
        // ring head on the window's second slot (the push count is odd).
        const CHECKPOINT_AT: usize = 3051;
        net_power_kw(&mut continuous, CHECKPOINT_AT);
        assert_eq!(
            continuous.billing_summaries().len(),
            1,
            "January closed before the checkpoint"
        );
        let checkpoint = continuous.save_checkpoint().expect("checkpoint");
        assert!(
            checkpoint.tariff_state.is_some(),
            "the checkpoint carries the tariff evaluator's state"
        );

        let mut resumed = dwelling_at_900s();
        resumed.load_checkpoint(checkpoint).expect("restore");
        assert!(
            resumed.tariff_evaluator().is_some(),
            "the restored dwelling carries the tariff"
        );

        // The tail crosses the February close (the step whose start is
        // Mar 1 00:00 UTC) and the window's re-sum cadence (the push count
        // passes a multiple of 1000 well inside the tail).
        const TAIL: usize = 2749;
        for i in 0..TAIL {
            let continuous_step = continuous.step().expect("step");
            let resumed_step = resumed.step().expect("step");
            assert_eq!(
                continuous_step.net_electric_power_kw.to_bits(),
                resumed_step.net_electric_power_kw.to_bits(),
                "step {i} net power"
            );
        }

        // The February bill is bitwise the continuous run's: its demand
        // charges ratchet against January's restored peaks, built from the
        // restored window's averages.
        let continuous_bills = continuous.billing_summaries();
        let resumed_bills = resumed.billing_summaries();
        assert_eq!(
            resumed_bills.len(),
            continuous_bills.len() - 1,
            "the resumed run closes February, the continuous run January and February"
        );
        let continuous_february = continuous_bills.last().expect("February closed");
        let resumed_february = resumed_bills.last().expect("February closed");
        assert_billing_bitwise_equal(resumed_february, continuous_february);
        assert!(
            resumed_february.demand_charge_usd > 0.0,
            "the February demand charge is nonzero: the restored window and ratchet state are load-bearing"
        );
    }

    /// The switch step the replacement-across-a-resume pins share: Jan 13
    /// 12:00, inside the January period.
    const REPLACEMENT_SWITCH_STEP: usize = 300;

    /// The two runs the post-resume replacement pins compare: a continuous
    /// run that replaces its tariff at the switch step, and a run that
    /// checkpoints at the switch step, restores onto a fresh dwelling and
    /// replaces there. Returned after the replacement's outgoing bill has
    /// closed on both.
    fn replacement_across_a_resume() -> (Dwelling, Dwelling) {
        let tz = chrono_tz::America::Denver;
        let mut expensive = flat_tariff();
        expensive.energy_rates[0].rate_per_kwh = 0.20;
        expensive.fixed_charges.monthly_usd = 20.0;

        let mut continuous = bestest_dwelling();
        continuous
            .set_tariff(flat_tariff(), tz)
            .expect("attach the tariff");
        net_power_kw(&mut continuous, REPLACEMENT_SWITCH_STEP);
        continuous
            .set_tariff(expensive.clone(), tz)
            .expect("replace the tariff");

        let mut checkpointed = bestest_dwelling();
        checkpointed
            .set_tariff(flat_tariff(), tz)
            .expect("attach the tariff");
        net_power_kw(&mut checkpointed, REPLACEMENT_SWITCH_STEP);
        let checkpoint = checkpointed.save_checkpoint().expect("checkpoint");
        let mut resumed = bestest_dwelling();
        resumed.load_checkpoint(checkpoint).expect("restore");
        resumed
            .set_tariff(expensive, tz)
            .expect("replace the tariff after the resume");

        (continuous, resumed)
    }

    /// A fresh tariff attached to a restored dwelling bills exactly as the
    /// continuous analogue's replacement does: the fresh attach runs
    /// activate_at over the restored billing state, the outgoing period
    /// closes on the restored accruals, and every later bill is bitwise
    /// the continuous run's.
    #[test]
    fn a_tariff_attached_after_a_resume_bills_like_the_continuous_run() {
        let (mut continuous, mut resumed) = replacement_across_a_resume();

        // Past the February close (step 1416).
        net_power_kw(&mut continuous, 1200);
        net_power_kw(&mut resumed, 1200);

        let continuous_bills = continuous.billing_summaries();
        let resumed_bills = resumed.billing_summaries();
        assert_eq!(
            resumed_bills.len(),
            continuous_bills.len(),
            "both runs close the outgoing period and February"
        );
        for (a, b) in resumed_bills.iter().zip(continuous_bills.iter()) {
            assert_billing_bitwise_equal(a, b);
        }
    }

    /// A replacement across a resume closes the outgoing period with the
    /// accruals the restored evaluator held, and opens the replacement's
    /// period at the switch step: its first bill, at the February
    /// boundary, is prorated from the switch instant.
    #[test]
    fn a_replacement_across_a_resume_closes_the_outgoing_period_with_its_accruals() {
        let tz = chrono_tz::America::Denver;
        let (mut continuous, mut resumed) = replacement_across_a_resume();

        // The outgoing bill the replacement closed, on both runs.
        assert_eq!(
            continuous.billing_summaries().len(),
            1,
            "exactly the outgoing period's bill exists on the continuous run"
        );
        assert_eq!(
            resumed.billing_summaries().len(),
            1,
            "exactly the outgoing period's bill exists on the resumed run"
        );
        let continuous_outgoing = &continuous.billing_summaries()[0];
        let resumed_outgoing = &resumed.billing_summaries()[0];
        assert_billing_bitwise_equal(resumed_outgoing, continuous_outgoing);

        // The outgoing period closed at the switch step, carrying the
        // accruals.
        let january_start = resumed.clock.start_time.with_timezone(&tz);
        let switch_time = january_start + chrono::Duration::hours(REPLACEMENT_SWITCH_STEP as i64);
        assert_eq!(resumed_outgoing.period_end, switch_time);
        assert!(
            resumed_outgoing.total_import_kwh > 0.0,
            "the outgoing bill carries the accruals"
        );

        // The replacement's period opens at the switch step: its first
        // bill, at the February boundary (step 744), is prorated from the
        // switch, and the tail stops before the March close.
        net_power_kw(&mut continuous, 500);
        net_power_kw(&mut resumed, 500);
        let continuous_bills = continuous.billing_summaries();
        let resumed_bills = resumed.billing_summaries();
        assert_eq!(
            resumed_bills.len(),
            2,
            "the replacement closed the February period"
        );
        let replacement_bill = &resumed_bills[1];
        assert_eq!(
            replacement_bill.period_start, january_start,
            "the replacement opens the period containing the switch step"
        );
        let full_month_seconds = (outgoing_end(january_start) - january_start).num_seconds() as f64;
        let switch_to_boundary =
            (replacement_bill.period_end - switch_time).num_seconds() as f64 / full_month_seconds;
        assert_eq!(
            replacement_bill.fixed_charge_usd.to_bits(),
            (20.0_f64 * switch_to_boundary).to_bits(),
            "the replacement's fixed charge is prorated from the switch step"
        );
        assert_billing_bitwise_equal(replacement_bill, &continuous_bills[1]);
    }

    /// A checkpoint whose stream cursor is not past every restored built-in
    /// driver's stream is rejected: restoring it would let the next driver
    /// share a live driver's stream.
    #[test]
    fn a_checkpoint_cursor_behind_a_restored_driver_is_rejected() {
        let mut continuous = bestest_dwelling();
        add_driven_ev(&mut continuous, "EV1");
        let ev1 = continuous.remove_equipment("EV1").expect("remove EV1");
        continuous.add_equipment(ev1).expect("add EV1 back");
        net_power_kw(&mut continuous, 4);
        let mut checkpoint = continuous.save_checkpoint().expect("checkpoint");
        checkpoint.next_ev_driver_stream = 1;

        let mut resumed = bestest_dwelling();
        add_driven_ev(&mut resumed, "EV1");
        let err = resumed
            .load_checkpoint(checkpoint)
            .expect_err("a cursor behind EV1's restored stream is rejected");
        assert!(err.to_string().contains("stream"), "got: {err}");
    }

    /// An EV replaced in kind by an EV with other driving parameters gets
    /// a driver built for the replacement, as if it had been removed and
    /// the replacement added.
    #[test]
    fn replacing_an_ev_in_kind_rebuilds_its_driver_for_the_new_ev() {
        let ev_of = |capacity_kwh: f64, env: &EnvironmentState| -> Box<dyn Equipment> {
            let config: hares_equipment::EvConfig = serde_json::from_value(serde_json::json!({
                "capacity_kwh": capacity_kwh,
                "charging_level": "L2",
                "max_charging_power_kw": 7.2,
            }))
            .expect("EvConfig");
            let config = EquipmentConfig::from_typed("EV1".to_string(), "EV".to_string(), config)
                .expect("typed EV config");
            let mut ev = EquipmentRegistry::new()
                .create("EV", config.clone())
                .expect("create EV");
            ev.init(&config, env).expect("init EV");
            ev
        };
        let mut replaced = bestest_dwelling();
        let mut re_added = bestest_dwelling();
        for dwelling in [&mut replaced, &mut re_added] {
            let small = ev_of(30.0, &dwelling.latest_env);
            dwelling.add_equipment(small).expect("add the 30 kWh EV");
        }
        let large = ev_of(100.0, &replaced.latest_env);
        if let Err(err) = replaced.replace_equipment("EV1", large) {
            panic!("replace in kind: {err}");
        }
        re_added
            .remove_equipment("EV1")
            .expect("remove the 30 kWh EV");
        let large = ev_of(100.0, &re_added.latest_env);
        re_added.add_equipment(large).expect("add the 100 kWh EV");

        assert_eq!(
            driver_stream(&replaced, "EV1"),
            driver_stream(&re_added, "EV1")
        );
        assert_eq!(
            net_power_kw(&mut replaced, 48),
            net_power_kw(&mut re_added, 48)
        );
        assert_eq!(
            actor_telemetry(&replaced, "EvDriver:EV1"),
            actor_telemetry(&re_added, "EvDriver:EV1")
        );
    }

    /// An EV replaced mid-run by an identically configured EV keeps its
    /// driver and the driver's state: the actor seed compared on a
    /// replacement holds build parameters, not the pack's aged state.
    #[test]
    fn an_identical_ev_replaced_mid_run_keeps_its_driver() {
        let mut dwelling = bestest_dwelling();
        add_driven_ev(&mut dwelling, "EV1");
        net_power_kw(&mut dwelling, 30);
        let stream = driver_stream(&dwelling, "EV1");
        let telemetry = actor_telemetry(&dwelling, "EvDriver:EV1");

        let identical = driven_ev("EV1", &dwelling.latest_env);
        if let Err(err) = dwelling.replace_equipment("EV1", identical) {
            panic!("replace in kind: {err}");
        }

        assert_eq!(driver_stream(&dwelling, "EV1"), stream);
        assert_eq!(actor_telemetry(&dwelling, "EvDriver:EV1"), telemetry);
    }

    /// An actor bound to `Target` that sends a power setpoint while
    /// declaring no capabilities, from `decide` or from `adjust_for_pv`,
    /// to `Target` or, when `to_other` is set, to another equipment.
    struct Undeclared {
        from_pv: bool,
        to_other: bool,
    }

    impl Undeclared {
        fn send(&self, out: &mut Vec<hares_control::DispatchRequest>) {
            let signal = ControlSignal::PowerSetpoint {
                active_power_kw: 0.0,
                reactive_power_kvar: None,
                min_soc: None,
                max_soc: None,
            };
            let target = if self.to_other { "Other" } else { "Target" };
            out.push(hares_control::DispatchRequest {
                target: DispatchTarget::ByName(Arc::from(target)),
                priority: PriorityTier::from(&signal),
                signal,
            });
        }
    }

    impl crate::Actor for Undeclared {
        fn name(&self) -> &str {
            "Undeclared"
        }
        fn dispatch_target_name(&self) -> Option<&str> {
            Some("Target")
        }
        fn decide(
            &mut self,
            _env: &hares_types::EnvironmentState,
            out: &mut Vec<hares_control::DispatchRequest>,
        ) {
            if !self.from_pv {
                self.send(out);
            }
        }
        fn adjust_for_pv(
            &mut self,
            _pv_kw: f64,
            _env: &hares_types::EnvironmentState,
            out: &mut Vec<hares_control::DispatchRequest>,
        ) {
            if self.from_pv {
                self.send(out);
            }
        }
    }

    fn step_with_undeclared(actor: Undeclared) -> Result<StepResult> {
        let mut dwelling = bestest_dwelling();
        for name in ["Target", "Other"] {
            dwelling
                .add_equipment(Box::new(TestEquipment::new(
                    name,
                    ControlCapabilities::POWER_SETPOINT,
                )))
                .expect("add the equipment");
        }
        dwelling.add_actor(Box::new(actor)).expect("add the actor");
        dwelling.step()
    }

    /// The re-evaluation after PV output holds a bound actor to its
    /// declaration as its decision does.
    #[test]
    fn an_undeclared_signal_from_the_pv_re_evaluation_fails_the_step() {
        let err = step_with_undeclared(Undeclared {
            from_pv: true,
            to_other: false,
        })
        .expect_err("an undeclared signal fails the step");
        assert!(err.to_string().contains("Undeclared"), "got: {err}");
    }

    /// The declaration covers what an actor sends its bound target; a
    /// signal it sends another equipment is that equipment's to accept.
    #[test]
    fn a_bound_actors_declaration_covers_only_its_target() {
        step_with_undeclared(Undeclared {
            from_pv: false,
            to_other: true,
        })
        .expect("a signal to another equipment is not held to the declaration");
    }

    /// An actor bound to one equipment that sends a signal its declared
    /// capabilities omit fails the step, naming the actor: the declaration
    /// replacements are checked against is the set of signals the actor
    /// sends.
    #[test]
    fn a_bound_actor_sending_an_undeclared_signal_fails_the_step() {
        struct Undeclared;
        impl crate::Actor for Undeclared {
            fn name(&self) -> &str {
                "Undeclared"
            }
            fn dispatch_target_name(&self) -> Option<&str> {
                Some("Target")
            }
            fn decide(
                &mut self,
                _env: &hares_types::EnvironmentState,
                out: &mut Vec<hares_control::DispatchRequest>,
            ) {
                let signal = ControlSignal::PowerSetpoint {
                    active_power_kw: 0.0,
                    reactive_power_kvar: None,
                    min_soc: None,
                    max_soc: None,
                };
                out.push(hares_control::DispatchRequest {
                    target: DispatchTarget::ByName(Arc::from("Target")),
                    priority: PriorityTier::from(&signal),
                    signal,
                });
            }
        }
        let mut dwelling = bestest_dwelling();
        dwelling
            .add_equipment(Box::new(TestEquipment::new(
                "Target",
                ControlCapabilities::POWER_SETPOINT,
            )))
            .expect("add the target");
        dwelling
            .add_actor(Box::new(Undeclared))
            .expect("add the actor");

        let err = dwelling
            .step()
            .expect_err("an undeclared signal fails the step");
        assert!(err.to_string().contains("Undeclared"), "got: {err}");
    }

    /// Replacing equipment in kind with equipment that rejects an actor's
    /// signals evicts that actor rather than leaving it dispatching to a
    /// target that refuses every command.
    #[test]
    fn replace_in_kind_evicts_actors_whose_signals_the_replacement_rejects() {
        let mut dwelling = bestest_dwelling();
        add_driven_ev(&mut dwelling, "EV1");
        let stream = driver_stream(&dwelling, "EV1");

        let another_ev = driven_ev("EV1", &dwelling.latest_env);
        if let Err(err) = dwelling.replace_equipment("EV1", another_ev) {
            panic!("replace the EV with another EV: {err}");
        }
        assert_eq!(
            driver_stream(&dwelling, "EV1"),
            stream,
            "an EV accepts its driver's signals, so the driver stays"
        );

        if let Err(err) = dwelling.replace_equipment(
            "EV1",
            Box::new(TestEquipment::new(
                "EV1",
                ControlCapabilities::POWER_SETPOINT,
            )),
        ) {
            panic!("replace in kind: {err}");
        }

        assert!(
            !actor_names(&dwelling).contains(&"EvDriver:EV1".to_string()),
            "the driver must leave with the EV: {:?}",
            actor_names(&dwelling)
        );
    }

    /// Every rejected add or replace returns `RejectedEquipment`, whether
    /// or not the candidate raised warnings, with the reason inside.
    #[test]
    fn rejected_add_and_replace_return_rejected_equipment_without_warnings() {
        let mut dwelling = bestest_dwelling();
        let existing = dwelling
            .equipment()
            .first()
            .expect("bestest assembles equipment")
            .descriptor()
            .name
            .clone();

        let add = dwelling.add_equipment(Box::new(TestEquipment::new(
            &existing,
            ControlCapabilities::empty(),
        )));
        let replace = dwelling.replace_equipment(
            "No Such Equipment",
            Box::new(TestEquipment::new("X", ControlCapabilities::empty())),
        );

        assert!(
            matches!(
                add,
                Err(HaresError::RejectedEquipment { ref reason, ref warnings })
                    if matches!(**reason, HaresError::Equipment(_)) && warnings.is_empty()
            ),
            "got: {add:?}"
        );
        assert!(
            matches!(
                replace,
                Err(HaresError::RejectedEquipment { ref reason, ref warnings })
                    if matches!(**reason, HaresError::Dwelling(_)) && warnings.is_empty()
            ),
            "got: {:?}",
            replace.map(|_| ())
        );
    }

    /// `add_equipment_with_actors` is one change: a supplied actor whose
    /// name is taken rejects the equipment too, and an equipment whose name
    /// is taken is rejected for that, ahead of its actors.
    #[test]
    fn add_equipment_with_actors_rejects_both_or_neither() {
        let mut dwelling = bestest_dwelling();
        dwelling
            .add_actor(Box::new(StubActor {
                name: "Taken".to_string(),
            }))
            .expect("add an actor");
        let before = roster_fingerprint(&dwelling);

        let taken_actor = dwelling.add_equipment_with_actors(
            Box::new(TestEquipment::new("Fresh", ControlCapabilities::empty())),
            vec![Box::new(StubActor {
                name: "Taken".to_string(),
            })],
        );
        assert!(
            matches!(
                &taken_actor,
                Err(HaresError::RejectedEquipment { reason, .. })
                    if matches!(**reason, HaresError::Control(_))
            ),
            "got: {taken_actor:?}"
        );
        assert_eq!(roster_fingerprint(&dwelling), before);

        let existing = dwelling.equipment()[0].descriptor().name.clone();
        let both_taken = dwelling.add_equipment_with_actors(
            Box::new(TestEquipment::new(&existing, ControlCapabilities::empty())),
            vec![Box::new(StubActor {
                name: "Taken".to_string(),
            })],
        );
        assert!(
            matches!(
                &both_taken,
                Err(HaresError::RejectedEquipment { reason, .. })
                    if matches!(**reason, HaresError::Equipment(_))
            ),
            "the duplicate equipment name is reported first: {both_taken:?}"
        );
        assert_eq!(roster_fingerprint(&dwelling), before);
    }

    /// A supplied actor holding the built-in actor's name takes its place:
    /// no built-in driver is built and no driver stream is consumed.
    #[test]
    fn a_supplied_driver_replaces_the_built_in_one() {
        let mut dwelling = bestest_dwelling();
        let cursor = dwelling.next_ev_driver_stream;
        let ev = driven_ev("EV1", &dwelling.latest_env);

        dwelling
            .add_equipment_with_actors(
                ev,
                vec![Box::new(StubActor {
                    name: "EvDriver:EV1".to_string(),
                })],
            )
            .expect("add the EV with its own driver");

        assert_eq!(dwelling.next_ev_driver_stream, cursor);
        assert!(
            !dwelling
                .auto_registered_actor_names
                .contains("EvDriver:EV1")
        );
        assert_eq!(
            actor_names(&dwelling)
                .iter()
                .filter(|n| n.as_str() == "EvDriver:EV1")
                .count(),
            1
        );
    }

    /// Equipment whose `core_output` panics: a roster change that reads it
    /// must panic before it changes anything.
    struct PanickingCoreOutput {
        inner: TestEquipment,
    }

    impl Equipment for PanickingCoreOutput {
        fn descriptor(&self) -> &EquipmentDescriptor {
            self.inner.descriptor()
        }
        fn rename(&mut self, name: String) {
            self.inner.rename(name);
        }
        fn set_equipment_id(&mut self, id: EquipmentId) -> std::result::Result<(), HaresError> {
            self.inner.set_equipment_id(id)
        }
        fn ports(&self) -> &[PortDeclaration] {
            self.inner.ports()
        }
        fn init(
            &mut self,
            config: &EquipmentConfig,
            env: &hares_types::EnvironmentState,
        ) -> std::result::Result<(), HaresError> {
            self.inner.init(config, env)
        }
        fn update_control(&mut self, env: &hares_types::EnvironmentState) -> OperatingMode {
            self.inner.update_control(env)
        }
        fn step(
            &mut self,
            env: &hares_types::EnvironmentState,
            dt: Duration,
            ports: &mut PortSlots,
        ) -> std::result::Result<(), HaresError> {
            self.inner.step(env, dt, ports)
        }
        fn telemetry(&self) -> &Telemetry {
            self.inner.telemetry()
        }
        fn core_output(&self) -> &CoreOutput {
            panic!("core_output panics");
        }
        fn save_state(&self) -> std::result::Result<Vec<u8>, HaresError> {
            self.inner.save_state()
        }
        fn load_state(&mut self, state: &[u8]) -> std::result::Result<(), HaresError> {
            self.inner.load_state(state)
        }
        fn apply_signal(&mut self, signal: &ControlSignal) -> std::result::Result<(), HaresError> {
            self.inner.apply_signal(signal)
        }
    }

    /// A panic raised by equipment code during a roster change leaves the
    /// dwelling as it was: every call into equipment and actor code happens
    /// before the change is committed.
    #[test]
    fn a_panicking_candidate_leaves_the_dwelling_unchanged() {
        let mut dwelling = bestest_dwelling();
        let before = roster_fingerprint(&dwelling);

        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            dwelling.add_equipment(Box::new(PanickingCoreOutput {
                inner: TestEquipment::new("Panicky", ControlCapabilities::empty()),
            }))
        }));

        assert!(panicked.is_err());
        assert_eq!(roster_fingerprint(&dwelling), before);
    }

    /// With output disabled the step result still reports every zone's
    /// temperature after a roster change, read from the environment.
    #[test]
    fn zone_temperatures_are_reported_without_output_after_a_roster_change() {
        let mut dwelling = bestest_dwelling();
        assert!(!dwelling.write_output);
        dwelling
            .add_equipment(Box::new(TestEquipment::new(
                "Added",
                ControlCapabilities::empty(),
            )))
            .expect("add");

        let result = dwelling.step().expect("step");

        let mut expected: Vec<(ZoneId, f64)> = dwelling
            .latest_env()
            .zones
            .iter()
            .map(|z| (z.id, z.temperature_c))
            .collect();
        expected.sort_by_key(|&(id, _)| id);
        assert!(!expected.is_empty());
        assert_eq!(result.zone_temperatures_c, expected);
    }

    /// The failing-equipment rollback path feeds the run-total health
    /// counter in every build profile (plain release included): two failed
    /// equipment steps in one timestep must land as exactly two rollbacks.
    #[test]
    fn health_counters_nonzero_in_default_build() {
        let mut dwelling = bestest_dwelling();
        let failing_a = FailingPortEquipment::new("FailingEq1", ExecutionStage::Independent);
        let failing_b = FailingPortEquipment::new("FailingEq2", ExecutionStage::Independent);
        replace_equipment_for_test(
            &mut dwelling,
            vec![Box::new(failing_a), Box::new(failing_b)],
        );

        dwelling
            .run_timestep(false)
            .expect("dwelling step tolerates equipment failure");

        assert_eq!(dwelling.health().port_rollbacks, 2);
    }

    /// A rejected control signal (out-of-bounds SOCTarget) is counted on
    /// the dwelling that received it (and only on that dwelling) when
    /// two dwellings are stepped alternately on one thread.
    #[test]
    fn rejected_signal_counted_per_run() {
        let mut dwelling_a = bestest_dwelling();
        let mut dwelling_b = bestest_dwelling();
        let mut eq_a = TestEquipment::new(
            "LoadA",
            ControlCapabilities::POWER_SETPOINT | ControlCapabilities::SOC_TARGET,
        );
        eq_a.init(&EquipmentConfig::default(), &dwelling_a.latest_env)
            .expect("init LoadA");
        replace_equipment_for_test(&mut dwelling_a, vec![Box::new(eq_a)]);
        let mut eq_b = TestEquipment::new(
            "LoadB",
            ControlCapabilities::POWER_SETPOINT | ControlCapabilities::SOC_TARGET,
        );
        eq_b.init(&EquipmentConfig::default(), &dwelling_b.latest_env)
            .expect("init LoadB");
        replace_equipment_for_test(&mut dwelling_b, vec![Box::new(eq_b)]);

        // Out of bounds: target_soc must be within [0, 1].
        dwelling_a.apply_control(
            "LoadA",
            ControlSignal::SOCTarget {
                target_soc: 1.5,
                min_soc: None,
                max_soc: None,
            },
        );

        for _ in 0..2 {
            dwelling_a.run_timestep(false).expect("dwelling A step");
            dwelling_b.run_timestep(false).expect("dwelling B step");
        }

        assert_eq!(
            dwelling_a.health().rejected_control_signals,
            1,
            "the out-of-bounds signal is counted once on the receiving dwelling"
        );
        assert_eq!(
            dwelling_b.health().rejected_control_signals,
            0,
            "the other dwelling must not inherit the rejection"
        );
        assert!(
            dwelling_a
                .warnings
                .iter()
                .any(|w| w.contains("control apply failed for 'LoadA'")),
            "every rejection stays a warning; warnings: {:?}",
            dwelling_a.warnings
        );
    }

    /// A multi-speed AC whose requested stage evaluates a curve index beyond
    /// the configured curve set (four speed stages, one shared cap+EIR
    /// pair, legitimate at init) is clamped at the hot-path curve lookup;
    /// the dwelling drains that clamp count into
    /// `health.curve_index_clamps` once per timestep.
    #[test]
    fn curve_index_clamp_counted() {
        use hares_equipment::hvac::air_conditioner::AirConditioner;
        use hares_equipment::{CentralAirConditionerConfig, DuctConfig, HvacSetpointConfig};

        let mut dwelling = bestest_dwelling();
        let config = EquipmentConfig::from_typed(
            "AC1".to_string(),
            "Air Conditioner".to_string(),
            CentralAirConditionerConfig {
                equipment_id: None,
                zone_id: Some(1),
                capacity_w: 8_000.0,
                eir: 0.33,
                shr: Some(0.75),
                number_of_speeds: 4,
                stage_capacities_w: Some(vec![2_000.0, 4_000.0, 6_000.0, 8_000.0]),
                stage_eirs: None,
                stage_shrs: None,
                fan_power_w: None,
                fan_power_w_per_cfm: None,
                setpoint: HvacSetpointConfig {
                    cooling_setpoint_c: Some(0.0),
                    heating_setpoint_c: Some(18.0),
                    heating_setpoint_source: None,
                    cooling_setpoint_source: None,
                },
                hysteresis_c: Some(1.0),
                airflow_m3_s_per_w: Some(hares_equipment::hvac::AIRFLOW_CENTRAL_AC_M3_S_PER_W),
                fraction_load_served: None,
                crankcase_heater_kw: None,
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
        .expect("typed AC config");
        let mut ac = AirConditioner::new(config.clone());
        ac.init(&config, &dwelling.latest_env)
            .expect("init AC with one shared curve pair across three stages");
        replace_equipment_for_test(&mut dwelling, vec![Box::new(ac)]);

        // Max cooling demand (setpoint 0 °C against a ~20 °C zone) drives
        // the AC to its highest stage, whose curve index (speed 2 → 4)
        // exceeds the single shared cap+EIR pair. The step itself may fail
        // the temperature-bounds invariant from the deliberately absurd
        // one-step cool-down; the health counter is the subject, and it is
        // drained before the invariant pass.
        let _ = dwelling.run_timestep(false);

        assert!(
            dwelling.health().curve_index_clamps >= 1,
            "the out-of-bounds curve evaluation must land in the dwelling's run total; got {:?}",
            dwelling.health()
        );
    }

    /// Warm-up capped at two days on a non-converging run records
    /// `Ran { converged: false, days_run: 2 }` on the run health and pushes
    /// exactly one warm-up warning naming the exceeding zones.
    #[test]
    fn warmup_nonconvergence_recorded_and_warned() {
        let mut dwelling = bestest_dwelling();

        // A 1e-9 °C threshold cannot be met across consecutive warm-up days
        // of a transient start; the fixture is non-converging under it.
        let days = dwelling
            .run_warmup_converged(1e-9, 2)
            .expect("warm-up runs to its iteration cap");

        assert_eq!(days, 2);
        match &dwelling.health().warmup {
            WarmupOutcome::Ran {
                days_run,
                converged,
                residuals,
            } => {
                assert_eq!(*days_run, 2);
                assert!(!converged);
                assert!(
                    residuals.is_some(),
                    "two warm-up days ran, so a predecessor existed and residuals were computed"
                );
            }
            other => panic!("expected WarmupOutcome::Ran, got {other:?}"),
        }
        let warmup_warnings = dwelling
            .warnings
            .iter()
            .filter(|w| w.contains("warm-up failed to converge"))
            .count();
        assert_eq!(
            warmup_warnings, 1,
            "exactly one warm-up non-convergence warning; warnings: {:?}",
            dwelling.warnings
        );
    }

    /// The recorded warm-up outcome mirrors what `run_warmup_converged`
    /// returned (days_run unchanged by the health recording), and the final
    /// day's residuals are present with finite entries whenever a
    /// predecessor day existed.
    #[test]
    fn warmup_residuals_recorded_without_changing_days_run() {
        let mut dwelling = bestest_dwelling();

        let days = dwelling.run_warmup_converged(0.5, 3).expect("warm-up runs");

        match &dwelling.health().warmup {
            WarmupOutcome::Ran {
                days_run,
                converged,
                residuals,
            } => {
                assert_eq!(
                    *days_run, days,
                    "recorded days_run must equal the returned count"
                );
                assert_eq!(*converged, days < 3);
                if days >= 2 {
                    let r = residuals.as_ref().expect("a predecessor day existed");
                    assert!(r.max_temperature_c.is_finite());
                    assert!(r.min_temperature_c.is_finite());
                    assert!(r.heating_load.is_finite());
                    assert!(r.cooling_load.is_finite());
                } else {
                    assert!(residuals.is_none(), "one warm-up day has no predecessor");
                }
            }
            other => panic!("expected WarmupOutcome::Ran, got {other:?}"),
        }
    }

    /// The thermal-balance seam forces `InvariantViolation` through the
    /// always-on check in every build profile, plain release included.
    #[test]
    fn thermal_balance_violation_fails_in_release() {
        let mut dwelling = bestest_dwelling();
        dwelling.set_thermal_invariant_failure_for_test();

        let err = dwelling
            .run_timestep(false)
            .expect_err("the armed seam must fail the step");

        assert!(
            matches!(
                &err,
                HaresError::InvariantViolation { check_name, .. } if check_name == "thermal_balance"
            ),
            "got: {err:?}"
        );
    }

    /// A latched non-finite telemetry write fails the step with
    /// `NanDetected`, naming the equipment and the key, in every build
    /// profile. NaN is never asserted equal to NaN: the error carries the
    /// key, the assertion matches by parts.
    #[test]
    fn non_finite_telemetry_fails_in_release() {
        let mut dwelling = bestest_dwelling();
        let mut eq = NonFiniteTelemetryEquipment::new("CorruptedTelemetry");
        eq.init(&EquipmentConfig::default(), &dwelling.latest_env)
            .expect("init");
        replace_equipment_for_test(&mut dwelling, vec![Box::new(eq)]);

        let err = dwelling
            .run_timestep(false)
            .expect_err("a latched non-finite telemetry write must fail the step");

        assert!(
            matches!(&err, HaresError::NanDetected { value_name, .. }
                if value_name.contains("corrupted_w")
                    && value_name.contains("CorruptedTelemetry")),
            "the failure must name the equipment and the key, got: {err:?}"
        );
    }

    /// A `set` on an unregistered telemetry key fails the step naming the
    /// equipment and the key, in every build profile: the unknown-key
    /// latch is enforced beside the non-finite latch at step end, turning
    /// the wiring fault the telemetry map latched into a run failure.
    #[test]
    fn unknown_key_telemetry_fails_in_release() {
        let mut dwelling = bestest_dwelling();
        let mut eq = UnknownKeyTelemetryEquipment::new("MiswiredTelemetry");
        eq.init(&EquipmentConfig::default(), &dwelling.latest_env)
            .expect("init");
        replace_equipment_for_test(&mut dwelling, vec![Box::new(eq)]);

        let err = dwelling
            .run_timestep(false)
            .expect_err("a latched unknown telemetry key must fail the step");

        assert!(
            matches!(&err, HaresError::Equipment(message)
                if message.contains("unregistered_w")
                    && message.contains("MiswiredTelemetry")),
            "the failure must name the equipment and the key, got: {err:?}"
        );
    }

    /// A NaN sensible gain into an unconditioned zone during warm-up fails
    /// the run: the zone NaN check covers every zone, warm-up included.
    #[test]
    fn nan_zone_temperature_during_warmup_fails() {
        let mut dwelling = bestest_dwelling();
        // Treat the single zone as unconditioned: the check grants no
        // exemption to unconditioned zones or warm-up.
        dwelling.zone_is_conditioned = vec![false];
        let mut eq = NanThermalGainEquipment::new("NanGain", ZoneId(1));
        eq.init(&EquipmentConfig::default(), &dwelling.latest_env)
            .expect("init");
        replace_equipment_for_test(&mut dwelling, vec![Box::new(eq)]);

        let err = dwelling
            .run_warmup_converged(0.5, 2)
            .expect_err("a NaN zone temperature during warm-up must fail the run");

        assert!(
            matches!(
                &err,
                HaresError::InvariantViolation { check_name, .. } if check_name == "zone_temperature_nan"
            ),
            "got: {err:?}"
        );
    }

    /// The delivered-energy seam armed before warm-up forces
    /// `NegativeDeliveredEnergy` during the warm-up loop itself: warm-up no
    /// longer relaxes the HVAC delivered-energy invariant.
    #[test]
    fn negative_delivered_energy_during_warmup_fails() {
        let mut dwelling = bestest_dwelling();
        dwelling.set_hvac_negative_energy_failure_for_test();

        let err = dwelling
            .run_warmup_converged(0.5, 2)
            .expect_err("the armed seam must fail the warm-up run");

        assert!(
            matches!(&err, HaresError::NegativeDeliveredEnergy { .. }),
            "got: {err:?}"
        );
    }

    /// Two dwellings built from the same config (with warmup) and simulated
    /// produce identical step results — verifying deterministic reproducibility.
    #[test]
    fn same_seed_produces_identical_simulation_with_warmup() {
        let dir = tempfile::tempdir().expect("temp dir");
        let toml_path_a = dir.path().join("repro-a.toml");
        let toml_path_b = dir.path().join("repro-b.toml");

        let toml_content = r#"building_id = 2002

[simulation]
start_time = "2024-01-15T00:00:00Z"
time_res_s = 3600
duration_s = 172800
initialization_duration_s = 86400

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.8

[hvac]
equipment_name = "Furnace"
fuel = "electricity"
heating_capacity_kbtu_h = 30.0

[weather]
outdoor_temp_c = 10.0
dew_point_c = 5.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325

[schedule]
occupancy = 1.0

[output]
write_output = false
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
master_seed = 42
"#;

        fs::write(&toml_path_a, toml_content).expect("write TOML A");
        fs::write(&toml_path_b, toml_content).expect("write TOML B");

        let mut dwelling_a =
            Dwelling::from_toml_config_with_write_output(&toml_path_a, Some(false))
                .expect("build dwelling A");
        let mut dwelling_b =
            Dwelling::from_toml_config_with_write_output(&toml_path_b, Some(false))
                .expect("build dwelling B");

        let results_a = dwelling_a.simulate().expect("simulate A");
        let results_b = dwelling_b.simulate().expect("simulate B");

        assert_eq!(
            results_a.steps.len(),
            results_b.steps.len(),
            "step counts must match"
        );

        for (i, (step_a, step_b)) in results_a
            .steps
            .iter()
            .zip(results_b.steps.iter())
            .enumerate()
        {
            assert_eq!(
                step_a.zone_temperatures_c, step_b.zone_temperatures_c,
                "zone temperatures must match at step {i}"
            );
        }
    }

    /// Two dwellings without warmup also produce identical results — the RNG
    /// restoration code must not break the no-warmup path.
    #[test]
    fn same_seed_produces_identical_simulation_without_warmup() {
        let dir = tempfile::tempdir().expect("temp dir");
        let toml_path_a = dir.path().join("nowu-repro-a.toml");
        let toml_path_b = dir.path().join("nowu-repro-b.toml");

        let toml_content = r#"building_id = 2003

[simulation]
start_time = "2024-01-15T00:00:00Z"
time_res_s = 60
duration_s = 600

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.8

[hvac]
equipment_name = "Furnace"
fuel = "electricity"
heating_capacity_kbtu_h = 30.0

[weather]
outdoor_temp_c = 10.0
dew_point_c = 5.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325

[schedule]
occupancy = 1.0

[output]
write_output = false
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
master_seed = 7
"#;

        fs::write(&toml_path_a, toml_content).expect("write TOML A");
        fs::write(&toml_path_b, toml_content).expect("write TOML B");

        let mut dwelling_a =
            Dwelling::from_toml_config_with_write_output(&toml_path_a, Some(false))
                .expect("build dwelling A");
        let mut dwelling_b =
            Dwelling::from_toml_config_with_write_output(&toml_path_b, Some(false))
                .expect("build dwelling B");

        let results_a = dwelling_a.simulate().expect("simulate A");
        let results_b = dwelling_b.simulate().expect("simulate B");

        assert_eq!(
            results_a.steps.len(),
            results_b.steps.len(),
            "step counts must match"
        );

        for (i, (step_a, step_b)) in results_a
            .steps
            .iter()
            .zip(results_b.steps.iter())
            .enumerate()
        {
            assert_eq!(
                step_a.zone_temperatures_c, step_b.zone_temperatures_c,
                "zone temperatures must match at step {i}"
            );
        }
    }

    #[cfg(feature = "observe")]
    #[test]
    fn custom_solvers_captured_in_observer_snapshot() {
        use hares_types::{DomainId, DomainSolver, DomainUpdate, EnvironmentState, PortSlots};
        use std::time::Duration;

        struct StubSolver {
            id: DomainId,
            state: Vec<f64>,
        }

        impl DomainSolver for StubSolver {
            fn domain_id(&self) -> DomainId {
                self.id
            }
            fn resolve(
                &mut self,
                _ports: &PortSlots,
                _env: &EnvironmentState,
                _dt: Duration,
                _out: &mut DomainUpdate,
            ) -> std::result::Result<(), HaresError> {
                Ok(())
            }
            fn observation_state(&self) -> Vec<f64> {
                self.state.clone()
            }
        }

        let dir = tempfile::tempdir().expect("temp dir");
        let toml_path = dir.path().join("custom-solver-capture.toml");

        std::fs::write(
            &toml_path,
            r#"building_id = 999
[simulation]
start_time = "2024-01-15T00:00:00Z"
time_res_s = 60
duration_s = 120

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.8

[hvac]
equipment_name = "Furnace"
fuel = "electricity"
heating_capacity_kbtu_h = 30.0

[weather]
outdoor_temp_c = -10.0
dew_point_c = -5.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325

[infiltration]
ach = 0.5

[schedule]
occupancy = 1.0

[output]
write_output = false
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
master_seed = 0
"#,
        )
        .expect("write synthetic TOML");

        let mut dwelling = Dwelling::from_toml_config(&toml_path).expect("build dwelling");

        dwelling
            .custom_update_bufs
            .push(hares_types::DomainUpdate::empty(hares_types::DomainId(0)));

        dwelling.custom_domain_solvers.push(Box::new(StubSolver {
            id: DomainId(42),
            state: vec![1.0, 2.0, 3.0],
        }));

        dwelling.enable_observer(10);

        dwelling.run_timestep(false).expect("step");

        let snapshots = dwelling.drain_observations();
        assert_eq!(snapshots.len(), 1, "expected one snapshot after one step");

        let snapshot = &snapshots[0];
        let custom_capture = snapshot
            .phases
            .post_custom_solvers
            .as_ref()
            .expect("post_custom_solvers must be populated");

        assert_eq!(custom_capture.solvers.len(), 1);
        assert_eq!(custom_capture.solvers[0].domain_id, DomainId(42));
        assert_eq!(custom_capture.solvers[0].state, vec![1.0, 2.0, 3.0]);
    }

    #[cfg(feature = "observe")]
    #[test]
    fn builtin_and_custom_solver_captures_coexist() {
        use hares_types::{DomainId, DomainSolver, DomainUpdate, EnvironmentState, PortSlots};
        use std::time::Duration;

        struct StubSolver {
            id: DomainId,
        }

        impl DomainSolver for StubSolver {
            fn domain_id(&self) -> DomainId {
                self.id
            }
            fn resolve(
                &mut self,
                _ports: &PortSlots,
                _env: &EnvironmentState,
                _dt: Duration,
                _out: &mut DomainUpdate,
            ) -> std::result::Result<(), HaresError> {
                Ok(())
            }
        }

        let dir = tempfile::tempdir().expect("temp dir");
        let toml_path = dir.path().join("coexist-capture.toml");

        std::fs::write(
            &toml_path,
            r#"building_id = 999
[simulation]
start_time = "2024-01-15T00:00:00Z"
time_res_s = 60
duration_s = 120

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.8

[hvac]
equipment_name = "Furnace"
fuel = "electricity"
heating_capacity_kbtu_h = 30.0

[weather]
outdoor_temp_c = -10.0
dew_point_c = -5.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325

[infiltration]
ach = 0.5

[schedule]
occupancy = 1.0

[output]
write_output = false
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
master_seed = 0
"#,
        )
        .expect("write synthetic TOML");

        let mut dwelling = Dwelling::from_toml_config(&toml_path).expect("build dwelling");

        dwelling
            .custom_update_bufs
            .push(hares_types::DomainUpdate::empty(hares_types::DomainId(0)));

        dwelling
            .custom_domain_solvers
            .push(Box::new(StubSolver { id: DomainId(77) }));

        dwelling.enable_observer(10);

        dwelling.run_timestep(false).expect("step");

        let snapshots = dwelling.drain_observations();
        assert_eq!(snapshots.len(), 1);

        let snapshot = &snapshots[0];

        let solver_capture = snapshot
            .phases
            .post_solvers
            .as_ref()
            .expect("post_solvers must be populated for built-in solvers");

        assert!(
            !solver_capture.thermal_update.zone_temperatures_c.is_empty(),
            "thermal update must contain zone temperatures"
        );

        let custom_capture = snapshot
            .phases
            .post_custom_solvers
            .as_ref()
            .expect("post_custom_solvers must be populated for custom solvers");

        assert_eq!(custom_capture.solvers.len(), 1);
        assert_eq!(custom_capture.solvers[0].domain_id, DomainId(77));
    }

    // ── Dwelling RNG advancement tests ──

    /// Construct a `Dwelling`, advance one timestep, and assert that the
    /// dwelling RNG's word position has changed (i.e. it was sampled during
    /// the timestep).  This verifies the core fix for T-0213.
    #[test]
    fn dwelling_rng_is_advanced_during_timestep() {
        let (_dir, toml_path) = temp_toml("rng_advance");
        write_minimal_toml(&toml_path);

        let mut dwelling = Dwelling::from_toml_config(&toml_path).expect("build dwelling");
        let pos_before = dwelling.rng.get_word_pos();
        dwelling.step().expect("step succeeds");
        let pos_after = dwelling.rng.get_word_pos();
        assert!(
            pos_after > pos_before,
            "dwelling RNG word position {pos_before} must increase after step, got {pos_after}"
        );
    }

    /// Construct two identical dwellings with the same `master_seed` and
    /// `bldg_id`, run `simulate()` to completion on both, and assert that
    /// all `StepResult` fields match within `f64::EPSILON`.  This is the
    /// end-to-end reproducibility regression test from Recommendation #3
    /// of the underlying review finding.
    #[test]
    fn identical_dwellings_produce_reproducible_results() {
        let (_dir_a, toml_path_a) = temp_toml("rng_repro_a");
        write_minimal_toml(&toml_path_a);
        let (_dir_b, toml_path_b) = temp_toml("rng_repro_b");
        fs::copy(&toml_path_a, &toml_path_b).expect("copy TOML");

        let mut a = Dwelling::from_toml_config(&toml_path_a).expect("build dwelling A");
        let mut b = Dwelling::from_toml_config(&toml_path_b).expect("build dwelling B");

        let results_a = a.simulate().expect("simulate A");
        let results_b = b.simulate().expect("simulate B");

        assert_eq!(
            results_a.steps.len(),
            results_b.steps.len(),
            "both dwellings must produce the same number of steps"
        );

        for (i, (step_a, step_b)) in results_a
            .steps
            .iter()
            .zip(results_b.steps.iter())
            .enumerate()
        {
            assert_eq!(
                step_a.timestamp, step_b.timestamp,
                "step {i}: timestamps must match"
            );
            assert!(
                (step_a.net_electric_power_kw - step_b.net_electric_power_kw).abs() < f64::EPSILON,
                "step {i}: net_electric_power_kw mismatch: {} vs {}",
                step_a.net_electric_power_kw,
                step_b.net_electric_power_kw,
            );
            assert_eq!(
                step_a.zone_temperatures_c.len(),
                step_b.zone_temperatures_c.len(),
                "step {i}: zone count mismatch"
            );
            for (z, (zt_a, zt_b)) in step_a
                .zone_temperatures_c
                .iter()
                .zip(step_b.zone_temperatures_c.iter())
                .enumerate()
            {
                assert!(
                    (zt_a.1 - zt_b.1).abs() < f64::EPSILON,
                    "step {i} zone {z}: temperature mismatch: {} vs {}",
                    zt_a.1,
                    zt_b.1,
                );
            }
            assert!(
                (step_a.hvac_heating_w - step_b.hvac_heating_w).abs() < f64::EPSILON,
                "step {i}: hvac_heating_w mismatch"
            );
            assert!(
                (step_a.hvac_cooling_w - step_b.hvac_cooling_w).abs() < f64::EPSILON,
                "step {i}: hvac_cooling_w mismatch"
            );
            assert!(
                (step_a.gas_power_w - step_b.gas_power_w).abs() < f64::EPSILON,
                "step {i}: gas_power_w mismatch"
            );
        }
    }

    /// Helper: write a synthetic TOML with a stochastic CookingRange event
    /// load so the reproducibility test exercises RNG-dependent behaviour.
    fn write_event_load_toml(path: &PathBuf, master_seed: u64) {
        let content = format!(
            r#"building_id = 9001

[simulation]
start_time = "2024-06-15T12:00:00Z"
time_res_s = 60
duration_s = 600

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.8

[hvac]
equipment_name = "none"

[weather]
outdoor_temp_c = 20.0
dew_point_c = 10.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325

[schedule]
occupancy = 0.0

[output]
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
write_output = false
master_seed = {master_seed}

[event_load]
active_power_kw = 0.5
active_duration_s = 60.0
cooldown_duration_s = 60.0
event_probability = 0.5
sensible_gain_fraction = 0.72
latent_gain_fraction = 0.08
"#
        );
        fs::write(path, &content).expect("write event-load TOML");
    }

    /// Construct two identical dwellings with a stochastic event-based load
    /// (CookingRange at p=0.5), run `simulate()` to completion, and assert
    /// all `StepResult` fields match within `f64::EPSILON`.  The event load
    /// consumes the dwelling's hierarchical RNG, so this test catches
    /// non-determinism from RNG state leakage, hash-map iteration order, and
    /// floating-point order-of-operations in stochastic code paths.
    #[test]
    fn identical_dwellings_with_stochastic_load_produce_reproducible_results() {
        let (_dir_a, toml_path_a) = temp_toml("rng_stoch_repro_a");
        write_event_load_toml(&toml_path_a, 42);
        let (_dir_b, toml_path_b) = temp_toml("rng_stoch_repro_b");
        fs::copy(&toml_path_a, &toml_path_b).expect("copy TOML");

        let mut a = Dwelling::from_toml_config(&toml_path_a).expect("build dwelling A");
        let mut b = Dwelling::from_toml_config(&toml_path_b).expect("build dwelling B");

        let results_a = a.simulate().expect("simulate A");
        let results_b = b.simulate().expect("simulate B");

        assert_eq!(
            results_a.steps.len(),
            results_b.steps.len(),
            "both dwellings must produce the same number of steps"
        );

        for (i, (step_a, step_b)) in results_a
            .steps
            .iter()
            .zip(results_b.steps.iter())
            .enumerate()
        {
            assert_eq!(
                step_a.timestamp, step_b.timestamp,
                "step {i}: timestamps must match"
            );
            assert!(
                (step_a.net_electric_power_kw - step_b.net_electric_power_kw).abs() < f64::EPSILON,
                "step {i}: net_electric_power_kw mismatch: {} vs {}",
                step_a.net_electric_power_kw,
                step_b.net_electric_power_kw,
            );
            assert_eq!(
                step_a.zone_temperatures_c.len(),
                step_b.zone_temperatures_c.len(),
                "step {i}: zone count mismatch"
            );
            for (z, (zt_a, zt_b)) in step_a
                .zone_temperatures_c
                .iter()
                .zip(step_b.zone_temperatures_c.iter())
                .enumerate()
            {
                assert!(
                    (zt_a.1 - zt_b.1).abs() < f64::EPSILON,
                    "step {i} zone {z}: temperature mismatch: {} vs {}",
                    zt_a.1,
                    zt_b.1,
                );
            }
            assert!(
                (step_a.hvac_heating_w - step_b.hvac_heating_w).abs() < f64::EPSILON,
                "step {i}: hvac_heating_w mismatch"
            );
            assert!(
                (step_a.hvac_cooling_w - step_b.hvac_cooling_w).abs() < f64::EPSILON,
                "step {i}: hvac_cooling_w mismatch"
            );
            assert!(
                (step_a.gas_power_w - step_b.gas_power_w).abs() < f64::EPSILON,
                "step {i}: gas_power_w mismatch"
            );
        }
    }

    /// Smoke test: two dwellings with the same stochastic event-load config
    /// but different `master_seed` values MUST produce meaningfully different
    /// output trajectories.  The event load draws from the dwelling's
    /// hierarchical RNG at each step to decide whether to start an event;
    /// with `event_probability = 0.5`, different seeds produce different
    /// event sequences, proving the RNG is actually consumed (not just a
    /// fixed sequence) and that the stochastic pipeline is live.
    #[test]
    fn different_master_seeds_produce_different_event_load_outputs() {
        let (_dir_a, toml_path_a) = temp_toml("rng_smoke_seed_a");
        write_event_load_toml(&toml_path_a, 42);
        let (_dir_b, toml_path_b) = temp_toml("rng_smoke_seed_b");
        write_event_load_toml(&toml_path_b, 99);

        let mut a = Dwelling::from_toml_config(&toml_path_a).expect("build dwelling A (seed 42)");
        let mut b = Dwelling::from_toml_config(&toml_path_b).expect("build dwelling B (seed 99)");

        let results_a = a.simulate().expect("simulate A");
        let results_b = b.simulate().expect("simulate B");

        assert_eq!(
            results_a.steps.len(),
            results_b.steps.len(),
            "both dwellings must produce the same number of steps"
        );

        // At least one timestep must have a different net electric power.
        // With event_probability = 0.5 and 10 independent RNG draws per
        // dwelling, the chance of two different ChaCha8Rng seeds producing
        // identical sequences is astronomically low (≈ 2^-10 < 0.1%).
        let any_differ = results_a
            .steps
            .iter()
            .zip(results_b.steps.iter())
            .any(|(sa, sb)| {
                (sa.net_electric_power_kw - sb.net_electric_power_kw).abs() > f64::EPSILON
            });
        assert!(
            any_differ,
            "different master seeds (42 vs 99) must produce different \
             net_electric_power_kw on at least one timestep; \
             if this fails consistently, the stochastic event-load RNG \
             pipeline is likely dead or producing a fixed sequence"
        );
    }

    /// Helper: write a synthetic TOML with a schedule-based event window and
    /// probability (24-hour vectors) so the reproducibility test exercises
    /// the `ColumnRef`-based schedule path through `build_synthetic_schedule`
    /// and `parse_event_schedule_sources`.
    fn write_event_load_schedule_toml(path: &PathBuf, master_seed: u64) {
        let content = format!(
            r#"building_id = 9002

[simulation]
start_time = "2024-06-15T12:00:00Z"
time_res_s = 60
duration_s = 600

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.8

[hvac]
equipment_name = "none"

[weather]
outdoor_temp_c = 20.0
dew_point_c = 10.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325

[schedule]
occupancy = 0.0

[output]
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
write_output = false
master_seed = {master_seed}

[event_load]
active_power_kw = 0.5
active_duration_s = 60.0
cooldown_duration_s = 60.0
event_probability = 0.5
sensible_gain_fraction = 0.72
latent_gain_fraction = 0.08
event_window_schedule = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0]
event_probability_schedule = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.0, 0.0, 0.0, 0.0]
"#
        );
        fs::write(path, &content).expect("write event-load schedule TOML");
    }

    /// Construct two identical dwellings with a schedule-based stochastic
    /// event load, run `simulate()` to completion, and assert all
    /// `StepResult` fields match within `f64::EPSILON`.  This exercises the
    /// `ColumnRef` path through `build_synthetic_schedule` →
    /// `parse_event_schedule_sources`, confirming that schedule-column-based
    /// event window and probability sources produce deterministic output when
    /// given the same master seed.
    #[test]
    fn identical_dwellings_with_schedule_based_stochastic_load_produce_reproducible_results() {
        let (_dir_a, toml_path_a) = temp_toml("rng_sched_repro_a");
        write_event_load_schedule_toml(&toml_path_a, 42);
        let (_dir_b, toml_path_b) = temp_toml("rng_sched_repro_b");
        fs::copy(&toml_path_a, &toml_path_b).expect("copy TOML");

        let mut a = Dwelling::from_toml_config(&toml_path_a).expect("build dwelling A");
        let mut b = Dwelling::from_toml_config(&toml_path_b).expect("build dwelling B");

        let results_a = a.simulate().expect("simulate A");
        let results_b = b.simulate().expect("simulate B");

        assert_eq!(
            results_a.steps.len(),
            results_b.steps.len(),
            "both dwellings must produce the same number of steps"
        );

        for (i, (step_a, step_b)) in results_a
            .steps
            .iter()
            .zip(results_b.steps.iter())
            .enumerate()
        {
            assert_eq!(
                step_a.timestamp, step_b.timestamp,
                "step {i}: timestamps must match"
            );
            assert!(
                (step_a.net_electric_power_kw - step_b.net_electric_power_kw).abs() < f64::EPSILON,
                "step {i}: net_electric_power_kw mismatch: {} vs {}",
                step_a.net_electric_power_kw,
                step_b.net_electric_power_kw,
            );
            for (z, (zt_a, zt_b)) in step_a
                .zone_temperatures_c
                .iter()
                .zip(step_b.zone_temperatures_c.iter())
                .enumerate()
            {
                assert!(
                    (zt_a.1 - zt_b.1).abs() < f64::EPSILON,
                    "step {i} zone {z}: temperature mismatch: {} vs {}",
                    zt_a.1,
                    zt_b.1,
                );
            }
            assert!(
                (step_a.hvac_heating_w - step_b.hvac_heating_w).abs() < f64::EPSILON,
                "step {i}: hvac_heating_w mismatch"
            );
            assert!(
                (step_a.hvac_cooling_w - step_b.hvac_cooling_w).abs() < f64::EPSILON,
                "step {i}: hvac_cooling_w mismatch"
            );
            assert!(
                (step_a.gas_power_w - step_b.gas_power_w).abs() < f64::EPSILON,
                "step {i}: gas_power_w mismatch"
            );
        }
    }

    /// Checkpoint a dwelling mid-simulation, restore it into a fresh
    /// dwelling, continue simulation, and assert that the restored
    /// dwelling produces identical results to the uninterrupted dwelling
    /// for all remaining timesteps.
    #[test]
    fn checkpoint_restart_produces_identical_continuation() {
        let (_dir_a, toml_path_a) = temp_toml("rng_ckpt_a");
        {
            let toml = format!(
                r#"building_id = 9001

[simulation]
start_time = "2024-06-15T12:00:00Z"
time_res_s = 60
duration_s = {}
# extra steps to allow restart+duration
[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0

[materials]
wall_r_value_m2_k_w = 2.8

[hvac]
equipment_name = "none"

[weather]
outdoor_temp_c = 20.0
dew_point_c = 10.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325

[schedule]
occupancy = 0.0

[output]
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
write_output = false
master_seed = 42
"#,
                // 5 min = 5 steps at 60s resolution.
                300
            );
            fs::write(&toml_path_a, toml).expect("write TOML A");
        }

        const CHECKPOINT_AT_STEP: u64 = 3;

        // --- Uninterrupted reference run ---
        let mut ref_dwelling =
            Dwelling::from_toml_config(&toml_path_a).expect("build ref dwelling");
        let ref_results = ref_dwelling.simulate().expect("simulate ref");

        // --- Interrupted run: step to checkpoint, save, restore, continue ---
        let mut dwelling_a = Dwelling::from_toml_config(&toml_path_a).expect("build dwelling A");
        for _ in 0..CHECKPOINT_AT_STEP {
            dwelling_a.step().expect("step before checkpoint");
        }
        let checkpoint = dwelling_a.save_checkpoint().expect("save checkpoint");

        let (_cp_dir, cp_path) = temp_toml("rng_ckpt");
        checkpoint.save(&cp_path).expect("write checkpoint file");
        let loaded_cp = DwellingCheckpoint::load(&cp_path).expect("load checkpoint");

        let mut dwelling_b = Dwelling::from_toml_config(&toml_path_a).expect("build dwelling B");
        dwelling_b
            .load_checkpoint(loaded_cp)
            .expect("restore checkpoint");

        let mut restarted_steps = Vec::new();
        while let Ok(step) = dwelling_b.step() {
            restarted_steps.push(step);
        }

        let ref_tail = &ref_results.steps[CHECKPOINT_AT_STEP as usize..];
        let compare_len = ref_tail.len().min(restarted_steps.len());

        assert!(
            compare_len > 0,
            "no steps to compare after checkpoint restart"
        );

        for i in 0..compare_len {
            let ref_step = &ref_tail[i];
            let res_step = &restarted_steps[i];
            assert_eq!(
                ref_step.timestamp, res_step.timestamp,
                "step {i}: timestamps must match"
            );
            assert!(
                (ref_step.net_electric_power_kw - res_step.net_electric_power_kw).abs()
                    < f64::EPSILON,
                "step {i}: net_electric_power_kw mismatch"
            );
            assert!(
                (ref_step.hvac_heating_w - res_step.hvac_heating_w).abs() < f64::EPSILON,
                "step {i}: hvac_heating_w mismatch"
            );
            assert!(
                (ref_step.hvac_cooling_w - res_step.hvac_cooling_w).abs() < f64::EPSILON,
                "step {i}: hvac_cooling_w mismatch"
            );
            assert!(
                (ref_step.gas_power_w - res_step.gas_power_w).abs() < f64::EPSILON,
                "step {i}: gas_power_w mismatch"
            );
        }
    }

    // ── Telemetry-to-Column Mapping Tests ──

    /// The `in_schema` scoping of the `expected` predicate does not silence
    /// the drift check: for equipment the schema KNOWS (membership = the
    /// schema's unconditional `{name} Electric Power (kW)` column), a
    /// missing applicable column at its verbosity is a typed error in every
    /// build. Only schema-unknown equipment (added after the schema froze,
    /// carrying no columns by design) are exempt.
    #[test]
    fn build_equipment_column_map_errors_on_schema_drift_for_known_equipment() {
        let mut eq = TestEquipment::new("Battery", ControlCapabilities::empty());
        eq.descriptor.end_use = EndUse::BATTERY;
        eq.descriptor.fuel = FuelType::Electric;
        let equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        // A schema-known equipment: the index carries its membership column
        // (Electric Power, emitted unconditionally at verbosity ≥ 1) and its
        // Mode column, but not the SOC column its name class demands at
        // verbosity 3 — drift for a known equipment must stay loud.
        let mut column_index = HashMap::new();
        column_index.insert("Battery Electric Power (kW)".to_string(), 0usize);
        column_index.insert("Battery Mode (-)".to_string(), 1usize);
        let err = build_equipment_column_map(&equipment_refs(&equipment), &column_index, 3)
            .expect_err("schema drift for a known equipment must be a typed error");
        assert!(
            matches!(&err, HaresError::InvariantViolation { check_name, .. }
                if check_name.contains("Battery SOC (-)")
                    && check_name.contains("Battery")),
            "the error must name the missing column and the equipment, got: {err:?}"
        );
    }

    /// An equipment whose name shadows a reserved aggregate column ("Total")
    /// must claim NOTHING from a column index that carries the aggregate —
    /// not even the column that exists under its own name. Claiming it would
    /// make record_step's later per-equipment write silently replace the
    /// dwelling's total; and treating the aggregate's presence as membership
    /// would panic on the absent Mode column. Both faces are pinned here:
    /// no claim, no panic.
    #[test]
    fn column_map_claims_nothing_for_an_equipment_name_shadowing_the_aggregate_column() {
        let eq = TestEquipment::new("Total", ControlCapabilities::empty());
        let equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        // The index carries only the level-0 aggregate — the exact state a
        // frozen schema presents to a mid-run equipment named "Total".
        let mut column_index = HashMap::new();
        column_index.insert("Total Electric Power (kW)".to_string(), 0usize);
        let col_map = build_equipment_column_map(&equipment_refs(&equipment), &column_index, 3)
            .expect("reserved-aggregate shadowing must not error");
        let cols = &col_map[0];
        assert!(
            cols.electric_power.is_none(),
            "an equipment named 'Total' must not claim the aggregate \
             'Total Electric Power (kW)' column as its own"
        );
        assert!(
            cols.mode.is_none(),
            "the aggregate's presence must not be read as schema membership — \
             the absent Mode column must not panic"
        );
    }

    /// At verbosity 7, all per-equipment column indices for applicable equipment
    /// types are resolved to `Some`.
    #[test]
    fn build_equipment_column_map_v7_all_columns_resolved_for_applicable_equipment() {
        let specs = vec![
            hares_io::EquipmentSpec {
                instance_name: None,
                name: "ASHP Heater".to_string(),
                fuel_type: FuelType::Electric,
                parameters: Map::new(),
                zip_params: None,
                typed_config: None,
                system_id: None,
                related_hvac_idref: None,
                primary_role: None,
            },
            hares_io::EquipmentSpec {
                instance_name: None,
                name: "Battery".to_string(),
                fuel_type: FuelType::Electric,
                parameters: Map::new(),
                zip_params: None,
                typed_config: None,
                system_id: None,
                related_hvac_idref: None,
                primary_role: None,
            },
        ];
        let schema = hares_io::build_schema(&specs, 7, &[]);
        let column_index = build_output_column_index(&schema);

        let mut eq1 = TestEquipment::new("ASHP Heater", ControlCapabilities::empty());
        eq1.descriptor.end_use = EndUse::HVAC_HEATING;
        eq1.descriptor.fuel = FuelType::Electric;
        let mut eq2 = TestEquipment::new("Battery", ControlCapabilities::empty());
        eq2.descriptor.end_use = EndUse::BATTERY;
        eq2.descriptor.fuel = FuelType::Electric;
        let equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq1), Box::new(eq2)];
        let col_map = build_equipment_column_map(&equipment_refs(&equipment), &column_index, 7)
            .expect("column map resolves");

        let ashp = &col_map[0];
        let bat = &col_map[1];

        assert!(ashp.electric_power.is_some());
        assert!(ashp.mode.is_some());
        assert!(ashp.setpoint.is_some());
        assert!(ashp.capacity.is_some());
        assert!(ashp.cop.is_some());
        assert!(ashp.energy_kwh.is_some());
        assert!(ashp.schedule.is_some());
        assert!(ashp.defrost_state.is_some());
        assert!(ashp.er_power.is_some());
        assert!(ashp.speed.is_some());
        assert!(ashp.fan_power.is_some());
        assert!(ashp.main_power.is_some());
        assert!(ashp.runtime_fraction.is_some());
        assert!(ashp.shr.is_none());
        assert!(ashp.latent_gains.is_none());

        assert!(bat.electric_power.is_some());
        assert!(bat.soc.is_some());
        assert!(bat.energy_kwh.is_some());
        assert!(bat.mode.is_some());
        assert!(bat.setpoint.is_none());
        assert!(bat.capacity.is_none());
        assert!(bat.cop.is_none());
    }

    /// For every `FuelType` variant, the schema emits a
    /// `"{name} Gas Power (therms/hour)"` column if and only if
    /// `build_equipment_column_map` resolves it. Both sites share
    /// `hares_io::fuel_reports_gas_power_column`; this test guards against the
    /// predicate being forked again (previously Wood/Coal/WoodPellet equipment
    /// got a schema column that was never populated).
    #[test]
    fn gas_power_schema_column_and_resolved_column_agree_for_every_fuel_type() {
        let all_fuels = hares_types::ports::ALL_FUEL_TYPES
            .iter()
            .copied()
            .chain(std::iter::once(FuelType::None));
        for fuel in all_fuels {
            let name = "Test Load";
            let specs = vec![hares_io::EquipmentSpec {
                instance_name: None,
                name: name.to_string(),
                fuel_type: fuel,
                parameters: Map::new(),
                zip_params: None,
                typed_config: None,
                system_id: None,
                related_hvac_idref: None,
                primary_role: None,
            }];
            let schema = hares_io::build_schema(&specs, 1, &[]);
            let column_index = build_output_column_index(&schema);
            let schema_has_gas_col =
                column_index.contains_key(&format!("{name} {GAS_POWER_SUFFIX}"));

            let mut eq = TestEquipment::new(name, ControlCapabilities::empty());
            eq.descriptor.fuel = fuel;
            let equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
            let col_map = build_equipment_column_map(&equipment_refs(&equipment), &column_index, 1)
                .expect("column map resolves");
            let resolved_gas_col = col_map[0].gas_power.is_some();

            assert_eq!(
                schema_has_gas_col, resolved_gas_col,
                "fuel {fuel:?}: schema column presence ({schema_has_gas_col}) must match \
                 resolved column presence ({resolved_gas_col})"
            );
            assert_eq!(
                schema_has_gas_col,
                hares_io::fuel_reports_gas_power_column(fuel),
                "fuel {fuel:?}: schema must follow the shared predicate"
            );
        }
    }

    /// At verbosity 0, per-equipment columns are not in the schema.
    #[test]
    fn build_equipment_column_map_v0_all_per_equipment_columns_none() {
        let schema = hares_io::build_schema(&[], 0, &[]);
        let column_index = build_output_column_index(&schema);
        let eq = TestEquipment::new("ASHP Heater", ControlCapabilities::empty());
        let equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        let col_map = build_equipment_column_map(&equipment_refs(&equipment), &column_index, 0)
            .expect("column map resolves");
        let cols = &col_map[0];
        assert!(cols.electric_power.is_none());
        assert!(cols.gas_power.is_none());
        assert!(cols.mode.is_none());
        assert!(cols.soc.is_none());
        assert!(cols.capacity.is_none());
        assert!(cols.energy_kwh.is_none());
        assert!(cols.schedule.is_none());
        assert!(cols.defrost_state.is_none());
        assert!(cols.duct_losses.is_none());
    }

    /// Gas-fueled equipment with electric parasitics (gas furnace blower at
    /// pf 0.87, gas boiler pump at 0.84, gas WH draft fan at 0.87) emits
    /// reactive power, so at verbosity ≥ 5 the Reactive Power / Power Factor
    /// columns must exist and resolve for gas equipment too — the columns
    /// mirror the unconditional Electric Power column instead of gating on
    /// fuel type.
    #[test]
    fn gas_equipment_reactive_and_power_factor_columns_resolve_at_verbosity_5() {
        let specs = vec![hares_io::EquipmentSpec {
            instance_name: None,
            name: "Gas Furnace".to_string(),
            fuel_type: FuelType::Gas,
            parameters: Map::new(),
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        }];
        let schema = hares_io::build_schema(&specs, 5, &[]);
        let column_index = build_output_column_index(&schema);
        let mut eq = TestEquipment::new("Gas Furnace", ControlCapabilities::empty());
        eq.descriptor.end_use = EndUse::HVAC_HEATING;
        eq.descriptor.fuel = FuelType::Gas;
        let equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        let col_map = build_equipment_column_map(&equipment_refs(&equipment), &column_index, 5)
            .expect("column map resolves");
        let cols = &col_map[0];
        assert!(
            cols.reactive_power.is_some(),
            "gas furnace blower emits Q — reactive column must resolve"
        );
        assert!(
            cols.power_factor.is_some(),
            "gas furnace blower emits Q — power factor column must resolve"
        );
        // Below verbosity 5 the columns are absent for all equipment.
        let schema_v4 = hares_io::build_schema(&specs, 4, &[]);
        let column_index_v4 = build_output_column_index(&schema_v4);
        let mut eq_v4 = TestEquipment::new("Gas Furnace", ControlCapabilities::empty());
        eq_v4.descriptor.end_use = EndUse::HVAC_HEATING;
        eq_v4.descriptor.fuel = FuelType::Gas;
        let equipment_v4: Vec<Box<dyn Equipment>> = vec![Box::new(eq_v4)];
        let col_map_v4 =
            build_equipment_column_map(&equipment_refs(&equipment_v4), &column_index_v4, 4)
                .expect("column map resolves");
        assert!(col_map_v4[0].reactive_power.is_none());
        assert!(col_map_v4[0].power_factor.is_none());
    }

    /// A typo in a column name string results in `None` index.
    #[test]
    fn column_name_typo_results_in_none_column_index() {
        let specs = vec![hares_io::EquipmentSpec {
            instance_name: None,
            name: "ASHP Heater".to_string(),
            fuel_type: FuelType::Electric,
            parameters: Map::new(),
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        }];
        let schema = hares_io::build_schema(&specs, 1, &[]);
        let column_index = build_output_column_index(&schema);
        assert!(column_index.contains_key("ASHP Heater Electric Power (kW)"));
        assert!(!column_index.contains_key("ASHP Heater Electrix Power (kW)"));
    }

    /// Column suffix constants produce names matching the schema.
    #[test]
    fn column_suffix_constants_match_build_schema_output() {
        let specs = vec![hares_io::EquipmentSpec {
            instance_name: None,
            name: "Air Conditioner".to_string(),
            fuel_type: FuelType::Electric,
            parameters: Map::new(),
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        }];
        let schema = hares_io::build_schema(&specs, 7, &[]);
        let column_index = build_output_column_index(&schema);
        let name = "Air Conditioner";
        assert!(column_index.contains_key(&format!("{name} {ELECTRIC_POWER_SUFFIX}")));
        assert!(column_index.contains_key(&format!("{name} {MODE_SUFFIX}")));
        assert!(column_index.contains_key(&format!("{name} {SETPOINT_SUFFIX}")));
        assert!(column_index.contains_key(&format!("{name} {CAPACITY_SUFFIX}")));
        assert!(column_index.contains_key(&format!("{name} {COP_SUFFIX}")));
        assert!(column_index.contains_key(&format!("{name} {SHR_SUFFIX}")));
        assert!(column_index.contains_key(&format!("{name} {LATENT_GAINS_SUFFIX}")));
        assert!(column_index.contains_key(&format!("{name} {SPEED_SUFFIX}")));
        assert!(column_index.contains_key(&format!("{name} {FAN_POWER_SUFFIX}")));
        assert!(column_index.contains_key(&format!("{name} {MAIN_POWER_SUFFIX}")));
        assert!(column_index.contains_key(&format!("{name} {RUNTIME_FRACTION_SUFFIX}")));
    }

    /// `HVAC_DUCT_LOSSES_COL` constant resolves in the schema at v5+.
    #[test]
    fn duct_losses_column_uses_shared_constant() {
        let schema_v5 = hares_io::build_schema(&[], 5, &[]);
        let col_idx_v5 = build_output_column_index(&schema_v5);
        assert!(col_idx_v5.contains_key(HVAC_DUCT_LOSSES_COL));
        let schema_v4 = hares_io::build_schema(&[], 4, &[]);
        let col_idx_v4 = build_output_column_index(&schema_v4);
        assert!(!col_idx_v4.contains_key(HVAC_DUCT_LOSSES_COL));
    }

    /// Every output-scoped telemetry key in `OUTPUT_SCOPE_KEYS` has a
    /// corresponding column mapping in the output schema when the
    /// relevant equipment type is present at max verbosity (8).
    ///
    /// This is the integration-level counterpart to the unit tests in
    /// `telemetry_keys.rs` — those verify every key constant has a scope
    /// annotation; this one verifies the scope annotations are backed
    /// by actual column definitions.
    #[test]
    fn all_output_scope_keys_have_column_mappings_at_max_verbosity() {
        use hares_types::telemetry_keys::{self as tk};
        let specs: Vec<hares_io::EquipmentSpec> = vec![
            // HVAC with compressor (heat pump)
            hares_io::EquipmentSpec {
                instance_name: Some("ASHP Heater".to_string()),
                name: "ASHP Heater".to_string(),
                fuel_type: FuelType::Electric,
                parameters: serde_json::Map::new(),
                zip_params: None,
                typed_config: None,
                system_id: None,
                related_hvac_idref: None,
                primary_role: None,
            },
            // PV
            hares_io::EquipmentSpec {
                instance_name: Some("PV".to_string()),
                name: "PV".to_string(),
                fuel_type: FuelType::Electric,
                parameters: serde_json::Map::new(),
                zip_params: None,
                typed_config: None,
                system_id: None,
                related_hvac_idref: None,
                primary_role: None,
            },
            // EV
            hares_io::EquipmentSpec {
                instance_name: Some("EV".to_string()),
                name: "EV".to_string(),
                fuel_type: FuelType::Electric,
                parameters: serde_json::Map::new(),
                zip_params: None,
                typed_config: None,
                system_id: None,
                related_hvac_idref: None,
                primary_role: None,
            },
        ];
        let schema = hares_io::build_schema(&specs, 8, &[]);
        let column_names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();

        // Build the expected column set from the schema and the
        // equipment column map logic. We can't call build_equipment_column_map
        // directly because it requires Box<dyn Equipment> instances, so we
        // verify against the column names in the schema instead.
        //
        // For each output-scoped key, check that the schema contains at
        // least one column whose suffix matches the key's column suffix.
        // The schema should have per-equipment columns for HVAC, PV, and EV.

        // Per-HVAC columns (including heat pump and compressor):
        assert!(
            column_names.contains(&"ASHP Heater Supply Temperature (C)"),
            "missing HVAC supply temp column; got: {column_names:?}"
        );
        assert!(
            column_names.contains(&"ASHP Heater Return Temperature (C)"),
            "missing HVAC return temp column"
        );
        assert!(
            column_names.contains(&"ASHP Heater Compressor Power (W)"),
            "missing compressor power (W) column"
        );
        assert!(
            column_names.contains(&"ASHP Heater Compressor Power (kW)"),
            "missing compressor power (kW) column"
        );
        assert!(
            column_names.contains(&"ASHP Heater Fan Electric Power (W)"),
            "missing fan electric power column"
        );
        assert!(
            column_names.contains(&"ASHP Heater Fan Power (W)"),
            "missing fan power (W) column"
        );
        assert!(
            column_names.contains(&"ASHP Heater Supply Air Temperature (C)"),
            "missing HP supply air temp column"
        );
        assert!(
            column_names.contains(&"ASHP Heater Pan Heater Power (kW)"),
            "missing pan heater power column"
        );
        assert!(
            column_names.contains(&"ASHP Heater Heat Pump Capacity (W)"),
            "missing HP capacity column"
        );
        assert!(
            column_names.contains(&"ASHP Heater ER Capacity (W)"),
            "missing ER capacity column"
        );

        // Per-PV columns:
        assert!(
            column_names.contains(&"PV DC Power (kW)"),
            "missing PV DC power column"
        );
        assert!(
            column_names.contains(&"PV Irradiance (W/m2)"),
            "missing PV irradiance column"
        );

        // Per-EV columns:
        assert!(
            column_names.contains(&"EV Connection State (-)"),
            "missing EV connection state column"
        );
        assert!(
            column_names.contains(&"EV Charging Level (-)"),
            "missing EV charging level column"
        );

        // Setpoint chain columns (dwelling-level, at v7):
        assert!(
            column_names.contains(&"Scheduled Heating Setpoint (C)"),
            "missing scheduled heating setpoint column"
        );
        assert!(
            column_names.contains(&"Scheduled Cooling Setpoint (C)"),
            "missing scheduled cooling setpoint column"
        );
        assert!(
            column_names.contains(&"Runtime Heating Setpoint (C)"),
            "missing runtime heating setpoint column"
        );
        assert!(
            column_names.contains(&"Runtime Cooling Setpoint (C)"),
            "missing runtime cooling setpoint column"
        );

        // Thermostat short-cycle protection columns (per-equipment):
        assert!(
            column_names.contains(&"ASHP Heater Min On Time (s)"),
            "missing min on time column"
        );
        assert!(
            column_names.contains(&"ASHP Heater Min Off Time (s)"),
            "missing min off time column"
        );

        // Verify all 27 output-scope keys are accounted for.
        // 7 legacy (FAN_KW, SHR, DEFROST_CYCLE_STATE, BACKUP_ER_KW,
        //   RUNTIME_FRACTION, LATENT_GAINS_W, DUCT_LOSS_W)
        // + 4 setpoint chain
        // + 3 HVAC temps
        // + 4 compressor/fan detail
        // + 3 HP detail
        // + 2 PV
        // + 2 EV
        // + 2 thermostat short-cycle protection
        // = 27 total keys
        //
        // 7 legacy output keys are verified by existing tests
        // (verbosity_7 tests for fan, SHR, defrost, ER, RTF, latent, duct).
        // The 20 new keys are verified above.
        assert_eq!(
            tk::OUTPUT_SCOPE_KEYS.len(),
            27,
            "OUTPUT_SCOPE_KEYS length changed; ensure all are tested above"
        );
    }

    #[test]
    fn enrich_schema_telemetry_units_tags_matching_column_with_unit_source() {
        let mut eq = TestEquipment::new("TestEq", ControlCapabilities::empty());
        eq.descriptor.telemetry_fields.push(TelemetryField {
            name: "test_reactive".to_string(),
            unit: "kVAR".to_string(),
            description: "test reactive power".to_string(),
        });
        let equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];

        let specs = vec![hares_io::EquipmentSpec {
            instance_name: Some("TestEq".to_string()),
            name: "TestEq".to_string(),
            fuel_type: FuelType::Electric,
            parameters: Map::new(),
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        }];
        let schema = hares_io::build_schema(&specs, 5, &[]);
        let enriched = enrich_schema_with_telemetry_units(schema, &equipment_refs(&equipment));
        let reactive_col = enriched
            .fields()
            .iter()
            .find(|f| f.name() == "TestEq Reactive Power (kVAR)")
            .expect("schema v5 must include per-equipment reactive power column");
        assert_eq!(
            reactive_col
                .metadata()
                .get("unit_source")
                .map(|s| s.as_str()),
            Some("telemetry_field"),
            "reactive power column with matching declared telemetry field unit should carry unit_source"
        );
    }

    #[test]
    fn enrich_schema_telemetry_units_sets_no_unit_source_when_unit_absent_from_descriptor() {
        let mut eq = TestEquipment::new("TestEq", ControlCapabilities::empty());
        eq.descriptor.telemetry_fields.push(TelemetryField {
            name: "test_kw".to_string(),
            unit: "kW".to_string(),
            description: "test active power".to_string(),
        });
        let equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];

        let specs = vec![hares_io::EquipmentSpec {
            instance_name: Some("TestEq".to_string()),
            name: "TestEq".to_string(),
            fuel_type: FuelType::Electric,
            parameters: Map::new(),
            zip_params: None,
            typed_config: None,
            system_id: None,
            related_hvac_idref: None,
            primary_role: None,
        }];
        let schema = hares_io::build_schema(&specs, 5, &[]);
        let enriched = enrich_schema_with_telemetry_units(schema, &equipment_refs(&equipment));
        let reactive_col = enriched
            .fields()
            .iter()
            .find(|f| f.name() == "TestEq Reactive Power (kVAR)")
            .expect("schema v5 must include per-equipment reactive power column");
        assert!(
            reactive_col.metadata().get("unit_source").is_none(),
            "reactive power column should not get unit_source when equipment declares no kVAR field"
        );
    }

    /// Interest-filtering state must stay per-equipment: `prev_equipment_modes`
    /// is `EquipmentId`-keyed and `equipment_mode_changed` attributes a mode
    /// change to the actor's target equipment by comparing against this map.
    /// Under the pre-I-06 identity collapse every equipment shared id 0, the
    /// map collapsed to a single entry, and every actor's target was compared
    /// against the *wrong* equipment's previous mode (spurious or missed
    /// wakeups). Built through the real assembly path (`from_toml_config` →
    /// `build_from_blueprint` → the id-injection pass), not hand-injected ids.
    #[test]
    fn prev_equipment_modes_has_one_entry_per_equipment_after_step() {
        let dir = tempfile::tempdir().expect("temp dir");
        let toml_path = dir.path().join("modes.toml");
        fs::write(
            &toml_path,
            r#"building_id = 998
[simulation]
start_time = "2024-01-15T00:00:00Z"
time_res_s = 60
duration_s = 120
[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0
[materials]
wall_r_value_m2_k_w = 2.8
[hvac]
equipment_name = "Furnace"
fuel = "electricity"
heating_capacity_kbtu_h = 30.0
[weather]
outdoor_temp_c = -10.0
dew_point_c = -5.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325
[schedule]
occupancy = 1.0
[event_load]
active_power_kw = 1.5
event_probability = 1.0
[output]
write_output = false
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
master_seed = 0
"#,
        )
        .expect("write synthetic TOML");
        let mut dwelling = Dwelling::from_toml_config(&toml_path).expect("build dwelling");

        assert!(
            dwelling.equipment.len() >= 2,
            "precondition: the synthetic dwelling must assemble multiple equipment \
             for the map-collapse check to bite, got {}",
            dwelling.equipment.len()
        );
        dwelling.step().expect("first step succeeds");
        assert_eq!(
            dwelling.prev_equipment_modes.len(),
            dwelling.latest_env.equipment_core.len(),
            "the mode-change map is rebuilt from equipment_core each step — it must \
             carry one entry per stepped equipment, not collapse onto shared ids"
        );
        assert_eq!(
            dwelling.prev_equipment_modes.len(),
            dwelling.equipment.len(),
            "every equipment in the vector must be individually keyed in \
             prev_equipment_modes so mode changes attribute to the right equipment"
        );
    }

    #[test]
    fn add_equipment_rejects_direct_lut_mutation_via_trait_reference() {
        let dir = tempfile::tempdir().expect("temp dir");
        let toml_path = dir.path().join("lut-guard.toml");
        fs::write(
            &toml_path,
            r#"building_id = 999
[simulation]
start_time = "2024-01-15T00:00:00Z"
time_res_s = 60
duration_s = 120
[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0
[materials]
wall_r_value_m2_k_w = 2.8
[hvac]
equipment_name = "Furnace"
fuel = "electricity"
heating_capacity_kbtu_h = 30.0
[weather]
outdoor_temp_c = -10.0
dew_point_c = -5.0
rel_humidity_pct = 50.0
pressure_kpa = 101.325
[schedule]
occupancy = 1.0
[output]
write_output = false
output_verbosity = 0
output_format = "csv"
output_chunk_size = 1000
master_seed = 0
"#,
        )
        .expect("write synthetic TOML");

        let mut dwelling = Dwelling::from_toml_config(&toml_path).expect("build dwelling");

        let eq = Box::new(TestEquipment::new(
            "GuardTest",
            ControlCapabilities::empty(),
        ));
        dwelling
            .add_equipment(eq)
            .expect("add_equipment should succeed");

        let eq_ref = dwelling
            .equipment
            .iter_mut()
            .find(|e| e.descriptor().name == "GuardTest")
            .expect("equipment should be in dwelling after add_equipment");

        let err = eq_ref
            .set_ocv_table(OcvTable::default_li_nmc())
            .unwrap_err();
        assert!(
            err.to_string().contains("already initialized"),
            "rejection must be an initialization guard error, got: {:?}",
            err
        );
    }
}
