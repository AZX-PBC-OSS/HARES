//! N-dimensional regular grid interpolator.
//!
//! Rust port of `scipy.interpolate.RegularGridInterpolator` with
//! `method="linear"` and configurable extrapolation strategy.
//!
//! Used by Battery and EV equipment to interpolate 4-D CC-CV charging curves
//! (SOC × temperature × C-rate × SOH → power fraction).

#[cfg(feature = "observe")]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicBool, Ordering};

use hares_types::HaresError;
use serde::{Deserialize, Serialize};
use tracing;

/// Strategy for out-of-bounds coordinates during interpolation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExtrapolationStrategy {
    /// Clamp each OOB coordinate to the axis bounds, then interpolate normally.
    Clamp,
    /// Return NaN if any coordinate is out of bounds.
    NaN,
    /// Extrapolate linearly using the edge-segment slope.
    /// Fractional positions outside [0,1] are allowed, producing weights <0 or >1.
    ///
    /// # Warning
    /// Linear extrapolation can produce physically nonsensical values (negative
    /// or unrealistically large results) for efficiency curves such as COP or
    /// capacity maps. Use only when the extrapolated function is approximately
    /// linear near the boundary (e.g., OCV tables near 0% or 100% SOC) or for
    /// controlled experiments and verification.
    Linear,
    /// Snap each OOB coordinate to the nearest axis endpoint (equivalent to Clamp).
    NearestNeighbor,
}

/// A regular grid interpolator over N dimensions.
///
/// Each axis is a strictly ascending `Vec<f64>`.  The values tensor is stored
/// in row-major (C) order with shape `[n0, n1, …, n_{N-1}]`.
///
/// At query time, the `strategy` determines how out-of-bounds coordinates
/// are handled.
#[derive(Debug, Serialize, Deserialize)]
pub struct RegularGridInterpolator {
    /// One axis per dimension, each strictly ascending.
    axes: Vec<Vec<f64>>,
    /// Flattened row-major values tensor.  Length = product of axis lengths.
    values: Vec<f32>,
    /// Cumulative strides for row-major indexing: `strides[i] = ∏ axes[j].len() for j > i`.
    strides: Vec<usize>,
    /// Extrapolation strategy for out-of-bounds coordinates.
    strategy: ExtrapolationStrategy,
    /// Throttle linear-extrapolation warnings to once per interpolator lifetime.
    #[serde(skip)]
    linear_extrap_warned: AtomicBool,
    /// Last-known lower bracket index per axis, used to accelerate consecutive
    /// queries via linear hunting instead of binary search.
    #[serde(skip)]
    cached_bracket: Vec<usize>,
    /// Count of out-of-bounds coordinate occurrences (only when feature "observe" is active).
    #[cfg(feature = "observe")]
    #[serde(skip)]
    pub oob_count: AtomicU64,
    /// Per-dimension count of linear-extrapolation events (only when feature "observe" is active).
    #[cfg(feature = "observe")]
    #[serde(skip)]
    pub linear_extrap_count: [AtomicU64; 8],
    /// Per-dimension cumulative count of linear hunt steps taken (only when feature "observe" is active).
    #[cfg(feature = "observe")]
    #[serde(skip)]
    pub hunt_steps: [AtomicU64; 8],
    /// Per-dimension count of binary-search fallback events (only when feature "observe" is active).
    #[cfg(feature = "observe")]
    #[serde(skip)]
    pub binary_fallback_count: [AtomicU64; 8],
}

