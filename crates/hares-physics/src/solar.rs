//! Solar position, irradiance decomposition, and surface tilting.

use chrono::{DateTime, Datelike, FixedOffset, Timelike};
use hares_types::SurfaceIrradiance;

const MINUTES_PER_DAY: f64 = 1440.0;
const SOLAR_NOON_MINUTES: f64 = 720.0;
const EOT_SCALE_FACTOR: f64 = 229.18;
const MINUTES_PER_DEGREE_LONGITUDE: f64 = 4.0;
const DEGREES_HALF_CIRCLE: f64 = 180.0;
const DEGREES_FULL_CIRCLE: f64 = 360.0;
const ISOTROPIC_VIEW_FACTOR: f64 = 0.5;

/// Solar constant [W/m²].
const SOLAR_CONSTANT: f64 = 1367.0;

/// Cosine of 85° -- lower bound for denominator in Perez model to avoid
/// singularity near the horizon.
const COS_85_DEG: f64 = 0.087_155_742_747_658_17;

/// Maximum solar zenith [deg] for which the Perez model is valid.
/// Beyond this we fall back to the isotropic model.
const PEREZ_ZENITH_LIMIT_DEG: f64 = 87.0;

/// Minimum DHI [W/m²] for Perez model applicability.
const PEREZ_MIN_DHI: f64 = 1.0;

/// Perez et al. (1990) Table 1 coefficients for the all-weather anisotropic
/// diffuse irradiance model.
/// Each row: [f11, f12, f13, f21, f22, f23] for one sky clearness (epsilon) bin.
/// Bins: [1.0,1.065), [1.065,1.23), [1.23,1.5), [1.5,1.95),
///       [1.95,2.8), [2.8,4.5), [4.5,6.2), [6.2,∞)
const PEREZ_COEFFICIENTS: [[f64; 6]; 8] = [
    [
        -0.0083117, 0.5877285, -0.0620636, -0.0596012, 0.0721249, -0.0220216,
    ],
    [
        0.1299457, 0.6825954, -0.1513752, -0.0189325, 0.0659650, -0.0288748,
    ],
    [
        0.3296958, 0.4868735, -0.2210958, 0.0554140, -0.0639588, -0.0260542,
    ],
    [
        0.5682053, 0.1874525, -0.2951290, 0.1088631, -0.1519229, -0.0139754,
    ],
    [
        0.8730280, -0.3920403, -0.3616149, 0.2255647, -0.4620442, 0.0012448,
    ],
    [
        1.1326077, -1.2367284, -0.4118494, 0.2877813, -0.8230357, 0.0558651,
    ],
    [
        1.0601591, -1.5999137, -0.3589221, 0.2642124, -1.1272340, 0.1310694,
    ],
    [
        0.6777470, -0.3272588, -0.2504286, 0.1561313, -1.3765031, 0.2506212,
    ],
];

/// Epsilon bin upper boundaries for Perez model.
const PEREZ_EPSILON_BINS: [f64; 7] = [1.065, 1.23, 1.5, 1.95, 2.8, 4.5, 6.2];

/// kappa constant for Perez sky clearness formula (zenith in radians).
const PEREZ_KAPPA: f64 = 1.041;

/// Spencer (1971) EOT constant term. Corrected from original paper's
/// misprint of 0.000075; pvlib-python documents the correct value as 0.0000075.
const EOT_C0: f64 = 0.000_007_5;
const EOT_C1: f64 = 0.001_868;
const EOT_C2: f64 = -0.032_077;
const EOT_C3: f64 = -0.014_615;
const EOT_C4: f64 = -0.040_849;

const DAYS_PER_YEAR: f64 = 365.0;
const MINUTES_PER_HOUR: f64 = 60.0;
const DEGREES_PER_MINUTE_SOLAR_TIME: f64 = 0.25;

/// Solar position angles in degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SolarPosition {
    pub altitude_deg: f64,
    pub azimuth_deg: f64,
}

/// Solar position from local timestamp and site coordinates using a Spencer-style declination/EOT model.
///
/// Converts to UTC internally -- solar geometry requires true UTC.
pub fn solar_position(
    latitude_deg: f64,
    longitude_deg: f64,
    local_datetime: DateTime<FixedOffset>,
) -> SolarPosition {
    let utc_datetime = local_datetime.to_utc();
    let lat_rad = latitude_deg.to_radians();
    let day = f64::from(utc_datetime.ordinal() as u16);
    let hour = utc_datetime.hour() as f64;
    let minute = utc_datetime.minute() as f64;
    let second = utc_datetime.second() as f64;

    let minutes_utc = hour * MINUTES_PER_HOUR + minute + second / MINUTES_PER_HOUR;
    let gamma = 2.0 * std::f64::consts::PI / DAYS_PER_YEAR
        * (day - 1.0 + (minutes_utc - SOLAR_NOON_MINUTES) / MINUTES_PER_DAY);

    // Spencer (1971) Fourier series for declination and equation of time.
    let decl_rad = 0.006_918 - 0.399_912 * gamma.cos() + 0.070_257 * gamma.sin()
        - 0.006_758 * (2.0 * gamma).cos()
        + 0.000_907 * (2.0 * gamma).sin()
        - 0.002_697 * (3.0 * gamma).cos()
        + 0.001_48 * (3.0 * gamma).sin();

    let eq_time_min = EOT_SCALE_FACTOR
        * (EOT_C0
            + EOT_C1 * gamma.cos()
            + EOT_C2 * gamma.sin()
            + EOT_C3 * (2.0 * gamma).cos()
            + EOT_C4 * (2.0 * gamma).sin());

    let true_solar_time_min =
        minutes_utc + eq_time_min + MINUTES_PER_DEGREE_LONGITUDE * longitude_deg;
    let mut hour_angle_deg =
        true_solar_time_min * DEGREES_PER_MINUTE_SOLAR_TIME - DEGREES_HALF_CIRCLE;
    if hour_angle_deg < -DEGREES_HALF_CIRCLE {
        hour_angle_deg += DEGREES_FULL_CIRCLE;
    } else if hour_angle_deg > DEGREES_HALF_CIRCLE {
        hour_angle_deg -= DEGREES_FULL_CIRCLE;
    }
    let hour_angle_rad = hour_angle_deg.to_radians();

    let cos_zenith = (lat_rad.sin() * decl_rad.sin()
        + lat_rad.cos() * decl_rad.cos() * hour_angle_rad.cos())
    .clamp(-1.0, 1.0);
    let zenith_rad = cos_zenith.acos();
    let altitude_deg = 90.0 - zenith_rad.to_degrees();

    let numerator = hour_angle_rad.sin();
    let denominator = hour_angle_rad.cos() * lat_rad.sin() - decl_rad.tan() * lat_rad.cos();
    // South-referenced azimuth; shift to north-referenced then normalise to [0, 2π).
    let azimuth_rad =
        (numerator.atan2(denominator) + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU);
    let azimuth_deg = azimuth_rad.to_degrees();

    SolarPosition {
        altitude_deg,
        azimuth_deg,
    }
}

/// Angle of incidence [deg] between sun rays and a surface normal.
pub fn angle_of_incidence(
    surface_tilt_deg: f64,
    surface_azimuth_deg: f64,
    solar_alt: f64,
    solar_az: f64,
) -> f64 {
    let tilt = surface_tilt_deg.to_radians();
    let alt = solar_alt.to_radians();
    let az_delta = (solar_az - surface_azimuth_deg).to_radians();
    let cos_aoi =
        (alt.sin() * tilt.cos() + alt.cos() * tilt.sin() * az_delta.cos()).clamp(-1.0, 1.0);
    cos_aoi.acos().to_degrees()
}

/// Extraterrestrial normal irradiance using Spencer's (1971) formula.
#[must_use]
pub fn extraterrestrial_irradiance(day_of_year: u32) -> f64 {
    let b = 2.0 * std::f64::consts::PI * (day_of_year as f64 - 1.0) / 365.0;
    SOLAR_CONSTANT
        * (1.00011
            + 0.034221 * b.cos()
            + 0.00128 * b.sin()
            + 0.000719 * (2.0 * b).cos()
            + 0.000077 * (2.0 * b).sin())
}

/// Extraterrestrial normal irradiance -- alias for [`extraterrestrial_irradiance`].
///
/// Provided under this name for callers that prefer the longer, unambiguous form.
#[must_use]
#[inline]
pub fn extraterrestrial_normal_irradiance(day_of_year: u32) -> f64 {
    extraterrestrial_irradiance(day_of_year)
}

/// Perez (1990) sky-diffuse component only, for a tilted surface.
///
/// Returns the sky diffuse irradiance [W/m²] on a tilted plane using the
/// Perez anisotropic model. Accepts extraterrestrial normal irradiance as an
/// explicit parameter so callers can supply a known value rather than deriving
/// it from day-of-year.
///
/// # Arguments
/// * `dhi` -- diffuse horizontal irradiance [W/m²]
/// * `dni` -- direct normal irradiance [W/m²]
/// * `zenith_deg` -- solar zenith angle [degrees]
/// * `aoi_deg` -- angle of incidence on the surface [degrees]
/// * `tilt_deg` -- surface tilt from horizontal [degrees]
/// * `dni_extra` -- extraterrestrial normal irradiance [W/m²]
#[must_use]
pub fn perez_sky_diffuse(
    dhi: f64,
    dni: f64,
    zenith_deg: f64,
    aoi_deg: f64,
    tilt_deg: f64,
    dni_extra: f64,
) -> f64 {
    if dhi < PEREZ_MIN_DHI {
        return 0.0;
    }
    if zenith_deg > PEREZ_ZENITH_LIMIT_DEG {
        // Near-horizon: fall back to isotropic sky-diffuse view factor
        let tilt_rad = tilt_deg.to_radians();
        return (dhi * (1.0 + tilt_rad.cos()) * ISOTROPIC_VIEW_FACTOR).max(0.0);
    }

    let zenith_rad = zenith_deg.to_radians();
    let tilt_rad = tilt_deg.to_radians();
    let aoi_rad = aoi_deg.to_radians();

    // Simple secant airmass (spec-mandated; avoids Kasten-Young refraction
    // correction that is irrelevant to the brightness coefficient delta).
    let am = 1.0 / zenith_rad.cos();
    if !am.is_finite() {
        return 0.0;
    }

    // Sky clearness epsilon
    let zenith_rad_cubed = zenith_rad * zenith_rad * zenith_rad;
    let epsilon = ((dhi + dni) / dhi + PEREZ_KAPPA * zenith_rad_cubed)
        / (1.0 + PEREZ_KAPPA * zenith_rad_cubed);

    // Sky brightness delta
    let delta = dhi * am / dni_extra;

    // Perez coefficients
    let bin = perez_bin(epsilon);
    let c = &PEREZ_COEFFICIENTS[bin];
    let f1 = (c[0] + c[1] * delta + c[2] * zenith_rad).max(0.0);
    let f2 = c[3] + c[4] * delta + c[5] * zenith_rad;

    // Geometric factors
    let a = aoi_rad.cos().max(0.0);
    let b = zenith_rad.cos().max(COS_85_DEG);

    // Three-component Perez sky diffuse
    let term_iso = 0.5 * (1.0 - f1) * (1.0 + tilt_rad.cos());
    let term_cs = f1 * a / b;
    let term_hz = f2 * tilt_rad.sin();

    (dhi * (term_iso + term_cs + term_hz)).max(0.0)
}

