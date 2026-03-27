//! Outdoor and zone environment state types.
//!
//! Defines `ZoneState` (temperatures, humidity from the envelope) and
//! `OutdoorConditions` (weather data) shared between envelope and equipment.

use std::fmt;

use chrono::{DateTime, Duration, FixedOffset};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Offset between Kelvin and Celsius [K].
pub const KELVIN_OFFSET: f64 = 273.15;

/// Default ground albedo for bare ground (EnergyPlus default: 0.2).
///
/// Used as the fallback when weather data does not include measured surface
/// albedo (e.g. EPW and ResStock CSV formats). PSM3 files may provide
/// satellite-derived albedo via the `Surface Albedo` column.
pub const DEFAULT_GROUND_ALBEDO: f64 = 0.2;

use crate::DomainUpdate;

/// Price-like external signals consumed by higher-level controllers.
///
/// This is intentionally separate from `ControlSignal`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct PriceSignal {
    pub electricity_price: Option<f64>,
    pub export_price: Option<f64>,
    /// Grid carbon intensity in `kg CO₂e/kWh`.
    pub ghg_intensity: Option<f64>,
}

/// Electrical power summary from the prior timestep's solver.
///
/// Provides read-only observation of the building's electrical state
/// so actors can make informed decisions (e.g., BMS self-consumption
/// needs to know PV generation vs home load).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ElectricalSummary {
    /// Total PV generation [kW], positive = producing.
    pub pv_generation_kw: f64,
    /// Total non-dispatchable load [kW], positive = consuming.
    /// Excludes battery and EV (those are dispatchable).
    pub base_load_kw: f64,
    /// Net grid power [kW], positive = importing, negative = exporting.
    pub net_grid_kw: f64,
    /// Total battery power [kW], positive = charging, negative = discharging.
    pub battery_power_kw: f64,
    /// Total EV power [kW], positive = charging.
    pub ev_power_kw: f64,
}

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
    /// Solar azimuth angle [degrees, 0=N, 90=E, 180=S, 270=W].
    /// Computed from `solar_position()` each timestep.
    #[serde(default)]
    pub solar_azimuth_deg: f64,
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
    /// Ground surface albedo (reflectance) [dimensionless, 0–1].
    /// From PSM3 satellite data when available; defaults to 0.2 (bare ground).
    #[serde(default = "default_ground_albedo")]
    pub ground_albedo: f64,
}

fn default_mains_temp_c() -> f64 {
    15.0
}

