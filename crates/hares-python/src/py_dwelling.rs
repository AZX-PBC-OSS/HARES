//! Python bindings for dwelling simulation.

use std::path::PathBuf;
use std::sync::Mutex;

use chrono::{DateTime, Duration, FixedOffset};
use hares_control::PriceSignal;
use hares_core::{
    ActorConfig, ActorRegistry, BatteryLutData, Dwelling, DwellingConfig,
    environment::SurfaceGeometry,
};
use hares_equipment::{BatteryLutType, EquipmentConfig, EquipmentRegistry, config::ConfigValue};
use hares_io::{OutputFormat, SimulationConfig, output::metrics::MetricsCalculator};
use hares_types::{BatteryChemistry, EvConnectionState, SurfaceIrradiance};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyBytes, PyDict, PyList, PyType};
use std::sync::MutexGuard;

use crate::conversions::batches_or_steps_to_polars_df;
use crate::py_actor::PyActor;
use crate::py_config::PySimulationConfig;
use crate::py_control::PyControlSignal;
use crate::py_enums::PyLutType;
use crate::py_enums::{PyEvArchetypeId, PyVehicleId};
use crate::py_equipment::{
    PyBattery, PyEquipmentDescriptor, PyEv, PyPv, extract_charging_lut, extract_ocv_table,
    extract_u_neg_table,
};
use crate::py_metrics::PySimulationMetrics;
use crate::py_telemetry::PyTelemetry;
use crate::utils::extract_datetime;

const DEFAULT_START: &str = "2019-01-01T00:00:00Z";
const DEFAULT_DURATION_S: i64 = 24 * 60 * 60;
const DEFAULT_STEP_S: i64 = 60;

fn parse_solar_override(
    py: Python<'_>,
    data: &Bound<'_, PyAny>,
) -> PyResult<Vec<Vec<SurfaceIrradiance>>> {
    if let Ok(path) = data.extract::<String>() {
        return parse_solar_override_from_path(py, &path);
    }
    if let Ok(list) = data.extract::<Bound<'_, PyList>>() {
        return parse_solar_override_from_list(py, &list);
    }
    if data.getattr("__class__").is_ok() {
        let class_name = data.getattr("__class__")?.getattr("__name__")?;
        let class_name_str: String = class_name.extract()?;
        if class_name_str == "DataFrame" {
            return parse_solar_override_from_dataframe(py, data);
        }
        if class_name_str == "NDArray" || class_name_str == "ndarray" {
            return parse_solar_override_from_dict(py, data);
        }
    }
    if let Ok(dict) = data.extract::<Bound<'_, PyDict>>() {
        return parse_solar_override_from_dict(py, dict.as_any());
    }
    Err(PyValueError::new_err(
        "solar override must be a file path (str), polars DataFrame, numpy array dict, or list of dicts",
    ))
}

fn parse_solar_override_from_path(
    py: Python<'_>,
    path: &str,
) -> PyResult<Vec<Vec<SurfaceIrradiance>>> {
    let polars = py.import("polars")?;
    let read_parquet = polars.getattr("read_parquet")?;
    let df = read_parquet.call1((path,))?;
    parse_solar_override_from_dataframe(py, df.as_any())
}

fn parse_solar_override_from_dataframe(
    _py: Python<'_>,
    df: &Bound<'_, PyAny>,
) -> PyResult<Vec<Vec<SurfaceIrradiance>>> {
    let columns: Vec<String> = df.getattr("columns")?.extract()?;
    if columns.is_empty() {
        return Err(PyValueError::new_err("DataFrame has no columns"));
    }

    let n_rows: usize = df.getattr("height")?.extract()?;
    if n_rows == 0 {
        return Err(PyValueError::new_err("DataFrame has no rows"));
    }

    let mut surface_ids: Vec<u32> = Vec::new();
    for col in &columns {
        if col.starts_with("s") && col.ends_with("_direct") {
            if let Ok(sid) = col[1..col.len() - 7].parse::<u32>() {
                surface_ids.push(sid);
            }
        }
    }
    surface_ids.sort();

    if surface_ids.is_empty() {
        return Err(PyValueError::new_err(
            "DataFrame columns must include surface columns (e.g., s0_direct, s0_diffuse, s0_reflected, s0_aoi)",
        ));
    }

    struct ColumnData {
        surface_id: u32,
        direct: Vec<f64>,
        diffuse: Vec<f64>,
        reflected: Vec<f64>,
        aoi: Vec<f64>,
    }

    let extract_f64_series = |col_name: &str| -> PyResult<Vec<f64>> {
        df.get_item(col_name)?
            .call_method0("to_list")?
            .extract::<Vec<f64>>()
    };

    let columns_data: Vec<ColumnData> = surface_ids
        .iter()
        .map(|&sid| {
            Ok(ColumnData {
                surface_id: sid,
                direct: extract_f64_series(&format!("s{}_direct", sid))?,
                diffuse: extract_f64_series(&format!("s{}_diffuse", sid))?,
                reflected: extract_f64_series(&format!("s{}_reflected", sid))?,
                aoi: extract_f64_series(&format!("s{}_aoi", sid))?,
            })
        })
        .collect::<PyResult<Vec<_>>>()?;

    let mut result: Vec<Vec<SurfaceIrradiance>> = Vec::with_capacity(n_rows);

    for row_idx in 0..n_rows {
        let surfaces: Vec<SurfaceIrradiance> = columns_data
            .iter()
            .map(|col| SurfaceIrradiance {
                surface_id: col.surface_id,
                direct_w_m2: col.direct[row_idx],
                diffuse_w_m2: col.diffuse[row_idx],
                reflected_w_m2: col.reflected[row_idx],
                angle_of_incidence_rad: col.aoi[row_idx],
            })
            .collect();
        result.push(surfaces);
    }

    Ok(result)
}

fn parse_solar_override_from_dict(
    _py: Python<'_>,
    data: &Bound<'_, PyAny>,
) -> PyResult<Vec<Vec<SurfaceIrradiance>>> {
    let dict: Bound<'_, PyDict> = data.extract()?;
    let mut surface_ids: Vec<u32> = Vec::new();

    for (key, _) in dict.iter() {
        if let Ok(sid) = key.extract::<u32>() {
            surface_ids.push(sid);
        }
    }
    surface_ids.sort();

    if surface_ids.is_empty() {
        return Err(PyValueError::new_err(
            "dict must have integer keys for surface IDs",
        ));
    }

    let first_surface: Bound<'_, PyDict> = dict.get_item(surface_ids[0])?.unwrap().extract()?;
    let direct_val = first_surface.get_item("direct")?.unwrap();
    let n_timesteps: usize = get_array_len(&direct_val)?;

    let mut result: Vec<Vec<SurfaceIrradiance>> = Vec::with_capacity(n_timesteps);

    for step_idx in 0..n_timesteps {
        let mut surfaces = Vec::with_capacity(surface_ids.len());
        for &sid in &surface_ids {
            let surface_dict: Bound<'_, PyDict> = dict.get_item(sid)?.unwrap().extract()?;

            let direct: f64 =
                extract_array_element(&surface_dict.get_item("direct")?.unwrap(), step_idx)?;
            let diffuse: f64 =
                extract_array_element(&surface_dict.get_item("diffuse")?.unwrap(), step_idx)?;
            let reflected: f64 =
                extract_array_element(&surface_dict.get_item("reflected")?.unwrap(), step_idx)?;
            let aoi: f64 =
                extract_array_element(&surface_dict.get_item("aoi")?.unwrap(), step_idx)?;

            surfaces.push(SurfaceIrradiance {
                surface_id: sid,
                direct_w_m2: direct,
                diffuse_w_m2: diffuse,
                reflected_w_m2: reflected,
                angle_of_incidence_rad: aoi,
            });
        }
        result.push(surfaces);
    }

    Ok(result)
}

