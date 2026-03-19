//! Water draw profile normalization for hot water systems.
//!
//! Implements the OCHRE/ResStock draw profile normalization pipeline:
//! 1. Compute expected average daily consumption from HPXML parameters (ANSI/RESNET 301-2014/2019).
//! 2. Scale raw fractional schedule data to match that consumption.
//! 3. Convert to mass flow rate (kg/s) for simulation.
//!
//! # Unit conventions
//! - All internal computation uses SI units.
//! - Daily consumption is expressed in L/day as a natural human-readable intermediate.
//! - The public API returns kg/s (mass flow).
//! - Gallons appear only inside conversion constants; they are never exposed.

/// US gallons per litre (exact by definition of US gallon).
const GAL_PER_L: f64 = 1.0 / 3.785_411_784;

/// Convert US gallons to litres.
#[inline]
fn gal_to_l(gal: f64) -> f64 {
    gal / GAL_PER_L
}

/// Normalize a raw water draw schedule (dimensionless fractions) to mass flow rates in kg/s.
///
/// Implements the OCHRE/ResStock draw profile normalization:
///
/// ```text
/// scale        = (avg_daily_L / 1440) / mean(raw_fractions)
/// result_kg_s  = raw_fractions * scale / 60.0
/// ```
///
/// The factor of 60 converts L/min → L/s, and water density is approximated as 1.0 kg/L.
///
/// # Parameters
/// - `raw_fractions`: dimensionless draw fractions (e.g., from a ResStock schedule column);
///   any non-negative values are accepted and their relative shape is preserved.
/// - `avg_daily_consumption_l`: expected average daily hot water consumption [L/day].
///
/// # Returns
/// A `Vec<f64>` of mass flow rates [kg/s], one per input sample.
/// Returns all-zeros when `avg_daily_consumption_l` is zero or negative,
/// or when the mean of the raw fractions is zero.
///
/// # Panics
/// Does not panic; edge cases return all-zeros.
pub fn normalize_draw_profile(raw_fractions: &[f64], avg_daily_consumption_l: f64) -> Vec<f64> {
    if raw_fractions.is_empty() {
        return Vec::new();
    }

    if avg_daily_consumption_l <= 0.0 {
        return vec![0.0; raw_fractions.len()];
    }

    let n = raw_fractions.len() as f64;
    let sum: f64 = raw_fractions.iter().sum();
    let mean = sum / n;

    if mean <= 0.0 {
        return vec![0.0; raw_fractions.len()];
    }

    // Convert daily target to L/min rate, then build a scale factor that maps
    // the fractional schedule mean to that rate.  Dividing by 60 converts L/min → L/s;
    // with density ≈ 1 kg/L this equals kg/s directly.
    let annual_mean_l_per_min = avg_daily_consumption_l / 1440.0;
    let scale = annual_mean_l_per_min / mean;

    raw_fractions.iter().map(|&f| f * scale / 60.0).collect()
}

/// Water fixture efficiency for a low-flow vs standard fixture set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixtureEfficiency {
    /// Standard fixtures: multiplier = 1.0.
    Standard,
    /// Low-flow fixtures: multiplier = 0.95 (OCHRE default).
    LowFlow,
}

impl FixtureEfficiency {
    /// Return the dimensionless efficiency multiplier.
    #[must_use]
    pub fn multiplier(self) -> f64 {
        match self {
            FixtureEfficiency::Standard => 1.0,
            FixtureEfficiency::LowFlow => 0.95,
        }
    }
}

/// Hot water distribution system type, used to compute distribution losses.
#[derive(Debug, Clone, PartialEq)]
pub enum DistributionSystem {
    /// Standard branched piping.  `pipe_r_value` is the insulation R-value [h·ft²·°F/Btu].
    /// `piping_length_m` is the total pipe length; defaults to a floor-area-derived value if
    /// `None`.
    Standard {
        pipe_r_value: f64,
        piping_length_m: Option<f64>,
        default_piping_length_m: f64,
    },
    /// Recirculation loop.  `pipe_r_value` is the insulation R-value [h·ft²·°F/Btu].
    /// `branch_loop_length_m` defaults to ~3.05 m (10 ft) if `None`.
    Recirculation {
        pipe_r_value: f64,
        branch_loop_length_m: Option<f64>,
    },
    /// Unknown or unspecified distribution: conservative defaults (no losses).
    Unknown,
}

