//! Defrost control for heat-pump heating: OnDemand (humidity-based) and Timed modes.

use hares_physics::psychrometrics::humidity_ratio_from_twb;
use serde::{Deserialize, Serialize};
use tracing::debug;

use super::constants::{
    DEFAULT_DEFROST_CYCLE_DURATION_S, DEFAULT_DEFROST_TIME_FRACTION,
    DEFROST_CAPACITY_MULTIPLIER_BASE, DEFROST_CAPACITY_UNIT_FACTOR, DEFROST_COIL_TEMP_OFFSET_C,
    DEFROST_COIL_TEMP_SLOPE, DEFROST_EIR_CURVE_TEMP_MIN_C, DEFROST_EIR_TEMP_MODIFIER,
    DEFROST_ENABLE_TEMP_C, DEFROST_EWMA_TAU_S, DEFROST_MIN_DELTA_HUMIDITY_RATIO,
    DEFROST_POWER_MULTIPLIER_NUMERATOR, DEFROST_Q_MULTIPLIER, DEFROST_REFERENCE_TEMP_C,
    DEFROST_TIME_FRACTION_NUMERATOR, FROST_DECAY_CLEAR_TEMP_C, FROST_DECAY_TAU_AT_0C_S,
    FROST_DECAY_TAU_AT_10C_S, MAX_DEFROST_CYCLE_DURATION_S, TIMED_DEFROST_CAP_MULT_BASE,
    TIMED_DEFROST_CAP_MULT_SLOPE, TIMED_DEFROST_PWR_MULT_BASE, TIMED_DEFROST_PWR_MULT_SLOPE,
};

/// Defrost activation / timing strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DefrostControl {
    /// Humidity-based -- defrost fraction computed from outdoor coil moisture
    /// accumulation. More physical but requires humidity data (OCHRE default).
    #[default]
    OnDemand,
    /// Timer-based -- fixed defrost time fraction. Simpler, used by many real
    /// units. Uses different capacity/EIR multiplier equations from DOE-2.
    Timed,
    /// Defrost is disabled. Used for ground-source and water-source heat pumps
    /// where the source-side temperature stays above freezing year-round.
    /// GSHP ground loops are typically at 0–15 °C, well above the frost threshold.
    Disabled,
}

/// How defrost heat is delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DefrostStrategy {
    /// Reverse refrigerant cycle (most common in modern heat pumps).
    #[default]
    ReverseCycle,
    /// Resistive heating element.
    Resistive,
}

/// Full defrost configuration including control mode and strategy.
///
/// Embedded in `HeatPumpHeaterConfig` via `#[serde(flatten)]`, following the
/// same pattern as `DuctConfig`. All fields default to the values produced by
/// `DefrostConfig::on_demand(1.0, 0.0)` so that existing configs without
/// defrost keys are unaffected.
///
/// `deny_unknown_fields` is intentionally omitted: serde `flatten` is
/// incompatible with `deny_unknown_fields` on the inner (flattened) struct.
/// Unknown-field rejection is the responsibility of the outer struct
/// (`HeatPumpCommonConfig` / `HeatPumpHeaterConfig`).
/// See <https://serde.rs/attr-flatten.html>.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct DefrostConfig {
    /// Legacy capacity scaling factor applied after defrost multiplier [0..1].
    /// Default 1.0: no additional reduction beyond the defrost multiplier.
    #[serde(
        default = "default_capacity_reduction_factor",
        rename = "defrost_capacity_reduction_factor"
    )]
    pub capacity_reduction_factor: f64,
    /// Additional fixed defrost power draw [W] (legacy/OCHRE field).
    #[serde(default)]
    pub defrost_power_w: f64,
    /// Control mode: OnDemand (humidity-based) or Timed.
    /// Default OnDemand: matches OCHRE default and the previous hardcoded path.
    /// EnergyPlus defaults to Timed, but HARES follows the OCHRE physics-based approach.
    #[serde(default, rename = "defrost_control")]
    pub control: DefrostControl,
    /// Defrost strategy: ReverseCycle or Resistive.
    /// Default ReverseCycle: most common in modern heat pumps.
    /// EnergyPlus I/O Reference `Coil:Heating:DX`: "If this input field is left blank,
    /// the default defrost strategy is reverse-cycle."
    #[serde(default, rename = "defrost_strategy")]
    pub strategy: DefrostStrategy,
    /// For Timed mode: fraction of hour spent in defrost [0..1].
    /// Typical value: 0.058 (~3.5 min/hr). Ignored in OnDemand mode.
    /// EnergyPlus I/O Reference: default 0.058333 (3.5/60).
    /// HARES uses 0.058 (truncated), matching DEFAULT_DEFROST_TIME_FRACTION.
    #[serde(default = "default_defrost_time_fraction")]
    pub defrost_time_fraction: f64,
    /// Maximum OAT for defrost activation [°C].
    /// OCHRE/HARES default: 4.4445°C (≈40°F). EnergyPlus default: 5°C.
    /// HARES follows OCHRE's 40°F threshold.
    #[serde(default = "default_max_oat_defrost_c", rename = "defrost_max_oat_c")]
    pub max_oat_defrost_c: f64,
    /// For ReverseCycle + Timed strategy: optional biquadratic EIR curve coefficients
    /// `[c0, c1, c2, c3, c4, c5]` evaluated at `(wb, db)` with a 15.555°C floor.
    /// `None` means no EIR adjustment (factor = 1.0).
    /// EnergyPlus `Coil:Heating:DX` field: `Defrost Energy Input Ratio Function of
    /// Temperature Curve Name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defrost_eir_coeffs: Option<[f64; 6]>,
    /// For Resistive strategy: rated defrost heater capacity [W].
    /// Must be > 0 when `strategy == Resistive`; zero is valid for ReverseCycle
    /// (the default) since reverse-cycle defrost uses the compressor, not a
    /// dedicated heater.
    #[serde(default)]
    pub resistive_defrost_capacity_w: f64,
}

fn default_capacity_reduction_factor() -> f64 {
    1.0
}

fn default_defrost_time_fraction() -> f64 {
    DEFAULT_DEFROST_TIME_FRACTION
}

fn default_max_oat_defrost_c() -> f64 {
    DEFROST_ENABLE_TEMP_C
}

impl DefrostConfig {
    /// Construct a backward-compatible OnDemand config from legacy fields.
    #[must_use]
    pub fn on_demand(capacity_reduction_factor: f64, defrost_power_w: f64) -> Self {
        Self {
            capacity_reduction_factor,
            defrost_power_w,
            control: DefrostControl::OnDemand,
            strategy: DefrostStrategy::ReverseCycle,
            defrost_time_fraction: DEFAULT_DEFROST_TIME_FRACTION,
            max_oat_defrost_c: DEFROST_ENABLE_TEMP_C,
            defrost_eir_coeffs: None,
            resistive_defrost_capacity_w: 0.0,
        }
    }

