//! Regression test for ticket #108 — `port_radiant_w` missing from
//! `EnvelopeDiag` and the Python `post_solvers` dict.
//!
//! Demonstrates the gap: `EnvelopeComponentGains` (produced by the thermal
//! solver) carries both `port_sensible_w` and `port_radiant_w`, but the
//! diagnostic wrapper `EnvelopeDiag` only exposes `port_sensible_w`, dropping
//! the radiant component before it can reach the CSV output or the Python
//! `post_solvers` dict.

use hares_core::diagnostics::EnvelopeDiag;
use hares_envelope::EnvelopeComponentGains;

/// `EnvelopeDiag` must have a `port_radiant_w` field that mirrors the
/// `port_radiant_w` tracked in `EnvelopeComponentGains`.  This test will
/// fail to compile until the field is added, confirming the missing-field bug.
#[test]
fn envelope_diag_exposes_port_radiant_w() {
    let diag = EnvelopeDiag {
        port_radiant_w: 300.0,
        ..EnvelopeDiag::default()
    };
    assert_eq!(diag.port_radiant_w, 300.0);
}

/// Verify that the value stored in `EnvelopeComponentGains::port_radiant_w`
/// would be accessible if it were forwarded to `EnvelopeDiag`.  Constructing
/// `EnvelopeComponentGains` with a known radiant value and then asserting the
/// field is present documents the data-flow that the ticket requires.
#[test]
fn envelope_component_gains_has_port_radiant_w() {
    let gains = EnvelopeComponentGains {
        port_sensible_w: 700.0,
        port_radiant_w: 300.0,
        ..EnvelopeComponentGains::default()
    };
    // Verify both sides of the split sum to the expected total.
    assert_eq!(gains.port_sensible_w + gains.port_radiant_w, 1000.0);
}
