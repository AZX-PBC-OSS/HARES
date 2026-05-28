//! Outdoor environment state (weather, grid signals).

use std::time::Duration as StdDuration;

#[cfg(test)]
use chrono::Duration;
use chrono::{DateTime, Datelike, FixedOffset, Timelike};
use hares_io::hpxml::building::{BoundaryType, ZoneType};
use hares_io::{Building, ScheduleTimeSeries, WeatherMeta, WeatherTimeSeries, monthly_day_counts};
use hares_io::{schedule::ScheduleError, weather::WeatherError, weather::WeatherField};
use hares_physics::{
    psychrometrics::{humidity_ratio_from_tdp, moist_air_enthalpy, wet_bulb_from_humidity_ratio},
    solar::{
        OMNI_AZIMUTH_SAMPLES, omni_directional_irradiance, perez_tilted_irradiance, solar_position,
    },
    water_mains::{Hemisphere, water_mains_temperature_c},
};
use hares_types::{
    DomainId, EnvironmentState, GridState, SCHEDULE_DOMAIN_ID, SurfaceIrradiance, WeatherState,
    ZoneId, ZoneState,
};
use rand::RngExt;
use rand_chacha::ChaCha8Rng;
use thiserror::Error;

use crate::SimClock;

const DEFAULT_ZONE_VOLUME_M3: f64 = 200.0;
const DEFAULT_GRID_VOLTAGE_PU: f64 = 1.0;
const DEFAULT_GRID_FREQUENCY_HZ: f64 = 60.0;
const MAINS_WATER_DOMAIN_ID: DomainId = DomainId(u16::MAX - 1);

/// Geometry required for solar irradiance projection.
///
/// When `omni_directional` is true, the surface has no known azimuth and
/// solar irradiance is computed as the azimuth-averaged Perez result rather
/// than a single-azimuth projection. `azimuth_deg` is retained at 180° for
/// non-solar consumers (LWR view factors, exterior film coefficients, etc.)
/// as documented in the Known Limitations.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceGeometry {
    pub surface_id: u32,
    pub azimuth_deg: f64,
    pub tilt_deg: f64,
    pub area_m2: f64,
    /// True when azimuth is unknown for walls/roofs (HPXML permits omission).
    /// Solar irradiance is computed via azimuth-sampled Perez averaging
    /// rather than a single-azimuth projection.
    pub omni_directional: bool,
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
    #[error(
        "surface {surface_type} at boundary index {boundary_idx} is missing azimuth; orientation is required for solar-receiving surfaces"
    )]
    MissingAzimuth {
        boundary_idx: usize,
        surface_type: String,
    },
}

#[derive(Debug, Clone, Default)]
pub struct EnvironmentInitOptions<'a> {
    pub civil_timezone: Option<&'a str>,
    pub resample_overrides: Option<&'a hares_io::ResampleOverrides>,
    pub initial_rng: Option<ChaCha8Rng>,
    pub setpoint_deadband_c: Option<f64>,
}

