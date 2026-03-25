//! Dwelling orchestrator: integrates environment, equipment, solvers, and output.

mod conversions;
mod solver_builder;
mod synthetic;

pub use conversions::{building_to_boundary_inputs, building_to_zone_inputs, stage_rank};

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Duration as StdDuration;
#[cfg(feature = "profiling")]
use std::time::Instant;

use chrono::{DateTime, Duration, FixedOffset};
use hares_control::{DispatchRequest, DispatchTarget, PriceSignal};
use hares_envelope::{ElectricalSolver, FluidSolver, HumiditySolver, ThermalSolver};
use hares_equipment::{Equipment, EquipmentRegistry};
use hares_io::{
    Building, DefaultsStore, ScheduleTimeSeries, SimulationConfig, StreamingRecorder,
    WeatherTimeSeries, build_schema, parse_hpxml, parse_schedule_csv, parse_weather,
    resolve_equipment,
};
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

use crate::checkpoint::{CHECKPOINT_VERSION, DwellingCheckpoint};
use crate::invariants::InvariantChecker;
use crate::telemetry::DwellingTelemetry;
use crate::{EnvironmentManager, SimClock, derive_dwelling_rng};

#[cfg(feature = "observe")]
use crate::observer::{EquipmentObservation, ObserverBuffer, PhaseSnapshots, StepSnapshot};
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
#[derive(Debug, Default)]
struct ControlDispatcher {
    queued: VecDeque<DispatchRequest>,
}

impl ControlDispatcher {
    fn queue(&mut self, request: DispatchRequest) {
        self.queued.push_back(request);
    }

    fn dispatch_into(&mut self, equipment: &mut [Box<dyn Equipment>], warnings: &mut Vec<String>) {
        while let Some(request) = self.queued.pop_front() {
            match request.target {
                DispatchTarget::ByName(name) => {
                    let mut delivered = false;
                    for eq in equipment.iter_mut() {
                        if eq.descriptor().name == name {
                            delivered = true;
                            if let Err(err) = eq.apply_control(&request.signal) {
                                warnings.push(format!(
                                    "control apply failed for '{}' : {err}",
                                    eq.descriptor().name
                                ));
                            }
                        }
                    }
                    if !delivered {
                        warnings.push(format!("control target not found by name: {name}"));
                    }
                }
                DispatchTarget::ByEndUse(end_use) => {
                    let mut delivered = false;
                    for eq in equipment.iter_mut() {
                        if eq.descriptor().end_use == end_use {
                            delivered = true;
                            if let Err(err) = eq.apply_control(&request.signal) {
                                warnings.push(format!(
                                    "control apply failed for '{}' : {err}",
                                    eq.descriptor().name
                                ));
                            }
                        }
                    }
                    if !delivered {
                        warnings.push(format!(
                            "control target not found by end-use: {:?}",
                            end_use
                        ));
                    }
                }
            }
        }
    }
}

/// Top-level single-dwelling simulation orchestrator.
pub struct Dwelling {
    pub bldg_id: i64,
    pub equipment: Vec<Box<dyn Equipment>>,
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

    control_dispatcher: ControlDispatcher,
    price_signal: PriceSignal,
    latest_env: EnvironmentState,
    simulation_results: SimulationResults,
    custom_domain_solvers: Vec<Box<dyn DomainSolver>>,
    stage_snapshot: Option<StageSnapshot>,
    output_column_index: HashMap<String, usize>,
    /// Number of numeric columns expected by the recorder (schema fields minus timestamp).
    output_value_count: usize,
    /// Schedule column index for the occupancy time series, or `None` if the
    /// schedule does not include an occupancy column.
    occupancy_column_idx: Option<usize>,
    /// Per-zone thermal capacitances [J/K] for lightweight gain-preview between
    /// non-thermal and thermal equipment passes.
    #[expect(dead_code, reason = "reserved for gain-preview pass")]
    zone_capacitances_j_k: Vec<(ZoneId, f64)>,
    #[cfg(feature = "profiling")]
    profiling: DwellingProfilingSummary,
    #[cfg(feature = "observe")]
    observer_buf: Option<ObserverBuffer>,
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
        let mut environment =
            EnvironmentManager::new_with_resample(
                weather,
                schedule,
                &building,
                time_res,
                local_start,
                config.sim_config.civil_timezone.as_deref(),
                config.resample_overrides.as_ref(),
            )
                .map_err(|err| {
                    HaresError::Io(format!("environment initialization failed: {err}"))
                })?;

        let occupancy_column_idx = environment.occupancy_column_idx();

        let initial_env = environment.update(&clock, &[]);

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

