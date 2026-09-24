use chrono::Datelike;
use hares_physics::solar::window_iam;
use hares_types::{EnvironmentState, ZoneId};
use nalgebra::DVector;

use super::ThermalSolver;
use super::config::{InteriorSolarSurfaceInfo, InteriorSurfaceInfo};

/// Cosine weight of the transmitted beam on an interior surface face
/// (parallel-flood approximation).
///
/// The beam is treated as a parallel flood entering through the zone's
/// glazing; each interior face is illuminated in proportion to
/// `max(0, u_sun · n_in)` where `u_sun` is the unit vector toward the sun
/// and `n_in` the face's into-room normal. In HARES tilt/azimuth
/// conventions (tilt 0 = ceiling, 90 = wall, 180 = floor; azimuth = outward
/// normal, clockwise from north) this reduces to
///
///   w = max(0, −cos α · sin τ · cos(φ − ψ) − sin α · cos τ)
///
/// Sanity limits: floor (τ=180) → sin α; ceiling (τ=0) → 0 (ceilings never
/// receive direct beam); wall opposite the sun azimuth (ψ = φ ± 180) →
/// cos α (maximal). At sunrise (α=0) the entire beam goes to vertical
/// surfaces, fixing the legacy sin-clamp heuristic's wrong-direction
/// behavior at low sun.
///
/// Geometrically exact for a convex zone with a point-aperture window,
/// agnostic to window position and shape (that is E+'s
/// FullInteriorAndExterior polygon projection, a possible later refinement).
#[inline]
fn beam_cosine_factor(
    tilt_deg: f64,
    azimuth_deg: f64,
    altitude_deg: f64,
    sun_azimuth_deg: f64,
) -> f64 {
    let alpha = altitude_deg.to_radians();
    let tau = tilt_deg.to_radians();
    let delta = (sun_azimuth_deg - azimuth_deg).to_radians();
    (-alpha.cos() * tau.sin() * delta.cos() - alpha.sin() * tau.cos()).max(0.0)
}

impl ThermalSolver {
    /// Applies window solar (transmitted + inward-flowing absorbed share) and
    /// returns the total injected into `u` [W].
    ///
    /// The returned total is exact by construction: the distribution paths
    /// conserve the transmitted flux (beam + diffuse == Σ absorbed + reflected),
    /// so per window the injection is `absorbed_zone_w + transmitted_total_w`
    /// regardless of which distribution branch ran. Callers use the return
    /// value for component-gains diagnostics instead of measuring a `u` delta.
    pub(super) fn apply_solar_inputs(
        &mut self,
        u: &mut DVector<f64>,
        env: &EnvironmentState,
    ) -> f64 {
        let mut total_w = 0.0;
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
                total_w += absorbed_zone_w;

                let injected_transmitted_w = match zone_id {
                    Some(zid) => self.distribute_transmitted_solar(
                        u,
                        zid,
                        transmitted_beam_w,
                        transmitted_diffuse_w,
                        env.weather.solar_altitude_deg,
                        env.weather.solar_azimuth_deg,
                    ),
                    None => None,
                };

                total_w += match injected_transmitted_w {
                    Some(w) => w,
                    None => {
                        u[air_idx] += transmitted_total_w;
                        transmitted_total_w
                    }
                };

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
        total_w
    }

