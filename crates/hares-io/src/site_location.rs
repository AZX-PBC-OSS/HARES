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
//!   timezone → IANA timezone looked up from the resolved coordinates (via
//!   [`tzf-rs`], evaluated at standard time) → derived from longitude
//!   (`round(longitude / 15)`) only if the coordinate lookup fails.
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

use chrono::{NaiveDate, Offset, TimeZone};

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
#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
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

/// The three explicit value sources for a field, highest precedence first.
/// Shared by coordinate and UTC-offset resolution so the disagreement-warning
/// and precedence-selection logic lives in exactly one place.
type Candidates = [(FieldSource, Option<f64>); 3];

/// Build the candidate list (override → HPXML → weather) for a field.
fn candidates(over: Option<f64>, hpxml: Option<f64>, weather: Option<f64>) -> Candidates {
    [
        (FieldSource::CallerOverride, over),
        (FieldSource::Hpxml, hpxml),
        (FieldSource::WeatherFile, weather),
    ]
}

/// Warn on every pairwise disagreement between *present* candidate sources
/// that exceeds `threshold`, so a deliberate override or a nearby-station
/// mismatch is always surfaced before the highest-precedence value is chosen.
fn warn_on_disagreement(field: &str, unit: &str, candidates: &Candidates, threshold: f64) {
    for i in 0..candidates.len() {
        for j in (i + 1)..candidates.len() {
            if let (Some(a), Some(b)) = (candidates[i].1, candidates[j].1)
                && (a - b).abs() > threshold
            {
                tracing::warn!(
                    field,
                    %unit,
                    source_a = candidates[i].0.label(),
                    value_a = a,
                    source_b = candidates[j].0.label(),
                    value_b = b,
                    threshold,
                    "site {field}: {} ({a} {unit}) and {} ({b} {unit}) disagree by more than \
                     {threshold} {unit}; using highest-precedence source",
                    candidates[i].0.label(),
                    candidates[j].0.label(),
                );
            }
        }
    }
}

/// Select the first present candidate by precedence, if any.
fn select_by_precedence(candidates: &Candidates) -> Option<(f64, FieldSource)> {
    candidates
        .iter()
        .find_map(|&(source, value)| value.map(|v| (v, source)))
}

/// Resolve a single coordinate (lat/lon/elevation) by precedence
/// override → HPXML → weather, warning when two present sources disagree.
///
/// `default` is used only when no source supplies the value (reported with
/// [`FieldSource::WeatherFile`], the lowest-precedence real source).
/// Disagreement is judged against `threshold`; `name`/`unit` label the warning.
fn resolve_coord(
    name: &str,
    unit: &str,
    over: Option<f64>,
    hpxml: Option<f64>,
    weather: Option<f64>,
    threshold: f64,
    default: f64,
) -> (f64, FieldSource) {
    let cands = candidates(over, hpxml, weather);
    warn_on_disagreement(name, unit, &cands, threshold);
    select_by_precedence(&cands).unwrap_or((default, FieldSource::WeatherFile))
}

