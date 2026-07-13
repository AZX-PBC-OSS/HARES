//! Shared physical constants with authoritative citations.
//!
//! All values cite their primary source. Downstream crates should import
//! from here rather than defining their own copies.
//!
//! ## Moisture mass balance convention
//!
//! All moisture mass balance computations use the 0°C ASHRAE reference
//! (2,501 kJ/kg). The 20°C reference constant (`LATENT_HEAT_VAPORISATION_J_KG`)
//! is retained for informational comparison only and must NOT be used in any
//! moisture mass balance computation.

use hares_types::FluidType;

// --- Dry Air ---

/// Specific gas constant for dry air [J/(kg·K)].
/// ASHRAE HOF 2021 Ch.1. The ASHRAE value (287.058 J/(kg·K)) uses an
/// effective molar mass of 28.96546 g/mol that differs slightly from NIST
/// CODATA (R/M_air = 287.055, M = 28.9645 g/mol). HARES uses the ASHRAE
/// value for consistency with psychrometric literature.
pub const DRY_AIR_GAS_CONSTANT_J_KG_K: f64 = 287.058;

/// Specific heat of dry air at constant pressure [J/(kg·K)].
/// ASHRAE HOF 2021 Ch.1, Table 2 footnote, valid 0–60°C range.
pub const CP_DRY_AIR_J_KG_K: f64 = 1_006.0;

/// Specific heat of dry air at constant pressure [kJ/(kg·K)].
/// Same source as CP_DRY_AIR_J_KG_K, in kJ for psychrometric formulas.
pub const CP_DRY_AIR_KJ_KG_K: f64 = 1.006;

// --- Water Vapour ---

/// Ratio of molecular masses: M_water / M_dry_air = 18.015268 / 28.96546.
/// ASHRAE HOF 2021 Ch.1, Eq. 20. Often denoted ε in psychrometric literature.
/// The 2021 edition uses the updated dry-air molar mass 28.96546 g/mol
/// (previously 28.9645 g/mol in the 2017 edition).
pub const MOLECULAR_WEIGHT_RATIO_WATER_AIR: f64 = 0.621_945;

/// Inverse ratio: M_dry_air / M_water = 1/ε. Used in moist air density correction
/// per ASHRAE HOF 2021 Ch.1 Eq.28, which publishes the rounded coefficient
/// 1.607858. Compiler-computed as the exact reciprocal of
/// `MOLECULAR_WEIGHT_RATIO_WATER_AIR` (≈ 1.607859) — within 0.00008% of the
/// Eq.28 published round value and within numerical noise for all simulation paths.
pub const HUMIDITY_DENSITY_CORRECTION: f64 = 1.0 / MOLECULAR_WEIGHT_RATIO_WATER_AIR;

/// Latent heat of vaporisation at 0°C [kJ/kg].
///
/// ASHRAE HOF 2021 Ch.1, Table 2 at 0°C. Used in psychrometric enthalpy
/// (Eq.30) where h = cp_da×T + W×(h_fg + cp_v×T). The 0°C reference is
/// standard for moist-air enthalpy calculations across the full operating
/// range (−20 to 60°C).
///
/// OCHRE also uses 2501 kJ/kg (via psychrolib) for enthalpy. No deviation.
pub const LATENT_HEAT_VAPORISATION_0C_KJ_KG: f64 = 2_501.0;
/// Latent heat of vaporisation at 0°C [J/kg] -- matches OCHRE/psychrolib. Use
/// this when computing moisture fluxes that must be consistent with the humidity
/// solver (which also uses the 0°C reference).
pub const LATENT_HEAT_VAPORISATION_0C_J_KG: f64 = 2_501_000.0;

/// Latent heat of vaporisation at ~20°C [J/kg].
///
/// ASHRAE HOF 2021 Ch.1, Table 2 (interpolated at 20°C: 2454 kJ/kg, rounded
/// to 2450 for the conventional zone-balance approximation). Used in moisture
/// balance calculations where indoor conditions cluster near 20°C, giving a
/// more accurate latent load estimate than the 0°C reference value.
///
/// Deviation from OCHRE: OCHRE uses 2454 kJ/kg (psychrolib constant at 20°C)
/// in some paths. The ~0.2% difference is within measurement uncertainty.
pub const LATENT_HEAT_VAPORISATION_J_KG: f64 = 2_450_000.0;

/// Latent heat of sublimation at 0°C [kJ/kg].
/// ASHRAE HOF 2021 Ch.1, Table 2, ice surface.
pub const LATENT_HEAT_SUBLIMATION_KJ_KG: f64 = 2_830.0;

