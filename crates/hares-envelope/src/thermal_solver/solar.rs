use chrono::Datelike;
use hares_physics::solar::window_iam;
use hares_types::{EnvironmentState, ZoneId};
use nalgebra::DVector;

use super::ThermalSolver;
use super::config::{InteriorSolarSurfaceInfo, InteriorSurfaceInfo};

/// Fraction of transmitted beam solar that strikes the floor, as a function of
/// solar altitude.
///
/// Uses `sin(altitude)` as a monotone heuristic: at 0° (horizontal beam, sunrise/
/// sunset) beam enters nearly parallel to the floor and strikes walls, giving a
/// floor fraction of 0.0. At 90° (solar zenith) beam strikes the floor
/// predominantly. Upper clamp 0.9: even overhead sun leaves some beam on walls
/// and ceiling through window reveals and diffuse scattering.
///
/// **This is a heuristic approximation, not a physics-derived model.** The
/// authoritative approach is the EnergyPlus FullInteriorAndExterior polygon-overlap
/// method (EnergyPlus Engineering Reference, Shading Module), which projects sun
/// rays geometrically onto each interior surface. That refactor is deferred; this
/// function is the immediate deliverable and must at minimum be physically monotone
/// and pass through zero, which it now does.
#[inline]
fn beam_floor_fraction(solar_altitude_deg: f64) -> f64 {
    solar_altitude_deg.to_radians().sin().clamp(0.0, 0.9)
}

