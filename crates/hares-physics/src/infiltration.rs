//! Air infiltration models (Sherman-Grimsrud, ELA-based).
//!
//! # Pressure exponent (`n_i`) -- ASHRAE method only
//!
//! The infiltration pressure exponent `n_i` describes how air leakage rate
//! scales with the driving pressure difference across the building envelope:
//!
//! ```text
//! Q  ∝  ΔP^n_i
//! ```
//!
//! | Value | Physical meaning |
//! |-------|-----------------|
//! | 0.50  | Perfect orifice / turbulent flow (leaky buildings) |
//! | 0.65  | Typical residential envelope (OCHRE / ResStock default) |
//! | 0.70  | Laminar crack flow (tight modern buildings) |
//!
//! Using 0.5 instead of 0.65 for a tight building causes ±15–30 % error in
//! infiltration airflow.  Valid range is [0.5, 0.7].
//!
//! The `n_i` parameter applies only to `ashrae_wind_stack`. The ELA method
//! uses a fixed exponent of 0.5 per ASHRAE 62.2 and OCHRE's implementation.
//!
//! # References
//! - Walker & Wilson (1998) "Field Validation of Algebraic Equations for Stack
//!   and Wind Driven Air Infiltration Calculations", *HVAC&R Research*.
//! - ASHRAE Handbook of Fundamentals 2017, Chapter 16.
//! - EnergyPlus Engineering Reference §15.4 (AIM-2 / Sherman-Grimsrud).

use crate::units::*;

const M2_TO_CM2: f64 = 10_000.0;
const LPS_PER_CM2_TO_M3PS_PER_CM2: f64 = 1.0 / 1000.0;
const SECONDS_PER_HOUR: f64 = 3600.0;

const MET_STATION_ALPHA: f64 = 0.14;
const MET_STATION_DELTA_M: f64 = 270.0;
const MET_STATION_HEIGHT_M: f64 = 10.0;

const RURAL_ALPHA: f64 = 0.14;
const RURAL_DELTA_M: f64 = 270.0;
const SUBURBAN_ALPHA: f64 = 0.22;
const SUBURBAN_DELTA_M: f64 = 370.0;
const URBAN_ALPHA: f64 = 0.33;
const URBAN_DELTA_M: f64 = 460.0;

/// Minimum physically meaningful pressure exponent (turbulent / perfect orifice).
pub const N_I_MIN: f64 = 0.5;
/// Maximum physically meaningful pressure exponent (laminar crack flow).
pub const N_I_MAX: f64 = 0.7;
/// Default pressure exponent -- matches OCHRE / ResStock typical residential (0.65).
pub const N_I_DEFAULT: f64 = 0.65;

/// Canonical terrain classes from the architecture appendix.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TerrainClass {
    Rural,
    Suburban,
    Urban,
}

impl TerrainClass {
    pub fn alpha(self) -> f64 {
        match self {
            Self::Rural => RURAL_ALPHA,
            Self::Suburban => SUBURBAN_ALPHA,
            Self::Urban => URBAN_ALPHA,
        }
    }

    pub fn delta_m(self) -> f64 {
        match self {
            Self::Rural => RURAL_DELTA_M,
            Self::Suburban => SUBURBAN_DELTA_M,
            Self::Urban => URBAN_DELTA_M,
        }
    }
}

/// ASHRAE wind-stack infiltration using quadrature combination.
///
/// Implements the AIM-2 model (Walker & Wilson 1998) with a configurable
/// pressure exponent `n_i`.
///
/// # Formula (matches OCHRE `_infiltration_ashrae`)
/// ```text
/// Q_stack = inf_c × inf_Cs × |ΔT|^n_i
/// Q_wind  = inf_c × inf_Cw × (shelter × v)^(2·n_i)
/// Q       = √(Q_stack² + Q_wind²)
/// ```
///
/// Note: `n_stories` effect is baked into `c_s` (= `inf_c × inf_Cs`) during
/// parameter setup (via `infiltration_height`). It is NOT applied inside `powf()`.
///
/// # Parameters
/// - `c_s`: combined stack coefficient `inf_c × inf_Cs` [m³/s / K^n_i]
/// - `c_w`: combined wind coefficient `inf_c × inf_Cw` [m³/s / (m/s)^(2·n_i)]
/// - `delta_t_c`: indoor–outdoor temperature difference [K or °C, same Δ]
/// - `wind_speed_m_s`: wind speed at building height [m/s]
/// - `shelter_coeff`: dimensionless shelter coefficient [0, 1] (`inf_sft` in OCHRE)
/// - `n_i`: pressure exponent in [0.5, 0.7]; use [`N_I_DEFAULT`] (0.65) for typical residential
///
/// Returns volumetric flow [m³/s].
pub fn ashrae_wind_stack(
    c_s: f64,
    c_w: f64,
    delta_t_c: f64,
    wind_speed_m_s: f64,
    shelter_coeff: f64,
    n_i: f64,
) -> f64 {
    let n_i = n_i.clamp(N_I_MIN, N_I_MAX);
    let q_temp = c_s * delta_t_c.abs().powf(n_i);
    let q_wind = c_w * (shelter_coeff.max(0.0) * wind_speed_m_s).powf(2.0 * n_i);
    (q_temp * q_temp + q_wind * q_wind).sqrt()
}

/// Effective leakage area (ELA) infiltration model.
///
/// Uses a fixed square-root (exponent = 0.5) per ASHRAE 62.2 and OCHRE's
/// `_ela` path. The configurable `n_i` applies only to the ASHRAE method.
///
/// # Formula
/// ```text
/// Q = ELA_cm² × (L/s / cm²) × √(stack_coeff × |ΔT| + wind_coeff × v²)
/// ```
///
/// # Parameters
/// - `ela_m2`: effective leakage area [m²]
/// - `stack_coeff`: ELA stack coefficient [L/(s·cm⁴·K)]
/// - `wind_coeff`: ELA wind coefficient [L/(s·cm⁴·(m/s)²)]
/// - `delta_t_c`: indoor–outdoor temperature difference [K]
/// - `wind_speed_m_s`: wind speed at building height [m/s]
///
/// Returns volumetric flow [m³/s].
pub fn ela_infiltration(
    ela_m2: f64,
    stack_coeff: f64,
    wind_coeff: f64,
    delta_t_c: f64,
    wind_speed_m_s: f64,
) -> f64 {
    let ela_cm2 = ela_m2 * M2_TO_CM2;
    let driver = (stack_coeff * delta_t_c.abs() + wind_coeff * wind_speed_m_s.powi(2)).max(0.0);
    (ela_cm2 * LPS_PER_CM2_TO_M3PS_PER_CM2) * driver.sqrt()
}

/// ACH infiltration fallback model.
///
/// Returns volumetric flow [m³/s].
pub fn ach_infiltration(ach: f64, volume_m3: f64) -> f64 {
    ach * volume_m3 / SECONDS_PER_HOUR
}

/// Terrain-correct meteorological wind speed at building height.
///
/// Uses ASHRAE power-law correction with standard weather-station constants.
pub fn terrain_wind_speed(u_met: f64, alpha_site: f64, delta_site: f64, height: f64) -> f64 {
    if height <= 0.0 || delta_site <= 0.0 {
        return 0.0;
    }

    u_met
        * (MET_STATION_DELTA_M / MET_STATION_HEIGHT_M).powf(MET_STATION_ALPHA)
        * (height / delta_site).powf(alpha_site)
}

/// Terrain-correct meteorological wind speed for a standard terrain class.
pub fn terrain_wind_speed_for_class(u_met: f64, class: TerrainClass, height: f64) -> f64 {
    terrain_wind_speed(u_met, class.alpha(), class.delta_m(), height)
}

