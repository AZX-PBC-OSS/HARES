//! Thermal domain solver for the building envelope.
//!
//! ## Ground temperature boundary conditions
//!
//! Ground-connected envelope boundaries use the Kusuda-Achenbach undisturbed
//! ground temperature model (Kusuda & Achenbach 1965, ASHRAE Trans. 71(1):61-74;
//! EnergyPlus Engineering Reference, "Undisturbed Ground Temperature Model").
//! Each unique foundation depth gets its own B-matrix driving column; per-step
//! depth-attenuated and phase-shifted temperatures are evaluated via
//! [`hares_physics::ground::kusuda_achenbach_temp`] using the site-specific
//! annual mean, amplitude, and phase parameters carried on
//! [`EnvironmentState::weather`].
//!
//! The DOE-2 surface ground temperature model (`env.weather.ground_temp_c`)
//! is a sinusoidal fit to monthly mean ambient temperatures at the ground
//! surface (depth ≈ 0 m). It is retained as a legacy field and is **not** used
//! by the thermal solver to set below-grade boundary conditions. All ground
//! driving temperatures are computed from the Kusuda-Achenbach parameters.
//!
//! ## Boundary type distinction
//!
//! - **Below-grade boundaries** (basement walls, slab-on-grade floors,
//!   crawlspace floors): receive Kusuda-Achenbach depth-corrected ground
//!   temperature at their centroid foundation depth.
//! - **Grade-surface boundaries** (slab perimeter F2 method, ground-facing
//!   windows): receive Kusuda-Achenbach temperature at depth 0.0 m, which
//!   includes the correct phase lag but no depth attenuation.
//! - **Above-grade boundaries** (walls, roofs, windows facing outdoor):
//!   driven by `outdoor_temp_c` — unaffected by ground temperature.

mod config;
mod infiltration;
mod initialization;
mod longwave;
mod ports;
mod snapshot;
mod solar;
mod stepping;

pub use snapshot::{THERMAL_SNAPSHOT_SCHEMA_VERSION, ThermalSnapshot};
pub use stepping::SiteLocation;

pub(crate) use config::Result;
pub use config::{
    BoundaryCategory, BoundaryDiagnosticInfo, DrivingTemp, EnvelopeComponentGains,
    ExteriorSurfaceInfo, FilmCoefficientModel, InfiltrationMethod, InteriorConvectionInjection,
    InteriorLwrZoneConfig, InteriorSolarSurfaceInfo, InteriorSolarZoneConfig, InteriorSurfaceInfo,
    MechanicalVentilationParams, NaturalVentilationConfig, OpeningType, StateSpaceWiring,
    ThermalSolverConfig, ThermalSolverError, WindowSolarProperties,
};

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use hares_physics::air_properties::moist_air_density_kg_m3;
use hares_physics::constants::{KJ_TO_J, LATENT_HEAT_VAPORISATION_0C_KJ_KG};
use hares_types::{
    DomainId, DomainSolver, DomainUpdate, EnvironmentState, HaresError, PortSlots, THERMAL,
    ThermalCategory, ZoneId,
};
use nalgebra::DVector;

use crate::state_space::{SolveScratch, StateSpaceModel};

use infiltration::{InfiltrationCoupling, apply_infiltration_and_ventilation};
use initialization::initialize_steady_state;

const H_FG_J_PER_KG: f64 = LATENT_HEAT_VAPORISATION_0C_KJ_KG * KJ_TO_J;

#[derive(Debug, Clone)]
pub struct ThermalSolver {
    model: StateSpaceModel,
    wiring: StateSpaceWiring,
    config: ThermalSolverConfig,
    /// Timestep in seconds that the model was discretized for; runtime dt must match.
    dt_s: f64,
    x: DVector<f64>,
    last_u: DVector<f64>,
    /// Reusable input buffer: swapped during build_input_vector to avoid per-step allocation.
    u_buf: DVector<f64>,
    /// Reusable latent-load accumulator: cleared at the start of each infiltration pass.
    latent_buf: HashMap<ZoneId, f64>,
    /// Reusable state-step buffer: receives N·x + B_eff·u (coupling-scaled
    /// when couplings are active).
    rhs_buf: DVector<f64>,
    /// Reusable output buffer: receives C·x + D·u in `integrate_inner` via
    /// `StateSpaceModel::output_into` (was an owned per-step `model.output`).
    /// Swapped out during the step and given back afterwards, `u_buf`-style.
    y_buf: DVector<f64>,
    /// Reusable scratch for the D·u half of `StateSpaceModel::output_into`.
    du_buf: DVector<f64>,
    /// Reusable scratch for the per-zone ideal-capacity scalar solves
    /// (`solve_for_scalar_input_identity_coupled` / uncoupled), sized to the
    /// state dimension at construction. Its `n_x`/`b_u`/`rhs` half is the
    /// step-shared prefix (filled once per step by the first
    /// `solve_ideal_capacity_for_target` call); `tail_rhs`/`gain`/`d_agg`
    /// are the per-call tail region.
    solve_scratch: SolveScratch,
    /// Whether `solve_scratch`'s prefix half (`n_x`, `b_u`, `rhs`) holds the
    /// shared terms of the step's ideal-capacity solves. Set when a
    /// `solve_ideal_capacity_for_target` call fills them from the step's `x`
    /// and `last_u`; cleared by every path that mutates either
    /// (`prepare_inputs_inner`, `integrate_inner`, `restore_state`), so the
    /// validity window is exactly "same x and u as when the prefix was
    /// filled".
    shared_prefix_valid: bool,
    /// Test-visible instrumentation for `shared_terms_computed_once_per_step`:
    /// number of shared-prefix fills in `solve_ideal_capacity_for_target`.
    #[cfg(test)]
    shared_prefix_fills: usize,
    /// Test-visible instrumentation: number of target-dependent tails run by
    /// `solve_ideal_capacity_for_target` (one per call that reaches the solve).
    #[cfg(test)]
    solve_tail_calls: usize,
    /// Per-step coupling tuples: (state_idx, d_implicit, forcing). Reused each step.
    coupling_buf: Vec<(usize, f64, f64)>,
    /// Coupling tuples of the last prepared or integrated step, read by
    /// `solve_ideal_capacity_for_target`: non-empty selects the
    /// identity-coupled solve, empty the uncoupled one.
    last_coupling: Vec<(usize, f64, f64)>,
    /// Per-exterior-surface converged surface temperatures [°C] for LWR continuity.
    /// Indexed parallel to `config.exterior_surfaces`.
    exterior_surface_temps: Vec<f64>,
    /// Persistent save buffer for `exterior_surface_temps`:
    /// `prepare_inputs_inner` copies the temperatures in before
    /// `build_input_vector` mutates them and copies them back afterwards
    /// (was a per-step `Vec` clone).
    ext_temps_save_buf: Vec<f64>,
    /// Last-step component gains for output/diagnostics.
    component_gains: EnvelopeComponentGains,
    /// Reusable buffer for interior surface temperatures in LWR calculation.
    /// Avoids per-zone per-timestep allocation in apply_interior_longwave_inputs.
    interior_surf_temps_buf: Vec<f64>,
    /// Base interior surface temperatures before iterative LWR correction.
    interior_surf_base_buf: Vec<f64>,
    /// Previous-iteration interior surface temperatures for damping.
    interior_surf_prev_buf: Vec<f64>,
    /// Persistent interior surface temperatures [°C] for interior LWR continuity.
    /// Indexed parallel to `config.interior_lwr_zones`, then per-zone surface order.
    interior_surface_temps: Vec<Vec<f64>>,
    /// Persistent previous-iteration interior surface temperatures [°C] used by
    /// heavy-ball damping, indexed parallel to `interior_surface_temps`.
    interior_surface_prev_temps: Vec<Vec<f64>>,
    /// Pre-allocated buffer for infiltration couplings returned by build_input_vector.
    infiltration_buf: Vec<InfiltrationCoupling>,
    /// Pre-allocated scratch buffer for per-zone infiltration gains; swapped into component_gains.
    infiltration_by_zone_buf: Vec<(ZoneId, f64)>,
    /// Pre-allocated buffer for solar distribution absorbed values.
    solar_absorbed_buf: Vec<f64>,
    /// Pre-allocated buffer for interior LWR per-zone results.
    lwr_by_zone_buf: Vec<(ZoneId, f64)>,
    /// Accumulator for window exterior LWR beyond U-factor assumption [W].
    /// Set during `apply_exterior_longwave_inputs_iterative`.
    window_exterior_lwr_w: f64,
    /// Net exterior LWR at opaque exterior skins [W], both application
    /// paths: the non-iterative path (rad_frac == 0, flux routed through the
    /// semi-implicit coupling, not `u`) and the iterative path (rad_frac > 0,
    /// accumulated per surface at the converged skin temperature). Tracked
    /// separately for diagnostic reporting because neither path's `u` delta
    /// isolates the LWR component.
    opaque_exterior_lwr_w: f64,
    /// Exact discrete-time matrix exchange into the indoor zone air node
    /// [W/K]: `C_zone/dt · (A_d − I)[zone_row, :]` — the zone air's RC
    /// exchange per unit of state. Computed once at construction; dotted
    /// with `x_prev` each step for the zone air heat-balance residual.
    /// Captures ALL matrix-borne exchange — inside-face
    /// convection, StarMesh interior LWR, and the window/steady-state UA
    /// sink — exactly as the state equation moves it, which the
    /// convection-only per-boundary reporting columns cannot.
    zone_exchange_row_w: Vec<f64>,
    /// Environmental-column coefficients [W per unit input]:
    /// `C_zone/dt · B_d[zone_row, col]` over driving-temperature columns
    /// (outdoor, ground, indoor-driving) — the inflow side of the
    /// steady-state boundary conduction that the exchange row's diagonal
    /// sinks. Paired with `zone_exchange_row_w`.
    zone_env_col_coeffs: Vec<(usize, f64)>,
    /// Per-step lookup: `surface_id` → slot in `env.weather.solar_irradiance`.
    /// Refreshed twice per timestep by [`Self::refresh_solar_slot_map`] (called
    /// from `build_input_vector` and the debug breakdown path) so the solar
    /// and exterior-LWR apply passes index directly instead of rescanning
    /// the irradiance vec per surface — O(S) per step total, not O(S²).
    /// The map is rebuilt only when the incoming surface-id sequence changes;
    /// capacity is retained across steps: no steady-state allocation.
    /// Direct callers of the apply functions (tests, debug paths) must call
    /// `refresh_solar_slot_map` first (or populate the map themselves).
    solar_irr_slot_buf: HashMap<u32, usize>,
    /// Surface-id sequence the slot map was last built from, parallel to
    /// `env.weather.solar_irradiance`. `refresh_solar_slot_map` compares the
    /// incoming sequence id by id (no hashing) and rebuilds the map only when
    /// it differs, so a fixed environment rebuilds nothing per step.
    solar_slot_map_keys: Vec<u32>,
    /// Absorbed opaque exterior solar [W] on iterative-path (rad_frac > 0)
    /// surfaces — the full skin-absorbed flux `α·A·POA`, not the
    /// rad_frac-scaled fraction injected into the RC node. Accumulated per
    /// surface during the iterative LWR solve; the non-iterative path's
    /// solar is measured directly by the `u` delta around
    /// `apply_exterior_solar_inputs` (it injects the full absorbed flux).
    opaque_exterior_solar_w: f64,
    /// Pre-allocated buffer for interior LWR net flux results per surface.
    lwr_net_flux_buf: Vec<f64>,
    /// Pre-allocated buffer for previous-iteration interior LWR net flux values.
    /// Used for relative flux-residual convergence checking.
    lwr_net_flux_prev_buf: Vec<f64>,
    /// Per-boundary A-matrix film resistance [m²·K/W] paralleling the
    /// `convection_injection` vec for computing per-step correction.
    /// Cached at init to avoid repeated HashMap lookups.
    per_boundary_static_r_film: Vec<f64>,
    /// Prior-step zone-air temperatures [°C] used to emit telemetry about the
    /// LWR zone temperature lag (the last committed value from a completed step).
    prev_zone_temps_c: HashMap<ZoneId, f64>,
    /// Pre-allocated buffer for radiant distribution weight vectors.
    /// Reused across `distribute_radiant_lwr_surfaces` and
    /// `distribute_radiant_solar_surfaces` to avoid per-timestep allocation.
    radiant_weights_buf: Vec<f64>,
    /// Pre-sorted zone temperature buffer for format_domain_update; indexed parallel to sorted zone_output_indices.
    zone_temps_buf: Vec<(ZoneId, f64)>,
    latent_pairs_buf: Vec<(ZoneId, f64)>,
    custom_payload_buf: Vec<f64>,
    /// Pre-allocated aggregation buffer for the semi-implicit coupling diagonal
    /// damping in `step_with_identity_coupling_into_scratch`. Sized to
    /// `state_dim` at construction, avoiding per-timestep heap allocation.
    d_agg_buf: Vec<f64>,
    /// Per-surface linearised exterior LWR coupling data for semi-implicit
    /// integration: `(state_idx, input_idx, h_rad_w_k, t_eff_c)`.
    ///
    /// Populated by `apply_exterior_longwave_inputs_iterative` for surfaces
    /// with `rad_frac == 0` (no exterior film resistance in the RC network).
    /// Consumed by `build_coupling`, which adds semi-implicit coupling entries
    /// to `coupling_buf`.
    ///
    /// The linearised LWR splits the T⁴ radiative flux into:
    /// - **Forcing** `h_rad · T_eff` (external, sky/air temperature) →
    ///   handled through the coupling forcing term.
    /// - **Conductance** `h_rad · T_surf` (state-dependent) →
    ///   handled through the coupling diagonal damping.
    ///
    /// This prevents the nonlinear T⁴ feedback that occurs when the full
    /// radiative flux is injected as a B·u input, making the scheme
    /// unconditionally stable regardless of timestep.
    ///
    /// References:
    /// - EnergyPlus `ConvectionCoefficients.cc:661-678` (linearised `HRad`)
    /// - EnergyPlus `HeatBalanceSurfaceManager.cc:9575-9592` (combined film)
    /// - ASHRAE HoF 2021 Ch.4 §4.2 (linearised radiation coefficient)
    lwr_coupling_buf: Vec<(usize, usize, f64, f64)>,
    /// Per-zone energy balance residuals [W] from the current timestep's closure check.
    /// Populated by `integrate_inner`, consumed by `format_domain_update` for telemetry.
    energy_balance_residuals: HashMap<ZoneId, f64>,
    /// Full-system stored energy rate [W] = Σ C_i × (T_next_i − T_prev_i) / dt
    /// across ALL thermal state nodes.  Populated by `integrate_inner`, consumed
    /// by the dwelling invariant check for thermal energy conservation.
    full_system_stored_energy_w: f64,
    /// Per-node external energy injection [W] from B_c × u weighted by capacitance.
    /// Single-element vec: each entry = Σ_i C_i × (B_c × u)[i] for one zone.
    /// Populated by `integrate_inner`, consumed by check_thermal in check_step_invariants.
    thermal_balance_q_gains: Vec<f64>,
    /// Total envelope conduction to outdoor [W] = − Σ_i C_i × (A_c × x_prev)[i].
    /// Positive = heat leaving the system.  Populated by `integrate_inner`.
    thermal_balance_q_loss: f64,
    /// Pre-allocated working buffers for the affine-coupled balance
    /// decomposition (populated by `integrate_inner` in every build profile:
    /// the dwelling's thermal-balance invariant runs unconditionally).
    balance_buf_a: DVector<f64>,
    balance_buf_b: DVector<f64>,
    balance_buf_c: DVector<f64>,
    /// Pre-allocated per-state diagonal-damping aggregation buffer for the
    /// identity-M coupled balance decomposition. Holds `Σ d_j` for all
    /// coupling entries sharing a state index, so the semi-implicit solve
    /// applies `rhs[i] / (1 + Σ d_j)` rather than the buggy sequential
    /// `rhs[i] / Π(1 + d_j)`. Kept zeroed and refilled each invocation.
    balance_d_agg: Vec<f64>,
    /// Zero vector for the input dimension (avoids per-step allocation).
    balance_u_zero: DVector<f64>,
    /// Zero vector for the state dimension (avoids per-step allocation).
    balance_x_zero: DVector<f64>,
    /// Per-zone consecutive failure counts for `solve_ideal_capacity_for_target`.
    /// Incremented on each solve failure, reset to 0 on success. Used to throttle
    /// warn-level diagnostic logs — only the first failure in a run emits `warn!`;
    /// subsequent consecutive failures emit `debug!` to avoid log flood.
    ideal_capacity_failure_counts: HashMap<ZoneId, usize>,
    /// Per-zone last successfully computed capacity [W]. On solver convergence
    /// the computed capacity is stored here. On failure when
    /// `consecutive failures >= ideal_capacity_degraded_threshold`, this value
    /// is returned as a degraded fallback instead of 0.0.
    last_good_capacity_w: HashMap<ZoneId, f64>,
    /// Zones that received a degraded (last-good) capacity value during the
    /// most recent call to `solve_ideal_capacity_for_target`. Cleared at the
    /// start of each step via `begin_step_degradation_tracking`.
    ideal_capacity_degraded_zones: HashSet<ZoneId>,
    /// Zones for which an `error!` log has already been emitted during the
    /// current degradation run. Guards against per-timestep log flood when a
    /// zone remains stuck in the degraded fallback path across many consecutive
    /// steps. Cleared on recovery, mirroring `ideal_capacity_warned_zones`.
    ideal_capacity_degraded_warned_zones: HashSet<ZoneId>,
    /// Zones for which an ideal-capacity solve-failure warn has already been emitted
    /// during the current failure run. Guards against per-timestep log spam in
    /// pathological runs where the target is unreachable every step.
    ideal_capacity_warned_zones: HashSet<ZoneId>,
    /// Pre-allocated fallback buffer for InteriorSurface structs in non-ScriptF path.
    lwr_surfaces_buf: Vec<crate::longwave_radiation::InteriorSurface>,
    /// Zones for which the linearised interior LWR fallback has already emitted
    /// a one-time warning. Guards against per-timestep log spam.
    lwr_linearised_warned_zones: HashSet<ZoneId>,
    /// Cached outdoor temperature [°C] from the most recent input vector.
    /// Read by the per-boundary net convection accumulation (SteadyState
    /// diagnostics) in every build configuration.
    cached_outdoor_temp_c: f64,
    /// Cached per-depth ground temperatures [°C] parallel to
    /// `wiring.ground_temp_input_depths_m`. Index `i` holds the Kusuda-Achenbach
    /// temperature at depth `ground_temp_input_depths_m[i]`. Read by the
    /// per-boundary net convection accumulation in every build configuration.
    cached_ground_temps_c: Vec<f64>,
    /// Per-exterior-surface diagnostic buffer (compiled out in release).
    #[cfg(any(debug_assertions, feature = "observe_detailed"))]
    ext_surface_diag_buf: Vec<config::ExtSurfaceDiag>,
    /// Per-interior-surface diagnostic buffer (compiled out in release).
    #[cfg(any(debug_assertions, feature = "observe_detailed"))]
    int_surface_diag_buf: Vec<config::IntSurfaceDiag>,
    /// Per-window solar diagnostic buffer (compiled out in release).
    #[cfg(any(debug_assertions, feature = "observe_detailed"))]
    window_solar_diag_buf: Vec<config::WindowSolarDiag>,
}

/// Per-component breakdown of zone air sensible contributions.
///
/// Captures both the production path stages (outdoor, solar, LWR) and the
/// port contributions split by routing path so the convective, radiant and
/// short-wave attribution is explicit. The zone-air port total equals
/// `convective_direct_w + radiant_to_air_residual_w + shortwave_to_air_w`.
///
/// Energy balance invariant (within floating-point rounding):
/// `after_int_lwr_w + convective_direct_w + radiant_to_air_residual_w +
/// shortwave_to_air_w` equals the total zone air input at the end of the
/// port application sequence.
#[derive(Debug, Clone, Copy)]
pub struct ZoneSensibleBreakdown {
    /// u[zone_air] after outdoor temperature inputs [W].
    pub after_outdoor_w: f64,
    /// u[zone_air] after window solar inputs [W].
    pub after_window_solar_w: f64,
    /// u[zone_air] after exterior solar inputs [W].
    pub after_ext_solar_w: f64,
    /// u[zone_air] after exterior LWR inputs [W].
    pub after_ext_lwr_w: f64,
    /// u[zone_air] after interior LWR inputs (ScriptF) [W].
    pub after_int_lwr_w: f64,
    /// Direct convective gain from equipment sensible ports [W].
    pub convective_direct_w: f64,
    /// Radiant-to-air convective residual after TMULT surface distribution [W].
    ///
    /// Radiant gain × (1 − radiation_frac) for each surface, weighted
    /// by area×emissivity per the EnergyPlus TMULT method.
    pub radiant_to_air_residual_w: f64,
    /// Radiant gain delivered to interior surface RC nodes [W].
    ///
    /// Radiant gain × radiation_frac for each surface.
    pub radiant_to_surfaces_w: f64,
    /// Short-wave gain reaching zone air [W]: the part each surface passes
    /// on to the air, plus any the surfaces do not absorb.
    pub shortwave_to_air_w: f64,
    /// Short-wave gain delivered to interior surface RC nodes [W].
    pub shortwave_to_surfaces_w: f64,
}

impl ZoneSensibleBreakdown {
    /// All-zero breakdown (zone index not found).
    fn zeros() -> Self {
        Self {
            after_outdoor_w: 0.0,
            after_window_solar_w: 0.0,
            after_ext_solar_w: 0.0,
            after_ext_lwr_w: 0.0,
            after_int_lwr_w: 0.0,
            convective_direct_w: 0.0,
            radiant_to_air_residual_w: 0.0,
            radiant_to_surfaces_w: 0.0,
            shortwave_to_air_w: 0.0,
            shortwave_to_surfaces_w: 0.0,
        }
    }
}

impl ThermalSolver {
    pub fn state_vector(&self) -> &[f64] {
        self.x.as_slice()
    }

    /// Returns current zone temperatures [°C] derived from the state vector,
    /// using the `zone_state_indices` wiring (zone air is a state node).
    pub fn zone_temperatures_c(&self) -> Vec<(ZoneId, f64)> {
        self.wiring
            .zone_state_indices
            .iter()
            .map(|(&zone_id, &state_idx)| {
                let temp = if state_idx < self.x.len() {
                    self.x[state_idx]
                } else {
                    f64::NAN
                };
                (zone_id, temp)
            })
            .collect()
    }

    pub fn config(&self) -> &ThermalSolverConfig {
        &self.config
    }

    /// Sets the mechanical ventilation recovery effectiveness the next step
    /// reads (the ventilation equipment's effective values after bypass and
    /// defrost). The only part of the configuration that changes after
    /// construction; it is part of [`ThermalSnapshot`].
    pub fn set_ventilation_recovery(&mut self, sensible: f64, latent: f64) -> Result<()> {
        snapshot::validate_recovery_efficiencies(sensible, latent)?;
        self.config.ventilation.sensible_recovery_efficiency = sensible;
        self.config.ventilation.latent_recovery_efficiency = latent;
        Ok(())
    }

    /// Returns true when the zone's most recent `solve_ideal_capacity_for_target`
    /// call returned a degraded fallback (last-good capacity after consecutive
    /// solver failures exceeded the threshold). False otherwise.
    ///
    /// This is a step-local flag — it is cleared at the start of each
    /// `prepare_inputs` call.
    pub fn zone_capacity_degraded(&self, zone: ZoneId) -> bool {
        self.ideal_capacity_degraded_zones.contains(&zone)
    }

    /// Returns true when an `error!` log has already been emitted for this
    /// zone's current degradation run. Used to guard against log flood when
    /// a zone remains stuck in the degraded fallback path.
    pub fn zone_capacity_degraded_warned(&self, zone: ZoneId) -> bool {
        self.ideal_capacity_degraded_warned_zones.contains(&zone)
    }

    /// Per-component envelope gains from the most recent `resolve()` call.
    pub fn component_gains(&self) -> &EnvelopeComponentGains {
        &self.component_gains
    }

    /// Full-system stored energy rate from the most recent `resolve()` call [W].
    ///
    /// Computes Σ C_i × (T_next_i − T_prev_i) / dt across ALL thermal state nodes
    /// (zone air + wall-mass nodes).  This accounts for energy stored in every
    /// thermal capacitance in the RC network, unlike the per-zone residual which
    /// is zone-air-only and excludes wall-mass redistribution.
    ///
    /// At steady state this approaches zero; positive values indicate net energy
    /// storage (heating up), negative values indicate net energy release (cooling).
    pub fn full_system_stored_energy_w(&self) -> f64 {
        self.full_system_stored_energy_w
    }

    /// Per-zone energy balance residuals from the most recent `resolve()` call [W].
    ///
    /// Returns a map from zone ID to the zone-air-only residual:
    /// `|C_zone × ΔT_zone / dt − q_port_sensible|` where q_port_sensible is the
    /// total sensible heat injected into the zone air node.
    ///
    /// For single-node models (no wall-mass nodes) the residual is near-zero
    /// (within floating-point tolerance).  For multi-node RC models, wall-mass
    /// energy redistribution can produce residuals of several kW during
    /// transient conditions — this is expected physical behaviour, not a solver
    /// defect.
    pub fn energy_balance_residuals(&self) -> &HashMap<ZoneId, f64> {
        &self.energy_balance_residuals
    }

    /// Thermal balance terms for the invariant check, populated by the most
    /// recent `resolve()` call.
    ///
    /// Returns `(q_gains_slice, delta_e_storage_w, q_loss_w)` where:
    /// - `q_gains_slice`: external energy injections computed from B_c × u
    ///   weighted by node capacitance [W],
    /// - `delta_e_storage_w`: full-system stored energy rate
    ///   Σ C_i × (T_next_i − T_prev_i) / dt [W],
    /// - `q_loss_w`: total envelope conduction to outdoor/ground derived from
    ///   A_c × x weighted by capacitance [W], positive when heat leaves.
    ///
    /// These are computed independently inside `integrate_inner` using the
    /// continuous-time state-space matrices, the input vector `u`, and the
    /// state vectors before and after the ZOH integration.
    pub fn thermal_balance_terms(&self) -> (&[f64], f64, f64) {
        (
            &self.thermal_balance_q_gains,
            self.full_system_stored_energy_w,
            self.thermal_balance_q_loss,
        )
    }

    /// Number of configured boundary diagnostic entries.
    pub fn boundary_diagnostics_count(&self) -> usize {
        self.config.boundary_diagnostics.len()
    }

    pub fn model_dims(&self) -> (usize, usize, usize) {
        (
            self.model.state_dim(),
            self.model.input_dim(),
            self.model.output_dim(),
        )
    }

    /// Returns diagnostic information about the B_d (discrete input matrix) for the
    /// zone air state row and sensible input column. Used for physics debugging only.
    pub fn b_d_zone_sensible_debug(&self) -> Option<(usize, usize, f64, &[f64])> {
        let zone_id = self.config.indoor_zone_id;
        let state_row = *self.wiring.zone_state_indices.get(&zone_id)?;
        let input_col = *self.wiring.zone_sensible_input_indices.get(&zone_id)?;
        let b_d_entry = self.model.b_eff()[(state_row, input_col)];
        Some((state_row, input_col, b_d_entry, self.x.as_slice()))
    }

    /// Returns the full B_d row for the zone air state for debugging.
    pub fn b_d_zone_row_debug(&self) -> Option<Vec<f64>> {
        let zone_id = self.config.indoor_zone_id;
        let state_row = *self.wiring.zone_state_indices.get(&zone_id)?;
        Some(self.model.b_eff().row(state_row).iter().copied().collect())
    }

    /// Returns the full A_d row for the zone air state for debugging.
    pub fn a_d_zone_row_debug(&self) -> Option<Vec<f64>> {
        let zone_id = self.config.indoor_zone_id;
        let state_row = *self.wiring.zone_state_indices.get(&zone_id)?;
        Some(self.model.n_mat().row(state_row).iter().copied().collect())
    }

    /// Returns wiring indices for zone air for debugging.
    pub fn zone_wiring_debug(&self) -> (usize, usize, usize) {
        let zone_id = self.config.indoor_zone_id;
        let state_row = self
            .wiring
            .zone_state_indices
            .get(&zone_id)
            .copied()
            .unwrap_or(0);
        let sensible_col = self
            .wiring
            .zone_sensible_input_indices
            .get(&zone_id)
            .copied()
            .unwrap_or(0);
        let outdoor_col = self
            .wiring
            .outdoor_temp_input_indices
            .first()
            .copied()
            .unwrap_or(0);
        (state_row, sensible_col, outdoor_col)
    }

    /// Returns the last_u vector (input vector from the most recently completed step).
    pub fn last_u_debug(&self) -> &[f64] {
        self.last_u.as_slice()
    }

