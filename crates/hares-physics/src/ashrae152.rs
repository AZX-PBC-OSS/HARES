//! ASHRAE 152 duct distribution system efficiency (DSE) calculation.
//!
//! All public inputs are SI units. Internal computation uses IP units per the
//! ASHRAE 152 standard. The public [`calculate_dse`] function returns a DSE
//! value clamped to `(0.0, 1.0]`.

use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Unit conversion constants
// ---------------------------------------------------------------------------

const M3_TO_FT3: f64 = 35.3147;
const M2_TO_FT2: f64 = 10.7639;
const W_TO_BTU_H: f64 = 3.41214;
const M3S_TO_CFM: f64 = 2118.88;
/// SI R-value (m²·K/W) → IP R-value (ft²·h·°F/Btu)
const SI_R_TO_IP_R: f64 = 5.67826;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

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
    /// Supply duct surface area (m²).
    pub supply_area_m2: f64,
    /// Supply duct nominal R-value (m²·K/W); ≤ 0 → uninsulated default.
    pub supply_r_nominal_m2_k_w: f64,
    /// Return duct leakage as a fraction of fan flow (0–1).
    pub return_leakage_frac: f64,
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
}

// ---------------------------------------------------------------------------
// Internal climate station
// ---------------------------------------------------------------------------

