//! Python bindings for equipment configuration.

use hares_equipment::battery::catalog::{BatteryProductId, BatterySpec};
use hares_equipment::ev::catalog::{EvArchetypeId, VehicleId, VehicleSpec};
use hares_equipment::ndinterp::ExtrapolationStrategy;
use hares_equipment::ndinterp::RegularGridInterpolator;
use hares_equipment::{OcvTable, UNegTable};
use hares_types::{
    CoreOutput as RustCoreOutput, ElectricPower, EquipmentDescriptor as RustEquipmentDescriptor,
    FuelType, OperatingMode, Telemetry as RustTelemetry, TelemetryField as RustTelemetryField,
};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

use crate::py_enums::{
    PyBatteryChemistry, PyBatteryProductId, PyEvArchetypeId, PyEvConnectionState, PyVehicleId,
};

/// Extract a `RegularGridInterpolator` from a Python dict with numpy arrays or an NPZ path.
///
/// Accepted forms:
/// - `str`: path to an NPZ file with keys `soc_grid`, `temp_grid`, `crate_grid`, `soh_grid`, `lut`
/// - `dict`: with the same keys as numpy arrays
///
/// The `lut` values are flattened to row-major f32.  Axis arrays are float64, 1-D, strictly ascending.
/// The interpolator is constructed with `ExtrapolationStrategy::Clamp` for safe runtime behaviour.
/// Use `ExtrapolationStrategy::NaN` during offline LUT validation to detect coverage gaps.
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

    let n_soc = soc.len();
    let n_temp = temp.len();
    let n_crate = crate_ax.len();
    let n_soh = soh.len();

    let mut interp = RegularGridInterpolator::new(
        vec![soc, temp, crate_ax, soh],
        values,
        ExtrapolationStrategy::Clamp,
    )
    .map_err(|e| PyValueError::new_err(e.to_string()))?;

    if let Ok(Some(mask_obj)) = dict.get_item("fallback_mask") {
        let flat = broadcast_fallback_mask(py, &mask_obj, n_soc, n_temp, n_crate, n_soh)?;
        if !flat.is_empty() {
            interp
                .set_fallback_mask(flat)
                .map_err(|e| PyValueError::new_err(e.to_string()))?;
        }
    }

    if interp.ndim() != 4 {
        return Err(PyValueError::new_err(format!(
            "charging_curve_lut must have exactly 4 dimensions (SOC × temp × c_rate × SOH), got {}",
            interp.ndim()
        )));
    }
    Ok(interp)
}

fn broadcast_fallback_mask(
    py: Python<'_>,
    mask_obj: &Bound<'_, PyAny>,
    n_soc: usize,
    n_temp: usize,
    n_crate: usize,
    n_soh: usize,
) -> PyResult<Vec<u8>> {
    let ndim: usize = mask_obj.getattr("ndim")?.extract()?;
    if ndim == 3 {
        // Outer-grid mask from Python: shape (n_temp, n_crate, n_soh).
        // Broadcast to full-grid shape (n_soc, n_temp, n_crate, n_soh).
        let np = py.import("numpy")?;
        let reshaped = mask_obj.call_method1(
            "reshape",
            ((1i64, n_temp as i64, n_crate as i64, n_soh as i64),),
        )?;
        let target = (n_soc as i64, n_temp as i64, n_crate as i64, n_soh as i64);
        let full = np.call_method1("broadcast_to", (reshaped, target))?;
        Ok(full
            .call_method0("copy")?
            .call_method1("astype", ("uint8",))?
            .call_method0("ravel")?
            .call_method0("tolist")?
            .extract()?)
    } else {
        // Already full-grid mask (4D), use as-is.
        Ok(mask_obj
            .call_method1("astype", ("uint8",))?
            .call_method0("ravel")?
            .call_method0("tolist")?
            .extract()?)
    }
}

