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

        for (i, info) in self.config.exterior_surfaces.iter().enumerate() {
            if info.input_index >= u.len() || info.state_index >= self.x.len() {
                continue;
            }

            // Windows: LWR is implicit in U-factor; skip exterior radiation calc.
            // Without this, windows use zone air temp as "surface temp" (rad_frac=0,
            // state_index = zone air), producing massive erroneous LWR cooling.
            if info.boundary_category == Some(super::config::BoundaryCategory::Window) {
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
            let f_gnd = 1.0 - f_sky;
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
                e_factor * ((f_gnd + (1.0 - beta) * f_sky) * t_air_k4 + beta * f_sky * t_sky_k4)
            };

            // Initial surface temperature estimate from linear interpolation.
            let t_surf_init = info.rad_frac * t_node_c + (1.0 - info.rad_frac) * t_ext;

            // Iterative solve with heavy-ball damping (matches OCHRE).
            let mut t_surf = self.exterior_surface_temps[i];
            let mut t_prev_iter = self.exterior_surface_temps[i];

            for _ in 0..info.n_iter {
                let lwr = h_lwr_inj - e_factor * (t_surf + CELSIUS_TO_KELVIN).powi(4);
                let t_new = t_surf_init + (solar_w + lwr) * info.rad_res_k_w;
                let t_new = t_new.clamp(t_surf - 2.0, t_surf + 2.0);
                let t_next = t_surf + 0.5 * (t_new - t_surf) + 0.1 * (t_surf - t_prev_iter);
                t_prev_iter = t_surf;
                t_surf = t_next;
                if (t_surf - t_prev_iter).abs() < 0.01 {
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
        for zone_cfg in &self.config.interior_lwr_zones {
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

            // Reuse pre-allocated buffer for surface temperatures.
            let buf = &mut self.interior_surf_temps_buf;
            buf.clear();
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
            let t_surfaces = &*buf;

            // Use ScriptF (exact T⁴ radiosity) when pre-computed at init,
            // linearized h_r approximation as fallback.
            if let Some(ref scriptf) = zone_cfg.scriptf {
                scriptf.net_flux_w_into(t_surfaces, &mut self.lwr_net_flux_buf);
            } else {
                self.lwr_surfaces_buf.clear();
                self.lwr_surfaces_buf
                    .extend(zone_cfg.surfaces.iter().map(|s| InteriorSurface {
                        area_m2: s.area_m2,
                        emissivity: s.emissivity,
                    }));
                interior_longwave_linearised_w_into(
                    &self.lwr_surfaces_buf,
                    t_surfaces,
                    t_zone_c,
                    &mut self.lwr_net_flux_buf,
                );
            };

            let mut zone_total = 0.0_f64;
            for (_j, (info, &q)) in zone_cfg
                .surfaces
                .iter()
                .zip(self.lwr_net_flux_buf.iter())
                .enumerate()
            {
                if info.input_index < u.len() {
                    u[info.input_index] += q;
                }
                zone_total += q;
                #[cfg(any(debug_assertions, feature = "observe_detailed"))]
                self.int_surface_diag_buf
                    .push(super::config::IntSurfaceDiag {
                        surface_temp_c: t_surfaces[_j],
                        lwr_flux_w: q,
                    });
            }
            self.lwr_by_zone_buf.push((zone_cfg.zone_id, zone_total));
        }
    }
}