/// Resolve the building's site location from all available sources.
///
/// See the [module docs](self) for the precedence rules. The returned
/// [`SiteLocation`] always has finite lat/lon/UTC-offset. Emits `warn!` on
/// cross-source disagreement and an `info!` announcing the final values and
/// their provenance.
///
/// Whether the weather file's lat/lon/timezone are real or `0.0` placeholders
/// is read from [`WeatherMeta::has_embedded_location`]: EPW/PSM3/TMY3 set it
/// `true` (their headers carry location), ResStock CSV sets it `false` (parsed
/// with `0.0` placeholders). When `false` the weather metadata is treated as
/// absent so a genuine equator/prime-meridian/UTC site is never confused with
/// a missing value.
#[must_use]
pub fn resolve_site_location(
    site: &Site,
    weather: &WeatherMeta,
    over: &SiteLocationOverride,
) -> SiteLocation {
    let has_location = weather.has_embedded_location;
    let weather_lat = has_location.then_some(weather.latitude);
    let weather_lon = has_location.then_some(weather.longitude);
    let weather_elev = has_location.then_some(weather.elevation_m);
    let weather_tz = has_location.then_some(weather.timezone_offset_h);

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
        latitude_deg,
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

/// Resolve the UTC offset by precedence
/// override → HPXML → weather → coordinate timezone lookup → longitude.
fn resolve_utc_offset(
    over: Option<f64>,
    hpxml: Option<f64>,
    weather: Option<f64>,
    resolved_latitude_deg: f64,
    resolved_longitude_deg: f64,
) -> (f64, FieldSource) {
    let cands = candidates(over, hpxml, weather);
    warn_on_disagreement("UTC offset", "h", &cands, UTC_OFFSET_MISMATCH_THRESHOLD_H);
    if let Some(selected) = select_by_precedence(&cands) {
        return selected;
    }

    // No explicit source. Look up the IANA timezone from coordinates and take
    // its standard-time offset — accurate civil time honouring political
    // boundaries. Fall back to a longitude estimate only if the lookup fails.
    if let Some((offset_h, tz_name)) =
        standard_offset_from_coords(resolved_latitude_deg, resolved_longitude_deg)
    {
        tracing::info!(
            latitude_deg = resolved_latitude_deg,
            longitude_deg = resolved_longitude_deg,
            iana_timezone = %tz_name,
            utc_offset_h = offset_h,
            "site UTC offset: no source provided an offset; looked up {offset_h:+}h \
             (standard time, {tz_name}) from coordinates. DST is applied separately via \
             civil_timezone when enabled.",
        );
        return (offset_h, FieldSource::TimezoneLookup);
    }

    let derived = (resolved_longitude_deg / DEGREES_PER_HOUR).round();
    tracing::warn!(
        longitude_deg = resolved_longitude_deg,
        derived_utc_offset_h = derived,
        "site UTC offset: no source provided an offset and coordinate-based timezone lookup \
         failed (coordinates may be over open ocean); derived {derived:+}h from longitude \
         {resolved_longitude_deg}° (round(lon/15)). This ignores political and DST boundaries; \
         pass an explicit UTC offset or an HPXML Site/TimeZone/UTCOffset for civil-time accuracy.",
    );
    (derived, FieldSource::DerivedFromLongitude)
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

    /// Weather metadata with embedded location (EPW/PSM3/TMY3-style).
    fn weather_embedded(lat: f64, lon: f64, elev: f64, tz: f64) -> WeatherMeta {
        WeatherMeta {
            location: "test".to_string(),
            latitude: lat,
            longitude: lon,
            timezone_offset_h: tz,
            elevation_m: elev,
            wf_allows_leap_years: true,
            source_step_secs: 3600,
            midpoint_offset_secs: 0,
            has_embedded_location: true,
        }
    }

    /// Weather metadata without embedded location (ResStock-CSV-style): the
    /// `0.0` placeholders must be treated as absent by the resolver.
    fn weather_no_location() -> WeatherMeta {
        WeatherMeta {
            has_embedded_location: false,
            ..weather_embedded(0.0, 0.0, 0.0, 0.0)
        }
    }

    /// HPXML coordinates win over the weather file's embedded station coords.
    #[test]
    fn hpxml_coords_win_over_weather() {
        let s = site(Some(33.5), Some(-86.5), Some(100.0), None);
        let w = weather_embedded(39.7, -105.0, 1609.0, -7.0);
        let loc = resolve_site_location(&s, &w, &SiteLocationOverride::default());
        assert_eq!(loc.latitude_deg, 33.5);
        assert_eq!(loc.longitude_deg, -86.5);
        assert_eq!(loc.latitude_source, FieldSource::Hpxml);
        assert_eq!(loc.longitude_source, FieldSource::Hpxml);
    }

    /// When HPXML lacks coordinates, the weather file fills them in.
    #[test]
    fn weather_fills_missing_hpxml_coords() {
        let s = site(None, None, None, None);
        let w = weather_embedded(39.7, -105.0, 1609.0, -7.0);
        let loc = resolve_site_location(&s, &w, &SiteLocationOverride::default());
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
        let w = weather_embedded(33.5, -86.5, 0.0, -5.0);
        let loc = resolve_site_location(&s, &w, &SiteLocationOverride::default());
        assert_eq!(loc.utc_offset_h, -6.0);
        assert_eq!(loc.utc_offset_source, FieldSource::Hpxml);
    }

    /// ResStock CSV: no embedded weather coords, no HPXML timezone — lat/lon
    /// from HPXML, UTC offset resolved by coordinate timezone lookup. This is
    /// the exact scenario behind the PV underproduction bug (Alabama → CST,
    /// -6h), which previously silently defaulted to UTC.
    #[test]
    fn resstock_csv_resolves_offset_from_coordinates() {
        let s = site(Some(33.5), Some(-86.5), Some(100.0), None);
        // ResStock weather is parsed with 0.0 placeholders.
        let w = weather_no_location();
        let loc = resolve_site_location(&s, &w, &SiteLocationOverride::default());
        assert_eq!(loc.latitude_deg, 33.5);
        assert_eq!(loc.longitude_deg, -86.5);
        assert_eq!(loc.latitude_source, FieldSource::Hpxml);
        // Alabama (33.5, -86.5) → America/Chicago → CST = UTC-6 (standard time).
        assert_eq!(loc.utc_offset_h, -6.0);
        assert_eq!(loc.utc_offset_source, FieldSource::TimezoneLookup);
    }

    /// A caller override wins over both HPXML and the weather file.
    #[test]
    fn caller_override_wins() {
        let s = site(Some(33.5), Some(-86.5), Some(100.0), Some(-6.0));
        let w = weather_embedded(33.5, -86.5, 100.0, -6.0);
        let over = SiteLocationOverride {
            latitude_deg: Some(40.0),
            longitude_deg: Some(-74.0),
            elevation_m: Some(10.0),
            utc_offset_h: Some(-5.0),
        };
        let loc = resolve_site_location(&s, &w, &over);
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
        let w = weather_embedded(33.5, -86.5, 100.0, -6.0);
        let over = SiteLocationOverride {
            utc_offset_h: Some(-5.0),
            ..SiteLocationOverride::default()
        };
        let loc = resolve_site_location(&s, &w, &over);
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
        let w = weather_embedded(0.0, 0.0, 0.0, 0.0);
        let loc = resolve_site_location(&s, &w, &SiteLocationOverride::default());
        assert_eq!(loc.longitude_deg, 0.0);
        assert_eq!(loc.utc_offset_h, 0.0);
        assert_eq!(loc.utc_offset_source, FieldSource::Hpxml);
    }

    /// Coordinate-based timezone lookup yields the correct civil standard-time
    /// offset (New York → America/New_York → EST = UTC-5).
    #[test]
    fn coordinate_lookup_resolves_civil_offset() {
        let s = site(Some(40.71), Some(-74.0), None, None);
        let w = weather_no_location();
        let loc = resolve_site_location(&s, &w, &SiteLocationOverride::default());
        assert_eq!(loc.utc_offset_h, -5.0);
        assert_eq!(loc.utc_offset_source, FieldSource::TimezoneLookup);
    }

    /// Southern-hemisphere lookup uses July (winter) for standard time:
    /// Sydney → Australia/Sydney → AEST = UTC+10 (not AEDT +11).
    #[test]
    fn southern_hemisphere_uses_winter_standard_time() {
        let s = site(Some(-33.87), Some(151.21), None, None);
        let w = weather_no_location();
        let loc = resolve_site_location(&s, &w, &SiteLocationOverride::default());
        assert_eq!(loc.utc_offset_h, 10.0);
        assert_eq!(loc.utc_offset_source, FieldSource::TimezoneLookup);
    }

    /// Open-ocean coordinates fall through to the longitude estimate.
    #[test]
    fn open_ocean_falls_back_to_longitude() {
        // Mid-Pacific, far from any landmass/timezone polygon.
        let s = site(Some(0.0), Some(-150.0), None, None);
        let w = weather_no_location();
        let loc = resolve_site_location(&s, &w, &SiteLocationOverride::default());
        // tzf-rs maps oceans to Etc/GMT zones in many cases; accept either a
        // lookup result or the longitude fallback, but the offset must be the
        // sensible -10h for longitude -150°.
        assert_eq!(loc.utc_offset_h, -10.0);
        assert!(matches!(
            loc.utc_offset_source,
            FieldSource::TimezoneLookup | FieldSource::DerivedFromLongitude
        ));
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
