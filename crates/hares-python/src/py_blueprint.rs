//! Python bindings for pre-build dwelling blueprint (equipment swapping).

use hares_core::dwelling::DwellingBlueprint;
use hares_io::EquipmentSpec;
use hares_types::EndUse;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyType};

use crate::py_dwelling::{PyDwelling, construct_detached, to_py_err};
use crate::py_enums::PyEndUse;
use crate::py_hvac::{
    PyASHPCooler, PyASHPHeater, PyAirConditioner, PyElectricBaseboard, PyElectricBoiler,
    PyElectricFurnace, PyGasBoiler, PyGasFurnace, PyIdealHVAC, ac_spec_from_py,
    ashp_cooler_spec_from_py, ashp_heater_spec_from_py, baseboard_spec_from_py,
    electric_boiler_spec_from_py, electric_furnace_spec_from_py, gas_boiler_spec_from_py,
    gas_furnace_spec_from_py, ideal_hvac_spec_from_py,
};
use crate::py_water_heater::{
    PyElectricResistanceWH, PyGasWaterHeater, PyHeatPumpWH, PyIndirectTank, PyTanklessWaterHeater,
    elec_res_wh_spec_from_py, gas_wh_spec_from_py, hpwh_spec_from_py, indirect_tank_spec_from_py,
    tankless_wh_spec_from_py,
};

#[pyclass(name = "DwellingBlueprint")]
#[doc = "A two-phase dwelling blueprint: first inspect and swap equipment, then build."]
pub struct PyDwellingBlueprint {
    inner: Option<DwellingBlueprint>,
    config: hares_core::DwellingConfig,
}

#[pymethods]
impl PyDwellingBlueprint {
    /// Create a DwellingBlueprint from HPXML, schedule, and weather file paths.
    #[classmethod]
    #[pyo3(signature = (hpxml, schedule, weather, config, *, bldg_id = 0, initialization_duration = None, defaults_path = None, overrides = None, **unexpected))]
    // PyO3 #[classmethod] constructor must match the Python API parameter list.
    #[allow(clippy::too_many_arguments)]
    pub fn from_hpxml(
        _cls: &Bound<'_, PyType>,
        hpxml: String,
        schedule: Option<String>,
        weather: String,
        config: crate::py_config::PySimulationConfig,
        bldg_id: i64,
        initialization_duration: Option<i64>,
        defaults_path: Option<String>,
        overrides: Option<&Bound<'_, PyDict>>,
        unexpected: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Self> {
        // The dwelling-level keywords are the config's own; every simulation
        // setting rides on the one SimulationConfig object.
        let py_config =
            crate::py_config::PyDwellingConfig::from_args(crate::py_config::DwellingConfigArgs {
                hpxml,
                schedule,
                weather,
                config,
                bldg_id,
                initialization_duration,
                defaults_path,
                overrides,
                unexpected,
            })?;
        let config = py_config.to_dwelling_config()?;
        let inner =
            construct_detached(_cls.py(), || DwellingBlueprint::from_config(config.clone()))
                .map_err(to_py_err)?;
        Ok(Self {
            inner: Some(inner),
            config,
        })
    }

    /// Return the list of equipment names currently in the blueprint.
    pub fn equipment_names(&self) -> PyResult<Vec<String>> {
        let bp = self
            .inner
            .as_ref()
            .ok_or_else(|| PyValueError::new_err("DwellingBlueprint has already been built"))?;
        Ok(bp.equipment_names().into_iter().map(String::from).collect())
    }

    /// Remove equipment by its instance name.
    pub fn remove_equipment(&mut self, name: &str) -> PyResult<()> {
        let bp = self
            .inner
            .as_mut()
            .ok_or_else(|| PyValueError::new_err("DwellingBlueprint has already been built"))?;
        bp.remove_equipment(name).map_err(to_py_err)
    }

    /// Remove all equipment serving the given end-use(s).
    /// Accepts a single EndUse or a list[EndUse].
    pub fn remove_equipment_by_end_use(&mut self, end_uses: &Bound<'_, PyAny>) -> PyResult<usize> {
        let bp = self
            .inner
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
            .inner
            .as_mut()
            .ok_or_else(|| PyValueError::new_err("DwellingBlueprint has already been built"))?;
        let spec = py_any_to_equipment_spec(obj)?;
        bp.add_equipment_spec(spec).map_err(to_py_err)?;
        Ok(())
    }

    /// Finalise the blueprint and return a PyDwelling ready for simulation.
    /// Consumes the blueprint; calling build() a second time will error.
    pub fn build(&mut self, py: Python<'_>) -> PyResult<PyDwelling> {
        let bp = self
            .inner
            .take()
            .ok_or_else(|| PyValueError::new_err("DwellingBlueprint has already been built"))?;
        let dwelling = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            construct_detached(py, || bp.build())
        }))
        .map_err(|_| PyValueError::new_err("DwellingBlueprint::build() panicked"))?
        .map_err(to_py_err)?;
        Ok(PyDwelling::from_blueprint_build(
            dwelling,
            self.config.clone(),
        ))
    }
}

fn py_any_to_equipment_spec(obj: &Bound<'_, PyAny>) -> PyResult<EquipmentSpec> {
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
