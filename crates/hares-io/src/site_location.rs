//! Unified site-location resolution.
//!
//! A building simulation needs one authoritative answer to "where is this
//! building, and what is its civil-time offset from UTC?" Three sources can
//! supply that information, and they frequently disagree:
//!
//! 1. **HPXML** — the building model carries authoritative `Site/Latitude`,
//!    `Site/Longitude`, `Site/Elevation`, and an optional
//!    `Site/TimeZone/UTCOffset` (standard time).
//! 2. **Weather file** — EPW/PSM3/TMY3 headers embed their own lat/lon/tz for
//!    the recording *station*; ResStock CSV carries none.
//! 3. **Caller override** — an explicit lat/lon/elevation/UTC-offset passed in
//!    by the API user (e.g. a Python `from_hpxml(..., latitude=...)` kwarg).
//!
//! [`resolve_site_location`] collapses these into a single [`SiteLocation`]
//! with per-field provenance, applying a fixed precedence:
//!
//! * **Latitude / longitude / elevation:** caller override → HPXML →
//!   weather file. The building's own coordinates win over the weather
//!   station's so that the *same* weather file at *different* building
//!   coordinates produces realistic solar-geometry variation rather than
//!   teleporting the building to the station.
//! * **UTC offset:** caller override → HPXML `UTCOffset` → weather-file
//!   timezone → derived from the resolved longitude (`round(longitude / 15)`).
//!
//! Whenever two *present* sources disagree beyond a tolerance the resolver
//! emits a loud [`tracing::warn!`] — a mismatch may be deliberate (a nearby
//! station, an intentional override), so it warns rather than errors. The
//! final resolved value and its source are always announced at
//! [`tracing::info!`] so the chosen geometry can never silently default to
//! the prime meridian / UTC again.
//!
//! The resolved [`SiteLocation`] is the single source of truth: callers write
//! it back into both the building's `Site` and the weather file's
//! [`WeatherMeta`] so every downstream consumer (solar position, start-time
//! reconciliation, pressure, mains temperature) reads consistent values.

use std::sync::OnceLock;

use chrono::{Datelike, NaiveDate, Offset, TimeZone};

use crate::WeatherMeta;
use crate::hpxml::Site;

/// Process-wide [`tzf_rs::DefaultFinder`]. Construction loads the embedded
/// timezone-polygon dataset (~non-trivial), so it is built once on first use
/// and shared. The finder is read-only and thread-safe.
fn tz_finder() -> &'static tzf_rs::DefaultFinder {
    static FINDER: OnceLock<tzf_rs::DefaultFinder> = OnceLock::new();
    FINDER.get_or_init(tzf_rs::DefaultFinder::new)
}

/// Derive a site's **standard-time** UTC offset (hours) from coordinates.
///
/// Looks up the IANA timezone for `(lat, lon)` via [`tzf-rs`] (Natural Earth
/// timezone polygons), then evaluates that zone's offset on a mid-winter date
/// (January 1) so the result is the standard-time offset with no daylight
/// saving applied — matching the HPXML `UTCOffset` convention and the local
/// standard time that weather files are stamped in.
///
/// Returns `None` when the coordinates fall outside any timezone polygon
/// (e.g. open ocean) so the caller can fall back to a longitude estimate.
fn standard_offset_from_coords(latitude_deg: f64, longitude_deg: f64) -> Option<(f64, String)> {
    // tzf-rs takes (lng, lat) order.
    let tz_name = tz_finder().get_tz_name(longitude_deg, latitude_deg);
    if tz_name.is_empty() {
        return None;
    }
    let tz: chrono_tz::Tz = tz_name.parse().ok()?;
    // January 1 noon UTC of an arbitrary recent non-leap year: a mid-winter
    // instant guarantees standard time in the Northern Hemisphere. For Southern
    // Hemisphere zones January is summer (DST), so use July there instead.
    let winter_month = if latitude_deg < 0.0 { 7 } else { 1 };
    let probe = NaiveDate::from_ymd_opt(2023, winter_month, 1)?.and_hms_opt(12, 0, 0)?;
    let dt = tz.from_utc_datetime(&probe);
    let offset_secs = dt.offset().fix().local_minus_utc();
    Some((f64::from(offset_secs) / 3600.0, tz_name.to_string()))
}