    /// Validate defrost configuration ranges and logical constraints.
    ///
    /// Returns `Err` with a descriptive message for:
    /// - `capacity_reduction_factor` outside [0.0, 1.0]
    /// - `defrost_time_fraction` outside [0.0, 1.0]
    /// - `max_oat_defrost_c` outside [-30.0, 21.0]
    /// - `resistive_defrost_capacity_w` < 0
    /// - `Resistive` strategy with `resistive_defrost_capacity_w == 0.0`
    ///   (a resistive defrost heater with zero capacity is a config error)
    pub fn validate(&self) -> Result<(), String> {
        if !(0.0..=1.0).contains(&self.capacity_reduction_factor) {
            return Err(format!(
                "defrost_capacity_reduction_factor must be in [0.0, 1.0], got {}",
                self.capacity_reduction_factor
            ));
        }
        if !(0.0..=1.0).contains(&self.defrost_time_fraction) {
            return Err(format!(
                "defrost_time_fraction must be in [0.0, 1.0], got {}",
                self.defrost_time_fraction
            ));
        }
        if !(-30.0..=21.0).contains(&self.max_oat_defrost_c) {
            return Err(format!(
                "defrost_max_oat_c must be in [-30.0, 21.0], got {}",
                self.max_oat_defrost_c
            ));
        }
        if self.resistive_defrost_capacity_w < 0.0 {
            return Err(format!(
                "resistive_defrost_capacity_w must be >= 0, got {}",
                self.resistive_defrost_capacity_w
            ));
        }
        if self.strategy == DefrostStrategy::Resistive && self.resistive_defrost_capacity_w == 0.0 {
            return Err(
                "defrost_strategy is Resistive but resistive_defrost_capacity_w is 0.0; \
                 a resistive defrost heater requires a positive rated capacity"
                    .to_string(),
            );
        }
        if let Some(coeffs) = &self.defrost_eir_coeffs {
            for (i, c) in coeffs.iter().enumerate() {
                if !c.is_finite() {
                    return Err(format!("defrost_eir_coeffs[{i}] must be finite, got {c}"));
                }
            }
        }
        Ok(())
    }
}

impl Default for DefrostConfig {
    fn default() -> Self {
        Self::on_demand(1.0, 0.0)
    }
}

/// Discrete defrost cycle state.
///
/// HARES-specific enhancement: models explicit ON/OFF defrost cycling rather than
/// the continuous/fractional approach in EnergyPlus ERM 26.1 — Coils: Single-Speed Electric DX Air Heating Coil — Defrost Operation. The continuous model
/// averages the defrost penalty across each timestep, underestimating peak power draw
/// and overestimating average capacity. The discrete model transitions between frost
/// accumulation and active defrost with distinct capacity/EIR in each phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DefrostCycleState {
    /// Normal heating — frost building on outdoor coil.
    Accumulating,
    /// Defrost cycle active (reverse-cycle or resistive).
    Defrosting,
}

impl DefrostCycleState {
    /// Numeric code for telemetry: 0 = Accumulating, 1 = Defrosting.
    #[must_use]
    pub fn code(self) -> f64 {
        match self {
            Self::Accumulating => 0.0,
            Self::Defrosting => 1.0,
        }
    }
}

impl std::fmt::Display for DefrostCycleState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Accumulating => write!(f, "Accumulating"),
            Self::Defrosting => write!(f, "Defrosting"),
        }
    }
}

/// Tracks discrete defrost cycle state for a heat pump.
///
/// The inter-defrost interval is derived from `cycle_duration_s / time_fraction`:
/// at a given `time_fraction`, this produces the same average time-in-defrost as the
/// continuous model but with distinct ON/OFF phases. Source: mathematically equivalent
/// to the EnergyPlus ERM 26.1 — Coils: Single-Speed Electric DX Air Heating Coil — Defrost Operation continuous model when averaged over full cycles; the
/// formula `interval = duration / dtf` is HARES-specific (not from E+ source).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefrostCycleTracker {
    /// Current cycle state.
    pub state: DefrostCycleState,
    /// Accumulated frost proxy: incremented by `dt * time_fraction` each step when
    /// the compressor is running and conditions favor frost. Reset on Defrosting entry.
    pub accumulated_frost_s: f64,
    /// Elapsed time in the current defrost cycle [s]. Reset on Accumulating entry.
    pub defrost_elapsed_s: f64,
    /// Target defrost cycle duration [s] (default 210 s = 3.5 min).
    pub cycle_duration_s: f64,
    /// Hard cap on defrost cycle duration [s] (default 600 s = 10 min).
    pub max_defrost_duration_s: f64,
    /// EWMA of defrost `time_fraction` used for inter-defrost interval calculation.
    /// Smooths step-to-step fluctuations when OAT oscillates around the defrost
    /// threshold. Reset to 0.0 when transitioning from Defrosting to Accumulating.
    #[serde(default)]
    pub ewma_time_fraction: f64,
}

impl DefrostCycleTracker {
    /// Construct a tracker starting in the Accumulating state with defaults.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: DefrostCycleState::Accumulating,
            accumulated_frost_s: 0.0,
            defrost_elapsed_s: 0.0,
            cycle_duration_s: DEFAULT_DEFROST_CYCLE_DURATION_S,
            max_defrost_duration_s: MAX_DEFROST_CYCLE_DURATION_S,
            ewma_time_fraction: 0.0,
        }
    }

    /// Advance the FSM by one timestep.
    ///
    /// - `dt_s` — timestep duration [s]
    /// - `time_fraction` — continuous defrost time fraction from `evaluate_defrost`
    ///   (only meaningful when `conditions_favor_frost` is true)
    /// - `conditions_favor_frost` — true when OAT < max_oat_defrost_c and the
    ///   compressor is running (i.e. `DefrostResult.active` from `evaluate_defrost`)
    /// - `outdoor_db_c` — outdoor dry-bulb temperature [°C]; used for frost
    ///   decay when conditions do not favor frost (preventing spurious defrost
    ///   after seasonal hiatus)
    pub fn advance(
        &mut self,
        dt_s: f64,
        time_fraction: f64,
        conditions_favor_frost: bool,
        outdoor_db_c: f64,
    ) {
        #[cfg_attr(
            not(any(debug_assertions, feature = "check_invariants", feature = "observe")),
            allow(unused_variables)
        )]
        let frost_before = self.accumulated_frost_s;
        let old_state = self.state;
        match self.state {
            DefrostCycleState::Accumulating => {
                if conditions_favor_frost && time_fraction > 0.0 {
                    // Continuous-time EWMA of time_fraction with τ = 10 min.
                    // Smooths step-to-step fluctuations when OAT oscillates
                    // around the defrost threshold, preventing the inter-defrost
                    // interval from varying wildly between steps.
                    // HARES-specific — EnergyPlus uses instantaneous conditions
                    // in its continuous defrost model (ERM 26.1).
                    let alpha = 1.0 - (-dt_s / DEFROST_EWMA_TAU_S).exp();
                    self.ewma_time_fraction =
                        alpha * time_fraction + (1.0 - alpha) * self.ewma_time_fraction;

                    self.accumulated_frost_s += dt_s * time_fraction;
                    let interval_s =
                        self.cycle_duration_s / self.ewma_time_fraction.max(f64::EPSILON);
                    if self.accumulated_frost_s >= interval_s {
                        self.state = DefrostCycleState::Defrosting;
                        self.defrost_elapsed_s = 0.0;
                    }
                } else if !conditions_favor_frost && outdoor_db_c > FROST_DECAY_CLEAR_TEMP_C {
                    // Exponential frost decay during extended off-periods.
                    // Time constant τ decreases as OAT rises: slower decay near
                    // freezing, faster decay in warm weather. This prevents the
                    // accumulator from holding stale frost values across seasonal
                    // hiatuses that would trigger spurious defrost on the first
                    // heating call.
                    let fraction = (outdoor_db_c / 10.0).clamp(0.0, 1.0);
                    let tau_s = FROST_DECAY_TAU_AT_0C_S
                        - fraction * (FROST_DECAY_TAU_AT_0C_S - FROST_DECAY_TAU_AT_10C_S);
                    self.accumulated_frost_s *= (-dt_s / tau_s).exp();
                }
            }
            DefrostCycleState::Defrosting => {
                self.defrost_elapsed_s += dt_s;
                if self.defrost_elapsed_s >= self.cycle_duration_s.min(self.max_defrost_duration_s)
                {
                    self.state = DefrostCycleState::Accumulating;
                    self.accumulated_frost_s = 0.0;
                    self.ewma_time_fraction = 0.0;
                    self.defrost_elapsed_s = 0.0;
                }
            }
        }

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            debug_assert!(
                self.accumulated_frost_s >= 0.0,
                "accumulated_frost_s {:.6} must be non-negative",
                self.accumulated_frost_s
            );
            debug_assert!(
                (0.0..=1.0).contains(&self.ewma_time_fraction),
                "ewma_time_fraction {:.6} must be in [0.0, 1.0]",
                self.ewma_time_fraction
            );
            // When OAT exceeds the defrost-enable temperature, accumulated frost
            // must not have increased during this advance call (it should hold
            // steady or decay). This guards against stale frost accumulation during
            // warm off-periods.
            if outdoor_db_c >= DEFROST_ENABLE_TEMP_C && !conditions_favor_frost {
                debug_assert!(
                    frost_before >= self.accumulated_frost_s - 1e-12,
                    "accumulated_frost_s must not increase when OAT ({:.2}°C) >= defrost-enable \
                     threshold ({:.2}°C) and conditions do not favor frost",
                    outdoor_db_c,
                    DEFROST_ENABLE_TEMP_C
                );
            }
        }

        #[cfg(feature = "observe")]
        {
            if (frost_before - self.accumulated_frost_s).abs() > 1e-12 || conditions_favor_frost {
                tracing::debug!(
                    frost_before,
                    frost_after = self.accumulated_frost_s,
                    outdoor_db_c,
                    conditions_favor_frost,
                    time_fraction,
                    ewma_time_fraction = self.ewma_time_fraction,
                    "defrost frost accumulation change",
                );
            }
        }

        if old_state != self.state {
            debug!(
                old = %old_state,
                new = %self.state,
                frost_s = self.accumulated_frost_s,
                "defrost FSM state transition"
            );
        }
    }

    /// Whether the compressor should suppress heating output this step.
    /// During Defrosting, zone capacity is zero (ReverseCycle) or resistive-only.
    #[must_use]
    pub fn is_defrosting(&self) -> bool {
        self.state == DefrostCycleState::Defrosting
    }
}

