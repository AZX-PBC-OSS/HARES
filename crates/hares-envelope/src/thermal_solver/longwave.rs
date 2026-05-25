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

use hares_physics::film_coefficients::H_OUT_NFRC;

use super::ThermalSolver;

impl ThermalSolver {
    /// Iterative exterior longwave radiation solver.
    ///
    /// For each exterior surface, converges on the true exterior surface temperature
    /// by coupling the RC node temperature with an iterative LWR balance, then
    /// injects the fraction of the net flux that reaches the RC node.
    ///
    /// Matches OCHRE `_solve_exterior_radiation` with heavy-ball damping.
    pub(super) fn apply_exterior_longwave_inputs_iterative(
        &mut self,
        u: &mut DVector<f64>,
        env: &EnvironmentState,
    ) {
        let t_ext = env.weather.outdoor_temp_c;
        let t_sky_raw = env.weather.sky_temp_c;
        let t_sky_valid = !t_sky_raw.is_nan();

        self.window_exterior_lwr_w = 0.0;

        for (i, info) in self.config.exterior_surfaces.iter().enumerate() {
            if info.input_index >= u.len() || info.state_index >= self.x.len() {
                continue;
            }

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
                        });
                    continue;
                }

                let f_sky = sky_view_factor(info.tilt_deg);
                let beta = beta_factor(info.tilt_deg);
                let t_sky_effective = if t_sky_valid { t_sky_raw } else { t_ext };

                let t_sky_k = t_sky_effective + CELSIUS_TO_KELVIN;
                let t_air_k = t_ext + CELSIUS_TO_KELVIN;

                // Net LWR flux density beyond U-factor's T_sky ≈ T_air assumption:
                // Δq = ε·σ·β·F_sky·(T_sky⁴ − T_air⁴) [W/m²]
                // Walton (1983) tilted-sky model; E+ Eng.Ref "External Longwave Radiation".
                let delta_q_w_m2 = info.emissivity
                    * STEFAN_BOLTZMANN
                    * beta
                    * f_sky
                    * (t_sky_k.powi(4) - t_air_k.powi(4));

                // T_eff approach: the effective outdoor temperature for window conduction
                // is T_eff = T_air + Δq / h_out. The additional zone cooling beyond the
                // U-factor (which assumes T_sky = T_air) is:
                //   ΔQ_zone = U·A·(T_air − T_eff) = (U / h_out) · Δq · A
                // This avoids double-counting: the U-factor's h_out already includes
                // radiative exchange at T_sky = T_air; we correct for T_sky ≠ T_air only.
                // Use actual h_out from boundary film resistance when available;
                // fall back to the ASHRAE conventional combined exterior coefficient
                // (34 W/(m²·K)) — the standard peak-load fallback for fenestration
                // when explicit film resistance is unavailable.
                // ASHRAE HoF 2021 Ch. 15, Table 1; Engineers Edge (citing ASHRAE).
                let h_out = if info.h_out_w_m2_k > 0.0 {
                    info.h_out_w_m2_k
                } else {
                    // The fallback value 34 W/(m²·K) is the ASHRAE conventional
                    // combined (convective + radiative) exterior coefficient for
                    // peak heating load calculations at ~15 mph wind. It is NOT the
                    // NFRC 100 / ISO 15099 convective boundary condition (26 W/(m²·K)
                    // at 5.5 m/s), but is the correct fallback for simplified
                    // fenestration load-calculation contexts (ASHRAE HoF 2021 Ch. 15).
                    // A non-positive computed h_out indicates an upstream computation
                    // failure and should be unreachable in production.
                    tracing::warn!(
                        surface_id = info.surface_id,
                        h_out_w_m2_k = info.h_out_w_m2_k,
                        "window exterior film coefficient is non-positive; \
                         using ASHRAE conventional fallback ({H_OUT_NFRC} W/(m²·K)). \
                         This should be rare — check upstream boundary film computation."
                    );
                    H_OUT_NFRC
                };
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
                    });
                continue;
            }

            // For surfaces with no film resistance in the conduction path (rad_frac == 0),
            // fall back to the simple (non-iterative) calculation.
            if info.rad_frac <= 0.0 {
                let t_node_c = self.x[info.state_index];
                let surface = ExteriorSurface {
                    area_m2: info.area_m2,
                    emissivity: info.emissivity,
                    sky_view_factor: sky_view_factor(info.tilt_deg),
                    beta: beta_factor(info.tilt_deg),
                };
                let q_lw = exterior_longwave_w(&surface, t_sky_raw, t_ext, t_node_c);
                u[info.input_index] += q_lw;
                #[cfg(any(debug_assertions, feature = "observe_detailed"))]
                self.ext_surface_diag_buf
                    .push(super::config::ExtSurfaceDiag {
                        surface_id: info.surface_id,
                        category: info.boundary_category,
                        solar_absorbed_w: 0.0,
                        lwr_gain_w: q_lw,
                        surface_temp_c: t_node_c,
                        injected_w: q_lw,
                    });
                continue;
            }

            let e_factor = info.emissivity * STEFAN_BOLTZMANN * info.area_m2;
            let f_sky = sky_view_factor(info.tilt_deg);
            let beta = beta_factor(info.tilt_deg);
            let t_node_c = self.x[info.state_index];

            // Per-surface solar gain [W] for the iteration (no allocation).
            let solar_w = env
                .weather
                .solar_irradiance
                .iter()
                .find(|s| s.surface_id == info.surface_id)
                .map(|irr| {
                    info.absorptance
                        * info.area_m2
                        * (irr.direct_w_m2 + irr.diffuse_w_m2 + irr.reflected_w_m2)
                })
                .unwrap_or(0.0);

            // Incoming LWR (environment → surface), independent of surface temp.
            let t_air_k4 = (t_ext + CELSIUS_TO_KELVIN).powi(4);
            let h_lwr_inj = if !t_sky_valid {
                e_factor * t_air_k4
            } else {
                let t_sky_k4 = (t_sky_raw + CELSIUS_TO_KELVIN).powi(4);
                e_factor * ((1.0 - beta * f_sky) * t_air_k4 + beta * f_sky * t_sky_k4)
            };

            // Initial surface temperature estimate from linear interpolation.
            let t_surf_init = info.rad_frac * t_node_c + (1.0 - info.rad_frac) * t_ext;

            // Iterative solve with heavy-ball damping (matches OCHRE).
            let mut t_surf = self.exterior_surface_temps[i];
            let mut t_surf_prev = self.exterior_surface_temps[i];

            for _ in 0..info.n_iter {
                let lwr = h_lwr_inj - e_factor * (t_surf + CELSIUS_TO_KELVIN).powi(4);
                let t_new = t_surf_init + (solar_w + lwr) * info.rad_res_k_w;
                let t_new = t_new.clamp(t_surf - 2.0, t_surf + 2.0);
                let t_next = t_surf + 0.5 * (t_new - t_surf) + 0.1 * (t_surf - t_surf_prev);
                t_surf_prev = t_surf;
                t_surf = t_next;
                // Converged when the per-iteration surface-temperature step is small.
                if (t_surf - t_surf_prev).abs() < 0.01 {
                    break;
                }
            }

            self.exterior_surface_temps[i] = t_surf;

            let q_lw = h_lwr_inj - e_factor * (t_surf + CELSIUS_TO_KELVIN).powi(4);
            let injected = (solar_w + q_lw) * info.rad_frac;
            u[info.input_index] += injected;

            #[cfg(any(debug_assertions, feature = "observe_detailed"))]
            self.ext_surface_diag_buf
                .push(super::config::ExtSurfaceDiag {
                    surface_id: info.surface_id,
                    category: info.boundary_category,
                    solar_absorbed_w: solar_w,
                    lwr_gain_w: q_lw,
                    surface_temp_c: t_surf,
                    injected_w: injected,
                });
        }
    }

    /// Applies interior longwave radiation exchange for each configured zone.
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
                .unwrap_or_else(|| {
                    tracing::debug!(zone_id = ?zone_cfg.zone_id, "interior LWR: zone not found in env, using 20°C");
                    20.0
                });

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
        let delta_q_w = (u_factor / H_OUT_NFRC) * delta_q_w_m2 * area_m2;

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
        let delta_q_w = (u_factor / H_OUT_NFRC) * delta_q_w_m2 * area_m2;

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
        let scaled_delta = (u_factor / H_OUT_NFRC) * delta_q_w_m2 * area_m2;

        let ratio = scaled_delta / raw_delta;
        let expected_ratio = u_factor / H_OUT_NFRC;
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

        let delta_nfrc = (u_factor / H_OUT_NFRC) * delta_q_w_m2 * area_m2;
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
        let delta_v = (u_factor / H_OUT_NFRC)
            * epsilon
            * STEFAN_BOLTZMANN
            * beta_v
            * f_sky_v
            * (t_sky_k.powi(4) - t_air_k.powi(4))
            * area_m2;

        let f_sky_h = sky_view_factor(0.0);
        let beta_h = beta_factor(0.0);
        let delta_h = (u_factor / H_OUT_NFRC)
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

    /// Verify: H_OUT_NFRC matches the ASHRAE conventional combined exterior
    /// film coefficient for peak heating load calculations.
    /// ASHRAE HoF 2021 Ch. 15, Table 1; Engineers Edge (citing ASHRAE).
    #[test]
    fn h_out_nfrc_matches_standard() {
        assert!(
            (H_OUT_NFRC - 34.0).abs() < 1e-9,
            "H_OUT_NFRC should be 34.0 W/(m²·K) per ASHRAE conventional peak-load value"
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
        let q_window = 669.0_f64;
        let zone_air_injection = q_window * (1.0 - radiation_frac);
        let to_cond_path = q_window * radiation_frac;

        assert!(
            (zone_air_injection - 428.0).abs() < 5.0,
            "zone-air injection should be ~428 W, got {zone_air_injection:.1}"
        );
        assert!(
            (to_cond_path - 241.0).abs() < 2.0,
            "to-conduction fraction should be ~241 W, got {to_cond_path:.1}"
        );
        assert!(
            zone_air_injection < q_window,
            "zone-air injection must be less than full q"
        );
        assert!(
            zone_air_injection > 0.0,
            "zone-air injection must be positive for cold window (net LWR gain)"
        );
        assert!(
            (zone_air_injection + to_cond_path - q_window).abs() < 1e-9,
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

    // ── Regression: window exterior LWR uses computed h_out, not NFRC fallback ──
    //
    // The guard at longwave.rs:86 previously used `> 1.0` instead of `> 0.0`,
    // causing any h_out_w_m2_k in (0, 1] to silently fall back to H_OUT_NFRC
    // (34 W/(m²·K)) — a 34× to 68× mismatch for valid low-wind conditions.
    // Fixed: guard is now `> 0.0` so any positive computed coefficient is used.
    //
    // `window_lwr_uses_computed_h_out_not_nfrc_fallback` exercises the production
    // code path through `apply_exterior_longwave_inputs_iterative` to verify that
    // the guard at line 86 selects the correct h_out. If the guard reverts to
    // `> 1.0`, h_out=0.5 triggers the NFRC fallback and the injected δq is 68×
    // too small, causing the ratio-based assertion to fail.
    //
    // `window_lwr_falls_back_to_nfrc_for_zero` guards the boundary case that
    // non-positive values still fall back to H_OUT_NFRC.

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
                relative_humidity: 0.45,
                wet_bulb_c: 17.0,
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

    /// Regression: a computed h_out of 0.5 W/(m²·K) is a valid low-wind exterior
    /// film coefficient and must be used directly rather than falling back to the
    /// NFRC default of 34 W/(m²·K).
    ///
    /// Natural convection coefficients for vertical surfaces can approach
    /// 1 W/(m²·K) in still air (ASHRAE HoF 2021 Ch. 4 §4.2); for non-vertical
    /// surfaces they fall well below it. The correct discriminant is `> 0.0`:
    /// any positive computed coefficient is more accurate than the rating-condition
    /// tabulated value.
    ///
    /// Exercises `apply_exterior_longwave_inputs_iterative` — if line 86's guard
    /// reverts to `> 1.0`, h_out=0.5 triggers the NFRC fallback and the injected
    /// δq is 68× too small.
    #[test]
    fn window_lwr_uses_computed_h_out_not_nfrc_fallback() {
        let env = window_lwr_env();

        let mut solver_0_5 = window_lwr_solver(0.5, &env);
        let mut solver_0 = window_lwr_solver(0.0, &env);
        let mut u_0_5 = DVector::zeros(solver_0_5.model.input_dim());
        let mut u_0 = DVector::zeros(solver_0.model.input_dim());

        solver_0_5.apply_exterior_longwave_inputs_iterative(&mut u_0_5, &env);
        solver_0.apply_exterior_longwave_inputs_iterative(&mut u_0, &env);

        // Analytic δq_w_m2 (identical for both; only h_out differs):
        //   δq = ε·σ·β·F_sky·(T_sky⁴ − T_air⁴)
        //   with β = √F_sky = √0.5 ≈ 0.7071
        let f_sky = sky_view_factor(90.0);
        let beta = beta_factor(90.0);
        let t_sky_k = -30.0 + CELSIUS_TO_KELVIN; // 243.15 K
        let t_air_k = -15.0 + CELSIUS_TO_KELVIN; // 258.15 K
        let delta_q_w_m2 =
            0.84 * STEFAN_BOLTZMANN * beta * f_sky * (t_sky_k.powi(4) - t_air_k.powi(4));

        let expected_0_5 = (3.0 / 0.5) * delta_q_w_m2 * 12.0;
        let expected_nfrc = (3.0 / H_OUT_NFRC) * delta_q_w_m2 * 12.0;

        // The solver with h_out=0.5 must use 0.5, not the NFRC fallback.
        let actual_0_5 = u_0_5[1];
        assert!(
            (actual_0_5 - expected_0_5).abs() < 1e-6,
            "h_out=0.5: window LWR correction expected {expected_0_5:.6} W, got {actual_0_5:.6} W. \
             The guard at line 86 may have fallen back to H_OUT_NFRC instead of using the \
             computed coefficient."
        );

        // The solver with h_out=0.0 must fall back to H_OUT_NFRC.
        let actual_0 = u_0[1];
        assert!(
            (actual_0 - expected_nfrc).abs() < 1e-6,
            "h_out=0.0: window LWR correction expected NFRC fallback {expected_nfrc:.6} W, \
             got {actual_0:.6} W"
        );

        // Final sanity: ratio must be 34/0.5 = 68.
        let ratio = actual_0_5 / actual_0;
        assert!(
            (ratio - H_OUT_NFRC / 0.5).abs() < 1e-3,
            "ratio u(0.5)/u(0.0) should be {:.0}, got {ratio}",
            H_OUT_NFRC / 0.5
        );
    }

    /// Guard-condition regression: h_out_w_m2_k = 0.0 must still use the
    /// NFRC fallback after the threshold is corrected to `> 0.0`.
    ///
    /// Exercises `apply_exterior_longwave_inputs_iterative` — if the guard
    /// were changed to accept zero (e.g. `>= 0.0` instead of `> 0.0`), a
    /// computed coefficient of zero would be used in the denominator,
    /// producing a NaN or infinite δq.
    #[test]
    fn window_lwr_falls_back_to_nfrc_for_zero() {
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
        let expected = (3.0 / H_OUT_NFRC) * delta_q_w_m2 * 12.0;

        let actual = u[1];
        assert!(
            (actual - expected).abs() < 1e-6,
            "h_out=0.0: window LWR correction should use NFRC fallback ({H_OUT_NFRC} W/(m²·K)), \
             expected {expected:.6} W, got {actual:.6} W"
        );
    }

    // ── H_OUT_NFRC is now exported from hares-physics::film_coefficients ──────
    //
    // Both this module and solver_builder.rs import the same constant, eliminating
    // the duplicate-literal drift risk. The `h_out_nfrc_matches_standard` test
    // above guards the numeric value (34.0 W/(m²·K) per ASHRAE Ch. 15).

}
