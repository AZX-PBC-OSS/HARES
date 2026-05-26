//! State-space integration and ideal HVAC capacity solving.
//!
//! Contains `resolve_internal` (the per-timestep ZOH step with semi-implicit
//! infiltration coupling) and the ideal capacity solving method:
//! - `solve_ideal_capacity_for_target`: compute HVAC capacity needed to reach an explicit target
//!
//! All methods operate on pre-allocated buffers owned by `ThermalSolver` -- zero per-step heap
//! allocation.

use hares_physics::film_coefficients::tarp_h_natural;
use hares_physics::solar::{clear_sky_irradiance, perez_tilted_irradiance, solar_position};
use hares_types::{DEFAULT_GROUND_ALBEDO, DomainUpdate, EnvironmentState, PortSlots, ZoneId};
use nalgebra::DVector;

use super::ThermalSolver;
use super::config::StateSpaceWiring;

impl ThermalSolver {
    /// Compute the HVAC capacity (W) required to maintain `target_c` at
    /// design outdoor conditions.
    ///
    /// Sets all non-outdoor inputs to zero (no solar, no internal gains),
    /// sets the outdoor temperature to `design_outdoor_c`, clears infiltration
    /// coupling, and solves for the zone sensible input that drives the zone
    /// temperature to `target_c` at steady state.
    ///
    /// The steady-state capacity is computed from the DC gain (steady-state gain)
    /// of the state-space model. This avoids the overshoot that a one-step back-
    /// solve from a cold-start state produces for high-R envelopes: the one-step
    /// solve includes the energy to warm up thermally massive envelope nodes from
    /// their uniform initial temperatures, inflating the apparent capacity by
    /// multiples of the true steady-state load.
    ///
    /// The DC gain from the HVAC input column to the zone temperature output row
    /// is computed by evaluating the steady-state response with zero HVAC, then
    /// with a 1 W unit perturbation. Because the state-space model is linear,
    /// the gain is constant and a single perturbation gives the exact slope.
    ///
    /// Returns the required capacity in watts (positive = heating, negative = cooling),
    /// or 0.0 if the zone is unknown or solving fails.
    ///
    /// This method does NOT modify the solver's persistent state (`x`, `last_u`,
    /// or `last_coupled_lu`).
    pub fn autosize_capacity(&self, zone: ZoneId, target_c: f64, design_outdoor_c: f64) -> f64 {
        let Some(&input_idx) = self.wiring.zone_sensible_input_indices.get(&zone) else {
            return 0.0;
        };
        let Some(&output_idx) = self.wiring.zone_output_indices.get(&zone) else {
            return 0.0;
        };

        // Build a design-condition input vector: outdoor temp = design_outdoor_c,
        // all other inputs (solar, ground, internal gains) = 0.0.
        let mut u_design = self.last_u.clone();
        u_design.fill(0.0);

        // Set outdoor temperature at the outdoor temp column(s).
        for &col in &self.wiring.outdoor_temp_input_indices {
            if col < u_design.len() {
                u_design[col] = design_outdoor_c;
            }
        }

        // Ground temperature columns: approximate as design_outdoor_c for
        // conservative sizing (cold ground in heating, warm ground in cooling).
        for &col in &self.wiring.ground_temp_input_indices {
            if col < u_design.len() {
                u_design[col] = design_outdoor_c;
            }
        }

        // ── Steady-state capacity via DC gain ─────────────────────────────
        self.dc_gain_autosize(
            &mut u_design,
            target_c,
            zone,
            input_idx,
            output_idx,
            "autosize_capacity",
        )
    }

    /// Access the wiring (zone ↔ state-space index mappings) for autosizing.
    pub fn wiring(&self) -> &StateSpaceWiring {
        &self.wiring
    }

