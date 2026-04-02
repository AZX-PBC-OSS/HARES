//! Full-pipeline weather integration tests.
//!
//! Verifies the complete chain: synthetic WeatherTimeSeries → resample →
//! EnvironmentManager → update() → WeatherState with derived psychrometrics
//! and per-surface irradiance.

use std::collections::HashMap;
use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, FixedOffset, TimeZone};
use hares_core::{EnvironmentManager, SimClock};
use hares_io::hpxml::building::XmlNode;
use hares_io::hpxml::{Boundary, BoundaryType, Site, Window, Zone, ZoneType};
use hares_io::schedule::ColumnAggregation;
use hares_io::{ScheduleTimeSeries, WeatherMeta, WeatherTimeSeries};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Denver-ish fixed offset: UTC-7.
fn denver_offset() -> FixedOffset {
    FixedOffset::west_opt(7 * 3600).expect("offset")
}

/// Timestamp for a given hour on 2024-07-15 in Denver local time.
fn ts(hour: u32) -> DateTime<FixedOffset> {
    denver_offset()
        .with_ymd_and_hms(2024, 7, 15, hour, 0, 0)
        .single()
        .expect("time")
}

/// Build a 24-hour synthetic weather series with physically realistic values.
fn synthetic_weather() -> WeatherTimeSeries {
    let n = 24;
    let mut dry_bulb_c = Vec::with_capacity(n);
    let mut dew_point_c = Vec::with_capacity(n);
    let mut rel_humidity_pct = Vec::with_capacity(n);
    let mut pressure_kpa = Vec::with_capacity(n);
    let mut ghi = Vec::with_capacity(n);
    let mut dni = Vec::with_capacity(n);
    let mut dhi = Vec::with_capacity(n);
    let mut wind_speed = Vec::with_capacity(n);
    let mut wind_dir = Vec::with_capacity(n);
    let mut opaque_sky = Vec::with_capacity(n);
    let mut h_ir = Vec::with_capacity(n);
    let mut sky_temp = Vec::with_capacity(n);
    let mut ground_temp = Vec::with_capacity(n);
    let mut precip = Vec::with_capacity(n);

    for h in 0..n {
        let hour = h as f64;

        // Temperature sinusoid: min 5°C at hour 5, max 25°C at hour 15.
        let t = 15.0 + 10.0 * ((hour - 15.0) * std::f64::consts::PI / 10.0).cos();
        dry_bulb_c.push(t);

        // Constant dew point 4°C (always below dry bulb min of 5.0°C at h=5
        // where cos((5-15)*pi/10) = cos(-pi) = -1, so t=5).
        dew_point_c.push(4.0);

        // Approximate RH from dew point (rough Magnus formula)
        let rh = 100.0 * (17.27 * 4.0 / (237.3 + 4.0) - 17.27 * t / (237.3 + t)).exp();
        rel_humidity_pct.push(rh.clamp(5.0, 100.0));

        pressure_kpa.push(101.325);

        // GHI bell curve: 0 at night, peak 800 at solar noon (~hour 13 local for Denver summer).
        let solar_noon = 13.0;
        let day_half = 7.0; // sunrise ~6, sunset ~20
        let solar_angle = (hour - solar_noon) / day_half * std::f64::consts::PI;
        let ghi_val = if solar_angle.abs() < std::f64::consts::FRAC_PI_2 {
            800.0 * solar_angle.cos().max(0.0)
        } else {
            0.0
        };
        ghi.push(ghi_val);
        // DNI/DHI split assumes cos(zenith)=1, non-physical but exercises the
        // pipeline end-to-end. The Perez model handles it correctly regardless.
        dni.push(ghi_val * 0.7);
        dhi.push(ghi_val * 0.3);

        wind_speed.push(3.0);
        wind_dir.push(180.0);
        opaque_sky.push(3.0);
        h_ir.push(300.0);
        sky_temp.push(0.0);
        ground_temp.push(10.0);

        // F-2: non-zero precipitation at hour 10
        if h == 10 {
            precip.push(0.005);
        } else {
            precip.push(0.0);
        }
    }

    WeatherTimeSeries {
        meta: WeatherMeta {
            location: "Synthetic-Denver".to_string(),
            latitude: 40.0,
            longitude: -105.0,
            timezone_offset_h: -7.0,
            elevation_m: 1609.0,
            source_step_secs: 3600,
            midpoint_offset_secs: 0,
        },
        dry_bulb_c,
        dew_point_c,
        rel_humidity_pct,
        pressure_kpa,
        ghi_w_m2: ghi,
        dni_w_m2: dni,
        dhi_w_m2: dhi,
        wind_speed_m_s: wind_speed,
        wind_dir_deg: wind_dir,
        opaque_sky_cover: opaque_sky,
        horizontal_infrared_w_m2: h_ir,
        sky_temp_c: sky_temp,
        ground_temp_c: ground_temp,
        liquid_precip_m: precip,
        surface_albedo: None,
    }
}

