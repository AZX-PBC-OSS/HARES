//! Dwelling orchestrator: integrates environment, equipment, solvers, and output.

mod autosize;
mod conversions;
mod loop_allocator;
mod solver_builder;
mod synthetic;

pub use conversions::{
    building_to_boundary_inputs, building_to_zone_inputs, mass_multiplier_for_zone, stage_rank,
};

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration as StdDuration;
#[cfg(any(feature = "profiling", feature = "actor_profiling"))]
use std::time::Instant;

use chrono::{DateTime, Duration, FixedOffset, Timelike};
use chrono_tz::Tz;
use hares_control::{
    DispatchRequest, DispatchTarget, PRIORITY_TIER_COUNT, PriceSignal, PriorityTier,
};
#[cfg(any(debug_assertions, feature = "observe_detailed"))]
use hares_envelope::EnvelopeDiagnostics;
use hares_envelope::{ElectricalSolver, FluidSolver, HumiditySolver, ThermalSolver};
use hares_equipment::{
    ActorSeed, BatteryLutType, Equipment, EquipmentRegistry, OcvTable, RegularGridInterpolator,
    SetpointReconciliation, UNegTable,
};
use hares_io::{
    Building, DefaultsStore, PvPanelDefaults, ScheduleTimeSeries, SimulationConfig,
    StreamingRecorder, WeatherTimeSeries, build_schema, end_use_electric_power_column,
    equipment_name_to_end_use, parse_hpxml, parse_schedule_csv, parse_weather, resolve_equipment,
    resolve_site_location,
};
use hares_physics::constants::{
    GAS_THERMS_PER_HOUR_TO_W, OCCUPANT_CONVECTIVE_FRACTION, OCCUPANT_LATENT_GAIN_W,
    OCCUPANT_RADIATIVE_FRACTION, OCCUPANT_SENSIBLE_GAIN_W, SECONDS_PER_HOUR,
};
use hares_physics::pv_sizing::RoofInfo;
use hares_physics::units::power_w_to_kw;
use hares_tariff::{BillingPeriodSummary, ElectricTariff, TariffEvaluator};
#[cfg(any(debug_assertions, feature = "check_invariants"))]
use hares_types::ControlCapabilities;
#[cfg(test)]
use hares_types::LoopId;
use hares_types::{
    ALL_FUEL_TYPES, BmsMode, ChargingStrategy, ControlSignal, DomainSolver, ElectricalSummary,
    EndUse, EnvironmentState, EquipmentId, ExecutionStage, GridState, HaresError, OperatingMode,
    PortContribution, PortDeclaration, PortSlots, SCHEDULE_DOMAIN_ID, ScheduleSource,
    ThermalCategory, ZoneId, ZoneMap, ZoneRole, telemetry_keys as tk, validate_core_contract,
    validate_fluid_type_consistency,
};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use serde_json::{Map, Value};

use crate::actors::{BatteryManagementActor, EvDriverActor, SolverFeedbackActor};
use crate::checkpoint::{CHECKPOINT_VERSION, DwellingCheckpoint};
use crate::diagnostics::{self, EnvelopeDiag};
use crate::environment::EnvironmentInitOptions;
#[cfg(any(debug_assertions, feature = "check_invariants"))]
use crate::invariants::InvariantChecker;
use crate::rng::{
    RNG_STREAM_EV_DRIVER_BASE, RNG_STREAM_EVENT_LOAD_BASE, advance_dwelling_rng, derive_sub_rng,
};
use crate::scheduler::{ActorSlot, ExecutionPhase, StepScheduler};
use crate::telemetry::DwellingTelemetry;
use crate::{Actor, ActorInterest, EnvironmentManager, SimClock, derive_dwelling_rng};

#[cfg(feature = "observe")]
use crate::diagnostics::DiagnosticAccumulator;
#[cfg(feature = "observe")]
use crate::observer::{
    DispatchCapture, DispatchedSignal, EquipmentObservation, MoistureInvariantCapture,
    MoistureZoneInvariant, ObserverBuffer, PhaseSnapshots, SameTierConflict, StepSnapshot,
};
#[cfg(feature = "observe")]
use crate::observer_capture;
use conversions::{
    apply_humidity_update_to_zones, apply_thermal_update_to_zones, build_output_column_index,
    chrono_to_std_duration, default_output_path, duration_to_u32_secs, equipment_config_from_spec,
    merged_equipment_config, required_datetime, required_duration, required_path,
    validate_sim_config,
};
use solver_builder::{build_default_solvers, compute_weather_averages};
use synthetic::{
    SyntheticTomlConfig, build_synthetic_building, build_synthetic_schedule,
    build_synthetic_weather,
};
#[cfg(feature = "profiling")]
use synthetic::{current_process_hwm_kb, hot_path_alloc_counter};

const DEFAULT_GRID_FREQUENCY_HZ: f64 = 60.0;

/// Minimum timestep resolution at which PV re-evaluation and BMS staleness
/// diagnostics have meaningful impact.  5 min matches the EnergyPlus minimum
/// `TimeStep` for sub-hourly simulation.
const PV_RE_EVAL_MIN_STEP_SECS: f64 = 300.0;

fn create_equipment_from_spec(
    registry: &EquipmentRegistry,
    spec: &hares_io::EquipmentSpec,
    rng_seed: Option<[u8; 32]>,
) -> Result<Box<dyn Equipment>> {
    let mut base_cfg = equipment_config_from_spec(spec);
    base_cfg.rng_seed = rng_seed;
    let name = base_cfg.name.clone();
    let ochre_class = base_cfg.ochre_class.clone();
    registry.create(&ochre_class, base_cfg).map_err(|err| {
        HaresError::Equipment(format!(
            "equipment '{}' (class '{}') create failed: {err}",
            name, ochre_class,
        ))
    })
}

/// Core result type for dwelling operations.
pub type Result<T> = std::result::Result<T, HaresError>;

/// Stable config contract between fleet runners and a single dwelling orchestrator.
#[derive(Debug, Clone)]
pub struct DwellingConfig {
    pub hpxml_path: PathBuf,
    pub schedule_path: PathBuf,
    pub weather_path: PathBuf,
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

/// Snapshot of accumulated port totals at a stage boundary.
#[cfg(debug_assertions)]
#[derive(Debug, Clone)]
struct StageSnapshot {
    // Why: field is written in debug_assertions builds to snapshot port
    // state at each stage boundary; not yet consumed by any assertion,
    // retained for future invariant checks once the verification path is
    // added.
    #[allow(dead_code)]
    ports: PortSlots,
}

#[cfg(feature = "profiling")]
#[derive(Debug, Clone, Default)]
pub struct DwellingProfilingSummary {
    pub envelope_solve: StdDuration,
    pub hvac: StdDuration,
    pub water_heater: StdDuration,
    pub schedule_load: StdDuration,
    pub io: StdDuration,
    pub other: StdDuration,
    pub memory_high_water_kb: u64,
    pub hot_path_alloc_violations: u64,
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
}

/// Build column index maps for each equipment piece using instance-qualified
/// names (matching `hares_io::output::columns::instance_qualified_names`).
fn build_equipment_column_map(
    equipment: &[Box<dyn Equipment>],
    column_index: &HashMap<String, usize>,
) -> Vec<EquipmentColumns> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for eq in equipment {
        *counts.entry(&eq.descriptor().name).or_default() += 1;
    }
    let mut indices: HashMap<&str, usize> = HashMap::new();
    equipment
        .iter()
        .map(|eq| {
            let base = &eq.descriptor().name;
            let name = if counts[base.as_str()] > 1 {
                let idx = indices.entry(base).or_insert(0);
                *idx += 1;
                format!("{base} #{idx}")
            } else {
                base.clone()
            };
            EquipmentColumns {
                electric_power: column_index
                    .get(&format!("{name} Electric Power (kW)"))
                    .copied(),
                gas_power: column_index
                    .get(&format!("{name} Gas Power (therms/hour)"))
                    .copied(),
                mode: column_index.get(&format!("{name} Mode (-)")).copied(),
                setpoint: column_index.get(&format!("{name} Setpoint (C)")).copied(),
                soc: column_index.get(&format!("{name} SOC (-)")).copied(),
                capacity: column_index.get(&format!("{name} Capacity (W)")).copied(),
                cop: column_index.get(&format!("{name} COP (-)")).copied(),
                reactive_power: column_index
                    .get(&format!("{name} Reactive Power (kVAR)"))
                    .copied(),
                power_factor: column_index
                    .get(&format!("{name} Power Factor (-)"))
                    .copied(),
                energy_kwh: column_index.get(&format!("{name} Energy (kWh)")).copied(),
                schedule: column_index.get(&format!("{name} Schedule (-)")).copied(),
                defrost_state: column_index
                    .get(&format!("{name} Defrost State (-)"))
                    .copied(),
                er_power: column_index.get(&format!("{name} ER Power (kW)")).copied(),
                shr: column_index.get(&format!("{name} SHR (-)")).copied(),
                speed: column_index.get(&format!("{name} Speed (-)")).copied(),
                fan_power: column_index.get(&format!("{name} Fan Power (kW)")).copied(),
                main_power: column_index
                    .get(&format!("{name} Main Power (kW)"))
                    .copied(),
                runtime_fraction: column_index
                    .get(&format!("{name} Runtime Fraction (-)"))
                    .copied(),
                latent_gains: column_index
                    .get(&format!("{name} Latent Gains (W)"))
                    .copied(),
                duct_losses: column_index.get("HVAC Duct Losses (W)").copied(),
            }
        })
        .collect()
}

