//! State-space integration and ideal HVAC capacity solving.
//!
//! Contains `resolve_internal` (the per-timestep ZOH step with semi-implicit
//! infiltration coupling), the ideal capacity solving method, and design-day
//! autosizing simulation.
//!
//! - `solve_ideal_capacity_for_target`: compute HVAC capacity needed to reach an explicit target
//! - `autosize_design_day_heating`: iterative design-day simulation for heating sizing
//! - `autosize_design_day_cooling`: iterative design-day simulation for cooling sizing
//!
//! All methods operate on pre-allocated buffers owned by `ThermalSolver` -- zero per-step heap
//! allocation.

use hares_physics::film_coefficients::tarp_h_natural;
use hares_physics::solar::{clear_sky_irradiance, perez_tilted_irradiance, solar_position};
use hares_types::{DEFAULT_GROUND_ALBEDO, DomainUpdate, EnvironmentState, PortSlots, ZoneId};
use nalgebra::DVector;

use super::CoupledState;
use super::ThermalSolver;
use super::config::{FilmCoefficientModel, StateSpaceWiring};

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
    /// or `last_coupled_state`).
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
        // Perturbation is base + 1 W (not a fixed 1 W) so that internal
        // gains and other non-zero base contributions are preserved.
        const UNIT_PERTURBATION_W: f64 = 1.0;
        let saved = u_design[input_idx];
        u_design[input_idx] = saved + UNIT_PERTURBATION_W;
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
    /// design outdoor conditions with peak solar gains and internal gains.
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
    /// for each surface. Internal gains from occupancy, lighting, and appliances
    /// are also included per ACCA Manual J-2016 §7 (cooling load includes
    /// internal gains).
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
    /// or `last_coupled_state`).
    pub fn autosize_capacity_cooling(
        &self,
        zone: ZoneId,
        target_c: f64,
        design_outdoor_c: f64,
        site_lat_deg: f64,
        site_lon_deg: f64,
        internal_gains_w: f64,
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

        // ── Internal gains ────────────────────────────────────────────────
        // ACCA Manual J-2016 §7: cooling design loads must include sensible
        // internal gains from occupancy, lighting, and appliances.
        // ASHRAE HoF 2021 Ch.18 Table 1: occupant sensible gain = 66 W/person,
        //     latent gain = 51.2 W/person at typical indoor conditions.
        // ASHRAE 62.2-2022 Appendix B: typical lights/plug density ≈ 5 W/m².
        // Internal gains enter through the zone sensible input column; the DC
        // gain method saves/restores this value, so the perturbation is
        // correctly always 1 W while the baseline includes internal gains.
        u_design[input_idx] += internal_gains_w;

        // ── Invariant: cooling internal gains must be non-negative and
        //    plausible for a single-family residence (< 5 kW sensible) ──
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            assert!(
                internal_gains_w >= 0.0,
                "autosize_capacity_cooling: internal_gains_w ({}) must be non-negative",
                internal_gains_w
            );
            assert!(
                internal_gains_w < 5_000.0,
                "autosize_capacity_cooling: internal_gains_w ({}) implausibly large \
                 for a single-family residence (≥ 5 kW)",
                internal_gains_w
            );
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
    /// Uses `last_u`, `last_coupling`, and `last_coupled_state` as background.
    /// When `prepare_inputs()` has been called first (two-phase path), these
    /// contain current-step weather/solar/infiltration data. Zero allocation —
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

        let total = match &self.last_coupled_state {
            CoupledState::LU(lu) => {
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
            CoupledState::Identity => self.model.solve_for_scalar_input_identity_coupled(
                &self.x,
                &self.last_u,
                target_c,
                output_idx,
                input_idx,
                &self.last_coupling,
            ),
            CoupledState::Uncoupled => self.model.solve_for_output_input(
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
                self.last_good_capacity_w.insert(zone, capacity);
                let prethreshold = self.ideal_capacity_warned_zones.remove(&zone);
                let degraded = self.ideal_capacity_degraded_warned_zones.remove(&zone);
                if prethreshold || degraded {
                    let count = self
                        .ideal_capacity_failure_counts
                        .remove(&zone)
                        .unwrap_or(0);
                    #[cfg(feature = "observe")]
                    {
                        self.consecutive_nonconvergence_count.remove(&zone);
                    }
                    tracing::info!(
                        zone_id = zone.0,
                        consecutive_failures = count,
                        degraded_fallback = degraded,
                        "solve_ideal_capacity_for_target: recovered after {count} \
                         consecutive failures"
                    );
                } else {
                    self.ideal_capacity_failure_counts.remove(&zone);
                    #[cfg(feature = "observe")]
                    {
                        self.consecutive_nonconvergence_count.remove(&zone);
                    }
                }
                capacity
            }
            Err(e) => {
                let count = self
                    .ideal_capacity_failure_counts
                    .entry(zone)
                    .and_modify(|c| *c += 1)
                    .or_insert(1);

                #[cfg(feature = "observe")]
                {
                    self.consecutive_nonconvergence_count
                        .entry(zone)
                        .and_modify(|c| *c += 1)
                        .or_insert(1);
                }

                let threshold = self.config.ideal_capacity_degraded_threshold;
                if *count >= threshold {
                    if let Some(&last_good) = self.last_good_capacity_w.get(&zone) {
                        self.ideal_capacity_degraded_zones.insert(zone);
                        if self.ideal_capacity_degraded_warned_zones.insert(zone) {
                            tracing::error!(
                                zone_id = zone.0,
                                target_c,
                                t_zone_c = t_zone,
                                oat_c = t_out,
                                consecutive_failures = count,
                                last_good_capacity_w = last_good,
                                error = %e,
                                "solve_ideal_capacity_for_target: threshold {threshold} exceeded, \
                                 falling back to last-good capacity {last_good:.0} W"
                            );
                        } else {
                            tracing::debug!(
                                zone_id = zone.0,
                                target_c,
                                consecutive_failures = count,
                                last_good_capacity_w = last_good,
                                "solve_ideal_capacity_for_target: degraded fallback \
                                 (suppressed), using last-good {last_good:.0} W"
                            );
                        }
                        return last_good;
                    }
                }

                if self.ideal_capacity_warned_zones.insert(zone) {
                    tracing::warn!(
                        zone_id = zone.0,
                        target_c,
                        t_zone_c = t_zone,
                        oat_c = t_out,
                        capacity_w = capacity_value,
                        consecutive_failures = count,
                        threshold,
                        error = %e,
                        "solve_ideal_capacity_for_target failed, returning 0"
                    );
                } else {
                    tracing::debug!(
                        zone_id = zone.0,
                        target_c,
                        consecutive_failures = count,
                        threshold,
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

    /// Compute per-step interior convection correction using semi-implicit coupling.
    ///
    /// For each interior boundary, evaluates `tarp_h_natural(tilt_deg, |T_surface − T_zone|, …)`
    /// and pushes per-node coupling entries into `self.coupling_buf` for the correction
    /// Δh = h_tarp − h_static (difference between per-step TARP coefficient and the frozen
    /// ASHRAE Simple value in the A-matrix).
    ///
    /// The correction is only applied when Δh > 0 (h_tarp exceeds the frozen value).
    /// When Δh < 0 (h_tarp is lower than the frozen value, common for walls/ceilings at
    /// typical indoor ΔT), the A-matrix already overestimates convection — a conservative
    /// overestimate that is numerically stable. Applying a negative semi-implicit diagonal
    /// would make the M matrix ill-conditioned. See Known Limitations.
    ///
    /// When applied (Δh > 0), each surface–zone pair produces two coupling entries:
    /// - Surface node: `d = dt·Δh·A / C_surface` (implicit diagonal, stabilising)
    ///   `forcing = dt·Δh·A·T_zone / C_surface` (explicit off-diagonal)
    /// - Zone node:    `d = dt·Δh·A / C_zone` (implicit diagonal, stabilising)
    ///   `forcing = dt·Δh·A·T_surface / C_zone` (explicit off-diagonal)
    ///
    /// Reference: Walton, G. N. 1983. TARP Reference Manual, NBSSIR 83-2655, Eqs. 90–92.
    fn apply_convection_forcing(&mut self) {
        if self.config.film_coefficient_model != FilmCoefficientModel::PerStepTarp
            || self.config.interior_convection_injections.is_empty()
        {
            return;
        }

        let dt = self.dt_s;

        for (i, inj) in self
            .config
            .interior_convection_injections
            .iter()
            .enumerate()
        {
            let t_surface = self.x[inj.surface_state_index];
            let t_zone = self.x[inj.zone_state_index];

            let delta_t_k = (t_surface - t_zone).abs();

            if delta_t_k < 1e-15 {
                continue;
            }

            let above_hotter = t_surface > t_zone;
            let h_tarp = tarp_h_natural(inj.tilt_deg, delta_t_k, above_hotter);

            #[cfg(any(debug_assertions, feature = "check_invariants"))]
            {
                if !(0.5..=10.0).contains(&h_tarp) {
                    tracing::warn!(
                        h_tarp,
                        delta_t_k,
                        tilt_deg = inj.tilt_deg,
                        "interior h_conv outside physically plausible range [0.5, 10.0] W/(m²·K)"
                    );
                }
            }

            let static_r_film = self.per_boundary_static_r_film[i];
            let h_static = if static_r_film > 1e-12 {
                1.0 / static_r_film
            } else {
                0.0
            };

            let delta_h = h_tarp - h_static;

            // Correction only applied when TARP exceeds the frozen static
            // coefficient. When h_tarp < h_static, the A-matrix already
            // overestimates convection — a conservative overestimate that
            // is numerically stable (the implicit diagonal would go negative
            // if we attempted to subtract damping via semi-implicit coupling).
            //
            // Known Limitation (T-0034): walls and ceilings at typical indoor
            // ΔT (1–10 K) have h_tarp < h_static and are not corrected.
            // Full correction requires partial A-matrix reassembly (approach (a)
            // in the ticket), which recomputes the affected rows each timestep.
            if delta_h <= 0.0 {
                continue;
            }

            let area = inj.area_m2;
            let c_surface = inj.c_surface_j_k.max(1e-12);
            let c_zone = inj.c_zone_j_k.max(1e-12);

            // Semi-implicit diagonal damping: added to M for unconditional stability.
            // delta_h > 0 guarantees d > 0, so M remains positive definite.
            let d_surface = dt * delta_h * area / c_surface;
            let d_zone = dt * delta_h * area / c_zone;

            // Explicit off-diagonal coupling: uses current (bounded) state.
            let forcing_surface = dt * delta_h * area * t_zone / c_surface;
            let forcing_zone = dt * delta_h * area * t_surface / c_zone;

            self.coupling_buf
                .push((inj.surface_state_index, d_surface, forcing_surface));
            self.coupling_buf
                .push((inj.zone_state_index, d_zone, forcing_zone));
        }
    }

    /// Phase 1: build input vector and coupling from current weather/solar/infiltration.
    /// Stores results in `last_u`, `last_coupling`, `last_coupled_state` so that
    /// `solve_ideal_capacity_for_target` sees current-step data.
    /// Does NOT cache u for the integration step -- `integrate` rebuilds it from
    /// post-dispatch ports.
    pub(super) fn prepare_inputs_inner(&mut self, ports: &PortSlots, env: &EnvironmentState) {
        // Clear per-step degradation tracking — a new step starts fresh.
        self.ideal_capacity_degraded_zones.clear();

        let saved_ext_temps = self.exterior_surface_temps.clone();
        let (u, _latent) = self.build_input_vector(ports, env);
        self.exterior_surface_temps = saved_ext_temps;

        self.build_coupling();

        if !self.coupling_buf.is_empty() {
            if self.model.m_is_identity() {
                self.last_coupled_state = CoupledState::Identity;
            } else {
                let coupled_lu = self
                    .model
                    .build_coupled_lu(&mut self.m_scratch, &self.coupling_buf);
                self.last_coupled_state = CoupledState::LU(coupled_lu);
            }
        } else {
            self.last_coupled_state = CoupledState::Uncoupled;
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

        // Per-step interior convection correction appends entries to coupling_buf
        // (semi-implicit: diagonal added to M, off-diagonal as explicit forcing).
        self.apply_convection_forcing();

        if !self.coupling_buf.is_empty() {
            if self.model.m_is_identity() {
                self.model.step_with_identity_coupling_into(
                    &self.x,
                    &u,
                    &mut self.rhs_buf,
                    &self.coupling_buf,
                );

                self.last_coupled_state = CoupledState::Identity;
            } else {
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

                self.last_coupled_state = CoupledState::LU(coupled_lu);
            }
        } else {
            self.model.step_into(&self.x, &u, &mut self.rhs_buf);
            self.last_coupled_state = CoupledState::Uncoupled;
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
        self.full_system_stored_energy_w = 0.0;
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
            self.full_system_stored_energy_w = stored_energy_w;
            tracing::debug!(stored_energy_w, "full-system stored energy change rate [W]");
        }

        // ── Thermal balance terms for invariant check ─────────────────────────────
        //
        // Compute the three terms the dwelling's thermal invariant needs.  For
        // uncoupled steps the discrete-time identities are exact.  For coupled
        // steps we use the SAME affine operator that production uses, so the
        // balance closes to machine precision.
        //
        // Uncoupled:  x_next = A_d·x + B_d·u
        // Coupled:    x_next = F·x + G·u + h   where h = (M+D)⁻¹·f
        //
        // Partition stored (Σ C_i·ΔT_i/dt) into external (G·u), internal
        // ((F−I)·x), and affine coupling (h) contributions.  Without coupling
        // (h ≡ 0) the three-term balance is identically zero for a correct
        // solver.  With coupling, including h in the gain terms makes the
        // balance exact.
        //
        // Reference: EnergyPlus Engineering Reference §"Basis for the Zone and
        // Air System Integration" — heat balance method must conserve energy.
        // ASHRAE HoF 2021 Ch.18 — first-law requirement for zone heat balance.
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            self.thermal_balance_q_gains.clear();
            self.thermal_balance_q_loss = 0.0;
            let n_states = self.x.len();
            let dt_s = self.dt_s;
            let x_zero = &self.balance_x_zero;
            let u_zero = &self.balance_u_zero;

            if self.coupling_buf.is_empty() {
                // Uncoupled step: external = B_d·u, internal = (A_d−I)·x_prev
                let b_d_u = &mut self.balance_buf_a;
                self.model.step_into(x_zero, &u, b_d_u);

                let mut external_w = 0.0_f64;
                for (node_id, &c_j_k) in &self.wiring.node_capacitances {
                    if let Some(&idx) = self.wiring.node_index.get(node_id) {
                        if idx < n_states {
                            external_w += c_j_k * b_d_u[idx] / dt_s;
                        }
                    }
                }
                self.thermal_balance_q_gains.push(external_w);

                // (A_d·x_prev − x_prev) = (A_d − I)·x_prev
                let a_d_x = &mut self.balance_buf_b;
                self.model.step_into(&self.rhs_buf, u_zero, a_d_x);
                let mut internal_w = 0.0_f64;
                for (node_id, &c_j_k) in &self.wiring.node_capacitances {
                    if let Some(&idx) = self.wiring.node_index.get(node_id) {
                        if idx < n_states {
                            internal_w += c_j_k * (a_d_x[idx] - self.rhs_buf[idx]) / dt_s;
                        }
                    }
                }
                self.thermal_balance_q_loss = -internal_w;
            } else if self.model.m_is_identity() {
                // Identity-M coupled step: O(n) diagonal solve.
                // RHS = (N−D)·x + B_eff·u + f
                // Solve: x_next[i] = RHS[i] / (1 + d_i)  for coupled rows,
                //        x_next[i] = RHS[i]              for uncoupled rows.
                //
                // Affine decomposition:
                //   h        = coupled_step(0, 0)         → affine term
                //   external = coupled_step(0, u) − h     → G·u
                //   internal = coupled_step(x_prev, 0) − h − x_prev → (F−I)·x
                let balance_bufs = (
                    &mut self.balance_buf_a,
                    &mut self.balance_buf_b,
                    &mut self.balance_buf_c,
                );
                let h = balance_bufs.0;
                let g_u = balance_bufs.1;
                let f_minus_i_x = balance_bufs.2;

                // h  = coupled_step(0, 0)
                self.model
                    .build_coupled_rhs(x_zero, u_zero, h, &self.coupling_buf);
                for &(idx, d, _) in &self.coupling_buf {
                    if idx < n_states {
                        h[idx] /= 1.0 + d;
                    }
                }

                // g_u = coupled_step(0, u) − h
                self.model
                    .build_coupled_rhs(x_zero, &u, g_u, &self.coupling_buf);
                for &(idx, d, _) in &self.coupling_buf {
                    if idx < n_states {
                        g_u[idx] /= 1.0 + d;
                    }
                }
                for i in 0..n_states {
                    g_u[i] -= h[i];
                }

                // f_minus_i_x = coupled_step(x_prev, 0) − h − x_prev
                self.model.build_coupled_rhs(
                    &self.rhs_buf,
                    u_zero,
                    f_minus_i_x,
                    &self.coupling_buf,
                );
                for &(idx, d, _) in &self.coupling_buf {
                    if idx < n_states {
                        f_minus_i_x[idx] /= 1.0 + d;
                    }
                }
                for i in 0..n_states {
                    f_minus_i_x[i] = f_minus_i_x[i] - h[i] - self.rhs_buf[i];
                }

                let mut external_w = 0.0_f64;
                let mut internal_w = 0.0_f64;
                let mut affine_w = 0.0_f64;
                for (node_id, &c_j_k) in &self.wiring.node_capacitances {
                    if let Some(&idx) = self.wiring.node_index.get(node_id) {
                        if idx < n_states {
                            external_w += c_j_k * g_u[idx] / dt_s;
                            internal_w += c_j_k * f_minus_i_x[idx] / dt_s;
                            affine_w += c_j_k * h[idx] / dt_s;
                        }
                    }
                }
                self.thermal_balance_q_gains.push(external_w);
                self.thermal_balance_q_gains.push(affine_w);
                self.thermal_balance_q_loss = -internal_w;
            } else {
                // Coupled step with non-identity M: rebuild the coupled LU
                // factorization and decompose via three separate solves.
                let coupled_lu = self
                    .model
                    .build_coupled_lu(&mut self.m_scratch, &self.coupling_buf);

                let balance_bufs = (
                    &mut self.balance_buf_a,
                    &mut self.balance_buf_b,
                    &mut self.balance_buf_c,
                );
                let h = balance_bufs.0;
                let g_u = balance_bufs.1;
                let f_minus_i_x = balance_bufs.2;

                // h = coupled_step(0, 0)
                self.model.step_with_coupled_lu_into(
                    x_zero,
                    u_zero,
                    h,
                    &coupled_lu,
                    &self.coupling_buf,
                );

                // g_u = coupled_step(0, u) − h
                self.model.step_with_coupled_lu_into(
                    x_zero,
                    &u,
                    g_u,
                    &coupled_lu,
                    &self.coupling_buf,
                );
                for i in 0..n_states {
                    g_u[i] -= h[i];
                }

                // f_minus_i_x = coupled_step(x_prev, 0) − h − x_prev
                self.model.step_with_coupled_lu_into(
                    &self.rhs_buf,
                    u_zero,
                    f_minus_i_x,
                    &coupled_lu,
                    &self.coupling_buf,
                );
                for i in 0..n_states {
                    f_minus_i_x[i] = f_minus_i_x[i] - h[i] - self.rhs_buf[i];
                }

                let mut external_w = 0.0_f64;
                let mut internal_w = 0.0_f64;
                let mut affine_w = 0.0_f64;
                for (node_id, &c_j_k) in &self.wiring.node_capacitances {
                    if let Some(&idx) = self.wiring.node_index.get(node_id) {
                        if idx < n_states {
                            external_w += c_j_k * g_u[idx] / dt_s;
                            internal_w += c_j_k * f_minus_i_x[idx] / dt_s;
                            affine_w += c_j_k * h[idx] / dt_s;
                        }
                    }
                }
                self.thermal_balance_q_gains.push(external_w);
                self.thermal_balance_q_gains.push(affine_w);
                self.thermal_balance_q_loss = -internal_w;
            }
        }

        #[cfg(not(any(debug_assertions, feature = "check_invariants")))]
        {
            // Release builds: clear gains so check_invariants skips the check.
            self.thermal_balance_q_gains.clear();
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

    // ── Design-day autosizing (T-0192) ──────────────────────────────────────────

    /// Run a design-day simulation for heating equipment sizing.
    ///
    /// Simulates `WARMUP_DAYS` warmup days + 1 recording day at the heating
    /// design outdoor condition. Outdoor temperature is constant at
    /// `design_outdoor_c` (heating design days have minimal diurnal variation;
    /// constant is worst-case for sizing). Zero solar, zero internal gains,
    /// zero infiltration.
    ///
    /// At each timestep the ideal HVAC input that drives zone temperature to
    /// `target_c` in one step is computed via
    /// [`StateSpaceModel::solve_for_output_input`], applied to the input
    /// vector, and the solver is stepped. The peak HVAC input across the
    /// recording day timesteps is the sizing capacity.
    ///
    /// Uses the solver's native timestep `dt_s`. Does NOT modify the solver's
    /// persistent state (`x`, `last_u`, couplings).
    ///
    /// Returns the required heating capacity in watts (positive), or 0.0 if
    /// the zone is unknown or solving fails.
    ///
    /// Reference: EnergyPlus `SizingManager.cc:285-390` (ZoneSizingCalc
    /// design-day methodology using ideal loads).
    pub fn autosize_design_day_heating(
        &self,
        zone: ZoneId,
        target_c: f64,
        design_outdoor_c: f64,
    ) -> f64 {
        self.run_design_day(zone, target_c, design_outdoor_c, 0.0, None, 0.0)
    }

    /// Run a design-day simulation for cooling equipment sizing.
    ///
    /// Simulates `WARMUP_DAYS` warmup days + 1 recording day on July 21 with
    /// ASHRAE cooling design-day diurnal dry-bulb profile and clear-sky solar.
    ///
    /// Outdoor temperature follows the ASHRAE 2017 HoF Ch.14 Table 1 profile
    /// with a diurnal range of [`COOLING_DESIGN_DAY_RANGE_C`]. Clear-sky
    /// solar irradiance is computed at hourly intervals using the ASHRAE
    /// clear-sky model and Perez (1990) anisotropic tilted irradiance model
    /// for each window and opaque surface.
    ///
    /// Internal gains (`internal_gains_w` sensible) are included per ACCA
    /// Manual J-2016 §7.
    ///
    /// Returns the peak HVAC input (positive) across the recording day
    /// timesteps as the sizing capacity, or 0.0 if the zone is unknown or
    /// solving fails.
    pub fn autosize_design_day_cooling(
        &self,
        zone: ZoneId,
        target_c: f64,
        design_outdoor_c: f64,
        site_lat_deg: f64,
        site_lon_deg: f64,
        internal_gains_w: f64,
    ) -> f64 {
        let solar = precompute_hourly_solar_july21(site_lat_deg, site_lon_deg);
        self.run_design_day(
            zone,
            target_c,
            design_outdoor_c,
            COOLING_DESIGN_DAY_RANGE_C,
            Some(&solar),
            internal_gains_w,
        )
    }

    /// Shared design-day simulation loop.
    ///
    /// Parameters:
    /// - `daily_range_c`: diurnal range for the outdoor temperature profile
    ///   (0 for constant — heating; ∼12 °C for cooling)
    /// - `solar_data`: pre-computed hourly solar data for the design day,
    ///   or `None` for zero solar (heating design day)
    /// - `internal_gains_w`: sensible internal gains [W]; added to the
    ///   HVAC load for cooling design days (solar_data is `Some`).
    ///   Zero for heating design days (conservative per Manual J).
    fn run_design_day(
        &self,
        zone: ZoneId,
        target_c: f64,
        design_outdoor_c: f64,
        daily_range_c: f64,
        solar_data: Option<&[Option<HourlySolar>]>,
        internal_gains_w: f64,
    ) -> f64 {
        let Some(&input_idx) = self.wiring.zone_sensible_input_indices.get(&zone) else {
            return 0.0;
        };
        let Some(&output_idx) = self.wiring.zone_output_indices.get(&zone) else {
            return 0.0;
        };

        let dt_s = self.dt_s;
        let timesteps_per_hour = (3600.0 / dt_s).round().max(1.0) as usize;
        let timesteps_per_day = 24 * timesteps_per_hour;
        let total_timesteps = (WARMUP_DAYS + 1) * timesteps_per_day;

        let n_inputs = self.model.input_dim();
        let n_states = self.model.state_dim();
        let mut x = self.x.clone();
        let mut u = DVector::zeros(n_inputs);
        let mut x_next = DVector::zeros(n_states);

        let mut peak_load: f64 = 0.0;
        #[cfg(feature = "observe")]
        let mut peak_timestep: usize = 0;

        #[cfg(feature = "observe")]
        let zone_state_idx = self.wiring.zone_state_indices.get(&zone).copied();
        #[cfg(feature = "observe")]
        let (mut observe_loads, mut observe_temps) = (
            Vec::with_capacity(timesteps_per_day),
            Vec::with_capacity(timesteps_per_day),
        );

        for t in 0..total_timesteps {
            let timestep_in_day = t % timesteps_per_day;
            // Hour of day at the midpoint of the timestep
            let hour_of_day = (timestep_in_day as f64 + 0.5) * dt_s / 3600.0;
            let t_out = ashrae_design_day_dry_bulb(design_outdoor_c, daily_range_c, hour_of_day);

            // ── Build simplified input vector ──────────────────────────
            u.fill(0.0);

            // Outdoor temperature inputs
            for &col in &self.wiring.outdoor_temp_input_indices {
                if col < n_inputs {
                    u[col] = t_out;
                }
            }

            // Ground temperature: approximate as design_outdoor_c for
            // conservative sizing (cold ground for heating, warm for cooling).
            for &col in &self.wiring.ground_temp_input_indices {
                if col < n_inputs {
                    u[col] = design_outdoor_c;
                }
            }

            // Solar gains — cooling design day only
            if let Some(solar) = solar_data {
                let hour_idx = (hour_of_day as usize).min(23);
                if let Some(sol) = solar.get(hour_idx).and_then(|o| o.as_ref()) {
                    let ghi = sol.0;
                    let dni = sol.1;
                    let dhi = sol.2;
                    let zenith_deg = sol.3;
                    let azimuth_deg = sol.4;
                    if zenith_deg < 90.0 {
                        let doy: u32 = 202; // July 21 (non-leap year)
                        // Window solar
                        for (surface_id, win_props) in &self.config.window_properties {
                            let Some(&solar_idx) = self.wiring.solar_input_indices.get(surface_id)
                            else {
                                continue;
                            };
                            if solar_idx >= n_inputs {
                                continue;
                            }
                            let irr = perez_tilted_irradiance(
                                *surface_id,
                                ghi,
                                dni,
                                dhi,
                                zenith_deg,
                                azimuth_deg,
                                win_props.tilt_deg,
                                win_props.azimuth_deg,
                                doy,
                                DEFAULT_GROUND_ALBEDO,
                            );
                            let poa_w_m2 = irr.direct_w_m2 + irr.diffuse_w_m2 + irr.reflected_w_m2;
                            u[solar_idx] += poa_w_m2 * win_props.shgc * win_props.area_m2;
                        }
                        // Opaque surface solar
                        for info in &self.config.exterior_surfaces {
                            if info.input_index >= n_inputs {
                                continue;
                            }
                            if self.config.window_properties.contains_key(&info.surface_id) {
                                continue;
                            }
                            let irr = perez_tilted_irradiance(
                                info.surface_id,
                                ghi,
                                dni,
                                dhi,
                                zenith_deg,
                                azimuth_deg,
                                info.tilt_deg,
                                info.azimuth_deg,
                                doy,
                                DEFAULT_GROUND_ALBEDO,
                            );
                            let poa_w_m2 = irr.direct_w_m2 + irr.diffuse_w_m2 + irr.reflected_w_m2;
                            u[info.input_index] += info.absorptance * info.area_m2 * poa_w_m2;
                        }
                    }
                }
            }

            // ── Solve for ideal HVAC input ─────────────────────────────
            let hvac_input = match self
                .model
                .solve_for_output_input(&x, &u, target_c, output_idx, input_idx)
            {
                Ok(val) if val.is_finite() => val,
                _other => {
                    tracing::debug!(
                        timestep = t,
                        "design-day solve_for_output_input returned non-finite or \
                         errored; using 0.0 for this timestep"
                    );
                    0.0
                }
            };
            u[input_idx] = hvac_input;

            // ── Step the state-space model forward ─────────────────────
            self.model.step_into(&x, &u, &mut x_next);
            std::mem::swap(&mut x, &mut x_next);

            // ── Record on the recording day ────────────────────────────
            if t >= WARMUP_DAYS * timesteps_per_day {
                // ACCA Manual J-2016 §7: cooling design loads must include
                // internal gains from occupancy, lighting, and appliances.
                // Internal gains are added to the required HVAC input:
                // they increase the cooling requirement because the HVAC
                // must remove additional heat generated inside the zone.
                let load = hvac_input.abs() + internal_gains_w;
                if load > peak_load {
                    peak_load = load;
                    #[cfg(feature = "observe")]
                    {
                        peak_timestep = timestep_in_day;
                    }
                }
                #[cfg(feature = "observe")]
                {
                    observe_loads.push(hvac_input);
                    let zone_db = if let Some(si) = zone_state_idx {
                        observe_temps.push(x[si]);
                        x[si]
                    } else {
                        f64::NAN
                    };
                    tracing::debug!(
                        sizing.timestep = timestep_in_day,
                        sizing.zone_db_c = zone_db,
                        sizing.hvac_input_w = hvac_input,
                        "design-day per-timestep telemetry"
                    );
                }
            }
        }

        // ── Invariant: peak load must be non-negative and finite ───────────
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            assert!(
                peak_load.is_finite() && peak_load >= 0.0,
                "design-day autosizing: peak_load ({}) must be non-negative and finite",
                peak_load
            );
            assert!(
                internal_gains_w >= 0.0,
                "design-day autosizing: internal_gains_w ({}) must be non-negative",
                internal_gains_w
            );
            assert!(
                internal_gains_w < 5_000.0,
                "design-day autosizing: internal_gains_w ({}) implausibly large \
                 for a single-family residence (≥ 5 kW)",
                internal_gains_w
            );
        }

        // ── Telemetry ──────────────────────────────────────────────────────
        #[cfg(feature = "observe")]
        {
            let mean_load = if observe_loads.is_empty() {
                f64::NAN
            } else {
                observe_loads.iter().sum::<f64>() / observe_loads.len() as f64
            };
            let mean_temp = if observe_temps.is_empty() {
                f64::NAN
            } else {
                observe_temps.iter().sum::<f64>() / observe_temps.len() as f64
            };
            tracing::info!(
                sizing.peak_load_w = peak_load,
                sizing.peak_timestep = peak_timestep,
                sizing.num_warmup_days = WARMUP_DAYS,
                sizing.mean_load_w = mean_load,
                sizing.mean_zone_db_c = mean_temp,
                sizing.daily_range_c = daily_range_c,
                sizing.timesteps_per_day = timesteps_per_day,
                sizing.internal_gains_w = internal_gains_w,
                "design-day autosizing complete"
            );
        }

        peak_load
    }
}

// ── Helper functions ────────────────────────────────────────────────────────

/// ASHRAE 2017 HoF Ch.14 daily dry-bulb temperature range for a clear-sky
/// cooling design day [°C]. The range is the difference between maximum
/// and minimum dry-bulb temperature over a 24-hour period.
///
/// ASHRAE 2017 HoF Ch.14 §4 Table 1: "Profile for Daily Dry-Bulb Temperature"
/// uses a mean coincident dry-bulb range of 11.7 °C (21 °F) for cooling
/// design days.
const COOLING_DESIGN_DAY_RANGE_C: f64 = 11.7;

/// Number of warmup days to run before recording the design-day peak load.
///
/// EnergyPlus uses 1–3 warmup days (SizingManager.cc:122–126). Two warmup
/// days are sufficient for the building thermal mass to reach a steady
/// periodic state for typical residential constructions with time constants
/// of 12–72 hours.
const WARMUP_DAYS: usize = 2;

/// Solar irradiance data at a single hour: (ghi, dni, dhi, zenith_deg, azimuth_deg)
/// all in [W/m²] for irradiance and [°] for angles.
type HourlySolar = (f64, f64, f64, f64, f64);

/// ASHRAE 2017 HoF Ch.14 design-day dry-bulb temperature [°C].
///
/// Returns the outdoor dry-bulb temperature at the given fractional hour of
/// day (0.0–23.999...) using a sinusoidal approximation to the ASHRAE
/// design-day profile:
///
/// ```text
///   T(h) = T_design − DR × f(h)
///   f(h) = 0.5 − 0.5 × sin(2π × (h − 9) / 24)
/// ```
///
/// The profile peaks at h = 15 (3 PM local, the hottest part of the day)
/// and troughs at h = 3 (3 AM local). This matches the observed 2–3 hour
/// lag between peak solar irradiance (noon) and peak air temperature
/// (ASHRAE HoF 2021 Ch.14 §4 Table 14.6).
///
/// When `daily_range_c` ≤ 0, returns `design_db_c` constant (used for
/// heating design days where sustained cold is the worst case).
fn ashrae_design_day_dry_bulb(design_db_c: f64, daily_range_c: f64, hour_of_day: f64) -> f64 {
    if daily_range_c <= 0.0 {
        return design_db_c;
    }
    // f(h) = 0.5 - 0.5 × sin(2π × (h − 9) / 24)
    let fraction = 0.5 - 0.5 * (std::f64::consts::TAU * (hour_of_day - 9.0) / 24.0).sin();
    design_db_c - daily_range_c * fraction
}

/// Pre-compute hourly solar position and clear-sky irradiance for July 21.
///
/// Returns 24 entries, one per hour (0..23), each `None` when the sun is
/// below the horizon or `Some((ghi, dni, dhi, zenith_deg, azimuth_deg))`
/// when the sun is above the horizon.
///
/// ASHRAE 2017 HoF Ch.14 clear-sky model for direct normal and diffuse
/// horizontal irradiance. Perez (1990) anisotropic model for tilted
/// irradiance is applied downstream by the caller.
fn precompute_hourly_solar_july21(
    site_lat_deg: f64,
    site_lon_deg: f64,
) -> Vec<Option<HourlySolar>> {
    use chrono::{Datelike, FixedOffset, TimeZone};
    let mut data = Vec::with_capacity(24);
    // July 21 = day 202 (non-leap year, 2025)
    if let Some(base) =
        FixedOffset::east_opt(0).and_then(|tz| tz.with_ymd_and_hms(2025, 7, 21, 0, 0, 0).single())
    {
        for h in 0..24 {
            let dt = base + chrono::Duration::hours(h);
            let pos = solar_position(site_lat_deg, site_lon_deg, dt);
            let zenith_deg = (90.0 - pos.altitude_deg).max(0.0);
            if pos.altitude_deg > 0.0 {
                let (dni, dhi, ghi) = clear_sky_irradiance(dt.ordinal(), pos.altitude_deg);
                data.push(Some((ghi, dni, dhi, zenith_deg, pos.azimuth_deg)));
            } else {
                data.push(None);
            }
        }
    } else {
        data.resize(24, None);
    }
    data
}
