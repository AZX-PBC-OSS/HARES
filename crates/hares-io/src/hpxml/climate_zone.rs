//! The IECC climate zone of a home whose HPXML gives none, derived from its
//! weather station as OpenStudio-HPXML does.

use std::collections::HashMap;
use std::sync::OnceLock;

use hares_types::Warning;

use super::Building;

/// Each weather station's IECC zone, by WMO number: the zone of the first
/// row naming the station in OpenStudio-HPXML v1.12.0
/// `HPXMLtoOpenStudio/resources/data/zipcode_weather_stations.csv` (sha256
/// d453ec428e9ddf21375dc1a3c606cb7ec628eed280cd49ca26d3e2c5ff094ecd), the
/// row `lookup_weather_data_from_wmo` (defaults.rb:5471-5500) returns.
/// `scripts/check_os_hpxml_tables.sh` regenerates it; the licence notice is
/// `data/OS-HPXML-LICENSE.md`.
const WMO_IECC_ZONES_CSV: &str = include_str!("../../data/wmo_iecc_zones.csv");

fn wmo_iecc_zones() -> &'static HashMap<&'static str, &'static str> {
    static ZONES: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    ZONES.get_or_init(|| {
        WMO_IECC_ZONES_CSV
            .lines()
            .skip(1)
            .map(|line| {
                line.split_once(',')
                    .expect("wmo_iecc_zones.csv rows are `station_wmo,iecc_zone`")
            })
            .collect()
    })
}

/// OS-HPXML's IECC zone for the weather station `wmo`, or `None` when its
/// table has no such station.
pub(crate) fn iecc_zone_for_wmo(wmo: &str) -> Option<&'static str> {
    wmo_iecc_zones().get(wmo.trim()).copied()
}

/// Give a building with no `ClimateZoneIECC` the zone of its weather
/// station, as OS-HPXML does before any climate-dependent default
/// (defaults.rb:974-983, `apply_climate_and_risk_zones`). The derived zone,
/// or its absence when the station is unknown or the weather format names
/// none (OS-HPXML then leaves the zone unset), is recorded as a warning.
pub fn apply_climate_zone_default(building: &mut Building, station_wmo: Option<&str>) {
    if building.climate_zone_iecc.is_some() {
        return;
    }
    let zone = station_wmo.and_then(iecc_zone_for_wmo);
    let message = match (station_wmo, zone) {
        (Some(wmo), Some(zone)) => format!(
            "the HPXML has no ClimateZoneIECC; IECC zone {zone} derived from weather \
             station WMO {wmo} as OS-HPXML does"
        ),
        (Some(wmo), None) => format!(
            "the HPXML has no ClimateZoneIECC and weather station WMO {wmo} is not in \
             OS-HPXML's station table; climate-dependent defaults take their no-zone rule"
        ),
        (None, _) => "the HPXML has no ClimateZoneIECC and the weather file names no \
                      station; climate-dependent defaults take their no-zone rule"
            .to_string(),
    };
    building.climate_zone_iecc = zone.map(str::to_string);
    building.parse_warnings.push(Warning::new("hpxml", message));
}

/// The building's effective IECC zone for a path-based reader: the HPXML's
/// declared zone, else the weather station's zone (the OS-HPXML default,
/// `apply_climate_and_risk_zones`, defaults.rb:974-983). One resolution
/// path for the dwelling ([`apply_climate_zone_default`]) and the fleet's
/// zone-match validation, so a building whose HPXML gives no zone is
/// validated against the same zone the dwelling would derive.
pub fn resolve_iecc_climate_zone(
    declared: Option<&str>,
    station_wmo: Option<&str>,
) -> Option<String> {
    declared
        .map(str::to_string)
        .or_else(|| station_wmo.and_then(iecc_zone_for_wmo).map(str::to_string))
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    use super::{WMO_IECC_ZONES_CSV, iecc_zone_for_wmo};

    /// The committed table is the one `scripts/check_os_hpxml_tables.sh`
    /// regenerates from the pinned upstream source; an edit by hand fails
    /// here.
    #[test]
    fn station_table_is_the_regenerated_one() {
        let digest = Sha256::digest(WMO_IECC_ZONES_CSV.as_bytes());
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "0648ef5cd50114870549a039cde22ee4f23b1220e8de1e9eb8271ff4f934bdcb"
        );
    }

    /// Denver International and Miami International, the stations of the
    /// OS-HPXML samples, and a station no table row names.
    #[test]
    fn station_zones_follow_the_os_hpxml_table() {
        assert_eq!(iecc_zone_for_wmo("725650"), Some("5B"));
        assert_eq!(iecc_zone_for_wmo("722020"), Some("1A"));
        assert_eq!(iecc_zone_for_wmo("999999"), None);
    }
}
