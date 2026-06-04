use std::collections::HashMap;
use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, FixedOffset, TimeZone};
use hares_core::{EnvironmentManager, SimClock};
use hares_io::hpxml::building::XmlNode;
use hares_io::hpxml::{Boundary, BoundaryType, Site, Window, Zone, ZoneType};
use hares_io::schedule::ColumnAggregation;
use hares_io::{ScheduleTimeSeries, WeatherMeta, WeatherTimeSeries};
use hares_types::SCHEDULE_DOMAIN_ID;

fn offset_east(seconds: i32) -> FixedOffset {
    FixedOffset::east_opt(seconds).expect("valid offset")
}

fn offset_west(seconds: i32) -> FixedOffset {
    FixedOffset::west_opt(seconds).expect("valid offset")
}

fn minimal_building() -> hares_io::Building {
    let details_xml = XmlNode {
        name: "BuildingDetails".to_string(),
        attrs: HashMap::new(),
        text: String::new(),
        children: vec![XmlNode {
            name: "IndoorTemperature".to_string(),
            attrs: HashMap::new(),
            text: "21.0".to_string(),
            children: Vec::new(),
        }],
    };

    hares_io::Building {
        site: Site {
            elevation_m: None,
            site_type: None,
            shielding_of_home: None,
            latitude_deg: Some(40.7128),
            longitude_deg: Some(-74.0060),
            utc_offset_h: None,
        },
        zones: vec![Zone {
            zone_type: ZoneType::Conditioned,
            floor_area_m2: Some(100.0),
            volume_m3: None,
            attached_wall_ids: vec![],
            duct_systems: vec![],
            vented: false,
            ventilation_ach: None,
            ventilation_sla: None,
        }],
        boundaries: vec![Boundary {
            id: "wall".to_string(),
            boundary_type: BoundaryType::Wall,
            area_m2: 20.0,
            azimuth_deg: Some(180.0),
            assembly_r_value_m2_k_w: None,
            r_value_layers_m2_k_w: vec![],
            interior_zone: Some(ZoneType::Conditioned),
            exterior_zone: Some(ZoneType::Outdoor),
            material_layers: vec![],
            construction_type: None,
            finish_type: None,
            insulation_details: None,
            has_radiant_barrier: false,
            solar_absorptance: None,
            emittance: None,
            tilt_deg: Some(90.0),
            framing_factor: None,
            lut_boundary_name: None,
            floor_or_ceiling: None,
            perimeter_m: None,
            perimeter_insulation_r_m2_k_w: None,
            foundation_depth_m: None,
        }],
        windows: Vec::<Window>::new(),
        infiltration_ach50: None,
        infiltration_cfm50: None,
        infiltration_ach_natural: None,
        infiltration_cfm_natural: None,
        infiltration_ela_cm2: None,
        infiltration_constant_ach: None,
        hvac_capacity_w: None,
        seer2: None,
        hspf2: None,
        water_heater_setpoint_c: None,
        heating_weekday_setpoints_c: None,
        heating_weekend_setpoints_c: None,
        cooling_weekday_setpoints_c: None,
        cooling_weekend_setpoints_c: None,
        battery_round_trip_efficiency: None,
        pv_tilt_deg: None,
        conditioned_volume_m3: None,
        ceiling_height_m: None,
        infiltration_height_m: None,
        floors_above_grade: None,
        has_flue_or_chimney: None,
        foundation_name: None,
        residential_facility_type: None,
        mass_multiplier_override: None,
        hvac_deadband_c: None,
        details_xml,
    }
}

fn hourly_schedule(start: DateTime<FixedOffset>) -> ScheduleTimeSeries {
    let timestamps: Vec<DateTime<FixedOffset>> = (0..24)
        .map(|hour| start + Duration::hours(hour.into()))
        .collect();
    let values: Vec<f64> = (0..24).map(|hour| hour as f64).collect();
    let mut index = HashMap::new();
    index.insert("occupancy".to_string(), 0);
    ScheduleTimeSeries {
        timestamps,
        column_names: vec!["occupancy".to_string()],
        columns: vec![values],
        column_index: index,
        source_step_secs: 3600,
        column_aggregations: vec![ColumnAggregation::Mean],
    }
}