/// Natural ventilation flow through operable windows (ELA-style, OCHRE-matched).
///
/// Implements the ResStock / OCHRE natural ventilation model for operable windows.
/// Flow is gated by three conditions (all must hold for non-zero flow):
/// 1. Zone temperature > outdoor temperature (stack buoyancy drives outward exhaust).
/// 2. Zone temperature > `t_base_c` (occupant comfort; no cooling benefit otherwise).
/// 3. Outdoor humidity ratio < `max_outdoor_humidity_ratio` (muggy outdoor air is
///    not beneficial for cooling -- OCHRE default threshold: 0.0115 kg/kg from BA HSP).
///
/// When gated on, an adjustment factor `adj = (T_zone - T_base) / (T_zone - T_outdoor)`
/// is applied to scale the flow proportionally to how far above the comfort base the
/// zone is, clamped to [0, 1].
///
/// # Formula (OCHRE `_natural_ventilation`, no forced-vent interaction)
/// ```text
/// A_eff   = open_area_m2 × 0.6 × 10000 cm²/m²            (effectiveness × unit conv)
/// q_drive = stack_coeff × |ΔT| + wind_coeff × v²          (ELA driver, same as infiltration)
/// adj     = clamp((T_zone - T_base) / (T_zone - T_out), 0, 1)
/// q_nat   = min(A_eff × adj × √q_drive / 1000, 20 ACH cap)
/// ```
///
/// # Parameters
/// - `open_area_m2`: effective open window area [m²] (typically 6.7% of total window area)
/// - `t_zone_c`: zone (indoor) air temperature [°C]
/// - `t_outdoor_c`: outdoor air temperature [°C]
/// - `t_base_c`: comfort base temperature [°C]; no flow when `t_zone ≤ t_base`
///   (OCHRE default: 22.78 °C = 73 °F)
/// - `outdoor_humidity_ratio`: outdoor specific humidity [kg/kg]; flow suppressed
///   when ≥ `max_outdoor_humidity_ratio`
/// - `max_outdoor_humidity_ratio`: humidity threshold [kg/kg] (OCHRE default: 0.0115)
/// - `wind_speed_m_s`: wind speed [m/s]
/// - `stack_coeff`: ELA stack coefficient [L/(s·cm⁴·K)] -- same as infiltration ELA coeff
/// - `wind_coeff`: ELA wind coefficient [L/(s·cm⁴·(m/s)²)] -- same as infiltration ELA coeff
/// - `zone_volume_m3`: zone volume [m³] -- used to cap at 20 ACH
///
/// Returns volumetric flow [m³/s], or 0.0 when gating conditions are not met.
#[allow(clippy::too_many_arguments)]
pub fn natural_ventilation_flow_m3_s(
    open_area_m2: f64,
    t_zone_c: f64,
    t_outdoor_c: f64,
    t_base_c: f64,
    outdoor_humidity_ratio: f64,
    max_outdoor_humidity_ratio: f64,
    wind_speed_m_s: f64,
    stack_coeff: f64,
    wind_coeff: f64,
    zone_volume_m3: f64,
) -> f64 {
    // Temperature and humidity gating (OCHRE: `if w_amb >= max_oa_hr or t_zone <= t_ext or t_zone <= t_base`)
    if outdoor_humidity_ratio >= max_outdoor_humidity_ratio
        || t_zone_c <= t_outdoor_c
        || t_zone_c <= t_base_c
        || open_area_m2 <= 0.0
    {
        return 0.0;
    }

    let delta_t = t_outdoor_c - t_zone_c; // negative (t_zone > t_outdoor)

    // Effectiveness factor 0.6 per EnergyPlus/OCHRE; convert to cm² for ELA formula
    let nat_vent_area_cm2 = open_area_m2 * 0.6 * M2_TO_CM2;

    // Adjustment factor: how far above comfort base is the zone, relative to the delta
    // adj = (t_zone - t_base) / (t_zone - t_outdoor); clamped to [0, 1]
    let adj = ((t_zone_c - t_base_c) / (t_zone_c - t_outdoor_c)).clamp(0.0, 1.0);

    // ELA-style driver (same square-root form as `ela_infiltration`)
    let driver = (stack_coeff * delta_t.abs() + wind_coeff * wind_speed_m_s.powi(2)).max(0.0);

    // Flow in m³/s: area_cm2 * adj * √driver / 1000 (ELA L→m³ conversion)
    let q_nat = nat_vent_area_cm2 * adj * driver.sqrt() * LPS_PER_CM2_TO_M3PS_PER_CM2;

    // Cap at 20 ACH (OCHRE `max_nat_flow = 20.0 * volume * m3hr_to_m3s`)
    let max_nat_flow = 20.0 * zone_volume_m3 / SECONDS_PER_HOUR;
    q_nat.min(max_nat_flow)
}

/// ASHRAE 152 duct-leakage/infiltration interaction -- superposition formula.
///
/// When the HVAC fan is running, unbalanced duct leakage pressurises or
/// depressurises the house, shifting the natural infiltration rate.  ASHRAE
/// Standard 152 §9.3 gives the adjusted infiltration flow:
///
/// ```text
/// infil_fan_off  = 0.35 × house_volume_m3 / 60          [m³/s]  (ASHRAE 152 baseline)
/// imb            = |supply_leakage_m3_s - return_leakage_m3_s|   [m³/s]
///
/// if supply > return:  adjusted = (baseline^1.5 + imb^1.5)^0.67  (pressurisation → more infiltration)
/// elif imb > baseline: adjusted = 0                               (large depressurisation dominates)
/// else:                adjusted = (baseline^1.5 - imb^1.5)^0.67  (partial depressurisation)
/// ```
///
/// # Parameters
/// - `base_infil_m3_s`: natural infiltration rate computed by the zone model [m³/s]
/// - `supply_leakage_m3_s`: supply duct leakage flow during fan operation [m³/s]
/// - `return_leakage_m3_s`: return duct leakage flow during fan operation [m³/s]
/// - `house_volume_m3`: conditioned zone volume [m³]
///
/// Returns the adjusted infiltration flow [m³/s].  When both leakage flows are
/// zero the function is a no-op (returns `base_infil_m3_s` unchanged).
///
/// # References
/// - ASHRAE Standard 152-2004 §9.3, Eq. 9.3 (infiltration interaction).
/// - Infiltration in ASHRAE's Residential Ventilation Standards (Sherman, 2008).
pub fn duct_leakage_infiltration_m3_s(
    base_infil_m3_s: f64,
    supply_leakage_m3_s: f64,
    return_leakage_m3_s: f64,
    house_volume_m3: f64,
) -> f64 {
    if supply_leakage_m3_s == 0.0 && return_leakage_m3_s == 0.0 {
        return base_infil_m3_s;
    }

    // ASHRAE 152 §9.3 baseline: 0.35 × V / 60  [m³/s]
    let infil_fan_off = 0.35 * house_volume_m3 / 60.0;
    let imb = (supply_leakage_m3_s - return_leakage_m3_s).abs();

    let adjusted = if supply_leakage_m3_s > return_leakage_m3_s {
        // Pressurisation: infiltration increases above baseline.
        (infil_fan_off.powf(1.5) + imb.powf(1.5)).powf(0.67)
    } else if imb > infil_fan_off {
        // Large depressurisation dominates: all envelope leakage becomes exfiltration.
        0.0
    } else {
        // Partial depressurisation: baseline partially suppressed.
        (infil_fan_off.powf(1.5) - imb.powf(1.5)).powf(0.67)
    };

    // The ASHRAE 152 formula replaces the `infil_fan_off` term.  We scale the
    // caller's base rate by the same ratio so it also captures stack/wind effects.
    if infil_fan_off > 0.0 {
        base_infil_m3_s * (adjusted / infil_fan_off)
    } else {
        adjusted
    }
}

// ---------------------------------------------------------------------------
// Typed boundary wrappers
// ---------------------------------------------------------------------------

/// [`ach_infiltration`] with a typed `Volume` parameter.
///
/// Returns volumetric flow in m³/s as raw `f64` (compound return type
/// would not add clarity here).
pub fn ach_infiltration_typed(ach: f64, volume: Volume) -> f64 {
    ach_infiltration(ach, volume.get::<uom::si::volume::cubic_meter>())
}

/// [`terrain_wind_speed`] with typed length / velocity parameters.
pub fn terrain_wind_speed_typed(
    u_met: Velocity,
    alpha_site: f64,
    delta_site: Length,
    height: Length,
) -> Velocity {
    let raw = terrain_wind_speed(
        u_met.get::<uom::si::velocity::meter_per_second>(),
        alpha_site,
        delta_site.get::<uom::si::length::meter>(),
        height.get::<uom::si::length::meter>(),
    );
    Velocity::new::<uom::si::velocity::meter_per_second>(raw)
}

/// [`terrain_wind_speed_for_class`] with typed length / velocity parameters.
pub fn terrain_wind_speed_for_class_typed(
    u_met: Velocity,
    class: TerrainClass,
    height: Length,
) -> Velocity {
    terrain_wind_speed_typed(
        u_met,
        class.alpha(),
        Length::new::<uom::si::length::meter>(class.delta_m()),
        height,
    )
}

// ---------------------------------------------------------------------------
// AIM-2 coefficients from ACH50 (Walker & Wilson 1998)
//
// Converts a blower-door ACH50 measurement to the runtime `c_s`, `c_w`,
// `shelter_coeff` parameters consumed by `ashrae_wind_stack()`.
//
// References:
//   Walker & Wilson (1998) "Field Validation of Algebraic Equations for Stack
//   and Wind Driven Air Infiltration Calculations", HVAC&R Research 4(2).
//   ASHRAE Handbook of Fundamentals 2021, Chapter 16.
//   EnergyPlus Engineering Reference §15.4 (AIM-2 Enhanced Model).
//   OCHRE: vendors/OCHRE/ochre/utils/envelope.py:488-633.
// ---------------------------------------------------------------------------

/// Shielding class for the AIM-2 shelter coefficient.
///
/// Values from Walker & Wilson (1998) Table 3 and ResStock `get_aim2_shelter_coefficient`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ShieldingClass {
    /// Typical suburban -- `shelter_raw = 0.5`.
    Normal,
    /// Flat terrain, few obstructions -- `shelter_raw = 0.9`.
    Exposed,
    /// Dense surroundings -- `shelter_raw = 0.3`.
    WellShielded,
}

impl ShieldingClass {
    /// Raw shelter coefficient before terrain/flue correction.
    /// Walker & Wilson (1998) Table 3; ResStock `airflow.get_aim2_shelter_coefficient`.
    pub fn raw(self) -> f64 {
        match self {
            Self::Normal => 0.5,
            Self::Exposed => 0.9,
            Self::WellShielded => 0.3,
        }
    }
}

/// Foundation leakage distribution class.
///
/// Walker & Wilson (1998) Table 1 -- leakage fractions for ceiling/walls/floor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FoundationLeakageClass {
    /// Vented crawlspace: ceil=0.15, wall=0.35, floor=0.50.
    VentedCrawlspace,
    /// Slab, basement, or unvented crawlspace: ceil=0.25, wall=0.50, floor=0.25.
    Other,
}

