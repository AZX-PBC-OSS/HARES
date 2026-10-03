//! Air of the HPXML equipment locations that name space with no modeled
//! thermal zone ("other heated space" and friends).
//!
//! Dry-bulb follows OS-HPXML v1.12.0 `geometry.rb`,
//! `get_temperature_scheduled_space_values`: an indoor/outdoor weighted
//! blend clamped to a location floor. Moisture follows the same weights on
//! the humidity ratio, which conserves water vapour in the blend (ASHRAE
//! Handbook Fundamentals 2021 ch. 1, adiabatic mixing of two moist-air
//! streams), and is held through the floor (heating air leaves its humidity
//! ratio unchanged). OS-HPXML instead averages the relative humidities of the
//! two sources (`waterheater.rb`, `apply_hpwh_loc_temp_rh_sensors`), which
//! does not conserve moisture and applies the result at a dry-bulb neither
//! source had. The wet-bulb is the thermodynamic wet-bulb of that state.

use hares_physics::psychrometrics::wet_bulb_from_humidity_ratio;
use hares_types::{AmbientAirTemps, AmbientLocation};

/// "other heated space" floor, 68 °F.
const OTHER_HEATED_SPACE_FLOOR_C: f64 = 20.0;
/// "other multifamily buffer space" floor, 50 °F.
const OTHER_MULTIFAMILY_BUFFER_SPACE_FLOOR_C: f64 = 10.0;
/// "other non-freezing space" floor, 40 °F.
const OTHER_NON_FREEZING_SPACE_FLOOR_C: f64 = 40.0 / 9.0;

/// Dry-bulb temperature and humidity ratio of one air source.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MoistAir {
    pub temperature_c: f64,
    pub humidity_ratio: f64,
}

/// Indoor weight, outdoor weight and floor of a scheduled-space placement.
fn blend(location: AmbientLocation) -> Option<(f64, f64, Option<f64>)> {
    match location {
        AmbientLocation::OtherHeatedSpace => Some((0.5, 0.5, Some(OTHER_HEATED_SPACE_FLOOR_C))),
        AmbientLocation::OtherMultifamilyBufferSpace => {
            Some((0.5, 0.5, Some(OTHER_MULTIFAMILY_BUFFER_SPACE_FLOOR_C)))
        }
        AmbientLocation::OtherNonFreezingSpace => {
            Some((0.0, 1.0, Some(OTHER_NON_FREEZING_SPACE_FLOOR_C)))
        }
        AmbientLocation::OtherHousingUnit => Some((1.0, 0.0, None)),
        AmbientLocation::OtherExterior => None,
    }
}

/// Air of a scheduled-space placement from the conditioned zone's and the
/// outdoor air. `None` for "other exterior", which is the outdoor air
/// itself, and for a placement with an indoor weight when `conditioned` is
/// `None`.
pub(crate) fn scheduled_space_air(
    location: AmbientLocation,
    conditioned: Option<MoistAir>,
    outdoor: MoistAir,
    pressure_pa: f64,
) -> Option<AmbientAirTemps> {
    let (indoor_weight, outdoor_weight, floor_c) = blend(location)?;
    let mut mixed_c = outdoor_weight * outdoor.temperature_c;
    let mut humidity_ratio = outdoor_weight * outdoor.humidity_ratio;
    if indoor_weight > 0.0 {
        let conditioned = conditioned?;
        mixed_c += indoor_weight * conditioned.temperature_c;
        humidity_ratio += indoor_weight * conditioned.humidity_ratio;
    }
    let dry_bulb_c = floor_c.map_or(mixed_c, |floor_c| mixed_c.max(floor_c));
    Some(AmbientAirTemps {
        dry_bulb_c,
        wet_bulb_c: wet_bulb_from_humidity_ratio(dry_bulb_c, humidity_ratio, pressure_pa),
    })
}

#[cfg(test)]
mod tests {
    use hares_physics::psychrometrics::{humidity_ratio_from_tdp, wet_bulb_from_humidity_ratio};
    use hares_types::AmbientLocation;

    use super::{MoistAir, scheduled_space_air};

    const P_PA: f64 = 101_325.0;