/// Specific heat of water vapour [kJ/(kg·K)].
/// ASHRAE HOF 2021 Ch.1, used in Eq. 30 (enthalpy).
pub const CP_WATER_VAPOUR_KJ_KG_K: f64 = 1.86;

// --- Liquid Water ---

/// Specific heat of liquid water [J/(kg·K)] at ~20–25°C.
///
/// EnergyPlus Engineering Reference uses 4180 J/(kg·K) as the fixed
/// specific heat for all water heating calculations (Psychrometrics.hh,
/// `CPHW` / `CPCW` functions, author Russell D. Taylor, April 1992).
/// This matches the ASHRAE HoF 2021 round value and is adopted here
/// for consistency with EnergyPlus.
///
/// Note: temperature-dependent cp ranges from 4182 J/(kg·K) at 20°C
/// to 4186 J/(kg·K) at 15°C per NIST; the 4180 approximation is
/// standard in building energy simulation.
pub const CP_LIQUID_WATER_J_KG_K: f64 = 4_180.0;

// --- Glycol / Refrigerant ---

/// Specific heat of 50% propylene glycol at 60°C [J/(kg·K)].
///
/// EnergyPlus FluidProperties.cc `DefaultPropGlyCpData`, concentration=0.5 row,
/// temperature index 19 (60°C): 3686 J/(kg·K). The table range at 50%
/// concentration over practical HVAC temperatures is 3_455–3_937 J/(kg·K)
/// (0–125°C). The rounded value 3_800 J/(kg·K) is the mid-range engineering
/// default used for single-zone residential simulation.
///
/// EnergyPlus FluidProperties.cc `DefaultEthGlyCpData`, concentration=0.5 row
/// gives an identical value of 3686 J/(kg·K) at 60°C, confirming the
/// approximation is reasonable for both glycol types.
pub const CP_PROP_GLYCOL_50PCT_J_KG_K: f64 = 3_800.0;

/// Specific heat of R-134a saturated liquid [J/(kg·K)] at typical residential
/// vapour-compression conditions.
///
/// ASHRAE Handbook of Refrigeration 2010, Chapter 30 — Thermophysical Properties
/// of Refrigerants, Table 9 (R-134a saturated properties): cp_liquid ≈ 1_430
/// J/(kg·K) at 30°C, ≈ 1_460 J/(kg·K) at 35°C, ≈ 1_490 J/(kg·K) at 40°C.
/// The rounded value 1_450 J/(kg·K) is the engineering default for the 35°C
/// design point typical of residential heat pump evaporator conditions.
pub const CP_R134A_SAT_LIQUID_J_KG_K: f64 = 1_450.0;

/// Specific heat capacity [J/(kg·K)] for the given working fluid type.
///
/// Sources:
/// - Water: 4_180 J/(kg·K) — ASHRAE HoF 2021 Ch.1; EnergyPlus `CPHW`.
/// - Glycol: 3_800 J/(kg·K) — EnergyPlus FluidProperties.cc
///   `DefaultPropGlyCpData`, 50% concentration at 60°C (3_686 J/(kg·K)
///   rounded to the mid-range engineering default for residential simulation).
/// - Refrigerant: 1_450 J/(kg·K) — ASHRAE Handbook of Refrigeration 2010
///   Ch.30 Table 9, R-134a saturated liquid at 35°C design point.
///
/// Matches the values used by `FluidSolverConfig::default()` in
/// `hares-envelope::fluid_solver` so that equipment supply temperature
/// calculations and fluid-solver energy balances use the same cp.
#[must_use]
#[inline]
pub fn cp_j_kg_k(fluid_type: FluidType) -> f64 {
    match fluid_type {
        FluidType::Water => CP_LIQUID_WATER_J_KG_K,
        FluidType::Glycol => CP_PROP_GLYCOL_50PCT_J_KG_K,
        FluidType::Refrigerant => CP_R134A_SAT_LIQUID_J_KG_K,
    }
}

// --- Atmosphere ---

/// Sea-level standard pressure [Pa]. ISA 1976 / ICAO Doc 7488.
pub const SEA_LEVEL_PRESSURE_PA: f64 = 101_325.0;

/// ISA temperature lapse coefficient [1/m].
/// ISA 1976: p = p0 * (1 - L*h)^E where L = 2.25577e-5.
pub const ISA_LAPSE_COEFFICIENT: f64 = 2.255_77e-5;