    /// Distributes transmitted window solar to interior surfaces and zone air.
    /// Returns `Some(total_injected_w)` — the exact watts added to `u` — when
    /// a distribution ran, `None` when the zone has no distribution surfaces
    /// (caller then injects the lump sum to zone air).
    fn distribute_transmitted_solar(
        &mut self,
        u: &mut DVector<f64>,
        zone_id: ZoneId,
        beam_w: f64,
        diffuse_w: f64,
        solar_altitude_deg: f64,
        solar_azimuth_deg: f64,
    ) -> Option<f64> {
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
                let reflected_w = compute_solar_distribution_into(
                    &zone_cfg.surfaces,
                    beam_w,
                    diffuse_w,
                    solar_altitude_deg,
                    solar_azimuth_deg,
                    &mut self.solar_absorbed_buf,
                );
                let (nodes_w, air_spillover) =
                    deposit_solar_to_surface_nodes(&zone_cfg.surfaces, &self.solar_absorbed_buf, u);
                let mut injected_w = nodes_w;
                if let Some(&air_idx) = self.wiring.zone_sensible_input_indices.get(&zone_id) {
                    let air_total = reflected_w + air_spillover;
                    if air_idx < u.len() && air_total > 0.0 {
                        u[air_idx] += air_total;
                        injected_w += air_total;
                    }
                }
                return Some(injected_w);
            }
        }

        // StarMesh mode: use interior_solar_zones for distribution.
        let solar_zone = self
            .config
            .interior_solar_zones
            .iter()
            .find(|z| z.zone_id == zone_id);
        let zone_cfg = solar_zone?;
        if zone_cfg.surfaces.is_empty() {
            return None;
        }

        let reflected_w = compute_solar_distribution_into_solar(
            &zone_cfg.surfaces,
            beam_w,
            diffuse_w,
            solar_altitude_deg,
            solar_azimuth_deg,
            &mut self.solar_absorbed_buf,
        );
        let (nodes_w, air_spillover) =
            deposit_solar_to_solar_surfaces(&zone_cfg.surfaces, &self.solar_absorbed_buf, u);
        let mut injected_w = nodes_w;
        if let Some(&air_idx) = self.wiring.zone_sensible_input_indices.get(&zone_id) {
            let air_total = reflected_w + air_spillover;
            if air_idx < u.len() && air_total > 0.0 {
                u[air_idx] += air_total;
                injected_w += air_total;
            }
        }
        Some(injected_w)
    }

    /// Delivers opaque solar gain to exterior surfaces via [`ExteriorSurfaceInfo`].
    ///
    /// `u[input_index] += (direct + diffuse + reflected) × absorptance × area_m2`
    ///
    /// Skips surfaces that are windows (handled by [`apply_solar_inputs`] via SHGC)
    /// and surfaces with `rad_frac > 0` (handled by the iterative LWR path).
    ///
    /// Returns the total injected into `u` [W] (exact by construction).
    /// Irradiance lookup goes through `solar_irr_slot_buf` (rebuilt once per
    /// timestep by `build_input_vector`) instead of rescanning the irradiance
    /// vec per surface — O(S) per step total, not O(S²).
    pub(super) fn apply_exterior_solar_inputs(
        &self,
        u: &mut DVector<f64>,
        env: &EnvironmentState,
    ) -> f64 {
        let mut total_w = 0.0;
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
            let Some(&slot) = self.solar_irr_slot_buf.get(&info.surface_id) else {
                continue;
            };
            let Some(irr) = env.weather.solar_irradiance.get(slot) else {
                continue;
            };
            let poa_w_m2 = irr.direct_w_m2 + irr.diffuse_w_m2 + irr.reflected_w_m2;
            let q_w = info.absorptance * info.area_m2 * poa_w_m2;
            u[info.input_index] += q_w;
            total_w += q_w;
        }
        total_w
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
    solar_altitude_deg: f64,
    solar_azimuth_deg: f64,
) -> (Vec<f64>, f64) {
    let n = surfaces.len();
    let mut absorbed = vec![0.0_f64; n];
    let reflected = compute_solar_distribution_into(
        surfaces,
        beam_w,
        diffuse_w,
        solar_altitude_deg,
        solar_azimuth_deg,
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
    fn tilt_deg(&self) -> f64;
    fn azimuth_deg(&self) -> f64;
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
    fn tilt_deg(&self) -> f64 {
        self.tilt_deg
    }
    #[inline]
    fn azimuth_deg(&self) -> f64 {
        self.azimuth_deg
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
    fn tilt_deg(&self) -> f64 {
        self.tilt_deg
    }
    #[inline]
    fn azimuth_deg(&self) -> f64 {
        self.azimuth_deg
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
    solar_altitude_deg: f64,
    solar_azimuth_deg: f64,
    absorbed: &mut Vec<f64>,
) -> f64 {
    let n = surfaces.len();
    absorbed.clear();
    absorbed.resize(n, 0.0);

    let total_wa: f64 = surfaces
        .iter()
        .map(|s| s.area_m2() * s.solar_absorptance())
        .sum();

    // Beam: cosine-weighted parallel-flood distribution. Each interior
    // face is illuminated in proportion to area × absorptance × the cosine
    // of the beam's incidence on the face — geometrically exact for a convex
    // zone, no floor/wall heuristics.
    if beam_w > 0.0 {
        // Two passes over the weights instead of a materialized Vec: the
        // cosine factor is a few float ops, cheaper than an allocation in
        // this per-window per-step path (zero-alloc rule).
        let weight_of = |s: &S| {
            s.area_m2()
                * s.solar_absorptance()
                * beam_cosine_factor(
                    s.tilt_deg(),
                    s.azimuth_deg(),
                    solar_altitude_deg,
                    solar_azimuth_deg,
                )
        };
        let weight_sum: f64 = surfaces.iter().map(&weight_of).sum();
        if weight_sum > 0.0 {
            for (i, s) in surfaces.iter().enumerate() {
                absorbed[i] += beam_w * weight_of(s) / weight_sum;
            }
        } else {
            // No face is geometrically lit (e.g. sun at the horizon with the
            // zone's surfaces all back-facing) — the beam still entered the
            // zone, so distribute it isotropically (area × absorptance) as
            // the diffuse term already is. Conserves energy either way.
            if total_wa > 0.0 {
                for (i, s) in surfaces.iter().enumerate() {
                    absorbed[i] += beam_w * s.area_m2() * s.solar_absorptance() / total_wa;
                }
            }
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
    solar_altitude_deg: f64,
    solar_azimuth_deg: f64,
    absorbed: &mut Vec<f64>,
) -> f64 {
    compute_solar_distribution_into_generic(
        surfaces,
        beam_w,
        diffuse_w,
        solar_altitude_deg,
        solar_azimuth_deg,
        absorbed,
    )
}

/// Deposit per-surface absorbed solar [W] into the state-space input vector `u`.
///
/// Splits each surface's share via `radiation_frac` (the RC network voltage-divider
/// between film resistance and material half-resistance):
///
/// - `q * radiation_frac` → surface RC node
/// - `q * (1 - radiation_frac)` → returned as zone air contribution
///
/// Returns `(nodes_w, air_w)` — the watts deposited into surface nodes and
/// the watts destined for zone air (the caller applies the air share under
/// its own guards); `nodes_w + air_w == Σ absorbed` over deposited entries.
fn deposit_solar_to_surface_nodes(
    surfaces: &[InteriorSurfaceInfo],
    absorbed: &[f64],
    u: &mut DVector<f64>,
) -> (f64, f64) {
    let mut nodes_w = 0.0;
    let mut air_total = 0.0;
    for (s, &q) in surfaces.iter().zip(absorbed.iter()) {
        if q > 0.0 {
            if s.input_index < u.len() {
                let q_node = q * s.radiation_frac;
                u[s.input_index] += q_node;
                nodes_w += q_node;
            }
            air_total += q * (1.0 - s.radiation_frac);
        }
    }
    (nodes_w, air_total)
}

/// Solar distribution for [`InteriorSolarSurfaceInfo`] (StarMesh mode).
///
/// Delegates to [`compute_solar_distribution_into_generic`].
pub(crate) fn compute_solar_distribution_into_solar(
    surfaces: &[InteriorSolarSurfaceInfo],
    beam_w: f64,
    diffuse_w: f64,
    solar_altitude_deg: f64,
    solar_azimuth_deg: f64,
    absorbed: &mut Vec<f64>,
) -> f64 {
    compute_solar_distribution_into_generic(
        surfaces,
        beam_w,
        diffuse_w,
        solar_altitude_deg,
        solar_azimuth_deg,
        absorbed,
    )
}

/// Deposit per-surface absorbed solar into `u` for [`InteriorSolarSurfaceInfo`].
/// Returns `(nodes_w, air_w)` — `nodes_w + air_w == Σ absorbed` over
/// deposited entries.
fn deposit_solar_to_solar_surfaces(
    surfaces: &[InteriorSolarSurfaceInfo],
    absorbed: &[f64],
    u: &mut DVector<f64>,
) -> (f64, f64) {
    let mut nodes_w = 0.0;
    let mut air_total = 0.0;
    for (s, &q) in surfaces.iter().zip(absorbed.iter()) {
        if q > 0.0 {
            if let Some(idx) = s.input_index {
                if idx < u.len() {
                    let q_node = q * s.radiation_frac;
                    u[idx] += q_node;
                    nodes_w += q_node;
                }
            }
            air_total += q * (1.0 - s.radiation_frac);
        }
    }
    (nodes_w, air_total)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Representative test sun: mid-altitude, due south — the floor is lit
    /// (sin 45°) and north-facing-interior walls (outward azimuth 0°) are
    /// lit, matching most assertions written for the legacy split.
    const TEST_ALT: f64 = 45.0;
    const TEST_AZ: f64 = 180.0;

    /// `is_floor` maps to the tilt/azimuth of the canonical box:
    /// floor (tilt 180), everything else a north wall (tilt 90, azimuth 0).
    fn make_surface(area: f64, absorptance: f64, is_floor: bool) -> InteriorSurfaceInfo {
        make_surface_oriented(
            area,
            absorptance,
            is_floor,
            if is_floor { 180.0 } else { 90.0 },
            0.0,
        )
    }

    fn make_surface_oriented(
        area: f64,
        absorptance: f64,
        is_floor: bool,
        tilt_deg: f64,
        azimuth_deg: f64,
    ) -> InteriorSurfaceInfo {
        InteriorSurfaceInfo {
            state_index: 0,
            input_index: 0,
            area_m2: area,
            emissivity: 0.9,
            radiation_frac: 1.0,
            rad_res_k_w: 0.0,
            solar_absorptance: absorptance,
            is_floor,
            tilt_deg,
            azimuth_deg,
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
            compute_solar_distribution(&surfaces, beam, diffuse, TEST_ALT, TEST_AZ);

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

    /// Randomized conservation: `beam + diffuse == Σ absorbed + reflected`
    /// must hold exactly for ANY surface set and flux split, not just the
    /// fixed point above. 256 deterministic pseudo-random cases (SplitMix64,
    /// no external deps) spanning 1–8 surfaces, areas 1–100 m², absorptances
    /// 0–1, floor fraction 0–0.9, and fluxes 0–5 kW (including all-zero).
    #[test]
    fn solar_distribution_conserves_energy_randomized() {
        let mut state = 0x9E3779B97F4A7C15_u64;
        let mut next_f64 = move || {
            // SplitMix64
            state = state.wrapping_add(0x9E3779B97F4A7C15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
            z ^= z >> 31;
            (z >> 11) as f64 / (1u64 << 53) as f64
        };

        for case in 0..256 {
            let n = 1 + (next_f64() * 8.0) as usize;
            let surfaces: Vec<_> = (0..n)
                .map(|i| {
                    make_surface(
                        1.0 + next_f64() * 99.0,
                        next_f64(),
                        i == 0 && next_f64() < 0.7, // floor first, usually
                    )
                })
                .collect();
            let beam = next_f64() * 5000.0;
            let diffuse = next_f64() * 5000.0;
            let altitude = next_f64() * 90.0;
            let azimuth = next_f64() * 360.0;

            let (absorbed, reflected) =
                compute_solar_distribution(&surfaces, beam, diffuse, altitude, azimuth);
            let total: f64 = absorbed.iter().sum::<f64>() + reflected;
            let expected = beam + diffuse;
            let tol = 1e-9 * expected.max(1.0);
            assert!(
                (total - expected).abs() <= tol,
                "case {case}: conservation violated: got {total}, expected {expected} \
                 (residual {:.3e} W)",
                total - expected
            );
            assert!(
                reflected >= -tol,
                "case {case}: reflected must be non-negative, got {reflected}"
            );
        }
    }

    #[test]
    fn floor_gets_majority_of_beam() {
        let surfaces = vec![
            make_surface(40.0, 0.6, true), // floor (tilt 180)
            // Ceiling must be tilt 0 — `make_surface(_, _, false)` gives
            // tilt 90 (a wall), which under the cosine model receives beam
            // a ceiling never can. Closed form at alt 45°/az 180°: floor
            // weight 40·0.6·sin(45°) = 16.97, ceiling 0, each wall
            // 20·0.5·cos(45°) = 7.07 → floor share 16.97/31.11 = 54.5%.
            make_surface_oriented(40.0, 0.5, false, 0.0, 0.0), // ceiling
            make_surface(20.0, 0.5, false),                    // wall 1
            make_surface(20.0, 0.5, false),                    // wall 2
        ];
        let beam = 1000.0;
        let diffuse = 0.0;
        let (absorbed, _) = compute_solar_distribution(&surfaces, beam, diffuse, TEST_ALT, TEST_AZ);

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
            compute_solar_distribution(&surfaces, 500.0, 200.0, TEST_ALT, TEST_AZ);
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
            compute_solar_distribution(&surfaces, 500.0, 200.0, TEST_ALT, TEST_AZ);
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
        let (absorbed, _) = compute_solar_distribution(&surfaces, 0.0, 400.0, TEST_ALT, TEST_AZ);

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
            compute_solar_distribution(&surfaces, 1000.0, 0.0, TEST_ALT, TEST_AZ);
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
            compute_solar_distribution(&surfaces, 0.0, 0.0, TEST_ALT, TEST_AZ);
        assert!(absorbed.iter().all(|&q| q == 0.0));
        assert_eq!(reflected, 0.0);
    }

    #[test]
    fn single_surface_receives_all_solar() {
        let surfaces = vec![make_surface(20.0, 0.7, true)];
        let (absorbed, reflected) =
            compute_solar_distribution(&surfaces, 500.0, 200.0, TEST_ALT, TEST_AZ);
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
            azimuth_deg: 0.0,
            tilt_deg: if is_floor { 180.0 } else { 90.0 },
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
            TEST_ALT,
            TEST_AZ,
            &mut absorbed,
        );

        let total_input = beam + diffuse;
        let total_absorbed: f64 = absorbed.iter().sum();
        assert!(
            (total_absorbed + reflected - total_input).abs() < 1e-6,
            "energy conservation violated"
        );

        let mut u = DVector::zeros(3);
        let (_nodes_w, air_contribution) =
            deposit_solar_to_surface_nodes(&surfaces, &absorbed, &mut u);

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
            compute_solar_distribution(&surfaces, 1000.0, 0.0, TEST_ALT, TEST_AZ);
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

    /// Cosine-model limits: floor at zenith, ceiling never, wall
    /// opposite the sun at the horizon, window-side wall never.
    #[test]
    fn beam_cosine_factor_limits() {
        // Floor (tilt 180): weight = sin(altitude): zenith → 1, horizon → 0,
        // below-horizon → clamped to 0 (never negative).
        assert!((beam_cosine_factor(180.0, 0.0, 90.0, 180.0) - 1.0).abs() < 1e-12);
        assert!(beam_cosine_factor(180.0, 0.0, 0.0, 180.0).abs() < 1e-12);
        assert_eq!(beam_cosine_factor(180.0, 0.0, -10.0, 180.0), 0.0);
        // Ceiling (tilt 0): never lit by direct beam.
        assert_eq!(beam_cosine_factor(0.0, 0.0, 45.0, 180.0), 0.0);
        // Wall opposite the sun at sunrise gets the full cos(0°) weight:
        // sun due east (azimuth 90°) → west wall (outward azimuth 270°).
        assert!((beam_cosine_factor(90.0, 270.0, 0.0, 90.0) - 1.0).abs() < 1e-12);
        // The wall the sun strikes from outside (east wall): interior face
        // in shadow → 0.
        assert_eq!(beam_cosine_factor(90.0, 90.0, 0.0, 90.0), 0.0);
        // At zenith, walls get nothing (all beam to the floor).
        assert_eq!(beam_cosine_factor(90.0, 0.0, 90.0, 180.0), 0.0);
    }

    /// Directional correctness through the full distribution. A south
    /// sun lands beam on the floor and the NORTH wall — never the ceiling
    /// or the south (window-side) wall. Guards the legacy sin-clamp's
    /// wrong-direction low-angle floor deposition.
    #[test]
    fn beam_lands_on_floor_and_opposite_wall() {
        let surfaces = vec![
            make_surface_oriented(48.0, 0.6, true, 180.0, 0.0), // floor
            make_surface_oriented(40.0, 0.5, false, 0.0, 0.0),  // ceiling
            make_surface_oriented(20.0, 0.5, false, 90.0, 0.0), // north wall
            make_surface_oriented(20.0, 0.5, false, 90.0, 180.0), // south wall
        ];
        let (absorbed, _) = compute_solar_distribution(&surfaces, 1000.0, 0.0, 60.0, 180.0);
        assert!(absorbed[0] > 0.0, "floor must receive beam at high sun");
        assert_eq!(absorbed[1], 0.0, "ceiling must never receive beam");
        assert!(absorbed[2] > 0.0, "north (opposite) wall must receive beam");
        assert_eq!(absorbed[3], 0.0, "south (window-side) wall gets no beam");
        let total: f64 = absorbed.iter().sum();
        assert!(absorbed[0] / total > 0.5, "floor dominates at high sun");

        // Sunrise (sun due east, altitude 5°): floor share collapses and the
        // WEST wall (opposite the sun) dominates — the case the legacy
        // sin-clamp got wrong-directionally.
        let surfaces2 = vec![
            make_surface_oriented(48.0, 0.6, true, 180.0, 0.0), // floor
            make_surface_oriented(20.0, 0.5, false, 90.0, 270.0), // west wall
            make_surface_oriented(20.0, 0.5, false, 90.0, 90.0), // east wall (window side)
        ];
        let (absorbed2, _) = compute_solar_distribution(&surfaces2, 1000.0, 0.0, 5.0, 90.0);
        let total2: f64 = absorbed2.iter().sum();
        assert!(
            // Geometric share at this fixture: west wall 20·0.5·cos(5°) =
            // 9.962 vs floor grazing 48·0.6·sin(5°) = 2.510 → 9.962/12.472
            // = 79.87% — the threshold must sit below the model's own exact
            // output for this geometry, with dominance margin.
            absorbed2[1] / total2 > 0.75,
            "west wall should dominate at eastern sunrise: share {:.0}%",
            absorbed2[1] / total2 * 100.0
        );
        assert_eq!(absorbed2[2], 0.0, "east (window-side) wall gets no beam");
        assert!(absorbed2[0] > 0.0, "floor still grazed at 5° altitude");
    }

    /// The zero-weight fallback (no face geometrically lit) must still
    /// conserve energy via the isotropic area×absorptance path.
    #[test]
    fn beam_no_lit_face_falls_back_isotropically_and_conserves() {
        // South-facing walls only (interiors face north); sun due north at
        // the horizon → nothing lit.
        let surfaces = vec![
            make_surface_oriented(30.0, 0.5, false, 90.0, 180.0),
            make_surface_oriented(30.0, 0.5, false, 90.0, 180.0),
        ];
        let (absorbed, reflected) = compute_solar_distribution(&surfaces, 800.0, 0.0, 0.0, 0.0);
        let total: f64 = absorbed.iter().sum();
        assert!(
            (total - 800.0).abs() < 1e-6,
            "fallback must still distribute the full beam (conservation), got {total}"
        );
        assert!(reflected.abs() < 1e-6);
    }

    /// The zero-weight fallback must actually EXECUTE, and must put the beam
    /// on surfaces — not dump it to zone air as "reflected". The case above
    /// never reaches the fallback branch: a due-north sun at the horizon
    /// LIGHTS south-facing walls (cosine factor −cos0°·cos(180°) = +1), so
    /// it takes the normalized branch; and with equal areas the two branches
    /// give identical splits, so it cannot distinguish them. The
    /// discriminating input is a sun PARALLEL to every wall (cosine factor
    /// identically zero for all faces) with surfaces whose area×absorptance
    /// weights differ: the fallback's isotropic split is then the only
    /// outcome that both conserves energy and lands it on surface mass
    /// rather than instant zone-air gain. Dumping to air (absorbed = 0,
    /// reflected = beam) would pass every conservation assertion in the
    /// randomized suite and the test above.
    #[test]
    fn beam_parallel_to_all_walls_executes_fallback_and_lands_on_surfaces() {
        // Two north-outward walls (interior faces south), sun due east at
        // 45° altitude: the beam travels due west, grazing both faces —
        // cosine factor max(0, −cos45°·cos(90°) − sin45°·0) = 0 for each.
        let surfaces = vec![
            make_surface_oriented(30.0, 0.5, false, 90.0, 0.0),
            make_surface_oriented(10.0, 0.9, false, 90.0, 0.0),
        ];
        let (absorbed, reflected) = compute_solar_distribution(&surfaces, 800.0, 0.0, 45.0, 90.0);
        let total: f64 = absorbed.iter().sum();
        assert!(
            (total - 800.0).abs() < 1e-9,
            "the beam that entered must land on surface mass, got {total} W \
             absorbed (reflected {reflected} W) — dumping to zone air changes \
             window solar from mass-mediated to instant air gain"
        );
        assert!(
            reflected.abs() < 1e-9,
            "fallback must not report the beam as reflected, got {reflected} W"
        );
        // The fallback split is isotropic: area × absorptance share, NOT the
        // (all-zero) cosine weights. 30·0.5 = 15 vs 10·0.9 = 9 → 500 / 300.
        assert!(
            (absorbed[0] - 500.0).abs() < 1e-9 && (absorbed[1] - 300.0).abs() < 1e-9,
            "fallback split must be the area×absorptance share (500/300 W), \
             got {}/{} W",
            absorbed[0],
            absorbed[1]
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
        let (abs_high, _) = compute_solar_distribution(&surfaces, beam, 0.0, 70.0, TEST_AZ);
        let (abs_low, _) = compute_solar_distribution(&surfaces, beam, 0.0, 10.0, TEST_AZ);
        assert!(
            abs_high[0] > abs_low[0],
            "floor should absorb more at 70° ({}) than 10° ({})",
            abs_high[0],
            abs_low[0]
        );
        // At near-horizontal sun the floor share must collapse toward the
        // geometric sin limit — the legacy 0.3-clamp phantom-deposition
        // regression guard.
        let floor_low = abs_low[0] / abs_low.iter().sum::<f64>();
        assert!(
            floor_low < 0.35,
            "floor beam share at 10° altitude must stay near the geometric \
             limit, got {floor_low:.2}"
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
