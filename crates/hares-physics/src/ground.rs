//! Ground heat transfer models.
//!
//! Implements the Kusuda-Achenbach undisturbed ground temperature model
//! (EnergyPlus Engineering Reference Ch. 3.17) and the ASHRAE 90.1-2022
//! perimeter conduction factor (F2) method for slab-on-grade heat loss.
//!
//! Reference:
//! - Kusuda, T. and Achenbach, P.R. (1965), "Earth Temperatures and Thermal
//!   Diffusivity at Selected Stations in the United States", ASHRAE Transactions,
//!   Vol. 71(1), pp. 61-74.
//! - ASHRAE Handbook of Fundamentals 2021, Ch. 27 (Heat, Air, and Moisture Control
//!   in Building Assemblies — Examples) — below-grade heat transfer boundary
//!   conditions for basement walls, slab-on-grade, and crawlspace floors.
//! - ASHRAE 90.1-2022, Table A6.3.1 — slab-on-grade F-factor perimeter heat loss
//!   coefficients, converted Btu/(h·ft·°F) → W/(m·K) via factor 1.73074.
//! - EnergyPlus Engineering Reference: Ground Heat Transfer, "Undisturbed Ground
//!   Temperature Model: Kusuda-Achenbach".

use std::f64::consts::PI;

/// Default soil thermal diffusivity [m²/day].
///
/// Typical range for moist soil: 0.04–0.07 m²/day.
/// HARES uses 0.05 m²/day for moist mixed clay/sand soils (representative of
/// typical residential sites). EnergyPlus CalcSoilSurfTemp defaults to
/// 0.0208 m²/day (2.4e-7 m²/s) for generic dry soil -- our higher value
/// reflects the wetter conditions typical of foundation-adjacent soil.
pub const DEFAULT_SOIL_DIFFUSIVITY_M2_PER_DAY: f64 = 0.05;

/// Default phase shift: day of minimum surface temperature.
///
/// For northern hemisphere mid-latitudes, minimum ground surface temp occurs
/// around day 35 (early February). EnergyPlus default.
pub const DEFAULT_PHASE_DAY_NORTHERN: f64 = 35.0;

/// Default phase shift for southern hemisphere.
/// 182.5-day offset from northern = average half-year (365/2).
pub const DEFAULT_PHASE_DAY_SOUTHERN: f64 = 35.0 + 182.5;

/// Period [days] -- one year.
const TAU_DAYS: f64 = 365.0;

/// Kusuda-Achenbach undisturbed ground temperature model.
///
/// Computes ground temperature at depth `z` and day of year `t`:
///
/// ```text
/// T(z,t) = T_mean - T_amplitude × exp(-z × √(π / (α × τ)))
///          × cos(2π(t - θ)/τ - z × √(π / (α × τ)))
/// ```
///
/// # Arguments
///
/// * `depth_m` -- Depth below ground surface [m]. 0 = surface.
/// * `day_of_year` -- Day of year [1–366]. 1 = January 1.
/// * `t_mean_annual_c` -- Average annual soil surface temperature [°C].
///   Approximated by annual average outdoor dry-bulb temperature.
/// * `t_amplitude_c` -- Amplitude of yearly soil surface temperature variation [°C].
///   Half of (max monthly average - min monthly average) outdoor temperature.
/// * `phase_day` -- Day of year with minimum surface temperature.
///   ~35 for northern hemisphere, ~217 for southern.
/// * `diffusivity_m2_per_day` -- Soil thermal diffusivity [m²/day].
///   Typical: 0.04–0.07. Use [`DEFAULT_SOIL_DIFFUSIVITY_M2_PER_DAY`] if unknown.
///
/// # Returns
///
/// Undisturbed ground temperature [°C] at the specified depth and time.
#[must_use]
pub fn kusuda_achenbach_temp(
    depth_m: f64,
    day_of_year: f64,
    t_mean_annual_c: f64,
    t_amplitude_c: f64,
    phase_day: f64,
    diffusivity_m2_per_day: f64,
) -> f64 {
    if diffusivity_m2_per_day <= 0.0 || !diffusivity_m2_per_day.is_finite() {
        return t_mean_annual_c;
    }
    let depth = depth_m.max(0.0);
    let decay = (PI / (diffusivity_m2_per_day * TAU_DAYS)).sqrt();
    let attenuation = (-depth * decay).exp();
    let phase = 2.0 * PI * (day_of_year - phase_day) / TAU_DAYS - depth * decay;

    t_mean_annual_c - t_amplitude_c * attenuation * phase.cos()
}

