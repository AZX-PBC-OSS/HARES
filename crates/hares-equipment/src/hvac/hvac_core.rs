//! Core HVAC equipment wrapper with thermostat state, step logic, and helpers.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use hares_physics::biquadratic::{BiquadraticCurve, quadratic};
use hares_physics::constants::{CFM_PER_M3_S, CFM_TO_M3_S, W_PER_TON};
use hares_types::{
    ControlSignal, EnvironmentState, PortContribution, PortSlots,
    ScheduleSource, ZoneId,
};

use crate::EquipmentConfig;

use super::core_config::{
    build_setpoint_source, extract_bool, extract_numeric, load_biquadratic_coeffs,
    load_bounds_pair, load_plr_coefficients, parse_speed_control_mode,
};
use super::speed_control::{SpeedControlMode, SpeedSelection, StartupConfig};
use super::thermostat::{
    RuntimeSetpointOverride, ScheduleSetpoints, ThermalSetpoints, ThermostatConfig, ThermostatMode,
    is_cycle_change_allowed, lookup_zone_temp,
};

const IDEAL_CAPACITY_TIME_RES_THRESHOLD_S: i64 = 300;
pub(super) const DEFAULT_BIQUADRATIC_COEFFS: [f64; 6] = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0];

/// Default fan power [W/CFM]. ACCA Manual D residential air handler.
const DEFAULT_FAN_POWER_W_PER_CFM: f64 = 0.365;
const DEFAULT_FAN_POWER_W_PER_M3_S: f64 = DEFAULT_FAN_POWER_W_PER_CFM * CFM_PER_M3_S;

/// Default outdoor temperature for supply-air initialization [C] (47 F).
/// AHRI 210/240 H1 heating test condition.
const DEFAULT_INIT_OUTDOOR_TEMP_C: f64 = 8.3;

/// Default low-speed capacity fraction for two-speed equipment.
const DEFAULT_LOW_SPEED_CAPACITY_FRACTION: f64 = 0.5;

/// Default thermostat cutout ratio when not specified in config.
const DEFAULT_CUTOUT_RATIO: f64 = 0.25;

/// Default minimum on/off cycle lockout [s] when not specified in config.
const DEFAULT_MIN_CYCLE_TIME_S: f64 = 60.0;

/// Default part-load factor degradation coefficient (Cd).
/// AHRI Standard 210/240-2023, S6.6.3 default when no test data available.
const DEFAULT_PLF_DEGRADATION_COEFF: f64 = 0.25;

/// OCHRE HVAC.py: biquadratic curve input bounds clamp physically impossible
/// extrapolation. These match OCHRE's fallback defaults (`min_Twb`/`max_Twb`,
/// `min_Tdb`/`max_Tdb`) when the biquadratic CSV does not specify bounds.
const DEFAULT_BIQUADRATIC_X1_BOUNDS: (f64, f64) = (-100.0, 100.0);
const DEFAULT_BIQUADRATIC_X2_BOUNDS: (f64, f64) = (-100.0, 100.0);

/// Equipment category for HVAC defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HvacEquipmentType {
    GasFurnace,
    ElectricFurnace,
    AshpHeatPumpOnly,
    AshpHeatPumpAux,
    MiniSplitHeat,
    /// Central AC or ASHP cooling coil. Uses 312 CFM/ton airflow default.
    AcCooler,
    /// MSHP cooling coil. Uses 312 CFM/ton airflow default; Cd=0 (no cycling penalty).
    MiniSplitCool,
    Baseboard,
    Other,
}

/// Default airflow [CFM/ton] by equipment category.
///
/// Source chain: ResStock `hvac.rb:2623` → OCHRE `HVAC.py:142`.
/// OCHRE selects 350 for heaters (`is_heater=True`) and 312 for coolers.
/// EnergyPlus valid range: 300–450 CFM/ton (0.00004027–0.00006041 m³/s/W).
/// Ref: EnergyPlus I/O Reference, Coil:Cooling:DX:SingleSpeed.
const AIRFLOW_HEATING_CFM_PER_TON: f64 = 350.0;
const AIRFLOW_COOLING_CFM_PER_TON: f64 = 312.0;

impl HvacEquipmentType {
    pub fn default_supply_air_temp_c(self, outdoor_temp_c: f64) -> f64 {
        match self {
            Self::GasFurnace => 54.4,
            Self::ElectricFurnace => 48.9,
            Self::AshpHeatPumpOnly => 32.2 + 0.15 * (outdoor_temp_c - 8.3),
            Self::AshpHeatPumpAux => 40.6,
            Self::MiniSplitHeat => 43.3,
            Self::AcCooler | Self::MiniSplitCool => 40.6,
            Self::Baseboard => 0.0,
            Self::Other => 40.6,
        }
    }

    /// Default airflow rate [CFM/ton] for this equipment category.
    ///
    /// Coolers (central AC, ASHP cooling coil, MSHP cooling coil) use 312;
    /// all heating equipment uses 350. Ref: OCHRE `HVAC.py:142`.
    pub fn default_airflow_cfm_per_ton(self) -> f64 {
        match self {
            Self::AcCooler | Self::MiniSplitCool => AIRFLOW_COOLING_CFM_PER_TON,
            Self::GasFurnace
            | Self::ElectricFurnace
            | Self::AshpHeatPumpOnly
            | Self::AshpHeatPumpAux
            | Self::MiniSplitHeat
            | Self::Baseboard
            | Self::Other => AIRFLOW_HEATING_CFM_PER_TON,
        }
    }

    /// True for heating-side equipment types.
    pub fn is_heating(self) -> bool {
        matches!(
            self,
            Self::GasFurnace
                | Self::ElectricFurnace
                | Self::AshpHeatPumpOnly
                | Self::AshpHeatPumpAux
                | Self::MiniSplitHeat
                | Self::Baseboard
        )
    }
}

/// Ideal-capacity callback implemented by thermal-domain solvers.
pub trait IdealCapacitySolver {
    fn solve_ideal_capacity(&self, env: &EnvironmentState, zone: ZoneId) -> f64;
}

impl<F> IdealCapacitySolver for F
where
    F: Fn(&EnvironmentState, ZoneId) -> f64,
{
    fn solve_ideal_capacity(&self, env: &EnvironmentState, zone: ZoneId) -> f64 {
        self(env, zone)
    }
}

/// Shared HVAC wrapper with thermostat state and base helper parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct HvacEquipment {
    pub equipment_type: HvacEquipmentType,
    pub zone_id: ZoneId,
    pub thermostat: ThermostatConfig,
    pub mode: ThermostatMode,
    pub duty_cycle: f64,
    pub static_setpoints: ThermalSetpoints,
    /// Time-varying heating setpoint source (CSV column, daily profile, or None).
    pub heating_setpoint_source: Option<ScheduleSource>,
    /// Time-varying cooling setpoint source (CSV column, daily profile, or None).
    pub cooling_setpoint_source: Option<ScheduleSource>,
    pub schedule_setpoints: Option<ScheduleSetpoints>,
    pub runtime_setpoints: Option<RuntimeSetpointOverride>,
    pub last_mode_switch_at: Option<DateTime<Utc>>,
    /// Timestamp when the current thermostat mode began.
    /// Used for compressor-level minimum on/off time enforcement.
    ///
    /// INVARIANT: `mode_start_at` must be updated atomically with `mode` whenever
    /// the mode changes. Always use `set_mode` to change `mode`; never assign
    /// `mode` directly without also updating `mode_start_at`, otherwise
    /// `can_transition_mode` will enforce constraints against a stale timestamp
    /// and the minimum on/off time protection will be silently bypassed.
    pub mode_start_at: Option<DateTime<Utc>>,
    /// Minimum time [s] compressor must remain On before an Off transition is
    /// allowed. Prevents short-cycle wear. 0.0 = disabled (default).
    /// OCHRE reference: 120 s for heat pump heating/cooling.
    pub min_on_time_s: f64,
    /// Minimum time [s] compressor must remain Off before an On transition is
    /// allowed. Prevents short-cycle wear. 0.0 = disabled (default).
    /// OCHRE reference: 180 s for heat pump off-cycle.
    pub min_off_time_s: f64,
    pub heating_capacities_w: Vec<f64>,
    pub cooling_capacities_w: Vec<f64>,
    pub eir_by_stage: Vec<f64>,
    pub fan_power_w_per_m3_s: f64,
    pub shr: f64,
    pub duct_dse: f64,
    /// Zone where duct losses are deposited (e.g. attic, garage, basement).
    /// `None` means duct losses are unrecoverable (lost to outdoors).
    /// OCHRE HVAC.py: `self.duct_zone`
    pub duct_zone_id: Option<ZoneId>,
    /// Fraction of delivered heat (post-DSE) routed to the basement zone [0, 1].
    ///
    /// When non-zero a `basement_zone_id` must also be set. The conditioned zone
    /// receives `dse * (1 - basement_heat_frac)`, the basement zone receives
    /// `dse * basement_heat_frac`, and the duct zone still gets `1 - dse`.
    ///
    /// Default is 0.0 (no basement routing).  OCHRE HVAC.py: `basement_frac`.
    pub basement_heat_frac: f64,
    /// Zone corresponding to the basement (destination for `basement_heat_frac`).
    /// Only meaningful when `basement_heat_frac > 0`.
    pub basement_zone_id: Option<ZoneId>,
    pub supply_air_temp_c: f64,
    pub airflow_m3_s_per_w: f64,
    /// Absolute heat fraction per zone. Each entry is `(zone_id, fraction)` where
    /// `fraction` is a direct multiplier on gross capacity (not normalized).
    /// Fractions sum to `<= 1.0`; the remainder is unrecoverable duct loss.
    ///
    /// Set via `update_zone_heat_fractions()` after configuring `duct_dse` and
    /// `duct_zone_id`. OCHRE HVAC.py: `self.zone_fractions`
    pub zone_heat_fractions: Vec<(ZoneId, f64)>,
    pub biquadratic_coeffs: Vec<[f64; 6]>,
    pub biquadratic_x1_bounds: (f64, f64),
    pub biquadratic_x2_bounds: (f64, f64),
    pub speed_control_mode: SpeedControlMode,
    pub low_speed_capacity_fraction: f64,
    pub plf_cooling_degradation_coeff: f64,
    pub plf_state: f64,
    pub startup: StartupConfig,
    pub last_speed_index: usize,
    pub last_speed_frac: f64,
    /// Seconds the unit has been running at the current speed stage.
    /// OCHRE HVAC.py: min_time_in_speed prevents rapid speed hunting by
    /// locking the current stage for at least `min_time_per_speed_s` seconds.
    pub time_at_current_speed_s: f64,
    /// Minimum time [s] before a two-speed stage change is allowed (default 300 s = 5 min).
    pub min_time_per_speed_s: f64,
    /// Per-speed EIR part-load ratio (PLR) quadratic coefficients `[a, b, c]`.
    ///
    /// OCHRE HVAC.py lines 823, 841–844: each speed stage has its own `eir_plr`
    /// quadratic that computes PLF = a + b·PLR + c·PLR². When `Some`, this
    /// replaces the simplified `1 − Cd·(1−PLR)` formula in `part_load_factor`.
    /// The PLF is clamped to `[min_plf, 1.0]` per OCHRE convention (min_plf=0.7).
    ///
    /// Indexed by speed stage. If a stage index is out of range, the last entry
    /// is used (same fallback pattern as `biquadratic_coeffs`).
    pub eir_plr_coefficients: Option<Vec<[f64; 3]>>,
    /// Per-speed disabled flags for demand-response control.
    ///
    /// OCHRE HVAC.py lines 754, 853–857: `disable_speeds[i]` marks speed stage `i`
    /// as unavailable. When a desired speed is disabled, equipment routes to the
    /// highest allowed (non-disabled) speed. Set via `set_disabled_speeds`.
    ///
    /// Length must match the number of speed stages. Empty = no stages disabled.
    pub disabled_speeds: Vec<bool>,
    /// Cached index of the highest non-disabled speed stage.
    /// Recomputed on every `set_disabled_speeds` call.
    /// OCHRE HVAC.py: `_max_enabled_speed`
    pub max_enabled_speed: usize,
    /// Previous zone temperature [°C], used by `TwoSpeedTime` mode to detect
    /// whether the temperature is still moving away from setpoint.
    /// Updated each timestep by `update_prev_zone_temp`.
    pub prev_zone_temp_c: Option<f64>,
    /// Fraction of conditioned space served by this equipment [0, 1].
    /// Scales electrical/fuel output but NOT thermal zone contributions.
    /// OCHRE HVAC.py:104: `self.space_fraction`.
    pub space_fraction: f64,
}

