//! State-space representation and zero-order hold discretization.

use nalgebra::{Complex, DMatrix, DVector};
use thiserror::Error;

const RCOND_THRESHOLD: f64 = 1.0e-12;
pub const ZERO_GAIN_EPSILON: f64 = 1.0e-12;

/// Discrete eigenvalue magnitude threshold for near-unity stability warning.
/// Eigenvalues with |λ| > this value trigger a tracing::warn for slow convergence.
const NEAR_UNITY_EIGENVALUE_THRESHOLD: f64 = 0.99;

/// Result type for state-space operations.
pub type Result<T> = std::result::Result<T, StateSpaceError>;

/// Recoverable errors for state-space construction and stepping.
#[derive(Debug, Error)]
pub enum StateSpaceError {
    #[error("matrix dimensions are incompatible: {0}")]
    DimensionMismatch(String),
    #[error("A_c is singular or ill-conditioned for direct ZOH solve")]
    SingularContinuousMatrix,
    #[error("timestep must be finite and non-negative, got {0}")]
    InvalidTimestep(f64),
    #[error("input index {index} is out of bounds for input dimension {input_dim}")]
    InputIndexOutOfBounds { index: usize, input_dim: usize },
    #[error("solve_for_input requires a single output, found {0}")]
    UnsupportedOutputCount(usize),
    #[error("effective gain is zero for input index {input_index}")]
    ZeroEffectiveGain { input_index: usize },
    #[error(
        "output mapping references out-of-bounds node index {node_index} for {node_dim} states"
    )]
    NodeMappingOutOfBounds { node_index: usize, node_dim: usize },
    #[error(
        "output mapping references out-of-bounds input index {input_index} for {input_dim} inputs"
    )]
    InputMappingOutOfBounds {
        input_index: usize,
        input_dim: usize,
    },
    #[error(
        "output mapping references out-of-bounds output index {output_index} for {output_dim} outputs"
    )]
    OutputMappingOutOfBounds {
        output_index: usize,
        output_dim: usize,
    },
    #[error("output index {output_index} is out of bounds for {output_dim} outputs")]
    OutputIndexOutOfBounds {
        output_index: usize,
        output_dim: usize,
    },
    #[error("Padé denominator matrix is singular in matrix_exp")]
    SingularPadeMatrix,
    #[error("state-space stability check failed: {0:?}")]
    UnstableSystem(StabilityResult),
}

/// Construction-time mapping from RC nodes and direct inputs to observable outputs.
#[derive(Debug, Clone, Default)]
pub struct OutputMapping {
    pub output_count: usize,
    pub node_to_output: Vec<(usize, usize, f64)>,
    pub input_to_output: Vec<(usize, usize, f64)>,
}

/// Discrete-time state-space model.
#[derive(Debug, Clone)]
pub struct StateSpaceModel {
    pub a_d: DMatrix<f64>,
    pub b_d: DMatrix<f64>,
    pub c: DMatrix<f64>,
    pub d: DMatrix<f64>,
}

/// Stability diagnostic payload returned by eigenvalue checks.
#[derive(Debug, Clone)]
pub struct StabilityResult {
    pub continuous_stable: bool,
    pub discrete_stable: bool,
    pub near_unity_eigenvalues: Vec<(usize, Complex<f64>)>,
}

/// Verdict from [`StateSpaceModel::verify_stability`].
#[derive(Debug, Clone, PartialEq)]
pub enum StabilityVerdict {
    /// All discrete eigenvalue magnitudes are within the unit circle.
    Stable,
    /// At least one eigenvalue magnitude exceeds 1 + epsilon.
    MarginallyUnstable { max_eigenvalue_magnitude: f64 },
}

impl StateSpaceModel {
    /// Constructs a fully-discrete model from matrix terms that are already discretized.
    pub fn from_discrete(
        a_d: DMatrix<f64>,
        b_d: DMatrix<f64>,
        c: DMatrix<f64>,
        d: DMatrix<f64>,
    ) -> Result<Self> {
        validate_state_space_dimensions(&a_d, &b_d, &c, &d)?;
        Ok(Self { a_d, b_d, c, d })
    }

