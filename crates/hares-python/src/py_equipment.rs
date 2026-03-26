//! Python bindings for equipment configuration.

use hares_equipment::ndinterp::RegularGridInterpolator;
use hares_equipment::{OcvTable, UNegTable};
use hares_types::{
    EquipmentDescriptor as RustEquipmentDescriptor, TelemetryField as RustTelemetryField,
};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

/// Extract a `RegularGridInterpolator` from a Python dict with numpy arrays or an NPZ path.
///
/// Accepted forms:
/// - `str`: path to an NPZ file with keys `soc_grid`, `temp_grid`, `crate_grid`, `soh_grid`, `lut`
/// - `dict`: with the same keys as numpy arrays
///
/// The `lut` values are flattened to row-major f32.  Axis arrays are float64, 1-D, strictly ascending.
pub fn extract_charging_lut(
    py: Python<'_>,
    obj: &Bound<'_, PyAny>,
) -> PyResult<RegularGridInterpolator> {
    // If it's a string, load NPZ
    if let Ok(path) = obj.extract::<String>() {
        return load_npz_lut(py, &path);
    }

    // Otherwise expect a dict with numpy arrays
    let dict: &Bound<'_, PyDict> = obj.cast()?;

    let soc = extract_f64_axis(py, dict, "soc_grid")?;
    let temp = extract_f64_axis(py, dict, "temp_grid")?;
    let crate_ax = extract_f64_axis(py, dict, "crate_grid")?;
    let soh = extract_f64_axis(py, dict, "soh_grid")?;

    let lut_arr = dict
        .get_item("lut")?
        .ok_or_else(|| PyValueError::new_err("charging_curve_lut dict missing 'lut' key"))?;
    let values = extract_flat_f32(py, &lut_arr)?;

    RegularGridInterpolator::new(vec![soc, temp, crate_ax, soh], values)
        .map_err(|e| PyValueError::new_err(e.to_string()))
}

fn extract_f64_axis(_py: Python<'_>, dict: &Bound<'_, PyDict>, key: &str) -> PyResult<Vec<f64>> {
    let arr = dict.get_item(key)?.ok_or_else(|| {
        PyValueError::new_err(format!("charging_curve_lut dict missing '{key}' key"))
    })?;
    // Convert to list of f64 via Python — works with any array-like
    let list: Vec<f64> = arr.call_method0("tolist")?.extract()?;
    Ok(list)
}

fn extract_flat_f32(_py: Python<'_>, arr: &Bound<'_, PyAny>) -> PyResult<Vec<f32>> {
    // Flatten and convert to f32 list via Python
    let flat = arr
        .call_method1("astype", ("float32",))?
        .call_method0("ravel")?;
    let list: Vec<f32> = flat.call_method0("tolist")?.extract()?;
    Ok(list)
}

fn load_npz_lut(py: Python<'_>, path: &str) -> PyResult<RegularGridInterpolator> {
    let np_mod = py.import("numpy")?;
    let data = np_mod.call_method1("load", (path,))?;

    let soc: Vec<f64> = data
        .get_item("soc_grid")?
        .call_method0("tolist")?
        .extract()?;
    let temp: Vec<f64> = data
        .get_item("temp_grid")?
        .call_method0("tolist")?
        .extract()?;
    let crate_ax: Vec<f64> = data
        .get_item("crate_grid")?
        .call_method0("tolist")?
        .extract()?;
    let soh: Vec<f64> = data
        .get_item("soh_grid")?
        .call_method0("tolist")?
        .extract()?;
    let values = extract_flat_f32(py, &data.get_item("lut")?)?;

    RegularGridInterpolator::new(vec![soc, temp, crate_ax, soh], values)
        .map_err(|e| PyValueError::new_err(e.to_string()))
}

#[pyclass(name = "Battery")]
#[derive(Debug)]
pub struct PyBattery {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub capacity_kwh: f64,
    #[pyo3(get)]
    pub max_charge_kw: Option<f64>,
    #[pyo3(get)]
    pub max_discharge_kw: Option<f64>,
    pub charging_curve_lut: Option<RegularGridInterpolator>,
    pub ocv_table: Option<OcvTable>,
    pub u_neg_table: Option<UNegTable>,
}

