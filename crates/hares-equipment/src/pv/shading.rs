//! PV shading models for residential near-shading scenarios.
//!
//! Configurable models for geometric shading from building features (dormers,
//! chimneys), trees, and neighboring buildings. Not a full ray-tracer (that's
//! SAM's domain) — these are practical models for common residential scenarios.

use serde::{Deserialize, Serialize};

/// Shading model applied per-array to reduce effective irradiance.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub enum ShadingModel {
    /// No shading applied (default). Identical to pre-shading behavior.
    #[default]
    None,
    /// Constant annual shading loss (e.g., 0.1 = 10% loss year-round).
    FixedLoss { annual_fraction: f64 },
    /// Per-month shading fractions [Jan..Dec]. Each in [0, 1] where 0 = no shade, 1 = full shade.
    MonthlyLoss { fractions: [f64; 12] },
    /// Single horizon obstruction defined by azimuth center, elevation, and angular width.
    /// Blocks direct beam when solar position falls within the obstruction cone.
    ObstructionAngle {
        azimuth_deg: f64,
        elevation_deg: f64,
        width_deg: f64,
    },
    /// Arbitrary horizon profile as sorted (azimuth_deg, elevation_deg) pairs.
    /// Sun positions below the interpolated horizon line are fully shaded (beam blocked).
    /// Diffuse sky dome reduction is not modeled (second-order for residential near-shading).
    HorizonProfile { points: Vec<(f64, f64)> },
}

impl ShadingModel {
    /// Compute the shading factor for the current solar position.
    ///
    /// Returns a value in [0, 1] where 1.0 = no shading, 0.0 = fully shaded.
    /// Applied multiplicatively to effective irradiance.
    ///
    /// `solar_altitude_deg`: sun elevation above horizon [0-90].
    /// `solar_azimuth_deg`: sun compass bearing [0-360].
    /// `month`: 1-indexed month [1-12].
    #[must_use]
    pub fn shading_factor(
        &self,
        solar_altitude_deg: f64,
        solar_azimuth_deg: f64,
        month: u32,
    ) -> f64 {
        match self {
            Self::None => 1.0,
            Self::FixedLoss { annual_fraction } => (1.0 - annual_fraction).clamp(0.0, 1.0),
            Self::MonthlyLoss { fractions } => {
                let idx = (month.saturating_sub(1) as usize).min(11);
                (1.0 - fractions[idx]).clamp(0.0, 1.0)
            }
            Self::ObstructionAngle {
                azimuth_deg,
                elevation_deg,
                width_deg,
            } => obstruction_factor(
                solar_altitude_deg,
                solar_azimuth_deg,
                *azimuth_deg,
                *elevation_deg,
                *width_deg,
            ),
            Self::HorizonProfile { points } => {
                horizon_profile_factor(solar_altitude_deg, solar_azimuth_deg, points)
            }
        }
    }
}

/// Shading from a single obstruction defined by center azimuth, elevation, and width.
///
/// If the sun's azimuth is within ±width/2 of the obstruction azimuth AND the
/// sun's altitude is below the obstruction elevation, beam is fully blocked.
fn obstruction_factor(
    solar_alt_deg: f64,
    solar_az_deg: f64,
    obs_az_deg: f64,
    obs_elev_deg: f64,
    obs_width_deg: f64,
) -> f64 {
    if solar_alt_deg <= 0.0 {
        return 1.0; // sun below horizon, no production anyway
    }
    let half_width = obs_width_deg / 2.0;
    let az_diff = angular_distance(solar_az_deg, obs_az_deg);
    if az_diff <= half_width && solar_alt_deg < obs_elev_deg {
        0.0 // beam fully blocked by obstruction
    } else {
        1.0
    }
}