/// Build a minimal 24-hour schedule (1 column, constant value).
fn minimal_schedule() -> ScheduleTimeSeries {
    let n = 24;
    let timestamps: Vec<DateTime<FixedOffset>> = (0..n).map(|h| ts(h as u32)).collect();
    let mut index = HashMap::new();
    index.insert("occupancy".to_string(), 0);
    ScheduleTimeSeries {
        timestamps,
        column_names: vec!["occupancy".to_string()],
        columns: vec![vec![1.0; n]],
        column_index: index,
        source_step_secs: 3600,
        column_aggregations: vec![ColumnAggregation::Mean],
    }
}

/// Build a minimal Building with south and north walls for solar surface testing.
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
            latitude_deg: Some(40.0),
            longitude_deg: Some(-105.0),
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
        boundaries: vec![
            Boundary {
                id: "south-wall".to_string(),
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
            },
            Boundary {
                id: "north-wall".to_string(),
                boundary_type: BoundaryType::Wall,
                area_m2: 20.0,
                azimuth_deg: Some(0.0),
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
            },
        ],
        windows: Vec::<Window>::new(),
        infiltration_ach50: None,
        infiltration_cfm50: None,
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
        details_xml,
    }
}

/// Helper: assert that a float is finite (not NaN or Inf).
fn assert_finite(val: f64, name: &str, step: u64) {
    assert!(
        val.is_finite(),
        "step {step}: {name} is not finite (got {val})"
    );
}

// ---------------------------------------------------------------------------
// Test 1: Full pipeline synthetic weather
// ---------------------------------------------------------------------------