fn extract_f64_axis(_py: Python<'_>, dict: &Bound<'_, PyDict>, key: &str) -> PyResult<Vec<f64>> {
    let arr = dict.get_item(key)?.ok_or_else(|| {
        PyValueError::new_err(format!("charging_curve_lut dict missing '{key}' key"))
    })?;
    // Convert to list of f64 via Python -- works with any array-like
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

    let n_soc = soc.len();
    let n_temp = temp.len();
    let n_crate = crate_ax.len();
    let n_soh = soh.len();

    let mut interp = RegularGridInterpolator::new(
        vec![soc, temp, crate_ax, soh],
        values,
        ExtrapolationStrategy::Clamp,
    )
    .map_err(|e| PyValueError::new_err(e.to_string()))?;

    if let Ok(mask_obj) = data.get_item("fallback_mask") {
        let flat = broadcast_fallback_mask(py, &mask_obj, n_soc, n_temp, n_crate, n_soh)?;
        if !flat.is_empty() {
            interp
                .set_fallback_mask(flat)
                .map_err(|e| PyValueError::new_err(e.to_string()))?;
        }
    }

    if interp.ndim() != 4 {
        return Err(PyValueError::new_err(format!(
            "charging_curve_lut must have exactly 4 dimensions (SOC × temp × c_rate × SOH), got {}",
            interp.ndim()
        )));
    }
    Ok(interp)
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
    #[pyo3(get)]
    pub chemistry: Option<PyBatteryChemistry>,
    #[pyo3(get)]
    pub initial_soc: Option<f64>,
    #[pyo3(get)]
    pub min_soc: Option<f64>,
    #[pyo3(get)]
    pub max_soc: Option<f64>,
    #[pyo3(get)]
    pub inverter_efficiency: Option<f64>,
    #[pyo3(get)]
    pub charge_efficiency: Option<f64>,
    #[pyo3(get)]
    pub discharge_efficiency: Option<f64>,
    /// Displacement power-factor magnitude for the battery inverter.
    /// `None` defaults to 1.0 (unity, no reactive power). Values in (0, 1]
    /// produce lagging reactive power proportional to active power.
    #[pyo3(get)]
    pub power_factor: Option<f64>,
    /// Inverter apparent-power rating [kVA]. When `None`, defaults to
    /// `max(max_charge_kw, max_discharge_kw)` at init time. Caps
    /// `|Q| ≤ sqrt(S² − P²)` (active-power priority).
    #[pyo3(get)]
    pub inverter_capacity_kva: Option<f64>,
    #[pyo3(get)]
    pub self_discharge_pct_per_day: Option<f64>,
    #[pyo3(get)]
    pub standby_power_w: Option<f64>,
    #[pyo3(get)]
    pub import_limit_w: Option<f64>,
    #[pyo3(get)]
    pub export_limit_w: Option<f64>,
    #[pyo3(get)]
    pub n_series: Option<u32>,
    #[pyo3(get)]
    pub n_parallel: Option<u32>,
    #[pyo3(get)]
    pub cell_resistance_ohm: Option<f64>,
    #[pyo3(get)]
    pub heater_power_w: Option<f64>,
    #[pyo3(get)]
    pub heater_threshold_c: Option<f64>,
    #[pyo3(get)]
    pub min_charge_temp_c: Option<f64>,
    #[pyo3(get)]
    pub full_power_temp_c: Option<f64>,
    #[pyo3(get)]
    pub cell_thermal_mass_j_per_k: Option<f64>,
    #[pyo3(get)]
    pub cell_ua_w_per_k: Option<f64>,
    pub charging_curve_lut: Option<RegularGridInterpolator>,
    pub ocv_table: Option<OcvTable>,
    pub u_neg_table: Option<UNegTable>,
}

