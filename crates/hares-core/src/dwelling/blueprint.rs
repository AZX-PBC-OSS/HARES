//! Pre-build dwelling state. Equipment can be mutated before calling build().

use std::path::PathBuf;

use chrono::{DateTime, Duration, FixedOffset};
use hares_io::{
    EquipmentSpec, ScheduleTimeSeries, SiteLocation, WeatherTimeSeries, defaults::DefaultsStore,
    epw::DesignConditions, hpxml::resolve_equipment, site_location::resolve_site_location,
};
use hares_types::{EndUse, HaresError};
use serde_json::{Map, Value};

use super::solver_builder::{WeatherAverages, compute_weather_averages};
use crate::derive_dwelling_rng;
use crate::dwelling::DwellingConfig;

/// Pre-build dwelling state. Equipment can be mutated before calling build().
pub struct DwellingBlueprint {
    pub config: DwellingConfig,
    pub(super) building: hares_io::Building,
    pub(super) weather: WeatherTimeSeries,
    pub schedule: ScheduleTimeSeries,
    pub(super) weather_avgs: WeatherAverages,
    pub(super) design_conditions: Option<DesignConditions>,
    pub(super) site_location: SiteLocation,
    pub(super) defaults: DefaultsStore,
    pub defaults_path: Option<PathBuf>,
    pub equipment_specs: Vec<EquipmentSpec>,
    pub(super) local_start: DateTime<FixedOffset>,
    pub(super) init_chrono: Duration,
    pub(super) time_res: std::time::Duration,
    #[cfg(feature = "dst")]
    pub(super) parsed_civil_tz: Option<chrono_tz::Tz>,
    pub(super) rng: rand_chacha::ChaCha8Rng,
}

impl DwellingBlueprint {
    /// Build blueprint from a DwellingConfig by parsing HPXML, weather, and schedule files.
    pub fn from_config(config: DwellingConfig) -> Result<Self, HaresError> {
        let building = hares_io::parse_hpxml(&config.hpxml_path)
            .map_err(|err| HaresError::Io(format!("HPXML parse failed: {err}")))?;

        let weather = hares_io::parse_weather(&config.weather_path)
            .map_err(|err| HaresError::Io(format!("weather parse failed: {err}")))?;

        let schedule_raw = if config.schedule_path.exists() {
            hares_io::parse_schedule_csv(&config.schedule_path, &[], Some(&weather.meta), None)
                .map_err(|err| HaresError::Io(format!("schedule parse failed: {err}")))?
        } else {
            hares_io::hpxml_schedule::generate_schedule_from_hpxml(
                &building,
                config.sim_config.start_time,
                config.sim_config.duration,
                config.sim_config.time_res,
                config.defaults_path.as_deref(),
            )
        };

        let target_step_secs =
            super::conversions::duration_to_u32_secs(config.sim_config.time_res)?;
        let schedule = schedule_raw
            .resample(target_step_secs)
            .map_err(|err| HaresError::Io(format!("schedule resample failed: {err}")))?;

        Self::from_parts(config, building, weather, schedule)
    }

