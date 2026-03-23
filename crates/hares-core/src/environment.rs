//! Outdoor environment state (weather, grid signals).

use std::time::Duration as StdDuration;

#[cfg(test)]
use chrono::Duration;
use chrono::{DateTime, Datelike, FixedOffset, Timelike};
use hares_io::{Building, ScheduleTimeSeries, WeatherMeta, WeatherTimeSeries};
use hares_io::{schedule::ScheduleError, weather::WeatherError, weather::WeatherField};
use hares_physics::{
    psychrometrics::{humidity_ratio_from_tdp, moist_air_enthalpy, wet_bulb_from_humidity_ratio},
    solar::{perez_tilted_irradiance, solar_position},
    water_mains::{Hemisphere, water_mains_temperature_c},
};
use hares_types::{
    DomainId, EnvironmentState, GridState, WeatherState, ZoneId, ZoneState, schedule_domain_id,
};
use thiserror::Error;

use crate::SimClock;

const DEFAULT_ZONE_VOLUME_M3: f64 = 200.0;
const DEFAULT_GRID_VOLTAGE_PU: f64 = 1.0;
const DEFAULT_GRID_FREQUENCY_HZ: f64 = 60.0;
const MAINS_WATER_DOMAIN_ID: DomainId = DomainId(u16::MAX - 1);

/// Geometry required for solar irradiance projection.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceGeometry {
    pub surface_id: u32,
    pub azimuth_deg: f64,
    pub tilt_deg: f64,
    pub area_m2: f64,
}

