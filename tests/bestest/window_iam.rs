//! Window incidence-angle-modifier pins against the EnergyPlus angular
//! performance table.
//!
//! The beam transmittance of a window follows T(theta) = T(0) * IAM(theta),
//! where IAM is a degree-4 polynomial in cos(theta) whose coefficients span
//! six named curves (A, BDCD, D, E, F, J) selected by the window's U-factor
//! and SHGC. Sources: EnergyPlus Engineering Reference, Window Calculation
//! Module, Step 7 "Determine Angular Performance"
//! (<https://bigladdersoftware.com/epx/docs/8-9/engineering-reference/window-calculation-module.html>),
//! and OCHRE `ochre/utils/envelope.py` `calculate_plane_irradiance`, which
//! carries the same coefficient table.
//!
//! The pins:
//! 1. The curve-selection table (U-factor, SHGC) -> curve, row by row.
//! 2. The IAM at 40, 60 and 80 degrees for every curve, evaluated here from
//!    the coefficient table pinned below (the table of record), so a
//!    coefficient or an evaluation-order drift in the engine fails here.
//! 3. The boundary behaviour: IAM(0) = 1 exactly, monotone decrease over the
//!    pinned angles, and no transmission at or beyond 90 degrees.

use hares_physics::solar::{GlazingCurve, window_iam};
/// The EnergyPlus normalized transmittance correlations, as
/// [a, b, c, d, e] for
/// `raw(cos) = a*cos^4 + b*cos^3 + c*cos^2 + d*cos + e`
/// (the Engineering Reference Step 7's `T = a*cos(phi)^4 + ... + e` form).
///
/// HARES's `GlazingCurve::coefficients` stores the same six curves in the
/// reverse (ascending-degree) order; this table is the pin the engine's
/// arrays are checked against, so reordering or retyping a coefficient in
/// the engine cannot pass silently.
const EPLUS_CURVE_COEFFICIENTS_A_TO_E: [([f64; 5], [f64; 5]); 6] = [
    // (descending-degree form from the EnergyPlus table, ascending-degree
    // form the engine stores). The engine's arrays are OCHRE's highest-degree
    // first arrays reversed; both spellings pin the same six curves.
    (
        [0.014_7, 1.486, -3.852, 3.355, -0.001_474],
        [-0.001_474, 3.355, -3.852, 1.486, 0.014_7],
    ),
    (
        [0.504_475, 0.047_482_5, -2.289, 2.742_25, -0.001_16],
        [-0.001_16, 2.742_25, -2.289, 0.047_482_5, 0.504_475],
    ),
    (
        [0.346_2, 0.396_3, -2.582, 2.845, -0.000_280_4],
        [-0.000_280_4, 2.845, -2.582, 0.396_3, 0.346_2],
    ),
    (
        [2.883, -5.873, 2.489, 1.51, -0.002_577],
        [-0.002_577, 1.51, 2.489, -5.873, 2.883],
    ),
    (
        [3.025, -6.366, 3.137, 1.213, -0.001_367],
        [-0.001_367, 1.213, 3.137, -6.366, 3.025],
    ),
    (
        [3.744, -8.836, 6.018, 0.084_07, 0.000_482_5],
        [0.000_482_5, 0.084_07, 6.018, -8.836, 3.744],
    ),
];

const CURVES: [GlazingCurve; 6] = [
    GlazingCurve::A,
    GlazingCurve::Bdcd,
    GlazingCurve::D,
    GlazingCurve::E,
    GlazingCurve::F,
    GlazingCurve::J,
];

/// The pinned angles, in degrees: the table's informative mid-range and the
/// two extremes where the curve families separate.
const PINNED_ANGLES_DEG: [f64; 3] = [40.0, 60.0, 80.0];

/// The IAM polynomial from the pinned coefficient table, at theta in radians.
/// The EnergyPlus table spells each curve descending-degree
/// (`a*cos^4 + b*cos^3 + c*cos^2 + d*cos + e`); the evaluation converts to
/// ascending-degree order and applies Horner's method, the same form the
/// engine evaluates with.
fn iam_from_table(theta_rad: f64, curve: GlazingCurve) -> f64 {
    let index = CURVES
        .iter()
        .position(|c| *c == curve)
        .expect("curve index");
    let mut ascending = EPLUS_CURVE_COEFFICIENTS_A_TO_E[index].0;
    ascending.reverse();
    let cos = theta_rad.cos();
    let raw = ascending[0]
        + cos * (ascending[1] + cos * (ascending[2] + cos * (ascending[3] + cos * ascending[4])));
    let normal = ascending.iter().sum::<f64>();
    (raw / normal).clamp(0.0, 1.0)
}