    /// Compute the required HVAC capacity from the DC gain of the state-space
    /// model, given a fully-prepared design-condition input vector `u_design`.
    ///
    /// Evaluates `steady_state` twice — once with the HVAC input at zero, once
    /// with a 1 W unit perturbation — to measure the steady-state gain (K/W)
    /// of the zone temperature output with respect to the HVAC input column.
    /// Because the state-space model is linear, a single perturbation gives
    /// the exact steady-state gain.
    ///
    /// Returns the required capacity in watts (positive = heating, negative =
    /// cooling), or 0.0 on failure.  `caller` is used for `tracing::warn!`
    /// context strings so the two upstream callers can be distinguished in logs.
    fn dc_gain_autosize(
        &self,
        u_design: &mut DVector<f64>,
        target_c: f64,
        zone: ZoneId,
        input_idx: usize,
        output_idx: usize,
        caller: &str,
    ) -> f64 {
        let x_ss_base = match self.model.steady_state(u_design) {
            Some(x) => x,
            None => {
                tracing::warn!(
                    ?zone,
                    target_c,
                    "{caller}: steady-state solve failed for baseline, returning 0"
                );
                return 0.0;
            }
        };
        let y_ss_base = self.model.output(&x_ss_base, u_design);

        // Perturb the HVAC input by 1 W, recompute the steady state, and
        // measure the output difference to get the DC gain in K/W.
        const UNIT_PERTURBATION_W: f64 = 1.0;
        let saved = u_design[input_idx];
        u_design[input_idx] = UNIT_PERTURBATION_W;
        let x_ss_pert = match self.model.steady_state(u_design) {
            Some(x) => x,
            None => {
                u_design[input_idx] = saved;
                tracing::warn!(
                    ?zone,
                    target_c,
                    "{caller}: steady-state solve failed for perturbation, returning 0"
                );
                return 0.0;
            }
        };
        let y_ss_pert = self.model.output(&x_ss_pert, u_design);
        u_design[input_idx] = saved;

        let gain_k_per_w = y_ss_pert[output_idx] - y_ss_base[output_idx];
        if gain_k_per_w.abs() <= f64::EPSILON {
            tracing::warn!(
                ?zone,
                target_c,
                gain_k_per_w,
                "{caller}: zero steady-state gain, returning 0"
            );
            return 0.0;
        }

        let baseline_zone_temp = y_ss_base[output_idx];
        (target_c - baseline_zone_temp) / gain_k_per_w
    }

