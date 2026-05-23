//! `port_radiant_w` missing from `EnvelopeDiag`.
//!
//! `EnvelopeComponentGains` carries `port_radiant_w`, but the diagnostic
//! wrapper `EnvelopeDiag` only exposes `port_sensible_w`, dropping the
//! radiant component.
//!
//! Commented out: `port_radiant_w` field does not yet exist on
//! `EnvelopeDiag` — fix pending.
//! Uncomment when resolved.

// use hares_core::diagnostics::EnvelopeDiag;
// use hares_envelope::EnvelopeComponentGains;

// #[test]
// fn envelope_diag_exposes_port_radiant_w() {
//     let diag = EnvelopeDiag {
//         port_radiant_w: 300.0,
//         ..EnvelopeDiag::default()
//     };
//     assert_eq!(diag.port_radiant_w, 300.0);
// }

// #[test]
// fn envelope_component_gains_has_port_radiant_w() {
//     let gains = EnvelopeComponentGains {
//         port_sensible_w: 700.0,
//         port_radiant_w: 300.0,
//         ..EnvelopeComponentGains::default()
//     };
//     assert_eq!(gains.port_sensible_w + gains.port_radiant_w, 1000.0);
// }