fn get_array_len(arr: &Bound<'_, PyAny>) -> PyResult<usize> {
    if let Ok(list) = arr.extract::<Bound<'_, PyList>>() {
        return Ok(list.len());
    }
    if let Ok(ndarray) = arr.getattr("__class__") {
        let name: String = ndarray.getattr("__name__")?.extract()?;
        if name == "ndarray" {
            if let Ok(shape) = arr.getattr("shape") {
                if let Ok(tuple) = shape.extract::<Bound<'_, pyo3::types::PyTuple>>() {
                    if tuple.len() > 0 {
                        if let Ok(Ok(dim)) = tuple.get_item(0).map(|v| v.extract::<usize>()) {
                            return Ok(dim);
                        }
                    }
                }
            }
        }
    }
    Err(PyValueError::new_err("Expected a list or numpy array"))
}

fn extract_array_element(arr: &Bound<'_, PyAny>, index: usize) -> PyResult<f64> {
    if let Ok(list) = arr.extract::<Bound<'_, PyList>>() {
        return list.get_item(index)?.extract::<f64>();
    }
    if let Ok(ndarray) = arr.getattr("__class__") {
        let name: String = ndarray.getattr("__name__")?.extract()?;
        if name == "ndarray" {
            return arr.get_item(index)?.extract::<f64>();
        }
    }
    arr.get_item(index)?.extract::<f64>()
}

fn parse_solar_override_from_list(
    _py: Python<'_>,
    list: &Bound<'_, PyList>,
) -> PyResult<Vec<Vec<SurfaceIrradiance>>> {
    let len = list.len();
    if len == 0 {
        return Err(PyValueError::new_err("list must not be empty"));
    }

    let mut result: Vec<Vec<SurfaceIrradiance>> = Vec::with_capacity(len);

    for item in list.iter() {
        let step_list: Bound<'_, PyList> = item.extract()?;
        let mut surfaces = Vec::with_capacity(step_list.len());

        for surface_item in step_list.iter() {
            let surface_dict: Bound<'_, PyDict> = surface_item.extract()?;

            let surface_id: u32 = surface_dict.get_item("surface_id")?.unwrap().extract()?;
            let direct_w_m2: f64 = surface_dict.get_item("direct_w_m2")?.unwrap().extract()?;
            let diffuse_w_m2: f64 = surface_dict.get_item("diffuse_w_m2")?.unwrap().extract()?;
            let reflected_w_m2: f64 = surface_dict
                .get_item("reflected_w_m2")?
                .unwrap()
                .extract()?;
            let angle_of_incidence_rad: f64 = surface_dict
                .get_item("angle_of_incidence_rad")
                .ok()
                .flatten()
                .map(|v| v.extract::<f64>())
                .unwrap_or(Ok(0.0))
                .unwrap_or(0.0);

            surfaces.push(SurfaceIrradiance {
                surface_id,
                direct_w_m2,
                diffuse_w_m2,
                reflected_w_m2,
                angle_of_incidence_rad,
            });
        }
        result.push(surfaces);
    }

    Ok(result)
}

fn insert_battery_optional_config(
    raw_config: &mut std::collections::HashMap<String, ConfigValue>,
    battery: &PyBattery,
) {
    if let Some(c) = battery.chemistry {
        raw_config.insert(
            "chemistry".to_string(),
            ConfigValue::Text(BatteryChemistry::from(c).as_config_str().to_string()),
        );
    }
    if let Some(v) = battery.initial_soc {
        raw_config.insert("initial_soc".to_string(), ConfigValue::Float(v));
    }
    if let Some(v) = battery.min_soc {
        raw_config.insert("min_soc".to_string(), ConfigValue::Float(v));
    }
    if let Some(v) = battery.max_soc {
        raw_config.insert("max_soc".to_string(), ConfigValue::Float(v));
    }
    if let Some(v) = battery.inverter_efficiency {
        raw_config.insert("inverter_efficiency".to_string(), ConfigValue::Float(v));
    }
    if let Some(v) = battery.charge_efficiency {
        raw_config.insert("charge_efficiency".to_string(), ConfigValue::Float(v));
    }
    if let Some(v) = battery.discharge_efficiency {
        raw_config.insert("discharge_efficiency".to_string(), ConfigValue::Float(v));
    }
    if let Some(v) = battery.self_discharge_pct_per_day {
        raw_config.insert(
            "self_discharge_pct_per_day".to_string(),
            ConfigValue::Float(v),
        );
    }
    if let Some(v) = battery.standby_power_w {
        raw_config.insert("standby_power_w".to_string(), ConfigValue::Float(v));
    }
    if let Some(v) = battery.import_limit_w {
        raw_config.insert("import_limit_w".to_string(), ConfigValue::Float(v));
    }
    if let Some(v) = battery.export_limit_w {
        raw_config.insert("export_limit_w".to_string(), ConfigValue::Float(v));
    }
    if let Some(v) = battery.n_series {
        raw_config.insert("n_series".to_string(), ConfigValue::Float(f64::from(v)));
    }
    if let Some(v) = battery.n_parallel {
        raw_config.insert("n_parallel".to_string(), ConfigValue::Float(f64::from(v)));
    }
    if let Some(v) = battery.cell_resistance_ohm {
        raw_config.insert("cell_resistance_ohm".to_string(), ConfigValue::Float(v));
    }
    if let Some(v) = battery.heater_power_w {
        raw_config.insert("heater_power_w".to_string(), ConfigValue::Float(v));
    }
    if let Some(v) = battery.heater_threshold_c {
        raw_config.insert("heater_threshold_c".to_string(), ConfigValue::Float(v));
    }
    if let Some(v) = battery.min_charge_temp_c {
        raw_config.insert("min_charge_temp_c".to_string(), ConfigValue::Float(v));
    }
    if let Some(v) = battery.full_power_temp_c {
        raw_config.insert("full_power_temp_c".to_string(), ConfigValue::Float(v));
    }
    if let Some(v) = battery.cell_thermal_mass_j_per_k {
        raw_config.insert(
            "cell_thermal_mass_j_per_k".to_string(),
            ConfigValue::Float(v),
        );
    }
    if let Some(v) = battery.cell_ua_w_per_k {
        raw_config.insert("cell_ua_w_per_k".to_string(), ConfigValue::Float(v));
    }
}

fn lock_dwelling(dwelling: &Mutex<Dwelling>) -> PyResult<MutexGuard<'_, Dwelling>> {
    dwelling.lock().map_err(|e| {
        PyRuntimeError::new_err(format!(
            "dwelling state is corrupted (internal panic: {}); create a new Dwelling instance",
            e
        ))
    })
}

/// GIL-free variant of [`lock_dwelling`] required by `Send` contexts (e.g. Rayon threads).
///
/// Returns `Err(String)` instead of `PyErr` so it can be used without holding the GIL.
pub(crate) fn lock_dwelling_string(
    dwelling: &Mutex<Dwelling>,
) -> Result<MutexGuard<'_, Dwelling>, String> {
    dwelling.lock().map_err(|e| {
        format!(
            "dwelling state is corrupted (internal panic: {}); create a new Dwelling instance",
            e
        )
    })
}

#[pyclass(name = "Dwelling")]
pub struct PyDwelling {
    pub(crate) dwelling: Mutex<Dwelling>,
    pub(crate) config: DwellingConfig,
    sim_config: PySimulationConfig,
    initial_state: Option<Vec<u8>>,
    initialized: bool,
    registry: ActorRegistry,
    equipment_registry: EquipmentRegistry,
    zone_keys: Option<Vec<String>>,
}

