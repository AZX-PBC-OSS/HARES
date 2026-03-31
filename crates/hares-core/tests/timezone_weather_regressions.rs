use std::collections::HashMap;
use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, FixedOffset, TimeZone};
use hares_core::{EnvironmentManager, SimClock};
use hares_io::hpxml::building::XmlNode;
use hares_io::hpxml::{Boundary, BoundaryType, Site, Window, Zone, ZoneType};
use hares_io::schedule::ColumnAggregation;
use hares_io::{ScheduleTimeSeries, WeatherMeta, WeatherTimeSeries};
use hares_types::SCHEDULE_DOMAIN_ID;

#[cfg(feature = "dst")]
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
        }],
        windows: Vec::<Window>::new(),
        infiltration_ach50: None,
        infiltration_cfm50: None,
        infiltration_ela_cm2: None,
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
            source_step_secs: 3600,
            midpoint_offset_secs: 0,
        },
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

#[test]
fn fixed_offset_schedule_does_not_apply_dst_without_civil_timezone() {
    let start = offset_west(5 * 3600)
        .with_ymd_and_hms(2024, 3, 10, 0, 0, 0)
        .single()
        .expect("valid spring-forward start");
    let mut manager = EnvironmentManager::new(
        sequential_weather(100.0, 24, -5.0),
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
        let env = manager.update(&clock, &[]);
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
        vec![100.0, 101.0, 102.0, 103.0],
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
        matches!(err, hares_core::environment::EnvironmentManagerError::DstNotEnabled),
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
        sequential_weather(200.0, 24, -5.0),
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
        let env = manager.update(&clock, &[]);
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
        vec![200.0, 201.0, 202.0, 203.0, 204.0],
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
        sequential_weather(300.0, 24, -4.0),
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
        let env = manager.update(&clock, &[]);
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
        vec![300.0, 301.0, 302.0, 303.0, 304.0],
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
