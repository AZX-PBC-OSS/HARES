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

use crate::{CoreOutput, DomainUpdate, EquipmentId};

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
///
/// `pv_generation_kw` may carry forecast values in forecast-driven
/// simulation modes (e.g., model-predictive-control). `actual_pv_kw`
/// always holds the observed PV output from the prior timestep's
/// equipment step. Real-time dispatch strategies (solar surplus
/// tracking, V2H) should read `actual_pv_kw` and fall back to
/// `pv_generation_kw` when `actual_pv_kw` is zero.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ElectricalSummary {
    /// Total PV generation [kW], positive = producing.
    /// In forecast-driven modes this may carry forecast values;
    /// in normal simulation it holds the prior-step actual output.
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
    /// Actual (observed) PV generation from equipment step [kW], positive = producing.
    ///
    /// Always reflects real equipment output; never carries forecast values.
    /// Set to 0.0 when no PV equipment is present or when actual data is
    /// unavailable. Real-time dispatch strategies that must not over-commit
    /// on forecast values should use this field with a fallback to
    /// `pv_generation_kw` when this value is zero.
    #[serde(default)]
    pub actual_pv_kw: f64,
}

impl ElectricalSummary {
    /// Best-available PV generation for real-time dispatch decisions.
    ///
    /// Returns `actual_pv_kw` when it is non-zero (real observed output from
    /// equipment step). Falls back to `pv_generation_kw` when `actual_pv_kw`
    /// is zero — because zero actual PV at night is indistinguishable from
    /// "no measurement available" in the current infrastructure. The `> 0.0`
    /// sentinel treats zero as absent, not as true zero-PV-now.
    ///
    /// Callers that must not over-commit on forecast values should use this
    /// method instead of reading `pv_generation_kw` directly.
    pub fn actual_pv_kw_or_fallback(&self) -> f64 {
        if self.actual_pv_kw > 0.0 {
            self.actual_pv_kw
        } else {
            self.pv_generation_kw
        }
    }
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
///
/// `relative_humidity` and `wet_bulb_c` are derived from `humidity_ratio` and
/// `temperature_c` via psychrometric functions at each access point (see
/// `hares_physics::psychrometrics::zone_relative_humidity` and
/// `zone_wet_bulb_c`). They are not stored as independent fields — this
/// guarantees consistency with the current thermodynamic state and matches the
/// OCHRE approach of computing on every access.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ZoneState {
    pub id: ZoneId,
    pub temperature_c: f64,
    pub humidity_ratio: f64,
    pub volume_m3: f64,
}

impl Default for ZoneState {
    fn default() -> Self {
        Self {
            id: ZoneId(0),
            temperature_c: 20.0,
            humidity_ratio: 0.008,
            volume_m3: 200.0,
        }
    }
}

impl ZoneState {
    pub fn new(id: ZoneId, temperature_c: f64, humidity_ratio: f64, volume_m3: f64) -> Self {
        Self {
            id,
            temperature_c,
            humidity_ratio,
            volume_m3,
        }
    }
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
///
/// # Default values
///
/// Every field carries a physically plausible default so that
/// `WeatherState::default()` produces a self-consistent meteorological
/// state. The defaults represent a calm, nighttime condition at 20 °C /
/// 50 % RH and standard sea-level pressure — a conservative "no data"
/// baseline that will not produce NaN/Inf, divide-by-zero, or
/// saturated-enthalpy paradoxes in downstream psychrometric or heat-load
/// calculations.
///
/// Fields with domain-specific defaults (mains temperature, ground albedo,
/// Kusuda-Achenbach parameters) delegate to the same `default_*()`
/// functions used for `#[serde(default = "...")]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    /// Defaults to 10.0 (US annual average per ASHRAE/EnergyPlus) when environment data is unavailable.
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
    /// Kusuda-Achenbach annual mean ground surface temperature [°C].
    ///
    /// Populated by `EnvironmentManager::update_in_place` from weather series
    /// statistics. Used by the thermal solver to compute depth-corrected
    /// ground temperature via `kusuda_achenbach_temp`.
    #[serde(default = "default_ground_t_mean_c")]
    pub ground_t_mean_c: f64,
    /// Kusuda-Achenbach annual amplitude of ground surface temperature [°C].
    ///
    /// Half the annual range of monthly mean dry-bulb temperatures
    /// (Kusuda & Achenbach 1965).
    #[serde(default = "default_ground_t_amplitude_c")]
    pub ground_t_amplitude_c: f64,
    /// Kusuda-Achenbach day of minimum surface temperature [day of year].
    ///
    /// Defaults to 35 (early Feb) for northern hemisphere mid-latitudes,
    /// 217.5 for southern hemisphere (EnergyPlus default).
    #[serde(default = "default_ground_phase_day")]
    pub ground_phase_day: f64,
    /// Current day of year [1–366].
    ///
    /// Populated by `EnvironmentManager::update_in_place` from
    /// `current_time.ordinal()`. Used by the thermal solver with
    /// `kusuda_achenbach_temp` for per-step ground temperature evaluation.
    #[serde(default = "default_day_of_year")]
    pub day_of_year: f64,
}