    /// Build blueprint from already-parsed building, weather, and schedule data.
    pub(super) fn from_parts(
        config: DwellingConfig,
        mut building: hares_io::Building,
        mut weather: WeatherTimeSeries,
        schedule: ScheduleTimeSeries,
    ) -> Result<Self, HaresError> {
        let site_location = resolve_site_location(
            &building.site,
            &weather.meta,
            &config.sim_config.site_location,
        );
        weather.meta.latitude = site_location.latitude_deg;
        weather.meta.longitude = site_location.longitude_deg;
        weather.meta.elevation_m = site_location.elevation_m;
        weather.meta.timezone_offset_h = site_location.utc_offset_h;
        weather.meta.has_embedded_location = true;
        building.site.latitude_deg = Some(site_location.latitude_deg);
        building.site.longitude_deg = Some(site_location.longitude_deg);
        building.site.elevation_m = Some(site_location.elevation_m);
        building.site.utc_offset_h = Some(site_location.utc_offset_h);

        let tz_offset = FixedOffset::east_opt((site_location.utc_offset_h * 3600.0).round() as i32)
            .unwrap_or_else(|| FixedOffset::east_opt(0).expect("UTC offset"));
        let local_start = config
            .sim_config
            .start_time
            .naive_local()
            .and_local_timezone(tz_offset)
            .single()
            .unwrap_or_else(|| config.sim_config.start_time.with_timezone(&tz_offset));

        let init_chrono = config
            .initialization_duration
            .map(|d| Duration::seconds(d.as_secs() as i64))
            .unwrap_or(Duration::zero());

        #[cfg(feature = "dst")]
        let parsed_civil_tz: Option<chrono_tz::Tz> = config
            .sim_config
            .civil_timezone
            .as_deref()
            .map(|name| {
                name.parse::<chrono_tz::Tz>()
                    .map_err(|_| HaresError::Io(format!("invalid civil timezone: {name}")))
            })
            .transpose()?;

        let time_res = super::conversions::chrono_to_std_duration(config.sim_config.time_res)?;
        let weather_avgs = compute_weather_averages(&weather);
        let design_conditions = weather.design_conditions;
        let rng = derive_dwelling_rng(config.sim_config.master_seed, config.bldg_id);

        let resolved_defaults_dir = config
            .defaults_path
            .clone()
            .unwrap_or_else(|| PathBuf::from("defaults"));
        let defaults = match DefaultsStore::load(&resolved_defaults_dir) {
            Ok(store) => store,
            // Corrupt defaults data must surface: continuing with an empty
            // store would silently swap every equipment's resolved defaults
            // (ZIP sidecars, HVAC curves) for class-table fallbacks — a
            // structurally normal dwelling running different numbers.
            Err(err @ hares_io::DefaultsError::MalformedToml { .. }) => {
                return Err(HaresError::Io(err.to_string()));
            }
            // Unavailable defaults (missing dir / I/O error) keep the
            // documented degradation: warn and run on the class tables.
            Err(err) => {
                tracing::warn!("defaults load failed; using empty defaults store: {err}");
                DefaultsStore::empty()
            }
        };

        let empty_overrides = Value::Object(Map::new());

        let equipment_specs = resolve_equipment(
            &building,
            &defaults,
            &empty_overrides,
            config.patches.as_ref(),
        )
        .map_err(|e| HaresError::Io(e.to_string()))?;

        Ok(Self {
            config,
            building,
            weather,
            schedule,
            weather_avgs,
            design_conditions,
            site_location,
            defaults,
            defaults_path: Some(resolved_defaults_dir),
            equipment_specs,
            local_start,
            init_chrono,
            time_res,
            #[cfg(feature = "dst")]
            parsed_civil_tz,
            rng,
        })
    }

    /// Return display names (instance_name if set, otherwise canonical name) of all
    /// equipment specs (excluding "Occupancy").
    pub fn equipment_names(&self) -> Vec<&str> {
        self.equipment_specs
            .iter()
            .filter(|s| s.name != "Occupancy")
            .map(|s| s.instance_name.as_deref().unwrap_or(&s.name))
            .collect()
    }

    /// Remove equipment by name. Returns error if not found.
    pub fn remove_equipment(&mut self, name: &str) -> Result<(), HaresError> {
        let pos = self
            .equipment_specs
            .iter()
            .position(|s| s.name == name || s.instance_name.as_deref() == Some(name))
            .ok_or_else(|| {
                HaresError::Dwelling(format!("equipment '{}' not found in blueprint", name))
            })?;
        self.equipment_specs.remove(pos);
        Ok(())
    }

    /// Remove equipment whose end use matches any of the given end uses.
    /// Returns the count of removed equipment specs.
    pub fn remove_equipment_by_end_use(&mut self, end_uses: &[EndUse]) -> usize {
        let before = self.equipment_specs.len();
        self.equipment_specs.retain(|s| {
            s.name == "Occupancy"
                || !end_uses.contains(&hares_io::equipment_name_to_end_use(&s.name))
        });
        before - self.equipment_specs.len()
    }

    /// Add an equipment spec.  Returns an error if an equipment with the same
    /// instance name (or canonical name, when instance name is absent) already
    /// exists in the blueprint.
    pub fn add_equipment_spec(&mut self, spec: EquipmentSpec) -> Result<(), HaresError> {
        let name = spec.instance_name.as_deref().unwrap_or(&spec.name);
        if self
            .equipment_specs
            .iter()
            .any(|s| s.instance_name.as_deref().unwrap_or(&s.name) == name)
        {
            return Err(HaresError::Dwelling(format!(
                "duplicate equipment name '{name}' — each equipment must have a unique name"
            )));
        }
        self.equipment_specs.push(spec);
        Ok(())
    }

    /// Build the dwelling from this blueprint.
    pub fn build(self) -> Result<super::Dwelling, HaresError> {
        super::build_from_blueprint(self)
    }
}
