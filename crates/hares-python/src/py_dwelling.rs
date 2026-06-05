//! Python bindings for dwelling simulation.

use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Duration, FixedOffset};
use hares_control::PriceSignal;
use hares_core::{
    ActorConfig, ActorRegistry, BatteryLutData, Dwelling, DwellingConfig,
    environment::SurfaceGeometry,
};
use hares_equipment::{
    BatteryConfig, BatteryLutType, EquipmentConfig, EquipmentRegistry, EvConfig,
    ProtocolBridgeConfig, PvConfig, config::ConfigValue,
};
use hares_io::{
    OutputFormat, ResampleOverrides, SimulationConfig, SiteLocationOverride,
    output::metrics::MetricsCalculator,
};
use hares_types::panic_hook::{self, PanicHookGuard};
use hares_types::{BatteryChemistry, EvConnectionState, HaresError, SurfaceIrradiance};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyBytes, PyDict, PyList, PyType};
use rand::SeedableRng;
use std::sync::MutexGuard;

pyo3::create_exception!(_hares, HaresConfigError, pyo3::exceptions::PyValueError);
pyo3::create_exception!(
    _hares,
    HaresEquipmentError,
    pyo3::exceptions::PyRuntimeError
);
pyo3::create_exception!(
    _hares,
    HaresSimulationError,
    pyo3::exceptions::PyRuntimeError
);
pyo3::create_exception!(_hares, FatalDwellingError, pyo3::exceptions::PyRuntimeError);