    fn air(temperature_c: f64, humidity_ratio: f64) -> MoistAir {
        MoistAir {
            temperature_c,
            humidity_ratio,
        }
    }

    /// Dry-bulb per the OS-HPXML table: blend, then the location floor.
    #[test]
    fn dry_bulb_follows_the_scheduled_space_table() {
        let conditioned = air(22.0, 0.008);
        let cold = air(2.0, 0.003);
        let hot = air(30.0, 0.012);
        let dry = |location, outdoor| {
            scheduled_space_air(location, Some(conditioned), outdoor, P_PA)
                .unwrap()
                .dry_bulb_c
        };
        // max(0.5 x 22 + 0.5 x 2, 20) = 20; max(26, 20) = 26.
        assert!((dry(AmbientLocation::OtherHeatedSpace, cold) - 20.0).abs() < 1e-12);
        assert!((dry(AmbientLocation::OtherHeatedSpace, hot) - 26.0).abs() < 1e-12);
        // max(12, 10) = 12.
        assert!((dry(AmbientLocation::OtherMultifamilyBufferSpace, cold) - 12.0).abs() < 1e-12);
        // max(2, 40 °F) = 4.444 °C.
        assert!((dry(AmbientLocation::OtherNonFreezingSpace, cold) - 40.0 / 9.0).abs() < 1e-12);
        // The conditioned zone itself.
        assert!((dry(AmbientLocation::OtherHousingUnit, cold) - 22.0).abs() < 1e-12);
        assert!(
            scheduled_space_air(
                AmbientLocation::OtherExterior,
                Some(conditioned),
                cold,
                P_PA
            )
            .is_none()
        );
    }

    /// The blended placement's moisture is the weighted humidity ratio,
    /// held through the floor: its wet-bulb is the wet-bulb of that state,
    /// below the dry-bulb in unsaturated air.
    #[test]
    fn wet_bulb_comes_from_the_blended_humidity_ratio() {
        let conditioned = air(22.0, humidity_ratio_from_tdp(10.0, P_PA));
        let outdoor = air(30.0, humidity_ratio_from_tdp(20.0, P_PA));
        let heated = scheduled_space_air(
            AmbientLocation::OtherHeatedSpace,
            Some(conditioned),
            outdoor,
            P_PA,
        )
        .unwrap();
        let w = 0.5 * conditioned.humidity_ratio + 0.5 * outdoor.humidity_ratio;
        assert!((heated.dry_bulb_c - 26.0).abs() < 1e-12);
        assert_eq!(
            heated.wet_bulb_c,
            wet_bulb_from_humidity_ratio(26.0, w, P_PA)
        );
        assert!(heated.wet_bulb_c < heated.dry_bulb_c - 1.0);
    }

    /// "Other housing unit" is the conditioned zone's own air, wet-bulb
    /// included; "other non-freezing space" carries the outdoor moisture.
    #[test]
    fn single_source_placements_carry_their_source_moisture() {
        let conditioned = air(22.0, 0.008);
        let outdoor = air(-5.0, 0.002);
        let housing = scheduled_space_air(
            AmbientLocation::OtherHousingUnit,
            Some(conditioned),
            outdoor,
            P_PA,
        )
        .unwrap();
        assert_eq!(
            housing.wet_bulb_c,
            wet_bulb_from_humidity_ratio(22.0, 0.008, P_PA)
        );
        let non_freezing = scheduled_space_air(
            AmbientLocation::OtherNonFreezingSpace,
            Some(conditioned),
            outdoor,
            P_PA,
        )
        .unwrap();
        assert_eq!(
            non_freezing.wet_bulb_c,
            wet_bulb_from_humidity_ratio(40.0 / 9.0, 0.002, P_PA)
        );
        // An outdoor-only placement needs no conditioned zone; a blended
        // one has no air without it.
        assert!(
            scheduled_space_air(AmbientLocation::OtherNonFreezingSpace, None, outdoor, P_PA)
                .is_some()
        );
        assert!(
            scheduled_space_air(AmbientLocation::OtherHeatedSpace, None, outdoor, P_PA).is_none()
        );
    }
}
