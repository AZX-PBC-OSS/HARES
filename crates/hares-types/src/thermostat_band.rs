//! The thermostat switching band: the temperature width between a
//! thermostat's turn-on and turn-off thresholds. It is the hysteresis of an
//! HVAC thermostat and the deadband of a storage water heater's tank
//! thermostat. Every source of it (equipment configuration, a checkpoint,
//! and the `deadband_c` of a `ThermalSetpoint` signal) is checked by
//! [`validate_thermostat_band_c`] against the range of the device class that
//! applies it.

use serde::{Deserialize, Serialize};

use crate::HaresError;

/// The kinds of thermostat a band belongs to, each with its own valid range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ThermostatBandClass {
    /// A thermostat that cycles a real unit on and off: an HVAC
    /// thermostat, whose band also spans the equivalent battery's window.
    Cycling,
    /// An ideal controller that holds its setpoint exactly (ASHRAE 140's
    /// ideal thermostat, EnergyPlus ideal loads): it has no switching band
    /// of its own and builds no equivalent battery, so zero is valid.
    Ideal,
    /// A storage water heater's tank thermostat.
    Tank,
}

/// Narrowest band [°C] a cycling or tank thermostat accepts.
///
/// A thermostat cannot switch on a temperature difference finer than its
/// sensor resolves. Residential thermostats resolve 0.1 °F to 0.1 °C, and the
/// narrowest differential they offer is 0.5 °F (0.28 °C), so 0.1 °C is below
/// every realizable band while keeping the turn-on and turn-off thresholds
/// distinct. It also keeps the HVAC equivalent battery's energy window
/// (zone capacitance times the band) about thirteen orders of magnitude
/// above `f64::EPSILON` for any physical zone capacitance (0.1 kWh/K and up).
pub const MIN_THERMOSTAT_BAND_C: f64 = 0.1;

/// Widest band [°C] an HVAC thermostat accepts, cycling or ideal: an
/// order of magnitude above the widest residential differential setting
/// (3 °F, 1.7 °C), so it rejects only a value no HVAC thermostat can hold.
pub const MAX_HVAC_THERMOSTAT_BAND_C: f64 = 10.0;

/// Widest band [°C] a tank thermostat accepts: HPWHsim's product presets
/// (`src/HPWHpresets.cc`, the water heater model CBECC-Res and OS-HPXML
/// use) turn heat sources on 20 to 80 °F below setpoint, and 80 °F is
/// 44.4 °C.
pub const MAX_TANK_THERMOSTAT_BAND_C: f64 = 80.0 * 5.0 / 9.0;

impl ThermostatBandClass {
    /// The inclusive valid range [°C].
    #[must_use]
    pub const fn range_c(self) -> (f64, f64) {
        match self {
            Self::Cycling => (MIN_THERMOSTAT_BAND_C, MAX_HVAC_THERMOSTAT_BAND_C),
            Self::Ideal => (0.0, MAX_HVAC_THERMOSTAT_BAND_C),
            Self::Tank => (MIN_THERMOSTAT_BAND_C, MAX_TANK_THERMOSTAT_BAND_C),
        }
    }

    /// Why a band outside the range is not one this class can hold.
    #[must_use]
    pub fn violation(self, band_c: f64) -> String {
        let (min, max) = self.range_c();
        if !band_c.is_finite() {
            "a band must be a finite temperature difference".to_string()
        } else if band_c < min && min == 0.0 {
            "a band cannot be negative".to_string()
        } else if band_c < min {
            format!(
                "it is below {min} °C, finer than a thermostat's sensor resolves, so the \
                 turn-on and turn-off thresholds would not be distinct"
            )
        } else {
            match self {
                Self::Tank => format!(
                    "it is above {max:.1} °C, wider than the widest tank thermostat preset \
                     (80 °F below setpoint)"
                ),
                Self::Cycling | Self::Ideal => {
                    format!("it is above {max} °C, wider than any HVAC thermostat holds")
                }
            }
        }
    }
}

/// Checks one thermostat band against its class. `field` names the input
/// it came from (`"hysteresis_c"`, `"deadband_c"`, ...).
///
/// # Errors
///
/// [`HaresError::ThermostatBand`] when `band_c` is non-finite or outside the
/// class's range.
pub fn validate_thermostat_band_c(
    class: ThermostatBandClass,
    field: &str,
    band_c: f64,
) -> Result<(), HaresError> {
    let (min, max) = class.range_c();
    if band_c.is_finite() && (min..=max).contains(&band_c) {
        Ok(())
    } else {
        Err(HaresError::ThermostatBand {
            field: field.to_string(),
            value_c: band_c,
            class,
        })
    }
}