/// ISA pressure exponent: E = g₀·M₀ / (R*·L).
///
/// U.S. Standard Atmosphere 1976 (NOAA-S/T 76-1562) Part 1 §1.2.5,
/// tropospheric layer (0–11 000 m):
///   g₀  = 9.80665 m/s²     — standard gravity (exact by definition)
///   M₀  = 0.0289644 kg/mol — mean molar mass of dry air at sea level
///   R*  = 8.31432 J/(mol·K)— USSA 1976 universal gas constant (intentionally
///          ～99.998% of the true value; see USSA 1976 §1.2.3 note)
///   L   = 0.0065 K/m       — tropospheric temperature lapse rate
///
/// → E = 9.80665 × 0.0289644 / (8.31432 × 0.0065) ≈ 5.2558761
///
/// Rounded to 8 significant figures from the full derivation value 5.2558761133….
/// EnergyPlus and ASHRAE HoF use the over-rounded 5.2559 (5 sig figs); HARES
/// stores the higher-precision USSA 1976 value. The practical difference at
/// 1 000 m is ～0.04 Pa — well within numerical noise.
pub const ISA_PRESSURE_EXPONENT: f64 = 5.255_876_1;

/// Minimum humidity ratio used as a floor guard in density calculations.
/// Prevents division by near-zero in moist air density formula.
pub const MIN_HUMIDITY_RATIO_DENSITY: f64 = 1e-5;

// --- Unit Conversions ---

/// Watts per ton of refrigeration. 1 ton = 12,000 BTU/h.
/// ASHRAE definition: 1 ton = 3516.8528... W.
pub const W_PER_TON: f64 = 3_516.852_842_066_667;

/// BTU/h per Watt. NIST exact conversion.
pub const BTU_PER_HR_PER_W: f64 = 3.412_141_633;

/// Cubic feet per minute to cubic meters per second conversion factor.
/// 1 ft^3 = 0.028_316_846_592 m^3 and 1 min = 60 s.
pub const CFM_TO_M3_S: f64 = 0.000_471_947_443_2;

/// Cubic meters per second to cubic feet per minute conversion factor.
pub const CFM_PER_M3_S: f64 = 1.0 / CFM_TO_M3_S;

/// Gas therms/hour to Watts conversion.
/// 1 US therm = 100,000 BTU_IT; 1 BTU_IT = 1055.055_852_62 J (NIST SP 811).
/// Conversion: 100_000 × 1055.055_852_62 / 3600 = 29_307.107_017_222_2 W per therm/hour.
/// Consistent with pint UnitRegistry used by OCHRE.
pub const GAS_THERMS_PER_HOUR_TO_W: f64 = 29_307.107_017_222_2;

/// kJ to J conversion factor.
pub const KJ_TO_J: f64 = 1_000.0;

/// kW to W conversion factor.
pub const KW_TO_W: f64 = 1_000.0;

// --- Time ---

/// Seconds per hour [s/h]. Exact by definition.
pub const SECONDS_PER_HOUR: f64 = 3_600.0;

/// Seconds per day [s/day]. Exact by definition.
pub const SECONDS_PER_DAY: f64 = 86_400.0;

/// Joules per kilowatt-hour [J/kWh]. 1 kWh = 1000 W × 3600 s = 3,600,000 J.
/// Exact by definition.
pub const J_PER_KWH: f64 = 3_600_000.0;

/// Minutes per day [min/day]. Exact by definition.
pub const MINUTES_PER_DAY: f64 = 1_440.0;

/// Hours per year [h/year]. ASHRAE 8760 h = 365 days × 24 h/day.
pub const HOURS_PER_YEAR: f64 = 8_760.0;

/// Boiler auxiliary operating hours per year [h/year].
///
/// ANSI/RESNET/ICC 301-2019 Equation 4.4-5 and the ResStock convention
/// use 2080 h/yr for boiler auxiliary loads (pumps, controls), reflecting
/// heating-season operation rather than year-round continuous duty.
/// OCHRE hvac.rb:1754 `get_default_boiler_eae` applies this same divisor
/// when converting `ElectricAuxiliaryEnergy` (kWh/yr) to watts for boilers.
pub const BOILER_AUXILIARY_HOURS_PER_YEAR: f64 = 2_080.0;

// --- Water Heater Performance Coefficients ---

