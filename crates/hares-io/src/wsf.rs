//! The ASHRAE 62.2 weather and shielding factor (WSF) of a weather file, as
//! OpenStudio-HPXML v1.12.0 computes it (`weather.rb:193-250`,
//! `calc_ashrae_622_wsf`): the station's tabulated value when its WMO number
//! is in `data/ashrae622_wsf.csv`, else the LBNL-5795E "Infiltration as
//! Ventilation: Weather-Induced Dilution" calculation from the hourly
//! dry-bulb and wind speed; rounded to two places either way.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::weather::{WeatherError, WeatherTimeSeries};

/// OS-HPXML v1.12.0 `HPXMLtoOpenStudio/resources/data/ashrae622_wsf.csv`,
/// unchanged; `scripts/check_os_hpxml_tables.sh` regenerates it and the
/// licence notice is `data/OS-HPXML-LICENSE.md`.
const ASHRAE622_WSF_CSV: &str = include_str!("../data/ashrae622_wsf.csv");

fn tabulated_wsf() -> &'static HashMap<&'static str, f64> {
    static TABLE: OnceLock<HashMap<&'static str, f64>> = OnceLock::new();
    TABLE.get_or_init(|| {
        ASHRAE622_WSF_CSV
            .lines()
            .skip(1)
            .map(|line| {
                let (wmo, wsf) = line
                    .split_once(',')
                    .expect("ashrae622_wsf.csv rows are `station_wmo,wsf`");
                (wmo, wsf.parse().expect("ashrae622_wsf.csv wsf is a number"))
            })
            .collect()
    })
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// The weather file's WSF: the tabulated value for its station, else the
/// LBNL-5795E calculation over its hourly records.
pub fn ashrae_622_wsf(weather: &WeatherTimeSeries) -> Result<f64, WeatherError> {
    if let Some(wsf) = weather
        .meta
        .station_wmo
        .as_deref()
        .and_then(|wmo| tabulated_wsf().get(wmo.trim()))
    {
        return Ok(round2(*wsf));
    }
    let step_secs = weather.meta.source_step_secs;
    if step_secs == 0 || 3600 % step_secs != 0 {
        return Err(WeatherError::Validation(format!(
            "the ASHRAE 62.2 WSF needs hourly records; a {step_secs} s step does not divide an hour"
        )));
    }
    let stride = (3600 / step_secs) as usize;
    let hourly = weather
        .dry_bulb_c
        .iter()
        .zip(&weather.wind_speed_m_s)
        .step_by(stride);
    Ok(round2(lbnl_5795e_wsf(hourly)))
}

/// LBNL-5795E WSF from hourly (dry-bulb °C, wind speed m/s) pairs, with
/// OS-HPXML's reference house (weather.rb:215-250).
fn lbnl_5795e_wsf<'a>(hours: impl Iterator<Item = (&'a f64, &'a f64)>) -> f64 {
    let c_d = 1.0; // ELA discharge coefficient (at 4 Pa)
    let t_indoor_c = 22.0;
    let n = 0.67; // pressure exponent
    let s = 0.7; // shelter class 4, one story with flue, enhanced model
    let delta_p_pa: f64 = 4.0;
    let u_min_m_s = 1.0;
    let ela_m2 = 0.074;
    let cfa_m2 = 185.0;
    let height_m = 2.5;
    let g = 0.48; // wind speed multiplier, one story, enhanced model
    let c_s = 0.069; // stack coefficient ((Pa/K)^n)
    let c_w = 0.142; // wind coefficient ((Pa s^2/m^2)^n)
    let rho_kg_m3 = 1.2;

    let c = c_d * ela_m2 * (2.0_f64 / rho_kg_m3).sqrt() * delta_p_pa.powf(0.5 - n);
    let mut tau_sum = 0.0;
    let mut count = 0usize;
    let mut previous_tau = 0.0;
    for (dry_bulb_c, wind_m_s) in hours {
        let q_s = c * c_s * (t_indoor_c - dry_bulb_c).abs().powf(n);
        let q_w = c * c_w * (s * g * wind_m_s.max(u_min_m_s)).powf(2.0 * n);
        let q_tot = (q_s * q_s + q_w * q_w).sqrt();
        let ach = 3600.0 * q_tot / (height_m * cfa_m2);
        let tau = (1.0 - (-ach).exp()) / ach + previous_tau * (-ach).exp();
        tau_sum += tau;
        count += 1;
        previous_tau = tau;
    }
    let tau = tau_sum / count as f64;
    (cfa_m2 / ela_m2) / (1000.0 * tau)
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    use super::{ASHRAE622_WSF_CSV, lbnl_5795e_wsf, tabulated_wsf};

    /// The committed table is OS-HPXML's file, unchanged.
    #[test]
    fn station_table_is_the_upstream_file() {
        let digest = Sha256::digest(ASHRAE622_WSF_CSV.as_bytes());
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "94e64f002695bd24a4ee1a10ad0425726fe35c7b822aabbc62f6d1a2bba9edbb"
        );
        assert_eq!(tabulated_wsf().get("725650"), Some(&0.59));
        assert_eq!(tabulated_wsf().get("722020"), Some(&0.41));
    }

    /// The LBNL-5795E calculation over Denver's TMY3 year reproduces the
    /// station's tabulated 0.59.
    #[test]
    fn calculated_wsf_reproduces_a_tabulated_station() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../data/examples/USA_CO_Denver.Intl.AP.725650_TMY3.epw");
        let weather = crate::epw::parse_epw(path).expect("Denver EPW");
        let calculated = lbnl_5795e_wsf(weather.dry_bulb_c.iter().zip(&weather.wind_speed_m_s));
        assert!(
            ((calculated * 100.0).round() / 100.0 - 0.59).abs() < 1e-9,
            "calculated {calculated}"
        );
    }

    /// Calm, mild weather barely ventilates the reference house, so its WSF
    /// is low; windy, cold weather raises it.
    #[test]
    fn calculated_wsf_rises_with_wind_and_temperature_difference() {
        let calm: Vec<(f64, f64)> = vec![(20.0, 0.0); 8760];
        let windy: Vec<(f64, f64)> = vec![(-10.0, 8.0); 8760];
        let wsf = |hours: &[(f64, f64)]| lbnl_5795e_wsf(hours.iter().map(|(t, w)| (t, w)));
        assert!(
            wsf(&calm) < wsf(&windy),
            "{} vs {}",
            wsf(&calm),
            wsf(&windy)
        );
        assert!(wsf(&calm) > 0.0);
    }
}
