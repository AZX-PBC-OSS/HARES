//! Unit definitions and type aliases built on the `uom` crate.
//!
//! Boundary policy:
//! - Public API boundaries between crates should prefer typed `uom` quantities.
//! - Inner-loop kernels may use raw `f64` for performance/ergonomics when the unit
//!   is explicit in the function name/signature/docs.
//! - This module provides aliases and helpers so boundary conversions stay explicit.

use uom::si::area::{square_foot, square_meter};
use uom::si::f64::{
    Area as UomArea, Energy as UomEnergy, HeatCapacity as UomHeatCapacity,
    HeatTransfer as UomHeatTransfer, Length as UomLength, MassDensity as UomMassDensity,
    MassRate as UomMassRate, Power as UomPower, Pressure as UomPressure,
    SpecificHeatCapacity as UomSpecificHeatCapacity, ThermodynamicTemperature as UomTemperature,
    Velocity as UomVelocity, Volume as UomVolume,
};
use uom::si::heat_transfer::{
    btu_it_per_hour_square_foot_degree_fahrenheit, watt_per_square_meter_kelvin,
};
use uom::si::length::{foot, inch, meter};
use uom::si::mass_density::{kilogram_per_cubic_meter, pound_per_cubic_foot};
use uom::si::pressure::pascal;
use uom::si::specific_heat_capacity::{btu_per_pound_degree_fahrenheit, joule_per_kilogram_kelvin};
use uom::si::thermodynamic_temperature::{degree_celsius, degree_fahrenheit, kelvin};
use uom::si::volume::{cubic_foot, cubic_meter, gallon, liter};

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

// ---------------------------------------------------------------------------
// Typed uom helpers (existing)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Plain f64 conversion functions for ergonomic use at callsites.
// Uses uom internally where possible; manual factors only where uom lacks
// direct support (R-value, conductivity per-inch, BTU/h→W).
// ---------------------------------------------------------------------------

// --- Area ---

#[inline]
pub fn area_ft2_to_m2(ft2: f64) -> f64 {
    UomArea::new::<square_foot>(ft2).get::<square_meter>()
}

#[inline]
pub fn area_m2_to_ft2(m2: f64) -> f64 {
    UomArea::new::<square_meter>(m2).get::<square_foot>()
}

// --- Length ---

#[inline]
pub fn length_ft_to_m(ft: f64) -> f64 {
    UomLength::new::<foot>(ft).get::<meter>()
}

#[inline]
pub fn length_in_to_m(inches: f64) -> f64 {
    UomLength::new::<inch>(inches).get::<meter>()
}

// --- Power (BTU/h ↔ W) ---
// uom's Power module does not include BTU/h, so we use the exact conversion.
// 1 BTU(IT)/h = 0.293_071_07 W (NIST).

const BTU_PER_HOUR_TO_WATT: f64 = 0.293_071_07;

#[inline]
pub fn power_btu_h_to_w(btu_h: f64) -> f64 {
    btu_h * BTU_PER_HOUR_TO_WATT
}

#[inline]
pub fn power_w_to_btu_h(w: f64) -> f64 {
    w / BTU_PER_HOUR_TO_WATT
}

#[inline]
pub fn power_btu_h_to_kbtu_h(btu_h: f64) -> f64 {
    btu_h * 0.001
}

// --- Power (kW ↔ W) ---
// Exact SI prefix conversion. Defined here so no call site writes `* 1_000.0`
// or `/ 1_000.0` inline — the conversion factor lives in one place.
const WATTS_PER_KILOWATT: f64 = 1_000.0;

/// Convert kilowatts to watts.  1 kW = 1000 W (exact SI).
#[inline]
pub fn power_kw_to_w(kw: f64) -> f64 {
    kw * WATTS_PER_KILOWATT
}

/// Convert watts to kilowatts.  1000 W = 1 kW (exact SI).
#[inline]
pub fn power_w_to_kw(w: f64) -> f64 {
    w / WATTS_PER_KILOWATT
}

// --- Energy: therms → kWh ---
// 1 therm = 100,000 BTU (IT). uom converts BTU → joule → kWh.
use uom::si::energy::{btu_it, kilowatt_hour};

