//! Unit definitions and type aliases built on the `uom` crate.
//!
//! Boundary policy:
//! - Public API boundaries between crates should prefer typed `uom` quantities.
//! - Inner-loop kernels may use raw `f64` for performance/ergonomics when the unit
//!   is explicit in the function name/signature/docs.
//! - This module provides aliases and helpers so boundary conversions stay explicit.

use uom::si::f64::{
    Area as UomArea, Energy as UomEnergy, HeatCapacity as UomHeatCapacity, Length as UomLength,
    MassRate as UomMassRate, Power as UomPower, Pressure as UomPressure,
    ThermodynamicTemperature as UomTemperature, Velocity as UomVelocity, Volume as UomVolume,
};
use uom::si::pressure::pascal;
use uom::si::thermodynamic_temperature::{degree_celsius, kelvin};

pub type Temperature = UomTemperature;
pub type Power = UomPower;
pub type Energy = UomEnergy;
pub type Pressure = UomPressure;
pub type MassFlowRate = UomMassRate;
pub type Length = UomLength;
pub type Area = UomArea;
pub type Volume = UomVolume;
pub type Velocity = UomVelocity;
pub type HeatCapacity = UomHeatCapacity;

pub fn temperature_from_celsius(value_c: f64) -> Temperature {
    Temperature::new::<degree_celsius>(value_c)
}

pub fn temperature_to_celsius(value: Temperature) -> f64 {
    value.get::<degree_celsius>()
}

pub fn temperature_from_kelvin(value_k: f64) -> Temperature {
    Temperature::new::<kelvin>(value_k)
}

pub fn temperature_to_kelvin(value: Temperature) -> f64 {
    value.get::<kelvin>()
}

pub fn pressure_from_pascal(value_pa: f64) -> Pressure {
    Pressure::new::<pascal>(value_pa)
}

pub fn pressure_to_pascal(value: Pressure) -> f64 {
    value.get::<pascal>()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(actual: f64, expected: f64, tol: f64) {
        assert!(
            (actual - expected).abs() <= tol,
            "actual={actual}, expected={expected}, tol={tol}"
        );
    }

    #[test]
    fn celsius_and_kelvin_helpers_are_consistent() {
        let t_c = temperature_from_celsius(25.0);
        approx_eq(temperature_to_celsius(t_c), 25.0, 1e-12);
        approx_eq(temperature_to_kelvin(t_c), 298.15, 1e-10);

        let t_k = temperature_from_kelvin(273.15);
        approx_eq(temperature_to_celsius(t_k), 0.0, 1e-10);
    }

    #[test]
    fn pressure_helpers_round_trip() {
        let p = pressure_from_pascal(101_325.0);
        approx_eq(pressure_to_pascal(p), 101_325.0, 1e-9);
    }
}