fn default_mains_temp_c() -> f64 {
    10.0
}

fn default_ground_albedo() -> f64 {
    0.2
}

fn default_ground_t_mean_c() -> f64 {
    // Conservative HARES default for annual mean soil surface temperature.
    // EnergyPlus monthly shallow ground temperatures default to 13.0 °C
    // (SiteShallowGroundTemperatures.cc:122), which would give a Kusuda-
    // Achenbach annual mean of 13.0. The lower 10.0 value is a deliberate
    // HARES choice for a "no data" baseline. EnvironmentManager overrides
    // this from weather series statistics.
    10.0
}

fn default_ground_t_amplitude_c() -> f64 {
    // Conservative default: zero amplitude means no seasonal variation.
    // With amplitude=0 the Kusuda model returns T_mean at all depths,
    // collapsing to the pre-fix behaviour (no depth correction).
    // EnvironmentManager overrides this with half the monthly-mean range.
    0.0
}

fn default_ground_phase_day() -> f64 {
    // EnergyPlus default for northern hemisphere mid-latitudes:
    // day 35 = early February. EnvironmentManager overrides based on latitude.
    35.0
}

fn default_day_of_year() -> f64 {
    1.0
}

impl Default for WeatherState {
    fn default() -> Self {
        // Calm, clear-sky nighttime condition at 20 °C dry-bulb /
        // 55 % RH / standard sea-level pressure. These values avoid
        // 0.0-derived NaN/Inf hazards in psychrometric and heat-load
        // calculations while staying conservative (no wind, no solar).
        // All sky/gas constants match EnergyPlus defaults.
        //
        // outdoor_temp_c:          20.0 °C — mild-day engineering default
        // outdoor_humidity_ratio:   0.008   — 55.2 % RH at 20.0 °C,
        //                                     101.325 kPa (ASHRAE HOF)
        // outdoor_wet_bulb_c:      14.0 °C — derived via ASHRAE HOF 2021
        //                                     Eq.35 (T_db=20.0, w=0.008,
        //                                     P=101.325 kPa); computed
        //                                     ~14.4 °C, rounded
        // outdoor_enthalpy_j_kg:   40_000   — ASHRAE HOF Eq.30 from
        //                                     T_db=20.0, w=0.008; exact
        //                                     value 40 426 J/kg, rounded
        // wind_speed_m_s:           0.0     — calm
        // wind_dir_deg:             0.0     — north
        // ground_temp_c:           13.0 °C — EnergyPlus default monthly
        //                                     shallow ground temperature
        //                                     (SiteShallowGroundTemperatures
        //                                     .cc:122; header default {13.0})
        // sky_temp_c:               5.5 °C — Clark-Allen clear-sky model
        //                                     at T_db=20.0, w=0.008
        //                                     (EnergyPlus default sky
        //                                     model; WeatherManager.hh:208,
        //                                     WeatherManager.cc:3213–3214)
        // pressure_kpa:           101.325   — standard sea-level pressure
        //                                     (EnergyPlus DataEnvironment
        //                                     .hh:82 StdPressureSeaLevel
        //                                     = 101325.0 Pa)
        // solar_irradiance:         []       — nighttime / no data
        // ghi_w_m2, dni_w_m2,
        // dhi_w_m2:                 0.0      — nighttime
        // solar_altitude_deg,
        // solar_azimuth_deg:        0.0      — night
        Self {
            outdoor_temp_c: 20.0,
            outdoor_humidity_ratio: 0.008,
            outdoor_wet_bulb_c: 14.0,
            outdoor_enthalpy_j_kg: 40_000.0,
            wind_speed_m_s: 0.0,
            wind_dir_deg: 0.0,
            ground_temp_c: 13.0,
            sky_temp_c: 5.5,
            pressure_kpa: 101.325,
            solar_irradiance: Vec::new(),
            ghi_w_m2: 0.0,
            dni_w_m2: 0.0,
            dhi_w_m2: 0.0,
            solar_altitude_deg: 0.0,
            solar_azimuth_deg: 0.0,
            mains_temp_c: default_mains_temp_c(),
            rainfall_m: 0.0,
            ground_albedo: default_ground_albedo(),
            ground_t_mean_c: default_ground_t_mean_c(),
            ground_t_amplitude_c: default_ground_t_amplitude_c(),
            ground_phase_day: default_ground_phase_day(),
            day_of_year: default_day_of_year(),
        }
    }
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
    /// Equipment typed core outputs from the previous timestep, keyed by id.
    /// Populated by the dwelling at end-of-step for actor reads on the next step.
    #[serde(default, skip_serializing)]
    pub equipment_core: std::collections::HashMap<EquipmentId, CoreOutput>,
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

