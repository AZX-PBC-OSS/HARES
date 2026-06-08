use std::sync::Arc;

use chrono::{Datelike, Timelike, Weekday};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use rand_distr::Distribution;
use serde::{Deserialize, Serialize};

use crate::{DomainId, EnvironmentState, HaresError};

/// Which days a [`TimeWindow`] applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DayFilter {
    /// Every day of the week.
    Any,
    /// Monday through Friday.
    Weekdays,
    /// Saturday and Sunday.
    Weekends,
    /// A specific day of the week.
    Day(Weekday),
}

impl DayFilter {
    /// Returns `true` if `weekday` is matched by this filter.
    pub fn matches(self, weekday: Weekday) -> bool {
        match self {
            Self::Any => true,
            Self::Weekdays => !matches!(weekday, Weekday::Sat | Weekday::Sun),
            Self::Weekends => matches!(weekday, Weekday::Sat | Weekday::Sun),
            Self::Day(d) => weekday == d,
        }
    }
}

/// A time-of-day window with an associated value.
///
/// Times are expressed as minutes from midnight.
/// `start_minute` is in `0..1440`, `end_minute` is in `0..=1440`.
/// The range is half-open: `[start, end)`.
/// `end_minute = 1440` represents end-of-day (24:00), allowing a full-day
/// window as `(0, 1440)`.
///
/// If `start_minute > end_minute` the window wraps across midnight
/// (e.g. 22:00–06:00 → `start_minute: 1320, end_minute: 360`).
/// For midnight-wrapping windows with a day-specific filter, the post-midnight
/// portion matches the *next* calendar day (e.g. `Day(Mon), 1320, 360` matches
/// Monday 22:00–23:59 and Tuesday 00:00–05:59).
///
/// `start_minute == end_minute` is invalid and will panic in debug builds.
///
/// When used inside [`ScheduleSource::TimeWindows`], windows are evaluated in
/// declaration order and the first match wins. Overlapping windows are allowed
/// -- use ordering to express priority.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimeWindow {
    pub day: DayFilter,
    pub start_minute: u16,
    pub end_minute: u16,
    pub value: f64,
    /// Optional per-window noise. When set, `value` is the center and the
    /// result is sampled from the distribution each call. When `None`,
    /// `value` is returned exactly (current behavior, zero-cost).
    pub noise: Option<DistributionKind>,
    /// Post-noise output floor. Prevents nonsensical values (e.g., negative
    /// miles driven from Gaussian noise on a small mean).
    pub min_value: Option<f64>,
    /// Post-noise output ceiling.
    pub max_value: Option<f64>,
}

impl TimeWindow {
    /// Create a new time window.
    ///
    /// `start_minute` must be in `0..1440`, `end_minute` in `0..=1440`.
    /// `start_minute == end_minute` panics in debug (zero-width window never matches).
    pub fn new(day: DayFilter, start_minute: u16, end_minute: u16, value: f64) -> Self {
        debug_assert!(start_minute < 1440, "start_minute out of range");
        debug_assert!(end_minute <= 1440, "end_minute out of range");
        debug_assert!(
            start_minute != end_minute,
            "zero-width window never matches; use default instead"
        );
        Self {
            day,
            start_minute,
            end_minute,
            value,
            noise: None,
            min_value: None,
            max_value: None,
        }
    }

    /// Create a time window with additive noise and optional clamping.
    ///
    /// `value` becomes the center of the distribution. The `noise` distribution
    /// should typically have `mean: 0.0` (for Gaussian) so that `value` is the
    /// true center; the `mean` field of the distribution acts as an additional
    /// additive offset. The noise is added to produce the final result, then
    /// clamped to `[min_value, max_value]`.
    ///
    /// # Panics
    ///
    /// Panics in debug builds if the distribution parameters are invalid.
    pub fn with_noise(
        day: DayFilter,
        start_minute: u16,
        end_minute: u16,
        value: f64,
        noise: DistributionKind,
        min_value: Option<f64>,
        max_value: Option<f64>,
    ) -> Self {
        debug_assert!(start_minute < 1440, "start_minute out of range");
        debug_assert!(end_minute <= 1440, "end_minute out of range");
        debug_assert!(
            start_minute != end_minute,
            "zero-width window never matches; use default instead"
        );
        debug_assert!(
            noise.validate().is_ok(),
            "invalid distribution params: {:?}",
            noise.validate().unwrap_err()
        );
        Self {
            day,
            start_minute,
            end_minute,
            value,
            noise: Some(noise),
            min_value,
            max_value,
        }
    }

    /// Does this window contain the given day and minute-of-day?
    pub fn contains(&self, weekday: Weekday, minute_of_day: u16) -> bool {
        if self.start_minute < self.end_minute {
            // Normal window: [start, end) on the anchor day
            self.day.matches(weekday)
                && minute_of_day >= self.start_minute
                && minute_of_day < self.end_minute
        } else {
            // Midnight-wrapping window: [start, 1440) on anchor day
            //                           ∪ [0, end) on the following day
            if minute_of_day >= self.start_minute {
                self.day.matches(weekday)
            } else if minute_of_day < self.end_minute {
                self.day.matches(weekday.pred())
            } else {
                false
            }
        }
    }
}

/// Canonical custom-domain id used for schedule payloads in `EnvironmentState.custom_domains`.
pub const SCHEDULE_DOMAIN_ID: DomainId = DomainId(u16::MAX);

/// Out-of-range index behavior for schedule-backed sources.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BoundaryPolicy {
    /// Clamp indices to the first/last valid sample.
    #[default]
    Clamp,
    /// Wrap indices modulo the source length.
    Wrap,
    /// Return an error when the index is out-of-bounds.
    Error,
}

/// Parameters for a specific probability distribution.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DistributionKind {
    /// Gaussian (normal): result = mean + std_dev × N(0,1).
    Gaussian { mean: f64, std_dev: f64 },
    /// Uniform: result ~ U(low, high).
    Uniform { low: f64, high: f64 },
    /// Log-normal: result ~ LogNormal(mu, sigma) where mu/sigma parameterise
    /// the underlying normal distribution.
    LogNormal { mu: f64, sigma: f64 },
    /// Exponential: result ~ Exp(lambda), mean = 1/lambda.
    Exponential { lambda: f64 },
    /// Poisson: result = Poisson(lambda) cast to f64.
    Poisson { lambda: f64 },
    /// Bernoulli: result is 1.0 with probability p, else 0.0.
    Bernoulli { p: f64 },
}

impl DistributionKind {
    /// Validate that the distribution parameters are well-formed.
    pub fn validate(&self) -> Result<(), HaresError> {
        match self {
            Self::Gaussian { mean, std_dev }
                if !mean.is_finite() || !std_dev.is_finite() || *std_dev < 0.0 =>
            {
                Err(HaresError::Equipment(
                    "Gaussian requires finite mean and non-negative finite std_dev".into(),
                ))
            }
            Self::Uniform { low, high } if !low.is_finite() || !high.is_finite() || low >= high => {
                Err(HaresError::Equipment(format!(
                    "Uniform requires finite low < high, got {low}, {high}"
                )))
            }
            Self::LogNormal { mu, sigma }
                if !mu.is_finite() || !sigma.is_finite() || *sigma <= 0.0 =>
            {
                Err(HaresError::Equipment(
                    "LogNormal requires finite mu and positive finite sigma".into(),
                ))
            }
            Self::Exponential { lambda } if !lambda.is_finite() || *lambda <= 0.0 => Err(
                HaresError::Equipment("Exponential requires positive finite lambda".into()),
            ),
            Self::Poisson { lambda } if !lambda.is_finite() || *lambda <= 0.0 => Err(
                HaresError::Equipment("Poisson requires positive finite lambda".into()),
            ),
            Self::Bernoulli { p } if !p.is_finite() || !(0.0..=1.0).contains(p) => Err(
                HaresError::Equipment(format!("Bernoulli requires finite p in [0, 1], got {p}")),
            ),
            _ => Ok(()),
        }
    }

    /// Draw a single sample from this distribution using the given RNG.
    pub fn sample(&self, rng: &mut ChaCha8Rng) -> Result<f64, HaresError> {
        match self {
            Self::Gaussian { mean, std_dev } => {
                let z: f64 = rand_distr::StandardNormal.sample(rng);
                Ok(mean + std_dev * z)
            }
            Self::Uniform { low, high } => rand_distr::Uniform::new(*low, *high)
                .map(|d| d.sample(rng))
                .map_err(|e| HaresError::Equipment(format!("invalid Uniform params: {e}"))),
            Self::LogNormal { mu, sigma } => rand_distr::LogNormal::new(*mu, *sigma)
                .map(|d| d.sample(rng))
                .map_err(|e| HaresError::Equipment(format!("invalid LogNormal params: {e}"))),
            Self::Exponential { lambda } => rand_distr::Exp::new(*lambda)
                .map(|d| d.sample(rng))
                .map_err(|e| HaresError::Equipment(format!("invalid Exponential params: {e}"))),
            Self::Poisson { lambda } => {
                // Poisson samples are already integer-valued f64 (e.g. 3.0, not 3.01).
                rand_distr::Poisson::new(*lambda)
                    .map(|d: rand_distr::Poisson<f64>| d.sample(rng))
                    .map_err(|e| HaresError::Equipment(format!("invalid Poisson params: {e}")))
            }
            Self::Bernoulli { p } => rand_distr::Bernoulli::new(*p)
                .map(|d| if d.sample(rng) { 1.0 } else { 0.0 })
                .map_err(|e| HaresError::Equipment(format!("invalid Bernoulli params: {e}"))),
        }
    }

    /// Analytical mean of the distribution.
    pub fn mean(&self) -> f64 {
        match self {
            Self::Gaussian { mean, .. } => *mean,
            Self::Uniform { low, high } => (low + high) / 2.0,
            Self::LogNormal { mu, sigma } => (*mu + sigma * sigma / 2.0).exp(),
            Self::Exponential { lambda } => 1.0 / lambda,
            Self::Poisson { lambda } => *lambda,
            Self::Bernoulli { p } => *p,
        }
    }
}

/// Shared RNG state for stochastic schedule sources.
#[derive(Clone, Debug)]
pub struct StochasticState {
    pub seed: [u8; 32],
    pub draw_count: u64,
    pub rng: ChaCha8Rng,
}

impl StochasticState {
    pub fn new(seed: [u8; 32]) -> Self {
        Self {
            seed,
            draw_count: 0,
            rng: ChaCha8Rng::from_seed(seed),
        }
    }

