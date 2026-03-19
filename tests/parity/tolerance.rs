use std::fmt;

pub const ZONE_TEMP_CONDITIONED_C_MAE_MAX: f64 = 0.1;
pub const ZONE_TEMP_UNCONDITIONED_C_MAE_MAX: f64 = 0.5;
pub const ANNUAL_HVAC_ENERGY_REL_PCT_MAX: f64 = 1.0;
pub const ANNUAL_WATER_HEATER_ENERGY_REL_PCT_MAX: f64 = 0.5;
pub const ANNUAL_TOTAL_SITE_ENERGY_REL_PCT_MAX: f64 = 1.0;
pub const PEAK_HVAC_POWER_REL_PCT_MAX: f64 = 2.0;
pub const BATTERY_SOC_MAE_ABS_MAX: f64 = 0.01;
pub const EQUIPMENT_MODE_CYCLE_COUNT_REL_PCT_MAX: f64 = 5.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ToleranceBand {
    AbsoluteMae { max_abs: f64 },
    RelativePercent { max_percent: f64 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct MetricCheck {
    pub metric: &'static str,
    pub band: ToleranceBand,
    pub actual: f64,
    pub allowed: f64,
    pub passed: bool,
}

impl MetricCheck {
    #[must_use]
    pub fn summary_line(&self) -> String {
        let status = if self.passed { "PASS" } else { "FAIL" };
        format!(
            "{status} {:<34} deviation={:.6} allowed={:.6} ({})",
            self.metric, self.actual, self.allowed, self.band
        )
    }
}

impl fmt::Display for ToleranceBand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AbsoluteMae { max_abs } => write!(f, "MAE absolute <= {max_abs:.6}"),
            Self::RelativePercent { max_percent } => {
                write!(f, "relative <= {max_percent:.6}%")
            }
        }
    }
}

#[must_use]
pub fn check_absolute_mae(
    metric: &'static str,
    actual: &[f64],
    reference: &[f64],
    max_abs: f64,
) -> MetricCheck {
    let mae = mae(actual, reference);
    MetricCheck {
        metric,
        band: ToleranceBand::AbsoluteMae { max_abs },
        actual: mae,
        allowed: max_abs,
        passed: mae <= max_abs,
    }
}

#[must_use]
pub fn check_relative_percent(
    metric: &'static str,
    actual: f64,
    reference: f64,
    max_percent: f64,
) -> MetricCheck {
    let deviation_pct = relative_percent_deviation(actual, reference);
    MetricCheck {
        metric,
        band: ToleranceBand::RelativePercent { max_percent },
        actual: deviation_pct,
        allowed: max_percent,
        passed: deviation_pct <= max_percent,
    }
}

#[must_use]
pub fn mae(actual: &[f64], reference: &[f64]) -> f64 {
    if actual.is_empty() || reference.is_empty() {
        return f64::INFINITY;
    }

    let n = actual.len().min(reference.len());
    if n == 0 {
        return f64::INFINITY;
    }

    let sum_abs = actual
        .iter()
        .zip(reference.iter())
        .take(n)
        .map(|(a, r)| (a - r).abs())
        .sum::<f64>();
    sum_abs / n as f64
}

#[must_use]
pub fn relative_percent_deviation(actual: f64, reference: f64) -> f64 {
    let denom = reference.abs();
    if denom <= f64::EPSILON {
        if actual.abs() <= f64::EPSILON {
            0.0
        } else {
            f64::INFINITY
        }
    } else {
        ((actual - reference).abs() / denom) * 100.0
    }
}
