//! TARP interior + DOE-2 exterior dynamic film coefficient calculations.
//!
//! Interior convection follows the TARP (Thermal Analysis Research Program)
//! model. Exterior forced convection follows the DOE-2 model with a
//! surface-roughness correction factor.
//!
//! All inputs and outputs are in SI units.
//!
//! # References
//! - EnergyPlus Engineering Reference §9.4 (TARP interior convection).
//! - EnergyPlus Engineering Reference §9.5 (DOE-2 exterior convection).
//! - OCHRE reference implementation `calculate_film_resistances`.

/// Zone height ordering for TARP above/below determination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneLabel {
    Ground,      // GND
    Foundation,  // FND
    Conditioned, // LIV
    Garage,      // GAR
    Attic,       // ATC
    Outdoor,     // EXT
}

impl ZoneLabel {
    /// Ordinal height rank (0 = lowest, 5 = highest / outdoor).
    ///
    /// Matches the `zone_order` list in the OCHRE reference implementation.
    pub fn height_order(self) -> u8 {
        match self {
            Self::Ground => 0,
            Self::Foundation => 1,
            Self::Conditioned => 2,
            Self::Garage => 3,
            Self::Attic => 4,
            Self::Outdoor => 5,
        }
    }
}

/// Surface roughness factor for DOE-2 exterior forced convection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceRoughness {
    VeryRough,
    Rough,
    MediumRough,
    MediumSmooth,
    Smooth,
    VerySmooth,
}

impl SurfaceRoughness {
    /// DOE-2 roughness correction factor `r_f`.
    ///
    /// Scales the excess convection coefficient `(h_glass - h_natural)`.
    pub fn factor(self) -> f64 {
        match self {
            Self::VeryRough => 2.17,
            Self::Rough => 1.67,
            Self::MediumRough => 1.52,
            Self::MediumSmooth => 1.13,
            Self::Smooth => 1.11,
            Self::VerySmooth => 1.00,
        }
    }
}

/// Typical zone temperatures [°C] for each [`ZoneLabel`] variant.
///
/// Anchor values: Ground = `avg_ground_c`, Conditioned = 20 °C,
/// Outdoor = `avg_ambient_c + 5`.  Foundation, Garage, and Attic are linearly
/// interpolated between adjacent anchors, matching the behaviour of
/// `pandas.Series.interpolate()` in the OCHRE reference.
///
/// Returns `[ground, foundation, conditioned, garage, attic, outdoor]`.
pub fn typical_zone_temps(avg_ground_c: f64, avg_ambient_c: f64) -> [f64; 6] {
    let t_ground = avg_ground_c;
    let t_conditioned = 20.0_f64;
    let t_outdoor = avg_ambient_c + 5.0;

    // Linear interpolation between the three anchors:
    //   Ground(0) -- Foundation(1) -- Conditioned(2) -- Garage(3) -- Attic(4) -- Outdoor(5)
    let t_foundation = t_ground + (t_conditioned - t_ground) * (1.0 / 2.0);
    let t_garage = t_conditioned + (t_outdoor - t_conditioned) * (1.0 / 3.0);
    let t_attic = t_conditioned + (t_outdoor - t_conditioned) * (2.0 / 3.0);

    [
        t_ground,
        t_foundation,
        t_conditioned,
        t_garage,
        t_attic,
        t_outdoor,
    ]
}

/// TARP natural convection coefficient h [W/(m²·K)].
///
/// - Vertical surface (`tilt_deg == 90`): `h = 1.31 · ΔT^(1/3)`
/// - Non-vertical, warm side above (`above_hotter = true`):
///   `h = 9.482 · ΔT^(1/3) / (7.238 − |cos(tilt)|)` (enhanced)
/// - Non-vertical, warm side below (`above_hotter = false`):
///   `h = 1.810 · ΔT^(1/3) / (1.382 + |cos(tilt)|)` (reduced)
///
/// `delta_t_k` must be non-negative; `tilt_deg` is measured from horizontal.
pub fn tarp_h_natural(tilt_deg: f64, delta_t_k: f64, above_hotter: bool) -> f64 {
    let cbrt_dt = delta_t_k.cbrt();
    if (tilt_deg - 90.0).abs() < 1e-9 {
        1.31 * cbrt_dt
    } else {
        let cos_tilt = tilt_deg.to_radians().cos().abs();
        if above_hotter {
            9.482 * cbrt_dt / (7.238 - cos_tilt)
        } else {
            1.810 * cbrt_dt / (1.382 + cos_tilt)
        }
    }
}