/// UEF→EF linear regression slope for gas storage water heaters.
///
/// Maguire & Roberts (2020) NREL/TP-5500-68035 — derived from regression
/// analysis of ResStock waterheater.rb. Converts Uniform Energy Factor
/// (UEF, post-2015 DOE test procedure) to Energy Factor (EF, pre-2015
/// procedure) for gas-fired storage water heaters when only UEF is available.
///
/// EF = UEF_TO_EF_GAS_SLOPE × UEF + UEF_TO_EF_GAS_INTERCEPT
pub const UEF_TO_EF_GAS_SLOPE: f64 = 0.9066;

/// UEF→EF linear regression intercept for gas storage water heaters.
///
/// Maguire & Roberts (2020) NREL/TP-5500-68035. Paired with
/// `UEF_TO_EF_GAS_SLOPE` to convert UEF to EF for gas-fired storage
/// water heaters.
pub const UEF_TO_EF_GAS_INTERCEPT: f64 = 0.0711;

// --- Building Materials ---

/// Density of concrete [kg/m³].
///
/// ASHRAE HoF 2021 Ch. 33, Table 1 — Structural Concrete, 144 lb/ft³
/// converted to SI (144 × 16.0185 ≈ 2307 kg/m³). The ASHRAE-recommended
/// design value for heavyweight concrete with stone aggregate is
/// 2400 kg/m³, matching EnergyPlus's HeavyWeightConcrete material default.
pub const CONCRETE_DENSITY_KG_M3: f64 = 2400.0;

/// Specific heat capacity of concrete [J/(kg·K)].
///
/// ASHRAE HoF 2021 Ch. 33, Table 1 — standard value for concrete with
/// stone aggregate. Matches EnergyPlus's HeavyWeightConcrete default.
/// ASHRAE gives 840–920 J/(kg·K); 880 J/(kg·K) is the mid-range
/// engineering default used in residential energy simulation.
pub const CONCRETE_CP_J_KG_K: f64 = 880.0;

// --- Occupant Internal Gains ---

/// Total sensible heat gain per occupant [W/person].
///
/// ASHRAE HoF 2021 Ch.18 Table 1: 75 W sensible for seated adult office work.
/// Split into convective (70%) and radiative (30%) via OCCUPANT_CONVECTIVE_FRACTION and
/// OCCUPANT_RADIATIVE_FRACTION per ASHRAE HoF 2021 Ch.18 Table 1.
pub const OCCUPANT_SENSIBLE_GAIN_W: f64 = 75.0;

/// Latent heat gain per occupant [W/person].
///
/// ASHRAE HoF 2021 Ch.18 Table 1: 55 W latent for seated adult office work.
pub const OCCUPANT_LATENT_GAIN_W: f64 = 55.0;

/// Fraction of occupant sensible gain delivered as longwave radiation to surrounding
/// surfaces [-]. ASHRAE Handbook of Fundamentals 2021, Chapter 18, Table 1 specifies
/// approximately 30% radiative for typical residential occupancy (seated, light activity).
/// Applied to the sensible portion only; latent gain is entirely convective (moisture to air).
pub const OCCUPANT_RADIATIVE_FRACTION: f64 = 0.30;

/// Fraction of occupant sensible gain delivered as convection to the zone air node [-].
/// Complement of OCCUPANT_RADIATIVE_FRACTION: 1.0 − 0.30 = 0.70.
/// OCHRE treats the sensible gain as entirely convective (radiative = 0 by default);
/// this constant overrides that assumption to match ASHRAE HoF 2021 Ch.18 Table 1.
pub const OCCUPANT_CONVECTIVE_FRACTION: f64 = 0.70;

/// Stefan-Boltzmann constant [W/(m²·K⁴)].
/// NIST CODATA 2018: σ = 5.670374419 × 10⁻⁸ W·m⁻²·K⁻⁴.
pub const STEFAN_BOLTZMANN: f64 = 5.670_374_419e-8;

/// Linearised radiative heat transfer coefficient [W/(m²·K)].
///
/// `h_rad = 4·ε·σ·T³` — the first-order Taylor expansion of the
/// Stefan-Boltzmann radiation exchange about the pivot temperature `t_kelvin`.
/// Valid when the temperature difference between the two participating
/// surfaces is small compared to their absolute temperature (ΔT ≪ T).
///
/// ASHRAE HoF 2021 Ch.4 §4.3 "Radiation Heat Transfer"; Incropera et al.
/// *Fundamentals of Heat and Mass Transfer* 7th ed. §1.2.3 Eq.1.9.
///
/// * `emissivity` — surface emissivity [-], typically 0.84 (glass) or 0.90 (opaque)
/// * `t_kelvin`    — linearisation pivot temperature [K]; commonly 293.15 K (20°C)
#[must_use]
#[inline]
pub fn linearised_h_rad(emissivity: f64, t_kelvin: f64) -> f64 {
    4.0 * emissivity * STEFAN_BOLTZMANN * t_kelvin.powi(3)
}