/// Degrees of latitude/longitude beyond which two *present* coordinate sources
/// are considered to disagree and a warning is emitted.
///
/// Mirrors the 1.0° threshold EnergyPlus's `WeatherManager.cc` uses when the
/// IDF `Site:Location` differs from the EPW header.
const COORD_MISMATCH_THRESHOLD_DEG: f64 = 1.0;

/// Hours of UTC offset beyond which two *present* timezone sources are
/// considered to disagree and a warning is emitted. Half an hour catches
/// whole-hour and 30-minute-zone mismatches while tolerating float noise.
const UTC_OFFSET_MISMATCH_THRESHOLD_H: f64 = 0.5;

/// Degrees of longitude per hour of solar time (`360° / 24h`). Used to derive
/// a fallback UTC offset when no source supplies one.
const DEGREES_PER_HOUR: f64 = 15.0;

/// Where a single resolved field's value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldSource {
    /// An explicit caller-supplied override.
    CallerOverride,
    /// The HPXML building model's `Site` element.
    Hpxml,
    /// The weather file's embedded header metadata.
    WeatherFile,
    /// Looked up from the resolved coordinates via the `tzf-rs` IANA timezone
    /// polygons, evaluated at standard time. Only applies to the UTC offset.
    TimezoneLookup,
    /// Derived from the resolved longitude (`round(longitude / 15)`) as a last
    /// resort when coordinate-based lookup fails (e.g. open ocean). Only
    /// applies to the UTC offset.
    DerivedFromLongitude,
}

impl FieldSource {
    /// Human-readable label for logging.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            FieldSource::CallerOverride => "caller override",
            FieldSource::Hpxml => "HPXML",
            FieldSource::WeatherFile => "weather file",
            FieldSource::TimezoneLookup => "timezone lookup (tzf-rs)",
            FieldSource::DerivedFromLongitude => "derived from longitude",
        }
    }
}

/// An explicit caller-supplied site location. Every field is optional; a
/// `Some` value overrides both HPXML and the weather file for that field.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SiteLocationOverride {
    pub latitude_deg: Option<f64>,
    pub longitude_deg: Option<f64>,
    pub elevation_m: Option<f64>,
    /// Standard-time offset from UTC in hours (e.g. `-5.0` for US Eastern).
    pub utc_offset_h: Option<f64>,
}

impl SiteLocationOverride {
    /// `true` when no override field is set.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.latitude_deg.is_none()
            && self.longitude_deg.is_none()
            && self.elevation_m.is_none()
            && self.utc_offset_h.is_none()
    }
}

/// A fully resolved site location with per-field provenance.
///
/// Latitude, longitude, and UTC offset are always populated (the resolver
/// guarantees a value via the longitude fallback). Elevation defaults to sea
/// level when no source provides it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SiteLocation {
    pub latitude_deg: f64,
    pub longitude_deg: f64,
    pub elevation_m: f64,
    /// Standard-time offset from UTC in hours.
    pub utc_offset_h: f64,
    pub latitude_source: FieldSource,
    pub longitude_source: FieldSource,
    pub elevation_source: FieldSource,
    pub utc_offset_source: FieldSource,
}

/// Resolve a single coordinate (lat/lon/elevation) by precedence
/// override → HPXML → weather, warning when two present sources disagree.
///
/// `default` is used only when no source supplies the value (returns
/// [`FieldSource::WeatherFile`] as the nominal source, since the weather file
/// is the lowest-precedence real source). Disagreement is judged against
/// `threshold`; `name` and `unit` label the warning.
fn resolve_coord(
    name: &str,
    unit: &str,
    over: Option<f64>,
    hpxml: Option<f64>,
    weather: Option<f64>,
    threshold: f64,
    default: f64,
) -> (f64, FieldSource) {
    // Warn on any pairwise disagreement between *present* sources before
    // selecting, so a deliberate override or a nearby-station mismatch is
    // always surfaced.
    let present: [(FieldSource, Option<f64>); 3] = [
        (FieldSource::CallerOverride, over),
        (FieldSource::Hpxml, hpxml),
        (FieldSource::WeatherFile, weather),
    ];
    for i in 0..present.len() {
        for j in (i + 1)..present.len() {
            if let (Some(a), Some(b)) = (present[i].1, present[j].1) {
                if (a - b).abs() > threshold {
                    tracing::warn!(
                        field = name,
                        %unit,
                        source_a = present[i].0.label(),
                        value_a = a,
                        source_b = present[j].0.label(),
                        value_b = b,
                        threshold,
                        "site {name}: {} ({a} {unit}) and {} ({b} {unit}) disagree by more than \
                         {threshold} {unit}; using highest-precedence source",
                        present[i].0.label(),
                        present[j].0.label(),
                    );
                }
            }
        }
    }

    if let Some(v) = over {
        (v, FieldSource::CallerOverride)
    } else if let Some(v) = hpxml {
        (v, FieldSource::Hpxml)
    } else if let Some(v) = weather {
        (v, FieldSource::WeatherFile)
    } else {
        (default, FieldSource::WeatherFile)
    }
}