impl PyBattery {
    fn from_spec(spec: &BatterySpec) -> Self {
        let eta = spec.round_trip_efficiency.sqrt();
        Self {
            name: spec.label.to_string(),
            capacity_kwh: spec.capacity_kwh,
            max_charge_kw: Some(spec.max_charge_kw),
            max_discharge_kw: Some(spec.max_discharge_kw),
            chemistry: Some(PyBatteryChemistry::from(spec.chemistry)),
            initial_soc: None,
            min_soc: Some(spec.min_soc),
            max_soc: Some(spec.max_soc),
            inverter_efficiency: None,
            charge_efficiency: Some(eta),
            discharge_efficiency: Some(eta),
            power_factor: None,
            inverter_capacity_kva: None,
            self_discharge_pct_per_day: Some(spec.self_discharge_pct_per_day),
            standby_power_w: Some(spec.standby_power_w),
            import_limit_w: None,
            export_limit_w: None,
            n_series: None,
            n_parallel: None,
            cell_resistance_ohm: None,
            heater_power_w: Some(spec.heater_power_w),
            heater_threshold_c: Some(spec.heater_threshold_c),
            min_charge_temp_c: Some(spec.min_charge_temp_c),
            full_power_temp_c: Some(spec.full_power_temp_c),
            cell_thermal_mass_j_per_k: Some(spec.thermal_mass_j_per_k),
            cell_ua_w_per_k: Some(spec.ua_w_per_k),
            charging_curve_lut: None,
            ocv_table: None,
            u_neg_table: None,
        }
    }
}