fn sequential_weather(start_temp_c: f64, rows: usize, timezone_offset_h: f64) -> WeatherTimeSeries {
    let seq = |base: f64| -> Vec<f64> { (0..rows).map(|idx| base + idx as f64).collect() };
    WeatherTimeSeries {
        meta: WeatherMeta {
            location: "DST regression".to_string(),
            latitude: 40.7128,
            longitude: -74.0060,
            timezone_offset_h,
            elevation_m: 10.0,
            wf_allows_leap_years: true,
            source_step_secs: 3600,
            midpoint_offset_secs: 0,
            has_embedded_location: true,
        },
        design_conditions: None,
        dry_bulb_c: seq(start_temp_c),
        dew_point_c: vec![0.0; rows],
        rel_humidity_pct: vec![50.0; rows],
        pressure_kpa: vec![101.325; rows],
        ghi_w_m2: vec![0.0; rows],
        dni_w_m2: vec![0.0; rows],
        dhi_w_m2: vec![0.0; rows],
        wind_speed_m_s: vec![1.0; rows],
        wind_dir_deg: vec![180.0; rows],
        opaque_sky_cover: vec![0.0; rows],
        horizontal_infrared_w_m2: vec![250.0; rows],
        sky_temp_c: vec![5.0; rows],
        ground_temp_c: vec![12.0; rows],
        liquid_precip_m: vec![0.0; rows],
        surface_albedo: None,
    }
}

fn schedule_value(env: &hares_types::EnvironmentState) -> f64 {
    env.custom_domains
        .iter()
        .find(|domain| domain.domain_id == SCHEDULE_DOMAIN_ID)
        .and_then(|domain| domain.custom_payload.as_ref())
        .and_then(|payload| payload.first())
        .copied()
        .expect("schedule payload must exist")
}

/// Build a sunny-weather series so solar irradiance is non-zero at noon.
fn sunny_weather(rows: usize, lat: f64, lon: f64, timezone_offset_h: f64) -> WeatherTimeSeries {
    let mut w = sequential_weather(20.0, rows, timezone_offset_h);
    w.meta.latitude = lat;
    w.meta.longitude = lon;
    // Clear-sky-ish magnitudes for every hour; the solar-position model gates
    // actual surface irradiance by the sun's altitude, so what matters for the
    // regression is the GEOMETRY at the simulated wall-clock time.
    w.ghi_w_m2 = vec![900.0; rows];
    w.dni_w_m2 = vec![800.0; rows];
    w.dhi_w_m2 = vec![120.0; rows];
    w
}