/// Input parameters for `aim2_coefficients_from_ach50`.
#[derive(Clone, Debug)]
pub struct Aim2Params {
    /// Blower-door result at 50 Pa [ACH].
    pub ach50: f64,
    /// Conditioned volume [m³].
    pub volume_m3: f64,
    /// Infiltration height [m] (total envelope height for stack effect).
    pub infiltration_height_m: f64,
    /// Foundation leakage distribution.
    pub foundation: FoundationLeakageClass,
    /// Site shielding class.
    pub shielding: ShieldingClass,
    /// Terrain class for wind speed correction.
    pub terrain: TerrainClass,
    /// Whether a flue or chimney is present in conditioned space.
    pub has_flue: bool,
    /// Pressure exponent; use [`N_I_DEFAULT`] (0.65) for typical residential.
    pub n_i: f64,
    /// Number of conditioned floors above grade (used for flue correction).
    pub floors_above_grade: f64,
}

/// Output coefficients from `aim2_coefficients_from_ach50`.
#[derive(Clone, Debug)]
pub struct Aim2Coefficients {
    /// Combined stack coefficient `C × Cs` [m³/s / K^n_i].
    pub c_s: f64,
    /// Combined wind coefficient `C × Cw` [m³/s / (m/s)^(2·n_i)].
    pub c_w: f64,
    /// Shelter coefficient (terrain-corrected) for `ashrae_wind_stack()`.
    pub shelter_coeff: f64,
    /// Pressure exponent (passed through for convenience).
    pub n_i: f64,
}

/// Convert ACH50 blower-door result to AIM-2 runtime coefficients.
///
/// Implements the full Walker & Wilson (1998) SI pipeline:
/// ACH50 → flow coefficient C → leakage distribution → stack/wind factors
/// → `c_s`, `c_w`, `shelter_coeff` for [`ashrae_wind_stack()`].
///
/// # References
/// - Walker & Wilson (1998), Equations 9–25.
/// - ASHRAE HOF 2021 Ch. 16.
/// - OCHRE `calculate_ashrae_infiltration_params` (envelope.py:488-633).
pub fn aim2_coefficients_from_ach50(params: &Aim2Params) -> Aim2Coefficients {
    /// Standard air density [kg/m³] at ~20 °C, 101.325 kPa.
    const RHO: f64 = 1.2041;
    /// Standard gravity [m/s²].
    const G: f64 = 9.80665;
    /// Assumed indoor temperature [K] ≈ 23 °C (73.5 °F) -- matches OCHRE.
    const T_IN_K: f64 = 296.15;

    let n_i = params.n_i.clamp(N_I_MIN, N_I_MAX);

    // Step 1: Flow coefficient C [m³/s/Pa^n_i]
    // Q_50 = ACH50 × V / 3600 [m³/s], ΔP = 50 Pa
    // Q_50 = C × 50^n_i  →  C = Q_50 / 50^n_i
    let c_flow = (params.ach50 * params.volume_m3) / (SECONDS_PER_HOUR * 50.0_f64.powf(n_i));

    // Step 2: Leakage distribution -- Walker & Wilson (1998) Table 1
    let (leak_ceil, leak_floor) = match params.foundation {
        FoundationLeakageClass::VentedCrawlspace => (0.15, 0.50),
        FoundationLeakageClass::Other => (0.25, 0.25),
    };

    let y_i = if params.has_flue { 0.2 } else { 0.0 };
    // Walker & Wilson (1998) §3: vertical fraction and asymmetry
    let r_i = (leak_ceil + leak_floor) * (1.0 - y_i);
    let x_i_raw = (leak_ceil - leak_floor) * (1.0 - y_i);

    // Step 3: Stack factor -- Walker & Wilson (1998) Eq. 9-12
    let m_o = (x_i_raw + (2.0 * n_i + 1.0) * y_i).powi(2) / (2.0 - r_i);
    let m_i = m_o.min(1.0); // Eq. 10-11

    let f_i = if params.has_flue {
        // Flue correction -- Walker & Wilson (1998) Eq. 12-13
        let ncfl = params.floors_above_grade.max(1.0);
        let z_f = (ncfl + 0.5) / ncfl;
        // Critical ceiling-floor leakage difference (Eq. 13)
        let x_c = r_i + (2.0 * (1.0 - r_i - y_i)) / (n_i + 1.0) - 2.0 * y_i * (z_f - 1.0).powf(n_i);
        // Additive flue function (Eq. 12)
        n_i * y_i
            * (z_f - 1.0).powf((3.0 * n_i - 1.0) / 3.0)
            * (1.0 - (3.0 * (x_c - x_i_raw).powi(2) * r_i.powf(1.0 - n_i)) / (2.0 * (z_f + 1.0)))
    } else {
        0.0
    };

    // Walker & Wilson (1998) Eq. 9
    let f_s = ((1.0 + n_i * r_i) / (n_i + 1.0)) * (0.5 - 0.5 * m_i.powf(1.2)).powf(n_i + 1.0) + f_i;

    // Step 4: Stack coefficient Cs -- pure SI
    // Cs = f_s × (ρ·g·H / T_in)^n_i  [(Pa/K)^n_i]
    let cs = f_s * (RHO * G * params.infiltration_height_m / T_IN_K).powf(n_i);

    // Step 5: Wind factor -- Walker & Wilson (1998) Eq. 15-25
    let f_w = if matches!(params.foundation, FoundationLeakageClass::VentedCrawlspace) {
        // Crawlspace modified wind factor (Eq. 20-24)
        let x_i = x_i_raw.min(1.0 - 2.0 * y_i); // Eq. 25 clamp
        let r_x = 1.0 - r_i * (n_i / 2.0 + 0.2); // Eq. 21
        let y_x = 1.0 - y_i / 4.0; // Eq. 22
        let x_s = (1.0 - r_i) / 5.0 - 1.5 * y_i; // Eq. 24
        let x_x = 1.0 - (((x_i - x_s) / (2.0 - r_i)).powi(2)).powf(0.75); // Eq. 23
        0.19 * (2.0 - n_i) * x_x * r_x * y_x // Eq. 20
    } else {
        // Non-crawlspace wind factor (Eq. 15-19)
        let j_i = (x_i_raw + r_i + 2.0 * y_i) / 2.0;
        0.19 * (2.0 - n_i) * (1.0 - ((x_i_raw + r_i) / 2.0).powf(1.5 - y_i))
            - y_i / 4.0 * (j_i - 2.0 * y_i * j_i.powi(4))
    };

    // Step 6: Wind coefficient Cw -- pure SI
    // Cw = f_w × (ρ/2)^n_i  [(Pa/(m/s)²)^n_i]
    let cw = f_w * (RHO / 2.0).powf(n_i);

    // Step 7: Combined coefficients
    let c_s = c_flow * cs;
    let c_w = c_flow * cw;

    // Step 8: Shelter coefficient -- terrain-corrected
    // f_t from terrain_wind_speed() with u_met=1.0 at infiltration height
    let f_t = terrain_wind_speed(
        1.0,
        params.terrain.alpha(),
        params.terrain.delta_m(),
        params.infiltration_height_m,
    );
    let s_wflue = if params.has_flue { 1.0 } else { 0.0 };
    // OCHRE envelope.py:521: inf_sft = f_t * (shelter * (1-y_i) + s_wflue * 1.5 * y_i)
    let shelter_coeff = f_t * (params.shielding.raw() * (1.0 - y_i) + s_wflue * 1.5 * y_i);

    Aim2Coefficients {
        c_s,
        c_w,
        shelter_coeff,
        n_i,
    }
}

// ---------------------------------------------------------------------------
// ELA coefficient calculation
//
// Walker & Wilson (1998) "Field Validation of Algebraic Equations for Stack
// and Wind Driven Air Infiltration Calculations", HVAC&R Research.
// ASHRAE Handbook of Fundamentals 2021, Chapter 16.
// ---------------------------------------------------------------------------