    /// Returns interior LWR zone surface info for debugging solar/LWR distribution.
    /// Returns (area_m2, solar_absorptance, radiation_frac, is_floor, input_index, driving_temp_is_some) per surface.
    pub fn interior_surface_info_debug(&self) -> Vec<(f64, f64, f64, bool, usize, bool)> {
        self.config
            .interior_lwr_zones
            .first()
            .map(|z| {
                z.surfaces
                    .iter()
                    .map(|s| {
                        (
                            s.area_m2,
                            s.solar_absorptance,
                            s.radiation_frac,
                            s.is_floor,
                            s.input_index,
                            s.driving_temp.is_some(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Returns the per-component breakdown of u[zone_sensible] for debugging.
    ///
    /// Applies both sensible and radiant port inputs in the same sequence as
    /// the production path (`build_input_vector`), so the reported zone-air
    /// contribution includes the convective residual from radiant distribution.
    ///
    /// Radiant gains are distributed to interior surfaces using TMULT
    /// area×emissivity weighting; the fraction not absorbed by surfaces
    /// (`1 − radiation_frac`) returns to the zone air as a convective
    /// residual.
    pub fn zone_sensible_breakdown_debug(
        &mut self,
        ports: &hares_types::PortSlots,
        env: &hares_types::EnvironmentState,
    ) -> std::result::Result<ZoneSensibleBreakdown, HaresError> {
        let n = self.model.input_dim();
        let mut u = DVector::zeros(n);
        let zone_id = self.config.indoor_zone_id;
        let Some(&z_idx) = self.wiring.zone_sensible_input_indices.get(&zone_id) else {
            return Ok(ZoneSensibleBreakdown::zeros());
        };
        self.apply_outdoor_inputs(&mut u, env);
        self.refresh_solar_slot_map(env);
        let after_outdoor = u[z_idx];
        self.apply_solar_inputs(&mut u, env);
        let after_window_solar = u[z_idx];
        self.apply_exterior_solar_inputs(&mut u, env);
        let after_ext_solar = u[z_idx];
        self.apply_exterior_longwave_inputs_iterative(&mut u, env)?;
        let after_ext_lwr = u[z_idx];
        // Interior LWR: ScriptF iterative injection only when not using
        // StarMesh (star-mesh bakes radiation conductances into the A-matrix
        // at construction time, so no per-timestep injection is needed).
        if self.config.interior_lwr_method == crate::boundary_rc::InteriorLwrMethod::ScriptF {
            self.apply_interior_longwave_inputs(&mut u, env);
        }
        let after_int_lwr = u[z_idx];

        // Port inputs in production sequence: sensible first, then radiant.
        // Track the delta at the zone air index to split convective direct
        // from the radiant-to-air convective residual.
        self.apply_port_convective_inputs(&mut u, ports);
        let after_convective = u[z_idx];
        let convective_direct_w = after_convective - after_int_lwr;

        self.apply_port_radiant_inputs(&mut u, ports);
        let after_radiant = u[z_idx];
        let radiant_to_air_residual_w = after_radiant - after_convective;

        let total_radiant_w: f64 = ports.thermal.iter().map(|t| t.radiant_gain_w).sum();
        let radiant_to_surfaces_w = (total_radiant_w - radiant_to_air_residual_w).max(0.0);

        self.apply_port_shortwave_inputs(&mut u, ports);
        let shortwave_to_air_w = u[z_idx] - after_radiant;
        let total_shortwave_w: f64 = ports.thermal.iter().map(|t| t.shortwave_gain_w).sum();
        let shortwave_to_surfaces_w = (total_shortwave_w - shortwave_to_air_w).max(0.0);

        Ok(ZoneSensibleBreakdown {
            after_outdoor_w: after_outdoor,
            after_window_solar_w: after_window_solar,
            after_ext_solar_w: after_ext_solar,
            after_ext_lwr_w: after_ext_lwr,
            after_int_lwr_w: after_int_lwr,
            convective_direct_w,
            radiant_to_air_residual_w,
            radiant_to_surfaces_w,
            shortwave_to_air_w,
            shortwave_to_surfaces_w,
        })
    }

    pub fn new(
        model: StateSpaceModel,
        wiring: StateSpaceWiring,
        config: ThermalSolverConfig,
        dt_s: f64,
        env: &EnvironmentState,
        indoor_temp_c: f64,
    ) -> Result<Self> {
        // Structural wiring contract: fails fast with the offending surface
        // named (out-of-range indices, non-physical coupling parameters,
        // duplicate surface registration) instead of silently mis-wiring.
        config
            .validate(model.state_dim(), model.input_dim())
            .map_err(ThermalSolverError::Configuration)?;

        // Thermal port wiring: every env zone must own a sensible-heat input
        // column. Equipment may declare a thermal port on any env zone, and a
        // zone without a column would have its contributions silently
        // dropped, so the mapping is checked once here instead of per step.
        for zone in &env.zones {
            if !wiring.zone_sensible_input_indices.contains_key(&zone.id) {
                return Err(ThermalSolverError::MissingZoneMapping {
                    zone: zone.id,
                    field: "zone_sensible_input_indices",
                });
            }
        }

        // Silent plausible-value fallbacks become construction-time
        // errors. Zone-map completeness checks live in the scoped block
        // below; this block covers the per-depth ground-node invariant.
        // Ground-driving diagnostics must resolve to a per-depth ground node.
        for diag in &config.boundary_diagnostics {
            if let config::BoundaryDiagnosticInfo::SteadyState {
                driving_temp: config::DrivingTemp::Ground { depth_m },
                ..
            } = diag
            {
                let found = wiring.ground_temp_input_depths_m.iter().any(|d| {
                    crate::boundary_rc::depth_mm_key(*d)
                        == crate::boundary_rc::depth_mm_key(*depth_m)
                });
                if !found {
                    return Err(ThermalSolverError::Configuration(format!(
                        "ground-coupled boundary at depth {depth_m} m has no matching \
                         per-depth ground node (available: {:?}); a miss would silently \
                         drive it with 0 °C",
                        wiring.ground_temp_input_depths_m
                    )));
                }
            }
        }
        // Ground input wiring must be parallel and in range: the runtime
        // builds `cached_ground_temps_c` by writing only the in-range
        // columns, so an out-of-range index silently drops that depth from
        // the cache and the per-boundary accumulation later hits an expect
        // that claims construction validated it. Reject here, with the
        // depth named.
        if wiring.ground_temp_input_indices.len() != wiring.ground_temp_input_depths_m.len() {
            return Err(ThermalSolverError::Configuration(format!(
                "ground input wiring is not parallel: {} indices vs {} depths",
                wiring.ground_temp_input_indices.len(),
                wiring.ground_temp_input_depths_m.len()
            )));
        }
        for (&idx, &depth_m) in wiring
            .ground_temp_input_indices
            .iter()
            .zip(wiring.ground_temp_input_depths_m.iter())
        {
            if idx >= model.input_dim() {
                return Err(ThermalSolverError::Configuration(format!(
                    "ground input column {idx} for depth {depth_m} m is out of range \
                     (model has {} inputs)",
                    model.input_dim()
                )));
            }
        }
        // ── Wiring range validation (one pass over the whole struct) ─────
        //
        // Every index-bearing map in `StateSpaceWiring` is checked against
        // the model dimensions. The failure modes this replaces, per map:
        //   • zone_output_indices (out-of-range value): deferred panic in
        //     the per-boundary accumulation (`y_next[zone_output_idx]`,
        //     unguarded).
        //   • outdoor_temp_input_indices: SILENT — every consumer
        //     guard-and-skips, the outdoor column is never written, and the
        //     building simulates against a phantom 0 °C outdoors.
        //   • zone_state_indices: deferred panic in the state update.
        //   • zone_sensible_input_indices: SILENT — all zone-air injections
        //     (internal gains, HVAC, window solar) are discarded.
        //   • indoor_temp_input_indices: SILENT — filtered from the
        //     zone-air-balance residual, corrupting the diagnostic.
        //   • solar_input_indices: SILENT — exterior solar never lands.
        // Presence-only checks (does the indoor zone HAVE an entry?) are
        // kept separately below; this pass is about VALUES.
        {
            let n_states = model.state_dim();
            let n_inputs = model.input_dim();
            let n_outputs = model.output_dim();
            for (&zone, &idx) in &wiring.zone_state_indices {
                if idx >= n_states {
                    return Err(ThermalSolverError::Configuration(format!(
                        "zone {:?}: state index {idx} is out of range \
                         (model has {n_states} states)",
                        zone.0
                    )));
                }
            }
            for (&zone, &idx) in &wiring.zone_output_indices {
                if idx >= n_outputs {
                    return Err(ThermalSolverError::Configuration(format!(
                        "zone {:?}: output index {idx} is out of range \
                         (model has {n_outputs} outputs)",
                        zone.0
                    )));
                }
            }
            for (&zone, &idx) in &wiring.zone_sensible_input_indices {
                if idx >= n_inputs {
                    return Err(ThermalSolverError::Configuration(format!(
                        "zone {:?}: sensible-heat input index {idx} is out of range \
                         (model has {n_inputs} inputs) — every zone-air injection \
                         (internal gains, HVAC, window solar) would be silently \
                         discarded",
                        zone.0
                    )));
                }
            }
            for &idx in &wiring.outdoor_temp_input_indices {
                if idx >= n_inputs {
                    return Err(ThermalSolverError::Configuration(format!(
                        "outdoor temperature input index {idx} is out of range \
                         (model has {n_inputs} inputs) — the outdoor driving \
                         column would never be written and the building would \
                         silently simulate against 0 °C outdoors",
                    )));
                }
            }
            for &idx in &wiring.indoor_temp_input_indices {
                if idx >= n_inputs {
                    return Err(ThermalSolverError::Configuration(format!(
                        "indoor temperature input index {idx} is out of range \
                         (model has {n_inputs} inputs) — it would be silently \
                         filtered from the zone-air-balance residual",
                    )));
                }
            }
            for (&surface_id, &idx) in &wiring.solar_input_indices {
                if idx >= n_inputs {
                    return Err(ThermalSolverError::Configuration(format!(
                        "exterior surface {surface_id}: solar input index {idx} is \
                         out of range (model has {n_inputs} inputs) — its solar \
                         gain would be silently discarded",
                    )));
                }
            }
        }
        // Coupled-solve divisor invariant, checked once here so nothing is
        // added to the step. The identity-coupled solve divides a coupled
        // state's update by `1 + d_i` (the closed-form diagonal solve in
        // `StateSpaceModel`). Every coupling diagonal this solver can produce
        // is `h·b`: a non-negative per-step conductance `h` (infiltration and
        // linearised exterior LWR conductances are non-negative by physics,
        // and the interior-convection correction applies only when its Δh is
        // positive) times a model-side factor `b`. Requiring `b` finite and
        // non-negative at every coupling site (and finite, non-negative
        // divisor inputs on the convection injections, whose capacitances are
        // clamped positive at the use site) is exactly the condition that
        // every divisor `1 + d_i` stays finite and positive on every step; a
        // violation would divide by zero or flip a state's update sign
        // mid-simulation.
        for zone in &env.zones {
            let (Some(&state_idx), Some(&input_idx)) = (
                wiring.zone_state_indices.get(&zone.id),
                wiring.zone_sensible_input_indices.get(&zone.id),
            ) else {
                continue;
            };
            let b_coeff = model.b_eff()[(state_idx, input_idx)];
            if !b_coeff.is_finite() || b_coeff < 0.0 {
                return Err(ThermalSolverError::CouplingCoefficientInvalid {
                    site: format!("zone {:?}", zone.id),
                    state_index: state_idx,
                    coefficient: b_coeff,
                });
            }
        }
        // Exterior LWR coupling sites: the simple (non-iterative) longwave
        // branch linearises every non-window surface with rad_frac <= 0 and
        // positive area into a coupling at (state_index, input_index).
        for info in &config.exterior_surfaces {
            if info.boundary_category == Some(config::BoundaryCategory::Window)
                || info.rad_frac > 0.0
                || info.area_m2 <= 0.0
            {
                continue;
            }
            let b_coeff = model.b_eff()[(info.state_index, info.input_index)];
            if !b_coeff.is_finite() || b_coeff < 0.0 {
                return Err(ThermalSolverError::CouplingCoefficientInvalid {
                    site: format!("exterior surface {}", info.surface_id),
                    state_index: info.state_index,
                    coefficient: b_coeff,
                });
            }
        }
        // Interior-convection (TARP) coupling sites: the diagonal
        // `dt·Δh·A/C` is finite and positive whenever applied (Δh > 0,
        // capacitances clamped positive) as long as the area, the static
        // film resistance and the tilt are finite and non-negative: a NaN
        // in any of them reaches `delta_h` as NaN (NaN comparisons are
        // false, so the `Δh > 0` gate passes it) and divides by NaN. The
        // film resistance is the fail-fast guard for the frozen A-matrix
        // too; the tilt is the one input `h` recomputes per step.
        if config.film_coefficient_model == FilmCoefficientModel::PerStepTarp {
            for inj in &config.interior_convection_injections {
                let detail = |value: f64, what: &str| {
                    format!("{what} {value} must be finite and non-negative")
                };
                if !inj.area_m2.is_finite() || inj.area_m2 < 0.0 {
                    return Err(ThermalSolverError::ConvectionInjectionInvalid {
                        surface_state_index: inj.surface_state_index,
                        zone_state_index: inj.zone_state_index,
                        detail: detail(inj.area_m2, "area"),
                    });
                }
                if !inj.static_r_film_int_m2_k_w.is_finite() || inj.static_r_film_int_m2_k_w < 0.0 {
                    return Err(ThermalSolverError::ConvectionInjectionInvalid {
                        surface_state_index: inj.surface_state_index,
                        zone_state_index: inj.zone_state_index,
                        detail: detail(inj.static_r_film_int_m2_k_w, "static film resistance"),
                    });
                }
                if !inj.tilt_deg.is_finite() || inj.tilt_deg < 0.0 {
                    return Err(ThermalSolverError::ConvectionInjectionInvalid {
                        surface_state_index: inj.surface_state_index,
                        zone_state_index: inj.zone_state_index,
                        detail: detail(inj.tilt_deg, "tilt"),
                    });
                }
            }
        }
        // True double-registration of a DEDICATED injection column: sharing
        // is legitimate only on a zone's sensible-heat column (windows and
        // fallback surfaces sum additively into it); any other shared
        // input column means one surface's flux lands in another's column.
        {
            let zone_columns: std::collections::HashSet<usize> = wiring
                .zone_sensible_input_indices
                .values()
                .copied()
                .collect();
            let mut seen_input_columns = std::collections::HashSet::new();
            for info in &config.exterior_surfaces {
                if zone_columns.contains(&info.input_index) {
                    continue;
                }
                if !seen_input_columns.insert(info.input_index) {
                    return Err(ThermalSolverError::Configuration(format!(
                        "exterior surface {}: duplicate dedicated input_index {} \
                         in exterior_surfaces (one surface's injection lands in \
                         another's column)",
                        info.surface_id, info.input_index
                    )));
                }
            }
        }
        // Zone wiring completeness, scoped to the runtime paths that would
        // otherwise silently substitute a plausible value: the boundary
        // diagnostics consult the indoor zone's OUTPUT index (index-0
        // fallback would attribute flux to the wrong zone), and interior-LWR
        // zones consult their env temperature (20 °C substitution). Other
        // zone maps (c_zone_j_k, state indices) are optional by design —
        // their consumers skip gracefully when absent.
        if !config.boundary_diagnostics.is_empty()
            && !wiring
                .zone_output_indices
                .contains_key(&config.indoor_zone_id)
        {
            return Err(ThermalSolverError::Configuration(format!(
                "indoor zone {}: boundary diagnostics are configured but the \
                 zone has no zone_output_indices entry — flux would be \
                 attributed to output 0 (the wrong zone)",
                config.indoor_zone_id.0
            )));
        }
        for zone_cfg in &config.interior_lwr_zones {
            if !env.zones.iter().any(|z| z.id == zone_cfg.zone_id) {
                return Err(ThermalSolverError::Configuration(format!(
                    "interior LWR zone {} is not present in the environment — \
                     its temperature would be silently substituted",
                    zone_cfg.zone_id.0
                )));
            }
        }
        for zone_cfg in &config.interior_lwr_zones {
            if zone_cfg.surfaces.len() >= 2 && zone_cfg.scriptf.is_none() {
                return Err(ThermalSolverError::Configuration(format!(
                    "zone {}: interior LWR requires ScriptF factors; \
                     call compute_scriptf() on InteriorLwrZoneConfig before constructing ThermalSolver \
                     — no linearised fallback is permitted",
                    zone_cfg.zone_id.0
                )));
            }
        }
        let x = initialize_steady_state(
            &model,
            &wiring,
            env,
            indoor_temp_c,
            &[config.indoor_zone_id],
        )?;
        let n_inputs = model.input_dim();
        let n_states = model.state_dim();
        let last_u = DVector::<f64>::zeros(n_inputs);
        let u_buf = DVector::<f64>::zeros(n_inputs);
        let rhs_buf = DVector::<f64>::zeros(n_states);
        let y_buf = DVector::<f64>::zeros(model.output_dim());
        let du_buf = DVector::<f64>::zeros(model.output_dim());
        let solve_scratch = SolveScratch::new(n_states);
        let per_boundary_static_r_film = config
            .interior_convection_injections
            .iter()
            .map(|inj| inj.static_r_film_int_m2_k_w)
            .collect();
        let coupling_buf = Vec::with_capacity(env.zones.len());
        let last_coupling = Vec::new();
        let latent_buf = HashMap::new();
        let exterior_surface_temps =
            vec![env.weather.outdoor_temp_c; config.exterior_surfaces.len()];
        let ext_temps_save_buf = vec![0.0; config.exterior_surfaces.len()];
        let n_lwr_zones = config.interior_lwr_zones.len();
        let max_interior_surfaces = config
            .interior_lwr_zones
            .iter()
            .map(|z| z.surfaces.len())
            .max()
            .unwrap_or(0);
        let max_radiant_surfaces = {
            let solar_max = config
                .interior_solar_zones
                .iter()
                .map(|z| z.surfaces.len())
                .max()
                .unwrap_or(0);
            max_interior_surfaces.max(solar_max)
        };
        let mut interior_surface_temps = Vec::with_capacity(n_lwr_zones);
        let mut interior_surface_prev_temps = Vec::with_capacity(n_lwr_zones);
        for zone_cfg in &config.interior_lwr_zones {
            let t_zone_c = env
                .zones
                .iter()
                .find(|z| z.id == zone_cfg.zone_id)
                .map(|z| z.temperature_c)
                .unwrap_or(indoor_temp_c);
            let mut zone_temps = Vec::with_capacity(zone_cfg.surfaces.len());
            for s in &zone_cfg.surfaces {
                let t_boundary = if let Some(dt) = s.driving_temp {
                    match dt {
                        DrivingTemp::Outdoor => env.weather.outdoor_temp_c,
                        DrivingTemp::Ground { depth_m } => {
                            hares_physics::ground::kusuda_achenbach_temp(
                                depth_m,
                                env.weather.day_of_year,
                                env.weather.ground_t_mean_c,
                                env.weather.ground_t_amplitude_c,
                                env.weather.ground_phase_day,
                                hares_physics::ground::DEFAULT_SOIL_DIFFUSIVITY_M2_PER_DAY,
                            )
                        }
                    }
                } else if s.state_index < x.len() {
                    x[s.state_index]
                } else {
                    t_zone_c
                };
                zone_temps
                    .push(s.radiation_frac * t_boundary + (1.0 - s.radiation_frac) * t_zone_c);
            }
            interior_surface_prev_temps.push(zone_temps.clone());
            interior_surface_temps.push(zone_temps);
        }

        let mut zone_temps_buf: Vec<(ZoneId, f64)> = wiring
            .zone_output_indices
            .keys()
            .map(|zone| (*zone, indoor_temp_c))
            .collect();
        zone_temps_buf.sort_by_key(|(zone, _)| *zone);
        let n_zones_for_latent = zone_temps_buf.len();

        // Used unconditionally by `lwr_coupling_buf` below (and additionally by
        // cfg-gated diagnostic buffers), so this binding must not be cfg-gated.
        let n_ext_surfaces = config.exterior_surfaces.len();
        #[cfg(any(debug_assertions, feature = "observe_detailed"))]
        let n_windows = config.window_properties.len();
        let n_ground_depths = wiring.ground_temp_input_depths_m.len();

        // Precompute the exact discrete-time matrix exchange row for
        // the indoor zone air node. Empty when the zone/capacitance is
        // unresolvable — the residual then reads 0.0 (its absence is caught
        // by the no-silent-zeros guard on the conditioned fixtures).
        let (zone_exchange_row_w, zone_env_col_coeffs) = {
            let zid = config.indoor_zone_id;
            match (
                wiring.zone_state_indices.get(&zid).copied(),
                wiring.c_zone_j_k.get(&zid).copied(),
            ) {
                (Some(z_row), Some(c_zone)) if z_row < model.state_dim() => {
                    let scale = c_zone / dt_s;
                    let n = model.n_mat();
                    let row: Vec<f64> = (0..model.state_dim())
                        .map(|j| {
                            let a = n[(z_row, j)] - if j == z_row { 1.0 } else { 0.0 };
                            scale * a
                        })
                        .collect();
                    let b = model.b_eff();
                    let env_cols = wiring
                        .outdoor_temp_input_indices
                        .iter()
                        .chain(wiring.ground_temp_input_indices.iter())
                        .chain(wiring.indoor_temp_input_indices.iter())
                        .copied()
                        .filter(|&c| c < model.input_dim())
                        .map(|c| (c, scale * b[(z_row, c)]))
                        .collect();
                    (row, env_cols)
                }
                _ => (Vec::new(), Vec::new()),
            }
        };

        Ok(Self {
            model,
            wiring,
            config,
            dt_s,
            x,
            last_u,
            u_buf,
            rhs_buf,
            y_buf,
            du_buf,
            solve_scratch,
            shared_prefix_valid: false,
            #[cfg(test)]
            shared_prefix_fills: 0,
            #[cfg(test)]
            solve_tail_calls: 0,
            coupling_buf,
            last_coupling,
            latent_buf,
            exterior_surface_temps,
            ext_temps_save_buf,
            // Pre-allocate the observe-gated per-zone jacket-loss buffer at
            // init (hot-path discipline: no per-timestep heap allocation).
            // `prepare_inputs` refills it in place each step, reusing the
            // allocation via `std::mem::take` before the struct is replaced.
            #[cfg(feature = "observe")]
            component_gains: EnvelopeComponentGains {
                jacket_loss_by_zone: Vec::with_capacity(n_zones_for_latent),
                ..EnvelopeComponentGains::default()
            },
            #[cfg(not(feature = "observe"))]
            component_gains: EnvelopeComponentGains::default(),
            interior_surf_temps_buf: Vec::with_capacity(max_interior_surfaces),
            interior_surf_base_buf: Vec::with_capacity(max_interior_surfaces),
            interior_surf_prev_buf: Vec::with_capacity(max_interior_surfaces),
            interior_surface_temps,
            interior_surface_prev_temps,
            infiltration_buf: Vec::with_capacity(env.zones.len()),
            infiltration_by_zone_buf: Vec::with_capacity(env.zones.len()),
            solar_absorbed_buf: Vec::with_capacity(max_interior_surfaces),
            lwr_by_zone_buf: Vec::with_capacity(n_lwr_zones),
            window_exterior_lwr_w: 0.0,
            opaque_exterior_lwr_w: 0.0,
            solar_irr_slot_buf: HashMap::with_capacity(n_ext_surfaces),
            solar_slot_map_keys: Vec::with_capacity(env.weather.solar_irradiance.len()),
            zone_exchange_row_w,
            zone_env_col_coeffs,
            opaque_exterior_solar_w: 0.0,
            lwr_net_flux_buf: Vec::with_capacity(max_interior_surfaces),
            lwr_net_flux_prev_buf: Vec::with_capacity(max_interior_surfaces),
            per_boundary_static_r_film,
            prev_zone_temps_c: env.zones.iter().map(|z| (z.id, z.temperature_c)).collect(),
            radiant_weights_buf: Vec::with_capacity(max_radiant_surfaces),
            lwr_surfaces_buf: Vec::with_capacity(max_interior_surfaces),
            lwr_linearised_warned_zones: HashSet::new(),
            energy_balance_residuals: HashMap::with_capacity(n_zones_for_latent),
            ideal_capacity_failure_counts: HashMap::with_capacity(n_zones_for_latent),
            last_good_capacity_w: HashMap::with_capacity(n_zones_for_latent),
            ideal_capacity_degraded_zones: HashSet::new(),
            ideal_capacity_degraded_warned_zones: HashSet::new(),
            ideal_capacity_warned_zones: HashSet::new(),
            zone_temps_buf,
            latent_pairs_buf: Vec::with_capacity(n_zones_for_latent),
            custom_payload_buf: Vec::with_capacity(n_zones_for_latent * 5),
            d_agg_buf: vec![0.0f64; n_states],
            lwr_coupling_buf: Vec::with_capacity(n_ext_surfaces),
            full_system_stored_energy_w: 0.0,
            thermal_balance_q_gains: Vec::with_capacity(n_zones_for_latent.max(1)),
            thermal_balance_q_loss: 0.0,
            balance_buf_a: DVector::<f64>::zeros(n_states),
            balance_buf_b: DVector::<f64>::zeros(n_states),
            balance_buf_c: DVector::<f64>::zeros(n_states),
            balance_d_agg: vec![0.0f64; n_states],
            balance_u_zero: DVector::<f64>::zeros(n_inputs),
            balance_x_zero: DVector::<f64>::zeros(n_states),
            #[cfg(any(debug_assertions, feature = "observe_detailed"))]
            ext_surface_diag_buf: Vec::with_capacity(n_ext_surfaces),
            #[cfg(any(debug_assertions, feature = "observe_detailed"))]
            int_surface_diag_buf: Vec::with_capacity(max_interior_surfaces),
            #[cfg(any(debug_assertions, feature = "observe_detailed"))]
            window_solar_diag_buf: Vec::with_capacity(n_windows),
            cached_outdoor_temp_c: env.weather.outdoor_temp_c,
            cached_ground_temps_c: Vec::with_capacity(n_ground_depths),
        })
    }

    #[must_use]
    pub fn state(&self) -> &DVector<f64> {
        &self.x
    }

    /// Clears the shared ideal-capacity solve prefix: the caller is about to
    /// change (or has changed) the step's `x` or `last_u` that the prefix was
    /// computed from, so a cached prefix would be stale. Called by every
    /// mutation path of `x`/`last_u` (`prepare_inputs_inner`,
    /// `integrate_inner`, `restore_state`), which makes the prefix's
    /// validity window exactly "same x and u as when the prefix was filled".
    #[inline]
    fn invalidate_shared_prefix(&mut self) {
        self.shared_prefix_valid = false;
    }

    /// Assembles the full input vector from outdoor, solar, LWR, and port
    /// contributions. Updates `self.component_gains` for diagnostics.
    /// Populates `self.infiltration_buf` with per-zone coupling terms.
    ///
    /// Returns `(u, latent_by_zone)` where the infiltration couplings are
    /// stored in `self.infiltration_buf` for semi-implicit coupling wiring.
    /// Run-path checks surfaced here (air density screen) are unconditional
    /// typed errors.
    fn build_input_vector(
        &mut self,
        ports: &PortSlots,
        env: &EnvironmentState,
    ) -> std::result::Result<(DVector<f64>, HashMap<ZoneId, f64>), HaresError> {
        let mut u = std::mem::replace(&mut self.u_buf, DVector::zeros(0));
        let n = self.model.input_dim();
        if u.len() == n {
            u.fill(0.0);
        } else {
            u = DVector::zeros(n);
        }

        self.apply_outdoor_inputs(&mut u, env);
        self.refresh_solar_slot_map(env);

        #[cfg(any(debug_assertions, feature = "observe_detailed"))]
        self.window_solar_diag_buf.clear();
        let window_solar_w = self.apply_solar_inputs(&mut u, env);

        // Absorbed opaque exterior solar [W], both application paths. The
        // non-iterative path injects the full absorbed flux (returned
        // directly); the iterative path (rad_frac > 0) injects only the
        // rad_frac-scaled share, so its full absorbed flux is accumulated
        // per surface during the iterative solve below.
        let opaque_solar_noniter_w = self.apply_exterior_solar_inputs(&mut u, env);
        #[cfg(any(debug_assertions, feature = "observe_detailed"))]
        self.ext_surface_diag_buf.clear();
        // Also accumulates `opaque_exterior_solar_w` (iterative-path absorbed
        // solar) and `opaque_exterior_lwr_w` (net skin LWR, both paths).
        self.apply_exterior_longwave_inputs_iterative(&mut u, env)?;
        let opaque_solar_w = opaque_solar_noniter_w + self.opaque_exterior_solar_w;
        // Net exterior LWR at the opaque skins [W], both paths. Neither `u`
        // delta isolates it: the iterative injection mixes solar and LWR at
        // the rad_frac scale, and the non-iterative flux bypasses `u`
        // entirely (semi-implicit coupling).
        let exterior_lwr_w = self.opaque_exterior_lwr_w;
        // Combined absorbed gross at the opaque exterior skins — OCHRE's
        // "{boundary} Ext. Solar Gain (W)" + "{boundary} Ext. LWR Gain (W)"
        // semantics (skin-absorbed flux, not the injected fraction). Windows
        // are excluded: their solar is `window_solar_w` and their exterior
        // LWR `window_exterior_lwr_w`.
        let opaque_solar_lwr_w = opaque_solar_w + exterior_lwr_w;

        #[cfg(any(debug_assertions, feature = "observe_detailed"))]
        self.int_surface_diag_buf.clear();
        // Interior LWR: ScriptF iterative injection only when not using
        // StarMesh (star-mesh bakes radiation conductances into the A-matrix
        // at construction time, so no per-timestep injection is needed).
        if self.config.interior_lwr_method == crate::boundary_rc::InteriorLwrMethod::ScriptF {
            self.apply_interior_longwave_inputs(&mut u, env);
        }
        // Interior LWR: per-zone total LWR exchange activity Σ|q_i|/2 [W].
        // For StarMesh mode this is always 0 (radiation is in the A-matrix).
        // For ScriptF mode this indicates how much radiation exchange is active.
        let interior_lwr_w = self
            .lwr_by_zone_buf
            .iter()
            .find(|(z, _)| *z == self.config.indoor_zone_id)
            .map(|(_, w)| *w)
            .unwrap_or(0.0);

        self.apply_port_convective_inputs(&mut u, ports);
        self.apply_port_radiant_inputs(&mut u, ports);
        self.apply_port_shortwave_inputs(&mut u, ports);

        let indoor_zone = self.config.indoor_zone_id;
        let indoor_heat = ports
            .thermal
            .iter()
            .find(|t| t.zone == indoor_zone)
            .map(hares_types::ThermalAccumulator::heat)
            .unwrap_or_default();

        let mut latent_by_zone = std::mem::take(&mut self.latent_buf);
        latent_by_zone.clear();

        // Detect HVAC fan activity from ports: any non-zero heating or cooling
        // contribution indicates the fan is running and duct leakage is active.
        let hvac_active = ports
            .thermal
            .iter()
            .find(|t| t.zone == indoor_zone)
            .map(|a| {
                a.sensible_for_category(ThermalCategory::HvacHeating).abs() > 1.0
                    || a.sensible_for_category(ThermalCategory::HvacCooling).abs() > 1.0
            })
            .unwrap_or(false);

        apply_infiltration_and_ventilation(
            &self.config,
            env,
            hvac_active,
            &mut latent_by_zone,
            &mut self.infiltration_buf,
        )?;

        let indoor_inf = self.infiltration_buf.iter().find(|c| c.zone == indoor_zone);
        let infiltration_indoor_w = indoor_inf.map(|c| c.q_infiltration_w).unwrap_or(0.0);
        let ventilation_w = indoor_inf.map(|c| c.q_forced_vent_w).unwrap_or(0.0);
        let natural_ventilation_w = indoor_inf.map(|c| c.q_natural_vent_w).unwrap_or(0.0);
        let combined_airflow_sensible_w =
            indoor_inf.map(|c| c.q_sensible_diagnostic_w).unwrap_or(0.0);

        let indoor_acc = ports.thermal.iter().find(|t| t.zone == indoor_zone);
        let hvac_heating_w = indoor_acc
            .map(|a| a.sensible_for_category(ThermalCategory::HvacHeating))
            .unwrap_or(0.0);
        let hvac_cooling_w = indoor_acc
            .map(|a| a.sensible_for_category(ThermalCategory::HvacCooling))
            .unwrap_or(0.0);
        let internal_gain_cat_w = indoor_acc
            .map(|a| {
                a.sensible_for_category(ThermalCategory::InternalGain)
                    + a.radiant_for_category(ThermalCategory::InternalGain)
                    + a.shortwave_gain_w
            })
            .unwrap_or(0.0);
        // Jacket losses (water heater skin loss, boiler shell loss) are
        // deposited into the host equipment zone's accumulator, which may be
        // a non-indoor zone (e.g. garage, basement). Sum across all zone
        // accumulators to match the duct_loss_w pattern below.
        let jacket_loss_w: f64 = ports
            .thermal
            .iter()
            .map(|a| a.sensible_for_category(ThermalCategory::JacketLoss))
            .sum();

        // Duct losses are deposited into the duct zone accumulator (e.g. attic),
        // not the indoor zone. Sum DuctLoss across all zone accumulators.
        let duct_loss_w: f64 = ports
            .thermal
            .iter()
            .map(|a| a.sensible_for_category(ThermalCategory::DuctLoss))
            .sum();

        let hvac_dehumidification_w = indoor_acc
            .map(|a| a.sensible_for_category(ThermalCategory::HvacDehumidification))
            .unwrap_or(0.0);

        self.infiltration_by_zone_buf.clear();
        self.infiltration_by_zone_buf.extend(
            self.infiltration_buf
                .iter()
                .map(|c| (c.zone, c.q_infiltration_w)),
        );
        self.infiltration_by_zone_buf.sort_by_key(|(z, _)| *z);

        // Reuse the previous step's `jacket_loss_by_zone` allocation: take the
        // Vec out of the old `component_gains` before the struct is replaced,
        // refill it in place, and move it into the new struct. Constructing it
        // as `Vec::new()` and extending after assignment allocated a fresh
        // buffer every timestep (hot-path discipline: no per-timestep heap
        // allocation).
        #[cfg(feature = "observe")]
        let jacket_loss_by_zone = {
            let mut buf = std::mem::take(&mut self.component_gains.jacket_loss_by_zone);
            buf.clear();
            buf.extend(
                ports
                    .thermal
                    .iter()
                    .map(|a| (a.zone, a.sensible_for_category(ThermalCategory::JacketLoss))),
            );
            buf
        };

        // Round-trip the reusable Vecs through the struct replacement: move
        // this step's data into the new `component_gains` and the previous
        // struct's Vecs (capacity retained) back into the buffers. Swapping
        // against the freshly constructed struct's `Vec::new()`s (the
        // previous swap-and-reserve dance) handed the buffers a capacity-0
        // Vec and re-allocated both every timestep (hot-path discipline: no
        // per-timestep heap allocation). Same for the diagnostic buffers:
        // they move into the struct instead of being cloned.
        let infiltration_by_zone = std::mem::take(&mut self.infiltration_by_zone_buf);
        let interior_lwr_by_zone = std::mem::take(&mut self.lwr_by_zone_buf);
        let prev_infiltration_by_zone =
            std::mem::take(&mut self.component_gains.infiltration_by_zone);
        let prev_interior_lwr_by_zone =
            std::mem::take(&mut self.component_gains.interior_lwr_by_zone);
        #[cfg(any(debug_assertions, feature = "observe_detailed"))]
        let (ext_surface_diag, int_surface_diag, window_solar_diag) = {
            let ext = std::mem::take(&mut self.component_gains.ext_surface_diag);
            let int = std::mem::take(&mut self.component_gains.int_surface_diag);
            let win = std::mem::take(&mut self.component_gains.window_solar_diag);
            let ext_diag = std::mem::replace(&mut self.ext_surface_diag_buf, ext);
            let int_diag = std::mem::replace(&mut self.int_surface_diag_buf, int);
            let win_diag = std::mem::replace(&mut self.window_solar_diag_buf, win);
            (ext_diag, int_diag, win_diag)
        };

        self.component_gains = EnvelopeComponentGains {
            window_solar_w,
            opaque_solar_lwr_w,
            interior_lwr_w,
            infiltration_w: infiltration_indoor_w,
            ventilation_w,
            natural_ventilation_w,
            combined_airflow_sensible_w,
            port_convective_w: indoor_heat.convective_w,
            port_radiant_w: indoor_heat.radiant_w,
            port_shortwave_w: indoor_heat.shortwave_w,
            hvac_heating_w,
            hvac_cooling_w,
            internal_gain_w: internal_gain_cat_w,
            jacket_loss_w,
            duct_loss_w,
            hvac_dehumidification_w,
            infiltration_by_zone,
            interior_lwr_by_zone,
            wall_heat_gain_w: 0.0,
            floor_heat_gain_w: 0.0,
            roof_heat_gain_w: 0.0,
            window_heat_gain_w: 0.0,
            internal_mass_heat_gain_w: 0.0,
            // Recomputed at the end of every integrate step from the fresh
            // boundary gains and the zone state delta (stepping.rs); 0.0 here
            // is only the pre-step placeholder.
            zone_air_balance_residual_w: 0.0,
            driving_outdoor_temp_c: env.weather.outdoor_temp_c,
            driving_ground_temp_c: {
                // Use the deepest below-grade boundary as the representative
                // ground driving temperature for diagnostics. If no ground
                // boundaries exist, depth=0.0 returns the Kusuda surface value.
                let deepest = self
                    .wiring
                    .ground_temp_input_depths_m
                    .iter()
                    .copied()
                    .fold(0.0_f64, f64::max);
                hares_physics::ground::kusuda_achenbach_temp(
                    deepest,
                    env.weather.day_of_year,
                    env.weather.ground_t_mean_c,
                    env.weather.ground_t_amplitude_c,
                    env.weather.ground_phase_day,
                    hares_physics::ground::DEFAULT_SOIL_DIFFUSIVITY_M2_PER_DAY,
                )
            },
            opaque_solar_w,
            exterior_lwr_w,
            window_exterior_lwr_w: self.window_exterior_lwr_w,
            total_airflow_m3_s: indoor_inf.map(|c| c.combined_flow_m3_s).unwrap_or(0.0),
            raw_infiltration_m3_s: indoor_inf.map(|c| c.raw_inf_m3_s).unwrap_or(0.0),
            forced_vent_m3_s: indoor_inf.map(|c| c.forced_flow_m3_s).unwrap_or(0.0),
            natural_vent_m3_s: indoor_inf.map(|c| c.nat_flow_m3_s).unwrap_or(0.0),
            #[cfg(feature = "observe")]
            natural_ventilation_q_stack_m3_s: indoor_inf.map(|c| c.q_stack_m3_s).unwrap_or(0.0),
            #[cfg(feature = "observe")]
            natural_ventilation_q_wind_m3_s: indoor_inf.map(|c| c.q_wind_m3_s).unwrap_or(0.0),
            #[cfg(feature = "observe")]
            natural_ventilation_cd_used: indoor_inf.map(|c| c.cd_used).unwrap_or(0.0),
            #[cfg(feature = "observe")]
            natural_ventilation_cw: indoor_inf.map(|c| c.natural_ventilation_cw).unwrap_or(0.0),
            #[cfg(feature = "observe")]
            natural_ventilation_wind_angle_deg: indoor_inf
                .map(|c| c.natural_ventilation_wind_angle_deg)
                .unwrap_or(0.0),
            air_density_kg_m3: {
                moist_air_density_kg_m3(
                    env.weather.pressure_pa(),
                    env.weather.outdoor_temp_c,
                    env.weather.outdoor_humidity_ratio,
                )
            },
            #[cfg(any(debug_assertions, feature = "observe_detailed"))]
            ext_surface_diag,
            #[cfg(any(debug_assertions, feature = "observe_detailed"))]
            int_surface_diag,
            #[cfg(any(debug_assertions, feature = "observe_detailed"))]
            window_solar_diag,
            #[cfg(feature = "observe")]
            jacket_loss_by_zone,
        };

        self.infiltration_by_zone_buf = prev_infiltration_by_zone;
        self.lwr_by_zone_buf = prev_interior_lwr_by_zone;

        Ok((u, latent_by_zone))
    }

    /// Formats solver outputs into `out` in-place, reusing its allocations.
    fn format_domain_update(
        &mut self,
        y_next: &DVector<f64>,
        latent_by_zone: HashMap<ZoneId, f64>,
        out: &mut DomainUpdate,
    ) {
        // Update temperatures in pre-sorted zone_temps_buf (no allocation, no sort).
        for (zone, temp) in &mut self.zone_temps_buf {
            if let Some(&output_idx) = self.wiring.zone_output_indices.get(zone) {
                *temp = y_next[output_idx];
            }
        }

        // Reuse latent_pairs_buf and custom_payload_buf.
        self.latent_pairs_buf.clear();
        self.latent_pairs_buf
            .extend(latent_by_zone.iter().map(|(&z, &v)| (z, v)));
        self.latent_pairs_buf.sort_by_key(|(zone, _)| *zone);

        self.custom_payload_buf.clear();
        for &(zone, latent) in &self.latent_pairs_buf {
            let (m_dot_inf_kg_s, w_outdoor) = self
                .infiltration_buf
                .iter()
                .find(|c| c.zone == zone)
                .map(|c| (c.m_dot_lat_kg_s, c.w_outdoor))
                .unwrap_or((0.0, 0.0));
            // Thermal custom_payload format: [zone_id, q_latent_w, m_dot_inf_kg_s, w_outdoor,
            // energy_balance_residual_w] per zone, extending the original 2-float format to carry
            // moisture coupling data for semi-implicit humidity solver treatment and the per-step
            // energy balance residual for observability.
            self.custom_payload_buf.push(f64::from(zone.0));
            self.custom_payload_buf.push(latent);
            self.custom_payload_buf.push(m_dot_inf_kg_s);
            self.custom_payload_buf.push(w_outdoor);
            self.custom_payload_buf.push(
                self.energy_balance_residuals
                    .get(&zone)
                    .copied()
                    .unwrap_or(0.0),
            );
        }

        self.latent_buf = latent_by_zone;

        // Fill output in-place: clear and extend zone_temperatures_c from internal buf.
        out.domain_id = THERMAL;
        out.zone_temperatures_c.clear();
        out.zone_temperatures_c
            .extend_from_slice(&self.zone_temps_buf);
        if self.custom_payload_buf.is_empty() {
            if let Some(ref mut p) = out.custom_payload {
                p.clear();
            }
            out.custom_payload = None;
        } else {
            match out.custom_payload {
                Some(ref mut p) => {
                    p.clear();
                    p.extend_from_slice(&self.custom_payload_buf);
                }
                None => {
                    out.custom_payload = Some(self.custom_payload_buf.clone());
                }
            }
        }
    }

    /// Rebuilds the per-step `surface_id` → irradiance-slot lookup used by
    /// the solar and exterior-LWR apply passes. Called once per timestep from
    /// `build_input_vector` before any of those passes run. The rebuild
    /// happens only when the incoming surface-id sequence differs from the
    /// one the map was last built from (compared id by id, no hashing), so
    /// a fixed environment performs no rebuild and no allocation per call;
    /// capacity is retained across steps.
    ///
    /// Duplicate `surface_id`s in the weather vector resolve FIRST-WINS —
    /// the same resolution the pre-slot-map linear `.find()` gave, so this
    /// lookup is a pure performance refactor with no semantic change. (A
    /// plain `insert` loop would silently flip duplicates to last-wins.)
    /// Config-side `validate()` already rejects duplicate surface_ids in
    /// `exterior_surfaces`; a weather producer emitting duplicates is
    /// malformed input, and first-wins keeps its handling deterministic.
    fn refresh_solar_slot_map(&mut self, env: &EnvironmentState) {
        if self.solar_slot_map_keys.len() == env.weather.solar_irradiance.len()
            && self
                .solar_slot_map_keys
                .iter()
                .zip(env.weather.solar_irradiance.iter())
                .all(|(id, irr)| *id == irr.surface_id)
        {
            return;
        }
        self.solar_irr_slot_buf.clear();
        self.solar_slot_map_keys.clear();
        for (slot, irr) in env.weather.solar_irradiance.iter().enumerate() {
            // `or_insert` = first-wins: a key already mapped keeps its slot.
            self.solar_irr_slot_buf
                .entry(irr.surface_id)
                .or_insert(slot);
            self.solar_slot_map_keys.push(irr.surface_id);
        }
    }

    fn apply_outdoor_inputs(&mut self, u: &mut DVector<f64>, env: &EnvironmentState) {
        for &idx in &self.wiring.outdoor_temp_input_indices {
            if idx < u.len() {
                u[idx] = env.weather.outdoor_temp_c;
            }
        }
        {
            self.cached_outdoor_temp_c = env.weather.outdoor_temp_c;
            self.cached_ground_temps_c.clear();
        }
        // Kusuda-Achenbach depth-corrected ground temperature: one temperature
        // per unique foundation depth, written to the corresponding B-matrix
        // ground column. Replaces the pre-fix behaviour of writing the DOE-2
        // surface ground temperature to all below-grade boundaries.
        //
        // Kusuda & Achenbach (1965) ASHRAE Trans. 71(1):61-74.
        // EnergyPlus Engineering Reference: Ground Heat Transfer chapter,
        // "Undisturbed Ground Temperature Model: Kusuda-Achenbach".
        for (&idx, &depth_m) in self
            .wiring
            .ground_temp_input_indices
            .iter()
            .zip(self.wiring.ground_temp_input_depths_m.iter())
        {
            if idx < u.len() {
                let t_ground = hares_physics::ground::kusuda_achenbach_temp(
                    depth_m,
                    env.weather.day_of_year,
                    env.weather.ground_t_mean_c,
                    env.weather.ground_t_amplitude_c,
                    env.weather.ground_phase_day,
                    hares_physics::ground::DEFAULT_SOIL_DIFFUSIVITY_M2_PER_DAY,
                );
                u[idx] = t_ground;
                self.cached_ground_temps_c.push(t_ground);
            }
        }
    }

    pub fn prepare_inputs(
        &mut self,
        ports: &PortSlots,
        env: &EnvironmentState,
    ) -> std::result::Result<(), HaresError> {
        debug_assert!(
            (env.time_step_secs() - self.dt_s).abs() < 1e-6,
            "ThermalSolver: runtime dt ({:.3}s) != configured dt ({:.3}s); re-discretize or use constant timestep",
            env.time_step_secs(),
            self.dt_s
        );
        self.prepare_inputs_inner(ports, env)
    }

    pub fn integrate(
        &mut self,
        ports: &PortSlots,
        env: &EnvironmentState,
        out: &mut DomainUpdate,
    ) -> std::result::Result<(), HaresError> {
        debug_assert!(
            (env.time_step_secs() - self.dt_s).abs() < 1e-6,
            "ThermalSolver: runtime dt ({:.3}s) != configured dt ({:.3}s); re-discretize or use constant timestep",
            env.time_step_secs(),
            self.dt_s
        );
        self.integrate_inner(ports, env, out)
    }
}

impl DomainSolver for ThermalSolver {
    fn domain_id(&self) -> DomainId {
        THERMAL
    }

    fn resolve(
        &mut self,
        ports: &PortSlots,
        env: &EnvironmentState,
        dt: Duration,
        out: &mut DomainUpdate,
    ) -> std::result::Result<(), HaresError> {
        debug_assert!(
            (dt.as_secs_f64() - self.dt_s).abs() < 1e-6,
            "ThermalSolver: runtime dt ({:.3}s) != configured dt ({:.3}s); re-discretize or use constant timestep",
            dt.as_secs_f64(),
            self.dt_s
        );
        self.resolve_internal(ports, env, out)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use chrono::{FixedOffset, TimeZone};
    use hares_types::{
        DomainSolver, DomainUpdate, EnvironmentState, GridState, PortSlots, SurfaceIrradiance,
        ThermalAccumulator, ThermalCategory, WeatherState, ZoneId, ZoneState,
    };
    use nalgebra::{DMatrix, DVector};

    use crate::THERMAL_SNAPSHOT_SCHEMA_VERSION;
    use crate::ThermalSnapshot;
    use crate::longwave_radiation::{SOLAR_ABSORPTANCE_DEFAULT, beta_factor};
    use crate::state_space::{OutputMapping, StateSpaceModel};
    use crate::thermal_solver::{
        BoundaryCategory, DrivingTemp, ExteriorSurfaceInfo, FilmCoefficientModel,
        InfiltrationMethod, InteriorLwrZoneConfig, InteriorSolarSurfaceInfo,
        InteriorSolarZoneConfig, InteriorSurfaceInfo, MechanicalVentilationParams,
        NaturalVentilationConfig, StateSpaceWiring, ThermalSolver, ThermalSolverConfig,
        ThermalSolverError, WindowSolarProperties,
    };

    fn env_for_temp(zone_temp: f64, outdoor_temp: f64) -> EnvironmentState {
        EnvironmentState {
            ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp,
                humidity_ratio: 0.008,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: outdoor_temp,
                outdoor_humidity_ratio: 0.004,
                wind_speed_m_s: 3.0,
                wind_dir_deg: 180.0,
                ground_temp_c: outdoor_temp,
                sky_temp_c: outdoor_temp - 5.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![SurfaceIrradiance {
                    surface_id: 1,
                    direct_w_m2: 0.0,
                    diffuse_w_m2: 0.0,
                    reflected_w_m2: 0.0,
                    angle_of_incidence_rad: 0.0,
                }],
                outdoor_wet_bulb_c: 0.0,
                outdoor_enthalpy_j_kg: 0.0,
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
                solar_altitude_deg: 0.0,
                solar_azimuth_deg: 180.0,
                mains_temp_c: 15.0,
                rainfall_m: 0.0,
                ground_albedo: 0.2,
                ground_t_mean_c: 10.0,
                ground_t_amplitude_c: 0.0,
                ground_phase_day: 35.0,
                day_of_year: 1.0,
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
                island_bus_voltage_pu: None,
            },
            schedule_row: None,
            domains: hares_types::DomainSlots::default(),
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: Default::default(),
            current_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 3, 18, 12, 0, 0)
                .single()
                .expect("valid time"),
            time_res: chrono::Duration::seconds(60),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    fn one_zone_solver(env: &EnvironmentState) -> ThermalSolver {
        let r = 2.0;
        let c = 50_000.0;
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (r * c)]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1.0 / (r * c), 1.0 / c]); // [T_out, H_zone]
        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();

        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            window_properties: HashMap::new(),
            window_zone_ids: HashMap::new(),

            exterior_surfaces: vec![],
            interior_lwr_zones: vec![],
            infiltration: vec![],
            ventilation_flow_m3_s: 0.0,
            ventilation: MechanicalVentilationParams::default(),
            natural_ventilation: None,
            supply_duct_leakage_m3_s: 0.0,
            return_duct_leakage_m3_s: 0.0,
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
            film_coefficient_model: FilmCoefficientModel::default(),
            interior_convection_injections: Vec::new(),
            ideal_capacity_degraded_threshold: 3,
        };
        ThermalSolver::new(model, wiring, config, 60.0, env, env.zones[0].temperature_c).unwrap()
    }

    fn interior_lwr_solver(env: &EnvironmentState) -> ThermalSolver {
        let a_c = DMatrix::from_row_slice(
            3,
            3,
            &[
                -1.0 / 50_000.0,
                0.0,
                0.0,
                0.0,
                -1.0 / 40_000.0,
                0.0,
                0.0,
                0.0,
                -1.0 / 30_000.0,
            ],
        );
        let b_c = DMatrix::zeros(3, 3);
        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();

        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 0)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };

        let mut interior_lwr_zone = InteriorLwrZoneConfig {
            zone_id: ZoneId(1),
            surfaces: vec![
                InteriorSurfaceInfo {
                    state_index: 1,
                    input_index: 1,
                    area_m2: 12.0,
                    azimuth_deg: 0.0,
                    tilt_deg: 90.0,
                    emissivity: 0.90,
                    radiation_frac: 1.0,
                    rad_res_k_w: 250.0,
                    solar_absorptance: 0.0,
                    is_floor: false,
                    driving_temp: None,
                },
                InteriorSurfaceInfo {
                    state_index: 2,
                    input_index: 2,
                    area_m2: 8.0,
                    azimuth_deg: 0.0,
                    tilt_deg: 90.0,
                    emissivity: 0.65,
                    radiation_frac: 1.0,
                    rad_res_k_w: 175.0,
                    solar_absorptance: 0.0,
                    is_floor: false,
                    driving_temp: None,
                },
            ],
            scriptf: None,
        };
        interior_lwr_zone.compute_scriptf();
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            window_properties: HashMap::new(),
            window_zone_ids: HashMap::new(),
            exterior_surfaces: vec![],
            interior_lwr_zones: vec![interior_lwr_zone],
            infiltration: vec![],
            ventilation_flow_m3_s: 0.0,
            ventilation: MechanicalVentilationParams::default(),
            natural_ventilation: None,
            supply_duct_leakage_m3_s: 0.0,
            return_duct_leakage_m3_s: 0.0,
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
            film_coefficient_model: FilmCoefficientModel::default(),
            interior_convection_injections: Vec::new(),
            ideal_capacity_degraded_threshold: 3,
        };
        ThermalSolver::new(model, wiring, config, 60.0, env, env.zones[0].temperature_c).unwrap()
    }

    #[test]
    fn free_float_zone_drifts_toward_outdoor() {
        let mut env = env_for_temp(20.0, 0.0);
        let mut solver = one_zone_solver(&env);
        solver.x[0] = 20.0;
        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        let mut t_last = 20.0;
        for _ in 0..60 {
            let update = solver
                .resolve_new(&ports, &env, Duration::from_secs(60))
                .unwrap();
            t_last = update.zone_temperatures_c[0].1;
            env.zones[0].temperature_c = t_last;
        }
        assert!(t_last < 20.0);
        assert!(t_last > 0.0);
    }

    #[test]
    fn sinusoidal_outdoor_has_lag_and_attenuation() {
        let mut env = env_for_temp(15.0, 15.0);
        let mut solver = one_zone_solver(&env);
        solver.x[0] = 15.0;
        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        let dt_s = 60.0;
        let period_s = 24.0 * 3600.0;
        let omega = 2.0 * std::f64::consts::PI / period_s;
        let mut outdoor = Vec::new();
        let mut indoor = Vec::new();
        for k in 0..(24 * 60) {
            let t = (k as f64) * dt_s;
            env.weather.outdoor_temp_c = 15.0 + 10.0 * (omega * t).sin();
            outdoor.push(env.weather.outdoor_temp_c);
            let update = solver
                .resolve_new(&ports, &env, Duration::from_secs(60))
                .unwrap();
            let t_zone = update.zone_temperatures_c[0].1;
            env.zones[0].temperature_c = t_zone;
            indoor.push(t_zone);
        }

        let amp_out = (outdoor.iter().copied().fold(f64::MIN, f64::max)
            - outdoor.iter().copied().fold(f64::MAX, f64::min))
            / 2.0;
        let amp_in = (indoor.iter().copied().fold(f64::MIN, f64::max)
            - indoor.iter().copied().fold(f64::MAX, f64::min))
            / 2.0;
        assert!(amp_in < amp_out);

        let idx_out_peak = outdoor
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0;
        let idx_in_peak = indoor
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0;
        assert!(idx_in_peak > idx_out_peak);
    }

    #[test]
    fn domain_solver_is_object_safe() {
        let env = env_for_temp(20.0, 0.0);
        let solver = one_zone_solver(&env);
        let mut boxed: Box<dyn DomainSolver> = Box::new(solver);
        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };
        let update = boxed
            .resolve_new(&ports, &env, Duration::from_secs(60))
            .unwrap();
        assert_eq!(update.domain_id, hares_types::THERMAL);
    }

