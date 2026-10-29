//! Pre-build dwelling state: the builder resolves equipment once and the
//! frozen blueprint builds dwellings that share its parsed inputs.

use std::sync::Arc;

use chrono::{DateTime, Datelike, Duration, FixedOffset};
use hares_io::{
    Catalogue, EquipmentSpec, InputRecords, InputSource, ParsedInputs, SiteLocation, WeatherInput,
    epw::DesignConditions, hpxml::resolve_equipment, site_location::resolve_site_location,
};
use hares_types::{EndUse, HaresError, Warning};

use super::solver_builder::{WeatherAverages, compute_weather_averages};
use crate::derive_dwelling_rng_scenario;
use crate::dwelling::{DwellingConfig, DwellingSettings, WarningLog};
use crate::resolved::ResolvedModel;

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
    /// Whether a `DwellingBlueprintBuilder` mutator changed the parsed
    /// equipment before this blueprint froze: a run built from it is not
    /// the inputs' resolution a replay would rebuild, so the dwelling
    /// records `ReplayBlocker::BlueprintEdited` for it.
    pub(super) blueprint_edited: bool,
    /// The input records of the files the config's parse read: the HPXML,
    /// the schedule when it is a file, the weather with its resolved
    /// format, and the defaults digest. `None` for the synthetic TOML
    /// dwellings: that path reads the TOML, recorded nowhere in the
    /// records, and the absence discloses that.
    pub(super) input_records: Option<InputRecords>,
    /// The per-surface irradiance table, built on the blueprint's first
    /// build from its surfaces and its forcing table and shared by every
    /// dwelling the blueprint builds (the RT-2 record's per-surface
    /// table): the per-surface Perez and incidence are functions of the
    /// step's weather and solar position and of the blueprint's own
    /// surface geometry, so the table is one computation per blueprint.
    /// The process's content registry extends the sharing across
    /// independently constructed blueprints of identical content: the
    /// same surfaces, weather, forcing and time base resolve to one table
    /// instance, different content never shares. Empty until the first
    /// build; never built when the per-step forcing path was forced at
    /// the first build.
    pub(super) per_surface_irradiance:
        std::sync::OnceLock<std::sync::Arc<crate::environment::SurfaceIrradianceTable>>,
}

impl DwellingBlueprint {
    /// Build the blueprint from a config by parsing every input once: the
    /// one construction path through
    /// [`DwellingBlueprintBuilder::from_config`] and
    /// [`DwellingBlueprintBuilder::freeze`], the one construction path
    /// through the builder.
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

    /// Run the assembly alone and read the resolved model back: every
    /// construction-time check `build` runs, through the same code, with
    /// no warm-up and no step. The returned `ResolvedModel` is the one the
    /// built dwelling's `resolved_model()` carries. An infeasible input
    /// fails here with the same error the build fails with, in a fraction
    /// of its cost (the warm-up's simulated days are the build's cost the
    /// caller does not pay to learn the input is infeasible).
    ///
    /// # Errors
    ///
    /// [`HaresError`] when the assembly fails, naming the cause the build
    /// names.
    pub fn validate(self: &Arc<Self>) -> Result<ResolvedModel, HaresError> {
        let dwelling = self.assemble_for_readback()?;
        Ok(dwelling.into_resolved_model())
    }

    /// Run the assembly alone and return the output variable catalogue:
    /// the equipment and the auto-registered actors the assembly
    /// instantiates, with no warm-up and no step. The catalogue equals the
    /// built dwelling's `catalogue()`; variables of actors a caller
    /// registers or `set_tariff` adds after build are not in it.
    ///
    /// # Errors
    ///
    /// [`HaresError`] when the assembly fails, naming the cause the build
    /// names.
    pub fn catalogue(self: &Arc<Self>) -> Result<Arc<Catalogue>, HaresError> {
        let dwelling = self.assemble_for_readback()?;
        Ok(dwelling.catalogue())
    }

    /// The assembly phase alone, with the blueprint's own settings: the
    /// construction-time warnings seeded the build seeds them, and a
    /// failure emits them the way the build's failure path emits them.
    fn assemble_for_readback(self: &Arc<Self>) -> Result<super::Dwelling, HaresError> {
        let mut warnings = WarningLog::new();
        for warning in &self.equipment_warnings {
            warnings.push_warning(warning.clone());
        }
        super::assemble_from_blueprint_with_seed(
            self,
            &self.settings,
            self.settings.sim_config.master_seed,
            &mut warnings,
        )
        .map_err(|err| {
            for entry in warnings.iter() {
                tracing::warn!("{entry}");
            }
            if warnings.dropped() > 0 {
                tracing::warn!(
                    "warning log capacity ({}) reached; {} additional warning(s) were dropped",
                    WarningLog::CAPACITY,
                    warnings.dropped()
                );
            }
            tracing::warn!("dwelling construction failed: {err}");
            err
        })
    }

