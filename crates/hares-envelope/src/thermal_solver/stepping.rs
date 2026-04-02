//! State-space integration and ideal HVAC capacity solving.
//!
//! Contains `resolve_internal` (the per-timestep ZOH step with semi-implicit
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
    /// Uses `last_u`, `last_coupling`, and `last_coupled_lu` as background.
    /// When `prepare_inputs()` has been called first (two-phase path), these
    /// contain current-step weather/solar/infiltration data. Zero allocation —
    /// the coupled LU is cached by `prepare_inputs` or the previous `integrate`.
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
            Some(lu) => {
                let coupling = crate::CouplingData {
                    lu,
                    couplings: &self.last_coupling,
                };
                self.model.solve_for_scalar_input_coupled(
                    &self.x,
                    &self.last_u,
                    target_c,
                    output_idx,
                    input_idx,
                    &coupling,
                )
            }
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

    fn build_coupling(&mut self) {
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
    }

    /// Phase 1: build input vector and coupling from current weather/solar/infiltration.
    /// Stores results in `last_u`, `last_coupling`, `last_coupled_lu` so that
    /// `solve_ideal_capacity_for_target` sees current-step data.
    /// Does NOT cache u for the integration step — `integrate` rebuilds it from
    /// post-dispatch ports.
    pub(super) fn prepare_inputs_inner(&mut self, ports: &PortSlots, env: &EnvironmentState) {
        let saved_ext_temps = self.exterior_surface_temps.clone();
        let (u, _latent) = self.build_input_vector(ports, env);
        self.exterior_surface_temps = saved_ext_temps;

        self.build_coupling();

        if !self.coupling_buf.is_empty() {
            let coupled_lu = self
                .model
                .build_coupled_lu(&mut self.m_scratch, &self.coupling_buf);
            self.last_coupled_lu = Some(coupled_lu);
        } else {
            self.last_coupled_lu = None;
        }

        self.last_u.clone_from(&u);
        self.last_coupling.clone_from(&self.coupling_buf);
        self.u_buf = u;
    }

    /// Phase 2: rebuild u from post-dispatch ports, rebuild coupling, run ZOH integration.
    pub(super) fn integrate_inner(
        &mut self,
        ports: &PortSlots,
        env: &EnvironmentState,
        out: &mut DomainUpdate,
    ) {
        let (u, latent_by_zone) = self.build_input_vector(ports, env);

        self.build_coupling();

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

        #[cfg(any(debug_assertions, feature = "observe_detailed"))]
        if !self.config.boundary_diagnostics.is_empty() {
            let zone_output_idx = self
                .wiring
                .zone_output_indices
                .get(&self.config.indoor_zone_id)
                .copied()
                .unwrap_or(0);
            let t_zone = y_next[zone_output_idx];
            for diag in &self.config.boundary_diagnostics {
                use super::config::{BoundaryDiagnosticInfo, DrivingTemp};
                let (q, category) = match diag {
                    BoundaryDiagnosticInfo::RCNode {
                        inner_state_index,
                        area_m2,
                        r_film_int_m2_k_w,
                        radiation_frac,
                        category,
                    } => {
                        let t_node = self.x[*inner_state_index];
                        let t_surface = radiation_frac * t_node + (1.0 - radiation_frac) * t_zone;
                        (
                            (t_surface - t_zone) * area_m2 / r_film_int_m2_k_w,
                            *category,
                        )
                    }
                    BoundaryDiagnosticInfo::SteadyState {
                        ua_w_k,
                        driving_temp,
                        category,
                    } => {
                        let t_driving = match driving_temp {
                            DrivingTemp::Outdoor => self.cached_outdoor_temp_c,
                            DrivingTemp::Ground => self.cached_ground_temp_c,
                        };
                        (ua_w_k * (t_driving - t_zone), *category)
                    }
                };
                match category {
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

        self.format_domain_update(&y_next, latent_by_zone, out);
    }

    /// Convenience: calls both phases with the same ports/env.
    /// Used by `DomainSolver::resolve` for non-dwelling callers.
    pub(super) fn resolve_internal(
        &mut self,
        ports: &PortSlots,
        env: &EnvironmentState,
        out: &mut DomainUpdate,
    ) {
        self.prepare_inputs_inner(ports, env);
        self.integrate_inner(ports, env, out);
    }
}
