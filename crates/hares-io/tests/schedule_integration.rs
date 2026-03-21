use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Datelike, Duration as ChronoDuration, FixedOffset, TimeZone, Timelike};
use hares_equipment::{Equipment, EquipmentConfig, config::ConfigValue};
use hares_equipment::{event_load::EventBasedLoad, scheduled_load::ScheduledLoad};
use hares_io::{EquipmentSpec, ScheduleTimeSeries, inject_schedule_into_specs};
use hares_types::{
    DomainUpdate, EndUse, EnvironmentState, FuelType, GridState, PortSlots, WeatherState, ZoneId,
    ZoneState, schedule_domain_id,
};
use serde_json::{Map, Value};
use tempfile::tempdir;

fn make_schedule(columns: &[(&str, &[f64])]) -> ScheduleTimeSeries {
    let rows = columns.first().map_or(0, |(_, col)| col.len());
    for (_, col) in columns {
        assert_eq!(col.len(), rows, "all columns must have same length");
    }

    let start: DateTime<chrono::FixedOffset> =
        DateTime::parse_from_rfc3339("2026-03-18T00:00:00+00:00").expect("valid timestamp");
    let timestamps = (0..rows)
        .map(|i| start + ChronoDuration::minutes(i as i64))
        .collect::<Vec<_>>();

    let mut column_names = Vec::with_capacity(columns.len());
    let mut column_index = HashMap::with_capacity(columns.len());
    let mut data = Vec::with_capacity(columns.len());
    for (idx, (name, col)) in columns.iter().enumerate() {
        column_names.push((*name).to_string());
        column_index.insert((*name).to_string(), idx);
        data.push(col.to_vec());
    }

    ScheduleTimeSeries {
        timestamps,
        column_names,
        columns: data,
        column_index,
        source_step_secs: 60,
        column_aggregations: vec![hares_io::ColumnAggregation::Mean; columns.len()],
    }
}

fn make_spec(name: &str, annual_kwh: f64) -> EquipmentSpec {
    let mut parameters = Map::new();
    parameters.insert("annual_electric_kwh".to_string(), Value::from(annual_kwh));
    EquipmentSpec {
        name: name.to_string(),
        fuel_type: FuelType::Electric,
        parameters,
        zip_params: None,
    }
}

fn json_value_to_config_value(value: &Value) -> Option<ConfigValue> {
    match value {
        Value::Number(n) => n.as_f64().map(ConfigValue::Float),
        Value::String(s) => Some(ConfigValue::Text(s.clone())),
        Value::Bool(b) => Some(ConfigValue::Bool(*b)),
        Value::Array(arr) => {
            let floats: Vec<f64> = arr.iter().filter_map(Value::as_f64).collect();
            (floats.len() == arr.len()).then_some(ConfigValue::FloatArray(floats))
        }
        _ => None,
    }
}

fn equipment_config_from_spec(spec: &EquipmentSpec) -> EquipmentConfig {
    let raw_config: HashMap<String, ConfigValue> = spec
        .parameters
        .iter()
        .filter_map(|(k, v)| json_value_to_config_value(v).map(|cv| (k.clone(), cv)))
        .collect();

    EquipmentConfig {
        name: spec.name.clone(),
        ochre_class: spec.name.clone(),
        raw_config,
    }
}

fn base_env(payload: Vec<f64>) -> EnvironmentState {
    EnvironmentState {
        zones: vec![ZoneState {
            id: ZoneId(1),
            temperature_c: 21.0,
            humidity_ratio: 0.008,
            relative_humidity: 0.45,
            wet_bulb_c: 14.0,
            volume_m3: 200.0,
        }],
        weather: WeatherState {
            outdoor_temp_c: 10.0,
            outdoor_humidity_ratio: 0.005,
            wind_speed_m_s: 2.0,
            wind_dir_deg: 0.0,
            ground_temp_c: 12.0,
            sky_temp_c: 8.0,
            pressure_kpa: 101.325,
            solar_irradiance: vec![],
            ..Default::default()
        },
        grid: GridState {
            voltage_pu: 1.0,
            frequency_hz: 60.0,
        },
        custom_domains: vec![DomainUpdate {
            domain_id: schedule_domain_id(),
            zone_temperatures_c: Vec::new(),
            custom_payload: Some(payload),
        }],
        current_time: FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 3, 18, 0, 0, 0)
            .single()
            .expect("valid timestamp"),
        time_res: ChronoDuration::minutes(1),
    }
}

