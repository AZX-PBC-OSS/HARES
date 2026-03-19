//! Longwave (thermal infrared) radiation exchange for building envelope surfaces.
//!
//! # Exterior longwave radiation
//!
//! Each exterior surface exchanges radiation with two sources:
//!   - The sky hemisphere, weighted by the sky view factor F_sky
//!   - The ground/surroundings hemisphere, weighted by (1 - F_sky)
//!
//! The net flux onto the surface is:
//!   Q_lw = ε × σ × A × [ F_sky × (T_sky⁴ - T_surf⁴)
//!                        + (1 - F_sky) × (T_ground⁴ - T_surf⁴) ]
//!
//! Sky view factor (EnergyPlus Engineering Reference, §External Longwave Radiation):
//!   F_sky = ((1 + cos β) / 2)^1.5    where β is the surface tilt from horizontal
//!   - Horizontal roof: β = 0°  → F_sky = 1.0
//!   - Vertical wall:   β = 90° → F_sky ≈ 0.354
//!
//! # Interior longwave radiation
//!
//! Within a zone, interior surfaces exchange radiation via area-weighted view
//! factors. For an N-surface zone the view-factor-weighted mean radiant
//! temperature T_mrt is computed, and each surface exchanging a net heat flux:
//!   Q_rad,i = h_r,i × A_i × (T_mrt - T_surf,i)
//!
//! where the linearised radiative heat transfer coefficient is:
//!   h_r = 4 × ε × σ × T_avg³    (W/(m²·K))
//!
//! This matches OCHRE's interior radiation model (area-weighted view factors,
//! iterative surface temperature update).
//!
//! # References
//! - EnergyPlus Engineering Reference 9.6: "External Longwave Radiation"
//! - ASHRAE HOF 2021, Ch. 25: "Heat, Air, and Moisture Control in Building Assemblies"
//! - OCHRE Envelope.py: `_solve_exterior_radiation`, `_solve_interior_radiation`

/// Stefan-Boltzmann constant [W/(m²·K⁴)].
/// NIST CODATA 2018: σ = 5.670374419 × 10⁻⁸ W·m⁻²·K⁻⁴.
pub const STEFAN_BOLTZMANN: f64 = 5.670_374_419e-8;

/// Celsius-to-Kelvin offset.
const CELSIUS_TO_KELVIN: f64 = 273.15;

/// Default emissivity for opaque building surfaces (walls, roof, floor).
/// EnergyPlus default; typical range 0.85–0.95.
pub const EMISSIVITY_DEFAULT: f64 = 0.90;

/// Emissivity for window glass (interior and exterior LWR).
/// Per OCHRE and EnergyPlus window module.
pub const EMISSIVITY_WINDOW: f64 = 0.84;

/// Emissivity for attic radiant barriers.
///
/// OCHRE overrides the default 0.90 emissivity to 0.05 for surfaces in the
/// attic zone when a radiant barrier is present. This matches reflective foil
/// products (aluminium facing), which have measured emissivities of 0.03–0.07.
pub const EMISSIVITY_RADIANT_BARRIER: f64 = 0.05;

// ─────────────────────────────────────────────────────────────────────────────
// Sky view factor
// ─────────────────────────────────────────────────────────────────────────────

/// Sky view factor for a surface tilted at `tilt_deg` from horizontal.
///
/// Uses the EnergyPlus Engineering Reference formula:
///   F_sky = ((1 + cos β) / 2)^1.5
///
/// # Arguments
/// * `tilt_deg` — surface tilt from horizontal [°]; 0 = horizontal roof, 90 = vertical wall
///
/// # Returns
/// Sky view factor in `[0, 1]`.
///
/// # Examples
/// ```
/// use hares_envelope::longwave_radiation::sky_view_factor;
/// assert!((sky_view_factor(0.0) - 1.0).abs() < 1e-9);    // horizontal roof
/// assert!((sky_view_factor(90.0) - 0.35355339).abs() < 1e-8); // vertical wall
/// ```
#[must_use]
pub fn sky_view_factor(tilt_deg: f64) -> f64 {
    let cos_beta = tilt_deg.to_radians().cos();
    ((1.0 + cos_beta) / 2.0).powf(1.5)
}

// ─────────────────────────────────────────────────────────────────────────────
// Exterior longwave radiation
// ─────────────────────────────────────────────────────────────────────────────

