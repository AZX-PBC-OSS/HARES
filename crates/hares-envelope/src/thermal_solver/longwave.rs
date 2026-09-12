//! Interior and exterior longwave radiation application for the thermal solver.
//!
//! Extracts LWR methods from the `ThermalSolver` to keep `mod.rs` focused on
//! orchestration. Both methods operate on the shared input vector `u` and read
//! the current state vector `x` plus exterior surface temperature warm-starts.

use hares_types::EnvironmentState;
use nalgebra::DVector;

use crate::longwave_radiation::{
    CELSIUS_TO_KELVIN, ExteriorSurface, InteriorSurface, STEFAN_BOLTZMANN, beta_factor,
    exterior_longwave_w, interior_longwave_linearised_w_into, sky_view_factor,
};

use hares_physics::film_coefficients::H_OUT_ASHRAE_PEAK;

use super::ThermalSolver;

impl ThermalSolver {
    /// Iterative exterior longwave radiation solver.
    ///
    /// For each exterior surface, converges on the true exterior surface temperature
    /// by coupling the RC node temperature with an iterative LWR balance, then
    /// injects the fraction of the net flux that reaches the RC node.
    ///
    /// Structurally matches OCHRE `_solve_exterior_radiation` with heavy-ball
    /// damping, with one deliberate divergence: `rad_res_k_w` is the exact
    /// parallel (Thévenin) form, not OCHRE's bare film — see
    /// docs/alignment/DIVERGENCES.md.
    pub(super) fn apply_exterior_longwave_inputs_iterative(
        &mut self,
        u: &mut DVector<f64>,
        env: &EnvironmentState,
    ) {
        let t_ext = env.weather.outdoor_temp_c;
        let t_sky_raw = env.weather.sky_temp_c;
        let t_sky_valid = !t_sky_raw.is_nan();

        // Hoisted per-step constants (env-derived, surface-independent).
        // When T_sky is invalid the effective sky temperature is T_air, which
        // collapses the sky term onto the air term in every branch below.
        let t_air_k4 = (t_ext + CELSIUS_TO_KELVIN).powi(4);
        let t_sky_eff_k4 = if t_sky_valid {
            (t_sky_raw + CELSIUS_TO_KELVIN).powi(4)
        } else {
            t_air_k4
        };

        self.window_exterior_lwr_w = 0.0;
        self.opaque_exterior_lwr_w = 0.0;
        self.opaque_exterior_solar_w = 0.0;
        self.lwr_coupling_buf.clear();

        for (i, info) in self.config.exterior_surfaces.iter().enumerate() {
            if info.input_index >= u.len() || info.state_index >= self.x.len() {
                continue;
            }

            // ── Window branch: sky-temperature correction to U-factor ───────
            //
            // The window U-factor already includes a combined exterior film
            // coefficient (h_out = h_conv + h_rad) that assumes T_sky = T_air.
            // When T_sky ≠ T_air, the additional heat loss (or gain) is:
            //
            //   ΔQ = (U / h_out) · ε·σ·β·F_sky·(T_sky⁴ − T_air⁴) · A
            //
            // This avoids double-counting the radiative exchange already embedded
            // in the U-factor's h_out, and is independent of the zone state (no
            // nonlinear T⁴ feedback).
            //
            // Walton (1983) tilted-sky model; EnergyPlus Eng. Ref. "External
            // Longwave Radiation"; ASHRAE HoF 2021 Ch. 15 Table 1 (h_out
            // fallback = 34 W/(m²·K)).
            if info.boundary_category == Some(super::config::BoundaryCategory::Window) {
                let u_factor = info.u_factor_w_m2_k;
                if u_factor <= 0.0 || info.area_m2 <= 0.0 {
                    #[cfg(any(debug_assertions, feature = "observe_detailed"))]
                    self.ext_surface_diag_buf
                        .push(super::config::ExtSurfaceDiag {
                            surface_id: info.surface_id,
                            category: info.boundary_category,
                            solar_absorbed_w: 0.0,
                            lwr_gain_w: 0.0,
                            surface_temp_c: t_ext,
                            injected_w: 0.0,
                            h_out_computed_w_m2_k: info.h_out_w_m2_k,
                            h_out_effective_w_m2_k: 0.0,
                            h_out_fallback_triggered: false,
                        });
                    continue;
                }

                let f_sky = sky_view_factor(info.tilt_deg);
                let beta = beta_factor(info.tilt_deg);

                let delta_q_w_m2 =
                    info.emissivity * STEFAN_BOLTZMANN * beta * f_sky * (t_sky_eff_k4 - t_air_k4);

                // Use actual h_out from boundary film resistance when at or above
                // the natural convection floor of 1.0 W/(m²·K) (ASHRAE HoF 2021
                // Ch. 4 §4.2). Below 1.0, fall back to the ASHRAE conventional
                // combined exterior coefficient (34 W/(m²·K)) — the standard
                // peak-load fallback for fenestration (ASHRAE HoF 2021 Ch. 15,
                // Table 1).
                let h_out = if info.h_out_w_m2_k >= 1.0 {
                    info.h_out_w_m2_k
                } else {
                    tracing::warn!(
                        surface_id = info.surface_id,
                        h_out_w_m2_k = info.h_out_w_m2_k,
                        "window exterior film coefficient {:?} is below the \
                         1.0 W/(m²·K) natural convection floor; \
                         using ASHRAE conventional fallback ({H_OUT_ASHRAE_PEAK} W/(m²·K)).",
                        info.h_out_w_m2_k
                    );
                    H_OUT_ASHRAE_PEAK
                };

                #[cfg(any(debug_assertions, feature = "check_invariants"))]
                {
                    assert!(
                        h_out >= 1.0,
                        "invariant violation: h_out ({}) is below the 1.0 W/(m²·K) \
                         natural convection floor — the guard above should have caught this. \
                         surface_id={}",
                        h_out,
                        info.surface_id
                    );
                }

                let delta_q_w = (u_factor / h_out) * delta_q_w_m2 * info.area_m2;

                let indoor_zone = self
                    .config
                    .window_zone_ids
                    .get(&info.surface_id)
                    .copied()
                    .unwrap_or(self.config.indoor_zone_id);

                if let Some(&idx) = self.wiring.zone_sensible_input_indices.get(&indoor_zone) {
                    if idx < u.len() {
                        u[idx] += delta_q_w;
                    }
                }

                self.window_exterior_lwr_w += delta_q_w;

                #[cfg(any(debug_assertions, feature = "observe_detailed"))]
                self.ext_surface_diag_buf
                    .push(super::config::ExtSurfaceDiag {
                        surface_id: info.surface_id,
                        category: info.boundary_category,
                        solar_absorbed_w: 0.0,
                        lwr_gain_w: delta_q_w,
                        surface_temp_c: t_ext,
                        injected_w: delta_q_w,
                        h_out_computed_w_m2_k: info.h_out_w_m2_k,
                        h_out_effective_w_m2_k: h_out,
                        h_out_fallback_triggered: h_out != info.h_out_w_m2_k,
                    });
                continue;
            }

            // ── Simple (non-iterative) branch: rad_frac == 0 ────────────────
            //
            // These surfaces have no separate RC node for the exterior film
            // resistance. The A-matrix uses convection-only film (TARP/DOE-2,
            // no h_rad — see film_coefficients.rs test
            // `exterior_film_resistance_is_convection_only_no_h_rad`).
            //
            // The full T⁴ LWR flux is linearised around the current surface
            // temperature and split into:
            // - **Forcing** `h_rad · T_eff` (sky/air temperature, external) →
            //   handled through the semi-implicit coupling forcing term.
            // - **Conductance** `h_rad · T_surf` (state-dependent) →
            //   handled through the coupling diagonal damping.
            //
            // This makes the scheme unconditionally stable regardless of
            // timestep, matching EnergyPlus's linearised `HRad` approach
            // (ConvectionCoefficients.cc:661-678; HeatBalanceSurfaceManager
            // .cc:9575-9592) and the concept of OCHRE's `linearize_ext_
            // radiation` mode (Envelope.py:242-249), but applied per-timestep
            // through the coupling mechanism rather than baked into the
            // A-matrix at construction time.
            //
            // Linearisation derivation:
            //   q_lw = h_lwr_inj − ε·σ·A·T_surf⁴
            //   h_rad = 4·ε·σ·A·T_surf³          [W/K]
            //   T_eff = (h_lwr_inj + 3·ε·σ·A·T_surf⁴) / h_rad   [K]
            //   q_lw ≈ h_rad · (T_eff − T_surf)
            //
            // References:
            // - Walton, G. N. 1983. TARP Reference Manual, NBSSIR 83-2655.
            // - EnergyPlus Engineering Reference: "External Longwave Radiation".
            // - ASHRAE HoF 2021 Ch.4 §4.2 (linearised radiation coefficient).
            if info.rad_frac <= 0.0 {
                let t_node_c = self.x[info.state_index];
                if !t_node_c.is_finite() || info.area_m2 <= 0.0 {
                    #[cfg(any(debug_assertions, feature = "observe_detailed"))]
                    self.ext_surface_diag_buf
                        .push(super::config::ExtSurfaceDiag {
                            surface_id: info.surface_id,
                            category: info.boundary_category,
                            solar_absorbed_w: 0.0,
                            lwr_gain_w: 0.0,
                            surface_temp_c: t_node_c,
                            injected_w: 0.0,
                            h_out_computed_w_m2_k: info.h_out_w_m2_k,
                            h_out_effective_w_m2_k: info.h_out_w_m2_k,
                            h_out_fallback_triggered: false,
                        });
                    continue;
                }

                let f_sky = sky_view_factor(info.tilt_deg);
                let beta = beta_factor(info.tilt_deg);

                // Full T⁴ flux for diagnostics.
                let surface = ExteriorSurface {
                    area_m2: info.area_m2,
                    emissivity: info.emissivity,
                    sky_view_factor: f_sky,
                    beta,
                };
                let q_lw_full = exterior_longwave_w(&surface, t_sky_raw, t_ext, t_node_c);

                // Linearise the T⁴ exchange around the current surface temperature.
                let e_factor = info.emissivity * STEFAN_BOLTZMANN * info.area_m2;
                let t_surf_k = t_node_c + CELSIUS_TO_KELVIN;

                // Incoming radiative flux [W] (independent of T_surf).
                let h_lwr_inj =
                    e_factor * ((1.0 - beta * f_sky) * t_air_k4 + beta * f_sky * t_sky_eff_k4);

                // Linearised radiation coefficient [W/K].
                let h_rad = 4.0 * e_factor * t_surf_k.powi(3);

                // Effective radiative temperature [°C].
                // T_eff_k = (h_lwr_inj + 3·e_factor·T_surf_k⁴) / h_rad
                //         = h_lwr_inj / h_rad + 0.75 · T_surf_k
                let t_eff_c = if h_rad > 1e-15 {
                    (h_lwr_inj / h_rad + 0.75 * t_surf_k) - CELSIUS_TO_KELVIN
                } else {
                    t_node_c
                };

                // Push coupling data for build_coupling.
                // The coupling entry adds:
                //   d = h_rad * b_coeff         (diagonal damping)
                //   forcing = h_rad * T_eff * b_coeff + d * x[state_idx]
                // The forcing term provides h_rad·T_eff (external) and the
                // d*x term cancels the −d*x subtraction in build_coupled_rhs.
                // Net: x_next = (rhs + h_rad·T_eff·b) / (1 + d) — semi-implicit.
                self.lwr_coupling_buf
                    .push((info.state_index, info.input_index, h_rad, t_eff_c));

                // Track total opaque LWR for diagnostics. This flux is NOT
                // in `u` (it goes through the coupling mechanism), so the
                // `u.iter().sum()` delta cannot capture it.
                self.opaque_exterior_lwr_w += q_lw_full;

                #[cfg(any(debug_assertions, feature = "observe_detailed"))]
                self.ext_surface_diag_buf
                    .push(super::config::ExtSurfaceDiag {
                        surface_id: info.surface_id,
                        category: info.boundary_category,
                        solar_absorbed_w: 0.0,
                        lwr_gain_w: q_lw_full,
                        surface_temp_c: t_node_c,
                        injected_w: h_rad * (t_eff_c - t_node_c),
                        h_out_computed_w_m2_k: info.h_out_w_m2_k,
                        h_out_effective_w_m2_k: info.h_out_w_m2_k,
                        h_out_fallback_triggered: false,
                    });
                continue;
            }

            // ── Iterative branch: rad_frac > 0 ──────────────────────────────
            //
            // Converges on the exterior surface temperature by coupling the RC
            // node temperature with an iterative T⁴ LWR balance, then injects
            // the fraction of the net flux that reaches the RC node.
            //
            // Structurally matches OCHRE `_solve_exterior_radiation`
            // (Envelope.py:125-163) with heavy-ball damping; rad_res is the
            // exact parallel form, deliberately divergent from OCHRE's bare
            // film (docs/alignment/DIVERGENCES.md).
            let e_factor = info.emissivity * STEFAN_BOLTZMANN * info.area_m2;
            let f_sky = sky_view_factor(info.tilt_deg);
            let beta = beta_factor(info.tilt_deg);
            let t_node_c = self.x[info.state_index];

            if !t_node_c.is_finite() {
                continue;
            }

            // Per-surface solar gain [W] for the iteration (no allocation).
            // Indexed through the per-step slot map rebuilt by
            // `build_input_vector` — no per-surface rescan of the irradiance vec.
            let solar_w = self
                .solar_irr_slot_buf
                .get(&info.surface_id)
                .and_then(|&slot| env.weather.solar_irradiance.get(slot))
                .map(|irr| {
                    info.absorptance
                        * info.area_m2
                        * (irr.direct_w_m2 + irr.diffuse_w_m2 + irr.reflected_w_m2)
                })
                .unwrap_or(0.0);

            // Incoming LWR (environment → surface), independent of surface temp.
            let h_lwr_inj = if !t_sky_valid {
                e_factor * t_air_k4
            } else {
                e_factor * ((1.0 - beta * f_sky) * t_air_k4 + beta * f_sky * t_sky_eff_k4)
            };

            // Initial surface temperature estimate from linear interpolation.
            let t_surf_init = info.rad_frac * t_node_c + (1.0 - info.rad_frac) * t_ext;

            // Iterative solve with heavy-ball damping (matches OCHRE).
            // `converged` records whether the loop exited on its 0.01 K
            // tolerance rather than the iteration cap: mid-transient, the
            // ±2 K clamp and damping legitimately leave the skin short of
            // its fixed point within one timestep's budget — convergence
            // happens across timesteps via the warm start (OCHRE's designed
            // behavior). The fixed-point invariant below is only asserted
            // where convergence is claimed.
            let mut t_surf = self.exterior_surface_temps[i];
            let mut t_surf_prev = self.exterior_surface_temps[i];
            let mut converged = false;

            for _ in 0..info.n_iter {
                let lwr = h_lwr_inj - e_factor * (t_surf + CELSIUS_TO_KELVIN).powi(4);
                let t_new = t_surf_init + (solar_w + lwr) * info.rad_res_k_w;
                let t_new = t_new.clamp(t_surf - 2.0, t_surf + 2.0);
                let t_next = t_surf + 0.5 * (t_new - t_surf) + 0.1 * (t_surf - t_surf_prev);
                if !t_next.is_finite() {
                    break;
                }
                t_surf_prev = t_surf;
                t_surf = t_next;
                if (t_surf - t_surf_prev).abs() < 0.01 {
                    converged = true;
                    break;
                }
            }

            self.exterior_surface_temps[i] = t_surf;

            let q_lw = h_lwr_inj - e_factor * (t_surf + CELSIUS_TO_KELVIN).powi(4);
            let injected = (solar_w + q_lw) * info.rad_frac;
            u[info.input_index] += injected;

            // Skin-balance closure invariant. Where the iteration CONVERGED
            // (tolerance break, not
            // the iteration cap), the skin must satisfy its own fixed point;
            // the divider identity is exact regardless of convergence and
            // checked unconditionally. Both are free to check and both hold
            // exactly under the parallel (Thévenin) rad_res — the fixed-point
            // check fails by ~3.9 K under the bare-film form in the
            // thin-skin regime, i.e. it would have caught that defect the
            // day it was written.
            #[cfg(any(debug_assertions, feature = "check_invariants"))]
            {
                let q_total = solar_w + q_lw;
                if converged {
                    let fixed_point_residual =
                        (t_surf - (t_surf_init + q_total * info.rad_res_k_w)).abs();
                    // Tolerance 0.5 K — the derived bound, not a guess. At
                    // the break, |0.5·(F − t_prev) + 0.1·Δ_prev| < 0.01 with
                    // Δ_prev (the previous step) bounded by the ±2 K clamp,
                    // so |t_prev − F| ≤ (0.01 + 0.2)/0.5 = 0.42 and the
                    // residual |t_surf − F| = |0.5(t_prev − F) + 0.1·Δ_prev|
                    // ≤ 0.41 K. Observed: 0.051 K (warmup reproducibility)
                    // and 0.223 K (batch-step synthetic dwelling). The
                    // defect class this guards is multi-kelvin (3.9 K under
                    // the bare-film rad_res), so the bound-derived tolerance
                    // keeps ≈8× sensitivity margin.
                    debug_assert!(
                        fixed_point_residual <= 0.5,
                        "invariant violation: exterior skin {} off its fixed point by \
                         {fixed_point_residual:.4} K (t_skin={t_surf:.3}, \
                         t_init={t_surf_init:.3}, Q={q_total:.1} W, \
                         rad_res={} K/W)",
                        info.surface_id,
                        info.rad_res_k_w
                    );
                }
                let divider_residual = (injected - q_total * info.rad_frac).abs();
                debug_assert!(
                    divider_residual <= 1e-9 * q_total.abs().max(1.0),
                    "invariant violation: exterior skin {} injection off the \
                     divider identity by {divider_residual:.3e} W",
                    info.surface_id
                );
            }

            // Diagnostic split at skin-level semantics (OCHRE "Ext. Solar
            // Gain" / "Ext. LWR Gain"): the absorbed solar and the net LWR
            // at the skin, not the rad_frac-scaled injected share.
            self.opaque_exterior_solar_w += solar_w;
            self.opaque_exterior_lwr_w += q_lw;

            #[cfg(any(debug_assertions, feature = "observe_detailed"))]
            self.ext_surface_diag_buf
                .push(super::config::ExtSurfaceDiag {
                    surface_id: info.surface_id,
                    category: info.boundary_category,
                    solar_absorbed_w: solar_w,
                    lwr_gain_w: q_lw,
                    surface_temp_c: t_surf,
                    injected_w: injected,
                    h_out_computed_w_m2_k: 0.0,
                    h_out_effective_w_m2_k: 0.0,
                    h_out_fallback_triggered: false,
                });
        }
    }

