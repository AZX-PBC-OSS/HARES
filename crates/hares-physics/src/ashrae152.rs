//! ASHRAE 152 duct distribution system efficiency (DSE) calculation.
//!
//! All public inputs are SI units. Internal computation uses IP units per the
//! ASHRAE 152 standard. The public [`calculate_dse`] function returns a DSE
//! value clamped to `(0.0, 1.0]`.

use std::f64::consts::PI;
use std::sync::OnceLock;

use super::constants::SECONDS_PER_DAY;

// ---------------------------------------------------------------------------
// Unit conversion constants
// ---------------------------------------------------------------------------

const M3_TO_FT3: f64 = 35.3147;
const M2_TO_FT2: f64 = 10.7639;
const W_TO_BTU_H: f64 = 3.41214;
const M3S_TO_CFM: f64 = 2118.88;
/// SI R-value (m²·K/W) → IP R-value (ft²·h·°F/Btu)
const SI_R_TO_IP_R: f64 = 5.67826;

/// Soil volumetric heat capacity [J/(m³·K)] for average moist soil.
///
/// Ingersoll, Zobel & Ingersoll (1954), *Heat Conduction with Engineering,
/// Geological, and Other Applications*, §2.4: ρc = 2.56 MJ/(m³·K).
/// This value is also cited in Kavanaugh & Rafferty (1997), *Ground-Source
/// Heat Pumps*, ASHRAE, Ch. 3, and is the standard reference for ground-
/// coupled heat exchanger design. Used to compute soil thermal diffusivity
/// from conductivity: α = k / (ρc).
const SOIL_VOLUMETRIC_HEAT_CAPACITY_J_M3_K: f64 = 2_560_000.0;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// ASHRAE 152-2014 Table 5 duct leakage class.
///
/// Each variant maps to a leakage rate in CFM per 100 ft² of duct surface area
/// measured at 25 Pa test pressure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuctLeakageClass {
    /// ~2 CFM/100 ft² at 25 Pa — ASHRAE 152-2014 Table 5.
    WellSealed,
    /// ~6 CFM/100 ft² at 25 Pa — ASHRAE 152-2014 Table 5.
    Sealed,
    /// ~12 CFM/100 ft² at 25 Pa — ASHRAE 152-2014 Table 5.
    Unsealed,
}

impl DuctLeakageClass {
    /// Leakage rate in CFM per 100 ft² at 25 Pa test pressure.
    ///
    /// ASHRAE 152-2014 Table 5.
    pub fn cfm_per_100ft2_at_25pa(self) -> f64 {
        match self {
            DuctLeakageClass::WellSealed => 2.0,
            DuctLeakageClass::Sealed => 6.0,
            DuctLeakageClass::Unsealed => 12.0,
        }
    }

    /// Convert a leakage class to a leakage fraction given duct surface area
    /// and fan airflow rate.
    ///
    /// ASHRAE 152-2014 §5: leakage fraction = leakage flow at test pressure
    /// divided by fan airflow. Leakage flow = LC × (duct_area_ft2 / 100).
    pub fn to_leakage_fraction(self, duct_area_ft2: f64, fan_flow_cfm: f64) -> f64 {
        if fan_flow_cfm <= 0.0 || duct_area_ft2 <= 0.0 {
            return 0.0;
        }
        let leakage_flow_cfm = self.cfm_per_100ft2_at_25pa() * duct_area_ft2 / 100.0;
        (leakage_flow_cfm / fan_flow_cfm).clamp(0.0, 1.0)
    }
}

/// Location of the duct zone relative to the conditioned space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ashrae152ZoneType {
    AtticVented,
    AtticVentedRadiantBarrier,
    AtticUnvented,
    AtticUnventedRadiantBarrier,
    Garage,
    UnventUninsulatedCrawlspace,
    UnventCrawlspaceInsFloorWall,
    UnventCrawlspaceInsFloor,
    VentUninsulatedCrawlspace,
    VentCrawlspaceInsFloorWall,
    VentCrawlspaceInsFloor,
    UninsulatedBasement,
    BasementInsWalls,
    BasementInsCeiling,
    UnderSlab,
    ExteriorWalls,
}

impl Ashrae152ZoneType {
    /// Default duct insulation R-values (IP units: ft²·h·°F/Btu) from
    /// ASHRAE 152-2014 Tables 5-A through 5-D.
    ///
    /// Returns `(supply_r_ip, return_r_ip)` — the default uninsulated R-values
    /// for supply and return ducts located in this zone type, keyed by
    /// heating vs. cooling season.
    pub fn default_insulation_r_ip(&self, _is_heating: bool) -> (f64, f64) {
        // ASHRAE 152-2014 Tables 5-A through 5-D define per-zone-type default
        // uninsulated duct R-values for heating and cooling seasons.
        // The standard text is not available in this codebase; R-1.7 is the
        // universal bare-sheet-metal-duct fallback used in OCHRE and the prior
        // HARES implementation. Per-zone-type values should be filled in when
        // the standard is obtained.
        // See Known Limitations in T-0412 Implementation Notes.
        (1.7, 1.7)
    }

    /// Default duct leakage class for supply and return ducts.
    ///
    /// ASHRAE 152-2014 Table 5 defines leakage classes that do not vary by
    /// season; `_is_heating` is accepted for API parity with
    /// `default_insulation_r_ip` and is ignored.
    ///
    /// Returns `(supply_class, return_class)`. Attic ducts default to
    /// `Unsealed` (most exposed), crawlspace and exterior ducts to `Sealed`,
    /// and basement/slab ducts to `WellSealed` (most protected).
    pub fn default_leakage_class(&self, _is_heating: bool) -> (DuctLeakageClass, DuctLeakageClass) {
        match self {
            Ashrae152ZoneType::AtticVented
            | Ashrae152ZoneType::AtticVentedRadiantBarrier
            | Ashrae152ZoneType::AtticUnvented
            | Ashrae152ZoneType::AtticUnventedRadiantBarrier => {
                (DuctLeakageClass::Unsealed, DuctLeakageClass::Unsealed)
            }
            Ashrae152ZoneType::Garage
            | Ashrae152ZoneType::UnventUninsulatedCrawlspace
            | Ashrae152ZoneType::UnventCrawlspaceInsFloorWall
            | Ashrae152ZoneType::UnventCrawlspaceInsFloor
            | Ashrae152ZoneType::VentUninsulatedCrawlspace
            | Ashrae152ZoneType::VentCrawlspaceInsFloorWall
            | Ashrae152ZoneType::VentCrawlspaceInsFloor
            | Ashrae152ZoneType::ExteriorWalls => {
                (DuctLeakageClass::Sealed, DuctLeakageClass::Sealed)
            }
            Ashrae152ZoneType::UninsulatedBasement
            | Ashrae152ZoneType::BasementInsWalls
            | Ashrae152ZoneType::BasementInsCeiling
            | Ashrae152ZoneType::UnderSlab => {
                (DuctLeakageClass::WellSealed, DuctLeakageClass::WellSealed)
            }
        }
    }

    /// Seasonal delivery effectiveness multiplier from ASHRAE 152-2014
    /// Tables 5-A through 5-D.
    ///
    /// These multipliers are empirical corrections that account for cyclic
    /// losses, part-load effects, and air distribution patterns not captured
    /// by the steady-state NTU-effectiveness model. Values vary by zone type,
    /// season (heating vs. cooling), and equipment type (heat pump vs.
    /// non-heat-pump for heating season).
    ///
    /// Returns a multiplier in (0.0, 1.0] per the ASHRAE 152 convention.
    pub fn seasonal_multiplier(&self, _is_heating: bool, _is_heat_pump: bool) -> f64 {
        // ASHRAE 152-2014 Tables 5-A (heating exterior), 5-B (heating interior),
        // 5-C (cooling exterior), and 5-D (cooling interior) define per-zone-type,
        // per-season seasonal delivery effectiveness multipliers.
        //
        // The standard text is not available in this codebase. All variants
        // currently return 1.0 (identity) so that existing DSE results are
        // preserved. Per-zone-type values from the standard tables must be
        // filled in when the standard is obtained.
        //
        // See Known Limitations in T-0414 Implementation Notes.
        1.0
    }
}

/// Inputs to the ASHRAE 152 DSE calculation.
///
/// All dimensional fields are **SI units**; conversion to IP happens internally.
pub struct DuctDseInput {
    pub zone_type: Ashrae152ZoneType,
    /// Decimal degrees north.
    pub latitude_deg: f64,
    /// Decimal degrees east (negative = west).
    pub longitude_deg: f64,
    /// Conditioned space volume (m³).
    pub house_volume_m3: f64,
    /// Supply duct leakage as a fraction of fan flow (0–1).
    pub supply_leakage_frac: f64,
    /// Optional ASHRAE 152 leakage class override for supply ducts.
    /// When present, the leakage fraction is derived from this class,
    /// duct surface area, and fan airflow.
    pub supply_leakage_class: Option<DuctLeakageClass>,
    /// Supply duct surface area (m²).
    pub supply_area_m2: f64,
    /// Supply duct nominal R-value (m²·K/W); ≤ 0 → uninsulated default.
    pub supply_r_nominal_m2_k_w: f64,
    /// Return duct leakage as a fraction of fan flow (0–1).
    pub return_leakage_frac: f64,
    /// Optional ASHRAE 152 leakage class override for return ducts.
    /// When present, the leakage fraction is derived from this class,
    /// duct surface area, and fan airflow.
    pub return_leakage_class: Option<DuctLeakageClass>,
    /// Return duct surface area (m²).
    pub return_area_m2: f64,
    /// Return duct nominal R-value (m²·K/W); ≤ 0 → uninsulated default.
    pub return_r_nominal_m2_k_w: f64,
    /// `true` for a heating system, `false` for cooling.
    pub is_heating: bool,
    /// Rated system capacity (W).
    pub capacity_w: f64,
    /// Fan airflow at high speed (m³/s).
    pub fan_flow_m3_s: f64,
    /// Number of compressor / fan speeds (1 or 2+).
    pub n_speeds: u8,
    /// Rated capacity at low speed (W). Required when `n_speeds > 1`.
    pub capacity_low_w: Option<f64>,
    /// Fan airflow at low speed (m³/s). Required when `n_speeds > 1`.
    pub fan_flow_low_m3_s: Option<f64>,
    /// `true` for a heat-pump heating system (affects equipment factor).
    pub is_heat_pump: bool,
    /// Burial depth of duct below slab grade [m].
    ///
    /// Only relevant for [`Ashrae152ZoneType::UnderSlab`]. When `Some` and
    /// positive, enables a bounded exponential interpolation between the
    /// conditioned-space reference temperature and the deep ground temperature.
    /// This interpolation is **unvalidated** — the exponential decay shape has
    /// not been verified against ASHRAE Standard 152-2014 and is a
    /// mathematical placeholder pending tabular correction factors (T-1965).
    /// When `None` or zero, zone temperature falls back to `gnd`.
    pub burial_depth_m: Option<f64>,
    /// Soil thermal conductivity [W/(m·K)].
    ///
    /// Only relevant for [`Ashrae152ZoneType::UnderSlab`]. Used to compute
    /// soil thermal diffusivity α = k/(ρc). When `None` or non-positive,
    /// the correction is not applied and zone temperature falls back to `gnd`.
    pub soil_conductivity_w_m_k: Option<f64>,
}

