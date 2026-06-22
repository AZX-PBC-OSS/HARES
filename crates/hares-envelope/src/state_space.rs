//! State-space representation with ZOH discretization and implicit-coupling support.

use nalgebra::linalg::LU;
use nalgebra::{Complex, DMatrix, DVector, Dyn};
use thiserror::Error;

#[cfg(feature = "observe_detailed")]
use std::sync::atomic::{AtomicU64, Ordering};

const RCOND_THRESHOLD: f64 = 1.0e-12;
pub const ZERO_GAIN_EPSILON: f64 = 1.0e-12;

/// Discrete eigenvalue magnitude threshold for near-unity stability warning.
/// Eigenvalues with |λ| > this value trigger a tracing::warn for slow convergence.
const NEAR_UNITY_EIGENVALUE_THRESHOLD: f64 = 0.99;

/// Result type for state-space operations.
pub type Result<T> = std::result::Result<T, StateSpaceError>;

/// Pre-allocated scratch buffers for zero-alloc solver methods.
///
/// Both vectors must be `state_dim()` long. They are overwritten on each call.
pub struct SolverScratch {
    pub rhs: DVector<f64>,
    pub gain: DVector<f64>,
}

impl SolverScratch {
    pub fn new(state_dim: usize) -> Self {
        Self {
            rhs: DVector::zeros(state_dim),
            gain: DVector::zeros(state_dim),
        }
    }
}

/// Pre-factorized coupling data for coupled solver methods.
pub struct CouplingData<'a> {
    pub lu: &'a LU<f64, Dyn, Dyn>,
    pub couplings: &'a [(usize, f64, f64)],
}

/// Solve target: which output to drive to what value, and which input to vary.
pub struct SolveTarget {
    pub y_target: f64,
    pub output_index: usize,
    pub input_index: usize,
}

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
    #[error("implicit system matrix (I - dt/2 * A_c) is singular")]
    ImplicitMatrixSingular,
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

/// Discrete-time state-space model (ZOH base step) with optional implicit couplings.
///
/// For `from_continuous`: M = I, N = A_d = expm(A_c·dt), B_eff = B_d = A_c⁻¹·(A_d − I)·B_c.
/// For `from_discrete`: M and N are caller-supplied; B_eff = B_d (caller-supplied).
///
/// The implicit coupling step modifies the diagonal of M to account for
/// infiltration and other conductances that depend on state variables.
///
/// When `m_is_identity` is true (always for `from_continuous` and `from_discrete`
/// constructors), the coupled step uses an O(n) closed-form solve instead of an
/// O(n³) LU factorization: `(I + D)·x = b` ⇒ `x[i] = b[i]` for uncoupled rows,
/// `x[i] = b[i] / (1 + d_i)` for coupled rows.
#[derive(Clone)]
pub struct StateSpaceModel {
    a_c: Option<DMatrix<f64>>,
    b_c: Option<DMatrix<f64>>,
    m_mat: DMatrix<f64>,
    m_lu: LU<f64, Dyn, Dyn>,
    n_mat: DMatrix<f64>,
    b_eff: DMatrix<f64>,
    pub c: DMatrix<f64>,
    pub d: DMatrix<f64>,
    /// Whether `m_mat` is the identity matrix. Set in both constructors;
    /// when true the coupled step and solve take the closed-form O(n)
    /// diagonal-scaling path, avoiding an O(n³) LU factorization.
    pub(crate) m_is_identity: bool,
    /// Gershgorin spectral radius upper bound for the discrete state matrix `A_d`.
    ///
    /// Always set for all construction paths; `bound < 1.0` does not guarantee
    /// stability (Gershgorin is conservative) but `bound >= 1.0` is a strong
    /// instability signal. `tracing::warn!` is emitted at construction when
    /// `bound >= 1.0 + 1e-10`.
    max_discrete_eigenvalue_magnitude: f64,
}

impl std::fmt::Debug for StateSpaceModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StateSpaceModel")
            .field("a_c", &self.a_c)
            .field("b_c", &self.b_c)
            .field("m_mat", &self.m_mat)
            .field("m_lu", &"LU{...}")
            .field("n_mat", &self.n_mat)
            .field("b_eff", &self.b_eff)
            .field("c", &self.c)
            .field("d", &self.d)
            .field("m_is_identity", &self.m_is_identity)
            .field(
                "max_discrete_eigenvalue_magnitude",
                &self.max_discrete_eigenvalue_magnitude,
            )
            .finish()
    }
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

/// Count of Gershgorin false-positive rejections averted by full eigenvalue fallback.
///
/// Incremented each time `from_continuous` initially flags a model as unstable via
/// Gershgorin but the full `eigenvalue_check` confirms stability.
#[cfg(feature = "observe_detailed")]
static GERSHGORIN_FALSE_POSITIVE_COUNT: AtomicU64 = AtomicU64::new(0);

/// Returns the number of Gershgorin false-positive rejections averted since startup.
#[cfg(feature = "observe_detailed")]
pub fn gershgorin_false_positive_count() -> u64 {
    GERSHGORIN_FALSE_POSITIVE_COUNT.load(Ordering::Relaxed)
}