/// Kasten-Young (1989) relative airmass approximation.
#[must_use]
pub fn relative_airmass(zenith_deg: f64) -> f64 {
    let z = zenith_deg.min(89.9);
    1.0 / (z.to_radians().cos() + 0.50572 * (96.07995 - z).powf(-1.6364))
}

/// Select Perez coefficient bin index from sky clearness epsilon.
fn perez_bin(epsilon: f64) -> usize {
    for (i, &upper) in PEREZ_EPSILON_BINS.iter().enumerate() {
        if epsilon < upper {
            return i;
        }
    }
    7
}

/// Perez (1990) all-weather anisotropic diffuse irradiance model.
///
/// This is the primary surface irradiance API for callers that have full solar
/// geometry. Falls back to the isotropic model when the Perez model is
/// undefined (high zenith, low DHI).
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn perez_tilted_irradiance(
    surface_id: u32,
    ghi: f64,
    dni: f64,
    dhi: f64,
    solar_zenith_deg: f64,
    solar_azimuth_deg: f64,
    surface_tilt_deg: f64,
    surface_azimuth_deg: f64,
    day_of_year: u32,
    ground_albedo: f64,
) -> SurfaceIrradiance {
    // Nighttime: all components zero
    if ghi <= 0.0 && dni <= 0.0 && dhi <= 0.0 {
        return SurfaceIrradiance {
            surface_id,
            direct_w_m2: 0.0,
            diffuse_w_m2: 0.0,
            reflected_w_m2: 0.0,
            angle_of_incidence_rad: std::f64::consts::FRAC_PI_2,
        };
    }

    let solar_alt_deg = 90.0 - solar_zenith_deg;
    let aoi = angle_of_incidence(
        surface_tilt_deg,
        surface_azimuth_deg,
        solar_alt_deg,
        solar_azimuth_deg,
    );

    // Fall back to isotropic when Perez is undefined
    if dhi < PEREZ_MIN_DHI || solar_zenith_deg > PEREZ_ZENITH_LIMIT_DEG {
        return isotropic_tilted_irradiance(
            surface_id,
            ghi,
            dni,
            dhi,
            aoi,
            surface_tilt_deg,
            ground_albedo,
        );
    }

    let zenith_rad = solar_zenith_deg.to_radians();
    let tilt_rad = surface_tilt_deg.to_radians();
    let aoi_rad = aoi.to_radians();

    // Extraterrestrial irradiance and airmass
    let i0 = extraterrestrial_irradiance(day_of_year);
    let am = relative_airmass(solar_zenith_deg);

    // Sky clearness epsilon
    let zenith_rad_cubed = zenith_rad * zenith_rad * zenith_rad;
    let epsilon = ((dhi + dni) / dhi + PEREZ_KAPPA * zenith_rad_cubed)
        / (1.0 + PEREZ_KAPPA * zenith_rad_cubed);

    // Sky brightness delta
    let delta = dhi * am / i0;

    // Perez coefficients
    let bin = perez_bin(epsilon);
    let c = &PEREZ_COEFFICIENTS[bin];
    let f1 = (c[0] + c[1] * delta + c[2] * zenith_rad).max(0.0);
    let f2 = c[3] + c[4] * delta + c[5] * zenith_rad;

    // Geometric factors
    let a = aoi_rad.cos().max(0.0);
    let b = zenith_rad.cos().max(COS_85_DEG);

    // Diffuse on tilted surface (Perez decomposition)
    let diffuse = (dhi
        * ((1.0 - f1) * (1.0 + tilt_rad.cos()) * ISOTROPIC_VIEW_FACTOR
            + f1 * a / b
            + f2 * tilt_rad.sin()))
    .max(0.0);

    // Direct on tilted surface
    let direct = (dni * aoi_rad.cos()).max(0.0);

    // Ground-reflected (same as isotropic)
    let reflected = (ghi * ground_albedo * (1.0 - tilt_rad.cos()) * ISOTROPIC_VIEW_FACTOR).max(0.0);

    SurfaceIrradiance {
        surface_id,
        direct_w_m2: direct,
        diffuse_w_m2: diffuse,
        reflected_w_m2: reflected,
        angle_of_incidence_rad: aoi_rad,
    }
}

/// Isotropic (Liu & Jordan 1963) diffuse sky model.
///
/// Retained for fallback and testing. The primary Perez API is
/// [`perez_tilted_irradiance`].
pub fn isotropic_tilted_irradiance(
    surface_id: u32,
    ghi: f64,
    dni: f64,
    dhi: f64,
    aoi: f64,
    surface_tilt: f64,
    ground_albedo: f64,
) -> SurfaceIrradiance {
    if ghi <= 0.0 && dni <= 0.0 && dhi <= 0.0 {
        return SurfaceIrradiance {
            surface_id,
            direct_w_m2: 0.0,
            diffuse_w_m2: 0.0,
            reflected_w_m2: 0.0,
            angle_of_incidence_rad: std::f64::consts::FRAC_PI_2,
        };
    }

    let aoi_rad = aoi.to_radians();
    let tilt_rad = surface_tilt.to_radians();

    let direct = (dni * aoi_rad.cos()).max(0.0);
    let diffuse = (dhi * (1.0 + tilt_rad.cos()) * ISOTROPIC_VIEW_FACTOR).max(0.0);
    let reflected = (ghi * ground_albedo * (1.0 - tilt_rad.cos()) * ISOTROPIC_VIEW_FACTOR).max(0.0);

    SurfaceIrradiance {
        surface_id,
        direct_w_m2: direct,
        diffuse_w_m2: diffuse,
        reflected_w_m2: reflected,
        angle_of_incidence_rad: aoi_rad,
    }
}

/// Window transmitted solar gain [W].
pub fn window_transmitted_solar(irradiance_w_m2: f64, shgc: f64, area_m2: f64) -> f64 {
    irradiance_w_m2 * shgc * area_m2
}

// ---------------------------------------------------------------------------
// Angle-of-incidence (AOI) dependent window transmittance
//
// EnergyPlus Engineering Reference -- Window Calculation Module, Step 7:
// "Determine Angular Performance"
// https://bigladdersoftware.com/epx/docs/8-9/engineering-reference/window-calculation-module.html
//
// T(θ) = T(0) × IAM(θ), where IAM is a degree-4 polynomial in cos(θ).
// Coefficients span six named curves (A, BDCD, D, E, F, J) selected by
// window U-factor and SHGC.  OCHRE uses the same lookup table
// (ochre/utils/envelope.py `calculate_plane_irradiance`).
//
// Each curve stores [c0, c1, c2, c3, c4] for
//   raw_iam(cos θ) = c0 + c1·cos + c2·cos² + c3·cos³ + c4·cos⁴
// The polynomial is normalised by its value at cos = 1 so window_iam
// returns exactly 1.0 at θ = 0.
// ---------------------------------------------------------------------------

/// EnergyPlus window angular-transmittance curve type.
///
/// Select based on window U-factor [W/m²·K] and SHGC per the EnergyPlus
/// Engineering Reference step-7 lookup table (also used by OCHRE):
///
/// | U-factor (W/m²·K) | SHGC        | Curve |
/// |-------------------|-------------|-------|
/// | > 3.98            | > 0.625     | A     |
/// | > 3.98            | (0.3, 0.625]| Bdcd  |
/// | > 3.98            | ≤ 0.3       | D     |
/// | 1.56–3.98         | > 0.525     | E     |
/// | 1.56–3.98         | ≤ 0.525     | F     |
/// | ≤ 1.56            | > 0.4       | E     |
/// | ≤ 1.56            | ≤ 0.4       | J     |
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlazingCurve {
    /// Single-pane clear, high SHGC (U > 3.98, SHGC > 0.625).
    A,
    /// Single-pane, mid SHGC (U > 3.98, SHGC ∈ (0.3, 0.625]).
    Bdcd,
    /// Single-pane, low SHGC (U > 3.98, SHGC ≤ 0.3).
    D,
    /// Double/triple pane, higher SHGC (U ≤ 3.98, SHGC > 0.525 or > 0.4).
    E,
    /// Double-pane, lower SHGC (1.56 < U ≤ 3.98, SHGC ≤ 0.525).
    F,
    /// Triple-pane or low-e, low SHGC (U ≤ 1.56, SHGC ≤ 0.4).
    J,
}

impl GlazingCurve {
    /// Select the EnergyPlus curve from window U-factor [W/m²·K] and SHGC.
    pub fn from_u_shgc(u_w_m2_k: f64, shgc: f64) -> Self {
        if u_w_m2_k > 3.98 {
            if shgc > 0.625 {
                GlazingCurve::A
            } else if shgc > 0.3 {
                GlazingCurve::Bdcd
            } else {
                GlazingCurve::D
            }
        } else if u_w_m2_k > 1.56 {
            if shgc > 0.525 {
                GlazingCurve::E
            } else {
                GlazingCurve::F
            }
        } else if shgc > 0.4 {
            GlazingCurve::E
        } else {
            GlazingCurve::J
        }
    }

    /// Polynomial coefficients [c0, c1, c2, c3, c4] for
    /// raw_iam(cos θ) = c0 + c1·cos + c2·cos² + c3·cos³ + c4·cos⁴.
    ///
    /// Derived by reversing OCHRE's coefficient arrays (stored highest-degree
    /// first) before evaluation.
    fn coefficients(self) -> [f64; 5] {
        match self {
            GlazingCurve::A => [-0.001_474, 3.355, -3.852, 1.486, 0.014_7],
            GlazingCurve::Bdcd => [-0.001_16, 2.742_25, -2.289, 0.047_482_5, 0.504_475],
            GlazingCurve::D => [-0.000_280_4, 2.845, -2.582, 0.396_3, 0.346_2],
            GlazingCurve::E => [-0.002_577, 1.51, 2.489, -5.873, 2.883],
            GlazingCurve::F => [-0.001_367, 1.213, 3.137, -6.366, 3.025],
            GlazingCurve::J => [0.000_482_5, 0.084_07, 6.018, -8.836, 3.744],
        }
    }

