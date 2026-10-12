//! Python bindings for pre-build dwelling blueprint: the builder mutates
//! equipment, and `build()` freezes it and builds the dwelling from the
//! frozen, shared blueprint.

use std::sync::Arc;

use hares_core::{DwellingBlueprint, DwellingBlueprintBuilder, DwellingConfig};
use hares_io::{ParsedDefaults, ParsedHpxml, ParsedSchedule, ParsedWeather};
use hares_types::EndUse;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyType};

use crate::py_dwelling::{PyDwelling, construct_detached, to_py_err};
use crate::py_enums::PyEndUse;
use crate::py_hvac::{
    PyASHPCooler, PyASHPHeater, PyAirConditioner, PyElectricBaseboard, PyElectricBoiler,
    PyElectricFurnace, PyGasBoiler, PyGasFurnace, PyIdealHVAC, ac_spec_from_py,
    ashp_cooler_spec_from_py, ashp_heater_spec_from_py, baseboard_spec_from_py,
    electric_boiler_spec_from_py, electric_furnace_spec_from_py, gas_boiler_spec_from_py,
    gas_furnace_spec_from_py, ideal_hvac_spec_from_py,
};
use crate::py_inputs::{
    DefaultsArg, HpxmlArg, PyParsedDefaults, PyParsedHpxml, PyParsedSchedule, PyParsedWeather,
    ScheduleArg, WeatherArg, defaults_arg, hpxml_arg, schedule_arg, weather_arg,
};
use crate::py_water_heater::{
    PyElectricResistanceWH, PyGasWaterHeater, PyHeatPumpWH, PyIndirectTank, PyTanklessWaterHeater,
    elec_res_wh_spec_from_py, gas_wh_spec_from_py, hpwh_spec_from_py, indirect_tank_spec_from_py,
    tankless_wh_spec_from_py,
};

#[pyclass(name = "DwellingBlueprint")]
#[doc = "A two-phase dwelling blueprint: first inspect and swap equipment, then build."]
pub struct PyDwellingBlueprint {
    builder: Option<DwellingBlueprintBuilder>,
    config: DwellingConfig,
}