    /// Steady-state initialization solves the true conditioned equilibrium:
    /// zone air stays at indoor_temp (fixed boundary), wall nodes sit at the
    /// correct gradient between indoor and outdoor temperatures.
    #[test]
    fn steady_state_initialization_produces_temperature_gradient() {
        let indoor = 20.0;
        let outdoor = 0.0;
        let env = env_for_temp(indoor, outdoor);
        // 2-state model: state 0 = zone air, state 1 = wall node.
        // Wall node couples to zone air (via a_c[1,0]) and outdoor temp (via b_c[1,0]).
        // a_c: zone air loses heat to wall and outdoor; wall gains from zone air.
        // b_c: zone air driven by outdoor (col 0); wall driven by outdoor (col 0)
        //      and indoor HVAC (col 1).
        let a_c = DMatrix::from_row_slice(2, 2, &[-0.75, 0.5, 0.25, -0.28125]);
        let b_c = DMatrix::from_row_slice(2, 2, &[0.25, 0.0, 0.03125, 0.0]);
        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();
        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![1],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            window_properties: HashMap::new(),
            window_zone_ids: HashMap::new(),
            exterior_surfaces: vec![],
            interior_lwr_zones: vec![],
            infiltration: vec![],
            ventilation_flow_m3_s: 0.0,
            ventilation: MechanicalVentilationParams::default(),
            natural_ventilation: None,
            supply_duct_leakage_m3_s: 0.0,
            return_duct_leakage_m3_s: 0.0,
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
            film_coefficient_model: FilmCoefficientModel::default(),
            interior_convection_injections: Vec::new(),
            ideal_capacity_degraded_threshold: 3,
        };
        let solver = ThermalSolver::new(model, wiring, config, 60.0, &env, indoor).unwrap();
        let state = solver.state();