/// The curve-selection table's rows: (U-factor, SHGC, expected curve).
/// EnergyPlus Engineering Reference Step 7 selection matrix; the engine's
/// doc comment carries the same table.
#[test]
fn curve_selection_follows_the_energyplus_table() {
    let rows: [(f64, f64, GlazingCurve); 7] = [
        // Single-pane clear (U > 3.98, SHGC > 0.625).
        (6.0, 0.9, GlazingCurve::A),
        // Single-pane, mid SHGC.
        (6.0, 0.5, GlazingCurve::Bdcd),
        // Single-pane, low SHGC.
        (6.0, 0.2, GlazingCurve::D),
        // Double/triple pane, higher SHGC.
        (3.0, 0.7, GlazingCurve::E),
        // Double-pane, lower SHGC.
        (3.0, 0.4, GlazingCurve::F),
        // Triple-pane or low-e, higher SHGC.
        (1.2, 0.5, GlazingCurve::E),
        // Triple-pane or low-e, low SHGC.
        (1.2, 0.3, GlazingCurve::J),
    ];
    for (u, shgc, expected) in rows {
        assert_eq!(
            GlazingCurve::from_u_shgc(u, shgc),
            expected,
            "curve selection at U = {u}, SHGC = {shgc} must follow the EnergyPlus table"
        );
    }
}

/// The BESTEST Case 600 window (U-factor 3.0, SHGC 0.769) selects curve E:
/// the double-pane higher-SHGC bin. The BESTEST annual cases' beam gains
/// ride this curve's angular response.
#[test]
fn the_bestest_window_selects_curve_e() {
    assert_eq!(
        GlazingCurve::from_u_shgc(3.0, 0.769),
        GlazingCurve::E,
        "the BESTEST window (U 3.0, SHGC 0.769) sits in the 1.56-3.98 / \
         SHGC > 0.525 bin: curve E"
    );
}

/// The IAM at 40, 60 and 80 degrees for every curve matches the polynomial
/// evaluated from the pinned EnergyPlus coefficient table. The comparison is
/// exact to 1e-12: both sides evaluate the same polynomial in the same
/// normalized form, so any engine-side coefficient, ordering or
/// normalization drift past that fails.
#[test]
fn iam_matches_the_energyplus_table_at_40_60_80_degrees() {
    for curve in CURVES {
        for deg in PINNED_ANGLES_DEG {
            let theta = deg.to_radians();
            let engine = window_iam(theta, curve);
            let table = iam_from_table(theta, curve);
            assert!(
                (engine - table).abs() < 1e-12,
                "IAM at {deg} degrees for curve {curve:?}: engine {engine:.12} \
                 vs the pinned EnergyPlus table {table:.12}"
            );
        }
    }
}

/// The boundary behaviour the angular model must keep: 1.0 at normal
/// incidence, strictly decreasing over the pinned angles, and zero at and
/// beyond 90 degrees (no beam transmission through a parallel plane).
///
/// The normal-incidence value is pinned within 1e-12, not bitwise: the
/// engine normalizes by the coefficient sum while the polynomial evaluates
/// by Horner's method, and the two association orders differ by at most an
/// ulp on these coefficients. Anything past 1e-12 is a normalization or
/// coefficient change, not association order.
#[test]
fn iam_boundaries_normal_incidence_monotone_and_grazing() {
    for curve in CURVES {
        assert!(
            (window_iam(0.0, curve) - 1.0).abs() <= 1e-12,
            "curve {curve:?}: the normalized IAM is 1.0 at normal incidence, got {}",
            window_iam(0.0, curve)
        );
        let mut prev = window_iam(0.0, curve);
        for deg in PINNED_ANGLES_DEG {
            let iam = window_iam(deg.to_radians(), curve);
            assert!(
                iam < prev,
                "curve {curve:?}: the IAM must decrease with incidence, \
                 {prev:.6} at the smaller angle then {iam:.6} at {deg} degrees"
            );
            prev = iam;
        }
        assert_eq!(
            window_iam(90.0_f64.to_radians(), curve),
            0.0,
            "curve {curve:?}: no beam transmission at 90 degrees"
        );
        assert_eq!(
            window_iam(100.0_f64.to_radians(), curve),
            0.0,
            "curve {curve:?}: no beam transmission beyond 90 degrees"
        );
    }
}
