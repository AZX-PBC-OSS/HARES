use hares_physics::solar::{GlazingCurve, window_iam};
use hares_types::{EnvironmentState, ZoneId};
use nalgebra::DVector;

use super::ThermalSolver;
use super::config::InteriorSurfaceInfo;

/// Default beam-to-floor fraction for residential buildings.
/// Per EnergyPlus FullInteriorAndExterior: 60% of beam solar hits floors.
const BEAM_FLOOR_FRACTION: f64 = 0.60;

impl ThermalSolver {
    pub(super) fn apply_solar_inputs(&mut self, u: &mut DVector<f64>, env: &EnvironmentState) {
        for irr in &env.weather.solar_irradiance {
            let Some(&idx) = self.wiring.solar_input_indices.get(&irr.surface_id) else {
                continue;
            };
            if idx >= u.len() {
                continue;
            }

            if let Some(win) = self.config.window_properties.get(&irr.surface_id) {
                let curve = GlazingCurve::from_u_shgc(win.u_factor_w_m2_k, win.shgc);
                let iam_beam = window_iam(irr.angle_of_incidence_rad, curve);
                let iam_diffuse = curve.diffuse_iam();

                let poa_beam = irr.direct_w_m2 * iam_beam;
                let poa_diffuse = irr.diffuse_w_m2 * iam_diffuse;
                let poa_w_m2 = poa_beam + poa_diffuse;

                let transmitted_beam_w = win.area_m2 * win.transmittance * poa_beam;
                let transmitted_diffuse_w = win.area_m2 * win.transmittance * poa_diffuse;
                let transmitted_total_w = transmitted_beam_w + transmitted_diffuse_w;

                debug_assert!(
                    win.shgc >= win.transmittance - 1e-6,
                    "SHGC ({}) < transmittance ({}): check window config",
                    win.shgc,
                    win.transmittance
                );
                let absorbed_inward = (win.shgc - win.transmittance).max(0.0) * win.radiation_frac;
                let absorbed_zone_w = win.area_m2 * absorbed_inward * poa_w_m2;

                let _shgc = win.shgc;
                let zone_id = self.config.window_zone_ids.get(&irr.surface_id).copied();
                let air_idx = zone_id
                    .and_then(|zid| self.wiring.zone_sensible_input_indices.get(&zid).copied())
                    .unwrap_or(idx);

                u[air_idx] += absorbed_zone_w;

                let distributed = match zone_id {
                    Some(zid) => self.distribute_transmitted_solar(
                        u,
                        zid,
                        transmitted_beam_w,
                        transmitted_diffuse_w,
                    ),
                    None => false,
                };

                if !distributed {
                    u[air_idx] += transmitted_total_w;
                }

                #[cfg(any(debug_assertions, feature = "observe_detailed"))]
                self.window_solar_diag_buf
                    .push(super::config::WindowSolarDiag {
                        surface_id: irr.surface_id,
                        poa_beam_w_m2: poa_beam,
                        poa_diffuse_w_m2: poa_diffuse,
                        iam_beam,
                        iam_diffuse,
                        transmitted_beam_w,
                        transmitted_diffuse_w,
                        absorbed_zone_w,
                        shgc: _shgc,
                    });
            }
        }
    }