/// Resolve the building's site location from all available sources.
///
/// See the [module docs](self) for the precedence rules. The returned
/// [`SiteLocation`] always has finite lat/lon/UTC-offset. Emits `warn!` on
/// cross-source disagreement and an `info!` announcing the final values and
/// their provenance.
///
/// `weather` may carry zero/sentinel values (ResStock CSV is parsed with
/// `0.0` placeholders); such values are treated as "absent" only for the UTC
/// offset (a `0.0` weather offset is ambiguous with a genuine UTC site, so the
/// caller indicates absence via `weather_has_timezone`). Latitude/longitude of
/// exactly `0.0` from the weather file are taken at face value — a real site
/// on the equator/prime meridian is possible — so for formats without embedded
/// coordinates the caller should pass `weather_has_coords = false`.
#[must_use]
pub fn resolve_site_location(
    site: &Site,
    weather: &WeatherMeta,
    weather_has_coords: bool,
    weather_has_timezone: bool,
    over: &SiteLocationOverride,
) -> SiteLocation {
    let weather_lat = weather_has_coords.then_some(weather.latitude);
    let weather_lon = weather_has_coords.then_some(weather.longitude);
    let weather_elev = weather_has_coords.then_some(weather.elevation_m);
    let weather_tz = weather_has_timezone.then_some(weather.timezone_offset_h);

    let (latitude_deg, latitude_source) = resolve_coord(
        "latitude",
        "°",
        over.latitude_deg,
        site.latitude_deg,
        weather_lat,
        COORD_MISMATCH_THRESHOLD_DEG,
        0.0,
    );
    let (longitude_deg, longitude_source) = resolve_coord(
        "longitude",
        "°",
        over.longitude_deg,
        site.longitude_deg,
        weather_lon,
        COORD_MISMATCH_THRESHOLD_DEG,
        0.0,
    );
    let (elevation_m, elevation_source) = resolve_coord(
        "elevation",
        "m",
        over.elevation_m,
        site.elevation_m,
        weather_elev,
        // Elevation rarely matters enough to warn; use a large threshold so
        // only gross disagreements (>500 m) surface.
        500.0,
        0.0,
    );

    let (utc_offset_h, utc_offset_source) = resolve_utc_offset(
        over.utc_offset_h,
        site.utc_offset_h,
        weather_tz,
        longitude_deg,
    );

    let resolved = SiteLocation {
        latitude_deg,
        longitude_deg,
        elevation_m,
        utc_offset_h,
        latitude_source,
        longitude_source,
        elevation_source,
        utc_offset_source,
    };

    announce(&resolved);
    resolved
}

/// Resolve the UTC offset by precedence override → HPXML → weather → longitude.
fn resolve_utc_offset(
    over: Option<f64>,
    hpxml: Option<f64>,
    weather: Option<f64>,
    resolved_longitude_deg: f64,
) -> (f64, FieldSource) {
    // Warn on disagreement between present explicit sources.
    let present: [(FieldSource, Option<f64>); 3] = [
        (FieldSource::CallerOverride, over),
        (FieldSource::Hpxml, hpxml),
        (FieldSource::WeatherFile, weather),
    ];
    for i in 0..present.len() {
        for j in (i + 1)..present.len() {
            if let (Some(a), Some(b)) = (present[i].1, present[j].1) {
                if (a - b).abs() > UTC_OFFSET_MISMATCH_THRESHOLD_H {
                    tracing::warn!(
                        source_a = present[i].0.label(),
                        value_a = a,
                        source_b = present[j].0.label(),
                        value_b = b,
                        "site UTC offset: {} ({a:+}h) and {} ({b:+}h) disagree by more than \
                         {UTC_OFFSET_MISMATCH_THRESHOLD_H}h; using highest-precedence source",
                        present[i].0.label(),
                        present[j].0.label(),
                    );
                }
            }
        }
    }

    if let Some(v) = over {
        (v, FieldSource::CallerOverride)
    } else if let Some(v) = hpxml {
        (v, FieldSource::Hpxml)
    } else if let Some(v) = weather {
        (v, FieldSource::WeatherFile)
    } else {
        let derived = (resolved_longitude_deg / DEGREES_PER_HOUR).round();
        tracing::warn!(
            longitude_deg = resolved_longitude_deg,
            derived_utc_offset_h = derived,
            "site UTC offset: no source provided an offset; derived {derived:+}h from longitude \
             {resolved_longitude_deg}° (round(lon/15)). This ignores political timezone and DST \
             boundaries; pass an explicit UTC offset or an HPXML Site/TimeZone/UTCOffset for \
             civil-time accuracy.",
        );
        (derived, FieldSource::DerivedFromLongitude)
    }
}