/// The band a `ThermalSetpoint` applies on a device of `class`: its
/// `deadband_c` when the signal names a setpoint, `None` when it carries no
/// deadband.
///
/// A signal that names no setpoint is the release form, which hands control
/// back to the equipment's schedule and configured band; it carries no
/// deadband, so a deadband without a named setpoint is rejected rather
/// than dropped.
///
/// # Errors
///
/// [`HaresError::Control`] for a deadband on a signal that names no setpoint,
/// and [`HaresError::ThermostatBand`] for a deadband outside the class.
pub fn thermal_setpoint_band_c(
    class: ThermostatBandClass,
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
    validate_thermostat_band_c(class, "ThermalSetpoint deadband_c", band_c)?;
    Ok(Some(band_c))
}

/// The `ThermalSetpoint` deadband contract a signal is held to before it
/// reaches a device: the release form carries none, and a named deadband
/// lies in the range of some device class. The receiving device holds it
/// to its own class.
///
/// # Errors
///
/// As [`thermal_setpoint_band_c`].
pub fn validate_thermal_setpoint_deadband(
    heating_setpoint_c: Option<f64>,
    cooling_setpoint_c: Option<f64>,
    deadband_c: Option<f64>,
) -> Result<(), HaresError> {
    // The classes together span the ideal range's floor to the tank range's
    // ceiling: a band above the HVAC ceiling can only be a tank's.
    let class = match deadband_c {
        Some(band_c) if band_c > MAX_HVAC_THERMOSTAT_BAND_C => ThermostatBandClass::Tank,
        _ => ThermostatBandClass::Ideal,
    };
    thermal_setpoint_band_c(class, heating_setpoint_c, cooling_setpoint_c, deadband_c).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_class_has_its_range() {
        use ThermostatBandClass::*;
        for (class, ok, bad) in [
            (
                Cycling,
                vec![0.1, 1.0, 10.0],
                vec![0.0, 1e-17, 0.099, 10.001, 44.0],
            ),
            (Ideal, vec![0.0, 1e-17, 10.0], vec![-0.1, -1e-17, 10.001]),
            (Tank, vec![0.1, 11.1, 44.4], vec![0.0, 0.05, 44.5]),
        ] {
            for v in ok {
                assert!(
                    validate_thermostat_band_c(class, "f", v).is_ok(),
                    "{class:?} {v}"
                );
            }
            for v in bad.into_iter().chain([f64::NAN, f64::INFINITY]) {
                let err = validate_thermostat_band_c(class, "f", v).unwrap_err();
                assert!(
                    matches!(err, HaresError::ThermostatBand { ref field, class: c, .. } if field == "f" && c == class),
                    "{class:?} {v}: {err}"
                );
            }
        }
    }

    #[test]
    fn the_error_gives_the_reason_of_the_bound_crossed() {
        let reason = |class, v| {
            validate_thermostat_band_c(class, "hysteresis_c", v)
                .unwrap_err()
                .to_string()
        };
        assert!(reason(ThermostatBandClass::Cycling, 0.0).contains("sensor resolves"));
        assert!(reason(ThermostatBandClass::Cycling, 20.0).contains("HVAC thermostat"));
        assert!(reason(ThermostatBandClass::Tank, 50.0).contains("tank thermostat"));
        assert!(reason(ThermostatBandClass::Ideal, -1.0).contains("negative"));
        assert!(reason(ThermostatBandClass::Cycling, f64::NAN).contains("finite"));
        assert!(reason(ThermostatBandClass::Cycling, 0.0).contains("hysteresis_c"));
    }

    #[test]
    fn deadband_without_a_named_setpoint_is_rejected() {
        let err = thermal_setpoint_band_c(ThermostatBandClass::Cycling, None, None, Some(1.0))
            .unwrap_err();
        assert!(err.to_string().contains("names no setpoint"), "{err}");
        assert!(validate_thermal_setpoint_deadband(None, None, Some(1.0)).is_err());
        assert!(validate_thermal_setpoint_deadband(None, None, None).is_ok());
    }

    #[test]
    fn a_signal_deadband_must_fit_some_class() {
        for ok in [0.0, 0.05, 1.0, 30.0] {
            assert!(
                validate_thermal_setpoint_deadband(Some(20.0), None, Some(ok)).is_ok(),
                "{ok}"
            );
        }
        for bad in [-0.1, 45.0, f64::NAN] {
            assert!(
                validate_thermal_setpoint_deadband(Some(20.0), None, Some(bad)).is_err(),
                "{bad}"
            );
        }
    }
}