    /// Constructs from continuous-time matrices, computing C/D from mapping and discretizing A/B.
    pub fn from_continuous(
        a_c: &DMatrix<f64>,
        b_c: &DMatrix<f64>,
        dt: f64,
        output_mapping: &OutputMapping,
    ) -> Result<Self> {
        validate_continuous_dimensions(a_c, b_c)?;
        if !dt.is_finite() || dt < 0.0 {
            return Err(StateSpaceError::InvalidTimestep(dt));
        }

        let (c, d) = build_output_matrices(a_c.nrows(), b_c.ncols(), output_mapping)?;
        let (a_d, b_d) = discretize_auto(a_c, b_c, dt)?;

        // Eigenvalue stability check is O(n³) via Schur decomposition and
        // prohibitively slow for large RC networks (n > 20) in debug builds.
        if a_c.nrows() <= 20 {
            let a_c_singular = is_singular(a_c);
            if let Err(stability) = eigenvalue_check(a_c, &a_d) {
                let discrete_marginally_stable = a_d
                    .clone()
                    .complex_eigenvalues()
                    .iter()
                    .all(|lambda| lambda.norm() <= 1.0 + 1e-10);
                if !(a_c_singular && discrete_marginally_stable) {
                    return Err(StateSpaceError::UnstableSystem(stability));
                }
            }
        } else {
            tracing::debug!(
                n = a_c.nrows(),
                "skipping full eigenvalue check for large matrix; using Gershgorin bound"
            );
            let bound = gershgorin_spectral_radius(&a_d);
            if bound > 1.0 + 1e-10 {
                return Err(StateSpaceError::UnstableSystem(StabilityResult {
                    continuous_stable: false,
                    discrete_stable: false,
                    near_unity_eigenvalues: Vec::new(),
                }));
            }
        }

        Self::from_discrete(a_d, b_d, c, d)
    }

    /// Steps one time increment: x[k+1] = A_d * x[k] + B_d * u[k].
    pub fn step(&self, x: &DVector<f64>, u: &DVector<f64>) -> DVector<f64> {
        &self.a_d * x + &self.b_d * u
    }

    /// Computes output from current state/input: y[k] = C * x[k] + D * u[k].
    pub fn output(&self, x: &DVector<f64>, u: &DVector<f64>) -> DVector<f64> {
        &self.c * x + &self.d * u
    }

    /// Full eigenvalue stability check on the discrete state matrix, regardless of size.
    ///
    /// Returns [`StabilityVerdict::Stable`] when all eigenvalue magnitudes are within
    /// the unit circle (1 + 1e-10 tolerance), or [`StabilityVerdict::MarginallyUnstable`]
    /// with the worst magnitude otherwise.
    pub fn verify_stability(&self) -> Result<StabilityVerdict> {
        let eigs = self.a_d.clone().complex_eigenvalues();
        let max_mag = eigs.iter().map(|l| l.norm()).fold(0.0_f64, f64::max);
        if max_mag <= 1.0 + 1e-10 {
            Ok(StabilityVerdict::Stable)
        } else {
            Ok(StabilityVerdict::MarginallyUnstable {
                max_eigenvalue_magnitude: max_mag,
            })
        }
    }

    /// Solves for a scalar input that drives a specific output row to `y_target`
    /// after one step, without cloning A_d or B_d.
    pub fn solve_for_output_input(
        &self,
        x: &DVector<f64>,
        u: &DVector<f64>,
        y_target: f64,
        output_index: usize,
        input_index: usize,
    ) -> Result<f64> {
        self.solve_for_scalar_input(x, u, y_target, output_index, input_index)
    }

    /// Solves for one scalar input value to hit a scalar output target after one step.
    pub fn solve_for_input(
        &self,
        x: &DVector<f64>,
        u: &DVector<f64>,
        y_target: f64,
        input_index: usize,
    ) -> Result<f64> {
        if self.c.nrows() != 1 {
            return Err(StateSpaceError::UnsupportedOutputCount(self.c.nrows()));
        }
        self.solve_for_scalar_input(x, u, y_target, 0, input_index)
    }