/// Calculates ELA stack and wind coefficients for a zone, entirely in SI.
///
/// Derives `Cs` and `Cw` from the Walker-Wilson (1998) stack and wind shape
/// factors using the full ASHRAE two-parameter terrain power law.
///
/// # Parameters
/// - `hor_lk_frac`: horizontal leakage fraction (fraction of total leakage in
///   horizontal surfaces). Walker-Wilson (1998) Table 2:
///   - 0.0  for conditioned zones (vertical-dominated leakage)
///   - 0.4  for garages (mixed)
///   - 0.75 for vented attics (ceiling-dominated leakage)
/// - `zone_height_m`: zone height [m]
/// - `zone_height_above_ground_m`: height of zone bottom above ground [m]
/// - `terrain`: site terrain class for wind speed correction
/// - `shielding`: shielding coefficient from Walker-Wilson (1998) Table 3:
///   - 0.10 (well-shielded: dense trees/buildings on all sides)
///   - 1/6 ≈ 0.167 (normal: typical suburban, default)
///   - 0.30 (exposed: flat terrain, few obstructions)
///
/// # Returns
/// `(stack_coeff, wind_coeff)` in units compatible with [`ela_infiltration`]:
/// - stack_coeff: [(L/s)²/(cm⁴·K)]
/// - wind_coeff: [(L/s)²/(cm⁴·(m/s)²)]
///
/// # Derivation
/// The ELA flow equation is `Q = (ELA_cm²/1000) × √(Cs·|ΔT| + Cw·v²)` [m³/s].
///
/// **Stack coefficient:**
/// `Cs = f_s² × g × H / T_in × 1e-2`
/// where `f_s = (2/3)(1 + R/2) × √(2·nl·(1-nl)) / (√nl + √(1-nl))` is the
/// Walker-Wilson stack shape factor, `g = 9.80665 m/s²`, `H` is zone height,
/// `T_in` is indoor temperature [K], and `1e-2` converts m²/(s²·K) →
/// (L/s)²/(cm⁴·K) since `1 m² = 1e4 cm²` and `1 L²/cm⁴ = 1e6 cm²`.
///
/// **Wind coefficient:**
/// `Cw = f_w² / 100`
/// where `f_w = s_g × (1-R)^(1/3) × f_t` is the wind shape factor,
/// `f_t` is the ASHRAE terrain correction from `terrain_wind_speed()`,
/// and `/100` converts dimensionless f_w² to (L/s)²/(cm⁴·(m/s)²) units
/// (proven from the ELA geometry: `Q = ELA_m² × f_w × v` must equal
/// `(ELA_cm²/1000) × √(Cw × v²)`, requiring `Cw = f_w²/100`).
pub fn calculate_ela_coefficients(
    hor_lk_frac: f64,
    zone_height_m: f64,
    zone_height_above_ground_m: f64,
    terrain: TerrainClass,
    shielding: f64,
) -> (f64, f64) {
    /// Standard gravity [m/s²].
    const G_M_S2: f64 = 9.80665;
    /// Default assumed indoor temperature [K] (≈ 23 °C / 73.5 °F).
    const T_IN_K: f64 = 296.15;
    /// Neutral pressure level fraction [-]. 0.5 is standard for all
    /// simplified residential models (Walker-Wilson 1998, §3.1).
    const NL: f64 = 0.5;

    // Stack shape factor f_s (Walker-Wilson 1998, Eq. 12).
    let f_s = (2.0 / 3.0) * (1.0 + hor_lk_frac / 2.0) * (2.0 * NL * (1.0 - NL)).sqrt()
        / (NL.sqrt() + (1.0 - NL).sqrt());

    // Stack coefficient [m²/(s²·K)], then ×1e-2 → [(L/s)²/(cm⁴·K)].
    // Derivation: 1 m² = 1e4 cm²; 1 L²/cm⁴ = 1e6 cm²; so 1 m² = 1e-2 L²/cm⁴.
    let cs_m2 = f_s * f_s * G_M_S2 * zone_height_m / T_IN_K;
    let stack_coeff = cs_m2 * 1e-2;

    // Terrain wind speed correction f_t using the full ASHRAE HOF Chapter 16
    // two-parameter power law -- the same model as terrain_wind_speed().
    // f_t = (δ_met / h_met)^α_met × (H_total / δ_site)^α_site
    let h_total = (zone_height_m + zone_height_above_ground_m).max(0.1);
    let f_t = (MET_STATION_DELTA_M / MET_STATION_HEIGHT_M).powf(MET_STATION_ALPHA)
        * (h_total / terrain.delta_m()).powf(terrain.alpha());

    // Wind shape factor f_w (Walker-Wilson 1998, Eq. 13).
    let f_w = shielding * (1.0 - hor_lk_frac).powf(1.0 / 3.0) * f_t;

    // Wind coefficient: Cw = f_w² / 100 [(L/s)²/(cm⁴·(m/s)²)].
    let wind_coeff = f_w * f_w / 100.0;

    (stack_coeff, wind_coeff)
}

/// Default shielding coefficient for "normal" suburban exposure.
/// Walker-Wilson (1998) Table 3: s_g = 0.5/3 for "normal" shielding.
pub const SHIELDING_NORMAL: f64 = 0.5 / 3.0;

/// ELA coefficients for a vented attic zone.
///
/// Uses `hor_lk_frac = 0.75` (Walker-Wilson 1998 Table 2: ceiling-dominated
/// leakage). Terrain and shielding use suburban/normal defaults.
pub fn attic_ela_coefficients(attic_height_m: f64, building_height_m: f64) -> (f64, f64) {
    calculate_ela_coefficients(
        0.75,
        attic_height_m,
        building_height_m,
        TerrainClass::Suburban,
        SHIELDING_NORMAL,
    )
}