/// Convert therms to kilowatt-hours. 1 therm = 100,000 BTU(IT) ≈ 29.3001 kWh.
#[inline]
pub fn energy_therms_to_kwh(therms: f64) -> f64 {
    UomEnergy::new::<btu_it>(therms * 100_000.0).get::<kilowatt_hour>()
}

// --- Thermal resistance (R-value) and transmittance (U-value) ---
// R: hr·ft²·°F/BTU → m²·K/W
// U: BTU/(hr·ft²·°F) → W/(m²·K)
// uom has HeatTransfer (U-value) with the exact unit, so we use that.

#[inline]
pub fn u_value_ip_to_si(u_ip: f64) -> f64 {
    UomHeatTransfer::new::<btu_it_per_hour_square_foot_degree_fahrenheit>(u_ip)
        .get::<watt_per_square_meter_kelvin>()
}

#[inline]
pub fn r_value_ip_to_si(r_ip: f64) -> f64 {
    // R = 1/U, so convert U=1/r_ip from IP to SI, then invert.
    1.0 / u_value_ip_to_si(1.0 / r_ip)
}

// --- Thermal conductivity ---
// BTU/(hr·ft·°F) → W/(m·K):  exact factor derived from BTU(IT) definition.
// uom's ThermalConductivity has no BTU-based units, so manual constant.
const CONDUCTIVITY_BTU_HR_FT_F_TO_W_M_K: f64 = 1.730_734_67;

// BTU·in/(hr·ft²·°F) → W/(m·K):  = above / 12.
const CONDUCTIVITY_BTU_IN_HR_FT2_F_TO_W_M_K: f64 = 0.144_227_91;

#[inline]
pub fn conductivity_btu_h_ft_f_to_w_m_k(k: f64) -> f64 {
    k * CONDUCTIVITY_BTU_HR_FT_F_TO_W_M_K
}

#[inline]
pub fn conductivity_btu_in_h_ft2_f_to_w_m_k(k: f64) -> f64 {
    k * CONDUCTIVITY_BTU_IN_HR_FT2_F_TO_W_M_K
}

// --- Density ---

#[inline]
pub fn density_lb_ft3_to_kg_m3(rho: f64) -> f64 {
    UomMassDensity::new::<pound_per_cubic_foot>(rho).get::<kilogram_per_cubic_meter>()
}

// --- Specific heat ---

#[inline]
pub fn specific_heat_btu_lb_f_to_j_kg_k(cp: f64) -> f64 {
    UomSpecificHeatCapacity::new::<btu_per_pound_degree_fahrenheit>(cp)
        .get::<joule_per_kilogram_kelvin>()
}

// --- Volume ---

#[inline]
pub fn volume_ft3_to_m3(ft3: f64) -> f64 {
    UomVolume::new::<cubic_foot>(ft3).get::<cubic_meter>()
}

#[inline]
pub fn volume_gal_to_m3(gal: f64) -> f64 {
    UomVolume::new::<gallon>(gal).get::<cubic_meter>()
}

#[inline]
pub fn volume_gal_to_l(gal: f64) -> f64 {
    UomVolume::new::<gallon>(gal).get::<liter>()
}

#[inline]
pub fn volume_m3_to_l(m3: f64) -> f64 {
    UomVolume::new::<cubic_meter>(m3).get::<liter>()
}

#[inline]
pub fn volume_l_to_m3(l: f64) -> f64 {
    UomVolume::new::<liter>(l).get::<cubic_meter>()
}

// --- Temperature (plain f64) ---

#[inline]
pub fn temperature_f_to_c(f: f64) -> f64 {
    UomTemperature::new::<degree_fahrenheit>(f).get::<degree_celsius>()
}

#[inline]
pub fn temperature_c_to_f(c: f64) -> f64 {
    UomTemperature::new::<degree_celsius>(c).get::<degree_fahrenheit>()
}

/// Temperature delta conversion: °C range → °F range (multiply by 9/5, no +32 offset).
#[inline]
pub fn temperature_delta_c_to_f(delta_c: f64) -> f64 {
    delta_c * 9.0 / 5.0
}