    fn solve_for_scalar_input(
        &self,
        x: &DVector<f64>,
        u: &DVector<f64>,
        y_target: f64,
        output_index: usize,
        input_index: usize,
    ) -> Result<f64> {
        if output_index >= self.c.nrows() {
            return Err(StateSpaceError::OutputIndexOutOfBounds {
                output_index,
                output_dim: self.c.nrows(),
            });
        }
        if input_index >= self.b_d.ncols() {
            return Err(StateSpaceError::InputIndexOutOfBounds {
                index: input_index,
                input_dim: self.b_d.ncols(),
            });
        }

        let u_i_original = u[input_index];

        // x_next_fixed = A_d * x + B_d * u - B_d[:,input_index] * u_i
        let x_next_fixed =
            &self.a_d * x + &self.b_d * u - self.b_d.column(input_index) * u_i_original;

        let c_row = self.c.row(output_index);
        let d_row = self.d.row(output_index);
        let y_fixed =
            (c_row * &x_next_fixed)[0] + (d_row * u)[0] - d_row[input_index] * u_i_original;

        let b_col = self.b_d.column(input_index);
        let effective_gain = (c_row * b_col)[0] + self.d[(output_index, input_index)];

        if effective_gain.abs() <= ZERO_GAIN_EPSILON {
            return Err(StateSpaceError::ZeroEffectiveGain { input_index });
        }

        Ok((y_target - y_fixed) / effective_gain)
    }
}

fn validate_continuous_dimensions(a_c: &DMatrix<f64>, b_c: &DMatrix<f64>) -> Result<()> {
    if a_c.nrows() != a_c.ncols() {
        return Err(StateSpaceError::DimensionMismatch(format!(
            "A_c must be square, got {}x{}",
            a_c.nrows(),
            a_c.ncols()
        )));
    }

    if b_c.nrows() != a_c.nrows() {
        return Err(StateSpaceError::DimensionMismatch(format!(
            "B_c row count ({}) must match A_c dimension ({})",
            b_c.nrows(),
            a_c.nrows()
        )));
    }

    Ok(())
}

fn validate_state_space_dimensions(
    a_d: &DMatrix<f64>,
    b_d: &DMatrix<f64>,
    c: &DMatrix<f64>,
    d: &DMatrix<f64>,
) -> Result<()> {
    if a_d.nrows() != a_d.ncols() {
        return Err(StateSpaceError::DimensionMismatch(format!(
            "A_d must be square, got {}x{}",
            a_d.nrows(),
            a_d.ncols()
        )));
    }

    if b_d.nrows() != a_d.nrows() {
        return Err(StateSpaceError::DimensionMismatch(format!(
            "B_d row count ({}) must match A_d dimension ({})",
            b_d.nrows(),
            a_d.nrows()
        )));
    }

    if c.ncols() != a_d.nrows() {
        return Err(StateSpaceError::DimensionMismatch(format!(
            "C column count ({}) must match state dimension ({})",
            c.ncols(),
            a_d.nrows()
        )));
    }

    if d.nrows() != c.nrows() || d.ncols() != b_d.ncols() {
        return Err(StateSpaceError::DimensionMismatch(format!(
            "D must have shape {}x{}, got {}x{}",
            c.nrows(),
            b_d.ncols(),
            d.nrows(),
            d.ncols()
        )));
    }

    Ok(())
}

fn build_output_matrices(
    state_dim: usize,
    input_dim: usize,
    mapping: &OutputMapping,
) -> Result<(DMatrix<f64>, DMatrix<f64>)> {
    let mut c = DMatrix::zeros(mapping.output_count, state_dim);
    let mut d = DMatrix::zeros(mapping.output_count, input_dim);

    for (output_index, node_index, coeff) in &mapping.node_to_output {
        if *output_index >= mapping.output_count {
            return Err(StateSpaceError::OutputMappingOutOfBounds {
                output_index: *output_index,
                output_dim: mapping.output_count,
            });
        }
        if *node_index >= state_dim {
            return Err(StateSpaceError::NodeMappingOutOfBounds {
                node_index: *node_index,
                node_dim: state_dim,
            });
        }
        c[(*output_index, *node_index)] += *coeff;
    }

    for (output_index, input_index, coeff) in &mapping.input_to_output {
        if *output_index >= mapping.output_count {
            return Err(StateSpaceError::OutputMappingOutOfBounds {
                output_index: *output_index,
                output_dim: mapping.output_count,
            });
        }
        if *input_index >= input_dim {
            return Err(StateSpaceError::InputMappingOutOfBounds {
                input_index: *input_index,
                input_dim,
            });
        }
        d[(*output_index, *input_index)] += *coeff;
    }

    Ok((c, d))
}

