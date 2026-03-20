use hares_physics::solar::{GlazingCurve, window_iam};
use hares_types::EnvironmentState;
use nalgebra::DVector;

use super::ThermalSolver;

impl ThermalSolver {
    pub(super) fn apply_solar_inputs(&self, u: &mut DVector<f64>, env: &EnvironmentState) {
        for irr in &env.weather.solar_irradiance {
            let Some(&idx) = self.config.solar_input_indices.get(&irr.surface_id) else {
                continue;
            };
            if idx >= u.len() {
                continue;
            }

            if let Some(win) = self.config.window_properties.get(&irr.surface_id) {
                // Window: EnergyPlus IAM correction with decomposed solar gain.
                let curve = GlazingCurve::from_u_shgc(win.u_factor_w_m2_k, win.shgc);
                let iam_beam = window_iam(irr.angle_of_incidence_rad, curve);
                let iam_diffuse = curve.diffuse_iam();

                // IAM-corrected plane-of-array irradiance [W/m²].
                let poa_beam = irr.direct_w_m2 * iam_beam;
                let poa_diffuse = (irr.diffuse_w_m2 + irr.reflected_w_m2) * iam_diffuse;
                let poa_w_m2 = poa_beam + poa_diffuse;

                // Transmitted solar: passes directly through glass to zone.
                let transmitted_w = win.area_m2 * win.transmittance * poa_w_m2;

                // Absorbed glass heat decomposition per ASHRAE Ch. 15:
                //   SHGC = T_sol + A_sol × N_i
                //   A_sol = (SHGC - T_sol) / N_i
                // Zone receives the inward-flowing fraction of absorbed solar:
                //   absorbed_zone = A_sol × N_i × POA × area = (SHGC - T_sol) × POA × area
                // Exterior receives the rest: A_sol × (1 - N_i) — lost to outdoor convection.
                let absorbed_inward = (win.shgc - win.transmittance).max(0.0);
                let absorbed_zone_w = win.area_m2 * absorbed_inward * poa_w_m2;

                u[idx] += transmitted_w + absorbed_zone_w;
            }
            // Opaque solar is handled by apply_exterior_solar_inputs via ExteriorSurfaceInfo.
        }
    }

    /// Delivers opaque solar gain to exterior surfaces via [`ExteriorSurfaceInfo`].
    ///
    /// `u[input_index] += (direct + diffuse + reflected) × absorptance × area_m2`
    ///
    /// Skips surfaces that are windows (handled by [`apply_solar_inputs`] via SHGC)
    /// and surfaces with `rad_frac > 0` (handled by the iterative LWR path).
    pub(super) fn apply_exterior_solar_inputs(&self, u: &mut DVector<f64>, env: &EnvironmentState) {
        for info in &self.config.exterior_surfaces {
            if info.input_index >= u.len() {
                continue;
            }
            // Surfaces with rad_frac > 0 get solar via the iterative LWR path
            // which applies the combined (solar + LWR) × rad_frac correctly.
            if info.rad_frac > 0.0 {
                continue;
            }
            // Windows get solar via apply_solar_inputs (SHGC/IAM path).
            // Don't also apply opaque absorptance — that would double-count.
            if self.config.window_properties.contains_key(&info.surface_id) {
                continue;
            }
            let Some(irr) = env
                .weather
                .solar_irradiance
                .iter()
                .find(|s| s.surface_id == info.surface_id)
            else {
                continue;
            };
            let poa_w_m2 = irr.direct_w_m2 + irr.diffuse_w_m2 + irr.reflected_w_m2;
            u[info.input_index] += info.absorptance * info.area_m2 * poa_w_m2;
        }
    }
}