    /// Compute the cooling HVAC capacity (W) required to maintain `target_c` at
    /// design outdoor conditions with peak solar gains.
    ///
    /// Unlike [`autosize_capacity`] which zeros all solar inputs, this method
    /// computes clear-sky solar irradiance for July 21 solar noon at the given
    /// latitude/longitude and applies per-surface solar gains to the input vector
    /// before solving. This produces a higher (more realistic) cooling capacity
    /// for buildings with significant window area facing the summer solar azimuth.
    ///
    /// Per ACCA Manual J-2016: cooling design uses peak solar conditions
    /// (July 21 solar noon). Solar irradiance is computed using the ASHRAE
    /// clear-sky model and the Perez (1990) anisotropic tilted irradiance model
    /// for each surface.
    ///
    /// As with [`autosize_capacity`], the steady-state capacity is computed
    /// from the DC gain of the state-space model rather than a one-step back-
    /// solve from the cold-start state. This avoids inflating the apparent
    /// capacity due to transient wall-mass warm-up.
    ///
    /// Returns the required capacity in watts (positive = heating, negative = cooling),
    /// or 0.0 if the zone is unknown or solving fails.
    ///
    /// This method does NOT modify the solver's persistent state (`x`, `last_u`,
    /// or `last_coupled_lu`).
    pub fn autosize_capacity_cooling(
        &self,
        zone: ZoneId,
        target_c: f64,
        design_outdoor_c: f64,
        site_lat_deg: f64,
        site_lon_deg: f64,
    ) -> f64 {
        use chrono::{Datelike, FixedOffset, TimeZone};

        let Some(&input_idx) = self.wiring.zone_sensible_input_indices.get(&zone) else {
            return 0.0;
        };
        let Some(&output_idx) = self.wiring.zone_output_indices.get(&zone) else {
            return 0.0;
        };

        // ── Build design-condition input vector ────────────────────────────
        let mut u_design = self.last_u.clone();
        u_design.fill(0.0);

        // Outdoor temperature
        for &col in &self.wiring.outdoor_temp_input_indices {
            if col < u_design.len() {
                u_design[col] = design_outdoor_c;
            }
        }

        // Ground temperature: approximate as design_outdoor_c for
        // conservative sizing.
        for &col in &self.wiring.ground_temp_input_indices {
            if col < u_design.len() {
                u_design[col] = design_outdoor_c;
            }
        }

        // ── Compute July 21 solar noon position and clear-sky irradiance ──
        // July 21 is day 202 (non-leap year). Use 2025 (non-leap) for day-of-year.
        let mut dt = FixedOffset::east_opt(0)
            .and_then(|tz| tz.with_ymd_and_hms(2025, 7, 21, 12, 0, 0).single());
        if dt.is_none() {
            // Longitude == -180.0 edge case; fall back far enough east.
            dt = FixedOffset::east_opt(12 * 3600)
                .and_then(|tz| tz.with_ymd_and_hms(2025, 7, 21, 12, 0, 0).single());
        }
        // Adjust UTC hour so local solar noon is at 12:00 local (approximate).
        let utc_hour = (12.0 - site_lon_deg / 15.0).round() as i64;
        let base_utc = dt.expect("valid July 21 noon UTC");
        let local_noon = base_utc + chrono::Duration::hours((utc_hour - 12).clamp(-12, 12));
        let pos = solar_position(site_lat_deg, site_lon_deg, local_noon);
        let doy = local_noon.ordinal();

        let (dni_clear, dhi_clear, ghi_clear) = clear_sky_irradiance(doy, pos.altitude_deg);
        let solar_zenith_deg = (90.0 - pos.altitude_deg).max(0.0);

        // ── Apply per-window solar gains ───────────────────────────────────
        for (surface_id, win_props) in &self.config.window_properties {
            let Some(&solar_idx) = self.wiring.solar_input_indices.get(surface_id) else {
                continue;
            };
            if solar_idx >= u_design.len() {
                continue;
            }

            let irr = perez_tilted_irradiance(
                *surface_id,
                ghi_clear,
                dni_clear,
                dhi_clear,
                solar_zenith_deg,
                pos.azimuth_deg,
                win_props.tilt_deg,
                win_props.azimuth_deg,
                doy,
                DEFAULT_GROUND_ALBEDO,
            );

            // Design-day approximation: total window solar gain = POA × SHGC × area.
            // This bypasses the per-timestep beam/diffuse split and IAM correction
            // used in full simulation, but captures the dominant solar heat gain
            // for equipment sizing at a single solar position.
            let poa_w_m2 = irr.direct_w_m2 + irr.diffuse_w_m2 + irr.reflected_w_m2;
            u_design[solar_idx] += poa_w_m2 * win_props.shgc * win_props.area_m2;
        }

        // ── Apply per-opaque-surface solar gains ───────────────────────────
        for info in &self.config.exterior_surfaces {
            if info.input_index >= u_design.len() {
                continue;
            }
            // Skip windows — they are handled above via SHGC.
            if self.config.window_properties.contains_key(&info.surface_id) {
                continue;
            }

            let irr = perez_tilted_irradiance(
                info.surface_id,
                ghi_clear,
                dni_clear,
                dhi_clear,
                solar_zenith_deg,
                pos.azimuth_deg,
                info.tilt_deg,
                info.azimuth_deg,
                doy,
                DEFAULT_GROUND_ALBEDO,
            );

            let poa_w_m2 = irr.direct_w_m2 + irr.diffuse_w_m2 + irr.reflected_w_m2;
            u_design[info.input_index] += info.absorptance * info.area_m2 * poa_w_m2;
        }

        // ── Steady-state capacity via DC gain ─────────────────────────────
        // Compute the zone temperature at steady state with zero HVAC input
        // but all other design inputs (outdoor temp, solar gains) present.
        self.dc_gain_autosize(
            &mut u_design,
            target_c,
            zone,
            input_idx,
            output_idx,
            "autosize_capacity_cooling",
        )
    }