#[pymethods]
impl PyDwellingBlueprint {
    /// Create a DwellingBlueprint from HPXML, schedule, and weather file paths
    /// or parsed handles. A handle is shared: its parse is used as is, a
    /// path is parsed here.
    #[classmethod]
    #[pyo3(signature = (hpxml, schedule, weather, config, *, bldg_id = 0, initialization_duration = None, defaults = None, overrides = None, **unexpected))]
    // PyO3 #[classmethod] constructor must match the Python API parameter list.
    #[allow(clippy::too_many_arguments)]
    pub fn from_hpxml(
        _cls: &Bound<'_, PyType>,
        hpxml: &Bound<'_, PyAny>,
        schedule: Option<&Bound<'_, PyAny>>,
        weather: &Bound<'_, PyAny>,
        config: crate::py_config::PySimulationConfig,
        bldg_id: i64,
        initialization_duration: Option<i64>,
        defaults: Option<&Bound<'_, PyAny>>,
        overrides: Option<&Bound<'_, PyDict>>,
        unexpected: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Self> {
        // The dwelling-level keywords are the config's own; every simulation
        // setting rides on the one SimulationConfig object. Every handle
        // argument is shared: its parse is used as is. The sources' parses
        // and the assembly run in the detached section, the GIL released.
        let (config, inputs) =
            crate::py_config::resolve_args(crate::py_config::DwellingConfigArgs {
                hpxml: hpxml_arg(hpxml)?,
                schedule: schedule_arg_maybe(schedule)?,
                weather: weather_arg(weather)?,
                defaults: match defaults {
                    Some(path) => defaults_arg(path)?,
                    None => DefaultsArg::Source(hares_io::DefaultsSource::default()),
                },
                config,
                bldg_id,
                initialization_duration,
                overrides,
                unexpected,
            })?;
        let settings = config.settings.clone();
        let builder = construct_detached(_cls.py(), move || {
            let inputs = builder_inputs(&inputs)?;
            DwellingBlueprintBuilder::new(inputs, settings)
        })
        .map_err(to_py_err)?;
        Ok(Self {
            builder: Some(builder),
            config,
        })
    }

    /// Return the list of equipment names currently in the blueprint.
    pub fn equipment_names(&self) -> PyResult<Vec<String>> {
        let bp = self
            .builder
            .as_ref()
            .ok_or_else(|| PyValueError::new_err("DwellingBlueprint has already been built"))?;
        Ok(bp.equipment_names().into_iter().map(String::from).collect())
    }

    /// Remove equipment by its instance name.
    pub fn remove_equipment(&mut self, name: &str) -> PyResult<()> {
        let bp = self
            .builder
            .as_mut()
            .ok_or_else(|| PyValueError::new_err("DwellingBlueprint has already been built"))?;
        let _removed = bp.remove_equipment(name).map_err(to_py_err)?;
        Ok(())
    }

    /// Remove all equipment serving the given end-use(s).
    /// Accepts a single EndUse or a list[EndUse].
    pub fn remove_equipment_by_end_use(&mut self, end_uses: &Bound<'_, PyAny>) -> PyResult<usize> {
        let bp = self
            .builder
            .as_mut()
            .ok_or_else(|| PyValueError::new_err("DwellingBlueprint has already been built"))?;
        let rust_uses: Vec<EndUse> = if let Ok(single) = end_uses.extract::<PyEndUse>() {
            vec![EndUse::from(single)]
        } else if let Ok(list) = end_uses.extract::<Vec<PyEndUse>>() {
            list.into_iter().map(EndUse::from).collect()
        } else {
            return Err(PyValueError::new_err(
                "end_uses must be an EndUse or list[EndUse]",
            ));
        };
        Ok(bp.remove_equipment_by_end_use(&rust_uses))
    }

    /// Add a typed equipment config object (GasFurnace, ASHPHeater, etc.).
    pub fn add_equipment(&mut self, obj: &Bound<'_, PyAny>) -> PyResult<()> {
        let bp = self
            .builder
            .as_mut()
            .ok_or_else(|| PyValueError::new_err("DwellingBlueprint has already been built"))?;
        let spec = py_any_to_equipment_spec(obj)?;
        bp.add_equipment_spec(spec).map_err(to_py_err)?;
        Ok(())
    }

    /// Finalise the blueprint and return a PyDwelling ready for simulation.
    /// Consumes the blueprint; calling build() a second time will error.
    pub fn build(&mut self, py: Python<'_>) -> PyResult<PyDwelling> {
        let builder = self
            .builder
            .take()
            .ok_or_else(|| PyValueError::new_err("DwellingBlueprint has already been built"))?;
        let blueprint = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            construct_detached(py, move || builder.freeze())
        }))
        .map_err(|_| PyValueError::new_err("DwellingBlueprint::build() panicked"))?
        .map_err(to_py_err)?;
        let dwelling = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            construct_detached(py, {
                let blueprint = Arc::clone(&blueprint);
                move || blueprint.build()
            })
        }))
        .map_err(|_| PyValueError::new_err("DwellingBlueprint::build() panicked"))?
        .map_err(to_py_err)?;
        Ok(PyDwelling::from_blueprint_build(
            dwelling,
            self.config.clone(),
            blueprint,
        ))
    }

    /// Run the assembly alone and read the resolved model back as a dict:
    /// every construction-time check `build` runs, with no warm-up and no
    /// step. The same dict form `Dwelling.resolved_model()` returns. The
    /// blueprint is not consumed: the builder is cloned, the frozen
    /// blueprint the clone freezes is validated and dropped.
    pub fn validate(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let blueprint = self.frozen_for_readback(py, "validate")?;
        let model = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            construct_detached(py, move || blueprint.validate())
        }))
        .map_err(|_| PyValueError::new_err("DwellingBlueprint::validate() panicked"))?
        .map_err(to_py_err)?;
        resolved_model_to_py(py, &model)
    }

    /// Run the assembly alone and return the output variable catalogue:
    /// the equipment and the auto-registered actors, with no warm-up and
    /// no step. Equals the built dwelling's `catalogue()`; variables of
    /// actors a caller registers after build are not in it.
    pub fn catalogue(&self, py: Python<'_>) -> PyResult<crate::py_catalogue::PyCatalogue> {
        let blueprint = self.frozen_for_readback(py, "catalogue")?;
        let catalogue = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            construct_detached(py, move || blueprint.catalogue())
        }))
        .map_err(|_| PyValueError::new_err("DwellingBlueprint::catalogue() panicked"))?
        .map_err(to_py_err)?;
        Ok(crate::py_catalogue::PyCatalogue { inner: catalogue })
    }
}