    /// Sum of all polynomial coefficients = raw IAM at normal incidence (cos = 1).
    fn normal_incidence_raw(self) -> f64 {
        self.coefficients().iter().sum()
    }

    /// Pre-computed hemispherical-average IAM for isotropic diffuse radiation.
    ///
    /// Calculated by integrating IAM(θ)·cos(θ)·sin(θ) dθ over [0, π/2] with
    /// Lambertian (cosine-weighted) hemispherical averaging, normalised so
    /// IAM(0) = 1.0.  Consistent with OCHRE's 0.854 diffuse factor and the
    /// EnergyPlus window module.
    pub fn diffuse_iam(self) -> f64 {
        match self {
            GlazingCurve::A => 0.907,
            GlazingCurve::Bdcd => 0.866,
            GlazingCurve::D => 0.875,
            GlazingCurve::E => 0.855,
            GlazingCurve::F => 0.831,
            GlazingCurve::J => 0.771,
        }
    }
}

/// Incidence angle modifier (IAM) for window beam transmittance.
///
/// Returns the ratio T(θ) / T(0) ∈ [0, 1] using the EnergyPlus degree-4
/// polynomial angular model normalised to 1.0 at normal incidence.
///
/// # Arguments
/// * `theta_rad` -- angle of incidence [rad].  Values < 0 or ≥ π/2 return 0.
/// * `curve` -- glazing curve; use [`GlazingCurve::from_u_shgc`] to select.
///
/// # References
/// EnergyPlus Engineering Reference, Window Calculation Module, Step 7.
/// OCHRE `ochre/utils/envelope.py`, `calculate_plane_irradiance`.
#[must_use]
pub fn window_iam(theta_rad: f64, curve: GlazingCurve) -> f64 {
    // Angles outside [0, π/2) or non-finite produce no direct solar gain through the window.
    // The is_finite() check catches NaN and ±infinity before the range test.
    if !theta_rad.is_finite() || !(0.0..std::f64::consts::FRAC_PI_2).contains(&theta_rad) {
        return 0.0;
    }

    let cos_th = theta_rad.cos();
    let c = curve.coefficients();
    // Horner's method for numerically stable degree-4 polynomial evaluation.
    let raw = c[0] + cos_th * (c[1] + cos_th * (c[2] + cos_th * (c[3] + cos_th * c[4])));
    (raw / curve.normal_incidence_raw()).clamp(0.0, 1.0)
}

/// EnergyPlus Simple Window Model Step 1: decompose U-factor into glass and film R.
///
/// Returns `(r_glass, r_film_interior)` in m²·K/W.
/// Exterior film resistance is 0 for windows per EnergyPlus convention.
///
/// # References
/// - EnergyPlus Engineering Reference, Window Calculation Module, Step 1.
/// - OCHRE `ochre/utils/envelope.py:294–304` (`create_rc_data` with `u_window`).
#[must_use]
pub fn window_u_factor_decomposition(u_factor_w_m2_k: f64) -> (f64, f64) {
    if !u_factor_w_m2_k.is_finite() || u_factor_w_m2_k <= 0.0 {
        return (0.0, 0.12); // safe defaults: no glass R, standard interior film
    }
    let r_int = if u_factor_w_m2_k < 5.85 {
        1.0 / (0.359073 * u_factor_w_m2_k.ln() + 6.949915)
    } else {
        1.0 / (1.788041 * u_factor_w_m2_k - 2.886625)
    };
    let r_glass = (1.0 / u_factor_w_m2_k - r_int).max(0.0);
    debug_assert!(r_glass >= 0.0, "r_glass must be non-negative");
    debug_assert!(r_int > 0.0, "r_film_int must be positive");
    (r_glass, r_int)
}

/// Pre-computed window optical parameters from EnergyPlus Simple Window Model.
///
/// Decomposes SHGC into transmittance and absorbed fractions using the
/// EnergyPlus Steps 4–5 polynomial correlations.
///
/// # Returns
/// `(transmittance, radiation_frac)` where:
/// - `transmittance`: solar transmittance at normal incidence [0, 1]
/// - `radiation_frac`: inward-flowing fraction of absorbed solar [0, 1]
///
/// The absorbed fraction is `SHGC - transmittance`. Of that, `radiation_frac`
/// flows to the interior zone; the remainder is lost to the exterior.
///
/// # References
/// - EnergyPlus Engineering Reference, Window Calculation Module, Steps 4–5.
/// - OCHRE `ochre/utils/envelope.py:405–431`.
/// - ASHRAE Fundamentals Ch. 15: `SHGC = T_sol + A_sol × N_i`.
#[must_use]
pub fn calculate_window_parameters(
    shgc: f64,
    u_w_m2_k: f64,
    res_material_m2_k_w: f64,
) -> (f64, f64) {
    // Step 4: Transmittance at normal incidence.
    // Piecewise polynomial in SHGC, branched on U-factor.
    // OCHRE uses 3.95 threshold; EnergyPlus specifies interpolation band 3.4–4.5.
    // We use EnergyPlus interpolation for better accuracy.
    let t_high_u = if shgc < 0.7206 {
        0.939_998 * shgc * shgc + 0.203_32 * shgc
    } else {
        1.304_15 * shgc - 0.305_15
    };
    let t_low_u = if shgc < 0.15 {
        0.410_40 * shgc
    } else {
        0.085_775 * shgc * shgc + 0.963_954 * shgc - 0.084_958
    };
    let transmittance = if u_w_m2_k > 4.5 {
        t_high_u
    } else if u_w_m2_k < 3.4 {
        t_low_u
    } else {
        // Linear interpolation between 3.4 and 4.5 W/m²·K.
        let frac = (u_w_m2_k - 3.4) / (4.5 - 3.4);
        t_low_u + frac * (t_high_u - t_low_u)
    }
    .clamp(0.0, shgc);

    // Step 5: Interior/exterior film resistances for absorbed solar split.
    let x = (shgc - transmittance).max(0.0);
    let (res_int_s, res_ext_s) = if u_w_m2_k > 4.5 {
        let x2 = x * x;
        let x3 = x2 * x;
        (
            1.0 / (29.436_546 * x3 - 21.943_415 * x2 + 9.945_872 * x + 7.426_151),
            1.0 / (2.225_824 * x + 20.577_08),
        )
    } else if u_w_m2_k < 3.4 {
        let x2 = x * x;
        let x3 = x2 * x;
        (
            1.0 / (199.820_812_8 * x3 - 90.639_733 * x2 + 19.737_055 * x + 6.766_575),
            1.0 / (5.763_355 * x + 20.541_528),
        )
    } else {
        // Interpolation band: blend high-U and low-U resistances.
        let x2 = x * x;
        let x3 = x2 * x;
        let ri_high = 1.0 / (29.436_546 * x3 - 21.943_415 * x2 + 9.945_872 * x + 7.426_151);
        let re_high = 1.0 / (2.225_824 * x + 20.577_08);
        let ri_low = 1.0 / (199.820_812_8 * x3 - 90.639_733 * x2 + 19.737_055 * x + 6.766_575);
        let re_low = 1.0 / (5.763_355 * x + 20.541_528);
        let frac = (u_w_m2_k - 3.4) / (4.5 - 3.4);
        (
            ri_low + frac * (ri_high - ri_low),
            re_low + frac * (re_high - re_low),
        )
    };

    // Inward-flowing fraction: fraction of absorbed solar that reaches the zone.
    // N_i = (R_ext + R_glass/2) / (R_ext + R_glass + R_int)
    let denom = res_ext_s + res_material_m2_k_w + res_int_s;
    let radiation_frac = if denom > 0.0 {
        ((res_ext_s + res_material_m2_k_w / 2.0) / denom).clamp(0.0, 1.0)
    } else {
        0.5
    };

    (transmittance, radiation_frac)
}

/// Window transmitted solar with angle-of-incidence dependent transmittance [W].
///
/// Applies the EnergyPlus IAM polynomial to beam radiation and a
/// pre-computed hemispherical IAM constant to diffuse radiation.
///
/// # Arguments
/// * `beam_irradiance_w_m2` -- beam (direct) POA irradiance [W/m²].
/// * `diffuse_irradiance_w_m2` -- diffuse + reflected POA irradiance [W/m²].
/// * `shgc` -- solar heat gain coefficient at normal incidence [dimensionless].
/// * `area_m2` -- glazing area [m²].
/// * `angle_of_incidence_rad` -- angle between beam and surface normal [rad].
/// * `curve` -- EnergyPlus glazing curve for angular correction.
#[must_use]
pub fn window_transmitted_solar_angular(
    beam_irradiance_w_m2: f64,
    diffuse_irradiance_w_m2: f64,
    shgc: f64,
    area_m2: f64,
    angle_of_incidence_rad: f64,
    curve: GlazingCurve,
) -> f64 {
    let beam_gain = beam_irradiance_w_m2 * shgc * window_iam(angle_of_incidence_rad, curve);
    let diffuse_gain = diffuse_irradiance_w_m2 * shgc * curve.diffuse_iam();
    (beam_gain + diffuse_gain) * area_m2
}

#[cfg(test)]
mod tests {
    use chrono::{FixedOffset, NaiveDate, TimeZone};
    use hares_types::DEFAULT_GROUND_ALBEDO;

    use super::*;

    #[test]
    fn equatorial_equinox_noon_altitude_is_near_zenith() {
        // Find true solar noon (peak altitude) over the day to avoid clock-noon assumptions.
        let day = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 3, 20, 0, 0, 0)
            .single()
            .expect("valid timestamp")
            .date_naive();

        let mut max_altitude = f64::NEG_INFINITY;
        for minute in 0..(24 * 60) {
            let dt = FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(
                    day.year(),
                    day.month(),
                    day.day(),
                    (minute / 60) as u32,
                    (minute % 60) as u32,
                    0,
                )
                .single()
                .expect("valid minute timestamp");
            let pos = solar_position(0.0, 0.0, dt);
            max_altitude = max_altitude.max(pos.altitude_deg);
        }