/// Celsius to Kelvin offset [K].
/// ISA 1976 / NIST: T(K) = T(°C) + 273.15.
pub const CELSIUS_TO_KELVIN: f64 = 273.15;

/// Fahrenheit to Celsius offset.
pub const FAHRENHEIT_OFFSET: f64 = 32.0;

/// Fahrenheit to Celsius scale factor.
pub const FAHRENHEIT_SCALE: f64 = 5.0 / 9.0;

/// Density of liquid water as a function of temperature [kg/m³].
///
/// Kell (1975) rational polynomial, valid 0–80°C:
///   ρ(T) = [999.83952 + 16.945176·T − 7.9870401e-3·T²
///           − 46.170461e-6·T³ + 105.56302e-9·T⁴ − 280.54253e-12·T⁵]
///           / (1 + 16.879850e-3·T)
///
/// Reference: Kell, G.S. (1975). "Density, thermal expansivity, and
/// compressibility of liquid water from 0° to 150°C." J. Chem. Eng. Data,
/// 20(1), 97–105. Matches NIST IAPWS tabulated values within ±0.05 kg/m³
/// over 0–80°C.
///
/// `t_celsius`: liquid water temperature in °C; valid range 0–80°C.
pub fn water_density_kg_m3(t_celsius: f64) -> f64 {
    let t = t_celsius;
    let numerator = 999.83952 + 16.945_176 * t - 7.987_040_1e-3 * t * t - 46.170_461e-6 * t * t * t
        + 105.563_02e-9 * t * t * t * t
        - 280.542_53e-12 * t * t * t * t * t;
    let denominator = 1.0 + 16.879_850e-3 * t;
    numerator / denominator
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linearised_h_rad_at_20c_is_physically_reasonable() {
        // At 20°C (293.15 K) with ε=0.9: h_rad ≈ 5.14 W/(m²·K)
        let h = linearised_h_rad(0.90, 293.15);
        assert!(
            (4.0..=7.0).contains(&h),
            "h_rad at 20°C should be ~5 W/(m²·K), got {h:.3}"
        );
    }

    #[test]
    fn linearised_h_rad_increases_with_temperature() {
        let h_cold = linearised_h_rad(0.90, 263.15);
        let h_hot = linearised_h_rad(0.90, 333.15);
        assert!(
            h_hot > h_cold,
            "h_rad should increase with temperature: h_cold={h_cold:.4}, h_hot={h_hot:.4}"
        );
    }

    #[test]
    fn linearised_h_rad_correctness() {
        // NIST CODATA 2018: σ = 5.670374419 × 10⁻⁸ W·m⁻²·K⁻⁴
        let sigma_nist: f64 = 5.670_374_419e-8;
        assert!(
            (STEFAN_BOLTZMANN - sigma_nist).abs() < 1e-18,
            "STEFAN_BOLTZMANN constant ({STEFAN_BOLTZMANN:e}) diverges from NIST CODATA 2018 ({sigma_nist:e})"
        );

        // At ε=0.9, T=293.15 K (20°C): h_rad = 4·0.9·σ·293.15³ ≈ 5.143 W/(m²·K)
        let h_at_20c = linearised_h_rad(0.90, 293.15);
        let expected_at_20c = 4.0 * 0.90 * sigma_nist * 293.15_f64.powi(3);
        assert!(
            (h_at_20c - expected_at_20c).abs() < 1e-6,
            "linearised_h_rad(0.9, 293.15 K) = {h_at_20c:.6}, expected {expected_at_20c:.6}"
        );

        // At ε=0.9, T=295 K: h_rad ≈ 5.241 W/(m²·K)
        let h_at_295k = linearised_h_rad(0.90, 295.0);
        let expected_at_295k = 4.0 * 0.90 * sigma_nist * 295.0_f64.powi(3);
        assert!(
            (h_at_295k - expected_at_295k).abs() < 1e-6,
            "linearised_h_rad(0.9, 295 K) = {h_at_295k:.6}, expected {expected_at_295k:.6}"
        );
        assert!(
            (h_at_295k - 5.241).abs() < 0.001,
            "h_rad at T=295 K, ε=0.9 should be ~5.241 W/(m²·K), got {h_at_295k:.4}"
        );
    }
}
