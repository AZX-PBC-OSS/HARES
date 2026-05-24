//! Thermal domain solver for the building envelope.

mod config;
mod infiltration;
mod initialization;
mod longwave;
mod ports;
mod solar;
mod stepping;

pub(crate) use config::Result;
pub use config::{
    BoundaryCategory, BoundaryDiagnosticInfo, DrivingTemp, EnvelopeComponentGains,
    ExteriorSurfaceInfo, InfiltrationMethod, InteriorLwrZoneConfig, InteriorSolarSurfaceInfo,
    InteriorSolarZoneConfig, InteriorSurfaceInfo, MechanicalVentilationParams,
    NaturalVentilationConfig, StateSpaceWiring, ThermalSolverConfig, ThermalSolverError,
    WindowSolarProperties,
};

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use hares_physics::constants::{KJ_TO_J, LATENT_HEAT_VAPORISATION_0C_KJ_KG};
use hares_types::{
    DomainId, DomainSolver, DomainUpdate, EnvironmentState, PortSlots, THERMAL, ThermalCategory,
    ZoneId,
};
use nalgebra::{DMatrix, DVector};

use crate::state_space::StateSpaceModel;

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
    /// Reusable state-step buffer: receives M⁻¹(N·x + B_eff·u).
    rhs_buf: DVector<f64>,
    /// Pre-allocated scratch matrix for per-step modified implicit matrix (M + D).
    m_scratch: DMatrix<f64>,
    /// Per-step coupling tuples: (state_idx, d_implicit, forcing). Reused each step.
    coupling_buf: Vec<(usize, f64, f64)>,
    /// Previous coupling tuples and pre-built LU for `solve_ideal_capacity_for_target`.
    last_coupling: Vec<(usize, f64, f64)>,
    last_coupled_lu: Option<nalgebra::linalg::LU<f64, nalgebra::Dyn, nalgebra::Dyn>>,
    /// Per-exterior-surface converged surface temperatures [°C] for LWR continuity.
    /// Indexed parallel to `config.exterior_surfaces`.
    exterior_surface_temps: Vec<f64>,
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
    /// Pre-allocated buffer for interior LWR net flux results per surface.
    lwr_net_flux_buf: Vec<f64>,
    /// Pre-allocated buffer for previous-iteration interior LWR net flux values.
    /// Used for relative flux-residual convergence checking.
    lwr_net_flux_prev_buf: Vec<f64>,
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
    /// Per-zone energy balance residuals [W] from the current timestep's closure check.
    /// Populated by `integrate_inner`, consumed by `format_domain_update` for telemetry.
    energy_balance_residuals: HashMap<ZoneId, f64>,
    /// Pre-allocated fallback buffer for InteriorSurface structs in non-ScriptF path.
    lwr_surfaces_buf: Vec<crate::longwave_radiation::InteriorSurface>,
    /// Zones for which the linearised interior LWR fallback has already emitted
    /// a one-time warning. Guards against per-timestep log spam.
    lwr_linearised_warned_zones: HashSet<ZoneId>,
    /// Cached outdoor temperature [°C] from the most recent input vector.
    /// Used by boundary diagnostics for non-RC boundaries.
    #[cfg(any(debug_assertions, feature = "observe_detailed"))]
    cached_outdoor_temp_c: f64,
    /// Cached ground temperature [°C] from the most recent input vector.
    #[cfg(any(debug_assertions, feature = "observe_detailed"))]
    cached_ground_temp_c: f64,
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

impl ThermalSolver {
    pub fn state_vector(&self) -> &[f64] {
        self.x.as_slice()
    }

    pub fn config(&self) -> &ThermalSolverConfig {
        &self.config
    }

