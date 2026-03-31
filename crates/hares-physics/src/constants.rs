//! Shared physical constants with authoritative citations.
//!
//! All values cite their primary source. Downstream crates should import
//! from here rather than defining their own copies.

// --- Dry Air ---

/// Specific gas constant for dry air [J/(kg·K)].
/// ASHRAE 2017 Handbook of Fundamentals, Ch. 1.
/// Note: NIST CODATA R/M_air yields 287.055; the ASHRAE value 287.058
/// uses a slightly different effective molar mass. We use the ASHRAE value
/// for consistency with psychrometric literature.
pub const DRY_AIR_GAS_CONSTANT_J_KG_K: f64 = 287.058;

/// Specific heat of dry air at constant pressure [J/(kg·K)].
/// ASHRAE 2017 HOF Ch. 1, Table 2 footnote, valid 0–60°C range.
pub const CP_DRY_AIR_J_KG_K: f64 = 1_006.0;

/// Specific heat of dry air at constant pressure [kJ/(kg·K)].
/// Same source as CP_DRY_AIR_J_KG_K, in kJ for psychrometric formulas.
pub const CP_DRY_AIR_KJ_KG_K: f64 = 1.006;

// --- Water Vapour ---

/// Ratio of molecular masses: M_water / M_dry_air = 18.015268 / 28.96546.
/// ASHRAE 2017 HOF Ch. 1, Eq. 20. Often denoted ε in psychrometric literature.
pub const MOLECULAR_WEIGHT_RATIO_WATER_AIR: f64 = 0.621_945;

/// Inverse ratio: M_dry_air / M_water. Used in moist air density correction.
/// ASHRAE 2017 HOF Ch. 1, Eq. 11.
pub const HUMIDITY_DENSITY_CORRECTION: f64 = 1.607_768_7;

/// Latent heat of vaporisation at 0°C [kJ/kg].
///
/// ASHRAE 2017 HOF Ch.1, Table 2 at 0°C. Used in psychrometric enthalpy
/// (Eq.30) where h = cp_da×T + W×(h_fg + cp_v×T). The 0°C reference is
/// standard for moist-air enthalpy calculations across the full operating
/// range (−20 to 60°C).
///
/// OCHRE also uses 2501 kJ/kg (via psychrolib) for enthalpy. No deviation.
pub const LATENT_HEAT_VAPORISATION_0C_KJ_KG: f64 = 2_501.0;
/// Latent heat of vaporisation at 0°C [J/kg] — matches OCHRE/psychrolib. Use
/// this when computing moisture fluxes that must be consistent with the humidity
/// solver (which also uses the 0°C reference).
pub const LATENT_HEAT_VAPORISATION_0C_J_KG: f64 = 2_501_000.0;

/// Latent heat of vaporisation at ~20°C [J/kg].
///
/// ASHRAE 2017 HOF Ch.1, Table 2 (interpolated at 20°C: 2454 kJ/kg, rounded
/// to 2450 for the conventional zone-balance approximation). Used in moisture
/// balance calculations where indoor conditions cluster near 20°C, giving a
/// more accurate latent load estimate than the 0°C reference value.
///
/// Deviation from OCHRE: OCHRE uses 2454 kJ/kg (psychrolib constant at 20°C)
/// in some paths. The ~0.2% difference is within measurement uncertainty.
pub const LATENT_HEAT_VAPORISATION_J_KG: f64 = 2_450_000.0;

/// Latent heat of sublimation at 0°C [kJ/kg].
/// ASHRAE 2017 HOF Ch. 1, Table 2, ice surface.
pub const LATENT_HEAT_SUBLIMATION_KJ_KG: f64 = 2_830.0;

/// Specific heat of water vapour [kJ/(kg·K)].
/// ASHRAE 2017 HOF Ch. 1, used in Eq. 30 (enthalpy).
pub const CP_WATER_VAPOUR_KJ_KG_K: f64 = 1.86;

// --- Liquid Water ---

/// Specific heat of liquid water at ~20°C [J/(kg·K)].
/// NIST / ASHRAE 2017 HOF Ch. 1.
pub const CP_LIQUID_WATER_J_KG_K: f64 = 4_186.0;

// --- Atmosphere ---

/// Sea-level standard pressure [Pa]. ISA 1976 / ICAO Doc 7488.
pub const SEA_LEVEL_PRESSURE_PA: f64 = 101_325.0;

/// ISA temperature lapse coefficient [1/m].
/// ISA 1976: p = p0 * (1 - L*h)^E where L = 2.25577e-5.
pub const ISA_LAPSE_COEFFICIENT: f64 = 2.255_77e-5;

/// ISA pressure exponent. Derived from g/(R_da * L) = 9.80665 / (287.058 * 0.0065).
/// ISA 1976 / ICAO Doc 7488.
pub const ISA_PRESSURE_EXPONENT: f64 = 5.2559;

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

// --- Occupant Internal Gains ---

/// Sensible heat gain per occupant delivered to zone air [W/person].
///
/// OCHRE Envelope.py:904-907: total gain = 400 BTU/h per person; sensible fraction = 0.563
/// (convective only — radiative fraction is 0 by default in OCHRE residential model).
/// 400 BTU/h × (1055.055_852_62 J / BTU) / 3600 s = 117.228 W; × 0.563 ≈ 66.0 W.
/// Value kept as the OCHRE-matched rounded constant.
pub const OCCUPANT_SENSIBLE_GAIN_W: f64 = 66.0;

/// Latent heat gain per occupant [W/person].
///
/// OCHRE Envelope.py:908: latent fraction = 0.437 of 400 BTU/h total.
/// 117.228 W × 0.437 ≈ 51.2 W.
pub const OCCUPANT_LATENT_GAIN_W: f64 = 51.2;

/// Fraction of occupant sensible gain delivered as convection to the zone air node [-].
/// Not a separate multiplier in OCHRE — the OCHRE sensible gain already represents
/// the convective component only (radiative = 0 by default). Kept for documentation.
pub const OCCUPANT_CONVECTIVE_FRACTION: f64 = 1.0;

/// Celsius to Kelvin offset [K].
/// ISA 1976 / NIST: T(K) = T(°C) + 273.15.
pub const CELSIUS_TO_KELVIN: f64 = 273.15;

/// Fahrenheit to Celsius offset.
pub const FAHRENHEIT_OFFSET: f64 = 32.0;

/// Fahrenheit to Celsius scale factor.
pub const FAHRENHEIT_SCALE: f64 = 5.0 / 9.0;