fn is_singular(a: &DMatrix<f64>) -> bool {
    let lu = a.clone().lu();
    let identity = DMatrix::<f64>::identity(a.nrows(), a.ncols());
    match lu.solve(&identity) {
        None => true,
        Some(a_inv) => reciprocal_condition_estimate_1_norm(a, &a_inv) <= RCOND_THRESHOLD,
    }
}

fn discretize_auto(
    a_c: &DMatrix<f64>,
    b_c: &DMatrix<f64>,
    dt: f64,
) -> Result<(DMatrix<f64>, DMatrix<f64>)> {
    if dt == 0.0 {
        let n = a_c.nrows();
        return Ok((
            DMatrix::<f64>::identity(n, n),
            DMatrix::<f64>::zeros(n, b_c.ncols()),
        ));
    }

    let lu = a_c.clone().lu();
    let identity = DMatrix::<f64>::identity(a_c.nrows(), a_c.ncols());

    if let Some(a_inv) = lu.solve(&identity) {
        let rcond = reciprocal_condition_estimate_1_norm(a_c, &a_inv);
        if rcond > RCOND_THRESHOLD {
            return discretize_zoh(a_c, b_c, dt);
        }
    }

    van_loan_discretize(a_c, b_c, dt)
}

/// Zero-order hold discretization, using direct LU solve for B_d.
pub fn discretize_zoh(
    a_c: &DMatrix<f64>,
    b_c: &DMatrix<f64>,
    dt: f64,
) -> Result<(DMatrix<f64>, DMatrix<f64>)> {
    validate_continuous_dimensions(a_c, b_c)?;
    if !dt.is_finite() || dt < 0.0 {
        return Err(StateSpaceError::InvalidTimestep(dt));
    }
    if dt == 0.0 {
        let n = a_c.nrows();
        return Ok((
            DMatrix::<f64>::identity(n, n),
            DMatrix::<f64>::zeros(n, b_c.ncols()),
        ));
    }

    let a_d = matrix_exp(&(a_c * dt))?;
    let identity = DMatrix::<f64>::identity(a_c.nrows(), a_c.ncols());
    let rhs = (&a_d - identity) * b_c;
    let lu = a_c.clone().lu();
    let b_d = lu
        .solve(&rhs)
        .ok_or(StateSpaceError::SingularContinuousMatrix)?;

    Ok((a_d, b_d))
}

/// Van Loan augmented-matrix discretization fallback for singular A_c.
pub fn van_loan_discretize(
    a_c: &DMatrix<f64>,
    b_c: &DMatrix<f64>,
    dt: f64,
) -> Result<(DMatrix<f64>, DMatrix<f64>)> {
    validate_continuous_dimensions(a_c, b_c)?;

    if dt == 0.0 {
        let n = a_c.nrows();
        return Ok((
            DMatrix::<f64>::identity(n, n),
            DMatrix::<f64>::zeros(n, b_c.ncols()),
        ));
    }

    let n = a_c.nrows();
    let m = b_c.ncols();
    let mut block = DMatrix::<f64>::zeros(n + m, n + m);

    for row in 0..n {
        for col in 0..n {
            block[(row, col)] = a_c[(row, col)];
        }
        for col in 0..m {
            block[(row, n + col)] = b_c[(row, col)];
        }
    }

    let expm = matrix_exp(&(block * dt))?;

    let mut a_d = DMatrix::<f64>::zeros(n, n);
    let mut b_d = DMatrix::<f64>::zeros(n, m);

    for row in 0..n {
        for col in 0..n {
            a_d[(row, col)] = expm[(row, col)];
        }
        for col in 0..m {
            b_d[(row, col)] = expm[(row, n + col)];
        }
    }

    Ok((a_d, b_d))
}

