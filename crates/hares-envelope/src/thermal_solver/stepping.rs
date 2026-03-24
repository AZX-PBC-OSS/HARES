//! State-space integration and ideal HVAC capacity solving.
//!
//! Contains `resolve_internal` (the per-timestep CN step with semi-implicit
//! infiltration coupling) and `solve_ideal_capacity` (one-step-stale HVAC
//! back-calculation). All methods operate on pre-allocated buffers owned by
//! `ThermalSolver` — zero per-step heap allocation.

use hares_types::{DomainUpdate, EnvironmentState, PortSlots, ZoneId};

use super::ThermalSolver;
use super::zone_setpoint_c;

impl ThermalSolver {
    /// Estimate the ideal HVAC capacity needed to maintain the zone setpoint.
    ///
    /// Uses `last_u`, `last_coupling`, and `last_coupled_lu` (previous timestep) as
    /// background. Called by equipment *before* `resolve()` builds the current-step
    /// inputs, so the estimate is one-step stale. Zero allocation — the coupled LU
    /// was cached at the end of the previous `resolve()` call.
    pub fn solve_ideal_capacity(&self, env: &EnvironmentState, zone: ZoneId) -> f64 {
        let Some(&input_idx) = self.wiring.zone_sensible_input_indices.get(&zone) else {
            return 0.0;
        };
        let Some(&output_idx) = self.wiring.zone_output_indices.get(&zone) else {
            return 0.0;
        };
        let y_target = zone_setpoint_c(&self.config, env, zone);

        let result = match &self.last_coupled_lu {
            Some(lu) => self.model.solve_for_scalar_input_coupled(
                &self.x,
                &self.last_u,
                y_target,
                output_idx,
                input_idx,
                lu,
                &self.last_coupling,
            ),
            None => self.model.solve_for_output_input(
                &self.x,
                &self.last_u,
                y_target,
                output_idx,
                input_idx,
            ),
        };

        result.unwrap_or_else(|e| {
            tracing::debug!(?zone, ?e, "solve_ideal_capacity failed, returning 0");
            0.0
        })
    }

    /// Core per-timestep resolve: builds input vector, applies semi-implicit
    /// infiltration coupling, runs CN step, and returns the domain update.
    pub(super) fn resolve_internal(
        &mut self,
        ports: &PortSlots,
        env: &EnvironmentState,
        ideal_hvac_zones: &[ZoneId],
    ) -> DomainUpdate {
        let (mut u, latent_by_zone, infiltration_couplings) =
            self.build_input_vector(ports, env);

        // Build per-step fully-implicit coupling tuples from infiltration.
        //
        // Infiltration conductance h_inf [W/K] enters the zone air heat balance as
        // q_inf = h_inf * (T_out - T_zone). Following EnergyPlus Engineering Reference
        // §13.3, the temperature-dependent term is treated fully implicitly to guarantee
        // monotonic, oscillation-free convergence even when the infiltration time
        // constant is much smaller than the timestep.
        //
        // The coupling API adds d to M's diagonal and subtracts d from N's diagonal.
        // For fully implicit treatment we want d on M only, so the forcing includes a
        // compensation term `+d * x[k]` to cancel the unwanted N-side subtraction:
        //   d = h_inf * b_eff[(state, input)]
        //   f = h_inf * T_out * b_eff + d * x[state]
        self.coupling_buf.clear();
        for inf in &infiltration_couplings {
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
            // Compensation: the coupling API subtracts d*x[k] from the explicit side,
            // but backward Euler wants zero on the explicit side, so add d*x[k] back.
            let forcing = inf.h_inf_w_k * inf.t_forcing_c * b_coeff + d * self.x[state_idx];
            self.coupling_buf.push((state_idx, d, forcing));
        }

        if !self.coupling_buf.is_empty() {
            // Coupled path: build modified LU once, reuse for HVAC solve, step, and cache.
            let coupled_lu =
                self.model
                    .build_coupled_lu(&mut self.m_scratch, &self.coupling_buf);

            for &zone in ideal_hvac_zones {
                let Some(&input_idx) = self.wiring.zone_sensible_input_indices.get(&zone) else {
                    continue;
                };
                let Some(&output_idx) = self.wiring.zone_output_indices.get(&zone) else {
                    continue;
                };
                let target = zone_setpoint_c(&self.config, env, zone);

                if let Ok(q) = self.model.solve_for_scalar_input_coupled(
                    &self.x,
                    &u,
                    target,
                    output_idx,
                    input_idx,
                    &coupled_lu,
                    &self.coupling_buf,
                ) {
                    u[input_idx] = q;
                }
            }

            self.model.step_with_coupled_lu_into(
                &self.x,
                &u,
                &mut self.rhs_buf,
                &coupled_lu,
                &self.coupling_buf,
            );

            self.last_coupled_lu = Some(coupled_lu);
        } else {
            for &zone in ideal_hvac_zones {
                let Some(&input_idx) = self.wiring.zone_sensible_input_indices.get(&zone) else {
                    continue;
                };
                let Some(&output_idx) = self.wiring.zone_output_indices.get(&zone) else {
                    continue;
                };
                let target = zone_setpoint_c(&self.config, env, zone);

                if let Ok(q) =
                    self.model
                        .solve_for_output_input(&self.x, &u, target, output_idx, input_idx)
                {
                    u[input_idx] = q;
                }
            }

            self.model.step_into(&self.x, &u, &mut self.rhs_buf);
            self.last_coupled_lu = None;
        }

        std::mem::swap(&mut self.x, &mut self.rhs_buf);
        let y_next = self.model.output(&self.x, &u);
        self.last_u.clone_from(&u);
        self.last_coupling.clone_from(&self.coupling_buf);
        self.u_buf = u;

        self.format_domain_update(&y_next, latent_by_zone)
    }
}
