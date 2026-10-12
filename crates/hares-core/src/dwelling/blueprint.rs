//! Pre-build dwelling state: the builder resolves equipment once and the
//! frozen blueprint builds dwellings that share its parsed inputs.

use std::sync::Arc;

use chrono::{DateTime, Duration, FixedOffset};
use hares_io::{
    EquipmentSpec, InputRecords, ParsedInputs, SiteLocation, epw::DesignConditions,
    hpxml::resolve_equipment, site_location::resolve_site_location,
};
use hares_types::{EndUse, HaresError, Warning};

use super::solver_builder::{WeatherAverages, compute_weather_averages};
use crate::derive_dwelling_rng;
use crate::dwelling::{DwellingConfig, DwellingSettings};

/// The dwelling's pre-build state, immutable and shared as
/// `Arc<DwellingBlueprint>`: the parsed-input handles (one parse per
/// unique input, shared with every other blueprint built from them), the
/// resolved site, the resampled weather and schedule, the resolved
/// equipment, and the derived run constants. Every
/// [`build`](DwellingBlueprint::build) clones the rng and the equipment
/// specs and shares the rest.
pub struct DwellingBlueprint {
    pub(super) inputs: ParsedInputs,
    pub(super) settings: DwellingSettings,
    pub(super) building: Arc<hares_io::Building>,
    pub(super) resampled_weather: Arc<hares_io::WeatherTimeSeries>,
    pub(super) resampled_schedule: Arc<hares_io::ScheduleTimeSeries>,
    pub(super) weather_avgs: WeatherAverages,
    pub(super) design_conditions: Option<DesignConditions>,
    pub(super) site_location: SiteLocation,
    pub(super) equipment_specs: Vec<EquipmentSpec>,
    /// Warnings raised while building the blueprint, in the order they are
    /// reported: the HPXML parse warnings first, then the climate zone
    /// resolution's, then the equipment resolution warnings. The dwelling
    /// build moves these into its warning log before the
    /// schedule-injection warnings and each equipment's `init` warnings.
    pub(super) equipment_warnings: Vec<Warning>,
    pub(super) local_start: DateTime<FixedOffset>,
    pub(super) init_chrono: Duration,
    pub(super) time_res: std::time::Duration,
    #[cfg(feature = "dst")]
    pub(super) parsed_civil_tz: Option<chrono_tz::Tz>,
    pub(super) rng: rand_chacha::ChaCha8Rng,
    /// The input records of the files the config's parse read: the HPXML,
    /// the schedule when it is a file, the weather with its resolved
    /// format, and the defaults digest. `None` when the blueprint was
    /// built from pre-parsed data whose files the parse did not read
    /// (the synthetic TOML dwellings).
    pub(super) input_records: Option<InputRecords>,
}

impl DwellingBlueprint {
    /// Build the blueprint from a config by parsing every input once: the
    /// one construction path through
    /// [`DwellingBlueprintBuilder::from_config`] and [`freeze`]
    /// (DwellingBlueprintBuilder::freeze).
    ///
    /// # Errors
    ///
    /// [`HaresError`] when the config is invalid or any input fails to
    /// parse.
    pub fn from_config(config: DwellingConfig) -> Result<Arc<Self>, HaresError> {
        DwellingBlueprintBuilder::from_config(config)?.freeze()
    }

    /// The blueprint's parsed-input handles: the shared parses.
    #[must_use]
    pub fn inputs(&self) -> &ParsedInputs {
        &self.inputs
    }

    /// The input records of the files the config's parse read: the HPXML,
    /// the schedule when it is a file, the weather with its resolved
    /// format, and the defaults digest. `None` when the parse read no
    /// file (the synthetic assemblies).
    #[must_use]
    pub fn input_records(&self) -> Option<&InputRecords> {
        self.input_records.as_ref()
    }

    /// The blueprint's settings: the sim config, the overrides, the
    /// building id, the warm-up duration and the HPXML patches.
    #[must_use]
    pub fn settings(&self) -> &DwellingSettings {
        &self.settings
    }

