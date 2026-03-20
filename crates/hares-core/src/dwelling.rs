//! Dwelling orchestrator: integrates environment, equipment, solvers, and output.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Duration as StdDuration;
#[cfg(feature = "profiling")]
use std::time::Instant;

use chrono::{DateTime, Duration, Utc};
use hares_control::{DispatchRequest, DispatchTarget, PriceSignal};
use hares_envelope::{
    BoundaryInput, BuildingRC, EMISSIVITY_DEFAULT, EMISSIVITY_RADIANT_BARRIER, ElectricalSolver,
    SOLAR_ABSORPTANCE_DEFAULT, SOLAR_ABSORPTANCE_RADIANT_BARRIER,
    ElectricalSolverConfig, ExteriorSurfaceInfo, ExteriorTarget, FluidSolver, FluidSolverConfig,
    HumiditySolver, HumiditySolverConfig, LayerInput, ThermalSolver, ThermalSolverConfig,
    WindowSolarProperties, ZoneInput, assemble_building_rc, derive_zone_capacitances,
};
use hares_equipment::{Equipment, EquipmentConfig, EquipmentRegistry, config::ConfigValue};
use hares_io::{
    Building, ColumnAggregation, DefaultsStore, ScheduleTimeSeries, SimulationConfig,
    StreamingRecorder, WeatherMeta, WeatherTimeSeries, build_schema, parse_epw, parse_hpxml,
    parse_schedule_csv, resolve_equipment,
};
use hares_physics::constants::{
    GAS_THERMS_PER_HOUR_TO_W, OCCUPANT_CONVECTIVE_FRACTION, OCCUPANT_LATENT_GAIN_W,
    OCCUPANT_SENSIBLE_GAIN_W,
};
use hares_types::{
    ControlSignal, DomainSolver, DomainUpdate, EndUse, EnvironmentState, ExecutionStage, GridState,
    HaresError, PortDeclaration, PortSlots, SCHEDULE_DOMAIN_ID, THERMAL, ZoneId,
};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::checkpoint::{CHECKPOINT_VERSION, DwellingCheckpoint};
use crate::telemetry::DwellingTelemetry;
use crate::{EnvironmentManager, SimClock, derive_dwelling_rng};

const DEFAULT_GRID_FREQUENCY_HZ: f64 = 60.0;
const DEFAULT_TEMP_SANITY_LOW_C: f64 = -80.0;
const DEFAULT_TEMP_SANITY_HIGH_C: f64 = 90.0;

/// Annual weather averages needed for film coefficient computation.
struct WeatherAverages {
    avg_wind_m_s: f64,
    avg_ambient_c: f64,
    avg_ground_c: f64,
}

fn compute_weather_averages(weather: &WeatherTimeSeries) -> WeatherAverages {
    let n = weather.len().max(1) as f64;
    WeatherAverages {
        avg_wind_m_s: weather.wind_speed_m_s.iter().sum::<f64>() / n,
        avg_ambient_c: weather.dry_bulb_c.iter().sum::<f64>() / n,
        avg_ground_c: weather.ground_temp_c.iter().sum::<f64>() / n,
    }
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
}

/// Snapshot of accumulated port totals at a stage boundary.
#[derive(Debug, Clone)]
struct StageSnapshot {
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
    pub timestamp: DateTime<Utc>,
    pub net_electric_power_kw: f64,
    pub zone_temperatures_c: Vec<(ZoneId, f64)>,
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
    /// non-thermal and thermal equipment passes (FIX-003).
    #[expect(dead_code, reason = "reserved for FIX-003 gain-preview")]
    zone_capacitances_j_k: Vec<(ZoneId, f64)>,
    #[cfg(feature = "profiling")]
    profiling: DwellingProfilingSummary,
}

impl Dwelling {
    /// Builds a dwelling from HPXML + schedule/weather paths and simulation config.
    pub fn from_config(config: DwellingConfig) -> Result<Self> {
        let building = parse_hpxml(&config.hpxml_path)
            .map_err(|err| HaresError::Io(format!("HPXML parse failed: {err}")))?;

        let weather = parse_epw(&config.weather_path)
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
        start_time: DateTime<Utc>,
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
        };
        validate_sim_config(&sim_config)?;

        let hpxml_building = build_synthetic_building(&config);
        let schedule = build_synthetic_schedule(&config)?;
        let weather = build_synthetic_weather(&config);
        let dwelling_config = DwellingConfig {
            hpxml_path: path.to_path_buf(),
            schedule_path: path.to_path_buf(),
            weather_path: path.to_path_buf(),
            defaults_path: None,
            sim_config,
            overrides: config.overrides.clone(),
            bldg_id: config.building_id.unwrap_or(0),
            initialization_duration: None,
        };