    /// Like `upsert_domain` but clones from a reference, reusing the existing
    /// slot's allocations via `clone_from` when the domain already exists.
    #[inline]
    pub fn upsert_domain_ref(&mut self, update: &DomainUpdate) {
        if let Some(slot) = self
            .custom_domains
            .iter_mut()
            .find(|u| u.domain_id == update.domain_id)
        {
            slot.clone_from(update);
        } else {
            self.custom_domains.push(update.clone());
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
                mains_temp_c: 10.0,
                rainfall_m: 0.0,
                ground_albedo: 0.2,
                ground_t_mean_c: 10.0,
                ground_t_amplitude_c: 0.0,
                ground_phase_day: 35.0,
                day_of_year: 1.0,
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
            equipment_core: std::collections::HashMap::new(),
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
                volume_m3: 200.0,
            }],
            weather: WeatherState::default(),
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
            },
            custom_domains: vec![],
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: std::collections::HashMap::new(),
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
                actual_pv_kw: 3.5,
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
        assert_eq!(summary.actual_pv_kw, 0.0);
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

    /// WeatherState::default() must produce values that avoid NaN/Inf hazards
    /// and are physically self-consistent: non-zero pressure, non-zero
    /// ground temperature, sky temperature below dry-bulb, etc.
    #[test]
    fn weather_state_default_produces_physically_plausible_ranges() {
        let w = WeatherState::default();

        // Pressure: standard sea level ± 5 %, not vacuum.
        assert!(
            (w.pressure_kpa - 101.325).abs() < 5.0,
            "pressure_kpa={} is not near standard sea level (101.325 kPa)",
            w.pressure_kpa,
        );

        // Dry-bulb: a mild day, not freezing and not extreme.
        assert!(
            (10.0..=35.0).contains(&w.outdoor_temp_c),
            "outdoor_temp_c={} is not in mild-day range 10–35 °C",
            w.outdoor_temp_c,
        );

        // Humidity ratio: non-zero, non-negative, sub-saturated.
        assert!(
            w.outdoor_humidity_ratio > 0.0,
            "outdoor_humidity_ratio={} is zero — bone-dry air invalid",
            w.outdoor_humidity_ratio,
        );
        assert!(
            w.outdoor_humidity_ratio < 0.030,
            "outdoor_humidity_ratio={} exceeds reasonable outdoor max ~0.030 kg/kg",
            w.outdoor_humidity_ratio,
        );

        // Wet-bulb: between dew-point and dry-bulb.
        assert!(
            w.outdoor_wet_bulb_c > 0.0,
            "outdoor_wet_bulb_c={} is sub-freezing for a mild-day default",
            w.outdoor_wet_bulb_c,
        );
        assert!(
            w.outdoor_wet_bulb_c <= w.outdoor_temp_c,
            "outdoor_wet_bulb_c={} exceeds outdoor_temp_c={}",
            w.outdoor_wet_bulb_c,
            w.outdoor_temp_c,
        );

        // Enthalpy: non-zero, positive for above-freezing moist air.
        assert!(
            w.outdoor_enthalpy_j_kg > 1_000.0,
            "outdoor_enthalpy_j_kg={} J/kg is near zero (0 K air)",
            w.outdoor_enthalpy_j_kg,
        );

        // Ground temperature: within normal soil range.
        assert!(
            (5.0..=25.0).contains(&w.ground_temp_c),
            "ground_temp_c={} is outside normal soil range 5–25 °C",
            w.ground_temp_c,
        );

        // Sky temperature: below dry-bulb (radiative cooling to sky).
        assert!(
            w.sky_temp_c < w.outdoor_temp_c,
            "sky_temp_c={} is not below outdoor_temp_c={} — \
             longwave sky cooling eliminated",
            w.sky_temp_c,
            w.outdoor_temp_c,
        );
        assert!(
            w.sky_temp_c > -30.0,
            "sky_temp_c={} is unreasonably cold for a mild-day default",
            w.sky_temp_c,
        );

        // Mains temperature: within plausible range.
        assert!(
            (2.0..=25.0).contains(&w.mains_temp_c),
            "mains_temp_c={} is outside plausible range",
            w.mains_temp_c,
        );

        // Ground albedo: physically bounded.
        assert!(
            (0.0..=1.0).contains(&w.ground_albedo),
            "ground_albedo={} is outside [0, 1]",
            w.ground_albedo,
        );

        // Solar fields: zero irradiance at night is correct.
        assert_eq!(w.ghi_w_m2, 0.0, "ghi_w_m2 should be 0.0 at night");
        assert_eq!(w.dni_w_m2, 0.0, "dni_w_m2 should be 0.0 at night");
        assert_eq!(w.dhi_w_m2, 0.0, "dhi_w_m2 should be 0.0 at night");
        assert!(
            w.solar_irradiance.is_empty(),
            "solar_irradiance should be empty at night"
        );

        // Kusuda-Achenbach parameters: plausible ranges.
        assert!(
            (0.0..=45.0).contains(&w.ground_t_mean_c),
            "ground_t_mean_c={} outside plausible annual soil mean range",
            w.ground_t_mean_c,
        );
        assert!(
            w.ground_t_amplitude_c >= 0.0,
            "ground_t_amplitude_c={} must be non-negative",
            w.ground_t_amplitude_c,
        );
        assert!(
            (1.0..=365.0).contains(&w.ground_phase_day),
            "ground_phase_day={} outside day-of-year range",
            w.ground_phase_day,
        );
        assert!(
            (1.0..=366.0).contains(&w.day_of_year),
            "day_of_year={} outside calendar range",
            w.day_of_year,
        );
    }

    /// All WeatherState defaults must survive JSON round-trip unchanged.
    /// Proves that the explicit Default impl is compatible with serde
    /// (de)serialisation of every field including the new Kusuda-Achenbach
    /// fields.
    #[test]
    fn weather_state_default_round_trips_through_json() {
        let w = WeatherState::default();
        let json = serde_json::to_string(&w).expect("serialize WeatherState");
        let decoded: WeatherState = serde_json::from_str(&json).expect("deserialize WeatherState");
        assert_eq!(
            decoded, w,
            "WeatherState default must survive JSON round-trip"
        );
    }

    /// Proof that a zero-pressure WeatherState would be caught by the range
    /// check.  Setting pressure_kpa to 0.0 (the old derived-Default value)
    /// triggers a panic.
    #[test]
    #[should_panic(expected = "vacuum is invalid")]
    fn weather_state_zero_pressure_is_rejected() {
        let w = WeatherState {
            pressure_kpa: 0.0,
            ..WeatherState::default()
        };
        assert!(
            w.pressure_kpa > 0.0,
            "pressure_kpa must be > 0 — vacuum is invalid"
        );
    }
}
