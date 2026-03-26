//! N-dimensional regular grid interpolator.
//!
//! Rust port of `scipy.interpolate.RegularGridInterpolator` with
//! `method="linear"` and `bounds_error=False, fill_value=None` (clamp).
//!
//! Used by [`ChargingCurveLut`](crate::ev::ChargingCurveLut) to interpolate
//! 4-D CC-CV charging curves (SOC × temperature × C-rate × SOH → power fraction).

use hares_types::HaresError;
use serde::{Deserialize, Serialize};

/// A regular grid interpolator over N dimensions.
///
/// Each axis is a strictly ascending `Vec<f64>`.  The values tensor is stored
/// in row-major (C) order with shape `[n0, n1, …, n_{N-1}]`.
///
/// At query time, each coordinate is clamped to the axis bounds (nearest-
/// neighbour extrapolation), then multilinear interpolation is performed
/// across the 2^N enclosing grid vertices.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegularGridInterpolator {
    /// One axis per dimension, each strictly ascending.
    axes: Vec<Vec<f64>>,
    /// Flattened row-major values tensor.  Length = product of axis lengths.
    values: Vec<f32>,
    /// Cumulative strides for row-major indexing: `strides[i] = ∏ axes[j].len() for j > i`.
    strides: Vec<usize>,
}

impl RegularGridInterpolator {
    /// Construct a validated interpolator.
    ///
    /// # Arguments
    /// * `axes` – One `Vec<f64>` per dimension, each strictly ascending, non-empty.
    /// * `values` – Flattened row-major tensor.  Length must equal the product of axis lengths.
    ///
    /// # Errors
    /// Returns `HaresError::Equipment` if any axis is empty, not strictly ascending,
    /// contains non-finite values, or the values length mismatches.
    pub fn new(axes: Vec<Vec<f64>>, values: Vec<f32>) -> crate::Result<Self> {
        if axes.is_empty() {
            return Err(HaresError::Equipment(
                "RegularGridInterpolator requires at least one axis".to_string(),
            ));
        }

        let mut total_cells: usize = 1;
        for (dim, axis) in axes.iter().enumerate() {
            if axis.is_empty() {
                return Err(HaresError::Equipment(format!(
                    "RegularGridInterpolator axis {dim} is empty"
                )));
            }
            for (i, v) in axis.iter().enumerate() {
                if !v.is_finite() {
                    return Err(HaresError::Equipment(format!(
                        "RegularGridInterpolator axis {dim}[{i}] is not finite: {v}"
                    )));
                }
                if i > 0 && *v <= axis[i - 1] {
                    return Err(HaresError::Equipment(format!(
                        "RegularGridInterpolator axis {dim} must be strictly ascending; \
                         [{i}]={v} <= [{}]={}",
                        i - 1,
                        axis[i - 1]
                    )));
                }
            }
            total_cells = total_cells.checked_mul(axis.len()).ok_or_else(|| {
                HaresError::Equipment("RegularGridInterpolator grid size overflow".to_string())
            })?;
        }

        if values.len() != total_cells {
            return Err(HaresError::Equipment(format!(
                "RegularGridInterpolator values length {} != expected {} (product of axis lengths)",
                values.len(),
                total_cells,
            )));
        }

        for (i, v) in values.iter().enumerate() {
            if !v.is_finite() {
                return Err(HaresError::Equipment(format!(
                    "RegularGridInterpolator values[{i}] is not finite: {v}"
                )));
            }
        }

        // Compute row-major strides.
        let ndim = axes.len();
        let mut strides = vec![1usize; ndim];
        for i in (0..ndim - 1).rev() {
            strides[i] = strides[i + 1] * axes[i + 1].len();
        }

        Ok(Self {
            axes,
            values,
            strides,
        })
    }

    /// Number of dimensions.
    #[inline]
    pub fn ndim(&self) -> usize {
        self.axes.len()
    }