    pub fn reset(&mut self) {
        self.rng = ChaCha8Rng::from_seed(self.seed);
        self.draw_count = 0;
    }
}

impl PartialEq for StochasticState {
    fn eq(&self, other: &Self) -> bool {
        self.seed == other.seed && self.draw_count == other.draw_count
    }
}

/// A lazily-evaluated source for schedule values.
#[non_exhaustive]
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum ScheduleSource {
    /// Fixed scalar value.
    Constant(f64),
    /// 24-hour weekday/weekend profile with monthly scaling and maximum value.
    DailyProfile {
        weekday: [f64; 24],
        weekend: [f64; 24],
        month_multipliers: [f64; 12],
        max_value: f64,
    },
    /// Column index in the schedule custom-domain payload.
    ColumnRef {
        col_idx: usize,
        boundary: BoundaryPolicy,
    },
    /// Solar-aware profile scaled by monthly multipliers and maximum value.
    ///
    /// Three-phase day model based on solar altitude:
    /// - **Daytime**: solar altitude > `dusk_altitude_threshold_deg`
    /// - **Evening/dusk**: solar altitude ≤ dusk threshold AND > `dawn_altitude_threshold_deg`
    /// - **Overnight**: solar altitude ≤ `dawn_altitude_threshold_deg`
    ///
    /// Typical thresholds: dusk = 0.0° (geometric sunset), dawn = -6.0° (civil twilight).
    /// Setting both to the same value collapses to a two-phase day/night model.
    SolarAware {
        daytime_fraction: f64,
        evening_fraction: f64,
        overnight_fraction: f64,
        month_multipliers: [f64; 12],
        max_value: f64,
        dusk_altitude_threshold_deg: f64,
        /// Solar altitude threshold below which "evening" becomes "overnight".
        /// Typically a negative value (e.g. -6.0 for civil twilight).
        dawn_altitude_threshold_deg: f64,
    },
    /// Stateful pseudo-random source, deterministic by `seed` + call order.
    Stochastic {
        kind: DistributionKind,
        seed: [u8; 32],
        draw_count: u64,
        rng: ChaCha8Rng,
        clamp_min: Option<f64>,
        clamp_max: Option<f64>,
    },
    /// Shared data with cursor-based advancement.
    Shared {
        data: Arc<[f64]>,
        cursor: usize,
        boundary: BoundaryPolicy,
    },
    /// Window-based lookup by day-of-week and time-of-day.
    ///
    /// Windows are evaluated in order; **the first matching window wins**.
    /// Overlapping windows are permitted -- use ordering to express priority
    /// (e.g. place a day-specific override before a broad weekday catch-all).
    ///
    /// If no window matches, `default` is used. If `default` is `None`
    /// and nothing matches, an error is returned.
    TimeWindows {
        windows: Vec<TimeWindow>,
        default: Option<f64>,
        /// Seeded RNG for windows with noise. `None` when all windows are
        /// deterministic (zero-cost -- no RNG allocated).
        rng_state: Option<Box<StochasticState>>,
    },
}

/// Serde-friendly schedule config representation for typed equipment configs.
///
/// This is the canonical persisted form used in config payloads; runtime code
/// converts it into [`ScheduleSource`] via [`ScheduleSourceConfig::into_runtime`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum ScheduleSourceConfig {
    Constant(f64),
    DailyProfile {
        weekday: [f64; 24],
        weekend: [f64; 24],
        #[serde(default = "default_month_multipliers")]
        month_multipliers: [f64; 12],
        #[serde(default = "default_max_value")]
        max_value: f64,
    },
    ColumnRef {
        col_idx: usize,
        #[serde(default)]
        boundary: BoundaryPolicy,
    },
    SolarAware {
        daytime_fraction: f64,
        evening_fraction: f64,
        overnight_fraction: f64,
        #[serde(default = "default_month_multipliers")]
        month_multipliers: [f64; 12],
        #[serde(default = "default_max_value")]
        max_value: f64,
        dusk_altitude_threshold_deg: f64,
        dawn_altitude_threshold_deg: f64,
    },
    TimeWindows {
        windows: Vec<TimeWindow>,
        default: Option<f64>,
        #[serde(default)]
        seed: Option<[u8; 32]>,
    },
}

impl ScheduleSourceConfig {
    #[must_use]
    pub fn into_runtime(self) -> ScheduleSource {
        match self {
            Self::Constant(v) => ScheduleSource::Constant(v),
            Self::DailyProfile {
                weekday,
                weekend,
                month_multipliers,
                max_value,
            } => ScheduleSource::DailyProfile {
                weekday,
                weekend,
                month_multipliers,
                max_value,
            },
            Self::ColumnRef { col_idx, boundary } => {
                ScheduleSource::ColumnRef { col_idx, boundary }
            }
            Self::SolarAware {
                daytime_fraction,
                evening_fraction,
                overnight_fraction,
                month_multipliers,
                max_value,
                dusk_altitude_threshold_deg,
                dawn_altitude_threshold_deg,
            } => ScheduleSource::SolarAware {
                daytime_fraction,
                evening_fraction,
                overnight_fraction,
                month_multipliers,
                max_value,
                dusk_altitude_threshold_deg,
                dawn_altitude_threshold_deg,
            },
            Self::TimeWindows {
                windows,
                default,
                seed,
            } => {
                let has_noise = windows.iter().any(|w| w.noise.is_some());
                if has_noise {
                    let seed = seed.unwrap_or([0_u8; 32]);
                    ScheduleSource::noisy_time_windows(windows, default, seed)
                } else {
                    ScheduleSource::TimeWindows {
                        windows,
                        default,
                        rng_state: None,
                    }
                }
            }
        }
    }
}

fn default_month_multipliers() -> [f64; 12] {
    [1.0; 12]
}

fn default_max_value() -> f64 {
    1.0
}

impl PartialEq for ScheduleSource {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Constant(a), Self::Constant(b)) => a == b,
            (
                Self::DailyProfile {
                    weekday: a_wd,
                    weekend: a_we,
                    month_multipliers: a_mm,
                    max_value: a_max,
                },
                Self::DailyProfile {
                    weekday: b_wd,
                    weekend: b_we,
                    month_multipliers: b_mm,
                    max_value: b_max,
                },
            ) => a_wd == b_wd && a_we == b_we && a_mm == b_mm && a_max == b_max,
            (
                Self::ColumnRef {
                    col_idx: a_idx,
                    boundary: a_boundary,
                },
                Self::ColumnRef {
                    col_idx: b_idx,
                    boundary: b_boundary,
                },
            ) => a_idx == b_idx && a_boundary == b_boundary,
            (
                Self::SolarAware {
                    daytime_fraction: a_day,
                    evening_fraction: a_eve,
                    overnight_fraction: a_overnight,
                    month_multipliers: a_mm,
                    max_value: a_max,
                    dusk_altitude_threshold_deg: a_dusk,
                    dawn_altitude_threshold_deg: a_dawn,
                },
                Self::SolarAware {
                    daytime_fraction: b_day,
                    evening_fraction: b_eve,
                    overnight_fraction: b_overnight,
                    month_multipliers: b_mm,
                    max_value: b_max,
                    dusk_altitude_threshold_deg: b_dusk,
                    dawn_altitude_threshold_deg: b_dawn,
                },
            ) => {
                a_day == b_day
                    && a_eve == b_eve
                    && a_overnight == b_overnight
                    && a_mm == b_mm
                    && a_max == b_max
                    && a_dusk == b_dusk
                    && a_dawn == b_dawn
            }
            (
                Self::Stochastic {
                    kind: a_kind,
                    seed: a_seed,
                    draw_count: a_count,
                    rng: _,
                    clamp_min: a_min,
                    clamp_max: a_max,
                },
                Self::Stochastic {
                    kind: b_kind,
                    seed: b_seed,
                    draw_count: b_count,
                    rng: _,
                    clamp_min: b_min,
                    clamp_max: b_max,
                },
            ) => {
                a_kind == b_kind
                    && a_seed == b_seed
                    && a_count == b_count
                    && a_min == b_min
                    && a_max == b_max
            }
            (
                Self::Shared {
                    data: a_data,
                    cursor: a_cursor,
                    boundary: a_boundary,
                },
                Self::Shared {
                    data: b_data,
                    cursor: b_cursor,
                    boundary: b_boundary,
                },
            ) => a_data == b_data && a_cursor == b_cursor && a_boundary == b_boundary,
            (
                Self::TimeWindows {
                    windows: a_win,
                    default: a_def,
                    rng_state: a_rng,
                },
                Self::TimeWindows {
                    windows: b_win,
                    default: b_def,
                    rng_state: b_rng,
                },
            ) => a_win == b_win && a_def == b_def && a_rng == b_rng,
            _ => false,
        }
    }
}

impl ScheduleSource {
    /// Resolve the current value from this source.
    pub fn value_at(&mut self, env: &EnvironmentState) -> Result<f64, HaresError> {
        match self {
            Self::Constant(v) => Ok(*v),
            Self::DailyProfile {
                weekday,
                weekend,
                month_multipliers,
                max_value,
            } => {
                let hour = env.current_time.hour() as usize;
                let month_idx = env.current_time.month0() as usize;
                let is_weekend = env.current_time.weekday().num_days_from_monday() >= 5;
                let frac = if is_weekend {
                    weekend[hour]
                } else {
                    weekday[hour]
                };
                Ok(frac * month_multipliers[month_idx] * *max_value)
            }
            Self::ColumnRef { col_idx, boundary } => {
                let payload = env
                    .custom_domains
                    .iter()
                    .find(|d| d.domain_id == SCHEDULE_DOMAIN_ID)
                    .and_then(|d| d.custom_payload.as_ref())
                    .ok_or_else(|| {
                        HaresError::Equipment(
                            "schedule domain payload not found in environment custom domains"
                                .to_string(),
                        )
                    })?;
                let idx = resolve_index(*col_idx, payload.len(), *boundary)?;
                Ok(payload[idx])
            }
            Self::SolarAware {
                daytime_fraction,
                evening_fraction,
                overnight_fraction,
                month_multipliers,
                max_value,
                dusk_altitude_threshold_deg,
                dawn_altitude_threshold_deg,
            } => {
                let month_idx = env.current_time.month0() as usize;
                let sun_alt = env.weather.solar_altitude_deg;

                let frac = if sun_alt > *dusk_altitude_threshold_deg {
                    *daytime_fraction
                } else if sun_alt > *dawn_altitude_threshold_deg {
                    *evening_fraction
                } else {
                    *overnight_fraction
                };

                Ok(frac * month_multipliers[month_idx] * *max_value)
            }
            Self::Stochastic {
                kind,
                seed: _,
                draw_count,
                rng,
                clamp_min,
                clamp_max,
            } => {
                let raw = kind.sample(rng)?;
                *draw_count = draw_count.checked_add(1).expect("draw_count overflow");
                let lo = clamp_min.unwrap_or(f64::NEG_INFINITY);
                let hi = clamp_max.unwrap_or(f64::INFINITY);
                Ok(raw.clamp(lo, hi))
            }
            Self::Shared {
                data,
                cursor,
                boundary,
            } => {
                let idx = resolve_index(*cursor, data.len(), *boundary)?;
                let value = data[idx];
                *cursor = cursor.saturating_add(1);
                Ok(value)
            }
            Self::TimeWindows {
                windows,
                default,
                rng_state,
            } => {
                let weekday = env.current_time.weekday();
                let minute_of_day =
                    env.current_time.hour() as u16 * 60 + env.current_time.minute() as u16;
                for w in windows.iter() {
                    if w.contains(weekday, minute_of_day) {
                        return resolve_window_value(w, rng_state);
                    }
                }
                default.ok_or_else(|| {
                    HaresError::Equipment(format!(
                        "no time window matches {} {:02}:{:02} and no default is set",
                        weekday,
                        env.current_time.hour(),
                        env.current_time.minute(),
                    ))
                })
            }
        }
    }