#[pymethods]
impl PyDwelling {
    #[classmethod]
    #[pyo3(signature = (hpxml, schedule, weather, **kwargs))]
    pub fn from_hpxml(
        _cls: &Bound<'_, PyType>,
        hpxml: String,
        schedule: String,
        weather: String,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Self> {
        let config = build_config(hpxml, schedule, weather, kwargs)?;
        let sim_config = PySimulationConfig::from_sim_config(&config.sim_config);
        let dwelling = Dwelling::from_config(config.clone()).map_err(to_py_err)?;
        Ok(Self {
            dwelling: Mutex::new(dwelling),
            config,
            sim_config,
            initial_state: None,
            initialized: false,
            registry: ActorRegistry::new(),
            equipment_registry: EquipmentRegistry::new(),
            zone_keys: None,
        })
    }

    pub fn initialize(&mut self) -> PyResult<()> {
        if self.initialized || self.initial_state.is_some() {
            self.initialized = true;
            return Ok(());
        }

        let bytes = {
            let dwelling = lock_dwelling(&self.dwelling)?;
            serde_json::to_vec(&dwelling.save_checkpoint()).map_err(to_py_err)?
        };

        self.initial_state = Some(bytes);
        self.initialized = true;
        Ok(())
    }

    pub fn timesteps(&self) -> PyResult<PyTimestepsIter> {
        let dwelling = lock_dwelling(&self.dwelling)?;

        let start = dwelling.clock.start_time;
        let time_res_s = dwelling.clock.time_res.num_seconds();
        let total_steps = dwelling.clock.total_steps();
        let current_step = dwelling.clock.current_step();

        Ok(PyTimestepsIter {
            start,
            time_res_s,
            next_step: current_step,
            total_steps,
        })
    }

    pub fn simulate(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        py.detach(|| {
            let mut dwelling = lock_dwelling_string(&self.dwelling)?;
            dwelling.simulate().map_err(|err| err.to_string())?;
            Ok::<_, String>(())
        })
        .map_err(PyRuntimeError::new_err)?;

        let dwelling = lock_dwelling(&self.dwelling)?;
        let batches: Vec<_> = dwelling.flushed_batches().to_vec();
        let steps = dwelling.results().steps.clone();
        drop(dwelling);
        batches_or_steps_to_polars_df(py, &batches, &steps)
    }

    pub fn step(&mut self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let step = py
            .detach(|| self.step_core())
            .map_err(PyRuntimeError::new_err)?;

        if self.zone_keys.is_none() {
            let keys: Vec<String> = step
                .zone_temperatures_c
                .iter()
                .map(|(zone_id, _)| {
                    if zone_id.0 == 0 {
                        "Temperature - Indoor (C)".to_string()
                    } else {
                        format!("Temperature - Zone_{} (C)", zone_id.0)
                    }
                })
                .collect();
            self.zone_keys = Some(keys);
        }

        let out = PyDict::new(py);
        out.set_item("time", chrono_to_py_datetime(py, step.timestamp)?)?;
        out.set_item("net_electric_power_kw", step.net_electric_power_kw)?;
        out.set_item("hvac_heating_w", step.hvac_heating_w)?;
        out.set_item("hvac_cooling_w", step.hvac_cooling_w)?;
        for ((_zone_id, temp_c), key) in step
            .zone_temperatures_c
            .iter()
            .zip(self.zone_keys.as_ref().unwrap().iter())
        {
            out.set_item(key, *temp_c)?;
        }
        Ok(out.unbind().into())
    }

    pub fn apply_control(&self, name: String, signal: &PyControlSignal) -> PyResult<()> {
        let mut dwelling = lock_dwelling(&self.dwelling)?;
        dwelling
            .apply_control_validated(&name, signal.signal.clone())
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }

    /// Adds a Python actor to the dwelling's decision-making loop.
    pub fn add_actor(&self, py: Python<'_>, actor: Py<PyActor>) -> PyResult<()> {
        let mut dwelling = lock_dwelling(&self.dwelling)?;
        let wrapper = crate::py_actor::PyActorWrapper::new(py, actor);
        dwelling.add_actor(Box::new(wrapper));
        Ok(())
    }

    /// Creates and adds an actor from the registry using configuration.
    ///
    /// # Arguments
    ///
    /// * `actor_type` - Registered actor type name (e.g., "IdealThermostat", "Occupant", "DrCompliance")
    /// * `name` - Actor instance name
    /// * `params` - Configuration parameters as a dictionary
    pub fn add_actor_by_name(
        &self,
        actor_type: String,
        name: String,
        params: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<()> {
        let mut config = ActorConfig::new(name, actor_type);

        if let Some(dict) = params {
            for (key, value) in dict.iter() {
                let key: String = key.extract()?;
                let config_value = py_to_config_value(&value)?;
                config = config.with_param(key, config_value);
            }
        }

        let mut dwelling = lock_dwelling(&self.dwelling)?;
        dwelling
            .add_actor_by_name(&self.registry, config)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        Ok(())
    }

    pub fn set_price_signal(&self, signal: &Bound<'_, PyDict>) -> PyResult<()> {
        let price = PriceSignal {
            electricity_price: dict_optional(signal, "electricity_price")?,
            export_price: dict_optional(signal, "export_price")?,
            ghg_intensity: dict_optional(signal, "ghg_intensity")?,
        };
        let mut dwelling = lock_dwelling(&self.dwelling)?;
        dwelling.set_price_signal(price);
        Ok(())
    }

    pub fn set_grid_voltage(&self, voltage_pu: f64) -> PyResult<()> {
        let mut dwelling = lock_dwelling(&self.dwelling)?;
        dwelling.set_grid_voltage(voltage_pu);
        Ok(())
    }

    #[cfg(feature = "observe")]
    pub fn enable_observer(&self, capacity: usize) -> PyResult<()> {
        let mut dwelling = lock_dwelling(&self.dwelling)?;
        dwelling.enable_observer(capacity);
        Ok(())
    }

    #[cfg(feature = "observe")]
    pub fn drain_observations(&self, py: Python<'_>) -> PyResult<Vec<Py<PyAny>>> {
        let mut dwelling = lock_dwelling(&self.dwelling)?;
        let snapshots = dwelling.drain_observations();
        snapshots
            .into_iter()
            .map(|snap| snapshot_to_py(py, &snap))
            .collect()
    }

    pub fn telemetry(&self) -> PyResult<PyTelemetry> {
        let dwelling = lock_dwelling(&self.dwelling)?;
        Ok(PyTelemetry::new(dwelling.telemetry()))
    }

    pub fn equipment_descriptors(&self) -> PyResult<Vec<PyEquipmentDescriptor>> {
        let dwelling = lock_dwelling(&self.dwelling)?;
        Ok(dwelling
            .equipment()
            .iter()
            .map(|eq| PyEquipmentDescriptor::new(eq.descriptor().clone()))
            .collect())
    }

    pub fn equipment_names(&self) -> PyResult<Vec<String>> {
        let dwelling = lock_dwelling(&self.dwelling)?;
        Ok(dwelling
            .equipment()
            .iter()
            .map(|eq| eq.descriptor().name.clone())
            .collect())
    }

    pub fn add_battery(&mut self, battery: &PyBattery) -> PyResult<()> {
        let mut raw_config = std::collections::HashMap::new();
        raw_config.insert(
            "capacity_kwh".to_string(),
            hares_equipment::config::ConfigValue::Float(battery.capacity_kwh),
        );
        if let Some(v) = battery.max_charge_kw {
            raw_config.insert(
                "max_charge_kw".to_string(),
                hares_equipment::config::ConfigValue::Float(v),
            );
        }
        if let Some(v) = battery.max_discharge_kw {
            raw_config.insert(
                "max_discharge_kw".to_string(),
                hares_equipment::config::ConfigValue::Float(v),
            );
        }
        insert_battery_optional_config(&mut raw_config, battery);

        let config = hares_equipment::EquipmentConfig {
            name: battery.name.clone(),
            ochre_class: "Battery".to_string(),
            raw_config,
        };

        let mut eq = self
            .equipment_registry
            .create("Battery", config.clone())
            .map_err(to_py_err)?;

        let mut dwelling = lock_dwelling(&self.dwelling)?;
        eq.init(&config, dwelling.latest_env()).map_err(to_py_err)?;

        // Set LUTs after init — init may reset internal state
        if let Some(ref lut) = battery.charging_curve_lut {
            eq.set_charging_curve_lut(Some(lut.clone()))
                .map_err(to_py_err)?;
        }
        if let Some(ref table) = battery.ocv_table {
            eq.set_ocv_table(table.clone()).map_err(to_py_err)?;
        }
        if let Some(ref table) = battery.u_neg_table {
            eq.set_u_neg_table(table.clone()).map_err(to_py_err)?;
        }

        dwelling.add_equipment(eq);
        Ok(())
    }

    pub fn add_pv(&mut self, pv: &PyPv) -> PyResult<()> {
        let mut raw_config = std::collections::HashMap::new();
        raw_config.insert(
            "capacity_kw".to_string(),
            hares_equipment::config::ConfigValue::Float(pv.capacity_kw),
        );
        raw_config.insert(
            "tilt_deg".to_string(),
            hares_equipment::config::ConfigValue::Float(pv.tilt),
        );
        raw_config.insert(
            "azimuth_deg".to_string(),
            hares_equipment::config::ConfigValue::Float(pv.azimuth),
        );

        let config = hares_equipment::EquipmentConfig {
            name: pv.name.clone(),
            ochre_class: "PV".to_string(),
            raw_config,
        };

        let mut eq = self
            .equipment_registry
            .create("PV", config.clone())
            .map_err(to_py_err)?;
        let mut dwelling = lock_dwelling(&self.dwelling)?;

        // Register the PV surface orientation with the environment so that
        // Perez irradiance is computed for this panel during simulation.
        let surface_id =
            hares_equipment::pv::surface_id_for_orientation(pv.tilt, pv.azimuth, 5.0)
                .map_err(to_py_err)?;
        dwelling.environment.register_surface(SurfaceGeometry {
            surface_id,
            azimuth_deg: pv.azimuth,
            tilt_deg: pv.tilt,
            area_m2: 1.0,
        });

        // PV init() validates that a SurfaceIrradiance entry exists for each
        // array. The environment computes real irradiance during simulation;
        // inject a placeholder so init() succeeds.
        let mut init_env = dwelling.latest_env().clone();
        if !init_env
            .weather
            .solar_irradiance
            .iter()
            .any(|s| s.surface_id == surface_id)
        {
            init_env.weather.solar_irradiance.push(SurfaceIrradiance {
                surface_id,
                direct_w_m2: 0.0,
                diffuse_w_m2: 0.0,
                reflected_w_m2: 0.0,
                angle_of_incidence_rad: 0.0,
            });
        }
        eq.init(&config, &init_env).map_err(to_py_err)?;

        dwelling.add_equipment(eq);
        Ok(())
    }

    pub fn add_ev(&mut self, ev: &PyEv) -> PyResult<()> {
        let mut raw_config = std::collections::HashMap::new();
        if let Some(v) = ev.capacity_kwh {
            raw_config.insert(
                "capacity_kwh".to_string(),
                hares_equipment::config::ConfigValue::Float(v),
            );
        }
        if let Some(v) = ev.max_charging_kw {
            raw_config.insert(
                "max_charging_power_kw".to_string(),
                hares_equipment::config::ConfigValue::Float(v),
            );
        }
        if let Some(v) = ev.initial_soc {
            raw_config.insert(
                "initial_soc".to_string(),
                hares_equipment::config::ConfigValue::Float(v),
            );
        }
        if let Some(state) = &ev.initial_connection_state {
            raw_config.insert(
                "initial_connection_state".to_string(),
                hares_equipment::config::ConfigValue::Text(
                    EvConnectionState::from(*state).to_string(),
                ),
            );
        }

        let config = hares_equipment::EquipmentConfig {
            name: ev.name.clone(),
            ochre_class: "EV".to_string(),
            raw_config,
        };

        let mut eq = self
            .equipment_registry
            .create("EV", config.clone())
            .map_err(to_py_err)?;

        let mut dwelling = lock_dwelling(&self.dwelling)?;
        eq.init(&config, dwelling.latest_env()).map_err(to_py_err)?;

        // Set LUT after init — init_from_config resets charging_curve_lut to None
        if let Some(ref lut) = ev.charging_curve_lut {
            eq.set_charging_curve_lut(Some(lut.clone()))
                .map_err(to_py_err)?;
        }

        dwelling.add_equipment(eq);
        Ok(())
    }

    pub fn add_ev_with_driver(
        &mut self,
        vehicle_id: PyVehicleId,
        archetype_id: PyEvArchetypeId,
        seed: u64,
    ) -> PyResult<()> {
        use hares_core::actors::ev_driver::EvDriverActor;
        use hares_equipment::ev::catalog::{EvArchetypeId, VehicleId};

        let rust_vid: VehicleId = vehicle_id.into();
        let rust_aid: EvArchetypeId = archetype_id.into();
        let spec = rust_vid.spec();
        let preset = rust_aid.preset();

        let max_power = match preset.charging_level {
            hares_types::ChargingLevel::L1 => spec.max_l2_power_kw.min(1.8),
            hares_types::ChargingLevel::L2 => spec.max_l2_power_kw,
        };

        let mut raw_config = std::collections::HashMap::new();
        raw_config.insert(
            "capacity_kwh".to_string(),
            hares_equipment::config::ConfigValue::Float(spec.capacity_kwh),
        );
        raw_config.insert(
            "max_charging_power_kw".to_string(),
            hares_equipment::config::ConfigValue::Float(max_power),
        );

        let config = hares_equipment::EquipmentConfig {
            name: spec.label.to_string(),
            ochre_class: "EV".to_string(),
            raw_config,
        };

        let mut eq = self
            .equipment_registry
            .create("EV", config.clone())
            .map_err(to_py_err)?;

        let mut dwelling = lock_dwelling(&self.dwelling)?;
        eq.init(&config, dwelling.latest_env()).map_err(to_py_err)?;
        dwelling.add_equipment(eq);

        let fuel_economy = spec.capacity_kwh / spec.range_miles;
        let seed_bytes = {
            let mut bytes = [0u8; 32];
            bytes[..8].copy_from_slice(&seed.to_le_bytes());
            bytes
        };

        let actor = EvDriverActor::new(
            &format!("{}_driver", spec.label),
            spec.label,
            preset.strategy.clone(),
            preset.plug_in_policy.clone(),
            preset.build_miles_schedule(seed_bytes),
            preset.build_departure_schedule(seed_bytes),
            preset.build_duration_schedule(seed_bytes),
            preset.event_day_ratio,
            fuel_economy,
            spec.capacity_kwh,
            max_power,
            30.0, // average_speed_mph
            20.0, // range_anxiety_miles
            0.0,  // away_charge_fraction
            0.0,  // away_charge_power_kw
            seed,
        );

        dwelling.add_actor(Box::new(actor));
        Ok(())
    }

    pub fn remove_equipment(&mut self, name: &str) -> PyResult<()> {
        let mut dwelling = lock_dwelling(&self.dwelling)?;
        dwelling.remove_equipment(name).map_err(to_py_err)?;
        Ok(())
    }

    /// Replace equipment by name with a new equipment config object (Battery, PV, or EV).
    pub fn replace_equipment(
        &mut self,
        name: String,
        equipment: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let new_eq = self.py_any_to_equipment(equipment)?;
        let mut dwelling = lock_dwelling(&self.dwelling)?;
        dwelling
            .replace_equipment(&name, new_eq)
            .map_err(to_py_err)?;
        Ok(())
    }

    /// Update LUT parameters on existing equipment by name.
    ///
    /// Supported kwargs:
    /// - `charging_curve`: 4D charging curve LUT (Battery, EV)
    /// - `ocv_table`: open-circuit voltage table (Battery)
    /// - `uneg_table`: negative electrode potential table (Battery)
    #[pyo3(signature = (name, **kwargs))]
    pub fn update_equipment(
        &mut self,
        py: Python<'_>,
        name: String,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<()> {
        let kwargs = kwargs.ok_or_else(|| {
            PyValueError::new_err("update_equipment requires at least one keyword argument")
        })?;

        let mut dwelling = lock_dwelling(&self.dwelling)?;

        if let Some(cc) = kwargs.get_item("charging_curve")? {
            let lut = extract_charging_lut(py, &cc)?;
            // Works for both Battery and EV via the trait method
            dwelling
                .set_battery_lut(
                    &name,
                    BatteryLutType::ChargingCurve,
                    BatteryLutData::ChargingCurve(lut.clone()),
                )
                .or_else(|_| dwelling.set_ev_charging_curve_lut(&name, lut))
                .map_err(to_py_err)?;
        }
        if let Some(ocv) = kwargs.get_item("ocv_table")? {
            let table = extract_ocv_table(py, &ocv)?;
            dwelling
                .set_battery_lut(&name, BatteryLutType::Ocv, BatteryLutData::Ocv(table))
                .map_err(to_py_err)?;
        }
        if let Some(uneg) = kwargs.get_item("uneg_table")? {
            let table = extract_u_neg_table(py, &uneg)?;
            dwelling
                .set_battery_lut(&name, BatteryLutType::UNeg, BatteryLutData::UNeg(table))
                .map_err(to_py_err)?;
        }

        Ok(())
    }

    /// Set a lookup table on equipment by name and LUT type.
    ///
    /// `lut_type` is a `LutType` enum value. `data` depends on the LUT type:
    /// - `LutType.ChargingCurve`: NPZ path (str) or dict with numpy arrays
    /// - `LutType.Ocv`: list of (soc, voltage) tuples, dict, or polars DataFrame
    /// - `LutType.UNeg`: list of (soc, potential) tuples, dict, or polars DataFrame
    pub fn set_equipment_lut(
        &mut self,
        py: Python<'_>,
        name: String,
        lut_type: &PyLutType,
        data: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let mut dwelling = lock_dwelling(&self.dwelling)?;
        match lut_type {
            PyLutType::ChargingCurve => {
                let lut = extract_charging_lut(py, data)?;
                // Try battery first, then EV
                dwelling
                    .set_battery_lut(
                        &name,
                        BatteryLutType::ChargingCurve,
                        BatteryLutData::ChargingCurve(lut.clone()),
                    )
                    .or_else(|_| dwelling.set_ev_charging_curve_lut(&name, lut))
                    .map_err(to_py_err)?;
            }
            PyLutType::Ocv => {
                let table = extract_ocv_table(py, data)?;
                dwelling
                    .set_battery_lut(&name, BatteryLutType::Ocv, BatteryLutData::Ocv(table))
                    .map_err(to_py_err)?;
            }
            PyLutType::UNeg => {
                let table = extract_u_neg_table(py, data)?;
                dwelling
                    .set_battery_lut(&name, BatteryLutType::UNeg, BatteryLutData::UNeg(table))
                    .map_err(to_py_err)?;
            }
        }
        Ok(())
    }

    /// Clear / reset a LUT on equipment by name and LUT type.
    pub fn clear_equipment_lut(&mut self, name: String, lut_type: &PyLutType) -> PyResult<()> {
        let mut dwelling = lock_dwelling(&self.dwelling)?;
        match lut_type {
            PyLutType::ChargingCurve => {
                dwelling
                    .clear_battery_lut(&name, BatteryLutType::ChargingCurve)
                    .or_else(|_| dwelling.clear_ev_charging_curve_lut(&name))
                    .map_err(to_py_err)?;
            }
            PyLutType::Ocv => {
                dwelling
                    .clear_battery_lut(&name, BatteryLutType::Ocv)
                    .map_err(to_py_err)?;
            }
            PyLutType::UNeg => {
                dwelling
                    .clear_battery_lut(&name, BatteryLutType::UNeg)
                    .map_err(to_py_err)?;
            }
        }
        Ok(())
    }

    /// Get current LUT state for equipment. Returns None if using defaults.
    ///
    /// For ChargingCurve: returns True if a LUT is set, None if not.
    /// For Ocv/UNeg: always returns True (tables always exist, even defaults).
    pub fn has_equipment_lut(&self, name: String, lut_type: &PyLutType) -> PyResult<bool> {
        let dwelling = lock_dwelling(&self.dwelling)?;
        let eq = dwelling
            .equipment()
            .iter()
            .find(|e| e.descriptor().name == name)
            .ok_or_else(|| PyValueError::new_err(format!("equipment '{}' not found", name)))?;
        Ok(match lut_type {
            PyLutType::ChargingCurve => eq.has_charging_curve_lut(),
            PyLutType::Ocv => eq.has_custom_ocv_table(),
            PyLutType::UNeg => eq.has_custom_u_neg_table(),
        })
    }

    pub fn validate_control(&self, name: &str, signal: &PyControlSignal) -> PyResult<bool> {
        let dwelling = lock_dwelling(&self.dwelling)?;
        let required_cap = signal.signal.required_capability();
        if let Some(eq) = dwelling
            .equipment()
            .iter()
            .find(|e| e.descriptor().name == name)
        {
            let caps = eq.descriptor().control_capabilities;
            Ok(caps.contains(required_cap))
        } else {
            Ok(false)
        }
    }

    pub fn results(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let dwelling = lock_dwelling(&self.dwelling)?;
        let batches: Vec<_> = dwelling.flushed_batches().to_vec();
        let steps = dwelling.results().steps.clone();
        drop(dwelling);
        batches_or_steps_to_polars_df(py, &batches, &steps)
    }

    pub fn metrics(&self) -> PyResult<PySimulationMetrics> {
        let dwelling = lock_dwelling(&self.dwelling)?;
        let batches: Vec<_> = dwelling.flushed_batches().to_vec();

        if batches.is_empty() {
            return Err(PyValueError::new_err(
                "no batches flushed yet; run simulate() first",
            ));
        }

        let schema = batches[0].schema();
        let time_res_secs = self.sim_config.time_res_s() as u32;

        let mut calculator =
            MetricsCalculator::new(&schema, time_res_secs, &self.config.sim_config).map_err(
                |e| PyValueError::new_err(format!("failed to create metrics calculator: {e}")),
            )?;

        for batch in &batches {
            calculator.accumulate(batch);
        }

        let full_metrics = calculator.finish();
        Ok(PySimulationMetrics {
            inner: full_metrics,
        })
    }

    pub fn reset_with_seed(&mut self, seed: u64) -> PyResult<()> {
        let mut config = self.config.clone();
        config.sim_config.master_seed = seed;

        let dwelling = Dwelling::from_config(config.clone()).map_err(to_py_err)?;
        self.dwelling = Mutex::new(dwelling);
        self.config = config;
        self.initialized = false;
        self.initial_state = None;
        self.zone_keys = None;

        self.initialize()
    }

    pub fn save_state<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let dwelling = lock_dwelling(&self.dwelling)?;
        let bytes = serde_json::to_vec(&dwelling.save_checkpoint()).map_err(to_py_err)?;
        Ok(PyBytes::new(py, &bytes))
    }

    pub fn load_state(&self, state: &[u8]) -> PyResult<()> {
        let checkpoint = serde_json::from_slice(state).map_err(to_py_err)?;
        let mut dwelling = lock_dwelling(&self.dwelling)?;
        dwelling.load_checkpoint(checkpoint).map_err(to_py_err)
    }

    fn __repr__(&self) -> String {
        format!(
            "Dwelling(bldg_id={}, initialized={})",
            self.config.bldg_id, self.initialized
        )
    }

    fn config(&self) -> PySimulationConfig {
        self.sim_config.clone()
    }

    pub fn set_solar_override(&self, py: Python<'_>, data: &Bound<'_, PyAny>) -> PyResult<()> {
        let solar_data = parse_solar_override(py, data)?;
        let mut dwelling = lock_dwelling(&self.dwelling)?;
        dwelling.environment.set_solar_override(solar_data);
        Ok(())
    }

    pub fn clear_solar_override(&self) -> PyResult<()> {
        let mut dwelling = lock_dwelling(&self.dwelling)?;
        dwelling.environment.clear_solar_override();
        Ok(())
    }

    pub fn has_solar_override(&self) -> PyResult<bool> {
        let dwelling = lock_dwelling(&self.dwelling)?;
        Ok(dwelling.environment.has_solar_override())
    }

    pub fn surface_ids(&self) -> PyResult<Vec<u32>> {
        let dwelling = lock_dwelling(&self.dwelling)?;
        Ok(dwelling
            .environment
            .surface_geometry()
            .iter()
            .map(|s| s.surface_id)
            .collect())
    }

    /// Return all roof planes from the parsed HPXML building.
    pub fn roof_planes(&self) -> PyResult<Vec<crate::py_pv_sizing::PyRoofPlane>> {
        let dwelling = lock_dwelling(&self.dwelling)?;
        Ok(crate::py_pv_sizing::roof_planes_from_dwelling(
            &dwelling.roof_info,
        ))
    }

    /// Enumerate all viable PV candidate placements, one per non-north-facing
    /// roof plane, sorted by solar production score (best first).
    pub fn pv_candidates(&self) -> PyResult<Vec<crate::py_pv_sizing::PyPvCandidate>> {
        let dwelling = lock_dwelling(&self.dwelling)?;
        let roof_shape = hares_physics::pv_sizing::infer_roof_shape(
            &dwelling.roof_info,
            dwelling.facility_type.as_deref(),
            dwelling.latitude_deg,
        );
        Ok(crate::py_pv_sizing::pv_candidates_from_dwelling(
            &dwelling.roof_info,
            roof_shape,
            &dwelling.wall_azimuths,
            dwelling.latitude_deg,
        ))
    }

    /// Size a PV system to a target capacity, constrained by roof geometry.
    ///
    /// Returns the best single-array sizing result. Use `pv_candidates()` to
    /// see all viable placements.
    #[pyo3(signature = (target_kw, min_kw=2.0, max_kw=14.0))]
    pub fn estimate_pv_capacity(
        &self,
        target_kw: f64,
        min_kw: f64,
        max_kw: f64,
    ) -> PyResult<crate::py_pv_sizing::PyPvSizingResult> {
        let dwelling = lock_dwelling(&self.dwelling)?;
        let roof_shape = hares_physics::pv_sizing::infer_roof_shape(
            &dwelling.roof_info,
            dwelling.facility_type.as_deref(),
            dwelling.latitude_deg,
        );
        crate::py_pv_sizing::size_pv_from_dwelling(
            &dwelling.roof_info,
            roof_shape,
            &dwelling.wall_azimuths,
            dwelling.latitude_deg,
            target_kw,
            min_kw,
            max_kw,
        )
        .map_err(pyo3::exceptions::PyValueError::new_err)
    }

    /// Attach an electric tariff to this dwelling for billing.
    #[pyo3(signature = (tariff, timezone = None))]
    pub fn set_electric_tariff(
        &self,
        tariff: &crate::py_tariff::PyElectricTariff,
        timezone: Option<&str>,
    ) -> PyResult<()> {
        let tz_str = timezone
            .map(|s| s.to_string())
            .or_else(|| self.config.sim_config.civil_timezone.clone())
            .unwrap_or_else(|| "UTC".to_string());
        let tz: chrono_tz::Tz = tz_str
            .parse()
            .map_err(|_| PyValueError::new_err(format!("unknown timezone '{tz_str}'")))?;
        let mut dwelling = lock_dwelling(&self.dwelling)?;
        dwelling
            .set_tariff(tariff.inner.clone(), tz)
            .map_err(to_py_err)
    }

    /// Returns accumulated billing period summaries.
    pub fn billing_summaries(&self) -> PyResult<Vec<crate::py_telemetry::PyBillingPeriodSummary>> {
        let dwelling = lock_dwelling(&self.dwelling)?;
        Ok(dwelling
            .billing_summaries()
            .iter()
            .map(crate::py_telemetry::PyBillingPeriodSummary::from_rust)
            .collect())
    }

    /// Returns a snapshot of current tariff state, or None if no tariff is set.
    pub fn tariff_telemetry(&self) -> PyResult<Option<crate::py_telemetry::PyTariffTelemetry>> {
        let dwelling = lock_dwelling(&self.dwelling)?;
        let evaluator = match dwelling.tariff_evaluator() {
            Some(ev) => ev,
            None => return Ok(None),
        };
        let billing = evaluator.billing_state();
        Ok(Some(crate::py_telemetry::PyTariffTelemetry {
            period_name: evaluator.current_period_name().to_string(),
            current_rate_usd_per_kwh: evaluator.current_price(),
            export_rate_usd_per_kwh: evaluator.current_export_price(),
            cumulative_import_kwh: billing.cumulative_import_kwh(),
            cumulative_export_kwh: billing.cumulative_export_kwh(),
            peak_demand_kw: billing.peak_demand_kw(),
            cumulative_energy_cost_usd: billing.cumulative_energy_cost_usd(),
        }))
    }
}

impl PyDwelling {
    pub(crate) fn step_core(&self) -> Result<hares_core::StepResult, String> {
        let mut dwelling = lock_dwelling_string(&self.dwelling)?;
        dwelling.step().map_err(|err| err.to_string())
    }

    pub(crate) fn observation(&self) -> Result<Vec<f64>, String> {
        let dwelling = lock_dwelling_string(&self.dwelling)?;
        let telemetry = dwelling.telemetry();
        telemetry
            .to_observation_vec(&["total_power_kw", "outdoor_temp", "outdoor_rh"])
            .map_err(|err| err.to_string())
    }

    /// Convert a Python equipment config object (Battery, PV, or EV) to a boxed Equipment.
    fn py_any_to_equipment(
        &self,
        obj: &Bound<'_, PyAny>,
    ) -> PyResult<Box<dyn hares_equipment::Equipment>> {
        if let Ok(battery) = obj.extract::<PyRef<'_, PyBattery>>() {
            let mut raw_config = std::collections::HashMap::new();
            raw_config.insert(
                "capacity_kwh".to_string(),
                ConfigValue::Float(battery.capacity_kwh),
            );
            if let Some(v) = battery.max_charge_kw {
                raw_config.insert("max_charge_kw".to_string(), ConfigValue::Float(v));
            }
            if let Some(v) = battery.max_discharge_kw {
                raw_config.insert("max_discharge_kw".to_string(), ConfigValue::Float(v));
            }
            insert_battery_optional_config(&mut raw_config, &battery);
            let config = EquipmentConfig {
                name: battery.name.clone(),
                ochre_class: "Battery".to_string(),
                raw_config,
            };
            let mut eq = self
                .equipment_registry
                .create("Battery", config)
                .map_err(to_py_err)?;
            if let Some(ref lut) = battery.charging_curve_lut {
                eq.set_charging_curve_lut(Some(lut.clone()))
                    .map_err(to_py_err)?;
            }
            return Ok(eq);
        }
        if let Ok(pv) = obj.extract::<PyRef<'_, PyPv>>() {
            let mut raw_config = std::collections::HashMap::new();
            raw_config.insert(
                "capacity_kw".to_string(),
                ConfigValue::Float(pv.capacity_kw),
            );
            raw_config.insert("tilt".to_string(), ConfigValue::Float(pv.tilt));
            raw_config.insert("azimuth".to_string(), ConfigValue::Float(pv.azimuth));
            let config = EquipmentConfig {
                name: pv.name.clone(),
                ochre_class: "PV".to_string(),
                raw_config,
            };
            return self
                .equipment_registry
                .create("PV", config)
                .map_err(to_py_err);
        }
        if let Ok(ev) = obj.extract::<PyRef<'_, PyEv>>() {
            let mut raw_config = std::collections::HashMap::new();
            if let Some(v) = ev.capacity_kwh {
                raw_config.insert("capacity_kwh".to_string(), ConfigValue::Float(v));
            }
            if let Some(v) = ev.max_charging_kw {
                raw_config.insert("max_charging_power_kw".to_string(), ConfigValue::Float(v));
            }
            if let Some(v) = ev.initial_soc {
                raw_config.insert("initial_soc".to_string(), ConfigValue::Float(v));
            }
            if let Some(state) = &ev.initial_connection_state {
                raw_config.insert(
                    "initial_connection_state".to_string(),
                    ConfigValue::Text(
                        EvConnectionState::from(*state).to_string(),
                    ),
                );
            }
            let config = EquipmentConfig {
                name: ev.name.clone(),
                ochre_class: "EV".to_string(),
                raw_config,
            };
            let mut eq = self
                .equipment_registry
                .create("EV", config)
                .map_err(to_py_err)?;
            if let Some(ref lut) = ev.charging_curve_lut {
                eq.set_charging_curve_lut(Some(lut.clone()))
                    .map_err(to_py_err)?;
            }
            return Ok(eq);
        }
        Err(PyValueError::new_err(
            "equipment must be a Battery, PV, or EV instance",
        ))
    }
}