impl HvacEquipment {
    pub fn new(equipment_type: HvacEquipmentType, zone_id: ZoneId) -> Self {
        Self {
            equipment_type,
            zone_id,
            thermostat: ThermostatConfig::default(),
            mode: ThermostatMode::Deadband,
            duty_cycle: 0.0,
            static_setpoints: ThermalSetpoints {
                heating_c: 20.0,
                cooling_c: 24.0,
            },
            heating_setpoint_source: None,
            cooling_setpoint_source: None,
            schedule_setpoints: None,
            runtime_setpoints: None,
            last_mode_switch_at: None,
            mode_start_at: None,
            min_on_time_s: 0.0,
            min_off_time_s: 0.0,
            heating_capacities_w: vec![],
            cooling_capacities_w: vec![],
            eir_by_stage: vec![],
            fan_power_w_per_m3_s: DEFAULT_FAN_POWER_W_PER_M3_S,
            shr: 1.0,
            duct_dse: 1.0,
            duct_zone_id: None,
            basement_heat_frac: 0.0,
            basement_zone_id: None,
            supply_air_temp_c: equipment_type
                .default_supply_air_temp_c(DEFAULT_INIT_OUTDOOR_TEMP_C),
            airflow_m3_s_per_w: equipment_type.default_airflow_cfm_per_ton() * CFM_TO_M3_S
                / W_PER_TON,
            zone_heat_fractions: vec![(zone_id, 1.0)],
            biquadratic_coeffs: vec![DEFAULT_BIQUADRATIC_COEFFS],
            // OCHRE HVAC.py: biquadratic curve inputs clamped to calibrated range.
            // Default ±100°C matches OCHRE fallback when no explicit bounds configured.
            // Prevents physically impossible extrapolation at extreme temperatures.
            biquadratic_x1_bounds: DEFAULT_BIQUADRATIC_X1_BOUNDS,
            biquadratic_x2_bounds: DEFAULT_BIQUADRATIC_X2_BOUNDS,
            speed_control_mode: SpeedControlMode::SingleSpeed,
            low_speed_capacity_fraction: DEFAULT_LOW_SPEED_CAPACITY_FRACTION,
            // MSHP selects discrete compressor stages rather than cycling, so the
            // AHRI 210/240 cycling-degradation penalty (Cd) does not apply.
            // Config init() can still override this via the "cooling_cd" key.
            plf_cooling_degradation_coeff: match equipment_type {
                HvacEquipmentType::MiniSplitHeat | HvacEquipmentType::MiniSplitCool => 0.0,
                _ => DEFAULT_PLF_DEGRADATION_COEFF,
            },
            plf_state: 1.0,
            startup: StartupConfig::default(),
            last_speed_index: 0,
            last_speed_frac: 0.0,
            time_at_current_speed_s: 0.0,
            min_time_per_speed_s: 300.0,
            eir_plr_coefficients: None,
            disabled_speeds: vec![],
            max_enabled_speed: 0,
            prev_zone_temp_c: None,
            space_fraction: 1.0,
        }
    }

    pub fn init(&mut self, config: &EquipmentConfig, env: &EnvironmentState) -> crate::Result<()> {
        self.thermostat = ThermostatConfig {
            hysteresis_c: extract_numeric(config, "hysteresis_c").unwrap_or(1.0),
            cutout_ratio: extract_numeric(config, "cutout_ratio").unwrap_or(DEFAULT_CUTOUT_RATIO),
            min_cycle_time_s: extract_numeric(config, "min_cycle_time_s")
                .unwrap_or(DEFAULT_MIN_CYCLE_TIME_S),
            use_ideal_capacity: extract_bool(config, "use_ideal_capacity").unwrap_or(false),
            deadband_offset: extract_numeric(config, "deadband_offset").unwrap_or(0.2),
        };
        self.thermostat.validate(env)?;

        if let Some(value) = extract_numeric(config, "heating_setpoint_c") {
            self.static_setpoints.heating_c = value;
        }
        if let Some(value) = extract_numeric(config, "cooling_setpoint_c") {
            self.static_setpoints.cooling_c = value;
        }

        // Build setpoint sources: CSV column > daily profile > constant (static).
        self.heating_setpoint_source = build_setpoint_source(config, "heating");
        self.cooling_setpoint_source = build_setpoint_source(config, "cooling");

        // Seed static setpoints from the source so the initial deadband check
        // is reasonable before the first update_mode call.
        if let Some(ScheduleSource::DailyProfile { weekday, .. }) = &self.heating_setpoint_source {
            self.static_setpoints.heating_c = weekday[0];
        }
        if let Some(ScheduleSource::DailyProfile { weekday, .. }) = &self.cooling_setpoint_source {
            self.static_setpoints.cooling_c = weekday[0];
        }

        self.static_setpoints
            .validate_for_deadband(self.thermostat.hysteresis_c)?;

        let mut airflow_cfm_per_ton =
            extract_numeric(config, "airflow_cfm_per_ton")
                .unwrap_or_else(|| self.equipment_type.default_airflow_cfm_per_ton());
        let airflow_defect_ratio = extract_numeric(config, "AirflowDefectRatio")
            .or_else(|| extract_numeric(config, "airflow_defect_ratio"))
            .unwrap_or(1.0);
        airflow_cfm_per_ton *= airflow_defect_ratio;
        self.airflow_m3_s_per_w = airflow_cfm_per_ton * CFM_TO_M3_S / W_PER_TON;
        if let Some(w_per_m3_s) = extract_numeric(config, "fan_power_w_per_m3_s") {
            self.fan_power_w_per_m3_s = w_per_m3_s.max(0.0);
        } else if let Some(w_per_cfm) = extract_numeric(config, "fan_power_w_per_cfm") {
            self.fan_power_w_per_m3_s = w_per_cfm.max(0.0) * CFM_PER_M3_S;
        }

        self.supply_air_temp_c =
            extract_numeric(config, "supply_air_temp_c").unwrap_or_else(|| {
                self.equipment_type
                    .default_supply_air_temp_c(env.weather.outdoor_temp_c)
            });

        self.speed_control_mode = parse_speed_control_mode(config);
        self.low_speed_capacity_fraction = extract_numeric(config, "low_speed_capacity_fraction")
            .unwrap_or(DEFAULT_LOW_SPEED_CAPACITY_FRACTION);
        self.plf_cooling_degradation_coeff = extract_numeric(config, "cooling_cd")
            .or_else(|| extract_numeric(config, "cd"))
            .unwrap_or(DEFAULT_PLF_DEGRADATION_COEFF);
        let cd = extract_numeric(config, "startup_cd")
            .or_else(|| extract_numeric(config, "cooling_cd"))
            .or_else(|| extract_numeric(config, "cd"))
            .unwrap_or(DEFAULT_PLF_DEGRADATION_COEFF);
        self.startup = StartupConfig {
            c_d: cd,
            time_since_start_min: 0.0,
        };
        self.startup.validate()?;
        self.biquadratic_coeffs = load_biquadratic_coeffs(config, "biquadratic_coeffs")?;
        // OCHRE HVAC.py: biquadratic CSV files specify `min_Twb`, `max_Twb`,
        // `min_Tdb`, `max_Tdb` bounds. Load from config if provided.
        self.biquadratic_x1_bounds = load_bounds_pair(
            config,
            "biquadratic_x1_min",
            "biquadratic_x1_max",
            DEFAULT_BIQUADRATIC_X1_BOUNDS,
        );
        self.biquadratic_x2_bounds = load_bounds_pair(
            config,
            "biquadratic_x2_min",
            "biquadratic_x2_max",
            DEFAULT_BIQUADRATIC_X2_BOUNDS,
        );
        self.min_time_per_speed_s =
            extract_numeric(config, "min_time_per_speed_s").unwrap_or(300.0);
        // 0.0 = disabled (no compressor-level on/off hold). Configure via
        // "min_on_time_s" / "min_off_time_s" keys. OCHRE defaults: 120 s / 180 s.
        self.min_on_time_s = extract_numeric(config, "min_on_time_s").unwrap_or(0.0);
        self.min_off_time_s = extract_numeric(config, "min_off_time_s").unwrap_or(0.0);

        // Apply equipment-type and efficiency-rating Cd defaults per AHRI / OCHRE table.
        // Explicit config keys ("cooling_cd", "cd", "startup_cd") take precedence because
        // they were already applied above; these overrides only run when no explicit key
        // was provided (i.e., the config had no Cd key at all).
        let explicit_cd = extract_numeric(config, "startup_cd")
            .or_else(|| extract_numeric(config, "cooling_cd"))
            .or_else(|| extract_numeric(config, "cd"));
        if explicit_cd.is_none() {
            let rated_seer = extract_numeric(config, "rated_seer");
            let rated_hspf = extract_numeric(config, "rated_hspf");
            let derived_cd = match self.speed_control_mode {
                SpeedControlMode::VariableSpeedIdeal => Some(0.0),
                SpeedControlMode::TwoSpeedSetpoint
                | SpeedControlMode::TwoSpeedTime
                | SpeedControlMode::TwoSpeedAlternating => Some(0.11),
                SpeedControlMode::SingleSpeed => {
                    // Low-SEER AC: Cd = 0.20; otherwise 0.07.
                    // High-HSPF HP: Cd = 0.11; low-HSPF: 0.20.
                    let from_seer = rated_seer.map(|s| if s < 13.0 { 0.20 } else { 0.07 });
                    let from_hspf = rated_hspf.map(|h| if h < 7.0 { 0.20 } else { 0.11 });
                    from_seer.or(from_hspf)
                }
                SpeedControlMode::MultiSpeedInterpolated => None,
            };
            if let Some(cd) = derived_cd {
                self.plf_cooling_degradation_coeff = cd;
                self.startup.c_d = cd;
            }
        }

        self.eir_plr_coefficients = load_plr_coefficients(config, "eir_plr_coefficients")?;

        // space_fraction: OCHRE HVAC.py:104. Heating/cooling each reads their
        // specific fraction key, falling back to the generic key.
        let frac = if self.equipment_type.is_heating() {
            extract_numeric(config, "fraction_heating_load_served")
                .or_else(|| extract_numeric(config, "fraction_load_served"))
        } else {
            extract_numeric(config, "fraction_cooling_load_served")
                .or_else(|| extract_numeric(config, "fraction_load_served"))
        };
        if let Some(f) = frac {
            if !(0.0..=1.0).contains(&f) {
                tracing::warn!(
                    "space_fraction {f} out of range [0,1] for {:?}, clamping",
                    self.equipment_type
                );
            }
            self.space_fraction = f.clamp(0.0, 1.0);
        }

        Ok(())
    }

    /// Set which speed stages are disabled for demand-response control.
    ///
    /// OCHRE HVAC.py lines 853–857: `disable_speeds` is updated from the external
    /// control signal. The highest non-disabled speed is cached as `max_enabled_speed`.
    /// If all speeds are disabled, `max_enabled_speed` falls back to the last stage.
    ///
    /// `disabled`: length must equal the number of speed stages, or may be shorter
    /// (remaining stages default to enabled). An empty slice re-enables all stages.
    pub fn set_disabled_speeds(&mut self, disabled: &[bool]) {
        let n = self.n_speed_stages();
        self.disabled_speeds.resize(n, false);
        for (i, slot) in self.disabled_speeds.iter_mut().enumerate() {
            *slot = disabled.get(i).copied().unwrap_or(false);
        }
        // Cache the highest non-disabled index (0-based).
        self.max_enabled_speed = self
            .disabled_speeds
            .iter()
            .enumerate()
            .rev()
            .find(|&(_, d)| !d)
            .map(|(i, _)| i)
            .unwrap_or(n.saturating_sub(1));
    }

    /// Number of discrete speed stages. For single-speed equipment this is 1.
    pub fn n_speed_stages(&self) -> usize {
        match self.speed_control_mode {
            SpeedControlMode::SingleSpeed => 1,
            SpeedControlMode::TwoSpeedSetpoint
            | SpeedControlMode::TwoSpeedTime
            | SpeedControlMode::TwoSpeedAlternating => 2,
            SpeedControlMode::MultiSpeedInterpolated => {
                let caps = self.heating_capacities_w.len().max(self.cooling_capacities_w.len());
                caps.max(1)
            }
            SpeedControlMode::VariableSpeedIdeal => 1,
        }
    }

    /// Record the current zone temperature for the next step's `TwoSpeedTime` comparison.
    /// Pass `None` when the unit turns off to ensure the next on-cycle starts at low speed.
    pub fn update_prev_zone_temp(&mut self, zone_temp_c: Option<f64>) {
        self.prev_zone_temp_c = zone_temp_c;
    }

    pub fn apply_control_signal(&mut self, signal: &ControlSignal) {
        if let ControlSignal::ThermalSetpoint {
            heating_setpoint_c,
            cooling_setpoint_c,
            ..
        } = signal
        {
            self.runtime_setpoints = Some(RuntimeSetpointOverride {
                heating_c: *heating_setpoint_c,
                cooling_c: *cooling_setpoint_c,
            });
        }
    }

    pub fn set_schedule_setpoints(&mut self, schedule: ScheduleSetpoints) {
        self.schedule_setpoints = Some(schedule);
    }

    pub fn clear_schedule_setpoints(&mut self) {
        self.schedule_setpoints = None;
    }

    pub fn effective_setpoints(&self) -> ThermalSetpoints {
        self.static_setpoints
            .with_schedule_override(self.schedule_setpoints)
            .with_control_override(self.runtime_setpoints)
    }

