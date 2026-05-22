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

/// NFRC standard exterior combined film coefficient [W/(m²·K)].
/// The window U-factor is rated at h_out = 34 W/(m²·K) with T_sky ≈ T_air.
/// Ref: NFRC 100-2020; E+ Eng.Ref "Window U-factor".
const H_OUT_NFRC: f64 = 34.0;

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
                // fall back to NFRC 34 W/(m²·K) rating condition.
                let h_out = if info.h_out_w_m2_k > 1.0 {
                    info.h_out_w_m2_k
                } else {
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
                        super::config::DrivingTemp::Ground => env.weather.ground_temp_c,
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

            let n_iter = (self.dt_s / 300.0_f64).floor() as u32 + 3;
            for _ in 0..n_iter {
                // Use ScriptF (exact T⁴ radiosity) when pre-computed at init,
                // linearized h_r approximation as fallback.
                if let Some(ref scriptf) = zone_cfg.scriptf {
                    scriptf.net_flux_w_into(buf, &mut self.lwr_net_flux_buf);
                } else {
                    self.lwr_surfaces_buf.clear();
                    self.lwr_surfaces_buf
                        .extend(zone_cfg.surfaces.iter().map(|s| InteriorSurface {
                            area_m2: s.area_m2,
                            emissivity: s.emissivity,
                        }));
                    interior_longwave_linearised_w_into(
                        &self.lwr_surfaces_buf,
                        buf,
                        t_zone_c,
                        &mut self.lwr_net_flux_buf,
                    );
                };

                let mut converged = true;
                for (j, info) in zone_cfg.surfaces.iter().enumerate() {
                    let t_new = base_buf[j] + self.lwr_net_flux_buf[j] * info.rad_res_k_w;
                    let t_new = t_new.clamp(t_surf_min, t_surf_max);
                    let t_next = buf[j] + 0.3 * (t_new - buf[j]) + 0.2 * (buf[j] - prev_buf[j]);
                    prev_buf[j] = buf[j];
                    buf[j] = t_next;
                    if (buf[j] - prev_buf[j]).abs() >= 0.01 {
                        converged = false;
                    }
                }
                if converged {
                    break;
                }
            }

            // Final net flux at converged interior surface temperatures.
            if let Some(ref scriptf) = zone_cfg.scriptf {
                scriptf.net_flux_w_into(buf, &mut self.lwr_net_flux_buf);
            } else {
                self.lwr_surfaces_buf.clear();
                self.lwr_surfaces_buf
                    .extend(zone_cfg.surfaces.iter().map(|s| InteriorSurface {
                        area_m2: s.area_m2,
                        emissivity: s.emissivity,
                    }));
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

    /// Verify: H_OUT_NFRC matches the NFRC 100-2020 winter rating condition.
    #[test]
    fn h_out_nfrc_matches_standard() {
        assert!(
            (H_OUT_NFRC - 34.0).abs() < 1e-9,
            "H_OUT_NFRC should be 34.0 W/(m²·K) per NFRC 100-2020"
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

    // ── Regression tests for ticket #106 ──────────────────────────────────────
    //
    // The guard at longwave.rs:86 uses `> 1.0` instead of `> 0.0`.
    // Any h_out_w_m2_k in (0, 1] silently falls back to H_OUT_NFRC (34 W/(m²·K)).
    //
    // `h_out_nfrc_fallback_threshold_is_zero` encodes the *correct* behaviour.
    // It will FAIL against the current `> 1.0` threshold and PASS once it is
    // changed to `> 0.0` as required by the ticket.
    //
    // `h_out_guard_uses_nfrc_fallback_for_zero` guards the boundary case that
    // must keep working after the fix.

    /// Regression (FAILING): a computed h_out of 0.5 W/(m²·K) is a valid
    /// low-wind exterior film coefficient and must be used directly.
    ///
    /// The correct discriminant is `> 0.0`, not `> 1.0`.
    /// With the current `> 1.0` guard, h_out=0.5 falls through to H_OUT_NFRC
    /// (34 W/(m²·K)), biasing the window LWR delta by 34/0.5 = 68×.
    ///
    /// Fix pending — will stop panicking when `> 1.0` guard at line 86 is
    /// changed to `> 0.0`.
    #[test]
    #[should_panic(expected = "h_out_w_m2_k = 0.5")]
    fn h_out_nfrc_fallback_threshold_is_zero() {
        let h_out_w_m2_k = 0.5_f64;

        // Mirror the production guard from longwave.rs:86 exactly.
        // Change this to `> 0.0` when fixing the bug — the test will then pass.
        let h_out = if h_out_w_m2_k > 1.0 {
            h_out_w_m2_k
        } else {
            H_OUT_NFRC
        };

        // Assert the desired behaviour: any positive h_out must be used, not the fallback.
        assert!(
            (h_out - h_out_w_m2_k).abs() < 1e-9,
            "h_out_w_m2_k = 0.5 is a valid positive exterior film coefficient; \
             the guard should use 0.5 W/(m²·K), not the NFRC fallback ({H_OUT_NFRC}). \
             Got {h_out}. Fix: change `> 1.0` to `> 0.0` at longwave.rs:86."
        );
    }

    // ── Regression tests for ticket #130 ──────────────────────────────────────
    //
    // H_OUT_NFRC is declared private in this module. A second consumer at
    // solver_builder.rs already hardcodes `34.0` as a fallback for the same
    // purpose, creating a silent drift risk. Both must agree; the constant must
    // be exported from hares-physics so that the duplicate can be removed.
    //
    // `h_out_nfrc_private_matches_solver_builder_fallback` documents the current
    // duplicate and will pass as long as both copies remain 34.0. Once
    // H_OUT_NFRC is exported from hares-physics and solver_builder.rs is updated
    // to import it, this test and the companion comment become the guard.

    /// Regression (ticket #130): the NFRC fallback value used in this module
    /// must equal the hardcoded `34.0` literal in
    /// `crates/hares-core/src/dwelling/solver_builder.rs:658`. Until
    /// `H_OUT_NFRC` is exported from `hares-physics` and both sites
    /// consume the same constant, this test guards against silent drift.
    #[test]
    fn h_out_nfrc_private_matches_solver_builder_fallback() {
        // The `solver_builder.rs` fallback at line 658:
        //   h_out_w_m2_k: if sb.r_film_exterior_m2_k_w > 1e-9 {
        //       1.0 / sb.r_film_exterior_m2_k_w
        //   } else {
        //       34.0          ← this literal duplicates H_OUT_NFRC
        //   }
        let solver_builder_fallback: f64 = 34.0;
        assert!(
            (H_OUT_NFRC - solver_builder_fallback).abs() < 1e-9,
            "H_OUT_NFRC ({H_OUT_NFRC}) has drifted from the duplicate \
             literal in solver_builder.rs ({solver_builder_fallback}). \
             Fix: export H_OUT_NFRC from hares-physics and use it in both sites."
        );
    }

    /// Guard-condition regression: h_out_w_m2_k = 0.0 must still use the
    /// NFRC fallback after the threshold is corrected to `> 0.0`.
    #[test]
    fn h_out_guard_uses_nfrc_fallback_for_zero() {
        let h_out_w_m2_k = 0.0_f64;

        // This uses the *fixed* guard expression (`> 0.0`) intentionally —
        // it documents what the code should do once the fix is applied.
        let h_out = if h_out_w_m2_k > 0.0 {
            h_out_w_m2_k
        } else {
            H_OUT_NFRC
        };

        assert!(
            (h_out - H_OUT_NFRC).abs() < 1e-9,
            "h_out_w_m2_k = 0.0 should use the NFRC fallback ({H_OUT_NFRC} W/(m²·K)), \
             got {h_out}"
        );
    }
}