impl StateSpaceModel {
    /// Constructs a fully-discrete model from pre-discretized matrices.
    ///
    /// Sets M = I (identity) so that `step()` degenerates to `A_d·x + B_d·u`.
    pub fn from_discrete(
        a_d: DMatrix<f64>,
        b_d: DMatrix<f64>,
        c: DMatrix<f64>,
        d: DMatrix<f64>,
    ) -> Result<Self> {
        validate_state_space_dimensions(&a_d, &b_d, &c, &d)?;
        let n = a_d.nrows();
        let eye = DMatrix::<f64>::identity(n, n);
        let m_lu = eye.clone().lu();

        let gershgorin_bound = gershgorin_spectral_radius(&a_d);
        if gershgorin_bound >= 1.0 + 1e-10 {
            tracing::warn!(
                gershgorin_bound,
                "Gershgorin discrete spectral bound exceeds unity; system may be unstable"
            );
        }
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            let eye_check = DMatrix::<f64>::identity(n, n);
            debug_assert!(
                eye.relative_eq(&eye_check, 1e-12, 1e-12),
                "from_discrete: M matrix must be identity"
            );
        }
        Ok(Self {
            a_c: None,
            b_c: None,
            m_mat: eye,
            m_lu,
            n_mat: a_d,
            b_eff: b_d,
            c,
            d,
            m_is_identity: true,
            max_discrete_eigenvalue_magnitude: gershgorin_bound,
        })
    }

    /// Number of state variables.
    pub fn state_dim(&self) -> usize {
        self.n_mat.nrows()
    }

    /// Number of inputs.
    pub fn input_dim(&self) -> usize {
        self.b_eff.ncols()
    }

    /// Number of outputs (rows of C).
    pub fn output_dim(&self) -> usize {
        self.c.nrows()
    }

    /// Gershgorin spectral radius upper bound for the discrete state matrix `A_d`.
    ///
    /// Always set for all construction paths; `bound < 1.0` does not guarantee
    /// stability (Gershgorin is conservative) but `bound >= 1.0` is a strong
    /// instability signal. `tracing::warn!` is emitted at construction when
    /// `bound >= 1.0 + 1e-10`.
    pub fn max_discrete_eigenvalue_magnitude(&self) -> f64 {
        self.max_discrete_eigenvalue_magnitude
    }

    /// Whether `m_mat` is the identity matrix.
    ///
    /// When true the coupled step and solve use the O(n) closed-form
    /// diagonal-scaling path instead of an O(n³) LU factorization.
    pub fn m_is_identity(&self) -> bool {
        self.m_is_identity
    }

    /// Implicit-half matrix M (I for continuous-path; caller-supplied for discrete-path).
    pub fn m_mat(&self) -> &DMatrix<f64> {
        &self.m_mat
    }

    /// Explicit-half matrix N = A_d (or caller-supplied for discrete-path).
    pub fn n_mat(&self) -> &DMatrix<f64> {
        &self.n_mat
    }

    /// Discrete input matrix B_d (ZOH-discretized, or caller-supplied for discrete-path).
    pub fn b_eff(&self) -> &DMatrix<f64> {
        &self.b_eff
    }

    /// Continuous-time system matrix, if built from `from_continuous()`.
    pub fn a_c(&self) -> Option<&DMatrix<f64>> {
        self.a_c.as_ref()
    }

    /// Continuous-time input matrix, if built from `from_continuous()`.
    pub fn b_c(&self) -> Option<&DMatrix<f64>> {
        self.b_c.as_ref()
    }

    /// Computes the steady-state solution for constant input u.
    ///
    /// Continuous path: solves `A_c·x = -B_c·u`.
    /// Discrete path: solves `(I - N)·x = B_eff·u`.
    /// Returns None if the system matrix is singular (e.g., integrating system).
    pub fn steady_state(&self, u: &DVector<f64>) -> Option<DVector<f64>> {
        if let (Some(a_c), Some(b_c)) = (&self.a_c, &self.b_c) {
            let rhs = -(b_c * u);
            a_c.clone().try_inverse().map(|inv| inv * rhs)
        } else {
            // This branch is only correct when n_mat = A_d (from_discrete path).
            // For from_continuous models, a_c/b_c are always Some and the branch above handles it.
            debug_assert!(
                self.a_c.is_none(),
                "discrete steady_state branch reached with a_c present"
            );
            let n = self.state_dim();
            let eye = DMatrix::<f64>::identity(n, n);
            let lhs = eye - &self.n_mat;
            let rhs = &self.b_eff * u;
            lhs.try_inverse().map(|inv| inv * rhs)
        }
    }

    /// Constructs from continuous A_c, B_c using ZOH (matrix exponential) discretization.
    ///
    /// ZOH is unconditionally stable and matches OCHRE's approach. The step becomes:
    /// `x[k+1] = A_d·x[k] + B_d·u[k]` with `M = I` (no implicit solve needed for the
    /// base step). Semi-implicit infiltration coupling adds to M and N via the coupling API.
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

        let n = a_c.nrows();
        let (c, d) = build_output_matrices(n, b_c.ncols(), output_mapping)?;

        let (a_d, b_d) = discretize_auto(a_c, b_c, dt)?;

        let eye = DMatrix::<f64>::identity(n, n);

        // Stability check on discrete A_d.
        //
        // Tiered approach:
        //   1. Fast Gershgorin pass (O(n²)) for the common stable case.
        //   2. Singular A_c exemption: ZOH discretization produces pole at |λ|=1,
        //      but CN implicit path remains well-behaved.
        //   3. Full eigenvalue fallback: Gershgorin is sufficient but not
        //      necessary — strong off-diagonal coupling (e.g. multi-zone RC
        //      networks) can overestimate the spectral radius above 1.0 even
        //      when all true eigenvalues lie within the unit circle.
        let continuous_stable = gershgorin_continuous_stable(a_c);
        let gershgorin_bound = gershgorin_spectral_radius(&a_d);
        let discrete_stable = gershgorin_bound < 1.0 + 1e-10;

        if !continuous_stable || !discrete_stable {
            let a_c_singular = is_singular(a_c);
            if a_c_singular && gershgorin_bound <= 1.0 + 1e-10 {
                // Existing exemption: singular A_c within Gershgorin tolerance.
                // ZOH discretization produces a pole at |λ| = 1, but the CN
                // implicit path stays well-behaved and stable.
            } else {
                // Gershgorin flagged potential instability — attempt full
                // eigenvalue decomposition before rejecting.
                let eigen_ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    eigenvalue_check(a_c, &a_d)
                }));
                match eigen_ok {
                    Ok(Ok(stable_result)) => {
                        // False positive: Gershgorin bound exceeded unity but
                        // all true eigenvalues are within the unit circle.
                        tracing::warn!(
                            gershgorin_bound,
                            continuous_stable = stable_result.continuous_stable,
                            discrete_stable = stable_result.discrete_stable,
                            "Gershgorin spectral bound exceeded unity but full eigenvalue check confirms stability; proceeding"
                        );
                        #[cfg(feature = "observe_detailed")]
                        GERSHGORIN_FALSE_POSITIVE_COUNT.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(Err(unstable)) => {
                        return Err(StateSpaceError::UnstableSystem(unstable));
                    }
                    Err(_) => {
                        // Schur QR panicked; fall back to conservative
                        // rejection based on Gershgorin bounds.
                        return Err(StateSpaceError::UnstableSystem(StabilityResult {
                            continuous_stable,
                            discrete_stable,
                            near_unity_eigenvalues: Vec::new(),
                        }));
                    }
                }
            }
        }

        if gershgorin_bound > NEAR_UNITY_EIGENVALUE_THRESHOLD {
            tracing::warn!(
                gershgorin_bound,
                "Gershgorin discrete spectral bound near unity; convergence may be slow"
            );
        }

        // Invariant check removed: calling eigenvalue_check unconditionally on every
        // from_continuous call stalls on large RC matrices (nalgebra Schur QR has
        // unlimited iterations). The fallback path above already runs eigenvalue_check
        // when Gershgorin flags instability — the common stable path does not need it.

        let m_lu = eye.clone().lu();

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            let eye_check = DMatrix::<f64>::identity(n, n);
            debug_assert!(
                eye.relative_eq(&eye_check, 1e-12, 1e-12),
                "from_continuous: M matrix must be identity"
            );
        }

        Ok(Self {
            a_c: Some(a_c.clone()),
            b_c: Some(b_c.clone()),
            m_mat: eye,
            m_lu,
            n_mat: a_d,
            b_eff: b_d,
            c,
            d,
            m_is_identity: true,
            max_discrete_eigenvalue_magnitude: gershgorin_bound,
        })
    }

    /// Zero-allocation step: x[k+1] = M⁻¹·(N·x[k] + B_eff·u[k]).
    pub fn step_into(&self, x: &DVector<f64>, u: &DVector<f64>, buf: &mut DVector<f64>) {
        buf.gemv(1.0, &self.n_mat, x, 0.0); // buf = N·x
        buf.gemv(1.0, &self.b_eff, u, 1.0); // buf += B_eff·u
        self.m_lu.solve_mut(buf);
    }

    /// Zero-allocation step with explicit state-dependent forcing: x[k+1] = M⁻¹·(N·x[k] + B_eff·u[k] + f).
    ///
    /// The forcing vector `f` (dimension = state_dim) is added to the RHS before solving.
    /// This supports explicit treatment of nonlinear effects (e.g., ΔT-dependent
    /// convective film coefficients) without modifying the static A-matrix.
    // Retained for the explicit forcing path; unused until the Courant-condition
    // constraint blocking PerStepTarp activation (T-0034 Known Limitations) is resolved.
    #[allow(dead_code)]
    pub fn step_into_with_forcing(
        &self,
        x: &DVector<f64>,
        u: &DVector<f64>,
        buf: &mut DVector<f64>,
        forcing: &DVector<f64>,
    ) {
        debug_assert_eq!(
            forcing.len(),
            self.state_dim(),
            "forcing vector dimension mismatch"
        );
        buf.gemv(1.0, &self.n_mat, x, 0.0); // buf = N·x
        buf.gemv(1.0, &self.b_eff, u, 1.0); // buf += B_eff·u
        *buf += forcing; // buf += f (explicit forcing)
        self.m_lu.solve_mut(buf);
    }

    /// Convenience step that allocates a new vector (use `step_into` for hot paths).
    pub fn step(&self, x: &DVector<f64>, u: &DVector<f64>) -> DVector<f64> {
        let mut buf = DVector::zeros(x.len());
        self.step_into(x, u, &mut buf);
        buf
    }

    /// Computes output from current state/input: y[k] = C * x[k] + D * u[k].
    pub fn output(&self, x: &DVector<f64>, u: &DVector<f64>) -> DVector<f64> {
        &self.c * x + &self.d * u
    }

    /// Full eigenvalue stability check on the equivalent discrete state matrix.
    ///
    /// Computes A_d = M⁻¹·N and checks all eigenvalue magnitudes.
    pub fn verify_stability(&self) -> Result<StabilityVerdict> {
        let n = self.state_dim();
        let eye = DMatrix::<f64>::identity(n, n);
        let m_inv = self
            .m_lu
            .solve(&eye)
            .ok_or(StateSpaceError::ImplicitMatrixSingular)?;
        let a_d_equiv = m_inv * &self.n_mat;
        let eigs = a_d_equiv.complex_eigenvalues();
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
    /// after one implicit step.
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

    /// Builds the coupled RHS into `buf`: `(N - D)·x + B_eff·u + f`.
    ///
    /// Each coupling entry `(state_idx, d, forcing)` subtracts `d * x[idx]`
    /// from the explicit half and adds `forcing` to the RHS.
    pub fn build_coupled_rhs(
        &self,
        x: &DVector<f64>,
        u: &DVector<f64>,
        buf: &mut DVector<f64>,
        couplings: &[(usize, f64, f64)],
    ) {
        buf.gemv(1.0, &self.n_mat, x, 0.0); // buf = N·x
        for &(idx, d, _) in couplings {
            debug_assert!(idx < self.state_dim(), "coupling index {idx} out of bounds");
            buf[idx] -= d * x[idx]; // subtract D·x from explicit half
        }
        buf.gemv(1.0, &self.b_eff, u, 1.0); // buf += B_eff·u
        for &(idx, _, forcing) in couplings {
            buf[idx] += forcing; // add forcing
        }
    }

    /// Coupled discrete step with per-step diagonal coupling, using a pre-built LU.
    ///
    /// Builds the RHS `(N - D)·x + B_eff·u + f` into `buf`, then solves
    /// `(M + D)·x[k+1] = buf` using the provided LU factorization.
    ///
    /// Use `build_coupled_lu` to create `coupled_lu` once per step, then
    /// pass it to both this method and `solve_for_scalar_input_coupled`.
    pub fn step_with_coupled_lu_into(
        &self,
        x: &DVector<f64>,
        u: &DVector<f64>,
        buf: &mut DVector<f64>,
        coupled_lu: &LU<f64, Dyn, Dyn>,
        couplings: &[(usize, f64, f64)],
    ) {
        self.build_coupled_rhs(x, u, buf, couplings);
        coupled_lu.solve_mut(buf);
    }

    /// Coupled discrete step with per-step diagonal coupling and explicit forcing.
    ///
    /// Same as [`step_with_coupled_lu_into`] but adds the forcing vector `f` to
    /// the RHS before solving. See [`step_into_with_forcing`] for rationale.
    // Retained for the explicit forcing path; unused until the Courant-condition
    // constraint blocking PerStepTarp activation (T-0034 Known Limitations) is resolved.
    #[allow(dead_code)]
    pub fn step_with_coupled_lu_into_with_forcing(
        &self,
        x: &DVector<f64>,
        u: &DVector<f64>,
        buf: &mut DVector<f64>,
        coupled_lu: &LU<f64, Dyn, Dyn>,
        couplings: &[(usize, f64, f64)],
        forcing: &DVector<f64>,
    ) {
        debug_assert_eq!(
            forcing.len(),
            self.state_dim(),
            "forcing vector dimension mismatch"
        );
        self.build_coupled_rhs(x, u, buf, couplings);
        *buf += forcing;
        coupled_lu.solve_mut(buf);
    }

    /// Coupled discrete step with identity M and diagonal coupling (closed-form O(n) solve).
    ///
    /// M = I, so `(I + D)·x = b` ⇒ `x[i] = b[i]` for uncoupled rows,
    /// `x[i] = b[i] / (1 + d_i)` for coupled rows. Builds the RHS via
    /// [`build_coupled_rhs`] then applies diagonal scaling.
    ///
    /// Zero allocations — `gemv` uses pre-allocated buffers and the scaling
    /// loop is O(k) over coupled rows.
    ///
    /// # Safety
    ///
    /// Caller must ensure `self.m_is_identity` is true. The method asserts this
    /// in debug builds.
    ///
    /// Vendor alignment: OCHRE StateSpaceModel.py `update_model()` (line 318)
    /// uses `A·x + B·u` forward multiplication with no per-step factorization.
    /// EnergyPlus infiltration enters as explicit ΣMCp terms, not matrix
    /// modifications.
    pub fn step_with_identity_coupling_into(
        &self,
        x: &DVector<f64>,
        u: &DVector<f64>,
        buf: &mut DVector<f64>,
        couplings: &[(usize, f64, f64)],
    ) {
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        debug_assert!(
            self.m_is_identity,
            "step_with_identity_coupling_into: M must be identity"
        );

        self.build_coupled_rhs(x, u, buf, couplings);

        // Aggregate diagonal damping per state index before dividing.
        //
        // Multiple coupling entries for the same state (e.g. infiltration +
        // linearised exterior LWR) must be summed before the implicit
        // division to maintain the correct semi-implicit scheme:
        //
        //   x_next[i] = rhs[i] / (1 + Σ d_j)   for all couplings j at state i
        //
        // Applying `buf[i] /= 1 + d_j` sequentially for each entry gives
        // `rhs[i] / Π(1 + d_j)`, which is incorrect when more than one
        // coupling acts on the same state.
        let n = self.state_dim();
        let mut d_agg = vec![0.0f64; n];
        for &(idx, d_diag, _) in couplings {
            debug_assert!(idx < n, "coupling index {idx} out of bounds");
            d_agg[idx] += d_diag;
        }
        for i in 0..n {
            if d_agg[i] != 0.0 {
                buf[i] /= 1.0 + d_agg[i];
            }
        }
    }

    /// Coupled discrete step with per-step diagonal coupling (convenience: builds LU internally).
    ///
    /// Dispatches to [`step_with_identity_coupling_into`] when `m_is_identity` is true
    /// (the common production path), falling back to O(n³) LU factorization only when
    /// M ≠ I (test-only `from_discrete` models with non-identity M).
    pub fn step_with_coupling_into(
        &self,
        x: &DVector<f64>,
        u: &DVector<f64>,
        buf: &mut DVector<f64>,
        m_scratch: &mut DMatrix<f64>,
        couplings: &[(usize, f64, f64)],
    ) {
        if self.m_is_identity {
            self.step_with_identity_coupling_into(x, u, buf, couplings);
        } else {
            let lu = self.build_coupled_lu(m_scratch, couplings);
            self.step_with_coupled_lu_into(x, u, buf, &lu, couplings);
        }
    }

    /// Builds the LU factorization of the coupled implicit matrix M + D.
    ///
    /// Used when the same coupling needs to be applied to both the step and the
    /// HVAC solve in the same timestep. Reuses `m_scratch` as working storage.
    pub fn build_coupled_lu(
        &self,
        m_scratch: &mut DMatrix<f64>,
        couplings: &[(usize, f64, f64)],
    ) -> LU<f64, Dyn, Dyn> {
        m_scratch.clone_from(&self.m_mat);
        for &(idx, d_diag, _) in couplings {
            debug_assert!(idx < self.state_dim(), "coupling index {idx} out of bounds");
            m_scratch[(idx, idx)] += d_diag;
        }
        // Take the matrix content (leaves m_scratch as 0x0), factorize without clone.
        // m_scratch will be rebuilt from m_mat on next call via clone_from anyway.
        std::mem::take(m_scratch).lu()
    }

    /// Like `solve_for_scalar_input` but with per-step diagonal coupling.
    ///
    /// Applies the same D perturbation and forcing as `step_with_coupling_into`:
    ///   RHS uses (N - D)·x instead of N·x, plus forcing terms.
    ///   LHS uses the pre-built coupled LU factorization of (M + D).
    ///
    /// `couplings` entries are `(state_idx, d_diag, forcing)` -- same format as
    /// `step_with_coupling_into`.
    pub fn solve_for_scalar_input_coupled(
        &self,
        x: &DVector<f64>,
        u: &DVector<f64>,
        y_target: f64,
        output_index: usize,
        input_index: usize,
        coupling: &CouplingData<'_>,
    ) -> Result<f64> {
        let m_coupled_lu = coupling.lu;
        let couplings = coupling.couplings;
        if output_index >= self.c.nrows() {
            return Err(StateSpaceError::OutputIndexOutOfBounds {
                output_index,
                output_dim: self.c.nrows(),
            });
        }
        if input_index >= self.b_eff.ncols() {
            return Err(StateSpaceError::InputIndexOutOfBounds {
                index: input_index,
                input_dim: self.b_eff.ncols(),
            });
        }

        let u_i_original = u[input_index];

        // Build RHS with coupling: (N - D)·x + B_eff·u_fixed + f
        let mut rhs_fixed =
            &self.n_mat * x + &self.b_eff * u - self.b_eff.column(input_index) * u_i_original;
        for &(idx, d_diag, forcing) in couplings {
            rhs_fixed[idx] -= d_diag * x[idx]; // subtract D·x
            rhs_fixed[idx] += forcing; // add forcing
        }

        let x_next_fixed = m_coupled_lu
            .solve(&rhs_fixed)
            .ok_or(StateSpaceError::ImplicitMatrixSingular)?;

        // Gain: how much does x_next change per unit of u[input_index]?
        let g = m_coupled_lu
            .solve(&self.b_eff.column(input_index).into_owned())
            .ok_or(StateSpaceError::ImplicitMatrixSingular)?;

        let c_row = self.c.row(output_index);
        let d_row = self.d.row(output_index);
        let y_fixed =
            (c_row * &x_next_fixed)[0] + (d_row * u)[0] - d_row[input_index] * u_i_original;

        let effective_gain = (c_row * &g)[0] + self.d[(output_index, input_index)];

        if effective_gain.abs() <= ZERO_GAIN_EPSILON {
            return Err(StateSpaceError::ZeroEffectiveGain { input_index });
        }

        Ok((y_target - y_fixed) / effective_gain)
    }

    /// Like [`solve_for_scalar_input_coupled`] but for the identity M case (closed-form O(n) solve).
    ///
    /// Instead of LU factorization, solves `(I + D)·x_next = rhs` and `(I + D)·g = b_col`
    /// by diagonal scaling: `x[i] = rhs[i] / (1 + d_i)` for coupled rows,
    /// `x[i] = rhs[i]` for uncoupled rows.
    pub fn solve_for_scalar_input_identity_coupled(
        &self,
        x: &DVector<f64>,
        u: &DVector<f64>,
        y_target: f64,
        output_index: usize,
        input_index: usize,
        couplings: &[(usize, f64, f64)],
    ) -> Result<f64> {
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        debug_assert!(
            self.m_is_identity,
            "solve_for_scalar_input_identity_coupled: M must be identity"
        );

        if output_index >= self.c.nrows() {
            return Err(StateSpaceError::OutputIndexOutOfBounds {
                output_index,
                output_dim: self.c.nrows(),
            });
        }
        if input_index >= self.b_eff.ncols() {
            return Err(StateSpaceError::InputIndexOutOfBounds {
                index: input_index,
                input_dim: self.b_eff.ncols(),
            });
        }

        let u_i_original = u[input_index];

        // Build RHS with coupling: (N - D)·x + B_eff·u_fixed + f
        let mut rhs_fixed =
            &self.n_mat * x + &self.b_eff * u - self.b_eff.column(input_index) * u_i_original;
        for &(idx, d_diag, forcing) in couplings {
            rhs_fixed[idx] -= d_diag * x[idx];
            rhs_fixed[idx] += forcing;
        }

        // Closed-form solve: (I + D)⁻¹ · rhs_fixed
        for &(idx, d_diag, _) in couplings {
            rhs_fixed[idx] /= 1.0 + d_diag;
        }

        // Gain: g = (I + D)⁻¹ · b_col
        let mut g = self.b_eff.column(input_index).into_owned();
        for &(idx, d_diag, _) in couplings {
            g[idx] /= 1.0 + d_diag;
        }

        let c_row = self.c.row(output_index);
        let d_row = self.d.row(output_index);
        let y_fixed = (c_row * &rhs_fixed)[0] + (d_row * u)[0] - d_row[input_index] * u_i_original;

        let effective_gain = (c_row * &g)[0] + self.d[(output_index, input_index)];

        if effective_gain.abs() <= ZERO_GAIN_EPSILON {
            return Err(StateSpaceError::ZeroEffectiveGain { input_index });
        }

        Ok((y_target - y_fixed) / effective_gain)
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
        if input_index >= self.b_eff.ncols() {
            return Err(StateSpaceError::InputIndexOutOfBounds {
                index: input_index,
                input_dim: self.b_eff.ncols(),
            });
        }

        let u_i_original = u[input_index];

        // Compute rhs without the variable input contribution
        let rhs_fixed =
            &self.n_mat * x + &self.b_eff * u - self.b_eff.column(input_index) * u_i_original;
        let x_next_fixed = self
            .m_lu
            .solve(&rhs_fixed)
            .ok_or(StateSpaceError::ImplicitMatrixSingular)?;

        // Gain: how much does x_next change per unit of u[input_index]?
        let g = self
            .m_lu
            .solve(&self.b_eff.column(input_index).into_owned())
            .ok_or(StateSpaceError::ImplicitMatrixSingular)?;

        let c_row = self.c.row(output_index);
        let d_row = self.d.row(output_index);
        let y_fixed =
            (c_row * &x_next_fixed)[0] + (d_row * u)[0] - d_row[input_index] * u_i_original;

        let effective_gain = (c_row * &g)[0] + self.d[(output_index, input_index)];

        if effective_gain.abs() <= ZERO_GAIN_EPSILON {
            return Err(StateSpaceError::ZeroEffectiveGain { input_index });
        }

        Ok((y_target - y_fixed) / effective_gain)
    }

    /// Like `solve_for_scalar_input_coupled` but uses pre-allocated buffers.
    pub fn solve_for_scalar_input_coupled_into(
        &self,
        x: &DVector<f64>,
        u: &DVector<f64>,
        target: &SolveTarget,
        coupling: &CouplingData<'_>,
        scratch: &mut SolverScratch,
    ) -> Result<f64> {
        let y_target = target.y_target;
        let output_index = target.output_index;
        let input_index = target.input_index;
        let m_coupled_lu = coupling.lu;
        let couplings = coupling.couplings;
        let rhs_buf = &mut scratch.rhs;
        let gain_buf = &mut scratch.gain;
        if output_index >= self.c.nrows() {
            return Err(StateSpaceError::OutputIndexOutOfBounds {
                output_index,
                output_dim: self.c.nrows(),
            });
        }
        if input_index >= self.b_eff.ncols() {
            return Err(StateSpaceError::InputIndexOutOfBounds {
                index: input_index,
                input_dim: self.b_eff.ncols(),
            });
        }

        let u_i_original = u[input_index];

        // Build RHS in-place: rhs_buf = N·x + B_eff·u - b_col·u_i - D·x + f
        rhs_buf.gemv(1.0, &self.n_mat, x, 0.0);
        rhs_buf.gemv(1.0, &self.b_eff, u, 1.0);
        let b_col = self.b_eff.column(input_index);
        for i in 0..rhs_buf.len() {
            rhs_buf[i] -= b_col[i] * u_i_original;
        }
        for &(idx, d_diag, forcing) in couplings {
            rhs_buf[idx] -= d_diag * x[idx];
            rhs_buf[idx] += forcing;
        }

        // Solve in-place: rhs_buf = M_coupled⁻¹ · rhs_buf
        if !m_coupled_lu.solve_mut(rhs_buf) {
            return Err(StateSpaceError::ImplicitMatrixSingular);
        }

        // Gain vector in-place: gain_buf = M_coupled⁻¹ · b_col
        gain_buf.copy_from(&b_col);
        if !m_coupled_lu.solve_mut(gain_buf) {
            return Err(StateSpaceError::ImplicitMatrixSingular);
        }

        let c_row = self.c.row(output_index);
        let d_row = self.d.row(output_index);
        let y_fixed = (c_row * &*rhs_buf)[0] + (d_row * u)[0] - d_row[input_index] * u_i_original;

        let effective_gain = (c_row * &*gain_buf)[0] + self.d[(output_index, input_index)];

        if effective_gain.abs() <= ZERO_GAIN_EPSILON {
            return Err(StateSpaceError::ZeroEffectiveGain { input_index });
        }

        Ok((y_target - y_fixed) / effective_gain)
    }

    /// Like `solve_for_scalar_input` but uses pre-allocated buffers.
    pub fn solve_for_output_input_into(
        &self,
        x: &DVector<f64>,
        u: &DVector<f64>,
        target: &SolveTarget,
        scratch: &mut SolverScratch,
    ) -> Result<f64> {
        let y_target = target.y_target;
        let output_index = target.output_index;
        let input_index = target.input_index;
        let rhs_buf = &mut scratch.rhs;
        let gain_buf = &mut scratch.gain;
        if output_index >= self.c.nrows() {
            return Err(StateSpaceError::OutputIndexOutOfBounds {
                output_index,
                output_dim: self.c.nrows(),
            });
        }
        if input_index >= self.b_eff.ncols() {
            return Err(StateSpaceError::InputIndexOutOfBounds {
                index: input_index,
                input_dim: self.b_eff.ncols(),
            });
        }

        let u_i_original = u[input_index];

        // Build RHS in-place: rhs_buf = N·x + B_eff·u - b_col·u_i
        rhs_buf.gemv(1.0, &self.n_mat, x, 0.0);
        rhs_buf.gemv(1.0, &self.b_eff, u, 1.0);
        let b_col = self.b_eff.column(input_index);
        for i in 0..rhs_buf.len() {
            rhs_buf[i] -= b_col[i] * u_i_original;
        }

        // Solve in-place: rhs_buf = M⁻¹ · rhs_buf
        if !self.m_lu.solve_mut(rhs_buf) {
            return Err(StateSpaceError::ImplicitMatrixSingular);
        }

        // Gain vector in-place: gain_buf = M⁻¹ · b_col
        gain_buf.copy_from(&b_col);
        if !self.m_lu.solve_mut(gain_buf) {
            return Err(StateSpaceError::ImplicitMatrixSingular);
        }

        let c_row = self.c.row(output_index);
        let d_row = self.d.row(output_index);
        let y_fixed = (c_row * &*rhs_buf)[0] + (d_row * u)[0] - d_row[input_index] * u_i_original;

        let effective_gain = (c_row * &*gain_buf)[0] + self.d[(output_index, input_index)];

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

pub fn discretize_auto(
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
        tracing::warn!(
            rcond,
            threshold = RCOND_THRESHOLD,
            "A_c is severely ill-conditioned; using Van Loan fallback for discretization"
        );
    } else {
        tracing::warn!("A_c is singular; using Van Loan fallback for discretization");
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

/// Checks if all Gershgorin discs of a matrix lie in the closed left half-plane.
///
/// For each row i, the disc is centered at `a_ii` with radius `Σ_{j≠i} |a_ij|`.
/// If `a_ii + radius <= 0` for all rows, all eigenvalues have `Re(lambda) <= 0`.
/// This is sufficient (but not necessary) for continuous-time stability, and
/// combined with CN A-stability, guarantees discrete stability.
fn gershgorin_continuous_stable(a: &DMatrix<f64>) -> bool {
    let n = a.nrows();
    for i in 0..n {
        let center = a[(i, i)];
        let radius: f64 = (0..n).filter(|&j| j != i).map(|j| a[(i, j)].abs()).sum();
        if center + radius > 1e-10 {
            return false;
        }
    }
    true
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
    fn three_r_two_c_cn_stepping_matches_zoh_reference_within_tolerance() {
        // CN discretization gives different internal matrices than ZOH, but
        // stepping behavior converges to the same steady state and tracks
        // the analytical solution within O(dt²) per step.
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

        // C and D matrices are independent of discretization method
        let c_expected = DMatrix::from_row_slice(2, 2, &[1.0, 0.0, 0.0, 1.0]);
        let d_expected = DMatrix::from_row_slice(2, 4, &[0.0, 0.0, 0.25, 0.0, 0.0, 0.0, 0.0, 0.10]);
        assert_matrix_close(&model.c, &c_expected, 1.0e-12);
        assert_matrix_close(&model.d, &d_expected, 1.0e-12);

        // Step the CN model and ZOH reference to compare behavior
        let (a_d_zoh, b_d_zoh) = discretize_zoh(&a_c, &b_c, 60.0).unwrap();
        let zoh_model =
            StateSpaceModel::from_discrete(a_d_zoh, b_d_zoh, c_expected.clone(), d_expected)
                .unwrap();

        let mut x_cn = DVector::from_row_slice(&[20.0, 15.0]);
        let mut x_zoh = x_cn.clone();
        let u = DVector::from_row_slice(&[30.0, 25.0, 10.0, 5.0]);

        for _ in 0..100 {
            x_cn = model.step(&x_cn, &u);
            x_zoh = zoh_model.step(&x_zoh, &u);
        }

        // After 100 steps (100 min), both should be close to the same steady state
        for i in 0..2 {
            assert!(
                (x_cn[i] - x_zoh[i]).abs() < 0.1,
                "CN vs ZOH diverged at state {i}: cn={}, zoh={}",
                x_cn[i],
                x_zoh[i]
            );
        }
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
    fn from_continuous_accepts_singular_a_c() {
        // A_c has a zero eigenvalue (row 0 is all zeros), making it singular.
        // CN discretization should still succeed: M = I - dt/2·A_c is non-singular
        // even when A_c is singular. The equivalent A_d = M⁻¹·N has eigenvalue 1.0
        // for the zero-eigenvalue mode, which is marginally stable.
        let a_c = DMatrix::from_row_slice(2, 2, &[0.0, 0.0, 0.0, -1.0]);
        let b_c = DMatrix::from_row_slice(2, 1, &[1.0, 2.0]);

        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };

        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 1.0, &mapping)
            .expect("from_continuous should succeed for singular A_c");

        // Verify stepping behavior: state 0 should integrate input,
        // state 1 should decay toward steady state
        let mut x = DVector::from_row_slice(&[0.0, 0.0]);
        let u = DVector::from_row_slice(&[1.0]);

        x = model.step(&x, &u);
        // State 0: with A_c=0, CN gives x[k+1] = x[k] + dt*b_c[0]*u = 0 + 1*1*1 = 1.0
        assert!(
            (x[0] - 1.0).abs() < 1e-10,
            "state 0 should integrate: got {}",
            x[0]
        );
        // State 1: should move toward steady state (positive, since b_c[1]=2 and a_c[1,1]=-1)
        assert!(x[1] > 0.0, "state 1 should increase from 0: got {}", x[1]);
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
        // the max absolute diagonal entry -- exactly the spectral radius.
        let diag = DMatrix::from_row_slice(3, 3, &[0.8, 0.0, 0.0, 0.0, -0.5, 0.0, 0.0, 0.0, 0.9]);
        let bound = gershgorin_spectral_radius(&diag);
        assert!((bound - 0.9).abs() < 1e-14, "expected 0.9, got {bound}");
    }

    #[test]
    fn cn_1r1c_24h_decay_matches_analytical() {
        // 1R1C thermal network: C=500kJ/K, UA=100W/K, τ=5000s
        // Zone starts at 20°C, outdoor at 0°C, free decay for 24h
        let c_th = 500_000.0; // J/K
        let ua = 100.0; // W/K
        let dt = 60.0; // s
        let steps = 1440; // 24h

        let a_c = DMatrix::from_row_slice(1, 1, &[-ua / c_th]);
        let b_c = DMatrix::from_row_slice(1, 1, &[ua / c_th]);

        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };

        let model = StateSpaceModel::from_continuous(&a_c, &b_c, dt, &mapping)
            .expect("1R1C model should build");

        let mut x = DVector::from_row_slice(&[20.0]);
        let u = DVector::from_row_slice(&[0.0]);

        for _ in 0..steps {
            x = model.step(&x, &u);
        }

        let t = (steps as f64) * dt;
        let t_analytical = 20.0 * (-ua / c_th * t).exp();
        assert!(
            (x[0] - t_analytical).abs() < 0.05,
            "CN result {:.6} should match analytical {:.6} within 0.05°C",
            x[0],
            t_analytical
        );
    }

    #[test]
    fn zoh_no_overshoot_where_explicit_overshoots() {
        // Massively stiff system: τ=0.1s with dt=60s (dt/τ=600)
        // Explicit Euler would wildly overshoot; ZOH (exact) decays to zero.
        let a_c = DMatrix::from_row_slice(1, 1, &[-10.0]);
        let b_c = DMatrix::from_row_slice(1, 1, &[10.0]);
        let dt = 60.0;

        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };

        let model = StateSpaceModel::from_continuous(&a_c, &b_c, dt, &mapping)
            .expect("ZOH model should build for stiff system");

        let x0 = DVector::from_row_slice(&[20.0]);
        let u = DVector::from_row_slice(&[0.0]);
        let x_next = model.step(&x0, &u);

        // ZOH is exact: exp(-10*60) ≈ 0, so the state decays to near zero.
        // No sign oscillation (unlike CN), no overshoot (unlike explicit Euler).
        assert!(
            x_next[0].abs() <= 20.0,
            "ZOH magnitude should not exceed initial: |T|={}, expected ≤ 20",
            x_next[0].abs()
        );
        assert!(
            x_next[0].abs() < 0.01,
            "ZOH should decay to near zero for extreme stiffness: got {}",
            x_next[0]
        );

        // Explicit ZOH: for this extreme stiffness, discretize_zoh may succeed
        // but the resulting A_d = exp(-10*60) ≈ 0 (no overshoot either for
        // exact ZOH). Use van_loan as a more general fallback.
        // Instead, manually construct the explicit (forward Euler) discrete model:
        // A_d_fe = I + dt*A_c, B_d_fe = dt*B_c
        let a_d_fe = DMatrix::from_row_slice(1, 1, &[1.0 + dt * (-10.0)]);
        let b_d_fe = DMatrix::from_row_slice(1, 1, &[dt * 10.0]);
        let c_mat = DMatrix::from_row_slice(1, 1, &[1.0]);
        let d_mat = DMatrix::from_row_slice(1, 1, &[0.0]);

        let explicit_model = StateSpaceModel::from_discrete(a_d_fe, b_d_fe, c_mat, d_mat)
            .expect("explicit model should build");

        let x_explicit = explicit_model.step(&x0, &u);
        let overshoots = x_explicit[0] < 0.0 || x_explicit[0] > 20.0;
        assert!(
            overshoots,
            "explicit forward Euler should overshoot: T={}, expected outside [0, 20]",
            x_explicit[0]
        );
    }

    #[test]
    fn step_into_produces_same_result_as_step() {
        let a_c = DMatrix::from_row_slice(2, 2, &[-0.02, 0.01, 0.005, -0.015]);
        let b_c = DMatrix::from_row_slice(2, 2, &[0.01, 0.005, 0.003, 0.007]);

        let mapping = OutputMapping {
            output_count: 2,
            node_to_output: vec![(0, 0, 1.0), (1, 1, 1.0)],
            input_to_output: vec![],
        };

        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping)
            .expect("model should build");

        let x = DVector::from_row_slice(&[20.0, 15.0]);
        let u = DVector::from_row_slice(&[30.0, 25.0]);

        let x_step = model.step(&x, &u);

        let mut x_into = DVector::zeros(2);
        model.step_into(&x, &u, &mut x_into);

        for i in 0..2 {
            assert!(
                (x_step[i] - x_into[i]).abs() == 0.0,
                "step and step_into differ at index {i}: step={}, step_into={}",
                x_step[i],
                x_into[i]
            );
        }
    }

    #[test]
    fn from_discrete_backward_compat() {
        // Verify that from_discrete degenerates to x[k+1] = A_d*x + B_d*u
        let a_d = DMatrix::from_row_slice(2, 2, &[0.9, 0.05, 0.0, 0.85]);
        let b_d = DMatrix::from_row_slice(2, 2, &[0.1, 0.0, 0.0, 0.15]);
        let c_mat = DMatrix::from_row_slice(2, 2, &[1.0, 0.0, 0.0, 1.0]);
        let d_mat = DMatrix::from_row_slice(2, 2, &[0.0, 0.0, 0.0, 0.0]);

        let model = StateSpaceModel::from_discrete(a_d.clone(), b_d.clone(), c_mat, d_mat)
            .expect("discrete model should build");

        let mut x = DVector::from_row_slice(&[20.0, 15.0]);
        let u = DVector::from_row_slice(&[5.0, 3.0]);

        for _ in 0..10 {
            let x_model = model.step(&x, &u);
            let x_manual = &a_d * &x + &b_d * &u;

            for i in 0..2 {
                assert!(
                    (x_model[i] - x_manual[i]).abs() == 0.0,
                    "from_discrete should exactly match A_d*x + B_d*u at index {i}: \
                     model={}, manual={}",
                    x_model[i],
                    x_manual[i]
                );
            }
            x = x_model;
        }
    }

    #[test]
    fn step_with_coupling_no_coupling_matches_step_into() {
        let a_c = DMatrix::from_row_slice(2, 2, &[-0.02, 0.01, 0.005, -0.015]);
        let b_c = DMatrix::from_row_slice(2, 2, &[0.01, 0.005, 0.003, 0.007]);

        let mapping = OutputMapping {
            output_count: 2,
            node_to_output: vec![(0, 0, 1.0), (1, 1, 1.0)],
            input_to_output: vec![],
        };

        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping)
            .expect("model should build");

        let x = DVector::from_row_slice(&[20.0, 15.0]);
        let u = DVector::from_row_slice(&[30.0, 25.0]);

        let mut buf_step = DVector::zeros(2);
        model.step_into(&x, &u, &mut buf_step);

        let mut buf_coupled = DVector::zeros(2);
        let mut m_scratch = DMatrix::zeros(2, 2);
        model.step_with_coupling_into(&x, &u, &mut buf_coupled, &mut m_scratch, &[]);

        for i in 0..2 {
            assert!(
                (buf_step[i] - buf_coupled[i]).abs() < 1e-12,
                "step_into and step_with_coupling_into differ at index {i}: \
                 step={}, coupled={}",
                buf_step[i],
                buf_coupled[i]
            );
        }
    }

    #[test]
    fn step_with_coupling_large_diagonal_damps_state() {
        // Multi-step test: large coupling should damp state faster than uncoupled.
        // We run enough steps for CN oscillations to settle and compare final magnitudes.
        let a_c = DMatrix::from_row_slice(1, 1, &[-0.001]);
        let b_c = DMatrix::from_row_slice(1, 1, &[0.001]);

        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };

        let dt = 60.0;
        let model =
            StateSpaceModel::from_continuous(&a_c, &b_c, dt, &mapping).expect("model should build");

        let x0 = DVector::from_row_slice(&[20.0]);
        let u = DVector::from_row_slice(&[0.0]);

        // Uncoupled: run 10 steps
        let mut x_uncoupled = x0.clone();
        let mut buf = DVector::zeros(1);
        for _ in 0..10 {
            model.step_into(&x_uncoupled, &u, &mut buf);
            x_uncoupled.copy_from(&buf);
        }

        // Coupled: run 10 steps with large diagonal
        let mut x_coupled = x0;
        let mut m_scratch = DMatrix::zeros(1, 1);
        let large_d = 5.0;
        for _ in 0..10 {
            model.step_with_coupling_into(
                &x_coupled,
                &u,
                &mut buf,
                &mut m_scratch,
                &[(0, large_d, 0.0)],
            );
            x_coupled.copy_from(&buf);
        }

        assert!(
            x_coupled[0].abs() < x_uncoupled[0].abs(),
            "after 10 steps, coupled state {} should be closer to zero than uncoupled {}",
            x_coupled[0],
            x_uncoupled[0]
        );
    }

    /// `from_discrete` computes a Gershgorin bound on the caller-supplied `A_d`
    /// and stores it in `max_discrete_eigenvalue_magnitude`. For a clearly
    /// unstable diagonal matrix, the bound must equal the max absolute diagonal
    /// entry and exceed 1.0.
    #[test]
    fn from_discrete_gershgorin_bound_stored_for_all_paths() {
        let a_d = DMatrix::from_row_slice(2, 2, &[1.5, 0.0, 0.0, 0.3]);
        let b_d = DMatrix::from_row_slice(2, 1, &[0.0, 0.0]);
        let c = DMatrix::identity(2, 2);
        let d = DMatrix::zeros(2, 1);

        let model = StateSpaceModel::from_discrete(a_d, b_d, c, d)
            .expect("from_discrete should accept any well-formed matrices");

        let bound = model.max_discrete_eigenvalue_magnitude();
        assert!(
            bound >= 1.0,
            "Gershgorin bound for unstable matrix should exceed 1.0: got {bound}"
        );
        // For a diagonal matrix, Gershgorin radii are zero, so the bound
        // equals the max absolute diagonal entry = 1.5.
        assert!(
            (bound - 1.5).abs() < 1e-14,
            "Gershgorin bound for diagonal [[1.5, 0], [0, 0.3]] should be 1.5: got {bound}"
        );
    }

    /// `from_continuous` calls `reciprocal_condition_estimate_1_norm` inside
    /// `discretize_auto` and emits `tracing::warn!` when `rcond < RCOND_THRESHOLD`,
    /// then falls back to Van Loan discretization. This test verifies the ill-conditioned
    /// path succeeds rather than panics; the warning cannot be asserted without a
    /// tracing subscriber but the behavioral contract (success via fallback) is tested.
    #[test]
    fn from_continuous_ill_conditioned_a_c_succeeds_without_panic() {
        // A_c = diag(-1e-13, -1.0): 1-norm rcond = 1e-13, well below RCOND_THRESHOLD (1e-12).
        // discretize_auto emits tracing::warn! and falls back to van_loan_discretize.
        let a_c = DMatrix::from_row_slice(2, 2, &[-1.0e-13_f64, 0.0, 0.0, -1.0]);
        let b_c = DMatrix::from_row_slice(2, 1, &[1.0e-13, 1.0]);

        let mapping = OutputMapping {
            output_count: 2,
            node_to_output: vec![(0, 0, 1.0), (1, 1, 1.0)],
            input_to_output: vec![],
        };

        // Must not panic; van_loan fallback handles ill-conditioned A_c.
        let result = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping);
        assert!(
            result.is_ok(),
            "from_continuous should succeed for ill-conditioned A_c via van_loan fallback"
        );
    }

    #[test]
    fn solve_coupled_matches_step_coupled() {
        let a_c = DMatrix::from_row_slice(1, 1, &[-0.01]);
        let b_c = DMatrix::from_row_slice(1, 2, &[0.01, 0.005]);

        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };

        let dt = 60.0;
        let model =
            StateSpaceModel::from_continuous(&a_c, &b_c, dt, &mapping).expect("model should build");

        let x = DVector::from_row_slice(&[20.0]);
        let u = DVector::from_row_slice(&[30.0, 0.0]);
        let y_target = 22.0;
        let d_diag = 0.5;
        let forcing = 1.0;

        let couplings = [(0_usize, d_diag, forcing)];
        let mut m_scratch = DMatrix::zeros(1, 1);
        let lu = model.build_coupled_lu(&mut m_scratch, &couplings);

        // Solve for input_index=1 to hit y_target
        let solved_u = model
            .solve_for_scalar_input_coupled(
                &x,
                &u,
                y_target,
                0,
                1,
                &CouplingData {
                    lu: &lu,
                    couplings: &couplings,
                },
            )
            .expect("coupled solve should succeed");

        // Step with the solved input and verify output matches target
        let mut u_solved = u.clone();
        u_solved[1] = solved_u;
        let mut buf = DVector::zeros(1);
        model.step_with_coupling_into(&x, &u_solved, &mut buf, &mut m_scratch, &couplings);
        let y_actual = model.output(&buf, &u_solved)[0];

        assert!(
            (y_actual - y_target).abs() < 1.0e-9,
            "output {y_actual} should match target {y_target}",
        );
    }

    /// Gershgorin continuous stability check fails for a non-singular A_c whose
    /// eigenvalues are in fact all strictly negative. The tiered fallback must
    /// invoke `eigenvalue_check`, confirm stability, and allow construction
    /// rather than rejecting the model.
    ///
    /// The test uses an upper-triangular A_c where row 0 fails the Gershgorin
    /// disc test (`a_00 + |a_01| = -0.9 + 1.0 = 0.1 > 0`) but the eigenvalues
    /// (-0.9, -0.5) are all negative real.
    #[test]
    fn from_continuous_gershgorin_false_positive_accepted_by_eigenvalue_fallback() {
        // A_c: row 0 has strong positive off-diagonal coupling relative to its
        // damping, causing the Gershgorin disc to cross the imaginary axis.
        let a_c = DMatrix::from_row_slice(2, 2, &[-0.9, 1.0, 0.0, -0.5]);
        let b_c = DMatrix::from_row_slice(2, 1, &[1.0, 1.0]);

        let mapping = OutputMapping {
            output_count: 2,
            node_to_output: vec![(0, 0, 1.0), (1, 1, 1.0)],
            input_to_output: vec![],
        };

        // Verify preconditions outside the constructor.
        assert!(
            !gershgorin_continuous_stable(&a_c),
            "precondition: Gershgorin continuous must flag instability"
        );
        assert!(
            !is_singular(&a_c),
            "precondition: A_c must be non-singular so the singular exemption does not apply"
        );

        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 1.0, &mapping)
            .expect("from_continuous should accept the model via eigenvalue fallback");

        // The constructed model must work: one step should produce finite output.
        let x = DVector::from_row_slice(&[20.0, 15.0]);
        let u = DVector::from_row_slice(&[5.0]);
        let x_next = model.step(&x, &u);
        assert!(
            x_next[0].is_finite() && x_next[1].is_finite(),
            "step output must be finite"
        );
    }

    /// The eigenvalue fallback must not weaken the rejection path:
    /// when a non-singular A_c has a genuinely unstable eigenvalue
    /// (positive real part), the fallback must still reject with
    /// `Err(UnstableSystem)`.
    #[test]
    fn from_continuous_genuinely_unstable_rejected_by_eigenvalue_fallback() {
        // A_c with eigenvalue +0.5 (unstable) and -0.9 (stable).
        let a_c = DMatrix::from_row_slice(2, 2, &[-0.9, 1.0, 0.0, 0.5]);
        let b_c = DMatrix::from_row_slice(2, 1, &[1.0, 1.0]);

        let mapping = OutputMapping {
            output_count: 2,
            node_to_output: vec![(0, 0, 1.0), (1, 1, 1.0)],
            input_to_output: vec![],
        };

        // Verify preconditions: Gershgorin continuous fails (both rows).
        assert!(
            !gershgorin_continuous_stable(&a_c),
            "precondition: Gershgorin continuous must flag instability"
        );
        assert!(!is_singular(&a_c), "precondition: A_c must be non-singular");

        let result = StateSpaceModel::from_continuous(&a_c, &b_c, 1.0, &mapping);
        assert!(
            matches!(result, Err(StateSpaceError::UnstableSystem(_))),
            "genuinely unstable system must be rejected with UnstableSystem"
        );
    }

    // ── Identity-coupled solver tests ───────────────────────────────────────

    /// The identity-coupled step path (closed-form diagonal scaling) produces
    /// numerically identical results to the LU factorization path when M = I.
    /// Tests a range of state dimensions, coupling counts, and diagonal
    /// magnitudes to cover the full production parameter space.
    #[test]
    fn identity_coupled_step_matches_lu_step() {
        for n in [1, 2, 5, 10, 50] {
            for k in [0, 1, 2, 3] {
                let k = k.min(n);
                // Build a stable n×n A_c with random-ish but deterministic entries.
                let mut a_c = DMatrix::<f64>::zeros(n, n);
                for i in 0..n {
                    a_c[(i, i)] = -(0.01 + 0.001 * i as f64);
                }
                let mut c_star = DMatrix::<f64>::zeros(n, n);
                for i in 1..n {
                    c_star[(i, i - 1)] = 0.002;
                }
                c_star[(0, n - 1)] = 0.001;
                let a_c = a_c + c_star;

                let mut b_c = DMatrix::<f64>::zeros(n, n);
                for i in 0..n {
                    b_c[(i, i)] = a_c[(i, i)].abs();
                }

                let mapping = OutputMapping {
                    output_count: n,
                    node_to_output: (0..n).map(|i| (i, i, 1.0)).collect(),
                    input_to_output: vec![],
                };

                let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping)
                    .expect("model should build");
                assert!(model.m_is_identity());

                let x = DVector::from_fn(n, |i, _| 20.0 + i as f64);
                let u = DVector::from_fn(n, |i, _| 5.0 + 0.5 * i as f64);

                // Couplings on the first k rows with varying diagonal strengths.
                let couplings: Vec<(usize, f64, f64)> = (0..k)
                    .map(|i| (i, 0.1 + 0.3 * i as f64, 1.0 + 0.5 * i as f64))
                    .collect();

                // Identity path
                let mut buf_id = DVector::zeros(n);
                model.step_with_identity_coupling_into(&x, &u, &mut buf_id, &couplings);

                // LU path
                let mut buf_lu = DVector::zeros(n);
                let mut m_scratch = DMatrix::zeros(n, n);
                let lu = model.build_coupled_lu(&mut m_scratch, &couplings);
                model.step_with_coupled_lu_into(&x, &u, &mut buf_lu, &lu, &couplings);

                for i in 0..n {
                    let delta = (buf_id[i] - buf_lu[i]).abs();
                    assert!(
                        delta <= 1e-10,
                        "n={n} k={k} row {i}: identity={} lu={} delta={delta:e}",
                        buf_id[i],
                        buf_lu[i]
                    );
                }
            }
        }
    }

    /// The identity-coupled scalar solve path produces numerically identical
    /// results to the LU-coupled scalar solve when M = I.
    #[test]
    fn identity_scalar_solve_matches_lu_scalar_solve() {
        for n in [1, 2, 5, 10, 20] {
            let n = n.max(3);

            let mut a_c = DMatrix::<f64>::zeros(n, n);
            for i in 0..n {
                a_c[(i, i)] = -(0.02 + 0.001 * i as f64);
            }
            for i in 1..n {
                a_c[(i, i - 1)] = 0.001;
            }

            let m = n;
            let mut b_c = DMatrix::<f64>::zeros(n, m);
            for i in 0..n {
                b_c[(i, i)] = a_c[(i, i)].abs();
            }

            let mapping = OutputMapping {
                output_count: n,
                node_to_output: (0..n).map(|i| (i, i, 1.0)).collect(),
                input_to_output: vec![],
            };

            let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping)
                .expect("model should build");
            assert!(model.m_is_identity());

            let x = DVector::from_fn(n, |i, _| 20.0 + i as f64);
            let u = DVector::from_fn(m, |i, _| 5.0 + (i as f64) * 0.5);

            let k = n.min(3);
            let couplings: Vec<(usize, f64, f64)> = (0..k)
                .map(|i| (i, 0.1 + 0.2 * i as f64, -0.5 + 0.5 * i as f64))
                .collect();

            let mut m_scratch = DMatrix::zeros(n, n);
            let lu = model.build_coupled_lu(&mut m_scratch, &couplings);

            // Test only (input_idx, output_idx) pairs where the path through
            // the system has non-zero effective gain. With diagonal B_eff and
            // identity C, the gain is non-zero when input_idx == output_idx
            // and the row is coupled.
            for &(row, _, _) in &couplings {
                let input_idx = row;
                let output_idx = row;
                if input_idx >= m || output_idx >= model.c.nrows() {
                    continue;
                }
                let y_target = 21.0 + output_idx as f64;

                let solved_id = model
                    .solve_for_scalar_input_identity_coupled(
                        &x, &u, y_target, output_idx, input_idx, &couplings,
                    )
                    .expect("identity scalar solve should succeed");

                let solved_lu = model
                    .solve_for_scalar_input_coupled(
                        &x,
                        &u,
                        y_target,
                        output_idx,
                        input_idx,
                        &CouplingData {
                            lu: &lu,
                            couplings: &couplings,
                        },
                    )
                    .expect("LU scalar solve should succeed");

                let delta = (solved_id - solved_lu).abs();
                assert!(
                    delta <= 1e-10,
                    "n={n} row={row}: id={solved_id:e} lu={solved_lu:e} delta={delta:e}"
                );
            }
        }
    }

    /// `step_with_coupling_into` dispatches to the identity path when
    /// `m_is_identity` is true, producing the same result as calling
    /// `step_with_identity_coupling_into` directly.
    #[test]
    fn step_with_coupling_into_dispatches_to_identity_path() {
        let a_c = DMatrix::from_row_slice(
            3,
            3,
            &[-0.02, 0.001, 0.0, 0.001, -0.015, 0.0, 0.0, 0.0, -0.01],
        );
        let b_c = DMatrix::from_row_slice(3, 3, &[0.02, 0.0, 0.0, 0.0, 0.015, 0.0, 0.0, 0.0, 0.01]);

        let mapping = OutputMapping {
            output_count: 3,
            node_to_output: vec![(0, 0, 1.0), (1, 1, 1.0), (2, 2, 1.0)],
            input_to_output: vec![],
        };

        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping)
            .expect("model should build");
        assert!(model.m_is_identity());

        let x = DVector::from_row_slice(&[22.0, 21.0, 20.0]);
        let u = DVector::from_row_slice(&[10.0, 8.0, 6.0]);
        let couplings = vec![(0_usize, 0.5, 2.0), (2_usize, 0.3, 1.0)];

        // Direct identity path
        let mut buf_direct = DVector::zeros(3);
        model.step_with_identity_coupling_into(&x, &u, &mut buf_direct, &couplings);

        // Via dispatch
        let mut buf_dispatch = DVector::zeros(3);
        let mut m_scratch = DMatrix::zeros(3, 3);
        model.step_with_coupling_into(&x, &u, &mut buf_dispatch, &mut m_scratch, &couplings);

        for i in 0..3 {
            assert!(
                (buf_direct[i] - buf_dispatch[i]).abs() < 1e-12,
                "row {i}: direct={} dispatch={}",
                buf_direct[i],
                buf_dispatch[i]
            );
        }
    }

    /// Round-trip: identity step + identity scalar solve must agree.
    /// Solve for an input value that produces a target, then step with
    /// that input and verify the output hits the target.
    #[test]
    fn identity_coupled_solve_round_trips_through_step() {
        let a_c = DMatrix::from_row_slice(
            3,
            3,
            &[-0.03, 0.005, 0.0, 0.005, -0.02, 0.002, 0.0, 0.002, -0.01],
        );
        let b_c = DMatrix::from_row_slice(
            3,
            4,
            &[
                0.03, 0.0, 0.01, 0.0, 0.0, 0.02, 0.005, 0.0, 0.0, 0.0, 0.01, 0.005,
            ],
        );

        let mapping = OutputMapping {
            output_count: 3,
            node_to_output: vec![(0, 0, 1.0), (1, 1, 1.0), (2, 2, 1.0)],
            input_to_output: vec![(0, 2, 0.1), (2, 3, 0.05)],
        };

        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping)
            .expect("model should build");
        assert!(model.m_is_identity());

        let x = DVector::from_row_slice(&[21.0, 20.5, 19.0]);
        let u = DVector::from_row_slice(&[5.0, 3.0, 0.0, 0.0]);
        let couplings = vec![(0_usize, 0.5, 2.0), (1_usize, 0.3, 1.5)];

        // Solve for input 2 to hit y_target at output 0
        let y_target = 22.0;
        let solved_u2 = model
            .solve_for_scalar_input_identity_coupled(&x, &u, y_target, 0, 2, &couplings)
            .expect("solve should succeed");

        let mut u_solved = u.clone();
        u_solved[2] = solved_u2;

        let mut buf = DVector::zeros(3);
        model.step_with_identity_coupling_into(&x, &u_solved, &mut buf, &couplings);
        let y_actual = model.output(&buf, &u_solved)[0];

        assert!(
            (y_actual - y_target).abs() < 1e-9,
            "round-trip failed: y_actual={y_actual} y_target={y_target}"
        );
    }

    /// `m_is_identity` is true after construction for both paths.
    #[test]
    fn m_is_identity_flag_is_set_in_both_constructors() {
        // from_continuous
        let a_c = DMatrix::from_row_slice(2, 2, &[-0.02, 0.0, 0.0, -0.01]);
        let b_c = DMatrix::from_row_slice(2, 1, &[0.02, 0.01]);
        let mapping = OutputMapping {
            output_count: 2,
            node_to_output: vec![(0, 0, 1.0), (1, 1, 1.0)],
            input_to_output: vec![],
        };
        let model =
            StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).expect("should build");
        assert!(model.m_is_identity());

        // from_discrete
        let a_d = DMatrix::from_row_slice(2, 2, &[0.5, 0.0, 0.0, 0.3]);
        let b_d = DMatrix::from_row_slice(2, 1, &[0.1, 0.2]);
        let c = DMatrix::identity(2, 2);
        let d = DMatrix::zeros(2, 1);
        let model2 = StateSpaceModel::from_discrete(a_d, b_d, c, d).expect("should build");
        assert!(model2.m_is_identity());
    }
}