        let (
            mut thermal_solver,
            humidity_solver,
            electrical_solver,
            fluid_solver,
            zone_capacitances_j_k,
        ) = build_default_solvers(
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
        if building.heating_weekday_setpoints_c.is_some()
            && building.cooling_weekday_setpoints_c.is_some()
        {
            let indoor_zone = thermal_solver.config().indoor_zone_id;
            thermal_solver.set_ideal_hvac_zones(vec![indoor_zone]);
        }

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
            match eq.init(&merged_cfg, &initial_env) {
                Ok(()) => equipment.push(eq),
                Err(err) => {
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

        let rng = derive_dwelling_rng(config.sim_config.master_seed, config.bldg_id);

        let mut dwelling = Self {
            bldg_id: config.bldg_id,
            equipment,
            thermal_solver,
            humidity_solver,
            electrical_solver,
            fluid_solver,
            clock: clock.clone(),
            environment,
            ports,
            recorder,
            rng,
            warnings,
            control_dispatcher: ControlDispatcher::default(),
            price_signal: PriceSignal::default(),
            latest_env: initial_env,
            simulation_results: SimulationResults::default(),
            custom_domain_solvers: Vec::new(),
            stage_snapshot: None,
            output_column_index,
            output_value_count,
            occupancy_column_idx,
            zone_capacitances_j_k,
            #[cfg(feature = "profiling")]
            profiling: DwellingProfilingSummary::default(),
            #[cfg(feature = "observe")]
            observer_buf: None,
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
            target: DispatchTarget::ByName(name.to_string()),
            signal,
        });
    }

    /// Queues a control signal by end-use category.
    pub fn queue_end_use_control(&mut self, end_use: EndUse, signal: ControlSignal) {
        self.control_dispatcher.queue(DispatchRequest {
            target: DispatchTarget::ByEndUse(end_use),
            signal,
        });
    }

    /// Queues a typed dispatch request.
    pub fn queue_dispatch(&mut self, request: DispatchRequest) {
        self.control_dispatcher.queue(request);
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
    /// Accumulates occupancy-driven internal heat gains into zone thermal ports.
    ///
    /// Reads the current occupancy count from the schedule payload carried in
    /// `latest_env.custom_domains`, then for every declared thermal zone injects:
    ///   - sensible convective: `n_occupants × OCCUPANT_SENSIBLE_GAIN_W × OCCUPANT_CONVECTIVE_FRACTION`
    ///   - latent:              `n_occupants × OCCUPANT_LATENT_GAIN_W`
    ///
    /// If no occupancy column is present in the schedule the method returns without
    /// side-effects, preserving backward-compatibility with synthetic TOML inputs.
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

        // Step 2: dispatch queued controls.
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
        let mut indices: Vec<usize> = (0..self.equipment.len()).collect();
        indices.sort_by_key(|&idx| stage_rank(self.equipment[idx].descriptor().stage));

        #[cfg(feature = "observe")]
        let observing = self.observer_buf.is_some();
        #[cfg(feature = "observe")]
        let mut nonthermal_obs: Vec<EquipmentObservation> = Vec::new();
        #[cfg(feature = "observe")]
        let mut pre_snapshot = if observing {
            Some(self.ports.clone())
        } else {
            None
        };

        for &idx in &indices {
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

        for &idx in &indices {
            if self.equipment[idx].descriptor().stage != ExecutionStage::Thermal {
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

        // Per-equipment power columns: "{Name} Electric Power (kW)"
        for eq in &self.equipment {
            let name = &eq.descriptor().name;
            let col_key = format!("{name} Electric Power (kW)");
            if let Some(&idx) = self.output_column_index.get(&col_key) {
                let telem = eq.telemetry();
                let kw = telem
                    .get("electric_kw")
                    .or_else(|| telem.get("active_power_kw"))
                    .or_else(|| telem.get("ac_power_kw"))
                    .unwrap_or(0.0);
                row[idx] = kw;
            }
            // Gas power column
            let gas_col_key = format!("{name} Gas Power (therms/hour)");
            if let Some(&idx) = self.output_column_index.get(&gas_col_key) {
                let telem = eq.telemetry();
                let gas_w = telem.get("fuel_input_w").unwrap_or(0.0);
                row[idx] = gas_w / GAS_THERMS_PER_HOUR_TO_W;
            }
            // Mode column
            let mode_col_key = format!("{name} Mode (-)");
            if let Some(&idx) = self.output_column_index.get(&mode_col_key) {
                let telem = eq.telemetry();
                let mode = telem.get("mode").unwrap_or(0.0);
                row[idx] = mode;
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
    use hares_equipment::config::ConfigValue;

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
}