/// Regression for the PV-underproduction / timezone bug.
///
/// When `start_time`'s UTC offset matches the site's resolved standard-time
/// offset (as `Dwelling::from_preparsed` now guarantees via the site-location
/// resolver), `solar_position` at LOCAL solar noon must place the sun HIGH in
/// the sky. The bug paired a naive `+00:00` offset with a far-west longitude,
/// so "noon" was computed as early-morning sun (~14° altitude) — collapsing PV
/// output. Here the building is at Birmingham, Alabama (lon ≈ -86.8, CST = -6h)
/// and the simulation starts at local noon on a summer day.
#[test]
fn local_noon_with_correct_offset_yields_high_solar_altitude() {
    let lat = 33.52;
    let lon = -86.81;
    let utc_offset_h = -6.0; // resolved CST for Alabama

    // Local wall-clock noon, stamped with the resolved standard-time offset —
    // exactly what from_preparsed produces after site-location resolution.
    let start = offset_west((-utc_offset_h * 3600.0) as i32)
        .with_ymd_and_hms(2024, 6, 21, 12, 0, 0)
        .single()
        .expect("valid local-noon start");

    let mut building = minimal_building();
    building.site.latitude_deg = Some(lat);
    building.site.longitude_deg = Some(lon);
    building.site.utc_offset_h = Some(utc_offset_h);

    let mut manager = EnvironmentManager::new(
        sunny_weather(24, lat, lon, utc_offset_h),
        hourly_schedule(start),
        &building,
        StdDuration::from_secs(3600),
        start,
        None,
    )
    .expect("manager");
    let clock = SimClock::new(start, Duration::hours(1), Duration::hours(1));

    let env = manager.update(&clock, &[]).unwrap();
    let altitude = env.weather.solar_altitude_deg;

    // At Birmingham on the summer solstice, solar noon altitude ≈ 90 - (33.5 -
    // 23.4) ≈ 80°. With the CORRECT offset the sun is high (>60°); the bug's
    // wrong +00:00 offset would yield ≈14° (early-morning sun).
    assert!(
        altitude > 60.0,
        "local-noon solar altitude with correct UTC offset must be high (sun overhead), \
         got {altitude:.1}° — a low value indicates the timezone/solar-geometry regression"
    );

    // And surfaces must receive substantial irradiance at this geometry. The
    // minimal building has only a south-facing VERTICAL wall (tilt 90°); with
    // the sun nearly overhead the AOI is large, so POA is naturally lower than
    // on a tilted panel — but it must be clearly non-collapsed (>150 W/m²),
    // unlike the early-morning-sun bug where the high-magnitude weather was
    // projected onto a near-horizon sun.
    let max_poa = env
        .weather
        .solar_irradiance
        .iter()
        .map(|s| s.direct_w_m2 + s.diffuse_w_m2 + s.reflected_w_m2)
        .fold(0.0_f64, f64::max);
    assert!(
        max_poa > 150.0,
        "plane-of-array irradiance at local noon must be substantial, got {max_poa:.0} W/m²"
    );
}

/// The mirror image: feeding the WRONG offset (naive +00:00, the pre-fix
/// behaviour) at this far-west longitude must produce a LOW solar altitude at
/// nominal "noon". This locks in WHY the offset must be resolved correctly.
#[test]
fn local_noon_with_wrong_utc_offset_yields_low_solar_altitude() {
    let lat = 33.52;
    let lon = -86.81;

    // Naive +00:00 — the buggy stamping the resolver now prevents.
    let start = offset_east(0)
        .with_ymd_and_hms(2024, 6, 21, 12, 0, 0)
        .single()
        .expect("valid start");

    let mut building = minimal_building();
    building.site.latitude_deg = Some(lat);
    building.site.longitude_deg = Some(lon);

    let mut manager = EnvironmentManager::new(
        sunny_weather(24, lat, lon, 0.0),
        hourly_schedule(start),
        &building,
        StdDuration::from_secs(3600),
        start,
        None,
    )
    .expect("manager");
    let clock = SimClock::new(start, Duration::hours(1), Duration::hours(1));

    let env = manager.update(&clock, &[]).unwrap();
    let altitude = env.weather.solar_altitude_deg;

    // 12:00 UTC at lon -86.8 is ~6 AM local → sun barely above the horizon.
    assert!(
        altitude < 30.0,
        "12:00 UTC at lon -86.8° must be early-morning sun (low altitude), got {altitude:.1}°; \
         this is the geometry the resolver corrects by stamping the right offset"
    );
}

#[test]
fn fixed_offset_schedule_does_not_apply_dst_without_civil_timezone() {
    let start = offset_west(5 * 3600)
        .with_ymd_and_hms(2024, 3, 10, 0, 0, 0)
        .single()
        .expect("valid spring-forward start");
    let mut manager = EnvironmentManager::new(
        sequential_weather(20.0, 24, -5.0),
        hourly_schedule(start),
        &minimal_building(),
        StdDuration::from_secs(3600),
        start,
        None,
    )
    .expect("manager");
    let mut clock = SimClock::new(start, Duration::hours(1), Duration::hours(4));

    let mut observed_schedule = Vec::new();
    let mut observed_weather = Vec::new();
    for _ in 0..4 {
        let env = manager.update(&clock, &[]).unwrap();
        observed_schedule.push(schedule_value(&env));
        observed_weather.push(env.weather.outdoor_temp_c);
        let _ = clock.next();
    }

    assert_eq!(
        observed_schedule,
        vec![0.0, 1.0, 2.0, 3.0],
        "without civil_timezone, schedule indexing must follow fixed-offset wall-clock hours"
    );
    assert_eq!(
        observed_weather,
        vec![20.0, 21.0, 22.0, 23.0],
        "weather indexing must advance sequentially by timestep"
    );
}

