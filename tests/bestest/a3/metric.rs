//! The A3 time-series comparison kernel: the two metrics Normative
//! Annex A3's time-series ranges are defined on.
//!
//! The addendum (A3.1): "comparison metrics (Pearson correlation
//! coefficient [for curve shape] and root mean square error [for value
//! scaling]) for the software results are calculated versus the
//! representative curve results"; a result is within a range when the
//! Pearson coefficient is above the table's limit and the RMSE is below
//! it. Both metrics are computed over the same paired samples: the 24
//! hourly zone loads of one date (kWh, hour-ending labels), the
//! software's day against the representative curve's day.

/// A kernel refusal: the metric is undefined for the inputs, and the
/// caller must not read a number off a failed comparison.
#[derive(Debug, Clone, PartialEq)]
pub enum MetricError {
    /// The two sample vectors differ in length: the metrics are defined
    /// over paired samples.
    LengthMismatch { x_len: usize, y_len: usize },
    /// No samples.
    Empty,
    /// One sample vector is constant, so its deviations from the mean
    /// are all zero and the Pearson coefficient's denominator is zero.
    ZeroVariance { side: ZeroVarianceSide },
    /// A sample is not a finite number; no metric may silently consume
    /// it.
    NonFinite { index: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZeroVarianceSide {
    X,
    Y,
    Both,
}

impl std::fmt::Display for MetricError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LengthMismatch { x_len, y_len } => {
                write!(f, "sample lengths differ: {x_len} vs {y_len}")
            }
            Self::Empty => write!(f, "no samples"),
            Self::ZeroVariance { side } => write!(
                f,
                "zero-variance sample ({side:?}): the Pearson coefficient is undefined"
            ),
            Self::NonFinite { index } => {
                write!(f, "non-finite sample at index {index}")
            }
        }
    }
}

impl std::error::Error for MetricError {}

fn check_samples(x: &[f64], y: &[f64]) -> Result<usize, MetricError> {
    if x.len() != y.len() {
        return Err(MetricError::LengthMismatch {
            x_len: x.len(),
            y_len: y.len(),
        });
    }
    if x.is_empty() {
        return Err(MetricError::Empty);
    }
    for (i, (a, b)) in x.iter().zip(y.iter()).enumerate() {
        if !a.is_finite() || !b.is_finite() {
            return Err(MetricError::NonFinite { index: i });
        }
    }
    Ok(x.len())
}

/// The Pearson product-moment correlation coefficient of two paired
/// sample vectors, r = sxy / sqrt(sxx * syy) over the deviations from
/// the means. The coefficient is invariant to the variance denominator
/// (the population and sample forms cancel), so the kernel uses the
/// population form throughout. A constant sample vector makes the
/// denominator zero and the coefficient undefined: the kernel refuses.
pub fn pearson(x: &[f64], y: &[f64]) -> Result<f64, MetricError> {
    let n = check_samples(x, y)?;
    let mean_x = x.iter().sum::<f64>() / n as f64;
    let mean_y = y.iter().sum::<f64>() / n as f64;
    let mut sxy = 0.0;
    let mut sxx = 0.0;
    let mut syy = 0.0;
    for (a, b) in x.iter().zip(y.iter()) {
        let dx = a - mean_x;
        let dy = b - mean_y;
        sxy += dx * dy;
        sxx += dx * dx;
        syy += dy * dy;
    }
    // A bitwise-constant vector has zero variance mathematically, but its
    // computed mean can round away from the repeated value (for example,
    // [0.1, 0.1, 0.1]'s mean is not the literal 0.1), leaving tiny nonzero
    // deviations. Detect the exact constant directly so the refusal does
    // not depend on the mean being representable.
    let constant_x = x.iter().all(|value| *value == x[0]);
    let constant_y = y.iter().all(|value| *value == y[0]);
    let side = match (constant_x || sxx == 0.0, constant_y || syy == 0.0) {
        (true, true) => ZeroVarianceSide::Both,
        (true, false) => ZeroVarianceSide::X,
        (false, true) => ZeroVarianceSide::Y,
        (false, false) => return Ok(sxy / (sxx * syy).sqrt()),
    };
    Err(MetricError::ZeroVariance { side })
}

/// The root mean square error of two paired sample vectors,
/// sqrt((1/N) * sum((x_i - y_i)^2)), in the samples' own unit (the
/// hourly loads' kWh).
pub fn rmse(x: &[f64], y: &[f64]) -> Result<f64, MetricError> {
    let n = check_samples(x, y)?;
    let total: f64 = x.iter().zip(y.iter()).map(|(a, b)| (a - b) * (a - b)).sum();
    Ok((total / n as f64).sqrt())
}

#[cfg(test)]
mod tests {
    use super::{MetricError, ZeroVarianceSide, pearson, rmse};

    // The hand computations the assertions carry, all on small integers
    // where every intermediate (the means, the deviations, the squared
    // and summed deviations) is exactly representable in f64, so the
    // sums sxx, syy, sxy are exact and the only rounding step is the
    // one sqrt. The pinned expectations are the same IEEE 754 sqrt on
    // the same exact rational, so exact equality is the correctly
    // rounded value, not an epsilon agreement.

    #[test]
    fn pearson_perfect_positive_shape_is_one() {
        // x = [1,2,3], y = [2,4,6]: dx = [-1,0,1], dy = [-2,0,2];
        // sxy = 2+0+2 = 4, sxx = 2, syy = 8; r = 4/sqrt(16) = 1.
        let r = pearson(&[1.0, 2.0, 3.0], &[2.0, 4.0, 6.0]).unwrap();
        assert_eq!(r, 1.0);
    }