    /// Interpolate at a single point.
    ///
    /// `point` must have exactly `ndim()` elements.  Each coordinate is clamped
    /// to the corresponding axis bounds before interpolation.
    ///
    /// # Panics
    /// Panics if `point.len() != self.ndim()`.
    pub fn interpolate(&self, point: &[f64]) -> f32 {
        assert_eq!(
            point.len(),
            self.axes.len(),
            "interpolate: expected {} coordinates, got {}",
            self.axes.len(),
            point.len()
        );

        let ndim = self.axes.len();

        // For each dimension, find the lower bracket index and the fractional position.
        // Stack-allocate for up to 8 dimensions; heap otherwise.
        let mut lo_indices = [0usize; 8];
        let mut fracs = [0.0f64; 8];
        debug_assert!(ndim <= 8, "ndim > 8 not supported in stack path");

        for (dim, axis) in self.axes.iter().enumerate() {
            let x = point[dim].clamp(axis[0], axis[axis.len() - 1]);
            let (lo, frac) = bracket(axis, x);
            lo_indices[dim] = lo;
            fracs[dim] = frac;
        }

        // Multilinear interpolation: iterate over 2^ndim corners.
        let n_corners = 1u32 << ndim;
        let mut result = 0.0f64;

        for corner in 0..n_corners {
            let mut weight = 1.0f64;
            let mut flat_idx = 0usize;
            for dim in 0..ndim {
                let bit = (corner >> dim) & 1;
                let idx = lo_indices[dim] + bit as usize;
                // Clamp to axis max index (handles single-point axes and boundary).
                let idx = idx.min(self.axes[dim].len() - 1);
                flat_idx += idx * self.strides[dim];
                weight *= if bit == 0 {
                    1.0 - fracs[dim]
                } else {
                    fracs[dim]
                };
            }
            result += weight * self.values[flat_idx] as f64;
        }

        result as f32
    }
}