    /// Reset any internal mutable state (for checkpoint restart consistency).
    pub fn reset(&mut self) {
        match self {
            Self::Stochastic {
                seed,
                draw_count,
                rng,
                ..
            } => {
                *rng = ChaCha8Rng::from_seed(*seed);
                *draw_count = 0;
            }
            Self::Shared {
                data: _,
                cursor,
                boundary: _,
            } => {
                *cursor = 0;
            }
            Self::TimeWindows { rng_state, .. } => {
                if let Some(st) = rng_state.as_mut() {
                    st.reset();
                }
            }
            Self::Constant(_)
            | Self::DailyProfile { .. }
            | Self::ColumnRef { .. }
            | Self::SolarAware { .. } => {}
        }
    }
}

impl ScheduleSource {
    /// Approximate statistical mean of this source, computed without mutation.
    ///
    /// Returns the analytical mean for `Constant` and `Shared`.
    /// For `Stochastic`, returns the analytical distribution mean clamped to
    /// `[clamp_min, clamp_max]` -- this is an approximation when clamping is
    /// active (truncated distribution mean differs from clamped analytical mean).
    /// For time-varying sources (`DailyProfile`, `SolarAware`, `TimeWindows`),
    /// returns a representative average (not duration-weighted for `TimeWindows`).
    /// For `ColumnRef`, returns 0 (no data available without environment state).
    pub fn mean(&self) -> f64 {
        match self {
            Self::Constant(v) => *v,
            Self::DailyProfile {
                weekday,
                weekend,
                month_multipliers,
                max_value,
            } => {
                // Weighted average: 5 weekday + 2 weekend hours, uniform month.
                let wd_avg: f64 = weekday.iter().sum::<f64>() / 24.0;
                let we_avg: f64 = weekend.iter().sum::<f64>() / 24.0;
                let day_avg = (wd_avg * 5.0 + we_avg * 2.0) / 7.0;
                let month_avg: f64 = month_multipliers.iter().sum::<f64>() / 12.0;
                day_avg * month_avg * max_value
            }
            Self::ColumnRef { .. } => 0.0,
            Self::SolarAware {
                daytime_fraction,
                evening_fraction,
                overnight_fraction,
                month_multipliers,
                max_value,
                ..
            } => {
                // Rough average across a day (12h day, 6h evening, 6h overnight).
                let day_avg =
                    (daytime_fraction * 12.0 + evening_fraction * 6.0 + overnight_fraction * 6.0)
                        / 24.0;
                let month_avg: f64 = month_multipliers.iter().sum::<f64>() / 12.0;
                day_avg * month_avg * max_value
            }
            Self::Stochastic {
                kind,
                clamp_min,
                clamp_max,
                ..
            } => {
                let raw = kind.mean();
                let lo = clamp_min.unwrap_or(f64::NEG_INFINITY);
                let hi = clamp_max.unwrap_or(f64::INFINITY);
                raw.clamp(lo, hi)
            }
            Self::Shared { data, .. } => {
                if data.is_empty() {
                    0.0
                } else {
                    data.iter().sum::<f64>() / data.len() as f64
                }
            }
            Self::TimeWindows {
                windows, default, ..
            } => {
                if windows.is_empty() {
                    return default.unwrap_or(0.0);
                }
                let sum: f64 = windows
                    .iter()
                    .map(|w| w.value + w.noise.as_ref().map(|n| n.mean()).unwrap_or(0.0))
                    .sum();
                sum / windows.len() as f64
            }
        }
    }

    /// Create a `TimeWindows` source with a seeded RNG for noisy windows.
    ///
    /// At least one window should have `noise` set; otherwise prefer the
    /// plain `TimeWindows` constructor (no RNG allocated).
    pub fn noisy_time_windows(
        windows: Vec<TimeWindow>,
        default: Option<f64>,
        seed: [u8; 32],
    ) -> Self {
        assert!(
            windows.iter().any(|w| w.noise.is_some()),
            "noisy_time_windows called but no window has noise; use TimeWindows directly"
        );
        Self::TimeWindows {
            windows,
            default,
            rng_state: Some(Box::new(StochasticState::new(seed))),
        }
    }
}

/// Resolve the value of a matched time window, sampling noise if present.
fn resolve_window_value(
    w: &TimeWindow,
    rng_state: &mut Option<Box<StochasticState>>,
) -> Result<f64, HaresError> {
    let raw = match &w.noise {
        None => w.value,
        Some(dist) => {
            let st = rng_state.as_mut().ok_or_else(|| {
                HaresError::Equipment("time window has noise but no rng_state is set".into())
            })?;
            let noise = dist.sample(&mut st.rng)?;
            st.draw_count = st.draw_count.checked_add(1).expect("draw_count overflow");
            w.value + noise
        }
    };
    let lo = w.min_value.unwrap_or(f64::NEG_INFINITY);
    let hi = w.max_value.unwrap_or(f64::INFINITY);
    Ok(raw.clamp(lo, hi))
}

fn resolve_index(
    raw_idx: usize,
    len: usize,
    boundary: BoundaryPolicy,
) -> Result<usize, HaresError> {
    if len == 0 {
        return Err(HaresError::Equipment(
            "schedule source data is empty".to_string(),
        ));
    }

    if raw_idx < len {
        return Ok(raw_idx);
    }

    match boundary {
        BoundaryPolicy::Clamp => Ok(len - 1),
        BoundaryPolicy::Wrap => Ok(raw_idx % len),
        BoundaryPolicy::Error => Err(HaresError::Equipment(format!(
            "schedule index out of bounds: idx={raw_idx} len={len}"
        ))),
    }
}

/// Which season a tariff rate applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum SeasonFilter {
    #[default]
    All,
    Summer,
    Winter,
}

impl SeasonFilter {
    /// Returns `true` if the given 1-indexed month falls within this season.
    ///
    /// - `Summer` = June through September (months 6..=9)
    /// - `Winter` = October through May (months 1..=5 and 10..=12)
    /// - `All` = always true
    ///
    /// Debug-asserts that `month` is in 1..=12; in release, returns `false`
    /// for out-of-range months.
    pub fn contains_month(self, month: u8) -> bool {
        debug_assert!((1..=12).contains(&month), "month out of range: {month}");
        if !(1..=12).contains(&month) {
            return false;
        }
        match self {
            Self::All => true,
            Self::Summer => (6..=9).contains(&month),
            Self::Winter => !(6..=9).contains(&month),
        }
    }
}

/// Configurable summer/winter boundary for tariffs that don't use the
/// default June–September split. Supports wrapping (e.g. southern hemisphere
/// where summer might be Nov–Feb).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SeasonalSplit {
    /// First month of summer (1-indexed, inclusive).
    pub summer_start_month: u8,
    /// Last month of summer (1-indexed, inclusive).
    pub summer_end_month: u8,
}

impl SeasonalSplit {
    /// Create a new `SeasonalSplit`, validating that both months are in 1..=12.
    pub fn new(summer_start_month: u8, summer_end_month: u8) -> Result<Self, HaresError> {
        if !(1..=12).contains(&summer_start_month) || !(1..=12).contains(&summer_end_month) {
            return Err(HaresError::Equipment(format!(
                "SeasonalSplit months must be 1..=12, got start={summer_start_month}, end={summer_end_month}"
            )));
        }
        Ok(Self {
            summer_start_month,
            summer_end_month,
        })
    }

    /// Validate that both months are in 1..=12.
    pub fn validate(&self) -> Result<(), HaresError> {
        if !(1..=12).contains(&self.summer_start_month)
            || !(1..=12).contains(&self.summer_end_month)
        {
            return Err(HaresError::Tariff(format!(
                "SeasonalSplit months must be 1..=12, got start={}, end={}",
                self.summer_start_month, self.summer_end_month
            )));
        }
        Ok(())
    }

    /// Returns `true` if the given 1-indexed month is in the summer range.
    ///
    /// Handles wrapping: if `summer_start_month > summer_end_month` the range
    /// spans the year boundary (e.g. start=11, end=2 → Nov, Dec, Jan, Feb).
    pub fn is_summer(&self, month: u8) -> bool {
        debug_assert!((1..=12).contains(&month), "month out of range: {month}");
        if !(1..=12).contains(&month) {
            return false;
        }
        if self.summer_start_month <= self.summer_end_month {
            month >= self.summer_start_month && month <= self.summer_end_month
        } else {
            month >= self.summer_start_month || month <= self.summer_end_month
        }
    }
}

/// How often billing periods reset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum BillingCycle {
    #[default]
    Monthly,
    /// Custom billing period length in days.
    Custom(u32),
}

/// A named time-of-use period with day/time windows and season applicability.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TouPeriod {
    pub name: String,
    pub schedule: Vec<TimeWindow>,
    pub season: SeasonFilter,
}