impl PyDwellingBlueprint {
    /// The frozen blueprint the read-only calls run on: the builder
    /// cloned and frozen (the builder itself is untouched, so `build()`
    /// still works after a `validate()` or `catalogue()`).
    fn frozen_for_readback(
        &self,
        py: Python<'_>,
        context: &str,
    ) -> PyResult<std::sync::Arc<hares_core::DwellingBlueprint>> {
        let builder = self
            .builder
            .as_ref()
            .ok_or_else(|| PyValueError::new_err("DwellingBlueprint has already been built"))?;
        let message = format!("DwellingBlueprint::{context}() panicked");
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            construct_detached(py, move || builder.clone().freeze())
        }))
        .map_err(|_| PyValueError::new_err(message))?
        .map_err(to_py_err)
    }
}

/// The shared-parse assembly every Python-facing dwelling constructor
/// runs in its detached section: handles used as is, sources parsed
/// there, the schedule generated from the defaults handle's profiles when
/// the schedule argument is absent, and the effective climate zone
/// resolved once from the HPXML building and the weather header.
pub(crate) fn builder_inputs(
    inputs: &crate::py_config::ConstructorInputs,
) -> Result<hares_io::ParsedInputs, hares_types::HaresError> {
    let defaults_handle: Arc<ParsedDefaults> = match &inputs.defaults {
        DefaultsArg::Parsed(handle) => Arc::clone(handle),
        DefaultsArg::Source(source) => Arc::new(ParsedDefaults::parse(source)?),
    };
    let weather_handle: Arc<ParsedWeather> = match &inputs.weather {
        WeatherArg::Parsed(handle) => Arc::clone(handle),
        WeatherArg::Source(source) => Arc::new(ParsedWeather::parse(source)?),
    };
    let hpxml_handle: Arc<ParsedHpxml> = match &inputs.hpxml {
        HpxmlArg::Parsed(handle) => Arc::clone(handle),
        HpxmlArg::Source(source) => Arc::new(ParsedHpxml::parse(source)?),
    };
    let schedule_handle: Option<Arc<ParsedSchedule>> = match &inputs.schedule {
        Some(ScheduleArg::Parsed(handle)) => Some(Arc::clone(handle)),
        Some(ScheduleArg::Source(source)) => Some(Arc::new(ParsedSchedule::parse(
            source,
            Some(weather_handle.meta()),
        )?)),
        None => None,
    };
    let mut parsed = hares_io::ParsedInputs::from_handles(
        hpxml_handle,
        schedule_handle,
        weather_handle,
        defaults_handle,
        inputs.generation,
    )?;
    // Every argument a source (the caller's path or content): the run
    // built from them records its inputs, the parse's own records. An
    // argument a pre-parsed handle: this construction read no file for
    // it, the handle's own parse recorded it where it happened, and a
    // mixed assembly records nothing (the records' `None` discloses
    // that not every input was a source here).
    let all_sources = matches!(inputs.hpxml, crate::py_inputs::HpxmlArg::Source(_))
        && matches!(inputs.weather, crate::py_inputs::WeatherArg::Source(_))
        && matches!(inputs.defaults, DefaultsArg::Source(_))
        && inputs
            .schedule
            .as_ref()
            .is_none_or(|schedule| matches!(schedule, ScheduleArg::Source(_)));
    if all_sources {
        parsed.compute_records()?;
    }
    Ok(parsed)
}