/// Shading from an arbitrary horizon profile.
///
/// The profile is a sorted list of (azimuth, elevation) pairs defining the
/// horizon line. The sun is shaded if its altitude is below the interpolated
/// horizon elevation at its azimuth.
fn horizon_profile_factor(solar_alt_deg: f64, solar_az_deg: f64, points: &[(f64, f64)]) -> f64 {
    if points.is_empty() || solar_alt_deg <= 0.0 {
        return 1.0;
    }
    let horizon_elev = interpolate_horizon(solar_az_deg, points);
    if solar_alt_deg < horizon_elev {
        0.0 // sun below horizon profile
    } else {
        1.0
    }
}

/// Linear interpolation of horizon elevation at a given azimuth.
///
/// Wraps around 360° if the profile doesn't cover the full circle.
fn interpolate_horizon(azimuth_deg: f64, points: &[(f64, f64)]) -> f64 {
    if points.len() == 1 {
        return points[0].1;
    }
    let az = azimuth_deg.rem_euclid(360.0);

    // Find bracketing points
    for i in 0..points.len() - 1 {
        let (az0, el0) = points[i];
        let (az1, el1) = points[i + 1];
        if az >= az0 && az <= az1 {
            let span = az1 - az0;
            if span.abs() < f64::EPSILON {
                return el0;
            }
            let frac = (az - az0) / span;
            return el0 + frac * (el1 - el0);
        }
    }

    // Wrap-around: interpolate between last and first point
    let (az_last, el_last) = points[points.len() - 1];
    let (az_first, el_first) = points[0];
    let span = (az_first + 360.0) - az_last;
    if span.abs() < f64::EPSILON {
        return el_last;
    }
    let effective_az = if az >= az_last { az } else { az + 360.0 };
    let frac = (effective_az - az_last) / span;
    el_last + frac * (el_first - el_last)
}

/// Angular distance between two azimuths in [0, 180].
fn angular_distance(a: f64, b: f64) -> f64 {
    let diff = (a - b).rem_euclid(360.0);
    diff.min(360.0 - diff)
}