/// Produces a complete [`EnvironmentState`] at each timestep.
#[derive(Debug, Clone)]
pub struct EnvironmentManager {
    weather: WeatherTimeSeries,
    schedule: ScheduleTimeSeries,
    pub(crate) weather_meta: WeatherMeta,
    zone_types: Vec<hares_io::hpxml::ZoneType>,
    surfaces: Vec<SurfaceGeometry>,
    grid_override: Option<GridState>,
    zones: Vec<ZoneState>,
    /// Offset into weather/schedule arrays for simulation start.
    /// EPW files are annual starting Jan 1; if the simulation starts
    /// mid-year, this shifts the index so step 0 reads the right row.
    weather_start_offset: usize,
    mains_t_annual_avg_c: f64,
    mains_dt_annual_range_c: f64,
    mains_hemisphere: Hemisphere,
    /// Annual mean ground surface temperature [°C] for Kusuda-Achenbach model.
    ground_t_mean_c: f64,
    /// Half-amplitude of yearly ground surface temperature variation [°C].
    ground_t_amplitude_c: f64,
    /// Day of minimum ground surface temperature (phase shift).
    ground_phase_day: f64,
    /// When set, schedule indexing uses DST-aware civil time from this IANA
    /// timezone instead of the fixed UTC offset. Weather indexing is unaffected.
    #[cfg(feature = "dst")]
    civil_tz: Option<chrono_tz::Tz>,
    /// Optional pre-computed per-surface irradiance for each timestep.
    /// When set, bypasses the built-in Perez tilted irradiance computation
    /// in `update()`. Indexed as `solar_override[step % len][surface_idx]`.
    /// Use for parity testing with OCHRE (pvlib) or injecting PySAM/PVWatts data.
    solar_override: Option<Vec<Vec<SurfaceIrradiance>>>,
    /// Per-roof PV coverage fractions. When PV panels are attached to a roof
    /// surface, the covered fraction reduces incident solar irradiance on that
    /// envelope surface (shading effect).
    pv_roof_coverage: std::collections::HashMap<u32, f64>,
    // Pre-allocated buffers reused each step to avoid per-step allocations.
    solar_irradiance_buf: Vec<SurfaceIrradiance>,
    schedule_values_buf: Vec<f64>,
    mains_payload_buf: Vec<f64>,
    schedule_payload_swap: Vec<f64>,
    mains_payload_swap: Vec<f64>,
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
        Self::new_with_resample(
            weather,
            schedule,
            building,
            time_res,
            start_time,
            EnvironmentInitOptions {
                civil_timezone,
                ..EnvironmentInitOptions::default()
            },
        )
    }

    /// Like [`new`] but with optional per-column weather resampling overrides.
    ///
    /// Pass `Some(ResampleOverrides::ochre_compat())` for OCHRE parity testing.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_resample(
        weather: WeatherTimeSeries,
        schedule: ScheduleTimeSeries,
        building: &Building,
        time_res: StdDuration,
        start_time: DateTime<FixedOffset>,
        options: EnvironmentInitOptions<'_>,
    ) -> Result<Self, EnvironmentManagerError> {
        let step_secs = u32::try_from(time_res.as_secs()).unwrap_or(u32::MAX);
        if step_secs == 0 {
            return Err(EnvironmentManagerError::ZeroTimeResolution);
        }

        let weather = match options.resample_overrides {
            Some(ov) => weather.resample_with(step_secs, ov)?,
            None => weather.resample(step_secs)?,
        };
        if weather.is_empty() {
            return Err(EnvironmentManagerError::EmptyWeather);
        }

        let schedule = schedule.resample(step_secs)?;
        if schedule.is_empty() {
            return Err(EnvironmentManagerError::EmptySchedule);
        }

        // Parse civil timezone for DST-aware schedule indexing.
        #[cfg(feature = "dst")]
        let civil_tz: Option<chrono_tz::Tz> = match options.civil_timezone {
            Some(name) => Some(
                name.parse::<chrono_tz::Tz>()
                    .map_err(|_| EnvironmentManagerError::InvalidTimezone(name.to_owned()))?,
            ),
            None => None,
        };
        #[cfg(not(feature = "dst"))]
        if options.civil_timezone.is_some() {
            return Err(EnvironmentManagerError::DstNotEnabled);
        }

        // Validate timezone offset: mismatched offset silently produces wrong solar position.
        // The caller is responsible for providing a correctly-offset local time; this
        // warning makes the error detectable without a breaking API change.
        let entry_offset_secs = start_time.offset().local_minus_utc();
        let weather_offset_secs = (weather.meta.timezone_offset_h * 3600.0).round() as i32;
        if (entry_offset_secs - weather_offset_secs).abs() > 1800 {
            tracing::warn!(
                entry_offset_h = entry_offset_secs as f64 / 3600.0,
                weather_offset_h = weather.meta.timezone_offset_h,
                "start_time timezone offset ({:.1}h) differs from weather file timezone offset \
                 ({:.1}h) by more than 0.5 hours; solar position will be incorrect",
                entry_offset_secs as f64 / 3600.0,
                weather.meta.timezone_offset_h,
            );
        }

        // Compute start offsets: weather and schedule time series are annual starting Jan 1.
        // Offset into them based on the simulation start time so step 0 reads the correct row.
        let weather_start_offset =
            compute_annual_offset(&weather.meta, start_time, step_secs, weather.len());
        let (mains_t_annual_avg_c, mains_dt_annual_range_c) =
            compute_mains_inputs(&weather, step_secs);

        let weather_meta = weather.meta.clone();
        let is_southern = weather_meta.latitude < 0.0;
        let mains_hemisphere = if is_southern {
            Hemisphere::Southern
        } else {
            Hemisphere::Northern
        };
        let ground_phase_day = if is_southern {
            hares_physics::ground::DEFAULT_PHASE_DAY_SOUTHERN
        } else {
            hares_physics::ground::DEFAULT_PHASE_DAY_NORTHERN
        };
        let zone_types = building
            .zones
            .iter()
            .map(|zone| zone.zone_type.clone())
            .collect();
        let surfaces = build_surface_geometry(building)?;
        // Use the shifted offset (with midpoint_offset_secs applied) for initial conditions
        // to match OCHRE's behavior of reading weather at the period midpoint.
        // EPW hour 12 (covers 11:00-12:00) has its representative value at 11:30.
        let init_offset =
            compute_annual_offset(&weather.meta, start_time, step_secs, weather.len());
        let initial_outdoor_temp_c = weather
            .dry_bulb_c
            .get(init_offset)
            .copied()
            .unwrap_or(DEFAULT_SETPOINT_C);
        let initial_ground_temp_c = weather
            .ground_temp_c
            .get(init_offset)
            .copied()
            .unwrap_or(initial_outdoor_temp_c);
        let zones = initial_zones(
            building,
            initial_outdoor_temp_c,
            initial_ground_temp_c,
            start_time,
            options.initial_rng,
            options.setpoint_deadband_c,
        )?;

        let num_surfaces = surfaces.len();
        let num_schedule_cols = schedule.columns.len();

        Ok(Self {
            weather,
            schedule,
            weather_meta,
            zone_types,
            surfaces,
            grid_override: None,
            zones,
            weather_start_offset,
            mains_t_annual_avg_c,
            mains_dt_annual_range_c,
            mains_hemisphere,
            ground_t_mean_c: mains_t_annual_avg_c,
            ground_t_amplitude_c: mains_dt_annual_range_c / 2.0,
            ground_phase_day,
            #[cfg(feature = "dst")]
            civil_tz,
            solar_override: None,
            pv_roof_coverage: std::collections::HashMap::new(),
            solar_irradiance_buf: Vec::with_capacity(num_surfaces),
            schedule_values_buf: Vec::with_capacity(num_schedule_cols),
            mains_payload_buf: vec![0.0],
            schedule_payload_swap: Vec::with_capacity(num_schedule_cols),
            mains_payload_swap: Vec::with_capacity(1),
        })
    }

    /// Set pre-computed per-surface irradiance for all timesteps.
    ///
    /// When set, `update()` reads from this table instead of computing Perez
    /// tilted irradiance from GHI/DNI/DHI. Use for parity testing with OCHRE
    /// (pvlib) or injecting PySAM/PVWatts data from Python.
    ///
    /// `data[step][surface_idx]` must match the surface geometry order from
    /// `build_surface_geometry()` (same as `building.boundaries` order).
    pub fn set_solar_override(&mut self, data: Vec<Vec<SurfaceIrradiance>>) {
        self.solar_override = Some(data);
    }

    /// Clear the solar override, reverting to built-in Perez computation.
    pub fn clear_solar_override(&mut self) {
        self.solar_override = None;
    }

    /// Returns whether a solar override is currently active.
    pub fn has_solar_override(&self) -> bool {
        self.solar_override.is_some()
    }

    /// Number of surfaces in the surface geometry array.
    pub fn surface_count(&self) -> usize {
        self.surfaces.len()
    }

    /// Surface geometry for building Python/external override data.
    pub fn surface_geometry(&self) -> &[SurfaceGeometry] {
        &self.surfaces
    }

    /// Register an additional surface for Perez irradiance computation.
    ///
    /// Used to add PV array orientations that don't correspond to an envelope
    /// boundary. Deduplicates by `surface_id`.
    pub fn register_surface(&mut self, geom: SurfaceGeometry) {
        if !self
            .surfaces
            .iter()
            .any(|s| s.surface_id == geom.surface_id)
        {
            self.surfaces.push(geom);
            self.solar_irradiance_buf.reserve(1);
        }
    }

    /// Record the fraction of a roof surface covered by PV panels.
    ///
    /// When nonzero, `update()` reduces incident solar irradiance on the
    /// corresponding envelope surface by `(1 - coverage)` to model shading.
    pub fn set_pv_roof_coverage(&mut self, surface_id: u32, fraction: f64) {
        self.pv_roof_coverage
            .insert(surface_id, fraction.clamp(0.0, 1.0));
    }

    /// Override default grid state for subsequent updates.
    pub fn set_grid_override(&mut self, grid: GridState) {
        self.grid_override = Some(grid);
    }

    /// Clear any active grid override and restore defaults.
    pub fn clear_grid_override(&mut self) {
        self.grid_override = None;
    }

    /// Kusuda-Achenbach ground temperature at a given depth and day of year.
    ///
    /// Uses annual climate statistics derived from weather data. More accurate
    /// than the surface ground temperature for foundation and slab boundaries.
    #[must_use]
    pub fn ground_temp_at_depth_c(&self, depth_m: f64, day_of_year: f64) -> f64 {
        hares_physics::ground::kusuda_achenbach_temp(
            depth_m,
            day_of_year,
            self.ground_t_mean_c,
            self.ground_t_amplitude_c,
            self.ground_phase_day,
            hares_physics::ground::DEFAULT_SOIL_DIFFUSIVITY_M2_PER_DAY,
        )
    }

    /// Compute the schedule array index for the current timestep.
    ///
    /// When a DST-aware civil timezone is configured, the schedule is indexed
    /// by civil (wall-clock) time so that occupancy/rate schedules follow local
    /// DST transitions naturally. Weather indexing is **not** affected -- solar
    /// position and meteorological data are physical quantities tied to UTC, not
    /// civil time.
    ///
    /// In the fallback (non-DST) path the index is also derived from calendar
    /// time, not a step counter. This ensures leap-year schedule alignment
    /// (T-0032) is correct regardless of DST configuration.
    fn compute_schedule_idx(&self, clock: &SimClock) -> usize {
        let schedule_len = self.schedule.len();

        #[cfg(feature = "dst")]
        if let Some(tz) = self.civil_tz {
            let sim_time = clock.current_time();
            let civil = sim_time.with_timezone(&tz);
            let mut doy0 = civil.ordinal0() as u64;
            // Schedules are always 365-day annual data, so leap-year dates after
            // Feb 29 must be adjusted to their non-leap ordinal to prevent the
            // one-day schedule misalignment described in T-0032.
            if is_leap_year(civil.year()) && doy0 > 59 {
                doy0 -= 1;
            }
            let h = civil.hour() as u64;
            let m = civil.minute() as u64;
            let s = civil.second() as u64;
            let civil_secs = doy0 * 86400 + h * 3600 + m * 60 + s;
            let step_secs = clock.time_res.num_seconds().unsigned_abs();
            if step_secs == 0 {
                return 0;
            }
            let year_secs = schedule_len as u64 * step_secs;
            return (civil_secs % year_secs / step_secs) as usize;
        }

        // Fallback: derive the schedule row from the current sim time with
        // the same leap-year doy0 adjustment as the DST path and compute_schedule_offset.
        let sim_time = clock.current_time();
        let mut doy0 = sim_time.ordinal0() as u64;
        if is_leap_year(sim_time.year()) && doy0 > 59 {
            doy0 -= 1;
        }
        let h = sim_time.hour() as u64;
        let m = sim_time.minute() as u64;
        let s = sim_time.second() as u64;
        let civil_secs = doy0 * 86400 + h * 3600 + m * 60 + s;
        let step_secs = clock.time_res.num_seconds().unsigned_abs();
        if step_secs == 0 {
            return 0;
        }
        let year_secs = schedule_len as u64 * step_secs;
        (civil_secs % year_secs / step_secs) as usize
    }
    #[must_use]
    pub fn schedule(&self) -> &ScheduleTimeSeries {
        &self.schedule
    }

    /// Mutably borrow the parsed schedule time series.
    pub fn schedule_mut(&mut self) -> &mut ScheduleTimeSeries {
        &mut self.schedule
    }

    /// Zone types aligned with [`EnvironmentState::zones`].
    #[must_use]
    pub fn zone_types(&self) -> &[hares_io::hpxml::ZoneType] {
        &self.zone_types
    }

    /// Returns the schedule column index for the occupancy column, if present.
    ///
    /// The returned index can be used to look up the occupancy value from the
    /// schedule payload in [`EnvironmentState::custom_domains`] at domain id
    /// [`hares_types::SCHEDULE_DOMAIN_ID`].
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

    /// Feed zone-state feedback into the internal zone buffer.
    ///
    /// Call this before [`update_in_place`] when the caller holds a borrow on
    /// the zone slice that would conflict with a simultaneous `&mut self`. This
    /// separates zone ingestion from the environment computation so the borrow
    /// checker can see two distinct phases.
    pub fn feed_zones(&mut self, zones: &[ZoneState]) {
        if !zones.is_empty() {
            self.zones.clear();
            self.zones.extend_from_slice(zones);
        }
    }

    /// Update `state` in-place for the current clock step.
    ///
    /// Reads zone temperatures from the internal buffer (populated by a prior
    /// [`feed_zones`] call or the previous [`update`] call). All heap-allocated
    /// fields inside `state` are cleared and refilled, reusing existing
    /// capacity and eliminating per-step allocations on the hot path.
    pub fn update_in_place(&mut self, state: &mut EnvironmentState, clock: &SimClock) {
        let step = usize::try_from(clock.current_step())
            .expect("step counter exceeds usize on this target");
        let weather_len = self.weather.len();
        let weather_idx = if weather_len == 0 {
            0
        } else {
            (step + self.weather_start_offset) % weather_len
        };
        let schedule_idx = if self.schedule.is_empty() {
            0
        } else {
            self.compute_schedule_idx(clock)
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

        // Step 2: per-surface solar irradiance -- compute into self.solar_irradiance_buf,
        // then swap into state.weather.solar_irradiance so the old Vec's capacity is
        // returned to self.solar_irradiance_buf for reuse next step.
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

        self.solar_irradiance_buf.clear();
        if let Some(ref overrides) = self.solar_override {
            let idx = (step + self.weather_start_offset) % overrides.len();
            self.solar_irradiance_buf.extend_from_slice(&overrides[idx]);
        } else {
            self.solar_irradiance_buf
                .extend(self.surfaces.iter().map(|surface| {
                    if surface.omni_directional {
                        // Omnidirectional (azimuth-averaged) Perez model for
                        // walls/roofs with unknown orientation.  N=12 uniformly-
                        // spaced azimuth samples avoid south-facing bias that a
                        // single 180° default would introduce.
                        // Perez et al. (1990) anisotropic diffuse model;
                        // Duffie & Beckman (2020) Eq. 1.6.2 — azimuth appears in
                        // three AOI terms.
                        omni_directional_irradiance(
                            surface.surface_id,
                            ghi,
                            dni,
                            dhi,
                            solar_zenith_deg,
                            pos.azimuth_deg,
                            surface.tilt_deg,
                            day_of_year,
                            ground_albedo,
                            OMNI_AZIMUTH_SAMPLES,
                        )
                    } else {
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
                    }
                }));
        }

        // Apply PV roof shading in-place: reduce irradiance on covered roof surfaces.
        if !self.pv_roof_coverage.is_empty() {
            for irr in &mut self.solar_irradiance_buf {
                if let Some(&coverage) = self.pv_roof_coverage.get(&irr.surface_id) {
                    let exposed = 1.0 - coverage;
                    irr.direct_w_m2 *= exposed;
                    irr.diffuse_w_m2 *= exposed;
                    irr.reflected_w_m2 *= exposed;
                }
            }
        }

        // Swap solar irradiance buffer into state -- the previous Vec's capacity
        // flows back into self.solar_irradiance_buf for use next step.
        std::mem::swap(
            &mut state.weather.solar_irradiance,
            &mut self.solar_irradiance_buf,
        );

        // Step 3: schedule values (reuse self.schedule_values_buf)
        self.schedule_values_buf.clear();
        self.schedule_values_buf
            .extend(self.schedule.columns.iter().map(|col| col[schedule_idx]));

        // Step 4: zones -- written from self.zones (already updated by feed_zones or update).
        state.zones.clear();
        state.zones.extend_from_slice(&self.zones);

        // Step 5: grid defaults / overrides
        state.grid = self.grid_override.clone().unwrap_or(GridState {
            voltage_pu: DEFAULT_GRID_VOLTAGE_PU,
            frequency_hz: DEFAULT_GRID_FREQUENCY_HZ,
        });

        // Step 6: custom_domains -- swap-based reuse to avoid per-step allocations.
        self.mains_payload_buf[0] = mains_temp_c;

        // Recover previously-swapped Vecs from the existing DomainUpdates.
        for du in state.custom_domains.drain(..) {
            if du.domain_id == SCHEDULE_DOMAIN_ID {
                if let Some(v) = du.custom_payload {
                    self.schedule_payload_swap = v;
                }
            } else if du.domain_id == MAINS_WATER_DOMAIN_ID {
                if let Some(v) = du.custom_payload {
                    self.mains_payload_swap = v;
                }
            }
        }

        self.schedule_payload_swap.clear();
        self.schedule_payload_swap
            .extend_from_slice(&self.schedule_values_buf);

        self.mains_payload_swap.clear();
        self.mains_payload_swap
            .extend_from_slice(&self.mains_payload_buf);

        let sched_payload = std::mem::take(&mut self.schedule_payload_swap);
        let mains_payload = std::mem::take(&mut self.mains_payload_swap);

        state.custom_domains.push(hares_types::DomainUpdate {
            domain_id: SCHEDULE_DOMAIN_ID,
            zone_temperatures_c: Vec::new(),
            custom_payload: Some(sched_payload),
        });
        state.custom_domains.push(hares_types::DomainUpdate {
            domain_id: MAINS_WATER_DOMAIN_ID,
            zone_temperatures_c: Vec::new(),
            custom_payload: Some(mains_payload),
        });

        // Step 7: weather scalar fields
        state.weather.outdoor_temp_c = outdoor_temp_c;
        state.weather.outdoor_humidity_ratio = outdoor_humidity_ratio;
        state.weather.outdoor_wet_bulb_c = outdoor_wet_bulb_c;
        state.weather.outdoor_enthalpy_j_kg = outdoor_enthalpy_j_kg;
        state.weather.wind_speed_m_s = self.weather.get(WeatherField::WindSpeedMS, weather_idx);
        state.weather.wind_dir_deg = self.weather.get(WeatherField::WindDirDeg, weather_idx);
        state.weather.ground_temp_c = self.weather.get(WeatherField::GroundTempC, weather_idx);
        state.weather.sky_temp_c = self.weather.get(WeatherField::SkyTempC, weather_idx);
        state.weather.pressure_kpa = pressure_kpa;
        state.weather.ghi_w_m2 = ghi;
        state.weather.dni_w_m2 = dni;
        state.weather.dhi_w_m2 = dhi;
        state.weather.solar_altitude_deg = pos.altitude_deg;
        state.weather.solar_azimuth_deg = pos.azimuth_deg;
        state.weather.mains_temp_c = mains_temp_c;
        state.weather.rainfall_m = self.weather.get(WeatherField::LiquidPrecipM, weather_idx);
        state.weather.ground_albedo = ground_albedo;
        state.weather.ground_t_mean_c = self.ground_t_mean_c;
        state.weather.ground_t_amplitude_c = self.ground_t_amplitude_c;
        state.weather.ground_phase_day = self.ground_phase_day;
        state.weather.day_of_year = clock.current_time().ordinal() as f64;

        state.current_time = clock.current_time();
        state.time_res = clock.time_res;
        state.price_signal = Default::default();
        state.electrical = Default::default();
    }

    /// Update environment state for the current clock step.
    #[must_use]
    pub fn update(&mut self, clock: &SimClock, zone_states: &[ZoneState]) -> EnvironmentState {
        self.feed_zones(zone_states);
        let mut state = EnvironmentState {
            zones: Vec::new(),
            weather: WeatherState::default(),
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
            },
            custom_domains: Vec::new(),
            equipment_telemetry: std::collections::HashMap::new(),
            equipment_core: std::collections::HashMap::new(),
            current_time: clock.current_time(),
            time_res: clock.time_res,
            price_signal: Default::default(),
            electrical: Default::default(),
        };
        self.update_in_place(&mut state, clock);
        state
    }
}