    /// Per-component envelope gains from the most recent `resolve()` call.
    pub fn component_gains(&self) -> &EnvelopeComponentGains {
        &self.component_gains
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
    /// Returns (after_outdoor, after_window_solar, after_ext_solar, after_ext_lwr, after_int_lwr, after_port) [W].
    pub fn zone_sensible_breakdown_debug(
        &mut self,
        ports: &hares_types::PortSlots,
        env: &hares_types::EnvironmentState,
    ) -> [f64; 6] {
        let n = self.model.input_dim();
        let mut u = DVector::zeros(n);
        let zone_id = self.config.indoor_zone_id;
        let Some(&z_idx) = self.wiring.zone_sensible_input_indices.get(&zone_id) else {
            return [0.0; 6];
        };
        self.apply_outdoor_inputs(&mut u, env);
        let after_outdoor = u[z_idx];
        self.apply_solar_inputs(&mut u, env);
        let after_window_solar = u[z_idx];
        self.apply_exterior_solar_inputs(&mut u, env);
        let after_ext_solar = u[z_idx];
        self.apply_exterior_longwave_inputs_iterative(&mut u, env);
        let after_ext_lwr = u[z_idx];
        // Interior LWR: ScriptF iterative injection only when not using
        // StarMesh (star-mesh bakes radiation conductances into the A-matrix
        // at construction time, so no per-timestep injection is needed).
        if self.config.interior_lwr_method == crate::boundary_rc::InteriorLwrMethod::ScriptF {
            self.apply_interior_longwave_inputs(&mut u, env);
        }
        let after_int_lwr = u[z_idx];
        self.apply_port_sensible_inputs(&mut u, ports);
        let after_port = u[z_idx];
        [
            after_outdoor,
            after_window_solar,
            after_ext_solar,
            after_ext_lwr,
            after_int_lwr,
            after_port,
        ]
    }

    pub fn new(
        model: StateSpaceModel,
        wiring: StateSpaceWiring,
        config: ThermalSolverConfig,
        dt_s: f64,
        env: &EnvironmentState,
        indoor_temp_c: f64,
    ) -> Result<Self> {
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
        let m_scratch = DMatrix::zeros(n_states, n_states);
        let coupling_buf = Vec::with_capacity(env.zones.len());
        let last_coupling = Vec::new();
        let last_coupled_lu = None;
        let latent_buf = HashMap::new();
        let exterior_surface_temps =
            vec![env.weather.outdoor_temp_c; config.exterior_surfaces.len()];
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
                        DrivingTemp::Ground => env.weather.ground_temp_c,
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

        #[cfg(any(debug_assertions, feature = "observe_detailed"))]
        let n_ext_surfaces = config.exterior_surfaces.len();
        #[cfg(any(debug_assertions, feature = "observe_detailed"))]
        let n_windows = config.window_properties.len();

        Ok(Self {
            model,
            wiring,
            config,
            dt_s,
            x,
            last_u,
            u_buf,
            rhs_buf,
            m_scratch,
            coupling_buf,
            last_coupling,
            last_coupled_lu,
            latent_buf,
            exterior_surface_temps,
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
            lwr_net_flux_buf: Vec::with_capacity(max_interior_surfaces),
            lwr_net_flux_prev_buf: Vec::with_capacity(max_interior_surfaces),
            prev_zone_temps_c: env.zones.iter().map(|z| (z.id, z.temperature_c)).collect(),
            radiant_weights_buf: Vec::with_capacity(max_radiant_surfaces),
            lwr_surfaces_buf: Vec::with_capacity(max_interior_surfaces),
            lwr_linearised_warned_zones: HashSet::new(),
            energy_balance_residuals: HashMap::with_capacity(n_zones_for_latent),
            zone_temps_buf,
            latent_pairs_buf: Vec::with_capacity(n_zones_for_latent),
            custom_payload_buf: Vec::with_capacity(n_zones_for_latent * 5),
            #[cfg(any(debug_assertions, feature = "observe_detailed"))]
            ext_surface_diag_buf: Vec::with_capacity(n_ext_surfaces),
            #[cfg(any(debug_assertions, feature = "observe_detailed"))]
            int_surface_diag_buf: Vec::with_capacity(max_interior_surfaces),
            #[cfg(any(debug_assertions, feature = "observe_detailed"))]
            window_solar_diag_buf: Vec::with_capacity(n_windows),
            #[cfg(any(debug_assertions, feature = "observe_detailed"))]
            cached_outdoor_temp_c: env.weather.outdoor_temp_c,
            #[cfg(any(debug_assertions, feature = "observe_detailed"))]
            cached_ground_temp_c: env.weather.ground_temp_c,
        })
    }

    #[must_use]
    pub fn state(&self) -> &DVector<f64> {
        &self.x
    }

    /// Returns checkpointable thermal state vectors.
    ///
    /// Returns `(x, last_u, lwr_t_prev_c)` where `lwr_t_prev_c` contains the
    /// per-exterior-surface converged surface temperatures for LWR continuity.
    #[must_use]
    pub fn snapshot_state(&self) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
        (
            self.x.iter().copied().collect(),
            self.last_u.iter().copied().collect(),
            self.exterior_surface_temps.clone(),
        )
    }

