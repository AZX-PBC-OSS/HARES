//! Pre-build dwelling state. Equipment can be mutated before calling build().

use std::path::PathBuf;

use chrono::{DateTime, Duration, FixedOffset};
use hares_io::{
    EquipmentSpec, ScheduleTimeSeries, SiteLocation, WeatherTimeSeries, defaults::DefaultsStore,
    epw::DesignConditions, hpxml::resolve_equipment, site_location::resolve_site_location,
};
use hares_types::{EndUse, HaresError};

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
    /// Warnings raised while building the blueprint, in the order they are
    /// reported: the HPXML parse warnings first, then the equipment
    /// resolution warnings. The dwelling build moves these into its warning
    /// log before the schedule-injection warnings and each equipment's
    /// `init` warnings.
    pub(super) equipment_warnings: Vec<hares_types::Warning>,
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
        let mut building = hares_io::parse_hpxml(&config.hpxml_path)
            .map_err(|err| HaresError::Io(format!("HPXML parse failed: {err}")))?;

        let weather = hares_io::parse_weather(&config.weather_path)
            .map_err(|err| HaresError::Io(format!("weather parse failed: {err}")))?;
        hares_io::hpxml::climate_zone::apply_climate_zone_default(
            &mut building,
            weather.meta.station_wmo.as_deref(),
        );

        let schedule_raw = config.load_schedule(&weather.meta)?;

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
        building: hares_io::Building,
        weather: WeatherTimeSeries,
        schedule: ScheduleTimeSeries,
    ) -> Result<Self, HaresError> {
        let resolved_defaults_dir = config
            .defaults_path
            .clone()
            .unwrap_or_else(|| PathBuf::from("defaults"));
        let defaults = DefaultsStore::load(&resolved_defaults_dir).map_err(|err| {
            HaresError::Io(format!(
                "defaults load failed for '{}': {err}",
                resolved_defaults_dir.display()
            ))
        })?;
        Self::from_parts_with_defaults(config, building, weather, schedule, defaults)
    }

    /// Build blueprint from already-parsed data and an explicit defaults
    /// store: the typed constructor for a caller that has already loaded (or
    /// deliberately declined) defaults. A synthetic dwelling, whose inputs
    /// are fully explicit, passes [`DefaultsStore::empty`]; no caller reaches
    /// an empty store through a missing file (one policy: the store's load
    /// failure is an error naming the path).
    pub(super) fn from_parts_with_defaults(
        config: DwellingConfig,
        mut building: hares_io::Building,
        mut weather: WeatherTimeSeries,
        schedule: ScheduleTimeSeries,
        defaults: DefaultsStore,
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
        let weather_avgs = compute_weather_averages(&weather, &building)?;
        let design_conditions = weather.design_conditions;
        let rng = derive_dwelling_rng(config.sim_config.master_seed, config.bldg_id);

        // The blueprint's construction-time warnings, in report order: the
        // HPXML parse warnings first, then the equipment resolution warnings.
        let mut equipment_warnings = building.parse_warnings.clone();
        let equipment_specs = resolve_equipment(
            &building,
            &defaults,
            config.patches.as_ref(),
            &mut equipment_warnings,
        )
        .map_err(|e| HaresError::Io(e.to_string()))?;

        // The config's own defaults directory, verbatim: the schedule
        // profiles load from what the config named, and a config with
        // no directory runs with no default profiles rather than on a
        // working-directory guess. The store above keeps the resolved
        // directory until the defaults path becomes required.
        let config_defaults_path = config.defaults_path.clone();
        Ok(Self {
            config,
            building,
            weather,
            schedule,
            weather_avgs,
            design_conditions,
            site_location,
            defaults,
            defaults_path: config_defaults_path,
            equipment_specs,
            equipment_warnings,
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
    pub fn add_equipment_spec(&mut self, mut spec: EquipmentSpec) -> Result<(), HaresError> {
        // The caller's spec-level overrides land from the dedicated
        // override field here, so the one config generation the assembly
        // builds from carries them; an override key no schema field reads
        // is an error naming the equipment and the field.
        super::conversions::apply_spec_bag_to_typed_config(&mut spec)?;
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