fn payload_for_row(schedule: &ScheduleTimeSeries, row: usize) -> Vec<f64> {
    schedule.columns.iter().map(|col| col[row]).collect()
}

fn write_default_profile_csv(path: &std::path::Path) {
    let mut csv = String::from("Category,Name,OCHRE Name,OCHRE Element,Values\n");
    csv.push_str(
        "Schedules,Lighting,Indoor Lighting,weekday_fractions,\"0.2,0.2,0.2,0.2,0.2,0.2,0.3,0.5,0.8,1.0,1.0,0.9,0.8,0.7,0.7,0.8,0.9,1.0,0.9,0.8,0.7,0.5,0.3,0.2\"\n",
    );
    csv.push_str(
        "Schedules,Lighting,Indoor Lighting,weekend_fractions,\"0.3,0.3,0.3,0.3,0.3,0.3,0.4,0.6,0.9,1.1,1.1,1.0,0.9,0.8,0.8,0.9,1.0,1.1,1.0,0.9,0.8,0.6,0.4,0.3\"\n",
    );
    csv.push_str(
        "Schedules,Lighting,Indoor Lighting,month_multipliers,\"0.8,0.8,0.9,1.0,1.0,1.0,1.1,1.1,1.0,0.9,0.8,0.8\"\n",
    );
    std::fs::write(path.join("Default Schedule Parameters.csv"), csv)
        .expect("write default profile csv");
}

#[test]
fn io_injection_to_scheduled_load_step_column_source() {
    let mut schedule = make_schedule(&[("lighting_interior", &[0.2, 1.0, 0.4])]);
    let mut specs = vec![make_spec("Indoor Lighting", 876.0)];
    inject_schedule_into_specs(&mut specs, &mut schedule, None);

    assert_eq!(
        specs[0]
            .parameters
            .get("power_schedule_source")
            .and_then(Value::as_str),
        Some("column")
    );

    let col_idx = specs[0]
        .parameters
        .get("power_schedule_col")
        .and_then(Value::as_u64)
        .expect("power_schedule_col should be injected") as usize;

    let config = equipment_config_from_spec(&specs[0]);
    let mut eq = ScheduledLoad::new(config.clone(), EndUse::Lighting, "Indoor Lighting");

    let env = base_env(payload_for_row(&schedule, 0));
    eq.init(&config, &env).expect("scheduled load init should pass");

    let mut ports = PortSlots::from_declarations(eq.ports());
    eq.step(&env, Duration::from_secs(60), &mut ports)
        .expect("scheduled load step should pass");

    let expected_kw = schedule.columns[col_idx][0];
    assert!((ports.electrical.net_active_kw() - expected_kw).abs() < 1e-12);
}

#[test]
fn io_injection_to_scheduled_load_step_daily_profile_source() {
    let defaults_dir = tempdir().expect("temp defaults dir");
    write_default_profile_csv(defaults_dir.path());

    let mut schedule = make_schedule(&[("occupants", &[1.0, 1.0, 1.0])]);
    let mut specs = vec![make_spec("Indoor Lighting", 876.0)];
    inject_schedule_into_specs(&mut specs, &mut schedule, Some(defaults_dir.path()));

    assert_eq!(
        specs[0]
            .parameters
            .get("power_schedule_source")
            .and_then(Value::as_str),
        Some("daily_profile")
    );

    let config = equipment_config_from_spec(&specs[0]);
    let mut eq = ScheduledLoad::new(config.clone(), EndUse::Lighting, "Indoor Lighting");

    let env = base_env(payload_for_row(&schedule, 0));
    eq.init(&config, &env).expect("scheduled load init should pass");

    let mut ports = PortSlots::from_declarations(eq.ports());
    eq.step(&env, Duration::from_secs(60), &mut ports)
        .expect("scheduled load step should pass");

    let weekday = specs[0]
        .parameters
        .get("power_profile_weekday")
        .and_then(Value::as_array)
        .expect("weekday profile");
    let month = specs[0]
        .parameters
        .get("power_profile_month")
        .and_then(Value::as_array)
        .expect("month multipliers");
    let max_kw = specs[0]
        .parameters
        .get("power_profile_max_kw")
        .and_then(Value::as_f64)
        .expect("max kw");

    let hour = env.current_time.hour() as usize;
    let month_idx = env.current_time.month0() as usize;
    let expected_kw = weekday[hour].as_f64().expect("hourly fraction")
        * month[month_idx].as_f64().expect("month fraction")
        * max_kw;

    assert!((ports.electrical.net_active_kw() - expected_kw).abs() < 1e-12);
}