#[cfg(not(feature = "dst"))]
#[test]
fn civil_timezone_requires_dst_feature() {
    let start = offset_west(5 * 3600)
        .with_ymd_and_hms(2024, 3, 10, 0, 0, 0)
        .single()
        .expect("valid start");
    let err = EnvironmentManager::new(
        sequential_weather(10.0, 24, -5.0),
        hourly_schedule(start),
        &minimal_building(),
        StdDuration::from_secs(3600),
        start,
        Some("America/New_York"),
    )
    .expect_err("civil timezone should require dst feature");

    assert!(
        matches!(
            err,
            hares_core::environment::EnvironmentManagerError::DstNotEnabled
        ),
        "expected DstNotEnabled, got {err:?}"
    );
}

#[cfg(feature = "dst")]
#[test]
fn spring_forward_civil_timezone_skips_schedule_hour_but_not_weather_hour() {
    let start = offset_west(5 * 3600)
        .with_ymd_and_hms(2024, 3, 10, 0, 0, 0)
        .single()
        .expect("valid spring-forward start");
    let mut manager = EnvironmentManager::new(
        sequential_weather(30.0, 24, -5.0),
        hourly_schedule(start),
        &minimal_building(),
        StdDuration::from_secs(3600),
        start,
        Some("America/New_York"),
    )
    .expect("manager");
    let mut clock = SimClock::new(start, Duration::hours(1), Duration::hours(5));

    let mut observed_schedule = Vec::new();
    let mut observed_weather = Vec::new();
    for _ in 0..5 {
        let env = manager.update(&clock, &[]).unwrap();
        observed_schedule.push(schedule_value(&env));
        observed_weather.push(env.weather.outdoor_temp_c);
        let _ = clock.next();
    }

    assert_eq!(
        observed_schedule,
        vec![0.0, 1.0, 3.0, 4.0, 5.0],
        "civil schedule indexing must skip the nonexistent 02:00 wall-clock hour on spring forward"
    );
    assert_eq!(
        observed_weather,
        vec![30.0, 31.0, 32.0, 33.0, 34.0],
        "weather indexing must remain sequential and physical across spring DST transition"
    );
}

#[cfg(feature = "dst")]
#[test]
fn fall_back_civil_timezone_repeats_schedule_hour_but_not_weather_hour() {
    let start = offset_west(4 * 3600)
        .with_ymd_and_hms(2024, 11, 3, 0, 0, 0)
        .single()
        .expect("valid fall-back start");
    let mut manager = EnvironmentManager::new(
        sequential_weather(40.0, 24, -4.0),
        hourly_schedule(start),
        &minimal_building(),
        StdDuration::from_secs(3600),
        start,
        Some("America/New_York"),
    )
    .expect("manager");
    let mut clock = SimClock::new(start, Duration::hours(1), Duration::hours(5));

    let mut observed_schedule = Vec::new();
    let mut observed_weather = Vec::new();
    for _ in 0..5 {
        let env = manager.update(&clock, &[]).unwrap();
        observed_schedule.push(schedule_value(&env));
        observed_weather.push(env.weather.outdoor_temp_c);
        let _ = clock.next();
    }

    assert_eq!(
        observed_schedule,
        vec![0.0, 1.0, 1.0, 2.0, 3.0],
        "civil schedule indexing must repeat the 01:00 wall-clock hour on fall back"
    );
    assert_eq!(
        observed_weather,
        vec![40.0, 41.0, 42.0, 43.0, 44.0],
        "weather indexing must remain sequential and physical across fall DST transition"
    );
}