#[test]
fn full_pipeline_synthetic_weather() {
    let weather = synthetic_weather();
    let schedule = minimal_schedule();
    let building = minimal_building();

    // EPW convention: start at hour 0:30 (midpoint of first hour-ending record).
    let start = ts(0) + Duration::minutes(30);
    let time_res = StdDuration::from_secs(300); // 5-minute steps (factor 12)
    let total_steps = 288u64; // 24h * 12 steps/h

    let mut mgr = EnvironmentManager::new(weather, schedule, &building, time_res, start, None)
        .expect("EnvironmentManager::new failed");

    let mut clock = SimClock::new(start, Duration::seconds(300), Duration::hours(24));

    let mut total_rainfall = 0.0_f64;
    let mut prev_temp: Option<f64> = None;
    let mut prev_enthalpy: Option<f64> = None;
    let mut ground_min = f64::INFINITY;
    let mut ground_max = f64::NEG_INFINITY;
    let mut air_min = f64::INFINITY;
    let mut air_max = f64::NEG_INFINITY;

    for _ in 0..total_steps {
        let env = mgr.update(&clock, &[]);
        let w = &env.weather;
        let step = clock.current_step();

        // Temperature in reasonable range
        assert_finite(w.outdoor_temp_c, "outdoor_temp_c", step);
        assert!(
            (-10.0..=35.0).contains(&w.outdoor_temp_c),
            "step {step}: outdoor_temp_c={} out of [-10, 35]",
            w.outdoor_temp_c
        );

        // Humidity ratio non-negative
        assert!(
            w.outdoor_humidity_ratio >= 0.0,
            "step {step}: humidity_ratio={} < 0",
            w.outdoor_humidity_ratio
        );

        // F-1: Wet bulb strictly below dry bulb, and bounded below
        assert!(
            w.outdoor_wet_bulb_c < w.outdoor_temp_c - 0.1,
            "step {step}: wet_bulb={} >= dry_bulb={} - 0.1",
            w.outdoor_wet_bulb_c,
            w.outdoor_temp_c
        );
        assert!(
            w.outdoor_wet_bulb_c > -10.0,
            "step {step}: wet_bulb={} below -10 lower bound",
            w.outdoor_wet_bulb_c
        );

        // Enthalpy finite
        assert_finite(w.outdoor_enthalpy_j_kg, "outdoor_enthalpy_j_kg", step);

        // F-12: Enthalpy > 0 for T > 0°C (all test temps are > 0)
        assert!(
            w.outdoor_enthalpy_j_kg > 0.0,
            "step {step}: enthalpy={} <= 0 but temp={} > 0°C",
            w.outdoor_enthalpy_j_kg,
            w.outdoor_temp_c
        );

        // Humidity ratio positive (from merged psychrometric_chain_consistent, F-11)
        assert!(
            w.outdoor_humidity_ratio > 0.0,
            "step {step}: humidity_ratio={} <= 0",
            w.outdoor_humidity_ratio
        );

        // Pressure in range
        assert!(
            (95.0..=110.0).contains(&w.pressure_kpa),
            "step {step}: pressure_kpa={} out of [95, 110]",
            w.pressure_kpa
        );

        // Solar components non-negative
        assert!(w.ghi_w_m2 >= -0.01, "step {step}: ghi={} < 0", w.ghi_w_m2);
        assert!(w.dni_w_m2 >= -0.01, "step {step}: dni={} < 0", w.dni_w_m2);
        assert!(w.dhi_w_m2 >= -0.01, "step {step}: dhi={} < 0", w.dhi_w_m2);

        // F-3: GHI >= DHI
        assert!(
            w.ghi_w_m2 >= w.dhi_w_m2 - 0.01,
            "step {step}: ghi={} < dhi={}",
            w.ghi_w_m2,
            w.dhi_w_m2
        );

        // F-4: Solar-altitude-based night check
        if w.solar_altitude_deg <= 0.0 {
            for surf in &w.solar_irradiance {
                let total = surf.direct_w_m2 + surf.diffuse_w_m2 + surf.reflected_w_m2;
                assert!(
                    total < 5.0,
                    "step {step}: night but surface {} irradiance {total}",
                    surf.surface_id,
                );
            }
        }

        // Per-surface irradiance components non-negative
        for si in &w.solar_irradiance {
            assert!(
                si.direct_w_m2 >= -0.01,
                "step {step}: surface {} direct={} < 0",
                si.surface_id,
                si.direct_w_m2
            );
            assert!(
                si.diffuse_w_m2 >= -0.01,
                "step {step}: surface {} diffuse={} < 0",
                si.surface_id,
                si.diffuse_w_m2
            );
            assert!(
                si.reflected_w_m2 >= -0.01,
                "step {step}: surface {} reflected={} < 0",
                si.surface_id,
                si.reflected_w_m2
            );
        }

        // F-8: Mains water temperature (tightened to [5, 25])
        assert_finite(w.mains_temp_c, "mains_temp_c", step);
        assert!(
            (5.0..=25.0).contains(&w.mains_temp_c),
            "step {step}: mains_temp_c={} out of [5, 25]",
            w.mains_temp_c
        );

        // Ground temperature
        assert_finite(w.ground_temp_c, "ground_temp_c", step);

        // Ground albedo in [0, 1]
        assert!(
            (0.0..=1.0).contains(&w.ground_albedo),
            "step {step}: ground_albedo={} out of [0, 1]",
            w.ground_albedo
        );

        // Rainfall non-negative
        assert!(
            w.rainfall_m >= 0.0,
            "step {step}: rainfall_m={} < 0",
            w.rainfall_m
        );

        // No NaN/Inf in any field
        assert_finite(w.wind_speed_m_s, "wind_speed_m_s", step);
        assert_finite(w.sky_temp_c, "sky_temp_c", step);
        assert_finite(w.solar_altitude_deg, "solar_altitude_deg", step);

        // F-5: Enthalpy increases with temperature
        if let (Some(pt), Some(pe)) = (prev_temp, prev_enthalpy) {
            if w.outdoor_temp_c > pt + 0.5 {
                assert!(
                    w.outdoor_enthalpy_j_kg > pe,
                    "step {step}: temp increased by {:.2}°C but enthalpy did not increase \
                     (prev_enthalpy={pe}, cur_enthalpy={})",
                    w.outdoor_temp_c - pt,
                    w.outdoor_enthalpy_j_kg
                );
            }
        }
        prev_temp = Some(w.outdoor_temp_c);
        prev_enthalpy = Some(w.outdoor_enthalpy_j_kg);

        // Accumulate for post-loop assertions
        total_rainfall += w.rainfall_m;
        ground_min = ground_min.min(w.ground_temp_c);
        ground_max = ground_max.max(w.ground_temp_c);
        air_min = air_min.min(w.outdoor_temp_c);
        air_max = air_max.max(w.outdoor_temp_c);

        let _ = clock.next();
    }

    // F-2: Total precipitation equals 0.005 (single hour 10 event, distributed across steps)
    assert!(
        (total_rainfall - 0.005).abs() < 1e-10,
        "total rainfall={total_rainfall}, expected 0.005"
    );

    // F-9: Ground temp varies less than air temp
    let ground_range = ground_max - ground_min;
    let air_range = air_max - air_min;
    assert!(
        ground_range < air_range,
        "ground temp range ({ground_range:.2}) >= air temp range ({air_range:.2})"
    );
}

// ---------------------------------------------------------------------------
// Test 2: Solar irradiance physical bounds
// ---------------------------------------------------------------------------