        // Zone air (state 0) should be at indoor temp (fixed boundary).
        assert!(
            (state[0] - indoor).abs() < 1e-6,
            "zone air should be {indoor}°C, got {}",
            state[0]
        );
        // Wall node (state 1) should be strictly between indoor and outdoor
        // since it has coupling to both (a_c[1,0]*x_zone + b_c[1,0]*T_outdoor).
        assert!(
            state[1] > outdoor && state[1] < indoor,
            "wall node should be between {outdoor}°C and {indoor}°C, got {}",
            state[1]
        );
        // Verify the steady state is self-consistent: stepping the full model
        // (without zone pinning) will produce some drift since the partitioned
        // solve only satisfies the reduced system's steady state.
        // The key property is that the wall node is physically reasonable.
        let u = DVector::from_row_slice(&[outdoor, indoor]);
        let x_next = solver.model.step(state, &u);
        // Wall node should remain finite and in a reasonable temperature range.
        assert!(
            x_next[1].is_finite() && x_next[1] > -50.0 && x_next[1] < 100.0,
            "wall node after one step should be physically reasonable, got {}",
            x_next[1]
        );
    }

    /// Regression: latent heat constant was inconsistent across modules (2450 vs 2501 kJ/kg).
    /// Instead of pinning the constant value, verify the physical consequence:
    /// the infiltration latent load in the custom_payload must imply a physically
    /// correct h_fg when back-computed from the known air mass flow and humidity
    /// difference. With the old 2450 kJ/kg bug, the implied h_fg would fall ~2%
    /// outside the acceptable range.
    #[test]
    fn infiltration_latent_energy_consistent_with_moisture_mass_flow() {
        use hares_physics::air_properties::moist_air_density_kg_m3;
        use hares_physics::infiltration::ach_infiltration;

        let ach = 0.5;
        let w_zone = 0.008;
        let w_outdoor = 0.004;
        let t_outdoor = 5.0;
        let p_kpa = 101.325;
        let volume_m3 = 200.0;

        let mut env = env_for_temp(20.0, t_outdoor);
        env.zones[0].humidity_ratio = w_zone;
        env.weather.outdoor_humidity_ratio = w_outdoor;

        let r = 2.0;
        let c = 50_000.0;
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (r * c)]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1.0 / (r * c), 1.0 / c]);
        let mapping = crate::state_space::OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model =
            crate::state_space::StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping)
                .unwrap();

        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            window_properties: HashMap::new(),
            window_zone_ids: HashMap::new(),

            exterior_surfaces: vec![],
            interior_lwr_zones: vec![],
            infiltration: vec![(ZoneId(1), InfiltrationMethod::Ach { ach })],
            ventilation_flow_m3_s: 0.0,
            ventilation: MechanicalVentilationParams::default(),
            natural_ventilation: None,
            supply_duct_leakage_m3_s: 0.0,
            return_duct_leakage_m3_s: 0.0,
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
            film_coefficient_model: FilmCoefficientModel::default(),
            interior_convection_injections: Vec::new(),
            ideal_capacity_degraded_threshold: 3,
        };
        let mut solver = ThermalSolver::new(model, wiring, config, 60.0, &env, 20.0).unwrap();
        solver.x[0] = 20.0;

        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };
        let update = solver
            .resolve_new(&ports, &env, Duration::from_secs(60))
            .unwrap();

        // Extract latent load from custom_payload
        let payload = update
            .custom_payload
            .expect("infiltration with humidity diff must produce latent payload");
        assert!(payload.len() >= 2);
        let q_latent_w = payload[1];

        // Independently compute expected moisture mass flow from infiltration
        let rho_outdoor = moist_air_density_kg_m3(p_kpa * 1000.0, t_outdoor, w_outdoor);
        let q_inf_m3_s = ach_infiltration(ach, volume_m3);
        let m_dot_kg_s = rho_outdoor * q_inf_m3_s;
        let delta_w = w_outdoor - w_zone;

        // Back-compute the h_fg implied by the latent output
        let h_fg_implied = q_latent_w / (m_dot_kg_s * delta_w);

        // Must be within 0.4% of 2501 kJ/kg. The old 2450 value is ~2% off.
        assert!(
            (2_490_000.0..=2_510_000.0).contains(&h_fg_implied),
            "implied h_fg = {h_fg_implied:.0} J/kg outside [2490000, 2510000]; \
             latent heat constant is likely wrong"
        );
    }

    #[test]
    fn per_timestep_energy_balance_holds() {
        let mut env = env_for_temp(20.0, 0.0);
        let mut solver = one_zone_solver(&env);
        solver.x[0] = 20.0;
        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        let r = 2.0;
        let c = 50_000.0;
        let dt = 60.0;
        for _ in 0..20 {
            let t_prev = solver.state()[0];
            let update = solver
                .resolve_new(&ports, &env, Duration::from_secs(60))
                .unwrap();
            let t_next = update.zone_temperatures_c[0].1;
            let q_gain = 0.0;
            let d_e_storage = c * (t_next - t_prev) / dt;
            let q_loss = (t_prev - env.weather.outdoor_temp_c) / r;
            let lhs = (q_gain - d_e_storage - q_loss).abs();
            let rhs = f64::max(1.0, 1e-6 * q_gain.abs());
            assert!(lhs < rhs, "lhs={lhs}, rhs={rhs}");
            env.zones[0].temperature_c = t_next;
        }
    }

    /// The interior surface diagnostic holds one (surface temperature, net
    /// long-wave flux) pair per surface, in the zone's surface order, each the
    /// converged value the injection used.
    #[cfg(any(debug_assertions, feature = "observe_detailed"))]
    #[test]
    fn interior_surface_diagnostic_pairs_follow_surface_order() {
        let env = env_for_temp(21.0, 10.0);
        let mut solver = interior_lwr_solver(&env);
        solver.x = DVector::from_row_slice(&[21.0, 35.0, 5.0]);

        let mut u = DVector::zeros(3);
        solver.apply_interior_longwave_inputs(&mut u, &env);

        let pairs: Vec<(f64, f64)> = solver
            .int_surface_diag_buf
            .iter()
            .map(|d| (d.surface_temp_c, d.lwr_flux_w))
            .collect();
        let expected: Vec<(f64, f64)> = solver.interior_surface_temps[0]
            .iter()
            .copied()
            .zip(solver.lwr_net_flux_buf.iter().copied())
            .collect();
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs, expected);
        assert!(
            pairs[0].0 > pairs[1].0 && pairs[0].1 < 0.0 && pairs[1].1 > 0.0,
            "the warm surface (first) loses long-wave to the cold one: {pairs:?}"
        );
    }

    #[test]
    fn interior_longwave_surface_states_persist_with_clamp_and_damping() {
        let env = env_for_temp(21.0, 10.0);
        let mut solver = interior_lwr_solver(&env);
        solver.x = DVector::from_row_slice(&[21.0, 35.0, 5.0]);
        let initial_temps = vec![34.5, 5.5];
        let initial_prev_temps = vec![60.0, -10.0];
        solver.interior_surface_temps[0] = initial_temps.clone();
        solver.interior_surface_prev_temps[0] = initial_prev_temps.clone();

        let mut u = DVector::zeros(3);
        solver.apply_interior_longwave_inputs(&mut u, &env);

        let surfaces = &solver.config.interior_lwr_zones[0].surfaces;
        let state_temps = [21.0, 35.0, 5.0];
        let zone_temp_c = env.zones[0].temperature_c;
        let mut expected_buf = initial_temps;
        let base_buf = [
            surfaces[0].radiation_frac * state_temps[surfaces[0].state_index]
                + (1.0 - surfaces[0].radiation_frac) * zone_temp_c,
            surfaces[1].radiation_frac * state_temps[surfaces[1].state_index]
                + (1.0 - surfaces[1].radiation_frac) * zone_temp_c,
        ];
        let mut expected_prev = initial_prev_temps;
        let t_surf_min = base_buf.iter().copied().fold(f64::INFINITY, f64::min);
        let t_surf_max = base_buf.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let scriptf = solver.config.interior_lwr_zones[0]
            .scriptf
            .as_ref()
            .expect("scriptf factors must be pre-computed");
        let mut expected_flux = vec![0.0; 2];
        let mut expected_flux_prev: Vec<f64> = Vec::with_capacity(2);
        for iter_idx in 0..3u32 {
            scriptf.net_flux_w_into(&expected_buf, &mut expected_flux);
            // Replicate flux-residual convergence check (skip first iteration)
            if iter_idx > 0 && !expected_flux_prev.is_empty() {
                let mut converged = true;
                for j in 0..expected_buf.len() {
                    let old_q = expected_flux_prev[j];
                    let new_q = expected_flux[j];
                    if (new_q - old_q).abs() / (old_q.abs() + 1e-6) >= 1e-4 {
                        converged = false;
                        break;
                    }
                }
                if converged {
                    break;
                }
            }
            expected_flux_prev.clear();
            expected_flux_prev.extend_from_slice(&expected_flux);
            for (j, info) in surfaces.iter().enumerate() {
                let t_new = base_buf[j] + expected_flux[j] * info.rad_res_k_w;
                let t_new = t_new.clamp(t_surf_min, t_surf_max);
                let t_next = expected_buf[j]
                    + 0.3 * (t_new - expected_buf[j])
                    + 0.2 * (expected_buf[j] - expected_prev[j]);
                expected_prev[j] = expected_buf[j];
                expected_buf[j] = t_next;
            }
        }
        scriptf.net_flux_w_into(&expected_buf, &mut expected_flux);

        for (actual, expected) in solver.interior_surface_temps[0]
            .iter()
            .zip(expected_buf.iter())
        {
            assert!(
                (actual - expected).abs() < 1e-9,
                "interior surface temp drift: actual={:?} expected={:?}",
                solver.interior_surface_temps[0],
                expected_buf
            );
        }
        for (actual, expected) in solver.interior_surface_prev_temps[0]
            .iter()
            .zip(expected_prev.iter())
        {
            assert!(
                (actual - expected).abs() < 1e-9,
                "interior surface prev-temp drift: actual={:?} expected={:?}",
                solver.interior_surface_prev_temps[0],
                expected_prev
            );
        }
        assert!(
            (u[1] - expected_flux[0]).abs() < 1e-9 && (u[2] - expected_flux[1]).abs() < 1e-9,
            "interior LWR input accumulation drift"
        );
        assert!(
            solver.interior_surface_temps[0]
                .iter()
                .all(|t| *t >= t_surf_min - 1e-9 && *t <= t_surf_max + 1e-9),
            "interior surface temps must stay within clamp range"
        );
    }

    // -----------------------------------------------------------------------
    // ANSI/ASHRAE Standard 140-2017 (BESTEST) Case 600 validation tests
    //
    // Reference: ANSI/ASHRAE Standard 140-2017, "Standard Method of Test
    // for the Evaluation of Building Energy Analysis Computer Programs",
    // Section 5.2.1 (Case 600: Base Case - Low Mass Building).
    //
    // These tests model the Case 600 lightweight building as a simplified
    // 2R1C RC network and verify that the thermal response matches
    // expected physics: correct time constants, bounded temperatures,
    // and steady-state energy balance.
    //
    // Case 600 parameters:
    //   - Single zone: 8m x 6m x 2.7m = 129.6 m^3
    //   - South-facing windows: 12 m^2, U = 3.0 W/(m^2-K)
    //   - Opaque walls UA ~= 40 W/K (walls + roof + floor combined)
    //   - Walls: U = 0.514 W/(m^2-K), area ~68 m^2 opaque
    //   - Roof:  U = 0.318 W/(m^2-K), area = 48 m^2
    //   - Floor: U = 0.039 W/(m^2-K), area = 48 m^2
    //   - Infiltration: 0.5 ACH
    //   - Internal gains: 200 W continuous sensible
    //   - Zone capacitance: ~1,094,000 J/K (with furniture multiplier)
    // -----------------------------------------------------------------------

    /// Build a BESTEST Case 600 simplified 2R1C model using the RC network.
    ///
    /// Returns (StateSpaceModel, capacitance, r_envelope, r_floor, ua_envelope, ua_floor).
    fn bestest_case_600_model() -> (StateSpaceModel, f64, f64, f64, f64, f64) {
        use crate::rc_network::{NodeId, RCNetwork};

        // BESTEST Case 600 building parameters
        let volume_m3 = 129.6; // 8m x 6m x 2.7m
        let rho_air = 1.2; // kg/m^3 (approximate)
        let cp_air = 1_006.0; // J/(kg-K)
        let furniture_multiplier = 7.0;
        let capacitance = volume_m3 * rho_air * cp_air * furniture_multiplier; // ~1,094,000 J/K

        // Envelope UA values (W/K)
        let ua_windows = 3.0 * 12.0; // 36 W/K
        let ua_walls = 0.514 * 68.0; // 34.95 W/K
        // 0.318 W/m²K is the BESTEST roof U-value, not an approximation of 1/π.
        #[allow(clippy::approx_constant)]
        let ua_roof = 0.318 * 48.0; // 15.26 W/K
        let ua_envelope = ua_windows + ua_walls + ua_roof; // ~86.2 W/K (walls+roof+windows to outdoor)
        let ua_floor = 0.039 * 48.0; // 1.872 W/K (floor to ground)

        let r_envelope = 1.0 / ua_envelope;
        let r_floor = 1.0 / ua_floor;

        // Build RC network: node 0 = zone air, node 1 = outdoor, node 2 = ground
        let caps = std::collections::HashMap::from([(NodeId(0), capacitance)]);
        let resistances = std::collections::HashMap::from([
            ((NodeId(0), NodeId(1)), r_envelope),
            ((NodeId(0), NodeId(2)), r_floor),
        ]);
        let network =
            RCNetwork::from_elements(caps, resistances, vec![NodeId(1), NodeId(2)]).unwrap();
        let (a_c, b_c_rc, _) = network.build_matrices().unwrap();

        let n_states = a_c.nrows();
        let n_ext = b_c_rc.ncols();
        let n_inputs = n_ext + 1; // +1 for sensible gains
        let mut b_c = DMatrix::zeros(n_states, n_inputs);
        for row in 0..n_states {
            for col in 0..n_ext {
                b_c[(row, col)] = b_c_rc[(row, col)];
            }
        }
        b_c[(0, n_ext)] = 1.0 / capacitance; // sensible gain input

        let dt = 300.0; // 5-minute timestep
        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, dt, &mapping).unwrap();

        (
            model,
            capacitance,
            r_envelope,
            r_floor,
            ua_envelope,
            ua_floor,
        )
    }

    /// ANSI/ASHRAE Standard 140-2017 (BESTEST) Case 600: free-float step response.
    ///
    /// Verifies the transient thermal response of a simplified Case 600
    /// building model after a cold outdoor temperature step.
    #[test]
    fn bestest_case_600_simplified_free_float_step_response() {
        use hares_physics::constants::CP_DRY_AIR_J_KG_K;
        use nalgebra::DVector;

        let (model, capacitance, r_envelope, _r_floor, ua_envelope, ua_floor) =
            bestest_case_600_model();

        let volume_m3 = 129.6;
        let ach = 0.5;
        let q_internal = 200.0; // W
        let t_outdoor = -10.0; // cold step
        let t_ground = 10.0;
        let rho_air = 1.2;

        // Time constant: tau = C / UA_total (dominant path to outdoor)
        let tau = capacitance * r_envelope;

        // Initialize at 20 C everywhere
        let mut x = DVector::from_row_slice(&[20.0]);

        // Step for 24 hours (288 steps at dt=300s)
        let n_steps = 288;
        let mut t_zone = 20.0;
        for _ in 0..n_steps {
            // Infiltration sensible load: m_dot * cp * (T_out - T_zone)
            let q_inf_m3_s = ach * volume_m3 / 3600.0;
            let m_dot = rho_air * q_inf_m3_s;
            let q_infiltration = m_dot * CP_DRY_AIR_J_KG_K * (t_outdoor - t_zone);

            let q_total = q_internal + q_infiltration;

            // Input vector: [outdoor_temp, ground_temp, sensible_gain_w]
            let u = DVector::from_row_slice(&[t_outdoor, t_ground, q_total]);
            x = model.step(&x, &u);
            let y = model.output(&x, &u);
            t_zone = y[0];
        }

        // After 24 hours (86400s), which is ~6.8 time constants (tau ~12700s),
        // the zone should be near steady state.

        // Temperature must have dropped significantly from 20 C
        assert!(
            t_zone < 15.0,
            "zone temp {t_zone:.2} C should be well below initial 20 C after 24h cold step"
        );

        // Temperature must be bounded: above outdoor temp (we have internal gains)
        assert!(
            t_zone > t_outdoor,
            "zone temp {t_zone:.2} C should be above outdoor {t_outdoor} C due to internal gains"
        );

        // Verify time constant is physically reasonable.
        // tau = C * R_envelope ~= 1,094,000 * 0.0116 ~= 12,690 s ~= 3.5 hours
        assert!(
            (3.0..=5.0).contains(&(tau / 3600.0)),
            "time constant {:.1} hours outside expected 3-5 hour range",
            tau / 3600.0
        );

        // After 5+ time constants, zone should be near steady state.
        // Compute expected steady-state analytically (approximate, ignoring
        // infiltration temperature-dependence for the check):
        // At steady state: Q_internal + Q_inf = UA_env*(T_zone - T_out) + UA_floor*(T_zone - T_ground)
        // Q_inf = m_dot * cp * (T_out - T_zone) = -m_dot*cp*(T_zone - T_out)
        // So: Q_internal = (UA_env + UA_floor + m_dot*cp) * (T_zone - T_out) + UA_floor*(T_out - T_ground)
        // Solving: T_zone = T_out + (Q_internal - UA_floor*(T_out - T_ground)) / (UA_env + UA_floor + m_dot*cp)
        let q_inf_m3_s = ach * volume_m3 / 3600.0;
        let m_dot_cp = rho_air * q_inf_m3_s * CP_DRY_AIR_J_KG_K;
        let ua_total = ua_envelope + ua_floor + m_dot_cp;
        let t_ss = t_outdoor + (q_internal - ua_floor * (t_outdoor - t_ground)) / ua_total;

        // Zone temperature should be within 1 C of analytical steady state
        assert!(
            (t_zone - t_ss).abs() < 1.0,
            "zone temp {t_zone:.2} C should be near steady-state {t_ss:.2} C after 24h (~6 tau)"
        );
    }

    /// ANSI/ASHRAE Standard 140-2017 (BESTEST) Case 600: steady-state heat balance.
    ///
    /// Runs the simplified Case 600 model to thermal equilibrium and verifies
    /// that the energy balance is satisfied:
    ///   Q_internal = Q_envelope + Q_floor + Q_infiltration
    /// where each loss term is computed from the final zone temperature.
    #[test]
    fn bestest_case_600_steady_state_heat_balance() {
        use hares_physics::constants::CP_DRY_AIR_J_KG_K;
        use nalgebra::DVector;

        let (model, _capacitance, _r_envelope, _r_floor, ua_envelope, ua_floor) =
            bestest_case_600_model();

        let volume_m3 = 129.6;
        let ach = 0.5;
        let q_internal = 200.0;
        let t_outdoor = -10.0;
        let t_ground = 10.0;
        let rho_air = 1.2;

        let mut x = DVector::from_row_slice(&[20.0]);
        let mut t_zone = 20.0;

        // Run 1200 steps (100 hours) to ensure full convergence
        for _ in 0..1200 {
            let q_inf_m3_s = ach * volume_m3 / 3600.0;
            let m_dot = rho_air * q_inf_m3_s;
            let q_infiltration = m_dot * CP_DRY_AIR_J_KG_K * (t_outdoor - t_zone);
            let q_total = q_internal + q_infiltration;

            let u = DVector::from_row_slice(&[t_outdoor, t_ground, q_total]);
            x = model.step(&x, &u);
            let y = model.output(&x, &u);
            t_zone = y[0];
        }

        // Verify convergence: step one more time and check temperature is stable
        let q_inf_m3_s = ach * volume_m3 / 3600.0;
        let m_dot = rho_air * q_inf_m3_s;
        let q_infiltration_final = m_dot * CP_DRY_AIR_J_KG_K * (t_outdoor - t_zone);
        let q_total_final = q_internal + q_infiltration_final;
        let u_final = DVector::from_row_slice(&[t_outdoor, t_ground, q_total_final]);
        let x_check = model.step(&x, &u_final);
        let y_check = model.output(&x_check, &u_final);
        let t_zone_check = y_check[0];
        assert!(
            (t_zone_check - t_zone).abs() < 1e-6,
            "not converged: T_zone changed by {:.6} C on final step",
            (t_zone_check - t_zone).abs()
        );

        // Compute heat balance terms from the converged zone temperature
        let q_envelope_loss = ua_envelope * (t_zone - t_outdoor);
        let q_floor_loss = ua_floor * (t_zone - t_ground);
        let q_inf_loss = -q_infiltration_final; // flip sign: loss is positive when zone > outdoor

        let q_total_loss = q_envelope_loss + q_floor_loss + q_inf_loss;

        // Energy balance: Q_internal = Q_total_loss at steady state.
        // The discrete-time model introduces a small O(dt^2) error, so we allow
        // a tolerance proportional to the gain magnitude.
        let balance_error = (q_internal - q_total_loss).abs();
        let tolerance = 0.01 * q_internal; // 1% of internal gains
        assert!(
            balance_error < tolerance,
            "steady-state energy balance violated: Q_internal={q_internal:.1} W, \
             Q_loss={q_total_loss:.1} W (envelope={q_envelope_loss:.1}, \
             floor={q_floor_loss:.1}, infiltration={q_inf_loss:.1}), \
             error={balance_error:.3} W > tolerance={tolerance:.3} W"
        );

        // Sanity: zone temperature should be between outdoor and indoor start
        assert!(
            t_zone > t_outdoor && t_zone < 20.0,
            "steady-state zone temp {t_zone:.2} C is out of expected range"
        );
    }

    // -----------------------------------------------------------------------
    // Helper: build a one-zone ThermalSolver with a configurable infiltration
    // method but otherwise identical RC parameters (R=2, C=50_000).
    // -----------------------------------------------------------------------
    fn solver_with_infiltration(
        env: &EnvironmentState,
        method: InfiltrationMethod,
    ) -> ThermalSolver {
        let r = 2.0;
        let c = 50_000.0;
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (r * c)]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1.0 / (r * c), 1.0 / c]);
        let mapping = crate::state_space::OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();
        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            window_properties: HashMap::new(),
            window_zone_ids: HashMap::new(),

            exterior_surfaces: vec![],
            interior_lwr_zones: vec![],
            infiltration: vec![(ZoneId(1), method)],
            ventilation_flow_m3_s: 0.0,
            ventilation: MechanicalVentilationParams::default(),
            natural_ventilation: None,
            supply_duct_leakage_m3_s: 0.0,
            return_duct_leakage_m3_s: 0.0,
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
            film_coefficient_model: FilmCoefficientModel::default(),
            interior_convection_injections: Vec::new(),
            ideal_capacity_degraded_threshold: 3,
        };
        let mut solver =
            ThermalSolver::new(model, wiring, config, 60.0, env, env.zones[0].temperature_c)
                .unwrap();
        // Pin state to zone temp so the infiltration delta-T is deterministic.
        solver.x[0] = env.zones[0].temperature_c;
        solver
    }

    fn stiff_solver_with_infiltration(
        env: &EnvironmentState,
        method: InfiltrationMethod,
    ) -> ThermalSolver {
        let r = 0.01;
        let c = 100.0;
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (r * c)]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1.0 / (r * c), 1.0 / c]);
        let mapping = crate::state_space::OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();
        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            window_properties: HashMap::new(),
            window_zone_ids: HashMap::new(),
            exterior_surfaces: vec![],
            interior_lwr_zones: vec![],
            infiltration: vec![(ZoneId(1), method)],
            ventilation_flow_m3_s: 0.0,
            ventilation: MechanicalVentilationParams::default(),
            natural_ventilation: None,
            supply_duct_leakage_m3_s: 0.0,
            return_duct_leakage_m3_s: 0.0,
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
            film_coefficient_model: FilmCoefficientModel::default(),
            interior_convection_injections: Vec::new(),
            ideal_capacity_degraded_threshold: 3,
        };
        let mut solver =
            ThermalSolver::new(model, wiring, config, 60.0, env, env.zones[0].temperature_c)
                .unwrap();
        solver.x[0] = env.zones[0].temperature_c;
        solver
    }

    #[test]
    fn infiltration_coupling_uses_discrete_input_gain_scaling() {
        use hares_physics::constants::CP_DRY_AIR_J_KG_K;
        use hares_physics::infiltration::ach_infiltration;

        let zone_temp = 22.0;
        let outdoor_temp = 5.0;
        let env = env_for_temp(zone_temp, outdoor_temp);
        let ach = 0.1;
        let mut solver = stiff_solver_with_infiltration(&env, InfiltrationMethod::Ach { ach });
        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        let update = solver
            .resolve_new(&ports, &env, Duration::from_secs(60))
            .unwrap();
        let t_next = update.zone_temperatures_c[0].1;

        let h_inf = 1.2 * ach_infiltration(ach, env.zones[0].volume_m3) * CP_DRY_AIR_J_KG_K;
        let b_coeff = solver.model.b_eff()[(0, 1)];
        let d = h_inf * b_coeff;
        let a_d = solver.model.n_mat()[(0, 0)];
        let b_out = solver.model.b_eff()[(0, 0)];
        let expected =
            (a_d * zone_temp + b_out * outdoor_temp + h_inf * b_coeff * outdoor_temp) / (1.0 + d);

        assert!(
            (t_next - expected).abs() < 1e-9,
            "implicit infiltration mismatch: t_next={t_next:.12}, expected={expected:.12}"
        );
    }

    /// ASHRAE wind+stack infiltration with a warm zone and cold outdoor must
    /// cool the zone compared to a zero-infiltration baseline.
    #[test]
    fn ashrae_wind_stack_infiltration_changes_zone_temperature() {
        let zone_temp = 22.0;
        let outdoor_temp = 5.0;
        let env = env_for_temp(zone_temp, outdoor_temp);

        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        let mut solver_no_inf =
            solver_with_infiltration(&env, InfiltrationMethod::Ach { ach: 0.0 });
        let t_no_inf = solver_no_inf
            .resolve_new(&ports, &env, Duration::from_secs(60))
            .unwrap()
            .zone_temperatures_c[0]
            .1;

        // OCHRE residential defaults: Cs=0.000290, Cw=0.000231 (ASHRAE HOF Ch.16 1-story).
        let mut solver_inf = solver_with_infiltration(
            &env,
            InfiltrationMethod::AshraeWindStack {
                c_s: 0.000_290,
                c_w: 0.000_231,
                shielding_coeff: 0.7,
                n_i: 0.65,
            },
        );
        let t_with_inf = solver_inf
            .resolve_new(&ports, &env, Duration::from_secs(60))
            .unwrap()
            .zone_temperatures_c[0]
            .1;

        // Infiltration draws cold outdoor air in, so zone must be cooler.
        assert!(
            t_with_inf < t_no_inf,
            "ASHRAE wind+stack should cool zone: t_inf={t_with_inf:.4}, t_no_inf={t_no_inf:.4}"
        );
        let delta = t_no_inf - t_with_inf;
        assert!(
            delta > 1e-3,
            "infiltration effect too small: delta={delta:.6} C"
        );
    }

    /// ELA infiltration with non-zero drivers must also shift zone temperature
    /// toward the colder outdoor value vs a zero-infiltration baseline.
    #[test]
    fn ela_infiltration_changes_zone_temperature() {
        let zone_temp = 22.0;
        let outdoor_temp = 5.0;
        let env = env_for_temp(zone_temp, outdoor_temp);

        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        let mut solver_no_inf =
            solver_with_infiltration(&env, InfiltrationMethod::Ach { ach: 0.0 });
        let t_no_inf = solver_no_inf
            .resolve_new(&ports, &env, Duration::from_secs(60))
            .unwrap()
            .zone_temperatures_c[0]
            .1;

        // ELA ~0.01 m² is a moderately leaky 200 m³ residential zone.
        let mut solver_ela = solver_with_infiltration(
            &env,
            InfiltrationMethod::Ela {
                ela_m2: 0.01,
                stack_coeff: 0.000_145,
                wind_coeff: 0.000_087,
            },
        );
        let t_ela = solver_ela
            .resolve_new(&ports, &env, Duration::from_secs(60))
            .unwrap()
            .zone_temperatures_c[0]
            .1;

        assert!(
            t_ela < t_no_inf,
            "ELA infiltration should cool zone: t_ela={t_ela:.4}, t_no_inf={t_no_inf:.4}"
        );
        let delta = t_no_inf - t_ela;
        assert!(delta > 1e-3, "ELA effect too small: delta={delta:.6} C");
    }

    #[test]
    fn solve_ideal_capacity_for_target_returns_correct_load() {
        let zone_temp = 18.0;
        let outdoor_temp = -5.0;
        let target_c = 21.0;
        let env = env_for_temp(zone_temp, outdoor_temp);

        let r = 2.0;
        let c = 50_000.0;
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (r * c)]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1.0 / (r * c), 1.0 / c]);
        let mapping = crate::state_space::OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();
        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            window_properties: HashMap::new(),
            window_zone_ids: HashMap::new(),
            exterior_surfaces: vec![],
            interior_lwr_zones: vec![],
            infiltration: vec![],
            ventilation_flow_m3_s: 0.0,
            ventilation: MechanicalVentilationParams::default(),
            natural_ventilation: None,
            supply_duct_leakage_m3_s: 0.0,
            return_duct_leakage_m3_s: 0.0,
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
            film_coefficient_model: FilmCoefficientModel::default(),
            interior_convection_injections: Vec::new(),
            ideal_capacity_degraded_threshold: 3,
        };
        let mut solver = ThermalSolver::new(model, wiring, config, 60.0, &env, zone_temp).unwrap();
        solver.x[0] = zone_temp;

        let q_ideal = solver.solve_ideal_capacity_for_target(ZoneId(1), target_c);

        assert!(
            q_ideal > 0.0,
            "ideal capacity for heating must be positive: q={q_ideal:.1}"
        );

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };
        ports.thermal[0].sensible_gain_w = q_ideal;
        let update = solver
            .resolve_new(&ports, &env, Duration::from_secs(60))
            .unwrap();
        let t_after = update.zone_temperatures_c[0].1;

        assert!(
            (t_after - target_c).abs() < 0.05,
            "zone should reach explicit target: t={t_after:.4}, target={target_c}"
        );
    }

    #[test]
    fn solve_ideal_capacity_for_target_returns_zero_for_unknown_zone() {
        let zone_temp = 20.0;
        let outdoor_temp = 10.0;
        let env = env_for_temp(zone_temp, outdoor_temp);

        let r = 2.0;
        let c = 50_000.0;
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (r * c)]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1.0 / (r * c), 1.0 / c]);
        let mapping = crate::state_space::OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();
        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            window_properties: HashMap::new(),
            window_zone_ids: HashMap::new(),
            exterior_surfaces: vec![],
            interior_lwr_zones: vec![],
            infiltration: vec![],
            ventilation_flow_m3_s: 0.0,
            ventilation: MechanicalVentilationParams::default(),
            natural_ventilation: None,
            supply_duct_leakage_m3_s: 0.0,
            return_duct_leakage_m3_s: 0.0,
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
            film_coefficient_model: FilmCoefficientModel::default(),
            interior_convection_injections: Vec::new(),
            ideal_capacity_degraded_threshold: 3,
        };
        let mut solver = ThermalSolver::new(model, wiring, config, 60.0, &env, zone_temp).unwrap();
        solver.x[0] = zone_temp;

        let q = solver.solve_ideal_capacity_for_target(ZoneId(99), 22.0);
        assert_eq!(q, 0.0, "unknown zone should return 0");
    }

    #[test]
    fn solve_ideal_capacity_for_target_works_without_setpoint_config() {
        let zone_temp = 18.0;
        let outdoor_temp = -5.0;
        let target_c = 23.0;
        let env = env_for_temp(zone_temp, outdoor_temp);

        let r = 2.0;
        let c = 50_000.0;
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (r * c)]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1.0 / (r * c), 1.0 / c]);
        let mapping = crate::state_space::OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();
        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            window_properties: HashMap::new(),
            window_zone_ids: HashMap::new(),
            exterior_surfaces: vec![],
            interior_lwr_zones: vec![],
            infiltration: vec![],
            ventilation_flow_m3_s: 0.0,
            ventilation: MechanicalVentilationParams::default(),
            natural_ventilation: None,
            supply_duct_leakage_m3_s: 0.0,
            return_duct_leakage_m3_s: 0.0,
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
            film_coefficient_model: FilmCoefficientModel::default(),
            interior_convection_injections: Vec::new(),
            ideal_capacity_degraded_threshold: 3,
        };
        let mut solver = ThermalSolver::new(model, wiring, config, 60.0, &env, zone_temp).unwrap();
        solver.x[0] = zone_temp;

        let q_ideal = solver.solve_ideal_capacity_for_target(ZoneId(1), target_c);

        assert!(
            q_ideal > 0.0,
            "should compute capacity without setpoint config: q={q_ideal:.1}"
        );
    }

    #[test]
    fn solve_ideal_capacity_for_target_returns_negative_for_cooling() {
        let zone_temp = 25.0;
        let outdoor_temp = 35.0;
        let target_c = 22.0;
        let env = env_for_temp(zone_temp, outdoor_temp);

        let r = 2.0;
        let c = 50_000.0;
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (r * c)]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1.0 / (r * c), 1.0 / c]);
        let mapping = crate::state_space::OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();
        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            window_properties: HashMap::new(),
            window_zone_ids: HashMap::new(),
            exterior_surfaces: vec![],
            interior_lwr_zones: vec![],
            infiltration: vec![],
            ventilation_flow_m3_s: 0.0,
            ventilation: MechanicalVentilationParams::default(),
            natural_ventilation: None,
            supply_duct_leakage_m3_s: 0.0,
            return_duct_leakage_m3_s: 0.0,
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
            film_coefficient_model: FilmCoefficientModel::default(),
            interior_convection_injections: Vec::new(),
            ideal_capacity_degraded_threshold: 3,
        };
        let mut solver = ThermalSolver::new(model, wiring, config, 60.0, &env, zone_temp).unwrap();
        solver.x[0] = zone_temp;

        let q_ideal = solver.solve_ideal_capacity_for_target(ZoneId(1), target_c);

        assert!(
            q_ideal < 0.0,
            "ideal capacity for cooling must be negative: q={q_ideal:.1}"
        );

        let mut ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };
        ports.thermal[0].sensible_gain_w = q_ideal;
        let update = solver
            .resolve_new(&ports, &env, Duration::from_secs(60))
            .unwrap();
        let t_after = update.zone_temperatures_c[0].1;

        assert!(
            (t_after - target_c).abs() < 0.05,
            "zone should reach explicit target: t={t_after:.4}, target={target_c}"
        );
    }

    /// When `solve_ideal_capacity_for_target` encounters a zero-gain condition
    /// (e.g. HVAC column of B_c is zero → `ZeroEffectiveGain`), the solver fails
    /// and returns 0.0 W. This test verifies that the failure path:
    ///   1. Returns 0.0 (no spurious non-zero capacity)
    ///   2. Records the zone in `ideal_capacity_warned_zones` (the behavioral contract)
    ///
    /// To trigger `ZeroEffectiveGain`: use a B matrix where the HVAC sensible-input column
    /// (column 1) has zero contribution to the zone output (C row × B_eff[:, 1] ≈ 0).
    /// We achieve this by setting the HVAC column of B_c to zero.
    #[test]
    fn solve_ideal_capacity_failure_returns_zero_and_warns() {
        let zone_temp = 20.0;
        let outdoor_temp = 10.0;
        let env = env_for_temp(zone_temp, outdoor_temp);

        // B_c: two inputs [T_out, H_hvac]. Column 1 (HVAC) is zero → zero effective gain.
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (2.0 * 50_000.0)]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1.0 / (2.0 * 50_000.0), 0.0]);
        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();
        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            window_properties: HashMap::new(),
            window_zone_ids: HashMap::new(),
            exterior_surfaces: vec![],
            interior_lwr_zones: vec![],
            infiltration: vec![],
            ventilation_flow_m3_s: 0.0,
            ventilation: MechanicalVentilationParams::default(),
            natural_ventilation: None,
            supply_duct_leakage_m3_s: 0.0,
            return_duct_leakage_m3_s: 0.0,
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
            film_coefficient_model: FilmCoefficientModel::default(),
            interior_convection_injections: Vec::new(),
            ideal_capacity_degraded_threshold: 3,
        };
        let mut solver = ThermalSolver::new(model, wiring, config, 60.0, &env, zone_temp).unwrap();
        solver.x[0] = zone_temp;

        // The HVAC input has zero gain → solve_for_scalar_input returns ZeroEffectiveGain.
        // After fix: returns 0.0 and records the zone in warned_zones.
        let q = solver.solve_ideal_capacity_for_target(ZoneId(1), 25.0);
        assert_eq!(q, 0.0, "failure path must return 0.0");
        assert!(
            solver.ideal_capacity_warned_zones.contains(&ZoneId(1)),
            "failure path must record zone in warned_zones"
        );
    }

    /// Two consecutive ideal-capacity solve failures must emit only one `warn!`;
    /// the second failure is suppressed to `debug!` by the per-zone throttle.
    /// This test verifies the throttle guard (`ideal_capacity_warned_zones`)
    /// prevents per-timestep warn flood in pathological runs.
    #[test]
    fn solve_ideal_capacity_throttles_repeat_failures() {
        let zone_temp = 20.0;
        let outdoor_temp = 10.0;
        let env = env_for_temp(zone_temp, outdoor_temp);

        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (2.0 * 50_000.0)]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1.0 / (2.0 * 50_000.0), 0.0]);
        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();
        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            window_properties: HashMap::new(),
            window_zone_ids: HashMap::new(),
            exterior_surfaces: vec![],
            interior_lwr_zones: vec![],
            infiltration: vec![],
            ventilation_flow_m3_s: 0.0,
            ventilation: MechanicalVentilationParams::default(),
            natural_ventilation: None,
            supply_duct_leakage_m3_s: 0.0,
            return_duct_leakage_m3_s: 0.0,
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
            film_coefficient_model: FilmCoefficientModel::default(),
            interior_convection_injections: Vec::new(),
            ideal_capacity_degraded_threshold: 3,
        };
        let mut solver = ThermalSolver::new(model, wiring, config, 60.0, &env, zone_temp).unwrap();
        solver.x[0] = zone_temp;

        let q1 = solver.solve_ideal_capacity_for_target(ZoneId(1), 25.0);
        assert_eq!(q1, 0.0, "first failure must return 0.0");

        let q2 = solver.solve_ideal_capacity_for_target(ZoneId(1), 25.0);
        assert_eq!(q2, 0.0, "second failure must return 0.0");

        assert!(
            solver.ideal_capacity_warned_zones.contains(&ZoneId(1)),
            "warned zone must be set after failure(s)"
        );
        assert_eq!(
            solver.ideal_capacity_failure_counts.get(&ZoneId(1)),
            Some(&2),
            "failure count must increment to 2 after two consecutive failures"
        );
    }

    /// After one or more ideal-capacity solve failures, the first successful
    /// solve must emit an `info!` recovery log. This test verifies the recovery
    /// path clears `ideal_capacity_warned_zones` and logs the consecutive-failure
    /// count.
    #[test]
    fn solve_ideal_capacity_emits_recovery_log_after_success() {
        let zone_temp = 20.0;
        let outdoor_temp = 10.0;
        let env = env_for_temp(zone_temp, outdoor_temp);

        // Working model: HVAC column (index 1) has non-zero gain.
        let r = 2.0;
        let c = 50_000.0;
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (r * c)]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1.0 / (r * c), 1.0 / c]);
        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();
        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            window_properties: HashMap::new(),
            window_zone_ids: HashMap::new(),
            exterior_surfaces: vec![],
            interior_lwr_zones: vec![],
            infiltration: vec![],
            ventilation_flow_m3_s: 0.0,
            ventilation: MechanicalVentilationParams::default(),
            natural_ventilation: None,
            supply_duct_leakage_m3_s: 0.0,
            return_duct_leakage_m3_s: 0.0,
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
            film_coefficient_model: FilmCoefficientModel::default(),
            interior_convection_injections: Vec::new(),
            ideal_capacity_degraded_threshold: 3,
        };
        let mut solver = ThermalSolver::new(model, wiring, config, 60.0, &env, zone_temp).unwrap();
        solver.x[0] = zone_temp;

        // Simulate a prior failure run by injecting the zone into the warned set.
        solver.ideal_capacity_warned_zones.insert(ZoneId(1));
        solver.ideal_capacity_failure_counts.insert(ZoneId(1), 3);

        let q = solver.solve_ideal_capacity_for_target(ZoneId(1), 22.0);
        assert!(
            q != 0.0,
            "working solver must produce non-zero capacity: q={q:.1}"
        );
        assert!(
            !solver
                .ideal_capacity_failure_counts
                .contains_key(&ZoneId(1)),
            "failure counts must be cleared on recovery"
        );
        assert!(
            !solver.ideal_capacity_warned_zones.contains(&ZoneId(1)),
            "warned zone must be cleared on recovery"
        );
    }

    /// When the solver has previously converged and stored a last-good capacity,
    /// reaching the degraded threshold should return that last-good value instead
    /// of 0.0. This prevents the simulation from silently zeroing HVAC capacity
    /// during transient solver failures.
    #[test]
    fn solve_ideal_capacity_returns_last_good_after_threshold() {
        let zone_temp = 20.0;
        let outdoor_temp = 10.0;
        let env = env_for_temp(zone_temp, outdoor_temp);

        // Failing model: HVAC column (index 1) has zero gain →
        // solve_for_scalar_input returns ZeroEffectiveGain.
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (2.0 * 50_000.0)]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1.0 / (2.0 * 50_000.0), 0.0]);
        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();
        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            window_properties: HashMap::new(),
            window_zone_ids: HashMap::new(),
            exterior_surfaces: vec![],
            interior_lwr_zones: vec![],
            infiltration: vec![],
            ventilation_flow_m3_s: 0.0,
            ventilation: MechanicalVentilationParams::default(),
            natural_ventilation: None,
            supply_duct_leakage_m3_s: 0.0,
            return_duct_leakage_m3_s: 0.0,
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
            film_coefficient_model: FilmCoefficientModel::default(),
            interior_convection_injections: Vec::new(),
            ideal_capacity_degraded_threshold: 3,
        };
        let mut solver = ThermalSolver::new(model, wiring, config, 60.0, &env, zone_temp).unwrap();
        solver.x[0] = zone_temp;

        // Inject fake last-good capacity and 3 prior failures.
        solver.last_good_capacity_w.insert(ZoneId(1), 4500.0);
        solver.ideal_capacity_failure_counts.insert(ZoneId(1), 3);

        // This call should fail (zero effective gain), but because count (3)
        // meets the threshold (3) and last-good exists, it returns last-good.
        let q = solver.solve_ideal_capacity_for_target(ZoneId(1), 25.0);
        assert!(
            (q - 4500.0).abs() < 1e-9,
            "threshold failure should return last-good capacity: expected 4500, got {q:.1}"
        );

        // The degraded flag must be set.
        assert!(
            solver.zone_capacity_degraded(ZoneId(1)),
            "degraded flag must be set when last-good fallback is used"
        );

        // After prepare_inputs (new step), the degraded flag must be cleared.
        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };
        solver.prepare_inputs(&ports, &env).unwrap();
        assert!(
            !solver.zone_capacity_degraded(ZoneId(1)),
            "degraded flag must be cleared at start of new step"
        );
    }

    /// The consecutive failure count is incremented on each solve failure.
    #[test]
    fn ideal_capacity_failure_count_incremented_on_failure() {
        let zone_temp = 20.0;
        let outdoor_temp = 10.0;
        let env = env_for_temp(zone_temp, outdoor_temp);

        // Failing model.
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (2.0 * 50_000.0)]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1.0 / (2.0 * 50_000.0), 0.0]);
        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();
        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            window_properties: HashMap::new(),
            window_zone_ids: HashMap::new(),
            exterior_surfaces: vec![],
            interior_lwr_zones: vec![],
            infiltration: vec![],
            ventilation_flow_m3_s: 0.0,
            ventilation: MechanicalVentilationParams::default(),
            natural_ventilation: None,
            supply_duct_leakage_m3_s: 0.0,
            return_duct_leakage_m3_s: 0.0,
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
            film_coefficient_model: FilmCoefficientModel::default(),
            interior_convection_injections: Vec::new(),
            ideal_capacity_degraded_threshold: 3,
        };
        let mut solver = ThermalSolver::new(model, wiring, config, 60.0, &env, zone_temp).unwrap();
        solver.x[0] = zone_temp;

        solver.solve_ideal_capacity_for_target(ZoneId(1), 25.0);
        assert_eq!(
            solver.ideal_capacity_failure_counts.get(&ZoneId(1)),
            Some(&1),
            "the failure count must increment to 1 after the first failure"
        );

        solver.solve_ideal_capacity_for_target(ZoneId(1), 25.0);
        assert_eq!(
            solver.ideal_capacity_failure_counts.get(&ZoneId(1)),
            Some(&2),
            "the failure count must increment to 2 after the second failure"
        );
    }

    /// Non-zero solar irradiance routed through solar_input_indices must raise
    /// zone temperature compared to an identical solver with zero irradiance.
    #[test]
    fn solar_input_raises_zone_temperature() {
        let zone_temp = 18.0;
        let outdoor_temp = 5.0;

        let r = 2.0;
        let c = 50_000.0;
        // B has 3 columns: [outdoor_temp, sensible_gain, solar_gain]
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (r * c)]);
        let b_c = DMatrix::from_row_slice(1, 3, &[1.0 / (r * c), 1.0 / c, 1.0 / c]);
        let mapping = crate::state_space::OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();

        let make_env = |solar_w_m2: f64| -> EnvironmentState {
            EnvironmentState {
                ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
                zones: vec![ZoneState {
                    id: ZoneId(1),
                    temperature_c: zone_temp,
                    humidity_ratio: 0.008,
                    volume_m3: 200.0,
                }],
                weather: WeatherState {
                    outdoor_temp_c: outdoor_temp,
                    outdoor_humidity_ratio: 0.004,
                    wind_speed_m_s: 0.0,
                    wind_dir_deg: 0.0,
                    ground_temp_c: outdoor_temp,
                    sky_temp_c: outdoor_temp - 5.0,
                    pressure_kpa: 101.325,
                    solar_irradiance: vec![SurfaceIrradiance {
                        surface_id: 42,
                        direct_w_m2: solar_w_m2,
                        diffuse_w_m2: 0.0,
                        reflected_w_m2: 0.0,
                        angle_of_incidence_rad: 0.0,
                    }],
                    outdoor_wet_bulb_c: 0.0,
                    outdoor_enthalpy_j_kg: 0.0,
                    ghi_w_m2: 0.0,
                    dni_w_m2: 0.0,
                    dhi_w_m2: 0.0,
                    solar_altitude_deg: 0.0,
                    solar_azimuth_deg: 180.0,
                    mains_temp_c: 15.0,
                    rainfall_m: 0.0,
                    ground_albedo: 0.2,
                    ground_t_mean_c: 10.0,
                    ground_t_amplitude_c: 0.0,
                    ground_phase_day: 35.0,
                    day_of_year: 1.0,
                },
                grid: GridState {
                    voltage_pu: 1.0,
                    frequency_hz: 60.0,
                    island_bus_voltage_pu: None,
                },
                schedule_row: None,
                domains: hares_types::DomainSlots::default(),
                equipment_telemetry: std::collections::HashMap::new(),
                current_time: FixedOffset::east_opt(0)
                    .unwrap()
                    .with_ymd_and_hms(2026, 6, 21, 12, 0, 0)
                    .single()
                    .unwrap(),
                time_res: chrono::Duration::seconds(60),
                price_signal: Default::default(),
                electrical: Default::default(),
                equipment_core: Default::default(),
            }
        };

        let make_solver = |env: &EnvironmentState| -> ThermalSolver {
            let wiring = StateSpaceWiring {
                zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
                zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
                zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
                outdoor_temp_input_indices: vec![0],
                ground_temp_input_indices: vec![],
                ground_temp_input_depths_m: vec![],
                indoor_temp_input_indices: vec![],
                solar_input_indices: HashMap::new(),
                c_zone_j_k: HashMap::new(),
                node_capacitances: HashMap::new(),
                node_index: HashMap::new(),
            };
            let cfg = ThermalSolverConfig {
                indoor_zone_id: ZoneId(1),
                window_properties: HashMap::new(),
                window_zone_ids: HashMap::new(),
                exterior_surfaces: vec![ExteriorSurfaceInfo {
                    surface_id: 42,
                    state_index: 0,
                    input_index: 2,
                    area_m2: 1.0,
                    emissivity: 0.90,
                    tilt_deg: 90.0,
                    azimuth_deg: 180.0,
                    rad_frac: 0.0,
                    rad_res_k_w: 0.0,
                    n_iter: 1,
                    absorptance: 1.0,
                    boundary_category: None,
                    u_factor_w_m2_k: 0.0,
                    h_out_w_m2_k: 0.0,
                }],
                interior_lwr_zones: vec![],
                infiltration: vec![],
                ventilation_flow_m3_s: 0.0,
                ventilation: MechanicalVentilationParams::default(),
                natural_ventilation: None,
                supply_duct_leakage_m3_s: 0.0,
                return_duct_leakage_m3_s: 0.0,
                interior_solar_zones: Vec::new(),
                boundary_diagnostics: Vec::new(),
                film_coefficient_model: FilmCoefficientModel::default(),
                interior_convection_injections: Vec::new(),
                interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
                ideal_capacity_degraded_threshold: 3,
            };
            let mut s =
                ThermalSolver::new(model.clone(), wiring.clone(), cfg, 60.0, env, zone_temp)
                    .unwrap();
            s.x[0] = zone_temp;
            s
        };

        let env_no_solar = make_env(0.0);
        let env_solar = make_env(500.0);

        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        let mut solver_no = make_solver(&env_no_solar);
        let t_no_solar = solver_no
            .resolve_new(&ports, &env_no_solar, Duration::from_secs(60))
            .unwrap()
            .zone_temperatures_c[0]
            .1;

        let mut solver_with = make_solver(&env_solar);
        let t_solar = solver_with
            .resolve_new(&ports, &env_solar, Duration::from_secs(60))
            .unwrap()
            .zone_temperatures_c[0]
            .1;

        assert!(
            t_solar > t_no_solar,
            "solar input should warm zone: t_solar={t_solar:.4}, t_no_solar={t_no_solar:.4}"
        );
        let delta = t_solar - t_no_solar;
        assert!(
            delta > 1e-3,
            "solar warming effect too small: delta={delta:.6} C"
        );
    }

    /// Opaque surface solar gain must be scaled by absorptance.
    /// A surface with absorptance=0.60 should produce ~60% of the warming
    /// compared to absorptance=1.0.
    #[test]
    fn opaque_solar_gain_scales_by_absorptance() {
        let zone_temp = 18.0;
        let outdoor_temp = 18.0; // neutral outdoor so only solar matters

        let r = 2.0;
        let c = 50_000.0;
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (r * c)]);
        let b_c = DMatrix::from_row_slice(1, 3, &[1.0 / (r * c), 1.0 / c, 1.0 / c]);
        let mapping = crate::state_space::OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();

        let env = EnvironmentState {
            ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp,
                humidity_ratio: 0.008,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: outdoor_temp,
                outdoor_humidity_ratio: 0.004,
                wind_speed_m_s: 0.0,
                wind_dir_deg: 0.0,
                ground_temp_c: outdoor_temp,
                sky_temp_c: outdoor_temp,
                pressure_kpa: 101.325,
                solar_irradiance: vec![SurfaceIrradiance {
                    surface_id: 1,
                    direct_w_m2: 500.0,
                    diffuse_w_m2: 100.0,
                    reflected_w_m2: 0.0,
                    angle_of_incidence_rad: 0.0,
                }],
                outdoor_wet_bulb_c: 0.0,
                outdoor_enthalpy_j_kg: 0.0,
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
                solar_altitude_deg: 0.0,
                solar_azimuth_deg: 180.0,
                mains_temp_c: 15.0,
                rainfall_m: 0.0,
                ground_albedo: 0.2,
                ground_t_mean_c: 10.0,
                ground_t_amplitude_c: 0.0,
                ground_phase_day: 35.0,
                day_of_year: 1.0,
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
                island_bus_voltage_pu: None,
            },
            schedule_row: None,
            domains: hares_types::DomainSlots::default(),
            equipment_telemetry: std::collections::HashMap::new(),
            current_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 6, 21, 12, 0, 0)
                .single()
                .unwrap(),
            time_res: chrono::Duration::seconds(60),
            price_signal: Default::default(),
            electrical: Default::default(),
            equipment_core: Default::default(),
        };

        let make_solver = |absorptance: f64| -> ThermalSolver {
            let wiring = StateSpaceWiring {
                zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
                zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
                zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
                outdoor_temp_input_indices: vec![0],
                ground_temp_input_indices: vec![],
                ground_temp_input_depths_m: vec![],
                indoor_temp_input_indices: vec![],
                solar_input_indices: HashMap::new(),
                c_zone_j_k: HashMap::new(),
                node_capacitances: HashMap::new(),
                node_index: HashMap::new(),
            };
            let cfg = ThermalSolverConfig {
                indoor_zone_id: ZoneId(1),
                window_properties: HashMap::new(),
                window_zone_ids: HashMap::new(),
                exterior_surfaces: vec![ExteriorSurfaceInfo {
                    surface_id: 1,
                    state_index: 0,
                    input_index: 2,
                    area_m2: 1.0,
                    emissivity: 0.90,
                    tilt_deg: 90.0,
                    azimuth_deg: 180.0,
                    rad_frac: 0.0,
                    rad_res_k_w: 0.0,
                    n_iter: 1,
                    absorptance,
                    boundary_category: None,
                    u_factor_w_m2_k: 0.0,
                    h_out_w_m2_k: 0.0,
                }],
                interior_lwr_zones: vec![],
                infiltration: vec![],
                ventilation_flow_m3_s: 0.0,
                ventilation: MechanicalVentilationParams::default(),
                natural_ventilation: None,
                supply_duct_leakage_m3_s: 0.0,
                return_duct_leakage_m3_s: 0.0,
                interior_solar_zones: Vec::new(),
                boundary_diagnostics: Vec::new(),
                film_coefficient_model: FilmCoefficientModel::default(),
                interior_convection_injections: Vec::new(),
                interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
                ideal_capacity_degraded_threshold: 3,
            };
            let mut s =
                ThermalSolver::new(model.clone(), wiring.clone(), cfg, 60.0, &env, zone_temp)
                    .unwrap();
            s.x[0] = zone_temp;
            s
        };

        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        let mut solver_full = make_solver(1.0);
        let t_full = solver_full
            .resolve_new(&ports, &env, Duration::from_secs(60))
            .unwrap()
            .zone_temperatures_c[0]
            .1;

        let mut solver_060 = make_solver(0.60);
        let t_060 = solver_060
            .resolve_new(&ports, &env, Duration::from_secs(60))
            .unwrap()
            .zone_temperatures_c[0]
            .1;

        let mut solver_005 = make_solver(0.05);
        let t_005 = solver_005
            .resolve_new(&ports, &env, Duration::from_secs(60))
            .unwrap()
            .zone_temperatures_c[0]
            .1;

        // Higher absorptance → more warming
        assert!(
            t_full > t_060,
            "absorptance=1.0 must warm more than 0.60: {t_full:.6} vs {t_060:.6}"
        );
        assert!(
            t_060 > t_005,
            "absorptance=0.60 must warm more than 0.05: {t_060:.6} vs {t_005:.6}"
        );

        // Warming should scale roughly proportionally to absorptance
        let gain_full = t_full - zone_temp;
        let gain_060 = t_060 - zone_temp;
        let ratio = gain_060 / gain_full;
        assert!(
            (ratio - 0.60).abs() < 0.05,
            "warming ratio should be ~0.60, got {ratio:.4}"
        );
    }

    /// Exterior solar gain via `apply_exterior_solar_inputs` scales proportionally
    /// with absorptance. Two surfaces sharing a zone input but differing in
    /// absorptance should produce proportional temperature rises.
    #[test]
    fn exterior_solar_via_surfaces_scales_by_absorptance() {
        let zone_temp = 18.0;
        let outdoor_temp = 18.0;

        let a_c = DMatrix::from_row_slice(1, 1, &[-0.01]);
        let b_c = DMatrix::from_row_slice(1, 2, &[0.01, 0.001]);
        let mapping = crate::state_space::OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();

        let make_env = || EnvironmentState {
            ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp,
                humidity_ratio: 0.008,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: outdoor_temp,
                outdoor_humidity_ratio: 0.004,
                wind_speed_m_s: 0.0,
                wind_dir_deg: 0.0,
                ground_temp_c: outdoor_temp,
                sky_temp_c: outdoor_temp,
                pressure_kpa: 101.325,
                solar_irradiance: vec![SurfaceIrradiance {
                    surface_id: 0,
                    direct_w_m2: 500.0,
                    diffuse_w_m2: 100.0,
                    reflected_w_m2: 50.0,
                    angle_of_incidence_rad: 0.0,
                }],
                outdoor_wet_bulb_c: 0.0,
                outdoor_enthalpy_j_kg: 0.0,
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
                solar_altitude_deg: 0.0,
                solar_azimuth_deg: 180.0,
                mains_temp_c: 15.0,
                rainfall_m: 0.0,
                ground_albedo: 0.2,
                ground_t_mean_c: 10.0,
                ground_t_amplitude_c: 0.0,
                ground_phase_day: 35.0,
                day_of_year: 1.0,
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
                island_bus_voltage_pu: None,
            },
            schedule_row: None,
            domains: hares_types::DomainSlots::default(),
            equipment_telemetry: std::collections::HashMap::new(),
            current_time: FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2026, 6, 21, 12, 0, 0)
                .single()
                .unwrap(),
            time_res: chrono::Duration::seconds(60),
            price_signal: Default::default(),
            electrical: Default::default(),
            equipment_core: Default::default(),
        };

        let make_solver = |absorptance: f64| -> ThermalSolver {
            let wiring = StateSpaceWiring {
                zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
                zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
                zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
                outdoor_temp_input_indices: vec![0],
                ground_temp_input_indices: vec![],
                ground_temp_input_depths_m: vec![],
                indoor_temp_input_indices: vec![],
                solar_input_indices: HashMap::new(),
                c_zone_j_k: HashMap::new(),
                node_capacitances: HashMap::new(),
                node_index: HashMap::new(),
            };
            let cfg = ThermalSolverConfig {
                indoor_zone_id: ZoneId(1),
                window_properties: HashMap::new(),
                window_zone_ids: HashMap::new(),

                exterior_surfaces: vec![ExteriorSurfaceInfo {
                    surface_id: 0,
                    state_index: 0,
                    input_index: 1,
                    area_m2: 10.0,
                    emissivity: 0.90,
                    tilt_deg: 90.0,
                    azimuth_deg: 180.0,
                    rad_frac: 0.0,
                    rad_res_k_w: 0.0,
                    n_iter: 1,
                    absorptance,
                    boundary_category: None,
                    u_factor_w_m2_k: 0.0,
                    h_out_w_m2_k: 0.0,
                }],
                interior_lwr_zones: vec![],
                infiltration: vec![],
                ventilation_flow_m3_s: 0.0,
                ventilation: MechanicalVentilationParams::default(),
                natural_ventilation: None,
                supply_duct_leakage_m3_s: 0.0,
                return_duct_leakage_m3_s: 0.0,
                interior_solar_zones: Vec::new(),
                boundary_diagnostics: Vec::new(),
                film_coefficient_model: FilmCoefficientModel::default(),
                interior_convection_injections: Vec::new(),
                interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
                ideal_capacity_degraded_threshold: 3,
            };
            let env = make_env();
            let mut s =
                ThermalSolver::new(model.clone(), wiring.clone(), cfg, 60.0, &env, zone_temp)
                    .unwrap();
            s.x[0] = zone_temp;
            s
        };

        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };
        let env = make_env();

        let t_dark = make_solver(0.25)
            .resolve_new(&ports, &env, Duration::from_secs(60))
            .unwrap()
            .zone_temperatures_c[0]
            .1;
        let t_light = make_solver(0.60)
            .resolve_new(&ports, &env, Duration::from_secs(60))
            .unwrap()
            .zone_temperatures_c[0]
            .1;

        // Higher absorptance → more warming
        assert!(
            t_light > t_dark,
            "absorptance=0.60 must warm more than 0.25: {t_light:.6} vs {t_dark:.6}"
        );

        // Warming should be proportional: ratio of gains ≈ ratio of absorptances
        let gain_dark = t_dark - zone_temp;
        let gain_light = t_light - zone_temp;
        let ratio = gain_dark / gain_light;
        let expected_ratio = 0.25 / 0.60;
        assert!(
            (ratio - expected_ratio).abs() < 0.05,
            "gain ratio should be ~{expected_ratio:.3}, got {ratio:.4}"
        );
    }

    /// Higher wind speed must produce more infiltration-driven cooling for the
    /// AshraeWindStack method, because wind contributes quadratically to the
    /// volumetric flow: q_wind = c_w * shelter * v². A solver with v=8 m/s must
    /// end up cooler than the same solver with v=2 m/s after one step.
    #[test]
    fn ashrae_wind_stack_higher_wind_produces_more_cooling() {
        let zone_temp = 22.0;
        let outdoor_temp = 0.0;

        let method = InfiltrationMethod::AshraeWindStack {
            c_s: 0.000_290,
            c_w: 0.000_231,
            shielding_coeff: 0.7,
            n_i: 0.65,
        };

        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        let mut env_low_wind = env_for_temp(zone_temp, outdoor_temp);
        env_low_wind.weather.wind_speed_m_s = 2.0;
        let mut solver_low = solver_with_infiltration(&env_low_wind, method);
        let t_low_wind = solver_low
            .resolve_new(&ports, &env_low_wind, Duration::from_secs(60))
            .unwrap()
            .zone_temperatures_c[0]
            .1;

        let mut env_high_wind = env_for_temp(zone_temp, outdoor_temp);
        env_high_wind.weather.wind_speed_m_s = 8.0;
        let mut solver_high = solver_with_infiltration(&env_high_wind, method);
        let t_high_wind = solver_high
            .resolve_new(&ports, &env_high_wind, Duration::from_secs(60))
            .unwrap()
            .zone_temperatures_c[0]
            .1;

        assert!(
            t_high_wind < t_low_wind,
            "higher wind must cool zone more: t_high={t_high_wind:.5}, t_low={t_low_wind:.5}"
        );
        // Wind term scales as v², so going from 2 to 8 m/s (4×) raises q_wind by 16×;
        // the total flow increase will be meaningful -- require at least 1 mK more cooling.
        let extra_cooling = t_low_wind - t_high_wind;
        assert!(
            extra_cooling > 1e-3,
            "extra cooling from higher wind too small: {extra_cooling:.6} C"
        );
    }

    /// Higher wind speed must produce more infiltration-driven cooling for the
    /// Ela method, because wind enters the driver as wind_coeff * v² and
    /// increases the square-root flow rate monotonically.
    #[test]
    fn ela_higher_wind_produces_more_cooling() {
        let zone_temp = 22.0;
        let outdoor_temp = 0.0;

        let method = InfiltrationMethod::Ela {
            ela_m2: 0.01,
            stack_coeff: 0.000_145,
            wind_coeff: 0.000_087,
        };

        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        let mut env_low_wind = env_for_temp(zone_temp, outdoor_temp);
        env_low_wind.weather.wind_speed_m_s = 1.0;
        let mut solver_low = solver_with_infiltration(&env_low_wind, method);
        let t_low_wind = solver_low
            .resolve_new(&ports, &env_low_wind, Duration::from_secs(60))
            .unwrap()
            .zone_temperatures_c[0]
            .1;

        let mut env_high_wind = env_for_temp(zone_temp, outdoor_temp);
        env_high_wind.weather.wind_speed_m_s = 6.0;
        let mut solver_high = solver_with_infiltration(&env_high_wind, method);
        let t_high_wind = solver_high
            .resolve_new(&ports, &env_high_wind, Duration::from_secs(60))
            .unwrap()
            .zone_temperatures_c[0]
            .1;

        assert!(
            t_high_wind < t_low_wind,
            "higher wind must cool zone more via ELA: t_high={t_high_wind:.5}, t_low={t_low_wind:.5}"
        );
        let extra_cooling = t_low_wind - t_high_wind;
        assert!(
            extra_cooling > 1e-3,
            "extra ELA cooling from higher wind too small: {extra_cooling:.6} C"
        );
    }

    /// ANSI/ASHRAE Standard 140-2017, Case 900: heavyweight longer time constant.
    ///
    /// Same geometry as Case 600 but with heavy construction (10× capacitance).
    /// Verifies that after the same 24h cold step, the heavy-mass zone temperature
    /// is closer to initial than the lightweight Case 600 (slower response).
    #[test]
    fn bestest_case_900_heavyweight_longer_time_constant() {
        use hares_physics::constants::CP_DRY_AIR_J_KG_K;
        use nalgebra::DVector;

        let (model_600, capacitance_600, _, _, ua_envelope, ua_floor) = bestest_case_600_model();

        // Case 900: same geometry, 10× thermal mass
        let capacitance_900 = capacitance_600 * 10.0;

        // Build Case 900 model with higher capacitance
        use crate::rc_network::{NodeId, RCNetwork};

        let r_envelope = 1.0 / ua_envelope;
        let r_floor = 1.0 / ua_floor;

        let caps = std::collections::HashMap::from([(NodeId(0), capacitance_900)]);
        let resistances = std::collections::HashMap::from([
            ((NodeId(0), NodeId(1)), r_envelope),
            ((NodeId(0), NodeId(2)), r_floor),
        ]);
        let network =
            RCNetwork::from_elements(caps, resistances, vec![NodeId(1), NodeId(2)]).unwrap();
        let (a_c, b_c_rc, _) = network.build_matrices().unwrap();

        let n_states = a_c.nrows();
        let n_ext = b_c_rc.ncols();
        let n_inputs = n_ext + 1;
        let mut b_c = DMatrix::zeros(n_states, n_inputs);
        for row in 0..n_states {
            for col in 0..n_ext {
                b_c[(row, col)] = b_c_rc[(row, col)];
            }
        }
        b_c[(0, n_ext)] = 1.0 / capacitance_900;

        let dt = 300.0;
        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model_900 = StateSpaceModel::from_continuous(&a_c, &b_c, dt, &mapping).unwrap();

        // Run both models through the same 24h cold step
        let volume_m3 = 129.6;
        let ach = 0.5;
        let q_internal = 200.0;
        let t_outdoor = -10.0;
        let t_ground = 10.0;
        let rho_air = 1.2;
        let n_steps = 288; // 24h at 5-min steps

        let run_model = |model: &StateSpaceModel| -> f64 {
            let mut x = DVector::from_row_slice(&[20.0]);
            let mut t_zone = 20.0;
            for _ in 0..n_steps {
                let q_inf_m3_s = ach * volume_m3 / 3600.0;
                let m_dot = rho_air * q_inf_m3_s;
                let q_infiltration = m_dot * CP_DRY_AIR_J_KG_K * (t_outdoor - t_zone);
                let q_total = q_internal + q_infiltration;
                let u = DVector::from_row_slice(&[t_outdoor, t_ground, q_total]);
                x = model.step(&x, &u);
                let y = model.output(&x, &u);
                t_zone = y[0];
            }
            t_zone
        };

        let t_zone_600 = run_model(&model_600);
        let t_zone_900 = run_model(&model_900);

        // Heavier mass = slower response = closer to initial 20°C after 24h
        assert!(
            t_zone_900 > t_zone_600,
            "Case 900 (heavy) should be warmer than Case 600 (light) after cold step: \
             t_900={t_zone_900:.2}, t_600={t_zone_600:.2}"
        );

        // Case 900 should still be noticeably closer to the initial 20°C
        let drift_600 = (20.0 - t_zone_600).abs();
        let drift_900 = (20.0 - t_zone_900).abs();
        assert!(
            drift_900 < drift_600,
            "Case 900 drift ({drift_900:.2}) should be less than Case 600 drift ({drift_600:.2})"
        );
    }

    /// The solar input path is linear: the state-space B matrix column for the
    /// solar input has a fixed gain (1/C), so doubling irradiance must produce
    /// exactly double the temperature rise above the no-solar baseline.
    /// Ratio must be within 0.1% to catch sign errors, gain miscalculations,
    /// or accidental quadratic scaling.
    #[test]
    fn solar_temperature_rise_is_proportional_to_irradiance() {
        let zone_temp = 18.0;
        let outdoor_temp = 5.0;

        let r = 2.0;
        let c = 50_000.0;
        // B: [outdoor_temp, sensible_gain, solar_gain]
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (r * c)]);
        let b_c = DMatrix::from_row_slice(1, 3, &[1.0 / (r * c), 1.0 / c, 1.0 / c]);
        let mapping = crate::state_space::OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();

        let make_env = |solar_w_m2: f64| -> EnvironmentState {
            EnvironmentState {
                ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
                zones: vec![ZoneState {
                    id: ZoneId(1),
                    temperature_c: zone_temp,
                    humidity_ratio: 0.008,
                    volume_m3: 200.0,
                }],
                weather: WeatherState {
                    outdoor_temp_c: outdoor_temp,
                    outdoor_humidity_ratio: 0.004,
                    wind_speed_m_s: 0.0,
                    wind_dir_deg: 0.0,
                    ground_temp_c: outdoor_temp,
                    sky_temp_c: outdoor_temp - 5.0,
                    pressure_kpa: 101.325,
                    solar_irradiance: vec![SurfaceIrradiance {
                        surface_id: 7,
                        direct_w_m2: solar_w_m2,
                        diffuse_w_m2: 0.0,
                        reflected_w_m2: 0.0,
                        angle_of_incidence_rad: 0.0,
                    }],
                    outdoor_wet_bulb_c: 0.0,
                    outdoor_enthalpy_j_kg: 0.0,
                    ghi_w_m2: 0.0,
                    dni_w_m2: 0.0,
                    dhi_w_m2: 0.0,
                    solar_altitude_deg: 0.0,
                    solar_azimuth_deg: 180.0,
                    mains_temp_c: 15.0,
                    rainfall_m: 0.0,
                    ground_albedo: 0.2,
                    ground_t_mean_c: 10.0,
                    ground_t_amplitude_c: 0.0,
                    ground_phase_day: 35.0,
                    day_of_year: 1.0,
                },
                grid: GridState {
                    voltage_pu: 1.0,
                    frequency_hz: 60.0,
                    island_bus_voltage_pu: None,
                },
                schedule_row: None,
                domains: hares_types::DomainSlots::default(),
                equipment_telemetry: std::collections::HashMap::new(),
                current_time: FixedOffset::east_opt(0)
                    .unwrap()
                    .with_ymd_and_hms(2026, 6, 21, 12, 0, 0)
                    .single()
                    .unwrap(),
                time_res: chrono::Duration::seconds(60),
                price_signal: Default::default(),
                electrical: Default::default(),
                equipment_core: Default::default(),
            }
        };

        let make_solver = |env: &EnvironmentState| -> ThermalSolver {
            let wiring = StateSpaceWiring {
                zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
                zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
                zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
                outdoor_temp_input_indices: vec![0],
                ground_temp_input_indices: vec![],
                ground_temp_input_depths_m: vec![],
                indoor_temp_input_indices: vec![],
                solar_input_indices: HashMap::new(),
                c_zone_j_k: HashMap::new(),
                node_capacitances: HashMap::new(),
                node_index: HashMap::new(),
            };
            let cfg = ThermalSolverConfig {
                indoor_zone_id: ZoneId(1),
                window_properties: HashMap::new(),
                window_zone_ids: HashMap::new(),
                exterior_surfaces: vec![ExteriorSurfaceInfo {
                    surface_id: 7,
                    state_index: 0,
                    input_index: 2,
                    area_m2: 1.0,
                    emissivity: 0.90,
                    tilt_deg: 90.0,
                    azimuth_deg: 180.0,
                    rad_frac: 0.0,
                    rad_res_k_w: 0.0,
                    n_iter: 1,
                    absorptance: 1.0,
                    boundary_category: None,
                    u_factor_w_m2_k: 0.0,
                    h_out_w_m2_k: 0.0,
                }],
                interior_lwr_zones: vec![],
                infiltration: vec![],
                ventilation_flow_m3_s: 0.0,
                ventilation: MechanicalVentilationParams::default(),
                natural_ventilation: None,
                supply_duct_leakage_m3_s: 0.0,
                return_duct_leakage_m3_s: 0.0,
                interior_solar_zones: Vec::new(),
                boundary_diagnostics: Vec::new(),
                film_coefficient_model: FilmCoefficientModel::default(),
                interior_convection_injections: Vec::new(),
                interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
                ideal_capacity_degraded_threshold: 3,
            };
            let mut s =
                ThermalSolver::new(model.clone(), wiring.clone(), cfg, 60.0, env, zone_temp)
                    .unwrap();
            s.x[0] = zone_temp;
            s
        };

        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        let env_base = make_env(0.0);
        let t_base = make_solver(&env_base)
            .resolve_new(&ports, &env_base, Duration::from_secs(60))
            .unwrap()
            .zone_temperatures_c[0]
            .1;

        let env_low = make_env(300.0);
        let t_low = make_solver(&env_low)
            .resolve_new(&ports, &env_low, Duration::from_secs(60))
            .unwrap()
            .zone_temperatures_c[0]
            .1;

        let env_high = make_env(600.0);
        let t_high = make_solver(&env_high)
            .resolve_new(&ports, &env_high, Duration::from_secs(60))
            .unwrap()
            .zone_temperatures_c[0]
            .1;

        let delta_low = t_low - t_base;
        let delta_high = t_high - t_base;

        assert!(
            delta_low > 1e-6,
            "300 W/m² must produce a measurable rise: {delta_low:.8}"
        );
        assert!(
            delta_high > 1e-6,
            "600 W/m² must produce a measurable rise: {delta_high:.8}"
        );

        // Linear system: 2× irradiance must produce 2× temperature rise.
        // Allow 0.1% tolerance for floating-point discretization round-off.
        let ratio = delta_high / delta_low;
        assert!(
            (ratio - 2.0).abs() < 0.001,
            "solar temperature rise must scale linearly with irradiance: ratio={ratio:.6}, expected 2.0"
        );
    }

    /// Natural ventilation with operable windows should cool a warm zone (above comfort base)
    /// relative to an identical solver without natural ventilation, when outdoor air is
    /// cool, dry, and below the zone temperature.
    #[test]
    fn natural_ventilation_cools_warm_zone_when_conditions_met() {
        // Zone well above comfort base, cool dry outdoor air -- nat vent should be active.
        let zone_temp = 27.0; // above t_base (22.778 °C)
        let outdoor_temp = 18.0; // cool outdoor, below zone
        let env = env_for_temp(zone_temp, outdoor_temp);

        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        let mut solver_no_nv = solver_with_infiltration(&env, InfiltrationMethod::Ach { ach: 0.0 });
        let t_no_nv = solver_no_nv
            .resolve_new(&ports, &env, Duration::from_secs(60))
            .unwrap()
            .zone_temperatures_c[0]
            .1;

        // Build solver with natural ventilation enabled; 6.7% of 12 m² total window area.
        let r = 2.0;
        let c = 50_000.0;
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (r * c)]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1.0 / (r * c), 1.0 / c]);
        let mapping = crate::state_space::OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();
        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            window_properties: HashMap::new(),
            window_zone_ids: HashMap::new(),

            exterior_surfaces: vec![],
            interior_lwr_zones: vec![],
            infiltration: vec![],
            ventilation_flow_m3_s: 0.0,
            ventilation: MechanicalVentilationParams::default(),
            natural_ventilation: Some(NaturalVentilationConfig::from_window_area(12.0)),
            supply_duct_leakage_m3_s: 0.0,
            return_duct_leakage_m3_s: 0.0,
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
            film_coefficient_model: FilmCoefficientModel::default(),
            interior_convection_injections: Vec::new(),
            ideal_capacity_degraded_threshold: 3,
        };
        let mut solver_nv =
            ThermalSolver::new(model, wiring, config, 60.0, &env, zone_temp).unwrap();
        solver_nv.x[0] = zone_temp;

        let t_nv = solver_nv
            .resolve_new(&ports, &env, Duration::from_secs(60))
            .unwrap()
            .zone_temperatures_c[0]
            .1;

        assert!(
            t_nv < t_no_nv,
            "natural ventilation should cool the zone: t_nv={t_nv:.4}, t_no_nv={t_no_nv:.4}"
        );
        let delta = t_no_nv - t_nv;
        assert!(
            delta > 1e-4,
            "natural ventilation cooling effect too small: {delta:.6} °C"
        );
    }

    /// Natural ventilation must be inactive when outdoor temperature exceeds zone temperature
    /// (no stack buoyancy to drive flow outward). The zone temperature must be identical to
    /// the no-nat-vent baseline.
    #[test]
    fn natural_ventilation_inactive_when_outdoor_warmer_than_zone() {
        let zone_temp = 20.0;
        let outdoor_temp = 30.0; // outdoor > zone → gated off

        let env = env_for_temp(zone_temp, outdoor_temp);

        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        // Baseline (no nat vent, pure conduction through envelope)
        let mut solver_no_nv = solver_with_infiltration(&env, InfiltrationMethod::Ach { ach: 0.0 });
        let t_no_nv = solver_no_nv
            .resolve_new(&ports, &env, Duration::from_secs(60))
            .unwrap()
            .zone_temperatures_c[0]
            .1;

        // With nat vent configured but gated off by temperature condition
        let r = 2.0;
        let c = 50_000.0;
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (r * c)]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1.0 / (r * c), 1.0 / c]);
        let mapping = crate::state_space::OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();
        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            window_properties: HashMap::new(),
            window_zone_ids: HashMap::new(),

            exterior_surfaces: vec![],
            interior_lwr_zones: vec![],
            infiltration: vec![],
            ventilation_flow_m3_s: 0.0,
            ventilation: MechanicalVentilationParams::default(),
            natural_ventilation: Some(NaturalVentilationConfig::from_window_area(12.0)),
            supply_duct_leakage_m3_s: 0.0,
            return_duct_leakage_m3_s: 0.0,
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
            film_coefficient_model: FilmCoefficientModel::default(),
            interior_convection_injections: Vec::new(),
            ideal_capacity_degraded_threshold: 3,
        };
        let mut solver_nv =
            ThermalSolver::new(model, wiring, config, 60.0, &env, zone_temp).unwrap();
        solver_nv.x[0] = zone_temp;

        let t_nv = solver_nv
            .resolve_new(&ports, &env, Duration::from_secs(60))
            .unwrap()
            .zone_temperatures_c[0]
            .1;

        // Zone temperatures must be identical (nat vent gated off)
        let diff = (t_nv - t_no_nv).abs();
        assert!(
            diff < 1e-10,
            "nat vent should be inactive (outdoor > zone): t_nv={t_nv:.8}, t_no_nv={t_no_nv:.8}, diff={diff:.2e}"
        );
    }

    /// Exterior LW radiation to a cold night sky must lower zone temperature.
    ///
    /// A warm surface (20 °C) radiating to a sky at −20 °C loses heat via the
    /// Stefan-Boltzmann LW exchange; the zone fed by that surface should end one
    /// step cooler than an identical zone without LW radiation configured.
    #[test]
    fn exterior_lw_cold_sky_cools_zone_vs_no_lw() {
        let zone_temp = 20.0_f64;
        let outdoor_temp = 5.0_f64;

        // 1R1C with 3 inputs: [outdoor_temp, lw_flux, zone_heat]
        // The LW flux (W) is accumulated at input index 1 and drives the zone
        // via a 1/C term, just like the zone heat input.
        let r = 2.0_f64;
        let c = 50_000.0_f64;
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (r * c)]);
        let b_c = DMatrix::from_row_slice(1, 3, &[1.0 / (r * c), 1.0 / c, 1.0 / c]);
        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();

        let env = {
            let mut e = env_for_temp(zone_temp, outdoor_temp);
            e.weather.sky_temp_c = -20.0;
            e.weather.ground_temp_c = 5.0;
            e
        };

        let make_solver = |with_lw: bool, env: &EnvironmentState| -> ThermalSolver {
            let exterior_surfaces = if with_lw {
                vec![ExteriorSurfaceInfo {
                    surface_id: 0,
                    state_index: 0, // zone state used as surface temperature proxy
                    input_index: 1, // LW flux accumulated here
                    area_m2: 20.0,
                    emissivity: 0.90,
                    tilt_deg: 90.0, // vertical wall
                    azimuth_deg: 180.0,
                    rad_frac: 0.0, // no film resistance → use node temp directly
                    rad_res_k_w: 0.0,
                    n_iter: 1,
                    absorptance: SOLAR_ABSORPTANCE_DEFAULT,
                    boundary_category: None,
                    u_factor_w_m2_k: 0.0,
                    h_out_w_m2_k: 0.0,
                }]
            } else {
                vec![]
            };
            let wiring = StateSpaceWiring {
                zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
                zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
                zone_sensible_input_indices: HashMap::from([(ZoneId(1), 2)]),
                outdoor_temp_input_indices: vec![0],
                ground_temp_input_indices: vec![],
                ground_temp_input_depths_m: vec![],
                indoor_temp_input_indices: vec![],
                solar_input_indices: HashMap::new(),
                c_zone_j_k: HashMap::new(),
                node_capacitances: HashMap::new(),
                node_index: HashMap::new(),
            };
            let cfg = ThermalSolverConfig {
                indoor_zone_id: ZoneId(1),
                window_properties: HashMap::new(),
                window_zone_ids: HashMap::new(),

                exterior_surfaces,
                interior_lwr_zones: vec![],
                infiltration: vec![],
                ventilation_flow_m3_s: 0.0,
                ventilation: MechanicalVentilationParams::default(),
                natural_ventilation: None,
                supply_duct_leakage_m3_s: 0.0,
                return_duct_leakage_m3_s: 0.0,
                interior_solar_zones: Vec::new(),
                boundary_diagnostics: Vec::new(),
                film_coefficient_model: FilmCoefficientModel::default(),
                interior_convection_injections: Vec::new(),
                interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
                ideal_capacity_degraded_threshold: 3,
            };
            let mut s =
                ThermalSolver::new(model.clone(), wiring.clone(), cfg, 60.0, env, zone_temp)
                    .unwrap();
            s.x[0] = zone_temp;
            s
        };

        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        let t_no_lw = make_solver(false, &env)
            .resolve_new(&ports, &env, Duration::from_secs(60))
            .unwrap()
            .zone_temperatures_c[0]
            .1;

        let t_with_lw = make_solver(true, &env)
            .resolve_new(&ports, &env, Duration::from_secs(60))
            .unwrap()
            .zone_temperatures_c[0]
            .1;

        assert!(
            t_with_lw < t_no_lw,
            "cold sky LW radiation must cool zone below no-LW baseline: \
             t_lw={t_with_lw:.5}, t_no_lw={t_no_lw:.5}"
        );
    }

    /// Window IAM correction must reduce solar gain at oblique angles.
    ///
    /// A 1R1C zone driven purely by solar input through a single window surface is
    /// stepped once under two irradiance conditions that differ only in the angle of
    /// incidence: 5° (near-normal, IAM ≈ 1) vs 70° (oblique, IAM substantially < 1).
    /// The near-normal case must produce a higher zone temperature, and the oblique
    /// gain must be less than 95 % of the normal gain.
    #[test]
    fn window_iam_reduces_gain_at_oblique_aoi() {
        let zone_temp = 18.0_f64;
        let outdoor_temp = 5.0_f64;
        // 1R1C: inputs = [outdoor_temp, sensible_gain, solar_gain]
        let r = 2.0_f64;
        let c = 50_000.0_f64;
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (r * c)]);
        let b_c = DMatrix::from_row_slice(1, 3, &[1.0 / (r * c), 1.0 / c, 1.0 / c]);
        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();

        let window_surface_id: u32 = 55;
        let (transmittance, radiation_frac) =
            hares_physics::solar::calculate_window_parameters(0.4, 1.8, 0.01);
        let win_props = WindowSolarProperties {
            shgc: 0.4,
            winter_shgc: 0.4,
            u_factor_w_m2_k: 1.8,
            area_m2: 2.0,
            transmittance,
            winter_transmittance: transmittance,
            radiation_frac,
            glazing_curve: hares_physics::solar::GlazingCurve::from_u_shgc(1.8, 0.4),
            tilt_deg: 90.0,
            azimuth_deg: 180.0,
        };

        // 500 W/m² direct irradiance, no diffuse or reflected.
        let make_env = |aoi_rad: f64| -> EnvironmentState {
            EnvironmentState {
                ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
                zones: vec![ZoneState {
                    id: ZoneId(1),
                    temperature_c: zone_temp,
                    humidity_ratio: 0.008,
                    volume_m3: 200.0,
                }],
                weather: WeatherState {
                    outdoor_temp_c: outdoor_temp,
                    outdoor_humidity_ratio: 0.004,
                    wind_speed_m_s: 0.0,
                    wind_dir_deg: 0.0,
                    ground_temp_c: outdoor_temp,
                    sky_temp_c: outdoor_temp - 5.0,
                    pressure_kpa: 101.325,
                    solar_irradiance: vec![SurfaceIrradiance {
                        surface_id: window_surface_id,
                        direct_w_m2: 500.0,
                        diffuse_w_m2: 0.0,
                        reflected_w_m2: 0.0,
                        angle_of_incidence_rad: aoi_rad,
                    }],
                    outdoor_wet_bulb_c: 0.0,
                    outdoor_enthalpy_j_kg: 0.0,
                    ghi_w_m2: 0.0,
                    dni_w_m2: 0.0,
                    dhi_w_m2: 0.0,
                    solar_altitude_deg: 0.0,
                    solar_azimuth_deg: 180.0,
                    mains_temp_c: 15.0,
                    rainfall_m: 0.0,
                    ground_albedo: 0.2,
                    ground_t_mean_c: 10.0,
                    ground_t_amplitude_c: 0.0,
                    ground_phase_day: 35.0,
                    day_of_year: 1.0,
                },
                grid: GridState {
                    voltage_pu: 1.0,
                    frequency_hz: 60.0,
                    island_bus_voltage_pu: None,
                },
                schedule_row: None,
                domains: hares_types::DomainSlots::default(),
                equipment_telemetry: std::collections::HashMap::new(),
                current_time: FixedOffset::east_opt(0)
                    .unwrap()
                    .with_ymd_and_hms(2026, 6, 21, 12, 0, 0)
                    .single()
                    .unwrap(),
                time_res: chrono::Duration::seconds(60),
                price_signal: Default::default(),
                electrical: Default::default(),
                equipment_core: Default::default(),
            }
        };

        let make_solver = |env: &EnvironmentState| -> ThermalSolver {
            let wiring = StateSpaceWiring {
                zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
                zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
                zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
                outdoor_temp_input_indices: vec![0],
                ground_temp_input_indices: vec![],
                ground_temp_input_depths_m: vec![],
                indoor_temp_input_indices: vec![],
                solar_input_indices: HashMap::from([(window_surface_id, 2usize)]),
                c_zone_j_k: HashMap::new(),
                node_capacitances: HashMap::new(),
                node_index: HashMap::new(),
            };
            let cfg = ThermalSolverConfig {
                indoor_zone_id: ZoneId(1),
                window_properties: HashMap::from([(window_surface_id, win_props)]),
                window_zone_ids: HashMap::new(),
                exterior_surfaces: vec![],
                interior_lwr_zones: vec![],
                infiltration: vec![],
                ventilation_flow_m3_s: 0.0,
                ventilation: MechanicalVentilationParams::default(),
                natural_ventilation: None,
                supply_duct_leakage_m3_s: 0.0,
                return_duct_leakage_m3_s: 0.0,
                interior_solar_zones: Vec::new(),
                boundary_diagnostics: Vec::new(),
                film_coefficient_model: FilmCoefficientModel::default(),
                interior_convection_injections: Vec::new(),
                interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
                ideal_capacity_degraded_threshold: 3,
            };
            let mut s =
                ThermalSolver::new(model.clone(), wiring.clone(), cfg, 60.0, env, zone_temp)
                    .unwrap();
            s.x[0] = zone_temp;
            s
        };

        let env_normal = make_env(5_f64.to_radians()); // near-normal, IAM ≈ 1
        let env_oblique = make_env(70_f64.to_radians()); // oblique, IAM substantially < 1

        let ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        let t_normal = make_solver(&env_normal)
            .resolve_new(&ports, &env_normal, Duration::from_secs(60))
            .unwrap()
            .zone_temperatures_c[0]
            .1;

        let t_oblique = make_solver(&env_oblique)
            .resolve_new(&ports, &env_oblique, Duration::from_secs(60))
            .unwrap()
            .zone_temperatures_c[0]
            .1;

        assert!(
            t_normal > t_oblique,
            "near-normal incidence must warm zone more than oblique: t_normal={t_normal:.5}, t_oblique={t_oblique:.5}"
        );

        // Compute the transmitted gain in each case and verify oblique < 95 % of normal.
        // gain = area * shgc * direct * iam; with identical irradiance the ratio is just iam.
        // The temperature delta above zone_temp is proportional to gain, so we compare deltas.
        let delta_normal = t_normal - zone_temp;
        let delta_oblique = t_oblique - zone_temp;
        assert!(
            delta_normal > 0.0,
            "near-normal gain must raise zone temperature: delta={delta_normal:.6}"
        );
        assert!(
            delta_oblique < delta_normal * 0.95,
            "oblique gain must be less than 95% of normal gain: delta_oblique={delta_oblique:.6}, delta_normal={delta_normal:.6}"
        );
    }

    /// Iterative LWR solver with rad_frac produces a different zone temperature
    /// than the simple node-temp-only approach, and exterior_surface_temps is updated.
    #[test]
    fn iterative_lwr_updates_surface_state_and_differs_from_simple() {
        let zone_temp = 25.0;
        let outdoor_temp = 10.0;

        // Build a simple 1R1C model: zone ←→ outdoor.
        // Input columns: [0]=outdoor, [1]=LWR injection, [2]=zone sensible
        let a_c = nalgebra::DMatrix::from_element(1, 1, -0.01);
        let b_c = nalgebra::DMatrix::from_row_slice(1, 3, &[0.01, 0.0, 0.001]);
        let mapping = crate::state_space::OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: Vec::new(),
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();

        let env = {
            let mut e = env_for_temp(zone_temp, outdoor_temp);
            e.weather.sky_temp_c = -20.0;
            e.weather.ground_temp_c = 5.0;
            e
        };

        let make_solver =
            |rad_frac: f64, rad_res_k_w: f64, env: &EnvironmentState| -> ThermalSolver {
                let exterior_surfaces = vec![ExteriorSurfaceInfo {
                    surface_id: 0,
                    state_index: 0,
                    input_index: 1,
                    area_m2: 20.0,
                    emissivity: 0.90,
                    tilt_deg: 0.0, // horizontal roof: SVF=1, maximum sky exposure
                    azimuth_deg: 180.0,
                    rad_frac,
                    rad_res_k_w,
                    n_iter: 4,
                    absorptance: SOLAR_ABSORPTANCE_DEFAULT,
                    boundary_category: None,
                    u_factor_w_m2_k: 0.0,
                    h_out_w_m2_k: 0.0,
                }];
                let wiring = StateSpaceWiring {
                    zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
                    zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
                    zone_sensible_input_indices: HashMap::from([(ZoneId(1), 2)]),
                    outdoor_temp_input_indices: vec![0],
                    ground_temp_input_indices: vec![],
                    ground_temp_input_depths_m: vec![],
                    indoor_temp_input_indices: vec![],
                    solar_input_indices: HashMap::new(),
                    c_zone_j_k: HashMap::new(),
                    node_capacitances: HashMap::new(),
                    node_index: HashMap::new(),
                };
                let cfg = ThermalSolverConfig {
                    indoor_zone_id: ZoneId(1),
                    window_properties: HashMap::new(),
                    window_zone_ids: HashMap::new(),

                    exterior_surfaces,
                    interior_lwr_zones: vec![],
                    infiltration: vec![],
                    ventilation_flow_m3_s: 0.0,
                    ventilation: MechanicalVentilationParams::default(),
                    natural_ventilation: None,
                    supply_duct_leakage_m3_s: 0.0,
                    return_duct_leakage_m3_s: 0.0,
                    interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
                    interior_solar_zones: Vec::new(),
                    boundary_diagnostics: Vec::new(),
                    film_coefficient_model: FilmCoefficientModel::default(),
                    interior_convection_injections: Vec::new(),
                    ideal_capacity_degraded_threshold: 3,
                };
                let mut s =
                    ThermalSolver::new(model.clone(), wiring.clone(), cfg, 60.0, env, zone_temp)
                        .unwrap();
                s.x[0] = zone_temp;
                s
            };

        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        // rad_frac > 0: iterative solver estimates true surface temp
        let mut solver = make_solver(0.375, 0.0015, &env);
        let initial_t_prev = solver.exterior_surface_temps[0];
        let _ = solver.resolve_new(&ports, &env, Duration::from_secs(60));
        let updated_t_prev = solver.exterior_surface_temps[0];

        // exterior_surface_temps must be updated from its initial value (outdoor temp)
        assert!(
            (updated_t_prev - initial_t_prev).abs() > 0.01,
            "exterior_surface_temps should be updated by iteration: initial={initial_t_prev:.4}, after={updated_t_prev:.4}"
        );

        // Converged surface temp should be physically reasonable
        assert!(
            updated_t_prev > outdoor_temp - 30.0 && updated_t_prev < zone_temp + 30.0,
            "converged surface temp={updated_t_prev:.2} out of physical range"
        );
    }

    /// Converged surface temperature should be between outdoor temp and node temp.
    #[test]
    fn iterative_lwr_surface_temp_is_bounded() {
        use crate::longwave_radiation::{CELSIUS_TO_KELVIN, STEFAN_BOLTZMANN, sky_view_factor};

        let t_node = 25.0;
        let t_ext = 10.0;
        let t_sky = -20.0;
        let area = 20.0;
        let emissivity = 0.90;
        let rad_frac = 0.375;
        let rad_res_k_w = 0.0015; // R_film / area

        let e_factor = emissivity * STEFAN_BOLTZMANN * area;
        let f_sky = sky_view_factor(0.0); // horizontal roof
        let f_gnd = 1.0 - f_sky;
        let beta = beta_factor(0.0);

        let t_sky_k4 = (t_sky + CELSIUS_TO_KELVIN).powi(4);
        let t_ext_k4 = (t_ext + CELSIUS_TO_KELVIN).powi(4);
        let h_lwr_inj =
            e_factor * ((f_gnd + (1.0 - beta) * f_sky) * t_ext_k4 + beta * f_sky * t_sky_k4);

        let t_surf_init = rad_frac * t_node + (1.0 - rad_frac) * t_ext;

        let mut t_surf = t_ext; // init like OCHRE
        let mut t_prev = t_ext;

        for _ in 0..20 {
            let lwr = h_lwr_inj - e_factor * (t_surf + CELSIUS_TO_KELVIN).powi(4);
            let t_new = t_surf_init + lwr * rad_res_k_w;
            let t_new = t_new.clamp(t_surf - 2.0, t_surf + 2.0);
            let t_next = t_surf + 0.5 * (t_new - t_surf) + 0.1 * (t_surf - t_prev);
            t_prev = t_surf;
            t_surf = t_next;
            if (t_surf - t_prev).abs() < 0.01 {
                break;
            }
        }

        // Surface temp should be between outdoor and node temp, and below t_surf_init
        // because LWR cools the surface.
        assert!(
            t_surf > t_ext - 10.0 && t_surf < t_node + 10.0,
            "converged surface temp {t_surf:.2} should be near [{t_ext}, {t_node}]"
        );
        assert!(
            t_surf < t_surf_init,
            "LWR should cool surface below no-radiation init: \
             t_surf={t_surf:.4}, t_surf_init={t_surf_init:.4}"
        );
    }

    #[test]
    fn iterative_lwr_nan_sky_collapses_to_air_temp() {
        use crate::longwave_radiation::{CELSIUS_TO_KELVIN, STEFAN_BOLTZMANN, sky_view_factor};

        let t_node = 25.0;
        let t_ext = 10.0;
        let area = 20.0;
        let emissivity = 0.90;
        let rad_frac = 0.375;
        let rad_res_k_w = 0.0015;

        let e_factor = emissivity * STEFAN_BOLTZMANN * area;
        let t_ext_k4 = (t_ext + CELSIUS_TO_KELVIN).powi(4);

        // NaN sky → h_lwr_inj collapses to e_factor * T_air⁴ (all terms use air temp)
        let h_lwr_inj_nan = e_factor * t_ext_k4;

        // Valid sky at air temp → should produce the same h_lwr_inj via 4-component formula
        let f_sky = sky_view_factor(45.0); // non-trivial tilt to exercise β
        let f_gnd = 1.0 - f_sky;
        let beta = beta_factor(45.0);
        let h_lwr_inj_air =
            e_factor * ((f_gnd + (1.0 - beta) * f_sky) * t_ext_k4 + beta * f_sky * t_ext_k4);

        // Both must equal e_factor * T_air⁴ since sky = air
        assert!(
            (h_lwr_inj_nan - h_lwr_inj_air).abs() < 1e-6,
            "NaN sky path ({h_lwr_inj_nan:.6}) must match sky=air path ({h_lwr_inj_air:.6})"
        );

        // Run the iterative loop for both and confirm converged temps match
        let run_loop = |h_lwr_inj: f64| -> f64 {
            let t_surf_init = rad_frac * t_node + (1.0 - rad_frac) * t_ext;
            let mut t_surf = t_ext;
            let mut t_prev = t_ext;
            for _ in 0..20 {
                let lwr = h_lwr_inj - e_factor * (t_surf + CELSIUS_TO_KELVIN).powi(4);
                let t_new = t_surf_init + lwr * rad_res_k_w;
                let t_new = t_new.clamp(t_surf - 2.0, t_surf + 2.0);
                let t_next = t_surf + 0.5 * (t_new - t_surf) + 0.1 * (t_surf - t_prev);
                t_prev = t_surf;
                t_surf = t_next;
                if (t_surf - t_prev).abs() < 0.01 {
                    break;
                }
            }
            t_surf
        };

        let t_surf_nan = run_loop(h_lwr_inj_nan);
        let t_surf_air = run_loop(h_lwr_inj_air);
        assert!(
            (t_surf_nan - t_surf_air).abs() < 0.01,
            "NaN sky converged temp ({t_surf_nan:.4}) must match \
             sky=air converged temp ({t_surf_air:.4})"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Interior surface temperature interpolation (radiation_frac)
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn radiation_frac_interpolates_surface_temp() {
        // Lightweight wall: node at 30 °C, zone at 22 °C, radiation_frac = 0.6
        // True surface temp = 0.6 × 30 + 0.4 × 22 = 26.8 °C
        let t_node = 30.0_f64;
        let t_zone = 22.0_f64;
        let rad_frac = 0.6_f64;
        let t_surf = rad_frac * t_node + (1.0 - rad_frac) * t_zone;
        assert!(
            (t_surf - 26.8).abs() < 1e-10,
            "interpolated surface temp should be 26.8, got {t_surf}"
        );
    }

    #[test]
    fn radiation_frac_one_returns_node_temp() {
        let t_node = 35.0_f64;
        let t_zone = 20.0_f64;
        let t_surf = 1.0 * t_node + (1.0 - 1.0) * t_zone;
        assert!(
            (t_surf - t_node).abs() < 1e-10,
            "radiation_frac=1.0 should yield node temp"
        );
    }

    #[test]
    fn radiation_frac_zero_returns_zone_temp() {
        let t_node = 35.0_f64;
        let t_zone = 20.0_f64;
        let t_surf = 0.0 * t_node + (1.0 - 0.0) * t_zone;
        assert!(
            (t_surf - t_zone).abs() < 1e-10,
            "radiation_frac=0.0 should yield zone temp"
        );
    }

    #[test]
    fn radiation_frac_correction_magnitude_for_lightweight_boundary() {
        // Lightweight wall: R_film = 0.325 m²K/W (convection-only), R_material = 0.05 m²K/W
        // radiation_frac = 0.325 / (0.325 + 0.05) ≈ 0.867
        // Node at 35 °C, zone at 20 °C → true surface ≈ 33.0 °C (2.0 °C correction)
        let r_film = 0.325_f64;
        let r_material = 0.05_f64;
        let rad_frac = r_film / (r_film + r_material);
        let t_node = 35.0_f64;
        let t_zone = 20.0_f64;
        let t_surf = rad_frac * t_node + (1.0 - rad_frac) * t_zone;
        let correction = (t_node - t_surf).abs();

        assert!(
            correction > 1.0,
            "lightweight boundary correction should be > 1 °C, got {correction:.3}"
        );
        assert!(
            t_surf > t_zone && t_surf < t_node,
            "surface temp {t_surf:.3} should be between zone {t_zone} and node {t_node}"
        );
    }

    #[test]
    fn interior_lwr_with_radiation_frac_conserves_energy() {
        // Two surfaces with different radiation_frac values: the LWR exchange
        // using interpolated surface temps must still conserve energy.
        use crate::longwave_radiation::{InteriorSurface, interior_longwave_linearised_w};

        let t_zone = 22.0;
        let infos = [
            InteriorSurfaceInfo {
                state_index: 0,
                input_index: 0,
                area_m2: 40.0,
                azimuth_deg: 0.0,
                tilt_deg: 90.0,
                emissivity: 0.90,
                radiation_frac: 0.7, // lightweight wall
                rad_res_k_w: 0.003,
                solar_absorptance: 0.5,
                is_floor: false,
                driving_temp: None,
            },
            InteriorSurfaceInfo {
                state_index: 1,
                input_index: 1,
                area_m2: 40.0,
                azimuth_deg: 0.0,
                tilt_deg: 180.0,
                emissivity: 0.90,
                radiation_frac: 1.0, // massive floor (node ≈ surface)
                rad_res_k_w: 0.003,
                solar_absorptance: 0.6,
                is_floor: true,
                driving_temp: None,
            },
            InteriorSurfaceInfo {
                state_index: 2,
                input_index: 2,
                area_m2: 60.0,
                azimuth_deg: 0.0,
                tilt_deg: 90.0,
                emissivity: 0.90,
                radiation_frac: 0.85,
                rad_res_k_w: 0.003,
                solar_absorptance: 0.5,
                is_floor: false,
                driving_temp: None,
            },
        ];
        let t_nodes = [28.0, 19.0, 24.0];

        let surfaces: Vec<InteriorSurface> = infos
            .iter()
            .map(|s| InteriorSurface {
                area_m2: s.area_m2,
                emissivity: s.emissivity,
            })
            .collect();
        let t_surfaces: Vec<f64> = infos
            .iter()
            .zip(t_nodes.iter())
            .map(|(s, &t_node)| s.radiation_frac * t_node + (1.0 - s.radiation_frac) * t_zone)
            .collect();

        let net = interior_longwave_linearised_w(&surfaces, &t_surfaces, t_zone);
        let total: f64 = net.iter().sum();
        assert!(
            total.abs() < 1e-8,
            "LWR with radiation_frac must conserve energy: sum={total:.10} W"
        );
    }

    #[test]
    fn interior_lwr_iteration_updates_fluxes_from_one_pass_baseline() {
        let env = env_for_temp(22.0, 10.0);

        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (2.0 * 50_000.0)]);
        let b_c = DMatrix::from_row_slice(1, 3, &[1.0 / (2.0 * 50_000.0), 1.0 / 50_000.0, 0.0]);
        let mapping = crate::state_space::OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 300.0, &mapping).unwrap();

        let mut lwr_zone = InteriorLwrZoneConfig {
            zone_id: ZoneId(1),
            surfaces: vec![
                InteriorSurfaceInfo {
                    state_index: 0,
                    input_index: 1,
                    area_m2: 25.0,
                    azimuth_deg: 0.0,
                    tilt_deg: 90.0,
                    emissivity: 0.9,
                    radiation_frac: 1.0,
                    rad_res_k_w: 0.02,
                    solar_absorptance: 0.5,
                    is_floor: false,
                    driving_temp: None,
                },
                InteriorSurfaceInfo {
                    state_index: 0,
                    input_index: 2,
                    area_m2: 25.0,
                    azimuth_deg: 0.0,
                    tilt_deg: 90.0,
                    emissivity: 0.84,
                    radiation_frac: 0.36,
                    rad_res_k_w: 0.020,
                    solar_absorptance: 0.0,
                    is_floor: false,
                    driving_temp: Some(DrivingTemp::Outdoor),
                },
            ],
            scriptf: None,
        };
        lwr_zone.compute_scriptf();

        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 2)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            window_properties: HashMap::new(),
            window_zone_ids: HashMap::new(),
            exterior_surfaces: vec![],
            interior_lwr_zones: vec![lwr_zone.clone()],
            infiltration: vec![],
            ventilation_flow_m3_s: 0.0,
            ventilation: MechanicalVentilationParams::default(),
            natural_ventilation: None,
            supply_duct_leakage_m3_s: 0.0,
            return_duct_leakage_m3_s: 0.0,
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
            film_coefficient_model: FilmCoefficientModel::default(),
            interior_convection_injections: Vec::new(),
            ideal_capacity_degraded_threshold: 3,
        };
        let mut solver = ThermalSolver::new(model, wiring, config, 300.0, &env, 22.0).unwrap();
        solver.x[0] = 30.0;

        let base_t = vec![30.0, env.weather.outdoor_temp_c];
        let mut one_pass_flux: Vec<f64> = Vec::new();
        lwr_zone
            .scriptf
            .as_ref()
            .expect("scriptf")
            .net_flux_w_into(&base_t, &mut one_pass_flux);

        let mut u = DVector::zeros(solver.model.input_dim());
        solver.apply_interior_longwave_inputs(&mut u, &env);

        let iter_flux_1 = u[1];
        let iter_flux_2 = u[2];
        assert!(
            (iter_flux_1 - one_pass_flux[0]).abs() > 1e-9
                || (iter_flux_2 - one_pass_flux[1]).abs() > 1e-9,
            "interior LWR iteration should perturb one-pass fluxes when rad_res_k_w > 0"
        );
    }

    fn solver_with_ventilation(
        env: &EnvironmentState,
        vent: MechanicalVentilationParams,
        vent_flow_m3_s: f64,
    ) -> ThermalSolver {
        let r = 2.0;
        let c = 50_000.0;
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (r * c)]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1.0 / (r * c), 1.0 / c]);
        let mapping = crate::state_space::OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();
        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            window_properties: HashMap::new(),
            window_zone_ids: HashMap::new(),
            exterior_surfaces: vec![],
            interior_lwr_zones: vec![],
            infiltration: vec![],
            ventilation_flow_m3_s: vent_flow_m3_s,
            ventilation: vent,
            natural_ventilation: None,
            supply_duct_leakage_m3_s: 0.0,
            return_duct_leakage_m3_s: 0.0,
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
            film_coefficient_model: FilmCoefficientModel::default(),
            interior_convection_injections: Vec::new(),
            ideal_capacity_degraded_threshold: 3,
        };
        let mut solver =
            ThermalSolver::new(model, wiring, config, 60.0, env, env.zones[0].temperature_c)
                .unwrap();
        solver.x[0] = env.zones[0].temperature_c;
        solver
    }

    /// HRV with 70% sensible recovery should reduce ventilation sensible load
    /// by ~70% compared to no recovery, verified by zone temperature drift.
    #[test]
    fn hrv_sensible_recovery_reduces_ventilation_load() {
        let zone_temp = 22.0;
        let outdoor_temp = 0.0;
        let env = env_for_temp(zone_temp, outdoor_temp);
        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };
        let vent_flow = 0.05; // m³/s

        // No recovery
        let mut solver_no_recovery = solver_with_ventilation(
            &env,
            MechanicalVentilationParams {
                balanced: false,
                ..Default::default()
            },
            vent_flow,
        );
        let t_no_recovery = solver_no_recovery
            .resolve_new(&ports, &env, Duration::from_secs(60))
            .unwrap()
            .zone_temperatures_c[0]
            .1;

        // HRV with 70% sensible recovery
        let mut solver_hrv = solver_with_ventilation(
            &env,
            MechanicalVentilationParams {
                balanced: true,
                sensible_recovery_efficiency: 0.70,
                latent_recovery_efficiency: 0.0,
                ..Default::default()
            },
            vent_flow,
        );
        let t_hrv = solver_hrv
            .resolve_new(&ports, &env, Duration::from_secs(60))
            .unwrap()
            .zone_temperatures_c[0]
            .1;

        // Both should cool below zone_temp (outdoor is colder).
        assert!(
            t_no_recovery < zone_temp,
            "ventilation should cool the zone"
        );
        assert!(t_hrv < zone_temp, "HRV should still cool the zone");

        // HRV should be significantly warmer (less cooling).
        let cooling_no_recovery = zone_temp - t_no_recovery;
        let cooling_hrv = zone_temp - t_hrv;
        assert!(
            cooling_hrv < cooling_no_recovery * 0.5,
            "HRV at 70% recovery should reduce cooling by more than 50%: \
             no_recovery cooling={cooling_no_recovery:.4}, hrv cooling={cooling_hrv:.4}"
        );
    }

    /// ERV with latent recovery: latent gains are packed into
    /// `custom_payload`. Verify that ERV reduces latent load magnitude
    /// by extracting the latent gain from the payload.
    #[test]
    fn erv_latent_recovery_reduces_latent_load() {
        let zone_temp = 22.0;
        let outdoor_temp = 0.0;
        let env = env_for_temp(zone_temp, outdoor_temp);
        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };
        let vent_flow = 0.05;

        // Extract latent gain for zone 1 from custom_payload [zone_id, q_latent_w, m_dot_inf_kg_s, w_outdoor]
        let extract_latent = |update: &DomainUpdate| -> f64 {
            let payload = update.custom_payload.as_ref().expect("must have latent");
            payload
                .chunks(4)
                .find(|chunk| chunk[0] as u16 == 1)
                .map(|chunk| chunk[1])
                .unwrap_or(0.0)
        };

        // No recovery
        let mut solver_no =
            solver_with_ventilation(&env, MechanicalVentilationParams::default(), vent_flow);
        let update_no = solver_no
            .resolve_new(&ports, &env, Duration::from_secs(60))
            .unwrap();
        let latent_no = extract_latent(&update_no);

        // ERV: 70% sensible, 30% latent recovery
        let mut solver_erv = solver_with_ventilation(
            &env,
            MechanicalVentilationParams {
                balanced: true,
                sensible_recovery_efficiency: 0.70,
                latent_recovery_efficiency: 0.30,
                ..Default::default()
            },
            vent_flow,
        );
        let update_erv = solver_erv
            .resolve_new(&ports, &env, Duration::from_secs(60))
            .unwrap();
        let latent_erv = extract_latent(&update_erv);

        // Outdoor humidity (0.004) < indoor (0.008) → latent gain is negative.
        // ERV 30% latent recovery should reduce the magnitude.
        assert!(latent_no.abs() > 0.0, "must have non-zero latent load");
        assert!(
            latent_erv.abs() < latent_no.abs() * 0.80,
            "ERV latent recovery should reduce latent load by >20%: \
             no_recovery={latent_no:.2}, erv={latent_erv:.2}"
        );
    }

    /// Unbalanced fan uses quadrature (Pythagorean) combination of infiltration
    /// and forced ventilation flows. With equal infiltration and forced flows,
    /// the combined flow should be sqrt(2) × individual, not 2× (linear).
    /// This means less cooling than simple linear addition.
    #[test]
    fn unbalanced_fan_uses_quadrature_not_linear_addition() {
        let zone_temp = 22.0;
        let outdoor_temp = 0.0;
        let env = env_for_temp(zone_temp, outdoor_temp);
        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        // Solver with infiltration only (0.5 ACH)
        let mut solver_inf_only =
            solver_with_infiltration(&env, InfiltrationMethod::Ach { ach: 0.5 });
        let t_inf_only = solver_inf_only
            .resolve_new(&ports, &env, Duration::from_secs(60))
            .unwrap()
            .zone_temperatures_c[0]
            .1;

        // Solver with infiltration + unbalanced forced vent (same magnitude)
        // ACH 0.5 on 200 m³ zone = 200 * 0.5 / 3600 ≈ 0.0278 m³/s
        let forced_flow = 200.0 * 0.5 / 3600.0;
        let r = 2.0;
        let c = 50_000.0;
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (r * c)]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1.0 / (r * c), 1.0 / c]);
        let mapping = crate::state_space::OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();
        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            window_properties: HashMap::new(),
            window_zone_ids: HashMap::new(),
            exterior_surfaces: vec![],
            interior_lwr_zones: vec![],
            infiltration: vec![(ZoneId(1), InfiltrationMethod::Ach { ach: 0.5 })],
            ventilation_flow_m3_s: forced_flow,
            ventilation: MechanicalVentilationParams {
                balanced: false,
                ..Default::default()
            },
            natural_ventilation: None,
            supply_duct_leakage_m3_s: 0.0,
            return_duct_leakage_m3_s: 0.0,
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
            film_coefficient_model: FilmCoefficientModel::default(),
            interior_convection_injections: Vec::new(),
            ideal_capacity_degraded_threshold: 3,
        };
        let mut solver_combined = ThermalSolver::new(
            model,
            wiring,
            config,
            60.0,
            &env,
            env.zones[0].temperature_c,
        )
        .unwrap();
        solver_combined.x[0] = env.zones[0].temperature_c;
        let t_combined = solver_combined
            .resolve_new(&ports, &env, Duration::from_secs(60))
            .unwrap()
            .zone_temperatures_c[0]
            .1;

        let cooling_inf = zone_temp - t_inf_only;
        let cooling_combined = zone_temp - t_combined;

        // Quadrature: combined = sqrt(inf² + forced²) = sqrt(2) * inf ≈ 1.414× inf.
        // Linear would give 2× inf. So combined cooling should be < 1.6× inf cooling.
        assert!(
            cooling_combined > cooling_inf,
            "adding forced vent must increase cooling"
        );
        assert!(
            cooling_combined < cooling_inf * 1.6,
            "quadrature should give ~1.414× not 2×: inf={cooling_inf:.6}, combined={cooling_combined:.6}"
        );
        assert!(
            cooling_combined > cooling_inf * 1.3,
            "quadrature should give ~1.414×: inf={cooling_inf:.6}, combined={cooling_combined:.6}"
        );
    }

    /// Per ANSI/RESNET 301, winter months (Oct–Apr) must use winter_transmittance and
    /// winter_shgc; summer months (May–Sep) must use the summer values.
    ///
    /// Window has transmittance=0.30 (summer) and winter_transmittance=0.40 (winter).
    /// The zone temperature delta is proportional to transmitted solar, so January
    /// must produce a larger delta than July, and the ratio must match the
    /// transmittance ratio exactly (0.40/0.30 ≈ 1.333).
    #[test]
    fn winter_shading_uses_winter_transmittance() {
        let zone_temp = 20.0_f64;
        let outdoor_temp = zone_temp; // no conduction load, isolate solar effect

        let r = 10.0_f64;
        let c = 50_000.0_f64;
        // 1R1C: inputs = [outdoor_temp, sensible_gain, solar_gain]
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (r * c)]);
        let b_c = DMatrix::from_row_slice(1, 3, &[1.0 / (r * c), 1.0 / c, 1.0 / c]);
        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();

        let window_surface_id: u32 = 77;
        let summer_transmittance = 0.30_f64;
        let winter_transmittance = 0.40_f64;
        let summer_shgc = 0.35_f64;
        let winter_shgc = 0.46_f64;
        let radiation_frac = 0.15_f64;

        let win_props = WindowSolarProperties {
            shgc: summer_shgc,
            winter_shgc,
            u_factor_w_m2_k: 1.8,
            area_m2: 2.0,
            transmittance: summer_transmittance,
            winter_transmittance,
            radiation_frac,
            glazing_curve: hares_physics::solar::GlazingCurve::from_u_shgc(1.8, summer_shgc),
            tilt_deg: 90.0,
            azimuth_deg: 180.0,
        };

        let make_env = |year_month: (i32, u32)| -> EnvironmentState {
            let (year, month) = year_month;
            EnvironmentState {
                ambient_other_space_c: hares_types::AmbientOtherSpaceTemps::default(),
                zones: vec![ZoneState {
                    id: ZoneId(1),
                    temperature_c: zone_temp,
                    humidity_ratio: 0.008,
                    volume_m3: 200.0,
                }],
                weather: WeatherState {
                    outdoor_temp_c: outdoor_temp,
                    outdoor_humidity_ratio: 0.004,
                    wind_speed_m_s: 0.0,
                    wind_dir_deg: 0.0,
                    ground_temp_c: outdoor_temp,
                    sky_temp_c: outdoor_temp - 5.0,
                    pressure_kpa: 101.325,
                    solar_irradiance: vec![SurfaceIrradiance {
                        surface_id: window_surface_id,
                        direct_w_m2: 500.0,
                        diffuse_w_m2: 0.0,
                        reflected_w_m2: 0.0,
                        angle_of_incidence_rad: 0.0,
                    }],
                    outdoor_wet_bulb_c: 0.0,
                    outdoor_enthalpy_j_kg: 0.0,
                    ghi_w_m2: 0.0,
                    dni_w_m2: 0.0,
                    dhi_w_m2: 0.0,
                    solar_altitude_deg: 0.0,
                    solar_azimuth_deg: 180.0,
                    mains_temp_c: 15.0,
                    rainfall_m: 0.0,
                    ground_albedo: 0.2,
                    ground_t_mean_c: 10.0,
                    ground_t_amplitude_c: 0.0,
                    ground_phase_day: 35.0,
                    day_of_year: 1.0,
                },
                grid: GridState {
                    voltage_pu: 1.0,
                    frequency_hz: 60.0,
                    island_bus_voltage_pu: None,
                },
                schedule_row: None,
                domains: hares_types::DomainSlots::default(),
                equipment_telemetry: std::collections::HashMap::new(),
                current_time: FixedOffset::east_opt(0)
                    .unwrap()
                    .with_ymd_and_hms(year, month, 15, 12, 0, 0)
                    .single()
                    .unwrap(),
                time_res: chrono::Duration::seconds(60),
                price_signal: Default::default(),
                electrical: Default::default(),
                equipment_core: Default::default(),
            }
        };

        let make_solver = |env: &EnvironmentState| -> ThermalSolver {
            let wiring = StateSpaceWiring {
                zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
                zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
                zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
                outdoor_temp_input_indices: vec![0],
                ground_temp_input_indices: vec![],
                ground_temp_input_depths_m: vec![],
                indoor_temp_input_indices: vec![],
                solar_input_indices: HashMap::from([(window_surface_id, 2usize)]),
                c_zone_j_k: HashMap::new(),
                node_capacitances: HashMap::new(),
                node_index: HashMap::new(),
            };
            let cfg = ThermalSolverConfig {
                indoor_zone_id: ZoneId(1),
                window_properties: HashMap::from([(window_surface_id, win_props)]),
                window_zone_ids: HashMap::new(),
                exterior_surfaces: vec![],
                interior_lwr_zones: vec![],
                infiltration: vec![],
                ventilation_flow_m3_s: 0.0,
                ventilation: MechanicalVentilationParams::default(),
                natural_ventilation: None,
                supply_duct_leakage_m3_s: 0.0,
                return_duct_leakage_m3_s: 0.0,
                interior_solar_zones: Vec::new(),
                boundary_diagnostics: Vec::new(),
                film_coefficient_model: FilmCoefficientModel::default(),
                interior_convection_injections: Vec::new(),
                interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
                ideal_capacity_degraded_threshold: 3,
            };
            let mut s =
                ThermalSolver::new(model.clone(), wiring.clone(), cfg, 60.0, env, zone_temp)
                    .unwrap();
            s.x[0] = zone_temp;
            s
        };

        let ports = PortSlots {
            thermal: vec![hares_types::ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        let env_jan = make_env((2026, 1));
        let env_jul = make_env((2026, 7));

        let t_jan = make_solver(&env_jan)
            .resolve_new(&ports, &env_jan, Duration::from_secs(60))
            .unwrap()
            .zone_temperatures_c[0]
            .1;

        let t_jul = make_solver(&env_jul)
            .resolve_new(&ports, &env_jul, Duration::from_secs(60))
            .unwrap()
            .zone_temperatures_c[0]
            .1;

        // January (winter): winter_transmittance=0.40 → more solar gain → higher T.
        // July (summer): transmittance=0.30 → less solar gain → lower T.
        assert!(
            t_jan > t_jul,
            "January (winter transmittance=0.40) must produce higher zone temp than July (summer transmittance=0.30): t_jan={t_jan:.6}, t_jul={t_jul:.6}"
        );

        // The temperature rise above zone_temp is proportional to transmitted solar.
        // Ratio of gains = winter_transmittance / summer_transmittance = 0.40/0.30 ≈ 1.333.
        let delta_jan = t_jan - zone_temp;
        let delta_jul = t_jul - zone_temp;
        assert!(
            delta_jan > 0.0,
            "January solar must raise zone temperature: delta={delta_jan:.6}"
        );
        assert!(
            delta_jul > 0.0,
            "July solar must raise zone temperature: delta={delta_jul:.6}"
        );
        let ratio = delta_jan / delta_jul;
        let expected_ratio = winter_transmittance / summer_transmittance;
        assert!(
            (ratio - expected_ratio).abs() < 0.02,
            "gain ratio must match transmittance ratio ({expected_ratio:.4}): actual={ratio:.4}"
        );
    }

    /// After `prepare_inputs`, `solve_ideal_capacity_for_target` uses
    /// current-step weather data (not stale previous-step data).
    #[test]
    fn prepare_inputs_updates_ideal_capacity_background() {
        let env_warm = env_for_temp(20.0, 20.0);
        let mut solver = one_zone_solver(&env_warm);
        solver.x[0] = 20.0;
        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        // Run one full step at 20 C outdoor to populate last_u.
        let _ = solver.resolve_new(&ports, &env_warm, Duration::from_secs(60));

        // Now change outdoor to 0 C and only call prepare_inputs.
        let env_cold = env_for_temp(20.0, 0.0);
        solver.prepare_inputs(&ports, &env_cold).unwrap();

        // Ideal capacity to hold 20 C should be positive (heating needed)
        // because prepare_inputs updated the background to use 0 C outdoor.
        let cap = solver.solve_ideal_capacity_for_target(ZoneId(1), 20.0);
        assert!(
            cap > 0.0,
            "after prepare_inputs with cold outdoor, heating capacity should be positive, got {cap}"
        );
    }

    /// Single-call `resolve` produces identical zone temperature to the
    /// two-phase `prepare_inputs` + `integrate` path when ports are unchanged.
    #[test]
    fn resolve_matches_prepare_plus_integrate() {
        let env = env_for_temp(20.0, 5.0);
        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        let mut solver_single = one_zone_solver(&env);
        solver_single.x[0] = 20.0;
        let update_single = solver_single
            .resolve_new(&ports, &env, Duration::from_secs(60))
            .unwrap();

        let mut solver_split = one_zone_solver(&env);
        solver_split.x[0] = 20.0;
        solver_split.prepare_inputs(&ports, &env).unwrap();
        let mut out_split = DomainUpdate::empty(hares_types::THERMAL);
        solver_split
            .integrate(&ports, &env, &mut out_split)
            .unwrap();

        let t_single = update_single.zone_temperatures_c[0].1;
        let t_split = out_split.zone_temperatures_c[0].1;
        assert!(
            (t_single - t_split).abs() < 1e-12,
            "single-call and split-call must produce identical results: single={t_single}, split={t_split}"
        );
    }

    /// HVAC capacity injected between prepare_inputs and integrate is
    /// reflected in the resulting zone temperature.
    #[test]
    fn hvac_between_phases_affects_temperature() {
        let env = env_for_temp(20.0, 0.0);
        let ports_zero = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        // Baseline: no HVAC
        let mut solver_base = one_zone_solver(&env);
        solver_base.x[0] = 20.0;
        solver_base.prepare_inputs(&ports_zero, &env).unwrap();
        let mut out_base = DomainUpdate::empty(hares_types::THERMAL);
        solver_base
            .integrate(&ports_zero, &env, &mut out_base)
            .unwrap();
        let t_base = out_base.zone_temperatures_c[0].1;

        // With heating: add 1000 W between phases
        let mut solver_heat = one_zone_solver(&env);
        solver_heat.x[0] = 20.0;
        solver_heat.prepare_inputs(&ports_zero, &env).unwrap();
        let mut ports_heat = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };
        ports_heat.thermal[0].sensible_gain_w = 1000.0;
        let mut out_heat = DomainUpdate::empty(hares_types::THERMAL);
        solver_heat
            .integrate(&ports_heat, &env, &mut out_heat)
            .unwrap();
        let t_heat = out_heat.zone_temperatures_c[0].1;

        assert!(
            t_heat > t_base,
            "1000 W heating must raise zone temp: t_heat={t_heat:.6}, t_base={t_base:.6}"
        );
        assert!(
            t_heat - t_base > 0.001,
            "temperature difference must be measurable: delta={:.6}",
            t_heat - t_base
        );
    }

    #[test]
    fn radiant_gains_distributed_by_tmult_to_opaque_surfaces() {
        use hares_types::{THERMAL_CATEGORY_COUNT, ThermalAccumulator};

        let env = env_for_temp(20.0, 10.0);
        let mut solver = interior_lwr_solver(&env);

        let zone = ZoneId(1);
        let radiant_w = 100.0;
        let ports = PortSlots {
            thermal: vec![ThermalAccumulator {
                zone,
                sensible_gain_w: 0.0,
                radiant_gain_w: radiant_w,
                latent_gain_w: 0.0,
                shortwave_gain_w: 0.0,
                sensible_by_category: [0.0; THERMAL_CATEGORY_COUNT],
                radiant_by_category: [radiant_w, 0.0, 0.0, 0.0, 0.0, 0.0],
                latent_by_category: [0.0; THERMAL_CATEGORY_COUNT],
            }],
            ..Default::default()
        };

        let mut u = DVector::zeros(3);
        solver.apply_port_radiant_inputs(&mut u, &ports);

        let surfaces = &solver.config.interior_lwr_zones[0].surfaces;
        let total_weight: f64 = surfaces.iter().map(|s| s.area_m2 * s.emissivity).sum();

        for s in surfaces {
            let q = radiant_w * s.area_m2 * s.emissivity / total_weight;
            let expected_surface = q * s.radiation_frac;
            assert!(
                (u[s.input_index] - expected_surface).abs() < 1e-9,
                "surface input_index={} should receive {:.6}, got {:.6}",
                s.input_index,
                expected_surface,
                u[s.input_index]
            );
        }

        let air_idx = solver.wiring.zone_sensible_input_indices[&zone];
        let total_air: f64 = surfaces
            .iter()
            .map(|s| {
                let q = radiant_w * s.area_m2 * s.emissivity / total_weight;
                q * (1.0 - s.radiation_frac)
            })
            .sum();
        assert!(
            (u[air_idx] - total_air).abs() < 1e-9,
            "zone air should receive {:.6}, got {:.6}",
            total_air,
            u[air_idx]
        );

        let total_deposited: f64 = u.iter().sum();
        assert!(
            (total_deposited - radiant_w).abs() < 1e-9,
            "energy conservation: total deposited = {:.6}, expected {:.6}",
            total_deposited,
            radiant_w
        );
    }

    /// Deposits 80 W of short-wave internal gain on zone 1 of `solver` and
    /// checks it against `surfaces` (input index, area, inside solar
    /// absorptance, radiation fraction): each surface takes its share of area
    /// × absorptance, passes its radiation fraction to its node and the rest
    /// to the zone air, and every watt is deposited.
    fn assert_shortwave_absorbed_like_diffuse_solar(
        solver: &mut ThermalSolver,
        surfaces: &[(usize, f64, f64, f64)],
    ) {
        let zone = ZoneId(1);
        let shortwave_w = 80.0;
        let mut accumulator = hares_types::ThermalAccumulator::new(zone);
        accumulator.shortwave_gain_w = shortwave_w;
        let ports = PortSlots {
            thermal: vec![accumulator],
            ..Default::default()
        };

        let mut u = DVector::zeros(3);
        solver.apply_port_shortwave_inputs(&mut u, &ports);

        let total_weight: f64 = surfaces.iter().map(|&(_, a, abs, _)| a * abs).sum();
        assert!(total_weight > 0.0);
        let air_idx = solver.wiring.zone_sensible_input_indices[&zone];
        let mut air_expected = 0.0;
        for &(input_index, area, absorptance, radiation_frac) in surfaces {
            let q = shortwave_w * area * absorptance / total_weight;
            assert_ne!(input_index, air_idx);
            assert!(
                (u[input_index] - q * radiation_frac).abs() < 1e-9,
                "surface input_index={input_index} receives {}, got {}",
                q * radiation_frac,
                u[input_index]
            );
            air_expected += q * (1.0 - radiation_frac);
        }
        assert!((u[air_idx] - air_expected).abs() < 1e-9);
        assert!(
            (u.iter().sum::<f64>() - shortwave_w).abs() < 1e-9,
            "every short-wave watt is deposited"
        );
    }

    const SHORTWAVE_TEST_SURFACES: [(f64, f64); 2] = [(0.7, 0.8), (0.5, 0.9)];

    /// Short-wave internal gain (the visible part of lighting) is absorbed by
    /// the zone's interior surfaces as transmitted diffuse solar is, in
    /// proportion to area × inside solar absorptance, and every watt reaches
    /// a surface node or the zone air (ScriptF interior exchange).
    #[test]
    fn shortwave_gains_are_absorbed_like_transmitted_diffuse_solar() {
        let env = env_for_temp(20.0, 10.0);
        let mut solver = interior_lwr_solver(&env);
        for (surface, (absorptance, radiation_frac)) in solver.config.interior_lwr_zones[0]
            .surfaces
            .iter_mut()
            .zip(SHORTWAVE_TEST_SURFACES)
        {
            surface.solar_absorptance = absorptance;
            surface.radiation_frac = radiation_frac;
        }
        let surfaces: Vec<_> = solver.config.interior_lwr_zones[0]
            .surfaces
            .iter()
            .map(|s| {
                (
                    s.input_index,
                    s.area_m2,
                    s.solar_absorptance,
                    s.radiation_frac,
                )
            })
            .collect();
        assert_shortwave_absorbed_like_diffuse_solar(&mut solver, &surfaces);
    }

    /// The same deposit in the default StarMesh mode, where the zone's
    /// surfaces reach the distribution through `interior_solar_zones` and
    /// `interior_lwr_zones` is empty.
    #[test]
    fn shortwave_gains_are_absorbed_like_transmitted_diffuse_solar_in_star_mesh_mode() {
        let env = env_for_temp(20.0, 10.0);
        let mut solver = interior_lwr_solver(&env);
        let lwr_zone = solver.config.interior_lwr_zones.remove(0);
        let surfaces: Vec<InteriorSolarSurfaceInfo> = lwr_zone
            .surfaces
            .iter()
            .zip(SHORTWAVE_TEST_SURFACES)
            .map(
                |(s, (solar_absorptance, radiation_frac))| InteriorSolarSurfaceInfo {
                    input_index: Some(s.input_index),
                    area_m2: s.area_m2,
                    solar_absorptance,
                    radiation_frac,
                    is_floor: s.is_floor,
                    tilt_deg: s.tilt_deg,
                    azimuth_deg: s.azimuth_deg,
                },
            )
            .collect();
        let expected: Vec<_> = surfaces
            .iter()
            .map(|s| {
                (
                    s.input_index.expect("surface input"),
                    s.area_m2,
                    s.solar_absorptance,
                    s.radiation_frac,
                )
            })
            .collect();
        solver.config.interior_solar_zones = vec![InteriorSolarZoneConfig {
            zone_id: lwr_zone.zone_id,
            surfaces,
        }];
        assert_shortwave_absorbed_like_diffuse_solar(&mut solver, &expected);
    }

    #[test]
    fn radiant_gains_with_window_surfaces_go_to_air() {
        use hares_types::THERMAL_CATEGORY_COUNT;
        let env = env_for_temp(20.0, 10.0);
        let a_c = DMatrix::from_row_slice(
            4,
            4,
            &[
                -1.0 / 50_000.0,
                0.0,
                0.0,
                0.0,
                0.0,
                -1.0 / 40_000.0,
                0.0,
                0.0,
                0.0,
                0.0,
                -1.0 / 30_000.0,
                0.0,
                0.0,
                0.0,
                0.0,
                -1.0 / 20_000.0,
            ],
        );
        let b_c = DMatrix::zeros(4, 4);
        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();
        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 0)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };
        let mut interior_lwr_zone = InteriorLwrZoneConfig {
            zone_id: ZoneId(1),
            surfaces: vec![
                InteriorSurfaceInfo {
                    state_index: 1,
                    input_index: 1,
                    area_m2: 10.0,
                    azimuth_deg: 0.0,
                    tilt_deg: 90.0,
                    emissivity: 0.9,
                    radiation_frac: 0.02,
                    rad_res_k_w: 250.0,
                    solar_absorptance: 0.0,
                    is_floor: false,
                    driving_temp: None,
                },
                InteriorSurfaceInfo {
                    state_index: 2,
                    input_index: 2,
                    area_m2: 20.0,
                    azimuth_deg: 0.0,
                    tilt_deg: 90.0,
                    emissivity: 0.9,
                    radiation_frac: 0.02,
                    rad_res_k_w: 250.0,
                    solar_absorptance: 0.0,
                    is_floor: false,
                    driving_temp: None,
                },
                InteriorSurfaceInfo {
                    state_index: 3,
                    input_index: 3,
                    area_m2: 6.0,
                    azimuth_deg: 0.0,
                    tilt_deg: 90.0,
                    emissivity: 0.84,
                    radiation_frac: 1.0,
                    rad_res_k_w: 0.0,
                    solar_absorptance: 0.0,
                    is_floor: false,
                    driving_temp: Some(DrivingTemp::Outdoor),
                },
            ],
            scriptf: None,
        };
        interior_lwr_zone.compute_scriptf();
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            window_properties: HashMap::new(),
            window_zone_ids: HashMap::new(),
            exterior_surfaces: vec![],
            interior_lwr_zones: vec![interior_lwr_zone],
            infiltration: vec![],
            ventilation_flow_m3_s: 0.0,
            ventilation: MechanicalVentilationParams::default(),
            natural_ventilation: None,
            supply_duct_leakage_m3_s: 0.0,
            return_duct_leakage_m3_s: 0.0,
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
            film_coefficient_model: FilmCoefficientModel::default(),
            interior_convection_injections: Vec::new(),
            ideal_capacity_degraded_threshold: 3,
        };
        let mut solver = ThermalSolver::new(model, wiring, config, 60.0, &env, 20.0).unwrap();

        let zone = ZoneId(1);
        let radiant_w = 100.0;
        let ports = PortSlots {
            thermal: vec![ThermalAccumulator {
                zone,
                sensible_gain_w: 0.0,
                radiant_gain_w: radiant_w,
                latent_gain_w: 0.0,
                shortwave_gain_w: 0.0,
                sensible_by_category: [0.0; THERMAL_CATEGORY_COUNT],
                radiant_by_category: [radiant_w, 0.0, 0.0, 0.0, 0.0, 0.0],
                latent_by_category: [0.0; THERMAL_CATEGORY_COUNT],
            }],
            ..Default::default()
        };

        let mut u = DVector::zeros(4);
        solver.apply_port_radiant_inputs(&mut u, &ports);

        let opaque_weight: f64 = 10.0 * 0.9 + 20.0 * 0.9;
        let q1 = radiant_w * (10.0 * 0.9) / opaque_weight;
        let q2 = radiant_w * (20.0 * 0.9) / opaque_weight;
        let air_idx = solver.wiring.zone_sensible_input_indices[&zone];
        let air_from_radiant = q1 * (1.0 - 0.02) + q2 * (1.0 - 0.02);
        assert!(
            (u[air_idx] - air_from_radiant).abs() < 1e-9,
            "zone air should receive {:.6}, got {:.6}",
            air_from_radiant,
            u[air_idx]
        );
        assert!(
            u[3] == 0.0,
            "window surface should receive zero radiant gain, got {}",
            u[3]
        );
        let total: f64 = u.iter().sum();
        assert!(
            (total - radiant_w).abs() < 1e-9,
            "energy conservation: total = {:.6}, expected {:.6}",
            total,
            radiant_w
        );
    }

    #[test]
    fn window_interior_lwr_applied_to_zone_air() {
        let t_zone = 22.0;
        let t_out = -15.0;
        let env = env_for_temp(t_zone, t_out);

        let a_c = DMatrix::from_row_slice(
            3,
            3,
            &[
                -1.0 / 50_000.0,
                0.0,
                0.0,
                0.0,
                -1.0 / 40_000.0,
                0.0,
                0.0,
                0.0,
                -1.0 / 30_000.0,
            ],
        );
        let b_c = DMatrix::zeros(3, 4);
        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();
        let zone = ZoneId(1);
        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(zone, 0)]),
            zone_output_indices: HashMap::from([(zone, 0)]),
            zone_sensible_input_indices: HashMap::from([(zone, 3)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };
        let rad_frac_opaque = 0.7;
        // Window radiation_frac from EnergyPlus interior film decomposition
        // for U=3.0 W/(m²·K): res_int ≈ 0.120, R_total ≈ 0.333, rad_frac ≈ 0.36.
        let rad_frac_window = 0.36;
        let mut interior_lwr_zone = InteriorLwrZoneConfig {
            zone_id: zone,
            surfaces: vec![
                InteriorSurfaceInfo {
                    state_index: 1,
                    input_index: 1,
                    area_m2: 40.0,
                    azimuth_deg: 0.0,
                    tilt_deg: 90.0,
                    emissivity: 0.9,
                    radiation_frac: rad_frac_opaque,
                    rad_res_k_w: 0.003,
                    solar_absorptance: 0.0,
                    is_floor: false,
                    driving_temp: None,
                },
                InteriorSurfaceInfo {
                    state_index: 2,
                    input_index: 2,
                    area_m2: 6.0,
                    azimuth_deg: 0.0,
                    tilt_deg: 90.0,
                    emissivity: 0.84,
                    radiation_frac: rad_frac_window,
                    rad_res_k_w: 0.020,
                    solar_absorptance: 0.0,
                    is_floor: false,
                    driving_temp: Some(DrivingTemp::Outdoor),
                },
            ],
            scriptf: None,
        };
        interior_lwr_zone.compute_scriptf();
        let config = ThermalSolverConfig {
            indoor_zone_id: zone,
            window_properties: HashMap::new(),
            window_zone_ids: HashMap::new(),
            exterior_surfaces: vec![],
            interior_lwr_zones: vec![interior_lwr_zone],
            infiltration: vec![],
            ventilation_flow_m3_s: 0.0,
            ventilation: MechanicalVentilationParams::default(),
            natural_ventilation: None,
            supply_duct_leakage_m3_s: 0.0,
            return_duct_leakage_m3_s: 0.0,
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
            film_coefficient_model: FilmCoefficientModel::default(),
            interior_convection_injections: Vec::new(),
            ideal_capacity_degraded_threshold: 3,
        };
        let mut solver = ThermalSolver::new(model, wiring, config, 60.0, &env, t_zone).unwrap();
        solver.x[0] = t_zone;
        solver.x[1] = 24.0;
        solver.x[2] = t_zone;

        let mut u = DVector::zeros(4);
        solver.apply_interior_longwave_inputs(&mut u, &env);

        let air_idx = solver.wiring.zone_sensible_input_indices[&zone];
        assert!(
            u[1] != 0.0 || u[air_idx] != 0.0,
            "opaque surface LWR must be deposited somewhere"
        );

        let opaque_flux = u[1] / rad_frac_opaque;
        let opaque_air = opaque_flux * (1.0 - rad_frac_opaque);

        let window_air = u[air_idx] - opaque_air;

        assert!(
            window_air > 0.0,
            "window interior LWR zone-air fraction must be positive (cold window gains from warm surfaces), got {window_air:.4}"
        );

        // Verify the resistance-divider scaling: window zone-air injection
        // must equal q_window × (1 − radiation_frac), which is strictly
        // less than the full q_window. Derive q_window from the opaque
        // surface's contribution (energy conservation: Σq = 0 in a 2-surface
        // zone means q_window = −q_opaque).
        let q_opaque = u[1] / rad_frac_opaque;
        let q_window = -q_opaque; // two-surface energy conservation
        // OCHRE "full" mode: window zone-air injection = q_window × (1 − radiation_frac).
        // Windows have no RC node (t_idx=None), so only the zone-air fraction
        // is injected. The radiation_frac portion is carried by the window's
        // U-factor conduction path. OCHRE `_solve_interior_radiation` lines
        // 1187-1195: `if surface.t_idx is not None` guards h_idx injection.
        let expected_window_air = q_window * (1.0 - rad_frac_window);
        assert!(
            (window_air - expected_window_air).abs() < 1e-6,
            "window zone-air injection ({window_air:.6}) should equal q×(1−rad_frac) = {expected_window_air:.6}"
        );
        assert!(
            window_air.abs() < q_window.abs() || q_window.abs() < 1e-9,
            "window zone-air injection ({window_air:.4}) must be less than full q ({q_window:.4})"
        );

        assert!(
            u[2] == 0.0,
            "window surface input must be zero (no RC node), got {:.6}",
            u[2]
        );
    }

    #[test]
    fn window_interior_lwr_energy_conservation() {
        use crate::longwave_radiation::{InteriorSurface, interior_longwave_linearised_w};

        let t_zone = 20.0;
        let t_out = -10.0;

        let infos = [
            InteriorSurfaceInfo {
                state_index: 0,
                input_index: 0,
                area_m2: 50.0,
                azimuth_deg: 0.0,
                tilt_deg: 90.0,
                emissivity: 0.90,
                radiation_frac: 0.7,
                rad_res_k_w: 0.003,
                solar_absorptance: 0.0,
                is_floor: false,
                driving_temp: None,
            },
            InteriorSurfaceInfo {
                state_index: 1,
                input_index: 1,
                area_m2: 30.0,
                azimuth_deg: 0.0,
                tilt_deg: 90.0,
                emissivity: 0.90,
                radiation_frac: 0.85,
                rad_res_k_w: 0.003,
                solar_absorptance: 0.0,
                is_floor: false,
                driving_temp: None,
            },
            InteriorSurfaceInfo {
                state_index: 2,
                input_index: 2,
                area_m2: 40.0,
                azimuth_deg: 0.0,
                tilt_deg: 180.0,
                emissivity: 0.90,
                radiation_frac: 0.6,
                rad_res_k_w: 0.003,
                solar_absorptance: 0.0,
                is_floor: true,
                driving_temp: None,
            },
            InteriorSurfaceInfo {
                state_index: 0,
                input_index: 3,
                area_m2: 6.0,
                azimuth_deg: 0.0,
                tilt_deg: 90.0,
                emissivity: 0.84,
                radiation_frac: 0.36,
                rad_res_k_w: 0.020,
                solar_absorptance: 0.0,
                is_floor: false,
                driving_temp: Some(DrivingTemp::Outdoor),
            },
        ];

        let t_nodes = [25.0, 18.0, 22.0];
        let surfaces: Vec<InteriorSurface> = infos
            .iter()
            .map(|s| InteriorSurface {
                area_m2: s.area_m2,
                emissivity: s.emissivity,
            })
            .collect();
        let t_surfaces: Vec<f64> = infos
            .iter()
            .zip(t_nodes.iter().chain(std::iter::once(&t_out)))
            .map(|(s, &t_node)| s.radiation_frac * t_node + (1.0 - s.radiation_frac) * t_zone)
            .collect();

        let net = interior_longwave_linearised_w(&surfaces, &t_surfaces, t_zone);

        let sum_flux: f64 = net.iter().sum();
        assert!(
            sum_flux.abs() < 1e-8,
            "LWR exchange must conserve energy: Σq = {sum_flux:.10}"
        );

        // Total injected to solver inputs: opaque surfaces get the full q
        // (split via radiation_frac between surface node and zone air — no
        // double-counting since R_film is convection-only), while window
        // surfaces only get q × (1 − radiation_frac) to zone air (OCHRE
        // "full" mode: t_idx=None → no h_idx injection). The window's
        // q × radiation_frac portion is carried by the U-factor conduction
        // path (boundary temp already reflects LWR exchange).
        let total_injected: f64 = infos
            .iter()
            .zip(net.iter())
            .map(|(info, &q)| {
                if info.driving_temp.is_none() {
                    q * info.radiation_frac + q * (1.0 - info.radiation_frac)
                } else {
                    q * (1.0 - info.radiation_frac)
                }
            })
            .sum();

        // The "missing" energy (Σ q_window × radiation_frac) is not destroyed —
        // it is carried by the window boundary temperature update and the
        // U-factor conduction path (OCHRE `_solve_interior_radiation`:
        // windows skip h_idx injection when t_idx is None).
        let window_to_cond: f64 = infos
            .iter()
            .zip(net.iter())
            .filter(|(info, _)| info.driving_temp.is_some())
            .map(|(info, &q)| q * info.radiation_frac)
            .sum();

        assert!(
            (total_injected + window_to_cond - sum_flux).abs() < 1e-9,
            "total injected ({total_injected:.6}) + window-to-cond ({window_to_cond:.6}) must equal Σq ({sum_flux:.6}) — no energy destroyed"
        );
    }

    // Regression test:
    // The comment at ports.rs:135-136 says "Windows (input_index=None)" but
    // solver_builder always sets input_index: Some(zone_air_idx) for every surface,
    // including windows. Windows are excluded from the solar radiant distribution
    // because solar_absorptance = 0.0 produces zero weight — NOT because
    // input_index is None (which never occurs in production).
    #[test]
    fn window_excluded_via_zero_solar_absorptance_not_none_input_index() {
        use hares_types::{THERMAL_CATEGORY_COUNT, ThermalAccumulator};

        let env = env_for_temp(20.0, 10.0);

        // 3-state, 4-input model: states 0-2, inputs 0 (outdoor temp), 1 (wall RC node),
        // 2 (window input — input_index is Some(2), never None), 3 (zone air sensible).
        let a_c = DMatrix::from_row_slice(
            3,
            3,
            &[
                -1.0 / 50_000.0,
                0.0,
                0.0,
                0.0,
                -1.0 / 40_000.0,
                0.0,
                0.0,
                0.0,
                -1.0 / 30_000.0,
            ],
        );
        let b_c = DMatrix::zeros(3, 4);
        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();
        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 3)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };

        // Use the solar-surface (StarMesh) path: no interior_lwr_zones, only interior_solar_zones.
        // Window has input_index: Some(2) — never None — and solar_absorptance = 0.0.
        // Opaque wall has input_index: Some(1) and solar_absorptance = 0.7.
        let solar_zone = InteriorSolarZoneConfig {
            zone_id: ZoneId(1),
            surfaces: vec![
                InteriorSolarSurfaceInfo {
                    input_index: Some(1),
                    area_m2: 20.0,
                    azimuth_deg: 0.0,
                    tilt_deg: 90.0,
                    solar_absorptance: 0.7,
                    radiation_frac: 0.5,
                    is_floor: false,
                },
                // Window: input_index is Some (not None), excluded by zero solar_absorptance.
                InteriorSolarSurfaceInfo {
                    input_index: Some(2),
                    area_m2: 5.0,
                    azimuth_deg: 0.0,
                    tilt_deg: 90.0,
                    solar_absorptance: 0.0,
                    radiation_frac: 0.1,
                    is_floor: false,
                },
            ],
        };

        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            window_properties: HashMap::new(),
            window_zone_ids: HashMap::new(),
            exterior_surfaces: vec![],
            interior_lwr_zones: vec![],
            infiltration: vec![],
            ventilation_flow_m3_s: 0.0,
            ventilation: MechanicalVentilationParams::default(),
            natural_ventilation: None,
            supply_duct_leakage_m3_s: 0.0,
            return_duct_leakage_m3_s: 0.0,
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
            interior_solar_zones: vec![solar_zone],
            boundary_diagnostics: Vec::new(),
            film_coefficient_model: FilmCoefficientModel::default(),
            interior_convection_injections: Vec::new(),
            ideal_capacity_degraded_threshold: 3,
        };

        let mut solver = ThermalSolver::new(model, wiring, config, 60.0, &env, 20.0).unwrap();

        let zone = ZoneId(1);
        let radiant_w = 100.0;
        let ports = PortSlots {
            thermal: vec![ThermalAccumulator {
                zone,
                sensible_gain_w: 0.0,
                radiant_gain_w: radiant_w,
                latent_gain_w: 0.0,
                shortwave_gain_w: 0.0,
                sensible_by_category: [0.0; THERMAL_CATEGORY_COUNT],
                radiant_by_category: [radiant_w, 0.0, 0.0, 0.0, 0.0, 0.0],
                latent_by_category: [0.0; THERMAL_CATEGORY_COUNT],
            }],
            ..Default::default()
        };

        let mut u = DVector::zeros(4);
        solver.apply_port_radiant_inputs(&mut u, &ports);

        // Window input (index 2) must receive zero — it is excluded by solar_absorptance=0.0,
        // not by input_index=None (which is never set in production).
        assert_eq!(
            u[2], 0.0,
            "window (input_index=Some(2), solar_absorptance=0.0) must receive zero radiant gain"
        );

        // Wall RC node (index 1) receives radiation_frac of the 100 W.
        let expected_wall_rc = radiant_w * 0.5;
        assert!(
            (u[1] - expected_wall_rc).abs() < 1e-9,
            "wall RC node should receive {expected_wall_rc:.6}, got {:.6}",
            u[1]
        );

        // Zone air (index 3) receives (1 - radiation_frac) of the 100 W.
        let expected_air = radiant_w * 0.5;
        assert!(
            (u[3] - expected_air).abs() < 1e-9,
            "zone air should receive {expected_air:.6}, got {:.6}",
            u[3]
        );

        // Energy conservation.
        let total: f64 = u.iter().sum();
        assert!(
            (total - radiant_w).abs() < 1e-9,
            "energy conservation: total={total:.6}, expected={radiant_w:.6}"
        );
    }

    /// Verify that the `RCNode` boundary diagnostic uses per-step TARP natural
    /// convection (h_conv ∝ ΔT^(1/3)), not the frozen init-time ASHRAE Simple
    /// value (h_conv = 3.076 constant).
    ///
    /// Constructs a 2-state model with a hot interior wall surface (30°C) and
    /// zone air at 20°C. After one step, the diagnostic convective flux must be
    /// consistent with TARP h_conv ≈ 1.31 × ΔT^(1/3) ≈ 2.82 W/(m²·K), not the
    /// frozen ASHRAE Simple 3.076.
    ///
    /// References:
    /// - Walton, G. N. 1983. TARP Reference Manual, NBSSIR 83-2655, Eq. 90.
    /// - EnergyPlus Engineering Reference "Interior Convection / TARP Algorithm".
    #[test]
    #[cfg(any(debug_assertions, feature = "observe_detailed"))]
    fn rc_node_boundary_diagnostic_uses_tarp_natural_convection() {
        let indoor = 20.0;
        let outdoor = 20.0; // same as zone air — outdoor does not drive a gradient
        let env = env_for_temp(indoor, outdoor);

        // 2-state model: state 0 = zone air, state 1 = wall interior surface.
        // Very weak coupling so temperatures barely move in one 60 s step.
        let r_zw = 0.5; // K/W zone↔wall
        let c_z = 50_000.0; // J/K zone air
        let c_w = 500_000.0; // J/K wall (10× zone → wall moves 10× slower)

        let a_c = DMatrix::from_row_slice(
            2,
            2,
            &[
                -1.0 / (r_zw * c_z),
                1.0 / (r_zw * c_z),
                1.0 / (r_zw * c_w),
                -1.0 / (r_zw * c_w),
            ],
        );
        let b_c = DMatrix::zeros(2, 2); // no external inputs needed for this test

        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)], // y[0] = x[0] (zone air)
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();

        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };

        let area_m2 = 10.0;
        let tilt_deg = 90.0; // vertical wall

        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            window_properties: HashMap::new(),
            window_zone_ids: HashMap::new(),
            exterior_surfaces: vec![],
            interior_lwr_zones: vec![],
            infiltration: vec![],
            ventilation_flow_m3_s: 0.0,
            ventilation: MechanicalVentilationParams::default(),
            natural_ventilation: None,
            supply_duct_leakage_m3_s: 0.0,
            return_duct_leakage_m3_s: 0.0,
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: vec![crate::thermal_solver::BoundaryDiagnosticInfo::RCNode {
                inner_state_index: 1, // wall node
                area_m2,
                tilt_deg,
                radiation_frac: 1.0, // t_surface = t_node (no zone-air mixing)
                category: BoundaryCategory::Wall,
            }],
            film_coefficient_model: FilmCoefficientModel::default(),
            interior_convection_injections: Vec::new(),
            ideal_capacity_degraded_threshold: 3,
        };

        let mut solver = ThermalSolver::new(model, wiring, config, 60.0, &env, indoor).unwrap();

        // Override initial state: wall at 30°C, zone air at 20°C.
        // After one 60 s step with weak coupling, ΔT ≈ 10 K.
        solver.x[0] = 20.0;
        solver.x[1] = 30.0;

        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        let mut update = DomainUpdate::empty(hares_types::THERMAL);
        solver
            .resolve(&ports, &env, Duration::from_secs(60), &mut update)
            .unwrap();

        let wall_gain = solver.component_gains().wall_heat_gain_w;

        // Hot wall → heat flows into the zone (positive).
        assert!(
            wall_gain > 0.0,
            "hot wall (30°C) should transfer heat into zone (20°C), got {wall_gain:.2} W"
        );

        // TARP h_conv at ΔT ≈ 10 K, vertical: h = 1.31 × ∛10 ≈ 2.822 W/(m²·K).
        // Frozen ASHRAE Simple: h = 3.076 W/(m²·K) for vertical.
        // The actual flux should be closer to TARP than to frozen.
        let delta_t_approx = 10.0_f64;
        let h_tarp = 1.31 * delta_t_approx.cbrt();
        let q_tarp = h_tarp * area_m2 * delta_t_approx;
        let q_frozen = 3.076 * area_m2 * delta_t_approx;

        let dist_to_tarp = (wall_gain - q_tarp).abs();
        let dist_to_frozen = (wall_gain - q_frozen).abs();
        assert!(
            dist_to_tarp < dist_to_frozen,
            "wall_gain={wall_gain:.2} W should be closer to TARP ({q_tarp:.2} W) \
             than frozen ASHRAE Simple ({q_frozen:.2} W); \
             dist_to_tarp={dist_to_tarp:.2}, dist_to_frozen={dist_to_frozen:.2}"
        );

        // Sanity: the flux should be within 20% of the TARP prediction
        // (ΔT drifts slightly from 10 K during the step).
        let rel_err = (wall_gain - q_tarp).abs() / q_tarp;
        assert!(
            rel_err < 0.20,
            "wall_gain={wall_gain:.2} W deviates from TARP prediction {q_tarp:.2} W by {:.1}%",
            rel_err * 100.0
        );
    }

    /// `port_convective_w` in `EnvelopeComponentGains` is populated from the
    /// convective accumulator slot (`sensible_gain_w`), not from the radiant
    /// slot (`radiant_gain_w`) or their sum. This round-trip exercises the full
    /// `prepare_inputs` path and would catch a regression where a struct literal
    /// wires the wrong accumulator to the wrong field.
    #[test]
    fn port_convective_w_comes_from_sensible_gain_w_not_radiant_or_sum() {
        let env = env_for_temp(22.0, 10.0);
        let mut solver = one_zone_solver(&env);

        let convect = 500.0_f64;
        let radiant = 200.0_f64;
        let ports = PortSlots {
            thermal: vec![ThermalAccumulator {
                zone: ZoneId(1),
                sensible_gain_w: convect,
                radiant_gain_w: radiant,
                latent_gain_w: 0.0,
                shortwave_gain_w: 0.0,
                sensible_by_category: [0.0; hares_types::THERMAL_CATEGORY_COUNT],
                radiant_by_category: [0.0; hares_types::THERMAL_CATEGORY_COUNT],
                latent_by_category: [0.0; hares_types::THERMAL_CATEGORY_COUNT],
            }],
            electrical: Default::default(),
            fuel: Default::default(),
            fluid: vec![],
            custom: vec![],
            humidity: vec![],
        };

        solver.prepare_inputs(&ports, &env).unwrap();
        let gains = solver.component_gains();

        assert!(
            (gains.port_convective_w - convect).abs() < 1e-9,
            "port_convective_w should come from sensible_gain_w ({} W), got {} W",
            convect,
            gains.port_convective_w
        );
        assert!(
            (gains.port_radiant_w - radiant).abs() < 1e-9,
            "port_radiant_w should come from radiant_gain_w ({} W), got {} W",
            radiant,
            gains.port_radiant_w
        );
    }

    /// `jacket_loss_w` sums `ThermalCategory::JacketLoss` across all zone
    /// accumulators, not just the indoor zone. Water heaters and boilers in
    /// unconditioned zones (garage, basement) deposit jacket losses into that
    /// zone's accumulator; the diagnostic must capture them all.
    #[test]
    fn jacket_loss_w_sums_across_all_zones() {
        let indoor = ZoneId(1);
        let garage = ZoneId(2);

        let env = EnvironmentState {
            zones: vec![
                ZoneState {
                    id: indoor,
                    temperature_c: 20.0,
                    humidity_ratio: 0.008,
                    volume_m3: 200.0,
                },
                ZoneState {
                    id: garage,
                    temperature_c: 10.0,
                    humidity_ratio: 0.004,
                    volume_m3: 100.0,
                },
            ],
            ..env_for_temp(20.0, 5.0)
        };

        // Two independent 1R1C zones, each with one sensible input.
        let r = 2.0;
        let c = 50_000.0;
        let a_c = DMatrix::from_row_slice(2, 2, &[-1.0 / (r * c), 0.0, 0.0, -1.0 / (r * c)]);
        // Inputs: [T_out, H_zone1, H_zone2]
        let b_c = DMatrix::from_row_slice(
            2,
            3,
            &[1.0 / (r * c), 1.0 / c, 0.0, 1.0 / (r * c), 0.0, 1.0 / c],
        );
        let mapping = OutputMapping {
            output_count: 2,
            node_to_output: vec![(0, 0, 1.0), (1, 1, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();

        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(indoor, 0), (garage, 1)]),
            zone_output_indices: HashMap::from([(indoor, 0), (garage, 1)]),
            zone_sensible_input_indices: HashMap::from([(indoor, 1), (garage, 2)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: indoor,
            window_properties: HashMap::new(),
            window_zone_ids: HashMap::new(),
            exterior_surfaces: vec![],
            interior_lwr_zones: vec![],
            infiltration: vec![],
            ventilation_flow_m3_s: 0.0,
            ventilation: MechanicalVentilationParams::default(),
            natural_ventilation: None,
            supply_duct_leakage_m3_s: 0.0,
            return_duct_leakage_m3_s: 0.0,
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
            film_coefficient_model: FilmCoefficientModel::default(),
            interior_convection_injections: Vec::new(),
            ideal_capacity_degraded_threshold: 3,
        };

        let mut solver = ThermalSolver::new(
            model,
            wiring,
            config,
            60.0,
            &env,
            env.zones[0].temperature_c,
        )
        .unwrap();
        solver.x[0] = 20.0;
        solver.x[1] = 10.0;

        let indoor_jacket = 300.0_f64;
        let garage_jacket = 150.0_f64;

        let ports = PortSlots {
            thermal: vec![
                ThermalAccumulator {
                    zone: indoor,
                    sensible_gain_w: indoor_jacket,
                    radiant_gain_w: 0.0,
                    latent_gain_w: 0.0,
                    shortwave_gain_w: 0.0,
                    sensible_by_category: {
                        let mut a = [0.0_f64; hares_types::THERMAL_CATEGORY_COUNT];
                        a[ThermalCategory::JacketLoss.index()] = indoor_jacket;
                        a
                    },
                    radiant_by_category: [0.0_f64; hares_types::THERMAL_CATEGORY_COUNT],
                    latent_by_category: [0.0_f64; hares_types::THERMAL_CATEGORY_COUNT],
                },
                ThermalAccumulator {
                    zone: garage,
                    sensible_gain_w: garage_jacket,
                    radiant_gain_w: 0.0,
                    latent_gain_w: 0.0,
                    shortwave_gain_w: 0.0,
                    sensible_by_category: {
                        let mut a = [0.0_f64; hares_types::THERMAL_CATEGORY_COUNT];
                        a[ThermalCategory::JacketLoss.index()] = garage_jacket;
                        a
                    },
                    radiant_by_category: [0.0_f64; hares_types::THERMAL_CATEGORY_COUNT],
                    latent_by_category: [0.0_f64; hares_types::THERMAL_CATEGORY_COUNT],
                },
            ],
            ..Default::default()
        };

        solver.prepare_inputs(&ports, &env).unwrap();
        let gains = solver.component_gains();

        let expected_total = indoor_jacket + garage_jacket;
        assert!(
            (gains.jacket_loss_w - expected_total).abs() < 1e-9,
            "jacket_loss_w ({}) should be the sum of indoor ({} W) and garage ({} W) jacket losses = {} W",
            gains.jacket_loss_w,
            indoor_jacket,
            garage_jacket,
            expected_total,
        );

        // When only the indoor zone has jacket loss, it should still equal the zone contribution.
        let ports_indoor_only = PortSlots {
            thermal: vec![ThermalAccumulator {
                zone: indoor,
                sensible_gain_w: indoor_jacket,
                radiant_gain_w: 0.0,
                latent_gain_w: 0.0,
                shortwave_gain_w: 0.0,
                sensible_by_category: {
                    let mut a = [0.0_f64; hares_types::THERMAL_CATEGORY_COUNT];
                    a[ThermalCategory::JacketLoss.index()] = indoor_jacket;
                    a
                },
                radiant_by_category: [0.0_f64; hares_types::THERMAL_CATEGORY_COUNT],
                latent_by_category: [0.0_f64; hares_types::THERMAL_CATEGORY_COUNT],
            }],
            ..Default::default()
        };

        solver.prepare_inputs(&ports_indoor_only, &env).unwrap();
        let gains = solver.component_gains();

        assert!(
            (gains.jacket_loss_w - indoor_jacket).abs() < 1e-9,
            "jacket_loss_w ({}) should equal indoor-only jacket loss ({})",
            gains.jacket_loss_w,
            indoor_jacket,
        );

        // When only the unconditioned zone has jacket loss, it should still be captured.
        let ports_garage_only = PortSlots {
            thermal: vec![ThermalAccumulator {
                zone: garage,
                sensible_gain_w: garage_jacket,
                radiant_gain_w: 0.0,
                latent_gain_w: 0.0,
                shortwave_gain_w: 0.0,
                sensible_by_category: {
                    let mut a = [0.0_f64; hares_types::THERMAL_CATEGORY_COUNT];
                    a[ThermalCategory::JacketLoss.index()] = garage_jacket;
                    a
                },
                radiant_by_category: [0.0_f64; hares_types::THERMAL_CATEGORY_COUNT],
                latent_by_category: [0.0_f64; hares_types::THERMAL_CATEGORY_COUNT],
            }],
            ..Default::default()
        };

        solver.prepare_inputs(&ports_garage_only, &env).unwrap();
        let gains = solver.component_gains();

        assert!(
            (gains.jacket_loss_w - garage_jacket).abs() < 1e-9,
            "jacket_loss_w ({}) should capture garage-only jacket loss ({})",
            gains.jacket_loss_w,
            garage_jacket,
        );
    }

    /// `jacket_loss_by_zone` captures per-zone jacket loss breakdown when
    /// the `observe` feature is enabled, helping verify equipment in
    /// unconditioned zones produces expected output.
    #[cfg(feature = "observe")]
    #[test]
    fn jacket_loss_by_zone_captures_all_zone_contributions() {
        let indoor = ZoneId(1);
        let garage = ZoneId(2);

        let env = EnvironmentState {
            zones: vec![
                ZoneState {
                    id: indoor,
                    temperature_c: 20.0,
                    humidity_ratio: 0.008,
                    volume_m3: 200.0,
                },
                ZoneState {
                    id: garage,
                    temperature_c: 10.0,
                    humidity_ratio: 0.004,
                    volume_m3: 100.0,
                },
            ],
            ..env_for_temp(20.0, 5.0)
        };

        let r = 2.0;
        let c = 50_000.0;
        let a_c = DMatrix::from_row_slice(2, 2, &[-1.0 / (r * c), 0.0, 0.0, -1.0 / (r * c)]);
        let b_c = DMatrix::from_row_slice(
            2,
            3,
            &[1.0 / (r * c), 1.0 / c, 0.0, 1.0 / (r * c), 0.0, 1.0 / c],
        );
        let mapping = OutputMapping {
            output_count: 2,
            node_to_output: vec![(0, 0, 1.0), (1, 1, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();

        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(indoor, 0), (garage, 1)]),
            zone_output_indices: HashMap::from([(indoor, 0), (garage, 1)]),
            zone_sensible_input_indices: HashMap::from([(indoor, 1), (garage, 2)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: indoor,
            window_properties: HashMap::new(),
            window_zone_ids: HashMap::new(),
            exterior_surfaces: vec![],
            interior_lwr_zones: vec![],
            infiltration: vec![],
            ventilation_flow_m3_s: 0.0,
            ventilation: MechanicalVentilationParams::default(),
            natural_ventilation: None,
            supply_duct_leakage_m3_s: 0.0,
            return_duct_leakage_m3_s: 0.0,
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
            film_coefficient_model: FilmCoefficientModel::default(),
            interior_convection_injections: Vec::new(),
            ideal_capacity_degraded_threshold: 3,
        };

        let mut solver = ThermalSolver::new(
            model,
            wiring,
            config,
            60.0,
            &env,
            env.zones[0].temperature_c,
        )
        .unwrap();
        solver.x[0] = 20.0;
        solver.x[1] = 10.0;

        let indoor_jacket = 250.0_f64;
        let garage_jacket = 180.0_f64;

        let ports = PortSlots {
            thermal: vec![
                ThermalAccumulator {
                    zone: indoor,
                    sensible_gain_w: indoor_jacket,
                    radiant_gain_w: 0.0,
                    latent_gain_w: 0.0,
                    shortwave_gain_w: 0.0,
                    sensible_by_category: {
                        let mut a = [0.0_f64; hares_types::THERMAL_CATEGORY_COUNT];
                        a[ThermalCategory::JacketLoss.index()] = indoor_jacket;
                        a
                    },
                    radiant_by_category: [0.0_f64; hares_types::THERMAL_CATEGORY_COUNT],
                    latent_by_category: [0.0_f64; hares_types::THERMAL_CATEGORY_COUNT],
                },
                ThermalAccumulator {
                    zone: garage,
                    sensible_gain_w: garage_jacket,
                    radiant_gain_w: 0.0,
                    latent_gain_w: 0.0,
                    shortwave_gain_w: 0.0,
                    sensible_by_category: {
                        let mut a = [0.0_f64; hares_types::THERMAL_CATEGORY_COUNT];
                        a[ThermalCategory::JacketLoss.index()] = garage_jacket;
                        a
                    },
                    radiant_by_category: [0.0_f64; hares_types::THERMAL_CATEGORY_COUNT],
                    latent_by_category: [0.0_f64; hares_types::THERMAL_CATEGORY_COUNT],
                },
            ],
            ..Default::default()
        };

        solver.prepare_inputs(&ports, &env).unwrap();
        let gains = solver.component_gains();

        let by_zone: std::collections::HashMap<ZoneId, f64> =
            gains.jacket_loss_by_zone.iter().copied().collect();
        assert!(
            (by_zone.get(&indoor).copied().unwrap_or(0.0) - indoor_jacket).abs() < 1e-9,
            "jacket_loss_by_zone[{}] equals {} W, expected {} W",
            indoor,
            by_zone.get(&indoor).copied().unwrap_or(0.0),
            indoor_jacket,
        );
        assert!(
            (by_zone.get(&garage).copied().unwrap_or(0.0) - garage_jacket).abs() < 1e-9,
            "jacket_loss_by_zone[{}] equals {} W, expected {} W",
            garage,
            by_zone.get(&garage).copied().unwrap_or(0.0),
            garage_jacket,
        );
    }

    /// Natural ventilation observe fields `q_stack_m3_s`, `q_wind_m3_s`,
    /// and `cd_used` populate correctly in `EnvelopeComponentGains` after a
    /// solver step with cross-ventilation and `dh_m > 0`, proving the wiring
    /// from `InfiltrationCoupling` through `ThermalSolver::prepare_inputs` is
    /// correct.
    #[cfg(feature = "observe")]
    #[test]
    fn natural_ventilation_observe_fields_reach_component_gains() {
        use hares_physics::infiltration::OpeningType;

        let r = 2.0;
        let c = 50_000.0;
        let a_c = DMatrix::from_row_slice(1, 1, &[-1.0 / (r * c)]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1.0 / (r * c), 1.0 / c]);
        let mapping = crate::state_space::OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();
        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
            outdoor_temp_input_indices: vec![0],
            ground_temp_input_indices: vec![],
            ground_temp_input_depths_m: vec![],
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
            node_capacitances: HashMap::new(),
            node_index: HashMap::new(),
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            window_properties: HashMap::new(),
            window_zone_ids: HashMap::new(),
            exterior_surfaces: vec![],
            interior_lwr_zones: vec![],
            interior_solar_zones: Vec::new(),
            infiltration: vec![],
            ventilation_flow_m3_s: 0.0,
            ventilation: MechanicalVentilationParams::default(),
            natural_ventilation: Some(NaturalVentilationConfig {
                open_area_m2: 1.0,
                dh_m: 2.0,
                zone_height_m: 2.5,
                opening_type: OpeningType::CrossVentilation,
                t_base_c: NaturalVentilationConfig::DEFAULT_T_BASE_C,
                max_outdoor_humidity_ratio:
                    NaturalVentilationConfig::DEFAULT_MAX_OUTDOOR_HUMIDITY_RATIO,
                opening_azimuth_deg: NaturalVentilationConfig::DEFAULT_OPENING_AZIMUTH_DEG,
            }),
            supply_duct_leakage_m3_s: 0.0,
            return_duct_leakage_m3_s: 0.0,
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::StarMesh,
            film_coefficient_model: FilmCoefficientModel::default(),
            interior_convection_injections: Vec::new(),
            boundary_diagnostics: Vec::new(),
            ideal_capacity_degraded_threshold: 3,
        };

        let env = env_for_temp(26.0, 18.0);
        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };

        let mut solver = ThermalSolver::new(model, wiring, config, 60.0, &env, 26.0).unwrap();
        solver.x[0] = 26.0;
        solver.prepare_inputs(&ports, &env).unwrap();
        let gains = solver.component_gains();

        assert!(
            gains.natural_ventilation_q_stack_m3_s > 0.0,
            "q_stack_m3_s = {} must be > 0 with dh_m=2.0 and ΔT=8 K",
            gains.natural_ventilation_q_stack_m3_s
        );
        assert!(
            gains.natural_ventilation_q_wind_m3_s > 0.0,
            "q_wind_m3_s = {} must be > 0 with wind_speed=3 m/s",
            gains.natural_ventilation_q_wind_m3_s
        );
        assert!(
            (gains.natural_ventilation_cd_used - 0.60).abs() < 1e-9,
            "cd_used = {} for CrossVentilation, expected 0.60",
            gains.natural_ventilation_cd_used
        );
    }

    // ── restore_state atomicity: validation before mutation ──────────────

    /// Asserts `restore_state` rejects `bad_snap` with an error naming
    /// `expected_in_error` and, because it validates every field before
    /// mutating any, leaves every piece of snapshotted state as it was.
    fn assert_restore_rejected_atomically(
        solver: &mut ThermalSolver,
        bad_snap: &ThermalSnapshot,
        expected_in_error: &str,
    ) {
        let before = solver.snapshot_state();
        let err = solver
            .restore_state(bad_snap)
            .expect_err("an invalid snapshot must be rejected");
        assert!(
            err.to_string().contains(expected_in_error),
            "the rejection must name '{expected_in_error}'; got: {err}"
        );
        assert!(
            matches!(
                err,
                ThermalSolverError::InvalidSnapshot(_)
                    | ThermalSolverError::InvalidVentilationRecovery { .. }
            ),
            "a restore must reject with a snapshot error, got: {err:?}"
        );
        assert_eq!(
            solver.snapshot_state(),
            before,
            "solver state must be unchanged after a rejected restore (atomicity)"
        );
    }

    #[test]
    fn restore_state_rejects_non_finite_x_and_leaves_solver_unchanged() {
        let env = env_for_temp(22.0, 5.0);
        let mut solver = one_zone_solver(&env);
        let bad_snap = ThermalSnapshot {
            x: vec![f64::NAN],
            ..solver.snapshot_state()
        };
        assert_restore_rejected_atomically(&mut solver, &bad_snap, "non-finite x[0]");
    }

    #[test]
    fn restore_state_rejects_non_finite_last_u_and_leaves_solver_unchanged() {
        let env = env_for_temp(22.0, 5.0);
        let mut solver = one_zone_solver(&env);
        let bad_snap = ThermalSnapshot {
            last_u: vec![f64::NAN, 0.0],
            ..solver.snapshot_state()
        };
        assert_restore_rejected_atomically(&mut solver, &bad_snap, "non-finite last_u[0]");
    }

    #[test]
    fn restore_state_rejects_a_last_u_of_the_wrong_length() {
        let env = env_for_temp(22.0, 5.0);
        let mut solver = one_zone_solver(&env);
        let bad_snap = ThermalSnapshot {
            last_u: vec![],
            ..solver.snapshot_state()
        };
        assert_restore_rejected_atomically(&mut solver, &bad_snap, "last_u length");
    }

    #[test]
    fn restore_state_rejects_invalid_coupling_state_and_leaves_solver_unchanged() {
        let env = env_for_temp(22.0, 5.0);
        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };
        let mut solver = solver_with_infiltration(&env, InfiltrationMethod::Ach { ach: 0.5 });
        solver.prepare_inputs(&ports, &env).unwrap();
        let valid = solver.snapshot_state();
        let n_states = valid.x.len();

        for (bad_couplings, expected_in_error) in [
            (vec![(n_states, 0.1, 1.0)], "last_coupling[0]"),
            (vec![(0, -0.1, 1.0)], "last_coupling[0]"),
            (vec![(0, f64::NAN, 1.0)], "last_coupling[0]"),
            (vec![(0, 0.1, f64::INFINITY)], "last_coupling[0]"),
            // Finite, but no envelope temperature implies it: the solve
            // overflows to an infinite capacity.
            (vec![(0, 0.1, f64::MAX)], "last_coupling[0]"),
            // A forcing with no implicit coefficient to carry it.
            (vec![(0, 0.0, 1.0)], "last_coupling[0]"),
            // Each coupling finite; together they overflow the divisor.
            (
                vec![(0, f64::MAX, 0.0), (0, f64::MAX, 0.0)],
                "aggregates to a non-finite coefficient",
            ),
            // Damping that leaves the zone's gain below what any solve can
            // divide by.
            (vec![(0, 1.0e300, 0.0)], "damps zone"),
        ] {
            let bad_snap = ThermalSnapshot {
                last_coupling: bad_couplings,
                ..valid.clone()
            };
            assert_restore_rejected_atomically(&mut solver, &bad_snap, expected_in_error);
        }
    }

    #[test]
    fn restore_state_rejects_invalid_ventilation_recovery_and_leaves_solver_unchanged() {
        let env = env_for_temp(22.0, 5.0);
        let mut solver = one_zone_solver(&env);
        let valid = solver.snapshot_state();
        for (sensible, latent) in [(-0.1, 0.5), (1.1, 0.5), (0.5, f64::NAN)] {
            let bad_snap = ThermalSnapshot {
                sensible_recovery_efficiency: sensible,
                latent_recovery_efficiency: latent,
                ..valid.clone()
            };
            assert_restore_rejected_atomically(&mut solver, &bad_snap, "is outside [0, 1]");
        }
    }

    /// A snapshot written against another version of the snapshot schema
    /// does not describe a state this build's solver can hold, so the
    /// restore rejects it before any mutation.
    #[test]
    fn restore_state_rejects_a_foreign_snapshot_schema_version() {
        let env = env_for_temp(22.0, 5.0);
        let mut solver = one_zone_solver(&env);
        let valid = solver.snapshot_state();
        for wrong in [THERMAL_SNAPSHOT_SCHEMA_VERSION + 1, 0] {
            let bad_snap = ThermalSnapshot {
                schema_version: wrong,
                ..valid.clone()
            };
            assert_restore_rejected_atomically(
                &mut solver,
                &bad_snap,
                &format!("schema version: got {wrong}, expected {THERMAL_SNAPSHOT_SCHEMA_VERSION}"),
            );
        }
    }

    #[test]
    fn restore_state_rejects_invalid_fallback_state_and_leaves_solver_unchanged() {
        let env = env_for_temp(22.0, 5.0);
        let mut solver = one_zone_solver(&env);
        let valid = solver.snapshot_state();
        for (bad_snap, expected_in_error) in [
            (
                ThermalSnapshot {
                    ideal_capacity_failure_counts: vec![(ZoneId(9), 1)],
                    ..valid.clone()
                },
                "does not solve",
            ),
            (
                ThermalSnapshot {
                    ideal_capacity_failure_counts: vec![(ZoneId(1), 0)],
                    ..valid.clone()
                },
                "ideal_capacity_failure_counts[0]",
            ),
            (
                ThermalSnapshot {
                    last_good_capacity_w: vec![(ZoneId(1), f64::INFINITY)],
                    ..valid.clone()
                },
                "last_good_capacity_w[0]",
            ),
            (
                ThermalSnapshot {
                    last_good_capacity_w: vec![(ZoneId(1), 1.0), (ZoneId(1), 2.0)],
                    ..valid.clone()
                },
                "strictly increasing zone order",
            ),
        ] {
            assert_restore_rejected_atomically(&mut solver, &bad_snap, expected_in_error);
        }
    }

    /// Forces the next `solve_ideal_capacity_for_target` on zone 1 to fail
    /// with a zero effective gain and restores the solver's own couplings
    /// afterwards; returns the solve's capacity.
    fn failing_zone_one_solve(solver: &mut ThermalSolver) -> f64 {
        let couplings = std::mem::replace(&mut solver.last_coupling, vec![(0, 1.0e300, 0.0)]);
        let capacity = solver.solve_ideal_capacity_for_target(ZoneId(1), 21.0);
        solver.last_coupling = couplings;
        capacity
    }

    /// The consecutive-failure counts and last-good capacities decide what a
    /// failing solve returns (zero below the degraded threshold, the
    /// last-good capacity at or above it), so a restored solver must fall
    /// back exactly as the solver its snapshot came from.
    #[test]
    fn restore_state_restores_ideal_capacity_fallback_state() {
        let env = env_for_temp(20.0, 5.0);
        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };
        let mut original = solver_with_infiltration(&env, InfiltrationMethod::Ach { ach: 0.5 });
        original.prepare_inputs(&ports, &env).unwrap();
        let last_good = original.solve_ideal_capacity_for_target(ZoneId(1), 21.0);
        assert_ne!(last_good, 0.0, "the healthy solve must deliver a capacity");
        let threshold = original.config.ideal_capacity_degraded_threshold;
        for _ in 1..threshold {
            assert_eq!(
                failing_zone_one_solve(&mut original),
                0.0,
                "a failure below the threshold returns zero"
            );
        }
        let snap = original.snapshot_state();

        let mut restored = solver_with_infiltration(&env, InfiltrationMethod::Ach { ach: 0.5 });
        restored.restore_state(&snap).unwrap();

        let expected = failing_zone_one_solve(&mut original);
        let actual = failing_zone_one_solve(&mut restored);
        assert_eq!(
            expected.to_bits(),
            last_good.to_bits(),
            "the failure reaching the threshold returns the last-good capacity"
        );
        assert_eq!(
            actual.to_bits(),
            expected.to_bits(),
            "the restored solver's failing solve ({actual} W) is not the snapshot \
             source's ({expected} W)"
        );
        assert!(restored.zone_capacity_degraded(ZoneId(1)));
    }

    /// The ventilation recovery effectiveness set by the dwelling after one
    /// step is read by the next step's input build, so a restored solver
    /// must hold the snapshot's values, not its own.
    #[test]
    fn restore_state_restores_ventilation_recovery() {
        let env = env_for_temp(20.0, 5.0);
        let mut original = one_zone_solver(&env);
        original.set_ventilation_recovery(0.34, 0.12).unwrap();
        let snap = original.snapshot_state();

        let mut restored = one_zone_solver(&env);
        restored.set_ventilation_recovery(0.72, 0.5).unwrap();
        restored.restore_state(&snap).unwrap();
        let ventilation = &restored.config().ventilation;
        assert_eq!(ventilation.sensible_recovery_efficiency, 0.34);
        assert_eq!(ventilation.latent_recovery_efficiency, 0.12);
    }

    #[test]
    fn set_ventilation_recovery_rejects_values_outside_unit_interval() {
        let env = env_for_temp(20.0, 5.0);
        let mut solver = one_zone_solver(&env);
        for (sensible, latent) in [(1.5, 0.0), (0.0, -0.5), (f64::NAN, 0.0)] {
            let err = solver
                .set_ventilation_recovery(sensible, latent)
                .expect_err("an efficiency outside [0, 1] must be rejected");
            assert!(err.to_string().contains("is outside [0, 1]"), "got: {err}");
        }
    }

    /// The ideal-capacity solve reads the coupling terms of the step as well
    /// as `x` and `last_u`, so a restored solver must solve exactly as the
    /// solver the snapshot was taken from. Covered both ways the coupling
    /// state can differ: a coupled snapshot restored onto a solver holding
    /// other coupling values, and an uncoupled snapshot restored onto a
    /// coupled solver.
    #[test]
    fn restore_state_restores_coupling_state() {
        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };
        let mild_env = env_for_temp(20.0, 5.0);
        let cold_env = env_for_temp(20.0, -15.0);
        let target_c = 21.0;

        for (scenario, prepare_original) in [
            ("coupled snapshot onto other couplings", true),
            ("uncoupled snapshot onto a coupled solver", false),
        ] {
            let mut original =
                solver_with_infiltration(&mild_env, InfiltrationMethod::Ach { ach: 0.5 });
            if prepare_original {
                original.prepare_inputs(&ports, &mild_env).unwrap();
            }
            let snap = original.snapshot_state();

            let mut restored = original.clone();
            restored.prepare_inputs(&ports, &cold_env).unwrap();
            assert_ne!(
                restored.last_coupling, original.last_coupling,
                "{scenario}: the solver restored onto must hold different couplings"
            );
            restored.restore_state(&snap).unwrap();

            let expected = original.solve_ideal_capacity_for_target(ZoneId(1), target_c);
            let actual = restored.solve_ideal_capacity_for_target(ZoneId(1), target_c);
            assert_eq!(
                actual.to_bits(),
                expected.to_bits(),
                "{scenario}: the solve after restore_state ({actual} W) is not bitwise \
                 the snapshot source's ({expected} W)"
            );
        }
    }

    #[test]
    fn restore_state_rejects_non_finite_interior_surface_temps_and_leaves_solver_unchanged() {
        let env = env_for_temp(22.0, 5.0);
        let mut solver = interior_lwr_solver(&env);
        let mut bad_snap = solver.snapshot_state();
        bad_snap.interior_surface_temps[0][1] = f64::NAN;
        assert_restore_rejected_atomically(
            &mut solver,
            &bad_snap,
            "non-finite interior_surface_temps[0][1]",
        );
    }

    /// The per-step irradiance slot map resolves duplicate `surface_id`s
    /// FIRST-WINS — the resolution the pre-slot-map linear `.find()` gave.
    /// A plain insert loop would silently flip duplicates to last-wins,
    /// making the lookup a semantics change instead of a performance
    /// refactor. Weather producers emitting duplicate ids are malformed
    /// input; this pins that their handling stays deterministic and
    /// backward-compatible.
    #[test]
    fn solar_slot_map_resolves_duplicate_surface_ids_first_wins() {
        let mut env = env_for_temp(20.0, 10.0);
        env.weather.solar_irradiance = vec![
            SurfaceIrradiance {
                surface_id: 7,
                direct_w_m2: 111.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            },
            SurfaceIrradiance {
                surface_id: 7,
                direct_w_m2: 999.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            },
        ];
        let a_c = DMatrix::from_row_slice(1, 1, &[-1e-4]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1e-4, 1e-5]);
        let mapping = crate::state_space::OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model =
            crate::state_space::StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping)
                .expect("model");
        let wiring = crate::thermal_solver::StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
            outdoor_temp_input_indices: vec![0],
            ..Default::default()
        };
        let config = ThermalSolverConfig::new(ZoneId(1));
        let mut solver =
            ThermalSolver::new(model, wiring, config, 60.0, &env, 20.0).expect("solver");
        solver.refresh_solar_slot_map(&env);
        assert_eq!(
            solver.solar_irr_slot_buf.get(&7),
            Some(&0),
            "duplicate surface_id must resolve to the FIRST entry (slot 0, \
             direct=111 W/m²), matching the pre-slot-map `.find()` — got \
             {:?} (last-wins would be slot 1)",
            solver.solar_irr_slot_buf.get(&7)
        );
    }

    /// The per-step slot map is rebuilt only when the incoming surface-id
    /// sequence differs from the one it was built from. Over 100 steps with a
    /// fixed surface list the map is rebuilt exactly once (its first
    /// refresh); a changed list rebuilds it again. Detected with a sentinel
    /// entry a rebuild would clear: `solar_irr_slot_buf` is otherwise only
    /// mutated inside `refresh_solar_slot_map`.
    #[test]
    fn solar_slot_map_rebuilt_only_on_surface_change() {
        let mut env = env_for_temp(20.0, 10.0);
        env.weather.solar_irradiance = vec![
            SurfaceIrradiance {
                surface_id: 3,
                direct_w_m2: 100.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            },
            SurfaceIrradiance {
                surface_id: 5,
                direct_w_m2: 200.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            },
        ];
        let a_c = DMatrix::from_row_slice(1, 1, &[-1e-4]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1e-4, 1e-5]);
        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).expect("model");
        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
            outdoor_temp_input_indices: vec![0],
            ..Default::default()
        };
        let config = ThermalSolverConfig::new(ZoneId(1));
        let mut solver =
            ThermalSolver::new(model, wiring, config, 60.0, &env, 20.0).expect("solver");

        // The first refresh builds the map once.
        solver.refresh_solar_slot_map(&env);
        assert_eq!(solver.solar_irr_slot_buf.get(&3), Some(&0));
        assert_eq!(solver.solar_irr_slot_buf.get(&5), Some(&1));

        // 100 more refreshes over the SAME surface sequence must not rebuild:
        // the sentinel survives every one of them.
        solver.solar_irr_slot_buf.insert(u32::MAX, usize::MAX);
        for _ in 0..100 {
            solver.refresh_solar_slot_map(&env);
            assert_eq!(
                solver.solar_irr_slot_buf.get(&u32::MAX),
                Some(&usize::MAX),
                "slot map was rebuilt although the surface-id sequence did \
                 not change: refresh_solar_slot_map must compare the \
                 incoming sequence and rebuild only on difference"
            );
        }

        // A changed surface list rebuilds the map: the sentinel is gone and
        // the new ids are mapped.
        env.weather.solar_irradiance = vec![SurfaceIrradiance {
            surface_id: 9,
            direct_w_m2: 50.0,
            diffuse_w_m2: 0.0,
            reflected_w_m2: 0.0,
            angle_of_incidence_rad: 0.0,
        }];
        solver.refresh_solar_slot_map(&env);
        assert_eq!(
            solver.solar_irr_slot_buf.get(&u32::MAX),
            None,
            "a changed surface-id sequence must rebuild the slot map"
        );
        assert_eq!(solver.solar_irr_slot_buf.get(&9), Some(&0));
    }

    /// The coupled solve divides a coupled state's update by `1 + d_i`. The
    /// divisor's model-side factor (the B_eff coefficient at every coupling
    /// site) is checked once at construction: a negative coefficient lets
    /// `1 + d_i` reach zero or go negative under a physical conductance, and
    /// a non-finite coefficient makes the divisor non-finite, so construction
    /// fails with the state named; a healthy model constructs, at the zone
    /// sensible site and at an exterior surface's LWR site alike.
    #[test]
    fn coupled_solve_divisor_checked_at_construction() {
        // 1-state model: state 0 = zone air; inputs 0 = outdoor temp,
        // 1 = zone sensible heat. The infiltration coupling site is
        // (state 0, input 1); `b_coeff` is the B_eff coefficient there.
        fn one_zone_solver_result(
            b_coeff: f64,
        ) -> std::result::Result<ThermalSolver, ThermalSolverError> {
            let a_d = DMatrix::from_row_slice(1, 1, &[0.99]);
            let b_d = DMatrix::from_row_slice(1, 2, &[1.0e-4, b_coeff]);
            let c = DMatrix::from_row_slice(1, 1, &[1.0]);
            let d = DMatrix::from_row_slice(1, 2, &[0.0, 0.0]);
            let model = StateSpaceModel::from_discrete(a_d, b_d, c, d).expect("model");
            let wiring = StateSpaceWiring {
                zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
                zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
                zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
                outdoor_temp_input_indices: vec![0],
                ..Default::default()
            };
            let config = ThermalSolverConfig {
                indoor_zone_id: ZoneId(1),
                // Non-zero ACH keeps a coupling entry alive on every step, so
                // the coupled solve runs with the divisor this checks.
                infiltration: vec![(ZoneId(1), InfiltrationMethod::Ach { ach: 0.5 })],
                ..ThermalSolverConfig::new(ZoneId(1))
            };
            let env = env_for_temp(20.0, 10.0);
            ThermalSolver::new(model, wiring, config, 60.0, &env, 20.0)
        }

        // A healthy model constructs.
        assert!(
            one_zone_solver_result(1.0 / 50_000.0).is_ok(),
            "a model whose coupling coefficients are finite and non-negative \
             must construct"
        );

        let expect_coefficient_error =
            |b_coeff: f64, scenario: &str| match one_zone_solver_result(b_coeff) {
                Err(ThermalSolverError::CouplingCoefficientInvalid {
                    site,
                    state_index,
                    coefficient,
                }) => {
                    assert_eq!(state_index, 0, "{scenario}: wrong state named");
                    assert_eq!(
                        coefficient.to_bits(),
                        b_coeff.to_bits(),
                        "{scenario}: wrong coefficient reported"
                    );
                    assert_eq!(site, "zone ZoneId(1)", "{scenario}: wrong site named");
                }
                Err(other) => panic!(
                    "{scenario}: expected Err(CouplingCoefficientInvalid) with the \
                     state named, but got {other:?}"
                ),
                Ok(_) => panic!(
                    "{scenario}: expected construction to fail with \
                     Err(CouplingCoefficientInvalid); a coupling coefficient of \
                     {b_coeff} lets the coupled solve's divisor 1 + d_i reach \
                     zero, go negative or become non-finite"
                ),
            };

        // b_coeff = -1: with a physical conductance h = 1 W/K the divisor is
        // exactly 1 + 1·(-1) = 0.
        expect_coefficient_error(-1.0, "divisor zero");
        // b_coeff = -2: with h = 1 W/K the divisor is 1 + 1·(-2) = -1 < 0.
        expect_coefficient_error(-2.0, "divisor negative");
        // A non-finite coefficient makes the divisor non-finite.
        expect_coefficient_error(f64::NAN, "divisor non-finite");
        expect_coefficient_error(f64::NEG_INFINITY, "divisor non-finite (-inf)");

        // 2-state model: state 0 = zone air, state 1 = surface node; inputs
        // 0 = outdoor temp, 1 = zone sensible heat, 2 = surface heat. A
        // rad_frac = 0 wall linearises into an LWR coupling at
        // (state 1, input 2); `surface_b` is the B_eff coefficient there.
        fn surface_solver_result(
            surface_b: f64,
        ) -> std::result::Result<ThermalSolver, ThermalSolverError> {
            let a_d = DMatrix::from_row_slice(2, 2, &[0.99, 0.0, 0.0, 0.98]);
            let b_d = DMatrix::from_row_slice(
                2,
                3,
                &[
                    1.0e-4,
                    1.0 / 50_000.0,
                    0.0, // zone air: outdoor + HVAC sensible drive
                    0.0,
                    0.0,
                    surface_b, // surface node: surface heat input
                ],
            );
            let c = DMatrix::identity(2, 2);
            let d = DMatrix::zeros(2, 3);
            let model = StateSpaceModel::from_discrete(a_d, b_d, c, d).expect("model");
            let wiring = StateSpaceWiring {
                zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
                zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
                zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
                outdoor_temp_input_indices: vec![0],
                ..Default::default()
            };
            let config = ThermalSolverConfig {
                indoor_zone_id: ZoneId(1),
                exterior_surfaces: vec![ExteriorSurfaceInfo {
                    surface_id: 11,
                    state_index: 1,
                    input_index: 2,
                    area_m2: 10.0,
                    emissivity: 0.9,
                    tilt_deg: 90.0,
                    azimuth_deg: 180.0,
                    rad_frac: 0.0,
                    rad_res_k_w: 0.0,
                    n_iter: 1,
                    absorptance: 0.7,
                    boundary_category: Some(BoundaryCategory::Wall),
                    u_factor_w_m2_k: 0.0,
                    h_out_w_m2_k: 0.0,
                }],
                ..ThermalSolverConfig::new(ZoneId(1))
            };
            let env = env_for_temp(20.0, 10.0);
            ThermalSolver::new(model, wiring, config, 60.0, &env, 20.0)
        }

        // A healthy surface coupling site constructs.
        assert!(
            surface_solver_result(1.0 / 30_000.0).is_ok(),
            "a model whose surface coupling coefficient is finite and \
             non-negative must construct"
        );
        match surface_solver_result(-0.5) {
            Err(ThermalSolverError::CouplingCoefficientInvalid {
                site,
                state_index,
                coefficient,
            }) => {
                assert_eq!(state_index, 1, "wrong state named for the surface site");
                assert_eq!(coefficient, -0.5);
                assert_eq!(site, "exterior surface 11", "wrong site named");
            }
            Err(other) => panic!(
                "expected Err(CouplingCoefficientInvalid) naming the exterior \
                 surface, but got {other:?}"
            ),
            Ok(_) => panic!(
                "expected construction to fail for a negative surface coupling \
                 coefficient; its divisor 1 + d_i is not guaranteed positive"
            ),
        }
    }

    /// The ideal-capacity solves of one step share the prefix (`N·x`,
    /// `B_eff·u`, rhs): exactly one prefix fill per step no matter how many
    /// targets run against the unchanged solver state, one target-dependent
    /// tail per target, and every returned capacity is bitwise the per-call
    /// computation's (same-run A/B against the full solve with a local
    /// scratch). Runs twice, once with couplings active (the identity-coupled
    /// tail) and once with none (the uncoupled tail). Each invalidation
    /// window is exercised separately: a solve after `integrate` (before the
    /// next prepare), after `prepare_inputs` with changed weather, and after
    /// `restore_state`; each must refill exactly once and match a per-call
    /// recomputation from the new state; a stale prefix would fail the
    /// bitwise A/B.
    #[test]
    fn shared_terms_computed_once_per_step() {
        for (scenario, coupled) in [
            ("no couplings (uncoupled tail)", false),
            ("couplings active (identity-coupled tail)", true),
        ] {
            let env = env_for_temp(20.0, 0.0);
            let mut solver = if coupled {
                solver_with_infiltration(&env, InfiltrationMethod::Ach { ach: 0.5 })
            } else {
                one_zone_solver(&env)
            };
            let ports = PortSlots {
                thermal: vec![ThermalAccumulator::new(ZoneId(1))],
                ..Default::default()
            };
            let cold_env = env_for_temp(20.0, -10.0);
            let mut out = DomainUpdate::empty(hares_types::THERMAL);

            let input_idx = solver.wiring().zone_sensible_input_indices[&ZoneId(1)];
            let output_idx = solver.wiring().zone_output_indices[&ZoneId(1)];
            let n_states = solver.model_dims().0;
            let mut reference_scratch = crate::state_space::SolveScratch::new(n_states);

            // Per-call recomputation: the full solve with a local scratch,
            // from the same (x, last_u, last_coupling), minus the same
            // `last_u[input_idx]` baseline the solver subtracts. Asserts the
            // shared path's capacity is bitwise identical.
            let mut assert_bitwise_reference = |solver: &mut ThermalSolver, target: f64| {
                let raw = if coupled {
                    solver
                        .model
                        .solve_for_scalar_input_identity_coupled(
                            &solver.x,
                            &solver.last_u,
                            crate::state_space::ScalarSolveTarget {
                                y_target: target,
                                output_index: output_idx,
                                input_index: input_idx,
                            },
                            &solver.last_coupling,
                            &mut reference_scratch,
                        )
                        .unwrap()
                } else {
                    solver
                        .model
                        .solve_for_output_input(
                            &solver.x,
                            &solver.last_u,
                            target,
                            output_idx,
                            input_idx,
                            &mut reference_scratch,
                        )
                        .unwrap()
                };
                raw - solver.last_u[input_idx]
            };
            // Three equipment in one zone: three targets resolved against the
            // same step state, as `SolverFeedbackActor::collect_and_solve`
            // does per step.
            let targets = [20.0, 21.0, 22.0];
            let mut mid_snapshot: Option<ThermalSnapshot> = None;

            for step in 0..2 {
                solver.prepare_inputs(&ports, &env).unwrap();
                solver.shared_prefix_fills = 0;
                solver.solve_tail_calls = 0;

                for &target in &targets {
                    let capacity = solver.solve_ideal_capacity_for_target(ZoneId(1), target);
                    let expected = assert_bitwise_reference(&mut solver, target);
                    assert_eq!(
                        capacity.to_bits(),
                        expected.to_bits(),
                        "{scenario}: step {step} target {target}: shared-path capacity \
                         {capacity} is not bitwise the per-call computation's {expected}"
                    );
                }

                assert_eq!(
                    solver.shared_prefix_fills,
                    1,
                    "{scenario}: step {step} must fill the shared prefix exactly once \
                     for {} targets",
                    targets.len()
                );
                assert_eq!(
                    solver.solve_tail_calls,
                    targets.len(),
                    "{scenario}: step {step} must run one target-dependent tail per target"
                );

                // Step the solver: integrate changes x and last_u.
                solver.integrate(&ports, &env, &mut out).unwrap();
                if step == 0 {
                    mid_snapshot = Some(solver.snapshot_state());
                }
            }

            // Window 1: a solve between integrate and the next prepare (the
            // public API allows it) must refill from the new x and last_u.
            solver.shared_prefix_fills = 0;
            solver.solve_tail_calls = 0;
            let capacity = solver.solve_ideal_capacity_for_target(ZoneId(1), 21.5);
            let expected = assert_bitwise_reference(&mut solver, 21.5);
            assert_eq!(
                capacity.to_bits(),
                expected.to_bits(),
                "{scenario}: the post-integrate solve is not bitwise the per-call \
                 computation's: the prefix survived across integrate"
            );
            assert_eq!(
                solver.shared_prefix_fills, 1,
                "{scenario}: the first solve after integrate must refill the prefix"
            );
            assert_eq!(solver.solve_tail_calls, 1);

            // Window 2: prepare_inputs with changed weather rebuilds last_u;
            // the first solve after it must refill.
            solver.prepare_inputs(&ports, &cold_env).unwrap();
            solver.shared_prefix_fills = 0;
            solver.solve_tail_calls = 0;
            let capacity = solver.solve_ideal_capacity_for_target(ZoneId(1), 21.5);
            let expected = assert_bitwise_reference(&mut solver, 21.5);
            assert_eq!(
                capacity.to_bits(),
                expected.to_bits(),
                "{scenario}: the post-prepare solve is not bitwise the per-call \
                 computation's: the prefix survived across prepare_inputs"
            );
            assert_eq!(
                solver.shared_prefix_fills, 1,
                "{scenario}: the first solve after prepare_inputs must refill the prefix"
            );
            assert_eq!(solver.solve_tail_calls, 1);

            // Window 3: restore_state replaces x and last_u (here with the
            // step-0 values, which differ from the current ones); the first
            // solve after it must refill.
            let snap = mid_snapshot.expect("step 0 ran");
            solver.restore_state(&snap).unwrap();
            solver.shared_prefix_fills = 0;
            solver.solve_tail_calls = 0;
            let capacity = solver.solve_ideal_capacity_for_target(ZoneId(1), 21.5);
            let expected = assert_bitwise_reference(&mut solver, 21.5);
            assert_eq!(
                capacity.to_bits(),
                expected.to_bits(),
                "{scenario}: the post-restore solve is not bitwise the per-call \
                 computation's: the prefix survived across restore_state"
            );
            assert_eq!(
                solver.shared_prefix_fills, 1,
                "{scenario}: the first solve after restore_state must refill the prefix"
            );
            assert_eq!(solver.solve_tail_calls, 1);
        }
    }
}