    /// The weather series resampled to the blueprint's step, shared with
    /// every blueprint at the same resolution over the same weather file.
    #[must_use]
    pub fn resampled_weather(&self) -> Arc<hares_io::WeatherTimeSeries> {
        Arc::clone(&self.resampled_weather)
    }

    /// The schedule resampled to the blueprint's step.
    #[must_use]
    pub fn resampled_schedule(&self) -> Arc<hares_io::ScheduleTimeSeries> {
        Arc::clone(&self.resampled_schedule)
    }

    /// Build the dwelling from this blueprint: the rng is cloned, the
    /// equipment specs are cloned, the parsed inputs are shared.
    ///
    /// # Errors
    ///
    /// [`HaresError`] when the assembly or the warm-up fails.
    pub fn build(self: &Arc<Self>) -> Result<super::Dwelling, HaresError> {
        super::build_from_blueprint(self)
    }

    /// Build the dwelling from this blueprint with the master seed given,
    /// the rng derived from the seed instead of the blueprint's own.
    ///
    /// # Errors
    ///
    /// [`HaresError`] when the assembly or the warm-up fails.
    pub fn build_with_seed(
        self: &Arc<Self>,
        master_seed: u64,
    ) -> Result<super::Dwelling, HaresError> {
        let mut settings = self.settings.clone();
        settings.sim_config.master_seed = master_seed;
        super::build_from_blueprint_with_seed(self, &settings, master_seed)
    }
}

/// Pre-build dwelling state under construction: the parsed inputs (which
/// [`new`](DwellingBlueprintBuilder::new) never parses) and the settings,
/// with the equipment resolved once and mutable until
/// [`freeze`](DwellingBlueprintBuilder::freeze). The mutators are the
/// blueprint's former ones: equipment inspection and swap before the
/// immutable blueprint is frozen.
pub struct DwellingBlueprintBuilder {
    pub(super) inputs: ParsedInputs,
    pub(super) settings: DwellingSettings,
    pub(super) building: Arc<hares_io::Building>,
    pub(super) site_location: SiteLocation,
    pub(super) equipment_specs: Vec<EquipmentSpec>,
    pub(super) equipment_warnings: Vec<Warning>,
    pub(super) local_start: DateTime<FixedOffset>,
    pub(super) init_chrono: Duration,
    pub(super) time_res: std::time::Duration,
    #[cfg(feature = "dst")]
    pub(super) parsed_civil_tz: Option<chrono_tz::Tz>,
    pub(super) input_records: Option<InputRecords>,
}