/// Film resistances for a building envelope boundary [m²·K/W].
///
/// Returns `(r_interior, r_exterior)`.
///
/// Interior resistance uses TARP natural convection.  Exterior resistance
/// additionally applies the DOE-2 forced-convection model when
/// `exterior_zone` is [`ZoneLabel::Outdoor`]; otherwise it equals the
/// interior resistance.
///
/// # Parameters
/// - `tilt_deg` -- surface tilt in degrees from horizontal (0 = floor/ceiling,
///   90 = vertical wall).
/// - `interior_zone` / `exterior_zone` -- zones bounding the surface.
/// - `avg_wind_speed_m_s` -- site average wind speed [m/s].
/// - `avg_ground_temp_c` -- annual average ground temperature [°C].
/// - `avg_ambient_temp_c` -- annual average outdoor dry-bulb temperature [°C].
/// - `roughness` -- surface roughness class for the DOE-2 `r_f` factor.
pub fn film_resistances(
    tilt_deg: f64,
    interior_zone: ZoneLabel,
    exterior_zone: ZoneLabel,
    avg_wind_speed_m_s: f64,
    avg_ground_temp_c: f64,
    avg_ambient_temp_c: f64,
    roughness: SurfaceRoughness,
) -> (f64, f64) {
    let temps = typical_zone_temps(avg_ground_temp_c, avg_ambient_temp_c);
    let t_int = temps[interior_zone.height_order() as usize];
    let t_ext = temps[exterior_zone.height_order() as usize];

    let ext_above = exterior_zone.height_order() > interior_zone.height_order();
    let t_ext_hotter = t_ext >= t_int;
    // "above_hotter": the warmer side faces upward -- enhanced convection.
    let above_hotter = !(ext_above ^ t_ext_hotter);

    // Minimum delta-T floor for TARP natural convection [°C / K].
    // Prevents near-zero h (and thus near-infinite R) when zone temperatures
    // are close. Value from EnergyPlus ConvectionCoefficients.cc.
    const MIN_DELTA_T_TARP_NATURAL_C: f64 = 12.9;

    let delta_t = (t_ext - t_int).abs().max(MIN_DELTA_T_TARP_NATURAL_C);

    let h_conv = tarp_h_natural(tilt_deg, delta_t, above_hotter);

    // Interior film resistance is convection-only: R_film = 1/h_conv.
    // Inter-surface longwave radiation is handled separately by the
    // star-mesh radiation conductances in the A-matrix (TRNSYS Type 56,
    // ESP-r, EnergyPlus "Option 2" architecture).  At ε_ir = 0.9 and
    // T_ref = 293.15 K the linearized h_rad ≈ 5.14 W/(m²·K); this is
    // NOT included in R_film to avoid double-counting with the star-mesh.
    //
    // The combined h_si = h_conv + h_rad ≈ 8.21 W/(m²·K) for a vertical
    // surface (TARP h_conv ≈ 3.08), consistent with ASHRAE 140-2017
    // Table 25.  In the decomposed model, h_conv provides the
    // zone-air ↔ surface coupling and h_rad provides the
    // surface ↔ surface coupling via the star-mesh.
    //
    // h_rad is computed here for use by the window U-factor decomposition
    // in boundary_rc.rs, where h_conv = h_si - h_rad recovers the
    // convection-only film from the combined window interior film.
    const INTERIOR_EMISSIVITY: f64 = 0.9;
    const INTERIOR_MEAN_TEMP_K: f64 = 293.15;
    let _h_rad = 4.0
        * INTERIOR_EMISSIVITY
        * crate::constants::STEFAN_BOLTZMANN
        * INTERIOR_MEAN_TEMP_K.powi(3);

    let r_int = 1.0 / h_conv;

    let r_ext = if exterior_zone == ZoneLabel::Outdoor {
        let h_glass = (h_conv.powi(2) + (3.40 * avg_wind_speed_m_s.powf(0.75)).powi(2)).sqrt();
        let h_forced = roughness.factor() * (h_glass - h_conv);
        1.0 / (h_conv + h_forced)
    } else if exterior_zone == ZoneLabel::Ground {
        1.0 / h_conv
    } else {
        r_int
    };

    (r_int, r_ext)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::test_utils::approx_eq;

    fn assert_approx(actual: f64, expected: f64, tol: f64) {
        approx_eq(actual, expected, tol);
    }

    #[test]
    fn vertical_surface_natural_h_and_resistance() {
        // h = 1.31 * 12.9^(1/3) ≈ 3.0759; R = 1/h ≈ 0.3251
        let h = tarp_h_natural(90.0, 12.9, true);
        let expected_h = 1.31 * 12.9_f64.cbrt();
        assert_approx(h, expected_h, 1e-10);
        assert_approx(1.0 / h, 1.0 / expected_h, 1e-6);
    }

    #[test]
    fn enhanced_tilt_above_hotter() {
        // tilt=0 (horizontal), above_hotter=true → enhanced formula
        // h = 9.482 * 12.9^(1/3) / (7.238 - |cos(0 rad)|)
        //   = 9.482 * 12.9^(1/3) / (7.238 - 1.0)
        let h = tarp_h_natural(0.0, 12.9, true);
        let expected_h = 9.482 * 12.9_f64.cbrt() / (7.238 - 0.0_f64.to_radians().cos().abs());
        assert_approx(h, expected_h, 1e-10);
    }

    #[test]
    fn film_resistances_typical_wall_outdoor() {
        let (r_int, r_ext) = film_resistances(
            90.0,
            ZoneLabel::Conditioned,
            ZoneLabel::Outdoor,
            2.0,
            10.0,
            10.0,
            SurfaceRoughness::Rough,
        );
        let h_conv = 1.31 * 12.9_f64.cbrt();
        let h_rad = 4.0 * 0.9 * crate::constants::STEFAN_BOLTZMANN * 293.15_f64.powi(3);
        // Interior film is convection-only (h_rad handled by star-mesh).
        assert_approx(r_int, 1.0 / h_conv, 1e-10);
        // Conv-only R_film is larger than combined R_film.
        assert!(r_int > 1.0 / (h_conv + h_rad), "conv-only r_int={r_int} must be > combined {:.4}", 1.0 / (h_conv + h_rad));
        let h_glass = (h_conv.powi(2) + (3.40 * 2.0_f64.powf(0.75)).powi(2)).sqrt();
        let h_forced = 1.67 * (h_glass - h_conv);
        assert_approx(r_ext, 1.0 / (h_conv + h_forced), 1e-10);
        assert!(r_ext < r_int, "r_ext={r_ext} should be < r_int={r_int}");
    }

    #[test]
    fn film_resistances_interior_boundary_no_forced_convection() {
        // LIV interior, ATC exterior -- not outdoor, so r_ext == r_int.
        let (r_int, r_ext) = film_resistances(
            90.0,
            ZoneLabel::Conditioned,
            ZoneLabel::Attic,
            2.0,
            10.0,
            10.0,
            SurfaceRoughness::Rough,
        );
        assert_approx(r_int, r_ext, 1e-15);
    }

    #[test]
    fn interior_film_resistance_is_convection_only() {
        // For a vertical wall (tilt=90°, ΔT=12.9°C), TARP gives
        // h_conv = 1.31 × 12.9^(1/3) ≈ 3.076 W/(m²·K).
        // Linearized h_rad = 4·ε·σ·T_ref³ ≈ 5.14 W/(m²·K) at ε=0.9, T=20°C.
        // In Option 2 (EnergyPlus/TRNSYS/ESP-r), R_film = 1/h_conv (conv-only).
        // h_rad is carried by the star-mesh radiation conductances in the
        // A-matrix, not by R_film.
        let (r_int, _) = film_resistances(
            90.0,
            ZoneLabel::Conditioned,
            ZoneLabel::Outdoor,
            2.0,
            10.0,
            10.0,
            SurfaceRoughness::Rough,
        );
        let h_conv = 1.31 * 12.9_f64.cbrt();
        let h_rad = 4.0 * 0.9 * crate::constants::STEFAN_BOLTZMANN * 293.15_f64.powi(3);
        let r_conv_only = 1.0 / h_conv;
        let r_combined = 1.0 / (h_conv + h_rad);

        // R_film must equal convection-only value.
        assert_approx(r_int, r_conv_only, 1e-10);
        // Conv-only R_film is distinctly larger than combined.
        assert!(
            r_int > r_combined + 0.1,
            "conv-only R_film ({r_int:.4}) must be significantly larger than combined ({r_combined:.4})"
        );
        // Verify approximate expected value for vertical wall conv-only film.
        assert!(
            (r_int - 0.325).abs() < 0.01,
            "vertical wall conv-only R_film ≈ 0.325, got {r_int:.4}"
        );
    }

    #[test]
    fn exterior_film_resistance_is_convection_only_no_h_rad() {
        // Exterior film resistance for outdoor-facing surfaces uses
        // h_conv + h_forced (no h_rad). Exterior LWR is handled by
        // the explicit exterior longwave solver, not by h_rad in R_film.
        let (_, r_ext) = film_resistances(
            90.0,
            ZoneLabel::Conditioned,
            ZoneLabel::Outdoor,
            2.0,
            10.0,
            10.0,
            SurfaceRoughness::Rough,
        );
        let h_conv = 1.31 * 12.9_f64.cbrt();
        let h_glass = (h_conv.powi(2) + (3.40 * 2.0_f64.powf(0.75)).powi(2)).sqrt();
        let h_forced = 1.67 * (h_glass - h_conv);
        let r_expected = 1.0 / (h_conv + h_forced);
        assert_approx(r_ext, r_expected, 1e-10);
    }

    /// When delta_t=0, all TARP formulas produce h=0 (cbrt(0)=0).
    /// This is physically correct: no temperature difference means no
    /// buoyancy-driven convection.
    #[test]
    fn tarp_h_natural_zero_delta_t_returns_zero() {
        let h_vert = tarp_h_natural(90.0, 0.0, true);
        assert_approx(h_vert, 0.0, 1e-15);

        let h_horiz_above = tarp_h_natural(0.0, 0.0, true);
        assert_approx(h_horiz_above, 0.0, 1e-15);

        let h_horiz_below = tarp_h_natural(0.0, 0.0, false);
        assert_approx(h_horiz_below, 0.0, 1e-15);
    }

    #[test]
    fn typical_zone_temps_known_anchors_and_interpolation() {
        let temps = typical_zone_temps(10.0, 10.0);
        // Anchors
        assert_approx(
            temps[ZoneLabel::Ground.height_order() as usize],
            10.0,
            1e-12,
        );
        assert_approx(
            temps[ZoneLabel::Conditioned.height_order() as usize],
            20.0,
            1e-12,
        );
        assert_approx(
            temps[ZoneLabel::Outdoor.height_order() as usize],
            15.0,
            1e-12,
        );
        // Interpolated: Foundation = 10 + (20-10)*(1/2) = 15
        assert_approx(
            temps[ZoneLabel::Foundation.height_order() as usize],
            15.0,
            1e-12,
        );
        // Garage = 20 + (15-20)*(1/3) = 20 - 5/3 ≈ 18.333...
        assert_approx(
            temps[ZoneLabel::Garage.height_order() as usize],
            20.0 + (15.0 - 20.0) / 3.0,
            1e-12,
        );
        // Attic = 20 + (15-20)*(2/3) = 20 - 10/3 ≈ 16.666...
        assert_approx(
            temps[ZoneLabel::Attic.height_order() as usize],
            20.0 + (15.0 - 20.0) * 2.0 / 3.0,
            1e-12,
        );
    }
}
