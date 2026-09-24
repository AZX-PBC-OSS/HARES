//! Compile-time test: WeatherField must be accessible from the crate root.

#[test]
fn weather_field_is_reexported_from_crate_root() {
    // This test verifies that WeatherField is publicly accessible via
    // hares_io::WeatherField. If the re-export is missing, this file
    // will fail to compile.
    let field = hares_io::WeatherField::HorizontalInfrared;
    assert_eq!(field, hares_io::WeatherField::HorizontalInfrared);

    // Also verify other commonly used fields are accessible
    let _ = hares_io::WeatherField::DryBulbC;
    let _ = hares_io::WeatherField::GhiWM2;
    let _ = hares_io::WeatherField::SkyTempC;
}