#[pymethods]
impl PyBattery {
    #[new]
    #[pyo3(signature = (
        name,
        capacity_kwh,
        max_charge_kw=None,
        max_discharge_kw=None,
        charging_curve_lut=None,
        ocv_table=None,
        uneg_table=None,
    ))]
    fn new(
        py: Python<'_>,
        name: String,
        capacity_kwh: f64,
        max_charge_kw: Option<f64>,
        max_discharge_kw: Option<f64>,
        charging_curve_lut: Option<&Bound<'_, PyAny>>,
        ocv_table: Option<&Bound<'_, PyAny>>,
        uneg_table: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        let lut = match charging_curve_lut {
            Some(obj) => Some(extract_charging_lut(py, obj)?),
            None => None,
        };
        let ocv = match ocv_table {
            Some(obj) => Some(extract_ocv_table(py, obj)?),
            None => None,
        };
        let u_neg = match uneg_table {
            Some(obj) => Some(extract_u_neg_table(py, obj)?),
            None => None,
        };
        Ok(Self {
            name,
            capacity_kwh,
            max_charge_kw,
            max_discharge_kw,
            charging_curve_lut: lut,
            ocv_table: ocv,
            u_neg_table: u_neg,
        })
    }

    fn __repr__(&self) -> String {
        format!(
            "Battery(name={:?}, capacity_kwh={})",
            self.name, self.capacity_kwh
        )
    }
}

#[pyclass(name = "PvSoilingConfig", from_py_object)]
#[derive(Debug, Clone)]
pub struct PyPvSoilingConfig {
    #[pyo3(get)]
    pub cleaning_threshold_mm: f64,
    #[pyo3(get)]
    pub loss_rate_per_day: f64,
    #[pyo3(get)]
    pub grace_period_days: f64,
    #[pyo3(get)]
    pub max_loss: f64,
    #[pyo3(get)]
    pub initial_loss: f64,
    #[pyo3(get)]
    pub rain_accum_hours: f64,
}

#[pymethods]
impl PyPvSoilingConfig {
    #[new]
    #[pyo3(signature = (
        cleaning_threshold_mm=6.0,
        loss_rate_per_day=0.0015,
        grace_period_days=14.0,
        max_loss=0.30,
        initial_loss=0.0,
        rain_accum_hours=24.0,
    ))]
    fn new(
        cleaning_threshold_mm: f64,
        loss_rate_per_day: f64,
        grace_period_days: f64,
        max_loss: f64,
        initial_loss: f64,
        rain_accum_hours: f64,
    ) -> Self {
        Self {
            cleaning_threshold_mm,
            loss_rate_per_day,
            grace_period_days,
            max_loss,
            initial_loss,
            rain_accum_hours,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "PvSoilingConfig(cleaning_threshold_mm={}, loss_rate_per_day={})",
            self.cleaning_threshold_mm, self.loss_rate_per_day
        )
    }
}

#[pyclass(name = "PV")]
#[derive(Debug)]
pub struct PyPv {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub capacity_kw: f64,
    #[pyo3(get)]
    pub tilt: f64,
    #[pyo3(get)]
    pub azimuth: f64,
    #[pyo3(get)]
    pub soiling: Option<PyPvSoilingConfig>,
}

#[pymethods]
impl PyPv {
    #[new]
    #[pyo3(signature = (name, capacity_kw, tilt, azimuth, soiling=None))]
    fn new(
        name: String,
        capacity_kw: f64,
        tilt: f64,
        azimuth: f64,
        soiling: Option<PyPvSoilingConfig>,
    ) -> Self {
        Self {
            name,
            capacity_kw,
            tilt,
            azimuth,
            soiling,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "PV(name={:?}, capacity_kw={}, tilt={}, azimuth={})",
            self.name, self.capacity_kw, self.tilt, self.azimuth
        )
    }
}

#[pyclass(name = "EV")]
#[derive(Debug)]
pub struct PyEv {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub capacity_kwh: Option<f64>,
    #[pyo3(get)]
    pub max_charging_kw: Option<f64>,
    /// Optional 4D charging curve LUT (soc × temp × c_rate × soh → power_fraction).
    pub charging_curve_lut: Option<RegularGridInterpolator>,
}

#[pymethods]
impl PyEv {
    #[new]
    #[pyo3(signature = (name, capacity_kwh=None, max_charging_kw=None, charging_curve_lut=None))]
    fn new(
        py: Python<'_>,
        name: String,
        capacity_kwh: Option<f64>,
        max_charging_kw: Option<f64>,
        charging_curve_lut: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        let lut = match charging_curve_lut {
            Some(obj) => Some(extract_charging_lut(py, obj)?),
            None => None,
        };
        Ok(Self {
            name,
            capacity_kwh,
            max_charging_kw,
            charging_curve_lut: lut,
        })
    }