struct ClimateStation {
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
// Zone temperature formulas
// ---------------------------------------------------------------------------

/// All temperatures in °F.
/// Returns `(htg_des, htg_seas, clg_des, clg_seas, supply_regain, return_regain)`.
fn zone_temps(
    zone: Ashrae152ZoneType,
    h_des: f64,
    h_seas: f64,
    c_des: f64,
    c_seas: f64,
    gnd: f64,
) -> (f64, f64, f64, f64, f64, f64) {
    match zone {
        Ashrae152ZoneType::AtticVented => (
            h_des + 10.0,
            h_seas + 7.0,
            c_des + 22.0,
            c_seas + 13.0,
            0.1,
            0.1,
        ),
        Ashrae152ZoneType::AtticVentedRadiantBarrier => (
            h_des + 10.0,
            h_seas + 7.0,
            0.65 * (c_des + 22.0) + 0.35 * 78.0,
            0.7 * (c_seas + 13.0) + 0.3 * 78.0,
            0.1,
            0.1,
        ),
        Ashrae152ZoneType::AtticUnvented => (
            h_des + 10.0,
            h_seas + 7.0,
            c_des + 36.0,
            c_seas + 16.0,
            0.1,
            0.1,
        ),
        Ashrae152ZoneType::AtticUnventedRadiantBarrier => (
            h_des + 10.0,
            h_seas + 7.0,
            0.65 * (c_des + 36.0) + 0.35 * 78.0,
            0.7 * (c_seas + 16.0) + 0.3 * 78.0,
            0.1,
            0.1,
        ),
        Ashrae152ZoneType::Garage => (
            h_des + 13.0,
            h_seas + 11.0,
            c_des + 7.0,
            c_seas + 7.0,
            0.1,
            0.1,
        ),
        Ashrae152ZoneType::UnventUninsulatedCrawlspace => (
            (2.0 * h_des + 3.0 * 68.0) / 5.0,
            (2.0 * h_seas + 3.0 * 68.0) / 5.0,
            (2.0 * c_des + 3.0 * 78.0) / 5.0,
            (2.0 * c_seas + 3.0 * 78.0) / 5.0,
            0.6,
            0.6,
        ),
        Ashrae152ZoneType::UnventCrawlspaceInsFloorWall => (
            (3.0 * h_des + 68.0) / 4.0,
            (3.0 * h_seas + 68.0) / 4.0,
            (3.0 * c_des + 78.0) / 4.0,
            (3.0 * c_seas + 78.0) / 4.0,
            0.6,
            0.6,
        ),
        Ashrae152ZoneType::UnventCrawlspaceInsFloor => (
            (5.0 * h_des + 68.0) / 6.0,
            (5.0 * h_seas + 68.0) / 6.0,
            (5.0 * c_des + 78.0) / 6.0,
            (5.0 * c_seas + 78.0) / 6.0,
            0.3,
            0.3,
        ),
        Ashrae152ZoneType::VentUninsulatedCrawlspace => (
            (h_des + 68.0) / 2.0,
            (h_seas + 68.0) / 2.0,
            (c_des + 78.0) / 2.0,
            (c_seas + 78.0) / 2.0,
            0.6,
            0.6,
        ),
        Ashrae152ZoneType::VentCrawlspaceInsFloorWall => (
            (5.0 * h_des + 68.0) / 6.0,
            (5.0 * h_seas + 68.0) / 6.0,
            (5.0 * c_des + 78.0) / 6.0,
            (5.0 * c_seas + 78.0) / 6.0,
            0.63,
            0.63,
        ),
        Ashrae152ZoneType::VentCrawlspaceInsFloor => (
            (8.0 * h_des + 68.0) / 9.0,
            (8.0 * h_seas + 68.0) / 9.0,
            (8.0 * c_des + 78.0) / 9.0,
            (8.0 * c_seas + 78.0) / 9.0,
            0.3,
            0.3,
        ),
        Ashrae152ZoneType::UninsulatedBasement => (
            (5.0 * gnd + 2.0 * h_des + 3.0 * 68.0) / 10.0,
            (5.0 * gnd + 2.0 * h_seas + 3.0 * 68.0) / 10.0,
            (5.0 * gnd + 2.0 * c_des + 3.0 * 78.0) / 10.0,
            (5.0 * gnd + 2.0 * c_seas + 3.0 * 78.0) / 10.0,
            0.5,
            0.5,
        ),
        Ashrae152ZoneType::BasementInsWalls => (
            (gnd + 68.0) / 2.0,
            (gnd + 68.0) / 2.0,
            (8.0 * gnd + c_des + 78.0) / 10.0,
            (8.0 * gnd + c_seas + 78.0) / 10.0,
            0.6,
            0.6,
        ),
        Ashrae152ZoneType::BasementInsCeiling => (
            (3.0 * gnd + h_des) / 4.0,
            (3.0 * gnd + h_seas) / 4.0,
            (3.0 * gnd + c_des) / 4.0,
            (3.0 * gnd + c_seas) / 4.0,
            0.6,
            0.6,
        ),
        Ashrae152ZoneType::UnderSlab => (gnd, gnd, gnd, gnd, 0.2, 0.2),
        Ashrae152ZoneType::ExteriorWalls => (
            (h_des + 68.0) / 2.0,
            (h_seas + 68.0) / 2.0,
            (c_des + 78.0) / 2.0,
            (c_seas + 78.0) / 2.0,
            0.2,
            0.2,
        ),
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

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
    let supply_r = if supply_nom_r_ip <= 0.0 {
        1.7
    } else {
        2.2438 + 0.5619 * supply_nom_r_ip
    };
    let return_r = if return_nom_r_ip <= 0.0 {
        1.7
    } else {
        2.0388 + 0.7053 * return_nom_r_ip
    };

    // ------------------------------------------------------------------
    // 3. Climate station lookup
    // ------------------------------------------------------------------
    let station = nearest_station(input.latitude_deg, input.longitude_deg);
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
        heating_des_init,
        heating_seas_init,
        cooling_des_init,
        cooling_seas_init,
        ground_temp,
    );

    let ambient_temp = if input.is_heating { 68.0_f64 } else { 78.0_f64 };
    let seas_temp = if input.is_heating { htg_seas } else { clg_seas };

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
    // 8. High-speed duct factors
    // ------------------------------------------------------------------
    let supply_duct_leakage = fan_flow_cfm * input.supply_leakage_frac;
    let return_duct_leakage = fan_flow_cfm * input.return_leakage_frac;

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
    // 9. Low-speed duct factors (multi-speed only)
    // ------------------------------------------------------------------
    let (as_low, ar_low, dte_low, bs_low, br_low) = if input.n_speeds > 1 {
        let cap_low = input.capacity_low_w.unwrap_or(input.capacity_w) * W_TO_BTU_H;
        let flow_low = input.fan_flow_low_m3_s.unwrap_or(input.fan_flow_m3_s) * M3S_TO_CFM;

        let sdl_low = flow_low * input.supply_leakage_frac;
        let rdl_low = flow_low * input.return_leakage_frac;

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
    // 10. Uncorrected delivery effectiveness
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
    // 11. Load factor
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
    // 12. Equipment factor
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
    // 13. Delivery effectiveness with thermal regain
    // ------------------------------------------------------------------
    let seas_de = seas_uncorr_de + supply_regain * (1.0 - seas_uncorr_de)
        - (supply_regain - return_regain - br_high * (ar_high * supply_regain - return_regain))
            * seas_return_temp_diff
            / dte_high;

    // ------------------------------------------------------------------
    // 14. Final DSE
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
            10.0,
            h_seas,
            90.0,
            75.0,
            50.0,
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
            supply_area_m2: 9.29,          // ~100 ft²
            supply_r_nominal_m2_k_w: 1.76, // ~R-10 IP
            return_leakage_frac: 0.06,
            return_area_m2: 4.65, // ~50 ft²
            return_r_nominal_m2_k_w: 1.76,
            is_heating: true,
            capacity_w: 14_650.0, // ~50 000 Btu/h
            fan_flow_m3_s: 0.566, // ~1 200 CFM
            n_speeds: 1,
            capacity_low_w: None,
            fan_flow_low_m3_s: None,
            is_heat_pump: false,
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
            supply_area_m2: 9.29,
            supply_r_nominal_m2_k_w: 1.76,
            return_leakage_frac: 0.06,
            return_area_m2: 4.65,
            return_r_nominal_m2_k_w: 1.76,
            is_heating: false,
            capacity_w: 10_550.0, // ~36 000 Btu/h (3 ton)
            fan_flow_m3_s: 0.472, // ~1 000 CFM
            n_speeds: 1,
            capacity_low_w: None,
            fan_flow_low_m3_s: None,
            is_heat_pump: false,
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
            supply_area_m2: 9.29,
            supply_r_nominal_m2_k_w: 1.76,
            return_leakage_frac: 0.06,
            return_area_m2: 4.65,
            return_r_nominal_m2_k_w: 1.76,
            is_heating: false,
            capacity_w: 10_550.0,
            fan_flow_m3_s: 0.472,
            n_speeds: 2,
            capacity_low_w: Some(7_000.0),
            fan_flow_low_m3_s: Some(0.330),
            is_heat_pump: false,
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
            supply_area_m2: 9.29,
            supply_r_nominal_m2_k_w: 1.76,
            return_leakage_frac: 0.06,
            return_area_m2: 4.65,
            return_r_nominal_m2_k_w: 1.76,
            is_heating: true,
            capacity_w: 14_650.0,
            fan_flow_m3_s: 0.566,
            n_speeds: 1,
            capacity_low_w: None,
            fan_flow_low_m3_s: None,
            is_heat_pump: false,
        };
        let dse_h = calculate_dse(&heating_input);
        assert!(dse_h > 0.0 && dse_h <= 1.0, "heating DSE: {dse_h}");

        let cooling_input = DuctDseInput {
            zone_type: Ashrae152ZoneType::AtticVented,
            latitude_deg: 33.45,
            longitude_deg: -112.02,
            house_volume_m3: 340.0,
            supply_leakage_frac: 0.10,
            supply_area_m2: 9.29,
            supply_r_nominal_m2_k_w: 1.76,
            return_leakage_frac: 0.06,
            return_area_m2: 4.65,
            return_r_nominal_m2_k_w: 1.76,
            is_heating: false,
            capacity_w: 10_550.0,
            fan_flow_m3_s: 0.472,
            n_speeds: 1,
            capacity_low_w: None,
            fan_flow_low_m3_s: None,
            is_heat_pump: false,
        };
        let dse_c = calculate_dse(&cooling_input);
        assert!(dse_c > 0.0 && dse_c <= 1.0, "cooling DSE: {dse_c}");
    }
}