#[test]
fn solar_irradiance_physical_bounds() {
    let weather = synthetic_weather();
    let schedule = minimal_schedule();
    let building = minimal_building();

    let start = ts(0) + Duration::minutes(30);
    let time_res = StdDuration::from_secs(300);
    let total_steps = 288u64;

    let mut mgr = EnvironmentManager::new(weather, schedule, &building, time_res, start, None)
        .expect("EnvironmentManager::new failed");

    let mut clock = SimClock::new(start, Duration::seconds(300), Duration::hours(24));

    for _ in 0..total_steps {
        let env = mgr.update(&clock, &[]);
        let w = &env.weather;
        let step = clock.current_step();

        let is_daytime = w.ghi_w_m2 > 1.0;

        // F-3: GHI >= DHI
        assert!(
            w.ghi_w_m2 >= w.dhi_w_m2 - 0.01,
            "step {step}: ghi={} < dhi={}",
            w.ghi_w_m2,
            w.dhi_w_m2
        );

        // F-4: Solar-altitude-based night check
        if w.solar_altitude_deg <= 0.0 {
            for surf in &w.solar_irradiance {
                let total = surf.direct_w_m2 + surf.diffuse_w_m2 + surf.reflected_w_m2;
                assert!(
                    total < 5.0,
                    "step {step}: night but surface {} irradiance {total}",
                    surf.surface_id,
                );
            }
        }

        if is_daytime {
            // Per-surface total <= extraterrestrial upper bound (1400 W/m²).
            for si in &w.solar_irradiance {
                let total = si.direct_w_m2 + si.diffuse_w_m2 + si.reflected_w_m2;
                assert!(
                    total <= 1400.0,
                    "step {step}: surface {} total irradiance {total} > 1400 W/m²",
                    si.surface_id
                );
                assert!(si.direct_w_m2 >= -0.01, "step {step}: direct < 0");
                assert!(si.diffuse_w_m2 >= -0.01, "step {step}: diffuse < 0");
                assert!(si.reflected_w_m2 >= -0.01, "step {step}: reflected < 0");
            }
        } else {
            // Nighttime: all solar components should be zero or very small.
            for si in &w.solar_irradiance {
                let total = si.direct_w_m2 + si.diffuse_w_m2 + si.reflected_w_m2;
                assert!(
                    total < 5.0,
                    "step {step}: surface {} nighttime total irradiance {total} > 5 W/m²",
                    si.surface_id
                );
            }
        }

        let _ = clock.next();
    }
}

// ---------------------------------------------------------------------------
// Test 3: Resampled weather produces smooth environment
// ---------------------------------------------------------------------------

#[test]
fn resampled_weather_produces_smooth_environment() {
    let weather = synthetic_weather();
    let schedule = minimal_schedule();
    let building = minimal_building();

    let start = ts(0);
    // 60-second resolution (factor 60 from 3600s source)
    let time_res = StdDuration::from_secs(60);
    let total_steps = 1440u64; // 24h * 60 steps/h

    let mut mgr = EnvironmentManager::new(weather, schedule, &building, time_res, start, None)
        .expect("EnvironmentManager::new failed");

    let mut clock = SimClock::new(start, Duration::seconds(60), Duration::hours(24));

    let mut prev_temp: Option<f64> = None;

    for _ in 0..total_steps {
        let env = mgr.update(&clock, &[]);
        let w = &env.weather;
        let step = clock.current_step();

        // No NaN/Inf
        assert_finite(w.outdoor_temp_c, "outdoor_temp_c", step);
        assert_finite(w.outdoor_humidity_ratio, "humidity_ratio", step);
        assert_finite(w.outdoor_wet_bulb_c, "wet_bulb_c", step);
        assert_finite(w.outdoor_enthalpy_j_kg, "enthalpy_j_kg", step);
        assert_finite(w.pressure_kpa, "pressure_kpa", step);
        assert_finite(w.ghi_w_m2, "ghi_w_m2", step);
        assert_finite(w.dni_w_m2, "dni_w_m2", step);
        assert_finite(w.dhi_w_m2, "dhi_w_m2", step);
        assert_finite(w.wind_speed_m_s, "wind_speed_m_s", step);
        assert_finite(w.sky_temp_c, "sky_temp_c", step);
        assert_finite(w.ground_temp_c, "ground_temp_c", step);
        assert_finite(w.mains_temp_c, "mains_temp_c", step);

        // F-6: Smoothness tightened to 0.15°C (PCHIP interpolation).
        if let Some(prev) = prev_temp {
            let diff = (w.outdoor_temp_c - prev).abs();
            assert!(
                diff < 0.15,
                "step {step}: temp jump of {diff:.3}°C (prev={prev:.2}, cur={:.2})",
                w.outdoor_temp_c
            );
        }
        prev_temp = Some(w.outdoor_temp_c);

        let _ = clock.next();
    }
}