    /// Resolve the current setpoint from config-owned schedule data and inject
    /// as `schedule_setpoints`. Priority:
    ///   1. Per-timestep schedule array (from CSV column)
    ///   2. 24-hour weekday/weekend profile (from HPXML thermostat)
    ///   3. None — falls through to static_setpoints
    fn resolve_profile_setpoints(&mut self, env: &EnvironmentState) {
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

    pub fn update_mode(&mut self, env: &EnvironmentState) -> crate::Result<ThermostatMode> {
        self.resolve_profile_setpoints(env);
        let zone_temp = lookup_zone_temp(env, self.zone_id)?;
        let setpoints = self.effective_setpoints();

        if !is_cycle_change_allowed(&self.thermostat, self.last_mode_switch_at, env.current_time) {
            return Ok(self.mode);
        }

        let hysteresis = self.thermostat.hysteresis_c;
        let offset = self.thermostat.deadband_offset.clamp(0.0, 1.0);
        let cutout = self.thermostat.cutout_ratio;

        // OCHRE HVAC.py lines 397–408: deadband_offset makes the band asymmetric.
        //   Heating turn_on  = setpoint − hysteresis × (1 − offset)
        //   Heating turn_off = setpoint + hysteresis × offset
        //
        // When offset=0: turn_on = setpoint − hysteresis, turn_off = setpoint (no overshoot)
        // When offset=0.2 (OCHRE default): the setpoint sits near the top of the
        //   deadband for heating and near the bottom for cooling.
        //
        // The legacy `cutout_ratio` path is used only when `deadband_offset == 0.0`
        // to preserve backward compatibility with configurations that did not set an
        // offset. In that case the symmetric ±hysteresis band with the cutout point
        // at `setpoint + hysteresis × cutout` is preserved.
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

        // Compressor-level minimum on/off time: block the transition if the
        // current mode's minimum hold time has not elapsed. The thermostat
        // `min_cycle_time_s` (above) handles debounce; this enforces the ASHRAE /
        // OCHRE short-cycle protection constraint.
        if !self.can_transition_mode(next_mode, env.current_time) {
            return Ok(self.mode);
        }

        self.set_mode(next_mode, env.current_time);
        Ok(self.mode)
    }

    pub fn update_mode_and_duty_with_ideal_solver<S: IdealCapacitySolver>(
        &mut self,
        env: &EnvironmentState,
        solver: &S,
    ) -> crate::Result<ThermostatMode> {
        let mode = self.update_mode(env)?;
        if mode == ThermostatMode::Deadband {
            self.duty_cycle = 0.0;
            return Ok(mode);
        }

        if self.use_ideal_capacity(env) {
            let ideal_rate_w = solver.solve_ideal_capacity(env, self.zone_id);
            let numerator = match mode {
                ThermostatMode::Heating => ideal_rate_w.max(0.0),
                ThermostatMode::Cooling => (-ideal_rate_w).max(0.0),
                ThermostatMode::Deadband => 0.0,
            };
            let rated_capacity = self.rated_capacity_w(mode).max(0.0);
            self.duty_cycle = if rated_capacity > 0.0 {
                (numerator / rated_capacity).clamp(0.0, 1.0)
            } else {
                0.0
            };
        } else {
            self.duty_cycle = 1.0;
        }

        Ok(mode)
    }

    pub fn use_ideal_capacity(&self, env: &EnvironmentState) -> bool {
        self.thermostat.use_ideal_capacity
            || env.time_res >= ChronoDuration::seconds(IDEAL_CAPACITY_TIME_RES_THRESHOLD_S)
    }

    /// Set the thermostat mode and record the transition timestamp.
    ///
    /// This is the only correct way to change `mode`. It atomically updates
    /// `mode_start_at` to `when`, upholding the invariant that `mode_start_at`
    /// always reflects when the current mode began. Callers that bypass this
    /// method by assigning `mode` directly will silently break minimum on/off
    /// time enforcement in `can_transition_mode`.
    pub fn set_mode(&mut self, mode: ThermostatMode, when: DateTime<Utc>) {
        if self.mode != mode {
            self.mode = mode;
            self.last_mode_switch_at = Some(when);
            self.mode_start_at = Some(when);
        }
    }

    /// Returns `false` when a minimum on-time or off-time constraint blocks the
    /// proposed mode transition.
    ///
    /// - Deadband → any On mode: blocked until the unit has been Off for at least
    ///   `min_off_time_s` (compressor short-cycle protection on restart).
    /// - Any On mode → Deadband: blocked until the unit has been On for at least
    ///   `min_on_time_s` (compressor short-cycle protection on shutdown).
    /// - On mode → different On mode (e.g. Heating↔Cooling reversal): blocked
    ///   until `min_on_time_s` in the current mode has elapsed.
    ///
    /// Returns `true` when `mode_start_at` is `None` (first transition ever) or
    /// when the minimum duration for the current mode has elapsed.
    pub fn can_transition_mode(&self, proposed: ThermostatMode, now: DateTime<Utc>) -> bool {
        if self.mode == proposed {
            return true; // no transition
        }
        let Some(start) = self.mode_start_at else {
            return true; // never been in a mode; allow
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

    pub fn evaluate_biquadratic(
        &self,
        curve_index: usize,
        t_indoor_c: f64,
        t_outdoor_c: f64,
    ) -> f64 {
        let coeffs = self
            .biquadratic_coeffs
            .get(curve_index)
            .copied()
            .or_else(|| self.biquadratic_coeffs.last().copied())
            .unwrap_or(DEFAULT_BIQUADRATIC_COEFFS);
        let curve = BiquadraticCurve {
            coeffs,
            x1_bounds: self.biquadratic_x1_bounds,
            x2_bounds: self.biquadratic_x2_bounds,
        };
        curve.evaluate(t_indoor_c, t_outdoor_c)
    }

    /// Evaluate a biquadratic curve and apply the flow-fraction correction.
    ///
    /// Returns `(raw, flow_adjusted)` where:
    /// - `raw` is the direct curve output
    /// - `flow_adjusted` is `raw * flow_fraction_correction`
    ///
    /// PLF is intentionally NOT applied here. Callers must apply it themselves:
    /// - Capacity curves: PLF does not apply.
    /// - EIR curves: divide `flow_adjusted` by PLF (efficiency penalty for cycling).
    pub fn evaluate_biquadratic_with_flow(
        &self,
        curve_index: usize,
        t_indoor_c: f64,
        t_outdoor_c: f64,
        flow_fraction_correction: f64,
    ) -> (f64, f64) {
        let raw = self.evaluate_biquadratic(curve_index, t_indoor_c, t_outdoor_c);
        let adjusted = raw * flow_fraction_correction;
        (raw, adjusted)
    }

    /// Compute the part-load factor (PLF) for the given PLR and speed stage.
    ///
    /// Two paths:
    /// 1. **Biquadratic PLR curve** (`eir_plr_coefficients` is `Some`):
    ///    OCHRE HVAC.py lines 823, 841–844: PLF = a + b·PLR + c·PLR² evaluated
    ///    from the per-speed `eir_plr` quadratic, then clamped to [0.7, 1.0].
    ///    The speed index selects the curve; out-of-range indices use the last entry.
    /// 2. **Simplified Cd formula** (fallback, backward-compatible):
    ///    PLF = 1 − Cd · (1 − PLR), clamped to [max(0.7, PLR), 1.0].
    ///
    /// Variable-speed equipment always returns 1.0 (no cycling degradation).
    pub fn part_load_factor(&mut self, plr: f64) -> f64 {
        self.part_load_factor_for_stage(plr, self.last_speed_index)
    }

    /// Compute PLF for an explicit speed stage index (used when the caller knows
    /// which stage was selected before `last_speed_index` is updated).
    pub fn part_load_factor_for_stage(&mut self, plr: f64, stage_index: usize) -> f64 {
        // Variable-speed equipment modulates compressor speed rather than cycling,
        // so the AHRI cycling-degradation penalty does not apply.
        if matches!(
            self.speed_control_mode,
            SpeedControlMode::VariableSpeedIdeal
        ) {
            self.plf_state = 1.0;
            return 1.0;
        }
        let plr = plr.clamp(0.0, 1.0);

        let plf_raw = if let Some(ref curves) = self.eir_plr_coefficients {
            // OCHRE biquadratic PLR path: per-speed quadratic coefficients.
            // a + b·PLR + c·PLR² (OCHRE _biquadratic with only plr terms active).
            let coeffs = curves
                .get(stage_index)
                .copied()
                .or_else(|| curves.last().copied())
                .unwrap_or([1.0, 0.0, 0.0]);
            quadratic(&coeffs, plr)
        } else {
            // Simplified Cd formula: PLF = 1 − Cd × (1 − PLR).
            let cd = self.plf_cooling_degradation_coeff.clamp(0.0, 1.0);
            1.0 - cd * (1.0 - plr)
        };

        // EnergyPlus constraints (ASHRAE 90.1 Appendix G / EnergyPlus I/O Reference):
        // PLF must be at least 0.7 (any lower indicates bad curve coefficients) and
        // must be >= PLR so that RTF = PLR/PLF never exceeds 1.0.
        if plf_raw < 0.7 {
            tracing::warn!(
                plf_raw,
                plr,
                stage_index,
                "PLF curve returned value < 0.7; check eir_plr or cooling_cd. \
                 Clamping to max(0.7, PLR)."
            );
        }
        let plf = plf_raw.clamp(0.7_f64.max(plr), 1.0);
        self.plf_state = plf;
        plf
    }

    /// Advance the speed-stage timer by `dt_s` seconds.
    /// Must be called once per timestep in the equipment `step()` method
    /// when the unit is running in `TwoSpeedSetpoint` mode.
    pub fn advance_speed_timer(&mut self, dt_s: f64) {
        self.time_at_current_speed_s += dt_s;
    }

    pub fn select_speed(&mut self, load_fraction: f64) -> SpeedSelection {
        self.select_speed_with_zone_temp(load_fraction, None, false)
    }

    /// Speed selection that accepts the current zone temperature and heating
    /// direction for `TwoSpeedTime` mode. Callers that know the zone temperature
    /// should prefer this over `select_speed`; the simpler `select_speed` wrapper
    /// is provided for call sites that do not track temperature.
    ///
    /// - `zone_temp_c` — current zone temperature; used only for `TwoSpeedTime`.
    /// - `is_heating`  — `true` for heating mode, `false` for cooling.
    pub fn select_speed_with_zone_temp(
        &mut self,
        load_fraction: f64,
        zone_temp_c: Option<f64>,
        is_heating: bool,
    ) -> SpeedSelection {
        let load_fraction = load_fraction.clamp(0.0, 1.0);
        let selection = match self.speed_control_mode {
            SpeedControlMode::SingleSpeed => SpeedSelection {
                speed_index: 0,
                part_load_ratio: load_fraction,
                speed_frac: load_fraction,
            },
            SpeedControlMode::TwoSpeedSetpoint => {
                let low_cap = self.low_speed_capacity_fraction.clamp(0.01, 0.999);
                let desired_index = if load_fraction > low_cap { 1 } else { 0 };
                let desired_index = self.apply_disabled_speeds_two_speed(desired_index);
                // OCHRE HVAC.py: min_time_in_speed — prevent speed hunting by
                // locking the current stage until the minimum dwell time elapses.
                let locked = self.time_at_current_speed_s < self.min_time_per_speed_s
                    && desired_index != self.last_speed_index;
                let speed_index = if locked {
                    self.last_speed_index
                } else {
                    if desired_index != self.last_speed_index {
                        self.time_at_current_speed_s = 0.0;
                    }
                    desired_index
                };
                if speed_index == 1 {
                    SpeedSelection {
                        speed_index: 1,
                        part_load_ratio: load_fraction,
                        speed_frac: 1.0,
                    }
                } else {
                    SpeedSelection {
                        speed_index: 0,
                        part_load_ratio: (load_fraction / low_cap).clamp(0.0, 1.0),
                        speed_frac: low_cap,
                    }
                }
            }
            SpeedControlMode::TwoSpeedTime => {
                // OCHRE HVAC.py lines 868–875 "Time" mode:
                // Start at low speed (index 0) on turn-on. Escalate to high speed
                // (index 1) if the zone temperature continues to move in the wrong
                // direction after min_time_per_speed_s has elapsed at the current stage.
                //
                //   Heating: "wrong direction" = temperature still dropping (current < prev)
                //   Cooling: "wrong direction" = temperature still rising  (current > prev)
                //
                // When prev_zone_temp_c is None (fresh cycle start after turn-off), force
                // speed 0 immediately — do not apply the min-time lock, which is only
                // intended to prevent speed hunting within an active cycle.
                let (desired_index, fresh_cycle) =
                    if let (Some(current), Some(prev)) = (zone_temp_c, self.prev_zone_temp_c) {
                        let moving_wrong_way = if is_heating {
                            current < prev
                        } else {
                            current > prev
                        };
                        let idx = if moving_wrong_way
                            && self.time_at_current_speed_s >= self.min_time_per_speed_s
                        {
                            1
                        } else {
                            self.last_speed_index
                        };
                        (idx, false)
                    } else {
                        (0, true) // no prior temperature data: start at low speed
                    };
                let desired_index = self.apply_disabled_speeds_two_speed(desired_index);
                // Skip the min-time lock when starting a fresh cycle: `prev_zone_temp_c`
                // being None means we just turned back on and must reset to low speed.
                let locked = !fresh_cycle
                    && self.time_at_current_speed_s < self.min_time_per_speed_s
                    && desired_index != self.last_speed_index;
                let speed_index = if locked {
                    self.last_speed_index
                } else {
                    if desired_index != self.last_speed_index {
                        self.time_at_current_speed_s = 0.0;
                    }
                    desired_index
                };
                let low_cap = self.low_speed_capacity_fraction.clamp(0.01, 0.999);
                if speed_index == 1 {
                    SpeedSelection {
                        speed_index: 1,
                        part_load_ratio: load_fraction,
                        speed_frac: 1.0,
                    }
                } else {
                    SpeedSelection {
                        speed_index: 0,
                        part_load_ratio: (load_fraction / low_cap).clamp(0.0, 1.0),
                        speed_frac: low_cap,
                    }
                }
            }
            SpeedControlMode::TwoSpeedAlternating => {
                // OCHRE HVAC.py lines 893–898 "Time2" mode:
                // Always runs at high speed (index 1) when on.
                let desired_index = self.apply_disabled_speeds_two_speed(1);
                if desired_index != self.last_speed_index {
                    self.time_at_current_speed_s = 0.0;
                }
                SpeedSelection {
                    speed_index: desired_index,
                    part_load_ratio: load_fraction,
                    speed_frac: 1.0,
                }
            }
            SpeedControlMode::MultiSpeedInterpolated => {
                let cap_fracs = self.capacity_fractions();
                if cap_fracs.is_empty() || load_fraction <= 0.0 {
                    SpeedSelection {
                        speed_index: 0,
                        speed_frac: 0.0,
                        part_load_ratio: 0.0,
                    }
                } else if load_fraction <= cap_fracs[0] {
                    // Below lowest stage capacity: cycle at speed 0.
                    SpeedSelection {
                        speed_index: 0,
                        speed_frac: 0.0,
                        part_load_ratio: load_fraction / cap_fracs[0],
                    }
                } else if load_fraction >= *cap_fracs.last().unwrap() {
                    // At or above max capacity: full output at top stage.
                    SpeedSelection {
                        speed_index: cap_fracs.len() - 1,
                        speed_frac: 0.0,
                        part_load_ratio: 1.0,
                    }
                } else {
                    // Inter-speed interpolation: find bracketing stages.
                    let hi = cap_fracs.partition_point(|&f| f < load_fraction);
                    let lo = hi - 1;
                    let span = cap_fracs[hi] - cap_fracs[lo];
                    let frac = if span > f64::EPSILON {
                        (load_fraction - cap_fracs[lo]) / span
                    } else {
                        0.0
                    };
                    SpeedSelection {
                        speed_index: lo,
                        speed_frac: frac,
                        part_load_ratio: 1.0,
                    }
                }
            }
            SpeedControlMode::VariableSpeedIdeal => SpeedSelection {
                speed_index: 0,
                part_load_ratio: 1.0,
                speed_frac: load_fraction,
            },
        };
        self.last_speed_index = selection.speed_index;
        self.last_speed_frac = selection.speed_frac;
        selection
    }

    /// Apply disabled-speed routing for two-speed modes.
    ///
    /// OCHRE HVAC.py lines 906–909: when the desired speed is disabled, route to
    /// the highest allowed (non-disabled) speed. If the `disabled_speeds` vec is
    /// empty (default), all speeds are enabled and `desired_index` is returned
    /// unchanged. If both stages are disabled this returns `desired_index`
    /// unchanged (misconfiguration — handled at a higher level).
    fn apply_disabled_speeds_two_speed(&self, desired_index: usize) -> usize {
        if self.disabled_speeds.is_empty() {
            return desired_index;
        }
        if self
            .disabled_speeds
            .get(desired_index)
            .copied()
            .unwrap_or(false)
        {
            self.max_enabled_speed
        } else {
            desired_index
        }
    }

    /// Apply the Winkler (2011) exponential startup capacity ramp.
    ///
    /// `steady_capacity_w` — the steady-state capacity before any startup penalty.
    /// `dt_min`            — timestep duration in minutes.
    ///
    /// Returns the derated capacity. When `c_d == 0` (variable-speed) or the ramp
    /// has completed, returns `steady_capacity_w` unchanged.
    pub fn apply_startup_capacity_degradation(
        &mut self,
        steady_capacity_w: f64,
        dt_min: f64,
    ) -> f64 {
        let on_now = self.duty_cycle > 0.0;
        let mult = self.startup.capacity_multiplier(on_now, dt_min);
        steady_capacity_w * mult
    }

    pub fn rated_capacity_w(&self, mode: ThermostatMode) -> f64 {
        match mode {
            ThermostatMode::Heating => self
                .heating_capacities_w
                .first()
                .copied()
                .unwrap_or_default(),
            ThermostatMode::Cooling => self
                .cooling_capacities_w
                .first()
                .copied()
                .unwrap_or_default(),
            ThermostatMode::Deadband => 0.0,
        }
    }

    pub fn capacity_at_stage(capacities: &[f64], stage_index: usize) -> f64 {
        if capacities.is_empty() {
            return 0.0;
        }
        capacities[stage_index.min(capacities.len() - 1)]
    }

    pub fn eir_at_stage(&self, stage_index: usize) -> f64 {
        if self.eir_by_stage.is_empty() {
            return 1.0;
        }
        self.eir_by_stage[stage_index.min(self.eir_by_stage.len() - 1)]
    }

    /// Normalized capacity fractions `cap[i] / cap[last]` for the populated capacities array.
    /// Uses whichever of heating/cooling has more stages (they should not both be populated
    /// for a single equipment instance, but if they are, the longer one wins).
    pub fn capacity_fractions(&self) -> Vec<f64> {
        let caps = if self.heating_capacities_w.len() >= self.cooling_capacities_w.len() {
            &self.heating_capacities_w
        } else {
            &self.cooling_capacities_w
        };
        let max_cap = caps.last().copied().unwrap_or(0.0);
        if max_cap <= 0.0 {
            return vec![];
        }
        caps.iter().map(|&c| c / max_cap).collect()
    }

    /// Interpolate capacity between two bracket stages using `speed_frac`.
    pub fn interpolated_capacity(&self, capacities: &[f64], speed_index: usize, speed_frac: f64) -> f64 {
        let cap_lo = Self::capacity_at_stage(capacities, speed_index);
        if speed_frac > 0.0 {
            let cap_hi = Self::capacity_at_stage(capacities, speed_index + 1);
            cap_lo * (1.0 - speed_frac) + cap_hi * speed_frac
        } else {
            cap_lo
        }
    }

    /// Interpolate EIR between two bracket stages using `speed_frac`.
    pub fn interpolated_eir(&self, speed_index: usize, speed_frac: f64) -> f64 {
        let eir_lo = self.eir_at_stage(speed_index);
        if speed_frac > 0.0 {
            let eir_hi = self.eir_at_stage(speed_index + 1);
            eir_lo * (1.0 - speed_frac) + eir_hi * speed_frac
        } else {
            eir_lo
        }
    }

    pub fn airflow_m3_s_for_capacity_w(&self, capacity_w: f64) -> f64 {
        capacity_w.max(0.0) * self.airflow_m3_s_per_w
    }

    pub fn fan_power_w(&self, airflow_m3_s: f64) -> f64 {
        airflow_m3_s.max(0.0) * self.fan_power_w_per_m3_s
    }

    pub fn sensible_latent_from_shr(&self, total_cooling_w: f64) -> (f64, f64) {
        let shr = self.shr.clamp(0.0, 1.0);
        let sensible = total_cooling_w * shr;
        let latent = total_cooling_w - sensible;
        (sensible, latent)
    }

    /// Compute `zone_heat_fractions` from `duct_dse` and `duct_zone_id`.
    ///
    /// Must be called after setting `duct_dse` and `duct_zone_id` in `init()`.
    /// Fractions are absolute multipliers on gross capacity:
    ///   - conditioned zone: `duct_dse`
    ///   - duct zone (if any and different from conditioned): `1.0 - duct_dse`
    ///
    /// When `duct_zone_id` is `None`, duct losses are unrecoverable (lost to
    /// outdoors). The conditioned-zone fraction equals `duct_dse`.
    ///
    /// OCHRE HVAC.py lines 188-197: `self.zone_fractions` computation.
    pub fn update_zone_heat_fractions(&mut self) {
        let dse = self.duct_dse.clamp(0.0, 1.0);
        let basement_frac = self.basement_heat_frac.clamp(0.0, 1.0);

        // Conditioned zone receives delivered heat minus any basement fraction.
        let conditioned_frac = dse * (1.0 - basement_frac);
        self.zone_heat_fractions = vec![(self.zone_id, conditioned_frac)];

        // Basement zone receives its share of delivered heat (if configured).
        if basement_frac > 0.0 {
            if let Some(basement_zone) = self.basement_zone_id {
                if basement_zone != self.zone_id {
                    self.zone_heat_fractions
                        .push((basement_zone, dse * basement_frac));
                }
            }
        }

        // Duct zone receives duct losses (if configured and distinct from conditioned).
        if dse < 1.0 {
            if let Some(duct_zone) = self.duct_zone_id {
                if duct_zone != self.zone_id {
                    self.zone_heat_fractions.push((duct_zone, 1.0 - dse));
                }
                // If duct_zone == zone_id, losses stay in the conditioned zone.
            }
        }
    }

    /// Distribute gross capacity across zones using absolute `zone_heat_fractions`.
    ///
    /// Each fraction is a direct multiplier on `sensible_gain_w` / `latent_gain_w`;
    /// fractions are **not** normalized. Callers must pass gross (pre-DSE) capacity.
    /// The conditioned-zone fraction equals `duct_dse`, so it naturally receives
    /// only the delivered portion. Any remainder is unrecoverable duct loss.
    pub fn write_zone_thermal_contributions(
        &self,
        ports: &mut PortSlots,
        sensible_gain_w: f64,
        latent_gain_w: f64,
    ) -> crate::Result<()> {
        let fractions: &[(ZoneId, f64)] = if self.zone_heat_fractions.is_empty() {
            &[(self.zone_id, 1.0)]
        } else {
            &self.zone_heat_fractions
        };

        // Fast path: single zone (>95% of dwellings)
        if fractions.len() == 1 {
            let f = fractions[0].1.max(0.0);
            return ports.accumulate(&PortContribution::Thermal {
                zone: fractions[0].0,
                sensible_gain_w: sensible_gain_w * f,
                latent_gain_w: latent_gain_w * f,
            });
        }

        // Multi-zone: fractions are absolute multipliers, not normalized.
        for &(zone, fraction) in fractions {
            if fraction > 0.0 {
                ports.accumulate(&PortContribution::Thermal {
                    zone,
                    sensible_gain_w: sensible_gain_w * fraction,
                    latent_gain_w: latent_gain_w * fraction,
                })?;
            }
        }
        Ok(())
    }

    pub fn update_supply_air_temp(&mut self, env: &EnvironmentState) {
        if self.equipment_type == HvacEquipmentType::AshpHeatPumpOnly {
            self.supply_air_temp_c = self
                .equipment_type
                .default_supply_air_temp_c(env.weather.outdoor_temp_c);
        }
    }

    /// Apply duct distribution system efficiency (DSE) to a capacity value.
    ///
    /// `capacity_w × duct_dse` is the effective delivered capacity after duct
    /// losses. `duct_dse = 1.0` means no duct losses (direct / ductless system).
    pub fn apply_duct_dse(&self, capacity_w: f64) -> f64 {
        capacity_w * self.duct_dse.clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration as ChronoDuration, TimeZone};
    use hares_types::{
        ControlSignal, EnvironmentState, GridState, PortSlots, SurfaceIrradiance,
        ThermalAccumulator, WeatherState, ZoneState,
    };

    use super::super::thermostat::{
        COOLING_DISABLED_SETPOINT_C, HEATING_DISABLED_SETPOINT_C, ScheduleSetpoints,
    };
    use super::*;
    use crate::EquipmentConfig;

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
                ground_temp_c: 10.0,
                sky_temp_c: 5.0,
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
            current_time: chrono::Utc
                .with_ymd_and_hms(2026, 3, 18, 0, 0, 0)
                .single()
                .expect("valid")
                + ChronoDuration::seconds(second),
            time_res: ChronoDuration::seconds(time_res_s),
        }
    }

    struct FixedIdealSolver(f64);

    impl IdealCapacitySolver for FixedIdealSolver {
        fn solve_ideal_capacity(&self, _env: &EnvironmentState, _zone: ZoneId) -> f64 {
            self.0
        }
    }

    #[test]
    fn thermostat_cutout_ratio_is_validated() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .raw_config
            .insert("cutout_ratio".to_string(), 1.2.into());
        let err = hvac
            .init(&config, &env(20.0, 60, 0))
            .expect_err("must fail");
        assert!(err.to_string().contains("cutout_ratio"));
    }

