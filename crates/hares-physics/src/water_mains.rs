//! Water mains temperature model (Burch-Christensen 2007).
//!
//! Reference: Burch, J. and Christensen, C. (2007). "Towards Development of an
//! Algorithm for Mains Water Temperature." Proceedings of the 2007 ASES National
//! Solar Conference. Also described in Hendron et al. (2004), "Development of an
//! Energy Savings Benchmark for All Residential End-Uses," SimBuild 2004.
//!
//! The model is reproduced in EnergyPlus and used by OCHRE as the default for
//! predicting seasonal variation in cold-water supply temperature.

use std::f64::consts::PI;

/// Degrees per day as specified in the Burch-Christensen (2007) paper (0.986).
/// Note: this is not derived from 360/365.25 (≈ 0.9856); the paper uses 0.986
/// directly, matching the value used in OCHRE and EnergyPlus.
const DEG_PER_DAY: f64 = 0.986;

/// Fixed warm-bias offset from annual average outdoor temperature to annual
/// average mains temperature (6 °F converted to Rankine/Fahrenheit difference,
/// then left as-is because the formula operates in °F internally).
const OFFSET_F: f64 = 6.0;

/// Reference annual average temperature used to anchor ratio and lag (44 °F =
/// ~6.67 °C -- the Building America benchmark calibration base).
const T_REF_F: f64 = 44.0;

/// Ratio coefficient -- base value at T_ref.
const RATIO_BASE: f64 = 0.4;

/// Linear sensitivity of ratio to annual average temperature (per °F).
const RATIO_SLOPE: f64 = 0.01;

/// Lag base value at T_ref [days].
const LAG_BASE: f64 = 35.0;

/// Linear sensitivity of lag to annual average temperature (per °F).
const LAG_SLOPE: f64 = 1.0;

/// Degrees-to-radians conversion.
const DEG_TO_RAD: f64 = PI / 180.0;

use crate::units::{temperature_c_to_f, temperature_delta_c_to_f, temperature_f_to_c};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Calculate water mains temperature using the Burch-Christensen (2007) model.
///
/// The model was calibrated in Imperial units. This function accepts SI inputs,
/// performs all arithmetic in °F (matching the original paper and OCHRE), and
/// returns the result in °C.
///
/// # Parameters
///
/// * `t_annual_avg_c` -- Annual average outdoor dry-bulb temperature [°C].
/// * `dt_annual_range_c` -- Full peak-to-peak difference between the hottest and
///   coldest monthly average outdoor temperatures over the year [°C]. The model
///   uses half of this value as the seasonal amplitude.
///
///   **IMPORTANT:** OCHRE's `dt_monthly` parameter is the *half-swing* (i.e.
///   half of this value). Pass `2 * ochre_dt_monthly` here. For US climates
///   `dt_annual_range_c` is typically 15–35 °C.
/// * `day_of_year` -- Day of year (1 = 1 Jan, 365/366 = 31 Dec). Valid range
///   is 1–366; values outside this range are not meaningful.
/// * `hemisphere` -- Hemisphere of the site. Defaults to [`Hemisphere::Northern`]
///   (the hemisphere the model was calibrated for).
///
/// # Valid climate range
///
/// Calibrated for contiguous US climates (annual average −5 °C to 30 °C).
/// Results outside this range are extrapolated and should be treated with
/// caution.
///
/// # Northern-hemisphere assumption
///
/// The model was developed for the continental United States (Northern
/// Hemisphere, the default). The sine phase shift of −90° produces a mains
/// temperature peak in late summer and a trough in late winter, consistent with
/// Northern Hemisphere ground-temperature lag. Pass `hemisphere:
/// Hemisphere::Southern` for Southern Hemisphere sites.
pub fn water_mains_temperature_c(
    t_annual_avg_c: f64,
    dt_annual_range_c: f64,
    day_of_year: u16,
    hemisphere: Hemisphere,
) -> f64 {
    debug_assert!(
        (1..=366).contains(&day_of_year),
        "day_of_year must be in 1..=366, got {day_of_year}"
    );

    let t_avg_f = temperature_c_to_f(t_annual_avg_c);
    let dt_annual_range_f = temperature_delta_c_to_f(dt_annual_range_c);

    // Clamp ratio to [0.0, 1.0] to prevent phase inversion for arctic climates
    // (annual avg < −24 °C) where the unclamped formula goes negative.
    let ratio = (RATIO_BASE + RATIO_SLOPE * (t_avg_f - T_REF_F)).clamp(0.0, 1.0);
    let lag = LAG_BASE - LAG_SLOPE * (t_avg_f - T_REF_F);

    // Hemisphere sign: Northern = −1, Southern = +1 (as per OCHRE)
    let sign: f64 = match hemisphere {
        Hemisphere::Northern => -1.0,
        Hemisphere::Southern => 1.0,
    };

    let day = f64::from(day_of_year);
    let angle_deg = DEG_PER_DAY * (day - 15.0 - lag) + sign * 90.0;
    let amplitude = ratio * (dt_annual_range_f / 2.0);

    let t_mains_f = t_avg_f + OFFSET_F + amplitude * (angle_deg * DEG_TO_RAD).sin();

    temperature_f_to_c(t_mains_f)
}