impl ThermalSolver {
    pub(super) fn apply_solar_inputs(&mut self, u: &mut DVector<f64>, env: &EnvironmentState) {
        let month = env.current_time.month();
        // ANSI/RESNET 301: winter = October through April (months 10–4).
        let is_winter = month >= 10 || month <= 4;

        for irr in &env.weather.solar_irradiance {
            let Some(&idx) = self.wiring.solar_input_indices.get(&irr.surface_id) else {
                continue;
            };
            if idx >= u.len() {
                continue;
            }

            if let Some(win) = self.config.window_properties.get(&irr.surface_id) {
                let curve = win.glazing_curve;
                let iam_beam = window_iam(irr.angle_of_incidence_rad, curve);
                let iam_diffuse = curve.diffuse_iam();

                let poa_beam = irr.direct_w_m2 * iam_beam;
                let poa_diffuse = (irr.diffuse_w_m2 + irr.reflected_w_m2) * iam_diffuse;
                let poa_w_m2 = poa_beam + poa_diffuse;

                let transmittance = if is_winter {
                    win.winter_transmittance
                } else {
                    win.transmittance
                };
                let shgc = if is_winter { win.winter_shgc } else { win.shgc };

                let transmitted_beam_w = win.area_m2 * transmittance * poa_beam;
                let transmitted_diffuse_w = win.area_m2 * transmittance * poa_diffuse;
                let transmitted_total_w = transmitted_beam_w + transmitted_diffuse_w;

                assert!(
                    shgc >= transmittance - 1e-6,
                    "SHGC ({}) < transmittance ({}): check window config",
                    shgc,
                    transmittance
                );
                // N_i: inward-flowing fraction of glass-absorbed solar (EnergyPlus window model).
                // Distinct from surface radiation_frac used for LWR exchange.
                let absorbed_inward = (shgc - transmittance).max(0.0) * win.radiation_frac;
                let absorbed_zone_w = win.area_m2 * absorbed_inward * poa_w_m2;

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
                        env.weather.solar_altitude_deg,
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
                        shgc,
                    });
            }
        }
    }

    fn distribute_transmitted_solar(
        &mut self,
        u: &mut DVector<f64>,
        zone_id: ZoneId,
        beam_w: f64,
        diffuse_w: f64,
        solar_altitude_deg: f64,
    ) -> bool {
        // Prefer interior_lwr_zones (populated in ScriptF mode, which has
        // full InteriorSurfaceInfo), fall back to interior_solar_zones
        // (populated in StarMesh mode).
        let lwr_zone = self
            .config
            .interior_lwr_zones
            .iter()
            .find(|z| z.zone_id == zone_id);
        if let Some(zone_cfg) = lwr_zone {
            if !zone_cfg.surfaces.is_empty() {
                let beam_floor_frac = beam_floor_fraction(solar_altitude_deg);
                let reflected_w = compute_solar_distribution_into(
                    &zone_cfg.surfaces,
                    beam_w,
                    diffuse_w,
                    beam_floor_frac,
                    &mut self.solar_absorbed_buf,
                );
                let air_spillover =
                    deposit_solar_to_surface_nodes(&zone_cfg.surfaces, &self.solar_absorbed_buf, u);
                if let Some(&air_idx) = self.wiring.zone_sensible_input_indices.get(&zone_id) {
                    let air_total = reflected_w + air_spillover;
                    if air_idx < u.len() && air_total > 0.0 {
                        u[air_idx] += air_total;
                    }
                }
                return true;
            }
        }

        // StarMesh mode: use interior_solar_zones for distribution.
        let solar_zone = self
            .config
            .interior_solar_zones
            .iter()
            .find(|z| z.zone_id == zone_id);
        let Some(zone_cfg) = solar_zone else {
            return false;
        };
        if zone_cfg.surfaces.is_empty() {
            return false;
        }

        let beam_floor_frac = beam_floor_fraction(solar_altitude_deg);
        let reflected_w = compute_solar_distribution_into_solar(
            &zone_cfg.surfaces,
            beam_w,
            diffuse_w,
            beam_floor_frac,
            &mut self.solar_absorbed_buf,
        );
        let air_spillover =
            deposit_solar_to_solar_surfaces(&zone_cfg.surfaces, &self.solar_absorbed_buf, u);
        if let Some(&air_idx) = self.wiring.zone_sensible_input_indices.get(&zone_id) {
            let air_total = reflected_w + air_spillover;
            if air_idx < u.len() && air_total > 0.0 {
                u[air_idx] += air_total;
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
/// Energy conservation: `beam_w + diffuse_w == Σ absorbed + reflected` always holds.
#[cfg(test)]
pub(crate) fn compute_solar_distribution(
    surfaces: &[InteriorSurfaceInfo],
    beam_w: f64,
    diffuse_w: f64,
    beam_floor_frac: f64,
) -> (Vec<f64>, f64) {
    let n = surfaces.len();
    let mut absorbed = vec![0.0_f64; n];
    let reflected = compute_solar_distribution_into(
        surfaces,
        beam_w,
        diffuse_w,
        beam_floor_frac,
        &mut absorbed,
    );
    (absorbed, reflected)
}

/// Common interface for surface types in solar distribution calculations.
///
/// Both `InteriorSurfaceInfo` (ScriptF mode) and `InteriorSolarSurfaceInfo`
/// (StarMesh mode) implement this, enabling a single generic distribution
/// function that eliminates the ~60-line duplication between the two paths.
pub(crate) trait SolarDistributableSurface {
    fn area_m2(&self) -> f64;
    fn solar_absorptance(&self) -> f64;
    fn is_floor(&self) -> bool;
}

impl SolarDistributableSurface for InteriorSurfaceInfo {
    #[inline]
    fn area_m2(&self) -> f64 {
        self.area_m2
    }
    #[inline]
    fn solar_absorptance(&self) -> f64 {
        self.solar_absorptance
    }
    #[inline]
    fn is_floor(&self) -> bool {
        self.is_floor
    }
}

impl SolarDistributableSurface for InteriorSolarSurfaceInfo {
    #[inline]
    fn area_m2(&self) -> f64 {
        self.area_m2
    }
    #[inline]
    fn solar_absorptance(&self) -> f64 {
        self.solar_absorptance
    }
    #[inline]
    fn is_floor(&self) -> bool {
        self.is_floor
    }
}

/// Generic solar distribution into a caller-owned buffer.
///
/// Parameterised over `S: SolarDistributableSurface` so that both
/// [`InteriorSurfaceInfo`] (ScriptF) and [`InteriorSolarSurfaceInfo`] (StarMesh)
/// paths share a single implementation. Returns total reflected [W].
///
/// `absorbed_buf` is resized and zeroed as needed.
///
/// View factors are normalized by `area × absorptance / Σ(area × absorptance)`.
/// When all surfaces have nonzero absorptance, all solar is distributed. When
/// absorptance sums to zero, all solar is returned as reflected to zone air.
fn compute_solar_distribution_into_generic<S: SolarDistributableSurface>(
    surfaces: &[S],
    beam_w: f64,
    diffuse_w: f64,
    beam_floor_frac: f64,
    absorbed: &mut Vec<f64>,
) -> f64 {
    let n = surfaces.len();
    absorbed.clear();
    absorbed.resize(n, 0.0);

    let floor_wa: f64 = surfaces
        .iter()
        .filter(|s| s.is_floor())
        .map(|s| s.area_m2() * s.solar_absorptance())
        .sum();
    let nonfloor_wa: f64 = surfaces
        .iter()
        .filter(|s| !s.is_floor())
        .map(|s| s.area_m2() * s.solar_absorptance())
        .sum();
    let total_wa: f64 = floor_wa + nonfloor_wa;

    // If one class is absent, redirect the full beam budget to the remaining class.
    if beam_w > 0.0 {
        let (beam_to_floors, beam_to_walls) = if floor_wa > 0.0 && nonfloor_wa > 0.0 {
            (beam_w * beam_floor_frac, beam_w * (1.0 - beam_floor_frac))
        } else if floor_wa > 0.0 {
            (beam_w, 0.0)
        } else if nonfloor_wa > 0.0 {
            (0.0, beam_w)
        } else {
            (0.0, 0.0)
        };
        for (i, s) in surfaces.iter().enumerate() {
            let factor = if s.is_floor() && floor_wa > 0.0 {
                s.area_m2() * s.solar_absorptance() / floor_wa
            } else if !s.is_floor() && nonfloor_wa > 0.0 {
                s.area_m2() * s.solar_absorptance() / nonfloor_wa
            } else {
                0.0
            };
            absorbed[i] += if s.is_floor() {
                beam_to_floors * factor
            } else {
                beam_to_walls * factor
            };
        }
    }

    // Diffuse: all surfaces by area × absorptance, normalized to sum to 1.
    if diffuse_w > 0.0 && total_wa > 0.0 {
        for (i, s) in surfaces.iter().enumerate() {
            let factor = s.area_m2() * s.solar_absorptance() / total_wa;
            absorbed[i] += diffuse_w * factor;
        }
    }

    // Any energy not distributed to surfaces is reflected back to zone air.
    // This handles zero-absorptance surfaces and numerical edge cases.
    let total_distributed: f64 = absorbed.iter().sum();
    (beam_w + diffuse_w) - total_distributed
}

/// Like `compute_solar_distribution` but writes into a caller-owned buffer.
///
/// Delegates to [`compute_solar_distribution_into_generic`] with
/// [`InteriorSurfaceInfo`] (ScriptF mode).
pub(crate) fn compute_solar_distribution_into(
    surfaces: &[InteriorSurfaceInfo],
    beam_w: f64,
    diffuse_w: f64,
    beam_floor_frac: f64,
    absorbed: &mut Vec<f64>,
) -> f64 {
    compute_solar_distribution_into_generic(surfaces, beam_w, diffuse_w, beam_floor_frac, absorbed)
}

/// Deposit per-surface absorbed solar [W] into the state-space input vector `u`.
///
/// Splits each surface's share via `radiation_frac` (the RC network voltage-divider
/// between film resistance and material half-resistance):
///   - `q * radiation_frac` → surface RC node
///   - `q * (1 - radiation_frac)` → returned as zone air contribution
fn deposit_solar_to_surface_nodes(
    surfaces: &[InteriorSurfaceInfo],
    absorbed: &[f64],
    u: &mut DVector<f64>,
) -> f64 {
    let mut air_total = 0.0;
    for (s, &q) in surfaces.iter().zip(absorbed.iter()) {
        if q > 0.0 {
            if s.input_index < u.len() {
                u[s.input_index] += q * s.radiation_frac;
            }
            air_total += q * (1.0 - s.radiation_frac);
        }
    }
    air_total
}

/// Solar distribution for [`InteriorSolarSurfaceInfo`] (StarMesh mode).
///
/// Delegates to [`compute_solar_distribution_into_generic`].
pub(crate) fn compute_solar_distribution_into_solar(
    surfaces: &[InteriorSolarSurfaceInfo],
    beam_w: f64,
    diffuse_w: f64,
    beam_floor_frac: f64,
    absorbed: &mut Vec<f64>,
) -> f64 {
    compute_solar_distribution_into_generic(surfaces, beam_w, diffuse_w, beam_floor_frac, absorbed)
}

/// Deposit per-surface absorbed solar into `u` for [`InteriorSolarSurfaceInfo`].
fn deposit_solar_to_solar_surfaces(
    surfaces: &[InteriorSolarSurfaceInfo],
    absorbed: &[f64],
    u: &mut DVector<f64>,
) -> f64 {
    let mut air_total = 0.0;
    for (s, &q) in surfaces.iter().zip(absorbed.iter()) {
        if q > 0.0 {
            if let Some(idx) = s.input_index {
                if idx < u.len() {
                    u[idx] += q * s.radiation_frac;
                }
            }
            air_total += q * (1.0 - s.radiation_frac);
        }
    }
    air_total
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEGACY_BEAM_FLOOR_FRAC: f64 = 0.6;

    fn make_surface(area: f64, absorptance: f64, is_floor: bool) -> InteriorSurfaceInfo {
        InteriorSurfaceInfo {
            state_index: 0,
            input_index: 0,
            area_m2: area,
            emissivity: 0.9,
            radiation_frac: 1.0,
            rad_res_k_w: 0.0,
            solar_absorptance: absorptance,
            is_floor,
            driving_temp: None,
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
        let (absorbed, reflected) =
            compute_solar_distribution(&surfaces, beam, diffuse, LEGACY_BEAM_FLOOR_FRAC);

        // Normalized view factors: all solar distributed to surfaces, reflected = 0.
        let total_absorbed: f64 = absorbed.iter().sum();
        assert!(
            (total_absorbed - (beam + diffuse)).abs() < 1e-6,
            "all solar should be absorbed: got {total_absorbed}, expected {}",
            beam + diffuse
        );
        assert!(
            reflected.abs() < 1e-6,
            "no reflection with normalized view factors, got {reflected}"
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
        let (absorbed, _) =
            compute_solar_distribution(&surfaces, beam, diffuse, LEGACY_BEAM_FLOOR_FRAC);

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
    fn zero_absorptance_means_zero_distributed() {
        let surfaces = vec![
            make_surface(40.0, 0.0, true),  // perfectly reflective floor
            make_surface(40.0, 0.0, false), // perfectly reflective ceiling
        ];
        let (absorbed, reflected) =
            compute_solar_distribution(&surfaces, 500.0, 200.0, LEGACY_BEAM_FLOOR_FRAC);
        let total_absorbed: f64 = absorbed.iter().sum();
        // With zero absorptance, no distribution occurs -- all energy reflected to zone air.
        assert!(
            total_absorbed.abs() < 1e-10,
            "zero absorptance should mean zero distribution, got {total_absorbed}"
        );
        assert!(
            (reflected - 700.0).abs() < 1e-6,
            "all energy should be reflected when absorptance is zero, got {reflected}"
        );
    }

    #[test]
    fn full_absorptance_means_no_reflection() {
        let surfaces = vec![
            make_surface(50.0, 1.0, true),  // blackbody floor
            make_surface(50.0, 1.0, false), // blackbody ceiling
        ];
        let (absorbed, reflected) =
            compute_solar_distribution(&surfaces, 500.0, 200.0, LEGACY_BEAM_FLOOR_FRAC);
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
        let (absorbed, _) =
            compute_solar_distribution(&surfaces, 0.0, 400.0, LEGACY_BEAM_FLOOR_FRAC);

        // Wall A gets 3× the incident of wall B (area ratio 30:10)
        // Both have same absorptance so absorbed ratio = area ratio
        assert!(
            (absorbed[0] / absorbed[1] - 3.0).abs() < 0.01,
            "diffuse should distribute by area: A/B = {:.2}, expected 3.0",
            absorbed[0] / absorbed[1]
        );
    }

    #[test]
    fn beam_with_no_floors_redirects_to_walls() {
        let surfaces = vec![
            make_surface(20.0, 0.5, false),
            make_surface(20.0, 0.5, false),
        ];
        let (absorbed, reflected) =
            compute_solar_distribution(&surfaces, 1000.0, 0.0, LEGACY_BEAM_FLOOR_FRAC);
        let total: f64 = absorbed.iter().sum();
        // No floors: full beam goes to walls. Normalized: each wall gets 500 W.
        assert!(
            (total - 1000.0).abs() < 1e-6,
            "all beam should be distributed to walls, got {total}"
        );
        assert!(
            reflected.abs() < 1e-6,
            "no reflection with normalized view factors, got {reflected}"
        );
    }

    #[test]
    fn window_solar_includes_ground_reflected() {
        use hares_physics::solar::{GlazingCurve, window_iam};

        let curve = GlazingCurve::E;
        let aoi_rad = 0.0_f64;
        let iam_beam = window_iam(aoi_rad, curve);
        let iam_diffuse = curve.diffuse_iam();

        let direct_w_m2 = 400.0;
        let diffuse_w_m2 = 100.0;
        let reflected_w_m2 = 50.0;
        let area_m2 = 2.0;
        let transmittance = 0.4;

        let poa_beam = direct_w_m2 * iam_beam;
        let poa_diffuse_with = (diffuse_w_m2 + reflected_w_m2) * iam_diffuse;
        let poa_diffuse_without = diffuse_w_m2 * iam_diffuse;

        let transmitted_with = area_m2 * transmittance * (poa_beam + poa_diffuse_with);
        let transmitted_without = area_m2 * transmittance * (poa_beam + poa_diffuse_without);

        let expected_extra = area_m2 * transmittance * reflected_w_m2 * iam_diffuse;
        assert!(
            (transmitted_with - transmitted_without - expected_extra).abs() < 1e-9,
            "ground-reflected contribution mismatch: extra={}, expected={}",
            transmitted_with - transmitted_without,
            expected_extra,
        );
        assert!(
            transmitted_with > transmitted_without,
            "transmitted solar with ground-reflected ({transmitted_with}) should exceed without ({transmitted_without})"
        );
    }

    #[test]
    fn zero_input_returns_zeros() {
        let surfaces = vec![
            make_surface(40.0, 0.6, true),
            make_surface(40.0, 0.5, false),
        ];
        let (absorbed, reflected) =
            compute_solar_distribution(&surfaces, 0.0, 0.0, LEGACY_BEAM_FLOOR_FRAC);
        assert!(absorbed.iter().all(|&q| q == 0.0));
        assert_eq!(reflected, 0.0);
    }

    #[test]
    fn single_surface_receives_all_solar() {
        let surfaces = vec![make_surface(20.0, 0.7, true)];
        let (absorbed, reflected) =
            compute_solar_distribution(&surfaces, 500.0, 200.0, LEGACY_BEAM_FLOOR_FRAC);
        // Single floor: sole surface gets all solar via normalized view factor.
        assert!(
            (absorbed[0] - 700.0).abs() < 1e-6,
            "single floor should absorb all solar, got {}",
            absorbed[0]
        );
        assert!(
            reflected.abs() < 1e-6,
            "no reflection with normalized view factors, got {reflected}"
        );
    }

    fn make_surface_with_rad_frac(
        area: f64,
        absorptance: f64,
        is_floor: bool,
        input_index: usize,
        radiation_frac: f64,
    ) -> InteriorSurfaceInfo {
        InteriorSurfaceInfo {
            state_index: 0,
            input_index,
            area_m2: area,
            emissivity: 0.9,
            radiation_frac,
            rad_res_k_w: 0.0,
            solar_absorptance: absorptance,
            is_floor,
            driving_temp: None,
        }
    }

    #[test]
    fn deposit_applies_radiation_frac_split() {
        let surfaces = vec![
            make_surface_with_rad_frac(40.0, 0.6, true, 0, 0.02),
            make_surface_with_rad_frac(30.0, 0.5, false, 1, 0.02),
        ];
        let beam = 600.0;
        let diffuse = 200.0;

        let mut absorbed = Vec::new();
        let reflected = compute_solar_distribution_into(
            &surfaces,
            beam,
            diffuse,
            LEGACY_BEAM_FLOOR_FRAC,
            &mut absorbed,
        );

        let total_input = beam + diffuse;
        let total_absorbed: f64 = absorbed.iter().sum();
        assert!(
            (total_absorbed + reflected - total_input).abs() < 1e-6,
            "energy conservation violated"
        );

        let mut u = DVector::zeros(3);
        let air_contribution = deposit_solar_to_surface_nodes(&surfaces, &absorbed, &mut u);

        // With radiation_frac = 0.02, only 2% goes to surface nodes.
        assert!(
            (u[0] - absorbed[0] * 0.02).abs() < 1e-10,
            "floor surface node should receive radiation_frac share: u[0]={}, expected={}",
            u[0],
            absorbed[0] * 0.02
        );
        assert!(
            (u[1] - absorbed[1] * 0.02).abs() < 1e-10,
            "wall surface node should receive radiation_frac share: u[1]={}, expected={}",
            u[1],
            absorbed[1] * 0.02
        );

        // 98% goes to zone air.
        let expected_air = absorbed[0] * 0.98 + absorbed[1] * 0.98;
        assert!(
            (air_contribution - expected_air).abs() < 1e-10,
            "zone air should receive (1 - radiation_frac) share: got={}, expected={}",
            air_contribution,
            expected_air
        );

        // Energy conservation: surface deposits + air contribution = total absorbed.
        assert!(
            (u[0] + u[1] + air_contribution - total_absorbed).abs() < 1e-10,
            "energy conservation violated in deposition"
        );
    }

    #[test]
    fn only_floors_receive_full_beam_budget() {
        let surfaces = vec![make_surface(30.0, 0.6, true), make_surface(20.0, 0.6, true)];
        let (absorbed, reflected) =
            compute_solar_distribution(&surfaces, 1000.0, 0.0, LEGACY_BEAM_FLOOR_FRAC);
        // All surfaces are floors with same absorptance. Normalized: all beam distributed.
        let total_absorbed: f64 = absorbed.iter().sum();
        assert!(
            (total_absorbed - 1000.0).abs() < 1e-6,
            "only-floors: all beam should be distributed, got {total_absorbed}"
        );
        assert!(
            reflected.abs() < 1e-6,
            "no reflection with normalized view factors, got {reflected}"
        );
    }

    #[test]
    fn beam_floor_fraction_boundaries() {
        assert_eq!(beam_floor_fraction(0.0), 0.0);
        assert_eq!(beam_floor_fraction(90.0), 0.9);
        assert!((beam_floor_fraction(30.0) - 0.5).abs() < 1e-9);
        assert_eq!(beam_floor_fraction(-10.0), 0.0);

        let alt_sin_09 = (0.9_f64).asin().to_degrees();
        assert!(
            (beam_floor_fraction(alt_sin_09) - 0.9).abs() < 1e-9,
            "clamp should engage at upper bound (sin ≈ 0.9)"
        );
    }

    #[test]
    fn floor_absorbs_more_at_high_altitude() {
        let surfaces = vec![
            make_surface(40.0, 0.6, true),
            make_surface(40.0, 0.5, false),
            make_surface(20.0, 0.5, false),
        ];
        let beam = 1000.0;
        let high_frac = beam_floor_fraction(70.0);
        let low_frac = beam_floor_fraction(10.0);
        let (abs_high, _) = compute_solar_distribution(&surfaces, beam, 0.0, high_frac);
        let (abs_low, _) = compute_solar_distribution(&surfaces, beam, 0.0, low_frac);
        assert!(
            abs_high[0] > abs_low[0],
            "floor should absorb more at 70° ({}) than 10° ({})",
            abs_high[0],
            abs_low[0]
        );
    }

    // Regression guard: lower clamp was historically 0.3, which was physically wrong.
    // At zero altitude (horizontal beam) the floor fraction must be 0.0 (sin(0°) = 0).
    // The 0.3 clamp deposited phantom heat to floor RC nodes at low solar angles.
    #[test]
    fn beam_floor_fraction_zero_altitude_must_be_zero() {
        let frac = beam_floor_fraction(0.0);
        assert_eq!(
            frac, 0.0,
            "beam_floor_fraction(0°) should be 0.0 (sin(0°) = 0), got {frac}"
        );
    }

    // Direction-reversal guard: at very low solar altitude (5°) the non-floor
    // fraction must exceed the floor fraction.
    // sin(5°) ≈ 0.087 — a tight upper bound of 0.15 catches any reintroduction
    // of the old 0.3 lower clamp, which inflated the floor share at low angles.
    #[test]
    fn beam_floor_fraction_low_altitude_walls_dominate() {
        let frac = beam_floor_fraction(5.0);
        assert!(
            frac < 0.5,
            "at 5° solar altitude the floor fraction should be < 0.5, got {frac}"
        );
        // With the buggy 0.3 lower clamp this assertion still passes (0.3 < 0.5),
        // so add a tighter bound: sin(5°) ≈ 0.087, so post-fix fraction ≈ 0.087.
        // Assert it is strictly less than 0.15 to catch the phantom 0.3 inflation.
        assert!(
            frac < 0.15,
            "beam_floor_fraction(5°) should be near sin(5°) ≈ 0.087, not inflated by the 0.3 clamp; got {frac}"
        );
    }

    // ---------------------------------------------------------------------------
    // Regression tests for ticket 049: window absorbed-solar inward fraction
    // ---------------------------------------------------------------------------
    //
    // The EnergyPlus SimpleGlazingSystem model (Engineering Reference, Window
    // Calculation Module, Step 5) computes the inward-flowing fraction of
    // glass-absorbed solar via:
    //
    //   Fracinward = (Ro,s + 0.5 * Rl,w) / (Ro,s + Rl,w + Ri,s)
    //
    // where Ri,s and Ro,s are polynomial functions of (SHGC − Tsol) and
    // Rl,w = 1/U − Ri,w − Ro,w (glass resistance without film coefficients).
    //
    // HARES correctly implements this in `hares_physics::solar::calculate_window_parameters`,
    // which returns `(transmittance, radiation_frac)`.  The `radiation_frac` field
    // in `WindowSolarProperties` IS this inward-flowing fraction — not the RC
    // voltage-divider ratio for longwave exchange.
    //
    // Ticket 049 claims that `radiation_frac` in `WindowSolarProperties` is the
    // "RC network surface-film voltage-divider ratio" and therefore wrong.  These
    // tests demonstrate:
    //   (a) The E+ Step-5 radiation_frac for a typical window is NOT ~0.196
    //       (the ticket's claimed N_i = h_ci/(h_ci+h_co) = 8.3/42.3 value).
    //   (b) The value returned by calculate_window_parameters for a representative
    //       window is in the range expected from the EnergyPlus polynomial model
    //       (≈ 0.40–0.70 for U < 3.4), not 0.196.
    //
    // The FAILING test below checks that the production absorbed_inward calculation
    // (which multiplies by radiation_frac — the EnergyPlus Step-5 value) does NOT
    // equal what the ticket's proposed N_i = h_ci/(h_ci+h_co) ≈ 0.196 would give.
    // After the ticket's fix lands, the compute path must NOT change to use 0.196.

    /// Verify that `calculate_window_parameters` produces a radiation_frac in the
    /// expected range from the EnergyPlus Step 5 polynomial model, and that this
    /// value is materially different from the ticket's proposed N_i = 8.3/42.3
    /// ≈ 0.196.  This is the reference calculation ticket 049 cites as "correct";
    /// the test shows that HARES already uses a more accurate formula.
    #[test]
    fn window_radiation_frac_is_not_nfrc_simple_ratio() {
        // Typical double-pane window: SHGC=0.25, U=1.8 W/m²·K (U < 3.4 branch).
        // r_glass ≈ 1/1.8 − 0.17 − 0.03 ≈ 0.356 m²·K/W (approximate).
        let shgc = 0.25_f64;
        let u = 1.8_f64;
        let r_total = 1.0 / u;
        let r_glass = (r_total - 0.17 - 0.03).max(0.0);
        let (_transmittance, radiation_frac) =
            hares_physics::solar::calculate_window_parameters(shgc, u, r_glass);

        // EnergyPlus Step-5 result for this window is ~0.45–0.60.
        // The ticket's proposed N_i = 8.3/(8.3+34.0) ≈ 0.196.
        let ticket_n_i = 8.3_f64 / (8.3 + 34.0);
        assert!(
            (radiation_frac - ticket_n_i).abs() > 0.10,
            "radiation_frac ({radiation_frac:.3}) should differ materially from ticket N_i ({ticket_n_i:.3}); \
             WindowSolarProperties.radiation_frac is already the EnergyPlus Step-5 inward fraction, \
             not a simple h_ci/(h_ci+h_co) ratio"
        );
        assert!(
            radiation_frac > 0.30,
            "E+ Step-5 radiation_frac for a low-U window should be > 0.30, got {radiation_frac:.3}"
        );
    }

    /// Regression guard: the absorbed_inward formula in apply_solar_inputs uses
    /// WindowSolarProperties.radiation_frac as the EnergyPlus Fracinward, which is
    /// correct.  Using the ticket's proposed N_i = 0.196 instead would underestimate
    /// the inward heat flux by ≈ 2–3× for typical residential windows.
    /// This test documents the expected energy magnitude and guards against
    /// an incorrect replacement of radiation_frac with a fixed 0.196.
    #[test]
    fn absorbed_inward_uses_ep_step5_fraction_not_nfrc_ratio() {
        let shgc = 0.25_f64;
        let transmittance = 0.21_f64;
        let u = 1.8_f64;
        let r_total = 1.0 / u;
        let r_glass = (r_total - 0.17 - 0.03).max(0.0);
        let (_, radiation_frac) =
            hares_physics::solar::calculate_window_parameters(shgc, u, r_glass);

        let area_m2 = 1.0_f64;
        let poa_w_m2 = 1000.0_f64; // 1 kW/m² reference irradiance

        // Production formula (current code):
        let absorbed_inward_ep =
            (shgc - transmittance).max(0.0) * radiation_frac * area_m2 * poa_w_m2;

        // Ticket's proposed formula using N_i = h_ci/(h_ci+h_co):
        let ticket_n_i = 8.3_f64 / (8.3 + 34.0);
        let absorbed_inward_ticket =
            (shgc - transmittance).max(0.0) * ticket_n_i * area_m2 * poa_w_m2;

        // The two should differ by > 20 W (for 1 m² at 1 kW/m²):
        // absorbed = (0.25-0.21) × poa = 40 W total glass absorption;
        // E+ fraction ≈ 0.5+ → ~20+ W inward; ticket fraction 0.196 → ~7.8 W inward.
        assert!(
            (absorbed_inward_ep - absorbed_inward_ticket).abs() > 5.0,
            "E+ Step-5 absorbed_inward ({absorbed_inward_ep:.2} W) should differ substantially \
             from ticket N_i estimate ({absorbed_inward_ticket:.2} W)"
        );
        // Guard: E+ value must be larger (more heat flows inward per E+ model).
        assert!(
            absorbed_inward_ep > absorbed_inward_ticket,
            "E+ Step-5 inward fraction ({radiation_frac:.3}) should exceed \
             NFRC simple ratio ({ticket_n_i:.3})"
        );
    }
}
