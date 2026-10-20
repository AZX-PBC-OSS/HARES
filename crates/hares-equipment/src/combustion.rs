//! Combustion loss destinations shared by fuel-burning equipment.
//!
//! A combustion appliance receives fuel and delivers useful heat to its
//! working fluid. The residual (fuel input minus useful output) leaves the
//! cabinet through exactly two paths: the flue (outdoors) and the jacket or
//! skin (the surrounding space). One helper decides that split so every
//! combustion model routes its loss the same way.

use hares_types::HaresError;

/// Split a combustion unit's residual between the flue and the jacket.
///
/// `flue_fraction` and `skin_fraction` are the two shares of the residual
/// `fuel_input_w - useful_w` and must sum to 1: the residual has no third
/// destination. Returns `(zone_w, outdoor_w)`: the jacket/skin share that
/// heats the surrounding space and the flue share that leaves outdoors.
pub fn combustion_loss_split(
    fuel_input_w: f64,
    useful_w: f64,
    flue_fraction: f64,
    skin_fraction: f64,
) -> Result<(f64, f64), HaresError> {
    for (name, fraction) in [("flue", flue_fraction), ("skin", skin_fraction)] {
        if !fraction.is_finite() || !(0.0..=1.0).contains(&fraction) {
            return Err(HaresError::Equipment(format!(
                "combustion loss split {name} fraction must be in [0.0, 1.0], got {fraction}"
            )));
        }
    }
    if (flue_fraction + skin_fraction - 1.0).abs() > 1e-9 {
        return Err(HaresError::Equipment(format!(
            "combustion loss split fractions must sum to 1 (the residual has no \
             third destination), got flue {flue_fraction} + skin {skin_fraction}"
        )));
    }
    let residual_w = (fuel_input_w - useful_w).max(0.0);
    let zone_w = skin_fraction * residual_w;
    Ok((zone_w, residual_w - zone_w))
}

#[cfg(test)]
mod combustion_tests {
    use super::*;

    fn err_of(result: Result<(f64, f64), HaresError>) -> String {
        match result {
            Err(HaresError::Equipment(message)) => message,
            other => panic!("expected an Equipment error, got {other:?}"),
        }
    }

    #[test]
    fn splits_the_residual_by_the_two_shares() {
        let (zone_w, outdoor_w) = combustion_loss_split(1200.0, 1000.0, 0.9, 0.1).unwrap();
        assert!((zone_w - 20.0).abs() < 1e-9, "got {zone_w}");
        assert!((outdoor_w - 180.0).abs() < 1e-9, "got {outdoor_w}");
    }

    #[test]
    fn a_surplus_of_useful_heat_over_fuel_is_not_a_loss() {
        // Condensing recovery: useful exceeds the fuel input; the residual
        // floors at zero and neither destination receives heat.
        let (zone_w, outdoor_w) = combustion_loss_split(1000.0, 1100.0, 0.9, 0.1).unwrap();
        assert_eq!(zone_w, 0.0);
        assert_eq!(outdoor_w, 0.0);
    }

    #[test]
    fn fractions_must_sum_to_one() {
        let message = err_of(combustion_loss_split(1000.0, 800.0, 0.9, 0.0));
        assert!(
            message.contains("must sum to 1"),
            "the error must name the broken contract, got: {message}"
        );
    }

    #[test]
    fn fractions_outside_the_unit_interval_are_rejected() {
        let message = err_of(combustion_loss_split(1000.0, 800.0, 1.5, -0.5));
        assert!(
            message.contains("must be in [0.0, 1.0]"),
            "the error must name the range, got: {message}"
        );
    }
}
