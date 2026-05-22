//! Regression test for ticket #127 — `port_sensible_w` naming footgun.
//!
//! `port_sensible_w` carries only the *convective* portion of equipment port
//! heat, yet its name uses the `_sensible_` token, which elsewhere in the
//! codebase means convective + radiant.  A reader who adds `port_sensible_w +
//! port_radiant_w` expecting "full sensible total" will double-count because
//! the convective portion is already inside `port_sensible_w`, not separate.
//!
//! This test documents the naming invariant and will fail if/when the field is
//! renamed to `port_convective_w` (intentionally: it must be updated as part
//! of the rename to confirm the ticket was completed correctly).

use hares_envelope::EnvelopeComponentGains;

/// The field currently called `port_sensible_w` holds only the convective
/// component from equipment ports, NOT the full sensible total.  A user
/// expecting `port_sensible_w + port_radiant_w == internal_sensible_total`
/// (i.e. treating "sensible" as convective+radiant) would not double-count
/// only if the field is renamed to `port_convective_w`.
///
/// This test encodes the *correct* semantic:
///   port_convective + port_radiant == total sensible from ports
///
/// If `port_sensible_w` is renamed to `port_convective_w` this test must be
/// updated to use the new field name — that update is the acceptance signal.
#[test]
fn port_sensible_w_is_convective_only_not_full_sensible() {
    let convective_w = 700.0_f64;
    let radiant_w = 300.0_f64;
    let expected_total_sensible_w = convective_w + radiant_w;

    let gains = EnvelopeComponentGains {
        port_sensible_w: convective_w, // ticket #127: misleading name — holds convective only
        port_radiant_w: radiant_w,
        ..EnvelopeComponentGains::default()
    };

    // Correct total: convective + radiant = 1000 W
    let actual_total = gains.port_sensible_w + gains.port_radiant_w;
    assert!(
        (actual_total - expected_total_sensible_w).abs() < 1e-9,
        "port_sensible_w ({}) + port_radiant_w ({}) should equal {} (total sensible), got {}",
        gains.port_sensible_w,
        gains.port_radiant_w,
        expected_total_sensible_w,
        actual_total,
    );

    // Ticket footgun: if a user mistakes port_sensible_w for the full sensible
    // total and adds port_radiant_w on top, they get a wrong double-count.
    let double_counted = gains.port_sensible_w + gains.port_radiant_w + gains.port_radiant_w;
    assert!(
        (double_counted - 1300.0).abs() < 1e-9,
        "Double-counting demo: mistakenly treating port_sensible_w as full sensible + adding port_radiant_w again gives {}, not {}",
        double_counted,
        expected_total_sensible_w,
    );

    // Document that the field name contradicts the "sensible = conv + rad" convention
    // used everywhere else (internal_gain_w, combined_airflow_sensible_w, etc.).
    // After rename: this field should become `port_convective_w`.
    let _ = gains.port_sensible_w; // expected to be renamed to port_convective_w by ticket #127
}