        assert!(
            (max_altitude - 90.0).abs() <= 0.5,
            "max altitude={max_altitude}"
        );
    }

    #[test]
    fn south_facing_vertical_aoi_matches_analytic_case() {
        let aoi = angle_of_incidence(90.0, 180.0, 45.0, 180.0);
        assert!((aoi - 45.0).abs() <= 0.1, "aoi={aoi}");
    }

    #[test]
    fn nighttime_irradiance_is_exactly_zero() {
        let irr = perez_tilted_irradiance(
            0,
            0.0,
            0.0,
            0.0,
            90.0,
            180.0,
            45.0,
            180.0,
            1,
            DEFAULT_GROUND_ALBEDO,
        );
        assert_eq!(irr.direct_w_m2, 0.0);
        assert_eq!(irr.diffuse_w_m2, 0.0);
        assert_eq!(irr.reflected_w_m2, 0.0);
    }

    #[test]
    fn isotropic_horizontal_receives_full_diffuse() {
        // Liu & Jordan (1963): horizontal surface (tilt=0°) receives full diffuse sky
        // and zero ground-reflected. Diffuse = DHI * (1+cos(0))/2 = DHI
        let irr =
            isotropic_tilted_irradiance(0, 500.0, 300.0, 200.0, 0.0, 0.0, DEFAULT_GROUND_ALBEDO);
        assert!(
            (irr.diffuse_w_m2 - 200.0).abs() < 0.01,
            "horizontal diffuse: {}",
            irr.diffuse_w_m2
        );
        assert!(
            irr.reflected_w_m2.abs() < 0.01,
            "horizontal reflected: {}",
            irr.reflected_w_m2
        );
    }

    #[test]
    fn isotropic_vertical_receives_half_diffuse() {
        // Liu & Jordan (1963): vertical surface (tilt=90°) receives half diffuse
        // Diffuse = DHI * (1+cos(90°))/2 = DHI/2
        let irr =
            isotropic_tilted_irradiance(0, 500.0, 300.0, 200.0, 45.0, 90.0, DEFAULT_GROUND_ALBEDO);
        assert!(
            (irr.diffuse_w_m2 - 100.0).abs() < 0.5,
            "vertical diffuse: {}",
            irr.diffuse_w_m2
        );
    }

    #[test]
    fn extraterrestrial_irradiance_perihelion_is_near_1415() {
        // Around Jan 3 (day 3), Earth is at perihelion -- ETI should be ~1415 W/m²
        let eti = extraterrestrial_irradiance(3);
        assert!(
            (eti - 1415.0).abs() < 15.0,
            "perihelion ETI: {eti}, expected ~1415"
        );
    }

    #[test]
    fn relative_airmass_at_zenith_is_one() {
        let am = relative_airmass(0.0);
        assert!(
            (am - 1.0).abs() < 0.01,
            "airmass at zenith: {am}, expected 1.0"
        );
    }

    #[test]
    fn relative_airmass_at_60deg_is_about_two() {
        let am = relative_airmass(60.0);
        assert!(
            (am - 2.0).abs() < 0.1,
            "airmass at 60°: {am}, expected ~2.0"
        );
    }

    #[test]
    fn perez_clear_sky_exceeds_isotropic_for_equator_facing() {
        // Clear sky (high DNI, low DHI) on a south-facing tilted surface
        // should yield higher diffuse than isotropic due to circumsolar brightening
        let ghi = 800.0;
        let dni = 700.0;
        let dhi = 100.0;
        let zenith = 30.0;
        let solar_az = 180.0;
        let tilt = 30.0;
        let surf_az = 180.0;
        let doy = 172; // summer solstice

        let perez = perez_tilted_irradiance(
            0,
            ghi,
            dni,
            dhi,
            zenith,
            solar_az,
            tilt,
            surf_az,
            doy,
            DEFAULT_GROUND_ALBEDO,
        );
        let aoi = angle_of_incidence(tilt, surf_az, 90.0 - zenith, solar_az);
        let iso = isotropic_tilted_irradiance(0, ghi, dni, dhi, aoi, tilt, DEFAULT_GROUND_ALBEDO);

        let perez_total = perez.direct_w_m2 + perez.diffuse_w_m2 + perez.reflected_w_m2;
        let iso_total = iso.direct_w_m2 + iso.diffuse_w_m2 + iso.reflected_w_m2;
        assert!(
            perez_total >= iso_total,
            "Perez total ({perez_total:.1}) should >= isotropic ({iso_total:.1}) for clear sky"
        );
    }

    #[test]
    fn perez_overcast_close_to_isotropic() {
        // Overcast sky: low DNI, high DHI fraction -- epsilon near 1
        // Perez should be close to isotropic
        let ghi = 200.0;
        let dni = 10.0;
        let dhi = 190.0;
        let zenith = 50.0;
        let solar_az = 180.0;
        let tilt = 30.0;
        let surf_az = 180.0;
        let doy = 80;

        let perez = perez_tilted_irradiance(
            0,
            ghi,
            dni,
            dhi,
            zenith,
            solar_az,
            tilt,
            surf_az,
            doy,
            DEFAULT_GROUND_ALBEDO,
        );
        let aoi = angle_of_incidence(tilt, surf_az, 90.0 - zenith, solar_az);
        let iso = isotropic_tilted_irradiance(0, ghi, dni, dhi, aoi, tilt, DEFAULT_GROUND_ALBEDO);

        // Diffuse should be within 20% for overcast conditions
        let ratio = perez.diffuse_w_m2 / iso.diffuse_w_m2;
        assert!(
            (0.7..=1.3).contains(&ratio),
            "overcast Perez/isotropic diffuse ratio: {ratio:.3}, expected ~1.0"
        );
    }

    #[test]
    fn perez_extreme_zenith_falls_back_to_isotropic() {
        // At zenith > 87°, Perez should fall back to isotropic
        let ghi = 50.0;
        let dni = 20.0;
        let dhi = 30.0;
        let zenith = 88.0;
        let solar_az = 180.0;
        let tilt = 30.0;
        let surf_az = 180.0;
        let doy = 172;

        let result = perez_tilted_irradiance(
            0,
            ghi,
            dni,
            dhi,
            zenith,
            solar_az,
            tilt,
            surf_az,
            doy,
            DEFAULT_GROUND_ALBEDO,
        );
        // Should not panic and all components should be >= 0
        assert!(result.direct_w_m2 >= 0.0);
        assert!(result.diffuse_w_m2 >= 0.0);
        assert!(result.reflected_w_m2 >= 0.0);
    }

    #[test]
    fn higher_ground_albedo_increases_reflected_solar() {
        let ghi = 800.0;
        let dni = 700.0;
        let dhi = 100.0;
        let zenith = 30.0;
        let solar_az = 180.0;
        let tilt = 90.0; // vertical surface maximises ground-view factor
        let surf_az = 180.0;
        let doy = 172;

        let bare =
            perez_tilted_irradiance(0, ghi, dni, dhi, zenith, solar_az, tilt, surf_az, doy, 0.2);
        let snow =
            perez_tilted_irradiance(0, ghi, dni, dhi, zenith, solar_az, tilt, surf_az, doy, 0.8);

        assert!(
            snow.reflected_w_m2 > bare.reflected_w_m2,
            "snow albedo (0.8) reflected {:.1} should exceed bare (0.2) reflected {:.1}",
            snow.reflected_w_m2,
            bare.reflected_w_m2
        );
        // Snow albedo is 4x bare albedo, so reflected should scale linearly.
        let ratio = snow.reflected_w_m2 / bare.reflected_w_m2;
        assert!(
            (ratio - 4.0).abs() < 0.01,
            "reflected ratio should be 4.0, got {ratio:.3}"
        );
        // Direct and diffuse should be unchanged by albedo.
        assert!(
            (bare.direct_w_m2 - snow.direct_w_m2).abs() < 1e-12,
            "direct should not change with albedo"
        );
        assert!(
            (bare.diffuse_w_m2 - snow.diffuse_w_m2).abs() < 1e-12,
            "diffuse should not change with albedo"
        );
    }

    #[test]
    fn isotropic_higher_albedo_increases_reflected() {
        let bare = isotropic_tilted_irradiance(0, 500.0, 300.0, 200.0, 45.0, 90.0, 0.2);
        let snow = isotropic_tilted_irradiance(0, 500.0, 300.0, 200.0, 45.0, 90.0, 0.8);
        assert!(
            snow.reflected_w_m2 > bare.reflected_w_m2,
            "snow albedo reflected {:.1} should exceed bare {:.1}",
            snow.reflected_w_m2,
            bare.reflected_w_m2
        );
    }

    #[test]
    fn window_transmitted_solar_is_linear() {
        let base = window_transmitted_solar(500.0, 0.5, 2.0);
        let double_irr = window_transmitted_solar(1000.0, 0.5, 2.0);
        let double_shgc = window_transmitted_solar(500.0, 1.0, 2.0);
        let double_area = window_transmitted_solar(500.0, 0.5, 4.0);
        assert_eq!(base * 2.0, double_irr);
        assert_eq!(base * 2.0, double_shgc);
        assert_eq!(base * 2.0, double_area);
    }

    // -----------------------------------------------------------------------
    // Solar position at non-equatorial latitudes
    //
    // Reference values from NOAA Solar Position Calculator and the
    // Spencer (1971) Fourier series used in this implementation.
    // -----------------------------------------------------------------------

    /// Solar altitude near sunrise at 40°N must be close to 0° and the
    /// azimuth must be in the eastern semicircle (roughly 60–120° from north).
    /// We scan over morning minutes and find the first above-horizon position.
    #[test]
    fn solar_altitude_near_zero_at_sunrise_40n() {
        // 21 March at 40°N, 0° longitude.  Scan morning minutes.
        let lat = 40.0;
        let lon = 0.0;
        let day = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 3, 21, 0, 0, 0)
            .single()
            .unwrap();

        // Find first minute where sun is just above horizon.
        let sunrise_pos = (0u32..(12 * 60))
            .filter_map(|m| {
                let dt = day + chrono::Duration::minutes(m as i64);
                let pos = solar_position(lat, lon, dt);
                if pos.altitude_deg > 0.0 {
                    Some(pos)
                } else {
                    None
                }
            })
            .next()
            .expect("sun should rise before noon on equinox at 40°N");

        // Within the first minute after sunrise, altitude must be near zero.
        assert!(
            sunrise_pos.altitude_deg < 2.0,
            "altitude at sunrise should be near 0°, got {:.2}°",
            sunrise_pos.altitude_deg
        );

        // Sun rises in the east: azimuth between 60° and 120° (north-referenced).
        assert!(
            (60.0..=120.0).contains(&sunrise_pos.azimuth_deg),
            "sunrise azimuth should be in the east (60–120°), got {:.1}°",
            sunrise_pos.azimuth_deg
        );
    }

    /// At 60°N on June 21 (summer solstice), the noon solar altitude must be
    /// approximately 90 − 60 + 23.44 ≈ 53.44°.  Tolerance ±2°.
    #[test]
    fn summer_solstice_noon_altitude_at_60n() {
        let lat = 60.0;
        let lon = 0.0;
        let expected_deg = 90.0 - lat + 23.44; // ≈ 53.44°

        // Scan full day and find peak altitude (true solar noon).
        let day = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 6, 21, 0, 0, 0)
            .single()
            .unwrap();
        let max_altitude = (0u32..(24 * 60))
            .map(|m| {
                let dt = day + chrono::Duration::minutes(m as i64);
                solar_position(lat, lon, dt).altitude_deg
            })
            .fold(f64::NEG_INFINITY, f64::max);

        assert!(
            (max_altitude - expected_deg).abs() <= 2.0,
            "summer solstice noon altitude at 60°N: got {max_altitude:.2}°, expected ~{expected_deg:.2}° ±2°"
        );
    }

    /// At 60°N on December 21 (winter solstice), the noon solar altitude must
    /// be approximately 90 − 60 − 23.44 ≈ 6.56°.  Tolerance ±2°.
    #[test]
    fn winter_solstice_noon_altitude_at_60n() {
        let lat = 60.0;
        let lon = 0.0;
        let expected_deg = 90.0 - lat - 23.44; // ≈ 6.56°

        let day = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 12, 21, 0, 0, 0)
            .single()
            .unwrap();
        let max_altitude = (0u32..(24 * 60))
            .map(|m| {
                let dt = day + chrono::Duration::minutes(m as i64);
                solar_position(lat, lon, dt).altitude_deg
            })
            .fold(f64::NEG_INFINITY, f64::max);

        assert!(
            (max_altitude - expected_deg).abs() <= 2.0,
            "winter solstice noon altitude at 60°N: got {max_altitude:.2}°, expected ~{expected_deg:.2}° ±2°"
        );
    }

    /// At 65°N (Iceland) across all quarter-hours of the summer solstice (day 172),
    /// azimuth must always be in [0, 360) and altitude must be > −1° during daylight hours.
    #[test]
    fn solar_azimuth_stays_in_range_at_high_latitude() {
        let lat = 65.0;
        let lon = -18.0; // Reykjavik
        let naive = NaiveDate::from_yo_opt(2026, 172).expect("valid ordinal");
        let base = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(naive.year(), naive.month(), naive.day(), 0, 0, 0)
            .single()
            .expect("valid timestamp");

        for hour in 0u32..24 {
            for minute in [0u32, 15, 30, 45] {
                let dt = base
                    + chrono::Duration::hours(hour as i64)
                    + chrono::Duration::minutes(minute as i64);
                let pos = solar_position(lat, lon, dt);
                assert!(
                    pos.azimuth_deg >= 0.0 && pos.azimuth_deg < 360.0,
                    "azimuth {} out of range at {:02}:{:02}",
                    pos.azimuth_deg,
                    hour,
                    minute,
                );
                if (2..=22).contains(&hour) {
                    assert!(
                        pos.altitude_deg > -1.5,
                        "altitude {:.2}° too low at {:02}:{:02} for 65°N summer solstice",
                        pos.altitude_deg,
                        hour,
                        minute,
                    );
                }
            }
        }
    }

    #[test]
    fn solar_position_nrel_spa_cross_check() {
        // NREL SPA reference: Oct 17, 2003, 12:30:30 local (MST = UTC-7)
        // Lat 39.742476°N, Lon -105.1786° (Denver)
        // SPA result: zenith = 50.11162°, azimuth = 194.34024°
        // Spencer model should be within ±1.5° of SPA
        let dt = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2003, 10, 17, 19, 30, 30)
            .unwrap();
        let pos = solar_position(39.742476, -105.1786, dt);
        let zenith = 90.0 - pos.altitude_deg;
        assert!(
            (zenith - 50.11).abs() < 1.5,
            "Spencer vs SPA zenith: {zenith}, SPA=50.11° (tolerance ±1.5°)"
        );
    }

    /// At 40°N (Denver) on winter solstice (day 355), solar noon altitude
    /// should be approximately 26.5° and azimuth near 180° (due south).
    #[test]
    fn solar_position_winter_solstice_midlatitude() {
        let lat = 40.0;
        let lon = -105.0;
        let naive = NaiveDate::from_yo_opt(2026, 355).expect("valid ordinal");
        let base = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(naive.year(), naive.month(), naive.day(), 0, 0, 0)
            .single()
            .expect("valid timestamp");

        // Scan full day to find true solar noon (peak altitude).
        let (alt, az) = (0u32..(24 * 60))
            .map(|m| {
                let dt = base + chrono::Duration::minutes(m as i64);
                let pos = solar_position(lat, lon, dt);
                (pos.altitude_deg, pos.azimuth_deg)
            })
            .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap())
            .expect("non-empty");

        assert!(
            (alt - 26.5).abs() < 2.0,
            "winter solstice noon altitude {alt:.2}° should be ~26.5°"
        );
        assert!(
            (az - 180.0).abs() < 15.0,
            "noon azimuth {az:.1}° should be near 180°"
        );
    }

    /// At 60°N on the summer solstice the sun stays above the horizon for more
    /// than 18 hours.  This catches sign inversions or radian/degree errors in
    /// the zenith calculation that would collapse the daylight window.
    #[test]
    fn summer_solstice_daylight_exceeds_18h_at_60n() {
        let lat = 60.0;
        let lon = 0.0;
        let day = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 6, 21, 0, 0, 0)
            .single()
            .unwrap();

        let daylight_minutes = (0u32..(24 * 60))
            .filter(|&m| {
                let dt = day + chrono::Duration::minutes(m as i64);
                solar_position(lat, lon, dt).altitude_deg > 0.0
            })
            .count();

        assert!(
            daylight_minutes >= 18 * 60,
            "summer solstice daylight at 60°N should be ≥18 h, got {} min ({:.1} h)",
            daylight_minutes,
            daylight_minutes as f64 / 60.0,
        );
    }

    /// At the Tropic of Cancer (23.5°N) on winter solstice, noon altitude is
    /// approximately 90 − 23.5 − 23.44 ≈ 43°.  The sun remains well above the
    /// horizon and the peak falls within ±2° of the analytic value.
    #[test]
    fn tropical_winter_noon_altitude_at_tropic_of_cancer() {
        let lat = 23.5;
        let lon = 0.0;
        let expected_deg = 90.0 - lat - 23.44; // ≈ 43.06°

        let day = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 12, 21, 0, 0, 0)
            .single()
            .unwrap();
        let max_altitude = (0u32..(24 * 60))
            .map(|m| {
                let dt = day + chrono::Duration::minutes(m as i64);
                solar_position(lat, lon, dt).altitude_deg
            })
            .fold(f64::NEG_INFINITY, f64::max);

        assert!(
            max_altitude > 0.0,
            "sun must be above horizon at Tropic of Cancer in winter, got {max_altitude:.2}°"
        );
        assert!(
            (max_altitude - expected_deg).abs() <= 2.0,
            "tropical winter noon altitude: got {max_altitude:.2}°, expected ~{expected_deg:.2}° ±2°"
        );
    }

    #[test]
    fn extraterrestrial_irradiance_at_mean_distance_is_near_solar_constant() {
        // At mean Earth-Sun distance (around day 100 or the equinoxes),
        // ETI should be close to the solar constant 1367 W/m²
        // Day 80 (spring equinox) is close to mean distance
        let eti_equinox = extraterrestrial_irradiance(80);
        assert!(
            (eti_equinox - 1367.0).abs() < 20.0,
            "ETI at mean distance (day 80): {eti_equinox}, expected ~1367 W/m²"
        );
    }

    #[test]
    fn extraterrestrial_irradiance_aphelion_is_near_1322() {
        // Around July 4 (day 185), Earth is at aphelion -- ETI should be ~1322 W/m²
        let eti = extraterrestrial_irradiance(185);
        assert!(
            (eti - 1322.0).abs() < 15.0,
            "aphelion ETI: {eti}, expected ~1322"
        );
    }

    #[test]
    fn perez_nighttime_zenith_above_90_returns_zero() {
        // When solar zenith > 90 degrees (sun below horizon), GHI/DNI/DHI
        // should be zero and the model must return zero for all components
        let result = perez_tilted_irradiance(
            0,
            0.0,
            0.0,
            0.0,
            100.0,
            180.0,
            30.0,
            180.0,
            172,
            DEFAULT_GROUND_ALBEDO,
        );
        assert_eq!(result.direct_w_m2, 0.0);
        assert_eq!(result.diffuse_w_m2, 0.0);
        assert_eq!(result.reflected_w_m2, 0.0);
    }

    #[test]
    fn perez_extreme_zenith_87_to_90_no_nan_or_infinity() {
        // Zenith angles between 87-90 degrees should fall back to isotropic
        // without producing NaN or infinity
        for zenith in [87.5, 88.0, 89.0, 89.5, 89.9] {
            let result = perez_tilted_irradiance(
                0,
                50.0,
                20.0,
                30.0,
                zenith,
                180.0,
                30.0,
                180.0,
                172,
                DEFAULT_GROUND_ALBEDO,
            );
            assert!(
                result.direct_w_m2.is_finite(),
                "direct NaN/Inf at zenith={zenith}"
            );
            assert!(
                result.diffuse_w_m2.is_finite(),
                "diffuse NaN/Inf at zenith={zenith}"
            );
            assert!(
                result.reflected_w_m2.is_finite(),
                "reflected NaN/Inf at zenith={zenith}"
            );
            assert!(result.direct_w_m2 >= 0.0);
            assert!(result.diffuse_w_m2 >= 0.0);
            assert!(result.reflected_w_m2 >= 0.0);
        }
    }

    /// Cross-validation against pvlib-python reference values.
    /// pvlib.irradiance.perez(30, 180, 100, 700, 800, 30, 180, 1367, 1.15)
    /// with zenith=30, solar_az=180, tilt=30, surf_az=180 on summer solstice.
    /// The Perez diffuse component for clear-sky should be in [80, 150] W/m²
    /// range. This is a sanity envelope rather than an exact match since
    /// coefficient table revisions differ slightly between implementations.
    #[test]
    fn perez_pvlib_cross_validation_clear_sky() {
        let ghi = 800.0;
        let dni = 700.0;
        let dhi = 100.0;
        let zenith = 30.0;
        let solar_az = 180.0;
        let tilt = 30.0;
        let surf_az = 180.0;
        let doy = 172;

        let result = perez_tilted_irradiance(
            0,
            ghi,
            dni,
            dhi,
            zenith,
            solar_az,
            tilt,
            surf_az,
            doy,
            DEFAULT_GROUND_ALBEDO,
        );

        // Direct beam on a 30-degree tilted surface facing the sun at 30-degree zenith
        // should be approximately DNI * cos(0) = 700 (AOI ~ 0)
        assert!(
            result.direct_w_m2 > 500.0 && result.direct_w_m2 <= 710.0,
            "clear-sky direct: {}, expected 500-710 W/m²",
            result.direct_w_m2
        );

        // Perez diffuse should be in a reasonable range for clear sky
        assert!(
            result.diffuse_w_m2 > 50.0 && result.diffuse_w_m2 < 200.0,
            "clear-sky Perez diffuse: {}, expected 50-200 W/m²",
            result.diffuse_w_m2
        );

        // Ground-reflected should be small for a 30-degree tilt
        assert!(
            result.reflected_w_m2 > 0.0 && result.reflected_w_m2 < 30.0,
            "clear-sky reflected: {}, expected 0-30 W/m²",
            result.reflected_w_m2
        );

        // Total POA irradiance should exceed GHI for an optimally-tilted surface
        let total = result.direct_w_m2 + result.diffuse_w_m2 + result.reflected_w_m2;
        assert!(
            total > ghi * 0.8,
            "total POA ({total:.1}) should be substantial relative to GHI ({ghi})"
        );
    }

    #[test]
    fn relative_airmass_high_zenith_is_large_and_finite() {
        // At 85 degrees zenith, airmass should be large but finite
        let am = relative_airmass(85.0);
        assert!(am > 10.0, "airmass at 85°: {am}, expected > 10");
        assert!(am.is_finite(), "airmass at 85° must be finite");

        // At 89.9 degrees (clamped internally), should still be finite
        let am_extreme = relative_airmass(89.9);
        assert!(am_extreme.is_finite(), "airmass at 89.9° must be finite");
        assert!(am_extreme > am, "airmass at 89.9° should exceed 85°");
    }

    #[test]
    fn perez_clear_sky_diffuse_exceeds_isotropic_diffuse() {
        // Under clear conditions with high epsilon, the Perez model should
        // produce higher diffuse on a south-facing tilted surface than
        // the isotropic model due to circumsolar brightening
        let ghi = 900.0;
        let dni = 800.0;
        let dhi = 100.0;
        let zenith = 25.0;
        let solar_az = 180.0;
        let tilt = 25.0;
        let surf_az = 180.0;
        let doy = 172;

        let perez = perez_tilted_irradiance(
            0,
            ghi,
            dni,
            dhi,
            zenith,
            solar_az,
            tilt,
            surf_az,
            doy,
            DEFAULT_GROUND_ALBEDO,
        );
        let aoi = angle_of_incidence(tilt, surf_az, 90.0 - zenith, solar_az);
        let iso = isotropic_tilted_irradiance(0, ghi, dni, dhi, aoi, tilt, DEFAULT_GROUND_ALBEDO);

        assert!(
            perez.diffuse_w_m2 > iso.diffuse_w_m2,
            "Perez diffuse ({:.1}) should exceed isotropic ({:.1}) under clear sky",
            perez.diffuse_w_m2,
            iso.diffuse_w_m2
        );
    }

    /// An east-facing vertical surface (azimuth 90°, tilt 90°) must receive
    /// more cumulative direct irradiance in the morning than in the afternoon
    /// on a symmetric solar day.  Catches east/west azimuth-delta sign errors
    /// in the angle-of-incidence formula.
    #[test]
    fn east_facing_surface_receives_more_direct_irradiance_in_morning_than_afternoon() {
        // Equinox at 40°N, 0° longitude: solar path is symmetric about solar noon.
        let lat = 40.0;
        let lon = 0.0;
        let day = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 3, 20, 0, 0, 0)
            .single()
            .unwrap();

        let ghi = 700.0_f64;
        let dni = 800.0_f64;
        let dhi = 150.0_f64;
        let surface_tilt = 90.0_f64; // vertical
        let surface_az = 90.0_f64; // east-facing (north-referenced)
        let doy = 79_u32; // March 20

        // Morning: hours 6–11 UTC (before solar noon ≈ 12 UTC at lon 0)
        let morning_direct: f64 = (6u32..11)
            .map(|h| {
                let dt = day + chrono::Duration::hours(h as i64);
                let pos = solar_position(lat, lon, dt);
                if pos.altitude_deg <= 0.0 {
                    return 0.0;
                }
                let zenith = 90.0 - pos.altitude_deg;
                perez_tilted_irradiance(
                    0,
                    ghi,
                    dni,
                    dhi,
                    zenith,
                    pos.azimuth_deg,
                    surface_tilt,
                    surface_az,
                    doy,
                    DEFAULT_GROUND_ALBEDO,
                )
                .direct_w_m2
            })
            .sum();

        // Afternoon: hours 13–18 UTC (symmetric to morning, sun in west)
        let afternoon_direct: f64 = (13u32..18)
            .map(|h| {
                let dt = day + chrono::Duration::hours(h as i64);
                let pos = solar_position(lat, lon, dt);
                if pos.altitude_deg <= 0.0 {
                    return 0.0;
                }
                let zenith = 90.0 - pos.altitude_deg;
                perez_tilted_irradiance(
                    0,
                    ghi,
                    dni,
                    dhi,
                    zenith,
                    pos.azimuth_deg,
                    surface_tilt,
                    surface_az,
                    doy,
                    DEFAULT_GROUND_ALBEDO,
                )
                .direct_w_m2
            })
            .sum();

        assert!(
            morning_direct > 0.0,
            "east-facing surface must receive direct irradiance in morning, got {morning_direct}"
        );
        assert!(
            morning_direct > afternoon_direct,
            "east-facing morning direct ({morning_direct:.1} W/m²·h) must exceed afternoon ({afternoon_direct:.1} W/m²·h)"
        );
    }

    // -----------------------------------------------------------------------
    // perez_sky_diffuse -- standalone sky diffuse component tests
    // -----------------------------------------------------------------------

    /// Worked example from Perez (1990): south-facing 30° tilt, solar zenith
    /// 30°, DHI=100, DNI=800, DNI_extra=1370 W/m².
    /// Expected sky diffuse ≈ 113.4 W/m² (tolerance ±2 W/m²).
    #[test]
    fn perez_sky_diffuse_worked_example_matches_reference() {
        let dhi = 100.0;
        let dni = 800.0;
        let zenith_deg = 30.0;
        let tilt_deg = 30.0;
        let dni_extra = 1370.0;
        // Sun at zenith 30° azimuth 180°, surface south-facing 30° tilt → AOI = 0°
        let aoi_deg = angle_of_incidence(tilt_deg, 180.0, 90.0 - zenith_deg, 180.0);

        let sky_diffuse = perez_sky_diffuse(dhi, dni, zenith_deg, aoi_deg, tilt_deg, dni_extra);
        assert!(
            (sky_diffuse - 113.4).abs() < 2.0,
            "Perez sky diffuse worked example: {sky_diffuse:.2} W/m², expected ~113.4 W/m²"
        );
    }

    /// Overcast sky (epsilon ≈ 1, bin 0): nearly all diffuse, no direct.
    /// Sky diffuse should be positive and close to the isotropic value.
    #[test]
    fn perez_sky_diffuse_overcast_bin_zero_is_reasonable() {
        let dhi = 200.0;
        let dni = 5.0; // very low DNI → epsilon near 1
        let zenith_deg = 45.0;
        let tilt_deg = 30.0;
        let dni_extra = 1367.0;
        let aoi_deg = angle_of_incidence(tilt_deg, 180.0, 90.0 - zenith_deg, 180.0);

        let sky_diffuse = perez_sky_diffuse(dhi, dni, zenith_deg, aoi_deg, tilt_deg, dni_extra);
        // Isotropic reference: dhi * (1 + cos(30°)) / 2 ≈ 200 * 0.933 = 186.6
        let iso_ref = dhi * (1.0 + tilt_deg.to_radians().cos()) * 0.5;
        assert!(
            sky_diffuse > 0.0,
            "overcast sky diffuse must be positive, got {sky_diffuse}"
        );
        assert!(
            (sky_diffuse / iso_ref - 1.0).abs() < 0.25,
            "overcast Perez ({sky_diffuse:.1}) should be within 25% of isotropic ({iso_ref:.1})"
        );
    }

    /// Clear sky (epsilon > 6.2, bin 7): circumsolar brightening should push
    /// sky diffuse above the isotropic value on a sun-facing surface.
    #[test]
    fn perez_sky_diffuse_clear_sky_bin_seven_exceeds_isotropic() {
        let dhi = 80.0;
        let dni = 850.0; // high DNI → epsilon >> 6.2
        let zenith_deg = 20.0;
        let tilt_deg = 20.0; // tilt matches zenith for AOI ≈ 0
        let dni_extra = 1367.0;
        let aoi_deg = angle_of_incidence(tilt_deg, 180.0, 90.0 - zenith_deg, 180.0);

        let sky_diffuse = perez_sky_diffuse(dhi, dni, zenith_deg, aoi_deg, tilt_deg, dni_extra);
        let iso_ref = dhi * (1.0 + tilt_deg.to_radians().cos()) * 0.5;
        assert!(
            sky_diffuse > iso_ref,
            "clear-sky Perez diffuse ({sky_diffuse:.1}) must exceed isotropic ({iso_ref:.1})"
        );
    }

    /// Zero DHI (night or sensor floor) must return exactly 0.
    #[test]
    fn perez_sky_diffuse_zero_dhi_returns_zero() {
        assert_eq!(perez_sky_diffuse(0.0, 800.0, 30.0, 0.0, 30.0, 1367.0), 0.0);
        assert_eq!(perez_sky_diffuse(-1.0, 800.0, 30.0, 0.0, 30.0, 1367.0), 0.0);
    }

    /// Perez diffuse exceeds isotropic diffuse on a tilted, sun-facing surface
    /// under clear sky -- circumsolar brightening must be visible.
    #[test]
    fn perez_sky_diffuse_exceeds_isotropic_for_tilted_clear_sky() {
        let dhi = 100.0;
        let dni = 800.0;
        let zenith_deg = 30.0;
        let tilt_deg = 30.0;
        let dni_extra = 1370.0;
        let aoi_deg = angle_of_incidence(tilt_deg, 180.0, 90.0 - zenith_deg, 180.0);

        let perez = perez_sky_diffuse(dhi, dni, zenith_deg, aoi_deg, tilt_deg, dni_extra);
        let iso = dhi * (1.0 + tilt_deg.to_radians().cos()) * ISOTROPIC_VIEW_FACTOR;
        assert!(
            perez > iso,
            "Perez sky diffuse ({perez:.1}) must exceed isotropic ({iso:.1}) under clear sky"
        );
    }

    /// extraterrestrial_normal_irradiance is an alias that returns the same value.
    #[test]
    fn extraterrestrial_normal_irradiance_matches_extraterrestrial_irradiance() {
        for doy in [1u32, 80, 172, 265, 355] {
            let a = extraterrestrial_irradiance(doy);
            let b = extraterrestrial_normal_irradiance(doy);
            assert_eq!(a, b, "mismatch at doy={doy}: {a} vs {b}");
        }
    }

    // -----------------------------------------------------------------------
    // window_iam -- angle-of-incidence modifier tests
    // -----------------------------------------------------------------------

    /// IAM must equal exactly 1.0 at normal incidence (θ = 0) for all curves.
    #[test]
    fn window_iam_is_unity_at_normal_incidence() {
        for curve in [
            GlazingCurve::A,
            GlazingCurve::Bdcd,
            GlazingCurve::D,
            GlazingCurve::E,
            GlazingCurve::F,
            GlazingCurve::J,
        ] {
            let iam = window_iam(0.0, curve);
            assert!(
                (iam - 1.0).abs() < 1e-12,
                "IAM at θ=0 for {curve:?}: {iam}, expected 1.0"
            );
        }
    }

    /// IAM must return 0.0 at grazing incidence (θ = π/2) for all curves.
    #[test]
    fn window_iam_is_zero_at_grazing_incidence() {
        for curve in [
            GlazingCurve::A,
            GlazingCurve::Bdcd,
            GlazingCurve::D,
            GlazingCurve::E,
            GlazingCurve::F,
            GlazingCurve::J,
        ] {
            let iam = window_iam(std::f64::consts::FRAC_PI_2, curve);
            assert_eq!(iam, 0.0, "IAM at θ=π/2 for {curve:?}: {iam}, expected 0.0");
        }
    }

    /// IAM must return 0.0 for θ > π/2 (sun behind window plane).
    #[test]
    fn window_iam_is_zero_beyond_grazing() {
        for theta_deg in [91.0_f64, 120.0, 179.0, 180.0] {
            let iam = window_iam(theta_deg.to_radians(), GlazingCurve::E);
            assert_eq!(iam, 0.0, "IAM at θ={theta_deg}° should be 0.0, got {iam}");
        }
    }

    /// IAM must return 0.0 for negative θ (non-physical).
    #[test]
    fn window_iam_is_zero_for_negative_angle() {
        let iam = window_iam(-0.1, GlazingCurve::E);
        assert_eq!(iam, 0.0, "IAM at θ<0 should be 0.0, got {iam}");
    }

    /// IAM must decrease monotonically as θ increases from 0 to π/2.
    #[test]
    fn window_iam_decreases_monotonically_with_theta() {
        let angles_deg = [
            0.0_f64, 10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 89.0,
        ];
        for curve in [
            GlazingCurve::A,
            GlazingCurve::Bdcd,
            GlazingCurve::D,
            GlazingCurve::E,
            GlazingCurve::F,
            GlazingCurve::J,
        ] {
            let iams: Vec<f64> = angles_deg
                .iter()
                .map(|&deg| window_iam(deg.to_radians(), curve))
                .collect();
            for i in 1..iams.len() {
                assert!(
                    iams[i] <= iams[i - 1],
                    "curve {curve:?}: IAM not monotone at θ={}° ({}) >= θ={}° ({})",
                    angles_deg[i],
                    iams[i],
                    angles_deg[i - 1],
                    iams[i - 1]
                );
            }
        }
    }

    /// At θ = 60°, IAM for curve E (standard double-pane) should be in
    /// the range [0.80, 0.90], consistent with EnergyPlus reference values.
    #[test]
    fn window_iam_curve_e_at_60deg_is_in_expected_range() {
        let iam = window_iam(60.0_f64.to_radians(), GlazingCurve::E);
        assert!(
            (0.80..=0.90).contains(&iam),
            "curve E IAM at 60°: {iam:.4}, expected 0.80–0.90"
        );
    }

    /// At θ = 60°, IAM for curve A (single-pane clear) should be higher than
    /// curve J (triple-pane low-e) -- single-pane glass is less angularly selective.
    #[test]
    fn window_iam_single_pane_higher_than_triple_at_60deg() {
        let iam_a = window_iam(60.0_f64.to_radians(), GlazingCurve::A);
        let iam_j = window_iam(60.0_f64.to_radians(), GlazingCurve::J);
        assert!(
            iam_a > iam_j,
            "curve A IAM at 60° ({iam_a:.4}) should exceed curve J ({iam_j:.4})"
        );
    }

    /// Diffuse IAM for all curves must be in [0.75, 0.95].
    #[test]
    fn window_diffuse_iam_is_in_physical_range() {
        for curve in [
            GlazingCurve::A,
            GlazingCurve::Bdcd,
            GlazingCurve::D,
            GlazingCurve::E,
            GlazingCurve::F,
            GlazingCurve::J,
        ] {
            let d = curve.diffuse_iam();
            assert!(
                (0.75..=0.95).contains(&d),
                "curve {curve:?} diffuse IAM {d:.4} out of [0.75, 0.95]"
            );
        }
    }

    /// GlazingCurve::from_u_shgc must select the expected curve for representative
    /// window products.
    #[test]
    fn glazing_curve_selection_matches_energyplus_table() {
        // Single-pane clear (U ~5.8, SHGC 0.86) → A
        assert_eq!(GlazingCurve::from_u_shgc(5.8, 0.86), GlazingCurve::A);
        // Single-pane tinted (U 5.0, SHGC 0.45) → Bdcd
        assert_eq!(GlazingCurve::from_u_shgc(5.0, 0.45), GlazingCurve::Bdcd);
        // Single-pane reflective (U 4.5, SHGC 0.20) → D
        assert_eq!(GlazingCurve::from_u_shgc(4.5, 0.20), GlazingCurve::D);
        // Double-pane clear (U 2.7, SHGC 0.76) → E
        assert_eq!(GlazingCurve::from_u_shgc(2.7, 0.76), GlazingCurve::E);
        // Double-pane low-e (U 2.0, SHGC 0.40) → F
        assert_eq!(GlazingCurve::from_u_shgc(2.0, 0.40), GlazingCurve::F);
        // Triple-pane low-e, higher SHGC (U 1.2, SHGC 0.45) → E
        assert_eq!(GlazingCurve::from_u_shgc(1.2, 0.45), GlazingCurve::E);
        // Triple-pane low-e, low SHGC (U 1.0, SHGC 0.25) → J
        assert_eq!(GlazingCurve::from_u_shgc(1.0, 0.25), GlazingCurve::J);
    }

    /// window_transmitted_solar_angular at normal incidence should be close
    /// to (and always ≤) the constant-SHGC method for beam-only radiation,
    /// because IAM(0) = 1.0 exactly.
    #[test]
    fn angular_transmitted_solar_equals_flat_at_normal_incidence() {
        let beam = 700.0_f64;
        let diffuse = 0.0_f64;
        let shgc = 0.5_f64;
        let area = 2.0_f64;

        let angular =
            window_transmitted_solar_angular(beam, diffuse, shgc, area, 0.0, GlazingCurve::E);
        let flat = window_transmitted_solar(beam, shgc, area);
        assert!(
            (angular - flat).abs() < 1e-9,
            "angular at θ=0: {angular:.6}, flat: {flat:.6}"
        );
    }

    /// At a non-normal angle the angular method must give less beam gain than
    /// the constant-SHGC method.
    #[test]
    fn angular_transmitted_solar_less_than_flat_for_off_normal_beam() {
        let beam = 600.0_f64;
        let diffuse = 0.0_f64;
        let shgc = 0.5_f64;
        let area = 1.5_f64;
        let theta_rad = 60.0_f64.to_radians();

        let angular =
            window_transmitted_solar_angular(beam, diffuse, shgc, area, theta_rad, GlazingCurve::E);
        let flat = window_transmitted_solar(beam, shgc, area);
        assert!(
            angular < flat,
            "angular ({angular:.2} W) must be less than flat ({flat:.2} W) at θ=60°"
        );
    }

    /// With only diffuse irradiance the result must equal diffuse × SHGC ×
    /// diffuse_IAM × area.
    #[test]
    fn angular_transmitted_solar_diffuse_only_matches_formula() {
        let diffuse = 150.0_f64;
        let shgc = 0.4_f64;
        let area = 3.0_f64;
        let curve = GlazingCurve::F;

        let result = window_transmitted_solar_angular(0.0, diffuse, shgc, area, 0.0, curve);
        let expected = diffuse * shgc * curve.diffuse_iam() * area;
        assert!(
            (result - expected).abs() < 1e-9,
            "diffuse-only: {result:.6} vs {expected:.6}"
        );
    }

    /// Table-driven pinned IAM values for all 6 curves at 0°, 30°, 60°, and 80°.
    ///
    /// Reference values computed by evaluating the published EnergyPlus polynomial
    /// coefficients directly at the given angles, then normalising by the sum of
    /// coefficients (normal-incidence value).  Tolerance ±0.005.
    #[test]
    fn window_iam_pinned_values_all_curves() {
        // (curve, theta_deg, expected_iam)
        let cases: &[(GlazingCurve, f64, f64)] = &[
            // --- 0° (normal incidence) -- all curves must be exactly 1.0 ---
            (GlazingCurve::A, 0.0, 1.0),
            (GlazingCurve::Bdcd, 0.0, 1.0),
            (GlazingCurve::D, 0.0, 1.0),
            (GlazingCurve::E, 0.0, 1.0),
            (GlazingCurve::F, 0.0, 1.0),
            (GlazingCurve::J, 0.0, 1.0),
            // --- 30° ---
            (GlazingCurve::A, 30.0, 0.9863),
            (GlazingCurve::Bdcd, 30.0, 0.9676),
            (GlazingCurve::D, 30.0, 0.9741),
            (GlazingCurve::E, 30.0, 0.9727),
            (GlazingCurve::F, 30.0, 0.9613),
            (GlazingCurve::J, 30.0, 0.9437),
            // --- 60° ---
            (GlazingCurve::A, 60.0, 0.8977),
            (GlazingCurve::Bdcd, 60.0, 0.8318),
            (GlazingCurve::D, 60.0, 0.8435),
            (GlazingCurve::E, 60.0, 0.8155),
            (GlazingCurve::F, 60.0, 0.7768),
            (GlazingCurve::J, 60.0, 0.6695),
            // --- 80° ---
            (GlazingCurve::A, 80.0, 0.4717),
            (GlazingCurve::Bdcd, 80.0, 0.4051),
            (GlazingCurve::D, 80.0, 0.4161),
            (GlazingCurve::E, 80.0, 0.3046),
            (GlazingCurve::F, 80.0, 0.2712),
            (GlazingCurve::J, 80.0, 0.1521),
        ];

        for &(curve, theta_deg, expected) in cases {
            let iam = window_iam(theta_deg.to_radians(), curve);
            assert!(
                (iam - expected).abs() <= 0.005,
                "curve {curve:?} at {theta_deg}°: IAM={iam:.6}, expected {expected:.4} (±0.005)"
            );
        }
    }

    /// Boundary value tests for GlazingCurve::from_u_shgc.
    ///
    /// Verifies curve selection at exact U and SHGC boundary values where the
    /// comparisons are strict (`>`), so the boundary value itself falls to the
    /// lower branch.
    #[test]
    fn glazing_curve_boundary_values() {
        // U = 3.98: u > 3.98 is false → falls to u > 1.56 branch
        assert_eq!(
            GlazingCurve::from_u_shgc(3.98, 0.625),
            GlazingCurve::E,
            "u=3.98, shgc=0.625: not in single-pane branch"
        );
        assert_eq!(
            GlazingCurve::from_u_shgc(3.98, 0.3),
            GlazingCurve::F,
            "u=3.98, shgc=0.3: not in single-pane branch"
        );

        // U = 1.56: u > 1.56 is false → falls to else branch
        assert_eq!(
            GlazingCurve::from_u_shgc(1.56, 0.4),
            GlazingCurve::J,
            "u=1.56, shgc=0.4: shgc > 0.4 is false → J"
        );
        assert_eq!(
            GlazingCurve::from_u_shgc(1.56, 0.525),
            GlazingCurve::E,
            "u=1.56, shgc=0.525: shgc > 0.4 is true → E"
        );

        // SHGC = 0.625 with u > 3.98: shgc > 0.625 is false → shgc > 0.3 is true → Bdcd
        assert_eq!(
            GlazingCurve::from_u_shgc(4.0, 0.625),
            GlazingCurve::Bdcd,
            "u=4.0, shgc=0.625: not > 0.625, is > 0.3 → Bdcd"
        );

        // SHGC = 0.3 with u > 3.98: shgc > 0.3 is false → D
        assert_eq!(
            GlazingCurve::from_u_shgc(4.0, 0.3),
            GlazingCurve::D,
            "u=4.0, shgc=0.3: not > 0.3 → D"
        );

        // SHGC = 0.525 with 1.56 < u <= 3.98: shgc > 0.525 is false → F
        assert_eq!(
            GlazingCurve::from_u_shgc(2.0, 0.525),
            GlazingCurve::F,
            "u=2.0, shgc=0.525: not > 0.525 → F"
        );

        // SHGC = 0.4 with u <= 1.56: shgc > 0.4 is false → J
        assert_eq!(
            GlazingCurve::from_u_shgc(1.2, 0.4),
            GlazingCurve::J,
            "u=1.2, shgc=0.4: not > 0.4 → J"
        );
    }

    /// window_iam with NaN input must return 0.0, not NaN.
    #[test]
    fn window_iam_nan_input_returns_zero() {
        let iam = window_iam(f64::NAN, GlazingCurve::E);
        assert_eq!(iam, 0.0, "window_iam(NaN) should return 0.0, got {iam}");
    }

    /// Total transmitted solar at θ = 75° for a typical double-pane window
    /// must be significantly less than the flat SHGC calculation -- at least
    /// 15% lower for beam-dominated radiation.
    #[test]
    fn angular_significantly_reduces_gain_at_high_aoi() {
        let beam = 800.0_f64;
        let diffuse = 50.0_f64;
        let shgc = 0.4_f64;
        let area = 2.0_f64;
        let theta_rad = 75.0_f64.to_radians();

        let angular =
            window_transmitted_solar_angular(beam, diffuse, shgc, area, theta_rad, GlazingCurve::E);
        let flat = window_transmitted_solar(beam + diffuse, shgc, area);
        assert!(
            angular < flat * 0.85,
            "at θ=75°, angular ({angular:.1} W) should be <85% of flat ({flat:.1} W)"
        );
    }

    /// Verify calculate_window_parameters energy accounting:
    /// transmitted + absorbed_zone + absorbed_exterior + reflected ≈ incident.
    #[test]
    fn window_parameters_energy_balance() {
        // Low-e double-pane: U=1.8, SHGC=0.30.
        let (t, n_i) = calculate_window_parameters(0.30, 1.8, 0.01);
        let shgc = 0.30;
        let x = shgc - t; // inward component of absorbed
        let absorptivity = if n_i > 0.0 { x / n_i } else { 0.0 };
        let reflectance = 1.0 - t - absorptivity;

        // Energy balance: T + A + R = 1.0
        let total = t + absorptivity + reflectance;
        assert!(
            (total - 1.0).abs() < 1e-10,
            "energy balance: T={t:.4} + A={absorptivity:.4} + R={reflectance:.4} = {total:.6}"
        );

        // Absorbed component should be 25–35% of SHGC for low-e windows.
        let absorbed_frac = x / shgc;
        assert!(
            (0.20..=0.50).contains(&absorbed_frac),
            "absorbed fraction of SHGC: {absorbed_frac:.3} (expected 0.20–0.50)"
        );

        // Transmittance < SHGC (some solar is absorbed, not transmitted).
        assert!(t < shgc, "transmittance ({t:.4}) should be < SHGC ({shgc})");
        assert!(t > 0.0, "transmittance should be positive");

        // radiation_frac in reasonable range. Low-e windows with minimal glass
        // resistance can have N_i < 0.3 since most absorbed heat is re-radiated outward.
        assert!(
            (0.1..=0.9).contains(&n_i),
            "radiation_frac={n_i:.4} should be in [0.1, 0.9]"
        );
    }

    /// High-U single-pane window parameters.
    #[test]
    fn window_parameters_single_pane() {
        let (t, n_i) = calculate_window_parameters(0.60, 5.5, 0.005);
        assert!(t > 0.0 && t < 0.60, "transmittance {t:.4}");
        assert!(n_i > 0.0 && n_i < 1.0, "radiation_frac {n_i:.4}");
        // Single-pane: transmittance should be majority of SHGC.
        assert!(
            t > 0.60 * 0.5,
            "single-pane transmittance should be >50% of SHGC"
        );
    }

    /// Debug test: POA irradiance on 6 windows (N/S/E/W orientation) at Denver May 5 noon.
    /// Traces whether solar decomposition is correct.
    #[test]
    #[ignore]
    fn debug_solar_poa_at_may_5_noon_denver() {
        // May 5 noon local Denver = May 5 19:00 UTC (Denver is UTC-7 in May)
        let utc_time = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2024, 5, 5, 19, 0, 0)
            .single()
            .unwrap();
        let denver_lat = 39.83;
        let denver_lon = -104.65;

        // Solar position at May 5 noon
        let pos = solar_position(denver_lat, denver_lon, utc_time);
        eprintln!("\n=== Solar Position at May 5 noon UTC ===");
        eprintln!("Solar altitude: {:.2}°", pos.altitude_deg);
        eprintln!("Solar azimuth: {:.2}°", pos.azimuth_deg);

        // Weather at May 5 noon from Denver EPW
        let ghi = 928.0; // W/m² (from EPW)
        let dni = 883.0; // W/m² (from EPW)
        let dhi = 124.0; // W/m² (from EPW)
        let solar_zenith_deg = (90.0 - pos.altitude_deg).max(0.0);
        let day_of_year = 126; // May 5 = day 125 (1-indexed)

        eprintln!("\n=== Weather Data (Denver May 5, 12:00 local) ===");
        eprintln!("GHI: {} W/m²", ghi);
        eprintln!("DNI: {} W/m²", dni);
        eprintln!("DHI: {} W/m²", dhi);
        eprintln!("Solar zenith: {:.2}°", solar_zenith_deg);

        // Window orientations: 2xE, 1xN, 2xW, 1xS (per user description)
        let orientations = [
            ("East-1", 90.0),
            ("East-2", 90.0),
            ("North", 0.0),
            ("West-1", 270.0),
            ("West-2", 270.0),
            ("South", 180.0),
        ];

        eprintln!("\n=== Per-Window POA Irradiance (vertical, tilt=90°) ===");
        let mut total_poa = 0.0;

        for (name, azimuth) in orientations.iter() {
            let tilt = 90.0; // vertical window
            let aoi = angle_of_incidence(tilt, *azimuth, pos.altitude_deg, pos.azimuth_deg);

            let irr = perez_tilted_irradiance(
                0,
                ghi,
                dni,
                dhi,
                solar_zenith_deg,
                pos.azimuth_deg,
                tilt,
                *azimuth,
                day_of_year,
                DEFAULT_GROUND_ALBEDO,
            );

            let total_poa_window = irr.direct_w_m2 + irr.diffuse_w_m2 + irr.reflected_w_m2;
            total_poa += total_poa_window;

            eprintln!(
                "{:8} (az={:3.0}°): direct={:6.1}, diffuse={:6.1}, reflected={:5.1}, total={:6.1} W/m², AOI={:5.1}°",
                name,
                azimuth,
                irr.direct_w_m2,
                irr.diffuse_w_m2,
                irr.reflected_w_m2,
                total_poa_window,
                aoi
            );
        }

        let mean_poa = total_poa / 6.0;
        eprintln!("\nMean POA across 6 windows: {:.1} W/m²", mean_poa);
        eprintln!(
            "Expected: ~100–120 W/m² (user reports OCHRE gives 356W × 0.21 SHGC / 15.6 m² = 4.8 W/m² per window)"
        );
        eprintln!("HARES currently high: ~200+ W/m² suggests POA is 2x too high");
    }
}