    /// Build the dwelling from this blueprint with the master seed given,
    /// the rng derived from the seed instead of the blueprint's own.
    ///
    /// The generated schedule's diversity draws key on the same
    /// scenario-mixed per-dwelling key as the rng, so a build under
    /// another seed re-derives the schedule under that seed's key before
    /// the assembly reads it; the rest of the blueprint (the parses, the
    /// equipment, the weather) is seed-independent and passes through.
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
        // Only a generated, diversified schedule keys on the master seed;
        // every other blueprint builds in place with no re-derivation.
        if master_seed == self.settings.sim_config.master_seed
            || !self.inputs.schedule.was_generated()
            || !self.settings.sim_config.schedule_diversity.is_on()
        {
            return super::build_from_blueprint_with_seed(self, &settings, master_seed);
        }
        let rebuilt = self.refrozen(master_seed)?;
        super::build_from_blueprint_with_seed(&rebuilt, &settings, master_seed)
    }

    /// The blueprint re-frozen under `master_seed`: the generated
    /// schedule re-derives under the seed's key (the one field the seed
    /// keys beyond the rng, which the freeze re-derives), and everything
    /// else is the frozen blueprint's own.
    fn refrozen(self: &Arc<Self>, master_seed: u64) -> Result<Arc<Self>, HaresError> {
        let mut builder = DwellingBlueprintBuilder {
            inputs: self.inputs.clone(),
            settings: self.settings.clone(),
            building: Arc::clone(&self.building),
            site_location: self.site_location,
            equipment_specs: self.equipment_specs.clone(),
            equipment_warnings: self.equipment_warnings.clone(),
            local_start: self.local_start,
            init_chrono: self.init_chrono,
            time_res: self.time_res,
            #[cfg(feature = "dst")]
            parsed_civil_tz: self.parsed_civil_tz,
            input_records: self.input_records.clone(),
            blueprint_edited: self.blueprint_edited,
        };
        builder.settings.sim_config.master_seed = master_seed;
        builder.rediversify_schedule()?;
        builder.freeze()
    }
}

/// Pre-build dwelling state under construction: the parsed inputs (which
/// [`new`](DwellingBlueprintBuilder::new) never parses) and the settings,
/// with the equipment resolved once and mutable until
/// [`freeze`](DwellingBlueprintBuilder::freeze). The mutators are the
/// blueprint's former ones: equipment inspection and swap before the
/// immutable blueprint is frozen. A read-only call that runs the assembly
/// without consuming the builder (`validate`, `catalogue` on the Python
/// surface) clones it: every field is a shared handle or a small vector.
#[derive(Clone)]
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
    /// Whether a mutator changed the parsed equipment: frozen into the
    /// blueprint as the blueprint-edited replay blocker's record.
    pub(super) blueprint_edited: bool,
}

