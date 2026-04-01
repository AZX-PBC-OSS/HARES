//! Shared types used across HARES crates.
//!
//! This crate is the leaf of the dependency DAG. It defines the cross-cutting
//! types that multiple sibling crates need without introducing circular deps:
//! port contributions, environment state, control signals, equipment descriptors.

pub mod control_signal;
pub mod domain_solver;
pub mod environment;
pub mod equipment;
pub mod error;
pub mod fluid;
pub mod ports;
pub mod schedule;
pub mod telemetry;
pub mod telemetry_keys;
pub mod text;

pub use control_signal::*;
pub use domain_solver::*;
pub use environment::*;
pub use equipment::*;
pub use error::*;
pub use fluid::*;

/// Crate-level result alias.
pub type Result<T> = std::result::Result<T, HaresError>;
pub use ports::*;
pub use schedule::*;
pub use telemetry::*;
pub use text::{normalize_ascii, parse_trimmed_f64};

#[cfg(test)]
pub mod test_utils {
    use chrono::{FixedOffset, TimeZone};

    use crate::{EnvironmentState, GridState, SurfaceIrradiance, WeatherState, ZoneId, ZoneState};

    /// Returns a sensible-default `EnvironmentState` with one zone at 21 °C.
    pub fn default_env() -> EnvironmentState {
        env_with_zone_temp(21.0)
    }

    /// Returns an `EnvironmentState` with one zone at `temp_c` and otherwise
    /// typical indoor/outdoor conditions.
    pub fn env_with_zone_temp(temp_c: f64) -> EnvironmentState {
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: temp_c,
                humidity_ratio: 0.008,
                relative_humidity: 0.45,
                wet_bulb_c: 14.0,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 10.0,
                outdoor_humidity_ratio: 0.005,
                outdoor_wet_bulb_c: 7.0,
                outdoor_enthalpy_j_kg: 22_800.0,
                wind_speed_m_s: 2.0,
                wind_dir_deg: 0.0,
                ground_temp_c: 12.0,
                sky_temp_c: 8.0,
                pressure_kpa: 101.325,
                solar_irradiance: vec![SurfaceIrradiance {
                    surface_id: 1,
                    direct_w_m2: 0.0,
                    diffuse_w_m2: 0.0,
                    reflected_w_m2: 0.0,
                    angle_of_incidence_rad: 0.0,
                }],
                ghi_w_m2: 0.0,
                dni_w_m2: 0.0,
                dhi_w_m2: 0.0,
                solar_altitude_deg: 0.0,
                solar_azimuth_deg: 180.0,
                mains_temp_c: 10.0,
                rainfall_m: 0.0,
                ground_albedo: 0.2,
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
            },
            custom_domains: vec![],
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: std::collections::HashMap::new(),
            current_time: FixedOffset::east_opt(0)
                .expect("offset")
                .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                .single()
                .expect("valid timestamp"),
            time_res: chrono::Duration::seconds(60),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }
}