#[pymethods]
impl PyBattery {
    #[new]
    #[pyo3(signature = (
        name,
        capacity_kwh,
        max_charge_kw=None,
        max_discharge_kw=None,
        chemistry=None,
        initial_soc=None,
        min_soc=None,
        max_soc=None,
        inverter_efficiency=None,
        charge_efficiency=None,
        discharge_efficiency=None,
        power_factor=None,
        inverter_capacity_kva=None,
        self_discharge_pct_per_day=None,
        standby_power_w=None,
        import_limit_w=None,
        export_limit_w=None,
        n_series=None,
        n_parallel=None,
        cell_resistance_ohm=None,
        heater_power_w=None,
        heater_threshold_c=None,
        min_charge_temp_c=None,
        full_power_temp_c=None,
        cell_thermal_mass_j_per_k=None,
        cell_ua_w_per_k=None,
        charging_curve_lut=None,
        ocv_table=None,
        uneg_table=None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        py: Python<'_>,
        name: String,
        capacity_kwh: f64,
        max_charge_kw: Option<f64>,
        max_discharge_kw: Option<f64>,
        chemistry: Option<PyBatteryChemistry>,
        initial_soc: Option<f64>,
        min_soc: Option<f64>,
        max_soc: Option<f64>,
        inverter_efficiency: Option<f64>,
        charge_efficiency: Option<f64>,
        discharge_efficiency: Option<f64>,
        power_factor: Option<f64>,
        inverter_capacity_kva: Option<f64>,
        self_discharge_pct_per_day: Option<f64>,
        standby_power_w: Option<f64>,
        import_limit_w: Option<f64>,
        export_limit_w: Option<f64>,
        n_series: Option<u32>,
        n_parallel: Option<u32>,
        cell_resistance_ohm: Option<f64>,
        heater_power_w: Option<f64>,
        heater_threshold_c: Option<f64>,
        min_charge_temp_c: Option<f64>,
        full_power_temp_c: Option<f64>,
        cell_thermal_mass_j_per_k: Option<f64>,
        cell_ua_w_per_k: Option<f64>,
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
            chemistry,
            initial_soc,
            min_soc,
            max_soc,
            inverter_efficiency,
            charge_efficiency,
            discharge_efficiency,
            power_factor,
            inverter_capacity_kva,
            self_discharge_pct_per_day,
            standby_power_w,
            import_limit_w,
            export_limit_w,
            n_series,
            n_parallel,
            cell_resistance_ohm,
            heater_power_w,
            heater_threshold_c,
            min_charge_temp_c,
            full_power_temp_c,
            cell_thermal_mass_j_per_k,
            cell_ua_w_per_k,
            charging_curve_lut: lut,
            ocv_table: ocv,
            u_neg_table: u_neg,
        })
    }

    #[staticmethod]
    fn from_product(product_id: PyBatteryProductId) -> Self {
        let rust_id: BatteryProductId = product_id.into();
        Self::from_spec(rust_id.spec())
    }

    #[staticmethod]
    fn by_product_id(id: &str) -> PyResult<Self> {
        let spec = hares_equipment::battery::catalog::by_id(id)
            .ok_or_else(|| PyValueError::new_err(format!("unknown battery product: {id}")))?;
        Ok(Self::from_spec(spec))
    }

    #[staticmethod]
    fn product_catalog() -> Vec<PyBatteryProductId> {
        BatteryProductId::ALL
            .iter()
            .copied()
            .map(PyBatteryProductId::from)
            .collect()
    }

    #[staticmethod]
    fn tesla_pw3() -> Self {
        Self::from_spec(BatteryProductId::TeslaPw3.spec())
    }
    #[staticmethod]
    fn tesla_pw2() -> Self {
        Self::from_spec(BatteryProductId::TeslaPw2.spec())
    }
    #[staticmethod]
    fn tesla_pw2_nca() -> Self {
        Self::from_spec(BatteryProductId::TeslaPw2Nca.spec())
    }
    #[staticmethod]
    fn tesla_pw3_x2() -> Self {
        Self::from_spec(BatteryProductId::TeslaPw3X2.spec())
    }
    #[staticmethod]
    fn enphase_iq5p() -> Self {
        Self::from_spec(BatteryProductId::EnphaseIq5p.spec())
    }
    #[staticmethod]
    fn enphase_iq5p_x2() -> Self {
        Self::from_spec(BatteryProductId::EnphaseIq5pX2.spec())
    }
    #[staticmethod]
    fn enphase_iq10c() -> Self {
        Self::from_spec(BatteryProductId::EnphaseIq10c.spec())
    }
    #[staticmethod]
    fn franklin_apower() -> Self {
        Self::from_spec(BatteryProductId::FranklinApower.spec())
    }
    #[staticmethod]
    fn franklin_apower2() -> Self {
        Self::from_spec(BatteryProductId::FranklinApower2.spec())
    }
    #[staticmethod]
    fn franklin_apower2_x2() -> Self {
        Self::from_spec(BatteryProductId::FranklinApower2X2.spec())
    }
    #[staticmethod]
    fn solaredge_home() -> Self {
        Self::from_spec(BatteryProductId::SolaredgeHome.spec())
    }
    #[staticmethod]
    fn lg_resu10h() -> Self {
        Self::from_spec(BatteryProductId::LgResu10h.spec())
    }

    fn __repr__(&self) -> String {
        let mut parts = vec![
            format!("name={:?}", self.name),
            format!("capacity_kwh={}", self.capacity_kwh),
        ];
        if let Some(v) = self.max_charge_kw {
            parts.push(format!("max_charge_kw={v}"));
        }
        if let Some(v) = self.max_discharge_kw {
            parts.push(format!("max_discharge_kw={v}"));
        }
        if let Some(c) = self.chemistry {
            parts.push(format!("chemistry={c:?}"));
        }
        if let Some(v) = self.initial_soc {
            parts.push(format!("initial_soc={v}"));
        }
        if let Some(v) = self.min_soc {
            parts.push(format!("min_soc={v}"));
        }
        if let Some(v) = self.max_soc {
            parts.push(format!("max_soc={v}"));
        }
        if let Some(v) = self.inverter_efficiency {
            parts.push(format!("inverter_efficiency={v}"));
        }
        if let Some(v) = self.charge_efficiency {
            parts.push(format!("charge_efficiency={v}"));
        }
        if let Some(v) = self.discharge_efficiency {
            parts.push(format!("discharge_efficiency={v}"));
        }
        if let Some(v) = self.power_factor {
            parts.push(format!("power_factor={v}"));
        }
        if let Some(v) = self.inverter_capacity_kva {
            parts.push(format!("inverter_capacity_kva={v}"));
        }
        if let Some(v) = self.self_discharge_pct_per_day {
            parts.push(format!("self_discharge_pct_per_day={v}"));
        }
        if let Some(v) = self.standby_power_w {
            parts.push(format!("standby_power_w={v}"));
        }
        if let Some(v) = self.import_limit_w {
            parts.push(format!("import_limit_w={v}"));
        }
        if let Some(v) = self.export_limit_w {
            parts.push(format!("export_limit_w={v}"));
        }
        if let Some(v) = self.n_series {
            parts.push(format!("n_series={v}"));
        }
        if let Some(v) = self.n_parallel {
            parts.push(format!("n_parallel={v}"));
        }
        if let Some(v) = self.cell_resistance_ohm {
            parts.push(format!("cell_resistance_ohm={v}"));
        }
        if let Some(v) = self.heater_power_w {
            parts.push(format!("heater_power_w={v}"));
        }
        if let Some(v) = self.heater_threshold_c {
            parts.push(format!("heater_threshold_c={v}"));
        }
        if let Some(v) = self.min_charge_temp_c {
            parts.push(format!("min_charge_temp_c={v}"));
        }
        if let Some(v) = self.full_power_temp_c {
            parts.push(format!("full_power_temp_c={v}"));
        }
        if let Some(v) = self.cell_thermal_mass_j_per_k {
            parts.push(format!("cell_thermal_mass_j_per_k={v}"));
        }
        if let Some(v) = self.cell_ua_w_per_k {
            parts.push(format!("cell_ua_w_per_k={v}"));
        }
        format!("Battery({})", parts.join(", "))
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
    pub sam_lut_path: Option<String>,
    #[pyo3(get)]
    pub soiling: Option<PyPvSoilingConfig>,
}