// --- Composite: BTU/(hr·°F) → W/K ---
// Used for water-heater UA conversions.  1 BTU/(hr·°F) = BTU_PER_HOUR_TO_W * 9/5.
#[inline]
pub fn btu_hr_per_f_to_w_per_k(x: f64) -> f64 {
    x * BTU_PER_HOUR_TO_WATT * 9.0 / 5.0
}

// --- Power → annual energy (W → kWh/year, Btuh → therms/year) ---
// Used for HPXML PlugLoadUnits / PoolHeaterUnits parsing where W and Btuh
// are power units that must be converted to annual energy equivalents.
// NIST: 1 BTU(IT)/h = 0.293_071_07 W.
// 1 therm = 100,000 BTU(IT).
// 1 year = 8760 h (non-leap).

const HOURS_PER_YEAR: f64 = 8760.0;

/// Convert power in watts to annual energy in kWh/year.
/// W * 8760 h/year / 1000 Wh/kWh = kWh/year.
#[inline]
pub fn power_watt_to_kwh_per_year(w: f64) -> f64 {
    w * HOURS_PER_YEAR / 1000.0
}

/// Convert power in BTU(IT)/h to annual energy in therms/year.
/// Btuh * 8760 h/year / 100000 BTU(IT)/therm = therms/year.
#[inline]
pub fn power_btuh_to_therms_per_year(btuh: f64) -> f64 {
    btuh * HOURS_PER_YEAR / 100_000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::approx_eq;

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

    // --- f64 conversion tests ---

    #[test]
    fn area_ft2_m2_matches_old_constant() {
        // Old constant: 0.092_903_04
        approx_eq(area_ft2_to_m2(1.0), 0.092_903_04, 1e-12);
        approx_eq(area_m2_to_ft2(1.0), 10.763_910_416_709_722, 1e-12);
    }

    #[test]
    fn area_round_trip() {
        let original = 150.0;
        approx_eq(area_m2_to_ft2(area_ft2_to_m2(original)), original, 1e-10);
    }

    #[test]
    fn length_ft_to_m_matches() {
        // 1 ft = 0.3048 m exactly
        approx_eq(length_ft_to_m(1.0), 0.3048, 1e-12);
    }

    #[test]
    fn length_in_to_m_matches() {
        // 1 in = 0.0254 m exactly
        approx_eq(length_in_to_m(1.0), 0.0254, 1e-12);
    }

    #[test]
    fn power_btu_h_to_w_matches_old_constant() {
        approx_eq(power_btu_h_to_w(1.0), 0.293_071_07, 1e-12);
    }

    #[test]
    fn power_w_to_btu_h_inverse() {
        approx_eq(power_w_to_btu_h(power_btu_h_to_w(1000.0)), 1000.0, 1e-8);
    }

    #[test]
    fn power_btu_h_to_kbtu_h_is_divide_by_1000() {
        approx_eq(power_btu_h_to_kbtu_h(12345.0), 12.345, 1e-12);
    }

    #[test]
    fn power_kw_to_w_converts() {
        approx_eq(power_kw_to_w(1.0), 1_000.0, 1e-12);
        approx_eq(power_kw_to_w(0.5), 500.0, 1e-12);
        approx_eq(power_kw_to_w(0.0), 0.0, 1e-12);
    }

    #[test]
    fn power_w_to_kw_converts() {
        approx_eq(power_w_to_kw(1_000.0), 1.0, 1e-12);
        approx_eq(power_w_to_kw(500.0), 0.5, 1e-12);
        approx_eq(power_w_to_kw(0.0), 0.0, 1e-12);
    }

    #[test]
    fn power_kw_w_round_trip() {
        let original_kw = 3.75;
        approx_eq(
            power_w_to_kw(power_kw_to_w(original_kw)),
            original_kw,
            1e-12,
        );
    }

    #[test]
    fn r_value_ip_to_si_matches_old_constant() {
        // Old constant: R_HR_FT2_F_BTU_TO_M2_K_W = 0.176_1
        // The uom-derived value should be close but more precise.
        approx_eq(r_value_ip_to_si(1.0), 0.176_1, 2e-4);
    }

    #[test]
    fn u_value_ip_to_si_matches_old_constant() {
        // Old constant: U_BTU_HR_FT2_F_TO_W_M2_K = 5.678
        // The uom value should be close.
        approx_eq(u_value_ip_to_si(1.0), 5.678, 2e-3);
    }

    #[test]
    fn r_u_value_inverse_relationship() {
        // R = 1/U in same unit system.  R(SI) for R(IP)=10 should equal 1/U(SI) for U(IP)=0.1
        let r_si = r_value_ip_to_si(10.0);
        let u_si = u_value_ip_to_si(0.1);
        approx_eq(r_si, 1.0 / u_si, 1e-12);
    }

    #[test]
    fn conductivity_btu_hr_ft_f_matches_old_constant() {
        approx_eq(conductivity_btu_h_ft_f_to_w_m_k(1.0), 1.730_734_67, 1e-12);
    }

    #[test]
    fn conductivity_btu_in_hr_ft2_f_matches_old_constant() {
        approx_eq(
            conductivity_btu_in_h_ft2_f_to_w_m_k(1.0),
            0.144_227_91,
            1e-12,
        );
    }

    #[test]
    fn density_lb_ft3_matches_old_constant() {
        // Old constant: 16.018_463_37
        approx_eq(density_lb_ft3_to_kg_m3(1.0), 16.018_463_37, 1e-5);
    }

    #[test]
    fn specific_heat_btu_lb_f_matches_old_constant() {
        // uom uses the IT BTU → 4183.9987 J/(kg·K), not the thermochemical 4186.8.
        approx_eq(
            specific_heat_btu_lb_f_to_j_kg_k(1.0),
            4_183.998_673_699_118,
            1e-9,
        );
    }

    #[test]
    fn volume_ft3_to_m3_matches() {
        // 1 ft³ ≈ 0.02831685 m³ (uom uses survey/rounded factor)
        approx_eq(volume_ft3_to_m3(1.0), 0.028_316_846_592, 1e-6);
    }

    #[test]
    fn volume_gal_to_m3_matches_old_constant() {
        // Old constant: 0.003_785_411_784.  uom: 0.003_785_412.
        approx_eq(volume_gal_to_m3(1.0), 0.003_785_411_784, 1e-6);
    }

    #[test]
    fn temperature_f_to_c_known_values() {
        approx_eq(temperature_f_to_c(32.0), 0.0, 1e-10);
        approx_eq(temperature_f_to_c(212.0), 100.0, 1e-10);
        approx_eq(temperature_f_to_c(72.0), (72.0 - 32.0) / 1.8, 1e-10);
    }

    #[test]
    fn temperature_c_to_f_known_values() {
        approx_eq(temperature_c_to_f(0.0), 32.0, 1e-10);
        approx_eq(temperature_c_to_f(100.0), 212.0, 1e-10);
    }

    #[test]
    fn temperature_f_c_round_trip() {
        let original = 98.6;
        approx_eq(
            temperature_c_to_f(temperature_f_to_c(original)),
            original,
            1e-10,
        );
    }

    #[test]
    fn btu_hr_per_f_to_w_per_k_matches_old_constant() {
        // Old constant: 0.293_071_07 * 9.0 / 5.0
        let expected = 0.293_071_07 * 9.0 / 5.0;
        approx_eq(btu_hr_per_f_to_w_per_k(1.0), expected, 1e-12);
    }

    #[test]
    fn power_watt_to_kwh_per_year_converts() {
        // 1000 W = 1 kW * 8760 h = 8760 kWh/year
        approx_eq(power_watt_to_kwh_per_year(1000.0), 8760.0, 1e-10);
        // 1 W = 8.76 kWh/year
        approx_eq(power_watt_to_kwh_per_year(1.0), 8.76, 1e-12);
    }

    #[test]
    fn power_btuh_to_therms_per_year_converts() {
        // 100,000 Btuh * 8760 / 100,000 = 8760 therms/year
        approx_eq(power_btuh_to_therms_per_year(100_000.0), 8760.0, 1e-10);
        // 1 Btuh * 8760 / 100000 = 0.08760 therms/year
        approx_eq(power_btuh_to_therms_per_year(1.0), 0.0876, 1e-12);
    }
}