    fn __repr__(&self) -> String {
        let lut_info = if self.charging_curve_lut.is_some() {
            ", lut=loaded"
        } else {
            ""
        };
        format!(
            "EV(name={:?}, capacity_kwh={:?}, max_charging_kw={:?}{lut_info})",
            self.name, self.capacity_kwh, self.max_charging_kw
        )
    }
}

#[pyclass(name = "TelemetryField", from_py_object)]
#[derive(Clone)]
pub struct PyTelemetryField {
    inner: RustTelemetryField,
}

impl PyTelemetryField {
    pub(crate) fn new(inner: RustTelemetryField) -> Self {
        Self { inner }
    }
}

#[pymethods]
impl PyTelemetryField {
    #[getter]
    fn name(&self) -> &str {
        &self.inner.name
    }

    #[getter]
    fn unit(&self) -> &str {
        &self.inner.unit
    }

    #[getter]
    fn description(&self) -> &str {
        &self.inner.description
    }

    fn __repr__(&self) -> String {
        format!("TelemetryField({} ({}))", self.inner.name, self.inner.unit)
    }
}

impl From<RustTelemetryField> for PyTelemetryField {
    fn from(inner: RustTelemetryField) -> Self {
        Self::new(inner)
    }
}

#[pyclass(name = "EquipmentDescriptor", frozen, from_py_object)]
#[derive(Clone)]
pub struct PyEquipmentDescriptor {
    inner: RustEquipmentDescriptor,
}

impl PyEquipmentDescriptor {
    pub(crate) fn new(inner: RustEquipmentDescriptor) -> Self {
        Self { inner }
    }
}

#[pymethods]
impl PyEquipmentDescriptor {
    #[getter]
    fn id(&self) -> u32 {
        self.inner.id.0
    }

    #[getter]
    fn name(&self) -> &str {
        &self.inner.name
    }

    #[getter]
    fn end_use(&self) -> crate::py_enums::PyEndUse {
        crate::py_enums::PyEndUse::new(self.inner.end_use.clone())
    }

    #[getter]
    fn equipment_type(&self) -> String {
        self.inner.equipment_type.to_string()
    }

    #[getter]
    fn zone(&self) -> Option<u16> {
        self.inner.zone.map(|z| z.0)
    }

    #[getter]
    fn fuel_type(&self) -> crate::py_enums::PyFuelType {
        self.inner.fuel.into()
    }

    #[getter]
    fn stage(&self) -> crate::py_enums::PyExecutionStage {
        self.inner.stage.into()
    }

    #[getter]
    fn control_capabilities(&self) -> crate::py_enums::PyControlCapabilities {
        crate::py_enums::PyControlCapabilities::new(self.inner.control_capabilities)
    }

    #[getter]
    fn telemetry_fields(&self) -> Vec<PyTelemetryField> {
        self.inner
            .telemetry_fields
            .iter()
            .map(|f| PyTelemetryField::new(f.clone()))
            .collect()
    }

    fn __repr__(&self) -> String {
        format!(
            "EquipmentDescriptor(name={:?}, equipment_type={:?}, end_use={})",
            self.inner.name,
            self.inner.equipment_type,
            self.inner.end_use.as_str()
        )
    }
}

impl From<RustEquipmentDescriptor> for PyEquipmentDescriptor {
    fn from(inner: RustEquipmentDescriptor) -> Self {
        Self::new(inner)
    }
}

// ---------------------------------------------------------------------------
// LUT table extraction helpers
// ---------------------------------------------------------------------------

/// Extract an `OcvTable` from Python data.
///
/// Accepted formats:
/// - list of (soc, voltage) tuples
/// - dict with `soc` and `voltage` keys (lists or numpy arrays)
/// - polars DataFrame with `soc` and `voltage` columns
pub fn extract_ocv_table(py: Python<'_>, obj: &Bound<'_, PyAny>) -> PyResult<OcvTable> {
    if let Ok(list) = obj.cast::<PyList>() {
        return extract_ocv_from_tuples(list);
    }
    if let Ok(dict) = obj.cast::<PyDict>() {
        return extract_ocv_from_dict(dict);
    }
    // Try polars DataFrame
    if is_polars_dataframe(obj)? {
        return extract_ocv_from_dataframe(py, obj);
    }
    Err(PyValueError::new_err(
        "ocv_table must be a list of (soc, voltage) tuples, \
         dict with 'soc'/'voltage' keys, or polars DataFrame",
    ))
}