impl Default for DefrostCycleTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DefrostResult {
    pub active: bool,
    pub time_fraction: f64,
    pub capacity_multiplier: f64,
    pub power_multiplier: f64,
    pub q_defrost_w: f64,
    pub extra_power_w: f64,
}

impl DefrostResult {
    fn inactive() -> Self {
        Self {
            active: false,
            capacity_multiplier: 1.0,
            power_multiplier: 1.0,
            ..Self::default()
        }
    }
}

/// Evaluate defrost adjustments for the current timestep.
///
/// # Parameters
/// - `config` -- defrost configuration (control mode, strategy, etc.)
/// - `outdoor_db_c` -- outdoor dry-bulb temperature [°C]
/// - `outdoor_humidity_ratio` -- outdoor humidity ratio [kg/kg]
/// - `pressure_pa` -- outdoor air pressure [Pa]
/// - `inlet_wb_c` -- indoor inlet wet-bulb temperature [°C]; used for the
///   optional biquadratic EIR curve in Timed + ReverseCycle mode
/// - `rated_capacity_w` -- heat pump rated (maximum) heating capacity [W]
/// - `current_capacity_w` -- current operating heating capacity [W]
/// - `runtime_fraction` -- compressor runtime fraction [0..1]; scales timed
///   defrost power proportionally with compressor operation
#[must_use]
// Why: the 8 parameters are all distinct physical inputs to a stateless
// defrost model (config, four weather/zone scalars, two capacity scalars, runtime
// fraction). Merging them into an intermediate struct would require a throw-away
// type used in exactly one call site and obscure the function's dependencies.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_defrost(
    config: &DefrostConfig,
    outdoor_db_c: f64,
    outdoor_humidity_ratio: f64,
    pressure_pa: f64,
    inlet_wb_c: f64,
    rated_capacity_w: f64,
    current_capacity_w: f64,
    runtime_fraction: f64,
) -> DefrostResult {
    if outdoor_db_c >= config.max_oat_defrost_c
        || current_capacity_w <= 0.0
        || rated_capacity_w <= 0.0
    {
        return DefrostResult::inactive();
    }

    // Shared coil temperature and delta-humidity computation used by both modes.
    let coil_out_temp_c = DEFROST_COIL_TEMP_SLOPE * outdoor_db_c + DEFROST_COIL_TEMP_OFFSET_C;
    let omega_sat_coil = humidity_ratio_from_twb(coil_out_temp_c, coil_out_temp_c, pressure_pa);
    let delta_omega =
        (outdoor_humidity_ratio - omega_sat_coil).max(DEFROST_MIN_DELTA_HUMIDITY_RATIO);

    match config.control {
        DefrostControl::OnDemand => {
            let time_fraction =
                (1.0 / (1.0 + DEFROST_TIME_FRACTION_NUMERATOR / delta_omega)).clamp(0.0, 1.0);
            let capacity_multiplier =
                (DEFROST_CAPACITY_MULTIPLIER_BASE * (1.0 - time_fraction)).clamp(0.0, 1.0);
            // power_multiplier is a fixed ratio (≈1.09) intentionally > 1.0 in the
            // OCHRE-derived OnDemand model; it is not subject to the negative-value
            // defect and must not be clamped.
            let power_multiplier =
                DEFROST_POWER_MULTIPLIER_NUMERATOR / DEFROST_CAPACITY_MULTIPLIER_BASE;

            let q_defrost_w = DEFROST_Q_MULTIPLIER
                * time_fraction
                * (DEFROST_REFERENCE_TEMP_C - outdoor_db_c)
                * (rated_capacity_w / DEFROST_CAPACITY_UNIT_FACTOR);

            // Use post-defrost capacity for extra power calculation
            // (OCHRE HVAC.py L1162: power_defrost uses capacity after defrost reduction).
            let post_defrost_cap_w =
                (current_capacity_w * capacity_multiplier - q_defrost_w).max(0.0);
            let extra_power_w = DEFROST_EIR_TEMP_MODIFIER
                * (post_defrost_cap_w / DEFROST_CAPACITY_UNIT_FACTOR)
                * time_fraction
                + config.defrost_power_w;

            DefrostResult {
                active: true,
                time_fraction,
                capacity_multiplier,
                power_multiplier,
                q_defrost_w,
                extra_power_w,
            }
        }

        DefrostControl::Disabled => DefrostResult::inactive(),

        DefrostControl::Timed => {
            let time_fraction = config.defrost_time_fraction;
            if time_fraction <= 0.0 {
                return DefrostResult::inactive();
            }

            // Timed mode multipliers from EnergyPlus / DOE-2.
            // Clamped to [0.0, 1.0]: high delta_omega can drive the linear equations
            // below zero, which is physically impossible (negative capacity / power).
            let capacity_multiplier = (TIMED_DEFROST_CAP_MULT_BASE
                - TIMED_DEFROST_CAP_MULT_SLOPE * delta_omega)
                .clamp(0.0, 1.0);
            let power_multiplier = (TIMED_DEFROST_PWR_MULT_BASE
                - TIMED_DEFROST_PWR_MULT_SLOPE * delta_omega)
                .clamp(0.0, 1.0);

            let (q_defrost_w, extra_power_w) = match config.strategy {
                DefrostStrategy::ReverseCycle => {
                    let q = 0.01
                        * time_fraction
                        * (DEFROST_REFERENCE_TEMP_C - outdoor_db_c)
                        * (rated_capacity_w / DEFROST_CAPACITY_UNIT_FACTOR);

                    let defrost_eir = match config.defrost_eir_coeffs {
                        Some(c) => {
                            let wb = inlet_wb_c.max(DEFROST_EIR_CURVE_TEMP_MIN_C);
                            let db = outdoor_db_c.max(DEFROST_EIR_CURVE_TEMP_MIN_C);
                            c[0] + c[1] * wb
                                + c[2] * wb * wb
                                + c[3] * db
                                + c[4] * wb * db
                                + c[5] * db * db
                        }
                        None => 1.0,
                    };

                    let power = defrost_eir
                        * (rated_capacity_w / DEFROST_CAPACITY_UNIT_FACTOR)
                        * time_fraction
                        * runtime_fraction
                        + config.defrost_power_w * time_fraction;

                    (q, power)
                }
                DefrostStrategy::Resistive => {
                    let power =
                        config.resistive_defrost_capacity_w * time_fraction * runtime_fraction
                            + config.defrost_power_w * time_fraction;
                    (0.0, power)
                }
            };

            DefrostResult {
                active: true,
                time_fraction,
                capacity_multiplier,
                power_multiplier,
                q_defrost_w,
                extra_power_w,
            }
        }
    }
}