#[pyclass(name = "TimestepsIter")]
pub struct PyTimestepsIter {
    start: DateTime<FixedOffset>,
    time_res_s: i64,
    next_step: u64,
    total_steps: u64,
}

#[pymethods]
impl PyTimestepsIter {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&mut self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
        if self.next_step >= self.total_steps {
            return Ok(None);
        }

        let ts = self.start
            + Duration::seconds(
                self.time_res_s
                    .saturating_mul(i64::try_from(self.next_step).unwrap_or(i64::MAX)),
            );
        self.next_step = self.next_step.saturating_add(1);
        Ok(Some(chrono_to_py_datetime(py, ts)?))
    }

    fn __repr__(&self) -> String {
        format!(
            "TimestepsIter(step={}/{})",
            self.next_step, self.total_steps
        )
    }
}

fn build_config(
    hpxml: String,
    schedule: String,
    weather: String,
    kwargs: Option<&Bound<'_, PyDict>>,
) -> PyResult<DwellingConfig> {
    let kwargs = kwargs.cloned();

    let py_config: Option<PySimulationConfig> = if let Some(k) = kwargs.as_ref() {
        if let Ok(Some(value)) = k.get_item("config") {
            Some(value.extract::<PySimulationConfig>()?)
        } else {
            None
        }
    } else {
        None
    };

    let sim_config = if let Some(py_cfg) = py_config {
        py_cfg.to_sim_config()?
    } else {
        let start_time = kwargs
            .as_ref()
            .and_then(|k| k.get_item("start_time").ok().flatten())
            .map(|obj| extract_datetime(&obj))
            .transpose()?
            .unwrap_or_else(default_start);

        let time_res = Duration::seconds(
            kwargs
                .as_ref()
                .and_then(|k| k.get_item("time_res_s").ok().flatten())
                .map(|obj| extract_seconds(&obj))
                .transpose()?
                .unwrap_or(DEFAULT_STEP_S),
        );

        let duration = Duration::seconds(
            kwargs
                .as_ref()
                .and_then(|k| k.get_item("duration_s").ok().flatten())
                .map(|obj| extract_seconds(&obj))
                .transpose()?
                .unwrap_or(DEFAULT_DURATION_S),
        );

        let output_to_parquet = kwargs
            .as_ref()
            .and_then(|k| k.get_item("output_to_parquet").ok().flatten())
            .map(|obj| obj.extract::<bool>())
            .transpose()?
            .unwrap_or(false);

        let output_path = kwargs
            .as_ref()
            .and_then(|k| k.get_item("output_path").ok().flatten())
            .map(|obj| obj.extract::<String>().map(PathBuf::from))
            .transpose()?;

        let output_verbosity = kwargs
            .as_ref()
            .and_then(|k| k.get_item("output_verbosity").ok().flatten())
            .map(|obj| obj.extract::<u8>())
            .transpose()?
            .unwrap_or(0);

        let output_chunk_size = kwargs
            .as_ref()
            .and_then(|k| k.get_item("output_chunk_size").ok().flatten())
            .map(|obj| obj.extract::<usize>())
            .transpose()?
            .unwrap_or(10_000);

        let master_seed = kwargs
            .as_ref()
            .and_then(|k| k.get_item("master_seed").ok().flatten())
            .map(|obj| obj.extract::<u64>())
            .transpose()?
            .unwrap_or(0);

        let civil_timezone = kwargs
            .as_ref()
            .and_then(|k| k.get_item("civil_timezone").ok().flatten())
            .map(|obj| obj.extract::<String>())
            .transpose()?;

        let setpoint_deadband_c = kwargs
            .as_ref()
            .and_then(|k| k.get_item("setpoint_deadband_c").ok().flatten())
            .map(|obj| obj.extract::<f64>())
            .transpose()?;

        SimulationConfig {
            start_time,
            duration,
            time_res,
            output_verbosity,
            output_path,
            output_format: if output_to_parquet {
                OutputFormat::Parquet
            } else {
                OutputFormat::Csv
            },
            output_chunk_size,
            setpoint_deadband_c,
            master_seed,
            civil_timezone,
        }
    };

    let bldg_id = kwargs
        .as_ref()
        .and_then(|k| k.get_item("bldg_id").ok().flatten())
        .map(|obj| obj.extract::<i64>())
        .transpose()?
        .unwrap_or(0);

    let initialization_duration = kwargs
        .as_ref()
        .and_then(|k| k.get_item("initialization_duration").ok().flatten())
        .map(|obj| {
            let secs = extract_seconds(&obj)?;
            if secs < 0 {
                return Err(PyValueError::new_err(
                    "initialization_duration must be non-negative",
                ));
            }
            Ok(secs)
        })
        .transpose()?
        .and_then(|secs| {
            if secs == 0 {
                None
            } else {
                Some(std::time::Duration::from_secs(secs as u64))
            }
        });

    let defaults_path = kwargs
        .as_ref()
        .and_then(|k| k.get_item("defaults_path").ok().flatten())
        .map(|obj| obj.extract::<String>())
        .transpose()?
        .map(PathBuf::from);

    Ok(DwellingConfig {
        hpxml_path: PathBuf::from(hpxml),
        schedule_path: PathBuf::from(schedule),
        weather_path: PathBuf::from(weather),
        defaults_path,
        sim_config,
        overrides: None,
        bldg_id,
        initialization_duration,
        resample_overrides: None,
    })
}