impl Clone for RegularGridInterpolator {
    fn clone(&self) -> Self {
        Self {
            axes: self.axes.clone(),
            values: self.values.clone(),
            strides: self.strides.clone(),
            strategy: self.strategy,
            linear_extrap_warned: AtomicBool::new(
                self.linear_extrap_warned.load(Ordering::Relaxed),
            ),
            // Reset cache on clone: clones from serde start cold.
            cached_bracket: vec![0usize; self.axes.len()],
            #[cfg(feature = "observe")]
            oob_count: AtomicU64::new(self.oob_count.load(Ordering::Relaxed)),
            #[cfg(feature = "observe")]
            linear_extrap_count: [
                AtomicU64::new(self.linear_extrap_count[0].load(Ordering::Relaxed)),
                AtomicU64::new(self.linear_extrap_count[1].load(Ordering::Relaxed)),
                AtomicU64::new(self.linear_extrap_count[2].load(Ordering::Relaxed)),
                AtomicU64::new(self.linear_extrap_count[3].load(Ordering::Relaxed)),
                AtomicU64::new(self.linear_extrap_count[4].load(Ordering::Relaxed)),
                AtomicU64::new(self.linear_extrap_count[5].load(Ordering::Relaxed)),
                AtomicU64::new(self.linear_extrap_count[6].load(Ordering::Relaxed)),
                AtomicU64::new(self.linear_extrap_count[7].load(Ordering::Relaxed)),
            ],
            #[cfg(feature = "observe")]
            hunt_steps: [
                AtomicU64::new(self.hunt_steps[0].load(Ordering::Relaxed)),
                AtomicU64::new(self.hunt_steps[1].load(Ordering::Relaxed)),
                AtomicU64::new(self.hunt_steps[2].load(Ordering::Relaxed)),
                AtomicU64::new(self.hunt_steps[3].load(Ordering::Relaxed)),
                AtomicU64::new(self.hunt_steps[4].load(Ordering::Relaxed)),
                AtomicU64::new(self.hunt_steps[5].load(Ordering::Relaxed)),
                AtomicU64::new(self.hunt_steps[6].load(Ordering::Relaxed)),
                AtomicU64::new(self.hunt_steps[7].load(Ordering::Relaxed)),
            ],
            #[cfg(feature = "observe")]
            binary_fallback_count: [
                AtomicU64::new(self.binary_fallback_count[0].load(Ordering::Relaxed)),
                AtomicU64::new(self.binary_fallback_count[1].load(Ordering::Relaxed)),
                AtomicU64::new(self.binary_fallback_count[2].load(Ordering::Relaxed)),
                AtomicU64::new(self.binary_fallback_count[3].load(Ordering::Relaxed)),
                AtomicU64::new(self.binary_fallback_count[4].load(Ordering::Relaxed)),
                AtomicU64::new(self.binary_fallback_count[5].load(Ordering::Relaxed)),
                AtomicU64::new(self.binary_fallback_count[6].load(Ordering::Relaxed)),
                AtomicU64::new(self.binary_fallback_count[7].load(Ordering::Relaxed)),
            ],
        }
    }
}