/// Matrix exponential via Padé (order-13) scaling-and-squaring.
pub fn matrix_exp(m: &DMatrix<f64>) -> Result<DMatrix<f64>> {
    if m.nrows() != m.ncols() {
        return Err(StateSpaceError::DimensionMismatch(format!(
            "matrix_exp requires a square matrix, got {}x{}",
            m.nrows(),
            m.ncols()
        )));
    }

    let n = m.nrows();
    if n == 0 {
        return Ok(DMatrix::zeros(0, 0));
    }

    let norm_1 = matrix_one_norm(m);
    if norm_1 == 0.0 {
        return Ok(DMatrix::<f64>::identity(n, n));
    }

    let theta_13 = 5.371_920_351_148_152_f64;
    let s = ((norm_1 / theta_13).log2().ceil().max(0.0)) as u32;
    let scale = 2_f64.powi(-(s as i32));
    let a = m * scale;

    let a2 = &a * &a;
    let a4 = &a2 * &a2;
    let a6 = &a4 * &a2;
    let identity = DMatrix::<f64>::identity(n, n);

    let b = [
        64_764_752_532_480_000.0,
        32_382_376_266_240_000.0,
        7_771_770_303_897_600.0,
        1_187_353_796_428_800.0,
        129_060_195_264_000.0,
        10_559_470_521_600.0,
        670_442_572_800.0,
        33_522_128_640.0,
        1_323_241_920.0,
        40_840_800.0,
        960_960.0,
        16_380.0,
        182.0,
        1.0,
    ];

    let u_inner = &a6 * (&a6 * b[13] + &a4 * b[11] + &a2 * b[9])
        + &a6 * b[7]
        + &a4 * b[5]
        + &a2 * b[3]
        + &identity * b[1];
    let u = &a * u_inner;

    let v = &a6 * (&a6 * b[12] + &a4 * b[10] + &a2 * b[8])
        + &a6 * b[6]
        + &a4 * b[4]
        + &a2 * b[2]
        + &identity * b[0];

    let p = &v + &u;
    let q = &v - &u;

    let lu = q.lu();
    let mut r = lu.solve(&p).ok_or(StateSpaceError::SingularPadeMatrix)?;

    for _ in 0..s {
        r = &r * &r;
    }

    Ok(r)
}

/// Checks RC stability; returns `Err(StabilityResult)` for recoverable failure.
pub fn eigenvalue_check(
    a_c: &DMatrix<f64>,
    a_d: &DMatrix<f64>,
) -> std::result::Result<StabilityResult, StabilityResult> {
    let continuous_eigs = a_c.clone().complex_eigenvalues();
    let discrete_eigs = a_d.clone().complex_eigenvalues();

    let continuous_stable = continuous_eigs.iter().all(|lambda| lambda.re < 0.0);
    let discrete_stable = discrete_eigs.iter().all(|lambda| lambda.norm() < 1.0);

    let near_unity_eigenvalues = discrete_eigs
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, lambda)| lambda.norm() > NEAR_UNITY_EIGENVALUE_THRESHOLD)
        .collect::<Vec<_>>();

    for (index, eigenvalue) in &near_unity_eigenvalues {
        tracing::warn!(
            eigen_index = *index,
            eigen_real = eigenvalue.re,
            eigen_imag = eigenvalue.im,
            magnitude = eigenvalue.norm(),
            "Near-unity discrete eigenvalue detected; convergence may be slow"
        );
    }

    let result = StabilityResult {
        continuous_stable,
        discrete_stable,
        near_unity_eigenvalues,
    };

    if result.continuous_stable && result.discrete_stable {
        Ok(result)
    } else {
        Err(result)
    }
}

/// Conservative upper bound on the spectral radius via Gershgorin circle theorem.
///
/// Computes max over rows of `|a_ii| + Σ_{j≠i} |a_ij|` (the matrix infinity-norm).
/// This is always ≥ the true spectral radius; equality holds for diagonal matrices.
/// Runs in O(n²) time.
pub fn gershgorin_spectral_radius(a: &DMatrix<f64>) -> f64 {
    let n = a.nrows();
    let mut max_row_sum = 0.0_f64;
    for i in 0..n {
        let center = a[(i, i)].abs();
        let radius: f64 = (0..n).filter(|&j| j != i).map(|j| a[(i, j)].abs()).sum();
        max_row_sum = max_row_sum.max(center + radius);
    }
    max_row_sum
}