/// EPW hour-ending midpoint shift (30 minutes = 1800 seconds).
/// Compute the step offset into an annual weather file for a given start time.
///
/// EPW files are indexed by **local standard time** (LST). The simulation
/// start time is already in local time (`DateTime<FixedOffset>`), so no
/// UTC→LST conversion is needed -- we extract wall-clock components directly.
///
/// The `midpoint_offset_secs` from `WeatherMeta` accounts for the source
/// file's timestamp convention. For EPW (hour-ending), subtracting half a
/// period aligns the PCHIP knot with the period midpoint, matching OCHRE's
/// pvlib +30min convention.
fn compute_annual_offset(
    meta: &WeatherMeta,
    start_time: DateTime<FixedOffset>,
    step_secs: u32,
    weather_len: usize,
) -> usize {
    if step_secs == 0 {
        return 0;
    }

    let doy0 = start_time.ordinal0() as u64;
    let h = start_time.hour() as u64;
    let m = start_time.minute() as u64;
    let s = start_time.second() as u64;
    let seconds_into_year = doy0 * 86400 + h * 3600 + m * 60 + s;

    // Year length in seconds derived from the actual weather array size.
    // After resampling, weather_len rows at step_secs each covers one year
    // (365 or 366 days).  EPW, PSM3, and TMY3 all support leap-year files;
    // using the actual row count avoids mis-indexing for 8784-row (leap) files.
    let year_secs = weather_len as u64 * step_secs as u64;
    let shifted = (seconds_into_year + year_secs - meta.midpoint_offset_secs as u64) % year_secs;

    (shifted / step_secs as u64) as usize
}

/// Compute offset into the schedule time series.
/// Schedules from ResStock/BEopt are annual, indexed from Jan 1 in local time.
/// ResStock/BEopt schedules are always 365-day non-leap-year data; the
/// `ordinal0()`-based computation is adjusted for leap-year start times so that
/// calendar dates after Feb 29 map to the same schedule rows as in a non-leap
/// year, preventing the one-day schedule misalignment described in T-0032.
#[cfg(test)]
fn compute_schedule_offset(
    schedule: &ScheduleTimeSeries,
    start_time: DateTime<FixedOffset>,
    step_secs: u32,
) -> usize {
    if step_secs == 0 || schedule.is_empty() {
        return 0;
    }
    let mut doy0 = start_time.ordinal0() as u64;
    // Schedules are always 365-day annual data. In a leap year, ordinal0()
    // is +1 for dates after Feb 29 relative to their non-leap counterpart.
    // Subtract 1 to restore the correct schedule row index.
    let year = start_time.year();
    if is_leap_year(year) && doy0 > 59 {
        doy0 -= 1;
    }
    let h = start_time.hour() as u64;
    let m = start_time.minute() as u64;
    let s = start_time.second() as u64;
    let seconds_into_year = doy0 * 86400 + h * 3600 + m * 60 + s;
    // Derive year length from the actual schedule array size so wrapping is
    // correct regardless of whether the schedule is hourly, sub-hourly, etc.
    let year_secs = schedule.len() as u64 * step_secs as u64;
    #[cfg(any(debug_assertions, feature = "check_invariants"))]
    debug_assert!(year_secs > 0, "schedule has zero length or step");
    (seconds_into_year % year_secs / step_secs as u64) as usize
}