impl TouPeriod {
    /// Validate that all contained `TimeWindow`s have valid minute ranges.
    pub fn validate(&self) -> Result<(), HaresError> {
        for (i, tw) in self.schedule.iter().enumerate() {
            if tw.start_minute >= 1440 {
                return Err(HaresError::Tariff(format!(
                    "TouPeriod '{}' schedule[{i}]: start_minute must be < 1440, got {}",
                    self.name, tw.start_minute
                )));
            }
            if tw.end_minute > 1440 {
                return Err(HaresError::Tariff(format!(
                    "TouPeriod '{}' schedule[{i}]: end_minute must be <= 1440, got {}",
                    self.name, tw.end_minute
                )));
            }
            if tw.start_minute == tw.end_minute {
                return Err(HaresError::Tariff(format!(
                    "TouPeriod '{}' schedule[{i}]: zero-width window (start == end == {})",
                    self.name, tw.start_minute
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::{FixedOffset, TimeZone};
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    use crate::{DomainUpdate, ZoneId, test_utils::default_env};

    use super::{
        BillingCycle, BoundaryPolicy, DayFilter, DistributionKind, SCHEDULE_DOMAIN_ID,
        ScheduleSource, SeasonFilter, SeasonalSplit, TimeWindow, TouPeriod,
    };

    #[test]
    fn constant_returns_constant() {
        let env = default_env();
        let mut source = ScheduleSource::Constant(7.25);
        assert_eq!(
            source.value_at(&env).expect("constant should resolve"),
            7.25
        );
    }

    #[test]
    fn daily_profile_varies_by_hour_weekday_and_month() {
        let mut env = default_env();
        let mut weekday = [0.0; 24];
        weekday[14] = 1.5;
        let mut weekend = [0.0; 24];
        weekend[14] = 0.75;
        let mut month = [1.0; 12];
        month[0] = 2.0;
        month[1] = 0.5;

        let mut source = ScheduleSource::DailyProfile {
            weekday,
            weekend,
            month_multipliers: month,
            max_value: 10.0,
        };

        let utc = FixedOffset::east_opt(0).expect("offset");
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 14, 14, 0, 0)
            .single()
            .expect("valid timestamp");
        assert_eq!(source.value_at(&env).expect("weekday value"), 30.0);

        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 17, 14, 0, 0)
            .single()
            .expect("valid timestamp");
        assert_eq!(source.value_at(&env).expect("weekend value"), 15.0);

        // 2026-02-10 is a Tuesday -- exercises the weekday+month_multiplier path.
        env.current_time = utc
            .with_ymd_and_hms(2026, 2, 10, 14, 0, 0)
            .single()
            .expect("valid timestamp");
        assert_eq!(source.value_at(&env).expect("month value"), 7.5);
    }

    // ── SolarAware tests ──────────────────────────────────────────

    fn solar_aware_source() -> ScheduleSource {
        ScheduleSource::SolarAware {
            daytime_fraction: 1.0,
            evening_fraction: 0.5,
            overnight_fraction: 0.1,
            month_multipliers: [1.0; 12],
            max_value: 100.0,
            dusk_altitude_threshold_deg: 0.0,
            dawn_altitude_threshold_deg: -6.0,
        }
    }

    #[test]
    fn solar_aware_daytime_when_sun_above_dusk_threshold() {
        let mut env = default_env();
        env.weather.solar_altitude_deg = 30.0; // well above 0°
        let mut source = solar_aware_source();
        assert_eq!(source.value_at(&env).unwrap(), 100.0);
    }

    #[test]
    fn solar_aware_evening_when_sun_between_thresholds() {
        let mut env = default_env();
        env.weather.solar_altitude_deg = -3.0; // below dusk (0°), above dawn (-6°)
        let mut source = solar_aware_source();
        assert_eq!(source.value_at(&env).unwrap(), 50.0);
    }

    #[test]
    fn solar_aware_overnight_when_sun_below_dawn_threshold() {
        let mut env = default_env();
        env.weather.solar_altitude_deg = -10.0; // below dawn (-6°)
        let mut source = solar_aware_source();
        assert_eq!(source.value_at(&env).unwrap(), 10.0);
    }

    #[test]
    fn solar_aware_monthly_scaling() {
        let mut env = default_env();
        env.weather.solar_altitude_deg = 30.0;
        let utc = FixedOffset::east_opt(0).expect("offset");
        // July (month index 6)
        env.current_time = utc
            .with_ymd_and_hms(2026, 7, 15, 12, 0, 0)
            .single()
            .expect("valid timestamp");

        let mut month_mults = [1.0; 12];
        month_mults[6] = 0.75; // July multiplier
        let mut source = ScheduleSource::SolarAware {
            daytime_fraction: 1.0,
            evening_fraction: 0.5,
            overnight_fraction: 0.1,
            month_multipliers: month_mults,
            max_value: 100.0,
            dusk_altitude_threshold_deg: 0.0,
            dawn_altitude_threshold_deg: -6.0,
        };
        assert_eq!(source.value_at(&env).unwrap(), 75.0);
    }

    #[test]
    fn solar_aware_boundary_at_exact_thresholds() {
        let mut env = default_env();
        let mut source = solar_aware_source();

        // Exactly at dusk threshold (0.0) → NOT daytime (strictly >)
        env.weather.solar_altitude_deg = 0.0;
        assert_eq!(source.value_at(&env).unwrap(), 50.0);

        // Exactly at dawn threshold (-6.0) → NOT evening (strictly >)
        env.weather.solar_altitude_deg = -6.0;
        assert_eq!(source.value_at(&env).unwrap(), 10.0);
    }

    #[test]
    fn column_ref_reads_schedule_domain_payload() {
        let mut env = default_env();
        env.custom_domains = vec![DomainUpdate {
            domain_id: SCHEDULE_DOMAIN_ID,
            zone_temperatures_c: vec![(ZoneId(1), 21.0)],
            custom_payload: Some(vec![2.0, 4.0, 6.0]),
        }];

        let mut source = ScheduleSource::ColumnRef {
            col_idx: 1,
            boundary: BoundaryPolicy::Clamp,
        };

        assert_eq!(source.value_at(&env).expect("column value"), 4.0);
    }

    #[test]
    fn boundary_policy_wrap_wraps() {
        let env = default_env();
        let data: Arc<[f64]> = Arc::from(vec![10.0, 20.0, 30.0]);
        let mut source = ScheduleSource::Shared {
            data,
            cursor: 5,
            boundary: BoundaryPolicy::Wrap,
        };

        assert_eq!(source.value_at(&env).expect("wrapped value"), 30.0);
    }

    #[test]
    fn boundary_policy_error_errors_when_out_of_bounds() {
        let env = default_env();
        let data: Arc<[f64]> = Arc::from(vec![10.0, 20.0]);
        let mut source = ScheduleSource::Shared {
            data,
            cursor: 5,
            boundary: BoundaryPolicy::Error,
        };

        let err = source
            .value_at(&env)
            .expect_err("out-of-bounds should error");
        assert!(
            err.to_string().contains("out of bounds"),
            "expected out-of-bounds error, got: {err}"
        );
    }

    #[test]
    fn stochastic_gaussian_deterministic_and_reset() {
        let kind = DistributionKind::Gaussian {
            mean: 5.0,
            std_dev: 1.2,
        };
        assert_deterministic(kind.clone(), 8);

        // reset() must rewind the RNG to the beginning of the sequence.
        let env = default_env();
        let mut src = stochastic(kind, [7_u8; 32]);
        let first = src.value_at(&env).expect("first draw");
        for _ in 0..7 {
            src.value_at(&env).unwrap();
        }
        src.reset();
        assert_eq!(
            src.value_at(&env).expect("after reset"),
            first,
            "reset() did not rewind to the first draw"
        );
    }

    #[test]
    fn shared_cursor_advances() {
        let env = default_env();
        let data: Arc<[f64]> = Arc::from(vec![1.0, 2.0, 3.0]);
        let mut source = ScheduleSource::Shared {
            data,
            cursor: 0,
            boundary: BoundaryPolicy::Clamp,
        };

        assert_eq!(source.value_at(&env).expect("first"), 1.0);
        assert_eq!(source.value_at(&env).expect("second"), 2.0);
        assert_eq!(source.value_at(&env).expect("third"), 3.0);
        assert_eq!(source.value_at(&env).expect("clamped"), 3.0);
    }

    // ── TimeWindows tests ──────────────────────────────────────────

    #[test]
    fn day_filter_matches_correctly() {
        use chrono::Weekday;

        assert!(DayFilter::Any.matches(Weekday::Mon));
        assert!(DayFilter::Any.matches(Weekday::Sat));

        assert!(DayFilter::Weekdays.matches(Weekday::Mon));
        assert!(DayFilter::Weekdays.matches(Weekday::Fri));
        assert!(!DayFilter::Weekdays.matches(Weekday::Sat));
        assert!(!DayFilter::Weekdays.matches(Weekday::Sun));

        assert!(!DayFilter::Weekends.matches(Weekday::Mon));
        assert!(DayFilter::Weekends.matches(Weekday::Sat));
        assert!(DayFilter::Weekends.matches(Weekday::Sun));

        assert!(DayFilter::Day(Weekday::Wed).matches(Weekday::Wed));
        assert!(!DayFilter::Day(Weekday::Wed).matches(Weekday::Thu));
    }

    #[test]
    fn time_window_contains_normal_range() {
        use chrono::Weekday;

        // 07:00–13:00 on any day
        let w = TimeWindow::new(DayFilter::Any, 420, 780, 21.0);

        assert!(!w.contains(Weekday::Mon, 419)); // 06:59
        assert!(w.contains(Weekday::Mon, 420)); // 07:00 inclusive
        assert!(w.contains(Weekday::Mon, 600)); // 10:00
        assert!(!w.contains(Weekday::Mon, 780)); // 13:00 exclusive
    }

    #[test]
    fn time_window_contains_midnight_wrap() {
        use chrono::Weekday;

        // 22:00–06:00 wrapping midnight
        let w = TimeWindow::new(DayFilter::Any, 1320, 360, 18.0);

        assert!(w.contains(Weekday::Tue, 1320)); // 22:00 inclusive
        assert!(w.contains(Weekday::Tue, 1439)); // 23:59
        assert!(w.contains(Weekday::Tue, 0)); // 00:00
        assert!(w.contains(Weekday::Tue, 359)); // 05:59
        assert!(!w.contains(Weekday::Tue, 360)); // 06:00 exclusive
        assert!(!w.contains(Weekday::Tue, 720)); // 12:00
    }

    #[test]
    fn time_windows_first_match_wins() {
        // default_env is 2026-01-01 00:00 UTC = Thursday
        let mut env = default_env();
        let utc = FixedOffset::east_opt(0).expect("offset");

        // Set to Thursday 10:30
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 1, 10, 30, 0)
            .single()
            .expect("valid timestamp");

        let mut source = ScheduleSource::TimeWindows {
            windows: vec![
                // Weekdays 07:00–13:00 → 21.0
                TimeWindow::new(DayFilter::Weekdays, 420, 780, 21.0),
                // Any day 00:00–23:59 → 18.0 (catch-all, lower priority)
                TimeWindow::new(DayFilter::Any, 0, 1440, 18.0),
            ],
            default: None,
            rng_state: None,
        };

        assert_eq!(
            source.value_at(&env).expect("should match weekday window"),
            21.0
        );
    }

    #[test]
    fn time_windows_falls_through_to_later_window() {
        let mut env = default_env();
        let utc = FixedOffset::east_opt(0).expect("offset");

        // Saturday 10:30
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 3, 10, 30, 0)
            .single()
            .expect("valid timestamp");

        let mut source = ScheduleSource::TimeWindows {
            windows: vec![
                // Weekdays only → won't match Saturday
                TimeWindow::new(DayFilter::Weekdays, 420, 780, 21.0),
                // Any day catch-all
                TimeWindow::new(DayFilter::Any, 0, 1440, 18.0),
            ],
            default: None,
            rng_state: None,
        };

        assert_eq!(
            source.value_at(&env).expect("should fall through to Any"),
            18.0
        );
    }

    #[test]
    fn time_windows_uses_default_when_no_match() {
        let mut env = default_env();
        let utc = FixedOffset::east_opt(0).expect("offset");

        // Thursday 03:00
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 1, 3, 0, 0)
            .single()
            .expect("valid timestamp");

        let mut source = ScheduleSource::TimeWindows {
            windows: vec![
                // Only 07:00–13:00
                TimeWindow::new(DayFilter::Any, 420, 780, 21.0),
            ],
            default: Some(15.0),
            rng_state: None,
        };

        assert_eq!(source.value_at(&env).expect("should use default"), 15.0);
    }

    #[test]
    fn time_windows_errors_without_match_or_default() {
        let mut env = default_env();
        let utc = FixedOffset::east_opt(0).expect("offset");

        // Thursday 03:00
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 1, 3, 0, 0)
            .single()
            .expect("valid timestamp");

        let mut source = ScheduleSource::TimeWindows {
            windows: vec![TimeWindow::new(DayFilter::Any, 420, 780, 21.0)],
            default: None,
            rng_state: None,
        };

        let err = source
            .value_at(&env)
            .expect_err("should error with no match and no default");
        assert!(
            err.to_string().contains("no time window matches"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn time_windows_specific_day_filter() {
        let mut env = default_env();
        let utc = FixedOffset::east_opt(0).expect("offset");

        // 2026-01-05 is a Monday
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 5, 8, 0, 0)
            .single()
            .expect("valid timestamp");

        let mut source = ScheduleSource::TimeWindows {
            windows: vec![
                // Monday 00:00–07:00 → 20.5
                TimeWindow::new(DayFilter::Day(chrono::Weekday::Mon), 0, 420, 20.5),
                // Monday 07:00–13:00 → 22.8
                TimeWindow::new(DayFilter::Day(chrono::Weekday::Mon), 420, 780, 22.8),
            ],
            default: Some(19.0),
            rng_state: None,
        };

        // 08:00 Monday → should hit second window
        assert_eq!(source.value_at(&env).expect("Monday 08:00"), 22.8);

        // 06:00 Monday → should hit first window
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 5, 6, 0, 0)
            .single()
            .expect("valid timestamp");
        assert_eq!(source.value_at(&env).expect("Monday 06:00"), 20.5);

        // Tuesday 08:00 → neither window matches, use default
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 6, 8, 0, 0)
            .single()
            .expect("valid timestamp");
        assert_eq!(source.value_at(&env).expect("Tuesday 08:00"), 19.0);
    }