// ---------------------------------------------------------------------------
// Internal climate station
// ---------------------------------------------------------------------------

struct ClimateStation {
    // Why: `state` is parsed from the CSV and asserted in tests to guard against
    // province/state code errors (e.g. MN for Winnipeg). Clippy's dead_code lint
    // cannot see test-only reads of a private field.
    #[allow(dead_code)]
    state: &'static str,
    latitude_deg: f64,
    longitude_deg: f64,
    heating_design_temp_f: f64,
    heating_seasonal_temp_f: f64,
    cooling_design_temp_f: f64,
    cooling_seasonal_temp_f: f64,
    w_seasonal: f64,
    seasonal_h_out: f64,
    seasonal_h_in: f64,
}

// ---------------------------------------------------------------------------
// Lazy-parsed climate data
// ---------------------------------------------------------------------------

static CLIMATE_DATA: OnceLock<Vec<ClimateStation>> = OnceLock::new();

const CLIMATE_CSV: &str = include_str!("../data/ASHRAE152_climate_data.csv");

fn climate_data() -> &'static [ClimateStation] {
    CLIMATE_DATA.get_or_init(parse_climate_csv)
}

fn parse_climate_csv() -> Vec<ClimateStation> {
    // Header: Index,Location,State,Latitude,Longitude,
    //         Heating Design Temp,Heating Seasonal Temp,
    //         Cooling Design Temp,Cooling Seasonal Temp,
    //         Wdesign,Wseasonal,Windesign,Winseasonal,
    //         Design hout,Seasonal hout,Design hin,Seasonal hin
    let mut stations = Vec::new();
    for line in CLIMATE_CSV.lines().skip(1) {
        let fields: Vec<&str> = line.split(',').collect();
        if fields.len() < 17 {
            continue;
        }
        // Parse a field by index, skipping the row on any parse failure or
        // empty cooling fields (some Alaska entries).
        let parse = |idx: usize| -> Option<f64> {
            let s = fields[idx].trim();
            if s.is_empty() {
                None
            } else {
                s.parse::<f64>().ok()
            }
        };
        let Some(lat) = parse(3) else { continue };
        let Some(lon) = parse(4) else { continue };
        let Some(htg_des) = parse(5) else { continue };
        let Some(htg_seas) = parse(6) else { continue };
        let Some(clg_des) = parse(7) else { continue };
        let Some(clg_seas) = parse(8) else { continue };
        let Some(w_seas) = parse(10) else { continue };
        let Some(seas_h_out) = parse(14) else {
            continue;
        };
        let Some(seas_h_in) = parse(16) else { continue };

        stations.push(ClimateStation {
            state: fields[2],
            latitude_deg: lat,
            longitude_deg: lon,
            heating_design_temp_f: htg_des,
            heating_seasonal_temp_f: htg_seas,
            cooling_design_temp_f: clg_des,
            cooling_seasonal_temp_f: clg_seas,
            w_seasonal: w_seas,
            seasonal_h_out: seas_h_out,
            seasonal_h_in: seas_h_in,
        });
    }
    stations
}

// ---------------------------------------------------------------------------
// Haversine nearest-station lookup
// ---------------------------------------------------------------------------

fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (dlon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());
    6_373.0 * c
}