/// Emit a single `info!` line announcing the resolved location and provenance.
fn announce(loc: &SiteLocation) {
    tracing::info!(
        latitude_deg = loc.latitude_deg,
        latitude_source = loc.latitude_source.label(),
        longitude_deg = loc.longitude_deg,
        longitude_source = loc.longitude_source.label(),
        elevation_m = loc.elevation_m,
        elevation_source = loc.elevation_source.label(),
        utc_offset_h = loc.utc_offset_h,
        utc_offset_source = loc.utc_offset_source.label(),
        "resolved site location: lat={:.4}° ({}), lon={:.4}° ({}), elev={:.0}m ({}), \
         UTC{:+}h ({})",
        loc.latitude_deg,
        loc.latitude_source.label(),
        loc.longitude_deg,
        loc.longitude_source.label(),
        loc.elevation_m,
        loc.elevation_source.label(),
        loc.utc_offset_h,
        loc.utc_offset_source.label(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(lat: Option<f64>, lon: Option<f64>, elev: Option<f64>, tz: Option<f64>) -> Site {
        Site {
            elevation_m: elev,
            site_type: None,
            shielding_of_home: None,
            latitude_deg: lat,
            longitude_deg: lon,
            utc_offset_h: tz,
        }
    }

    fn weather_meta(lat: f64, lon: f64, elev: f64, tz: f64) -> WeatherMeta {
        WeatherMeta {
            location: "test".to_string(),
            latitude: lat,
            longitude: lon,
            timezone_offset_h: tz,
            elevation_m: elev,
            wf_allows_leap_years: true,
            source_step_secs: 3600,
            midpoint_offset_secs: 0,
        }
    }

    /// HPXML coordinates win over the weather file's embedded station coords.
    #[test]
    fn hpxml_coords_win_over_weather() {
        let s = site(Some(33.5), Some(-86.5), Some(100.0), None);
        let w = weather_meta(39.7, -105.0, 1609.0, -7.0);
        let loc = resolve_site_location(&s, &w, true, true, &SiteLocationOverride::default());
        assert_eq!(loc.latitude_deg, 33.5);
        assert_eq!(loc.longitude_deg, -86.5);
        assert_eq!(loc.latitude_source, FieldSource::Hpxml);
        assert_eq!(loc.longitude_source, FieldSource::Hpxml);
    }

    /// When HPXML lacks coordinates, the weather file fills them in.
    #[test]
    fn weather_fills_missing_hpxml_coords() {
        let s = site(None, None, None, None);
        let w = weather_meta(39.7, -105.0, 1609.0, -7.0);
        let loc = resolve_site_location(&s, &w, true, true, &SiteLocationOverride::default());
        assert_eq!(loc.latitude_deg, 39.7);
        assert_eq!(loc.longitude_deg, -105.0);
        assert_eq!(loc.latitude_source, FieldSource::WeatherFile);
        assert_eq!(loc.utc_offset_h, -7.0);
        assert_eq!(loc.utc_offset_source, FieldSource::WeatherFile);
    }

    /// HPXML UTCOffset wins over a weather-file timezone.
    #[test]
    fn hpxml_utc_offset_wins() {
        let s = site(Some(33.5), Some(-86.5), None, Some(-6.0));
        let w = weather_meta(33.5, -86.5, 0.0, -5.0);
        let loc = resolve_site_location(&s, &w, true, true, &SiteLocationOverride::default());
        assert_eq!(loc.utc_offset_h, -6.0);
        assert_eq!(loc.utc_offset_source, FieldSource::Hpxml);
    }

    /// ResStock CSV: no embedded weather coords, no HPXML timezone — lat/lon
    /// from HPXML, UTC offset derived from longitude. This is the exact
    /// scenario behind the PV underproduction bug (Alabama, lon -86.5 → -6h).
    #[test]
    fn resstock_csv_derives_offset_from_longitude() {
        let s = site(Some(33.5), Some(-86.5), Some(100.0), None);
        // ResStock weather is parsed with 0.0 placeholders.
        let w = weather_meta(0.0, 0.0, 0.0, 0.0);
        let loc = resolve_site_location(&s, &w, false, false, &SiteLocationOverride::default());
        assert_eq!(loc.latitude_deg, 33.5);
        assert_eq!(loc.longitude_deg, -86.5);
        assert_eq!(loc.latitude_source, FieldSource::Hpxml);
        // round(-86.5 / 15) = round(-5.77) = -6
        assert_eq!(loc.utc_offset_h, -6.0);
        assert_eq!(loc.utc_offset_source, FieldSource::DerivedFromLongitude);
    }

    /// A caller override wins over both HPXML and the weather file.
    #[test]
    fn caller_override_wins() {
        let s = site(Some(33.5), Some(-86.5), Some(100.0), Some(-6.0));
        let w = weather_meta(33.5, -86.5, 100.0, -6.0);
        let over = SiteLocationOverride {
            latitude_deg: Some(40.0),
            longitude_deg: Some(-74.0),
            elevation_m: Some(10.0),
            utc_offset_h: Some(-5.0),
        };
        let loc = resolve_site_location(&s, &w, true, true, &over);
        assert_eq!(loc.latitude_deg, 40.0);
        assert_eq!(loc.longitude_deg, -74.0);
        assert_eq!(loc.elevation_m, 10.0);
        assert_eq!(loc.utc_offset_h, -5.0);
        assert_eq!(loc.latitude_source, FieldSource::CallerOverride);
        assert_eq!(loc.utc_offset_source, FieldSource::CallerOverride);
    }

    /// A partial override only replaces the fields it specifies.
    #[test]
    fn partial_override_leaves_other_fields() {
        let s = site(Some(33.5), Some(-86.5), Some(100.0), Some(-6.0));
        let w = weather_meta(33.5, -86.5, 100.0, -6.0);
        let over = SiteLocationOverride {
            utc_offset_h: Some(-5.0),
            ..SiteLocationOverride::default()
        };
        let loc = resolve_site_location(&s, &w, true, true, &over);
        assert_eq!(loc.latitude_deg, 33.5);
        assert_eq!(loc.latitude_source, FieldSource::Hpxml);
        assert_eq!(loc.utc_offset_h, -5.0);
        assert_eq!(loc.utc_offset_source, FieldSource::CallerOverride);
    }

    /// Genuine equator/prime-meridian site: weather file declares 0/0 and the
    /// caller marks it as having real coords, so 0.0 is honoured.
    #[test]
    fn genuine_zero_coords_honoured() {
        let s = site(None, None, None, Some(0.0));
        let w = weather_meta(0.0, 0.0, 0.0, 0.0);
        let loc = resolve_site_location(&s, &w, true, true, &SiteLocationOverride::default());
        assert_eq!(loc.longitude_deg, 0.0);
        assert_eq!(loc.utc_offset_h, 0.0);
        assert_eq!(loc.utc_offset_source, FieldSource::Hpxml);
    }

    /// Longitude fallback rounds to the nearest whole hour.
    #[test]
    fn longitude_fallback_rounds_to_nearest_hour() {
        // lon -74 (US Eastern) → round(-4.93) = -5
        let s = site(Some(40.0), Some(-74.0), None, None);
        let w = weather_meta(0.0, 0.0, 0.0, 0.0);
        let loc = resolve_site_location(&s, &w, false, false, &SiteLocationOverride::default());
        assert_eq!(loc.utc_offset_h, -5.0);
        assert_eq!(loc.utc_offset_source, FieldSource::DerivedFromLongitude);
    }

    #[test]
    fn override_is_empty_detects_no_fields() {
        assert!(SiteLocationOverride::default().is_empty());
        assert!(
            !SiteLocationOverride {
                latitude_deg: Some(1.0),
                ..SiteLocationOverride::default()
            }
            .is_empty()
        );
    }
}