impl RegularGridInterpolator {
    /// Construct a validated interpolator.
    ///
    /// # Arguments
    /// * `axes` – One `Vec<f64>` per dimension, each strictly ascending, non-empty.
    /// * `values` – Flattened row-major tensor.  Length must equal the product of axis lengths.
    /// * `strategy` – Extrapolation strategy for out-of-bounds coordinates.
    ///
    /// # Errors
    /// Returns `HaresError::Equipment` if any axis is empty, not strictly ascending,
    /// contains non-finite values, or the values length mismatches.
    pub fn new(
        axes: Vec<Vec<f64>>,
        values: Vec<f32>,
        strategy: ExtrapolationStrategy,
    ) -> crate::Result<Self> {
        if axes.is_empty() {
            return Err(HaresError::Equipment(
                "RegularGridInterpolator requires at least one axis".to_string(),
            ));
        }
        if axes.len() > 8 {
            return Err(HaresError::Equipment(format!(
                "RegularGridInterpolator supports at most 8 dimensions, got {}",
                axes.len()
            )));
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

        let cached_bracket = vec![0usize; ndim];

        Ok(Self {
            axes,
            values,
            strides,
            strategy,
            linear_extrap_warned: AtomicBool::new(false),
            cached_bracket,
            #[cfg(feature = "observe")]
            oob_count: AtomicU64::new(0),
            #[cfg(feature = "observe")]
            linear_extrap_count: [
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
            ],
            #[cfg(feature = "observe")]
            hunt_steps: [
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
            ],
            #[cfg(feature = "observe")]
            binary_fallback_count: [
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
            ],
        })
    }

    /// Number of dimensions.
    #[inline]
    pub fn ndim(&self) -> usize {
        self.axes.len()
    }

    /// Interpolate at a single point.
    ///
    /// `point` must have exactly `ndim()` elements.  Out-of-bounds coordinates
    /// are handled according to `self.strategy`.
    ///
    /// Uses a hunting strategy starting from the last-known bracket position
    /// to accelerate temporally-coherent queries, falling back to binary
    /// search when the target has moved more than `HUNT_THRESHOLD` steps.
    ///
    /// # Panics
    /// Panics if `point.len() != self.ndim()`.
    /// In debug/check_invariants builds, panics if strategy is `NaN` and any
    /// coordinate is out of bounds.
    pub fn interpolate(&mut self, point: &[f64]) -> f32 {
        assert_eq!(
            point.len(),
            self.axes.len(),
            "interpolate: expected {} coordinates, got {}",
            self.axes.len(),
            point.len()
        );

        let ndim = self.axes.len();

        assert!(
            ndim <= 8,
            "RegularGridInterpolator supports at most 8 dimensions, got {ndim}"
        );

        // Deserialised instances bypass new() and start with an empty cache
        // (serde skips cached_bracket, defaulting Vec to len=0).  Self-heal on
        // first interpolate call so deserialise-then-interpolate never panics.
        if self.cached_bracket.len() != ndim {
            self.cached_bracket.resize(ndim, 0);
        }

        // NaN strategy: check OOB first so we can return early.
        if self.strategy == ExtrapolationStrategy::NaN {
            for (dim, axis) in self.axes.iter().enumerate() {
                let x = point[dim];
                if x < axis[0] || x > axis[axis.len() - 1] {
                    #[cfg(feature = "observe")]
                    {
                        self.oob_count.fetch_add(1, Ordering::Relaxed);
                    }
                    #[cfg(any(debug_assertions, feature = "check_invariants"))]
                    {
                        panic!(
                            "RegularGridInterpolator: NaN strategy: coord[{dim}] = {x} \
                             is out of bounds [{}, {}]",
                            axis[0],
                            axis[axis.len() - 1]
                        );
                    }
                    #[cfg(not(any(debug_assertions, feature = "check_invariants")))]
                    {
                        return f32::NAN;
                    }
                }
            }
        }

        let mut lo_indices = [0usize; 8];
        let mut fracs = [0.0f64; 8];

        let needs_clamp = self.strategy != ExtrapolationStrategy::Linear;

        for (dim, axis) in self.axes.iter().enumerate() {
            let x = point[dim];
            let x_proc = if needs_clamp {
                x.clamp(axis[0], axis[axis.len() - 1])
            } else {
                x
            };
            let (lo, frac, steps, fallback) = hunt_bracket(
                axis,
                x_proc,
                &mut self.cached_bracket[dim],
                needs_clamp,
                HUNT_THRESHOLD,
            );
            lo_indices[dim] = lo;
            fracs[dim] = frac;
            #[cfg(not(feature = "observe"))]
            {
                let _ = (steps, fallback);
            }
            #[cfg(feature = "observe")]
            {
                self.hunt_steps[dim].fetch_add(steps, Ordering::Relaxed);
                if fallback {
                    self.binary_fallback_count[dim].fetch_add(1, Ordering::Relaxed);
                }
            }

            #[cfg(feature = "observe")]
            if !needs_clamp && (x < axis[0] || x > axis[axis.len() - 1]) {
                self.oob_count.fetch_add(1, Ordering::Relaxed);
                self.linear_extrap_count[dim].fetch_add(1, Ordering::Relaxed);
            }

            if !needs_clamp
                && !self.linear_extrap_warned.load(Ordering::Relaxed)
                && (x < axis[0] || x > axis[axis.len() - 1])
            {
                self.linear_extrap_warned.store(true, Ordering::Relaxed);
                tracing::warn!(
                    dim = dim,
                    query_value = x,
                    axis_min = axis[0],
                    axis_max = axis[axis.len() - 1],
                    "Linear extrapolation active: coordinate out of grid bounds"
                );
            }
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

/// Maximum number of linear hunt steps before falling back to binary search.
/// Beyond this distance, an O(log n) binary search is faster than O(n) linear
/// scanning. Chosen as a conservative threshold: hunting 20 elements is ~40 ns
/// on modern CPUs, well within the cost of one binary search step.
const HUNT_THRESHOLD: usize = 20;

/// Find the lower bracket index and fractional position of `x` in a sorted axis
/// using a hunting strategy starting from `guess`.
///
/// Returns `(lo, frac, linear_steps, binary_fallback)` where `lo` is the lower
/// bracket index (clamped to `[0, n-2]`), `frac` is the fractional position
/// within the bracket, `linear_steps` is the number of elements scanned, and
/// `binary_fallback` is true if a binary search was used because the target
/// moved beyond `hunt_threshold` steps.
///
/// When `clamp_frac` is false, the fractional position is not clamped to [0,1],
/// allowing negative or >1 values for linear extrapolation.
#[inline]
fn hunt_bracket(
    axis: &[f64],
    x: f64,
    guess: &mut usize,
    clamp_frac: bool,
    hunt_threshold: usize,
) -> (usize, f64, u64, bool) {
    let n = axis.len();
    if n == 1 {
        *guess = 0;
        return (0, 0.0, 0, false);
    }

    // Clamp guess to valid bracket index range [0, n-2].
    *guess = (*guess).min(n - 2);

    let mut lo = *guess;
    let mut steps: u64 = 0;

    if x >= axis[lo] {
        // Hunt upward: walk forward while x is at or past axis[hi].
        let mut hi = lo + 1;
        while hi < n && x >= axis[hi] {
            steps += 1;
            lo = hi;
            hi += 1;
        }
    } else {
        // Hunt downward: walk backward while x is before axis[lo].
        while lo > 0 && x < axis[lo] {
            steps += 1;
            lo -= 1;
        }
    }

    // Clamp lo after the hunt: an upward walk can overshoot to n-1 when
    // x >= axis[n-1]; a downward walk never undershoots below 0.
    lo = lo.min(n - 2);

    let binary_fallback = steps as usize > hunt_threshold;
    if binary_fallback {
        let pos = axis.partition_point(|&v| v <= x);
        lo = if pos == 0 {
            0
        } else if pos >= n {
            n - 2
        } else {
            pos - 1
        };
    }

    *guess = lo;
    let span = axis[lo + 1] - axis[lo];
    let frac = if span.abs() < f64::EPSILON {
        0.0
    } else {
        let raw = (x - axis[lo]) / span;
        if clamp_frac { raw.clamp(0.0, 1.0) } else { raw }
    };
    (lo, frac, steps, binary_fallback)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interp_1d_linear() {
        let mut interp = RegularGridInterpolator::new(
            vec![vec![0.0, 1.0]],
            vec![0.0, 10.0],
            ExtrapolationStrategy::Clamp,
        )
        .unwrap();
        assert!((interp.interpolate(&[0.0]) - 0.0).abs() < 1e-5);
        assert!((interp.interpolate(&[0.5]) - 5.0).abs() < 1e-5);
        assert!((interp.interpolate(&[1.0]) - 10.0).abs() < 1e-5);
    }

    #[test]
    fn interp_1d_clamp() {
        let mut interp = RegularGridInterpolator::new(
            vec![vec![0.0, 1.0]],
            vec![2.0, 8.0],
            ExtrapolationStrategy::Clamp,
        )
        .unwrap();
        // Below lower bound → clamp to first value.
        assert!((interp.interpolate(&[-1.0]) - 2.0).abs() < 1e-5);
        // Above upper bound → clamp to last value.
        assert!((interp.interpolate(&[2.0]) - 8.0).abs() < 1e-5);
    }

    #[test]
    fn interp_2d_bilinear() {
        // f(x, y) = x + y on [0,1] × [0,1]
        let mut interp = RegularGridInterpolator::new(
            vec![vec![0.0, 1.0], vec![0.0, 1.0]],
            // Row-major: (0,0)=0, (0,1)=1, (1,0)=1, (1,1)=2
            vec![0.0, 1.0, 1.0, 2.0],
            ExtrapolationStrategy::Clamp,
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
        let mut interp =
            RegularGridInterpolator::new(axes, values, ExtrapolationStrategy::Clamp).unwrap();
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
                    values[8 + t * 4 + c * 2 + h] = 1.0;
                }
            }
        }
        let mut interp =
            RegularGridInterpolator::new(axes, values, ExtrapolationStrategy::Clamp).unwrap();
        assert!((interp.interpolate(&[0.0, 0.5, 0.5, 0.5]) - 0.0).abs() < 1e-5);
        assert!((interp.interpolate(&[1.0, 0.5, 0.5, 0.5]) - 1.0).abs() < 1e-5);
        assert!((interp.interpolate(&[0.5, 0.5, 0.5, 0.5]) - 0.5).abs() < 1e-5);
        assert!((interp.interpolate(&[0.25, 0.0, 0.0, 0.0]) - 0.25).abs() < 1e-5);
    }

    #[test]
    fn interp_3_point_axis() {
        // 1D with 3 points: [0, 0.5, 1.0] → [0, 1, 0]
        let mut interp = RegularGridInterpolator::new(
            vec![vec![0.0, 0.5, 1.0]],
            vec![0.0, 1.0, 0.0],
            ExtrapolationStrategy::Clamp,
        )
        .unwrap();
        assert!((interp.interpolate(&[0.25]) - 0.5).abs() < 1e-5);
        assert!((interp.interpolate(&[0.5]) - 1.0).abs() < 1e-5);
        assert!((interp.interpolate(&[0.75]) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn rejects_empty_axis() {
        let err = RegularGridInterpolator::new(vec![vec![]], vec![], ExtrapolationStrategy::Clamp)
            .unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn rejects_non_ascending_axis() {
        let err = RegularGridInterpolator::new(
            vec![vec![1.0, 0.5]],
            vec![1.0, 2.0],
            ExtrapolationStrategy::Clamp,
        )
        .unwrap_err();
        assert!(err.to_string().contains("ascending"));
    }

    #[test]
    fn rejects_values_length_mismatch() {
        let err = RegularGridInterpolator::new(
            vec![vec![0.0, 1.0], vec![0.0, 1.0]],
            vec![1.0, 2.0, 3.0], // needs 4
            ExtrapolationStrategy::Clamp,
        )
        .unwrap_err();
        assert!(err.to_string().contains("length"));
    }

    #[test]
    fn rejects_nan_in_axis() {
        let err = RegularGridInterpolator::new(
            vec![vec![0.0, f64::NAN]],
            vec![1.0, 2.0],
            ExtrapolationStrategy::Clamp,
        )
        .unwrap_err();
        assert!(err.to_string().contains("not finite"));
    }

    #[test]
    fn rejects_nan_in_values() {
        let err = RegularGridInterpolator::new(
            vec![vec![0.0, 1.0]],
            vec![1.0, f32::NAN],
            ExtrapolationStrategy::Clamp,
        )
        .unwrap_err();
        assert!(err.to_string().contains("not finite"));
    }

    #[test]
    fn single_point_axis() {
        // Single-point axis: always returns that value.
        let mut interp = RegularGridInterpolator::new(
            vec![vec![0.5]],
            vec![3.125],
            ExtrapolationStrategy::Clamp,
        )
        .unwrap();
        assert!((interp.interpolate(&[0.0]) - 3.125).abs() < 1e-5);
        assert!((interp.interpolate(&[0.5]) - 3.125).abs() < 1e-5);
        assert!((interp.interpolate(&[1.0]) - 3.125).abs() < 1e-5);
    }

    #[test]
    fn rejects_more_than_8_dimensions() {
        let axes: Vec<Vec<f64>> = (0..9).map(|_| vec![0.0, 1.0]).collect();
        let values = vec![0.0f32; 512]; // 2^9
        let err =
            RegularGridInterpolator::new(axes, values, ExtrapolationStrategy::Clamp).unwrap_err();
        assert!(err.to_string().contains("8 dimensions"));
    }

    #[test]
    fn rejects_no_axes() {
        let err =
            RegularGridInterpolator::new(vec![], vec![], ExtrapolationStrategy::Clamp).unwrap_err();
        assert!(err.to_string().contains("at least one"));
    }

    #[test]
    fn rejects_inf_in_axis() {
        let err = RegularGridInterpolator::new(
            vec![vec![0.0, f64::INFINITY]],
            vec![1.0, 2.0],
            ExtrapolationStrategy::Clamp,
        )
        .unwrap_err();
        assert!(err.to_string().contains("not finite"));
    }

    #[test]
    fn rejects_inf_in_values() {
        let err = RegularGridInterpolator::new(
            vec![vec![0.0, 1.0]],
            vec![1.0, f32::INFINITY],
            ExtrapolationStrategy::Clamp,
        )
        .unwrap_err();
        assert!(err.to_string().contains("not finite"));
    }

    #[test]
    fn rejects_duplicate_axis_values() {
        let err = RegularGridInterpolator::new(
            vec![vec![0.0, 0.0, 1.0]],
            vec![1.0, 2.0, 3.0],
            ExtrapolationStrategy::Clamp,
        )
        .unwrap_err();
        assert!(err.to_string().contains("ascending"));
    }

    #[test]
    fn interp_2d_clamp_both_axes() {
        // f(x,y) = x*10 + y on [0,1]×[0,1]
        let mut interp = RegularGridInterpolator::new(
            vec![vec![0.0, 1.0], vec![0.0, 1.0]],
            vec![0.0, 1.0, 10.0, 11.0],
            ExtrapolationStrategy::Clamp,
        )
        .unwrap();
        // Out of bounds on both axes → clamp to corner (1,1) = 11.0
        assert!((interp.interpolate(&[5.0, 5.0]) - 11.0).abs() < 1e-5);
        // Out of bounds negative → clamp to corner (0,0) = 0.0
        assert!((interp.interpolate(&[-5.0, -5.0]) - 0.0).abs() < 1e-5);
    }

    #[test]
    fn interp_4d_varies_on_second_axis() {
        // f(s,t,c,h) = t (only temperature axis varies)
        let axes = vec![
            vec![0.0, 1.0],
            vec![0.0, 1.0],
            vec![0.0, 1.0],
            vec![0.0, 1.0],
        ];
        let mut values = vec![0.0f32; 16];
        // Row-major: index = s*8 + t*4 + c*2 + h
        // When t=1, value = 1.0
        for s in 0..2 {
            for c in 0..2 {
                for h in 0..2 {
                    values[s * 8 + 4 + c * 2 + h] = 1.0;
                }
            }
        }
        let mut interp =
            RegularGridInterpolator::new(axes, values, ExtrapolationStrategy::Clamp).unwrap();
        assert!((interp.interpolate(&[0.5, 0.0, 0.5, 0.5]) - 0.0).abs() < 1e-5);
        assert!((interp.interpolate(&[0.5, 1.0, 0.5, 0.5]) - 1.0).abs() < 1e-5);
        assert!((interp.interpolate(&[0.5, 0.5, 0.5, 0.5]) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn ndim_reports_correctly() {
        let interp = RegularGridInterpolator::new(
            vec![vec![0.0, 1.0], vec![0.0, 1.0], vec![0.0, 1.0]],
            vec![0.0; 8],
            ExtrapolationStrategy::Clamp,
        )
        .unwrap();
        assert_eq!(interp.ndim(), 3);
    }

    #[test]
    #[should_panic(expected = "expected 2 coordinates")]
    fn interpolate_panics_on_wrong_point_len() {
        let mut interp = RegularGridInterpolator::new(
            vec![vec![0.0, 1.0], vec![0.0, 1.0]],
            vec![0.0; 4],
            ExtrapolationStrategy::Clamp,
        )
        .unwrap();
        interp.interpolate(&[0.5]); // wrong: 1 coord for 2D
    }

    // ── ExtrapolationStrategy tests ──────────────────────────────────────

    fn make_2d_grid_with_strategy(s: ExtrapolationStrategy) -> RegularGridInterpolator {
        RegularGridInterpolator::new(
            vec![vec![0.0, 1.0], vec![0.0, 1.0]],
            vec![0.0, 1.0, 10.0, 11.0],
            s,
        )
        .unwrap()
    }

    #[test]
    fn strategy_clamp_fully_oob_returns_corner_value() {
        let mut interp = make_2d_grid_with_strategy(ExtrapolationStrategy::Clamp);
        // Both coords OOB — clamp both to 1.0, bilinear at (1.0,1.0) → f(1,1)=11.0
        assert!((interp.interpolate(&[2.0, 2.0]) - 11.0).abs() < 1e-5);
    }

    #[test]
    fn strategy_clamp_mixed_oob() {
        let mut interp = make_2d_grid_with_strategy(ExtrapolationStrategy::Clamp);
        // y OOB (2.0), x in-bounds (0.5) — clamp y to 1.0, interp x → f(0.5,1.0)=6.0
        assert!((interp.interpolate(&[0.5, 2.0]) - 6.0).abs() < 1e-5);
    }

    /// In debug/check_invariants builds, NaN strategy panics on OOB.
    /// In release builds without check_invariants, it returns NaN.
    #[test]
    #[cfg_attr(
        any(debug_assertions, feature = "check_invariants"),
        should_panic(expected = "NaN strategy")
    )]
    fn strategy_nan_fully_oob_returns_nan() {
        let mut interp = make_2d_grid_with_strategy(ExtrapolationStrategy::NaN);
        let result = interp.interpolate(&[2.0, 2.0]);
        #[cfg(not(any(debug_assertions, feature = "check_invariants")))]
        assert!(result.is_nan());
        // In invariant builds the panic already asserted — disable the unused warning
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        let _ = result;
    }

    /// In debug/check_invariants builds, NaN strategy panics on OOB.
    #[test]
    #[cfg_attr(
        any(debug_assertions, feature = "check_invariants"),
        should_panic(expected = "NaN strategy")
    )]
    fn strategy_nan_mixed_oob_returns_nan() {
        let mut interp = make_2d_grid_with_strategy(ExtrapolationStrategy::NaN);
        let result = interp.interpolate(&[0.5, 2.0]);
        #[cfg(not(any(debug_assertions, feature = "check_invariants")))]
        assert!(result.is_nan());
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        let _ = result;
    }

    #[test]
    fn strategy_nan_inbounds_returns_same_as_clamp() {
        let mut clamp = make_2d_grid_with_strategy(ExtrapolationStrategy::Clamp);
        let mut nan_interp = make_2d_grid_with_strategy(ExtrapolationStrategy::NaN);
        let point = [0.25, 0.75];
        let v_clamp = clamp.interpolate(&point);
        let v_nan = nan_interp.interpolate(&point);
        assert!((v_clamp - v_nan).abs() < 1e-5);
        assert!(!v_nan.is_nan());
    }

    #[test]
    fn strategy_linear_fully_oob_extrapolates() {
        let mut interp = make_2d_grid_with_strategy(ExtrapolationStrategy::Linear);
        // f(x,y) = x*10 + y. At (2.0, 2.0): f = 20 + 2 = 22.0
        assert!((interp.interpolate(&[2.0, 2.0]) - 22.0).abs() < 1e-5);
    }

    #[test]
    fn strategy_linear_mixed_oob() {
        let mut interp = make_2d_grid_with_strategy(ExtrapolationStrategy::Linear);
        // y=2.0 means y_frac = (2-0)/1 = 2.0 (extrapolates up)
        // x=0.5 means x_frac = 0.5 (interpolates)
        // Weights: (1-0.5)*(1-2.0)=0.5*(-1)=-0.5 for (0,0), (1-0.5)*2.0=1.0 for (0,1),
        //          0.5*(-1)=-0.5 for (1,0), 0.5*2.0=1.0 for (1,1)
        // Values: 0, 1, 10, 11 → result = -0.5*0 + 1.0*1 + -0.5*10 + 1.0*11 = 0 + 1 - 5 + 11 = 7.0
        assert!((interp.interpolate(&[0.5, 2.0]) - 7.0).abs() < 1e-5);
    }

    #[test]
    fn strategy_linear_below_bounds() {
        let mut interp = make_2d_grid_with_strategy(ExtrapolationStrategy::Linear);
        // x=-1.0→frac=-1, y=0.5→frac=0.5
        // Weights: (1-(-1))*(1-0.5)=2*0.5=1 for (0,0), 2*0.5=1 for (0,1),
        //          (-1)*(1-0.5)=-0.5 for (1,0), -1*0.5=-0.5 for (1,1)
        // Result: 1*0 + 1*1 + -0.5*10 + -0.5*11 = 0 + 1 - 5 - 5.5 = -9.5
        assert!((interp.interpolate(&[-1.0, 0.5]) - (-9.5)).abs() < 1e-5);
    }

    #[test]
    fn strategy_nearest_neighbor_fully_oob_snaps_to_corner() {
        let mut interp = make_2d_grid_with_strategy(ExtrapolationStrategy::NearestNeighbor);
        // behaves identically to Clamp for 2-point axes
        assert!((interp.interpolate(&[2.0, 2.0]) - 11.0).abs() < 1e-5);
    }

    #[test]
    fn strategy_nearest_neighbor_mixed_oob() {
        let mut interp = make_2d_grid_with_strategy(ExtrapolationStrategy::NearestNeighbor);
        // behaves identically to Clamp for 2-point axes
        assert!((interp.interpolate(&[0.5, 2.0]) - 6.0).abs() < 1e-5);
    }

    #[test]
    fn strategy_stored_and_used_correctly() {
        for strategy in [
            ExtrapolationStrategy::Clamp,
            ExtrapolationStrategy::NaN,
            ExtrapolationStrategy::Linear,
            ExtrapolationStrategy::NearestNeighbor,
        ] {
            let mut interp = make_2d_grid_with_strategy(strategy);
            let result = interp.interpolate(&[0.5, 0.5]); // in-bounds, should not panic
            assert!(!result.is_nan() || strategy == ExtrapolationStrategy::NaN);
            // in-bounds should never be NaN even with NaN strategy
            if strategy == ExtrapolationStrategy::NaN {
                assert!(
                    !result.is_nan(),
                    "NaN strategy should not produce NaN for in-bounds queries"
                );
            }
        }
    }

    #[test]
    fn strategy_linear_1d_extrapolation_above() {
        let mut interp = RegularGridInterpolator::new(
            vec![vec![0.0, 1.0, 2.0]],
            vec![0.0, 1.0, 2.0],
            ExtrapolationStrategy::Linear,
        )
        .unwrap();
        let result = interp.interpolate(&[3.0]);
        assert!((result - 3.0).abs() < 1e-5, "expected 3.0, got {result}");
    }

    #[test]
    fn strategy_linear_1d_extrapolation_below() {
        let mut interp = RegularGridInterpolator::new(
            vec![vec![0.0, 1.0, 2.0]],
            vec![0.0, 1.0, 2.0],
            ExtrapolationStrategy::Linear,
        )
        .unwrap();
        let result = interp.interpolate(&[-1.0]);
        assert!(
            (result - (-1.0)).abs() < 1e-5,
            "expected -1.0, got {result}"
        );
    }

    #[test]
    fn strategy_linear_inbounds_matches_clamp() {
        let grids: Vec<RegularGridInterpolator> = vec![
            // 1D non-uniform
            RegularGridInterpolator::new(
                vec![vec![0.0, 0.25, 7.0]],
                vec![10.0, 11.0, 13.0],
                ExtrapolationStrategy::Linear,
            )
            .unwrap(),
            // 2D
            make_2d_grid_with_strategy(ExtrapolationStrategy::Linear),
            // 4D constant
            RegularGridInterpolator::new(
                vec![
                    vec![0.0, 0.5, 1.0],
                    vec![-10.0, 25.0, 45.0],
                    vec![0.1, 1.0, 2.0],
                    vec![0.7, 1.0],
                ],
                vec![0.5f32; 54],
                ExtrapolationStrategy::Linear,
            )
            .unwrap(),
        ];
        for mut interp in grids {
            // Query at various in-bounds points; should match Clamp.
            let mut clamp = RegularGridInterpolator::new(
                interp.axes.clone(),
                interp.values.clone(),
                ExtrapolationStrategy::Clamp,
            )
            .unwrap();
            let ndim = interp.ndim();
            let test_points: Vec<Vec<f64>> = if ndim == 1 {
                vec![vec![0.0], vec![3.0], vec![6.99]]
            } else if ndim == 2 {
                vec![
                    vec![0.0, 0.0],
                    vec![0.5, 0.5],
                    vec![1.0, 1.0],
                    vec![0.25, 0.75],
                ]
            } else {
                vec![vec![0.3, 15.0, 0.5, 0.85]]
            };
            for pt in &test_points {
                let v_linear = interp.interpolate(pt);
                let v_clamp = clamp.interpolate(pt);
                assert!(
                    (v_linear - v_clamp).abs() < 1e-5,
                    "Linear vs Clamp mismatch at {:?}: linear={v_linear}, clamp={v_clamp}",
                    pt
                );
            }
        }
    }

    #[test]
    fn strategy_linear_single_element_axis_does_not_panic() {
        let mut interp = RegularGridInterpolator::new(
            vec![vec![0.5]],
            vec![3.125],
            ExtrapolationStrategy::Linear,
        )
        .unwrap();
        // Single-element axis: bracket returns (0, 0.0), result is the lone value.
        let result = interp.interpolate(&[0.0]);
        assert!((result - 3.125).abs() < 1e-5);
        let result = interp.interpolate(&[1.0]);
        assert!((result - 3.125).abs() < 1e-5);
    }

    #[test]
    fn cache_tracks_bracket_across_temporally_coherent_queries() {
        // 4D grid: 3 SOC points × 3 temp points × 2 c-rate points × 2 SOH points.
        let axes = vec![
            vec![0.0, 0.5, 1.0],
            vec![-10.0, 25.0, 45.0],
            vec![0.1, 1.0, 2.0],
            vec![0.7, 0.85, 1.0],
        ];
        let total: usize = axes.iter().map(|a| a.len()).product();
        let values = vec![0.5f32; total];
        let mut interp =
            RegularGridInterpolator::new(axes, values, ExtrapolationStrategy::Clamp).unwrap();

        // All cached brackets should start at 0.
        for dim in 0..interp.ndim() {
            assert_eq!(interp.cached_bracket[dim], 0);
        }

        // First query at mid-range positions: should update the cache from 0.
        let _result = interp.interpolate(&[0.51, 25.0, 1.0, 0.85]);
        // After first query, cached brackets should reflect actual positions.
        assert!(
            interp.cached_bracket[0] > 0,
            "SOC bracket should have moved from 0"
        );
        assert!(
            interp.cached_bracket[1] > 0,
            "temp bracket should have moved from 0"
        );
        assert!(
            interp.cached_bracket[2] > 0,
            "c-rate bracket should have moved from 0"
        );
        assert!(
            interp.cached_bracket[3] > 0,
            "SOH bracket should have moved from 0"
        );

        // Record cache positions after first query.
        let first_cache: Vec<usize> = interp.cached_bracket.clone();

        // Second query: small movement — should use linear hunting, not binary search.
        let _result = interp.interpolate(&[0.50, 25.0, 1.0, 0.85]);
        // Most dimensions unchanged; SOC moved slightly from 0.51 to 0.50.
        // Cache should still be near the first positions.
        for (dim, &prev) in first_cache.iter().enumerate() {
            let delta = interp.cached_bracket[dim].abs_diff(prev);
            assert!(
                delta <= 1,
                "dim {dim}: cache moved {delta} steps, expected <=1 for coherent query"
            );
        }

        // Third query: large jump in one dimension — falls back to binary search.
        let _result = interp.interpolate(&[0.9, -5.0, 0.05, 0.95]);
        // Temp went from 25.0 to -5.0 — a big jump that should trigger binary fallback.
        // The result should still be correct (0.5 constant grid).
        assert!(
            (_result - 0.5).abs() < 1e-5,
            "numerical result should be unchanged"
        );

        // Cache should be valid (each entry in [0, axis.len()-2] range).
        for dim in 0..interp.ndim() {
            let max_lo = interp.axes[dim].len().saturating_sub(2);
            assert!(
                interp.cached_bracket[dim] <= max_lo,
                "dim {dim}: cached bracket {} out of range [0, {max_lo}]",
                interp.cached_bracket[dim]
            );
        }
    }
}
