//! The thermostat switching band: the temperature width between a
//! thermostat's turn-on and turn-off thresholds. It is the hysteresis of an
//! HVAC thermostat and the deadband of a storage water heater's tank
//! thermostat, and every source of it (equipment configuration and the
//! `deadband_c` of a `ThermalSetpoint` signal) is checked by
//! [`validate_thermostat_band_c`].

use crate::HaresError;

/// Narrowest thermostat switching band [°C] a model accepts.
///
/// A thermostat cannot switch on a temperature difference finer than its
/// sensor resolves. Residential thermostats resolve 0.1 °F to 0.1 °C, and the
/// narrowest differential they offer is 0.5 °F (0.28 °C), so 0.1 °C is below
/// every realizable band while keeping the turn-on and turn-off thresholds
/// distinct. It also keeps the HVAC equivalent battery's energy window
/// (zone capacitance times the band) about thirteen orders of magnitude
/// above `f64::EPSILON` for any physical zone capacitance (0.1 kWh/K and up),
/// so the window can never degenerate numerically.
pub const MIN_THERMOSTAT_BAND_C: f64 = 0.1;

/// Widest thermostat switching band [°C] a model accepts. It covers the
/// widest default any equipment carries, the heat pump water heater's
/// 14.7 °F (8.17 °C) tank deadband.
pub const MAX_THERMOSTAT_BAND_C: f64 = 10.0;

/// Checks one thermostat switching band. `field` names its source in the
/// error (`"hysteresis_c"`, `"ThermalSetpoint deadband_c"`, ...).
///
/// # Errors
///
/// [`HaresError::ThermostatBand`] when `band_c` is non-finite or outside
/// [`MIN_THERMOSTAT_BAND_C`, `MAX_THERMOSTAT_BAND_C`].
pub fn validate_thermostat_band_c(field: &str, band_c: f64) -> Result<(), HaresError> {
    if band_c.is_finite() && (MIN_THERMOSTAT_BAND_C..=MAX_THERMOSTAT_BAND_C).contains(&band_c) {
        Ok(())
    } else {
        Err(HaresError::ThermostatBand {
            field: field.to_string(),
            value_c: band_c,
        })
    }
}

/// The switching band a `ThermalSetpoint` applies: its `deadband_c` when the
/// signal names a setpoint, `None` when it carries no deadband.
///
/// A signal that names no setpoint is the release form, which hands control
/// back to the equipment's own schedule and changes nothing else; it carries
/// no deadband, so a deadband without a named setpoint is rejected rather
/// than dropped.
///
/// # Errors
///
/// [`HaresError::Control`] for a deadband on a signal that names no setpoint,
/// and [`HaresError::ThermostatBand`] for a deadband outside the band range.
pub fn thermal_setpoint_band_c(
    heating_setpoint_c: Option<f64>,
    cooling_setpoint_c: Option<f64>,
    deadband_c: Option<f64>,
) -> Result<Option<f64>, HaresError> {
    let Some(band_c) = deadband_c else {
        return Ok(None);
    };
    if heating_setpoint_c.is_none() && cooling_setpoint_c.is_none() {
        return Err(HaresError::Control(format!(
            "ThermalSetpoint carries deadband_c {band_c} but names no setpoint: the \
             deadband applies only with a named setpoint, and the release form \
             (no setpoint named) carries no deadband"
        )));
    }
    validate_thermostat_band_c("ThermalSetpoint deadband_c", band_c)?;
    Ok(Some(band_c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_range_is_inclusive_and_rejects_everything_outside_it() {
        for ok in [MIN_THERMOSTAT_BAND_C, 1.0, MAX_THERMOSTAT_BAND_C] {
            assert!(
                validate_thermostat_band_c("hysteresis_c", ok).is_ok(),
                "{ok}"
            );
        }
        for bad in [
            0.0,
            -0.0,
            f64::from_bits(1),
            f64::MIN_POSITIVE,
            1e-16,
            0.099,
            10.001,
            -1.0,
            f64::NAN,
            f64::INFINITY,
        ] {
            let err = validate_thermostat_band_c("hysteresis_c", bad).unwrap_err();
            assert!(
                matches!(err, HaresError::ThermostatBand { ref field, .. } if field == "hysteresis_c"),
                "{bad}: {err}"
            );
        }
    }

    #[test]
    fn deadband_without_a_named_setpoint_is_rejected() {
        let err = thermal_setpoint_band_c(None, None, Some(1.0)).unwrap_err();
        assert!(err.to_string().contains("names no setpoint"), "{err}");
        assert_eq!(thermal_setpoint_band_c(None, None, None).unwrap(), None);
    }

    #[test]
    fn named_deadband_is_the_band() {
        assert_eq!(
            thermal_setpoint_band_c(Some(20.0), None, Some(0.5)).unwrap(),
            Some(0.5)
        );
        assert_eq!(
            thermal_setpoint_band_c(None, Some(24.0), None).unwrap(),
            None
        );
        assert!(matches!(
            thermal_setpoint_band_c(None, Some(24.0), Some(1e-17)),
            Err(HaresError::ThermostatBand { .. })
        ));
    }
}