    #[test]
    fn pearson_perfect_negative_shape_is_minus_one() {
        // x = [1,2,3], y = [6,4,2]: dy = [2,0,-2]; sxy = -4, sxx = 2,
        // syy = 8; r = -4/sqrt(16) = -1.
        let r = pearson(&[1.0, 2.0, 3.0], &[6.0, 4.0, 2.0]).unwrap();
        assert_eq!(r, -1.0);
    }

    #[test]
    fn pearson_and_rmse_match_the_hand_computed_fraction() {
        // x = [1,2,3,4], y = [2,3,1,4]: means 2.5, 2.5;
        // dx = [-1.5,-0.5,0.5,1.5], dy = [-0.5,0.5,-1.5,1.5];
        // sxy = 0.75-0.25-0.75+2.25 = 2, sxx = syy = 5;
        // r = 2/sqrt(25) = 0.4 exactly.
        let x = [1.0, 2.0, 3.0, 4.0];
        let y = [2.0, 3.0, 1.0, 4.0];
        let r = pearson(&x, &y).unwrap();
        assert_eq!(r, 0.4);
        // Squared errors 1+1+4+0 = 6 over 4 pairs: RMSE = sqrt(6/4).
        let e = rmse(&x, &y).unwrap();
        assert_eq!(e, (6.0_f64 / 4.0).sqrt());
    }

    #[test]
    fn pearson_is_shape_only_and_rmse_carries_the_scaling() {
        // The addendum's own framing: the Pearson coefficient for curve
        // shape, the RMSE for value scaling. y = 100x is the same shape
        // (r = 1, the coefficient is scale- and offset-invariant) while
        // the RMSE scales with the values: x = [1,2,3] against
        // [100,200,300] carries squared errors 9801 + 39204 + 88209 =
        // 137214 over 3 pairs, RMSE = sqrt(137214/3).
        let x = [1.0, 2.0, 3.0];
        let y = [100.0, 200.0, 300.0];
        let r = pearson(&x, &y).unwrap();
        assert_eq!(r, 1.0);
        let e = rmse(&x, &y).unwrap();
        assert_eq!(e, (137214.0_f64 / 3.0).sqrt());
        assert!(e > 100.0, "the scaling error is in the values' unit");
    }

    #[test]
    fn pearson_refuses_zero_variance_samples() {
        // A constant vector has zero deviation from its mean: the
        // denominator sxx*syy is zero and r is undefined.
        assert_eq!(
            pearson(&[5.0, 5.0, 5.0], &[1.0, 2.0, 3.0]).unwrap_err(),
            MetricError::ZeroVariance {
                side: ZeroVarianceSide::X
            }
        );
        assert_eq!(
            pearson(&[1.0, 2.0, 3.0], &[7.0, 7.0, 7.0]).unwrap_err(),
            MetricError::ZeroVariance {
                side: ZeroVarianceSide::Y
            }
        );
        assert_eq!(
            pearson(&[5.0, 5.0], &[7.0, 7.0]).unwrap_err(),
            MetricError::ZeroVariance {
                side: ZeroVarianceSide::Both
            }
        );
    }

    #[test]
    fn pearson_refuses_bitwise_constant_samples_whose_mean_rounds() {
        // 0.1's f64 mean over three copies is not the literal 0.1, so the
        // summed deviations are tiny but nonzero; the sample is still
        // constant, so the coefficient is undefined and the kernel must
        // refuse rather than return a number.
        assert_eq!(
            pearson(&[0.1, 0.1, 0.1], &[1.0, 2.0, 3.0]).unwrap_err(),
            MetricError::ZeroVariance {
                side: ZeroVarianceSide::X
            }
        );
        assert_eq!(
            pearson(&[1.0, 2.0, 3.0], &[-0.1, -0.1, -0.1]).unwrap_err(),
            MetricError::ZeroVariance {
                side: ZeroVarianceSide::Y
            }
        );
        assert_eq!(
            pearson(&[0.1, 0.1, 0.1], &[0.1, 0.1, 0.1]).unwrap_err(),
            MetricError::ZeroVariance {
                side: ZeroVarianceSide::Both
            }
        );
    }

    #[test]
    fn both_kernels_refuse_length_mismatch() {
        let x = [1.0, 2.0, 3.0];
        let y = [1.0, 2.0];
        assert_eq!(
            pearson(&x, &y).unwrap_err(),
            MetricError::LengthMismatch { x_len: 3, y_len: 2 }
        );
        assert_eq!(
            rmse(&y, &x).unwrap_err(),
            MetricError::LengthMismatch { x_len: 2, y_len: 3 }
        );
    }

    #[test]
    fn both_kernels_refuse_empty_samples() {
        assert_eq!(pearson(&[], &[]).unwrap_err(), MetricError::Empty);
        assert_eq!(rmse(&[], &[]).unwrap_err(), MetricError::Empty);
    }

    #[test]
    fn both_kernels_refuse_non_finite_samples() {
        // NaN and infinity are not loads; no metric may read a number
        // off a pair that carries one.
        assert_eq!(
            pearson(&[1.0, f64::NAN, 3.0], &[1.0, 2.0, 3.0]).unwrap_err(),
            MetricError::NonFinite { index: 1 }
        );
        assert_eq!(
            rmse(&[1.0, 2.0, 3.0], &[1.0, f64::INFINITY, 3.0]).unwrap_err(),
            MetricError::NonFinite { index: 1 }
        );
    }

    #[test]
    fn rmse_is_zero_only_for_identical_samples() {
        let x = [0.5, 1.25, -3.0, 1.0e6];
        assert_eq!(rmse(&x, &x).unwrap(), 0.0);
        assert!(rmse(&x, &[0.5, 1.25, -3.0, 1.0e5]).unwrap() > 0.0);
    }
}