#[pymethods]
impl PyPv {
    #[new]
    #[pyo3(signature = (name, capacity_kw, tilt, azimuth, sam_lut_path=None, soiling=None))]
    fn new(
        name: String,
        capacity_kw: f64,
        tilt: f64,
        azimuth: f64,
        sam_lut_path: Option<String>,
        soiling: Option<PyPvSoilingConfig>,
    ) -> Self {
        Self {
            name,
            capacity_kw,
            tilt,
            azimuth,
            sam_lut_path,
            soiling,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "PV(name={:?}, capacity_kw={}, tilt={}, azimuth={}, sam_lut_path={:?})",
            self.name, self.capacity_kw, self.tilt, self.azimuth, self.sam_lut_path
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
    #[pyo3(get)]
    pub initial_soc: Option<f64>,
    #[pyo3(get)]
    pub initial_connection_state: Option<PyEvConnectionState>,
    #[pyo3(get)]
    pub power_factor: Option<f64>,
    #[pyo3(get)]
    pub charger_capacity_kva: Option<f64>,
    /// Optional 4D charging curve LUT (soc × temp × c_rate × soh → power_fraction).
    pub charging_curve_lut: Option<RegularGridInterpolator>,
}

#[pymethods]
impl PyEv {
    #[new]
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (name, capacity_kwh=None, max_charging_kw=None, initial_soc=None, initial_connection_state=None, power_factor=None, charger_capacity_kva=None, charging_curve_lut=None))]
    fn new(
        py: Python<'_>,
        name: String,
        capacity_kwh: Option<f64>,
        max_charging_kw: Option<f64>,
        initial_soc: Option<f64>,
        initial_connection_state: Option<PyEvConnectionState>,
        power_factor: Option<f64>,
        charger_capacity_kva: Option<f64>,
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
            initial_soc,
            initial_connection_state,
            power_factor,
            charger_capacity_kva,
            charging_curve_lut: lut,
        })
    }

    #[staticmethod]
    fn from_vehicle(vehicle_id: PyVehicleId) -> Self {
        let rust_id: VehicleId = vehicle_id.into();
        Self::from_spec(rust_id.spec())
    }

    #[staticmethod]
    #[pyo3(signature = (vehicle_id, archetype_id, seed=0))]
    fn from_vehicle_with_archetype(
        vehicle_id: PyVehicleId,
        archetype_id: PyEvArchetypeId,
        #[allow(unused_variables)] seed: u64,
    ) -> Self {
        let rust_vid: VehicleId = vehicle_id.into();
        let rust_aid: EvArchetypeId = archetype_id.into();
        let spec = rust_vid.spec();
        let preset = rust_aid.preset();
        let max_power = match preset.charging_level {
            hares_types::ChargingLevel::L1 => spec.max_l2_power_kw.min(1.8),
            hares_types::ChargingLevel::L2 => spec.max_l2_power_kw,
        };
        Self {
            name: spec.label.to_string(),
            capacity_kwh: Some(spec.capacity_kwh),
            max_charging_kw: Some(max_power),
            initial_soc: None,
            initial_connection_state: None,
            power_factor: None,
            charger_capacity_kva: None,
            charging_curve_lut: None,
        }
    }

    #[staticmethod]
    fn by_vehicle_id(id: &str) -> PyResult<Self> {
        let spec = hares_equipment::ev::catalog::by_id(id)
            .ok_or_else(|| PyValueError::new_err(format!("unknown vehicle: {id}")))?;
        Ok(Self::from_spec(spec))
    }

    #[staticmethod]
    fn vehicle_catalog() -> Vec<PyVehicleId> {
        VehicleId::ALL
            .iter()
            .copied()
            .map(PyVehicleId::from)
            .collect()
    }

    #[staticmethod]
    fn tesla_model_y_lr() -> Self {
        Self::from_spec(VehicleId::TeslaModelYLr.spec())
    }
    #[staticmethod]
    fn tesla_model_y_sr() -> Self {
        Self::from_spec(VehicleId::TeslaModelYSr.spec())
    }
    #[staticmethod]
    fn tesla_model_3_lr() -> Self {
        Self::from_spec(VehicleId::TeslaModel3Lr.spec())
    }
    #[staticmethod]
    fn chevy_bolt_ev() -> Self {
        Self::from_spec(VehicleId::ChevyBoltEv.spec())
    }
    #[staticmethod]
    fn chevy_bolt_euv() -> Self {
        Self::from_spec(VehicleId::ChevyBoltEuv.spec())
    }
    #[staticmethod]
    fn ford_mache_sr() -> Self {
        Self::from_spec(VehicleId::FordMacheSr.spec())
    }
    #[staticmethod]
    fn ford_mache_er() -> Self {
        Self::from_spec(VehicleId::FordMacheEr.spec())
    }
    #[staticmethod]
    fn ford_lightning_er() -> Self {
        Self::from_spec(VehicleId::FordLightningEr.spec())
    }
    #[staticmethod]
    fn hyundai_ioniq5_lr() -> Self {
        Self::from_spec(VehicleId::HyundaiIoniq5Lr.spec())
    }
    #[staticmethod]
    fn nissan_leaf30() -> Self {
        Self::from_spec(VehicleId::NissanLeaf30.spec())
    }
    #[staticmethod]
    fn jeep_4xe() -> Self {
        Self::from_spec(VehicleId::Jeep4xe.spec())
    }
    #[staticmethod]
    fn toyota_rav4_prime() -> Self {
        Self::from_spec(VehicleId::ToyotaRav4Prime.spec())
    }
    #[staticmethod]
    fn chevy_volt_gen1() -> Self {
        Self::from_spec(VehicleId::ChevyVoltGen1.spec())
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

impl PyEv {
    fn from_spec(spec: &VehicleSpec) -> Self {
        Self {
            name: spec.label.to_string(),
            capacity_kwh: Some(spec.capacity_kwh),
            max_charging_kw: Some(spec.max_l2_power_kw),
            initial_soc: None,
            initial_connection_state: None,
            power_factor: None,
            charger_capacity_kva: None,
            charging_curve_lut: None,
        }
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

#[pyclass(name = "CoreOutput", frozen, from_py_object)]
#[derive(Clone)]
pub struct PyCoreOutput {
    inner: RustCoreOutput,
}

impl PyCoreOutput {
    pub(crate) fn new(inner: RustCoreOutput) -> Self {
        Self { inner }
    }
}

#[pymethods]
impl PyCoreOutput {
    #[getter]
    fn electric_kw(&self) -> Option<f64> {
        self.inner.flows.electric_kw.map(|e| e.net_consumption_kw())
    }

    #[getter]
    fn electric_convention(&self) -> Option<String> {
        match self.inner.flows.electric_kw {
            Some(ElectricPower::Consumption(_)) => Some("consumption".to_string()),
            Some(ElectricPower::Generation(_)) => Some("generation".to_string()),
            Some(ElectricPower::Bidirectional(_)) => Some("bidirectional".to_string()),
            None => None,
        }
    }

    #[getter]
    fn reactive_power_kvar(&self) -> Option<f64> {
        self.inner.flows.reactive_power_kvar
    }

    #[getter]
    fn fuel_w(&self) -> Option<f64> {
        self.inner.flows.fuel_w.map(|f| f.consumption_w)
    }

    #[getter]
    fn fuel_type(&self) -> Option<String> {
        self.inner.flows.fuel_w.map(|f| match f.fuel_type {
            FuelType::Electric => "Electric".to_string(),
            FuelType::Gas => "Gas".to_string(),
            FuelType::Propane => "Propane".to_string(),
            FuelType::Oil => "Oil".to_string(),
            FuelType::Wood => "Wood".to_string(),
            FuelType::Coal => "Coal".to_string(),
            FuelType::WoodPellet => "WoodPellet".to_string(),
            FuelType::None => "NoFuel".to_string(),
        })
    }

    #[getter]
    fn operating_mode(&self) -> Option<String> {
        self.inner.state.operating_mode.map(|mode| match mode {
            OperatingMode::Off => "Off".to_string(),
            OperatingMode::Heating => "Heating".to_string(),
            OperatingMode::Cooling => "Cooling".to_string(),
            OperatingMode::Defrost => "Defrost".to_string(),
            OperatingMode::Standby => "Standby".to_string(),
            OperatingMode::Charging => "Charging".to_string(),
            OperatingMode::Discharging => "Discharging".to_string(),
            OperatingMode::HeatingHP => "HeatingHP".to_string(),
            OperatingMode::HeatingER => "HeatingER".to_string(),
            OperatingMode::HeatingHPAndER => "HeatingHPAndER".to_string(),
            OperatingMode::HeatPumpWH => "HeatPumpWH".to_string(),
            OperatingMode::BackupElement => "BackupElement".to_string(),
            OperatingMode::On => "On".to_string(),
        })
    }

    #[getter]
    fn soc(&self) -> Option<f64> {
        self.inner.state.soc.map(|s| s.get())
    }

    fn __repr__(&self) -> String {
        format!(
            "CoreOutput(electric_kw={:?}, mode={:?}, soc={:?})",
            self.electric_kw(),
            self.operating_mode(),
            self.soc()
        )
    }
}

/// Render a resolved [`ZipLoad`] as a Python dict with one key per struct
/// field (`zp`/`ip`/`pp`/`zq`/`iq`/`pq`/`pf`/`v0`), matching the
/// `defaults/zip_parameters.toml` row names and the `"zip"` override keys.
pub(crate) fn zip_to_pydict<'py>(
    py: Python<'py>,
    zip: &hares_types::zip::ZipLoad,
) -> PyResult<Bound<'py, PyDict>> {
    let out = PyDict::new(py);
    out.set_item("zp", zip.zp)?;
    out.set_item("ip", zip.ip)?;
    out.set_item("pp", zip.pp)?;
    out.set_item("zq", zip.zq)?;
    out.set_item("iq", zip.iq)?;
    out.set_item("pq", zip.pq)?;
    out.set_item("pf", zip.pf)?;
    out.set_item("v0", zip.v0)?;
    Ok(out)
}

#[pyclass(name = "Equipment", frozen, from_py_object)]
#[derive(Clone)]
pub struct PyEquipment {
    descriptor: RustEquipmentDescriptor,
    core_output: RustCoreOutput,
    telemetry: RustTelemetry,
    resolved_zip: Option<hares_types::zip::ZipLoad>,
}

impl PyEquipment {
    pub(crate) fn new(
        descriptor: RustEquipmentDescriptor,
        core_output: RustCoreOutput,
        telemetry: RustTelemetry,
        resolved_zip: Option<hares_types::zip::ZipLoad>,
    ) -> Self {
        Self {
            descriptor,
            core_output,
            telemetry,
            resolved_zip,
        }
    }
}

#[pymethods]
impl PyEquipment {
    #[getter]
    fn name(&self) -> &str {
        &self.descriptor.name
    }

    #[getter]
    fn descriptor(&self) -> PyEquipmentDescriptor {
        PyEquipmentDescriptor::new(self.descriptor.clone())
    }

    #[getter]
    fn core_output(&self) -> PyCoreOutput {
        PyCoreOutput::new(self.core_output.clone())
    }

    /// The primary resolved ZIP/power-factor model this equipment applies to
    /// its (primary-component) real power for reactive purposes, as a dict
    /// with keys ``zp``/``ip``/``pp``/``zq``/``iq``/``pq``/``pf``/``v0``.
    ///
    /// For DER with live var control (battery, EV, PV) the ``pf`` value is
    /// the *current* effective power factor (config baseline, later mutated
    /// by a ``PowerFactorSetpoint``). ``None`` means the equipment has no
    /// electrical ZIP concept; ``pf == 0.0`` is the "reactive disabled"
    /// sentinel. Pure inspection — never influences the simulation.
    #[getter]
    fn resolved_zip<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyDict>>> {
        self.resolved_zip
            .as_ref()
            .map(|zip| zip_to_pydict(py, zip))
            .transpose()
    }

    /// Non-authoritative diagnostic telemetry map for this equipment.
    ///
    /// For simulation-critical values (power, fuel, mode, SoC), use
    /// `equipment.core_output` instead of this dictionary.
    fn telemetry<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let out = PyDict::new(py);
        for (k, v) in &self.telemetry {
            out.set_item(k, v)?;
        }
        Ok(out)
    }

    fn __repr__(&self) -> String {
        format!(
            "Equipment(name={:?}, type={:?})",
            self.descriptor.name, self.descriptor.equipment_type
        )
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

/// Python-facing protocol bridge configuration.
///
/// Registers protocol IDs that the bridge should recognise.
/// An empty or omitted `registered_protocols` means accept all.
///
/// `json_handlers` specifies protocol IDs for which a JSON payload handler
/// is configured. Each entry creates a `JsonHandler` that parses UTF-8 JSON
/// payloads of the form `[{"target": "...", "signal": {...}}]`. Without at
/// least one handler, the bridge records dispatch in telemetry but never
/// routes commands to target equipment.
#[pyclass(name = "ProtocolBridge")]
#[derive(Debug)]
pub struct PyProtocolBridge {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub registered_protocols: Vec<u16>,
    #[pyo3(get)]
    pub json_handlers: Vec<u16>,
}

#[pymethods]
impl PyProtocolBridge {
    #[new]
    #[pyo3(signature = (name, registered_protocols=None, json_handlers=None))]
    pub fn new(
        name: String,
        registered_protocols: Option<Vec<u16>>,
        json_handlers: Option<Vec<u16>>,
    ) -> Self {
        Self {
            name,
            registered_protocols: registered_protocols.unwrap_or_default(),
            json_handlers: json_handlers.unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hares_types::ElectricPower;

    #[test]
    fn py_core_output_missing_electric_has_no_convention() {
        let py_core = PyCoreOutput::new(RustCoreOutput::default());
        assert_eq!(py_core.electric_kw(), None);
        assert_eq!(py_core.electric_convention(), None);
    }

    #[test]
    fn py_core_output_reports_electric_convention_variants() {
        let consumption = PyCoreOutput::new(RustCoreOutput {
            flows: hares_types::CoreFlows {
                electric_kw: Some(ElectricPower::Consumption(1.2)),
                ..Default::default()
            },
            ..Default::default()
        });
        assert_eq!(
            consumption.electric_convention(),
            Some("consumption".to_string())
        );

        let generation = PyCoreOutput::new(RustCoreOutput {
            flows: hares_types::CoreFlows {
                electric_kw: Some(ElectricPower::Generation(1.2)),
                ..Default::default()
            },
            ..Default::default()
        });
        assert_eq!(
            generation.electric_convention(),
            Some("generation".to_string())
        );

        let bidirectional = PyCoreOutput::new(RustCoreOutput {
            flows: hares_types::CoreFlows {
                electric_kw: Some(ElectricPower::Bidirectional(-0.2)),
                ..Default::default()
            },
            ..Default::default()
        });
        assert_eq!(
            bidirectional.electric_convention(),
            Some("bidirectional".to_string())
        );
    }
}