    /// Restores thermal state vectors from checkpoint payloads.
    pub fn restore_state(
        &mut self,
        x_state: &[f64],
        last_u_state: &[f64],
        lwr_t_prev_c: &[f64],
    ) -> Result<()> {
        if x_state.len() != self.x.len() {
            return Err(ThermalSolverError::Initialization(format!(
                "invalid x state length: got {}, expected {}",
                x_state.len(),
                self.x.len()
            )));
        }
        if !last_u_state.is_empty() && last_u_state.len() != self.last_u.len() {
            return Err(ThermalSolverError::Initialization(format!(
                "invalid last_u length: got {}, expected {}",
                last_u_state.len(),
                self.last_u.len()
            )));
        }

        self.x = DVector::from_column_slice(x_state);
        if last_u_state.is_empty() {
            self.last_u.fill(0.0);
        } else {
            self.last_u = DVector::from_column_slice(last_u_state);
        }

        if lwr_t_prev_c.len() != self.exterior_surface_temps.len() {
            return Err(ThermalSolverError::Initialization(format!(
                "invalid lwr_t_prev_c length: got {}, expected {}",
                lwr_t_prev_c.len(),
                self.exterior_surface_temps.len()
            )));
        }
        for (i, &t) in lwr_t_prev_c.iter().enumerate() {
            if !t.is_finite() {
                return Err(ThermalSolverError::Initialization(format!(
                    "non-finite surface temperature at index {i}: {t}"
                )));
            }
        }
        self.exterior_surface_temps.copy_from_slice(lwr_t_prev_c);
        Ok(())
    }