/// Parse a `ShadingModel` from equipment config keys.
///
/// Expected config keys:
/// - `shading_model`: "none", "fixed", "monthly", "obstruction", "horizon"
/// - `shading_annual_fraction`: for FixedLoss
/// - `shading_monthly_fractions`: [f64; 12] for MonthlyLoss
/// - `shading_obstruction_azimuth_deg`, `_elevation_deg`, `_width_deg`: for ObstructionAngle
/// - `shading_horizon_points`: flat [az0, el0, az1, el1, ...] for HorizonProfile
pub fn parse_shading_config(config: &crate::EquipmentConfig) -> ShadingModel {
    let model_name = config.get_str("shading_model").unwrap_or("none");
    match model_name {
        "fixed" | "FixedLoss" => {
            let frac = config.get_f64("shading_annual_fraction").unwrap_or(0.0);
            ShadingModel::FixedLoss {
                annual_fraction: frac.clamp(0.0, 1.0),
            }
        }
        "monthly" | "MonthlyLoss" => {
            let raw = config
                .get_f64_array("shading_monthly_fractions")
                .unwrap_or_default();
            let mut fractions = [0.0_f64; 12];
            for (i, f) in fractions.iter_mut().enumerate() {
                *f = raw.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
            }
            ShadingModel::MonthlyLoss { fractions }
        }
        "obstruction" | "ObstructionAngle" => {
            let az = config
                .get_f64("shading_obstruction_azimuth_deg")
                .unwrap_or(0.0);
            let el = config
                .get_f64("shading_obstruction_elevation_deg")
                .unwrap_or(30.0);
            let w = config
                .get_f64("shading_obstruction_width_deg")
                .unwrap_or(60.0);
            ShadingModel::ObstructionAngle {
                azimuth_deg: az.rem_euclid(360.0),
                elevation_deg: el.clamp(0.0, 90.0),
                width_deg: w.clamp(0.0, 180.0),
            }
        }
        "horizon" | "HorizonProfile" => {
            let raw = config
                .get_f64_array("shading_horizon_points")
                .unwrap_or_default();
            if !raw.len().is_multiple_of(2) {
                tracing::warn!(
                    "shading_horizon_points has odd length {}; trailing value ignored",
                    raw.len()
                );
            }
            let mut points: Vec<(f64, f64)> = raw
                .chunks_exact(2)
                .filter(|c| c[0].is_finite() && c[1].is_finite())
                .map(|c| (c[0].rem_euclid(360.0), c[1].clamp(0.0, 90.0)))
                .collect();
            points.sort_by(|a, b| {
                a.0.partial_cmp(&b.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            });
            ShadingModel::HorizonProfile { points }
        }
        _ => ShadingModel::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_returns_one() {
        let m = ShadingModel::None;
        assert_eq!(m.shading_factor(45.0, 180.0, 6), 1.0);
    }

    #[test]
    fn fixed_loss_reduces_by_fraction() {
        let m = ShadingModel::FixedLoss {
            annual_fraction: 0.1,
        };
        let f = m.shading_factor(45.0, 180.0, 6);
        assert!((f - 0.9).abs() < 1e-10);
    }

    #[test]
    fn monthly_loss_uses_correct_month() {
        let mut fractions = [0.0; 12];
        fractions[0] = 0.5; // January = 50% shaded
        fractions[6] = 0.1; // July = 10% shaded
        let m = ShadingModel::MonthlyLoss { fractions };

        assert!((m.shading_factor(45.0, 180.0, 1) - 0.5).abs() < 1e-10); // Jan
        assert!((m.shading_factor(45.0, 180.0, 7) - 0.9).abs() < 1e-10); // Jul
        assert!((m.shading_factor(45.0, 180.0, 3) - 1.0).abs() < 1e-10); // Mar (0% shade)
    }

    #[test]
    fn obstruction_blocks_when_sun_behind() {
        let m = ShadingModel::ObstructionAngle {
            azimuth_deg: 90.0, // east
            elevation_deg: 20.0,
            width_deg: 60.0,
        };
        // Sun in the east at low altitude — blocked
        assert_eq!(m.shading_factor(10.0, 90.0, 6), 0.0);
        // Sun in the east but high altitude — not blocked
        assert_eq!(m.shading_factor(30.0, 90.0, 6), 1.0);
        // Sun in the south — not blocked (outside width)
        assert_eq!(m.shading_factor(10.0, 180.0, 6), 1.0);
    }

    #[test]
    fn obstruction_east_blocks_morning_not_afternoon() {
        let m = ShadingModel::ObstructionAngle {
            azimuth_deg: 90.0,
            elevation_deg: 25.0,
            width_deg: 40.0,
        };
        // Morning sun (east, low) — blocked
        assert_eq!(m.shading_factor(15.0, 85.0, 6), 0.0);
        // Afternoon sun (west) — not blocked
        assert_eq!(m.shading_factor(15.0, 270.0, 6), 1.0);
    }

    #[test]
    fn horizon_profile_interpolates() {
        let m = ShadingModel::HorizonProfile {
            points: vec![(0.0, 10.0), (90.0, 20.0), (180.0, 5.0), (270.0, 15.0)],
        };
        // At azimuth 45° (between 0° and 90°), horizon ≈ 15°
        // Sun at 10° altitude → blocked
        assert_eq!(m.shading_factor(10.0, 45.0, 6), 0.0);
        // Sun at 20° altitude → not blocked
        assert_eq!(m.shading_factor(20.0, 45.0, 6), 1.0);
    }

    #[test]
    fn horizon_profile_empty_returns_one() {
        let m = ShadingModel::HorizonProfile { points: vec![] };
        assert_eq!(m.shading_factor(10.0, 90.0, 6), 1.0);
    }

    #[test]
    fn angular_distance_wraps() {
        assert!((angular_distance(350.0, 10.0) - 20.0).abs() < 1e-10);
        assert!((angular_distance(10.0, 350.0) - 20.0).abs() < 1e-10);
        assert!((angular_distance(0.0, 180.0) - 180.0).abs() < 1e-10);
    }

    #[test]
    fn fixed_loss_clamps_to_valid_range() {
        let over = ShadingModel::FixedLoss {
            annual_fraction: 1.5,
        };
        assert_eq!(over.shading_factor(45.0, 180.0, 6), 0.0);

        let under = ShadingModel::FixedLoss {
            annual_fraction: -0.5,
        };
        assert_eq!(under.shading_factor(45.0, 180.0, 6), 1.0);
    }

    #[test]
    fn horizon_profile_wrap_around_interpolates() {
        // Profile ends at 350° and starts at 10° — azimuth 0° wraps between them.
        let m = ShadingModel::HorizonProfile {
            points: vec![(10.0, 20.0), (180.0, 5.0), (350.0, 10.0)],
        };
        // At azimuth 0°, interpolate between 350°(10°) and 10°(20°):
        // span = (10 + 360) - 350 = 20, frac = (360 - 350) / 20 = 0.5
        // expected elevation = 10 + 0.5 * (20 - 10) = 15°
        // Sun at 10° altitude → below 15° horizon → blocked
        assert_eq!(m.shading_factor(10.0, 0.0, 6), 0.0);
        // Sun at 20° altitude → above 15° horizon → not blocked
        assert_eq!(m.shading_factor(20.0, 0.0, 6), 1.0);
    }

    #[test]
    fn config_parsing_fixed_loss() {
        let mut raw = std::collections::HashMap::new();
        raw.insert("shading_model".to_string(), "fixed".into());
        raw.insert("shading_annual_fraction".to_string(), 0.15.into());
        let cfg = crate::EquipmentConfig {
            name: "PV".to_string(),
            ochre_class: "PV".to_string(),
            raw_config: raw,
        };
        let model = parse_shading_config(&cfg);
        assert!((model.shading_factor(45.0, 180.0, 6) - 0.85).abs() < 1e-10);
    }

    #[test]
    fn config_parsing_monthly_loss() {
        let mut raw = std::collections::HashMap::new();
        raw.insert("shading_model".to_string(), "monthly".into());
        raw.insert(
            "shading_monthly_fractions".to_string(),
            crate::config::ConfigValue::FloatArray(vec![
                0.5, 0.4, 0.3, 0.2, 0.1, 0.0, 0.0, 0.1, 0.2, 0.3, 0.4, 0.5,
            ]),
        );
        let cfg = crate::EquipmentConfig {
            name: "PV".to_string(),
            ochre_class: "PV".to_string(),
            raw_config: raw,
        };
        let model = parse_shading_config(&cfg);
        assert!((model.shading_factor(45.0, 180.0, 1) - 0.5).abs() < 1e-10); // Jan: 50% shade
        assert!((model.shading_factor(45.0, 180.0, 6) - 1.0).abs() < 1e-10); // Jun: 0% shade
    }

    #[test]
    fn config_parsing_obstruction() {
        let mut raw = std::collections::HashMap::new();
        raw.insert("shading_model".to_string(), "obstruction".into());
        raw.insert("shading_obstruction_azimuth_deg".to_string(), 90.0.into());
        raw.insert("shading_obstruction_elevation_deg".to_string(), 25.0.into());
        raw.insert("shading_obstruction_width_deg".to_string(), 40.0.into());
        let cfg = crate::EquipmentConfig {
            name: "PV".to_string(),
            ochre_class: "PV".to_string(),
            raw_config: raw,
        };
        let model = parse_shading_config(&cfg);
        // Sun at east (90°), low altitude (10°) → blocked
        assert_eq!(model.shading_factor(10.0, 90.0, 6), 0.0);
        // Sun at south (180°) → not blocked
        assert_eq!(model.shading_factor(10.0, 180.0, 6), 1.0);
    }

    #[test]
    fn config_parsing_none_default() {
        let raw = std::collections::HashMap::new();
        let cfg = crate::EquipmentConfig {
            name: "PV".to_string(),
            ochre_class: "PV".to_string(),
            raw_config: raw,
        };
        let model = parse_shading_config(&cfg);
        assert_eq!(model.shading_factor(45.0, 180.0, 6), 1.0);
    }

    #[test]
    fn config_parsing_horizon_profile() {
        let mut raw = std::collections::HashMap::new();
        raw.insert("shading_model".to_string(), "horizon".into());
        raw.insert(
            "shading_horizon_points".to_string(),
            crate::config::ConfigValue::FloatArray(vec![90.0, 20.0, 180.0, 10.0, 270.0, 15.0]),
        );
        let cfg = crate::EquipmentConfig {
            name: "PV".to_string(),
            ochre_class: "PV".to_string(),
            raw_config: raw,
        };
        let model = parse_shading_config(&cfg);
        // Sun at east (90°), altitude 15° → below 20° horizon → blocked
        assert_eq!(model.shading_factor(15.0, 90.0, 6), 0.0);
        // Sun at east (90°), altitude 25° → above 20° horizon → not blocked
        assert_eq!(model.shading_factor(25.0, 90.0, 6), 1.0);
    }

    #[test]
    fn monthly_loss_clamps_out_of_range_months() {
        let mut fractions = [0.1; 12];
        fractions[0] = 0.5; // Jan
        fractions[11] = 0.3; // Dec
        let m = ShadingModel::MonthlyLoss { fractions };
        // month=0 (invalid) saturates to index 0 (January)
        assert!((m.shading_factor(45.0, 180.0, 0) - 0.5).abs() < 1e-10);
        // month=13 (invalid) clamps to index 11 (December)
        assert!((m.shading_factor(45.0, 180.0, 13) - 0.7).abs() < 1e-10);
    }

    #[test]
    fn horizon_profile_single_point_returns_constant() {
        let m = ShadingModel::HorizonProfile {
            points: vec![(0.0, 15.0)],
        };
        // Below 15° → blocked
        assert_eq!(m.shading_factor(10.0, 90.0, 6), 0.0);
        // Above 15° → not blocked
        assert_eq!(m.shading_factor(20.0, 90.0, 6), 1.0);
    }

    #[test]
    fn horizon_profile_two_points_interpolates() {
        let m = ShadingModel::HorizonProfile {
            points: vec![(0.0, 10.0), (180.0, 30.0)],
        };
        // At azimuth 90° (midpoint), horizon ≈ 20°
        assert_eq!(m.shading_factor(15.0, 90.0, 6), 0.0); // below
        assert_eq!(m.shading_factor(25.0, 90.0, 6), 1.0); // above
    }

    #[test]
    fn obstruction_exact_boundary_altitude() {
        let m = ShadingModel::ObstructionAngle {
            azimuth_deg: 90.0,
            elevation_deg: 20.0,
            width_deg: 60.0,
        };
        // Altitude exactly at elevation → still blocked (< check means equal is not blocked)
        assert_eq!(m.shading_factor(20.0, 90.0, 6), 1.0);
        // Just below → blocked
        assert_eq!(m.shading_factor(19.9, 90.0, 6), 0.0);
    }

    #[test]
    fn obstruction_exact_boundary_azimuth() {
        let m = ShadingModel::ObstructionAngle {
            azimuth_deg: 90.0,
            elevation_deg: 20.0,
            width_deg: 60.0,
        };
        // Azimuth exactly at half_width boundary (60°/2 = 30° from center)
        // angular_distance(60, 90) = 30 = half_width → blocked (<=)
        assert_eq!(m.shading_factor(10.0, 60.0, 6), 0.0);
        // Just outside → not blocked
        assert_eq!(m.shading_factor(10.0, 59.0, 6), 1.0);
    }
}
