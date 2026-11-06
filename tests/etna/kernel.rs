//! The Section 13 empirical-validation comparison kernel.
//!
//! The kernel compares a software tool's hourly integrated heater energy
//! against the measured best steady-state window, the way the standard's
//! own analysis bundle does:
//!
//! 1. The uncertainty-band comparison (the addendum's Sections
//!    13.3.2.4.2 through 13.3.7.4.2 informative notes): the modeled
//!    hourly output during the best steady-state period is expected to
//!    fall within the averaged measured Qhtr ± u(Qhtr), with u(Qhtr)
//!    the 2-sigma (95% confidence) measurement uncertainty as a
//!    fraction of the measured average.
//! 2. The CV(RMSE) acceptance check (the archive's
//!    `ET_AccCrit_processor.py`, mirroring the informative acceptance
//!    criteria): CV(RMSE) = sqrt(mean((sim_i − mean_meas)²)) / mean_meas
//!    over the best steady-state hours, passing when it does not exceed
//!    the case's threshold, with the B12.2 rounding tolerance of
//!    max(1% of the threshold, 0.01%).
//!
//! Every simulated hour is compared against the MEASURED AVERAGE, not
//! the hourly measurement: the archive's own `calc_cvrmse` docstring
//! states the reason: intra-period measurement fluctuations are within
//! the uncertainty band and are not part of the acceptance signal.

/// The measured best steady-state window for one case.
#[derive(Debug, Clone)]
pub struct MeasuredWindow {
    pub case_id: String,
    /// The window's hourly integrated measured heater energy [Wh].
    pub hourly_qhtr_wh: Vec<f64>,
    /// The measured average over the window [Wh]; the file's own
    /// `Qhtr_avg` summary column.
    pub avg_wh: f64,
    /// The propagated lower uncertainty bound of the average [Wh].
    pub lower_wh: f64,
    /// The propagated upper uncertainty bound of the average [Wh].
    pub upper_wh: f64,
}

/// The verdict of one case's Qhtr comparison, with the numbers the
/// report names.
#[derive(Debug, Clone, PartialEq)]
pub struct QhtrComparison {
    pub case_id: String,
    /// Modeled hourly average over the window [Wh].
    pub modeled_avg_wh: f64,
    /// Measured hourly average over the window [Wh].
    pub measured_avg_wh: f64,
    /// The measured average's ± band half-width from the case's
    /// u(Qhtr) fraction [Wh].
    pub band_half_width_wh: f64,
    /// Whether the modeled average falls inside measured ± u(Qhtr).
    pub modeled_avg_in_band: bool,
    /// CV(RMSE) of the modeled hours against the measured average,
    /// as a fraction of the measured average.
    pub cvrmse: f64,
    /// The case's CV(RMSE) acceptance threshold (fraction).
    pub acccrit_max: f64,
    /// Whether the CV(RMSE) acceptance criterion passes, with the
    /// B12.2 rounding tolerance.
    pub acccrit_pass: bool,
}

impl std::fmt::Display for QhtrComparison {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let status = if self.modeled_avg_in_band && self.acccrit_pass {
            "PASS"
        } else {
            "FAIL"
        };
        write!(
            f,
            "{} {status} modeled_avg={:.6} Wh measured_avg={:.6} Wh \
             u_band=+-{:.6} Wh in_band={} acccrit_max={:.4} \
             cvrmse={:.6}",
            self.case_id,
            self.modeled_avg_wh,
            self.measured_avg_wh,
            self.band_half_width_wh,
            self.modeled_avg_in_band,
            self.acccrit_max,
            self.cvrmse,
        )
    }
}

/// CV(RMSE) of the simulated hours against the measured average, as a
/// fraction of the measured average: the archive's `calc_cvrmse` form,
/// `sqrt(mean((sim_i − mean_meas)²)) / mean_meas`.
///
/// A non-finite input or a non-positive measured average is a typed
/// error naming the case: the comparison cannot run on degenerate data.
pub fn cvrmse(case_id: &str, simulated_wh: &[f64], measured_avg_wh: f64) -> Result<f64, String> {
    if simulated_wh.is_empty() {
        return Err(format!(
            "case {case_id}: the simulated Qhtr series is empty; the \
             comparison needs the best steady-state window's hours"
        ));
    }
    if !measured_avg_wh.is_finite() || measured_avg_wh <= 0.0 {
        return Err(format!(
            "case {case_id}: the measured average Qhtr must be finite and \
             positive, got {measured_avg_wh}"
        ));
    }
    for (hour, value) in simulated_wh.iter().enumerate() {
        if !value.is_finite() {
            return Err(format!(
                "case {case_id}: the simulated Qhtr is non-finite at window \
                 hour {hour}"
            ));
        }
    }
    let sum_sq = simulated_wh
        .iter()
        .map(|s| {
            let d = s - measured_avg_wh;
            d * d
        })
        .sum::<f64>();
    Ok((sum_sq / simulated_wh.len() as f64).sqrt() / measured_avg_wh)
}