/// Calculate average daily hot water consumption (fixture draws only) from HPXML parameters.
///
/// Based on ANSI/RESNET 301-2014 Addendum A-2015, Amendment on Domestic Hot Water (DHW) Systems,
/// as implemented in OCHRE and ResStock.
///
/// # Parameters
/// - `n_bedrooms`: number of bedrooms (integer, cast to `f64` for the formula).
/// - `fixture_efficiency`: low-flow vs standard fixture set.
/// - `usage_multiplier`: per-simulation usage multiplier from the HPXML extension element
///   (`WaterFixturesUsageMultiplier`).  Use `1.0` when absent.
///
/// # Returns
/// Average daily fixture hot water consumption in L/day.
pub fn fixture_daily_hot_water_l(
    n_bedrooms: f64,
    fixture_efficiency: FixtureEfficiency,
    usage_multiplier: f64,
) -> f64 {
    // ANSI/RESNET 301 reference: 14.6 + 10.0 * n_bedrooms [gal/day]
    let fixture_ref_gpd = 14.6 + 10.0 * n_bedrooms;
    gal_to_l(fixture_ref_gpd) * fixture_efficiency.multiplier() * usage_multiplier
}

/// Calculate average daily hot water consumption (distribution losses only) from HPXML parameters.
///
/// Based on ANSI/RESNET 301-2014 Addendum A-2015, Amendment on Domestic Hot Water (DHW) Systems,
/// as implemented in OCHRE and ResStock.
///
/// # Parameters
/// - `n_bedrooms`: number of bedrooms.
/// - `fixture_efficiency`: fixture set multiplier.
/// - `usage_multiplier`: per-simulation usage multiplier.
/// - `distribution`: hot water distribution system description.
///
/// # Returns
/// Average daily distribution hot water consumption in L/day (losses due to pipe waste water).
pub fn distribution_daily_hot_water_l(
    n_bedrooms: f64,
    fixture_efficiency: FixtureEfficiency,
    usage_multiplier: f64,
    distribution: &DistributionSystem,
) -> f64 {
    // Reference waste water rate [gal/day]; power law from ANSI/RESNET 301.
    let ref_w_gpd = 9.8 * n_bedrooms.powf(0.43);

    // Fraction of reference that is "on-demand" (off-fixture cold drain).
    const O_FRAC: f64 = 0.25;
    // Cold drain efficiency (fraction recovered; 0.0 = all wasted).
    const O_CD_EFF: f64 = 0.0;

    let o_w_gpd = ref_w_gpd * O_FRAC * (1.0 - O_CD_EFF);

    let (distribution_factor, p_ratio, wd_eff) = match distribution {
        DistributionSystem::Standard {
            pipe_r_value,
            piping_length_m,
            default_piping_length_m,
        } => {
            let dist_factor = if *pipe_r_value >= 3.0 { 0.9 } else { 1.0 };
            // Default piping length expressed in metres; ratio is dimensionless.
            let actual_m = piping_length_m.unwrap_or(*default_piping_length_m);
            let ratio = if *default_piping_length_m > 0.0 {
                actual_m / default_piping_length_m
            } else {
                1.0
            };
            (dist_factor, ratio, 1.0_f64)
        }
        DistributionSystem::Recirculation {
            pipe_r_value,
            branch_loop_length_m,
        } => {
            let dist_factor = if *pipe_r_value >= 3.0 { 1.0 } else { 1.11 };
            // Default branch loop length = 10 ft ≈ 3.048 m.
            const DEFAULT_BRANCH_M: f64 = 3.048;
            let actual_m = branch_loop_length_m.unwrap_or(DEFAULT_BRANCH_M);
            let ratio = actual_m / DEFAULT_BRANCH_M;
            (dist_factor, ratio, 0.1_f64)
        }
        DistributionSystem::Unknown => (1.0, 1.0, 1.0),
    };

    let s_w_gpd = (ref_w_gpd - ref_w_gpd * O_FRAC) * p_ratio * distribution_factor;
    let mw_gpd = fixture_efficiency.multiplier() * (o_w_gpd + s_w_gpd * wd_eff);
    gal_to_l(mw_gpd) * usage_multiplier
}