#[cfg(test)]
mod defrost_tests {
    use super::*;

    const P_PA: f64 = 101_325.0;

    fn on_demand_config() -> DefrostConfig {
        DefrostConfig::on_demand(1.0, 0.0)
    }

    fn timed_config() -> DefrostConfig {
        DefrostConfig {
            capacity_reduction_factor: 1.0,
            defrost_power_w: 0.0,
            control: DefrostControl::Timed,
            strategy: DefrostStrategy::ReverseCycle,
            defrost_time_fraction: DEFAULT_DEFROST_TIME_FRACTION,
            max_oat_defrost_c: DEFROST_ENABLE_TEMP_C,
            defrost_eir_coeffs: None,
            resistive_defrost_capacity_w: 0.0,
        }
    }

    /// Test 1: OnDemand mode is unchanged from current HARES implementation.
    #[test]
    fn on_demand_regression() {
        let cfg = on_demand_config();
        // Conditions that previously produced an active defrost result.
        let result = evaluate_defrost(&cfg, 0.0, 0.005, P_PA, 10.0, 8_000.0, 6_000.0, 1.0);
        assert!(result.active, "OnDemand must be active below threshold");
        assert!(
            result.time_fraction > 0.0 && result.time_fraction < 1.0,
            "time_fraction must be in (0, 1)"
        );
        assert!(
            result.capacity_multiplier > 0.0 && result.capacity_multiplier < 1.0,
            "capacity_multiplier must be in (0, 1)"
        );
        assert!(result.q_defrost_w >= 0.0, "q_defrost must be non-negative");
        assert!(
            result.extra_power_w >= 0.0,
            "extra_power must be non-negative"
        );
        // power_multiplier is constant in OnDemand mode.
        let expected_pwr = DEFROST_POWER_MULTIPLIER_NUMERATOR / DEFROST_CAPACITY_MULTIPLIER_BASE;
        assert!(
            (result.power_multiplier - expected_pwr).abs() < 1e-12,
            "power_multiplier {:.6} != expected {expected_pwr:.6}",
            result.power_multiplier
        );
    }

    /// Test 2: Timed mode produces correct capacity multiplier.
    #[test]
    fn timed_capacity_multiplier_correct() {
        let cfg = timed_config();
        // At OAT = 0°C with a specific humidity ratio, verify the multiplier formula.
        let outdoor_hr = 0.005_f64;
        let result = evaluate_defrost(&cfg, 0.0, outdoor_hr, P_PA, 10.0, 8_000.0, 6_000.0, 1.0);
        assert!(result.active);

        // Recompute expected value.
        let coil_t = DEFROST_COIL_TEMP_SLOPE * 0.0 + DEFROST_COIL_TEMP_OFFSET_C;
        let omega_sat = humidity_ratio_from_twb(coil_t, coil_t, P_PA);
        let dw = (outdoor_hr - omega_sat).max(DEFROST_MIN_DELTA_HUMIDITY_RATIO);
        let expected_cap = TIMED_DEFROST_CAP_MULT_BASE - TIMED_DEFROST_CAP_MULT_SLOPE * dw;
        assert!(
            (result.capacity_multiplier - expected_cap).abs() < 1e-12,
            "capacity_multiplier {:.8} != expected {expected_cap:.8}",
            result.capacity_multiplier
        );
    }

    /// Test 3: Timed mode produces correct power multiplier.
    #[test]
    fn timed_power_multiplier_correct() {
        let cfg = timed_config();
        let outdoor_hr = 0.005_f64;
        let result = evaluate_defrost(&cfg, 0.0, outdoor_hr, P_PA, 10.0, 8_000.0, 6_000.0, 1.0);
        assert!(result.active);

        let coil_t = DEFROST_COIL_TEMP_SLOPE * 0.0 + DEFROST_COIL_TEMP_OFFSET_C;
        let omega_sat = humidity_ratio_from_twb(coil_t, coil_t, P_PA);
        let dw = (outdoor_hr - omega_sat).max(DEFROST_MIN_DELTA_HUMIDITY_RATIO);
        let expected_pwr = TIMED_DEFROST_PWR_MULT_BASE - TIMED_DEFROST_PWR_MULT_SLOPE * dw;
        assert!(
            (result.power_multiplier - expected_pwr).abs() < 1e-12,
            "power_multiplier {:.8} != expected {expected_pwr:.8}",
            result.power_multiplier
        );
    }

    /// Test 4: Timed and OnDemand yield different results for the same conditions.
    #[test]
    fn timed_vs_on_demand_differ() {
        let od = on_demand_config();
        let td = timed_config();
        let r_od = evaluate_defrost(&od, 0.0, 0.005, P_PA, 10.0, 8_000.0, 6_000.0, 1.0);
        let r_td = evaluate_defrost(&td, 0.0, 0.005, P_PA, 10.0, 8_000.0, 6_000.0, 1.0);
        // The formulas differ -- at minimum the capacity multiplier must differ.
        assert!(
            (r_od.capacity_multiplier - r_td.capacity_multiplier).abs() > 1e-6,
            "OnDemand and Timed capacity multipliers should differ"
        );
    }

    /// Test 5: Above max_oat_defrost, neither mode activates.
    #[test]
    fn no_defrost_above_max_oat() {
        for cfg in [on_demand_config(), timed_config()] {
            let result = evaluate_defrost(
                &cfg,
                cfg.max_oat_defrost_c + 1.0,
                0.005,
                P_PA,
                10.0,
                8_000.0,
                6_000.0,
                1.0,
            );
            assert!(
                !result.active,
                "{:?} must be inactive above max_oat_defrost_c",
                cfg.control
            );
            assert_eq!(result.capacity_multiplier, 1.0);
            assert_eq!(result.power_multiplier, 1.0);
        }
    }