    #[test]
    fn thermostat_defaults_cutout_ratio_when_unspecified() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        let config = EquipmentConfig::default();
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        assert!((hvac.thermostat.cutout_ratio - DEFAULT_CUTOUT_RATIO).abs() < 1e-12);
    }

    #[test]
    fn thermostat_defaults_min_cycle_time_when_unspecified() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        let config = EquipmentConfig::default();
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        assert!((hvac.thermostat.min_cycle_time_s - DEFAULT_MIN_CYCLE_TIME_S).abs() < 1e-12);
    }

    #[test]
    fn thermostat_uses_explicit_cutout_ratio_override() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .raw_config
            .insert("cutout_ratio".to_string(), 0.1.into());
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        assert!((hvac.thermostat.cutout_ratio - 0.1).abs() < 1e-12);
    }

    #[test]
    fn default_min_cycle_time_blocks_turnoff_before_60s() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        let config = EquipmentConfig::default();
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        hvac.static_setpoints = ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 25.0,
        };

        let mode = hvac.update_mode(&env(18.0, 60, 0)).expect("turns on");
        assert_eq!(mode, ThermostatMode::Heating);

        let mode = hvac.update_mode(&env(22.0, 60, 30)).expect("still locked");
        assert_eq!(mode, ThermostatMode::Heating);

        let mode = hvac.update_mode(&env(22.0, 60, 61)).expect("lock expired");
        assert_eq!(mode, ThermostatMode::Deadband);
    }

    #[test]
    fn thermostat_setpoint_deadband_invariant_is_validated() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .raw_config
            .insert("hysteresis_c".to_string(), 1.0.into());
        config
            .raw_config
            .insert("heating_setpoint_c".to_string(), 21.0.into());
        config
            .raw_config
            .insert("cooling_setpoint_c".to_string(), 22.0.into());
        let err = hvac
            .init(&config, &env(20.0, 60, 0))
            .expect_err("must fail");
        assert!(err.to_string().contains("cooling-heating"));
    }

    #[test]
    fn thermostat_fsm_heating_cycle_turns_on_and_off() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.thermostat.hysteresis_c = 1.0;
        hvac.thermostat.cutout_ratio = 0.5;
        hvac.static_setpoints = ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 25.0,
        };

        let mode = hvac.update_mode(&env(18.9, 60, 0)).expect("mode updates");
        assert_eq!(mode, ThermostatMode::Heating);

        let mode = hvac.update_mode(&env(20.6, 60, 61)).expect("mode updates");
        assert_eq!(mode, ThermostatMode::Deadband);
    }

    #[test]
    fn min_cycle_time_holds_mode_until_elapsed() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.thermostat.min_cycle_time_s = 300.0;
        hvac.thermostat.hysteresis_c = 1.0;
        hvac.static_setpoints = ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 25.0,
        };

        let mode = hvac.update_mode(&env(18.0, 60, 0)).expect("mode updates");
        assert_eq!(mode, ThermostatMode::Heating);

        let mode = hvac.update_mode(&env(22.0, 60, 120)).expect("locked");
        assert_eq!(mode, ThermostatMode::Heating);

        let mode = hvac.update_mode(&env(22.0, 60, 301)).expect("unlocked");
        assert_eq!(mode, ThermostatMode::Deadband);
    }

    #[test]
    fn control_signal_thermal_setpoint_has_top_priority() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.static_setpoints = ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 24.0,
        };
        hvac.set_schedule_setpoints(ScheduleSetpoints {
            heating_c: Some(19.0),
            cooling_c: Some(25.0),
            ..ScheduleSetpoints::default()
        });
        hvac.apply_control_signal(&ControlSignal::ThermalSetpoint {
            heating_setpoint_c: Some(18.0),
            cooling_setpoint_c: Some(26.0),
            deadband_c: None,
        });

        let setpoints = hvac.effective_setpoints();
        assert_eq!(setpoints.heating_c, 18.0);
        assert_eq!(setpoints.cooling_c, 26.0);
    }

    #[test]
    fn no_space_heating_schedule_disables_heating_call() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.static_setpoints = ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 26.0,
        };
        hvac.set_schedule_setpoints(ScheduleSetpoints {
            no_space_heating: true,
            ..ScheduleSetpoints::default()
        });
        let mode = hvac.update_mode(&env(-20.0, 60, 0)).expect("mode updates");
        assert_eq!(mode, ThermostatMode::Deadband);
        assert_eq!(
            hvac.effective_setpoints().heating_c,
            HEATING_DISABLED_SETPOINT_C
        );
    }

    #[test]
    fn no_space_cooling_schedule_disables_cooling_call() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.static_setpoints = ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 24.0,
        };
        hvac.set_schedule_setpoints(ScheduleSetpoints {
            no_space_cooling: true,
            ..ScheduleSetpoints::default()
        });
        let mode = hvac.update_mode(&env(45.0, 60, 0)).expect("mode updates");
        assert_eq!(mode, ThermostatMode::Deadband);
        assert_eq!(
            hvac.effective_setpoints().cooling_c,
            COOLING_DISABLED_SETPOINT_C
        );
    }

    #[test]
    fn dse_reduces_effective_capacity() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.duct_dse = 0.8;
        assert_eq!(hvac.apply_duct_dse(10_000.0), 8_000.0);
    }

    #[test]
    fn supply_air_defaults_match_equipment_tables() {
        assert!((HvacEquipmentType::GasFurnace.default_supply_air_temp_c(8.3) - 54.4).abs() < 1e-9);
        assert!(
            (HvacEquipmentType::ElectricFurnace.default_supply_air_temp_c(8.3) - 48.9).abs() < 1e-9
        );
        assert!(
            (HvacEquipmentType::AshpHeatPumpOnly.default_supply_air_temp_c(8.3) - 32.2).abs()
                < 1e-9
        );
        assert!(
            (HvacEquipmentType::AshpHeatPumpAux.default_supply_air_temp_c(8.3) - 40.6).abs() < 1e-9
        );
        assert!(
            (HvacEquipmentType::MiniSplitHeat.default_supply_air_temp_c(8.3) - 43.3).abs() < 1e-9
        );
        // Cooling variants use 40.6 as a construction-time placeholder;
        // overwritten by step() before any real use.
        assert!(
            (HvacEquipmentType::AcCooler.default_supply_air_temp_c(8.3) - 40.6).abs() < 1e-9
        );
        assert!(
            (HvacEquipmentType::MiniSplitCool.default_supply_air_temp_c(8.3) - 40.6).abs() < 1e-9
        );
    }

    #[test]
    fn airflow_heating_defaults_to_350_and_scales_by_defect_ratio() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .raw_config
            .insert("AirflowDefectRatio".to_string(), 0.8.into());
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        // 350 CFM/ton * 0.8 defect ratio = 280 CFM/ton
        let expected = 280.0 * CFM_TO_M3_S / W_PER_TON;
        assert!((hvac.airflow_m3_s_per_w - expected).abs() < 1e-12);
    }

    #[test]
    fn airflow_cooling_defaults_to_312_and_scales_by_defect_ratio() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::AcCooler, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .raw_config
            .insert("AirflowDefectRatio".to_string(), 0.8.into());
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        // 312 CFM/ton * 0.8 defect ratio = 249.6 CFM/ton
        let expected = 249.6 * CFM_TO_M3_S / W_PER_TON;
        assert!((hvac.airflow_m3_s_per_w - expected).abs() < 1e-12);
    }

    #[test]
    fn ideal_capacity_mode_engages_on_coarse_timesteps_and_sets_duty() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.heating_capacities_w = vec![10_000.0];
        hvac.static_setpoints = ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 26.0,
        };
        let solver = FixedIdealSolver(5_000.0);
        let mode = hvac
            .update_mode_and_duty_with_ideal_solver(&env(18.0, 300, 0), &solver)
            .expect("mode updates");
        assert_eq!(mode, ThermostatMode::Heating);
        assert!((hvac.duty_cycle - 0.5).abs() < 1e-9);
    }

    #[test]
    fn two_hvac_instances_same_zone_accumulate_thermal_ports() {
        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        let hvac_a = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        let hvac_b = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));

        hvac_a
            .write_zone_thermal_contributions(&mut ports, 1000.0, 0.0)
            .expect("write a");
        hvac_b
            .write_zone_thermal_contributions(&mut ports, 1500.0, 0.0)
            .expect("write b");

        assert_eq!(ports.thermal[0].sensible_gain_w, 2500.0);
    }

    #[test]
    fn biquadratic_evaluation_matches_reference_value() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.biquadratic_coeffs = vec![[1.0, 0.2, 0.01, -0.1, 0.005, 0.02]];
        let x = 20.0;
        let y = 30.0;
        let expected = 1.0 + 0.2 * x + 0.01 * x * x - 0.1 * y + 0.005 * y * y + 0.02 * x * y;
        let got = hvac.evaluate_biquadratic(0, x, y);
        assert!((got - expected).abs() < 1e-12);
    }

    #[test]
    fn flow_fraction_is_applied_after_biquadratic_plf_is_not() {
        // PLF is NOT applied by evaluate_biquadratic_with_flow; callers handle
        // PLF separately: capacity does not use PLF, EIR divides by PLF.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.biquadratic_coeffs = vec![[1.0, 0.0, 0.0, 0.0, 0.0, 0.0]];
        let (raw, adjusted) = hvac.evaluate_biquadratic_with_flow(0, 19.0, 35.0, 1.1);
        assert!((raw - 1.0).abs() < 1e-12);
        assert!((adjusted - 1.1).abs() < 1e-12);
    }

    #[test]
    fn part_load_factor_matches_ticket_formula() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.plf_cooling_degradation_coeff = 0.25;
        let plf = hvac.part_load_factor(0.5);
        assert!((plf - 0.875).abs() < 1e-12);
    }

    #[test]
    fn two_speed_setpoint_control_switches_at_threshold() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.speed_control_mode = SpeedControlMode::TwoSpeedSetpoint;
        hvac.low_speed_capacity_fraction = 0.6;
        // Disable minimum dwell time so threshold-crossing tests are not blocked.
        hvac.min_time_per_speed_s = 0.0;
        let low = hvac.select_speed(0.4);
        let high = hvac.select_speed(0.8);
        assert_eq!(low.speed_index, 0);
        assert_eq!(high.speed_index, 1);
    }

    #[test]
    fn two_speed_min_time_locks_stage_during_dwell_period() {
        // OCHRE HVAC.py: min_time_in_speed prevents rapid hunting between low and
        // high compressor stages. With a 600 s lockout and 60 s steps, the stage
        // must remain locked for 10 consecutive steps after selection.
        //
        // Simulation order matches the actual control/step cycle:
        //   update_control() → select_speed()  (speed decision first)
        //   step()           → advance_speed_timer()  (then accumulate dwell time)
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.speed_control_mode = SpeedControlMode::TwoSpeedSetpoint;
        hvac.low_speed_capacity_fraction = 0.5;
        hvac.min_time_per_speed_s = 600.0;

        // Start at low speed with a low load.
        let sel = hvac.select_speed(0.3);
        assert_eq!(sel.speed_index, 0, "initial selection must be low stage");
        hvac.advance_speed_timer(60.0); // first step's dwell at low speed

        // Now jump to a high load — should be blocked by min-time.
        // Timer starts at 60 s; after 9 more select+advance cycles it will be 600 s.
        for step in 1..=9 {
            let sel = hvac.select_speed(0.9);
            assert_eq!(
                sel.speed_index, 0,
                "speed change must be blocked at step {step} while min_time_per_speed_s not elapsed"
            );
            hvac.advance_speed_timer(60.0);
        }
        // Timer is now 600 s = min_time_per_speed_s.

        // After 600 s the lock expires (OCHRE uses strict <); the next
        // select_speed with high load switches to stage 1.
        let sel = hvac.select_speed(0.9);
        assert_eq!(
            sel.speed_index, 1,
            "speed change must be allowed after min-time elapses"
        );
    }

    /// Helper: create an HvacEquipment in MultiSpeedInterpolated mode with 4 heating stages.
    fn make_msi_hvac() -> HvacEquipment {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.speed_control_mode = SpeedControlMode::MultiSpeedInterpolated;
        // Capacities: [4000, 6000, 8000, 10000] W → fractions [0.4, 0.6, 0.8, 1.0]
        hvac.heating_capacities_w = vec![4000.0, 6000.0, 8000.0, 10000.0];
        hvac
    }

    #[test]
    fn multi_speed_bracket_selection() {
        // load_fraction=0.5 is between cap_frac[0]=0.4 and cap_frac[1]=0.6
        // → speed_index=0, speed_frac = (0.5 - 0.4) / (0.6 - 0.4) = 0.5
        let mut hvac = make_msi_hvac();
        let sel = hvac.select_speed(0.5);
        assert_eq!(sel.speed_index, 0, "lower bracket index");
        assert!((sel.speed_frac - 0.5).abs() < 1e-12, "speed_frac={}", sel.speed_frac);
        assert_eq!(sel.part_load_ratio, 1.0, "PLR=1 when interpolating between stages");
    }

    #[test]
    fn multi_speed_plr_below_lowest_stage() {
        // load_fraction=0.30 < cap_frac[0]=0.40 → cycling at speed 0
        // PLR = 0.30 / 0.40 = 0.75
        let mut hvac = make_msi_hvac();
        let sel = hvac.select_speed(0.30);
        assert_eq!(sel.speed_index, 0);
        assert_eq!(sel.speed_frac, 0.0, "no interpolation at lowest stage");
        assert!((sel.part_load_ratio - 0.75).abs() < 1e-12, "PLR={}", sel.part_load_ratio);
    }

    #[test]
    fn multi_speed_midpoint_interpolation() {
        // load_fraction=0.7 is between cap_frac[1]=0.6 and cap_frac[2]=0.8
        // → speed_index=1, speed_frac = (0.7 - 0.6) / (0.8 - 0.6) = 0.5
        let mut hvac = make_msi_hvac();
        let sel = hvac.select_speed(0.7);
        assert_eq!(sel.speed_index, 1);
        assert!((sel.speed_frac - 0.5).abs() < 1e-12, "speed_frac={}", sel.speed_frac);
        assert_eq!(sel.part_load_ratio, 1.0);
    }

    #[test]
    fn multi_speed_full_capacity_clamp() {
        // load_fraction >= 1.0 → last stage, speed_frac=0, PLR=1
        let mut hvac = make_msi_hvac();
        let sel = hvac.select_speed(1.0);
        assert_eq!(sel.speed_index, 3, "top stage index");
        assert_eq!(sel.speed_frac, 0.0);
        assert_eq!(sel.part_load_ratio, 1.0);
    }

    #[test]
    fn variable_speed_matches_requested_capacity_fraction() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.speed_control_mode = SpeedControlMode::VariableSpeedIdeal;
        let sel = hvac.select_speed(0.63);
        assert!((sel.speed_frac - 0.63).abs() < 1e-12);
        assert_eq!(sel.part_load_ratio, 1.0);
    }

    #[test]
    fn ahri_210_240_plf_degradation_default() {
        // AHRI Standard 210/240-2023, S6.6.3
        // PLF = 1 - Cd * (1 - PLR), default Cd = 0.25
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.plf_cooling_degradation_coeff = super::DEFAULT_PLF_DEGRADATION_COEFF;

        // At PLR=0.5: PLF = 1 - 0.25*0.5 = 0.875
        let plf_half = hvac.part_load_factor(0.5);
        assert!(
            (plf_half - 0.875).abs() < 1e-12,
            "PLF at PLR=0.5: {plf_half}"
        );

        // At PLR=0.0: PLF = 1 - 0.25*1.0 = 0.75 (minimum)
        let plf_zero = hvac.part_load_factor(0.0);
        assert!(
            (plf_zero - 0.75).abs() < 1e-12,
            "PLF at PLR=0.0: {plf_zero}"
        );

        // At PLR=1.0: PLF = 1 - 0.25*0.0 = 1.0 (full load)
        let plf_full = hvac.part_load_factor(1.0);
        assert!((plf_full - 1.0).abs() < 1e-12, "PLF at PLR=1.0: {plf_full}");
    }

    #[test]
    fn startup_ramp_degrades_first_step_and_recovers_to_full() {
        // c_d=0.25 → t_full=5.4 min; with dt=1 min the ramp takes ~6 steps.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.startup.c_d = 0.25;
        hvac.duty_cycle = 1.0;

        // First on-step must be degraded (t = 0.5 min < t_full = 5.4 min).
        let step1 = hvac.apply_startup_capacity_degradation(10_000.0, 1.0);
        assert!(step1 < 10_000.0, "first startup step must degrade: {step1}");

        // After enough on-steps the ramp reaches full capacity.
        let mut last = step1;
        for _ in 0..10 {
            last = hvac.apply_startup_capacity_degradation(10_000.0, 1.0);
        }
        assert!(
            (last - 10_000.0).abs() < 1e-9,
            "must reach full after ramp: {last}"
        );

        // Turn off resets the ramp.
        hvac.duty_cycle = 0.0;
        let off = hvac.apply_startup_capacity_degradation(10_000.0, 1.0);
        assert!((off - 10_000.0).abs() < 1e-9);
        assert_eq!(hvac.startup.time_since_start_min, 0.0);

        // Restart must degrade again.
        hvac.duty_cycle = 1.0;
        let restart = hvac.apply_startup_capacity_degradation(10_000.0, 1.0);
        assert!(restart < 10_000.0, "restart must degrade again: {restart}");
    }

    #[test]
    fn mshp_plf_degradation_coeff_defaults_to_zero() {
        // MSHP selects discrete compressor stages rather than cycling, so the
        // AHRI cycling-degradation penalty (Cd) must be zero by default.
        for eq_type in [
            HvacEquipmentType::MiniSplitHeat,
            HvacEquipmentType::MiniSplitCool,
        ] {
            let hvac = HvacEquipment::new(eq_type, ZoneId(1));
            assert_eq!(
                hvac.plf_cooling_degradation_coeff, 0.0,
                "{eq_type:?} must default to Cd=0 (no cycling penalty)"
            );
        }
    }

    #[test]
    fn non_mshp_plf_degradation_coeff_defaults_to_ahri_standard() {
        for eq_type in [
            HvacEquipmentType::GasFurnace,
            HvacEquipmentType::ElectricFurnace,
            HvacEquipmentType::AshpHeatPumpOnly,
            HvacEquipmentType::AshpHeatPumpAux,
            HvacEquipmentType::AcCooler,
            HvacEquipmentType::Baseboard,
            HvacEquipmentType::Other,
        ] {
            let hvac = HvacEquipment::new(eq_type, ZoneId(1));
            assert_eq!(
                hvac.plf_cooling_degradation_coeff, DEFAULT_PLF_DEGRADATION_COEFF,
                "{eq_type:?} must default to AHRI standard Cd={DEFAULT_PLF_DEGRADATION_COEFF}"
            );
        }
    }

    #[test]
    fn biquadratic_clamps_extreme_inputs_to_default_bounds() {
        // OCHRE HVAC.py: default bounds ±100°C clamp physically impossible inputs.
        // At -200°C input, the curve must be evaluated as if the input were -100°C.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        // Linear curve: f(x1, x2) = x1 (coefficient on x1 = 1, rest 0)
        hvac.biquadratic_coeffs = vec![[0.0, 1.0, 0.0, 0.0, 0.0, 0.0]];
        // Default bounds are (-100.0, 100.0); input -200°C must be clamped to -100°C.
        let result = hvac.evaluate_biquadratic(0, -200.0, 30.0);
        assert!(
            (result - (-100.0)).abs() < 1e-9,
            "input -200°C must clamp to -100°C bound; got {result}"
        );
    }

    #[test]
    fn multi_speed_zero_load_returns_zero_plr() {
        let mut hvac = make_msi_hvac();
        let sel = hvac.select_speed(0.0);
        assert_eq!(sel.speed_index, 0);
        assert_eq!(sel.speed_frac, 0.0);
        assert_eq!(sel.part_load_ratio, 0.0, "zero load → zero PLR");
    }

    #[test]
    fn multi_speed_at_exact_stage_boundary() {
        // load_fraction = cap_frac[1] = 0.6 exactly.
        // partition_point(|&f| f < 0.6): cap_fracs[0]=0.4 < 0.6 (true), cap_fracs[1]=0.6 < 0.6 (false)
        // → hi=1, lo=0, frac = (0.6 - 0.4) / (0.6 - 0.4) = 1.0
        // This means "fully at the upper bracket" — equivalent to being at speed_index=1.
        let mut hvac = make_msi_hvac();
        let sel = hvac.select_speed(0.6);
        assert_eq!(sel.speed_index, 0, "lower bracket index is 0");
        assert!((sel.speed_frac - 1.0).abs() < 1e-12, "speed_frac=1.0 at exact upper boundary");
        assert_eq!(sel.part_load_ratio, 1.0);
    }

    #[test]
    fn grid_emergency_off_blocked_until_min_on_time_elapses() {
        // Verifies interaction between min-on-time (feature 2) and a GridEmergency
        // signal (feature 11) that tries to force the compressor off before the
        // minimum hold has elapsed.
        //
        // Scenario:
        //   t=0s   Heating begins; min_on_time_s=120s.
        //   t=60s  GridEmergency arrives → thermostat would prefer Deadband, but
        //          min-on-time blocks the transition. Equipment stays Heating.
        //   t=120s min_on_time has elapsed → Deadband transition is now allowed.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.thermostat.hysteresis_c = 1.0;
        hvac.thermostat.cutout_ratio = 0.5;
        hvac.static_setpoints = ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 25.0,
        };
        hvac.min_on_time_s = 120.0;

        // t=0s: zone is cold → enters Heating.
        let mode = hvac.update_mode(&env(18.0, 60, 0)).expect("initial mode");
        assert_eq!(mode, ThermostatMode::Heating, "must enter Heating at t=0");

        // t=60s: DR signal forces setpoints so thermostat would cut out
        // (simulate GridEmergency by pushing the zone temp far above cutout).
        // In a full DR implementation the signal would also set a setpoint
        // override; here we drive the same code path by passing a hot zone temp
        // while min_on_time has not yet elapsed.
        let mode = hvac.update_mode(&env(30.0, 60, 60)).expect("mode at 60s");
        assert_eq!(
            mode,
            ThermostatMode::Heating,
            "min_on_time_s=120 must block Heating→Deadband at t=60s even under DR pressure"
        );

        // t=120s: min_on_time has elapsed → equipment shuts off.
        let mode = hvac.update_mode(&env(30.0, 60, 120)).expect("mode at 120s");
        assert_eq!(
            mode,
            ThermostatMode::Deadband,
            "min_on_time_s=120 must release at t=120s and allow Deadband"
        );
    }

    #[test]
    fn low_seer_single_speed_overrides_cd_on_init() {
        // SEER < 13 → Cd = 0.20 (applies to both PLF coeff and startup ramp).
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .raw_config
            .insert("rated_seer".to_string(), 10.0.into());
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        assert!(
            (hvac.plf_cooling_degradation_coeff - 0.20).abs() < 1e-12,
            "low-SEER Cd must be 0.20, got {}",
            hvac.plf_cooling_degradation_coeff
        );
        assert!(
            (hvac.startup.c_d - 0.20).abs() < 1e-12,
            "startup c_d must also be 0.20 for low-SEER: {}",
            hvac.startup.c_d
        );
    }

    #[test]
    fn high_seer_single_speed_overrides_cd_on_init() {
        // SEER >= 13 → Cd = 0.07.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .raw_config
            .insert("rated_seer".to_string(), 15.0.into());
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        assert!(
            (hvac.plf_cooling_degradation_coeff - 0.07).abs() < 1e-12,
            "high-SEER Cd must be 0.07, got {}",
            hvac.plf_cooling_degradation_coeff
        );
        assert!(
            (hvac.startup.c_d - 0.07).abs() < 1e-12,
            "startup c_d must also be 0.07: {}",
            hvac.startup.c_d
        );
    }

    #[test]
    fn low_hspf_single_speed_overrides_cd_on_init() {
        // HSPF < 7 → Cd = 0.20.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .raw_config
            .insert("rated_hspf".to_string(), 6.5.into());
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        assert!(
            (hvac.plf_cooling_degradation_coeff - 0.20).abs() < 1e-12,
            "low-HSPF Cd must be 0.20, got {}",
            hvac.plf_cooling_degradation_coeff
        );
    }

    #[test]
    fn high_hspf_single_speed_overrides_cd_on_init() {
        // HSPF >= 7 → Cd = 0.11 (applies to both PLF coeff and startup ramp).
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .raw_config
            .insert("rated_hspf".to_string(), 8.0.into());
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        assert!(
            (hvac.plf_cooling_degradation_coeff - 0.11).abs() < 1e-12,
            "high-HSPF Cd must be 0.11, got {}",
            hvac.plf_cooling_degradation_coeff
        );
        assert!(
            (hvac.startup.c_d - 0.11).abs() < 1e-12,
            "startup c_d must also be 0.11 for high-HSPF: {}",
            hvac.startup.c_d
        );
    }

    #[test]
    fn two_speed_defaults_cd_to_0_11_on_init() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .raw_config
            .insert("speed_control_mode".to_string(), "two_speed".into());
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        assert!(
            (hvac.plf_cooling_degradation_coeff - 0.11).abs() < 1e-12,
            "two-speed must default Cd to 0.11, got {}",
            hvac.plf_cooling_degradation_coeff
        );
        assert!(
            (hvac.startup.c_d - 0.11).abs() < 1e-12,
            "two-speed startup c_d must be 0.11: {}",
            hvac.startup.c_d
        );
    }

    #[test]
    fn variable_speed_defaults_cd_to_zero_on_init() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .raw_config
            .insert("speed_control_mode".to_string(), "variable".into());
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        assert_eq!(
            hvac.startup.c_d, 0.0,
            "variable-speed must default startup c_d to 0.0"
        );
    }

    #[test]
    fn explicit_cd_config_overrides_derived_defaults() {
        // When "cooling_cd" is explicitly set, rating-based overrides must not apply.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .raw_config
            .insert("cooling_cd".to_string(), 0.15.into());
        config
            .raw_config
            .insert("rated_seer".to_string(), 10.0.into()); // would give 0.20 if derived
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        assert!(
            (hvac.plf_cooling_degradation_coeff - 0.15).abs() < 1e-12,
            "explicit cooling_cd must not be overridden by SEER table"
        );
        assert!(
            (hvac.startup.c_d - 0.15).abs() < 1e-12,
            "startup c_d must also respect explicit cooling_cd"
        );
    }

    #[test]
    fn biquadratic_bounds_loaded_from_config_override_defaults() {
        // OCHRE HVAC.py: biquadratic CSV specifies min_Twb/max_Twb/min_Tdb/max_Tdb.
        // Verify config keys override the ±100°C fallback defaults.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .raw_config
            .insert("biquadratic_x1_min".to_string(), 12.0.into());
        config
            .raw_config
            .insert("biquadratic_x1_max".to_string(), 30.0.into());
        config
            .raw_config
            .insert("biquadratic_x2_min".to_string(), (-15.0).into());
        config
            .raw_config
            .insert("biquadratic_x2_max".to_string(), 55.0.into());
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");

        assert_eq!(hvac.biquadratic_x1_bounds, (12.0, 30.0));
        assert_eq!(hvac.biquadratic_x2_bounds, (-15.0, 55.0));

        // Evaluation must clamp: x1=5.0 → 12.0, x2=60.0 → 55.0
        hvac.biquadratic_coeffs = vec![[0.0, 1.0, 0.0, 1.0, 0.0, 0.0]];
        let result = hvac.evaluate_biquadratic(0, 5.0, 60.0);
        let expected = 12.0 + 55.0; // clamped x1 + clamped x2
        assert!(
            (result - expected).abs() < 1e-9,
            "config bounds must clamp inputs; expected {expected}, got {result}"
        );
    }

    #[test]
    fn plf_formula_applies_rating_dependent_cd() {
        // Verify the PLF = 1 - Cd * (1 - PLR) formula produces correct values
        // for the three rating-dependent Cd values:
        //   default Cd=0.25, low-SEER Cd=0.2, high-HSPF Cd=0.11
        let plf = |cd: f64, plr: f64| 1.0 - cd * (1.0 - plr);
        let plr = 0.6;

        // Default AHRI Cd = 0.25
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.plf_cooling_degradation_coeff = 0.25;
        assert!((hvac.part_load_factor(plr) - plf(0.25, plr)).abs() < 1e-12);

        // Low-SEER Cd = 0.2
        hvac.plf_cooling_degradation_coeff = 0.2;
        assert!((hvac.part_load_factor(plr) - plf(0.2, plr)).abs() < 1e-12);

        // High-HSPF Cd = 0.11
        hvac.plf_cooling_degradation_coeff = 0.11;
        assert!((hvac.part_load_factor(plr) - plf(0.11, plr)).abs() < 1e-12);
    }

    // --- Feature: PLF cycling degradation floor (Ticket 11) ---

    #[test]
    fn plf_floor_prevents_plf_below_0_7() {
        // EnergyPlus constraint: PLF >= 0.7 regardless of curve.
        // With Cd=1.0 (extreme) and PLR=0.0: PLF_raw = 0.0, must be floored to 0.7.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.plf_cooling_degradation_coeff = 1.0;
        let plf = hvac.part_load_factor(0.0);
        assert!(
            plf >= 0.7,
            "PLF must be >= 0.7 (floor); got {plf} with Cd=1.0, PLR=0.0"
        );
    }

    #[test]
    fn plf_floor_prevents_plf_below_plr() {
        // EnergyPlus constraint: PLF >= PLR to keep RTF = PLR/PLF <= 1.0.
        // With Cd=0.5 and PLR=0.8: PLF_raw = 1 - 0.5*0.2 = 0.9 >= PLR, no change.
        // With Cd=0.5 and PLR=0.75: PLF_raw = 1 - 0.5*0.25 = 0.875 >= PLR, no change.
        // Edge: PLR clamped to 1.0, PLF_raw = 1.0 by formula, max stays 1.0.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.plf_cooling_degradation_coeff = 0.25;
        for &plr in &[0.0, 0.3, 0.5, 0.7, 0.9, 1.0] {
            let plf = hvac.part_load_factor(plr);
            assert!(
                plf >= plr,
                "PLF ({plf:.4}) must be >= PLR ({plr:.4}); RTF = PLR/PLF must be <= 1.0"
            );
            let rtf = (plr / plf.max(f64::EPSILON)).min(1.0);
            assert!(rtf <= 1.0, "RTF ({rtf:.4}) must be <= 1.0 at PLR={plr:.4}");
        }
    }

    #[test]
    fn plf_normal_range_not_affected_by_floor() {
        // Standard AHRI Cd=0.25 produces PLF well above 0.7 at any PLR >= 0.
        // PLF_raw at PLR=0.0: 1 - 0.25 = 0.75 > 0.7 — floor has no effect.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.plf_cooling_degradation_coeff = 0.25;
        let plf_at_zero = hvac.part_load_factor(0.0);
        assert!(
            (plf_at_zero - 0.75).abs() < 1e-12,
            "Cd=0.25 at PLR=0.0 must give PLF=0.75; got {plf_at_zero}"
        );
    }

    // --- Feature: compressor minimum on/off time (Ticket 2) ---

    #[test]
    fn can_transition_mode_allows_when_disabled() {
        // With defaults (min_on_time_s=0, min_off_time_s=0), all transitions
        // are always allowed regardless of elapsed time.
        let hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        let t0 = chrono::Utc
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .single()
            .expect("valid");
        // No mode_start_at → always allowed
        assert!(hvac.can_transition_mode(ThermostatMode::Heating, t0));
        assert!(hvac.can_transition_mode(ThermostatMode::Cooling, t0));
        assert!(hvac.can_transition_mode(ThermostatMode::Deadband, t0));
    }

    #[test]
    fn min_on_time_blocks_early_shutdown() {
        // With min_on_time_s=120s: transition from Heating→Deadband is blocked
        // until 120 s have elapsed in Heating mode.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.min_on_time_s = 120.0;

        let t0 = chrono::Utc
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .single()
            .expect("valid");
        // Simulate entering Heating mode at t0.
        hvac.mode = ThermostatMode::Heating;
        hvac.mode_start_at = Some(t0);

        // At 60 s: still blocked.
        let t60 = t0 + ChronoDuration::seconds(60);
        assert!(
            !hvac.can_transition_mode(ThermostatMode::Deadband, t60),
            "min_on_time_s=120 should block Heating→Deadband at 60s"
        );

        // At exactly 120 s: allowed (elapsed >= min_s).
        let t120 = t0 + ChronoDuration::seconds(120);
        assert!(
            hvac.can_transition_mode(ThermostatMode::Deadband, t120),
            "min_on_time_s=120 should allow Heating→Deadband at 120s"
        );
    }

    #[test]
    fn min_off_time_blocks_early_restart() {
        // With min_off_time_s=180s: transition from Deadband→Heating is blocked
        // until 180 s have elapsed in Deadband mode.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.min_off_time_s = 180.0;

        let t0 = chrono::Utc
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .single()
            .expect("valid");
        hvac.mode = ThermostatMode::Deadband;
        hvac.mode_start_at = Some(t0);

        // At 90 s: blocked.
        let t90 = t0 + ChronoDuration::seconds(90);
        assert!(
            !hvac.can_transition_mode(ThermostatMode::Heating, t90),
            "min_off_time_s=180 should block Deadband→Heating at 90s"
        );

        // At exactly 180 s: allowed.
        let t180 = t0 + ChronoDuration::seconds(180);
        assert!(
            hvac.can_transition_mode(ThermostatMode::Heating, t180),
            "min_off_time_s=180 should allow Deadband→Heating at 180s"
        );
    }

    #[test]
    fn update_mode_respects_min_on_time_when_configured() {
        // Wire the compressor minimum on-time through the full update_mode path.
        // Equipment with min_on_time_s=120 must stay Heating even when zone is warm.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.thermostat.hysteresis_c = 1.0;
        hvac.thermostat.cutout_ratio = 0.5;
        hvac.static_setpoints = ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 25.0,
        };
        hvac.min_on_time_s = 120.0;

        // Step 1: cold zone → enters Heating mode.
        let mode = hvac.update_mode(&env(18.0, 60, 0)).expect("mode updates");
        assert_eq!(mode, ThermostatMode::Heating, "should enter Heating");

        // Step 2 at 60s: zone is warm (above cutout), thermostat wants Deadband,
        // but min_on_time_s=120 blocks the transition.
        let mode = hvac
            .update_mode(&env(22.0, 60, 60))
            .expect("locked by min_on_time");
        assert_eq!(
            mode,
            ThermostatMode::Heating,
            "min_on_time_s=120 must hold Heating at 60s"
        );

        // Step 3 at 120s: min_on_time has elapsed, transition is allowed.
        let mode = hvac.update_mode(&env(22.0, 60, 120)).expect("unlocked");
        assert_eq!(
            mode,
            ThermostatMode::Deadband,
            "min_on_time_s=120 must release Heating at 120s"
        );
    }

    #[test]
    fn same_mode_transition_always_allowed() {
        // can_transition_mode returns true when proposed == current (no transition).
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.min_on_time_s = 9999.0;
        hvac.mode = ThermostatMode::Heating;
        let t0 = chrono::Utc
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .single()
            .expect("valid");
        hvac.mode_start_at = Some(t0);

        // Propose same mode — no transition needed, always allowed.
        assert!(
            hvac.can_transition_mode(ThermostatMode::Heating, t0),
            "same-mode 'transition' must always be allowed"
        );
    }

    #[test]
    fn biquadratic_parser_handles_negative_coefficients() {
        let input = "-1.0, -2.5, 3.0, -0.001, 0.5, -0.02";
        let result = super::super::core_config::parse_biquadratic_list(input).expect("should parse successfully");
        assert_eq!(result.len(), 1, "should produce exactly one curve");
        let coeffs = result[0];
        let expected = [-1.0_f64, -2.5, 3.0, -0.001, 0.5, -0.02];
        for (i, (&got, &exp)) in coeffs.iter().zip(expected.iter()).enumerate() {
            assert!(
                (got - exp).abs() < 1e-9,
                "coefficient {i}: expected {exp}, got {got}"
            );
        }
    }

    // --- Feature: DSE routing via zone_heat_fractions ---

    #[test]
    fn update_zone_heat_fractions_dse_one_produces_single_full_fraction() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.duct_dse = 1.0;
        hvac.duct_zone_id = None;
        hvac.update_zone_heat_fractions();
        assert_eq!(hvac.zone_heat_fractions, vec![(ZoneId(1), 1.0)]);
    }

    #[test]
    fn update_zone_heat_fractions_dse_below_one_no_duct_zone() {
        // Without a duct zone the conditioned-zone fraction equals DSE; duct losses
        // are unrecoverable (lost to outdoors). OCHRE HVAC.py line 190-192.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.duct_dse = 0.7;
        hvac.duct_zone_id = None;
        hvac.update_zone_heat_fractions();
        assert_eq!(hvac.zone_heat_fractions, vec![(ZoneId(1), 0.7)]);
    }

    #[test]
    fn update_zone_heat_fractions_dse_below_one_with_duct_zone() {
        // With a duct zone specified: conditioned gets DSE, duct zone gets 1-DSE.
        // OCHRE HVAC.py lines 190-192: zone_fractions[duct_zone] = 1 - duct_dse.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.duct_dse = 0.7;
        hvac.duct_zone_id = Some(ZoneId(2));
        hvac.update_zone_heat_fractions();
        assert_eq!(hvac.zone_heat_fractions.len(), 2);
        assert!((hvac.zone_heat_fractions[0].1 - 0.7).abs() < 1e-12);
        assert!((hvac.zone_heat_fractions[1].1 - 0.3).abs() < 1e-12);
        assert_eq!(hvac.zone_heat_fractions[0].0, ZoneId(1));
        assert_eq!(hvac.zone_heat_fractions[1].0, ZoneId(2));
    }

    #[test]
    fn write_zone_thermal_contributions_routes_duct_loss_to_duct_zone() {
        // With DSE=0.7 and a duct zone: 10_000 W gross → 7_000 W conditioned,
        // 3_000 W to duct zone.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.duct_dse = 0.7;
        hvac.duct_zone_id = Some(ZoneId(2));
        hvac.update_zone_heat_fractions();

        let mut ports = PortSlots {
            thermal: vec![
                ThermalAccumulator::new(ZoneId(1)),
                ThermalAccumulator::new(ZoneId(2)),
            ],
            ..PortSlots::default()
        };
        hvac.write_zone_thermal_contributions(&mut ports, 10_000.0, 0.0)
            .expect("write ok");

        assert!(
            (ports.thermal[0].sensible_gain_w - 7_000.0).abs() < 1e-9,
            "conditioned zone must get gross * DSE = 7_000 W"
        );
        assert!(
            (ports.thermal[1].sensible_gain_w - 3_000.0).abs() < 1e-9,
            "duct zone must get gross * (1-DSE) = 3_000 W"
        );
    }

    #[test]
    fn write_zone_thermal_contributions_discards_duct_loss_when_no_duct_zone() {
        // Without a duct zone, only the conditioned zone receives heat.
        // Duct loss (1-DSE) is discarded (unrecoverable).
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.duct_dse = 0.7;
        hvac.duct_zone_id = None;
        hvac.update_zone_heat_fractions();

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..PortSlots::default()
        };
        hvac.write_zone_thermal_contributions(&mut ports, 10_000.0, 0.0)
            .expect("write ok");

        assert!(
            (ports.thermal[0].sensible_gain_w - 7_000.0).abs() < 1e-9,
            "conditioned zone must get gross * DSE = 7_000 W; duct loss discarded"
        );
    }

    #[test]
    fn write_zone_thermal_contributions_duct_zone_same_as_conditioned_merges() {
        // OCHRE HVAC.py line 175-178: if duct_zone == zone, DSE is ignored.
        // update_zone_heat_fractions skips adding duct zone when it equals zone_id.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.duct_dse = 0.7;
        hvac.duct_zone_id = Some(ZoneId(1)); // same as conditioned zone
        hvac.update_zone_heat_fractions();
        // Only one entry since duct_zone == zone_id.
        assert_eq!(hvac.zone_heat_fractions.len(), 1);
        assert!(
            (hvac.zone_heat_fractions[0].1 - 0.7).abs() < 1e-12,
            "conditioned-zone fraction must still equal duct_dse"
        );
    }

    // --- Change 1: TwoSpeedTime and TwoSpeedAlternating speed control ---

    #[test]
    fn two_speed_time_starts_low_and_escalates_on_continued_temperature_drop() {
        // OCHRE "Time" mode: start at low speed; switch to high if temperature
        // still moving away from setpoint after min_time_per_speed_s.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.speed_control_mode = SpeedControlMode::TwoSpeedTime;
        hvac.low_speed_capacity_fraction = 0.5;
        hvac.min_time_per_speed_s = 300.0;

        // Step 1: no prior temp → start at low speed.
        let sel = hvac.select_speed_with_zone_temp(0.8, Some(19.0), true);
        assert_eq!(sel.speed_index, 0, "first step must start at low speed");
        hvac.advance_speed_timer(300.0); // dwell min_time_per_speed_s at low

        // Step 2: temperature still dropping (19.0 → 18.5) → escalate to high.
        hvac.update_prev_zone_temp(Some(19.0));
        let sel = hvac.select_speed_with_zone_temp(0.8, Some(18.5), true);
        assert_eq!(
            sel.speed_index, 1,
            "temperature still dropping after min-time → must escalate to high speed"
        );
    }

    #[test]
    fn two_speed_time_does_not_escalate_if_temperature_recovering() {
        // If temperature is moving toward setpoint, keep current low speed.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.speed_control_mode = SpeedControlMode::TwoSpeedTime;
        hvac.low_speed_capacity_fraction = 0.5;
        hvac.min_time_per_speed_s = 300.0;
        hvac.update_prev_zone_temp(Some(19.0));

        // Dwell past min-time.
        hvac.advance_speed_timer(300.0);

        // Temperature rising (19.0 → 19.5): zone is recovering → stay at low.
        let sel = hvac.select_speed_with_zone_temp(0.8, Some(19.5), true);
        assert_eq!(
            sel.speed_index, 0,
            "temperature recovering → must stay at low speed"
        );
    }

    #[test]
    fn two_speed_alternating_always_selects_high_speed() {
        // OCHRE "Time2" mode: always run at high speed (index 1) when on.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.speed_control_mode = SpeedControlMode::TwoSpeedAlternating;
        hvac.low_speed_capacity_fraction = 0.5;

        let sel = hvac.select_speed(0.3);
        assert_eq!(
            sel.speed_index, 1,
            "TwoSpeedAlternating must always select high speed (index 1)"
        );
        let sel = hvac.select_speed(0.9);
        assert_eq!(
            sel.speed_index, 1,
            "TwoSpeedAlternating must always select high speed regardless of load"
        );
    }

    #[test]
    fn speed_control_mode_parses_time_and_alternating_from_config() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .raw_config
            .insert("speed_control_mode".to_string(), "time".into());
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        assert_eq!(hvac.speed_control_mode, SpeedControlMode::TwoSpeedTime);

        let mut config2 = EquipmentConfig::default();
        config2
            .raw_config
            .insert("speed_control_mode".to_string(), "time2".into());
        hvac.init(&config2, &env(20.0, 60, 0)).expect("init ok");
        assert_eq!(
            hvac.speed_control_mode,
            SpeedControlMode::TwoSpeedAlternating
        );
    }

    // --- Change 2: Per-speed biquadratic PLR curves ---

    #[test]
    fn eir_plr_curve_replaces_cd_formula_when_configured() {
        // PLR curve: PLF = 0.85 + 0.15 * PLR + 0.0 * PLR²
        // At PLR=0.6: PLF = 0.85 + 0.09 = 0.94
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.eir_plr_coefficients = Some(vec![[0.85, 0.15, 0.0]]);
        let plf = hvac.part_load_factor(0.6);
        assert!(
            (plf - 0.94).abs() < 1e-12,
            "per-speed PLR curve PLF must be 0.94 at PLR=0.6; got {plf}"
        );
    }

    #[test]
    fn eir_plr_falls_back_to_cd_when_not_configured() {
        // Without per-speed curves, PLF = 1 - Cd * (1 - PLR).
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.eir_plr_coefficients = None;
        hvac.plf_cooling_degradation_coeff = 0.25;
        let plf = hvac.part_load_factor(0.5);
        assert!(
            (plf - 0.875).abs() < 1e-12,
            "fallback Cd=0.25 at PLR=0.5 must give PLF=0.875; got {plf}"
        );
    }

    #[test]
    fn eir_plr_per_speed_selects_correct_curve_by_stage() {
        // Two curves: stage 0 = [0.9, 0.1, 0], stage 1 = [0.8, 0.2, 0].
        // At PLR=0.5: stage 0 → PLF=0.95, stage 1 → PLF=0.90.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.eir_plr_coefficients = Some(vec![[0.9, 0.1, 0.0], [0.8, 0.2, 0.0]]);

        hvac.last_speed_index = 0;
        let plf0 = hvac.part_load_factor(0.5);
        assert!(
            (plf0 - 0.95).abs() < 1e-12,
            "stage 0 PLF at PLR=0.5 must be 0.95; got {plf0}"
        );

        hvac.last_speed_index = 1;
        let plf1 = hvac.part_load_factor(0.5);
        assert!(
            (plf1 - 0.90).abs() < 1e-12,
            "stage 1 PLF at PLR=0.5 must be 0.90; got {plf1}"
        );
    }

    #[test]
    fn eir_plr_curve_clamped_to_min_0_7() {
        // Even with a curve that produces < 0.7, PLF must be floored to 0.7.
        // Curve: PLF = 0.5 + 0.1 * PLR at PLR=0 → 0.5 (below floor).
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.eir_plr_coefficients = Some(vec![[0.5, 0.1, 0.0]]);
        let plf = hvac.part_load_factor(0.0);
        assert!(
            plf >= 0.7,
            "PLF from curve must be clamped to at least 0.7; got {plf}"
        );
    }

    // --- Change 3: Speed disabling for demand response ---

    #[test]
    fn set_disabled_speeds_routes_to_max_enabled_when_desired_disabled() {
        // OCHRE HVAC.py lines 906–909: disabled speed → highest allowed speed.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.speed_control_mode = SpeedControlMode::TwoSpeedSetpoint;
        hvac.low_speed_capacity_fraction = 0.5;
        hvac.min_time_per_speed_s = 0.0;

        // Disable high speed (index 1).
        hvac.set_disabled_speeds(&[false, true]);
        assert_eq!(hvac.max_enabled_speed, 0, "highest non-disabled is index 0");

        // Load fraction above threshold would normally select high speed.
        let sel = hvac.select_speed(0.8);
        assert_eq!(
            sel.speed_index, 0,
            "high speed disabled → must route to index 0 (max enabled)"
        );
    }

    #[test]
    fn set_disabled_speeds_empty_re_enables_all() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.speed_control_mode = SpeedControlMode::TwoSpeedSetpoint;
        hvac.low_speed_capacity_fraction = 0.5;
        hvac.min_time_per_speed_s = 0.0;

        hvac.set_disabled_speeds(&[false, true]);
        let sel = hvac.select_speed(0.8);
        assert_eq!(sel.speed_index, 0, "high speed disabled initially");

        // Re-enable all.
        hvac.set_disabled_speeds(&[]);
        let sel = hvac.select_speed(0.8);
        assert_eq!(
            sel.speed_index, 1,
            "after re-enabling, high speed must be selectable again"
        );
    }

    #[test]
    fn n_speed_stages_matches_mode() {
        for (mode, expected) in [
            (SpeedControlMode::SingleSpeed, 1),
            (SpeedControlMode::TwoSpeedSetpoint, 2),
            (SpeedControlMode::TwoSpeedTime, 2),
            (SpeedControlMode::TwoSpeedAlternating, 2),
            (SpeedControlMode::VariableSpeedIdeal, 1),
        ] {
            let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
            hvac.speed_control_mode = mode;
            assert_eq!(
                hvac.n_speed_stages(),
                expected,
                "{mode:?} must have {expected} speed stages"
            );
        }
        // MultiSpeedInterpolated derives stage count from capacities.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.speed_control_mode = SpeedControlMode::MultiSpeedInterpolated;
        hvac.heating_capacities_w = vec![4000.0, 6000.0, 8000.0, 10000.0];
        assert_eq!(hvac.n_speed_stages(), 4, "MultiSpeedInterpolated with 4 stages");
    }

    // --- Change 4: Deadband offset (asymmetric setpoint bands) ---

    #[test]
    fn deadband_offset_defaults_to_0_2() {
        let hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        assert!(
            (hvac.thermostat.deadband_offset - 0.2).abs() < 1e-12,
            "default deadband_offset must be 0.2 (OCHRE default)"
        );
    }

    #[test]
    fn deadband_offset_loaded_from_config() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .raw_config
            .insert("deadband_offset".to_string(), 0.3.into());
        hvac.init(&config, &env(20.0, 60, 0)).expect("init ok");
        assert!(
            (hvac.thermostat.deadband_offset - 0.3).abs() < 1e-12,
            "deadband_offset must be loaded from config"
        );
    }

    #[test]
    fn deadband_offset_asymmetric_heating_turn_on_threshold() {
        // With setpoint=20°C, hysteresis=1°C, offset=0.2:
        //   turn_on  = 20 - 1×(1-0.2) = 20 - 0.8 = 19.2°C
        //   turn_off = 20 + 1×0.2     = 20.2°C
        // At 19.3°C the furnace must be in Deadband.
        // At 19.1°C it must enter Heating.
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.thermostat.hysteresis_c = 1.0;
        hvac.thermostat.deadband_offset = 0.2;
        hvac.static_setpoints = ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 26.0,
        };

        // 19.3°C: above turn_on threshold (19.2°C) → Deadband.
        let mode = hvac.update_mode(&env(19.3, 60, 0)).expect("mode");
        assert_eq!(
            mode,
            ThermostatMode::Deadband,
            "19.3°C is above turn-on 19.2°C → must stay Deadband"
        );

        // 19.1°C: below turn_on threshold → Heating.
        let mode = hvac.update_mode(&env(19.1, 60, 1)).expect("mode");
        assert_eq!(
            mode,
            ThermostatMode::Heating,
            "19.1°C is below turn-on 19.2°C → must enter Heating"
        );

        // Heating turn_off at 20.2°C.
        let mode = hvac.update_mode(&env(20.3, 60, 2)).expect("mode");
        assert_eq!(
            mode,
            ThermostatMode::Deadband,
            "20.3°C exceeds heating turn-off 20.2°C → must return to Deadband"
        );
    }

    #[test]
    fn deadband_offset_zero_uses_legacy_symmetric_hysteresis() {
        // With offset=0.0 the legacy symmetric path is used:
        //   turn_on  = setpoint - hysteresis
        //   turn_off = setpoint + hysteresis × cutout (cutout=0 → at setpoint)
        let mut hvac = HvacEquipment::new(HvacEquipmentType::GasFurnace, ZoneId(1));
        hvac.thermostat.hysteresis_c = 1.0;
        hvac.thermostat.deadband_offset = 0.0;
        hvac.thermostat.cutout_ratio = 0.0;
        hvac.static_setpoints = ThermalSetpoints {
            heating_c: 20.0,
            cooling_c: 26.0,
        };

        // Below 19°C → Heating.
        let mode = hvac.update_mode(&env(18.9, 60, 0)).expect("mode");
        assert_eq!(mode, ThermostatMode::Heating, "18.9°C < 19°C → Heating");

        // Any temperature at or above setpoint and cutout=0 → Deadband immediately.
        let mode = hvac.update_mode(&env(20.1, 60, 1)).expect("mode");
        assert_eq!(
            mode,
            ThermostatMode::Deadband,
            "20.1°C > 20°C (cutout=0) → Deadband"
        );
    }

    /// `TwoSpeedTime` must restart at low speed (index 0) on a new heating cycle,
    /// even if the previous cycle ended at high speed.
    ///
    /// OCHRE HVAC.py "Time" mode: `prev_zone_temp_c = None` when the unit shuts off,
    /// so the next `select_speed_with_zone_temp` call takes the `else { 0 }` branch
    /// and returns index 0 regardless of `last_speed_index`.
    #[test]
    fn two_speed_time_restarts_at_low_speed_after_off_cycle() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        hvac.speed_control_mode = SpeedControlMode::TwoSpeedTime;
        hvac.low_speed_capacity_fraction = 0.5;
        hvac.min_time_per_speed_s = 300.0;

        // Simulate a completed cycle: HVAC reached high speed (index 1).
        hvac.update_prev_zone_temp(Some(19.0));
        hvac.advance_speed_timer(300.0);
        let sel = hvac.select_speed_with_zone_temp(0.8, Some(18.5), true);
        assert_eq!(
            sel.speed_index, 1,
            "precondition: cycle escalated to high speed"
        );

        // HVAC turns off: clear prev_zone_temp so the next on-cycle starts fresh.
        hvac.update_prev_zone_temp(None);

        // Re-enable: first speed selection with no prior temp must start at low speed,
        // regardless of last_speed_index still being 1.
        let sel = hvac.select_speed_with_zone_temp(0.8, Some(18.5), true);
        assert_eq!(
            sel.speed_index, 0,
            "TwoSpeedTime must start at low speed (0) on re-enable; got {}",
            sel.speed_index
        );
    }

    #[test]
    fn deadband_offset_validation_rejects_out_of_range() {
        let mut hvac = HvacEquipment::new(HvacEquipmentType::Other, ZoneId(1));
        let mut config = EquipmentConfig::default();
        config
            .raw_config
            .insert("deadband_offset".to_string(), 1.5.into());
        let err = hvac
            .init(&config, &env(20.0, 60, 0))
            .expect_err("must fail for deadband_offset > 1.0");
        assert!(
            err.to_string().contains("deadband_offset"),
            "error must mention deadband_offset"
        );
    }
}