fn dict_optional<T>(d: &Bound<'_, PyDict>, key: &str) -> PyResult<Option<T>>
where
    T: for<'a, 'py> FromPyObject<'a, 'py>,
{
    let Some(value) = d.get_item(key)? else {
        return Ok(None);
    };
    if value.is_none() {
        Ok(None)
    } else {
        Ok(Some(value.extract::<T>().map_err(|_| {
            PyValueError::new_err(format!("invalid value for `{key}`"))
        })?))
    }
}

fn extract_seconds(obj: &Bound<'_, PyAny>) -> PyResult<i64> {
    if let Ok(v) = obj.extract::<i64>() {
        return Ok(v);
    }

    if let Ok(v) = obj.extract::<f64>() {
        return Ok(v.round() as i64);
    }

    if let Ok(seconds) = obj.getattr("total_seconds")?.call0()?.extract::<f64>() {
        return Ok(seconds.round() as i64);
    }

    Err(PyValueError::new_err(
        "expected seconds as int/float or datetime.timedelta",
    ))
}

fn default_start() -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(DEFAULT_START).expect("DEFAULT_START is a valid RFC3339 constant")
}

fn chrono_to_py_datetime(py: Python<'_>, dt: DateTime<FixedOffset>) -> PyResult<Py<PyAny>> {
    let datetime = py.import("datetime")?.getattr("datetime")?;
    let obj = datetime.call_method1("fromisoformat", (dt.to_rfc3339(),))?;
    Ok(obj.unbind())
}