fn default_ground_albedo() -> f64 {
    0.2
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
    /// Equipment telemetry snapshots from the previous timestep, keyed by
    /// equipment name. Populated by the dwelling before calling actors.
    /// Actors can read equipment state (SOC, power, connection_state, etc.)
    /// to make informed decisions.
    #[serde(default, skip_serializing)]
    pub equipment_telemetry: std::collections::HashMap<String, crate::Telemetry>,
    pub current_time: DateTime<FixedOffset>,
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
    #[serde(default)]
    pub price_signal: PriceSignal,
    #[serde(default)]
    pub electrical: ElectricalSummary,
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

    /// Replace the domain update for `update.domain_id` in-place, or append if
    /// no entry for that domain exists yet. Avoids the O(n) `retain` + `push`
    /// pattern that would reallocate on every timestep.
    #[inline]
    pub fn upsert_domain(&mut self, update: DomainUpdate) {
        if let Some(slot) = self
            .custom_domains
            .iter_mut()
            .find(|u| u.domain_id == update.domain_id)
        {
            *slot = update;
        } else {
            self.custom_domains.push(update);
        }
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
                solar_azimuth_deg: 180.0,
                mains_temp_c: 15.0,
                rainfall_m: 0.0,
                ground_albedo: 0.2,
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
            equipment_telemetry: std::collections::HashMap::new(),
            current_time: FixedOffset::east_opt(0)
                .expect("offset")
                .with_ymd_and_hms(2026, 3, 18, 12, 0, 0)
                .single()
                .expect("valid timestamp"),
            time_res: Duration::minutes(5),
            price_signal: PriceSignal::default(),
            electrical: ElectricalSummary::default(),
        };

        let json = serde_json::to_string(&state).expect("serialize environment state");
        let decoded: EnvironmentState =
            serde_json::from_str(&json).expect("deserialize environment state");
        assert_eq!(decoded, state);
    }

    #[test]
    fn environment_state_with_price_signal_roundtrip() {
        let state = EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: 21.0,
                humidity_ratio: 0.008,
                relative_humidity: 0.45,
                wet_bulb_c: 14.0,
                volume_m3: 200.0,
            }],
            weather: WeatherState::default(),
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
            },
            custom_domains: vec![],
            equipment_telemetry: std::collections::HashMap::new(),
            current_time: FixedOffset::east_opt(0)
                .expect("offset")
                .with_ymd_and_hms(2026, 3, 18, 12, 0, 0)
                .single()
                .expect("valid timestamp"),
            time_res: Duration::minutes(5),
            price_signal: PriceSignal {
                electricity_price: Some(0.25),
                export_price: Some(0.08),
                ghg_intensity: Some(0.4),
            },
            electrical: ElectricalSummary {
                pv_generation_kw: 3.5,
                base_load_kw: 1.2,
                net_grid_kw: -2.3,
                battery_power_kw: 0.0,
                ev_power_kw: 0.0,
            },
        };

        let json = serde_json::to_string(&state).expect("serialize");
        let decoded: EnvironmentState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded.price_signal.electricity_price, Some(0.25));
        assert_eq!(decoded.electrical.pv_generation_kw, 3.5);
        assert_eq!(decoded, state);
    }

    #[test]
    fn environment_state_default_electrical_summary() {
        let summary = ElectricalSummary::default();
        assert_eq!(summary.pv_generation_kw, 0.0);
        assert_eq!(summary.base_load_kw, 0.0);
        assert_eq!(summary.net_grid_kw, 0.0);
        assert_eq!(summary.battery_power_kw, 0.0);
        assert_eq!(summary.ev_power_kw, 0.0);
    }

    #[test]
    fn environment_state_backward_compat() {
        // JSON without price_signal or electrical fields should deserialize
        // with defaults thanks to #[serde(default)].
        let json = r#"{
            "zones": [],
            "weather": {
                "outdoor_temp_c": 5.0,
                "outdoor_humidity_ratio": 0.004,
                "outdoor_wet_bulb_c": 3.5,
                "outdoor_enthalpy_j_kg": 15000.0,
                "wind_speed_m_s": 3.2,
                "wind_dir_deg": 180.0,
                "ground_temp_c": 10.5,
                "sky_temp_c": -2.0,
                "pressure_kpa": 101.3,
                "solar_irradiance": []
            },
            "grid": { "voltage_pu": 1.0, "frequency_hz": 60.0 },
            "custom_domains": [],
            "current_time": "2026-03-18T12:00:00+00:00",
            "time_res": 300000
        }"#;

        let decoded: EnvironmentState = serde_json::from_str(json).expect("deserialize");
        assert_eq!(decoded.price_signal, PriceSignal::default());
        assert_eq!(decoded.electrical, ElectricalSummary::default());
    }

    #[test]
    fn upsert_domain_inserts_when_empty() {
        let mut state = crate::test_utils::default_env();
        state.custom_domains.clear();
        let update = DomainUpdate {
            domain_id: crate::DomainId(99),
            zone_temperatures_c: vec![(ZoneId(1), 22.0)],
            custom_payload: None,
        };
        state.upsert_domain(update.clone());
        assert_eq!(state.custom_domains.len(), 1);
        assert_eq!(state.custom_domains[0], update);
    }

    #[test]
    fn upsert_domain_replaces_existing() {
        let mut state = crate::test_utils::default_env();
        state.custom_domains.clear();
        let v1 = DomainUpdate {
            domain_id: crate::DomainId(5),
            zone_temperatures_c: vec![(ZoneId(1), 20.0)],
            custom_payload: Some(vec![1.0]),
        };
        let v2 = DomainUpdate {
            domain_id: crate::DomainId(5),
            zone_temperatures_c: vec![(ZoneId(1), 25.0)],
            custom_payload: Some(vec![2.0, 3.0]),
        };
        state.upsert_domain(v1);
        state.upsert_domain(v2.clone());
        assert_eq!(state.custom_domains.len(), 1);
        assert_eq!(state.custom_domains[0], v2);
    }

    #[test]
    fn upsert_domain_preserves_other_domains() {
        let mut state = crate::test_utils::default_env();
        state.custom_domains.clear();
        let a = DomainUpdate {
            domain_id: crate::DomainId(1),
            zone_temperatures_c: vec![],
            custom_payload: None,
        };
        let b = DomainUpdate {
            domain_id: crate::DomainId(2),
            zone_temperatures_c: vec![],
            custom_payload: None,
        };
        let b_updated = DomainUpdate {
            domain_id: crate::DomainId(2),
            zone_temperatures_c: vec![(ZoneId(1), 30.0)],
            custom_payload: Some(vec![99.0]),
        };
        state.upsert_domain(a.clone());
        state.upsert_domain(b);
        state.upsert_domain(b_updated.clone());
        assert_eq!(state.custom_domains.len(), 2);
        assert_eq!(state.custom_domains[0], a);
        assert_eq!(state.custom_domains[1], b_updated);
    }
}