#[test]
fn io_injection_to_scheduled_load_step_constant_source() {
    let mut schedule = make_schedule(&[("occupants", &[1.0, 1.0])]);
    let mut specs = vec![make_spec("Indoor Lighting", 876.0)];
    inject_schedule_into_specs(&mut specs, &mut schedule, None);

    assert_eq!(
        specs[0]
            .parameters
            .get("power_schedule_source")
            .and_then(Value::as_str),
        Some("constant")
    );

    let expected_kw = specs[0]
        .parameters
        .get("power_constant_kw")
        .and_then(Value::as_f64)
        .expect("power_constant_kw should be injected");

    let config = equipment_config_from_spec(&specs[0]);
    let mut eq = ScheduledLoad::new(config.clone(), EndUse::Lighting, "Indoor Lighting");

    let env = base_env(payload_for_row(&schedule, 0));
    eq.init(&config, &env).expect("scheduled load init should pass");

    let mut ports = PortSlots::from_declarations(eq.ports());
    eq.step(&env, Duration::from_secs(60), &mut ports)
        .expect("scheduled load step should pass");

    assert!((ports.electrical.net_active_kw() - expected_kw).abs() < 1e-12);
}

#[test]
fn io_injection_to_event_load_step_uses_wrap_semantics() {
    let mut schedule = make_schedule(&[
        ("dummy_a", &[0.0, 0.0]),
        ("dummy_b", &[0.0, 0.0]),
        ("dishwasher", &[1.0, 1.0]),
    ]);
    let mut specs = vec![make_spec("Dishwasher", 0.0)];
    inject_schedule_into_specs(&mut specs, &mut schedule, None);

    let injected_col = specs[0]
        .parameters
        .get("event_window_schedule_col")
        .and_then(Value::as_u64)
        .expect("event_window_schedule_col should be injected") as usize;
    assert_eq!(injected_col, 2);

    let mut config = equipment_config_from_spec(&specs[0]);
    config
        .raw_config
        .insert("active_power_kw".to_string(), 1.5.into());
    config
        .raw_config
        .insert("active_duration_s".to_string(), 60.0.into());
    config
        .raw_config
        .insert("cooldown_duration_s".to_string(), 0.0.into());

    let mut eq = EventBasedLoad::new(config.clone());
    // payload len=2 while injected column index is 2. BoundaryPolicy::Wrap should map idx 2 -> 0.
    let env = base_env(vec![1.0, 1.0]);
    eq.init(&config, &env).expect("event load init should pass");

    let mut ports = PortSlots::from_declarations(eq.ports());
    eq.step(&env, Duration::from_secs(60), &mut ports)
        .expect("event load step should pass with wrap semantics");

    assert!(ports.electrical.load_power_kw > 0.0);
}

#[test]
fn missing_column_index_errors_at_init_not_step() {
    let mut schedule = make_schedule(&[("lighting_interior", &[0.5, 0.5])]);
    let mut specs = vec![make_spec("Indoor Lighting", 876.0)];
    inject_schedule_into_specs(&mut specs, &mut schedule, None);

    let mut config = equipment_config_from_spec(&specs[0]);
    config
        .raw_config
        .insert("power_schedule_source".to_string(), "column".into());
    config.raw_config.remove("power_schedule_col");

    let mut eq = ScheduledLoad::new(config.clone(), EndUse::Lighting, "Indoor Lighting");
    let env = base_env(payload_for_row(&schedule, 0));
    let err = eq
        .init(&config, &env)
        .expect_err("init should fail without power_schedule_col");

    assert!(
        err.to_string().contains("power_schedule_col"),
        "expected init-time missing-column-index error, got: {err}"
    );
}