fn matrix_one_norm(m: &DMatrix<f64>) -> f64 {
    (0..m.ncols())
        .map(|col| (0..m.nrows()).map(|row| m[(row, col)].abs()).sum::<f64>())
        .fold(0.0, f64::max)
}

fn reciprocal_condition_estimate_1_norm(a: &DMatrix<f64>, a_inv: &DMatrix<f64>) -> f64 {
    let norm_a = matrix_one_norm(a);
    let norm_a_inv = matrix_one_norm(a_inv);

    if norm_a == 0.0 || norm_a_inv == 0.0 {
        return 0.0;
    }

    1.0 / (norm_a * norm_a_inv)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_matrix_close(actual: &DMatrix<f64>, expected: &DMatrix<f64>, tol: f64) {
        assert_eq!(actual.shape(), expected.shape());
        for row in 0..actual.nrows() {
            for col in 0..actual.ncols() {
                let delta = (actual[(row, col)] - expected[(row, col)]).abs();
                assert!(
                    delta <= tol,
                    "matrix mismatch at ({row}, {col}): actual={}, expected={}, delta={delta}",
                    actual[(row, col)],
                    expected[(row, col)]
                );
            }
        }
    }

    #[test]
    fn matrix_exp_matches_known_rotation_case() {
        let m = DMatrix::from_row_slice(2, 2, &[0.0, 1.0, -1.0, 0.0]);
        let expm = matrix_exp(&m).unwrap();
        let expected = DMatrix::from_row_slice(
            2,
            2,
            &[1.0_f64.cos(), 1.0_f64.sin(), -1.0_f64.sin(), 1.0_f64.cos()],
        );
        assert_matrix_close(&expm, &expected, 1.0e-12);
    }

    #[test]
    fn one_r_one_c_matches_analytic_solution() {
        let r = 1.0;
        let c = 1000.0;
        let dt = 60.0;
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (r * c)]);
        let b_c = DMatrix::from_row_slice(1, 1, &[1.0 / (r * c)]);

        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };

        let model = StateSpaceModel::from_continuous(&a_c, &b_c, dt, &mapping)
            .expect("state-space model should build");

        let mut x = DVector::from_row_slice(&[20.0]);
        let u = DVector::from_row_slice(&[0.0]);

        for _ in 0..100 {
            x = model.step(&x, &u);
        }

        let t = 100.0 * dt;
        let expected = 0.0 + (20.0 - 0.0) * (-t / (r * c)).exp();
        assert!((x[0] - expected).abs() < 0.01);
    }

    #[test]
    fn three_r_two_c_reference_matches_expected_discretization_and_output_mappings() {
        // Reference values from scipy.signal.cont2discrete(method='zoh') for this A_c/B_c.
        let a_c = DMatrix::from_row_slice(2, 2, &[-0.020, 0.0, 0.0, -0.010]);
        let b_c = DMatrix::from_row_slice(
            2,
            4,
            &[0.010, 0.005, 0.000, 0.002, 0.000, 0.004, 0.006, 0.001],
        );

        let mapping = OutputMapping {
            output_count: 2,
            node_to_output: vec![(0, 0, 1.0), (1, 1, 1.0)],
            input_to_output: vec![(0, 2, 0.25), (1, 3, 0.10)],
        };

        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping)
            .expect("state-space model should build");

        let ad_expected = DMatrix::from_row_slice(
            2,
            2,
            &[0.301_194_211_912_202, 0.0, 0.0, 0.548_811_636_094_026],
        );
        let bd_expected = DMatrix::from_row_slice(
            2,
            4,
            &[
                0.349_402_894_043_899,
                0.174_701_447_021_949,
                0.0,
                0.069_880_578_808_780,
                0.0,
                0.180_475_345_562_389,
                0.270_713_018_343_583,
                0.045_118_836_390_597,
            ],
        );
        let c_expected = DMatrix::from_row_slice(2, 2, &[1.0, 0.0, 0.0, 1.0]);
        let d_expected = DMatrix::from_row_slice(2, 4, &[0.0, 0.0, 0.25, 0.0, 0.0, 0.0, 0.0, 0.10]);

        assert_matrix_close(&model.a_d, &ad_expected, 1.0e-12);
        assert_matrix_close(&model.b_d, &bd_expected, 1.0e-12);
        assert_matrix_close(&model.c, &c_expected, 1.0e-12);
        assert_matrix_close(&model.d, &d_expected, 1.0e-12);
    }

    #[test]
    fn singular_a_uses_van_loan_fallback_and_matches_expected() {
        let a_c = DMatrix::from_row_slice(2, 2, &[0.0, 0.0, 0.0, -1.0]);
        let b_c = DMatrix::from_row_slice(2, 1, &[1.0, 2.0]);

        let zoh = discretize_zoh(&a_c, &b_c, 1.0);
        assert!(matches!(
            zoh,
            Err(StateSpaceError::SingularContinuousMatrix)
        ));

        let (a_d, b_d) = van_loan_discretize(&a_c, &b_c, 1.0)
            .expect("van_loan_discretize should succeed for valid dimensions");
        let a_expected = DMatrix::from_row_slice(2, 2, &[1.0, 0.0, 0.0, (-1.0_f64).exp()]);
        let b_expected = DMatrix::from_row_slice(2, 1, &[1.0, 2.0 * (1.0 - (-1.0_f64).exp())]);

        assert_matrix_close(&a_d, &a_expected, 1.0e-12);
        assert_matrix_close(&b_d, &b_expected, 1.0e-12);

        let stability = eigenvalue_check(&a_c, &a_d)
            .expect_err("degenerate singular case should fail strict stability check");
        assert!(!stability.continuous_stable);
        assert!(!stability.discrete_stable);
        assert!(!stability.near_unity_eigenvalues.is_empty());
    }

    #[test]
    fn unstable_continuous_system_returns_err_stability_result() {
        let a_c = DMatrix::from_row_slice(1, 1, &[0.1]);
        let a_d = matrix_exp(&(&a_c * 1.0)).unwrap();

        let stability = eigenvalue_check(&a_c, &a_d)
            .expect_err("positive continuous eigenvalue should fail stability");

        assert!(!stability.continuous_stable);
        assert!(!stability.discrete_stable);
    }

    #[test]
    fn solve_for_input_recovers_exact_scalar_with_non_zero_d_column() {
        let model = StateSpaceModel::from_discrete(
            DMatrix::from_row_slice(1, 1, &[0.9]),
            DMatrix::from_row_slice(1, 2, &[0.05, 0.20]),
            DMatrix::from_row_slice(1, 1, &[1.0]),
            DMatrix::from_row_slice(1, 2, &[0.0, 0.10]),
        )
        .expect("discrete model should build");

        let x = DVector::from_row_slice(&[10.0]);
        let mut u = DVector::from_row_slice(&[3.0, 0.0]);

        let known_u_i = 4.25;
        u[1] = known_u_i;
        let y_target = model.output(&model.step(&x, &u), &u)[0];

        u[1] = 0.0;
        let solved = model
            .solve_for_input(&x, &u, y_target, 1)
            .expect("input should be solvable");

        assert!((solved - known_u_i).abs() < 1.0e-9);

        u[1] = solved;
        let y_roundtrip = model.output(&model.step(&x, &u), &u)[0];
        assert!((y_roundtrip - y_target).abs() < 1.0e-9);
    }

    #[test]
    fn discrete_eigenvalues_match_continuous_exponential() {
        // Standard control theory: eigenvalues of A_d = exp(eigenvalues(A_c) * dt)
        // For a 2x2 system with known eigenvalues
        let a_c = DMatrix::from_row_slice(2, 2, &[-2.0, 0.0, 0.0, -3.0]);
        let b_c = DMatrix::from_row_slice(2, 1, &[1.0, 1.0]);
        let dt = 0.5;
        let (a_d, _) = discretize_zoh(&a_c, &b_c, dt).unwrap();

        // Eigenvalues of diagonal A_c are -2, -3
        // Expected discrete eigenvalues: exp(-2*0.5) = exp(-1), exp(-3*0.5) = exp(-1.5)
        let eigs = a_d.complex_eigenvalues();
        let mut eig_reals: Vec<f64> = eigs.iter().map(|e| e.re).collect();
        eig_reals.sort_by(|a, b| a.partial_cmp(b).unwrap());

        assert!(
            (eig_reals[0] - (-1.5_f64).exp()).abs() < 1e-10,
            "λ1: {}, expected {}",
            eig_reals[0],
            (-1.5_f64).exp()
        );
        assert!(
            (eig_reals[1] - (-1.0_f64).exp()).abs() < 1e-10,
            "λ2: {}, expected {}",
            eig_reals[1],
            (-1.0_f64).exp()
        );
    }

    #[test]
    fn matrix_exp_2x2_diagonal_matches_scalar_exp() {
        // For diagonal matrix, expm(diag(a,b)) = diag(exp(a), exp(b))
        let m = DMatrix::from_row_slice(2, 2, &[-0.5, 0.0, 0.0, -2.0]);
        let result = matrix_exp(&m).unwrap();
        assert!((result[(0, 0)] - (-0.5_f64).exp()).abs() < 1e-14);
        assert!((result[(1, 1)] - (-2.0_f64).exp()).abs() < 1e-14);
        assert!(result[(0, 1)].abs() < 1e-14);
        assert!(result[(1, 0)].abs() < 1e-14);
    }

    #[test]
    fn from_continuous_accepts_singular_a_c_via_van_loan() {
        // A_c has a zero eigenvalue (row 0 is all zeros), making it singular.
        // The Van Loan fallback produces a valid A_d with eigenvalue 1.0 for
        // that mode, but the second mode (decay at rate -1.0) is strictly
        // stable.  `from_continuous` must accept this because discrete
        // stability holds (|lambda| <= 1.0).
        let a_c = DMatrix::from_row_slice(2, 2, &[0.0, 0.0, 0.0, -1.0]);
        let b_c = DMatrix::from_row_slice(2, 1, &[1.0, 2.0]);

        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };

        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 1.0, &mapping)
            .expect("from_continuous should succeed for singular A_c with discrete-stable A_d");

        let a_expected = DMatrix::from_row_slice(2, 2, &[1.0, 0.0, 0.0, (-1.0_f64).exp()]);
        let b_expected = DMatrix::from_row_slice(2, 1, &[1.0, 2.0 * (1.0 - (-1.0_f64).exp())]);

        assert_matrix_close(&model.a_d, &a_expected, 1.0e-12);
        assert_matrix_close(&model.b_d, &b_expected, 1.0e-12);
    }

    #[test]
    fn verify_stability_returns_stable_for_known_stable_system() {
        let model = StateSpaceModel::from_discrete(
            DMatrix::from_row_slice(2, 2, &[0.5, 0.0, 0.0, 0.3]),
            DMatrix::from_row_slice(2, 1, &[0.1, 0.2]),
            DMatrix::from_row_slice(1, 2, &[1.0, 0.0]),
            DMatrix::from_row_slice(1, 1, &[0.0]),
        )
        .unwrap();

        let verdict = model.verify_stability().unwrap();
        assert_eq!(verdict, StabilityVerdict::Stable);
    }

    #[test]
    fn verify_stability_returns_marginally_unstable_for_eigenvalue_gt_one() {
        let model = StateSpaceModel::from_discrete(
            DMatrix::from_row_slice(2, 2, &[1.5, 0.0, 0.0, 0.3]),
            DMatrix::from_row_slice(2, 1, &[0.1, 0.2]),
            DMatrix::from_row_slice(1, 2, &[1.0, 0.0]),
            DMatrix::from_row_slice(1, 1, &[0.0]),
        )
        .unwrap();

        let verdict = model.verify_stability().unwrap();
        match verdict {
            StabilityVerdict::MarginallyUnstable {
                max_eigenvalue_magnitude,
            } => {
                assert!((max_eigenvalue_magnitude - 1.5).abs() < 1e-10);
            }
            other => panic!("expected MarginallyUnstable, got {other:?}"),
        }
    }

    #[test]
    fn gershgorin_bound_is_tight_for_diagonal_matrix() {
        // For a diagonal matrix, Gershgorin radii are zero so the bound equals
        // the max absolute diagonal entry — exactly the spectral radius.
        let diag = DMatrix::from_row_slice(3, 3, &[0.8, 0.0, 0.0, 0.0, -0.5, 0.0, 0.0, 0.0, 0.9]);
        let bound = gershgorin_spectral_radius(&diag);
        assert!((bound - 0.9).abs() < 1e-14, "expected 0.9, got {bound}");
    }
}