    /// Distribute transmitted window solar to interior surfaces.
    ///
    /// EnergyPlus FullInteriorAndExterior method:
    /// - Beam: 60% to floor surfaces (by area), 40% to walls/ceiling (by area)
    /// - Diffuse: distributed to all surfaces by area
    /// - Each surface absorbs incident × solar_absorptance (single application)
    /// - Reflected fraction (1 - absorptance) goes to zone air node
    ///
    /// Returns `true` if distribution occurred (surfaces found), `false` if no
    /// interior surfaces configured for this zone (caller should use legacy path).
    fn distribute_transmitted_solar(
        &mut self,
        u: &mut DVector<f64>,
        zone_id: ZoneId,
        beam_w: f64,
        diffuse_w: f64,
    ) -> bool {
        let zone_cfg = self
            .config
            .interior_lwr_zones
            .iter()
            .find(|z| z.zone_id == zone_id);
        let Some(zone_cfg) = zone_cfg else {
            return false;
        };
        if zone_cfg.surfaces.is_empty() {
            return false;
        }

        let reflected_w = compute_solar_distribution_into(
            &zone_cfg.surfaces,
            beam_w,
            diffuse_w,
            &mut self.solar_absorbed_buf,
        );

        // Split absorbed solar via radiation_frac (OCHRE RC voltage-divider model):
        //   to_surface_node = absorbed × radiation_frac     (heat into RC mass node)
        //   to_zone_air     = absorbed × (1 - radiation_frac) (bypasses to zone air)
        // For thin interior surfaces (drywall), radiation_frac ≈ 0.15 → ~85% to air.
        let mut zone_air_gain = reflected_w;
        for (s, &absorbed) in zone_cfg.surfaces.iter().zip(self.solar_absorbed_buf.iter()) {
            let to_node = absorbed * s.radiation_frac;
            let to_air = absorbed * (1.0 - s.radiation_frac);
            if s.input_index < u.len() && to_node > 0.0 {
                u[s.input_index] += to_node;
            }
            zone_air_gain += to_air;
        }
        if let Some(&air_idx) = self.wiring.zone_sensible_input_indices.get(&zone_id) {
            if air_idx < u.len() && zone_air_gain > 0.0 {
                u[air_idx] += zone_air_gain;
            }
        }

        true
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
            if info.rad_frac > 0.0 {
                continue;
            }
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

/// Pure solar distribution calculation. Extracted for testability.
///
/// Distributes `beam_w` and `diffuse_w` to surfaces. Returns per-surface
/// absorbed [W] and total reflected to zone air [W].
///
/// Energy conservation: `beam_w + diffuse_w == Σ absorbed + reflected`.
#[cfg(test)]
pub(crate) fn compute_solar_distribution(
    surfaces: &[InteriorSurfaceInfo],
    beam_w: f64,
    diffuse_w: f64,
) -> (Vec<f64>, f64) {
    let n = surfaces.len();
    let mut absorbed = vec![0.0_f64; n];
    let reflected = compute_solar_distribution_into(surfaces, beam_w, diffuse_w, &mut absorbed);
    (absorbed, reflected)
}

/// Like `compute_solar_distribution` but writes into a caller-owned buffer.
///
/// `absorbed_buf` is resized and zeroed as needed. Returns total reflected [W].
pub(crate) fn compute_solar_distribution_into(
    surfaces: &[InteriorSurfaceInfo],
    beam_w: f64,
    diffuse_w: f64,
    absorbed: &mut Vec<f64>,
) -> f64 {
    let n = surfaces.len();
    absorbed.clear();
    absorbed.resize(n, 0.0);
    let floor_area: f64 = surfaces
        .iter()
        .filter(|s| s.is_floor)
        .map(|s| s.area_m2)
        .sum();
    let nonfloor_area: f64 = surfaces
        .iter()
        .filter(|s| !s.is_floor)
        .map(|s| s.area_m2)
        .sum();
    let total_area: f64 = surfaces.iter().map(|s| s.area_m2).sum();

    // Beam: 60% to floors by area, 40% to walls by area, then absorb once.
    if beam_w > 0.0 {
        let beam_to_floors = beam_w * BEAM_FLOOR_FRACTION;
        let beam_to_walls = beam_w * (1.0 - BEAM_FLOOR_FRACTION);
        for (i, s) in surfaces.iter().enumerate() {
            let incident = if s.is_floor && floor_area > 0.0 {
                beam_to_floors * (s.area_m2 / floor_area)
            } else if !s.is_floor && nonfloor_area > 0.0 {
                beam_to_walls * (s.area_m2 / nonfloor_area)
            } else {
                0.0
            };
            absorbed[i] += incident * s.solar_absorptance;
        }
    }

    // Diffuse: all surfaces by area, then absorb once.
    if diffuse_w > 0.0 && total_area > 0.0 {
        for (i, s) in surfaces.iter().enumerate() {
            let incident = diffuse_w * (s.area_m2 / total_area);
            absorbed[i] += incident * s.solar_absorptance;
        }
    }

    let total_absorbed: f64 = absorbed.iter().sum();
    let total_input = beam_w + diffuse_w;
    let reflected = total_input - total_absorbed;
    debug_assert!(
        reflected >= -1e-6,
        "solar distribution: absorbed ({total_absorbed}) exceeds input ({total_input})"
    );
    reflected.max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_surface(area: f64, absorptance: f64, is_floor: bool) -> InteriorSurfaceInfo {
        InteriorSurfaceInfo {
            state_index: 0,
            input_index: 0,
            area_m2: area,
            emissivity: 0.9,
            radiation_frac: 1.0,
            solar_absorptance: absorptance,
            is_floor,
        }
    }

    #[test]
    fn solar_distribution_conserves_energy() {
        let surfaces = vec![
            make_surface(40.0, 0.6, true),  // floor
            make_surface(40.0, 0.5, false), // ceiling
            make_surface(20.0, 0.5, false), // wall 1
            make_surface(20.0, 0.5, false), // wall 2
        ];
        let beam = 500.0;
        let diffuse = 200.0;
        let (absorbed, reflected) = compute_solar_distribution(&surfaces, beam, diffuse);

        let total_absorbed: f64 = absorbed.iter().sum();
        let total = total_absorbed + reflected;
        assert!(
            (total - (beam + diffuse)).abs() < 1e-6,
            "energy conservation: absorbed({total_absorbed}) + reflected({reflected}) = {total}, expected {}",
            beam + diffuse
        );
    }

    #[test]
    fn floor_gets_majority_of_beam() {
        let surfaces = vec![
            make_surface(40.0, 0.6, true),  // floor
            make_surface(40.0, 0.5, false), // ceiling
            make_surface(20.0, 0.5, false), // wall 1
            make_surface(20.0, 0.5, false), // wall 2
        ];
        let beam = 1000.0;
        let diffuse = 0.0;
        let (absorbed, _) = compute_solar_distribution(&surfaces, beam, diffuse);

        // Floor should absorb 60% × 1.0 (sole floor) × 0.6 (absorptance) = 360 W
        // Walls should absorb 40% split by area × absorptance
        assert!(
            absorbed[0] > absorbed[1],
            "floor should absorb more beam than ceiling: floor={}, ceiling={}",
            absorbed[0],
            absorbed[1]
        );
        // Floor fraction of total absorbed should be > 50%
        let total: f64 = absorbed.iter().sum();
        let floor_fraction = absorbed[0] / total;
        assert!(
            floor_fraction > 0.5,
            "floor should get >50% of absorbed beam, got {:.0}%",
            floor_fraction * 100.0
        );
    }

    #[test]
    fn no_absorptance_means_all_reflected() {
        let surfaces = vec![
            make_surface(40.0, 0.0, true),  // perfectly reflective floor
            make_surface(40.0, 0.0, false), // perfectly reflective ceiling
        ];
        let (absorbed, reflected) = compute_solar_distribution(&surfaces, 500.0, 200.0);
        let total_absorbed: f64 = absorbed.iter().sum();
        assert!(
            total_absorbed.abs() < 1e-10,
            "zero absorptance should mean zero absorption, got {total_absorbed}"
        );
        assert!(
            (reflected - 700.0).abs() < 1e-6,
            "all solar should be reflected, got {reflected}"
        );
    }

    #[test]
    fn full_absorptance_means_no_reflection() {
        let surfaces = vec![
            make_surface(50.0, 1.0, true),  // blackbody floor
            make_surface(50.0, 1.0, false), // blackbody ceiling
        ];
        let (absorbed, reflected) = compute_solar_distribution(&surfaces, 500.0, 200.0);
        let total_absorbed: f64 = absorbed.iter().sum();
        assert!(
            (total_absorbed - 700.0).abs() < 1e-6,
            "full absorptance should absorb everything, got {total_absorbed}"
        );
        assert!(
            reflected.abs() < 1e-6,
            "no reflection expected, got {reflected}"
        );
    }

    #[test]
    fn diffuse_only_distributes_by_area() {
        let surfaces = vec![
            make_surface(30.0, 0.6, false), // wall A (30 m²)
            make_surface(10.0, 0.6, false), // wall B (10 m²)
        ];
        let (absorbed, _) = compute_solar_distribution(&surfaces, 0.0, 400.0);

        // Wall A gets 3× the incident of wall B (area ratio 30:10)
        // Both have same absorptance so absorbed ratio = area ratio
        assert!(
            (absorbed[0] / absorbed[1] - 3.0).abs() < 0.01,
            "diffuse should distribute by area: A/B = {:.2}, expected 3.0",
            absorbed[0] / absorbed[1]
        );
    }

    #[test]
    fn beam_with_no_floors_distributes_to_walls() {
        let surfaces = vec![
            make_surface(20.0, 0.5, false),
            make_surface(20.0, 0.5, false),
        ];
        let (absorbed, reflected) = compute_solar_distribution(&surfaces, 1000.0, 0.0);
        let total: f64 = absorbed.iter().sum();
        // No floors: floor portion (60%) has nowhere to go... actually floor_area=0
        // so beam_to_floors × (area/0) = 0. Only wall portion distributes.
        // beam_to_walls = 400, each wall gets 200, absorbed = 200×0.5 = 100 each
        assert!(
            (total - 200.0).abs() < 1e-6,
            "walls absorb 40% × 0.5 = 200 W total, got {total}"
        );
        assert!(
            (reflected - 800.0).abs() < 1e-6,
            "60% beam to floors lost (no floors) + 40% wall reflection = 800 W, got {reflected}"
        );
    }

    #[test]
    fn zero_input_returns_zeros() {
        let surfaces = vec![
            make_surface(40.0, 0.6, true),
            make_surface(40.0, 0.5, false),
        ];
        let (absorbed, reflected) = compute_solar_distribution(&surfaces, 0.0, 0.0);
        assert!(absorbed.iter().all(|&q| q == 0.0));
        assert_eq!(reflected, 0.0);
    }

    #[test]
    fn single_surface_receives_solar() {
        let surfaces = vec![make_surface(20.0, 0.7, true)];
        let (absorbed, reflected) = compute_solar_distribution(&surfaces, 500.0, 200.0);
        // Single floor: gets 60% beam = 300, plus all diffuse = 200.
        // Absorbed = 500 × 0.7 = 350. But 40% beam (200) has no wall target.
        // Total incident on floor: 300 (beam floor) + 200 (diffuse) = 500.
        // Absorbed: 500 × 0.7 = 350. Reflected: 700 - 350 = 350.
        let total = absorbed[0] + reflected;
        assert!(
            (total - 700.0).abs() < 1e-6,
            "energy conservation: {total} != 700"
        );
    }

    #[test]
    fn only_floors_loses_wall_beam_fraction() {
        let surfaces = vec![make_surface(30.0, 0.6, true), make_surface(20.0, 0.6, true)];
        let (absorbed, reflected) = compute_solar_distribution(&surfaces, 1000.0, 0.0);
        // All surfaces are floors. 60% beam (600) goes to floors.
        // 40% beam (400) goes to nonfloor_area=0 → incident=0 for walls.
        // Floor absorbed: 600 × 0.6 = 360.
        // Reflected: 1000 - 360 = 640 (includes lost 40% wall beam + floor reflection).
        let total_absorbed: f64 = absorbed.iter().sum();
        assert!(
            (total_absorbed - 360.0).abs() < 1e-6,
            "only-floors: expected 360 absorbed, got {total_absorbed}"
        );
        assert!(
            (reflected - 640.0).abs() < 1e-6,
            "only-floors: expected 640 reflected, got {reflected}"
        );
    }
}