/// Hemisphere selector for [`water_mains_temperature_c`].
///
/// The default is [`Hemisphere::Northern`], matching the original
/// Burch-Christensen calibration for the continental United States.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Hemisphere {
    #[default]
    Northern,
    Southern,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use crate::test_utils::approx_eq;

    /// Compute mains temperature for all 365 days and return (min, max, mean).
    fn annual_stats(t_avg_c: f64, dt_annual_range_c: f64) -> (f64, f64, f64) {
        let temps: Vec<f64> = (1u16..=365)
            .map(|d| water_mains_temperature_c(t_avg_c, dt_annual_range_c, d, Hemisphere::Northern))
            .collect();
        let min = temps.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = temps.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let mean = temps.iter().sum::<f64>() / temps.len() as f64;
        (min, max, mean)
    }

    /// Day of the annual min/max.
    fn peak_day(t_avg_c: f64, dt_annual_range_c: f64) -> (u16, u16) {
        let temps: Vec<f64> = (1u16..=365)
            .map(|d| water_mains_temperature_c(t_avg_c, dt_annual_range_c, d, Hemisphere::Northern))
            .collect();
        let max_day = temps
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i as u16 + 1)
            .unwrap();
        let min_day = temps
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i as u16 + 1)
            .unwrap();
        (max_day, min_day)
    }

    // --- Valid day_of_year range ---

    /// Documents the valid input range: 1–366 (inclusive, covering leap years).
    #[test]
    fn day_of_year_boundary_values_are_accepted() {
        // day 1 (1 Jan) and day 366 (31 Dec in a leap year) must both succeed
        // without triggering the debug_assert.
        let t = water_mains_temperature_c(12.0, 25.0, 1, Hemisphere::Northern);
        assert!(t.is_finite(), "day 1 should produce a finite result");
        let t = water_mains_temperature_c(12.0, 25.0, 366, Hemisphere::Northern);
        assert!(t.is_finite(), "day 366 should produce a finite result");
    }

    // --- Leap year day 366 ---

    #[test]
    fn leap_year_day_366_produces_finite_reasonable_value() {
        let t = water_mains_temperature_c(12.0, 25.0, 366, Hemisphere::Northern);
        assert!(t.is_finite(), "day 366 should return a finite value");
        // Day 366 is late December; mains should be below annual mean for NH.
        // Annual mean ≈ 12 + 3.33 ≈ 15.3 °C; late Dec should be well below.
        assert!(
            t < 15.0,
            "day 366 mains temp ({t:.2} °C) should be below annual mean (~15 °C)"
        );
    }

    // --- Arctic climate (ratio clamping) ---

    /// Verifies that at −40 °C (arctic, ratio would go negative without clamping)
    /// the function returns a physically plausible, finite result rather than
    /// producing an inverted seasonal curve.
    #[test]
    fn arctic_climate_neg40c_ratio_clamped_to_zero() {
        // At −40 °C, unclamped ratio = 0.4 + 0.01*(−40 °C in °F − 44) = very negative.
        // With clamping ratio = 0.0, amplitude = 0, and the result equals t_avg + offset.
        let t_avg_c = -40.0_f64;
        let expected_mean_c = temperature_f_to_c(temperature_c_to_f(t_avg_c) + OFFSET_F);
        for d in [1u16, 91, 182, 274, 365] {
            let t = water_mains_temperature_c(t_avg_c, 30.0, d, Hemisphere::Northern);
            assert!(t.is_finite(), "arctic day {d} should be finite");
            // With ratio=0 all days return the same flat value.
            approx_eq(t, expected_mean_c, 0.001);
        }
    }

    // --- Seasonality ---

    #[test]
    fn summer_mains_exceeds_winter_mains() {
        // Mid-summer (day 210) vs mid-winter (day 20) for moderate US climate.
        let t_avg = 12.0; // ~54 °F -- typical US mid-latitude
        let dt = 25.0;
        let summer = water_mains_temperature_c(t_avg, dt, 210, Hemisphere::Northern);
        let winter = water_mains_temperature_c(t_avg, dt, 20, Hemisphere::Northern);
        assert!(
            summer > winter,
            "summer mains ({summer:.2}) should exceed winter mains ({winter:.2})"
        );
    }

    // --- Annual mean ≈ T_avg + offset ---

    #[test]
    fn annual_mean_close_to_t_avg_plus_offset_c() {
        // The offset is 6 °F = 6 * 5/9 °C = 10/3 °C ≈ 3.33 °C.
        // The sine term averages to zero over a full year, so the mean should
        // equal t_avg + offset.
        let offset_c = OFFSET_F * 5.0 / 9.0;
        let t_avg = 12.0;
        let (_, _, mean) = annual_stats(t_avg, 25.0);
        // Allow ±0.5 °C for discretisation (365 steps vs continuous integral).
        approx_eq(mean, t_avg + offset_c, 0.5);
    }

    // --- Hot climate range (T_avg = 25 °C ≈ 77 °F, dt_annual_range = 15 °C) ---
    //
    // With ratio ≈ 0.73 and amplitude ≈ 9.9 °F, the mains temperature ranges
    // roughly 23–34 °C (annual average ~28 °C = 25 °C + 6 °F offset).
    #[test]
    fn hot_climate_mains_range_reasonable() {
        let (min, max, mean) = annual_stats(25.0, 15.0);
        // Annual mean should be close to T_avg + 6°F offset ≈ 28.3 °C
        assert!(
            (22.0..=24.0).contains(&min),
            "hot climate min ({min:.2}) should be 22–24 °C"
        );
        assert!(
            (32.0..=36.0).contains(&max),
            "hot climate max ({max:.2}) should be 32–36 °C"
        );
        // The mean should equal T_avg + offset (6°F = 3.33°C → ~28.3°C)
        assert!(
            (27.0..=30.0).contains(&mean),
            "hot climate annual mean ({mean:.2}) should be 27–30 °C"
        );
    }

    // --- Cold climate range (T_avg = 5 °C ≈ 41 °F, dt_annual_range = 25 °C) ---
    //
    // With ratio ≈ 0.37 and amplitude ≈ 8.2 °F, the mains temperature ranges
    // roughly 3–14 °C (annual average ~8.3 °C = 5 °C + 6 °F offset).
    #[test]
    fn cold_climate_mains_range_3_to_15c() {
        let (min, max, _) = annual_stats(5.0, 25.0);
        assert!(min >= 3.0, "cold climate min ({min:.2}) should be >= 3 °C");
        assert!(
            max <= 15.0,
            "cold climate max ({max:.2}) should be <= 15 °C"
        );
    }

    // --- Peak timing ---

    #[test]
    fn peak_mains_temp_occurs_late_summer_day_220_to_260() {
        let (max_day, _) = peak_day(12.0, 25.0);
        assert!(
            (220..=260).contains(&max_day),
            "peak mains temp on day {max_day}, expected 220–260"
        );
    }

    #[test]
    fn min_mains_temp_occurs_late_winter_day_40_to_80() {
        let (_, min_day) = peak_day(12.0, 25.0);
        assert!(
            (40..=80).contains(&min_day),
            "min mains temp on day {min_day}, expected 40–80"
        );
    }

    // --- Cross-validation against OCHRE for Chicago (T_avg ≈ 9.69 °C, ΔT ≈ 28.1 °C) ---
    //
    // OCHRE computes t_mains in °F, then converts. We replicate the exact
    // arithmetic here to verify our implementation matches.
    //
    // Note: OCHRE's `dt_monthly` is the half-swing. The Chicago value of 28.1 °C
    // used here is the full peak-to-peak range (i.e. 2 × OCHRE's dt_monthly).
    #[test]
    fn chicago_tmy2_cross_validation_vs_ochre() {
        // Parameters from EnergyPlus Engineering Reference Chicago-O'Hare TMY2:
        //   Tout_avg = 9.69 °C → 49.44 °F
        //   ΔTout_annual_range = 28.1 °C → 50.58 °F (difference, ×9/5)
        let t_avg_c = 9.69_f64;
        let dt_c = 28.1_f64;

        // Replicate OCHRE formula directly (Northern Hemisphere: sign = -1)
        // t_mains_f = t_avg_f + 6 + ratio * (dt_f / 2) * sin(π/180 * (0.986*(yday-15-lag) - 90))
        let t_avg_f = temperature_c_to_f(t_avg_c);
        let dt_f = temperature_delta_c_to_f(dt_c);
        // Chicago: t_avg_f ≈ 49.44 which is above T_REF_F=44, so ratio > 0 and no clamping.
        let ratio = (RATIO_BASE + RATIO_SLOPE * (t_avg_f - T_REF_F)).clamp(0.0, 1.0);
        let lag = LAG_BASE - LAG_SLOPE * (t_avg_f - T_REF_F);

        for day in [1u16, 91, 182, 274, 365] {
            let ochre_f = t_avg_f
                + OFFSET_F
                + ratio
                    * (dt_f / 2.0)
                    * ((DEG_TO_RAD * (DEG_PER_DAY * (f64::from(day) - 15.0 - lag) - 90.0)).sin());
            let ochre_c = temperature_f_to_c(ochre_f);

            let ours = water_mains_temperature_c(t_avg_c, dt_c, day, Hemisphere::Northern);
            approx_eq(ours, ochre_c, 0.001);
        }
    }

    // --- Edge cases ---

    #[test]
    fn very_hot_climate_35c_produces_finite_result() {
        for d in [1u16, 100, 200, 300, 365] {
            let t = water_mains_temperature_c(35.0, 10.0, d, Hemisphere::Northern);
            assert!(
                t.is_finite(),
                "expected finite result for hot climate day {d}"
            );
            assert!(
                t > 20.0,
                "very hot climate mains ({t:.2}) should be > 20 °C"
            );
        }
    }

    #[test]
    fn very_cold_climate_neg5c_produces_finite_result() {
        for d in [1u16, 100, 200, 300, 365] {
            let t = water_mains_temperature_c(-5.0, 30.0, d, Hemisphere::Northern);
            assert!(
                t.is_finite(),
                "expected finite result for cold climate day {d}"
            );
        }
    }

    #[test]
    fn southern_hemisphere_peaks_in_northern_winter() {
        // In Southern Hemisphere summer = Northern winter, so mains peak should
        // be in the first quarter of the year (days ~20–100) when sign = +1.
        let temps: Vec<f64> = (1u16..=365)
            .map(|d| water_mains_temperature_c(15.0, 20.0, d, Hemisphere::Southern))
            .collect();
        let max_day = temps
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i as u16 + 1)
            .unwrap();
        // Northern Hemisphere peak is ~day 220–260. With sign flipped (+1 vs −1)
        // the phase shifts by ~180°, placing the Southern peak around day 40–60.
        assert!(
            (20..=100).contains(&max_day),
            "Southern Hemisphere peak on day {max_day}, expected ~day 20–100"
        );
    }

    #[test]
    fn unit_conversion_round_trips() {
        for t in [-20.0_f64, 0.0, 15.0, 35.0, 50.0] {
            let round = temperature_f_to_c(temperature_c_to_f(t));
            approx_eq(round, t, 1e-10);
        }
    }
}