/// Extract a `UNegTable` from Python data.
///
/// Same formats as `extract_ocv_table` but with `soc` and `potential` keys/columns.
pub fn extract_u_neg_table(py: Python<'_>, obj: &Bound<'_, PyAny>) -> PyResult<UNegTable> {
    if let Ok(list) = obj.cast::<PyList>() {
        return extract_u_neg_from_tuples(list);
    }
    if let Ok(dict) = obj.cast::<PyDict>() {
        return extract_u_neg_from_dict(dict);
    }
    if is_polars_dataframe(obj)? {
        return extract_u_neg_from_dataframe(py, obj);
    }
    Err(PyValueError::new_err(
        "uneg_table must be a list of (soc, potential) tuples, \
         dict with 'soc'/'potential' keys, or polars DataFrame",
    ))
}

fn is_polars_dataframe(obj: &Bound<'_, PyAny>) -> PyResult<bool> {
    if let Ok(cls) = obj.getattr("__class__") {
        if let Ok(name) = cls.getattr("__name__") {
            if let Ok(s) = name.extract::<String>() {
                return Ok(s == "DataFrame");
            }
        }
    }
    Ok(false)
}

fn extract_f64_column(obj: &Bound<'_, PyAny>, col: &str) -> PyResult<Vec<f64>> {
    let series = obj.call_method1("get_column", (col,))?;
    let py_list = series.call_method0("to_list")?;
    py_list.extract::<Vec<f64>>()
}

fn extract_ocv_from_tuples(list: &Bound<'_, PyList>) -> PyResult<OcvTable> {
    let mut soc_points = Vec::with_capacity(list.len());
    let mut voltage_v = Vec::with_capacity(list.len());
    for item in list.iter() {
        let (s, v): (f64, f64) = item.extract()?;
        soc_points.push(s);
        voltage_v.push(v);
    }
    OcvTable::new(soc_points, voltage_v).map_err(|e| PyValueError::new_err(e.to_string()))
}

fn extract_ocv_from_dict(dict: &Bound<'_, PyDict>) -> PyResult<OcvTable> {
    let soc = dict
        .get_item("soc")?
        .ok_or_else(|| PyValueError::new_err("ocv dict missing 'soc' key"))?
        .extract::<Vec<f64>>()?;
    let voltage = dict
        .get_item("voltage")?
        .ok_or_else(|| PyValueError::new_err("ocv dict missing 'voltage' key"))?
        .extract::<Vec<f64>>()?;
    OcvTable::new(soc, voltage).map_err(|e| PyValueError::new_err(e.to_string()))
}

fn extract_ocv_from_dataframe(py: Python<'_>, df: &Bound<'_, PyAny>) -> PyResult<OcvTable> {
    let _ = py;
    let soc = extract_f64_column(df, "soc")?;
    let voltage = extract_f64_column(df, "voltage")?;
    OcvTable::new(soc, voltage).map_err(|e| PyValueError::new_err(e.to_string()))
}

fn extract_u_neg_from_tuples(list: &Bound<'_, PyList>) -> PyResult<UNegTable> {
    let mut soc_points = Vec::with_capacity(list.len());
    let mut potential_v = Vec::with_capacity(list.len());
    for item in list.iter() {
        let (s, p): (f64, f64) = item.extract()?;
        soc_points.push(s);
        potential_v.push(p);
    }
    UNegTable::new(soc_points, potential_v).map_err(|e| PyValueError::new_err(e.to_string()))
}

fn extract_u_neg_from_dict(dict: &Bound<'_, PyDict>) -> PyResult<UNegTable> {
    let soc = dict
        .get_item("soc")?
        .ok_or_else(|| PyValueError::new_err("uneg dict missing 'soc' key"))?
        .extract::<Vec<f64>>()?;
    let potential = dict
        .get_item("potential")?
        .ok_or_else(|| PyValueError::new_err("uneg dict missing 'potential' key"))?
        .extract::<Vec<f64>>()?;
    UNegTable::new(soc, potential).map_err(|e| PyValueError::new_err(e.to_string()))
}

fn extract_u_neg_from_dataframe(py: Python<'_>, df: &Bound<'_, PyAny>) -> PyResult<UNegTable> {
    let _ = py;
    let soc = extract_f64_column(df, "soc")?;
    let potential = extract_f64_column(df, "potential")?;
    UNegTable::new(soc, potential).map_err(|e| PyValueError::new_err(e.to_string()))
}