fn to_py_err<E: std::fmt::Display>(err: E) -> PyErr {
    PyValueError::new_err(err.to_string())
}

#[cfg(feature = "observe")]
fn snapshot_to_py(
    py: Python<'_>,
    snap: &hares_core::observer::StepSnapshot,
) -> PyResult<Py<PyAny>> {
    let dict = PyDict::new(py);
    dict.set_item("step_index", snap.step_index)?;
    dict.set_item("timestamp", snap.timestamp.to_rfc3339())?;

    let phases = &snap.phases;

    if let Some(env) = &phases.post_environment {
        let d = PyDict::new(py);
        d.set_item("outdoor_temp_c", env.outdoor_temp_c)?;
        d.set_item("ghi_w_m2", env.ghi_w_m2)?;
        d.set_item("wind_speed_m_s", env.wind_speed_m_s)?;
        d.set_item("mains_temp_c", env.mains_temp_c)?;
        d.set_item(
            "zone_temps_c",
            env.zone_temps_c
                .iter()
                .map(|(z, t)| (z.0, *t))
                .collect::<Vec<_>>(),
        )?;
        d.set_item(
            "zone_humidity_ratios",
            env.zone_humidity_ratios
                .iter()
                .map(|(z, h)| (z.0, *h))
                .collect::<Vec<_>>(),
        )?;
        dict.set_item("post_environment", d)?;
    }

    for (key, phase) in [
        (
            "post_nonthermal_equipment",
            &phases.post_nonthermal_equipment,
        ),
        ("post_thermal_equipment", &phases.post_thermal_equipment),
    ] {
        if let Some(eq_phase) = phase {
            let d = PyDict::new(py);
            let eq_list: Vec<Py<PyAny>> = eq_phase
                .equipment
                .iter()
                .map(|obs| {
                    let ed = PyDict::new(py);
                    ed.set_item("name", &obs.name)?;
                    ed.set_item("equipment_type", &obs.equipment_type)?;
                    ed.set_item("end_use", format!("{:?}", obs.end_use))?;
                    let telem: Vec<(&String, &f64)> = obs.telemetry.0.iter().collect();
                    let td = PyDict::new(py);
                    for (k, v) in telem {
                        td.set_item(k, v)?;
                    }
                    ed.set_item("telemetry", td)?;
                    Ok(ed.unbind().into())
                })
                .collect::<PyResult<Vec<_>>>()?;
            d.set_item("equipment", eq_list)?;

            let ports = &eq_phase.ports;
            let pd = PyDict::new(py);
            pd.set_item(
                "thermal",
                ports
                    .thermal
                    .iter()
                    .map(|(z, s, l)| (z.0, *s, *l))
                    .collect::<Vec<_>>(),
            )?;
            pd.set_item("electrical_load_kw", ports.electrical_load_kw)?;
            pd.set_item("electrical_gen_kw", ports.electrical_gen_kw)?;
            pd.set_item("electrical_reactive_kvar", ports.electrical_reactive_kvar)?;
            pd.set_item(
                "fuel_consumption_w",
                ports
                    .fuel_consumption_w
                    .iter()
                    .map(|(ft, v)| (format!("{ft:?}"), *v))
                    .collect::<Vec<_>>(),
            )?;
            d.set_item("ports", pd)?;
            dict.set_item(key, d)?;
        }
    }

    if let Some(solvers) = &phases.post_solvers {
        let d = PyDict::new(py);
        let gains = &solvers.envelope_gains;
        d.set_item("window_solar_w", gains.window_solar_w)?;
        d.set_item("opaque_solar_lwr_w", gains.opaque_solar_lwr_w)?;
        d.set_item("interior_lwr_w", gains.interior_lwr_w)?;
        d.set_item("infiltration_w", gains.infiltration_w)?;
        d.set_item("ventilation_w", gains.ventilation_w)?;
        d.set_item("natural_ventilation_w", gains.natural_ventilation_w)?;
        d.set_item("port_sensible_w", gains.port_sensible_w)?;
        d.set_item("internal_gain_w", gains.internal_gain_w)?;
        dict.set_item("post_solvers", d)?;
    }

    if let Some(zu) = &phases.post_zone_update {
        let d = PyDict::new(py);
        d.set_item(
            "zone_temps_c",
            zu.zone_temps_c
                .iter()
                .map(|(z, t)| (z.0, *t))
                .collect::<Vec<_>>(),
        )?;
        d.set_item(
            "zone_humidity_ratios",
            zu.zone_humidity_ratios
                .iter()
                .map(|(z, h)| (z.0, *h))
                .collect::<Vec<_>>(),
        )?;
        dict.set_item("post_zone_update", d)?;
    }

    Ok(dict.unbind().into())
}