    /// Test 6: ReverseCycle strategy -- q_defrost and extra_power are positive.
    #[test]
    fn reverse_cycle_q_defrost_and_power() {
        let cfg = timed_config(); // ReverseCycle by default
        let result = evaluate_defrost(&cfg, -5.0, 0.005, P_PA, 10.0, 8_000.0, 6_000.0, 1.0);
        assert!(result.active);
        assert!(
            result.q_defrost_w > 0.0,
            "ReverseCycle must produce positive q_defrost"
        );
        assert!(
            result.extra_power_w > 0.0,
            "ReverseCycle must produce positive extra_power"
        );
    }

    /// Test 7: Resistive strategy -- q_defrost = 0, power from heater capacity.
    #[test]
    fn resistive_strategy_no_reverse_cycle_load() {
        let cfg = DefrostConfig {
            strategy: DefrostStrategy::Resistive,
            resistive_defrost_capacity_w: 1_000.0,
            ..timed_config()
        };
        let result = evaluate_defrost(&cfg, -5.0, 0.005, P_PA, 10.0, 8_000.0, 6_000.0, 1.0);
        assert!(result.active);
        assert_eq!(
            result.q_defrost_w, 0.0,
            "Resistive must have zero q_defrost"
        );
        let expected_power = 1_000.0 * DEFAULT_DEFROST_TIME_FRACTION * 1.0; // rtf=1.0, defrost_power_w=0
        assert!(
            (result.extra_power_w - expected_power).abs() < 1e-9,
            "Resistive extra_power {:.6} != expected {expected_power:.6}",
            result.extra_power_w
        );
    }