/// Parameters for a single exterior surface.
#[derive(Debug, Clone, Copy)]
pub struct ExteriorSurface {
    /// Surface area [m²].
    pub area_m2: f64,
    /// Longwave emissivity [-], typically 0.90 for opaque, 0.84 for glass.
    pub emissivity: f64,
    /// Sky view factor [-]; use [`sky_view_factor`] or supply directly.
    pub sky_view_factor: f64,
}

/// Net longwave radiation flux onto one exterior surface [W].
///
/// Positive = net heat gain onto the surface.
///
/// The formula is:
///   Q = ε·σ·A·[ F_sky·(T_sky⁴ − T_surf⁴) + (1 − F_sky)·(T_gnd⁴ − T_surf⁴) ]
///
/// # Arguments
/// * `surface`       — surface geometry and optical properties
/// * `t_sky_c`       — effective sky temperature [°C]; if NaN (e.g. missing
///   EPW data), falls back to `t_ground_c` (matching OCHRE's behavior)
/// * `t_ground_c`    — ground/surroundings temperature [°C]
/// * `t_surface_c`   — actual exterior surface temperature [°C].  The caller
///   is responsible for converting RC-network node temperatures to true
///   surface temperatures by accounting for any film resistance; passing a
///   node temperature directly will introduce systematic error.
///
/// # Returns
/// Net longwave heat flux onto the surface [W]. Negative means net emission.
#[must_use]
pub fn exterior_longwave_w(
    surface: &ExteriorSurface,
    t_sky_c: f64,
    t_ground_c: f64,
    t_surface_c: f64,
) -> f64 {
    // Fall back to ground temperature when sky temperature is unavailable.
    let t_sky_effective = if t_sky_c.is_nan() {
        t_ground_c
    } else {
        t_sky_c
    };
    let e_factor = surface.emissivity * STEFAN_BOLTZMANN * surface.area_m2;
    let t_sky_k4 = (t_sky_effective + CELSIUS_TO_KELVIN).powi(4);
    let t_gnd_k4 = (t_ground_c + CELSIUS_TO_KELVIN).powi(4);
    let t_surf_k4 = (t_surface_c + CELSIUS_TO_KELVIN).powi(4);
    let svf = surface.sky_view_factor;
    e_factor * (svf * (t_sky_k4 - t_surf_k4) + (1.0 - svf) * (t_gnd_k4 - t_surf_k4))
}

