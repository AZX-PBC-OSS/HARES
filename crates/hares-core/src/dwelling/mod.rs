//! Dwelling orchestrator: integrates environment, equipment, solvers, and output.

mod conversions;
mod solver_builder;
mod synthetic;

pub use conversions::{building_to_boundary_inputs, building_to_zone_inputs, stage_rank};

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration as StdDuration;
#[cfg(any(feature = "profiling", feature = "actor_profiling"))]
use std::time::Instant;

use chrono::{DateTime, Duration, FixedOffset};
use hares_control::{DispatchRequest, DispatchTarget, PRIORITY_TIER_COUNT, PriceSignal};
#[cfg(any(debug_assertions, feature = "observe_detailed"))]
use hares_envelope::EnvelopeDiagnostics;
use hares_envelope::{ElectricalSolver, FluidSolver, HumiditySolver, ThermalSolver};
use hares_equipment::{Equipment, EquipmentRegistry};
use hares_io::{
    Building, DefaultsStore, ScheduleTimeSeries, SimulationConfig, StreamingRecorder,
    WeatherTimeSeries, build_schema, parse_hpxml, parse_schedule_csv, parse_weather,
    resolve_equipment,
};
use hares_physics::pv_sizing::RoofInfo;
use hares_physics::constants::{
    GAS_THERMS_PER_HOUR_TO_W, OCCUPANT_CONVECTIVE_FRACTION, OCCUPANT_LATENT_GAIN_W,
    OCCUPANT_SENSIBLE_GAIN_W,
};
use hares_types::{
    ControlSignal, DomainSolver, DomainUpdate, EndUse, EnvironmentState, ExecutionStage, GridState,
    HaresError, PortDeclaration, PortSlots, SCHEDULE_DOMAIN_ID, THERMAL, ThermalCategory, ZoneId,
};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use serde_json::{Map, Value};

use crate::actors::SolverFeedbackActor;
use crate::checkpoint::{CHECKPOINT_VERSION, DwellingCheckpoint};
use crate::invariants::InvariantChecker;
use crate::telemetry::DwellingTelemetry;
use crate::{Actor, EnvironmentManager, SimClock, derive_dwelling_rng};

#[cfg(feature = "observe")]
use crate::observer::{
    DispatchCapture, DispatchedSignal, EquipmentObservation, ObserverBuffer, PhaseSnapshots,
    StepSnapshot,
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
}

/// Snapshot of accumulated port totals at a stage boundary.
#[derive(Debug, Clone)]
struct StageSnapshot {
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
            }
        })
        .collect()
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
}

/// Accumulated simulation outputs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SimulationResults {
    pub steps: Vec<StepResult>,
}

/// Internal control queue and routing logic.
///
/// Signals are bucketed into tier queues on `queue()` and drained low→high in
/// `dispatch_into()`. Because every signal fires (no deduplication), the
/// highest-priority tier writes last and wins. Equipment `apply_control` must
/// be overwrite-safe (idempotent set, not accumulate).
struct ControlDispatcher {
    by_tier: [VecDeque<DispatchRequest>; PRIORITY_TIER_COUNT],
    /// Scratch buffer for conflict detection — tracks (target, tier_index).
    /// Pre-allocated, cleared each step. Linear scan for typical <16 signals.
    seen_targets: Vec<(DispatchTarget, usize)>,
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

    fn dispatch_into(&mut self, equipment: &mut [Box<dyn Equipment>], warnings: &mut Vec<String>) {
        self.drain_tiers(equipment, warnings, |_, _, _| {});
    }

    #[cfg(feature = "observe")]
    fn dispatch_into_observed(
        &mut self,
        equipment: &mut [Box<dyn Equipment>],
        warnings: &mut Vec<String>,
    ) -> DispatchCapture {
        let mut signals = Vec::new();
        self.drain_tiers(equipment, warnings, |request, delivered, overwrote| {
            signals.push(DispatchedSignal {
                target: request.target.clone(),
                signal: request.signal.clone(),
                priority: request.priority,
                overwrote_earlier: overwrote,
                delivered,
            });
        });
        DispatchCapture { signals }
    }

    fn drain_tiers(
        &mut self,
        equipment: &mut [Box<dyn Equipment>],
        warnings: &mut Vec<String>,
        mut on_signal: impl FnMut(&DispatchRequest, bool, bool),
    ) {
        self.seen_targets.clear();

        for (tier_idx, tier_que) in self.by_tier.iter_mut().enumerate() {
            for request in tier_que.drain(..) {
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
                on_signal(&request, delivered, overwrote);
            }
        }
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
                warnings.push(format!("control target not found by name: {name}"));
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
fn register_pv_surfaces(
    specs: &[hares_io::EquipmentSpec],
    env: &mut EnvironmentManager,
) {
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
                area_m2: 1.0, // area irrelevant for Perez — only orientation matters
            });
        }
    }
}