/// Calculate combined average daily hot water consumption (fixtures + distribution losses).
///
/// This is the value OCHRE stores as `"Average Water Draw (L/day)"` and uses to scale the
/// fractional draw schedule via [`normalize_draw_profile`].
///
/// # Parameters
/// - `n_bedrooms`: number of bedrooms.
/// - `fixture_efficiency`: low-flow vs standard fixture set.
/// - `usage_multiplier`: from `WaterFixturesUsageMultiplier` in the HPXML extension.
/// - `distribution`: hot water distribution system.
///
/// # Returns
/// Average daily hot water consumption in L/day.
pub fn combined_daily_hot_water_l(
    n_bedrooms: f64,
    fixture_efficiency: FixtureEfficiency,
    usage_multiplier: f64,
    distribution: &DistributionSystem,
) -> f64 {
    fixture_daily_hot_water_l(n_bedrooms, fixture_efficiency, usage_multiplier)
        + distribution_daily_hot_water_l(
            n_bedrooms,
            fixture_efficiency,
            usage_multiplier,
            distribution,
        )
}

/// Simplified version of [`combined_daily_hot_water_l`] using scalar inputs.
///
/// This is the `ansi_resnet_daily_hot_water_L` entry point described in the ticket.
/// Intended for callers who only have the raw ANSI/RESNET 301 scalars (no distribution
/// system details).  Uses [`DistributionSystem::Unknown`] (conservative no-loss default).
///
/// # Parameters
/// - `n_bedrooms`: number of bedrooms (used as proxy for occupancy).
/// - `fixture_efficiency`: low-flow fixture multiplier [0, 1] (1.0 = standard fixtures).
/// - `distribution_loss_factor`: pipe loss multiplier applied to distribution waste (typically
///   1.0-1.2).  A value of `1.0` means no additional loss.
///
/// # Returns
/// Average daily hot water consumption in L/day.
pub fn ansi_resnet_daily_hot_water_l(
    n_bedrooms: f64,
    fixture_efficiency: f64,
    distribution_loss_factor: f64,
) -> f64 {
    // Fixture draw: ANSI/RESNET 301 formula.
    let fixture_ref_gpd = 14.6 + 10.0 * n_bedrooms;
    let fixture_l = gal_to_l(fixture_ref_gpd) * fixture_efficiency;

    // Distribution waste: OCHRE formula, scaled by distribution_loss_factor.
    let ref_w_gpd = 9.8 * n_bedrooms.powf(0.43);
    const O_FRAC: f64 = 0.25;
    const O_CD_EFF: f64 = 0.0;
    let o_w_gpd = ref_w_gpd * O_FRAC * (1.0 - O_CD_EFF);
    let s_w_gpd = (ref_w_gpd - ref_w_gpd * O_FRAC) * distribution_loss_factor;
    let distribution_l = gal_to_l(fixture_efficiency * (o_w_gpd + s_w_gpd));

    fixture_l + distribution_l
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── normalize_draw_profile ────────────────────────────────────────────────

    #[test]
    fn empty_input_returns_empty() {
        assert!(normalize_draw_profile(&[], 100.0).is_empty());
    }

    #[test]
    fn zero_consumption_returns_all_zeros() {
        let result = normalize_draw_profile(&[0.5, 1.0, 0.5], 0.0);
        assert_eq!(result, vec![0.0, 0.0, 0.0]);
    }

    #[test]
    fn negative_consumption_returns_all_zeros() {
        let result = normalize_draw_profile(&[0.5, 1.0], -10.0);
        assert_eq!(result, vec![0.0, 0.0]);
    }

    #[test]
    fn all_zero_fractions_returns_all_zeros() {
        let result = normalize_draw_profile(&[0.0, 0.0, 0.0], 100.0);
        assert_eq!(result, vec![0.0, 0.0, 0.0]);
    }

    #[test]
    fn uniform_fractions_produce_uniform_flow() {
        // When all fractions are equal the output should be uniform.
        let fractions = vec![0.5_f64; 1440]; // 1440 samples at 1-min intervals
        let avg_daily_l = 150.0;
        let result = normalize_draw_profile(&fractions, avg_daily_l);

        // All values should be identical (uniform input → uniform output).
        let first = result[0];
        for v in &result {
            assert!(
                (v - first).abs() < 1e-12,
                "expected uniform output, got spread at {v}"
            );
        }

        // The daily total should match: sum(kg/s) * 60 s/sample * 1 kg/L = sum_L_per_sample
        // With 1440 samples at 1-min intervals: total_l = sum(kg_s) * 60
        let total_l: f64 = result.iter().map(|&v| v * 60.0).sum();
        assert!(
            (total_l - avg_daily_l).abs() < 1e-9,
            "daily total {total_l:.6} L ≠ {avg_daily_l} L"
        );
    }

    #[test]
    fn nonuniform_fractions_preserve_shape() {
        // Fractions should scale proportionally; relative ratios must be preserved.
        let fractions = vec![1.0, 2.0, 3.0, 4.0];
        let result = normalize_draw_profile(&fractions, 200.0);

        // Each pair of adjacent elements should maintain the original ratio.
        for i in 1..fractions.len() {
            let expected_ratio = fractions[i] / fractions[i - 1];
            let actual_ratio = result[i] / result[i - 1];
            assert!(
                (actual_ratio - expected_ratio).abs() < 1e-12,
                "ratio at index {i}: expected {expected_ratio}, got {actual_ratio}"
            );
        }
    }

    #[test]
    fn round_trip_integrates_to_daily_total() {
        // With 1440 one-minute samples, integrating (kg/s * 60 s) must equal avg_daily_l.
        // (density ≈ 1 kg/L so kg/s * 60 s/sample → L/sample)
        let fractions: Vec<f64> = (0..1440).map(|i| (i % 24) as f64 + 1.0).collect();
        let avg_daily_l = 189.3;
        let result = normalize_draw_profile(&fractions, avg_daily_l);

        let total_l: f64 = result.iter().map(|&v| v * 60.0).sum();
        assert!(
            (total_l - avg_daily_l).abs() < 1e-8,
            "round-trip failed: total={total_l:.6} L ≠ {avg_daily_l} L"
        );
    }

    #[test]
    fn known_values_normalize_correctly() {
        // Uniform fractions of 0.5, avg_daily = 60 L/day.
        // annual_mean_l_per_min = 60 / 1440 = 1/24 L/min
        // scale = (1/24) / 0.5 = 1/12
        // result_kg_s = 0.5 * (1/12) / 60 = 0.5 / 720 ≈ 6.944e-4
        let fractions = vec![0.5_f64; 4];
        let result = normalize_draw_profile(&fractions, 60.0);
        let expected = 0.5 * (60.0 / 1440.0 / 0.5) / 60.0;
        for v in &result {
            assert!(
                (v - expected).abs() < 1e-15,
                "got {v:.6e}, expected {expected:.6e}"
            );
        }
    }

    // ── ansi_resnet_daily_hot_water_l ─────────────────────────────────────────

    #[test]
    fn three_bedroom_home_in_expected_range() {
        // 3 bedrooms, standard fixtures, distribution_loss_factor=1.0 (no additional scaling).
        // OCHRE produces ~228 L/day for 3 bedrooms including distribution losses:
        //   fixture: 44.6 gal/day ≈ 169 L/day
        //   distribution waste: ~15.7 gal/day ≈ 59 L/day
        //   total: ~60.3 gal/day ≈ 228 L/day
        let result = ansi_resnet_daily_hot_water_l(3.0, 1.0, 1.0);
        assert!(
            (200.0..=260.0).contains(&result),
            "3-bedroom home: {result:.1} L/day not in [200, 260]"
        );
    }

    #[test]
    fn more_bedrooms_means_more_consumption() {
        let values: Vec<f64> = (1..=5)
            .map(|n| ansi_resnet_daily_hot_water_l(n as f64, 1.0, 1.0))
            .collect();
        for i in 1..values.len() {
            assert!(
                values[i] > values[i - 1],
                "consumption should increase with bedrooms: {} vs {}",
                values[i - 1],
                values[i]
            );
        }
    }

    #[test]
    fn low_flow_fixtures_reduce_consumption() {
        let standard = ansi_resnet_daily_hot_water_l(3.0, 1.0, 1.0);
        let low_flow = ansi_resnet_daily_hot_water_l(3.0, 0.95, 1.0);
        assert!(
            low_flow < standard,
            "low-flow should use less: {low_flow:.2} vs {standard:.2}"
        );
    }

    #[test]
    fn distribution_loss_factor_scales_distribution_component() {
        // Higher distribution_loss_factor → more distribution waste → higher total.
        let no_loss = ansi_resnet_daily_hot_water_l(3.0, 1.0, 1.0);
        let with_loss = ansi_resnet_daily_hot_water_l(3.0, 1.0, 1.2);
        assert!(
            with_loss > no_loss,
            "higher distribution loss should increase consumption: {with_loss:.2} vs {no_loss:.2}"
        );
    }

    // ── fixture_daily_hot_water_l ─────────────────────────────────────────────

    #[test]
    fn fixture_formula_matches_ansi_resnet_301() {
        // Reference: 14.6 + 10.0 * 3 = 44.6 gal/day for 3 bedrooms, standard fixtures.
        let expected_l = 44.6 / GAL_PER_L;
        let result = fixture_daily_hot_water_l(3.0, FixtureEfficiency::Standard, 1.0);
        assert!(
            (result - expected_l).abs() < 1e-6,
            "fixture formula: {result:.4} L ≠ {expected_l:.4} L"
        );
    }

    #[test]
    fn low_flow_multiplier_is_0_95() {
        let standard = fixture_daily_hot_water_l(3.0, FixtureEfficiency::Standard, 1.0);
        let low_flow = fixture_daily_hot_water_l(3.0, FixtureEfficiency::LowFlow, 1.0);
        assert!(
            (low_flow / standard - 0.95).abs() < 1e-12,
            "LowFlow multiplier: {:.4}",
            low_flow / standard
        );
    }

    #[test]
    fn usage_multiplier_scales_output_linearly() {
        let base = fixture_daily_hot_water_l(3.0, FixtureEfficiency::Standard, 1.0);
        let scaled = fixture_daily_hot_water_l(3.0, FixtureEfficiency::Standard, 1.5);
        assert!(
            (scaled / base - 1.5).abs() < 1e-12,
            "usage multiplier scaling: {:.4}",
            scaled / base
        );
    }

    // ── combined_daily_hot_water_l ────────────────────────────────────────────

    #[test]
    fn combined_equals_fixture_plus_distribution() {
        let n = 3.0;
        let eff = FixtureEfficiency::Standard;
        let mult = 1.0;
        let dist = DistributionSystem::Standard {
            pipe_r_value: 0.0,
            piping_length_m: None,
            default_piping_length_m: 30.0,
        };
        let fixture = fixture_daily_hot_water_l(n, eff, mult);
        let distribution = distribution_daily_hot_water_l(n, eff, mult, &dist);
        let combined = combined_daily_hot_water_l(n, eff, mult, &dist);
        assert!(
            (combined - (fixture + distribution)).abs() < 1e-12,
            "combined ≠ fixture + distribution: {combined:.6} vs {:.6}",
            fixture + distribution
        );
    }

    #[test]
    fn standard_distribution_with_insulation_reduces_losses() {
        let n = 3.0;
        let eff = FixtureEfficiency::Standard;
        let mult = 1.0;
        let insulated = DistributionSystem::Standard {
            pipe_r_value: 4.0,
            piping_length_m: None,
            default_piping_length_m: 30.0,
        };
        let uninsulated = DistributionSystem::Standard {
            pipe_r_value: 0.0,
            piping_length_m: None,
            default_piping_length_m: 30.0,
        };
        let dist_insulated = distribution_daily_hot_water_l(n, eff, mult, &insulated);
        let dist_uninsulated = distribution_daily_hot_water_l(n, eff, mult, &uninsulated);
        assert!(
            dist_insulated < dist_uninsulated,
            "insulated pipe should have lower losses: {dist_insulated:.4} vs {dist_uninsulated:.4}"
        );
    }

    #[test]
    fn recirculation_insulated_lower_than_uninsulated() {
        let n = 3.0;
        let eff = FixtureEfficiency::Standard;
        let mult = 1.0;
        let insulated = DistributionSystem::Recirculation {
            pipe_r_value: 4.0,
            branch_loop_length_m: None,
        };
        let uninsulated = DistributionSystem::Recirculation {
            pipe_r_value: 0.0,
            branch_loop_length_m: None,
        };
        let d_ins = distribution_daily_hot_water_l(n, eff, mult, &insulated);
        let d_unins = distribution_daily_hot_water_l(n, eff, mult, &uninsulated);
        assert!(
            d_ins < d_unins,
            "insulated recirculation should have lower losses: {d_ins:.4} vs {d_unins:.4}"
        );
    }
}