/// The generated-schedule family's behavioral diversity: a no-CSV
/// dwelling's generated schedule is diversified under the dwelling's
/// scenario-mixed key before anything reads it. A CSV schedule's or a
/// code-built series' inputs pass through untouched, and the config's knob
/// turns the layer off. The span is the run's own, the same span the
/// schedule was generated over.
fn diversify_generated_schedule(
    inputs: ParsedInputs,
    settings: &DwellingSettings,
) -> Result<ParsedInputs, HaresError> {
    let generation = hares_io::ScheduleGeneration {
        start: settings.sim_config.start_time,
        duration: settings.sim_config.duration,
        interval: super::conversions::chrono_to_std_duration(settings.sim_config.time_res)?,
    };
    let dwelling_key = hares_types::rng::dwelling_seed_scenario(
        settings.sim_config.master_seed,
        settings.sim_config.scenario_seed,
        settings.bldg_id,
    );
    inputs.diversify_generated_schedule(
        generation,
        dwelling_key,
        settings.sim_config.schedule_diversity.is_on(),
    )
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
        let inputs = diversify_generated_schedule(inputs, &settings)?;
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
        // The warm-up's extra clock span: a converging warm-up replays one
        // 24-hour day (the run's first day), so the clock needs a day's
        // span beyond the run; disabled, none.
        let init_chrono = if settings.sim_config.warmup.config().is_some() {
            Duration::hours(24)
        } else {
            Duration::zero()
        };

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

        // A start_time on a skipped Feb 29 is an error naming the date:
        // the day is not simulated (the weather-year rule's Feb 29 skip
        // depends on the weather file, which SimulationConfig::validate
        // does not see), so the run cannot start there.
        if settings
            .sim_config
            .weather_year
            .skips_feb29(&inputs.weather.series())
        {
            let start_date = settings.sim_config.start_time.naive_local().date();
            if start_date.month() == 2 && start_date.day() == 29 {
                return Err(HaresError::Io(format!(
                    "the simulation starts on {start_date}, a Feb 29 the weather file has no \
                     records for: a typical-year run over a file without Feb 29 does not \
                     simulate that day, so the run cannot start on it"
                )));
            }
        }

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
            blueprint_edited: false,
        })
    }

    /// Build the builder from a config by parsing every input once: the
    /// config's validation, [`ParsedInputs::parse`] and
    /// [`DwellingBlueprintBuilder::new`], the one construction path.
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

    /// Re-derive the generated schedule under the settings' current
    /// master seed: the same transform the constructor runs, for a build
    /// under another seed (`DwellingBlueprint::build_with_seed`).
    fn rediversify_schedule(&mut self) -> Result<(), HaresError> {
        self.inputs = diversify_generated_schedule(self.inputs.clone(), &self.settings)?;
        Ok(())
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
        self.blueprint_edited = true;
        Ok(self.equipment_specs.remove(pos))
    }

    /// Remove equipment whose declared end use matches any of the given
    /// end uses. The match reads the class's typed declaration
    /// (`hares_equipment::class_declared_surface`), never the name
    /// string: a spec whose name is not a registered class declares no
    /// end use and no end-use removal addresses it. Returns the count of
    /// removed equipment specs.
    pub fn remove_equipment_by_end_use(&mut self, end_uses: &[EndUse]) -> usize {
        let before = self.equipment_specs.len();
        self.equipment_specs.retain(|s| {
            s.name == "Occupancy"
                || !hares_equipment::class_declared_surface(&s.name)
                    .is_some_and(|surface| end_uses.contains(&surface.end_use))
        });
        if self.equipment_specs.len() != before {
            self.blueprint_edited = true;
        }
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
        self.blueprint_edited = true;
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
            blueprint_edited,
        } = self;

        let step_secs = super::conversions::duration_to_u32_secs(settings.sim_config.time_res)?;
        // The schedule file's four unavailable-period flag columns must
        // match the HPXML periods on the file's own resolution, before
        // any resample: HARES reads the columns and ignores nothing.
        hares_io::check_schedule_flag_columns(
            &inputs.schedule.schedule(),
            &building.unavailable_periods,
        )?;
        let resampled_weather = inputs.weather.resampled(
            step_secs,
            settings.sim_config.first_hour_interpolation,
            settings.sim_config.start_time.naive_local(),
            settings.sim_config.weather_year,
        )?;
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
        let rng = derive_dwelling_rng_scenario(
            settings.sim_config.master_seed,
            settings.sim_config.scenario_seed,
            settings.bldg_id,
        );

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
            blueprint_edited,
            input_records,
            per_surface_irradiance: std::sync::OnceLock::new(),
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

/// Reads a home's site UTC offset from its inputs the way the built
/// dwelling resolves it: the HPXML document's `Site` and the weather
/// file's header (the same read-only-header functions the full parsers
/// call, with no schedule, no defaults tree and no weather row read) and
/// the config's site-location override through `resolve_site_location`,
/// the offset hours converted with the blueprint's checked conversion.
/// The fleet's alignment pre-pass reads every home's offset through this
/// before the first home runs.
///
/// # Errors
///
/// [`HaresError`] when the site or the weather header cannot be read, or
/// the resolved offset is not a finite offset within ±24 h.
pub fn site_offset(config: &DwellingConfig) -> Result<FixedOffset, HaresError> {
    site_offset_from_inputs(
        &config.hpxml,
        &config.weather,
        &config.settings.sim_config.site_location,
    )
}

/// The site-offset resolution over one home's inputs directly: the same
/// read `site_offset` runs on a config's fields, for a caller that
/// resolves the offset before it has built the config (the ResStock
/// fleet's constructor, which sets every home's start from the offset
/// the config will then carry).
///
/// # Errors
///
/// [`HaresError`] when the site or the weather header cannot be read, or
/// the resolved offset is not a finite offset within ±24 h.
pub fn site_offset_from_inputs(
    hpxml: &InputSource,
    weather: &WeatherInput,
    override_location: &hares_io::SiteLocationOverride,
) -> Result<FixedOffset, HaresError> {
    let site = hares_io::read_site_input(hpxml)
        .map_err(|err| HaresError::Io(format!("HPXML site read failed: {err}")))?;
    let weather_meta = hares_io::read_weather_meta_input(weather)
        .map_err(|err| HaresError::Io(format!("weather header read failed: {err}")))?;
    let location = resolve_site_location(&site, &weather_meta, override_location);
    checked_offset(location.utc_offset_h)
}
