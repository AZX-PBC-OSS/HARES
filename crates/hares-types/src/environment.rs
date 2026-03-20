//! Outdoor and zone environment state types.
//!
//! Defines `ZoneState` (temperatures, humidity from the envelope) and
//! `OutdoorConditions` (weather data) shared between envelope and equipment.

use std::fmt;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::DomainUpdate;

/// Stable zone identifier.
#[derive(
    Hash, Eq, PartialEq, Copy, Clone, Debug, Default, Ord, PartialOrd, Serialize, Deserialize,
)]
pub struct ZoneId(pub u16);

impl fmt::Display for ZoneId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<u16> for ZoneId {
    fn from(v: u16) -> Self {
        Self(v)
    }
}

impl From<ZoneId> for u16 {
    fn from(id: ZoneId) -> Self {
        id.0
    }
}

/// Zone-level state used by equipment and envelope models each timestep.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ZoneState {
    pub id: ZoneId,
    pub temperature_c: f64,
    pub humidity_ratio: f64,
    pub relative_humidity: f64,
    pub wet_bulb_c: f64,
    pub volume_m3: f64,
}

/// Solar irradiance components mapped to an envelope surface.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SurfaceIrradiance {
    pub surface_id: u32,
    pub direct_w_m2: f64,
    pub diffuse_w_m2: f64,
    pub reflected_w_m2: f64,
    /// Angle of incidence between beam radiation and the surface normal [rad].
    ///
    /// Used by the thermal solver to apply window IAM corrections.
    /// Defaults to 0.0 (normal incidence) when not set or deserialized from
    /// older data that lacks this field.
    #[serde(default)]
    pub angle_of_incidence_rad: f64,
}

/// Weather boundary state for the current timestep.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct WeatherState {
    pub outdoor_temp_c: f64,
    pub outdoor_humidity_ratio: f64,
    /// Outdoor wet-bulb temperature [°C], derived from dry-bulb, humidity ratio, and pressure.
    pub outdoor_wet_bulb_c: f64,
    /// Outdoor moist-air enthalpy [J/kg dry-air], derived from dry-bulb and humidity ratio.
    pub outdoor_enthalpy_j_kg: f64,
    pub wind_speed_m_s: f64,
    pub wind_dir_deg: f64,
    pub ground_temp_c: f64,
    pub sky_temp_c: f64,
    pub pressure_kpa: f64,
    pub solar_irradiance: Vec<SurfaceIrradiance>,
    /// Horizontal global irradiance (W/m2) from weather station / TMY data.
    #[serde(default)]
    pub ghi_w_m2: f64,
    /// Direct normal irradiance (W/m2) from weather station / TMY data.
    #[serde(default)]
    pub dni_w_m2: f64,
    /// Diffuse horizontal irradiance (W/m2) from weather station / TMY data.
    #[serde(default)]
    pub dhi_w_m2: f64,
    /// Solar altitude angle above the horizon [degrees].
    /// Positive when sun is up, negative when below horizon.
    /// Computed from `solar_position()` each timestep.
    #[serde(default)]
    pub solar_altitude_deg: f64,
    /// Municipal cold-water supply temperature [°C], computed each step using
    /// the Burch-Christensen (2007) model with annual climate statistics.
    /// Defaults to 15.0 when environment data is unavailable.
    #[serde(default = "default_mains_temp_c")]
    pub mains_temp_c: f64,
    /// Liquid precipitation depth for this timestep [m].
    /// Parsed from EPW field 33 (Liquid Precipitation Depth).
    /// Zero when data is unavailable.
    #[serde(default)]
    pub rainfall_m: f64,
}

fn default_mains_temp_c() -> f64 {
    15.0
}

impl WeatherState {
    /// Atmospheric pressure in Pascals.
    #[inline]
    pub fn pressure_pa(&self) -> f64 {
        self.pressure_kpa * 1000.0
    }
}

/// Electrical grid state exposed to equipment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GridState {
    pub voltage_pu: f64,
    pub frequency_hz: f64,
}

/// Complete runtime environment state fed into physics calls.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentState {
    pub zones: Vec<ZoneState>,
    pub weather: WeatherState,
    pub grid: GridState,
    pub custom_domains: Vec<DomainUpdate>,
    pub current_time: DateTime<Utc>,
    /// Simulation timestep.
    ///
    /// Stored as `chrono::Duration` because `current_time` arithmetic (scheduling,
    /// schedule index lookup) requires it. All physics/equipment APIs accept
    /// `std::time::Duration` instead; use [`EnvironmentState::time_step_secs`] when
    /// you only need the scalar duration.
    #[serde(
        serialize_with = "serialize_duration_millis",
        deserialize_with = "deserialize_duration_millis"
    )]
    pub time_res: Duration,
}

impl EnvironmentState {
    /// Timestep duration in seconds as a float.
    ///
    /// Equivalent to `time_res.num_milliseconds() as f64 / 1000.0`. Prefer this over
    /// reaching into `time_res` directly so call sites stay insulated from the
    /// `chrono::Duration` API.
    #[inline]
    pub fn time_step_secs(&self) -> f64 {
        self.time_res.num_milliseconds() as f64 / 1000.0
    }
}

/// Serialize `chrono::Duration` as integer milliseconds.
///
/// Note: sub-millisecond precision is truncated. This is acceptable for
/// simulation timesteps (typically seconds to minutes).
fn serialize_duration_millis<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_i64(duration.num_milliseconds())
}

fn deserialize_duration_millis<'de, D>(deserializer: D) -> Result<Duration, D::Error>
where
    D: Deserializer<'de>,
{
    let millis = i64::deserialize(deserializer)?;
    Ok(Duration::milliseconds(millis))
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn environment_state_round_trips_through_json() {
        let state = EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: 21.5,
                humidity_ratio: 0.008,
                relative_humidity: 0.45,
                wet_bulb_c: 14.2,
                volume_m3: 240.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 5.0,
                outdoor_humidity_ratio: 0.004,
                outdoor_wet_bulb_c: 3.5,
                outdoor_enthalpy_j_kg: 15_000.0,
                wind_speed_m_s: 3.2,
                wind_dir_deg: 180.0,
                ground_temp_c: 10.5,
                sky_temp_c: -2.0,
                pressure_kpa: 101.3,
                solar_irradiance: vec![SurfaceIrradiance {
                    surface_id: 7,
                    direct_w_m2: 300.0,
                    diffuse_w_m2: 120.0,
                    reflected_w_m2: 50.0,
                    angle_of_incidence_rad: 0.0,
                }],
                ghi_w_m2: 400.0,
                dni_w_m2: 300.0,
                dhi_w_m2: 100.0,
                solar_altitude_deg: 30.0,
                mains_temp_c: 15.0,
                rainfall_m: 0.0,
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
            },
            custom_domains: vec![DomainUpdate {
                domain_id: crate::DomainId(9),
                zone_temperatures_c: vec![(ZoneId(1), 21.0)],
                custom_payload: Some(vec![1.0, 2.0, 3.0]),
            }],
            current_time: Utc
                .with_ymd_and_hms(2026, 3, 18, 12, 0, 0)
                .single()
                .expect("valid UTC timestamp"),
            time_res: Duration::minutes(5),
        };

        let json = serde_json::to_string(&state).expect("serialize environment state");
        let decoded: EnvironmentState =
            serde_json::from_str(&json).expect("deserialize environment state");
        assert_eq!(decoded, state);
    }
}
