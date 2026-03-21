/// Shared test helpers for hares-equipment integration tests.
///
/// Provides factory functions for EnvironmentState, EquipmentConfig, and PortSlots
/// with sensible defaults and chainable setters.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{FixedOffset, TimeZone};
use hares_equipment::config::ConfigValue;
use hares_equipment::{Equipment, EquipmentConfig, Telemetry};
use hares_types::{
    EnvironmentState, GridState, PortSlots, SurfaceIrradiance, WeatherState, ZoneId, ZoneState,
};

/// Build an EnvironmentState with one zone at the given temperature.
pub fn env_with_zone_temp(zone_temp_c: f64) -> EnvironmentState {
    env_builder().zone_temp(zone_temp_c).build()
}

/// Build an EnvironmentState with one zone at 21 C and mild outdoor conditions.
pub fn default_env() -> EnvironmentState {
    env_builder().build()
}

/// Builder for EnvironmentState with chainable setters.
pub struct EnvBuilder {
    zone_temp_c: f64,
    outdoor_temp_c: f64,
    humidity_ratio: f64,
    wind_speed_m_s: f64,
    voltage_pu: f64,
    ghi_w_m2: f64,
    surfaces: Vec<SurfaceIrradiance>,
    zone_volume_m3: f64,
}

impl Default for EnvBuilder {
    fn default() -> Self {
        Self {
            zone_temp_c: 21.0,
            outdoor_temp_c: 10.0,
            humidity_ratio: 0.008,
            wind_speed_m_s: 2.0,
            voltage_pu: 1.0,
            ghi_w_m2: 0.0,
            surfaces: vec![],
            zone_volume_m3: 200.0,
        }
    }
}

impl EnvBuilder {
    pub fn zone_temp(mut self, t: f64) -> Self {
        self.zone_temp_c = t;
        self
    }

    pub fn outdoor_temp(mut self, t: f64) -> Self {
        self.outdoor_temp_c = t;
        self
    }

    pub fn voltage(mut self, v: f64) -> Self {
        self.voltage_pu = v;
        self
    }

    pub fn wind_speed(mut self, ws: f64) -> Self {
        self.wind_speed_m_s = ws;
        self
    }

    pub fn ghi(mut self, ghi: f64) -> Self {
        self.ghi_w_m2 = ghi;
        self
    }

    pub fn surfaces(mut self, s: Vec<SurfaceIrradiance>) -> Self {
        self.surfaces = s;
        self
    }

    pub fn build(self) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: self.zone_temp_c,
                humidity_ratio: self.humidity_ratio,
                relative_humidity: 0.45,
                // Approximate wet-bulb: always ≤ dry-bulb to stay physically valid.
                wet_bulb_c: (self.zone_temp_c - 3.0).min(self.zone_temp_c),
                volume_m3: self.zone_volume_m3,
            }],
            weather: WeatherState {
                outdoor_temp_c: self.outdoor_temp_c,
                outdoor_humidity_ratio: 0.005,
                outdoor_wet_bulb_c: 7.0,
                outdoor_enthalpy_j_kg: 22_800.0,
                wind_speed_m_s: self.wind_speed_m_s,
                wind_dir_deg: 0.0,
                ground_temp_c: 12.0,
                sky_temp_c: 8.0,
                pressure_kpa: 101.325,
                solar_irradiance: self.surfaces,
                ghi_w_m2: self.ghi_w_m2,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
                solar_altitude_deg: 0.0,
                mains_temp_c: 15.0,
                rainfall_m: 0.0,
            },
            grid: GridState {
                voltage_pu: self.voltage_pu,
                frequency_hz: 60.0,
            },
            custom_domains: vec![],
            current_time: FixedOffset::east_opt(0)
                .expect("UTC offset")
                .with_ymd_and_hms(2026, 6, 21, 12, 0, 0)
                .single()
                .expect("valid timestamp"),
            time_res: chrono::Duration::seconds(60),
        }
    }
}

pub fn env_builder() -> EnvBuilder {
    EnvBuilder::default()
}

/// Build a config with the given entries.
pub fn config(name: &str, ochre_class: &str, entries: &[(&str, f64)]) -> EquipmentConfig {
    let mut raw = HashMap::new();
    for &(key, value) in entries {
        raw.insert(key.to_string(), ConfigValue::Float(value));
    }
    EquipmentConfig {
        name: name.to_string(),
        ochre_class: ochre_class.to_string(),
        raw_config: raw,
    }
}

/// Build a config with string and float entries.
pub fn config_mixed(
    name: &str,
    ochre_class: &str,
    floats: &[(&str, f64)],
    strings: &[(&str, &str)],
) -> EquipmentConfig {
    let mut raw: HashMap<String, ConfigValue> = HashMap::new();
    for &(key, value) in floats {
        raw.insert(key.to_string(), ConfigValue::Float(value));
    }
    for &(key, value) in strings {
        raw.insert(key.to_string(), ConfigValue::Text(value.to_string()));
    }
    EquipmentConfig {
        name: name.to_string(),
        ochre_class: ochre_class.to_string(),
        raw_config: raw,
    }
}

/// Step equipment for N timesteps, returning the final telemetry snapshot.
pub fn step_n(
    equipment: &mut dyn Equipment,
    env: &EnvironmentState,
    ports: &mut PortSlots,
    dt: Duration,
    n: usize,
) -> Telemetry {
    for _ in 0..n {
        ports.zero();
        equipment.update_control(env);
        equipment.step(env, dt, ports).unwrap();
    }
    equipment.telemetry().clone()
}

/// Assert a value is within [min, max].
pub fn assert_bounded(value: f64, min: f64, max: f64, label: &str) {
    assert!(
        value >= min && value <= max,
        "{label}: {value} not in [{min}, {max}]"
    );
}

/// Assert approximate equality within tolerance.
pub fn approx_eq(actual: f64, expected: f64, tol: f64) {
    assert!(
        (actual - expected).abs() <= tol,
        "actual={actual}, expected={expected}, tol={tol}"
    );
}