/// Find the lower bracket index and fractional position of `x` in a sorted axis.
///
/// Returns `(lo, frac)` where `axis[lo] <= x <= axis[lo+1]` and
/// `frac = (x - axis[lo]) / (axis[lo+1] - axis[lo])`.
///
/// If `x` is at or beyond the last point, returns `(len-2, 1.0)` so that
/// interpolation yields the last value.  For single-element axes, returns `(0, 0.0)`.
#[inline]
fn bracket(axis: &[f64], x: f64) -> (usize, f64) {
    let n = axis.len();
    if n == 1 {
        return (0, 0.0);
    }
    // Binary search for the interval containing x.
    // `partition_point` gives us the first index where axis[i] > x.
    let pos = axis.partition_point(|&v| v <= x);
    let lo = if pos == 0 {
        0
    } else if pos >= n {
        n - 2
    } else {
        pos - 1
    };
    let span = axis[lo + 1] - axis[lo];
    let frac = if span.abs() < f64::EPSILON {
        0.0
    } else {
        ((x - axis[lo]) / span).clamp(0.0, 1.0)
    };
    (lo, frac)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interp_1d_linear() {
        let interp = RegularGridInterpolator::new(vec![vec![0.0, 1.0]], vec![0.0, 10.0]).unwrap();
        assert!((interp.interpolate(&[0.0]) - 0.0).abs() < 1e-5);
        assert!((interp.interpolate(&[0.5]) - 5.0).abs() < 1e-5);
        assert!((interp.interpolate(&[1.0]) - 10.0).abs() < 1e-5);
    }

    #[test]
    fn interp_1d_clamp() {
        let interp = RegularGridInterpolator::new(vec![vec![0.0, 1.0]], vec![2.0, 8.0]).unwrap();
        // Below lower bound → clamp to first value.
        assert!((interp.interpolate(&[-1.0]) - 2.0).abs() < 1e-5);
        // Above upper bound → clamp to last value.
        assert!((interp.interpolate(&[2.0]) - 8.0).abs() < 1e-5);
    }

    #[test]
    fn interp_2d_bilinear() {
        // f(x, y) = x + y on [0,1] × [0,1]
        let interp = RegularGridInterpolator::new(
            vec![vec![0.0, 1.0], vec![0.0, 1.0]],
            // Row-major: (0,0)=0, (0,1)=1, (1,0)=1, (1,1)=2
            vec![0.0, 1.0, 1.0, 2.0],
        )
        .unwrap();
        assert!((interp.interpolate(&[0.5, 0.5]) - 1.0).abs() < 1e-5);
        assert!((interp.interpolate(&[0.0, 0.0]) - 0.0).abs() < 1e-5);
        assert!((interp.interpolate(&[1.0, 1.0]) - 2.0).abs() < 1e-5);
        assert!((interp.interpolate(&[0.25, 0.75]) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn interp_4d_constant() {
        // All values = 0.5; should always return 0.5.
        let axes = vec![
            vec![0.0, 0.5, 1.0],
            vec![-10.0, 25.0, 45.0],
            vec![0.1, 1.0, 2.0],
            vec![0.7, 1.0],
        ];
        let total: usize = axes.iter().map(|a| a.len()).product();
        let values = vec![0.5f32; total];
        let interp = RegularGridInterpolator::new(axes, values).unwrap();
        assert!((interp.interpolate(&[0.3, 15.0, 0.5, 0.85]) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn interp_4d_matches_scipy_convention() {
        // 4D grid: shape (2, 2, 2, 2) = 16 values.
        // f(s, t, c, h) = s (only first axis varies).
        let axes = vec![
            vec![0.0, 1.0],
            vec![0.0, 1.0],
            vec![0.0, 1.0],
            vec![0.0, 1.0],
        ];
        let mut values = vec![0.0f32; 16];
        // Row-major: index = s*8 + t*4 + c*2 + h
        // When s=1, all values = 1.0.
        for t in 0..2 {
            for c in 0..2 {
                for h in 0..2 {
                    values[1 * 8 + t * 4 + c * 2 + h] = 1.0;
                }
            }
        }
        let interp = RegularGridInterpolator::new(axes, values).unwrap();
        assert!((interp.interpolate(&[0.0, 0.5, 0.5, 0.5]) - 0.0).abs() < 1e-5);
        assert!((interp.interpolate(&[1.0, 0.5, 0.5, 0.5]) - 1.0).abs() < 1e-5);
        assert!((interp.interpolate(&[0.5, 0.5, 0.5, 0.5]) - 0.5).abs() < 1e-5);
        assert!((interp.interpolate(&[0.25, 0.0, 0.0, 0.0]) - 0.25).abs() < 1e-5);
    }

    #[test]
    fn interp_3_point_axis() {
        // 1D with 3 points: [0, 0.5, 1.0] → [0, 1, 0]
        let interp =
            RegularGridInterpolator::new(vec![vec![0.0, 0.5, 1.0]], vec![0.0, 1.0, 0.0]).unwrap();
        assert!((interp.interpolate(&[0.25]) - 0.5).abs() < 1e-5);
        assert!((interp.interpolate(&[0.5]) - 1.0).abs() < 1e-5);
        assert!((interp.interpolate(&[0.75]) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn rejects_empty_axis() {
        let err = RegularGridInterpolator::new(vec![vec![]], vec![]).unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn rejects_non_ascending_axis() {
        let err = RegularGridInterpolator::new(vec![vec![1.0, 0.5]], vec![1.0, 2.0]).unwrap_err();
        assert!(err.to_string().contains("ascending"));
    }

    #[test]
    fn rejects_values_length_mismatch() {
        let err = RegularGridInterpolator::new(
            vec![vec![0.0, 1.0], vec![0.0, 1.0]],
            vec![1.0, 2.0, 3.0], // needs 4
        )
        .unwrap_err();
        assert!(err.to_string().contains("length"));
    }

    #[test]
    fn rejects_nan_in_axis() {
        let err =
            RegularGridInterpolator::new(vec![vec![0.0, f64::NAN]], vec![1.0, 2.0]).unwrap_err();
        assert!(err.to_string().contains("not finite"));
    }

    #[test]
    fn rejects_nan_in_values() {
        let err =
            RegularGridInterpolator::new(vec![vec![0.0, 1.0]], vec![1.0, f32::NAN]).unwrap_err();
        assert!(err.to_string().contains("not finite"));
    }

    #[test]
    fn single_point_axis() {
        // Single-point axis: always returns that value.
        let interp = RegularGridInterpolator::new(vec![vec![0.5]], vec![3.14]).unwrap();
        assert!((interp.interpolate(&[0.0]) - 3.14).abs() < 1e-5);
        assert!((interp.interpolate(&[0.5]) - 3.14).abs() < 1e-5);
        assert!((interp.interpolate(&[1.0]) - 3.14).abs() < 1e-5);
    }
}