fn extend_schema_with_actor_columns(
    schema: &arrow::datatypes::Schema,
    actors: &[Box<dyn Actor>],
) -> arrow::datatypes::Schema {
    use arrow::datatypes::{DataType, Field};
    let mut fields: Vec<Field> = schema.fields().iter().map(|f| f.as_ref().clone()).collect();
    for actor in actors {
        if let Some(tel) = actor.telemetry() {
            for key in tel.0.keys() {
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
struct ZoneColumnCaches {
    /// (ZoneId, column_index) for temperature columns, sorted by ZoneId.
    temp_columns: Vec<(ZoneId, usize)>,
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

    let mut temp_columns = Vec::new();
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
        if let Some(idx) = temp_col {
            temp_columns.push((zone_id, idx));
        }
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
        temp_columns,
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
/// Used by the same-tier conflict pre-scans in `dispatch_into_observed` and
/// the debug invariant block in `drain_tiers`, which operate on raw
/// pre-expansion queue entries where cross-variant calls are expected.
#[cfg(any(feature = "observe", debug_assertions, feature = "check_invariants"))]
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

    fn dispatch_into(&mut self, equipment: &mut [Box<dyn Equipment>], warnings: &mut Vec<String>) {
        self.drain_tiers(equipment, warnings, |_, _, _, _| {});
    }

    #[cfg(feature = "observe")]
    fn dispatch_into_observed(
        &mut self,
        equipment: &mut [Box<dyn Equipment>],
        warnings: &mut Vec<String>,
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
        warnings: &mut Vec<String>,
        mut on_signal: impl FnMut(&DispatchRequest, bool, bool, bool),
    ) {
        // NOTE: `seen_targets` is deliberately NOT cleared here. The per-step
        // ledger is reset by `begin_step()` exactly once per timestep so that
        // a lower-priority signal queued after a higher-priority one in an
        // earlier pass does not silently overwrite it.

        for (tier_idx, tier_que) in self.by_tier.iter_mut().enumerate() {
            // Same-tier conflict invariant check: warn when two or more
            // dispatch requests in the same tier target the same equipment.
            // This is conditional (log::warn!) because it reflects a real
            // design choice (last-write-wins) rather than a bug, but the
            // diagnostic helps users understand non-deterministic outcomes.
            #[cfg(any(debug_assertions, feature = "check_invariants"))]
            {
                let (head, tail) = tier_que.as_slices();
                let requests: Vec<&DispatchRequest> = head.iter().chain(tail.iter()).collect();
                for i in 0..requests.len() {
                    for j in (i + 1)..requests.len() {
                        if targets_conflict(&requests[i].target, &requests[j].target, equipment) {
                            let tier = PriorityTier::from_index(tier_idx);
                            let already_warned = (0..i).any(|k| {
                                targets_conflict(
                                    &requests[k].target,
                                    &requests[i].target,
                                    equipment,
                                )
                            });
                            if !already_warned {
                                let conflict_signals: Vec<&ControlSignal> = requests
                                    .iter()
                                    .filter(|r| {
                                        targets_conflict(&r.target, &requests[i].target, equipment)
                                    })
                                    .map(|r| &r.signal)
                                    .collect();
                                tracing::warn!(
                                    target = ?requests[i].target,
                                    tier = ?tier,
                                    signal_count = conflict_signals.len(),
                                    signals = ?conflict_signals,
                                    "same-tier conflict: multiple signals at the same priority tier target the same equipment; last-queued signal wins (FIFO / last-write-wins)"
                                );
                            }
                        }
                    }
                }
            }

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

                        let delivered = route_request(&request, equipment, warnings);
                        on_signal(&request, delivered, overwrote, false);
                    }
                    DispatchTarget::ByEndUse(end_use) => {
                        // Expand ByEndUse to individual ByName targets per
                        // matching equipment. This ensures seen_targets contains
                        // only homogeneous ByName entries, so conflicts_with
                        // comparisons are reliable across dispatch passes.
                        let mut any_delivered = false;
                        let mut any_overwrote = false;
                        let mut any_skipped = false;

                        for eq in equipment.iter_mut() {
                            if eq.descriptor().end_use != *end_use {
                                continue;
                            }
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
                                warnings.push(format!(
                                    "control apply failed for '{}' : {err}",
                                    eq_name
                                ));
                            } else {
                                any_delivered = true;
                            }
                        }

                        if !any_delivered && !any_skipped {
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

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
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
    warnings: &mut Vec<String>,
) -> bool {
    match &request.target {
        DispatchTarget::ByName(name) => {
            let delivered = apply_to_matching(equipment, &request.signal, warnings, |eq| {
                eq.descriptor().name.as_str() == &**name
            });
            if !delivered {
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
            let delivered = apply_to_matching(equipment, &request.signal, warnings, |eq| {
                eq.descriptor().end_use == *end_use
            });
            if !delivered {
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
    warnings: &mut Vec<String>,
    matches: impl Fn(&dyn Equipment) -> bool,
) -> bool {
    let mut delivered = false;
    for eq in equipment.iter_mut() {
        if matches(&**eq) {
            delivered = true;
            if let Err(err) = eq.apply_control(signal) {
                warnings.push(format!(
                    "control apply failed for '{}' : {err}",
                    eq.descriptor().name
                ));
            }
        }
    }
    delivered
}

fn compute_equipment_execution_order(equipment: &[Box<dyn Equipment>]) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..equipment.len()).collect();
    indices.sort_by_key(|&idx| stage_rank(equipment[idx].descriptor().stage));
    indices
}

fn compute_equipment_dispatch_targets(equipment: &[Box<dyn Equipment>]) -> Vec<DispatchTarget> {
    equipment
        .iter()
        .map(|eq| DispatchTarget::ByName(Arc::from(eq.descriptor().name.as_str())))
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
        if let Some(zone) = decl.zone {
            if !env_zone_ids.contains(&zone) {
                return Err(HaresError::Dwelling(format!(
                    "equipment declares port for zone {zone:?} \
                     which does not exist in the environment model"
                )));
            }
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
        if let Some(loop_id) = decl.loop_id {
            if !allocated_loop_ids.contains(&loop_id.0) {
                return Err(HaresError::Dwelling(format!(
                    "equipment declares fluid port for loop {loop_id:?} \
                     which has not been allocated to any equipment spec"
                )));
            }
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
    pub thermal_solver: ThermalSolver,
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
    pub warnings: Vec<String>,
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
    stage_snapshot: Option<StageSnapshot>,
    #[cfg(debug_assertions)]
    test_panic_on_step: bool,
    #[cfg(debug_assertions)]
    test_assert_panic_on_step: bool,
    /// Test-only: causes the next thermal invariant check in
    /// [`check_invariants`](Self::check_invariants) to receive deliberately
    /// broken balance terms, forcing `InvariantViolation { check_name: "thermal_balance" }`.
    #[cfg(debug_assertions)]
    test_thermal_invariant_failure: bool,
    /// Test-only: causes the HVAC delivered-energy non-negativity invariant
    /// checks in [`check_invariants`](Self::check_invariants) to receive a
    /// deliberately negative `hvac_heating_w` and `hvac_cooling_w`, forcing
    /// `NegativeDeliveredEnergy`.
    #[cfg(debug_assertions)]
    test_hvac_negative_energy_failure: bool,
    output_column_index: HashMap<String, usize>,
    /// Pre-resolved output column indices for each equipment piece, avoiding
    /// per-timestep name allocation in `record_step`.
    equipment_column_map: Vec<EquipmentColumns>,
    /// Pre-resolved aggregate end-use column index for each equipment's
    /// electric power contribution, or `None` if no aggregate column exists
    /// for that equipment's EndUse category.
    end_use_aggregate_indices: Vec<Option<usize>>,
    /// Pre-resolved actor telemetry column indices per actor, avoiding
    /// per-timestep `format!()` allocation in `record_step`.
    /// Each inner vec maps telemetry key → column index.
    /// Empty for actors returning `None` telemetry.
    actor_column_map: Vec<Vec<(String, usize)>>,
    /// Number of numeric columns expected by the recorder (schema fields minus timestamp).
    output_value_count: usize,
    /// Pre-allocated scratch buffer for `record_step`, reused each timestep.
    record_scratch: Vec<f64>,
    /// Pre-allocated buffer for RFC 3339 timestamp formatting, reused each step.
    timestamp_buf: String,
    /// Pre-resolved zone temperature column indices, sorted by ZoneId.
    zone_temp_columns: Vec<(ZoneId, usize)>,
    /// Pre-resolved per-zone infiltration column indices (non-indoor zones only).
    zone_infiltration_columns: HashMap<ZoneId, usize>,
    /// Pre-resolved per-zone interior LWR column indices.
    zone_lwr_columns: HashMap<ZoneId, usize>,
    /// Pre-resolved per-zone HVAC thermal attribution column indices.
    /// (heating_column, cooling_column) for each zone in the building.
    zone_hvac_columns: HashMap<ZoneId, (usize, usize)>,
    /// Pre-sorted zone ID order for StepResult, computed once at init.
    sorted_zone_ids: Vec<ZoneId>,
    /// For each entry in sorted_zone_ids, the index into EnvironmentState::zones.
    /// Enables O(1) temperature extraction in the hot loop.
    zone_env_indices: Vec<Option<usize>>,
    /// For each entry in sorted_zone_ids, the output column index for zone temperature,
    /// or None if no temperature column exists for that zone.
    zone_temp_col_indices: Vec<Option<usize>>,
    /// Pre-allocated buffer for zone temperatures in StepResult, reused each step.
    zone_temp_scratch: Vec<(ZoneId, f64)>,
    /// Schedule column index for the occupancy time series, or `None` if the
    /// schedule does not include an occupancy column.
    occupancy_column_idx: Option<usize>,
    /// Scale factor applied to the raw occupancy schedule fraction (0–1) to
    /// convert it to a person count.  Equals `number_of_occupants` from the
    /// Occupancy equipment spec (defaults to 1.0 when unspecified).
    occupancy_scale: f64,
    #[expect(dead_code, reason = "reserved for potential future use")]
    zone_capacitances_j_k: Vec<(ZoneId, f64)>,
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    prev_humidity_ratios: Vec<(ZoneId, f64)>,
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
    /// Pre-computed equipment execution order (sorted by stage rank).
    /// Computed once at init time, reused each timestep.
    equipment_execution_order: Vec<usize>,
    /// Numerical invariant checker, allocated once and reused each step.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    invariant_checker: InvariantChecker,
    /// Pre-allocated scratch buffer for conditioned zone temps in check_invariants.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    invariant_conditioned_temps: Vec<f64>,
    /// Pre-allocated scratch buffer for unconditioned zone temps in check_invariants.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    invariant_unconditioned_temps: Vec<f64>,
    /// Pre-allocated scratch buffer for tank node temps in check_invariants.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    invariant_tank_temps: Vec<f64>,
    /// Pre-computed tank node telemetry keys, avoiding format!() per step.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    tank_node_keys: Vec<String>,
    /// Pre-allocated scratch buffer for infiltration latent by zone in check_invariants.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    invariant_infiltration_latent: Vec<(ZoneId, f64)>,
    /// Pre-allocated scratch maps for semi-implicit infiltration coupling data.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    invariant_infiltration_m_dot: HashMap<ZoneId, f64>,
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    invariant_infiltration_w_outdoor: HashMap<ZoneId, f64>,
    /// Whether the dwelling is running a warm-up convergence loop.
    /// Moisture invariants are skipped during warm-up because the initial
    /// humidity ratio from the HPXML model can be far from the steady-state
    /// value, producing large initialization transients that false-positive
    /// the sorption bound check.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    is_warming_up: bool,
    /// Per-zone moisture invariant capture data from `check_moisture`.
    /// Populated by check_invariants; consumed by the observer push when
    /// both `check_invariants` (or debug_assertions) and `observe` are active.
    #[cfg(all(
        feature = "observe",
        any(debug_assertions, feature = "check_invariants")
    ))]
    invariant_moisture_capture: Vec<MoistureZoneInvariant>,
    /// Thermal consistency flag computed during `run_timestep` before ports
    /// are zeroed, then consumed by `telemetry()` to populate
    /// `telemetry_consistency_flag`.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    thermal_consistency_flag: bool,
    /// Set to `true` when `load_checkpoint` restores state. The first
    /// post-restore `run_timestep` asserts that `equipment_core` is populated
    /// for every equipment instance, then resets this flag to `false`.
    restored_from_checkpoint: bool,
    /// Per-zone conditioning status, aligned with `latest_env.zones` order.
    /// `true` = conditioned (HVAC-served), `false` = unconditioned (attic, garage, etc.).
    zone_is_conditioned: Vec<bool>,
    /// Output config retained for schema rebuilds when equipment changes.
    output_verbosity: u8,
    output_chunk_size: usize,
    output_format: hares_io::OutputFormat,
    output_path: PathBuf,
    write_output: bool,
    retain_batches: bool,
    /// Diagnostic CSV writer, opened when `output_verbosity >= 4`.
    diagnostic_writer: Option<std::io::BufWriter<std::fs::File>>,
    #[cfg(feature = "profiling")]
    profiling: DwellingProfilingSummary,
    #[cfg(feature = "actor_profiling")]
    per_actor_timing: Vec<(usize, StdDuration)>,
    #[cfg(feature = "actor_profiling")]
    actor_name_cache: Vec<String>,
    #[cfg(feature = "observe")]
    observer_buf: Option<ObserverBuffer>,
    /// Number of equipment whose port contributions were rolled back this
    /// timestep following a failed `step()` call.
    #[cfg(feature = "observe")]
    rolled_back_port_equipment: usize,
    /// Number of zone temperature NaN values detected this timestep by the
    /// always-on invariant check. Incremented per-zone when any zone temperature
    /// is non-finite; resets to 0 each step.
    #[cfg(feature = "observe")]
    nan_temperature_count: usize,
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

impl Dwelling {
    /// Builds a dwelling from HPXML + schedule/weather paths and simulation config.
    pub fn from_config(config: DwellingConfig) -> Result<Self> {
        let building = parse_hpxml(&config.hpxml_path)
            .map_err(|err| HaresError::Io(format!("HPXML parse failed: {err}")))?;

        let weather = parse_weather(&config.weather_path)
            .map_err(|err| HaresError::Io(format!("weather parse failed: {err}")))?;

        let schedule_raw = if config.schedule_path.exists() {
            parse_schedule_csv(&config.schedule_path, &[], Some(&weather.meta), None)
                .map_err(|err| HaresError::Io(format!("schedule parse failed: {err}")))?
        } else {
            hares_io::hpxml_schedule::generate_schedule_from_hpxml(
                &building,
                config.sim_config.start_time,
                config.sim_config.duration,
                config.sim_config.time_res,
                config.defaults_path.as_deref(),
            )
        };

        let target_step_secs = duration_to_u32_secs(config.sim_config.time_res)?;
        let schedule = schedule_raw
            .resample(target_step_secs)
            .map_err(|err| HaresError::Io(format!("schedule resample failed: {err}")))?;

        Self::from_preparsed(config, building, weather, schedule)
    }

    /// Builds a dwelling directly from ResStock-style input files.
    pub fn from_hpxml(
        hpxml_path: &Path,
        schedule_path: &Path,
        weather_path: &Path,
        start_time: DateTime<FixedOffset>,
        time_res: Duration,
        duration: Duration,
        overrides: Option<Value>,
    ) -> Result<Self> {
        let sim_config = SimulationConfig {
            start_time,
            duration,
            time_res,
            output_verbosity: 0,
            output_path: None,
            write_output: true,
            output_format: hares_io::OutputFormat::Csv,
            output_chunk_size: 10_000,
            setpoint_deadband_c: None,
            master_seed: 0,
            civil_timezone: None,
            site_location: hares_io::SiteLocationOverride::default(),
            retain_batches: false,
        };

        let config = DwellingConfig {
            hpxml_path: hpxml_path.to_path_buf(),
            schedule_path: schedule_path.to_path_buf(),
            weather_path: weather_path.to_path_buf(),
            defaults_path: None,
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
            schedule_path: path.to_path_buf(),
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
        mut building: Building,
        mut weather: WeatherTimeSeries,
        schedule: ScheduleTimeSeries,
    ) -> Result<Self> {
        // Resolve the authoritative site location ONCE from all available
        // sources (explicit override → HPXML → weather file), then make it the
        // single source of truth: written into both the weather metadata (which
        // drives solar position) and the building's Site (which drives autosize
        // and equipment placement). This guarantees solar geometry, weather
        // magnitudes, and the start-time UTC offset are all mutually
        // consistent. See `hares_io::site_location`.
        let site_location = resolve_site_location(
            &building.site,
            &weather.meta,
            &config.sim_config.site_location,
        );
        weather.meta.latitude = site_location.latitude_deg;
        weather.meta.longitude = site_location.longitude_deg;
        weather.meta.elevation_m = site_location.elevation_m;
        weather.meta.timezone_offset_h = site_location.utc_offset_h;
        weather.meta.has_embedded_location = true;
        building.site.latitude_deg = Some(site_location.latitude_deg);
        building.site.longitude_deg = Some(site_location.longitude_deg);
        building.site.elevation_m = Some(site_location.elevation_m);
        building.site.utc_offset_h = Some(site_location.utc_offset_h);

        // Reinterpret the user's start time in the site's resolved standard-time
        // zone. The naive wall-clock components (year, month, day, hour, minute,
        // second) are preserved — the user passes local wall-clock time — and
        // the offset is set to the resolved UTC offset so `solar_position`'s
        // internal `to_utc()` yields the correct sun geometry. DST, when
        // requested, is applied separately via `civil_timezone`.
        let tz_offset =
            chrono::FixedOffset::east_opt((site_location.utc_offset_h * 3600.0).round() as i32)
                .unwrap_or_else(|| chrono::FixedOffset::east_opt(0).expect("UTC offset"));
        let local_start = config
            .sim_config
            .start_time
            .naive_local()
            .and_local_timezone(tz_offset)
            .single()
            .unwrap_or_else(|| config.sim_config.start_time.with_timezone(&tz_offset));

        let init_chrono = config
            .initialization_duration
            .map(|d| Duration::seconds(d.as_secs() as i64))
            .unwrap_or(Duration::zero());

        #[cfg(feature = "dst")]
        let parsed_civil_tz: Option<chrono_tz::Tz> = config
            .sim_config
            .civil_timezone
            .as_deref()
            .map(|name| {
                name.parse::<chrono_tz::Tz>()
                    .map_err(|_| HaresError::Io(format!("invalid civil timezone: {name}")))
            })
            .transpose()?;

        let mut clock = SimClock::new(
            local_start,
            config.sim_config.time_res,
            config.sim_config.duration + init_chrono,
        );
        #[cfg(feature = "dst")]
        {
            clock.civil_tz = parsed_civil_tz;
        }

        let time_res = chrono_to_std_duration(config.sim_config.time_res)?;
        let weather_avgs = compute_weather_averages(&weather);
        let weather_design_conditions = weather.design_conditions;
        let rng = derive_dwelling_rng(config.sim_config.master_seed, config.bldg_id);
        let mut environment = EnvironmentManager::new_with_resample(
            weather,
            schedule,
            &building,
            time_res,
            local_start,
            EnvironmentInitOptions {
                civil_timezone: config.sim_config.civil_timezone.as_deref(),
                resample_overrides: config.resample_overrides.as_ref(),
                initial_rng: Some(rng.clone()),
                setpoint_deadband_c: config.sim_config.setpoint_deadband_c,
            },
        )
        .map_err(|err| HaresError::Io(format!("environment initialization failed: {err}")))?;

        let mut warnings = Vec::new();
        let defaults_dir = config
            .defaults_path
            .clone()
            .unwrap_or_else(|| PathBuf::from("defaults"));
        let resolved_defaults_dir = if defaults_dir.exists() {
            defaults_dir.clone()
        } else {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../defaults")
        };
        let mut defaults = match DefaultsStore::load(&resolved_defaults_dir) {
            Ok(store) => store,
            Err(err) => {
                warnings.push(format!(
                    "defaults load failed; using empty defaults store: {err}"
                ));
                DefaultsStore::empty()
            }
        };

        let empty_overrides = Value::Object(Map::new());

        let mut equipment_specs = resolve_equipment(
            &building,
            &defaults,
            &empty_overrides,
            config.patches.as_ref(),
        )
        .map_err(|e| HaresError::Io(e.to_string()))?;

        // Centralized fluid loop ID allocation — must run after wiring
        // (resolve_loop_wiring, inside resolve_equipment) and before
        // equipment construction so every instance receives a unique
        // loop ID above the wired range.
        loop_allocator::allocate_loop_ids(&mut equipment_specs);

        // Register PV surfaces with the environment so Perez irradiance is
        // computed for PV orientations (which may not match any envelope surface).
        register_pv_surfaces(&equipment_specs, &mut environment);

        // Auto-attach PV arrays to the closest matching roof surface and
        // register shading coverage on attached roofs.
        attach_pv_to_roofs(&mut equipment_specs, &building);
        register_pv_roof_shading(&equipment_specs, &building, &mut environment);

        let initial_env = environment.update(&clock, &[])?;

        let solvers = build_default_solvers(
            &initial_env,
            &config.sim_config,
            &building,
            &defaults,
            &weather_avgs,
            &equipment_specs,
        )?;

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
            let duct_params =
                match hares_io::hpxml::resolve_hvac::compute_duct_dse_params(&building) {
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
                design_conditions: weather_design_conditions,
                weather_lat: site_location.latitude_deg,
                weather_lon: site_location.longitude_deg,
                duct_params,
                internal_gains_w: 0.0,
                internal_gains_latent_w: 0.0,
            };

            crate::dwelling::autosize::autosize_equipment_capacities(
                &mut equipment_specs,
                &solvers.thermal,
                &ctx,
                &building,
                indoor_zone,
            );
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
                hares_io::hpxml::extract_bedroom_count(&building, config.patches.as_ref());
            crate::dwelling::autosize::autosize_water_heater_capacities(
                &mut equipment_specs,
                Some(n_bedrooms),
                initial_env.weather.mains_temp_c,
            );
        }

        // Enable ideal HVAC on the indoor zone when both heating AND cooling
        // setpoints are configured -- the thermal solver back-calculates the exact
        // load needed to maintain the setpoint at each timestep.
        hares_io::inject_schedule_into_specs(
            &mut equipment_specs,
            environment.schedule_mut(),
            Some(&resolved_defaults_dir),
        );

        // occupancy_column_idx must be resolved AFTER inject_schedule_into_specs,
        // which may generate an occupancy column from HPXML extension fractions or
        // the default schedule profile when the schedule CSV lacks one.
        let occupancy_column_idx = environment.occupancy_column_idx();

        // Gated invariant: ensure every HVAC spec has a setpoint source.
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            hares_io::check_hvac_setpoint_invariants(&equipment_specs);
            hares_io::check_foundation_zone_invariant(&building);
        }
        let override_root = config
            .overrides
            .clone()
            .unwrap_or_else(|| Value::Object(Map::new()));

        // Read number_of_occupants from the Occupancy spec to scale the raw
        // schedule fraction (0–1) into a person count for internal heat gains.
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        let has_occupancy_spec;
        let occupancy_scale = match equipment_specs.iter().find(|s| s.name == "Occupancy") {
            Some(spec) => {
                #[cfg(any(debug_assertions, feature = "check_invariants"))]
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
                #[cfg(any(debug_assertions, feature = "check_invariants"))]
                {
                    has_occupancy_spec = false;
                }
                1.0
            }
        };

        // Invariant: number_of_occupants must be non-negative regardless of
        // derivation path (HPXML NumberofResidents, derived from bedrooms, or default).
        // A negative value indicates a data error in the parser or input.
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
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
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
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
            for (idx, zone) in building.zones.iter().enumerate() {
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
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            for zone in &building.zones {
                if zone.zone_type == hares_io::hpxml::ZoneType::Attic {
                    tracing::debug!(
                        vented = zone.vented,
                        floor_area_m2 = zone.floor_area_m2,
                        "dwelling assembly: Attic zone vented status"
                    );
                }
            }
        }

        // Equipment names whose loads are handled outside the registry (e.g. directly in the
        // simulation loop) -- silently skip them rather than emitting a warning.
        const HANDLED_OUTSIDE_REGISTRY: &[&str] = &["Occupancy"];

        let mut setpoints_reconciled_by_equipment: HashMap<
            String,
            Option<Vec<SetpointReconciliation>>,
        > = HashMap::new();

        let registry = EquipmentRegistry::new();
        let mut equipment: Vec<Box<dyn Equipment>> = Vec::new();
        let mut rng_event_stream_idx: u64 = 0;
        for spec in &equipment_specs {
            if HANDLED_OUTSIDE_REGISTRY.contains(&spec.name.as_str()) {
                continue;
            }
            let sub_rng = derive_sub_rng(&rng, RNG_STREAM_EVENT_LOAD_BASE + rng_event_stream_idx);
            rng_event_stream_idx += 1;
            let mut eq = create_equipment_from_spec(&registry, spec, Some(sub_rng.get_seed()))?;

            let mut merged_cfg = merged_equipment_config(spec, &override_root);
            setpoints_reconciled_by_equipment.insert(
                merged_cfg.name.clone(),
                merged_cfg.setpoints_reconciled.clone(),
            );
            merged_cfg.zone_map = Some(zone_map.clone());
            merged_cfg.rng_seed = Some(sub_rng.get_seed());
            match eq.init(&merged_cfg, &initial_env) {
                Ok(()) => equipment.push(eq),
                Err(err) => {
                    let end_use = eq.descriptor().end_use.clone();
                    let is_critical = end_use == EndUse::HVAC_HEATING
                        || end_use == EndUse::HVAC_COOLING
                        || end_use == EndUse::WATER_HEATING
                        || end_use == EndUse::EV
                        || end_use == EndUse::BATTERY
                        || end_use == EndUse::PV;
                    if is_critical {
                        return Err(HaresError::Equipment(format!(
                            "equipment '{}' init failed: {err}",
                            merged_cfg.name
                        )));
                    }
                    let msg = format!(
                        "equipment '{}' init failed, skipping: {err}",
                        merged_cfg.name
                    );
                    tracing::error!("{msg}");
                    warnings.push(msg);
                }
            }
        }
        let mut equipment_id_by_name = HashMap::with_capacity(equipment.len());
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
        }

        let mut declarations: Vec<PortDeclaration> = Vec::new();
        for eq in &equipment {
            declarations.extend_from_slice(eq.ports());
        }

        let env_zone_ids: HashSet<ZoneId> = initial_env.zones.iter().map(|z| z.id).collect();
        validate_equipment_zones(&declarations, &env_zone_ids)?;

        let mut allocated_loop_ids = loop_allocator::collect_allocated_loop_ids(&equipment_specs);
        // Sentinel loop IDs always valid — well-known addresses used by
        // equipment as sentinels when no typed config is available (0) or
        // for the shared DHW demand loop (u16::MAX - 1).
        allocated_loop_ids.insert(0); // LoopId::default() — fallback for raw-config equipment
        allocated_loop_ids.insert(hares_equipment::DHW_DEMAND_LOOP.0);
        validate_equipment_loops(&declarations, &allocated_loop_ids)?;

        let ports = PortSlots::from_declarations(&declarations);
        let rollback_ports = PortSlots::from_declarations(&declarations);

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            // Invariant: every thermal accumulator must have at least one
            // equipment declarant matching its zone. A thermal accumulator
            // with no equipment declarant indicates the env-zone safety-net
            // loop was reintroduced, weakening wire-to-slot safety.
            let declared_thermal_zones: HashSet<ZoneId> = declarations
                .iter()
                .filter(|d| d.port_type == hares_types::PortType::Thermal)
                .filter_map(|d| d.zone)
                .collect();
            for acc in &ports.thermal {
                if !declared_thermal_zones.contains(&acc.zone) {
                    tracing::warn!(
                        zone = ?acc.zone,
                        "thermal accumulator created for zone with no equipment declarant"
                    );
                }
            }
        }

        // Reject equipment configurations where two pieces of equipment wired
        // to the same fluid loop_id declare different fluid types. This is a
        // configuration error that would silently produce incorrect simulation
        // results — the fluid solver groups by loop_id alone and uses the
        // first entry's fluid_type for all entries, discarding contributions
        // from the mismatched accumulator.
        validate_fluid_type_consistency(&declarations).map_err(|err| {
            HaresError::Dwelling(format!("fluid type consistency validation failed: {err}"))
        })?;

        let zone_types = environment.zone_types().to_vec();
        let indoor_zone = solvers.thermal.config().indoor_zone_id;
        let zone_names: Vec<(ZoneId, String)> = initial_env
            .zones
            .iter()
            .map(|z| {
                let zone_type = initial_env
                    .zones
                    .iter()
                    .position(|zt| zt.id == z.id)
                    .and_then(|idx| zone_types.get(idx));
                (z.id, zone_display_name(z.id, indoor_zone, zone_type))
            })
            .collect();
        let schema = build_schema(
            &equipment_specs,
            config.sim_config.output_verbosity,
            &zone_names,
        );
        let output_value_count = schema.fields().len() - 1; // exclude timestamp
        let output_column_index = build_output_column_index(&schema);
        let output_path = config
            .sim_config
            .output_path
            .clone()
            .unwrap_or_else(|| default_output_path(&config));
        let recorder = if config.sim_config.write_output {
            Some(
                StreamingRecorder::new(
                    schema,
                    config.sim_config.output_chunk_size,
                    config.sim_config.output_format,
                    &output_path,
                    config.sim_config.retain_batches,
                )
                .map_err(|err| HaresError::Io(format!("output recorder init failed: {err}")))?,
            )
        } else {
            None
        };

        let (roof_info, wall_azimuths) = hares_io::pv_sizing::extract_roof_info(&building);
        let latitude_deg = building.site.latitude_deg;
        let facility_type = building.residential_facility_type.clone();

        let equipment_column_map = build_equipment_column_map(&equipment, &output_column_index);
        let end_use_aggregate_indices: Vec<Option<usize>> = equipment_specs
            .iter()
            .map(|spec| {
                let end_use = equipment_name_to_end_use(&spec.name);
                let col_name = end_use_electric_power_column(&end_use);
                output_column_index.get(&col_name).copied()
            })
            .collect();
        let zone_types = environment.zone_types().to_vec();
        let zone_caches = build_zone_column_caches(
            &initial_env.zones,
            &zone_types,
            solvers.thermal.config().indoor_zone_id,
            &output_column_index,
        );
        let record_scratch = vec![0.0; output_value_count];
        let equipment_execution_order = compute_equipment_execution_order(&equipment);
        let mut solver_feedback_actor = SolverFeedbackActor::new();
        solver_feedback_actor.set_dispatch_targets(compute_equipment_dispatch_targets(&equipment));

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        let init_humidity_ratios: Vec<(ZoneId, f64)> = solvers
            .humidity
            .humidity_ratios
            .iter()
            .map(|(&z, &w)| (z, w))
            .collect();

        let mut dwelling = Self {
            bldg_id: config.bldg_id,
            failed: false,
            restored_from_checkpoint: false,
            #[cfg(debug_assertions)]
            test_panic_on_step: false,
            #[cfg(debug_assertions)]
            test_assert_panic_on_step: false,
            #[cfg(debug_assertions)]
            test_thermal_invariant_failure: false,
            test_hvac_negative_energy_failure: false,
            equipment,
            equipment_id_by_name,
            thermal_solver: solvers.thermal,
            humidity_solver: solvers.humidity,
            electrical_solver: solvers.electrical,
            fluid_solver: solvers.fluid,
            clock: clock.clone(),
            environment,
            ports,
            rollback_ports,
            recorder,
            rng,
            warnings,
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
            #[cfg(debug_assertions)]
            stage_snapshot: None,
            #[cfg(any(debug_assertions, feature = "check_invariants"))]
            thermal_consistency_flag: true,
            equipment_column_map,
            end_use_aggregate_indices,
            output_column_index,
            output_value_count,
            record_scratch,
            timestamp_buf: String::with_capacity(32),
            zone_temp_columns: zone_caches.temp_columns,
            zone_infiltration_columns: zone_caches.infiltration_columns,
            zone_lwr_columns: zone_caches.lwr_columns,
            zone_hvac_columns: zone_caches.hvac_columns,
            zone_temp_scratch: zone_caches
                .sorted_zone_ids
                .iter()
                .map(|&z| (z, 0.0))
                .collect(),
            sorted_zone_ids: zone_caches.sorted_zone_ids,
            zone_env_indices: zone_caches.zone_env_indices,
            zone_temp_col_indices: zone_caches.zone_temp_col_indices,
            occupancy_column_idx,
            occupancy_scale,
            zone_capacitances_j_k: solvers.zone_capacitances_j_k,
            #[cfg(any(debug_assertions, feature = "check_invariants"))]
            prev_humidity_ratios: init_humidity_ratios,
            actors: Vec::new(),
            scheduler: StepScheduler::default(),
            actor_column_map: Vec::new(),
            auto_registered_actor_names: HashSet::new(),
            actor_dispatch_buf: Vec::with_capacity(16),
            solver_feedback_actor,
            prev_zone_temps: HashMap::new(),
            prior_zone_temps: HashMap::new(),
            prev_price_signal: PriceSignal::default(),
            prev_equipment_modes: HashMap::new(),
            equipment_execution_order,
            #[cfg(any(debug_assertions, feature = "check_invariants"))]
            invariant_checker: InvariantChecker::new(),
            #[cfg(any(debug_assertions, feature = "check_invariants"))]
            invariant_conditioned_temps: Vec::with_capacity(building.zones.len()),
            #[cfg(any(debug_assertions, feature = "check_invariants"))]
            invariant_unconditioned_temps: Vec::with_capacity(building.zones.len()),
            #[cfg(any(debug_assertions, feature = "check_invariants"))]
            invariant_tank_temps: Vec::new(),
            #[cfg(any(debug_assertions, feature = "check_invariants"))]
            tank_node_keys: (0..24).map(tk::tank_node_key).collect(),
            #[cfg(any(debug_assertions, feature = "check_invariants"))]
            invariant_infiltration_latent: Vec::new(),
            #[cfg(any(debug_assertions, feature = "check_invariants"))]
            invariant_infiltration_m_dot: HashMap::new(),
            #[cfg(any(debug_assertions, feature = "check_invariants"))]
            invariant_infiltration_w_outdoor: HashMap::new(),
            #[cfg(any(debug_assertions, feature = "check_invariants"))]
            is_warming_up: false,
            #[cfg(all(
                feature = "observe",
                any(debug_assertions, feature = "check_invariants")
            ))]
            invariant_moisture_capture: Vec::new(),
            zone_is_conditioned: if building.zones.is_empty() {
                vec![true]
            } else {
                building
                    .zones
                    .iter()
                    .map(|z| z.zone_type == hares_io::hpxml::ZoneType::Conditioned)
                    .collect()
            },
            output_verbosity: config.sim_config.output_verbosity,
            output_chunk_size: config.sim_config.output_chunk_size,
            output_format: config.sim_config.output_format,
            output_path: output_path.clone(),
            write_output: config.sim_config.write_output,
            retain_batches: config.sim_config.retain_batches,
            diagnostic_writer: None,
            #[cfg(feature = "profiling")]
            profiling: DwellingProfilingSummary::default(),
            #[cfg(feature = "actor_profiling")]
            per_actor_timing: Vec::new(),
            #[cfg(feature = "actor_profiling")]
            actor_name_cache: Vec::new(),
            #[cfg(feature = "observe")]
            observer_buf: None,
            #[cfg(feature = "observe")]
            rolled_back_port_equipment: 0,
            #[cfg(feature = "observe")]
            nan_temperature_count: 0,
            #[cfg(feature = "observe")]
            diagnostic_accum: None,
            #[cfg(any(debug_assertions, feature = "observe_detailed"))]
            envelope_diagnostics: solvers.envelope_diagnostics,
        };

        dwelling.auto_register_actors();

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

            #[allow(
                unused_variables,
                reason = "iterations is logged in the observe feature block below; #[cfg(feature = \"observe\")] gates the only use site"
            )]
            let iterations = dwelling.run_warmup_converged(0.5, 25)?;

            #[cfg(feature = "observe")]
            let rng_word_pos_after_warmup = dwelling.rng.get_word_pos();

            // Restore RNG state to pre-warmup position.
            let mut restored_rng = ChaCha8Rng::from_seed(rng_seed_before);
            restored_rng.set_stream(rng_stream_before);
            restored_rng.set_word_pos(rng_word_pos_before);
            dwelling.rng = restored_rng;

            #[cfg(feature = "observe")]
            {
                let rng_delta = (rng_word_pos_after_warmup as i128) - (rng_word_pos_before as i128);
                tracing::debug!(
                    warmup_iterations = iterations,
                    rng_word_pos_delta = rng_delta,
                    "warmup complete; RNG restored for production-phase reproducibility"
                );
            }

            #[cfg(any(debug_assertions, feature = "check_invariants"))]
            {
                assert_eq!(
                    dwelling.rng.get_word_pos(),
                    rng_word_pos_before,
                    "RNG word_pos changed during warmup; restoration failed"
                );
            }

            clock = SimClock::new(
                local_start,
                config.sim_config.time_res,
                config.sim_config.duration,
            );
            #[cfg(feature = "dst")]
            {
                clock.civil_tz = parsed_civil_tz;
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
            let file = std::fs::File::create(&diag_path)
                .map_err(|err| HaresError::Io(format!("diagnostic file create failed: {err}")))?;
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
            diagnostics::write_equipment_init(&mut writer, &equipment_zones);
            Some(writer)
        } else {
            None
        };

        Ok(dwelling)
    }

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
    /// [`check_invariants`](Self::check_invariants) call to receive deliberately
    /// broken balance terms, forcing `InvariantViolation { check_name: "thermal_balance" }`.
    ///
    /// Only available in debug_assertions builds. Has no effect on the
    /// production simulation — the real balance terms computed by the solver
    /// are preserved.
    #[cfg(debug_assertions)]
    pub fn set_thermal_invariant_failure_for_test(&mut self) {
        self.test_thermal_invariant_failure = true;
    }

    /// Test-only: causes the next HVAC delivered-energy invariant checks to
    /// receive deliberately negative values, forcing `NegativeDeliveredEnergy`.
    /// The flag is reset to `false` after one check so the effect is scoped.
    #[cfg(debug_assertions)]
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
    /// Returns `Err` if the equipment is not found or the signal is rejected
    /// by the equipment's current state (e.g. EvDrive while plugged in).
    pub fn apply_control_validated(&mut self, name: &str, signal: ControlSignal) -> Result<()> {
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
        let priority = PriorityTier::from(&signal);
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

    /// Adds an actor to the dwelling's decision-making loop.
    ///
    /// Actors are called in registration order each timestep. They emit
    /// dispatch requests that are routed through the control dispatcher
    /// by [`PriorityTier`].
    pub fn add_actor(&mut self, actor: Box<dyn Actor>) {
        #[cfg(feature = "actor_profiling")]
        self.actor_name_cache.push(actor.name().to_string());
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        let actor_name = actor.name().to_string();
        self.actors.push(actor);
        self.rebuild_schedule();
        self.refresh_equipment_caches();
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        tracing::info!(
            actor_name = actor_name,
            actor_index = self.actors.len() - 1,
            "actor registered"
        );
    }

    /// Creates and adds an actor from the registry using the provided config.
    ///
    /// # Errors
    ///
    /// Returns an error if the actor type is not registered.
    pub fn add_actor_by_name(
        &mut self,
        registry: &crate::actor_registry::ActorRegistry,
        config: crate::actor_registry::ActorConfig,
    ) -> Result<()> {
        let actor = registry.create(config)?;
        #[cfg(feature = "actor_profiling")]
        self.actor_name_cache.push(actor.name().to_string());
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        let actor_name = actor.name().to_string();
        self.actors.push(actor);
        self.rebuild_schedule();
        self.refresh_equipment_caches();
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        tracing::info!(
            actor_name = actor_name,
            actor_index = self.actors.len() - 1,
            "actor registered"
        );
        Ok(())
    }

    /// Returns the number of registered actors.
    #[must_use]
    pub fn actor_count(&self) -> usize {
        self.actors.len()
    }

    /// Rebuilds the actor execution plan from current actor registrations.
    ///
    /// Auto-registered actors (BMS, EV driver) receive priority 0 within
    /// [`ExecutionPhase::ActorDecide`]; user-added actors receive priority 10.
    /// The solver feedback actor is registered in
    /// [`ExecutionPhase::SolverFeedback`] to run before all others.
    fn rebuild_schedule(&mut self) {
        self.scheduler.clear();
        self.scheduler.register_solver_feedback();
        // Auto-registered actor names (built from equipment seeds) get
        // priority 0, user-added actors get priority 10. This preserves the
        // current behaviour where built-in actors run before user actors.
        for (i, actor) in self.actors.iter().enumerate() {
            let priority = if self.auto_registered_actor_names.contains(actor.name()) {
                0
            } else {
                10
            };
            self.scheduler.register_actor(
                ActorSlot(i),
                ExecutionPhase::ActorDecide,
                priority,
                actor.name(),
            );
        }
    }

    /// Auto-register built-in BMS and EV actors based on equipment configuration.
    ///
    /// Called after equipment init and optionally after `set_tariff()`.
    /// Built-in actors are prepended before any existing (user) actors.
    /// Idempotent: skips registration if an actor with the same name already exists.
    pub fn auto_register_actors(&mut self) {
        let interval_secs = self.clock.time_res.num_seconds() as u32;
        assert!(
            interval_secs > 0,
            "time resolution must be positive; got 0 seconds"
        );
        let steps_per_day = 86_400 / interval_secs as usize;

        let price_schedule: Option<Arc<[f64]>> = self
            .tariff_evaluator
            .as_ref()
            .and_then(|te| te.price_slice(0, te.total_steps()).map(Arc::from));

        let has_tariff = self.tariff_evaluator.is_some();

        // Evict previously auto-registered actors so they can be rebuilt
        // with updated tariff/price data.
        if !self.auto_registered_actor_names.is_empty() {
            self.actors
                .retain(|a| !self.auto_registered_actor_names.contains(a.name()));
            self.auto_registered_actor_names.clear();
        }

        let built_in_actors = build_actors_from_seeds(
            &self.equipment,
            &self.actors,
            has_tariff,
            price_schedule,
            steps_per_day,
            &self.equipment_id_by_name,
            &self.rng,
        );

        if !built_in_actors.is_empty() {
            for a in &built_in_actors {
                self.auto_registered_actor_names
                    .insert(a.name().to_string());
            }
            let user_actors = std::mem::take(&mut self.actors);
            self.actors = built_in_actors;
            self.actors.extend(user_actors);
        }

        #[cfg(feature = "actor_profiling")]
        {
            self.actor_name_cache.clear();
            self.actor_name_cache
                .extend(self.actors.iter().map(|a| a.name().to_string()));
        }
        self.rebuild_schedule();
        self.refresh_equipment_caches();

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        for (i, actor) in self.actors.iter().enumerate() {
            tracing::info!(
                actor_name = actor.name(),
                actor_index = i,
                "actor registered"
            );
        }
    }
}

/// Typed payload for `set_battery_lut` -- ensures the data matches the LUT type at compile time.
pub enum BatteryLutData {
    ChargingCurve(RegularGridInterpolator),
    Ocv(OcvTable),
    UNeg(UNegTable),
}

impl Dwelling {
    /// Returns a slice of equipment for read-only access.
    #[must_use]
    pub fn equipment(&self) -> &[Box<dyn Equipment>] {
        &self.equipment
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
                eq.set_charging_curve_lut(Some(interp))
            }
            (BatteryLutType::Ocv, BatteryLutData::Ocv(table)) => eq.set_ocv_table(table),
            (BatteryLutType::UNeg, BatteryLutData::UNeg(table)) => eq.set_u_neg_table(table),
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
            BatteryLutType::ChargingCurve => eq.set_charging_curve_lut(None),
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

        eq.set_charging_curve_lut(Some(lut))
    }

    /// Clear the charging curve LUT on an EV equipment by name.
    pub fn clear_ev_charging_curve_lut(&mut self, name: &str) -> Result<()> {
        let eq = self
            .equipment
            .iter_mut()
            .find(|e| e.descriptor().name == name)
            .ok_or_else(|| HaresError::Equipment(format!("equipment '{}' not found", name)))?;

        eq.set_charging_curve_lut(None)
    }

    /// Returns the current environment state (zone temps, weather, grid, time).
    ///
    /// Useful for initializing equipment with realistic state before the first step.
    #[must_use]
    pub fn latest_env(&self) -> &EnvironmentState {
        &self.latest_env
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

    /// Adds equipment to the dwelling and refreshes internal caches.
    ///
    /// Equipment execution order and dispatch targets are pre-computed for
    /// hot-loop efficiency. This method maintains those caches when adding
    /// equipment after construction.
    ///
    /// If an equipment with the same name already exists, the incoming
    /// equipment is automatically renamed using the `"Name #N"` convention
    /// (e.g. a second `"PV"` becomes `"PV #2"`).
    pub fn add_equipment(&mut self, mut eq: Box<dyn Equipment>) {
        let name = eq.descriptor().name.clone();
        if self.equipment.iter().any(|e| e.descriptor().name == name) {
            let count = self
                .equipment
                .iter()
                .filter(|e| {
                    let n = e.descriptor().name.as_str();
                    n == name.as_str() || n.starts_with(&format!("{name} #"))
                })
                .count()
                + 1;
            let new_name = format!("{name} #{count}");
            tracing::warn!(
                incoming = %name,
                renamed = %new_name,
                "add_equipment: renaming duplicate equipment to avoid collision"
            );
            eq.rename(new_name);
        }
        self.equipment.push(eq);
        self.refresh_equipment_caches();
    }

    /// Removes all equipment and refreshes internal caches.
    pub fn clear_equipment(&mut self) {
        self.equipment.clear();
        self.refresh_equipment_caches();
    }

    /// Removes equipment by name and returns it.
    ///
    /// Returns `Err` if no equipment with the given name exists.
    pub fn remove_equipment(&mut self, name: &str) -> Result<Box<dyn Equipment>> {
        let pos = self
            .equipment
            .iter()
            .position(|e| e.descriptor().name == name)
            .ok_or_else(|| HaresError::Dwelling(format!("equipment '{}' not found", name)))?;
        let removed = self.equipment.remove(pos);
        self.refresh_equipment_caches();
        Ok(removed)
    }

    /// Removes all equipment whose end-use matches any of the given set.
    ///
    /// Returns the count of equipment removed.
    pub fn remove_equipment_by_end_use(&mut self, end_uses: &[EndUse]) -> usize {
        let before = self.equipment.len();
        self.equipment
            .retain(|e| !end_uses.contains(&e.descriptor().end_use));
        let removed = before - self.equipment.len();
        if removed > 0 {
            self.refresh_equipment_caches();
        }
        removed
    }

    /// Replaces equipment by name with new equipment, returning the old equipment.
    ///
    /// Returns `Err` if no equipment with the given name exists.
    pub fn replace_equipment(
        &mut self,
        name: &str,
        new_equipment: Box<dyn Equipment>,
    ) -> Result<Box<dyn Equipment>> {
        let pos = self
            .equipment
            .iter()
            .position(|e| e.descriptor().name == name)
            .ok_or_else(|| HaresError::Dwelling(format!("equipment '{}' not found", name)))?;
        let old = std::mem::replace(&mut self.equipment[pos], new_equipment);
        self.refresh_equipment_caches();
        Ok(old)
    }

    /// Refreshes internal caches after equipment list modification.
    ///
    /// Rebuilds execution order, dispatch targets, and -- if no rows have been
    /// recorded yet -- the output schema, column index, equipment column map,
    /// and streaming recorder so that dynamically added equipment appears in
    /// simulation output.
    pub fn refresh_equipment_caches(&mut self) {
        self.equipment_id_by_name = self
            .equipment
            .iter()
            .map(|eq| (eq.descriptor().name.clone(), eq.descriptor().id))
            .collect();
        self.equipment_execution_order = compute_equipment_execution_order(&self.equipment);
        self.solver_feedback_actor
            .set_dispatch_targets(compute_equipment_dispatch_targets(&self.equipment));

        // Rebuild output schema so dynamically added equipment and actors get columns.
        // Only safe before any rows have been recorded; mid-simulation schema
        // changes would corrupt the output file.
        if self
            .recorder
            .as_ref()
            .map_or(0, StreamingRecorder::total_rows)
            == 0
        {
            let specs: Vec<hares_io::EquipmentSpec> = self
                .equipment
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
                .collect();

            let mut schema = build_schema(
                &specs,
                self.output_verbosity,
                &self
                    .latest_env
                    .zones
                    .iter()
                    .map(|z| {
                        let zone_type = self
                            .latest_env
                            .zones
                            .iter()
                            .position(|zt| zt.id == z.id)
                            .and_then(|idx| self.environment.zone_types().get(idx));
                        (
                            z.id,
                            zone_display_name(
                                z.id,
                                self.thermal_solver.config().indoor_zone_id,
                                zone_type,
                            ),
                        )
                    })
                    .collect::<Vec<_>>(),
            );
            schema = extend_schema_with_actor_columns(&schema, &self.actors);
            {
                use arrow::datatypes::DataType;
                let mut fields: Vec<arrow::datatypes::Field> =
                    schema.fields().iter().map(|f| f.as_ref().clone()).collect();
                fields.push(arrow::datatypes::Field::new(
                    "port_rollback_count",
                    DataType::Float64,
                    true,
                ));
                fields.push(arrow::datatypes::Field::new(
                    "nan_temperature_count",
                    DataType::Float64,
                    true,
                ));
                schema =
                    arrow::datatypes::Schema::new_with_metadata(fields, schema.metadata().clone());
            }
            self.output_value_count = schema.fields().len() - 1;
            self.output_column_index = build_output_column_index(&schema);
            self.equipment_column_map =
                build_equipment_column_map(&self.equipment, &self.output_column_index);
            self.end_use_aggregate_indices = {
                // Rebuild using the specs derived from current equipment descriptors.
                let specs: Vec<hares_io::EquipmentSpec> = self
                    .equipment
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
                    .collect();
                specs
                    .iter()
                    .map(|spec| {
                        let end_use = equipment_name_to_end_use(&spec.name);
                        let col_name = end_use_electric_power_column(&end_use);
                        self.output_column_index.get(&col_name).copied()
                    })
                    .collect()
            };

            // Pre-resolve actor telemetry column indices; avoids format!() per step.
            self.actor_column_map.clear();
            self.actor_column_map.reserve(self.actors.len());
            for actor in &self.actors {
                if let Some(tel) = actor.telemetry() {
                    let actor_name = actor.name();
                    let mut entries = Vec::with_capacity(tel.0.len());
                    for key in tel.0.keys() {
                        let col_name = format!("actor:{}:{}", actor_name, key);
                        if let Some(&idx) = self.output_column_index.get(&col_name) {
                            entries.push((key.clone(), idx));
                        }
                    }
                    self.actor_column_map.push(entries);
                } else {
                    self.actor_column_map.push(Vec::new());
                }
            }

            let zone_types = self.environment.zone_types().to_vec();
            let zone_caches = build_zone_column_caches(
                &self.latest_env.zones,
                &zone_types,
                self.thermal_solver.config().indoor_zone_id,
                &self.output_column_index,
            );
            self.zone_temp_columns = zone_caches.temp_columns;
            self.zone_infiltration_columns = zone_caches.infiltration_columns;
            self.zone_lwr_columns = zone_caches.lwr_columns;
            self.zone_hvac_columns = zone_caches.hvac_columns;
            self.zone_temp_scratch = zone_caches
                .sorted_zone_ids
                .iter()
                .map(|&z| (z, 0.0))
                .collect();
            self.sorted_zone_ids = zone_caches.sorted_zone_ids;
            self.zone_env_indices = zone_caches.zone_env_indices;
            self.zone_temp_col_indices = zone_caches.zone_temp_col_indices;
            self.record_scratch.clear();
            self.record_scratch.resize(self.output_value_count, 0.0);

            if self.write_output
                && let Ok(recorder) = StreamingRecorder::new(
                    schema,
                    self.output_chunk_size,
                    self.output_format,
                    &self.output_path,
                    self.retain_batches,
                )
            {
                self.recorder = Some(recorder);
            }
        }
    }

    /// Returns per-actor timing from the simulation (requires `actor_profiling` feature).
    /// Each entry is `(actor_name, elapsed_duration)`.
    #[cfg(feature = "actor_profiling")]
    #[must_use]
    pub fn actor_timing(&self) -> Vec<(String, StdDuration)> {
        self.per_actor_timing
            .iter()
            .map(|&(idx, dur)| {
                let name = self
                    .actor_name_cache
                    .get(idx)
                    .cloned()
                    .unwrap_or_else(|| format!("actor_{idx}"));
                (name, dur)
            })
            .collect()
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

    /// Configures a tariff evaluator from an electric tariff definition.
    ///
    /// The evaluator precomputes prices for the entire simulation horizon so
    /// that `run_timestep` can populate `EnvironmentState.price_signal` before
    /// actors run. Billing accumulation happens post-solver each step.
    pub fn set_tariff(&mut self, tariff: ElectricTariff, tz: Tz) -> Result<()> {
        let start = self.clock.start_time.with_timezone(&tz);
        let end_fixed = self.clock.start_time + self.clock.duration;
        let end = end_fixed.with_timezone(&tz);
        let interval_secs = self.clock.time_res.num_seconds() as u32;
        let evaluator = TariffEvaluator::new(tariff, start, end, interval_secs)?;
        self.tariff_evaluator = Some(evaluator);
        self.auto_register_actors();
        Ok(())
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

    /// Applies a grid voltage override through the environment manager.
    pub fn set_grid_voltage(&mut self, voltage_pu: f64) {
        self.environment.set_grid_override(GridState {
            voltage_pu,
            frequency_hz: DEFAULT_GRID_FREQUENCY_HZ,
        });
    }

    /// Returns current observable state.
    #[must_use]
    pub fn telemetry(&self) -> DwellingTelemetry {
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
            {
                if let Some(sp) = co.state.setpoint_c {
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
        }

        let mut actor_telemetry: HashMap<String, HashMap<String, f64>> =
            HashMap::with_capacity(self.actors.len() + 1);

        // Collect solver feedback actor telemetry (always None, but included for completeness).
        if let Some(tel) = self.solver_feedback_actor.telemetry() {
            actor_telemetry.insert(self.solver_feedback_actor.name().to_string(), tel.0.clone());
        }

        for actor in &self.actors {
            if let Some(tel) = actor.telemetry() {
                actor_telemetry.insert(actor.name().to_string(), tel.0.clone());
            }
        }

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            let w = self.latest_env.weather.outdoor_humidity_ratio;
            assert!(
                w >= 0.0,
                "outdoor_humidity_ratio={w} is negative — invalid since humidity ratio mass must be non-negative"
            );
            // NaN screening on every float field before the telemetry snapshot
            // is emitted.  Non-finite values reach downstream consumers
            // (RL agents, monitor scripts, fleet controller) and cause silent
            // misbehaviour that is far more expensive to diagnose than the
            // invariant check that could have caught it here.
            let step = self.clock.current_step();
            let _ = self.invariant_checker.check_nan_screen(
                step,
                &[(
                    "outdoor_temp_c",
                    None,
                    self.latest_env.weather.outdoor_temp_c,
                )],
            );
            for (i, &t) in zone_temperatures_c.iter().enumerate() {
                if !t.is_finite() {
                    let zid = zone_ids.get(i).copied();
                    tracing::error!(
                        step = step,
                        zone = ?zid,
                        field = "zone_temperature_c",
                        value = t,
                        "NaN/Inf in DwellingTelemetry construction"
                    );
                }
            }
            for (i, &p) in equipment_power_kw.iter().enumerate() {
                if !p.is_finite() {
                    let name = equipment_names.get(i).map(|s| s.as_str()).unwrap_or("?");
                    tracing::error!(
                        step = step,
                        equipment = name,
                        field = "equipment_power_kw",
                        value = p,
                        "NaN/Inf in DwellingTelemetry construction"
                    );
                }
            }
            for (i, &p) in energy_balance_residuals.iter().enumerate() {
                if !p.is_finite() {
                    let zid = zone_ids.get(i).copied();
                    tracing::error!(
                        step = step,
                        zone = ?zid,
                        field = "energy_balance_residuals",
                        value = p,
                        "NaN/Inf in DwellingTelemetry construction"
                    );
                }
            }
            if !self.electrical_solver.net_active_kw().is_finite() {
                tracing::error!(
                    step = step,
                    field = "total_power_kw",
                    "NaN/Inf in DwellingTelemetry construction"
                );
            }
            if !self.electrical_solver.net_reactive_kvar().is_finite() {
                tracing::error!(
                    step = step,
                    field = "reactive_power_kvar",
                    "NaN/Inf in DwellingTelemetry construction"
                );
            }
        }

        let mut telem = DwellingTelemetry {
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
            outdoor_temp_c: self.latest_env.weather.outdoor_temp_c,
            outdoor_humidity_ratio: self.latest_env.weather.outdoor_humidity_ratio,
            actor_telemetry,
            dwelling_failed: self.failed,
            telemetry_consistency_flag: true,
        };

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            let step = self.clock.current_step();
            telem.verify_consistency(step);
            telem.telemetry_consistency_flag =
                telem.telemetry_consistency_flag && self.thermal_consistency_flag;
        }

        telem
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
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    fn verify_per_zone_thermal_consistency(&self, step: u64) -> bool {
        let port_by_zone: HashMap<ZoneId, f64> = self
            .ports
            .thermal
            .iter()
            .map(|a| {
                let cat_total: f64 = [ThermalCategory::HvacHeating, ThermalCategory::HvacCooling]
                    .into_iter()
                    .map(|cat| {
                        a.sensible_for_category(cat)
                            + a.radiant_for_category(cat)
                            + a.latent_for_category(cat)
                    })
                    .sum();
                (a.zone, cat_total)
            })
            .collect();

        let equip_by_zone: HashMap<ZoneId, f64> = {
            let mut map: HashMap<ZoneId, f64> = HashMap::new();
            for eq in &self.equipment {
                let desc = eq.descriptor();
                if let Some(zone) = desc.zone {
                    if desc.end_use == EndUse::HVAC_HEATING || desc.end_use == EndUse::HVAC_COOLING
                    {
                        let thermal_w = eq.core_output().flows.thermal_output_w.unwrap_or(0.0);
                        *map.entry(zone).or_insert(0.0) += thermal_w;
                    }
                }
            }
            map
        };

        let all_zones: HashSet<ZoneId> = port_by_zone
            .keys()
            .chain(equip_by_zone.keys())
            .copied()
            .collect();

        for zone in all_zones {
            let port_total = port_by_zone.get(&zone).copied().unwrap_or(0.0);
            let equip_total = equip_by_zone.get(&zone).copied().unwrap_or(0.0);
            let tolerance = 1.0_f64.max(1e-6 * port_total.abs());
            let diff = (port_total - equip_total).abs();
            if diff > tolerance {
                tracing::warn!(
                    step = step,
                    zone = ?zone,
                    port_total_w = port_total,
                    equip_total_w = equip_total,
                    diff_w = diff,
                    "Telemetry consistency: per-zone thermal totals do not match equipment contributions"
                );
                return false;
            }
        }
        true
    }

    /// Drains and returns warning messages accumulated since the previous call.
    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    #[cfg(feature = "profiling")]
    #[must_use]
    pub fn profiling_summary(&self) -> DwellingProfilingSummary {
        self.profiling.clone()
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

    /// Snapshot current simulation state to an in-memory checkpoint struct.
    pub fn save_checkpoint(&self) -> Result<DwellingCheckpoint> {
        let (envelope_state, thermal_last_u, lwr_t_prev_c) = self.thermal_solver.snapshot_state();
        let humidity_states: Vec<(ZoneId, f64)> = self
            .humidity_solver
            .humidity_ratios
            .iter()
            .map(|(zone, value)| (*zone, *value))
            .collect();
        let fluid_states = self.fluid_solver.snapshot_payload();

        Ok(DwellingCheckpoint {
            format_version: CHECKPOINT_VERSION,
            bldg_id: self.bldg_id,
            timestep_index: self.clock.current_step(),
            equipment_states: self
                .equipment
                .iter()
                .map(|eq| eq.save_state())
                .collect::<std::result::Result<Vec<_>, _>>()?,
            rng_state: self.rng.get_seed(),
            envelope_state,
            humidity_states,
            fluid_states,
            rng_stream: self.rng.get_stream(),
            rng_word_pos: self.rng.get_word_pos(),
            thermal_last_u,
            lwr_t_prev_c,
            actor_states: self
                .actors
                .iter()
                .map(|a| a.save_state().map(|blob| (a.name().to_string(), blob)))
                .collect::<std::result::Result<Vec<_>, HaresError>>()?,
            prior_electrical_summary: self.prior_electrical_summary.clone(),
        })
    }

    /// Restore simulation state from a checkpoint.
    pub fn load_checkpoint(&mut self, cp: DwellingCheckpoint) -> Result<()> {
        if cp.format_version != CHECKPOINT_VERSION {
            return Err(HaresError::Io(format!(
                "checkpoint version mismatch: file={}, expected={}",
                cp.format_version, CHECKPOINT_VERSION
            )));
        }

        self.clock.current_step = cp.timestep_index;
        let mut restored_rng = ChaCha8Rng::from_seed(cp.rng_state);
        restored_rng.set_stream(cp.rng_stream);
        restored_rng.set_word_pos(cp.rng_word_pos);
        self.rng = restored_rng;

        if cp.equipment_states.len() != self.equipment.len() {
            return Err(HaresError::Io(format!(
                "checkpoint equipment count mismatch: checkpoint has {} equipment states, dwelling has {} equipment",
                cp.equipment_states.len(),
                self.equipment.len()
            )));
        }
        for (eq, state) in self.equipment.iter_mut().zip(cp.equipment_states) {
            eq.load_state(&state)?;
        }

        self.thermal_solver
            .restore_state(&cp.envelope_state, &cp.thermal_last_u, &cp.lwr_t_prev_c)
            .map_err(|err| HaresError::Envelope(format!("restore thermal state failed: {err}")))?;

        let checkpoint_zones: HashMap<ZoneId, f64> = cp.humidity_states.into_iter().collect();
        for zone in &mut self.latest_env.zones {
            let humidity = checkpoint_zones.get(&zone.id).ok_or_else(|| {
                HaresError::Io(format!(
                    "checkpoint missing humidity state for zone {:?}",
                    zone.id
                ))
            })?;
            self.humidity_solver
                .humidity_ratios
                .insert(zone.id, *humidity);
            zone.humidity_ratio = *humidity;
        }
        self.fluid_solver
            .restore_from_payload(&cp.fluid_states)
            .map_err(|err| HaresError::Envelope(format!("restore fluid state failed: {err}")))?;

        // Restore actor decision-state.
        let mut restored_count = 0usize;
        let cp_actor_map: std::collections::HashMap<&str, &[u8]> = cp
            .actor_states
            .iter()
            .map(|(name, blob)| (name.as_str(), blob.as_slice()))
            .collect();
        for actor in self.actors.iter_mut() {
            if let Some(blob) = cp_actor_map.get(actor.name()) {
                actor.load_state(blob)?;
                restored_count += 1;
            }
        }

        if restored_count != self.actors.len() {
            return Err(HaresError::Io(format!(
                "checkpoint actor count mismatch: checkpoint has {} actor states, dwelling has {} actors",
                restored_count,
                self.actors.len()
            )));
        }
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            for actor in self.actors.iter() {
                tracing::debug!(actor = actor.name(), "post-restore actor state",);
            }
        }

        self.prior_electrical_summary = cp.prior_electrical_summary;

        // Populate latest_env.equipment_core and equipment_telemetry from the
        // restored equipment state so that actors read correct SOC, power flows,
        // and connection state on the first post-restore step.
        self.snapshot_equipment_state();
        self.restored_from_checkpoint = true;

        Ok(())
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

    /// Iterative warm-up convergence per EnergyPlus Engineering Reference §"Warmup Convergence".
    ///
    /// Repeatedly simulates the first 24-hour weather day until the maximum zone
    /// temperature change across all conditioned zones between consecutive iterations
    /// falls below `threshold_c` °C, up to `max_iter` iterations.
    ///
    /// EnergyPlus defaults: threshold = 0.5 °C, max_iter = 25 iterations.
    /// EnergyPlus I/O Reference §"Building": "This value represents the number at
    /// which the zone temperatures must agree ... before 'convergence' is reached."
    /// Typical convergence: 1-2 iterations for lightweight construction, 4-7 for
    /// heavyweight (concrete slab, masonry).
    pub fn run_warmup_converged(&mut self, threshold_c: f64, max_iter: u32) -> Result<u32> {
        let time_res_s = u64::try_from(self.clock.time_res.num_seconds())
            .map_err(|_| HaresError::Io("invalid time resolution".to_string()))?;
        let steps_per_day = (24u64 * 3600).checked_div(time_res_s).unwrap_or(0);

        if steps_per_day == 0 {
            return Ok(1);
        }

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            self.is_warming_up = true;
        }

        let mut prev_zone_temps: Vec<f64> = Vec::new();

        for iteration in 1..=max_iter {
            // Reset clock to start of first day for weather replay.
            // Thermal state carries forward from previous iteration:
            // EnergyPlus §"Warmup Convergence" — initial conditions for each
            // warmup day are the final conditions from the previous warmup day.
            self.clock.current_step = 0;

            for _ in 0..steps_per_day {
                self.run_timestep(false)?;
            }

            // Collect conditioned zone temperatures from the environment state.
            // Order is stable: both `latest_env.zones` and `zone_is_conditioned`
            // are aligned by construction.
            let zone_temps: Vec<f64> = (0..self.latest_env.zones.len())
                .filter(|&i| self.zone_is_conditioned[i])
                .map(|i| self.latest_env.zones[i].temperature_c)
                .collect();

            if !prev_zone_temps.is_empty() {
                // EnergyPlus Engineering Reference §"Warmup Convergence":
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
                    #[cfg(any(debug_assertions, feature = "check_invariants"))]
                    {
                        self.is_warming_up = false;
                    }
                    return Ok(iteration);
                }
            }

            prev_zone_temps = zone_temps;
        }

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            self.is_warming_up = false;
        }

        self.simulation_results.steps.clear();
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
    /// the observe-gated counter, and emits a `tracing::warn!`.
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
        #[cfg(feature = "observe")]
        {
            self.rolled_back_port_equipment += 1;
        }
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
            return Err(HaresError::Dwelling(
                "dwelling permanently failed after prior panic, cannot step".to_string(),
            ));
        }
        if self.clock.current_step() >= self.clock.total_steps() {
            return Err(HaresError::Dwelling(
                "simulation already reached configured end".to_string(),
            ));
        }

        // Advance the dwelling RNG on every timestep so checkpoint captures
        // reflect simulation progress.  The value is intentionally discarded;
        // stochastic components use independent sub-RNGs derived from the
        // dwelling RNG's seed via stream partitioning.
        let rng_word_pos_before = self.rng.get_word_pos();
        let _ = advance_dwelling_rng(&mut self.rng);

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        let rng_consumed_this_step = self.rng.get_word_pos() > rng_word_pos_before;

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            // After the first step, every equipment instance must have a core
            // output in the environment. Missing entries after checkpoint
            // restore indicate that snapshot_equipment_state was not called in
            // load_checkpoint.
            if self.clock.current_step() > 0 {
                for eq in &self.equipment {
                    let desc = eq.descriptor();
                    let id =
                        self.equipment_id_by_name.get(&desc.name).copied().expect(
                            "invariant: equipment_id_by_name is built from this equipment set",
                        );
                    debug_assert!(
                        self.latest_env.equipment_core.contains_key(&id),
                        "equipment_core missing entry for equipment '{}' (id={:?}) \
                         at start of step {}; checkpoint restore must call \
                         snapshot_equipment_state",
                        desc.name,
                        id,
                        self.clock.current_step(),
                    );
                }
            }

            // First step after checkpoint restore: double-check that
            // equipment_core is not accidentally empty when we expect
            // restored state.
            if self.restored_from_checkpoint && !self.equipment.is_empty() {
                debug_assert!(
                    !self.latest_env.equipment_core.is_empty(),
                    "equipment_core is empty on first post-restore step {}; \
                     load_checkpoint must call snapshot_equipment_state",
                    self.clock.current_step(),
                );
                self.restored_from_checkpoint = false;
            }
        }

        #[cfg(feature = "profiling")]
        let step_started = Instant::now();
        #[cfg(feature = "profiling")]
        let alloc_before = hot_path_alloc_counter();
        #[cfg(feature = "profiling")]
        let step_schedule: Option<StdDuration>;
        #[cfg(feature = "profiling")]
        let step_hvac: Option<StdDuration>;
        #[cfg(feature = "profiling")]
        let step_envelope: Option<StdDuration>;
        #[cfg(feature = "profiling")]
        let mut step_io: Option<StdDuration> = None;

        #[cfg(feature = "observe")]
        let mut obs_phases = PhaseSnapshots::default();

        #[cfg(feature = "observe")]
        {
            self.rolled_back_port_equipment = 0;
            self.nan_temperature_count = 0;
        }

        // Step 1: update environment at current clock state.
        // Feed zone temperatures back first so the borrow on self.latest_env.zones
        // is released before we mutably borrow self.latest_env for update_in_place.
        #[cfg(feature = "profiling")]
        let schedule_started = Instant::now();
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

        // Step-start humidity invariant: confirm that the humidity ratio in
        // `latest_env.zones` matches the humidity solver's committed state.
        // Both are updated together at the end of each timestep by
        // `apply_humidity_update_to_zones` → `humidity_solver.resolve`, and
        // Step 1's environment update preserves zone state byte-for-byte via
        // `extend_from_slice`. A mismatch here indicates a zone was added
        // after solver construction without seeding its initial humidity ratio.
        #[cfg(debug_assertions)]
        for zone in &self.latest_env.zones {
            // f64::EPSILON is safe here: both values come from the same
            // humidity_ratios map (updated together by `resolve` and read back
            // by `humidity_ratio` without intermediate arithmetic), so exact
            // bitwise equality holds.
            debug_assert!(
                (zone.humidity_ratio - self.humidity_solver.humidity_ratio(zone.id)).abs()
                    < f64::EPSILON,
                "zone {} humidity ratio {:.6e} diverged from solver committed {:.6e} at start of step",
                zone.id.0,
                zone.humidity_ratio,
                self.humidity_solver.humidity_ratio(zone.id),
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
            Some(
                self.control_dispatcher
                    .dispatch_into_observed(&mut self.equipment, &mut self.warnings),
            )
        } else {
            self.control_dispatcher
                .dispatch_into(&mut self.equipment, &mut self.warnings);
            None
        };
        #[cfg(not(feature = "observe"))]
        self.control_dispatcher
            .dispatch_into(&mut self.equipment, &mut self.warnings);

        // Step 1b: deposit deterministic internal gains (occupancy, plug loads)
        // BEFORE prepare_inputs so the ideal solver sees them when computing
        // required HVAC capacity.
        self.apply_occupancy_gains()?;

        let mut step_succeeded = vec![false; self.equipment.len()];

        // Step 1c: thermal equipment update_control() to determine mode and ideal targets.
        // Must run BEFORE solver feedback actor collects targets.
        for &idx in &self.equipment_execution_order {
            if self.equipment[idx].descriptor().stage == ExecutionStage::Thermal {
                let _ = self.equipment[idx].update_control(&self.latest_env);
            }
        }

        // Step 1d: build current-step inputs with all non-HVAC gains already on ports.
        self.thermal_solver
            .prepare_inputs(&self.ports, &self.latest_env);

        // Steps 1e–1f: phase-ordered actor execution driven by the scheduler.
        // The scheduler plan replaces the previously hard-coded
        // "solver feedback first, then all actors in registration order"
        // with explicit phase registration and within-phase priority ordering.
        #[expect(
            unused_must_use,
            reason = "plan is accessed via .plan() below; build() ensures freshness"
        )]
        self.scheduler.build();

        #[cfg(feature = "observe")]
        let mut scheduled_phases: Vec<String> = Vec::new();
        #[cfg(feature = "observe")]
        let mut actor_skips: usize = 0;
        #[cfg(feature = "observe")]
        let mut actor_calls: usize = 0;
        #[cfg(debug_assertions)]
        let mut executed_actors: std::collections::HashSet<usize> =
            std::collections::HashSet::new();
        #[cfg(feature = "actor_profiling")]
        {
            self.per_actor_timing.clear();
            self.per_actor_timing.reserve(self.actors.len());
        }

        self.actor_dispatch_buf.clear();
        for entry in self.scheduler.plan() {
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

                        #[cfg(feature = "actor_profiling")]
                        let start = Instant::now();

                        self.actors[idx].decide(&self.latest_env, &mut self.actor_dispatch_buf);

                        #[cfg(feature = "actor_profiling")]
                        self.per_actor_timing.push((idx, start.elapsed()));
                    }
                }
            }
        }
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
                    .any(|e| e.slot == Some(ActorSlot(i)));
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
            let capture = self
                .control_dispatcher
                .dispatch_into_observed(&mut self.equipment, &mut self.warnings);
            let merged = match pre_dispatch_capture {
                Some(mut pre) => {
                    pre.signals.extend(capture.signals);
                    pre
                }
                None => capture,
            };
            obs_phases.post_dispatch = Some(merged);
        } else {
            self.control_dispatcher
                .dispatch_into(&mut self.equipment, &mut self.warnings);
        }
        #[cfg(not(feature = "observe"))]
        self.control_dispatcher
            .dispatch_into(&mut self.equipment, &mut self.warnings);
        #[cfg(feature = "profiling")]
        {
            let elapsed = schedule_started.elapsed();
            step_schedule = Some(elapsed);
            self.profiling.schedule_load += elapsed;
            self.profiling.memory_high_water_kb = self
                .profiling
                .memory_high_water_kb
                .max(current_process_hwm_kb());
        }

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
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
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
            #[cfg(any(debug_assertions, feature = "check_invariants"))]
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
                let capture = self
                    .control_dispatcher
                    .dispatch_into_observed(&mut self.equipment, &mut self.warnings);
                if let Some(ref mut merged) = obs_phases.post_dispatch {
                    merged.signals.extend(capture.signals);
                }
            } else {
                self.control_dispatcher
                    .dispatch_into(&mut self.equipment, &mut self.warnings);
            }
            #[cfg(not(feature = "observe"))]
            self.control_dispatcher
                .dispatch_into(&mut self.equipment, &mut self.warnings);
        }

        // Step 2a: re-run update_control for thermal equipment after control
        // dispatch so ThermalSetpoint / ModeOverride / DR signals take effect
        // on the current timestep, not one step later.
        for &idx in &self.equipment_execution_order {
            if self.equipment[idx].descriptor().stage == ExecutionStage::Thermal {
                let _ = self.equipment[idx].update_control(&self.latest_env);
            }
        }

        #[cfg(feature = "profiling")]
        let hvac_started = Instant::now();

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
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        let mut stepped_stage_ranks: Vec<u8> =
            Vec::with_capacity(self.equipment_execution_order.len());

        // Phase 1: Independent-stage equipment (PV, generators).
        for _oi in 0..self.equipment_execution_order.len() {
            let idx = self.equipment_execution_order[_oi];
            let stage = self.equipment[idx].descriptor().stage;
            if stage != ExecutionStage::Independent {
                continue;
            }
            #[cfg(feature = "observe")]
            let pre_ports = pre_snapshot.as_ref().map(observer_capture::capture_ports);

            let _ = self.equipment[idx].update_control(&self.latest_env);
            self.rollback_ports.copy_into(&self.ports);
            if let Err(err) = self.equipment[idx].step(&self.latest_env, dt, &mut self.ports) {
                self.rollback_failed_equipment_ports(idx, &err);
            } else {
                validate_core_contract(
                    self.equipment[idx].descriptor(),
                    self.equipment[idx].core_output(),
                )?;
                step_succeeded[idx] = true;
                #[cfg(any(debug_assertions, feature = "check_invariants"))]
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
                    actor.adjust_for_pv(pv_kw, &self.latest_env, &mut self.actor_dispatch_buf);
                }
                if !self.actor_dispatch_buf.is_empty() {
                    for req in self.actor_dispatch_buf.drain(..) {
                        self.control_dispatcher.queue(req);
                    }
                    self.control_dispatcher
                        .dispatch_into(&mut self.equipment, &mut self.warnings);
                }
            }
        }

        // Phase 2: Electrical-stage equipment (Battery, EV).
        for _oi in 0..self.equipment_execution_order.len() {
            let idx = self.equipment_execution_order[_oi];
            let stage = self.equipment[idx].descriptor().stage;
            if stage != ExecutionStage::Electrical {
                continue;
            }
            #[cfg(feature = "observe")]
            let pre_ports = pre_snapshot.as_ref().map(observer_capture::capture_ports);

            let _ = self.equipment[idx].update_control(&self.latest_env);
            self.rollback_ports.copy_into(&self.ports);
            if let Err(err) = self.equipment[idx].step(&self.latest_env, dt, &mut self.ports) {
                self.rollback_failed_equipment_ports(idx, &err);
            } else {
                validate_core_contract(
                    self.equipment[idx].descriptor(),
                    self.equipment[idx].core_output(),
                )?;
                step_succeeded[idx] = true;
                #[cfg(any(debug_assertions, feature = "check_invariants"))]
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
        for _oi in 0..self.equipment_execution_order.len() {
            let idx = self.equipment_execution_order[_oi];
            if self.equipment[idx].descriptor().stage != ExecutionStage::Thermal {
                continue;
            }
            #[cfg(feature = "observe")]
            let pre_ports = pre_snapshot.as_ref().map(observer_capture::capture_ports);

            self.rollback_ports.copy_into(&self.ports);
            if let Err(err) = self.equipment[idx].step(&self.latest_env, dt, &mut self.ports) {
                self.rollback_failed_equipment_ports(idx, &err);
            } else {
                validate_core_contract(
                    self.equipment[idx].descriptor(),
                    self.equipment[idx].core_output(),
                )?;
                step_succeeded[idx] = true;
                #[cfg(any(debug_assertions, feature = "check_invariants"))]
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
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            self.invariant_checker
                .check_equipment_step_order(&stepped_stage_ranks)?;
        }

        #[cfg(debug_assertions)]
        {
            self.stage_snapshot = Some(StageSnapshot {
                ports: self.ports.clone(),
            });
        }

        #[cfg(feature = "profiling")]
        {
            let elapsed = hvac_started.elapsed();
            step_hvac = Some(elapsed);
            self.profiling.hvac += elapsed;
            self.profiling.memory_high_water_kb = self
                .profiling
                .memory_high_water_kb
                .max(current_process_hwm_kb());
        }

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
                let vent = &mut self.thermal_solver.config_mut().ventilation;
                vent.sensible_recovery_efficiency = eff_s;
                vent.latent_recovery_efficiency = eff_l;
            }
        }

        // Step 4: envelope/domain resolution.
        #[cfg(feature = "profiling")]
        let envelope_started = Instant::now();
        self.thermal_solver
            .integrate(&self.ports, &self.latest_env, &mut self.thermal_update_buf);

        // Apply zone temps and capture observer data while we still have the borrow.
        apply_thermal_update_to_zones(&mut self.latest_env, &self.thermal_update_buf);

        // Upsert thermal domain so humidity/electrical solvers see updated state.
        // Zone temperatures are already applied above; the upsert stores the full
        // DomainUpdate in custom_domains for solvers that read it (humidity).
        #[cfg(feature = "observe")]
        let thermal_for_observer = self.thermal_update_buf.clone();
        self.latest_env.upsert_domain_ref(&self.thermal_update_buf);

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            self.prev_humidity_ratios.clear();
            self.prev_humidity_ratios.extend(
                self.humidity_solver
                    .humidity_ratios
                    .iter()
                    .map(|(&z, &w)| (z, w)),
            );
        }

        self.humidity_solver.resolve(
            &self.ports,
            &self.latest_env,
            dt,
            &mut self.humidity_update_buf,
        );
        self.electrical_solver.resolve(
            &self.ports,
            &self.latest_env,
            dt,
            &mut self.electrical_update_buf,
        );
        self.fluid_solver.resolve(
            &self.ports,
            &self.latest_env,
            dt,
            &mut self.fluid_update_buf,
        );

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
                .zip_load_scale(self.latest_env.grid.voltage_pu);
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
                zip_scale,
                port_load_raw_kw,
                port_load_adj_kw,
                residual,
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
            );
            self.latest_env
                .upsert_domain_ref(&self.custom_update_bufs[i]);
        }

        #[cfg(feature = "observe")]
        if self.observer_buf.is_some() {
            let capture = observer_capture::capture_custom_solvers(&self.custom_domain_solvers);
            #[cfg(any(debug_assertions, feature = "check_invariants"))]
            {
                debug_assert_eq!(
                    capture.solvers.len(),
                    self.custom_domain_solvers.len(),
                    "custom solver capture count mismatch"
                );
            }
            obs_phases.post_custom_solvers = Some(capture);
        }

        #[cfg(feature = "profiling")]
        {
            let elapsed = envelope_started.elapsed();
            step_envelope = Some(elapsed);
            self.profiling.envelope_solve += elapsed;
            self.profiling.memory_high_water_kb = self
                .profiling
                .memory_high_water_kb
                .max(current_process_hwm_kb());
        }

        // Always-on invariant checks — unconditional in all build configurations.
        // These catch unrecoverable data corruption (NaN/Inf) in the two domains
        // where silent propagation would corrupt output records and downstream
        // metrics. O(n_zones + O(1)) per step — cheap enough to run every timestep.

        // Electrical finiteness: NaN or Inf in the electrical solver output is
        // unrecoverable data corruption. The check mirrors the one inside the
        // cfg-gated check_invariants() so that release builds without the feature
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
        }

        // Zone temperature NaN: silently propagating a NaN zone temperature
        // corrupts output recording, equipment control, and downstream metrics
        // for the remainder of the simulation. Every non-finite zone is logged
        // before quarantining.
        {
            let mut any_nan = false;
            for zone in &self.latest_env.zones {
                if !zone.temperature_c.is_finite() {
                    any_nan = true;
                    tracing::error!(
                        zone_id = %zone.id,
                        temperature_c = zone.temperature_c,
                        "zone temperature is NaN — quarantining dwelling"
                    );
                    #[cfg(feature = "observe")]
                    {
                        self.nan_temperature_count += 1;
                    }
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

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        self.check_invariants(dt)?;

        // Push observer step snapshot after invariant checks so moisture invariant
        // capture data (populated in check_invariants) is available for the snapshot.
        #[cfg(feature = "observe")]
        if let Some(buf) = &mut self.observer_buf {
            obs_phases.post_zone_update =
                Some(observer_capture::capture_zone_update(&self.latest_env));
            #[cfg(all(
                feature = "observe",
                any(debug_assertions, feature = "check_invariants")
            ))]
            let moisture_invariant = if self.invariant_moisture_capture.is_empty() {
                None
            } else {
                Some(MoistureInvariantCapture {
                    zones: std::mem::take(&mut self.invariant_moisture_capture),
                })
            };
            #[cfg(not(all(
                feature = "observe",
                any(debug_assertions, feature = "check_invariants")
            )))]
            let moisture_invariant: Option<MoistureInvariantCapture> = None;
            buf.push(StepSnapshot {
                step_index: self.clock.current_step(),
                timestamp: self.latest_env.current_time,
                phases: obs_phases,
                actor_skips,
                actor_calls,
                moisture_invariant,
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

        for (i, entry) in self.zone_temp_scratch.iter_mut().enumerate() {
            entry.1 = if let Some(env_idx) = self.zone_env_indices[i] {
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
            zone_temperatures_c: self.zone_temp_scratch.clone(),
            hvac_heating_w,
            hvac_cooling_w,
            gas_power_w,
        };

        // Step 5: record outputs to disk (when enabled) and accumulate step results.
        if record_output {
            #[cfg(feature = "profiling")]
            let io_started = Instant::now();
            self.record_step(&step_result)?;
            #[cfg(feature = "profiling")]
            {
                let elapsed = io_started.elapsed();
                step_io = Some(elapsed);
                self.profiling.io += elapsed;
                self.profiling.memory_high_water_kb = self
                    .profiling
                    .memory_high_water_kb
                    .max(current_process_hwm_kb());
            }
        }
        #[cfg(feature = "profiling")]
        {
            let accounted = step_envelope.unwrap_or_default()
                + step_hvac.unwrap_or_default()
                + step_schedule.unwrap_or_default()
                + step_io.unwrap_or_default();
            let elapsed = step_started.elapsed();
            if elapsed > accounted {
                self.profiling.other += elapsed - accounted;
            }

            let alloc_after = hot_path_alloc_counter();
            if alloc_after > alloc_before {
                self.profiling.hot_path_alloc_violations += 1;
                debug_assert_eq!(
                    alloc_after, alloc_before,
                    "hot-path allocation detected during timestep"
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
        self.prior_electrical_summary = ElectricalSummary {
            pv_generation_kw: -pv_kw,
            actual_pv_kw: -pv_kw,
            base_load_kw: power_w_to_kw(self.ports.electrical.load_power_w)
                - battery_kw.max(0.0)
                - ev_kw,
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
        self.prev_equipment_modes.clear();
        self.prev_equipment_modes.extend(
            self.latest_env
                .equipment_core
                .iter()
                .map(|(&id, co)| (id, co.state.operating_mode)),
        );

        // ORDERING: ports.zero() must come AFTER check_invariants() (called above)
        // because the electrical balance check reads self.ports.electrical.load_power_w
        // and generation_power_w to compute the ZIP-adjusted port net.
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            self.thermal_consistency_flag =
                self.verify_per_zone_thermal_consistency(self.clock.current_step());
        }
        self.ports.zero();
        let _ = self.clock.next();

        self.simulation_results.steps.push(step_result);

        // Invariant: the dwelling RNG must have been consumed during this
        // timestep.  A zero-delta means run_timestep never called
        // advance_dwelling_rng, which would make checkpoint captures
        // reflect a dead RNG state.
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
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

        Ok(())
    }

    fn record_step(&mut self, step: &StepResult) -> Result<()> {
        self.record_scratch.fill(0.0);
        let row = &mut self.record_scratch;

        if let Some(&idx) = self.output_column_index.get("Total Electric Power (kW)") {
            row[idx] = step.net_electric_power_kw;
        }
        if let Some(&idx) = self
            .output_column_index
            .get("Total Gas Power (therms/hour)")
        {
            let gas_w = self.ports.fuel.get(hares_types::FuelType::Gas);
            row[idx] = gas_w / GAS_THERMS_PER_HOUR_TO_W;
        }
        if let Some(&idx) = self.output_column_index.get("Total Reactive Power (kVAR)") {
            row[idx] = self.electrical_solver.net_reactive_kvar();
        }
        #[cfg(feature = "observe")]
        if let Some(&idx) = self.output_column_index.get("port_rollback_count") {
            row[idx] = self.rolled_back_port_equipment as f64;
        }
        #[cfg(feature = "observe")]
        if let Some(&idx) = self.output_column_index.get("nan_temperature_count") {
            row[idx] = self.nan_temperature_count as f64;
        }

        // Per-equipment columns via pre-resolved index map.
        for (eq, cols) in self.equipment.iter().zip(&self.equipment_column_map) {
            let co = eq.core_output();
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
        }

        // Per-EndUse aggregate electric power columns.
        // Each equipment's electric power is accumulated into the aggregate column
        // for its EndUse category (e.g. all HVAC_HEATING equipment contribute to
        // "HVAC Heating End Use Electric Power (kW)").
        for (eq, &agg_idx_opt) in self.equipment.iter().zip(&self.end_use_aggregate_indices) {
            if let Some(idx) = agg_idx_opt {
                let co = eq.core_output();
                row[idx] += co.flows.electric_kw.map_or(0.0, |e| e.net_consumption_kw());
            }
        }

        // Actor telemetry columns: pre-resolved column indices avoid
        // per-timestep format!() allocation (parallel to equipment_column_map).
        for (actor, pre_resolved) in self.actors.iter().zip(&self.actor_column_map) {
            if let Some(tel) = actor.telemetry() {
                for (key, column_idx) in pre_resolved {
                    if let Some(value) = tel.get(key) {
                        row[*column_idx] = value;
                    }
                }
            }
        }

        // Zone temperature columns -- direct index lookup, no allocations.
        for (i, &(_, temp_c)) in self.zone_temp_scratch.iter().enumerate() {
            if let Some(idx) = self.zone_temp_col_indices[i] {
                row[idx] = temp_c;
            }
        }
        // Fallback for old-style column name
        if let Some(&idx) = self.output_column_index.get("Indoor Temperature (C)")
            && let Some(&(_, temp_c)) = self.zone_temp_scratch.first()
        {
            row[idx] = temp_c;
        }

        if let Some(&idx) = self.output_column_index.get("Outdoor Dry Bulb (C)") {
            row[idx] = self.latest_env.weather.outdoor_temp_c;
        }

        // Envelope, boundary, and HVAC thermal gains from the thermal solver.
        let gains = self.thermal_solver.component_gains();
        let net_sensible_indoor_w = gains.window_solar_w
            + gains.infiltration_w
            + gains.ventilation_w
            + gains.natural_ventilation_w
            + gains.internal_gain_w
            + gains.jacket_loss_w
            + gains.duct_loss_w
            + gains.opaque_solar_lwr_w
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
            ("Net Sensible Heat Gain - Indoor (W)", net_sensible_indoor_w),
            ("Internal Heat Gain - Indoor (W)", gains.internal_gain_w),
            ("Interior LWR Exchange - Indoor (W)", gains.interior_lwr_w),
            (
                "Opaque Surface Heat Gain - Indoor (W)",
                gains.opaque_solar_lwr_w,
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
        ];
        for &(col_name, value) in envelope_cols {
            if let Some(&idx) = self.output_column_index.get(col_name) {
                row[idx] = value;
            }
        }

        if let Some(&idx) = self.output_column_index.get("Temperature - Ground (C)") {
            row[idx] = self.latest_env.weather.ground_temp_c;
        }
        if let Some(&idx) = self
            .output_column_index
            .get("Hot Water Mains Temperature (C)")
        {
            row[idx] = self.latest_env.weather.mains_temp_c;
        }

        for &(zone, value) in &gains.infiltration_by_zone {
            if let Some(&idx) = self.zone_infiltration_columns.get(&zone) {
                row[idx] = value;
            }
        }

        for &(zone, value) in &gains.interior_lwr_by_zone {
            if let Some(&idx) = self.zone_lwr_columns.get(&zone) {
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
            if let Some(&(heat_idx, cool_idx)) = self.zone_hvac_columns.get(&thermal.zone) {
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
    /// Active when `cfg(any(debug_assertions, feature = "check_invariants"))`.
    /// Returns `Err(HaresError::InvariantViolation { .. })` on the first violation;
    /// the engine then quarantines this dwelling rather than propagating a panic.
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    fn check_invariants(&mut self, dt: StdDuration) -> Result<()> {
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

        // SOC bounds for storage equipment.
        for eq in &self.equipment {
            let end_use = &eq.descriptor().end_use;
            if *end_use != EndUse::BATTERY && *end_use != EndUse::EV {
                continue;
            }
            if let Some(soc) = eq.core_output().state.soc.map(|s| s.get()) {
                checker.check_soc(soc, 0.0)?;
            }
        }

        // Electrical finiteness — screened before any residual computation.
        let net_kw = self.electrical_solver.net_active_kw();
        checker.check_nan_screen(self.clock.current_step(), &[("net_kw", None, net_kw)])?;

        // Electrical balance: solver net must match ZIP-adjusted port accumulation.
        // The solver applies ZIP load scaling (`net_active_kw() = P_load·scale + P_gen`).
        // Adjust the port-side load accumulation by the same scale factor for a
        // like-for-like comparison; without this, a non-default ZIP model at non-nominal
        // voltage produces a false-positive residual of P_load·(scale − 1).
        let scale = self
            .electrical_solver
            .zip_load_scale(self.latest_env.grid.voltage_pu);
        let port_net = power_w_to_kw(self.ports.electrical.load_power_w) * scale
            + power_w_to_kw(self.ports.electrical.generation_power_w);
        checker.check_electrical(net_kw, &[-port_net])?;

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
            #[cfg(debug_assertions)]
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
            #[cfg(not(debug_assertions))]
            {
                checker.check_thermal(q_gains, delta_e_storage, q_loss)?;
            }
        }

        // HVAC delivered-energy non-negativity: per-step and cumulative checks.
        // These fire before the historical clamping `max(0.0)` / `abs()` masked
        // sign errors in equipment port contributions or thermal solver gains.
        // Skipped during warm-up: the initial thermal transients can produce
        // temporarily negative delivered-energy values before the solver converges.
        if !self.is_warming_up {
            let indoor_zone = self.thermal_solver.config().indoor_zone_id;
            let gains = self.thermal_solver.component_gains();
            let step = self.clock.current_step();
            let dt_h = dt_s / SECONDS_PER_HOUR;
            let (heating_w, cooling_w) = {
                #[cfg(debug_assertions)]
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
                #[cfg(not(debug_assertions))]
                {
                    (gains.hvac_heating_w, gains.hvac_cooling_w)
                }
            };
            checker.check_heating_accumulator(step, indoor_zone, heating_w, cooling_w, dt_h)?;
            checker.check_hvac_power_non_negative(step, indoor_zone, heating_w, cooling_w)?;
        }

        // Moisture balance: mass conservation across the humidity solver.
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
        #[cfg(all(
            feature = "observe",
            any(debug_assertions, feature = "check_invariants")
        ))]
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
        // Moisture invariants are skipped during warm-up because the initial
        // humidity ratio from the HPXML model can produce large transients that
        // false-positive the sorption bound check. Convergence is driven by
        // zone temperatures; moisture tracks temperature once the thermal
        // solver has settled.
        if self.is_warming_up {
            return Ok(());
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
            // Skip if humidity ratio change exceeds 50% of the zone moisture level.
            // Such large jumps only occur during initialization transients (warm-up)
            // when the initial HPXML humidity ratio is far from the steady-state value
            // determined by the thermal and moisture solvers. The sorption bound
            // is calibrated for steady-state operation and false-positives
            // on these startup transients.
            if d_w.abs() > 0.5 * w_new.max(w_old) {
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
            )?;

            #[cfg(all(
                feature = "observe",
                any(debug_assertions, feature = "check_invariants")
            ))]
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
#[cfg(any(debug_assertions, feature = "check_invariants"))]
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

/// Build actor instances from equipment seeds.
///
/// Pure function for testability -- takes equipment, existing actors,
/// tariff availability, price schedule, and returns new built-in actors.
fn build_actors_from_seeds(
    equipment: &[Box<dyn Equipment>],
    existing_actors: &[Box<dyn Actor>],
    has_tariff: bool,
    price_schedule: Option<Arc<[f64]>>,
    steps_per_day: usize,
    equipment_id_by_name: &HashMap<String, EquipmentId>,
    rng: &ChaCha8Rng,
) -> Vec<Box<dyn Actor>> {
    let seeds: Vec<(String, ActorSeed)> = equipment
        .iter()
        .filter_map(|eq| {
            eq.actor_seed()
                .map(|seed| (eq.descriptor().name.clone(), seed))
        })
        .collect();

    let existing_names: HashSet<String> = existing_actors
        .iter()
        .map(|a| a.name().to_string())
        .collect();

    let mut built_in_actors: Vec<Box<dyn Actor>> = Vec::new();
    let mut ev_actor_index: u64 = 0;

    #[cfg(any(debug_assertions, feature = "check_invariants"))]
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
                if existing_names.contains(&actor_name) {
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
                    bms_mode,
                    grid_export_rule,
                    max_charge_kw,
                    max_discharge_kw,
                    price_schedule.clone(),
                    steps_per_day,
                    min_dwell_steps,
                );
                let mut actor = actor;
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
                if existing_names.contains(&actor_name) {
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

                let ev_seed = {
                    let stream = RNG_STREAM_EV_DRIVER_BASE + ev_actor_index;
                    let sub = derive_sub_rng(rng, stream);
                    #[cfg(any(debug_assertions, feature = "check_invariants"))]
                    check_ev_rng_stream_no_collision(
                        &mut seen_rng_pairs,
                        sub.get_seed(),
                        sub.get_stream(),
                        &actor_name,
                    );
                    ev_actor_index += 1;
                    sub
                };
                let mut actor = EvDriverActor::new(
                    &format!("EvDriver:{name}"),
                    &name,
                    strategy,
                    plug_in_policy,
                    ScheduleSource::Constant(30.0),
                    ScheduleSource::Constant(480.0),
                    ScheduleSource::Constant(600.0),
                    None,
                    0.8,
                    fuel_economy_kwh_per_mi,
                    capacity_kwh,
                    max_charge_kw,
                    30.0,
                    20.0,
                    0.0,
                    6.6,
                    ev_seed,
                );

                if let Some(ref prices) = price_schedule {
                    actor = actor.with_price_schedule(Arc::clone(prices), steps_per_day);
                }
                actor.resolve_equipment_id(equipment_id_by_name);

                built_in_actors.push(Box::new(actor));
            }
        }
    }

    built_in_actors
}

#[cfg(test)]
mod tests {
    use super::*;
    use conversions::json_value_to_config_value;
    use hares_control::PriorityTier;
    use hares_equipment::config::ConfigValue;
    use hares_equipment::{Equipment, EquipmentConfig};
    use hares_types::ports::{PortContribution, PortSlots};
    use hares_types::{
        ControlCapabilities, ControlSignal, CoreCapabilities, CoreOutput, DRLevel, EndUse,
        EquipmentDescriptor, EquipmentId, ExecutionStage, FuelType, OperatingMode, PortDeclaration,
        Telemetry, TelemetryField, ZoneId, ZoneState,
    };
    use std::borrow::Cow;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
    use std::time::Duration;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn strip_strings_and_comments(src: &str) -> String {
        let mut out = String::with_capacity(src.len());
        let bytes = src.as_bytes();
        let mut i = 0usize;

        while i < bytes.len() {
            match bytes[i] {
                b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                    out.push(' ');
                    out.push(' ');
                    i += 2;
                    while i < bytes.len() && bytes[i] != b'\n' {
                        out.push(' ');
                        i += 1;
                    }
                }
                b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                    out.push(' ');
                    out.push(' ');
                    i += 2;
                    let mut depth = 1usize;
                    while i < bytes.len() && depth > 0 {
                        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
                            out.push(' ');
                            out.push(' ');
                            i += 2;
                            depth += 1;
                            continue;
                        }
                        if i + 1 < bytes.len() && bytes[i] == b'*' && bytes[i + 1] == b'/' {
                            out.push(' ');
                            out.push(' ');
                            i += 2;
                            depth = depth.saturating_sub(1);
                            continue;
                        }
                        if bytes[i] == b'\n' {
                            out.push('\n');
                        } else {
                            out.push(' ');
                        }
                        i += 1;
                    }
                }
                b'r' => {
                    let mut j = i + 1;
                    while j < bytes.len() && bytes[j] == b'#' {
                        j += 1;
                    }
                    if j < bytes.len() && bytes[j] == b'"' {
                        let hashes = j - (i + 1);
                        for _ in i..=j {
                            out.push(' ');
                        }
                        i = j + 1;
                        loop {
                            if i >= bytes.len() {
                                break;
                            }
                            if bytes[i] == b'"' {
                                let mut matches_hashes = true;
                                for h in 0..hashes {
                                    if i + 1 + h >= bytes.len() || bytes[i + 1 + h] != b'#' {
                                        matches_hashes = false;
                                        break;
                                    }
                                }
                                if matches_hashes {
                                    out.push(' ');
                                    for _ in 0..hashes {
                                        out.push(' ');
                                    }
                                    i += 1 + hashes;
                                    break;
                                }
                            }
                            if bytes[i] == b'\n' {
                                out.push('\n');
                            } else {
                                out.push(' ');
                            }
                            i += 1;
                        }
                    } else {
                        out.push('r');
                        i += 1;
                    }
                }
                b'"' => {
                    out.push(' ');
                    i += 1;
                    while i < bytes.len() {
                        match bytes[i] {
                            b'\\' if i + 1 < bytes.len() => {
                                out.push(' ');
                                out.push(' ');
                                i += 2;
                            }
                            b'"' => {
                                out.push(' ');
                                i += 1;
                                break;
                            }
                            b'\n' => {
                                out.push('\n');
                                i += 1;
                            }
                            _ => {
                                out.push(' ');
                                i += 1;
                            }
                        }
                    }
                }
                b'\'' => {
                    out.push(' ');
                    i += 1;
                    while i < bytes.len() {
                        match bytes[i] {
                            b'\\' if i + 1 < bytes.len() => {
                                out.push(' ');
                                out.push(' ');
                                i += 2;
                            }
                            b'\'' => {
                                out.push(' ');
                                i += 1;
                                break;
                            }
                            b'\n' => {
                                out.push('\n');
                                i += 1;
                            }
                            _ => {
                                out.push(' ');
                                i += 1;
                            }
                        }
                    }
                }
                c => {
                    if c.is_ascii() {
                        out.push(c as char);
                    } else {
                        out.push(' ');
                    }
                    i += 1;
                }
            }
        }

        out
    }

    fn find_matching_brace(src: &str, open_brace: usize) -> Option<usize> {
        let bytes = src.as_bytes();
        let mut depth = 1usize;
        let mut i = open_brace + 1;
        while i < bytes.len() {
            match bytes[i] {
                b'{' => depth += 1,
                b'}' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some(i + 1);
                    }
                }
                _ => {}
            }
            i += 1;
        }
        None
    }

    fn find_excluded_test_module_spans(sanitized: &str) -> Vec<(usize, usize)> {
        let bytes = sanitized.as_bytes();
        let mut spans = Vec::new();
        let mut i = 0usize;
        let mut pending_cfg_test_attr = false;

        while i < bytes.len() {
            if bytes[i].is_ascii_whitespace() {
                i += 1;
                continue;
            }

            if i + 1 < bytes.len() && bytes[i] == b'#' && bytes[i + 1] == b'[' {
                let mut j = i + 2;
                while j < bytes.len() && bytes[j] != b']' {
                    j += 1;
                }
                if j < bytes.len() {
                    let attr = &sanitized[i + 2..j];
                    let compact: String = attr.chars().filter(|c| !c.is_whitespace()).collect();
                    if compact.contains("cfg(test)")
                        || compact.contains("cfg(any(test,")
                        || compact.contains("cfg(all(test,")
                    {
                        pending_cfg_test_attr = true;
                    }
                    i = j + 1;
                    continue;
                }
            }

            if i + 2 < bytes.len()
                && &sanitized[i..i + 3] == "mod"
                && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric() && bytes[i - 1] != b'_')
                && (i + 3 == bytes.len()
                    || !bytes[i + 3].is_ascii_alphanumeric() && bytes[i + 3] != b'_')
            {
                let mut j = i + 3;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                let name_start = j;
                while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                    j += 1;
                }
                if name_start == j {
                    pending_cfg_test_attr = false;
                    i += 3;
                    continue;
                }
                let module_name = &sanitized[name_start..j];
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < bytes.len() && bytes[j] == b'{' {
                    if (module_name == "tests" || pending_cfg_test_attr)
                        && let Some(end) = find_matching_brace(sanitized, j)
                    {
                        spans.push((i, end));
                        i = end;
                        pending_cfg_test_attr = false;
                        continue;
                    }
                    pending_cfg_test_attr = false;
                } else if j < bytes.len() && bytes[j] == b';' {
                    pending_cfg_test_attr = false;
                }
            } else {
                pending_cfg_test_attr = false;
            }

            i += 1;
        }

        spans.sort_unstable_by_key(|(start, _)| *start);
        spans
    }

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

    fn offset_to_line_col(src: &str, offset: usize) -> (usize, usize) {
        let mut line = 1usize;
        let mut col = 1usize;
        for (idx, ch) in src.char_indices() {
            if idx >= offset {
                break;
            }
            if ch == '\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
        }
        (line, col)
    }

    fn is_in_span(offset: usize, spans: &[(usize, usize)]) -> bool {
        spans
            .iter()
            .any(|(start, end)| offset >= *start && offset < *end)
    }

    fn has_allowlist_comment(raw: &str, line_start: usize, line_end: usize) -> bool {
        let line_text = &raw[line_start..line_end];
        if line_text.contains("// allowed:") {
            return true;
        }

        if line_end >= raw.len() {
            return false;
        }
        let next_start = line_end + 1;
        if next_start >= raw.len() {
            return false;
        }
        let next_end = raw[next_start..]
            .find('\n')
            .map_or(raw.len(), |p| next_start + p);
        raw[next_start..next_end].contains("// allowed:")
    }

    #[test]
    fn no_string_telemetry_reads_in_core() {
        let src_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        collect_rs_files(&src_root, &mut files);
        files.sort();

        let banned_patterns = [
            "telemetry().get(",
            "telemetry.get(",
            "telem.get(",
            ".0.get(",
        ];
        let mut hits = Vec::new();

        for path in files {
            let raw = fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("failed reading {}: {e}", path.display()));
            let sanitized = strip_strings_and_comments(&raw);
            let excluded_spans = find_excluded_test_module_spans(&sanitized);

            for pattern in banned_patterns {
                for (idx, _) in sanitized.match_indices(pattern) {
                    if is_in_span(idx, &excluded_spans) {
                        continue;
                    }
                    let line_start = raw[..idx].rfind('\n').map_or(0, |p| p + 1);
                    let line_end = raw[idx..].find('\n').map_or(raw.len(), |p| idx + p);
                    let line_text = &raw[line_start..line_end];
                    if has_allowlist_comment(&raw, line_start, line_end) {
                        continue;
                    }
                    let (line, col) = offset_to_line_col(&raw, idx);
                    let rel = path
                        .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                        .unwrap_or(&path)
                        .display()
                        .to_string();
                    hits.push(format!(
                        "{rel}:{line}:{col}: found `{pattern}` in `{}`",
                        line_text.trim()
                    ));
                }
            }
        }

        assert!(
            hits.is_empty(),
            "found disallowed telemetry string reads in core:\n{}",
            hits.join("\n")
        );
    }

    fn replace_equipment_for_test(dwelling: &mut Dwelling, equipment: Vec<Box<dyn Equipment>>) {
        dwelling.equipment = equipment;
        dwelling.equipment_id_by_name = dwelling
            .equipment
            .iter()
            .map(|eq| (eq.descriptor().name.clone(), eq.descriptor().id))
            .collect();
        dwelling.equipment_execution_order = compute_equipment_execution_order(&dwelling.equipment);
        dwelling.equipment_column_map =
            build_equipment_column_map(&dwelling.equipment, &dwelling.output_column_index);
        dwelling
            .solver_feedback_actor
            .set_dispatch_targets(compute_equipment_dispatch_targets(&dwelling.equipment));
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

        let mut declarations: Vec<PortDeclaration> = Vec::new();
        for eq in &dwelling.equipment {
            declarations.extend_from_slice(eq.ports());
        }
        dwelling.ports = PortSlots::from_declarations(&declarations);
        dwelling.rollback_ports = PortSlots::from_declarations(&declarations);
    }

    struct TestEquipment {
        descriptor: EquipmentDescriptor,
        telemetry: Telemetry,
        last_power_kw: f64,
        last_soc_target: f64,
        last_dr_level: Option<DRLevel>,
        core_output: CoreOutput,
    }

    impl TestEquipment {
        fn new(name: &str, capabilities: ControlCapabilities) -> Self {
            Self {
                descriptor: EquipmentDescriptor {
                    id: EquipmentId(1),
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

        fn apply_control_unchecked(
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
                    core_capabilities: CoreCapabilities::empty(),
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

        fn apply_control_unchecked(
            &mut self,
            _signal: &ControlSignal,
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }
    }

    /// Test equipment that correctly deposits power into ports but
    /// under-reports electric power in `core_output()`, used to verify
    /// the telemetry consistency check catches the discrepancy.
    struct UnderReportingEquipment {
        descriptor: EquipmentDescriptor,
        telemetry: Telemetry,
        core_output: CoreOutput,
        true_power_kw: f64,
        reported_power_kw: f64,
        ports: Vec<PortDeclaration>,
    }

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

    impl Equipment for UnderReportingEquipment {
        fn descriptor(&self) -> &EquipmentDescriptor {
            &self.descriptor
        }

        fn rename(&mut self, name: String) {
            self.descriptor.name = name;
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

        fn apply_control_unchecked(
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
                    core_capabilities: CoreCapabilities::HAS_MODE,
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
            _ports: &mut PortSlots,
        ) -> std::result::Result<(), hares_types::HaresError> {
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

        fn apply_control_unchecked(
            &mut self,
            signal: &ControlSignal,
        ) -> std::result::Result<(), hares_types::HaresError> {
            if let ControlSignal::ModeOverride { mode } = signal {
                self.mode_override = Some(*mode);
            }
            Ok(())
        }
    }

    struct ThermalUnderReportingEquipment {
        descriptor: EquipmentDescriptor,
        telemetry: Telemetry,
        core_output: CoreOutput,
        deposited_thermal_w: f64,
        ports: Vec<PortDeclaration>,
    }

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

    impl Equipment for ThermalUnderReportingEquipment {
        fn descriptor(&self) -> &EquipmentDescriptor {
            &self.descriptor
        }

        fn rename(&mut self, name: String) {
            self.descriptor.name = name;
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

        fn apply_control_unchecked(
            &mut self,
            _signal: &ControlSignal,
        ) -> std::result::Result<(), hares_types::HaresError> {
            Ok(())
        }
    }

    struct WaterHeatingEquipment {
        descriptor: EquipmentDescriptor,
        telemetry: Telemetry,
        core_output: CoreOutput,
        thermal_w: f64,
        ports: Vec<PortDeclaration>,
    }

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

    impl Equipment for WaterHeatingEquipment {
        fn descriptor(&self) -> &EquipmentDescriptor {
            &self.descriptor
        }

        fn rename(&mut self, name: String) {
            self.descriptor.name = name;
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

        fn apply_control_unchecked(
            &mut self,
            _signal: &ControlSignal,
        ) -> std::result::Result<(), HaresError> {
            Ok(())
        }
    }

    struct CoolingWithLatentEquipment {
        descriptor: EquipmentDescriptor,
        telemetry: Telemetry,
        core_output: CoreOutput,
        sensible_w: f64,
        latent_w: f64,
        ports: Vec<PortDeclaration>,
    }

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

    impl Equipment for CoolingWithLatentEquipment {
        fn descriptor(&self) -> &EquipmentDescriptor {
            &self.descriptor
        }

        fn rename(&mut self, name: String) {
            self.descriptor.name = name;
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

        fn apply_control_unchecked(
            &mut self,
            _signal: &ControlSignal,
        ) -> std::result::Result<(), HaresError> {
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
        let telemetry = dwelling.telemetry();

        assert!((telemetry.reactive_power_kvar - 0.75).abs() < 1e-9);
    }

    #[test]
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    fn telemetry_consistency_flag_goes_false_when_equipment_under_reports_power() {
        let base_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/bestest/600.toml");
        let mut dwelling = Dwelling::from_toml_config_with_write_output(&base_path, Some(false))
            .expect("build dwelling");

        let mut eq = UnderReportingEquipment::new("UnderReporter", 3.0, 1.0);
        eq.init(&EquipmentConfig::default(), &dwelling.latest_env)
            .expect("init under-reporting equipment");

        replace_equipment_for_test(&mut dwelling, vec![Box::new(eq)]);

        dwelling.run_timestep(false).expect("dwelling step");
        let telemetry = dwelling.telemetry();

        assert!(
            !telemetry.telemetry_consistency_flag,
            "consistency check should detect equipment reporting 1.0 kW while depositing 3.0 kW into electrical port"
        );
    }

    #[test]
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    fn telemetry_consistency_flag_goes_false_when_thermal_equipment_under_reports() {
        let base_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/bestest/600.toml");
        let mut dwelling = Dwelling::from_toml_config_with_write_output(&base_path, Some(false))
            .expect("build dwelling");

        let mut eq = ThermalUnderReportingEquipment::new("ThermalUnderReporter", 1000.0, 50.0);
        eq.init(&EquipmentConfig::default(), &dwelling.latest_env)
            .expect("init thermal under-reporting equipment");

        replace_equipment_for_test(&mut dwelling, vec![Box::new(eq)]);

        dwelling.run_timestep(false).expect("dwelling step");
        let telemetry = dwelling.telemetry();

        assert!(
            !telemetry.telemetry_consistency_flag,
            "thermal consistency check should detect equipment reporting 50 W while depositing 1000 W into HvacHeating port"
        );
    }

    #[test]
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    fn thermal_consistency_excludes_water_heating_by_end_use() {
        let base_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/bestest/600.toml");
        let mut dwelling = Dwelling::from_toml_config_with_write_output(&base_path, Some(false))
            .expect("build dwelling");

        let mut eq = WaterHeatingEquipment::new("IndirectTank", 1000.0);
        eq.init(&EquipmentConfig::default(), &dwelling.latest_env)
            .expect("init water heating equipment");

        replace_equipment_for_test(&mut dwelling, vec![Box::new(eq)]);

        dwelling.run_timestep(false).expect("dwelling step");
        let telemetry = dwelling.telemetry();

        assert!(
            telemetry.telemetry_consistency_flag,
            "water heating equipment depositing 1000 W via JacketLoss with thermal_output_w=1000 W \
             should be excluded from thermal consistency check (end_use != HVAC_HEATING/COOLING)"
        );
    }

    #[test]
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
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

        dwelling.run_timestep(false).expect("dwelling step");
        let telemetry = dwelling.telemetry();

        assert!(
            telemetry.telemetry_consistency_flag,
            "cooling equipment depositing -1000 W sensible + -200 W latent with \
             thermal_output_w=-1200 W should match when latent is included in port comparison"
        );
    }

    #[test]
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
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

        dwelling.run_timestep(false).expect("dwelling step");
        let telemetry = dwelling.telemetry();

        assert!(
            !telemetry.telemetry_consistency_flag,
            "thermal consistency should detect -200 W latent mismatch when equipment \
             reports thermal_output_w=-1000 W but ports receive -1200 W total"
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
    fn simulate_accumulates_steps_when_write_output_disabled() {
        let toml_path = {
            let mut path = std::env::temp_dir();
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock before UNIX_EPOCH")
                .as_nanos();
            path.push(format!("hares-write-output-off-{nanos}.toml"));
            path
        };

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
wall_area_m2 = 145.0

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
        let _ = fs::remove_file(&toml_path);
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
        let toml_path = {
            let mut path = std::env::temp_dir();
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock before UNIX_EPOCH")
                .as_nanos();
            path.push(format!("hares-unregistered-critical-{nanos}.toml"));
            path
        };

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
wall_area_m2 = 145.0

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
        let _ = fs::remove_file(&toml_path);

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

        let mut warnings = Vec::new();
        let equipment: &mut [Box<dyn Equipment>] = &mut [];
        dispatcher.dispatch_into(equipment, &mut warnings);

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

        let mut warnings = Vec::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings);

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

        let mut warnings = Vec::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings);

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

        let mut warnings = Vec::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings);

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

        let mut warnings = Vec::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings);

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

        let mut warnings = Vec::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings);

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

        let mut warnings = Vec::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings);

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

        let mut warnings = Vec::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings);

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

        let mut warnings = Vec::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings);

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
        actor.set_dispatch_targets(compute_equipment_dispatch_targets(&equipment));

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

        fn apply_control_unchecked(
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
        actor.set_dispatch_targets(compute_equipment_dispatch_targets(&equipment));

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
        let mut warnings = Vec::new();
        dispatcher.dispatch_into(&mut equipment, &mut warnings);

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
        actor.set_dispatch_targets(compute_equipment_dispatch_targets(&equipment));

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
        actor.set_dispatch_targets(compute_equipment_dispatch_targets(&equipment));
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
        actor.set_dispatch_targets(compute_equipment_dispatch_targets(&equipment));
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
        let order = compute_equipment_execution_order(&equipment);

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
        let targets = compute_equipment_dispatch_targets(&equipment);

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
        let order = compute_equipment_execution_order(&equipment);

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

        let mut warnings = Vec::new();
        let mut delivered_count = 0u32;
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq1), Box::new(eq2)];
        dispatcher.begin_step();
        dispatcher.drain_tiers(&mut equipment, &mut warnings, |_, delivered, _, _| {
            if delivered {
                delivered_count += 1;
            }
        });

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

        let mut warnings = Vec::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings);

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

        let mut warnings = Vec::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings);

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

        let mut warnings = Vec::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings);

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

        let mut warnings = Vec::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        let capture = dispatcher.dispatch_into_observed(&mut equipment, &mut warnings);

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

        let mut warnings = Vec::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq1), Box::new(eq2)];
        let capture = dispatcher.dispatch_into_observed(&mut equipment, &mut warnings);

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

        let mut warnings = Vec::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        let capture = dispatcher.dispatch_into_observed(&mut equipment, &mut warnings);

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

        let mut warnings = Vec::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(battery)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings);

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

        let mut warnings = Vec::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(battery)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings);

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

        let mut warnings = Vec::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(battery)];
        let capture = dispatcher.dispatch_into_observed(&mut equipment, &mut warnings);

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

        let mut warnings = Vec::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        let capture = dispatcher.dispatch_into_observed(&mut equipment, &mut warnings);

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

        let mut warnings = Vec::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(battery), Box::new(heater)];
        let capture = dispatcher.dispatch_into_observed(&mut equipment, &mut warnings);

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

        let mut warnings = Vec::new();
        let mut equipment: Vec<Box<dyn Equipment>> =
            vec![Box::new(eq1), Box::new(eq2), Box::new(eq3)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings);

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

        let mut warnings = Vec::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(ev), Box::new(battery)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings);

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
        let mut warnings = Vec::new();

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
        dispatcher.drain_tiers(&mut equipment, &mut warnings, |_, _, _, _| {});
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
        dispatcher.drain_tiers(&mut equipment, &mut warnings, |_, _, _, s| {
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
        let mut warnings = Vec::new();

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
        dispatcher.drain_tiers(&mut equipment, &mut warnings, |_, d, _, _| {
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
        let mut warnings = Vec::new();

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
        dispatcher.drain_tiers(&mut equipment, &mut warnings, |_, _, _, _| {});

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
        dispatcher.drain_tiers(&mut equipment, &mut warnings, |_, _, _, s| {
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
        dispatcher.drain_tiers(&mut equipment, &mut warnings, |_, _, _, s| {
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

        let mut warnings = Vec::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];

        // First dispatch
        dispatcher.dispatch_into(&mut equipment, &mut warnings);
        assert_eq!(equipment[0].telemetry().get(tk::LAST_POWER_KW), Some(5.0));

        // Second dispatch with nothing queued -- queues should be empty
        let mut delivered_count = 0u32;
        dispatcher.begin_step();
        dispatcher.drain_tiers(&mut equipment, &mut warnings, |_, delivered, _, _| {
            if delivered {
                delivered_count += 1;
            }
        });
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
        actor.set_dispatch_targets(compute_equipment_dispatch_targets(&equipment));
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
        actor.set_dispatch_targets(compute_equipment_dispatch_targets(&equipment));
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
        let mut warnings = Vec::new();
        dispatcher.dispatch_into(&mut equipment, &mut warnings);

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

        let mut warnings = Vec::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq1), Box::new(eq2)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings);

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
        fn apply_control_unchecked(
            &mut self,
            signal: &ControlSignal,
        ) -> std::result::Result<(), hares_types::HaresError> {
            self.inner.apply_control_unchecked(signal)
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

    // ---------------------------------------------------------------
    // auto_register tests
    // ---------------------------------------------------------------

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

        let actors = build_actors_from_seeds(
            &[eq],
            &[],
            true,
            Some(Arc::from(vec![0.10; 24])),
            24,
            &std::collections::HashMap::new(),
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

        let actors = build_actors_from_seeds(
            &[eq],
            &[],
            true,
            Some(Arc::from(vec![0.10; 24])),
            24,
            &std::collections::HashMap::new(),
            &derive_dwelling_rng(0, 0),
        );
        assert_eq!(actors.len(), 1);
        assert_eq!(actors[0].name(), "EvDriver:EV1");
    }

    #[test]
    fn manual_mode_no_bms_actor() {
        // Equipment with no ActorSeed (simulates BmsMode::Manual)
        let eq: Box<dyn Equipment> = Box::new(SeedableTestEquipment::new("Battery1", None));

        let actors = build_actors_from_seeds(
            &[eq],
            &[],
            false,
            None,
            24,
            &std::collections::HashMap::new(),
            &derive_dwelling_rng(0, 0),
        );
        assert!(actors.is_empty());
    }

    #[test]
    fn immediate_no_ev_actor() {
        // Equipment with no ActorSeed (simulates ChargingStrategy::Immediate)
        let eq: Box<dyn Equipment> = Box::new(SeedableTestEquipment::new("EV1", None));

        let actors = build_actors_from_seeds(
            &[eq],
            &[],
            false,
            None,
            24,
            &std::collections::HashMap::new(),
            &derive_dwelling_rng(0, 0),
        );
        assert!(actors.is_empty());
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
        let actors = build_actors_from_seeds(
            &[eq],
            &[],
            false,
            None,
            24,
            &std::collections::HashMap::new(),
            &derive_dwelling_rng(0, 0),
        );
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
        let actors = build_actors_from_seeds(
            &[eq],
            &[],
            false,
            None,
            24,
            &std::collections::HashMap::new(),
            &derive_dwelling_rng(0, 0),
        );
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

        let built_in = build_actors_from_seeds(
            &[eq],
            &existing,
            false,
            None,
            24,
            &std::collections::HashMap::new(),
            &derive_dwelling_rng(0, 0),
        );
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

        let built_in = build_actors_from_seeds(
            &[eq],
            &existing,
            false,
            None,
            24,
            &std::collections::HashMap::new(),
            &derive_dwelling_rng(0, 0),
        );
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

        let actors = build_actors_from_seeds(
            &[eq1, eq2],
            &[],
            false,
            None,
            24,
            &std::collections::HashMap::new(),
            &derive_dwelling_rng(0, 0),
        );
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

        let actors = build_actors_from_seeds(
            &[eq1, eq2],
            &[],
            false,
            None,
            24,
            &std::collections::HashMap::new(),
            &derive_dwelling_rng(0, 0),
        );
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

        let actors = build_actors_from_seeds(
            &[eq],
            &[],
            false,
            None,
            24,
            &std::collections::HashMap::new(),
            &derive_dwelling_rng(0, 0),
        );
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

        let actors = build_actors_from_seeds(
            &[eq1, eq2],
            &[],
            false,
            None,
            24,
            &std::collections::HashMap::new(),
            &derive_dwelling_rng(42, 1),
        );
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
        // Convective sensible = 2.0 × 66.0 × 0.70 = 92.4 W
        // Radiative sensible  = 2.0 × 66.0 × 0.30 = 39.6 W
        // Latent              = 2.0 × 51.2 = 102.4 W
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
        };
        validate_sim_config(&sim_config).expect("valid sim config");

        // Point defaults_path to a non-existent directory so the default
        // Occupancy profile is NOT available, and the Occupancy spec has
        // no HPXML extension fractions (only NumberofResidents).
        let empty_defaults = tempfile::tempdir().expect("create empty temp dir");
        let dwelling_config = DwellingConfig {
            hpxml_path: base_path.clone(),
            schedule_path: base_path.clone(),
            weather_path: base_path.clone(),
            defaults_path: Some(empty_defaults.path().to_path_buf()),
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

        let err = match create_equipment_from_spec(&registry, &spec, None) {
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
        let schema = hares_io::build_schema(&[], 2, &[]);
        let column_index = build_output_column_index(&schema);
        let caches = build_zone_column_caches(&zones, &zone_types, ZoneId(10), &column_index);

        let attic_idx = column_index
            .get("Temperature - Attic (C)")
            .copied()
            .expect("schema must include attic temperature column");
        assert!(
            caches.temp_columns.contains(&(ZoneId(3), attic_idx)),
            "attic zone should resolve to the attic output column even when its ZoneId is not 2"
        );
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
        let extended = extend_schema_with_actor_columns(&schema, &actors);
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
            ceiling_height_m: None,
            infiltration_height_m: None,
            floors_above_grade: None,
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
        let toml_path = {
            let mut path = std::env::temp_dir();
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock before UNIX_EPOCH")
                .as_nanos();
            path.push(format!("hares-corrupt-window-{nanos}.toml"));
            path
        };

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
wall_area_m2 = 145.0

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
        let _ = fs::remove_file(&toml_path);

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
    /// all steps returns `HaresError::Dwelling`, not a spurious physics error.
    /// Regression: T-0144 reclassified the step-overflow guard from
    /// `HaresError::Physics` to `HaresError::Dwelling`.
    #[test]
    fn run_timestep_past_end_returns_dwelling_error() {
        let toml_path = {
            let mut path = std::env::temp_dir();
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock before UNIX_EPOCH")
                .as_nanos();
            path.push(format!("hares-step-overflow-{nanos}.toml"));
            path
        };

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
wall_area_m2 = 145.0

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
        let _ = fs::remove_file(&toml_path);

        let total = dwelling.clock.total_steps();
        for _ in 0..total {
            dwelling.run_timestep(false).expect("step within bounds");
        }

        let err = dwelling
            .run_timestep(false)
            .expect_err("step past end must error");
        assert!(
            matches!(err, HaresError::Dwelling(_)),
            "step-past-end error must be HaresError::Dwelling, got {:?}",
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
        use hares_types::FluidType;

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
        let toml_path = unique_temp_toml("save_checkpoint_no_panic");
        write_minimal_toml(&toml_path);
        let _guard = TempFile(toml_path.clone());

        let mut dwelling = Dwelling::from_toml_config(&toml_path).expect("build dwelling");
        dwelling.step().expect("step succeeds");
        let result = dwelling.save_checkpoint();
        assert!(
            result.is_ok(),
            "save_checkpoint should succeed on minimal dwelling"
        );
    }

    #[test]
    fn save_checkpoint_propagates_equipment_error() {
        // Build a minimal dwelling, inject equipment whose save_state() always
        // fails, and verify save_checkpoint() returns Err rather than panicking.
        let toml_path = unique_temp_toml("save_checkpoint_failure");
        write_minimal_toml(&toml_path);
        let _guard = TempFile(toml_path.clone());

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
            fn apply_control_unchecked(
                &mut self,
                _: &ControlSignal,
            ) -> std::result::Result<(), HaresError> {
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

    // Helpers for save_checkpoint tests
    fn nanos_suffix() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before UNIX epoch")
            .as_nanos()
    }

    fn unique_temp_toml(tag: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("hares-checkpoint-{tag}-{}.toml", nanos_suffix()));
        path
    }

    fn write_minimal_toml(path: &PathBuf) {
        let content = r#"building_id = 9001

[simulation]
start_time = "2024-06-15T12:00:00Z"
time_res_s = 60
duration_s = 600

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0
wall_area_m2 = 145.0

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

    struct TempFile(PathBuf);
    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
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

        fn apply_control_unchecked(
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
                    core_capabilities: CoreCapabilities::empty(),
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

        fn apply_control_unchecked(
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

        fn apply_control_unchecked(
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

    /// Two dwellings built from the same config (with warmup) and simulated
    /// produce identical step results — verifying deterministic reproducibility.
    #[test]
    fn same_seed_produces_identical_simulation_with_warmup() {
        let toml_path_a = {
            let mut path = std::env::temp_dir();
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock before UNIX_EPOCH")
                .as_nanos();
            path.push(format!("hares-core-repro-a-{nanos}.toml"));
            path
        };
        let toml_path_b = {
            let mut path = std::env::temp_dir();
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock before UNIX_EPOCH")
                .as_nanos();
            path.push(format!("hares-core-repro-b-{nanos}.toml"));
            path
        };

        let toml_content = r#"building_id = 2002

[simulation]
start_time = "2024-01-15T00:00:00Z"
time_res_s = 3600
duration_s = 172800
initialization_duration_s = 86400

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0
wall_area_m2 = 145.0

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

        let _ = fs::remove_file(&toml_path_a);
        let _ = fs::remove_file(&toml_path_b);

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
        let toml_path_a = {
            let mut path = std::env::temp_dir();
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock before UNIX_EPOCH")
                .as_nanos();
            path.push(format!("hares-core-nowu-repro-a-{nanos}.toml"));
            path
        };
        let toml_path_b = {
            let mut path = std::env::temp_dir();
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock before UNIX_EPOCH")
                .as_nanos();
            path.push(format!("hares-core-nowu-repro-b-{nanos}.toml"));
            path
        };

        let toml_content = r#"building_id = 2003

[simulation]
start_time = "2024-01-15T00:00:00Z"
time_res_s = 60
duration_s = 600

[geometry]
floor_area_m2 = 48.0
zone_volume_m3 = 120.0
wall_area_m2 = 145.0

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

        let _ = fs::remove_file(&toml_path_a);
        let _ = fs::remove_file(&toml_path_b);

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
            ) {
            }
            fn observation_state(&self) -> Vec<f64> {
                self.state.clone()
            }
        }

        let toml_path = {
            let mut path = std::env::temp_dir();
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock before UNIX_EPOCH")
                .as_nanos();
            path.push(format!("hares-custom-solver-capture-{nanos}.toml"));
            path
        };

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
wall_area_m2 = 145.0

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
        let _ = std::fs::remove_file(&toml_path);

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
            ) {
            }
        }

        let toml_path = {
            let mut path = std::env::temp_dir();
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock before UNIX_EPOCH")
                .as_nanos();
            path.push(format!("hares-coexist-capture-{nanos}.toml"));
            path
        };

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
wall_area_m2 = 145.0

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
        let _ = std::fs::remove_file(&toml_path);

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
        let toml_path = unique_temp_toml("rng_advance");
        write_minimal_toml(&toml_path);
        let _guard = TempFile(toml_path.clone());

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
        let toml_path_a = unique_temp_toml("rng_repro_a");
        write_minimal_toml(&toml_path_a);
        let toml_path_b = unique_temp_toml("rng_repro_b");
        fs::copy(&toml_path_a, &toml_path_b).expect("copy TOML");
        let _guard_a = TempFile(toml_path_a.clone());
        let _guard_b = TempFile(toml_path_b.clone());

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
wall_area_m2 = 145.0

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
        let toml_path_a = unique_temp_toml("rng_stoch_repro_a");
        write_event_load_toml(&toml_path_a, 42);
        let toml_path_b = unique_temp_toml("rng_stoch_repro_b");
        fs::copy(&toml_path_a, &toml_path_b).expect("copy TOML");
        let _guard_a = TempFile(toml_path_a.clone());
        let _guard_b = TempFile(toml_path_b.clone());

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
        let toml_path_a = unique_temp_toml("rng_smoke_seed_a");
        write_event_load_toml(&toml_path_a, 42);
        let toml_path_b = unique_temp_toml("rng_smoke_seed_b");
        write_event_load_toml(&toml_path_b, 99);
        let _guard_a = TempFile(toml_path_a.clone());
        let _guard_b = TempFile(toml_path_b.clone());

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
wall_area_m2 = 145.0

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
        let toml_path_a = unique_temp_toml("rng_sched_repro_a");
        write_event_load_schedule_toml(&toml_path_a, 42);
        let toml_path_b = unique_temp_toml("rng_sched_repro_b");
        fs::copy(&toml_path_a, &toml_path_b).expect("copy TOML");
        let _guard_a = TempFile(toml_path_a.clone());
        let _guard_b = TempFile(toml_path_b.clone());

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
        let toml_path_a = unique_temp_toml("rng_ckpt_a");
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
wall_area_m2 = 145.0

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
        let _guard_a = TempFile(toml_path_a.clone());

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

        let cp_path = unique_temp_toml("rng_ckpt_json");
        let _cp_guard = TempFile(cp_path.clone());
        checkpoint.save(&cp_path).expect("write checkpoint file");
        let loaded_cp = DwellingCheckpoint::load(&cp_path).expect("load checkpoint");

        let mut dwelling_b = Dwelling::from_toml_config(&toml_path_a).expect("build dwelling B");
        dwelling_b
            .load_checkpoint(loaded_cp)
            .expect("restore checkpoint");

        let mut restarted_steps = Vec::new();
        loop {
            match dwelling_b.step() {
                Ok(step) => restarted_steps.push(step),
                Err(_) => break,
            }
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
}