#[cfg(feature = "dst")]
#[test]
fn invalid_civil_timezone_is_rejected() {
    let start = offset_east(0)
        .with_ymd_and_hms(2024, 1, 1, 0, 0, 0)
        .single()
        .expect("valid start");
    let err = EnvironmentManager::new(
        sequential_weather(0.0, 24, 0.0),
        hourly_schedule(start),
        &minimal_building(),
        StdDuration::from_secs(3600),
        start,
        Some("Mars/Olympus_Mons"),
    )
    .expect_err("invalid timezone must fail");

    assert!(
        matches!(
            err,
            hares_core::environment::EnvironmentManagerError::InvalidTimezone(ref name)
                if name == "Mars/Olympus_Mons"
        ),
        "expected InvalidTimezone for bad tz name, got {err:?}"
    );
}

#[cfg(feature = "dst")]
mod simclock_dst_tests {
    use chrono::{Datelike, Duration, FixedOffset, TimeZone, Timelike};
    use chrono_tz::America::New_York;
    use hares_core::SimClock;

    /// Spring-forward: at step 23 starting from 2024-03-10 00:00 EST (-05:00),
    /// the fixed-offset clock says March 10 23:00 EST (ordinal0 = 69), but the
    /// DST-aware civil time must be March 11 00:00 EDT (ordinal0 = 70).
    #[test]
    fn civil_time_skips_spring_forward_hour() {
        let start = FixedOffset::west_opt(5 * 3600)
            .expect("valid offset")
            .with_ymd_and_hms(2024, 3, 10, 0, 0, 0)
            .single()
            .expect("valid start");

        let mut clock = SimClock::new(start, Duration::hours(1), Duration::hours(24));
        clock.civil_tz = Some(New_York);

        // Step through to step 23 (= 23 hours elapsed).
        for _ in 0..23 {
            assert!(clock.next().is_some());
        }
        assert_eq!(clock.current_step(), 23);

        // Fixed-offset time at step 23: March 10 23:00 EST (ordinal0 = 69).
        let fixed = clock.current_time();
        assert_eq!(fixed.hour(), 23);
        assert_eq!(
            fixed.ordinal0(),
            69,
            "fixed-offset ordinal0 should be March 10 (day 69)"
        );

        // Civil time at step 23: March 11 00:00 EDT (ordinal0 = 70).
        let civil = clock.current_civil_time().expect("civil time must be Some");
        assert_eq!(
            civil.hour(),
            0,
            "civil hour at step 23 must be 00:00 EDT, not 23:00 EST"
        );
        assert_eq!(
            civil.ordinal0(),
            70,
            "civil ordinal0 must be March 11 (day 70), not March 10 (day 69)"
        );
        // UTC instant is preserved: fixed-offset 23:00 EST = 04:00 UTC on March 11.
        // Civil 00:00 EDT = 04:00 UTC on March 11.
        let fixed_utc = fixed.naive_utc();
        let civil_utc = civil.naive_utc();
        assert_eq!(
            civil_utc, fixed_utc,
            "civil time must reference the same UTC instant as the fixed-offset clock"
        );
    }

    /// Fall-back: starting from 2024-11-03 00:00 EDT (-04:00), steps 1 and 2
    /// both map to civil hour 01:00 — first as 01:00 EDT (before the transition)
    /// and then as 01:00 EST (after the transition). These are distinct UTC
    /// instants.
    #[test]
    fn civil_time_repeats_fall_back_hour() {
        let start = FixedOffset::west_opt(4 * 3600)
            .expect("valid offset")
            .with_ymd_and_hms(2024, 11, 3, 0, 0, 0)
            .single()
            .expect("valid start");

        let mut clock = SimClock::new(start, Duration::hours(1), Duration::hours(4));
        clock.civil_tz = Some(New_York);

        // Step 0: civil = 00:00 EDT.
        let civil0 = clock.current_civil_time().expect("civil step 0");
        assert_eq!(civil0.hour(), 0);
        clock.next();

        // Step 1: civil = 01:00 EDT (first 01:00, before fall-back).
        let civil1 = clock.current_civil_time().expect("civil step 1");
        assert_eq!(civil1.hour(), 1);
        let utc1 = civil1.naive_utc();
        // At UTC 05:00 on Nov 3, America/New_York is still in EDT (UTC-4).
        // 01:00 EDT maps from UTC 05:00.
        clock.next();

        // Step 2: civil = 01:00 EST (second 01:00, after fall-back).
        let civil2 = clock.current_civil_time().expect("civil step 2");
        assert_eq!(civil2.hour(), 1);
        let utc2 = civil2.naive_utc();

        // Both are hour 1, but with different UTC times.
        assert_ne!(
            utc1, utc2,
            "fall-back repeated 01:00 hour must map to distinct UTC instants"
        );
        // Step 2 (UTC 06:00) is exactly 1 hour after step 1 (UTC 05:00),
        // even though both civil hours appear as 01:00.
        let diff_secs = (utc2 - utc1).num_seconds();
        assert!(
            diff_secs == 3600,
            "UTC gap between repeated 01:00 hours must be exactly 1 hour, got {diff_secs}s"
        );

        clock.next();
        // Step 3: civil = 02:00 EST.
        let civil3 = clock.current_civil_time().expect("civil step 3");
        assert_eq!(civil3.hour(), 2);
    }
}