    #[test]
    fn time_windows_midnight_wrap_integration() {
        let mut env = default_env();
        let utc = FixedOffset::east_opt(0).expect("offset");

        let mut source = ScheduleSource::TimeWindows {
            windows: vec![
                // Overnight setback: 22:00–06:00 → 18.0
                TimeWindow::new(DayFilter::Any, 1320, 360, 18.0),
                // Daytime: 06:00–22:00 → 22.0
                TimeWindow::new(DayFilter::Any, 360, 1320, 22.0),
            ],
            default: None,
            rng_state: None,
        };

        // 23:00 → overnight
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 1, 23, 0, 0)
            .single()
            .expect("valid timestamp");
        assert_eq!(source.value_at(&env).expect("23:00"), 18.0);

        // 02:00 → overnight
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 1, 2, 0, 0)
            .single()
            .expect("valid timestamp");
        assert_eq!(source.value_at(&env).expect("02:00"), 18.0);

        // 12:00 → daytime
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 1, 12, 0, 0)
            .single()
            .expect("valid timestamp");
        assert_eq!(source.value_at(&env).expect("12:00"), 22.0);
    }

    #[test]
    fn time_window_full_day_with_1440() {
        use chrono::Weekday;

        let w = TimeWindow::new(DayFilter::Any, 0, 1440, 20.0);
        assert!(w.contains(Weekday::Mon, 0));
        assert!(w.contains(Weekday::Mon, 720));
        assert!(w.contains(Weekday::Mon, 1439));
    }

    #[test]
    fn time_window_midnight_wrap_with_day_filter() {
        use chrono::Weekday;

        // Monday 22:00 – Tuesday 06:00
        let w = TimeWindow::new(DayFilter::Day(Weekday::Mon), 1320, 360, 18.0);

        // Monday 23:00 → anchor day matches
        assert!(w.contains(Weekday::Mon, 1380));
        // Tuesday 03:00 → post-midnight, predecessor is Monday
        assert!(w.contains(Weekday::Tue, 180));
        // Wednesday 03:00 → predecessor is Tuesday, not Monday
        assert!(!w.contains(Weekday::Wed, 180));
        // Monday 12:00 → outside the time range
        assert!(!w.contains(Weekday::Mon, 720));
    }

    #[test]
    fn time_window_midnight_wrap_weekday_to_weekend_boundary() {
        use chrono::Weekday;

        // Friday 22:00 – Saturday 06:00 with Weekdays filter
        let w = TimeWindow::new(DayFilter::Weekdays, 1320, 360, 18.0);

        // Friday 23:00 → weekday, matches
        assert!(w.contains(Weekday::Fri, 1380));
        // Saturday 03:00 → pred is Friday (weekday), matches
        assert!(w.contains(Weekday::Sat, 180));
        // Sunday 03:00 → pred is Saturday (weekend), no match
        assert!(!w.contains(Weekday::Sun, 180));
    }

    #[test]
    fn time_windows_empty_windows_with_no_default_errors() {
        let env = default_env();
        let mut source = ScheduleSource::TimeWindows {
            windows: vec![],
            default: None,
            rng_state: None,
        };

        let err = source
            .value_at(&env)
            .expect_err("empty windows + no default should error");
        assert!(err.to_string().contains("no time window matches"));
    }

    #[test]
    fn time_windows_empty_windows_with_default() {
        let env = default_env();
        let mut source = ScheduleSource::TimeWindows {
            windows: vec![],
            default: Some(17.0),
            rng_state: None,
        };

        assert_eq!(source.value_at(&env).expect("should use default"), 17.0);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "zero-width")]
    fn time_window_zero_width_panics_in_debug() {
        TimeWindow::new(DayFilter::Any, 420, 420, 21.0);
    }

    #[test]
    fn time_windows_weekday_weekend_split() {
        let mut env = default_env();
        let utc = FixedOffset::east_opt(0).expect("offset");

        let mut source = ScheduleSource::TimeWindows {
            windows: vec![
                TimeWindow::new(DayFilter::Weekdays, 0, 1440, 21.0),
                TimeWindow::new(DayFilter::Weekends, 0, 1440, 24.0),
            ],
            default: None,
            rng_state: None,
        };

        // Thursday (weekday)
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 1, 12, 0, 0)
            .single()
            .expect("valid timestamp");
        assert_eq!(source.value_at(&env).expect("weekday"), 21.0);

        // Saturday (weekend)
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 3, 12, 0, 0)
            .single()
            .expect("valid timestamp");
        assert_eq!(source.value_at(&env).expect("weekend"), 24.0);
    }

    // ── Stochastic distribution tests ────────────────────────────────

    /// Helper: build a `Stochastic` source with the given kind and seed.
    fn stochastic(kind: DistributionKind, seed: [u8; 32]) -> ScheduleSource {
        ScheduleSource::Stochastic {
            kind,
            seed,
            draw_count: 0,
            rng: ChaCha8Rng::from_seed(seed),
            clamp_min: None,
            clamp_max: None,
        }
    }

    /// Assert two identically-seeded sources produce the same sequence.
    fn assert_deterministic(kind: DistributionKind, n: usize) {
        let env = default_env();
        let seed = [42_u8; 32];
        let mut a = stochastic(kind.clone(), seed);
        let mut b = stochastic(kind, seed);
        for i in 0..n {
            let va = a.value_at(&env).unwrap();
            let vb = b.value_at(&env).unwrap();
            assert_eq!(va, vb, "draw {i} diverged");
        }
    }

    #[test]
    fn stochastic_uniform_deterministic_and_in_range() {
        let kind = DistributionKind::Uniform {
            low: 2.0,
            high: 5.0,
        };
        assert_deterministic(kind.clone(), 20);

        let env = default_env();
        let mut src = stochastic(kind, [1_u8; 32]);
        for _ in 0..100 {
            let v = src.value_at(&env).unwrap();
            assert!((2.0..5.0).contains(&v), "uniform out of range: {v}");
        }
    }

    #[test]
    fn stochastic_lognormal_deterministic_and_positive() {
        let kind = DistributionKind::LogNormal {
            mu: 0.0,
            sigma: 1.0,
        };
        assert_deterministic(kind.clone(), 20);

        let env = default_env();
        let mut src = stochastic(kind, [2_u8; 32]);
        for _ in 0..100 {
            let v = src.value_at(&env).unwrap();
            assert!(v > 0.0, "lognormal should be positive: {v}");
        }
    }

    #[test]
    fn stochastic_exponential_deterministic_and_non_negative() {
        let kind = DistributionKind::Exponential { lambda: 2.0 };
        assert_deterministic(kind.clone(), 20);

        let env = default_env();
        let mut src = stochastic(kind, [3_u8; 32]);
        for _ in 0..100 {
            let v = src.value_at(&env).unwrap();
            assert!(v >= 0.0, "exponential should be non-negative: {v}");
        }
    }

    #[test]
    fn stochastic_poisson_deterministic_and_integer_valued() {
        let kind = DistributionKind::Poisson { lambda: 5.0 };
        assert_deterministic(kind.clone(), 20);

        let env = default_env();
        let mut src = stochastic(kind, [4_u8; 32]);
        for _ in 0..100 {
            let v = src.value_at(&env).unwrap();
            assert!(v >= 0.0, "poisson should be non-negative: {v}");
            assert_eq!(v, v.floor(), "poisson should be integer-valued: {v}");
        }
    }

    #[test]
    fn stochastic_bernoulli_deterministic_and_binary() {
        let kind = DistributionKind::Bernoulli { p: 0.5 };
        assert_deterministic(kind.clone(), 20);

        let env = default_env();
        let mut src = stochastic(kind, [5_u8; 32]);
        let mut saw_zero = false;
        let mut saw_one = false;
        for _ in 0..100 {
            let v = src.value_at(&env).unwrap();
            assert!(v == 0.0 || v == 1.0, "bernoulli should be 0 or 1: {v}");
            if v == 0.0 {
                saw_zero = true;
            } else {
                saw_one = true;
            }
        }
        assert!(saw_zero && saw_one, "p=0.5 should produce both 0 and 1");
    }

    #[test]
    fn stochastic_poisson_reset_replays_sequence() {
        let env = default_env();
        let kind = DistributionKind::Poisson { lambda: 3.0 };
        let mut src = stochastic(kind, [6_u8; 32]);

        let mut first_values = Vec::with_capacity(10);
        for _ in 0..10 {
            first_values.push(src.value_at(&env).unwrap());
        }

        src.reset();

        for (i, expected) in first_values.iter().enumerate() {
            let v = src.value_at(&env).unwrap();
            assert_eq!(v, *expected, "post-reset draw {i} diverged");
        }
    }

    #[test]
    fn stochastic_clamp_min_prevents_negative() {
        let env = default_env();
        let mut src = ScheduleSource::Stochastic {
            kind: DistributionKind::Gaussian {
                mean: 1.0,
                std_dev: 5.0,
            },
            seed: [8_u8; 32],
            draw_count: 0,
            rng: ChaCha8Rng::from_seed([8_u8; 32]),
            clamp_min: Some(0.0),
            clamp_max: None,
        };
        for _ in 0..200 {
            let v = src.value_at(&env).unwrap();
            assert!(v >= 0.0, "clamp_min violated: {v}");
        }
    }

    #[test]
    fn stochastic_clamp_max_caps_output() {
        let env = default_env();
        let mut src = ScheduleSource::Stochastic {
            kind: DistributionKind::Gaussian {
                mean: 90.0,
                std_dev: 20.0,
            },
            seed: [9_u8; 32],
            draw_count: 0,
            rng: ChaCha8Rng::from_seed([9_u8; 32]),
            clamp_min: None,
            clamp_max: Some(100.0),
        };
        for _ in 0..200 {
            let v = src.value_at(&env).unwrap();
            assert!(v <= 100.0, "clamp_max violated: {v}");
        }
    }

    // ── Noisy TimeWindows tests (DER-011) ────────────────────────────

    #[test]
    fn noisy_time_windows_backward_compat_no_noise() {
        let env = default_env();
        let mut source = ScheduleSource::TimeWindows {
            windows: vec![TimeWindow::new(DayFilter::Any, 0, 1440, 21.0)],
            default: None,
            rng_state: None,
        };
        assert_eq!(source.value_at(&env).unwrap(), 21.0);
    }

    #[test]
    fn noisy_time_windows_gaussian_clusters_around_value() {
        let mut env = default_env();
        let utc = FixedOffset::east_opt(0).expect("offset");
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 5, 10, 0, 0)
            .single()
            .expect("valid timestamp");

        let seed = [10_u8; 32];
        let mut source = ScheduleSource::noisy_time_windows(
            vec![TimeWindow::with_noise(
                DayFilter::Any,
                0,
                1440,
                38.0,
                DistributionKind::Gaussian {
                    mean: 0.0,
                    std_dev: 5.0,
                },
                None,
                None,
            )],
            None,
            seed,
        );

        let mut sum = 0.0;
        let n = 100;
        for _ in 0..n {
            sum += source.value_at(&env).unwrap();
        }
        let mean = sum / n as f64;
        assert!(
            (mean - 38.0).abs() < 3.0,
            "mean should cluster around 38.0, got {mean}"
        );
    }

    #[test]
    fn noisy_time_windows_deterministic_with_same_seed() {
        let mut env = default_env();
        let utc = FixedOffset::east_opt(0).expect("offset");
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 5, 10, 0, 0)
            .single()
            .expect("valid timestamp");

        let seed = [11_u8; 32];
        let windows = vec![TimeWindow::with_noise(
            DayFilter::Any,
            0,
            1440,
            10.0,
            DistributionKind::Gaussian {
                mean: 0.0,
                std_dev: 3.0,
            },
            None,
            None,
        )];

        let mut a = ScheduleSource::noisy_time_windows(windows.clone(), None, seed);
        let mut b = ScheduleSource::noisy_time_windows(windows, None, seed);

        for i in 0..20 {
            let va = a.value_at(&env).unwrap();
            let vb = b.value_at(&env).unwrap();
            assert_eq!(va, vb, "draw {i} diverged");
        }
    }

    #[test]
    fn noisy_time_windows_different_seeds_differ() {
        let mut env = default_env();
        let utc = FixedOffset::east_opt(0).expect("offset");
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 5, 10, 0, 0)
            .single()
            .expect("valid timestamp");

        let windows = vec![TimeWindow::with_noise(
            DayFilter::Any,
            0,
            1440,
            10.0,
            DistributionKind::Gaussian {
                mean: 0.0,
                std_dev: 3.0,
            },
            None,
            None,
        )];

        let mut a = ScheduleSource::noisy_time_windows(windows.clone(), None, [12_u8; 32]);
        let mut b = ScheduleSource::noisy_time_windows(windows, None, [13_u8; 32]);

        let mut any_differ = false;
        for _ in 0..10 {
            if a.value_at(&env).unwrap() != b.value_at(&env).unwrap() {
                any_differ = true;
                break;
            }
        }
        assert!(
            any_differ,
            "different seeds should produce different values"
        );
    }

    #[test]
    fn noisy_time_windows_reset_replays() {
        let mut env = default_env();
        let utc = FixedOffset::east_opt(0).expect("offset");
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 5, 10, 0, 0)
            .single()
            .expect("valid timestamp");

        let mut source = ScheduleSource::noisy_time_windows(
            vec![TimeWindow::with_noise(
                DayFilter::Any,
                0,
                1440,
                20.0,
                DistributionKind::Gaussian {
                    mean: 0.0,
                    std_dev: 4.0,
                },
                None,
                None,
            )],
            None,
            [14_u8; 32],
        );

        let mut first = Vec::with_capacity(5);
        for _ in 0..5 {
            first.push(source.value_at(&env).unwrap());
        }
        source.reset();
        for (i, expected) in first.iter().enumerate() {
            let v = source.value_at(&env).unwrap();
            assert_eq!(v, *expected, "post-reset draw {i} diverged");
        }
    }

    #[test]
    fn noisy_time_windows_mixed_noisy_and_noiseless() {
        let mut env = default_env();
        let utc = FixedOffset::east_opt(0).expect("offset");

        let mut source = ScheduleSource::noisy_time_windows(
            vec![
                // Weekdays: noisy
                TimeWindow::with_noise(
                    DayFilter::Weekdays,
                    0,
                    1440,
                    12.0,
                    DistributionKind::Gaussian {
                        mean: 0.0,
                        std_dev: 6.0,
                    },
                    Some(0.0),
                    None,
                ),
                // Weekends: fixed
                TimeWindow::new(DayFilter::Weekends, 0, 1440, 25.0),
            ],
            Some(0.0),
            [15_u8; 32],
        );

        // Saturday → fixed, exact
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 3, 12, 0, 0)
            .single()
            .expect("valid timestamp");
        assert_eq!(source.value_at(&env).unwrap(), 25.0);

        // Monday → noisy, varies
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 5, 12, 0, 0)
            .single()
            .expect("valid timestamp");
        let v = source.value_at(&env).unwrap();
        assert!(v >= 0.0, "clamp should prevent negative: {v}");
    }

    #[test]
    fn noisy_time_windows_no_match_uses_exact_default() {
        let mut env = default_env();
        let utc = FixedOffset::east_opt(0).expect("offset");
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 5, 3, 0, 0)
            .single()
            .expect("valid timestamp");

        let mut source = ScheduleSource::noisy_time_windows(
            vec![TimeWindow::with_noise(
                DayFilter::Any,
                420,
                780,
                10.0,
                DistributionKind::Gaussian {
                    mean: 0.0,
                    std_dev: 2.0,
                },
                None,
                None,
            )],
            Some(0.0),
            [16_u8; 32],
        );
        assert_eq!(source.value_at(&env).unwrap(), 0.0);
    }

    #[test]
    fn noisy_time_windows_errors_when_noise_without_rng() {
        let mut env = default_env();
        let utc = FixedOffset::east_opt(0).expect("offset");
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 5, 10, 0, 0)
            .single()
            .expect("valid timestamp");

        let mut source = ScheduleSource::TimeWindows {
            windows: vec![TimeWindow::with_noise(
                DayFilter::Any,
                0,
                1440,
                10.0,
                DistributionKind::Gaussian {
                    mean: 0.0,
                    std_dev: 1.0,
                },
                None,
                None,
            )],
            default: None,
            rng_state: None, // oops -- no RNG
        };

        let err = source
            .value_at(&env)
            .expect_err("should error when noise set but no rng_state");
        assert!(
            err.to_string().contains("no rng_state"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn noisy_time_windows_clamp_min_prevents_negative() {
        let mut env = default_env();
        let utc = FixedOffset::east_opt(0).expect("offset");
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 5, 10, 0, 0)
            .single()
            .expect("valid timestamp");

        let mut source = ScheduleSource::noisy_time_windows(
            vec![TimeWindow::with_noise(
                DayFilter::Any,
                0,
                1440,
                5.0,
                DistributionKind::Gaussian {
                    mean: 0.0,
                    std_dev: 10.0,
                },
                Some(0.0),
                None,
            )],
            None,
            [17_u8; 32],
        );

        for _ in 0..1000 {
            let v = source.value_at(&env).unwrap();
            assert!(v >= 0.0, "clamp_min violated: {v}");
        }
    }

    #[test]
    fn noisy_time_windows_clamp_max_caps_output() {
        let mut env = default_env();
        let utc = FixedOffset::east_opt(0).expect("offset");
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 5, 10, 0, 0)
            .single()
            .expect("valid timestamp");

        let mut source = ScheduleSource::noisy_time_windows(
            vec![TimeWindow::with_noise(
                DayFilter::Any,
                0,
                1440,
                90.0,
                DistributionKind::Gaussian {
                    mean: 0.0,
                    std_dev: 20.0,
                },
                None,
                Some(100.0),
            )],
            None,
            [18_u8; 32],
        );

        for _ in 0..1000 {
            let v = source.value_at(&env).unwrap();
            assert!(v <= 100.0, "clamp_max violated: {v}");
        }
    }

    #[test]
    fn noiseless_window_with_clamp_returns_exact_value() {
        let env = default_env();
        let mut source = ScheduleSource::TimeWindows {
            windows: vec![{
                let mut w = TimeWindow::new(DayFilter::Any, 0, 1440, 50.0);
                w.min_value = Some(0.0);
                w.max_value = Some(100.0);
                w
            }],
            default: None,
            rng_state: None,
        };
        assert_eq!(source.value_at(&env).unwrap(), 50.0);
    }

    #[test]
    fn noisy_time_windows_weekday_vs_weekend_distributions() {
        let mut env = default_env();
        let utc = FixedOffset::east_opt(0).expect("offset");

        let mut source = ScheduleSource::noisy_time_windows(
            vec![
                TimeWindow::with_noise(
                    DayFilter::Weekdays,
                    0,
                    1440,
                    12.0,
                    DistributionKind::Gaussian {
                        mean: 0.0,
                        std_dev: 6.0,
                    },
                    Some(0.0),
                    None,
                ),
                TimeWindow::with_noise(
                    DayFilter::Weekends,
                    0,
                    1440,
                    25.0,
                    DistributionKind::Gaussian {
                        mean: 0.0,
                        std_dev: 10.0,
                    },
                    Some(0.0),
                    None,
                ),
            ],
            Some(0.0),
            [19_u8; 32],
        );

        // Collect weekday samples (center=12, clamped ≥ 0)
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 5, 12, 0, 0)
            .single()
            .expect("valid timestamp");
        let mut wd_sum = 0.0;
        let n = 200;
        for _ in 0..n {
            wd_sum += source.value_at(&env).unwrap();
        }
        let wd_mean = wd_sum / n as f64;

        // Collect weekend samples (center=25, clamped ≥ 0)
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 3, 12, 0, 0)
            .single()
            .expect("valid timestamp");
        let mut we_sum = 0.0;
        for _ in 0..n {
            we_sum += source.value_at(&env).unwrap();
        }
        let we_mean = we_sum / n as f64;

        // Each mean should be within a reasonable bound of its center.
        assert!(
            (wd_mean - 12.0).abs() < 4.0,
            "weekday mean {wd_mean} should be near 12.0"
        );
        assert!(
            (we_mean - 25.0).abs() < 6.0,
            "weekend mean {we_mean} should be near 25.0"
        );
    }

    // ── Validation tests ─────────────────────────────────────────────

    #[test]
    fn validate_rejects_nan_parameters() {
        assert!(
            DistributionKind::Gaussian {
                mean: f64::NAN,
                std_dev: 1.0
            }
            .validate()
            .is_err()
        );
        assert!(
            DistributionKind::Uniform {
                low: f64::NAN,
                high: 1.0
            }
            .validate()
            .is_err()
        );
        assert!(
            DistributionKind::LogNormal {
                mu: 0.0,
                sigma: f64::NAN
            }
            .validate()
            .is_err()
        );
        assert!(
            DistributionKind::Exponential { lambda: f64::NAN }
                .validate()
                .is_err()
        );
        assert!(
            DistributionKind::Poisson { lambda: f64::NAN }
                .validate()
                .is_err()
        );
        assert!(
            DistributionKind::Bernoulli { p: f64::NAN }
                .validate()
                .is_err()
        );
    }

    #[test]
    fn validate_rejects_infinity_parameters() {
        assert!(
            DistributionKind::Gaussian {
                mean: f64::INFINITY,
                std_dev: 1.0
            }
            .validate()
            .is_err()
        );
        assert!(
            DistributionKind::Exponential {
                lambda: f64::INFINITY
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn validate_accepts_valid_parameters() {
        assert!(
            DistributionKind::Gaussian {
                mean: 0.0,
                std_dev: 1.0
            }
            .validate()
            .is_ok()
        );
        assert!(
            DistributionKind::Uniform {
                low: 0.0,
                high: 1.0
            }
            .validate()
            .is_ok()
        );
        assert!(
            DistributionKind::LogNormal {
                mu: 0.0,
                sigma: 1.0
            }
            .validate()
            .is_ok()
        );
        assert!(
            DistributionKind::Exponential { lambda: 2.0 }
                .validate()
                .is_ok()
        );
        assert!(DistributionKind::Poisson { lambda: 5.0 }.validate().is_ok());
        assert!(DistributionKind::Bernoulli { p: 0.5 }.validate().is_ok());
    }

    // ── Boundary / edge-case tests ───────────────────────────────────

    #[test]
    fn validate_boundary_gaussian_zero_std_dev() {
        assert!(
            DistributionKind::Gaussian {
                mean: 5.0,
                std_dev: 0.0
            }
            .validate()
            .is_ok()
        );
        // Zero std_dev degrades to constant -- verify sampling returns mean.
        let env = default_env();
        let mut src = stochastic(
            DistributionKind::Gaussian {
                mean: 5.0,
                std_dev: 0.0,
            },
            [20_u8; 32],
        );
        assert_eq!(src.value_at(&env).unwrap(), 5.0);
    }

    #[test]
    fn validate_boundary_uniform_low_eq_high() {
        assert!(
            DistributionKind::Uniform {
                low: 3.0,
                high: 3.0
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn validate_boundary_lognormal_zero_sigma() {
        assert!(
            DistributionKind::LogNormal {
                mu: 0.0,
                sigma: 0.0
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn validate_boundary_bernoulli_zero_and_one() {
        assert!(DistributionKind::Bernoulli { p: 0.0 }.validate().is_ok());
        assert!(DistributionKind::Bernoulli { p: 1.0 }.validate().is_ok());

        // p=0 always returns 0, p=1 always returns 1
        let env = default_env();
        let mut src0 = stochastic(DistributionKind::Bernoulli { p: 0.0 }, [21_u8; 32]);
        for _ in 0..10 {
            assert_eq!(src0.value_at(&env).unwrap(), 0.0);
        }
        let mut src1 = stochastic(DistributionKind::Bernoulli { p: 1.0 }, [22_u8; 32]);
        for _ in 0..10 {
            assert_eq!(src1.value_at(&env).unwrap(), 1.0);
        }
    }

    #[test]
    fn stochastic_partial_eq_differs_on_clamp() {
        let seed = [23_u8; 32];
        let kind = DistributionKind::Gaussian {
            mean: 0.0,
            std_dev: 1.0,
        };
        let a = ScheduleSource::Stochastic {
            kind: kind.clone(),
            seed,
            draw_count: 0,
            rng: ChaCha8Rng::from_seed(seed),
            clamp_min: Some(0.0),
            clamp_max: None,
        };
        let b = ScheduleSource::Stochastic {
            kind,
            seed,
            draw_count: 0,
            rng: ChaCha8Rng::from_seed(seed),
            clamp_min: None,
            clamp_max: None,
        };
        assert_ne!(a, b);
    }

    #[test]
    fn stochastic_state_reset_zeroes_draw_count() {
        let env = default_env();
        let mut src = stochastic(
            DistributionKind::Gaussian {
                mean: 0.0,
                std_dev: 1.0,
            },
            [24_u8; 32],
        );
        for _ in 0..5 {
            src.value_at(&env).unwrap();
        }
        src.reset();
        match &src {
            ScheduleSource::Stochastic { draw_count, .. } => {
                assert_eq!(*draw_count, 0, "reset should zero draw_count");
            }
            _ => panic!("expected Stochastic"),
        }
    }

    #[test]
    fn noisy_time_windows_reset_zeroes_draw_count() {
        let mut env = default_env();
        let utc = FixedOffset::east_opt(0).expect("offset");
        env.current_time = utc
            .with_ymd_and_hms(2026, 1, 5, 10, 0, 0)
            .single()
            .expect("valid timestamp");

        let mut source = ScheduleSource::noisy_time_windows(
            vec![TimeWindow::with_noise(
                DayFilter::Any,
                0,
                1440,
                10.0,
                DistributionKind::Gaussian {
                    mean: 0.0,
                    std_dev: 1.0,
                },
                None,
                None,
            )],
            None,
            [25_u8; 32],
        );
        for _ in 0..5 {
            source.value_at(&env).unwrap();
        }
        source.reset();
        match &source {
            ScheduleSource::TimeWindows { rng_state, .. } => {
                let st = rng_state.as_ref().unwrap();
                assert_eq!(st.draw_count, 0, "reset should zero draw_count");
            }
            _ => panic!("expected TimeWindows"),
        }
    }

    #[test]
    #[should_panic(expected = "no window has noise")]
    fn noisy_time_windows_panics_without_any_noise() {
        ScheduleSource::noisy_time_windows(
            vec![TimeWindow::new(DayFilter::Any, 0, 1440, 10.0)],
            None,
            [26_u8; 32],
        );
    }

    #[test]
    fn season_filter_contains_month_all() {
        for m in 1..=12 {
            assert!(
                SeasonFilter::All.contains_month(m),
                "All should match month {m}"
            );
        }
    }

    #[test]
    fn season_filter_contains_month_summer() {
        for m in 1..=12 {
            let expected = (6..=9).contains(&m);
            assert_eq!(
                SeasonFilter::Summer.contains_month(m),
                expected,
                "Summer mismatch for month {m}"
            );
        }
    }

    #[test]
    fn season_filter_contains_month_winter() {
        for m in 1..=12 {
            let expected = !(6..=9).contains(&m);
            assert_eq!(
                SeasonFilter::Winter.contains_month(m),
                expected,
                "Winter mismatch for month {m}"
            );
        }
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "month out of range")]
    fn season_filter_debug_asserts_invalid_month_zero() {
        SeasonFilter::All.contains_month(0);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "month out of range")]
    fn season_filter_debug_asserts_invalid_month_thirteen() {
        SeasonFilter::All.contains_month(13);
    }

    #[test]
    fn seasonal_split_is_summer_normal_range() {
        let split = SeasonalSplit::new(6, 9).unwrap();
        assert!(split.is_summer(6));
        assert!(split.is_summer(9));
        assert!(!split.is_summer(5));
        assert!(!split.is_summer(10));
    }

    #[test]
    fn seasonal_split_is_summer_wrapping() {
        let split = SeasonalSplit::new(11, 2).unwrap();
        assert!(split.is_summer(11));
        assert!(split.is_summer(12));
        assert!(split.is_summer(1));
        assert!(split.is_summer(2));
        assert!(!split.is_summer(3));
        assert!(!split.is_summer(10));
    }

    #[test]
    fn seasonal_split_rejects_invalid_months() {
        assert!(SeasonalSplit::new(0, 9).is_err());
        assert!(SeasonalSplit::new(6, 13).is_err());
        assert!(SeasonalSplit::new(0, 0).is_err());
    }

    #[test]
    #[cfg(not(debug_assertions))]
    fn season_filter_invalid_month_returns_false_in_release() {
        assert!(!SeasonFilter::All.contains_month(0));
        assert!(!SeasonFilter::All.contains_month(13));
        assert!(!SeasonFilter::Summer.contains_month(0));
        assert!(!SeasonFilter::Winter.contains_month(255));
    }

    #[test]
    #[cfg(not(debug_assertions))]
    fn seasonal_split_invalid_month_returns_false_in_release() {
        let split = SeasonalSplit::new(6, 9).unwrap();
        assert!(!split.is_summer(0));
        assert!(!split.is_summer(13));
    }

    #[test]
    fn billing_cycle_default_is_monthly() {
        assert_eq!(BillingCycle::default(), BillingCycle::Monthly);
    }

    #[test]
    fn billing_cycle_serde_roundtrip() {
        let monthly = BillingCycle::Monthly;
        let json = serde_json::to_string(&monthly).unwrap();
        let back: BillingCycle = serde_json::from_str(&json).unwrap();
        assert_eq!(back, monthly);

        let custom = BillingCycle::Custom(14);
        let json = serde_json::to_string(&custom).unwrap();
        let back: BillingCycle = serde_json::from_str(&json).unwrap();
        assert_eq!(back, custom);
    }

    #[test]
    fn tou_period_serde_roundtrip() {
        let period = TouPeriod {
            name: "on-peak".to_string(),
            schedule: vec![TimeWindow::new(DayFilter::Weekdays, 780, 1260, 0.25)],
            season: SeasonFilter::Summer,
        };
        let json = serde_json::to_string(&period).unwrap();
        let back: TouPeriod = serde_json::from_str(&json).unwrap();
        assert_eq!(back, period);
    }

    #[test]
    fn tou_period_validate_accepts_valid() {
        let period = TouPeriod {
            name: "on-peak".to_string(),
            schedule: vec![TimeWindow::new(DayFilter::Weekdays, 780, 1260, 0.25)],
            season: SeasonFilter::Summer,
        };
        assert!(period.validate().is_ok());

        let empty = TouPeriod {
            name: "empty".to_string(),
            schedule: vec![],
            season: SeasonFilter::All,
        };
        assert!(empty.validate().is_ok());
    }

    #[test]
    fn tou_period_validate_rejects_invalid_window() {
        let bad_start = TouPeriod {
            name: "bad".to_string(),
            schedule: vec![TimeWindow {
                day: DayFilter::Any,
                start_minute: 1440,
                end_minute: 1440,
                value: 0.0,
                noise: None,
                min_value: None,
                max_value: None,
            }],
            season: SeasonFilter::All,
        };
        assert!(bad_start.validate().is_err());

        let zero_width = TouPeriod {
            name: "zero".to_string(),
            schedule: vec![TimeWindow {
                day: DayFilter::Any,
                start_minute: 300,
                end_minute: 300,
                value: 0.0,
                noise: None,
                min_value: None,
                max_value: None,
            }],
            season: SeasonFilter::All,
        };
        assert!(zero_width.validate().is_err());
    }

    // ── ScheduleSource::mean() tests ────────────────────────────────

    #[test]
    fn mean_constant() {
        assert_eq!(ScheduleSource::Constant(42.0).mean(), 42.0);
        assert_eq!(ScheduleSource::Constant(0.0).mean(), 0.0);
    }

    #[test]
    fn mean_daily_profile() {
        let mut weekday = [0.0; 24];
        weekday.fill(1.0); // all hours = 1.0
        let weekend = [0.5; 24];
        let month = [1.0; 12];
        let source = ScheduleSource::DailyProfile {
            weekday,
            weekend,
            month_multipliers: month,
            max_value: 10.0,
        };
        // wd_avg = 1.0, we_avg = 0.5, day_avg = (5*1.0 + 2*0.5)/7 ≈ 0.857
        // month_avg = 1.0, max = 10.0 → ~8.571
        let expected = (5.0 * 1.0 + 2.0 * 0.5) / 7.0 * 10.0;
        assert!((source.mean() - expected).abs() < 1e-10);
    }

    #[test]
    fn mean_stochastic_gaussian() {
        let source = ScheduleSource::Stochastic {
            kind: DistributionKind::Gaussian {
                mean: 30.0,
                std_dev: 5.0,
            },
            seed: [0; 32],
            draw_count: 0,
            rng: ChaCha8Rng::from_seed([0; 32]),
            clamp_min: None,
            clamp_max: None,
        };
        assert_eq!(source.mean(), 30.0);
    }

    #[test]
    fn mean_stochastic_clamped() {
        let source = ScheduleSource::Stochastic {
            kind: DistributionKind::Gaussian {
                mean: 100.0,
                std_dev: 10.0,
            },
            seed: [0; 32],
            draw_count: 0,
            rng: ChaCha8Rng::from_seed([0; 32]),
            clamp_min: None,
            clamp_max: Some(50.0),
        };
        assert_eq!(source.mean(), 50.0);
    }

    #[test]
    fn mean_shared() {
        let data: Arc<[f64]> = Arc::from(vec![10.0, 20.0, 30.0]);
        let source = ScheduleSource::Shared {
            data,
            cursor: 0,
            boundary: BoundaryPolicy::Clamp,
        };
        assert!((source.mean() - 20.0).abs() < 1e-10);
    }

    #[test]
    fn mean_shared_empty() {
        let data: Arc<[f64]> = Arc::from(vec![]);
        let source = ScheduleSource::Shared {
            data,
            cursor: 0,
            boundary: BoundaryPolicy::Clamp,
        };
        assert_eq!(source.mean(), 0.0);
    }

    #[test]
    fn mean_time_windows() {
        let source = ScheduleSource::TimeWindows {
            windows: vec![
                TimeWindow::new(DayFilter::Weekdays, 0, 1440, 21.0),
                TimeWindow::new(DayFilter::Weekends, 0, 1440, 24.0),
            ],
            default: None,
            rng_state: None,
        };
        assert!((source.mean() - 22.5).abs() < 1e-10);
    }

    #[test]
    fn mean_time_windows_empty_with_default() {
        let source = ScheduleSource::TimeWindows {
            windows: vec![],
            default: Some(15.0),
            rng_state: None,
        };
        assert_eq!(source.mean(), 15.0);
    }

    #[test]
    fn mean_time_windows_with_noise_includes_noise_analytical_mean() {
        // When a window carries a noise distribution (e.g. log-normal),
        // mean() must include the distribution's analytical mean, not just
        // the window's value field. This is the pattern used by NoisyTimeWindows
        // archetypes (WfhOccasional, WeekendWarrior) where value=0.0 and the
        // full distribution mean lives in the noise DistributionKind.
        let lognorm_mean = (2.27_f64 + 0.65_f64 * 0.65_f64 / 2.0_f64).exp(); // ≈ 11.95
        let w1 = TimeWindow::with_noise(
            DayFilter::Weekdays,
            0,
            1440,
            0.0,
            DistributionKind::LogNormal {
                mu: 2.27,
                sigma: 0.65,
            },
            None,
            None,
        );
        let w2 = TimeWindow::with_noise(
            DayFilter::Weekends,
            0,
            1440,
            0.0,
            DistributionKind::LogNormal {
                mu: 3.01,
                sigma: 0.65,
            },
            None,
            None,
        );
        let w2_mean = (3.01_f64 + 0.65_f64 * 0.65_f64 / 2.0_f64).exp(); // ≈ 25.06
        let source = ScheduleSource::TimeWindows {
            windows: vec![w1, w2],
            default: None,
            rng_state: None,
        };
        let expected = (lognorm_mean + w2_mean) / 2.0;
        let got = source.mean();
        assert!(
            (got - expected).abs() / expected < 1e-10,
            "TimeWindows mean with log-normal noise: got {got}, expected {expected}"
        );
        assert!(got > 0.0, "mean should be non-zero with log-normal noise");
    }

    #[test]
    fn mean_time_windows_mixed_noise_and_noise_free() {
        // Window with value=42.0 (no noise) + window with value=0.0
        // and noise mean=30.0 → combined mean = (42.0 + 30.0) / 2 = 36.0
        let noisy = TimeWindow::with_noise(
            DayFilter::Weekdays,
            0,
            1440,
            0.0,
            DistributionKind::Gaussian {
                mean: 30.0,
                std_dev: 5.0,
            },
            None,
            None,
        );
        let plain = TimeWindow::new(DayFilter::Weekends, 0, 1440, 42.0);
        let source = ScheduleSource::TimeWindows {
            windows: vec![noisy, plain],
            default: None,
            rng_state: None,
        };
        let got = source.mean();
        assert!(
            (got - 36.0).abs() < 1e-10,
            "mixed noise/noiseless mean: got {got}, expected 36.0"
        );
    }

    // ── DistributionKind::mean() tests ──────────────────────────────

    #[test]
    fn distribution_mean_uniform() {
        let d = DistributionKind::Uniform {
            low: 10.0,
            high: 20.0,
        };
        assert_eq!(d.mean(), 15.0);
    }

    #[test]
    fn distribution_mean_exponential() {
        let d = DistributionKind::Exponential { lambda: 0.5 };
        assert_eq!(d.mean(), 2.0);
    }

    #[test]
    fn distribution_mean_bernoulli() {
        let d = DistributionKind::Bernoulli { p: 0.3 };
        assert!((d.mean() - 0.3).abs() < 1e-10);
    }

    #[test]
    fn distribution_mean_poisson() {
        let d = DistributionKind::Poisson { lambda: 4.5 };
        assert_eq!(d.mean(), 4.5);
    }

    #[test]
    fn distribution_mean_lognormal() {
        let d = DistributionKind::LogNormal {
            mu: 1.0,
            sigma: 0.5,
        };
        // E[X] = exp(mu + sigma²/2) = exp(1.0 + 0.125) = exp(1.125)
        assert!((d.mean() - 1.125_f64.exp()).abs() < 1e-10);
    }
}