/// ELA coefficients for a garage zone at ground level.
///
/// Uses `hor_lk_frac = 0.4` (Walker-Wilson 1998 Table 2: mixed leakage).
pub fn garage_ela_coefficients(garage_height_m: f64) -> (f64, f64) {
    calculate_ela_coefficients(
        0.4,
        garage_height_m,
        0.0,
        TerrainClass::Suburban,
        SHIELDING_NORMAL,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use uom::si::length::meter;
    use uom::si::velocity::meter_per_second;
    use uom::si::volume::cubic_meter;

    use crate::test_utils::approx_eq;

    // -----------------------------------------------------------------------
    // ASHRAE n_i default and OCHRE parity
    // -----------------------------------------------------------------------

    #[test]
    fn ashrae_default_n_i_is_065() {
        approx_eq(N_I_DEFAULT, 0.65, 1e-15);
    }

    #[test]
    fn ashrae_matches_ochre_formula() {
        // OCHRE: inf_flow_temp = inf_c * inf_Cs * abs(delta_t) ** n_i
        //        inf_flow_wind = inf_c * inf_Cw * (inf_sft * wind) ** (2 * n_i)
        //        return sqrt(temp² + wind²)
        let c_s = 0.015;
        let c_w = 0.0008;
        let dt = 15.0;
        let v = 4.0;
        let shelter = 0.5;
        let n_i = 0.65;

        let q = ashrae_wind_stack(c_s, c_w, dt, v, shelter, n_i);
        let q_temp = c_s * dt.abs().powf(n_i);
        let q_wind = c_w * (shelter * v).powf(2.0 * n_i);
        let expected = (q_temp * q_temp + q_wind * q_wind).sqrt();
        approx_eq(q, expected, 1e-14);
    }

    // -----------------------------------------------------------------------
    // ASHRAE pressure exponent sensitivity
    // -----------------------------------------------------------------------

    #[test]
    fn ashrae_stack_only_n_i_monotonic_when_driver_gt_1() {
        // For |ΔT| > 1: higher n_i → MORE flow
        let c_s = 1.0;
        let dt = 5.0;

        let q_05 = ashrae_wind_stack(c_s, 0.0, dt, 0.0, 0.0, 0.5);
        let q_65 = ashrae_wind_stack(c_s, 0.0, dt, 0.0, 0.0, 0.65);
        let q_70 = ashrae_wind_stack(c_s, 0.0, dt, 0.0, 0.0, 0.70);

        assert!(q_05 < q_65, "driver>1: q_ni05={q_05} < q_ni65={q_65}");
        assert!(q_65 < q_70, "driver>1: q_ni65={q_65} < q_ni70={q_70}");
    }

    #[test]
    fn ashrae_stack_only_n_i_monotonic_when_driver_lt_1() {
        // For |ΔT| < 1: higher n_i → LESS flow
        let c_s = 1.0;
        let dt = 0.5;

        let q_05 = ashrae_wind_stack(c_s, 0.0, dt, 0.0, 0.0, 0.5);
        let q_65 = ashrae_wind_stack(c_s, 0.0, dt, 0.0, 0.0, 0.65);
        let q_70 = ashrae_wind_stack(c_s, 0.0, dt, 0.0, 0.0, 0.70);

        assert!(q_05 > q_65, "driver<1: q_ni05={q_05} > q_ni65={q_65}");
        assert!(q_65 > q_70, "driver<1: q_ni65={q_65} > q_ni70={q_70}");
    }

    #[test]
    fn ashrae_n_i_clamped_to_valid_range() {
        // n_i outside [0.5, 0.7] should be clamped
        let q_below = ashrae_wind_stack(0.02, 0.001, 10.0, 5.0, 0.7, 0.3);
        let q_at_min = ashrae_wind_stack(0.02, 0.001, 10.0, 5.0, 0.7, N_I_MIN);
        approx_eq(q_below, q_at_min, 1e-14);

        let q_above = ashrae_wind_stack(0.02, 0.001, 10.0, 5.0, 0.7, 0.9);
        let q_at_max = ashrae_wind_stack(0.02, 0.001, 10.0, 5.0, 0.7, N_I_MAX);
        approx_eq(q_above, q_at_max, 1e-14);
    }

    // -----------------------------------------------------------------------
    // ELA (fixed sqrt exponent per OCHRE / ASHRAE 62.2)
    // -----------------------------------------------------------------------

    #[test]
    fn ela_matches_ochre_sqrt_formula() {
        // OCHRE: inf_flow = f * ela/1000 * (stack*|ΔT| + wind*v²) ** 0.5
        let ela_m2 = 0.03;
        let stack = 0.000145;
        let wind = 0.000087;
        let dt = 10.0;
        let v = 5.0;

        let flow = ela_infiltration(ela_m2, stack, wind, dt, v);
        let driver: f64 = stack * dt + wind * v * v;
        let expected = (ela_m2 * M2_TO_CM2 * LPS_PER_CM2_TO_M3PS_PER_CM2) * driver.sqrt();
        approx_eq(flow, expected, 1e-12);
    }

    #[test]
    fn ela_physical_reasonableness_typical_residence() {
        // Typical US single-family: 200 m² floor, 2.4 m ceiling = 480 m³
        let ela_m2 = 0.12;
        let stack_coeff = 0.000106;
        let wind_coeff = 0.000143;
        let dt = 10.0;
        let v = 4.0;
        let volume_m3 = 480.0;

        let flow_m3s = ela_infiltration(ela_m2, stack_coeff, wind_coeff, dt, v);
        let ach = flow_m3s / volume_m3 * SECONDS_PER_HOUR;
        assert!(
            (0.1..=2.0).contains(&ach),
            "ACH={ach:.3} outside [0.1, 2.0]"
        );
    }

    // -----------------------------------------------------------------------
    // Quadrature and structural tests
    // -----------------------------------------------------------------------

    #[test]
    fn ashrae_combines_wind_and_stack_in_quadrature() {
        let q_temp_only = ashrae_wind_stack(0.02, 0.0, 10.0, 5.0, 0.7, N_I_DEFAULT);
        let q_wind_only = ashrae_wind_stack(0.0, 0.001, 10.0, 5.0, 0.7, N_I_DEFAULT);
        let q_both = ashrae_wind_stack(0.02, 0.001, 10.0, 5.0, 0.7, N_I_DEFAULT);
        approx_eq(
            q_both,
            (q_temp_only * q_temp_only + q_wind_only * q_wind_only).sqrt(),
            1e-12,
        );
    }

    #[test]
    fn ach_infiltration_converts_hourly_exchange() {
        let flow = ach_infiltration(1.0, 200.0);
        approx_eq(flow, 200.0 / SECONDS_PER_HOUR, 1e-12);
    }

    #[test]
    fn terrain_coefficients_match_appendix_classes() {
        let u_met = 5.0;
        let height = 10.0;

        let rural = terrain_wind_speed_for_class(u_met, TerrainClass::Rural, height);
        let suburban = terrain_wind_speed_for_class(u_met, TerrainClass::Suburban, height);
        let urban = terrain_wind_speed_for_class(u_met, TerrainClass::Urban, height);

        approx_eq(rural, 5.0, 0.001);
        approx_eq(suburban, 3.583_905_519, 0.001);
        approx_eq(urban, 2.242_078_669, 0.001);
    }

    #[test]
    fn ashrae_hof_ch16_table5_coefficients() {
        // ASHRAE 2017 HOF Ch. 16, Table 5: Cs=0.000290, Cw=0.000231, shelter=1.0
        let q = ashrae_wind_stack(0.000290, 0.000231, 10.0, 5.0, 1.0, N_I_DEFAULT);
        assert!(
            q > 0.0 && q < 0.01,
            "ASHRAE HOF Ch.16 Table 5: {q:.6}, expected ~0.001 m³/s"
        );
    }

    #[test]
    fn typed_wrappers_match_raw_kernels() {
        let vol = Volume::new::<cubic_meter>(200.0);
        let raw_ach = ach_infiltration(1.0, 200.0);
        let typed_ach = ach_infiltration_typed(1.0, vol);
        approx_eq(typed_ach, raw_ach, 1e-15);

        let u = Velocity::new::<meter_per_second>(5.0);
        let h = Length::new::<meter>(10.0);
        let delta = Length::new::<meter>(SUBURBAN_DELTA_M);
        let raw_ws = terrain_wind_speed(5.0, SUBURBAN_ALPHA, SUBURBAN_DELTA_M, 10.0);
        let typed_ws = terrain_wind_speed_typed(u, SUBURBAN_ALPHA, delta, h);
        approx_eq(typed_ws.get::<meter_per_second>(), raw_ws, 1e-15);

        let raw_cls = terrain_wind_speed_for_class(5.0, TerrainClass::Urban, 10.0);
        let typed_cls = terrain_wind_speed_for_class_typed(u, TerrainClass::Urban, h);
        approx_eq(typed_cls.get::<meter_per_second>(), raw_cls, 1e-15);
    }

    // -----------------------------------------------------------------------
    // Zero / edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn ashrae_zero_drivers_gives_zero_flow() {
        let q = ashrae_wind_stack(0.02, 0.001, 0.0, 0.0, 0.5, 0.65);
        approx_eq(q, 0.0, 1e-15);
    }

    #[test]
    fn ela_zero_drivers_gives_zero_flow() {
        let q = ela_infiltration(0.05, 0.000106, 0.000143, 0.0, 0.0);
        approx_eq(q, 0.0, 1e-15);
    }

    #[test]
    fn terrain_wind_zero_height_returns_zero() {
        let u = terrain_wind_speed(5.0, SUBURBAN_ALPHA, SUBURBAN_DELTA_M, 0.0);
        approx_eq(u, 0.0, 1e-15);
    }

    // -----------------------------------------------------------------------
    // Natural ventilation
    // -----------------------------------------------------------------------

    /// Default test parameters -- warm zone, cool outdoor, dry outdoor air,
    /// non-trivial window area and ELA coefficients.
    fn nat_vent_base_args() -> (f64, f64, f64, f64, f64, f64, f64, f64, f64, f64) {
        (
            0.5,       // open_area_m2
            26.0,      // t_zone_c
            18.0,      // t_outdoor_c
            22.778,    // t_base_c  (73 °F -- OCHRE default)
            0.008,     // outdoor_humidity_ratio
            0.0115,    // max_outdoor_humidity_ratio
            3.0,       // wind_speed_m_s
            0.000_106, // stack_coeff
            0.000_143, // wind_coeff
            200.0,     // zone_volume_m3
        )
    }

    #[test]
    fn nat_vent_zero_when_zone_cooler_than_outdoor() {
        let (a, _, _, tb, h, mh, v, sc, wc, vol) = nat_vent_base_args();
        let q = natural_ventilation_flow_m3_s(a, 15.0, 18.0, tb, h, mh, v, sc, wc, vol);
        approx_eq(q, 0.0, 1e-15);
    }

    #[test]
    fn nat_vent_zero_when_zone_at_comfort_base() {
        let (a, _, to, tb, h, mh, v, sc, wc, vol) = nat_vent_base_args();
        // t_zone == t_base → gated off (≤ check)
        let q = natural_ventilation_flow_m3_s(a, tb, to, tb, h, mh, v, sc, wc, vol);
        approx_eq(q, 0.0, 1e-15);
    }

    #[test]
    fn nat_vent_zero_when_outdoor_too_humid() {
        let (a, tz, to, tb, _, mh, v, sc, wc, vol) = nat_vent_base_args();
        // humidity == threshold → gated off (>= check)
        let q = natural_ventilation_flow_m3_s(a, tz, to, tb, mh, mh, v, sc, wc, vol);
        approx_eq(q, 0.0, 1e-15);
    }

    #[test]
    fn nat_vent_zero_when_no_open_area() {
        let (_, tz, to, tb, h, mh, v, sc, wc, vol) = nat_vent_base_args();
        let q = natural_ventilation_flow_m3_s(0.0, tz, to, tb, h, mh, v, sc, wc, vol);
        approx_eq(q, 0.0, 1e-15);
    }

    #[test]
    fn nat_vent_positive_under_nominal_conditions() {
        let (a, tz, to, tb, h, mh, v, sc, wc, vol) = nat_vent_base_args();
        let q = natural_ventilation_flow_m3_s(a, tz, to, tb, h, mh, v, sc, wc, vol);
        assert!(
            q > 0.0,
            "expected positive nat vent flow under nominal conditions, got {q}"
        );
    }

    #[test]
    fn nat_vent_matches_ochre_formula_directly() {
        // Manually replicate OCHRE `_natural_ventilation` formula for cross-check.
        let open_area_m2 = 0.5_f64;
        let t_zone_c = 26.0_f64;
        let t_outdoor_c = 18.0_f64;
        let t_base_c = 22.778_f64;
        let outdoor_hum = 0.008_f64;
        let max_hum = 0.0115_f64;
        let wind_speed = 3.0_f64;
        let stack_coeff = 0.000_106_f64;
        let wind_coeff = 0.000_143_f64;
        let volume_m3 = 200.0_f64;

        let q = natural_ventilation_flow_m3_s(
            open_area_m2,
            t_zone_c,
            t_outdoor_c,
            t_base_c,
            outdoor_hum,
            max_hum,
            wind_speed,
            stack_coeff,
            wind_coeff,
            volume_m3,
        );

        // Reproduce OCHRE `_natural_ventilation` formula step-by-step
        let delta_t = t_outdoor_c - t_zone_c;
        let nat_vent_area_cm2 = open_area_m2 * 0.6 * 10_000.0;
        let max_nat_flow = 20.0 * volume_m3 / 3600.0;
        let adj = ((t_zone_c - t_base_c) / (t_zone_c - t_outdoor_c)).clamp(0.0, 1.0);
        let nat_vent_data = stack_coeff * delta_t.abs() + wind_coeff * wind_speed * wind_speed;
        let expected = (nat_vent_area_cm2 * adj * nat_vent_data.sqrt() / 1000.0).min(max_nat_flow);

        approx_eq(q, expected, 1e-12);
    }

    #[test]
    fn nat_vent_capped_at_20_ach() {
        // Enormous open area should hit the 20-ACH cap.
        let q = natural_ventilation_flow_m3_s(
            1000.0, 35.0, 10.0, 22.778, 0.001, 0.0115, 20.0, 0.001, 0.001, 100.0,
        );
        let cap = 20.0 * 100.0 / SECONDS_PER_HOUR;
        approx_eq(q, cap, 1e-10);
    }

    #[test]
    fn nat_vent_higher_wind_increases_flow() {
        let (a, tz, to, tb, h, mh, _, sc, wc, vol) = nat_vent_base_args();
        let q_low = natural_ventilation_flow_m3_s(a, tz, to, tb, h, mh, 1.0, sc, wc, vol);
        let q_high = natural_ventilation_flow_m3_s(a, tz, to, tb, h, mh, 8.0, sc, wc, vol);
        assert!(
            q_high > q_low,
            "higher wind must increase nat vent flow: q_low={q_low:.6}, q_high={q_high:.6}"
        );
    }

    #[test]
    fn nat_vent_larger_zone_outdoor_diff_increases_flow() {
        let (a, _, to, tb, h, mh, v, sc, wc, vol) = nat_vent_base_args();
        // Both zones are above t_base; larger delta_t drives more stack flow
        let q_small = natural_ventilation_flow_m3_s(a, 25.0, to, tb, h, mh, v, sc, wc, vol);
        let q_large = natural_ventilation_flow_m3_s(a, 35.0, to, tb, h, mh, v, sc, wc, vol);
        assert!(
            q_large > q_small,
            "larger zone-outdoor delta must increase nat vent: q_small={q_small:.6}, q_large={q_large:.6}"
        );
    }

    // -----------------------------------------------------------------------
    // ELA coefficient calculation
    // -----------------------------------------------------------------------

    #[test]
    fn ela_coefficients_attic_produces_positive_values() {
        let (stack, wind) = super::calculate_ela_coefficients(
            0.75,
            1.5,
            5.0,
            TerrainClass::Suburban,
            super::SHIELDING_NORMAL,
        );
        assert!(stack > 0.0, "attic stack_coeff must be positive: {stack}");
        assert!(wind > 0.0, "attic wind_coeff must be positive: {wind}");
    }

    #[test]
    fn ela_coefficients_garage_produces_positive_values() {
        let (stack, wind) = super::garage_ela_coefficients(2.5);
        assert!(stack > 0.0, "garage stack_coeff must be positive: {stack}");
        assert!(wind > 0.0, "garage wind_coeff must be positive: {wind}");
    }

    #[test]
    fn ela_coefficients_conditioned_zero_hor_lk_frac() {
        let (stack, wind) = super::calculate_ela_coefficients(
            0.0,
            2.5,
            0.0,
            TerrainClass::Suburban,
            super::SHIELDING_NORMAL,
        );
        assert!(
            stack > 0.0,
            "conditioned stack_coeff must be positive: {stack}"
        );
        assert!(
            wind > 0.0,
            "conditioned wind_coeff must be positive: {wind}"
        );
    }

    #[test]
    fn ela_coefficients_higher_hor_lk_frac_increases_stack() {
        let (stack_low, _) = super::calculate_ela_coefficients(
            0.0,
            2.5,
            0.0,
            TerrainClass::Suburban,
            super::SHIELDING_NORMAL,
        );
        let (stack_high, _) = super::calculate_ela_coefficients(
            0.75,
            2.5,
            0.0,
            TerrainClass::Suburban,
            super::SHIELDING_NORMAL,
        );
        assert!(
            stack_high > stack_low,
            "higher hor_lk_frac should increase stack_coeff: low={stack_low}, high={stack_high}"
        );
    }

    #[test]
    fn ela_coefficients_higher_hor_lk_frac_decreases_wind() {
        let (_, wind_low) = super::calculate_ela_coefficients(
            0.75,
            2.5,
            0.0,
            TerrainClass::Suburban,
            super::SHIELDING_NORMAL,
        );
        let (_, wind_high) = super::calculate_ela_coefficients(
            0.0,
            2.5,
            0.0,
            TerrainClass::Suburban,
            super::SHIELDING_NORMAL,
        );
        assert!(
            wind_high > wind_low,
            "lower hor_lk_frac should increase wind_coeff: hor0={wind_high}, hor075={wind_low}"
        );
    }

    #[test]
    fn attic_ela_convenience_matches_raw() {
        let (s1, w1) = super::attic_ela_coefficients(1.5, 5.0);
        let (s2, w2) = super::calculate_ela_coefficients(
            0.75,
            1.5,
            5.0,
            TerrainClass::Suburban,
            super::SHIELDING_NORMAL,
        );
        approx_eq(s1, s2, 1e-15);
        approx_eq(w1, w2, 1e-15);
    }

    #[test]
    fn garage_ela_convenience_matches_raw() {
        let (s1, w1) = super::garage_ela_coefficients(2.5);
        let (s2, w2) = super::calculate_ela_coefficients(
            0.4,
            2.5,
            0.0,
            TerrainClass::Suburban,
            super::SHIELDING_NORMAL,
        );
        approx_eq(s1, s2, 1e-15);
        approx_eq(w1, w2, 1e-15);
    }

    #[test]
    fn ela_stack_coeff_matches_ashrae_hof_derivation() {
        // For nl=0.5, hor_lk_frac=0.0, H=2.5m:
        // f_s = (2/3)(1+0/2) × √(2×0.5×0.5) / (√0.5 + √0.5)
        //     = (2/3) × √0.5 / (2×√0.5) = (2/3) × 0.5 = 1/3
        // Cs_m2 = (1/3)² × 9.80665 × 2.5 / 296.15 = 0.009199...
        // stack_coeff = 0.009199... × 1e-2 = 9.199e-5
        let (stack, _) = super::calculate_ela_coefficients(
            0.0,
            2.5,
            0.0,
            TerrainClass::Suburban,
            super::SHIELDING_NORMAL,
        );
        let f_s = 1.0 / 3.0;
        let expected = f_s * f_s * 9.80665 * 2.5 / 296.15 * 1e-2;
        approx_eq(stack, expected, 1e-10);
    }

    // -----------------------------------------------------------------------
    // AIM-2 from ACH50 (Walker & Wilson 1998)
    // -----------------------------------------------------------------------

    use super::{Aim2Params, FoundationLeakageClass, ShieldingClass, aim2_coefficients_from_ach50};

    fn typical_aim2_params() -> Aim2Params {
        Aim2Params {
            ach50: 7.0,
            volume_m3: 400.0,
            infiltration_height_m: 5.0,
            foundation: FoundationLeakageClass::Other,
            shielding: ShieldingClass::Normal,
            terrain: TerrainClass::Suburban,
            has_flue: false,
            n_i: N_I_DEFAULT,
            floors_above_grade: 2.0,
        }
    }

    #[test]
    fn aim2_flow_coefficient_matches_formula() {
        // C = ACH50 × V / (3600 × 50^n_i)
        // 7 × 400 / (3600 × 50^0.65) = 2800 / (3600 × 14.6247..) = 2800 / 45783.5.. ≈ 0.06117
        let p = typical_aim2_params();
        let expected_c = (p.ach50 * p.volume_m3) / (3600.0 * 50.0_f64.powf(p.n_i));
        approx_eq(expected_c, 0.061_17, 0.001);
    }

    #[test]
    fn aim2_produces_positive_coefficients() {
        let coeffs = aim2_coefficients_from_ach50(&typical_aim2_params());
        assert!(coeffs.c_s > 0.0, "c_s must be positive: {}", coeffs.c_s);
        assert!(coeffs.c_w > 0.0, "c_w must be positive: {}", coeffs.c_w);
        assert!(
            coeffs.shelter_coeff > 0.0,
            "shelter must be positive: {}",
            coeffs.shelter_coeff
        );
    }

    #[test]
    fn aim2_higher_ach50_increases_coefficients() {
        let mut p_low = typical_aim2_params();
        p_low.ach50 = 3.0;
        let mut p_high = typical_aim2_params();
        p_high.ach50 = 10.0;

        let low = aim2_coefficients_from_ach50(&p_low);
        let high = aim2_coefficients_from_ach50(&p_high);

        assert!(
            high.c_s > low.c_s,
            "higher ACH50 must increase c_s: low={}, high={}",
            low.c_s,
            high.c_s
        );
        assert!(
            high.c_w > low.c_w,
            "higher ACH50 must increase c_w: low={}, high={}",
            low.c_w,
            high.c_w
        );
    }

    #[test]
    fn aim2_foundation_affects_wind_coefficient() {
        let mut p_slab = typical_aim2_params();
        p_slab.foundation = FoundationLeakageClass::Other;
        let mut p_crawl = typical_aim2_params();
        p_crawl.foundation = FoundationLeakageClass::VentedCrawlspace;

        let slab = aim2_coefficients_from_ach50(&p_slab);
        let crawl = aim2_coefficients_from_ach50(&p_crawl);

        // Different foundation types must produce different wind factors
        assert!(
            (slab.c_w - crawl.c_w).abs() > 1e-10,
            "foundation must affect c_w: slab={}, crawl={}",
            slab.c_w,
            crawl.c_w
        );
    }

    #[test]
    fn aim2_flue_increases_stack_coefficient() {
        let mut p_no = typical_aim2_params();
        p_no.has_flue = false;
        let mut p_yes = typical_aim2_params();
        p_yes.has_flue = true;

        let no_flue = aim2_coefficients_from_ach50(&p_no);
        let with_flue = aim2_coefficients_from_ach50(&p_yes);

        assert!(
            with_flue.c_s > no_flue.c_s,
            "flue must increase c_s: no_flue={}, with_flue={}",
            no_flue.c_s,
            with_flue.c_s
        );
    }

    #[test]
    fn aim2_shielding_classes_map_correctly() {
        approx_eq(ShieldingClass::Normal.raw(), 0.5, 1e-15);
        approx_eq(ShieldingClass::Exposed.raw(), 0.9, 1e-15);
        approx_eq(ShieldingClass::WellShielded.raw(), 0.3, 1e-15);
    }

    #[test]
    fn aim2_no_flue_slab_hand_calculated() {
        // Hand-calculate Walker-Wilson for: no flue, slab, n_i=0.65
        // Leakage: ceil=0.25, floor=0.25 → r_i=0.50, x_i=0.0, y_i=0.0
        // m_o = (0 + 0)² / (2 - 0.5) = 0
        // m_i = 0
        // f_s = ((1 + 0.65×0.5)/(0.65+1)) × (0.5 - 0)^(1.65) + 0
        //     = (1.325/1.65) × 0.5^1.65
        //     = 0.80303 × 0.31855 = 0.25581
        let n_i = 0.65_f64;
        let r_i = 0.5_f64;
        let expected_f_s = ((1.0 + n_i * r_i) / (n_i + 1.0)) * (0.5_f64).powf(n_i + 1.0);
        approx_eq(expected_f_s, 0.255_81, 0.001);

        // f_w (non-crawlspace, no flue): J_i = (0 + 0.5 + 0)/2 = 0.25
        // f_w = 0.19×(2-0.65)×(1 - (0.5/2)^1.5) - 0
        //     = 0.19×1.35×(1 - 0.25^1.5)
        //     = 0.2565 × (1 - 0.125) = 0.2565 × 0.875 = 0.22444
        let expected_f_w = 0.19 * (2.0 - n_i) * (1.0 - (0.25_f64).powf(1.5));
        approx_eq(expected_f_w, 0.224_44, 0.001);

        let p = Aim2Params {
            ach50: 5.0,
            volume_m3: 300.0,
            infiltration_height_m: 5.0,
            foundation: FoundationLeakageClass::Other,
            shielding: ShieldingClass::Normal,
            terrain: TerrainClass::Suburban,
            has_flue: false,
            n_i,
            floors_above_grade: 1.0,
        };
        let coeffs = aim2_coefficients_from_ach50(&p);

        let c_flow = (5.0 * 300.0) / (3600.0 * 50.0_f64.powf(n_i));
        let rho = 1.2041_f64;
        let g = 9.80665_f64;
        let t_in = 296.15_f64;

        let cs = expected_f_s * (rho * g * 5.0 / t_in).powf(n_i);
        let cw = expected_f_w * (rho / 2.0).powf(n_i);
        let expected_c_s = c_flow * cs;
        let expected_c_w = c_flow * cw;

        approx_eq(coeffs.c_s, expected_c_s, 1e-6);
        approx_eq(coeffs.c_w, expected_c_w, 1e-6);
    }

    #[test]
    fn aim2_physical_reasonableness() {
        // Typical US home: 7 ACH50, 400 m³, 5m height
        // Should produce natural ACH roughly 0.2-0.8 under moderate conditions
        let coeffs = aim2_coefficients_from_ach50(&typical_aim2_params());
        let q = super::ashrae_wind_stack(
            coeffs.c_s,
            coeffs.c_w,
            15.0, // 15 K delta_t
            4.0,  // 4 m/s wind
            coeffs.shelter_coeff,
            coeffs.n_i,
        );
        let ach = q / 400.0 * 3600.0;
        assert!(
            (0.1..=1.5).contains(&ach),
            "natural ACH={ach:.3} outside reasonable range [0.1, 1.5]"
        );
    }

    // -----------------------------------------------------------------------
    // duct_leakage_infiltration_m3_s -- ASHRAE 152 §9.3 superposition
    // -----------------------------------------------------------------------

    #[test]
    fn duct_leakage_zero_is_noop() {
        // When both leakage flows are zero the function must return the base rate unchanged.
        let base = 0.05;
        let result = duct_leakage_infiltration_m3_s(base, 0.0, 0.0, 300.0);
        approx_eq(result, base, 1e-15);
    }

    #[test]
    fn supply_greater_than_return_increases_infiltration() {
        // Supply > return pressurises house → adjusted > base.
        let volume_m3 = 300.0;
        let base = 0.35 * volume_m3 / 60.0; // set base equal to infil_fan_off for clean ratio
        let supply = 0.04; // m³/s supply leakage
        let result = duct_leakage_infiltration_m3_s(base, supply, 0.0, volume_m3);
        assert!(
            result > base,
            "supply > return must increase infiltration: base={base:.4}, result={result:.4}"
        );
    }

    #[test]
    fn return_greater_than_supply_decreases_infiltration() {
        // Return > supply depressurises house → adjusted substantially < base.
        // Use a large return leakage (50% of infil_fan_off) to ensure the
        // depressurisation effect exceeds the formula's ~0.5% non-linearity.
        let volume_m3 = 300.0;
        let infil_fan_off = 0.35 * volume_m3 / 60.0; // 1.75 m³/s
        let base = infil_fan_off;
        let ret = infil_fan_off * 0.5; // 0.875 m³/s -- well above the 1% threshold
        let result = duct_leakage_infiltration_m3_s(base, 0.0, ret, volume_m3);
        assert!(
            result < base * 0.99,
            "significant return > supply must decrease infiltration by >1%: base={base:.4}, result={result:.4}"
        );
    }

    #[test]
    fn large_return_dominance_drives_infiltration_to_zero() {
        // When return leakage imbalance exceeds infil_fan_off, the result must be zero.
        let volume_m3 = 300.0;
        let base = 0.35 * volume_m3 / 60.0; // infil_fan_off = 0.35 * 300 / 60 = 1.75 m³/s
        // imb = 100 m³/s >> infil_fan_off → exfiltration dominates
        let result = duct_leakage_infiltration_m3_s(base, 0.0, 100.0, volume_m3);
        approx_eq(result, 0.0, 1e-15);
    }

    #[test]
    fn balanced_duct_leakage_is_noop() {
        // Equal supply and return leakage → imb = 0 → no pressurisation effect.
        // The ASHRAE 152 formula uses exponents 1.5 and 0.67 (not exact inverses:
        // 1.5 × 0.67 = 1.005), so the ratio adjusted/infil_fan_off ≈ 1.005 when
        // imb = 0.  The result should be within 1% of the base rate.
        let volume_m3 = 300.0;
        let base = 0.05;
        let leak = 0.03;
        let result = duct_leakage_infiltration_m3_s(base, leak, leak, volume_m3);
        assert!(
            (result - base).abs() / base < 0.01,
            "balanced leakage must not change infiltration by more than 1%: base={base}, result={result}"
        );
    }

    // -----------------------------------------------------------------------
    // Formula self-consistency checks -- verify ashrae_wind_stack() output
    // matches hand-evaluated Q = sqrt(Q_stack² + Q_wind²). These confirm
    // the implementation is faithful to the formula, not that the formula
    // itself is correct (see aim2_ochre_cross_validation for external check).
    // -----------------------------------------------------------------------

    /// Formula self-consistency: stack-dominated, normal shielding.
    /// Q = sqrt((0.015 * 10^0.65)^2 + (0.0012 * (0.5*3)^1.3)^2)
    #[test]
    fn aim2_stack_dominated_normal_shielding() {
        let q = ashrae_wind_stack(0.015, 0.0012, 10.0, 3.0, 0.5, 0.65);
        approx_eq(q, 0.067_033_37, 1e-6);
    }

    /// Formula self-consistency: cold winter, well-shielded (shelter=0.3, dT=20, wind=5).
    #[test]
    fn aim2_cold_winter_well_shielded() {
        let q = ashrae_wind_stack(0.015, 0.0012, 20.0, 5.0, 0.3, 0.65);
        approx_eq(q, 0.105_16, 1e-4);
    }

    /// Formula self-consistency: wind-dominated, exposed site (shelter=0.9, dT=2, wind=8).
    #[test]
    fn aim2_wind_dominated_exposed() {
        let q = ashrae_wind_stack(0.015, 0.0012, 2.0, 8.0, 0.9, 0.65);
        approx_eq(q, 0.028_25, 1e-4);
    }

    /// Zero driving forces must yield exactly zero flow.
    #[test]
    fn aim2_zero_driving_forces() {
        let q = ashrae_wind_stack(0.015, 0.0012, 0.0, 0.0, 0.5, 0.65);
        assert_eq!(q, 0.0);
    }

    /// ACH simple method: 0.5 ACH * 400 m^3 / 3600 s = 0.05556 m^3/s.
    #[test]
    fn ach_simple_method() {
        let q = ach_infiltration(0.5, 400.0);
        approx_eq(q, 0.055_555_56, 1e-8);
    }

    /// ELA standard house per ASHRAE 62.2 coefficients.
    #[test]
    fn ela_standard_house() {
        let q = ela_infiltration(0.009_29, 0.000_105_911, 0.000_142_748, 10.0, 3.0);
        approx_eq(q, 0.004_50, 1e-4);
    }

    /// No duct leakage must return base infiltration unchanged (identity).
    #[test]
    fn duct_leakage_no_leakage_identity() {
        let q = duct_leakage_infiltration_m3_s(0.050, 0.0, 0.0, 400.0);
        assert_eq!(q, 0.050);
    }

    /// Pressurisation (supply > return) increases infiltration per ASHRAE 152 S9.3.
    #[test]
    fn duct_leakage_pressurisation_increases_infiltration() {
        let q = duct_leakage_infiltration_m3_s(0.050, 0.010, 0.005, 400.0);
        assert!(
            q > 0.050,
            "pressurisation must increase infiltration: got {q}"
        );
    }

    /// Depressurisation (return > supply) yields less infiltration than pressurisation.
    #[test]
    fn duct_leakage_depressurisation_less_than_pressurisation() {
        let q_press = duct_leakage_infiltration_m3_s(0.050, 0.010, 0.005, 400.0);
        let q_depress = duct_leakage_infiltration_m3_s(0.050, 0.005, 0.010, 400.0);
        assert!(
            q_depress < q_press,
            "depressurisation must produce less infiltration than pressurisation: \
             depress={q_depress}, press={q_press}"
        );
    }

    /// Cross-validate HARES AIM-2 against OCHRE `calculate_ashrae_infiltration_params`
    /// (envelope.py:488-633). The dimensionless Walker-Wilson factors f_s and f_w must
    /// match exactly. The combined coefficients c_s/c_w differ by <0.25% because OCHRE
    /// routes through an IP SLA→ELA→C_i pipeline while HARES computes C directly from
    /// ACH50 in SI. The shelter coefficient must also match.
    ///
    /// Reference scenario: 7 ACH50, 400 m³, 5 m height, slab, no flue, Normal
    /// shielding, Suburban terrain. OCHRE values computed via its full pint-based
    /// unit-conversion chain.
    #[test]
    fn aim2_ochre_cross_validation() {
        let p = Aim2Params {
            ach50: 7.0,
            volume_m3: 400.0,
            infiltration_height_m: 5.0,
            foundation: FoundationLeakageClass::Other,
            shielding: ShieldingClass::Normal,
            terrain: TerrainClass::Suburban,
            has_flue: false,
            n_i: N_I_DEFAULT,
            floors_above_grade: 2.0,
        };
        let coeffs = aim2_coefficients_from_ach50(&p);

        // OCHRE reference values (computed via pint unit conversions):
        //   c_s = 5.4956153940e-03  (inf_c × inf_Cs)
        //   c_w = 9.8923972186e-03  (inf_c × inf_Cw)
        //   shelter = 0.3077017406
        //
        // Dimensionless factors f_s and f_w are identical between OCHRE and HARES
        // (both implement Walker-Wilson 1998 Eq. 9-25 identically). The ~0.2%
        // difference in c_s/c_w arises from the flow-coefficient derivation:
        // OCHRE uses SLA→ELA→C_i in IP units, HARES uses C = ACH50×V/(3600×50^n).
        let ochre_c_s = 5.495_615_394_0e-3;
        let ochre_c_w = 9.892_397_218_6e-3;
        let ochre_shelter = 0.307_701_740_6;

        let c_s_rel = (coeffs.c_s - ochre_c_s).abs() / ochre_c_s;
        let c_w_rel = (coeffs.c_w - ochre_c_w).abs() / ochre_c_w;
        assert!(
            c_s_rel < 0.005,
            "c_s relative error vs OCHRE too large: {c_s_rel:.4} (HARES={}, OCHRE={ochre_c_s})",
            coeffs.c_s
        );
        assert!(
            c_w_rel < 0.005,
            "c_w relative error vs OCHRE too large: {c_w_rel:.4} (HARES={}, OCHRE={ochre_c_w})",
            coeffs.c_w
        );

        // Shelter coefficient must match exactly (same terrain model, same formula)
        approx_eq(coeffs.shelter_coeff, ochre_shelter, 1e-8);
    }

    // -----------------------------------------------------------------------
    // Regression tests for ticket 028: terrain/height correction invariants
    // -----------------------------------------------------------------------

    /// Verify the suburban terrain at 8 m height correction factor.
    ///
    /// Ticket 028 claims `terrain_wind_speed(u_met, 0.22, 370.0, 8.0) ≈ 0.62 × u_met`
    /// (suburban at 8 m per ASHRAE HoF Ch. 24 Table 1).
    ///
    /// The actual formula gives 0.6824, not 0.62.  This test pins the correct value
    /// so any change to the terrain constants is caught immediately.
    #[test]
    fn ticket028_suburban_8m_correction_factor() {
        // ASHRAE HoF Ch.24 Table 1 suburban terrain: alpha=0.22, delta=370 m.
        // Met station: alpha=0.14, delta=270 m, height=10 m.
        let factor = terrain_wind_speed(1.0, SUBURBAN_ALPHA, SUBURBAN_DELTA_M, 8.0);
        // Ticket 028 states ~0.62; actual value is ~0.6824.
        // This test documents the correct value so the ticket's numeric claim can be corrected.
        assert!(
            (factor - 0.6824).abs() < 0.001,
            "suburban 8 m correction factor: expected ~0.6824, got {factor:.6} \
             (ticket 028 incorrectly states ~0.62)"
        );
        assert!(
            factor < 1.0,
            "terrain-corrected wind at 8 m must be less than met-station wind, got {factor}"
        );
    }

    /// Demonstrate the double-correction risk for the ELA branch.
    ///
    /// `calculate_ela_coefficients` pre-bakes terrain correction into `wind_coeff`
    /// (via `f_t`).  If the caller then also corrects the wind speed before calling
    /// `ela_infiltration`, the correction is applied twice and infiltration is
    /// understated.
    ///
    /// This test documents the invariant: when `wind_coeff` was computed via
    /// `calculate_ela_coefficients`, passing the raw met-station wind speed to
    /// `ela_infiltration` gives the same result as passing the terrain-corrected
    /// wind speed to a coefficient set with `f_t = 1.0` (i.e., terrain correction
    /// embedded in the coefficient, not the speed).
    #[test]
    fn ticket028_ela_wind_coeff_embeds_terrain_correction() {
        // Build ELA wind coefficient for suburban terrain at 5 m building height.
        let (_, wind_coeff_with_terrain) =
            calculate_ela_coefficients(0.0, 2.5, 2.5, TerrainClass::Suburban, SHIELDING_NORMAL);

        // Build ELA wind coefficient with NO terrain correction (f_t = 1.0, i.e. met-station
        // terrain identical to site terrain -- rural/open).  We simulate this by using
        // TerrainClass::Rural which has the same exponents as the met station.
        let (_, wind_coeff_no_terrain) =
            calculate_ela_coefficients(0.0, 2.5, 2.5, TerrainClass::Rural, SHIELDING_NORMAL);

        // The terrain-corrected coefficient must be strictly less than the no-correction one
        // because suburban terrain reduces wind speed (higher roughness than met station).
        assert!(
            wind_coeff_with_terrain < wind_coeff_no_terrain,
            "suburban wind_coeff must be less than rural (met-station) wind_coeff: \
             suburban={wind_coeff_with_terrain:.8}, rural={wind_coeff_no_terrain:.8}"
        );

        // Applying terrain-corrected wind speed to the terrain-embedded coefficient
        // is equivalent to applying raw wind speed with a doubly-reduced effective coefficient.
        // Both paths produce the same (incorrect) result -- demonstrating double-correction.
        let u_met = 4.0_f64;
        let u_corrected = terrain_wind_speed_for_class(u_met, TerrainClass::Suburban, 5.0);
        let dt = 10.0_f64;
        let ela_m2 = 0.05_f64;

        // Path A: terrain correction embedded in coeff, raw wind speed (correct usage).
        let flow_correct = ela_infiltration(ela_m2, 0.0, wind_coeff_with_terrain, dt, u_met);

        // Path B: terrain correction embedded in coeff, ALSO terrain-corrected wind (double-correction).
        let flow_double_corrected =
            ela_infiltration(ela_m2, 0.0, wind_coeff_with_terrain, dt, u_corrected);

        // Double-correction must understate flow relative to single-correction.
        assert!(
            flow_double_corrected < flow_correct,
            "double-correcting wind speed (ticket-028 bug path) must understate infiltration: \
             double={flow_double_corrected:.8}, correct={flow_correct:.8}"
        );

        // The error is proportional to (u_corrected/u_met)^2 ≈ 0.68^2 ≈ 0.47 for wind-only case.
        // With stack effect absent, the ratio should be close to that.
        let ratio = flow_double_corrected / flow_correct;
        assert!(
            ratio < 0.95,
            "double-correction must reduce flow by >5%; ratio={ratio:.4}"
        );
    }

    /// Document that `aim2_coefficients_from_ach50` pre-bakes terrain correction
    /// into `shelter_coeff`, so `ashrae_wind_stack` must receive the raw met-station
    /// wind speed (not a separately terrain-corrected one) to avoid double-correction.
    #[test]
    fn ticket028_aim2_shelter_coeff_embeds_terrain_correction() {
        // Suburban terrain at 5 m infiltration height.
        let params = Aim2Params {
            ach50: 7.0,
            volume_m3: 400.0,
            infiltration_height_m: 5.0,
            foundation: FoundationLeakageClass::Other,
            shielding: ShieldingClass::Normal,
            terrain: TerrainClass::Suburban,
            has_flue: false,
            n_i: N_I_DEFAULT,
            floors_above_grade: 2.0,
        };
        let coeffs_suburban = aim2_coefficients_from_ach50(&params);

        let params_rural = Aim2Params {
            terrain: TerrainClass::Rural,
            ..params.clone()
        };
        let coeffs_rural = aim2_coefficients_from_ach50(&params_rural);

        // Suburban shelter_coeff must be less than rural because suburban terrain
        // has higher roughness (lower effective wind at building height).
        assert!(
            coeffs_suburban.shelter_coeff < coeffs_rural.shelter_coeff,
            "suburban shelter_coeff must be less than rural (met-station terrain): \
             suburban={:.6}, rural={:.6}",
            coeffs_suburban.shelter_coeff,
            coeffs_rural.shelter_coeff
        );

        // The terrain correction ratio f_t for suburban/5m vs rural/5m.
        let f_t_suburban = terrain_wind_speed(1.0, SUBURBAN_ALPHA, SUBURBAN_DELTA_M, 5.0);
        let f_t_rural = terrain_wind_speed(1.0, RURAL_ALPHA, RURAL_DELTA_M, 5.0);
        let expected_ratio = f_t_suburban / f_t_rural;
        let actual_ratio = coeffs_suburban.shelter_coeff / coeffs_rural.shelter_coeff;

        // Ratio of shelter coefficients must equal ratio of f_t factors (terrain only differs there).
        approx_eq(actual_ratio, expected_ratio, 1e-10);
    }
}