    /// Assembles the full input vector from outdoor, solar, LWR, and port
    /// contributions. Updates `self.component_gains` for diagnostics.
    /// Populates `self.infiltration_buf` with per-zone coupling terms.
    ///
    /// Returns `(u, latent_by_zone)` where the infiltration couplings are
    /// stored in `self.infiltration_buf` for semi-implicit coupling wiring.
    fn build_input_vector(
        &mut self,
        ports: &PortSlots,
        env: &EnvironmentState,
    ) -> (DVector<f64>, HashMap<ZoneId, f64>) {
        let mut u = std::mem::replace(&mut self.u_buf, DVector::zeros(0));
        let n = self.model.input_dim();
        if u.len() == n {
            u.fill(0.0);
        } else {
            u = DVector::zeros(n);
        }

        self.apply_outdoor_inputs(&mut u, env);

        let u_pre = u.iter().sum::<f64>();
        #[cfg(any(debug_assertions, feature = "observe_detailed"))]
        self.window_solar_diag_buf.clear();
        self.apply_solar_inputs(&mut u, env);
        let window_solar_w = u.iter().sum::<f64>() - u_pre;

        let u_pre = u.iter().sum::<f64>();
        self.apply_exterior_solar_inputs(&mut u, env);
        let opaque_solar_w = u.iter().sum::<f64>() - u_pre;
        let u_pre = u.iter().sum::<f64>();
        #[cfg(any(debug_assertions, feature = "observe_detailed"))]
        self.ext_surface_diag_buf.clear();
        self.apply_exterior_longwave_inputs_iterative(&mut u, env);
        let exterior_lwr_w = u.iter().sum::<f64>() - u_pre;
        let opaque_solar_lwr_w = opaque_solar_w + exterior_lwr_w - self.window_exterior_lwr_w;

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

        self.apply_port_sensible_inputs(&mut u, ports);
        self.apply_port_radiant_inputs(&mut u, ports);

        let indoor_zone = self.config.indoor_zone_id;
        let port_sensible_indoor_w = ports
            .thermal
            .iter()
            .find(|t| t.zone == indoor_zone)
            .map(|t| t.sensible_gain_w)
            .unwrap_or(0.0);
        let port_radiant_indoor_w = ports
            .thermal
            .iter()
            .find(|t| t.zone == indoor_zone)
            .map(|t| t.radiant_gain_w)
            .unwrap_or(0.0);

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
        );

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
            })
            .unwrap_or(0.0);
        let jacket_loss_w = indoor_acc
            .map(|a| a.sensible_for_category(ThermalCategory::JacketLoss))
            .unwrap_or(0.0);
        // Duct losses are deposited into the duct zone accumulator (e.g. attic),
        // not the indoor zone. Sum DuctLoss across all zone accumulators.
        let duct_loss_w: f64 = ports
            .thermal
            .iter()
            .map(|a| a.sensible_for_category(ThermalCategory::DuctLoss))
            .sum();

        self.infiltration_by_zone_buf.clear();
        self.infiltration_by_zone_buf.extend(
            self.infiltration_buf
                .iter()
                .map(|c| (c.zone, c.q_infiltration_w)),
        );
        self.infiltration_by_zone_buf.sort_by_key(|(z, _)| *z);

        self.component_gains = EnvelopeComponentGains {
            window_solar_w,
            opaque_solar_lwr_w,
            interior_lwr_w,
            infiltration_w: infiltration_indoor_w,
            ventilation_w,
            natural_ventilation_w,
            combined_airflow_sensible_w,
            port_sensible_w: port_sensible_indoor_w,
            port_radiant_w: port_radiant_indoor_w,
            hvac_heating_w,
            hvac_cooling_w,
            internal_gain_w: internal_gain_cat_w,
            jacket_loss_w,
            duct_loss_w,
            infiltration_by_zone: Vec::new(),
            interior_lwr_by_zone: Vec::new(),
            wall_heat_gain_w: 0.0,
            floor_heat_gain_w: 0.0,
            roof_heat_gain_w: 0.0,
            window_heat_gain_w: 0.0,
            internal_mass_heat_gain_w: 0.0,
            driving_outdoor_temp_c: env.weather.outdoor_temp_c,
            driving_ground_temp_c: env.weather.ground_temp_c,
            opaque_solar_w,
            exterior_lwr_w,
            window_exterior_lwr_w: self.window_exterior_lwr_w,
            total_airflow_m3_s: indoor_inf.map(|c| c.combined_flow_m3_s).unwrap_or(0.0),
            raw_infiltration_m3_s: indoor_inf.map(|c| c.raw_inf_m3_s).unwrap_or(0.0),
            forced_vent_m3_s: indoor_inf.map(|c| c.forced_flow_m3_s).unwrap_or(0.0),
            natural_vent_m3_s: indoor_inf.map(|c| c.nat_flow_m3_s).unwrap_or(0.0),
            #[cfg(any(debug_assertions, feature = "observe_detailed"))]
            ext_surface_diag: self.ext_surface_diag_buf.clone(),
            #[cfg(any(debug_assertions, feature = "observe_detailed"))]
            int_surface_diag: self.int_surface_diag_buf.clone(),
            #[cfg(any(debug_assertions, feature = "observe_detailed"))]
            window_solar_diag: self.window_solar_diag_buf.clone(),
        };

        std::mem::swap(
            &mut self.infiltration_by_zone_buf,
            &mut self.component_gains.infiltration_by_zone,
        );
        std::mem::swap(
            &mut self.lwr_by_zone_buf,
            &mut self.component_gains.interior_lwr_by_zone,
        );
        // After swap, the buf fields hold the empty Vecs from the freshly
        // constructed struct. Reserve capacity so the next step avoids realloc.
        let n_inf = self.component_gains.infiltration_by_zone.len();
        let n_lwr = self.component_gains.interior_lwr_by_zone.len();
        if self.infiltration_by_zone_buf.capacity() < n_inf {
            self.infiltration_by_zone_buf.reserve(n_inf);
        }
        if self.lwr_by_zone_buf.capacity() < n_lwr {
            self.lwr_by_zone_buf.reserve(n_lwr);
        }

        (u, latent_by_zone)
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

    fn apply_outdoor_inputs(&mut self, u: &mut DVector<f64>, env: &EnvironmentState) {
        for &idx in &self.wiring.outdoor_temp_input_indices {
            if idx < u.len() {
                u[idx] = env.weather.outdoor_temp_c;
            }
        }
        #[cfg(any(debug_assertions, feature = "observe_detailed"))]
        {
            self.cached_outdoor_temp_c = env.weather.outdoor_temp_c;
            self.cached_ground_temp_c = env.weather.ground_temp_c;
        }
        for &idx in &self.wiring.ground_temp_input_indices {
            if idx < u.len() {
                u[idx] = env.weather.ground_temp_c;
            }
        }
    }

    pub fn prepare_inputs(&mut self, ports: &PortSlots, env: &EnvironmentState) {
        debug_assert!(
            (env.time_step_secs() - self.dt_s).abs() < 1e-6,
            "ThermalSolver: runtime dt ({:.3}s) != configured dt ({:.3}s); re-discretize or use constant timestep",
            env.time_step_secs(),
            self.dt_s
        );
        self.prepare_inputs_inner(ports, env);
    }

    pub fn integrate(&mut self, ports: &PortSlots, env: &EnvironmentState, out: &mut DomainUpdate) {
        debug_assert!(
            (env.time_step_secs() - self.dt_s).abs() < 1e-6,
            "ThermalSolver: runtime dt ({:.3}s) != configured dt ({:.3}s); re-discretize or use constant timestep",
            env.time_step_secs(),
            self.dt_s
        );
        self.integrate_inner(ports, env, out);
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
    ) {
        debug_assert!(
            (dt.as_secs_f64() - self.dt_s).abs() < 1e-6,
            "ThermalSolver: runtime dt ({:.3}s) != configured dt ({:.3}s); re-discretize or use constant timestep",
            dt.as_secs_f64(),
            self.dt_s
        );
        self.resolve_internal(ports, env, out);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use chrono::{FixedOffset, TimeZone};
    use hares_types::{
        DomainSolver, DomainUpdate, EnvironmentState, GridState, PortSlots, SurfaceIrradiance,
        ThermalAccumulator, WeatherState, ZoneId, ZoneState,
    };
    use nalgebra::{DMatrix, DVector};

    use crate::longwave_radiation::{SOLAR_ABSORPTANCE_DEFAULT, beta_factor};
    use crate::state_space::{OutputMapping, StateSpaceModel};
    use crate::thermal_solver::{
        DrivingTemp, ExteriorSurfaceInfo, InfiltrationMethod, InteriorLwrZoneConfig,
        InteriorSolarSurfaceInfo, InteriorSolarZoneConfig, InteriorSurfaceInfo,
        MechanicalVentilationParams, NaturalVentilationConfig, StateSpaceWiring, ThermalSolver,
        ThermalSolverConfig, WindowSolarProperties,
    };

    fn env_for_temp(zone_temp: f64, outdoor_temp: f64) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp,
                humidity_ratio: 0.008,
                relative_humidity: 0.45,
                wet_bulb_c: zone_temp - 5.0,
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
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
            },
            custom_domains: vec![],
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
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
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
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::default(),
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
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
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
        };

        let mut interior_lwr_zone = InteriorLwrZoneConfig {
            zone_id: ZoneId(1),
            surfaces: vec![
                InteriorSurfaceInfo {
                    state_index: 1,
                    input_index: 1,
                    area_m2: 12.0,
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
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::default(),
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
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
            let update = solver.resolve_new(&ports, &env, Duration::from_secs(60));
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
            let update = solver.resolve_new(&ports, &env, Duration::from_secs(60));
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
        let update = boxed.resolve_new(&ports, &env, Duration::from_secs(60));
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
            indoor_temp_input_indices: vec![1],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
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
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::default(),
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
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
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
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
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::default(),
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
        };
        let mut solver = ThermalSolver::new(model, wiring, config, 60.0, &env, 20.0).unwrap();
        solver.x[0] = 20.0;

        let ports = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };
        let update = solver.resolve_new(&ports, &env, Duration::from_secs(60));

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
            let update = solver.resolve_new(&ports, &env, Duration::from_secs(60));
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
        #[allow(clippy::approx_constant)]
        #[allow(clippy::approx_constant)]
        let ua_roof = 0.318 * 48.0; // 15.26 W/K -- U-value, not 1/π -- U-value, not 1/π
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
        let (a_c, b_c_rc) = network.build_matrices().unwrap();

        // Augment B_c with a sensible-gain input column (1/C per watt).
        // RC network gives B_c as 1x2 [outdoor, ground]; we add column for Q_sensible.
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
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
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
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::default(),
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
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
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
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
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::default(),
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
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

        let update = solver.resolve_new(&ports, &env, Duration::from_secs(60));
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
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
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
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::default(),
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
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
        let update = solver.resolve_new(&ports, &env, Duration::from_secs(60));
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
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
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
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::default(),
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
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
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
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
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::default(),
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
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
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
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
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::default(),
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
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
        let update = solver.resolve_new(&ports, &env, Duration::from_secs(60));
        let t_after = update.zone_temperatures_c[0].1;

        assert!(
            (t_after - target_c).abs() < 0.05,
            "zone should reach explicit target: t={t_after:.4}, target={target_c}"
        );
    }

    /// Regression test: when `solve_ideal_capacity_for_target` encounters a
    /// singular/zero-gain condition (failure path), it silently returns 0.0 and currently
    /// logs at `debug!`. This test documents the failure path returns 0 and verifies the
    /// bug is present (no `warn!` is emitted). When the ticket fix is applied, the log
    /// level should be promoted to `warn!` and this test should be accompanied by a
    /// `tracing-test` assertion.
    ///
    /// To trigger `ZeroEffectiveGain`: use a B matrix where the HVAC sensible-input column
    /// (column 1) has zero contribution to the zone output (C row × B_eff[:, 1] ≈ 0).
    /// We achieve this by setting the HVAC column of B_c to zero.
    #[test]
    fn solve_ideal_capacity_failure_returns_zero_without_warn() {
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
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
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
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::default(),
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
        };
        let mut solver = ThermalSolver::new(model, wiring, config, 60.0, &env, zone_temp).unwrap();
        solver.x[0] = zone_temp;

        // The HVAC input has zero gain → solve_for_scalar_input returns ZeroEffectiveGain.
        // Current behaviour: silently returns 0.0 and logs at debug! (bug).
        // Expected behaviour after fix: returns 0.0 AND logs at warn!.
        let q = solver.solve_ideal_capacity_for_target(ZoneId(1), 25.0);
        assert_eq!(
            q, 0.0,
            "failure path must return 0.0 (silent fallback — log level should be warn)"
        );
        // NOTE: this test intentionally does NOT assert a warn! is captured, because
        // `tracing-test` is not yet a dev-dependency. Once the log level fix is applied, add
        // `tracing-test` to [dev-dependencies] and add:
        //   #[traced_test] and assert!(logs_contain("solve_ideal_capacity_for_target failed"))
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
                zones: vec![ZoneState {
                    id: ZoneId(1),
                    temperature_c: zone_temp,
                    humidity_ratio: 0.008,
                    relative_humidity: 0.45,
                    wet_bulb_c: zone_temp - 5.0,
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
                },
                grid: GridState {
                    voltage_pu: 1.0,
                    frequency_hz: 60.0,
                },
                custom_domains: vec![],
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
                indoor_temp_input_indices: vec![],
                solar_input_indices: HashMap::new(),
                c_zone_j_k: HashMap::new(),
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
                interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::default(),
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
            .zone_temperatures_c[0]
            .1;

        let mut solver_with = make_solver(&env_solar);
        let t_solar = solver_with
            .resolve_new(&ports, &env_solar, Duration::from_secs(60))
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
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp,
                humidity_ratio: 0.008,
                relative_humidity: 0.45,
                wet_bulb_c: zone_temp - 5.0,
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
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
            },
            custom_domains: vec![],
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
                indoor_temp_input_indices: vec![],
                solar_input_indices: HashMap::new(),
                c_zone_j_k: HashMap::new(),
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
                interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::default(),
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
            .zone_temperatures_c[0]
            .1;

        let mut solver_060 = make_solver(0.60);
        let t_060 = solver_060
            .resolve_new(&ports, &env, Duration::from_secs(60))
            .zone_temperatures_c[0]
            .1;

        let mut solver_005 = make_solver(0.05);
        let t_005 = solver_005
            .resolve_new(&ports, &env, Duration::from_secs(60))
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
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: zone_temp,
                humidity_ratio: 0.008,
                relative_humidity: 0.45,
                wet_bulb_c: zone_temp - 5.0,
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
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
            },
            custom_domains: vec![],
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
                indoor_temp_input_indices: vec![],
                solar_input_indices: HashMap::new(),
                c_zone_j_k: HashMap::new(),
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
                interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::default(),
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
            .zone_temperatures_c[0]
            .1;
        let t_light = make_solver(0.60)
            .resolve_new(&ports, &env, Duration::from_secs(60))
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
            .zone_temperatures_c[0]
            .1;

        let mut env_high_wind = env_for_temp(zone_temp, outdoor_temp);
        env_high_wind.weather.wind_speed_m_s = 8.0;
        let mut solver_high = solver_with_infiltration(&env_high_wind, method);
        let t_high_wind = solver_high
            .resolve_new(&ports, &env_high_wind, Duration::from_secs(60))
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
            .zone_temperatures_c[0]
            .1;

        let mut env_high_wind = env_for_temp(zone_temp, outdoor_temp);
        env_high_wind.weather.wind_speed_m_s = 6.0;
        let mut solver_high = solver_with_infiltration(&env_high_wind, method);
        let t_high_wind = solver_high
            .resolve_new(&ports, &env_high_wind, Duration::from_secs(60))
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
        let (a_c, b_c_rc) = network.build_matrices().unwrap();

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
                zones: vec![ZoneState {
                    id: ZoneId(1),
                    temperature_c: zone_temp,
                    humidity_ratio: 0.008,
                    relative_humidity: 0.45,
                    wet_bulb_c: zone_temp - 5.0,
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
                },
                grid: GridState {
                    voltage_pu: 1.0,
                    frequency_hz: 60.0,
                },
                custom_domains: vec![],
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
                indoor_temp_input_indices: vec![],
                solar_input_indices: HashMap::new(),
                c_zone_j_k: HashMap::new(),
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
                interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::default(),
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
            .zone_temperatures_c[0]
            .1;

        let env_low = make_env(300.0);
        let t_low = make_solver(&env_low)
            .resolve_new(&ports, &env_low, Duration::from_secs(60))
            .zone_temperatures_c[0]
            .1;

        let env_high = make_env(600.0);
        let t_high = make_solver(&env_high)
            .resolve_new(&ports, &env_high, Duration::from_secs(60))
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
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
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
            natural_ventilation: Some(NaturalVentilationConfig::from_window_area(
                12.0,      // 12 m² total window area
                0.000_106, // ELA stack coeff
                0.000_143, // ELA wind coeff
            )),
            supply_duct_leakage_m3_s: 0.0,
            return_duct_leakage_m3_s: 0.0,
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::default(),
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
        };
        let mut solver_nv =
            ThermalSolver::new(model, wiring, config, 60.0, &env, zone_temp).unwrap();
        solver_nv.x[0] = zone_temp;

        let t_nv = solver_nv
            .resolve_new(&ports, &env, Duration::from_secs(60))
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
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
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
            natural_ventilation: Some(NaturalVentilationConfig::from_window_area(
                12.0, 0.000_106, 0.000_143,
            )),
            supply_duct_leakage_m3_s: 0.0,
            return_duct_leakage_m3_s: 0.0,
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::default(),
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
        };
        let mut solver_nv =
            ThermalSolver::new(model, wiring, config, 60.0, &env, zone_temp).unwrap();
        solver_nv.x[0] = zone_temp;

        let t_nv = solver_nv
            .resolve_new(&ports, &env, Duration::from_secs(60))
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
                    rad_frac: 0.0,  // no film resistance → use node temp directly
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
                indoor_temp_input_indices: vec![],
                solar_input_indices: HashMap::new(),
                c_zone_j_k: HashMap::new(),
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
                interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::default(),
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
            .zone_temperatures_c[0]
            .1;

        let t_with_lw = make_solver(true, &env)
            .resolve_new(&ports, &env, Duration::from_secs(60))
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
        };

        // 500 W/m² direct irradiance, no diffuse or reflected.
        let make_env = |aoi_rad: f64| -> EnvironmentState {
            EnvironmentState {
                zones: vec![ZoneState {
                    id: ZoneId(1),
                    temperature_c: zone_temp,
                    humidity_ratio: 0.008,
                    relative_humidity: 0.45,
                    wet_bulb_c: zone_temp - 5.0,
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
                },
                grid: GridState {
                    voltage_pu: 1.0,
                    frequency_hz: 60.0,
                },
                custom_domains: vec![],
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
                indoor_temp_input_indices: vec![],
                solar_input_indices: HashMap::from([(window_surface_id, 2usize)]),
                c_zone_j_k: HashMap::new(),
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
                interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::default(),
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
            .zone_temperatures_c[0]
            .1;

        let t_oblique = make_solver(&env_oblique)
            .resolve_new(&ports, &env_oblique, Duration::from_secs(60))
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
                    indoor_temp_input_indices: vec![],
                    solar_input_indices: HashMap::new(),
                    c_zone_j_k: HashMap::new(),
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
                    interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::default(),
                    interior_solar_zones: Vec::new(),
                    boundary_diagnostics: Vec::new(),
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
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
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
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::default(),
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
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
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
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
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::default(),
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
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
        let update_no = solver_no.resolve_new(&ports, &env, Duration::from_secs(60));
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
        let update_erv = solver_erv.resolve_new(&ports, &env, Duration::from_secs(60));
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
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
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
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::default(),
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
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
        };

        let make_env = |year_month: (i32, u32)| -> EnvironmentState {
            let (year, month) = year_month;
            EnvironmentState {
                zones: vec![ZoneState {
                    id: ZoneId(1),
                    temperature_c: zone_temp,
                    humidity_ratio: 0.008,
                    relative_humidity: 0.45,
                    wet_bulb_c: zone_temp - 5.0,
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
                },
                grid: GridState {
                    voltage_pu: 1.0,
                    frequency_hz: 60.0,
                },
                custom_domains: vec![],
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
                indoor_temp_input_indices: vec![],
                solar_input_indices: HashMap::from([(window_surface_id, 2usize)]),
                c_zone_j_k: HashMap::new(),
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
                interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::default(),
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
            .zone_temperatures_c[0]
            .1;

        let t_jul = make_solver(&env_jul)
            .resolve_new(&ports, &env_jul, Duration::from_secs(60))
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
        solver.prepare_inputs(&ports, &env_cold);

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
        let update_single = solver_single.resolve_new(&ports, &env, Duration::from_secs(60));

        let mut solver_split = one_zone_solver(&env);
        solver_split.x[0] = 20.0;
        solver_split.prepare_inputs(&ports, &env);
        let mut out_split = DomainUpdate::empty(hares_types::THERMAL);
        solver_split.integrate(&ports, &env, &mut out_split);

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
        solver_base.prepare_inputs(&ports_zero, &env);
        let mut out_base = DomainUpdate::empty(hares_types::THERMAL);
        solver_base.integrate(&ports_zero, &env, &mut out_base);
        let t_base = out_base.zone_temperatures_c[0].1;

        // With heating: add 1000 W between phases
        let mut solver_heat = one_zone_solver(&env);
        solver_heat.x[0] = 20.0;
        solver_heat.prepare_inputs(&ports_zero, &env);
        let mut ports_heat = PortSlots {
            thermal: vec![ThermalAccumulator::new(ZoneId(1))],
            ..Default::default()
        };
        ports_heat.thermal[0].sensible_gain_w = 1000.0;
        let mut out_heat = DomainUpdate::empty(hares_types::THERMAL);
        solver_heat.integrate(&ports_heat, &env, &mut out_heat);
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
                sensible_by_category: [0.0; THERMAL_CATEGORY_COUNT],
                radiant_by_category: [radiant_w, 0.0, 0.0, 0.0, 0.0],
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
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
        };
        let mut interior_lwr_zone = InteriorLwrZoneConfig {
            zone_id: ZoneId(1),
            surfaces: vec![
                InteriorSurfaceInfo {
                    state_index: 1,
                    input_index: 1,
                    area_m2: 10.0,
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
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::default(),
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
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
                sensible_by_category: [0.0; THERMAL_CATEGORY_COUNT],
                radiant_by_category: [radiant_w, 0.0, 0.0, 0.0, 0.0],
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
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
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
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::default(),
            interior_solar_zones: Vec::new(),
            boundary_diagnostics: Vec::new(),
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
            indoor_temp_input_indices: vec![],
            solar_input_indices: HashMap::new(),
            c_zone_j_k: HashMap::new(),
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
                    solar_absorptance: 0.7,
                    radiation_frac: 0.5,
                    is_floor: false,
                },
                // Window: input_index is Some (not None), excluded by zero solar_absorptance.
                InteriorSolarSurfaceInfo {
                    input_index: Some(2),
                    area_m2: 5.0,
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
            interior_lwr_method: crate::boundary_rc::InteriorLwrMethod::default(),
            interior_solar_zones: vec![solar_zone],
            boundary_diagnostics: Vec::new(),
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
                sensible_by_category: [0.0; THERMAL_CATEGORY_COUNT],
                radiant_by_category: [radiant_w, 0.0, 0.0, 0.0, 0.0],
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
}