    /// Test 8: Defrost EIR curve is evaluated with 15.555°C floor on inputs.
    #[test]
    fn defrost_eir_curve_with_floor_clipping() {
        // Constant-1.0 curve: c0=1, others=0 → EIR always 1.0 regardless of temp
        let constant_one = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let cfg_flat = DefrostConfig {
            defrost_eir_coeffs: Some(constant_one),
            ..timed_config()
        };
        // A non-trivial curve: purely wb-linear, c1=1 → EIR = wb (floored at 15.555)
        let linear_wb = [0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
        let cfg_linear = DefrostConfig {
            defrost_eir_coeffs: Some(linear_wb),
            ..timed_config()
        };

        // With inlet_wb = 5°C < 15.555 → clamped to 15.555.
        let r_flat = evaluate_defrost(&cfg_flat, -5.0, 0.005, P_PA, 5.0, 8_000.0, 6_000.0, 1.0);
        let r_linear = evaluate_defrost(&cfg_linear, -5.0, 0.005, P_PA, 5.0, 8_000.0, 6_000.0, 1.0);

        assert!(r_flat.active && r_linear.active);
        // linear curve EIR = floored wb = 15.555; flat EIR = 1.0
        // power_linear / power_flat should be ≈ 15.555
        let ratio = r_linear.extra_power_w / r_flat.extra_power_w;
        assert!(
            (ratio - DEFROST_EIR_CURVE_TEMP_MIN_C).abs() < 1e-6,
            "EIR curve ratio {ratio:.6} != expected {DEFROST_EIR_CURVE_TEMP_MIN_C}"
        );
    }

    /// Test 9: Runtime fraction scales defrost power proportionally.
    #[test]
    fn runtime_fraction_scales_defrost_power() {
        let cfg = timed_config();
        let r_full = evaluate_defrost(&cfg, -5.0, 0.005, P_PA, 10.0, 8_000.0, 6_000.0, 1.0);
        let r_half = evaluate_defrost(&cfg, -5.0, 0.005, P_PA, 10.0, 8_000.0, 6_000.0, 0.5);
        assert!(r_full.active && r_half.active);
        let ratio = r_half.extra_power_w / r_full.extra_power_w;
        assert!(
            (ratio - 0.5).abs() < 1e-9,
            "defrost power ratio {ratio:.6} != 0.5 for half runtime fraction"
        );
    }

    /// Test 10: time_fraction = 0 in Timed mode → inactive result.
    #[test]
    fn timed_zero_time_fraction_is_inactive() {
        let cfg = DefrostConfig {
            defrost_time_fraction: 0.0,
            ..timed_config()
        };
        let result = evaluate_defrost(&cfg, -5.0, 0.005, P_PA, 10.0, 8_000.0, 6_000.0, 1.0);
        assert!(!result.active, "time_fraction=0 must yield inactive result");
    }

    /// Test 11: Very cold OAT (-20°C) -- both modes produce finite, reasonable values.
    #[test]
    fn very_cold_oat_produces_finite_values() {
        for cfg in [on_demand_config(), timed_config()] {
            let result = evaluate_defrost(&cfg, -20.0, 0.0005, P_PA, 10.0, 10_000.0, 8_000.0, 1.0);
            assert!(result.active, "{:?} must be active at -20°C", cfg.control);
            assert!(
                result.capacity_multiplier.is_finite(),
                "capacity_multiplier must be finite at -20°C"
            );
            assert!(
                result.power_multiplier.is_finite(),
                "power_multiplier must be finite at -20°C"
            );
            assert!(
                result.q_defrost_w.is_finite() && result.q_defrost_w >= 0.0,
                "q_defrost_w must be finite and non-negative at -20°C"
            );
            assert!(
                result.extra_power_w.is_finite() && result.extra_power_w >= 0.0,
                "extra_power_w must be finite and non-negative at -20°C"
            );
        }
    }

    /// Test 12: Timed multipliers match EnergyPlus reference values for known conditions.
    ///
    /// At OAT = 0°C, outdoor_hr = 0.003 kg/kg, the coil outlet temp is
    /// `0.82 * 0.0 - 8.589 = -8.589°C`. The saturation HR at -8.589°C is very small
    /// (sub-zero), so delta_omega ≈ 0.003. We verify the formula output is physically
    /// plausible: cap_mult < 1.0, pwr_mult < 1.0.
    #[test]
    fn timed_multipliers_plausible_energyplus_reference() {
        let cfg = timed_config();
        // Use a moderate humidity ratio (0.003) at OAT=0°C.
        let result = evaluate_defrost(&cfg, 0.0, 0.003, P_PA, 10.0, 10_000.0, 8_000.0, 1.0);
        assert!(result.active);
        // EnergyPlus reference: at typical winter conditions, cap_mult < 1.0 and
        // pwr_mult < 1.0 indicate reduced capacity / power during defrost.
        assert!(
            result.capacity_multiplier < 1.0,
            "cap_mult {:.4} must be < 1.0 in defrost",
            result.capacity_multiplier
        );
        assert!(
            result.capacity_multiplier > 0.0,
            "cap_mult must be positive"
        );
        assert!(
            result.power_multiplier < 1.0,
            "pwr_mult {:.4} must be < 1.0 in defrost",
            result.power_multiplier
        );
        assert!(result.power_multiplier > 0.0, "pwr_mult must be positive");
    }

    /// Test 13: Timed multipliers are clamped to [0.0, 1.0] when delta_omega exceeds
    /// the threshold that would drive the linear equations negative.
    ///
    /// TIMED_DEFROST_CAP_MULT_SLOPE = 107.33, so cap_mult goes negative above
    /// delta_omega ≈ 0.909 / 107.33 ≈ 0.00847 kg/kg.
    /// TIMED_DEFROST_PWR_MULT_SLOPE = 36.45, so pwr_mult goes negative above
    /// delta_omega ≈ 0.90 / 36.45 ≈ 0.0247 kg/kg.
    #[test]
    fn timed_multipliers_clamped_at_high_humidity() {
        let cfg = timed_config();
        // delta_omega ≈ 0.01 > 0.00847 -- drives cap_mult negative without the clamp.
        let result = evaluate_defrost(&cfg, 0.0, 0.011, P_PA, 10.0, 10_000.0, 8_000.0, 1.0);
        assert!(result.active);
        assert!(
            result.capacity_multiplier >= 0.0,
            "capacity_multiplier must be >= 0.0, got {:.6}",
            result.capacity_multiplier
        );
        assert!(
            result.capacity_multiplier <= 1.0,
            "capacity_multiplier must be <= 1.0, got {:.6}",
            result.capacity_multiplier
        );
        assert!(
            result.power_multiplier >= 0.0,
            "power_multiplier must be >= 0.0, got {:.6}",
            result.power_multiplier
        );
        assert!(
            result.power_multiplier <= 1.0,
            "power_multiplier must be <= 1.0, got {:.6}",
            result.power_multiplier
        );
    }

    /// Test 14: Both timed multipliers clamp to exactly 0.0 at extreme humidity,
    /// not to a negative value.
    #[test]
    fn timed_multipliers_clamped_at_extreme_humidity() {
        let cfg = timed_config();
        // delta_omega ≈ 0.05 -- well above both thresholds.
        // Without the clamp: cap_mult = 0.909 - 107.33*0.05 ≈ -4.46
        //                    pwr_mult = 0.90  -  36.45*0.05 ≈ -0.92
        let result = evaluate_defrost(&cfg, 0.0, 0.051, P_PA, 10.0, 10_000.0, 8_000.0, 1.0);
        assert!(result.active);
        assert_eq!(
            result.capacity_multiplier, 0.0,
            "capacity_multiplier must be exactly 0.0 at extreme humidity"
        );
        assert_eq!(
            result.power_multiplier, 0.0,
            "power_multiplier must be exactly 0.0 at extreme humidity"
        );
    }

    /// Test 15: OnDemand capacity multiplier stays in [0.0, 1.0] and power
    /// multiplier stays non-negative even at extreme humidity that drives
    /// time_fraction toward 1.0.
    ///
    /// As delta_omega → ∞, time_fraction → 1.0 and capacity_multiplier → 0.0.
    /// The capacity clamp guards against floating-point edge cases going below 0.
    ///
    /// Note: OnDemand power_multiplier is a fixed ratio (≈1.09) derived from the
    /// OCHRE/EnergyPlus formula. It is intentionally > 1.0 and is not subject to
    /// the negative-value defect fixed in Timed mode.
    #[test]
    fn on_demand_multipliers_clamped_when_time_fraction_approaches_one() {
        let cfg = on_demand_config();
        // Very high humidity ratio forces time_fraction very close to 1.0.
        let result = evaluate_defrost(&cfg, 0.0, 0.5, P_PA, 10.0, 10_000.0, 8_000.0, 1.0);
        assert!(result.active);
        assert!(
            result.capacity_multiplier >= 0.0,
            "OnDemand capacity_multiplier must be >= 0.0, got {:.6}",
            result.capacity_multiplier
        );
        assert!(
            result.capacity_multiplier <= 1.0,
            "OnDemand capacity_multiplier must be <= 1.0, got {:.6}",
            result.capacity_multiplier
        );
        assert!(
            result.power_multiplier >= 0.0,
            "OnDemand power_multiplier must be non-negative, got {:.6}",
            result.power_multiplier
        );
        // time_fraction must be strictly in (0, 1).
        assert!(
            result.time_fraction > 0.0 && result.time_fraction < 1.0,
            "time_fraction {:.8} must be in (0, 1)",
            result.time_fraction
        );
    }

    // ── DefrostCycleTracker unit tests ───────────────────────────────────────

    #[test]
    fn tracker_starts_in_accumulating() {
        let tracker = DefrostCycleTracker::new();
        assert_eq!(tracker.state, DefrostCycleState::Accumulating);
        assert!(!tracker.is_defrosting());
        assert_eq!(tracker.accumulated_frost_s, 0.0);
        assert_eq!(tracker.defrost_elapsed_s, 0.0);
    }

    #[test]
    fn tracker_accumulates_frost_when_conditions_favor_frost() {
        let mut tracker = DefrostCycleTracker::new();
        let time_fraction = 0.2;
        tracker.advance(60.0, time_fraction, true, 0.0);
        assert_eq!(tracker.state, DefrostCycleState::Accumulating);
        let expected = 60.0 * 0.2;
        assert!(
            (tracker.accumulated_frost_s - expected).abs() < 1e-9,
            "accumulated_frost_s {} != expected {expected}",
            tracker.accumulated_frost_s,
        );
    }

    #[test]
    fn tracker_does_not_accumulate_when_no_frost() {
        let mut tracker = DefrostCycleTracker::new();
        tracker.advance(60.0, 0.0, false, 0.0);
        assert_eq!(tracker.state, DefrostCycleState::Accumulating);
        assert_eq!(tracker.accumulated_frost_s, 0.0);
    }

    #[test]
    fn tracker_transitions_to_defrosting_after_interval() {
        let mut tracker = DefrostCycleTracker::new();
        tracker.cycle_duration_s = 210.0;
        let time_fraction: f64 = 0.2;
        // interval = cycle_duration / time_fraction = 210 / 0.2 = 1050 s
        // Each step accumulates dt * time_fraction = 60 * 0.2 = 12 s of frost.
        let interval_s: f64 = 210.0 / 0.2;
        let dt: f64 = 60.0;
        let frost_per_step = dt * time_fraction;

        // Simulate steps until just before threshold
        let steps_before: usize = (interval_s / frost_per_step).floor() as usize;
        for _ in 0..steps_before {
            tracker.advance(dt, time_fraction, true, 0.0);
        }
        assert_eq!(
            tracker.state,
            DefrostCycleState::Accumulating,
            "must still be accumulating before threshold"
        );

        // One more step crosses the threshold
        tracker.advance(dt, time_fraction, true, 0.0);
        assert_eq!(
            tracker.state,
            DefrostCycleState::Defrosting,
            "must transition to Defrosting after accumulated frost >= interval"
        );
        assert!(tracker.is_defrosting());
        assert_eq!(tracker.defrost_elapsed_s, 0.0, "elapsed resets on entry");
    }

    #[test]
    fn tracker_returns_to_accumulating_after_cycle_duration() {
        let mut tracker = DefrostCycleTracker::new();
        tracker.cycle_duration_s = 210.0;
        tracker.max_defrost_duration_s = 600.0;
        tracker.state = DefrostCycleState::Defrosting;
        tracker.defrost_elapsed_s = 0.0;
        tracker.accumulated_frost_s = 0.0;

        let dt = 60.0;
        // 3 steps = 180s, not yet 210s
        for _ in 0..3 {
            tracker.advance(dt, 1.0, true, 0.0);
        }
        assert_eq!(tracker.state, DefrostCycleState::Defrosting);

        // 4th step = 240s > 210s → back to Accumulating
        tracker.advance(dt, 1.0, true, 0.0);
        assert_eq!(
            tracker.state,
            DefrostCycleState::Accumulating,
            "must return to Accumulating after cycle_duration_s"
        );
        assert_eq!(tracker.accumulated_frost_s, 0.0, "frost resets on exit");
        assert_eq!(tracker.defrost_elapsed_s, 0.0, "elapsed resets on exit");
    }

    #[test]
    fn tracker_max_duration_caps_defrost_cycle() {
        let mut tracker = DefrostCycleTracker::new();
        tracker.cycle_duration_s = 210.0;
        tracker.max_defrost_duration_s = 180.0; // lower than cycle_duration
        tracker.state = DefrostCycleState::Defrosting;
        tracker.defrost_elapsed_s = 0.0;

        // 3 steps = 180s = max_duration
        for _ in 0..2 {
            tracker.advance(60.0, 1.0, true, 0.0);
        }
        assert_eq!(tracker.state, DefrostCycleState::Defrosting);
        tracker.advance(60.0, 1.0, true, 0.0);
        assert_eq!(
            tracker.state,
            DefrostCycleState::Accumulating,
            "max_defrost_duration_s must override cycle_duration_s"
        );
    }

    #[test]
    fn tracker_cycle_state_code() {
        assert_eq!(DefrostCycleState::Accumulating.code(), 0.0);
        assert_eq!(DefrostCycleState::Defrosting.code(), 1.0);
    }

    #[test]
    fn tracker_zero_time_fraction_does_not_accumulate() {
        let mut tracker = DefrostCycleTracker::new();
        tracker.advance(60.0, 0.0, true, 0.0);
        assert_eq!(tracker.accumulated_frost_s, 0.0);
        assert_eq!(tracker.state, DefrostCycleState::Accumulating);
    }

    #[test]
    fn tracker_defrosting_ignores_frost_conditions() {
        // When already in Defrosting, frost conditions should not matter;
        // only elapsed time drives the state back to Accumulating.
        let mut tracker = DefrostCycleTracker::new();
        tracker.cycle_duration_s = 210.0;
        tracker.state = DefrostCycleState::Defrosting;
        tracker.defrost_elapsed_s = 200.0;

        // Advance with conditions_favor_frost = false — should still progress
        tracker.advance(60.0, 0.0, false, 0.0);
        // 260s > 210s → back to Accumulating
        assert_eq!(
            tracker.state,
            DefrostCycleState::Accumulating,
            "Defrosting state must still progress even without frost conditions"
        );
    }

    #[test]
    fn frost_decays_when_no_frost_conditions_and_warm_oat() {
        let mut tracker = DefrostCycleTracker::new();
        tracker.accumulated_frost_s = 10.0;

        // Advance at OAT = 7°C (well above freezing) with conditions_favor_frost = false.
        // At 7°C, tau = 3600 - 0.7*(3600-600) = 3600 - 2100 = 1500 s.
        // After 1500 s, frost = 10.0 * exp(-1) ≈ 10.0 * 0.3679 ≈ 3.679.
        // After 6000 s, frost = 10.0 * exp(-4) ≈ 10.0 * 0.0183 ≈ 0.183.
        for _ in 0..100 {
            tracker.advance(60.0, 0.0, false, 7.0);
        }
        // After 6000 s (~1.67 h), frost should have decayed substantially.
        assert!(
            tracker.accumulated_frost_s < 1.0,
            "frost must decay below 1.0 after 6000 s at 7°C, got {:.6}",
            tracker.accumulated_frost_s
        );
        assert!(
            tracker.accumulated_frost_s > 0.0,
            "exponential decay approaches but never reaches zero"
        );
        assert_eq!(
            tracker.state,
            DefrostCycleState::Accumulating,
            "decay must not trigger defrost"
        );
    }

    #[test]
    fn frost_does_not_decay_when_oat_below_freezing() {
        let mut tracker = DefrostCycleTracker::new();
        tracker.accumulated_frost_s = 10.0;

        // Advance at OAT = -5°C (below decay threshold) with no frost conditions.
        tracker.advance(3600.0, 0.0, false, -5.0);

        assert!(
            (tracker.accumulated_frost_s - 10.0).abs() < 1e-12,
            "frost must not decay when OAT <= 0°C, got {:.6}",
            tracker.accumulated_frost_s
        );
        assert_eq!(tracker.state, DefrostCycleState::Accumulating);
    }

    #[test]
    fn frost_decay_prevents_spurious_defrost_after_hiatus() {
        let mut tracker = DefrostCycleTracker::new();
        tracker.cycle_duration_s = 210.0;
        let time_fraction: f64 = 0.2;
        // interval = 210 / 0.2 = 1050 s

        // Phase 1: heating season — accumulate frost with cold OAT.
        // Each 60s step accumulates 60 * 0.2 = 12 s of frost.
        // After 50 steps (3000 s), frost = 600.0 — not enough to trigger defrost
        // (threshold is 1050 s).
        for _ in 0..50 {
            tracker.advance(60.0, time_fraction, true, -5.0);
        }
        let frost_after_heating = tracker.accumulated_frost_s;
        assert_eq!(tracker.state, DefrostCycleState::Accumulating);
        assert!(
            frost_after_heating > 0.0,
            "frost must accumulate during heating"
        );

        // Phase 2: spring off-period — warm OAT, no heating calls.
        // At OAT = 10°C, tau = 600 s. After 3600 s = 6 * tau,
        // frost = frost_after_heating * exp(-6) ≈ frost_after_heating * 0.0025.
        for _ in 0..60 {
            tracker.advance(60.0, 0.0, false, 10.0);
        }
        assert!(
            tracker.accumulated_frost_s < frost_after_heating * 0.01,
            "frost must decay substantially during spring hiatus, {:.6} >= {:.6}",
            tracker.accumulated_frost_s,
            frost_after_heating * 0.01
        );

        // Phase 3: resume heating — frost must be too small to trigger defrost.
        // The decayed frost value is far below the interval threshold (=1050 s),
        // so the first heating call must stay in Accumulating.
        tracker.advance(60.0, time_fraction, true, -5.0);
        assert_eq!(
            tracker.state,
            DefrostCycleState::Accumulating,
            "first heating call after hiatus must not trigger spurious defrost"
        );
    }

    // ── EWMA time_fraction tests ────────────────────────────────────────────

    /// Feed oscillating `time_fraction` (alternating 0.1 and 0.3) and verify
    /// that `ewma_time_fraction` converges to the mean (0.2).
    #[test]
    fn ewma_converges_to_mean_under_oscillating_time_fraction() {
        let mut tracker = DefrostCycleTracker::new();
        // Use very large cycle duration so defrost never triggers during the test,
        // allowing the EWMA to converge without reset.
        tracker.cycle_duration_s = 1_000_000.0;
        tracker.max_defrost_duration_s = 1_000_000.0;
        let dt = 60.0;
        let mut tfs = [0.1_f64, 0.3_f64].into_iter().cycle();
        for _ in 0..200 {
            let tf = tfs.next().unwrap();
            tracker.advance(dt, tf, true, 0.0);
        }
        let error = (tracker.ewma_time_fraction - 0.2).abs();
        assert!(
            error < 0.01,
            "ewma_time_fraction {:.6} should be within 0.01 of 0.2 after 200 steps",
            tracker.ewma_time_fraction
        );
    }

    /// EWMA starts at 0.0 and builds up gradually; verify it is below the steady
    /// state after just a few steps.
    #[test]
    fn ewma_starts_at_zero_and_ramps_up() {
        let mut tracker = DefrostCycleTracker::new();
        tracker.cycle_duration_s = 1_000_000.0;
        tracker.max_defrost_duration_s = 1_000_000.0;
        let dt = 60.0;
        let tf = 0.3;
        tracker.advance(dt, tf, true, 0.0);
        assert!(
            tracker.ewma_time_fraction > 0.0 && tracker.ewma_time_fraction < tf,
            "ewma {:.6} should be between 0 and {tf} after one step",
            tracker.ewma_time_fraction
        );
        for _ in 0..199 {
            tracker.advance(dt, tf, true, 0.0);
        }
        assert!(
            (tracker.ewma_time_fraction - tf).abs() < 0.01,
            "ewma {:.6} should be near {tf} after 200 steps",
            tracker.ewma_time_fraction
        );
    }

    /// When transitioning from Defrosting to Accumulating, `ewma_time_fraction`
    /// must reset to 0.0.
    #[test]
    fn ewma_resets_on_defrost_to_accumulating_transition() {
        let mut tracker = DefrostCycleTracker::new();
        tracker.cycle_duration_s = 210.0;
        let dt = 60.0;
        let tf = 0.2;

        // Build up EWMA and trigger a defrost cycle.
        for _ in 0..200 {
            tracker.advance(dt, tf, true, 0.0);
        }
        assert!(tracker.ewma_time_fraction > 0.0);

        // Force into Defrosting and then complete the cycle.
        tracker.state = DefrostCycleState::Defrosting;
        tracker.defrost_elapsed_s = 200.0;
        tracker.advance(dt, tf, true, 0.0);
        // Should have transitioned back to Accumulating.
        assert_eq!(tracker.state, DefrostCycleState::Accumulating);
        assert_eq!(
            tracker.ewma_time_fraction, 0.0,
            "ewma_time_fraction must reset to 0.0 after leaving Defrosting"
        );
    }

    /// Verify that under an oscillating `time_fraction`, the discrete FSM's
    /// cycle-trigger timing is determined by the EWMA (which converges to the
    /// long-run average), not by the current step's instantaneous value.
    ///
    /// Pre-warms the EWMA to steady state with a large `cycle_duration_s` to
    /// prevent triggering during warmup, then resets frost and measures steps
    /// to the first `Accumulating → Defrosting` transition under normal
    /// `cycle_duration_s`. With EWMA ≈ 0.2 the expected interval is
    /// ≈ 210 / 0.2 = 1050 s and frost accumulation per step is ≈ 12 s/step,
    /// so ~88 steps. Instantaneous tf would trigger at ~60 (tf=0.3 → interval
    /// 700 s) or ~175 (tf=0.1 → interval 2100 s) — this test catches a
    /// regression where `interval_s` reverts to the instantaneous value.
    #[test]
    fn ewma_driven_interval_determines_cycle_trigger_timing_under_oscillation() {
        let mut tracker = DefrostCycleTracker::new();
        let dt = 60.0;
        let mut tfs = [0.1_f64, 0.3_f64].into_iter().cycle();

        // Pre-warm EWMA with large cycle_duration to prevent triggering.
        tracker.cycle_duration_s = 1_000_000.0;
        tracker.max_defrost_duration_s = 1_000_000.0;
        for _ in 0..200 {
            tracker.advance(dt, tfs.next().unwrap(), true, 0.0);
        }
        assert!(
            (tracker.ewma_time_fraction - 0.2).abs() < 0.02,
            "EWMA {:.6} should have converged to ~0.2 after 200 warmup steps",
            tracker.ewma_time_fraction
        );

        // Reset frost and set normal cycle duration for the timing measurement.
        tracker.accumulated_frost_s = 0.0;
        tracker.cycle_duration_s = 210.0;
        tracker.max_defrost_duration_s = 210.0;

        let mut steps = 0u32;
        for _ in 0..500 {
            let old_state = tracker.state;
            tracker.advance(dt, tfs.next().unwrap(), true, 0.0);
            steps += 1;
            if old_state == DefrostCycleState::Accumulating
                && tracker.state == DefrostCycleState::Defrosting
            {
                // With EWMA ≈ 0.2: interval = 1050 s, frost/step ≈ 12 s → ~88 steps.
                // With instantaneous tf=0.3: interval = 700 s → ~60 steps.
                // With instantaneous tf=0.1: interval = 2100 s → ~175 steps.
                let expected = 210.0 / 0.2 / (dt * 0.2); // ≈ 87.5
                assert!(
                    steps >= 70 && steps <= 110,
                    "first defrost trigger at step {steps}; expected near {expected:.0} (±25 %) \
                     with EWMA (≈ 88), not at the instantaneous-tf extremes (≈ 60 or ≈ 175)"
                );
                return;
            }
        }
        panic!("defrost did not trigger within 500 steps under oscillating time_fraction");
    }

    /// Multi-cycle simulation with OAT repeatedly crossing the defrost-enable
    /// threshold under an oscillating `time_fraction` (alternating 0.15 and
    /// 0.25, mean 0.2). Frost should decay during warm periods and accumulate
    /// during cold periods; the number of defrost cycles should be proportional
    /// to the time spent below the threshold, not spurious from threshold
    /// crossings. The EWMA smoothing ensures the inter-defrost interval is
    /// based on the long-run average time_fraction, not the instantaneous
    /// value that would drift under oscillation.
    #[test]
    fn multi_cycle_defrost_proportional_to_cold_duration() {
        let mut tracker = DefrostCycleTracker::new();
        tracker.cycle_duration_s = 210.0;
        let dt = 60.0;
        let mut tfs = [0.15_f64, 0.25_f64].into_iter().cycle(); // mean = 0.2

        // Phase 1: 2 hours continuous cold (7200 s).
        // Mean frost/step = 60 * 0.2 = 12 s. With EWMA → 0.2, interval ≈
        // 210 / 0.2 = 1050 s → defrost fires after ~88 steps (and resets frost).
        let mut defrost_count = 0u32;
        for _ in 0..120 {
            let tf = tfs.next().unwrap();
            let old_state = tracker.state;
            tracker.advance(dt, tf, true, -5.0);
            if old_state == DefrostCycleState::Accumulating
                && tracker.state == DefrostCycleState::Defrosting
            {
                defrost_count += 1;
            }
        }
        assert!(
            defrost_count >= 1,
            "must trigger defrost during 2h of cold; got {defrost_count}"
        );
        assert!(
            tracker.ewma_time_fraction > 0.0,
            "ewma_time_fraction {:.6} should be non-zero after cold-weather accumulation",
            tracker.ewma_time_fraction
        );

        // Phase 2: 1 hour warm hiatus (3600 s at 10°C) — frost decays.
        // After 3600 s at 10°C (tau = 600 s), decay factor = exp(-6) ≈ 0.0025.
        // The remaining post-defrost frost is negligible after decay.
        for _ in 0..60 {
            tracker.advance(dt, 0.0, false, 10.0);
        }
        assert_eq!(tracker.state, DefrostCycleState::Accumulating);
        assert!(
            tracker.accumulated_frost_s < 10.0,
            "frost must be low after warm hiatus, got {:.3}",
            tracker.accumulated_frost_s
        );

        // Phase 3: 2 more hours cold with oscillating tf — must trigger defrost
        // again (not spurious from the warm→cold transition).
        let before = defrost_count;
        for _ in 0..120 {
            let tf = tfs.next().unwrap();
            let old_state = tracker.state;
            tracker.advance(dt, tf, true, -5.0);
            if old_state == DefrostCycleState::Accumulating
                && tracker.state == DefrostCycleState::Defrosting
            {
                defrost_count += 1;
            }
        }
        assert!(
            defrost_count > before,
            "must trigger defrost again after hiatus; defrost count went from {before} to {defrost_count}"
        );
        assert!(
            tracker.ewma_time_fraction > 0.0,
            "ewma_time_fraction {:.6} should be non-zero after second cold-weather accumulation",
            tracker.ewma_time_fraction
        );
    }
}