/// Return true if `year` is a leap year (29-day February per Gregorian calendar).
fn is_leap_year(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn compute_mains_inputs(weather: &WeatherTimeSeries, step_secs: u32) -> (f64, f64) {
    let annual_avg_c = mean_or_default(&weather.dry_bulb_c, 20.0);

    let temps = &weather.dry_bulb_c;

    // For annual weather (EPW-derived), compute the range of monthly average dry-bulb
    // temperatures, as required by the Burch-Christensen mains model.
    // EPW, PSM3, and TMY3 all support leap-year files (8784 hourly rows); use
    // 29 days for February when the data length implies a leap year.
    let samples_per_day = usize::try_from(86_400 / step_secs.max(1)).unwrap_or(0);
    if samples_per_day == 0 {
        return (annual_avg_c, 0.0);
    }
    let is_leap = temps.len() == 366 * samples_per_day;
    let month_days = monthly_day_counts(is_leap);

    // Accumulate month means for months fully covered by the available data.
    // For a full year this processes all 12 months. For short or synthetic
    // weather this processes as many complete months as the data allows.
    let mut month_means = Vec::with_capacity(12);
    let mut cursor = 0usize;
    for days in month_days {
        let month_samples = days * samples_per_day;
        let end = cursor + month_samples;
        if end > temps.len() {
            break;
        }
        let slice = &temps[cursor..end];
        month_means.push(mean_or_default(slice, annual_avg_c));
        cursor = end;
    }

    if month_means.len() >= 2 {
        let monthly_range_c = simple_range(&month_means);
        return (annual_avg_c, monthly_range_c.max(1.0));
    }

    // Fewer than 2 full months: conservative fallback.
    // Per Kusuda & Achenbach (1965) ASHRAE Trans. 71(1):61-74, T_amplitude
    // is half the range of monthly mean temperatures. When fewer than 2
    // monthly means can be computed from the available data, use 0.0
    // amplitude (constant ground temperature equal to annual mean), which is
    // physically conservative and matches the synthetic weather case.
    (annual_avg_c, 0.0)
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

fn build_surface_geometry(
    building: &Building,
) -> Result<Vec<SurfaceGeometry>, EnvironmentManagerError> {
    building
        .boundaries
        .iter()
        .enumerate()
        .map(|(idx, boundary)| {
            let tilt_deg = boundary.tilt_deg.unwrap_or(90.0);
            let (azimuth_deg, omni_directional) = match boundary.azimuth_deg {
                Some(az) => (az, false),
                None => {
                    let is_exterior = matches!(boundary.exterior_zone, Some(ZoneType::Outdoor));
                    match (&boundary.boundary_type, is_exterior) {
                        (BoundaryType::Window, _) => {
                            return Err(EnvironmentManagerError::MissingAzimuth {
                                boundary_idx: idx,
                                surface_type: "Window".to_string(),
                            });
                        }
                        (BoundaryType::Wall, true) => {
                            tracing::warn!(
                                boundary_idx = idx,
                                surface_type = "Wall",
                                "wall surface has no azimuth; using omnidirectional \
                                 (azimuth-averaged) Perez model to avoid south-facing bias \
                                 — HPXML does not require azimuth on walls; this is a valid \
                                 input.  Non-solar consumers (LWR view factors, film \
                                 coefficients) default to 180° as documented in Known \
                                 Limitations"
                            );
                            (180.0, true)
                        }
                        (BoundaryType::Roof, true) => {
                            tracing::warn!(
                                boundary_idx = idx,
                                surface_type = "Roof",
                                "roof surface has no azimuth; using omnidirectional \
                                 (azimuth-averaged) Perez model — hip and flat roofs \
                                 receive solar from all azimuth angles.  Non-solar \
                                 consumers (LWR view factors, film coefficients) default \
                                 to 180° as documented in Known Limitations"
                            );
                            (180.0, true)
                        }
                        _ => {
                            // Interior surfaces (inside walls, furniture) or
                            // non-sky-facing boundaries (foundation, slab, floor,
                            // rim joist): azimuth is irrelevant for solar.
                            (180.0, false)
                        }
                    }
                }
            };
            Ok(SurfaceGeometry {
                surface_id: u32::try_from(idx).unwrap_or(u32::MAX),
                azimuth_deg,
                tilt_deg,
                area_m2: boundary.area_m2,
                omni_directional,
            })
        })
        .collect()
}

/// Outdoor temperature threshold for selecting heating vs cooling setpoint
/// at initialization, matching OCHRE's Envelope.initialize_state().
const OUTDOOR_HEATING_COOLING_THRESHOLD_C: f64 = 12.0;
const DEFAULT_SETPOINT_C: f64 = 21.0;

fn initial_zones(
    building: &Building,
    outdoor_temp_c: f64,
    ground_temp_c: f64,
    start_time: DateTime<FixedOffset>,
    initial_rng: Option<ChaCha8Rng>,
    setpoint_deadband_c: Option<f64>,
) -> Result<Vec<ZoneState>, EnvironmentManagerError> {
    let default_temp = determine_initial_indoor_temp_c(
        building,
        outdoor_temp_c,
        start_time.hour() as usize,
        matches!(
            start_time.weekday(),
            chrono::Weekday::Sat | chrono::Weekday::Sun
        ),
        initial_rng,
        setpoint_deadband_c,
    );
    if building.zones.is_empty() {
        return Ok(vec![ZoneState {
            id: ZoneId(1),
            temperature_c: default_temp,
            humidity_ratio: 0.008,
            relative_humidity: 0.45,
            wet_bulb_c: default_temp,
            volume_m3: DEFAULT_ZONE_VOLUME_M3,
        }]);
    }

    building
        .zones
        .iter()
        .enumerate()
        .map(
            |(idx, zone)| -> Result<ZoneState, EnvironmentManagerError> {
                use hares_io::hpxml::ZoneType;
                // Conditioned zones start near the active HVAC setpoint.
                // Foundation zones start at ground temperature.
                // Other unconditioned zones (attic, garage) start at outdoor temp.
                let temp = match zone.zone_type {
                    ZoneType::Conditioned => default_temp,
                    ZoneType::Foundation => ground_temp_c,
                    _ => outdoor_temp_c,
                };
                let volume_m3 = match zone.volume_m3 {
                    Some(v) => v,
                    None => {
                        let estimated = zone
                            .floor_area_m2
                            .map(|area| match zone.zone_type {
                                ZoneType::Attic => 0.5 * area * 1.5,
                                ZoneType::Garage | ZoneType::Foundation | ZoneType::Conditioned => {
                                    area * 2.44
                                }
                                _ => DEFAULT_ZONE_VOLUME_M3,
                            })
                            .unwrap_or(DEFAULT_ZONE_VOLUME_M3);
                        tracing::warn!(
                            zone_idx = idx,
                            zone_type = ?zone.zone_type,
                            estimated_m3 = estimated,
                            "zone is missing volume_m3; using geometry-based estimate"
                        );
                        estimated
                    }
                };
                Ok(ZoneState {
                    id: ZoneId(u16::try_from(idx + 1).unwrap_or(u16::MAX)),
                    temperature_c: temp,
                    humidity_ratio: 0.008,
                    relative_humidity: 0.45,
                    wet_bulb_c: temp,
                    volume_m3,
                })
            },
        )
        .collect()
}

/// Determines initial indoor temperature from HVAC setpoints and outdoor temp.
///
/// Matches OCHRE's `Envelope.initialize_state()`:
/// - outdoor > 12°C → cooling setpoint (building is in cooling mode)
/// - outdoor ≤ 12°C → heating setpoint (building is in heating mode)
/// - conditioned-zone temperature is randomized within half the deadband
///   around the selected setpoint
/// - No setpoints available → outdoor temperature (free-float zone)
///
/// `start_hour` must be in `[0, 23]` (e.g. from `chrono::DateTime::hour()`).
fn determine_initial_indoor_temp_c(
    building: &Building,
    outdoor_temp_c: f64,
    start_hour: usize,
    is_weekend: bool,
    mut initial_rng: Option<ChaCha8Rng>,
    setpoint_deadband_c: Option<f64>,
) -> f64 {
    debug_assert!(start_hour <= 23, "start_hour out of range: {start_hour}");
    let (heating_sp, cooling_sp) = if is_weekend {
        (
            building
                .heating_weekend_setpoints_c
                .as_ref()
                .and_then(|v| v.get(start_hour).copied())
                .or_else(|| {
                    building
                        .heating_weekday_setpoints_c
                        .as_ref()
                        .and_then(|v| v.get(start_hour).copied())
                }),
            building
                .cooling_weekend_setpoints_c
                .as_ref()
                .and_then(|v| v.get(start_hour).copied())
                .or_else(|| {
                    building
                        .cooling_weekday_setpoints_c
                        .as_ref()
                        .and_then(|v| v.get(start_hour).copied())
                }),
        )
    } else {
        (
            building
                .heating_weekday_setpoints_c
                .as_ref()
                .and_then(|v| v.get(start_hour).copied())
                .or_else(|| {
                    building
                        .heating_weekend_setpoints_c
                        .as_ref()
                        .and_then(|v| v.get(start_hour).copied())
                }),
            building
                .cooling_weekday_setpoints_c
                .as_ref()
                .and_then(|v| v.get(start_hour).copied())
                .or_else(|| {
                    building
                        .cooling_weekend_setpoints_c
                        .as_ref()
                        .and_then(|v| v.get(start_hour).copied())
                }),
        )
    };

    let deadband_c = setpoint_deadband_c.unwrap_or(1.0);

    let select_with_noise = |setpoint_c: f64, rng: &mut ChaCha8Rng| {
        let random_delta = (rng.random::<f64>() - 0.5) * deadband_c;
        setpoint_c + random_delta
    };

    match (heating_sp, cooling_sp) {
        (Some(h), Some(c)) => {
            if outdoor_temp_c > OUTDOOR_HEATING_COOLING_THRESHOLD_C {
                if let Some(rng) = initial_rng.as_mut() {
                    select_with_noise(c, rng)
                } else {
                    c
                }
            } else {
                if let Some(rng) = initial_rng.as_mut() {
                    select_with_noise(h, rng)
                } else {
                    h
                }
            }
        }
        (Some(h), None) => {
            if let Some(rng) = initial_rng.as_mut() {
                select_with_noise(h, rng)
            } else {
                h
            }
        }
        (None, Some(c)) => {
            if let Some(rng) = initial_rng.as_mut() {
                select_with_noise(c, rng)
            } else {
                c
            }
        }
        (None, None) => {
            // Free-float zone (no HVAC setpoints): start at outdoor temperature
            // rather than DEFAULT_SETPOINT_C. For heavyweight buildings the floor
            // concrete time constant can exceed 30 days; starting at 21 °C creates
            // a multi-week transient that biases annual results. EnergyPlus
            // Eng.Ref §1.2 requires zone temperatures to converge to periodic
            // steady state before results are collected.
            outdoor_temp_c
        }
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
                wf_allows_leap_years: true,
                source_step_secs: 3600,
                midpoint_offset_secs: 0,
            },
            design_conditions: None,
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

    /// Construct a schedule with `n` rows for offset computation tests.
    fn hourly_schedule(n: usize) -> ScheduleTimeSeries {
        let start = utc_offset().with_ymd_and_hms(2007, 1, 1, 0, 0, 0).unwrap();
        let timestamps: Vec<_> = (0..n).map(|i| start + Duration::hours(i as i64)).collect();
        let mut index = HashMap::new();
        index.insert("test".to_string(), 0);
        ScheduleTimeSeries {
            timestamps,
            column_names: vec!["test".to_string()],
            columns: vec![vec![0.0; n]],
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
                    framing_factor: None,
                    lut_boundary_name: None,
                    floor_or_ceiling: None,
                    perimeter_m: None,
                    perimeter_insulation_r_m2_k_w: None,
                    foundation_depth_m: None,
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
                    perimeter_m: None,
                    perimeter_insulation_r_m2_k_w: None,
                    foundation_depth_m: None,
                },
            ],
            windows: Vec::<Window>::new(),
            infiltration_ach50: None,
            infiltration_cfm50: None,
            infiltration_ach_natural: None,
            infiltration_cfm_natural: None,
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
            mass_multiplier_override: None,
            hvac_deadband_c: None,
            details_xml,
        }
    }

    fn building_with_missing_attic_volume() -> Building {
        let mut b = building(Some(21.0));
        b.zones.push(Zone {
            zone_type: ZoneType::Attic,
            floor_area_m2: Some(100.0),
            volume_m3: None,
            attached_wall_ids: vec![],
            duct_systems: vec![],
            vented: true,
            ventilation_ach: None,
            ventilation_sla: None,
        });
        b
    }

    fn building_with_conditioned_foundation_and_garage() -> Building {
        let mut b = building(Some(21.0));
        b.zones = vec![
            Zone {
                zone_type: ZoneType::Conditioned,
                floor_area_m2: Some(100.0),
                volume_m3: Some(250.0),
                attached_wall_ids: vec![],
                duct_systems: vec![],
                vented: false,
                ventilation_ach: None,
                ventilation_sla: None,
            },
            Zone {
                zone_type: ZoneType::Foundation,
                floor_area_m2: Some(50.0),
                volume_m3: Some(120.0),
                attached_wall_ids: vec![],
                duct_systems: vec![],
                vented: false,
                ventilation_ach: None,
                ventilation_sla: None,
            },
            Zone {
                zone_type: ZoneType::Garage,
                floor_area_m2: Some(40.0),
                volume_m3: Some(90.0),
                attached_wall_ids: vec![],
                duct_systems: vec![],
                vented: false,
                ventilation_ach: None,
                ventilation_sla: None,
            },
        ];
        b
    }

    fn clock() -> SimClock {
        let start = DateTime::parse_from_rfc3339("2024-06-21T12:00:00+00:00").expect("parse");
        SimClock::new(start, Duration::seconds(60), Duration::hours(2))
    }

    #[test]
    fn weather_step_0_matches_first_epw_record() {
        // Start at 00:00 LST: no midpoint shift, maps directly to row 0.
        let start = utc_offset().with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap();
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
    fn manager_defaults_attic_volume_when_missing() {
        let result = EnvironmentManager::new(
            weather_series(),
            schedule_series(),
            &building_with_missing_attic_volume(),
            StdDuration::from_secs(60),
            utc_offset().with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap(),
            None,
        );
        assert!(
            result.is_ok(),
            "attic missing volume should fall back to DEFAULT_ZONE_VOLUME_M3, got: {result:?}"
        );
    }

    #[test]
    fn free_float_zone_starts_at_outdoor_temp() {
        let b = building(None);
        let result = determine_initial_indoor_temp_c(&b, 10.0, 0, false, None, None);
        assert!((result - 10.0).abs() < 1e-10, "expected 10.0, got {result}");
    }

    #[test]
    fn free_float_zone_starts_at_outdoor_temp_cold() {
        let b = building(None);
        let result = determine_initial_indoor_temp_c(&b, -10.0, 0, false, None, None);
        assert!(
            (result - (-10.0)).abs() < 1e-10,
            "expected -10.0, got {result}"
        );
    }

    #[test]
    fn conditioned_zone_with_setpoints_unchanged() {
        let mut b = building(None);
        b.heating_weekday_setpoints_c = Some(vec![20.0; 24]);
        b.heating_weekend_setpoints_c = Some(vec![20.0; 24]);
        let result = determine_initial_indoor_temp_c(&b, 0.0, 0, false, None, None);
        assert!((result - 20.0).abs() < 1e-10, "expected 20.0, got {result}");
    }

    #[test]
    fn initial_zone_temperatures_use_ground_for_foundation_and_outdoor_for_other_unconditioned() {
        let start = utc_offset().with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap();
        let mut manager = EnvironmentManager::new(
            weather_series(),
            schedule_series(),
            &building_with_conditioned_foundation_and_garage(),
            StdDuration::from_secs(60),
            start,
            None,
        )
        .expect("manager");
        let sim_clock = SimClock::new(start, Duration::seconds(60), Duration::hours(1));
        let env = manager.update(&sim_clock, &[]);

        assert_eq!(env.zones.len(), 3);
        assert!(
            (env.zones[1].temperature_c - env.weather.ground_temp_c).abs() < 1.0e-9,
            "foundation zone must initialize from ground temperature"
        );
        assert!(
            (env.zones[2].temperature_c - env.weather.outdoor_temp_c).abs() < 1.0e-9,
            "garage (unconditioned non-foundation) must initialize from outdoor temperature"
        );
    }

    #[test]
    fn weather_step_59_uses_pchip_interpolation() {
        // With 2-element [10.0, 20.0] input and PCHIP (linear for 2-pt),
        // step 59 of 60 sub-hour slots ≈ 10.0 + (59/60)*10.0 ≈ 19.833.
        let start = utc_offset().with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap();
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
        assert!((env.weather.wind_dir_deg - 185.0).abs() < 1.0e-6);
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
    fn missing_setpoints_starts_at_outdoor_temp() {
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
        // Free-float zone (no HVAC setpoints) starts at outdoor temperature,
        // not 21 °C. weather_series() has dry_bulb_c[0] = 10.0.
        assert!((env.zones[0].temperature_c - 10.0).abs() < 1e-6);
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
        // High DNI (800), moderate DHI (100) -- Perez conditions satisfied (dhi >= 1).
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
        // The test clock starts 2024-06-21T12:00:00Z -- solar altitude > 60°.
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

    /// compute_annual_offset: leap year (2024) -- May 5 noon.
    /// Feb has 29 days so May 5 = ordinal 126, ordinal0 = 125.
    /// (125*86400 + 12*3600) / 3600 = 3012.
    #[test]
    fn annual_offset_leap_year_may_5_noon() {
        let meta = weather_series().meta;
        let start = utc_offset().with_ymd_and_hms(2024, 5, 5, 12, 0, 0).unwrap();
        assert_eq!(compute_annual_offset(&meta, start, 3600, 8784), 3012);
    }

    /// compute_annual_offset: non-leap year (2023) -- May 5 noon.
    /// (124*86400 + 12*3600) / 3600 = 2988.
    #[test]
    fn annual_offset_non_leap_year_may_5_noon() {
        let meta = weather_series().meta;
        let start = utc_offset().with_ymd_and_hms(2023, 5, 5, 12, 0, 0).unwrap();
        assert_eq!(compute_annual_offset(&meta, start, 3600, 8760), 2988);
    }

    /// compute_annual_offset: leap year Jan 1 00:00 → index 0.
    #[test]
    fn annual_offset_leap_year_jan_1() {
        let meta = weather_series().meta;
        let start = utc_offset().with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        assert_eq!(compute_annual_offset(&meta, start, 3600, 8784), 0);
    }

    /// compute_annual_offset: non-leap year Jan 1 00:00 → index 0.
    #[test]
    fn annual_offset_non_leap_year_jan_1() {
        let meta = weather_series().meta;
        let start = utc_offset().with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap();
        assert_eq!(compute_annual_offset(&meta, start, 3600, 8760), 0);
    }

    /// compute_annual_offset: leap year Dec 31 23:00.
    /// With 8784 hourly rows (366 days), Dec 31 ordinal0=365, seconds_into_year
    /// = 365*86400 + 23*3600 = 31,618,800.  year_secs = 8784*3600 = 31,622,400.
    /// shifted = 31,618,800 % 31,622,400 = 31,618,800.  31,618,800 / 3600 = 8783.
    #[test]
    fn annual_offset_leap_year_dec_31() {
        let meta = weather_series().meta;
        let start = utc_offset()
            .with_ymd_and_hms(2024, 12, 31, 23, 0, 0)
            .unwrap();
        assert_eq!(compute_annual_offset(&meta, start, 3600, 8784), 8783);
    }

    /// compute_annual_offset: non-leap year Dec 31 23:00.
    /// (364*86400 + 23*3600) / 3600 = 8759.
    #[test]
    fn annual_offset_non_leap_year_dec_31() {
        let meta = weather_series().meta;
        let start = utc_offset()
            .with_ymd_and_hms(2023, 12, 31, 23, 0, 0)
            .unwrap();
        assert_eq!(compute_annual_offset(&meta, start, 3600, 8760), 8759);
    }

    /// compute_annual_offset: leap year Feb 29 → ordinal0 = 59.
    /// (59*86400 + 6*3600) / 3600 = 1422.
    #[test]
    fn annual_offset_leap_year_feb_29() {
        let meta = weather_series().meta;
        let start = utc_offset().with_ymd_and_hms(2024, 2, 29, 6, 0, 0).unwrap();
        assert_eq!(compute_annual_offset(&meta, start, 3600, 8784), 1422);
    }

    /// compute_annual_offset: non-leap year Mar 1 → ordinal0 = 59.
    /// (59*86400 + 6*3600) / 3600 = 1422.
    #[test]
    fn annual_offset_non_leap_year_mar_1() {
        let meta = weather_series().meta;
        let start = utc_offset().with_ymd_and_hms(2023, 3, 1, 6, 0, 0).unwrap();
        assert_eq!(compute_annual_offset(&meta, start, 3600, 8760), 1422);
    }

    /// compute_annual_offset: sub-hourly resolution (15-min steps).
    /// start = 2023-01-01T01:30:00: seconds=5400.  35040 rows at 15-min.
    /// year_secs = 35040*900 = 31,536,000 (= 365*86400).  5400/900 = 6.
    #[test]
    fn annual_offset_15min_resolution() {
        let meta = weather_series().meta;
        let start = utc_offset().with_ymd_and_hms(2023, 1, 1, 1, 30, 0).unwrap();
        assert_eq!(compute_annual_offset(&meta, start, 900, 35040), 6);
    }

    // ----- compute_schedule_offset leap-year tests (T-0032) -----

    /// Schedule offset for Feb 29 in a leap year.
    /// Feb 29 ordinal0=59, same as non-leap Mar 1. The schedule has 8760 rows
    /// (365-day non-leap) so Feb 29 maps to row 59*24=1416 (March 1 data).
    /// No adjustment needed since ordinal0 <= 59, but year_secs wrapping
    /// protects against out-of-bounds.
    #[test]
    fn compute_schedule_offset_leap_year_feb_29() {
        let schedule = hourly_schedule(8760);
        let start = utc_offset().with_ymd_and_hms(2024, 2, 29, 0, 0, 0).unwrap();
        let offset = compute_schedule_offset(&schedule, start, 3600);
        assert_eq!(offset, 1416, "Feb 29 should index row 1416 (day 59)");
    }

    /// Schedule offset for March 1 in a leap year.
    /// ordinal0=60, adjusted -1 → 59 so the schedule row matches the non-leap
    /// index for March 1 (59*24=1416).
    #[test]
    fn compute_schedule_offset_leap_year_mar_1() {
        let schedule = hourly_schedule(8760);
        let start = utc_offset().with_ymd_and_hms(2024, 3, 1, 0, 0, 0).unwrap();
        let offset = compute_schedule_offset(&schedule, start, 3600);
        assert_eq!(offset, 1416, "March 1 leap year should index day 59 (1416)");
    }

    /// Regression: same calendar date across leap and non-leap years produces
    /// identical schedule offsets. The one-day shift after Feb 29 is the
    /// defect T-0032 fixes.
    #[test]
    fn compute_schedule_offset_same_date_leap_vs_non_leap() {
        let schedule = hourly_schedule(8760);
        let start_leap = utc_offset()
            .with_ymd_and_hms(2024, 7, 4, 12, 30, 0)
            .unwrap();
        let start_non_leap = utc_offset()
            .with_ymd_and_hms(2023, 7, 4, 12, 30, 0)
            .unwrap();
        let leap_offset = compute_schedule_offset(&schedule, start_leap, 3600);
        let non_leap_offset = compute_schedule_offset(&schedule, start_non_leap, 3600);
        assert_eq!(
            leap_offset, non_leap_offset,
            "same calendar date (Jul 4) must produce identical schedule offset"
        );
    }

    /// Leap year Dec 31 wraps correctly. ordinal0=365, adjusted -1 → 364.
    /// Row = 364*24 = 8736.
    #[test]
    fn compute_schedule_offset_leap_year_dec_31() {
        let schedule = hourly_schedule(8760);
        let start = utc_offset()
            .with_ymd_and_hms(2024, 12, 31, 0, 0, 0)
            .unwrap();
        let offset = compute_schedule_offset(&schedule, start, 3600);
        assert_eq!(offset, 8736, "Dec 31 leap year → adjusted ordinal 364");
    }

    /// Sub-hourly schedule (15-min steps, 35040 rows) in a leap year.
    /// Mar 1 ordinal0=60, adjusted -1 → 59. seconds = 59*86400=5097600.
    /// index = 5097600/900 = 5664.
    #[test]
    fn compute_schedule_offset_leap_year_15min() {
        let schedule = hourly_schedule(35040);
        let start = utc_offset().with_ymd_and_hms(2024, 3, 1, 0, 0, 0).unwrap();
        let offset = compute_schedule_offset(&schedule, start, 900);
        assert_eq!(offset, 5664, "15-min March 1 leap year");
    }

    /// Regression: non-DST running path produces correct schedule index after Feb 29.
    ///
    /// The original non-DST fallback used `(step + schedule_start_offset) % len`,
    /// which accumulates the step counter linearly. After Feb 29 in a leap year,
    /// the step counter is +1 relative to the adjusted doy0, permanently shifting
    /// the schedule by one day for the remainder of the year. T-0032.
    #[test]
    fn non_dst_running_path_handles_leap_year_feb_29() {
        let n = 8760;
        // Schedule where each row has value == row_index for easy verification.
        let base = utc_offset().with_ymd_and_hms(2007, 1, 1, 0, 0, 0).unwrap();
        let timestamps: Vec<_> = (0..n).map(|i| base + Duration::hours(i as i64)).collect();
        let row_values: Vec<f64> = (0..n).map(|i| i as f64).collect();
        let mut col_idx = HashMap::new();
        col_idx.insert("hour_idx".to_string(), 0);
        let schedule = ScheduleTimeSeries {
            timestamps,
            column_names: vec!["hour_idx".to_string()],
            columns: vec![row_values],
            column_index: col_idx,
            source_step_secs: 3600,
            column_aggregations: vec![],
        };
        let weather = WeatherTimeSeries {
            meta: WeatherMeta {
                location: "Test".to_string(),
                latitude: 40.0,
                longitude: -74.0,
                timezone_offset_h: -5.0,
                elevation_m: 10.0,
                wf_allows_leap_years: true,
                source_step_secs: 3600,
                midpoint_offset_secs: 0,
            },
            design_conditions: None,
            dry_bulb_c: vec![20.0; n],
            dew_point_c: vec![10.0; n],
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
        };

        let est = FixedOffset::west_opt(5 * 3600).expect("offset");
        let start = est.with_ymd_and_hms(2024, 2, 28, 0, 0, 0).unwrap();
        let mut mgr = EnvironmentManager::new(
            weather,
            schedule,
            &building(Some(21.0)),
            StdDuration::from_secs(3600),
            start,
            None, // no DST
        )
        .expect("manager");

        // Simulate 72 hours: Feb 28 00:00 through March 1 23:00.
        let mut clock = SimClock::new(start, Duration::hours(1), Duration::hours(72));

        // Feb 28 00:00 (step 0): doy0=58, no adjustment → 58*24+0 = 1392
        // Feb 29 00:00 (step 24): doy0=59, not >59, no adjustment → 59*24+0 = 1416
        // Mar  1 00:00 (step 48): doy0=60, adjusted -1 → 59*24+0 = 1416
        for step in 0..72u64 {
            let env = mgr.update(&clock, &[]);
            let got = env
                .custom_domains
                .iter()
                .find(|d| d.domain_id == SCHEDULE_DOMAIN_ID)
                .and_then(|d| d.custom_payload.as_ref())
                .and_then(|p| p.first())
                .copied()
                .expect("schedule domain payload");

            let sim_time = clock.current_time();
            let mut doy0 = sim_time.ordinal0() as u64;
            if is_leap_year(sim_time.year()) && doy0 > 59 {
                doy0 -= 1;
            }
            let expected = (doy0 * 24 + sim_time.hour() as u64) as f64;

            assert!(
                (got - expected).abs() < 1e-6,
                "step {step}: civil {sim_time}, expected row {expected}, got {got}"
            );

            // Specific assertions at the key transition points.
            if step == 24 {
                assert!(
                    (got - 1416.0).abs() < 1e-6,
                    "Feb 29 00:00 should index schedule row 1416 (day 59), got {got}"
                );
            }
            if step == 48 {
                assert!(
                    (got - 1416.0).abs() < 1e-6,
                    "March 1 00:00 leap year should index schedule row 1416 \
                     (adjusted doy0 59), got {got} — step-counter bug would produce 1440"
                );
            }

            if clock.next().is_none() {
                break;
            }
        }
    }

    // ---------------------------------------------------------------

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
    /// model and differ from the static 10°C fallback for a real climate.
    #[test]
    fn weather_state_mains_temp_c_is_populated_and_not_constant() {
        // 8760-row annual weather with enough seasonal variation to produce
        // a mains temperature that differs from the 10°C fallback.
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
        // winter (January) will be well below the 10°C fallback default.
        assert!(
            env.weather.mains_temp_c < 10.0,
            "January mains_temp_c ({}) should be below 10°C fallback for a cold-start climate",
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

    #[test]
    fn register_surface_deduplicates() {
        let start = ts(0);
        let mut env = EnvironmentManager::new(
            weather_series(),
            schedule_series(),
            &building(Some(21.0)),
            StdDuration::from_secs(3600),
            start,
            None,
        )
        .unwrap();
        let initial_count = env.surface_count();
        env.register_surface(SurfaceGeometry {
            surface_id: 999_999,
            azimuth_deg: 180.0,
            tilt_deg: 30.0,
            area_m2: 1.0,
            omni_directional: false,
        });
        assert_eq!(env.surface_count(), initial_count + 1);
        // Duplicate should be ignored.
        env.register_surface(SurfaceGeometry {
            surface_id: 999_999,
            azimuth_deg: 180.0,
            tilt_deg: 30.0,
            area_m2: 1.0,
            omni_directional: false,
        });
        assert_eq!(env.surface_count(), initial_count + 1);
    }

    #[test]
    fn pv_roof_shading_reduces_irradiance() {
        use chrono::Duration as ChronoDuration;
        let start = ts(0);
        let mut env = EnvironmentManager::new(
            weather_series(),
            schedule_series(),
            &building(Some(21.0)),
            StdDuration::from_secs(3600),
            start,
            None,
        )
        .unwrap();

        let clock = SimClock::new(
            start,
            ChronoDuration::seconds(3600),
            ChronoDuration::hours(1),
        );

        // Get baseline irradiance on first surface.
        let baseline = env.update(&clock, &[]);
        let surface_0_id = baseline.weather.solar_irradiance[0].surface_id;
        let baseline_direct = baseline.weather.solar_irradiance[0].direct_w_m2;

        // Set 50% coverage on that surface.
        env.set_pv_roof_coverage(surface_0_id, 0.5);
        let shaded = env.update(&clock, &[]);
        let shaded_direct = shaded.weather.solar_irradiance[0].direct_w_m2;

        // Irradiance should be halved.
        let ratio = if baseline_direct > 0.0 {
            shaded_direct / baseline_direct
        } else {
            0.5 // If baseline is zero, shaded should also be zero.
        };
        assert!(
            (ratio - 0.5).abs() < 0.01,
            "expected ~50% reduction, got ratio={ratio:.4}"
        );
    }

    // ---- Regression tests for Kusuda amplitude derivation ----

    /// Regression for Defect 1: leap-year weather (8784 hourly records)
    /// must use 29-day February when computing monthly means for the Kusuda amplitude.
    ///
    /// The bug: `month_days` was hardcoded to `[31, 28, ...]` so February consumed only
    /// 672 samples instead of 696.  With the fix, February has 29 days for leap-year data.
    ///
    /// Fixture: 8784-record series where March has a constant +100°C and all other
    /// months are 0°C.  With correct 29-day February, March mean = 100.0°C exactly,
    /// range = 100.0°C.  With the bug, the 29th February day leaks into March, lowering
    /// the observed March mean below 100°C and shrinking the range.
    #[test]
    fn leap_year_february_uses_29_days_for_monthly_mean() {
        // 8784 hours in a leap year (366 days × 24).
        let n = 8784usize;
        let step_secs = 3600u32; // hourly

        let jan_hours = 31 * 24; // 744
        let feb_hours = 29 * 24; // 696

        // Build dry_bulb_c: March = 100°C, all other months = 0°C.
        // With correct 29-day February, March's slice starts at the right offset
        // and its mean is exactly 100°C, giving range = 100°C.
        // With buggy 28-day February, the 29th Feb day (0°C) leaks into March,
        // lowering the March mean below 100°C and shrinking the range.
        let mut dry_bulb = vec![0.0f64; n];
        let mar_start = jan_hours + feb_hours;
        let mar_end = mar_start + 31 * 24;
        for v in &mut dry_bulb[mar_start..mar_end] {
            *v = 100.0;
        }

        let weather = WeatherTimeSeries {
            meta: WeatherMeta {
                location: "Leap-034".to_string(),
                latitude: 40.0,
                longitude: 0.0,
                timezone_offset_h: 0.0,
                elevation_m: 0.0,
                wf_allows_leap_years: true,
                source_step_secs: step_secs,
                midpoint_offset_secs: 0,
            },
            design_conditions: None,
            dry_bulb_c: dry_bulb,
            dew_point_c: vec![0.0; n],
            rel_humidity_pct: vec![50.0; n],
            pressure_kpa: vec![101.325; n],
            ghi_w_m2: vec![0.0; n],
            dni_w_m2: vec![0.0; n],
            dhi_w_m2: vec![0.0; n],
            wind_speed_m_s: vec![1.0; n],
            wind_dir_deg: vec![180.0; n],
            opaque_sky_cover: vec![0.0; n],
            horizontal_infrared_w_m2: vec![250.0; n],
            sky_temp_c: vec![0.0; n],
            ground_temp_c: vec![10.0; n],
            liquid_precip_m: vec![0.0; n],
            surface_albedo: None,
        };

        let (_avg, range) = compute_mains_inputs(&weather, step_secs);

        // With correct 29-day February, March mean = 100.0°C exactly, range = 100.0.
        // With buggy 28-day February, March slice starts 24 hours early, absorbing one
        // day of zero temperature from the gap region, lowering the mean to ≈96.77°C
        // and shrinking the range.
        assert!(
            (range - 100.0).abs() < 0.01,
            "expected monthly range = 100.0°C when March is \
             100°C and all other months 0°C in a leap-year (8784-hour) series, \
             but got {range:.4} — indicates February is not using 29-day slice"
        );
    }

    /// Regression for Defect 2: when weather has fewer than 8760 hourly
    /// records, the fallback must derive the amplitude from whatever monthly means are
    /// available, NOT from the instantaneous min/max of the raw hourly series.
    ///
    /// Fixture: 48-hour series (2 days) where values alternate between -20°C and
    /// +20°C, giving an instantaneous range of 40°C.  No full calendar month can be
    /// assembled (31 days minimum), so the correct behaviour is to return amplitude = 0.0
    /// (constant ground temperature equal to annual mean, the physically conservative
    /// choice per Kusuda & Achenbach 1965).
    #[test]
    fn short_weather_fallback_uses_monthly_means_not_instantaneous_extremes() {
        let step_secs = 3600u32;
        let n = 48usize; // 2 days — far less than one full calendar month

        // Alternating -20°C / +20°C: instantaneous range = 40°C.
        let dry_bulb_c: Vec<f64> = (0..n)
            .map(|i| if i % 2 == 0 { -20.0 } else { 20.0 })
            .collect();

        let weather = WeatherTimeSeries {
            meta: WeatherMeta {
                location: "Short-034".to_string(),
                latitude: 40.0,
                longitude: 0.0,
                timezone_offset_h: 0.0,
                elevation_m: 0.0,
                wf_allows_leap_years: true,
                source_step_secs: step_secs,
                midpoint_offset_secs: 0,
            },
            design_conditions: None,
            dry_bulb_c,
            dew_point_c: vec![0.0; n],
            rel_humidity_pct: vec![50.0; n],
            pressure_kpa: vec![101.325; n],
            ghi_w_m2: vec![0.0; n],
            dni_w_m2: vec![0.0; n],
            dhi_w_m2: vec![0.0; n],
            wind_speed_m_s: vec![1.0; n],
            wind_dir_deg: vec![180.0; n],
            opaque_sky_cover: vec![0.0; n],
            horizontal_infrared_w_m2: vec![250.0; n],
            sky_temp_c: vec![0.0; n],
            ground_temp_c: vec![0.0; n],
            liquid_precip_m: vec![0.0; n],
            surface_albedo: None,
        };

        let (_avg, range) = compute_mains_inputs(&weather, step_secs);

        // The Burch-Christensen model requires the monthly-mean range.  When fewer
        // than 2 full months are available, the amplitude should be 0.0 (constant
        // ground temperature = annual mean), not the instantaneous 40°C range.
        // The fallback path uses monthly means only (never simple_range on raw temps).
        assert!(
            (range - 0.0).abs() < 1e-9,
            "expected amplitude ≈ 0.0 for 48-hour weather \
             (fewer than one full calendar month), but got {range:.4} — \
             indicates amplitude is derived from instantaneous hourly extremes \
             instead of monthly means"
        );
    }

    /// DoD requirement: a weather series with exactly 2 full months of data
    /// must derive the amplitude from monthly means, not from instantaneous
    /// hourly extremes.  Additionally, a partial third month must not be
    /// included in the accumulation.
    ///
    /// Fixture: 1416-hour series (31 + 28 = 59 days, non-leap 2 months).
    /// January alternates −10°C / +10°C (mean = 0°C, hourly extremes −10..+10).
    /// February alternates +20°C / +40°C (mean = 30°C, hourly extremes +20..+40).
    /// Old Defect-2 code: `simple_range(&temps)` = 40 − (−10) = 50°C.
    /// Correct code: `simple_range(&month_means)` = 30 − 0 = 30°C.
    #[test]
    fn partial_year_two_months_uses_monthly_mean_range() {
        let step_secs = 3600u32;
        let samples_per_day = 24usize;
        let jan_days = 31usize;
        let feb_days = 28usize;
        let jan_samples = jan_days * samples_per_day;
        let feb_samples = feb_days * samples_per_day;
        let n = jan_samples + feb_samples; // 1416 — exactly 2 calendar months

        // January: alternating −10/+10 °C → mean 0 °C, hourly extremes −10..+10
        // February: alternating +20/+40 °C → mean 30 °C, hourly extremes +20..+40
        let mut dry_bulb = vec![0.0f64; n];
        for (h, v) in dry_bulb.iter_mut().enumerate().take(jan_samples) {
            *v = if h % 2 == 0 { -10.0 } else { 10.0 };
        }
        for (h, v) in dry_bulb.iter_mut().enumerate().skip(jan_samples) {
            *v = if h % 2 == 0 { 20.0 } else { 40.0 };
        }

        let weather = WeatherTimeSeries {
            meta: WeatherMeta {
                location: "TwoMonths-034".to_string(),
                latitude: 40.0,
                longitude: 0.0,
                timezone_offset_h: 0.0,
                elevation_m: 0.0,
                wf_allows_leap_years: true,
                source_step_secs: step_secs,
                midpoint_offset_secs: 0,
            },
            design_conditions: None,
            dry_bulb_c: dry_bulb,
            dew_point_c: vec![0.0; n],
            rel_humidity_pct: vec![50.0; n],
            pressure_kpa: vec![101.325; n],
            ghi_w_m2: vec![0.0; n],
            dni_w_m2: vec![0.0; n],
            dhi_w_m2: vec![0.0; n],
            wind_speed_m_s: vec![1.0; n],
            wind_dir_deg: vec![180.0; n],
            opaque_sky_cover: vec![0.0; n],
            horizontal_infrared_w_m2: vec![250.0; n],
            sky_temp_c: vec![0.0; n],
            ground_temp_c: vec![10.0; n],
            liquid_precip_m: vec![0.0; n],
            surface_albedo: None,
        };

        let (_avg, range) = compute_mains_inputs(&weather, step_secs);

        // With two full months available, the loop processes Jan and Feb
        // (cursor advances through each), then breaks on March because
        // cursor + 31*24 > n.  Jan mean = 0 °C, Feb mean = 30 °C,
        // range = 30 °C.  The old Defect-2 path returned 50 °C
        // (instantaneous extremes −10..+40); the fix returns 30 °C.
        assert!(
            (range - 30.0).abs() < 0.01,
            "expected 2-month range = 30.0 °C (Jan mean = 0 °C, Feb mean = 30 °C), \
             but got {range:.4} — indicates amplitude derivation for partial-year \
             data is not using monthly means"
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
            let start = utc_offset().with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
            let timestamps: Vec<DateTime<FixedOffset>> =
                (0..n).map(|i| start + Duration::hours(i as i64)).collect();
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
                    wf_allows_leap_years: true,
                    source_step_secs: 3600,
                    midpoint_offset_secs: 0,
                },
                design_conditions: None,
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
                .find(|d| d.domain_id == SCHEDULE_DOMAIN_ID)
                .and_then(|d| d.custom_payload.as_ref())
                .and_then(|p| p.first())
                .copied()
                .expect("schedule domain payload")
        }

        /// `None` civil_timezone produces the same schedule index as the
        /// pre-DST fixed-offset behavior.
        #[test]
        fn schedule_without_dst_unchanged() {
            // Use 2023 (non-leap year) to isolate DST behaviour from leap-year
            // schedule offset concerns (T-0032).
            let start = utc_offset().with_ymd_and_hms(2023, 3, 12, 6, 0, 0).unwrap();
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
            // March 12, 2023 (non-leap): ordinal0 = 70.  Hour 6: row = 70*24 + 6 = 1686.
            let val = schedule_val(&env);
            assert!(
                (val - 1686.0).abs() < 1e-6,
                "expected schedule row 1686, got {val}"
            );
        }

        /// Spring forward (America/New_York, 2023-03-12 at 2:00 AM EST → 3:00 AM EDT):
        /// Civil time jumps from 1:59:59 to 3:00:00. Schedule row for civil
        /// hour 2 AM is never accessed; hour 3 AM is used instead.
        /// Uses 2023 (non-leap year) to isolate DST behaviour from leap-year
        /// schedule offset concerns (T-0032).
        #[test]
        fn schedule_spring_forward_skips_civil_hour() {
            // EST = UTC-5. At 2023-03-12T07:00:00Z the wall clock is 2:00 AM EST,
            // which is the instant of spring-forward → becomes 3:00 AM EDT.
            let est = FixedOffset::west_opt(5 * 3600).expect("offset");
            let start = est.with_ymd_and_hms(2023, 3, 12, 1, 0, 0).unwrap();

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
            // March 12 (non-leap): ordinal0 = 70.  Hour 1: row = 70*24 + 1 = 1681.
            assert!(
                (val0 - 1681.0).abs() < 1e-6,
                "step 0: expected civil hour 1 (row 1681), got {val0}"
            );

            let _ = clock.next(); // advance to step 1
            let env1 = mgr.update(&clock, &[]);
            let val1 = schedule_val(&env1);
            // Civil time is now 3:00 AM EDT (skipped 2 AM). Row = 70*24 + 3 = 1683.
            assert!(
                (val1 - 1683.0).abs() < 1e-6,
                "step 1: expected civil hour 3 (row 1683, spring-forward skip), got {val1}"
            );
        }

        /// Fall back (America/New_York, 2023-11-05 at 2:00 AM EDT → 1:00 AM EST):
        /// Civil time 1:00 AM occurs twice. The schedule row for civil hour 1 AM
        /// is reused for both occurrences.
        /// Uses 2023 (non-leap year) to isolate DST behaviour from leap-year
        /// schedule offset concerns (T-0032).
        #[test]
        fn schedule_fall_back_reuses_civil_hour() {
            // EDT = UTC-4. At 2023-11-05T05:00:00Z the wall clock is 1:00 AM EDT.
            // One hour later (06:00Z), clocks fall back: 1:00 AM EST again.
            let edt = FixedOffset::west_opt(4 * 3600).expect("offset");
            let start = edt.with_ymd_and_hms(2023, 11, 5, 0, 0, 0).unwrap();

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
            // Nov 5, 2023 = day 308 (ordinal0). Hour 1: row = 308*24 + 1 = 7393.
            assert!(
                (val1 - 7393.0).abs() < 1e-6,
                "first 1 AM: expected row 7393, got {val1}"
            );

            let _ = clock.next(); // step 2 → civil 2:00 AM EDT → falls back to 1:00 AM EST
            let env2 = mgr.update(&clock, &[]);
            let val2 = schedule_val(&env2);
            // Civil time is 1:00 AM EST (second occurrence). Same row 7393.
            assert!(
                (val2 - 7393.0).abs() < 1e-6,
                "second 1 AM (fall-back): expected row 7393, got {val2}"
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

            let mut clock = SimClock::new(start, Duration::hours(1), Duration::hours(8760));

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
            let start = utc_offset().with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
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

    // ---------------------------------------------------------------
    // Omnidirectional surface geometry tests
    // ---------------------------------------------------------------

    /// Exterior wall with `azimuth_deg: None` must produce a `SurfaceGeometry`
    /// with `omni_directional: true` so the solar loop uses azimuth-averaged
    /// Perez rather than a 180° default.
    #[test]
    fn wall_missing_azimuth_sets_omni_directional_flag() {
        let building = {
            let mut b = building(Some(21.0));
            // Add an exterior wall with no azimuth.
            b.boundaries.push(Boundary {
                id: "wall-no-az".to_string(),
                boundary_type: BoundaryType::Wall,
                area_m2: 15.0,
                azimuth_deg: None,
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
                perimeter_m: None,
                perimeter_insulation_r_m2_k_w: None,
                foundation_depth_m: None,
            });
            b
        };
        let manager = EnvironmentManager::new(
            weather_series(),
            schedule_series(),
            &building,
            StdDuration::from_secs(3600),
            utc_offset().with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap(),
            None,
        )
        .expect("manager");
        let surfaces = manager.surface_geometry();
        let no_az_wall = surfaces
            .iter()
            .find(|s| s.surface_id == 2)
            .expect("wall-no-az surface");
        assert!(
            no_az_wall.omni_directional,
            "exterior wall with missing azimuth must have omni_directional=true, got false"
        );
        // azimuth_deg retained at 180° for non-solar consumers.
        assert!((no_az_wall.azimuth_deg - 180.0).abs() < 1e-9);
    }

    /// Exterior wall with explicit azimuth must have `omni_directional: false`.
    #[test]
    fn wall_with_explicit_azimuth_has_omni_directional_false() {
        let building = building(Some(21.0));
        let manager = EnvironmentManager::new(
            weather_series(),
            schedule_series(),
            &building,
            StdDuration::from_secs(3600),
            utc_offset().with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap(),
            None,
        )
        .expect("manager");
        let surfaces = manager.surface_geometry();
        for s in surfaces {
            assert!(
                !s.omni_directional,
                "surface {} with explicit azimuth must have omni_directional=false, got true",
                s.surface_id
            );
        }
    }

    /// Exterior roof with missing azimuth must set `omni_directional: true`.
    #[test]
    fn roof_missing_azimuth_sets_omni_directional_flag() {
        let building = {
            let mut b = building(Some(21.0));
            b.boundaries.push(Boundary {
                id: "roof-no-az".to_string(),
                boundary_type: BoundaryType::Roof,
                area_m2: 80.0,
                azimuth_deg: None,
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
                tilt_deg: Some(30.0),
                framing_factor: None,
                lut_boundary_name: None,
                floor_or_ceiling: None,
                perimeter_m: None,
                perimeter_insulation_r_m2_k_w: None,
                foundation_depth_m: None,
            });
            b
        };
        let manager = EnvironmentManager::new(
            weather_series(),
            schedule_series(),
            &building,
            StdDuration::from_secs(3600),
            utc_offset().with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap(),
            None,
        )
        .expect("manager");
        let surfaces = manager.surface_geometry();
        let no_az_roof = surfaces
            .iter()
            .find(|s| s.surface_id == 2)
            .expect("roof-no-az surface");
        assert!(
            no_az_roof.omni_directional,
            "exterior roof with missing azimuth must have omni_directional=true, got false"
        );
    }

    /// Interior wall with missing azimuth must NOT be omnidirectional
    /// (azimuth is irrelevant for interior surfaces — no solar gain).
    #[test]
    fn interior_wall_missing_azimuth_is_not_omni_directional() {
        let building = {
            let mut b = building(Some(21.0));
            b.boundaries.push(Boundary {
                id: "interior-wall".to_string(),
                boundary_type: BoundaryType::Wall,
                area_m2: 10.0,
                azimuth_deg: None,
                assembly_r_value_m2_k_w: None,
                r_value_layers_m2_k_w: vec![],
                interior_zone: Some(ZoneType::Conditioned),
                exterior_zone: Some(ZoneType::Conditioned), // interior surface
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
                perimeter_m: None,
                perimeter_insulation_r_m2_k_w: None,
                foundation_depth_m: None,
            });
            b
        };
        let manager = EnvironmentManager::new(
            weather_series(),
            schedule_series(),
            &building,
            StdDuration::from_secs(3600),
            utc_offset().with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap(),
            None,
        )
        .expect("manager");
        let surfaces = manager.surface_geometry();
        let interior_wall = surfaces
            .iter()
            .find(|s| s.surface_id == 2)
            .expect("interior-wall surface");
        assert!(
            !interior_wall.omni_directional,
            "interior wall with missing azimuth must have omni_directional=false, got true"
        );
    }

    /// Omnidirectional wall must produce lower direct irradiance at solar noon
    /// than the south-facing wall, confirming the azimuth averaging avoids
    /// the south-facing over-estimation bias.
    #[test]
    fn omni_wall_direct_lower_than_south_facing_at_noon() {
        let building = {
            // Keep the existing south and north walls, add an omni wall.
            let mut b = building(Some(21.0));
            b.boundaries.push(Boundary {
                id: "omni-wall".to_string(),
                boundary_type: BoundaryType::Wall,
                area_m2: 20.0,
                azimuth_deg: None,
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
                perimeter_m: None,
                perimeter_insulation_r_m2_k_w: None,
                foundation_depth_m: None,
            });
            b
        };
        let mut manager = EnvironmentManager::new(
            weather_series(),
            schedule_series(),
            &building,
            StdDuration::from_secs(3600),
            utc_offset().with_ymd_and_hms(1970, 1, 1, 0, 0, 0).unwrap(),
            None,
        )
        .expect("manager");

        // Solar noon on June 21 at 40°N — south surface 0, north surface 1, omni surface 2.
        let env = manager.update(&clock(), &[]);
        let south = &env.weather.solar_irradiance[0];
        let north = &env.weather.solar_irradiance[1];
        let omni = &env.weather.solar_irradiance[2];

        // South must get the most direct beam; north the least.
        assert!(
            south.direct_w_m2 > omni.direct_w_m2,
            "south direct ({:.1}) must exceed omni direct ({:.1}) at solar noon",
            south.direct_w_m2,
            omni.direct_w_m2
        );
        assert!(
            omni.direct_w_m2 > north.direct_w_m2,
            "omni direct ({:.1}) must exceed north direct ({:.1}) — \
             omnidirectional averaging should place it between south and north",
            omni.direct_w_m2,
            north.direct_w_m2
        );
    }
}