/// Errors returned by [`EnvironmentManager`].
#[derive(Debug, Error)]
pub enum EnvironmentManagerError {
    #[error("weather resample error: {0}")]
    WeatherResample(#[from] WeatherError),
    #[error("schedule resample error: {0}")]
    ScheduleResample(#[from] ScheduleError),
    #[error("time_res must be non-zero")]
    ZeroTimeResolution,
    #[error("weather time series is empty")]
    EmptyWeather,
    #[error("schedule time series is empty")]
    EmptySchedule,
    #[error("invalid IANA timezone: {0}")]
    InvalidTimezone(String),
    #[error("civil_timezone requires the 'dst' cargo feature")]
    DstNotEnabled,
}

/// Produces a complete [`EnvironmentState`] at each timestep.
#[derive(Debug, Clone)]
pub struct EnvironmentManager {
    weather: WeatherTimeSeries,
    schedule: ScheduleTimeSeries,
    weather_meta: WeatherMeta,
    surfaces: Vec<SurfaceGeometry>,
    grid_override: Option<GridState>,
    zones: Vec<ZoneState>,
    /// Offset into weather/schedule arrays for simulation start.
    /// EPW files are annual starting Jan 1; if the simulation starts
    /// mid-year, this shifts the index so step 0 reads the right row.
    weather_start_offset: usize,
    schedule_start_offset: usize,
    mains_t_annual_avg_c: f64,
    mains_dt_annual_range_c: f64,
    mains_hemisphere: Hemisphere,
    /// When set, schedule indexing uses DST-aware civil time from this IANA
    /// timezone instead of the fixed UTC offset. Weather indexing is unaffected.
    #[cfg(feature = "dst")]
    civil_tz: Option<chrono_tz::Tz>,
}

impl EnvironmentManager {
    /// Build an environment manager from weather/schedule series and parsed HPXML building.
    ///
    /// `civil_timezone` optionally specifies an IANA timezone string (e.g.
    /// `"America/New_York"`) for DST-aware schedule indexing. When `None`,
    /// schedules are indexed by the fixed UTC offset from `start_time`.
    /// Requires the `dst` cargo feature; returns [`EnvironmentManagerError::DstNotEnabled`]
    /// if a timezone is supplied without the feature.
    pub fn new(
        weather: WeatherTimeSeries,
        schedule: ScheduleTimeSeries,
        building: &Building,
        time_res: StdDuration,
        start_time: DateTime<FixedOffset>,
        civil_timezone: Option<&str>,
    ) -> Result<Self, EnvironmentManagerError> {
        let step_secs = u32::try_from(time_res.as_secs()).unwrap_or(u32::MAX);
        if step_secs == 0 {
            return Err(EnvironmentManagerError::ZeroTimeResolution);
        }

        let weather = weather.resample(step_secs)?;
        if weather.is_empty() {
            return Err(EnvironmentManagerError::EmptyWeather);
        }

        let schedule = schedule.resample(step_secs)?;
        if schedule.is_empty() {
            return Err(EnvironmentManagerError::EmptySchedule);
        }

        // Parse civil timezone for DST-aware schedule indexing.
        #[cfg(feature = "dst")]
        let civil_tz: Option<chrono_tz::Tz> = match civil_timezone {
            Some(name) => Some(
                name.parse::<chrono_tz::Tz>()
                    .map_err(|_| EnvironmentManagerError::InvalidTimezone(name.to_owned()))?,
            ),
            None => None,
        };
        #[cfg(not(feature = "dst"))]
        if civil_timezone.is_some() {
            return Err(EnvironmentManagerError::DstNotEnabled);
        }

        // Compute start offsets: EPW files are annual starting Jan 1.
        // The schedule CSV may also be annual. Offset into them based on
        // the simulation start time so step 0 reads the correct row.
        let weather_start_offset = compute_annual_offset(&weather.meta, start_time, step_secs);
        let schedule_start_offset = compute_schedule_offset(&schedule, start_time, step_secs);
        let (mains_t_annual_avg_c, mains_dt_annual_range_c) =
            compute_mains_inputs(&weather, step_secs);

        let weather_meta = weather.meta.clone();
        let mains_hemisphere = if weather_meta.latitude < 0.0 {
            Hemisphere::Southern
        } else {
            Hemisphere::Northern
        };
        let surfaces = build_surface_geometry(building);
        let init_offset = compute_annual_offset_unshifted(&weather.meta, start_time, step_secs);
        let initial_outdoor_temp_c = weather
            .dry_bulb_c
            .get(init_offset)
            .copied()
            .unwrap_or(DEFAULT_SETPOINT_C);
        let zones = initial_zones(building, initial_outdoor_temp_c);

        Ok(Self {
            weather,
            schedule,
            weather_meta,
            surfaces,
            grid_override: None,
            zones,
            weather_start_offset,
            schedule_start_offset,
            mains_t_annual_avg_c,
            mains_dt_annual_range_c,
            mains_hemisphere,
            #[cfg(feature = "dst")]
            civil_tz,
        })
    }

    /// Override default grid state for subsequent updates.
    pub fn set_grid_override(&mut self, grid: GridState) {
        self.grid_override = Some(grid);
    }

    /// Clear any active grid override and restore defaults.
    pub fn clear_grid_override(&mut self) {
        self.grid_override = None;
    }

    /// Compute the schedule array index for the current timestep.
    ///
    /// When a DST-aware civil timezone is configured, the schedule is indexed
    /// by civil (wall-clock) time so that occupancy/rate schedules follow local
    /// DST transitions naturally. Weather indexing is **not** affected — solar
    /// position and meteorological data are physical quantities tied to UTC, not
    /// civil time.
    #[allow(unused_variables)]
    fn compute_schedule_idx(&self, clock: &SimClock, step: usize) -> usize {
        let schedule_len = self.schedule.len();

        #[cfg(feature = "dst")]
        if let Some(tz) = self.civil_tz {
            let sim_time = clock.current_time();
            let civil = sim_time.with_timezone(&tz);
            let doy0 = civil.ordinal0() as u64;
            let h = civil.hour() as u64;
            let m = civil.minute() as u64;
            let s = civil.second() as u64;
            let civil_secs = doy0 * 86400 + h * 3600 + m * 60 + s;
            let step_secs = clock.time_res.num_seconds().unsigned_abs();
            if step_secs == 0 {
                return 0;
            }
            return (civil_secs / step_secs) as usize % schedule_len;
        }

        // Fixed-offset fallback: use precomputed start offset.
        (step + self.schedule_start_offset) % schedule_len
    }

    /// Borrow the parsed schedule time series.
    #[must_use]
    pub fn schedule(&self) -> &ScheduleTimeSeries {
        &self.schedule
    }

    /// Mutably borrow the parsed schedule time series.
    pub fn schedule_mut(&mut self) -> &mut ScheduleTimeSeries {
        &mut self.schedule
    }

    /// Returns the schedule column index for the occupancy column, if present.
    ///
    /// The returned index can be used to look up the occupancy value from the
    /// schedule payload in [`EnvironmentState::custom_domains`] at domain id
    /// [`hares_types::schedule_domain_id`].
    #[must_use]
    pub fn occupancy_column_idx(&self) -> Option<usize> {
        // Exact-match fast path for common lowercase keys.
        if let Some(&idx) = self
            .schedule
            .column_index
            .get("occupants")
            .or_else(|| self.schedule.column_index.get("occupancy"))
        {
            return Some(idx);
        }
        // Case-insensitive fallback: matches "Occupancy (Persons)" and similar variants.
        self.schedule
            .column_index
            .iter()
            .find(|(key, _)| {
                let lower = key.to_lowercase();
                lower.starts_with("occupan")
            })
            .map(|(_, &idx)| idx)
    }

    /// Update environment state for the current clock step.
    #[must_use]
    pub fn update(&mut self, clock: &SimClock, zone_states: &[ZoneState]) -> EnvironmentState {
        let step = usize::try_from(clock.current_step()).unwrap_or(usize::MAX);
        let weather_len = self.weather.len();
        let weather_idx = if weather_len == 0 {
            0
        } else {
            (step + self.weather_start_offset) % weather_len
        };
        let schedule_idx = if self.schedule.is_empty() {
            0
        } else {
            self.compute_schedule_idx(clock, step)
        };

        // Step 1: weather lookup and psychrometric derivations
        let outdoor_temp_c = self.weather.get(WeatherField::DryBulbC, weather_idx);
        let dew_point_c = self.weather.get(WeatherField::DewPointC, weather_idx);
        let pressure_kpa = self.weather.get(WeatherField::PressureKpa, weather_idx);
        let pressure_pa = pressure_kpa * 1000.0;
        let outdoor_humidity_ratio = humidity_ratio_from_tdp(dew_point_c, pressure_pa);
        let outdoor_wet_bulb_c =
            wet_bulb_from_humidity_ratio(outdoor_temp_c, outdoor_humidity_ratio, pressure_pa);
        let outdoor_enthalpy_j_kg = moist_air_enthalpy(outdoor_temp_c, outdoor_humidity_ratio);

        // Step 2: per-surface solar irradiance
        // solar_position() converts to UTC internally — it needs true UTC for
        // solar geometry. We pass the local-time clock value.
        let now = clock.current_time();
        let pos = solar_position(self.weather_meta.latitude, self.weather_meta.longitude, now);
        let ghi = self.weather.get(WeatherField::GhiWM2, weather_idx);
        let dni = self.weather.get(WeatherField::DniWM2, weather_idx);
        let dhi = self.weather.get(WeatherField::DhiWM2, weather_idx);
        let solar_zenith_deg = (90.0 - pos.altitude_deg).max(0.0);
        let day_of_year = now.ordinal();
        let mains_temp_c = water_mains_temperature_c(
            self.mains_t_annual_avg_c,
            self.mains_dt_annual_range_c,
            u16::try_from(day_of_year).unwrap_or(366),
            self.mains_hemisphere,
        );
        let ground_albedo = self.weather.get(WeatherField::SurfaceAlbedo, weather_idx);
        let solar_irradiance = self
            .surfaces
            .iter()
            .map(|surface| {
                perez_tilted_irradiance(
                    surface.surface_id,
                    ghi,
                    dni,
                    dhi,
                    solar_zenith_deg,
                    pos.azimuth_deg,
                    surface.tilt_deg,
                    surface.azimuth_deg,
                    day_of_year,
                    ground_albedo,
                )
            })
            .collect();

        // Step 3: schedule values
        let schedule_values = self
            .schedule
            .columns
            .iter()
            .map(|col| col[schedule_idx])
            .collect::<Vec<_>>();

        // Step 4: zone-state feedback
        if !zone_states.is_empty() {
            self.zones = zone_states.to_vec();
        }

        // Step 5: grid defaults / overrides
        let grid = self.grid_override.clone().unwrap_or(GridState {
            voltage_pu: DEFAULT_GRID_VOLTAGE_PU,
            frequency_hz: DEFAULT_GRID_FREQUENCY_HZ,
        });

        EnvironmentState {
            zones: self.zones.clone(),
            weather: WeatherState {
                outdoor_temp_c,
                outdoor_humidity_ratio,
                outdoor_wet_bulb_c,
                outdoor_enthalpy_j_kg,
                wind_speed_m_s: self.weather.get(WeatherField::WindSpeedMS, weather_idx),
                wind_dir_deg: self.weather.get(WeatherField::WindDirDeg, weather_idx),
                ground_temp_c: self.weather.get(WeatherField::GroundTempC, weather_idx),
                sky_temp_c: self.weather.get(WeatherField::SkyTempC, weather_idx),
                pressure_kpa,
                solar_irradiance,
                ghi_w_m2: ghi,
                dni_w_m2: dni,
                dhi_w_m2: dhi,
                solar_altitude_deg: pos.altitude_deg,
                mains_temp_c,
                rainfall_m: self.weather.get(WeatherField::LiquidPrecipM, weather_idx),
                ground_albedo,
            },
            grid,
            custom_domains: vec![
                hares_types::DomainUpdate {
                    domain_id: schedule_domain_id(),
                    zone_temperatures_c: Vec::new(),
                    custom_payload: Some(schedule_values),
                },
                hares_types::DomainUpdate {
                    domain_id: MAINS_WATER_DOMAIN_ID,
                    zone_temperatures_c: Vec::new(),
                    custom_payload: Some(vec![mains_temp_c]),
                },
            ],
            current_time: clock.current_time(),
            time_res: clock.time_res,
        }
    }
}

/// EPW hour-ending midpoint shift (30 minutes = 1800 seconds).
/// EPW files use hour-ending convention: hour 13 covers 12:00–13:00.
/// Subtracting 30 minutes aligns the data with the period midpoint,
/// matching the pvlib/OCHRE convention.
const EPW_MIDPOINT_SHIFT_SECS: u64 = 1800;

/// Compute the step offset into an annual weather file for a given start time.
///
/// EPW files are indexed by **local standard time** (LST). The simulation
/// start time is already in local time (`DateTime<FixedOffset>`), so no
/// UTC→LST conversion is needed — we extract wall-clock components directly.
fn compute_annual_offset(
    _meta: &WeatherMeta,
    start_time: DateTime<FixedOffset>,
    step_secs: u32,
) -> usize {
    if step_secs == 0 {
        return 0;
    }

    let doy0 = start_time.ordinal0() as u64;
    let h = start_time.hour() as u64;
    let m = start_time.minute() as u64;
    let s = start_time.second() as u64;
    let seconds_into_year = doy0 * 86400 + h * 3600 + m * 60 + s;

    let year_secs = if start_time.date_naive().leap_year() {
        366 * 86400_u64
    } else {
        365 * 86400_u64
    };
    let shifted = (seconds_into_year + year_secs - EPW_MIDPOINT_SHIFT_SECS) % year_secs;

    (shifted / step_secs as u64) as usize
}

/// Un-shifted annual offset for initial-condition lookup.
/// OCHRE reads the schedule/weather at the raw hour-ending position for init.
fn compute_annual_offset_unshifted(
    _meta: &WeatherMeta,
    start_time: DateTime<FixedOffset>,
    step_secs: u32,
) -> usize {
    if step_secs == 0 {
        return 0;
    }
    let doy0 = start_time.ordinal0() as u64;
    let h = start_time.hour() as u64;
    let m = start_time.minute() as u64;
    let s = start_time.second() as u64;
    let seconds_into_year = doy0 * 86400 + h * 3600 + m * 60 + s;
    (seconds_into_year / step_secs as u64) as usize
}

/// Compute offset into the schedule time series.
/// Schedules from ResStock/BEopt are annual, indexed from Jan 1 in local time.
fn compute_schedule_offset(
    schedule: &ScheduleTimeSeries,
    start_time: DateTime<FixedOffset>,
    step_secs: u32,
) -> usize {
    if step_secs == 0 || schedule.is_empty() {
        return 0;
    }
    let doy0 = start_time.ordinal0() as u64;
    let h = start_time.hour() as u64;
    let m = start_time.minute() as u64;
    let s = start_time.second() as u64;
    let seconds_into_year = doy0 * 86400 + h * 3600 + m * 60 + s;
    (seconds_into_year / step_secs as u64) as usize
}

fn compute_mains_inputs(weather: &WeatherTimeSeries, step_secs: u32) -> (f64, f64) {
    let annual_avg_c = mean_or_default(&weather.dry_bulb_c, 20.0);

    let temps = &weather.dry_bulb_c;
    let samples_per_day = usize::try_from(86_400 / step_secs.max(1)).unwrap_or(0);
    if samples_per_day == 0 {
        return (annual_avg_c, simple_range(temps));
    }

    // For annual weather (EPW-derived), compute the range of monthly average dry-bulb
    // temperatures, as required by the Burch-Christensen mains model.
    let month_days = [31usize, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let year_samples: usize = month_days.iter().sum::<usize>() * samples_per_day;
    if temps.len() < year_samples {
        return (annual_avg_c, simple_range(temps));
    }

    let mut month_means = Vec::with_capacity(12);
    let mut cursor = 0usize;
    for days in month_days {
        let month_samples = days * samples_per_day;
        let end = cursor + month_samples;
        let slice = &temps[cursor..end];
        month_means.push(mean_or_default(slice, annual_avg_c));
        cursor = end;
    }

    let monthly_range_c = simple_range(&month_means);
    (annual_avg_c, monthly_range_c.max(1.0))
}

fn mean_or_default(values: &[f64], default: f64) -> f64 {
    if values.is_empty() {
        return default;
    }
    let sum = values.iter().copied().sum::<f64>();
    sum / values.len() as f64
}

fn simple_range(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 20.0;
    }
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    (max - min).max(0.0)
}

fn build_surface_geometry(building: &Building) -> Vec<SurfaceGeometry> {
    building
        .boundaries
        .iter()
        .enumerate()
        .map(|(idx, boundary)| {
            let tilt_deg = boundary.tilt_deg.unwrap_or(90.0);
            SurfaceGeometry {
                surface_id: u32::try_from(idx).unwrap_or(u32::MAX),
                azimuth_deg: boundary.azimuth_deg.unwrap_or(180.0),
                tilt_deg,
                area_m2: boundary.area_m2,
            }
        })
        .collect()
}

/// Outdoor temperature threshold for selecting heating vs cooling setpoint
/// at initialization, matching OCHRE's Envelope.initialize_state().
const OUTDOOR_HEATING_COOLING_THRESHOLD_C: f64 = 12.0;
const DEFAULT_SETPOINT_C: f64 = 21.0;

fn initial_zones(building: &Building, outdoor_temp_c: f64) -> Vec<ZoneState> {
    let default_temp = determine_initial_indoor_temp_c(building, outdoor_temp_c);
    if building.zones.is_empty() {
        return vec![ZoneState {
            id: ZoneId(1),
            temperature_c: default_temp,
            humidity_ratio: 0.008,
            relative_humidity: 0.45,
            wet_bulb_c: default_temp,
            volume_m3: DEFAULT_ZONE_VOLUME_M3,
        }];
    }

    building
        .zones
        .iter()
        .enumerate()
        .map(|(idx, zone)| ZoneState {
            id: ZoneId(u16::try_from(idx + 1).unwrap_or(u16::MAX)),
            temperature_c: default_temp,
            humidity_ratio: 0.008,
            relative_humidity: 0.45,
            wet_bulb_c: default_temp,
            volume_m3: zone.volume_m3.unwrap_or(DEFAULT_ZONE_VOLUME_M3),
        })
        .collect()
}

/// Determines initial indoor temperature from HVAC setpoints and outdoor temp.
///
/// Matches OCHRE's `Envelope.initialize_state()`:
/// - outdoor > 12°C → cooling setpoint (building is in cooling mode)
/// - outdoor ≤ 12°C → heating setpoint (building is in heating mode)
/// - No setpoints available → 21°C (OCHRE default)
fn determine_initial_indoor_temp_c(building: &Building, outdoor_temp_c: f64) -> f64 {
    let heating_sp = building
        .heating_weekday_setpoints_c
        .as_ref()
        .and_then(|v| v.first().copied());
    let cooling_sp = building
        .cooling_weekday_setpoints_c
        .as_ref()
        .and_then(|v| v.first().copied());

    match (heating_sp, cooling_sp) {
        (Some(h), Some(c)) => {
            if outdoor_temp_c > OUTDOOR_HEATING_COOLING_THRESHOLD_C {
                c
            } else {
                h
            }
        }
        (Some(h), None) => h,
        (None, Some(c)) => c,
        (None, None) => DEFAULT_SETPOINT_C,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, FixedOffset, TimeZone};
    use hares_io::hpxml::building::XmlNode;
    use hares_io::hpxml::{Boundary, BoundaryType, Site, Window, Zone, ZoneType};
    use std::collections::HashMap;

    fn utc_offset() -> FixedOffset {
        FixedOffset::east_opt(0).expect("offset")
    }

    fn ts(hour: u32) -> DateTime<FixedOffset> {
        utc_offset()
            .with_ymd_and_hms(2024, 1, 1, hour, 0, 0)
            .single()
            .expect("time")
    }

    fn weather_series() -> WeatherTimeSeries {
        WeatherTimeSeries {
            meta: WeatherMeta {
                location: "Test".to_string(),
                latitude: 40.0,
                longitude: 0.0,
                timezone_offset_h: 0.0,
                elevation_m: 1000.0,
                source_step_secs: 3600,
            },
            dry_bulb_c: vec![10.0, 20.0],
            dew_point_c: vec![2.0, 3.0],
            rel_humidity_pct: vec![50.0, 55.0],
            pressure_kpa: vec![100.0, 100.5],
            ghi_w_m2: vec![500.0, 600.0],
            dni_w_m2: vec![700.0, 800.0],
            dhi_w_m2: vec![100.0, 110.0],
            wind_speed_m_s: vec![3.0, 4.0],
            wind_dir_deg: vec![180.0, 190.0],
            opaque_sky_cover: vec![2.0, 3.0],
            horizontal_infrared_w_m2: vec![300.0, 310.0],
            sky_temp_c: vec![5.0, 6.0],
            ground_temp_c: vec![8.0, 9.0],
            liquid_precip_m: vec![0.0, 0.0],
            surface_albedo: None,
        }
    }

    fn schedule_series() -> ScheduleTimeSeries {
        let mut index = HashMap::new();
        index.insert("known_schedule".to_string(), 0);
        ScheduleTimeSeries {
            timestamps: vec![ts(0), ts(1)],
            column_names: vec!["known_schedule".to_string()],
            columns: vec![vec![1.23, 4.56]],
            column_index: index,
            source_step_secs: 3600,
            column_aggregations: vec![],
        }
    }

    fn building(indoor_design_temp_c: Option<f64>) -> Building {
        let mut details_xml = XmlNode {
            name: "BuildingDetails".to_string(),
            attrs: HashMap::new(),
            text: String::new(),
            children: Vec::new(),
        };
        if let Some(temp) = indoor_design_temp_c {
            details_xml.children.push(XmlNode {
                name: "IndoorTemperature".to_string(),
                attrs: HashMap::new(),
                text: temp.to_string(),
                children: Vec::new(),
            });
        }

        Building {
            site: Site {
                elevation_m: None,
                site_type: None,
                shielding_of_home: None,
                latitude_deg: Some(40.0),
                longitude_deg: Some(0.0),
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
                },
            ],
            windows: Vec::<Window>::new(),
            infiltration_ach50: None,
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
            details_xml,
        }
    }

    fn clock() -> SimClock {
        let start = DateTime::parse_from_rfc3339("2024-06-21T12:00:00+00:00").expect("parse");
        SimClock::new(start, Duration::seconds(60), Duration::hours(2))
    }

    #[test]
    fn weather_step_0_matches_first_epw_record() {
        // Start at 00:30 LST: midpoint shift (shifted = 1800 - 1800 = 0) maps to
        // row 0 of the weather series (the first EPW hour-ending record).
        let start = utc_offset().with_ymd_and_hms(2023, 1, 1, 0, 30, 0).unwrap();
        let mut manager = EnvironmentManager::new(
            weather_series(),
            schedule_series(),
            &building(Some(21.0)),
            StdDuration::from_secs(60),
            start,
            None,
        )
        .expect("manager");
        let sim_clock = SimClock::new(start, Duration::seconds(60), Duration::hours(2));
        let env = manager.update(&sim_clock, &[]);
        assert!((env.weather.outdoor_temp_c - 10.0).abs() < 1.0e-6);
    }

    #[test]
    fn weather_step_59_uses_pchip_interpolation() {
        // With 2-element [10.0, 20.0] input and PCHIP (linear for 2-pt),
        // step 59 of 60 sub-hour slots ≈ 10.0 + (59/60)*10.0 ≈ 19.833.
        let start = utc_offset().with_ymd_and_hms(2023, 1, 1, 0, 30, 0).unwrap();
        let mut manager = EnvironmentManager::new(
            weather_series(),
            schedule_series(),
            &building(Some(21.0)),
            StdDuration::from_secs(60),
            start,
            None,
        )
        .expect("manager");
        let mut sim_clock = SimClock::new(start, Duration::seconds(60), Duration::hours(2));
        for _ in 0..59 {
            let _ = sim_clock.next();
        }
        let env = manager.update(&sim_clock, &[]);
        // PCHIP linearly interpolates between 10.0 and 20.0 for 2-element input.
        let expected = 10.0 + (59.0 / 60.0) * 10.0;
        assert!(
            (env.weather.outdoor_temp_c - expected).abs() < 0.1,
            "expected ~{expected}, got {}",
            env.weather.outdoor_temp_c
        );
    }

    #[test]
    fn wind_direction_is_populated_from_weather_series() {
        let start = utc_offset().with_ymd_and_hms(2023, 1, 1, 0, 30, 0).unwrap();
        let mut manager = EnvironmentManager::new(
            weather_series(),
            schedule_series(),
            &building(Some(21.0)),
            StdDuration::from_secs(60),
            start,
            None,
        )
        .expect("manager");
        let sim_clock = SimClock::new(start, Duration::seconds(60), Duration::hours(2));
        let env = manager.update(&sim_clock, &[]);
        assert!((env.weather.wind_dir_deg - 180.0).abs() < 1.0e-6);
    }

    #[test]
    fn schedule_value_is_loaded_each_timestep() {
        let mut manager = EnvironmentManager::new(
            weather_series(),
            schedule_series(),
            &building(Some(21.0)),
            StdDuration::from_secs(60),
            utc_offset().with_ymd_and_hms(1970, 1, 1, 0, 0, 0).unwrap(),
            None,
        )
        .expect("manager");
        let env = manager.update(&clock(), &[]);
        let payload = env.custom_domains[0]
            .custom_payload
            .clone()
            .expect("schedule payload");
        assert!((payload[0] - 1.23).abs() < 1.0e-6);
    }

    #[test]
    fn schedule_index_wraps_after_schedule_length() {
        let mut manager = EnvironmentManager::new(
            weather_series(),
            schedule_series(),
            &building(Some(21.0)),
            StdDuration::from_secs(60),
            utc_offset().with_ymd_and_hms(1970, 1, 1, 0, 0, 0).unwrap(),
            None,
        )
        .expect("manager");

        let start = DateTime::parse_from_rfc3339("2024-06-21T12:00:00+00:00").expect("parse");
        let mut clock = SimClock::new(start, Duration::seconds(60), Duration::hours(3));

        for _ in 0..120 {
            let _ = clock.next();
        }
        let env = manager.update(&clock, &[]);
        let payload = env.custom_domains[0]
            .custom_payload
            .clone()
            .expect("schedule payload");
        assert!((payload[0] - 1.23).abs() < 1.0e-6);
    }

    #[test]
    fn south_surface_receives_more_direct_solar_than_north_at_noon() {
        let mut manager = EnvironmentManager::new(
            weather_series(),
            schedule_series(),
            &building(Some(21.0)),
            StdDuration::from_secs(60),
            utc_offset().with_ymd_and_hms(1970, 1, 1, 0, 0, 0).unwrap(),
            None,
        )
        .expect("manager");
        let env = manager.update(&clock(), &[]);
        assert!(
            env.weather.solar_irradiance[0].direct_w_m2
                > env.weather.solar_irradiance[1].direct_w_m2
        );
    }

    #[test]
    fn zone_feedback_overrides_previous_zone_state() {
        let mut manager = EnvironmentManager::new(
            weather_series(),
            schedule_series(),
            &building(Some(21.0)),
            StdDuration::from_secs(60),
            utc_offset().with_ymd_and_hms(1970, 1, 1, 0, 0, 0).unwrap(),
            None,
        )
        .expect("manager");
        let feedback = vec![ZoneState {
            id: ZoneId(1),
            temperature_c: 22.0,
            humidity_ratio: 0.008,
            relative_humidity: 0.45,
            wet_bulb_c: 22.0,
            volume_m3: 200.0,
        }];
        let env = manager.update(&clock(), &feedback);
        assert_eq!(env.zones[0].temperature_c, 22.0);
    }

    #[test]
    fn grid_defaults_and_override_lifecycle_work() {
        let mut manager = EnvironmentManager::new(
            weather_series(),
            schedule_series(),
            &building(Some(21.0)),
            StdDuration::from_secs(60),
            utc_offset().with_ymd_and_hms(1970, 1, 1, 0, 0, 0).unwrap(),
            None,
        )
        .expect("manager");

        let env_default = manager.update(&clock(), &[]);
        assert_eq!(env_default.grid.voltage_pu, 1.0);
        assert_eq!(env_default.grid.frequency_hz, 60.0);

        manager.set_grid_override(GridState {
            voltage_pu: 0.95,
            frequency_hz: 59.8,
        });
        let env_override = manager.update(&clock(), &[]);
        assert_eq!(env_override.grid.voltage_pu, 0.95);
        assert_eq!(env_override.grid.frequency_hz, 59.8);

        manager.clear_grid_override();
        let env_cleared = manager.update(&clock(), &[]);
        assert_eq!(env_cleared.grid.voltage_pu, 1.0);
    }

    #[test]
    fn missing_setpoints_falls_back_to_ochre_default_21c() {
        let mut manager = EnvironmentManager::new(
            weather_series(),
            schedule_series(),
            &building(None),
            StdDuration::from_secs(60),
            utc_offset().with_ymd_and_hms(1970, 1, 1, 0, 0, 0).unwrap(),
            None,
        )
        .expect("manager");
        let env = manager.update(&clock(), &[]);
        assert_eq!(env.zones[0].temperature_c, 21.0);
    }

    /// Clear-sky solar noon on a south-facing tilted surface must produce non-zero
    /// diffuse irradiance via the Perez circumsolar term.
    ///
    /// With high DNI and moderate DHI, the Perez model applies a circumsolar
    /// brightening correction that makes `diffuse_w_m2 > 0` on a tilted surface,
    /// whereas a simple isotropic model would give much less or zero circumsolar
    /// contribution for surfaces tilted toward the sun.
    #[test]
    fn perez_model_produces_nonzero_diffuse_on_south_tilted_surface() {
        // June 21 at solar noon for a south-facing building at lat 40°N.
        // High DNI (800), moderate DHI (100) — Perez conditions satisfied (dhi >= 1).
        let mut weather = weather_series();
        weather.dni_w_m2 = vec![800.0, 800.0];
        weather.dhi_w_m2 = vec![100.0, 100.0];
        weather.ghi_w_m2 = vec![700.0, 700.0];

        let mut manager = EnvironmentManager::new(
            weather,
            schedule_series(),
            &building(Some(21.0)),
            StdDuration::from_secs(3600),
            utc_offset().with_ymd_and_hms(1970, 1, 1, 0, 0, 0).unwrap(),
            None,
        )
        .expect("manager");

        // Solar noon on June 21 UTC at longitude 0°, lat 40°N.
        // The test clock starts 2024-06-21T12:00:00Z — solar altitude > 60°.
        let env = manager.update(&clock(), &[]);

        // The building fixture has a south-facing vertical wall (azimuth 180°, tilt 90°).
        // Perez circumsolar term must produce diffuse_w_m2 > 0.
        let south_wall = env.weather.solar_irradiance.first().expect("surface");
        assert!(
            south_wall.diffuse_w_m2 > 0.0,
            "Perez model must produce non-zero diffuse on south-facing tilted surface, got {}",
            south_wall.diffuse_w_m2
        );
    }

    #[test]
    fn wet_bulb_and_enthalpy_are_derived_from_weather() {
        let mut manager = EnvironmentManager::new(
            weather_series(),
            schedule_series(),
            &building(Some(21.0)),
            StdDuration::from_secs(60),
            utc_offset().with_ymd_and_hms(1970, 1, 1, 0, 0, 0).unwrap(),
            None,
        )
        .expect("manager");
        let env = manager.update(&clock(), &[]);

        // Wet-bulb must be <= dry-bulb and finite.
        let wb = env.weather.outdoor_wet_bulb_c;
        let db = env.weather.outdoor_temp_c;
        assert!(wb.is_finite(), "wet-bulb must be finite");
        assert!(
            wb <= db + 0.01,
            "wet-bulb ({wb}) must be <= dry-bulb ({db})"
        );

        // Enthalpy at 10°C dry-bulb should be a small positive number.
        let h = env.weather.outdoor_enthalpy_j_kg;
        assert!(h.is_finite(), "enthalpy must be finite");
        assert!(h > 0.0, "enthalpy must be positive at 10°C");
    }

    /// Weather offset: simulation starting mid-year reads the correct row.
    /// With the EPW 30-min midpoint shift, starting at 02:30 LST aligns with
    /// the EPW row covering 02:00–03:00 (index 2, temp = 20°C).
    #[test]
    fn weather_offset_reads_correct_row_for_midyear_start() {
        // Build 4-row hourly weather: temps = [-10, 5, 20, 35]
        let mut weather = weather_series();
        weather.dry_bulb_c = vec![-10.0, 5.0, 20.0, 35.0];
        weather.dew_point_c = vec![-15.0, 0.0, 10.0, 20.0];
        weather.rel_humidity_pct = vec![50.0; 4];
        weather.pressure_kpa = vec![101.3; 4];
        weather.ghi_w_m2 = vec![0.0; 4];
        weather.dni_w_m2 = vec![0.0; 4];
        weather.dhi_w_m2 = vec![0.0; 4];
        weather.wind_speed_m_s = vec![3.0; 4];
        weather.wind_dir_deg = vec![180.0; 4];
        weather.opaque_sky_cover = vec![2.0; 4];
        weather.horizontal_infrared_w_m2 = vec![300.0; 4];
        weather.sky_temp_c = vec![5.0; 4];
        weather.ground_temp_c = vec![8.0; 4];
        weather.liquid_precip_m = vec![0.0; 4];
        weather.surface_albedo = None;

        // Simulation starts at 02:30 LST: midpoint shift places this at row 2 (20°C).
        // seconds_into_year = 9000, shifted = 9000 - 1800 = 7200, 7200/3600 = 2.
        let start = utc_offset().with_ymd_and_hms(2024, 1, 1, 2, 30, 0).unwrap();
        let mut manager = EnvironmentManager::new(
            weather,
            schedule_series(),
            &building(Some(21.0)),
            StdDuration::from_secs(3600),
            start,
            None,
        )
        .expect("manager");

        let clock = SimClock::new(start, Duration::hours(1), Duration::hours(2));
        let env = manager.update(&clock, &[]);
        assert!(
            (env.weather.outdoor_temp_c - 20.0).abs() < 1e-6,
            "step 0 with offset 2 should read row 2 (20°C), got {}",
            env.weather.outdoor_temp_c
        );
    }

    /// compute_annual_offset: leap year (2024) — May 5 noon.
    /// Feb has 29 days so May 5 = ordinal 126, ordinal0 = 125.
    /// With 30-min EPW midpoint shift: (125*86400 + 12*3600 - 1800) / 3600 = 3011.
    #[test]
    fn annual_offset_leap_year_may_5_noon() {
        let meta = weather_series().meta;
        let start = utc_offset().with_ymd_and_hms(2024, 5, 5, 12, 0, 0).unwrap();
        let offset = compute_annual_offset(&meta, start, 3600);
        assert_eq!(offset, 3011);
    }

    /// compute_annual_offset: non-leap year (2023) — May 5 noon.
    /// Feb has 28 days so May 5 = ordinal 125, ordinal0 = 124.
    /// With 30-min EPW midpoint shift: (124*86400 + 12*3600 - 1800) / 3600 = 2987.
    #[test]
    fn annual_offset_non_leap_year_may_5_noon() {
        let meta = weather_series().meta;
        let start = utc_offset().with_ymd_and_hms(2023, 5, 5, 12, 0, 0).unwrap();
        let offset = compute_annual_offset(&meta, start, 3600);
        assert_eq!(offset, 2987);
    }

    /// compute_annual_offset: leap year Jan 1 00:00.
    /// Shift wraps to last step of previous year: (366*86400 - 1800) / 3600 = 8783.
    #[test]
    fn annual_offset_leap_year_jan_1() {
        let meta = weather_series().meta;
        let start = utc_offset().with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        assert_eq!(compute_annual_offset(&meta, start, 3600), 8783);
    }

    /// compute_annual_offset: non-leap year Jan 1 00:00.
    /// Shift wraps to last step of previous year: (365*86400 - 1800) / 3600 = 8759.
    #[test]
    fn annual_offset_non_leap_year_jan_1() {
        let meta = weather_series().meta;
        let start = utc_offset().with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap();
        assert_eq!(compute_annual_offset(&meta, start, 3600), 8759);
    }

    /// compute_annual_offset: leap year Dec 31 23:00.
    /// With 30-min shift: (365*86400 + 23*3600 - 1800) / 3600 = 8782.
    #[test]
    fn annual_offset_leap_year_dec_31() {
        let meta = weather_series().meta;
        let start = utc_offset()
            .with_ymd_and_hms(2024, 12, 31, 23, 0, 0)
            .unwrap();
        assert_eq!(compute_annual_offset(&meta, start, 3600), 8782);
    }

    /// compute_annual_offset: non-leap year Dec 31 23:00.
    /// With 30-min shift: (364*86400 + 23*3600 - 1800) / 3600 = 8758.
    #[test]
    fn annual_offset_non_leap_year_dec_31() {
        let meta = weather_series().meta;
        let start = utc_offset()
            .with_ymd_and_hms(2023, 12, 31, 23, 0, 0)
            .unwrap();
        assert_eq!(compute_annual_offset(&meta, start, 3600), 8758);
    }

    /// compute_annual_offset: leap year Feb 29 → ordinal0 = 59.
    /// With 30-min shift: (59*86400 + 6*3600 - 1800) / 3600 = 1421.
    #[test]
    fn annual_offset_leap_year_feb_29() {
        let meta = weather_series().meta;
        let start = utc_offset().with_ymd_and_hms(2024, 2, 29, 6, 0, 0).unwrap();
        assert_eq!(compute_annual_offset(&meta, start, 3600), 1421);
    }

    /// compute_annual_offset: non-leap year Mar 1 → ordinal0 = 59.
    /// With 30-min shift: (59*86400 + 6*3600 - 1800) / 3600 = 1421.
    #[test]
    fn annual_offset_non_leap_year_mar_1() {
        let meta = weather_series().meta;
        let start = utc_offset().with_ymd_and_hms(2023, 3, 1, 6, 0, 0).unwrap();
        assert_eq!(compute_annual_offset(&meta, start, 3600), 1421);
    }

    /// compute_annual_offset: sub-hourly resolution (15-min steps).
    /// start = 2023-01-01T01:30:00: seconds_into_year=5400, shifted=3600, 3600/900=4.
    #[test]
    fn annual_offset_15min_resolution() {
        let meta = weather_series().meta;
        let start = utc_offset().with_ymd_and_hms(2023, 1, 1, 1, 30, 0).unwrap();
        assert_eq!(compute_annual_offset(&meta, start, 900), 4);
    }

    /// Various climate offsets produce correct temperatures.
    #[test]
    fn weather_offset_winter_vs_summer() {
        // 8760-row annual weather: index i has temp = -20 + i * (50/8760)
        // So index 0 = -20°C (Jan), index 4380 ≈ 5°C (Jul)
        let n = 8760;
        let mut weather = weather_series();
        weather.dry_bulb_c = (0..n)
            .map(|i| -20.0 + (i as f64) * 50.0 / (n as f64))
            .collect();
        weather.dew_point_c = vec![0.0; n];
        weather.rel_humidity_pct = vec![50.0; n];
        weather.pressure_kpa = vec![101.3; n];
        weather.ghi_w_m2 = vec![0.0; n];
        weather.dni_w_m2 = vec![0.0; n];
        weather.dhi_w_m2 = vec![0.0; n];
        weather.wind_speed_m_s = vec![3.0; n];
        weather.wind_dir_deg = vec![180.0; n];
        weather.opaque_sky_cover = vec![2.0; n];
        weather.horizontal_infrared_w_m2 = vec![300.0; n];
        weather.sky_temp_c = vec![5.0; n];
        weather.ground_temp_c = vec![8.0; n];
        weather.liquid_precip_m = vec![0.0; n];
        weather.surface_albedo = None;

        // Start Jan 1 00:00 → offset 0 → temp ≈ -20°C
        let jan_start = utc_offset().with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let mut mgr = EnvironmentManager::new(
            weather.clone(),
            schedule_series(),
            &building(Some(21.0)),
            StdDuration::from_secs(3600),
            jan_start,
            None,
        )
        .expect("jan");
        let clock_jan = SimClock::new(jan_start, Duration::hours(1), Duration::hours(1));
        let env_jan = mgr.update(&clock_jan, &[]);
        assert!(
            env_jan.weather.outdoor_temp_c < -15.0,
            "January start should be cold, got {}",
            env_jan.weather.outdoor_temp_c
        );
        let mains_jan = env_jan
            .custom_domains
            .iter()
            .find(|d| d.domain_id == MAINS_WATER_DOMAIN_ID)
            .and_then(|d| d.custom_payload.as_ref())
            .and_then(|p| p.first())
            .copied()
            .expect("mains domain payload");
        assert!(
            (2.0..=8.0).contains(&mains_jan),
            "January mains should be near winter trough (2-8C), got {}",
            mains_jan
        );

        // Start Jul 1 00:00 → offset ~4344 → temp should be positive
        let jul_start = utc_offset().with_ymd_and_hms(2024, 7, 1, 0, 0, 0).unwrap();
        let mut mgr = EnvironmentManager::new(
            weather,
            schedule_series(),
            &building(Some(21.0)),
            StdDuration::from_secs(3600),
            jul_start,
            None,
        )
        .expect("jul");
        let clock_jul = SimClock::new(jul_start, Duration::hours(1), Duration::hours(1));
        let env_jul = mgr.update(&clock_jul, &[]);
        assert!(
            env_jul.weather.outdoor_temp_c > 0.0,
            "July start should be warm, got {}",
            env_jul.weather.outdoor_temp_c
        );
        let mains_jul = env_jul
            .custom_domains
            .iter()
            .find(|d| d.domain_id == MAINS_WATER_DOMAIN_ID)
            .and_then(|d| d.custom_payload.as_ref())
            .and_then(|p| p.first())
            .copied()
            .expect("mains domain payload");
        assert!(
            (12.0..=22.0).contains(&mains_jul),
            "July mains should be near summer peak (12-22C), got {}",
            mains_jul
        );
        assert!(
            mains_jul > mains_jan + 5.0,
            "mains temperature should vary seasonally, jan={} jul={}",
            mains_jan,
            mains_jul
        );
    }

    /// `WeatherState.mains_temp_c` must be populated from the Burch-Christensen
    /// model and differ from the static 15°C fallback for a real climate.
    #[test]
    fn weather_state_mains_temp_c_is_populated_and_not_constant() {
        // 8760-row annual weather with enough seasonal variation to produce
        // a mains temperature that differs from the 15°C fallback.
        let n = 8760;
        let mut weather = weather_series();
        weather.dry_bulb_c = (0..n)
            .map(|i| -10.0 + (i as f64) * 30.0 / (n as f64))
            .collect();
        weather.dew_point_c = vec![0.0; n];
        weather.rel_humidity_pct = vec![50.0; n];
        weather.pressure_kpa = vec![101.3; n];
        weather.ghi_w_m2 = vec![0.0; n];
        weather.dni_w_m2 = vec![0.0; n];
        weather.dhi_w_m2 = vec![0.0; n];
        weather.wind_speed_m_s = vec![3.0; n];
        weather.wind_dir_deg = vec![180.0; n];
        weather.opaque_sky_cover = vec![2.0; n];
        weather.horizontal_infrared_w_m2 = vec![300.0; n];
        weather.sky_temp_c = vec![5.0; n];
        weather.ground_temp_c = vec![8.0; n];
        weather.liquid_precip_m = vec![0.0; n];
        weather.surface_albedo = None;

        let start = utc_offset().with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let mut mgr = EnvironmentManager::new(
            weather,
            schedule_series(),
            &building(Some(21.0)),
            StdDuration::from_secs(3600),
            start,
            None,
        )
        .expect("manager");

        let clock = SimClock::new(start, Duration::hours(1), Duration::hours(1));
        let env = mgr.update(&clock, &[]);

        // The field must be finite and populated.
        assert!(
            env.weather.mains_temp_c.is_finite(),
            "mains_temp_c must be finite, got {}",
            env.weather.mains_temp_c
        );
        // For a moderate US-like climate the annual average is ~5°C, so mains in
        // winter (January) will be well below the 15°C fallback default.
        assert!(
            env.weather.mains_temp_c < 15.0,
            "January mains_temp_c ({}) should be below 15°C fallback for a cold-start climate",
            env.weather.mains_temp_c
        );
    }

    /// For a Northern-hemisphere site, mains temp at day 1 (winter) must be
    /// strictly lower than mains temp at day 180 (summer).
    #[test]
    fn mains_temp_c_winter_lower_than_summer_northern_hemisphere() {
        let n = 8760;
        let mut weather = weather_series();
        // Moderate US climate: annual avg ~10°C, range ~28°C.
        weather.dry_bulb_c = (0..n)
            .map(|i| {
                let day_frac = (i as f64) / (n as f64);
                10.0 + 14.0 * (2.0 * std::f64::consts::PI * (day_frac - 0.5)).sin()
            })
            .collect();
        weather.dew_point_c = vec![0.0; n];
        weather.rel_humidity_pct = vec![50.0; n];
        weather.pressure_kpa = vec![101.3; n];
        weather.ghi_w_m2 = vec![0.0; n];
        weather.dni_w_m2 = vec![0.0; n];
        weather.dhi_w_m2 = vec![0.0; n];
        weather.wind_speed_m_s = vec![3.0; n];
        weather.wind_dir_deg = vec![180.0; n];
        weather.opaque_sky_cover = vec![2.0; n];
        weather.horizontal_infrared_w_m2 = vec![300.0; n];
        weather.sky_temp_c = vec![5.0; n];
        weather.ground_temp_c = vec![8.0; n];
        weather.liquid_precip_m = vec![0.0; n];
        weather.surface_albedo = None;

        // Day 1 of year (Jan 1, winter).
        let winter_start = utc_offset().with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let mut mgr_winter = EnvironmentManager::new(
            weather.clone(),
            schedule_series(),
            &building(Some(21.0)),
            StdDuration::from_secs(3600),
            winter_start,
            None,
        )
        .expect("winter manager");
        let clock_winter = SimClock::new(winter_start, Duration::hours(1), Duration::hours(1));
        let env_winter = mgr_winter.update(&clock_winter, &[]);
        let mains_winter = env_winter.weather.mains_temp_c;

        // Day 180 of year (~late June, summer).
        let summer_start = utc_offset().with_ymd_and_hms(2024, 6, 28, 0, 0, 0).unwrap();
        let mut mgr_summer = EnvironmentManager::new(
            weather,
            schedule_series(),
            &building(Some(21.0)),
            StdDuration::from_secs(3600),
            summer_start,
            None,
        )
        .expect("summer manager");
        let clock_summer = SimClock::new(summer_start, Duration::hours(1), Duration::hours(1));
        let env_summer = mgr_summer.update(&clock_summer, &[]);
        let mains_summer = env_summer.weather.mains_temp_c;

        assert!(
            mains_winter < mains_summer,
            "Northern hemisphere: winter mains ({mains_winter:.2}°C) must be \
             lower than summer mains ({mains_summer:.2}°C)"
        );
    }

    // ---- DST-aware schedule tests (require `dst` feature) ----

    #[cfg(feature = "dst")]
    mod dst_tests {
        use super::*;

        /// Build an annual hourly schedule (8760 rows) where column 0 holds
        /// `value = hour_of_year` (0..8759), making it easy to verify which
        /// schedule row was selected.
        fn annual_hourly_schedule() -> ScheduleTimeSeries {
            let n = 8760;
            let start = utc_offset()
                .with_ymd_and_hms(2024, 1, 1, 0, 0, 0)
                .unwrap();
            let timestamps: Vec<DateTime<FixedOffset>> = (0..n)
                .map(|i| start + Duration::hours(i as i64))
                .collect();
            let values: Vec<f64> = (0..n).map(|i| i as f64).collect();
            let mut index = HashMap::new();
            index.insert("hour_idx".to_string(), 0);
            ScheduleTimeSeries {
                timestamps,
                column_names: vec!["hour_idx".to_string()],
                columns: vec![values],
                column_index: index,
                source_step_secs: 3600,
                column_aggregations: vec![],
            }
        }

        /// Build an annual hourly weather series (8760 rows) with constant
        /// values. Only the dry-bulb field varies so we can verify weather
        /// indexing is independent of DST.
        fn annual_hourly_weather() -> WeatherTimeSeries {
            let n = 8760;
            WeatherTimeSeries {
                meta: WeatherMeta {
                    location: "Test".to_string(),
                    latitude: 40.0,
                    longitude: -74.0,
                    timezone_offset_h: -5.0,
                    elevation_m: 10.0,
                    source_step_secs: 3600,
                },
                dry_bulb_c: (0..n).map(|i| i as f64 * 0.01).collect(),
                dew_point_c: vec![2.0; n],
                rel_humidity_pct: vec![50.0; n],
                pressure_kpa: vec![101.3; n],
                ghi_w_m2: vec![0.0; n],
                dni_w_m2: vec![0.0; n],
                dhi_w_m2: vec![0.0; n],
                wind_speed_m_s: vec![3.0; n],
                wind_dir_deg: vec![180.0; n],
                opaque_sky_cover: vec![2.0; n],
                horizontal_infrared_w_m2: vec![300.0; n],
                sky_temp_c: vec![5.0; n],
                ground_temp_c: vec![8.0; n],
                liquid_precip_m: vec![0.0; n],
                surface_albedo: None,
            }
        }

        /// Helper: extract the first schedule value from an EnvironmentState.
        fn schedule_val(env: &EnvironmentState) -> f64 {
            env.custom_domains
                .iter()
                .find(|d| d.domain_id == schedule_domain_id())
                .and_then(|d| d.custom_payload.as_ref())
                .and_then(|p| p.first())
                .copied()
                .expect("schedule domain payload")
        }

        /// `None` civil_timezone produces the same schedule index as the
        /// pre-DST fixed-offset behavior.
        #[test]
        fn schedule_without_dst_unchanged() {
            let start = utc_offset()
                .with_ymd_and_hms(2024, 3, 10, 6, 0, 0)
                .unwrap();
            let mut mgr_no_dst = EnvironmentManager::new(
                annual_hourly_weather(),
                annual_hourly_schedule(),
                &building(Some(21.0)),
                StdDuration::from_secs(3600),
                start,
                None,
            )
            .expect("no-dst manager");
            let clock = SimClock::new(start, Duration::hours(1), Duration::hours(4));
            let env = mgr_no_dst.update(&clock, &[]);
            // Hour 6 of day 69 (March 10, leap year 2024): schedule row =
            // 69 * 24 + 6 = 1662.
            let val = schedule_val(&env);
            assert!(
                (val - 1662.0).abs() < 1e-6,
                "expected schedule row 1662, got {val}"
            );
        }

        /// Spring forward (America/New_York, 2024-03-10 at 2:00 AM EST → 3:00 AM EDT):
        /// Civil time jumps from 1:59:59 to 3:00:00. Schedule row for civil
        /// hour 2 AM is never accessed; hour 3 AM is used instead.
        #[test]
        fn schedule_spring_forward_skips_civil_hour() {
            // EST = UTC-5. At 2024-03-10T07:00:00Z the wall clock is 2:00 AM EST,
            // which is the instant of spring-forward → becomes 3:00 AM EDT.
            let est = FixedOffset::west_opt(5 * 3600).expect("offset");
            let start = est.with_ymd_and_hms(2024, 3, 10, 1, 0, 0).unwrap();

            let mut mgr = EnvironmentManager::new(
                annual_hourly_weather(),
                annual_hourly_schedule(),
                &building(Some(21.0)),
                StdDuration::from_secs(3600),
                start,
                Some("America/New_York"),
            )
            .expect("dst manager");

            // Step 0 → civil 1:00 AM EST, Step 1 → civil 3:00 AM EDT (skips 2 AM).
            let mut clock = SimClock::new(start, Duration::hours(1), Duration::hours(4));
            let env0 = mgr.update(&clock, &[]);
            let val0 = schedule_val(&env0);
            // Day 69 (March 10), hour 1: row = 69*24 + 1 = 1657.
            assert!(
                (val0 - 1657.0).abs() < 1e-6,
                "step 0: expected civil hour 1 (row 1657), got {val0}"
            );

            let _ = clock.next(); // advance to step 1
            let env1 = mgr.update(&clock, &[]);
            let val1 = schedule_val(&env1);
            // Civil time is now 3:00 AM EDT (skipped 2 AM). Row = 69*24 + 3 = 1659.
            assert!(
                (val1 - 1659.0).abs() < 1e-6,
                "step 1: expected civil hour 3 (row 1659, spring-forward skip), got {val1}"
            );
        }

        /// Fall back (America/New_York, 2024-11-03 at 2:00 AM EDT → 1:00 AM EST):
        /// Civil time 1:00 AM occurs twice. The schedule row for civil hour 1 AM
        /// is reused for both occurrences.
        #[test]
        fn schedule_fall_back_reuses_civil_hour() {
            // EDT = UTC-4. At 2024-11-03T05:00:00Z the wall clock is 1:00 AM EDT.
            // One hour later (06:00Z), clocks fall back: 1:00 AM EST again.
            let edt = FixedOffset::west_opt(4 * 3600).expect("offset");
            let start = edt.with_ymd_and_hms(2024, 11, 3, 0, 0, 0).unwrap();

            let mut mgr = EnvironmentManager::new(
                annual_hourly_weather(),
                annual_hourly_schedule(),
                &building(Some(21.0)),
                StdDuration::from_secs(3600),
                start,
                Some("America/New_York"),
            )
            .expect("dst manager");

            // Step 1 → civil 1:00 AM EDT (first occurrence).
            let mut clock = SimClock::new(start, Duration::hours(1), Duration::hours(6));
            let _ = clock.next(); // step 1
            let env1 = mgr.update(&clock, &[]);
            let val1 = schedule_val(&env1);
            // Nov 3, 2024 = day 307 (ordinal0). Hour 1: row = 307*24 + 1 = 7369.
            assert!(
                (val1 - 7369.0).abs() < 1e-6,
                "first 1 AM: expected row 7369, got {val1}"
            );

            let _ = clock.next(); // step 2 → civil 2:00 AM EDT → falls back to 1:00 AM EST
            let env2 = mgr.update(&clock, &[]);
            let val2 = schedule_val(&env2);
            // Civil time is 1:00 AM EST (second occurrence). Same row 7369.
            assert!(
                (val2 - 7369.0).abs() < 1e-6,
                "second 1 AM (fall-back): expected row 7369, got {val2}"
            );
        }

        /// Weather state is identical regardless of whether DST is enabled.
        /// DST only affects schedule indexing, never weather.
        #[test]
        fn weather_unaffected_by_dst_setting() {
            let est = FixedOffset::west_opt(5 * 3600).expect("offset");
            let start = est.with_ymd_and_hms(2024, 6, 21, 12, 0, 0).unwrap();
            let weather = annual_hourly_weather();

            let mut mgr_no_dst = EnvironmentManager::new(
                weather.clone(),
                annual_hourly_schedule(),
                &building(Some(21.0)),
                StdDuration::from_secs(3600),
                start,
                None,
            )
            .expect("no-dst");

            let mut mgr_dst = EnvironmentManager::new(
                weather,
                annual_hourly_schedule(),
                &building(Some(21.0)),
                StdDuration::from_secs(3600),
                start,
                Some("America/Denver"),
            )
            .expect("dst");

            let clock = SimClock::new(start, Duration::hours(1), Duration::hours(4));
            let env_no = mgr_no_dst.update(&clock, &[]);
            let env_yes = mgr_dst.update(&clock, &[]);

            assert_eq!(
                env_no.weather.outdoor_temp_c, env_yes.weather.outdoor_temp_c,
                "dry-bulb must match"
            );
            assert_eq!(
                env_no.weather.pressure_kpa, env_yes.weather.pressure_kpa,
                "pressure must match"
            );
            assert_eq!(
                env_no.weather.wind_speed_m_s, env_yes.weather.wind_speed_m_s,
                "wind speed must match"
            );
        }

        /// A full-year simulation with DST verifies that schedule rows align
        /// with civil time at every hour.
        #[test]
        fn year_round_schedule_alignment() {
            // Use 2023 (non-leap year = 8760 hours) so the loop covers the full year.
            let est = FixedOffset::west_opt(5 * 3600).expect("offset");
            let start = est.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap();

            let mut mgr = EnvironmentManager::new(
                annual_hourly_weather(),
                annual_hourly_schedule(),
                &building(Some(21.0)),
                StdDuration::from_secs(3600),
                start,
                Some("America/New_York"),
            )
            .expect("year-round manager");

            let mut clock = SimClock::new(
                start,
                Duration::hours(1),
                Duration::hours(8760),
            );

            let tz: chrono_tz::Tz = "America/New_York".parse().unwrap();
            for step in 0..8760u64 {
                let sim_time = clock.current_time();
                let civil = sim_time.with_timezone(&tz);
                let expected_row = civil.ordinal0() as u64 * 24 + civil.hour() as u64;
                let env = mgr.update(&clock, &[]);
                let got = schedule_val(&env);
                let expected = (expected_row as usize % 8760) as f64;
                assert!(
                    (got - expected).abs() < 1e-6,
                    "step {step}: civil {civil}, expected row {expected}, got {got}"
                );
                if clock.next().is_none() {
                    break;
                }
            }
        }

        /// Invalid timezone string returns an error.
        #[test]
        fn invalid_timezone_returns_error() {
            let start = utc_offset()
                .with_ymd_and_hms(2024, 1, 1, 0, 0, 0)
                .unwrap();
            let result = EnvironmentManager::new(
                annual_hourly_weather(),
                annual_hourly_schedule(),
                &building(Some(21.0)),
                StdDuration::from_secs(3600),
                start,
                Some("Not/A/Timezone"),
            );
            assert!(
                matches!(result, Err(EnvironmentManagerError::InvalidTimezone(_))),
                "expected InvalidTimezone error, got {result:?}"
            );
        }
    }
}
