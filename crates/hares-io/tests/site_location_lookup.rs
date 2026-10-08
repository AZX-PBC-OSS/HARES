//! The site UTC offset resolved through the pinned time-zone database for a
//! real fixture site: the OCHRE sample HPXML (which declares coordinates but
//! no `Site/TimeZone/UTCOffset`) with a ResStock CSV weather file (which
//! carries no embedded location), resolved as the blueprint resolves it.

use std::path::Path;

use hares_io::{
    FieldSource, SiteLocationOverride, parse_hpxml, parse_weather, resolve_site_location,
};

/// Repo-root fixture paths, resolved the way the crate's other integration
/// tests do (`CARGO_MANIFEST_DIR` is `crates/hares-io`).
fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives in <repo>/crates/hares-io")
}

/// `base.xml` declares Latitude 40.01 / Longitude -105.27 and no
/// `Site/TimeZone/UTCOffset`, and the ResStock CSV supplies no offset, so the
/// tzf-rs lookup (data release 2026-d-fix1) supplies it: Boulder, Colorado
/// sits in America/Denver, whose standard time is MST (UTC-7).
#[test]
fn site_offset_from_the_time_zone_database_for_a_fixture_site() {
    let building = parse_hpxml(&repo_root().join("tests/fixtures/hpxml/ochre_samples/base.xml"))
        .expect("base.xml must parse");
    let weather =
        parse_weather(repo_root().join("tests/fixtures/resstock/2025.1/weather/G0900090_2018.csv"))
            .expect("the ResStock weather CSV must parse");

    let location = resolve_site_location(
        &building.site,
        &weather.meta,
        &SiteLocationOverride::default(),
    );

    assert_eq!(location.utc_offset_h, -7.0);
    assert_eq!(location.utc_offset_source, FieldSource::TimezoneLookup);
}