fn nearest_station(lat: f64, lon: f64) -> &'static ClimateStation {
    climate_data()
        .iter()
        .min_by(|a, b| {
            let da = haversine_km(lat, lon, a.latitude_deg, a.longitude_deg);
            let db = haversine_km(lat, lon, b.latitude_deg, b.longitude_deg);
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
        // Safety: the CSV is embedded and always has ≥1 valid row after parsing.
        .expect("climate data must not be empty")
}

// ---------------------------------------------------------------------------
// Buried-duct soil temperature correction
// ---------------------------------------------------------------------------

/// Interpolated soil temperature at duct burial depth beneath a conditioned slab.
///
/// Returns a monotonic, bounded exponential interpolation between the
/// conditioned-space reference temperature (68°F heating, 78°F cooling) and
/// the deep ground temperature:
///
/// ```text
/// T(z) = T_gnd + (T_conditioned − T_gnd) × exp(−z / D_char)
/// ```
///
/// where `D_char = √(α·τ/π)` is a characteristic depth computed from the
/// soil thermal diffusivity α = k / (ρc) and an annual period τ = 365 days.
/// This produces a physically bounded but **unvalidated** interpolation: soil
/// temperature at shallow depth approaches `T_conditioned`, while soil at
/// depth approaches `T_gnd`, with an exponential decay shape that is not
/// derived from the steady-state conduction equation for this boundary
/// condition.
///
/// **This function is a mathematical placeholder.** The exponential decay
/// shape and characteristic depth have not been verified against ASHRAE
/// Standard 152-2014. Tabular buried-duct correction factors from the
/// standard (if they exist) would replace this interpolation; see T-1965.
///
/// All temperatures in °F. Depth in metres; conductivity in W/(m·K).
///
/// # References
///
/// - Ingersoll, L.R., Zobel, O.J. & Ingersoll, A.C. (1954), *Heat Conduction
///   with Engineering, Geological, and Other Applications*, §2.4 — ρc =
///   2.56 MJ/(m³·K) for average moist soil.
fn soil_temp_at_burial_depth_f(
    t_conditioned_f: f64,
    gnd_temp_f: f64,
    burial_depth_m: f64,
    soil_conductivity_w_m_k: f64,
) -> f64 {
    if burial_depth_m <= 0.0 || soil_conductivity_w_m_k <= 0.0 {
        return gnd_temp_f;
    }
    // α = k / (ρc) — thermal diffusivity [m²/s]
    let alpha_m2_per_s = soil_conductivity_w_m_k / SOIL_VOLUMETRIC_HEAT_CAPACITY_J_M3_K;
    // τ = 365 days [s] — annual period for damping depth
    let tau_s = 365.0 * SECONDS_PER_DAY;
    // Unvalidated characteristic depth: √(α·τ/π) with τ = 365 days.
    // The exponential decay shape is not derived from steady-state
    // conduction for this boundary condition. See T-0415 Known Limitations.
    let damping_depth_m = (alpha_m2_per_s * tau_s / PI).sqrt();
    let attenuation = (-burial_depth_m / damping_depth_m).exp();
    gnd_temp_f + (t_conditioned_f - gnd_temp_f) * attenuation
}

// ---------------------------------------------------------------------------
// Zone temperature formulas
// ---------------------------------------------------------------------------

/// Climate station temperatures passed to [`zone_temps`].
struct StationTemps {
    /// Heating design dry-bulb temperature [°F]
    h_des: f64,
    /// Heating seasonal dry-bulb temperature [°F]
    h_seas: f64,
    /// Cooling design dry-bulb temperature [°F]
    c_des: f64,
    /// Cooling seasonal dry-bulb temperature [°F]
    c_seas: f64,
    /// Ground temperature — mean of heating and cooling design [°F]
    gnd: f64,
}

/// All temperatures in °F.
/// Returns `(htg_des, htg_seas, clg_des, clg_seas, supply_regain, return_regain)`.
fn zone_temps(
    zone: Ashrae152ZoneType,
    station: &StationTemps,
    burial_depth_m: Option<f64>,
    soil_conductivity_w_m_k: Option<f64>,
) -> (f64, f64, f64, f64, f64, f64) {
    // When burial parameters are present for an UnderSlab duct, compute
    // an interpolated soil temperature at burial depth (bounded by the
    // conditioned-space reference temperature and deep ground temperature).
    // When absent, fall back to gnd.
    let under_slab_zone_temp = |t_conditioned_f: f64| -> f64 {
        match (burial_depth_m, soil_conductivity_w_m_k) {
            (Some(depth), Some(k)) if depth > 0.0 && k > 0.0 => {
                soil_temp_at_burial_depth_f(t_conditioned_f, station.gnd, depth, k)
            }
            _ => station.gnd,
        }
    };

    match zone {
        Ashrae152ZoneType::AtticVented => (
            station.h_des + 10.0,
            station.h_seas + 7.0,
            station.c_des + 22.0,
            station.c_seas + 13.0,
            0.1,
            0.1,
        ),
        Ashrae152ZoneType::AtticVentedRadiantBarrier => (
            station.h_des + 10.0,
            station.h_seas + 7.0,
            0.65 * (station.c_des + 22.0) + 0.35 * 78.0,
            0.7 * (station.c_seas + 13.0) + 0.3 * 78.0,
            0.1,
            0.1,
        ),
        Ashrae152ZoneType::AtticUnvented => (
            station.h_des + 10.0,
            station.h_seas + 7.0,
            station.c_des + 36.0,
            station.c_seas + 16.0,
            0.1,
            0.1,
        ),
        Ashrae152ZoneType::AtticUnventedRadiantBarrier => (
            station.h_des + 10.0,
            station.h_seas + 7.0,
            0.65 * (station.c_des + 36.0) + 0.35 * 78.0,
            0.7 * (station.c_seas + 16.0) + 0.3 * 78.0,
            0.1,
            0.1,
        ),
        Ashrae152ZoneType::Garage => (
            station.h_des + 13.0,
            station.h_seas + 11.0,
            station.c_des + 7.0,
            station.c_seas + 7.0,
            0.1,
            0.1,
        ),
        Ashrae152ZoneType::UnventUninsulatedCrawlspace => (
            (2.0 * station.h_des + 3.0 * 68.0) / 5.0,
            (2.0 * station.h_seas + 3.0 * 68.0) / 5.0,
            (2.0 * station.c_des + 3.0 * 78.0) / 5.0,
            (2.0 * station.c_seas + 3.0 * 78.0) / 5.0,
            0.6,
            0.6,
        ),
        Ashrae152ZoneType::UnventCrawlspaceInsFloorWall => (
            (3.0 * station.h_des + 68.0) / 4.0,
            (3.0 * station.h_seas + 68.0) / 4.0,
            (3.0 * station.c_des + 78.0) / 4.0,
            (3.0 * station.c_seas + 78.0) / 4.0,
            0.6,
            0.6,
        ),
        Ashrae152ZoneType::UnventCrawlspaceInsFloor => (
            (5.0 * station.h_des + 68.0) / 6.0,
            (5.0 * station.h_seas + 68.0) / 6.0,
            (5.0 * station.c_des + 78.0) / 6.0,
            (5.0 * station.c_seas + 78.0) / 6.0,
            0.3,
            0.3,
        ),
        Ashrae152ZoneType::VentUninsulatedCrawlspace => (
            (station.h_des + 68.0) / 2.0,
            (station.h_seas + 68.0) / 2.0,
            (station.c_des + 78.0) / 2.0,
            (station.c_seas + 78.0) / 2.0,
            0.6,
            0.6,
        ),
        Ashrae152ZoneType::VentCrawlspaceInsFloorWall => (
            (5.0 * station.h_des + 68.0) / 6.0,
            (5.0 * station.h_seas + 68.0) / 6.0,
            (5.0 * station.c_des + 78.0) / 6.0,
            (5.0 * station.c_seas + 78.0) / 6.0,
            0.63,
            0.63,
        ),
        Ashrae152ZoneType::VentCrawlspaceInsFloor => (
            (8.0 * station.h_des + 68.0) / 9.0,
            (8.0 * station.h_seas + 68.0) / 9.0,
            (8.0 * station.c_des + 78.0) / 9.0,
            (8.0 * station.c_seas + 78.0) / 9.0,
            0.3,
            0.3,
        ),
        Ashrae152ZoneType::UninsulatedBasement => (
            (5.0 * station.gnd + 2.0 * station.h_des + 3.0 * 68.0) / 10.0,
            (5.0 * station.gnd + 2.0 * station.h_seas + 3.0 * 68.0) / 10.0,
            (5.0 * station.gnd + 2.0 * station.c_des + 3.0 * 78.0) / 10.0,
            (5.0 * station.gnd + 2.0 * station.c_seas + 3.0 * 78.0) / 10.0,
            0.5,
            0.5,
        ),
        Ashrae152ZoneType::BasementInsWalls => (
            (station.gnd + 68.0) / 2.0,
            (station.gnd + 68.0) / 2.0,
            (8.0 * station.gnd + station.c_des + 78.0) / 10.0,
            (8.0 * station.gnd + station.c_seas + 78.0) / 10.0,
            0.6,
            0.6,
        ),
        Ashrae152ZoneType::BasementInsCeiling => (
            (3.0 * station.gnd + station.h_des) / 4.0,
            (3.0 * station.gnd + station.h_seas) / 4.0,
            (3.0 * station.gnd + station.c_des) / 4.0,
            (3.0 * station.gnd + station.c_seas) / 4.0,
            0.6,
            0.6,
        ),
        Ashrae152ZoneType::UnderSlab => {
            // Conditioned space reference temperatures per ASHRAE 152:
            // 68°F for heating, 78°F for cooling.
            let soil_htg_des = under_slab_zone_temp(68.0);
            let soil_htg_seas = under_slab_zone_temp(68.0);
            let soil_clg_des = under_slab_zone_temp(78.0);
            let soil_clg_seas = under_slab_zone_temp(78.0);
            (
                soil_htg_des,
                soil_htg_seas,
                soil_clg_des,
                soil_clg_seas,
                0.2,
                0.2,
            )
        }
        Ashrae152ZoneType::ExteriorWalls => (
            (station.h_des + 68.0) / 2.0,
            (station.h_seas + 68.0) / 2.0,
            (station.c_des + 78.0) / 2.0,
            (station.c_seas + 78.0) / 2.0,
            0.2,
            0.2,
        ),
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Look up the nearest ASHRAE 152 climate station and return its
/// heating and cooling design dry-bulb temperatures in °F.
///
/// Returns `None` when lat/lon are both 0.0 (unset site location).
pub fn design_temperatures_f(lat: f64, lon: f64) -> Option<(f64, f64)> {
    if lat.abs() < f64::EPSILON && lon.abs() < f64::EPSILON {
        return None;
    }
    let station = nearest_station(lat, lon);
    let station_distance_km = haversine_km(lat, lon, station.latitude_deg, station.longitude_deg);
    if station_distance_km > 200.0 {
        tracing::warn!(
            input_lat = lat,
            input_lon = lon,
            station_lat = station.latitude_deg,
            station_lon = station.longitude_deg,
            distance_km = station_distance_km,
            "ASHRAE 152: nearest climate station for design temperatures is {:.0} km away — \
             design temperatures may be unreliable",
            station_distance_km
        );
    }
    Some((station.heating_design_temp_f, station.cooling_design_temp_f))
}

/// Calculate the ASHRAE 152 Duct Distribution System Efficiency.
///
/// Returns a DSE clamped to `(0.0, 1.0]`.  All inputs must be in SI units;
/// see [`DuctDseInput`] for field documentation.
pub fn calculate_dse(input: &DuctDseInput) -> f64 {
    let hvac_mult = if input.is_heating { 1.0 } else { -1.0 };

    // ------------------------------------------------------------------
    // 1. Convert inputs to IP units
    // ------------------------------------------------------------------
    let house_volume_ft3 = input.house_volume_m3 * M3_TO_FT3;
    let supply_area_ft2 = input.supply_area_m2 * M2_TO_FT2;
    let return_area_ft2 = input.return_area_m2 * M2_TO_FT2;
    let supply_nom_r_ip = input.supply_r_nominal_m2_k_w * SI_R_TO_IP_R;
    let return_nom_r_ip = input.return_r_nominal_m2_k_w * SI_R_TO_IP_R;
    let capacity_btu_h = input.capacity_w * W_TO_BTU_H;
    let fan_flow_cfm = input.fan_flow_m3_s * M3S_TO_CFM;

    // ------------------------------------------------------------------
    // 2. R-value transform
    // ------------------------------------------------------------------
    let (default_supply_r_ip, default_return_r_ip) =
        input.zone_type.default_insulation_r_ip(input.is_heating);

    let supply_r = if supply_nom_r_ip <= 0.0 {
        default_supply_r_ip
    } else {
        2.2438 + 0.5619 * supply_nom_r_ip
    };
    let return_r = if return_nom_r_ip <= 0.0 {
        default_return_r_ip
    } else {
        2.0388 + 0.7053 * return_nom_r_ip
    };

    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    {
        // ASHRAE 152-2014 §5: resolved effective R-value must be positive —
        // a non-positive value implies a degenerate duct configuration.
        assert!(
            supply_r > 0.0,
            "resolved supply effective R-value must be positive, got {supply_r}"
        );
        assert!(
            return_r > 0.0,
            "resolved return effective R-value must be positive, got {return_r}"
        );
    }

    #[cfg(feature = "observe")]
    {
        let supply_used_default = supply_nom_r_ip <= 0.0;
        let return_used_default = return_nom_r_ip <= 0.0;
        if supply_used_default || return_used_default {
            tracing::debug!(
                zone_type = ?input.zone_type,
                is_heating = input.is_heating,
                default_supply_r_ip,
                default_return_r_ip,
                supply_nom_r_ip,
                return_nom_r_ip,
                resolved_supply_r = supply_r,
                resolved_return_r = return_r,
                supply_used_default,
                return_used_default,
                "ASHRAE 152 duct insulation R-value default applied"
            );
        }
    }

    // ------------------------------------------------------------------
    // 3. Climate station lookup
    // ------------------------------------------------------------------
    let station = nearest_station(input.latitude_deg, input.longitude_deg);
    let station_distance_km = haversine_km(
        input.latitude_deg,
        input.longitude_deg,
        station.latitude_deg,
        station.longitude_deg,
    );
    if station_distance_km > 200.0 {
        tracing::warn!(
            input_lat = input.latitude_deg,
            input_lon = input.longitude_deg,
            station_lat = station.latitude_deg,
            station_lon = station.longitude_deg,
            distance_km = station_distance_km,
            "ASHRAE 152 DSE: nearest climate station is {:.0} km away — \
             DSE may be unreliable",
            station_distance_km
        );
    }
    let heating_des_init = station.heating_design_temp_f;
    let heating_seas_init = station.heating_seasonal_temp_f;
    let cooling_des_init = station.cooling_design_temp_f;
    let cooling_seas_init = station.cooling_seasonal_temp_f;
    let seas_hr = station.w_seasonal;
    let seas_enthalpy = station.seasonal_h_out;
    let seas_in_enthalpy = station.seasonal_h_in;
    let ground_temp = (heating_des_init + cooling_des_init) / 2.0;

    // ------------------------------------------------------------------
    // 4. Zone temperatures
    // ------------------------------------------------------------------
    let (_htg_des, htg_seas, _clg_des, clg_seas, supply_regain, return_regain) = zone_temps(
        input.zone_type,
        &StationTemps {
            h_des: heating_des_init,
            h_seas: heating_seas_init,
            c_des: cooling_des_init,
            c_seas: cooling_seas_init,
            gnd: ground_temp,
        },
        input.burial_depth_m,
        input.soil_conductivity_w_m_k,
    );

    let ambient_temp = if input.is_heating { 68.0_f64 } else { 78.0_f64 };
    let seas_temp = if input.is_heating { htg_seas } else { clg_seas };

    #[cfg(feature = "observe")]
    {
        if matches!(input.zone_type, Ashrae152ZoneType::UnderSlab) {
            let correction_applied =
                input.burial_depth_m.is_some() && input.soil_conductivity_w_m_k.is_some();
            if correction_applied {
                let depth = input.burial_depth_m.unwrap_or(0.0);
                let k = input.soil_conductivity_w_m_k.unwrap_or(0.0);
                let correction_delta_f = seas_temp - ground_temp;
                tracing::debug!(
                    target: "observe",
                    column = "ashrae152_under_slab_soil_correction",
                    zone_type = ?input.zone_type,
                    is_heating = input.is_heating,
                    burial_depth_m = depth,
                    soil_conductivity_w_m_k = k,
                    corrected_zone_temp_f = seas_temp,
                    raw_ground_temp_f = ground_temp,
                    correction_delta_f,
                    "ASHRAE 152 under-slab soil temperature correction applied: \
                     zone temp = {:.1}°F, gnd = {:.1}°F, delta = {:.1}°F",
                    seas_temp, ground_temp, correction_delta_f
                );
            } else {
                tracing::debug!(
                    target: "observe",
                    column = "ashrae152_under_slab_soil_correction",
                    zone_type = ?input.zone_type,
                    is_heating = input.is_heating,
                    correction_applied = false,
                    burial_depth_provided = input.burial_depth_m.is_some(),
                    soil_conductivity_provided = input.soil_conductivity_w_m_k.is_some(),
                    raw_ground_temp_f = ground_temp,
                    "ASHRAE 152 under-slab soil temperature correction not applied — \
                     burial depth and/or soil conductivity not provided; \
                     using raw ground temperature gnd = {:.1}°F",
                    ground_temp
                );
            }
        }
    }

    // ------------------------------------------------------------------
    // 5. Supply / return zone temperatures
    // ------------------------------------------------------------------
    let seas_supply_zone_temp = seas_temp;
    let seas_return_zone_temp = if input.is_heating {
        if seas_temp > ambient_temp {
            (heating_seas_init + seas_supply_zone_temp) / 2.0
        } else {
            seas_supply_zone_temp
        }
    } else {
        if seas_temp < ambient_temp {
            (cooling_seas_init + seas_supply_zone_temp) / 2.0
        } else {
            seas_supply_zone_temp
        }
    };

    // ------------------------------------------------------------------
    // 6. Enthalpy calculations
    // ------------------------------------------------------------------
    let seas_supply_zone_enthalpy =
        seas_supply_zone_temp * 0.24 + seas_hr * (1061.0 + 0.444 * seas_supply_zone_temp);
    let seas_return_zone_enthalpy =
        if seas_supply_zone_enthalpy * hvac_mult > seas_in_enthalpy * hvac_mult {
            (seas_enthalpy + seas_supply_zone_enthalpy) / 2.0
        } else {
            seas_supply_zone_enthalpy
        };

    // ------------------------------------------------------------------
    // 7. Cycle loss and infiltration baseline
    // ------------------------------------------------------------------
    let fcycloss = 0.05; // sheet metal default
    let infil_fan_off = 0.35 * house_volume_ft3 / 60.0;
    // manu_fan_flow is only used for cooling equipment factor
    let manu_fan_flow = if !input.is_heating {
        0.0333 * capacity_btu_h
    } else {
        0.0 // unused for heating
    };

    // ------------------------------------------------------------------
    // 8. Resolve leakage fractions from leakage class when specified
    // ------------------------------------------------------------------
    let resolved_supply_leakage_frac = match input.supply_leakage_class {
        Some(lc) => lc.to_leakage_fraction(supply_area_ft2, fan_flow_cfm),
        None => input.supply_leakage_frac,
    };
    let resolved_return_leakage_frac = match input.return_leakage_class {
        Some(lc) => lc.to_leakage_fraction(return_area_ft2, fan_flow_cfm),
        None => input.return_leakage_frac,
    };

    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    {
        // ASHRAE 152-2014 §5: resolved leakage fractions must be in [0, 1].
        assert!(
            (0.0..=1.0).contains(&resolved_supply_leakage_frac),
            "resolved supply leakage fraction must be in [0, 1], got {resolved_supply_leakage_frac}"
        );
        assert!(
            (0.0..=1.0).contains(&resolved_return_leakage_frac),
            "resolved return leakage fraction must be in [0, 1], got {resolved_return_leakage_frac}"
        );
    }

    #[cfg(feature = "observe")]
    {
        if input.supply_leakage_class.is_some() || input.return_leakage_class.is_some() {
            tracing::debug!(
                zone_type = ?input.zone_type,
                is_heating = input.is_heating,
                supply_leakage_class = ?input.supply_leakage_class,
                return_leakage_class = ?input.return_leakage_class,
                resolved_supply_leakage_frac,
                resolved_return_leakage_frac,
                raw_supply_leakage_frac = input.supply_leakage_frac,
                raw_return_leakage_frac = input.return_leakage_frac,
                "ASHRAE 152 duct leakage class resolved to leakage fraction"
            );
        }
    }

    // ------------------------------------------------------------------
    // 9. High-speed duct factors
    // ------------------------------------------------------------------
    let supply_duct_leakage = fan_flow_cfm * resolved_supply_leakage_frac;
    let return_duct_leakage = fan_flow_cfm * resolved_return_leakage_frac;

    let as_high = (fan_flow_cfm - supply_duct_leakage) / fan_flow_cfm;
    let ar_high = (fan_flow_cfm - return_duct_leakage) / fan_flow_cfm;

    let denom_high = 60.0 * fan_flow_cfm * 0.075 * 0.24;
    let dte_high = capacity_btu_h * hvac_mult / denom_high;
    let bs_high = f64::exp(-supply_area_ft2 / (denom_high * supply_r));
    let br_high = f64::exp(-return_area_ft2 / (denom_high * return_r));

    let seas_supply_temp_diff = ambient_temp - seas_supply_zone_temp;
    let seas_return_temp_diff = ambient_temp - seas_return_zone_temp;

    // Infiltration (based on high-speed leakage imbalance)
    let imb_flow = (supply_duct_leakage - return_duct_leakage).abs();
    let infil = if supply_duct_leakage > return_duct_leakage {
        (infil_fan_off.powf(1.5) + imb_flow.powf(1.5)).powf(0.67)
    } else if imb_flow > infil_fan_off {
        0.0
    } else {
        (infil_fan_off.powf(1.5) - imb_flow.powf(1.5)).powf(0.67)
    };

    // ------------------------------------------------------------------
    // 10. Low-speed duct factors (multi-speed only)
    // ------------------------------------------------------------------
    let (as_low, ar_low, dte_low, bs_low, br_low) = if input.n_speeds > 1 {
        let cap_low = input.capacity_low_w.unwrap_or(input.capacity_w) * W_TO_BTU_H;
        let flow_low = input.fan_flow_low_m3_s.unwrap_or(input.fan_flow_m3_s) * M3S_TO_CFM;

        let sdl_low = flow_low * resolved_supply_leakage_frac;
        let rdl_low = flow_low * resolved_return_leakage_frac;

        let as_l = (flow_low - sdl_low) / flow_low;
        let ar_l = (flow_low - rdl_low) / flow_low;

        let denom_low = 60.0 * flow_low * 0.075 * 0.24;
        let dte_l = cap_low * hvac_mult / denom_low;
        let bs_l = f64::exp(-supply_area_ft2 / (denom_low * supply_r));
        let br_l = f64::exp(-return_area_ft2 / (denom_low * return_r));

        (as_l, ar_l, dte_l, bs_l, br_l)
    } else {
        // Values are only read when n_speeds > 1; placeholders here.
        (as_high, ar_high, dte_high, bs_high, br_high)
    };

    // ------------------------------------------------------------------
    // 11. Uncorrected delivery effectiveness
    // ------------------------------------------------------------------
    let seas_uncorr_de = if input.is_heating {
        if input.n_speeds == 1 {
            as_high * bs_high
                - as_high * bs_high * (1.0 - br_high * ar_high) * seas_return_temp_diff / dte_high
                - as_high * (1.0 - bs_high) * seas_supply_temp_diff / dte_high
        } else {
            as_low * bs_low
                - as_low * bs_low * (1.0 - br_low * ar_low) * seas_return_temp_diff / dte_low
                - as_low * (1.0 - bs_low) * seas_supply_temp_diff / dte_low
        }
    } else {
        // Cooling
        if input.n_speeds == 1 {
            as_high * fan_flow_cfm * 60.0 * 0.075 / (-capacity_btu_h)
                * (-capacity_btu_h / fan_flow_cfm / (0.075 * 60.0)
                    + (1.0 - ar_high) * (seas_return_zone_enthalpy - seas_in_enthalpy)
                    + 0.24 * ar_high * (br_high - 1.0) * (ambient_temp - seas_return_zone_temp)
                    + 0.24 * (bs_high - 1.0) * (55.0 - seas_supply_zone_temp))
        } else {
            let cap_low_btu = input.capacity_low_w.unwrap_or(input.capacity_w) * W_TO_BTU_H;
            let flow_low_cfm = input.fan_flow_low_m3_s.unwrap_or(input.fan_flow_m3_s) * M3S_TO_CFM;
            as_low * flow_low_cfm * 60.0 * 0.075 / (-cap_low_btu)
                * (-cap_low_btu / flow_low_cfm / (0.075 * 60.0)
                    + (1.0 - ar_low) * (seas_return_zone_enthalpy - seas_in_enthalpy)
                    + 0.24 * ar_low * (br_low - 1.0) * (ambient_temp - seas_return_zone_temp)
                    + 0.24 * (bs_low - 1.0) * (55.0 - seas_supply_zone_temp))
        }
    };

    // ------------------------------------------------------------------
    // 12. Load factor
    // ------------------------------------------------------------------
    let seas_load_factor = if input.is_heating {
        1.0 - (60.0 * 0.075 * 0.24 * (ambient_temp - heating_seas_init) * (infil - infil_fan_off))
            / seas_uncorr_de
            / capacity_btu_h
    } else {
        1.0 - (60.0 * 0.075 * (infil - infil_fan_off) * (seas_in_enthalpy - seas_enthalpy))
            / (-capacity_btu_h)
            / seas_uncorr_de
    };

    // ------------------------------------------------------------------
    // 13. Equipment factor
    // ------------------------------------------------------------------
    let seas_equip_factor = if input.is_heating {
        if input.n_speeds == 1 {
            1.0
        } else if input.is_heat_pump {
            0.44 + 0.56 * seas_uncorr_de
        } else {
            0.91 + 0.09 * seas_uncorr_de
        }
    } else {
        // Cooling; TXV control assumed throughout
        if input.n_speeds == 1 {
            1.62 - 0.62 * fan_flow_cfm / manu_fan_flow + 0.647 * (fan_flow_cfm / manu_fan_flow).ln()
        } else {
            (0.82 + 0.18 * seas_uncorr_de) * (1.62 - 0.62 * fan_flow_cfm / manu_fan_flow)
                + 0.647 * (fan_flow_cfm / manu_fan_flow).ln()
        }
    };

    // ------------------------------------------------------------------
    // 14. Delivery effectiveness with thermal regain
    // ------------------------------------------------------------------
    let seas_de = seas_uncorr_de + supply_regain * (1.0 - seas_uncorr_de)
        - (supply_regain - return_regain - br_high * (ar_high * supply_regain - return_regain))
            * seas_return_temp_diff
            / dte_high;

    // ------------------------------------------------------------------
    // 15. Seasonal delivery effectiveness multiplier
    // ------------------------------------------------------------------
    // ASHRAE 152-2014 Tables 5-A through 5-D specify per-zone-type, per-season
    // empirical multipliers that account for cyclic losses, part-load effects,
    // and air distribution patterns not captured by the steady-state
    // NTU-effectiveness model. These are applied after regain but before
    // equipment, load, and cycle-loss factors.
    let seasonal_mult = input
        .zone_type
        .seasonal_multiplier(input.is_heating, input.is_heat_pump);

    #[cfg(feature = "observe")]
    {
        tracing::debug!(
            target: "observe",
            column = "ashrae152_seasonal_multiplier",
            zone_type = ?input.zone_type,
            is_heating = input.is_heating,
            is_heat_pump = input.is_heat_pump,
            seasonal_multiplier = seasonal_mult,
            "ASHRAE 152 seasonal delivery effectiveness multiplier applied"
        );
    }

    let seas_de = seas_de * seasonal_mult;

    // ------------------------------------------------------------------
    // 16. Final DSE
    // ------------------------------------------------------------------
    let seas_dse = seas_de * seas_equip_factor * seas_load_factor * (1.0 - fcycloss);
    if !seas_dse.is_finite() {
        // Degenerate duct config (e.g. extreme leakage) produced NaN/Inf.
        // Fall back to no distribution loss rather than propagating poison.
        return 1.0;
    }
    seas_dse.clamp(f64::MIN_POSITIVE, 1.0)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn climate_data_loads_with_sufficient_stations() {
        let data = climate_data();
        assert!(
            data.len() > 200,
            "expected >200 stations, got {}",
            data.len()
        );
    }

    /// AtticVented heating seasonal temp: h_seas + 7.
    /// With heating_seas_init = 40 °F → seas_temp = 47 °F.
    #[test]
    fn attic_vented_heating_seas_temp_formula() {
        // Dummy climate values -- only h_seas matters for this formula.
        let h_seas = 40.0_f64;
        let (_htg_des, htg_seas, _clg_des, _clg_seas, _sr, _rr) = zone_temps(
            Ashrae152ZoneType::AtticVented,
            &StationTemps {
                h_des: 10.0,
                h_seas,
                c_des: 90.0,
                c_seas: 75.0,
                gnd: 50.0,
            },
            None,
            None,
        );
        assert!(
            (htg_seas - 47.0).abs() < 1e-9,
            "expected htg_seas=47.0, got {htg_seas}"
        );
    }

    /// The nearest station to Denver (lat=39.7, lon=-104.9) should be close to
    /// the Denver entry in the CSV (Denver is at 39.74, -104.87).
    #[test]
    fn haversine_finds_denver_station() {
        let station = nearest_station(39.7, -104.9);
        let dist = haversine_km(39.7, -104.9, station.latitude_deg, station.longitude_deg);
        assert!(
            dist < 100.0,
            "nearest station to Denver is {dist:.1} km away -- expected <100 km"
        );
        // Denver heating design temp is well below zero °C but above -10 °F
        assert!(
            station.heating_design_temp_f > -10.0 && station.heating_design_temp_f < 20.0,
            "unexpected heating design temp for Denver station: {}",
            station.heating_design_temp_f
        );
    }

    /// Smoke test: calculate_dse returns a value in (0, 1] for a
    /// representative single-speed heating scenario.
    #[test]
    fn dse_heating_single_speed_in_bounds() {
        let input = DuctDseInput {
            zone_type: Ashrae152ZoneType::AtticVented,
            latitude_deg: 39.74,
            longitude_deg: -104.87,
            house_volume_m3: 340.0, // ~12 000 ft³
            supply_leakage_frac: 0.10,
            supply_leakage_class: None,
            supply_area_m2: 9.29,          // ~100 ft²
            supply_r_nominal_m2_k_w: 1.76, // ~R-10 IP
            return_leakage_frac: 0.06,
            return_leakage_class: None,
            return_area_m2: 4.65, // ~50 ft²
            return_r_nominal_m2_k_w: 1.76,
            is_heating: true,
            capacity_w: 14_650.0, // ~50 000 Btu/h
            fan_flow_m3_s: 0.566, // ~1 200 CFM
            n_speeds: 1,
            capacity_low_w: None,
            fan_flow_low_m3_s: None,
            is_heat_pump: false,
            burial_depth_m: None,
            soil_conductivity_w_m_k: None,
        };
        let dse = calculate_dse(&input);
        assert!(dse > 0.0 && dse <= 1.0, "DSE out of bounds: {dse}");
        // Typical ASHRAE 152 result for this configuration is 0.70–0.95
        assert!(dse > 0.6, "DSE unexpectedly low: {dse}");
    }

    /// Smoke test for single-speed cooling.
    #[test]
    fn dse_cooling_single_speed_in_bounds() {
        let input = DuctDseInput {
            zone_type: Ashrae152ZoneType::AtticVented,
            latitude_deg: 33.45,
            longitude_deg: -112.02, // Phoenix
            house_volume_m3: 340.0,
            supply_leakage_frac: 0.10,
            supply_leakage_class: None,
            supply_area_m2: 9.29,
            supply_r_nominal_m2_k_w: 1.76,
            return_leakage_frac: 0.06,
            return_leakage_class: None,
            return_area_m2: 4.65,
            return_r_nominal_m2_k_w: 1.76,
            is_heating: false,
            capacity_w: 10_550.0, // ~36 000 Btu/h (3 ton)
            fan_flow_m3_s: 0.472, // ~1 000 CFM
            n_speeds: 1,
            capacity_low_w: None,
            fan_flow_low_m3_s: None,
            is_heat_pump: false,
            burial_depth_m: None,
            soil_conductivity_w_m_k: None,
        };
        let dse = calculate_dse(&input);
        assert!(dse > 0.0 && dse <= 1.0, "DSE out of bounds: {dse}");
    }

    #[test]
    fn dse_cooling_multi_speed_in_bounds() {
        let input = DuctDseInput {
            zone_type: Ashrae152ZoneType::AtticVented,
            latitude_deg: 33.45,
            longitude_deg: -112.02,
            house_volume_m3: 340.0,
            supply_leakage_frac: 0.10,
            supply_leakage_class: None,
            supply_area_m2: 9.29,
            supply_r_nominal_m2_k_w: 1.76,
            return_leakage_frac: 0.06,
            return_leakage_class: None,
            return_area_m2: 4.65,
            return_r_nominal_m2_k_w: 1.76,
            is_heating: false,
            capacity_w: 10_550.0,
            fan_flow_m3_s: 0.472,
            n_speeds: 2,
            capacity_low_w: Some(7_000.0),
            fan_flow_low_m3_s: Some(0.330),
            is_heat_pump: false,
            burial_depth_m: None,
            soil_conductivity_w_m_k: None,
        };
        let dse = calculate_dse(&input);
        assert!(
            dse > 0.0 && dse <= 1.0,
            "multi-speed cooling DSE out of bounds: {dse}"
        );
    }

    #[test]
    fn dse_heating_and_cooling_derive_correct_hvac_mult_sign() {
        let heating_input = DuctDseInput {
            zone_type: Ashrae152ZoneType::AtticVented,
            latitude_deg: 39.74,
            longitude_deg: -104.87,
            house_volume_m3: 340.0,
            supply_leakage_frac: 0.10,
            supply_leakage_class: None,
            supply_area_m2: 9.29,
            supply_r_nominal_m2_k_w: 1.76,
            return_leakage_frac: 0.06,
            return_leakage_class: None,
            return_area_m2: 4.65,
            return_r_nominal_m2_k_w: 1.76,
            is_heating: true,
            capacity_w: 14_650.0,
            fan_flow_m3_s: 0.566,
            n_speeds: 1,
            capacity_low_w: None,
            fan_flow_low_m3_s: None,
            is_heat_pump: false,
            burial_depth_m: None,
            soil_conductivity_w_m_k: None,
        };
        let dse_h = calculate_dse(&heating_input);
        assert!(dse_h > 0.0 && dse_h <= 1.0, "heating DSE: {dse_h}");

        let cooling_input = DuctDseInput {
            zone_type: Ashrae152ZoneType::AtticVented,
            latitude_deg: 33.45,
            longitude_deg: -112.02,
            house_volume_m3: 340.0,
            supply_leakage_frac: 0.10,
            supply_leakage_class: None,
            supply_area_m2: 9.29,
            supply_r_nominal_m2_k_w: 1.76,
            return_leakage_frac: 0.06,
            return_leakage_class: None,
            return_area_m2: 4.65,
            return_r_nominal_m2_k_w: 1.76,
            is_heating: false,
            capacity_w: 10_550.0,
            fan_flow_m3_s: 0.472,
            n_speeds: 1,
            capacity_low_w: None,
            fan_flow_low_m3_s: None,
            is_heat_pump: false,
            burial_depth_m: None,
            soil_conductivity_w_m_k: None,
        };
        let dse_c = calculate_dse(&cooling_input);
        assert!(dse_c > 0.0 && dse_c <= 1.0, "cooling DSE: {dse_c}");
    }

    /// Regression: Montreal has State == "QC", Winnipeg has State == "MB".
    #[test]
    fn canadian_province_codes_correct_after_parse() {
        let stations = climate_data();
        // Find Montreal (≈45.5°N, −73.57°W) and Winnipeg (≈49.9°N, −97.14°W)
        // by coordinates since some Alaska rows are skipped during parsing.
        let montreal = stations
            .iter()
            .find(|s| {
                (s.latitude_deg - 45.5).abs() < 0.1 && (s.longitude_deg - (-73.57)).abs() < 0.1
            })
            .expect("Montreal station not found");
        assert_eq!(
            montreal.state, "QC",
            "Montreal State should be QC, got {}",
            montreal.state
        );
        let winnipeg = stations
            .iter()
            .find(|s| {
                (s.latitude_deg - 49.9).abs() < 0.1 && (s.longitude_deg - (-97.14)).abs() < 0.1
            })
            .expect("Winnipeg station not found");
        assert_eq!(
            winnipeg.state, "MB",
            "Winnipeg State should be MB, got {}",
            winnipeg.state
        );
    }

    /// No row with State == "MN" should have coordinates in Canadian territory
    /// (lat ≥ 49°N, long between −141°W and −52°W).
    #[test]
    fn no_minnesota_in_canadian_territory() {
        let stations = climate_data();
        for station in stations {
            if station.state == "MN" {
                let in_canada = station.latitude_deg >= 49.0
                    && station.longitude_deg >= -141.0
                    && station.longitude_deg <= -52.0;
                assert!(
                    !in_canada,
                    "station with State=MN has Canadian coordinates: lat={}, lon={}",
                    station.latitude_deg, station.longitude_deg
                );
            }
        }
    }

    /// Remote Pacific location (0°N, 170°W) is far from any ASHRAE 152
    /// station, but `calculate_dse()` must still return a value in (0, 1]
    /// rather than failing.
    #[test]
    fn dse_distant_station_returns_valid_value() {
        let input = DuctDseInput {
            zone_type: Ashrae152ZoneType::AtticVented,
            latitude_deg: 0.0,
            longitude_deg: -170.0,
            house_volume_m3: 340.0,
            supply_leakage_frac: 0.10,
            supply_leakage_class: None,
            supply_area_m2: 9.29,
            supply_r_nominal_m2_k_w: 1.76,
            return_leakage_frac: 0.06,
            return_leakage_class: None,
            return_area_m2: 4.65,
            return_r_nominal_m2_k_w: 1.76,
            is_heating: true,
            capacity_w: 14_650.0,
            fan_flow_m3_s: 0.566,
            n_speeds: 1,
            capacity_low_w: None,
            fan_flow_low_m3_s: None,
            is_heat_pump: false,
            burial_depth_m: None,
            soil_conductivity_w_m_k: None,
        };
        let dse = calculate_dse(&input);
        assert!(dse > 0.0 && dse <= 1.0, "DSE out of bounds: {dse}");
    }

    /// Denver (39.7°N, 104.9°W) is a typical continental US location;
    /// the nearest ASHRAE 152 station must be within 200 km so the DSE
    /// uses representative design temperatures.
    #[test]
    fn denver_nearest_station_within_threshold() {
        let station = nearest_station(39.7, -104.9);
        let distance_km = haversine_km(39.7, -104.9, station.latitude_deg, station.longitude_deg);
        assert!(
            distance_km <= 200.0,
            "Denver nearest station is {:.1} km away — exceeds 200 km threshold",
            distance_km
        );
    }

    /// Remote Pacific location (0°N, 170°W) is far from any ASHRAE 152
    /// station, but `design_temperatures_f()` must still return valid
    /// design temperatures rather than failing.
    #[test]
    fn design_temps_distant_station_returns_valid_value() {
        let temps = design_temperatures_f(0.0, -170.0);
        assert!(
            temps.is_some(),
            "design_temperatures_f returned None for distant location"
        );
        let (htg_f, clg_f) = temps.unwrap();
        assert!(htg_f.is_finite(), "heating design temp not finite: {htg_f}");
        assert!(clg_f.is_finite(), "cooling design temp not finite: {clg_f}");
    }

    /// Denver (39.7°N, 104.9°W) is a typical continental US location;
    /// the returned heating and cooling design temperatures must be within
    /// a reasonable range for that climate.
    #[test]
    fn design_temps_denver_within_reasonable_range() {
        let temps = design_temperatures_f(39.7, -104.9);
        assert!(
            temps.is_some(),
            "design_temperatures_f returned None for Denver"
        );
        let (htg_f, clg_f) = temps.unwrap();
        assert!(
            (-40.0..=60.0).contains(&htg_f),
            "Denver heating design temp {htg_f}°F outside -40..60 °F range"
        );
        assert!(
            (60.0..=120.0).contains(&clg_f),
            "Denver cooling design temp {clg_f}°F outside 60..120 °F range"
        );
    }

    // ------------------------------------------------------------------
    // Zone-type default insulation R-value tests
    // ------------------------------------------------------------------

    /// Every `Ashrae152ZoneType` variant must return a finite,
    /// strictly-positive `(supply_r_ip, return_r_ip)` tuple from
    /// `default_insulation_r_ip` for both heating and cooling.
    ///
    /// Once ASHRAE 152-2014 Tables 5-A through 5-D per-zone-type defaults
    /// are filled in, each variant should return distinct values appropriate
    /// for its zone type. The current implementation returns R-1.7 for all
    /// variants because the standard text is not available in the codebase.
    /// The assertion shape here ensures each variant is covered and forward-
    /// compatible with per-zone-type values.
    #[test]
    fn all_zone_types_return_nonzero_insulation_defaults() {
        let variants = [
            Ashrae152ZoneType::AtticVented,
            Ashrae152ZoneType::AtticVentedRadiantBarrier,
            Ashrae152ZoneType::AtticUnvented,
            Ashrae152ZoneType::AtticUnventedRadiantBarrier,
            Ashrae152ZoneType::Garage,
            Ashrae152ZoneType::UnventUninsulatedCrawlspace,
            Ashrae152ZoneType::UnventCrawlspaceInsFloorWall,
            Ashrae152ZoneType::UnventCrawlspaceInsFloor,
            Ashrae152ZoneType::VentUninsulatedCrawlspace,
            Ashrae152ZoneType::VentCrawlspaceInsFloorWall,
            Ashrae152ZoneType::VentCrawlspaceInsFloor,
            Ashrae152ZoneType::UninsulatedBasement,
            Ashrae152ZoneType::BasementInsWalls,
            Ashrae152ZoneType::BasementInsCeiling,
            Ashrae152ZoneType::UnderSlab,
            Ashrae152ZoneType::ExteriorWalls,
        ];
        assert_eq!(
            variants.len(),
            16,
            "precondition: all 16 Ashrae152ZoneType variants must be enumerated"
        );

        for &zone in &variants {
            for &is_heating in &[true, false] {
                let (supply_r, return_r) = zone.default_insulation_r_ip(is_heating);
                assert!(
                    supply_r.is_finite() && supply_r > 0.0,
                    "{zone:?} is_heating={is_heating}: supply R-value must be finite and \
                     positive, got {supply_r}"
                );
                assert!(
                    return_r.is_finite() && return_r > 0.0,
                    "{zone:?} is_heating={is_heating}: return R-value must be finite and \
                     positive, got {return_r}"
                );
            }
        }
    }

    /// When `supply_r_nominal_m2_k_w` is 0.0 (or negative), the resolved
    /// effective R-value must be a finite positive value from the zone-type
    /// default path, and the DSE result must be valid. Verifies that the
    /// zero-nominal-R path produces valid DSE and that different zone types
    /// yield different results (driven by zone temperature differences).
    ///
    /// Note: currently does not verify that the default R-value itself varies
    /// by zone type, since all variants return the same R-1.7 bare-duct
    /// fallback (see Known Limitations in the ticket). Once per-zone-type
    /// constants from ASHRAE 152 Tables 5-A through 5-D are filled in,
    /// this test should be extended to assert that two zone types with
    /// deliberately different table defaults produce different
    /// `default_insulation_r_ip` tuples.
    #[test]
    fn zero_nominal_r_uses_zone_type_default() {
        let input = DuctDseInput {
            zone_type: Ashrae152ZoneType::AtticVented,
            latitude_deg: 39.74,
            longitude_deg: -104.87,
            house_volume_m3: 340.0,
            supply_leakage_frac: 0.10,
            supply_leakage_class: None,
            supply_area_m2: 9.29,
            supply_r_nominal_m2_k_w: 0.0, // ≤ 0 → default path
            return_leakage_frac: 0.06,
            return_leakage_class: None,
            return_area_m2: 4.65,
            return_r_nominal_m2_k_w: 0.0, // ≤ 0 → default path
            is_heating: true,
            capacity_w: 14_650.0,
            fan_flow_m3_s: 0.566,
            n_speeds: 1,
            capacity_low_w: None,
            fan_flow_low_m3_s: None,
            is_heat_pump: false,
            burial_depth_m: None,
            soil_conductivity_w_m_k: None,
        };
        // The DSE must be valid — a zero default R-value would produce NaN.
        let dse = calculate_dse(&input);
        assert!(
            dse > 0.0 && dse <= 1.0,
            "DSE with default insulation must be in (0, 1], got {dse}"
        );

        // Verify that different zone types affect the result even when
        // both use the default R-value path (because zone temperatures differ).
        // BasementInsCeiling has a very different zone temperature formula than
        // AtticVented — the DSE should differ meaningfully.
        let basement_input = DuctDseInput {
            zone_type: Ashrae152ZoneType::BasementInsCeiling,
            ..input
        };
        let dse_basement = calculate_dse(&basement_input);
        assert!(
            dse_basement > 0.0 && dse_basement <= 1.0,
            "basement DSE with default insulation must be in (0, 1], got {dse_basement}"
        );
        // Basement ducts are in a more moderate environment (ground-coupled)
        // than attic ducts, so DSE should be higher (less loss).
        assert!(
            dse_basement > dse,
            "basement DSE ({dse_basement}) should exceed attic DSE ({dse}) — \
             basements are ground-moderated, attics are ambient-coupled"
        );
    }

    /// Regression: a cold-climate home (Minneapolis, ~45°N) with ducts in an
    /// unconditioned vented attic and no explicit insulation must produce
    /// a lower heating DSE than a mild-climate home (Phoenix, ~33°N) with the
    /// same duct configuration. The colder attic drives more conduction loss.
    #[test]
    fn cold_climate_attic_dse_lower_than_mild_climate() {
        let base = DuctDseInput {
            zone_type: Ashrae152ZoneType::AtticVented,
            // filled per test case
            latitude_deg: 0.0,
            longitude_deg: 0.0,
            house_volume_m3: 340.0,
            supply_leakage_frac: 0.10,
            supply_leakage_class: None,
            supply_area_m2: 18.58, // ~200 ft² — larger area amplifies conduction effect
            supply_r_nominal_m2_k_w: 0.0, // default path
            return_leakage_frac: 0.06,
            return_leakage_class: None,
            return_area_m2: 9.29,
            return_r_nominal_m2_k_w: 0.0, // default path
            is_heating: true,
            capacity_w: 14_650.0,
            fan_flow_m3_s: 0.566,
            n_speeds: 1,
            capacity_low_w: None,
            fan_flow_low_m3_s: None,
            is_heat_pump: false,
            burial_depth_m: None,
            soil_conductivity_w_m_k: None,
        };

        let cold = DuctDseInput {
            latitude_deg: 45.0,
            longitude_deg: -93.0,
            ..base
        };
        let mild = DuctDseInput {
            latitude_deg: 33.45,
            longitude_deg: -112.02,
            ..base
        };

        let dse_cold = calculate_dse(&cold);
        let dse_mild = calculate_dse(&mild);

        assert!(
            dse_cold > 0.0 && dse_cold <= 1.0,
            "cold-climate DSE out of bounds: {dse_cold}"
        );
        assert!(
            dse_mild > 0.0 && dse_mild <= 1.0,
            "mild-climate DSE out of bounds: {dse_mild}"
        );
        assert!(
            dse_cold < dse_mild,
            "cold-climate attic DSE ({dse_cold}) should be lower than mild-climate \
             attic DSE ({dse_mild}) — colder attic → more conduction loss"
        );
    }

    // ------------------------------------------------------------------
    // DuctLeakageClass tests
    // ------------------------------------------------------------------

    /// Verify each `DuctLeakageClass` variant returns the correct
    /// CFM/100 ft²-at-25 Pa value from ASHRAE 152-2014 Table 5.
    #[test]
    fn leakage_class_cfm_values_match_ashrae152_table5() {
        assert!(
            (DuctLeakageClass::WellSealed.cfm_per_100ft2_at_25pa() - 2.0).abs() < 1e-9,
            "WellSealed should be 2 CFM/100 ft²"
        );
        assert!(
            (DuctLeakageClass::Sealed.cfm_per_100ft2_at_25pa() - 6.0).abs() < 1e-9,
            "Sealed should be 6 CFM/100 ft²"
        );
        assert!(
            (DuctLeakageClass::Unsealed.cfm_per_100ft2_at_25pa() - 12.0).abs() < 1e-9,
            "Unsealed should be 12 CFM/100 ft²"
        );
    }

    /// Verify `to_leakage_fraction` computes the correct fraction from
    /// leakage class, duct surface area (ft²), and fan airflow (CFM).
    ///
    /// ASHRAE 152-2014 §5: leakage fraction = (LC × A / 100) / Q.
    #[test]
    fn leakage_class_to_fraction_derivation() {
        // 2 CFM/100ft² × 200 ft² / 100 = 4 CFM leakage
        // 4 / 400 CFM fan flow = 0.01
        let frac = DuctLeakageClass::WellSealed.to_leakage_fraction(200.0, 400.0);
        assert!(
            (frac - 0.01).abs() < 1e-9,
            "WellSealed × 200 ft² / 400 CFM should be 0.01, got {frac}"
        );

        // 6 × 100/100 = 6 CFM; 6 / 600 = 0.01
        let frac = DuctLeakageClass::Sealed.to_leakage_fraction(100.0, 600.0);
        assert!(
            (frac - 0.01).abs() < 1e-9,
            "Sealed × 100 ft² / 600 CFM should be 0.01, got {frac}"
        );

        // 12 × 300/100 = 36 CFM; 36 / 1200 = 0.03
        let frac = DuctLeakageClass::Unsealed.to_leakage_fraction(300.0, 1200.0);
        assert!(
            (frac - 0.03).abs() < 1e-9,
            "Unsealed × 300 ft² / 1200 CFM should be 0.03, got {frac}"
        );
    }

    /// `to_leakage_fraction` must return 0.0 when fan flow or duct area is
    /// non-positive (degenerate input).
    #[test]
    fn leakage_class_to_fraction_zero_on_degenerate_input() {
        assert_eq!(
            DuctLeakageClass::Unsealed.to_leakage_fraction(100.0, 0.0),
            0.0,
            "zero fan flow → zero fraction"
        );
        assert_eq!(
            DuctLeakageClass::Unsealed.to_leakage_fraction(0.0, 500.0),
            0.0,
            "zero duct area → zero fraction"
        );
        assert_eq!(
            DuctLeakageClass::Unsealed.to_leakage_fraction(-50.0, 500.0),
            0.0,
            "negative duct area → zero fraction"
        );
    }

    /// `to_leakage_fraction` must clamp to 1.0 for pathological inputs
    /// (tiny fan flow relative to large duct area).
    #[test]
    fn leakage_class_to_fraction_clamps_to_one() {
        // 12 × 10000/100 = 1200 CFM; 1200 / 1 = 1200 → clamped to 1.0
        let frac = DuctLeakageClass::Unsealed.to_leakage_fraction(10000.0, 1.0);
        assert!((frac - 1.0).abs() < 1e-9, "should clamp to 1.0, got {frac}");
    }

    /// Verify that every `Ashrae152ZoneType` variant has a default leakage
    /// class and that the returned tuple is non-None for both heating and
    /// cooling seasons.
    #[test]
    fn all_zone_types_have_default_leakage_class() {
        let variants = [
            Ashrae152ZoneType::AtticVented,
            Ashrae152ZoneType::AtticVentedRadiantBarrier,
            Ashrae152ZoneType::AtticUnvented,
            Ashrae152ZoneType::AtticUnventedRadiantBarrier,
            Ashrae152ZoneType::Garage,
            Ashrae152ZoneType::UnventUninsulatedCrawlspace,
            Ashrae152ZoneType::UnventCrawlspaceInsFloorWall,
            Ashrae152ZoneType::UnventCrawlspaceInsFloor,
            Ashrae152ZoneType::VentUninsulatedCrawlspace,
            Ashrae152ZoneType::VentCrawlspaceInsFloorWall,
            Ashrae152ZoneType::VentCrawlspaceInsFloor,
            Ashrae152ZoneType::UninsulatedBasement,
            Ashrae152ZoneType::BasementInsWalls,
            Ashrae152ZoneType::BasementInsCeiling,
            Ashrae152ZoneType::UnderSlab,
            Ashrae152ZoneType::ExteriorWalls,
        ];
        assert_eq!(
            variants.len(),
            16,
            "precondition: all 16 Ashrae152ZoneType variants must be enumerated"
        );

        for &zone in &variants {
            for &is_heating in &[true, false] {
                let (supply_class, return_class) = zone.default_leakage_class(is_heating);
                assert!(
                    supply_class.cfm_per_100ft2_at_25pa() > 0.0,
                    "{zone:?} is_heating={is_heating}: supply leakage class must have \
                     positive CFM/100ft² value"
                );
                assert!(
                    return_class.cfm_per_100ft2_at_25pa() > 0.0,
                    "{zone:?} is_heating={is_heating}: return leakage class must have \
                     positive CFM/100ft² value"
                );
            }
        }
    }

    /// Attic zone types default to `Unsealed` (12 CFM/100 ft²),
    /// basement types default to `WellSealed` (2 CFM/100 ft²).
    #[test]
    fn zone_type_leakage_class_defaults_match_expected_exposure() {
        // Attic ducts → Unsealed (most exposed)
        let (s, r) = Ashrae152ZoneType::AtticVented.default_leakage_class(true);
        assert_eq!(s, DuctLeakageClass::Unsealed);
        assert_eq!(r, DuctLeakageClass::Unsealed);

        // Basement ducts → WellSealed (most protected)
        let (s, r) = Ashrae152ZoneType::UninsulatedBasement.default_leakage_class(true);
        assert_eq!(s, DuctLeakageClass::WellSealed);
        assert_eq!(r, DuctLeakageClass::WellSealed);

        // Crawlspace ducts → Sealed (intermediate)
        let (s, r) = Ashrae152ZoneType::VentUninsulatedCrawlspace.default_leakage_class(true);
        assert_eq!(s, DuctLeakageClass::Sealed);
        assert_eq!(r, DuctLeakageClass::Sealed);
    }

    // ------------------------------------------------------------------
    // Seasonal multiplier tests
    // ------------------------------------------------------------------

    /// Every `Ashrae152ZoneType` variant must return a seasonal multiplier
    /// in (0.0, 1.0] for both heating and cooling.
    #[test]
    fn all_zone_types_return_plausible_seasonal_multiplier() {
        let variants = [
            Ashrae152ZoneType::AtticVented,
            Ashrae152ZoneType::AtticVentedRadiantBarrier,
            Ashrae152ZoneType::AtticUnvented,
            Ashrae152ZoneType::AtticUnventedRadiantBarrier,
            Ashrae152ZoneType::Garage,
            Ashrae152ZoneType::UnventUninsulatedCrawlspace,
            Ashrae152ZoneType::UnventCrawlspaceInsFloorWall,
            Ashrae152ZoneType::UnventCrawlspaceInsFloor,
            Ashrae152ZoneType::VentUninsulatedCrawlspace,
            Ashrae152ZoneType::VentCrawlspaceInsFloorWall,
            Ashrae152ZoneType::VentCrawlspaceInsFloor,
            Ashrae152ZoneType::UninsulatedBasement,
            Ashrae152ZoneType::BasementInsWalls,
            Ashrae152ZoneType::BasementInsCeiling,
            Ashrae152ZoneType::UnderSlab,
            Ashrae152ZoneType::ExteriorWalls,
        ];
        assert_eq!(
            variants.len(),
            16,
            "precondition: all 16 Ashrae152ZoneType variants must be enumerated"
        );

        for &zone in &variants {
            for &is_heating in &[true, false] {
                for &is_heat_pump in &[true, false] {
                    let mult = zone.seasonal_multiplier(is_heating, is_heat_pump);
                    assert!(
                        mult > 0.0 && mult <= 1.0,
                        "{zone:?} is_heating={is_heating} is_heat_pump={is_heat_pump}: \
                         seasonal multiplier must be in (0.0, 1.0], got {mult}"
                    );
                }
            }
        }
    }

    /// ASHRAE 152-2014 Table 5-A prescribes a seasonal delivery effectiveness
    /// multiplier for ducts in unconditioned vented attics during heating
    /// season that is less than 1.0 (typically 0.80–0.95).
    ///
    /// This test currently expects failure because the standard text is not
    /// available in the codebase — all variants return 1.0 (identity) as a
    /// placeholder. Remove `#[should_panic]` once the ASHRAE 152-2014
    /// table values are filled in.
    #[test]
    #[should_panic(
        expected = "attic vented heating multiplier must be <1.0 per ASHRAE 152-2014 Table 5-A"
    )]
    fn attic_vented_heating_multiplier_less_than_one() {
        let mult = Ashrae152ZoneType::AtticVented.seasonal_multiplier(true, false);
        assert!(
            mult < 1.0,
            "attic vented heating multiplier must be <1.0 per ASHRAE 152-2014 Table 5-A; \
             current value is {mult} — the standard table values have not been filled in"
        );
    }

    /// ASHRAE 152-2014 Table 5-C prescribes a seasonal delivery effectiveness
    /// multiplier for ducts in unconditioned vented attics during cooling
    /// season that is less than 1.0 (typically 0.80–0.95).
    ///
    /// This test currently expects failure because the standard text is not
    /// available in the codebase — all variants return 1.0 (identity) as a
    /// placeholder. Remove `#[should_panic]` once the ASHRAE 152-2014
    /// table values are filled in.
    #[test]
    #[should_panic(
        expected = "attic vented cooling multiplier must be <1.0 per ASHRAE 152-2014 Table 5-C"
    )]
    fn attic_vented_cooling_multiplier_less_than_one() {
        let mult = Ashrae152ZoneType::AtticVented.seasonal_multiplier(false, false);
        assert!(
            mult < 1.0,
            "attic vented cooling multiplier must be <1.0 per ASHRAE 152-2014 Table 5-C; \
             current value is {mult} — the standard table values have not been filled in"
        );
    }

    /// Verify that the seasonal multiplier is applied within `calculate_dse`
    /// and that the DSE result with multiplier matches the DSE without
    /// multiplier when the multiplier is 1.0 (identity). This confirms the
    /// multiplier path is exercised end-to-end.
    #[test]
    fn dse_with_identity_seasonal_multiplier_preserves_result() {
        let input = DuctDseInput {
            zone_type: Ashrae152ZoneType::AtticVented,
            latitude_deg: 39.74,
            longitude_deg: -104.87,
            house_volume_m3: 340.0,
            supply_leakage_frac: 0.10,
            supply_leakage_class: None,
            supply_area_m2: 9.29,
            supply_r_nominal_m2_k_w: 1.76,
            return_leakage_frac: 0.06,
            return_leakage_class: None,
            return_area_m2: 4.65,
            return_r_nominal_m2_k_w: 1.76,
            is_heating: true,
            capacity_w: 14_650.0,
            fan_flow_m3_s: 0.566,
            n_speeds: 1,
            capacity_low_w: None,
            fan_flow_low_m3_s: None,
            is_heat_pump: false,
            burial_depth_m: None,
            soil_conductivity_w_m_k: None,
        };
        let dse = calculate_dse(&input);
        assert!(
            dse > 0.0 && dse <= 1.0,
            "DSE with seasonal multiplier path must be in (0, 1], got {dse}"
        );

        // Verify the multiplier is 1.0 for this zone type and season
        let mult = input
            .zone_type
            .seasonal_multiplier(input.is_heating, input.is_heat_pump);
        assert!(
            (mult - 1.0).abs() < 1e-9,
            "seasonal multiplier must be 1.0 (identity); value will change when \
             ASHRAE 152-2014 table values are filled in"
        );

        // Verify DSE is in the typical range for this configuration
        assert!(dse > 0.6, "DSE unexpectedly low: {dse}");
    }

    /// Regression: compute DSE with `WellSealed` leakage class vs.
    /// `Unsealed` leakage class for an attic zone type and confirm
    /// the DSE changes in the expected direction (higher DSE for
    /// well-sealed = less leakage = less loss).
    #[test]
    fn dse_well_sealed_exceeds_unsealed_for_attic() {
        let base = DuctDseInput {
            zone_type: Ashrae152ZoneType::AtticVented,
            latitude_deg: 39.74,
            longitude_deg: -104.87,
            house_volume_m3: 340.0,
            supply_leakage_frac: 0.0,
            supply_leakage_class: None,
            supply_area_m2: 9.29,
            supply_r_nominal_m2_k_w: 1.76,
            return_leakage_frac: 0.0,
            return_leakage_class: None,
            return_area_m2: 4.65,
            return_r_nominal_m2_k_w: 1.76,
            is_heating: true,
            capacity_w: 14_650.0,
            fan_flow_m3_s: 0.566,
            n_speeds: 1,
            capacity_low_w: None,
            fan_flow_low_m3_s: None,
            is_heat_pump: false,
            burial_depth_m: None,
            soil_conductivity_w_m_k: None,
        };

        let well_sealed = DuctDseInput {
            supply_leakage_class: Some(DuctLeakageClass::WellSealed),
            return_leakage_class: Some(DuctLeakageClass::WellSealed),
            ..base
        };
        let unsealed = DuctDseInput {
            supply_leakage_class: Some(DuctLeakageClass::Unsealed),
            return_leakage_class: Some(DuctLeakageClass::Unsealed),
            ..base
        };
        let none = DuctDseInput {
            supply_leakage_frac: 0.10,
            return_leakage_frac: 0.06,
            ..base
        };

        let dse_ws = calculate_dse(&well_sealed);
        let dse_us = calculate_dse(&unsealed);
        let dse_none = calculate_dse(&none);

        assert!(
            dse_ws > 0.0 && dse_ws <= 1.0,
            "WellSealed DSE out of bounds: {dse_ws}"
        );
        assert!(
            dse_us > 0.0 && dse_us <= 1.0,
            "Unsealed DSE out of bounds: {dse_us}"
        );
        // Well-sealed ducts lose less → higher DSE
        assert!(
            dse_ws > dse_us,
            "WellSealed DSE ({dse_ws}) should exceed Unsealed DSE ({dse_us})"
        );

        // The raw-fraction fallback path must also produce a valid DSE
        assert!(
            dse_none > 0.0 && dse_none <= 1.0,
            "raw-fraction fallback DSE out of bounds: {dse_none}"
        );
    }

    // ------------------------------------------------------------------
    // UnderSlab soil temperature correction tests
    // ------------------------------------------------------------------

    /// At 0.5 m burial depth with soil conductivity = 1.5 W/(m·K),
    /// the soil temperature beneath a conditioned slab should lie between
    /// the conditioned-space temperature and deep ground temperature.
    /// For a slab at 68°F (heating reference) with gnd = 50°F, the soil
    /// temp at depth should be warmer than gnd but cooler than 68°F.
    /// For a slab at 78°F (cooling reference), the soil should be warmer
    /// than gnd but cooler than 78°F.
    #[test]
    fn under_slab_soil_temp_with_burial_correction() {
        let depth = 0.5;
        let k = 1.5;
        let gnd = 50.0;

        // Heating: conditioned space is 68°F, soil temp should be between gnd and 68
        let soil_heating = soil_temp_at_burial_depth_f(68.0, gnd, depth, k);
        assert!(
            soil_heating > gnd && soil_heating < 68.0,
            "heating: expected {gnd} < soil < 68°F, got {soil_heating}"
        );

        // Cooling: conditioned space is 78°F, soil temp should be between gnd and 78
        let soil_cooling = soil_temp_at_burial_depth_f(78.0, gnd, depth, k);
        assert!(
            soil_cooling > gnd && soil_cooling < 78.0,
            "cooling: expected {gnd} < soil < 78°F, got {soil_cooling}"
        );

        // Deeper burial → closer to gnd
        let soil_deep = soil_temp_at_burial_depth_f(68.0, gnd, 3.0, k);
        assert!(
            (soil_deep - gnd).abs() < (soil_heating - gnd).abs(),
            "deeper burial should be closer to gnd: shallow delta = {}, deep delta = {}",
            soil_heating - gnd,
            soil_deep - gnd
        );
    }

    /// Degenerate inputs (zero/negative burial depth or conductivity)
    /// return the ground temperature unchanged.
    #[test]
    fn under_slab_soil_temp_degenerate_inputs_return_gnd() {
        let gnd = 60.0;
        let result = soil_temp_at_burial_depth_f(68.0, gnd, 0.0, 1.5);
        assert!(
            (result - gnd).abs() < 1e-9,
            "zero burial depth should return gnd, got {result}"
        );
        let result = soil_temp_at_burial_depth_f(68.0, gnd, 0.5, 0.0);
        assert!(
            (result - gnd).abs() < 1e-9,
            "zero conductivity should return gnd, got {result}"
        );
    }

    /// When burial params are None, UnderSlab zone temps must fall back
    /// to gnd for all four temperatures, matching the pre-correction behavior.
    #[test]
    fn under_slab_none_burial_params_falls_back_to_gnd() {
        let station = StationTemps {
            h_des: 10.0,
            h_seas: 40.0,
            c_des: 90.0,
            c_seas: 75.0,
            gnd: 50.0,
        };

        let result = zone_temps(Ashrae152ZoneType::UnderSlab, &station, None, None);
        assert!(
            (result.0 - station.gnd).abs() < 1e-9,
            "heating design temp should be gnd without burial params, got {}",
            result.0
        );
        assert!(
            (result.1 - station.gnd).abs() < 1e-9,
            "heating seasonal temp should be gnd without burial params, got {}",
            result.1
        );
        assert!(
            (result.2 - station.gnd).abs() < 1e-9,
            "cooling design temp should be gnd without burial params, got {}",
            result.2
        );
        assert!(
            (result.3 - station.gnd).abs() < 1e-9,
            "cooling seasonal temp should be gnd without burial params, got {}",
            result.3
        );
        assert!(
            (result.4 - 0.2).abs() < 1e-9,
            "supply regain should remain 0.2"
        );
        assert!(
            (result.5 - 0.2).abs() < 1e-9,
            "return regain should remain 0.2"
        );
    }

    /// With burial depth = 0.5 m, soil conductivity = 1.5 W/(m·K),
    /// the UnderSlab zone temps differ from gnd in the expected direction:
    /// approaching the conditioned-space temperature (68°F heating,
    /// 78°F cooling) at shallow depth, and decaying toward gnd at depth.
    #[test]
    fn under_slab_with_burial_correction_modifies_zone_temps() {
        let station = StationTemps {
            h_des: 10.0,
            h_seas: 40.0,
            c_des: 90.0,
            c_seas: 75.0,
            gnd: 50.0,
        };
        let depth = Some(0.5);
        let k = Some(1.5);

        let result = zone_temps(Ashrae152ZoneType::UnderSlab, &station, depth, k);

        // Heating: soil temp should be between gnd and 68°F (conditioned space)
        assert!(
            result.0 > station.gnd && result.0 < 68.0,
            "heating design: expected gnd < soil < 68°F, got {}",
            result.0
        );
        assert!(
            result.1 > station.gnd && result.1 < 68.0,
            "heating seasonal: expected gnd < soil < 68°F, got {}",
            result.1
        );
        // Cooling: soil temp should be between gnd and 78°F (conditioned space)
        assert!(
            result.2 > station.gnd && result.2 < 78.0,
            "cooling design: expected gnd < soil < 78°F, got {}",
            result.2
        );
        assert!(
            result.3 > station.gnd && result.3 < 78.0,
            "cooling seasonal: expected gnd < soil < 78°F, got {}",
            result.3
        );
    }

    /// DSE for an UnderSlab duct with and without burial correction
    /// must produce valid results in (0, 1] for both heating and cooling.
    #[test]
    fn under_slab_dse_valid_both_seasons() {
        let base = DuctDseInput {
            zone_type: Ashrae152ZoneType::UnderSlab,
            latitude_deg: 39.74,
            longitude_deg: -104.87,
            house_volume_m3: 340.0,
            supply_leakage_frac: 0.1,
            supply_leakage_class: None,
            supply_area_m2: 9.29,
            supply_r_nominal_m2_k_w: 1.76,
            return_leakage_frac: 0.06,
            return_leakage_class: None,
            return_area_m2: 4.65,
            return_r_nominal_m2_k_w: 1.76,
            is_heating: true,
            capacity_w: 14_650.0,
            fan_flow_m3_s: 0.566,
            n_speeds: 1,
            capacity_low_w: None,
            fan_flow_low_m3_s: None,
            is_heat_pump: false,
            burial_depth_m: None,
            soil_conductivity_w_m_k: None,
        };

        let uncorrected_dse = calculate_dse(&base);
        assert!(
            uncorrected_dse > 0.0 && uncorrected_dse <= 1.0,
            "uncorrected under-slab DSE out of bounds: {uncorrected_dse}"
        );

        let corrected = DuctDseInput {
            burial_depth_m: Some(0.5),
            soil_conductivity_w_m_k: Some(1.5),
            ..base
        };
        let corrected_dse = calculate_dse(&corrected);
        assert!(
            corrected_dse > 0.0 && corrected_dse <= 1.0,
            "corrected under-slab DSE out of bounds: {corrected_dse}"
        );

        // Cooling season must also produce valid DSE
        let cooling = DuctDseInput {
            is_heating: false,
            latitude_deg: 33.45,
            longitude_deg: -112.02,
            capacity_w: 10_550.0,
            fan_flow_m3_s: 0.472,
            ..corrected
        };
        let cooling_dse = calculate_dse(&cooling);
        assert!(
            cooling_dse > 0.0 && cooling_dse <= 1.0,
            "corrected cooling under-slab DSE out of bounds: {cooling_dse}"
        );
    }
}