#[cfg(feature = "dst")]
#[test]
fn spring_forward_day_of_year_at_boundary_step_uses_civil_ordinal() {
    let start = offset_west(5 * 3600)
        .with_ymd_and_hms(2024, 3, 10, 0, 0, 0)
        .single()
        .expect("valid spring-forward start");
    let mut manager = EnvironmentManager::new(
        sequential_weather(20.0, 24, -5.0),
        hourly_schedule(start),
        &minimal_building(),
        StdDuration::from_secs(3600),
        start,
        Some("America/New_York"),
    )
    .expect("manager");
    let mut clock = SimClock::new(start, Duration::hours(1), Duration::hours(24));

    // Step to step 23: wall-clock 23:00 EST on the fixed-offset clock.
    // Civil time via America/New_York: 23:00 EST = 04:00 UTC March 11
    // = 00:00 EDT March 11.  ordinal() on March 11 in leap year 2024 is 71
    // (31 Jan + 29 Feb + 11 = 71).  Without the DST-aware fix the raw
    // FixedOffset ordinal would be March 10 = 70.
    for _ in 0..23 {
        clock.next();
    }
    assert_eq!(clock.current_step(), 23);

    let env = manager.update(&clock, &[]).unwrap();

    assert_eq!(
        env.weather.day_of_year, 71.0,
        "at step 23 (civil time 00:00 EDT March 11), day_of_year must be 71 (March 11), \
         not 70 (March 10 fixed-offset); got {}",
        env.weather.day_of_year
    );
}

#[cfg(feature = "dst")]
#[test]
fn spring_forward_day_of_year_at_15min_resolution_all_steps() {
    let start = offset_west(5 * 3600)
        .with_ymd_and_hms(2024, 3, 10, 0, 0, 0)
        .single()
        .expect("valid spring-forward start");
    let mut manager = EnvironmentManager::new(
        sequential_weather(20.0, 96, -5.0),
        hourly_schedule(start),
        &minimal_building(),
        StdDuration::from_secs(900), // 15 minutes
        start,
        Some("America/New_York"),
    )
    .expect("manager");
    let mut clock = SimClock::new(start, Duration::minutes(15), Duration::hours(24));

    // 96 steps at 15-min resolution = 24 physical hours.
    // Fixed-offset clock: all 96 steps are calendar March 10 (ordinal 70).
    // Civil time: steps 0-91 are March 10 (ordinal 70), steps 92-95 are
    // March 11 (ordinal 71) because 23:00 EST = 00:00 EDT March 11.
    // ordinal() on March 10 in leap year 2024 = 70 (31+29+10).
    // ordinal() on March 11 in leap year 2024 = 71 (31+29+11).
    for step in 0..96 {
        let env = manager.update(&clock, &[]).unwrap();
        let doy = env.weather.day_of_year;
        if step < 92 {
            assert_eq!(
                doy, 70.0,
                "step {step}: before civil midnight, day_of_year must be 70 (March 10); got {doy}"
            );
        } else {
            assert_eq!(
                doy, 71.0,
                "step {step}: after civil midnight, day_of_year must be 71 (March 11); got {doy}"
            );
        }
        // Advance for next step (skip on last iteration).
        if step < 95 {
            clock.next();
        }
    }
}