impl DwellingBlueprintBuilder {
    /// Assemble the builder over handles parsed once and shared across
    /// blueprints: parses nothing, resolves the site and the equipment
    /// from the handles once.
    ///
    /// # Errors
    ///
    /// [`HaresError`] when the site cannot be resolved or the equipment
    /// resolution fails.
    pub fn new(inputs: ParsedInputs, settings: DwellingSettings) -> Result<Self, HaresError> {
        let building = inputs.hpxml.building();
        let site_location = resolve_site_location(
            &building.site,
            inputs.weather.meta(),
            &settings.sim_config.site_location,
        );

        // The blueprint's construction-time warnings, in report order: the
        // HPXML parse warnings first, then the climate zone resolution's,
        // then the equipment resolution warnings. The parsed building's
        // own warnings are the shared parse's; the zone warning is this
        // blueprint's (it names the weather file's station).
        let mut equipment_warnings: Vec<Warning> = building.parse_warnings.clone();
        if let Some(warning) = &inputs.climate_zone_warning {
            equipment_warnings.push(warning.clone());
        }
        let effective_zone = inputs.effective_climate_zone.clone();

        let time_res = super::conversions::chrono_to_std_duration(settings.sim_config.time_res)?;
        let init_chrono = settings
            .initialization_duration
            .map(|d| Duration::seconds(d.as_secs() as i64))
            .unwrap_or(Duration::zero());

        #[cfg(feature = "dst")]
        let parsed_civil_tz: Option<chrono_tz::Tz> = settings
            .sim_config
            .civil_timezone
            .as_deref()
            .map(|name| {
                name.parse::<chrono_tz::Tz>()
                    .map_err(|_| HaresError::Io(format!("invalid civil timezone: {name}")))
            })
            .transpose()?;

        // A FixedOffset has no gaps or repeated hours, so a naive local
        // wall time converts to exactly one instant at that offset: the
        // infallible construction (naive UTC + offset). The offset is a
        // physical site input, so an unrepresentable or non-finite one is
        // a construction error naming the value (the checked conversion
        // the fleet's alignment pre-pass shares).
        let tz_offset = checked_offset(site_location.utc_offset_h)?;
        let naive_utc = settings.sim_config.start_time.naive_local() - tz_offset;
        let local_start = DateTime::<FixedOffset>::from_naive_utc_and_offset(naive_utc, tz_offset);

        let equipment_specs = resolve_equipment(
            &building,
            effective_zone.as_deref(),
            Some(&site_location),
            &inputs.defaults.store(),
            settings.patches.as_ref(),
            &mut equipment_warnings,
        )
        .map_err(|e| HaresError::Io(e.to_string()))?;

        Ok(Self {
            input_records: inputs.records().cloned(),
            inputs,
            settings,
            building,
            site_location,
            equipment_specs,
            equipment_warnings,
            local_start,
            init_chrono,
            time_res,
            #[cfg(feature = "dst")]
            parsed_civil_tz,
        })
    }