/// ASHRAE slab perimeter heat loss [W].
///
/// Simple perimeter-loss method from ASHRAE Handbook of Fundamentals Ch. 27:
/// `Q = F2 × P × (T_indoor - T_ground_surface)`
///
/// where F2 is the heat loss coefficient per unit length of exposed perimeter
/// [W/(m·K)], determined by insulation configuration.
///
/// # Arguments
///
/// * `perimeter_m` -- Exposed perimeter length [m].
/// * `f2_w_per_m_k` -- Perimeter heat loss coefficient [W/(m·K)].
///   See [`f2_coefficient`] for ASHRAE 90.1-2022 Table A6.3.1 values.
/// * `t_indoor_c` -- Indoor zone temperature [°C].
/// * `t_ground_surface_c` -- Ground surface temperature [°C].
#[must_use]
pub fn slab_perimeter_loss_w(
    perimeter_m: f64,
    f2_w_per_m_k: f64,
    t_indoor_c: f64,
    t_ground_surface_c: f64,
) -> f64 {
    f2_w_per_m_k * perimeter_m * (t_indoor_c - t_ground_surface_c)
}

/// Foundation wall heat loss [W].
///
/// Simplified below-grade wall loss:
/// `Q = A × (T_indoor - T_ground) / R_wall`
///
/// where T_ground is the Kusuda-Achenbach temperature at the average
/// below-grade depth.
///
/// # Arguments
///
/// * `below_grade_area_m2` -- Below-grade wall area [m²].
/// * `r_wall_m2_k_w` -- Total wall R-value including insulation [m²·K/W].
/// * `t_indoor_c` -- Indoor zone temperature [°C].
/// * `t_ground_c` -- Ground temperature at average below-grade depth [°C].
#[must_use]
pub fn foundation_wall_loss_w(
    below_grade_area_m2: f64,
    r_wall_m2_k_w: f64,
    t_indoor_c: f64,
    t_ground_c: f64,
) -> f64 {
    if r_wall_m2_k_w <= 0.0 {
        return 0.0;
    }
    below_grade_area_m2 * (t_indoor_c - t_ground_c) / r_wall_m2_k_w
}

