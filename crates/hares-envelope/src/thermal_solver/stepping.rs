//! State-space integration and ideal HVAC capacity solving.
//!
//! Contains `resolve_internal` (the per-timestep CN step with semi-implicit
//! infiltration coupling) and the ideal capacity solving method:
//! - `solve_ideal_capacity_for_target`: compute HVAC capacity needed to reach an explicit target
//!
//! All methods operate on pre-allocated buffers owned by `ThermalSolver` — zero per-step heap
//! allocation.

use hares_types::{DomainUpdate, EnvironmentState, PortSlots, ZoneId};

use super::ThermalSolver;

impl ThermalSolver {
    /// Estimate the ideal HVAC capacity needed to reach an explicit target temperature.
    ///
    /// Uses `last_u`, `last_coupling`, and `last_coupled_lu` (previous timestep) as
    /// background. Called by equipment *before* `resolve()` builds the current-step
    /// inputs, so the estimate is one-step stale. Zero allocation — the coupled LU
    /// was cached at the end of the previous `resolve()` call.
    ///
    /// Returns the required capacity in watts (positive = heating, negative = cooling),
    /// or 0.0 if the zone is unknown or solving fails.
    pub fn solve_ideal_capacity_for_target(&self, zone: ZoneId, target_c: f64) -> f64 {
        let Some(&input_idx) = self.wiring.zone_sensible_input_indices.get(&zone) else {
            return 0.0;
        };
        let Some(&output_idx) = self.wiring.zone_output_indices.get(&zone) else {
            return 0.0;
        };

        let result = match &self.last_coupled_lu {
            Some(lu) => self.model.solve_for_scalar_input_coupled(
                &self.x,
                &self.last_u,
                target_c,
                output_idx,
                input_idx,
                lu,
                &self.last_coupling,
            ),
            None => self.model.solve_for_output_input(
                &self.x,
                &self.last_u,
                target_c,
                output_idx,
                input_idx,
            ),
        };

        result.unwrap_or_else(|e| {
            tracing::debug!(
                ?zone,
                ?e,
                "solve_ideal_capacity_for_target failed, returning 0"
            );
            0.0
        })
    }

    /// Core per-timestep resolve: builds input vector, applies semi-implicit
    /// infiltration coupling, runs CN step, and returns the domain update.
    pub(super) fn resolve_internal(
        &mut self,
        ports: &PortSlots,
        env: &EnvironmentState,
    ) -> DomainUpdate {
        let (u, latent_by_zone) = self.build_input_vector(ports, env);

        self.coupling_buf.clear();
        for inf in &self.infiltration_buf {
            if inf.h_inf_w_k.abs() < 1e-15 {
                continue;
            }
            let Some(&state_idx) = self.wiring.zone_state_indices.get(&inf.zone) else {
                continue;
            };
            let Some(&input_idx) = self.wiring.zone_sensible_input_indices.get(&inf.zone) else {
                continue;
            };
            let b_coeff = self.model.b_eff()[(state_idx, input_idx)];
            let d = inf.h_inf_w_k * b_coeff;
            let forcing = inf.h_inf_w_k * inf.t_forcing_c * b_coeff + d * self.x[state_idx];
            self.coupling_buf.push((state_idx, d, forcing));
        }

        if !self.coupling_buf.is_empty() {
            let coupled_lu = self
                .model
                .build_coupled_lu(&mut self.m_scratch, &self.coupling_buf);

            self.model.step_with_coupled_lu_into(
                &self.x,
                &u,
                &mut self.rhs_buf,
                &coupled_lu,
                &self.coupling_buf,
            );

            self.last_coupled_lu = Some(coupled_lu);
        } else {
            self.model.step_into(&self.x, &u, &mut self.rhs_buf);
            self.last_coupled_lu = None;
        }

        std::mem::swap(&mut self.x, &mut self.rhs_buf);
        let y_next = self.model.output(&self.x, &u);

        // Per-boundary convective heat gain diagnostics.
        // T_surface = radiation_frac × T_node + (1 - radiation_frac) × T_zone
        // Q_conv = (T_surface - T_zone) × A / R_film_int
        if !self.config.boundary_diagnostics.is_empty() {
            let zone_output_idx = self
                .wiring
                .zone_output_indices
                .get(&self.config.indoor_zone_id)
                .copied()
                .unwrap_or(0);
            let t_zone = y_next[zone_output_idx];
            for diag in &self.config.boundary_diagnostics {
                let t_node = self.x[diag.inner_state_index];
                let t_surface = diag.radiation_frac * t_node + (1.0 - diag.radiation_frac) * t_zone;
                let q = (t_surface - t_zone) * diag.area_m2 / diag.r_film_int_m2_k_w;
                match diag.category {
                    super::config::BoundaryCategory::Wall => {
                        self.component_gains.wall_heat_gain_w += q
                    }
                    super::config::BoundaryCategory::Floor => {
                        self.component_gains.floor_heat_gain_w += q
                    }
                    super::config::BoundaryCategory::Roof => {
                        self.component_gains.roof_heat_gain_w += q
                    }
                    super::config::BoundaryCategory::Window => {
                        self.component_gains.window_heat_gain_w += q
                    }
                    super::config::BoundaryCategory::InternalMass => {
                        self.component_gains.internal_mass_heat_gain_w += q
                    }
                }
            }
        }

        self.last_u.clone_from(&u);
        self.last_coupling.clone_from(&self.coupling_buf);
        self.u_buf = u;

        self.format_domain_update(&y_next, latent_by_zone)
    }
}