    /// Estimate the ideal HVAC capacity needed to reach an explicit target temperature.
    ///
    /// Uses `last_u`, `last_coupling`, and `last_coupled_lu` as background.
    /// When `prepare_inputs()` has been called first (two-phase path), these
    /// contain current-step weather/solar/infiltration data. Zero allocation --
    /// the coupled LU is cached by `prepare_inputs` or the previous `integrate`.
    ///
    /// Returns the required capacity in watts (positive = heating, negative = cooling),
    /// or 0.0 if the zone is unknown or solving fails.
    ///
    /// Logging behaviour: failure emits `tracing::warn!` with structured context
    /// (zone, target, zone temp, outdoor temp, error) on the first failure in a
    /// consecutive run; subsequent consecutive failures are suppressed to `debug!`
    /// to avoid log flood in pathological runs. When the solver next succeeds after
    /// one or more failures, a single `info!` recovery log is emitted.
    pub fn solve_ideal_capacity_for_target(&mut self, zone: ZoneId, target_c: f64) -> f64 {
        let Some(&input_idx) = self.wiring.zone_sensible_input_indices.get(&zone) else {
            return 0.0;
        };
        let Some(&output_idx) = self.wiring.zone_output_indices.get(&zone) else {
            return 0.0;
        };

        let total = match &self.last_coupled_lu {
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

        let t_zone = self
            .wiring
            .zone_state_indices
            .get(&zone)
            .map(|&idx| self.x[idx])
            .unwrap_or(f64::NAN);
        let t_out = self
            .wiring
            .outdoor_temp_input_indices
            .first()
            .map(|&idx| self.last_u[idx])
            .unwrap_or(f64::NAN);
        let capacity_value = self.last_u[input_idx];

        match total.map(|raw| raw - capacity_value) {
            Ok(capacity) => {
                if self.ideal_capacity_warned_zones.remove(&zone) {
                    let count = self
                        .ideal_capacity_failure_counts
                        .remove(&zone)
                        .unwrap_or(0);
                    tracing::info!(
                        zone_id = zone.0,
                        consecutive_failures = count,
                        "solve_ideal_capacity_for_target: recovered after {count} \
                         consecutive failures"
                    );
                } else {
                    self.ideal_capacity_failure_counts.remove(&zone);
                }
                capacity
            }
            Err(e) => {
                let count = self
                    .ideal_capacity_failure_counts
                    .entry(zone)
                    .and_modify(|c| *c += 1)
                    .or_insert(1);
                if self.ideal_capacity_warned_zones.insert(zone) {
                    tracing::warn!(
                        zone_id = zone.0,
                        target_c,
                        t_zone_c = t_zone,
                        oat_c = t_out,
                        capacity_w = capacity_value,
                        error = %e,
                        "solve_ideal_capacity_for_target failed, returning 0"
                    );
                } else {
                    tracing::debug!(
                        zone_id = zone.0,
                        target_c,
                        consecutive_failures = count,
                        error = %e,
                        "solve_ideal_capacity_for_target failed (suppressed), returning 0"
                    );
                }
                0.0
            }
        }
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
    /// Does NOT cache u for the integration step -- `integrate` rebuilds it from
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
                        tilt_deg,
                        radiation_frac,
                        category,
                    } => {
                        let t_node = self.x[*inner_state_index];
                        let t_surface = radiation_frac * t_node + (1.0 - radiation_frac) * t_zone;
                        let delta_t_k = (t_surface - t_zone).abs();
                        // 0.1 K floor for numerical stability at exactly-equal
                        // temperatures (matches the floor in film_coefficients.rs
                        // exterior TARP path). At ΔT = 0 K, h_conv = 0 (cbrt(0) = 0),
                        // which is physically correct — no buoyancy-driven convection
                        // without a temperature difference.
                        let delta_t_clamped = delta_t_k.max(0.1);
                        // above_hotter: true when the surface is warmer than the
                        // zone air (heat flows from surface to air — enhanced
                        // convection for upward heat flow, reduced for downward).
                        let above_hotter = t_surface > t_zone;
                        // Per-step TARP natural convection coefficient [W/(m²·K)].
                        // Replaces the frozen init-time film resistance with the
                        // ΔT-dependent EnergyPlus default interior convection model.
                        // Eq. 90–92, Walton 1983 NBSSIR 83-2655.
                        // Note: the A-matrix conductance is still frozen (per
                        // T-0082 Known Limitations); this diagnostic-only fix
                        // reports the physically correct convective flux without
                        // changing the state-space discretization.
                        let h_nat = tarp_h_natural(*tilt_deg, delta_t_clamped, above_hotter);
                        let q_conv_w = h_nat * area_m2 * (t_surface - t_zone);
                        (q_conv_w, *category)
                    }
                    BoundaryDiagnosticInfo::SteadyState {
                        ua_w_k,
                        driving_temp,
                        category,
                    } => {
                        let t_driving = match driving_temp {
                            DrivingTemp::Outdoor => self.cached_outdoor_temp_c,
                            DrivingTemp::Ground { depth_m } => {
                                // Look up the cached Kusuda temperature for this depth.
                                // The cached_ground_temps_c vec is parallel to
                                // wiring.ground_temp_input_depths_m; depths are
                                // matched by rounding to millimetre precision.
                                let key = (depth_m * 1000.0).round() as i64;
                                self.cached_ground_temps_c
                                    .iter()
                                    .zip(self.wiring.ground_temp_input_depths_m.iter())
                                    .find(|(_, d)| (*d * 1000.0).round() as i64 == key)
                                    .map(|(t, _)| *t)
                                    .unwrap_or(0.0)
                            }
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

        // ── Zone energy balance closure check ────────────────────────────────────
        //
        // Per-zone first-law check: compare C_zone × ΔT / dt against the direct
        // port sensible injection for each conditioned zone. EnergyPlus Engineering
        // Reference (2024) "Basis for the Zone and Air System Integration" states
        // that the heat balance method must conserve energy; ASHRAE HoF 2021 Ch.18
        // codifies this as the fundamental requirement of any zone heat balance.
        //
        // This check is intentionally loose: wall-mass energy redistribution
        // (multi-node RC models) and envelope conduction losses are not accounted
        // for here — they flow through the A-matrix coupling and are balanced
        // by construction of the ZOH state equation. The check catches gross
        // errors: sign flips in port injection, wrong B_d columns, mis-wired
        // port indices. A properly configured model should not approach the
        // assertion threshold; the warning threshold will fire on multi-node
        // models during transient conditions where wall-mass exchange dominates
        // the zone air energy balance (~kW-range residuals).
        //
        // Thresholds are wider than the EnergyPlus 0.001 W check because HARES
        // multi-node models distribute energy across wall-mass nodes. The actual
        // residual for a working multi-node model can approach 6 kW during step
        // changes (see invariants.rs:2770-2777 comment).
        self.energy_balance_residuals.clear();
        for (&zone, &state_idx) in &self.wiring.zone_state_indices {
            let Some(&c_zone) = self.wiring.c_zone_j_k.get(&zone) else {
                continue;
            };
            let Some(&input_idx) = self.wiring.zone_sensible_input_indices.get(&zone) else {
                continue;
            };
            let t_prev = self.rhs_buf[state_idx];
            let t_next = self.x[state_idx];
            let delta_stored = c_zone * (t_next - t_prev) / self.dt_s;
            let q_port = u[input_idx];
            let residual = (delta_stored - q_port).abs();

            self.energy_balance_residuals.insert(zone, residual);

            // Diagnostic: residual above 5 kW may indicate port-wiring errors.
            // Wall-mass energy redistribution in multi-node RC models can produce
            // residuals of several kW when q_port is large and zone air capacitance
            // is small relative to wall-node capacitances (see invariants.rs:2770-2777).
            // The 5 kW threshold is a practical compromise: catches gross errors
            // (sign flips, missing B_d entries) while tolerating transient wall-mass
            // exchange in models with realistic envelope construction.
            // No assertion is performed — the residual magnitude depends on model
            // complexity and cannot be bounded by a single constant.
            if residual > 5000.0 {
                tracing::warn!(
                    zone = zone.0,
                    residual_w = residual,
                    delta_stored_w = delta_stored,
                    q_port_w = q_port,
                    "zone energy balance residual exceeds 5000 W — \
                     possible port wiring or sign error"
                );
            }
        }

        // ── Full-system energy balance diagnostic ───────────────────────────────
        //
        // Compute Σ C_i × ΔT_i / dt over ALL thermal nodes (not just zone air).
        // This accounts for wall-mass energy redistribution, conduction through
        // the A-matrix, and all B_d column contributions. The ZOH discretization
        // conserves energy by construction, so at steady state this approaches
        // zero. Non-zero values during transients reflect energy being stored
        // in or released from thermal capacitances.
        //
        // This is a diagnostic — not an assertion or correction. The per-zone
        // check above catches gross port-wiring errors; this full-system check
        // provides visibility into total stored energy for multi-node RC models.
        //
        // Reference: EnergyPlus Engineering Reference
        // "Basis for the Zone and Air System Integration" — the heat balance
        // method requires that the sum of all thermal energy flows across the
        // system boundary equals the rate of change of stored energy in all
        // thermal capacitances.
        if !self.wiring.node_capacitances.is_empty() {
            let mut stored_energy_w: f64 = 0.0;
            for (node_id, c_j_k) in &self.wiring.node_capacitances {
                if let Some(&idx) = self.wiring.node_index.get(node_id) {
                    if idx < self.x.len() && idx < self.rhs_buf.len() {
                        // After the swap, self.x is T_next and self.rhs_buf is T_prev.
                        stored_energy_w += c_j_k * (self.x[idx] - self.rhs_buf[idx]) / self.dt_s;
                    }
                }
            }
            tracing::debug!(stored_energy_w, "full-system stored energy change rate [W]");
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
