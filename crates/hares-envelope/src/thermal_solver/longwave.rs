//! Interior and exterior longwave radiation application for the thermal solver.
//!
//! Extracts LWR methods from the `ThermalSolver` to keep `mod.rs` focused on
//! orchestration. Both methods operate on the shared input vector `u` and read
//! the current state vector `x` plus exterior surface temperature warm-starts.

use hares_types::{EnvironmentState, ZoneId};
use nalgebra::DVector;

use crate::longwave_radiation::{
    CELSIUS_TO_KELVIN, ExteriorSurface, InteriorSurface, STEFAN_BOLTZMANN, beta_factor,
    exterior_longwave_w, interior_longwave_linearised_w, sky_view_factor,
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

            // Incoming LWR (environment -> surface), independent of surface temp.
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
        }
    }

    /// Applies linearised interior longwave radiation exchange for each configured zone.
    ///
    /// Returns per-zone net interior LWR heat gains [W] for diagnostics output.
    pub(super) fn apply_interior_longwave_inputs(
        &self,
        u: &mut DVector<f64>,
        env: &EnvironmentState,
    ) -> Vec<(ZoneId, f64)> {
        let mut lwr_by_zone = Vec::with_capacity(self.config.interior_lwr_zones.len());
        for zone_cfg in &self.config.interior_lwr_zones {
            if zone_cfg.surfaces.len() < 2 {
                continue;
            }
            let t_zone_c = env
                .zones
                .iter()
                .find(|z| z.id == zone_cfg.zone_id)
                .map(|z| z.temperature_c)
                .unwrap_or(20.0);

            let surfaces: Vec<InteriorSurface> = zone_cfg
                .surfaces
                .iter()
                .map(|s| InteriorSurface {
                    area_m2: s.area_m2,
                    emissivity: s.emissivity,
                })
                .collect();
            let t_surfaces: Vec<f64> = zone_cfg
                .surfaces
                .iter()
                .map(|s| {
                    let t_node = if s.state_index < self.x.len() {
                        self.x[s.state_index]
                    } else {
                        t_zone_c
                    };
                    s.radiation_frac * t_node + (1.0 - s.radiation_frac) * t_zone_c
                })
                .collect();

            let net_lw = interior_longwave_linearised_w(&surfaces, &t_surfaces, t_zone_c);
            let mut zone_total = 0.0_f64;
            for (info, &q) in zone_cfg.surfaces.iter().zip(net_lw.iter()) {
                if info.input_index < u.len() {
                    u[info.input_index] += q;
                }
                zone_total += q;
            }
            lwr_by_zone.push((zone_cfg.zone_id, zone_total));
        }
        lwr_by_zone
    }
}