/// The B12.2 rounding tolerance the archive's acceptance script applies:
/// a CV above the threshold still passes when it sits within
/// max(1% of the threshold, 0.01%) of it.
pub fn acccrit_tolerance(threshold: f64) -> f64 {
    (0.01 * threshold).max(1e-4)
}

/// Whether a CV(RMSE) value passes the case's acceptance threshold,
/// with the B12.2 rounding tolerance.
pub fn passes_acccrit(cv: f64, threshold: f64) -> bool {
    if cv <= threshold {
        return true;
    }
    (cv - threshold) <= acccrit_tolerance(threshold)
}

/// Whether `value` sits inside the measured average ± the u(Qhtr)
/// half-width (the bounds inclusive: a value on the bound is within the
/// measurement's uncertainty).
pub fn within_uncertainty_band(value: f64, measured_avg_wh: f64, u_rel: f64) -> bool {
    let half = measured_avg_wh * u_rel;
    (measured_avg_wh - half..=measured_avg_wh + half).contains(&value)
}

/// The full Qhtr comparison for one case: the modeled hourly series
/// against the measured window, with both the uncertainty-band and the
/// CV(RMSE) acceptance verdicts.
pub fn compare_qhtr(
    case_id: &str,
    simulated_wh: &[f64],
    measured: &MeasuredWindow,
    u_rel: f64,
    acccrit_max: f64,
) -> Result<QhtrComparison, String> {
    if simulated_wh.len() != measured.hourly_qhtr_wh.len() {
        return Err(format!(
            "case {case_id}: the simulated window has {} hours against the \
             measured window's {}; the comparison aligns the best \
             steady-state hours exactly",
            simulated_wh.len(),
            measured.hourly_qhtr_wh.len()
        ));
    }
    let cv = cvrmse(case_id, simulated_wh, measured.avg_wh)?;
    let modeled_avg = simulated_wh.iter().sum::<f64>() / simulated_wh.len() as f64;
    let half = measured.avg_wh * u_rel;
    Ok(QhtrComparison {
        case_id: case_id.to_string(),
        modeled_avg_wh: modeled_avg,
        measured_avg_wh: measured.avg_wh,
        band_half_width_wh: half,
        modeled_avg_in_band: within_uncertainty_band(modeled_avg, measured.avg_wh, u_rel),
        cvrmse: cv,
        acccrit_max,
        acccrit_pass: passes_acccrit(cv, acccrit_max),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // The hand-computed references: sim [100, 200] against measured
    // average 150. Deviations [−50, +50]; mean square (2500 + 2500) / 2
    // = 2500; RMSE 50; CV = 50 / 150 = 1/3.
    const HAND_COMPUTED_CV: f64 = 1.0 / 3.0;

    #[test]
    fn cvrmse_matches_the_hand_computed_reference() {
        let cv = cvrmse("hand-example", &[100.0, 200.0], 150.0)
            .expect("the hand example's inputs are finite and positive");
        assert!(
            (cv - HAND_COMPUTED_CV).abs() < 1e-12,
            "CV(RMSE) of [100, 200] against the measured average 150 must be \
             1/3, got {cv}"
        );
    }

    #[test]
    fn cvrmse_of_the_constant_series_at_the_measured_average_is_zero() {
        let cv = cvrmse("hand-example", &[150.0, 150.0, 150.0], 150.0)
            .expect("constant-series inputs are finite");
        assert_eq!(
            cv, 0.0,
            "a series identical to the measured average has \
             zero root-mean-square deviation"
        );
    }

    #[test]
    fn cvrmse_rejects_degenerate_inputs_by_typed_error() {
        let empty = cvrmse("hand-example", &[], 150.0);
        assert!(
            empty.is_err(),
            "an empty simulated window cannot produce a comparison"
        );
        let zero_mean = cvrmse("hand-example", &[1.0], 0.0);
        assert!(
            zero_mean.is_err(),
            "a zero measured average cannot normalize the CV"
        );
        let nan_hour = cvrmse("hand-example", &[1.0, f64::NAN], 150.0);
        assert!(
            nan_hour.is_err(),
            "a non-finite simulated hour cannot enter the comparison"
        );
    }

    // The band examples, hand-computed: measured average 1000 Wh with
    // u(Qhtr) = 0.9% gives the ±9 Wh band [991, 1009].
    #[test]
    fn uncertainty_band_examples_are_inside_and_outside_as_hand_computed() {
        assert!(
            within_uncertainty_band(1008.99, 1000.0, 0.009),
            "1008.99 is inside the [991, 1009] band"
        );
        assert!(
            within_uncertainty_band(991.01, 1000.0, 0.009),
            "991.01 is inside the [991, 1009] band"
        );
        assert!(
            !within_uncertainty_band(1009.01, 1000.0, 0.009),
            "1009.01 is outside the [991, 1009] band"
        );
        assert!(
            !within_uncertainty_band(990.99, 1000.0, 0.009),
            "990.99 is outside the [991, 1009] band"
        );
    }

    #[test]
    fn the_uncertainty_band_bounds_are_inclusive() {
        // The bound itself, computed the way the kernel computes it,
        // sits inside: a modeled average equal to the measurement's
        // uncertainty bound is within the measurement's uncertainty.
        let bound = 1000.0 + 1000.0 * 0.009;
        assert!(within_uncertainty_band(bound, 1000.0, 0.009));
    }

    // The acceptance-threshold examples, hand-computed: threshold
    // 0.0099 carries the B12.2 rounding tolerance
    // max(0.01 × 0.0099, 0.0001) = 0.0001, so CVs up to 0.01 pass and
    // anything beyond 0.0100001 fails.
    #[test]
    fn acccrit_tolerance_examples_pass_and_fail_as_hand_computed() {
        assert!(passes_acccrit(0.0099, 0.0099), "CV at the threshold passes");
        assert!(
            passes_acccrit(0.0099 + 0.00009, 0.0099),
            "CV 0.00009 above the threshold sits inside the 0.0001 rounding \
             tolerance"
        );
        assert!(
            !passes_acccrit(0.0099 + 0.00011, 0.0099),
            "CV 0.00011 above the threshold exceeds the 0.0001 rounding \
             tolerance"
        );
        assert!(
            passes_acccrit(0.005, 0.0099),
            "a CV below the threshold passes without the tolerance"
        );
    }

    #[test]
    fn the_tolerance_is_the_b12_2_maximum_of_the_relative_and_absolute_terms() {
        // For a small threshold the absolute 0.0001 term governs; for a
        // large one the 1% relative term does.
        assert!((acccrit_tolerance(0.0099) - 1e-4).abs() < 1e-18);
        assert!((acccrit_tolerance(0.5) - 0.005).abs() < 1e-12);
    }

    // The planted-error witnesses: a comparison kernel that cannot see
    // a biased series, a unit error, or a sign error would validate
    // whatever it is fed. Each planted error is a hand-computed
    // distance from the true value.
    #[test]
    fn the_kernel_flags_a_planted_one_percent_bias() {
        // A +1% multiplicative bias on every modeled hour: the
        // deviations from the measured average grow by at least the
        // bias' own 1% share, so the CV exceeds the 0.99% threshold.
        let honest = [1000.0, 1010.0, 995.0, 1005.0];
        let measured_avg = 1002.5;
        let biased: Vec<f64> = honest.iter().map(|s| s * 1.01).collect();
        let cv_honest = cvrmse("planted", &honest, measured_avg).expect("finite");
        let cv_biased = cvrmse("planted", &biased, measured_avg).expect("finite");
        assert!(
            cv_biased > cv_honest,
            "the biased series' CV must exceed the honest series' CV: \
             {cv_biased} vs {cv_honest}"
        );
        assert!(
            !passes_acccrit(cv_biased, 0.0099),
            "the planted 1% bias must fail the 0.99% acceptance threshold"
        );
    }

    #[test]
    fn the_kernel_flags_a_planted_unit_error() {
        // The same series reported in kWh against a Wh measured average
        // lands three orders off: the CV approaches 1.
        let series_wh = [1000.0, 1010.0, 995.0, 1005.0];
        let measured_avg = 1002.5;
        let cv_kwh = cvrmse("planted", &series_wh, measured_avg / 1000.0).expect("finite");
        assert!(
            cv_kwh > 0.9,
            "the unit error's CV approaches 1, got {cv_kwh}"
        );
        assert!(!passes_acccrit(cv_kwh, 0.0099));
    }

    #[test]
    fn the_kernel_flags_a_planted_sign_error() {
        // Heat delivered reported as heat extracted: the deviations
        // double, the CV approaches 2.
        let series = [1000.0, 1010.0, 995.0, 1005.0];
        let measured_avg = 1002.5;
        let flipped: Vec<f64> = series.iter().map(|s| -s).collect();
        let cv = cvrmse("planted", &flipped, measured_avg).expect("finite");
        assert!(cv > 1.9, "the sign error's CV approaches 2, got {cv}");
        assert!(!passes_acccrit(cv, 0.0099));
    }

    #[test]
    fn the_comparison_rejects_misaligned_windows() {
        let measured = MeasuredWindow {
            case_id: "hand-example".into(),
            hourly_qhtr_wh: vec![100.0; 18],
            avg_wh: 100.0,
            lower_wh: 99.0,
            upper_wh: 101.0,
        };
        let misaligned = vec![100.0; 17];
        let err = compare_qhtr("hand-example", &misaligned, &measured, 0.009, 0.0099)
            .expect_err("a 17-hour window cannot align with an 18-hour window");
        assert!(
            err.contains("17") && err.contains("18"),
            "the error must name both windows' hour counts, got: {err}"
        );
    }
}