/// Perimeter loss coefficient F2 [W/(m·K)] per ASHRAE 90.1-2022 Table A6.3.1.
///
/// Returns the F2 heat loss coefficient for slab-on-grade per unit length of
/// exposed perimeter.  Values are derived from the ASHRAE 90.1-2022 F-factor
/// table (Table A6.3.1) via the conversion `Btu/(h·ft·°F) × 1.73074 = W/(m·K)`.
///
/// # Arguments
///
/// * `insulation_r_m2_k_w` -- Perimeter insulation nominal R-value [m²·K/W].
///   Domestic slabs are assumed uninsulated when this is missing/zero.
/// * `heated` -- Whether the slab is heated (`true`) or unheated (`false`).
///   Heated slabs have significantly higher perimeter loss because they
///   maintain a higher temperature at the slab edge.  Typical residential
///   slabs-on-grade are unheated.
#[must_use]
pub fn f2_coefficient(insulation_r_m2_k_w: f64, heated: bool) -> f64 {
    if insulation_r_m2_k_w >= 1.76 {
        // R-10+ perimeter insulation
        if heated { 2.250 } else { 1.229 }
    } else if insulation_r_m2_k_w >= 0.88 {
        // R-5 perimeter insulation
        if heated { 2.267 } else { 1.246 }
    } else {
        // Uninsulated slab
        if heated { 2.336 } else { 1.263 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kusuda_at_surface_matches_sinusoidal() {
        // At depth=0, the model reduces to T_mean - T_amp * cos(2π(t-θ)/τ)
        let t_mean = 12.0;
        let t_amp = 10.0;
        let phase = 35.0;

        // At t=phase (day of minimum), cos(0) = 1 → T = T_mean - T_amp = 2°C
        let t_min = kusuda_achenbach_temp(0.0, phase, t_mean, t_amp, phase, 0.05);
        assert!(
            (t_min - 2.0).abs() < 0.1,
            "surface temp at minimum day should be ~2°C, got {t_min}"
        );

        // At t=phase+182.5 (day of maximum), cos(π) = -1 → T = T_mean + T_amp = 22°C
        let t_max = kusuda_achenbach_temp(0.0, phase + 182.5, t_mean, t_amp, phase, 0.05);
        assert!(
            (t_max - 22.0).abs() < 0.1,
            "surface temp at maximum day should be ~22°C, got {t_max}"
        );
    }

    #[test]
    fn kusuda_at_depth_attenuates_amplitude() {
        let t_mean = 12.0;
        let t_amp = 10.0;
        let phase = 35.0;
        let alpha = 0.05;

        // At 3m depth, amplitude should be significantly attenuated
        let t_3m_min = kusuda_achenbach_temp(3.0, phase, t_mean, t_amp, phase, alpha);
        let t_3m_max = kusuda_achenbach_temp(3.0, phase + 182.5, t_mean, t_amp, phase, alpha);
        let amplitude_3m = (t_3m_max - t_3m_min) / 2.0;

        assert!(
            amplitude_3m < 2.0,
            "amplitude at 3m should be <2°C (highly damped), got {amplitude_3m}"
        );
        assert!(
            amplitude_3m > 0.0,
            "amplitude at 3m should still be positive"
        );
    }

    #[test]
    fn kusuda_deep_approaches_annual_mean() {
        let t_mean = 12.0;
        let t_amp = 10.0;

        // At 10m depth, temperature should be very close to annual mean
        let t_deep = kusuda_achenbach_temp(10.0, 180.0, t_mean, t_amp, 35.0, 0.05);
        assert!(
            (t_deep - t_mean).abs() < 1.0,
            "deep ground temp should be ~{t_mean}°C ±1, got {t_deep}"
        );
    }

    #[test]
    fn slab_perimeter_loss_positive_when_indoor_warmer() {
        let q = slab_perimeter_loss_w(40.0, 1.17, 20.0, 5.0);
        assert!(q > 0.0, "heat loss should be positive when indoor > ground");
        // Expected: 1.17 × 40 × 15 = 702 W
        assert!((q - 702.0).abs() < 1.0, "expected ~702 W, got {q}");
    }

    #[test]
    fn insulated_slab_loses_less_than_uninsulated() {
        let q_uninsulated = slab_perimeter_loss_w(40.0, f2_coefficient(0.0, false), 20.0, 5.0);
        let q_r5 = slab_perimeter_loss_w(40.0, f2_coefficient(0.88, false), 20.0, 5.0);
        let q_r10 = slab_perimeter_loss_w(40.0, f2_coefficient(1.76, false), 20.0, 5.0);

        assert!(q_uninsulated > q_r5, "R-5 should reduce loss");
        assert!(q_r5 > q_r10, "R-10 should reduce further");
    }

    #[test]
    fn foundation_wall_loss_proportional_to_area() {
        let q1 = foundation_wall_loss_w(10.0, 2.0, 20.0, 8.0);
        let q2 = foundation_wall_loss_w(20.0, 2.0, 20.0, 8.0);
        assert!(
            (q2 - 2.0 * q1).abs() < 0.01,
            "doubling area should double loss"
        );
    }

    #[test]
    fn foundation_wall_zero_r_returns_zero() {
        let q = foundation_wall_loss_w(10.0, 0.0, 20.0, 8.0);
        assert_eq!(
            q, 0.0,
            "zero R-value should return zero loss (not infinite)"
        );
    }

    #[test]
    fn kusuda_invalid_diffusivity_returns_mean() {
        let t = kusuda_achenbach_temp(1.0, 100.0, 12.0, 10.0, 35.0, 0.0);
        assert_eq!(t, 12.0, "zero diffusivity should return annual mean");
    }

    /// Regression test: verifies the Definition-of-Done
    /// numerical example (140 m² slab, P=50 m, uninsulated unheated F2≈1.263, ΔT=15°C → ~947 W).
    ///
    /// This test exercises slab_perimeter_loss_w and f2_coefficient in isolation.
    /// It will keep passing regardless of whether those functions are wired into
    /// the solver — use it to confirm the physics is correct, not that the
    /// integration is done.
    #[test]
    fn slab_140m2_p50_uninsulated_15k_delta_approx_947w() {
        let perimeter_m = 50.0;
        let f2 = f2_coefficient(0.0, false); // uninsulated unheated → 1.263 W/(m·K)
        let t_indoor_c = 20.0;
        let t_ground_c = 5.0; // ΔT = 15 K
        let q = slab_perimeter_loss_w(perimeter_m, f2, t_indoor_c, t_ground_c);
        // Expected: 1.263 × 50 × 15 = 947.25 W (~947 W, ±5%)
        let expected = 947.25;
        let tolerance = expected * 0.05;
        assert!(
            (q - expected).abs() <= tolerance,
            "expected {expected} ± {tolerance} W, got {q} W"
        );
    }

    /// Regression test: insulated slab (R-5 perimeter) must
    /// produce strictly lower heat loss than uninsulated at the same ΔT.
    #[test]
    fn r5_perimeter_slab_lower_loss_than_uninsulated() {
        let perimeter_m = 50.0;
        let delta_t = 15.0;
        let t_indoor = 20.0;
        let t_ground = t_indoor - delta_t;
        let q_unins =
            slab_perimeter_loss_w(perimeter_m, f2_coefficient(0.0, false), t_indoor, t_ground);
        let q_r5 =
            slab_perimeter_loss_w(perimeter_m, f2_coefficient(0.88, false), t_indoor, t_ground);
        assert!(
            q_r5 < q_unins,
            "R-5 perimeter insulation should reduce loss: unins={q_unins} W, R-5={q_r5} W"
        );
    }
}