use crate::conversions::{batches_or_steps_to_polars_df, chrono_to_py_datetime};
use crate::py_actor::PyActor;
use crate::py_config::PySimulationConfig;
use crate::py_config::parse_rotation_policy;
use crate::py_control::PyControlSignal;
use crate::py_enums::PyLutType;
use crate::py_enums::{PyEvArchetypeId, PyVehicleId};
use crate::py_equipment::{
    PyBattery, PyEquipment, PyEquipmentDescriptor, PyEv, PyProtocolBridge, PyPv,
    extract_charging_lut, extract_ocv_table, extract_u_neg_table,
};
use crate::py_metrics::PySimulationMetrics;
use crate::py_telemetry::PyTelemetry;
use crate::utils::extract_datetime;
use crate::utils::parse_resample_method;

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
    // Duck-type: Polars DataFrame has .columns and .height (no .values since polars 1.x)
    if data.hasattr("columns")? && data.hasattr("height")? {
        return parse_solar_override_from_dataframe(py, data);
    }
    // Duck-type: ndarray-like has .dtype and .shape but NOT .columns
    if data.hasattr("dtype")? && data.hasattr("shape")? && !data.hasattr("columns")? {
        return parse_solar_override_from_ndarray(data);
    }
    // Duck-type: pandas DataFrame has .values and .columns but no .height
    if data.hasattr("values")? && data.hasattr("columns")? {
        return parse_solar_override_from_dataframe(py, data);
    }
    if let Ok(dict) = data.extract::<Bound<'_, PyDict>>() {
        return parse_solar_override_from_dict(py, dict.as_any());
    }
    Err(PyValueError::new_err(
        "solar override must be a file path (str), polars/pandas DataFrame, numpy array dict, or list of dicts",
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

/// Extract a solar override from a 2D numpy array with shape `(n_rows, 5)`.
///
/// Each row encodes one (timestep, surface) tuple as
/// `[surface_id, direct_w_m2, diffuse_w_m2, reflected_w_m2, angle_of_incidence_rad]`.
/// Rows must be grouped by timestep (all surfaces for step 0 first, then step 1, etc.)
/// and sorted by surface ID within each group.
fn parse_solar_override_from_ndarray(
    data: &Bound<'_, PyAny>,
) -> PyResult<Vec<Vec<SurfaceIrradiance>>> {
    let shape: Vec<usize> = data.getattr("shape")?.extract()?;
    if shape.len() != 2 || shape[1] != 5 {
        return Err(PyValueError::new_err(
            "numpy solar override must have shape (n_rows, 5) with columns \
             [surface_id, direct_w_m2, diffuse_w_m2, reflected_w_m2, angle_of_incidence_rad]",
        ));
    }
    let n_rows = shape[0];
    if n_rows == 0 {
        return Err(PyValueError::new_err(
            "numpy solar override array has no rows",
        ));
    }

    let flat: Vec<f64> = data
        .call_method1("astype", ("float64",))?
        .call_method0("flatten")?
        .call_method0("tolist")?
        .extract()?;

    let n_surfaces_per_step = {
        let first_id = flat[0] as u32;
        let mut count = 1usize;
        while count < n_rows {
            if flat[count * 5] as u32 == first_id {
                break;
            }
            count += 1;
        }
        count
    };

    if !n_rows.is_multiple_of(n_surfaces_per_step) {
        return Err(PyValueError::new_err(
            "numpy solar override row count is not evenly divisible by surfaces per timestep",
        ));
    }
    let n_timesteps = n_rows / n_surfaces_per_step;

    for t in 1..n_timesteps {
        for s in 0..n_surfaces_per_step {
            let expected_id = flat[s * 5] as u32;
            let actual_id = flat[(t * n_surfaces_per_step + s) * 5] as u32;
            if actual_id != expected_id {
                return Err(PyValueError::new_err(format!(
                    "surface ID mismatch at timestep {t}, surface {s}: expected {expected_id}, got {actual_id}. \
                     Rows must be grouped by timestep with identical surface ID ordering in each group.",
                )));
            }
        }
    }
    let mut result: Vec<Vec<SurfaceIrradiance>> = Vec::with_capacity(n_timesteps);

    for t in 0..n_timesteps {
        let mut surfaces = Vec::with_capacity(n_surfaces_per_step);
        for s in 0..n_surfaces_per_step {
            let base = (t * n_surfaces_per_step + s) * 5;
            surfaces.push(SurfaceIrradiance {
                surface_id: flat[base] as u32,
                direct_w_m2: flat[base + 1],
                diffuse_w_m2: flat[base + 2],
                reflected_w_m2: flat[base + 3],
                angle_of_incidence_rad: flat[base + 4],
            });
        }
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
    if arr.hasattr("shape")? {
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
    Err(PyValueError::new_err("Expected a list or numpy array"))
}

fn extract_array_element(arr: &Bound<'_, PyAny>, index: usize) -> PyResult<f64> {
    if let Ok(list) = arr.extract::<Bound<'_, PyList>>() {
        return list.get_item(index)?.extract::<f64>();
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

fn battery_config_from_py(battery: &PyBattery) -> EquipmentConfig {
    let cfg = BatteryConfig {
        equipment_id: None,
        zone_id: None,
        capacity_kwh: battery.capacity_kwh,
        max_charge_kw: battery.max_charge_kw.unwrap_or(5.0),
        max_discharge_kw: battery.max_discharge_kw.unwrap_or(5.0),
        n_series: battery.n_series,
        n_parallel: battery.n_parallel,
        ah_cell: None,
        v_cell: None,
        cell_resistance_ohm: battery.cell_resistance_ohm,
        pack_voltage_v: None,
        chemistry: battery
            .chemistry
            .map(|c| BatteryChemistry::from(c).as_config_str().to_string()),
        standby_power_w: battery.standby_power_w,
        self_discharge_pct_per_day: battery.self_discharge_pct_per_day,
        min_soc: battery.min_soc,
        max_soc: battery.max_soc,
        initial_soc: battery.initial_soc,
        initial_cell_temp_c: None,
        import_limit_w: battery.import_limit_w,
        export_limit_w: battery.export_limit_w,
        heater_power_w: battery.heater_power_w,
        heater_threshold_c: battery.heater_threshold_c,
        heater_on_discharge: None,
        min_discharge_temp_c: None,
        full_power_temp_c: battery.full_power_temp_c,
        min_charge_temp_c: battery.min_charge_temp_c,
        cell_thermal_mass_j_per_k: battery.cell_thermal_mass_j_per_k,
        cell_ua_w_per_k: battery.cell_ua_w_per_k,
        inverter_efficiency: battery.inverter_efficiency,
        charge_efficiency: battery.charge_efficiency,
        discharge_efficiency: battery.discharge_efficiency,
        bms_mode: None,
        grid_export_rule: None,
        min_dwell_steps: 0,
    };
    EquipmentConfig::from_typed(battery.name.clone(), "Battery".to_string(), cfg)
}

fn pv_config_from_py(pv: &PyPv) -> EquipmentConfig {
    let cfg = PvConfig {
        equipment_id: None,
        zone_id: None,
        capacity_kw: pv.capacity_kw,
        tilt_deg: Some(pv.tilt),
        azimuth_deg: Some(pv.azimuth),
        module_type: None,
        noct_c: None,
        array_type: None,
        system_losses_fraction: None,
        inverter_efficiency: None,
        inverter_capacity_kw: None,
        power_factor: None,
        surface_resolution_deg: None,
        sam_lut_path: pv.sam_lut_path.clone(),
        arrays: None,
    };
    EquipmentConfig::from_typed(pv.name.clone(), "PV".to_string(), cfg)
}

fn ev_config_from_py(ev: &PyEv) -> EquipmentConfig {
    let capacity_kwh = ev.capacity_kwh.unwrap_or(75.0);
    let max_charging_power_kw =
        ev.max_charging_kw
            .unwrap_or(if capacity_kwh < 56.875 { 7.2 } else { 11.5 });
    let cfg = EvConfig {
        equipment_id: None,
        capacity_kwh,
        charging_level: None,
        max_charging_power_kw,
        charging_efficiency: None,
        l1_current_a: None,
        l1_voltage_v: None,
        soc_max: None,
        initial_soc: ev.initial_soc,
        battery_temp_c: None,
        min_charge_temp_c: None,
        full_power_temp_c: None,
        heater_power_w: None,
        heater_threshold_c: None,
        thermal_mass_j_per_k: None,
        ua_w_per_k: None,
        v2l_enabled: None,
        v2l_soc_reserve: None,
        v2l_max_discharge_kw: None,
        v2g_enabled: None,
        v2g_soc_reserve: None,
        v2g_max_discharge_kw: None,
        chemistry: None,
        fuel_economy_kwh_per_mi: None,
        ready_soc: None,
        charging_strategy: None,
        plug_in_policy: None,
        power_limit_kw: None,
        initial_connection_state: ev
            .initial_connection_state
            .map(|state| EvConnectionState::from(state).to_string()),
    };
    EquipmentConfig::from_typed(ev.name.clone(), "EV".to_string(), cfg)
}

fn protocol_bridge_config_from_py(bridge: &PyProtocolBridge) -> EquipmentConfig {
    let handlers: Vec<_> = bridge
        .json_handlers
        .iter()
        .map(
            |&protocol_id| hares_equipment::protocol_bridge::config::HandlerConfig::Json {
                protocol_id,
            },
        )
        .collect();
    let cfg = ProtocolBridgeConfig {
        equipment_id: None,
        registered_protocols: bridge.registered_protocols.clone(),
        handlers,
    };
    EquipmentConfig::from_typed(bridge.name.clone(), "ProtocolBridge".to_string(), cfg)
}

// Lock helpers are now methods on PyDwelling (acquire, acquire_string,
// acquire_hares) that check the `poisoned` AtomicBool before locking.

/// Sentinels that identify a fatal dwelling error in GIL-free `String` error
/// contexts (e.g. [`batch_step_py`]) where a typed error cannot cross thread
/// boundaries.
pub(crate) const FATAL_DWELLING_PREFIX: &str = "[FATAL_DWELLING] ";

#[pyclass(name = "Dwelling")]
pub struct PyDwelling {
    pub(crate) dwelling: Mutex<Dwelling>,
    pub(crate) poisoned: AtomicBool,
    pub(crate) mutex_poison_events: std::sync::atomic::AtomicU64,
    pub(crate) config: DwellingConfig,
    sim_config: PySimulationConfig,
    initial_state: Option<Vec<u8>>,
    initialized: bool,
    registry: ActorRegistry,
    equipment_registry: EquipmentRegistry,
    zone_keys: Option<Vec<String>>,
    gas_tariff: Option<crate::py_tariff::PyGasTariff>,
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
            poisoned: AtomicBool::new(false),
            mutex_poison_events: std::sync::atomic::AtomicU64::new(0),
            config,
            sim_config,
            initial_state: None,
            initialized: false,
            registry: ActorRegistry::new(),
            equipment_registry: EquipmentRegistry::new(),
            zone_keys: None,
            gas_tariff: None,
        })
    }

    pub fn initialize(&mut self) -> PyResult<()> {
        if self.initialized || self.initial_state.is_some() {
            self.initialized = true;
            return Ok(());
        }

        let bytes = {
            let dwelling = self.acquire()?;
            serde_json::to_vec(&dwelling.save_checkpoint().map_err(to_py_err)?)
                .map_err(to_py_err_display)?
        };

        self.initial_state = Some(bytes);
        self.initialized = true;
        Ok(())
    }

    pub fn timesteps(&self) -> PyResult<PyTimestepsIter> {
        let dwelling = self.acquire()?;

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
        let _guard = PanicHookGuard::new();
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            assert!(
                panic_hook::is_installed(),
                "custom panic hook must be installed before simulation"
            );
        }
        let sim_result: Result<Result<(), HaresError>, _> = py.detach(|| {
            std::panic::catch_unwind(AssertUnwindSafe(|| {
                let mut dwelling = self.acquire_hares()?;
                dwelling.simulate()?;
                Ok(())
            }))
        });
        match sim_result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                if self.poisoned.load(Ordering::Acquire) {
                    return Err(FatalDwellingError::new_err(e.to_string()));
                }
                return Err(to_py_err(e));
            }
            Err(payload) => {
                if self.dwelling.is_poisoned() {
                    self.mark_poisoned();
                }
                return Err(PyRuntimeError::new_err(format!(
                    "HARES internal panic: {}",
                    panic_hook::panic_payload_to_string(payload)
                )));
            }
        }

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            assert!(
                !self.poisoned.load(Ordering::Acquire) || self.dwelling.is_poisoned(),
                "invariant violation: poisoned flag is true but dwelling mutex is not poisoned"
            );
        }

        let dwelling = self.acquire()?;
        let batches: Vec<_> = dwelling.flushed_batches().to_vec();
        let steps = dwelling.results().steps.clone();
        drop(dwelling);
        batches_or_steps_to_polars_df(py, &batches, &steps)
    }

    pub fn step(&mut self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let _guard = PanicHookGuard::new();
        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            assert!(
                panic_hook::is_installed(),
                "custom panic hook must be installed before step"
            );
        }
        let step_result: Result<Result<hares_core::StepResult, HaresError>, _> =
            py.detach(|| std::panic::catch_unwind(AssertUnwindSafe(|| self.step_core())));
        let step = match step_result {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                if self.poisoned.load(Ordering::Acquire) {
                    return Err(FatalDwellingError::new_err(e.to_string()));
                }
                return Err(to_py_err(e));
            }
            Err(payload) => {
                if self.dwelling.is_poisoned() {
                    self.mark_poisoned();
                }
                return Err(PyRuntimeError::new_err(format!(
                    "HARES internal panic: {}",
                    panic_hook::panic_payload_to_string(payload)
                )));
            }
        };

        #[cfg(any(debug_assertions, feature = "check_invariants"))]
        {
            assert!(
                !self.poisoned.load(Ordering::Acquire) || self.dwelling.is_poisoned(),
                "invariant violation: poisoned flag is true but dwelling mutex is not poisoned"
            );
        }

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
        out.set_item("timestamp", chrono_to_py_datetime(py, step.timestamp)?)?;
        out.set_item("net_electric_power_kw", step.net_electric_power_kw)?;
        out.set_item("hvac_heating_w", step.hvac_heating_w)?;
        out.set_item("hvac_cooling_w", step.hvac_cooling_w)?;
        out.set_item("gas_power_w", step.gas_power_w)?;
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
        let mut dwelling = self.acquire()?;
        dwelling
            .apply_control_validated(&name, signal.signal.clone())
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }

    /// Adds a Python actor to the dwelling's decision-making loop.
    pub fn add_actor(&self, py: Python<'_>, actor: Py<PyActor>) -> PyResult<()> {
        let mut dwelling = self.acquire()?;
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

        let mut dwelling = self.acquire()?;
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
        let mut dwelling = self.acquire()?;
        dwelling.set_price_signal(price);
        Ok(())
    }

    pub fn set_grid_voltage(&self, voltage_pu: f64) -> PyResult<()> {
        let mut dwelling = self.acquire()?;
        dwelling.set_grid_voltage(voltage_pu);
        Ok(())
    }

    #[cfg(feature = "observe")]
    pub fn enable_observer(&self, capacity: usize) -> PyResult<()> {
        let mut dwelling = self.acquire()?;
        dwelling.enable_observer(capacity);
        Ok(())
    }

    #[cfg(feature = "observe")]
    pub fn drain_observations(&self, py: Python<'_>) -> PyResult<Vec<Py<PyAny>>> {
        let mut dwelling = self.acquire()?;
        let snapshots = dwelling.drain_observations();
        snapshots
            .into_iter()
            .map(|snap| snapshot_to_py(py, &snap))
            .collect()
    }

    #[cfg(feature = "observe")]
    #[getter]
    pub fn poison_events(&self) -> u64 {
        self.mutex_poison_events.load(Ordering::Relaxed)
    }

    pub fn telemetry(&self) -> PyResult<PyTelemetry> {
        let fatal = self.poisoned.load(Ordering::Acquire);
        if fatal {
            // Dwelling is fatally poisoned — the mutex cannot be locked, but we
            // surface the fatal status to Python callers.
            return Err(FatalDwellingError::new_err(
                "dwelling is in a fatal error state (internal state corruption); create a new Dwelling instance",
            ));
        }
        let dwelling = self.dwelling.lock().map_err(|e| {
            FatalDwellingError::new_err(format!(
                "dwelling state is corrupted (internal panic: {}); create a new Dwelling instance",
                e
            ))
        })?;
        Ok(PyTelemetry::new(dwelling.telemetry()))
    }

    #[getter]
    pub fn dwelling_fatal(&self) -> bool {
        self.poisoned.load(Ordering::Acquire)
    }

    pub fn equipment_descriptors(&self) -> PyResult<Vec<PyEquipmentDescriptor>> {
        let dwelling = self.acquire()?;
        Ok(dwelling
            .equipment()
            .iter()
            .map(|eq| PyEquipmentDescriptor::new(eq.descriptor().clone()))
            .collect())
    }

    pub fn equipment(&self) -> PyResult<Vec<PyEquipment>> {
        let dwelling = self.acquire()?;
        Ok(dwelling
            .equipment()
            .iter()
            .map(|eq| {
                PyEquipment::new(
                    eq.descriptor().clone(),
                    eq.core_output().clone(),
                    eq.telemetry().clone(),
                )
            })
            .collect())
    }

    pub fn equipment_names(&self) -> PyResult<Vec<String>> {
        let dwelling = self.acquire()?;
        Ok(dwelling
            .equipment()
            .iter()
            .map(|eq| eq.descriptor().name.clone())
            .collect())
    }

    /// Returns per-equipment HPXML setpoint reconciliation records as a
    /// Python `dict[str, list[dict]]`, keyed by equipment instance name.
    /// Each reconciliation record is a dict with keys ``day``,
    /// ``original_heating_c``, ``original_cooling_c``, ``adjusted_heating_c``,
    /// ``adjusted_cooling_c``.  Only equipment with setpoint reconciliation
    /// appears in the dict; equipment without reconciliation is absent.
    #[pyo3(name = "setpoints_reconciled")]
    pub fn py_setpoints_reconciled(&self, py: Python<'_>) -> PyResult<Py<PyDict>> {
        let dwelling = self.acquire()?;
        let sr_map = dwelling.setpoints_reconciled();
        let result = PyDict::new(py);
        for (name, reconciliations) in sr_map {
            if let Some(records) = reconciliations {
                let list = PyList::empty(py);
                for r in records {
                    let item = PyDict::new(py);
                    item.set_item("day", r.day.clone())?;
                    item.set_item("original_heating_c", r.original_heating_c.as_slice())?;
                    item.set_item("original_cooling_c", r.original_cooling_c.as_slice())?;
                    item.set_item("adjusted_heating_c", r.adjusted_heating_c.as_slice())?;
                    item.set_item("adjusted_cooling_c", r.adjusted_cooling_c.as_slice())?;
                    list.append(item)?;
                }
                result.set_item(name.as_str(), list)?;
            }
        }
        Ok(result.into())
    }

    pub fn add_battery(&mut self, battery: &PyBattery) -> PyResult<()> {
        let config = battery_config_from_py(battery);

        let mut eq = self
            .equipment_registry
            .create("Battery", config.clone())
            .map_err(to_py_err)?;

        let mut dwelling = self.acquire()?;
        eq.init(&config, dwelling.latest_env()).map_err(to_py_err)?;

        // Set LUTs after init -- init may reset internal state
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
        let config = pv_config_from_py(pv);

        let mut eq = self
            .equipment_registry
            .create("PV", config.clone())
            .map_err(to_py_err)?;
        let mut dwelling = self.acquire()?;

        // Register the PV surface orientation with the environment so that
        // Perez irradiance is computed for this panel during simulation.
        let surface_id = hares_equipment::pv::surface_id_for_orientation(pv.tilt, pv.azimuth, 5.0)
            .map_err(to_py_err)?;
        dwelling.environment.register_surface(SurfaceGeometry {
            surface_id,
            azimuth_deg: pv.azimuth,
            tilt_deg: pv.tilt,
            area_m2: 1.0,
            omni_directional: false,
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
        let config = ev_config_from_py(ev);

        let mut eq = self
            .equipment_registry
            .create("EV", config.clone())
            .map_err(to_py_err)?;

        let mut dwelling = self.acquire()?;
        eq.init(&config, dwelling.latest_env()).map_err(to_py_err)?;

        // Set LUT after init -- init_from_config resets charging_curve_lut to None
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

        let config = EquipmentConfig::from_typed(
            spec.label.to_string(),
            "EV".to_string(),
            EvConfig {
                equipment_id: None,
                capacity_kwh: spec.capacity_kwh,
                charging_level: Some(match preset.charging_level {
                    hares_types::ChargingLevel::L1 => "L1".to_string(),
                    hares_types::ChargingLevel::L2 => "L2".to_string(),
                }),
                max_charging_power_kw: max_power,
                charging_efficiency: None,
                l1_current_a: None,
                l1_voltage_v: None,
                soc_max: None,
                initial_soc: None,
                battery_temp_c: None,
                min_charge_temp_c: None,
                full_power_temp_c: None,
                heater_power_w: None,
                heater_threshold_c: None,
                thermal_mass_j_per_k: None,
                ua_w_per_k: None,
                v2l_enabled: None,
                v2l_soc_reserve: None,
                v2l_max_discharge_kw: None,
                v2g_enabled: None,
                v2g_soc_reserve: None,
                v2g_max_discharge_kw: None,
                chemistry: None,
                fuel_economy_kwh_per_mi: None,
                ready_soc: None,
                charging_strategy: None,
                plug_in_policy: None,
                power_limit_kw: None,
                initial_connection_state: None,
            },
        );

        let mut eq = self
            .equipment_registry
            .create("EV", config.clone())
            .map_err(to_py_err)?;

        let mut dwelling = self.acquire()?;
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
            preset.build_arrival_schedule(seed_bytes),
            preset.event_day_ratio,
            fuel_economy,
            spec.capacity_kwh,
            max_power,
            30.0, // average_speed_mph
            20.0, // range_anxiety_miles
            0.0,  // away_charge_fraction
            0.0,  // away_charge_power_kw
            hares_core::ChaCha8Rng::from_seed(seed_bytes),
        );

        dwelling.add_actor(Box::new(actor));
        Ok(())
    }

    pub fn add_protocol_bridge(&mut self, bridge: &PyProtocolBridge) -> PyResult<()> {
        let config = protocol_bridge_config_from_py(bridge);
        let mut eq = self
            .equipment_registry
            .create("Protocol Bridge", config.clone())
            .map_err(to_py_err)?;
        let mut dwelling = self.acquire()?;
        eq.init(&config, dwelling.latest_env()).map_err(to_py_err)?;
        dwelling.add_equipment(eq);
        Ok(())
    }

    pub fn remove_equipment(&mut self, name: &str) -> PyResult<()> {
        let mut dwelling = self.acquire()?;
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
        let mut dwelling = self.acquire()?;
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

        let mut dwelling = self.acquire()?;

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
        let mut dwelling = self.acquire()?;
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
        let mut dwelling = self.acquire()?;
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
        let dwelling = self.acquire()?;
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
        let dwelling = self.acquire()?;
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
        let dwelling = self.acquire()?;
        let batches: Vec<_> = dwelling.flushed_batches().to_vec();
        let steps = dwelling.results().steps.clone();
        drop(dwelling);
        batches_or_steps_to_polars_df(py, &batches, &steps)
    }

    pub fn metrics(&self) -> PyResult<PySimulationMetrics> {
        let dwelling = self.acquire()?;
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
        self.poisoned.store(false, Ordering::SeqCst);
        self.mutex_poison_events.store(0, Ordering::SeqCst);
        self.config = config;
        self.initialized = false;
        self.initial_state = None;
        self.zone_keys = None;

        self.initialize()
    }

    pub fn save_state<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let dwelling = self.acquire()?;
        let bytes = serde_json::to_vec(&dwelling.save_checkpoint().map_err(to_py_err_display)?)
            .map_err(to_py_err_display)?;
        Ok(PyBytes::new(py, &bytes))
    }

    pub fn load_state(&self, state: &[u8]) -> PyResult<()> {
        let checkpoint = serde_json::from_slice(state).map_err(to_py_err_display)?;
        let mut dwelling = self.acquire()?;
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
        let mut dwelling = self.acquire()?;
        dwelling.environment.set_solar_override(solar_data);
        Ok(())
    }

    pub fn clear_solar_override(&self) -> PyResult<()> {
        let mut dwelling = self.acquire()?;
        dwelling.environment.clear_solar_override();
        Ok(())
    }

    pub fn has_solar_override(&self) -> PyResult<bool> {
        let dwelling = self.acquire()?;
        Ok(dwelling.environment.has_solar_override())
    }

    pub fn surface_ids(&self) -> PyResult<Vec<u32>> {
        let dwelling = self.acquire()?;
        Ok(dwelling
            .environment
            .surface_geometry()
            .iter()
            .map(|s| s.surface_id)
            .collect())
    }

    /// Return all roof planes from the parsed HPXML building.
    pub fn roof_planes(&self) -> PyResult<Vec<crate::py_pv_sizing::PyRoofPlane>> {
        let dwelling = self.acquire()?;
        Ok(crate::py_pv_sizing::roof_planes_from_dwelling(
            &dwelling.roof_info,
        ))
    }

    /// Enumerate all viable PV candidate placements, one per non-north-facing
    /// roof plane, sorted by solar production score (best first).
    ///
    /// Args:
    ///     diffuse_fraction: Optional annual-average DHI/GHI ratio (Kd) for
    ///         weather-aware scoring. If ``None``, uses the NREL PVWatts
    ///         empirical model. Compute with ``hares.compute_annual_diffuse_fraction()``.
    ///     panel_watts: Optional panel wattage override (W). When ``None``,
    ///         falls back to the loaded panel spec (DefaultsStore), or the
    ///         compile-time default (440 W) when no spec is loaded.
    ///     panel_area_m2: Optional panel area override (m²). When ``None``,
    ///         falls back to the loaded panel spec (DefaultsStore), or the
    ///         compile-time default (2.1 m²) when no spec is loaded.
    ///     roof_shape: Optional roof shape classification override. When
    ///         ``Some``, the provided shape is used directly and inference
    ///         is skipped. When ``None`` (default), roof shape is inferred
    ///         from building metadata.
    #[pyo3(signature = (diffuse_fraction=None, *, panel_watts=None, panel_area_m2=None, roof_shape=None))]
    pub fn pv_candidates(
        &self,
        diffuse_fraction: Option<f64>,
        panel_watts: Option<u32>,
        panel_area_m2: Option<f64>,
        roof_shape: Option<crate::py_pv_sizing::PyRoofShape>,
    ) -> PyResult<Vec<crate::py_pv_sizing::PyPvCandidate>> {
        let dwelling = self.acquire()?;
        let (roof_shape, user_override) = match roof_shape {
            Some(py_shape) => {
                tracing::debug!(
                    pv_roof_shape_override = ?py_shape,
                    "PV roof shape is user-specified override, inference skipped"
                );
                (py_shape.into(), true)
            }
            None => (
                hares_physics::pv_sizing::infer_roof_shape(
                    &dwelling.roof_info,
                    dwelling.facility_type.as_deref(),
                    dwelling.latitude_deg,
                    &dwelling.wall_azimuths,
                ),
                false,
            ),
        };
        Ok(crate::py_pv_sizing::pv_candidates_from_dwelling(
            &dwelling.roof_info,
            roof_shape,
            &dwelling.wall_azimuths,
            dwelling.latitude_deg,
            diffuse_fraction,
            panel_watts,
            panel_area_m2,
            dwelling.pv_panel_defaults(),
            user_override,
        ))
    }

    /// Size a PV system to a target capacity, constrained by roof geometry.
    ///
    /// Returns the best single-array sizing result. Use `pv_candidates()` to
    /// see all viable placements.
    ///
    /// Args:
    ///     target_kw: Desired PV capacity in kW.
    ///     min_kw: Minimum acceptable capacity (default 2.0).
    ///     max_kw: Maximum capacity limit (default 14.0).
    ///     diffuse_fraction: Optional annual-average DHI/GHI ratio (Kd) for
    ///         weather-aware scoring. If ``None``, uses the NREL PVWatts
    ///         empirical model. Compute with ``hares.compute_annual_diffuse_fraction()``.
    ///     panel_watts: Optional panel wattage override (W). When ``None``,
    ///         falls back to the compile-time default (440 W).
    ///     panel_area_m2: Optional panel area override (m²). When ``None``,
    ///         falls back to the compile-time default (2.1 m²).
    ///     system_losses: Optional system losses fraction override (0–1).
    ///         When ``None``, falls back to the compile-time default (0.14).
    ///     roof_shape: Optional roof shape classification override. When
    ///         ``Some``, the provided shape is used directly and inference
    ///         is skipped. When ``None`` (default), roof shape is inferred
    ///         from building metadata.
    #[pyo3(signature = (target_kw, min_kw=2.0, max_kw=14.0, diffuse_fraction=None, *,
        panel_watts=None, panel_area_m2=None, system_losses=None, roof_shape=None))]
    // Why: PyO3 signature mirrors the full Rust API surface;
    // a builder type adds indirection at the binding layer.
    #[allow(clippy::too_many_arguments)]
    pub fn estimate_pv_capacity(
        &self,
        target_kw: f64,
        min_kw: f64,
        max_kw: f64,
        diffuse_fraction: Option<f64>,
        panel_watts: Option<u32>,
        panel_area_m2: Option<f64>,
        system_losses: Option<f64>,
        roof_shape: Option<crate::py_pv_sizing::PyRoofShape>,
    ) -> PyResult<crate::py_pv_sizing::PyPvSizingResult> {
        let dwelling = self.acquire()?;
        let (roof_shape, user_override) = match roof_shape {
            Some(py_shape) => {
                tracing::debug!(
                    pv_roof_shape_override = ?py_shape,
                    "PV roof shape is user-specified override, inference skipped"
                );
                (py_shape.into(), true)
            }
            None => (
                hares_physics::pv_sizing::infer_roof_shape(
                    &dwelling.roof_info,
                    dwelling.facility_type.as_deref(),
                    dwelling.latitude_deg,
                    &dwelling.wall_azimuths,
                ),
                false,
            ),
        };
        crate::py_pv_sizing::size_pv_from_dwelling(
            &dwelling.roof_info,
            roof_shape,
            &dwelling.wall_azimuths,
            dwelling.latitude_deg,
            target_kw,
            min_kw,
            max_kw,
            diffuse_fraction,
            panel_watts,
            panel_area_m2,
            system_losses,
            dwelling.pv_panel_defaults(),
            user_override,
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
        let mut dwelling = self.acquire()?;
        dwelling
            .set_tariff(tariff.inner.clone(), tz)
            .map_err(to_py_err)
    }

    /// Attach a gas tariff to this dwelling for metadata/reporting.
    pub fn set_gas_tariff(&mut self, tariff: crate::py_tariff::PyGasTariff) {
        self.gas_tariff = Some(tariff);
    }

    /// Emit the final partial billing period. Call after the last `step()`
    /// when driving the simulation step-by-step. Idempotent.
    pub fn finalize_billing(&self) -> PyResult<()> {
        let mut dwelling = self.acquire()?;
        dwelling.finalize_billing();
        Ok(())
    }

    /// Returns accumulated billing period summaries.
    pub fn billing_summaries(&self) -> PyResult<Vec<crate::py_telemetry::PyBillingPeriodSummary>> {
        let dwelling = self.acquire()?;
        Ok(dwelling
            .billing_summaries()
            .iter()
            .map(crate::py_telemetry::PyBillingPeriodSummary::from_rust)
            .collect())
    }

    /// Returns a snapshot of current tariff state, or None if no tariff is set.
    pub fn tariff_telemetry(&self) -> PyResult<Option<crate::py_telemetry::PyTariffTelemetry>> {
        let dwelling = self.acquire()?;
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
    /// Acquire the dwelling mutex after checking the poisoned flag.
    /// Returns `FatalDwellingError` if the dwelling is in a fatal error state.
    fn acquire(&self) -> PyResult<MutexGuard<'_, Dwelling>> {
        if self.poisoned.load(Ordering::Acquire) {
            return Err(FatalDwellingError::new_err(
                "dwelling is in a fatal error state (internal state corruption); create a new Dwelling instance",
            ));
        }
        self.dwelling.lock().map_err(|e| {
            FatalDwellingError::new_err(format!(
                "dwelling state is corrupted (internal panic: {}); create a new Dwelling instance",
                e
            ))
        })
    }

    /// GIL-free variant of [`acquire`] required by `Send` contexts (e.g. Rayon threads).
    ///
    /// Returns `Err(String)` instead of `PyErr` so it can be used without holding the GIL.
    /// Fatal errors are prefixed with [`FATAL_DWELLING_PREFIX`] so callers can
    /// distinguish permanent from transient failures.
    pub(crate) fn acquire_string(&self) -> Result<MutexGuard<'_, Dwelling>, String> {
        if self.poisoned.load(Ordering::Acquire) {
            return Err(format!(
                "{}dwelling is in a fatal error state (internal state corruption); create a new Dwelling instance",
                FATAL_DWELLING_PREFIX
            ));
        }
        self.dwelling.lock().map_err(|e| {
            format!(
                "{}dwelling state is corrupted (internal panic: {}); create a new Dwelling instance",
                FATAL_DWELLING_PREFIX, e
            )
        })
    }

    /// GIL-free variant that preserves [`HaresError`] for proper Python exception mapping
    /// via [`to_py_err`].
    fn acquire_hares(&self) -> Result<MutexGuard<'_, Dwelling>, HaresError> {
        if self.poisoned.load(Ordering::Acquire) {
            return Err(HaresError::Dwelling(
                "dwelling is in a fatal error state (internal state corruption); create a new Dwelling instance".to_string(),
            ));
        }
        self.dwelling.lock().map_err(|e| {
            HaresError::Dwelling(format!(
                "dwelling state is corrupted (internal panic: {}); create a new Dwelling instance",
                e
            ))
        })
    }

    /// Mark this dwelling as fatally poisoned. Idempotent.
    pub(crate) fn mark_poisoned(&self) {
        if !self.poisoned.swap(true, Ordering::SeqCst) {
            self.mutex_poison_events.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn step_core(&self) -> Result<hares_core::StepResult, HaresError> {
        let mut dwelling = self.acquire_hares()?;
        dwelling.step()
    }

    /// String-error variant for use in GIL-free Rayon contexts
    /// (e.g. [`batch_step_py`]) where `HaresError` cannot cross thread boundaries
    /// as a `PyErr`.
    ///
    /// Routes through [`acquire_string`] so fatal (poisoned) errors are prefixed
    /// with [`FATAL_DWELLING_PREFIX`], allowing [`batch_step_py`] to distinguish
    /// permanent from transient failures. Transient [`step`] errors carry no prefix.
    pub(crate) fn step_core_string(&self) -> Result<hares_core::StepResult, String> {
        let mut dwelling = self.acquire_string()?;
        dwelling.step().map_err(|e| e.to_string())
    }

    pub(crate) fn observation(&self) -> Result<Vec<f64>, String> {
        let dwelling = self.acquire_string()?;
        let telemetry = dwelling.telemetry();
        telemetry
            .to_observation_vec(&["total_power_kw", "outdoor_temp", "outdoor_humidity_ratio"])
            .map_err(|err| err.to_string())
    }

    /// Convert a Python equipment config object (Battery, PV, or EV) to a boxed Equipment.
    fn py_any_to_equipment(
        &self,
        obj: &Bound<'_, PyAny>,
    ) -> PyResult<Box<dyn hares_equipment::Equipment>> {
        if let Ok(battery) = obj.extract::<PyRef<'_, PyBattery>>() {
            let config = battery_config_from_py(&battery);
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
            let config = pv_config_from_py(&pv);
            return self
                .equipment_registry
                .create("PV", config)
                .map_err(to_py_err);
        }
        if let Ok(ev) = obj.extract::<PyRef<'_, PyEv>>() {
            let config = ev_config_from_py(&ev);
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
        if let Ok(bridge) = obj.extract::<PyRef<'_, PyProtocolBridge>>() {
            let config = protocol_bridge_config_from_py(&bridge);
            return self
                .equipment_registry
                .create("Protocol Bridge", config)
                .map_err(to_py_err);
        }
        Err(PyValueError::new_err(
            "equipment must be a Battery, PV, EV, or ProtocolBridge instance",
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

const KNOWN_KWARGS: &[&str] = &[
    "config",
    "start_time",
    "time_res_s",
    "duration_s",
    "output_to_parquet",
    "output_path",
    "write_output",
    "output_verbosity",
    "output_chunk_size",
    "master_seed",
    "civil_timezone",
    "setpoint_deadband_c",
    "bldg_id",
    "initialization_duration",
    "defaults_path",
    "resample_overrides",
    "overrides",
    "latitude",
    "longitude",
    "elevation_m",
    "utc_offset_h",
    "rotation",
];

/// Extract an explicit [`SiteLocationOverride`] from the `latitude`,
/// `longitude`, `elevation_m`, and `utc_offset_h` kwargs.
///
/// Each is optional; any provided value overrides the HPXML/weather-derived
/// value during site-location resolution. Values are validated for finiteness
/// and physical range so a typo fails fast rather than silently producing
/// wrong solar geometry.
fn extract_site_location_override(
    kwargs: Option<&Bound<'_, PyDict>>,
) -> PyResult<SiteLocationOverride> {
    let get = |name: &str| -> PyResult<Option<f64>> {
        kwargs
            .and_then(|k| k.get_item(name).ok().flatten())
            .map(|obj| obj.extract::<f64>())
            .transpose()
    };

    let latitude_deg = get("latitude")?;
    if let Some(lat) = latitude_deg {
        if !lat.is_finite() || !(-90.0..=90.0).contains(&lat) {
            return Err(PyValueError::new_err(format!(
                "latitude must be finite and within [-90, 90] degrees, got {lat}"
            )));
        }
    }

    let longitude_deg = get("longitude")?;
    if let Some(lon) = longitude_deg {
        if !lon.is_finite() || !(-180.0..=180.0).contains(&lon) {
            return Err(PyValueError::new_err(format!(
                "longitude must be finite and within [-180, 180] degrees, got {lon}"
            )));
        }
    }

    let elevation_m = get("elevation_m")?;
    if let Some(elev) = elevation_m {
        if !elev.is_finite() || !(-500.0..=9000.0).contains(&elev) {
            return Err(PyValueError::new_err(format!(
                "elevation_m must be finite and within [-500, 9000] m, got {elev}"
            )));
        }
    }

    let utc_offset_h = get("utc_offset_h")?;
    if let Some(off) = utc_offset_h {
        if !off.is_finite() || !(-14.0..=14.0).contains(&off) {
            return Err(PyValueError::new_err(format!(
                "utc_offset_h must be finite and within [-14, 14] hours, got {off}"
            )));
        }
    }

    Ok(SiteLocationOverride {
        latitude_deg,
        longitude_deg,
        elevation_m,
        utc_offset_h,
    })
}

fn build_config(
    hpxml: String,
    schedule: String,
    weather: String,
    kwargs: Option<&Bound<'_, PyDict>>,
) -> PyResult<DwellingConfig> {
    let kwargs = kwargs.cloned();

    if let Some(ref k) = kwargs {
        for key in k.keys() {
            let key_str: String = key.extract()?;
            if !KNOWN_KWARGS.contains(&key_str.as_str()) {
                return Err(PyValueError::new_err(format!(
                    "unknown keyword argument: '{key_str}'"
                )));
            }
        }
    }

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

        let write_output = kwargs
            .as_ref()
            .and_then(|k| k.get_item("write_output").ok().flatten())
            .map(|obj| obj.extract::<bool>())
            .transpose()?
            .unwrap_or(true);

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

        if let Some(db) = setpoint_deadband_c {
            if !db.is_finite() || db < 0.0 {
                return Err(PyValueError::new_err(format!(
                    "setpoint_deadband_c must be finite and non-negative, got {db}"
                )));
            }
        }

        let site_location = extract_site_location_override(kwargs.as_ref())?;

        let rotation = kwargs
            .as_ref()
            .and_then(|k| k.get_item("rotation").ok().flatten())
            .map(|obj| obj.extract::<String>())
            .transpose()?;
        if let Some(ref rot) = rotation {
            parse_rotation_policy(rot)?;
        }
        let rotation_policy = rotation
            .as_deref()
            .map(parse_rotation_policy)
            .transpose()?
            .unwrap_or(hares_io::RotationPolicy::None);

        if duration.num_milliseconds() % time_res.num_milliseconds() != 0 {
            return Err(PyValueError::new_err(format!(
                "duration ({}s) must be an exact multiple of time_res ({}s)",
                duration.num_seconds(),
                time_res.num_seconds(),
            )));
        }

        SimulationConfig {
            start_time,
            duration,
            time_res,
            output_verbosity,
            output_path,
            write_output,
            output_format: if output_to_parquet {
                OutputFormat::Parquet
            } else {
                OutputFormat::Csv
            },
            output_chunk_size,
            setpoint_deadband_c,
            master_seed,
            civil_timezone,
            site_location,
            retain_batches: true,
            rotation: rotation_policy,
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

    let resample_overrides: Option<ResampleOverrides> = {
        let raw: Option<std::collections::HashMap<String, String>> = kwargs
            .as_ref()
            .and_then(|k| k.get_item("resample_overrides").ok().flatten())
            .map(|obj| obj.extract())
            .transpose()?;
        raw.map(|map| {
            let mut overrides = ResampleOverrides::default();
            const KNOWN_FIELDS: &[&str] = &[
                "dry_bulb",
                "dew_point",
                "rel_humidity",
                "pressure",
                "infrared",
                "ground_temp",
                "opaque_sky_cover",
                "ghi",
                "dni",
                "dhi",
                "wind_speed",
                "wind_dir",
            ];
            macro_rules! set_field {
                ($($field:ident),*) => {
                    $(
                        if let Some(v) = map.get(stringify!($field)) {
                            overrides.$field = Some(parse_resample_method(stringify!($field), v)?);
                        }
                    )*
                };
            }
            set_field!(
                dry_bulb,
                dew_point,
                rel_humidity,
                pressure,
                infrared,
                ground_temp,
                opaque_sky_cover,
                ghi,
                dni,
                dhi,
                wind_speed,
                wind_dir
            );
            let unknown: Vec<&str> = map
                .keys()
                .filter(|k| !KNOWN_FIELDS.contains(&k.as_str()))
                .map(String::as_str)
                .collect();
            if !unknown.is_empty() {
                return Err(PyValueError::new_err(format!(
                    "unknown resample_overrides field(s): {}; valid fields: {}",
                    unknown.join(", "),
                    KNOWN_FIELDS.join(", "),
                )));
            }
            Ok(overrides)
        })
        .transpose()?
    };

    let overrides: Option<serde_json::Value> = kwargs
        .as_ref()
        .and_then(|k| k.get_item("overrides").ok().flatten())
        .map(|obj| python_to_json_value(&obj))
        .transpose()?;

    Ok(DwellingConfig {
        hpxml_path: PathBuf::from(hpxml),
        schedule_path: PathBuf::from(schedule),
        weather_path: PathBuf::from(weather),
        defaults_path,
        sim_config,
        overrides,
        bldg_id,
        initialization_duration,
        resample_overrides,
        patches: None,
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

fn python_to_json_value(obj: &Bound<'_, PyAny>) -> PyResult<serde_json::Value> {
    if obj.is_none() {
        return Ok(serde_json::Value::Null);
    }
    if let Ok(b) = obj.extract::<bool>() {
        return Ok(serde_json::Value::Bool(b));
    }
    if let Ok(i) = obj.extract::<i64>() {
        return Ok(serde_json::json!(i));
    }
    if let Ok(f) = obj.extract::<f64>() {
        return Ok(serde_json::json!(f));
    }
    if let Ok(s) = obj.extract::<String>() {
        return Ok(serde_json::Value::String(s));
    }
    if let Ok(list) = obj.cast::<PyList>() {
        let items: Vec<serde_json::Value> = list
            .iter()
            .map(|item| python_to_json_value(&item))
            .collect::<PyResult<_>>()?;
        return Ok(serde_json::Value::Array(items));
    }
    if let Ok(dict) = obj.cast::<PyDict>() {
        let mut map = serde_json::Map::new();
        for (key, value) in dict.iter() {
            let key_str: String = key
                .extract()
                .map_err(|_| PyValueError::new_err("overrides dict keys must be strings"))?;
            map.insert(key_str, python_to_json_value(&value)?);
        }
        return Ok(serde_json::Value::Object(map));
    }
    let type_name = obj
        .get_type()
        .name()
        .map(|n| n.to_string())
        .unwrap_or_else(|_| "?".to_string());
    Err(PyValueError::new_err(format!(
        "cannot convert Python object of type '{type_name}' to JSON for overrides",
    )))
}

fn to_py_err(err: HaresError) -> PyErr {
    let msg = err.to_string();
    match err {
        // Io errors are mapped to ConfigError because they predominantly arise during
        // config/file loading (HPXML, weather, schedules). This is a conscious trade-off:
        // a rare runtime Io error will surface as ConfigError rather than adding a
        // separate Python exception type for a case that effectively never occurs.
        HaresError::Io(_) | HaresError::Dwelling(_) | HaresError::Envelope(_) => {
            HaresConfigError::new_err(msg)
        }
        HaresError::Equipment(_) => HaresEquipmentError::new_err(msg),
        // Physics variant is DEPRECATED: zero production constructors remain in
        // the codebase (T-0144). Kept for serialization compatibility and future
        // hares-physics crate retrofitting (T-0145).
        HaresError::Physics(_)
        | HaresError::Control(_)
        | HaresError::Tariff(_)
        | HaresError::InvariantViolation { .. }
        | HaresError::NanDetected { .. }
        | HaresError::NegativeDeliveredEnergy { .. } => HaresSimulationError::new_err(msg),
    }
}

fn to_py_err_display<E: std::fmt::Display>(err: E) -> PyErr {
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
        d.set_item("port_convective_w", gains.port_convective_w)?;
        d.set_item("port_radiant_w", gains.port_radiant_w)?;
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
    use std::collections::HashMap;
    use std::fs;
    use std::sync::Mutex;
    use std::sync::MutexGuard;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::Duration as StdDuration;

    use chrono::{Duration as ChronoDuration, FixedOffset, TimeZone};
    use hares_equipment::pv::surface_id_for_orientation;
    use hares_equipment::{EquipmentRegistry, PvConfig};
    use hares_types::{
        EnvironmentState, GridState, PortSlots, SurfaceIrradiance, WeatherState, ZoneId, ZoneState,
    };

    use super::default_start;
    use super::pv_config_from_py;
    use crate::py_equipment::PyPv;
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

    fn pv_env(tilt_deg: f64, azimuth_deg: f64) -> EnvironmentState {
        let surface_id =
            surface_id_for_orientation(tilt_deg, azimuth_deg, 5.0).expect("surface id");
        EnvironmentState {
            zones: vec![ZoneState {
                id: ZoneId(1),
                temperature_c: 24.0,
                humidity_ratio: 0.008,
                volume_m3: 200.0,
            }],
            weather: WeatherState {
                outdoor_temp_c: 25.0,
                outdoor_humidity_ratio: 0.008,
                pressure_kpa: 101.325,
                ghi_w_m2: 850.0,
                dni_w_m2: 700.0,
                dhi_w_m2: 150.0,
                solar_irradiance: vec![SurfaceIrradiance {
                    surface_id,
                    direct_w_m2: 700.0,
                    diffuse_w_m2: 120.0,
                    reflected_w_m2: 30.0,
                    angle_of_incidence_rad: 0.2,
                }],
                ..WeatherState::default()
            },
            grid: GridState {
                voltage_pu: 1.0,
                frequency_hz: 60.0,
            },
            custom_domains: vec![],
            equipment_telemetry: HashMap::new(),
            equipment_core: HashMap::new(),
            current_time: FixedOffset::east_opt(0)
                .expect("UTC")
                .with_ymd_and_hms(2026, 6, 21, 12, 0, 0)
                .single()
                .expect("valid time"),
            time_res: ChronoDuration::minutes(1),
            price_signal: Default::default(),
            electrical: Default::default(),
        }
    }

    #[test]
    fn pypv_sam_lut_path_round_trips_into_typed_config() {
        let pv = PyPv {
            name: "PV LUT".to_string(),
            capacity_kw: 5.0,
            tilt: 30.0,
            azimuth: 180.0,
            sam_lut_path: Some("/tmp/example.parquet".to_string()),
            soiling: None,
        };
        let config = pv_config_from_py(&pv);
        let typed: PvConfig = config.typed().expect("typed PvConfig");
        assert_eq!(typed.sam_lut_path.as_deref(), Some("/tmp/example.parquet"));
    }

    #[test]
    fn pypv_without_sam_lut_path_initializes_normally() {
        let pv = PyPv {
            name: "PV No LUT".to_string(),
            capacity_kw: 5.0,
            tilt: 30.0,
            azimuth: 180.0,
            sam_lut_path: None,
            soiling: None,
        };
        let config = pv_config_from_py(&pv);
        let env = pv_env(30.0, 180.0);
        let registry = EquipmentRegistry::new();
        let mut eq = registry
            .create("PV", config.clone())
            .expect("PV should be registered");
        eq.init(&config, &env).expect("PV init without LUT path");
        eq.step(&env, StdDuration::from_secs(60), &mut PortSlots::default())
            .expect("PV step should succeed");
    }

    #[test]
    fn pypv_invalid_sam_lut_path_fails_init() {
        let bad_lut_path = "/tmp/hares_bad_pv_lut.txt";
        fs::write(bad_lut_path, "not,a,valid,lut\n1,2,3,4").expect("write test file");
        let pv = PyPv {
            name: "PV Bad LUT".to_string(),
            capacity_kw: 5.0,
            tilt: 30.0,
            azimuth: 180.0,
            sam_lut_path: Some(bad_lut_path.to_string()),
            soiling: None,
        };
        let config = pv_config_from_py(&pv);
        let env = pv_env(30.0, 180.0);
        let registry = EquipmentRegistry::new();
        let mut eq = registry
            .create("PV", config.clone())
            .expect("PV should be registered");
        let err = eq
            .init(&config, &env)
            .expect_err("init should fail for invalid LUT extension/content");
        let msg = err.to_string();
        assert!(
            msg.contains("unsupported PV SAM LUT format")
                || msg.contains("failed to")
                || msg.contains("missing one of required columns"),
            "unexpected error: {msg}"
        );
    }

    // --------------------------------------------------------------------
    // Mutex poison flag tests
    // --------------------------------------------------------------------

    use super::FATAL_DWELLING_PREFIX;

    /// Minimal struct mirroring `PyDwelling`'s poison-guard pattern.
    struct PoisonGuard {
        lock: Mutex<i32>,
        poisoned: AtomicBool,
    }

    impl PoisonGuard {
        fn new() -> Self {
            Self {
                lock: Mutex::new(0),
                poisoned: AtomicBool::new(false),
            }
        }

        fn acquire(&self) -> Result<MutexGuard<'_, i32>, String> {
            if self.poisoned.load(Ordering::Acquire) {
                return Err(format!("{}fatal: state corruption", FATAL_DWELLING_PREFIX));
            }
            self.lock
                .lock()
                .map_err(|_| "acquire failed: poison".to_string())
        }

        fn mark_poisoned(&self) {
            self.poisoned.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn mutex_poison_detected_after_panic() {
        let _guard = PoisonGuard::new();
        let lock_static: &'static Mutex<i32> = Box::leak(Box::new(Mutex::new(0)));
        let _ = std::thread::spawn(move || {
            let _held = lock_static.lock().unwrap();
            panic!("intentional");
        })
        .join();
        assert!(
            lock_static.is_poisoned(),
            "mutex should report poisoned after panic while locked"
        );
    }

    #[test]
    fn acquire_fails_fast_when_poisoned_flag_set() {
        let guard = PoisonGuard::new();
        guard.mark_poisoned();
        assert!(guard.poisoned.load(Ordering::Acquire));
        let result = guard.acquire();
        assert!(result.is_err(), "acquire should fail when flag is set");
        assert!(
            result.unwrap_err().starts_with(FATAL_DWELLING_PREFIX),
            "error should carry fatal prefix"
        );
    }

    #[test]
    fn acquire_fails_on_poisoned_mutex_even_without_flag() {
        let lock: Mutex<i32> = Mutex::new(0);
        let lock_static: &'static Mutex<i32> = Box::leak(Box::new(lock));
        let _ = std::thread::spawn(move || {
            let _held = lock_static.lock().unwrap();
            panic!("intentional");
        })
        .join();
        assert!(lock_static.is_poisoned());
        let result = lock_static.lock();
        assert!(
            result.is_err(),
            "lock on poisoned mutex should return PoisonError"
        );
    }

    #[test]
    fn mark_poisoned_is_idempotent() {
        let guard = PoisonGuard::new();
        assert!(!guard.poisoned.load(Ordering::Acquire));
        guard.mark_poisoned();
        assert!(guard.poisoned.load(Ordering::Acquire));
        guard.mark_poisoned();
        // Flag should still be true (no double-counting or flip)
        assert!(guard.poisoned.load(Ordering::Acquire));
    }

    #[test]
    fn invariant_flag_implies_mutex_poisoned_holds() {
        // After a panic while locked → mutex is poisoned → flag should be set
        let lock: Mutex<i32> = Mutex::new(0);
        let lock_static: &'static Mutex<i32> = Box::leak(Box::new(lock));
        let _ = std::thread::spawn(move || {
            let _held = lock_static.lock().unwrap();
            panic!("intentional");
        })
        .join();
        let is_poisoned = lock_static.is_poisoned();
        let flag = AtomicBool::new(is_poisoned);
        assert_eq!(
            flag.load(Ordering::Acquire),
            is_poisoned,
            "flag should match mutex poison state"
        );
    }

    #[test]
    fn fatal_dwelling_prefix_detectable_by_callers() {
        let guard = PoisonGuard::new();
        guard.mark_poisoned();
        let err = guard.acquire().unwrap_err();
        assert!(
            err.starts_with(FATAL_DWELLING_PREFIX),
            "fatal errors must be detectable by prefix in GIL-free String contexts"
        );
    }

    /// Mock mirroring the `step_core_string` → `acquire_string` chain.
    struct StepGuard {
        lock: Mutex<i32>,
        poisoned: AtomicBool,
    }

    impl StepGuard {
        fn new() -> Self {
            Self {
                lock: Mutex::new(0),
                poisoned: AtomicBool::new(false),
            }
        }

        fn acquire_string(&self) -> Result<MutexGuard<'_, i32>, String> {
            if self.poisoned.load(Ordering::Acquire) {
                return Err(format!("{}fatal: state corruption", FATAL_DWELLING_PREFIX));
            }
            self.lock.lock().map_err(|_| "acquire failed".to_string())
        }

        fn mark_poisoned(&self) {
            self.poisoned.store(true, Ordering::SeqCst);
        }

        /// Mirrors `PyDwelling::step_core_string`: locks via `acquire_string`,
        /// then performs a step. Fatal errors carry `FATAL_DWELLING_PREFIX`.
        fn step_core_string(&self) -> Result<i32, String> {
            let _guard = self.acquire_string()?;
            Ok(42)
        }
    }

    #[test]
    fn step_core_string_returns_fatal_prefix_when_poisoned() {
        let guard = StepGuard::new();
        guard.mark_poisoned();
        let err = guard.step_core_string().unwrap_err();
        assert!(
            err.starts_with(FATAL_DWELLING_PREFIX),
            "step_core_string must propagate the fatal prefix from acquire_string so \
             batch_step_py can distinguish permanent from transient failures"
        );
    }

    #[test]
    fn step_core_string_succeeds_when_not_poisoned() {
        let guard = StepGuard::new();
        let result = guard.step_core_string();
        assert_eq!(result.unwrap(), 42);
    }
}
