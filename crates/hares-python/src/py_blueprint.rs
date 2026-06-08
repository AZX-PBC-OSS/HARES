//! Python bindings for pre-build dwelling blueprint (equipment swapping).

use hares_core::dwelling::DwellingBlueprint;
use hares_io::EquipmentSpec;
use hares_types::EndUse;
use pyo3::prelude::*;
use pyo3::exceptions::PyValueError;
use pyo3::types::{PyDict, PyType};

use crate::py_dwelling::{PyDwelling, to_py_err};
use crate::py_enums::PyEndUse;
use crate::py_hvac::{
    PyGasFurnace, PyAirConditioner, PyASHPHeater, PyASHPCooler,
    PyIdealHVAC, PyElectricBaseboard,
    gas_furnace_spec_from_py, ac_spec_from_py,
    ashp_heater_spec_from_py, ashp_cooler_spec_from_py,
    ideal_hvac_spec_from_py, baseboard_spec_from_py,
};
use crate::py_water_heater::{
    PyGasWaterHeater, PyElectricResistanceWH, PyHeatPumpWH,
    gas_wh_spec_from_py, elec_res_wh_spec_from_py, hpwh_spec_from_py,
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
    #[pyo3(signature = (hpxml, schedule, weather, **kwargs))]
    pub fn from_hpxml(
        _cls: &Bound<'_, PyType>,
        hpxml: String,
        schedule: String,
        weather: String,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Self> {
        let config =
            crate::py_dwelling::build_config(hpxml, schedule, weather, kwargs)?;
        let inner = DwellingBlueprint::from_config(config.clone())
            .map_err(to_py_err)?;
        Ok(Self {
            inner: Some(inner),
            config,
        })
    }

    /// Return the list of equipment names currently in the blueprint.
    pub fn equipment_names(&self) -> PyResult<Vec<String>> {
        let bp = self.inner.as_ref().ok_or_else(|| {
            PyValueError::new_err("DwellingBlueprint has already been built")
        })?;
        Ok(bp
            .equipment_names()
            .into_iter()
            .map(String::from)
            .collect())
    }

    /// Remove equipment by its instance name.
    pub fn remove_equipment(&mut self, name: &str) -> PyResult<()> {
        let bp = self.inner.as_mut().ok_or_else(|| {
            PyValueError::new_err("DwellingBlueprint has already been built")
        })?;
        bp.remove_equipment(name).map_err(to_py_err)
    }

    /// Remove all equipment serving the given end-use(s).
    /// Accepts a single EndUse or a list[EndUse].
    pub fn remove_equipment_by_end_use(&mut self, end_uses: &Bound<'_, PyAny>) -> PyResult<usize> {
        let bp = self.inner.as_mut().ok_or_else(|| {
            PyValueError::new_err("DwellingBlueprint has already been built")
        })?;
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
        let bp = self.inner.as_mut().ok_or_else(|| {
            PyValueError::new_err("DwellingBlueprint has already been built")
        })?;
        let spec = py_any_to_equipment_spec(obj)?;
        bp.add_equipment_spec(spec);
        Ok(())
    }

    /// Finalise the blueprint and return a PyDwelling ready for simulation.
    /// Consumes the blueprint; calling build() a second time will error.
    pub fn build(&mut self) -> PyResult<PyDwelling> {
        let bp = self.inner.take().ok_or_else(|| {
            PyValueError::new_err("DwellingBlueprint has already been built")
        })?;
        let dwelling = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            bp.build()
        }))
        .map_err(|_| PyValueError::new_err("DwellingBlueprint::build() panicked"))?
        .map_err(to_py_err)?;
        Ok(PyDwelling::from_blueprint_build(dwelling, self.config.clone()))
    }
}

fn py_any_to_equipment_spec(obj: &Bound<'_, PyAny>) -> PyResult<EquipmentSpec> {
    if let Ok(f) = obj.extract::<PyRef<'_, PyGasFurnace>>() {
        return Ok(gas_furnace_spec_from_py(&f));
    }
    if let Ok(ac) = obj.extract::<PyRef<'_, PyAirConditioner>>() {
        return Ok(ac_spec_from_py(&ac));
    }
    if let Ok(hp) = obj.extract::<PyRef<'_, PyASHPHeater>>() {
        return Ok(ashp_heater_spec_from_py(&hp));
    }
    if let Ok(hp) = obj.extract::<PyRef<'_, PyASHPCooler>>() {
        return Ok(ashp_cooler_spec_from_py(&hp));
    }
    if let Ok(hvac) = obj.extract::<PyRef<'_, PyIdealHVAC>>() {
        return Ok(ideal_hvac_spec_from_py(&hvac));
    }
    if let Ok(bb) = obj.extract::<PyRef<'_, PyElectricBaseboard>>() {
        return Ok(baseboard_spec_from_py(&bb));
    }
    if let Ok(wh) = obj.extract::<PyRef<'_, PyGasWaterHeater>>() {
        return Ok(gas_wh_spec_from_py(&wh));
    }
    if let Ok(wh) = obj.extract::<PyRef<'_, PyElectricResistanceWH>>() {
        return Ok(elec_res_wh_spec_from_py(&wh));
    }
    if let Ok(wh) = obj.extract::<PyRef<'_, PyHeatPumpWH>>() {
        return Ok(hpwh_spec_from_py(&wh));
    }
    Err(PyValueError::new_err(
        "equipment must be a typed HVAC or water heater config object"
    ))
}