        Self::from_preparsed(dwelling_config, hpxml_building, weather, schedule)
    }

    fn from_preparsed(
        config: DwellingConfig,
        building: Building,
        weather: WeatherTimeSeries,
        schedule: ScheduleTimeSeries,
    ) -> Result<Self> {
        let mut clock = SimClock::new(
            config.sim_config.start_time,
            config.sim_config.time_res,
            config.sim_config.duration,
        );

        let time_res = chrono_to_std_duration(config.sim_config.time_res)?;
        let weather_avgs = compute_weather_averages(&weather);
        let mut environment = EnvironmentManager::new(
            weather, schedule, &building, time_res, config.sim_config.start_time,
        )
        .map_err(|err| HaresError::Io(format!("environment initialization failed: {err}")))?;

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

        let (thermal_solver, humidity_solver, electrical_solver, fluid_solver, zone_capacitances_j_k) =
            build_default_solvers(&initial_env, &config.sim_config, &building, &defaults, &weather_avgs)?;

        let empty_overrides = Value::Object(Map::new());
        let mut equipment_specs = resolve_equipment(&building, &defaults, &empty_overrides)
            .map_err(|e| HaresError::Io(e.to_string()))?;
        hares_io::inject_schedule_into_specs(&mut equipment_specs, environment.schedule_mut(), Some(&defaults_dir));
        let override_root = config
            .overrides
            .clone()
            .unwrap_or_else(|| Value::Object(Map::new()));

        let registry = EquipmentRegistry::new();
        let mut equipment: Vec<Box<dyn Equipment>> = Vec::new();
        for spec in &equipment_specs {
            let base_cfg = equipment_config_from_spec(spec);
            let mut eq = match registry.create(&base_cfg.ochre_class, base_cfg.clone()) {
                Ok(eq) => eq,
                Err(err) => {
                    // Equipment class not yet implemented in registry; skip with warning.
                    let msg = format!(
                        "equipment '{}' (class '{}') not available, skipping: {err}",
                        base_cfg.name, base_cfg.ochre_class
                    );
                    eprintln!("[WARN] {msg}");
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
                    eprintln!("[WARN] {msg}");
                    warnings.push(msg);
                }
            }
        }

        // Dump equipment config after init for debugging.
        for eq in &equipment {
            let d = eq.descriptor();
            eprintln!(
                "[init] equip='{}' class='{}' stage={:?}",
                d.name, d.equipment_type, d.stage,
            );
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
        };

        if let Some(init_dur) = config.initialization_duration {
            dwelling.run_warmup(init_dur)?;
            clock = SimClock::new(
                config.sim_config.start_time,
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
        let (envelope_state, thermal_last_u, lwr_t_prev_c) =
            self.thermal_solver.snapshot_state();
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
            thermal.add(sensible_w, latent_w);
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

    #[allow(unused_assignments)]
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

        let step_num = self.clock.current_step();

        // Step 1: update environment at current clock state.
        #[cfg(feature = "profiling")]
        let schedule_started = Instant::now();
        let env = self.environment.update(&self.clock, &self.latest_env.zones);
        self.latest_env = env;


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

        for &idx in &indices {
            let stage = self.equipment[idx].descriptor().stage;
            if stage == ExecutionStage::Thermal {
                continue;
            }
            let _ = self.equipment[idx].update_control(&self.latest_env);
            if let Err(err) = self.equipment[idx].step(&self.latest_env, dt, &mut self.ports) {
                self.warnings.push(format!(
                    "equipment step failed for '{}' : {err}",
                    self.equipment[idx].descriptor().name
                ));
            }
        }
        self.stage_snapshot = Some(StageSnapshot {
            ports: self.ports.clone(),
        });

        // Step 3b: thermal stage equipment.
        for &idx in &indices {
            if self.equipment[idx].descriptor().stage != ExecutionStage::Thermal {
                continue;
            }
            let _ = self.equipment[idx].update_control(&self.latest_env);
            if let Err(err) = self.equipment[idx].step(&self.latest_env, dt, &mut self.ports) {
                self.warnings.push(format!(
                    "equipment step failed for '{}' : {err}",
                    self.equipment[idx].descriptor().name
                ));
            }
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

        // Split HVAC sensible from internal gains.
        // Only zone-conditioning HVAC (HvacHeating/HvacCooling) is subtracted.
        // Water heaters are Thermal stage but heat water, not the zone.
        {
            let hvac_sensible_w: f64 = self
                .equipment
                .iter()
                .filter(|eq| matches!(
                    eq.descriptor().end_use,
                    EndUse::HvacHeating | EndUse::HvacCooling
                ))
                .map(|eq| {
                    let t = eq.telemetry();
                    // Heaters: thermal_output_w (positive = heating zone)
                    // Coolers: sensible_cooling_w (positive magnitude, port gets negative)
                    t.get("thermal_output_w").unwrap_or(0.0)
                        - t.get("sensible_cooling_w").unwrap_or(0.0)
                })
                .sum();
            let gains = self.thermal_solver.component_gains_mut();
            gains.internal_gain_w = gains.port_sensible_w - hvac_sensible_w;
        }

        apply_thermal_update_to_zones(&mut self.latest_env, &thermal_update);
        apply_humidity_update_to_zones(&mut self.latest_env, &humidity_update);


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

        // Diagnostic output for thermal debugging.
        if step_num < 5 || step_num.is_multiple_of(10) {
            let diag = crate::diagnostics::capture(step_num, &self.latest_env, &self.ports);
            let zone_str: String = diag.zone_temps_c.iter()
                .map(|(z, t)| format!("z{}={t:.2}", z.0))
                .collect::<Vec<_>>().join(" ");
            let gain_str: String = diag.thermal_gains_w.iter()
                .map(|(z, g)| format!("z{}={g:.0}W", z.0))
                .collect::<Vec<_>>().join(" ");

            // Per-equipment dump
            let mut eq_lines = Vec::new();
            for eq in &self.equipment {
                let d = eq.descriptor();
                let t = eq.telemetry();
                let mode = t.get("operating_mode").unwrap_or(f64::NAN);
                let elec = t.get("electric_kw").unwrap_or(0.0);
                let therm = t.get("thermal_output_w")
                    .or_else(|| t.get("sensible_cooling_w"))
                    .unwrap_or(0.0);
                let cop = t.get("cop").unwrap_or(f64::NAN);
                if elec.abs() > 0.01 || therm.abs() > 0.01 || mode > 0.5 {
                    eq_lines.push(format!(
                        "    {} mode={mode:.0} elec={elec:.2}kW sens={therm:.0}W cop={cop:.2}",
                        d.name,
                    ));
                }
            }

            eprintln!(
                "[diag s={step_num}] out={:.1}C {zone_str} | gains: {gain_str} | elec={:.2}kW",
                diag.outdoor_temp_c, diag.electrical_net_kw,
            );
            eprintln!(
                "  weather: ghi={:.0} dni={:.0} dhi={:.0} wind={:.1}m/s",
                self.latest_env.weather.ghi_w_m2,
                self.latest_env.weather.dni_w_m2,
                self.latest_env.weather.dhi_w_m2,
                self.latest_env.weather.wind_speed_m_s,
            );
            eprintln!(
                "  zones: {}",
                self.latest_env.zones.iter()
                    .map(|z| format!("z{}(T={:.1} wb={:.1} w={:.5} rh={:.2} vol={:.0}m3)",
                        z.id.0, z.temperature_c, z.wet_bulb_c, z.humidity_ratio, z.relative_humidity, z.volume_m3))
                    .collect::<Vec<_>>().join(" "),
            );
            eprintln!(
                "  thermal_update: {:?}",
                thermal_update.zone_temperatures_c,
            );
            for line in &eq_lines {
                eprintln!("{line}");
            }
        }

        self.debug_assert_invariants(&thermal_update, dt);

        let mut zone_temperatures_c: Vec<(ZoneId, f64)> = self
            .latest_env
            .zones
            .iter()
            .map(|z| (z.id, z.temperature_c))
            .collect();
        zone_temperatures_c.sort_by_key(|(z, _)| *z);

        let step_result = StepResult {
            timestamp: self.latest_env.current_time,
            net_electric_power_kw: self.electrical_solver.net_active_kw(),
            zone_temperatures_c,
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
        if let Some(&idx) = self
            .output_column_index
            .get("Total Reactive Power (kVAR)")
        {
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
                let gas_w = telem.get("gas_consumption_w").unwrap_or(0.0);
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
        // ZoneId(1) is the primary conditioned zone ("Indoor" in OCHRE convention).
        for (zone_id, temp_c) in &step.zone_temperatures_c {
            let zone_label = if zone_id.0 == 1 { "Indoor" } else { &format!("Zone_{}", zone_id.0) };
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
            ("Forced Ventilation Heat Gain - Indoor (W)", gains.ventilation_w),
            ("Natural Ventilation Heat Gain - Indoor (W)", gains.natural_ventilation_w),
            ("Internal Heat Gain - Indoor (W)", gains.internal_gain_w),
            ("Radiation Heat Gain - Indoor (W)", gains.interior_lwr_w),
        ];
        for &(col_name, value) in envelope_cols {
            if let Some(&idx) = self.output_column_index.get(col_name) {
                row[idx] = value;
            }
        }

        self.recorder
            .push_row(&step.timestamp.to_rfc3339(), &row)
            .map_err(|err| HaresError::Io(format!("record push failed: {err}")))
    }

    fn debug_assert_invariants(&self, thermal_update: &DomainUpdate, dt: StdDuration) {
        debug_assert!(self.electrical_solver.net_active_kw().is_finite());

        // Thermal sanity checks.
        for &(_, temp_c) in &thermal_update.zone_temperatures_c {
            debug_assert!(
                (DEFAULT_TEMP_SANITY_LOW_C..=DEFAULT_TEMP_SANITY_HIGH_C).contains(&temp_c),
                "zone temperature out of sanity range: {temp_c} C"
            );
        }

        // Approximate electrical balance: net bus power should equal accumulated bus value.
        let bus_power = self.ports.electrical.net_active_kw();
        debug_assert!((self.electrical_solver.net_active_kw() - bus_power).abs() < 1e-3);

        if let Some(snapshot) = &self.stage_snapshot {
            debug_assert!(snapshot.ports.electrical.net_active_kw().is_finite());
        }

        // Moisture payload sanity: every value should be finite.
        if let Some(update) = self
            .latest_env
            .custom_domains
            .iter()
            .find(|u| u.domain_id == hares_types::HUMIDITY)
            && let Some(payload) = &update.custom_payload
        {
            for value in payload {
                debug_assert!(value.is_finite());
            }
        }

        // Temporal sanity.
        debug_assert!(dt.as_secs_f64().is_finite() && dt.as_secs_f64() > 0.0);
    }
}

/// Solvers plus per-zone thermal capacitances [J/K] for gain preview.
type SolverBundle = (ThermalSolver, HumiditySolver, ElectricalSolver, FluidSolver, Vec<(ZoneId, f64)>);

fn build_default_solvers(
    env: &EnvironmentState,
    sim_config: &SimulationConfig,
    building: &Building,
    defaults: &DefaultsStore,
    weather_avgs: &WeatherAverages,
) -> Result<SolverBundle> {
    use hares_envelope::state_space::{OutputMapping, StateSpaceModel};
    use nalgebra::DMatrix;

    let n_zones = env.zones.len().max(1);

    // Convert building data to envelope-crate input types.
    let zone_inputs = building_to_zone_inputs(building, n_zones);
    let boundary_inputs = building_to_boundary_inputs(
        building, n_zones, defaults,
        weather_avgs.avg_wind_m_s, weather_avgs.avg_ambient_c, weather_avgs.avg_ground_c,
    );

    // Zone air node capacitances [J/K].
    let zone_capacitances = derive_zone_capacitances(&zone_inputs);

    // Build the RC network from material layers where available.
    let rc = assemble_building_rc(&boundary_inputs, n_zones, &zone_capacitances)
        .map_err(HaresError::Envelope)?;

    let BuildingRC {
        a_c,
        b_ext,
        node_index,
        zone_state_rows,
        layer_info,
        outdoor_col,
        n_ext,
        node_capacitances,
        ..
    } = rc;
    let n_states = a_c.nrows();

    // Pre-scan: identify exterior surfaces with RC layers that need per-surface
    // B-matrix input columns for solar/LWR injection at the outer node.
    let mut ext_surface_columns: Vec<(usize, hares_envelope::NodeId, usize)> = Vec::new();
    for (surface_idx, boundary) in building.boundaries.iter().enumerate() {
        let is_exterior = boundary
            .exterior_zone
            .as_ref()
            .map(|z| *z == hares_io::hpxml::ZoneType::Outdoor)
            .unwrap_or(false);
        if !is_exterior {
            continue;
        }
        if let Some(info) = layer_info.get(&surface_idx) {
            if let Some(&state_row) = node_index.get(&info.outer_node) {
                let col_offset = ext_surface_columns.len();
                ext_surface_columns.push((surface_idx, info.outer_node, state_row));
                let _ = col_offset; // used below when building B_c
            }
        }
    }
    let n_ext_surface_inputs = ext_surface_columns.len();

    // Build surface_idx → B-matrix column index map.
    let ext_surface_col_map: std::collections::HashMap<usize, usize> = ext_surface_columns
        .iter()
        .enumerate()
        .map(|(i, &(surface_idx, _, _))| (surface_idx, n_ext + i))
        .collect();

    // Augment B_c: [B_ext | per-surface solar/LWR columns | zone sensible heat columns].
    //
    // Column layout:
    //   [0..n_ext)                                         External driving (outdoor, ground temps)
    //   [n_ext..n_ext+n_ext_surface_inputs)                Per-exterior-surface solar/LWR injection
    //   [n_ext+n_ext_surface_inputs..n_ext+n_ext_surface_inputs+n_zones)  Zone sensible heat
    let n_total_inputs = n_ext + n_ext_surface_inputs + n_zones;
    let mut b_c = DMatrix::<f64>::zeros(n_states, n_total_inputs);

    // Copy B_ext columns.
    for row in 0..n_states {
        for col in 0..n_ext {
            b_c[(row, col)] = b_ext[(row, col)];
        }
    }

    // Per-surface injection columns: gain = 1/C_outer_node.
    for (i, &(_, outer_node, state_row)) in ext_surface_columns.iter().enumerate() {
        let c_node = node_capacitances
            .get(&outer_node)
            .copied()
            .unwrap_or(hares_envelope::boundary_rc::MIN_CAPACITANCE_J_K)
            .max(hares_envelope::boundary_rc::MIN_CAPACITANCE_J_K);
        b_c[(state_row, n_ext + i)] = 1.0 / c_node;
    }

    // Zone sensible heat columns (shifted by n_ext_surface_inputs).
    let zone_input_offset = n_ext + n_ext_surface_inputs;
    for (zone_idx, &state_row) in zone_state_rows.iter().enumerate() {
        b_c[(state_row, zone_input_offset + zone_idx)] =
            1.0 / zone_capacitances[zone_idx].max(hares_envelope::boundary_rc::MIN_CAPACITANCE_J_K);
    }

    let output_mapping = OutputMapping {
        output_count: n_zones,
        node_to_output: zone_state_rows
            .iter()
            .enumerate()
            .map(|(out_idx, &state_row)| (state_row, out_idx, 1.0))
            .collect(),
        input_to_output: Vec::new(),
    };

    let dt_s = sim_config.time_res.num_milliseconds() as f64 / 1000.0;
    let model = StateSpaceModel::from_continuous(&a_c, &b_c, dt_s, &output_mapping)
        .map_err(|err| HaresError::Envelope(format!("state-space setup failed: {err}")))?;

    // --- ThermalSolverConfig ---
    // Determine outdoor node column index in B_ext (external nodes sorted ascending by NodeId).
    // OUTDOOR_NODE = NodeId(10_000), GROUND_NODE = NodeId(10_001).
    // Collect the unique external nodes actually used, sorted.
    // outdoor_col already computed by assemble_building_rc.

    let mut thermal_cfg = ThermalSolverConfig::default();
    for (zone_idx, zone) in env.zones.iter().enumerate() {
        thermal_cfg
            .zone_state_indices
            .insert(zone.id, zone_state_rows[zone_idx]);
        thermal_cfg.zone_output_indices.insert(zone.id, zone_idx);
        thermal_cfg
            .zone_sensible_input_indices
            .insert(zone.id, zone_input_offset + zone_idx);
        thermal_cfg
            .ideal_setpoints_c
            .insert(zone.id, zone.temperature_c);
    }
    if let Some(col) = outdoor_col {
        thermal_cfg.outdoor_temp_input_indices = vec![col];
    } else {
        thermal_cfg.outdoor_temp_input_indices = vec![];
    }

    // Build window lookup: boundary id → Window, for matching windows to their
    // boundary entries when populating solar properties.
    let window_by_id: std::collections::HashMap<&str, &hares_io::hpxml::building::Window> =
        building
            .windows
            .iter()
            .map(|w| (w.id.as_str(), w))
            .collect();

    // Populate exterior surfaces for longwave radiation; use outermost layer node
    // (or zone air node for boundaries without material layers) as the surface node.
    for (surface_idx, boundary) in building.boundaries.iter().enumerate() {
        let is_exterior = boundary
            .exterior_zone
            .as_ref()
            .map(|z| *z == hares_io::hpxml::ZoneType::Outdoor)
            .unwrap_or(false);
        if !is_exterior {
            continue;
        }
        let zone_idx = boundary_zone_index(building, boundary.interior_zone.as_ref(), n_zones);
        let zone_id = env
            .zones
            .get(zone_idx)
            .map(|z| z.id)
            .unwrap_or(hares_types::ZoneId(1));

        // Use the outermost layer node if this boundary has material layers.
        // Surfaces with RC layers get a dedicated B-matrix input column that
        // injects heat at the exterior node (gain = 1/C_outer_node), so solar/LWR
        // conducts inward through the wall resistance chain.
        // Surfaces without layers fall back to the zone air node + zone column.
        let (state_index, input_index) = if let Some(&col) = ext_surface_col_map.get(&surface_idx)
        {
            let info = &layer_info[&surface_idx];
            let state_row = node_index.get(&info.outer_node).copied().unwrap_or(0);
            (state_row, col)
        } else {
            let si = *thermal_cfg.zone_state_indices.get(&zone_id).unwrap_or(&0);
            let ii = *thermal_cfg
                .zone_sensible_input_indices
                .get(&zone_id)
                .unwrap_or(&(zone_input_offset + zone_idx));
            (si, ii)
        };

        // OCHRE Envelope.py:221-222 gates radiant-barrier defaults on attic zone.
        let is_attic_radiant_barrier = boundary.has_radiant_barrier
            && boundary.interior_zone.as_ref() == Some(&hares_io::hpxml::ZoneType::Attic);
        let emissivity = boundary.emittance.unwrap_or(
            if is_attic_radiant_barrier {
                EMISSIVITY_RADIANT_BARRIER
            } else {
                EMISSIVITY_DEFAULT
            },
        );
        let solar_absorptance = boundary.solar_absorptance.unwrap_or(
            if is_attic_radiant_barrier {
                SOLAR_ABSORPTANCE_RADIANT_BARRIER
            } else {
                SOLAR_ABSORPTANCE_DEFAULT
            },
        );
        let tilt_deg = match boundary.boundary_type {
            hares_io::hpxml::BoundaryType::Roof => 0.0,
            hares_io::hpxml::BoundaryType::Slab | hares_io::hpxml::BoundaryType::Floor => 180.0,
            _ => 90.0,
        };
        // Compute radiation fraction and resistance for iterative LWR solver.
        // rad_frac = R_film / (R_film + R_outermost_half) [m²·K/W]
        // rad_res  = R_film / area [K/W]
        let bd_input = &boundary_inputs[surface_idx];
        let r_film = bd_input.r_film_exterior_m2_k_w;
        let r_outermost_half = if !bd_input.precomputed_rc.is_empty() {
            bd_input.precomputed_rc.last().unwrap().resistance_m2_k_w / 2.0
        } else {
            let valid_layers: Vec<&hares_envelope::LayerInput> = bd_input
                .material_layers
                .iter()
                .filter(|l| l.conductivity_w_m_k > 0.0 && l.thickness_m > 0.0)
                .collect();
            if let Some(outer) = valid_layers.last() {
                outer.thickness_m / (2.0 * outer.conductivity_w_m_k)
            } else {
                // No material layers: single-resistance boundary, skip iteration.
                0.0
            }
        };
        let (rad_frac, rad_res_k_w) = if r_outermost_half > 0.0 {
            (
                r_film / (r_film + r_outermost_half),
                r_film / boundary.area_m2,
            )
        } else {
            (0.0, 0.0)
        };
        // OCHRE Envelope.py:207: iterations = time_res // timedelta(minutes=5) + 1
        let n_iter = (dt_s / 300.0).floor() as u32 + 1;
        let outdoor_temp_c = env.weather.outdoor_temp_c;

        let surface_id = u32::try_from(surface_idx).unwrap_or(u32::MAX);
        thermal_cfg.exterior_surfaces.push(ExteriorSurfaceInfo {
            surface_id,
            state_index,
            input_index,
            area_m2: boundary.area_m2,
            emissivity,
            tilt_deg,
            rad_frac,
            rad_res_k_w,
            n_iter,
            t_prev_c: outdoor_temp_c,
            absorptance: solar_absorptance,
        });
        // Route solar irradiance to the zone's sensible heat input column.
        thermal_cfg.solar_input_indices.insert(surface_id, input_index);

        // Windows use the IAM-corrected path via solar_input_indices + window_properties.
        // Opaque surfaces receive solar gain via apply_exterior_solar_inputs (ExteriorSurfaceInfo.absorptance).
        if let Some(win) = window_by_id.get(boundary.id.as_str()) {
            let u_factor = win.u_factor_w_m2_k.unwrap_or(5.0);
            let effective_shgc = win.shgc.unwrap_or(0.4) * win.interior_shading_fraction;
            // Window glass material resistance: total R minus film resistances.
            // For simple glazing, approximate from U-factor:
            //   R_total = 1/U, R_glass ≈ R_total - R_film_int - R_film_ext
            let r_total = 1.0 / u_factor.max(0.01);
            let r_glass = (r_total
                - bd_input.r_film_interior_m2_k_w
                - bd_input.r_film_exterior_m2_k_w)
                .max(0.0);
            let (transmittance, radiation_frac) =
                hares_physics::solar::calculate_window_parameters(
                    effective_shgc,
                    u_factor,
                    r_glass,
                );
            thermal_cfg.window_properties.insert(
                surface_id,
                WindowSolarProperties {
                    shgc: effective_shgc,
                    u_factor_w_m2_k: u_factor,
                    area_m2: boundary.area_m2,
                    transmittance,
                    radiation_frac,
                },
            );
        }
    }

    // --- Per-zone infiltration (Walker-Wilson 1998 / ASHRAE HOF Ch. 16) ---
    // Each zone gets a physics-appropriate infiltration method.
    // Conditioned zone uses AIM-2 model when ACH50 is available.
    {
        use hares_envelope::InfiltrationMethod;
        use hares_io::hpxml::ZoneType;
        use hares_physics::infiltration::{
            Aim2Params, FoundationLeakageClass, N_I_DEFAULT,
            aim2_coefficients_from_ach50, attic_ela_coefficients,
        };

        let default_ceiling_height_m = building.ceiling_height_m.unwrap_or(2.5);
        let building_height_m = default_ceiling_height_m
            * building
                .zones
                .iter()
                .filter(|z| z.zone_type == ZoneType::Conditioned)
                .count()
                .max(1) as f64;

        for (zone_idx, bz) in building.zones.iter().enumerate() {
            let zone_id = env
                .zones
                .get(zone_idx)
                .map(|z| z.id)
                .unwrap_or(hares_types::ZoneId(zone_idx as u16));

            let method = match bz.zone_type {
                ZoneType::Conditioned => {
                    if let Some(ach50) = building.infiltration_ach50 {
                        let terrain = site_type_to_terrain(&building.site.site_type);
                        let shielding =
                            shielding_str_to_class(building.site.shielding_of_home.as_deref());
                        let foundation = if has_vented_crawlspace(building) {
                            FoundationLeakageClass::VentedCrawlspace
                        } else {
                            FoundationLeakageClass::Other
                        };
                        let h = building.infiltration_height_m.unwrap_or(
                            default_ceiling_height_m
                                * building.floors_above_grade.unwrap_or(1.0),
                        );
                        let coeffs = aim2_coefficients_from_ach50(&Aim2Params {
                            ach50,
                            volume_m3: building.conditioned_volume_m3.unwrap_or(400.0),
                            infiltration_height_m: h,
                            foundation,
                            shielding,
                            terrain,
                            has_flue: building.has_flue_or_chimney.unwrap_or(false),
                            n_i: N_I_DEFAULT,
                            floors_above_grade: building.floors_above_grade.unwrap_or(1.0),
                        });
                        InfiltrationMethod::AshraeWindStack {
                            c_s: coeffs.c_s,
                            c_w: coeffs.c_w,
                            shielding_coeff: coeffs.shelter_coeff,
                            n_i: coeffs.n_i,
                        }
                    } else {
                        InfiltrationMethod::Ach { ach: 0.0 }
                    }
                }
                ZoneType::Attic => {
                    if let Some(ach) = bz.ventilation_ach {
                        InfiltrationMethod::Ach { ach }
                    } else if let Some(sla) = bz.ventilation_sla {
                        let floor_area_m2 = bz.floor_area_m2.unwrap_or(100.0);
                        let ela_m2 = sla * floor_area_m2;
                        let attic_height_m = 1.5; // default triangular attic
                        let (stack_coeff, wind_coeff) =
                            attic_ela_coefficients(attic_height_m, building_height_m);
                        InfiltrationMethod::Ela {
                            ela_m2,
                            stack_coeff,
                            wind_coeff,
                        }
                    } else if bz.vented {
                        // Vented attic with no HPXML data: default 2.0 ACH
                        // (ASHRAE 62.2 typical for vented attics).
                        InfiltrationMethod::Ach { ach: 2.0 }
                    } else {
                        // Unvented attic: minimal air exchange.
                        InfiltrationMethod::Ach { ach: 0.1 }
                    }
                }
                ZoneType::Garage => {
                    // Garage: use building ACH50 if available, else default 0.5 ACH.
                    let garage_ach = building
                        .infiltration_ach50
                        .map(|ach50| ach50 / 20.0) // rough conversion ACH50→natural ACH
                        .unwrap_or(0.5);
                    InfiltrationMethod::Ach { ach: garage_ach }
                }
                ZoneType::Foundation => {
                    if bz.vented {
                        // Vented crawlspace: high air exchange.
                        let ach = bz.ventilation_ach.unwrap_or(2.0);
                        InfiltrationMethod::Ach { ach }
                    } else {
                        // Unvented crawlspace/basement: conduction only.
                        InfiltrationMethod::Ach { ach: 0.0 }
                    }
                }
                ZoneType::Outdoor | ZoneType::Other(_) => continue,
            };

            thermal_cfg.infiltration.push((zone_id, method));
        }
    }

    let initial_temp = env.zones.first().map(|z| z.temperature_c).unwrap_or(21.0);
    let thermal_solver = ThermalSolver::new(model, thermal_cfg, dt_s, env, initial_temp)
        .map_err(|err| HaresError::Envelope(format!("thermal solver init failed: {err}")))?;

    let humidity_solver = HumiditySolver::new(HumiditySolverConfig::default(), env);
    let electrical_solver = ElectricalSolver::new(ElectricalSolverConfig::default())
        .map_err(|err| HaresError::Envelope(format!("electrical solver init failed: {err}")))?;
    let fluid_solver = FluidSolver::new(FluidSolverConfig::default(), &[])
        .map_err(|err| HaresError::Envelope(format!("fluid solver init failed: {err}")))?;

    let zone_caps: Vec<(ZoneId, f64)> = env
        .zones
        .iter()
        .enumerate()
        .map(|(i, z)| {
            let cap = zone_capacitances
                .get(i)
                .copied()
                .unwrap_or(hares_envelope::boundary_rc::MIN_CAPACITANCE_J_K);
            (z.id, cap.max(hares_envelope::boundary_rc::MIN_CAPACITANCE_J_K))
        })
        .collect();

    Ok((
        thermal_solver,
        humidity_solver,
        electrical_solver,
        fluid_solver,
        zone_caps,
    ))
}

/// Default assembly R-value fallback (re-exported for dwelling-level use).
const DEFAULT_R_M2_K_W: f64 = hares_envelope::boundary_rc::DEFAULT_R_M2_K_W;

/// Convert building zones to envelope-crate ZoneInput.
pub fn building_to_zone_inputs(building: &Building, n_zones: usize) -> Vec<ZoneInput> {
    (0..n_zones)
        .map(|idx| ZoneInput {
            floor_area_m2: building.zones.get(idx).and_then(|z| z.floor_area_m2),
            volume_m3: building.zones.get(idx).and_then(|z| z.volume_m3),
        })
        .collect()
}

/// Convert building boundaries to envelope-crate BoundaryInput with pre-resolved zone indices.
///
/// When the defaults store contains an envelope LUT, attempts to resolve each
/// boundary to OCHRE pre-computed RC layers. Falls through to raw material
/// layers on LUT miss.
pub fn building_to_boundary_inputs(
    building: &Building,
    n_zones: usize,
    defaults: &DefaultsStore,
    avg_wind_m_s: f64,
    avg_ambient_c: f64,
    avg_ground_c: f64,
) -> Vec<BoundaryInput> {
    use hares_envelope::PrecomputedRCLayer;
    use hares_io::envelope_lut::resolve_boundary_name;
    use hares_physics::film_coefficients::{SurfaceRoughness, film_resistances};

    let envelope_lut = defaults.envelope_lut();

    building
        .boundaries
        .iter()
        .map(|bd| {
            let interior_zone_idx =
                find_zone_idx(building, bd.interior_zone.as_ref(), n_zones);
            let exterior = resolve_exterior(building, bd, n_zones);
            let fallback_r = bd
                .assembly_r_value_m2_k_w
                .or_else(|| {
                    let sum: f64 = bd.r_value_layers_m2_k_w.iter().sum();
                    if sum > 0.0 { Some(sum) } else { None }
                })
                .unwrap_or(DEFAULT_R_M2_K_W)
                .max(1e-6);

            // Try LUT lookup for precomputed RC layers.
            let precomputed_rc = envelope_lut
                .and_then(|lut| {
                    let boundary_name = resolve_boundary_name(
                        &bd.boundary_type,
                        bd.interior_zone.as_ref(),
                        bd.exterior_zone.as_ref(),
                    )?;
                    let r_value = bd.assembly_r_value_m2_k_w.or_else(|| {
                        let sum: f64 = bd.r_value_layers_m2_k_w.iter().sum();
                        if sum > 0.0 { Some(sum) } else { None }
                    });
                    let result = lut.lookup(
                        boundary_name,
                        bd.construction_type.as_deref(),
                        bd.finish_type.as_deref(),
                        bd.insulation_details.as_deref(),
                        r_value,
                    )?;
                    Some(
                        result
                            .layers
                            .into_iter()
                            .map(|l| PrecomputedRCLayer {
                                resistance_m2_k_w: l.resistance_m2_k_w,
                                capacitance_kj_m2_k: l.capacitance_kj_m2_k,
                            })
                            .collect::<Vec<_>>(),
                    )
                })
                .unwrap_or_default();

            let tilt_deg = match bd.boundary_type {
                hares_io::hpxml::BoundaryType::Roof => 0.0,
                hares_io::hpxml::BoundaryType::Slab
                | hares_io::hpxml::BoundaryType::Floor => 180.0,
                _ => 90.0,
            };
            let interior_label = zone_type_to_label(bd.interior_zone.as_ref());
            let exterior_label = zone_type_to_label(bd.exterior_zone.as_ref());
            let (r_film_int, r_film_ext) = film_resistances(
                tilt_deg,
                interior_label,
                exterior_label,
                avg_wind_m_s,
                avg_ground_c,
                avg_ambient_c,
                SurfaceRoughness::Rough,
            );

            BoundaryInput {
                area_m2: bd.area_m2,
                interior_zone_idx,
                exterior,
                material_layers: bd
                    .material_layers
                    .iter()
                    .map(|l| LayerInput {
                        thickness_m: l.thickness_m,
                        conductivity_w_m_k: l.conductivity_w_m_k,
                        density_kg_m3: l.density_kg_m3,
                        specific_heat_j_kg_k: l.specific_heat_j_kg_k,
                        area_m2: l.area_m2,
                    })
                    .collect(),
                precomputed_rc,
                fallback_r_m2_k_w: fallback_r,
                r_film_interior_m2_k_w: r_film_int,
                r_film_exterior_m2_k_w: r_film_ext,
            }
        })
        .collect()
}

/// Map HPXML ZoneType to film-coefficient ZoneLabel.
fn zone_type_to_label(zt: Option<&hares_io::hpxml::ZoneType>) -> hares_physics::film_coefficients::ZoneLabel {
    use hares_io::hpxml::ZoneType;
    use hares_physics::film_coefficients::ZoneLabel;
    match zt {
        Some(ZoneType::Conditioned) => ZoneLabel::Conditioned,
        Some(ZoneType::Attic) => ZoneLabel::Attic,
        Some(ZoneType::Garage) => ZoneLabel::Garage,
        Some(ZoneType::Foundation) => ZoneLabel::Foundation,
        Some(ZoneType::Outdoor) | None => ZoneLabel::Outdoor,
        Some(ZoneType::Other(_)) => ZoneLabel::Outdoor,
    }
}

/// Find zone index by ZoneType equality (exact match including Other payload).
fn find_zone_idx(
    building: &Building,
    zone_type: Option<&hares_io::hpxml::ZoneType>,
    n_zones: usize,
) -> usize {
    if n_zones == 0 {
        return 0;
    }
    if let Some(target) = zone_type {
        building
            .zones
            .iter()
            .position(|z| z.zone_type == *target)
            .unwrap_or(0)
            .min(n_zones - 1)
    } else {
        0
    }
}

fn resolve_exterior(
    building: &Building,
    boundary: &hares_io::hpxml::Boundary,
    n_zones: usize,
) -> ExteriorTarget {
    match boundary.exterior_zone.as_ref() {
        Some(hares_io::hpxml::ZoneType::Outdoor) => ExteriorTarget::Outdoor,
        Some(hares_io::hpxml::ZoneType::Foundation)
            if boundary.boundary_type == hares_io::hpxml::BoundaryType::Slab =>
        {
            ExteriorTarget::Ground
        }
        Some(zt) => {
            let idx = find_zone_idx(building, Some(zt), n_zones);
            ExteriorTarget::Zone(idx)
        }
        None => ExteriorTarget::Outdoor,
    }
}

fn build_output_column_index(schema: &arrow::datatypes::Schema) -> HashMap<String, usize> {
    // Recorder rows exclude the timestamp column.
    schema
        .fields()
        .iter()
        .skip(1)
        .enumerate()
        .map(|(idx, field)| (field.name().to_string(), idx))
        .collect()
}

fn default_output_path(config: &DwellingConfig) -> PathBuf {
    let ext = match config.sim_config.output_format {
        hares_io::OutputFormat::Csv => "csv",
        hares_io::OutputFormat::Parquet => "parquet",
    };
    PathBuf::from(format!("dwelling_{}.{}", config.bldg_id, ext))
}

fn chrono_to_std_duration(duration: Duration) -> Result<StdDuration> {
    let millis = duration.num_milliseconds();
    if millis <= 0 {
        return Err(HaresError::Physics(format!(
            "time resolution must be positive, got {millis} ms"
        )));
    }
    let ms_u64 = u64::try_from(millis)
        .map_err(|_| HaresError::Physics("failed converting duration to u64 ms".to_string()))?;
    Ok(StdDuration::from_millis(ms_u64))
}

fn stage_rank(stage: ExecutionStage) -> u8 {
    match stage {
        ExecutionStage::Independent => 0,
        ExecutionStage::Electrical => 1,
        ExecutionStage::Thermal => 2,
        ExecutionStage::EnvelopeResolution => 3,
    }
}

fn apply_thermal_update_to_zones(env: &mut EnvironmentState, update: &DomainUpdate) {
    for &(zone_id, temp_c) in &update.zone_temperatures_c {
        if let Some(zone) = env.zones.iter_mut().find(|z| z.id == zone_id) {
            zone.temperature_c = temp_c;
        }
    }
}

fn apply_humidity_update_to_zones(env: &mut EnvironmentState, update: &DomainUpdate) {
    let Some(payload) = &update.custom_payload else {
        return;
    };
    for chunk in payload.chunks_exact(4) {
        let zone_raw = chunk[0];
        let humidity_ratio = chunk[1];
        let relative_humidity = chunk[2];
        let wet_bulb_c = chunk[3];

        if !zone_raw.is_finite() || zone_raw < 0.0 || zone_raw > f64::from(u16::MAX) {
            continue;
        }
        let zone_id = ZoneId(zone_raw as u16);
        if let Some(zone) = env.zones.iter_mut().find(|z| z.id == zone_id) {
            zone.humidity_ratio = humidity_ratio;
            zone.relative_humidity = relative_humidity;
            zone.wet_bulb_c = wet_bulb_c;
        }
    }
}

fn equipment_config_from_spec(spec: &hares_io::EquipmentSpec) -> EquipmentConfig {
    let mut raw_config: HashMap<String, ConfigValue> = spec
        .parameters
        .iter()
        .filter_map(|(k, v)| json_value_to_config_value(v).map(|cv| (k.clone(), cv)))
        .collect();
    if let Some(zip) = &spec.zip_params {
        raw_config.insert("zip_z".to_string(), ConfigValue::Float(zip.zp));
        raw_config.insert("zip_i".to_string(), ConfigValue::Float(zip.ip));
        raw_config.insert("zip_p".to_string(), ConfigValue::Float(zip.pp));
        raw_config.insert("zip_v0".to_string(), ConfigValue::Float(1.0));
        raw_config.insert("zip_zq".to_string(), ConfigValue::Float(zip.zq));
        raw_config.insert("zip_iq".to_string(), ConfigValue::Float(zip.iq));
        raw_config.insert("zip_pq".to_string(), ConfigValue::Float(zip.pq));
        raw_config.insert("zip_pf".to_string(), ConfigValue::Float(zip.pf));
    }

    EquipmentConfig {
        name: spec.name.clone(),
        ochre_class: spec.name.clone(),
        raw_config,
    }
}

fn merged_equipment_config(spec: &hares_io::EquipmentSpec, overrides: &Value) -> EquipmentConfig {
    let mut merged = spec.parameters.clone();
    apply_equipment_overrides(&mut merged, overrides, &spec.name);
    let merged_spec = hares_io::EquipmentSpec {
        name: spec.name.clone(),
        fuel_type: spec.fuel_type,
        parameters: merged,
        zip_params: spec.zip_params.clone(),
    };
    equipment_config_from_spec(&merged_spec)
}

fn apply_equipment_overrides(base: &mut Map<String, Value>, overrides: &Value, name: &str) {
    let Value::Object(root) = overrides else {
        return;
    };
    if let Some(Value::Object(all)) = root.get("all").or_else(|| root.get("*")) {
        hares_io::hpxml::nested_update(base, all);
    }
    if let Some(Value::Object(eq)) = root.get(name) {
        hares_io::hpxml::nested_update(base, eq);
    }
}

fn boundary_zone_index(
    building: &Building,
    zone_type: Option<&hares_io::hpxml::ZoneType>,
    n_zones: usize,
) -> usize {
    if n_zones == 0 {
        return 0;
    }
    if let Some(target) = zone_type
        && let Some(idx) = building.zones.iter().position(|z| z.zone_type == *target)
    {
        return idx.min(n_zones - 1);
    }
    0
}

fn duration_to_u32_secs(duration: Duration) -> Result<u32> {
    let secs = duration.num_seconds();
    if secs <= 0 {
        return Err(HaresError::Physics(format!(
            "duration must be positive seconds, got {secs}"
        )));
    }
    u32::try_from(secs)
        .map_err(|_| HaresError::Physics(format!("duration seconds exceed u32 range: {secs}")))
}

fn required_path(kwargs: &HashMap<String, Value>, key: &str) -> Result<PathBuf> {
    let value = kwargs
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| HaresError::Io(format!("missing required kwarg `{key}`")))?;
    Ok(PathBuf::from(value))
}

fn required_datetime(kwargs: &HashMap<String, Value>, key: &str) -> Result<DateTime<Utc>> {
    let value = kwargs
        .get(key)
        .ok_or_else(|| HaresError::Io(format!("missing required kwarg `{key}`")))?;
    if let Some(text) = value.as_str() {
        let dt = DateTime::parse_from_rfc3339(text).map_err(|err| {
            HaresError::Io(format!("failed parsing `{key}` as RFC3339 datetime: {err}"))
        })?;
        return Ok(dt.with_timezone(&Utc));
    }
    if let Some(epoch) = value.as_i64() {
        let dt = DateTime::<Utc>::from_timestamp(epoch, 0)
            .ok_or_else(|| HaresError::Io(format!("invalid epoch timestamp for `{key}`")))?;
        return Ok(dt);
    }
    Err(HaresError::Io(format!(
        "unsupported datetime format for `{key}`"
    )))
}

fn required_duration(kwargs: &HashMap<String, Value>, key: &str) -> Result<Duration> {
    let value = kwargs
        .get(key)
        .ok_or_else(|| HaresError::Io(format!("missing required kwarg `{key}`")))?;
    if let Some(secs) = value.as_i64() {
        if secs <= 0 {
            return Err(HaresError::Io(format!(
                "`{key}` must be positive seconds, got {secs}"
            )));
        }
        return Ok(Duration::seconds(secs));
    }
    if let Some(text) = value.as_str() {
        let secs: i64 = text.parse().map_err(|err| {
            HaresError::Io(format!("failed parsing `{key}` duration seconds: {err}"))
        })?;
        if secs <= 0 {
            return Err(HaresError::Io(format!(
                "`{key}` must be positive seconds, got {secs}"
            )));
        }
        return Ok(Duration::seconds(secs));
    }
    Err(HaresError::Io(format!(
        "unsupported duration format for `{key}`"
    )))
}

fn validate_sim_config(sim_config: &SimulationConfig) -> Result<()> {
    if sim_config.duration.num_seconds() <= 0 {
        return Err(HaresError::Io("duration must be > 0".to_string()));
    }
    if sim_config.time_res.num_seconds() <= 0 {
        return Err(HaresError::Io("time_res must be > 0".to_string()));
    }
    if sim_config.duration.num_seconds() % sim_config.time_res.num_seconds() != 0 {
        return Err(HaresError::Io(
            "duration must be evenly divisible by time_res".to_string(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
struct SyntheticTomlConfig {
    #[serde(default)]
    building_id: Option<i64>,
    simulation: SyntheticSimulationConfig,
    geometry: SyntheticGeometryConfig,
    materials: SyntheticMaterialsConfig,
    hvac: SyntheticHvacConfig,
    #[serde(default)]
    weather: SyntheticWeatherConfig,
    #[serde(default)]
    schedule: SyntheticScheduleConfig,
    #[serde(default)]
    overrides: Option<Value>,
    #[serde(default)]
    output: SyntheticOutputConfig,
}

#[derive(Debug, Clone, Deserialize)]
struct SyntheticSimulationConfig {
    start_time: DateTime<Utc>,
    time_res_s: i64,
    duration_s: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct SyntheticGeometryConfig {
    floor_area_m2: f64,
    zone_volume_m3: f64,
    #[serde(default = "default_wall_area_m2")]
    wall_area_m2: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct SyntheticMaterialsConfig {
    wall_r_value_m2_k_w: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct SyntheticHvacConfig {
    equipment_name: String,
    #[serde(default)]
    fuel: Option<String>,
    #[serde(default)]
    heating_capacity_kbtu_h: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
struct SyntheticWeatherConfig {
    #[serde(default = "default_outdoor_temp_c")]
    outdoor_temp_c: f64,
    #[serde(default = "default_dew_point_c")]
    dew_point_c: f64,
    #[serde(default = "default_rel_humidity_pct")]
    rel_humidity_pct: f64,
    #[serde(default = "default_pressure_kpa")]
    pressure_kpa: f64,
}

impl Default for SyntheticWeatherConfig {
    fn default() -> Self {
        Self {
            outdoor_temp_c: default_outdoor_temp_c(),
            dew_point_c: default_dew_point_c(),
            rel_humidity_pct: default_rel_humidity_pct(),
            pressure_kpa: default_pressure_kpa(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct SyntheticScheduleConfig {
    #[serde(default = "default_schedule_value")]
    occupancy: f64,
}

impl Default for SyntheticScheduleConfig {
    fn default() -> Self {
        Self {
            occupancy: default_schedule_value(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct SyntheticOutputConfig {
    #[serde(default)]
    output_verbosity: u8,
    #[serde(default)]
    output_path: Option<String>,
    #[serde(default)]
    output_format: hares_io::OutputFormat,
    #[serde(default = "default_output_chunk_size")]
    output_chunk_size: usize,
    #[serde(default)]
    master_seed: u64,
}

impl Default for SyntheticOutputConfig {
    fn default() -> Self {
        Self {
            output_verbosity: 0,
            output_path: None,
            output_format: hares_io::OutputFormat::Csv,
            output_chunk_size: default_output_chunk_size(),
            master_seed: 0,
        }
    }
}

fn default_wall_area_m2() -> f64 {
    120.0
}

fn default_outdoor_temp_c() -> f64 {
    10.0
}

fn default_dew_point_c() -> f64 {
    5.0
}

fn default_rel_humidity_pct() -> f64 {
    50.0
}

fn default_pressure_kpa() -> f64 {
    101.325
}

fn default_schedule_value() -> f64 {
    1.0
}

fn default_output_chunk_size() -> usize {
    10_000
}

/// Map HPXML `<SiteType>` to [`TerrainClass`] for AIM-2 wind correction.
fn site_type_to_terrain(
    site_type: &Option<hares_io::hpxml::SiteType>,
) -> hares_physics::infiltration::TerrainClass {
    use hares_io::hpxml::SiteType;
    use hares_physics::infiltration::TerrainClass;
    match site_type {
        Some(SiteType::Rural) => TerrainClass::Rural,
        Some(SiteType::Urban) => TerrainClass::Urban,
        _ => TerrainClass::Suburban,
    }
}

/// Map HPXML `<ShieldingOfHome>` string to [`ShieldingClass`].
///
/// HPXML values: "normal", "exposed", "well-shielded".
/// Walker & Wilson (1998) Table 3; ResStock `airflow.get_aim2_shelter_coefficient`.
fn shielding_str_to_class(
    s: Option<&str>,
) -> hares_physics::infiltration::ShieldingClass {
    use hares_physics::infiltration::ShieldingClass;
    match s {
        Some("exposed") => ShieldingClass::Exposed,
        Some("well-shielded") => ShieldingClass::WellShielded,
        _ => ShieldingClass::Normal,
    }
}

/// Check if any Foundation zone has `vented == true` (vented crawlspace).
///
/// Used to select [`FoundationLeakageClass`] for AIM-2 leakage distribution.
fn has_vented_crawlspace(building: &Building) -> bool {
    use hares_io::hpxml::ZoneType;
    building
        .zones
        .iter()
        .any(|z| z.zone_type == ZoneType::Foundation && z.vented)
}

fn build_synthetic_building(config: &SyntheticTomlConfig) -> Building {
    use hares_io::hpxml::{Boundary, BoundaryType, Site, Zone, ZoneType};

    let heating_capacity = config.hvac.heating_capacity_kbtu_h.unwrap_or(30.0);
    let floor_area = if config.geometry.floor_area_m2 > 0.0 {
        config.geometry.floor_area_m2
    } else {
        config.geometry.zone_volume_m3 / 2.5
    };
    let fuel = config
        .hvac
        .fuel
        .clone()
        .unwrap_or_else(|| "natural gas".to_string());

    let details_xml = hares_io::hpxml::building::XmlNode {
        name: "BuildingDetails".to_string(),
        attrs: HashMap::new(),
        text: String::new(),
        children: vec![hares_io::hpxml::building::XmlNode {
            name: "Systems".to_string(),
            attrs: HashMap::new(),
            text: String::new(),
            children: vec![hares_io::hpxml::building::XmlNode {
                name: "HVAC".to_string(),
                attrs: HashMap::new(),
                text: String::new(),
                children: vec![hares_io::hpxml::building::XmlNode {
                    name: "HeatingSystem".to_string(),
                    attrs: HashMap::new(),
                    text: String::new(),
                    children: vec![
                        hares_io::hpxml::building::XmlNode {
                            name: "HeatingSystemType".to_string(),
                            attrs: HashMap::new(),
                            text: config.hvac.equipment_name.clone(),
                            children: Vec::new(),
                        },
                        hares_io::hpxml::building::XmlNode {
                            name: "HeatingSystemFuel".to_string(),
                            attrs: HashMap::new(),
                            text: fuel,
                            children: Vec::new(),
                        },
                        hares_io::hpxml::building::XmlNode {
                            name: "HeatingCapacity".to_string(),
                            attrs: HashMap::new(),
                            text: (heating_capacity * 1000.0).to_string(),
                            children: Vec::new(),
                        },
                    ],
                }],
            }],
        }],
    };

    Building {
        site: Site {
            elevation_m: Some(0.0),
            site_type: None,
            shielding_of_home: None,
            latitude_deg: Some(39.0),
            longitude_deg: Some(-105.0),
        },
        zones: vec![Zone {
            zone_type: ZoneType::Conditioned,
            floor_area_m2: Some(floor_area),
            volume_m3: Some(config.geometry.zone_volume_m3),
            attached_wall_ids: vec!["wall-1".to_string()],
            duct_systems: Vec::new(),
            vented: false,
            ventilation_ach: None,
            ventilation_sla: None,
        }],
        boundaries: vec![Boundary {
            id: "wall-1".to_string(),
            boundary_type: BoundaryType::Wall,
            area_m2: config.geometry.wall_area_m2,
            azimuth_deg: Some(180.0),
            assembly_r_value_m2_k_w: Some(config.materials.wall_r_value_m2_k_w),
            r_value_layers_m2_k_w: vec![config.materials.wall_r_value_m2_k_w],
            interior_zone: Some(ZoneType::Conditioned),
            exterior_zone: Some(ZoneType::Outdoor),
            material_layers: Vec::new(),
            construction_type: None,
            finish_type: None,
            insulation_details: None,
            has_radiant_barrier: false,
            solar_absorptance: None,
            emittance: None,
            tilt_deg: Some(90.0),
        }],
        windows: Vec::new(),
        infiltration_ach50: None,
        hvac_capacity_w: Some(heating_capacity),
        seer2: None,
        hspf2: None,
        water_heater_setpoint_c: None,
        heating_weekday_setpoints_c: None,
        heating_weekend_setpoints_c: None,
        cooling_weekday_setpoints_c: None,
        cooling_weekend_setpoints_c: None,
        battery_round_trip_efficiency: None,
        pv_tilt_deg: None,
        conditioned_volume_m3: Some(config.geometry.zone_volume_m3),
        ceiling_height_m: None,
        infiltration_height_m: None,
        floors_above_grade: None,
        has_flue_or_chimney: None,
        details_xml,
    }
}

fn build_synthetic_weather(config: &SyntheticTomlConfig) -> WeatherTimeSeries {
    let n = 8760usize;
    let meta = WeatherMeta {
        location: "Synthetic".to_string(),
        latitude: 39.0,
        longitude: -105.0,
        timezone_offset_h: 0.0,
        elevation_m: 0.0,
    };
    WeatherTimeSeries {
        meta,
        dry_bulb_c: vec![config.weather.outdoor_temp_c; n],
        dew_point_c: vec![config.weather.dew_point_c; n],
        rel_humidity_pct: vec![config.weather.rel_humidity_pct; n],
        pressure_kpa: vec![config.weather.pressure_kpa; n],
        ghi_w_m2: vec![0.0; n],
        dni_w_m2: vec![0.0; n],
        dhi_w_m2: vec![0.0; n],
        wind_speed_m_s: vec![0.0; n],
        wind_dir_deg: vec![0.0; n],
        opaque_sky_cover: vec![0.0; n],
        horizontal_infrared_w_m2: vec![300.0; n],
        sky_temp_c: vec![config.weather.outdoor_temp_c; n],
        ground_temp_c: vec![config.weather.outdoor_temp_c; n],
    }
}

fn build_synthetic_schedule(config: &SyntheticTomlConfig) -> Result<ScheduleTimeSeries> {
    use chrono::{FixedOffset, TimeDelta};

    let step_secs = duration_to_u32_secs(Duration::seconds(config.simulation.time_res_s))?;
    let total_steps = (config.simulation.duration_s / config.simulation.time_res_s).max(1) as usize;
    let offset = FixedOffset::east_opt(0)
        .ok_or_else(|| HaresError::Io("failed to build UTC offset".to_string()))?;
    let start = config.simulation.start_time.with_timezone(&offset);

    let mut timestamps = Vec::with_capacity(total_steps);
    for i in 0..total_steps {
        timestamps.push(start + TimeDelta::seconds((i as i64) * i64::from(step_secs)));
    }

    let column_names = vec!["occupancy".to_string()];
    let columns = vec![vec![config.schedule.occupancy; total_steps]];
    let column_index = HashMap::from([("occupancy".to_string(), 0usize)]);
    Ok(ScheduleTimeSeries {
        timestamps,
        column_names,
        columns,
        column_index,
        source_step_secs: step_secs,
        column_aggregations: vec![ColumnAggregation::Mean],
    })
}

fn json_value_to_config_value(value: &serde_json::Value) -> Option<ConfigValue> {
    match value {
        serde_json::Value::Number(n) => n.as_f64().map(ConfigValue::Float),
        serde_json::Value::String(s) => Some(ConfigValue::Text(s.clone())),
        serde_json::Value::Bool(b) => Some(ConfigValue::Bool(*b)),
        serde_json::Value::Array(arr) => {
            let floats: Vec<f64> = arr.iter().filter_map(|v| v.as_f64()).collect();
            if floats.len() == arr.len() {
                Some(ConfigValue::FloatArray(floats))
            } else {
                None
            }
        }
        _ => None,
    }
}

#[cfg(feature = "profiling")]
fn current_process_hwm_kb() -> u64 {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return 0;
    };

    status
        .lines()
        .find_map(|line| {
            if !line.starts_with("VmHWM:") {
                return None;
            }
            line.split_whitespace().nth(1)?.parse::<u64>().ok()
        })
        .unwrap_or(0)
}

#[cfg(feature = "profiling")]
fn hot_path_alloc_counter() -> u64 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

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