/// A read-only reference to the frozen blueprint a dwelling was built
/// from: the parsed-input handles it shares.
#[pyclass(name = "DwellingBlueprintRef")]
#[doc = "The frozen blueprint a dwelling was built from."]
pub struct PyDwellingBlueprintRef {
    pub(crate) inner: Arc<DwellingBlueprint>,
}

#[pymethods]
impl PyDwellingBlueprintRef {
    /// The blueprint's parsed-input handles, shared: the same `Arc`s every
    /// build of the blueprint holds.
    pub fn parsed_inputs(&self) -> ParsedInputsRef {
        ParsedInputsRef {
            hpxml: Arc::clone(&self.inner.inputs().hpxml),
            weather: Arc::clone(&self.inner.inputs().weather),
            schedule: Arc::clone(&self.inner.inputs().schedule),
            defaults: Arc::clone(&self.inner.inputs().defaults),
        }
    }
}

/// The parsed-input handles' read surface.
#[pyclass(frozen, name = "ParsedInputs")]
#[doc = "The parsed-input handles one blueprint is built from."]
pub struct ParsedInputsRef {
    hpxml: Arc<ParsedHpxml>,
    weather: Arc<ParsedWeather>,
    schedule: Arc<ParsedSchedule>,
    defaults: Arc<ParsedDefaults>,
}

#[pymethods]
impl ParsedInputsRef {
    /// The shared HPXML handle.
    #[getter]
    pub fn hpxml(&self) -> PyParsedHpxml {
        PyParsedHpxml {
            inner: Arc::clone(&self.hpxml),
        }
    }

    /// The shared weather handle.
    #[getter]
    pub fn weather(&self) -> PyParsedWeather {
        PyParsedWeather {
            inner: Arc::clone(&self.weather),
        }
    }

    /// The shared schedule handle.
    #[getter]
    pub fn schedule(&self) -> PyParsedSchedule {
        PyParsedSchedule {
            inner: Arc::clone(&self.schedule),
        }
    }

    /// The shared defaults handle.
    #[getter]
    pub fn defaults(&self) -> PyParsedDefaults {
        PyParsedDefaults {
            inner: Arc::clone(&self.defaults),
        }
    }
}

/// The `schedule` argument that may be absent entirely: `None` means no
/// schedule argument, `Some(arg)` the path/handle resolution.
pub(crate) fn schedule_arg_maybe(obj: Option<&Bound<'_, PyAny>>) -> PyResult<Option<ScheduleArg>> {
    match obj {
        None => Ok(None),
        Some(obj) => schedule_arg(obj),
    }
}

fn py_any_to_equipment_spec(obj: &Bound<'_, PyAny>) -> PyResult<hares_io::EquipmentSpec> {
    if let Ok(f) = obj.extract::<PyRef<'_, PyGasFurnace>>() {
        return gas_furnace_spec_from_py(&f);
    }
    if let Ok(ac) = obj.extract::<PyRef<'_, PyAirConditioner>>() {
        return ac_spec_from_py(&ac);
    }
    if let Ok(hp) = obj.extract::<PyRef<'_, PyASHPHeater>>() {
        return ashp_heater_spec_from_py(&hp);
    }
    if let Ok(hp) = obj.extract::<PyRef<'_, PyASHPCooler>>() {
        return ashp_cooler_spec_from_py(&hp);
    }
    if let Ok(hvac) = obj.extract::<PyRef<'_, PyIdealHVAC>>() {
        return ideal_hvac_spec_from_py(&hvac);
    }
    if let Ok(bb) = obj.extract::<PyRef<'_, PyElectricBaseboard>>() {
        return baseboard_spec_from_py(&bb);
    }
    if let Ok(wh) = obj.extract::<PyRef<'_, PyGasWaterHeater>>() {
        return gas_wh_spec_from_py(&wh);
    }
    if let Ok(wh) = obj.extract::<PyRef<'_, PyElectricResistanceWH>>() {
        return elec_res_wh_spec_from_py(&wh);
    }
    if let Ok(wh) = obj.extract::<PyRef<'_, PyHeatPumpWH>>() {
        return hpwh_spec_from_py(&wh);
    }
    if let Ok(gb) = obj.extract::<PyRef<'_, PyGasBoiler>>() {
        return gas_boiler_spec_from_py(&gb);
    }
    if let Ok(eb) = obj.extract::<PyRef<'_, PyElectricBoiler>>() {
        return electric_boiler_spec_from_py(&eb);
    }
    if let Ok(ef) = obj.extract::<PyRef<'_, PyElectricFurnace>>() {
        return electric_furnace_spec_from_py(&ef);
    }
    if let Ok(wh) = obj.extract::<PyRef<'_, PyTanklessWaterHeater>>() {
        return tankless_wh_spec_from_py(&wh);
    }
    if let Ok(it) = obj.extract::<PyRef<'_, PyIndirectTank>>() {
        return indirect_tank_spec_from_py(&it);
    }
    Err(PyValueError::new_err(
        "equipment must be a typed HVAC or water heater config object",
    ))
}