/// Auto-attach PV arrays to the closest matching roof boundary by orientation.
/// Sets `attached_boundary_id` on matching specs so the thermal model can
/// account for PV shading.
fn attach_pv_to_roofs(
    specs: &mut [hares_io::EquipmentSpec],
    building: &Building,
) {
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
        let capacity_kw = spec
            .parameters
            .get("capacity_kw")
            .and_then(|v| v.as_f64());

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

/// Top-level single-dwelling simulation orchestrator.
pub struct Dwelling {
    pub bldg_id: i64,
    equipment: Vec<Box<dyn Equipment>>,
    pub thermal_solver: ThermalSolver,
    pub humidity_solver: HumiditySolver,
    pub electrical_solver: ElectricalSolver,
    pub fluid_solver: FluidSolver,
    pub clock: SimClock,
    pub environment: EnvironmentManager,
    pub ports: PortSlots,
    pub recorder: StreamingRecorder,
    pub rng: ChaCha8Rng,
    pub warnings: Vec<String>,

    /// Roof geometry extracted from HPXML at construction time.
    pub roof_info: RoofInfo,
    /// Wall azimuths from HPXML, for PV sizing fallback orientation.
    pub wall_azimuths: Vec<f64>,
    /// Site latitude from HPXML, for PV sizing.
    pub latitude_deg: Option<f64>,
    /// Facility type from HPXML, for roof shape inference.
    pub facility_type: Option<String>,

    control_dispatcher: ControlDispatcher,
    price_signal: PriceSignal,
    latest_env: EnvironmentState,
    simulation_results: SimulationResults,
    custom_domain_solvers: Vec<Box<dyn DomainSolver>>,
    stage_snapshot: Option<StageSnapshot>,
    output_column_index: HashMap<String, usize>,
    /// Pre-resolved output column indices for each equipment piece, avoiding
    /// per-timestep name allocation in `record_step`.
    equipment_column_map: Vec<EquipmentColumns>,
    /// Number of numeric columns expected by the recorder (schema fields minus timestamp).
    output_value_count: usize,
    /// Schedule column index for the occupancy time series, or `None` if the
    /// schedule does not include an occupancy column.
    occupancy_column_idx: Option<usize>,
    /// Per-zone thermal capacitances [J/K] for lightweight gain-preview between
    /// non-thermal and thermal equipment passes.
    #[expect(dead_code, reason = "reserved for gain-preview pass")]
    zone_capacitances_j_k: Vec<(ZoneId, f64)>,
    /// Actor decision-makers that emit control signals each timestep.
    /// Actors execute in registration order. Signals are dispatched by PriorityTier.
    actors: Vec<Box<dyn Actor>>,
    /// Pre-allocated buffer for actor dispatch requests, reused each step.
    actor_dispatch_buf: Vec<DispatchRequest>,
    /// Solver feedback actor: bridges thermal solver to IdealHvac equipment.
    /// Stored separately (not in actors Vec) so dwelling can call collect_and_solve().
    solver_feedback_actor: SolverFeedbackActor,
    /// Pre-computed equipment execution order (sorted by stage rank).
    /// Computed once at init time, reused each timestep.
    equipment_execution_order: Vec<usize>,
    #[cfg(feature = "profiling")]
    profiling: DwellingProfilingSummary,
    #[cfg(feature = "actor_profiling")]
    per_actor_timing: Vec<(String, StdDuration)>,
    #[cfg(feature = "observe")]
    observer_buf: Option<ObserverBuffer>,
    #[cfg(any(debug_assertions, feature = "observe_detailed"))]
    envelope_diagnostics: EnvelopeDiagnostics,
}

impl Dwelling {
    /// Builds a dwelling from HPXML + schedule/weather paths and simulation config.
    pub fn from_config(config: DwellingConfig) -> Result<Self> {
        let building = parse_hpxml(&config.hpxml_path)
            .map_err(|err| HaresError::Io(format!("HPXML parse failed: {err}")))?;

        let weather = parse_weather(&config.weather_path)
            .map_err(|err| HaresError::Io(format!("weather parse failed: {err}")))?;

        let schedule_raw =
            parse_schedule_csv(&config.schedule_path, &[], Some(&weather.meta), None)
                .map_err(|err| HaresError::Io(format!("schedule parse failed: {err}")))?;

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
            output_format: hares_io::OutputFormat::Csv,
            output_chunk_size: 10_000,
            setpoint_deadband_c: None,
            master_seed: 0,
            civil_timezone: None,
        };

        let config = DwellingConfig {
            hpxml_path: hpxml_path.to_path_buf(),
            schedule_path: schedule_path.to_path_buf(),
            weather_path: weather_path.to_path_buf(),
            defaults_path: None,
            sim_config,
            overrides,
            bldg_id: 0,
            initialization_duration: None,
            resample_overrides: None,
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
        let toml_str = std::fs::read_to_string(path)
            .map_err(|err| HaresError::Io(format!("failed to read TOML config: {err}")))?;
        let config: SyntheticTomlConfig = toml::from_str(&toml_str)
            .map_err(|err| HaresError::Io(format!("failed to parse TOML config: {err}")))?;

        let sim_config = SimulationConfig {
            start_time: config.simulation.start_time,
            duration: Duration::seconds(config.simulation.duration_s),
            time_res: Duration::seconds(config.simulation.time_res_s),
            output_verbosity: config.output.output_verbosity,
            output_path: config.output.output_path.as_ref().map(PathBuf::from),
            output_format: config.output.output_format,
            output_chunk_size: config.output.output_chunk_size,
            setpoint_deadband_c: None,
            master_seed: config.output.master_seed,
            civil_timezone: None,
        };
        validate_sim_config(&sim_config)?;

        let hpxml_building = build_synthetic_building(&config);
        let schedule = build_synthetic_schedule(&config)?;
        let weather = build_synthetic_weather(&config, path)?;
        let dwelling_config = DwellingConfig {
            hpxml_path: path.to_path_buf(),
            schedule_path: path.to_path_buf(),
            weather_path: path.to_path_buf(),
            defaults_path: None,
            sim_config,
            overrides: config.overrides.clone(),
            bldg_id: config.building_id.unwrap_or(0),
            initialization_duration: None,
            resample_overrides: None,
        };

        Self::from_preparsed(dwelling_config, hpxml_building, weather, schedule)
    }

    fn from_preparsed(
        config: DwellingConfig,
        building: Building,
        weather: WeatherTimeSeries,
        schedule: ScheduleTimeSeries,
    ) -> Result<Self> {
        // Reinterpret the user's start time in the weather file's local timezone.
        // The naive wall-clock components (year, month, day, hour, minute, second)
        // are preserved and the offset is replaced with the EPW file's timezone.
        let tz_offset =
            chrono::FixedOffset::east_opt((weather.meta.timezone_offset_h * 3600.0) as i32)
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
        let mut clock = SimClock::new(
            local_start,
            config.sim_config.time_res,
            config.sim_config.duration + init_chrono,
        );

        let time_res = chrono_to_std_duration(config.sim_config.time_res)?;
        let weather_avgs = compute_weather_averages(&weather);
        let mut environment = EnvironmentManager::new_with_resample(
            weather,
            schedule,
            &building,
            time_res,
            local_start,
            config.sim_config.civil_timezone.as_deref(),
            config.resample_overrides.as_ref(),
        )
        .map_err(|err| HaresError::Io(format!("environment initialization failed: {err}")))?;

        let occupancy_column_idx = environment.occupancy_column_idx();

        let mut warnings = Vec::new();
        let defaults_dir = config
            .defaults_path
            .clone()
            .unwrap_or_else(|| PathBuf::from("defaults"));
        let defaults = match DefaultsStore::load(&defaults_dir) {
            Ok(store) => store,
            Err(err) => {
                warnings.push(format!(
                    "defaults load failed; using empty defaults store: {err}"
                ));
                DefaultsStore::empty()
            }
        };

        let empty_overrides = Value::Object(Map::new());
        let mut equipment_specs = resolve_equipment(&building, &defaults, &empty_overrides)
            .map_err(|e| HaresError::Io(e.to_string()))?;

        eprintln!("DEBUG: equipment_specs count = {}", equipment_specs.len());

        // Register PV surfaces with the environment so Perez irradiance is
        // computed for PV orientations (which may not match any envelope surface).
        register_pv_surfaces(&equipment_specs, &mut environment);

        // Auto-attach PV arrays to the closest matching roof surface and
        // register shading coverage on attached roofs.
        attach_pv_to_roofs(&mut equipment_specs, &building);
        register_pv_roof_shading(&equipment_specs, &building, &mut environment);

        let initial_env = environment.update(&clock, &[]);

        let solvers = build_default_solvers(
            &initial_env,
            &config.sim_config,
            &building,
            &defaults,
            &weather_avgs,
            &equipment_specs,
        )?;

        // Enable ideal HVAC on the indoor zone when both heating AND cooling
        // setpoints are configured — the thermal solver back-calculates the exact
        // load needed to maintain the setpoint at each timestep.
        hares_io::inject_schedule_into_specs(
            &mut equipment_specs,
            environment.schedule_mut(),
            Some(&defaults_dir),
        );
        let override_root = config
            .overrides
            .clone()
            .unwrap_or_else(|| Value::Object(Map::new()));

        // Equipment names whose loads are handled outside the registry (e.g. directly in the
        // simulation loop) — silently skip them rather than emitting a warning.
        const HANDLED_OUTSIDE_REGISTRY: &[&str] = &["Occupancy"];

        let registry = EquipmentRegistry::new();
        let mut equipment: Vec<Box<dyn Equipment>> = Vec::new();
        for spec in &equipment_specs {
            if HANDLED_OUTSIDE_REGISTRY.contains(&spec.name.as_str()) {
                continue;
            }
            let base_cfg = equipment_config_from_spec(spec);
            eprintln!(
                "DEBUG: Creating equipment: name={}, ochre_class={}",
                base_cfg.name, base_cfg.ochre_class
            );
            let mut eq = match registry.create(&base_cfg.ochre_class, base_cfg.clone()) {
                Ok(eq) => eq,
                Err(err) => {
                    // Equipment class not yet implemented in registry; skip with warning.
                    let msg = format!(
                        "equipment '{}' (class '{}') not available, skipping: {err}",
                        base_cfg.name, base_cfg.ochre_class
                    );
                    tracing::warn!("{msg}");
                    warnings.push(msg);
                    continue;
                }
            };

            let merged_cfg = merged_equipment_config(spec, &override_root);
            eprintln!("DEBUG: Initializing equipment: name={}", merged_cfg.name);
            match eq.init(&merged_cfg, &initial_env) {
                Ok(()) => {
                    eprintln!(
                        "DEBUG: Equipment initialized successfully: {}",
                        merged_cfg.name
                    );
                    equipment.push(eq)
                }
                Err(err) => {
                    eprintln!(
                        "DEBUG: Equipment init FAILED: name={}, err={}",
                        merged_cfg.name, err
                    );
                    // Non-critical equipment (appliances, loads) may lack schedule data;
                    // skip them with a warning rather than aborting the entire simulation.
                    let msg = format!(
                        "equipment '{}' init failed, skipping: {err}",
                        merged_cfg.name
                    );
                    tracing::warn!("{msg}");
                    warnings.push(msg);
                }
            }
        }

        let mut declarations: Vec<PortDeclaration> = Vec::new();
        for eq in &equipment {
            declarations.extend_from_slice(eq.ports());
        }
        for zone in &initial_env.zones {
            declarations.push(PortDeclaration {
                port_type: hares_types::PortType::Thermal,
                zone: Some(zone.id),
                loop_id: None,
                domain_id: None,
                fluid_type: None,
            });
        }
        let ports = PortSlots::from_declarations(&declarations);

        let schema = build_schema(&equipment_specs, config.sim_config.output_verbosity);
        let output_value_count = schema.fields().len() - 1; // exclude timestamp
        let output_column_index = build_output_column_index(&schema);
        let output_path = config
            .sim_config
            .output_path
            .clone()
            .unwrap_or_else(|| default_output_path(&config));
        let recorder = StreamingRecorder::new(
            schema,
            config.sim_config.output_chunk_size,
            config.sim_config.output_format,
            &output_path,
        )
        .map_err(|err| HaresError::Io(format!("output recorder init failed: {err}")))?;

        let (roof_info, wall_azimuths) = hares_io::pv_sizing::extract_roof_info(&building);
        let latitude_deg = building.site.latitude_deg;
        let facility_type = building.residential_facility_type.clone();

        let rng = derive_dwelling_rng(config.sim_config.master_seed, config.bldg_id);

        let equipment_column_map = build_equipment_column_map(&equipment, &output_column_index);
        let equipment_execution_order = compute_equipment_execution_order(&equipment);
        let mut solver_feedback_actor = SolverFeedbackActor::new();
        solver_feedback_actor.set_dispatch_targets(compute_equipment_dispatch_targets(&equipment));

        let mut dwelling = Self {
            bldg_id: config.bldg_id,
            equipment,
            thermal_solver: solvers.thermal,
            humidity_solver: solvers.humidity,
            electrical_solver: solvers.electrical,
            fluid_solver: solvers.fluid,
            clock: clock.clone(),
            environment,
            ports,
            recorder,
            rng,
            warnings,
            roof_info,
            wall_azimuths,
            latitude_deg,
            facility_type,
            control_dispatcher: ControlDispatcher::default(),
            price_signal: PriceSignal::default(),
            latest_env: initial_env,
            simulation_results: SimulationResults::default(),
            custom_domain_solvers: Vec::new(),
            stage_snapshot: None,
            equipment_column_map,
            output_column_index,
            output_value_count,
            occupancy_column_idx,
            zone_capacitances_j_k: solvers.zone_capacitances_j_k,
            actors: Vec::new(),
            actor_dispatch_buf: Vec::with_capacity(16),
            solver_feedback_actor,
            equipment_execution_order,
            #[cfg(feature = "profiling")]
            profiling: DwellingProfilingSummary::default(),
            #[cfg(feature = "actor_profiling")]
            per_actor_timing: Vec::new(),
            #[cfg(feature = "observe")]
            observer_buf: None,
            #[cfg(any(debug_assertions, feature = "observe_detailed"))]
            envelope_diagnostics: solvers.envelope_diagnostics,
        };

        if let Some(init_dur) = config.initialization_duration {
            dwelling.run_warmup(init_dur)?;
            clock = SimClock::new(
                local_start,
                config.sim_config.time_res,
                config.sim_config.duration,
            );
            dwelling.clock = clock;
        }

        Ok(dwelling)
    }

    /// Runs the full configured horizon and returns accumulated results.
    pub fn simulate(&mut self) -> Result<SimulationResults> {
        while self.clock.current_step() < self.clock.total_steps() {
            let _ = self.run_timestep(true)?;
        }
        self.recorder
            .flush()
            .map_err(|err| HaresError::Io(format!("output flush failed: {err}")))?;
        Ok(self.simulation_results.clone())
    }

    /// Returns accumulated results gathered so far.
    #[must_use]
    pub fn results(&self) -> SimulationResults {
        self.simulation_results.clone()
    }

    /// Executes exactly one simulation timestep.
    pub fn step(&mut self) -> Result<StepResult> {
        self.run_timestep(true)
    }

    /// Queues a control signal for one equipment instance by name.
    pub fn apply_control(&mut self, name: &str, signal: ControlSignal) {
        self.control_dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from(name)),
            signal,
            priority: Default::default(),
        });
    }

    /// Queues a control signal by end-use category.
    pub fn queue_end_use_control(&mut self, end_use: EndUse, signal: ControlSignal) {
        self.control_dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByEndUse(end_use),
            signal,
            priority: Default::default(),
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
        self.actors.push(actor);
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
        self.actors.push(actor);
        Ok(())
    }

    /// Returns the number of registered actors.
    #[must_use]
    pub fn actor_count(&self) -> usize {
        self.actors.len()
    }

    /// Returns a slice of equipment for read-only access.
    #[must_use]
    pub fn equipment(&self) -> &[Box<dyn Equipment>] {
        &self.equipment
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
    pub fn add_equipment(&mut self, eq: Box<dyn Equipment>) {
        self.equipment.push(eq);
        self.refresh_equipment_caches();
    }

    /// Removes all equipment and refreshes internal caches.
    pub fn clear_equipment(&mut self) {
        self.equipment.clear();
        self.refresh_equipment_caches();
    }

    /// Refreshes internal caches after equipment list modification.
    pub fn refresh_equipment_caches(&mut self) {
        self.equipment_execution_order = compute_equipment_execution_order(&self.equipment);
        self.solver_feedback_actor
            .set_dispatch_targets(compute_equipment_dispatch_targets(&self.equipment));
    }

    /// Returns per-actor timing from the simulation (requires `actor_profiling` feature).
    #[cfg(feature = "actor_profiling")]
    #[must_use]
    pub fn actor_timing(&self) -> &[(String, StdDuration)] {
        &self.per_actor_timing
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

        let mut setpoint_heat_c = zone_temperatures_c.clone();
        let mut setpoint_cool_c = zone_temperatures_c.clone();
        let mut equipment_names = Vec::with_capacity(self.equipment.len());
        let mut equipment_modes = Vec::with_capacity(self.equipment.len());
        let mut equipment_states = Vec::with_capacity(self.equipment.len());
        let mut equipment_soc = Vec::with_capacity(self.equipment.len());
        let mut equipment_power_kw = Vec::with_capacity(self.equipment.len());

        for eq in &self.equipment {
            let telemetry = eq.telemetry();
            equipment_names.push(eq.descriptor().name.clone());
            equipment_modes.push(telemetry.get("mode").unwrap_or(0.0));
            equipment_states.push(telemetry.get("state").unwrap_or(0.0));
            equipment_soc.push(
                telemetry
                    .get("soc")
                    .or_else(|| telemetry.get("state_of_charge"))
                    .unwrap_or(0.0),
            );
            equipment_power_kw.push(
                telemetry
                    .get("electric_power_kw")
                    .or_else(|| telemetry.get("power_kw"))
                    .or_else(|| telemetry.get("active_power_kw"))
                    .or_else(|| telemetry.get("net_power_kw"))
                    .unwrap_or(0.0),
            );

            if let Some(zone_id) = eq.descriptor().zone
                && let Some(zone_idx) = zone_ids.iter().position(|z| *z == zone_id)
            {
                if let Some(heat_sp) = telemetry.get("heating_setpoint_c") {
                    setpoint_heat_c[zone_idx] = heat_sp;
                }
                if let Some(cool_sp) = telemetry.get("cooling_setpoint_c") {
                    setpoint_cool_c[zone_idx] = cool_sp;
                }
            }
        }

        DwellingTelemetry {
            timestep_index: self.clock.current_step(),
            current_time: self.latest_env.current_time,
            zone_names,
            zone_temperatures_c,
            equipment_names,
            equipment_modes,
            equipment_states,
            equipment_soc,
            equipment_power_kw,
            setpoint_heat_c,
            setpoint_cool_c,
            total_power_kw: self.electrical_solver.net_active_kw(),
            outdoor_temp_c: self.latest_env.weather.outdoor_temp_c,
            outdoor_rh: self.latest_env.weather.outdoor_humidity_ratio,
        }
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
    #[cfg(feature = "observe")]
    pub fn enable_observer(&mut self, capacity: usize) {
        self.observer_buf = Some(ObserverBuffer::new(capacity));
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
        self.recorder.flushed_batches()
    }

    /// Snapshot current simulation state to an in-memory checkpoint struct.
    #[must_use]
    pub fn save_checkpoint(&self) -> DwellingCheckpoint {
        let (envelope_state, thermal_last_u, lwr_t_prev_c) = self.thermal_solver.snapshot_state();
        let humidity_states: Vec<(ZoneId, f64)> = self
            .humidity_solver
            .humidity_ratios
            .iter()
            .map(|(zone, value)| (*zone, *value))
            .collect();
        let fluid_states = self.fluid_solver.snapshot_payload();

        DwellingCheckpoint {
            format_version: CHECKPOINT_VERSION,
            bldg_id: self.bldg_id,
            timestep_index: self.clock.current_step(),
            equipment_states: self
                .equipment
                .iter()
                .map(|eq| (eq.descriptor().id, eq.save_state()))
                .collect(),
            rng_state: self.rng.get_seed(),
            envelope_state,
            humidity_states,
            fluid_states,
            rng_stream: self.rng.get_stream(),
            rng_word_pos: self.rng.get_word_pos(),
            thermal_last_u,
            lwr_t_prev_c,
        }
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

        let states_by_id: HashMap<_, _> = cp.equipment_states.into_iter().collect();
        for eq in &mut self.equipment {
            let state = states_by_id.get(&eq.descriptor().id).ok_or_else(|| {
                HaresError::Io(format!(
                    "checkpoint missing equipment state for id {:?}",
                    eq.descriptor().id
                ))
            })?;
            eq.load_state(state)?;
        }

        self.thermal_solver
            .restore_state(&cp.envelope_state, &cp.thermal_last_u, &cp.lwr_t_prev_c)
            .map_err(|err| HaresError::Envelope(format!("restore thermal state failed: {err}")))?;

        let checkpoint_zones: HashMap<ZoneId, f64> = cp.humidity_states.into_iter().collect();
        for zone in &self.latest_env.zones {
            let humidity = checkpoint_zones.get(&zone.id).ok_or_else(|| {
                HaresError::Io(format!(
                    "checkpoint missing humidity state for zone {:?}",
                    zone.id
                ))
            })?;
            self.humidity_solver
                .humidity_ratios
                .insert(zone.id, *humidity);
        }
        self.fluid_solver
            .restore_from_payload(&cp.fluid_states)
            .map_err(|err| HaresError::Envelope(format!("restore fluid state failed: {err}")))?;

        Ok(())
    }

    /// Accumulates occupancy-driven internal heat gains into zone thermal ports.
    ///
    /// Reads the current occupancy count from the schedule payload carried in
    /// `latest_env.custom_domains`, then for every declared thermal zone injects:
    ///   - sensible convective: `n_occupants × OCCUPANT_SENSIBLE_GAIN_W × OCCUPANT_CONVECTIVE_FRACTION`
    ///   - latent:              `n_occupants × OCCUPANT_LATENT_GAIN_W`
    ///
    /// If no occupancy column is present in the schedule the method returns without
    /// side-effects, supporting synthetic TOML inputs that omit occupancy schedules.
    fn apply_occupancy_gains(&mut self) {
        let Some(col_idx) = self.occupancy_column_idx else {
            return;
        };

        let n_occupants = self
            .latest_env
            .custom_domains
            .iter()
            .find(|u| u.domain_id == SCHEDULE_DOMAIN_ID)
            .and_then(|u| u.custom_payload.as_ref())
            .and_then(|p| p.get(col_idx))
            .copied()
            .unwrap_or(0.0);

        if n_occupants <= 0.0 {
            return;
        }

        let sensible_w = n_occupants * OCCUPANT_SENSIBLE_GAIN_W * OCCUPANT_CONVECTIVE_FRACTION;
        let latent_w = n_occupants * OCCUPANT_LATENT_GAIN_W;

        for thermal in &mut self.ports.thermal {
            thermal.add(sensible_w, latent_w, ThermalCategory::InternalGain);
        }
    }

    fn run_warmup(&mut self, initialization_duration: StdDuration) -> Result<()> {
        let warmup_steps = initialization_duration
            .as_secs()
            .checked_div(
                u64::try_from(self.clock.time_res.num_seconds())
                    .map_err(|_| HaresError::Physics("invalid time resolution".to_string()))?,
            )
            .unwrap_or(0);
        for _ in 0..warmup_steps {
            let _ = self.run_timestep(false)?;
        }
        self.simulation_results.steps.clear();
        Ok(())
    }

    fn run_timestep(&mut self, record_output: bool) -> Result<StepResult> {
        if self.clock.current_step() >= self.clock.total_steps() {
            return Err(HaresError::Physics(
                "simulation already reached configured end".to_string(),
            ));
        }

        #[cfg(feature = "profiling")]
        let step_started = Instant::now();
        #[cfg(feature = "profiling")]
        let alloc_before = hot_path_alloc_counter();
        #[cfg(feature = "profiling")]
        let mut step_schedule: Option<StdDuration> = None;
        #[cfg(feature = "profiling")]
        let mut step_hvac: Option<StdDuration> = None;
        #[cfg(feature = "profiling")]
        let mut step_envelope: Option<StdDuration> = None;
        #[cfg(feature = "profiling")]
        let mut step_io: Option<StdDuration> = None;

        #[cfg(feature = "observe")]
        let mut obs_phases = PhaseSnapshots::default();

        // Step 1: update environment at current clock state.
        #[cfg(feature = "profiling")]
        let schedule_started = Instant::now();
        let env = self.environment.update(&self.clock, &self.latest_env.zones);
        self.latest_env = env;

        #[cfg(feature = "observe")]
        if self.observer_buf.is_some() {
            obs_phases.post_environment =
                Some(observer_capture::capture_environment(&self.latest_env));
        }

        #[cfg(feature = "observe")]
        let observing = self.observer_buf.is_some();

        // Step 1b: thermal equipment update_control() to determine mode and ideal targets.
        // Must run BEFORE solver feedback actor collects targets.
        for &idx in &self.equipment_execution_order {
            if self.equipment[idx].descriptor().stage == ExecutionStage::Thermal {
                let _ = self.equipment[idx].update_control(&self.latest_env);
            }
        }

        // Step 1c: solver feedback actor collects ideal targets and solves for capacities.
        self.solver_feedback_actor
            .collect_and_solve(&self.equipment, &self.thermal_solver);

        // Step 1d: actors decide and queue control signals (registration order, last write wins).
        // Solver feedback actor decides first (Schedule priority, can be overridden by user actors).
        self.actor_dispatch_buf.clear();
        self.solver_feedback_actor
            .decide(&self.latest_env, &mut self.actor_dispatch_buf);
        for req in self.actor_dispatch_buf.drain(..) {
            self.control_dispatcher.queue(req);
        }
        #[cfg(feature = "actor_profiling")]
        {
            self.per_actor_timing.clear();
            self.per_actor_timing.reserve(self.actors.len());
            for actor in &mut self.actors {
                let start = Instant::now();
                actor.decide(&self.latest_env, &mut self.actor_dispatch_buf);
                self.per_actor_timing
                    .push((actor.name().to_string(), start.elapsed()));
            }
        }
        #[cfg(not(feature = "actor_profiling"))]
        for actor in &mut self.actors {
            actor.decide(&self.latest_env, &mut self.actor_dispatch_buf);
        }
        for req in self.actor_dispatch_buf.drain(..) {
            self.control_dispatcher.queue(req);
        }

        // Step 2: dispatch queued controls.
        #[cfg(feature = "observe")]
        if self.observer_buf.is_some() {
            let capture = self
                .control_dispatcher
                .dispatch_into_observed(&mut self.equipment, &mut self.warnings);
            obs_phases.post_dispatch = Some(capture);
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

        let dt = chrono_to_std_duration(self.clock.time_res)?;

        // Step 2b: apply occupancy-driven internal heat gains.
        // OCHRE Envelope.py:904-908: 400 BTU/h total; sensible=66 W, latent=51 W.
        // Gains go to the primary (indoor) zone only per OCHRE's single-zone approach.
        self.apply_occupancy_gains();

        // Step 3a: run stage-ordered equipment, snapshot after stage 1.
        #[cfg(feature = "profiling")]
        let hvac_started = Instant::now();

        #[cfg(feature = "observe")]
        let mut nonthermal_obs: Vec<EquipmentObservation> = Vec::new();
        #[cfg(feature = "observe")]
        let mut pre_snapshot = if observing {
            Some(self.ports.clone())
        } else {
            None
        };

        for &idx in &self.equipment_execution_order {
            let stage = self.equipment[idx].descriptor().stage;
            if stage == ExecutionStage::Thermal {
                continue;
            }
            #[cfg(feature = "observe")]
            let pre_ports = pre_snapshot.as_ref().map(observer_capture::capture_ports);

            let _ = self.equipment[idx].update_control(&self.latest_env);
            if let Err(err) = self.equipment[idx].step(&self.latest_env, dt, &mut self.ports) {
                self.warnings.push(format!(
                    "equipment step failed for '{}' : {err}",
                    self.equipment[idx].descriptor().name
                ));
            }

            #[cfg(feature = "observe")]
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
        #[cfg(debug_assertions)]
        {
            self.stage_snapshot = Some(StageSnapshot {
                ports: self.ports.clone(),
            });
        }

        #[cfg(feature = "observe")]
        if observing {
            obs_phases.post_nonthermal_equipment = Some(observer_capture::capture_equipment_phase(
                nonthermal_obs,
                &self.ports,
            ));
        }

        // Step 3b: thermal stage equipment.
        #[cfg(feature = "observe")]
        let mut thermal_obs: Vec<EquipmentObservation> = Vec::new();
        #[cfg(feature = "observe")]
        {
            pre_snapshot = if observing {
                Some(self.ports.clone())
            } else {
                None
            };
        }

        for &idx in &self.equipment_execution_order {
            if self.equipment[idx].descriptor().stage != ExecutionStage::Thermal {
                continue;
            }
            #[cfg(feature = "observe")]
            let pre_ports = pre_snapshot.as_ref().map(observer_capture::capture_ports);

            // update_control() was already called in Step 1b for thermal equipment
            if let Err(err) = self.equipment[idx].step(&self.latest_env, dt, &mut self.ports) {
                self.warnings.push(format!(
                    "equipment step failed for '{}' : {err}",
                    self.equipment[idx].descriptor().name
                ));
            }

            #[cfg(feature = "observe")]
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

        #[cfg(feature = "observe")]
        if observing {
            obs_phases.post_thermal_equipment = Some(observer_capture::capture_equipment_phase(
                thermal_obs,
                &self.ports,
            ));
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

        // Step 4: envelope/domain resolution.
        #[cfg(feature = "profiling")]
        let envelope_started = Instant::now();
        let thermal_update = self
            .thermal_solver
            .resolve(&self.ports, &self.latest_env, dt);
        self.latest_env
            .custom_domains
            .retain(|u| u.domain_id != THERMAL);
        self.latest_env.custom_domains.push(thermal_update.clone());

        let humidity_update = self
            .humidity_solver
            .resolve(&self.ports, &self.latest_env, dt);
        let electrical_update = self
            .electrical_solver
            .resolve(&self.ports, &self.latest_env, dt);
        let fluid_update = self.fluid_solver.resolve(&self.ports, &self.latest_env, dt);

        #[cfg(feature = "observe")]
        let obs_fluid_update = if self.observer_buf.is_some() {
            Some(fluid_update.clone())
        } else {
            None
        };

        self.latest_env
            .custom_domains
            .retain(|u| u.domain_id != humidity_update.domain_id);
        self.latest_env.custom_domains.push(humidity_update.clone());
        self.latest_env
            .custom_domains
            .retain(|u| u.domain_id != electrical_update.domain_id);
        self.latest_env
            .custom_domains
            .push(electrical_update.clone());
        self.latest_env
            .custom_domains
            .retain(|u| u.domain_id != fluid_update.domain_id);
        self.latest_env.custom_domains.push(fluid_update);

        for solver in &mut self.custom_domain_solvers {
            let update = solver.resolve(&self.ports, &self.latest_env, dt);
            self.latest_env
                .custom_domains
                .retain(|u| u.domain_id != update.domain_id);
            self.latest_env.custom_domains.push(update);
        }

        // internal_gain_w is computed directly in the thermal solver from
        // ThermalCategory subtotals — no post-hoc subtraction needed.

        #[cfg(feature = "observe")]
        if let Some(fluid) = obs_fluid_update {
            obs_phases.post_solvers = Some(observer_capture::capture_solvers(
                &thermal_update,
                &humidity_update,
                &electrical_update,
                &fluid,
                &self.thermal_solver,
            ));
        }

        apply_thermal_update_to_zones(&mut self.latest_env, &thermal_update);
        apply_humidity_update_to_zones(&mut self.latest_env, &humidity_update);

        #[cfg(feature = "observe")]
        if let Some(buf) = &mut self.observer_buf {
            obs_phases.post_zone_update =
                Some(observer_capture::capture_zone_update(&self.latest_env));
            buf.push(StepSnapshot {
                step_index: self.clock.current_step(),
                timestamp: self.latest_env.current_time,
                phases: obs_phases,
            });
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

        self.check_invariants(&thermal_update, dt)?;

        let mut zone_temperatures_c: Vec<(ZoneId, f64)> = self
            .latest_env
            .zones
            .iter()
            .map(|z| (z.id, z.temperature_c))
            .collect();
        zone_temperatures_c.sort_by_key(|(z, _)| *z);

        // HVAC thermal delivery: use component_gains which includes both equipment
        // port contributions and ideal HVAC loads from the thermal solver.
        let gains = self.thermal_solver.component_gains();
        let hvac_heating_w = gains.hvac_heating_w.max(0.0);
        let hvac_cooling_w = gains.hvac_cooling_w.abs();

        let step_result = StepResult {
            timestamp: self.latest_env.current_time,
            net_electric_power_kw: self.electrical_solver.net_active_kw(),
            zone_temperatures_c,
            hvac_heating_w,
            hvac_cooling_w,
        };

        // Step 5: record outputs.
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
            self.simulation_results.steps.push(step_result.clone());
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

        // ORDERING: ports.zero() must come AFTER check_invariants() (called above)
        // because the electrical balance check reads self.ports.electrical.net_active_kw().
        self.ports.zero();
        let _ = self.clock.next();

        Ok(step_result)
    }

    fn record_step(&mut self, step: &StepResult) -> Result<()> {
        let mut row = vec![0.0; self.output_value_count];

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

        // Per-equipment columns via pre-resolved index map.
        for (eq, cols) in self.equipment.iter().zip(&self.equipment_column_map) {
            let telem = eq.telemetry();
            if let Some(idx) = cols.electric_power {
                row[idx] = telem
                    .get("electric_kw")
                    .or_else(|| telem.get("active_power_kw"))
                    .or_else(|| telem.get("ac_power_kw"))
                    .unwrap_or(0.0);
            }
            if let Some(idx) = cols.gas_power {
                row[idx] = telem.get("fuel_input_w").unwrap_or(0.0) / GAS_THERMS_PER_HOUR_TO_W;
            }
            if let Some(idx) = cols.mode {
                row[idx] = telem.get("mode").unwrap_or(0.0);
            }
        }

        // Zone temperature columns.
        let indoor_zone = self.thermal_solver.config().indoor_zone_id;
        for (zone_id, temp_c) in &step.zone_temperatures_c {
            let zone_label = zone_display_name(*zone_id, indoor_zone);
            let col_key = format!("Temperature - {zone_label} (C)");
            if let Some(&idx) = self.output_column_index.get(&col_key) {
                row[idx] = *temp_c;
            }
        }
        // Fallback for old-style column name
        if let Some(&idx) = self.output_column_index.get("Indoor Temperature (C)")
            && let Some((_, temp_c)) = step.zone_temperatures_c.first()
        {
            row[idx] = *temp_c;
        }

        // Envelope component gains from the thermal solver (verbosity >= 6).
        let gains = self.thermal_solver.component_gains();
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
            (
                "Internal Heat Gain - Indoor (W)",
                gains.internal_gain_w + gains.jacket_loss_w,
            ),
            ("Radiation Heat Gain - Indoor (W)", gains.interior_lwr_w),
            (
                "Opaque Surface Heat Gain - Indoor (W)",
                gains.opaque_solar_lwr_w,
            ),
            ("Duct Loss Heat Gain - Indoor (W)", gains.duct_loss_w),
            ("HVAC Heating Delivered (W)", gains.hvac_heating_w),
            ("HVAC Cooling Delivered (W)", gains.hvac_cooling_w),
        ];
        for &(col_name, value) in envelope_cols {
            if let Some(&idx) = self.output_column_index.get(col_name) {
                row[idx] = value;
            }
        }

        // Per-zone infiltration columns (e.g., attic zone).
        for &(zone, value) in &gains.infiltration_by_zone {
            if zone == indoor_zone {
                continue; // already written as "Infiltration Heat Gain - Indoor (W)"
            }
            let col_name = format!(
                "Infiltration Heat Gain - {} (W)",
                zone_display_name(zone, indoor_zone)
            );
            if let Some(&idx) = self.output_column_index.get(col_name.as_str()) {
                row[idx] = value;
            }
        }

        // Per-zone interior LWR columns.
        for &(zone, value) in &gains.interior_lwr_by_zone {
            let col_name = format!(
                "Radiation Heat Gain - {} (W)",
                zone_display_name(zone, indoor_zone)
            );
            if let Some(&idx) = self.output_column_index.get(col_name.as_str()) {
                row[idx] = value;
            }
        }

        self.recorder
            .push_row(&step.timestamp.to_rfc3339(), &row)
            .map_err(|err| HaresError::Io(format!("record push failed: {err}")))
    }

    /// Runs per-timestep invariant checks.
    ///
    /// Active when `cfg(any(debug_assertions, feature = "check_invariants"))`.
    /// Returns `Err(HaresError::InvariantViolation { .. })` on the first violation;
    /// the engine then quarantines this dwelling rather than propagating a panic.
    fn check_invariants(&self, thermal_update: &DomainUpdate, dt: StdDuration) -> Result<()> {
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            let checker = InvariantChecker::new();

            // Temporal sanity: dt must be positive and finite.
            let dt_s = dt.as_secs_f64();
            if !dt_s.is_finite() || dt_s <= 0.0 {
                return Err(HaresError::InvariantViolation {
                    check_name: "timestep_dt".to_string(),
                    value: dt_s,
                    tolerance: 0.0,
                });
            }

            // Zone temperature bounds.
            let zone_temps_c: Vec<f64> = thermal_update
                .zone_temperatures_c
                .iter()
                .map(|&(_, t)| t)
                .collect();
            checker.check_temperatures(&zone_temps_c, &[])?;

            // Electrical finiteness.
            let net_kw = self.electrical_solver.net_active_kw();
            if !net_kw.is_finite() {
                return Err(HaresError::InvariantViolation {
                    check_name: "electrical_net_finite".to_string(),
                    value: net_kw,
                    tolerance: 0.0,
                });
            }

            // Electrical balance: solver net must match port accumulation.
            let bus_power = self.ports.electrical.net_active_kw();
            checker.check_electrical(net_kw, &[-bus_power])?;

            // Moisture payload: every humidity value must be finite.
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
        }

        Ok(())
    }
}

/// Maps a ZoneId to its display name for output column labels.
/// The configured indoor zone = "Indoor", ZoneId(2) = "Attic", others = "Zone_{id}".
fn zone_display_name(zone: ZoneId, indoor_zone: ZoneId) -> String {
    if zone == indoor_zone {
        "Indoor".to_string()
    } else {
        match zone.0 {
            2 => "Attic".to_string(),
            n => format!("Zone_{n}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conversions::json_value_to_config_value;
    use hares_control::PriorityTier;
    use hares_equipment::config::ConfigValue;
    use hares_equipment::{Equipment, EquipmentConfig};
    use hares_types::ports::PortSlots;
    use hares_types::{
        ControlCapabilities, ControlSignal, EndUse, EquipmentDescriptor, EquipmentId,
        ExecutionStage, FuelType, OperatingMode, PortDeclaration, Telemetry, TelemetryField,
        ZoneId,
    };
    use std::borrow::Cow;
    use std::time::Duration;

    struct TestEquipment {
        descriptor: EquipmentDescriptor,
        telemetry: Telemetry,
        last_power_kw: f64,
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
                    telemetry_fields: vec![TelemetryField {
                        name: "last_power_kw".to_string(),
                        unit: "kW".to_string(),
                        description: "last applied power".to_string(),
                    }],
                },
                telemetry: Telemetry::with_capacity(1),
                last_power_kw: 0.0,
            }
        }
    }

    impl Equipment for TestEquipment {
        fn descriptor(&self) -> &EquipmentDescriptor {
            &self.descriptor
        }

        fn ports(&self) -> &[PortDeclaration] {
            &[]
        }

        fn init(
            &mut self,
            _config: &EquipmentConfig,
            _env: &hares_types::EnvironmentState,
        ) -> std::result::Result<(), hares_types::HaresError> {
            self.telemetry.insert("last_power_kw", self.last_power_kw);
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

        fn save_state(&self) -> Vec<u8> {
            vec![]
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
            if let ControlSignal::PowerSetpoint {
                active_power_kw, ..
            } = signal
            {
                self.last_power_kw = *active_power_kw;
                self.telemetry.insert("last_power_kw", self.last_power_kw);
            }
            Ok(())
        }
    }

    #[test]
    fn stage_rank_orders_execution_stages() {
        assert!(stage_rank(ExecutionStage::Independent) < stage_rank(ExecutionStage::Electrical));
        assert!(stage_rank(ExecutionStage::Electrical) < stage_rank(ExecutionStage::Thermal));
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
            },
            priority: PriorityTier::Schedule,
        });
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Heater")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 5.0,
                reactive_power_kvar: None,
            },
            priority: PriorityTier::Grid,
        });

        let mut warnings = Vec::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings);

        assert!(warnings.is_empty());
        assert_eq!(equipment[0].telemetry().get("last_power_kw"), Some(5.0));
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
            },
            priority: PriorityTier::Schedule,
        });
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Heater")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 2.0,
                reactive_power_kvar: None,
            },
            priority: PriorityTier::UserOverride,
        });
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Heater")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 3.0,
                reactive_power_kvar: None,
            },
            priority: PriorityTier::Grid,
        });
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Heater")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 0.0,
                reactive_power_kvar: None,
            },
            priority: PriorityTier::Safety,
        });

        let mut warnings = Vec::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings);

        assert!(warnings.is_empty());
        assert_eq!(equipment[0].telemetry().get("last_power_kw"), Some(0.0));
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
            },
            priority: PriorityTier::Schedule,
        });

        let mut warnings = Vec::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings);

        // Should find the equipment and apply the signal
        assert!(warnings.is_empty(), "unexpected warnings: {:?}", warnings);
        assert_eq!(equipment[0].telemetry().get("last_power_kw"), Some(2.5));
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
            ControlSignal::IdealCapacity { capacity_w } => {
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
                    telemetry_fields: vec![TelemetryField {
                        name: "ideal_capacity_w".to_string(),
                        unit: "W".to_string(),
                        description: "ideal capacity from solver".to_string(),
                    }],
                },
                telemetry: Telemetry::with_capacity(1),
                ideal_capacity_w: 0.0,
                ideal_zone: zone,
                ideal_target_c: target_c,
            }
        }
    }

    impl Equipment for TestIdealEquipment {
        fn descriptor(&self) -> &EquipmentDescriptor {
            &self.descriptor
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
                .insert("ideal_capacity_w", self.ideal_capacity_w);
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

        fn save_state(&self) -> Vec<u8> {
            vec![]
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
            if let ControlSignal::IdealCapacity { capacity_w } = signal {
                self.ideal_capacity_w = *capacity_w;
                self.telemetry.insert("ideal_capacity_w", *capacity_w);
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
                .get("ideal_capacity_w")
                .unwrap_or(0.0)
                - 5000.0)
                .abs()
                < 1e-9,
            "equipment should have received 5000W ideal capacity"
        );
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
            },
            priority: PriorityTier::Schedule,
        });
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Eq2")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 2.0,
                reactive_power_kvar: None,
            },
            priority: PriorityTier::Schedule,
        });
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Eq1")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 3.0,
                reactive_power_kvar: None,
            },
            priority: PriorityTier::Grid,
        });

        let mut warnings = Vec::new();
        let mut delivered_count = 0u32;
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq1), Box::new(eq2)];
        dispatcher.drain_tiers(&mut equipment, &mut warnings, |_, delivered, _| {
            if delivered {
                delivered_count += 1;
            }
        });

        assert!(warnings.is_empty(), "unexpected warnings: {:?}", warnings);
        assert_eq!(delivered_count, 3, "all 3 signals must be delivered");
        // Eq1 gets Grid (3.0) as last write, Eq2 gets Schedule (2.0)
        assert_eq!(equipment[0].telemetry().get("last_power_kw"), Some(3.0));
        assert_eq!(equipment[1].telemetry().get("last_power_kw"), Some(2.0));
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
            },
            priority: PriorityTier::Schedule,
        });
        dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByName(Arc::from("Heater")),
            signal: ControlSignal::PowerSetpoint {
                active_power_kw: 9.0,
                reactive_power_kvar: None,
            },
            priority: PriorityTier::Schedule,
        });

        let mut warnings = Vec::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings);

        assert!(warnings.is_empty());
        // Last queued signal in the same tier wins (FIFO within tier, last write wins)
        assert_eq!(equipment[0].telemetry().get("last_power_kw"), Some(9.0));
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
            },
            priority: PriorityTier::Schedule,
        });

        let mut warnings = Vec::new();
        let mut equipment: Vec<Box<dyn Equipment>> =
            vec![Box::new(eq1), Box::new(eq2), Box::new(eq3)];
        dispatcher.dispatch_into(&mut equipment, &mut warnings);

        assert!(warnings.is_empty());
        // Both HVAC_HEATING equipment should receive the signal
        assert_eq!(equipment[0].telemetry().get("last_power_kw"), Some(5.0));
        assert_eq!(equipment[1].telemetry().get("last_power_kw"), Some(5.0));
        // Battery should NOT receive it
        assert_eq!(equipment[2].telemetry().get("last_power_kw"), None);
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
            },
            priority: PriorityTier::Schedule,
        });

        let mut warnings = Vec::new();
        let mut equipment: Vec<Box<dyn Equipment>> = vec![Box::new(eq)];

        // First dispatch
        dispatcher.dispatch_into(&mut equipment, &mut warnings);
        assert_eq!(equipment[0].telemetry().get("last_power_kw"), Some(5.0));

        // Second dispatch with nothing queued — queues should be empty
        let mut delivered_count = 0u32;
        dispatcher.drain_tiers(&mut equipment, &mut warnings, |_, delivered, _| {
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
            ControlSignal::IdealCapacity { capacity_w } => {
                assert!((capacity_w - 3000.0).abs() < 1e-9, "zone 1 → 3000W");
            }
            _ => panic!("expected IdealCapacity"),
        }
        match &requests[1].signal {
            ControlSignal::IdealCapacity { capacity_w } => {
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
            signal: ControlSignal::IdealCapacity { capacity_w: 0.0 },
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
                .get("ideal_capacity_w")
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
            equipment[1].telemetry().get("last_power_kw"),
            Some(7.0),
            "second equipment should still receive signal despite first rejecting"
        );
    }
}