fn py_to_config_value(value: &Bound<'_, PyAny>) -> PyResult<ConfigValue> {
    // Bool must be checked before f64 because Python bool extracts as f64 (True → 1.0)
    if let Ok(b) = value.extract::<bool>() {
        return Ok(ConfigValue::Bool(b));
    }
    if let Ok(f) = value.extract::<f64>() {
        return Ok(ConfigValue::Float(f));
    }
    if let Ok(s) = value.extract::<String>() {
        return Ok(ConfigValue::Text(s));
    }
    if let Ok(arr) = value.extract::<Vec<f64>>() {
        return Ok(ConfigValue::FloatArray(arr));
    }
    Err(PyValueError::new_err(
        "config value must be bool, float, string, or list of floats",
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::MutexGuard;
    use std::thread;

    use super::default_start;
    use crate::utils::parse_datetime_str;

    #[derive(Debug)]
    struct TestDwelling {
        _data: i32,
    }

    fn lock_dwelling_test<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, String> {
        mutex.lock().map_err(|_| {
            "dwelling state is corrupted (internal panic occurred); \
             create a new Dwelling instance"
                .to_string()
        })
    }

    #[test]
    fn test_lock_dwelling_string_returns_error_on_poisoned_mutex() {
        let mutex: Mutex<TestDwelling> = Mutex::new(TestDwelling { _data: 0 });
        let mutex_static: &'static Mutex<TestDwelling> = Box::leak(Box::new(mutex));

        thread::spawn(move || {
            let _guard = mutex_static.lock().unwrap();
            panic!("intentional panic to poison mutex");
        })
        .join()
        .ok();

        let result = lock_dwelling_test(mutex_static);
        assert!(result.is_err(), "Expected error on poisoned mutex");
        let error_message = result.unwrap_err();
        assert!(
            error_message.contains("corrupted"),
            "Error should mention 'corrupted'"
        );
        assert!(
            error_message.contains("create a new Dwelling"),
            "Error should mention 'create a new Dwelling'"
        );
    }

    #[test]
    fn test_parse_datetime_str_rfc3339_with_z_suffix() {
        let result = parse_datetime_str("2019-01-01T00:00:00Z");
        assert!(result.is_ok());
        let dt = result.unwrap();
        assert_eq!(
            dt.format("%Y-%m-%dT%H:%M:%S").to_string(),
            "2019-01-01T00:00:00"
        );
    }

    #[test]
    fn test_parse_datetime_str_rfc3339_with_positive_offset() {
        let result = parse_datetime_str("2019-01-01T00:00:00+05:00");
        assert!(result.is_ok());
        let dt = result.unwrap();
        assert_eq!(
            dt.format("%Y-%m-%dT%H:%M:%S%z").to_string(),
            "2019-01-01T00:00:00+0500"
        );
    }

    #[test]
    fn test_parse_datetime_str_rfc3339_with_negative_offset() {
        let result = parse_datetime_str("2019-06-15T12:30:00-08:00");
        assert!(result.is_ok());
        let dt = result.unwrap();
        assert_eq!(
            dt.format("%Y-%m-%dT%H:%M:%S%z").to_string(),
            "2019-06-15T12:30:00-0800"
        );
    }

    #[test]
    fn test_parse_datetime_str_naive_fallback_assumes_utc() {
        let result = parse_datetime_str("2019-01-01T00:00:00");
        assert!(result.is_ok());
        let dt = result.unwrap();
        assert_eq!(
            dt.format("%Y-%m-%dT%H:%M:%S").to_string(),
            "2019-01-01T00:00:00"
        );
    }

    #[test]
    fn test_parse_datetime_str_rejects_invalid_string() {
        let result = parse_datetime_str("not-a-date");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_datetime_str_rejects_invalid_month() {
        let result = parse_datetime_str("2019-13-01T00:00:00");
        assert!(result.is_err());
    }

    #[test]
    fn test_default_start_returns_expected_value() {
        let result = default_start();
        assert_eq!(
            result.format("%Y-%m-%dT%H:%M:%S%z").to_string(),
            "2019-01-01T00:00:00+0000"
        );
    }
}
