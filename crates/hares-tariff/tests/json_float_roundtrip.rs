//! JSON float parsing is exact in every build profile: a value written with
//! serde_json's shortest round-trip form reads back bit-identical. Without
//! the `float_roundtrip` feature the parser is best-effort and a mantissa
//! above 2^53 or an exponent outside plus-or-minus 22 can land one ULP away,
//! so this suite guards the workspace dependency's feature: a build that
//! loses it fails here.

use serde_json::{from_str, to_string};

/// The values the checkpoint, tariff and override surfaces move: mantissas
/// above 2^53, decimal exponents outside plus-or-minus 22, a subnormal, and
/// the extremes, beside the ordinary tariff figures a schedule file carries.
fn round_trip_values() -> Vec<f64> {
    vec![
        0.1 + 0.2,
        std::f64::consts::PI * 1e-30,
        6.022_140_76e23,
        f64::MIN_POSITIVE,
        5e-324,
        f64::MAX,
        -104.673_178,
        39.847_222_222_222_22,
    ]
}

#[test]
fn json_floats_round_trip_bitwise() {
    for value in round_trip_values() {
        let written = to_string(&value).expect("a finite f64 always serialises");
        let read_back: f64 = from_str(&written).expect("the written form always parses");
        assert_eq!(
            value.to_bits(),
            read_back.to_bits(),
            "the f64 {written} parsed back one or more ULP off the value written"
        );
    }
}