/// The resolved model as the dict the read-back surfaces return: the site
/// with its per-field sources, the clock, and every boundary's layers.
/// `Dwelling.resolved_model()` returns the same form.
pub(crate) fn resolved_model_to_py(
    py: Python<'_>,
    model: &hares_core::ResolvedModel,
) -> PyResult<Py<PyAny>> {
    use hares_io::hpxml::BoundaryType;

    let site = PyDict::new(py);
    for (value, name, source) in [
        (
            model.site.latitude_deg,
            "latitude_deg",
            model.site.latitude_source,
        ),
        (
            model.site.longitude_deg,
            "longitude_deg",
            model.site.longitude_source,
        ),
        (
            model.site.elevation_m,
            "elevation_m",
            model.site.elevation_source,
        ),
        (
            model.site.utc_offset_h,
            "utc_offset_h",
            model.site.utc_offset_source,
        ),
    ] {
        site.set_item(name, value)?;
        site.set_item(format!("{name}_source"), source.label())?;
    }

    let clock = PyDict::new(py);
    clock.set_item(
        "start",
        model.clock.start.format("%Y-%m-%dT%H:%M:%S").to_string(),
    )?;
    clock.set_item("utc_offset_s", model.clock.utc_offset_s)?;
    clock.set_item("time_res_s", model.clock.time_res_s)?;
    clock.set_item("steps", model.clock.steps)?;

    let kind_label = |kind: &BoundaryType| -> String {
        match kind {
            BoundaryType::Wall => "Wall".to_string(),
            BoundaryType::Roof => "Roof".to_string(),
            BoundaryType::Floor => "Floor".to_string(),
            BoundaryType::Window => "Window".to_string(),
            BoundaryType::Skylight => "Skylight".to_string(),
            BoundaryType::Door => "Door".to_string(),
            BoundaryType::FoundationWall => "FoundationWall".to_string(),
            BoundaryType::RimJoist => "RimJoist".to_string(),
            BoundaryType::Slab => "Slab".to_string(),
            BoundaryType::Other(raw) => raw.clone(),
        }
    };
    let boundaries = PyList::empty(py);
    for boundary in &model.boundaries {
        let entry = PyDict::new(py);
        entry.set_item("id", boundary.id.clone())?;
        entry.set_item("kind", kind_label(&boundary.kind))?;
        entry.set_item("area_m2", boundary.area_m2)?;
        entry.set_item("r_effective_m2k_w", boundary.r_effective_m2k_w)?;
        entry.set_item("films_m2k_w", boundary.films_m2k_w)?;
        entry.set_item("c_j_k", boundary.c_j_k)?;
        let layers = PyList::empty(py);
        for layer in &boundary.layers {
            let item = PyDict::new(py);
            item.set_item("r_m2k_w", layer.r_m2k_w)?;
            item.set_item("c_j_m2k", layer.c_j_m2k)?;
            layers.append(item)?;
        }
        entry.set_item("layers", layers)?;
        boundaries.append(entry)?;
    }

    let out = PyDict::new(py);
    out.set_item("site", site)?;
    out.set_item("clock", clock)?;
    out.set_item("boundaries", boundaries)?;
    Ok(out.into_any().unbind())
}
