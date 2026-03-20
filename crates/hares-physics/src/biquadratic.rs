//! Biquadratic performance curve evaluation utilities.

/// Evaluate a quadratic polynomial `a + b*x + c*x^2`.
pub fn quadratic(coeffs: &[f64; 3], x: f64) -> f64 {
    coeffs[0] + coeffs[1] * x + coeffs[2] * x * x
}

/// Evaluate a biquadratic polynomial `a + b*x1 + c*x1^2 + d*x2 + e*x2^2 + f*x1*x2`.
pub fn biquadratic(coeffs: &[f64; 6], x1: f64, x2: f64) -> f64 {
    coeffs[0]
        + coeffs[1] * x1
        + coeffs[2] * x1 * x1
        + coeffs[3] * x2
        + coeffs[4] * x2 * x2
        + coeffs[5] * x1 * x2
}

/// Biquadratic curve with input clamping.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BiquadraticCurve {
    pub coeffs: [f64; 6],
    pub x1_bounds: (f64, f64),
    pub x2_bounds: (f64, f64),
}

impl BiquadraticCurve {
    pub fn evaluate(&self, x1: f64, x2: f64) -> f64 {
        let x1_clamped = x1.clamp(self.x1_bounds.0, self.x1_bounds.1);
        let x2_clamped = x2.clamp(self.x2_bounds.0, self.x2_bounds.1);
        biquadratic(&self.coeffs, x1_clamped, x2_clamped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELATIVE_ERROR_TOLERANCE: f64 = 1e-12;
    const DENOMINATOR_FLOOR: f64 = 1e-15;

    fn rel_error(a: f64, b: f64) -> f64 {
        let denom = b.abs().max(DENOMINATOR_FLOOR);
        (a - b).abs() / denom
    }

    #[allow(clippy::too_many_arguments)]
    fn ochre_biquadratic_reference(
        t_in: f64,
        t_ext: f64,
        coeffs_t: [f64; 6],
        ff: f64,
        coeffs_ff: [f64; 3],
        plr: f64,
        coeffs_plr: [f64; 3],
        rated: f64,
        twb_bounds: (f64, f64),
        tdb_bounds: (f64, f64),
        ff_bounds: (f64, f64),
        plf_bounds: (f64, f64),
    ) -> f64 {
        let t_in = t_in.clamp(twb_bounds.0, twb_bounds.1);
        let t_ext = t_ext.clamp(tdb_bounds.0, tdb_bounds.1);
        let ff = ff.clamp(ff_bounds.0, ff_bounds.1);

        let t_ratio = biquadratic(&coeffs_t, t_in, t_ext);
        let ff_ratio = quadratic(&coeffs_ff, ff);
        let plf_ratio = quadratic(&coeffs_plr, plr).clamp(plf_bounds.0, plf_bounds.1);
        rated * t_ratio * ff_ratio / plf_ratio
    }

    #[test]
    fn quadratic_evaluates_expected_polynomial() {
        let coeffs = [2.0, -1.5, 0.25];
        let x = 4.0;
        let expected = 2.0 - 1.5 * x + 0.25 * x * x;
        assert_eq!(quadratic(&coeffs, x), expected);
    }

    #[test]
    fn curve_clamps_both_axes_when_out_of_bounds() {
        let curve = BiquadraticCurve {
            coeffs: [1.0, 0.2, 0.01, -0.1, 0.005, 0.02],
            x1_bounds: (10.0, 20.0),
            x2_bounds: (0.0, 5.0),
        };

        let both_oob = curve.evaluate(40.0, -3.0);
        let boundary_value = curve.evaluate(20.0, 0.0);
        assert_eq!(both_oob, boundary_value);
    }

    #[test]
    fn ahri_210_240_rating_conditions_documented() {
        // AHRI Standard 210/240-2023, Table 1
        // Cooling rated conditions: indoor Twb = 19.44°C (67°F), outdoor Tdb = 35.0°C (95°F)
        // A properly normalized capacity curve should return 1.0 at these conditions.
        // The OCHRE Single_1 curve is NOT normalized to unity at AHRI conditions;
        // it returns ~0.994 because it uses a different reference point.
        let ochre_coeffs = [1.5509, -0.07505, 0.0031, 0.0024, -0.00005, -0.00043];
        let result = biquadratic(&ochre_coeffs, 19.44, 35.0);
        // Analytical: 1.5509 + (-0.07505)*19.44 + 0.0031*19.44² + 0.0024*35.0
        //           + (-0.00005)*35.0² + (-0.00043)*19.44*35.0 ≈ 0.9936
        assert!(
            (result - 0.9936).abs() < 0.01,
            "OCHRE curve at AHRI 210/240 rated conditions: {result}"
        );
    }

    #[test]
    fn ochre_single_speed_ac_capacity_curve_matches_reference() {
        // Coefficients from vendors/OCHRE/defaults/HVAC Cooling/Biquadratic Air Conditioner.csv (Single_1)
        let coeffs_t = [1.5509, -0.07505, 0.0031, 0.0024, -0.00005, -0.00043];
        let coeffs_ff = [0.718605468, 0.41009989, -0.128705457];
        let coeffs_plr = [0.93, 0.07, 0.0];

        // Same clamping convention as OCHRE HVAC._biquadratic.
        let twb_bounds = (13.88, 23.88);
        let tdb_bounds = (18.33, 51.66);
        let ff_bounds = (0.75, 1.25);
        let plf_bounds = (0.7, 1.0);
        let rated = 1.0;

        let samples = [
            (18.0, 22.0, 0.8, 0.6),
            (19.5, 30.0, 1.0, 0.8),
            (24.0, 45.0, 1.3, 1.1),
            (12.0, 60.0, 0.2, 0.3), // clamp on all constrained axes
        ];

        for (t_in, t_ext, ff, plr) in samples {
            let expected = ochre_biquadratic_reference(
                t_in, t_ext, coeffs_t, ff, coeffs_ff, plr, coeffs_plr, rated, twb_bounds,
                tdb_bounds, ff_bounds, plf_bounds,
            );

            let curve = BiquadraticCurve {
                coeffs: coeffs_t,
                x1_bounds: twb_bounds,
                x2_bounds: tdb_bounds,
            };
            let got = rated
                * curve.evaluate(t_in, t_ext)
                * quadratic(&coeffs_ff, ff.clamp(ff_bounds.0, ff_bounds.1))
                / quadratic(&coeffs_plr, plr).clamp(plf_bounds.0, plf_bounds.1);

            assert!(
                rel_error(got, expected) <= RELATIVE_ERROR_TOLERANCE,
                "got={got}, expected={expected}, rel_err={}",
                rel_error(got, expected)
            );
        }
    }
}