    /// Build the builder from a config by parsing every input once: the
    /// config's validation, [`ParsedInputs::parse`] and [`new`]
    /// (DwellingBlueprintBuilder::new), the one construction path.
    ///
    /// # Errors
    ///
    /// [`HaresError`] when the config is invalid or any input fails to
    /// parse.
    pub fn from_config(config: DwellingConfig) -> Result<Self, HaresError> {
        // The same validation the TOML path runs: a config built in Rust
        // code is checked exactly like one parsed from a file.
        config
            .settings
            .sim_config
            .validate()
            .map_err(|err| HaresError::Io(err.to_string()))?;

        let DwellingConfig {
            hpxml,
            schedule,
            weather,
            defaults,
            settings,
        } = config;
        let generation = hares_io::ScheduleGeneration {
            start: settings.sim_config.start_time,
            duration: settings.sim_config.duration,
            interval: super::conversions::chrono_to_std_duration(settings.sim_config.time_res)?,
        };
        let inputs = ParsedInputs::parse(
            &hpxml,
            schedule.as_ref(),
            &weather,
            &defaults,
            Some(generation),
        )?;
        Self::new(inputs, settings)
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

    /// Remove equipment by name, returning the removed spec: the caller
    /// that re-adds it (the reorder probe) needs the whole spec.
    /// Returns an error naming the name when not found.
    pub fn remove_equipment(&mut self, name: &str) -> Result<EquipmentSpec, HaresError> {
        let pos = self
            .equipment_specs
            .iter()
            .position(|s| s.name == name || s.instance_name.as_deref() == Some(name))
            .ok_or_else(|| {
                HaresError::Dwelling(format!("equipment '{name}' not found in blueprint"))
            })?;
        Ok(self.equipment_specs.remove(pos))
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

    /// Freeze the builder and build the dwelling: the mutator tests' sugar
    /// for `freeze()` followed by `DwellingBlueprint::build`.
    ///
    /// # Errors
    ///
    /// [`HaresError`] when the freeze or the build fails.
    pub fn build(self) -> Result<super::Dwelling, HaresError> {
        self.freeze()?.build()
    }

    /// Freeze the builder into the immutable, shared blueprint: the
    /// resampled weather and schedule, the weather averages and the rng
    /// are derived once, and every build of the blueprint shares them.
    ///
    /// # Errors
    ///
    /// [`HaresError`] when the resample or the weather averages fail.
    pub fn freeze(self) -> Result<Arc<DwellingBlueprint>, HaresError> {
        let DwellingBlueprintBuilder {
            inputs,
            settings,
            building,
            site_location,
            equipment_specs,
            equipment_warnings,
            local_start,
            init_chrono,
            time_res,
            #[cfg(feature = "dst")]
            parsed_civil_tz,
            input_records,
        } = self;

        let step_secs = super::conversions::duration_to_u32_secs(settings.sim_config.time_res)?;
        let resampled_weather = inputs.weather.resampled(step_secs)?;
        let resampled_schedule = Arc::new(
            inputs
                .schedule
                .schedule()
                .resample(step_secs)
                .map_err(|err| HaresError::Io(format!("schedule resample failed: {err}")))?,
        );
        // The averages over the file's own step, the same series the
        // single-dwelling parse averaged.
        let weather_avgs = compute_weather_averages(&inputs.weather.series(), &building)?;
        let design_conditions = inputs.weather.series().design_conditions;
        let rng = derive_dwelling_rng(settings.sim_config.master_seed, settings.bldg_id);

        Ok(Arc::new(DwellingBlueprint {
            inputs,
            settings,
            building,
            resampled_weather,
            resampled_schedule,
            weather_avgs,
            design_conditions,
            site_location,
            equipment_specs,
            equipment_warnings,
            local_start,
            init_chrono,
            time_res,
            #[cfg(feature = "dst")]
            parsed_civil_tz,
            rng,
            input_records,
        }))
    }
}

/// The site's UTC offset hours as a `FixedOffset`, the checked conversion
/// the blueprint reinterprets the naive start at: the offset is a physical
/// site input, so an offset the fixed-offset type cannot represent, or a
/// non-finite one (whose float→int cast would round to zero and silently
/// become UTC), is an error naming the value. The representability check
/// reads the rounded whole seconds, the value `FixedOffset::east_opt`
/// consumes: a pre-rounding guard would let an offset inside ±24 h whose
/// seconds round to exactly ±86 400 through to the type's own rejection.
/// The fleet's alignment pre-pass runs the same conversion, so the pre-pass
/// and the built dwelling cannot disagree except through their inputs.
pub(crate) fn checked_offset(offset_hours: f64) -> Result<FixedOffset, HaresError> {
    let offset_secs = offset_hours * 3600.0;
    let secs = offset_secs.round() as i32;
    if !offset_secs.is_finite() || secs.abs() >= 86_400 {
        return Err(HaresError::Dwelling(format!(
            "site UTC offset {offset_hours} h is not a finite offset within \
             ±24 h; the site's offset is a physical input and no UTC \
             fallback may stand in for it"
        )));
    }
    // A FixedOffset has no gaps or repeated hours; the value was checked
    // finite and within ±24 h after rounding.
    Ok(FixedOffset::east_opt(secs).expect("valid site offset"))
}

/// Reads a home's site UTC offset the way the built dwelling resolves it:
/// the HPXML document's `Site` and the weather file's header (the same
/// read-only-header functions the full parsers call, with no schedule, no
/// defaults tree and no weather row read) and the config's site-location
/// override through `resolve_site_location`, the offset hours converted
/// with the blueprint's checked conversion. The fleet's alignment pre-pass
/// reads every home's offset through this before the first home runs.
///
/// # Errors
///
/// [`HaresError`] when the site or the weather header cannot be read, or
/// the resolved offset is not a finite offset within ±24 h.
pub fn site_offset(config: &DwellingConfig) -> Result<FixedOffset, HaresError> {
    let site = hares_io::read_site_input(&config.hpxml)
        .map_err(|err| HaresError::Io(format!("HPXML site read failed: {err}")))?;
    let weather_meta = hares_io::read_weather_meta_input(&config.weather)
        .map_err(|err| HaresError::Io(format!("weather header read failed: {err}")))?;
    let location = resolve_site_location(
        &site,
        &weather_meta,
        &config.settings.sim_config.site_location,
    );
    checked_offset(location.utc_offset_h)
}