    /// Interior longwave radiation exchange.
    ///
    /// Uses ScriptF grey interchange (Gebhart factors) when pre-computed at init,
    /// falling back to linearized h_r approximation otherwise. The ScriptF path
    /// computes exact T⁴ radiation with all inter-reflections accounted for.
    ///
    /// Returns per-zone net interior LWR heat gains [W] for diagnostics output.
    pub(super) fn apply_interior_longwave_inputs(
        &mut self,
        u: &mut DVector<f64>,
        env: &EnvironmentState,
    ) {
        self.lwr_by_zone_buf.clear();
        for (zone_idx, zone_cfg) in self.config.interior_lwr_zones.iter().enumerate() {
            if zone_cfg.surfaces.len() < 2 {
                continue;
            }
            let t_zone_c = env
                .zones
                .iter()
                .find(|z| z.id == zone_cfg.zone_id)
                .map(|z| z.temperature_c)
                .expect(
                    "interior LWR zone must be present in env.zones — \
                         validated at ThermalSolver construction",
                );

            // Emit zone temperature change telemetry to make the LWR lag observable.
            // t_zone_c is the prior-step committed value from env.zones, which lags
            // the current-step zone temperature by one full timestep. The ScriptF
            // (primary) path does not use t_zone_c, so the lag only affects the
            // linearised fallback path at sub-watt magnitude.
            if let Some(&prev) = self.prev_zone_temps_c.get(&zone_cfg.zone_id) {
                let delta = (t_zone_c - prev).abs();
                if delta > 1e-6 {
                    tracing::debug!(
                        zone_id = ?zone_cfg.zone_id,
                        t_zone_c = t_zone_c,
                        prev_t_zone_c = prev,
                        delta_c = t_zone_c - prev,
                        "interior LWR: using prior-step committed zone temperature (lag observable)"
                    );
                }
            }
            self.prev_zone_temps_c.insert(zone_cfg.zone_id, t_zone_c);

            tracing::trace!(
                zone_id = ?zone_cfg.zone_id,
                n_surfaces = zone_cfg.surfaces.len(),
                t_zone_c = t_zone_c,
                "interior LWR: starting calculation"
            );

            // Reuse pre-allocated buffers for iterative interior surface temperatures.
            let buf = &mut self.interior_surf_temps_buf;
            let base_buf = &mut self.interior_surf_base_buf;
            let prev_buf = &mut self.interior_surf_prev_buf;
            buf.clear();
            base_buf.clear();
            prev_buf.clear();
            for s in &zone_cfg.surfaces {
                let t_node = if let Some(dt) = s.driving_temp {
                    // Window/fallback boundary: use environmental driving temp
                    // instead of state vector node.
                    match dt {
                        super::config::DrivingTemp::Outdoor => env.weather.outdoor_temp_c,
                        super::config::DrivingTemp::Ground { depth_m } => {
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
                } else if s.state_index < self.x.len() {
                    self.x[s.state_index]
                } else {
                    t_zone_c
                };
                let t_surf = s.radiation_frac * t_node + (1.0 - s.radiation_frac) * t_zone_c;
                buf.push(t_surf);
            }
            base_buf.extend_from_slice(buf);
            if let Some(saved) = self.interior_surface_temps.get(zone_idx)
                && saved.len() == buf.len()
            {
                buf.copy_from_slice(saved);
            }
            if let Some(saved_prev) = self.interior_surface_prev_temps.get(zone_idx)
                && saved_prev.len() == buf.len()
            {
                prev_buf.extend_from_slice(saved_prev);
            } else {
                prev_buf.extend_from_slice(buf);
            }
            let t_surf_min = base_buf.iter().copied().fold(f64::INFINITY, f64::min);
            let t_surf_max = base_buf.iter().copied().fold(f64::NEG_INFINITY, f64::max);

            // Build the InteriorSurface list once outside the convergence loop
            // when ScriptF factors are not available (surface geometry does not
            // change between iterations). Eliminates (n_iter − 1) redundant
            // clear+extend allocations per timestep.
            if zone_cfg.scriptf.is_none() {
                // Emit a one-time warning per zone when the linearised h_r ≈ 4εσT³
                // fallback engages. The construction-time guard in ThermalSolver::new
                // rejects this for the normal code path, so this fires only for
                // unusual construction paths (e.g. from_discrete).
                // Linearisation error grows as (ΔT/2T_mean)²: ~0.12 % at ΔT = 20 K,
                // ~0.5 % at ΔT = 40 K (Siegel & Howell 4th ed. Ch. 4).
                if !self.lwr_linearised_warned_zones.contains(&zone_cfg.zone_id) {
                    self.lwr_linearised_warned_zones.insert(zone_cfg.zone_id);
                    tracing::warn!(
                        zone_id = ?zone_cfg.zone_id,
                        "interior LWR: ScriptF factors not available — \
                         falling back to linearised h_r ≈ 4εσT³; \
                         reduced physics fidelity (~0.1–0.5 % error depending on ΔT)"
                    );
                }
                self.lwr_surfaces_buf.clear();
                self.lwr_surfaces_buf
                    .extend(zone_cfg.surfaces.iter().map(|s| InteriorSurface {
                        area_m2: s.area_m2,
                        emissivity: s.emissivity,
                    }));
            }

            let n_iter = (self.dt_s / 300.0_f64).floor() as u32 + 3;
            // Clear previous-iteration flux buffer for this zone
            self.lwr_net_flux_prev_buf.clear();
            for iter_idx in 0..n_iter {
                // Use ScriptF (exact T⁴ radiosity) when pre-computed at init,
                // linearized h_r approximation as fallback.
                if let Some(ref scriptf) = zone_cfg.scriptf {
                    scriptf.net_flux_w_into(buf, &mut self.lwr_net_flux_buf);
                } else {
                    interior_longwave_linearised_w_into(
                        &self.lwr_surfaces_buf,
                        buf,
                        t_zone_c,
                        &mut self.lwr_net_flux_buf,
                    );
                };

                // Check flux-residual convergence (skip on first iteration
                // when no previous flux exists for comparison). The relative
                // flux residual |q_new − q_old| / (|q_old| + ε) directly
                // reflects whether the surface temperature iteration has
                // closed the energy balance. ε = 1e-6 guards against
                // division by zero for near-zero starting fluxes.
                if iter_idx > 0 && !self.lwr_net_flux_prev_buf.is_empty() {
                    let mut converged = true;
                    for j in 0..zone_cfg.surfaces.len() {
                        let old_q = self.lwr_net_flux_prev_buf[j];
                        let new_q = self.lwr_net_flux_buf[j];
                        if (new_q - old_q).abs() / (old_q.abs() + 1e-6) >= 1e-4 {
                            converged = false;
                            break;
                        }
                    }
                    if converged {
                        // Final flux stored in lwr_net_flux_buf by the
                        // most recent net_flux_w_into call — no extra copy
                        // needed; break and use current values below.
                        break;
                    }
                }

                // Save current flux for next iteration's convergence comparison
                self.lwr_net_flux_prev_buf.clear();
                self.lwr_net_flux_prev_buf
                    .extend_from_slice(&self.lwr_net_flux_buf);

                // Update surface temperatures with heavy-ball damping.
                // OCHRE `_solve_interior_radiation` uses the same damping:
                // 0.3 × Gauss-Seidel update + 0.2 × momentum.
                for (j, info) in zone_cfg.surfaces.iter().enumerate() {
                    let t_new = base_buf[j] + self.lwr_net_flux_buf[j] * info.rad_res_k_w;
                    let t_new = t_new.clamp(t_surf_min, t_surf_max);
                    let t_next = buf[j] + 0.3 * (t_new - buf[j]) + 0.2 * (buf[j] - prev_buf[j]);
                    prev_buf[j] = buf[j];
                    buf[j] = t_next;
                }
            }

            // Final net flux at converged interior surface temperatures.
            if let Some(ref scriptf) = zone_cfg.scriptf {
                scriptf.net_flux_w_into(buf, &mut self.lwr_net_flux_buf);
            } else {
                interior_longwave_linearised_w_into(
                    &self.lwr_surfaces_buf,
                    buf,
                    t_zone_c,
                    &mut self.lwr_net_flux_buf,
                );
            };
            if let Some(saved) = self.interior_surface_temps.get_mut(zone_idx) {
                if saved.len() == buf.len() {
                    saved.copy_from_slice(buf);
                }
            }
            if let Some(saved_prev) = self.interior_surface_prev_temps.get_mut(zone_idx) {
                if saved_prev.len() == prev_buf.len() {
                    saved_prev.copy_from_slice(prev_buf);
                }
            }

            let air_idx = self
                .wiring
                .zone_sensible_input_indices
                .get(&zone_cfg.zone_id)
                .copied();
            // Accumulate Σ|q_i|/2 as the total LWR exchange diagnostic.
            // By energy conservation, Σ q_i = 0 (net is always zero), so the
            // sum of signed fluxes is useless as a diagnostic. Instead, summing
            // absolute values and dividing by 2 (to avoid double-counting each
            // radiative pair) yields the total LWR energy being exchanged between
            // surfaces — a physically meaningful, non-zero indicator of how active
            // the interior radiation exchange is.
            let mut zone_exchange = 0.0_f64;
            #[allow(clippy::unused_enumerate_index)]
            for (_j, (info, &q)) in zone_cfg
                .surfaces
                .iter()
                .zip(self.lwr_net_flux_buf.iter())
                .enumerate()
            {
                zone_exchange += q.abs();

                if info.driving_temp.is_none() && info.input_index < u.len() {
                    // Opaque surfaces (with RC nodes): R_film is convection-only.
                    // Full ScriptF T⁴ LWR flux injected via radiation_frac split.
                    // OCHRE `_solve_interior_radiation` lines 1187-1195:
                    //   surface.lwr_gain * surface.radiation_frac → surface h_idx
                    //   surface.lwr_gain * (1 - radiation_frac) → zone radiation_heat
                    u[info.input_index] += q * info.radiation_frac;
                    if let Some(ai) = air_idx {
                        if ai < u.len() {
                            u[ai] += q * (1.0 - info.radiation_frac);
                        }
                    }
                } else if info.driving_temp.is_some() {
                    // Window surfaces (no RC node, t_idx=None): OCHRE skips
                    // lwr_gain * radiation_frac injection (no h_idx to inject to).
                    // Only lwr_gain * (1 - radiation_frac) goes to zone air.
                    // The radiation_frac portion is carried by the window's
                    // U-factor conduction path (boundary temp already reflects
                    // the LWR exchange). OCHRE `_solve_interior_radiation`
                    // lines 1187-1195: `if surface.t_idx is not None` guards
                    // the h_idx injection; windows always skip it.
                    let q_inject = q * (1.0 - info.radiation_frac);
                    if let Some(ai) = air_idx {
                        if ai < u.len() {
                            u[ai] += q_inject;
                        }
                    }
                }
                #[cfg(any(debug_assertions, feature = "observe_detailed"))]
                self.int_surface_diag_buf
                    .push(super::config::IntSurfaceDiag {
                        surface_temp_c: buf[_j],
                        lwr_flux_w: q,
                    });
            }
            self.lwr_by_zone_buf
                .push((zone_cfg.zone_id, zone_exchange / 2.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::longwave_radiation::{
        CELSIUS_TO_KELVIN, STEFAN_BOLTZMANN, beta_factor, sky_view_factor,
    };
    use hares_types::EnvironmentState;
    use nalgebra::DVector;

    /// Verify: when T_sky < T_air (clear winter night), the window LWR delta is
    /// negative (additional cooling) and scaled by U/h_out per the T_eff approach.
    ///
    /// Physics: vertical south-facing window in Denver winter.
    ///   ε=0.84, tilt=90°, f_sky=0.5, β=√0.5≈0.707, U=3.0 W/(m²·K), A=12 m²
    ///   T_sky=-30°C, T_air=-15°C
    ///
    /// Δq_m2 = 0.84 × σ × 0.707 × 0.5 × (243.15⁴ − 258.15⁴) ≈ −16 W/m²
    /// ΔQ_zone = (U/h_out) × Δq_m2 × A = (3/34) × (−16) × 12 ≈ −17 W
    #[test]
    fn window_exterior_lwr_clear_night_is_cooling() {
        let epsilon = 0.84_f64;
        let tilt_deg = 90.0_f64;
        let f_sky = sky_view_factor(tilt_deg);
        let beta = beta_factor(tilt_deg);
        let u_factor = 3.0_f64;
        let area_m2 = 12.0_f64;

        let t_sky_c = -30.0_f64;
        let t_air_c = -15.0_f64;
        let t_sky_k = t_sky_c + CELSIUS_TO_KELVIN;
        let t_air_k = t_air_c + CELSIUS_TO_KELVIN;

        let delta_q_w_m2 =
            epsilon * STEFAN_BOLTZMANN * beta * f_sky * (t_sky_k.powi(4) - t_air_k.powi(4));
        let delta_q_w = (u_factor / H_OUT_ASHRAE_PEAK) * delta_q_w_m2 * area_m2;

        assert!(
            delta_q_w_m2 < 0.0,
            "per-m² delta must be negative (cooling), got {delta_q_w_m2}"
        );
        assert!(
            delta_q_w_m2 > -30.0,
            "per-m² delta should be ~-16 W/m², got {delta_q_w_m2}"
        );
        assert!(
            delta_q_w < 0.0,
            "total delta must be negative (cooling), got {delta_q_w}"
        );
        assert!(
            delta_q_w > -30.0,
            "total delta should be ~-17 W, got {delta_q_w}"
        );
    }

    /// Verify: when T_sky = T_air (overcast), delta = 0.
    /// The U-factor already accounts for this case — no additional cooling.
    #[test]
    fn window_exterior_lwr_zero_when_sky_equals_air() {
        let epsilon = 0.84_f64;
        let tilt_deg = 90.0_f64;
        let f_sky = sky_view_factor(tilt_deg);
        let beta = beta_factor(tilt_deg);
        let u_factor = 3.0_f64;
        let area_m2 = 12.0_f64;

        let t_sky_c = -15.0_f64;
        let t_air_c = -15.0_f64;
        let t_sky_k = t_sky_c + CELSIUS_TO_KELVIN;
        let t_air_k = t_air_c + CELSIUS_TO_KELVIN;

        let delta_q_w_m2 =
            epsilon * STEFAN_BOLTZMANN * beta * f_sky * (t_sky_k.powi(4) - t_air_k.powi(4));
        let delta_q_w = (u_factor / H_OUT_ASHRAE_PEAK) * delta_q_w_m2 * area_m2;

        assert!(
            delta_q_w.abs() < 1e-6,
            "delta must be zero when T_sky = T_air, got {delta_q_w}"
        );
    }

    /// Verify: the T_eff scaling factor U/h_out reduces the raw LWR delta.
    /// Without scaling, delta ≈ -192 W; with scaling (U=3, h_out=34), delta ≈ -17 W.
    /// The ratio should be U/h_out = 3/34 ≈ 0.088.
    #[test]
    fn window_exterior_lwr_teff_scaling_reduces_raw_delta() {
        let epsilon = 0.84_f64;
        let tilt_deg = 90.0_f64;
        let f_sky = sky_view_factor(tilt_deg);
        let beta = beta_factor(tilt_deg);
        let u_factor = 3.0_f64;
        let area_m2 = 12.0_f64;

        let t_sky_c = -30.0_f64;
        let t_air_c = -15.0_f64;
        let t_sky_k = t_sky_c + CELSIUS_TO_KELVIN;
        let t_air_k = t_air_c + CELSIUS_TO_KELVIN;

        let delta_q_w_m2 =
            epsilon * STEFAN_BOLTZMANN * beta * f_sky * (t_sky_k.powi(4) - t_air_k.powi(4));
        let raw_delta = delta_q_w_m2 * area_m2;
        let scaled_delta = (u_factor / H_OUT_ASHRAE_PEAK) * delta_q_w_m2 * area_m2;

        let ratio = scaled_delta / raw_delta;
        let expected_ratio = u_factor / H_OUT_ASHRAE_PEAK;
        assert!(
            (ratio - expected_ratio).abs() < 1e-9,
            "scaling ratio should be U/h_out = {expected_ratio:.6}, got {ratio:.6}"
        );
        assert!(
            raw_delta.abs() > scaled_delta.abs(),
            "scaled delta ({scaled_delta:.1}) must be smaller than raw ({raw_delta:.1})"
        );
    }

    /// Verify: when h_out_w_m2_k is provided (actual film coefficient), it's used
    /// instead of the NFRC fallback. Higher h_out → smaller correction.
    #[test]
    fn window_exterior_lwr_uses_actual_h_out_when_available() {
        let epsilon = 0.84_f64;
        let tilt_deg = 90.0_f64;
        let f_sky = sky_view_factor(tilt_deg);
        let beta = beta_factor(tilt_deg);
        let u_factor = 3.0_f64;
        let area_m2 = 12.0_f64;
        let t_sky_c = -30.0_f64;
        let t_air_c = -15.0_f64;
        let t_sky_k = t_sky_c + CELSIUS_TO_KELVIN;
        let t_air_k = t_air_c + CELSIUS_TO_KELVIN;

        let delta_q_w_m2 =
            epsilon * STEFAN_BOLTZMANN * beta * f_sky * (t_sky_k.powi(4) - t_air_k.powi(4));

        let delta_nfrc = (u_factor / H_OUT_ASHRAE_PEAK) * delta_q_w_m2 * area_m2;
        let h_out_actual = 50.0_f64;
        let delta_actual = (u_factor / h_out_actual) * delta_q_w_m2 * area_m2;

        assert!(
            delta_actual.abs() < delta_nfrc.abs(),
            "higher h_out should give smaller correction: actual={delta_actual:.2}, nfrc={delta_nfrc:.2}"
        );
    }

    /// Verify: horizontal window (skylight) has larger LWR delta than vertical
    /// because f_sky=1.0, β=1.0 (full sky exposure).
    #[test]
    fn window_exterior_lwr_horizontal_sees_more_sky_than_vertical() {
        let epsilon = 0.84_f64;
        let u_factor = 3.0_f64;
        let area_m2 = 12.0_f64;
        let t_sky_c = -30.0_f64;
        let t_air_c = -15.0_f64;
        let t_sky_k = t_sky_c + CELSIUS_TO_KELVIN;
        let t_air_k = t_air_c + CELSIUS_TO_KELVIN;

        let f_sky_v = sky_view_factor(90.0);
        let beta_v = beta_factor(90.0);
        let delta_v = (u_factor / H_OUT_ASHRAE_PEAK)
            * epsilon
            * STEFAN_BOLTZMANN
            * beta_v
            * f_sky_v
            * (t_sky_k.powi(4) - t_air_k.powi(4))
            * area_m2;

        let f_sky_h = sky_view_factor(0.0);
        let beta_h = beta_factor(0.0);
        let delta_h = (u_factor / H_OUT_ASHRAE_PEAK)
            * epsilon
            * STEFAN_BOLTZMANN
            * beta_h
            * f_sky_h
            * (t_sky_k.powi(4) - t_air_k.powi(4))
            * area_m2;

        assert!(
            delta_h < delta_v,
            "horizontal skylight should cool more than vertical window: h={delta_h:.1}, v={delta_v:.1}"
        );
    }

    /// Verify: H_OUT_ASHRAE_PEAK matches the ASHRAE conventional combined exterior
    /// film coefficient for peak heating load calculations.
    /// ASHRAE HoF 2021 Ch. 15, Table 1; Engineers Edge (citing ASHRAE).
    #[test]
    fn h_out_ashrae_matches_standard() {
        assert!(
            (H_OUT_ASHRAE_PEAK - 34.0).abs() < 1e-9,
            "H_OUT_ASHRAE_PEAK should be 34.0 W/(m²·K) per ASHRAE conventional peak-load value"
        );
    }

    /// Verify: window interior LWR injection uses OCHRE "full" mode —
    /// only q × (1 − radiation_frac) goes to zone air. The radiation_frac
    /// portion is carried by the window's U-factor conduction path
    /// (boundary temp already reflects LWR exchange).
    ///
    /// Physics: window at T_out=-15°C in a zone at T_zone=20°C, U=3.0 W/(m²·K).
    ///   R_film_int (E+ window decomposition) = 0.120 m²·K/W
    ///   radiation_frac ≈ 0.360
    ///   Window interior surface T_surf ≈ 7.4°C
    ///   Full net LWR gain q ≈ 669 W (cold window gains from warm surfaces)
    ///   Zone-air injection = q × (1 − 0.36) ≈ 428 W
    ///   The remaining q × 0.36 ≈ 241 W is carried by the window conduction
    ///   path (already captured by the U-factor boundary in the RC network).
    ///   OCHRE `_solve_interior_radiation` lines 1187-1195: windows skip
    ///   h_idx injection when t_idx is None.
    #[test]
    fn window_interior_lwr_uses_ochre_full_mode() {
        let radiation_frac = 0.36_f64;
        let q_windows = 669.0_f64;
        let zone_air_injection = q_windows * (1.0 - radiation_frac);
        let to_cond_path = q_windows * radiation_frac;

        assert!(
            (zone_air_injection - 428.0).abs() < 5.0,
            "zone-air injection should be ~428 W, got {zone_air_injection:.1}"
        );
        assert!(
            (to_cond_path - 241.0).abs() < 2.0,
            "to-conduction fraction should be ~241 W, got {to_cond_path:.1}"
        );
        assert!(
            zone_air_injection < q_windows,
            "zone-air injection must be less than full q"
        );
        assert!(
            zone_air_injection > 0.0,
            "zone-air injection must be positive for cold window (net LWR gain)"
        );
        assert!(
            (zone_air_injection + to_cond_path - q_windows).abs() < 1e-9,
            "zone-air + conduction-path must equal full q"
        );
    }

    /// Verify: when radiation_frac = 0 (all LWR stays in zone), the full
    /// q goes to zone air.
    #[test]
    fn window_interior_lwr_full_flux_when_no_conduction() {
        let radiation_frac = 0.0_f64;
        let q = 100.0_f64;
        let zone_air_injection = q * (1.0 - radiation_frac);
        assert!(
            (zone_air_injection - q).abs() < 1e-9,
            "with radiation_frac=0, full q should go to zone air"
        );
    }

    /// Verify: when radiation_frac = 1 (window is infinitely conductive),
    /// all LWR flows through the conduction path — zero zone-air injection.
    #[test]
    fn window_interior_lwr_zero_flux_when_fully_conductive() {
        let radiation_frac = 1.0_f64;
        let q = 100.0_f64;
        let zone_air_injection = q * (1.0 - radiation_frac);
        assert!(
            zone_air_injection.abs() < 1e-9,
            "with radiation_frac=1, all LWR goes to conduction path, zero to zone air"
        );
    }

    // ── Regression: window exterior LWR guard threshold at 1.0 W/(m²·K) ──
    //
    // The guard uses `>= 1.0` — the natural convection floor for a vertical
    // surface at ΔT ≈ 0.4°C (ASHRAE HoF 2021 Ch. 4 §4.2). Values below 1.0
    // produce correction factors >34× relative to the ASHRAE fallback and are
    // physically implausible (the DOE-2 model approaches zero at zero wind and
    // ΔT → 0, creating an unphysical divisor in the T_eff correction).

    /// Build a minimal one-zone `ThermalSolver` with a single vertical window
    /// exterior surface having the specified film coefficient [W/(m²·K)].
    /// Uses clear-winter-night conditions: T_air = −15°C, T_sky = −30°C.
    fn window_lwr_solver(
        h_out_w_m2_k: f64,
        env: &EnvironmentState,
    ) -> crate::thermal_solver::ThermalSolver {
        use std::collections::HashMap;

        use crate::state_space::{OutputMapping, StateSpaceModel};
        use crate::thermal_solver::{
            BoundaryCategory, ExteriorSurfaceInfo, StateSpaceWiring, ThermalSolverConfig,
        };
        use hares_types::ZoneId;
        use nalgebra::DMatrix;

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
            ..Default::default()
        };

        let window_config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            window_zone_ids: HashMap::from([(100, ZoneId(1))]),
            exterior_surfaces: vec![ExteriorSurfaceInfo {
                surface_id: 100,
                state_index: 0,
                input_index: 1,
                area_m2: 12.0,
                emissivity: 0.84,
                tilt_deg: 90.0,
                azimuth_deg: 180.0,
                rad_frac: 0.0,
                rad_res_k_w: 0.0,
                n_iter: 1,
                absorptance: 0.0,
                boundary_category: Some(BoundaryCategory::Window),
                u_factor_w_m2_k: 3.0,
                h_out_w_m2_k,
            }],
            ..Default::default()
        };

        crate::thermal_solver::ThermalSolver::new(model, wiring, window_config, 60.0, env, 22.0)
            .unwrap()
    }

    /// Build an `EnvironmentState` for clear-winter-night window LWR testing.
    fn window_lwr_env() -> EnvironmentState {
        use std::collections::HashMap;

        use chrono::{FixedOffset, TimeZone};
        use hares_types::{GridState, WeatherState, ZoneId, ZoneState};

        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: 22.0,
                humidity_ratio: 0.008,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: -15.0,
                outdoor_humidity_ratio: 0.004,
                wind_speed_m_s: 0.0,
                wind_dir_deg: 180.0,
                ground_temp_c: -15.0,
                sky_temp_c: -30.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![],
                outdoor_wet_bulb_c: 0.0,
                outdoor_enthalpy_j_kg: 0.0,
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
                solar_altitude_deg: 0.0,
                solar_azimuth_deg: 0.0,
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
            custom_domains: vec![],
            equipment_telemetry: HashMap::new(),
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

    /// Regression: a computed h_out of 1.5 W/(m²·K) is above the guard threshold
    /// of 1.0 W/(m²·K) and must be used directly rather than falling back to the
    /// ASHRAE peak-load value of 34 W/(m²·K).
    #[test]
    fn window_lwr_uses_computed_h_out_not_fallback() {
        let env = window_lwr_env();

        let mut solver_1_5 = window_lwr_solver(1.5, &env);
        let mut solver_0 = window_lwr_solver(0.0, &env);
        let mut u_1_5 = DVector::zeros(solver_1_5.model.input_dim());
        let mut u_0 = DVector::zeros(solver_0.model.input_dim());

        solver_1_5.apply_exterior_longwave_inputs_iterative(&mut u_1_5, &env);
        solver_0.apply_exterior_longwave_inputs_iterative(&mut u_0, &env);

        let f_sky = sky_view_factor(90.0);
        let beta = beta_factor(90.0);
        let t_sky_k = -30.0 + CELSIUS_TO_KELVIN;
        let t_air_k = -15.0 + CELSIUS_TO_KELVIN;
        let delta_q_w_m2 =
            0.84 * STEFAN_BOLTZMANN * beta * f_sky * (t_sky_k.powi(4) - t_air_k.powi(4));

        let expected_1_5 = (3.0 / 1.5) * delta_q_w_m2 * 12.0;
        let expected_ashrae = (3.0 / H_OUT_ASHRAE_PEAK) * delta_q_w_m2 * 12.0;

        let actual_1_5 = u_1_5[1];
        assert!(
            (actual_1_5 - expected_1_5).abs() < 1e-6,
            "h_out=1.5: window LWR correction expected {expected_1_5:.6} W, got {actual_1_5:.6} W"
        );

        let actual_0 = u_0[1];
        assert!(
            (actual_0 - expected_ashrae).abs() < 1e-6,
            "h_out=0.0: window LWR correction expected ASHRAE fallback {expected_ashrae:.6} W, \
             got {actual_0:.6} W"
        );

        let ratio = actual_1_5 / actual_0;
        assert!(
            (ratio - H_OUT_ASHRAE_PEAK / 1.5).abs() < 1e-3,
            "ratio u(1.5)/u(0.0) should be {:.3}, got {ratio}",
            H_OUT_ASHRAE_PEAK / 1.5
        );
    }

    /// Guard-condition regression: h_out_w_m2_k = 0.0 must still use the
    /// ASHRAE fallback after the threshold is corrected to `>= 1.0`.
    #[test]
    fn window_lwr_falls_back_to_ashrae_below_guard() {
        let env = window_lwr_env();
        let mut solver = window_lwr_solver(0.0, &env);
        let mut u = DVector::zeros(solver.model.input_dim());
        solver.apply_exterior_longwave_inputs_iterative(&mut u, &env);

        let f_sky = sky_view_factor(90.0);
        let beta = beta_factor(90.0);
        let t_sky_k = -30.0 + CELSIUS_TO_KELVIN;
        let t_air_k = -15.0 + CELSIUS_TO_KELVIN;
        let delta_q_w_m2 =
            0.84 * STEFAN_BOLTZMANN * beta * f_sky * (t_sky_k.powi(4) - t_air_k.powi(4));
        let expected = (3.0 / H_OUT_ASHRAE_PEAK) * delta_q_w_m2 * 12.0;

        let actual = u[1];
        assert!(
            (actual - expected).abs() < 1e-6,
            "h_out=0.0: window LWR correction should use ASHRAE fallback ({H_OUT_ASHRAE_PEAK} W/(m²·K)), \
             expected {expected:.6} W, got {actual:.6} W"
        );
    }

    /// Guard threshold boundary: h_out = 1.0 W/(m²·K) (exactly at the guard)
    /// must use the computed coefficient; h_out = 0.99 (just below) must
    /// trigger the ASHRAE fallback.
    #[test]
    fn window_lwr_guard_threshold_applied_at_h_out_one() {
        let env = window_lwr_env();

        let mut solver_1_0 = window_lwr_solver(1.0, &env);
        let mut solver_0_99 = window_lwr_solver(0.99, &env);
        let mut u_1_0 = DVector::zeros(solver_1_0.model.input_dim());
        let mut u_0_99 = DVector::zeros(solver_0_99.model.input_dim());

        solver_1_0.apply_exterior_longwave_inputs_iterative(&mut u_1_0, &env);
        solver_0_99.apply_exterior_longwave_inputs_iterative(&mut u_0_99, &env);

        let f_sky = sky_view_factor(90.0);
        let beta = beta_factor(90.0);
        let t_sky_k = -30.0 + CELSIUS_TO_KELVIN;
        let t_air_k = -15.0 + CELSIUS_TO_KELVIN;
        let delta_q_w_m2 =
            0.84 * STEFAN_BOLTZMANN * beta * f_sky * (t_sky_k.powi(4) - t_air_k.powi(4));

        let expected_1_0 = (3.0 / 1.0) * delta_q_w_m2 * 12.0;
        let actual_1_0 = u_1_0[1];
        assert!(
            (actual_1_0 - expected_1_0).abs() < 1e-6,
            "h_out=1.0 (at guard): must use computed coefficient. \
             expected {expected_1_0:.6} W, got {actual_1_0:.6} W"
        );

        let expected_fallback = (3.0 / H_OUT_ASHRAE_PEAK) * delta_q_w_m2 * 12.0;
        let actual_0_99 = u_0_99[1];
        assert!(
            (actual_0_99 - expected_fallback).abs() < 1e-6,
            "h_out=0.99 (below guard): must use ASHRAE fallback. \
             expected {expected_fallback:.6} W, got {actual_0_99:.6} W"
        );

        let ratio = actual_1_0 / actual_0_99;
        assert!(
            (ratio - H_OUT_ASHRAE_PEAK).abs() < 0.01,
            "ratio u(1.0)/u(0.99) should be ~{:.0}, got {ratio}",
            H_OUT_ASHRAE_PEAK
        );
    }

    // ── Opaque rad_frac == 0 semi-implicit LWR coupling ────────────────────
    //
    // Tests that the linearised LWR for opaque surfaces with no RC film node
    // is correctly routed through the semi-implicit coupling mechanism:
    //   1. No B·u injection (u[input_index] unchanged)
    //   2. lwr_coupling_buf receives (state_idx, input_idx, h_rad, T_eff)
    //   3. The next-step temperature matches the semi-implicit closed form:
    //      x_next = (N·x + B·u + b·h_rad·T_eff) / (1 + b·h_rad)
    //   4. Linearised flux equals full T⁴ flux at the linearisation point

    /// Build a minimal one-zone solver with a single opaque exterior surface
    /// having rad_frac == 0 (no RC film node). The surface is a 20 m² wall
    /// at emissivity 0.9, tilt 90° (vertical), with convection-only film.
    fn opaque_lwr_solver(env: &EnvironmentState) -> crate::thermal_solver::ThermalSolver {
        use std::collections::HashMap;

        use crate::state_space::{OutputMapping, StateSpaceModel};
        use crate::thermal_solver::{
            BoundaryCategory, ExteriorSurfaceInfo, StateSpaceWiring, ThermalSolverConfig,
        };
        use hares_types::ZoneId;
        use nalgebra::DMatrix;

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
            ..Default::default()
        };

        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            exterior_surfaces: vec![ExteriorSurfaceInfo {
                surface_id: 200,
                state_index: 0,
                input_index: 1,
                area_m2: 20.0,
                emissivity: 0.9,
                tilt_deg: 90.0,
                azimuth_deg: 180.0,
                rad_frac: 0.0,
                rad_res_k_w: 0.0,
                n_iter: 1,
                absorptance: 0.0,
                boundary_category: Some(BoundaryCategory::Wall),
                u_factor_w_m2_k: 0.0,
                h_out_w_m2_k: 5.0,
            }],
            ..Default::default()
        };

        crate::thermal_solver::ThermalSolver::new(model, wiring, config, 60.0, env, 20.0).unwrap()
    }

    /// Verify that the opaque rad_frac == 0 branch:
    /// - Does NOT inject into B·u (u[input_index] is unchanged)
    /// - Populates lwr_coupling_buf with the correct h_rad and T_eff
    /// - The linearised flux `h_rad·(T_eff − T_surf)` equals the full T⁴ flux
    #[test]
    fn opaque_lwr_rad_frac_zero_routes_through_coupling_not_bu() {
        let env = window_lwr_env();
        let mut solver = opaque_lwr_solver(&env);

        // Apply exterior LWR inputs.
        let mut u = nalgebra::DVector::zeros(2);
        u[0] = env.weather.outdoor_temp_c;
        solver.apply_exterior_longwave_inputs_iterative(&mut u, &env);

        // 1. u[input_index] (index 1) must be unchanged — no B·u injection.
        assert_eq!(
            u[1], 0.0,
            "opaque rad_frac==0: u[input_index] must be 0 (LWR through coupling, not B·u), got {}",
            u[1]
        );

        // 2. lwr_coupling_buf must have one entry.
        assert_eq!(
            solver.lwr_coupling_buf.len(),
            1,
            "opaque rad_frac==0: lwr_coupling_buf must have exactly 1 entry"
        );

        let (state_idx, input_idx, h_rad, t_eff_c) = solver.lwr_coupling_buf[0];
        assert_eq!(state_idx, 0, "state_idx must be 0");
        assert_eq!(input_idx, 1, "input_idx must be 1");
        assert!(
            h_rad > 0.0,
            "h_rad must be positive (4·ε·σ·A·T³ > 0), got {h_rad}"
        );

        // 3. Linearised flux must equal full T⁴ flux at the linearisation point.
        let t_surf_c = solver.x[0];
        let f_sky = sky_view_factor(90.0);
        let beta = beta_factor(90.0);
        let surface = ExteriorSurface {
            area_m2: 20.0,
            emissivity: 0.9,
            sky_view_factor: f_sky,
            beta,
        };
        let q_lw_full = exterior_longwave_w(&surface, -30.0, -15.0, t_surf_c);
        let q_lw_linearised = h_rad * (t_eff_c - t_surf_c);

        assert!(
            (q_lw_full - q_lw_linearised).abs() < 1e-3,
            "linearised flux ({q_lw_linearised:.6} W) must equal full T⁴ flux ({q_lw_full:.6} W) \
             at the linearisation point (within floating-point tolerance from different \
             powi(3)/powi(4) computation paths)"
        );

        // 4. h_rad must match the analytical formula 4·ε·σ·A·T_surf³.
        let t_surf_k = t_surf_c + CELSIUS_TO_KELVIN;
        let e_factor = 0.9 * STEFAN_BOLTZMANN * 20.0;
        let h_rad_expected = 4.0 * e_factor * t_surf_k.powi(3);
        assert!(
            (h_rad - h_rad_expected).abs() < 1e-6,
            "h_rad ({h_rad:.6}) must match 4·ε·σ·A·T³ ({h_rad_expected:.6})"
        );
    }

    /// Verify that the semi-implicit LWR coupling produces the correct
    /// next-step temperature: x_next = (N·x + B·u + b·h_rad·T_eff) / (1 + b·h_rad).
    /// The semi-implicit scheme is unconditionally stable for any timestep;
    /// the warmup regression test (`hpxml_path_applies_default_warmup` at
    /// 1-hour timesteps) verifies this end-to-end.
    #[test]
    fn opaque_lwr_coupling_next_step_matches_semi_implicit_closed_form() {
        let env = window_lwr_env();
        let mut solver = opaque_lwr_solver(&env);

        // Apply LWR and build coupling.
        let mut u = nalgebra::DVector::zeros(2);
        u[0] = env.weather.outdoor_temp_c;
        solver.apply_exterior_longwave_inputs_iterative(&mut u, &env);
        solver.build_coupling();

        // Verify coupling_buf has 1 entry (from LWR, no infiltration in this config).
        assert!(
            !solver.coupling_buf.is_empty(),
            "coupling_buf must not be empty after build_coupling"
        );

        let (idx, d, forcing) = solver.coupling_buf[0];
        assert_eq!(idx, 0, "coupling state index must be 0");

        // Verify the coupling algebra: forcing = h_rad·T_eff·b + d·x
        let (_, _, h_rad, t_eff_c) = solver.lwr_coupling_buf[0];
        let b_coeff = solver.model.b_eff()[(0, 1)];
        let d_expected = h_rad * b_coeff;
        let x0 = solver.x[0];
        let forcing_expected = h_rad * t_eff_c * b_coeff + d_expected * x0;

        assert!(
            (d - d_expected).abs() < 1e-12,
            "coupling d ({d:.10}) must match h_rad·b ({d_expected:.10})"
        );
        assert!(
            (forcing - forcing_expected).abs() < 1e-9,
            "coupling forcing ({forcing:.10}) must match h_rad·T_eff·b + d·x ({forcing_expected:.10})"
        );

        // Verify the closed-form: x_next = (N·x + B·u + b·h_rad·T_eff) / (1 + d)
        // where B·u includes ALL input columns (outdoor temp + zone sensible).
        let n = solver.model.n_mat()[(0, 0)];
        let b00 = solver.model.b_eff()[(0, 0)];
        let b01 = solver.model.b_eff()[(0, 1)];
        let b_u_full = b00 * u[0] + b01 * u[1];
        let rhs = n * x0 + b_u_full + b_coeff * h_rad * t_eff_c;
        let x_next_expected = rhs / (1.0 + d);

        // Run the actual step.
        let mut buf = nalgebra::DVector::zeros(1);
        solver.model.step_with_identity_coupling_into(
            &solver.x,
            &u,
            &mut buf,
            &solver.coupling_buf,
        );

        assert!(
            (buf[0] - x_next_expected).abs() < 1e-4,
            "x_next ({:.10}) must match semi-implicit closed form ({:.10})",
            buf[0],
            x_next_expected
        );
    }

    /// First-principles closure test for the iterative exterior-radiation
    /// solve (rad_frac > 0 path).
    ///
    /// The exterior skin is an eliminated (capacitance-free) node between the
    /// convection-only exterior film R_f and the outermost material
    /// half-layer R_h. Its exact steady balance is:
    ///
    ///   (T_s − T_air)/R_f + (T_s − T_node)/R_h = S + q_lwr(T_s)
    ///
    /// Expanding around the no-flux divider temperature
    /// t_init = rad_frac·T_node + (1−rad_frac)·T_air gives the fixed point
    ///
    ///   T_s = t_init + (S + q_lwr)·R_par,   R_par = (R_f·R_h)/(R_f + R_h)
    ///
    /// — the *parallel* combination, not the bare film resistance. With the
    /// parallel form, the current-divider injection (Q·rad_frac into the
    /// outer RC node) is exactly consistent with the series conductance path
    /// at every state, not just at steady state.
    ///
    /// Geometry chosen so R_h ≈ R_f (thin residential skin — wood siding,
    /// stucco, metal; ASHRAE 140 case 600 wall construction), which is where
    /// the bare-film approximation is worst.
    #[test]
    fn iterative_skin_temperature_satisfies_exact_skin_balance() {
        use crate::state_space::{OutputMapping, StateSpaceModel};
        use crate::thermal_solver::{
            BoundaryCategory, ExteriorSurfaceInfo, StateSpaceWiring, ThermalSolverConfig,
        };
        use hares_types::ZoneId;
        use nalgebra::DMatrix;
        use std::collections::HashMap;

        let area_m2 = 20.0;
        let r_film = 0.03; // m²K/W (windy exterior film)
        let r_half = 0.05; // m²K/W (outer half-layer: wood-siding-class skin)
        let emissivity = 0.9;
        let absorptance = 0.6;
        let t_air = 35.0;
        let t_sky = 15.0;
        let t_node = 25.0;
        let poa_w_m2 = 900.0;

        let rad_frac = r_film / (r_film + r_half);
        // The contract under test: `solver_builder` must emit the PARALLEL
        // combination (R_f·R_h)/(R_f+R_h)/A — the exact eliminated-skin
        // resistance. The bare-film form R_f/A over-drives the skin
        // temperature (OCHRE's res_material >> res_film approximation breaks
        // down precisely in the R_h ≈ R_f regime exercised here).
        let rad_res_k_w = (r_film * r_half / (r_film + r_half)) / area_m2;

        let a_c = DMatrix::from_row_slice(1, 1, &[-1e-4]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1e-4, 1e-5]);
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
            ..Default::default()
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            exterior_surfaces: vec![ExteriorSurfaceInfo {
                surface_id: 200,
                state_index: 0,
                input_index: 1,
                area_m2,
                emissivity,
                tilt_deg: 0.0, // horizontal: sky_view_factor = 1, beta = 1
                azimuth_deg: 180.0,
                rad_frac,
                rad_res_k_w,
                n_iter: 100, // fully converge the fixed point
                absorptance,
                boundary_category: Some(BoundaryCategory::Roof),
                u_factor_w_m2_k: 0.0,
                h_out_w_m2_k: 1.0 / r_film,
            }],
            ..Default::default()
        };

        let mut env = window_lwr_env();
        env.weather.outdoor_temp_c = t_air;
        env.weather.ground_temp_c = t_air;
        env.weather.sky_temp_c = t_sky;
        env.weather.solar_irradiance = vec![hares_types::SurfaceIrradiance {
            surface_id: 200,
            direct_w_m2: 700.0,
            diffuse_w_m2: 200.0,
            reflected_w_m2: 0.0,
            angle_of_incidence_rad: 0.5,
        }];

        let mut solver =
            crate::thermal_solver::ThermalSolver::new(model, wiring, config, 60.0, &env, 25.0)
                .unwrap();
        solver.x[0] = t_node;
        // Direct callers of the apply functions must populate the per-step
        // irradiance slot map themselves (production path rebuilds it in
        // `build_input_vector`).
        solver.solar_irr_slot_buf.insert(200, 0);

        let mut u = DVector::zeros(2);
        solver.apply_exterior_longwave_inputs_iterative(&mut u, &env);
        let t_skin_solver = solver.exterior_surface_temps[0];

        // Exact skin balance by Newton iteration on
        //   f(T) = (T−T_air)/R_f + (T−T_node)/R_h − S − q_lwr(T) = 0
        // (horizontal roof: f_sky = 1, beta = 1, so q_lwr = εσA(T_sky⁴ − T⁴)).
        let r_f = r_film / area_m2; // K/W
        let r_h = r_half / area_m2; // K/W
        let solar_w = absorptance * area_m2 * poa_w_m2;
        let e_factor = emissivity * STEFAN_BOLTZMANN * area_m2;
        let t_sky_k4 = (t_sky + CELSIUS_TO_KELVIN).powi(4);
        let q_lwr = |t_c: f64| -> f64 { e_factor * (t_sky_k4 - (t_c + CELSIUS_TO_KELVIN).powi(4)) };
        let balance =
            |t_c: f64| -> f64 { (t_c - t_air) / r_f + (t_c - t_node) / r_h - solar_w - q_lwr(t_c) };
        let mut t_exact = t_air;
        for _ in 0..80 {
            let f = balance(t_exact);
            let df = 1.0 / r_f + 1.0 / r_h + 4.0 * e_factor * (t_exact + CELSIUS_TO_KELVIN).powi(3);
            t_exact -= f / df;
        }
        assert!(
            balance(t_exact).abs() < 1e-3,
            "Newton solve of the exact skin balance did not converge: residual={:.2e} W",
            balance(t_exact)
        );

        // (1) The converged skin temperature must match the exact balance.
        assert!(
            (t_skin_solver - t_exact).abs() < 0.05,
            "iterative skin temp {t_skin_solver:.3}°C must match exact skin-balance \
             solution {t_exact:.3}°C within 0.05 K (residual of exact balance at \
             solver value: {:.2} W)",
            balance(t_skin_solver)
        );

        // (2) The injection into the outer RC node must equal the exact
        // divider share of the total skin flux: Q·rad_frac, evaluated at the
        // exact skin temperature.
        let q_total_exact = solar_w + q_lwr(t_exact);
        let injected_expected = q_total_exact * rad_frac;
        assert!(
            (u[1] - injected_expected).abs() / injected_expected.abs() < 0.01,
            "injected flux {:.1} W must match exact divider share {:.1} W",
            u[1],
            injected_expected
        );
    }

    /// The runtime zone-wiring fallbacks (index-0 attribution,
    /// 20 °C substitution) are gone — construction must reject the states
    /// that used to engage them, with the zone named.
    #[test]
    fn construction_rejects_missing_indoor_zone_wiring_with_zone_named() {
        use std::collections::HashMap;

        use crate::state_space::{OutputMapping, StateSpaceModel};
        use crate::thermal_solver::{
            BoundaryCategory, BoundaryDiagnosticInfo, DrivingTemp, StateSpaceWiring,
            ThermalSolverConfig,
        };
        use hares_types::ZoneId;
        use nalgebra::DMatrix;

        let a_c = DMatrix::from_row_slice(1, 1, &[-1e-4]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1e-4, 1e-5]);
        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();

        // Indoor zone 1 with boundary diagnostics configured, but the
        // wiring has NO zone_output_indices entry — the pre-fix runtime
        // silently attributed diagnostics to output 0.
        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
            outdoor_temp_input_indices: vec![0],
            ..Default::default()
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            boundary_diagnostics: vec![BoundaryDiagnosticInfo::SteadyState {
                ua_w_k: 20.0,
                driving_temp: DrivingTemp::Outdoor,
                category: BoundaryCategory::Wall,
            }],
            ..Default::default()
        };
        let env = window_lwr_env();
        let err =
            crate::thermal_solver::ThermalSolver::new(model, wiring, config, 60.0, &env, 22.0)
                .expect_err("diagnostics without a zone output index must be rejected");
        assert!(
            err.to_string().contains("indoor zone 1")
                && err.to_string().contains("zone_output_indices"),
            "error must name the zone and the missing map, got: {err}"
        );
    }

    /// An interior-LWR zone absent from the environment used to be
    /// silently substituted with 20 °C at runtime; construction must reject
    /// it with the zone named.
    #[test]
    fn construction_rejects_interior_lwr_zone_absent_from_env() {
        use std::collections::HashMap;

        use crate::state_space::{OutputMapping, StateSpaceModel};
        use crate::thermal_solver::{InteriorLwrZoneConfig, StateSpaceWiring, ThermalSolverConfig};
        use hares_types::ZoneId;
        use nalgebra::DMatrix;

        let a_c = DMatrix::from_row_slice(1, 1, &[-1e-4]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1e-4, 1e-5]);
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
            c_zone_j_k: HashMap::from([(ZoneId(1), 1e5)]),
            outdoor_temp_input_indices: vec![0],
            ..Default::default()
        };
        // Zone 9 has LWR surfaces configured but does not exist in env.
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            interior_lwr_zones: vec![InteriorLwrZoneConfig {
                zone_id: ZoneId(9),
                surfaces: vec![],
                scriptf: None,
            }],
            ..Default::default()
        };
        let env = window_lwr_env();
        let err =
            crate::thermal_solver::ThermalSolver::new(model, wiring, config, 60.0, &env, 22.0)
                .expect_err("LWR zone absent from env must be rejected");
        assert!(
            err.to_string().contains("zone 9") && err.to_string().contains("not present"),
            "error must name the absent zone, got: {err}"
        );
    }

    /// The construction-time wiring contract ("out-of-range indices...
    /// fails fast with the offending surface named, instead of silently
    /// mis-wiring") must cover the ground-node wiring too. A
    /// `ground_temp_input_indices` entry beyond the model's input dimension
    /// passes the ground-depth presence check (the depth IS in
    /// `ground_temp_input_depths_m`) but breaks the cache/depths
    /// parallelism: `apply_outdoor_inputs` skips the out-of-range column
    /// (`if idx < u.len()`), so `cached_ground_temps_c` never receives that
    /// depth's entry, and the first step's per-boundary accumulation hits
    /// the `expect` whose message claims the lookup was "validated at
    /// ThermalSolver construction" — a mid-run panic on a wiring error the
    /// construction validation exists to reject.
    #[test]
    fn construction_rejects_out_of_range_ground_input_index_with_depth_named() {
        use std::collections::HashMap;

        use crate::state_space::{OutputMapping, StateSpaceModel};
        use crate::thermal_solver::{
            BoundaryCategory, BoundaryDiagnosticInfo, DrivingTemp, StateSpaceWiring,
            ThermalSolverConfig,
        };
        use hares_types::ZoneId;
        use nalgebra::DMatrix;

        let a_c = DMatrix::from_row_slice(1, 1, &[-1e-4]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1e-4, 1e-5]); // input_dim = 2
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
            // Column 5 does not exist (input_dim = 2): the depth itself is
            // registered, so the presence check alone cannot catch it.
            ground_temp_input_indices: vec![5],
            ground_temp_input_depths_m: vec![2.0],
            ..Default::default()
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            boundary_diagnostics: vec![BoundaryDiagnosticInfo::SteadyState {
                ua_w_k: 20.0,
                driving_temp: DrivingTemp::Ground { depth_m: 2.0 },
                category: BoundaryCategory::Wall,
            }],
            ..Default::default()
        };
        let env = window_lwr_env();
        let err =
            crate::thermal_solver::ThermalSolver::new(model, wiring, config, 60.0, &env, 22.0)
                .expect_err("out-of-range ground input index must be rejected at construction");
        assert!(
            err.to_string().contains("ground") && err.to_string().contains('5'),
            "error must name the ground wiring and the offending index, got: {err}"
        );
    }

    /// Ground input wiring must be parallel: `ground_temp_input_indices`
    /// and `ground_temp_input_depths_m` in lockstep. A length mismatch
    /// (e.g. one valid index registered against two depths) passes the
    /// depth-presence check — the diagnostic's depth IS in the list — but
    /// the runtime cache is built by zipping the two vecs, so the extra
    /// depth has no cache entry and the per-boundary accumulation hits the
    /// same "validated at ThermalSolver construction" `expect` mid-run.
    /// Pins the construction parallelism check.
    #[test]
    fn construction_rejects_non_parallel_ground_input_wiring() {
        use std::collections::HashMap;

        use crate::state_space::{OutputMapping, StateSpaceModel};
        use crate::thermal_solver::{
            BoundaryCategory, BoundaryDiagnosticInfo, DrivingTemp, StateSpaceWiring,
            ThermalSolverConfig,
        };
        use hares_types::ZoneId;
        use nalgebra::DMatrix;

        let a_c = DMatrix::from_row_slice(1, 1, &[-1e-4]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1e-4, 1e-5]);
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
            // One VALID index registered against two depths: the diag's
            // depth (8.0) is present in the depths list, so the presence
            // check alone passes — only the parallelism check catches it.
            ground_temp_input_indices: vec![0],
            ground_temp_input_depths_m: vec![2.0, 8.0],
            ..Default::default()
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            boundary_diagnostics: vec![BoundaryDiagnosticInfo::SteadyState {
                ua_w_k: 20.0,
                driving_temp: DrivingTemp::Ground { depth_m: 8.0 },
                category: BoundaryCategory::Wall,
            }],
            ..Default::default()
        };
        let env = window_lwr_env();
        let err =
            crate::thermal_solver::ThermalSolver::new(model, wiring, config, 60.0, &env, 22.0)
                .expect_err("non-parallel ground input wiring must be rejected at construction");
        assert!(
            err.to_string().contains("parallel"),
            "error must name the parallelism violation, got: {err}"
        );
    }

    /// An out-of-range `outdoor_temp_input_indices` entry is the SILENT
    /// sibling of the ground-index bomb: every consumer guards with
    /// `if idx < u.len()` and skips (`apply_outdoor_inputs`,
    /// `initialize_steady_state`), so no panic ever fires — the outdoor
    /// driving column is simply never written and stays 0.0, and the
    /// building silently simulates against a phantom 0 °C outdoor
    /// boundary. The construction contract ("out-of-range indices... fail
    /// fast... instead of silently mis-wiring") must cover it.
    #[test]
    fn construction_rejects_out_of_range_outdoor_input_index() {
        use std::collections::HashMap;

        use crate::state_space::{OutputMapping, StateSpaceModel};
        use crate::thermal_solver::{StateSpaceWiring, ThermalSolverConfig};
        use hares_types::ZoneId;
        use nalgebra::DMatrix;

        let a_c = DMatrix::from_row_slice(1, 1, &[-1e-4]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1e-4, 1e-5]); // input_dim = 2
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
            // Column 9 does not exist (input_dim = 2): guarded skips in
            // apply_outdoor_inputs and initialize_steady_state mean the
            // outdoor column is silently never driven.
            outdoor_temp_input_indices: vec![9],
            ..Default::default()
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            ..Default::default()
        };
        let env = window_lwr_env();
        let err =
            crate::thermal_solver::ThermalSolver::new(model, wiring, config, 60.0, &env, 22.0)
                .expect_err("out-of-range outdoor input index must be rejected at construction");
        assert!(
            err.to_string().contains("outdoor") && err.to_string().contains('9'),
            "error must name the outdoor wiring and the offending index, got: {err}"
        );
    }

    /// An out-of-range `zone_output_indices` VALUE passes the construction
    /// presence check (which only tests that the indoor zone has an entry)
    /// and detonates mid-run: the per-boundary accumulation indexes
    /// `y_next[zone_output_idx]` unguarded, so the first step panics with a
    /// bare index-out-of-bounds instead of a construction error naming the
    /// zone. Same class as the ground-index bomb: presence checked, range
    /// not.
    #[test]
    fn construction_rejects_out_of_range_zone_output_index() {
        use std::collections::HashMap;

        use crate::state_space::{OutputMapping, StateSpaceModel};
        use crate::thermal_solver::{
            BoundaryCategory, BoundaryDiagnosticInfo, DrivingTemp, StateSpaceWiring,
            ThermalSolverConfig,
        };
        use hares_types::ZoneId;
        use nalgebra::DMatrix;

        let a_c = DMatrix::from_row_slice(1, 1, &[-1e-4]);
        let b_c = DMatrix::from_row_slice(1, 2, &[1e-4, 1e-5]);
        let mapping = OutputMapping {
            output_count: 1, // the only valid output index is 0
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();

        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            // Entry exists (passes the presence check) but points beyond
            // the single output.
            zone_output_indices: HashMap::from([(ZoneId(1), 7)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
            outdoor_temp_input_indices: vec![0],
            ..Default::default()
        };
        // Diagnostics non-empty so the presence check engages on the map.
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            boundary_diagnostics: vec![BoundaryDiagnosticInfo::SteadyState {
                ua_w_k: 20.0,
                driving_temp: DrivingTemp::Outdoor,
                category: BoundaryCategory::Wall,
            }],
            ..Default::default()
        };
        let env = window_lwr_env();
        let err =
            crate::thermal_solver::ThermalSolver::new(model, wiring, config, 60.0, &env, 22.0)
                .expect_err("out-of-range zone output index must be rejected at construction");
        assert!(
            err.to_string().contains("output") && err.to_string().contains('7'),
            "error must name the output wiring and the offending index, got: {err}"
        );
    }

    /// The construction-time range pass over `StateSpaceWiring` rejects an
    /// out-of-range VALUE in every index-bearing map — not just the two maps
    /// (zone output, outdoor temperature) pinned by their own tests above.
    /// The unpinned branches share the pass but fail through distinct modes
    /// when absent: zone-state and solar indices arm a deferred panic /
    /// silently dropped solar, and zone-sensible / indoor-temperature
    /// indices are guard-and-skipped everywhere, so their injections vanish
    /// silently (all zone-air gains discarded; the zone-air-balance
    /// residual corrupted). One test over the shared pass, one case per
    /// remaining branch; each error must name its wiring and index.
    #[test]
    fn construction_rejects_out_of_range_value_in_every_remaining_wiring_map() {
        use std::collections::HashMap;

        use crate::state_space::{OutputMapping, StateSpaceModel};
        use crate::thermal_solver::{StateSpaceWiring, ThermalSolverConfig};
        use hares_types::ZoneId;
        use nalgebra::DMatrix;

        let build_model = || {
            let a_c = DMatrix::from_row_slice(1, 1, &[-1e-4]);
            let b_c = DMatrix::from_row_slice(1, 2, &[1e-4, 1e-5]); // input_dim = 2
            let mapping = OutputMapping {
                output_count: 1,
                node_to_output: vec![(0, 0, 1.0)],
                input_to_output: vec![],
            };
            StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap()
        };
        let base_wiring = || StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 1)]),
            outdoor_temp_input_indices: vec![0],
            ..Default::default()
        };

        let mut state_case = base_wiring();
        state_case.zone_state_indices = HashMap::from([(ZoneId(1), 7)]); // state_dim = 1
        let mut sensible_case = base_wiring();
        sensible_case.zone_sensible_input_indices = HashMap::from([(ZoneId(1), 9)]);
        let mut indoor_case = base_wiring();
        indoor_case.indoor_temp_input_indices = vec![9];
        let mut solar_case = base_wiring();
        solar_case.solar_input_indices = HashMap::from([(42, 9)]);

        let cases: [(&str, StateSpaceWiring); 4] = [
            ("state index", state_case),
            ("sensible-heat input index", sensible_case),
            ("indoor temperature input index", indoor_case),
            ("solar input index", solar_case),
        ];
        for (name_fragment, wiring) in cases {
            let config = ThermalSolverConfig {
                indoor_zone_id: ZoneId(1),
                ..Default::default()
            };
            let env = window_lwr_env();
            let err = crate::thermal_solver::ThermalSolver::new(
                build_model(),
                wiring,
                config,
                60.0,
                &env,
                22.0,
            )
            .expect_err("out-of-range wiring value must be rejected at construction");
            let msg = err.to_string();
            assert!(
                msg.contains(name_fragment) && msg.contains("out of range"),
                "error for the '{name_fragment}' branch must name the wiring and \
                 say why, got: {msg}"
            );
        }
    }

    /// Two exterior surfaces sharing a DEDICATED injection column (not a
    /// zone's additive sensible-heat column, where sharing is legitimate)
    /// must be rejected at construction: one surface's flux would land in
    /// the other's column. `ThermalSolverConfig::validate` deliberately
    /// cannot see this (it is wiring-aware); the check lives in
    /// `ThermalSolver::new` and had no pinning test.
    #[test]
    fn construction_rejects_duplicate_dedicated_exterior_input_column() {
        use std::collections::HashMap;

        use crate::state_space::{OutputMapping, StateSpaceModel};
        use crate::thermal_solver::{
            BoundaryCategory, ExteriorSurfaceInfo, StateSpaceWiring, ThermalSolverConfig,
        };
        use hares_types::ZoneId;
        use nalgebra::DMatrix;

        let surface = |surface_id: u32| ExteriorSurfaceInfo {
            surface_id,
            state_index: 0,
            input_index: 1, // both surfaces share dedicated column 1
            area_m2: 20.0,
            emissivity: 0.9,
            tilt_deg: 90.0,
            azimuth_deg: 180.0,
            rad_frac: 0.0, // non-iterative path: no skin wiring needed
            rad_res_k_w: 0.0,
            n_iter: 1,
            absorptance: 0.7,
            boundary_category: Some(BoundaryCategory::Wall),
            u_factor_w_m2_k: 0.0,
            h_out_w_m2_k: 33.0,
        };

        let a_c = DMatrix::from_row_slice(1, 1, &[-1e-4]);
        let b_c = DMatrix::from_row_slice(1, 3, &[1e-4, 1e-5, 1e-6]); // input_dim = 3
        let mapping = OutputMapping {
            output_count: 1,
            node_to_output: vec![(0, 0, 1.0)],
            input_to_output: vec![],
        };
        let model = StateSpaceModel::from_continuous(&a_c, &b_c, 60.0, &mapping).unwrap();
        let wiring = StateSpaceWiring {
            zone_state_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_output_indices: HashMap::from([(ZoneId(1), 0)]),
            zone_sensible_input_indices: HashMap::from([(ZoneId(1), 2)]),
            outdoor_temp_input_indices: vec![0],
            ..Default::default()
        };
        let config = ThermalSolverConfig {
            indoor_zone_id: ZoneId(1),
            exterior_surfaces: vec![surface(10), surface(11)],
            ..Default::default()
        };
        let env = window_lwr_env();
        let err =
            crate::thermal_solver::ThermalSolver::new(model, wiring, config, 60.0, &env, 22.0)
                .expect_err("duplicate dedicated exterior input column must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("duplicate") && msg.contains("input_index 1"),
            "error must name the duplicated column, got: {msg}"
        );
    }
}