/// Net longwave radiation flux density onto one exterior surface [W/m²].
///
/// Same as [`exterior_longwave_w`] but per unit area (independent of area_m2
/// field in `surface`).  NaN handling for `t_sky_c` and the surface
/// temperature contract are identical to [`exterior_longwave_w`].
#[must_use]
pub fn exterior_longwave_w_m2(
    surface: &ExteriorSurface,
    t_sky_c: f64,
    t_ground_c: f64,
    t_surface_c: f64,
) -> f64 {
    if surface.area_m2 > 0.0 {
        exterior_longwave_w(surface, t_sky_c, t_ground_c, t_surface_c) / surface.area_m2
    } else {
        0.0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Interior longwave radiation
// ─────────────────────────────────────────────────────────────────────────────

/// Parameters for a single interior surface for zone radiation exchange.
#[derive(Debug, Clone, Copy)]
pub struct InteriorSurface {
    /// Surface area [m²].
    pub area_m2: f64,
    /// Longwave emissivity [-].
    pub emissivity: f64,
}

/// Linearised radiative heat transfer coefficient [W/(m²·K)].
///
/// h_r = 4 × ε × σ × T_avg³
///
/// where `t_avg_c` is a representative mean temperature of the participating
/// surfaces, usually close to the zone air temperature.
///
/// # Arguments
/// * `emissivity` — surface emissivity [-]
/// * `t_avg_c`    — representative average temperature [°C]
#[must_use]
pub fn linearised_h_r(emissivity: f64, t_avg_c: f64) -> f64 {
    4.0 * emissivity * STEFAN_BOLTZMANN * (t_avg_c + CELSIUS_TO_KELVIN).powi(3)
}

/// Net longwave radiation exchange for each interior surface in a zone [W].
///
/// Returns a `Vec<f64>` of net fluxes (positive = net gain, negative = net
/// loss) corresponding 1-to-1 with the input `surfaces` slice.
///
/// This uses the area-and-emissivity weighted mean radiant temperature
/// (MRT) approach with a linearised Stefan-Boltzmann coefficient, which
/// matches OCHRE's interior radiation model.
///
/// The view factor for surface i is:
///   F_i = A_i · ε_i / Σ(A_j · ε_j)
///
/// The MRT seen by surface i is the emissivity-area weighted mean of the
/// outgoing fluxes of all other surfaces (i.e. the total radiosity):
///   J_total = Σ_j (ε_j · σ · A_j · T_j⁴)
///   J_in,i  = J_total · F_i   (view-factor-weighted incoming radiation)
///   Q_net,i = J_in,i - ε_i · σ · A_i · T_surf,i⁴
///
/// # Arguments
/// * `surfaces`      — slice of interior surface parameters
/// * `t_surfaces_c`  — actual surface temperatures [°C], same length as
///   `surfaces`.  The caller must supply true surface temperatures
///   (accounting for any film resistance), not RC-network node temperatures.
///
/// # Returns
/// Net LW heat flux for each surface [W].  The sum is zero by energy conservation.
///
/// # Panics
/// Does not panic; returns all-zeros if `surfaces` is empty or total area is zero.
#[must_use]
pub fn interior_longwave_net_w(surfaces: &[InteriorSurface], t_surfaces_c: &[f64]) -> Vec<f64> {
    debug_assert_eq!(surfaces.len(), t_surfaces_c.len());
    let n = surfaces.len();
    if n == 0 {
        return Vec::new();
    }

    // Total emissivity-area factor (denominator of view factors)
    let total_ea: f64 = surfaces.iter().map(|s| s.area_m2 * s.emissivity).sum();
    if total_ea <= 0.0 {
        return vec![0.0; n];
    }

    // Outgoing radiosity per surface [W] = ε_i · σ · A_i · T_i⁴
    let out_w: Vec<f64> = surfaces
        .iter()
        .zip(t_surfaces_c.iter())
        .map(|(s, &t_c)| {
            s.emissivity * STEFAN_BOLTZMANN * s.area_m2 * (t_c + CELSIUS_TO_KELVIN).powi(4)
        })
        .collect();

    // Total outgoing radiosity [W]
    let total_out: f64 = out_w.iter().sum();

    // Net flux for each surface: incoming (view-factor-weighted total) minus outgoing
    surfaces
        .iter()
        .zip(out_w.iter())
        .map(|(s, &q_out)| {
            let view_factor = s.area_m2 * s.emissivity / total_ea;
            let q_in = total_out * view_factor;
            q_in - q_out
        })
        .collect()
}

/// Linearised net interior longwave radiation for each surface [W].
///
/// This is a faster, linear approximation that avoids T⁴ per timestep.
/// Useful when an analytic linearisation around an operating point is
/// acceptable (e.g. for embedding in the state-space matrix).
///
/// h_r is computed once at the zone air temperature, then:
///   Q_net,i = h_r × A_i × (T_mrt - T_surf,i)
///
/// where T_mrt is the area-weighted mean surface temperature.
///
/// # Arguments
/// * `surfaces`     — interior surface parameters
/// * `t_surfaces_c` — actual surface temperatures [°C].  The caller must
///   supply true surface temperatures (accounting for any film resistance),
///   not RC-network node temperatures.
/// * `t_zone_c`     — zone air temperature used for h_r linearisation [°C]
#[must_use]
pub fn interior_longwave_linearised_w(
    surfaces: &[InteriorSurface],
    t_surfaces_c: &[f64],
    t_zone_c: f64,
) -> Vec<f64> {
    debug_assert_eq!(surfaces.len(), t_surfaces_c.len());
    let n = surfaces.len();
    if n == 0 {
        return Vec::new();
    }

    // Emissivity-area weighted mean radiant temperature [°C].
    // Matches OCHRE's interior radiation model: surfaces with higher ε·A
    // contribute proportionally more to the radiative environment seen by
    // other surfaces.
    let total_ea: f64 = surfaces.iter().map(|s| s.area_m2 * s.emissivity).sum();
    if total_ea <= 0.0 {
        return vec![0.0; n];
    }
    let t_mrt: f64 = surfaces
        .iter()
        .zip(t_surfaces_c.iter())
        .map(|(s, &t)| s.area_m2 * s.emissivity * t)
        .sum::<f64>()
        / total_ea;

    surfaces
        .iter()
        .zip(t_surfaces_c.iter())
        .map(|(s, &t_surf)| {
            let h_r = linearised_h_r(s.emissivity, t_zone_c);
            h_r * s.area_m2 * (t_mrt - t_surf)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─────────────────────────────────────────────────────────────────────────
    // Sky view factor
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn sky_view_factor_horizontal_is_one() {
        let f = sky_view_factor(0.0);
        assert!(
            (f - 1.0).abs() < 1e-9,
            "horizontal roof SVF should be 1.0, got {f}"
        );
    }

    #[test]
    fn sky_view_factor_vertical_matches_energyplus() {
        // EnergyPlus: ((1 + cos(90°)) / 2)^1.5 = (0.5)^1.5 = 0.353553...
        let f = sky_view_factor(90.0);
        let expected = 0.5_f64.powf(1.5);
        assert!(
            (f - expected).abs() < 1e-9,
            "vertical wall SVF: {f}, expected {expected}"
        );
    }

    #[test]
    fn sky_view_factor_45_degree_is_between_wall_and_roof() {
        let f_roof = sky_view_factor(0.0);
        let f_wall = sky_view_factor(90.0);
        let f_45 = sky_view_factor(45.0);
        assert!(
            f_45 > f_wall && f_45 < f_roof,
            "45° SVF {f_45} should be between wall ({f_wall}) and roof ({f_roof})"
        );
    }

    #[test]
    fn sky_view_factor_ranges_in_zero_to_one() {
        for tilt in [0.0_f64, 15.0, 30.0, 45.0, 60.0, 75.0, 90.0] {
            let f = sky_view_factor(tilt);
            assert!(
                (0.0..=1.0).contains(&f),
                "SVF {f} out of [0,1] at tilt={tilt}°"
            );
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Exterior longwave radiation
    // ─────────────────────────────────────────────────────────────────────────

    fn roof_surface() -> ExteriorSurface {
        ExteriorSurface {
            area_m2: 48.0, // BESTEST Case 600 roof area
            emissivity: 0.90,
            sky_view_factor: sky_view_factor(0.0), // horizontal roof
        }
    }

    #[test]
    fn zero_flux_when_sky_equals_surface() {
        let surf = roof_surface();
        let t = 20.0; // same sky, ground, and surface temp
        let q = exterior_longwave_w(&surf, t, t, t);
        assert!(
            q.abs() < 1e-6,
            "zero flux expected when T_sky=T_gnd=T_surf, got {q} W"
        );
    }

    #[test]
    fn cold_sky_produces_net_emission_from_surface() {
        // Clear night: surface at 20 °C, sky at -10 °C → surface emits (negative Q)
        let surf = roof_surface();
        let q = exterior_longwave_w(&surf, -10.0, 10.0, 20.0);
        assert!(q < 0.0, "surface should lose heat to cold sky, got Q={q} W");
    }

    #[test]
    fn warm_sky_produces_net_gain_to_surface() {
        // Uncommon but possible: hot sky temperature
        let surf = roof_surface();
        let q = exterior_longwave_w(&surf, 60.0, 20.0, 20.0);
        assert!(q > 0.0, "hot sky should heat surface, got Q={q} W");
    }

    #[test]
    fn exterior_lw_flux_magnitude_physically_reasonable() {
        // Clear night: surface at 20 °C, sky at -10 °C, ground at 10 °C
        // Expected: ~50–150 W/m² net emission (negative flux)
        let surf = ExteriorSurface {
            area_m2: 1.0,
            emissivity: 0.90,
            sky_view_factor: sky_view_factor(0.0), // horizontal
        };
        let q_m2 = exterior_longwave_w_m2(&surf, -10.0, 10.0, 20.0);
        assert!(
            q_m2 < -50.0 && q_m2 > -200.0,
            "exterior LW flux density {q_m2:.1} W/m² out of expected range -50 to -200 W/m²"
        );
    }

    #[test]
    fn exterior_lw_scales_linearly_with_area() {
        let base = ExteriorSurface {
            area_m2: 10.0,
            emissivity: 0.90,
            sky_view_factor: 0.5,
        };
        let double = ExteriorSurface {
            area_m2: 20.0,
            ..base
        };
        let q_base = exterior_longwave_w(&base, -5.0, 10.0, 20.0);
        let q_double = exterior_longwave_w(&double, -5.0, 10.0, 20.0);
        assert!(
            (q_double - 2.0 * q_base).abs() < 1e-6,
            "LW flux must scale linearly with area: q_base={q_base:.4}, q_double={q_double:.4}"
        );
    }

    #[test]
    fn exterior_lw_scales_linearly_with_emissivity() {
        let base = ExteriorSurface {
            area_m2: 10.0,
            emissivity: 0.45,
            sky_view_factor: 0.5,
        };
        let double_e = ExteriorSurface {
            emissivity: 0.90,
            ..base
        };
        let q_base = exterior_longwave_w(&base, -5.0, 10.0, 20.0);
        let q_double = exterior_longwave_w(&double_e, -5.0, 10.0, 20.0);
        assert!(
            (q_double - 2.0 * q_base).abs() < 1e-6,
            "LW flux must scale linearly with emissivity"
        );
    }

    #[test]
    fn hotter_surface_emits_more_than_cooler_surface() {
        let surf = ExteriorSurface {
            area_m2: 10.0,
            emissivity: 0.90,
            sky_view_factor: sky_view_factor(0.0),
        };
        let t_sky = 0.0;
        let t_gnd = 5.0;
        let q_warm = exterior_longwave_w(&surf, t_sky, t_gnd, 40.0);
        let q_cool = exterior_longwave_w(&surf, t_sky, t_gnd, 20.0);
        // Warm surface emits more → more negative flux (larger magnitude loss)
        assert!(
            q_warm < q_cool,
            "hotter surface must have more negative flux: q_warm={q_warm:.2}, q_cool={q_cool:.2}"
        );
    }

    #[test]
    fn very_cold_sky_produces_large_cooling_flux() {
        // Extreme cold sky: -40 °C (radiative frost condition)
        let surf = ExteriorSurface {
            area_m2: 1.0,
            emissivity: 0.90,
            sky_view_factor: 1.0,
        };
        let q = exterior_longwave_w(&surf, -40.0, 0.0, 20.0);
        // Should be a sizeable cooling flux
        assert!(
            q < -100.0,
            "very cold sky should produce large cooling flux, got {q:.1} W/m²"
        );
    }

    #[test]
    fn roof_loses_more_to_sky_than_wall_at_equal_area() {
        // Roof (SVF=1) exchanges more with sky than vertical wall (SVF≈0.354)
        // with the same cold sky temperature
        let t_sky = -15.0;
        let t_gnd = 10.0;
        let t_surf = 20.0;
        let roof = ExteriorSurface {
            area_m2: 1.0,
            emissivity: 0.90,
            sky_view_factor: sky_view_factor(0.0),
        };
        let wall = ExteriorSurface {
            area_m2: 1.0,
            emissivity: 0.90,
            sky_view_factor: sky_view_factor(90.0),
        };
        let q_roof = exterior_longwave_w(&roof, t_sky, t_gnd, t_surf);
        let q_wall = exterior_longwave_w(&wall, t_sky, t_gnd, t_surf);
        // Roof sees a colder effective sky → more net emission (more negative)
        assert!(
            q_roof < q_wall,
            "roof (SVF=1) should lose more heat than wall (SVF≈0.35): roof={q_roof:.1}, wall={q_wall:.1}"
        );
    }

    #[test]
    fn exterior_lw_clear_night_regression() {
        // Regression: known input → known output
        // Surface: 1 m², ε=0.90, horizontal (SVF=1.0)
        // T_surf=20°C, T_sky=-10°C, T_gnd=10°C
        // Q = 0.90 × 5.670374419e-8 × 1.0 × [1.0×(263.15⁴ - 293.15⁴) + 0.0×(283.15⁴ - 293.15⁴)]
        let surf = ExteriorSurface {
            area_m2: 1.0,
            emissivity: 0.90,
            sky_view_factor: 1.0,
        };
        let q = exterior_longwave_w(&surf, -10.0, 10.0, 20.0);
        let t_sky_k = 263.15_f64;
        let t_surf_k = 293.15_f64;
        let expected = 0.90 * STEFAN_BOLTZMANN * (t_sky_k.powi(4) - t_surf_k.powi(4));
        assert!(
            (q - expected).abs() < 1e-4,
            "regression failed: got {q:.4}, expected {expected:.4}"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Linearised h_r
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn linearised_h_r_at_20c_is_physically_reasonable() {
        // At 20°C: h_r = 4 × 0.90 × σ × 293.15³ ≈ 5.2 W/(m²·K)
        let h = linearised_h_r(0.90, 20.0);
        assert!(
            (4.0..=7.0).contains(&h),
            "h_r at 20°C should be ~5 W/(m²·K), got {h:.3}"
        );
    }

    #[test]
    fn linearised_h_r_increases_with_temperature() {
        let h_cold = linearised_h_r(0.90, -10.0);
        let h_hot = linearised_h_r(0.90, 60.0);
        assert!(
            h_hot > h_cold,
            "h_r should increase with temperature: h_cold={h_cold:.4}, h_hot={h_hot:.4}"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Interior longwave radiation (exact)
    // ─────────────────────────────────────────────────────────────────────────

    fn uniform_zone_surfaces(n: usize, area_m2: f64, emissivity: f64) -> Vec<InteriorSurface> {
        (0..n)
            .map(|_| InteriorSurface {
                area_m2,
                emissivity,
            })
            .collect()
    }

    #[test]
    fn interior_lw_conserves_energy_uniform_zone() {
        // All surfaces at different temps: sum of net fluxes must be zero.
        let surfaces = vec![
            InteriorSurface {
                area_m2: 40.0,
                emissivity: 0.90,
            }, // floor
            InteriorSurface {
                area_m2: 40.0,
                emissivity: 0.90,
            }, // ceiling
            InteriorSurface {
                area_m2: 30.0,
                emissivity: 0.90,
            }, // wall 1
            InteriorSurface {
                area_m2: 30.0,
                emissivity: 0.90,
            }, // wall 2
            InteriorSurface {
                area_m2: 20.0,
                emissivity: 0.90,
            }, // wall 3
            InteriorSurface {
                area_m2: 20.0,
                emissivity: 0.90,
            }, // wall 4
        ];
        let temps = vec![18.0, 22.0, 21.0, 20.0, 19.0, 21.5];
        let net = interior_longwave_net_w(&surfaces, &temps);
        let total: f64 = net.iter().sum();
        assert!(
            total.abs() < 1e-3,
            "interior LW must conserve energy: sum={total:.6} W"
        );
    }

    #[test]
    fn interior_lw_net_zero_when_isothermal() {
        // All surfaces at the same temperature → no net exchange.
        let surfaces = uniform_zone_surfaces(4, 20.0, 0.90);
        let temps = vec![20.0_f64; 4];
        let net = interior_longwave_net_w(&surfaces, &temps);
        for (i, &q) in net.iter().enumerate() {
            assert!(
                q.abs() < 1e-4,
                "isothermal: surface {i} net flux should be 0, got {q:.6}"
            );
        }
    }

    #[test]
    fn interior_lw_hot_surface_loses_heat_to_cold_surfaces() {
        // One hot surface surrounded by cold surfaces: it must emit (negative flux).
        let surfaces = vec![
            InteriorSurface {
                area_m2: 10.0,
                emissivity: 0.90,
            }, // hot
            InteriorSurface {
                area_m2: 90.0,
                emissivity: 0.90,
            }, // cold (combined)
        ];
        let temps = vec![50.0, 20.0];
        let net = interior_longwave_net_w(&surfaces, &temps);
        assert!(
            net[0] < 0.0,
            "hot surface must lose heat (negative net flux), got {:.3}",
            net[0]
        );
        assert!(
            net[1] > 0.0,
            "cold surface must gain heat (positive net flux), got {:.3}",
            net[1]
        );
    }

    #[test]
    fn interior_lw_energy_conservation_asymmetric_areas() {
        // Unequal areas — energy must still balance.
        let surfaces = vec![
            InteriorSurface {
                area_m2: 5.0,
                emissivity: 0.85,
            },
            InteriorSurface {
                area_m2: 25.0,
                emissivity: 0.92,
            },
            InteriorSurface {
                area_m2: 15.0,
                emissivity: 0.88,
            },
        ];
        let temps = vec![30.0, 15.0, 22.0];
        let net = interior_longwave_net_w(&surfaces, &temps);
        let total: f64 = net.iter().sum();
        assert!(
            total.abs() < 1e-4,
            "energy conservation with asymmetric areas: sum={total:.8} W"
        );
    }

    #[test]
    fn interior_lw_returns_empty_for_no_surfaces() {
        let net = interior_longwave_net_w(&[], &[]);
        assert!(net.is_empty());
    }

    #[test]
    fn interior_lw_single_surface_net_zero() {
        // Single surface: it can only "see" itself (view factor = 1).
        // By definition net flux is J_in - J_out = J_out × 1 - J_out = 0.
        let surfaces = vec![InteriorSurface {
            area_m2: 10.0,
            emissivity: 0.90,
        }];
        let temps = vec![25.0];
        let net = interior_longwave_net_w(&surfaces, &temps);
        assert!(
            net[0].abs() < 1e-6,
            "single surface net flux must be zero, got {:.8}",
            net[0]
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Interior longwave radiation (linearised)
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn linearised_interior_lw_hot_surface_loses_heat() {
        let surfaces = vec![
            InteriorSurface {
                area_m2: 10.0,
                emissivity: 0.90,
            }, // hot
            InteriorSurface {
                area_m2: 90.0,
                emissivity: 0.90,
            }, // cold
        ];
        let temps = vec![50.0, 20.0];
        let net = interior_longwave_linearised_w(&surfaces, &temps, 22.0);
        assert!(
            net[0] < 0.0,
            "hot surface should lose heat (linearised): {:.3}",
            net[0]
        );
        assert!(
            net[1] > 0.0,
            "cold surfaces should gain heat (linearised): {:.3}",
            net[1]
        );
    }

    #[test]
    fn linearised_interior_lw_isothermal_is_zero() {
        let surfaces = uniform_zone_surfaces(5, 20.0, 0.90);
        let temps = vec![22.0_f64; 5];
        let net = interior_longwave_linearised_w(&surfaces, &temps, 22.0);
        for (i, &q) in net.iter().enumerate() {
            assert!(
                q.abs() < 1e-10,
                "linearised isothermal: surface {i} net flux should be 0, got {q:.12}"
            );
        }
    }

    #[test]
    fn linearised_interior_lw_conserves_energy() {
        let surfaces = vec![
            InteriorSurface {
                area_m2: 40.0,
                emissivity: 0.90,
            },
            InteriorSurface {
                area_m2: 40.0,
                emissivity: 0.90,
            },
            InteriorSurface {
                area_m2: 60.0,
                emissivity: 0.90,
            },
        ];
        let temps = vec![18.0, 22.0, 21.0];
        let net = interior_longwave_linearised_w(&surfaces, &temps, 21.0);
        let total: f64 = net.iter().sum();
        assert!(
            total.abs() < 1e-8,
            "linearised interior LW must conserve energy: sum={total:.10}"
        );
    }

    #[test]
    fn linearised_interior_lw_conserves_energy_mixed_emissivity() {
        // Surfaces with different emissivities — exercises the ε·A weighted MRT
        // path.  A pure area-weighted MRT produces a non-zero sum, so this test
        // would fail without the HIGH-1 fix.
        let surfaces = vec![
            InteriorSurface {
                area_m2: 40.0,
                emissivity: 0.85,
            }, // floor
            InteriorSurface {
                area_m2: 40.0,
                emissivity: 0.92,
            }, // ceiling
            InteriorSurface {
                area_m2: 60.0,
                emissivity: 0.88,
            }, // combined walls
        ];
        let temps = vec![18.0, 24.0, 21.0];
        let net = interior_longwave_linearised_w(&surfaces, &temps, 21.0);
        let total: f64 = net.iter().sum();
        assert!(
            total.abs() < 1e-8,
            "mixed-ε linearised interior LW must conserve energy: sum={total:.10} W"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Sky view factor — tilt > 90°
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn sky_view_factor_downward_floor_is_zero() {
        // A downward-facing floor (tilt = 180°) faces away from the sky entirely.
        // ((1 + cos 180°) / 2)^1.5 = ((1 − 1) / 2)^1.5 = 0.0
        let f = sky_view_factor(180.0);
        assert!(
            f.abs() < 1e-9,
            "downward-facing floor SVF should be 0.0, got {f}"
        );
    }

    #[test]
    fn sky_view_factor_decreases_monotonically_from_90_to_180() {
        // Past 90° (tilted past vertical, facing downward), SVF must decrease
        // monotonically toward zero.
        let tilts = [90.0_f64, 120.0, 135.0, 150.0, 165.0, 180.0];
        let svfs: Vec<f64> = tilts.iter().map(|&t| sky_view_factor(t)).collect();
        for window in svfs.windows(2) {
            assert!(
                window[0] >= window[1],
                "SVF should decrease from 90° to 180°: got {:.6} then {:.6}",
                window[0],
                window[1]
            );
        }
    }
}
