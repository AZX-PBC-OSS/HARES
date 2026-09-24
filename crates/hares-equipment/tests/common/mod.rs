/// Shared test helpers for hares-equipment integration tests.
///
/// Provides factory functions for EnvironmentState, EquipmentConfig, and PortSlots
/// with sensible defaults and chainable setters.
use chrono::{FixedOffset, TimeZone};
use hares_types::{
    EnvironmentState, GridState, SurfaceIrradiance, WeatherState, ZoneId, ZoneState,
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

    pub fn build(self) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: self.zone_temp_c,
                humidity_ratio: self.humidity_ratio,
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
                solar_azimuth_deg: 180.0,
                mains_temp_c: 15.0,
                rainfall_m: 0.0,
                ground_albedo: 0.2,
                ground_t_mean_c: 10.0,
                ground_t_amplitude_c: 0.0,
                ground_phase_day: 35.0,
                day_of_year: 1.0,
            },
            grid: GridState {
                voltage_pu: self.voltage_pu,
                frequency_hz: 60.0,
                island_bus_voltage_pu: None,
            },
            custom_domains: vec![],
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: Default::default(),
            current_time: FixedOffset::east_opt(0)
                .expect("UTC offset")
                .with_ymd_and_hms(2026, 6, 21, 12, 0, 0)
                .single()
                .expect("valid timestamp"),
            time_res: chrono::Duration::seconds(60),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }
}

pub fn env_builder() -> EnvBuilder {
    EnvBuilder::default()
}
